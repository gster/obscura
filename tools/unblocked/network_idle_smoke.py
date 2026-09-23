#!/usr/bin/env python3
"""Qualify truthful network-idle through official Playwright Python."""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.metadata
import json
import platform
import subprocess
import sys
import threading
import time
import traceback
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Iterator

if __package__:
    from .cdp_fixture import external_process, free_port
else:
    from cdp_fixture import external_process, free_port


class FixtureServer(ThreadingHTTPServer):
    daemon_threads = True


class FixtureCapture:
    def __init__(self, path: Path):
        self.path = path
        self.lock = threading.Lock()

    def write(self, value: dict[str, Any]) -> None:
        encoded = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
        with self.lock, self.path.open("a", encoding="utf-8") as output:
            output.write(encoded + "\n")


class FixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "ObscuraNetworkIdleFixture/1"
    sys_version = ""

    def log_message(self, format: str, *args: Any) -> None:
        return

    def do_GET(self) -> None:
        started_wall_ns = time.time_ns()
        started_monotonic_ns = time.monotonic_ns()
        capture = getattr(self.server, "capture")
        capture.write(
            {
                "kind": "request",
                "wallTimeNs": started_wall_ns,
                "monotonicTimeNs": started_monotonic_ns,
                "method": self.command,
                "target": self.path,
                "requestVersion": self.request_version,
                "headers": list(self.headers.raw_items()),
                "bodyBase64": "",
                "bodyLength": 0,
                "clientAddress": list(self.client_address),
            }
        )
        path = self.path.split("?", 1)[0]
        if path == "/static":
            self.respond(200, "text/html", b"<title>static</title><p>ready</p>")
            return
        if path == "/delayed":
            body = (
                b"<title>delayed</title><body><script>"
                b"fetch('/hold').then(r=>r.text()).then(v=>document.body.dataset.done=v)"
                b"</script></body>"
            )
            self.respond(200, "text/html", body)
            return
        if path == "/hold":
            time.sleep(1.5)
            self.respond(200, "text/plain", b"held-complete")
            return
        if path == "/never":
            body = (
                b"<title>never</title><body><script>"
                b"fetch('/hang').catch(()=>{})"
                b"</script></body>"
            )
            self.respond(200, "text/html", body)
            return
        if path == "/document-open":
            body = (
                b"<title>before-open</title><body><script>"
                b"const end=Date.now()+450;while(Date.now()<end){};"
                b"document.open();document.write('<title>after-open</title>"
                b"<body id=done>replacement</body>');document.close();"
                b"globalThis.__openedWall=Date.now();globalThis.__opened=true"
                b"</script></body>"
            )
            self.respond(200, "text/html", body)
            return
        if path == "/base":
            self.respond(200, "text/html", b"<title>base</title><body id=base>preserved</body>")
            return
        if path == "/no-content":
            self.respond(204, "text/plain", b"")
            return
        if path == "/hang":
            getattr(self.server, "release_hang").wait(15)
            try:
                self.respond(200, "text/plain", b"released")
            except (BrokenPipeError, ConnectionResetError) as error:
                capture.write(
                    {
                        "kind": "responseError",
                        "wallTimeNs": time.time_ns(),
                        "monotonicTimeNs": time.monotonic_ns(),
                        "target": self.path,
                        "error": {
                            "type": type(error).__name__,
                            "message": str(error),
                        },
                    }
                )
            return
        self.respond(404, "text/plain", b"missing")

    def respond(self, status: int, content_type: str, body: bytes) -> None:
        headers = [
            ("Server", self.version_string()),
            ("Date", self.date_time_string()),
            ("Content-Type", content_type),
            ("Content-Length", str(len(body))),
            ("Cache-Control", "no-store"),
            ("Connection", "close"),
        ]
        self.send_response_only(status)
        for name, value in headers:
            self.send_header(name, value)
        self.end_headers()
        if body:
            self.wfile.write(body)
        getattr(self.server, "capture").write(
            {
                "kind": "response",
                "wallTimeNs": time.time_ns(),
                "monotonicTimeNs": time.monotonic_ns(),
                "target": self.path,
                "status": status,
                "headers": headers,
                "bodyBase64": base64.b64encode(body).decode("ascii"),
                "bodyLength": len(body),
            }
        )


@contextmanager
def fixture_server(capture_path: Path) -> Iterator[str]:
    server = FixtureServer(("127.0.0.1", 0), FixtureHandler)
    server.capture = FixtureCapture(capture_path)  # type: ignore[attr-defined]
    server.release_hang = threading.Event()  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        host, port = server.server_address
        yield f"http://{host}:{port}"
    finally:
        server.release_hang.set()  # type: ignore[attr-defined]
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_revision() -> str | None:
    completed = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=Path(__file__).resolve().parents[2],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    if completed.returncode != 0:
        return None
    return completed.stdout.decode("utf-8", errors="surrogateescape").strip()


def record_event(events: list[dict[str, Any]], kind: str, value: Any) -> None:
    events.append(
        {
            "kind": kind,
            "wallTimeNs": time.time_ns(),
            "monotonicTimeNs": time.monotonic_ns(),
            "value": value,
        }
    )


def attach_page_events(page: Any, session: Any, events: list[dict[str, Any]]) -> None:
    page.on(
        "request",
        lambda request: record_event(
            events,
            "playwright.request",
            {
                "url": request.url,
                "method": request.method,
                "headers": request.headers,
                "postData": request.post_data,
            },
        ),
    )
    page.on(
        "response",
        lambda response: record_event(
            events,
            "playwright.response",
            {
                "url": response.url,
                "status": response.status,
                "headers": response.headers,
            },
        ),
    )
    page.on(
        "requestfinished",
        lambda request: record_event(
            events,
            "playwright.requestfinished",
            {"url": request.url, "method": request.method},
        ),
    )
    page.on(
        "requestfailed",
        lambda request: record_event(
            events,
            "playwright.requestfailed",
            {
                "url": request.url,
                "method": request.method,
                "failure": request.failure,
            },
        ),
    )
    methods = (
        "Network.requestWillBeSent",
        "Network.requestWillBeSentExtraInfo",
        "Network.responseReceived",
        "Network.responseReceivedExtraInfo",
        "Network.loadingFinished",
        "Network.loadingFailed",
        "Page.domContentEventFired",
        "Page.loadEventFired",
        "Page.lifecycleEvent",
        "Page.frameStoppedLoading",
    )
    for method in methods:
        session.on(
            method,
            lambda params, event_method=method: record_event(
                events, f"cdp.{event_method}", params
            ),
        )
    session.send("Network.enable")
    session.send("Page.setLifecycleEventsEnabled", {"enabled": True})


def run_probe(obscura_bin: Path, output: Path, result: dict[str, Any]) -> None:
    from playwright.sync_api import TimeoutError as PlaywrightTimeoutError
    from playwright.sync_api import sync_playwright

    port = free_port()
    endpoint = f"http://127.0.0.1:{port}"
    command = [
        str(obscura_bin.resolve()),
        "--allow-private-network",
        "serve",
        "--host",
        "127.0.0.1",
        "--port",
        str(port),
        "--persona",
        "windows_chrome145",
    ]
    result["endpoint"] = endpoint
    result["serverCommand"] = command
    result["processCapture"] = {}
    with fixture_server(output / "http.jsonl") as fixture_origin, external_process(
        command,
        endpoint,
        capture=result["processCapture"],
        log_root=output,
    ):
        result["fixtureOrigin"] = fixture_origin
        with sync_playwright() as playwright:
            browser = playwright.chromium.connect_over_cdp(endpoint)
            result["browserVersion"] = browser.version
            context = browser.contexts[0]
            try:
                for name, path, timeout_ms in (
                    ("static", "/static", 4000),
                    ("delayed", "/delayed", 5000),
                    ("never", "/never", 1000),
                    ("document_open", "/document-open", 4000),
                ):
                    page = context.new_page()
                    events: list[dict[str, Any]] = []
                    session = context.new_cdp_session(page)
                    attach_page_events(page, session, events)
                    started_wall_ns = time.time_ns()
                    started_monotonic_ns = time.monotonic_ns()
                    timed_out = False
                    error = None
                    try:
                        page.goto(
                            fixture_origin + path,
                            wait_until="networkidle",
                            timeout=timeout_ms,
                        )
                    except PlaywrightTimeoutError as caught:
                        timed_out = True
                        error = {
                            "type": type(caught).__name__,
                            "message": str(caught),
                            "traceback": traceback.format_exc(),
                        }
                    ended_wall_ns = time.time_ns()
                    ended_monotonic_ns = time.monotonic_ns()
                    post_goto_observation_ms = 0
                    if name == "document_open":
                        # Playwright's client-side networkidle waiter can return
                        # before the server's replacement-document lifecycle
                        # candidate completes. Keep the page alive long enough
                        # to observe that independent CDP evidence.
                        post_goto_observation_ms = 750
                        page.wait_for_timeout(post_goto_observation_ms)
                    observed_until_wall_ns = time.time_ns()
                    observed_until_monotonic_ns = time.monotonic_ns()
                    case = {
                        "name": name,
                        "path": path,
                        "timeoutMs": timeout_ms,
                        "startedWallTimeNs": started_wall_ns,
                        "startedMonotonicTimeNs": started_monotonic_ns,
                        "endedWallTimeNs": ended_wall_ns,
                        "endedMonotonicTimeNs": ended_monotonic_ns,
                        "elapsedMs": (
                            ended_monotonic_ns - started_monotonic_ns
                        )
                        / 1_000_000,
                        "postGotoObservationMs": post_goto_observation_ms,
                        "observedUntilWallTimeNs": observed_until_wall_ns,
                        "observedUntilMonotonicTimeNs": observed_until_monotonic_ns,
                        "timedOut": timed_out,
                        "error": error,
                        "url": page.url,
                        "title": page.title(),
                        "evaluation": page.evaluate(
                            "({title:document.title,readyState:document.readyState,"
                            "bodyId:document.body.id,bodyText:document.body.textContent,"
                            "opened:globalThis.__opened===true,"
                            "openedWall:globalThis.__openedWall??null,"
                            "bodyDataset:{...document.body.dataset}})"
                        ),
                        "events": events,
                    }
                    result["cases"].append(case)
                    page.close()
                    try:
                        session.detach()
                    except Exception as error:
                        record_event(
                            events,
                            "cdp.detach.error",
                            {
                                "type": type(error).__name__,
                                "message": str(error),
                                "traceback": traceback.format_exc(),
                            },
                        )
                page = context.new_page()
                page.goto(fixture_origin + "/base", wait_until="load", timeout=4000)
                events = []
                session = context.new_cdp_session(page)
                attach_page_events(page, session, events)
                started_wall_ns = time.time_ns()
                started_monotonic_ns = time.monotonic_ns()
                error = None
                try:
                    page.goto(
                        fixture_origin + "/no-content",
                        wait_until="networkidle",
                        timeout=4000,
                    )
                except Exception as caught:
                    error = {
                        "type": type(caught).__name__,
                        "message": str(caught),
                        "traceback": traceback.format_exc(),
                    }
                ended_wall_ns = time.time_ns()
                ended_monotonic_ns = time.monotonic_ns()
                # The CDP frameStoppedLoading notification is sent before the
                # Page.navigate response, but Playwright dispatches its public
                # goto rejection before every raw-session callback has run.
                # Drain that already-received event without changing the
                # measured navigation duration.
                post_goto_observation_ms = 100
                page.wait_for_timeout(post_goto_observation_ms)
                observed_until_wall_ns = time.time_ns()
                observed_until_monotonic_ns = time.monotonic_ns()
                result["cases"].append(
                    {
                        "name": "no_content",
                        "path": "/no-content",
                        "timeoutMs": 4000,
                        "startedWallTimeNs": started_wall_ns,
                        "startedMonotonicTimeNs": started_monotonic_ns,
                        "endedWallTimeNs": ended_wall_ns,
                        "endedMonotonicTimeNs": ended_monotonic_ns,
                        "elapsedMs": (
                            ended_monotonic_ns - started_monotonic_ns
                        )
                        / 1_000_000,
                        "postGotoObservationMs": post_goto_observation_ms,
                        "observedUntilWallTimeNs": observed_until_wall_ns,
                        "observedUntilMonotonicTimeNs": observed_until_monotonic_ns,
                        "timedOut": error is not None
                        and error["type"] == "TimeoutError",
                        "error": error,
                        "url": page.url,
                        "title": page.title(),
                        "evaluation": page.evaluate(
                            "({title:document.title,readyState:document.readyState,"
                            "bodyId:document.body.id,bodyText:document.body.textContent})"
                        ),
                        "events": events,
                    }
                )
                page.close()
                try:
                    session.detach()
                except Exception as error:
                    record_event(
                        events,
                        "cdp.detach.error",
                        {
                            "type": type(error).__name__,
                            "message": str(error),
                            "traceback": traceback.format_exc(),
                        },
                    )
            finally:
                browser.close()


def cdp_request(case: dict[str, Any], url: str) -> dict[str, Any]:
    for event in case["events"]:
        if event["kind"] != "cdp.Network.requestWillBeSent":
            continue
        value = event["value"]
        if value.get("request", {}).get("url") == url:
            return event
    raise AssertionError(f"missing CDP request start for {url}: {case!r}")


def cdp_terminal(case: dict[str, Any], request_id: str) -> dict[str, Any]:
    for event in case["events"]:
        if event["kind"] not in {
            "cdp.Network.loadingFinished",
            "cdp.Network.loadingFailed",
        }:
            continue
        if event["value"].get("requestId") == request_id:
            return event
    raise AssertionError(f"missing CDP terminal for {request_id}: {case!r}")


def loader_lifecycle(case: dict[str, Any], loader_id: str) -> list[dict[str, Any]]:
    return [
        event
        for event in case["events"]
        if event["kind"] == "cdp.Page.lifecycleEvent"
        and event["value"].get("loaderId") == loader_id
    ]


def verify(result: dict[str, Any]) -> None:
    cases = {case["name"]: case for case in result["cases"]}
    if result["playwrightVersion"] != "1.60.0":
        raise AssertionError(
            f"unexpected Playwright version: {result['playwrightVersion']!r}"
        )
    static = cases["static"]
    delayed = cases["delayed"]
    never = cases["never"]
    document_open = cases["document_open"]
    no_content = cases["no_content"]
    if static["timedOut"] or static["elapsedMs"] < 450:
        raise AssertionError(f"static network-idle was not truthful: {static!r}")
    if delayed["timedOut"] or delayed["elapsedMs"] < 1900:
        raise AssertionError(f"delayed network-idle returned too early: {delayed!r}")
    if delayed["evaluation"]["bodyDataset"].get("done") != "held-complete":
        raise AssertionError(f"delayed fetch did not complete: {delayed!r}")
    if not never["timedOut"] or never["elapsedMs"] < 900:
        raise AssertionError(f"held fetch produced false network-idle: {never!r}")
    if never["evaluation"] != {
        "title": "never",
        "readyState": "complete",
        "bodyId": "",
        "bodyText": "fetch('/hang').catch(()=>{})",
        "opened": False,
        "openedWall": None,
        "bodyDataset": {},
    }:
        raise AssertionError(f"timed-out page was not usable: {never!r}")
    if document_open["timedOut"] or document_open["elapsedMs"] < 450:
        raise AssertionError(
            f"document.open navigation did not observe network-idle: {document_open!r}"
        )
    if not document_open["evaluation"]["opened"]:
        raise AssertionError(f"document.open callback did not execute: {document_open!r}")
    if no_content["timedOut"] or no_content["error"] is None:
        raise AssertionError(f"204 navigation did not abort immediately: {no_content!r}")
    if no_content["error"]["type"] != "Error":
        raise AssertionError(f"204 returned the wrong Playwright error class: {no_content!r}")
    if "net::ERR_ABORTED" not in no_content["error"]["message"]:
        raise AssertionError(f"204 errorText was not preserved: {no_content!r}")
    if no_content["url"] != result["fixtureOrigin"] + "/base" or no_content[
        "evaluation"
    ] != {
        "title": "base",
        "readyState": "complete",
        "bodyId": "base",
        "bodyText": "preserved",
    }:
        raise AssertionError(f"204 replaced the old document: {no_content!r}")

    static_start = cdp_request(static, result["fixtureOrigin"] + "/static")
    static_lifecycle = loader_lifecycle(static, static_start["value"]["loaderId"])
    if not {"networkAlmostIdle", "networkIdle"}.issubset(
        {event["value"].get("name") for event in static_lifecycle}
    ):
        raise AssertionError(f"explicit CDP session missed static idle: {static!r}")

    delayed_start = cdp_request(delayed, result["fixtureOrigin"] + "/delayed")
    delayed_lifecycle = loader_lifecycle(delayed, delayed_start["value"]["loaderId"])
    delayed_by_name = {
        event["value"].get("name"): event for event in delayed_lifecycle
    }
    held_start = cdp_request(delayed, result["fixtureOrigin"] + "/hold")
    held_terminal = cdp_terminal(delayed, held_start["value"]["requestId"])
    almost = delayed_by_name.get("networkAlmostIdle")
    idle = delayed_by_name.get("networkIdle")
    if almost is None or idle is None:
        raise AssertionError(f"explicit CDP session missed delayed idle: {delayed!r}")
    if almost["monotonicTimeNs"] >= held_terminal["monotonicTimeNs"]:
        raise AssertionError(f"networkAlmostIdle did not precede held terminal: {delayed!r}")
    if idle["monotonicTimeNs"] - held_terminal["monotonicTimeNs"] < 450_000_000:
        raise AssertionError(f"networkIdle did not wait 500ms after terminal: {delayed!r}")

    never_start = cdp_request(never, result["fixtureOrigin"] + "/never")
    never_names = {
        event["value"].get("name")
        for event in loader_lifecycle(never, never_start["value"]["loaderId"])
    }
    if "networkAlmostIdle" not in never_names or "networkIdle" in never_names:
        raise AssertionError(f"held request lifecycle was not truthful: {never!r}")

    document_open_start = cdp_request(
        document_open, result["fixtureOrigin"] + "/document-open"
    )
    document_open_by_name = {
        event["value"].get("name"): event
        for event in loader_lifecycle(
            document_open, document_open_start["value"]["loaderId"]
        )
    }
    document_open_almost = document_open_by_name.get("networkAlmostIdle")
    document_open_idle = document_open_by_name.get("networkIdle")
    if document_open_almost is None or document_open_idle is None:
        raise AssertionError(
            f"explicit CDP session missed replacement idle: {document_open!r}"
        )
    opened_wall = document_open["evaluation"].get("openedWall")
    if not isinstance(opened_wall, (int, float)):
        raise AssertionError(f"document.open wall clock was not retained: {document_open!r}")
    if document_open_idle["wallTimeNs"] - int(opened_wall * 1_000_000) < 450_000_000:
        raise AssertionError(
            f"replacement lifecycle did not wait a fresh quiet window: {document_open!r}"
        )

    no_content_url = result["fixtureOrigin"] + "/no-content"
    no_content_starts = [
        event for event in no_content["events"]
        if event["kind"] == "cdp.Network.requestWillBeSent"
        and event["value"].get("request", {}).get("url") == no_content_url
    ]
    if len(no_content_starts) != 1:
        raise AssertionError(f"204 must have exactly one request start: {no_content!r}")
    no_content_start = no_content_starts[0]
    attempted_id = no_content_start["value"]["requestId"]
    no_content_terminals = [
        event for event in no_content["events"]
        if event["kind"] in {
            "cdp.Network.loadingFinished", "cdp.Network.loadingFailed"
        }
        and event["value"].get("requestId") == attempted_id
    ]
    if len(no_content_terminals) != 1:
        raise AssertionError(f"204 must have exactly one request terminal: {no_content!r}")
    no_content_terminal = no_content_terminals[0]
    if no_content_terminal["kind"] != "cdp.Network.loadingFailed":
        raise AssertionError(f"204 did not terminate as a failure: {no_content!r}")
    if no_content_terminal["value"].get("errorText") != "net::ERR_ABORTED" or not (
        no_content_terminal["value"].get("canceled")
    ):
        raise AssertionError(f"204 terminal did not retain exact abort data: {no_content!r}")
    no_content_responses = [
        event
        for event in no_content["events"]
        if event["kind"] == "cdp.Network.responseReceived"
        and event["value"].get("requestId")
        == attempted_id
    ]
    if len(no_content_responses) != 1 or no_content_responses[0]["value"].get(
        "loaderId"
    ) != no_content_start["value"]["loaderId"]:
        raise AssertionError(f"204 response did not preserve the attempted loader: {no_content!r}")
    if no_content["title"] != "base":
        raise AssertionError(f"204 changed the Playwright-visible title: {no_content!r}")
    no_content_stops = [
        event for event in no_content["events"]
        if event["kind"] == "cdp.Page.frameStoppedLoading"
    ]
    if len(no_content_stops) != 1:
        raise AssertionError(f"204 must stop the provisional frame exactly once: {no_content!r}")
    response_index = no_content["events"].index(no_content_responses[0])
    terminal_index = no_content["events"].index(no_content_terminal)
    stopped_index = no_content["events"].index(no_content_stops[0])
    if not response_index < terminal_index < stopped_index:
        raise AssertionError(f"204 response/failure/stopped order was wrong: {no_content!r}")
    if loader_lifecycle(no_content, no_content_start["value"]["loaderId"]):
        raise AssertionError(f"204 fabricated lifecycle events: {no_content!r}")


def write_manifest(output: Path) -> None:
    entries = []
    for path in sorted(output.rglob("*")):
        if path.is_file() and path.name != "manifest.json":
            entries.append(
                {
                    "path": str(path.relative_to(output)),
                    "byteLength": path.stat().st_size,
                    "sha256": sha256_file(path),
                }
            )
    (output / "manifest.json").write_text(
        json.dumps({"schemaVersion": 1, "files": entries}, ensure_ascii=False, indent=2)
        + "\n",
        encoding="utf-8",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obscura-bin", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary = args.obscura_bin.resolve()
    result: dict[str, Any] = {
        "schemaVersion": 1,
        "status": "running",
        "gitRevision": git_revision(),
        "platform": platform.platform(),
        "pythonVersion": sys.version,
        "playwrightVersion": importlib.metadata.version("playwright"),
        "connection": "BrowserType.connect_over_cdp",
        "waitUntil": "networkidle",
        "quietWindowMs": 500,
        "persona": "windows_chrome145",
        "binary": {
            "path": str(binary),
            "byteLength": binary.stat().st_size,
            "sha256": sha256_file(binary),
        },
        "cases": [],
    }
    exit_code = 0
    try:
        run_probe(binary, output, result)
        verify(result)
        result["status"] = "passed"
    except BaseException as error:
        exit_code = 1
        result["status"] = "failed"
        result["failure"] = {
            "type": type(error).__name__,
            "message": str(error),
            "traceback": traceback.format_exc(),
        }
    (output / "result.json").write_text(
        json.dumps(result, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    print(json.dumps(result, ensure_ascii=False, indent=2))
    sys.stdout.flush()
    sys.stderr.flush()
    write_manifest(output)
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())

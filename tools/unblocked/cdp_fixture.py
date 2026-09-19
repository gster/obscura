#!/usr/bin/env python3
"""Run the OB-026 fixture through normal Chrome, Chrome CDP, and Obscura CDP."""

from __future__ import annotations

import argparse
import hashlib
import importlib.metadata
import json
import os
import signal
import socket
import subprocess
import tempfile
import threading
import time
import urllib.error
import urllib.request
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Iterator

if __package__:
    from .cdp_trace import Trace, first_divergence
else:
    from cdp_trace import Trace, first_divergence


ROOT = Path(__file__).resolve().parent
FIXTURE = ROOT / "fixtures" / "cdp-minimal.html"
EVENTS = (
    "Network.dataReceived",
    "Network.loadingFailed",
    "Network.loadingFinished",
    "Network.requestWillBeSent",
    "Network.requestWillBeSentExtraInfo",
    "Network.responseReceived",
    "Network.responseReceivedExtraInfo",
    "Page.domContentEventFired",
    "Page.frameNavigated",
    "Page.loadEventFired",
    "Runtime.executionContextCreated",
)


class ProbeFailure(RuntimeError):
    def __init__(self, document: dict[str, Any], error: Exception):
        super().__init__(str(error))
        self.document = document
        self.original = error


class FixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "ObscuraFixture/1"
    sys_version = ""

    def date_time_string(self, timestamp: float | None = None) -> str:
        return "Sat, 19 Sep 2026 00:00:00 GMT"

    def do_GET(self) -> None:
        if self.path.split("?", 1)[0] != "/fixture":
            self.send_error(404)
            return
        body = FIXTURE.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: Any) -> None:
        return


@contextmanager
def fixture_server() -> Iterator[str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), FixtureHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        host, port = server.server_address
        yield f"http://{host}:{port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def free_port() -> int:
    with socket.socket() as candidate:
        candidate.bind(("127.0.0.1", 0))
        return int(candidate.getsockname()[1])


def wait_for_endpoint(endpoint: str, process: subprocess.Popen[Any], timeout: float = 15) -> None:
    deadline = time.monotonic() + timeout
    last_error = "not ready"
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"browser process exited with status {process.returncode}")
        try:
            with urllib.request.urlopen(f"{endpoint}/json/version", timeout=0.25) as response:
                if response.status == 200:
                    return
        except (OSError, urllib.error.URLError) as error:
            last_error = str(error)
        time.sleep(0.05)
    raise TimeoutError(f"CDP endpoint {endpoint} was not ready: {last_error}")


def process_group_exists(process_group: int) -> bool:
    try:
        os.killpg(process_group, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def terminate_process_group(process: subprocess.Popen[Any], timeout: float = 5) -> None:
    process_group = process.pid
    if process_group_exists(process_group):
        try:
            os.killpg(process_group, signal.SIGTERM)
        except ProcessLookupError:
            pass

    deadline = time.monotonic() + timeout
    while process_group_exists(process_group) and time.monotonic() < deadline:
        process.poll()
        time.sleep(0.02)

    if process_group_exists(process_group):
        try:
            os.killpg(process_group, signal.SIGKILL)
        except ProcessLookupError:
            pass
        kill_deadline = time.monotonic() + max(timeout, 1)
        while process_group_exists(process_group) and time.monotonic() < kill_deadline:
            process.poll()
            time.sleep(0.02)

    if process.poll() is None:
        process.wait(timeout=max(timeout, 1))
    if process_group_exists(process_group):
        raise RuntimeError(f"process group {process_group} did not exit")


@contextmanager
def external_process(command: list[str], endpoint: str) -> Iterator[None]:
    process = subprocess.Popen(
        command,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        start_new_session=True,
    )
    try:
        wait_for_endpoint(endpoint, process)
        yield
    finally:
        terminate_process_group(process)


def page_for(browser: Any) -> Any:
    context = browser.contexts[0] if browser.contexts else browser.new_context()
    return context.pages[0] if context.pages else context.new_page()


def record_probe(mode: str, browser: Any, fixture_origin: str) -> dict[str, Any]:
    trace = Trace(mode, fixture_origin)
    try:
        page = page_for(browser)
        session = page.context.new_cdp_session(page)
        trace.observe(session, EVENTS, session_name="page-1")
        document_requests: list[str] = []

        def remember_document_response(params: dict[str, Any]) -> None:
            response = params.get("response", {})
            if params.get("type") == "Document" and response.get("url", "").startswith(
                fixture_origin
            ):
                document_requests.append(params["requestId"])

        session.on("Network.responseReceived", remember_document_response)

        trace.send(session, "Page.enable", session_name="page-1")
        trace.send(session, "Runtime.enable", session_name="page-1")
        trace.send(session, "Network.enable", session_name="page-1")
        trace.send(
            session,
            "Page.navigate",
            {"url": f"{fixture_origin}/fixture?token=fixture-secret"},
            session_name="page-1",
        )
        page.wait_for_function(
            "document.title === 'OB-026 CDP fixture' && document.documentElement.dataset.ready === 'yes'",
            timeout=10_000,
        )
        if not document_requests:
            raise RuntimeError("fixture document response was not observed")
        trace.send(
            session,
            "Network.getResponseBody",
            {"requestId": document_requests[-1]},
            session_name="page-1",
        )
        trace.send(
            session,
            "Runtime.evaluate",
            {
                "expression": "({title:document.title,ready:document.documentElement.dataset.ready,input:document.querySelector('#name').value})",
                "returnByValue": True,
                "awaitPromise": True,
            },
            session_name="page-1",
        )
        trace.send(session, "DOM.getDocument", {"depth": 1}, session_name="page-1")
        return trace.document()
    except Exception as error:
        trace.record_harness_error("record-probe", error)
        raise ProbeFailure(trace.document(), error) from error


def run_mode(playwright: Any, mode: str, fixture_origin: str, obscura_bin: Path) -> dict[str, Any]:
    if mode == "chromium-launch":
        browser = playwright.chromium.launch(headless=True)
        try:
            return record_probe(mode, browser, fixture_origin)
        finally:
            browser.close()

    port = free_port()
    endpoint = f"http://127.0.0.1:{port}"
    if mode == "chromium-cdp":
        with tempfile.TemporaryDirectory(prefix="obscura-chrome-") as profile:
            command = [
                playwright.chromium.executable_path,
                "--headless=new",
                "--disable-gpu",
                "--no-first-run",
                "--no-default-browser-check",
                f"--remote-debugging-port={port}",
                f"--user-data-dir={profile}",
                "about:blank",
            ]
            with external_process(command, endpoint):
                browser = playwright.chromium.connect_over_cdp(endpoint)
                try:
                    return record_probe(mode, browser, fixture_origin)
                finally:
                    browser.close()

    if mode == "obscura-cdp":
        command = [
            str(obscura_bin),
            "--allow-private-network",
            "serve",
            "--host",
            "127.0.0.1",
            "--port",
            str(port),
        ]
        with external_process(command, endpoint):
            browser = playwright.chromium.connect_over_cdp(endpoint)
            try:
                return record_probe(mode, browser, fixture_origin)
            finally:
                browser.close()

    raise ValueError(f"unknown mode: {mode}")


def compare_runs(runs: dict[str, dict[str, Any]]) -> list[dict[str, Any]]:
    comparisons = []
    reference = runs.get("chromium-launch")
    pairs = []
    if reference is not None:
        pairs.extend(
            ("chromium-launch", mode)
            for mode in ("chromium-cdp", "obscura-cdp")
            if mode in runs
        )
    elif "chromium-cdp" in runs and "obscura-cdp" in runs:
        pairs.append(("chromium-cdp", "obscura-cdp"))
    for left_mode, right_mode in pairs:
        left = runs[left_mode]
        candidate = runs[right_mode]
        divergence = first_divergence(left, candidate)
        comparisons.append(
            {
                "left": left_mode,
                "right": right_mode,
                "equal": divergence is None,
                "firstDivergence": divergence,
            }
        )
    return comparisons


def comparison_status(comparisons: list[dict[str, Any]]) -> str:
    if not comparisons:
        return "inconclusive"
    if any(not item["equal"] for item in comparisons):
        return "different"
    return "passed"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--obscura-bin", type=Path, default=Path("target/release/obscura"))
    parser.add_argument(
        "--mode",
        action="append",
        choices=("chromium-launch", "chromium-cdp", "obscura-cdp"),
        dest="modes",
    )
    args = parser.parse_args()
    modes = args.modes or ["chromium-launch", "chromium-cdp", "obscura-cdp"]
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)

    from playwright.sync_api import sync_playwright

    runs: dict[str, dict[str, Any]] = {}
    traces: dict[str, dict[str, Any]] = {}
    failures: dict[str, str] = {}
    with fixture_server() as fixture_origin, sync_playwright() as playwright:
        for mode in modes:
            try:
                document = run_mode(playwright, mode, fixture_origin, args.obscura_bin.resolve())
                runs[mode] = document
                traces[mode] = document
                (output / f"{mode}.json").write_text(
                    json.dumps(document, indent=2, sort_keys=True) + "\n"
                )
            except ProbeFailure as error:
                traces[mode] = error.document
                (output / f"{mode}.json").write_text(
                    json.dumps(error.document, indent=2, sort_keys=True) + "\n"
                )
                failures[mode] = f"{type(error.original).__name__}: {error.original}"
            except Exception as error:
                failures[mode] = f"{type(error).__name__}: {error}"

    comparisons = compare_runs(runs)
    comparison_result = comparison_status(comparisons)
    result = {
        "schemaVersion": 1,
        "fixtureSha256": hashlib.sha256(FIXTURE.read_bytes()).hexdigest(),
        "playwrightVersion": importlib.metadata.version("playwright"),
        "modes": {
            mode: {
                "status": "passed" if mode in runs else "failed",
                "trace": f"{mode}.json" if mode in traces else None,
                "error": failures.get(mode),
            }
            for mode in modes
        },
        "comparisons": comparisons,
        "comparisonStatus": comparison_result,
    }
    (output / "result.json").write_text(json.dumps(result, indent=2, sort_keys=True) + "\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    return 1 if failures or comparison_result != "passed" else 0


if __name__ == "__main__":
    raise SystemExit(main())

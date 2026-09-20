#!/usr/bin/env python3
"""Qualify the official Playwright mouse path against an offline fixture.

The worker is deliberately a child process.  A stuck CDP command must not hold
the parent or hide the browser's raw stdout/stderr, and the protocol stream is
kept byte-for-byte in the worker stderr capture produced by ``capture_worker``.
The default run targets Obscura with an explicit persona.  ``--with-chrome``
adds a Chromium CDP reference run and compares the Obscura observations with
the reference for every implemented case.
"""

from __future__ import annotations

import argparse
import base64
from contextlib import contextmanager
import json
from pathlib import Path
import sys
import tempfile
import threading
from typing import Any, Iterator
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

if __package__:
    from .cdp_fixture import error_record, external_process, free_port
    from .migration_smoke import capture_worker
    from .migration_cors import _WireReader
else:
    from cdp_fixture import error_record, external_process, free_port
    from migration_smoke import capture_worker
    from migration_cors import _WireReader


DEFAULT_PERSONA = "windows_chrome145"
WORKER_TIMEOUT_SECONDS = 45
CLICK_X = 50
CLICK_Y = 40

CORE_EVENT_TYPES = [
    "pointermove",
    "mousemove",
    "pointerdown",
    "mousedown",
    "focus",
    "focusin",
    "pointerup",
    "mouseup",
    "click",
]

EXPECTED_CONSTRUCTORS = {
    "pointermove": "PointerEvent",
    "mousemove": "MouseEvent",
    "pointerdown": "PointerEvent",
    "mousedown": "MouseEvent",
    "focus": "FocusEvent",
    "focusin": "FocusEvent",
    "pointerup": "PointerEvent",
    "mouseup": "MouseEvent",
    "click": "PointerEvent",
    "dblclick": "MouseEvent",
}

# These are the fields where the Chrome reference is stable for this fixture.
# The complete raw records remain in worker-result.json for later inspection.
EVENT_FIELDS = (
    "type",
    "target",
    "constructor",
    "isTrusted",
    "clientX",
    "clientY",
    "button",
    "buttons",
    "detail",
    "pressure",
    "defaultPrevented",
)


HTML = r"""<!doctype html>
<meta charset="utf-8">
<title>OB-021 native mouse fixture</title>
<style>
  html, body { margin: 0; }
  button, textarea { font: 16px sans-serif; }
  #target, #decoy { position: absolute; left: 20px; top: 20px; width: 120px; height: 40px; }
  #decoy { top: 100px; }
  #check { position: absolute; left: 20px; top: 170px; }
  #text { position: absolute; left: 20px; top: 220px; width: 240px; height: 50px; }
</style>
<button id="target">target</button>
<button id="decoy">decoy</button>
<input id="check" type="checkbox">
<textarea id="text">alpha beta gamma</textarea>
<script>
(() => {
  const eventNames = [
    'pointermove', 'mousemove', 'pointerdown', 'mousedown', 'focus', 'focusin',
    'pointerup', 'mouseup', 'click', 'dblclick', 'auxclick', 'contextmenu',
  ];
  const record = (event) => ({
    type: event.type,
    target: event.target && event.target.id,
    constructor: event.constructor && event.constructor.name,
    isTrusted: event.isTrusted,
    clientX: typeof event.clientX === 'number' ? event.clientX : null,
    clientY: typeof event.clientY === 'number' ? event.clientY : null,
    button: typeof event.button === 'number' ? event.button : null,
    buttons: typeof event.buttons === 'number' ? event.buttons : null,
    detail: typeof event.detail === 'number' ? event.detail : null,
    pressure: typeof event.pressure === 'number' ? event.pressure : null,
    altKey: !!event.altKey,
    ctrlKey: !!event.ctrlKey,
    metaKey: !!event.metaKey,
    shiftKey: !!event.shiftKey,
    defaultPrevented: event.defaultPrevented,
  });
  for (const id of ['target', 'decoy', 'check', 'text']) {
    const node = document.getElementById(id);
    for (const name of eventNames) {
      node.addEventListener(name, event => {
        globalThis.__mouseEvents.push(record(event));
        if (name === 'click' && id === 'target') globalThis.__targetClicks += 1;
        if (name === 'click' && id === 'decoy') globalThis.__decoyClicks += 1;
      });
    }
  }
  globalThis.__resetMouseProbe = () => {
    globalThis.__mouseEvents = [];
    globalThis.__targetClicks = 0;
    globalThis.__decoyClicks = 0;
  };
  globalThis.__resetMouseProbe();
})();
</script>
"""


class MouseFixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "ObscuraOB021MouseFixture/1"
    sys_version = ""

    def setup(self) -> None:
        super().setup()
        self.rfile = _WireReader(self.rfile)

    def end_headers(self) -> None:
        self._response_headers = b"".join(self._headers_buffer) + b"\r\n"
        super().end_headers()

    def do_GET(self) -> None:
        path = self.path.split("?", 1)[0]
        content_type = "text/html; charset=utf-8"
        if path != "/mouse":
            body = b"not found"
            status = 404
            content_type = "text/plain; charset=utf-8"
            self.send_response(404)
        else:
            body = HTML.encode("utf-8")
            status = 200
            self.send_response(200)
        wire = self.rfile
        content_length = int(self.headers.get("Content-Length", "0") or "0")
        request_body = wire.read(content_length)
        request = {
            "serverOrigin": self.server.origin,  # type: ignore[attr-defined]
            "method": self.command,
            "path": self.path,
            "headers": [
                {"name": name, "value": value}
                for name, value in self.headers.raw_items()
            ],
            "rawHeadersBase64": base64.b64encode(wire.header_bytes).decode("ascii"),
            "bodyBase64": base64.b64encode(request_body).decode("ascii"),
        }
        request_number = self.server.capture.request(  # type: ignore[attr-defined]
            request, wire.header_bytes + request_body
        )
        wire.finish_request()
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)
        self.server.capture.response(  # type: ignore[attr-defined]
            {
                "requestNumber": request_number,
                "status": status,
                "contentType": content_type,
                "bodyLength": len(body),
            },
            body,
            self._response_headers,
        )

    def log_message(self, format: str, *args: Any) -> None:
        message = "%s - - [%s] %s" % (
            self.address_string(), self.log_date_time_string(), format % args
        )
        self.server.capture.stdout(message)  # type: ignore[attr-defined]


class FixtureCapture:
    """Retain fixture wire inputs, outputs, and server stdout per engine."""

    def __init__(self, directory: Path):
        self.directory = directory
        self.directory.mkdir(parents=True, exist_ok=True)
        self.request_jsonl = self.directory / "fixture-requests.jsonl"
        self.response_jsonl = self.directory / "fixture-responses.jsonl"
        self.stdout_path = self.directory / "fixture.stdout.log"
        self.html_path = self.directory / "fixture-response.html"
        self.html_path.write_bytes(HTML.encode("utf-8"))
        self._lock = threading.Lock()
        self._request_number = 0

    def _append_json(self, path: Path, record: dict[str, Any]) -> None:
        data = (json.dumps(record, separators=(",", ":"), ensure_ascii=True) + "\n").encode()
        with self._lock:
            with path.open("ab") as output:
                output.write(data)
                output.flush()

    def stdout(self, message: str) -> None:
        data = (message + "\n").encode("utf-8", errors="surrogateescape")
        with self._lock:
            with self.stdout_path.open("ab") as output:
                output.write(data)
                output.flush()
        print(message, flush=True)

    def request(self, record: dict[str, Any], raw: bytes) -> int:
        with self._lock:
            self._request_number += 1
            number = self._request_number
        raw_path = self.directory / f"fixture-request-{number:04d}.bin"
        raw_path.write_bytes(raw)
        record["requestNumber"] = number
        record["rawRequestPath"] = str(raw_path)
        self._append_json(self.request_jsonl, record)
        self.stdout(json.dumps(record, separators=(",", ":"), ensure_ascii=True))
        return number

    def response(self, record: dict[str, Any], body: bytes, headers: bytes) -> None:
        number = record["requestNumber"]
        raw_path = self.directory / f"fixture-response-{number:04d}.bin"
        raw_path.write_bytes(headers + body)
        record["rawResponsePath"] = str(raw_path)
        record["rawHeadersBase64"] = base64.b64encode(headers).decode("ascii")
        record["bodyBase64"] = base64.b64encode(body).decode("ascii")
        self._append_json(self.response_jsonl, record)

    def metadata(self) -> dict[str, Any]:
        return {
            "requestRecordsPath": str(self.request_jsonl),
            "responseRecordsPath": str(self.response_jsonl),
            "stdoutPath": str(self.stdout_path),
            "fixtureHtmlPath": str(self.html_path),
        }


@contextmanager
def fixture_server(directory: Path) -> Iterator[FixtureCapture]:
    """Serve one origin for one engine run; each engine gets a fresh server."""

    capture = FixtureCapture(directory)
    server = ThreadingHTTPServer(("127.0.0.1", 0), MouseFixtureHandler)
    server.capture = capture  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    capture.origin = f"http://{host}:{port}"  # type: ignore[attr-defined]
    server.origin = capture.origin  # type: ignore[attr-defined]
    try:
        yield capture
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def _page_snapshot(page: Any) -> dict[str, Any]:
    return page.evaluate(
        """() => ({
          events: globalThis.__mouseEvents,
          targetClicks: globalThis.__targetClicks,
          decoyClicks: globalThis.__decoyClicks,
          focus: document.activeElement && document.activeElement.id,
          selection: (() => {
            const node = document.getElementById('text');
            return {start: node.selectionStart, end: node.selectionEnd, value: node.value};
          })(),
          checked: document.getElementById('check').checked
        })"""
    )


def _setup_poison(page: Any) -> None:
    page.evaluate(
        """() => {
          document.elementFromPoint = () => document.getElementById('decoy');
          globalThis.MouseEvent = function PoisonedMouseEvent() {
            throw new Error('page MouseEvent called');
          };
          globalThis.PointerEvent = function PoisonedPointerEvent() {
            throw new Error('page PointerEvent called');
          };
          Element.prototype.dispatchEvent = function poisonedDispatchEvent() {
            throw new Error('page dispatchEvent called');
          };
        }"""
    )


def _reset(page: Any) -> None:
    page.evaluate("globalThis.__resetMouseProbe()")


def _mouse_click(page: Any, *, click_count: int = 1) -> None:
    page.mouse.move(CLICK_X, CLICK_Y)
    page.mouse.down()
    page.mouse.up()
    if click_count != 1:
        raise AssertionError("use page.mouse.click for non-default click counts")


def _run_core_case(page: Any, poisoned: bool) -> dict[str, Any]:
    _reset(page)
    if poisoned:
        _setup_poison(page)
    action_error: dict[str, str] | None = None
    try:
        _mouse_click(page)
    except Exception as error:  # retain the snapshot even when CDP reports an action error
        action_error = error_record(error)
    snapshot = _page_snapshot(page)
    return {"poisoned": poisoned, "actionError": action_error, **snapshot}


def _run_mousedown_cancel_case(page: Any) -> dict[str, Any]:
    _reset(page)
    page.evaluate(
        """() => document.getElementById('target').addEventListener(
          'mousedown', event => event.preventDefault(), {once: true})"""
    )
    _mouse_click(page)
    return _page_snapshot(page)


def _run_checkbox_cancel_case(page: Any) -> dict[str, Any]:
    _reset(page)
    page.evaluate(
        """() => document.getElementById('check').addEventListener(
          'click', event => event.preventDefault(), {once: true})"""
    )
    page.mouse.click(30, 180)
    return _page_snapshot(page)


def _run_pointerdown_cancel_case(page: Any) -> dict[str, Any]:
    _reset(page)
    page.evaluate(
        """() => document.getElementById('target').addEventListener(
          'pointerdown', event => event.preventDefault(), {once: true})"""
    )
    _mouse_click(page)
    return _page_snapshot(page)


def _run_triple_click_case(page: Any) -> dict[str, Any]:
    _reset(page)
    page.mouse.click(70, 240, click_count=3)
    return _page_snapshot(page)


def _event_projection(event: dict[str, Any]) -> dict[str, Any]:
    return {field: event.get(field) for field in EVENT_FIELDS}


def assert_core_events(observation: dict[str, Any], *, label: str) -> None:
    events = observation.get("events")
    if not isinstance(events, list):
        raise AssertionError(f"{label}: events is not a list: {observation!r}")
    projected = [_event_projection(event) for event in events]
    types = [event["type"] for event in projected]
    if types != CORE_EVENT_TYPES:
        raise AssertionError(f"{label}: event order differs: {projected!r}")
    _assert_event_metadata(projected, target="target", x=CLICK_X, y=CLICK_Y, label=label)
    if observation.get("targetClicks") != 1 or observation.get("decoyClicks") != 0:
        raise AssertionError(f"{label}: click side effect differs: {observation!r}")
    if observation.get("focus") != "target":
        raise AssertionError(f"{label}: target did not receive focus: {observation!r}")
    if observation.get("actionError") is not None:
        raise AssertionError(f"{label}: mouse action failed: {observation!r}")


def _assert_event_metadata(
    events: list[dict[str, Any]],
    *,
    target: str,
    x: int,
    y: int,
    label: str,
    expected_details: list[int] | None = None,
) -> None:
    """Assert the Chrome-observed metadata for every event in a case."""

    detail_by_type = {
        "pointermove": 0,
        "mousemove": 0,
        "pointerdown": 0,
        "mousedown": 1,
        "focus": 0,
        "focusin": 0,
        "pointerup": 0,
        "mouseup": 1,
        "click": 1,
        "dblclick": 2,
    }
    buttons_by_type = {
        "pointermove": (0, -1),
        "mousemove": (0, 0),
        "pointerdown": (1, 0),
        "mousedown": (1, 0),
        "pointerup": (0, 0),
        "mouseup": (0, 0),
        "click": (0, 0),
        "dblclick": (0, 0),
    }
    coordinate_types = set(buttons_by_type)
    for index, event in enumerate(events):
        event_type = event.get("type")
        if event.get("target") != target or event.get("isTrusted") is not True:
            raise AssertionError(f"{label}: target/trust differs: {events!r}")
        if event.get("constructor") != EXPECTED_CONSTRUCTORS.get(event_type):
            raise AssertionError(f"{label}: constructor differs: {events!r}")
        expected_detail = (
            expected_details[index]
            if expected_details is not None
            else detail_by_type.get(event_type)
        )
        if event.get("detail") != expected_detail:
            raise AssertionError(f"{label}: detail differs: {events!r}")
        expected_pressure = (
            0.5 if event_type == "pointerdown" else
            0 if event_type in {"pointermove", "pointerup", "click"} else None
        )
        if event.get("pressure") != expected_pressure:
            raise AssertionError(f"{label}: pointer pressure differs: {events!r}")
        if event.get("defaultPrevented") is not False:
            raise AssertionError(f"{label}: unexpected defaultPrevented: {events!r}")
        if event_type in coordinate_types:
            if (event.get("clientX"), event.get("clientY")) != (x, y):
                raise AssertionError(f"{label}: coordinates differ: {events!r}")
            if (event.get("buttons"), event.get("button")) != buttons_by_type[event_type]:
                raise AssertionError(f"{label}: button metadata differs: {events!r}")
        elif (event.get("clientX"), event.get("clientY"), event.get("button"), event.get("buttons")) != (None, None, None, None):
            raise AssertionError(f"{label}: focus metadata differs: {events!r}")


def assert_same_as_chrome(chrome: dict[str, Any], candidate: dict[str, Any], *, label: str) -> None:
    """Compare fields that the paired Chrome run actually observed."""

    chrome_events = [_event_projection(event) for event in chrome.get("events", [])]
    candidate_events = [_event_projection(event) for event in candidate.get("events", [])]
    if chrome_events != candidate_events:
        raise AssertionError(
            f"{label}: candidate differs from Chrome\nChrome={chrome_events!r}\nCandidate={candidate_events!r}"
        )
    for key in ("targetClicks", "decoyClicks", "focus", "checked", "selection"):
        if candidate.get(key) != chrome.get(key):
            raise AssertionError(
                f"{label}: candidate {key} differs from Chrome: "
                f"{candidate.get(key)!r} != {chrome.get(key)!r}"
            )


def assert_mousedown_cancel(observation: dict[str, Any], *, label: str) -> None:
    types = [event.get("type") for event in observation.get("events", [])]
    expected_types = ["pointermove", "mousemove", "pointerdown", "mousedown", "pointerup", "mouseup", "click"]
    if types != expected_types:
        raise AssertionError(f"{label}: mousedown cancellation event sequence differs: {observation!r}")
    _assert_event_metadata(
        [_event_projection(event) for event in observation["events"]],
        target="target", x=CLICK_X, y=CLICK_Y, label=label,
    )
    if observation.get("focus") != "" or observation.get("targetClicks") != 1:
        raise AssertionError(f"{label}: mousedown cancellation focus/click differs: {observation!r}")


def assert_checkbox_cancel(observation: dict[str, Any], *, label: str) -> None:
    expected_types = [
        "pointermove", "mousemove", "pointerdown", "mousedown", "focus", "focusin",
        "pointerup", "mouseup", "click",
    ]
    if [event.get("type") for event in observation.get("events", [])] != expected_types:
        raise AssertionError(f"{label}: checkbox cancellation event sequence differs: {observation!r}")
    _assert_event_metadata(
        [_event_projection(event) for event in observation["events"]],
        target="check", x=30, y=180, label=label,
    )
    if observation.get("checked") is not False:
        raise AssertionError(f"{label}: canceled checkbox click did not roll back: {observation!r}")
    clicks = [event for event in observation.get("events", []) if event.get("type") == "click"]
    if len(clicks) != 1 or clicks[0].get("target") != "check":
        raise AssertionError(f"{label}: canceled checkbox click was not observed once: {observation!r}")
    if observation.get("focus") != "check":
        raise AssertionError(f"{label}: checkbox focus differs: {observation!r}")


def assert_pointerdown_cancel(observation: dict[str, Any], *, label: str) -> None:
    types = [event.get("type") for event in observation.get("events", [])]
    expected_types = ["pointermove", "mousemove", "pointerdown", "pointerup", "click"]
    if types != expected_types:
        raise AssertionError(f"{label}: pointerdown cancellation compatibility differs: {observation!r}")
    _assert_event_metadata(
        [_event_projection(event) for event in observation["events"]],
        target="target", x=CLICK_X, y=CLICK_Y, label=label,
    )
    if observation.get("focus") != "" or observation.get("targetClicks") != 1:
        raise AssertionError(f"{label}: pointerdown cancellation focus/click differs: {observation!r}")


def assert_triple_click(observation: dict[str, Any], *, label: str) -> None:
    expected_types = [
        "pointermove", "mousemove", "pointerdown", "mousedown", "focus", "focusin",
        "pointerup", "mouseup", "click",
        "pointerdown", "mousedown", "pointerup", "mouseup", "click", "dblclick",
        "pointerdown", "mousedown", "pointerup", "mouseup", "click",
    ]
    events = observation.get("events", [])
    if [event.get("type") for event in events] != expected_types:
        raise AssertionError(f"{label}: triple click event sequence differs: {observation!r}")
    projected = [_event_projection(event) for event in events]
    _assert_event_metadata(
        projected,
        target="text", x=70, y=240, label=label,
        expected_details=[0, 0, 0, 1, 0, 0, 0, 1, 1, 0, 2, 0, 2, 2, 2, 0, 3, 0, 3, 3],
    )
    clicks = [event for event in observation.get("events", []) if event.get("type") == "click"]
    if [event.get("detail") for event in clicks] != [1, 2, 3]:
        raise AssertionError(f"{label}: triple click detail differs: {observation!r}")
    dblclicks = [event for event in observation.get("events", []) if event.get("type") == "dblclick"]
    if len(dblclicks) != 1 or dblclicks[0].get("detail") != 2:
        raise AssertionError(f"{label}: double-click compatibility differs: {observation!r}")
    selection = observation.get("selection", {})
    if observation.get("focus") != "text" or selection.get("start") != 0 or selection.get("end") != len(selection.get("value", "")):
        raise AssertionError(f"{label}: triple click selection differs: {observation!r}")


def assert_case_observation(case: str, observation: dict[str, Any], *, label: str) -> None:
    if case in {"plain", "poisoned"}:
        assert_core_events(observation, label=label)
    elif case == "mousedown-cancel":
        assert_mousedown_cancel(observation, label=label)
    elif case == "checkbox-cancel":
        assert_checkbox_cancel(observation, label=label)
    elif case == "pointerdown-cancel":
        assert_pointerdown_cancel(observation, label=label)
    elif case == "triple-click":
        assert_triple_click(observation, label=label)


def _write_worker_result(output: Path, result: dict[str, Any]) -> None:
    """Atomically retain the latest worker checkpoint for hard-kill recovery."""

    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(output.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    temporary.replace(output)


def run_worker(endpoint: str, origin: str, output: Path, cases: list[str]) -> int:
    result: dict[str, Any] = {
        "schemaVersion": 1,
        "status": "running",
        "endpoint": endpoint,
        "fixtureOrigin": origin,
        "connection": "BrowserType.connect_over_cdp",
        "cases": {},
        "completedCases": [],
        "checkpoint": "initial",
    }
    _write_worker_result(output, result)
    try:
        from playwright.sync_api import sync_playwright

        with sync_playwright() as playwright:
            browser = playwright.chromium.connect_over_cdp(endpoint)
            result["browserVersion"] = browser.version
            try:
                context = browser.contexts[0] if browser.contexts else browser.new_context()
                for case in cases:
                    page = context.new_page()
                    record: dict[str, Any] = {"status": "running", "case": case}
                    result["cases"][case] = record
                    try:
                        page.goto(f"{origin}/mouse?case={case}", wait_until="load")
                        if case == "plain":
                            record["observations"] = _run_core_case(page, poisoned=False)
                        elif case == "poisoned":
                            record["observations"] = _run_core_case(page, poisoned=True)
                        elif case == "mousedown-cancel":
                            record["observations"] = _run_mousedown_cancel_case(page)
                        elif case == "checkbox-cancel":
                            record["observations"] = _run_checkbox_cancel_case(page)
                        elif case == "pointerdown-cancel":
                            record["observations"] = _run_pointerdown_cancel_case(page)
                        elif case == "triple-click":
                            record["observations"] = _run_triple_click_case(page)
                        else:
                            raise ValueError(f"unknown case: {case}")
                        assert_case_observation(case, record["observations"], label=f"worker:{case}")
                        record["status"] = "passed"
                    except Exception as error:
                        record["status"] = "failed"
                        record["error"] = error_record(error)
                    finally:
                        result["completedCases"] = list(result["cases"])
                        result["checkpoint"] = case
                        _write_worker_result(output, result)
                        page.close()
            finally:
                browser.close()
        result["status"] = "passed" if all(
            item["status"] == "passed" for item in result["cases"].values()
        ) else "failed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = error_record(error)
    result["checkpoint"] = "final"
    _write_worker_result(output, result)
    print(json.dumps(result), flush=True)
    return 0 if result["status"] == "passed" else 1


def _chrome_command(executable: Path, port: int, profile: Path) -> list[str]:
    return [
        str(executable),
        "--headless=new",
        "--disable-gpu",
        "--no-first-run",
        "--no-default-browser-check",
        "--remote-debugging-address=127.0.0.1",
        f"--remote-debugging-port={port}",
        f"--user-data-dir={profile}",
        "about:blank",
    ]


def run_engine(
    engine: str,
    *,
    obscura_bin: Path,
    persona: str,
    root: Path,
    cases: list[str],
    chrome_executable: Path | None,
) -> dict[str, Any]:
    directory = root / engine
    directory.mkdir(parents=True, exist_ok=True)
    port = free_port()
    endpoint = f"http://127.0.0.1:{port}"
    record: dict[str, Any] = {
        "engine": engine,
        "status": "running",
        "browserCapture": {},
        "persona": persona if engine == "obscura" else None,
    }
    with fixture_server(directory) as fixture:
        origin = fixture.origin  # type: ignore[attr-defined]
        record["fixtureCapture"] = fixture.metadata()
        record["fixtureCapture"]["origin"] = origin
        if engine == "chrome":
            if chrome_executable is None:
                raise RuntimeError("Playwright Chromium executable was not found")
            command = _chrome_command(chrome_executable, port, directory / "profile")
        else:
            command = [
                str(obscura_bin.resolve()),
                "--persona",
                persona,
                "--allow-private-network",
                "serve",
                "--host",
                "127.0.0.1",
                "--port",
                str(port),
            ]
        record["command"] = command
        child_output = directory / "worker-result.json"
        try:
            with external_process(
                command,
                endpoint,
                capture=record["browserCapture"],
                log_root=directory,
            ):
                record["workerCapture"] = capture_worker(
                    [
                        sys.executable,
                        str(Path(__file__).resolve()),
                        "--worker",
                        "--endpoint",
                        endpoint,
                        "--origin",
                        origin,
                        "--output",
                        str(child_output),
                        *sum((["--case", case] for case in cases), []),
                    ],
                    directory,
                    WORKER_TIMEOUT_SECONDS,
                )
                if child_output.exists():
                    record["workerResult"] = json.loads(
                        child_output.read_text(encoding="utf-8")
                    )
                worker_ok = (
                    record["workerCapture"]["status"] == "passed"
                    and record.get("workerResult", {}).get("status") == "passed"
                )
                record["status"] = "passed" if worker_ok else "failed"
        except Exception as error:
            record["status"] = "failed"
            record["error"] = error_record(error)
        finally:
            # The child writes its result in a finally block.  Read it after
            # process cleanup as well, so a timeout/close error still leaves
            # the browser version and every observation collected so far in
            # the top-level summary.
            if child_output.exists():
                try:
                    record["workerResult"] = json.loads(
                        child_output.read_text(encoding="utf-8")
                    )
                    if "browserVersion" in record["workerResult"]:
                        record["browserVersion"] = record["workerResult"]["browserVersion"]
                    if record["workerResult"].get("status") != "passed":
                        record["status"] = "failed"
                except Exception as error:
                    record["workerResultReadError"] = error_record(error)
                    record["status"] = "failed"
        record["fixtureCapture"] = fixture.metadata()
        record["fixtureCapture"]["origin"] = origin
    return record


def run(
    *,
    obscura_bin: Path,
    output: Path,
    persona: str,
    engines: list[str],
    cases: list[str],
) -> dict[str, Any]:
    output.parent.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="ob021-native-mouse-", dir=output.parent)).resolve()
    result: dict[str, Any] = {
        "schemaVersion": 1,
        "status": "running",
        "root": str(root),
        "cases": cases,
        "engines": {},
        "comparison": {},
    }
    try:
        chrome_executable: Path | None = None
        if "chrome" in engines:
            from playwright.sync_api import sync_playwright

            with sync_playwright() as playwright:
                chrome_executable = Path(playwright.chromium.executable_path)
        for engine in engines:
            try:
                result["engines"][engine] = run_engine(
                    engine,
                    obscura_bin=obscura_bin,
                    persona=persona,
                    root=root,
                    cases=cases,
                    chrome_executable=chrome_executable,
                )
            except Exception as error:
                result["engines"][engine] = {
                    "engine": engine,
                    "status": "failed",
                    "error": error_record(error),
                }

        chrome_record = result["engines"].get("chrome", {})
        obscura_record = result["engines"].get("obscura", {})
        chrome_worker = chrome_record.get("workerResult")
        obscura_worker = obscura_record.get("workerResult")

        def require_worker(record: dict[str, Any], worker: Any, engine: str) -> dict[str, Any]:
            if record.get("status") != "passed" or not isinstance(worker, dict):
                raise AssertionError(
                    f"{engine} worker did not pass: "
                    f"status={record.get('status')!r}, worker={worker!r}"
                )
            if worker.get("status") != "passed":
                raise AssertionError(f"{engine} worker result failed: {worker!r}")
            return worker

        def require_observation(worker: dict[str, Any], case: str, engine: str) -> dict[str, Any]:
            case_result = worker.get("cases", {}).get(case)
            observation = case_result.get("observations") if isinstance(case_result, dict) else None
            if not isinstance(observation, dict) or case_result.get("status") != "passed":
                raise AssertionError(
                    f"{engine}:{case} has no passing observation: {case_result!r}"
                )
            return observation

        if "chrome" in engines:
            chrome_worker = require_worker(chrome_record, chrome_worker, "chrome")
            for case in cases:
                observation = require_observation(chrome_worker, case, "chrome")
                assert_case_observation(
                    case,
                    observation,
                    label=f"{chrome_worker.get('browserVersion')}:{case}",
                )
            if "obscura" in engines:
                obscura_worker = require_worker(obscura_record, obscura_worker, "obscura")
                for case in cases:
                    chrome_observation = require_observation(chrome_worker, case, "chrome")
                    obscura_observation = require_observation(obscura_worker, case, "obscura")
                    assert_same_as_chrome(
                        chrome_observation,
                        obscura_observation,
                        label=f"obscura:{case}",
                    )
                result["comparison"] = {"reference": "chrome", "status": "passed"}
            else:
                result["comparison"] = {"reference": "chrome baseline", "status": "passed"}
        else:
            obscura_worker = require_worker(obscura_record, obscura_worker, "obscura")
            for case in cases:
                observation = require_observation(obscura_worker, case, "obscura")
                assert_case_observation(case, observation, label=f"obscura:{case}")
            result["comparison"] = {
                "reference": "embedded Chrome-verified contract",
                "status": "passed",
            }
    except Exception as error:
        result["comparison"] = {
            "reference": "chrome" if "chrome" in engines else "embedded Chrome-verified contract",
            "status": "failed",
            "error": error_record(error),
        }
    result["status"] = "passed" if all(
        item.get("status") == "passed" for item in result["engines"].values()
    ) and result["comparison"].get("status") == "passed" else "failed"
    output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obscura-bin", type=Path, default=Path("target/release/obscura"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--persona", default=DEFAULT_PERSONA)
    engine_group = parser.add_mutually_exclusive_group()
    engine_group.add_argument("--with-chrome", action="store_true")
    engine_group.add_argument("--chrome-only", action="store_true")
    parser.add_argument(
        "--case",
        action="append",
        choices=("plain", "poisoned", "mousedown-cancel", "checkbox-cancel", "pointerdown-cancel", "triple-click"),
    )
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--endpoint", help=argparse.SUPPRESS)
    parser.add_argument("--origin", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        if not args.endpoint or not args.origin or not args.output or not args.case:
            parser.error("worker needs endpoint, origin, output, and at least one case")
        return run_worker(args.endpoint, args.origin, args.output, args.case)

    cases = args.case or ["plain", "poisoned", "mousedown-cancel", "checkbox-cancel", "pointerdown-cancel", "triple-click"]
    engines = ["chrome"] if args.chrome_only else ["obscura"] + (["chrome"] if args.with_chrome else [])
    try:
        result = run(
            obscura_bin=args.obscura_bin,
            output=args.output.resolve(),
            persona=args.persona,
            engines=engines,
            cases=cases,
        )
    except Exception as error:
        print(json.dumps({"status": "failed", "error": error_record(error)}, indent=2))
        return 1
    print(json.dumps(result, indent=2))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Official Playwright wheel qualification for OB-021.

The probe keeps the browser and Playwright client in bounded child processes,
reuses the repository's process/capture helpers, and records fixture wire bytes
alongside every observation.  It deliberately records Chrome behavior rather
than converting event counts or timing into exact assertions.
"""

from __future__ import annotations

import argparse
import base64
from contextlib import contextmanager
import json
from pathlib import Path
import sys
import threading
import tempfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Iterator

if __package__:
    from .cdp_fixture import error_record, external_process, free_port
    from .migration_cors import _WireReader
    from .migration_smoke import capture_worker
else:
    from cdp_fixture import error_record, external_process, free_port
    from migration_cors import _WireReader
    from migration_smoke import capture_worker


CORE_CASES = ("metadata", "boundary-y", "boundary-x", "cancel-active",
              "cancel-passive", "cancel-default", "poison")
CASES = CORE_CASES + ("zero-delta", "omitted-delta")
WORKER_TIMEOUT_SECONDS = 45

HTML = r"""<!doctype html>
<meta charset="utf-8">
<title>OB-021 wheel fixture</title>
<style>
  html, body { margin: 0; padding: 0; }
  body { width: 1800px; height: 2200px; background: #eee; }
  #root-tail { position: absolute; left: 1300px; top: 1500px; width: 300px; height: 300px; }
  #outer { position: absolute; left: 80px; top: 80px; width: 420px; height: 300px;
           overflow: auto; border: 4px solid #111; background: #cde; }
  #outer-content { position: relative; width: 950px; height: 950px; }
  #inner { position: sticky; left: 30px; top: 30px; width: 250px; height: 180px;
           overflow: auto; border: 4px solid #822; background: #edc; }
  #inner-content { position: relative; width: 800px; height: 800px; background: #dec; }
  #leaf { position: absolute; left: 20px; top: 20px; width: 600px; height: 600px; }
  #root-wheel { position: absolute; left: 900px; top: 100px; width: 120px; height: 80px; }
</style>
<div id="outer"><div id="outer-content"><div id="inner"><div id="inner-content">
  <div id="leaf">inner leaf</div>
</div></div></div></div>
<div id="root-wheel">root wheel area</div>
<div id="root-tail">root tail</div>
<script>
(() => {
  const names = ['window', 'document', 'body', 'outer', 'inner'];
  const nodeFor = name => name === 'window' ? window : name === 'document' ? document :
    name === 'body' ? document.body : document.getElementById(name);
  const ident = node => {
    if (node === window) return 'window';
    if (node === document) return 'document';
    return node && (node.id || node.tagName || node.nodeName);
  };
  const offsets = () => ({
    rootX: window.scrollX, rootY: window.scrollY,
    outerX: document.getElementById('outer').scrollLeft,
    outerY: document.getElementById('outer').scrollTop,
    innerX: document.getElementById('inner').scrollLeft,
    innerY: document.getElementById('inner').scrollTop,
    maxRootX: Math.max(0, document.scrollingElement.scrollWidth - document.scrollingElement.clientWidth),
    maxRootY: Math.max(0, document.scrollingElement.scrollHeight - document.scrollingElement.clientHeight),
    maxOuterX: Math.max(0, document.getElementById('outer').scrollWidth - document.getElementById('outer').clientWidth),
    maxOuterY: Math.max(0, document.getElementById('outer').scrollHeight - document.getElementById('outer').clientHeight),
    maxInnerX: Math.max(0, document.getElementById('inner').scrollWidth - document.getElementById('inner').clientWidth),
    maxInnerY: Math.max(0, document.getElementById('inner').scrollHeight - document.getElementById('inner').clientHeight),
  });
  const recordEvent = (kind, scope, event, phase) => {
    const rec = {
      kind, scope, phase, type: event.type,
      target: ident(event.target), currentTarget: ident(event.currentTarget),
      constructor: event.constructor && event.constructor.name,
      isTrusted: event.isTrusted,
      bubbles: event.bubbles, cancelable: event.cancelable,
      defaultPrevented: event.defaultPrevented,
      clientX: typeof event.clientX === 'number' ? event.clientX : null,
      clientY: typeof event.clientY === 'number' ? event.clientY : null,
      button: typeof event.button === 'number' ? event.button : null,
      buttons: typeof event.buttons === 'number' ? event.buttons : null,
      detail: typeof event.detail === 'number' ? event.detail : null,
      deltaX: typeof event.deltaX === 'number' ? event.deltaX : null,
      deltaY: typeof event.deltaY === 'number' ? event.deltaY : null,
      deltaZ: typeof event.deltaZ === 'number' ? event.deltaZ : null,
      deltaMode: typeof event.deltaMode === 'number' ? event.deltaMode : null,
      altKey: !!event.altKey, ctrlKey: !!event.ctrlKey,
      metaKey: !!event.metaKey, shiftKey: !!event.shiftKey,
      offsets: offsets(),
    };
    globalThis.__wheelEvents.push(rec);
  };
  globalThis.__resetWheelProbe = () => {
    globalThis.__wheelEvents = [];
    globalThis.__poisonCalls = {elementFromPoint: 0, wheelEvent: 0,
      dispatchEvent: 0, elementScrollBy: 0, rootScrollBy: 0, windowScrollBy: 0};
  };
  globalThis.__wheelSnapshot = () => ({events: globalThis.__wheelEvents,
    poisonCalls: globalThis.__poisonCalls, offsets: offsets()});
  for (const scope of names) {
    const node = nodeFor(scope);
    node.addEventListener('wheel', e => recordEvent('wheel', scope, e, 'capture'), true);
    node.addEventListener('wheel', e => recordEvent('wheel', scope, e, 'bubble'), false);
    node.addEventListener('scroll', e => recordEvent('scroll', scope, e, 'bubble'), false);
  }
  globalThis.__resetWheelProbe();
})();
</script>
"""


class FixtureCapture:
    def __init__(self, directory: Path):
        self.directory = directory
        self.directory.mkdir(parents=True, exist_ok=True)
        self.request_jsonl = directory / "fixture-requests.jsonl"
        self.response_jsonl = directory / "fixture-responses.jsonl"
        self.stdout_path = directory / "fixture.stdout.log"
        self.html_path = directory / "fixture-response.html"
        self.html_path.write_bytes(HTML.encode())
        self._lock = threading.Lock()
        self._number = 0

    def _append(self, path: Path, record: dict[str, Any]) -> None:
        data = (json.dumps(record, separators=(",", ":"), ensure_ascii=True) + "\n").encode()
        with self._lock:
            with path.open("ab") as stream:
                stream.write(data)
                stream.flush()

    def stdout(self, text: str) -> None:
        with self._lock:
            with self.stdout_path.open("ab") as stream:
                stream.write((text + "\n").encode(errors="surrogateescape"))
                stream.flush()
        print(text, flush=True)

    def request(self, record: dict[str, Any], raw: bytes) -> int:
        with self._lock:
            self._number += 1
            number = self._number
        raw_path = self.directory / f"fixture-request-{number:04d}.bin"
        raw_path.write_bytes(raw)
        record["requestNumber"] = number
        record["rawRequestPath"] = str(raw_path)
        self._append(self.request_jsonl, record)
        self.stdout(json.dumps(record, separators=(",", ":"), ensure_ascii=True))
        return number

    def response(self, record: dict[str, Any], body: bytes, headers: bytes) -> None:
        number = record["requestNumber"]
        raw_path = self.directory / f"fixture-response-{number:04d}.bin"
        raw_path.write_bytes(headers + body)
        record["rawResponsePath"] = str(raw_path)
        record["rawHeadersBase64"] = base64.b64encode(headers).decode()
        record["bodyBase64"] = base64.b64encode(body).decode()
        self._append(self.response_jsonl, record)

    def metadata(self) -> dict[str, str]:
        return {"requestRecordsPath": str(self.request_jsonl),
                "responseRecordsPath": str(self.response_jsonl),
                "stdoutPath": str(self.stdout_path),
                "fixtureHtmlPath": str(self.html_path)}


class WheelHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "ObscuraOB021WheelFixture/1"
    sys_version = ""

    def setup(self) -> None:
        super().setup()
        self.rfile = _WireReader(self.rfile)

    def do_GET(self) -> None:
        path = self.path.split("?", 1)[0]
        if path == "/wheel":
            status, body, content_type = 200, HTML.encode(), "text/html; charset=utf-8"
        else:
            status, body, content_type = 404, b"not found", "text/plain; charset=utf-8"
        content_length = int(self.headers.get("Content-Length", "0") or "0")
        request_body = self.rfile.read(content_length)
        wire = self.rfile
        record = {"serverOrigin": self.server.origin, "method": self.command,
                  "path": self.path,
                  "headers": [{"name": n, "value": v} for n, v in self.headers.raw_items()],
                  "rawHeadersBase64": base64.b64encode(wire.header_bytes).decode(),
                  "bodyBase64": base64.b64encode(request_body).decode()}
        number = self.server.capture.request(record, wire.header_bytes + request_body)
        wire.finish_request()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        headers = getattr(self, "_response_headers", b"")
        self.wfile.write(body)
        self.server.capture.response({"requestNumber": number, "status": status,
                                      "contentType": content_type, "bodyLength": len(body)},
                                     body, headers)

    def end_headers(self) -> None:
        self._response_headers = b"".join(self._headers_buffer) + b"\r\n"
        super().end_headers()

    def log_message(self, fmt: str, *args: Any) -> None:
        self.server.capture.stdout("%s - - [%s] %s" %
                                   (self.address_string(), self.log_date_time_string(), fmt % args))


@contextmanager
def fixture_server(directory: Path) -> Iterator[FixtureCapture]:
    capture = FixtureCapture(directory)
    server = ThreadingHTTPServer(("127.0.0.1", 0), WheelHandler)
    server.capture = capture  # type: ignore[attr-defined]
    capture.origin = f"http://127.0.0.1:{server.server_address[1]}"  # type: ignore[attr-defined]
    server.origin = capture.origin  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield capture
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)


def _write_checkpoint(path: Path, result: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    temporary.replace(path)


def _settle(page: Any) -> dict[str, int]:
    result = page.evaluate("""() => new Promise(resolve => {
      let previous = null, stable = 0, frames = 0;
      const read = () => JSON.stringify([
        window.scrollX, window.scrollY,
        document.getElementById('outer').scrollLeft,
        document.getElementById('outer').scrollTop,
        document.getElementById('inner').scrollLeft,
        document.getElementById('inner').scrollTop,
      ]);
      const frame = () => {
        const current = read();
        stable = current === previous ? stable + 1 : 0;
        previous = current;
        frames++;
        if (stable >= 2 || frames >= 30) return resolve({frames, stable});
        requestAnimationFrame(frame);
      };
      requestAnimationFrame(frame);
    })""")
    if result.get("stable", 0) < 2:
        observation = page.evaluate("globalThis.__wheelSnapshot()")
        raise AssertionError(f"wheel offsets did not settle: {result!r}; {observation!r}")
    return result


def _snapshot(page: Any) -> dict[str, Any]:
    settle = _settle(page)
    snapshot = page.evaluate("globalThis.__wheelSnapshot()")
    snapshot["settle"] = settle
    return snapshot


def _reset(page: Any) -> None:
    page.evaluate("""() => {
      window.scrollTo(0, 0);
      for (const id of ['outer', 'inner']) {
        const node = document.getElementById(id); node.scrollLeft = 0; node.scrollTop = 0;
      }
      globalThis.__resetWheelProbe();
    }""")
    _settle(page)
    page.evaluate("globalThis.__resetWheelProbe()")


def _inner_point(page: Any) -> tuple[float, float]:
    box = page.locator("#inner").bounding_box()
    if not box:
        raise RuntimeError("inner has no bounding box")
    return box["x"] + box["width"] / 2, box["y"] + box["height"] / 2


def _wheel(page: Any, dx: float, dy: float) -> dict[str, Any]:
    x, y = _inner_point(page)
    page.mouse.move(x, y)
    before = page.evaluate("globalThis.__wheelSnapshot()")
    page.mouse.wheel(dx, dy)
    after = _snapshot(page)
    return {"point": {"x": x, "y": y}, "delta": {"x": dx, "y": dy},
            "before": before, "after": after}


def _set_offsets(page: Any, *, root_x: float = 0, root_y: float = 0,
                 outer_x: float = 0, outer_y: float = 0,
                 inner_x: float = 0, inner_y: float = 0) -> None:
    page.evaluate("""v => {
      window.scrollTo(v.rootX, v.rootY);
      const outer = document.getElementById('outer');
      const inner = document.getElementById('inner');
      outer.scrollLeft = v.outerX; outer.scrollTop = v.outerY;
      inner.scrollLeft = v.innerX; inner.scrollTop = v.innerY;
    }""", {"rootX": root_x, "rootY": root_y, "outerX": outer_x,
           "outerY": outer_y, "innerX": inner_x, "innerY": inner_y})
    _settle(page)
    page.evaluate("globalThis.__resetWheelProbe()")


def _metadata(page: Any) -> dict[str, Any]:
    _reset(page)
    page.evaluate("""() => {
      globalThis.__resetWheelProbe();
      for (const node of [window, document]) node.addEventListener('wheel', e => {
        globalThis.__metadataFinal = {
          constructor: e.constructor && e.constructor.name,
          isTrusted: e.isTrusted, button: e.button, buttons: e.buttons,
          detail: e.detail, deltaX: e.deltaX, deltaY: e.deltaY, deltaZ: e.deltaZ,
          deltaMode: e.deltaMode, cancelable: e.cancelable, passiveSeenDefaultPrevented: e.defaultPrevented,
          clientX: e.clientX, clientY: e.clientY,
        };
      }, true);
    }""")
    page.mouse.move(*_inner_point(page))
    page.mouse.wheel(12.5, -8.25)
    observation = _snapshot(page)
    observation["metadataListener"] = page.evaluate("globalThis.__metadataFinal || null")
    return observation


def _boundary(page: Any, axis: str) -> dict[str, Any]:
    _reset(page)
    axis_delta = (0, 80) if axis == "y" else (80, 0)
    near = page.evaluate("""axis => {
      const outer = document.getElementById('outer');
      const inner = document.getElementById('inner');
      return {
        innerX: axis === 'x' ? Math.max(0, inner.scrollWidth - inner.clientWidth - 20) : 0,
        innerY: axis === 'y' ? Math.max(0, inner.scrollHeight - inner.clientHeight - 20) : 0,
        outerX: axis === 'x' ? Math.max(0, outer.scrollWidth - outer.clientWidth - 20) : 0,
        outerY: axis === 'y' ? Math.max(0, outer.scrollHeight - outer.clientHeight - 20) : 0,
      };
    }""", axis)
    steps: list[dict[str, Any]] = []
    _set_offsets(page)
    steps.append({"name": "inner-open", "operation": _wheel(page, *axis_delta)})
    _set_offsets(page, inner_x=near["innerX"], inner_y=near["innerY"])
    steps.append({"name": "inner-near-boundary-large-delta", "operation": _wheel(page, *(axis_delta[0] * 3, axis_delta[1] * 3))})
    if axis == "y":
        near_outer = {"inner_x": 0, "inner_y": 10_000,
                      "outer_x": 0, "outer_y": near["outerY"]}
    else:
        near_outer = {"inner_x": 10_000, "inner_y": 0,
                      "outer_x": near["outerX"], "outer_y": 0}
    _set_offsets(page, **near_outer)
    steps.append({"name": "outer-near-boundary-large-delta", "operation": _wheel(page, *(axis_delta[0] * 3, axis_delta[1] * 3))})
    if axis == "y":
        saturated = {"inner_x": 0, "inner_y": 10_000,
                     "outer_x": 0, "outer_y": 10_000}
    else:
        saturated = {"inner_x": 10_000, "inner_y": 0,
                     "outer_x": 10_000, "outer_y": 0}
    _set_offsets(page, **saturated, root_x=0, root_y=0)
    steps.append({"name": "root-open-after-ancestors-saturated", "operation": _wheel(page, *(axis_delta[0] * 3, axis_delta[1] * 3))})
    return {"axis": axis, "nearOffsets": near, "steps": steps,
            "final": _snapshot(page)}


def _cancel(page: Any, mode: str) -> dict[str, Any]:
    _reset(page)
    page.evaluate("""mode => {
      const action = e => {
        const before = e.defaultPrevented;
        e.preventDefault();
        globalThis.__cancelObservation = {mode, before, after: e.defaultPrevented};
      };
      if (mode === 'active') document.getElementById('inner').addEventListener('wheel', action, {passive:false});
      if (mode === 'passive') document.getElementById('inner').addEventListener('wheel', action, {passive:true});
      if (mode === 'default') {
        document.addEventListener('wheel', action);
        window.addEventListener('wheel', action);
      }
    }""", mode)
    page.mouse.move(*_inner_point(page))
    page.mouse.wheel(0, 120)
    observation = _snapshot(page)
    observation["cancelObservation"] = page.evaluate("globalThis.__cancelObservation || null")
    return observation


def _poison(page: Any) -> dict[str, Any]:
    _reset(page)
    page.evaluate("""() => {
      const calls = globalThis.__poisonCalls;
      document.elementFromPoint = () => { calls.elementFromPoint++; return document.getElementById('root-wheel'); };
      globalThis.WheelEvent = function PoisonedWheelEvent() { calls.wheelEvent++; throw new Error('poison WheelEvent'); };
      Element.prototype.dispatchEvent = function poisonedDispatch() { calls.dispatchEvent++; return false; };
      Element.prototype.scrollBy = function poisonedElementScrollBy() { calls.elementScrollBy++; throw new Error('poison element scrollBy'); };
      document.scrollingElement.scrollBy = function poisonedRootScrollBy() { calls.rootScrollBy++; throw new Error('poison root scrollBy'); };
      globalThis.scrollBy = function poisonedWindowScrollBy() { calls.windowScrollBy++; throw new Error('poison window scrollBy'); };
    }""")
    page.mouse.move(*_inner_point(page))
    page.mouse.wheel(0, 100)
    return _snapshot(page)


def _zero_delta(page: Any) -> dict[str, Any]:
    _reset(page)
    point = _inner_point(page)
    page.mouse.move(*point)
    before = page.evaluate("globalThis.__wheelSnapshot()")
    page.mouse.wheel(0, 0)
    after = _snapshot(page)
    return {"point": {"x": point[0], "y": point[1]}, "before": before, "after": after}


def _omitted_delta(page: Any) -> dict[str, Any]:
    _reset(page)
    point = _inner_point(page)
    page.mouse.move(*point)
    before = page.evaluate("globalThis.__wheelSnapshot()")
    session = page.context.new_cdp_session(page)
    response: Any = None
    action_error: dict[str, Any] | None = None
    try:
        try:
            response = session.send("Input.dispatchMouseEvent", {
                "type": "mouseWheel", "x": point[0], "y": point[1],
            })
        except Exception as error:
            action_error = error_record(error)
    finally:
        session.detach()
    after = _snapshot(page)
    return {"point": {"x": point[0], "y": point[1]}, "response": response,
            "actionError": action_error,
            "before": before, "after": after}


def run_case(page: Any, case: str) -> dict[str, Any]:
    if case == "metadata": return _metadata(page)
    if case == "boundary-y": return _boundary(page, "y")
    if case == "boundary-x": return _boundary(page, "x")
    if case == "cancel-active": return _cancel(page, "active")
    if case == "cancel-passive": return _cancel(page, "passive")
    if case == "cancel-default": return _cancel(page, "default")
    if case == "poison": return _poison(page)
    if case == "zero-delta": return _zero_delta(page)
    if case == "omitted-delta": return _omitted_delta(page)
    raise ValueError(case)


def _offsets(observation: dict[str, Any]) -> dict[str, Any]:
    return observation["offsets"]


def _wheel_fields(observation: dict[str, Any]) -> dict[str, Any]:
    metadata = observation.get("metadataListener")
    if isinstance(metadata, dict):
        return {key: metadata.get(key) for key in (
            "constructor", "isTrusted", "button", "buttons", "detail",
            "deltaX", "deltaY", "deltaZ", "deltaMode", "cancelable",
            "clientX", "clientY")}
    wheels = [event for event in observation.get("events", [])
              if event.get("kind") == "wheel"]
    if not wheels:
        return {}
    return {key: wheels[0].get(key) for key in (
        "constructor", "isTrusted", "button", "buttons", "detail",
        "deltaX", "deltaY", "deltaZ", "deltaMode", "cancelable",
        "clientX", "clientY")}


def assert_chrome_contract(case: str, observation: dict[str, Any], *, label: str) -> None:
    """Assert stable Chrome observations while retaining all raw records."""

    if case == "metadata":
        expected = {
            "constructor": "WheelEvent", "isTrusted": True, "button": 0,
            "buttons": 0, "detail": 0, "deltaX": 12.5, "deltaY": -8.25,
            "deltaZ": 0, "deltaMode": 0, "cancelable": True,
        }
        actual = _wheel_fields(observation)
        for key, value in expected.items():
            if actual.get(key) != value:
                raise AssertionError(f"{label}: metadata {key}={actual.get(key)!r}, expected {value!r}")
        return
    if case.startswith("boundary-"):
        axis = case[-1]
        steps = observation.get("steps", [])
        if len(steps) != 4:
            raise AssertionError(f"{label}: expected four boundary steps")
        first = steps[0]["operation"]["after"]["offsets"]
        near = steps[1]["operation"]["after"]["offsets"]
        outer = steps[2]["operation"]["after"]["offsets"]
        root = steps[3]["operation"]["after"]["offsets"]
        if axis == "y":
            if first["innerY"] != 80 or first["outerY"] != 0 or first["rootY"] != 0:
                raise AssertionError(f"{label}: inner open offsets differ: {first}")
            if near["innerY"] != near["maxInnerY"] or near["outerY"] != 0 or near["rootY"] != 0:
                raise AssertionError(f"{label}: inner boundary offsets differ: {near}")
            if outer["outerY"] != outer["maxOuterY"] or outer["rootY"] != 0:
                raise AssertionError(f"{label}: outer boundary offsets differ: {outer}")
            if root["rootY"] != 240 or root["outerY"] != root["maxOuterY"]:
                raise AssertionError(f"{label}: root handoff offsets differ: {root}")
        else:
            if first["innerX"] != 80 or first["outerX"] != 0 or first["rootX"] != 0:
                raise AssertionError(f"{label}: inner open offsets differ: {first}")
            if near["innerX"] != near["maxInnerX"] or near["outerX"] != 0 or near["rootX"] != 0:
                raise AssertionError(f"{label}: inner boundary offsets differ: {near}")
            if outer["outerX"] != outer["maxOuterX"] or outer["rootX"] != 0:
                raise AssertionError(f"{label}: outer boundary offsets differ: {outer}")
            if root["rootX"] != 240 or root["outerX"] != root["maxOuterX"]:
                raise AssertionError(f"{label}: root handoff offsets differ: {root}")
        return
    if case.startswith("cancel-"):
        cancel = observation.get("cancelObservation") or {}
        offsets = _offsets(observation)
        if case == "cancel-active":
            if cancel.get("after") is not True or any(offsets[key] != 0 for key in ("rootX", "rootY", "outerX", "outerY", "innerX", "innerY")):
                raise AssertionError(f"{label}: active cancellation differs: {observation}")
        elif cancel.get("after") is not False or offsets.get("innerY") != 120:
            raise AssertionError(f"{label}: passive/default cancellation differs: {observation}")
        return
    if case == "poison":
        calls = observation.get("poisonCalls") or {}
        expected_calls = {"elementFromPoint", "wheelEvent", "dispatchEvent",
                          "elementScrollBy", "rootScrollBy", "windowScrollBy"}
        if set(calls) != expected_calls or any(value != 0 for value in calls.values()) or observation.get("offsets", {}).get("innerY") != 100:
            raise AssertionError(f"{label}: poison was observable: {observation}")
        if "leaf" not in {event.get("target") for event in observation.get("events", []) if event.get("kind") == "wheel"}:
            raise AssertionError(f"{label}: native hit target was not leaf: {observation}")
        return
    if case == "zero-delta":
        keys = ("rootX", "rootY", "outerX", "outerY", "innerX", "innerY")
        before = observation.get("before", {}).get("offsets", {})
        after = observation.get("after", {}).get("offsets", {})
        if any(key not in before or key not in after or before[key] != after[key] for key in keys):
            raise AssertionError(f"{label}: zero delta changed offsets: {observation}")
        if any(event.get("kind") == "scroll" for event in observation.get("after", {}).get("events", [])):
            raise AssertionError(f"{label}: zero delta generated scroll: {observation}")
        if not any(
            event.get("kind") == "wheel" and event.get("deltaX") == 0 and event.get("deltaY") == 0
            for event in observation.get("after", {}).get("events", [])):
            raise AssertionError(f"{label}: zero delta did not dispatch a zero wheel: {observation}")
        return
    if case == "omitted-delta":
        error = observation.get("actionError") or {}
        message = error.get("message", "")
        if not all(part in message for part in ("Protocol error (Input.dispatchMouseEvent)", "deltaX", "deltaY")):
            raise AssertionError(f"{label}: omitted delta did not fail with the Chrome protocol error: {observation}")
        if any(event.get("kind") == "wheel" for event in observation.get("after", {}).get("events", [])):
            raise AssertionError(f"{label}: omitted delta unexpectedly dispatched wheel: {observation}")
        return


def compare_case(case: str, reference: dict[str, Any], candidate: dict[str, Any], *, label: str) -> None:
    """Compare behavior fields whose meaning is independent of event timing."""

    if case == "metadata":
        if _wheel_fields(reference) != _wheel_fields(candidate):
            raise AssertionError(f"{label}: metadata differs: {_wheel_fields(reference)!r} != {_wheel_fields(candidate)!r}")
    elif case.startswith("boundary-"):
        ref_steps = reference.get("steps", [])
        got_steps = candidate.get("steps", [])
        if len(ref_steps) != len(got_steps):
            raise AssertionError(f"{label}: boundary step count differs")
        keys = ("rootX", "rootY", "outerX", "outerY", "innerX", "innerY")
        for index, (ref, got) in enumerate(zip(ref_steps, got_steps)):
            ro = ref["operation"]["after"]["offsets"]
            go = got["operation"]["after"]["offsets"]
            if {key: ro[key] for key in keys} != {key: go[key] for key in keys}:
                raise AssertionError(f"{label}: step {index} offsets differ: {ro!r} != {go!r}")
    elif case.startswith("cancel-"):
        if reference.get("cancelObservation") != candidate.get("cancelObservation"):
            raise AssertionError(f"{label}: cancel observation differs")
        keys = ("rootX", "rootY", "outerX", "outerY", "innerX", "innerY")
        if {key: reference["offsets"][key] for key in keys} != {key: candidate["offsets"][key] for key in keys}:
            raise AssertionError(f"{label}: cancel offsets differ")
    elif case == "poison":
        if reference.get("poisonCalls") != candidate.get("poisonCalls"):
            raise AssertionError(f"{label}: poison calls differ")
        if candidate.get("offsets") != reference.get("offsets"):
            raise AssertionError(f"{label}: poison offsets differ")
    elif case in {"zero-delta", "omitted-delta"}:
        if reference.get("after", {}).get("offsets") != candidate.get("after", {}).get("offsets"):
            raise AssertionError(f"{label}: zero/omitted offsets differ")
        if bool(reference.get("actionError")) != bool(candidate.get("actionError")):
            raise AssertionError(f"{label}: zero/omitted action error presence differs")


def run_worker(endpoint: str, origin: str, output: Path, cases: list[str]) -> int:
    result: dict[str, Any] = {"schemaVersion": 1, "status": "running", "endpoint": endpoint,
                              "fixtureOrigin": origin, "connection": "BrowserType.connect_over_cdp",
                              "cases": {}, "completedCases": [], "checkpoint": "initial"}
    _write_checkpoint(output, result)
    try:
        from playwright.sync_api import sync_playwright
        with sync_playwright() as playwright:
            browser = playwright.chromium.connect_over_cdp(endpoint)
            result["browserVersion"] = browser.version
            context = browser.contexts[0] if browser.contexts else browser.new_context()
            try:
                for case in cases:
                    page = context.new_page()
                    record: dict[str, Any] = {"status": "running", "case": case}
                    result["cases"][case] = record
                    try:
                        page.set_viewport_size({"width": 1280, "height": 720})
                        page.goto(f"{origin}/wheel?case={case}", wait_until="load")
                        record["observations"] = run_case(page, case)
                        assert_chrome_contract(case, record["observations"], label=case)
                        record["status"] = "passed"
                    except Exception as error:
                        record["status"] = "failed"
                        record["error"] = error_record(error)
                    finally:
                        result["completedCases"] = list(result["cases"])
                        result["checkpoint"] = case
                        _write_checkpoint(output, result)
                        page.close()
            finally:
                browser.close()
        result["status"] = "passed" if all(v.get("status") == "passed" for v in result["cases"].values()) else "failed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = error_record(error)
    result["checkpoint"] = "final"
    _write_checkpoint(output, result)
    print(json.dumps(result), flush=True)
    return 0 if result["status"] == "passed" else 1


def chrome_command(executable: Path, port: int, profile: Path) -> list[str]:
    return [str(executable), "--headless=new", "--disable-gpu", "--no-first-run",
            "--no-default-browser-check", "--window-size=1280,720",
            "--remote-debugging-address=127.0.0.1", f"--remote-debugging-port={port}",
            f"--user-data-dir={profile}", "about:blank"]


def run_engine(engine: str, *, obscura_bin: Path, persona: str, root: Path,
               cases: list[str], chrome_executable: Path | None) -> dict[str, Any]:
    directory = root / engine
    directory.mkdir(parents=True, exist_ok=True)
    port = free_port()
    endpoint = f"http://127.0.0.1:{port}"
    record: dict[str, Any] = {"engine": engine, "status": "running",
                              "browserCapture": {}, "persona": persona if engine == "obscura" else None}
    try:
        with fixture_server(directory / "fixture") as fixture:
            origin = fixture.origin  # type: ignore[attr-defined]
            record["fixtureCapture"] = fixture.metadata()
            record["fixtureCapture"]["origin"] = origin
            if engine == "chrome":
                if chrome_executable is None:
                    raise RuntimeError("Playwright Chromium executable was not found")
                command = chrome_command(chrome_executable, port, directory / "profile")
            else:
                command = [str(obscura_bin.resolve()), "--persona", persona,
                           "--allow-private-network", "serve", "--host", "127.0.0.1",
                           "--port", str(port)]
            record["command"] = command
            child_output = directory / "worker-result.json"
            with external_process(command, endpoint, capture=record["browserCapture"], log_root=directory):
                record["workerCapture"] = capture_worker(
                    [sys.executable, str(Path(__file__).resolve()), "--worker",
                     "--endpoint", endpoint, "--origin", origin, "--output", str(child_output),
                     *sum((["--case", case] for case in cases), [])],
                    directory, WORKER_TIMEOUT_SECONDS)
                if child_output.exists():
                    record["workerResult"] = json.loads(child_output.read_text(encoding="utf-8"))
            record["fixtureCapture"] = fixture.metadata()
            record["fixtureCapture"]["origin"] = origin
        worker = record.get("workerResult", {})
        record["status"] = "passed" if record.get("workerCapture", {}).get("status") == "passed" and worker.get("status") == "passed" else "failed"
    except Exception as error:
        record["status"] = "failed"
        record["error"] = error_record(error)
        child_output = directory / "worker-result.json"
        if child_output.exists():
            try:
                record["workerResult"] = json.loads(child_output.read_text(encoding="utf-8"))
            except Exception as read_error:
                record["workerResultReadError"] = error_record(read_error)
    return record


def _load_reference(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if isinstance(data.get("engines"), dict) and isinstance(data["engines"].get("chrome"), dict):
        worker = data["engines"]["chrome"].get("workerResult")
        if isinstance(worker, dict):
            return worker
    if isinstance(data.get("workerResult"), dict):
        return data["workerResult"]
    if isinstance(data.get("cases"), dict):
        return data
    raise ValueError(f"reference JSON has no Chrome worker cases: {path}")


def run(*, obscura_bin: Path, output: Path, persona: str, engines: list[str],
        cases: list[str], reference_json: Path | None) -> dict[str, Any]:
    output.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="native-wheel-", dir=output)).resolve()
    result: dict[str, Any] = {"schemaVersion": 1, "status": "running", "root": str(root),
                              "cases": cases, "persona": persona, "engines": {}, "comparison": {}}
    chrome_executable: Path | None = None
    try:
        if "chrome" in engines:
            from playwright.sync_api import sync_playwright
            with sync_playwright() as playwright:
                chrome_executable = Path(playwright.chromium.executable_path)
        for engine in engines:
            result["engines"][engine] = run_engine(
                engine, obscura_bin=obscura_bin, persona=persona, root=root,
                cases=cases, chrome_executable=chrome_executable)
            (output / "wheel-result.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")

        reference_worker: dict[str, Any] | None = None
        if reference_json is not None:
            reference_worker = _load_reference(reference_json)
            reference_label = str(reference_json)
        elif "chrome" in engines:
            reference_worker = result["engines"].get("chrome", {}).get("workerResult")
            reference_label = "paired Chrome"
        else:
            reference_label = "embedded Chrome contract"

        if reference_worker is not None:
            if reference_worker.get("status") != "passed":
                raise AssertionError(f"reference worker did not pass: {reference_worker!r}")
            for case in cases:
                ref_case = reference_worker.get("cases", {}).get(case, {})
                ref_observation = ref_case.get("observations") if isinstance(ref_case, dict) else None
                if ref_case.get("status") != "passed" or not isinstance(ref_observation, dict):
                    raise AssertionError(f"reference case has no observation: {case}")
                assert_chrome_contract(case, ref_observation, label=f"reference:{case}")

        candidate = result["engines"].get("obscura")
        if candidate is not None:
            worker = candidate.get("workerResult")
            if candidate.get("status") != "passed" or not isinstance(worker, dict) or worker.get("status") != "passed":
                raise AssertionError(f"obscura worker did not pass: {candidate!r}")
            for case in cases:
                case_result = worker.get("cases", {}).get(case, {})
                observation = case_result.get("observations") if isinstance(case_result, dict) else None
                if case_result.get("status") != "passed" or not isinstance(observation, dict):
                    raise AssertionError(f"obscura case has no observation: {case}")
                assert_chrome_contract(case, observation, label=f"obscura:{case}")
                if reference_worker is not None:
                    ref_observation = reference_worker["cases"][case]["observations"]
                    compare_case(case, ref_observation, observation, label=f"obscura:{case}")
            result["comparison"] = {"reference": reference_label, "status": "passed"}
        else:
            result["comparison"] = {"reference": reference_label, "status": "passed"}
    except Exception as error:
        result["comparison"] = {"reference": reference_label if "reference_label" in locals() else "chrome", "status": "failed", "error": error_record(error)}
    result["status"] = "passed" if all(record.get("status") == "passed" for record in result["engines"].values()) and result["comparison"].get("status") == "passed" else "failed"
    (output / "wheel-result.json").write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obscura-bin", type=Path, default=Path("target/release/obscura"))
    parser.add_argument("--persona", default="windows_chrome145")
    parser.add_argument("--output", type=Path, default=Path("/tmp/ob021-wheel-smoke"))
    parser.add_argument("--case", action="append", choices=CASES)
    parser.add_argument("--with-chrome", action="store_true")
    parser.add_argument("--chrome-only", action="store_true")
    parser.add_argument("--reference-json", type=Path)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--endpoint", help=argparse.SUPPRESS)
    parser.add_argument("--origin", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        if not args.endpoint or not args.origin or not args.case:
            parser.error("worker needs endpoint, origin and cases")
        return run_worker(args.endpoint, args.origin, args.output, args.case)
    if args.with_chrome and args.chrome_only:
        parser.error("--with-chrome and --chrome-only are mutually exclusive")
    cases = args.case or list(CASES)
    if args.reference_json is not None and args.chrome_only:
        parser.error("--reference-json cannot be combined with --chrome-only")
    if args.chrome_only:
        engines = ["chrome"]
    else:
        engines = ["obscura"] + (["chrome"] if args.with_chrome else [])
    result = run(obscura_bin=args.obscura_bin, output=args.output.resolve(), persona=args.persona,
                 engines=engines, cases=cases, reference_json=args.reference_json)
    print(json.dumps(result, indent=2))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())

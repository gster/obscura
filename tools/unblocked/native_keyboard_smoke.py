#!/usr/bin/env python3
"""Bounded offline Chrome-vs-Obscura keyboard/text qualification.

The fixture and browser captures are intentionally raw.  Each browser runs in
its own process and the Playwright worker has a hard deadline, so a broken CDP
keyboard path cannot strand the qualification process.
"""

from __future__ import annotations

import argparse
import base64
from contextlib import contextmanager
import json
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import sys
import tempfile
import threading
from typing import Any, Iterator

if __package__:
    from .cdp_fixture import error_record, external_process, free_port
    from .migration_cors import _WireReader
    from .migration_smoke import capture_worker
else:
    from cdp_fixture import error_record, external_process, free_port
    from migration_cors import _WireReader
    from migration_smoke import capture_worker


DEFAULT_PERSONA = "windows_chrome145"
WORKER_TIMEOUT_SECONDS = 45
CASES = (
    "insert-text",
    "key-phases",
    "cancellation",
    "metadata",
    "poison",
    "reentrancy",
    "maxlength",
    "protocol-negative",
)

HTML = r'''<!doctype html>
<meta charset="utf-8"><title>OB-027 native keyboard fixture</title>
<style>input,textarea{font:16px sans-serif} #edit{position:absolute;left:20px;top:20px}
#other{position:absolute;left:20px;top:80px} #ro{position:absolute;left:20px;top:140px}</style>
<input id="edit" value="abcd"><textarea id="other">new</textarea>
<input id="ro" value="readonly" readonly><button id="button">button</button>
<script>
(() => {
  const nodes = ['edit', 'other', 'ro', 'button'].map(id => document.getElementById(id))
    .concat([document, window]);
  const name = n => n === document ? 'document' : n === window ? 'window' : n && n.id;
  const read = e => ({type:e.type, target:name(e.target), currentTarget:name(e.currentTarget),
    constructor:e.constructor && e.constructor.name, isTrusted:e.isTrusted,
    bubbles:e.bubbles, cancelable:e.cancelable, composed:e.composed,
    defaultPrevented:e.defaultPrevented, key:e.key, code:e.code,
    location:e.location, repeat:e.repeat, isComposing:e.isComposing,
    altKey:e.altKey, ctrlKey:e.ctrlKey, metaKey:e.metaKey, shiftKey:e.shiftKey,
    keyCode:e.keyCode, charCode:e.charCode, which:e.which,
    data:e.data === undefined ? null : e.data,
    inputType:e.inputType === undefined ? null : e.inputType,
    targetValue:e.target && typeof e.target.value === 'string' ? e.target.value : null,
    targetSelectionStart:e.target && typeof e.target.selectionStart === 'number' ? e.target.selectionStart : null,
    targetSelectionEnd:e.target && typeof e.target.selectionEnd === 'number' ? e.target.selectionEnd : null,
    targetMaxlength:e.target && e.target.getAttribute ? e.target.getAttribute('maxlength') : null});
  globalThis.__resetKeyProbe = () => { globalThis.__keyEvents=[];
    globalThis.__poisonCalls={KeyboardEvent:0,InputEvent:0,Event:0,dispatchEvent:0}; };
  globalThis.__snapshotKeyProbe = () => ({events:globalThis.__keyEvents,
    poisonCalls:globalThis.__poisonCalls, active:document.activeElement && document.activeElement.id,
    values:Object.fromEntries(['edit','other','ro'].map(id => { const n=document.getElementById(id);
      return [id,{value:n.value,start:n.selectionStart,end:n.selectionEnd}]; }))});
  for (const node of nodes) for (const type of ['keydown','keypress','beforeinput','input','keyup','change'])
    node.addEventListener(type, e => globalThis.__keyEvents.push(read(e)), true);
  globalThis.__resetKeyProbe();
  globalThis.__keyProbeReady = true;
})();
</script>'''


class FixtureCapture:
    def __init__(self, directory: Path):
        self.directory = directory
        directory.mkdir(parents=True, exist_ok=True)
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
        record.update(requestNumber=number, rawRequestPath=str(raw_path))
        self._append(self.request_jsonl, record)
        self.stdout(json.dumps(record, separators=(",", ":"), ensure_ascii=True))
        return number

    def response(self, record: dict[str, Any], body: bytes, headers: bytes) -> None:
        number = record["requestNumber"]
        raw_path = self.directory / f"fixture-response-{number:04d}.bin"
        raw_path.write_bytes(headers + body)
        record.update(rawResponsePath=str(raw_path),
                      rawHeadersBase64=base64.b64encode(headers).decode(),
                      bodyBase64=base64.b64encode(body).decode())
        self._append(self.response_jsonl, record)

    def metadata(self) -> dict[str, str]:
        return {"requestRecordsPath": str(self.request_jsonl),
                "responseRecordsPath": str(self.response_jsonl),
                "stdoutPath": str(self.stdout_path),
                "fixtureHtmlPath": str(self.html_path)}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "ObscuraOB027KeyboardFixture/1"
    sys_version = ""

    def setup(self) -> None:
        super().setup()
        self.rfile = _WireReader(self.rfile)

    def end_headers(self) -> None:
        self._response_headers = b"".join(self._headers_buffer) + b"\r\n"
        super().end_headers()

    def do_GET(self) -> None:
        path = self.path.split("?", 1)[0]
        body = HTML.encode() if path == "/keyboard" else b"not found"
        status = 200 if path == "/keyboard" else 404
        content_type = "text/html; charset=utf-8" if status == 200 else "text/plain"
        wire = self.rfile
        request_body = wire.read(int(self.headers.get("Content-Length", "0") or "0"))
        record = {"serverOrigin": self.server.origin, "method": self.command,
                  "path": self.path,
                  "headers":[{"name":n,"value":v} for n,v in self.headers.raw_items()],
                  "rawHeadersBase64":base64.b64encode(wire.header_bytes).decode(),
                  "bodyBase64":base64.b64encode(request_body).decode()}
        number = self.server.capture.request(record, wire.header_bytes + request_body)
        wire.finish_request()
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)
        self.server.capture.response({"requestNumber":number,"status":status,
                                      "contentType":content_type,"bodyLength":len(body)},
                                     body, self._response_headers)

    def log_message(self, fmt: str, *args: Any) -> None:
        self.server.capture.stdout("%s - - [%s] %s" %
                                   (self.address_string(), self.log_date_time_string(), fmt % args))


@contextmanager
def fixture_server(directory: Path) -> Iterator[FixtureCapture]:
    capture = FixtureCapture(directory)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.capture = capture  # type: ignore[attr-defined]
    host, port = server.server_address
    capture.origin = f"http://{host}:{port}"  # type: ignore[attr-defined]
    server.origin = capture.origin  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield capture
    finally:
        server.shutdown(); server.server_close(); thread.join(timeout=2)


def _checkpoint(path: Path, result: dict[str, Any]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(result, indent=2, ensure_ascii=True) + "\n", encoding="utf-8")
    temporary.replace(path)


def _reset(page: Any) -> None:
    page.evaluate("""html => { document.open(); document.write(html); document.close(); }""", HTML)


def _select_range(page: Any, node: str = "#edit", start: int = 1, end: int = 3) -> None:
    page.evaluate("""args => { const n = document.querySelector(args.node);
      n.focus(); n.setSelectionRange(args.start, args.end); }""",
                  {"node": node, "start": start, "end": end})


def _dispatch(
    page: Any,
    params: dict[str, Any],
    method: str = "Input.dispatchKeyEvent",
) -> dict[str, Any]:
    session = page.context.new_cdp_session(page)
    try:
        response: Any = None; action_error: dict[str, Any] | None = None
        try: response = session.send(method, params)
        except Exception as error: action_error = error_record(error)
        return {"method": method, "params": params, "response": response, "actionError": action_error}
    finally:
        session.detach()


def _snapshot(page: Any) -> dict[str, Any]:
    return page.evaluate("globalThis.__snapshotKeyProbe()")


def _insert_text(page: Any) -> dict[str, Any]:
    _select_range(page)
    page.evaluate("globalThis.__resetKeyProbe()")
    replacement_action = _dispatch(page, {"text":"XY"}, "Input.insertText")
    replacement = _snapshot(page)
    page.reload(); _select_range(page)
    page.evaluate("globalThis.__resetKeyProbe()")
    empty_selection = {"action": _dispatch(page, {"text":""}, "Input.insertText"),
                       "snapshot": _snapshot(page)}
    _select_range(page, "#ro")
    page.evaluate("globalThis.__resetKeyProbe()")
    readonly = {"action": _dispatch(page, {"text":"Q"}, "Input.insertText"),
                "snapshot": _snapshot(page)}
    page.locator("#button").focus()
    page.evaluate("globalThis.__resetKeyProbe()")
    noneditable = {"action": _dispatch(page, {"text":"Q"}, "Input.insertText"),
                   "snapshot": _snapshot(page)}
    return {"action":replacement_action, "snapshot":replacement,
            "emptySelection":empty_selection, "readonly":readonly, "noneditable":noneditable}


def _key_phases(page: Any) -> dict[str, Any]:
    _select_range(page)
    page.evaluate("globalThis.__resetKeyProbe()")
    actions = [_dispatch(page, {"type":"keyDown", "key":"x", "code":"KeyX",
                                "text":"x", "unmodifiedText":"x", "windowsVirtualKeyCode":88}),
               _dispatch(page, {"type":"rawKeyDown", "key":"x", "code":"KeyX",
                                "text":"x", "windowsVirtualKeyCode":88}),
               _dispatch(page, {"type":"char", "text":"y", "unmodifiedText":"y"}),
               _dispatch(page, {"type":"keyUp", "key":"y", "code":"KeyY",
                                "windowsVirtualKeyCode":89})]
    return {"actions":actions, "snapshot":_snapshot(page)}


def _cancellation(page: Any) -> dict[str, Any]:
    results: dict[str, Any] = {}
    for phase in ("keydown", "keypress", "beforeinput"):
        page.reload(); _select_range(page)
        page.evaluate("""phase => { globalThis.__resetKeyProbe();
          document.getElementById('edit').addEventListener(phase, e => e.preventDefault(), {once:true});
        }""", phase)
        results[phase] = {"action":_dispatch(page, {"type":"keyDown","key":"x","code":"KeyX",
                                           "text":"x","unmodifiedText":"x","windowsVirtualKeyCode":88}),
                          "snapshot":_snapshot(page)}
    page.reload(); _select_range(page)
    page.evaluate("""() => { globalThis.__resetKeyProbe();
      document.getElementById('edit').addEventListener(
        'beforeinput', event => event.preventDefault(), {once:true});
    }""")
    results["insertText"] = {"action":_dispatch(page, {"text":"XY"}, "Input.insertText"),
                              "snapshot":_snapshot(page)}
    return results


def _metadata(page: Any) -> dict[str, Any]:
    _select_range(page, start=0, end=0); page.evaluate("globalThis.__resetKeyProbe()")
    actions = [_dispatch(page, {"type":"rawKeyDown","key":"X","code":"KeyX","text":"x",
                                "unmodifiedText":"x","windowsVirtualKeyCode":88,
                                "modifiers":13,"autoRepeat":True,"location":2,"isKeypad":True}),
               _dispatch(page, {"type":"keyUp","key":"X","code":"KeyX",
                                "windowsVirtualKeyCode":88,"location":2})]
    return {"actions":actions, "snapshot":_snapshot(page)}


def _poison(page: Any) -> dict[str, Any]:
    _select_range(page, start=0, end=0)
    page.evaluate("""() => { globalThis.KeyboardEvent = () => { __poisonCalls.KeyboardEvent++; throw Error('poison'); };
      globalThis.InputEvent = () => { __poisonCalls.InputEvent++; throw Error('poison'); };
      globalThis.Event = () => { __poisonCalls.Event++; throw Error('poison'); };
      EventTarget.prototype.dispatchEvent = () => { __poisonCalls.dispatchEvent++; throw Error('poison'); };
      globalThis.__resetKeyProbe(); }""")
    action = _dispatch(page, {"type":"keyDown","key":"x","code":"KeyX","text":"x",
                               "unmodifiedText":"x","windowsVirtualKeyCode":88})
    return {"action":action, "snapshot":_snapshot(page)}


def _reentrancy(page: Any) -> dict[str, Any]:
    _select_range(page)
    page.evaluate("""() => { globalThis.__resetKeyProbe();
      const edit = document.getElementById('edit');
      const other = document.getElementById('other');
      edit.addEventListener('keydown', () => {
        other.focus(); other.setSelectionRange(0, 0);
      }, {once:true}); }""")
    focus_action = _dispatch(page, {"type":"keyDown","key":"x","code":"KeyX","text":"x",
                                    "unmodifiedText":"x","windowsVirtualKeyCode":88})
    focus_snapshot = _snapshot(page)

    page.reload(); _select_range(page)
    page.evaluate("""() => { document.getElementById('edit').addEventListener(
      'beforeinput', () => {
        document.open(); document.write('<input id="next" value="new">'); document.close();
        const next = document.getElementById('next'); next.focus(); next.setSelectionRange(0, 0);
      }, {once:true});
    }""")
    document_action = _dispatch(page, {"text":"XY"}, "Input.insertText")
    document_snapshot = page.evaluate("""() => ({
      active: document.activeElement && document.activeElement.id,
      value: document.getElementById('next') && document.getElementById('next').value
    })""")
    return {"focusAction":focus_action, "focusSnapshot":focus_snapshot,
            "documentAction":document_action, "documentSnapshot":document_snapshot}


def _prepare_maxlength(
    page: Any,
    *,
    node: str,
    maxlength: str,
    value: str,
    start: int,
    end: int,
    beforeinput_hook: str | None = None,
) -> None:
    page.reload()
    page.evaluate(
        """args => {
          const target = document.querySelector(args.node);
          target.setAttribute('maxlength', args.maxlength);
          target.value = args.value;
          target.focus();
          target.setSelectionRange(args.start, args.end);
          globalThis.__resetKeyProbe();
          if (args.beforeinputHook) {
            target.addEventListener('beforeinput', Function(args.beforeinputHook), {once:true});
          }
        }""",
        {
            "node": node,
            "maxlength": maxlength,
            "value": value,
            "start": start,
            "end": end,
            "beforeinputHook": beforeinput_hook,
        },
    )


def _maxlength(page: Any) -> dict[str, Any]:
    records: dict[str, Any] = {}

    _prepare_maxlength(page, node="#edit", maxlength="4", value="", start=0, end=0)
    records["partial"] = {
        "action": _dispatch(page, {"text": "A😀BC"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="0", value="", start=0, end=0)
    records["zero"] = {
        "action": _dispatch(page, {"text": "A"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="4", value="ABCD", start=1, end=3)
    records["selection"] = {
        "action": _dispatch(page, {"text": "😀X"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(
        page,
        node="#edit",
        maxlength="4",
        value="X",
        start=1,
        end=1,
        beforeinput_hook="document.getElementById('edit').maxLength=2",
    )
    records["dynamicShrink"] = {
        "action": _dispatch(page, {"text": "ABC"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(
        page,
        node="#edit",
        maxlength="2",
        value="X",
        start=1,
        end=1,
        beforeinput_hook="document.getElementById('edit').maxLength=4",
    )
    records["dynamicGrow"] = {
        "action": _dispatch(page, {"text": "ABC"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(
        page,
        node="#edit",
        maxlength="4",
        value="ABCD",
        start=4,
        end=4,
        beforeinput_hook=(
            "const target=document.getElementById('edit');"
            "target.value='Q';target.setSelectionRange(1,1)"
        ),
    )
    records["reentryValueSelection"] = {
        "action": _dispatch(page, {"text": "XY"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="3", value="ABCD", start=1, end=4)
    records["overlongDelete"] = {
        "action": _dispatch(page, {"text": ""}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="2", value="ABC", start=1, end=2)
    records["selectionNoCapacity"] = {
        "action": _dispatch(page, {"text": "X"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="4suffix", value="", start=0, end=0)
    records["parsedPrefix"] = {
        "action": _dispatch(page, {"text": "ABCDE"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="-1", value="", start=0, end=0)
    records["negativeUnbounded"] = {
        "action": _dispatch(page, {"text": "ABCDE"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="2", value="", start=0, end=0)
    records["inputNewline"] = {
        "action": _dispatch(page, {"text": "A\r\nB"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#other", maxlength="3", value="", start=0, end=0)
    records["textarea"] = {
        "action": _dispatch(page, {"text": "A\r\nB😀"}, "Input.insertText"),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="4", value="ABCD", start=1, end=3)
    records["keyText"] = {
        "action": _dispatch(
            page,
            {
                "type": "keyDown",
                "key": "",
                "code": "",
                "text": "😀X",
                "unmodifiedText": "😀X",
                "windowsVirtualKeyCode": 0,
            },
        ),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="2", value="ABC", start=1, end=2)
    records["keyTextNoCapacity"] = {
        "action": _dispatch(
            page,
            {
                "type": "keyDown",
                "key": "",
                "code": "",
                "text": "X",
                "unmodifiedText": "X",
                "windowsVirtualKeyCode": 0,
            },
        ),
        "snapshot": _snapshot(page),
    }

    enter = {
        "type": "keyDown",
        "key": "Enter",
        "code": "Enter",
        "text": "\r",
        "unmodifiedText": "\r",
        "windowsVirtualKeyCode": 13,
    }
    _prepare_maxlength(page, node="#other", maxlength="3", value="ABC", start=3, end=3)
    records["lineBreakFull"] = {
        "action": _dispatch(page, enter),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#other", maxlength="3", value="AB", start=2, end=2)
    records["lineBreakOneCapacity"] = {
        "action": _dispatch(page, enter),
        "snapshot": _snapshot(page),
    }

    _prepare_maxlength(page, node="#edit", maxlength="4", value="", start=0, end=0)
    fill_error: dict[str, Any] | None = None
    try:
        page.locator("#edit").fill("A😀BC")
    except Exception as error:
        fill_error = error_record(error)
    records["locatorFill"] = {
        "action": {"method": "Locator.fill", "actionError": fill_error},
        "snapshot": _snapshot(page),
    }
    return records


def _protocol_negative(page: Any) -> dict[str, Any]:
    cases = [
        ("Input.dispatchKeyEvent", {"type":"keyDown"}),
        ("Input.dispatchKeyEvent", {"type":"keyDown","modifiers":"bad"}),
        ("Input.dispatchKeyEvent", {"type":"keyDown","location":"bad"}),
        ("Input.insertText", {}),
        ("Input.insertText", {"text":None}),
        ("Input.insertText", {"text":3}),
        ("Input.dispatchKeyEvent", {"type":"notAKeyEvent"}),
    ]
    return {"actions":[_dispatch(page, params, method) for method, params in cases],
            "snapshot":_snapshot(page)}


def run_case(page: Any, case: str) -> dict[str, Any]:
    if case == "insert-text": return _insert_text(page)
    if case == "key-phases": return _key_phases(page)
    if case == "cancellation": return _cancellation(page)
    if case == "metadata": return _metadata(page)
    if case == "poison": return _poison(page)
    if case == "reentrancy": return _reentrancy(page)
    if case == "maxlength": return _maxlength(page)
    if case == "protocol-negative": return _protocol_negative(page)
    raise ValueError(case)


def assert_contract(case: str, observation: dict[str, Any], *, label: str) -> None:
    """Stable Chrome guards; raw observations remain in the worker result."""
    events = observation.get("snapshot", {}).get("events", [])
    if case == "insert-text":
        for record in (observation, observation["emptySelection"], observation["readonly"], observation["noneditable"]):
            action = record.get("action")
            if not isinstance(action, dict) or action.get("actionError"):
                raise AssertionError(f"{label}: insertText action failed: {action!r}")
        snap = observation["snapshot"]; edit = snap["values"]["edit"]
        if edit["value"] != "aXYd" or edit["start"] != 3 or edit["end"] != 3:
            raise AssertionError(f"{label}: insertText selection contract: {observation!r}")
        empty = observation["emptySelection"]["snapshot"]
        if empty["values"]["edit"]["value"] != "ad" \
                or empty["values"]["edit"]["start"] != 1 \
                or empty["values"]["edit"]["end"] != 1:
            raise AssertionError(f"{label}: empty insertText did not delete selection")
        if not any(e.get("type") == "beforeinput" and e.get("data") == "" for e in empty["events"]):
            raise AssertionError(f"{label}: empty insertText did not produce beforeinput")
        if observation["readonly"]["snapshot"]["values"]["ro"]["value"] != "readonly":
            raise AssertionError(f"{label}: readonly input mutated")
        readonly_events = observation["readonly"]["snapshot"]["events"]
        if not any(e.get("type") == "beforeinput" for e in readonly_events) \
                or any(e.get("type") == "input" for e in readonly_events):
            raise AssertionError(f"{label}: readonly event contract differs")
        noneditable_events = observation["noneditable"]["snapshot"]["events"]
        if not any(e.get("type") == "beforeinput" for e in noneditable_events) \
                or any(e.get("type") == "input" for e in noneditable_events):
            raise AssertionError(f"{label}: noneditable beforeinput contract differs")
    elif case == "key-phases":
        if any(action.get("actionError") for action in observation["actions"]):
            raise AssertionError(f"{label}: key phase action failed: {observation['actions']!r}")
        types = [e["type"] for e in events]
        expected = [event_type for event_type in (
            "keydown", "keypress", "beforeinput", "input",
            "keydown", "keypress", "beforeinput", "input", "keyup",
        ) for _ in range(3)]
        if types != expected:
            raise AssertionError(f"{label}: phase sequence differs: {types!r}")
        if observation["snapshot"]["values"]["edit"]["value"] != "axy d".replace(" ", ""):
            raise AssertionError(f"{label}: phase editing differs")
    elif case == "cancellation":
        for phase, record in observation.items():
            if record["action"].get("actionError"):
                raise AssertionError(f"{label}: {phase} action failed: {record['action']!r}")
            if record["snapshot"]["values"]["edit"]["value"] != "abcd":
                raise AssertionError(f"{label}: {phase} cancellation mutated input")
            types = [event["type"] for event in record["snapshot"]["events"]]
            expected_terminal = "beforeinput" if phase in ("beforeinput", "insertText") else phase
            if expected_terminal not in types:
                raise AssertionError(f"{label}: {phase} cancellation event missing: {types!r}")
            allowed = {
                "keydown": {"keydown"},
                "keypress": {"keydown", "keypress"},
                "beforeinput": {"keydown", "keypress", "beforeinput"},
                "insertText": {"beforeinput"},
            }[phase]
            if not set(types).issubset(allowed):
                raise AssertionError(f"{label}: {phase} cancellation leaked downstream: {types!r}")
    elif case == "metadata":
        if any(action.get("actionError") for action in observation["actions"]):
            raise AssertionError(f"{label}: metadata action failed: {observation['actions']!r}")
        keydown = next((e for e in events if e.get("type") == "keydown"), {})
        for key, value in {"isTrusted":True,"bubbles":True,"cancelable":True,"composed":True,
                           "location":3,"repeat":True,"keyCode":88,
                           "altKey":True,"ctrlKey":False,"metaKey":True,"shiftKey":True}.items():
            if keydown.get(key) != value: raise AssertionError(f"{label}: metadata {key}={keydown.get(key)!r}")
    elif case == "poison":
        if observation["action"].get("actionError"):
            raise AssertionError(f"{label}: poisoned action failed: {observation['action']!r}")
        calls = observation["snapshot"].get("poisonCalls", {})
        expected_calls = {"KeyboardEvent":0, "InputEvent":0, "Event":0, "dispatchEvent":0}
        if calls != expected_calls:
            raise AssertionError(f"{label}: public hook poisoned or counters missing: {calls!r}")
        if observation["snapshot"]["values"]["edit"]["value"] != "xabcd" \
                or not all(kind in [event["type"] for event in events]
                           for kind in ("keydown", "keypress", "beforeinput", "input")):
            raise AssertionError(f"{label}: protected input did not execute: {observation!r}")
    elif case == "reentrancy":
        for action in (observation["focusAction"], observation["documentAction"]):
            if action.get("actionError"):
                raise AssertionError(f"{label}: reentrant action failed: {action!r}")
        focus = observation["focusSnapshot"]
        if focus["active"] != "other" or focus["values"]["edit"]["value"] != "abcd" \
                or focus["values"]["other"]["value"] != "xnew":
            raise AssertionError(f"{label}: keydown focus transfer differs: {observation!r}")
        document = observation["documentSnapshot"]
        if document != {"active":"next", "value":"XYnew"}:
            raise AssertionError(f"{label}: document.open retarget differs: {observation!r}")
    elif case == "maxlength":
        expected_values = {
            "partial": ("edit", "A😀B", 4),
            "zero": ("edit", "", 0),
            "selection": ("edit", "A😀D", 4),
            "dynamicShrink": ("edit", "XA", 2),
            "dynamicGrow": ("edit", "XABC", 4),
            "reentryValueSelection": ("edit", "QXY", 3),
            "overlongDelete": ("edit", "A", 1),
            "selectionNoCapacity": ("edit", "AC", 2),
            "parsedPrefix": ("edit", "ABCD", 4),
            "negativeUnbounded": ("edit", "ABCDE", 5),
            "inputNewline": ("edit", "A ", 2),
            "textarea": ("other", "A\nB", 3),
            "keyText": ("edit", "A😀D", 3),
            "keyTextNoCapacity": ("edit", "AC", 1),
            "lineBreakFull": ("other", "ABC", 3),
            "lineBreakOneCapacity": ("other", "AB\n", 3),
            "locatorFill": ("edit", "A😀B", 4),
        }
        for name, (node, value, caret) in expected_values.items():
            record = observation.get(name, {})
            action = record.get("action", {})
            if action.get("actionError"):
                raise AssertionError(f"{label}: maxlength {name} action failed: {action!r}")
            snapshot = record.get("snapshot", {})
            field = snapshot.get("values", {}).get(node, {})
            if (field.get("value"), field.get("start"), field.get("end")) != (value, caret, caret):
                raise AssertionError(f"{label}: maxlength {name} value/selection differs: {record!r}")
        expected_target_events = {
            "partial": [("beforeinput", "A😀BC"), ("input", "A😀B")],
            "zero": [("beforeinput", "A")],
            "selection": [("beforeinput", "😀X"), ("input", "😀")],
            "dynamicShrink": [("beforeinput", "ABC"), ("input", "A")],
            "dynamicGrow": [("beforeinput", "ABC"), ("input", "ABC")],
            "reentryValueSelection": [("beforeinput", "XY"), ("input", "XY")],
            "overlongDelete": [("beforeinput", ""), ("input", "")],
            "selectionNoCapacity": [("beforeinput", "X"), ("input", "")],
            "parsedPrefix": [("beforeinput", "ABCDE"), ("input", "ABCD")],
            "negativeUnbounded": [("beforeinput", "ABCDE"), ("input", "ABCDE")],
            "inputNewline": [("beforeinput", "A\r\nB"), ("input", "A ")],
            "textarea": [
                ("beforeinput", "A\r\nB😀"),
                ("input", "A"),
                ("input", None),
                ("input", "B"),
            ],
            "keyText": [
                ("keydown", None),
                ("keypress", None),
                ("beforeinput", "😀X"),
                ("input", "😀"),
            ],
            "keyTextNoCapacity": [
                ("keydown", None),
                ("keypress", None),
                ("beforeinput", "X"),
                ("input", ""),
            ],
            "lineBreakFull": [
                ("keydown", None),
                ("keypress", None),
                ("beforeinput", None),
            ],
            "lineBreakOneCapacity": [
                ("keydown", None),
                ("keypress", None),
                ("beforeinput", None),
                ("input", None),
            ],
            "locatorFill": [("beforeinput", "A😀BC"), ("input", "A😀B")],
        }
        for name, expected in expected_target_events.items():
            node = expected_values[name][0]
            events = observation[name]["snapshot"].get("events", [])
            actual = [
                (event.get("type"), event.get("data"))
                for event in events
                if event.get("currentTarget") == node
            ]
            if actual != expected:
                raise AssertionError(f"{label}: maxlength {name} events differ: {actual!r}")
        expected_event_states = {
            "selection": [("ABCD", 1, 3, "4"), ("A😀D", 4, 4, "4")],
            "dynamicShrink": [("X", 1, 1, "4"), ("XA", 2, 2, "2")],
            "dynamicGrow": [("X", 1, 1, "2"), ("XABC", 4, 4, "4")],
            "reentryValueSelection": [("ABCD", 4, 4, "4"), ("QXY", 3, 3, "4")],
            "textarea": [
                ("", 0, 0, "3"),
                ("A\nB", 3, 3, "3"),
                ("A\nB", 3, 3, "3"),
                ("A\nB", 3, 3, "3"),
            ],
            "keyTextNoCapacity": [
                ("ABC", 1, 2, "2"),
                ("ABC", 1, 2, "2"),
                ("ABC", 1, 2, "2"),
                ("AC", 1, 1, "2"),
            ],
        }
        for name, expected in expected_event_states.items():
            node = expected_values[name][0]
            events = observation[name]["snapshot"].get("events", [])
            actual = [
                (
                    event.get("targetValue"),
                    event.get("targetSelectionStart"),
                    event.get("targetSelectionEnd"),
                    event.get("targetMaxlength"),
                )
                for event in events
                if event.get("currentTarget") == node
            ]
            if actual != expected:
                raise AssertionError(f"{label}: maxlength {name} event states differ: {actual!r}")
    elif case == "protocol-negative":
        expected_errors = [False, True, True, True, True, True, True]
        actual_errors = [bool(item.get("actionError")) for item in observation["actions"]]
        if actual_errors != expected_errors:
            raise AssertionError(f"{label}: protocol validation differs: {actual_errors!r}")
        for item in observation["actions"][1:]:
            error = item["actionError"]
            marker = f"Protocol error ({item['method']}):"
            if error.get("type") != "Error" or marker not in error.get("message", ""):
                raise AssertionError(f"{label}: non-protocol failure accepted: {item!r}")


def compare_case(case: str, reference: dict[str, Any], candidate: dict[str, Any], *, label: str) -> None:
    if case == "protocol-negative":
        if [bool(x.get("actionError")) for x in reference["actions"]] != [bool(x.get("actionError")) for x in candidate["actions"]]:
            raise AssertionError(f"{label}: protocol negative result differs")
        return
    if case == "reentrancy":
        for key in ("focusSnapshot", "documentSnapshot"):
            if reference.get(key) != candidate.get(key):
                raise AssertionError(f"{label}: {key} differs")
        return
    if case == "maxlength":
        for name in (
            "partial",
            "zero",
            "selection",
            "dynamicShrink",
            "dynamicGrow",
            "reentryValueSelection",
            "overlongDelete",
            "selectionNoCapacity",
            "parsedPrefix",
            "negativeUnbounded",
            "inputNewline",
            "textarea",
            "keyText",
            "keyTextNoCapacity",
            "lineBreakFull",
            "lineBreakOneCapacity",
            "locatorFill",
        ):
            ref = reference.get(name, {})
            got = candidate.get(name, {})
            if bool(ref.get("action", {}).get("actionError")) != bool(
                got.get("action", {}).get("actionError")
            ) or ref.get("snapshot") != got.get("snapshot"):
                raise AssertionError(f"{label}: maxlength {name} differs")
        return
    if case == "cancellation":
        for phase in ("keydown", "keypress", "beforeinput", "insertText"):
            if reference.get(phase, {}).get("snapshot") != candidate.get(phase, {}).get("snapshot"):
                raise AssertionError(f"{label}: cancellation {phase} differs")
        return
    if case == "insert-text":
        for path in ("snapshot", "emptySelection", "readonly", "noneditable"):
            ref = reference.get(path, {})
            got = candidate.get(path, {})
            if path != "snapshot":
                ref = ref.get("snapshot", {})
                got = got.get("snapshot", {})
            if ref != got:
                raise AssertionError(f"{label}: insert-text {path} differs")
        return
    ref = reference.get("snapshot", {}); got = candidate.get("snapshot", {})
    for key in ("events", "active", "values"):
        if ref.get(key) != got.get(key): raise AssertionError(f"{label}: {key} differs")


def reference_worker_result(document: Any) -> dict[str, Any] | None:
    if not isinstance(document, dict):
        return None
    if isinstance(document.get("cases"), dict):
        return document
    worker = document.get("engines", {}).get("chrome", {}).get("workerResult")
    return worker if isinstance(worker, dict) and isinstance(worker.get("cases"), dict) else None


def run_worker(endpoint: str, origin: str, output: Path, cases: list[str]) -> int:
    result: dict[str, Any] = {"schemaVersion":1,"status":"running","endpoint":endpoint,
                              "fixtureOrigin":origin,"connection":"BrowserType.connect_over_cdp",
                              "cases":{},"completedCases":[],"checkpoint":"initial"}
    _checkpoint(output, result)
    try:
        from playwright.sync_api import sync_playwright
        with sync_playwright() as playwright:
            browser = playwright.chromium.connect_over_cdp(endpoint)
            result["browserVersion"] = browser.version
            context = browser.contexts[0] if browser.contexts else browser.new_context()
            try:
                for case in cases:
                    page = context.new_page(); record = {"status":"running","case":case}
                    result["cases"][case] = record
                    try:
                        page.set_viewport_size({"width":800,"height":600})
                        page_errors: list[str] = []
                        page.on("pageerror", lambda error: page_errors.append(str(error)))
                        page.goto(f"{origin}/keyboard?case={case}", wait_until="load")
                        if page.evaluate("globalThis.__keyProbeReady") is not True:
                            raise AssertionError("keyboard fixture probe did not initialize")
                        record["observations"] = run_case(page, case)
                        record["pageErrors"] = page_errors
                        if page_errors:
                            raise AssertionError(f"keyboard fixture page errors: {page_errors!r}")
                        assert_contract(case, record["observations"], label=case)
                        record["status"] = "passed"
                    except Exception as error:
                        record["status"] = "failed"; record["error"] = error_record(error)
                    finally:
                        result["completedCases"] = list(result["cases"]); result["checkpoint"] = case
                        _checkpoint(output, result); page.close()
            finally: browser.close()
        result["status"] = "passed" if all(v.get("status") == "passed" for v in result["cases"].values()) else "failed"
    except Exception as error:
        result["status"] = "failed"; result["error"] = error_record(error)
    result["checkpoint"] = "final"; _checkpoint(output, result)
    print(json.dumps(result, ensure_ascii=True), flush=True)
    return 0 if result["status"] == "passed" else 1


def chrome_command(executable: Path, port: int, profile: Path) -> list[str]:
    return [str(executable), "--headless=new", "--disable-gpu", "--no-first-run",
            "--no-default-browser-check", "--window-size=800,600",
            "--remote-debugging-address=127.0.0.1", f"--remote-debugging-port={port}",
            f"--user-data-dir={profile}", "about:blank"]


def run_engine(engine: str, *, obscura_bin: Path, persona: str, root: Path,
               cases: list[str], chrome_executable: Path | None) -> dict[str, Any]:
    directory = root / engine; directory.mkdir(parents=True, exist_ok=True)
    port = free_port(); endpoint = f"http://127.0.0.1:{port}"
    record: dict[str, Any] = {"engine":engine,"status":"running","browserCapture":{},
                              "persona":persona if engine == "obscura" else None}
    child_output = directory / "worker-result.json"
    try:
        with fixture_server(directory / "fixture") as fixture:
            origin = fixture.origin  # type: ignore[attr-defined]
            record["fixtureCapture"] = fixture.metadata(); record["fixtureCapture"]["origin"] = origin
            if engine == "chrome":
                if chrome_executable is None: raise RuntimeError("Playwright Chromium executable was not found")
                command = chrome_command(chrome_executable, port, directory / "profile")
            else:
                command = [str(obscura_bin.resolve()), "--persona", persona, "--allow-private-network",
                           "serve", "--host", "127.0.0.1", "--port", str(port)]
            record["command"] = command
            with external_process(command, endpoint, capture=record["browserCapture"], log_root=directory):
                record["workerCapture"] = capture_worker(
                    [sys.executable, str(Path(__file__).resolve()), "--worker", "--endpoint", endpoint,
                     "--origin", origin, "--output", str(child_output),
                     *sum((["--case", case] for case in cases), [])], directory, WORKER_TIMEOUT_SECONDS)
                if child_output.exists(): record["workerResult"] = json.loads(child_output.read_text())
                record["status"] = "passed" if record["workerCapture"]["status"] == "passed" and record.get("workerResult", {}).get("status") == "passed" else "failed"
    except Exception as error:
        record["status"] = "failed"; record["error"] = error_record(error)
    finally:
        if child_output.exists():
            try: record["workerResult"] = json.loads(child_output.read_text())
            except Exception as error: record["workerResultReadError"] = error_record(error)
    return record


def run(*, obscura_bin: Path, output: Path, persona: str, engines: list[str], cases: list[str],
        reference_json: Path | None = None,
        chrome_executable_override: Path | None = None) -> dict[str, Any]:
    output.parent.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="ob027-native-keyboard-", dir=output.parent)).resolve()
    result: dict[str, Any] = {"schemaVersion":1,"status":"running","root":str(root),"cases":cases,"engines":{},"comparison":{}}
    try:
        chrome_executable: Path | None = None
        if "chrome" in engines:
            if chrome_executable_override is not None:
                chrome_executable = chrome_executable_override.resolve()
            else:
                from playwright.sync_api import sync_playwright
                with sync_playwright() as playwright:
                    chrome_executable = Path(playwright.chromium.executable_path)
            if not chrome_executable.is_file():
                raise FileNotFoundError(f"Chrome executable does not exist: {chrome_executable}")
        for engine in engines:
            result["engines"][engine] = run_engine(engine, obscura_bin=obscura_bin, persona=persona, root=root, cases=cases, chrome_executable=chrome_executable)
        reference_document = json.loads(reference_json.read_text()) if reference_json else result["engines"].get("chrome", {}).get("workerResult")
        reference = reference_worker_result(reference_document)
        candidate = result["engines"].get("obscura", {}).get("workerResult")
        ref_engine = "chrome" if "chrome" in engines else "reference-json" if reference_json else "embedded Chrome contract"
        if "chrome" not in engines and reference_json is None:
            if not isinstance(candidate, dict):
                raise AssertionError("missing passing Obscura worker result")
            for case in cases:
                item = candidate.get("cases", {}).get(case, {})
                observation = item.get("observations")
                if item.get("status") != "passed" or not isinstance(observation, dict):
                    raise AssertionError(f"obscura:{case} missing observation")
                assert_contract(case, observation, label=f"obscura:{case}")
            result["comparison"] = {"reference":ref_engine,"status":"passed"}
            reference = None
        elif not isinstance(reference, dict) or ("obscura" in engines and not isinstance(candidate, dict)):
            raise AssertionError("missing passing worker result")
        for case in cases if reference is not None else ():
            ref_case = reference.get("cases", {}).get(case, {}); ref_obs = ref_case.get("observations")
            if ref_case.get("status") != "passed" or not isinstance(ref_obs, dict): raise AssertionError(f"reference:{case} missing observation")
            if "chrome" not in engines: assert_contract(case, ref_obs, label=f"reference:{case}")
            if candidate is not None:
                got = candidate.get("cases", {}).get(case, {}); got_obs = got.get("observations")
                if got.get("status") != "passed" or not isinstance(got_obs, dict): raise AssertionError(f"obscura:{case} missing observation")
                compare_case(case, ref_obs, got_obs, label=f"obscura:{case}")
        result["comparison"] = {"reference":ref_engine,"status":"passed"}
    except Exception as error:
        result["comparison"] = {"reference":"chrome" if "chrome" in engines else "embedded Chrome contract","status":"failed","error":error_record(error)}
    result["status"] = "passed" if all(item.get("status") == "passed" for item in result["engines"].values()) and result["comparison"].get("status") == "passed" else "failed"
    _checkpoint(output, result)
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obscura-bin", type=Path, default=Path("target/release/obscura"))
    parser.add_argument("--with-chrome", action="store_true")
    parser.add_argument("--chrome-only", action="store_true")
    parser.add_argument("--reference-json", type=Path)
    parser.add_argument("--chrome-executable", type=Path)
    parser.add_argument("--case", action="append", choices=CASES)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--persona", default=DEFAULT_PERSONA)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--endpoint", help=argparse.SUPPRESS)
    parser.add_argument("--origin", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        if not args.endpoint or not args.origin or not args.case: parser.error("worker needs endpoint, origin, output, and at least one case")
        return run_worker(args.endpoint, args.origin, args.output, args.case)
    if args.reference_json and args.chrome_only: parser.error("--reference-json cannot be combined with --chrome-only")
    cases = args.case or list(CASES)
    engines = ["chrome"] if args.chrome_only else ["obscura"] + (["chrome"] if args.with_chrome else [])
    result = run(
        obscura_bin=args.obscura_bin,
        output=args.output.resolve(),
        persona=args.persona,
        engines=engines,
        cases=cases,
        reference_json=args.reference_json,
        chrome_executable_override=args.chrome_executable,
    )
    print(json.dumps(result, indent=2, ensure_ascii=True)); return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())

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
    "contenteditable",
    "ignore-input",
    "protocol-negative",
)

HTML = r'''<!doctype html>
<meta charset="utf-8"><title>OB-027 native keyboard fixture</title>
<style>input,textarea{font:16px sans-serif} #edit{position:absolute;left:20px;top:20px}
#other{position:absolute;left:20px;top:80px} #ro{position:absolute;left:20px;top:140px}</style>
<style>#scrollbox{position:absolute;left:300px;top:20px;width:120px;height:60px;overflow:auto}#scrollpad{height:500px;width:20px}</style>
<input id="edit" value="abcd"><textarea id="other">new</textarea>
<input id="ro" value="readonly" readonly><button id="button">button</button>
<div id="ce" contenteditable="true">ab<span id="ce-span">CD</span>ef <span id="ce-island" contenteditable="false">LOCK</span> gh</div>
<div id="scrollbox"><div id="scrollpad"></div></div>
<script>
(() => {
    const nodes = ['edit', 'other', 'ro', 'button', 'scrollbox'].map(id => document.getElementById(id))
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
    scrollTop:document.getElementById('scrollbox').scrollTop,
    values:Object.fromEntries(['edit','other','ro'].map(id => { const n=document.getElementById(id);
      return [id,{value:n.value,start:n.selectionStart,end:n.selectionEnd}]; }))});
  const cePath = node => { const result=[];
    for (let current=node; current && current.nodeType; current=current.parentNode) {
      let siblingIndex=0;
      for (let sibling=current.previousSibling; sibling; sibling=sibling.previousSibling) siblingIndex++;
      result.push({nodeName:current.nodeName,id:current.id||null,siblingIndex,
        parentId:current.parentNode&&current.parentNode.id||null,
        parentHTML:current.parentNode&&current.parentNode.outerHTML||null});
    }
    return result;
  };
  const ceSelection = () => { const selection=getSelection();
    const range=selection&&selection.rangeCount?selection.getRangeAt(0):null;
    return {anchorNode:selection.anchorNode&&selection.anchorNode.nodeName,
      anchorOffset:selection.anchorOffset,focusNode:selection.focusNode&&selection.focusNode.nodeName,
      focusOffset:selection.focusOffset,anchorPath:cePath(selection.anchorNode),
      focusPath:cePath(selection.focusNode),range:range?{
        startPath:cePath(range.startContainer),startOffset:range.startOffset,
        endPath:cePath(range.endContainer),endOffset:range.endOffset}:null};
  };
  const ceRead = event => ({type:event.type,target:name(event.target),
    currentTarget:name(event.currentTarget),constructor:event.constructor&&event.constructor.name,
    isTrusted:event.isTrusted,bubbles:event.bubbles,cancelable:event.cancelable,
    composed:event.composed,defaultPrevented:event.defaultPrevented,key:event.key,
    code:event.code,location:event.location,repeat:event.repeat,isComposing:event.isComposing,
    altKey:event.altKey,ctrlKey:event.ctrlKey,metaKey:event.metaKey,shiftKey:event.shiftKey,
    keyCode:event.keyCode,charCode:event.charCode,which:event.which,
    data:event.data===undefined?null:event.data,
    inputType:event.inputType===undefined?null:event.inputType,
    targetOuterHTML:event.target&&event.target.outerHTML||null,
    targetTextContent:event.target&&event.target.textContent,
    selection:ceSelection(),targetRanges:typeof event.getTargetRanges==='function'
      ? Array.from(event.getTargetRanges(), range => ({
          startPath:cePath(range.startContainer),startOffset:range.startOffset,
          endPath:cePath(range.endContainer),endOffset:range.endOffset})) : null});
  globalThis.__resetContenteditableProbe = () => { globalThis.__ceEvents=[]; };
  globalThis.__snapshotContenteditableProbe = () => { const root=document.getElementById('ce');
    return {outerHTML:root.outerHTML,innerHTML:root.innerHTML,textContent:root.textContent,
      active:document.activeElement&&document.activeElement.id,events:globalThis.__ceEvents,
      selection:ceSelection(),controls:Object.fromEntries(['edit','other'].map(id=>{const n=document.getElementById(id);
        return [id,{value:n.value,start:n.selectionStart,end:n.selectionEnd}]}))};
  };
  const ce=document.getElementById('ce');
  for (const type of ['keydown','keypress','beforeinput','input','keyup'])
    ce.addEventListener(type,event=>globalThis.__ceEvents.push(ceRead(event)),true);
  globalThis.__resetContenteditableProbe();
  for (const node of nodes) for (const type of ['keydown','keypress','beforeinput','input','keyup','change','pointermove','pointerdown','mousedown','pointerup','mouseup','click','wheel'])
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
        return _session_dispatch(session, params, method)
    finally:
        session.detach()


def _session_dispatch(
    session: Any,
    params: dict[str, Any],
    method: str = "Input.dispatchKeyEvent",
) -> dict[str, Any]:
    response: Any = None
    action_error: dict[str, Any] | None = None
    try:
        response = session.send(method, params)
    except Exception as error:
        action_error = error_record(error)
    return {"method": method, "params": params, "response": response, "actionError": action_error}


def _snapshot(page: Any) -> dict[str, Any]:
    return page.evaluate("globalThis.__snapshotKeyProbe()")


def _settle_ignore_scroll(page: Any) -> dict[str, Any]:
    return page.evaluate("""() => new Promise(resolve => {
      const samples = [];
      let previous = null, stable = 0;
      const frame = () => {
        const value = document.getElementById('scrollbox').scrollTop;
        samples.push(value);
        stable = value === previous ? stable + 1 : 0;
        previous = value;
        if (stable >= 2 || samples.length >= 30) {
          resolve({samples, stable, finalValue:value});
          return;
        }
        requestAnimationFrame(frame);
      };
      requestAnimationFrame(frame);
    })""")


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


def _prepare_contenteditable(
    page: Any,
    *,
    node: str,
    start: int,
    end: int,
    beforeinput_hook: str | None = None,
) -> None:
    page.reload()
    page.evaluate(
        """args => {
          const root=document.getElementById('ce');
          const text=document.querySelector(args.node).firstChild;
          root.focus();
          const range=document.createRange();
          range.setStart(text,args.start);range.setEnd(text,args.end);
          const selection=getSelection();selection.removeAllRanges();selection.addRange(range);
          globalThis.__resetContenteditableProbe();
          if (args.beforeinputHook) {
            root.addEventListener('beforeinput',Function('event',args.beforeinputHook),{once:true});
          }
        }""",
        {"node":node,"start":start,"end":end,"beforeinputHook":beforeinput_hook},
    )


def _contenteditable(page: Any) -> dict[str, Any]:
    records: dict[str, Any] = {}

    _prepare_contenteditable(page,node="#ce-span",start=1,end=1)
    records["insertText"] = {"action":_dispatch(page,{"text":"X"},"Input.insertText"),
                               "snapshot":page.evaluate("globalThis.__snapshotContenteditableProbe()")}

    _prepare_contenteditable(page,node="#ce-span",start=0,end=2)
    records["selectionReplacement"] = {
        "action":_dispatch(page,{"text":"Y"},"Input.insertText"),
        "snapshot":page.evaluate("globalThis.__snapshotContenteditableProbe()")}

    _prepare_contenteditable(page,node="#ce-span",start=1,end=1)
    records["keyText"] = {"action":_dispatch(page,{"type":"keyDown","key":"x","code":"KeyX",
                                                 "text":"x","unmodifiedText":"x","windowsVirtualKeyCode":88}),
                           "snapshot":page.evaluate("globalThis.__snapshotContenteditableProbe()")}

    _prepare_contenteditable(page,node="#ce-span",start=1,end=1,
                             beforeinput_hook="event.preventDefault()")
    records["cancel"] = {"action":_dispatch(page,{"text":"Q"},"Input.insertText"),
                          "snapshot":page.evaluate("globalThis.__snapshotContenteditableProbe()")}

    _prepare_contenteditable(page,node="#ce-span",start=1,end=1,
                             beforeinput_hook="document.getElementById('ce').append('M')")
    records["domReentry"] = {"action":_dispatch(page,{"text":"Q"},"Input.insertText"),
                              "snapshot":page.evaluate("globalThis.__snapshotContenteditableProbe()")}

    _prepare_contenteditable(page,node="#ce-island",start=2,end=2)
    records["falseIsland"] = {"action":_dispatch(page,{"text":"Z"},"Input.insertText"),
                               "snapshot":page.evaluate("globalThis.__snapshotContenteditableProbe()")}

    _prepare_contenteditable(page,node="#ce-span",start=0,end=2)
    records["emptySelection"] = {"action":_dispatch(page,{"text":""},"Input.insertText"),
                                  "snapshot":page.evaluate("globalThis.__snapshotContenteditableProbe()")}

    _prepare_contenteditable(page,node="#ce-span",start=1,end=1)
    page.evaluate("""() => { const edit=document.getElementById('edit');
      edit.focus(); edit.setSelectionRange(1,1); globalThis.__resetContenteditableProbe(); }""")
    records["focusTransfer"] = {"action":_dispatch(page,{"text":"X"},"Input.insertText"),
                                  "snapshot":page.evaluate("globalThis.__snapshotContenteditableProbe()")}
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
        ("Input.setIgnoreInputEvents", {}),
        ("Input.setIgnoreInputEvents", {"ignore":"true"}),
        ("Input.setIgnoreInputEvents", {"ignore":1}),
        ("Input.setIgnoreInputEvents", {"ignore":None}),
    ]
    return {"actions":[_dispatch(page, params, method) for method, params in cases],
            "snapshot":_snapshot(page)}


def _ignore_input(page: Any) -> dict[str, Any]:
    def select_end(target: Any) -> None:
        target.locator("#edit").focus()
        target.evaluate("""() => { const n=document.querySelector('#edit');
          n.setSelectionRange(n.value.length, n.value.length); }""")

    def active_phase(session: Any, ignore: bool) -> dict[str, Any]:
        select_end(page)
        page.evaluate("globalThis.__resetKeyProbe()")
        before = _snapshot(page)
        actions = [
            _session_dispatch(session, {"ignore": ignore}, "Input.setIgnoreInputEvents"),
            _session_dispatch(session, {"type":"mouseMoved", "x":25, "y":25}, "Input.dispatchMouseEvent"),
            _session_dispatch(session, {"type":"mousePressed", "x":25, "y":25, "button":"left"}, "Input.dispatchMouseEvent"),
            _session_dispatch(session, {"type":"mouseReleased", "x":25, "y":25, "button":"left"}, "Input.dispatchMouseEvent"),
            _session_dispatch(session, {"type":"mouseWheel", "x":330, "y":35, "deltaX":0, "deltaY":40}, "Input.dispatchMouseEvent"),
        ]
        # A successful mouse press can change the caret.  Pin it again before
        # keyboard editing so layout differences cannot masquerade as an
        # ignore-state difference.
        select_end(page)
        actions += [
            _session_dispatch(session, {"type":"keyDown", "key":"a", "code":"KeyA", "text":"a"}),
            _session_dispatch(session, {"type":"keyUp", "key":"a", "code":"KeyA"}),
            _session_dispatch(session, {"text":"x"}, "Input.insertText"),
        ]
        settle = _settle_ignore_scroll(page)
        if settle.get("stable", 0) < 2:
            raise AssertionError(f"ignore-input wheel did not settle: {settle!r}")
        return {"before": before, "actions": actions, "settle":settle,
                "after": _snapshot(page)}

    def key_action(session: Any, text: str) -> dict[str, Any]:
        return _session_dispatch(
            session,
            {"type":"keyDown", "key":text, "code":f"Key{text.upper()}", "text":text},
        )

    page.reload()
    owner = page.context.new_cdp_session(page)
    try:
        true_phase = active_phase(owner, True)
        false_phase = active_phase(owner, False)

        # Each CDP session owns one contribution to the target switch.  The
        # target ignores browser input while any attached session contributes
        # true; false removes only the calling session's contribution.
        page.reload(); select_end(page); page.evaluate("globalThis.__resetKeyProbe()")
        sibling = page.context.new_cdp_session(page)
        try:
            sibling_actions = [
                _session_dispatch(owner, {"ignore": True}, "Input.setIgnoreInputEvents"),
                _session_dispatch(sibling, {"ignore": False}, "Input.setIgnoreInputEvents"),
                key_action(sibling, "b"),
                key_action(owner, "c"),
                _session_dispatch(sibling, {"ignore": True}, "Input.setIgnoreInputEvents"),
                key_action(sibling, "d"),
                _session_dispatch(owner, {"ignore": False}, "Input.setIgnoreInputEvents"),
                key_action(owner, "e"),
            ]
            sibling_snapshot = _snapshot(page)

            # Navigation replaces the document but keeps the attached session,
            # so that session's ignore state remains set.  insertText is
            # deliberately exempt in Chrome.
            page.reload(); select_end(page); page.evaluate("globalThis.__resetKeyProbe()")
            navigation_actions = [
                key_action(sibling, "f"),
                _session_dispatch(sibling, {"text":"n"}, "Input.insertText"),
            ]
            navigation_snapshot = _snapshot(page)
        finally:
            # Detach discards the session-local state.  A replacement session
            # begins enabled even though the old session was ignored.
            sibling.detach()

        replacement = page.context.new_cdp_session(page)
        try:
            page.reload(); select_end(page); page.evaluate("globalThis.__resetKeyProbe()")
            reattach_actions = [
                key_action(replacement, "g"),
                _session_dispatch(replacement, {"text":"r"}, "Input.insertText"),
            ]
            reattach_snapshot = _snapshot(page)

            # A different target is independent and continues accepting key
            # input while the first target remains ignored.
            select_end(page); page.evaluate("globalThis.__resetKeyProbe()")
            other_actions = [
                _session_dispatch(
                    replacement, {"ignore": True}, "Input.setIgnoreInputEvents"
                ),
                key_action(replacement, "i"),
            ]
            first_target_snapshot = _snapshot(page)
            other = page.context.new_page()
            try:
                other.goto(page.url, wait_until="load")
                if other.evaluate("globalThis.__keyProbeReady") is not True:
                    raise AssertionError("secondary ignore-input fixture did not initialize")
                select_end(other); other.evaluate("globalThis.__resetKeyProbe()")
                other_session = page.context.new_cdp_session(other)
                try:
                    other_actions.append(key_action(other_session, "h"))
                    other_snapshot = _snapshot(other)
                finally:
                    other_session.detach()
            finally:
                other.close()
            cleanup_action = _session_dispatch(
                replacement, {"ignore": False}, "Input.setIgnoreInputEvents"
            )
        finally:
            replacement.detach()
    finally:
        if owner is not None:
            try:
                _session_dispatch(owner, {"ignore": False}, "Input.setIgnoreInputEvents")
            finally:
                owner.detach()
    return {
        "true": true_phase,
        "false": false_phase,
        "sibling": {"actions": sibling_actions, "snapshot": sibling_snapshot},
        "navigation": {"actions": navigation_actions, "snapshot": navigation_snapshot},
        "reattach": {"actions": reattach_actions, "snapshot": reattach_snapshot},
        "otherTarget": {
            "actions": other_actions,
            "firstSnapshot": first_target_snapshot,
            "snapshot": other_snapshot,
        },
        "cleanupAction": cleanup_action,
    }


def run_case(page: Any, case: str) -> dict[str, Any]:
    if case == "insert-text": return _insert_text(page)
    if case == "key-phases": return _key_phases(page)
    if case == "cancellation": return _cancellation(page)
    if case == "metadata": return _metadata(page)
    if case == "poison": return _poison(page)
    if case == "reentrancy": return _reentrancy(page)
    if case == "maxlength": return _maxlength(page)
    if case == "contenteditable": return _contenteditable(page)
    if case == "ignore-input": return _ignore_input(page)
    if case == "protocol-negative": return _protocol_negative(page)
    raise ValueError(case)


def _contenteditable_path_contract(path: Any) -> list[dict[str, Any]]:
    projected: list[dict[str, Any]] = []
    if not isinstance(path, list):
        return projected
    for item in path:
        if not isinstance(item, dict):
            projected.append({"invalid": repr(item)})
            break
        projected.append({key:item.get(key) for key in (
            "nodeName","id","siblingIndex","parentId",
        )})
        if item.get("id") == "ce":
            break
    return projected


def _contenteditable_selection_contract(selection: Any) -> dict[str, Any] | None:
    if not isinstance(selection, dict):
        return None
    range_value = selection.get("range")
    range_contract = None
    if isinstance(range_value, dict):
        range_contract = {
            "startPath":_contenteditable_path_contract(range_value.get("startPath")),
            "startOffset":range_value.get("startOffset"),
            "endPath":_contenteditable_path_contract(range_value.get("endPath")),
            "endOffset":range_value.get("endOffset"),
        }
    return {
        "anchorNode":selection.get("anchorNode"),
        "anchorOffset":selection.get("anchorOffset"),
        "focusNode":selection.get("focusNode"),
        "focusOffset":selection.get("focusOffset"),
        "anchorPath":_contenteditable_path_contract(selection.get("anchorPath")),
        "focusPath":_contenteditable_path_contract(selection.get("focusPath")),
        "range":range_contract,
    }


def _contenteditable_event_contract(event: Any) -> dict[str, Any]:
    if not isinstance(event, dict):
        return {"invalid":repr(event)}
    target_ranges = event.get("targetRanges")
    projected_ranges = None
    if isinstance(target_ranges, list):
        projected_ranges = [{
            "startPath":_contenteditable_path_contract(item.get("startPath")),
            "startOffset":item.get("startOffset"),
            "endPath":_contenteditable_path_contract(item.get("endPath")),
            "endOffset":item.get("endOffset"),
        } if isinstance(item, dict) else {"invalid":repr(item)} for item in target_ranges]
    return {
        key:event.get(key) for key in (
            "type","target","currentTarget","constructor","isTrusted","bubbles",
            "cancelable","composed","defaultPrevented","data","inputType","isComposing",
            "targetOuterHTML","targetTextContent",
        )
    } | {
        "selection":_contenteditable_selection_contract(event.get("selection")),
        "targetRanges":projected_ranges,
    }


def _contenteditable_snapshot_contract(snapshot: Any) -> dict[str, Any] | None:
    if not isinstance(snapshot, dict):
        return None
    return {
        "outerHTML":snapshot.get("outerHTML"),
        "innerHTML":snapshot.get("innerHTML"),
        "textContent":snapshot.get("textContent"),
        "active":snapshot.get("active"),
        "controls":snapshot.get("controls"),
        "events":[_contenteditable_event_contract(event) for event in snapshot.get("events", [])],
        "selection":_contenteditable_selection_contract(snapshot.get("selection")),
    }


def _contenteditable_selection_point(selection: Any) -> tuple[Any, Any, Any, Any]:
    contract = _contenteditable_selection_contract(selection) or {}
    anchor_path = contract.get("anchorPath") or []
    focus_path = contract.get("focusPath") or []
    anchor_parent = anchor_path[0].get("parentId") if anchor_path else None
    focus_parent = focus_path[0].get("parentId") if focus_path else None
    return (anchor_parent,contract.get("anchorOffset"),focus_parent,contract.get("focusOffset"))


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
    elif case == "contenteditable":
        expected = {
            "insertText": {
                "html":"ab<span id=\"ce-span\">CXD</span>ef <span id=\"ce-island\" contenteditable=\"false\">LOCK</span> gh",
                "text":"abCXDef LOCK gh","events":["beforeinput","input"],
                "initial":("ce-span",1,1),"final":("ce-span",2,2),"data":"X",
            },
            "selectionReplacement": {
                "html":"ab<span id=\"ce-span\">Y</span>ef <span id=\"ce-island\" contenteditable=\"false\">LOCK</span> gh",
                "text":"abYef LOCK gh","events":["beforeinput","input"],
                "initial":("ce-span",0,2),"final":("ce-span",1,1),"data":"Y",
            },
            "keyText": {
                "html":"ab<span id=\"ce-span\">CxD</span>ef <span id=\"ce-island\" contenteditable=\"false\">LOCK</span> gh",
                "text":"abCxDef LOCK gh","events":["keydown","keypress","beforeinput","input"],
                "initial":("ce-span",1,1),"final":("ce-span",2,2),"data":"x",
            },
            "cancel": {
                "html":"ab<span id=\"ce-span\">CD</span>ef <span id=\"ce-island\" contenteditable=\"false\">LOCK</span> gh",
                "text":"abCDef LOCK gh","events":["beforeinput"],
                "initial":("ce-span",1,1),"final":("ce-span",1,1),"data":"Q",
            },
            "domReentry": {
                "html":"ab<span id=\"ce-span\">CQD</span>ef <span id=\"ce-island\" contenteditable=\"false\">LOCK</span> ghM",
                "text":"abCQDef LOCK ghM","events":["beforeinput","input"],
                "initial":("ce-span",1,1),"final":("ce-span",2,2),"data":"Q",
            },
            "falseIsland": {
                "html":"ab<span id=\"ce-span\">CD</span>ef <span id=\"ce-island\" contenteditable=\"false\">LOCK</span> gh",
                "text":"abCDef LOCK gh","events":["beforeinput"],
                "initial":("ce-island",2,2),"final":("ce-island",2,2),"data":"Z",
            },
            "emptySelection": {
                "html":"abef <span id=\"ce-island\" contenteditable=\"false\">LOCK</span> gh",
                "text":"abef LOCK gh","events":["beforeinput","input"],
                "initial":("ce-span",0,2),"final":("ce",2,2),"data":"",
            },
            "focusTransfer": {
                "html":"ab<span id=\"ce-span\">CD</span>ef <span id=\"ce-island\" contenteditable=\"false\">LOCK</span> gh",
                "text":"abCDef LOCK gh","events":[],"initial":None,"final":None,"data":None,
            },
        }

        def selection_matches(selection: Any, point: tuple[str, int, int]) -> bool:
            parent, start, end = point
            contract = _contenteditable_selection_contract(selection) or {}
            anchor = contract.get("anchorPath") or []
            focus = contract.get("focusPath") or []
            range_value = contract.get("range") or {}
            range_start = range_value.get("startPath") or []
            range_end = range_value.get("endPath") or []
            return (
                contract.get("anchorNode") == "#text"
                and contract.get("focusNode") == "#text"
                and contract.get("anchorOffset") == start
                and contract.get("focusOffset") == end
                and anchor and anchor[0].get("parentId") == parent
                and focus and focus[0].get("parentId") == parent
                and range_value.get("startOffset") == start
                and range_value.get("endOffset") == end
                and range_start and range_start[0].get("parentId") == parent
                and range_end and range_end[0].get("parentId") == parent
            )

        for name, expectation in expected.items():
            record = observation.get(name, {})
            action = record.get("action", {})
            if action.get("response") != {} or action.get("actionError") is not None:
                raise AssertionError(f"{label}: contenteditable {name} action failed: {action!r}")
            snapshot = record.get("snapshot", {})
            actual_types = [event.get("type") for event in snapshot.get("events", [])]
            if snapshot.get("innerHTML") != expectation["html"] \
                    or snapshot.get("textContent") != expectation["text"] \
                    or actual_types != expectation["events"]:
                raise AssertionError(f"{label}: contenteditable {name} differs: {record!r}")
            active = "edit" if name == "focusTransfer" else "ce"
            if snapshot.get("active") != active:
                raise AssertionError(f"{label}: contenteditable {name} active target differs")
            if name == "focusTransfer":
                control = snapshot.get("controls", {}).get("edit")
                if control != {"value":"aXbcd","start":2,"end":2}:
                    raise AssertionError(f"{label}: contenteditable focusTransfer control differs")
                continue
            if not selection_matches(snapshot.get("selection"), expectation["final"]):
                raise AssertionError(f"{label}: contenteditable {name} final selection differs")
            for event in snapshot.get("events", []):
                event_type = event.get("type")
                constructor = "KeyboardEvent" if event_type in ("keydown","keypress") else "InputEvent"
                if any((
                    event.get("target") != "ce",
                    event.get("currentTarget") != "ce",
                    event.get("constructor") != constructor,
                    event.get("isTrusted") is not True,
                    event.get("bubbles") is not True,
                    event.get("cancelable") is not (event_type != "input"),
                    event.get("composed") is not True,
                    event.get("defaultPrevented") is not False,
                    event.get("isComposing") is not False,
                )):
                    raise AssertionError(f"{label}: contenteditable {name} event contract differs: {event!r}")
                expected_point = expectation["final"] if event_type == "input" else expectation["initial"]
                if not selection_matches(event.get("selection"), expected_point):
                    raise AssertionError(f"{label}: contenteditable {name} event selection differs")
                if event_type in ("keydown","keypress"):
                    if event.get("data") is not None or event.get("inputType") is not None \
                            or event.get("targetRanges") is not None:
                        raise AssertionError(f"{label}: contenteditable {name} keyboard fields differ")
                    continue
                if event.get("data") != expectation["data"] \
                        or event.get("inputType") != "insertText":
                    raise AssertionError(f"{label}: contenteditable {name} input fields differ")
                ranges = event.get("targetRanges")
                if event_type == "input":
                    if ranges != []:
                        raise AssertionError(f"{label}: contenteditable {name} input ranges differ")
                else:
                    if not isinstance(ranges, list) or len(ranges) != 1:
                        raise AssertionError(f"{label}: contenteditable {name} beforeinput ranges differ")
                    range_value = ranges[0]
                    start_path = _contenteditable_path_contract(range_value.get("startPath"))
                    end_path = _contenteditable_path_contract(range_value.get("endPath"))
                    parent, start, end = expectation["initial"]
                    if range_value.get("startOffset") != start or range_value.get("endOffset") != end \
                            or not start_path or start_path[0].get("parentId") != parent \
                            or not end_path or end_path[0].get("parentId") != parent:
                        raise AssertionError(f"{label}: contenteditable {name} target range differs")
    elif case == "protocol-negative":
        expected_errors = [False, True, True, True, True, True, True, True, True, True, True]
        actual_errors = [bool(item.get("actionError")) for item in observation["actions"]]
        if actual_errors != expected_errors:
            raise AssertionError(f"{label}: protocol validation differs: {actual_errors!r}")
        for item in observation["actions"][1:]:
            error = item["actionError"]
            marker = f"Protocol error ({item['method']}):"
            if error.get("type") != "Error" or marker not in error.get("message", ""):
                raise AssertionError(f"{label}: non-protocol failure accepted: {item!r}")
    elif case == "ignore-input":
        def successful(actions: list[dict[str, Any]], name: str) -> None:
            if not actions or any(
                not isinstance(item.get("params"), dict)
                or item.get("response") != {}
                or item.get("actionError") is not None
                for item in actions
            ):
                raise AssertionError(f"{label}: ignore-input {name} action failed: {actions!r}")

        for name in ("true", "false"):
            record = observation.get(name, {})
            actions = record.get("actions", [])
            if len(actions) != 8:
                raise AssertionError(f"{label}: ignore-input {name} action count differs: {record!r}")
            successful(actions, name)
            if actions[0].get("method") != "Input.setIgnoreInputEvents":
                raise AssertionError(f"{label}: ignore-input state action missing")
            if record.get("before", {}).get("scrollTop") != 0:
                raise AssertionError(f"{label}: ignore-input initial scroll differs")
        true_after = observation["true"]["after"]
        false_after = observation["false"]["after"]
        true_types = [event.get("type") for event in true_after.get("events", [])
                      if event.get("currentTarget") == "edit"]
        if true_after.get("values", {}).get("edit") != {"value":"abcdx","start":5,"end":5} \
                or true_after.get("scrollTop") != 0 \
                or true_types != ["beforeinput", "input"]:
            raise AssertionError(f"{label}: ignore=true did not suppress browser input: {true_after!r}")
        false_types = [event.get("type") for event in false_after.get("events", [])
                       if event.get("currentTarget") == "edit"]
        for event_type in ("pointermove", "pointerdown", "mousedown", "pointerup", "mouseup", "keydown", "keyup", "beforeinput", "input"):
            if event_type not in false_types:
                raise AssertionError(f"{label}: ignore=false missing {event_type}: {false_types!r}")
        wheel_types = [event.get("type") for event in false_after.get("events", [])
                       if event.get("currentTarget") == "scrollbox"]
        if false_after.get("values", {}).get("edit") != {"value":"abcdxax","start":7,"end":7} \
                or false_after.get("scrollTop") != 40 \
                or wheel_types != ["wheel"]:
            raise AssertionError(f"{label}: ignore=false state differs: {false_after!r}")

        sibling = observation.get("sibling", {})
        successful(sibling.get("actions", []), "sibling")
        sibling_types = [event.get("type") for event in sibling.get("snapshot", {}).get("events", [])
                         if event.get("currentTarget") == "edit"]
        if sibling.get("snapshot", {}).get("values", {}).get("edit") != {"value":"abcd","start":4,"end":4} \
                or sibling_types:
            raise AssertionError(f"{label}: sibling session state differs: {sibling!r}")

        expected_lifecycle = {
            "navigation": (
                {"value":"abcdn","start":5,"end":5}, "n", ["beforeinput", "input"]
            ),
            "reattach": (
                {"value":"abcdgr","start":6,"end":6}, "r",
                ["keydown", "keypress", "beforeinput", "input", "beforeinput", "input"],
            ),
            "otherTarget": (
                {"value":"abcdh","start":5,"end":5}, "h",
                ["keydown", "keypress", "beforeinput", "input"],
            ),
        }
        for name, (field, accepted, expected_types) in expected_lifecycle.items():
            record = observation.get(name, {})
            successful(record.get("actions", []), name)
            snapshot = record.get("snapshot", {})
            types = [event.get("type") for event in snapshot.get("events", [])
                     if event.get("currentTarget") == "edit"]
            if snapshot.get("values", {}).get("edit") != field \
                    or types != expected_types \
                    or not any(event.get("data") == accepted for event in snapshot.get("events", [])):
                raise AssertionError(f"{label}: ignore-input {name} lifecycle differs: {record!r}")
        first_target = observation.get("otherTarget", {}).get("firstSnapshot", {})
        first_types = [event.get("type") for event in first_target.get("events", [])
                       if event.get("currentTarget") == "edit"]
        if first_target.get("values", {}).get("edit") != {
            "value":"abcdgr", "start":6, "end":6
        } or first_types:
            raise AssertionError(
                f"{label}: ignore-input first target was not isolated: {first_target!r}"
            )
        successful([observation.get("cleanupAction", {})], "cleanup")


def compare_case(case: str, reference: dict[str, Any], candidate: dict[str, Any], *, label: str) -> None:
    if case == "protocol-negative":
        def contract(action: dict[str, Any]) -> tuple[Any, ...]:
            error = action.get("actionError") or {}
            method = action.get("method")
            message = str(error.get("message", ""))
            marker = f"Protocol error ({method}):"
            return (method, action.get("params"), action.get("response"),
                    error.get("type"), bool(error), marker in message)
        if [contract(x) for x in reference["actions"]] != [contract(x) for x in candidate["actions"]]:
            raise AssertionError(f"{label}: protocol negative result differs")
        return
    if case == "ignore-input":
        def restored_snapshot_contract(snapshot: dict[str, Any]) -> dict[str, Any]:
            # setIgnoreInputEvents only qualifies whether coordinate input is
            # admitted again.  Preserve every raw event in the result, but do
            # not make this case re-qualify the independent mouse parity
            # matrix: click synthesis, legacy `which`, non-control value
            # projection, and caret timing during a coordinate event remain
            # independent mouse-parity work outside this gate.  Their complete
            # raw records stay in the result.  All other fields, including the
            # complete keyboard/text events, remain exact.
            coordinate_types = {
                "pointermove", "pointerdown", "pointerup",
                "mousedown", "mouseup", "wheel",
            }
            events = []
            for raw in snapshot.get("events", []):
                if raw.get("type") == "click":
                    continue
                event = dict(raw)
                if event.get("type") in coordinate_types:
                    for key in (
                        "which", "targetValue",
                        "targetSelectionStart", "targetSelectionEnd",
                    ):
                        event.pop(key, None)
                events.append(event)
            return {
                "events": events,
                "poisonCalls": snapshot.get("poisonCalls"),
                "active": snapshot.get("active"),
                "scrollTop": snapshot.get("scrollTop"),
                "values": snapshot.get("values"),
            }

        for name in ("true", "false", "sibling", "navigation", "reattach", "otherTarget"):
            ref = reference.get(name, {}); got = candidate.get(name, {})
            ref_after = ref.get("after")
            got_after = got.get("after")
            after_differs = (
                restored_snapshot_contract(ref_after or {})
                != restored_snapshot_contract(got_after or {})
                if name == "false" else ref_after != got_after
            )
            if ref.get("before") != got.get("before") or after_differs \
                    or ref.get("snapshot") != got.get("snapshot") \
                    or ref.get("firstSnapshot") != got.get("firstSnapshot"):
                raise AssertionError(f"{label}: ignore-input {name} snapshot differs")
            if ref.get("settle", {}).get("finalValue") != got.get("settle", {}).get("finalValue") \
                    or bool(ref.get("settle", {}).get("stable", 0) >= 2) != bool(
                        got.get("settle", {}).get("stable", 0) >= 2
                    ):
                raise AssertionError(f"{label}: ignore-input {name} settle differs")
            if ref.get("actions") != got.get("actions"):
                raise AssertionError(f"{label}: ignore-input {name} action result differs")
        if reference.get("cleanupAction") != candidate.get("cleanupAction"):
            raise AssertionError(f"{label}: ignore-input cleanup action differs")
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
    if case == "contenteditable":
        for name in (
            "insertText","selectionReplacement","keyText","cancel",
            "domReentry","falseIsland","emptySelection","focusTransfer",
        ):
            ref = reference.get(name, {})
            got = candidate.get(name, {})
            ref_snapshot = _contenteditable_snapshot_contract(ref.get("snapshot"))
            got_snapshot = _contenteditable_snapshot_contract(got.get("snapshot"))
            # Focusing a text control collapses Chrome's document Selection to
            # body while Obscura retains the inactive rich selection. The
            # qualified boundary is that focus and editing follow the text
            # control; both complete raw Selection snapshots remain recorded.
            if name == "focusTransfer":
                if ref_snapshot is not None: ref_snapshot = {k:v for k,v in ref_snapshot.items() if k != "selection"}
                if got_snapshot is not None: got_snapshot = {k:v for k,v in got_snapshot.items() if k != "selection"}
            if ref.get("action") != got.get("action") or ref_snapshot != got_snapshot:
                raise AssertionError(f"{label}: contenteditable {name} differs")
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

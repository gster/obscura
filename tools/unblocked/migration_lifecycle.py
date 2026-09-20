#!/usr/bin/env python3
"""Official Playwright lifecycle acceptance cases for the runtime migration."""

from __future__ import annotations

import asyncio
import base64
import json
import threading
import time
import urllib.parse
import urllib.request
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Iterator


_NO_REPLAY_HTML = b"""<!doctype html>
<meta charset="utf-8">
<title>migration no replay</title>
<button id="effect">perform effect</button>
<script>
document.querySelector('#effect').addEventListener('click', () => {
  fetch('/effect?counter=post-dispatch', {
    method: 'POST',
    headers: {'Content-Type': 'application/octet-stream'},
    body: new Uint8Array([0, 1, 2, 255])
  });
});
</script>
"""

_WATCHDOG_HTML = b"""<!doctype html>
<meta charset="utf-8">
<title>migration click watchdog</title>
<button id="infinite">enter infinite handler</button>
<button id="finite">finite recovery action</button>
<output id="status">ready</output>
<script>
globalThis.entered = 0;
globalThis.finiteClicks = 0;
document.querySelector('#infinite').addEventListener('click', () => {
  globalThis.entered += 1;
  document.querySelector('#status').textContent = 'entered';
  while (true) {}
});
document.querySelector('#finite').addEventListener('click', () => {
  globalThis.finiteClicks += 1;
  document.querySelector('#status').textContent = 'recovered';
});
</script>
"""


def _emit(kind: str, **fields: Any) -> None:
    print(
        json.dumps(
            {"kind": kind, "monotonicNs": time.monotonic_ns(), **fields},
            sort_keys=True,
            separators=(",", ":"),
        ),
        flush=True,
    )


def _error_record(error: BaseException) -> dict[str, str]:
    return {
        "type": type(error).__name__,
        "message": str(error),
        "repr": repr(error),
    }


class _FixtureState:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.counts = {
            "task-cancel": 0,
            "official-timeout": 0,
            "page-close": 0,
            "sibling-control": 0,
            "post-dispatch": 0,
        }
        self.requests: list[dict[str, Any]] = []

    def record_request(self, record: dict[str, Any]) -> None:
        with self.lock:
            record["sequence"] = len(self.requests) + 1
            self.requests.append(record)
        _emit("fixture-request", **record)

    def increment(self, name: str) -> int:
        with self.lock:
            self.counts[name] = self.counts.get(name, 0) + 1
            value = self.counts[name]
        _emit("fixture-counter", counter=name, value=value)
        return value

    def snapshot(self) -> dict[str, Any]:
        with self.lock:
            return {
                "counts": dict(self.counts),
                "requests": [dict(item) for item in self.requests],
            }


class _HeaderCapture:
    def __init__(self, stream: Any) -> None:
        self.stream = stream
        self.chunks: list[bytes] = []

    def readline(self, *args: Any, **kwargs: Any) -> bytes:
        line = self.stream.readline(*args, **kwargs)
        self.chunks.append(line)
        return line

    def __getattr__(self, name: str) -> Any:
        return getattr(self.stream, name)


class _LifecycleFixtureHandler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    server_version = "ObscuraMigrationLifecycle/1"
    sys_version = ""

    @property
    def state(self) -> _FixtureState:
        return getattr(self.server, "lifecycle_state")

    def date_time_string(self, timestamp: float | None = None) -> str:
        return "Sun, 20 Sep 2026 00:00:00 GMT"

    def parse_request(self) -> bool:
        stream = self.rfile
        capture = _HeaderCapture(stream)
        self.rfile = capture
        try:
            return super().parse_request()
        finally:
            self.rfile = stream
            self._raw_header_bytes = b"".join(capture.chunks)

    def _read_and_record(self) -> tuple[urllib.parse.SplitResult, bytes]:
        length = int(self.headers.get("Content-Length", "0"))
        body = self.rfile.read(length) if length else b""
        target = urllib.parse.urlsplit(self.path)
        peer_host, peer_port = self.client_address[:2]
        request_head = self.raw_requestline + self._raw_header_bytes
        self.state.record_request(
            {
                "method": self.command,
                "target": self.path,
                "path": target.path,
                "query": target.query,
                "parsedHeaders": list(self.headers.raw_items()),
                "rawRequestHeadBase64": base64.b64encode(request_head).decode("ascii"),
                "rawRequestBase64": base64.b64encode(request_head + body).decode(
                    "ascii"
                ),
                "bodyBase64": base64.b64encode(body).decode("ascii"),
                "client": {"host": peer_host, "port": peer_port},
            }
        )
        return target, body

    def _send(self, status: int, content_type: str, body: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self) -> None:
        target, _ = self._read_and_record()
        if target.path == "/no-replay":
            self._send(200, "text/html; charset=utf-8", _NO_REPLAY_HTML)
            return
        if target.path == "/click-watchdog":
            self._send(200, "text/html; charset=utf-8", _WATCHDOG_HTML)
            return
        if target.path == "/counts":
            body = json.dumps(
                self.state.snapshot()["counts"], sort_keys=True
            ).encode("utf-8")
            self._send(200, "application/json", body)
            return
        self._send(404, "text/plain; charset=utf-8", b"not found")

    def do_POST(self) -> None:
        target, _ = self._read_and_record()
        if target.path != "/effect":
            self._send(404, "text/plain; charset=utf-8", b"not found")
            return
        query = urllib.parse.parse_qs(target.query, keep_blank_values=True)
        counter = query.get("counter", [""])[0]
        if counter not in self.state.counts:
            self._send(400, "text/plain; charset=utf-8", b"unknown counter")
            return
        value = self.state.increment(counter)
        self._send(
            200,
            "application/json",
            json.dumps({"counter": counter, "value": value}, sort_keys=True).encode(
                "utf-8"
            ),
        )

    def log_message(self, format: str, *args: Any) -> None:
        return


@contextmanager
def lifecycle_fixture_server() -> Iterator[tuple[str, _FixtureState]]:
    """Serve lifecycle pages and yield ``(origin, state)`` for one case."""

    server = ThreadingHTTPServer(("127.0.0.1", 0), _LifecycleFixtureHandler)
    state = _FixtureState()
    server.lifecycle_state = state  # type: ignore[attr-defined]
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    host, port = server.server_address
    origin = f"http://{host}:{port}"
    _emit("fixture-started", origin=origin)
    try:
        yield origin, state
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
        _emit("fixture-stopped", origin=origin, threadAlive=thread.is_alive())


def _control_counts(origin: str) -> dict[str, Any]:
    url = f"{origin}/counts"
    with urllib.request.urlopen(url, timeout=2) as response:
        body = response.read()
        return {
            "url": response.url,
            "status": response.status,
            "parsedHeaders": list(response.headers.raw_items()),
            "bodyBase64": base64.b64encode(body).decode("ascii"),
            "counts": json.loads(body),
        }


async def _read_counts(origin: str) -> dict[str, Any]:
    return await asyncio.to_thread(_control_counts, origin)


async def _wait_for_count(
    origin: str,
    counter: str,
    expected: int,
    observations: list[dict[str, Any]],
    *,
    timeout: float = 1.5,
) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        snapshot = await _read_counts(origin)
        observations.append(snapshot)
        if snapshot["counts"].get(counter) == expected:
            return snapshot
        await asyncio.sleep(0.01)
    raise TimeoutError(
        f"fixture counter {counter!r} did not reach {expected}: {observations!r}"
    )


def _stage(observations: dict[str, Any], name: str, **fields: Any) -> None:
    record = {"name": name, "monotonicNs": time.monotonic_ns(), **fields}
    observations.setdefault("stages", []).append(record)
    _emit("case-stage", case=observations.get("case"), **record)


async def _install_effect_button(page: Any, selector: str, counter: str) -> None:
    await page.evaluate(
        """({selector, counter}) => {
          const existing = document.querySelector(selector);
          if (existing) existing.remove();
          const button = document.createElement('button');
          button.id = selector.slice(1);
          button.textContent = counter;
          button.addEventListener('click', () => {
            fetch('/effect?counter=' + encodeURIComponent(counter), {
              method: 'POST',
              headers: {'Content-Type': 'application/octet-stream'},
              body: new Uint8Array([9, 8, 7, 0, 255])
            });
          });
          document.body.append(button);
        }""",
        {"selector": selector, "counter": counter},
    )


async def _run_no_replay(page: Any, origin: str, observations: dict[str, Any]) -> None:
    from playwright.async_api import TimeoutError as PlaywrightTimeoutError

    await page.goto(f"{origin}/no-replay", wait_until="load")
    _stage(observations, "no-replay-page-loaded", url=page.url)

    task_cancel_selector = "#task-cancel-target"
    missing = asyncio.create_task(page.locator(task_cancel_selector).click(timeout=0))
    await asyncio.sleep(0.15)
    if missing.done():
        try:
            await missing
        except BaseException as error:
            raise AssertionError(
                f"missing-element click stopped before explicit cancellation: {error}"
            ) from error
        raise AssertionError("missing-element click returned before explicit cancellation")
    missing.cancel()
    try:
        await missing
    except asyncio.CancelledError:
        observations["taskCancellation"] = {
            "localTaskCancelled": True,
            "remoteCommitState": "indeterminate",
            "selector": task_cancel_selector,
        }
    else:
        raise AssertionError("missing-element click ignored task cancellation")
    before = await _read_counts(origin)
    observations["taskCancellation"]["serverAtCancellation"] = before
    if before["counts"]["task-cancel"] != 0:
        raise AssertionError(f"task-cancel effect ran before its target existed: {before!r}")
    await _install_effect_button(page, task_cancel_selector, "task-cancel")
    task_count_observations: list[dict[str, Any]] = []
    observations["taskCancellation"]["countObservations"] = task_count_observations
    first_task_effect = await _wait_for_count(
        origin,
        "task-cancel",
        1,
        task_count_observations,
    )
    await asyncio.sleep(0.3)
    stable_task_effect = await _read_counts(origin)
    observations["taskCancellation"].update(
        {
            "lateTargetInserted": True,
            "firstEffect": first_task_effect,
            "serverAfterAdditionalWait": stable_task_effect,
            "automaticReplay": False,
        }
    )
    if stable_task_effect["counts"]["task-cancel"] != 1:
        raise AssertionError(
            f"indeterminate task cancellation replayed or lost the action: {stable_task_effect!r}"
        )
    _stage(
        observations,
        "task-cancellation-indeterminate",
        counts=stable_task_effect["counts"],
    )

    timeout_selector = "#official-timeout-target"
    timeout_started = time.monotonic()
    try:
        await page.locator(timeout_selector).click(timeout=300)
    except PlaywrightTimeoutError as error:
        observations["officialActionTimeout"] = {
            "selector": timeout_selector,
            "timedOut": True,
            "elapsedMs": (time.monotonic() - timeout_started) * 1000,
            "error": _error_record(error),
        }
    else:
        raise AssertionError("missing locator did not honor its official action timeout")
    at_timeout = await _read_counts(origin)
    observations["officialActionTimeout"]["serverAtTimeout"] = at_timeout
    if at_timeout["counts"]["official-timeout"] != 0:
        raise AssertionError(f"official timeout dispatched the missing action: {at_timeout!r}")
    await _install_effect_button(page, timeout_selector, "official-timeout")
    await asyncio.sleep(0.3)
    after_timeout_target = await _read_counts(origin)
    observations["officialActionTimeout"].update(
        {
            "lateTargetInserted": True,
            "serverAfterInsertion": after_timeout_target,
        }
    )
    if after_timeout_target["counts"]["official-timeout"] != 0:
        raise AssertionError(
            f"officially timed-out action resumed after its target appeared: {after_timeout_target!r}"
        )
    _stage(
        observations,
        "official-action-timeout-stopped",
        counts=after_timeout_target["counts"],
    )

    closing_page = await page.context.new_page()
    closing_selector = "#page-close-target"
    try:
        await closing_page.goto(f"{origin}/no-replay", wait_until="load")
        closing_action = asyncio.create_task(
            closing_page.locator(closing_selector).click(timeout=0)
        )
        await asyncio.sleep(0.15)
        if closing_action.done():
            raise AssertionError("page-close action stopped before the page was closed")
        before_close = await _read_counts(origin)
        if before_close["counts"]["page-close"] != 0:
            raise AssertionError(f"page-close action dispatched too early: {before_close!r}")
        await closing_page.close()
        try:
            await closing_action
        except BaseException as error:
            closing_error = _error_record(error)
        else:
            raise AssertionError("pending locator succeeded while its page closed")
        observations["pageCloseCancellation"] = {
            "selector": closing_selector,
            "serverBeforeClose": before_close,
            "pageClosed": closing_page.is_closed(),
            "pendingActionError": closing_error,
        }
    finally:
        if not closing_page.is_closed():
            await closing_page.close()

    await _install_effect_button(page, closing_selector, "page-close")
    await asyncio.sleep(0.3)
    after_close_target = await _read_counts(origin)
    if after_close_target["counts"]["page-close"] != 0:
        raise AssertionError(
            f"closed page action escaped into its sibling: {after_close_target!r}"
        )
    sibling_selector = "#sibling-control"
    await _install_effect_button(page, sibling_selector, "sibling-control")
    await page.locator(sibling_selector).click(timeout=5_000)
    sibling_count_observations: list[dict[str, Any]] = []
    sibling_control = await _wait_for_count(
        origin,
        "sibling-control",
        1,
        sibling_count_observations,
    )
    observations["pageCloseCancellation"].update(
        {
            "lateTargetInsertedInSibling": True,
            "serverAfterClose": after_close_target,
            "siblingControlCountObservations": sibling_count_observations,
            "siblingControl": sibling_control,
        }
    )
    _stage(
        observations,
        "page-close-stopped-pending-action",
        counts=sibling_control["counts"],
    )

    absent_url = f"{origin}/response-that-never-occurs"
    count_observations: list[dict[str, Any]] = []
    observations["postDispatchCountObservations"] = count_observations
    action_started = time.monotonic()
    try:
        async with page.expect_response(
            lambda response: response.url == absent_url,
            timeout=2_000,
        ) as response_info:
            await page.locator("#effect").click(timeout=5_000)
            observations["postDispatchClick"] = {
                "returned": True,
                "elapsedMs": (time.monotonic() - action_started) * 1000,
            }
            confirmed = await _wait_for_count(
                origin,
                "post-dispatch",
                1,
                count_observations,
            )
            _stage(
                observations,
                "post-dispatch-effect-confirmed",
                counts=confirmed["counts"],
            )
            observations["postDispatchEffectConfirmed"] = confirmed
        await response_info.value
    except PlaywrightTimeoutError as error:
        if not observations.get("postDispatchClick", {}).get("returned") or not observations.get(
            "postDispatchEffectConfirmed"
        ):
            raise AssertionError(
                "click or server effect did not complete before the response wait timed out"
            ) from error
        observations["absentResponseWait"] = {
            "url": absent_url,
            "timedOut": True,
            "elapsedMs": (time.monotonic() - action_started) * 1000,
            "error": _error_record(error),
        }
        _stage(observations, "absent-response-timeout", url=absent_url)
    else:
        raise AssertionError("waiting for the absent response unexpectedly completed")

    await asyncio.sleep(0.3)
    final = await _read_counts(origin)
    observations["finalServerCounts"] = final
    expected_counts = {
        "task-cancel": 1,
        "official-timeout": 0,
        "page-close": 0,
        "sibling-control": 1,
        "post-dispatch": 1,
    }
    if final["counts"] != expected_counts:
        raise AssertionError(
            f"lifecycle side-effect counts differ: expected {expected_counts!r}, got {final!r}"
        )
    _stage(observations, "no-replay-proven", counts=final["counts"])


async def _run_click_watchdog(
    page: Any, origin: str, observations: dict[str, Any]
) -> None:
    await page.goto(f"{origin}/click-watchdog", wait_until="load")
    original_page_url = page.url
    _stage(observations, "watchdog-page-loaded", url=original_page_url)

    box = await page.locator("#infinite").bounding_box()
    if box is None or box["width"] <= 0 or box["height"] <= 0:
        raise AssertionError(f"infinite-handler target has no input geometry: {box!r}")
    observations["watchdogInput"] = {
        "api": "page.mouse.click",
        "x": box["x"] + box["width"] / 2,
        "y": box["y"] + box["height"] / 2,
        "boundingBox": box,
    }
    click_started = time.monotonic()
    try:
        # Locator actions retry protocol failures. Mouse sends this click once,
        # letting the command watchdog's actual error reach the official client.
        await page.mouse.click(
            observations["watchdogInput"]["x"], observations["watchdogInput"]["y"]
        )
    except Exception as error:
        observations["infiniteClick"] = {
            "returned": False,
            "elapsedMs": (time.monotonic() - click_started) * 1000,
            "error": _error_record(error),
        }
        _stage(
            observations,
            "infinite-click-returned-error",
            error=observations["infiniteClick"]["error"],
        )
        if "INPUT_DISPATCH_FAILED" not in str(error):
            raise AssertionError("infinite click did not expose the native dispatch failure") from error
    else:
        observations["infiniteClick"] = {
            "returned": True,
            "elapsedMs": (time.monotonic() - click_started) * 1000,
            "error": None,
        }
        _stage(observations, "infinite-click-returned")
        raise AssertionError("infinite click silently succeeded instead of reporting termination")

    after_watchdog = await page.evaluate(
        """() => ({
          entered: globalThis.entered,
          finiteClicks: globalThis.finiteClicks,
          status: document.querySelector('#status').textContent,
          url: location.href
        })"""
    )
    observations["samePageAfterWatchdog"] = after_watchdog
    if after_watchdog != {
        "entered": 1,
        "finiteClicks": 0,
        "status": "entered",
        "url": original_page_url,
    }:
        raise AssertionError(
            f"original page did not recover after the infinite handler: {after_watchdog!r}"
        )
    _stage(observations, "same-page-evaluate-recovered", value=after_watchdog)

    evaluation = await page.evaluate(
        """() => {
          document.documentElement.dataset.recovery = 'evaluate-ok';
          return document.documentElement.dataset.recovery;
        }"""
    )
    if evaluation != "evaluate-ok":
        raise AssertionError(f"same-page recovery evaluation failed: {evaluation!r}")
    observations["samePageRecoveryEvaluation"] = evaluation

    await page.locator("#finite").click(timeout=5_000)
    recovered = await page.evaluate(
        """() => ({
          entered: globalThis.entered,
          finiteClicks: globalThis.finiteClicks,
          status: document.querySelector('#status').textContent,
          recovery: document.documentElement.dataset.recovery,
          url: location.href
        })"""
    )
    observations["samePageFiniteClick"] = recovered
    expected = {
        "entered": 1,
        "finiteClicks": 1,
        "status": "recovered",
        "recovery": "evaluate-ok",
        "url": original_page_url,
    }
    if recovered != expected:
        raise AssertionError(f"same-page finite action did not recover: {recovered!r}")
    _stage(observations, "same-page-click-recovered", value=recovered)


async def run_case(case: str, endpoint: str, observations: dict[str, Any]) -> None:
    """Run one migration acceptance case against an existing CDP endpoint.

    The caller owns the browser process and must enforce an external hard deadline.
    ``click-watchdog`` uses the official one-shot mouse API so locator retries
    cannot replay the infinite handler. The browser watchdog must return its
    dispatch error before the caller's process deadline.
    """

    if case not in {"no-replay", "click-watchdog"}:
        raise ValueError(f"unknown migration lifecycle case: {case}")

    from playwright.async_api import async_playwright

    observations.update(
        {
            "schemaVersion": 1,
            "case": case,
            "endpoint": endpoint,
            "connection": "BrowserType.connect_over_cdp",
            "status": "running",
            "stages": [],
        }
    )
    _emit("case-entry", case=case, endpoint=endpoint)
    fixture_state: _FixtureState | None = None
    try:
        with lifecycle_fixture_server() as (origin, fixture_state):
            observations["fixtureOrigin"] = origin
            async with async_playwright() as playwright:
                browser = await playwright.chromium.connect_over_cdp(
                    endpoint, timeout=10_000
                )
                try:
                    observations["browserVersion"] = browser.version
                    observations["initialContextCount"] = len(browser.contexts)
                    if len(browser.contexts) != 1:
                        raise AssertionError(
                            f"expected one browser context, got {len(browser.contexts)}"
                        )
                    context = browser.contexts[0]
                    page = context.pages[0] if context.pages else await context.new_page()
                    observations["initialPageCount"] = len(context.pages)
                    if case == "no-replay":
                        await _run_no_replay(page, origin, observations)
                    else:
                        await _run_click_watchdog(page, origin, observations)
                finally:
                    observations["fixture"] = fixture_state.snapshot()
                    await browser.close()
        observations["status"] = "passed"
        _emit("case-complete", case=case, status="passed")
    except BaseException as error:
        observations["status"] = "failed"
        observations["error"] = _error_record(error)
        if fixture_state is not None:
            observations["fixture"] = fixture_state.snapshot()
        _emit("case-complete", case=case, status="failed", error=_error_record(error))
        raise

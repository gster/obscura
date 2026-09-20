#!/usr/bin/env python3
"""Required official Playwright Python smoke for the OB-027 CDP profile."""

from __future__ import annotations

import argparse
import base64
import hashlib
import importlib.metadata
import json
import struct
import zlib
from pathlib import Path
from typing import Any

if __package__:
    from .cdp_fixture import (
        external_process,
        fixture_control_get,
        fixture_server,
        free_port,
        wait_for_fixture_hold,
    )
    from .cdp_trace import Trace
else:
    from cdp_fixture import (
        external_process,
        fixture_control_get,
        fixture_server,
        free_port,
        wait_for_fixture_hold,
    )
    from cdp_trace import Trace


def expect_protocol_error(
    trace: Trace,
    session: Any,
    method: str,
    params: dict[str, Any],
    expected: str,
) -> str:
    try:
        trace.send(session, method, params, session_name="page-1")
    except Exception as error:
        message = f"{type(error).__name__}: {error}"
        if expected not in message:
            raise AssertionError(f"{method} failed with the wrong error: {message}") from error
        return message
    raise AssertionError(f"{method} returned placeholder success")


def inspect_png(data: bytes) -> dict[str, Any]:
    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise AssertionError("Page.screenshot did not return a PNG")
    offset = 8
    header = None
    compressed = bytearray()
    while offset < len(data):
        length = struct.unpack(">I", data[offset : offset + 4])[0]
        kind = data[offset + 4 : offset + 8]
        payload = data[offset + 8 : offset + 8 + length]
        offset += 12 + length
        if kind == b"IHDR":
            header = struct.unpack(">IIBBBBB", payload)
        elif kind == b"IDAT":
            compressed.extend(payload)
        elif kind == b"IEND":
            break
    if header is None:
        raise AssertionError("Page.screenshot PNG has no IHDR")
    width, height, bit_depth, color_type, compression, filtering, interlace = header
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if (
        bit_depth != 8
        or channels is None
        or compression != 0
        or filtering != 0
        or interlace != 0
    ):
        raise AssertionError(f"unsupported screenshot PNG header: {header!r}")

    encoded = zlib.decompress(compressed)
    stride = width * channels
    rows: list[bytes] = []
    position = 0
    previous = bytes(stride)
    for _ in range(height):
        filter_type = encoded[position]
        position += 1
        row = bytearray(encoded[position : position + stride])
        position += stride
        for index in range(stride):
            left = row[index - channels] if index >= channels else 0
            above = previous[index]
            upper_left = previous[index - channels] if index >= channels else 0
            if filter_type == 1:
                row[index] = (row[index] + left) & 0xFF
            elif filter_type == 2:
                row[index] = (row[index] + above) & 0xFF
            elif filter_type == 3:
                row[index] = (row[index] + ((left + above) // 2)) & 0xFF
            elif filter_type == 4:
                estimate = left + above - upper_left
                distances = (
                    abs(estimate - left),
                    abs(estimate - above),
                    abs(estimate - upper_left),
                )
                predictor = (left, above, upper_left)[distances.index(min(distances))]
                row[index] = (row[index] + predictor) & 0xFF
            elif filter_type != 0:
                raise AssertionError(f"unsupported screenshot PNG filter: {filter_type}")
        previous = bytes(row)
        rows.append(previous)
    pixels = b"".join(rows)
    unique_pixels = {
        pixels[index : index + channels]
        for index in range(0, len(pixels), channels)
    }
    return {
        "data": base64.b64encode(data).decode("ascii"),
        "byteLength": len(data),
        "sha256": hashlib.sha256(data).hexdigest(),
        "pixelSha256": hashlib.sha256(pixels).hexdigest(),
        "width": width,
        "height": height,
        "bitDepth": bit_depth,
        "colorType": color_type,
        "uniquePixelCount": len(unique_pixels),
    }


def run(obscura_bin: Path, *, log_root: Path | None = None) -> dict[str, Any]:
    from playwright.sync_api import Error as PlaywrightError, sync_playwright

    result: dict[str, Any] = {
        "schemaVersion": 1,
        "playwrightVersion": importlib.metadata.version("playwright"),
        "connection": "BrowserType.connect_over_cdp",
        "status": "running",
        "processCapture": {},
    }
    trace: Trace | None = None
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
    ]
    try:
        with fixture_server() as fixture_origin, external_process(
            command,
            endpoint,
            capture=result["processCapture"],
            log_root=log_root,
        ):
            trace = Trace("obscura-cdp-smoke", fixture_origin)
            with sync_playwright() as playwright:
                browser = playwright.chromium.connect_over_cdp(endpoint)
                try:
                    result["browserVersion"] = browser.version
                    if len(browser.contexts) != 1:
                        raise AssertionError(
                            f"expected one context, got {len(browser.contexts)}"
                        )
                    context = browser.contexts[0]
                    page = context.pages[0] if context.pages else context.new_page()
                    fixture_url = f"{fixture_origin}/fixture?token=full-cdp-smoke-value"
                    result["fixtureUrl"] = fixture_url
                    response = page.goto(fixture_url, wait_until="load")
                    if response is None:
                        raise AssertionError("Page.goto returned no document response")
                    result["documentResponse"] = {
                        "url": response.url,
                        "status": response.status,
                        "statusText": response.status_text,
                        "headers": response.all_headers(),
                        "body": response.body().decode("utf-8", errors="surrogateescape"),
                    }
                    page.wait_for_function(
                        "document.documentElement.dataset.ready === 'yes'", timeout=10_000
                    )

                    initial = page.evaluate(
                        "({title:document.title,input:document.querySelector('#name').value})"
                    )
                    result["initial"] = initial
                    page.evaluate(
                        """(() => {
                          globalThis.__obscuraLocatorEvents = [];
                          const input = document.querySelector('#name');
                          const select = document.querySelector('#city');
                          const multiSelect = document.querySelector('#destinations');
                          const button = document.querySelector('#submit');
                          for (const type of ['focus', 'input', 'change']) {
                            input.addEventListener(type, event => {
                              __obscuraLocatorEvents.push({
                                target: 'input', type, trusted: event.isTrusted,
                                value: input.value
                              });
                            });
                          }
                          for (const [target, control] of [
                            ['select', select], ['multi-select', multiSelect]
                          ]) {
                            for (const type of ['input', 'change']) control.addEventListener(type, event => {
                              __obscuraLocatorEvents.push({
                                target, type, trusted: event.isTrusted,
                                value: control.value,
                                selected: Array.from(control.selectedOptions, option => option.value)
                              });
                            });
                          }
                          for (const type of ['mousedown', 'focus', 'mouseup', 'click']) {
                            button.addEventListener(type, event => {
                              __obscuraLocatorEvents.push({
                                target: 'button', type, trusted: event.isTrusted
                              });
                            });
                          }
                        })()"""
                    )
                    result["selectedOptions"] = None
                    result["selectedMultipleOptions"] = None
                    try:
                        page.get_by_label("Name").fill("official-playwright")
                        result["selectedOptions"] = page.get_by_label("City").select_option("pek")
                        result["selectedMultipleOptions"] = page.get_by_label(
                            "Destinations"
                        ).select_option(["sha", "can"])
                        page.get_by_role("button", name="Submit").click()
                    finally:
                        try:
                            result["locator"] = page.evaluate(
                                """({
                                  input: document.querySelector('#name').value,
                                  city: document.querySelector('#city').value,
                                  destinations: Array.from(
                                    document.querySelector('#destinations').selectedOptions,
                                    option => option.value
                                  ),
                                  result: document.querySelector('#result').textContent,
                                  activeElement: document.activeElement && document.activeElement.id,
                                  events: globalThis.__obscuraLocatorEvents
                                })"""
                            )
                        except Exception as snapshot_error:
                            result["locatorSnapshotError"] = {
                                "type": type(snapshot_error).__name__,
                                "message": str(snapshot_error),
                            }
                    locator = result["locator"]
                    result["changed"] = locator["result"]
                    if initial != {"title": "OB-026 CDP fixture", "input": "fixture"}:
                        raise AssertionError(f"unexpected initial page state: {initial!r}")
                    if locator["input"] != "official-playwright":
                        raise AssertionError(f"locator fill did not update the input: {locator!r}")
                    if result["selectedOptions"] != ["pek"] or locator["city"] != "pek":
                        raise AssertionError(f"locator select did not choose Beijing: {locator!r}")
                    if (
                        result["selectedMultipleOptions"] != ["sha", "can"]
                        or locator["destinations"] != ["sha", "can"]
                    ):
                        raise AssertionError(f"locator multi-select did not choose both cities: {locator!r}")
                    if locator["result"] != "official-playwright":
                        raise AssertionError(f"locator click did not update the output: {locator!r}")
                    observed_events = [
                        (event["target"], event["type"]) for event in locator["events"]
                    ]
                    required_events = [
                        ("input", "focus"),
                        ("input", "input"),
                        ("select", "input"),
                        ("select", "change"),
                        ("multi-select", "input"),
                        ("multi-select", "change"),
                        ("button", "mousedown"),
                        ("button", "mouseup"),
                        ("button", "click"),
                    ]
                    positions = [observed_events.index(event) for event in required_events]
                    if positions != sorted(positions):
                        raise AssertionError(f"locator events are out of order: {locator!r}")
                    select_events = [
                        event
                        for event in locator["events"]
                        if event["target"] in {"select", "multi-select"}
                    ]
                    expected_select_events = [
                        {
                            "target": "select",
                            "type": "input",
                            "trusted": False,
                            "value": "pek",
                            "selected": ["pek"],
                        },
                        {
                            "target": "select",
                            "type": "change",
                            "trusted": False,
                            "value": "pek",
                            "selected": ["pek"],
                        },
                        {
                            "target": "multi-select",
                            "type": "input",
                            "trusted": False,
                            "value": "sha",
                            "selected": ["sha", "can"],
                        },
                        {
                            "target": "multi-select",
                            "type": "change",
                            "trusted": False,
                            "value": "sha",
                            "selected": ["sha", "can"],
                        },
                    ]
                    if select_events != expected_select_events:
                        raise AssertionError(
                            f"locator select events differ from Chrome: {select_events!r}"
                        )
                    for event in locator["events"]:
                        expected_trusted = event["target"] not in {"select", "multi-select"}
                        if event.get("trusted") is not expected_trusted:
                            raise AssertionError(f"locator event trust differs from Chrome: {locator!r}")

                    page.set_viewport_size({"width": 320, "height": 240})
                    screenshot = inspect_png(page.screenshot(type="png"))
                    result["screenshot"] = screenshot
                    if screenshot["width"] != 320 or screenshot["height"] != 240:
                        raise AssertionError(f"unexpected screenshot dimensions: {screenshot!r}")
                    if screenshot["uniquePixelCount"] < 2:
                        raise AssertionError(f"screenshot is blank: {screenshot!r}")

                    context_result: dict[str, Any] = {
                        "initialContextCount": len(browser.contexts),
                    }
                    result["contextIsolation"] = context_result
                    page.evaluate(
                        """(() => {
                          localStorage.setItem('__obscuraContextOwner', 'default');
                          globalThis.__obscuraDefaultContextSentinel = 73;
                        })()"""
                    )
                    isolated_context = None
                    isolated_page = None
                    sibling_page = None
                    page_a_handle = None
                    try:
                        isolated_context = browser.new_context()
                        context_result["createdContextCount"] = len(browser.contexts)
                        isolated_page = isolated_context.new_page()
                        isolated_response = isolated_page.goto(
                            f"{fixture_origin}/fixture?token=isolated-context-value",
                            wait_until="load",
                        )
                        if isolated_response is None:
                            raise AssertionError("isolated Page.goto returned no document response")
                        context_result["documentResponse"] = {
                            "url": isolated_response.url,
                            "status": isolated_response.status,
                            "statusText": isolated_response.status_text,
                            "headers": isolated_response.all_headers(),
                            "body": isolated_response.body().decode(
                                "utf-8", errors="surrogateescape"
                            ),
                        }
                        context_result["isolatedBefore"] = isolated_page.evaluate(
                            """({
                              storage: localStorage.getItem('__obscuraContextOwner'),
                              defaultSentinel: globalThis.__obscuraDefaultContextSentinel,
                              viewport: [innerWidth, innerHeight, visualViewport.width, visualViewport.height],
                              screen: [screen.width, screen.height, screen.availWidth, screen.availHeight],
                              devicePixelRatio
                            })"""
                        )
                        isolated_page.evaluate(
                            """(() => {
                              localStorage.setItem('__obscuraContextOwner', 'isolated');
                              globalThis.__obscuraIsolatedContextSentinel = 91;
                            })()"""
                        )
                        context_result["defaultWhileOpen"] = page.evaluate(
                            """({
                              storage: localStorage.getItem('__obscuraContextOwner'),
                              defaultSentinel: globalThis.__obscuraDefaultContextSentinel,
                              isolatedSentinel: globalThis.__obscuraIsolatedContextSentinel
                            })"""
                        )
                        context_result["isolatedWhileOpen"] = isolated_page.evaluate(
                            """({
                              storage: localStorage.getItem('__obscuraContextOwner'),
                              defaultSentinel: globalThis.__obscuraDefaultContextSentinel,
                              isolatedSentinel: globalThis.__obscuraIsolatedContextSentinel
                            })"""
                        )
                        isolated_page.evaluate(
                            """(() => {
                              globalThis.__obscuraPageAClosure = (() => {
                                let value = 40;
                                return () => ++value;
                              })();
                              globalThis.__obscuraPageAPromise = new Promise(resolve =>
                                setTimeout(() => resolve('page-a-timer'), 40));
                            })()"""
                        )
                        page_a_handle = isolated_page.evaluate_handle(
                            "({owner: 'page-a', value: 41})"
                        )
                        sibling_page = isolated_context.new_page()
                        sibling_response = sibling_page.goto(
                            f"{fixture_origin}/fixture?token=sibling-page-value",
                            wait_until="load",
                        )
                        if sibling_response is None:
                            raise AssertionError("sibling Page.goto returned no document response")
                        context_result["siblingDocumentResponse"] = {
                            "url": sibling_response.url,
                            "status": sibling_response.status,
                            "statusText": sibling_response.status_text,
                            "headers": sibling_response.all_headers(),
                            "body": sibling_response.body().decode(
                                "utf-8", errors="surrogateescape"
                            ),
                        }
                        closed_page_events: list[dict[str, Any]] = []
                        context_result["closedPageRuntimeEvents"] = closed_page_events
                        close_phase = {"value": "before-close"}
                        sibling_session = isolated_context.new_cdp_session(sibling_page)
                        sibling_session.send("Runtime.enable")
                        sibling_session.on(
                            "Runtime.consoleAPICalled",
                            lambda event: closed_page_events.append(
                                {"phase": close_phase["value"], "event": event}
                            ),
                        )
                        context_result["heldFetchInitialState"] = sibling_page.evaluate(
                            """(() => {
                              globalThis.__obscuraPageBSentinel = 'page-b';
                              globalThis.__obscuraHeldFetchState = 'pending';
                              fetch('/lifecycle-hold?token=closed-page-pending-request')
                                .then(response => response.text())
                                .then(body => {
                                  globalThis.__obscuraHeldFetchState = 'resolved';
                                  console.log('closed-page-held-fetch', body);
                                }, error => {
                                  globalThis.__obscuraHeldFetchState = 'rejected';
                                  console.log('closed-page-held-fetch-error', String(error));
                                });
                              return globalThis.__obscuraHeldFetchState;
                            })()"""
                        )
                        held_start_observations: list[dict[str, Any]] = []
                        context_result["heldRequestStartObservations"] = (
                            held_start_observations
                        )
                        context_result["heldRequestStarted"] = wait_for_fixture_hold(
                            fixture_origin, "started", held_start_observations
                        )
                        context_result["pageCountWhileOpen"] = len(isolated_context.pages)
                        context_result["siblingBeforeClose"] = sibling_page.evaluate(
                            """({
                              sentinel: globalThis.__obscuraPageBSentinel,
                              heldFetchState: globalThis.__obscuraHeldFetchState,
                              title: document.title
                            })"""
                        )
                        context_result["eventCountBeforeClose"] = len(closed_page_events)
                        close_phase["value"] = "closing"
                        sibling_page.close()
                        close_phase["value"] = "closed"
                        context_result["pageCountAfterSiblingClose"] = len(
                            isolated_context.pages
                        )
                        context_result["siblingPageClosed"] = sibling_page.is_closed()
                        try:
                            sibling_page.evaluate("document.title")
                        except PlaywrightError as error:
                            context_result["closedPageError"] = str(error)
                        else:
                            raise AssertionError("closed sibling page still accepted evaluation")
                        context_result["heldRequestRelease"] = fixture_control_get(
                            f"{fixture_origin}/lifecycle-hold/release"
                        )
                        held_finish_observations: list[dict[str, Any]] = []
                        context_result["heldRequestFinishObservations"] = (
                            held_finish_observations
                        )
                        context_result["heldRequestFinished"] = wait_for_fixture_hold(
                            fixture_origin, "finished", held_finish_observations
                        )
                        context_result["survivorAfterSiblingClose"] = {
                            "closure": isolated_page.evaluate(
                                "globalThis.__obscuraPageAClosure()"
                            ),
                            "timer": isolated_page.evaluate(
                                "() => globalThis.__obscuraPageAPromise"
                            ),
                            "handle": page_a_handle.json_value(),
                            "title": isolated_page.title(),
                        }
                        isolated_page.wait_for_timeout(100)
                        context_result["postCloseRuntimeEvents"] = [
                            item
                            for item in closed_page_events
                            if item["phase"] == "closed"
                        ]
                        page_a_handle.dispose()
                        page_a_handle = None
                        isolated_context.close()
                        isolated_context = None
                        context_result["closedContextCount"] = len(browser.contexts)
                        context_result["isolatedPageClosed"] = isolated_page.is_closed()
                        context_result["defaultAfterClose"] = page.evaluate(
                            """({
                              storage: localStorage.getItem('__obscuraContextOwner'),
                              defaultSentinel: globalThis.__obscuraDefaultContextSentinel,
                              title: document.title
                            })"""
                        )
                    finally:
                        if page_a_handle is not None:
                            try:
                                page_a_handle.dispose()
                            except PlaywrightError:
                                pass
                        if isolated_context is not None:
                            isolated_context.close()

                    expected_context = {
                        "initialContextCount": 1,
                        "createdContextCount": 2,
                        "isolatedBefore": {
                            "storage": None,
                            "defaultSentinel": None,
                            "viewport": [1280, 720, 1280, 720],
                            "screen": [1280, 720, 1280, 720],
                            "devicePixelRatio": 1,
                        },
                        "defaultWhileOpen": {
                            "storage": "default",
                            "defaultSentinel": 73,
                            "isolatedSentinel": None,
                        },
                        "isolatedWhileOpen": {
                            "storage": "isolated",
                            "defaultSentinel": None,
                            "isolatedSentinel": 91,
                        },
                        "pageCountWhileOpen": 2,
                        "heldFetchInitialState": "pending",
                        "siblingBeforeClose": {
                            "sentinel": "page-b",
                            "heldFetchState": "pending",
                            "title": "OB-026 CDP fixture",
                        },
                        "eventCountBeforeClose": 0,
                        "pageCountAfterSiblingClose": 1,
                        "siblingPageClosed": True,
                        "survivorAfterSiblingClose": {
                            "closure": 41,
                            "timer": "page-a-timer",
                            "handle": {"owner": "page-a", "value": 41},
                            "title": "OB-026 CDP fixture",
                        },
                        "postCloseRuntimeEvents": [],
                        "closedContextCount": 1,
                        "isolatedPageClosed": True,
                        "defaultAfterClose": {
                            "storage": "default",
                            "defaultSentinel": 73,
                            "title": "OB-026 CDP fixture",
                        },
                    }
                    for name, expected in expected_context.items():
                        if context_result.get(name) != expected:
                            raise AssertionError(
                                f"browser context {name} differs from Chrome: {context_result!r}"
                            )
                    if "closed" not in context_result.get("closedPageError", "").lower():
                        raise AssertionError(
                            f"closed page returned the wrong error: {context_result!r}"
                        )
                    started = context_result["heldRequestStarted"]["state"]
                    finished = context_result["heldRequestFinished"]["state"]
                    release = context_result["heldRequestRelease"]
                    if started != {
                        "started": True,
                        "released": False,
                        "finished": False,
                    }:
                        raise AssertionError(
                            f"held request was not pending before close: {context_result!r}"
                        )
                    if finished != {
                        "started": True,
                        "released": True,
                        "finished": True,
                    }:
                        raise AssertionError(
                            f"held request did not finish after release: {context_result!r}"
                        )
                    if release["status"] != 200 or json.loads(release["body"]) != {
                        "released": True
                    }:
                        raise AssertionError(
                            f"held request release failed: {context_result!r}"
                        )

                    session = context.new_cdp_session(page)
                    document = trace.send(
                        session,
                        "DOM.getDocument",
                        {"depth": 1},
                        session_name="page-1",
                    )
                    if document.get("root", {}).get("nodeName") != "#document":
                        raise AssertionError("DOM.getDocument did not return the document root")
                    trace.send(session, "Log.enable", session_name="page-1")
                    unknown_error = expect_protocol_error(
                        trace,
                        session,
                        "Log.auditMethodDoesNotExist",
                        {},
                        "Unknown Log method: auditMethodDoesNotExist",
                    )
                    invalid_params_error = expect_protocol_error(
                        trace,
                        session,
                        "Log.enable",
                        {"invented": True},
                        "Log.enable supports only empty params",
                    )

                    result["document"] = document
                    result["unknownMethodError"] = unknown_error
                    result["invalidParamsError"] = invalid_params_error
                    result["status"] = "passed"
                finally:
                    browser.close()
    except Exception as error:
        result["status"] = "failed"
        result["error"] = {
            "type": type(error).__name__,
            "message": str(error),
        }
    finally:
        if trace is not None:
            result["cdpTrace"] = trace.document()
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obscura-bin", type=Path, default=Path("target/release/obscura"))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    result = run(
        args.obscura_bin,
        log_root=args.output.parent.resolve() if args.output is not None else None,
    )
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())

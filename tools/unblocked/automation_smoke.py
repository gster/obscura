#!/usr/bin/env python3
"""Required official Playwright Python smoke for the OB-027 CDP profile."""

from __future__ import annotations

import argparse
import importlib.metadata
import json
from pathlib import Path
from typing import Any

if __package__:
    from .cdp_fixture import external_process, fixture_server, free_port
    from .cdp_trace import Trace
else:
    from cdp_fixture import external_process, fixture_server, free_port
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


def run(obscura_bin: Path) -> dict[str, Any]:
    from playwright.sync_api import sync_playwright

    result: dict[str, Any] = {
        "schemaVersion": 1,
        "playwrightVersion": importlib.metadata.version("playwright"),
        "connection": "BrowserType.connect_over_cdp",
        "status": "running",
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
        with fixture_server() as fixture_origin, external_process(command, endpoint):
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
                          const button = document.querySelector('#submit');
                          for (const type of ['focus', 'input', 'change']) {
                            input.addEventListener(type, event => {
                              __obscuraLocatorEvents.push({
                                target: 'input', type, trusted: event.isTrusted,
                                value: input.value
                              });
                            });
                          }
                          for (const type of ['input', 'change']) {
                            select.addEventListener(type, event => {
                              __obscuraLocatorEvents.push({
                                target: 'select', type, trusted: event.isTrusted,
                                value: select.value
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
                    try:
                        page.get_by_label("Name").fill("official-playwright")
                        result["selectedOptions"] = page.get_by_label("City").select_option("pek")
                        page.get_by_role("button", name="Submit").click()
                    finally:
                        try:
                            result["locator"] = page.evaluate(
                                """({
                                  input: document.querySelector('#name').value,
                                  city: document.querySelector('#city').value,
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
                        ("button", "mousedown"),
                        ("button", "mouseup"),
                        ("button", "click"),
                    ]
                    positions = [observed_events.index(event) for event in required_events]
                    if positions != sorted(positions):
                        raise AssertionError(f"locator events are out of order: {locator!r}")
                    for event in locator["events"]:
                        expected_trusted = event["target"] != "select"
                        if event.get("trusted") is not expected_trusted:
                            raise AssertionError(f"locator event trust differs from Chrome: {locator!r}")

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
    result = run(args.obscura_bin)
    rendered = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(rendered, encoding="utf-8")
    print(rendered, end="")
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())

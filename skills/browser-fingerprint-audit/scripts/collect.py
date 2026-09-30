#!/usr/bin/env python3
"""Read-only browser audit collector; attach to an existing dedicated CDP profile.

This collects evidence, NOT a pass/fail certification. No browser launches,
spoofing, network interception, CAPTCHA handling, or browser.close().
Creates a fresh anonymous context by default and disposes only that owned context.
Driver attachment itself can have observable effects/default overrides.
Public interfaces checked 2026-09-30; see ../references/sources.md.
"""
from __future__ import annotations

import argparse
import asyncio
import hashlib
import importlib.metadata
import inspect
import ipaddress
import json
import math
import os
from pathlib import Path
import re
import sys
import time
from datetime import datetime, timezone
from typing import Any, Awaitable, Callable
from urllib.parse import urlsplit, urlunsplit

SITES = {
    "tls-peet": "https://tls.peet.ws/api/all",
    "fingerprint-scan": "https://fingerprint-scan.com/",
    "rebrowser": "https://bot-detector.rebrowser.net/",
}
REBROWSER_TYPES = {
    "dummyFn", "sourceUrlLeak", "mainWorldExecution", "runtimeEnableLeak",
    "exposeFunctionLeak", "navigatorWebdriver", "bypassCsp", "viewport",
    "useragent", "pwInitScripts",
}


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def safe_url(url: str) -> str:
    """Do not persist userinfo, query strings, or fragments in metadata/logs."""
    try:
        p = urlsplit(url)
        host = p.hostname or ""
        if ":" in host:
            host = f"[{host}]"
        if p.port:
            host += f":{p.port}"
        return urlunsplit((p.scheme, host, p.path, "", ""))
    except ValueError:
        return "[invalid-url]"


def error_text(exc: BaseException, endpoint: str = "") -> str:
    text = str(exc)
    if endpoint:
        text = text.replace(endpoint, "[CDP endpoint]")
    text = re.sub(r"(?:https?|wss?)://[^\s\"'<>]+", lambda m: safe_url(m.group()), text)
    return f"{type(exc).__name__}: {text[:2500]}"


def write_json(path: Path, data: Any) -> None:
    # Fail rather than silently converting a non-serializable value to a string.
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2, allow_nan=False) + "\n", encoding="utf-8")


def validate_data(site: str, raw: Any) -> list[str]:
    """Return schema/coverage problems without turning absent fields into false."""
    problems: list[str] = []
    if site == "fingerprint-scan":
        if not isinstance(raw, dict):
            return ["result_not_object"]
        if raw.get("status") != "COLLECT_SUCCESSFUL":
            problems.append("collection_not_successful")
        for key in ("fingerprintHash", "fingerprintId"):
            if not isinstance(raw.get(key), str) or not raw[key]:
                problems.append(f"missing_or_invalid:{key}")
        for key in ("fingerprintParts", "signals"):
            if not isinstance(raw.get(key), dict) or not raw[key]:
                problems.append(f"missing_or_invalid:{key}")
        score = raw.get("botScore")
        if isinstance(score, bool) or not isinstance(score, (int, float)) or not math.isfinite(score):
            problems.append("missing_or_invalid:botScore")
        if not isinstance(raw.get("reasons"), list):
            problems.append("missing_or_invalid:reasons")
    elif site == "tls-peet":
        if not isinstance(raw, dict):
            return ["result_not_object"]
        if not isinstance(raw.get("tls"), dict) or not raw["tls"]:
            problems.append("missing_or_invalid:tls")
        for key in ("ip", "http_version"):
            if not isinstance(raw.get(key), str) or not raw[key]:
                problems.append(f"missing_or_invalid:{key}")
    elif site == "rebrowser":
        if not isinstance(raw, list) or not raw:
            return ["result_not_nonempty_array"]
        seen: set[str] = set()
        for index, row in enumerate(raw):
            if not isinstance(row, dict) or not isinstance(row.get("type"), str):
                problems.append(f"invalid_row:{index}")
                continue
            seen.add(row["type"])
            rating = row.get("rating")
            if isinstance(rating, bool) or not isinstance(rating, (int, float)) or not math.isfinite(rating):
                problems.append(f"invalid_rating:{row['type']}")
        problems.extend(f"missing_test:{key}" for key in sorted(REBROWSER_TYPES - seen))
    else:
        problems.append("unknown_adapter")
    return problems


def validate_endpoint(endpoint: str, allow_remote: bool) -> None:
    p = urlsplit(endpoint)
    if p.scheme not in {"http", "https", "ws", "wss"} or not p.hostname:
        raise ValueError("--cdp must be an HTTP(S) or WS(S) CDP endpoint")
    if p.username or p.password:
        raise ValueError("Do not put credentials in the CDP URL; use an authorized local tunnel")
    is_local = p.hostname.lower() == "localhost"
    try:
        is_local = is_local or ipaddress.ip_address(p.hostname).is_loopback
    except ValueError:
        pass
    if not is_local and not allow_remote:
        raise ValueError("Remote CDP requires explicit --allow-remote-cdp; prefer an authorized local tunnel")


class SiteProblem(Exception):
    def __init__(self, status: str, message: str):
        super().__init__(message)
        self.status = status


async def action(
    result: dict[str, Any], name: str, world: str,
    fn: Callable[[], Awaitable[Any]], timeout_seconds: float,
) -> None:
    step: dict[str, Any] = {"name": name, "world": world, "started_at": utc_now(), "succeeded": False}
    result["actions"].append(step)
    try:
        value = await asyncio.wait_for(fn(), timeout=timeout_seconds)
        if value is False:
            raise RuntimeError("probe returned false")
        step["succeeded"] = True
    except Exception as exc:
        step["error"] = error_text(exc)
    finally:
        step["finished_at"] = utc_now()


async def isolated_probe(page: Any) -> bool:
    """Diagnostic only. Does not change the default Playwright evaluate world."""
    session = await page.context.new_cdp_session(page)
    try:
        tree = await session.send("Page.getFrameTree")
        frame_id = tree["frameTree"]["frame"]["id"]
        world = await session.send("Page.createIsolatedWorld", {
            "frameId": frame_id, "worldName": "browser-fingerprint-audit-diagnostic",
        })
        output = await session.send("Runtime.evaluate", {
            "contextId": world["executionContextId"],
            "expression": "document.getElementsByClassName('div'); true",
            "returnByValue": True,
        })
        if "exceptionDetails" in output:
            raise RuntimeError(f"isolated evaluation failed: {output['exceptionDetails']}")
        if output.get("result", {}).get("value") is not True:
            raise RuntimeError("isolated evaluation did not confirm execution")
        return True
    finally:
        # No Browser.close/Page.close command is sent by this diagnostic session.
        await session.detach()


async def collect_fingerprint(page: Any, result: dict[str, Any], args: argparse.Namespace) -> None:
    deadline = time.monotonic() + args.collection_timeout
    await page.wait_for_function(
        "() => window.FINGERPRINT_SCAN?.status === 'COLLECT_SUCCESSFUL'",
        timeout=args.collection_timeout * 1000,
    )
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        raise asyncio.TimeoutError("Fingerprint Scan collection deadline exceeded")
    result["raw"] = await asyncio.wait_for(page.evaluate("() => window.FINGERPRINT_SCAN"), timeout=remaining)
    result["coverage"] = {"scope": "public-result-object", "behavior_exercised": False}


async def collect_rebrowser(page: Any, result: dict[str, Any], args: argparse.Namespace) -> None:
    output = page.locator("#detections-json")
    await output.wait_for(state="attached", timeout=args.collection_timeout * 1000)
    # Only wait for initialization here; some rows depend on the explicit actions.
    while True:
        text = await output.input_value(timeout=3000)
        try:
            raw = json.loads(text)
        except json.JSONDecodeError:
            raw = None
        if isinstance(raw, list) and raw:
            result["raw"] = raw
            break
        await asyncio.sleep(0.5)

    if args.rebrowser_actions != "none":
        await action(result, "dummyFn", "main-via-official-playwright", lambda: page.evaluate(
            "() => window.dummyFn()"), args.action_timeout)
        await action(result, "exposeFunction", "page-binding", lambda: page.expose_function(
            "exposedFn", lambda: None), args.action_timeout)
        await action(result, "sourceUrl", "main-via-official-playwright", lambda: page.evaluate(
            "() => { document.getElementById('detections-json'); return true; }"), args.action_timeout)
        if args.rebrowser_actions == "main":
            await action(result, "mainWorldProbe", "main-via-official-playwright", lambda: page.evaluate(
                "() => { document.getElementsByClassName('div'); return true; }"), args.action_timeout)
        else:
            await action(result, "mainWorldProbe", "isolated-cdp-diagnostic",
                         lambda: isolated_probe(page), args.action_timeout)

    # This is an observation window, NOT proof of terminal completion.
    await asyncio.sleep(args.observe_seconds)
    while True:
        text = await output.input_value(timeout=3000)
        try:
            raw = json.loads(text)
        except json.JSONDecodeError:
            raw = None
        if raw is not None:
            result["raw"] = raw
            problems = validate_data("rebrowser", raw)
            if not problems:
                break
        await asyncio.sleep(0.5)
    result["coverage"] = {
        "scope": "observation" if args.rebrowser_actions == "none" else args.rebrowser_actions,
        "requested_actions_succeeded": all(x["succeeded"] for x in result["actions"])
            if result["actions"] else None,
        "active_actions_requested": args.rebrowser_actions != "none",
        "observation_window_seconds": args.observe_seconds,
        "rating_zero_rows": [r["type"] for r in raw if r.get("rating") == 0],
        "note": "rating 0 is not PASS; inspect action/world evidence and the raw note",
    }


async def save_snapshots(page: Any, directory: Path, result: dict[str, Any]) -> None:
    """Supporting artifacts; failure does not erase a valid structured result."""
    try:
        text = await asyncio.wait_for(page.locator("body").inner_text(timeout=4000), timeout=5)
        limit = 250_000
        (directory / "page.txt").write_text(text[:limit], encoding="utf-8")
        result["evidence"]["page_text"] = "page.txt"
        result["evidence"]["page_text_truncated"] = len(text) > limit
    except Exception as exc:
        result["artifact_errors"].append(error_text(exc))
    try:
        await asyncio.wait_for(page.screenshot(path=str(directory / "screenshot.png"),
                                              full_page=False, timeout=5000), timeout=6)
        result["evidence"]["screenshot"] = "screenshot.png"
    except Exception as exc:
        result["artifact_errors"].append(error_text(exc))


async def collect_one(context: Any, site: str, output_root: Path, args: argparse.Namespace) -> dict[str, Any]:
    from playwright.async_api import TimeoutError as PlaywrightTimeout

    directory = output_root / site
    directory.mkdir()
    result: dict[str, Any] = {
        "schema_version": "1.0", "site": site, "url": SITES[site],
        "started_at": utc_now(), "execution_status": "NOT_RUN",
        "comparison_verdict": "INCONCLUSIVE", "review_required": True,
        "method": {"fingerprint-scan": "public-page-object", "tls-peet": "browser-navigation-response",
                   "rebrowser": "verified-dom-value"}[site],
        "actions": [], "coverage": {}, "schema_problems": [], "errors": [],
        "page_errors": [], "artifact_errors": [], "evidence": {},
    }
    page = None
    started = time.monotonic()
    try:
        page = await asyncio.wait_for(context.new_page(), timeout=args.action_timeout)
        page.set_default_timeout(args.action_timeout * 1000)
        page.on("pageerror", lambda error: result["page_errors"].append(error_text(error)))
        page.on("crash", lambda _: result["errors"].append("page_crash"))
        response = await page.goto(SITES[site], wait_until="domcontentloaded",
                                   timeout=args.navigation_timeout * 1000)
        result["final_url"] = safe_url(page.url)
        if response is None:
            raise SiteProblem("TOOL_ERROR", "navigation produced no main-document response")
        result["http_status"] = response.status
        expected_host = urlsplit(SITES[site]).hostname
        if urlsplit(response.url).hostname != expected_host:
            raise SiteProblem("BLOCKED", "unexpected cross-host navigation; no further actions performed")
        if response.status in {401, 403, 407, 429}:
            raise SiteProblem("BLOCKED", f"HTTP {response.status}; no automatic retries")
        if not response.ok:
            raise SiteProblem("TOOL_ERROR", f"HTTP {response.status}")
        headers = await asyncio.wait_for(response.all_headers(), timeout=5)
        result["response_metadata"] = {k: headers[k] for k in
            ("content-type", "etag", "last-modified", "retry-after") if k in headers}
        body = await asyncio.wait_for(response.body(), timeout=10)
        result["main_document_sha256"] = hashlib.sha256(body).hexdigest()

        if site == "tls-peet":
            # These are the bytes of the actual browser navigation, not requests/curl/APIRequestContext.
            try:
                result["raw"] = json.loads(body.decode("utf-8-sig"))
            except (UnicodeDecodeError, json.JSONDecodeError) as exc:
                raise SiteProblem("PARTIAL", "expected browser JSON response; received non-JSON or changed schema") from exc
            result["coverage"] = {"scope": "this-browser-navigation", "transport_request": "browser"}
        elif site == "fingerprint-scan":
            await collect_fingerprint(page, result, args)
        else:
            await asyncio.wait_for(collect_rebrowser(page, result, args), timeout=args.collection_timeout)
        result["schema_problems"] = validate_data(site, result.get("raw"))
        action_failed = any(not step["succeeded"] for step in result["actions"])
        result["execution_status"] = "PARTIAL" if result["schema_problems"] or action_failed else "COMPLETE"
    except SiteProblem as exc:
        result["execution_status"] = exc.status
        result["errors"].append(error_text(exc, args.cdp))
    except (asyncio.TimeoutError, PlaywrightTimeout) as exc:
        result["execution_status"] = "TIMEOUT"
        result["errors"].append(error_text(exc, args.cdp))
    except Exception as exc:
        result["execution_status"] = "TOOL_ERROR"
        result["errors"].append(error_text(exc, args.cdp))
    finally:
        # On failures retain any available partial data, without promoting it to success.
        if page is not None and not page.is_closed():
            if "raw" not in result:
                try:
                    if site == "fingerprint-scan":
                        partial = await asyncio.wait_for(page.evaluate("() => window.FINGERPRINT_SCAN ?? null"), timeout=3)
                    elif site == "rebrowser":
                        partial = json.loads(await asyncio.wait_for(
                            page.locator("#detections-json").input_value(timeout=2000), timeout=3))
                    else:
                        partial = None
                    if partial is not None:
                        result["raw"] = partial
                        result["partial_data_recovered"] = True
                except Exception:
                    pass  # Original error is already preserved; no fake empty result.
            if args.snapshots:
                await save_snapshots(page, directory, result)
            try:
                await asyncio.wait_for(page.close(), timeout=5)
            except Exception as exc:
                result["artifact_errors"].append("owned_tab_close: " + error_text(exc, args.cdp))
        if "raw" in result:
            write_json(directory / "raw.json", result.pop("raw"))
            result["evidence"]["raw"] = "raw.json"
        result["finished_at"] = utc_now()
        result["elapsed_seconds"] = round(time.monotonic() - started, 3)
        write_json(directory / "result.json", result)
    return result


async def select_context(browser: Any, args: argparse.Namespace, manifest: dict[str, Any]) -> tuple[Any, bool]:
    """Return a test context and its ownership; never reuse a Chrome control's state."""
    role = manifest.get("role")
    if role not in {"candidate", "baseline"}:
        raise ValueError("manifest role must be candidate or baseline")
    if args.context_mode == "fresh":
        # Keep the browser's viewport; do not add UA/locale/timezone spoofing.
        context = await asyncio.wait_for(browser.new_context(no_viewport=True), timeout=args.action_timeout)
        return context, True
    if role == "baseline":
        raise ValueError("Chrome baselines require a newly created anonymous context")
    if args.context_index >= len(browser.contexts):
        raise ValueError("--context-index is outside existing contexts")
    return browser.contexts[args.context_index], False


async def main_async(args: argparse.Namespace) -> int:
    from playwright.async_api import async_playwright

    validate_endpoint(args.cdp, args.allow_remote_cdp)
    if not args.dedicated_profile:
        raise ValueError("Use --dedicated-profile only after verifying this is a disposable test profile")
    manifest = json.loads(Path(args.manifest).read_text(encoding="utf-8"))
    if not isinstance(manifest, dict) or manifest.get("profile_dedicated") is not True:
        raise ValueError("manifest must be an object with profile_dedicated=true")
    if manifest.get("role") not in {"candidate", "baseline"}:
        raise ValueError("manifest role must be candidate or baseline")
    if manifest["role"] == "baseline" and args.context_mode != "fresh":
        raise ValueError("Chrome baselines require --context-mode fresh")
    if manifest.get("automation", {}).get("transport") not in {None, "cdp"}:
        raise ValueError("This collector measures CDP, not a manual/uninstrumented session")
    if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_.-]{0,79}", args.label):
        raise ValueError("--label must contain 1-80 letters, digits, dots, hyphens, or underscores")
    root = Path(args.out).expanduser().resolve()
    root.mkdir(parents=True, exist_ok=False)  # Never overwrite a prior audit.
    os.chmod(root, 0o700)
    meta = {
        "started_at": utc_now(), "label": args.label, "declared_environment": manifest,
        "collector_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "python_version": sys.version.split()[0],
        "playwright_version": importlib.metadata.version("playwright"),
        "connection_transport": "cdp", "cdp_endpoint": "[not persisted]",
        "observer": {"official_playwright_attach": True, "pageerror_listener": True,
                     "console_listener": False,
                     "explicit_page_evaluation": "fingerprint-scan" in args.sites or
                         ("rebrowser" in args.sites and args.rebrowser_actions != "none"),
                     "dom_reads": "rebrowser" in args.sites or args.snapshots,
                     "snapshots_enabled": args.snapshots,
                     "note": "Not an uninstrumented headed-Chrome baseline"},
        "sites": args.sites, "rebrowser_actions": args.rebrowser_actions,
        "timeouts_seconds": {"navigation": args.navigation_timeout, "collection": args.collection_timeout,
                             "action": args.action_timeout},
    }
    write_json(root / "manifest.json", meta)
    results: list[dict[str, Any]] = []
    exit_code = 1
    playwright = None
    context = None
    owns_context = False
    try:
        playwright = await async_playwright().start()
        connect_kwargs: dict[str, Any] = {"timeout": args.navigation_timeout * 1000}
        supports_no_defaults = "no_defaults" in inspect.signature(playwright.chromium.connect_over_cdp).parameters
        if args.cdp_context_defaults == "preserve" and not supports_no_defaults:
            raise RuntimeError("This Playwright version has no no_defaults option; preserve mode cannot be guaranteed")
        if supports_no_defaults:
            connect_kwargs["no_defaults"] = args.cdp_context_defaults != "framework"
        meta["observer"].update({
            "cdp_context_defaults_requested": args.cdp_context_defaults,
            "supports_no_defaults": supports_no_defaults,
            "no_defaults_argument": connect_kwargs.get("no_defaults"),
            "default_context_overrides_possible": not connect_kwargs.get("no_defaults", False),
        })
        browser = await playwright.chromium.connect_over_cdp(args.cdp, **connect_kwargs)
        meta["reported_browser_version"] = browser.version
        context, owns_context = await select_context(browser, args, manifest)
        meta["context"] = {"mode": args.context_mode, "owned": owns_context,
                           "new_context_options": {"no_viewport": True} if owns_context else None,
                           "existing_index": None if owns_context else args.context_index}
        meta["existing_tab_count"] = len(context.pages)
        write_json(root / "manifest.json", meta)
        for index, site in enumerate(args.sites):
            if index:
                await asyncio.sleep(2)  # Modest serial pacing, not fingerprint manipulation.
            results.append(await collect_one(context, site, root, args))
            write_json(root / "results.json", {"schema_version": "1.0", "results": results})
        exit_code = 0 if all(r["execution_status"] == "COMPLETE" for r in results) else 2
    except Exception as exc:
        meta["fatal_error"] = error_text(exc, args.cdp)
    finally:
        # Dispose only the context created by this run, before disconnecting.
        if owns_context and context is not None:
            try:
                await asyncio.wait_for(context.close(), timeout=5)
                meta["owned_context_closed"] = True
            except Exception as exc:
                meta["context_close_error"] = error_text(exc, args.cdp)
                exit_code = 1
        # Disconnect the client transport. Do NOT close the external browser.
        if playwright is not None:
            try:
                await asyncio.wait_for(playwright.stop(), timeout=10)
            except Exception as exc:
                meta["disconnect_error"] = error_text(exc, args.cdp)
                exit_code = 1
        meta["finished_at"] = utc_now()
        meta["exit_code"] = exit_code
        write_json(root / "manifest.json", meta)
        write_json(root / "results.json", {"schema_version": "1.0", "results": results,
            "unexecuted_sites": [site for site in args.sites if site not in {r['site'] for r in results}],
            "note": "COMPLETE means evidence collected, not PASS. Agent comparison/review is required."})
    print(json.dumps({"output_directory": str(root), "exit_code": exit_code,
                      "states": {r["site"]: r["execution_status"] for r in results}}, ensure_ascii=False))
    return exit_code


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cdp", required=True, help="Existing browser CDP endpoint; preferably loopback")
    parser.add_argument("--label", required=True)
    parser.add_argument("--manifest", required=True, help="Environment JSON; see assets/run-manifest.example.json")
    parser.add_argument("--out", required=True, help="New output directory; existing paths are rejected")
    parser.add_argument("--dedicated-profile", action="store_true")
    parser.add_argument("--allow-remote-cdp", action="store_true")
    parser.add_argument("--context-mode", choices=["fresh", "existing"], default="fresh",
                        help="fresh creates an owned anonymous context; existing is candidate-only")
    parser.add_argument("--context-index", type=int, default=0)
    parser.add_argument("--cdp-context-defaults", choices=["auto", "framework", "preserve"], default="auto",
                        help="auto uses no_defaults when supported and records older-version limitations; preserve requires support")
    parser.add_argument("--sites", nargs="+", choices=list(SITES), default=list(SITES))
    parser.add_argument("--rebrowser-actions", choices=["none", "main", "isolated"], default="none")
    parser.add_argument("--navigation-timeout", type=float, default=45)
    parser.add_argument("--collection-timeout", type=float, default=60)
    parser.add_argument("--action-timeout", type=float, default=15)
    parser.add_argument("--observe-seconds", type=float, default=2,
                        help="Bounded Rebrowser observation window after actions, not a completion signal")
    parser.add_argument("--no-snapshots", dest="snapshots", action="store_false", default=True,
                        help="Disable supporting text/screenshot; does not make CDP uninstrumented")
    args = parser.parse_args()
    if args.context_index < 0:
        parser.error("--context-index must be nonnegative")
    if args.context_mode == "fresh" and args.context_index != 0:
        parser.error("--context-index is only used with --context-mode existing")
    for name in ("navigation_timeout", "collection_timeout", "action_timeout", "observe_seconds"):
        if not 0 < getattr(args, name) <= 180:
            parser.error(f"--{name.replace('_','-')} must be in (0, 180]")
    args.sites = list(dict.fromkeys(args.sites))
    return args


if __name__ == "__main__":
    os.umask(0o077)
    arguments = parse_args()
    try:
        sys.exit(asyncio.run(main_async(arguments)))
    except KeyboardInterrupt:
        print("Interrupted; inspect partial artifacts. No success claim was made.", file=sys.stderr)
        sys.exit(130)
    except Exception as error:
        print(error_text(error, arguments.cdp), file=sys.stderr)
        sys.exit(1)

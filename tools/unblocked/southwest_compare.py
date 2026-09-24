#!/usr/bin/env python3
"""Capture a fresh Chrome and Obscura Southwest session through one proxy.

Raw CDP events, response bodies and Chrome NetLog contain session secrets.
Keep the output directory private and outside the repository.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import socket
import subprocess
import tempfile
import time
import urllib.request
from collections import Counter
from pathlib import Path
from urllib.parse import urlparse

if __package__:
    from .southwest_report import make_report
else:
    from southwest_report import make_report


DEFAULT_URL = (
    "https://www.southwest.com/air/booking/select-depart.html"
    "?adultPassengersCount=1&adultsCount=1&destinationAirportCode=MCO"
    "&departureDate=2026-09-30&departureTimeOfDay=ALL_DAY&fareType=USD"
    "&int=HOMEQBOMAIR&originationAirportCode=BWI&passengerType=ADULT"
    "&tripType=oneway&returnDate=&returnTimeOfDay=ALL_DAY&promoCode="
)
CONTEXT_OPTIONS = {"viewport": {"width": 1365, "height": 768},
                   "screen": {"width": 1365, "height": 768}, "device_scale_factor": 2}
EVENTS = (
    "Network.requestWillBeSent", "Network.requestWillBeSentExtraInfo",
    "Network.responseReceived", "Network.responseReceivedExtraInfo",
    "Network.loadingFinished", "Network.loadingFailed",
    "Page.frameNavigated", "Page.domContentEventFired", "Page.loadEventFired",
    "Runtime.executionContextCreated", "Runtime.consoleAPICalled",
    "Runtime.exceptionThrown", "Debugger.scriptParsed",
    "Obscura.scriptExecution", "Obscura.storageMutation", "Obscura.cookieWrite",
    "DOMStorage.domStorageItemAdded", "DOMStorage.domStorageItemUpdated",
    "DOMStorage.domStorageItemRemoved", "DOMStorage.domStorageItemsCleared",
)
SNAPSHOT_JS = """(() => {
  const storage = {};
  try { for (let i = 0; i < localStorage.length; i++) {
    const k = localStorage.key(i); storage[k] = localStorage.getItem(k);
  }} catch (e) { storage.__error = String(e); }
  return {url: location.href, title: document.title,
    readyState: document.readyState, localStorage: storage,
    cookie: document.cookie, bodyTextStart: document.body?.innerText?.slice(0, 600),
    identity: {userAgent: navigator.userAgent, language: navigator.language,
      languages: navigator.languages, platform: navigator.platform,
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      viewport: [innerWidth, innerHeight], devicePixelRatio,
      screen: {width: screen.width, height: screen.height,
        availWidth: screen.availWidth, availHeight: screen.availHeight,
        colorDepth: screen.colorDepth, pixelDepth: screen.pixelDepth},
      outer: [outerWidth, outerHeight]}};
})()"""


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def save_json(path: Path, value: object) -> None:
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2, default=str) + "\n")


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def wait_endpoint(port: int, process: subprocess.Popen, seconds: float = 20) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Obscura exited during startup: {process.returncode}")
        try:
            with urllib.request.urlopen(f"http://127.0.0.1:{port}/json/version", timeout=1):
                return
        except Exception:
            time.sleep(0.2)
    raise RuntimeError("Obscura CDP endpoint did not become ready")


def classify(url: str) -> str:
    path = urlparse(url).path.lower()
    if "/booking/shopping" in path:
        return "shopping"
    if "/akam/" in path or "akamai" in url.lower():
        return "akamai"
    if "/resources/" in path and len(path.rsplit("/", 1)[-1]) > 20:
        return "protection-resource"
    if path.endswith(".js"):
        return "script"
    if "select-depart.html" in path:
        return "document"
    return "other"


def cookie_names(header: str) -> list[str]:
    return [part.split("=", 1)[0].strip() for part in header.split(";") if "=" in part]


def set_cookie_names(header: str) -> list[str]:
    return [line.split("=", 1)[0].strip() for line in header.splitlines() if "=" in line]


def headers_summary(headers: dict) -> dict:
    out = {}
    for name, value in headers.items():
        lower = name.lower()
        value = str(value)
        if lower == "cookie":
            out[lower] = {"names": cookie_names(value), "bytes": len(value)}
        elif lower == "set-cookie":
            out[lower] = {"names": set_cookie_names(value), "bytes": len(value),
                          "sha256": sha(value.encode())}
        elif lower.startswith("ee30") or lower == "authorization":
            out[lower] = {"bytes": len(value), "sha256": sha(value.encode())}
        else:
            out[lower] = value
    return out


def response_headers_summary(rec: dict, headers: dict) -> dict:
    summary = headers_summary(headers)
    raw = rec.get("response", {}).get("rawHeaders") or rec.get("responseExtraInfo", {}).get("rawHeaders") or {}
    values = [base64.b64decode(field["valueBase64"]) for field in raw.get("fields", [])
              if base64.b64decode(field["nameBase64"]).lower() == b"set-cookie"]
    if values:
        joined = b"\n".join(values)
        summary["set-cookie"] = {"names": [v.split(b"=", 1)[0].decode("ascii", "replace")
                                            for v in values if b"=" in v],
                                 "bytes": len(joined), "sha256": sha(joined)}
    return summary


def collect(browser, mode: str, url: str, output: Path, wait_seconds: int) -> dict:
    # new_context is a newly created anonymous/incognito BrowserContext for BOTH engines.
    options = dict(CONTEXT_OPTIONS)
    if mode == "chrome":
        options.update(locale="en-US", timezone_id="Asia/Shanghai")
    context = browser.new_context(**options)
    page = context.new_page()
    session = context.new_cdp_session(page)
    start = time.monotonic()
    start_wall = time.time()
    events = []
    requests = {}
    snapshots = []
    capability = {}
    errors = []
    playwright_requests = []

    def on_request(request):
        playwright_requests.append({"url": request.url, "method": request.method,
                                    "atMs": round((time.monotonic()-start)*1000, 3),
                                    "headers": request.headers,
                                    "postData": request.post_data})

    def on_response(response):
        for row in reversed(playwright_requests):
            if row["url"] == response.url and row["method"] == response.request.method and "status" not in row:
                row.update(status=response.status, responseHeaders=response.headers,
                           responseAtMs=round((time.monotonic()-start)*1000, 3))
                break

    page.on("request", on_request)
    page.on("response", on_response)

    def observe(name):
        def handler(params):
            event = {"atMs": round((time.monotonic() - start) * 1000, 3),
                     "method": name, "params": params}
            events.append(event)
            rid = params.get("requestId")
            if rid:
                rec = requests.setdefault(rid, {"requestId": rid})
                if name == "Network.requestWillBeSent":
                    req = params.get("request", {})
                    rec.update(url=req.get("url"), method=req.get("method"),
                               atMs=event["atMs"], request=req,
                               protocolTimestamp=params.get("timestamp"),
                               initiator=params.get("initiator"), type=params.get("type"))
                elif name == "Network.requestWillBeSentExtraInfo":
                    rec["requestExtraInfo"] = params
                elif name == "Network.responseReceived":
                    rec["response"] = params.get("response", {})
                    rec["responseAtMs"] = event["atMs"]
                elif name == "Network.responseReceivedExtraInfo":
                    rec["responseExtraInfo"] = params
                elif name in ("Network.loadingFinished", "Network.loadingFailed"):
                    rec["end"] = {"method": name, "atMs": event["atMs"], "params": params}
        session.on(name, handler)

    for name in EVENTS:
        observe(name)
    for domain in ("Network", "Page", "Runtime", "Debugger", "DOMStorage"):
        try:
            session.send(domain + ".enable")
            capability[domain] = "enabled"
        except Exception as exc:
            capability[domain] = f"unavailable: {exc}"

    def snapshot(label):
        try:
            value = page.evaluate(SNAPSHOT_JS)
            snapshots.append({"label": label, "atMs": round((time.monotonic()-start)*1000, 3),
                              "value": value})
        except Exception as exc:
            snapshots.append({"label": label, "error": str(exc)})

    try:
        session.send("Page.navigate", {"url": url})
    except Exception as exc:
        errors.append({"stage": "goto", "error": str(exc)})
    snapshot("after-navigation")

    deadline = time.monotonic() + wait_seconds
    consent_clicked = False
    consent_attempted = False
    shopping_seen_at = None
    while time.monotonic() < deadline:
        if not consent_attempted:
            try:
                button = page.locator("#onetrust-accept-btn-handler")
                if button.count() and button.is_visible():
                    consent_attempted = True
                    button.click(timeout=2500)
                    consent_clicked = True
            except Exception as exc:
                errors.append({"stage": "consent", "error": str(exc)})
        shopping = [r for r in requests.values() if classify(r.get("url", "")) == "shopping"
                    and "response" in r]
        if shopping and shopping_seen_at is None:
            shopping_seen_at = time.monotonic()
        if shopping_seen_at and time.monotonic() - shopping_seen_at >= 4:
            break
        page.wait_for_timeout(400)
    snapshot("after-shopping-or-timeout")
    try:
        cookies = context.cookies()
    except Exception as exc:
        cookies = {"error": str(exc)}

    bodies = {}
    for rid, rec in list(requests.items()):
        kind = classify(rec.get("url", ""))
        if kind not in ("shopping", "akamai", "protection-resource", "document", "script"):
            continue
        if "response" not in rec or rec.get("end", {}).get("method") != "Network.loadingFinished":
            continue
        try:
            data = session.send("Network.getResponseBody", {"requestId": rid})
            body = (base64.b64decode(data["body"]) if data.get("base64Encoded")
                    else data["body"].encode())
            bodies[rid] = {"bytes": len(body), "sha256": sha(body),
                           "base64": base64.b64encode(body).decode()}
        except Exception as exc:
            bodies[rid] = {"error": str(exc)}

    post_bodies = {}
    for rid, rec in list(requests.items()):
        if rec.get("method") != "POST" or classify(rec.get("url", "")) not in ("shopping", "akamai"):
            continue
        try:
            post_bodies[rid] = session.send("Network.getRequestPostData", {"requestId": rid})
        except Exception as exc:
            post_bodies[rid] = {"error": str(exc)}

    result = {"mode": mode, "url": url, "startedAt": start_wall,
              "capabilities": capability, "consentClicked": consent_clicked,
              "events": events, "requests": list(requests.values()),
              "playwrightRequests": playwright_requests,
              "snapshots": snapshots, "cookies": cookies, "bodies": bodies,
              "postBodies": post_bodies,
              "errors": errors}
    save_json(output / f"{mode}-raw.json", result)
    context.close()
    return result


def summarize(run: dict) -> dict:
    rows = []
    first_timestamp = min((r["protocolTimestamp"] for r in run["requests"]
                           if isinstance(r.get("protocolTimestamp"), (float, int))),
                          default=None)
    for rec in run["requests"]:
        req = rec.get("request", {})
        response = rec.get("response", {})
        url = rec.get("url", "")
        sent = rec.get("requestExtraInfo", {}).get("headers") or req.get("headers") or {}
        received = rec.get("responseExtraInfo", {}).get("headers") or response.get("headers") or {}
        queried_post_data = run.get("postBodies", {}).get(rec["requestId"], {}).get("postData")
        post_data = queried_post_data
        if post_data is None:
            post_data = req.get("postData")
        body = run["bodies"].get(rec["requestId"], {})
        started = rec.get("protocolTimestamp")
        finished = rec.get("end", {}).get("params", {}).get("timestamp")
        timing = rec.get("requestExtraInfo", {}).get("obscuraTiming", {})
        def elapsed(stage):
            value = timing.get(stage)
            return (round((value-started)*1000, 3)
                    if isinstance(started, (float, int)) and isinstance(value, (float, int))
                    and value >= started else None)
        duration = (round((finished-started)*1000, 3)
                    if isinstance(started, (float, int)) and isinstance(finished, (float, int))
                    and finished >= started else None)
        rows.append({"kind": classify(url), "url": url, "method": rec.get("method"),
                     "atMs": rec.get("atMs"), "responseAtMs": rec.get("responseAtMs"),
                     "requestDurationMs": duration,
                     "requestPreparedAfterMs": elapsed("requestPreparedAt"),
                     "responseHeadersAfterMs": elapsed("responseHeadersAt"),
                     "relativeProtocolMs": (round((rec["protocolTimestamp"]-first_timestamp)*1000, 3)
                                            if first_timestamp is not None and isinstance(rec.get("protocolTimestamp"), (float, int)) else None),
                     "status": response.get("status"), "protocol": response.get("protocol"),
                     "securityDetails": response.get("securityDetails"),
                     "initiator": rec.get("initiator"),
                     "requestHeaders": headers_summary(sent),
                     "responseHeaders": response_headers_summary(rec, received),
                     "requestBody": ({"bytes": len(post_data.encode()),
                                      "sha256": sha(post_data.encode())}
                                     if post_data is not None else None),
                     "requestBodyQuery": run.get("postBodies", {}).get(rec["requestId"], {}).get("error", "ok")
                         if rec["requestId"] in run.get("postBodies", {}) else None,
                     "requestBodyEventMatchesQuery": (queried_post_data == req["postData"])
                         if queried_post_data is not None and "postData" in req else None,
                     "responseBody": {k: v for k, v in body.items() if k != "base64"},
                     "end": rec.get("end", {}).get("method"),
                     "failure": rec.get("end", {}).get("params", {}).get("errorText")})
    rows.sort(key=lambda row: row["atMs"] if row["atMs"] is not None else float("inf"))
    observation_source = "cdp"
    if not rows and run.get("playwrightRequests"):
        observation_source = "playwright-fallback"
        for rec in run["playwrightRequests"]:
            post_data = rec.get("postData")
            rows.append({"kind": classify(rec["url"]), "url": rec["url"],
                         "method": rec["method"], "atMs": rec["atMs"],
                         "responseAtMs": rec.get("responseAtMs"), "status": rec.get("status"),
                         "protocol": None, "securityDetails": None, "initiator": None,
                         "requestHeaders": headers_summary(rec["headers"]),
                         "responseHeaders": headers_summary(rec.get("responseHeaders", {})),
                         "requestBody": ({"bytes": len(post_data.encode()),
                                          "sha256": sha(post_data.encode())}
                                         if post_data is not None else None),
                         "responseBody": None, "end": None, "failure": None})
    snapshots = []
    for item in run["snapshots"]:
        value = item.get("value", {})
        storage = value.get("localStorage", {})
        snapshots.append({"label": item["label"], "atMs": item.get("atMs"),
                          "url": value.get("url"), "readyState": value.get("readyState"),
                          "identity": value.get("identity"),
                          "storage": {k: {"bytes": len(str(v)), "sha256": sha(str(v).encode())}
                                      for k, v in storage.items()},
                          "documentCookieNames": cookie_names(value.get("cookie", "")),
                          "error": item.get("error")})
    return {"mode": run["mode"], "url": run["url"], "capabilities": run["capabilities"],
            "observationSource": observation_source,
            "playwrightRequestCount": len(run.get("playwrightRequests", [])),
            "consentClicked": run["consentClicked"], "requests": rows,
            "snapshots": snapshots, "cookieNamesFinal":
                sorted({c["name"] for c in run["cookies"]}) if isinstance(run["cookies"], list) else [],
            "scriptParsed": sum(e["method"] == "Debugger.scriptParsed" for e in run["events"]),
            "scriptExecutions": [e["params"] for e in run["events"]
                                 if e["method"] == "Obscura.scriptExecution"],
            "consoleCounts": dict(Counter(
                e["params"].get("type") for e in run["events"]
                if e["method"] == "Runtime.consoleAPICalled")),
            "exceptions": [e for e in run["events"] if e["method"] == "Runtime.exceptionThrown"],
            "storageEvents": [e for e in run["events"] if e["method"].startswith("DOMStorage.")],
            "storageMutations": [e["params"] for e in run["events"]
                                  if e["method"] == "Obscura.storageMutation"],
            "cookieWrites": [e["params"] for e in run["events"]
                             if e["method"] == "Obscura.cookieWrite"],
            "errors": run["errors"]}


def compare(chrome: dict, obscura: dict) -> dict:
    def key_events(run):
        return [{k: r.get(k) for k in ("kind", "method", "status", "atMs", "url")
                 if k != "status" or r["kind"] != "shopping"}
                for r in run["requests"] if r["kind"] in
                ("document", "akamai", "protection-resource", "shopping")]
    def shopping(run):
        return [r for r in run["requests"] if r["kind"] == "shopping"]
    c, o = shopping(chrome), shopping(obscura)
    return {"chromeShoppingRequests": [{"atMs": r.get("atMs"), "requestBody": r.get("requestBody")}
                                       for r in c],
            "obscuraShoppingRequests": [{"atMs": r.get("atMs"), "requestBody": r.get("requestBody")}
                                        for r in o],
            "chromeEvents": key_events(chrome), "obscuraEvents": key_events(obscura),
            "chromeSnapshots": chrome["snapshots"], "obscuraSnapshots": obscura["snapshots"],
            "chromeCookieNamesFinal": chrome["cookieNamesFinal"],
            "obscuraCookieNamesFinal": obscura["cookieNamesFinal"],
            "chromeScriptParsed": chrome["scriptParsed"],
            "obscuraScriptParsed": obscura["scriptParsed"],
            "chromeExceptions": chrome["exceptions"], "obscuraExceptions": obscura["exceptions"],
            "chromeStorageEvents": chrome["storageEvents"],
            "obscuraStorageEvents": obscura["storageEvents"],
            "limits": ["CDP headers are browser reports, not decrypted wire bytes.",
                       "Chrome NetLog is separate; Obscura HTTP/2 frames are not captured here.",
                       "Obscura Debugger.enable emits no script events; DOMStorage is unavailable.",
                       "Page snapshots are not lifecycle-aligned and do not prove each write time.",
                       "No in-page API wrappers or sensor decoding were installed."]}


def main() -> None:
    from playwright.sync_api import sync_playwright

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--url", default=DEFAULT_URL)
    parser.add_argument("--proxy", default="http://127.0.0.1:7890")
    parser.add_argument("--obscura-bin", type=Path, default=Path("target/release/obscura"))
    parser.add_argument("--chrome-bin", type=Path,
                        default=Path("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"))
    parser.add_argument("--output", type=Path)
    parser.add_argument("--wait-seconds", type=int, default=35)
    parser.add_argument("--block-trackers", action="store_true",
                        help="Enable Obscura's optional tracker blocklist for a privacy-policy comparison")
    args = parser.parse_args()
    output = args.output or Path(tempfile.mkdtemp(prefix="obscura-southwest-"))
    output = output.resolve()
    repository = Path(__file__).resolve().parents[2]
    if output.is_relative_to(repository):
        parser.error("--output must be outside the repository")
    if output.exists():
        if output.stat().st_mode & 0o077:
            parser.error("--output directory must have private permissions (0700)")
    else:
        output.mkdir(mode=0o700, parents=True)
    save_json(output / "inputs.json", {"url": args.url, "proxy": args.proxy,
                                       "obscuraBin": str(args.obscura_bin.resolve()),
                                       "obscuraSha256": sha(args.obscura_bin.read_bytes()),
                                       "chromeBin": str(args.chrome_bin),
                                       "waitSeconds": args.wait_seconds,
                                       "captureContext": CONTEXT_OPTIONS,
                                       "obscuraDiagnosticsEnabled": True,
                                       "obscuraBlockTrackers": args.block_trackers})
    persona_file = output / "persona.json"
    save_json(persona_file, {"schema_version": "1", "persona_id": "southwest-compare-mac153",
                             "revision": "1", "profile": "macos_chrome153",
                             "language": "en-US", "languages": ["en-US"],
                             "accept_language": "en-US",
                             "timezone": "Asia/Shanghai",
                             "viewport": {"width": 1365, "height": 768},
                             "screen_width": 1365, "screen_height": 768,
                             "screen_avail_width": 1365, "screen_avail_height": 768,
                             "outer_width": 1367, "outer_height": 848,
                             "screen_color_depth": 30})
    with sync_playwright() as playwright:
        chrome_port = free_port()
        chrome_command = [str(args.chrome_bin), f"--user-data-dir={output / 'chrome-profile'}",
                          f"--remote-debugging-port={chrome_port}",
                          f"--proxy-server={args.proxy}", "--no-first-run",
                          "--no-default-browser-check",
                          f"--log-net-log={output / 'chrome-netlog.json'}",
                          "--net-log-capture-mode=Everything", "about:blank"]
        chrome_stdout = (output / "chrome-stdout.bin").open("wb")
        chrome_stderr = (output / "chrome-stderr.bin").open("wb")
        chrome_process = subprocess.Popen(chrome_command, stdout=chrome_stdout,
                                          stderr=chrome_stderr)
        try:
            wait_endpoint(chrome_port, chrome_process)
            chrome = playwright.chromium.connect_over_cdp(f"http://127.0.0.1:{chrome_port}")
            try:
                chrome_run = collect(chrome, "chrome", args.url, output, args.wait_seconds)
            finally:
                chrome.close()
        finally:
            chrome_process.terminate()
            try:
                chrome_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                chrome_process.kill()
                chrome_process.wait(timeout=5)
            chrome_stdout.close()
            chrome_stderr.close()
        port = free_port()
        stdout = (output / "obscura-stdout.bin").open("wb")
        stderr = (output / "obscura-stderr.bin").open("wb")
        command = [str(args.obscura_bin.resolve()), "--persona", str(persona_file),
                   "--proxy", args.proxy, "serve", "--host", "127.0.0.1", "--port", str(port)]
        obscura_env = os.environ.copy()
        obscura_env["OBSCURA_BLOCK_TRACKERS"] = "1" if args.block_trackers else "0"
        obscura_env["OBSCURA_CDP_DIAGNOSTICS"] = "1"
        process = subprocess.Popen(command, stdout=stdout, stderr=stderr, env=obscura_env)
        try:
            wait_endpoint(port, process)
            browser = playwright.chromium.connect_over_cdp(f"http://127.0.0.1:{port}")
            try:
                obscura_run = collect(browser, "obscura", args.url, output, args.wait_seconds)
            finally:
                browser.close()
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=5)
            stdout.close()
            stderr.close()
        c, o = summarize(chrome_run), summarize(obscura_run)
        save_json(output / "chrome-summary.json", c)
        save_json(output / "obscura-summary.json", o)
        comparison = compare(c, o)
        save_json(output / "comparison.json", comparison)
        report = make_report(output)
        print(json.dumps({"output": str(output),
                          "report": str(report),
                          "chromeRequests": len(c["requests"]), "obscuraRequests": len(o["requests"])}, indent=2))


if __name__ == "__main__":
    main()

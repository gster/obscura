#!/usr/bin/env python3
"""Optional LOCAL fixtures only. Launches an owned temporary Chromium process.

Never treats fixture data as external site observations. Existing user browsers are
not touched. Requires --chromium pointing to a local Chromium executable.
"""
from __future__ import annotations
import argparse
import asyncio
from functools import partial
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import importlib.util
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import threading
import time
from types import SimpleNamespace
import urllib.request

SPEC = importlib.util.spec_from_file_location("audit_collect", Path(__file__).resolve().parents[1] / "scripts" / "collect.py")
assert SPEC and SPEC.loader
collector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(collector)

FP = {"status": "COLLECT_SUCCESSFUL", "fingerprintHash": "LOCAL_FIXTURE_HASH", "fingerprintId": "LOCAL_FIXTURE_ID",
      "fingerprintParts": {"fixture": "ONLY"},
      "signals": {"fixture": True, "sentinelStorage": None, "sentinelCookie": ""}, "botScore": 42, "reasons": []}
TLS = {"ip": "192.0.2.10:12345", "tls": {"fixture": True}, "http_version": "LOCAL_FIXTURE_NOT_TLS_MEASUREMENT"}


def rebrowser_html() -> str:
    # An independently authored minimal contract fixture, not a detector or a copy of the site.
    script = """
const rows = TYPES.map(type => ({type, rating: ['dummyFn','sourceUrlLeak','mainWorldExecution','exposeFunctionLeak'].includes(type) ? 0 : -1, note:'LOCAL FIXTURE ONLY'}));
function update(type, rating) { rows.find(r => r.type === type).rating = rating; render(); }
function render() { document.querySelector('#detections-json').value = JSON.stringify(rows); }
window.dummyFn = () => { update('dummyFn', -1); return true; };
const byId = document.getElementById.bind(document);
document.getElementById = (...args) => { update('sourceUrlLeak', -1); return byId(...args); };
const byClass = document.getElementsByClassName.bind(document);
document.getElementsByClassName = (...args) => { update('mainWorldExecution', 1); return byClass(...args); };
setInterval(() => { if (typeof window.exposedFn === 'function') update('exposeFunctionLeak', -1); }, 30);
render();
""".replace("TYPES", json.dumps(sorted(collector.REBROWSER_TYPES)))
    return '<!doctype html><title>LOCAL FIXTURE</title><h1>LOCAL FIXTURE ONLY</h1><textarea id="detections-json"></textarea><script>' + script + '</script>'


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass
    def do_GET(self):
        status, mime = 200, "text/html; charset=utf-8"
        if self.path == "/tls-peet":
            data, mime = json.dumps(TLS), "application/json"
        elif self.path == "/fingerprint-scan":
            data = '<title>LOCAL FIXTURE</title><body>LOCAL FIXTURE<script>setTimeout(() => { window.FINGERPRINT_SCAN = ' + json.dumps(FP) + '; window.FINGERPRINT_SCAN.signals.sentinelStorage = localStorage.getItem("audit-sentinel"); window.FINGERPRINT_SCAN.signals.sentinelCookie = document.cookie; }, 50);</script></body>'
        elif self.path == "/rebrowser":
            data = rebrowser_html()
        elif self.path == "/not-ready":
            data = '<body>LOCAL INCOMPLETE FIXTURE<script>window.FINGERPRINT_SCAN={status:"COLLECTING"};</script></body>'
        elif self.path == "/bad-schema":
            data = '<body>LOCAL BAD SCHEMA<script>window.FINGERPRINT_SCAN={status:"COLLECT_SUCCESSFUL"};</script></body>'
        elif self.path == "/not-json":
            data = '<body>LOCAL NON JSON FIXTURE</body>'
        elif self.path == "/blocked":
            status, data = 429, '<body>LOCAL RATE LIMIT FIXTURE</body>'
        elif self.path == "/sentinel":
            data = '<title>PRE-EXISTING FIXTURE TAB</title><body>Do not close this tab</body>'
        else:
            status, data = 404, "not found"
        body = data.encode()
        self.send_response(status)
        self.send_header("Content-Type", mime)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


async def run_all(base: str, endpoint: str, root: Path) -> dict:
    from playwright.async_api import async_playwright
    manifest = root / "fixture-manifest.json"
    manifest.write_text(json.dumps({"role": "baseline", "profile_dedicated": True, "fixture_only": True,
        "automation": {"transport": "cdp"}}))
    p = await async_playwright().start()
    browser = await p.chromium.connect_over_cdp(endpoint)
    sentinel = await browser.contexts[0].new_page()
    await sentinel.goto(base + "/sentinel")
    await sentinel.evaluate("() => { localStorage.setItem('audit-sentinel', 'existing-profile'); document.cookie='audit-sentinel=existing-profile; path=/'; }")
    expected_tabs = len(browser.contexts[0].pages)
    await p.stop()
    completed = []

    def args_for(name, mode="none", sites=None):
        return SimpleNamespace(cdp=endpoint, allow_remote_cdp=False, dedicated_profile=True,
            label="LOCAL-FIXTURE-ONLY", manifest=str(manifest), out=str(root / name),
            context_mode="fresh", context_index=0, cdp_context_defaults="auto", sites=sites or list(collector.SITES),
            rebrowser_actions=mode, snapshots=True, navigation_timeout=10, collection_timeout=5,
            action_timeout=3, observe_seconds=0.15)

    for mode in ("none", "main", "isolated"):
        collector.SITES.update({key: base + "/" + key for key in collector.SITES})
        args = args_for(mode, mode)
        rc = await collector.main_async(args)
        assert rc == 0, (mode, rc)
        result = json.loads((root / mode / "results.json").read_text())
        assert all(r["execution_status"] == "COMPLETE" for r in result["results"])
        assert all(r["comparison_verdict"] == "INCONCLUSIVE" for r in result["results"])
        fp = json.loads((root / mode / "fingerprint-scan" / "raw.json").read_text())
        assert fp == FP
        meta = json.loads((root / mode / "manifest.json").read_text())
        assert meta["context"]["owned"] and meta["owned_context_closed"]
        raw = json.loads((root / mode / "rebrowser" / "raw.json").read_text())
        main_row = next(r for r in raw if r["type"] == "mainWorldExecution")
        assert main_row["rating"] == (1 if mode == "main" else 0), (mode, main_row)
        detail = json.loads((root / mode / "rebrowser" / "result.json").read_text())
        if mode != "none":
            assert all(a["succeeded"] for a in detail["actions"]), detail["actions"]
        completed.append(f"positive-{mode}")

    for site, path, expected in (
        ("fingerprint-scan", "/not-ready", "TIMEOUT"),
        ("fingerprint-scan", "/bad-schema", "PARTIAL"),
        ("tls-peet", "/not-json", "PARTIAL"),
        ("tls-peet", "/blocked", "BLOCKED"),
    ):
        name = path.strip("/")
        collector.SITES[site] = base + path
        args = args_for(name, sites=[site]); args.collection_timeout = 0.7
        rc = await collector.main_async(args)
        assert rc == 2, (name, rc)
        detail = json.loads((root / name / site / "result.json").read_text())
        assert detail["execution_status"] == expected, detail
        if name == "not-ready":
            raw = json.loads((root / name / site / "raw.json").read_text())
            assert raw["status"] == "COLLECTING"
        completed.append(f"negative-{name}")

    p = await async_playwright().start()
    browser = await p.chromium.connect_over_cdp(endpoint)
    pages = browser.contexts[0].pages
    assert len(pages) == expected_tabs, (len(pages), expected_tabs)
    assert any(page.url == base + "/sentinel" for page in pages)
    sentinel = next(page for page in pages if page.url == base + "/sentinel")
    assert await sentinel.evaluate("() => localStorage.getItem('audit-sentinel')") == "existing-profile"
    assert "audit-sentinel=existing-profile" in await sentinel.evaluate("() => document.cookie")
    assert len(browser.contexts) == 1, "collector leaked an owned context"
    await p.stop()
    completed.append("existing-browser-and-tab-preserved")
    completed.append("fresh-baseline-storage-isolation-and-cleanup")
    return {"fixture_only": True, "cases_passed": completed,
            "external_sites_executed": False, "candidate_browser_executed": False}


async def run_dom_contracts(endpoint: str) -> dict:
    """No page navigation/network: real Chromium DOM/CDP adapter contract tests."""
    from playwright.async_api import async_playwright
    p = await async_playwright().start()
    browser = await p.chromium.connect_over_cdp(endpoint)
    context = browser.contexts[0]
    sentinel = await context.new_page()
    await sentinel.set_content("<title>OWNED-DOM-SENTINEL</title><body>fixture</body>")
    expected_tabs = len(context.pages)
    passed = []
    try:
        page = await context.new_page()
        await page.set_content("<body>LOCAL DOM FIXTURE<script>window.FINGERPRINT_SCAN=" + json.dumps(FP) + ";</script></body>")
        result = {"actions": []}
        args = SimpleNamespace(collection_timeout=3, action_timeout=2, observe_seconds=0.15)
        await collector.collect_fingerprint(page, result, args)
        assert result["raw"] == FP
        await page.close()
        passed.append("fingerprint-public-object-real-dom")
        for mode in ("none", "main", "isolated"):
            page = await context.new_page()
            await page.set_content(rebrowser_html())
            result = {"actions": []}
            args.rebrowser_actions = mode
            await asyncio.wait_for(collector.collect_rebrowser(page, result, args), timeout=5)
            assert collector.validate_data("rebrowser", result["raw"]) == []
            item = next(r for r in result["raw"] if r["type"] == "mainWorldExecution")
            assert item["rating"] == (1 if mode == "main" else 0)
            if mode != "none":
                assert len(result["actions"]) == 4
                assert all(a["succeeded"] for a in result["actions"]), result["actions"]
            await page.close()
            passed.append("rebrowser-" + mode + "-real-dom-cdp")
        assert len(context.pages) == expected_tabs
    finally:
        await p.stop()
    p = await async_playwright().start()
    try:
        browser = await p.chromium.connect_over_cdp(endpoint)
        assert len(browser.contexts[0].pages) == expected_tabs
        assert "OWNED-DOM-SENTINEL" in [await tab.title() for tab in browser.contexts[0].pages]
        passed.append("client-disconnect-preserves-existing-browser-and-tab")
    finally:
        await p.stop()
    return {"fixture_only": True, "mode": "dom-only-real-chromium-cdp",
            "cases_passed": passed, "external_sites_executed": False,
            "candidate_browser_executed": False, "http_navigation_pipeline_tested": False,
            "tls_on_wire_tested": False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--chromium", required=True)
    parser.add_argument("--no-sandbox", action="store_true", help="Only for a dedicated root container fixture run; not a production recommendation")
    parser.add_argument("--dom-only", action="store_true", help="Test real DOM/CDP adapter contracts without HTTP navigation; does not test TLS/HTTP pipeline")
    parser.add_argument("--report", help="Optional path for non-sensitive test summary")
    args = parser.parse_args()
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
    with tempfile.TemporaryDirectory(prefix="fingerprint-audit-fixture-") as tmp:
        root = Path(tmp)
        port = free_port()
        endpoint = f"http://127.0.0.1:{port}"
        command = [args.chromium, "--headless=new", f"--remote-debugging-port={port}",
            f"--user-data-dir={root / 'owned-profile'}", "--no-first-run", "--no-default-browser-check",
            "--disable-background-networking", "about:blank"]
        if args.no_sandbox:
            command.insert(1, "--no-sandbox")
        process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            for _ in range(100):
                if process.poll() is not None:
                    raise RuntimeError("Owned fixture Chromium exited before its CDP endpoint became ready")
                try:
                    with urllib.request.urlopen(endpoint + "/json/version", timeout=0.5) as response:
                        version = json.loads(response.read())
                    break
                except Exception:
                    time.sleep(0.1)
            else:
                raise RuntimeError("Owned fixture CDP endpoint did not become ready")
            report = asyncio.run(run_dom_contracts(endpoint) if args.dom_only else run_all(f"http://127.0.0.1:{server.server_port}", endpoint, root))
            assert process.poll() is None, "Collector unexpectedly terminated the external browser"
            report["local_chromium_version"] = version.get("Browser")
            report["playwright_version"] = collector.importlib.metadata.version("playwright")
            report["validated_at"] = collector.utc_now()
            if args.report:
                Path(args.report).write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
            print(json.dumps(report, ensure_ascii=False, indent=2))
        finally:
            # This fixture harness owns this process; unlike collect.py it may terminate it.
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait()
            server.shutdown(); server.server_close()


if __name__ == "__main__":
    main()

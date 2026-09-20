#!/usr/bin/env python3
"""Official Playwright migration probe for dynamic classic-script CORS."""

from __future__ import annotations

import base64
import json
import threading
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Iterator
from urllib.parse import parse_qs, urlsplit


CASE_NAMES = ("default", "anonymous", "credentials", "denied", "base")


class _WireReader:
    """Record the bytes consumed by BaseHTTPRequestHandler's parser."""

    def __init__(self, stream: Any):
        self._stream = stream
        self._header_lines: list[bytes] = []
        self._request_done = False

    def readline(self, *args: Any) -> bytes:
        if self._request_done:
            self._header_lines.clear()
            self._request_done = False
        line = self._stream.readline(*args)
        self._header_lines.append(line)
        return line

    @property
    def header_bytes(self) -> bytes:
        return b"".join(self._header_lines)

    def finish_request(self) -> None:
        self._request_done = True

    def __getattr__(self, name: str) -> Any:
        return getattr(self._stream, name)


class ScriptHandler(BaseHTTPRequestHandler):
    """Serve the two-origin document/scripts and retain every request byte."""

    protocol_version = "HTTP/1.1"
    server_version = "ObscuraCorsFixture/1"
    sys_version = ""

    def setup(self) -> None:
        super().setup()
        self.rfile = _WireReader(self.rfile)

    def log_message(self, format: str, *args: Any) -> None:
        return

    def _record_request(self) -> dict[str, Any]:
        content_length = int(self.headers.get("Content-Length", "0") or "0")
        body = self.rfile.read(content_length)
        wire = self.rfile
        header_bytes = wire.header_bytes
        record: dict[str, Any] = {
            "serverOrigin": self.server.origin,  # type: ignore[attr-defined]
            "method": self.command,
            "path": self.path,
            "headers": [
                {"name": name, "value": value}
                for name, value in self.headers.raw_items()
            ],
            "rawHeadersBase64": base64.b64encode(header_bytes).decode("ascii"),
            "bodyBase64": base64.b64encode(body).decode("ascii"),
        }
        self.server.request_records.append(record)  # type: ignore[attr-defined]
        # The external runner captures stdout as a lossless process log. Keep
        # this record independent of the final observations JSON so a deadline
        # cannot discard a request that was already received by this server.
        print(json.dumps(record, separators=(",", ":"), ensure_ascii=True), flush=True)
        wire.finish_request()
        return record

    def do_GET(self) -> None:
        self._record_request()
        parsed = urlsplit(self.path)
        query = parse_qs(parsed.query)
        if parsed.path == "/script.js":
            name = query["case"][0]
            body = (
                "window.actualExecution = true;"
                "window.received = "
                + json.dumps(
                    {
                        "cookie": "script_session=fixture"
                        in self.headers.get("Cookie", ""),
                        "origin": self.headers.get("Origin"),
                        "destination": self.headers.get("Sec-Fetch-Dest"),
                    }
                )
                + ";"
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/javascript")
            origin = self.headers.get("Origin")
            if name != "denied" and origin:
                self.send_header("Access-Control-Allow-Origin", origin)
                self.send_header("Access-Control-Allow-Credentials", "true")
        else:
            target = self.server.script_origin  # type: ignore[attr-defined]
            body = (
                """<!doctype html><html><head></head><body>
<pre id=result>pending</pre>
<script>
(async () => {
  document.cookie = 'script_session=fixture; Path=/';
  const results = [];
  for (const [name, attr] of [['default', null], ['anonymous', 'anonymous'],
                              ['credentials', 'use-credentials'],
                              ['denied', 'anonymous'], ['base', 'anonymous']]) {
    if (name === 'base') {
      const base = document.createElement('base');
      base.href = TARGET + '/';
      document.head.appendChild(base);
    }
    window.received = null;
    window.actualExecution = false;
    const script = document.createElement('script');
    if (attr !== null) script.setAttribute('crossorigin', attr);
    script.src = name === 'base'
      ? 'script.js?case=base'
      : TARGET + '/script.js?case=' + name;
    const event = await new Promise(resolve => {
      script.onload = () => resolve('load');
      script.onerror = () => resolve('error');
      document.head.appendChild(script);
    });
    results.push({
      name,
      event,
      data: window.received,
      executed: window.actualExecution === true,
    });
  }
  document.querySelector('#result').textContent = JSON.stringify(results);
})().catch(error => {
  document.querySelector('#result').textContent = 'ERROR:' + error.message;
});
</script></body></html>""".replace("TARGET", json.dumps(target))
            ).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(body)


class ScriptServers:
    """Own the document and script HTTP origins without owning a browser."""

    def __enter__(self) -> "ScriptServers":
        self.servers = [
            ThreadingHTTPServer(("127.0.0.1", 0), ScriptHandler)
            for _ in range(2)
        ]
        self.origins = [
            f"http://127.0.0.1:{server.server_port}" for server in self.servers
        ]
        self.threads: list[threading.Thread] = []
        for server, origin in zip(self.servers, self.origins):
            server.origin = origin  # type: ignore[attr-defined]
            server.script_origin = self.origins[1]  # type: ignore[attr-defined]
            server.request_records = []  # type: ignore[attr-defined]
            server.daemon_threads = True
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            self.threads.append(thread)
        return self

    def __exit__(self, *args: Any) -> None:
        for server in self.servers:
            server.shutdown()
            server.server_close()
        for thread in self.threads:
            thread.join(timeout=2)

    @property
    def requests(self) -> list[dict[str, Any]]:
        return [
            record
            for server in self.servers
            for record in server.request_records  # type: ignore[attr-defined]
        ]


@contextmanager
def local_http_servers() -> Iterator[ScriptServers]:
    servers = ScriptServers()
    with servers:
        yield servers


def _expected_results(origins: list[str]) -> list[dict[str, Any]]:
    document_origin = origins[0]
    return [
        {
            "name": "default",
            "event": "load",
            "data": {"cookie": True, "origin": None, "destination": "script"},
            "executed": True,
        },
        {
            "name": "anonymous",
            "event": "load",
            "data": {
                "cookie": False,
                "origin": document_origin,
                "destination": "script",
            },
            "executed": True,
        },
        {
            "name": "credentials",
            "event": "load",
            "data": {
                "cookie": True,
                "origin": document_origin,
                "destination": "script",
            },
            "executed": True,
        },
        {
            "name": "denied",
            "event": "error",
            "data": None,
            "executed": False,
        },
        {
            "name": "base",
            "event": "load",
            "data": {
                "cookie": False,
                "origin": document_origin,
                "destination": "script",
            },
            "executed": True,
        },
    ]


def _header_values(record: dict[str, Any], name: str) -> list[str]:
    folded = name.lower()
    return [
        item["value"]
        for item in record["headers"]
        if item["name"].lower() == folded
    ]


async def run_case(endpoint: str, observations: dict[str, Any]) -> None:
    """Run the two-origin dynamic-script matrix through an existing CDP browser.

    The caller owns the browser process and its hard deadline. This function
    owns only the two local HTTP servers and the Playwright CDP connection.
    Every request is printed as a complete JSON record and also retained in
    observations, including ordered duplicate headers and raw body bytes.
    """
    from playwright.async_api import async_playwright

    observations["endpoint"] = endpoint
    observations["status"] = "running"
    with local_http_servers() as servers:
        observations["origins"] = list(servers.origins)
        try:
            async with async_playwright() as playwright:
                browser = await playwright.chromium.connect_over_cdp(endpoint)
                page = None
                try:
                    context = browser.contexts[0] if browser.contexts else await browser.new_context()
                    page = await context.new_page()
                    await page.goto(servers.origins[0], wait_until="load")
                    await page.wait_for_function(
                        "document.querySelector('#result')?.textContent !== 'pending'",
                        timeout=5000,
                    )
                    raw_result = await page.locator("#result").text_content()
                    if raw_result is None or raw_result.startswith("ERROR:"):
                        raise AssertionError(f"dynamic script fixture failed: {raw_result!r}")
                    results = json.loads(raw_result)
                    expected = _expected_results(servers.origins)
                    if results != expected:
                        raise AssertionError(
                            f"dynamic script matrix differs: expected={expected!r} actual={results!r}"
                        )
                    observations["cases"] = results
                    observations["actualExecution"] = {
                        item["name"]: item["executed"] for item in results
                    }

                    script_requests = [
                        record
                        for record in servers.requests
                        if urlsplit(record["path"]).path == "/script.js"
                    ]
                    observations["requests"] = list(servers.requests)
                    observations["scriptRequests"] = script_requests
                    by_case: dict[str, list[dict[str, Any]]] = {
                        name: [] for name in CASE_NAMES
                    }
                    for record in script_requests:
                        query = parse_qs(urlsplit(record["path"]).query)
                        case = query.get("case", [None])[0]
                        if case in by_case:
                            by_case[case].append(record)
                    observations["scriptRequestsByCase"] = by_case
                    if any(len(records) != 1 for records in by_case.values()):
                        raise AssertionError(
                            f"expected one script request per case: {by_case!r}"
                        )
                    expected_headers = {
                        "default": {"origin": [], "sec-fetch-dest": ["script"]},
                        "anonymous": {
                            "origin": [servers.origins[0]],
                            "sec-fetch-dest": ["script"],
                        },
                        "credentials": {
                            "origin": [servers.origins[0]],
                            "sec-fetch-dest": ["script"],
                        },
                        "denied": {
                            "origin": [servers.origins[0]],
                            "sec-fetch-dest": ["script"],
                        },
                        "base": {
                            "origin": [servers.origins[0]],
                            "sec-fetch-dest": ["script"],
                        },
                    }
                    for name, records in by_case.items():
                        record = records[0]
                        actual_headers = {
                            "origin": _header_values(record, "Origin"),
                            "sec-fetch-dest": _header_values(record, "Sec-Fetch-Dest"),
                        }
                        if actual_headers != expected_headers[name]:
                            raise AssertionError(
                                f"{name} request headers differ: "
                                f"expected={expected_headers[name]!r} actual={actual_headers!r}"
                            )
                        if name in {"default", "credentials"}:
                            cookie = _header_values(record, "Cookie")
                            if not any("script_session=fixture" in value for value in cookie):
                                raise AssertionError(
                                    f"{name} request did not carry the fixture cookie: {record!r}"
                                )
                        else:
                            cookie = _header_values(record, "Cookie")
                            if any("script_session=fixture" in value for value in cookie):
                                raise AssertionError(
                                    f"{name} request unexpectedly carried the fixture cookie: {record!r}"
                                )
                    observations["status"] = "passed"
                finally:
                    if page is not None:
                        await page.close()
                    await browser.close()
        except BaseException as error:
            observations["status"] = "failed"
            observations["error"] = {
                "type": type(error).__name__,
                "message": str(error),
            }
            raise
        finally:
            observations["requests"] = list(servers.requests)
            observations.setdefault("scriptRequests", [
                record
                for record in servers.requests
                if urlsplit(record["path"]).path == "/script.js"
            ])

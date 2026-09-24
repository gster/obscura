#!/usr/bin/env python3
"""Turn southwest_compare raw captures into a compact, secret-safe phase report."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
from collections import Counter
from pathlib import Path
from urllib.parse import urlparse


def load(path: Path):
    return json.loads(path.read_text())


def first(run, kind, method=None):
    return next((r for r in run["requests"] if r["kind"] == kind
                 and (method is None or r["method"] == method)), None)


def fmt_time(row):
    value = row.get("relativeProtocolMs") if row else None
    return f"{value / 1000:.3f}s" if value is not None else "missing"


def fmt_status(row):
    return str(row.get("status")) if row else "missing"


def request_json(raw, fragment):
    for rec in raw["requests"]:
        if fragment in rec.get("url", "") and rec.get("method") == "POST":
            try:
                return json.loads(rec.get("request", {}).get("postData", ""))
            except (TypeError, ValueError):
                return None
    return None


def json_structure_differences(left, right):
    """Report schema, type, and empty-value gaps without exposing payload values."""
    differences = set()
    def walk(a, b, path):
        if type(a) is not type(b):
            differences.add(f"{path}: {type(a).__name__} / {type(b).__name__}")
        elif isinstance(a, dict):
            for key in a.keys() | b.keys():
                child = f"{path}/{key}"
                if key not in a or key not in b:
                    differences.add(f"{child}: {'Chrome' if key in a else 'Obscura'} only")
                else:
                    walk(a[key], b[key], child)
        elif isinstance(a, list):
            if len(a) != len(b):
                differences.add(f"{path}: array count {len(a)} / {len(b)}")
            for x, y in zip(a, b):
                walk(x, y, f"{path}[]")
        elif isinstance(a, str) and bool(a) != bool(b):
            differences.add(f"{path}: {'Chrome' if a else 'Obscura'} nonempty")
    walk(left, right, "")
    return sorted(differences)


def netlog_h2(path):
    if not path.exists():
        return []
    data = load(path)
    names = {v: k for k, v in data["constants"]["logEventTypes"].items()}
    events = data["events"]
    sessions = []
    for event in events:
        if names.get(event["type"]) != "HTTP2_SESSION":
            continue
        params = event.get("params", {})
        if params.get("host") != "www.southwest.com:443":
            continue
        sid = event["source"]["id"]
        own = [e for e in events if e["source"]["id"] == sid]
        settings = next((e.get("params", {}).get("settings") for e in own
                         if names.get(e["type"]) == "HTTP2_SESSION_SEND_SETTINGS"), None)
        windows = [e.get("params", {}).get("delta") for e in own
                   if names.get(e["type"]) == "HTTP2_SESSION_SEND_WINDOW_UPDATE"]
        headers = [e.get("params", {}).get("headers", []) for e in own
                   if names.get(e["type"]) == "HTTP2_SESSION_SEND_HEADERS"]
        setting_parts = []
        for setting in settings or []:
            match = re.search(r"id:(\d+).*value:(\d+)", setting)
            if match:
                setting_parts.append(f"{match.group(1)}:{match.group(2)}")
        pseudo = []
        if headers:
            for item in headers[0]:
                if item.startswith(":"):
                    key = item.split(":", 2)[1]
                    code = {"method": "m", "authority": "a", "scheme": "s", "path": "p"}.get(key)
                    if code:
                        pseudo.append(code)
        akamai_text = (f"{';'.join(setting_parts)}|{windows[0]}|0|{','.join(pseudo)}"
                       if setting_parts and windows and pseudo else None)
        sessions.append({"session": sid, "proxy": params.get("proxy"),
                         "settings": settings, "windowUpdates": windows,
                         "akamaiText": akamai_text,
                         "shopping": any(any("/booking/shopping" in h for h in frame)
                                         for frame in headers),
                         "headerFrames": len(headers)})
    return sessions


def tls_shape(record_base64):
    record = base64.b64decode(record_base64)
    if len(record) < 5 or record[0] != 22:
        return {"error": "not a TLS handshake record"}
    data = record[5:]
    if not data or data[0] != 1:
        return {"error": "not a ClientHello"}
    pos = 4 + 2 + 32
    pos += 1 + data[pos]
    size = int.from_bytes(data[pos:pos+2], "big")
    pos += 2
    ciphers = [int.from_bytes(data[i:i+2], "big") for i in range(pos, pos+size, 2)]
    pos += size
    pos += 1 + data[pos]
    size = int.from_bytes(data[pos:pos+2], "big")
    pos += 2
    end = pos + size
    extensions = []
    lengths = {}
    payload_hashes = {}
    groups = []
    signature = []
    alpn = []
    alps = []
    trust_anchor_ids = None
    while pos < end:
        key = int.from_bytes(data[pos:pos+2], "big")
        ext_length = int.from_bytes(data[pos+2:pos+4], "big")
        value = data[pos+4:pos+4+ext_length]
        extensions.append(key)
        lengths[f"{key:04x}"] = ext_length
        if key in (0x44cd, 0xca34):
            payload_hashes[f"{key:04x}"] = hashlib.sha256(value).hexdigest()
        if key in (10, 13):
            count = int.from_bytes(value[:2], "big")
            values = [int.from_bytes(value[i:i+2], "big")
                      for i in range(2, 2+count, 2)]
            if key == 10:
                groups = values
            else:
                signature = values
        if key == 16:
            count = int.from_bytes(value[:2], "big")
            offset = 2
            while offset < 2 + count:
                name_length = value[offset]
                alpn.append(value[offset+1:offset+1+name_length].decode("ascii", "replace"))
                offset += 1 + name_length
        if key == 0x44cd and len(value) >= 2:
            count = int.from_bytes(value[:2], "big")
            offset = 2
            while offset < min(len(value), 2 + count):
                name_length = value[offset]
                alps.append(value[offset+1:offset+1+name_length].decode("utf-8", "replace"))
                offset += 1 + name_length
        if key == 0xca34 and len(value) >= 2:
            count = int.from_bytes(value[:2], "big")
            offset = 2
            ids = []
            while offset < len(value) and offset < count + 2:
                id_length = value[offset]
                offset += 1
                if offset + id_length > len(value):
                    ids = []
                    break
                ids.append(value[offset:offset+id_length].hex())
                offset += id_length
            if offset == len(value) == count + 2:
                trust_anchor_ids = ids
        pos += 4 + ext_length
    def grease(value):
        return value & 0x0f0f == 0x0a0a and value >> 8 == value & 255
    ciphers = [v for v in ciphers if not grease(v)]
    extensions = [v for v in extensions if not grease(v)]
    groups = [v for v in groups if not grease(v)]
    signature = [v for v in signature if not grease(v)]
    ja3 = "771," + "-".join(map(str, ciphers)) + "," + "-".join(map(str, extensions)) + "," + "-".join(map(str, groups)) + ","
    return {"recordBytes": len(record),
            "handshakeSha256": hashlib.sha256(data).hexdigest(),
            "ja3": hashlib.md5(ja3.encode()).hexdigest(),
            "cipherSuites": [f"{v:04x}" for v in ciphers],
            "extensions": [f"{v:04x}" for v in extensions],
            "extensionLengths": lengths,
            "extensionPayloadSha256": payload_hashes,
            "trustAnchorIds": trust_anchor_ids,
            "signatureAlgorithms": [f"{v:04x}" for v in signature],
            "groups": [f"{v:04x}" for v in groups], "alpn": alpn, "alps": alps,
            "hasPsk": 41 in extensions}


def chrome_hello_hashes(netlog_path):
    if not netlog_path.exists():
        return {}, set()
    data = load(netlog_path)
    names = {v: k for k, v in data["constants"]["logEventTypes"].items()}
    hashes_by_socket = {e["source"]["id"]: hashlib.sha256(base64.b64decode(e["params"]["bytes"])).hexdigest()
                        for e in data["events"]
                        if names.get(e["type"]) == "SSL_HANDSHAKE_MESSAGE_SENT"
                        and e.get("params", {}).get("type") == 1}
    roles = {}
    for event in data["events"]:
        if names.get(event["type"]) != "HTTP2_SESSION" or event.get("params", {}).get("host") != "www.southwest.com:443":
            continue
        sid = event["source"]["id"]
        own = [e for e in data["events"] if e["source"]["id"] == sid]
        init = next((e for e in own if names.get(e["type"]) == "HTTP2_SESSION_INITIALIZED"), None)
        if not init:
            continue
        socket_id = init["params"]["source_dependency"]["id"]
        digest = hashes_by_socket.get(socket_id)
        if not digest:
            continue
        frames = [e.get("params", {}).get("headers", []) for e in own
                  if names.get(e["type"]) == "HTTP2_SESSION_SEND_HEADERS"]
        roles[digest] = "shopping" if any(any("/booking/shopping" in h for h in frame) for frame in frames) else "document"
    return roles, set(hashes_by_socket.values())


def tap_shapes(path, netlog_path):
    if not path or not path.exists():
        return []
    roles, known_chrome = chrome_hello_hashes(netlog_path)
    shapes = []
    for row in (json.loads(line) for line in path.read_text().splitlines()):
        shape = tls_shape(row["recordBase64"])
        shape.update(atUnix=row["atUnix"], target=row["target"],
                     recordSha256=row["recordSha256"],
                     client="Chrome" if shape.get("handshakeSha256") in known_chrome else "Obscura",
                     netlogSession=roles.get(shape.get("handshakeSha256")))
        shapes.append(shape)
    return shapes


def make_report(directory, tap=None, timings=None):
    chrome, obscura = (load(directory / f"{mode}-summary.json")
                       for mode in ("chrome", "obscura"))
    c_raw, o_raw = (load(directory / f"{mode}-raw.json")
                    for mode in ("chrome", "obscura"))
    lines = ["# Southwest direct-page comparison", "",
             "Raw CDP, bodies, NetLog and proxy tap contain session material. Keep this directory private.", "",
             "## Capture and request observation", ""]
    for mode, run in (("Chrome", chrome), ("Obscura", obscura)):
        lines.append(f"- {mode}: {len(run['requests'])} CDP request records; "
                     f"{run['playwrightRequestCount']} Playwright request events.")
    lines.append("Shopping response status and business content are excluded from comparison: the endpoint may degrade with access frequency or IP. Its outgoing request is still inspected when present.")
    lines.extend(["", "## Browser identity snapshot", "", "| Surface | Chrome | Obscura |", "| --- | --- | --- |"])
    c_identity = chrome["snapshots"][-1].get("identity") or {}
    o_identity = obscura["snapshots"][-1].get("identity") or {}
    for key in ("userAgent", "language", "languages", "platform", "timezone",
                "viewport", "screen", "outer", "devicePixelRatio"):
        lines.append(f"| {key} | `{json.dumps(c_identity.get(key), ensure_ascii=False)}` | "
                     f"`{json.dumps(o_identity.get(key), ensure_ascii=False)}` |")
    lines.extend(["", "## First navigation to shopping timeline", "",
                  "Times use each engine's CDP request timestamp, relative to its first observed request. Obscura traces JS fetch/XHR start time even when it delivers requestWillBeSent after completion; native navigation/resource events still use completion time. Durations below are meaningful for traced JS requests and include browser scheduling, not just transport RTT.", "",
                  "| Stage | Chrome | Obscura |", "| --- | --- | --- |"])
    for label, kind, method in (("document", "document", "GET"),
                                ("protection bundle", "protection-resource", "GET"),
                                ("Akamai script", "akamai", "GET"),
                                ("shopping", "shopping", "POST"),
                                ("Akamai pixel", "akamai", "POST")):
        a, b = first(chrome, kind, method), first(obscura, kind, method)
        if kind == "shopping":
            lines.append(f"| {label} | {fmt_time(a)} | {fmt_time(b)} |")
        else:
            lines.append(f"| {label} | {fmt_time(a)} / {fmt_status(a)} | {fmt_time(b)} / {fmt_status(b)} |")
    lines.extend(["", "### Request lifecycle durations", "", "| Request | Chrome | Obscura |", "| --- | --- | --- |"])
    for label, fragment in (("shopping", "/booking/shopping"),
                            ("Adobe delivery", "soptimize.southwest.com/rest/v1/delivery"),
                            ("Akamai pixel", "/akam/")):
        def matching(run):
            return next((row for row in run["requests"] if fragment in row["url"] and row["method"] == "POST"), None)
        def duration(row):
            value = row.get("requestDurationMs") if row else None
            return f"{value:.0f} ms / {fmt_status(row)}" if value is not None else f"unavailable / {fmt_status(row)}"
        if label == "shopping":
            def shopping_duration(row):
                value = row.get("requestDurationMs") if row else None
                return f"{value:.0f} ms" if value is not None else "unavailable"
            lines.append(f"| {label} | {shopping_duration(matching(chrome))} | {shopping_duration(matching(obscura))} |")
        else:
            lines.append(f"| {label} | {duration(matching(chrome))} | {duration(matching(obscura))} |")
    lines.extend(["", "Obscura transport preparation is recorded just before handing a prepared request to primp. It does not prove the bytes were sent on the wire.", "",
                  "| Obscura request | Prepared after start | Response headers after start |", "| --- | --- | --- |"])
    for label, fragment in (("shopping", "/booking/shopping"),
                            ("Adobe delivery", "soptimize.southwest.com/rest/v1/delivery"),
                            ("Akamai pixel", "/akam/")):
        row = next((r for r in obscura["requests"] if fragment in r["url"] and r["method"] == "POST"), None)
        def elapsed(key):
            value = row.get(key) if row else None
            return f"{value:.0f} ms" if value is not None else "unavailable"
        lines.append(f"| {label} | {elapsed('requestPreparedAfterMs')} | {elapsed('responseHeadersAfterMs')} |")
    lines.extend(["", "## Request inputs and Akamai markers", ""])
    for label, kind, method in (("Akamai script", "akamai", "GET"),
                                ("Akamai pixel", "akamai", "POST"),
                                ("shopping", "shopping", "POST")):
        lines.append(f"### {label}")
        lines.append("")
        for mode, run in (("Chrome", chrome), ("Obscura", obscura)):
            row = first(run, kind, method)
            if not row:
                lines.append(f"- {mode}: missing")
                continue
            headers = row["requestHeaders"]
            cookie = headers.get("cookie", {})
            markers = {key: value.get("bytes") for key, value in headers.items()
                       if key.startswith("ee30") and isinstance(value, dict)}
            body = row.get("requestBody")
            lines.append(f"- {mode}: Cookie {cookie.get('bytes', 0)} bytes, names "
                         f"`{cookie.get('names', [])}`; ee30 lengths `{markers}`; "
                         f"body `{body}`.")
        lines.append("")
    lines.extend(["### Header-name differences by stage", "",
                  "CDP header maps are browser reports, not decrypted wire evidence.", "",
                  "| Stage | Request Chrome only | Request Obscura only | Response Chrome only | Response Obscura only |",
                  "| --- | --- | --- | --- | --- |"])
    for label, kind, method in (("document", "document", "GET"),
                                ("protection bundle", "protection-resource", "GET"),
                                ("Akamai script", "akamai", "GET"),
                                ("Akamai pixel", "akamai", "POST"),
                                ("shopping", "shopping", "POST")):
        ca, oa = first(chrome, kind, method), first(obscura, kind, method)
        if not ca or not oa:
            continue
        c_req, o_req = set(ca["requestHeaders"]), set(oa["requestHeaders"])
        c_res, o_res = set(ca["responseHeaders"]), set(oa["responseHeaders"])
        lines.append(f"| {label} | `{sorted(c_req-o_req)}` | `{sorted(o_req-c_req)}` | "
                     f"`{sorted(c_res-o_res)}` | `{sorted(o_res-c_res)}` |")
    lines.extend(["", "### Sec-Fetch-Site values", "", "| Stage | Chrome | Obscura |", "| --- | --- | --- |"])
    for label, kind, method in (("document", "document", "GET"),
                                ("Akamai script", "akamai", "GET"),
                                ("Akamai pixel", "akamai", "POST"),
                                ("shopping", "shopping", "POST")):
        a, b = first(chrome, kind, method), first(obscura, kind, method)
        lines.append(f"| {label} | `{(a or {}).get('requestHeaders', {}).get('sec-fetch-site')}` | "
                     f"`{(b or {}).get('requestHeaders', {}).get('sec-fetch-site')}` |")
    for label, fragment in (("Adobe delivery", "soptimize.southwest.com/rest/v1/delivery"),):
        a = next((row for row in chrome["requests"] if fragment in row["url"] and row["method"] == "POST"), None)
        b = next((row for row in obscura["requests"] if fragment in row["url"] and row["method"] == "POST"), None)
        lines.append(f"| {label} | `{(a or {}).get('requestHeaders', {}).get('sec-fetch-site')}` | "
                     f"`{(b or {}).get('requestHeaders', {}).get('sec-fetch-site')}` |")
    lines.append("")
    for mode, run in (("Chrome", chrome), ("Obscura", obscura)):
        queries = [(kind, first(run, kind, "POST")) for kind in ("akamai", "shopping")]
        lines.append(f"- {mode} `Network.getRequestPostData`: " + ", ".join(
            f"{kind} {row.get('requestBodyQuery', 'unavailable')} "
            f"(event match {row.get('requestBodyEventMatchesQuery')})"
            for kind, row in queries if row))
    lines.extend(["", "## Script content and execution visibility", ""])
    for kind in ("protection-resource", "akamai", "script"):
        left = {urlparse(r["url"]).path: r["responseBody"] for r in chrome["requests"]
                if r["kind"] == kind and r["method"] == "GET"}
        right = {urlparse(r["url"]).path: r["responseBody"] for r in obscura["requests"]
                 if r["kind"] == kind and r["method"] == "GET"}
        common = set(left) & set(right)
        available = [k for k in common if left[k] and right[k]
                     and left[k].get("sha256") and right[k].get("sha256")]
        mismatched = sorted(k for k in available if left[k]["sha256"] != right[k]["sha256"])
        matched = len(available) - len(mismatched)
        lines.append(f"- {kind}: {matched}/{len(available)} available common response body SHA-256 matches; "
                     f"{len(common)-len(available)} common bodies unavailable; "
                     f"Chrome only {len(set(left)-set(right))}, Obscura only {len(set(right)-set(left))}; "
                     f"mismatched paths `{mismatched}`.")
    lines.append(f"- `Debugger.scriptParsed` events: Chrome {chrome['scriptParsed']}, Obscura {obscura['scriptParsed']}; "
                 "Obscura's Debugger initializer has no script event implementation. "
                 "This is an observation gap, not evidence that scripts did not execute.")
    executions = obscura.get("scriptExecutions", [])
    lines.append(f"- Obscura native classic-script executions: {len(executions)}; outcomes "
                 f"`{dict(Counter(event.get('outcome') for event in executions))}`. "
                 "These record execution results, while Chrome scriptParsed records compilation; their counts are not equivalent.")
    important = [event for event in executions if "/akam/" in event.get("url", "")
                 or "/resources/" in event.get("url", "")]
    for event in important[:20]:
        path = urlparse(event.get("url", "")).path
        lines.append(f"  - `{path}`: {event.get('outcome')}, {event.get('durationMs', 0):.1f} ms, "
                     f"source {event.get('sourceBytes')} bytes / SHA-256 `{event.get('sourceSha256')}`.")
    for kind in ("protection-resource", "akamai"):
        checks = []
        for row in obscura["requests"]:
            if row["kind"] != kind or row["method"] != "GET":
                continue
            executed = [event for event in executions if event.get("url") == row["url"]]
            if executed and row.get("responseBody", {}).get("sha256"):
                checks.append((row["responseBody"]["sha256"] == executed[0].get("sourceSha256"),
                               executed[0].get("outcome")))
        lines.append(f"- Obscura {kind} response-to-executed-source checks: `{checks}`. "
                     "A matching hash proves the captured response bytes were the bytes passed to classic-script execution.")
    lines.append(f"- `Runtime.exceptionThrown` events: Chrome {len(chrome['exceptions'])}, Obscura {len(obscura['exceptions'])}.")
    adobe_c = request_json(c_raw, "soptimize.southwest.com/rest/v1/delivery")
    adobe_o = request_json(o_raw, "soptimize.southwest.com/rest/v1/delivery")
    if adobe_c is not None and adobe_o is not None:
        lines.append("- Adobe delivery POST JSON schema/type/empty-value differences "
                     f"(dynamic values omitted): `{json_structure_differences(adobe_c, adobe_o)}`.")
    lines.extend(["", "## Failed resources and console output", ""])
    for mode, run in (("Chrome", chrome), ("Obscura", obscura)):
        failures = [r for r in run["requests"] if r.get("end") == "Network.loadingFailed"]
        grouped = sorted({(urlparse(r["url"]).netloc + urlparse(r["url"]).path,
                           r.get("failure") or "unknown") for r in failures})
        lines.append(f"- {mode}: {len(failures)} loadingFailed records; console types "
                     f"`{run.get('consoleCounts', {})}`; unique failures `{grouped}`.")
    lines.append("A failed script or CORS request is an observed browser behavior difference. "
                 "Its effect on Akamai scoring cannot be inferred from one pair of sessions.")
    lines.extend(["", "## Storage and cookies", ""])
    c, o = chrome["snapshots"][-1], obscura["snapshots"][-1]
    for label, x, y in (("localStorage keys", set(c["storage"]), set(o["storage"])),
                        ("document.cookie names", set(c["documentCookieNames"]), set(o["documentCookieNames"])),
                        ("final jar names", set(chrome["cookieNamesFinal"]), set(obscura["cookieNamesFinal"]))):
        lines.append(f"- {label}: common {len(x&y)}; Chrome only `{sorted(x-y)}`; Obscura only `{sorted(y-x)}`.")
    for mode, run in (("Chrome", chrome), ("Obscura", obscura)):
        writes = sorted((round(row.get("responseAtMs") or 0), urlparse(row["url"]).netloc,
                         row["responseHeaders"]["set-cookie"].get("names", []))
                        for row in run["requests"]
                        if isinstance(row.get("responseHeaders", {}).get("set-cookie"), dict))
        lines.append(f"- {mode} response Set-Cookie headers (CDP response event ms after capture start, host, names): "
                     f"{len(writes)} records; first 20 `{writes[:20]}`. "
                     "A header is evidence of an attempted write, not proof of cookie-jar acceptance. "
                     "Obscura may buffer native-resource CDP events until navigation completes, so this is event delivery time, not wire receipt time.")
    cookie_writes = obscura.get("cookieWrites", [])
    write_preview = [(round((event.get("timestamp", 0) / 1000 - o_raw["startedAt"]) * 1000),
                      event.get("origin"), event.get("name")) for event in cookie_writes[:20]]
    lines.append(f"- Obscura native document.cookie setter calls: {len(cookie_writes)}; "
                 f"first 20 `{write_preview}`. "
                 "These are attempted JS writes; the private raw event has assignment size/hash, and the final jar shows accepted state.")
    mutations = obscura.get("storageMutations", [])
    lines.append(f"- Chrome DOMStorage events: {len(chrome['storageEvents'])}; Obscura native storage mutations: "
                 f"{len(mutations)}. Obscura records operation, key, time, and value size/hash without wrapping JS APIs. "
                 "These totals have different event scopes and are not a parity metric. Snapshots still occur at different lifecycle points.")
    for mode, rows in (("Chrome", [{"atMs": e.get("atMs"), "operation": e["method"].rsplit(".", 1)[-1],
                                   "key": e.get("params", {}).get("key")} for e in chrome["storageEvents"]]),
                       ("Obscura", [{"atMs": (e.get("timestamp", 0) / 1000 - o_raw["startedAt"]) * 1000,
                                     "operation": e.get("operation"), "key": e.get("key")}
                                    for e in mutations if e.get("area") == "local"])):
        lines.append(f"- {mode} first localStorage writes (time since capture start): "
                     f"`{[(round(e['atMs']) if e['atMs'] is not None else None, e['operation'], e['key']) for e in rows[:20]]}`.")
    lines.extend(["", "## Transport evidence", ""])
    h2 = netlog_h2(directory / "chrome-netlog.json")
    lines.append(f"- Chrome NetLog has {len(h2)} Southwest HTTP/2 sessions: "
                 f"`{[{k: s[k] for k in ('proxy','shopping','akamaiText','windowUpdates')} for s in h2]}`. "
                 "The first WINDOW_UPDATE is part of this fingerprint; later updates are flow control.")
    shapes = tap_shapes(tap, directory / "chrome-netlog.json")
    if shapes:
        counts = {client: sum(s["client"] == client for s in shapes) for client in ("Chrome", "Obscura")}
        lines.append(f"- Transparent proxy tap captured {len(shapes)} Southwest ClientHello records "
                     f"({counts}); parsed details in `tls-shapes.json`. "
                     "Chrome records are identified by exact ClientHello bytes in its NetLog.")
        c_tls = next((s for s in shapes if s["client"] == "Chrome" and s.get("netlogSession") == "document"), None)
        if c_tls is None:
            c_tls = next((s for s in shapes if s["client"] == "Chrome"
                          and not s.get("hasPsk") and s.get("extensionLengths", {}).get("44cd") is not None), None)
        o_tls = next((s for s in shapes if s["client"] == "Obscura" and not s.get("hasPsk")), None)
        if c_tls and o_tls:
            c_ext, o_ext = set(c_tls["extensions"]), set(o_tls["extensions"])
            length_diffs = {key: [c_tls["extensionLengths"][key], o_tls["extensionLengths"][key]]
                            for key in sorted(set(c_tls["extensionLengths"]) & set(o_tls["extensionLengths"]))
                            if c_tls["extensionLengths"][key] != o_tls["extensionLengths"][key]}
            lines.append(f"- Fresh Southwest ClientHello: Chrome {c_tls['recordBytes']} bytes / JA3 "
                         f"`{c_tls['ja3']}`; Obscura {o_tls['recordBytes']} bytes / JA3 "
                         f"`{o_tls['ja3']}`. Cipher suites equal "
                         f"{c_tls['cipherSuites'] == o_tls['cipherSuites']}, signature algorithms equal "
                         f"{c_tls['signatureAlgorithms'] == o_tls['signatureAlgorithms']}, "
                         f"groups equal {c_tls['groups'] == o_tls['groups']}, ALPN equal "
                         f"{c_tls['alpn'] == o_tls['alpn']}, ALPS names "
                         f"`{c_tls['alps']}` versus `{o_tls['alps']}`; extension set Chrome only "
                         f"`{sorted(c_ext-o_ext)}`, Obscura only `{sorted(o_ext-c_ext)}`; "
                         f"different extension lengths `{length_diffs}`. "
                         "JA3 changes with extension order; these observations do not show rejection causality.")
            lines.append(f"- TLS extension payloads: ALPS exact match "
                         f"{c_tls['extensionPayloadSha256'].get('44cd') == o_tls['extensionPayloadSha256'].get('44cd')}; "
                         f"Trust Anchor ID set match "
                         f"{sorted(c_tls.get('trustAnchorIds') or []) == sorted(o_tls.get('trustAnchorIds') or [])}; "
                         f"Trust Anchor ID order match "
                         f"{c_tls.get('trustAnchorIds') == o_tls.get('trustAnchorIds')}. "
                         "Chrome can shuffle Trust Anchor IDs once per process, so order alone is observational.")
        lines.append(f"- Chrome resumed Southwest TLS connections with PSK: "
                     f"{sum(s['client'] == 'Chrome' and s.get('hasPsk') for s in shapes)}; "
                     f"Obscura: {sum(s['client'] == 'Obscura' and s.get('hasPsk') for s in shapes)}.")
        (directory / "tls-shapes.json").write_text(json.dumps(shapes, indent=2) + "\n")
    lines.append("- Obscura HTTP/2 SETTINGS are only covered by primp's offline Chrome 153 golden in this run; "
                 "the tap does not decrypt HTTP/2 frames. Compare actual ClientHello records separately from static profile assertions.")
    if timings:
        lines.extend(["", "### Proxy tunnel milestones", "",
                      "Times are milliseconds after each CONNECT request. The chunk sequence records direction and byte count only; TLS 1.3 encryption prevents identifying HTTP messages within it.", "",
                      "| Browser | Target | CONNECT reply | ClientHello | First server TLS bytes | Chunk sequence |",
                      "| --- | --- | ---: | ---: | ---: | --- |"])
        for line in timings.read_text().splitlines():
            tunnel = json.loads(line)
            start = tunnel.get("connectRequestAt")
            if not start:
                continue
            browser = "Chrome" if start < o_raw["startedAt"] else "Obscura"
            def relative(key):
                at = tunnel.get(key)
                return f"{(at - start) * 1000:.0f} ms" if at is not None else "unavailable"
            chunks = tunnel.get("chunks", [])
            sequence = [(round((chunk["at"] - start) * 1000),
                         "C" if chunk["direction"] == "client" else "S", chunk["bytes"])
                        for chunk in chunks[:32]]
            if len(chunks) > 32:
                sequence.append(("more", len(chunks) - 32, "chunks"))
            lines.append(f"| {browser} | `{tunnel['target']}` | {relative('connectResponseAt')} | "
                         f"{relative('clientHelloAt')} | {relative('firstServerTlsAt')} | `{sequence}` |")
        lines.append("")
    lines.extend(["", "## Capture limits and follow-up", "",
                  "- Obscura CDP captures UTF-8 POST body text up to 16 KiB in `Network.requestWillBeSent`; "
                  "larger or binary bodies have only `bodySize` and need a separate wire capture.",
                  "- The in-page sensor was not wrapped or decoded; that avoids changing native API surfaces during measurement.",
                  "- This is one pair of live sessions. Shopping endpoint outcomes are not an acceptance metric.",
                  "- Keep raw Cookie, ee30, NetLog and response-body data out of Git and shared reports."])
    path = directory / "report.md"
    path.write_text("\n".join(lines) + "\n")
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--tap", type=Path)
    parser.add_argument("--timings", type=Path)
    args = parser.parse_args()
    print(make_report(args.directory, args.tap, args.timings))


if __name__ == "__main__":
    main()

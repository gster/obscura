#!/usr/bin/env python3
"""Check stable Chrome/Obscura Southwest differences against an earlier pair.

Use a baseline captured with the same Obscura tracker policy. Dynamic timing,
cookie values, and sensor tokens are deliberately excluded from hard gates.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from urllib.parse import urlparse

if __package__:
    from .southwest_report import first, load
else:
    from southwest_report import first, load


def signals(directory: Path) -> dict:
    inputs = load(directory / "inputs.json")
    chrome = load(directory / "chrome-summary.json")
    obscura = load(directory / "obscura-summary.json")
    hard = set()
    observed = set()
    if chrome["snapshots"] and obscura["snapshots"]:
        c_identity = chrome["snapshots"][-1].get("identity") or {}
        o_identity = obscura["snapshots"][-1].get("identity") or {}
        for key in ("userAgent", "language", "languages", "platform", "timezone",
                    "viewport", "screen", "outer", "devicePixelRatio"):
            if key in c_identity and key in o_identity and c_identity[key] != o_identity[key]:
                hard.add(f"identity:{key}:diff")
    for kind, method in (("document", "GET"), ("protection-resource", "GET"),
                         ("akamai", "GET"), ("shopping", "POST")):
        c, o = first(chrome, kind, method), first(obscura, kind, method)
        if c and not o:
            (observed if kind == "shopping" else hard).add(f"{kind}:{method}:obscura-missing")
        elif kind != "shopping" and c and o and c.get("status") == 200 and o.get("status") != 200:
            hard.add(f"{kind}:{method}:obscura-status-{o.get('status')}")
        if c and o and c.get("requestBody") and o.get("requestBody"):
            if c["requestBody"] != o["requestBody"]:
                hard.add(f"{kind}:{method}:request-body-diff")
        elif kind == "shopping":
            observed.add("shopping:request-body-unavailable")
    for run, label in ((chrome, "chrome"), (obscura, "obscura")):
        for kind in ("shopping", "akamai"):
            row = first(run, kind, "POST")
            if row and row.get("requestBodyQuery") not in (None, "ok"):
                observed.add(f"{label}:{kind}:post-data-query-unavailable")
            if row and row.get("requestBodyEventMatchesQuery") is False:
                hard.add(f"{label}:{kind}:post-data-query-event-diff")
    for kind, method in (("akamai", "GET"), ("akamai", "POST"), ("shopping", "POST")):
        c, o = first(chrome, kind, method), first(obscura, kind, method)
        if not c or not o:
            continue
        c_headers, o_headers = c.get("requestHeaders", {}), o.get("requestHeaders", {})
        c_cookies = set(c_headers.get("cookie", {}).get("names", []))
        o_cookies = set(o_headers.get("cookie", {}).get("names", []))
        observed.update(f"{kind}:{method}:cookie-chrome-only:{name}" for name in c_cookies - o_cookies)
        observed.update(f"{kind}:{method}:cookie-obscura-only:{name}" for name in o_cookies - c_cookies)
        c_markers = {name: value.get("bytes") for name, value in c_headers.items()
                     if name.startswith("ee30") and isinstance(value, dict)}
        o_markers = {name: value.get("bytes") for name, value in o_headers.items()
                     if name.startswith("ee30") and isinstance(value, dict)}
        for name in c_markers.keys() | o_markers.keys():
            if name not in c_markers or name not in o_markers:
                observed.add(f"{kind}:{method}:{name}:missing")
            elif c_markers[name] != o_markers[name]:
                observed.add(f"{kind}:{method}:{name}:length-diff")
        if kind == "akamai" and method == "POST":
            c_body, o_body = c.get("requestBody"), o.get("requestBody")
            if c_body and o_body and c_body.get("bytes") != o_body.get("bytes"):
                observed.add("akamai:POST:body-size-diff")
    for label, left, right in (
        ("shopping", first(chrome, "shopping", "POST"), first(obscura, "shopping", "POST")),
        ("adobe", next((r for r in chrome["requests"] if "soptimize.southwest.com/rest/v1/delivery" in r["url"] and r["method"] == "POST"), None),
         next((r for r in obscura["requests"] if "soptimize.southwest.com/rest/v1/delivery" in r["url"] and r["method"] == "POST"), None)),
    ):
        if left and right:
            c_site = left.get("requestHeaders", {}).get("sec-fetch-site")
            o_site = right.get("requestHeaders", {}).get("sec-fetch-site")
            if c_site and o_site and c_site != o_site:
                hard.add(f"{label}:sec-fetch-site-diff")
    for kind in ("protection-resource", "akamai", "script"):
        bodies = [{urlparse(r["url"]).path: r.get("responseBody") or {}
                   for r in run["requests"] if r["kind"] == kind and r["method"] == "GET"}
                  for run in (chrome, obscura)]
        for path in bodies[0].keys() & bodies[1].keys():
            a, b = bodies[0][path].get("sha256"), bodies[1][path].get("sha256")
            if a and b and a != b:
                signal = f"{kind}:response-body-diff:{path}"
                (observed if kind == "script" else hard).add(signal)
    if inputs.get("obscuraDiagnosticsEnabled"):
        executions = obscura.get("scriptExecutions", [])
        for kind in ("protection-resource", "akamai"):
            for row in obscura["requests"]:
                if row["kind"] != kind or row["method"] != "GET":
                    continue
                body_hash = (row.get("responseBody") or {}).get("sha256")
                if not body_hash:
                    continue
                matching = [event for event in executions if event.get("url") == row["url"]
                            and event.get("sourceSha256") == body_hash]
                if not matching:
                    hard.add(f"{kind}:source-not-executed")
                elif not any(event.get("outcome") == "ok" for event in matching):
                    hard.add(f"{kind}:execution-failed")
    tls_path = directory / "tls-shapes.json"
    if tls_path.exists():
        shapes = load(tls_path)
        c_tls = next((s for s in shapes if s.get("client") == "Chrome"
                      and s.get("netlogSession") == "document"), None)
        if c_tls is None:
            c_tls = next((s for s in shapes if s.get("client") == "Chrome"
                          and not s.get("hasPsk") and s.get("extensionLengths", {}).get("44cd") is not None), None)
        o_tls = next((s for s in shapes if s.get("client") == "Obscura"
                      and not s.get("hasPsk")), None)
        if c_tls and o_tls:
            for field in ("cipherSuites", "signatureAlgorithms", "groups", "alpn", "alps"):
                if c_tls.get(field) != o_tls.get(field):
                    hard.add(f"tls:{field}:diff")
            if c_tls.get("extensions") != o_tls.get("extensions"):
                observed.add("tls:extension-order-diff")
            for extension in ("ca34", "44cd"):
                if c_tls.get("extensionLengths", {}).get(extension) != o_tls.get("extensionLengths", {}).get(extension):
                    hard.add(f"tls:{extension}:payload-length-diff")
                c_hash = c_tls.get("extensionPayloadSha256", {}).get(extension)
                o_hash = o_tls.get("extensionPayloadSha256", {}).get(extension)
                if extension == "ca34" and c_tls.get("trustAnchorIds") is not None and o_tls.get("trustAnchorIds") is not None:
                    if sorted(c_tls["trustAnchorIds"]) != sorted(o_tls["trustAnchorIds"]):
                        hard.add("tls:ca34:trust-anchor-ids-diff")
                    elif c_tls["trustAnchorIds"] != o_tls["trustAnchorIds"]:
                        observed.add("tls:ca34:trust-anchor-order-diff")
                elif c_hash and o_hash and c_hash != o_hash:
                    hard.add(f"tls:{extension}:payload-content-diff")
        else:
            observed.add("tls:document-client-hello-unavailable")
    for run, label in ((chrome, "chrome"), (obscura, "obscura")):
        if run["requests"] and run.get("playwrightRequestCount") == 0:
            observed.add(f"{label}:playwright-request-events-unavailable")
        for row in run["requests"]:
            if row.get("end") == "Network.loadingFailed" and row.get("failure") != "net::ERR_ABORTED":
                parsed = urlparse(row["url"])
                observed.add(f"{label}:failed:{parsed.netloc}{parsed.path}:{row.get('failure')}")
    c_keys = set(chrome["snapshots"][-1].get("storage", {})) if chrome["snapshots"] else set()
    o_keys = set(obscura["snapshots"][-1].get("storage", {})) if obscura["snapshots"] else set()
    observed.update(f"storage:chrome-only:{key}" for key in c_keys - o_keys)
    observed.update(f"storage:obscura-only:{key}" for key in o_keys - c_keys)
    return {"hard": sorted(hard), "observed": sorted(observed)}


def check(candidate: Path, baseline: Path | None = None) -> dict:
    candidate_inputs = load(candidate / "inputs.json")
    baseline_inputs = load(baseline / "inputs.json") if baseline else None
    if baseline:
        if baseline_inputs.get("obscuraBlockTrackers", True) != candidate_inputs.get("obscuraBlockTrackers", True):
            raise ValueError("baseline and candidate use different tracker policies")
        for field in ("url", "proxy", "chromeBin", "waitSeconds", "captureContext"):
            if baseline_inputs.get(field) != candidate_inputs.get(field):
                raise ValueError(f"baseline and candidate use different {field}")
        candidate_persona = candidate / "persona.json"
        baseline_persona = baseline / "persona.json"
        if candidate_persona.exists() != baseline_persona.exists() or (
            candidate_persona.exists() and load(candidate_persona) != load(baseline_persona)
        ):
            raise ValueError("baseline and candidate use different personas")
    current = signals(candidate)
    previous = signals(baseline) if baseline else {"hard": [], "observed": []}
    new_hard = sorted(set(current["hard"]) - set(previous["hard"]))
    return {"candidate": str(candidate), "baseline": str(baseline) if baseline else None,
            "candidateObscuraSha256": candidate_inputs.get("obscuraSha256"),
            "baselineObscuraSha256": baseline_inputs.get("obscuraSha256") if baseline_inputs else None,
            "newHardGaps": new_hard,
            "resolvedHardGaps": sorted(set(previous["hard"]) - set(current["hard"])),
            "persistentHardGaps": sorted(set(previous["hard"]) & set(current["hard"])),
            "newObservedGaps": sorted(set(current["observed"]) - set(previous["observed"])),
            "resolvedObservedGaps": sorted(set(previous["observed"]) - set(current["observed"])),
            "passed": not new_hard}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("candidate", type=Path)
    parser.add_argument("--baseline", type=Path)
    args = parser.parse_args()
    result = check(args.candidate, args.baseline)
    (args.candidate / "regression.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    if not result["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()

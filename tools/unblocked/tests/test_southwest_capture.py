import base64
import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from tools.unblocked.southwest_compare import classify, headers_summary, response_headers_summary, summarize
from tools.unblocked.southwest_report import tls_shape, json_structure_differences
from tools.unblocked.southwest_gate import check, signals


def client_hello(extension_items):
    extensions = b"".join(
        kind.to_bytes(2, "big") + len(payload).to_bytes(2, "big") + payload
        for kind, payload in extension_items
    )
    hello = (
        b"\x03\x03" + b"\x00" * 32 + b"\x00"
        + b"\x00\x02\x13\x01" + b"\x01\x00"
        + len(extensions).to_bytes(2, "big") + extensions
    )
    handshake = b"\x01" + len(hello).to_bytes(3, "big") + hello
    record = b"\x16\x03\x01" + len(handshake).to_bytes(2, "big") + handshake
    return base64.b64encode(record).decode()


class SouthwestCaptureTests(unittest.TestCase):
    def test_json_structure_diff_omits_dynamic_values(self):
        left = {"items": [{"state": "", "curve": "42"}], "id": "secret-one"}
        right = {"items": [{"state": "token", "curve": 42}], "id": "secret-two"}
        differences = json_structure_differences(left, right)
        self.assertEqual(differences, ["/items[]/curve: str / int", "/items[]/state: Obscura nonempty"])
        self.assertNotIn("secret", str(differences))

    def test_summary_uses_protocol_start_and_finish_for_duration(self):
        run = {"mode": "obscura", "url": "https://example.test/", "capabilities": {},
               "consentClicked": False, "events": [], "errors": [], "snapshots": [],
               "cookies": [], "playwrightRequests": [], "postBodies": {}, "bodies": {},
               "requests": [{"requestId": "1", "url": "https://example.test/api",
                             "method": "GET", "request": {"headers": {}},
                             "protocolTimestamp": 100.0,
                             "requestExtraInfo": {"obscuraTiming": {
                                 "requestPreparedAt": 100.25, "responseHeadersAt": 101.0}},
                             "end": {"method": "Network.loadingFinished",
                                     "params": {"timestamp": 101.25}}}]}
        self.assertEqual(summarize(run)["requests"][0]["requestDurationMs"], 1250.0)
        self.assertEqual(summarize(run)["requests"][0]["requestPreparedAfterMs"], 250.0)
        self.assertEqual(summarize(run)["requests"][0]["responseHeadersAfterMs"], 1000.0)

    def test_tls_parser_keeps_extensions_after_alpn(self):
        encoded = client_hello([
            (16, b"\x00\x03\x02h2"),
            (0x44CD, b"\x00\x03\x02h2"),
            (0xCA34, b"\x00\x00"),
            (10, b"\x00\x04\x00\x1d\x00\x17"),
            (13, b"\x00\x04\x04\x03\x08\x04"),
        ])
        shape = tls_shape(encoded)
        self.assertEqual(shape["alpn"], ["h2"])
        self.assertEqual(shape["extensions"], ["0010", "44cd", "ca34", "000a", "000d"])
        self.assertEqual(shape["extensionLengths"]["44cd"], 5)
        self.assertEqual(shape["extensionPayloadSha256"]["44cd"], hashlib.sha256(b"\x00\x03\x02h2").hexdigest())
        self.assertEqual(shape["extensionPayloadSha256"]["ca34"], hashlib.sha256(b"\x00\x00").hexdigest())
        self.assertEqual(shape["trustAnchorIds"], [])
        self.assertEqual(shape["groups"], ["001d", "0017"])
        self.assertEqual(shape["signatureAlgorithms"], ["0403", "0804"])

    def test_tls_gate_compares_anchor_ids_not_process_shuffle(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            (directory / "inputs.json").write_text("{}")
            summary = {"requests": [], "snapshots": []}
            for browser in ("chrome", "obscura"):
                (directory / f"{browser}-summary.json").write_text(json.dumps(summary))
            common = {"cipherSuites": ["1301"], "signatureAlgorithms": ["0403"],
                      "groups": ["001d"], "alpn": ["h2"], "alps": ["h2"],
                      "extensionLengths": {"44cd": 5, "ca34": 5},
                      "extensionPayloadSha256": {"44cd": "same", "ca34": "different"}}
            chrome = dict(common, client="Chrome", netlogSession="document",
                          extensions=["ca34", "44cd"], trustAnchorIds=["aa", "bb"])
            obscura = dict(common, client="Obscura", hasPsk=False,
                           extensions=["44cd", "ca34"], trustAnchorIds=["bb", "aa"])
            (directory / "tls-shapes.json").write_text(json.dumps([chrome, obscura]))
            result = signals(directory)
            self.assertEqual(result["hard"], [])
            self.assertIn("tls:ca34:trust-anchor-order-diff", result["observed"])
            self.assertIn("tls:extension-order-diff", result["observed"])
            obscura["trustAnchorIds"] = ["bb", "cc"]
            (directory / "tls-shapes.json").write_text(json.dumps([chrome, obscura]))
            self.assertIn("tls:ca34:trust-anchor-ids-diff", signals(directory)["hard"])

    def test_classification_and_secret_safe_summary(self):
        self.assertEqual(classify("https://www.southwest.com/akam/13/pixel_x"), "akamai")
        self.assertEqual(classify("https://www.southwest.com/api/air-booking/v1/air-booking/page/air/booking/shopping"), "shopping")
        result = headers_summary({"Cookie": "a=one; b=two", "ee30zvqlwf-a": "secret", "Accept": "*/*"})
        self.assertEqual(result["cookie"], {"names": ["a", "b"], "bytes": 12})
        self.assertEqual(result["ee30zvqlwf-a"]["bytes"], 6)
        self.assertNotIn("secret", str(result))
        response = headers_summary({"Set-Cookie": "sid=secret; Path=/\nfoo=other; HttpOnly"})
        self.assertEqual(response["set-cookie"]["names"], ["sid", "foo"])
        self.assertNotIn("secret", str(response))

    def test_response_summary_uses_all_raw_set_cookie_fields(self):
        raw = {"fields": [{"nameBase64": base64.b64encode(b"set-cookie").decode(),
                           "valueBase64": base64.b64encode(value).decode()}
                          for value in (b"first=secret; Path=/", b"second=token; Path=/")]}
        summary = response_headers_summary({"response": {"rawHeaders": raw}},
                                           {"set-cookie": "second=token; Path=/"})
        self.assertEqual(summary["set-cookie"]["names"], ["first", "second"])
        self.assertNotIn("secret", str(summary))
        self.assertNotIn("token", str(summary))

    def test_regression_gate_detects_new_post_body_gap_and_rejects_mixed_policy(self):
        with tempfile.TemporaryDirectory() as root:
            def pair(name, obscura_body, blocked):
                directory = Path(root) / name
                directory.mkdir()
                (directory / "inputs.json").write_text(json.dumps({"obscuraBlockTrackers": blocked}))
                for mode, request_body in (("chrome", "same"), ("obscura", obscura_body)):
                    shopping = {"kind": "shopping", "method": "POST", "url": "https://www.southwest.com/booking/shopping",
                                "status": 200, "requestBody": {"sha256": request_body}, "end": "Network.loadingFinished"}
                    (directory / f"{mode}-summary.json").write_text(json.dumps({"requests": [shopping], "snapshots": []}))
                    body = {"success": True, "data": {"searchResults": {"airProducts": [
                        {"details": [{}], "originationAirportCode": "BWI", "destinationAirportCode": "MCO"}]}}}
                    raw = {"requests": [{"requestId": "1", "url": shopping["url"]}],
                           "bodies": {"1": {"base64": base64.b64encode(json.dumps(body).encode()).decode()}}}
                    (directory / f"{mode}-raw.json").write_text(json.dumps(raw))
                return directory
            baseline = pair("baseline", "same", True)
            candidate = pair("candidate", "different", True)
            self.assertEqual(check(candidate, baseline)["newHardGaps"],
                             ["shopping:POST:request-body-diff"])
            for mode, width in (("chrome", 1365), ("obscura", 2560)):
                path = candidate / f"{mode}-summary.json"
                summary = json.loads(path.read_text())
                summary["snapshots"] = [{"identity": {"screen": {"width": width}}}]
                path.write_text(json.dumps(summary))
            self.assertIn("identity:screen:diff", check(candidate, baseline)["newHardGaps"])
            (candidate / "inputs.json").write_text(json.dumps({"obscuraBlockTrackers": False}))
            with self.assertRaisesRegex(ValueError, "different tracker policies"):
                check(candidate, baseline)
            (candidate / "inputs.json").write_text(json.dumps({"obscuraBlockTrackers": True,
                                                                 "proxy": "http://127.0.0.1:17890"}))
            with self.assertRaisesRegex(ValueError, "different proxy"):
                check(candidate, baseline)
            (candidate / "inputs.json").write_text(json.dumps({"obscuraBlockTrackers": True,
                                                                 "captureContext": {"screen": [1365, 768]}}))
            with self.assertRaisesRegex(ValueError, "different captureContext"):
                check(candidate, baseline)

    def test_shopping_response_degradation_is_not_a_regression_signal(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            (directory / "inputs.json").write_text("{}")
            for mode, status in (("chrome", 403), ("obscura", None)):
                (directory / f"{mode}-summary.json").write_text(json.dumps({
                    "requests": [{"kind": "shopping", "method": "POST",
                                  "url": "https://www.southwest.com/booking/shopping",
                                  "status": status, "requestBody": {"sha256": "same"}}],
                    "snapshots": []}))
            result = check(directory)
            self.assertTrue(result["passed"])
            self.assertEqual(result["newHardGaps"], [])
            self.assertNotIn("candidateBusiness", result)

    def test_diagnostic_gate_requires_executed_akamai_source(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root)
            (directory / "inputs.json").write_text(json.dumps({"obscuraDiagnosticsEnabled": True}))
            row = {"kind": "akamai", "method": "GET", "url": "https://www.southwest.com/akam/13/a",
                   "status": 200, "responseBody": {"sha256": "source-hash"}}
            for mode in ("chrome", "obscura"):
                (directory / f"{mode}-summary.json").write_text(json.dumps({
                    "requests": [row], "snapshots": [], "scriptExecutions": []}))
            self.assertIn("akamai:source-not-executed", check(directory)["newHardGaps"])
            (directory / "obscura-summary.json").write_text(json.dumps({
                "requests": [row], "snapshots": [], "scriptExecutions": [{
                    "url": row["url"], "sourceSha256": "source-hash", "outcome": "ok"}]}))
            self.assertEqual(check(directory)["newHardGaps"], [])


if __name__ == "__main__":
    unittest.main()

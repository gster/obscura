"""Offline contract/guardrail tests. No external network and no browser required."""
from __future__ import annotations
import asyncio
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

SPEC = importlib.util.spec_from_file_location("audit_collect", Path(__file__).resolve().parents[1] / "scripts" / "collect.py")
assert SPEC and SPEC.loader
collector = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(collector)


def fingerprint():
    return {"status": "COLLECT_SUCCESSFUL", "fingerprintHash": "fixture-hash", "fingerprintId": "fixture-id",
            "fingerprintParts": {"part": "fixture"}, "signals": {"fixture": True}, "botScore": 0, "reasons": []}


def rebrowser():
    return [{"type": key, "rating": 0, "note": "unexercised fixture"} for key in sorted(collector.REBROWSER_TYPES)]


class ContractTests(unittest.TestCase):
    def test_fingerprint_success(self):
        self.assertEqual(collector.validate_data("fingerprint-scan", fingerprint()), [])

    def test_fingerprint_not_ready(self):
        value = fingerprint(); value["status"] = "COLLECTING"
        self.assertIn("collection_not_successful", collector.validate_data("fingerprint-scan", value))

    def test_fingerprint_empty_is_not_success(self):
        self.assertTrue(collector.validate_data("fingerprint-scan", {}))

    def test_missing_signal_is_not_false(self):
        value = fingerprint(); del value["signals"]
        self.assertIn("missing_or_invalid:signals", collector.validate_data("fingerprint-scan", value))
        self.assertNotIn("signals", value)

    def test_boolean_is_not_numeric_score(self):
        value = fingerprint(); value["botScore"] = False
        self.assertIn("missing_or_invalid:botScore", collector.validate_data("fingerprint-scan", value))

    def test_tls_h2_is_optional(self):
        value = {"ip": "192.0.2.1", "tls": {"fixture": True}, "http_version": "HTTP/1.1"}
        self.assertEqual(collector.validate_data("tls-peet", value), [])

    def test_tls_empty_is_partial(self):
        self.assertTrue(collector.validate_data("tls-peet", {"tls": {}}))

    def test_rebrowser_zero_remains_zero(self):
        value = rebrowser()
        self.assertEqual(collector.validate_data("rebrowser", value), [])
        self.assertTrue(all(row["rating"] == 0 for row in value))
        self.assertFalse(any("verdict" in row for row in value))

    def test_rebrowser_missing_test(self):
        value = [r for r in rebrowser() if r["type"] != "sourceUrlLeak"]
        self.assertIn("missing_test:sourceUrlLeak", collector.validate_data("rebrowser", value))

    def test_rebrowser_bad_shape(self):
        self.assertTrue(collector.validate_data("rebrowser", {}))

    def test_unknown_adapter(self):
        self.assertIn("unknown_adapter", collector.validate_data("unknown", {}))

    def test_loopback_endpoints(self):
        for endpoint in ("http://127.0.0.1:9222", "ws://localhost:9222/devtools/browser/fixture", "http://[::1]:9222"):
            collector.validate_endpoint(endpoint, False)

    def test_remote_requires_opt_in(self):
        with self.assertRaises(ValueError):
            collector.validate_endpoint("https://browser.example.invalid:9222", False)
        collector.validate_endpoint("https://browser.example.invalid:9222", True)

    def test_endpoint_credentials_rejected(self):
        with self.assertRaises(ValueError):
            collector.validate_endpoint("http://name:secret@127.0.0.1:9222", False)

    def test_invalid_scheme_rejected(self):
        with self.assertRaises(ValueError):
            collector.validate_endpoint("file:///tmp/endpoint", True)

    def test_url_metadata_redacted(self):
        self.assertEqual(collector.safe_url("https://name:secret@example.invalid/a?token=secret#x"), "https://example.invalid/a")

    def test_error_endpoint_redacted(self):
        endpoint = "http://127.0.0.1:9222/?token=secret"
        output = collector.error_text(RuntimeError("failed " + endpoint), endpoint)
        self.assertNotIn("secret", output)
        self.assertIn("[CDP endpoint]", output)

    def test_write_json_preserves_types(self):
        with tempfile.TemporaryDirectory() as d:
            target = Path(d) / "result.json"
            collector.write_json(target, {"zero": 0, "false": False, "null": None})
            self.assertEqual(json.loads(target.read_text()), {"zero": 0, "false": False, "null": None})


class AsyncGuardTests(unittest.IsolatedAsyncioTestCase):
    async def test_fresh_baseline_does_not_read_existing_contexts(self):
        owned = object()
        class Browser:
            @property
            def contexts(self):
                raise AssertionError("must not reuse contexts")
            async def new_context(self, **options):
                self.options = options
                return owned
        browser = Browser()
        args = SimpleNamespace(context_mode="fresh", action_timeout=1, context_index=0)
        context, owns = await collector.select_context(browser, args, {"role": "baseline"})
        self.assertIs(context, owned)
        self.assertTrue(owns)
        self.assertEqual(browser.options, {"no_viewport": True})

    async def test_failed_fresh_context_never_falls_back(self):
        class Browser:
            @property
            def contexts(self):
                raise AssertionError("must not fall back")
            async def new_context(self, **options):
                raise RuntimeError("create context failed")
        args = SimpleNamespace(context_mode="fresh", action_timeout=1, context_index=0)
        with self.assertRaisesRegex(RuntimeError, "create context failed"):
            await collector.select_context(Browser(), args, {"role": "baseline"})

    async def test_baseline_existing_context_rejected(self):
        args = SimpleNamespace(context_mode="existing", context_index=0)
        with self.assertRaisesRegex(ValueError, "newly created anonymous"):
            await collector.select_context(object(), args, {"role": "baseline"})

    async def test_candidate_existing_context_is_not_owned(self):
        existing = object()
        browser = SimpleNamespace(contexts=[existing])
        args = SimpleNamespace(context_mode="existing", context_index=0)
        context, owns = await collector.select_context(browser, args, {"role": "candidate"})
        self.assertIs(context, existing)
        self.assertFalse(owns)

    async def test_missing_role_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "role"):
            await collector.select_context(object(), SimpleNamespace(), {})

    async def test_action_failure_recorded(self):
        async def fail():
            raise RuntimeError("fixture failure")
        result = {"actions": []}
        await collector.action(result, "fixture", "main", fail, 1)
        self.assertFalse(result["actions"][0]["succeeded"])
        self.assertIn("fixture failure", result["actions"][0]["error"])

    async def test_action_timeout_recorded(self):
        async def slow():
            await asyncio.sleep(10)
        result = {"actions": []}
        await collector.action(result, "fixture", "main", slow, 0.01)
        self.assertFalse(result["actions"][0]["succeeded"])

    async def test_isolated_exception_rejected_and_detached(self):
        class Session:
            detached = False
            async def send(self, name, params=None):
                if name == "Page.getFrameTree":
                    return {"frameTree": {"frame": {"id": "fixture-frame"}}}
                if name == "Page.createIsolatedWorld":
                    return {"executionContextId": 7}
                return {"exceptionDetails": {"text": "fixture exception"}}
            async def detach(self):
                self.detached = True
        session = Session()
        async def new_session(_):
            return session
        page = SimpleNamespace(context=SimpleNamespace(new_cdp_session=new_session))
        with self.assertRaises(RuntimeError):
            await collector.isolated_probe(page)
        self.assertTrue(session.detached)


if __name__ == "__main__":
    unittest.main()

import json
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from tools.unblocked.cdp_trace import Normalizer, Trace, first_divergence
from tools.unblocked.cdp_fixture import (
    compare_runs,
    comparison_status,
    process_group_exists,
    terminate_process_group,
)


class FakeSession:
    def __init__(self):
        self.handlers = {}

    def on(self, event, handler):
        self.handlers[event] = handler

    def send(self, method, params=None):
        if method == "Page.navigate":
            self.handlers["Page.frameNavigated"](
                {"frame": {"id": "frame-random", "url": params["url"]}}
            )
            return {"frameId": "frame-random", "loaderId": "loader-random"}
        return {}


class TraceTests(unittest.TestCase):
    def test_normalizer_aliases_ids_without_dropping_payload_data(self):
        normalizer = Normalizer("http://127.0.0.1:43111")

        value = normalizer.normalize(
            {
                "frameId": "random-frame",
                "parentFrameId": "random-frame",
                "requestId": "request-99",
                "url": "http://127.0.0.1:43111/fixture?token=secret",
                "headers": {
                    "Authorization": "Bearer 24e5880e-d062-4f6a-a47f-8e1d5e751234",
                    "Cookie": "sid=secret",
                    "Accept": "text/html",
                },
                "postData": "private body 24e5880e-d062-4f6a-a47f-8e1d5e751234",
                "name": "__playwright_utility_world_page@3fa34dee1e524514c7ae8aa1313706fa",
                "timestamp": 487466.197911,
                "documentURL": "http://127.0.0.1:43111/fixture?token=another-secret",
            }
        )

        self.assertEqual(value["frameId"], "<frameId:1>")
        self.assertEqual(value["parentFrameId"], "<frameId:1>")
        self.assertEqual(value["requestId"], "<requestId:1>")
        self.assertEqual(value["url"], "<fixture>/fixture?token=secret")
        self.assertEqual(
            value["headers"]["Authorization"],
            "Bearer 24e5880e-d062-4f6a-a47f-8e1d5e751234",
        )
        self.assertEqual(value["headers"]["Cookie"], "sid=secret")
        self.assertEqual(value["headers"]["Accept"], "text/html")
        self.assertEqual(
            value["postData"], "private body 24e5880e-d062-4f6a-a47f-8e1d5e751234"
        )
        self.assertEqual(value["name"], "__playwright_utility_world_page@<id>")
        self.assertEqual(value["timestamp"], "<protocol-time>")
        self.assertEqual(value["documentURL"], "<fixture>/fixture?token=another-secret")
        self.assertEqual(
            normalizer.normalize({"value": {"requestId": "business-request-id"}}),
            {"value": {"requestId": "business-request-id"}},
        )

    def test_trace_preserves_raw_data_and_temporal_order(self):
        trace = Trace("chromium-cdp", "http://127.0.0.1:43111")
        session = FakeSession()
        trace.observe(session, ["Page.frameNavigated"], session_name="page-1")

        result = trace.send(
            session,
            "Page.navigate",
            {
                "url": "http://127.0.0.1:43111/fixture?token=secret",
                "headers": {"Authorization": "Bearer 24e5880e-d062-4f6a-a47f-8e1d5e751234"},
            },
            session_name="page-1",
        )

        self.assertEqual(result["frameId"], "frame-random")
        entries = trace.document()["entries"]
        self.assertEqual([entry["kind"] for entry in entries], ["command", "event", "response"])
        self.assertEqual([entry["sequence"] for entry in entries], [1, 2, 3])
        self.assertEqual(entries[0]["callId"], 1)
        self.assertEqual(entries[2]["callId"], 1)
        self.assertEqual(entries[1]["receivedDuringCallId"], 1)
        self.assertEqual(entries[1]["afterCallId"], 1)
        self.assertEqual(entries[0]["session"], "page-1")
        self.assertEqual(entries[1]["params"]["frame"]["id"], "frame-random")
        self.assertEqual(
            entries[0]["params"]["headers"]["Authorization"],
            "Bearer 24e5880e-d062-4f6a-a47f-8e1d5e751234",
        )
        self.assertEqual(
            entries[0]["params"]["url"],
            "http://127.0.0.1:43111/fixture?token=secret",
        )
        self.assertGreaterEqual(entries[2]["atMs"], entries[0]["atMs"])

    def test_failed_command_retains_raw_input_and_error(self):
        class FailingSession:
            def send(self, method, params):
                raise RuntimeError(f"failed with {params['token']}")

        trace = Trace("obscura-cdp", "http://127.0.0.1:43111")
        with self.assertRaisesRegex(RuntimeError, "secret-value"):
            trace.send(
                FailingSession(),
                "Runtime.evaluate",
                {"token": "secret-value"},
                session_name="page-1",
            )
        entries = trace.document()["entries"]
        self.assertEqual(entries[0]["params"]["token"], "secret-value")
        self.assertEqual(entries[1]["kind"], "error")
        self.assertIn("secret-value", entries[1]["error"]["message"])

    def test_first_divergence_ignores_timing_but_reports_stable_path(self):
        left = {
            "entries": [
                {"sequence": 1, "atMs": 1.2, "kind": "command", "method": "Page.enable"},
                {"sequence": 2, "atMs": 2.1, "kind": "response", "method": "Page.enable", "result": {}},
            ]
        }
        right = json.loads(json.dumps(left))
        right["entries"][0]["atMs"] = 40.0
        right["entries"][1]["atMs"] = 41.0
        self.assertIsNone(first_divergence(left, right))

        right["entries"][1]["result"] = {"unexpected": True}
        divergence = first_divergence(left, right)
        self.assertEqual(divergence["entry"], 2)
        self.assertEqual(divergence["path"], "entries[1].result.unexpected")
        self.assertEqual(divergence["left"], "<missing>")
        self.assertTrue(divergence["right"])

    def test_comparison_normalizes_protocol_ids_but_not_business_payload(self):
        def document(frame_id, credential):
            return {
                "fixtureOrigin": "http://127.0.0.1:43111",
                "entries": [
                    {
                        "sequence": 1,
                        "kind": "event",
                        "method": "Page.frameNavigated",
                        "params": {
                            "frame": {"id": frame_id},
                            "headers": {"Authorization": credential},
                        },
                    }
                ],
            }

        left = document("frame-left", "Bearer 24e5880e-d062-4f6a-a47f-8e1d5e751234")
        right = document("frame-right", "Bearer 24e5880e-d062-4f6a-a47f-8e1d5e751234")
        self.assertIsNone(first_divergence(left, right))

        right["entries"][0]["params"]["headers"]["Authorization"] = (
            "Bearer d07c4a7e-afdf-4bf4-946a-4b809c0d4567"
        )
        divergence = first_divergence(left, right)
        self.assertEqual(
            divergence["path"], "entries[0].params.headers.Authorization"
        )

    def test_comparison_is_type_sensitive_and_preserves_nested_at_ms(self):
        left = {
            "entries": [
                {
                    "sequence": 1,
                    "kind": "response",
                    "method": "Runtime.evaluate",
                    "result": {"value": {"enabled": True, "atMs": 1}},
                }
            ]
        }
        right = json.loads(json.dumps(left))
        right["entries"][0]["result"]["value"]["enabled"] = 1
        divergence = first_divergence(left, right)
        self.assertEqual(divergence["path"], "entries[0].result.value.enabled")

        right = json.loads(json.dumps(left))
        right["entries"][0]["result"]["value"]["atMs"] = 2
        divergence = first_divergence(left, right)
        self.assertEqual(divergence["path"], "entries[0].result.value.atMs")

    def test_comparison_preserves_unavailable_resource_timing(self):
        left = {
            "entries": [
                {
                    "sequence": 1,
                    "kind": "event",
                    "method": "Network.responseReceived",
                    "params": {"response": {"timing": {"sslStart": -1}}},
                }
            ]
        }
        right = json.loads(json.dumps(left))
        right["entries"][0]["params"]["response"]["timing"]["sslStart"] = 2.5
        divergence = first_divergence(left, right)
        self.assertEqual(
            divergence["path"], "entries[0].params.response.timing.sslStart"
        )

    def test_comparison_rejects_invalid_or_empty_documents(self):
        with self.assertRaisesRegex(ValueError, "must contain entries"):
            first_divergence({}, {})
        with self.assertRaisesRegex(ValueError, "non-empty"):
            first_divergence({"entries": []}, {"entries": []})

    def test_missing_entry_reports_its_actual_position(self):
        left = {
            "entries": [
                {"sequence": 1, "kind": "command", "method": "Page.enable"},
                {"sequence": 2, "kind": "response", "method": "Page.enable"},
            ]
        }
        right = {"entries": [left["entries"][0]]}
        divergence = first_divergence(left, right)
        self.assertEqual(divergence["entry"], 2)
        self.assertEqual(divergence["path"], "entries[1]")
        self.assertEqual(divergence["right"], "<missing>")

    def test_two_cdp_modes_compare_without_launch_reference(self):
        document = {
            "entries": [
                {"sequence": 1, "kind": "command", "method": "Page.enable"}
            ]
        }
        comparisons = compare_runs(
            {"chromium-cdp": document, "obscura-cdp": json.loads(json.dumps(document))}
        )
        self.assertEqual(len(comparisons), 1)
        self.assertEqual(comparisons[0]["left"], "chromium-cdp")
        self.assertTrue(comparisons[0]["equal"])
        self.assertEqual(comparison_status(comparisons), "passed")
        self.assertEqual(comparison_status([]), "inconclusive")

    def test_trace_document_round_trips_as_json(self):
        trace = Trace("obscura-cdp", "http://127.0.0.1:43111")
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.json"
            trace.write(path)
            document = json.loads(path.read_text())
            self.assertEqual(document["mode"], "obscura-cdp")
            self.assertEqual(document["logicalScopes"]["browserContext"], "context-1")

    def test_process_group_cleanup_after_leader_exits_first(self):
        with tempfile.TemporaryDirectory() as directory:
            ready = Path(directory) / "child-ready"
            child_code = (
                "import pathlib,sys,time;"
                "pathlib.Path(sys.argv[1]).write_text('ready');"
                "time.sleep(60)"
            )
            leader_code = (
                "import subprocess,sys,time,pathlib;"
                "subprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2]]);"
                "p=pathlib.Path(sys.argv[2]);"
                "\nwhile not p.exists(): time.sleep(0.01)"
            )
            process = subprocess.Popen(
                [sys.executable, "-c", leader_code, child_code, str(ready)],
                start_new_session=True,
            )
            process.wait(timeout=5)
            self.assertTrue(process_group_exists(process.pid))
            terminate_process_group(process, timeout=0.2)
            self.assertFalse(process_group_exists(process.pid))

    def test_process_group_cleanup_kills_child_that_ignores_term(self):
        with tempfile.TemporaryDirectory() as directory:
            ready = Path(directory) / "child-ready"
            child_code = (
                "import pathlib,signal,sys,time;"
                "signal.signal(signal.SIGTERM,signal.SIG_IGN);"
                "pathlib.Path(sys.argv[1]).write_text('ready');"
                "time.sleep(60)"
            )
            leader_code = (
                "import subprocess,sys,time;"
                "subprocess.Popen([sys.executable,'-c',sys.argv[1],sys.argv[2]]);"
                "time.sleep(60)"
            )
            process = subprocess.Popen(
                [sys.executable, "-c", leader_code, child_code, str(ready)],
                start_new_session=True,
            )
            deadline = time.monotonic() + 5
            while not ready.exists() and time.monotonic() < deadline:
                time.sleep(0.01)
            self.assertTrue(ready.exists())
            terminate_process_group(process, timeout=0.2)
            self.assertFalse(process_group_exists(process.pid))


if __name__ == "__main__":
    unittest.main()

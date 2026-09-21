import tempfile
import unittest
import socket
from pathlib import Path

from tools.unblocked.cdp_capacity import parse_http_response
from tools.unblocked.cdp_ws_capacity import (
    Evidence,
    headers_named,
    sha256_file,
    parse_darwin_thread_count,
    validate_cdp_command,
    validate_rejection,
    validate_snapshot_recovery,
    validate_ws,
    ws_close,
    ws_command_prefix,
)


class CdpWsCapacityTests(unittest.TestCase):
    def test_snapshot_validator_rejects_missing_baseline_fields(self):
        failures = validate_snapshot_recovery(
            {"serverFdCount": None, "listenQueue": None, "process": None,
             "threads": {"threadCount": 0}},
            {},
        )
        self.assertIn("baseline serverFdCount is missing", failures)
        self.assertIn("baseline listenQueue is missing", failures)
        self.assertIn("baseline process snapshot is missing", failures)
        self.assertIn("baseline threadCount is missing or non-positive", failures)

    def test_snapshot_validator_rejects_missing_recovery_fields_and_drift(self):
        baseline = {
            "serverFdCount": 8,
            "listenQueue": {"completed": 0},
            "process": {"pid": 1},
            "threads": {"threadCount": 3},
        }
        recovery = {
            "serverFdCount": None,
            "listenQueue": None,
            "process": None,
            "threads": {"threadCount": 2},
        }
        failures = validate_snapshot_recovery(baseline, {"recovery1": recovery})
        self.assertIn("recovery1 serverFdCount is missing", failures)
        self.assertIn("recovery1 listenQueue is missing", failures)
        self.assertIn("recovery1 process snapshot is missing", failures)
        self.assertIn("recovery1 thread count drifted", failures)
    def test_darwin_thread_count_ignores_ps_header_and_blank_lines(self):
        raw = (
            b"USER PID   THRD ST A CPU TIME\n"
            b"root 123   1    S 0.0 00:00\n"
            b"root 123   2    S 0.0 00:00\n\n"
        )
        self.assertEqual(parse_darwin_thread_count(raw), 2)

    def test_darwin_thread_count_retains_non_utf8_as_latin1(self):
        self.assertEqual(parse_darwin_thread_count(b"USER PID\n\xff\n"), 1)

    def test_rejection_requires_complete_503_and_reason(self):
        response = parse_http_response(
            b"HTTP/1.1 503 Service Unavailable\r\n"
            b"X-Obscura-Reason: max-connections\r\n"
            b"Content-Length: 2\r\n\r\nOK"
        )
        result = {"error": None, "parsedResponse": response}
        self.assertEqual(headers_named(response, "x-obscura-reason"), ["max-connections"])
        self.assertEqual(validate_rejection(result), [])

    def test_rejection_rejects_wrong_reason_and_incomplete_body(self):
        response = parse_http_response(
            b"HTTP/1.1 503 Service Unavailable\r\n"
            b"X-Obscura-Reason: busy\r\nContent-Length: 3\r\n\r\nno"
        )
        failures = validate_rejection({"error": None, "parsedResponse": response})
        self.assertTrue(any("body was incomplete" in failure for failure in failures))
        self.assertTrue(any("unexpected X-Obscura-Reason" in failure for failure in failures))

    def test_ws_validation_requires_matching_id_and_clean_close(self):
        result = {
            "name": "ws1",
            "error": None,
            "parsedResponse": {"status": 101},
            "result": {"path": "upgrade.result.json"},
            "command": {
                "commandError": None,
                "commandJsonError": None,
                "serverCommandJson": {"id": 2, "result": {}},
                "result": {"path": "command.result.json"},
            },
            "closeError": None,
        }
        self.assertTrue(
            any("wrong or unsuccessful CDP response" in failure for failure in validate_ws(result, 1))
        )
        result["command"]["serverCommandJson"]["id"] = 1
        self.assertEqual(validate_ws(result, 1), [])
        self.assertEqual(result["result"]["path"], "upgrade.result.json")
        self.assertEqual(result["command"]["result"]["path"], "command.result.json")

    def test_cdp_command_rejects_matching_id_with_error(self):
        command = {
            "commandError": None,
            "commandJsonError": None,
            "serverCommandJson": {
                "id": 2,
                "error": {"code": -32000, "message": "failed"},
            },
        }
        failures = validate_cdp_command(command, 2, "held socket")
        self.assertTrue(any("unsuccessful CDP response" in failure for failure in failures))

    def test_ws_close_retains_partial_wire_before_timeout(self):
        client, peer = socket.socketpair()
        try:
            peer.sendall(b"\x88\x02partial-close-wire")
            with tempfile.TemporaryDirectory() as temporary:
                evidence = Evidence(Path(temporary) / "evidence")
                result = ws_close(evidence, "partial", client, 0.01)
                wire = evidence.root / result["serverCloseWire"]["path"]
                self.assertEqual(wire.read_bytes(), b"\x88\x02partial-close-wire")
                self.assertIsNotNone(result["closeError"])
        finally:
            peer.close()

    def test_sha256_file_is_binary_exact(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / "binary"
            path.write_bytes(b"\x00\xffraw\n")
            self.assertEqual(
                sha256_file(path),
                "bfac4c888b20ad05480038bd413886a95ef455ab47680f5a439207f3db228fff",
            )

    def test_evidence_does_not_decode_or_truncate_raw_bytes(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            raw = bytes(range(256)) * 32
            relative = evidence.write_bytes("raw/frame.bin", raw)
            self.assertEqual((evidence.root / relative).read_bytes(), raw)
            self.assertEqual(evidence.artifacts[relative]["bytes"], len(raw))

    def test_evidence_keeps_repeated_socket_commands_in_unique_paths(self):
        first_path = ws_command_prefix("ws1", 1)
        second_path = ws_command_prefix("ws1", 2)
        self.assertNotEqual(first_path, second_path)
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            first = evidence.write_bytes(f"{first_path}/payload.bin", b"one")
            second = evidence.write_bytes(f"{second_path}/payload.bin", b"two")
            self.assertNotEqual(first, second)
            self.assertEqual((evidence.root / first).read_bytes(), b"one")
            self.assertEqual((evidence.root / second).read_bytes(), b"two")


if __name__ == "__main__":
    unittest.main()

import argparse
import socket
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools.unblocked import cdp_silent_pending as silent_pending
from tools.unblocked.cdp_capacity import parse_http_response
from tools.unblocked.cdp_silent_pending import (
    CLASSIFICATION_RESERVE,
    PENDING_LIMIT_REASON,
    PENDING_TIMEOUT_REASON,
    SILENT_PENDING_LIMIT,
    SILENT_TTL_SECONDS,
    Evidence,
    cpu_window,
    parse_cpu_time,
    parse_darwin_thread_count,
    parse_lsof_target_tcp,
    parse_process_cpu,
    persist_open_results,
    record_http,
    read_wire_timed,
    validate_recovery,
    validate_socket_group,
    validate_ttl_timing,
    validate_accepted_barrier,
    validate_close_result,
    validate_http,
    validate_snapshot,
    wait_snapshot,
)


class CdpSilentPendingTests(unittest.TestCase):
    def test_production_contract_constants_are_explicit(self):
        self.assertEqual(SILENT_PENDING_LIMIT, 256)
        self.assertEqual(CLASSIFICATION_RESERVE, 16)
        self.assertEqual(SILENT_TTL_SECONDS, 10.0)
        self.assertEqual(PENDING_LIMIT_REASON, "max-pending-request-heads")
        self.assertEqual(PENDING_TIMEOUT_REASON, "request-head-timeout")

    def test_parse_cpu_time_supports_darwin_ps_shapes(self):
        self.assertAlmostEqual(parse_cpu_time("00:01.25"), 1.25)
        self.assertAlmostEqual(parse_cpu_time("01:02:03.50"), 3723.5)
        self.assertAlmostEqual(parse_cpu_time("1-00:00:02.00"), 86402.0)
        self.assertIsNone(parse_cpu_time("not-time"))

    def test_parse_process_cpu_rejects_missing_fields(self):
        self.assertIsNone(parse_process_cpu(b"123 R 2.0 10"))
        parsed = parse_process_cpu(b"123 R 2.0 10 20 00:01 00:00.05 obscura serve\n")
        self.assertEqual(parsed["pid"], 123)
        self.assertEqual(parsed["state"], "R")
        self.assertEqual(parsed["cpuPercent"], 2.0)
        self.assertEqual(parsed["cpuTime"], "00:00.05")

    def test_parse_thread_count_keeps_non_utf8_losslessly(self):
        raw = b"USER PID THRD ST\nroot 123 1 S\nroot 123 2 S\n\xff\n"
        self.assertEqual(parse_darwin_thread_count(raw), 3)

    def test_parse_lsof_target_tcp_requires_exact_listener(self):
        raw = (
            b"COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n"
            b"obscura 1 me 7u IPv4 0 0 0 TCP 127.0.0.1:3000 (LISTEN)\n"
            b"obscura 1 me 8u IPv4 0 0 0 TCP 127.0.0.1:3000->127.0.0.1:4000 (ESTABLISHED)\n"
            b"obscura 1 me 9u IPv4 0 0 0 TCP 127.0.0.1:30000->127.0.0.1:4001 (ESTABLISHED)\n"
        )
        parsed = parse_lsof_target_tcp(raw, "127.0.0.1", 3000)
        self.assertEqual(parsed["listenerCount"], 1)
        self.assertEqual(parsed["establishedCount"], 1)
        self.assertEqual(len(parsed["rows"]), 2)

    def test_snapshot_validator_rejects_missing_authoritative_fields(self):
        failures = validate_snapshot(
            {
                "serverFdCount": None,
                "listenQueue": None,
                "process": None,
                "threads": {"threadCount": None},
                "targetTcp": {"listenerCount": 0, "establishedCount": None},
                "cpu": None,
            }
        )
        self.assertIn("server FD count missing", failures)
        self.assertIn("listen queue missing", failures)
        self.assertIn("process snapshot missing", failures)
        self.assertIn("thread count missing", failures)
        self.assertIn("target listener TCP row is missing or ambiguous", failures)
        self.assertIn("CPU sample missing", failures)

    def test_accepted_barrier_requires_zero_queue_exact_tcp_and_fd_delta(self):
        baseline = {"serverFdCount": 10}
        snapshot = {
            "serverFdCount": 265,
            "listenQueue": {"completed": 0},
            "process": {"pid": 1},
            "threads": {"threadCount": 4},
            "targetTcp": {"listenerCount": 1, "establishedCount": 255},
            "cpu": {"cpuPercent": 0.1},
        }
        failures = validate_accepted_barrier(baseline, snapshot, 256)
        self.assertTrue(any("ESTABLISHED" in failure for failure in failures))
        self.assertTrue(any("FD delta" in failure for failure in failures))

    def test_validate_http_requires_complete_body_and_reason(self):
        response = parse_http_response(
            b"HTTP/1.1 503 Service Unavailable\r\n"
            b"Content-Length: 0\r\nConnection: close\r\n"
            b"X-Obscura-Reason: max-pending-request-heads\r\n\r\n"
        )
        result = {"error": None, "recvError": None, "eof": True, "parsedResponse": response}
        self.assertEqual(validate_http(result, 503, PENDING_LIMIT_REASON), [])
        self.assertTrue(validate_http(result, 408, PENDING_TIMEOUT_REASON))

    def test_validate_http_rejects_timeout_or_reset_after_complete_head(self):
        response = parse_http_response(
            b"HTTP/1.1 503 Service Unavailable\r\n"
            b"Content-Length: 0\r\nConnection: close\r\n"
            b"X-Obscura-Reason: max-pending-request-heads\r\n\r\n"
        )
        timed_out = {"recvError": "TimeoutError()", "eof": False, "parsedResponse": response}
        reset = {"recvError": "ConnectionResetError()", "eof": False, "parsedResponse": response}
        self.assertTrue(validate_http(timed_out, 503, PENDING_LIMIT_REASON))
        self.assertTrue(validate_http(reset, 503, PENDING_LIMIT_REASON))

    def test_socket_group_rejects_missing_connections_and_results(self):
        failures = validate_socket_group("classification", [{"connected": False}], [], [], 1)
        self.assertTrue(any("connected count" in failure for failure in failures))
        self.assertTrue(any("result count" in failure for failure in failures))

    def test_recovery_requires_queue_tcp_fd_and_thread_baseline(self):
        baseline = {
            "serverFdCount": 10,
            "threads": {"threadCount": 4},
        }
        snapshot = {
            "serverFdCount": 11,
            "listenQueue": {"completed": 1},
            "process": {"pid": 1},
            "threads": {"threadCount": 5},
            "targetTcp": {"listenerCount": 1, "establishedCount": 1},
            "cpu": {"cpuPercent": 0.1},
        }
        failures = validate_recovery(baseline, snapshot, "recovery")
        self.assertTrue(any("queue" in failure for failure in failures))
        self.assertTrue(any("ESTABLISHED" in failure for failure in failures))
        self.assertTrue(any("FD count" in failure for failure in failures))
        self.assertTrue(any("thread count" in failure for failure in failures))

    def test_ttl_uses_first_byte_not_late_eof(self):
        result = {
            "connection": {"connectStartedMonotonicNs": 0, "connectedMonotonicNs": 1_000_000_000},
            "firstObservedMonotonicNs": 9_500_000_000,
            "readFinishedMonotonicNs": 20_000_000_000,
        }
        failures = validate_ttl_timing(result, "partial", 2.0)
        self.assertTrue(any("before production TTL" in failure for failure in failures))

    def test_ttl_accepts_first_eof_inside_observation_window(self):
        result = {
            "connection": {"connectStartedMonotonicNs": 0, "connectedMonotonicNs": 1_000_000_000},
            "firstObservedMonotonicNs": 10_500_000_000,
            "readFinishedMonotonicNs": 20_000_000_000,
        }
        self.assertEqual(validate_ttl_timing(result, "zero-byte", 2.0), [])

    def test_ttl_rejects_first_observation_past_connected_upper_bound(self):
        result = {
            "connection": {"connectStartedMonotonicNs": 0, "connectedMonotonicNs": 0},
            "firstObservedMonotonicNs": 12_300_000_000,
            "readFinishedMonotonicNs": 12_400_000_000,
        }
        failures = validate_ttl_timing(result, "zero-byte", 2.0)
        self.assertTrue(any("exceeded TTL plus grace" in failure for failure in failures))

    def test_read_wire_timed_records_first_eof(self):
        client, peer = socket.socketpair()
        try:
            peer.shutdown(socket.SHUT_WR)
            wire, error, _, eof, started_ns, first_ns, finished_ns = read_wire_timed(client, 0.5)
            self.assertEqual(wire, b"")
            self.assertIsNone(error)
            self.assertTrue(eof)
            self.assertIsNotNone(first_ns)
            self.assertGreaterEqual(first_ns, started_ns)
            self.assertLessEqual(first_ns, finished_ns)
        finally:
            client.close()
            peer.close()

    def test_wait_snapshot_timeout_is_explicit(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            with mock.patch.object(silent_pending, "capture_snapshot", return_value={}):
                with self.assertRaises(TimeoutError):
                    wait_snapshot(evidence, "timeout", 1, "127.0.0.1", 1, lambda _: False, 0.01, 0.001)

    def test_run_reaches_popen_and_writes_failure_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "obscura"
            binary.write_bytes(b"release-binary")
            args = argparse.Namespace(
                binary=binary,
                output=Path(temporary) / "evidence",
                host="127.0.0.1",
                port=43210,
                persona="windows_chrome145",
                workers=1,
                max_connections=16,
                pending=256,
                normal_probes=8,
                ttl_grace_seconds=2.0,
                sample_window_seconds=0.01,
                timeout=0.01,
                shutdown_timeout=0.01,
            )
            process = mock.Mock()
            process.pid = 43211
            process.returncode = None
            process.poll.return_value = None

            def wait_process(timeout):
                process.returncode = 0
                return 0

            process.wait.side_effect = wait_process
            uname = mock.MagicMock(
                system="Darwin",
                node="test-host",
                release="release",
                version="version",
                machine="arm64",
                processor="arm",
            )
            uname.__iter__.return_value = iter(
                ["Darwin", "test-host", "release", "version", "arm64", "arm"]
            )
            with (
                mock.patch.object(silent_pending.platform, "system", return_value="Darwin"),
                mock.patch.object(silent_pending.platform, "uname", return_value=uname),
                mock.patch.object(silent_pending.subprocess, "Popen", return_value=process) as popen,
                mock.patch.object(silent_pending.os, "getpgid", return_value=43211),
                mock.patch.object(silent_pending.os, "killpg"),
                mock.patch.object(silent_pending, "wait_ready", side_effect=RuntimeError("readiness")),
            ):
                manifest, exit_code = silent_pending.run(args)

            self.assertEqual(exit_code, 1)
            popen.assert_called_once()
            self.assertEqual(manifest["status"], "failed")
            self.assertTrue((args.output / "manifest.json").exists())

    def test_validate_close_requires_clean_eof(self):
        self.assertEqual(validate_close_result({"recvError": None, "eof": True}), [])
        self.assertTrue(validate_close_result({"recvError": "timeout", "eof": False}))

    def test_cpu_window_reports_facts_without_inventing_a_limit(self):
        samples = [
            {"process": {"cpuTime": "00:00.10", "cpuPercent": 1.0}},
            {"process": {"cpuTime": "00:00.20", "cpuPercent": 2.0}},
        ]
        summary = cpu_window(samples)
        self.assertEqual(summary["sampleCount"], 2)
        self.assertAlmostEqual(summary["cpuTimeDeltaSeconds"], 0.1)
        self.assertEqual(summary["cpuPercentMax"], 2.0)
        self.assertNotIn("maxAllowedCpu", summary)

    def test_record_http_keeps_partial_wire_and_result_paths_unique(self):
        client, peer = socket.socketpair()
        try:
            with tempfile.TemporaryDirectory() as temporary:
                evidence = Evidence(Path(temporary) / "evidence")
                peer.sendall(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 2\r\n\r\nO")
                peer.shutdown(socket.SHUT_WR)
                result = record_http(evidence, "partial", client, b"GET /x HTTP/1.1\r\n\r\n", 0.5)
                response = evidence.root / result["response"]["path"]
                self.assertEqual(response.read_bytes(), b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 2\r\n\r\nO")
                self.assertTrue((evidence.root / result["result"]["path"]).exists())
        finally:
            client.close()
            peer.close()

    def test_evidence_uses_exclusive_unique_paths(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            first = evidence.write_bytes("clients/one.request.bin", b"one")
            second = evidence.write_bytes("clients/two.request.bin", b"two")
            self.assertNotEqual(first, second)
            self.assertEqual((evidence.root / first).read_bytes(), b"one")
            self.assertEqual((evidence.root / second).read_bytes(), b"two")
            with self.assertRaises(FileExistsError):
                evidence.write_bytes(first, b"overwrite")

    def test_open_manifest_keeps_empty_and_binary_request_artifacts(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            result = {"index": 3, "connected": True, "localAddress": ["127.0.0.1", 1]}
            persist_open_results(evidence, "binary", [result], b"\x00\xff")
            request = evidence.root / result["request"]["path"]
            manifest_result = evidence.root / result["result"]["path"]
            self.assertEqual(request.read_bytes(), b"\x00\xff")
            self.assertTrue(manifest_result.exists())
            self.assertEqual(result["request"]["bytes"], 2)
            failed = {
                "index": 4,
                "connected": False,
                "connectError": "ConnectionRefusedError()",
                "connectTraceback": "raw connect traceback",
            }
            persist_open_results(evidence, "failed", [failed])
            failed_result = evidence.root / failed["result"]["path"]
            failed_traceback = evidence.root / failed["connectTracebackArtifact"]["path"]
            self.assertTrue(failed_result.exists())
            self.assertEqual(failed_traceback.read_text(), "raw connect traceback")


if __name__ == "__main__":
    unittest.main()

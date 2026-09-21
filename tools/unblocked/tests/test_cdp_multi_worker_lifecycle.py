import argparse
import socket
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.unblocked.cdp_multi_worker_lifecycle import (
    apply_cleanup_failures,
    build_command,
    discovery_request,
    parse_process_table,
    parse_ready_records,
    pid_exists,
    socket_exchange,
    validate_ready_records,
)


class LifecycleTests(unittest.TestCase):
    def test_ready_records_are_exact_and_complete(self):
        raw = (
            b"banner\n"
            b'{"protocol":"obscura-multi-worker-control","version":1,"event":"ready","worker":1,"port":9223,"pid":101}\n'
            b'{"protocol":"obscura-multi-worker-control","version":1,"event":"ready","worker":2,"port":9224,"pid":102}\n'
        )
        records, invalid = parse_ready_records(raw)
        self.assertEqual(invalid, [])
        self.assertEqual(validate_ready_records(records, 9222, 2), [])

    def test_ready_validation_rejects_duplicate_pid_and_extra_field(self):
        records = [
            {"protocol": "obscura-multi-worker-control", "version": 1, "event": "ready", "worker": 1, "port": 9223, "pid": 101},
            {"protocol": "obscura-multi-worker-control", "version": 1, "event": "ready", "worker": 2, "port": 9224, "pid": 101, "extra": True},
        ]
        failures = validate_ready_records(records, 9222, 2)
        self.assertTrue(any("duplicate readiness pid" in failure for failure in failures))
        self.assertTrue(any("unexpected readiness fields" in failure for failure in failures))

    def test_process_table_retains_commands_and_relationships(self):
        raw = (
            b" 100 1 100 S /tmp/obscura serve --workers 2\n"
            b" 101 100 100 R /tmp/obscura serve --supervised-worker 1\n"
        )
        rows = parse_process_table(raw)
        self.assertEqual(rows[1]["pid"], 101)
        self.assertEqual(rows[1]["ppid"], 100)
        self.assertIn("--supervised-worker 1", rows[1]["command"])
        self.assertEqual(rows[1]["raw"], " 101 100 100 R /tmp/obscura serve --supervised-worker 1")

    def test_parameter_command_keeps_secrets_out_of_argv(self):
        args = argparse.Namespace(
            binary=Path("/tmp/obscura"), persona="windows_chrome145",
            host="127.0.0.1", workers=2, max_connections=3,
        )
        command, environment, metadata = build_command(
            args, 9222, parameter_projection=True, font_dir=Path("/complete/fonts")
        )
        argv = " ".join(command)
        self.assertIn("--allow-private-network", command)
        self.assertIn("--allow-file-access", command)
        self.assertIn("/complete/fonts", command)
        self.assertNotIn(metadata["token"], argv)
        self.assertNotIn(metadata["proxy"], argv)
        self.assertEqual(environment["OBSCURA_CDP_TOKEN"], metadata["token"])
        self.assertEqual(environment["OBSCURA_PROXY"], metadata["proxy"])

    def test_discovery_exchange_preserves_complete_bytes(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        port = listener.getsockname()[1]
        observed = []

        def serve():
            connection, _ = listener.accept()
            observed.append(connection.recv(4096))
            connection.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
            connection.close()
            listener.close()

        thread = threading.Thread(target=serve)
        thread.start()
        request = discovery_request("qualification.test:9222", "complete-token", "https://origin.example")
        response, error = socket_exchange("127.0.0.1", port, request, 2.0)
        thread.join()
        self.assertIsNone(error)
        self.assertEqual(observed, [request])
        self.assertTrue(response.endswith(b"{}"))
        self.assertIn(b"Authorization: Bearer complete-token\r\n", request)

    def test_exchange_retains_partial_response_when_receive_times_out(self):
        listener = socket.socket()
        listener.bind(("127.0.0.1", 0))
        listener.listen(1)
        port = listener.getsockname()[1]
        partial = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nab"

        def serve():
            connection, _ = listener.accept()
            connection.recv(4096)
            connection.sendall(partial)
            threading.Event().wait(0.15)
            connection.close()
            listener.close()

        thread = threading.Thread(target=serve)
        thread.start()
        response, error = socket_exchange(
            "127.0.0.1", port, discovery_request("127.0.0.1:9222"), 0.03
        )
        thread.join()
        self.assertEqual(response, partial)
        self.assertIsNotNone(error)

    def test_cleanup_failures_are_promoted_to_scenario_failures(self):
        result = {
            "failures": [],
            "cleanup": {
                "errors": ["complete cleanup error"],
                "groupGone": False,
                "returncode": None,
                "forcedTerm": True,
                "forcedKill": True,
                "serverStreamsStable": False,
            },
        }
        apply_cleanup_failures(result)
        combined = "\n".join(result["failures"])
        self.assertIn("complete cleanup error", combined)
        self.assertIn("process-group members alive", combined)
        self.assertIn("did not reap", combined)
        self.assertIn("fallback process-group cleanup", combined)
        self.assertIn("streams could not be finalized", combined)

    def test_pid_exists_reports_missing_process(self):
        with patch("tools.unblocked.cdp_multi_worker_lifecycle.os.kill", side_effect=ProcessLookupError):
            self.assertFalse(pid_exists(123))


if __name__ == "__main__":
    unittest.main()

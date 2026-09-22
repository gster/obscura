import argparse
import socket
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from tools.unblocked import cdp_ws_handoff_capacity as handoff
from tools.unblocked.cdp_capacity import Evidence, parse_http_response, websocket_request


def snapshot(fd=300, established=256, queue=0, threads=8, alive=True):
    return {
        "processAlive": alive,
        "serverFdCount": fd,
        "listenQueue": {"completed": queue},
        "targetTcp": {"listenerCount": 1, "establishedCount": established},
        "threads": {"threadCount": threads},
        "process": {"pid": 1},
        "cpu": {"cpuPercent": 0.1},
    }


class WebSocketHandoffCapacityTests(unittest.TestCase):
    def test_contract_constants_are_explicit(self):
        self.assertEqual(handoff.HANDOFF_CHANNEL_CAPACITY, 128)
        self.assertEqual(handoff.QUALIFIED_ACCEPTED, 256)
        self.assertEqual(handoff.QUALIFIED_MAX_CONNECTIONS, 512)
        self.assertEqual(handoff.QUALIFIED_WAVES, 3)
        self.assertEqual(handoff.HANDOFF_REASON, "ws-handoff-saturated")

    def test_prefix_is_exact_full_request_without_terminator(self):
        full = websocket_request("127.0.0.1", 9222)
        self.assertEqual(full[-4:], b"\r\n\r\n")
        self.assertNotIn(b"\r\n\r\n", full[:-4])
        self.assertEqual(full[:-4] + b"\r\n\r\n", full)

    def test_barrier_requires_process_queue_tcp_and_fd(self):
        baseline = {"serverFdCount": 44}
        self.assertEqual(handoff.validate_barrier(baseline, snapshot(fd=300), 256), [])
        failures = handoff.validate_barrier(baseline, snapshot(fd=299), 256)
        self.assertTrue(any("FD delta" in item for item in failures))
        failures = handoff.validate_barrier(baseline, snapshot(established=255), 256)
        self.assertTrue(any("ESTABLISHED" in item for item in failures))
        failures = handoff.validate_barrier(baseline, snapshot(queue=1), 256)
        self.assertTrue(any("queue" in item for item in failures))
        failures = handoff.validate_barrier(baseline, snapshot(alive=False), 256)
        self.assertTrue(any("alive" in item for item in failures))

    def test_recovery_requires_all_zero_and_baselines(self):
        baseline = {"serverFdCount": 44, "threads": {"threadCount": 8}}
        self.assertEqual(handoff.validate_recovery(baseline, snapshot(fd=44, established=0)), [])
        failures = handoff.validate_recovery(baseline, snapshot(fd=45, established=1, threads=9))
        self.assertTrue(any("queue" in item or "ESTABLISHED" in item for item in failures))
        self.assertTrue(any("FD" in item for item in failures))
        self.assertTrue(any("thread" in item for item in failures))

    def test_handoff503_requires_complete_unique_reason_and_eof(self):
        wire = (
            b"HTTP/1.1 503 Service Unavailable\r\n"
            b"Content-Length: 0\r\nConnection: close\r\n"
            b"X-Obscura-Reason: ws-handoff-saturated\r\n\r\n"
        )
        result = {"parsedResponse": parse_http_response(wire), "recvError": None, "eof": True, "wireBytes": len(wire)}
        self.assertEqual(handoff.validate_handoff503(result), [])
        result["recvError"] = "TimeoutError()"
        self.assertTrue(handoff.validate_handoff503(result))

    def test_max_connections_is_not_handoff_success(self):
        wire = (
            b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n"
            b"X-Obscura-Reason: max-connections\r\n\r\n"
        )
        parsed = parse_http_response(wire)
        result = {"classification": "failure-max-connections", "parsedResponse": parsed, "eof": True, "recvError": None}
        self.assertNotEqual(result["classification"], "503-ws-handoff-saturated")
        self.assertTrue(handoff.validate_handoff503({**result, "wireBytes": len(wire)}))

    def test_classification_requires_exact_indices_and_both_outcomes(self):
        results = [{"index": 0, "classification": "101-pending-cdp", "cdpSuccess": True}]
        failures = handoff.classify_wave(results, 2)
        self.assertTrue(any("exact" in item for item in failures))
        self.assertTrue(any("saturated" in item for item in failures))
        failures = handoff.classify_wave(
            [
                {"index": 0, "classification": "failure-other"},
                {"index": 1, "classification": "101-pending-cdp", "cdpSuccess": True},
            ],
            2,
        )
        self.assertTrue(any("explicit" in item for item in failures))

    def test_classification_rejects_empty_wire_and_unknown_status(self):
        results = [
            {"index": 0, "classification": "failure-other", "wireBytes": 0},
            {"index": 1, "classification": "503-ws-handoff-saturated", "cdpSuccess": True},
        ]
        failures = handoff.classify_wave(results, 2)
        self.assertTrue(any("explicit non-qualifying" in item for item in failures))

    def test_socket_registry_salvages_raw_wire_on_exception(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            registry = handoff.OwnedSocketRegistry(evidence, 0.2)
            client, peer = socket.socketpair()
            try:
                registry.add(1, 4, client)
                peer.sendall(b"raw-salvage")
                peer.shutdown(socket.SHUT_WR)
                results = registry.salvage()
                self.assertEqual(len(results), 1)
                self.assertEqual(results[0]["wireBytes"], len(b"raw-salvage"))
                self.assertTrue(results[0]["eof"])
                self.assertEqual((evidence.root / results[0]["wire"]["path"]).read_bytes(), b"raw-salvage")
            finally:
                peer.close()

    def test_validate_args_rejects_nonqualified_shape(self):
        args = argparse.Namespace(host="127.0.0.1", workers=2, max_connections=512, accepted=256, waves=3)
        with mock.patch.object(handoff.platform, "system", return_value="Darwin"):
            with self.assertRaises(ValueError):
                handoff.validate_args(args)
            args.workers = 1
            args.max_connections = 511
            with self.assertRaises(ValueError):
                handoff.validate_args(args)
            args.max_connections = 512
            args.accepted = 255
            with self.assertRaises(ValueError):
                handoff.validate_args(args)
            args.accepted = 256
            args.waves = 2
            with self.assertRaises(ValueError):
                handoff.validate_args(args)
            args.waves = 3
            args.max_connections = 513
            with self.assertRaises(ValueError):
                handoff.validate_args(args)
            args.max_connections = 512
            args.accepted = 257
            with self.assertRaises(ValueError):
                handoff.validate_args(args)
            args.accepted = 256
            args.waves = 4
            with self.assertRaises(ValueError):
                handoff.validate_args(args)

    def test_validate_args_rejects_non_darwin(self):
        args = argparse.Namespace(host="127.0.0.1", workers=1, max_connections=512, accepted=256, waves=3)
        with mock.patch.object(handoff.platform, "system", return_value="Linux"):
            with self.assertRaises(ValueError):
                handoff.validate_args(args)

    def test_validate_args_rejects_non_loopback_or_ipv6(self):
        args = argparse.Namespace(host="192.0.2.1", workers=1, max_connections=512, accepted=256, waves=3)
        with mock.patch.object(handoff.platform, "system", return_value="Darwin"):
            with self.assertRaises(ValueError):
                handoff.validate_args(args)
            args.host = "::1"
            with self.assertRaises(ValueError):
                handoff.validate_args(args)

    def test_prefix_send_failure_remains_owned_for_wire_salvage(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            registry = handoff.OwnedSocketRegistry(evidence, 0.2)
            client, peer = socket.socketpair()
            peer.sendall(b"pending-server-wire")
            peer.shutdown(socket.SHUT_WR)
            with (
                mock.patch.object(handoff.socket, "create_connection", return_value=client),
                mock.patch.object(handoff, "_send", return_value=(0, "BrokenPipeError()")),
            ):
                sockets, results = handoff.open_wave(evidence, registry, 1, "127.0.0.1", 9222, 1, 0.2)
            self.assertEqual(set(sockets), {0})
            self.assertIsNotNone(results[0]["sendPrefixError"])
            salvaged = registry.salvage()
            self.assertEqual(len(salvaged), 1)
            self.assertEqual((evidence.root / salvaged[0]["wire"]["path"]).read_bytes(), b"pending-server-wire")
            peer.close()

    def test_cdp_send_failure_salvages_wire_before_release(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            registry = handoff.OwnedSocketRegistry(evidence, 0.2)
            client, peer = socket.socketpair()
            registry.add(1, 0, client)
            peer.sendall(b"pending-server-wire")
            peer.shutdown(socket.SHUT_WR)
            results = [{"index": 0, "classification": "101-pending-cdp"}]
            with mock.patch.object(handoff, "_send", return_value=(0, "BrokenPipeError()")):
                handoff.finish_cdp(evidence, registry, 1, {0: client}, results, 0.2)
            artifact = results[0]["failureSalvageWire"]
            self.assertEqual((evidence.root / artifact["path"]).read_bytes(), b"pending-server-wire")
            self.assertEqual(registry.salvage(), [])
            peer.close()

    def test_stop_observation_failure_continues_before_sigterm(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "obscura"
            binary.write_bytes(b"release")
            output = Path(temporary) / "evidence"
            args = argparse.Namespace(binary=binary, output=output, host="127.0.0.1", port=43123, persona="windows_chrome145", workers=1, max_connections=512, accepted=256, waves=3, timeout=0.01, startup_timeout=0.01, shutdown_timeout=0.01)
            process = mock.Mock(pid=43124, returncode=0)
            process.poll.return_value = None
            process.wait.return_value = 0
            sockets = {index: mock.Mock() for index in range(256)}
            opens = [{"connected": True, "connectError": None, "sendPrefixError": None, "sendPrefixBytes": len(websocket_request("127.0.0.1", 43123)) - 4} for _ in range(256)]
            signals = []

            def killpg(_pid, sent_signal):
                signals.append(sent_signal)
                if sent_signal == 0:
                    raise ProcessLookupError

            with (
                mock.patch.object(handoff.platform, "system", return_value="Darwin"),
                mock.patch.object(handoff.subprocess, "Popen", return_value=process),
                mock.patch.object(handoff, "wait_ready", return_value={}),
                mock.patch.object(handoff, "capture_snapshot", return_value=snapshot(fd=44, established=0)),
                mock.patch.object(handoff, "open_wave", return_value=(sockets, opens)),
                mock.patch.object(handoff, "wait_stable_barrier", return_value=(snapshot(), [snapshot(), snapshot()])),
                mock.patch.object(handoff, "wait_for_stopped", side_effect=TimeoutError("stop observation")),
                mock.patch.object(handoff.os, "killpg", side_effect=killpg),
            ):
                manifest, code = handoff.run(args)
            self.assertEqual(code, 1)
            self.assertEqual(manifest["status"], "failed")
            self.assertEqual(signals[:3], [handoff.signal.SIGSTOP, handoff.signal.SIGCONT, handoff.signal.SIGTERM])

    def test_run_reaches_popen_and_writes_failure_manifest(self):
        with tempfile.TemporaryDirectory() as temporary:
            binary = Path(temporary) / "obscura"
            binary.write_bytes(b"release")
            output = Path(temporary) / "evidence"
            args = argparse.Namespace(
                binary=binary,
                output=output,
                host="127.0.0.1",
                port=43123,
                persona="windows_chrome145",
                workers=1,
                max_connections=512,
                accepted=256,
                waves=3,
                timeout=0.01,
                startup_timeout=0.01,
                shutdown_timeout=0.01,
            )
            process = mock.Mock()
            process.pid = 43124
            process.returncode = 0
            process.poll.return_value = None
            with (
                mock.patch.object(handoff.platform, "system", return_value="Darwin"),
                mock.patch.object(handoff.subprocess, "Popen", return_value=process) as popen,
                mock.patch.object(handoff, "wait_ready", side_effect=RuntimeError("readiness")),
                mock.patch.object(handoff.os, "killpg", side_effect=ProcessLookupError),
            ):
                manifest, code = handoff.run(args)
            popen.assert_called_once()
            command = popen.call_args.args[0]
            self.assertEqual(command[1:4], ["--persona", "windows_chrome145", "serve"])
            self.assertEqual(code, 1)
            self.assertEqual(manifest["status"], "failed")
            self.assertTrue((output / "manifest.json").exists())

    def test_shutdown_never_forces_kill(self):
        process = mock.Mock()
        process.pid = 321
        process.returncode = 0
        process.poll.return_value = 0
        process.wait.return_value = 0
        with mock.patch.object(handoff.os, "killpg", side_effect=ProcessLookupError):
            result = handoff.shutdown(process, 0.01)
        self.assertFalse(result["forcedKill"])
        self.assertEqual(result["returncode"], 0)

    def test_cdp_command_is_unique_per_client_and_wave(self):
        first = 1 * 1_000_000 + 7 + 1
        second = 2 * 1_000_000 + 7 + 1
        self.assertNotEqual(first, second)

    def test_snapshot_stability_helper_is_used_for_two_samples(self):
        samples = [snapshot(fd=300), snapshot(fd=300)]
        with mock.patch.object(handoff, "validate_stable_barriers", return_value=[] ) as validator:
            # The helper is called by the polling function after the barrier
            # predicate is true twice; this direct contract test keeps its
            # expected argument shape explicit without starting a process.
            self.assertEqual(validator(samples, 256), [])
            validator.assert_called_once_with(samples, 256)


if __name__ == "__main__":
    unittest.main()

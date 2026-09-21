import concurrent.futures
import errno
import json
import socket
import tempfile
import threading
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.unblocked.cdp_capacity import (
    Evidence,
    drain_backlog_connections,
    discovery_request,
    finalize_backlog_connections,
    masked_websocket_close_frame,
    masked_websocket_frame,
    open_backlog_connections,
    parse_darwin_listen_queue,
    parse_http_response,
    parse_linux_listen_queue,
    parse_lsof_fd_count,
    parse_ps_usage,
    validate,
)


class CdpCapacityTests(unittest.TestCase):
    def test_discovery_request_is_complete_and_exact(self):
        self.assertEqual(
            discovery_request("127.0.0.1", 9222),
            b"GET /json/version HTTP/1.1\r\n"
            b"Host: 127.0.0.1:9222\r\n"
            b"Connection: close\r\n\r\n",
        )

    def test_http_parser_retains_duplicate_and_binary_header_values(self):
        body = b'{"ready":true}'
        response = (
            b"HTTP/1.1 200 OK\r\n"
            b"X-Raw: \xff\r\n"
            b"X-Raw: second\r\n"
            b"Content-Length: 14\r\n\r\n"
            + body
        )
        parsed = parse_http_response(response)
        self.assertEqual(parsed["status"], 200)
        self.assertEqual(
            parsed["headers"],
            [["X-Raw", "ÿ"], ["X-Raw", "second"], ["Content-Length", "14"]],
        )
        self.assertTrue(parsed["contentLengthMatches"])
        self.assertEqual(parsed["json"], {"ready": True})
        self.assertIsNone(parsed["parseError"])

    def test_http_parser_reports_incomplete_body_without_changing_bytes(self):
        parsed = parse_http_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nabc"
        )
        self.assertEqual(parsed["bodyBytes"], 3)
        self.assertFalse(parsed["contentLengthMatches"])

    def test_websocket_close_frame_is_masked(self):
        self.assertEqual(
            masked_websocket_close_frame(),
            b"\x88\x82\x12\x34\x56\x78\x11\xdc",
        )
        self.assertEqual(
            masked_websocket_frame(b"ok", 0x1, b"\x01\x02\x03\x04"),
            b"\x81\x82\x01\x02\x03\x04\x6e\x69",
        )

    def test_queue_parsers_select_the_exact_listener(self):
        darwin = (
            b"Current listen queue sizes (qlen/incqlen/maxqlen)\n"
            b"Listen Local Address\n"
            b"0/0/128 127.0.0.1.9000\n"
            b"127/0/128 127.0.0.1.9222\n"
        )
        self.assertEqual(
            parse_darwin_listen_queue(darwin, "127.0.0.1", 9222),
            {"completed": 127, "incomplete": 0, "maximum": 128},
        )
        linux = (
            b"State Recv-Q Send-Q Local Address:Port Peer Address:Port Process\n"
            b"LISTEN 99 128 127.0.0.2:9222 0.0.0.0:* users:((\"x\",pid=2,fd=1))\n"
            b"LISTEN 88 128 127.0.0.1:9222 0.0.0.0:* users:((\"old\",pid=1,fd=1))\n"
            b"LISTEN 128 128 127.0.0.1:9222 0.0.0.0:* users:((\"obscura\",pid=2,fd=3))\n"
        )
        self.assertEqual(
            parse_linux_listen_queue(linux, "127.0.0.1", 9222, 2),
            {"completed": 128, "maximum": 128},
        )

    def test_lsof_fd_count_excludes_cwd_text_and_memory_rows(self):
        output = (
            b"COMMAND PID USER FD TYPE DEVICE SIZE/OFF NODE NAME\n"
            b"obscura 9 user cwd DIR 1,2 1 2 /tmp\n"
            b"obscura 9 user txt REG 1,2 1 2 /tmp/obscura\n"
            b"obscura 9 user 0r CHR 3,2 0t0 1 /dev/null\n"
            b"obscura 9 user 17u IPv4 1 0t0 TCP 127.0.0.1:9 (LISTEN)\n"
        )
        self.assertEqual(parse_lsof_fd_count(output), 2)

    def test_ps_parser_keeps_command_tail(self):
        self.assertEqual(
            parse_ps_usage(
                b"4310 100 4310 S 12345 67890 00:01 /path/obscura serve --port 9\n"
            ),
            {
                "pid": 4310,
                "ppid": 100,
                "processGroup": 4310,
                "state": "S",
                "rssKiB": 12345,
                "vszKiB": 67890,
                "elapsed": "00:01",
                "command": "/path/obscura serve --port 9",
            },
        )

    def test_validation_requires_real_pressure_and_full_recovery(self):
        full_response = {
            "status": 200,
            "completeHead": True,
            "contentLengthMatches": True,
            "parseError": None,
        }
        clients = [
            {
                "index": 0,
                "connected": True,
                "requestSent": True,
                "responseError": None,
                "parsedResponse": full_response,
            },
            {
                "index": 1,
                "connected": False,
                "requestSent": False,
                "connectErrorStage": "connect",
                "connectErrorType": "TimeoutError",
                "connectErrno": None,
                "responseError": None,
                "parsedResponse": {"status": None},
            },
        ]
        failures = validate(
            2,
            {"serverFdCount": 20},
            {"serverFdCount": 20, "listenQueue": {"completed": 1, "maximum": 1}},
            {"serverFdCount": 20, "listenQueue": {"completed": 0}},
            clients,
            {
                "error": None,
                "parsedResponse": full_response,
            },
            {
                "parsedResponse": {"status": 101},
                "error": None,
                "commandError": None,
                "commandJsonError": None,
                "serverCommandMetadata": {"fin": True, "opcode": 1},
                "serverCommandJson": {"id": 1, "result": {}},
                "closeError": None,
            },
        )
        self.assertEqual(failures, [])

        clients[1]["connected"] = True
        clients[1]["requestSent"] = True
        clients[1]["connectErrorStage"] = None
        clients[1]["connectErrorType"] = None
        clients[1]["parsedResponse"] = full_response
        failures = validate(
            2,
            {"serverFdCount": 20},
            {"serverFdCount": 20, "listenQueue": {"completed": 2, "maximum": 2}},
            {"serverFdCount": 20, "listenQueue": {"completed": 0}},
            clients,
            {"error": None, "parsedResponse": full_response},
            {
                "parsedResponse": {"status": 101},
                "error": None,
                "commandError": None,
                "commandJsonError": None,
                "serverCommandMetadata": {"fin": True, "opcode": 1},
                "serverCommandJson": {"id": 1, "result": {}},
                "closeError": None,
            },
        )
        self.assertTrue(any("did not exercise backlog capacity" in item for item in failures))
        self.assertTrue(any("no burst connection observed" in item for item in failures))

    def test_validation_rejects_local_failure_and_unsaturated_queue(self):
        full_response = {
            "status": 200,
            "completeHead": True,
            "contentLengthMatches": True,
            "parseError": None,
        }
        clients = [
            {
                "index": 0,
                "connected": True,
                "requestSent": True,
                "responseError": None,
                "parsedResponse": full_response,
            },
            {
                "index": 1,
                "connected": False,
                "requestSent": False,
                "connectErrorStage": "socket",
                "connectErrorType": "OSError",
                "connectErrno": errno.EMFILE,
            },
        ]
        failures = validate(
            2,
            {"serverFdCount": 20},
            {"serverFdCount": 20, "listenQueue": {"completed": 1, "maximum": 4096}},
            {"serverFdCount": 20, "listenQueue": {"completed": 0}},
            clients,
            {"error": None, "parsedResponse": full_response},
            {
                "parsedResponse": {"status": 101},
                "error": None,
                "commandError": None,
                "commandJsonError": None,
                "serverCommandMetadata": {"fin": True, "opcode": 1},
                "serverCommandJson": {"id": 1, "result": {}},
                "closeError": None,
            },
        )
        self.assertTrue(any("did not reach its reported capacity" in item for item in failures))
        self.assertTrue(any("non-backlog client failures" in item for item in failures))
        self.assertTrue(any("no burst connection observed" in item for item in failures))

    def test_incomplete_recovery_http_is_rejected(self):
        clients = [
            {
                "index": 0,
                "connected": True,
                "requestSent": True,
                "responseError": None,
                "parsedResponse": {
                    "status": 200,
                    "completeHead": True,
                    "contentLengthMatches": True,
                    "parseError": None,
                },
            },
            {
                "index": 1,
                "connected": False,
                "requestSent": False,
                "connectErrorStage": "connect",
                "connectErrorType": "TimeoutError",
                "connectErrno": None,
            },
        ]
        failures = validate(
            2,
            {"serverFdCount": 20},
            {"serverFdCount": 20, "listenQueue": {"completed": 1, "maximum": 1}},
            {"serverFdCount": 20, "listenQueue": {"completed": 0}},
            clients,
            {
                "error": "TimeoutError('timed out')",
                "parsedResponse": {
                    "status": 200,
                    "completeHead": True,
                    "contentLengthMatches": False,
                    "parseError": None,
                },
            },
            {
                "parsedResponse": {"status": 101},
                "error": None,
                "commandError": None,
                "commandJsonError": None,
                "serverCommandMetadata": {"fin": True, "opcode": 1},
                "serverCommandJson": {"id": 1, "result": {}},
                "closeError": None,
            },
        )
        self.assertTrue(any("HTTP discovery transport failed" in item for item in failures))

    def test_socket_creation_failure_is_bounded_and_persisted(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            with patch(
                "tools.unblocked.cdp_capacity.socket.socket",
                side_effect=OSError(errno.EMFILE, "too many open files"),
            ):
                connections, results = open_backlog_connections(
                    evidence, "127.0.0.1", 9, 3, 0.05
                )
            self.assertEqual(connections, [])
            self.assertEqual(len(results), 3)
            self.assertTrue(all(result["connectErrorStage"] == "socket" for result in results))
            self.assertTrue(all(result["connectErrno"] == errno.EMFILE for result in results))
            self.assertEqual(
                len(list((evidence.root / "backlog").glob("*.connect.result.json"))),
                3,
            )

    def test_thread_submission_failure_releases_started_workers(self):
        sent: list[bytes] = []
        sockets = []

        class FakeSocket:
            def __init__(self, *_args):
                self.closed = False
                sockets.append(self)

            def settimeout(self, _timeout):
                return None

            def connect(self, _address):
                return None

            def getsockname(self):
                return ("127.0.0.1", 40000 + len(sockets))

            def getpeername(self):
                return ("127.0.0.1", 9222)

            def sendall(self, payload):
                sent.append(payload)

            def close(self):
                self.closed = True

        real_start = threading.Thread.start
        start_count = 0

        def flaky_start(thread):
            nonlocal start_count
            start_count += 1
            if start_count == 2:
                raise RuntimeError("synthetic thread start failure after enqueue")
            return real_start(thread)

        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            with patch(
                "tools.unblocked.cdp_capacity.socket.socket", FakeSocket
            ), patch(
                "threading.Thread.start", flaky_start
            ):
                connections, results = open_backlog_connections(
                    evidence, "127.0.0.1", 9222, 3, 0.05
                )
            self.assertEqual(len(results), 3)
            by_index = {result["index"]: result for result in results}
            self.assertEqual(len(sent), 2)
            self.assertTrue(by_index[0]["requestSent"])
            self.assertTrue(by_index[1]["requestSent"])
            self.assertEqual(by_index[2]["connectErrorStage"], "thread-submit")
            self.assertEqual(len(connections), 2)
            finalize_backlog_connections(evidence, connections, results, "snapshot failed")
            self.assertTrue(all(connection.closed for connection in sockets))
            self.assertEqual(
                len(
                    [
                        path
                        for path in (evidence.root / "backlog").glob(
                            "client-*.result.json"
                        )
                        if ".connect." not in path.name
                    ]
                ),
                3,
            )

    def test_drain_collects_work_enqueued_by_raising_submit(self):
        response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}"

        class ResponseSocket:
            def __init__(self):
                self.chunks = [response, b""]
                self.closed = False

            def settimeout(self, _timeout):
                return None

            def recv(self, _size):
                return self.chunks.pop(0)

            def close(self):
                self.closed = True

        real_pool_class = concurrent.futures.ThreadPoolExecutor

        class PostEnqueueFailurePool:
            def __init__(self, max_workers):
                self.pool = real_pool_class(max_workers=max_workers)
                self.submissions = 0

            def submit(self, function, *arguments):
                self.submissions += 1
                future = self.pool.submit(function, *arguments)
                if self.submissions == 2:
                    raise RuntimeError("synthetic failure after enqueue")
                return future

            def shutdown(self, wait=True):
                return self.pool.shutdown(wait=wait)

        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            sockets = [ResponseSocket() for _ in range(3)]
            connections = list(enumerate(sockets))
            results = [
                {"index": index, "connected": True, "requestSent": True}
                for index in range(3)
            ]
            with patch(
                "tools.unblocked.cdp_capacity.concurrent.futures.ThreadPoolExecutor",
                PostEnqueueFailurePool,
            ):
                drain_backlog_connections(evidence, connections, results, 0.1)
            self.assertTrue(all(connection.closed for connection in sockets))
            self.assertTrue(all(result["responseError"] is None for result in results))
            self.assertTrue(
                all(result["parsedResponse"]["status"] == 200 for result in results)
            )
            for index in range(3):
                self.assertEqual(
                    (evidence.root / f"backlog/client-{index:04d}.response.bin").read_bytes(),
                    response,
                )

    def test_failure_finalizer_closes_socket_and_writes_empty_response(self):
        with tempfile.TemporaryDirectory() as temporary:
            evidence = Evidence(Path(temporary) / "evidence")
            connection, peer = socket.socketpair()
            try:
                result = {
                    "index": 0,
                    "connected": True,
                    "requestSent": True,
                    "request": {"path": "synthetic"},
                }
                finalize_backlog_connections(
                    evidence,
                    [(0, connection)],
                    [result],
                    "queued snapshot failed",
                )
                self.assertEqual(connection.fileno(), -1)
                self.assertEqual(result["responseError"], "queued snapshot failed")
                self.assertEqual(
                    (evidence.root / "backlog/client-0000.response.bin").read_bytes(),
                    b"",
                )
                persisted = json.loads(
                    (evidence.root / "backlog/client-0000.result.json").read_text()
                )
                self.assertEqual(persisted["responseError"], "queued snapshot failed")
            finally:
                peer.close()


if __name__ == "__main__":
    unittest.main()

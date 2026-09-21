import unittest
from unittest.mock import patch

from tools.unblocked.cdp_capacity import parse_http_response
from tools.unblocked.cdp_multi_worker import (
    header_value,
    overload_request,
    parse_parent_tcp_mappings,
    validate,
)


def http_result(status, body=b"", extra_headers=()):
    headers = [
        (b"Content-Length", str(len(body)).encode("ascii")),
        *extra_headers,
    ]
    raw = (
        f"HTTP/1.1 {status} Test\r\n".encode("ascii")
        + b"".join(name + b": " + value + b"\r\n" for name, value in headers)
        + b"\r\n"
        + body
    )
    return {
        "parsedResponse": parse_http_response(raw),
        "readError": None,
    }


class CdpMultiWorkerTests(unittest.TestCase):
    def test_overload_request_exceeds_header_limit_and_is_complete(self):
        request = overload_request("127.0.0.1", 9222)
        self.assertGreater(len(request), 4096)
        self.assertTrue(request.startswith(b"GET /json/version HTTP/1.1\r\n"))
        self.assertTrue(request.endswith(b"\r\n\r\n"))
        self.assertIn(b"Host: 127.0.0.1:9222\r\n", request)

    def test_darwin_mapping_parser_keeps_exact_parent_and_worker_links(self):
        output = (
            b"obscura 123 user 8u IPv4 x 0t0 TCP 127.0.0.1:40000->127.0.0.1:50123 (ESTABLISHED)\n"
            b"obscura 123 user 9u IPv4 x 0t0 TCP 127.0.0.1:50124->127.0.0.1:40001 (ESTABLISHED)\n"
            b"obscura 999 user 9u IPv4 x 0t0 TCP 127.0.0.1:40000->127.0.0.1:50125 (ESTABLISHED)\n"
        )
        with patch("tools.unblocked.cdp_multi_worker.platform.system", return_value="Darwin"):
            result = parse_parent_tcp_mappings(output, 123, 40000, {40001, 40002})
        self.assertEqual(result["acceptedClientPorts"], [50123])
        self.assertEqual(result["workerConnectionCount"], 1)
        self.assertEqual(result["workerConnections"][0]["remote"], "127.0.0.1:40001")

    def test_linux_mapping_parser_keeps_exact_parent_and_worker_links(self):
        output = (
            b'ESTAB 0 0 127.0.0.1:40000 127.0.0.1:50123 users:(("obscura",pid=123,fd=8))\n'
            b'ESTAB 0 0 127.0.0.1:50124 127.0.0.1:40002 users:(("obscura",pid=123,fd=9))\n'
            b'ESTAB 0 0 127.0.0.1:40000 127.0.0.1:50125 users:(("obscura",pid=999,fd=8))\n'
        )
        with patch("tools.unblocked.cdp_multi_worker.platform.system", return_value="Linux"):
            result = parse_parent_tcp_mappings(output, 123, 40000, {40001, 40002})
        self.assertEqual(result["acceptedClientPorts"], [50123])
        self.assertEqual(result["workerConnectionCount"], 1)
        self.assertEqual(result["workerConnections"][0]["remote"], "127.0.0.1:40002")

    def test_header_lookup_is_case_insensitive_and_retains_value(self):
        result = http_result(
            503,
            extra_headers=((b"X-Obscura-Reason", b"max-relays"),),
        )
        self.assertEqual(header_value(result, "x-obscura-reason"), "max-relays")

    def test_validate_accepts_complete_recovery_sequence(self):
        discovery = http_result(200, b"{}")
        overload = http_result(
            503,
            extra_headers=((b"X-Obscura-Reason", b"max-relays"),),
        )
        websocket = {
            "parsedResponse": {"status": 101},
            "error": None,
            "commandError": None,
            "commandJsonError": None,
            "serverCommandJson": {"id": 1, "result": {}},
            "closeError": None,
        }
        self.assertEqual(validate(discovery, overload, discovery, websocket), [])

    def test_validate_rejects_wrong_overload_reason_and_missing_cdp_reply(self):
        discovery = http_result(200, b"{}")
        overload = http_result(
            503,
            extra_headers=((b"X-Obscura-Reason", b"worker-unreachable"),),
        )
        websocket = {
            "parsedResponse": {"status": 101},
            "error": None,
            "commandError": None,
            "commandJsonError": None,
            "serverCommandJson": {"id": 9, "result": {}},
            "closeError": None,
        }
        failures = validate(discovery, overload, discovery, websocket)
        self.assertIn(
            "aggregate relay refusal omitted X-Obscura-Reason: max-relays",
            failures,
        )
        self.assertIn(
            "recovery WebSocket did not return Browser.getVersion id=1",
            failures,
        )


if __name__ == "__main__":
    unittest.main()

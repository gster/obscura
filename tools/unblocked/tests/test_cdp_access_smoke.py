import json
import socket
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.unblocked.cdp_access_smoke import exchange


class FailingConnection:
    def __enter__(self):
        return self

    def __exit__(self, *_args):
        return False

    def sendall(self, _request):
        return None

    def recv(self, _size):
        if not hasattr(self, "delivered"):
            self.delivered = True
            return b"HTTP/1.1 200 OK\r\nX-Raw: \xff\r\n"
        raise socket.timeout("synthetic timeout")


class CdpAccessSmokeTests(unittest.TestCase):
    def test_exchange_retains_partial_bytes_error_and_each_attempt(self):
        request = b"GET /json/version HTTP/1.1\r\nHost: test\r\n\r\n"
        partial_response = b"HTTP/1.1 200 OK\r\nX-Raw: \xff\r\n"
        with tempfile.TemporaryDirectory() as root:
            output = Path(root)
            for _ in range(2):
                with patch(
                    "tools.unblocked.cdp_access_smoke.socket.create_connection",
                    return_value=FailingConnection(),
                ):
                    with self.assertRaisesRegex(socket.timeout, "synthetic timeout"):
                        exchange(1, request, output, "readiness")

            for attempt in (1, 2):
                prefix = output / f"readiness.attempt-{attempt:03d}"
                self.assertEqual(
                    Path(f"{prefix}.request.bin").read_bytes(),
                    request,
                )
                self.assertEqual(
                    Path(f"{prefix}.response.bin").read_bytes(),
                    partial_response,
                )
                result = json.loads(Path(f"{prefix}.result.json").read_text())
                self.assertEqual(result["attempt"], attempt)
                self.assertEqual(result["status"], "error")
                self.assertIn("synthetic timeout", result["error"])
                self.assertEqual(result["requestBytes"], len(request))
                self.assertEqual(result["responseBytes"], len(partial_response))


if __name__ == "__main__":
    unittest.main()

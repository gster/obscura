import base64
import contextlib
import io
import socket
import unittest

from tools.unblocked.migration_cors import local_http_servers


class MigrationCorsCaptureTests(unittest.TestCase):
    def test_request_capture_keeps_wire_header_spacing_and_duplicates(self):
        with local_http_servers() as servers, contextlib.redirect_stdout(io.StringIO()):
            port = int(servers.origins[0].rsplit(":", 1)[1])
            request = (
                f"GET /wire HTTP/1.1\r\n"
                f"Host: 127.0.0.1:{port}\r\n"
                "X-Repeat: first\r\n"
                "X-Repeat:  second\r\n"
                "X-Spacing:\tvalue\r\n"
                "Content-Length: 4\r\n"
                "Connection: close\r\n"
                "\r\n"
                "DATA"
            ).encode("ascii")
            with socket.create_connection(("127.0.0.1", port), timeout=2) as client:
                client.sendall(request)
                client.shutdown(socket.SHUT_WR)
                while client.recv(4096):
                    pass

            self.assertEqual(len(servers.requests), 1)
            record = servers.requests[0]
            header_end = request.index(b"\r\n\r\n") + len(b"\r\n\r\n")
            self.assertEqual(
                base64.b64decode(record["rawHeadersBase64"]), request[:header_end]
            )
            self.assertEqual(base64.b64decode(record["bodyBase64"]), b"DATA")
            self.assertEqual(
                [item["value"] for item in record["headers"] if item["name"] == "X-Repeat"],
                ["first", "second"],
            )


if __name__ == "__main__":
    unittest.main()

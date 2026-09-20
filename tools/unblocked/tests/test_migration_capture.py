import base64
import contextlib
import io
import os
from pathlib import Path
import sys
import socket
import tempfile
import unittest

from tools.unblocked.migration_smoke import capture_worker
from tools.unblocked.migration_lifecycle import lifecycle_fixture_server


class MigrationCaptureTests(unittest.TestCase):
    def test_lifecycle_request_capture_preserves_wire_bytes(self):
        with contextlib.redirect_stdout(io.StringIO()), lifecycle_fixture_server() as (origin, state):
            port = int(origin.rsplit(":", 1)[1])
            head = (
                f"POST /effect?counter=post-dispatch HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n"
                "X-Repeat: first\r\nX-Repeat:\t second\r\n"
                "Content-Length: 4\r\nConnection: close\r\n\r\n"
            ).encode("ascii")
            body = b"\x00\xff\x01\x80"
            with socket.create_connection(("127.0.0.1", port), timeout=2) as client:
                client.sendall(head + body)
                client.shutdown(socket.SHUT_WR)
                while client.recv(4096):
                    pass
            record, = state.snapshot()["requests"]
            self.assertEqual(base64.b64decode(record["rawRequestHeadBase64"]), head)
            self.assertEqual(base64.b64decode(record["rawRequestBase64"]), head + body)
            self.assertEqual(base64.b64decode(record["bodyBase64"]), body)

    def test_failed_worker_retains_complete_binary_streams(self):
        payload = bytes(range(256)) * 4096
        program = (
            "import os\n"
            "for fd in (1,2):\n"
            " data=bytes(range(256))*4096\n"
            " while data: data=data[os.write(fd,data):]\n"
            "raise SystemExit(23)\n"
        )
        with tempfile.TemporaryDirectory() as root:
            result = capture_worker([sys.executable, "-c", program], Path(root), 10)
            self.assertEqual(result["status"], "failed")
            self.assertEqual(result["returncode"], 23)
            self.assertEqual(Path(result["stdoutPath"]).read_bytes(), payload)
            self.assertEqual(Path(result["stderrPath"]).read_bytes(), payload)

    def test_hard_deadline_is_failure_and_reaps_worker(self):
        program = "import os,time; os.write(1,b'entered\\x00worker'); time.sleep(60)"
        with tempfile.TemporaryDirectory() as root:
            result = capture_worker([sys.executable, "-c", program], Path(root), 2)
            self.assertEqual(result["status"], "hard-timeout")
            self.assertEqual(result["error"]["type"], "TimeoutExpired")
            self.assertIsNotNone(result["returncode"])
            self.assertEqual(Path(result["stdoutPath"]).read_bytes(), b"entered\x00worker")
            with self.assertRaises(ProcessLookupError):
                os.kill(result["pid"], 0)

import shutil
import sys
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.unblocked.cdp_fixture import external_process


class ProcessCaptureTests(unittest.TestCase):
    def test_captures_binary_output_larger_than_pipe_buffer(self):
        code = (
            "import os, time\n"
            "for fd, payload in ((1, bytes(range(256)) * 8192), "
            "(2, bytes(range(255, -1, -1)) * 8192)):\n"
            "    while payload:\n"
            "        payload = payload[os.write(fd, payload):]\n"
            "time.sleep(30)\n"
        )
        capture: dict[str, object] = {}
        with tempfile.TemporaryDirectory() as root:
            output_root = Path(root)
            with patch("tools.unblocked.cdp_fixture.wait_for_endpoint"):
                with external_process(
                    [sys.executable, "-c", code],
                    "http://127.0.0.1:1",
                    capture=capture,
                    log_root=output_root,
                ):
                    capture_dir = Path(str(capture["captureDir"]))
                    self.assertEqual(capture_dir.parent, output_root)
                    self.assertTrue(capture_dir.is_dir())
                    stdout_path = capture_dir / "stdout.bin"
                    stderr_path = capture_dir / "stderr.bin"
                    expected_size = 256 * 8192
                    deadline = time.monotonic() + 2
                    while (
                        (stdout_path.stat().st_size < expected_size
                         or stderr_path.stat().st_size < expected_size)
                        and time.monotonic() < deadline
                    ):
                        time.sleep(0.01)
            capture_dir = Path(str(capture["captureDir"]))
            self.assertEqual(
                (capture_dir / "stdout.bin").read_bytes(), bytes(range(256)) * 8192
            )
            self.assertEqual(
                (capture_dir / "stderr.bin").read_bytes(),
                bytes(range(255, -1, -1)) * 8192,
            )
            self.assertEqual(capture["state"], "stopped")
            self.assertIsInstance(capture["pid"], int)
            self.assertIsInstance(capture["returncode"], int)
            shutil.rmtree(capture_dir)

    def test_early_exit_retains_output_and_wait_error(self):
        code = (
            "import os; os.write(1,b'early\\x00out'); "
            "os.write(2,b'early\\xfferr'); os._exit(17)"
        )
        capture: dict[str, object] = {}
        with tempfile.TemporaryDirectory() as root:
            output_root = Path(root)
            with self.assertRaisesRegex(RuntimeError, "exited with status 17"):
                with external_process(
                    [sys.executable, "-c", code],
                    "http://127.0.0.1:1",
                    capture=capture,
                    log_root=output_root,
                ):
                    self.fail("wait_for_endpoint unexpectedly returned")
            capture_dir = Path(str(capture["captureDir"]))
            self.assertEqual((capture_dir / "stdout.bin").read_bytes(), b"early\x00out")
            self.assertEqual((capture_dir / "stderr.bin").read_bytes(), b"early\xfferr")
            self.assertEqual(capture["state"], "stopped")
            self.assertEqual(capture["returncode"], 17)
            self.assertEqual(capture["error"]["type"], "RuntimeError")  # type: ignore[index]
            shutil.rmtree(capture_dir)

    def test_spawn_failure_retains_paths_and_closes_files(self):
        capture: dict[str, object] = {}
        with tempfile.TemporaryDirectory() as root:
            output_root = Path(root)
            with self.assertRaises(FileNotFoundError):
                with external_process(
                    ["/definitely/missing/obscura-helper"],
                    "http://127.0.0.1:1",
                    capture=capture,
                    log_root=output_root,
                ):
                    self.fail("spawn unexpectedly succeeded")
            capture_dir = Path(str(capture["captureDir"]))
            self.assertEqual(capture["state"], "spawn-failed")
            self.assertEqual(capture["spawnError"]["type"], "FileNotFoundError")  # type: ignore[index]
            self.assertIsNone(capture["returncode"])
            self.assertEqual((capture_dir / "stdout.bin").read_bytes(), b"")
            self.assertEqual((capture_dir / "stderr.bin").read_bytes(), b"")
            (capture_dir / "stdout.bin").unlink()
            (capture_dir / "stderr.bin").unlink()
            capture_dir.rmdir()

    def test_cleanup_error_preserves_primary_failure_metadata(self):
        class FakeProcess:
            pid = 43123

            def poll(self):
                return None

        capture: dict[str, object] = {}
        with tempfile.TemporaryDirectory() as root:
            with patch(
                "tools.unblocked.cdp_fixture.subprocess.Popen",
                return_value=FakeProcess(),
            ), patch(
                "tools.unblocked.cdp_fixture.wait_for_endpoint",
                side_effect=RuntimeError("endpoint startup failed"),
            ), patch(
                "tools.unblocked.cdp_fixture.terminate_process_group",
                side_effect=RuntimeError("cleanup failed"),
            ):
                with self.assertRaisesRegex(RuntimeError, "endpoint startup failed"):
                    with external_process(
                        ["fake-browser"],
                        "http://127.0.0.1:1",
                        capture=capture,
                        log_root=Path(root),
                    ):
                        self.fail("wait_for_endpoint unexpectedly returned")

            capture_dir = Path(str(capture["captureDir"]))
            self.assertEqual(capture["state"], "cleanup-failed")
            self.assertEqual(capture["error"]["message"], "endpoint startup failed")  # type: ignore[index]
            self.assertEqual(capture["cleanupError"]["message"], "cleanup failed")  # type: ignore[index]
            (capture_dir / "stdout.bin").unlink()
            (capture_dir / "stderr.bin").unlink()
            capture_dir.rmdir()


if __name__ == "__main__":
    unittest.main()

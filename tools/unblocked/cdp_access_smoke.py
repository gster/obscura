#!/usr/bin/env python3
"""Official Playwright and multi-worker qualification for CDP admission."""

from __future__ import annotations

import argparse
import importlib.metadata
import json
import os
import random
import signal
import socket
import subprocess
import tempfile
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator


TOKEN = "ob034-playwright-token-0123456789abcdef"


def contiguous_ports(count: int) -> int:
    for _ in range(200):
        base = random.randint(40_000, 55_000)
        sockets: list[socket.socket] = []
        try:
            for port in range(base, base + count):
                listener = socket.socket()
                listener.bind(("127.0.0.1", port))
                sockets.append(listener)
            return base
        except OSError:
            pass
        finally:
            for listener in sockets:
                listener.close()
    raise RuntimeError("could not reserve a contiguous local port range")


def request_bytes(port: int, *, token: str | None, host: str | None = None,
                  origin: str | None = None) -> bytes:
    fields = [
        "GET /json/version HTTP/1.1",
        f"Host: {host or f'127.0.0.1:{port}'}",
    ]
    if token is not None:
        fields.append(f"Authorization: Bearer {token}")
    if origin is not None:
        fields.append(f"Origin: {origin}")
    fields.append("Connection: close")
    return ("\r\n".join(fields) + "\r\n\r\n").encode()


def exchange(port: int, request: bytes, root: Path, name: str) -> bytes:
    attempt = 1
    while (root / f"{name}.attempt-{attempt:03d}.result.json").exists():
        attempt += 1
    prefix = root / f"{name}.attempt-{attempt:03d}"
    Path(f"{prefix}.request.bin").write_bytes(request)
    chunks: list[bytes] = []
    failure: str | None = None
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=2) as connection:
            connection.sendall(request)
            while True:
                chunk = connection.recv(65_536)
                if not chunk:
                    break
                chunks.append(chunk)
    except BaseException as error:
        failure = repr(error)
        raise
    finally:
        response = b"".join(chunks)
        Path(f"{prefix}.response.bin").write_bytes(response)
        Path(f"{prefix}.result.json").write_text(
            json.dumps(
                {
                    "attempt": attempt,
                    "status": "error" if failure is not None else "completed",
                    "error": failure,
                    "requestBytes": len(request),
                    "responseBytes": len(response),
                },
                indent=2,
            )
            + "\n"
        )
    return b"".join(chunks)


def wait_ready(process: subprocess.Popen[bytes], port: int, root: Path) -> None:
    deadline = time.monotonic() + 15
    last_error = "not ready"
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited with status {process.returncode}")
        try:
            response = exchange(
                port,
                request_bytes(port, token=TOKEN),
                root,
                "readiness",
            )
            if response.startswith(b"HTTP/1.1 200"):
                return
            last_error = response.split(b"\r\n", 1)[0].decode("ascii", "replace")
        except OSError as error:
            last_error = repr(error)
        time.sleep(0.05)
    raise TimeoutError(f"CDP endpoint did not become ready: {last_error}")


@contextmanager
def server(command: list[str], port: int, root: Path) -> Iterator[subprocess.Popen[bytes]]:
    root.mkdir(parents=True, exist_ok=True)
    environment = os.environ.copy()
    environment["OBSCURA_CDP_TOKEN"] = TOKEN
    with (root / "stdout.bin").open("wb") as stdout, (root / "stderr.bin").open("wb") as stderr:
        process = subprocess.Popen(
            command,
            env=environment,
            stdin=subprocess.DEVNULL,
            stdout=stdout,
            stderr=stderr,
            start_new_session=True,
        )
        try:
            wait_ready(process, port, root)
            yield process
        finally:
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=5)


def require_status(response: bytes, status: int, name: str) -> None:
    expected = f"HTTP/1.1 {status}".encode()
    if not response.startswith(expected):
        raise AssertionError(
            f"{name}: expected {status}, got {response.split(bytes([13, 10]), 1)[0]!r}"
        )


def official_client(endpoint: str) -> dict[str, object]:
    from playwright.sync_api import sync_playwright

    with sync_playwright() as playwright:
        browser = playwright.chromium.connect_over_cdp(
            endpoint,
            headers={"Authorization": f"Bearer {TOKEN}"},
        )
        context = browser.contexts[0]
        page = context.pages[0] if context.pages else context.new_page()
        evaluated = page.evaluate(
            "({ready:true, href:location.href, userAgent:navigator.userAgent})"
        )
        result = {"browserVersion": browser.version, "evaluated": evaluated}
        browser.close()
        return result


def run(obscura: Path, output: Path) -> dict[str, object]:
    playwright_version = importlib.metadata.version("playwright")
    if playwright_version != "1.60.0":
        raise RuntimeError(f"expected Playwright 1.60.0, got {playwright_version}")
    output.mkdir(parents=True, exist_ok=True)
    result: dict[str, object] = {
        "schemaVersion": 1,
        "status": "running",
        "playwrightVersion": playwright_version,
        "token": TOKEN,
        "phases": {},
    }

    implicit_port = contiguous_ports(1)
    implicit_root = output / "implicit"
    implicit_command = [
        str(obscura.resolve()), "--persona", "windows_chrome145",
        "--port", str(implicit_port),
    ]
    with server(implicit_command, implicit_port, implicit_root):
        require_status(
            exchange(
                implicit_port,
                request_bytes(implicit_port, token=None),
                implicit_root,
                "missing-auth",
            ),
            401,
            "implicit-missing-auth",
        )
        require_status(
            exchange(
                implicit_port,
                request_bytes(implicit_port, token=TOKEN),
                implicit_root,
                "valid",
            ),
            200,
            "implicit-valid",
        )
        result["phases"]["implicit"] = {
            "command": implicit_command,
            "port": implicit_port,
            "officialClient": official_client(f"http://127.0.0.1:{implicit_port}"),
        }

    single_port = contiguous_ports(1)
    single_root = output / "single"
    single_command = [
        str(obscura.resolve()), "--persona", "windows_chrome145", "serve",
        "--host", "127.0.0.1", "--port", str(single_port),
    ]
    with server(single_command, single_port, single_root):
        cases = {
            "missing-auth": (request_bytes(single_port, token=None), 401),
            "bad-host": (request_bytes(single_port, token=TOKEN, host="attacker.test"), 421),
            "bad-origin": (
                request_bytes(single_port, token=TOKEN, origin="https://attacker.test"),
                403,
            ),
            "valid": (request_bytes(single_port, token=TOKEN), 200),
        }
        for name, (request, status) in cases.items():
            require_status(exchange(single_port, request, single_root, name), status, name)
        result["phases"]["single"] = {
            "command": single_command,
            "port": single_port,
            "officialClient": official_client(f"http://127.0.0.1:{single_port}"),
        }

    multi_port = contiguous_ports(3)
    multi_root = output / "multi"
    multi_command = [
        str(obscura.resolve()), "--persona", "windows_chrome145", "serve",
        "--host", "127.0.0.1", "--port", str(multi_port), "--workers", "2",
    ]
    with server(multi_command, multi_port, multi_root):
        require_status(
            exchange(
                multi_port,
                request_bytes(multi_port, token=None),
                multi_root,
                "parent-missing-auth",
            ),
            401,
            "parent-missing-auth",
        )
        require_status(
            exchange(
                multi_port + 1,
                request_bytes(multi_port + 1, token=None, host=f"127.0.0.1:{multi_port}"),
                multi_root,
                "worker-missing-auth",
            ),
            401,
            "worker-missing-auth",
        )
        require_status(
            exchange(
                multi_port + 1,
                request_bytes(multi_port + 1, token=TOKEN),
                multi_root,
                "worker-wrong-host",
            ),
            421,
            "worker-wrong-host",
        )
        result["phases"]["multi"] = {
            "command": multi_command,
            "port": multi_port,
            "officialClient": official_client(f"http://127.0.0.1:{multi_port}"),
        }

    result["status"] = "passed"
    (output / "evidence.json").write_text(json.dumps(result, indent=2) + "\n")
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obscura-bin", type=Path, default=Path("target/release/obscura"))
    parser.add_argument("--output", type=Path)
    arguments = parser.parse_args()
    output = arguments.output or Path(tempfile.mkdtemp(prefix="ob034-access-smoke."))
    print(json.dumps({"output": str(output), **run(arguments.obscura_bin, output)}, indent=2))


if __name__ == "__main__":
    main()

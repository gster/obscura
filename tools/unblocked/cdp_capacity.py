#!/usr/bin/env python3
"""Qualify the real OS listen backlog in front of a single CDP worker.

The runner stops the complete Obscura process group before opening a burst of
TCP connections. Successful connects therefore remain in the kernel listen
queue rather than Obscura's accepted/silent-pending queue. Every request,
response, command stdout/stderr stream, and server stdout/stderr stream is
retained as raw bytes under a caller-owned output directory.

This is a host qualification, not a portable capacity constant. It does not
qualify the multi-worker supervisor, a container network namespace, total
kernel memory, total RSS, the WebSocket handoff queue, or the live slot limit.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import errno
import hashlib
import json
import os
import platform
import re
import resource
import signal
import socket
import subprocess
import sys
import threading
import time
import traceback
from pathlib import Path
from typing import Any


DEFAULT_BURST = 240
DEFAULT_CONNECT_TIMEOUT_SECONDS = 2.0
DEFAULT_IO_TIMEOUT_SECONDS = 10.0
DEFAULT_STARTUP_TIMEOUT_SECONDS = 30.0
DEFAULT_SHUTDOWN_TIMEOUT_SECONDS = 15.0
COMMAND_TIMEOUT_SECONDS = 15.0


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).isoformat()


def sha256_bytes(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def json_bytes(value: object) -> bytes:
    return (json.dumps(value, indent=2, ensure_ascii=False) + "\n").encode("utf-8")


class Evidence:
    def __init__(self, root: Path) -> None:
        self.root = root
        self.root.mkdir(parents=True, exist_ok=False)
        self.artifacts: dict[str, dict[str, object]] = {}
        self.commands: list[dict[str, object]] = []

    def write_bytes(self, relative: str, data: bytes) -> str:
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("xb") as output:
            output.write(data)
        self.record_path(relative)
        return relative

    def write_json(self, relative: str, value: object) -> str:
        return self.write_bytes(relative, json_bytes(value))

    def record_path(self, relative: str) -> dict[str, object]:
        if relative in self.artifacts:
            return self.artifacts[relative]
        path = self.root / relative
        data = path.read_bytes()
        artifact = {
            "path": relative,
            "bytes": len(data),
            "sha256": sha256_bytes(data),
        }
        self.artifacts[relative] = artifact
        return artifact

    def capture_command(self, phase: str, argv: list[str]) -> tuple[dict[str, object], bytes]:
        index = len(self.commands)
        stem = f"host/{phase}/{index:03d}-{Path(argv[0]).name}"
        started_utc = utc_now()
        started_ns = time.monotonic_ns()
        stdout = b""
        stderr = b""
        returncode: int | None = None
        error: str | None = None
        timed_out = False
        try:
            completed = subprocess.run(
                argv,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                timeout=COMMAND_TIMEOUT_SECONDS,
                check=False,
            )
            stdout = completed.stdout
            stderr = completed.stderr
            returncode = completed.returncode
        except subprocess.TimeoutExpired as exc:
            timed_out = True
            stdout = exc.stdout or b""
            stderr = exc.stderr or b""
            error = repr(exc)
        except BaseException as exc:
            error = repr(exc)
        stdout_path = self.write_bytes(f"{stem}.stdout.bin", stdout)
        stderr_path = self.write_bytes(f"{stem}.stderr.bin", stderr)
        result: dict[str, object] = {
            "argv": argv,
            "startedUtc": started_utc,
            "startedMonotonicNs": started_ns,
            "finishedMonotonicNs": time.monotonic_ns(),
            "returncode": returncode,
            "timedOut": timed_out,
            "error": error,
            "stdout": self.artifacts[stdout_path],
            "stderr": self.artifacts[stderr_path],
        }
        result_path = self.write_json(f"{stem}.result.json", result)
        result["result"] = self.artifacts[result_path]
        self.commands.append(result)
        return result, stdout

    def capture_file(self, phase: str, source: Path) -> dict[str, object]:
        safe_name = str(source).strip("/").replace("/", "-") or "root"
        stem = f"host/{phase}/file-{safe_name}"
        error: str | None = None
        data = b""
        try:
            data = source.read_bytes()
        except BaseException as exc:
            error = repr(exc)
        raw_path = self.write_bytes(f"{stem}.bin", data)
        result = {
            "source": str(source),
            "error": error,
            "raw": self.artifacts[raw_path],
        }
        result_path = self.write_json(f"{stem}.result.json", result)
        result["result"] = self.artifacts[result_path]
        return result


def reserve_port(host: str) -> int:
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as listener:
        listener.bind((host, 0))
        return int(listener.getsockname()[1])


def discovery_request(host: str, port: int) -> bytes:
    return (
        f"GET /json/version HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Connection: close\r\n"
        "\r\n"
    ).encode("ascii")


def websocket_request(host: str, port: int) -> bytes:
    return (
        "GET /devtools/browser HTTP/1.1\r\n"
        f"Host: {host}:{port}\r\n"
        "Upgrade: websocket\r\n"
        "Connection: Upgrade\r\n"
        "Sec-WebSocket-Key: T2JzY3VyYUNhcGFjaXR5IQ==\r\n"
        "Sec-WebSocket-Version: 13\r\n"
        "\r\n"
    ).encode("ascii")


def masked_websocket_frame(payload: bytes, opcode: int, mask: bytes) -> bytes:
    if len(mask) != 4:
        raise ValueError("WebSocket mask must contain exactly four bytes")
    if len(payload) >= 126:
        raise ValueError("qualification frames must use the short WebSocket length")
    masked = bytes(value ^ mask[index % 4] for index, value in enumerate(payload))
    return bytes([0x80 | opcode, 0x80 | len(payload)]) + mask + masked


def masked_websocket_close_frame() -> bytes:
    return masked_websocket_frame(b"\x03\xe8", 0x8, b"\x12\x34\x56\x78")


def receive_websocket_frame(
    connection: socket.socket,
) -> tuple[bytes, bytes, dict[str, object], str | None]:
    raw = bytearray()
    metadata: dict[str, object] = {
        "fin": None,
        "opcode": None,
        "masked": None,
        "payloadBytes": None,
    }

    def receive_exact(size: int) -> bytes:
        start = len(raw)
        while len(raw) - start < size:
            chunk = connection.recv(size - (len(raw) - start))
            if not chunk:
                raise EOFError(f"WebSocket frame ended with {size - (len(raw) - start)} bytes missing")
            raw.extend(chunk)
        return bytes(raw[start:])

    try:
        first, second = receive_exact(2)
        metadata["fin"] = bool(first & 0x80)
        metadata["opcode"] = first & 0x0F
        masked = bool(second & 0x80)
        metadata["masked"] = masked
        length = second & 0x7F
        if length == 126:
            length = int.from_bytes(receive_exact(2), "big")
        elif length == 127:
            length = int.from_bytes(receive_exact(8), "big")
        metadata["payloadBytes"] = length
        mask = receive_exact(4) if masked else b""
        payload = receive_exact(length)
        if masked:
            payload = bytes(
                value ^ mask[index % 4] for index, value in enumerate(payload)
            )
        return bytes(raw), payload, metadata, None
    except BaseException as exc:
        return bytes(raw), b"", metadata, repr(exc)


def read_to_eof(connection: socket.socket) -> tuple[bytes, str | None]:
    chunks: list[bytes] = []
    error: str | None = None
    try:
        while True:
            chunk = connection.recv(65_536)
            if not chunk:
                break
            chunks.append(chunk)
    except BaseException as exc:
        error = repr(exc)
    return b"".join(chunks), error


def read_response_head(connection: socket.socket) -> tuple[bytes, str | None]:
    response = bytearray()
    error: str | None = None
    try:
        while b"\r\n\r\n" not in response:
            chunk = connection.recv(65_536)
            if not chunk:
                break
            response.extend(chunk)
    except BaseException as exc:
        error = repr(exc)
    return bytes(response), error


def parse_http_response(response: bytes) -> dict[str, object]:
    parsed: dict[str, object] = {
        "completeHead": False,
        "statusLine": None,
        "status": None,
        "headers": [],
        "bodyBytes": 0,
        "contentLength": None,
        "contentLengthMatches": None,
        "json": None,
        "parseError": None,
    }
    try:
        head, separator, body = response.partition(b"\r\n\r\n")
        parsed["completeHead"] = bool(separator)
        lines = head.split(b"\r\n") if head else []
        if not lines:
            raise ValueError("missing HTTP status line")
        status_line = lines[0].decode("latin-1")
        parsed["statusLine"] = status_line
        match = re.match(r"^HTTP/\d(?:\.\d)?\s+(\d{3})(?:\s|$)", status_line)
        if not match:
            raise ValueError(f"invalid HTTP status line: {status_line!r}")
        parsed["status"] = int(match.group(1))
        headers: list[list[str]] = []
        content_length: int | None = None
        for raw_line in lines[1:]:
            name, colon, value = raw_line.partition(b":")
            if not colon:
                raise ValueError(f"invalid HTTP header: {raw_line!r}")
            decoded_name = name.decode("latin-1")
            decoded_value = value.lstrip().decode("latin-1")
            headers.append([decoded_name, decoded_value])
            if decoded_name.lower() == "content-length":
                content_length = int(decoded_value)
        parsed["headers"] = headers
        parsed["bodyBytes"] = len(body)
        parsed["contentLength"] = content_length
        parsed["contentLengthMatches"] = (
            None if content_length is None else content_length == len(body)
        )
        if body:
            parsed["json"] = json.loads(body)
    except BaseException as exc:
        parsed["parseError"] = repr(exc)
    return parsed


def exchange(
    evidence: Evidence,
    relative_prefix: str,
    host: str,
    port: int,
    request: bytes,
    timeout: float,
) -> tuple[bytes, dict[str, object]]:
    request_path = evidence.write_bytes(f"{relative_prefix}.request.bin", request)
    started_ns = time.monotonic_ns()
    response = b""
    error: str | None = None
    local_address: object = None
    remote_address: object = None
    try:
        with socket.create_connection((host, port), timeout=timeout) as connection:
            connection.settimeout(timeout)
            local_address = list(connection.getsockname())
            remote_address = list(connection.getpeername())
            connection.sendall(request)
            response, error = read_to_eof(connection)
    except BaseException as exc:
        error = repr(exc)
    response_path = evidence.write_bytes(f"{relative_prefix}.response.bin", response)
    parsed = parse_http_response(response)
    result = {
        "startedMonotonicNs": started_ns,
        "finishedMonotonicNs": time.monotonic_ns(),
        "localAddress": local_address,
        "remoteAddress": remote_address,
        "error": error,
        "request": evidence.artifacts[request_path],
        "response": evidence.artifacts[response_path],
        "parsedResponse": parsed,
    }
    result_path = evidence.write_json(f"{relative_prefix}.result.json", result)
    result["result"] = evidence.artifacts[result_path]
    return response, result


def websocket_upgrade_and_close(
    evidence: Evidence,
    relative_prefix: str,
    host: str,
    port: int,
    timeout: float,
) -> dict[str, object]:
    request = websocket_request(host, port)
    command_payload = b'{"id":1,"method":"Browser.getVersion"}'
    command_frame = masked_websocket_frame(
        command_payload, 0x1, b"\x21\x43\x65\x87"
    )
    close_frame = masked_websocket_close_frame()
    request_path = evidence.write_bytes(f"{relative_prefix}.request.bin", request)
    command_payload_path = evidence.write_bytes(
        f"{relative_prefix}.client-command-payload.bin", command_payload
    )
    command_request_path = evidence.write_bytes(
        f"{relative_prefix}.client-command-frame.bin", command_frame
    )
    close_request_path = evidence.write_bytes(
        f"{relative_prefix}.client-close-frame.bin", close_frame
    )
    started_ns = time.monotonic_ns()
    response = b""
    command_response = b""
    command_response_payload = b""
    command_response_metadata: dict[str, object] = {}
    command_error: str | None = None
    close_response = b""
    error: str | None = None
    close_error: str | None = None
    local_address: object = None
    remote_address: object = None
    try:
        with socket.create_connection((host, port), timeout=timeout) as connection:
            connection.settimeout(timeout)
            local_address = list(connection.getsockname())
            remote_address = list(connection.getpeername())
            connection.sendall(request)
            response, error = read_response_head(connection)
            parsed = parse_http_response(response)
            if error is None and parsed.get("status") == 101:
                connection.sendall(command_frame)
                (
                    command_response,
                    command_response_payload,
                    command_response_metadata,
                    command_error,
                ) = receive_websocket_frame(connection)
                connection.sendall(close_frame)
                close_response, close_error = read_to_eof(connection)
    except BaseException as exc:
        error = repr(exc)
    response_path = evidence.write_bytes(f"{relative_prefix}.response.bin", response)
    command_response_path = evidence.write_bytes(
        f"{relative_prefix}.server-command-frame.bin", command_response
    )
    command_response_payload_path = evidence.write_bytes(
        f"{relative_prefix}.server-command-payload.bin", command_response_payload
    )
    close_response_path = evidence.write_bytes(
        f"{relative_prefix}.server-close-wire.bin", close_response
    )
    command_json: object = None
    command_json_error: str | None = None
    if command_response_payload:
        try:
            command_json = json.loads(command_response_payload)
        except BaseException as exc:
            command_json_error = repr(exc)
    result = {
        "startedMonotonicNs": started_ns,
        "finishedMonotonicNs": time.monotonic_ns(),
        "localAddress": local_address,
        "remoteAddress": remote_address,
        "error": error,
        "commandError": command_error,
        "commandJsonError": command_json_error,
        "closeError": close_error,
        "request": evidence.artifacts[request_path],
        "response": evidence.artifacts[response_path],
        "clientCommandPayload": evidence.artifacts[command_payload_path],
        "clientCommandFrame": evidence.artifacts[command_request_path],
        "serverCommandFrame": evidence.artifacts[command_response_path],
        "serverCommandPayload": evidence.artifacts[command_response_payload_path],
        "serverCommandMetadata": command_response_metadata,
        "serverCommandJson": command_json,
        "clientCloseFrame": evidence.artifacts[close_request_path],
        "serverCloseWire": evidence.artifacts[close_response_path],
        "parsedResponse": parse_http_response(response),
    }
    result_path = evidence.write_json(f"{relative_prefix}.result.json", result)
    result["result"] = evidence.artifacts[result_path]
    return result


def wait_ready(
    evidence: Evidence,
    process: subprocess.Popen[bytes],
    host: str,
    port: int,
    timeout: float,
) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    attempt = 0
    last_result: dict[str, object] | None = None
    request = discovery_request(host, port)
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited during readiness with {process.returncode}")
        attempt += 1
        _, result = exchange(
            evidence,
            f"readiness/attempt-{attempt:03d}",
            host,
            port,
            request,
            min(1.0, timeout),
        )
        last_result = result
        parsed = result["parsedResponse"]
        if (
            result.get("error") is None
            and isinstance(parsed, dict)
            and parsed.get("status") == 200
            and parsed.get("completeHead") is True
            and parsed.get("contentLengthMatches") is True
            and parsed.get("parseError") is None
        ):
            return result
        time.sleep(0.05)
    raise TimeoutError(f"CDP endpoint not ready; last result: {last_result!r}")


def wait_for_stopped(pid: int, timeout: float = 5.0) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        waited_pid, status = os.waitpid(pid, os.WNOHANG | os.WUNTRACED)
        if waited_pid == pid:
            if os.WIFSTOPPED(status):
                return {
                    "pid": pid,
                    "waitStatus": status,
                    "stopSignal": os.WSTOPSIG(status),
                    "observedMonotonicNs": time.monotonic_ns(),
                }
            if os.WIFEXITED(status) or os.WIFSIGNALED(status):
                raise RuntimeError(f"server terminated while waiting for SIGSTOP: {status}")
        time.sleep(0.01)
    raise TimeoutError("server process did not enter stopped state")


def parse_darwin_listen_queue(output: bytes, host: str, port: int) -> dict[str, int] | None:
    target = f"{host}.{port}"
    for raw_line in output.decode("latin-1").splitlines():
        fields = raw_line.split()
        if len(fields) >= 2 and fields[1] == target:
            match = re.fullmatch(r"(\d+)/(\d+)/(\d+)", fields[0])
            if match:
                return {
                    "completed": int(match.group(1)),
                    "incomplete": int(match.group(2)),
                    "maximum": int(match.group(3)),
                }
    return None


def parse_linux_listen_queue(
    output: bytes, host: str, port: int, pid: int | None = None
) -> dict[str, int] | None:
    target = f"{host}:{port}"
    for raw_line in output.decode("latin-1").splitlines():
        fields = raw_line.split()
        if len(fields) < 5 or fields[3] != target:
            continue
        if pid is not None and f"pid={pid}," not in raw_line:
            continue
        try:
            return {"completed": int(fields[1]), "maximum": int(fields[2])}
        except ValueError:
            continue
    return None


def parse_lsof_fd_count(output: bytes) -> int | None:
    lines = [line for line in output.splitlines() if line]
    if not lines or not lines[0].startswith(b"COMMAND"):
        return None
    count = 0
    for line in lines[1:]:
        fields = line.split()
        if len(fields) >= 4 and re.fullmatch(rb"\d+[A-Za-z]*", fields[3]):
            count += 1
    return count


def parse_ps_usage(output: bytes) -> dict[str, object] | None:
    fields = output.decode("latin-1").strip().split(maxsplit=7)
    if len(fields) < 7:
        return None
    try:
        return {
            "pid": int(fields[0]),
            "ppid": int(fields[1]),
            "processGroup": int(fields[2]),
            "state": fields[3],
            "rssKiB": int(fields[4]),
            "vszKiB": int(fields[5]),
            "elapsed": fields[6],
            "command": fields[7] if len(fields) == 8 else "",
        }
    except ValueError:
        return None


def linux_fd_inventory(pid: int) -> list[dict[str, object]]:
    directory = Path(f"/proc/{pid}/fd")
    entries: list[dict[str, object]] = []
    try:
        names = sorted(directory.iterdir(), key=lambda path: int(path.name))
    except BaseException as exc:
        return [{"directory": str(directory), "error": repr(exc)}]
    for path in names:
        target: str | None = None
        error: str | None = None
        try:
            target = os.readlink(path)
        except BaseException as exc:
            error = repr(exc)
        entries.append({"fd": path.name, "target": target, "error": error})
    return entries


def capture_snapshot(
    evidence: Evidence,
    phase: str,
    pid: int,
    host: str,
    port: int,
) -> dict[str, object]:
    result: dict[str, object] = {"phase": phase, "capturedUtc": utc_now()}
    evidence.capture_command(phase, ["uname", "-a"])
    _, ps_output = evidence.capture_command(
        phase,
        [
            "ps",
            "-o",
            "pid=,ppid=,pgid=,state=,rss=,vsz=,etime=,command=",
            "-p",
            str(pid),
        ],
    )
    result["psRawSha256"] = sha256_bytes(ps_output)
    result["process"] = parse_ps_usage(ps_output)
    system = platform.system()
    if system == "Darwin":
        evidence.capture_command(phase, ["sysctl", "kern.ipc.somaxconn"])
        evidence.capture_command(phase, ["sysctl", "kern.maxfiles"])
        _, listen_output = evidence.capture_command(phase, ["netstat", "-Lan", "-p", "tcp"])
        evidence.capture_command(phase, ["netstat", "-anv", "-p", "tcp"])
        _, lsof_output = evidence.capture_command(
            phase, ["lsof", "-nP", "-a", "-p", str(pid)]
        )
        evidence.capture_command(phase, ["ps", "-M", "-p", str(pid)])
        result["listenQueue"] = parse_darwin_listen_queue(listen_output, host, port)
        result["serverFdCount"] = parse_lsof_fd_count(lsof_output)
    elif system == "Linux":
        evidence.capture_command(
            phase, ["sysctl", "net.core.somaxconn", "net.ipv4.tcp_max_syn_backlog"]
        )
        _, listen_output = evidence.capture_command(phase, ["ss", "-lntp"])
        evidence.capture_command(phase, ["ss", "-antp"])
        result["listenQueue"] = parse_linux_listen_queue(
            listen_output, host, port, pid
        )
        proc_files = [
            Path(f"/proc/{pid}/status"),
            Path(f"/proc/{pid}/limits"),
            Path(f"/proc/{pid}/cgroup"),
            Path("/proc/net/tcp"),
            Path("/proc/sys/net/core/somaxconn"),
            Path("/proc/sys/net/ipv4/tcp_max_syn_backlog"),
            Path("/sys/fs/cgroup/memory.current"),
            Path("/sys/fs/cgroup/memory.max"),
            Path("/sys/fs/cgroup/memory.events"),
            Path("/sys/fs/cgroup/pids.current"),
            Path("/sys/fs/cgroup/pids.max"),
        ]
        result["files"] = [evidence.capture_file(phase, path) for path in proc_files]
        fd_inventory = linux_fd_inventory(pid)
        fd_path = evidence.write_json(f"host/{phase}/proc-fd-inventory.json", fd_inventory)
        result["serverFdCount"] = sum(1 for item in fd_inventory if item.get("fd"))
        result["fdInventory"] = evidence.artifacts[fd_path]
    else:
        raise RuntimeError(f"unsupported platform for SIGSTOP backlog qualification: {system}")
    snapshot_path = evidence.write_json(f"host/{phase}/snapshot.json", result)
    result["snapshot"] = evidence.artifacts[snapshot_path]
    return result


def wait_for_recovered_snapshot(
    evidence: Evidence,
    pid: int,
    host: str,
    port: int,
    baseline_fds: object,
    timeout: float,
) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    attempt = 0
    latest: dict[str, object] | None = None
    while time.monotonic() < deadline:
        attempt += 1
        latest = capture_snapshot(
            evidence, f"recovered-attempt-{attempt:03d}", pid, host, port
        )
        queue = latest.get("listenQueue")
        fd_count = latest.get("serverFdCount")
        if queue is None or fd_count is None:
            return latest
        if (
            isinstance(queue, dict)
            and int(queue.get("completed", -1)) == 0
            and (baseline_fds is None or fd_count == baseline_fds)
        ):
            return latest
        time.sleep(0.05)
    assert latest is not None
    return latest


def open_backlog_connections(
    evidence: Evidence,
    host: str,
    port: int,
    burst: int,
    timeout: float,
) -> tuple[list[tuple[int, socket.socket]], list[dict[str, object]]]:
    request = discovery_request(host, port)
    request_paths = [
        evidence.write_bytes(f"backlog/client-{index:04d}.request.bin", request)
        for index in range(burst)
    ]
    start_event = threading.Event()
    worker_outputs: dict[
        int, tuple[int, socket.socket | None, dict[str, object]]
    ] = {}
    worker_outputs_lock = threading.Lock()

    def connect(index: int) -> tuple[int, socket.socket | None, dict[str, object]]:
        worker_started_ns = time.monotonic_ns()
        started_ns: int | None = None
        finished_ns: int | None = None
        connection: socket.socket | None = None
        error: str | None = None
        error_type: str | None = None
        error_errno: int | None = None
        error_args: list[str] | None = None
        error_stage: str | None = None
        local_address: object = None
        remote_address: object = None
        tcp_connected = False
        request_sent = False
        try:
            error_stage = "socket"
            connection = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            error_stage = "set-timeout"
            connection.settimeout(timeout)
            error_stage = "burst-release"
            if not start_event.wait(timeout=max(5.0, timeout * 2.0)):
                raise TimeoutError("burst start event was not released")
            started_ns = time.monotonic_ns()
            error_stage = "connect"
            connection.connect((host, port))
            tcp_connected = True
            local_address = list(connection.getsockname())
            remote_address = list(connection.getpeername())
            error_stage = "send-request"
            connection.sendall(request)
            request_sent = True
            error_stage = None
        except BaseException as exc:
            error = repr(exc)
            error_type = type(exc).__name__
            error_errno = getattr(exc, "errno", None)
            error_args = [repr(argument) for argument in getattr(exc, "args", ())]
        finished_ns = time.monotonic_ns()
        if not request_sent and connection is not None:
            try:
                connection.close()
            except BaseException:
                pass
            connection = None
        result = {
            "index": index,
            "workerStartedMonotonicNs": worker_started_ns,
            "startedMonotonicNs": started_ns,
            "finishedMonotonicNs": finished_ns,
            "connected": tcp_connected,
            "requestSent": request_sent,
            "localAddress": local_address,
            "remoteAddress": remote_address,
            "connectError": error,
            "connectErrorType": error_type,
            "connectErrno": error_errno,
            "connectErrorArgs": error_args,
            "connectErrorStage": error_stage,
            "request": evidence.artifacts[request_paths[index]],
        }
        output = (index, connection, result)
        with worker_outputs_lock:
            worker_outputs[index] = output
        return output

    futures: dict[concurrent.futures.Future[tuple[int, socket.socket | None, dict[str, object]]], int] = {}
    submit_error: BaseException | None = None
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=burst)
    try:
        for index in range(burst):
            try:
                futures[pool.submit(connect, index)] = index
            except BaseException as exc:
                submit_error = exc
                break
    finally:
        # Every submitted worker waits on this event only. Releasing it from a
        # finally block makes thread creation and submission failures bounded.
        start_event.set()
    # A real submit() may enqueue work and then raise while starting another
    # thread. Existing workers can still execute that orphaned work item. Wait
    # for the executor first, then trust the worker-owned side channel rather
    # than assuming that a missing Future means the task did not run.
    pool.shutdown(wait=True)
    completed = list(worker_outputs.values())
    completed_indexes = {index for index, _, _ in completed}
    for index in range(burst):
        if index in completed_indexes:
            continue
        future_error: BaseException | None = None
        for future, future_index in futures.items():
            if future_index != index or not future.done():
                continue
            try:
                future.result()
            except BaseException as exc:
                future_error = exc
            break
        exc = (
            future_error
            or submit_error
            or RuntimeError("connection worker was not submitted or did not publish a result")
        )
        completed.append(
            (
                index,
                None,
                {
                    "index": index,
                    "workerStartedMonotonicNs": None,
                    "startedMonotonicNs": None,
                    "finishedMonotonicNs": time.monotonic_ns(),
                    "connected": False,
                    "requestSent": False,
                    "localAddress": None,
                    "remoteAddress": None,
                    "connectError": repr(exc),
                    "connectErrorType": type(exc).__name__,
                    "connectErrno": getattr(exc, "errno", None),
                    "connectErrorArgs": [
                        repr(argument) for argument in getattr(exc, "args", ())
                    ],
                    "connectErrorStage": (
                        "worker-future" if future_error is not None else "thread-submit"
                    ),
                    "request": evidence.artifacts[request_paths[index]],
                },
            )
        )
    completed.sort(key=lambda item: item[0])
    sockets = [
        (index, connection)
        for index, connection, _ in completed
        if connection is not None
    ]
    results = [result for _, _, result in completed]
    for result in results:
        index = int(result["index"])
        path = evidence.write_json(
            f"backlog/client-{index:04d}.connect.result.json", result
        )
        result["connectResult"] = evidence.artifacts[path]
    return sockets, results


def drain_backlog_connections(
    evidence: Evidence,
    connections: list[tuple[int, socket.socket]],
    results: list[dict[str, object]],
    timeout: float,
) -> None:
    by_index = {int(result["index"]): result for result in results}
    drain_outputs: dict[int, tuple[bytes, str | None]] = {}
    drain_outputs_lock = threading.Lock()

    def drain(item: tuple[int, socket.socket]) -> tuple[int, bytes, str | None]:
        index, connection = item
        response = b""
        error: str | None = None
        try:
            connection.settimeout(timeout)
            response, error = read_to_eof(connection)
        except BaseException as exc:
            error = repr(exc)
        finally:
            try:
                connection.close()
            except BaseException as exc:
                if error is None:
                    error = repr(exc)
        with drain_outputs_lock:
            drain_outputs[index] = (response, error)
        return index, response, error

    futures: dict[concurrent.futures.Future[tuple[int, bytes, str | None]], int] = {}
    submit_errors: dict[int, str] = {}
    pool = concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(connections)))
    for index, connection in connections:
        try:
            futures[pool.submit(drain, (index, connection))] = index
        except BaseException as exc:
            submit_errors[index] = repr(exc)
    # As in the connect phase, collect only after shutdown so work enqueued by
    # a submit() call that raised is still represented by its worker output.
    pool.shutdown(wait=True)
    connections_by_index = dict(connections)
    for index, _ in connections:
        output = drain_outputs.get(index)
        if output is None:
            connection = connections_by_index[index]
            error = submit_errors.get(index, "drain worker did not publish a result")
            try:
                connection.close()
            except BaseException as exc:
                error = f"{error}; close: {exc!r}"
            response = b""
        else:
            response, error = output
        response_path = evidence.write_bytes(
            f"backlog/client-{index:04d}.response.bin", response
        )
        result = by_index[index]
        result["responseFinishedMonotonicNs"] = time.monotonic_ns()
        result["responseError"] = error
        result["response"] = evidence.artifacts[response_path]
        result["parsedResponse"] = parse_http_response(response)

    finalize_backlog_connections(evidence, connections, results, None)


def finalize_backlog_connections(
    evidence: Evidence,
    connections: list[tuple[int, socket.socket]],
    results: list[dict[str, object]],
    unfinished_reason: str | None,
) -> None:
    for _, connection in connections:
        try:
            connection.close()
        except BaseException:
            pass
    for result in results:
        index = int(result["index"])
        if "result" in result:
            continue
        if "response" not in result:
            response_relative = f"backlog/client-{index:04d}.response.bin"
            response_file = evidence.root / response_relative
            if response_file.exists():
                evidence.record_path(response_relative)
            else:
                evidence.write_bytes(response_relative, b"")
            result["responseFinishedMonotonicNs"] = None
            result["responseError"] = (
                unfinished_reason if result.get("requestSent") else None
            )
            result["response"] = evidence.artifacts[response_relative]
            result["parsedResponse"] = parse_http_response(b"")
        result_relative = f"backlog/client-{index:04d}.result.json"
        result_file = evidence.root / result_relative
        if result_file.exists():
            evidence.record_path(result_relative)
        else:
            evidence.write_json(result_relative, result)
        result["result"] = evidence.artifacts[result_relative]


def is_backlog_pressure_result(result: dict[str, object]) -> bool:
    return (
        result.get("connected") is False
        and result.get("connectErrorStage") == "connect"
        and (
            result.get("connectErrorType") == "TimeoutError"
            or result.get("connectErrno") == errno.ETIMEDOUT
        )
    )


def validate_complete_http(
    name: str,
    result: dict[str, object],
    status: int,
    failures: list[str],
    *,
    require_body: bool,
) -> None:
    parsed = result.get("parsedResponse")
    if result.get("error") is not None or result.get("responseError") is not None:
        failures.append(f"{name} transport failed")
    elif not isinstance(parsed, dict) or parsed.get("status") != status:
        failures.append(f"{name} did not receive HTTP {status}")
    elif parsed.get("completeHead") is not True:
        failures.append(f"{name} received an incomplete HTTP head")
    elif require_body and parsed.get("contentLengthMatches") is not True:
        failures.append(f"{name} received an incomplete HTTP body")
    elif parsed.get("parseError") is not None:
        failures.append(f"{name} response parse failed: {parsed['parseError']}")


def validate(
    burst: int,
    baseline: dict[str, object],
    queued: dict[str, object],
    recovered: dict[str, object],
    client_results: list[dict[str, object]],
    recovery_http: dict[str, object],
    recovery_ws: dict[str, object],
) -> list[str]:
    failures: list[str] = []
    accepted = [result for result in client_results if result.get("requestSent") is True]
    pressured = [result for result in client_results if is_backlog_pressure_result(result)]
    local_failures = [
        result
        for result in client_results
        if result.get("requestSent") is not True
        and not is_backlog_pressure_result(result)
    ]
    queue = queued.get("listenQueue")
    if not accepted:
        failures.append("no TCP connection completed while the server was stopped")
    if not isinstance(queue, dict) or int(queue.get("completed", 0)) <= 0:
        failures.append("the stopped-server snapshot did not show a nonzero listen queue")
    elif int(queue.get("maximum", 0)) <= 0:
        failures.append("the stopped-server snapshot did not report listen queue capacity")
    elif int(queue.get("completed", 0)) < int(queue["maximum"]):
        failures.append(
            "the stopped-server listen queue did not reach its reported capacity"
        )
    elif int(queue.get("completed", 0)) != len(accepted):
        failures.append(
            "completed listen queue entries did not match successful burst requests"
        )
    if len(accepted) == burst:
        failures.append(
            "every burst connection completed, so the configured burst did not exercise backlog capacity"
        )
    if local_failures:
        summaries = [
            {
                "index": result.get("index"),
                "stage": result.get("connectErrorStage"),
                "type": result.get("connectErrorType"),
                "errno": result.get("connectErrno"),
            }
            for result in local_failures
        ]
        failures.append(f"burst contained non-backlog client failures: {summaries!r}")
    baseline_fds = baseline.get("serverFdCount")
    queued_fds = queued.get("serverFdCount")
    if baseline_fds is None or queued_fds is None:
        failures.append("server FD counts were unavailable")
    elif baseline_fds != queued_fds:
        failures.append(
            f"server FD count changed while stopped: baseline={baseline_fds}, queued={queued_fds}"
        )
    for result in accepted:
        validate_complete_http(
            f"client {result['index']}", result, 200, failures, require_body=True
        )
    recovered_queue = recovered.get("listenQueue")
    if not isinstance(recovered_queue, dict) or int(recovered_queue.get("completed", -1)) != 0:
        failures.append("listen queue did not return to zero after recovery")
    recovered_fds = recovered.get("serverFdCount")
    if baseline_fds is not None and recovered_fds != baseline_fds:
        failures.append(
            f"server FD count did not recover: baseline={baseline_fds}, recovered={recovered_fds}"
        )
    validate_complete_http(
        "post-burst HTTP discovery",
        recovery_http,
        200,
        failures,
        require_body=True,
    )
    ws_parsed = recovery_ws.get("parsedResponse")
    if not isinstance(ws_parsed, dict) or ws_parsed.get("status") != 101:
        failures.append("post-burst WebSocket upgrade did not recover")
    if recovery_ws.get("error") is not None:
        failures.append("post-burst WebSocket upgrade failed")
    if recovery_ws.get("commandError") is not None or recovery_ws.get("commandJsonError") is not None:
        failures.append("post-burst WebSocket CDP round trip failed")
    command_metadata = recovery_ws.get("serverCommandMetadata")
    command_json = recovery_ws.get("serverCommandJson")
    if (
        not isinstance(command_metadata, dict)
        or command_metadata.get("fin") is not True
        or command_metadata.get("opcode") != 1
        or not isinstance(command_json, dict)
        or command_json.get("id") != 1
        or not isinstance(command_json.get("result"), dict)
    ):
        failures.append("post-burst WebSocket did not return Browser.getVersion result")
    if recovery_ws.get("closeError") is not None:
        failures.append("post-burst WebSocket masked Close did not reach clean EOF")
    if not pressured:
        failures.append("no burst connection observed backlog pressure")
    return failures


def run(args: argparse.Namespace) -> tuple[dict[str, object], int]:
    evidence = Evidence(args.output)
    manifest: dict[str, Any] = {
        "schemaVersion": 1,
        "status": "running",
        "startedUtc": utc_now(),
        "scope": {
            "qualified": [
                "single-worker IPv4 loopback kernel listen backlog on this host and binary",
                "recovery of accepted discovery requests and a new WebSocket upgrade",
            ],
            "notQualified": [
                "multi-worker supervisor",
                "container network namespace or published-port proxy",
                "total kernel TCP memory",
                "total process or container RSS bound",
                "accepted silent-pending limit",
                "WebSocket handoff queue",
                "live connection slot limit",
                "other operating systems or kernels",
            ],
        },
        "platform": {
            "system": platform.system(),
            "release": platform.release(),
            "version": platform.version(),
            "machine": platform.machine(),
            "python": sys.version,
            "uname": list(platform.uname()),
            "rlimitNofile": list(resource.getrlimit(resource.RLIMIT_NOFILE)),
        },
        "configuration": {
            "binary": str(args.binary.resolve()),
            "host": args.host,
            "port": None,
            "persona": args.persona,
            "burst": args.burst,
            "maxConnections": args.max_connections,
            "connectTimeoutSeconds": args.connect_timeout,
            "ioTimeoutSeconds": args.io_timeout,
            "startupTimeoutSeconds": args.startup_timeout,
            "shutdownTimeoutSeconds": args.shutdown_timeout,
        },
        "server": {},
        "phases": {},
        "failures": [],
    }
    process: subprocess.Popen[bytes] | None = None
    stdout_handle = None
    stderr_handle = None
    stopped = False
    connections: list[tuple[int, socket.socket]] = []
    client_results: list[dict[str, object]] = []
    unfinished_reason: str | None = "qualification ended before response collection"
    exit_code = 1
    try:
        if platform.system() not in {"Darwin", "Linux"}:
            raise RuntimeError("this qualification requires POSIX SIGSTOP and Darwin or Linux queue tools")
        if not args.binary.is_file():
            raise FileNotFoundError(f"missing Obscura binary: {args.binary}")
        if args.burst < 2:
            raise ValueError("--burst must be at least 2")
        port = args.port or reserve_port(args.host)
        manifest["configuration"]["port"] = port
        command = [
            str(args.binary.resolve()),
            "--persona",
            args.persona,
            "serve",
            "--host",
            args.host,
            "--port",
            str(port),
            "--workers",
            "1",
            "--max-connections",
            str(args.max_connections),
        ]
        stdout_path = evidence.root / "server/stdout.bin"
        stderr_path = evidence.root / "server/stderr.bin"
        stdout_path.parent.mkdir(parents=True, exist_ok=True)
        stdout_handle = stdout_path.open("xb")
        stderr_handle = stderr_path.open("xb")
        process = subprocess.Popen(
            command,
            stdin=subprocess.DEVNULL,
            stdout=stdout_handle,
            stderr=stderr_handle,
            start_new_session=True,
        )
        manifest["server"] = {
            "argv": command,
            "pid": process.pid,
            "processGroup": os.getpgid(process.pid),
            "startedMonotonicNs": time.monotonic_ns(),
        }
        manifest["phases"]["readiness"] = wait_ready(
            evidence, process, args.host, port, args.startup_timeout
        )
        baseline = capture_snapshot(evidence, "baseline", process.pid, args.host, port)
        manifest["phases"]["baseline"] = baseline

        os.killpg(process.pid, signal.SIGSTOP)
        stop_observation = wait_for_stopped(process.pid)
        stopped = True
        manifest["phases"]["stopObservation"] = stop_observation
        connections, client_results = open_backlog_connections(
            evidence,
            args.host,
            port,
            args.burst,
            args.connect_timeout,
        )
        manifest["phases"]["burst"] = {
            "attempted": args.burst,
            "tcpConnected": sum(
                result.get("connected") is True for result in client_results
            ),
            "requestSent": sum(
                result.get("requestSent") is True for result in client_results
            ),
            "pressured": sum(
                is_backlog_pressure_result(result) for result in client_results
            ),
            "clientFailures": sum(
                result.get("requestSent") is not True
                and not is_backlog_pressure_result(result)
                for result in client_results
            ),
        }
        queued = capture_snapshot(evidence, "queued", process.pid, args.host, port)
        manifest["phases"]["queued"] = queued

        os.killpg(process.pid, signal.SIGCONT)
        stopped = False
        manifest["phases"]["continuedMonotonicNs"] = time.monotonic_ns()
        drain_backlog_connections(
            evidence, connections, client_results, args.io_timeout
        )
        unfinished_reason = None
        recovery_response, recovery_http = exchange(
            evidence,
            "recovery/discovery",
            args.host,
            port,
            discovery_request(args.host, port),
            args.io_timeout,
        )
        del recovery_response
        recovery_ws = websocket_upgrade_and_close(
            evidence,
            "recovery/websocket",
            args.host,
            port,
            args.io_timeout,
        )
        recovered = wait_for_recovered_snapshot(
            evidence,
            process.pid,
            args.host,
            port,
            baseline.get("serverFdCount"),
            args.io_timeout,
        )
        manifest["phases"]["recovery"] = {
            "discovery": recovery_http,
            "websocket": recovery_ws,
            "snapshot": recovered,
        }
        failures = validate(
            args.burst,
            baseline,
            queued,
            recovered,
            client_results,
            recovery_http,
            recovery_ws,
        )
        manifest["failures"] = failures
        manifest["status"] = "passed" if not failures else "failed"
        exit_code = 0 if not failures else 1
    except BaseException as exc:
        unfinished_reason = repr(exc)
        manifest["status"] = "failed"
        manifest["failures"].append(repr(exc))
        manifest["exceptionType"] = type(exc).__name__
        evidence.write_bytes("failure.traceback.txt", traceback.format_exc().encode("utf-8"))
        exit_code = 1
    finally:
        if client_results:
            try:
                finalize_backlog_connections(
                    evidence, connections, client_results, unfinished_reason
                )
            except BaseException as exc:
                manifest["status"] = "failed"
                manifest["failures"].append(
                    f"backlog client cleanup failed: {exc!r}"
                )
                exit_code = 1
        if process is not None:
            if stopped:
                try:
                    os.killpg(process.pid, signal.SIGCONT)
                except ProcessLookupError:
                    pass
            shutdown: dict[str, object] = {
                "signal": "SIGTERM",
                "startedMonotonicNs": time.monotonic_ns(),
                "forcedKill": False,
            }
            if process.poll() is None:
                try:
                    os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError:
                    pass
                try:
                    process.wait(timeout=args.shutdown_timeout)
                except subprocess.TimeoutExpired:
                    shutdown["forcedKill"] = True
                    try:
                        os.killpg(process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    process.wait(timeout=args.shutdown_timeout)
            shutdown["finishedMonotonicNs"] = time.monotonic_ns()
            shutdown["returncode"] = process.returncode
            manifest["server"]["shutdown"] = shutdown
        if stdout_handle is not None:
            stdout_handle.close()
            evidence.record_path("server/stdout.bin")
        if stderr_handle is not None:
            stderr_handle.close()
            evidence.record_path("server/stderr.bin")
        manifest["finishedUtc"] = utc_now()
        manifest["artifacts"] = sorted(
            evidence.artifacts.values(), key=lambda item: str(item["path"])
        )
        with (evidence.root / "evidence.json").open("xb") as output:
            output.write(json_bytes(manifest))
    return manifest, exit_code


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--persona", default="windows_chrome145")
    parser.add_argument("--burst", type=int, default=DEFAULT_BURST)
    parser.add_argument("--max-connections", type=int, default=128)
    parser.add_argument(
        "--connect-timeout", type=float, default=DEFAULT_CONNECT_TIMEOUT_SECONDS
    )
    parser.add_argument("--io-timeout", type=float, default=DEFAULT_IO_TIMEOUT_SECONDS)
    parser.add_argument(
        "--startup-timeout", type=float, default=DEFAULT_STARTUP_TIMEOUT_SECONDS
    )
    parser.add_argument(
        "--shutdown-timeout", type=float, default=DEFAULT_SHUTDOWN_TIMEOUT_SECONDS
    )
    return parser.parse_args()


def main() -> None:
    args = parse_args()
    _, exit_code = run(args)
    raise SystemExit(exit_code)


if __name__ == "__main__":
    main()

#!/usr/bin/env python3
"""Qualify the bounded byte-transparent front door for multi-worker CDP.

The runner holds a real zero-byte TCP relay open in front of a release Obscura
supervisor. It proves that a silent first client no longer blocks later
discovery, that the aggregate ``workers * max-connections`` relay limit emits a
complete 503, that releasing one relay restores admission, and that a fresh
WebSocket still completes a raw Browser.getVersion round trip.

Every request, response, WebSocket frame, host-command stream, server stream,
exception, and hash is retained under a caller-owned new output directory. No
captured field or byte stream is redacted, truncated, or removed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import random
import resource
import select
import signal
import socket
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Any

try:
    from .cdp_capacity import (
        Evidence,
        discovery_request,
        exchange,
        json_bytes,
        masked_websocket_close_frame,
        masked_websocket_frame,
        parse_http_response,
        read_response_head,
        read_to_eof,
        receive_websocket_frame,
        utc_now,
        validate_complete_http,
        wait_ready,
        websocket_request,
        websocket_upgrade_and_close,
    )
except ImportError:
    from cdp_capacity import (
        Evidence,
        discovery_request,
        exchange,
        json_bytes,
        masked_websocket_close_frame,
        masked_websocket_frame,
        parse_http_response,
        read_response_head,
        read_to_eof,
        receive_websocket_frame,
        utc_now,
        validate_complete_http,
        wait_ready,
        websocket_request,
        websocket_upgrade_and_close,
    )


DEFAULT_CONNECT_TIMEOUT_SECONDS = 2.0
DEFAULT_IO_TIMEOUT_SECONDS = 15.0
DEFAULT_STARTUP_TIMEOUT_SECONDS = 30.0
DEFAULT_SHUTDOWN_TIMEOUT_SECONDS = 15.0


def probe_contiguous_ports(host: str, count: int) -> int:
    if count < 1:
        raise ValueError("contiguous port count must be positive")
    for _ in range(200):
        listeners: list[socket.socket] = []
        try:
            base = random.randint(40_000, 45_000 - count)
            first = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            first.bind((host, base))
            listeners.append(first)
            for port in range(base + 1, base + count):
                listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
                listener.bind((host, port))
                listeners.append(listener)
            return base
        except OSError:
            pass
        finally:
            for listener in listeners:
                listener.close()
    raise RuntimeError(f"could not find {count} probed-free contiguous ports on {host}")


def process_group_exists(process_group: int) -> bool:
    try:
        os.killpg(process_group, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def wait_for_process_group_exit(process_group: int, timeout: float) -> bool:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if not process_group_exists(process_group):
            return True
        time.sleep(0.02)
    return not process_group_exists(process_group)


def sha256_file(path: Path) -> tuple[str, int]:
    digest = hashlib.sha256()
    size = 0
    with path.open("rb") as source:
        while True:
            chunk = source.read(1024 * 1024)
            if not chunk:
                return digest.hexdigest(), size
            digest.update(chunk)
            size += len(chunk)


def overload_request(host: str, port: int) -> bytes:
    return (
        f"GET /json/version HTTP/1.1\r\nHost: {host}:{port}\r\nX-Fill: "
        + "x" * 6_000
        + "\r\nConnection: close\r\n\r\n"
    ).encode("ascii")


def open_held_connection(
    evidence: Evidence,
    relative_prefix: str,
    host: str,
    port: int,
    timeout: float,
    payload: bytes = b"",
) -> tuple[socket.socket, dict[str, object]]:
    request_path = evidence.write_bytes(f"{relative_prefix}.request.bin", payload)
    started_ns = time.monotonic_ns()
    connection = socket.create_connection((host, port), timeout=timeout)
    connection.settimeout(timeout)
    if payload:
        connection.sendall(payload)
    result: dict[str, object] = {
        "status": "connected-and-sent" if payload else "connected",
        "startedMonotonicNs": started_ns,
        "connectedMonotonicNs": time.monotonic_ns(),
        "localAddress": list(connection.getsockname()),
        "remoteAddress": list(connection.getpeername()),
        "request": evidence.artifacts[request_path],
        "error": None,
    }
    result_path = evidence.write_json(f"{relative_prefix}.connect.json", result)
    result["connectResult"] = evidence.artifacts[result_path]
    return connection, result


def open_held_websocket(
    evidence: Evidence,
    relative_prefix: str,
    host: str,
    port: int,
    timeout: float,
) -> tuple[socket.socket, dict[str, object]]:
    request = websocket_request(host, port)
    request_path = evidence.write_bytes(
        f"{relative_prefix}.handshake.request.bin", request
    )
    started_ns = time.monotonic_ns()
    connection = socket.create_connection((host, port), timeout=timeout)
    connection.settimeout(timeout)
    connection.sendall(request)
    response, response_error = read_response_head(connection)
    response_path = evidence.write_bytes(
        f"{relative_prefix}.handshake.response.bin", response
    )
    parsed = parse_http_response(response)
    result: dict[str, object] = {
        "status": "upgraded" if parsed.get("status") == 101 else "failed",
        "startedMonotonicNs": started_ns,
        "finishedMonotonicNs": time.monotonic_ns(),
        "localAddress": list(connection.getsockname()),
        "remoteAddress": list(connection.getpeername()),
        "responseError": response_error,
        "request": evidence.artifacts[request_path],
        "response": evidence.artifacts[response_path],
        "parsedResponse": parsed,
    }
    result_path = evidence.write_json(
        f"{relative_prefix}.handshake.result.json", result
    )
    result["handshakeResult"] = evidence.artifacts[result_path]
    if response_error is not None or parsed.get("status") != 101:
        connection.close()
        raise RuntimeError(f"held WebSocket upgrade failed: {result!r}")
    return connection, result


def command_held_websocket(
    evidence: Evidence,
    relative_prefix: str,
    connection: socket.socket,
) -> dict[str, object]:
    payload = b'{"id":2,"method":"Browser.getVersion"}'
    frame = masked_websocket_frame(payload, 0x1, b"\x31\x42\x53\x64")
    payload_path = evidence.write_bytes(f"{relative_prefix}.payload.bin", payload)
    frame_path = evidence.write_bytes(f"{relative_prefix}.frame.bin", frame)
    started_ns = time.monotonic_ns()
    connection.sendall(frame)
    raw, response_payload, metadata, receive_error = receive_websocket_frame(connection)
    raw_path = evidence.write_bytes(f"{relative_prefix}.response-frame.bin", raw)
    response_payload_path = evidence.write_bytes(
        f"{relative_prefix}.response-payload.bin", response_payload
    )
    parsed: object = None
    json_error: str | None = None
    try:
        parsed = json.loads(response_payload)
    except BaseException as exc:
        json_error = repr(exc)
    result = {
        "startedMonotonicNs": started_ns,
        "finishedMonotonicNs": time.monotonic_ns(),
        "error": receive_error,
        "jsonError": json_error,
        "metadata": metadata,
        "requestPayload": evidence.artifacts[payload_path],
        "requestFrame": evidence.artifacts[frame_path],
        "responseFrame": evidence.artifacts[raw_path],
        "responsePayload": evidence.artifacts[response_payload_path],
        "responseJson": parsed,
    }
    result_path = evidence.write_json(f"{relative_prefix}.result.json", result)
    result["result"] = evidence.artifacts[result_path]
    if (
        receive_error is not None
        or json_error is not None
        or not isinstance(parsed, dict)
        or parsed.get("id") != 2
        or not isinstance(parsed.get("result"), dict)
    ):
        raise RuntimeError(f"held WebSocket command failed: {result!r}")
    return result


def close_held_websocket(
    evidence: Evidence,
    relative_prefix: str,
    connection: socket.socket,
    result: dict[str, object],
    reason: str,
) -> None:
    close_frame = masked_websocket_close_frame()
    close_request_path = evidence.write_bytes(
        f"{relative_prefix}.close.request.bin", close_frame
    )
    started_ns = time.monotonic_ns()
    close_error: str | None = None
    response = b""
    try:
        connection.sendall(close_frame)
        response, close_error = read_to_eof(connection)
    except BaseException as exc:
        close_error = repr(exc)
    finally:
        connection.close()
    close_response_path = evidence.write_bytes(
        f"{relative_prefix}.close.response.bin", response
    )
    final = {
        **result,
        "status": "closed",
        "closeReason": reason,
        "closeStartedMonotonicNs": started_ns,
        "closeFinishedMonotonicNs": time.monotonic_ns(),
        "closeError": close_error,
        "closeRequest": evidence.artifacts[close_request_path],
        "closeResponse": evidence.artifacts[close_response_path],
    }
    evidence.write_json(f"{relative_prefix}.close.result.json", final)
    if close_error is not None:
        raise RuntimeError(f"held WebSocket close failed: {final!r}")


def close_held_connection(
    evidence: Evidence,
    relative_prefix: str,
    connection: socket.socket,
    result: dict[str, object],
    reason: str,
) -> None:
    chunks: list[bytes] = []
    receive_error: str | None = None
    try:
        connection.setblocking(False)
        while True:
            try:
                chunk = connection.recv(65_536)
            except BlockingIOError:
                break
            if not chunk:
                break
            chunks.append(chunk)
    except BaseException as exc:
        receive_error = repr(exc)
    finally:
        try:
            connection.close()
        except BaseException as exc:
            if receive_error is None:
                receive_error = repr(exc)
    response_path = evidence.write_bytes(
        f"{relative_prefix}.response.bin", b"".join(chunks)
    )
    final = {
        **result,
        "status": "closed",
        "closeReason": reason,
        "finishedMonotonicNs": time.monotonic_ns(),
        "receiveError": receive_error,
        "response": evidence.artifacts[response_path],
    }
    evidence.write_json(f"{relative_prefix}.final.json", final)


def capture_host_state(
    evidence: Evidence, phase: str, pid: int, host: str, port: int
) -> None:
    commands = [
        ["uname", "-a"],
        ["ps", "-axww", "-o", "pid,ppid,pgid,state,rss,vsz,command"],
    ]
    if platform.system() == "Darwin":
        commands.extend(
            [
                ["lsof", "-nP", "-p", str(pid)],
                ["netstat", "-anv", "-p", "tcp"],
            ]
        )
    elif platform.system() == "Linux":
        commands.extend(
            [
                ["ss", "-lntp"],
                ["ss", "-antp"],
                ["ps", "--ppid", str(pid), "-o", "pid,ppid,pgid,state,rss,vsz,args"],
            ]
        )
        for source in [
            Path(f"/proc/{pid}/status"),
            Path(f"/proc/{pid}/limits"),
        ]:
            evidence.capture_file(phase, source)
    for argv in commands:
        evidence.capture_command(phase, argv)
    evidence.write_json(
        f"host/{phase}/target.json",
        {"pid": pid, "host": host, "port": port},
    )


def endpoint_port(endpoint: str) -> int | None:
    try:
        return int(endpoint.rsplit(":", 1)[1])
    except (IndexError, ValueError):
        return None


def parse_parent_tcp_mappings(
    output: bytes,
    pid: int,
    parent_port: int,
    worker_ports: set[int],
) -> dict[str, object]:
    accepted_client_ports: list[int] = []
    worker_connections: list[dict[str, object]] = []
    lines = output.decode("utf-8", "surrogateescape").splitlines()
    if platform.system() == "Darwin":
        for line in lines:
            if " TCP " not in line or "(ESTABLISHED)" not in line:
                continue
            fields = line.split()
            if len(fields) < 9:
                continue
            try:
                line_pid = int(fields[1])
            except ValueError:
                continue
            if line_pid != pid or "->" not in fields[8]:
                continue
            local, remote = fields[8].split("->", 1)
            local_port = endpoint_port(local)
            remote_port = endpoint_port(remote)
            if local_port == parent_port and remote_port is not None:
                accepted_client_ports.append(remote_port)
            if remote_port in worker_ports:
                worker_connections.append(
                    {"local": local, "remote": remote, "line": line}
                )
    elif platform.system() == "Linux":
        marker = f"pid={pid},"
        for line in lines:
            if not line.startswith("ESTAB ") or marker not in line:
                continue
            fields = line.split()
            if len(fields) < 5:
                continue
            local, remote = fields[3], fields[4]
            local_port = endpoint_port(local)
            remote_port = endpoint_port(remote)
            if local_port == parent_port and remote_port is not None:
                accepted_client_ports.append(remote_port)
            if remote_port in worker_ports:
                worker_connections.append(
                    {"local": local, "remote": remote, "line": line}
                )
    return {
        "acceptedClientPorts": sorted(accepted_client_ports),
        "workerConnections": worker_connections,
        "workerConnectionCount": len(worker_connections),
    }


def held_socket_probe(
    evidence: Evidence,
    relative_prefix: str,
    connection: socket.socket,
) -> dict[str, object]:
    readable = False
    peek = b""
    error: str | None = None
    eof = False
    try:
        readable = bool(select.select([connection], [], [], 0)[0])
        if readable:
            peek = connection.recv(65_536, socket.MSG_PEEK)
            eof = peek == b""
    except BaseException as exc:
        error = repr(exc)
    peek_path = evidence.write_bytes(f"{relative_prefix}.probe.bin", peek)
    result = {
        "monotonicNs": time.monotonic_ns(),
        "readable": readable,
        "eof": eof,
        "error": error,
        "peek": evidence.artifacts[peek_path],
    }
    result_path = evidence.write_json(f"{relative_prefix}.probe.json", result)
    result["result"] = evidence.artifacts[result_path]
    return result


def wait_for_held_relay_barrier(
    evidence: Evidence,
    phase: str,
    process: subprocess.Popen[bytes],
    host: str,
    parent_port: int,
    worker_ports: set[int],
    held: list[tuple[str, socket.socket, dict[str, object]]],
    timeout: float,
) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    attempt = 0
    last: dict[str, object] | None = None
    expected_client_ports = sorted(
        int(connection.getsockname()[1]) for _, connection, _ in held
    )
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited with status {process.returncode}")
        attempt += 1
        if platform.system() == "Darwin":
            command = ["lsof", "-nP", "-a", "-p", str(process.pid), "-iTCP"]
        elif platform.system() == "Linux":
            command = ["ss", "-antp"]
        else:
            raise RuntimeError("relay mapping requires Darwin lsof or Linux ss")
        command_result, stdout = evidence.capture_command(
            f"{phase}/attempt-{attempt:03d}", command
        )
        mappings = parse_parent_tcp_mappings(
            stdout, process.pid, parent_port, worker_ports
        )
        probes = [
            held_socket_probe(
                evidence,
                f"host/{phase}/attempt-{attempt:03d}/{Path(prefix).name}",
                connection,
            )
            for prefix, connection, _ in held
        ]
        observed_client_ports = mappings["acceptedClientPorts"]
        mapped = all(port in observed_client_ports for port in expected_client_ports)
        worker_count_matches = mappings["workerConnectionCount"] == len(held)
        held_are_quiet = all(
            probe["readable"] is False
            and probe["eof"] is False
            and probe["error"] is None
            for probe in probes
        )
        last = {
            "attempt": attempt,
            "command": command_result,
            "expectedClientPorts": expected_client_ports,
            "mappings": mappings,
            "probes": probes,
            "allHeldClientsAccepted": mapped,
            "workerConnectionCountMatches": worker_count_matches,
            "allHeldSocketsQuiet": held_are_quiet,
        }
        evidence.write_json(
            f"host/{phase}/attempt-{attempt:03d}/observation.json", last
        )
        if mapped and worker_count_matches and held_are_quiet:
            return last
        time.sleep(0.05)
    raise TimeoutError(f"held relays did not reach admission barrier: {last!r}")


def header_value(result: dict[str, object], name: str) -> str | None:
    parsed = result.get("parsedResponse")
    if not isinstance(parsed, dict):
        return None
    headers = parsed.get("headers")
    if not isinstance(headers, list):
        return None
    for item in headers:
        if (
            isinstance(item, list)
            and len(item) == 2
            and str(item[0]).lower() == name.lower()
        ):
            return str(item[1])
    return None


def wait_for_recovery(
    evidence: Evidence,
    process: subprocess.Popen[bytes],
    host: str,
    port: int,
    timeout: float,
) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    attempt = 0
    last: dict[str, object] | None = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"server exited with status {process.returncode}")
        attempt += 1
        _, result = exchange(
            evidence,
            f"recovery/discovery-attempt-{attempt:03d}",
            host,
            port,
            discovery_request(host, port),
            min(1.0, max(0.1, deadline - time.monotonic())),
        )
        last = result
        parsed = result.get("parsedResponse")
        if isinstance(parsed, dict) and parsed.get("status") == 200:
            return result
        time.sleep(0.25)
    raise TimeoutError(f"relay admission did not recover: {last!r}")


def validate(
    first_discovery: dict[str, object],
    overload: dict[str, object],
    recovery: dict[str, object],
    websocket: dict[str, object],
) -> list[str]:
    failures: list[str] = []
    validate_complete_http(
        "discovery behind a silent first relay",
        first_discovery,
        200,
        failures,
        require_body=True,
    )
    validate_complete_http(
        "aggregate relay refusal",
        overload,
        503,
        failures,
        require_body=True,
    )
    if header_value(overload, "X-Obscura-Reason") != "max-relays":
        failures.append("aggregate relay refusal omitted X-Obscura-Reason: max-relays")
    validate_complete_http(
        "discovery after releasing one relay",
        recovery,
        200,
        failures,
        require_body=True,
    )
    parsed = websocket.get("parsedResponse")
    if not isinstance(parsed, dict) or parsed.get("status") != 101:
        failures.append("recovery WebSocket did not receive HTTP 101")
    if websocket.get("error") is not None:
        failures.append("recovery WebSocket upgrade failed")
    command = websocket.get("serverCommandJson")
    if (
        websocket.get("commandError") is not None
        or websocket.get("commandJsonError") is not None
        or not isinstance(command, dict)
        or command.get("id") != 1
        or not isinstance(command.get("result"), dict)
    ):
        failures.append("recovery WebSocket did not return Browser.getVersion id=1")
    if websocket.get("closeError") is not None:
        failures.append("recovery WebSocket masked Close did not reach clean EOF")
    return failures


def run(args: argparse.Namespace) -> tuple[dict[str, object], int]:
    evidence = Evidence(args.output)
    binary = args.binary.resolve()
    relay_limit = args.workers * args.max_connections
    manifest: dict[str, Any] = {
        "schemaVersion": 1,
        "status": "running",
        "startedUtc": utc_now(),
        "scope": {
            "qualified": [
                "host-specific multi-worker IPv4 loopback zero-byte silent-client liveness",
                "aggregate parent relay admission at workers times max-connections",
                "complete oversized-request 503 refusal and prompt release with another WebSocket still live",
                "fresh HTTP discovery and raw WebSocket Browser.getVersion recovery",
            ],
            "notQualified": [
                "worker readiness, crash detection, child reaping, or parent-only shutdown",
                "kernel listen backlog or TCP memory",
                "container namespaces or published-port proxies",
                "total FD, task, RSS, V8 heap, or socket-buffer bounds",
                "other operating systems, kernels, addresses, or binaries",
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
            "binary": str(binary),
            "host": args.host,
            "port": None,
            "persona": args.persona,
            "workers": args.workers,
            "maxConnectionsPerWorker": args.max_connections,
            "aggregateRelayLimit": relay_limit,
            "connectTimeoutSeconds": args.connect_timeout,
            "ioTimeoutSeconds": args.io_timeout,
            "startupTimeoutSeconds": args.startup_timeout,
            "shutdownTimeoutSeconds": args.shutdown_timeout,
            "portSelection": "probed free in 40000-45000; not reserved across process spawn",
        },
        "server": {},
        "phases": {},
        "failures": [],
    }
    process: subprocess.Popen[bytes] | None = None
    stdout_handle = None
    stderr_handle = None
    held: list[tuple[str, socket.socket, dict[str, object]]] = []
    exit_code = 1
    try:
        if not binary.is_file():
            raise FileNotFoundError(f"missing Obscura binary: {binary}")
        if args.workers < 2:
            raise ValueError("--workers must be at least 2")
        if args.max_connections < 1:
            raise ValueError("--max-connections must be positive")
        if relay_limit < 2:
            raise ValueError("aggregate relay limit must be at least 2")
        binary_sha256, binary_bytes = sha256_file(binary)
        manifest["configuration"]["binarySha256"] = binary_sha256
        manifest["configuration"]["binaryBytes"] = binary_bytes
        port = args.port or probe_contiguous_ports(args.host, args.workers + 1)
        if port + args.workers > 65_535:
            raise ValueError("parent and worker ports exceed the u16 range")
        manifest["configuration"]["port"] = port
        command = [
            str(binary),
            "--persona",
            args.persona,
            "serve",
            "--host",
            args.host,
            "--port",
            str(port),
            "--workers",
            str(args.workers),
            "--max-connections",
            str(args.max_connections),
        ]
        evidence.write_json(
            "server/command.json",
            {"argv": command, "environment": {"inherit": True, "overrides": {}}},
        )
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
        capture_host_state(evidence, "baseline", process.pid, args.host, port)

        first_socket, first_result = open_held_connection(
            evidence,
            "clients/held-000",
            args.host,
            port,
            args.connect_timeout,
        )
        held.append(("clients/held-000", first_socket, first_result))
        first_barrier = wait_for_held_relay_barrier(
            evidence,
            "first-held-barrier",
            process,
            args.host,
            port,
            set(range(port + 1, port + args.workers + 1)),
            held,
            args.io_timeout,
        )
        _, first_discovery = exchange(
            evidence,
            "silent-unblocked/discovery",
            args.host,
            port,
            discovery_request(args.host, port),
            args.io_timeout,
        )
        manifest["phases"]["silentUnblocked"] = {
            "barrier": first_barrier,
            "discovery": first_discovery,
        }
        prefix, connection, result = held.pop(0)
        close_held_connection(evidence, prefix, connection, result, "silent-phase-complete")
        silent_cleanup_barrier = wait_for_held_relay_barrier(
            evidence,
            "silent-cleanup-barrier",
            process,
            args.host,
            port,
            set(range(port + 1, port + args.workers + 1)),
            held,
            args.io_timeout,
        )
        manifest["phases"]["silentUnblocked"]["cleanupBarrier"] = silent_cleanup_barrier

        for index in range(relay_limit):
            prefix = f"clients/held-websocket-{index:03d}"
            connection, result = open_held_websocket(
                evidence, prefix, args.host, port, args.io_timeout
            )
            held.append((prefix, connection, result))
        capacity_barrier = wait_for_held_relay_barrier(
            evidence,
            "capacity-barrier",
            process,
            args.host,
            port,
            set(range(port + 1, port + args.workers + 1)),
            held,
            args.io_timeout,
        )
        _, overload = exchange(
            evidence,
            "overload/refusal",
            args.host,
            port,
            overload_request(args.host, port),
            args.io_timeout,
        )
        manifest["phases"]["overload"] = {
            "barrier": capacity_barrier,
            "refusal": overload,
        }
        capture_host_state(evidence, "overload", process.pid, args.host, port)

        prefix, connection, result = held.pop(0)
        close_held_websocket(
            evidence, prefix, connection, result, "release-one-relay"
        )
        release_barrier = wait_for_held_relay_barrier(
            evidence,
            "release-barrier",
            process,
            args.host,
            port,
            set(range(port + 1, port + args.workers + 1)),
            held,
            args.io_timeout,
        )
        recovery = wait_for_recovery(
            evidence, process, args.host, port, args.io_timeout
        )
        manifest["phases"]["releaseRecovery"] = {
            "barrier": release_barrier,
            "discovery": recovery,
            "remainingWebSocketCommands": [],
        }

        while held:
            prefix, connection, result = held[0]
            command_result = command_held_websocket(
                evidence, f"{prefix}.post-release-command", connection
            )
            manifest["phases"]["releaseRecovery"]["remainingWebSocketCommands"].append(
                command_result
            )
            close_held_websocket(
                evidence, prefix, connection, result, "qualification-cleanup"
            )
            held.pop(0)
        final_cleanup_barrier = wait_for_held_relay_barrier(
            evidence,
            "final-cleanup-barrier",
            process,
            args.host,
            port,
            set(range(port + 1, port + args.workers + 1)),
            held,
            args.io_timeout,
        )
        manifest["phases"]["releaseRecovery"]["cleanupBarrier"] = final_cleanup_barrier
        websocket = websocket_upgrade_and_close(
            evidence,
            "recovery/websocket",
            args.host,
            port,
            args.io_timeout,
        )
        manifest["phases"]["recoveryWebSocket"] = websocket
        capture_host_state(evidence, "recovered", process.pid, args.host, port)
        failures = validate(first_discovery, overload, recovery, websocket)
        if process.poll() is not None:
            failures.append(f"server exited early with status {process.returncode}")
        manifest["failures"] = failures
        manifest["status"] = "passed" if not failures else "failed"
        exit_code = 0 if not failures else 1
    except BaseException as exc:
        manifest["status"] = "failed"
        manifest["failures"].append(repr(exc))
        manifest["exceptionType"] = type(exc).__name__
        evidence.write_bytes("failure.traceback.txt", traceback.format_exc().encode("utf-8"))
    finally:
        while held:
            prefix, connection, result = held.pop(0)
            try:
                close_held_connection(
                    evidence, prefix, connection, result, "exception-cleanup"
                )
            except BaseException as exc:
                manifest["failures"].append(f"held connection cleanup failed: {exc!r}")
                manifest["status"] = "failed"
                exit_code = 1
        if process is not None:
            process_group = int(manifest["server"]["processGroup"])
            shutdown: dict[str, object] = {
                "signal": "SIGTERM",
                "startedMonotonicNs": time.monotonic_ns(),
                "forcedKill": False,
                "signalErrors": [],
            }
            try:
                os.killpg(process_group, signal.SIGTERM)
            except ProcessLookupError:
                pass
            except BaseException as exc:
                shutdown["signalErrors"].append(repr(exc))
            try:
                process.wait(timeout=min(1.0, args.shutdown_timeout))
            except subprocess.TimeoutExpired:
                pass
            except BaseException as exc:
                shutdown["signalErrors"].append(repr(exc))
            shutdown["groupGoneAfterTerm"] = wait_for_process_group_exit(
                process_group, args.shutdown_timeout
            )
            if not shutdown["groupGoneAfterTerm"]:
                shutdown["forcedKill"] = True
                try:
                    os.killpg(process_group, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                except BaseException as exc:
                    shutdown["signalErrors"].append(repr(exc))
            shutdown["groupGoneFinal"] = wait_for_process_group_exit(
                process_group, args.shutdown_timeout
            )
            try:
                if process.poll() is None:
                    process.wait(timeout=args.shutdown_timeout)
            except BaseException as exc:
                shutdown["signalErrors"].append(repr(exc))
            shutdown["finishedMonotonicNs"] = time.monotonic_ns()
            shutdown["returncode"] = process.returncode
            manifest["server"]["shutdown"] = shutdown
            if not shutdown["groupGoneFinal"] or shutdown["signalErrors"]:
                manifest["status"] = "failed"
                manifest["failures"].append(
                    f"process group cleanup incomplete: {shutdown!r}"
                )
                exit_code = 1
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
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--max-connections", type=int, default=1)
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
    _, exit_code = run(parse_args())
    raise SystemExit(exit_code)


if __name__ == "__main__":
    main()

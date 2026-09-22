#!/usr/bin/env python3
"""Qualify the single-worker accepted silent request-head budget on Darwin.

The runner deliberately keeps the raw sockets and all host/process output.  It
does not use SIGSTOP: the point of this qualification is to distinguish sockets
accepted by Obscura from connections which merely completed a TCP handshake in
the kernel listen queue.  The constants below mirror the private production
contract in ``obscura-cdp::server`` and are recorded in the manifest.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import datetime as dt
import hashlib
import json
import os
import platform
import resource
import signal
import socket
import subprocess
import sys
import time
import traceback
from pathlib import Path
from typing import Any, Iterable

try:
    from .cdp_capacity import (
        Evidence,
        discovery_request,
        masked_websocket_close_frame,
        masked_websocket_frame,
        parse_darwin_listen_queue,
        parse_http_response,
        parse_lsof_fd_count,
        read_to_eof,
        receive_websocket_frame,
        reserve_port,
        sha256_bytes,
        utc_now,
        wait_ready,
        websocket_request,
    )
except ImportError:
    from cdp_capacity import (
        Evidence,
        discovery_request,
        masked_websocket_close_frame,
        masked_websocket_frame,
        parse_darwin_listen_queue,
        parse_http_response,
        parse_lsof_fd_count,
        read_to_eof,
        receive_websocket_frame,
        reserve_port,
        sha256_bytes,
        utc_now,
        wait_ready,
        websocket_request,
    )


SILENT_PENDING_LIMIT = 256
CLASSIFICATION_RESERVE = 16
CLASSIFICATION_GRACE_SECONDS = 0.100
SILENT_TTL_SECONDS = 10.0
DEFAULT_TIMEOUT = 10.0
DEFAULT_SHUTDOWN_TIMEOUT = 15.0
DEFAULT_TTL_GRACE = 2.0
DEFAULT_SAMPLE_WINDOW = 3.0
DEFAULT_NORMAL_PROBES = 8
PENDING_LIMIT_REASON = "max-pending-request-heads"
PENDING_TIMEOUT_REASON = "request-head-timeout"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def parse_darwin_thread_count(raw: bytes) -> int | None:
    rows = []
    for line in raw.decode("latin-1").splitlines():
        stripped = line.strip()
        if not stripped or stripped.upper().startswith(("USER", "PID")):
            continue
        rows.append(stripped)
    return len(rows) if rows else None


def parse_process_cpu(raw: bytes) -> dict[str, object] | None:
    fields = raw.decode("latin-1").strip().split(maxsplit=7)
    if len(fields) < 7:
        return None
    try:
        return {
            "pid": int(fields[0]),
            "state": fields[1],
            "cpuPercent": float(fields[2]),
            "rssKiB": int(fields[3]),
            "vszKiB": int(fields[4]),
            "elapsed": fields[5],
            "cpuTime": fields[6],
            "command": fields[7] if len(fields) == 8 else "",
        }
    except (TypeError, ValueError):
        return None


def parse_cpu_time(value: str) -> float | None:
    """Parse Darwin ps time fields such as 00:01.23 or 1-02:03:04."""
    try:
        days = 0
        if "-" in value:
            day_text, value = value.split("-", 1)
            days = int(day_text)
        parts = value.split(":")
        if len(parts) == 2:
            minutes, seconds = parts
            return days * 86400 + int(minutes) * 60 + float(seconds)
        if len(parts) == 3:
            hours, minutes, seconds = parts
            return days * 86400 + int(hours) * 3600 + int(minutes) * 60 + float(seconds)
    except (TypeError, ValueError):
        pass
    return None


def parse_lsof_target_tcp(raw: bytes, host: str, port: int) -> dict[str, object]:
    """Parse only the target listener's TCP rows; raw lsof remains in evidence."""
    endpoint = f"{host}:{port}"
    rows: list[dict[str, object]] = []
    for line in raw.decode("latin-1").splitlines():
        fields = line.split()
        if "TCP" not in fields:
            continue
        index = fields.index("TCP")
        name = fields[index + 1] if index + 1 < len(fields) else ""
        state = fields[index + 2] if index + 2 < len(fields) else None
        if not (name == endpoint or name.startswith(endpoint + "->")):
            continue
        rows.append({"name": name, "state": state, "rawLine": line})
    established = [row for row in rows if row["state"] == "(ESTABLISHED)"]
    listeners = [row for row in rows if row["state"] == "(LISTEN)"]
    return {
        "listenerCount": len(listeners),
        "establishedCount": len(established),
        "rows": rows,
    }


def headers_named(parsed: dict[str, object], name: str) -> list[str]:
    return [
        str(value)
        for key, value in parsed.get("headers", [])
        if str(key).lower() == name.lower()
    ]


def validate_http(result: dict[str, object], status: int, reason: str | None = None) -> list[str]:
    parsed = result.get("parsedResponse")
    failures: list[str] = []
    if result.get("error") is not None or result.get("sendError") is not None:
        failures.append("HTTP transport error")
    if result.get("recvError") is not None:
        failures.append("HTTP receive failed")
    if result.get("eof") is not True:
        failures.append("HTTP response did not reach EOF")
    if not isinstance(parsed, dict) or parsed.get("status") != status:
        failures.append(f"HTTP status is not {status}")
    if not isinstance(parsed, dict) or parsed.get("completeHead") is not True:
        failures.append("HTTP response head is incomplete")
    if not isinstance(parsed, dict) or parsed.get("contentLengthMatches") is not True:
        failures.append("HTTP response body is incomplete")
    if reason is not None and headers_named(parsed or {}, "X-Obscura-Reason") != [reason]:
        failures.append(f"unexpected X-Obscura-Reason for {reason}")
    if isinstance(parsed, dict) and parsed.get("parseError") is not None:
        failures.append("HTTP response parse failed")
    return failures


def validate_socket_group(
    label: str,
    open_results: list[dict[str, object]],
    sockets: list[tuple[int, socket.socket]],
    results: list[dict[str, object]] | None,
    expected: int,
    initial_payload_bytes: int | None = None,
) -> list[str]:
    """Reject a partial group: every requested socket must connect and finish."""
    failures: list[str] = []
    if len(open_results) != expected:
        failures.append(f"{label}: open result count {len(open_results)} != {expected}")
    connected = [item for item in open_results if item.get("connected") is True]
    if len(connected) != expected:
        failures.append(f"{label}: connected count {len(connected)} != {expected}")
    if len(sockets) != expected:
        failures.append(f"{label}: live socket count {len(sockets)} != {expected}")
    if results is not None and len(results) != expected:
        failures.append(f"{label}: result count {len(results)} != {expected}")
    if initial_payload_bytes is not None:
        for item in connected:
            if item.get("initialSendError") is not None:
                failures.append(f"{label}: initial send failed for index {item.get('index')}")
            if item.get("initialRequestSentBytes") != initial_payload_bytes:
                failures.append(f"{label}: initial send was incomplete for index {item.get('index')}")
    return failures


def validate_recovery(
    baseline: dict[str, object], snapshot: dict[str, object], label: str
) -> list[str]:
    failures = validate_snapshot(snapshot)
    queue = snapshot.get("listenQueue")
    if not isinstance(queue, dict) or queue.get("completed") != 0:
        failures.append(f"{label}: listener queue did not return to zero")
    target = snapshot.get("targetTcp")
    if not isinstance(target, dict) or target.get("establishedCount") != 0:
        failures.append(f"{label}: target ESTABLISHED count did not return to zero")
    if snapshot.get("serverFdCount") != baseline.get("serverFdCount"):
        failures.append(f"{label}: server FD count did not return to baseline")
    baseline_threads = baseline.get("threads")
    threads = snapshot.get("threads")
    if (
        not isinstance(baseline_threads, dict)
        or not isinstance(threads, dict)
        or threads.get("threadCount") != baseline_threads.get("threadCount")
    ):
        failures.append(f"{label}: server thread count did not return to baseline")
    return failures


def validate_ttl_timing(
    result: dict[str, object],
    label: str,
    grace_seconds: float,
) -> list[str]:
    failures: list[str] = []
    connection = result.get("connection")
    connect_started_ns = connection.get("connectStartedMonotonicNs") if isinstance(connection, dict) else None
    connected_ns = connection.get("connectedMonotonicNs") if isinstance(connection, dict) else None
    first_observed_ns = result.get("firstObservedMonotonicNs")
    if (
        not isinstance(connect_started_ns, int)
        or not isinstance(connected_ns, int)
        or not isinstance(first_observed_ns, int)
    ):
        return [f"{label}: missing connection/first-observation monotonic timestamps"]
    lower_elapsed = (first_observed_ns - connect_started_ns) / 1_000_000_000
    upper_elapsed = (first_observed_ns - connected_ns) / 1_000_000_000
    if lower_elapsed < SILENT_TTL_SECONDS:
        failures.append(f"{label}: response/EOF arrived before production TTL")
    if upper_elapsed > SILENT_TTL_SECONDS + grace_seconds + 0.25:
        failures.append(f"{label}: response/EOF exceeded TTL plus grace tolerance")
    return failures


def validate_cdp(result: dict[str, object], command_id: int) -> list[str]:
    failures: list[str] = []
    if result.get("error") is not None or result.get("commandError") is not None:
        failures.append("WebSocket transport failed")
    parsed = result.get("parsedResponse")
    if not isinstance(parsed, dict) or parsed.get("status") != 101:
        failures.append("WebSocket did not upgrade with 101")
    value = result.get("serverCommandJson")
    if (
        result.get("commandJsonError") is not None
        or not isinstance(value, dict)
        or value.get("id") != command_id
        or not isinstance(value.get("result"), dict)
    ):
        failures.append("CDP response id/result is not successful")
    if result.get("closeError") is not None:
        failures.append("WebSocket did not reach clean EOF")
    return failures


def validate_snapshot(snapshot: dict[str, object]) -> list[str]:
    failures: list[str] = []
    if snapshot.get("serverFdCount") is None:
        failures.append("server FD count missing")
    if snapshot.get("listenQueue") is None:
        failures.append("listen queue missing")
    if snapshot.get("process") is None:
        failures.append("process snapshot missing")
    threads = snapshot.get("threads")
    if not isinstance(threads, dict) or threads.get("threadCount") is None:
        failures.append("thread count missing")
    target = snapshot.get("targetTcp")
    if not isinstance(target, dict) or target.get("listenerCount") != 1:
        failures.append("target listener TCP row is missing or ambiguous")
    cpu = snapshot.get("cpu")
    if not isinstance(cpu, dict) or cpu.get("cpuPercent") is None:
        failures.append("CPU sample missing")
    return failures


def validate_accepted_barrier(
    baseline: dict[str, object], snapshot: dict[str, object], expected: int
) -> list[str]:
    failures = validate_snapshot(snapshot)
    queue = snapshot.get("listenQueue")
    if not isinstance(queue, dict) or queue.get("completed") != 0:
        failures.append("listener queue is not empty at accepted barrier")
    target = snapshot.get("targetTcp")
    if not isinstance(target, dict) or target.get("establishedCount") != expected:
        failures.append("target ESTABLISHED count does not equal accepted count")
    baseline_fd = baseline.get("serverFdCount")
    if isinstance(baseline_fd, int) and isinstance(snapshot.get("serverFdCount"), int):
        if snapshot["serverFdCount"] - baseline_fd < expected:  # type: ignore[operator]
            failures.append("server FD delta is smaller than accepted count")
    return failures


def validate_stable_barriers(
    snapshots: Iterable[dict[str, object]], expected_established: int
) -> list[str]:
    values = list(snapshots)
    failures: list[str] = []
    if len(values) < 2:
        return ["fewer than two stability snapshots"]
    for snapshot in values:
        failures.extend(validate_snapshot(snapshot))
        target = snapshot.get("targetTcp")
        if isinstance(target, dict) and target.get("establishedCount") != expected_established:
            failures.append("target ESTABLISHED count changed during stability window")
        queue = snapshot.get("listenQueue")
        if isinstance(queue, dict) and queue.get("completed") != 0:
            failures.append("listen queue became non-empty during stability window")
    fds = [item.get("serverFdCount") for item in values]
    threads = [
        item.get("threadCount")
        for item in (
            snapshot.get("threads") if isinstance(snapshot.get("threads"), dict) else {}
            for snapshot in values
        )
    ]
    if len(set(fds)) != 1:
        failures.append("server FD count was not stable")
    if len(set(threads)) != 1:
        failures.append("server thread count was not stable")
    return failures


def validate_close_result(result: dict[str, object], require_empty: bool = False) -> list[str]:
    failures: list[str] = []
    if result.get("recvError") is not None:
        failures.append("socket close read failed")
    if result.get("eof") is not True:
        failures.append("socket did not reach EOF")
    wire = result.get("wire")
    if require_empty and (not isinstance(wire, dict) or wire.get("bytes") != 0):
        failures.append("silent socket produced an unexpected response wire")
    return failures


def capture_snapshot(evidence: Evidence, phase: str, pid: int, host: str, port: int) -> dict[str, object]:
    result: dict[str, object] = {"phase": phase, "capturedUtc": utc_now()}
    evidence.capture_command(phase, ["uname", "-a"])
    evidence.capture_command(phase, ["sysctl", "kern.ipc.somaxconn"])
    _, ps_raw = evidence.capture_command(
        phase,
        ["ps", "-o", "pid=,state=,%cpu=,rss=,vsz=,etime=,time=,command=", "-p", str(pid)],
    )
    _, threads_raw = evidence.capture_command(phase, ["ps", "-M", "-p", str(pid)])
    _, lsof_raw = evidence.capture_command(phase, ["lsof", "-nP", "-a", "-p", str(pid)])
    _, queue_raw = evidence.capture_command(phase, ["netstat", "-Lan", "-p", "tcp"])
    evidence.capture_command(phase, ["netstat", "-anv", "-p", "tcp"])
    result["process"] = parse_process_cpu(ps_raw)
    result["cpu"] = result["process"]
    result["serverFdCount"] = parse_lsof_fd_count(lsof_raw)
    result["listenQueue"] = parse_darwin_listen_queue(queue_raw, host, port)
    result["threads"] = {
        "threadCount": parse_darwin_thread_count(threads_raw),
        "rawSha256": sha256_bytes(threads_raw),
        "rawBytes": len(threads_raw),
    }
    result["targetTcp"] = parse_lsof_target_tcp(lsof_raw, host, port)
    result["rawSha256"] = {
        "ps": sha256_bytes(ps_raw),
        "threads": sha256_bytes(threads_raw),
        "lsof": sha256_bytes(lsof_raw),
        "listenQueue": sha256_bytes(queue_raw),
    }
    path = evidence.write_json(f"host/{phase}/snapshot.json", result)
    result["snapshot"] = evidence.artifacts[path]
    return result


def write_trace(evidence: Evidence, relative: str, exc: BaseException) -> dict[str, object]:
    path = evidence.write_bytes(relative, traceback.format_exc().encode("utf-8"))
    return {"exception": repr(exc), "traceback": evidence.artifacts[path]}


def connect_client(host: str, port: int, timeout: float) -> tuple[socket.socket | None, dict[str, object]]:
    result: dict[str, object] = {
        "connectStartedMonotonicNs": time.monotonic_ns(),
        "connected": False,
        "localAddress": None,
        "remoteAddress": None,
        "connectError": None,
        "connectErrorType": None,
        "connectErrorArgs": None,
    }
    connection: socket.socket | None = None
    try:
        connection = socket.create_connection((host, port), timeout=timeout)
        connection.settimeout(timeout)
        result["connected"] = True
        result["localAddress"] = list(connection.getsockname())
        result["remoteAddress"] = list(connection.getpeername())
    except BaseException as exc:
        result["connectError"] = repr(exc)
        result["connectErrorType"] = type(exc).__name__
        result["connectErrorArgs"] = [repr(value) for value in getattr(exc, "args", ())]
        result["connectTraceback"] = traceback.format_exc()
        if connection is not None:
            connection.close()
            connection = None
    result["connectedMonotonicNs"] = time.monotonic_ns()
    return connection, result


def persist_open_results(
    evidence: Evidence,
    phase: str,
    results: list[dict[str, object]],
    request: bytes = b"",
) -> list[dict[str, object]]:
    """Persist every connect result, including failed connects and empty requests."""
    for result in results:
        index = int(result["index"])
        prefix = f"opens/{phase}/client-{index:04d}"
        request_path = evidence.write_bytes(f"{prefix}.request.bin", request)
        result["request"] = evidence.artifacts[request_path]
        if result.get("connectTraceback") is not None:
            traceback_path = evidence.write_bytes(
                f"{prefix}.traceback.txt", str(result["connectTraceback"]).encode()
            )
            result["connectTracebackArtifact"] = evidence.artifacts[traceback_path]
        if result.get("initialSendTraceback") is not None:
            traceback_path = evidence.write_bytes(
                f"{prefix}.initial-send.traceback.txt",
                str(result["initialSendTraceback"]).encode(),
            )
            result["initialSendTracebackArtifact"] = evidence.artifacts[traceback_path]
        result_path = evidence.write_json(f"{prefix}.result.json", result)
        result["result"] = evidence.artifacts[result_path]
    return results


def open_clients(
    host: str,
    port: int,
    count: int,
    timeout: float,
    initial_payload: bytes | None = None,
) -> tuple[list[tuple[int, socket.socket]], list[dict[str, object]]]:
    outputs: list[tuple[int, socket.socket | None, dict[str, object]]] = []

    def open_one(index: int) -> tuple[int, socket.socket | None, dict[str, object]]:
        connection, result = connect_client(host, port, timeout)
        result["index"] = index
        if connection is not None and initial_payload is not None:
            sent, error, trace = send_bytes(connection, initial_payload)
            result["initialRequestSentBytes"] = sent
            result["initialSendError"] = error
            if trace is not None:
                result["initialSendTraceback"] = trace
        return index, connection, result

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, count)) as pool:
        futures = [pool.submit(open_one, index) for index in range(count)]
        for future in futures:
            outputs.append(future.result())
    outputs.sort(key=lambda item: item[0])
    return (
        [(index, connection) for index, connection, _ in outputs if connection is not None],
        [result for _, _, result in outputs],
    )


def send_bytes(connection: socket.socket, payload: bytes) -> tuple[int, str | None, str | None]:
    sent = 0
    error = None
    trace = None
    try:
        while sent < len(payload):
            sent += connection.send(payload[sent:])
    except BaseException as exc:
        error = repr(exc)
        trace = traceback.format_exc()
    return sent, error, trace


def read_wire(connection: socket.socket, timeout: float) -> tuple[bytes, str | None, str | None, bool]:
    connection.settimeout(timeout)
    chunks: list[bytes] = []
    error: str | None = None
    trace: str | None = None
    eof = False
    try:
        while True:
            chunk = connection.recv(65_536)
            if not chunk:
                eof = True
                break
            chunks.append(chunk)
    except BaseException as exc:
        error = repr(exc)
        trace = traceback.format_exc()
    return b"".join(chunks), error, trace, eof


def read_wire_timed(
    connection: socket.socket, timeout: float
) -> tuple[bytes, str | None, str | None, bool, int, int | None, int]:
    connection.settimeout(timeout)
    started_ns = time.monotonic_ns()
    first_observed_ns: int | None = None
    chunks: list[bytes] = []
    error: str | None = None
    trace: str | None = None
    eof = False
    try:
        while True:
            chunk = connection.recv(65_536)
            observed_ns = time.monotonic_ns()
            if first_observed_ns is None:
                first_observed_ns = observed_ns
            if not chunk:
                eof = True
                break
            chunks.append(chunk)
    except BaseException as exc:
        error = repr(exc)
        trace = traceback.format_exc()
    return b"".join(chunks), error, trace, eof, started_ns, first_observed_ns, time.monotonic_ns()


def record_http(
    evidence: Evidence,
    name: str,
    connection: socket.socket,
    request: bytes,
    timeout: float,
) -> dict[str, object]:
    prefix = f"clients/{name}"
    request_path = evidence.write_bytes(f"{prefix}.request.bin", request)
    sent, send_error, send_trace = send_bytes(connection, request)
    return record_http_response(
        evidence, name, connection, request, request_path, sent, send_error, send_trace, timeout
    )


def record_http_response(
    evidence: Evidence,
    name: str,
    connection: socket.socket,
    request: bytes,
    request_path: str,
    sent: int,
    send_error: str | None,
    send_trace: str | None,
    timeout: float,
    connection_info: dict[str, object] | None = None,
) -> dict[str, object]:
    prefix = f"clients/{name}"
    wire, recv_error, recv_trace, eof = read_wire(connection, timeout)
    response_path = evidence.write_bytes(f"{prefix}.response.bin", wire)
    result: dict[str, object] = {
        "name": name,
        "requestSentBytes": sent,
        "requestBytes": len(request),
        "sendError": send_error,
        "recvError": recv_error,
        "eof": eof,
        "response": evidence.artifacts[response_path],
        "request": evidence.artifacts[request_path],
        "parsedResponse": parse_http_response(wire),
        "finishedMonotonicNs": time.monotonic_ns(),
    }
    if connection_info is not None:
        result["connection"] = connection_info
    if send_trace is not None:
        trace_path = evidence.write_bytes(f"{prefix}.send.traceback.txt", send_trace.encode())
        result["sendTraceback"] = evidence.artifacts[trace_path]
    if recv_trace is not None:
        trace_path = evidence.write_bytes(f"{prefix}.recv.traceback.txt", recv_trace.encode())
        result["recvTraceback"] = evidence.artifacts[trace_path]
    path = evidence.write_json(f"{prefix}.result.json", result)
    result["result"] = evidence.artifacts[path]
    return result


def record_ws(
    evidence: Evidence,
    name: str,
    connection: socket.socket,
    request: bytes,
    command_id: int,
    timeout: float,
    request_already_sent: bool = False,
    connection_info: dict[str, object] | None = None,
) -> dict[str, object]:
    prefix = f"clients/{name}"
    request_path = evidence.write_bytes(f"{prefix}.request.bin", request)
    response = b""
    command_frame = masked_websocket_frame(
        json.dumps({"id": command_id, "method": "Browser.getVersion"}, separators=(",", ":")).encode(),
        1,
        bytes((command_id & 255, 0x43, 0x65, 0x87)),
    )
    command_payload = json.dumps(
        {"id": command_id, "method": "Browser.getVersion"}, separators=(",", ":")
    ).encode()
    close_frame = masked_websocket_close_frame()
    command_response = b""
    command_payload_response = b""
    metadata: dict[str, object] = {}
    error = None
    command_error = None
    command_json_error = None
    close_wire = b""
    close_error = None
    head_trace = None
    try:
        if not request_already_sent:
            connection.sendall(request)
        response, error, head_trace = _read_http_head(connection, timeout)
        parsed = parse_http_response(response)
        if error is None and parsed.get("status") == 101:
            connection.sendall(command_frame)
            command_response, command_payload_response, metadata, command_error = receive_websocket_frame(connection)
            if command_payload_response:
                try:
                    command_json = json.loads(command_payload_response)
                except BaseException as exc:
                    command_json = None
                    command_json_error = repr(exc)
            else:
                command_json = None
            connection.sendall(close_frame)
            close_wire, close_error = read_to_eof(connection)
        else:
            command_json = None
    except BaseException as exc:
        error = repr(exc)
        command_json = None
        error_trace = traceback.format_exc()
    else:
        error_trace = None
    response_path = evidence.write_bytes(f"{prefix}.upgrade.response.bin", response)
    payload_path = evidence.write_bytes(f"{prefix}.command.payload.bin", command_payload)
    frame_path = evidence.write_bytes(f"{prefix}.command.frame.bin", command_frame)
    command_response_path = evidence.write_bytes(f"{prefix}.command.response.frame.bin", command_response)
    command_payload_path = evidence.write_bytes(f"{prefix}.command.response.payload.bin", command_payload_response)
    close_frame_path = evidence.write_bytes(f"{prefix}.close.frame.bin", close_frame)
    close_wire_path = evidence.write_bytes(f"{prefix}.close.response.bin", close_wire)
    result: dict[str, object] = {
        "name": name,
        "commandId": command_id,
        "error": error,
        "commandError": command_error,
        "commandJsonError": command_json_error,
        "closeError": close_error,
        "request": evidence.artifacts[request_path],
        "upgradeResponse": evidence.artifacts[response_path],
        "parsedResponse": parse_http_response(response),
        "clientCommandPayload": evidence.artifacts[payload_path],
        "clientCommandFrame": evidence.artifacts[frame_path],
        "serverCommandFrame": evidence.artifacts[command_response_path],
        "serverCommandPayload": evidence.artifacts[command_payload_path],
        "serverCommandMetadata": metadata,
        "serverCommandJson": command_json,
        "clientCloseFrame": evidence.artifacts[close_frame_path],
        "serverCloseWire": evidence.artifacts[close_wire_path],
        "finishedMonotonicNs": time.monotonic_ns(),
    }
    if connection_info is not None:
        result["connection"] = connection_info
    if error_trace is not None:
        trace_path = evidence.write_bytes(f"{prefix}.traceback.txt", error_trace.encode())
        result["traceback"] = evidence.artifacts[trace_path]
    if head_trace is not None:
        trace_path = evidence.write_bytes(f"{prefix}.head.traceback.txt", head_trace.encode())
        result["headTraceback"] = evidence.artifacts[trace_path]
    path = evidence.write_json(f"{prefix}.result.json", result)
    result["result"] = evidence.artifacts[path]
    return result


def _read_http_head(
    connection: socket.socket, timeout: float
) -> tuple[bytes, str | None, str | None]:
    connection.settimeout(timeout)
    response = bytearray()
    error = None
    trace = None
    try:
        while b"\r\n\r\n" not in response:
            chunk = connection.recv(65_536)
            if not chunk:
                break
            response.extend(chunk)
    except BaseException as exc:
        error = repr(exc)
        trace = traceback.format_exc()
    return bytes(response), error, trace


def record_close(evidence: Evidence, name: str, connection: socket.socket, timeout: float) -> dict[str, object]:
    prefix = f"clients/{name}"
    (
        wire,
        recv_error,
        recv_trace,
        eof,
        read_started_ns,
        first_observed_ns,
        read_finished_ns,
    ) = read_wire_timed(
        connection, timeout
    )
    path = evidence.write_bytes(f"{prefix}.server-wire.bin", wire)
    result: dict[str, object] = {
        "name": name,
        "wire": evidence.artifacts[path],
        "recvError": recv_error,
        "eof": eof,
        "readStartedMonotonicNs": read_started_ns,
        "firstObservedMonotonicNs": first_observed_ns,
        "readFinishedMonotonicNs": read_finished_ns,
        "finishedMonotonicNs": time.monotonic_ns(),
    }
    if recv_trace is not None:
        trace_path = evidence.write_bytes(f"{prefix}.traceback.txt", recv_trace.encode())
        result["traceback"] = evidence.artifacts[trace_path]
    result_path = evidence.write_json(f"{prefix}.close.result.json", result)
    result["result"] = evidence.artifacts[result_path]
    return result


class OwnedSocketRegistry:
    """Own every live client socket until its wire is durably recorded."""

    def __init__(self, evidence: Evidence, timeout: float) -> None:
        self.evidence = evidence
        self.timeout = timeout
        self._entries: dict[int, tuple[str, socket.socket]] = {}
        self._next_id = 0

    def add(self, phase: str, sockets: list[tuple[int, socket.socket]]) -> None:
        for index, connection in sockets:
            token = self._next_id
            self._next_id += 1
            self._entries[id(connection)] = (f"{phase}-{index:04d}-{token:04d}", connection)

    def release(self, connection: socket.socket) -> None:
        self._entries.pop(id(connection), None)
        connection.close()

    def forget(self, sockets: list[tuple[int, socket.socket]]) -> None:
        for _, connection in sockets:
            self._entries.pop(id(connection), None)

    def salvage(self, phase: str = "exception-final") -> list[dict[str, object]]:
        entries = list(self._entries.values())

        def salvage_one(item: tuple[str, socket.socket]) -> dict[str, object]:
            name, connection = item
            try:
                result = record_close(self.evidence, f"{phase}-{name}", connection, self.timeout)
            except BaseException as exc:
                prefix = f"clients/{phase}-{name}"
                traceback_path = self.evidence.write_bytes(
                    f"{prefix}.traceback.txt", traceback.format_exc().encode("utf-8")
                )
                wire_path = self.evidence.write_bytes(f"{prefix}.server-wire.bin", b"")
                result = {
                    "name": name,
                    "wire": self.evidence.artifacts[wire_path],
                    "recvError": repr(exc),
                    "eof": False,
                    "readStartedMonotonicNs": None,
                    "firstObservedMonotonicNs": None,
                    "readFinishedMonotonicNs": time.monotonic_ns(),
                    "traceback": self.evidence.artifacts[traceback_path],
                }
                result_path = self.evidence.write_json(f"{prefix}.close.result.json", result)
                result["result"] = self.evidence.artifacts[result_path]
            finally:
                connection.close()
                self._entries.pop(id(connection), None)
            return result

        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(entries))) as pool:
            return list(pool.map(salvage_one, entries))


def close_clients(
    evidence: Evidence,
    sockets: list[tuple[int, socket.socket]],
    names: dict[int, str],
    timeout: float,
    phase: str,
    connection_info: dict[int, dict[str, object]] | None = None,
) -> list[dict[str, object]]:
    outputs: list[dict[str, object]] = []

    def close_one(item: tuple[int, socket.socket]) -> dict[str, object]:
        index, connection = item
        try:
            result = record_close(
                evidence,
                f"{phase}-{names.get(index, index)}",
                connection,
                timeout,
            )
            if connection_info and index in connection_info:
                result["connection"] = connection_info[index]
                first_ns = result.get("firstObservedMonotonicNs")
                connect_started_ns = connection_info[index].get("connectStartedMonotonicNs")
                connected_ns = connection_info[index].get("connectedMonotonicNs")
                if isinstance(first_ns, int) and isinstance(connect_started_ns, int):
                    result["elapsedFromConnectStartedSeconds"] = (
                        first_ns - connect_started_ns
                    ) / 1_000_000_000
                if isinstance(first_ns, int) and isinstance(connected_ns, int):
                    result["elapsedFromConnectedSeconds"] = (
                        first_ns - connected_ns
                    ) / 1_000_000_000
        except BaseException as exc:
            prefix = f"clients/{phase}-{names.get(index, index)}"
            traceback_path = evidence.write_bytes(
                f"{prefix}.traceback.txt", traceback.format_exc().encode("utf-8")
            )
            wire_path = evidence.write_bytes(f"{prefix}.server-wire.bin", b"")
            result = {
                "name": names.get(index, index),
                "wire": evidence.artifacts[wire_path],
                "recvError": repr(exc),
                "eof": False,
                "readStartedMonotonicNs": None,
                "firstObservedMonotonicNs": None,
                "readFinishedMonotonicNs": time.monotonic_ns(),
                "traceback": evidence.artifacts[traceback_path],
            }
            if connection_info and index in connection_info:
                result["connection"] = connection_info[index]
            result_path = evidence.write_json(f"{prefix}.close.result.json", result)
            result["result"] = evidence.artifacts[result_path]
        finally:
            connection.close()
        result["index"] = index
        return result

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(sockets))) as pool:
        futures = [pool.submit(close_one, item) for item in sockets]
        for future in futures:
            outputs.append(future.result())
    return outputs


def wait_snapshot(
    evidence: Evidence,
    phase: str,
    pid: int,
    host: str,
    port: int,
    predicate,
    timeout: float,
    interval: float = 0.05,
) -> tuple[dict[str, object], list[dict[str, object]]]:
    deadline = time.monotonic() + timeout
    attempts: list[dict[str, object]] = []
    latest: dict[str, object] | None = None
    index = 0
    while time.monotonic() < deadline:
        index += 1
        latest = capture_snapshot(evidence, f"{phase}-attempt-{index:03d}", pid, host, port)
        attempts.append(latest)
        if predicate(latest):
            return latest, attempts
        time.sleep(interval)
    if latest is None:
        raise TimeoutError(f"no snapshot captured for {phase}")
    raise TimeoutError(f"{phase} timed out after {timeout:.3f}s")


def sample_cpu(
    evidence: Evidence,
    pid: int,
    host: str,
    port: int,
    phase: str,
    duration: float,
) -> list[dict[str, object]]:
    samples: list[dict[str, object]] = []
    deadline = time.monotonic() + duration
    index = 0
    while time.monotonic() < deadline or not samples:
        index += 1
        samples.append(capture_snapshot(evidence, f"{phase}-{index:03d}", pid, host, port))
        if time.monotonic() >= deadline:
            break
        time.sleep(0.25)
    return samples


def cpu_window(samples: list[dict[str, object]]) -> dict[str, object]:
    values = [sample.get("process") for sample in samples]
    cpu_times = [parse_cpu_time(item["cpuTime"]) for item in values if isinstance(item, dict) and item.get("cpuTime")]
    percentages = [float(item["cpuPercent"]) for item in values if isinstance(item, dict) and item.get("cpuPercent") is not None]
    return {
        "sampleCount": len(samples),
        "cpuTimeSecondsFirst": cpu_times[0] if cpu_times else None,
        "cpuTimeSecondsLast": cpu_times[-1] if cpu_times else None,
        "cpuTimeDeltaSeconds": (cpu_times[-1] - cpu_times[0]) if len(cpu_times) >= 2 else None,
        "cpuPercentMin": min(percentages) if percentages else None,
        "cpuPercentMax": max(percentages) if percentages else None,
        "cpuPercentMean": sum(percentages) / len(percentages) if percentages else None,
    }


def run(args: argparse.Namespace) -> tuple[dict[str, object], int]:
    evidence = Evidence(args.output)
    owned = OwnedSocketRegistry(evidence, args.timeout)
    manifest: dict[str, Any] = {
        "schemaVersion": 1,
        "status": "running",
        "startedUtc": utc_now(),
        "scope": {
            "qualified": [
                "Darwin IPv4 loopback single-worker accepted silent request-head budget",
                "observed classification-capacity responses and request-head TTL responses",
                "normal HTTP/WebSocket progress while silent peers are held",
            ],
            "notQualified": [
                "Linux, Windows, containers, multi-worker relay, kernel backlog or total RSS",
                "a portable CPU or FD limit beyond the recorded host facts",
                "simultaneous 16-slot classification-reserve occupancy or the 272-slot hard-total state",
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
            "workers": 1,
            "maxConnections": args.max_connections,
            "silentPendingLimit": SILENT_PENDING_LIMIT,
            "classificationReserve": CLASSIFICATION_RESERVE,
            "classificationGraceSeconds": CLASSIFICATION_GRACE_SECONDS,
            "silentTtlSeconds": SILENT_TTL_SECONDS,
            "ttlGraceSeconds": args.ttl_grace_seconds,
            "normalProbeCount": args.normal_probes,
            "timeoutSeconds": args.timeout,
            "sampleWindowSeconds": args.sample_window_seconds,
        },
        "binary": {},
        "server": {},
        "phases": {},
        "assertions": [],
        "failures": [],
    }
    process: subprocess.Popen[bytes] | None = None
    stdout_handle = None
    stderr_handle = None
    silent_sockets: list[tuple[int, socket.socket]] = []
    exit_code = 1
    try:
        if platform.system() != "Darwin":
            raise RuntimeError("this evidence runner requires Darwin")
        if not args.binary.is_file():
            raise FileNotFoundError(args.binary)
        if args.pending != SILENT_PENDING_LIMIT:
            raise ValueError(f"--pending must equal the production limit {SILENT_PENDING_LIMIT}")
        port = args.port or reserve_port(args.host)
        manifest["configuration"]["port"] = port
        manifest["binary"] = {
            "path": str(args.binary.resolve()),
            "sha256": sha256_file(args.binary),
            "size": args.binary.stat().st_size,
        }
        command = [
            str(args.binary.resolve()), "--persona", args.persona, "serve",
            "--host", args.host, "--port", str(port), "--workers", "1",
            "--max-connections", str(args.max_connections),
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
        manifest["phases"]["readiness"] = wait_ready(evidence, process, args.host, port, args.timeout)
        baseline = capture_snapshot(evidence, "baseline", process.pid, args.host, port)
        manifest["phases"]["baseline"] = baseline
        manifest["failures"].extend(validate_snapshot(baseline))

        # Fill the ordinary silent budget and wait for an application-level
        # barrier: no listener queue entry and exactly N ESTABLISHED rows.
        silent_sockets, silent_results = open_clients(args.host, port, args.pending, args.timeout)
        owned.add("accepted", silent_sockets)
        persist_open_results(evidence, "accepted", silent_results)
        manifest["phases"]["acceptedOpen"] = {"clients": silent_results}
        manifest["failures"].extend(
            validate_socket_group("accepted", silent_results, silent_sockets, None, args.pending)
        )
        accepted, accepted_attempts = wait_snapshot(
            evidence,
            "accepted-barrier",
            process.pid,
            args.host,
            port,
            lambda snapshot: not validate_accepted_barrier(baseline, snapshot, args.pending),
            args.timeout,
        )
        manifest["phases"]["acceptedBarrier"] = {"snapshot": accepted, "attempts": accepted_attempts}
        manifest["failures"].extend(validate_accepted_barrier(baseline, accepted, args.pending))

        stable = sample_cpu(evidence, process.pid, args.host, port, "accepted-cpu", args.sample_window_seconds)
        manifest["phases"]["acceptedCpu"] = {"samples": stable, "summary": cpu_window(stable)}
        manifest["failures"].extend(
            failure for sample in stable for failure in validate_snapshot(sample)
        )
        manifest["failures"].extend(validate_stable_barriers(stable, args.pending))

        # Complete requests use the classification reserve while all ordinary
        # silent slots are occupied. Keep the probe count below the reserve.
        http_request = discovery_request(args.host, port)
        ws_request = websocket_request(args.host, port)
        http_sockets, http_open = open_clients(
            args.host, port, args.normal_probes, args.timeout, http_request
        )
        owned.add("classification-http", http_sockets)
        ws_sockets, ws_open = open_clients(
            args.host, port, max(1, args.normal_probes // 2), args.timeout, ws_request
        )
        owned.add("classification-ws", ws_sockets)
        persist_open_results(evidence, "classification-http", http_open, http_request)
        persist_open_results(evidence, "classification-ws", ws_open, ws_request)
        http_results: list[dict[str, object]] = []
        ws_results: list[dict[str, object]] = []
        for index, connection in http_sockets:
            opened = next(result for result in http_open if result["index"] == index)
            completed = False
            try:
                result = record_http_response(
                    evidence,
                    f"cap-http-{index:03d}",
                    connection,
                    http_request,
                    opened["request"]["path"],
                    int(opened.get("initialRequestSentBytes", 0)),
                    opened.get("initialSendError"),
                    opened.get("initialSendTraceback"),
                    args.timeout,
                    connection_info=opened,
                )
                completed = True
            finally:
                if completed:
                    owned.release(connection)
            http_results.append(result)
        for index, connection in ws_sockets:
            completed = False
            try:
                result = record_ws(
                    evidence,
                    f"cap-ws-{index:03d}",
                    connection,
                    ws_request,
                    index + 1,
                    args.timeout,
                    request_already_sent=True,
                    connection_info=next(result for result in ws_open if result["index"] == index),
                )
                completed = True
            finally:
                if completed:
                    owned.release(connection)
            ws_results.append(result)
        manifest["phases"]["classificationSuccess"] = {
            "httpOpen": http_open,
            "http": http_results,
            "websocketOpen": ws_open,
            "websocket": ws_results,
        }
        for result in http_results:
            manifest["failures"].extend(validate_http(result, 200))
        for result in ws_results:
            manifest["failures"].extend(validate_cdp(result, int(result["commandId"])))
        manifest["failures"].extend(
            validate_socket_group(
                "classification-http", http_open, http_sockets, http_results,
                args.normal_probes, len(http_request),
            )
        )
        manifest["failures"].extend(
            validate_socket_group(
                "classification-ws", ws_open, ws_sockets, ws_results,
                max(1, args.normal_probes // 2), len(ws_request),
            )
        )

        # Reserve saturation: 16 incomplete heads and one extra incomplete
        # head must all receive complete 503 responses rather than linger.
        partial = b"GET /json/version HTTP/1.1\r\nHost: " + f"{args.host}:{port}".encode() + b"\r\n"
        reserve_partial_sockets, reserve_partial_open = open_clients(
            args.host, port, CLASSIFICATION_RESERVE, args.timeout, partial
        )
        owned.add("classification-overflow-partial", reserve_partial_sockets)
        reserve_silent_sockets_raw, reserve_silent_open_raw = open_clients(
            args.host, port, 1, args.timeout
        )
        owned.add("classification-overflow-silent", reserve_silent_sockets_raw)
        reserve_silent_sockets = [
            (index + CLASSIFICATION_RESERVE, connection)
            for index, connection in reserve_silent_sockets_raw
        ]
        reserve_silent_open = []
        for result in reserve_silent_open_raw:
            result["index"] = int(result["index"]) + CLASSIFICATION_RESERVE
            reserve_silent_open.append(result)
        reserve_sockets = reserve_partial_sockets + reserve_silent_sockets
        reserve_open = reserve_partial_open + reserve_silent_open
        reserve_sockets.sort(key=lambda item: item[0])
        reserve_open.sort(key=lambda item: item["index"])
        persist_open_results(evidence, "classification-overflow-partial", reserve_partial_open, partial)
        persist_open_results(evidence, "classification-overflow-silent", reserve_silent_open)
        reserve_results: list[dict[str, object]] = []
        for index, connection in reserve_sockets:
            opened = next(result for result in reserve_open if result["index"] == index)
            completed = False
            try:
                reserve_results.append(
                    record_http_response(
                        evidence,
                        f"reserve-{index:03d}",
                        connection,
                        partial if index < CLASSIFICATION_RESERVE else b"",
                        opened["request"]["path"],
                        int(opened.get("initialRequestSentBytes", 0)),
                        opened.get("initialSendError"),
                        opened.get("initialSendTraceback"),
                        args.timeout,
                        connection_info=opened,
                    )
                )
                completed = True
            finally:
                if completed:
                    owned.release(connection)
        manifest["phases"]["classificationOverflow"] = {
            "open": reserve_open,
            "results": reserve_results,
        }
        for result in reserve_results:
            manifest["failures"].extend(validate_http(result, 503, PENDING_LIMIT_REASON))
        manifest["failures"].extend(
            validate_socket_group(
                "classification-overflow", reserve_open, reserve_sockets, reserve_results,
                CLASSIFICATION_RESERVE + 1,
            )
        )
        manifest["failures"].extend(
            validate_socket_group(
                "classification-overflow-partial", reserve_partial_open, reserve_partial_sockets,
                None, CLASSIFICATION_RESERVE, len(partial),
            )
        )
        manifest["failures"].extend(
            validate_socket_group(
                "classification-overflow-silent", reserve_silent_open, reserve_silent_sockets,
                None, 1,
            )
        )

        # Drop the silent clients deliberately before the independent TTL row.
        close_results = close_clients(
            evidence, silent_sockets, {index: f"silent-{index:03d}" for index, _ in silent_sockets}, args.timeout, "cap-close"
        )
        manifest["phases"]["acceptedClose"] = {"results": close_results}
        for result in close_results:
            manifest["failures"].extend(validate_close_result(result))
        owned.forget(silent_sockets)
        silent_sockets = []
        recovered, recovery_attempts = wait_snapshot(
            evidence, "after-cap", process.pid, args.host, port,
            lambda snapshot: (
                isinstance(snapshot.get("listenQueue"), dict)
                and snapshot["listenQueue"].get("completed") == 0  # type: ignore[index]
                and snapshot.get("serverFdCount") == baseline.get("serverFdCount")
                and isinstance(snapshot.get("targetTcp"), dict)
                and snapshot["targetTcp"].get("establishedCount") == 0  # type: ignore[index]
            ),
            args.timeout,
        )
        manifest["phases"]["afterCap"] = {"snapshot": recovered, "attempts": recovery_attempts}
        manifest["failures"].extend(validate_recovery(baseline, recovered, "after-cap"))

        # First prove that a base partial head can complete into HTTP 200 before its TTL.
        base_partial_sockets, base_partial_open = open_clients(args.host, port, args.pending - 1, args.timeout)
        owned.add("partial-completion-base", base_partial_sockets)
        partial_sockets, partial_open = open_clients(args.host, port, 1, args.timeout)
        owned.add("partial-completion", partial_sockets)
        persist_open_results(evidence, "partial-completion-base", base_partial_open)
        persist_open_results(evidence, "partial-completion", partial_open)
        manifest["failures"].extend(
            validate_socket_group("partial-completion-base", base_partial_open, base_partial_sockets, None, args.pending - 1)
        )
        manifest["failures"].extend(
            validate_socket_group("partial-completion", partial_open, partial_sockets, None, 1)
        )
        if not partial_sockets:
            raise RuntimeError("partial completion socket failed to connect")
        partial_socket = partial_sockets[0][1]
        partial_index = partial_sockets[0][0]
        partial_open_info = next(result for result in partial_open if result["index"] == partial_index)
        completion_barrier, completion_attempts = wait_snapshot(
            evidence,
            "partial-completion-barrier",
            process.pid,
            args.host,
            port,
            lambda snapshot: not validate_accepted_barrier(baseline, snapshot, args.pending),
            args.timeout,
        )
        manifest["phases"]["partialCompletionBarrier"] = {
            "snapshot": completion_barrier,
            "attempts": completion_attempts,
        }
        manifest["failures"].extend(
            validate_accepted_barrier(baseline, completion_barrier, args.pending)
        )
        completion_request = partial + b"\r\n"
        completion_request_path = evidence.write_bytes(
            f"clients/partial-completion-{partial_index:03d}.request.bin", completion_request
        )
        completion_prefix_path = evidence.write_bytes(
            f"clients/partial-completion-{partial_index:03d}.prefix.bin", partial
        )
        completion_suffix_path = evidence.write_bytes(
            f"clients/partial-completion-{partial_index:03d}.completion-suffix.bin", b"\r\n"
        )
        sent_partial, partial_send_error, partial_send_trace = send_bytes(partial_socket, partial)
        sent_final, final_send_error, final_send_trace = send_bytes(partial_socket, b"\r\n")
        completion_result = record_http_response(
            evidence,
            f"partial-completion-{partial_index:03d}",
            partial_socket,
            completion_request,
            completion_request_path,
            sent_partial + sent_final,
            partial_send_error or final_send_error,
            partial_send_trace or final_send_trace,
            args.timeout,
            connection_info=partial_open_info,
        )
        manifest["phases"]["partialCompletion"] = completion_result
        manifest["phases"]["partialCompletion"]["prefix"] = evidence.artifacts[completion_prefix_path]
        manifest["phases"]["partialCompletion"]["completionSuffix"] = evidence.artifacts[completion_suffix_path]
        manifest["failures"].extend(validate_http(completion_result, 200))
        owned.release(partial_socket)
        completion_close_results = close_clients(
            evidence,
            base_partial_sockets,
            {index: f"partial-completion-base-{index:03d}" for index, _ in base_partial_sockets},
            args.timeout,
            "partial-completion-base-close",
        )
        manifest["phases"]["partialCompletionBaseClose"] = {"results": completion_close_results}
        for result in completion_close_results:
            manifest["failures"].extend(validate_close_result(result))
        owned.forget(base_partial_sockets)
        completion_recovered, completion_recovery_attempts = wait_snapshot(
            evidence,
            "after-partial-completion",
            process.pid,
            args.host,
            port,
            lambda snapshot: (
                isinstance(snapshot.get("listenQueue"), dict)
                and snapshot["listenQueue"].get("completed") == 0  # type: ignore[index]
                and snapshot.get("serverFdCount") == baseline.get("serverFdCount")
                and isinstance(snapshot.get("targetTcp"), dict)
                and snapshot["targetTcp"].get("establishedCount") == 0  # type: ignore[index]
            ),
            args.timeout,
        )
        manifest["phases"]["afterPartialCompletion"] = {
            "snapshot": completion_recovered,
            "attempts": completion_recovery_attempts,
        }
        manifest["failures"].extend(
            validate_recovery(baseline, completion_recovered, "after-partial-completion")
        )

        # A base partial head receives the actual 408 after the ten-second TTL.
        base_partial_sockets, base_partial_open = open_clients(args.host, port, args.pending - 1, args.timeout)
        owned.add("partial-ttl-base", base_partial_sockets)
        partial_sockets, partial_open = open_clients(args.host, port, 1, args.timeout)
        owned.add("partial-ttl", partial_sockets)
        persist_open_results(evidence, "partial-ttl-base", base_partial_open)
        persist_open_results(evidence, "partial-ttl", partial_open)
        manifest["failures"].extend(
            validate_socket_group("partial-ttl-base", base_partial_open, base_partial_sockets, None, args.pending - 1)
        )
        manifest["failures"].extend(
            validate_socket_group("partial-ttl", partial_open, partial_sockets, None, 1)
        )
        if not partial_sockets:
            raise RuntimeError("partial TTL socket failed to connect")
        partial_socket = partial_sockets[0][1]
        partial_index = partial_sockets[0][0]
        partial_ttl_open_info = next(result for result in partial_open if result["index"] == partial_index)
        ttl_barrier, ttl_barrier_attempts = wait_snapshot(
            evidence,
            "partial-ttl-barrier",
            process.pid,
            args.host,
            port,
            lambda snapshot: not validate_accepted_barrier(baseline, snapshot, args.pending),
            args.timeout,
        )
        manifest["phases"]["partialTtlBarrier"] = {
            "snapshot": ttl_barrier,
            "attempts": ttl_barrier_attempts,
        }
        manifest["failures"].extend(validate_accepted_barrier(baseline, ttl_barrier, args.pending))
        connect_started_monotonic_ns = partial_ttl_open_info.get("connectStartedMonotonicNs")
        connected_monotonic_ns = partial_ttl_open_info.get("connectedMonotonicNs")
        partial_send_started_monotonic_ns = time.monotonic_ns()
        if not isinstance(connect_started_monotonic_ns, int):
            manifest["failures"].append("partial TTL connection missing connectStartedMonotonicNs")
            connect_started_monotonic_ns = partial_send_started_monotonic_ns
        if not isinstance(connected_monotonic_ns, int):
            manifest["failures"].append("partial TTL connection missing connectedMonotonicNs")
            connected_monotonic_ns = partial_send_started_monotonic_ns
        partial_path = evidence.write_bytes(
            f"clients/partial-ttl-{partial_index:03d}.request.bin", partial
        )
        partial_prefix_path = evidence.write_bytes(
            f"clients/partial-ttl-{partial_index:03d}.prefix.bin", partial
        )
        sent, send_error, send_trace = send_bytes(partial_socket, partial)
        manifest["phases"]["partialTtlOpen"] = {
            "baseOpen": base_partial_open,
            "partialOpen": partial_open,
            "request": evidence.artifacts[partial_path],
            "prefix": evidence.artifacts[partial_prefix_path],
            "sentBytes": sent,
            "sendError": send_error,
            "ttlStartConnectStartedMonotonicNs": connect_started_monotonic_ns,
            "ttlStartConnectedMonotonicNs": connected_monotonic_ns,
            "partialSendStartedMonotonicNs": partial_send_started_monotonic_ns,
            "connection": partial_ttl_open_info,
        }
        if send_trace is not None:
            trace_path = evidence.write_bytes("clients/partial-ttl.send.traceback.txt", send_trace.encode())
            manifest["phases"]["partialTtlOpen"]["traceback"] = evidence.artifacts[trace_path]
        (
            ttl_wire,
            ttl_error,
            ttl_trace,
            ttl_eof,
            ttl_read_started_ns,
            ttl_first_observed_ns,
            ttl_read_finished_ns,
        ) = read_wire_timed(
            partial_socket, max(args.timeout, SILENT_TTL_SECONDS + args.ttl_grace_seconds + 1.0)
        )
        ttl_elapsed_from_connect_started = (
            ttl_first_observed_ns - connect_started_monotonic_ns
        ) / 1_000_000_000 if isinstance(ttl_first_observed_ns, int) else None
        ttl_elapsed_from_connected = (
            ttl_first_observed_ns - connected_monotonic_ns
        ) / 1_000_000_000 if isinstance(ttl_first_observed_ns, int) else None
        ttl_response_path = evidence.write_bytes("clients/partial-ttl.response.bin", ttl_wire)
        ttl_result: dict[str, object] = {
            "sentBytes": sent,
            "response": evidence.artifacts[ttl_response_path],
            "recvError": ttl_error,
            "eof": ttl_eof,
            "readStartedMonotonicNs": ttl_read_started_ns,
            "firstObservedMonotonicNs": ttl_first_observed_ns,
            "readFinishedMonotonicNs": ttl_read_finished_ns,
            "elapsedFromConnectStartedSeconds": ttl_elapsed_from_connect_started,
            "elapsedFromConnectedSeconds": ttl_elapsed_from_connected,
            "ttlStartConnectStartedMonotonicNs": connect_started_monotonic_ns,
            "ttlStartConnectedMonotonicNs": connected_monotonic_ns,
            "partialSendStartedMonotonicNs": partial_send_started_monotonic_ns,
            "connection": partial_ttl_open_info,
            "parsedResponse": parse_http_response(ttl_wire),
        }
        if ttl_trace is not None:
            trace_path = evidence.write_bytes("clients/partial-ttl.recv.traceback.txt", ttl_trace.encode())
            ttl_result["traceback"] = evidence.artifacts[trace_path]
        ttl_result_path = evidence.write_json("clients/partial-ttl.result.json", ttl_result)
        ttl_result["result"] = evidence.artifacts[ttl_result_path]
        manifest["phases"]["partialTtl"] = ttl_result
        manifest["failures"].extend(validate_http(ttl_result, 408, PENDING_TIMEOUT_REASON))
        manifest["failures"].extend(
            validate_ttl_timing(ttl_result, "partial TTL", args.ttl_grace_seconds)
        )
        owned.release(partial_socket)
        base_close_results = close_clients(
            evidence, base_partial_sockets,
            {index: f"partial-base-{index:03d}" for index, _ in base_partial_sockets}, args.timeout, "partial-base-close"
        )
        manifest["phases"]["partialBaseClose"] = {"results": base_close_results}
        for result in base_close_results:
            manifest["failures"].extend(validate_close_result(result))
        owned.forget(base_partial_sockets)

        # Zero-byte silent TTL: all server-side accepted sockets must close by
        # the server, then the host facts must return to the initial baseline.
        silent_sockets, silent_results = open_clients(args.host, port, args.pending, args.timeout)
        owned.add("zero-byte", silent_sockets)
        persist_open_results(evidence, "zero-byte", silent_results)
        manifest["phases"]["zeroByteOpen"] = {"clients": silent_results}
        manifest["failures"].extend(
            validate_socket_group("zero-byte", silent_results, silent_sockets, None, args.pending)
        )
        zero_barrier, zero_attempts = wait_snapshot(
            evidence, "zero-byte-barrier", process.pid, args.host, port,
            lambda snapshot: not validate_accepted_barrier(baseline, snapshot, args.pending), args.timeout
        )
        manifest["phases"]["zeroByteBarrier"] = {"snapshot": zero_barrier, "attempts": zero_attempts}
        manifest["failures"].extend(validate_accepted_barrier(baseline, zero_barrier, args.pending))
        zero_close_results = close_clients(
            evidence,
            silent_sockets,
            {index: f"zero-byte-{index:03d}" for index, _ in silent_sockets},
            max(args.timeout, SILENT_TTL_SECONDS + args.ttl_grace_seconds + 1.0),
            "zero-byte-ttl",
            {result["index"]: result for result in silent_results},
        )
        manifest["phases"]["zeroByteTtl"] = {"results": zero_close_results}
        for result in zero_close_results:
            manifest["failures"].extend(validate_close_result(result, require_empty=True))
            manifest["failures"].extend(
                validate_ttl_timing(result, "zero-byte TTL", args.ttl_grace_seconds)
            )
        owned.forget(silent_sockets)
        silent_sockets = []
        recovered, recovery_attempts = wait_snapshot(
            evidence, "zero-byte-recovery", process.pid, args.host, port,
            lambda snapshot: (
                isinstance(snapshot.get("listenQueue"), dict)
                and snapshot["listenQueue"].get("completed") == 0  # type: ignore[index]
                and snapshot.get("serverFdCount") == baseline.get("serverFdCount")
                and isinstance(snapshot.get("targetTcp"), dict)
                and snapshot["targetTcp"].get("establishedCount") == 0  # type: ignore[index]
            ), args.timeout
        )
        manifest["phases"]["zeroByteRecovery"] = {"snapshot": recovered, "attempts": recovery_attempts}
        manifest["failures"].extend(validate_recovery(baseline, recovered, "zero-byte-recovery"))
        recovered_cpu = sample_cpu(evidence, process.pid, args.host, port, "recovery-cpu", args.sample_window_seconds)
        manifest["phases"]["recoveryCpu"] = {"samples": recovered_cpu, "summary": cpu_window(recovered_cpu)}
        manifest["failures"].extend(
            failure
            for sample in recovered_cpu
            for failure in validate_recovery(baseline, sample, "recovery-cpu")
        )

        # Final shutdown with accepted silent sockets still alive. No SIGKILL
        # is ever used by this runner; a SIGTERM timeout is a failed run.
        silent_sockets, silent_results = open_clients(args.host, port, args.pending, args.timeout)
        owned.add("shutdown", silent_sockets)
        persist_open_results(evidence, "shutdown", silent_results)
        manifest["phases"]["shutdownOpen"] = {"clients": silent_results}
        manifest["failures"].extend(
            validate_socket_group("shutdown", silent_results, silent_sockets, None, args.pending)
        )
        shutdown_barrier, shutdown_attempts = wait_snapshot(
            evidence, "shutdown-barrier", process.pid, args.host, port,
            lambda snapshot: not validate_accepted_barrier(baseline, snapshot, args.pending), args.timeout
        )
        manifest["phases"]["shutdownBarrier"] = {"snapshot": shutdown_barrier, "attempts": shutdown_attempts}
        manifest["failures"].extend(validate_accepted_barrier(baseline, shutdown_barrier, args.pending))
        os.killpg(process.pid, signal.SIGTERM)
        shutdown_read = close_clients(
            evidence, silent_sockets,
            {index: f"shutdown-{index:03d}" for index, _ in silent_sockets}, args.timeout, "shutdown-silent"
        )
        owned.forget(silent_sockets)
        silent_sockets = []
        process.wait(timeout=args.shutdown_timeout)
        manifest["phases"]["shutdown"] = {"clients": shutdown_read}
        for result in shutdown_read:
            manifest["failures"].extend(validate_close_result(result, require_empty=True))
        shutdown_server = {
            "signal": "SIGTERM",
            "returncode": process.returncode,
            "forcedKill": False,
            "finishedMonotonicNs": time.monotonic_ns(),
        }
        if process.returncode != 0:
            manifest["failures"].append(f"SIGTERM returned {process.returncode!r}")
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            shutdown_server["processGroupGone"] = True
        else:
            shutdown_server["processGroupGone"] = False
            manifest["failures"].append("server process group remained after SIGTERM")
        manifest["server"]["shutdown"] = shutdown_server
        manifest["status"] = "passed" if not manifest["failures"] else "failed"
        exit_code = 0 if manifest["status"] == "passed" else 1
    except BaseException as exc:
        manifest["status"] = "failed"
        manifest["failures"].append(repr(exc))
        manifest["exceptionType"] = type(exc).__name__
        try:
            path = evidence.write_bytes("failure.traceback.txt", traceback.format_exc().encode("utf-8"))
            manifest["failureTraceback"] = evidence.artifacts[path]
        except BaseException:
            pass
    finally:
        if owned._entries:
            try:
                manifest["finalSocketSalvage"] = owned.salvage()
            except BaseException as exc:
                manifest["failures"].append(f"final socket salvage failed: {exc!r}")
        silent_sockets = []
        if process is not None and process.poll() is None:
            shutdown = {"signal": "SIGTERM", "forcedKill": False, "startedMonotonicNs": time.monotonic_ns()}
            try:
                os.killpg(process.pid, signal.SIGTERM)
                process.wait(timeout=args.shutdown_timeout)
            except BaseException as exc:
                shutdown["error"] = repr(exc)
                manifest["failures"].append(f"cleanup SIGTERM failed: {exc!r}")
                manifest["status"] = "failed"
            shutdown.update({"returncode": process.returncode, "finishedMonotonicNs": time.monotonic_ns()})
            manifest["server"]["cleanupShutdown"] = shutdown
        if stdout_handle is not None:
            stdout_handle.close()
            evidence.record_path("server/stdout.bin")
        if stderr_handle is not None:
            stderr_handle.close()
            evidence.record_path("server/stderr.bin")
        manifest["finishedUtc"] = utc_now()
        manifest["artifacts"] = sorted(evidence.artifacts.values(), key=lambda item: str(item["path"]))
        try:
            evidence.write_json("manifest.json", manifest)
            evidence.write_json("evidence.json", manifest)
        except BaseException:
            exit_code = 1
    return manifest, exit_code


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    parser.add_argument("--persona", default="windows_chrome145")
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--max-connections", type=int, default=16)
    parser.add_argument("--pending", type=int, default=SILENT_PENDING_LIMIT)
    parser.add_argument("--normal-probes", type=int, default=DEFAULT_NORMAL_PROBES)
    parser.add_argument("--ttl-grace-seconds", type=float, default=DEFAULT_TTL_GRACE)
    parser.add_argument("--sample-window-seconds", type=float, default=DEFAULT_SAMPLE_WINDOW)
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT)
    parser.add_argument("--shutdown-timeout", type=float, default=DEFAULT_SHUTDOWN_TIMEOUT)
    args = parser.parse_args()
    if args.workers != 1:
        parser.error("--workers must be 1")
    if args.pending != SILENT_PENDING_LIMIT:
        parser.error(f"--pending must be {SILENT_PENDING_LIMIT}")
    return args


if __name__ == "__main__":
    _, status = run(parse_args())
    raise SystemExit(status)

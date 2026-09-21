#!/usr/bin/env python3
"""Qualify the one-worker WebSocket connection limit on Darwin.

This is a real-process evidence runner.  It keeps WS1 open while WS2 is
rejected, proves that WS1 can still execute id=2, then proves a fresh WS3 can
execute id=3 after Close cleanup.  A second active/reject/recover cycle is
run to detect capacity drift.  Every wire byte, command stream, snapshot and
exception is retained without redaction or truncation.
"""

from __future__ import annotations

import argparse
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
from typing import Any

try:
    from .cdp_capacity import (
        Evidence, capture_snapshot, discovery_request, parse_http_response,
        read_response_head, read_to_eof, receive_websocket_frame, reserve_port,
        utc_now, websocket_request, masked_websocket_close_frame,
        masked_websocket_frame, wait_ready,
    )
except ImportError:
    from cdp_capacity import (
        Evidence, capture_snapshot, discovery_request, parse_http_response,
        read_response_head, read_to_eof, receive_websocket_frame, reserve_port,
        utc_now, websocket_request, masked_websocket_close_frame,
        masked_websocket_frame, wait_ready,
    )


DEFAULT_TIMEOUT = 10.0


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def headers_named(parsed: dict[str, object], name: str) -> list[str]:
    return [str(value) for key, value in parsed.get("headers", [])
            if str(key).lower() == name.lower()]


def validate_rejection(result: dict[str, object]) -> list[str]:
    parsed = result.get("parsedResponse")
    failures: list[str] = []
    if result.get("error") is not None:
        failures.append("rejected WebSocket transport failed")
    if not isinstance(parsed, dict) or parsed.get("status") != 503:
        failures.append("active second WebSocket did not receive HTTP 503")
    if not isinstance(parsed, dict) or parsed.get("completeHead") is not True:
        failures.append("rejected WebSocket response head was incomplete")
    if not isinstance(parsed, dict) or parsed.get("contentLengthMatches") is not True:
        failures.append("rejected WebSocket response body was incomplete")
    reasons = headers_named(parsed or {}, "X-Obscura-Reason")
    if reasons != ["max-connections"]:
        failures.append(f"unexpected X-Obscura-Reason values: {reasons!r}")
    return failures


def validate_cdp_command(
    command: dict[str, object], command_id: int, label: str
) -> list[str]:
    failures: list[str] = []
    if command.get("commandError") is not None or command.get("commandJsonError") is not None:
        failures.append(f"{label} command failed")
    value = command.get("serverCommandJson")
    if (
        not isinstance(value, dict)
        or value.get("id") != command_id
        or not isinstance(value.get("result"), dict)
    ):
        failures.append(f"{label} returned wrong or unsuccessful CDP response")
    return failures


def validate_ws(result: dict[str, object], command_id: int) -> list[str]:
    failures: list[str] = []
    parsed = result.get("parsedResponse")
    command = result.get("command")
    if not isinstance(command, dict):
        command = result
    if result.get("error") is not None or not isinstance(parsed, dict) or parsed.get("status") != 101:
        failures.append(f"WebSocket {result.get('name')} did not upgrade with 101")
    failures.extend(
        validate_cdp_command(command, command_id, f"WebSocket {result.get('name')}")
    )
    if result.get("closeError") is not None:
        failures.append(f"WebSocket {result.get('name')} close did not reach EOF")
    return failures


def parse_darwin_thread_count(raw: bytes) -> int:
    """Count `ps -M` rows while retaining the original command output."""
    rows = []
    for line in raw.decode("latin-1").splitlines():
        stripped = line.strip()
        if not stripped or stripped.upper().startswith(("USER", "PID")):
            continue
        rows.append(stripped)
    return len(rows)


def capture_thread_snapshot(evidence: Evidence, phase: str, pid: int) -> dict[str, object]:
    command, raw = evidence.capture_command(phase, ["ps", "-M", "-p", str(pid)])
    result = {
        "phase": phase,
        "pid": pid,
        "threadCount": parse_darwin_thread_count(raw),
        "raw": {"sha256": hashlib.sha256(raw).hexdigest(), "bytes": len(raw)},
        "command": command,
    }
    path = evidence.write_json(f"host/{phase}/threads.json", result)
    result["snapshot"] = evidence.artifacts[path]
    return result


def wait_for_recovered_snapshot_with_threads(
    evidence: Evidence, phase: str, pid: int, host: str, port: int,
    baseline_fds: object, baseline_threads: object, timeout: float,
) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    attempt = 0
    latest: dict[str, object] | None = None
    while time.monotonic() < deadline:
        attempt += 1
        attempt_phase = f"{phase}-attempt-{attempt:03d}"
        latest = capture_snapshot(evidence, attempt_phase, pid, host, port)
        latest["threads"] = capture_thread_snapshot(
            evidence, attempt_phase, pid
        )
        queue = latest.get("listenQueue")
        if (
            isinstance(queue, dict)
            and int(queue.get("completed", -1)) == 0
            and (baseline_fds is None or latest.get("serverFdCount") == baseline_fds)
            and (baseline_threads is None or latest["threads"].get("threadCount") == baseline_threads)
        ):
            return latest
        time.sleep(0.05)
    assert latest is not None
    return latest


def validate_snapshot_recovery(
    baseline: dict[str, object], recoveries: dict[str, dict[str, object]]
) -> list[str]:
    """Reject incomplete host evidence instead of treating missing values as equal."""
    failures: list[str] = []
    required = (("baseline", baseline), *recoveries.items())
    for label, snapshot in required:
        if snapshot.get("serverFdCount") is None:
            failures.append(f"{label} serverFdCount is missing")
        queue = snapshot.get("listenQueue")
        if not isinstance(queue, dict):
            failures.append(f"{label} listenQueue is missing")
        elif queue.get("completed") != 0:
            failures.append(f"{label} listenQueue is not empty")
        if snapshot.get("process") is None:
            failures.append(f"{label} process snapshot is missing")
        threads = snapshot.get("threads")
        thread_count = threads.get("threadCount") if isinstance(threads, dict) else None
        if not isinstance(thread_count, int) or thread_count <= 0:
            failures.append(f"{label} threadCount is missing or non-positive")
    baseline_fd = baseline.get("serverFdCount")
    baseline_threads = baseline.get("threads", {}).get("threadCount") if isinstance(baseline.get("threads"), dict) else None
    for label, snapshot in recoveries.items():
        if baseline_fd is not None and snapshot.get("serverFdCount") != baseline_fd:
            failures.append(f"{label} FD count drifted")
        threads = snapshot.get("threads")
        thread_count = threads.get("threadCount") if isinstance(threads, dict) else None
        if baseline_threads is not None and thread_count != baseline_threads:
            failures.append(f"{label} thread count drifted")
    return failures


def record_ws_open(
    evidence: Evidence, name: str, host: str, port: int, timeout: float
) -> tuple[socket.socket | None, dict[str, object]]:
    """Open one WS and retain the complete HTTP upgrade exchange."""
    request = websocket_request(host, port)
    prefix = f"websocket/{name}"
    request_path = evidence.write_bytes(f"{prefix}.request.bin", request)
    started = time.monotonic_ns()
    response = b""
    error: str | None = None
    connection: socket.socket | None = None
    try:
        connection = socket.create_connection((host, port), timeout=timeout)
        connection.settimeout(timeout)
        connection.sendall(request)
        response, error = read_response_head(connection)
    except BaseException as exc:
        error = repr(exc)
        if connection is not None:
            connection.close()
            connection = None
    response_path = evidence.write_bytes(f"{prefix}.response.bin", response)
    result: dict[str, object] = {
        "name": name, "startedMonotonicNs": started,
        "finishedMonotonicNs": time.monotonic_ns(), "error": error,
        "request": evidence.artifacts[request_path],
        "response": evidence.artifacts[response_path],
        "parsedResponse": parse_http_response(response),
        "localAddress": list(connection.getsockname()) if connection else None,
        "remoteAddress": list(connection.getpeername()) if connection else None,
    }
    result_path = evidence.write_json(f"{prefix}.upgrade.result.json", result)
    result["result"] = evidence.artifacts[result_path]
    if error is not None or result["parsedResponse"].get("status") != 101:  # type: ignore[union-attr]
        if connection is not None:
            connection.close()
        return None, result
    return connection, result


def ws_command_prefix(name: str, command_id: int) -> str:
    return f"websocket/{name}/command-{command_id}"


def ws_command(
    evidence: Evidence, name: str, connection: socket.socket, command_id: int,
    timeout: float,
) -> dict[str, object]:
    prefix = ws_command_prefix(name, command_id)
    payload = json.dumps({"id": command_id, "method": "Browser.getVersion"}, separators=(",", ":")).encode()
    frame = masked_websocket_frame(payload, 1, bytes((command_id, 0x43, 0x65, 0x87)))
    payload_path = evidence.write_bytes(f"{prefix}.command.payload.bin", payload)
    frame_path = evidence.write_bytes(f"{prefix}.command.frame.bin", frame)
    response = b""; response_payload = b""; metadata: dict[str, object] = {}; error = None; json_error = None
    try:
        connection.settimeout(timeout)
        connection.sendall(frame)
        response, response_payload, metadata, error = receive_websocket_frame(connection)
    except BaseException as exc:
        error = repr(exc)
    response_path = evidence.write_bytes(f"{prefix}.command.response.frame.bin", response)
    response_payload_path = evidence.write_bytes(f"{prefix}.command.response.payload.bin", response_payload)
    value: object = None
    if response_payload:
        try:
            value = json.loads(response_payload)
        except BaseException as exc:
            json_error = repr(exc)
    result = {"commandId": command_id, "commandError": error, "commandJsonError": json_error,
              "clientCommandPayload": evidence.artifacts[payload_path], "clientCommandFrame": evidence.artifacts[frame_path],
              "serverCommandFrame": evidence.artifacts[response_path], "serverCommandPayload": evidence.artifacts[response_payload_path],
              "serverCommandMetadata": metadata, "serverCommandJson": value}
    path = evidence.write_json(f"{prefix}.command.result.json", result); result["result"] = evidence.artifacts[path]
    return result


def ws_close(evidence: Evidence, name: str, connection: socket.socket, timeout: float) -> dict[str, object]:
    prefix = f"websocket/{name}"
    frame = masked_websocket_close_frame(); frame_path = evidence.write_bytes(f"{prefix}.close.frame.bin", frame)
    wire = b""; error = None
    try:
        connection.settimeout(timeout)
        connection.sendall(frame)
        wire, error = read_to_eof(connection)
    except BaseException as exc:
        error = repr(exc)
    finally:
        connection.close()
    wire_path = evidence.write_bytes(f"{prefix}.close.response.bin", wire)
    result = {"closeError": error, "clientCloseFrame": evidence.artifacts[frame_path], "serverCloseWire": evidence.artifacts[wire_path]}
    path = evidence.write_json(f"{prefix}.close.result.json", result); result["result"] = evidence.artifacts[path]
    return result


def rejected_ws(evidence: Evidence, name: str, host: str, port: int, timeout: float) -> dict[str, object]:
    request = websocket_request(host, port); prefix = f"websocket/{name}"; request_path = evidence.write_bytes(f"{prefix}.request.bin", request)
    response = b""; error = None
    try:
        with socket.create_connection((host, port), timeout=timeout) as connection:
            connection.settimeout(timeout); connection.sendall(request)
            response, error = read_response_head(connection)
            if error is None:
                tail, tail_error = read_to_eof(connection)
                response += tail
                if tail_error is not None:
                    error = tail_error
    except BaseException as exc:
        error = repr(exc)
    response_path = evidence.write_bytes(f"{prefix}.response.bin", response)
    result = {"name": name, "error": error, "request": evidence.artifacts[request_path], "response": evidence.artifacts[response_path], "parsedResponse": parse_http_response(response)}
    path = evidence.write_json(f"{prefix}.result.json", result); result["result"] = evidence.artifacts[path]
    return result


def run_cycle(evidence: Evidence, cycle: int, host: str, port: int, timeout: float) -> tuple[dict[str, object], list[str]]:
    failures: list[str] = []
    first, first_result = record_ws_open(evidence, f"ws{1 if cycle == 1 else 3}", host, port, timeout)
    ws_id = 1 if cycle == 1 else 3
    if first is None:
        return {"active": first_result}, [f"WS{ws_id} failed to open"]
    command_one = ws_command(evidence, f"ws{ws_id}", first, ws_id, timeout)
    first_result["command"] = command_one
    rejection = rejected_ws(evidence, f"ws{ws_id + 1}", host, port, timeout)
    failures.extend(validate_rejection(rejection))
    command_two = ws_command(evidence, f"ws{ws_id}", first, ws_id + 1, timeout)
    first_result["afterRejectionCommand"] = command_two
    failures.extend(
        validate_cdp_command(
            command_two,
            ws_id + 1,
            f"WS{ws_id} after rejected WS{ws_id + 1}",
        )
    )
    close = ws_close(evidence, f"ws{ws_id}", first, timeout); first_result["close"] = close
    first_result["closeError"] = close.get("closeError")
    failures.extend(validate_ws(first_result, ws_id))
    if close.get("closeError") is not None: failures.append(f"WS{ws_id} close failed")
    return {"active": first_result, "rejected": rejection, "close": close}, failures


def run(args: argparse.Namespace) -> tuple[dict[str, object], int]:
    evidence = Evidence(args.output)
    manifest: dict[str, Any] = {"schemaVersion": 1, "status": "running", "startedUtc": utc_now(), "platform": {"system": platform.system(), "release": platform.release(), "version": platform.version(), "machine": platform.machine(), "python": sys.version, "uname": list(platform.uname()), "rlimitNofile": list(resource.getrlimit(resource.RLIMIT_NOFILE))}, "configuration": {"binary": str(args.binary.resolve()), "host": args.host, "port": None, "persona": args.persona, "workers": 1, "maxConnections": 1, "timeoutSeconds": args.timeout}, "binary": {}, "server": {}, "phases": {}, "failures": []}
    process: subprocess.Popen[bytes] | None = None; stdout = stderr = None; exit_code = 1
    shutdown_active: socket.socket | None = None
    try:
        if platform.system() != "Darwin": raise RuntimeError("this evidence runner requires Darwin")
        if not args.binary.is_file(): raise FileNotFoundError(args.binary)
        port = args.port or reserve_port(args.host); manifest["configuration"]["port"] = port
        manifest["binary"] = {"path": str(args.binary.resolve()), "sha256": sha256_file(args.binary), "size": args.binary.stat().st_size}
        command = [str(args.binary.resolve()), "--persona", args.persona, "serve", "--host", args.host, "--port", str(port), "--workers", "1", "--max-connections", "1"]
        stdout_path = evidence.root / "server/stdout.bin"; stderr_path = evidence.root / "server/stderr.bin"; stdout_path.parent.mkdir(parents=True, exist_ok=True)
        stdout = stdout_path.open("xb"); stderr = stderr_path.open("xb")
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, start_new_session=True)
        manifest["server"] = {"argv": command, "pid": process.pid, "processGroup": os.getpgid(process.pid), "startedMonotonicNs": time.monotonic_ns()}
        manifest["phases"]["readiness"] = wait_ready(evidence, process, args.host, port, args.timeout)
        baseline = capture_snapshot(evidence, "baseline", process.pid, args.host, port)
        baseline["threads"] = capture_thread_snapshot(evidence, "baseline", process.pid)
        manifest["phases"]["baseline"] = baseline
        baseline_threads = baseline.get("threads", {}).get("threadCount") if isinstance(baseline.get("threads"), dict) else None
        cycle_one, failures = run_cycle(evidence, 1, args.host, port, args.timeout); manifest["phases"]["cycle1"] = cycle_one
        recovered_one = wait_for_recovered_snapshot_with_threads(evidence, "recovery1", process.pid, args.host, port, baseline.get("serverFdCount"), baseline_threads, args.timeout); manifest["phases"]["recovery1"] = recovered_one
        cycle_two, failures_two = run_cycle(evidence, 2, args.host, port, args.timeout); manifest["phases"]["cycle2"] = cycle_two; failures.extend(failures_two)
        recovered_two = wait_for_recovered_snapshot_with_threads(evidence, "recovery2", process.pid, args.host, port, baseline.get("serverFdCount"), baseline_threads, args.timeout); manifest["phases"]["recovery2"] = recovered_two
        failures.extend(validate_snapshot_recovery(
            baseline, {"recovery1": recovered_one, "recovery2": recovered_two}
        ))
        shutdown_active, shutdown_active_result = record_ws_open(evidence, "shutdown-active", args.host, port, args.timeout)
        if shutdown_active is None:
            failures.append("shutdown-active WebSocket did not upgrade")
        else:
            shutdown_active_result["command"] = ws_command(
                evidence, "shutdown-active", shutdown_active, 5, args.timeout
            )
            manifest["phases"]["shutdownActive"] = shutdown_active_result
            failures.extend(validate_ws({**shutdown_active_result, "closeError": None}, 5))
        manifest["failures"] = failures; manifest["status"] = "passed" if not failures else "failed"; exit_code = 0 if not failures else 1
    except BaseException as exc:
        manifest["status"] = "failed"; manifest["failures"].append(repr(exc)); manifest["exceptionType"] = type(exc).__name__; evidence.write_bytes("failure.traceback.txt", traceback.format_exc().encode()); exit_code = 1
    finally:
        if process is not None:
            shutdown = {"signal": "SIGTERM", "forcedKill": False, "startedMonotonicNs": time.monotonic_ns()}
            if process.poll() is None:
                try: os.killpg(process.pid, signal.SIGTERM)
                except ProcessLookupError: pass
                try: process.wait(timeout=args.timeout)
                except subprocess.TimeoutExpired:
                    # This qualification deliberately never escalates to
                    # SIGKILL: a timeout is evidence of failed graceful
                    # shutdown and must remain visible as such.
                    shutdown["termTimeout"] = True
            if shutdown_active is not None:
                wire, read_error = read_to_eof(shutdown_active)
                wire_path = evidence.write_bytes("websocket/shutdown-active.server-wire.bin", wire)
                shutdown_active.close()
                active_read = {"wire": evidence.artifacts[wire_path], "readError": read_error, "eof": read_error is None}
                manifest.setdefault("phases", {}).setdefault("shutdownActive", {})["shutdownRead"] = active_read
                if read_error is not None:
                    message = "active WebSocket did not close cleanly during SIGTERM"
                    if message not in manifest["failures"]:
                        manifest["failures"].append(message)
                    manifest["status"] = "failed"
                    exit_code = 1
            if process.returncode != 0:
                message = f"SIGTERM returned nonzero status: {process.returncode!r}"
                if message not in manifest["failures"]:
                    manifest["failures"].append(message)
                manifest["status"] = "failed"
                exit_code = 1
            shutdown.update({"finishedMonotonicNs": time.monotonic_ns(), "returncode": process.returncode}); manifest["server"]["shutdown"] = shutdown
            if shutdown.get("termTimeout"):
                message = "SIGTERM did not stop server within timeout"
                if message not in manifest["failures"]:
                    manifest["failures"].append(message)
                manifest["status"] = "failed"
                exit_code = 1
        for handle, relative in ((stdout, "server/stdout.bin"), (stderr, "server/stderr.bin")):
            if handle is not None: handle.close(); evidence.record_path(relative)
        manifest["finishedUtc"] = utc_now(); manifest["artifacts"] = sorted(evidence.artifacts.values(), key=lambda item: str(item["path"]))
        evidence.write_json("manifest.json", manifest)
        evidence.write_json("evidence.json", manifest)
    return manifest, exit_code


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__); parser.add_argument("--binary", type=Path, required=True); parser.add_argument("--output", type=Path, required=True); parser.add_argument("--host", default="127.0.0.1"); parser.add_argument("--port", type=int, default=0); parser.add_argument("--persona", default="windows_chrome145"); parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT); return parser.parse_args()


if __name__ == "__main__":
    arguments = parse_args(); _, status = run(arguments); raise SystemExit(status)

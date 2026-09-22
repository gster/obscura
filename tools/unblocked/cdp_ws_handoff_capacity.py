#!/usr/bin/env python3
"""Qualify WebSocket handoff saturation with a real release process.

This runner intentionally does not use a product test hook.  A wave first
establishes 256 accepted, incomplete request heads, then completes those
heads concurrently.  The request prefix and the terminator are separate so
the head reader cannot consume a pipelined command.  A result is qualified
only when every socket has a complete, lossless wire record and the wave has
observed both a normal 101 handoff and the production saturation 503.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import hashlib
import ipaddress
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
        Evidence,
        discovery_request,
        masked_websocket_close_frame,
        masked_websocket_frame,
        parse_http_response,
        parse_lsof_fd_count,
        read_response_head,
        read_to_eof,
        receive_websocket_frame,
        reserve_port,
        sha256_bytes,
        utc_now,
        wait_for_stopped,
        wait_ready,
        websocket_upgrade_and_close,
        websocket_request,
    )
    from .cdp_silent_pending import (
        parse_darwin_listen_queue,
        parse_darwin_thread_count,
        parse_lsof_target_tcp,
        parse_process_cpu,
        validate_stable_barriers,
    )
except ImportError:
    from cdp_capacity import (  # type: ignore
        Evidence,
        discovery_request,
        masked_websocket_close_frame,
        masked_websocket_frame,
        parse_http_response,
        parse_lsof_fd_count,
        read_response_head,
        read_to_eof,
        receive_websocket_frame,
        reserve_port,
        sha256_bytes,
        utc_now,
        wait_for_stopped,
        wait_ready,
        websocket_upgrade_and_close,
        websocket_request,
    )
    from cdp_silent_pending import (  # type: ignore
        parse_darwin_listen_queue,
        parse_darwin_thread_count,
        parse_lsof_target_tcp,
        parse_process_cpu,
        validate_stable_barriers,
    )


HANDOFF_CHANNEL_CAPACITY = 128
QUALIFIED_ACCEPTED = 256
QUALIFIED_MAX_CONNECTIONS = 512
QUALIFIED_WAVES = 3
DEFAULT_TIMEOUT = 10.0
DEFAULT_STARTUP_TIMEOUT = 30.0
DEFAULT_SHUTDOWN_TIMEOUT = 15.0
HANDOFF_REASON = "ws-handoff-saturated"


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def write_trace(evidence: Evidence, relative: str) -> dict[str, object]:
    path = evidence.write_bytes(relative, traceback.format_exc().encode("utf-8"))
    return evidence.artifacts[path]


def headers_named(parsed: dict[str, object], name: str) -> list[str]:
    return [str(value) for key, value in parsed.get("headers", []) if str(key).lower() == name.lower()]


def validate_handoff503(result: dict[str, object]) -> list[str]:
    parsed = result.get("parsedResponse")
    failures: list[str] = []
    if not isinstance(parsed, dict) or parsed.get("status") != 503:
        failures.append("status is not 503")
    if not isinstance(parsed, dict) or parsed.get("completeHead") is not True:
        failures.append("response head is incomplete")
    if not isinstance(parsed, dict) or parsed.get("contentLengthMatches") is not True:
        failures.append("response body is incomplete")
    if not isinstance(parsed, dict) or parsed.get("parseError") is not None:
        failures.append("response parse failed")
    if headers_named(parsed or {}, "X-Obscura-Reason") != [HANDOFF_REASON]:
        failures.append("reason is not the unique handoff saturation reason")
    if result.get("recvError") is not None or result.get("eof") is not True:
        failures.append("response did not reach clean EOF")
    if not result.get("wireBytes"):
        failures.append("response wire is empty")
    return failures


def validate_barrier(baseline: dict[str, object], snapshot: dict[str, object], expected: int) -> list[str]:
    failures: list[str] = []
    if snapshot.get("processAlive") is not True:
        failures.append("process is not alive")
    queue = snapshot.get("listenQueue")
    if not isinstance(queue, dict) or queue.get("completed") != 0:
        failures.append("listener completed queue is not zero")
    target = snapshot.get("targetTcp")
    if not isinstance(target, dict) or target.get("establishedCount") != expected:
        failures.append("target ESTABLISHED count is not exactly accepted count")
    before = baseline.get("serverFdCount")
    current = snapshot.get("serverFdCount")
    if not isinstance(before, int) or not isinstance(current, int) or current - before < expected:
        failures.append("server FD delta is below accepted count")
    if not isinstance(snapshot.get("threads"), dict) or snapshot["threads"].get("threadCount") is None:  # type: ignore[index]
        failures.append("thread sample missing")
    return failures


def validate_recovery(baseline: dict[str, object], snapshot: dict[str, object]) -> list[str]:
    failures: list[str] = []
    if snapshot.get("processAlive") is not True:
        failures.append("process is not alive")
    queue = snapshot.get("listenQueue")
    if not isinstance(queue, dict) or queue.get("completed") != 0:
        failures.append("listener completed queue did not return to zero")
    target = snapshot.get("targetTcp")
    if not isinstance(target, dict) or target.get("establishedCount") != 0:
        failures.append("target ESTABLISHED did not return to zero")
    if snapshot.get("serverFdCount") != baseline.get("serverFdCount"):
        failures.append("server FD count did not return to baseline")
    old_threads = baseline.get("threads")
    new_threads = snapshot.get("threads")
    if not isinstance(old_threads, dict) or not isinstance(new_threads, dict) or new_threads.get("threadCount") != old_threads.get("threadCount"):
        failures.append("thread count did not return to baseline")
    return failures


def capture_snapshot(evidence: Evidence, phase: str, process: subprocess.Popen[bytes], host: str, port: int) -> dict[str, object]:
    pid = process.pid
    _, ps_raw = evidence.capture_command(phase, ["ps", "-o", "pid=,state=,%cpu=,rss=,vsz=,etime=,time=,command=", "-p", str(pid)])
    _, threads_raw = evidence.capture_command(phase, ["ps", "-M", "-p", str(pid)])
    _, lsof_raw = evidence.capture_command(phase, ["lsof", "-nP", "-a", "-p", str(pid)])
    _, queue_raw = evidence.capture_command(phase, ["netstat", "-Lan", "-p", "tcp"])
    evidence.capture_command(phase, ["uname", "-a"])
    evidence.capture_command(phase, ["sysctl", "kern.ipc.somaxconn"])
    evidence.capture_command(phase, ["netstat", "-anv", "-p", "tcp"])
    snapshot: dict[str, object] = {
        "phase": phase,
        "capturedUtc": utc_now(),
        "processAlive": process.poll() is None,
        "process": parse_process_cpu(ps_raw),
        "cpu": parse_process_cpu(ps_raw),
        "threads": {"threadCount": parse_darwin_thread_count(threads_raw), "rawSha256": sha256_bytes(threads_raw), "rawBytes": len(threads_raw)},
        "serverFdCount": parse_lsof_fd_count(lsof_raw),
        "listenQueue": parse_darwin_listen_queue(queue_raw, host, port),
        "targetTcp": parse_lsof_target_tcp(lsof_raw, host, port),
        "rawSha256": {"ps": sha256_bytes(ps_raw), "threads": sha256_bytes(threads_raw), "lsof": sha256_bytes(lsof_raw), "listenQueue": sha256_bytes(queue_raw)},
    }
    path = evidence.write_json(f"host/{phase}/snapshot.json", snapshot)
    snapshot["snapshot"] = evidence.artifacts[path]
    return snapshot


def wait_stable_barrier(evidence: Evidence, phase: str, process: subprocess.Popen[bytes], host: str, port: int, baseline: dict[str, object], expected: int, timeout: float, recovery: bool = False) -> tuple[dict[str, object], list[dict[str, object]]]:
    deadline = time.monotonic() + timeout
    snapshots: list[dict[str, object]] = []
    while time.monotonic() < deadline:
        snapshot = capture_snapshot(evidence, f"{phase}-attempt-{len(snapshots):03d}", process, host, port)
        snapshots.append(snapshot)
        predicate_failures = validate_recovery(baseline, snapshot) if recovery else validate_barrier(baseline, snapshot, expected)
        if len(snapshots) >= 2 and not predicate_failures:
            stable = snapshots[-2:]
            if not validate_stable_barriers(stable, expected):
                return snapshot, snapshots
        time.sleep(min(0.05, max(0.0, deadline - time.monotonic())))
    raise TimeoutError(f"{phase} stable accepted barrier timed out after {timeout:.3f}s")


class OwnedSocketRegistry:
    """Keep ownership until the final wire/error/EOF record is durable."""

    def __init__(self, evidence: Evidence, timeout: float) -> None:
        self.evidence = evidence
        self.timeout = timeout
        self._entries: dict[int, tuple[str, socket.socket]] = {}

    def add(self, wave: int, index: int, connection: socket.socket) -> None:
        self._entries[id(connection)] = (f"wave-{wave:02d}-client-{index:04d}", connection)

    def release(self, connection: socket.socket) -> None:
        self._entries.pop(id(connection), None)
        connection.close()

    def salvage(self, phase: str = "exception-final") -> list[dict[str, object]]:
        entries = list(self._entries.values())

        def one(item: tuple[str, socket.socket]) -> dict[str, object]:
            name, connection = item
            prefix = f"salvage/{phase}/{name}"
            try:
                connection.settimeout(self.timeout)
                wire, error = read_to_eof(connection)
                eof = error is None
                wire_path = self.evidence.write_bytes(f"{prefix}.wire.bin", wire)
                result: dict[str, object] = {"name": name, "wire": self.evidence.artifacts[wire_path], "wireBytes": len(wire), "recvError": error, "eof": eof, "observedMonotonicNs": time.monotonic_ns()}
            except BaseException as exc:
                wire_path = self.evidence.write_bytes(f"{prefix}.wire.bin", b"")
                result = {"name": name, "wire": self.evidence.artifacts[wire_path], "wireBytes": 0, "recvError": repr(exc), "eof": False, "observedMonotonicNs": time.monotonic_ns(), "traceback": write_trace(self.evidence, f"{prefix}.traceback.txt")}
            finally:
                connection.close()
                self._entries.pop(id(connection), None)
            result_path = self.evidence.write_json(f"{prefix}.result.json", result)
            result["result"] = self.evidence.artifacts[result_path]
            return result

        with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(entries))) as pool:
            return list(pool.map(one, entries))


def _send(connection: socket.socket, payload: bytes) -> tuple[int, str | None]:
    sent = 0
    try:
        while sent < len(payload):
            sent += connection.send(payload[sent:])
        return sent, None
    except BaseException as exc:
        return sent, repr(exc)


def open_wave(evidence: Evidence, registry: OwnedSocketRegistry, wave: int, host: str, port: int, count: int, timeout: float) -> tuple[dict[int, socket.socket], list[dict[str, object]]]:
    full = websocket_request(host, port)
    prefix = full[:-4]
    wave_root = f"waves/wave-{wave:02d}/open"
    evidence.write_bytes(f"{wave_root}/websocket-prefix.request.bin", prefix)
    evidence.write_bytes(f"{wave_root}/websocket-full.request.bin", full)
    outputs: list[tuple[int, socket.socket | None, dict[str, object]]] = []

    def one(index: int) -> tuple[int, socket.socket | None, dict[str, object]]:
        result: dict[str, object] = {"index": index, "connectStartedMonotonicNs": time.monotonic_ns(), "connected": False, "localAddress": None, "remoteAddress": None, "sendPrefixStartedMonotonicNs": None, "sendPrefixFinishedMonotonicNs": None, "sendPrefixBytes": 0, "connectError": None, "sendPrefixError": None}
        connection: socket.socket | None = None
        registered = False
        try:
            connection = socket.create_connection((host, port), timeout=timeout)
            registry.add(wave, index, connection)
            registered = True
            connection.settimeout(timeout)
            result["connected"] = True
            result["connectedMonotonicNs"] = time.monotonic_ns()
            result["localAddress"] = list(connection.getsockname())
            result["remoteAddress"] = list(connection.getpeername())
            result["sendPrefixStartedMonotonicNs"] = time.monotonic_ns()
            result["sendPrefixBytes"], result["sendPrefixError"] = _send(connection, prefix)
            result["sendPrefixFinishedMonotonicNs"] = time.monotonic_ns()
            if result["sendPrefixError"] is not None:
                raise OSError(str(result["sendPrefixError"]))
        except BaseException as exc:
            result["connectError"] = result["connectError"] or repr(exc)
            result["traceback"] = traceback.format_exc()
            if connection is not None and not registered:
                connection.close()
                connection = None
        result["finishedMonotonicNs"] = time.monotonic_ns()
        return index, connection, result

    with concurrent.futures.ThreadPoolExecutor(max_workers=count) as pool:
        futures = [pool.submit(one, index) for index in range(count)]
        for future in futures:
            outputs.append(future.result())
    outputs.sort(key=lambda item: item[0])
    sockets: dict[int, socket.socket] = {}
    results: list[dict[str, object]] = []
    for index, connection, result in outputs:
        request_path = evidence.write_bytes(f"{wave_root}/client-{index:04d}.prefix.request.bin", prefix)
        full_path = evidence.write_bytes(f"{wave_root}/client-{index:04d}.full.request.bin", full)
        result["prefixRequest"] = evidence.artifacts[request_path]
        result["fullRequest"] = evidence.artifacts[full_path]
        if result.get("traceback"):
            trace_path = evidence.write_bytes(f"{wave_root}/client-{index:04d}.traceback.txt", str(result["traceback"]).encode())
            result["tracebackArtifact"] = evidence.artifacts[trace_path]
        result_path = evidence.write_json(f"{wave_root}/client-{index:04d}.result.json", result)
        result["result"] = evidence.artifacts[result_path]
        results.append(result)
        if connection is not None:
            sockets[index] = connection
            registry.add(wave, index, connection)
    return sockets, results


def send_suffixes(evidence: Evidence, wave: int, sockets: dict[int, socket.socket]) -> dict[int, dict[str, object]]:
    suffix = b"\r\n\r\n"
    root = f"waves/wave-{wave:02d}/results"
    suffix_path = evidence.write_bytes(f"{root}/websocket-suffix.request.bin", suffix)

    def one(index: int) -> dict[str, object]:
        connection = sockets[index]
        result: dict[str, object] = {"index": index, "sendSuffixBytes": 0, "sendSuffixError": None, "suffixRequest": evidence.artifacts[suffix_path]}
        result["sendSuffixStartedMonotonicNs"] = time.monotonic_ns()
        result["sendSuffixBytes"], result["sendSuffixError"] = _send(connection, suffix)
        result["sendSuffixFinishedMonotonicNs"] = time.monotonic_ns()
        result_path = evidence.write_json(f"{root}/client-{index:04d}.suffix.result.json", result)
        result["suffixResult"] = evidence.artifacts[result_path]
        return result

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(sockets))) as pool:
        results = list(pool.map(one, sorted(sockets)))
    return {int(result["index"]): result for result in results}


def finish_heads(evidence: Evidence, registry: OwnedSocketRegistry, wave: int, sockets: dict[int, socket.socket], suffix_results: dict[int, dict[str, object]], timeout: float) -> list[dict[str, object]]:
    root = f"waves/wave-{wave:02d}/results"

    def one(index: int) -> dict[str, object]:
        connection = sockets[index]
        result: dict[str, object] = {**suffix_results[index], "recvError": None, "eof": False, "wireBytes": 0, "firstResponseMonotonicNs": None}
        try:
            if result["sendSuffixError"] is not None:
                raise OSError(str(result["sendSuffixError"]))
            connection.settimeout(timeout)
            head, head_error = read_response_head(connection)
            result["firstResponseMonotonicNs"] = time.monotonic_ns()
            result["recvError"] = head_error
            tail = b""
            parsed_head = parse_http_response(head)
            # A 101 upgrades the stream and must stay owned for the CDP
            # command.  Only HTTP rejection responses are read through EOF.
            if head_error is None and parsed_head.get("status") != 101:
                tail, tail_error = read_to_eof(connection)
                result["recvError"] = tail_error
                result["eof"] = tail_error is None
            wire = head + tail
            result["wireBytes"] = len(wire)
            response_path = evidence.write_bytes(f"{root}/client-{index:04d}.response.bin", wire)
            result["response"] = evidence.artifacts[response_path]
            result["parsedResponse"] = parse_http_response(wire)
            parsed = result["parsedResponse"]
            status = parsed.get("status") if isinstance(parsed, dict) else None
            if status == 101 and result["recvError"] is None and isinstance(parsed, dict) and parsed.get("completeHead") is True and parsed.get("parseError") is None:
                result["classification"] = "101-pending-cdp"
            elif status == 503 and headers_named(parsed or {}, "X-Obscura-Reason") == [HANDOFF_REASON] and not validate_handoff503(result):
                result["classification"] = "503-ws-handoff-saturated"
            elif status == 503 and headers_named(parsed or {}, "X-Obscura-Reason") == ["max-connections"]:
                result["classification"] = "failure-max-connections"
            else:
                result["classification"] = "failure-other"
        except BaseException as exc:
            result["classification"] = "failure-exception"
            result["recvError"] = repr(exc)
            result["tracebackArtifact"] = write_trace(evidence, f"{root}/client-{index:04d}.traceback.txt")
            failure_wire, failure_error = read_to_eof(connection)
            result["failureSalvageError"] = failure_error
            result["failureSalvageEof"] = failure_error is None
            response_path = evidence.write_bytes(f"{root}/client-{index:04d}.response.bin", failure_wire)
            result["response"] = evidence.artifacts[response_path]
            result["wireBytes"] = len(failure_wire)
            result["parsedResponse"] = parse_http_response(failure_wire)
        result_path = evidence.write_json(f"{root}/client-{index:04d}.result.json", result)
        result["result"] = evidence.artifacts[result_path]
        return result

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(sockets))) as pool:
        results = list(pool.map(one, sorted(sockets)))
    results.sort(key=lambda item: int(item["index"]))
    for result in results:
        if result.get("classification") != "101-pending-cdp":
            registry.release(sockets[int(result["index"])])
    return results


def finish_cdp(evidence: Evidence, registry: OwnedSocketRegistry, wave: int, sockets: dict[int, socket.socket], results: list[dict[str, object]], timeout: float) -> None:
    root = f"waves/wave-{wave:02d}/cdp"
    targets = [result for result in results if result.get("classification") == "101-pending-cdp"]

    def one(result: dict[str, object]) -> dict[str, object]:
        index = int(result["index"])
        connection = sockets[index]
        command_id = wave * 1_000_000 + index + 1
        payload = json.dumps({"id": command_id, "method": "Browser.getVersion"}, separators=(",", ":")).encode()
        frame = masked_websocket_frame(payload, 0x1, (index + 1).to_bytes(4, "big"))
        close = masked_websocket_close_frame()
        item: dict[str, object] = {"index": index, "commandId": command_id, "sendBytes": 0, "sendError": None, "commandError": None, "sendCloseBytes": 0, "sendCloseError": None, "closeError": None, "closeEof": False}
        raw = b""
        decoded = b""
        metadata: dict[str, object] = {}
        tail = b""
        try:
            payload_path = evidence.write_bytes(f"{root}/client-{index:04d}.request-payload.bin", payload)
            frame_path = evidence.write_bytes(f"{root}/client-{index:04d}.request-frame.bin", frame)
            close_path = evidence.write_bytes(f"{root}/client-{index:04d}.close-frame.bin", close)
            item["requestPayload"] = evidence.artifacts[payload_path]
            item["requestFrame"] = evidence.artifacts[frame_path]
            item["closeFrame"] = evidence.artifacts[close_path]
            connection.settimeout(timeout)
            item["sendBytes"], item["sendError"] = _send(connection, frame)
            if item["sendError"] is not None:
                raise OSError(str(item["sendError"]))
            raw, decoded, metadata, command_error = receive_websocket_frame(connection)
            item["commandError"] = command_error
            item["responseMetadata"] = metadata
            try:
                item["responseJson"] = json.loads(decoded)
            except BaseException as exc:
                item["responseJsonError"] = repr(exc)
            item["sendCloseBytes"], item["sendCloseError"] = _send(connection, close)
            if item["sendCloseError"] is not None:
                raise OSError(str(item["sendCloseError"]))
            tail, item["closeError"] = read_to_eof(connection)
            item["closeEof"] = item["closeError"] is None
        except BaseException as exc:
            if item.get("commandError") is None:
                item["commandError"] = repr(exc)
            item["tracebackArtifact"] = write_trace(evidence, f"{root}/client-{index:04d}.traceback.txt")
            failure_wire, failure_error = read_to_eof(connection)
            item["failureSalvageError"] = failure_error
            item["failureSalvageEof"] = failure_error is None
            failure_path = evidence.write_bytes(f"{root}/client-{index:04d}.failure-salvage-wire.bin", failure_wire)
            item["failureSalvageWire"] = evidence.artifacts[failure_path]
        raw_path = evidence.write_bytes(f"{root}/client-{index:04d}.response-frame.bin", raw)
        payload_path = evidence.write_bytes(f"{root}/client-{index:04d}.response-payload.bin", decoded)
        tail_path = evidence.write_bytes(f"{root}/client-{index:04d}.close-wire.bin", tail)
        item["responseFrame"] = evidence.artifacts[raw_path]
        item["responsePayload"] = evidence.artifacts[payload_path]
        item["closeWire"] = evidence.artifacts[tail_path]
        item["cdpSuccess"] = item.get("commandError") is None and isinstance(item.get("responseJson"), dict) and item["responseJson"].get("id") == command_id and isinstance(item["responseJson"].get("result"), dict) and item.get("closeEof") is True  # type: ignore[union-attr]
        result.update(item)
        result_path = evidence.write_json(f"{root}/client-{index:04d}.result.json", result)
        result["result"] = evidence.artifacts[result_path]
        registry.release(connection)
        return result

    with concurrent.futures.ThreadPoolExecutor(max_workers=max(1, len(targets))) as pool:
        list(pool.map(one, targets))


def classify_wave(results: list[dict[str, object]], expected: int) -> list[str]:
    failures: list[str] = []
    if len(results) != expected or {item.get("index") for item in results} != set(range(expected)):
        failures.append("wave results are not an exact 0..255 set")
    if sum(item.get("classification") == "503-ws-handoff-saturated" for item in results) < 1:
        failures.append("qualification-no-saturation: wave observed no ws-handoff-saturated 503")
    if sum(item.get("classification") == "101-pending-cdp" for item in results) < 1:
        failures.append("wave observed no 101")
    for item in results:
        if item.get("classification") not in {"503-ws-handoff-saturated", "101-pending-cdp"}:
            failures.append(f"client {item.get('index')} has explicit non-qualifying classification {item.get('classification')}")
        if item.get("classification") == "101-pending-cdp" and item.get("cdpSuccess") is not True:
            failures.append(f"client {item.get('index')} 101 lacks successful CDP result")
    return failures


def recovery_probe(evidence: Evidence, host: str, port: int, timeout: float, phase: str) -> list[str]:
    failures: list[str] = []
    request = discovery_request(host, port)
    request_path = evidence.write_bytes(f"{phase}/http.request.bin", request)
    response = b""
    error: str | None = None
    try:
        with socket.create_connection((host, port), timeout=timeout) as connection:
            connection.settimeout(timeout)
            connection.sendall(request)
            response, error = read_to_eof(connection)
    except BaseException as exc:
        error = repr(exc)
    response_path = evidence.write_bytes(f"{phase}/http.response.bin", response)
    parsed = parse_http_response(response)
    result = {"request": evidence.artifacts[request_path], "response": evidence.artifacts[response_path], "error": error, "eof": error is None, "parsedResponse": parsed}
    result_path = evidence.write_json(f"{phase}/http.result.json", result)
    result["result"] = evidence.artifacts[result_path]
    if error is not None or parsed.get("status") != 200 or parsed.get("completeHead") is not True or parsed.get("contentLengthMatches") is not True:
        failures.append("recovery HTTP was not a complete 200")
    ws = websocket_upgrade_and_close(evidence, f"{phase}/websocket", host, port, timeout)
    ws_parsed = ws.get("parsedResponse")
    ws_json = ws.get("serverCommandJson")
    if ws.get("error") is not None or not isinstance(ws_parsed, dict) or ws_parsed.get("status") != 101:
        failures.append("recovery WebSocket was not a 101")
    if ws.get("commandError") is not None or not isinstance(ws_json, dict) or ws_json.get("id") != 1 or not isinstance(ws_json.get("result"), dict):
        failures.append("recovery WebSocket CDP result was not successful")
    if ws.get("closeError") is not None:
        failures.append("recovery WebSocket did not reach EOF")
    return failures


def shutdown(process: subprocess.Popen[bytes], timeout: float) -> dict[str, object]:
    result: dict[str, object] = {"sigtermSent": False, "forcedKill": False, "returncode": None, "groupGone": False}
    if process.poll() is None:
        try:
            os.killpg(process.pid, signal.SIGTERM)
            result["sigtermSent"] = True
        except ProcessLookupError:
            pass
    try:
        process.wait(timeout=timeout)
    except subprocess.TimeoutExpired:
        result["shutdownError"] = "SIGTERM timeout; no forced kill performed"
    result["returncode"] = process.returncode
    try:
        os.killpg(process.pid, 0)
    except ProcessLookupError:
        result["groupGone"] = True
    except PermissionError:
        result["groupGone"] = False
    return result


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int)
    parser.add_argument("--persona", default="windows_chrome145")
    parser.add_argument("--workers", type=int, default=1)
    parser.add_argument("--max-connections", type=int, default=QUALIFIED_MAX_CONNECTIONS)
    parser.add_argument("--accepted", type=int, default=QUALIFIED_ACCEPTED)
    parser.add_argument("--waves", type=int, default=QUALIFIED_WAVES)
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT)
    parser.add_argument("--startup-timeout", type=float, default=DEFAULT_STARTUP_TIMEOUT)
    parser.add_argument("--shutdown-timeout", type=float, default=DEFAULT_SHUTDOWN_TIMEOUT)
    return parser


def validate_args(args: argparse.Namespace) -> None:
    if platform.system() != "Darwin":
        raise ValueError("real release qualification is restricted to Darwin")
    try:
        host = ipaddress.ip_address(args.host)
    except ValueError as exc:
        raise ValueError("--host must be an IPv4 loopback address") from exc
    if host.version != 4 or not host.is_loopback:
        raise ValueError("--host must be an IPv4 loopback address")
    if args.workers != 1:
        raise ValueError("--workers must be exactly 1")
    if args.max_connections != QUALIFIED_MAX_CONNECTIONS:
        raise ValueError("--max-connections must be exactly 512")
    if args.accepted != QUALIFIED_ACCEPTED:
        raise ValueError("--accepted must be exactly 256")
    if args.waves != QUALIFIED_WAVES:
        raise ValueError("--waves must be exactly 3")


def run(args: argparse.Namespace) -> tuple[dict[str, object], int]:
    evidence = Evidence(args.output)
    configuration = {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()}
    manifest: dict[str, Any] = {"schemaVersion": 1, "status": "running", "startedUtc": utc_now(), "configuration": configuration, "contract": {"handoffChannelCapacity": HANDOFF_CHANNEL_CAPACITY, "accepted": QUALIFIED_ACCEPTED, "maxConnections": QUALIFIED_MAX_CONNECTIONS, "waves": QUALIFIED_WAVES, "workers": 1}, "notQualified": ["non-Darwin hosts", "multi-worker relay totals", "portable process, FD, RSS, V8, or socket-buffer limits", "direct internal queue occupancy from the release process; the unique production response proves the handoff saturation branch and the deterministic Rust receive-gate test proves the exact 128 capacity"], "waves": [], "failures": [], "artifacts": evidence.artifacts}
    process: subprocess.Popen[bytes] | None = None
    stopped = False
    stream_handles: list[Any] = []
    registry = OwnedSocketRegistry(evidence, args.timeout)
    try:
        validate_args(args)
        port = args.port or reserve_port(args.host)
        manifest["configuration"]["port"] = port
        command = [str(args.binary.resolve()), "--persona", args.persona, "serve", "--host", args.host, "--port", str(port), "--workers", "1", "--max-connections", str(args.max_connections)]
        stdout_path = evidence.root / "server.stdout.bin"
        stderr_path = evidence.root / "server.stderr.bin"
        stdout = stdout_path.open("xb")
        stderr = stderr_path.open("xb")
        stream_handles = [stdout, stderr]
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=stdout, stderr=stderr, start_new_session=True)
        manifest["server"] = {"pid": process.pid, "command": command, "stdout": str(stdout_path.relative_to(evidence.root)), "stderr": str(stderr_path.relative_to(evidence.root))}
        manifest["binary"] = {"path": str(args.binary), "bytes": args.binary.stat().st_size, "sha256": sha256_file(args.binary)}
        wait_ready(evidence, process, args.host, port, args.startup_timeout)
        baseline = capture_snapshot(evidence, "baseline", process, args.host, port)
        for wave in range(1, args.waves + 1):
            wave_record: dict[str, object] = {"wave": wave}
            sockets, opens = open_wave(evidence, registry, wave, args.host, port, QUALIFIED_ACCEPTED, args.timeout)
            wave_record["openResults"] = opens
            if len(opens) != QUALIFIED_ACCEPTED or len(sockets) != QUALIFIED_ACCEPTED or any(item.get("connected") is not True or item.get("connectError") is not None or item.get("sendPrefixError") is not None or item.get("sendPrefixBytes") != len(websocket_request(args.host, port)) - 4 for item in opens):
                raise RuntimeError(f"wave {wave} did not establish and prefix exactly 256 sockets")
            _, barrier_attempts = wait_stable_barrier(evidence, f"wave-{wave:02d}-barrier", process, args.host, port, baseline, QUALIFIED_ACCEPTED, args.timeout)
            wave_record["barrier"] = barrier_attempts
            os.killpg(process.pid, signal.SIGSTOP)
            stopped = True
            wave_record["stopObservation"] = wait_for_stopped(process.pid)
            stopped_snapshot = capture_snapshot(evidence, f"wave-{wave:02d}-stopped", process, args.host, port)
            wave_record["stoppedSnapshot"] = stopped_snapshot
            stopped_failures = validate_barrier(baseline, stopped_snapshot, QUALIFIED_ACCEPTED)
            if stopped_failures:
                raise RuntimeError(f"wave {wave} stopped barrier failed: {stopped_failures!r}")
            suffix_results = send_suffixes(evidence, wave, sockets)
            wave_record["suffixResults"] = [suffix_results[index] for index in sorted(suffix_results)]
            if len(suffix_results) != QUALIFIED_ACCEPTED or any(item.get("sendSuffixError") is not None or item.get("sendSuffixBytes") != 4 for item in suffix_results.values()):
                raise RuntimeError(f"wave {wave} did not send every request terminator while stopped")
            wave_record["suffixesFinishedMonotonicNs"] = time.monotonic_ns()
            os.killpg(process.pid, signal.SIGCONT)
            stopped = False
            wave_record["continuedMonotonicNs"] = time.monotonic_ns()
            results = finish_heads(evidence, registry, wave, sockets, suffix_results, args.timeout)
            finish_cdp(evidence, registry, wave, sockets, results, args.timeout)
            wave_record["results"] = results
            failures = classify_wave(results, QUALIFIED_ACCEPTED)
            qualification_failures = [item for item in failures if item.startswith("qualification-no-saturation:")]
            failures = [item for item in failures if not item.startswith("qualification-no-saturation:")]
            wave_record["qualification"] = "not-observed" if qualification_failures else "observed"
            wave_record["failures"] = failures
            wave_record["qualificationFailures"] = qualification_failures
            manifest["waves"].append(wave_record)
            if failures:
                manifest["failures"].extend([f"wave {wave}: {failure}" for failure in failures])
            recovery_snapshot, recovery_attempts = wait_stable_barrier(evidence, f"wave-{wave:02d}-recovery", process, args.host, port, baseline, 0, args.timeout, recovery=True)
            recovery_failures = validate_recovery(baseline, recovery_snapshot)
            wave_record["recovery"] = recovery_attempts
            wave_record["recoveryFailures"] = recovery_failures
            manifest["failures"].extend([f"wave {wave}: {failure}" for failure in recovery_failures])
            manifest["failures"].extend([f"wave {wave}: {failure}" for failure in recovery_probe(evidence, args.host, port, args.timeout, f"waves/wave-{wave:02d}/recovery-probe")])
        shutdown_result = shutdown(process, args.shutdown_timeout)
        manifest["shutdown"] = shutdown_result
        if shutdown_result.get("returncode") != 0 or shutdown_result.get("forcedKill") is not False or shutdown_result.get("groupGone") is not True:
            manifest["failures"].append("shutdown did not prove SIGTERM returncode 0, no forced kill, and groupGone")
        if manifest["failures"]:
            manifest["status"] = "failed"
            code = 1
        elif not all(any(item.get("classification") == "503-ws-handoff-saturated" for item in wave.get("results", [])) for wave in manifest["waves"]):
            manifest["status"] = "not-qualified"
            code = 2
        else:
            manifest["status"] = "passed"
            code = 0
    except BaseException as exc:
        manifest["status"] = "failed"
        manifest["failures"].append(repr(exc))
        manifest["traceback"] = write_trace(evidence, "runner.traceback.txt")
        if process is not None:
            if stopped:
                try:
                    os.killpg(process.pid, signal.SIGCONT)
                    stopped = False
                except ProcessLookupError:
                    pass
            manifest["shutdown"] = shutdown(process, args.shutdown_timeout)
        code = 1
    finally:
        if process is not None and stopped:
            try:
                os.killpg(process.pid, signal.SIGCONT)
            except ProcessLookupError:
                pass
        manifest["salvage"] = registry.salvage()
        manifest["finishedUtc"] = utc_now()
        manifest["artifacts"] = evidence.artifacts
        for stream in stream_handles:
            stream.close()
        if process is not None:
            server_info = manifest.setdefault("server", {})
            if isinstance(server_info, dict):
                server_info["stdoutArtifact"] = evidence.capture_file("server-final", evidence.root / "server.stdout.bin")
                server_info["stderrArtifact"] = evidence.capture_file("server-final", evidence.root / "server.stderr.bin")
        evidence.write_json("manifest.json", manifest)
    return manifest, code


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    try:
        _, code = run(args)
    except (ValueError, OSError) as exc:
        parser.error(str(exc))
    return code


if __name__ == "__main__":
    raise SystemExit(main())

#!/usr/bin/env python3
"""Qualify multi-worker readiness, failure, shutdown, and child reaping.

The runner executes independent real-process scenarios against one exact
Obscura binary. Every command, request, response, process-table probe, server
stream, signal, exception, and hash is retained under a new caller-owned
evidence directory. Captured data is never redacted, truncated, or field-
filtered.

Results qualify only the recorded host, operating system, architecture, and
binary. They do not qualify Windows, another Unix, or a container unless this
runner is executed there with a platform-specific signal scenario.
"""

from __future__ import annotations

import argparse
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
from dataclasses import dataclass
from pathlib import Path
from typing import Any

try:
    from .cdp_capacity import Evidence, json_bytes, parse_http_response, utc_now
    from .cdp_multi_worker import (
        probe_contiguous_ports,
        process_group_exists,
        sha256_file,
        wait_for_process_group_exit,
    )
except ImportError:
    from cdp_capacity import Evidence, json_bytes, parse_http_response, utc_now
    from cdp_multi_worker import (
        probe_contiguous_ports,
        process_group_exists,
        sha256_file,
        wait_for_process_group_exit,
    )


CONTROL_PROTOCOL = "obscura-multi-worker-control"
CONTROL_VERSION = 1
DEFAULT_STARTUP_TIMEOUT_SECONDS = 30.0
DEFAULT_SHUTDOWN_TIMEOUT_SECONDS = 15.0
DEFAULT_IO_TIMEOUT_SECONDS = 10.0


def discovery_request(
    authority: str,
    token: str | None = None,
    origin: str | None = None,
) -> bytes:
    lines = [
        "GET /json/version HTTP/1.1",
        f"Host: {authority}",
        "Connection: close",
    ]
    if token is not None:
        lines.append(f"Authorization: Bearer {token}")
    if origin is not None:
        lines.append(f"Origin: {origin}")
    return ("\r\n".join(lines) + "\r\n\r\n").encode("utf-8")


def socket_exchange(
    host: str, port: int, request: bytes, timeout: float
) -> tuple[bytes, str | None]:
    chunks: list[bytes] = []
    error: str | None = None
    try:
        with socket.create_connection((host, port), timeout=timeout) as connection:
            connection.settimeout(timeout)
            connection.sendall(request)
            while True:
                chunk = connection.recv(65_536)
                if not chunk:
                    break
                chunks.append(chunk)
    except BaseException as exc:
        error = repr(exc)
    return b"".join(chunks), error


def capture_exchange(
    evidence: Evidence,
    prefix: str,
    host: str,
    port: int,
    request: bytes,
    timeout: float,
) -> dict[str, object]:
    request_path = evidence.write_bytes(f"{prefix}.request.bin", request)
    started_ns = time.monotonic_ns()
    response, error = socket_exchange(host, port, request, timeout)
    response_path = evidence.write_bytes(f"{prefix}.response.bin", response)
    result = {
        "host": host,
        "port": port,
        "startedMonotonicNs": started_ns,
        "finishedMonotonicNs": time.monotonic_ns(),
        "error": error,
        "request": evidence.artifacts[request_path],
        "response": evidence.artifacts[response_path],
        "parsedResponse": parse_http_response(response),
    }
    result_path = evidence.write_json(f"{prefix}.result.json", result)
    result["result"] = evidence.artifacts[result_path]
    return result


def parse_ready_records(raw: bytes) -> tuple[list[dict[str, Any]], list[dict[str, Any]]]:
    records: list[dict[str, Any]] = []
    invalid: list[dict[str, Any]] = []
    for index, line in enumerate(raw.splitlines(keepends=True), start=1):
        if not line.lstrip().startswith(b"{"):
            continue
        try:
            value = json.loads(line)
        except BaseException as exc:
            invalid.append({"line": index, "rawHex": line.hex(), "error": repr(exc)})
            continue
        if isinstance(value, dict) and value.get("protocol") == CONTROL_PROTOCOL:
            records.append(value)
    return records, invalid


def validate_ready_records(
    records: list[dict[str, Any]], base_port: int, workers: int
) -> list[str]:
    failures: list[str] = []
    if len(records) != workers:
        failures.append(f"expected {workers} readiness records, got {len(records)}")
    expected = {(index, base_port + index) for index in range(1, workers + 1)}
    observed: set[tuple[object, object]] = set()
    pids: set[object] = set()
    for record in records:
        observed.add((record.get("worker"), record.get("port")))
        pid = record.get("pid")
        if not isinstance(pid, int) or pid <= 0:
            failures.append(f"invalid readiness pid: {record!r}")
        if pid in pids:
            failures.append(f"duplicate readiness pid: {pid!r}")
        pids.add(pid)
        if record.get("version") != CONTROL_VERSION:
            failures.append(f"unexpected readiness version: {record!r}")
        if record.get("event") != "ready":
            failures.append(f"unexpected readiness event: {record!r}")
        if set(record) != {"protocol", "version", "event", "worker", "port", "pid"}:
            failures.append(f"unexpected readiness fields: {record!r}")
    if observed != expected:
        failures.append(
            f"readiness worker/port mismatch: expected={sorted(expected)!r} observed={sorted(observed)!r}"
        )
    return failures


def parse_process_table(raw: bytes) -> list[dict[str, object]]:
    rows: list[dict[str, object]] = []
    for raw_line in raw.decode("utf-8", "surrogateescape").splitlines():
        fields = raw_line.strip().split(None, 4)
        if len(fields) < 5:
            continue
        try:
            pid, ppid, pgid = map(int, fields[:3])
        except ValueError:
            continue
        rows.append(
            {
                "pid": pid,
                "ppid": ppid,
                "pgid": pgid,
                "state": fields[3],
                "command": fields[4],
                "raw": raw_line,
            }
        )
    return rows


def process_table() -> tuple[list[str], bytes, bytes, int]:
    argv = ["ps", "-axww", "-o", "pid=,ppid=,pgid=,state=,command="]
    completed = subprocess.run(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=False)
    return argv, completed.stdout, completed.stderr, completed.returncode


def capture_process_table(
    evidence: Evidence, prefix: str, parent_pid: int, process_group: int
) -> dict[str, object]:
    argv, stdout, stderr, returncode = process_table()
    stdout_path = evidence.write_bytes(f"{prefix}.stdout.bin", stdout)
    stderr_path = evidence.write_bytes(f"{prefix}.stderr.bin", stderr)
    rows = parse_process_table(stdout)
    result = {
        "argv": argv,
        "returncode": returncode,
        "stdout": evidence.artifacts[stdout_path],
        "stderr": evidence.artifacts[stderr_path],
        "parentPid": parent_pid,
        "processGroup": process_group,
        "directChildren": [row for row in rows if row["ppid"] == parent_pid],
        "groupMembers": [row for row in rows if row["pgid"] == process_group],
    }
    result_path = evidence.write_json(f"{prefix}.result.json", result)
    result["result"] = evidence.artifacts[result_path]
    return result


def pid_exists(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def wait_pids_gone(pids: set[int], timeout: float) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    samples: list[dict[str, object]] = []
    while True:
        alive = sorted(pid for pid in pids if pid_exists(pid))
        samples.append({"monotonicNs": time.monotonic_ns(), "alive": alive})
        if not alive or time.monotonic() >= deadline:
            return {
                "pids": sorted(pids),
                "gone": not alive,
                "aliveFinal": alive,
                "samples": samples,
            }
        time.sleep(0.02)


def wait_process_exit(process: subprocess.Popen[bytes], timeout: float) -> bool:
    try:
        process.wait(timeout=timeout)
        return True
    except subprocess.TimeoutExpired:
        return False


@dataclass
class ServerProcess:
    name: str
    process: subprocess.Popen[bytes]
    process_group: int
    stdout_path: Path
    stderr_path: Path
    stdout_handle: Any
    stderr_handle: Any


def build_command(
    args: argparse.Namespace,
    port: int,
    *,
    parameter_projection: bool,
    font_dir: Path | None = None,
) -> tuple[list[str], dict[str, str], dict[str, str]]:
    token = "complete-lifecycle-token-0123456789abcdef+/=" if parameter_projection else ""
    proxy = "http://user:complete-proxy-secret-123@127.0.0.1:9" if parameter_projection else ""
    authority = f"qualification.test:{port}" if parameter_projection else f"{args.host}:{port}"
    origin = "https://console.qualification.example" if parameter_projection else ""
    advertised = "wss://qualification.example" if parameter_projection else ""
    command = [str(args.binary.resolve()), "--persona", args.persona]
    if parameter_projection:
        command.extend(["--v8-flags", "--expose-gc", "--verbose"])
    command.extend(
        [
            "serve", "--host", args.host, "--port", str(port),
            "--workers", str(args.workers),
            "--max-connections", str(args.max_connections),
        ]
    )
    if parameter_projection:
        command.extend(
            [
                "--quiet", "--allow-file-access", "--allow-private-network",
                "--allow-host", authority,
                "--allow-origin", origin,
                "--advertise-websocket-url", advertised,
            ]
        )
        if font_dir is not None:
            command.extend(["--font-dir", str(font_dir)])
    environment = os.environ.copy()
    if parameter_projection:
        environment.update({"OBSCURA_CDP_TOKEN": token, "OBSCURA_PROXY": proxy})
    metadata = {
        "token": token,
        "proxy": proxy,
        "authority": authority,
        "origin": origin,
        "advertisedWebSocketUrl": advertised,
    }
    return command, environment, metadata


def spawn_server(
    evidence: Evidence,
    args: argparse.Namespace,
    name: str,
    port: int,
    *,
    parameter_projection: bool = False,
) -> tuple[ServerProcess, dict[str, str]]:
    prefix = f"scenarios/{name}/server"
    font_dir: Path | None = None
    if parameter_projection:
        font_dir = evidence.root / "scenarios" / name / "font-fixture"
        font_dir.mkdir(parents=True, exist_ok=False)
    argv, environment, metadata = build_command(
        args, port, parameter_projection=parameter_projection, font_dir=font_dir
    )
    overrides = {
        key: environment[key]
        for key in ["OBSCURA_CDP_TOKEN", "OBSCURA_PROXY"]
        if key in environment
    }
    evidence.write_json(
        f"{prefix}.command.json",
        {"argv": argv, "environment": {"inherit": True, "overrides": overrides}},
    )
    stdout_path = evidence.root / f"{prefix}.stdout.bin"
    stderr_path = evidence.root / f"{prefix}.stderr.bin"
    stdout_path.parent.mkdir(parents=True, exist_ok=True)
    stdout_handle = stdout_path.open("xb")
    stderr_handle = stderr_path.open("xb")
    process = subprocess.Popen(
        argv,
        stdin=subprocess.DEVNULL,
        stdout=stdout_handle,
        stderr=stderr_handle,
        env=environment,
        start_new_session=True,
    )
    server = ServerProcess(
        name, process, os.getpgid(process.pid), stdout_path, stderr_path,
        stdout_handle, stderr_handle,
    )
    evidence.write_json(
        f"{prefix}.spawn.json",
        {
            "argv": argv,
            "environment": {"inherit": True, "overrides": overrides},
            "pid": process.pid,
            "processGroup": server.process_group,
            "startedUtc": utc_now(),
            "startedMonotonicNs": time.monotonic_ns(),
        },
    )
    return server, metadata


def wait_ready_records(
    server: ServerProcess, workers: int, timeout: float
) -> tuple[list[dict[str, Any]], list[dict[str, Any]], bytes]:
    deadline = time.monotonic() + timeout
    raw = b""
    while time.monotonic() < deadline:
        raw = server.stdout_path.read_bytes()
        records, invalid = parse_ready_records(raw)
        if len(records) >= workers or server.process.poll() is not None:
            return records, invalid, raw
        time.sleep(0.02)
    records, invalid = parse_ready_records(raw)
    return records, invalid, raw


def wait_public_ready(
    evidence: Evidence,
    prefix: str,
    server: ServerProcess,
    host: str,
    port: int,
    authority: str,
    token: str | None,
    origin: str | None,
    timeout: float,
) -> dict[str, object]:
    deadline = time.monotonic() + timeout
    attempts: list[dict[str, object]] = []
    while time.monotonic() < deadline:
        result = capture_exchange(
            evidence,
            f"{prefix}/attempt-{len(attempts):03d}",
            host,
            port,
            discovery_request(authority, token, origin),
            min(1.0, timeout),
        )
        attempts.append(result)
        if result["parsedResponse"].get("status") == 200:
            return {"ready": True, "attempts": attempts, "final": result}
        if server.process.poll() is not None:
            break
        time.sleep(0.05)
    return {"ready": False, "attempts": attempts, "final": attempts[-1] if attempts else None}


def finish_server_evidence(
    evidence: Evidence, server: ServerProcess, streams_stable: bool
) -> None:
    server.stdout_handle.flush()
    server.stderr_handle.flush()
    server.stdout_handle.close()
    server.stderr_handle.close()
    if streams_stable:
        evidence.record_path(str(server.stdout_path.relative_to(evidence.root)))
        evidence.record_path(str(server.stderr_path.relative_to(evidence.root)))


def cleanup_server(evidence: Evidence, server: ServerProcess, timeout: float) -> dict[str, object]:
    cleanup: dict[str, object] = {
        "startedMonotonicNs": time.monotonic_ns(),
        "forcedTerm": False,
        "forcedKill": False,
        "errors": [],
    }
    if server.process.poll() is None:
        cleanup["forcedTerm"] = True
        try:
            os.killpg(server.process_group, signal.SIGTERM)
        except ProcessLookupError:
            pass
        except BaseException as exc:
            cleanup["errors"].append(repr(exc))
        wait_process_exit(server.process, timeout)
    if process_group_exists(server.process_group):
        cleanup["forcedKill"] = True
        try:
            os.killpg(server.process_group, signal.SIGKILL)
        except ProcessLookupError:
            pass
        except BaseException as exc:
            cleanup["errors"].append(repr(exc))
    cleanup["groupGone"] = wait_for_process_group_exit(server.process_group, timeout)
    if server.process.poll() is None:
        wait_process_exit(server.process, timeout)
    cleanup["returncode"] = server.process.returncode
    cleanup["serverStreamsStable"] = cleanup["groupGone"]
    cleanup["finishedMonotonicNs"] = time.monotonic_ns()
    evidence.write_json(f"scenarios/{server.name}/cleanup.json", cleanup)
    finish_server_evidence(evidence, server, bool(cleanup["serverStreamsStable"]))
    return cleanup


def apply_cleanup_failures(result: dict[str, Any]) -> None:
    cleanup = result.get("cleanup")
    if not isinstance(cleanup, dict):
        return
    failures = result.setdefault("failures", [])
    if cleanup.get("errors"):
        failures.append(f"cleanup reported errors: {cleanup['errors']!r}")
    if not cleanup.get("groupGone"):
        failures.append("cleanup left process-group members alive")
    if cleanup.get("returncode") is None:
        failures.append("cleanup did not reap the parent process")
    if cleanup.get("forcedTerm") or cleanup.get("forcedKill"):
        failures.append(
            "scenario required fallback process-group cleanup: "
            f"forcedTerm={cleanup.get('forcedTerm')!r} forcedKill={cleanup.get('forcedKill')!r}"
        )
    if not cleanup.get("serverStreamsStable"):
        failures.append("server streams could not be finalized after process-group cleanup")


def sample_until_exit(
    evidence: Evidence,
    server: ServerProcess,
    prefix: str,
    timeout: float,
) -> tuple[bool, set[int], list[dict[str, object]]]:
    deadline = time.monotonic() + timeout
    seen: set[int] = set()
    samples: list[dict[str, object]] = []
    while True:
        sample = capture_process_table(
            evidence, f"{prefix}/sample-{len(samples):03d}",
            server.process.pid, server.process_group
        )
        samples.append(sample)
        seen.update(int(row["pid"]) for row in sample["directChildren"])
        if server.process.poll() is not None:
            return True, seen, samples
        if time.monotonic() >= deadline:
            return False, seen, samples
        time.sleep(0.02)


def base_result(name: str, port: int, server: ServerProcess) -> dict[str, Any]:
    return {
        "name": name, "port": port, "pid": server.process.pid,
        "processGroup": server.process_group, "failures": [],
    }


def run_public_port_conflict(evidence: Evidence, args: argparse.Namespace) -> dict[str, Any]:
    name = "public-port-conflict"
    port = probe_contiguous_ports(args.host, args.workers + 1)
    reservation = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    reservation.bind((args.host, port))
    reservation.listen(1)
    evidence.write_json(f"scenarios/{name}/reservation.json", {"address": list(reservation.getsockname())})
    server, _ = spawn_server(evidence, args, name, port)
    result = base_result(name, port, server)
    try:
        exited, seen, samples = sample_until_exit(
            evidence, server, f"scenarios/{name}/process-polls", min(5.0, args.startup_timeout)
        )
        result.update({"exited": exited, "observedChildPids": sorted(seen), "samples": samples})
        if not exited:
            result["failures"].append("parent did not fail promptly while public port was occupied")
        if server.process.returncode == 0:
            result["failures"].append("public-port conflict returned success")
        if seen:
            result["failures"].append(f"children observed despite public bind failure: {sorted(seen)!r}")
    finally:
        reservation.close()
        result["cleanup"] = cleanup_server(evidence, server, args.shutdown_timeout)
    return result


def run_worker_port_conflict(evidence: Evidence, args: argparse.Namespace) -> dict[str, Any]:
    name = "worker-port-conflict"
    port = probe_contiguous_ports(args.host, args.workers + 1)
    occupied_port = port + args.workers
    reservation = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    reservation.bind((args.host, occupied_port))
    reservation.listen(1)
    evidence.write_json(f"scenarios/{name}/reservation.json", {"address": list(reservation.getsockname())})
    server, _ = spawn_server(evidence, args, name, port)
    result = base_result(name, port, server)
    try:
        exited, seen, samples = sample_until_exit(
            evidence, server, f"scenarios/{name}/process-polls", args.startup_timeout
        )
        raw = server.stdout_path.read_bytes()
        records, invalid = parse_ready_records(raw)
        known = seen | {int(record["pid"]) for record in records if isinstance(record.get("pid"), int)}
        gone = wait_pids_gone(known, args.shutdown_timeout)
        evidence.write_json(f"scenarios/{name}/children-gone.json", gone)
        result.update({
            "occupiedWorkerPort": occupied_port, "exited": exited,
            "observedChildPids": sorted(seen), "readinessRecords": records,
            "invalidJsonLines": invalid, "stdoutBytesObserved": len(raw),
            "childrenGone": gone, "samples": samples,
        })
        if not exited:
            result["failures"].append("parent did not fail within startup timeout for worker bind conflict")
        if server.process.returncode == 0:
            result["failures"].append("worker-port conflict returned success")
        if not gone["gone"]:
            result["failures"].append(f"worker-port conflict left children alive: {gone['aliveFinal']!r}")
    finally:
        reservation.close()
        result["cleanup"] = cleanup_server(evidence, server, args.shutdown_timeout)
    stderr = server.stderr_path.read_bytes()
    expected = f"bind {args.host}:{occupied_port}".encode("utf-8")
    result["workerBindFailureInRawStderr"] = expected in stderr
    if expected not in stderr:
        result["failures"].append(
            f"raw child stderr did not contain the occupied worker bind failure {expected!r}"
        )
    if not result["cleanup"]["groupGone"]:
        result["failures"].append("worker-port conflict left process-group members alive")
    return result


def wait_normal_readiness(
    evidence: Evidence,
    args: argparse.Namespace,
    server: ServerProcess,
    metadata: dict[str, str],
    port: int,
    prefix: str,
) -> tuple[list[dict[str, Any]], dict[str, object], dict[str, object], list[str]]:
    records, invalid, raw = wait_ready_records(server, args.workers, args.startup_timeout)
    evidence.write_json(
        f"{prefix}/readiness-records.json",
        {"records": records, "invalidJsonLines": invalid, "stdoutBytesObserved": len(raw)},
    )
    failures = validate_ready_records(records, port, args.workers)
    if invalid:
        failures.append(f"invalid JSON-like stdout lines observed: {invalid!r}")
    public = wait_public_ready(
        evidence, f"{prefix}/public-readiness", server, args.host, port,
        metadata["authority"], metadata["token"] or None,
        metadata["origin"] or None, args.startup_timeout
    )
    if not public["ready"]:
        failures.append("public discovery did not become ready after readiness records")
    process = capture_process_table(
        evidence, f"{prefix}/ready-process-table", server.process.pid, server.process_group
    )
    direct = {int(row["pid"]) for row in process["directChildren"]}
    record_pids = {int(record["pid"]) for record in records if isinstance(record.get("pid"), int)}
    if direct != record_pids:
        failures.append(f"ready PID mismatch: processTable={sorted(direct)!r} records={sorted(record_pids)!r}")
    return records, public, process, failures


def run_child_crash(evidence: Evidence, args: argparse.Namespace) -> dict[str, Any]:
    name = "ready-child-crash"
    port = probe_contiguous_ports(args.host, args.workers + 1)
    server, metadata = spawn_server(evidence, args, name, port)
    result = base_result(name, port, server)
    try:
        records, public, process, failures = wait_normal_readiness(
            evidence, args, server, metadata, port, f"scenarios/{name}"
        )
        result["failures"].extend(failures)
        child_pids = {int(record["pid"]) for record in records if isinstance(record.get("pid"), int)}
        result.update({"readinessRecords": records, "publicReadiness": public, "processAtReady": process})
        if server.process.poll() is not None:
            result["failures"].append(
                f"parent exited before crash injection with {server.process.returncode!r}"
            )
            return result
        if not child_pids:
            result["failures"].append("no ready child PID available for crash injection")
        else:
            victim = min(child_pids)
            signal_event = {"targetPid": victim, "signal": "SIGKILL", "sentMonotonicNs": time.monotonic_ns()}
            os.kill(victim, signal.SIGKILL)
            evidence.write_json(f"scenarios/{name}/signal.json", signal_event)
            exited = wait_process_exit(server.process, args.shutdown_timeout)
            gone = wait_pids_gone(child_pids, args.shutdown_timeout)
            evidence.write_json(f"scenarios/{name}/children-gone.json", gone)
            result.update({"signal": signal_event, "exited": exited, "childrenGone": gone})
            if not exited:
                result["failures"].append("parent did not fail after a ready child was killed")
            if server.process.returncode == 0:
                result["failures"].append("parent returned success after unexpected child death")
            if not gone["gone"]:
                result["failures"].append(f"child crash left child PIDs alive: {gone['aliveFinal']!r}")
    finally:
        result["cleanup"] = cleanup_server(evidence, server, args.shutdown_timeout)
    return result


def validate_parameter_projection(
    rows: list[dict[str, object]],
    records: list[dict[str, Any]],
    metadata: dict[str, str],
    font_dir: Path,
) -> list[str]:
    failures: list[str] = []
    by_pid = {int(row["pid"]): str(row["command"]) for row in rows}
    required = [
        "--v8-flags", "--expose-gc", "--verbose", "serve",
        "--host 127.0.0.1", "--workers 1", "--max-connections",
        "--supervised-worker", "--quiet", "--allow-file-access",
        "--allow-private-network", f"--allow-host {metadata['authority']}",
        f"--allow-origin {metadata['origin']}",
        f"--advertise-websocket-url {metadata['advertisedWebSocketUrl']}",
        f"--font-dir {font_dir}",
    ]
    for record in records:
        pid = int(record["pid"])
        command = by_pid.get(pid)
        if command is None:
            failures.append(f"missing process-table command for worker pid {pid}")
            continue
        for fragment in required:
            if fragment not in command:
                failures.append(f"worker {pid} command omitted {fragment!r}: {command!r}")
        for secret_name in ["token", "proxy"]:
            secret = metadata[secret_name]
            if secret and secret in command:
                failures.append(f"worker {pid} argv exposed {secret_name}: {command!r}")
    return failures


def run_parent_sigterm(evidence: Evidence, args: argparse.Namespace) -> dict[str, Any]:
    name = "parent-only-sigterm-and-parameters"
    port = probe_contiguous_ports(args.host, args.workers + 1)
    server, metadata = spawn_server(evidence, args, name, port, parameter_projection=True)
    result = base_result(name, port, server)
    try:
        records, public, process, failures = wait_normal_readiness(
            evidence, args, server, metadata, port, f"scenarios/{name}"
        )
        result["failures"].extend(failures)
        child_pids = {int(record["pid"]) for record in records if isinstance(record.get("pid"), int)}
        result.update({"readinessRecords": records, "publicReadiness": public, "processAtReady": process})
        if server.process.poll() is not None:
            result["failures"].append(
                f"parent exited before parameter and SIGTERM checks with {server.process.returncode!r}"
            )
            return result
        font_dir = evidence.root / "scenarios" / name / "font-fixture"
        result["failures"].extend(
            validate_parameter_projection(process["groupMembers"], records, metadata, font_dir)
        )
        unauthorized = capture_exchange(
            evidence, f"scenarios/{name}/access/without-token", args.host, port,
            discovery_request(metadata["authority"], None, metadata["origin"]), args.io_timeout
        )
        invalid_host = capture_exchange(
            evidence, f"scenarios/{name}/access/invalid-host", args.host, port,
            discovery_request(f"invalid.example:{port}", metadata["token"], metadata["origin"]), args.io_timeout
        )
        authorized = capture_exchange(
            evidence, f"scenarios/{name}/access/authorized", args.host, port,
            discovery_request(metadata["authority"], metadata["token"], metadata["origin"]), args.io_timeout
        )
        if unauthorized["parsedResponse"].get("status") != 401:
            result["failures"].append(f"child did not enforce bearer token: {unauthorized!r}")
        if invalid_host["parsedResponse"].get("status") != 421:
            result["failures"].append(f"child did not enforce allowed Host: {invalid_host!r}")
        authorized_json = authorized["parsedResponse"].get("json")
        if authorized["parsedResponse"].get("status") != 200:
            result["failures"].append(f"authorized discovery failed: {authorized!r}")
        elif not isinstance(authorized_json, dict) or authorized_json.get("webSocketDebuggerUrl") != metadata["advertisedWebSocketUrl"] + "/devtools/browser":
            result["failures"].append(f"advertised WebSocket URL was not propagated: {authorized_json!r}")
        signal_event = {
            "targetPid": server.process.pid, "targetScope": "parent-pid-only",
            "signal": "SIGTERM", "sentMonotonicNs": time.monotonic_ns(),
        }
        os.kill(server.process.pid, signal.SIGTERM)
        evidence.write_json(f"scenarios/{name}/signal.json", signal_event)
        exited = wait_process_exit(server.process, args.shutdown_timeout)
        gone = wait_pids_gone(child_pids, args.shutdown_timeout)
        group_gone = wait_for_process_group_exit(server.process_group, args.shutdown_timeout)
        evidence.write_json(f"scenarios/{name}/children-gone.json", gone)
        result.update({
            "access": {"withoutToken": unauthorized, "invalidHost": invalid_host, "authorized": authorized},
            "signal": signal_event, "exited": exited,
            "childrenGone": gone, "groupGone": group_gone,
        })
        if not exited:
            result["failures"].append("parent did not exit after parent-only SIGTERM")
        if server.process.returncode != 0:
            result["failures"].append(f"parent-only SIGTERM returned {server.process.returncode!r}, expected 0")
        if not gone["gone"]:
            result["failures"].append(f"parent-only SIGTERM left child PIDs alive: {gone['aliveFinal']!r}")
        if not group_gone:
            result["failures"].append("parent-only SIGTERM left process-group members alive")
    finally:
        result["cleanup"] = cleanup_server(evidence, server, args.shutdown_timeout)
    return result


def run_parent_sigkill(evidence: Evidence, args: argparse.Namespace) -> dict[str, Any]:
    name = "parent-sigkill-stdin-eof"
    port = probe_contiguous_ports(args.host, args.workers + 1)
    server, metadata = spawn_server(evidence, args, name, port)
    result = base_result(name, port, server)
    try:
        records, public, process, failures = wait_normal_readiness(
            evidence, args, server, metadata, port, f"scenarios/{name}"
        )
        result["failures"].extend(failures)
        child_pids = {int(record["pid"]) for record in records if isinstance(record.get("pid"), int)}
        result.update({"readinessRecords": records, "publicReadiness": public, "processAtReady": process})
        if server.process.poll() is not None:
            result["failures"].append(
                f"parent exited before SIGKILL injection with {server.process.returncode!r}"
            )
            return result
        signal_event = {
            "targetPid": server.process.pid, "targetScope": "parent-pid-only",
            "signal": "SIGKILL", "sentMonotonicNs": time.monotonic_ns(),
        }
        os.kill(server.process.pid, signal.SIGKILL)
        evidence.write_json(f"scenarios/{name}/signal.json", signal_event)
        exited = wait_process_exit(server.process, args.shutdown_timeout)
        gone = wait_pids_gone(child_pids, args.shutdown_timeout)
        group_gone = wait_for_process_group_exit(server.process_group, args.shutdown_timeout)
        evidence.write_json(f"scenarios/{name}/children-gone.json", gone)
        result.update({
            "signal": signal_event,
            "exited": exited, "childrenGone": gone, "groupGone": group_gone,
        })
        if not exited:
            result["failures"].append("parent did not terminate after SIGKILL")
        if server.process.returncode != -signal.SIGKILL:
            result["failures"].append(f"parent SIGKILL returned {server.process.returncode!r}")
        if not gone["gone"]:
            result["failures"].append(f"stdin EOF did not stop child PIDs: {gone['aliveFinal']!r}")
        if not group_gone:
            result["failures"].append("parent SIGKILL left child process-group members alive")
    finally:
        result["cleanup"] = cleanup_server(evidence, server, args.shutdown_timeout)
    return result


def run(args: argparse.Namespace) -> tuple[dict[str, Any], int]:
    evidence = Evidence(args.output)
    binary = args.binary.resolve()
    manifest: dict[str, Any] = {
        "schemaVersion": 1,
        "status": "running",
        "startedUtc": utc_now(),
        "scope": {
            "qualified": [
                "public bind failure returned before any child process was observed",
                "worker bind failure causes sibling shutdown and reaping",
                "exact bounded worker readiness records",
                "ready child crash causes parent fail-fast and sibling shutdown",
                "parent-PID-only SIGTERM causes graceful child shutdown and reaping",
                "parent SIGKILL closes control pipes and children exit on stdin EOF",
                "worker argv projection plus actual Host, Bearer, and advertised URL behavior",
            ],
            "notQualified": [
                "automatic worker restart or session migration",
                "shared multi-worker storage ownership",
                "Windows console control or process-handle lifecycle",
                "another operating system, container namespace, init system, or binary",
                "total process, FD, RSS, V8 heap, or socket-buffer capacity",
            ],
        },
        "platform": {
            "system": platform.system(), "release": platform.release(),
            "version": platform.version(), "machine": platform.machine(),
            "python": sys.version, "uname": list(platform.uname()),
            "rlimitNofile": list(resource.getrlimit(resource.RLIMIT_NOFILE)),
        },
        "configuration": {
            "binary": str(binary), "binarySha256": None, "binaryBytes": None,
            "host": args.host, "persona": args.persona, "workers": args.workers,
            "maxConnectionsPerWorker": args.max_connections,
            "startupTimeoutSeconds": args.startup_timeout,
            "shutdownTimeoutSeconds": args.shutdown_timeout,
            "ioTimeoutSeconds": args.io_timeout,
            "ports": "independently probed free contiguous ranges; released before process spawn",
        },
        "scenarios": [], "failures": [],
    }
    exit_code = 1
    try:
        if os.name == "nt":
            raise RuntimeError("this qualifier uses Unix signals; Windows needs a platform-specific lifecycle runner")
        if not binary.is_file():
            raise FileNotFoundError(f"missing Obscura binary: {binary}")
        if args.workers < 2:
            raise ValueError("--workers must be at least 2")
        if args.max_connections < 1:
            raise ValueError("--max-connections must be positive")
        digest, size = sha256_file(binary)
        manifest["configuration"]["binarySha256"] = digest
        manifest["configuration"]["binaryBytes"] = size
        for scenario in [
            run_public_port_conflict, run_worker_port_conflict, run_child_crash,
            run_parent_sigterm, run_parent_sigkill,
        ]:
            try:
                result = scenario(evidence, args)
            except BaseException as exc:
                name = scenario.__name__.removeprefix("run_").replace("_", "-")
                trace_path = evidence.write_bytes(
                    f"scenarios/{name}/failure.traceback.txt", traceback.format_exc().encode("utf-8")
                )
                result = {
                    "name": name, "failures": [repr(exc)],
                    "exceptionType": type(exc).__name__, "traceback": evidence.artifacts[trace_path],
                }
            apply_cleanup_failures(result)
            manifest["scenarios"].append(result)
            manifest["failures"].extend(
                f"{result['name']}: {failure}" for failure in result.get("failures", [])
            )
        manifest["status"] = "passed" if not manifest["failures"] else "failed"
        exit_code = 0 if manifest["status"] == "passed" else 1
    except BaseException as exc:
        manifest["status"] = "failed"
        manifest["failures"].append(repr(exc))
        manifest["exceptionType"] = type(exc).__name__
        trace_path = evidence.write_bytes("failure.traceback.txt", traceback.format_exc().encode("utf-8"))
        manifest["traceback"] = evidence.artifacts[trace_path]
    finally:
        manifest["finishedUtc"] = utc_now()
        manifest["artifacts"] = sorted(evidence.artifacts.values(), key=lambda item: str(item["path"]))
        with (evidence.root / "evidence.json").open("xb") as output:
            output.write(json_bytes(manifest))
    return manifest, exit_code


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--persona", default="windows_chrome145")
    parser.add_argument("--workers", type=int, default=2)
    parser.add_argument("--max-connections", type=int, default=1)
    parser.add_argument("--startup-timeout", type=float, default=DEFAULT_STARTUP_TIMEOUT_SECONDS)
    parser.add_argument("--shutdown-timeout", type=float, default=DEFAULT_SHUTDOWN_TIMEOUT_SECONDS)
    parser.add_argument("--io-timeout", type=float, default=DEFAULT_IO_TIMEOUT_SECONDS)
    return parser.parse_args()


def main() -> None:
    _, exit_code = run(parse_args())
    raise SystemExit(exit_code)


if __name__ == "__main__":
    main()

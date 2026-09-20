#!/usr/bin/env python3
"""Official-client gates required before retiring the private SDK/runtime."""

from __future__ import annotations

import argparse
import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from typing import Any

if __package__:
    from .cdp_fixture import error_record, external_process, free_port, terminate_process_group
else:
    from cdp_fixture import error_record, external_process, free_port, terminate_process_group


CASES = ("no-replay", "click-watchdog", "script-cors")
WORKER_TIMEOUT_SECONDS = 45


def capture_worker(command: list[str], directory: Path, timeout: float) -> dict[str, Any]:
    directory.mkdir(parents=True, exist_ok=True)
    stdout_path = directory / "worker.stdout.bin"
    stderr_path = directory / "worker.stderr.bin"
    capture: dict[str, Any] = {
        "command": command,
        "stdoutPath": str(stdout_path),
        "stderrPath": str(stderr_path),
        "hardDeadlineSeconds": timeout,
        "status": "starting",
    }
    environment = os.environ.copy()
    environment["PYTHONUNBUFFERED"] = "1"
    debug = environment.get("DEBUG", "")
    environment["DEBUG"] = f"{debug},pw:protocol" if debug else "pw:protocol"
    started = time.monotonic()
    process = None
    with stdout_path.open("wb") as stdout, stderr_path.open("wb") as stderr:
        try:
            process = subprocess.Popen(
                command, stdout=stdout, stderr=stderr, stdin=subprocess.DEVNULL,
                start_new_session=True, env=environment,
            )
            capture["pid"] = process.pid
            try:
                process.wait(timeout=timeout)
                capture["status"] = "passed" if process.returncode == 0 else "failed"
            except subprocess.TimeoutExpired as error:
                capture["status"] = "hard-timeout"
                capture["error"] = error_record(error)
        except Exception as error:
            capture["status"] = "failed"
            capture["error"] = error_record(error)
        finally:
            if process is not None:
                try:
                    terminate_process_group(process)
                except Exception as error:
                    capture["cleanupError"] = error_record(error)
                    capture["status"] = "cleanup-failed"
                capture["returncode"] = process.poll()
            capture["elapsedSeconds"] = time.monotonic() - started
    return capture


async def worker(case: str, endpoint: str, output: Path) -> int:
    result: dict[str, Any] = {"case": case, "status": "running", "observations": {}}
    try:
        if case == "script-cors":
            if __package__:
                from .migration_cors import run_case
            else:
                from migration_cors import run_case
            await run_case(endpoint, result["observations"])
        else:
            if __package__:
                from .migration_lifecycle import run_case
            else:
                from migration_lifecycle import run_case
            await run_case(case, endpoint, result["observations"])
        result["status"] = "passed"
    except Exception as error:
        result["status"] = "failed"
        result["error"] = error_record(error)
    finally:
        output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(result), flush=True)
    return 0 if result["status"] == "passed" else 1


def run(binary: Path, output: Path, cases: list[str]) -> dict[str, Any]:
    output.parent.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="migration-smoke-", dir=output.parent)).resolve()
    result: dict[str, Any] = {"schemaVersion": 1, "status": "running", "cases": {}}
    for case in cases:
        directory = root / case
        directory.mkdir()
        endpoint = f"http://127.0.0.1:{free_port()}"
        record: dict[str, Any] = {"status": "running", "browserCapture": {}}
        result["cases"][case] = record
        command = [str(binary.resolve()), "--allow-private-network", "serve",
                   "--host", "127.0.0.1", "--port", endpoint.rsplit(":", 1)[1]]
        if case == "click-watchdog":
            command = ["env", "OBSCURA_CDP_COMMAND_TIMEOUT_MS=2000", *command]
            record["configuredCommandTimeoutMs"] = 2000
        child_output = directory / "worker-result.json"
        try:
            # The parent owns the browser even if the client worker hangs.
            with external_process(command, endpoint, capture=record["browserCapture"],
                                  log_root=directory):
                record["workerCapture"] = capture_worker(
                    [sys.executable, str(Path(__file__).resolve()), "--worker",
                     "--case", case, "--endpoint", endpoint, "--output", str(child_output)],
                    directory, WORKER_TIMEOUT_SECONDS,
                )
                if child_output.exists():
                    record["workerResult"] = json.loads(child_output.read_text(encoding="utf-8"))
                record["status"] = (
                    "passed" if record["workerCapture"]["status"] == "passed"
                    and record.get("workerResult", {}).get("status") == "passed" else "failed"
                )
        except Exception as error:
            record["status"] = "failed"
            record["error"] = error_record(error)
        output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    result["status"] = "passed" if all(
        item["status"] == "passed" for item in result["cases"].values()
    ) else "failed"
    output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--obscura-bin", type=Path, default=Path("target/release/obscura"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--case", choices=CASES, action="append")
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--endpoint", help=argparse.SUPPRESS)
    args = parser.parse_args()
    if args.worker:
        if not args.endpoint or not args.case or len(args.case) != 1:
            parser.error("worker needs one case and an endpoint")
        return asyncio.run(worker(args.case[0], args.endpoint, args.output))
    result = run(args.obscura_bin, args.output.resolve(), args.case or list(CASES))
    print(json.dumps(result, indent=2))
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())

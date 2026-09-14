"""Bounded browser messages and durable private runtime state."""

import asyncio
import hashlib
import json
import os
from pathlib import Path
import tempfile

LINE_LIMIT = 64 * 1024


def file_hash(path):
    with Path(path).open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def encode(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False).encode()


def private_dir(path):
    path = Path(path)
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    if path.is_symlink():
        raise ValueError("state directory cannot be a symlink")
    path.chmod(0o700)
    return path


def sync_dir(path):
    fd = os.open(path, os.O_RDONLY)
    try:
        os.fsync(fd)
    finally:
        os.close(fd)


def write_json(path, value):
    path = Path(path)
    data = encode(value) + b"\n"
    fd, tmp = tempfile.mkstemp(prefix=".pending-", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as output:
            output.write(data)
            output.flush()
            os.fsync(output.fileno())
        os.replace(tmp, path)
        sync_dir(path.parent)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


async def read_message(reader):
    try:
        line = await reader.readline()
    except (ValueError, asyncio.LimitOverrunError) as error:
        raise ValueError("protocol line exceeds limit") from error
    if not line:
        return None
    if len(line) > LINE_LIMIT or not line.endswith(b"\n"):
        raise ValueError("oversized or incomplete protocol line")
    value = json.loads(line, parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite JSON")))
    if not isinstance(value, dict):
        raise ValueError("protocol message must be an object")
    return value

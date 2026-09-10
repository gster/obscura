"""An immutable Rust browser process with an independent RPC reader."""

import asyncio
from dataclasses import dataclass
import hashlib
from pathlib import Path
import uuid

import psutil

from ._io import LINE_LIMIT, encode, file_hash, private_dir, read_message, write_json


class BrowserError(Exception):
    def __init__(self, code, dispatch_state="NOT_SENT"):
        super().__init__(code)
        self.code = code
        self.dispatch_state = dispatch_state


@dataclass(frozen=True)
class PageRef:
    page_id: str
    page_generation: int


@dataclass(frozen=True)
class ClickResult:
    page: PageRef
    default_prevented: bool


class BrowserSession:
    @classmethod
    async def start(cls, spec, workspace, persona, allowed_origins, initial_mode="PAUSED"):
        binary = Path(spec["binary"]).resolve(strict=True)
        if file_hash(binary) != spec["sha256"]:
            raise BrowserError("BROWSER_BINARY_MISMATCH")
        self = cls()
        self.workspace = private_dir(workspace).resolve(strict=True)
        self.sequence = 0
        self.pending = {}
        self.broken = None
        self.closing = False
        self.action_lock = asyncio.Lock()
        self.write_lock = asyncio.Lock()
        self.record = {"owner": "browser_" + uuid.uuid4().hex, "binary": str(binary), "sha256": spec["sha256"]}
        write_json(self.workspace / "browser-runtime.json", self.record)
        spawn = asyncio.create_task(asyncio.create_subprocess_exec(
            str(binary), "--workspace", str(self.workspace), "--owner", self.record["owner"],
            cwd=self.workspace, env={"PATH": "/usr/bin:/bin", "LANG": "C.UTF-8", "TZ": "UTC"},
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE,
            limit=LINE_LIMIT,
        ))
        try:
            self.process = await asyncio.shield(spawn)
        except asyncio.CancelledError:
            # Creation may already have spawned Rust without returning its handle.
            # Retain ownership until it is reaped; never proceed to init after cancel.
            self.process = await spawn
            if self.process.returncode is None:
                try:
                    self.process.kill()
                except ProcessLookupError:
                    pass
            await self.process.wait()
            self.record.update(pid=self.process.pid, closed=True, exit_code=self.process.returncode)
            write_json(self.workspace / "browser-runtime.json", self.record)
            raise
        # Rust remains in the Executor's process group/container, including before init.
        self.readers = [asyncio.create_task(self._responses()), asyncio.create_task(self._stderr())]
        try:
            self.record.update(pid=self.process.pid, create_time=psutil.Process(self.process.pid).create_time())
            write_json(self.workspace / "browser-runtime.json", self.record)
            ready = await self._call("init", {"protocol_version": "1", "runtime_sha256": spec["sha256"],
                "initial_mode": initial_mode, "persona": persona, "allowed_origins": allowed_origins,
                **({"proxy_url": spec["proxy_url"]} if "proxy_url" in spec else {})}, timeout_ms=5000)
            if (ready.get("ready") is not True or ready.get("protocol_version") != "1"
                    or ready.get("runtime_sha256") != spec["sha256"]
                    or ready.get("runtime_version") != "br_" + spec["sha256"][:24]
                    or ready.get("mode") != initial_mode or ready.get("generation") != 0):
                raise BrowserError("BROWSER_READY_MISMATCH")
            self.ready = ready
            return self
        except BaseException:
            await self.close()
            raise

    def _fail(self, error):
        if self.broken is None:
            self.broken = error
        for future in self.pending.values():
            if not future.done():
                future.set_exception(self.broken)
        if self.process.returncode is None:
            try:
                self.process.kill()
            except ProcessLookupError:
                pass

    async def attach_takeover(self):
        return await self._call("attach_takeover", {}, timeout_ms=3000)

    async def _responses(self):
        try:
            while (response := await read_message(self.process.stdout)) is not None:
                request_id = response.get("id")
                if type(request_id) is not int or request_id not in self.pending or type(response.get("ok")) is not bool:
                    raise BrowserError("BROWSER_PROTOCOL_FAILED", "UNKNOWN")
                if response["ok"]:
                    if not isinstance(response.get("result"), dict):
                        raise BrowserError("BROWSER_PROTOCOL_FAILED", "UNKNOWN")
                elif (not isinstance(response.get("error"), dict)
                        or not isinstance(response["error"].get("code"), str)
                        or response.get("dispatch_state") not in {"NOT_SENT", "SENT", "UNKNOWN"}):
                    raise BrowserError("BROWSER_PROTOCOL_FAILED", "UNKNOWN")
                future = self.pending[request_id]
                if future.done():
                    raise BrowserError("DUPLICATE_BROWSER_RESPONSE", "UNKNOWN")
                future.set_result(response)
            if not self.closing:
                self._fail(BrowserError("BROWSER_EOF", "UNKNOWN"))
        except asyncio.CancelledError:
            raise
        except Exception:
            self._fail(BrowserError("BROWSER_PROTOCOL_FAILED", "UNKNOWN"))

    async def _stderr(self):
        total, digest = 0, hashlib.sha256()
        try:
            while chunk := await self.process.stderr.read(8192):
                total += len(chunk)
                digest.update(chunk)
                if total > 10 * 1024 * 1024:
                    raise BrowserError("BROWSER_LOG_LIMIT", "UNKNOWN")
            write_json(self.workspace / "browser-stderr.json", {"bytes": total, "sha256": digest.hexdigest()})
        except asyncio.CancelledError:
            raise
        except Exception:
            self._fail(BrowserError("BROWSER_LOG_FAILED", "UNKNOWN"))

    async def _call(self, method, params, page=None, timeout_ms=5000):
        if self.broken:
            raise self.broken
        if type(timeout_ms) is not int or not 1 <= timeout_ms <= 30000:
            raise BrowserError("INVALID_BROWSER_TIMEOUT")
        future = None
        request_id = None
        sent = False
        try:
            async with asyncio.timeout(timeout_ms / 1000 + 0.5):
                async with self.write_lock:
                    self.sequence += 1
                    request_id = self.sequence
                    message = {"id": request_id, "method": method, "params": params, "timeout_ms": timeout_ms}
                    if page:
                        message.update(page_id=page.page_id, page_generation=page.page_generation)
                    data = encode(message) + b"\n"
                    if len(data) > LINE_LIMIT:
                        raise BrowserError("BROWSER_REQUEST_LIMIT")
                    future = asyncio.get_running_loop().create_future()
                    self.pending[request_id] = future
                    self.process.stdin.write(data)
                    sent = True
                    await self.process.stdin.drain()
                response = await future
            if response["ok"]:
                if not isinstance(response.get("result"), dict):
                    raise BrowserError("BROWSER_PROTOCOL_FAILED", "UNKNOWN")
                return response["result"]
            error = response.get("error") or {}
            if not isinstance(error.get("code"), str) or response.get("dispatch_state") not in {"NOT_SENT", "SENT", "UNKNOWN"}:
                raise BrowserError("BROWSER_PROTOCOL_FAILED", "UNKNOWN")
            failure = BrowserError(error["code"], response["dispatch_state"])
            if failure.dispatch_state != "NOT_SENT":
                self._fail(failure)
            raise failure
        except BrowserError:
            raise
        except (TimeoutError, asyncio.CancelledError):
            if sent:
                self._fail(BrowserError("BROWSER_REQUEST_INTERRUPTED", "UNKNOWN"))
            raise
        except Exception:
            failure = BrowserError("BROWSER_TRANSPORT_FAILED", "UNKNOWN" if sent else "NOT_SENT")
            self._fail(failure)
            raise failure from None
        finally:
            if request_id is not None:
                self.pending.pop(request_id, None)
            if future and future.done() and not future.cancelled():
                future.exception()  # Consume failures even if writing failed before awaiting it.

    async def _action(self, method, params, page=None, timeout_ms=5000):
        if self.action_lock.locked():
            raise BrowserError("BROWSER_ACTION_IN_FLIGHT")
        async with self.action_lock:
            return await self._call(method, params, page, timeout_ms)

    async def new_page(self):
        return PageRef(**await self._action("new_page", {}))

    async def navigate(self, page, url, timeout_ms=5000):
        result = await self._action("navigate", {"url": url}, page, timeout_ms)
        return PageRef(result["page_id"], result["page_generation"])

    async def read_text(self, page, selector, timeout_ms=5000):
        return (await self._action("read_text", {"selector": selector}, page, timeout_ms))["text"]

    async def read_value(self, page, selector, timeout_ms=5000):
        return (await self._action("read_value", {"selector": selector}, page, timeout_ms))["value"]

    async def read_checked(self, page, selector, timeout_ms=5000):
        return (await self._action("read_checked", {"selector": selector}, page, timeout_ms))["checked"]

    async def click(self, page, selector, navigation="none", timeout_ms=5000):
        result = await self._action("click", {"selector": selector, "navigation": navigation}, page, timeout_ms)
        return ClickResult(PageRef(result["page_id"], result["page_generation"]), result["default_prevented"])

    async def fill(self, page, selector, value, timeout_ms=5000):
        return await self._action("fill", {"selector": selector, "value": value}, page, timeout_ms)

    async def wait_text(self, page, selector, text, timeout_ms=5000, *, contains=False):
        params = {"selector": selector, "text": text}
        if contains:
            params["contains"] = True
        return (await self._action("wait", params, page, timeout_ms))["text"]

    async def capture(self, page, timeout_ms=5000):
        result = await self._action("capture", {}, page, timeout_ms)
        relative = Path(result["path"])
        path = self.workspace / relative
        if (relative.is_absolute() or len(relative.parts) != 2 or relative.parts[0] != "captures"
                or not relative.name.startswith("capture-") or relative.suffix != ".png"
                or path.resolve(strict=True) != path or not path.is_file()
                or path.stat().st_size != result["bytes"] or result["bytes"] > 2 * 1024 * 1024
                or file_hash(path) != result["sha256"]):
            self._fail(BrowserError("BROWSER_CAPTURE_INVALID", "UNKNOWN"))
            raise self.broken
        return path

    async def set_mode(self, generation, mode):
        return await self._call("set_mode", {"generation": generation, "mode": mode})

    async def begin_recheck(self, generation):
        return await self._call("begin_recheck", {"generation": generation})

    async def finish_recheck(self, generation):
        return await self._call("finish_recheck", {"generation": generation})

    async def close(self):
        if self.closing:
            return
        self.closing = True
        try:
            if self.process.returncode is None and self.broken is None:
                await self._call("close", {}, timeout_ms=1000)
        except (Exception, asyncio.CancelledError):
            pass
        finally:
            if self.process.returncode is None:
                try:
                    async with asyncio.timeout(1):
                        await self.process.wait()
                except TimeoutError:
                    self._fail(BrowserError("BROWSER_CLOSE_TIMEOUT", "UNKNOWN"))
            await self.process.wait()
            for reader in self.readers:
                if not reader.done():
                    reader.cancel()
            await asyncio.gather(*self.readers, return_exceptions=True)
            self.record.update(closed=True, exit_code=self.process.returncode)
            write_json(self.workspace / "browser-runtime.json", self.record)

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_):
        await self.close()

"""A deliberately small Playwright-style API over the isolated runtime."""
import asyncio
import inspect
import json
import re
from pathlib import Path
from .browser import BrowserError, BrowserSession, PageRef


class TimeoutError(BrowserError):
    """A condition timed out. dispatch_state records whether input was sent."""


def _timeout(value):
    if type(value) is not int or not 1 <= value <= 300000:
        raise ValueError("timeout must be 1..300000 milliseconds")
    return value


def _url_matches(pattern, url):
    if callable(pattern):
        return pattern(url)
    # Glob '*' excludes '/', while '**' crosses path boundaries.
    expr = re.escape(pattern).replace(r"\*\*", ".*").replace(r"\*", "[^/]*")
    return re.fullmatch(expr, url) is not None


class _Session(BrowserSession):
    def _fail(self, error):
        super()._fail(error)
        owner = getattr(self, "owner", None)
        if owner:
            for page in owner.pages.values():
                for waiter in tuple(page.waiters):
                    waiter._fail(error)

    protocol_version = "2"
    max_timeout_ms = 300000

    def _network_event(self, message):
        owner = getattr(self, "owner", None)
        if owner is not None:
            owner._event(message)


class Browser:
    @classmethod
    async def launch(cls, spec, workspace, persona, allowed_origins, *, initial_mode):
        self = cls()
        self.pages = {}
        persona = {"profile": "macos_chrome152", **persona}
        self.session = await _Session.start(spec, workspace, persona, allowed_origins, initial_mode)
        self.identity = self.session.ready.get("browser_identity")
        self.session.owner = self
        return self

    async def new_page(self):
        ref = await self.session.new_page()
        page = Page(self, ref)
        self.pages[ref.page_id] = page
        return page

    def _event(self, message):
        page = self.pages.get(message.get("page_id"))
        if page:
            page._event(message["event"], message["data"])

    async def close(self):
        for page in list(self.pages.values()):
            await page._dispose()
        await self.session.close()
        self.pages.clear()

    async def __aenter__(self):
        return self

    async def __aexit__(self, *_):
        await self.close()


class _Queries:
    def locator(self, selector):
        return self._query({"kind": "css", "value": selector})

    def get_by_role(self, role, *, name=None, exact=False):
        return self._query({"kind": "role", "value": role, "name": name, "exact": exact})

    def get_by_text(self, text, *, exact=False):
        return self._query({"kind": "text", "value": text, "exact": exact})

    def get_by_label(self, text, *, exact=False):
        return self._query({"kind": "label", "value": text, "exact": exact})

    def get_by_alt_text(self, text, *, exact=False):
        return self._query({"kind": "alt", "value": text, "exact": exact})


class Page(_Queries):
    def __init__(self, browser, ref):
        self.browser, self.ref = browser, ref
        self.url = "about:blank"
        self.timeout = 30000
        self.closed = False
        self.input_sequence = 0
        self.listeners = {"request": [], "response": []}
        self.waiters = set()
        self.callback_tasks = set()
        self.callback_errors = []
        self.events = asyncio.Queue(maxsize=256)
        self.dispatcher = asyncio.create_task(self._dispatch())

    def _query(self, step):
        return Locator(self, [step])

    def set_default_timeout(self, timeout):
        self.timeout = _timeout(timeout)

    async def _call(self, method, params, timeout=None):
        if self.closed:
            raise BrowserError("PAGE_CLOSED")
        timeout = _timeout(self.timeout if timeout is None else timeout)
        try:
            result = await self.browser.session._action(method, params, self.ref, timeout)
        except BrowserError as error:
            if getattr(error, "page_generation", None) is not None:
                self.ref = PageRef(self.ref.page_id, error.page_generation)
                self.url = error.url
            if error.code == "WAIT_TIMEOUT":
                raise TimeoutError(error.code, error.dispatch_state) from None
            raise
        if "page_generation" in result:
            self.ref = PageRef(self.ref.page_id, result["page_generation"])
        self.url = result.get("url", self.url)
        return result

    async def _action(self, operation, locator=(), timeout=None, **params):
        result = await self._call("automation", {"operation": operation, "locator": list(locator), **params}, timeout)
        if operation in {"click", "fill", "select", "check", "uncheck"}:
            self.input_sequence += 1
        return result

    async def goto(self, url, *, wait_until="domcontentloaded", timeout=None):
        if wait_until != "domcontentloaded":
            raise ValueError("only domcontentloaded is supported")
        await self._call("navigate", {"url": url}, timeout)

    async def wait_for_url(self, url, *, timeout=None):
        await self._action("url", value=url, timeout=timeout)

    async def screenshot(self, *, path=None, timeout=None):
        if self.closed:
            raise BrowserError("PAGE_CLOSED")
        captured = await self.browser.session.capture(self.ref, timeout_ms=_timeout(self.timeout if timeout is None else timeout))
        data = captured.read_bytes()
        if path:
            Path(path).write_bytes(data)
        return data

    def on(self, event, callback):
        if event not in self.listeners:
            raise ValueError("only request and response events are supported")
        self.listeners[event].append(callback)

    def off(self, event, callback):
        if callback in self.listeners[event]:
            self.listeners[event].remove(callback)

    def _event(self, event, data):
        if event not in self.listeners:
            raise BrowserError("BROWSER_PROTOCOL_FAILED")
        obj = Response(self, data) if event == "response" else Request(self, data)
        for waiter in tuple(self.waiters):
            waiter._accept(event, obj)
        if self.listeners[event]:
            try:
                self.events.put_nowait((obj, tuple(self.listeners[event])))
            except asyncio.QueueFull:
                raise BrowserError("NETWORK_EVENT_LIMIT") from None

    async def _dispatch(self):
        while True:
            obj, callbacks = await self.events.get()
            for callback in callbacks:
                if len(self.callback_tasks) >= 256:
                    self.browser.session._fail(BrowserError("NETWORK_CALLBACK_LIMIT"))
                    return
                task = asyncio.create_task(self._callback(callback, obj))
                self.callback_tasks.add(task)
                task.add_done_callback(self.callback_tasks.discard)
            self.events.task_done()

    async def _callback(self, callback, obj):
        try:
            value = callback(obj)
            if inspect.isawaitable(value):
                await value
        except Exception as error:
            self.callback_errors.append(error)
            del self.callback_errors[:-64]

    def expect_response(self, pattern, *, timeout=None):
        return _EventWait(self, "response", pattern, timeout)

    def expect_request(self, pattern, *, timeout=None):
        return _EventWait(self, "request", pattern, timeout)

    async def _dispose(self):
        self.closed = True
        for waiter in tuple(self.waiters):
            waiter._fail(BrowserError("PAGE_CLOSED"))
        self.listeners = {"request": [], "response": []}
        self.dispatcher.cancel()
        for task in tuple(self.callback_tasks):
            task.cancel()
        await asyncio.gather(self.dispatcher, *self.callback_tasks, return_exceptions=True)

    async def close(self):
        if self.closed:
            return
        await self._action("close")
        await self._dispose()
        self.browser.pages.pop(self.ref.page_id, None)


class Locator(_Queries):
    def __init__(self, page, steps):
        self.page, self.steps = page, steps

    def _query(self, step):
        return Locator(self.page, self.steps + [step])

    def nth(self, index):
        return self._query({"kind": "nth", "index": index})

    @property
    def first(self):
        return self.nth(0)

    @property
    def last(self):
        return self.nth(-1)

    async def _do(self, op, timeout=None, **params):
        return await self.page._action(op, self.steps, timeout, **params)

    async def click(self, *, timeout=None):
        await self._do("click", timeout)

    async def fill(self, value, *, timeout=None):
        await self._do("fill", timeout, value=value)

    async def select_option(self, value, *, timeout=None):
        await self._do("select", timeout, value=value)
        return [value]

    async def check(self, *, timeout=None):
        await self._do("check", timeout)

    async def uncheck(self, *, timeout=None):
        await self._do("uncheck", timeout)

    async def scroll_into_view_if_needed(self, *, timeout=None):
        await self._do("scroll", timeout)

    async def bounding_box(self, *, timeout=None):
        return (await self._do("box", timeout))["value"]

    async def count(self):
        return (await self._do("count"))["value"]

    async def all(self):
        return [self.nth(index) for index in range(await self.count())]

    async def text_content(self, *, timeout=None):
        return (await self._do("text", timeout))["value"]

    async def get_attribute(self, name, *, timeout=None):
        return (await self._do("attribute", timeout, value=name))["value"]

    async def input_value(self, *, timeout=None):
        return (await self._do("value", timeout))["value"]

    async def is_visible(self):
        return (await self._do("visible_now"))["value"]

    async def wait_for(self, *, state="visible", timeout=None):
        if state not in {"visible", "hidden", "attached", "detached"}:
            raise ValueError("unsupported state")
        await self._do(state, timeout)


class _Expect:
    def __init__(self, locator):
        self.locator = locator

    async def to_be_visible(self, *, timeout=None):
        await self.locator.wait_for(state="visible", timeout=timeout)

    async def to_be_hidden(self, *, timeout=None):
        await self.locator.wait_for(state="hidden", timeout=timeout)

    async def to_have_text(self, value, *, timeout=None):
        await self.locator._do("text_is", timeout, value=value)

    async def to_contain_text(self, value, *, timeout=None):
        await self.locator._do("text_contains", timeout, value=value)

    async def to_have_value(self, value, *, timeout=None):
        await self.locator._do("value_is", timeout, value=value)

    async def to_have_count(self, value, *, timeout=None):
        await self.locator._do("count_is", timeout, count=value)


def expect(locator):
    return _Expect(locator)


class _Body:
    async def body(self):
        if self.page.closed:
            raise BrowserError("PAGE_CLOSED")
        data = bytearray()
        while True:
            # A synchronous V8 task can delay the evidence reader. Its own action
            # deadline/watchdog remains authoritative; a shorter body timeout must
            # not terminate that otherwise valid action.
            session = self.page.browser.session
            chunk = await session._call("network_body", {
                "body_id": self.data["body_id"], "offset": len(data)
            }, self.page.ref, timeout_ms=session.max_timeout_ms)
            data.extend(chunk["bytes"])
            if chunk["eof"]:
                return bytes(data)

    async def text(self):
        return (await self.body()).decode("utf-8", errors="replace")

    async def json(self):
        return json.loads(await self.body())


class Request(_Body):
    def __init__(self, page, data):
        self.page, self.data = page, data
        self.url, self.method, self.headers = data["url"], data["method"], data["headers"]

    async def post_data(self):
        return await self.text()


class Response(_Body):
    def __init__(self, page, data):
        self.page, self.data = page, data
        self.url, self.status, self.headers = data["url"], data["status"], data["headers"]
        self.request = Request(page, data["request"])
        self.redirected_from = data.get("redirected_from", [])

    @property
    def ok(self):
        return 200 <= self.status < 300


class _EventWait:
    def __init__(self, page, event, pattern, timeout):
        self.page, self.event, self.pattern = page, event, pattern
        self.timeout = _timeout(page.timeout if timeout is None else timeout)
        self.future = asyncio.get_running_loop().create_future()
        self.sequence = page.input_sequence
        self.timer = None

    @property
    def value(self):
        return self.future

    def _fail(self, error):
        if not self.future.done():
            self.future.set_exception(error)

    def _accept(self, event, obj):
        if event != self.event or self.future.done():
            return
        try:
            accepted = self.pattern(obj) if callable(self.pattern) else _url_matches(self.pattern, obj.url)
            if inspect.isawaitable(accepted):
                raise TypeError("event predicates must be synchronous")
            if accepted:
                self.future.set_result(obj)
        except Exception as error:
            self._fail(error)

    def _expired(self):
        state = "UNKNOWN" if self.page.browser.session.action_lock.locked() else (
            "SENT" if self.page.input_sequence != self.sequence else "NOT_SENT")
        self._fail(TimeoutError("RESPONSE_TIMEOUT", state))

    async def __aenter__(self):
        if self.page.closed:
            raise BrowserError("PAGE_CLOSED")
        self.page.waiters.add(self)
        self.timer = asyncio.get_running_loop().call_later(
            self.timeout / 1000, self._expired)
        return self

    async def __aexit__(self, exc_type, *_):
        try:
            if exc_type is None:
                await self.future
            elif not self.future.done():
                self.future.cancel()
        finally:
            self.timer.cancel()
            self.page.waiters.discard(self)
            if self.future.done() and not self.future.cancelled():
                self.future.exception()

# Obscura Python SDK

`obscura-runtime` supplies a small asynchronous, Playwright-style API over the
isolated Rust runtime. It does not install Playwright, start CDP, or depend on
Autopilot. Python 3.12+ and `psutil>=7,<8` are required. Build the runtime and wheel
separately; the wheel does not contain the executable.

```bash
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release)
(cd bindings/python && uv build --wheel --out-dir /absolute/wheelhouse)
```

## Start a browser

```python
from obscura_runtime import Browser, expect

# spec contains an absolute binary path and its expected SHA-256.
# persona and allowed_origins follow the isolated runtime contract.
async with await Browser.launch(
    spec, workspace, persona, allowed_origins, initial_mode="RUNNING"
) as browser:
    page = await browser.new_page()
    await page.goto("https://example.com")
    await page.get_by_label("Email", exact=True).fill("test@example.com")
    async with page.expect_response("**/search") as pending:
        await page.get_by_role("button", name="Search", exact=True).click()
    data = await (await pending.value).json()
    await expect(page.get_by_role("heading", name="Results")).to_be_visible()
```

`initial_mode` is required. Starting PAUSED does not authorize page actions.
The existing `BrowserSession`, `PageRef`, `ClickResult`, and `file_hash` remain
available with protocol 1 behavior. The high-level `Browser` uses protocol 2;
it requires the matching new runtime and rejects incompatible startup replies.

## Supported interfaces

| Object | Interfaces |
| --- | --- |
| Browser | `launch`, `new_page`, `close`, async context manager |
| Page | `goto`, `wait_for_url` (exact URL), `screenshot`, `close`, `set_default_timeout` |
| Page and Locator | `locator` (CSS), `get_by_role`, `get_by_text`, `get_by_label`, `get_by_alt_text` |
| Locator | chaining, `first`, `last`, `nth`, `click`, `fill`, `select_option` (single value), `check`, `uncheck`, `scroll_into_view_if_needed`, `bounding_box` |
| Locator observations | `count`, `all`, `text_content`, `get_attribute`, `input_value`, `is_visible`, `wait_for` |
| expect(locator) | `to_be_visible`, `to_be_hidden`, `to_have_text`, `to_contain_text`, `to_have_value`, `to_have_count` |
| Page network | `on`, `off`, `expect_request`, `expect_response` |
| Request | `url`, `method`, `headers`, async `body`, `post_data`, `text`, `json` |
| Response | `url`, `status`, `ok`, `headers`, `request`, async `body`, `text`, `json` |

Role lookup implements the common native roles used by forms, explicit `role`,
ARIA names, associated labels, and text names. This is a supported subset, not a
claim of full accessibility-name or Playwright selector compatibility. Arbitrary
XPath, evaluation, force-click, Inspector, frames, and BrowserContext are not
part of this API. CSS selectors are handled by Obscura's native selector engine;
use `get_by_text` instead of Playwright's `:has-text` extension.

## Waiting and input

Page actions are serial. One action submits one RPC; Rust re-resolves the locator
and advances the page while waiting. Clicks scroll into view, wait for stable
geometry over successive frame opportunities, and use native hit testing and
pointer/mouse input. They do not call the page's `element.click()` or dispatch
synthetic clicks. Filling and selecting use the engine's native input bridge.

The default timeout is 30,000 ms; a per-operation timeout or page default may be
1..300,000 ms. `goto` supports `domcontentloaded`. Readiness after navigation is
expressed with locators, URLs, or expected network events, not network-idle guesses.
`all`, `count`, `is_visible`, and `bounding_box` observe the current state.
Text/value reads wait for a unique element; assertions wait for the expected value.
Single-element operations reject ambiguity; use `nth/first/last` explicitly.

`TimeoutError` denotes an unmet condition. Its `dispatch_state` distinguishes
`NOT_SENT` from `SENT`. Ordinary waiting may be retried by the application; input
already dispatched is never automatically repeated. Transport interruption,
engine watchdog expiration, or unknown execution state terminates the owned
process. Cancellation of an in-flight action also terminates the process.

Bounding boxes, supported geometry, and actionability follow the engine's native
rendering capabilities. Unsupported transformations/input targets return errors;
they are not bypassed by JavaScript input.

## Network evidence

Listeners are installed on each protocol-2 page before navigation. Events cover
the engine's page-scoped navigation, fetch/XHR and resource callback paths.
Redirect chains follow the underlying observer contract: they expose the final
response and do not promise a separate event for every intermediate redirect.
Headers are the engine observer's header map, not a wire-level duplicate-header
capture. Request bodies are bytes, and JSON decoding waits for a complete body.
No cookies, proxy policy, origin validation, or request contents are changed.

`expect_response` registers before its context body runs. It accepts a URL glob
(`*` excludes slashes, `**` crosses them) or a synchronous predicate receiving the
Response. `expect_request` has the same contract. The timeout covers the whole
context, including its triggering action. Async event callbacks run outside the
RPC reader; they can read bodies while a page action is waiting. Callback errors
are retained in `page.callback_errors` (last 64). Callbacks should avoid issuing
competing page actions.

Bodies are retained per page: at most 8 MiB per body, 32 MiB total and 256 stored
body entries. Reads use bounded chunks, each with a 300-second RPC ceiling so a legal
synchronous page task cannot trigger the legacy 5-second read timeout. Page action
deadlines and the V8 watchdog remain independent. Eviction returns `BODY_RELEASED` and
oversized bodies return `BODY_LIMIT`; neither silently truncates content. Close
unregisters observers and releases evidence. Listener/callback queues are bounded.

## Tests

```bash
PYTHONPATH=bindings/python/src OBSCURA_RUNTIME_BIN=/absolute/runtime \
  python -m unittest discover -s bindings/python/tests -v

# Optional Chromium comparison; Playwright is a test-only dependency.
PYTHONPATH=bindings/python/src OBSCURA_RUNTIME_BIN=/absolute/runtime \
  CHROMIUM_BIN=/absolute/chromium python bindings/python/tests/compare_playwright.py
```

Tests without `OBSCURA_RUNTIME_BIN` skip; a skipped run is not runtime validation.
The ZG example is documented in [examples/zg](../../examples/zg/README.md).

## SDK browser identity

Protocol 2 accepts `macos_chrome152` and the legacy `windows_chrome145` persona
profiles. `Browser.launch` defaults an omitted persona `profile` to
`macos_chrome152`; an explicit profile is preserved. Protocol 1 retains its
Windows Chrome 145 restriction. The ZG example selects the macOS profile.

The macOS profile matches the captured headed Chrome 152.0.7977.83 identity:
reduced UA `Chrome/152.0.0.0`, `MacIntel`, Client Hints `macOS`, platform version
`26.6.2`, architecture `arm`, full version `152.0.7977.83`, and English language
preferences. These are a pinned reference identity, not host auto-detection.
Navigation, subresources and fetch/XHR share the same stealth client. Frame
realms inherit the identity. This does not claim all browser APIs, graphics,
fonts, hardware values or automation flags are identical to real Chrome.

The macOS profile uses primp 2.0.1's Chrome 152/macOS TLS and HTTP/2
preset. Windows Chrome 145 uses primp ChromeV145/Windows. Obscura owns
redirect validation, cookies, request headers and body limits for both paths.
The primp prefetch header is removed; explicit proxies and custom DNS validation
remain authoritative. The vendored client has a narrow custom-resolver patch.

**Transport limitation:** primp's ALPS payload and requested trust-anchor list
still differ from the captured Chrome binary. Matching a version label does not
make every wire or browser surface identical. The outer retry policy is disabled;
primp's H2 implementation can still replay a REFUSED_STREAM request, which the
protocol defines as unprocessed. This does not replay a page input action.
See [the source comparison](../../docs/Primp-and-wreq-comparison.md) for details. No hostname-specific behavior is introduced.

`browser.identity` exposes the negotiated identity and actual transport preset;
`chrome152_transport_verified` is currently false. Do not treat the persona name
as a claim that the transport is identical to the Chrome 152 binary.

Large DOM text/value/attribute results that cannot fit the bounded RPC response
raise `BrowserError("VALUE_LIMIT")` without truncation; the session remains
usable. Narrow the locator. Text assertions compare inside the runtime and
return only success. Network bodies use the separate chunked read path.

Both stealth profiles use primp; wreq and BoringSSL are no longer dependencies.
Obscura does not replay GET or POST after a connection reset. This does not
disable protocol-level recovery inside primp (for example HTTP/2 REFUSED_STREAM).

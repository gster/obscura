# Unblocked qualification tools

This directory holds development-only fixtures and evidence for the CDP-first
migration. Its Python dependencies are isolated from every Rust production
package and binary.

## Frozen inputs

- `baseline.json` records the source, lockfiles, Rust toolchains, benchmark
  revision, platform environment, exact commands, and explicit
  passed/failed/skipped/not-run results for OB-001.
  Its lockfile digests are resolved from the exact `source.revision` Git
  commit, so later working-tree changes do not rewrite historical evidence;
  an unavailable revision or source lock blob is an error. The removed private
  runtime's toolchain manifest is also checked from that historical commit.
  The root toolchain manifest, CI pins, and release pins remain current
  qualification inputs checked from the working tree independently.
- `client-scope.json` records the official Playwright Python version, bundled
  driver and reference Chromium revision, connection boundary, and the
  required/deferred/unsupported API inventory for OB-025.
- `pyproject.toml` and `uv.lock` pin the unmodified official client and all of
  its Python dependencies. They are not part of an Obscura build or release.
- `fixtures/cdp-minimal.html`, `cdp_fixture.py`, and `cdp_trace.py` run the same
  explicit CDP probe through normal Chromium launch, Chromium CDP, and Obscura
  CDP. `cdp-evidence.json` records the current macOS result without committing
  generated raw traces.
- `automation-cdp-profile.json` publishes the method-level OB-027 status,
  implementation state, verification state, parameter/result/event/scope/error
  boundary, and evidence. `automation_smoke.py` is the required unmodified
  Playwright Python end-to-end connection smoke.

Validate the committed records with the system Python:

```bash
python3 tools/unblocked/validate.py baseline tools/unblocked/baseline.json
python3 tools/unblocked/validate.py client tools/unblocked/client-scope.json
python3 tools/unblocked/validate.py profile tools/unblocked/automation-cdp-profile.json
python3 -m unittest discover -s tools/unblocked/tests -v
```

Create the isolated client environment without resolving newer dependencies:

```bash
uv sync --project tools/unblocked --frozen --python 3.12
```

Playwright starts no Obscura process. A host starts `obscura serve`, then the
official client uses `BrowserType.connect_over_cdp`. `BrowserType.connect`
uses Playwright's own protocol and is outside the product boundary.
`APIRequestContext` and `Route.fetch` execute in the Playwright driver, so they
are not certified for protected traffic that must use Obscura's primp path.

The manifests report missing evidence rather than treating it as success. The
initial record still has Linux results marked not-run, so this foundation does
not close OB-001 or release gates.

## Minimal CDP trace

Install the Chromium revision bundled with the locked client, then write all
generated traces outside the repository:

```bash
uv run --project tools/unblocked --frozen --python 3.12 playwright install chromium
RUN_ROOT="$(mktemp -d)"
uv run --project tools/unblocked --frozen --python 3.12 \
  python tools/unblocked/cdp_fixture.py \
  --output "$RUN_ROOT" \
  --obscura-bin target/release/obscura
```

The runner records the explicit fixture commands, selected events, full network
headers and document body, responses, logical browser context/session names,
errors, and relative timing. Raw traces preserve all collected values. The
separate comparison view normalizes only protocol IDs, temporary fixture
origins, protocol timestamps, and Playwright world suffixes; business headers,
request bodies, query values, and nested result data remain exact. Raw traces
stay outside the repository and are managed by the test runner after use.
Command/response pairs use a recorder call ID. Events retain their CDP
frame/loader/request identifiers plus arrival order and the call active when
they arrived; the recorder does not invent causal links from timing alone.
Playwright's private driver traffic outside the explicit CDP session is not
claimed as part of this trace.

For externally launched Chromium CDP and Obscura CDP processes, `processCapture`
records the command, PID, exit status, and paths to separate `stdout.bin` and
`stderr.bin` files. These files retain emitted bytes without decoding, redaction,
truncation, or an in-memory pipe budget. Spawn and endpoint-readiness failures
retain their capture metadata too. With `--output`, per-process directories stay
under the output directory (the smoke uses its JSON file's parent); direct Python
calls without an output root use retained OS temporary directories. The caller
owns cleanup after inspecting the evidence. The Playwright-managed normal launch
mode does not use this external-process capture path.

CI uploads the complete smoke directory on success or failure, including the raw
Playwright protocol log, JSON result, and browser stdout/stderr files, with a
seven-day artifact retention period. Local runs keep files until the operator
removes them.

## Required automation smoke

Build the render binary and run the pinned official client without installing
Chromium; the client connects to the host-started Obscura CDP endpoint:

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo build --release -p obscura-cli --bins --features render
RUN_ROOT="$(mktemp -d)"
DEBUG=pw:protocol OBSCURA_PERSONA=windows_chrome145 \
  uv run --project tools/unblocked --frozen --python 3.12 \
  python tools/unblocked/automation_smoke.py \
    --obscura-bin target/release/obscura \
    --output "$RUN_ROOT/smoke.json" \
    2> "$RUN_ROOT/playwright-protocol.log"
python3 tools/unblocked/validate.py protocol-log \
  tools/unblocked/automation-cdp-profile.json \
  "$RUN_ROOT/playwright-protocol.log"
```

The smoke navigates the fixed local fixture, fills a labelled input, selects
labelled single and multiple options, and clicks a role locator through the
official client. It fixes the first page viewport at 320 by 240, captures an
official `Page.screenshot()` PNG, decodes its pixels to prove the image is
nonblank, and retains the complete PNG as base64 plus its hashes and metadata.
It creates a second browser context with two pages, checks same-origin
localStorage and global-object isolation, verifies the default viewport/screen
metrics, then closes one page. The surviving page must retain its closure,
settled timer promise, remote handle and document while the closed page rejects
evaluation. A fixture response remains server-held until after the close
boundary; the runner records every console event with its phase, releases the
response, and requires that no event arrive from the closed page. The smoke then closes that
context and confirms that the default page remains live. It also reads the DOM
through CDP and proves that an
unknown method and invalid initializer parameters fail. Its optional JSON
output retains all complete document responses, the full screenshot, locator
and context return values and event records, the complete DOM response, and
every explicit CDP
command, response, and error it collects. `DEBUG=pw:protocol` emits the
official client's complete raw driver protocol stream without normalization or
field deletion. The `protocol-log` validator reads that untouched file and
fails if the smoke's required inventory is truncated or any observed method is
absent from the profile.

## SDK removal behavior gates

Run the remaining shared-behavior migration checks through the same pinned
official client and a render binary:

```bash
OBSCURA_PERSONA=windows_chrome145 \
  uv run --project tools/unblocked --frozen --python 3.12 \
  python tools/unblocked/migration_smoke.py \
    --obscura-bin target/release/obscura \
    --output "$RUN_ROOT/migration.json"
```

Each case owns a separate browser and client worker. Cancelling a Python
`asyncio.Task` cancels the local wait, not the remote action: the cancellation
case makes that locator actionable afterwards and records its single eventual
side effect without retrying it. Separate probes check an official action
timeout and explicit page close before dispatch. A completed click must produce
exactly one server effect even when a subsequent response wait times out.
The watchdog case sets
`OBSCURA_CDP_COMMAND_TIMEOUT_MS=2000` only for its browser, enters a synchronous
infinite click handler through official `page.mouse.click`, requires the native
`INPUT_DISPATCH_FAILED` error and exactly one handler entry, then requires
JavaScript evaluation and a finite locator click to work on the original page
and connection. Playwright 1.60 locator clicks retry this protocol error; using
`locator.click(timeout=0)` would repeatedly enter the handler until the worker
is killed. This gate proves one-shot input termination and recovery, not that
locator actions never replay after a protocol failure. This proves recovery with the
configured two-second budget, not the default timeout. The script case checks
cross-origin classic scripts with default, anonymous, and credentialed modes,
including denied CORS and document base URL resolution.

The parent enforces a 45-second worker deadline. A forced termination is always
a failure and cannot count as watchdog recovery. Browser and worker stdout and
stderr remain complete binary files, including the worker's `pw:protocol`
stream. HTTP fixture records retain request headers and bodies, and each case
records its assertions and errors in JSON. CI uploads the complete directory
on success or failure and validates the combined regular-smoke and migration
protocol inventory against the declared CDP profile.

## Trace comparison

Each successful mode writes one trace and `result.json` reports the first exact
normalized divergence. Exit status is zero only when every selected mode runs
and all available comparisons match; a collected but different Obscura trace
therefore exits nonzero while retaining the evidence. Compare any two saved
traces directly with:

```bash
python3 tools/unblocked/cdp_trace.py left.json right.json
```

## Native mouse qualification

`native_mouse_smoke.py` drives the official Playwright mouse API through CDP.
It uses six isolated offline pages: ordinary input, page overrides of public
hit-test/event APIs, canceled mousedown, canceled checkbox click, canceled
pointerdown, and triple-click text selection. It checks event order, trusted
constructors, coordinates, button metadata, focus, cancellation and effects.

```bash
RUN_ROOT="$(mktemp -d)"
uv run --project tools/unblocked --frozen --python 3.12 \
  python tools/unblocked/native_mouse_smoke.py \
  --obscura-bin target/release/obscura \
  --persona windows_chrome145 \
  --with-chrome \
  --output "$RUN_ROOT/native-mouse.json"
```

`--with-chrome` compares both engines against the Chromium bundled with the
locked client. `--chrome-only` runs the reference alone. Without either flag,
the runner checks Obscura against the recorded fixture contract. Each browser
has a separate process and each worker has a 45-second deadline. Browser and
worker stdout/stderr, including the complete Playwright protocol stream, stay
in the output directory alongside fixture request/response wire bytes and HTML.
Workers checkpoint completed cases so a later hard deadline preserves earlier
observations. Failed comparisons retain the observations and failure details;
missing observations and forced termination fail the gate.
CI runs the Obscura fixture and uploads the complete evidence directory.

This gate covers the listed mouse fixtures. Wheel, keyboard/text lifecycle,
pen input, pointer capture, hover boundary events, multi-button gestures and
user activation require their own qualification. Coordinate mouse input needs
a render build; a no-render build reports that capability as unsupported.


## Native wheel qualification

`native_wheel_smoke.py` uses the official Playwright mouse API against nine
isolated local fixture cases: metadata, horizontal and vertical nested
saturation/handoff, active/passive/default-root cancellation, public API
poisoning, zero deltas, and a raw CDP request missing both deltas. Both engines
use a 1280 by 720 viewport and wait for stable offsets. The default run checks
the frozen Chrome behavior contract; `--with-chrome` also compares observations
with a fresh Chromium run.

```bash
RUN_ROOT="$(mktemp -d)"
uv run --project tools/unblocked --frozen --python 3.12 \
  python tools/unblocked/native_wheel_smoke.py \
    --obscura-bin target/release/obscura \
    --persona windows_chrome145 \
    --with-chrome \
    --output "$RUN_ROOT/wheel"
```

Each run retains a separate raw-data directory, browser and worker byte streams,
complete `pw:protocol`, fixture request/response bytes and per-case checkpoints.
`wheel-result.json` records the chosen directory and all outcomes. An unfinished
settle, missing observation, unexpected protocol error or failed worker fails
the gate. CI runs all nine cases and uploads the complete evidence directory.
Event timestamps and scroll-event counts are retained but are not exact paired
assertions. This gate does not qualify gesture latching, inertia, scroll snap,
zoom, complete CSSOM overflow strings or a full wheel implementation on all
platforms. Native coordinate input requires the render build.

## Native keyboard and text qualification

`native_keyboard_smoke.py` uses Playwright's raw CDP session against seven
isolated local cases. It checks `Input.insertText` selection replacement and
empty deletion, keyDown/rawKeyDown/char/keyUp phase order, cancellation,
keyboard metadata, browser-owned dispatch despite poisoned public APIs,
focus/document reentry, and malformed protocol parameters.

```bash
RUN_ROOT="$(mktemp -d)"
uv run --project tools/unblocked --frozen --python 3.12 \
  python tools/unblocked/native_keyboard_smoke.py \
    --obscura-bin target/release/obscura \
    --persona windows_chrome145 \
    --with-chrome \
    --output "$RUN_ROOT/native-keyboard.json"
```

`--with-chrome` requires every bounded observation to match the Chromium
bundled with the locked client. `--chrome-only` validates the reference
contract, while the default CI mode checks Obscura against the embedded stable
contract. `--reference-json` accepts either a worker result or a complete prior
runner result. Every browser runs in a separate process under a 45-second
worker deadline. Complete browser/worker byte streams, `pw:protocol`, fixture
request/response bytes, HTML, checkpoints and failed comparisons remain in the
result directory without field removal.

This gate qualifies ordinary input/textarea selection edits, readonly and
non-editable beforeinput behavior, the listed event phases and metadata, and
the measured focus/document reentry. It does not qualify contenteditable,
IME/composition, grapheme or word editing, arbitrary editor commands,
platform-shortcut defaults, complex implicit form submission, button/checkbox
activation, or maxlength truncation. Keyboard and text native input currently
requires the render build; no-render reports it as unsupported.

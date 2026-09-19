# Unblocked qualification tools

This directory holds development-only fixtures and evidence for the CDP-first
migration. Its Python dependencies are isolated from every Rust production
package and binary.

## Frozen inputs

- `baseline.json` records the source, lockfiles, Rust toolchains, benchmark
  revision, platform environment, exact commands, and explicit
  passed/failed/skipped/not-run results for OB-001.
- `client-scope.json` records the official Playwright Python version, bundled
  driver and reference Chromium revision, connection boundary, and the
  required/deferred/unsupported API inventory for OB-025.
- `pyproject.toml` and `uv.lock` pin the unmodified official client and all of
  its Python dependencies. They are not part of an Obscura build or release.
- `fixtures/cdp-minimal.html`, `cdp_fixture.py`, and `cdp_trace.py` run the same
  explicit CDP probe through normal Chromium launch, Chromium CDP, and Obscura
  CDP. `cdp-evidence.json` records the current macOS result without committing
  generated raw traces.

Validate the committed records with the system Python:

```bash
python3 tools/unblocked/validate.py baseline tools/unblocked/baseline.json
python3 tools/unblocked/validate.py client tools/unblocked/client-scope.json
python3 -m unittest tools.unblocked.tests.test_validate -v
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

Each successful mode writes one trace and `result.json` reports the first exact
normalized divergence. Exit status is zero only when every selected mode runs
and all available comparisons match; a collected but different Obscura trace
therefore exits nonzero while retaining the evidence. Compare any two saved
traces directly with:

```bash
python3 tools/unblocked/cdp_trace.py left.json right.json
```

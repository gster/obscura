## Test suites

### Rust unit and integration

```bash
cargo nextest run --release --features render --no-fail-fast
```

Crate-scoped:

```bash
cargo nextest run --release --features render -p obscura-cdp
cargo nextest run --release --features render -p obscura-browser
```

By name:

```bash
cargo nextest run --release --features render runtime_click_submit_prevent_default
```

Use `cargo nextest`, not `cargo test`. Runtime tests require process isolation
under the project's V8 isolation requirements; production pages can own separate isolates. Render tests must run in
release mode; debug builds are not a fidelity or performance gate.

### CDP parity tests

`crates/obscura-cdp/tests/cdp_*.rs` exercise CDP methods end-to-end with a real `dispatch` call and an in-process HTTP server.

Pattern:

```rust
#[tokio::test(flavor = "current_thread")]
async fn my_test() {
    std::env::set_var("OBSCURA_ALLOW_PRIVATE_NETWORK", "1");
    let url = serve_once().await;
    let mut ctx = CdpContext::new();
    let page_id = ctx.create_page();
    let session_id = "session-1";
    ctx.sessions.insert(session_id.to_string(), page_id.clone());

    cdp(&mut ctx, 1, "Page.navigate", json!({"url": url}), session_id).await;
    // assertions
}
```

`serve_once` and `cdp` helpers are copied across the parity tests; reuse them.

## Logging

```bash
RUST_LOG=obscura=info  obscura serve
RUST_LOG=obscura=debug obscura serve
RUST_LOG=obscura_cdp=trace,obscura_browser=debug obscura serve
```

Logs go to stderr.

`--verbose` on any subcommand is equivalent to `RUST_LOG=obscura=info`.

## Driving the CDP server manually

```bash
obscura serve --port 9222 --verbose
```

In another shell:

```bash
wscat -c ws://127.0.0.1:9222
> {"id":1,"method":"Target.createTarget","params":{"url":"about:blank"}}
> {"id":2,"method":"Target.attachToTarget","params":{"targetId":"...","flatten":true}}
> {"id":3,"sessionId":"<sessionId returned by attachToTarget>","method":"Page.navigate","params":{"url":"https://example.com"}}
> {"id":4,"sessionId":"<sessionId returned by attachToTarget>","method":"Runtime.evaluate","params":{"expression":"document.title"}}
```

Useful for reproducing what Puppeteer or Playwright is doing without their abstraction.

## Common failure modes

### Target or context failures

Inspect owning connection/session, target lifetime and execution-context events. Context IDs currently validate routing but do not establish true utility-world isolation. Do not assume every timeout is a global V8-lock bug.

### V8 isolate failures

Check owner-thread access, isolate entry/exit, creation serialization and suspension across awaits. Pages and Workers have separate isolates; do not debug from the obsolete single-isolate model.

### Test hangs

A handler is awaiting something that never resolves. Run with `RUST_LOG=obscura=trace` and check the last log line before the hang.

## Reproducing user bug reports

Use existing crate integration fixtures or an external disposable repro. There is no root `tests/test_all.py` in this checkout. A client repro must record its exact client/driver and engine build; direct dispatch coverage alone is not a full WebSocket/client test.

The reduced airline protection collector is documented in
[Protection script regression case](Protection-script-regression-case.md). Use
it when changing Worker message plumbing, persona defaults, WebGL, SVG, or
Chrome identity surfaces.

### Rendering regressions

Start with the committed deterministic fixtures, then use the representative
real-site suite at both the top and bottom of pages:

```bash
RUN_ROOT="$(mktemp -d)"
OBSCURA_BIN=./target/release/obscura render-repros/run.sh "$RUN_ROOT/fixtures"
OBSCURA_BIN=./target/release/obscura render-repros/representative-suite/run.sh "$RUN_ROOT/top"
OBSCURA_BIN=./target/release/obscura render-repros/representative-suite/run.sh "$RUN_ROOT/bottom" bottom
```

Set `BASELINE_BIN` or `CHROMIUM_BIN` when producing paired captures. Keep the
viewport, user agent, settle policy, scroll position, animation sample, and
capture boundary identical. A pixel-distance score is a regression tripwire,
not a verdict: verify both engines succeeded and produced nonblank output,
then inspect missing resources, geometry, structural edges, and a reduced
fixture. Do not add hostname-specific render branches.

## Profiling

CPU with `perf` and a flamegraph:

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render
perf record -F 99 -g -- ./target/release/obscura fetch https://heavy-spa.example
perf script | flamegraph.pl > flame.svg
```

Memory with heaptrack:

```bash
heaptrack ./target/release/obscura serve
```

## Independent runtime and obstacle course

```bash
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --locked --release --no-fail-fast)
```

The companion `obscura-benchmark` repository is separate. Check out a fixed revision, then execute from that repository with an absolute candidate path:

```bash
OBSCURA_BIN=/absolute/obscura/target/release/obscura python3 obstacle-course/run.py --runs 1 --warmup 0
```

The current CI pin is `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` in the `gster/obscura-benchmark` fork. That revision repairs the observer fixture so its first real intersection drains the bounded initial page; reference Chrome and Obscura were both re-run, and the full Obscura course reached 33/33. Keep failed/skipped/not-run separate and do not replace a failed fixture's expected value to obtain 33/33. WPT results use subtest denominators. Current audit results are in [SUMMARY](SUMMARY.md).

Profiling requires tooling that actually exists in the selected build; `tokio-console` needs application instrumentation as well as dependency features. No instrumented console subscriber is asserted by this guide.

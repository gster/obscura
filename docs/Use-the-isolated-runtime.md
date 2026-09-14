# Use the isolated runtime

Obscura owns the browser engine (`crates/`), the isolated Rust executable
(`runtime/`), the Python client (`bindings/python/`), and the embedded font
manifest (`persona-fonts.json`). The executable is named
`autopilot-browser-runtime` for compatibility. It does not contain airline
workflows, Dispatcher scheduling, or Agent credentials.

Applications use `obscura_runtime.BrowserSession` to start a process over bounded
stdin/stdout NDJSON RPC. This path embeds the engine directly; it does not start
a CDP server. Use the [CDP guide](Connect-Puppeteer-or-Playwright.md) for
Puppeteer or Playwright instead.

## Repository remotes

This integration is maintained in [gster/obscura](https://github.com/gster/obscura).
`origin` uses `git@github.com:gster/obscura.git`; `upstream` retains
`https://github.com/h4ckf0r0day/obscura.git`. Track `origin/main` for this fork.
Upstream release downloads do not establish the consumer's pinned runtime build.

## Build and install

From the Obscura repository root:

```bash
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release)
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --locked --release)
(cd bindings/python && uv build --wheel --out-dir /absolute/wheelhouse)
```

Run Cargo inside `runtime/` so rustup selects its pinned toolchain. This is a
separate Cargo workspace with its own lockfile; root CLI builds do not build it.
It enables both render and stealth. See the [runtime build notes](../runtime/README.md)
for cross-compilation and snapshot requirements. `CARGO_TARGET_DIR`, when set,
overrides the default `runtime/target/` output directory.

Install the wheel in the application's Python 3.12+ environment. Its metadata
requires `psutil>=7,<8`; process inspection is imported only when starting a
browser. The wheel does not embed the Rust executable or depend on Autopilot.

The application supplies the binary's absolute path and full SHA-256, persona,
allowed origins, and a private attempt workspace. The client checks the binary
hash before spawn. Startup defaults to PAUSED; application control must authorize
actions and retain responsibility for cancellation and process cleanup.

## Autopilot consumer

MoneyMachine keeps a sibling Obscura checkout and pins its exact HEAD in
`autopilot/obscura-revision`. `autopilot/verify_obscura.py` requires that commit
and a clean checkout. A branch name or a newer main commit is not sufficient.
The consumer's `docs/browser-runtime.md` describes its install, executor identity,
Docker mounts, integration tests, and offline release procedure.

When changing this interface, update those consumer documents and verify the
real runtime tests before advancing the pin. Publish or transfer the exact
Obscura commit to every build host; a local merge does not publish it remotely.
Build and hash the SDK wheel and Rust binary separately. A source merge and
passing local tests do not establish deployment or airline-task success.

## Commit lineage

The runtime integration starts at `84971e7` (native browser support and fonts),
followed by `0c94f15` (isolated Rust workspace and standalone Python client),
and `98f6795` (lazy process-inspection import). Their common ancestor with the
September 14 main update is `727cc46`; the main side reaches `89964b3`.
The merge retains both lines of history. The older local preservation branch
`codex/local-runtime-merge` is a separate historical snapshot, not the current
consumer revision.

## Integration validation (2026-09-14)

The render CLI and isolated runtime release builds passed.
The isolated runtime passed 178 release tests. Autopilot passed 233 tests with
the real runtime configured, with no skips. The standalone Python wheel also
completed navigation, DOM access, and process cleanup without Autopilot installed.

The root render suite ran 1,708 tests: 1,706 passed, two failed, and four other
tests were skipped. One failure is the host DNS mapping `example.com` into
`198.18.0.0/15`, which the SSRF gate correctly blocks. The other is an intermittent
late-font timer test; this run does not establish a fully green root suite.

The unmodified companion obstacle course reported 32/33. Its
`observer-intersection` stage also fails on upstream `89964b3`; the fixture's
expected repeated intersection is not reproduced by Chrome either. Keep the
33/33 gate open rather than changing browser semantics to satisfy that fixture.
The deterministic render harness produced 65 paired captures; four assertions
failed on the Chrome side. Fifteen real sites were captured at both top and
bottom, with unstable or blank states excluded from fidelity comparisons.
These checks are local integration evidence, not a release or deployment claim.

# Local deno_core patch

This directory preserves the published `deno_core` 0.350.0 crate source,
including its upstream tests. The crate was published from upstream commit
`fc1ef37491950e82d2fb2e5e426a55723e730ce1` in
<https://github.com/denoland/deno_core> (`core` at that revision), as recorded in
`.cargo_vcs_info.json`. The crates.io archive checksum is
`e273b731fce500130790e777cb2631dc451db412975304d23816c1e444a10c5b`.
`LICENSE.md` is the MIT license from that upstream revision.

The local change in `error.rs` adds the opt-in host callback
`prepare_stack_trace_callback_with_v8_display`. Existing callbacks retain their
original behavior. Obscura installs the callback for page and worker runtimes
and during snapshot creation.

The original prepare-stack-trace path still saves the exception's callsites,
reads the user hook once, and returns arbitrary user-hook values unchanged.
Only default frame display changes: it uses the original V8 CallSite's
`toString()` and replaces its trailing source location with the mapped location
already computed by `JsStackFrame`. Eval origins receive their independently
mapped origin. Function, receiver, and alias names stay untouched, even when
they contain a filename. There is no JavaScript default hook or page/site
condition. Exception metadata, cause chains, and the existing source mapper
remain owned by deno_core.

Obscura regression coverage is in
`crates/obscura-js/src/browser_compat_tests.rs`, including native and proxy
frames, page and worker paths, custom hooks and object return values, source
maps, eval origins, and exception causes. Run with release-mode nextest as
required by the repository's V8 process isolation rules.

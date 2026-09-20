# Isolated browser runtime

This Rust RPC executable and the Python client in `../bindings/python` are owned
by the independent Obscura repository. The executable keeps the name
`autopilot-browser-runtime` for compatibility with existing execution manifests.
It contains browser transport, native input, capture, pause, and takeover primitives;
airline workflows and task scheduling belong to the consuming application.

From this directory:

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --locked --release
```

The adjacent crates and vendored taffy/cosmic-text are path dependencies in the same
Git revision. `build.rs` verifies all 16 embedded fonts against
`../persona-fonts.json`, including byte counts, SHA-256, and source declarations.
It neither reads nor patches a consuming repository.

For Linux cross-compilation on macOS, use the pinned toolchain plus cargo-zigbuild
and Zig, with target llvm-nm/llvm-objcopy on PATH:

```bash
cargo zigbuild --locked --release --target x86_64-unknown-linux-gnu.2.31
```

Native builds create an architecture-matched V8 snapshot at build time. Cross
builds create and retain the snapshot in the target process; do not substitute a
host snapshot. The Python client verifies the executable's SHA-256 before starting
its bounded RPC session. Applications supply the binary path and expected hash.

This is a separate Cargo workspace. Run commands here to select the pinned
`rust-toolchain.toml`; a root CLI build does not build this executable. See the
[integration guide](../docs/Use-the-isolated-runtime.md) for consumer ownership
and revision coordination.

Protocol 2 is negotiated by the high-level Python SDK. It adds `automation` and
`network_body`, page network event envelopes, idle event-loop pumping and bounded
body storage. Page actions remain serial; evidence reads can be served while an
action is waiting. Protocol 1 remains available for `BrowserSession` consumers.
See the [SDK contract](../bindings/python/README.md) for limits and failure states.

## SDK browser identity

Protocol 2 accepts `macos_chrome152`, `macos_chrome153`, and the legacy
`windows_chrome145` persona profiles. Every caller must provide a complete
versioned PersonaSpec including `schema_version`, `persona_id`, `revision`, and
`profile`; there is no implicit profile. Protocol 1 retains its Windows Chrome
145 restriction. The ZG example selects the macOS profile explicitly.

The macOS profile matches the captured headed Chrome 152.0.7977.83 identity:
reduced UA `Chrome/152.0.0.0`, `MacIntel`, Client Hints `macOS`, platform version
`26.6.2`, architecture `arm`, full version `152.0.7977.83`, and the captured
`en,zh-CN` language preference. These are a pinned reference identity, not host auto-detection.
Navigation, subresources and fetch/XHR share the same stealth client. Frame
realms inherit the identity.

Optional persona fields are resolved once during `init` and returned in the
ready payload. Callers may set `language`, `languages`, `accept_language`,
`timezone`, `do_not_track`, `hardware_concurrency`, `device_memory`,
`screen_width`, `screen_height`, `screen_avail_width`, `screen_avail_height`,
`outer_width`, `outer_height`, `device_scale_factor`, `battery_charging`,
`battery_level`, `network_rtt`, `storage_quota`, `webgl_vendor`, and
`webgl_renderer` in the startup Persona. Omitted fields use the selected
profile's defaults. The
runtime rejects locale combinations whose primary navigator language disagrees
with `Accept-Language`; the shared persona compiler freezes the process timezone
and ICU primary language before V8 and drives HTTP headers, JS globals, Intl, screen values, and WebGL
identity together. Privacy policy such as tracker blocking is a separate init
field and is not part of PersonaSpec.

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
See [the source comparison](../docs/Primp-and-wreq-comparison.md) for details. No hostname-specific behavior is introduced.

Both profiles use primp. Obscura does not replay GET or POST after a connection
reset. This does not disable protocol-level recovery inside primp (for example
HTTP/2 REFUSED_STREAM).

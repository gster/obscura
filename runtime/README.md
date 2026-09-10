# Isolated browser runtime

This Rust RPC executable and the Python client in `../bindings/python` are owned
by the independent Obscura repository. The executable keeps the name
`autopilot-browser-runtime` for compatibility with existing execution manifests.
It contains browser transport, native input, capture, pause, and takeover primitives;
airline workflows and task scheduling belong to the consuming application.

From this directory:

```bash
cargo build --locked --release
cargo nextest run --locked --release
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

## Requirements

- Rust 1.98.1 (validated toolchain; [rustup.rs](https://rustup.rs))
- C compiler (gcc or clang)
- ~5 GB free disk space (V8 compiles from source on first build)

First build takes about 5 minutes. Incremental builds are seconds.

## Build

```bash
git clone git@github.com:gster/obscura.git
cd obscura
cargo build --release -p obscura-cli --bins --features render
```

Binary is at `./target/release/obscura` unless `CARGO_TARGET_DIR` overrides it.
This builds the CLI. The [isolated runtime](Use-the-isolated-runtime.md) has its
own workspace, lockfile, and pinned toolchain in `runtime/`.

This produces the release binary with geometry, screenshots, screencasting,
and PDF export.

## Rendering and stealth

```bash
cargo build --release -p obscura-cli --bins --features render,stealth
```

This is the complete rendering build with the stealth primp/Rustls transport,
browser TLS profiles, browser-identity protections, and tracker
blocklist. See [Configure stealth and proxies](Configure-stealth-and-proxies.md).

## Without rendering

```bash
cargo build --release -p obscura-cli --bins --no-default-features
cargo build --release -p obscura-cli --bins --no-default-features --features stealth
```

The second command keeps stealth while excluding layout, screenshots,
screencasting, and PDF export.

The stealth feature builds primp with Rustls/AWS-LC. In addition
to the default requirements, install CMake, Clang, and the libclang/LLVM
development libraries. On Ubuntu/Debian:

```bash
sudo apt-get install build-essential cmake clang libclang-dev llvm-dev
```

On macOS, install the Xcode Command Line Tools and CMake. On Windows, install
the Visual Studio C++ Build Tools, CMake, and LLVM/Clang. Ensure the directory
containing `libclang` is available through `LIBCLANG_PATH` if bindgen cannot
locate it automatically.

## Run from the build

```bash
./target/release/obscura --version
./target/release/obscura fetch https://example.com --eval "document.title"
```

Install system-wide:

```bash
cargo install --path crates/obscura-cli --features render
```

## Tests

```bash
cargo nextest run --release --features render --no-fail-fast
```

Integration suite:

```bash
python3 tests/test_all.py
```

Use `cargo nextest`, not `cargo test`: runtime tests require process isolation
because the engine owns a single V8 isolate per process.

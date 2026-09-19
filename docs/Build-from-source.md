> 目标变更：stealth 将成为不可关闭的基线，所有产品出站 HTTP(S) 统一使用校准后的 primp（OB-012/044）。本页保留当前源码所需的 feature/开关用法，不能将计划当作已实现。

## Requirements

- Rust 1.98.1 (validated toolchain; [rustup.rs](https://rustup.rs))
- C compiler (gcc or clang)
- Sufficient disk and memory for Rust, V8 artifacts, native dependencies and linking; measure on the target host.

The `v8` dependency normally downloads a prebuilt archive; `V8_FROM_SOURCE` selects its source build. Obscura itself generates a bootstrap snapshot during native builds. Build time depends on caches and platform, so there is no fixed five-minute guarantee. The root workspace and `runtime/` both pin Rust 1.98.1 in their own toolchain files.

## Build

```bash
git clone git@github.com:gster/obscura.git
cd obscura
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release -p obscura-cli --bins --features render
```

Binary is at `./target/release/obscura` unless `CARGO_TARGET_DIR` overrides it.
This builds the CLI. The [isolated runtime](Use-the-isolated-runtime.md) has its
own workspace, lockfile, and pinned toolchain in `runtime/`.

This produces the release binary with geometry, screenshots, screencasting,
and PDF export.

## Rendering and stealth

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release -p obscura-cli --bins --features render,stealth
```

This is the complete rendering build with the stealth primp/Rustls transport,
browser TLS profiles, browser-identity protections, and tracker
blocklist. See [Configure stealth and proxies](Configure-stealth-and-proxies.md).

## Without rendering

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release -p obscura-cli --bins --no-default-features
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release -p obscura-cli --bins --no-default-features --features stealth
```

The second command keeps stealth while excluding layout, screenshots,
screencasting, and PDF export.

The stealth feature builds primp with Rustls/AWS-LC. In addition
to the default requirements, install CMake, Clang, and the libclang/LLVM
development libraries. On Ubuntu/Debian:

```bash
sudo apt-get install build-essential cmake clang libclang-dev llvm-dev
```

On macOS, install the Xcode Command Line Tools and CMake. Ensure the directory
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

The previously documented `tests/test_all.py` does not exist in this checkout. Use the tracked crate tests and the independent runtime tests described in [Testing and debugging](Testing-and-debugging.md).

Use `cargo nextest`, not `cargo test`: runtime tests require process isolation
under the project's V8 test-isolation requirements; production pages can own separate isolates.

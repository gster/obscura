# Obscura Python runtime client

`obscura-runtime` exposes `BrowserSession`, `BrowserError`, `PageRef`, `ClickResult`,
and `file_hash`. It launches and owns the Rust executable from `../../runtime`,
with bounded RPC, cancellation, native input, capture, and takeover support.
There is no dependency on Autopilot or airline business code.

```bash
uv build --wheel --out-dir /absolute/wheelhouse
```

Consumers install this wheel and `psutil>=7,<8`. The Rust executable is supplied
separately with its SHA-256; it is not embedded in the Python wheel. Consumers keep
their workflow orchestration and task-control policy outside this package.

## Runtime

Unless an example is specifically demonstrating `OBSCURA_PERSONA`, shell
examples below assume `export OBSCURA_PERSONA=windows_chrome145` is already set.

### `OBSCURA_PERSONA`

Required startup identity when `--persona` is not supplied. The value is either
a built-in preset (`windows_chrome145`, `macos_chrome152`, or
`macos_chrome153`) or a path to a versioned PersonaSpec JSON file. The CLI
option wins when both are present. Missing or invalid persona input stops before
the command creates a browser context, worker, or listening service.

```bash
OBSCURA_PERSONA=windows_chrome145 obscura fetch https://example.com
```

### `OBSCURA_ALLOW_PRIVATE_NETWORK`

Allow fetches to loopback (`127.0.0.0/8`), RFC1918 (`10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`), and link-local (`169.254.0.0/16`, including the `169.254.169.254` cloud-metadata endpoint) addresses. The deny-set also covers the unspecified address (`0.0.0.0` / `::`), IPv6 unique-local (`fc00::/7`), and any IPv4-mapped form of the above. Off by default to block SSRF.

The guard validates at DNS-resolution time as well as on literal hosts, for direct destination resolution. With HTTP CONNECT or remote-DNS SOCKS, the proxy resolves the destination; local DNS filtering alone does not establish the proxy's final destination IP policy. Literal URL and redirect checks still apply.

Truthy values: `1`, `true`, `yes`, `on`.

```bash
OBSCURA_ALLOW_PRIVATE_NETWORK=1 obscura fetch http://localhost:8080
```

Per-process equivalent: `--allow-private-network` on any subcommand.

### `OBSCURA_NAV_TIMEOUT_MS`

Hard ceiling on a single navigation. Default 30000 (30 seconds). Applies to `Page.navigate` and the CLI `fetch` command.

```bash
OBSCURA_NAV_TIMEOUT_MS=60000 obscura serve
```

### `OBSCURA_NAV_CHAIN_LIMIT`

How many documents a navigation chain may load, the first navigation included. Default 10, which allows the requested document and nine navigations the page itself triggers via `location` assignments or form submissions. Raise the value for an endpoint that chains longer for good reasons, such as an SSO handover across several providers. The low default is what stops a page that resets `location` on every load.

A zero is raised to 1. This loads the requested document. If the page wants to chain further afterwards, the call reports an error, as at any other limit. A value the engine does not read as a number is replaced by the default. This also applies to a negative value and to a value with a trailing space.

The time budget is not tied to this limit. A longer chain usually also needs a higher `OBSCURA_NAV_TIMEOUT_MS`, because its default of 30 seconds applies to the whole chain and not to the individual document.

```bash
OBSCURA_NAV_CHAIN_LIMIT=20 obscura serve
```

### `OBSCURA_SCRIPT_DEADLINE_MS`

Soft deadline for the complete page script-execution phase, including classic scripts and ES modules. Default 30000 (30 seconds). Raise it for a heavy SPA whose initial module is responsible for mounting an otherwise empty document. The engine also uses this value as a hard V8 watchdog budget, with a one-second grace period, so a synchronous script cannot run forever.

```bash
OBSCURA_SCRIPT_DEADLINE_MS=60000 obscura serve
```

### `OBSCURA_MODULE_BUDGET_MS`

Per-module graph-loading and evaluation budget for modules that enhance an already-rendered page. Default 3000 (3 seconds). Raise it when a module such as the Vite HMR client legitimately needs longer to evaluate:

```bash
OBSCURA_MODULE_BUDGET_MS=10000 obscura serve
```

This shorter budget applies when the document body already contains more than 50 descendant nodes, where modules are normally progressive enhancement and should not delay navigation indefinitely. For an unmounted SPA shell, Obscura instead gives each module the full `OBSCURA_SCRIPT_DEADLINE_MS` budget so the app has time to mount. Module network requests remain independently bounded by `OBSCURA_FETCH_TIMEOUT_MS`.

### `OBSCURA_CDP_COMMAND_TIMEOUT_MS`

Per-command deadline for the CDP server. The dispatcher arms the V8 watchdog around commands. This bounds synchronous V8 work on the owning connection; it is not a universal cancellation guarantee for arbitrary native work. Default 60000 (60 seconds); `0` disables it. Navigation self-bounds via `OBSCURA_NAV_TIMEOUT_MS` well under this.

```bash
OBSCURA_CDP_COMMAND_TIMEOUT_MS=30000 obscura serve
```

### `OBSCURA_CDP_TOKEN`

Static Bearer token required by every CDP discovery and WebSocket request when
configured. Use at least 32 visible ASCII bytes without whitespace. This is an
alternative to `serve --auth-token-file`; configuring both is an error. The
token is admission metadata: it is not placed in discovery URLs, logs, or CDP
network events. Page-request Authorization headers remain complete in the
normal observation pipeline.

```bash
OBSCURA_CDP_TOKEN='replace-with-at-least-32-visible-bytes' \
  obscura --persona windows_chrome145 serve --port 9222
```

### `OBSCURA_FETCH_TIMEOUT_MS`

Request timeout for scripted `fetch()`, `XMLHttpRequest`, and ES-module loads. Without it a request to a server that accepts the connection but never responds (including a CORS preflight) hangs forever and the XHR is stuck with no completion event. Default 30000 (30 seconds).

```bash
OBSCURA_FETCH_TIMEOUT_MS=15000 obscura serve
```

### `OBSCURA_PROXY`

Default proxy URL used by `obscura-worker` for the parallel `scrape` command when no `--proxy` flag is set.

```bash
OBSCURA_PROXY=http://proxy.example.com:8080 obscura scrape - < urls.txt
```

## Identity configuration

Timezone, geolocation, locale, viewport and the transport profile belong to the
single PersonaSpec selected with `--persona` or `OBSCURA_PERSONA`. The legacy
`OBSCURA_PROFILE`, `OBSCURA_ROTATE_PROFILE`, `OBSCURA_TIMEZONE`, and
`OBSCURA_GEOLOCATION` variables are not read as product identity inputs. See
[Configure stealth and proxies](Configure-stealth-and-proxies.md).

The built-in preset's timezone is part of that preset. Override it in an
external PersonaSpec, not with a separate environment variable.

## MCP

### `OBSCURA_MCP_ALLOWED_ORIGINS`

Comma-separated `Origin` allowlist for the HTTP MCP transport (`obscura mcp --http`). Off by default, which keeps the permissive behavior. When set, a browser request whose `Origin` is not listed is refused with `403` before it can drive the server; native, non-browser MCP clients (which send no `Origin`) are always allowed. Use it to stop cross-origin pages from reaching a loopback MCP port.

```bash
OBSCURA_MCP_ALLOWED_ORIGINS="https://app.example.com" obscura mcp --http --host 0.0.0.0
```

## Logging

### `RUST_LOG`

Standard `tracing` filter. Common settings:

```bash
RUST_LOG=obscura=info obscura serve
RUST_LOG=obscura=debug obscura serve
RUST_LOG=obscura_cdp=trace,obscura_browser=debug obscura serve
```

`--verbose` on the CLI is equivalent to `RUST_LOG=obscura=info`.

## Build

### `OPENSSL_NO_VENDOR`

Forces `cargo build` to use the system OpenSSL instead of compiling the vendored copy. Set to `1` on hosts where the vendored OpenSSL fails (older VPS with AVX-512 issues).

```bash
OPENSSL_NO_VENDOR=1 cargo build --release --features render
```

## V8

V8 flags are passed via `--v8-flags`, not environment variables:

```bash
obscura serve --v8-flags "--max-old-space-size=2048 --expose-gc"
```

Defaults are `--max-old-space-size=4096 --max-semi-space-size=4 --optimize-for-size` on 64-bit systems (a 4 GB old-space ceiling, a capped young generation, and codegen tuned for a smaller footprint to cut RSS). Anything you pass with `--v8-flags` is appended after these, and V8 uses the last value for a repeated flag, so your value wins for that flag while the other defaults stay in effect.

## HTTP proxy environment

Obscura does not honor `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY`. Use `--proxy` or `OBSCURA_PROXY`.

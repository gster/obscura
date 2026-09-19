> 所有产品出站 HTTP(S) 默认使用统一 persona 和 primp；没有运行时 stealth 开关。

# 当前部署边界

这里描述现有 serve 的运维要求，不构成生产资格。新宿主、安全边界和双平台发布仍在 [TODO](TODO.md) 中。

## Container build status

The tracked Dockerfile copies vendored dependencies and builds the root CLI with `--features render`, including the mandatory primp and browser-identity baseline, then uses a nonroot distroless runtime (uid/gid 65532). The registry image is an upstream artifact and does not identify this fork's build. No Docker build was run in this audit.

Before deployment, select and test a pinned build, bind published ports to host loopback, set writable private storage and explicit resource limits. The current cookie save path can ignore write errors; inspect persisted files and restored behavior.

## Systemd

`/etc/systemd/system/obscura.service`:

```ini
[Unit]
Description=Obscura headless browser
After=network.target

[Service]
ExecStart=/usr/local/bin/obscura serve --port 9222 --storage-dir /var/lib/obscura
Restart=always
RestartSec=5
User=obscura
Group=obscura
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```bash
systemctl enable --now obscura
journalctl -fu obscura
```

## Workers

`obscura serve --workers N` runs N CDP server workers behind the listener.

```bash
obscura serve --workers 4
```

Select worker count using measured workload and resource budgets. Each accepted CDP connection owns its pages; reconnecting does not restore its targets.

## V8 heap

Default V8 heap is 4 GB on 64-bit systems. The defaults also cap the young generation (`--max-semi-space-size=4`) and pass `--optimize-for-size` to hold RSS down. Override:

```bash
obscura serve --v8-flags "--max-old-space-size=2048"
```

Flags you pass are appended after the defaults, and V8 uses the last value for a repeated flag, so your `--max-old-space-size` wins while the memory-tuning defaults stay in effect. Lower for memory-constrained hosts, raise for heavy SPAs.

## Parallel scrape

`obscura scrape` fans out a list of URLs across worker processes:

```bash
obscura scrape \
  --concurrency 20 \
  --format json \
  --timeout 60 \
  url1 url2 url3 ...
```

Reads URLs from stdin:

```bash
cat urls.txt | obscura scrape --concurrency 20 -
```

Requires `obscura-worker` next to `obscura` in `PATH`.

## Resource limits

Per-process memory cap with systemd:

```ini
[Service]
MemoryMax=4G
MemoryHigh=3G
```

Per-container with Docker:

```bash
docker run --memory=4g --cpus=2 ...
```

## Reverse proxy

Expose obscura on TLS through nginx or caddy:

```nginx
location /obscura/ {
  proxy_pass http://127.0.0.1:9222/;
  proxy_http_version 1.1;
  proxy_set_header Upgrade $http_upgrade;
  proxy_set_header Connection "upgrade";
  proxy_read_timeout 86400;
}
```

CDP needs WebSocket upgrade and long read timeouts.

## Authentication

Obscura's CDP server has no built-in auth. Anyone who can reach the port can drive the browser. Options:

- Bind to `127.0.0.1` and require SSH for access (default).
- Put it behind a reverse proxy that enforces auth.
- Use Docker network isolation.

Never bind `0.0.0.0` on a public IP without one of the above.

## MCP HTTP transport

`obscura mcp --http` binds `127.0.0.1` by default. To reach it from another container, bind with `--host 0.0.0.0` and set an `Origin` allowlist so a browser page cannot drive it cross-origin:

```bash
OBSCURA_MCP_ALLOWED_ORIGINS="https://app.example.com" \
  obscura mcp --http --host 0.0.0.0 --port 3000
```

Request bodies are capped at 16 MiB. Like the CDP server it has no built-in auth, so keep it on an internal network or behind an authenticating proxy. See [Use the MCP server](Use-the-MCP-server.md).

## Observability

```bash
obscura serve --verbose
RUST_LOG=obscura=debug obscura serve
```

`--verbose` enables info-level logs. `RUST_LOG=obscura=debug` enables debug-level. Logs go to stderr.

## Reliability and timeouts

The engine includes V8 termination, panic guards, DOM cycle rejection and several request deadlines. These reduce specific failure modes; they do not guarantee arbitrary pages cannot crash or exhaust a process, and do not replace OS isolation or supervision.

Tune the bounds with environment variables (see [Environment variables](Environment-variables.md)):

```bash
OBSCURA_NAV_TIMEOUT_MS=60000 \
OBSCURA_SCRIPT_DEADLINE_MS=45000 \
OBSCURA_MODULE_BUDGET_MS=10000 \
OBSCURA_CDP_COMMAND_TIMEOUT_MS=70000 \
OBSCURA_FETCH_TIMEOUT_MS=20000 \
  obscura serve
```

`OBSCURA_NAV_TIMEOUT_MS` is the per-navigation ceiling (default 30000). `OBSCURA_SCRIPT_DEADLINE_MS` bounds the complete script phase (default 30000), while `OBSCURA_MODULE_BUDGET_MS` bounds each enhancement module's graph loading and evaluation (default 3000; unmounted SPA shells use the full script deadline). `OBSCURA_CDP_COMMAND_TIMEOUT_MS` is the outer per-CDP-command V8 deadline (default 60000, `0` disables), so keep it above the navigation ceiling. `OBSCURA_FETCH_TIMEOUT_MS` bounds scripted fetch/XHR and module network requests (default 30000).

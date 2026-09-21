> MCP 与有用的 CLI 保留，用于自动化和 agent 接入。所有入口自动使用统一 persona 和 primp；没有运行时 stealth 开关。

## `obscura`

Global flags can appear before or after the subcommand. Their effect depends on the command; `--obey-robots` applies to fetch and scrape. Every product command requires `--persona` or `OBSCURA_PERSONA`; there is no implicit identity.

```
-v, --verbose                Enable info logging
-p, --port <PORT>            CDP port (default 9222)
    --proxy <URL>            HTTP or SOCKS5 proxy
    --persona <VALUE>        Built-in preset name or PersonaSpec JSON path (required)
    --obey-robots            Respect robots.txt
    --storage-dir <DIR>      Cookie persistence only; see storage limitations
    --allow-private-network  Permit loopback / RFC1918 / link-local
    --v8-flags <FLAGS>       Raw V8 flags, applied at startup
-h, --help                   Help
-V, --version                Version
```

## `obscura fetch <URL>`

Load a URL and print its content or an evaluated expression.

```
    --dump <FORMAT>          html | text | links | markdown | original | assets | cookies
                             (default html)
    --selector <CSS>         Narrow output to a CSS selector
    --wait <SECONDS>         Fixed post-load delay; omitted uses adaptive settle (5s cap)
    --timeout <SECONDS>      Navigation timeout (default 30)
    --wait-until <LEVEL>     domcontentloaded | load | networkidle2 | networkidle0
                             (default load)
    --proxy <URL>            HTTP or SOCKS5 proxy
-e, --eval <JS>              Evaluate JS, print the result as JSON
-o, --output <FILE>          Write to a file instead of stdout
-s, --screenshot <FILE>      Capture the settled page as PNG (single URL)
-q, --quiet                  Suppress info logging
-v, --verbose                Enable verbose logging
```

`--screenshot` requires a render-enabled build. It uses a 1280×720 viewport by
default and may be combined with `--eval`; the expression runs before capture,
which is useful for scrolling or preparing page state. It is not available in
`--file` batch mode.

When `--wait` is omitted, Obscura drives timers and async work until the page
becomes quiescent, with a five-second ceiling. Supplying `--wait N` instead
requests a fixed `N`-second delay. `--timeout` separately bounds navigation.

`--dump` values:

| Value      | Output                                                    |
| ---------- | --------------------------------------------------------- |
| `html`     | Rendered HTML (default)                                   |
| `text`     | Plain text                                                |
| `markdown` | Markdown conversion                                       |
| `links`    | Every `<a href>`, one URL per line                        |
| `assets`   | Every external resource, one JSON object per line (DOM assets plus `fetch()`/XHR requests) |
| `original` | Raw HTTP response body (binary-safe, bypasses the engine) |
| `cookies`  | All cookies in the jar as a JSON array, including HttpOnly cookies invisible to `document.cookie` |

## `obscura serve`

Run the CDP server. The supported client path is official Playwright Python over WebSocket.

```
-p, --port <PORT>            CDP port (default 9222)
    --host <HOST>            Bind host (default 127.0.0.1)
    --allow-host <HOST[:PORT]>  Accepted CDP Host authority (repeatable)
    --allow-origin <ORIGIN>  Additional accepted browser Origin (repeatable)
    --auth-token-file <FILE> Read the CDP Bearer token from a file
    --advertise-websocket-url <WS_URL>
                             Public root ws:// or wss:// discovery URL
    --allow-unauthenticated-remote
                             Permit remote bind without a token; unsafe unless
                             another authenticated boundary protects it
    --proxy <URL>            HTTP or SOCKS5 proxy
    --workers <N>            Worker processes (default 1)
    --max-connections <N>    Maximum admitted CDP WebSockets per worker,
                             including authorized handoffs and active connections
                             (default 128); the multi-worker parent admits at
                             most workers * max-connections concurrent relays
    --font-dir <DIR>         Recursively load fonts once per worker (repeatable; render build)
    --allow-file-access      Permit CDP clients to navigate to file:// URLs
    --storage-dir <DIR>      Cookie persistence only; see storage limitations
    --allow-private-network  Permit loopback / RFC1918 / link-local
-q, --quiet                  Suppress info logging
-v, --verbose                Enable info logging
```

Default endpoint is `ws://127.0.0.1:9222`. Loopback accepts only the exact
listener authority (plus `localhost` at the same port). Native clients may omit
`Origin`; a supplied Origin must be same-origin or listed by `--allow-origin`.
Non-loopback binds require at least one `--allow-host` and, by default, a token
from `--auth-token-file` or `OBSCURA_CDP_TOKEN`. These ingress controls are
independent of outbound `--allow-private-network`. Host authorities are exact:
if an allowlist entry includes a port, the request Host must include that port;
an entry without a port matches only a Host without a port.

The exact HTTP discovery routes are `/json`, `/json/`, `/json/list`,
`/json/version`, `/json/version/` (used by Playwright 1.60), and
`/json/protocol`. Query-bearing and substring-lookalike routes are rejected.

## `obscura scrape [URLS]...`

Run a JS expression across many URLs in parallel.

```
-e, --eval <JS>              JS to run on each page
    --concurrency <N>        Parallel pages (default 10)
    --format <FORMAT>        Output format (default json)
    --timeout <SECONDS>      Per-URL timeout (default 60)
    --proxy <URL>            HTTP or SOCKS5 proxy
    --allow-private-network  Permit loopback / RFC1918 / link-local
-q, --quiet                  Suppress info logging
-v, --verbose                Enable verbose logging
```

`--proxy` and `--allow-private-network` are global flags: they work before or after any subcommand, and each `scrape` worker inherits them.

Read URLs from stdin with `-`:

```bash
cat urls.txt | obscura --persona windows_chrome145 scrape - --eval "document.title" --concurrency 20
```

Requires `obscura-worker` next to `obscura` in `PATH`.

## `obscura mcp`

Run obscura as an MCP server.

```
    --http                   HTTP transport instead of stdio
    --host <HOST>            HTTP bind host (default 127.0.0.1)
    --port <PORT>            HTTP port (default 3000)
    --proxy <URL>            HTTP or SOCKS5 proxy
    --allow-private-network  Permit loopback / RFC1918 / link-local
-v, --verbose                Enable info logging
```

`--host` only applies with `--http`. The default `127.0.0.1` keeps the server loopback-only; set `0.0.0.0` to bind all interfaces (for example a Docker Compose sidecar) and pair it with `OBSCURA_MCP_ALLOWED_ORIGINS`.

Default transport is stdio. See [Use the MCP server](Use-the-MCP-server.md).

Render-enabled builds add `browser_screenshot` and `browser_pdf` to the MCP
tool list. Streaming screencasts are available through CDP rather than MCP.

`--stealth` and `--user-agent` are not CLI or `serve` parameters. The active
persona owns the primp transport profile, HTTP headers, Client Hints and
JavaScript navigator identity as one calibrated identity. The persona is fixed
when a BrowserContext is created and remains immutable until that context is
closed. `Network.setUserAgentOverride` is therefore unsupported.
`Network.setExtraHTTPHeaders` still forwards ordinary headers unchanged, while
persona-owned User-Agent, Client Hints, language, encoding, and DNT headers are
rejected for the same context-lifetime reason.

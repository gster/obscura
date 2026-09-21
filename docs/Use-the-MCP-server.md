> MCP 与有用的 CLI 保留，用于自动化和 agent 接入。所有入口必须显式选择统一 persona；下列命令假定已设置 `export OBSCURA_PERSONA=windows_chrome145`。

`obscura mcp` exposes obscura as a Model Context Protocol server so MCP-capable clients (Claude Desktop, Claude Code, etc.) can drive it.

## Run

Stdio (default, for direct client integration):

```bash
obscura mcp
```

HTTP (for remote or shared use):

```bash
obscura mcp --http --port 3000
```

The HTTP transport binds `127.0.0.1` by default. Bind all interfaces with `--host` for a container or sidecar deployment:

```bash
obscura mcp --http --host 0.0.0.0 --port 3000
```

With a proxy:

```bash
obscura mcp --proxy http://proxy.example.com:8080
```

To retain Network observation archives across process restarts:

```bash
obscura mcp --storage-dir ./obscura-state
```

## Security

The HTTP transport has no built-in auth, so anyone who can reach the port can drive the browser. Two guards ship for the HTTP transport:

- **Origin allowlist.** Set `OBSCURA_MCP_ALLOWED_ORIGINS` to a comma-separated list of allowed `Origin` values. When set, a browser request from an unlisted origin is refused with `403` before it can drive the server, which blocks a malicious page from POSTing to a loopback MCP port. Native, non-browser clients send no `Origin` and are always allowed. Unset (the default) keeps the permissive behavior.
- **Body cap.** A single request body is capped at 16 MiB, so an unauthenticated caller cannot force a large allocation with an oversized `Content-Length`.

```bash
OBSCURA_MCP_ALLOWED_ORIGINS="https://app.example.com" obscura mcp --http --host 0.0.0.0
```

When you expose the HTTP transport beyond loopback, set the allowlist and put it behind a reverse proxy or network isolation that enforces auth.

## Tools exposed

The server keeps a live browser session, so tools operate on the current page rather than taking a URL each call. Navigate first, then read or act.

Navigation and lifecycle:

- `browser_navigate`, `browser_back`, `browser_forward`, `browser_reload`, `browser_close`

Read the page:

- `browser_snapshot`: current URL, title, readable body text, and interactive
  element references. Optional `max_chars` limits the returned text.
- `browser_markdown`, `browser_links`, `browser_extract`: page as markdown, link list, or structured content.
- `browser_interactive_elements`, `browser_detect_forms`: actionable elements and form fields.
- `browser_get_attribute`, `browser_count`, `browser_search`: read an attribute, count matches, find text.

Interact:

- `browser_click`, `browser_fill`, `browser_fill_form`, `browser_type`, `browser_press_key`, `browser_select_option`, `browser_scroll`

Wait and run JS:

- `browser_wait_for`, `browser_wait_for_text`, `browser_evaluate`

The two wait tools use one absolute timeout (30 seconds by default), advance
page timers and queued navigation within that budget, and read the native DOM.
`timeout` accepts fractional non-negative seconds; invalid or out-of-range
values return a tool error.

Diagnostics:

- `browser_network_requests`, `browser_network_histories`,
  `browser_network_history`, `browser_network_body`,
  `browser_console_messages`

Visual output (render-enabled builds):

- `browser_screenshot`: current viewport as an MCP `image/png` content block.
- `browser_pdf`: current page as an embedded `application/pdf` resource.

`browser_screenshot` accepts optional positive `width` and `height` values in
CSS pixels and enforces a bounded capture size. `browser_pdf` accepts
`landscape`, `print_background`, `scale`, paper width/height, and top, bottom,
left, and right margins. Paper dimensions and margins are measured in inches.

Cookies and storage:

- `browser_get_cookies`, `browser_set_cookie`, `browser_clear_cookies`, `browser_storage_state`, `browser_set_storage_state`

Tabs:

- `browser_tab_new`, `browser_tab_list`, `browser_tab_switch`, `browser_tab_close`

Element references describe the current rendered page state and can become
stale after navigation, interaction, scrolling, or a framework rerender. Take
a fresh snapshot or interactive-element listing before acting again.

MCP exposes still-image and PDF output. It does not stream video frames; use
CDP `Page.startScreencast` for activity-driven screencasting.

### Network observations

`browser_network_requests` returns pretty JSON with an `events` array, including
`{"events": []}` for an empty active-page history. Each entry is one lifecycle
observation (`started`, `redirect`, `completed`, or `failed`), not one
deduplicated request. Records come from the context-owned append-only history,
so navigation does not replace earlier records and closing a page does not
delete them. Use `sequence`, `history_id`, immutable `page_instance_id`,
`request_id`, and document identity fields to relate entries. A standalone
started record is not guaranteed: ordinary successful fetches may have only a
completed record, while preflight paths can publish a separate start.

The output preserves every current `NetworkEvent` field, including errors,
request/response compatibility header maps and lossless raw header captures.
Raw header fields retain repeated values and arbitrary bytes as base64, including
Cookie and Authorization. `headers` is the request compatibility map;
`response_headers` is the response compatibility map; `request_raw_headers` and
`raw_headers` are their respective lossless captures. Exact request,
transport-request, and response bodies are included as UTF-8 or standard base64,
with an immutable `body_ref`; explicit empty bodies remain distinct from absent
bodies. Capture stages describe their source, not HTTP wire framing. Reading
records or bodies never consumes them.

For bounded retrieval, first call `browser_network_histories` to discover the
live history and archives recovered from `--storage-dir`. Then call
`browser_network_history` with `history_id`, `after_sequence`, `limit`, and an
optional `page_instance_id`. The result includes the next committed sequence,
page registration/close state, finalization, and the complete sticky terminal
failure when present. Use `browser_network_body` with a record's `body_key` for
repeatable offset/length chunks; binary chunks are standard base64. Recovery
keeps every checksum-valid committed record before an incomplete, corrupt, or
over-limit tail and reports the failure explicitly.

History admission is bounded by the `OBSCURA_NETWORK_HISTORY_*` variables. It
is an accepted-prefix contract, not an eviction policy: a limit or persistence
failure does not drop old records and does not permit later network work to
continue unobserved.

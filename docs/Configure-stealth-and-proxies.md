## Browser identity baseline

```bash
obscura --persona windows_chrome145 fetch https://example.com
obscura --persona windows_chrome145 serve
obscura --persona windows_chrome145 scrape url1 url2
obscura --persona windows_chrome145 mcp
```

All product entry points always use the same baseline:

- Uses the primp HTTP client with Chrome TLS profiles (ClientHello, ALPN, cipher order). See [known fidelity gaps](Primp-and-wreq-comparison.md).
- Includes an optional tracker blocklist. Third-party requests are allowed by
  default to preserve Chrome network behavior. CDP serve can enable blocking
  with `OBSCURA_BLOCK_TRACKERS=1`; Rust embedders use
  `BrowserContextOptions.block_trackers = true`.
- Uses bundled webpki roots and supports explicitly configured certificate roots; see [certificate configuration](Environment-variables.md).

The primp transport is part of every build. The legacy Cargo `stealth` feature
is an empty compatibility alias and does not change behavior. Build the
rendering variant with:

```bash
cargo build --release -p obscura-cli --bins --features render
```

Omit rendering with `cargo build --release -p obscura-cli --bins --no-default-features`.

## Verification boundary

Transport presets and resource blocking do not guarantee website acceptance or full Chrome identity. Blocking functional third-party resources may break a page. Validate the actual request path, realm, effective persona and response; do not infer correctness from a JA3/JA4 match or a single HTTP 200.

## Proxies

HTTP proxy:

```bash
obscura --persona windows_chrome145 fetch https://example.com --proxy http://proxy.example.com:8080
obscura --persona windows_chrome145 serve --proxy http://proxy.example.com:8080
```

With auth:

```bash
obscura --persona windows_chrome145 fetch https://example.com --proxy http://user:pass@proxy.example.com:8080
```

SOCKS5:

```bash
obscura --persona windows_chrome145 fetch https://example.com --proxy socks5://proxy.example.com:1080
```

## Browser identity

`--stealth` and `--user-agent` are not CLI or `serve` parameters. Browser
identity is configured at the persona layer so the primp TLS/HTTP profile,
Client Hints and JavaScript identity are compiled together before the first
page or request. The effective persona is immutable for the BrowserContext
lifetime; create a new context to use a different persona.
`Network.setUserAgentOverride` is unsupported because its partial, page-scoped
inputs cannot atomically replace an already-active context identity.
`Network.setExtraHTTPHeaders` accepts ordinary headers with their complete
values, but rejects persona-owned User-Agent, Client Hints, language, encoding,
and DNT headers. Create a new context when those identity fields must change.
Current primp presets include Windows Chrome 145 and macOS Chrome 152/153;
these are implementation choices, not certified Linux/macOS personas.
The `macos_chrome153` persona selects the Chrome 153 transport profile,
including its measured five-byte `h2` ALPS offer and the Chrome 153 Trust
Anchor ID set. The ID order is randomized once per Obscura process, following
the observed Chrome process behavior. These bytes are versioned profile data:
changing only `persona_id`, User-Agent text, or viewport does not turn another
browser version into this profile. Other presets retain their own TLS data.

## Persona, timezone, and geolocation

All entry points consume the same versioned PersonaSpec. A built-in preset name
is the shortest valid input; a JSON file can override optional fields such as
timezone, geolocation, viewport and WebGL metadata while retaining a supported
transport profile:

`screen_width`, `screen_height`, `screen_avail_width`, `screen_avail_height`,
`outer_width`, `outer_height`, and `screen_color_depth` (24, 30, or 32) are
separate persona fields. Set them together when matching a measured Chrome
environment; a window's outer dimensions may exceed its reported screen.

The built-in `windows_chrome145` preset includes `Europe/Berlin`; the macOS
presets include `Asia/Shanghai`. These values are fields of those complete
personas, not independent global defaults. Use an external PersonaSpec when the
identity needs a different timezone.

```json
{
  "schema_version": "1",
  "persona_id": "new-york-worker",
  "revision": "2026-09-20",
  "profile": "windows_chrome145",
  "timezone": "America/New_York",
  "geolocation": {"latitude": 40.7128, "longitude": -74.0060}
}
```

```bash
obscura --persona ./persona-new-york.json serve
```

The compiler rejects unknown schema versions, unknown fields, unsupported
profiles and incoherent locale values before a context becomes usable. The
Persona v1 locale grammar is a canonical BCP47 subset: language, optional
Script, optional REGION and ordinary variants (for example `en-US` or
`zh-Hant-TW`). Non-canonical casing, extensions and private-use tags are
rejected until their V8 and HTTP projections are explicitly supported. The
compiled result is immutable and drives primp, HTTP headers, JavaScript,
frames, workers,
screen geometry and CDP diagnostics. Because V8 timezone state is process-wide,
one process accepts only personas with the startup timezone and ICU primary
language; a conflicting CDP context is rejected before registration. Use
separate processes for different timezone/locale combinations.
`OBSCURA_PROFILE`, `OBSCURA_ROTATE_PROFILE`, `OBSCURA_TIMEZONE` and
`OBSCURA_GEOLOCATION` are not runtime identity sources.

## Combine

```bash
obscura --persona windows_chrome145 serve \
  --proxy http://user:pass@proxy.example.com:8080
```

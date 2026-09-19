## Browser identity baseline

```bash
obscura fetch https://example.com
obscura serve
obscura scrape url1 url2
obscura mcp
```

All product entry points always use the same baseline:

- Uses the primp HTTP client with Chrome TLS profiles (ClientHello, ALPN, cipher order). See [known fidelity gaps](Primp-and-wreq-comparison.md).
- Loads a tracker blocklist that drops requests to known analytics and fingerprinting endpoints.
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
obscura fetch https://example.com --proxy http://proxy.example.com:8080
obscura serve --proxy http://proxy.example.com:8080
```

With auth:

```bash
obscura fetch https://example.com --proxy http://user:pass@proxy.example.com:8080
```

SOCKS5:

```bash
obscura fetch https://example.com --proxy socks5://proxy.example.com:1080
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

## Browser profile, timezone, and geolocation

The CLI's environment-selected profile and the isolated runtime's Persona are different configuration paths. Do not transfer a setting between them without checking its consumer. WebGL has a limited shim and renderer metadata; the old statement that every `getContext('webgl')` returns null is obsolete. Neither metadata nor a working clear/readPixels subset establishes complete GPU emulation.

A single stable profile is used by the product entry points. Persona selection
will move to the unified configuration tracked by OB-015/016; the old
`OBSCURA_PROFILE` and `OBSCURA_ROTATE_PROFILE` selectors are not product knobs.

Timezone is driven by the process zone so `Date` (`getTimezoneOffset`, `toString`) and `Intl.DateTimeFormat` report the same region. Default is `Europe/Berlin`; set it to match the exit IP:

```bash
OBSCURA_TIMEZONE=America/New_York obscura serve
```

`navigator.geolocation` reports configurable coordinates. Set them as `lat,lon` and keep them consistent with the timezone and proxy region:

```bash
OBSCURA_GEOLOCATION="40.7128,-74.0060" obscura serve
```

Keep these aligned. A rotated or mismatched profile carries no matching TLS or timezone fingerprint, so when you pin a proxy region or TLS fingerprint, leave rotation off and set the timezone and geolocation to the same region. See [Environment variables](Environment-variables.md) for the full list.

## Combine

```bash
obscura serve \
  --proxy http://user:pass@proxy.example.com:8080
```

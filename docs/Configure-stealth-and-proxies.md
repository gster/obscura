> 目标变更：stealth 将成为不可关闭的基线，所有产品出站 HTTP(S) 统一使用校准后的 primp（OB-012/044）。本页保留当前源码所需的 feature/开关用法，不能将计划当作已实现。

## Stealth mode

```bash
obscura fetch https://example.com --stealth
obscura serve --stealth
obscura scrape url1 url2 --stealth
obscura mcp --stealth
```

`--stealth` is a global flag, so it works before or after the subcommand and applies to `fetch`, `serve`, `scrape`, and `mcp`. In a `scrape` run each worker inherits it.

What `--stealth` changes:

- Uses the primp HTTP client with Chrome TLS profiles (ClientHello, ALPN, cipher order). See [known fidelity gaps](Primp-and-wreq-comparison.md).
- Loads a tracker blocklist that drops requests to known analytics and fingerprinting endpoints.
- Uses bundled webpki roots and supports explicitly configured certificate roots; see [certificate configuration](Environment-variables.md).

The primp transport requires a build that includes the stealth feature. Fix the source revision and build features in the artifact manifest. To build the rendering variant:

```bash
cargo build --release -p obscura-cli --bins --features render,stealth
```

Omit rendering with `cargo build --release -p obscura-cli --bins --no-default-features --features stealth`.

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

## Custom User-Agent

```bash
obscura fetch https://example.com --user-agent "Mozilla/5.0 (...) ..."
obscura serve --user-agent "Mozilla/5.0 (...) ..."
```

A custom UA does not automatically change the TLS, Client Hints, JavaScript, font or graphics configuration. Current primp presets include Windows Chrome 145 and macOS Chrome 152/153; these are implementation choices, not certified Linux/macOS personas.

## Browser profile, timezone, and geolocation

The CLI's environment-selected profile and the isolated runtime's Persona are different configuration paths. Do not transfer a setting between them without checking its consumer. WebGL has a limited shim and renderer metadata; the old statement that every `getContext('webgl')` returns null is obsolete. Neither metadata nor a working clear/readPixels subset establishes complete GPU emulation.

A single stable profile is used by default. Rotation is opt-in:

```bash
OBSCURA_PROFILE=2 obscura serve          # pin a specific profile by index
OBSCURA_ROTATE_PROFILE=1 obscura serve   # random profile per browser context
```

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
  --stealth \
  --proxy http://user:pass@proxy.example.com:8080
```

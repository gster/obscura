# Primp and wreq comparison

For the current Southwest business result and the limits of a matched
transport probe, see [Southwest search comparison](Southwest-search-comparison.md).

Source review: 2026-09-15. Scope: Chrome 152 on macOS, TLS signatures and trust-anchor extension, and integration into Obscura. This note separates source facts, supplied capture evidence, and recommendations. The source-review sections precede the integration work; the implementation status below records the subsequent changes.

## Conclusion

Primp supplies a newer Chrome preset and real ML-DSA verification support. It does **not** exactly reproduce the observed Chrome 152 ClientHello: its ALPS protocol payload deliberately differs, and its trust-anchor list is empty. Use it as a separately identified transport option if justified by tests; do not describe it as full Chrome TLS equivalence.

Keeping wreq is technically possible. Trust-anchor transmission already exists in its pinned BoringSSL and needs Rust API exposure. ML-DSA needs a TLS implementation update, not just a new User-Agent or signature-list string. The two gaps should be treated independently.

## Pinned sources

| Component | Reviewed version and immutable revision |
|---|---|
| primp | Source version 2.0.1, [84c1c45](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/Cargo.toml) |
| primp TLS | In-tree primp-rustls 0.23.43 with AWS-LC and ml-dsa features, [dependency declaration](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/Cargo.toml#L94) |
| wreq | 6.0.0-rc.29, [18436d1](https://github.com/0x676e67/wreq/blob/18436d1f7ddfdd5cc4dd16049970cee96d70540e/Cargo.toml) |
| wreq-util | 3.0.0-rc.12, [f43b415](https://github.com/0x676e67/wreq-util/blob/f43b41539d3ded0701c06a74264db0c8e7e63bcf/Cargo.toml) |
| btls / btls-sys | 0.5.6, [4edbf5d](https://github.com/0x676e67/btls/blob/4edbf5d716ba014384569ac5c631cea83827abfc/btls/Cargo.toml) |
| Vendored BoringSSL | [91a66a5](https://github.com/google/boringssl/blob/91a66a59b6c1435120ff83e245d7719411294386/include/openssl/ssl.h), identified by btls's pinned submodule |

The wreq/btls revisions were read from installed crates' `.cargo_vcs_info.json`. Primp was inspected in a separate temporary clone. The source commit is not proof that every published Python wheel was built from that identical commit.

## Static facts

### Chrome 152 and macOS

Primp's ChromeV152 + MacOS preset uses the frozen Macintosh/Intel Mac OS X 10_15_7 User-Agent and Chrome/152.0.0.0. It emits the Chromium 152, Not?A_Brand 24, Google Chrome 152 brand sequence. OS selection changes headers; the TLS emulator is selected by Chrome major version. [Preset construction and UA](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/imp/chrome/mod.rs#L26), [brands](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/imp/chrome/mod.rs#L202), [TLS emulator](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/imp/chrome/mod.rs#L268).

Primp uses an in-tree Rustls fork with AWS-LC cryptographic operations, rather than wreq's btls/BoringSSL handshake implementation. Its dependency explicitly enables `ml-dsa`. [Dependencies](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/Cargo.toml#L94), [feature definition](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp-rustls/rustls/Cargo.toml#L25).

### Signature algorithms 2308, 2309, 2310

These decimal identifiers are 0x0904, 0x0905 and 0x0906: ML-DSA-44, ML-DSA-65 and ML-DSA-87. They are not RSA-PSS schemes. Primp prepends all three to its eight classic Chrome schemes for Chrome 150 and later. [Identifiers](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp-rustls/rustls/src/enums.rs#L522), [ordered list](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp-rustls/rustls/src/crypto/emulation/mod.rs#L133).

Primp also selects the extended verification provider for emulators advertising ML-DSA. Its AWS-LC/WebPKI mapping includes all three schemes. This is more than arbitrary ClientHello advertising. Successful end-to-end negotiation with an ML-DSA certificate was not tested here. [Provider selection](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/impersonation.rs#L182), [verification mappings](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp-rustls/rustls/src/crypto/aws_lc_rs/mod.rs#L235).

The pinned wreq-util Chrome TLS list contains only the eight classic schemes. wreq forwards its string through btls `set_sigalgs_list` into BoringSSL. The reviewed pinned BoringSSL TLS signature implementation has no ML-DSA entries. Adding the three names to application configuration therefore does not supply their implementation. A backend update or a substantive TLS patch would be required; a safe change must include verification support for advertised schemes. [Chrome list](https://github.com/0x676e67/wreq-util/blob/f43b41539d3ded0701c06a74264db0c8e7e63bcf/src/emulate/profile/chrome/tls.rs), [wreq forwarding](https://github.com/0x676e67/wreq/blob/18436d1f7ddfdd5cc4dd16049970cee96d70540e/src/tls/conn.rs#L403), [btls wrapper](https://github.com/0x676e67/btls/blob/4edbf5d716ba014384569ac5c631cea83827abfc/btls/src/ssl/mod.rs#L1988), [BoringSSL signature implementation](https://github.com/google/boringssl/blob/91a66a59b6c1435120ff83e245d7719411294386/ssl/ssl_privkey.cc).

### Extension 51764 / 0xCA34

Primp adds this extension for Chrome 152 and later with the two-byte body `00 00`, representing an empty trust-anchor list. This is static and does not derive the list from the local root store. [Payload helper](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp-rustls/rustls/src/client/hs.rs#L821), [extension insertion](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp-rustls/rustls/src/client/hs.rs#L1100).

Pinned BoringSSL already has `SSL_CTX_set1_requested_trust_anchors` and `SSL_set1_requested_trust_anchors`. Input is a sequence of nonempty, one-byte-length-prefixed IDs; an empty list still sends the extension. It affects what is advertised, not certificate verification. The reviewed btls safe wrapper and wreq TlsOptions do not expose this configuration. Porting this feature therefore means exposing an existing C API through btls and wreq, without replacing the transport. A captured list should not be treated as a timeless, universal Chrome constant. [C API contract](https://github.com/google/boringssl/blob/91a66a59b6c1435120ff83e245d7719411294386/include/openssl/ssl.h#L3159), [serializer](https://github.com/google/boringssl/blob/91a66a59b6c1435120ff83e245d7719411294386/ssl/extensions.cc#L2662), [wreq options](https://github.com/0x676e67/wreq/blob/18436d1f7ddfdd5cc4dd16049970cee96d70540e/src/tls.rs#L180).

### ALPS and request semantics

Primp deliberately encodes its ALPS protocol as bytes `c9 bb 32` instead of `68 32` (ASCII h2). Its source explicitly says this prevents servers negotiating ALPS because the Rustls path cannot handle the response. Matching an extension ID or JA4 cannot establish matching extension contents or negotiation behavior. Do not copy this workaround into wreq. [ALPS construction](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp-rustls/rustls/src/client/hs.rs#L1093).

The Chrome150/151/152 preset also unconditionally inserts `sec-purpose: prefetch;prerender`. Obscura's ordinary navigation and resource requests must retain their actual semantics; this preset header should not leak into them. Primp's navigation-shaped sec-fetch defaults also require replacement when the engine already computes request-context headers. [Preset defaults](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/imp/chrome/mod.rs#L60).

The safe public Rust API is to edit the finished client's default headers before sharing it:

```rust
let mut client = primp::Client::builder()
    .impersonate(primp::Impersonate::ChromeV152)
    .impersonate_os(primp::ImpersonateOS::MacOS)
    .build()?;
client.headers_mut().clear();
// Existing Obscura request policy supplies identity, fetch metadata,
// cookies, referer, language and other headers per request.
```

Alternatively assign a complete, reviewed default map through `*client.headers_mut() = headers`. Removing only `sec-purpose` is sufficient only when all other defaults are intentional. Builder `default_headers` merges values, and impersonation applies preset defaults during `build`, so an empty builder map does not remove them. Mutating an individual Request before `execute` is also insufficient to suppress a missing header which the client later fills from defaults. [Public client mutation](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L2739), [merge behavior](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L1302), [preset application](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/impersonation.rs#L29).

## Capture evidence supplied by the coordinating task

The task's disposable local TLS/H2 capture was inspected separately from source review. It recorded one Chrome, one primp and one Obscura/wreq connection, with normalized GREASE-sensitive fields. No capture artifacts are committed here.

| Observed field | Chrome 152 | primp Python 2.0.1 | Obscura wreq before backend changes |
|---|---|---|---|
| Signature list | Three ML-DSA schemes then eight classic | Same list | Eight classic only |
| Trust anchors | Nonempty list | Empty list | Absent |
| ALPS body | `0003026832` | `000403c9bb32` | `0003026832` |
| H2 SETTINGS and initial WINDOW_UPDATE | Reference | Same captured values | Same captured values |
| Extra sec-purpose | Absent | `prefetch;prerender` | Absent |

The accompanying primp website probe recorded HTTP 200, a ZIPAIR page title and a 691667-character response, with its blocked flag false. This supports that individual request's success. It does not prove causal attribution to ML-DSA, universal site acceptance, full rendering, or identical transport behavior. The coordinating task owns live comparisons and implementation validation.

## Integration recommendation

A separate opt-in MacChrome152 primp transport can be evaluated while preserving the Windows wreq path. Keep Obscura's resolver/SSRF gate, redirect policy, cookies, interception, response limits, and per-request headers as the authority; adopting a preset must not bypass them. Clear primp defaults after building. Existing functionality must be exercised through the actual Rust integration, since the Python probe does not validate that path.

If the objective is exact TLS fidelity, primp alone does not finish it. The remaining ALPS and nonempty trust-anchor differences need protocol-level work and fresh captures. Updating BoringSSL plus exposing its trust-anchor API is a separate viable direction, but its build, compatibility, performance and verification costs were not measured in this review.

## Follow-up: isolation, DNS, redirects, cookies and replay

The following are source findings for the same inspected commit. They are not a passing integration test.

| Control | Result and limitation |
|---|---|
| `.no_proxy()` | Clears configured proxies and disables automatic system proxy use. Apply before adding the explicitly selected session proxy, otherwise it removes that proxy too. [Implementation](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L1549). |
| `.redirect(primp::redirect::Policy::none())` | Disables automatic redirect following. Obscura can retain its own validation and method/body transition rules. [None policy](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/redirect.rs#L50), [disabled redirect service](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/redirect.rs#L276). |
| `default-features = false`, no cookies feature | Cookie middleware is absent. Even when the feature is enabled elsewhere through Cargo feature unification, the client initially has no cookie store. Do not enable `cookie_store` or `cookie_provider`; keep explicit Cookie/Set-Cookie handling in Obscura. [Feature guards and defaults](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L426), [middleware](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L1123), [cookie API](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L1310). |
| `.retry(primp::retry::never())` | Disables the outer Tower retry policy. It does not disable a separate H2 REFUSED_STREAM replay loop. [Retry API](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/retry.rs#L52). |
| `.dns_resolver(custom)` | Hooks DNS resolution, but is **not by itself a complete SSRF guard** in this source revision. See below. |

### Custom resolver bypass in the hosts layer

`build()` unconditionally wraps the configured custom resolver inside `HostsFileResolver`. Once its process-global hosts cache is loaded, a matching `/etc/hosts` name returns those addresses directly without invoking the inner resolver. Thus a public-looking hostname mapped to a private address can bypass an IP-denying custom resolver. Explicit `resolve` / `resolve_to_addrs` overrides also take precedence over the custom resolver. There is no public option in the reviewed builder to disable the hosts wrapper. [Resolver assembly](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L519), [hosts early return](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/dns/hosts.rs#L119), [override precedence](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/dns/resolve.rs#L163).

For direct connections, a narrow upstream/vendor correction can preserve the custom resolver as the complete resolution authority when explicitly supplied, while keeping hosts support for the default resolver. IP literals must still be validated at Obscura's URL gate, and any application-provided override addresses must be checked. Merely setting DNS cache TTL to zero does not eliminate the hosts bypass.

HTTP CONNECT and remote-DNS SOCKS proxy paths have another boundary: the local resolver sees the proxy endpoint, while the destination hostname can be resolved by the proxy. Client-side DNS filtering cannot prove the final destination IP in that mode. Private proxy endpoints should be treated as explicitly authorized transport infrastructure separately from private destination authorization; granting general private-network access just to reach a local proxy would widen the policy. [CONNECT proxy resolution](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/h2_client/connect.rs#L247), [CONNECT target construction](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/h2_client/connect.rs#L454), [SOCKS local versus remote resolution](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/connect.rs#L1069).

### Hidden H2 replay and request-body behavior

The H2 pool hardcodes `MAX_REFUSED_RETRIES = 32`. When the remote peer resets a stream with REFUSED_STREAM and the body is cloneable, the pool opens another stream on the same connection and replays the request. This includes POST with buffered bodies; there is no method gate or public disable switch in this loop. The source invokes the protocol guarantee that such a stream was not processed, so this is not arbitrary replay after an uncertain transport failure. Still, an integration must not advertise zero implicit POST retries after setting `retry::never()`. A stricter no-replay requirement needs a pool-level change or explicit configuration propagated to that loop. [Retry loop](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/h2_client/pool.rs#L957), [exact replay condition](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/h2_client/pool.rs#L1143).

`RequestBuilder::body` directly stores the supplied body; it does not select form or JSON serialization. Default headers are filled for missing request headers at execution time, which is why clearing client defaults after building is necessary. With redirect following disabled, the redirect layer does not silently turn POST into GET. Use raw `.body(...)` when Obscura already owns encoded bytes. [Body setter](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/request.rs#L291), [default insertion](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L2857).

If Obscura must own compressed bytes and decompression, use `.no_gzip().no_brotli().no_deflate().no_zstd()` before build. These controls exist even when their respective Cargo features are absent; preserve the intended Accept-Encoding header separately. If primp instead owns decompression, response headers and body-size enforcement must be checked on the decoded stream. [Decompression controls](https://github.com/deedy5/primp/blob/84c1c45d614d2085192826409517e813ecf461f1/crates/primp/src/async_impl/client.rs#L1445).

### Focused vendoring option

The published `primp-2.0.1.crate` was separately inspected: 305323 compressed bytes, 90 files, and 1520391 uncompressed file bytes. Its recorded source revision is `cf29cd94f339c1a69ec7f271f532f0f2411aa06e`. Its client constructor and hosts resolver are byte-identical to the reviewed HEAD. Its normalized Cargo.toml uses registry primp-rustls/primp-h2 dependencies, so patching only this published HTTP-client crate does not require vendoring the whole repository. [Published source revision](https://github.com/deedy5/primp/blob/cf29cd94f339c1a69ec7f271f532f0f2411aa06e/crates/primp/Cargo.toml), [constructor](https://github.com/deedy5/primp/blob/cf29cd94f339c1a69ec7f271f532f0f2411aa06e/crates/primp/src/async_impl/client.rs#L519).

Enabling hickory-dns does not avoid the hosts wrapper. The smallest source correction is to replace the unconditional two-line wrapping with:

```rust
base = config.dns_resolver.unwrap_or_else(|| {
    Arc::new(crate::dns::hosts::HostsFileResolver::new(base))
});
```

This preserves hosts handling for the default resolver and makes an explicitly supplied resolver authoritative. Existing per-client cache and explicit override behavior remain; Obscura must continue to avoid unvalidated overrides. Validate this with a focused hosts-entry/custom-deny-resolver regression, plus a normal-resolution check. Preserve the existing explicitly selected proxy behavior and its existing remote-DNS trust boundary rather than prohibiting proxy use as a side effect of this patch.

## Implemented integration

The SDK macos_chrome152 profile now uses primp 2.0.1 ChromeV152/MacOS.
Windows Chrome 145 uses primp ChromeV145/Windows as well. The transport leaves
Obscura response limits, redirects, cookies, interception and CORS in the shared
policy layer. Navigation, subresources and fetch/XHR use that same client.
Primp defaults are cleared after building, so ordinary navigation does not gain
the preset's sec-purpose prefetch header. H1 fields are ordered explicitly; H2
uses the impersonation preset's order.

The vendored primp patch makes an explicit DNS resolver authoritative. A seeded
process-local hosts-cache regression failed before the patch and passed after it.
A separately configured proxy endpoint is permitted without enabling private
destination URLs; a local CONNECT fixture verifies this distinction. The remote
proxy's destination resolution remains the existing trust boundary.

Local TLS capture with the integrated runtime confirms matching cipher suites,
key-share groups and lengths, signature algorithms, H2 SETTINGS, connection
WINDOW_UPDATE, and navigation header values/order against headed Chrome
152.0.7977.83. ALPS and trust-anchor payloads still differ as documented above.
Random bytes, GREASE values and extension permutations are normalized, not
asserted byte-identical. No claim of complete fingerprint equivalence is made.

The integrated runtime received HTTP 200 from the ZG homepage through the
explicit local port-7890 proxy. A diagnostic whole-body DOM text query then
exceeded the NDJSON limit because textContent includes scripts. Oversized SDK
text/value/attribute reads now report VALUE_LIMIT without truncation or session
termination, and text assertions return only their success status. HTTP success
alone does not establish checkout or rendering acceptance.

The first live script comparison exposed an engine routing bug: parser-loaded
external classic scripts used the ordinary client even when the page was in
stealth mode. They now share the page's stealth client. A local regression
failed before the change and passes afterward; ZG's Nuxt script responses
changed from 403 to 200 through the same proxy. The page then shows its own
error dialog, so live booking acceptance remains incomplete.

The extended TLS fixture also exercises an external classic script. Its UA and
Client Hints match, but subresource header order and the Priority header still
differ from Chrome (the static primp order/default is navigation-oriented).
Navigation header equality must not be generalized to every resource class.

Cross-origin preflight was a second ordinary-client escape. OPTIONS requests
now use the selected stealth transport, remain credential-free, do not follow
redirects, and still pass the existing origin/method/header checks. Repeated
response fields are merged instead of silently taking the last value. A local
SDK test reproduces the missing preflight identity before this change. After
the change, ZG's routes and recommended-flights APIs return HTTP 200 and the
initial departure city appears. An application error dialog remains; console
diagnostics also show the carousel's updateSize accessing an undefined $el.
The relationship of that exception to every remaining UI failure is not yet
proven. Neither homepage responses nor fixture success count as live checkout
acceptance.

A subsequent native-input attempt located the error dialog's OK button but
returned INPUT_GEOMETRY_UNSUPPORTED before dispatch. The existing scroll
preparation deliberately rejects non-translation transforms. This remains an
input/geometry compatibility limitation; the implementation does not replace
it with JavaScript click or force the page past an uncertain input state.

A headed-Chrome control restricted requests to the same three business origins
(www.zipair.net, bff.zipair.net and images.zipair.net), through the same local
proxy and at the same 1280x900 viewport. It still loaded the departure city and
returned both BFF APIs successfully, with no application error dialog. Thus the
remaining Obscura error is not explained solely by excluding third-party origins.

## Unified transport

Both supported profiles now use primp. wreq, wreq-util and their BoringSSL
dependencies are removed. The public Rust `wreq_client` module path remains an
alias for `stealth_client` for source compatibility, without a wreq backend.
The old extra GET connection-reset retry is removed; Obscura does not replay
requests after connection reset. Primp protocol-level recovery still applies.
This migration does not close the ALPS, trust-anchor or resource-header gaps above.

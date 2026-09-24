# Local patches to `primp-rustls` 0.23.43

This crate is vendored and wired up via `[patch.crates-io]` in the root workspace
for the local ClientHello profile and ALPS handshake fixes. The dependency's
Apache-2.0 / ISC / MIT licenses are retained (`LICENSE`, `LICENSE-APACHE`,
`LICENSE-ISC`, `LICENSE-MIT`).

The manifest declares an independent workspace, mirroring `vendor/primp`, so the
crate is not absorbed into the host workspace. The registry package omits
upstream `testdata` and `test-ca` files required to compile its standalone
unit tests; compile it and run the Obscura network and integration gates:

```sh
cargo check --release --manifest-path vendor/primp-rustls/Cargo.toml --lib
cargo nextest run --release -p obscura-net
```

## 1. ClientHello extension order is permuted per connection

**File:** `src/client/hs.rs` (call site in `emit_client_hello_for_retry`,
helper `permute_order_insensitive_extensions`), plus `src/msgs/handshake.rs`
where `low_quality_integer_hash` is widened to `pub(crate)` so the helper can
reuse the crate's own permutation primitive.

**Problem.** `emulator_extension_order()` returns one order "taken verbatim from
real captures" per browser version. Setting that as `contiguous_extensions`
makes every connection emit a byte-identical extension list forever.

Real Chrome does not do that. It permutes the order-insensitive ClientHello
extensions on every connection. Measured against Chrome 153 on one machine,
comparing two connections that *share a profile* (so a per-profile shuffle is
ruled out):

```
origin A (tls.peet.ws)          ja3 c60481f682f12c7327a9489fb1acd38f
  35-51764-65037-16-17613-45-23-43-65281-27-13-0-11-51-18-10-5
origin B (tls.browserleaks.com) ja3 6b80fa5f8e213ef772761a7efeb3b9d9
  51-17613-18-65281-11-5-13-27-23-45-43-10-0-35-51764-16-65037

ja4  identical in both: t13d1517h2_8daaf6152771_cb7bf5808d99
```

Same 17 extensions, same JA4, different JA3. A fourth independent connection
produced a third order. Unpatched primp, by contrast, emitted the *same* JA3
across six consecutive connections. A constant extension order is therefore
itself a fingerprint — the opposite of what browser emulation is for.

Note the crate's own `choose_extension_order_seed` doc comment already states
that a "random per-connection seed ... match[es] real Chrome's per-connection
variation"; the pinned list contradicted it.

**Fix.** Keep the captured list, but reorder its interior with a fresh
per-connection seed:

- slot 0 and the final slot are left alone, because `emit` substitutes the
  GREASE placeholders positionally *after* this runs;
- `pre_shared_key` and `encrypted_client_hello_outer_extensions` keep their
  position (RFC 8446 requires `pre_shared_key` to be the last extension);
- everything between is permuted using `low_quality_integer_hash` over
  `(seed << 16) | extension_type` — the same primitive the crate already uses
  for its seed-randomized ordering.

**Verified after the patch** (5 consecutive connections to `tls.peet.ws`):

| | result |
| --- | --- |
| distinct JA3 values | 5 of 5 |
| JA4 | constant `t13d1517h2_8daaf6152771_cb7bf5808d99` |
| JA4_r | constant |
| extension count | 19 (17 non-GREASE + 2 GREASE) |
| GREASE position | first and last, every connection |

`emulator_extension_order` itself is untouched, so the crate's per-version
golden tests still assert the captured orders.

## 2. Chrome 153 ALPS and Trust Anchor IDs

**Files:** `src/client/hs.rs`, `src/client/tls13.rs`,
`src/msgs/{enums,handshake}.rs`, and `src/common_state.rs`.

The Chrome 153 persona offers `h2` in extension `0x44cd` as the five-byte value
`00 03 02 68 32`. The previous profile emitted a six-byte non-`h2` name,
avoiding server negotiation because rustls did not support the client
EncryptedExtensions message. The TLS 1.3 state machine now retains peer ALPS
settings and sends the empty client ALPS extension in EncryptedExtensions
before Finished when the server negotiates ALPS. The HTTP/2 caller reads the
authenticated settings after the handshake.
Other personas retain their previous ALPS bytes until checked against a
matching reference browser build.

Chrome 153's `0xca34` payload is a 186-byte TrustAnchorIDList. Its 28 IDs
were matched as a set against Chromium's 153.0.8010.50 root store. Chrome
shuffles them once per process and keeps the order across connections, so the
profile follows that process-level behavior. Other browser
versions keep their previous payloads pending matching reference captures.

## Not patched

- TLS versions, cipher suites, named groups, signature algorithms, and ECH
  contents.
- Certificate verification, crypto implementation, session resumption, retry
  or redirect policy.

## Known remaining fidelity gap

JA4 hashes extension *types* only, not extension *bodies*, so equal JA4 does not
imply byte-identical ClientHellos. The bodies of `key_share`,
`encrypted_client_hello` have not been compared byte-for-byte against a real
Chrome capture. The TrustAnchorIDList reflects a Chrome profile; it does not
replace rustls certificate verification or prove that Obscura trusts the same
root store as Chrome.

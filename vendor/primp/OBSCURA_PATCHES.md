# Local primp patch

Base: published primp 2.0.1, upstream revision cf29cd94f339c1a69ec7f271f532f0f2411aa06e.

The client builder now leaves an explicitly configured DNS resolver authoritative.
Upstream wrapped it with HostsFileResolver, which returned hosts-file entries
without calling Obscura's SSRF resolver. The default resolver remains hosts-aware.
Regression: dns::hosts::tests::explicit_resolver_validates_cached_hosts seeds a
process-local hosts entry and verifies that a deny resolver runs before any
connection. Run with nextest so this global-cache test has its own process.
The obscura-net macos_transport_rejects_localhost test also checks direct
loopback rejection.

No TLS fingerprints, certificate verification, crypto implementation or request
retry policy are patched. See docs/Primp-and-wreq-comparison.md for known fidelity
limits. The dependency's MIT license is retained.

The manifest declares an independent workspace for focused dependency testing:

```sh
cargo nextest run --release --manifest-path vendor/primp/Cargo.toml --no-default-features --lib -E 'test(explicit_resolver_validates_cached_hosts)'
```

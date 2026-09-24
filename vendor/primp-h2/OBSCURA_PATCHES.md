# Local patches to primp-h2 0.4.19

Vendored through the workspace `[patch.crates-io]`. The upstream MIT license
is retained. `[workspace]` keeps the crate's own tests independent from the
Obscura workspace.

`src/client.rs` parses HTTP/2 frames carried in authenticated TLS ALPS data
before the first request. It accepts SETTINGS and unknown extension frames,
rejects malformed or forbidden frames, and applies remote SETTINGS without
sending an HTTP/2 ACK. `src/proto/{connection,settings}.rs` applies those
settings to stream flow control and the send codec. Ordinary wire SETTINGS
processing remains unchanged.

The registry package omits upstream HPACK fixtures, so its full standalone
unit suite has unrelated fixture failures. Run the ALPS unit test by name and
the Obscura network/workspace gates.

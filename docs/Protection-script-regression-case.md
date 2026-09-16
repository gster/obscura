# Protection script regression case

This case records a reduced, host-neutral version of the browser checks found
while debugging an airline shopping workflow. It is intended to catch browser
surface regressions without committing a third-party minified script, cookies,
request tokens, or a site-specific bypass.

The executable case is
`southwest_style_protection_probe_remains_coherent` in
`crates/obscura-js/src/runtime.rs`.

## What the original failure exposed

The site protection layer creates a Blob worker and uses messages shaped like
`{n, s, w, b}` for a private bootstrap protocol. It installs listeners through
`EventTarget.prototype.addEventListener.call(...)`, then replaces
`Worker.prototype.onmessage` with a descriptor that filters those private
messages before the application callback runs.

Obscura previously implemented worker listeners and `onmessage` as separate
paths. The private bootstrap message escaped the filter and reached the page's
application handler. The handler interpreted it as a normal response and
raised repeated type errors. The reduced test preserves the protocol shape,
including an own `b` property whose value is `undefined`, because structured
clone and property presence are both part of the check.

## Surfaces covered

The test treats the configured persona as one browser identity and checks the
following values together:

- macOS Chrome user agent, platform, locale list, hardware concurrency, device
  memory, Do Not Track, and `navigator.webdriver`;
- Chrome's five PDF plugins, their PDF MIME entries, and the reverse
  `enabledPlugin` link;
- `chrome.app`, `chrome.csi`, `chrome.loadTimes`, the absence of
  `chrome.runtime`, and `webkitRequestFileSystem`;
- browser-compatible binary-string `btoa` and `atob` semantics, including the
  `InvalidCharacterError` raised for characters outside Latin-1;
- Chrome navigator names and aliases, storage access, battery, storage quota,
  network information values, and the desktop null-valued device-orientation
  sample;
- lazy console previews that do not synchronously invoke user-defined error
  getters or function `toString` hooks used by developer-tools detectors;
- `document.createElement()` name validation, including Chromium's
  `InvalidCharacterError` for old IE-style markup passed as a tag name;
- WebGL's default 300 by 150 drawing buffer, generic WebKit vendor strings,
  persona-specific unmasked renderer strings, clear, and `readPixels`;
- SVG constructor chains, `getBBox`, `getTotalLength`, and
  `getPointAtLength`;
- Worker prototype descriptor wrapping, EventTarget dispatch order, private
  message filtering, worker global and navigator prototype identity, and
  asynchronous application replies.

A companion test,
`dynamic_classic_script_runs_after_post_insertion_callback_assignment`, records
the ordering used by the IPv6 collector: insert an asynchronous external
script, then install the callback that the fetched script invokes. This keeps
the ordering regression reproducible without storing the returned token. A
timer queued after fetch completion must observe the callback value; script
execution must not acquire a second zero-delay timer that lets the collection
deadline run first.

This companion test uses a mocked fetch operation. Its timer ordering has not
yet been checked against a controlled Chrome networking fixture, so passing
it does not establish Chromium task-order conformance or explain a live 403.

The isolated runtime separately validates startup persona defaults and rejects
locale combinations where `navigator.language`, `navigator.languages`, and
`Accept-Language` contradict one another. The effective persona is returned by
the runtime's ready response so the caller can record the identity that was
actually applied.

The name-validation change also has an independent DOM regression test,
`create_element_accepts_unicode_xml_names_and_rejects_invalid_names`. A local
fixture opened through Chrome computer use and through the isolated Python SDK
showed that an ASCII-only validator rejected valid Chinese, accented, and
supplementary-plane names. The shared validator now uses the
[XML 1.0 Name grammar](https://www.w3.org/TR/xml/#NT-NameStartChar), preserving
invalid-name errors without rejecting valid Unicode names. This DOM mismatch
has not been established as a cause of the airline's HTTP 403 response.

## Run it

Use nextest because each runtime test needs its own V8 process:

```bash
cargo nextest run --release --features render -p obscura-js \
  -E 'test(southwest_style_protection_probe_remains_coherent) or test(dynamic_classic_script_runs_after_post_insertion_callback_assignment)'
```

For a stealth change, also build and exercise the stealth transport path:

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release \
  -p obscura-cli --bins --features render,stealth
```

## Comparing a live failure

Record a successful Chrome session as HAR and compare these layers in order:

1. request URL, method, body bytes, redirect chain, HTTP version, proxy route,
   and static headers, including the fetch/XHR `priority: u=1, i` value;
2. effective startup persona and the JavaScript surfaces listed above;
3. worker bootstrap messages and whether private messages reach application
   handlers;
4. protected request status, response headers, and a hash or length of dynamic
   protection headers.

Do not commit HAR files from authenticated or protected sessions. They can
contain cookies and per-request protection values. Keep captures in a local
artifact directory and document only stable observations. A passing reduced
case proves the local browser contract; the live workflow must still return a
successful shopping response before the 403 investigation is considered
complete.

## Verification boundary

The reduced tests document the specific browser contracts above. The current
Worker implementation has separate realm support, but the September 16 review
found regressions in structured message cloning and Worker termination. Passing
the protection-protocol fixture does not establish complete Worker conformance.
See the [final Southwest repair and review record](Southwest-fix-record.md) for
current findings, reproduced failures, historical business samples, and gates
that have not passed. This fixture alone does not explain a live shopping 403.

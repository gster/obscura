# Southwest checkout comparison

## Publication: 2026-10-01

The user authorized committing the scoped changes on main and pushing them.
The source changes are separated into four commits:

- `1a45455`: iframe internals and classic-script realm execution.
- `9372812`: Cookie scope, replacement, expiry and snapshot semantics.
- `1d078cf`: reusable H2 body EOF framing and local protocol regressions.
- `2347b46`: distinct browser attachment sessions and detach coverage.

Before publication, focused release/render nextest coverage passes 107/107
with 1173 tests skipped by selection. Previously recorded gates for the same
source pass 2393 render tests with 4 skipped, both render and no-render release
builds, and the 33/33 obstacle course. These are not a claim that all no-render
tests pass: the broader no-render run retains the recorded scrolling failure.

This document is committed separately. Raw captures, generated reports and
private handover material are excluded, as are the unrelated untracked
same-document navigation and TLS-signature tests. Source publication is not a
deployment and does not complete the Southwest checkout investigation. Earlier
statements that no commit occurred describe their individual investigation arms.

## Investigation 24: scope-aware Cookie identity and Domain classification

Status: bounded generic Cookie repair passes independent source review, release
gates, matched local comparison and performance-byte audit. A bounded ordinary
site arm stops before Search on capture-helper label mismatch; checkout is
incomplete. The previous native
48-boundary control and the fully audited matched D23 arm establish the local
DNS Domain/coexistence discrepancy. This is not an established shopping HTTP403
cause. Local qualification makes no Southwest request. The subsequent bounded
site arm is described below; no card input, payment, commit or deployment occurs.

Two release regressions fail before the change: explicit exact DNS Domain does
not reach a child, and same-name/path host-only and Domain entries overwrite.
The candidate adds host_only to storage/snapshot identity and propagates it
through insertion, assignment expiry, HttpOnly checks, delta/merge and v1
persistence. The shared resolver preserves raw Domain until validation and uses
typed origin IP plus the already locked PSL, including PRIVATE rules. Matching
IP/intranet/suffix equality stays host-only; registrable exact DNS is Domain
scope; unrelated or public-suffix parent assignments reject. Request matching
does not add PSL work. No hostname exception or JS fallback is introduced.

The CDP compatibility choice is explicit: public CookieInfo shape and canonical
domain projection remain lossy, with trusted imports defaulting to Domain
scope. Imports and import expiry now target only that identity, preserving a
host-only sibling. A paired projection/reimport remains last-row-wins within
Domain scope, not a lossless roundtrip. Public bulk deletion still covers both
scopes; old version1 host_only flags are not promoted and the format is unchanged.

ASCII/punycode and single trailing-dot behavior are covered. Raw Unicode Domain
remains unsupported, not claimed as Chromium default parity: the pinned source's
non-ASCII rejection is feature-dependent. Full GURL/IDNA and scheme/port/partition
parity remain open. See the [pinned classifier](https://github.com/chromium/chromium/blob/78e5e45d4bb41035e17ea4da2cc257f496416ac9/net/cookies/cookie_util.cc#L347)
and [feature default](https://github.com/chromium/chromium/blob/78e5e45d4bb41035e17ea4da2cc257f496416ac9/net/base/features.cc#L294).

Initial focused network coverage passes 209 tests. Independent Spec review finds
a pre-existing required natural-expiry copy gap: cloning drops expired identity,
so a delta against the original snapshot can remove a concurrent refresh. Its
new regression fails before repair, and stored-identity copying closes it; reads,
projections and saving still filter expired entries. Focused coverage then passes
210 tests. Re-review finds that retained expired HttpOnly entries still block JS
recreation. Its regression fails before repair; the shared live-only guard then
protects both JS overwrite and expiry while allowing an expired target to be
recreated and preserving live protected siblings. Final focused network coverage
passes 211 tests. Final GPT6.1 Sol xhigh Standards and Spec reviews each report
zero findings. Both earlier P2 reports and their red results remain retained.
The initial full render run is interrupted for source corrections, exit130;
it is not a passing gate. The final full render run passes 2393 tests with four
skips. Affected no-render library coverage passes 137 tests, with 998 filtered
out; it is not full no-render coverage. Exact release CLI builds succeed for
both configurations. Obstacle passes 33/33.

A fresh official Playwright matched-persona arm completes and acknowledges all
48 boundaries. Independent offline verification reparses all 48 native original
POSTs and finds zero Domain/coexistence phase differences and zero differences
in all 26 per-mutation setter/target Cookie echoes. It verifies the 18 empty
first-setter scopes, 257 actual HTTP wire pairs, 514 TCP files and all 3730
protocol send/reply pairs, with zero protocol errors. Each of the three actual
pre-close/post-page-close/post-context-close history stages retains 514 records,
449 complete body copies and 919 chunks, totaling 39805296 decoded bytes.
Offsets, EOF, sizes, hashes, protocol replies and wire bodies all match. Context
history becomes finalized only after actual context close. Eight connection-
refused readiness observations before server startup remain retained; there are
no blocking capture failures. The fixed journal prefix is a live read-time
boundary, not a finalized server lifetime. Exact served fixture source is
retained, not a universal V8 source catalog or all-instruction trace.

A separately reviewed performance harness completes 24 fresh processes in six
ABBA blocks, with the same 32 Domain-only synthetic names and all 100 warmup /
1000 measured read-write-read iterations retained per process. Full stdout,
stderr, inherited environment, resource records, journal ranges and raw wire
files remain available. Independent offline verification reparses all 24 full
workloads, 24 actual Cookie-empty GET200 requests, 48 TCP files and all original
journal-prefix/range bytes. It reproduces the distributions and six block means,
checks 50 compiler/capture-idle inventories and records no timeout, signal,
forced kill or unreaped child. Kernel resource values are retained wait4
evidence, not postmortem remeasurement. The fixed CLI controls are the same
manifest URL/persona, 1280x720/DPR2, load boundary and zero settle on both arms;
this is latency-only evidence, not rendering fidelity.

| Whole CLI child metric | Old p50 / p95 | Candidate p50 / p95 | Median change |
| --- | --- | --- | --- |
| Wall, ms | 134.642 / 838.926 | 135.784 / 149.825 | +0.85% |
| CPU total, ms | 96.637 / 121.240 | 97.755 / 107.970 | +1.16% |
| Max RSS, MiB | 49.844 / 51.547 | 49.609 / 51.000 | -0.47% |

The medians lie within the approximate 10% noise floor. Nearest-rank p95 with
twelve process samples is the maximum. The first baseline wall outlier remains
in the distribution and block means, without an asserted cause or exclusion.
All process medians of JS write/total time are zero at integer-millisecond
resolution, with 0/1ms observations; percentage changes are undefined, not zero
cost or a speedup. Whole-child metrics include startup, serialization, full
stdout/raw fixture IO and up to one 10ms exit-observation interval. The workload
does not verify outgoing HTTP Cookie headers after writes, all-site speed or
shopping rejection causality.

The ordinary-site retest remains pending a bounded, separately reviewed capture
helper. The old helper stops after search and has source/history/cleanup gaps;
its outputs cannot certify the credit-card boundary. Source preparation fixes
four capture gaps before execution: rejected shopping now stops business
actions, lost/unreaped child ownership is an explicit qualification failure,
readiness validates a complete bounded HTTP response, and the dead Faker branch
is removed. Response-header reads and active execution are bounded; final
capture and cleanup have separate deadlines. Physical Playwright protocol logs
and exact owned-listener evidence are required. Fare-rule acceptance requires
trusted human approval of the exact current quote snapshot, not an AI review
flag. These are capture-helper corrections, not a product fix or site result.
Independent source review then finds two P1 action-safety gaps: a stored fare
row/label is not rebound to the live list immediately before selection, and the
No seat selected/$0 condition is not rechecked immediately before continuation.
Site execution is withheld until correction and independent re-review. The
corrected helper binds the fresh full flight list/quote and exact pinned target
before selection or continuation, and rechecks the exact no-seat/$0 state before
continuation. Nineteen offline mocked guard cases pass, without a browser,
request or click; they qualify Python guard predicates, not actual JS geometry
or atomic server-side quote semantics. Final independent source review reports
zero Spec or Standards findings. No site action is sent by the source reviews.

Main then starts one fresh bounded delivery arm. The booking document returns
HTTP200 and the snapshot confirms LGA to MDW, 27 October 2026, one adult, USD,
1365x768/DPR2, Mac153/en-US/Asia-Shanghai. The visible controls read Dismiss and
Search flights, unlike the helper's assumed Accept All Cookies/Search labels.
Main stops with the allowlisted finish command before any click. There is no
shopping POST, fare selection, passenger input or card boundary. The helper
reports zero capture failures and complete three-phase history/body copies;
each phase retains 308 context records and 192 body copies, with finalization
only after context close. Own PID exits0 after TERM; controller exits1 because
the business goal is not reached. Independent runtime byte audit remains
pending. Original scripts, snapshots, screenshots and full raw logs are retained.
Only the two fixed expected labels are then corrected in the capture helper.
Independent source review reports zero findings for that exact two-literal
delta; all action/capture/ownership guards remain unchanged. Dismiss is not
asserted to mean acceptance of all optional Cookies; this label mismatch is not
a product defect.

Main separately authorizes a new unused bounded arm, with the same delivery,
persona, proxy and viewport. It completes ordinary Dismiss then exactly one
Search flights click. FIRST own shopping is fetch-130, native pending/final
sequences372/391, HTTP403. The site itself subsequently generates fetch-154,
sequences392/407, also403; main sends no repeated Search or other business action.
The retained terminal rejection projection points to the later fetch-154, so
FIRST is derived from raw chronology, not that last-updated field. Main decodes
the FIRST JS and transport body chunks and independently recomputes both hashes:
355 bytes, `b05659a296020be81bafa58ba1491e28acd8bdd715410b32feb088216a0f9689`,
matching the historical Chrome200 request body. This does not establish actual
H2 fields, Cookie equality or rejection causality.

The arm reports zero capture failures and complete three-phase body copies;
pre-close context history has656 records,417 copies,999 chunks and43518636
decoded bytes. History identity is selected from inventory, not its unstable
ordinal. Independent complete raw/protocol audit remains pending. Own PID exits0
after TERM and controller exits1 because checkout is incomplete. No fare,
passenger, card or payment action occurs. The local Cookie fix is qualified, but
this ordinary-site rejection remains unresolved; no automatic retry is made.

Source preparation of that independent audit identifies two read-time facts
which must not be collapsed: the initial snapshot000 is an early shell without
the Dismiss control, while the immediate pinned click guard contains it; and
the finalized history grows to680 records/435 body references, rather than
remaining the656-record pre-close snapshot. The audit checks immutable shared
prefixes and retained appended records, not repeated concatenated histories.
FIRST pending headers are scriptRequest metadata; final headers are distinct
transportRequest metadata. They must be retained and compared at their actual
stages, never declared identical or promoted to actual H2 wire evidence.

The first explicit offline audit exits1 and is retained, not overwritten. Its
3086-file before/after manifest matches exactly, but full raw qualification
fails while pairing Network.getResponseBody for page-1.9, operation106. A
read-only independent join confirms the correct physical session and command;
the stored and physical-log result strings differ by21 characters. Both raw
origins remain unchanged. The failed report SHA256 is
`736a1747d62271971dd7fa5ce2a13984570c9bb10145852d9fd504f4a9d50706`.
This is an unresolved protocol/body representation check, not evidence that
shopping succeeded or that403 has a known cause. No audit retry or new site
request follows this failure automatically.

Read-only diagnosis identifies installed Playwright1.60 debug-format
substitution, not response-byte corruption. All3526 physical command/reply
pairs parse, all2123 stored events match, and3396 stored CDP results comprise
3388 exact matches plus8 Network.getResponseBody differences. All8 match the
logger's forward percent-format substitution, with no unexplained difference;
this lossy model does not recover original bytes or certify an audit pass.
The original failed audit remains unchanged. Complete structured/native body
checks remain required separately.

Current delivery has no accepted-H2 observer, so prepared headers cannot stand
for a new actual FIRST wire capture. An isolated diagnostic release build
succeeds after three observer tests pass. The temporary framed-write patch is
reversed exactly; all preserved dirty-source hashes, HEAD and empty index still
match. Workspace and immutable delivery binaries remain unchanged. This
diagnostic has not made a site request or qualified new actual H2 evidence;
decoder/history adaptation and an independently approved bounded arm remain
required.

Main fully reads the447-line offline decoder adapter and adds a missing local
request-end qualification: matching request bytes alone does not prove an
observed END_STREAM boundary. The check requires one final DATA END_STREAM for
a nonempty POST, rejects earlier ends followed by DATA, pre-end resets and
extra HEADERS blocks, and handles empty-body HEADERS/empty-DATA closure. All18
AST-only frame-metadata guard tests pass. Their complete private result SHA256
is `2ad92eb09f2cfb152a4a802a565125f83e2762b8f57fd46f51cfde4addacdfa1`.
These tests execute neither HPACK nor the full decoder and make no browser or
network request. Local outbound closure is not evidence of peer receipt. The
final decoder still requires independent source review and a new own-session
diagnostic capture before actual-wire qualification.

The separate search-only diagnostic helper passes independent source review
with0 Spec and0 Standards findings. After main's explicit GO and inspection of
the actual controls, one fresh arm performs Dismiss and Search once. FIRST
fetch-142 is HTTP403, native pending/final sequences407/433. Its full returned
response is23 bytes, SHA
`a794934770bcfb52e802431580b24f616ccd1258268142256ba8a043750dbc4f`.
The helper reports0 failures, complete three-phase copies and62 observer files
(31 raw/journal pairs), no warning, and known own PID17801 wait4exit0 after TERM.
Capture-controller exit0 qualifies only its search diagnostic boundary, not
checkout. No fare, passenger, seat, card or payment action occurs. Result SHA
is `9437f4a362c7725faaa43a014c717ede0a494bc15df5f071f41046336c2a1499`.
The own context has762 pre-close records/486 body references and784 later
records/507 references, finalized only after context close. Actual H2 values
remain unqualified until the independent offline decoder completes.

Decoder source review finds2 Spec P2 binding gaps, Standards0: captured
page-session FIRST and completed-capture observer-file pins were not required.
Main preserves the447-line source and original review, adds those mandatory
bindings in a503-line version, and passes20 synthetic AST-only guard tests.
These tests import no HPACK, execute no decoder main and read no real capture.
Their private result SHA is
`2fab5ffea055288233838c001dd0774138bd25c180ab155615f5a25d76343295`.
Fresh independent source approval remains mandatory before any full decoding;
no automatic new Search or retry is used to address these offline-tool gaps.

The503-line decoder re-review closes both P2s with0 Spec/0 Standards findings.
One explicitly authorized strict offline run then exits1, retained unchanged:
all62 files match the completed capture inventory; all31 observer journals are
healthy and all connections parse completely, with3409 input hashes unchanged.
However, the website generates four HTTP403 shopping responses and a fifth
shopping request, fetch-195, which ends status0/Aborted with no response body.
That later request fails the strict all-shopping completion requirement. The
error text says FIRST chronology, but this is not a failed HTTP response for
the actual FIRST fetch-142. Metadata SHA is
`3e63c71eaebe4d38aacf7e88a49e50ff7324b9b334eef8a9d4ec8d31adcbe783`.
No strict audit pass or whole shopping lifetime qualification is asserted.

A separate FIRST-only PARTIAL report is proposed, explicitly narrowing the
qualification scope without changing the strict source/failure/raw data. It
must keep all global evidence checks, unique actual-FIRST correlation, full
body/captured-FIRST bindings and END_STREAM checks, while retaining every
later request/stream and leaving status0/Aborted unqualified. Its preparation
does not authorize execution or another Search; main full read and independent
source review remain required before an offline GO.

Independent review of its294-line preparation finds2 Spec P2s and0 Standards
findings: unknown actual shopping ownership could coexist with a qualified
native FIRST, and final input recheck failure left a stale global-health flag.
Main preserves that source/plan/review, then makes a296-line revision that
conservatively blocks FIRST on any unresolved actual mapping and initializes
and clears global health on evidence-check failure. Native chronology remains
distinct from cross-connection accepted-write chronology. Both independent
review axes then report0 findings, but the one offline attempt exits1 during
Python parsing because of a missing dictionary-key quote. No source statement,
capture input read, output directory or actual decode occurs. The failed source,
source reviews and empty stdout/error stderr are preserved. Main fixes only
that quote and passes an explicit AST/compile-only grammar check, without source
execution/import/cache/network. Fresh independent Spec and Standards reviews
approve only the corrected source with0 findings and acknowledge the earlier
syntax miss. A separate offline GO uses new logs/output and exits0, with no
new Search. Its overall result is still PARTIAL, not the earlier strict pass.

The separate FIRST flag qualifies fetch-142, native sequences407/433 and actual
H2 stream239, HTTP403. All five native/actual shopping streams map uniquely;
every later row remains UNQUALIFIED, including fetch-195 status0/Aborted.
All31 connections/62 observer files and three-phase body histories are retained;
the final3417 input pins, including all3409 prior pins, remain unchanged. The
strict failed receipts remain original. Partial metadata SHA256 is
`47981abffea5b8c61d819011055795ad50e5ca369dc4e641ad3b4c128d1cd43a`.
Exit0 certifies only generation of this FIRST-qualified partial evidence, not
the whole shopping lifetime, receipt by the peer, checkout or performance.

FIRST's355-byte request body, pseudo-header order, HEADERS priority and single
355-byte DATA END_STREAM match the pinned historical Chrome200. Ordinary header
order still differs. Obscura sends one Cookie field containing29 items; Chrome
sends28 separate fields/items. The only safe standard-value difference is
Accept-Language: current `en-US` versus Chrome `en-US,en;q=0.9`. The current
wire matches the explicitly pinned persona's `accept_language: en-US`; the
helper does not supply a Playwright locale override. This is a comparison-input
difference, not evidence of a new product-language bug. Fresh generic control
alignment and provenance analysis remain separate work; no header/Cookie value
is replayed, no hostname-specific repair is made, and none of these differences
establishes the cause of HTTP403.

Read-only language-origin analysis confirms zero locale/UA override commands
in this arm. D21 used a different recorded persona hash and its retained JS
snapshots expose languages `[en-US,en]`; its exact historical persona literal
remains unverified. Existing generic persona derivation already expands a
regional language when no explicit Accept-Language is supplied. A separate
matched ordinary HTTP/navigation/fetch/XHR control can test that configuration
without TLS handoff; no product-language patch or new Search occurs here. The
source-only analysis SHA is
`4f610e3b83766843de5368e386269fdfe2ed909883f0d918836db4865f59c4ff`.

Independent read-only saved-result inspection finds0 discrepancies and rechecks
all3417 unique input hashes twice. It independently recomputes both355-byte
body digests and matches the saved own/historical ordered field names, Cookie
grouping, priority and DATA boundaries to the summary. Its result SHA256 is
`a587d865d72f2e3ff4ecf27b4481530760ba75540cbf416318a84790bab75db2`.
This is saved-result consistency and unchanged-input evidence, not a second
HPACK decoder execution, complete protocol/JS audit or checkout qualification.

Read-only generic Cookie-shape analysis traces jar serialization through primp
HeaderMap and the H2 iterator: current supplied Cookie values remain whole.
[RFC9113 section8.2.3](https://www.rfc-editor.org/rfc/rfc9113.html#section-8.2.3)
permits crumbling, and the pinned Chromium/QUICHE encoder corroborates Chrome's
split behavior. Either representation is valid HTTP2. A candidate H2-only
expansion must preserve occurrence/crumb order, duplicates and sensitivity,
leave Set-Cookie/non-Cookie/H1 behavior unchanged, and retain header-size and
frame safeguards. Own29 items would become29 fields, not a replayed28-item
Chrome jar. No implementation or rejection-causality claim is made. Fresh
matched local Chrome H2/H1 controls and source/release gates remain required;
the existing Chrome certificate prompt is again visibly confirmed without
agent bypass.

D23's wider no-render scrolling failure remains unresolved. The native TLS/H2
fixture is still listening and its Chrome certificate warning is freshly
observed; the window is handed to the user, without agent bypass.

## Investigation 23: isolated Domain controls and browser-session collision

Status: the native local matrix is independently qualified. A generic CDP
browser-session repair passes source review, full render coverage and two
complete official Playwright captures. Independent capture/hash and local
attachment-performance audits pass. A broader no-render scrolling test fails.
Checkout remains incomplete. No Southwest request or payment occurs here.

Investigation 22 below records its earlier blocked snapshot. The user later
authorizes direct local routing. Only the active Ethernet system proxy bypass
list gains exact lvh.me:19368 and one.lvh.me:19368 entries, preserving existing
exceptions and enabled proxy endpoints. System DNS already resolves both to
loopback. No hosts, DNS, Mihomo configuration or Wi-Fi setting is changed.
The prior failed first native arm is retained and never retried.

A subsequent CFNetwork proxy-resolution check still selects HTTP/SOCKS for
both port-qualified bypass entries. Under the user's routing authorization,
the active Ethernet list gains the two exact hostnames lvh.me and one.lvh.me,
preserving all thirteen existing entries. These hostname entries apply to all
ports on these two hosts. CFNetwork then returns direct for both target URLs,
and both manifest probes return HTTP200 from 127.0.0.1:19368. Proxy endpoints
remain enabled and unchanged; no DNS, hosts, proxy-core or other service setting
changes. This later verification does not retroactively qualify prior routes.

A fresh arm in the existing Chrome153 Incognito profile acknowledges 48
boundaries but completes 44: old root-path Cookies trigger the frozen initial
Cookie guard in four localhost/IP setters before any mutation. It is not
reported as a complete matrix. A separate signed-out local profile's Incognito
arm completes 48/48 with all 18 initial setter requests Cookie-empty. No old
state is copied or cleared. The fixture and six frozen input files are
unchanged. Both native arms retain full unsanitized HAR and gzip JS traces.
Temporary capture preferences and DPR are restored after export.

Independent raw-wire audits match all 285 and 307 respective native requests,
full document/POST copies, acknowledgements and summaries. Both HAR files omit
24 probe uploads and the summary body; authoritative wire and saved POST/body
copies retain those nonempty bytes. The isolated trace contains 48 exact
fixture executions and source coverage for all 48 scripts, not every JS
instruction, a complete ambient source catalog or pure-tab timings.

HTTP/JS exact and dotted DNS Domain reach the child; absent/empty Domain stay
parent-only. Parent-domain writes and unrelated-domain rejection behave as
expected. Both coexistence controls preserve distinct same-name/path host and
domain Cookies and selectively delete each. Exact localhost/IP Domain is
accepted on same-host HTTP/JS probes; those observations do not establish
subdomain scope or an internal hostOnly flag.

The immutable D20 Obscura arm completes 48 operations and acknowledges 48
boundaries, but its first history inventory crashes the official Playwright
driver. All three required pre/post-close inventories are missing, so full
capture fails. Secondary JS languages also differ from native. This arm is
retained as failed, not retroactively completed by later controls.

Independent GPT6.1 Sol xhigh analysis traces two browser attachments through
the official driver to the same fixed browser-session ID. The second session
replaces the driver's routing entry, consistent with the unknown-reply-id
assertion. Original offending raw WebSocket frames are not captured; the
source/stack chain is not a frame-level proof. A second physical WebSocket has
an independent history registry and cannot substitute for the original one.

The generic repair allocates each browser attachment through the existing
unique target-session allocator, consistently using its ID in the mapping,
return and event. A release-nextest regression fails before the change on
equal IDs, then passes. It covers reply id/session identity, browser/raw/managed
page separation, sibling detach and history live/finalized states. No Cookie,
history-handler, identity, hostname, transport or security behavior is changed.
Standards initially identifies one nonblocking repeated test predicate, which
is extracted and re-reviewed. Final Standards and Spec each report zero
findings. CDP focused render coverage passes 411 tests with three skips. Full
render coverage passes 2379 tests with four skips, including the final regression
source. Exact render/no-render CLI builds succeed and obstacle passes 33/33.
Affected no-render target/history/session coverage passes 17 tests, with 252
filtered out. A wider no-render library/integration run passes 268 and fails one
unchanged scroll-centering test. It is not reported as a passing group. The same
scrolling JS geometry condition returns false in both the immutable old and new
no-render CLI controls; these controls are not the original nextest binary.
No scrolling code or test is modified, waived or automatically retried.

A separately reviewed private helper version pairs explicit candidate binary
path/SHA and retains its own source/effective pin. Original fixture/input
checks, official Playwright attachment order, same physical connection, 48
boundaries and all three history/body/hash gates remain intact. It does not
modify the frozen helper or erase its failed capture. Both fresh candidate
arms complete 48/48 and report no failures. Original-persona protocol logs show
two actual attachments returning browser-session-1 and browser-session-2. Each
arm retains all three inventories and 514 isolated-context records, with 449 body
copies per close stage. Page closure leaves the context live; context closure
records live=false and finalized=true. Independent audits match all 3679 and
3655 respective protocol send/reply pairs with no unmatched ID/session or
protocol error. Every history body chunk is decoded and its size/SHA recomputed;
each arm's 257 actual HTTP requests and 514 bidirectional TCP files also match
framing, bytes, chunks and closes. The fixed journal read-time prefix remains
live, not a finalized server lifetime. The second arm uses the reviewed local
persona and matches the
native observed UA, primary/secondary languages, timezone, viewport and DPR;
this is not universal fingerprint or wire-header parity.

Cookie Domain/coexistence differences remain, with cookies.rs unchanged. A
separate coherent key/resolver design identifies a lossy legacy CDP import
compatibility decision before its own implementation; it is not bundled into
the session repair. The initial attachment performance attempt fails before
measurement because its Windows persona fixes DPR1 but the helper asks for
DPR2. Zero measured samples and complete failure/cleanup evidence are retained.
A separate helper version uses matching DPR1 on both arms and records that
configuration explicitly. Its reviewed cargo-idle run completes six ABBA
blocks, 24 fresh owned CLI processes, with ten warmup and 100 measured sequential
attach/version/detach iterations per process. Full official protocol, child
stdout/stderr, resource records and source/pins are retained. The failed first
attempt is excluded, not overwritten. Independent verification confirms all
24 protocol connections, 24 unique owned PIDs, all 2640 warmup/measured
attach/version/detach triples, pins and retained resource records. It confirms
these p50/p90 distributions, with no protocol error or forced kill:

| Metric | Old p50 / p90 | Candidate p50 / p90 | Median change |
| --- | --- | --- | --- |
| 100-iteration wall, ms | 144.947 / 150.417 | 143.061 / 150.413 | -1.30% |
| CLI child CPU, ms | 36.826 / 37.530 | 37.630 / 38.321 | +2.18% |
| CLI child max RSS, MiB | 34.922 / 35.527 | 34.938 / 35.559 | +0.04% |

These medians lie within the approximate 10% noise floor. Wall timing includes
full protocol-log overhead, and CPU/RSS describe the CLI child, not the Python
or official driver. This is an attachment microbenchmark, not navigation,
site, TLS/H2 or overall browser performance evidence. Native TLS/H2 certificate
handling remains a separate human handoff. Neither observed Domain divergence
nor this protocol issue proves Southwest HTTP403 cause or checkout completion.

## Investigation 22: Cookie Domain qualification and local-route boundary

Status: source analysis and a reviewed neutral fixture are prepared, but the
native arm fails before its first fixture document loads. No Domain repair,
matched browser result, HTTP403 cause or checkout completion is established.

The current shared HTTP/JS resolver classifies explicit Domain equal to origin
as host-only. The current canonical storage and snapshot keys also omit the
host-only bit. Pinned [Chrome153 domain resolution](https://raw.githubusercontent.com/chromium/chromium/78e5e45d4bb41035e17ea4da2cc257f496416ac9/net/cookies/cookie_util.cc)
distinguishes registrable DNS equality from absent/empty Domain and exceptional
IP, intranet and public-suffix cases. This source-derived candidate is not a
new native behavioral observation. Both scopes match the shopping host, so an
exact-origin scope difference alone explains no HTTP403. A repair cannot be an
unconditional equality-bit flip or a scope-only change that ignores key identity.

A self-owned HTTP matrix covers eighteen cases, twenty-four phases and
forty-eight acknowledged page boundaries. It separates HTTP/JS absent, exact,
leading-dot and empty Domain, valid parent and unrelated rejection, localhost
and IPv4 acceptance, and same-name/path host/domain coexistence with selective
deletion. Ordered Cookie strings retain duplicate names. Every mutation awaits
both setter and target echoes before the next write, and each phase has an
independent top-level probe. Predictions remain hypotheses, not observations.

Independent GPT6.1 Sol xhigh Standards and Spec final reviews each report zero
findings. Initial Spec review finds one missing intermediate child observation;
it is corrected and re-reviewed before execution. Frozen sources, an immutable
D20 binary and explicit persona hashes are retained. JavaScript syntax checks
and Python compilation return zero. All configured system DNS addresses resolve
to loopback and all three ports are free before the owned fixture starts.
Eight direct exact-Host manifest/source-hash preflight GETs return HTTP200.
Those local diagnostics do not qualify the native browser's route.

Native Chrome153 Incognito starts per-tab Network and Performance JS sampling
before one first-case navigation. The browser returns HTTP502, not a fixture
result. The saved unsanitized HAR has one GET entry, an empty response body,
server address 127.0.0.1, connection 7890 and Proxy-Connection fields. The live
fixture's audited read-time prefix has only the eight preflights and no native
case request. No reload or case retry occurs. HAR is a browser projection, not
raw proxy TCP field order. No global NetLog is started in the shared process.

The original HAR and gzip Performance trace are retained privately without
redaction. The trace exports resource-content and source-map options; its
identified compilation belongs to the browser error page, not fixture JS.
Sampling and export choices are not every instruction or a complete source
catalog. Native Keep log, sensitive-HAR preference and the temporary DPR are
restored to their original values. A reserved localhost-hostname alternative
fails system DNS, so Chrome's builtin synthesis is not substituted for the
immutable Obscura arm or used to waive preflight.

Human local-route setup is required before a fresh matched arm. No proxy, DNS,
hosts or security settings are changed by the agent. Native TLS/H2 remains a
separate certificate-warning handoff. No Obscura arm, product edit, fresh release
gate, Southwest request, passenger/card entry or payment occurs in D22. Existing
dirty work and the qualified D20 binary remain unchanged and unstaged.

## Investigation 21: current accepted-H2 shopping representation

Status: a separate diagnostic captures the current candidate's actual outbound
H2 representation. Checkout remains incomplete. Four completed shopping
responses return HTTP403; a fifth outgoing shopping has native status zero,
not an established HTTP response. No product behavior is repaired in this
investigation, and no rejection cause is established.

Offline D20 analysis confirms that the first prepared body matches three
historical Chrome200 actual H2 bodies. Its Cookie name set matches one of those
successful sessions. D20 has no `_mibhv` assignment record, while its
`swa_spa_grp` value matches preceding ordinary-resource Set-Cookies. Twenty
app/vendor/en sources match Chrome; independently served swa-common sources
differ. These observations do not prove a missing Cookie, execution defect or
HTTP403 cause. D20 prepared headers and D17 diagnostic wire bytes cannot be
substituted for a new current-session wire capture.

The temporary opt-in observer wraps the existing scalar and vectored outbound
writes after HPACK/framing. It records only prefixes accepted by TLS IO, without
changing inputs, returned Poll, framing, TLS, proxy or browser identity. This
boundary is not remote encrypted-packet receipt and starts after the client
preface. Raw bytes and per-connection journals use exclusive mode0600 files
under a private directory. Capture errors remain sticky evidence failures,
not transport errors. Observer drop describes only the local lifetime.

Independent Standards and Spec inspection each find zero observer findings.
Three private release-nextest tests pass for partial scalar/vectored writes,
Pending, zero, underlying errors, flush/shutdown forwarding and a deliberately
failed recording sink. Test raw bytes and journals are retained. Separate
decoder review identifies three evidence-boundary errors, all corrected before
execution: non-Cookie duplicate values must survive semantic projection, the
historical Chrome anchor must be pinned and fully parsed, and safe-field order
must not stand for every ordinary field occurrence. Full ordered HPACK fields
remain separate from the Cookie-only semantic join.

The exact render diagnostic build succeeds. Its immutable binary SHA256 is
`1c230c18f470d4ac4abbdd63888005ed939496cc9266f47654b167e86a37e815`.
One ordinary search uses the unchanged historical Mac153 persona, configured
proxy, 1365x768/DPR2 and consent condition. No state is transferred from Chrome.
The thirty captured connections contain 712504 accepted bytes. Every journal
has a healthy local-drop boundary and matching accepted/recorded/file counts,
with no capture failure. All connections and the pinned historical Chrome
connection parse completely with separate chronological HPACK state.

Five outgoing shopping streams uniquely correlate to their own full prepared
header multimap and transport body digest. FIRST is fetch-137, native sequences
396/416 and H2 stream221, HTTP403. Every body has 355 bytes and SHA256
`b05659a296020be81bafa58ba1491e28acd8bdd715410b32feb088216a0f9689`.
FIRST matches the successful Chrome body's bytes, standard-field values,
pseudo order and HEADERS priority. The D18 body-framing repair now has current
online wire evidence: both FIRST bodies close with one 355-byte DATA frame
carrying END_STREAM. Ordinary field order still differs. Obscura sends one
Cookie field containing twenty-nine items; the pinned Chrome200 sends
twenty-eight separate Cookie fields. Different session Cookie inputs remain
distinct, and neither representation difference establishes rejection causality.
Generic [pinned QUICHE Cookie crumbling](https://github.com/google/quiche/blob/2c4a124642f095f995cd2e9e2fe10decc08df662/quiche/http2/hpack/hpack_encoder.cc)
corroborates the splitting behavior, not a universal ordinary-field order.

An independent offline parser reproduces all current ordered header blocks,
body bytes, journal counts and five own-session correlations, and reparses the
original Chrome plaintext. Current FIRST actually includes `_mibhv`; one JS
assignment precedes it, with no received Set-Cookie. The after-search snapshot
matches the sent value, but assignment telemetry does not establish accepted
scope or setter stack. Current `swa_spa_grp` matches five preceding ordinary
resource response values and has no recorded JS assignment. The changed
akaalb value matches the latest received response and after-search snapshot.
All twenty ordinary app/vendor/en source hashes match Chrome; nineteen
recorded pre-shopping ordinary executions report `ok`, not a complete execution
inventory. These current-session observations are distinct from D20 and do
not establish an omission, execution defect or rejection cause. Protection
sources are not interpreted or modified.

The diagnostic retains 2407 network/JS events, including 103 attempted Cookie
writes, 337 script-execution events and 118 storage mutations. Observed native
histories are exhausted at their live pre-close boundary: 713 records and an
empty default history, not finalized post-close history. Document HTTP200,
zero credit-card controls, no submitted payment, empty capture-failure list
and server exit zero are recorded. Diagnostic disk IO and overlapping delivery
compilation exclude this run from performance evidence.

The temporary source edit is removed byte-for-byte. All prior source hashes
and the exact rebuilt delivery binary match the frozen D20 candidate,
`517935fc0109f8dfa2d84cb5f4c09ad4c1a6701353a677f8cf50de82a2f9ae2b`.
Existing dirty work and unrelated untracked tests are preserved. No commit or
push is made. The native self-owned TLS/H2 fixture still requires human
handoff at its certificate warning; an agent does not bypass it. Explicit
Domain equal to origin is another generic qualification candidate: Chrome
accepts a domain Cookie, while the current resolver predicts host-only.
The existing D20 snapshots omit that bit, and both scopes match shopping's
host. A matched neutral parent/child-host fixture is required before repair.

## Difference 20: changed-value browser Cookie creation order

Status: the bounded repair passes focused net and full release-render tests,
both CLI builds, matched candidate fixtures, the obstacle course and a local
cargo-idle performance comparison. The independent online retest still returns
shopping HTTP403. Checkout is incomplete; no HTTP403 cause or credit-card-page
success is established.

The D19 first shopping request is fetch-107, 355 bytes, HTTP403. Its request
and transport body SHA matches the historical Chrome200 body. D19's received
HTTP cookie lineage, snapshots and final sent value link a changed same-scope
akaalb cookie to its retained early position. Historical Chrome accepts a
changed overwrite and sends that key late. Another Chrome200 session has no
such overwrite and sends it early. Early position is therefore not a rejection
condition. Accepted Chrome event-time proxies corroborate value-sensitive
ordering, but do not expose internal creation dates, and independent sessions
cannot establish HTTP403 causation. D19 transportRequest is prepared metadata,
not a new live H2 wire capture. Its JS assignment hashes represent attempts,
not accepted values, scopes or setter stacks. No Cookie/token state is replayed.

A native Chrome153.0.8010.52 incognito control completes sixteen fresh-scope,
self-owned HTTP cases. Separate awaited HTTP responses establish seed order.
All initial requests have no Cookie; full result POSTs are acknowledged. Actual
before/after request bytes show that HTTP and JS same-value replacements retain
position, while changed-value replacements receive a later position. Reverse
seed order and neutral names exclude name ranking. Rejected HttpOnly JS writes
and deletes preserve the old value and position; deletion/recreation moves the
key later; longer paths remain first. The frozen D19 binary completes the same
matrix but retains initial order in the six discriminating changed-value cases.
UA, primary language, timezone, 1365x768 viewport and DPR2 match. The original
Obscura persona has an extra navigator.languages entry; a separate local-only
matched-language persona rerun completes all sixteen cases with the same
results. The historical online persona is unchanged.

The first local frontend start fails because macOS has no 127.0.0.2 loopback
alias. Failed startup and capture evidence remain intact. The successful
fixture binds existing IPv6 loopback without changing system configuration,
and its helper checks the exact served manifest before spawning a browser.
Raw TCP bytes in both directions, headers, bodies, fixture source, journals,
CDP events, cookie jars, native history chunks and failures remain private and
complete. Native computer use activates fifteen Next links and the final
summary; mouse clicks that do not navigate remain attempts, not completion. Chrome's raw
NetLog and source-inclusive JS sampling trace cover the first navigation and
all cases, and are saved and parsed after recording stops. Sampling is not
every JS instruction. This HTTP control does not replace the pending native
Chrome TLS/H2 qualification, and no certificate warning is bypassed.

The independent raw audit correlates 354 complete HTTP request/response pairs
over 24 bidirectional TCP captures, with zero trailing bytes, at an explicit
read-time server-log prefix. All 111 native case request anchors match NetLog
header order/values, Cookie and received Set-Cookie. Forty-seven accepted adds
and two HttpOnly rejections match the declared synthetic inputs. Sixteen case
scripts are compiled/evaluated and their retained source exactly matches the
served inline source. The NetLog has 42816 events; the gzip trace has 134218
events and 16037 CPU ProfileChunks. Later arms append to the live server, so
this is not an audit of a later final file or of every JS instruction.

The baseline preserves a live prior ordinal for every replacement. The repair
selects a value-sensitive policy only for ordinary HTTP and JS storage, after
existing validation. Equal values preserve the ordinal; changed values receive
a new monotonic ordinal under the existing write lock. Trusted CDP, snapshot
and file imports retain their destination/relative-order contracts. Snapshot
deltas propagate renewed source positions and keep unchanged concurrent
destination values. The comparator, matching, JS API, transport and SSRF gates
are unchanged. This targets
[pinned Chrome CookieMonster behavior](https://github.com/chromium/chromium/blob/78e5e45d4bb41035e17ea4da2cc257f496416ac9/net/cookies/cookie_monster.cc),
not an RFC6265 violation: [section 5.3](https://www.rfc-editor.org/rfc/rfc6265.html#section-5.3)
unconditionally inherits the old creation time, unlike Chrome's same-value rule.

The new release regression fails on the D19 implementation. It covers HTTP/JS,
both initial orders, neutral names, host-only/domain scopes, same-value
attribute changes and the internal allocation counter. Existing persistence,
projection, snapshot and actual primp-wire expectations are updated for the
new browser order; trusted import behavior stays explicit. Rejected HttpOnly
tests also assert unchanged snapshots and allocation counters. The first net
run exposes one old wire-order expectation after the new regression passes;
the corrected complete net suite passes 199/199. Standards source review finds
zero hard violations and one optional low-priority duplicated-test-assertion
smell. Short explicit assertions are retained. Independent Spec source review
finds zero missing, extra or wrong source requirements. Source review alone
is not delivery or checkout completion.

Full release-render nextest passes 2378/2378, with four existing skips. The
exact render CLI build succeeds, and immutable candidate SHA-256 is
`517935fc0109f8dfa2d84cb5f4c09ad4c1a6701353a677f8cf50de82a2f9ae2b`.
Its official Playwright/CDP run completes sixteen cases with no capture or
runtime failures. Exact before/after/deep/deleted Cookie results and all
fixture-reported identity fields, excluding arm URLs, match the native Chrome
control. Both observed native histories are exhausted at the pre-close read
boundary: 222 records plus an empty default history. They are live snapshots,
not a finalized post-close-history claim. The fixed candidate passes 33/33
obstacle stages. These functional runs overlap no-render compilation; their
reported timings are not performance evidence.

Focused no-render net/JS coverage passes 116/116; 629 filtered tests are not
full no-render coverage. The exact no-render CLI build succeeds, SHA-256
`9910529d57c86483f9b6620b546c7cd5e827f430b8037e1c86f2673b08945e5c`.
Its separate official Playwright/CDP run completes all sixteen cases, with the
same exact native Cookie/identity comparison and no failures. The exact render
build is restored, and its hash matches the immutable tested candidate.

The independent online run uses that immutable render binary, the unchanged
historical Mac153 persona, 1365x768/DPR2 and the same configured proxy. Document
HTTP200 completes; FIRST shopping fetch-107 and three completed shopping
responses remain HTTP403. JS and transport bodies are both 355 bytes, SHA-256
`b05659a296020be81bafa58ba1491e28acd8bdd715410b32feb088216a0f9689`.
Its prepared Cookie order has 28 names; the changed akaalb and sRp keys are now
late. This is prepared transportRequest metadata, not a new live H2 capture,
and different response inputs do not establish across-session causality. The
capture retains 2162 events, including 101 attempted Cookie writes, 308 script
execution events and 104 storage mutations. Both observed histories are
exhausted at the live pre-close read boundary: 644 records plus an empty
default history. No capture failure or missing-body/screenshot event is
reported; server exit is zero. Credit-card controls remain zero and no payment
is submitted. This functional run overlaps compilation, not the later
cargo-idle performance comparison.

With compilation and native capture idle, twelve ABBA blocks each for the
HTTP and JS changed-value cases execute 96 fresh CLI processes, 48 per arm
and 24 per case/arm. All samples validate the complete result and actual
before/after order against their distinct baseline/candidate expectations.
The server audit validates all 624 expected requests and full result POSTs.
Input binary, persona, fixture and helper hashes remain unchanged. Both arms
use the same local fixture, Mac153 identity, configured CLI 1280x720/DPR2,
load boundary, one-second fixed settle and JSON capture path. This CLI viewport
is separate from the 1365x768 native/PW control. Dynamic URLs, IDs, timings and
legitimate semantic/POST differences mean the workload is not byte-identical.

Combined baseline/candidate CPU median is 103.601/105.019 ms (+1.4%), p95
107.317/110.048 ms. Wall median is 1131.484/1131.603 ms, p95
1135.003/1135.944 ms. Peak RSS median is 50544640/50642944 bytes, p95
51052544/51118080 bytes. Separately, HTTP CPU median/p95 is
104.304/107.317 versus 105.158/109.482 ms; JS is 103.351/107.308 versus
104.722/110.048 ms. Complete per-case wall/RSS distributions and block deltas
are retained with raw stdout/stderr, rusage and network evidence. Differences
are within the approximate ten-percent noise floor, not a demonstrated
speedup or regression. Fixed settle dominates wall time; no warmup and
remaining page-cache effects limit interpretation. This is self-owned HTTP
Cookie performance, not TLS/H2, paint or broad real-site performance evidence.

## Difference 19: mismatched Cookie Domain acceptance

Status: bounded generic Cookie acceptance repair passes full render tests,
both CLI builds, focused no-render tests and matched local fixtures. Shopping
still returns HTTP403. No rejection cause or credit-card-page completion is
established.

The three current raw sessions do not support reusing an older Cookie absence
list. The latest D18 first shopping (`fetch-82`, 355 bytes, HTTP403) already
sends the three Adobe identity/consent/cluster cookies and consent cookies.
One Chrome200 session has `_mibhv` and no response-produced `swa_spa_grp`; the
other Chrome200 has `swa_spa_grp` and no `_mibhv`. These presence differences
are not necessary-success conditions or a proved rejection cause. Cookie
values and tokens are not copied or replayed.

The native Chrome153 incognito self-owned HTTP fixture rejects the synthetic
assignment `rejected=no; Domain=unrelated.example; Path=/`. D18 instead stores
it as localhost host-only and sends it on a subsequent request. The first
navigation requests contain no Cookie fields. Chrome initially uses 1200x780
and DPR1, while D18 uses 1365x768 and its frozen Mac persona DPR2. An attempted
D18 DPR1 capture fails explicitly before navigation because it conflicts with
that persona; the failed capture is retained, not counted as a matched control.
The subsequent fresh 127.0.0.1 controls match the desktop UA, 1365x768 viewport
and DPR2. D18 still accepts and sends the invalid cookie; D19 rejects it,
matching Chrome's accepted/deleted values and scopes. Chrome and Obscura's
remaining root/HttpOnly cookie ordering differs and is retained separately.
Known fixture JS and raw client requests, server request records, result
journals and Obscura native events are retained privately. A later Chrome
fixture reload preserves a raw-byte NetLog (8450 events) and source-inclusive
JS sampling trace (5577 events), both parsed successfully after saving. That
reload is not the fresh no-Cookie control, and sampling is not a record of
every JS instruction. This HTTP Cookie comparison does not replace the pending
native Chrome TLS/H2 control, and no certificate warning is bypassed.

The baseline shared resolver returned origin host-only for unrelated Domain
attributes. Both HTTP and JS setters then created, overwrote or expired cookies on the
origin, contrary to the whole-cookie rejection in
[RFC 6265 section 5.3](https://www.rfc-editor.org/rfc/rfc6265.html#section-5.3).
The new product-root regression fails on the original code when an unrelated
Domain cookie appears on attacker.test. It covers HTTP and JS inputs, two
origins, new cookies, replacement and expired deletion. The candidate changes
only the rejected-domain fallback to None before any mutation. The old HTTP
test's origin-fallback expectation is corrected, and JS/single-label suffix
tests gain origin-jar rejection assertions. Existing valid-domain handling,
Cookie sorting, exact-origin behavior and PSL policy are unchanged; broader
PSL and exact-origin semantics remain separate qualification gaps. No JS API,
site resource, protection script or transport representation is changed.

Existing Cookie diagnostic events still represent attempted writes and have
no accepted result, timestamped scope, generation or setter stack. This fixture
proves the synthetic acceptance delta from its actual journal/jar/requests,
not from treating those events as acceptance. Other observed Cookie ordering
differences are not included in this repair. Standards and Spec source reviews
each have zero confirmed findings. The product-root release-render net suite
passes 198/198, and the exact render CLI build passes. Candidate SHA-256 is
`b27182fada62c65ab1bc7795ae749916ac1d17cf2ba3ddf37edfb75d99a8312e`.
The official Playwright/CDP candidate completes the matched fixture with no
invalid-domain cookie in its jar or requests. The full release-render nextest
suite passes 2377/2377 with four existing skips, and the immutable candidate
passes the authoritative obstacle course, 33/33. The obstacle run overlaps
compilation and is not latency evidence. Focused no-render net/JS coverage
passes 210/210; 534 filtered tests are not full no-render coverage. The exact
no-render CLI build passes with SHA-256
`49cccad35b8067b2a3798ccdfb19bd716452d4698b6e5f2ad20c25776a43fe58`.
Its separate official Playwright/CDP fixture completes the same six Cookie
write checks, jar and request results at 1365x768/DPR2. The exact render build
is restored, and its hash equals the tested immutable render binary above.
The independent official Playwright/CDP shopping retest uses that immutable
render binary, the pinned
Mac153 persona, 1365x768/DPR2 and the same configured proxy. Document HTTP200
completes; the first shopping request (`fetch-107`) and three completed shopping
responses remain HTTP403. First request and transport bodies are both 355 bytes,
with SHA-256
`b05659a296020be81bafa58ba1491e28acd8bdd715410b32feb088216a0f9689`.
The raw capture retains 2174 events and both observed context histories (644
records plus an empty default history), each exhausted with hasMore=false at
the read boundary. They are live pre-close snapshots, not a finalized
post-close-history claim. Capture reports no failures or missing response
bodies, the server exits normally, and credit-card controls are zero. No
payment is submitted. This behavioral retest overlaps compilation and is not
performance evidence; response-count differences from independent sessions
do not establish repair causality.

A cargo-idle comparison executes twelve ABBA blocks, 24 fresh CLI processes
per arm, against the same self-owned HTTP Cookie fixture. All 48 validate the
six complete write journals and exact per-arm results; the audit passes all
192 actual requests. D18 must retain its invalid-domain cookie, while D19
must reject it; that semantic delta is not normalized away. Binary, fixture,
persona and helper hashes remain unchanged. CLI inputs use the same Mac153
persona, configured 1280x720/DPR2 and one-second settle, rather than the separate
1365x768 Playwright/Chrome comparison. Baseline/candidate CPU median is
88.897/87.282 ms (-1.8%), p95 98.526/90.451 ms; wall median is
1119.121/1118.287 ms and p95 1132.246/1121.120 ms. Peak RSS median is
39010304/38944768 bytes and p95 39354368/39174144 bytes. Results are within
the approximate ten-percent noise floor, not a demonstrated speedup. Fixed
settle dominates wall time; no warmup and remaining page-cache effects limit
interpretation. Original timings, bytes, headers, stdout/stderr, resource use
and server records remain private and intact. Expected Cookie and posted
journal differences mean this is not byte-identical work, TLS/H2, paint or
real-site performance evidence.

## Difference 18: known request-body EOF and DATA closure

Status: bounded generic transport repair passes the full render suite, both
CLI builds and local framing checks. A fresh online recheck completes document
lifecycle but shopping remains HTTP403. An earlier process abort is retained
and not reproduced by the recheck or the old-binary control. Checkout is still
incomplete; no shopping403 cause or successful payment-page claim.

The actual Chrome153 HTTP200 anchor ends its 355-byte request on the real DATA
frame. D17 sends the same bytes in nonfinal DATA plus empty terminal DATA. A
self-owned local TLS/H2 fixture reproduces the latter behavior for both fetch
and XHR through official Playwright Python 1.60.0 and Obscura CDP. Raw decrypted
client frames, ordered HPACK fields, JS results and native histories are retained
privately. The Chrome153 computer-use fixture is blocked by its self-signed
certificate warning and awaits human handoff; no warning is bypassed and no
global trust or browser security setting is changed. This is not a completed
local Chrome control.

The product-root raw H2 regression fails on the original sender with an extra
empty final DATA even for a one-byte reusable body. The candidate checks the
body's real `is_end_stream()` after polling a frame, marking only the final
flow-controlled chunk END_STREAM when EOF is known. It does not infer EOF from
Content-Length or size hints and does not buffer a lookahead frame. Unknown EOF
retains the empty final DATA fallback; bodyless HEADERS closure and trailer
sending stay unchanged. Reusable retry clones are unchanged. The test covers
1/355/1024/1025-byte bodies, multi-window bodies, a wrapped known-EOF body,
unknown and empty streams, trailers, and two refused-stream retries. Large
bodies use offset-dependent byte patterns rather than identical bytes. A second
typed H2 server test checks the exact decoded trailer fields, including duplicate
values. Scenario offsets derive from the reusable size list rather than magic
request ordinals. The final product-root net suite passes 197/197. Its second
unknown-stream chunk depends on server receipt of the first, testing the absence
of lookahead waiting.

Header ordering and Cookie splitting are not changed. This removes redundant
framing for known EOF while matching the observed Chrome boundary; both old
and new forms are valid H2 representations. The exact render CLI build passes;
binary SHA-256 is
`b76d2aef515c2dc118064d3b6a28be446073a9296d7593304d6ec45e201c75ae`.
The local official Playwright/CDP candidate completes the same thirteen fixture
results. Its four 355-byte fetch/XHR captures each contain one real DATA frame
with END_STREAM; empty bodies still close on HEADERS. Cookie text and body bytes
are unchanged. The full release-render suite passes 2376/2376 with four existing
skips. The no-render focused net/JS run passes 209/209; its 534 filtered tests are
not full no-render coverage. The exact no-render CLI build passes, with SHA-256
`baecd9a040b41c05ad10e0332255a1d55f6fe8c662803e036cd68aa251a7339e`.
Its separate official Playwright/CDP fixture completes thirteen results, and
all four 355-byte fetch/XHR requests end on their real DATA frame. The exact
render build is then restored; its hash equals the immutable tested render
binary above. Final Standards
and Spec source reviews each have zero remaining confirmed findings after
addressing the test-maintainability and coverage suggestions; documentation
status was synchronized with the completed gates. Earlier D16/D17 evidence
does not qualify this new candidate.

The immutable candidate passes the authoritative obstacle course, 33/33. This
behavioral run overlaps compilation and is not latency evidence. A separate
cargo-idle TLS/H2 CLI comparison runs twelve ABBA blocks, 24 processes per arm,
with equal configured persona/default viewport, one-second settle and capture.
All 48 processes complete thirteen exact body/Cookie/path result checks. CPU
median is 42.327/42.568 ms and p95 44.605/46.044 ms (baseline/candidate); wall
median is 1046.085/1045.487 ms. Median RSS changes by 0.055 MiB. This fixture
does not establish a material regression or a broad speedup; its fixed settle
dilutes wall differences, and server-side raw capture remains part of the
controlled fixture. The previous D16 stress CPU cost remains separate.

Raw POST header order varies across baseline processes as well as candidate
processes. A strict order-invariance audit therefore fails and is preserved.
The separately saved semantic audit confirms equal header-name/value multisets
and exact body bytes across all 720 server requests, apart from intentional
arm labels in path/referer. Do not claim byte-identical request-header order
from this benchmark or attribute this pre-existing variation to D18.

The independent D18 online run uses the fixed render SHA, official Playwright
1.60.0, the existing Chrome153 persona, 1365x768/DPR2 and the same proxy route.
It aborts at the V8 137.3.0 `Weak<Context>::first_pass_callback` unwrap in
handle.rs:869, followed by a non-unwinding panic. The retained CDP file contains
1322 events and zero observed shopping-path events. Native-history export
then fails with TargetClosed and the original helper does not save its final
result, so neither a shopping status nor a complete native history is claimed.
The original server log, capture and helper failure are preserved. A separate
crash-safe diagnostic helper retains stage/error/exit metadata without
replacing the original record. No runtime or V8 source fix is made from this
single observation, and no payment action occurs.

The crash-safe independent D18 recheck then completes document HTTP200 and both
snapshots, with six completed shopping HTTP403 responses, zero card controls,
no capture failures and normal server exit. Its first shopping is native
`fetch-82`; both request and transport body references report 355 bytes and
SHA-256 `b05659a296020be81bafa58ba1491e28acd8bdd715410b32feb088216a0f9689`,
matching the historical anchor. `transportRequest` is a native capture stage,
not a new decrypted live H2 wire capture. The fixed D17 old-binary control with
the same persona/query/proxy also completes document HTTP200 and first shopping
HTTP403, with two completed HTTP403 responses and zero card controls. Both are
independent sessions; two versus six application retries does not establish a
D18 effect. The crash is not stable in these two checks. Independent read-only
analysis locates the ContextAnnex weak-finalizer lifecycle in pre-existing
V8/deno/frame context setup; no V8 or deno version changes are in D18. This
does not rule out timing-sensitive triggering. Further crash work needs a
minimal GC/frame lifecycle repro, not a callback-panic suppression.
The existing release-render retained-document GC regression is repeated with
nextest stress-count 20 and passes 20/20 iterations. It exercises saved child
document ownership, frame drop, collection while retained and four additional
GC passes after reference deletion. This small isolated test does not reproduce
the online abort and does not qualify navigation/fetch/snapshot timing stress.

## First-shopping actual HTTP/2 wire comparison, 2026-09-30

Chrome153's fresh first-shopping HTTP200 is anchored to NetLog source 19090,
HTTP/2 session 17114 and stream 295. A separate D17 diagnostic build captures
encoded HTTP/2 plaintext accepted by the TLS writer, not merely prepared
request headers. Its first shopping correlates uniquely to native request
`fetch-108`, stream 213 and HTTP403 through the complete header map and body
digest. All 25 connection files parse fully with independent persistent HPACK
tables, no trailing bytes or incomplete header blocks. The temporary writer is
removed from delivery source; capture disk overhead is not performance evidence,
and local TLS acceptance is not proof of remote receipt.

The actual DATA body is 355 bytes in both browsers, SHA-256
`b05659a296020be81bafa58ba1491e28acd8bdd715410b32feb088216a0f9689`.
The ordinary header-name sets and sixteen allowlisted values match. Pseudo-header
order is `:method`, `:authority`, `:scheme`, `:path` in both; both HEADERS frames
use dependency zero, exclusive true and weight 220. Ordinary field order differs.
Chrome sends 28 separate Cookie fields; Obscura sends one field containing 25
cookie names. Chrome ends the stream on the 355-byte DATA frame; Obscura sends
that body in a nonfinal DATA frame followed by an empty final DATA frame.
Cookie name order and session-specific opaque values also differ.
These are wire observations, not evidence that any one difference caused 403.
Cookies and protection values are neither copied nor replayed.

The diagnostic capture contains four shopping requests: the first three receive
HTTP403; the fourth has native status zero with no HTTP result. The online
summary's three HTTP403 results describe only completed responses. This is a
separate session and binary from D17's four-HTTP403 lifecycle qualification.
Full raw frames, decoded ordered duplicate fields and source traces remain
private. Source analysis traces field ordering to the client-wide Chrome153
profile and the rank sorter in `vendor/primp-h2/src/frame/headers.rs`; Cookie
assembly to `serialize_cookie_header` and unchanged HeaderMap field emission;
and DATA closure to the streaming sender in
`vendor/primp/src/async_impl/h2_client/pool.rs`. These explain representations,
not an established semantic error or server rejection reason. No transport
repair is made from this single shopping pair.

The qualified next comparison is a self-owned local TLS/H2 fixture covering
navigation, fetch and XHR; synthetic header insertion orders; absent, empty
and 355-byte bodies; and synthetic Cookie path/creation/overwrite cases.
Request-class rules need qualification before replacing a global profile list.
Exact Cookie setter stack and timestamped accepted scope remain separate
pending work.

Read-only follow-up on this diagnostic session finds `swa_spa_grp` in five
pre-shopping HTTP200 responses, with matching sent value and identical
attributes: expiry 2038-12-31, Path=/ and Secure, with no explicit Domain,
HttpOnly or SameSite. The after-search cookie snapshot reports
www.southwest.com, Path=/, Secure, non-HttpOnly and SameSite=Lax. This snapshot
is after shopping, not a timestamped acceptance event for each earlier setter;
the trace records no prior JS assignment for this name. It narrows the response
producer and observed scope without proving a rejection cause or an exact
setter stack. No values are replayed and the complete raw records are retained.

## Difference 17: internal iframe document construction reentry

Status: generic reduced-fixture repair passes focused tests and source review,
and the online retry passes document lifecycle. Checkout remains incomplete.

A native V8 CPU profile captured during D16's document-lifecycle timeout shows
a repeated chain through Window indexed access, `contentWindow`,
`contentDocument`, `_IframeDocument` and a page-installed DOM method wrapper.
The final ten seconds contain approximately 49.5% `op_dom`, 18.7% `_dom`,
15.5% the calling resource function and 11.6% `_IframeDocument` leaf samples.
The repeated chain reaches depth 262. These are sampling observations, not a
transcript of every instruction or proof about the shopping response.

An independent local fixture installs a `document.createElement` wrapper that
reads `window[0]` while a connected empty iframe initializes. Both D15 and D16
reenter the constructor and reach the fixture's bounded recursion limit.
Internal construction called the mutable public method before publishing its
document/window, so a page observer could recursively request the same context.
This is a pre-existing browser-semantic defect, not a site-specific rule.

The candidate keeps public DOM methods mutable but uses captured native method
references for internal element creation, tree assembly and title query.
The regression additionally places traps on prototype create/append/query,
checks that those method overrides are not invoked for the initial empty
document, and verifies stable window/document identity, initial tree structure
and disconnected-frame behavior. Nonempty fetched-document parsing and other
public getter/custom-element callback boundaries are not qualified by this
fixture. No site script, opaque Cookie value, task budget or protection setting
is changed. The twelve focused release-render tests pass. Independent Standards
and Spec reviews each report zero confirmed findings for this bounded scope.
A separate native Chrome154 local fixture returns zero wrapper calls and stable
window identity; its process shutdown timed out after producing the completed
DOM result, so no clean-exit claim is made. Chrome153 qualification is also
confirmed through computer use in the actual
153.0.8010.52 incognito browser: the visible local fixture returns zero wrapper
calls and stable window identity. D17's release binary returns the same result
while the paired D15 binary reaches thirteen calls and the bounded limit.

The exact release-render CLI build passed. Binary SHA-256 is
`840bbbd6c72e5bb0f51ef986bcc56415c77b3af2011ae1d142ba697babdfc922`.
The same-persona official Playwright/CDP retry loads document HTTP200, dismisses
the Cookie banner before search and reaches first shopping HTTP403. All four
shopping responses are HTTP403; no card input or payment is reached. No
autonomous watchdog failure or Deno stack-overflow warning appears in this
retry. This qualifies recovery of the previously failing navigation boundary,
not elimination of shopping403 or successful checkout. The fixed binary above
passes the authoritative obstacle course, 33/33. The behavioral run overlaps
compilation and is not performance evidence. The final release-render nextest
suite passes 2374/2374 with four existing skips. All twelve focused tests also
pass in release no-render mode; the 534 excluded tests are filter exclusions,
not executed behavioral checks. Both exact CLI builds pass. The saved no-render
binary SHA-256 is
`557dc4596898dd2230b83d0f8c9def844422f72141b7b4804ea815a80ce93241`.
The restored `target/release/obscura` matches the tested render binary SHA-256
above exactly. Its cached display version is `0.1.0-dev+1099f5a`; this string
does not identify the current checkout. Qualification uses the binary digest
and the reviewed dirty diff against HEAD `172443f`, not that stale display label.

A controlled framework extraction comparison uses equal local HTTP/vendor
assets, MacChrome153, 1280x720 DPR2, one-second settle and the same CLI capture.
Twelve ABBA rounds give 24 successful processes per arm and per fixture. Median
CPU milliseconds baseline/candidate are React 112.292/111.959, Preact
105.778/105.762, Vue 113.223/113.242 and SSR hydration 104.440/104.351. Median
wall-time deltas are below 0.1%, and median RSS differs by at most 0.086 MiB.
Private records preserve per-process values, errors and median/p95 summaries.
No regression outside the approximate noise floor is established on these
fixtures. This measures extraction, not paint, and none of these fixtures
exercises initial iframe construction; the extreme dynamic-script CPU delta
in Difference 16 remains a separate qualification boundary.

A separate initial-iframe ABBA comparison runs 200 connected empty iframe
documents per process, 24 processes per arm, with equal HTTP input, measured
1280x720 DPR2 and zero settle. Baseline/D17 median CPU is 92.719/92.521 ms,
wall 116.492/116.354 ms and RSS 39.773/39.617 MiB. Both arms pass every document
and stable-identity check. CPU p95 is 93.614/94.000 ms and wall p95
118.158/117.921 ms. The full 200-document loop median is 11 ms in both arms;
individual-access measurements are quantized and do not establish a speedup.

## Difference 16: inserted classic-script execution semantics

Status: general reduced-fixture repair validated offline. Online checkout remains
incomplete; two candidate retries failed during document lifecycle work,
before shopping. The profiled iframe reentry and subsequent navigation recovery
are recorded in Difference 17 above.

Inserted inline and external classic scripts previously used indirect `eval`.
Two such scripts could not share a top-level `let`/`const` binding: the reduced
fixture failed in Obscura but returned `[42, false]` in native Chrome153. This is
a general Script-versus-eval defect, not an established cause of shopping403.
The missing select-depart execution events belong to a parser fetch path and
must not be attributed to this defect without further evidence.

Inserted classics now compile and run as native V8 Scripts in their owning
document realm. Native state borrows end before reentrant JS execution. The
change preserves lexical declarations, original exceptions and watchdog
termination, restores `currentScript` at the execution boundary, and uses the
final response URL as the base for relative imports. Optional execution
diagnostics remain page-owned and disabled unless explicitly requested.
There are no hostname-specific branches or changes to site protection scripts.

The independent Standards and Spec reviews have no remaining blocking findings.
The initial eight focused tests passed in both render and no-render release
modes. That candidate's full release-render suite passed 2372 tests with four
existing skips, both exact CLI builds passed, and its fixed-binary obstacle
course passed 33/33. The initial render binary SHA-256 is
`f7fff7d14d09b7669f29a5ff6bd003d85ab23bd4f994fe34be34b8111b4db76b`.
The fixtures cover inline/external lexical persistence, parse/redeclaration
errors, child-document ownership and diagnostics, nested `currentScript`,
redirect-relative imports, asynchronous ordering, callback assignment and
fetch/abort reentry. The final native-string/inline-base optimization adds a
nested watchdog recovery test and directly verifies UTF-16 native-op ingress,
strict global declarations and thrown-object identity. Its nine focused render
tests pass. The final D16 release-render suite passed 2373/2373 tests with four
existing skips before the D17 addition. Final D16/D17 requalification passes
all twelve focused tests in both render and no-render release modes and the
2374-test full render suite, as recorded above.
This does not change the existing DOM text storage's unpaired-surrogate boundary.

An interleaved ABBA benchmark executed 4000 inserted inline scripts per process,
64 processes and 2560 timed batches per arm, with equal local HTTP input,
MacChrome153 persona, 1280x720 viewport, DPR2 and zero settle. Baseline/candidate
process medians are 143.431/149.990 ms, CPU 126.749/139.927 ms and RSS
49.094/50.086 MiB. Batch median/p95 are 1/2 ms for both arms. The candidate
SHA-256 is `4f60c4f4167af652fb0477aa2c1dccda11b2cf27217c94df09f22fadddb4d7ef`;
the actual paired baseline is
`94e7ad2062724626804aebf442609c4ba3fd8e89e4363b9c71f2d4a8d8aaf1ba`.
Wall time rises 4.6%, CPU 10.4%, and RSS about 1 MiB on this extreme workload.
The measured CPU cost remains a qualification boundary, not an exemption as
noise or a passed no-difference gate. Representative framework extraction is
qualified separately in Difference 17, without eliminating this stress result.
Raw per-process records remain private.

The first online candidate used the Chromium-brand and language configuration
from the new Chrome200 anchor. It returned `DOCUMENT_LIFECYCLE_FAILED` after
autonomous task-budget failures and repeated Deno stack-frame/stack-overflow
warnings. No shopping POST or card input was reached, and no payment was made.
This result is not a shopping status or proof that the new Script op caused the
failure. A repeated same-configuration D16 retry failed at the same lifecycle
boundary. The paired D15 control loaded document HTTP200, dismissed the Cookie
banner and issued five shopping POSTs, all HTTP403, without reaching card input.
A local task/microtask and exception-stack fixture has identical ordinary
ordering in D15 and D16 and does not reproduce the warnings. A separate native
V8 Profiler sampling build captured a valid CPU profile at the microtask
checkpoint timeout; its three start commands and stop response were acknowledged.
Its temporary inspector instrumentation is excluded from the delivery diff.
The profile's repeated iframe-construction edge is qualified separately in
Difference 17; it is not a cause attribution for shopping403.

## Main merge and first-shopping recheck, 2026-09-30

Remote main was fast-forwarded from `6a27ce7` to `172443f`. Existing untracked
work was preserved. The exact release-render CLI build passed; its SHA-256 is
`9327c50aaa5a0c384f2aaebb74e882c89bf7bc5a36a3194531726a29c2c9401d`.
The official Playwright/CDP retry returned document HTTP 200 and first shopping
HTTP 403; the page subsequently issued three further shopping requests, all 403. No
credit-card input was present and no payment was submitted.

A separate Chrome for Testing153 incognito window, started and operated with
computer use, returned first shopping HTTP 200 on the same LGA-MDW 2026-10-27 search.
Its lowest-fare earliest option was WN2341, 07:25-08:45, Basic 138.40 USD including
taxes. Raw-byte NetLog and a DevTools Performance trace with resource content
were saved privately. The trace contains JS samples, function-call and
script-evaluation events; it is not a transcript of every executed instruction.
System proxy configuration was 127.0.0.1:7890, matching the explicit Obscura
proxy. Chrome's default third-party-cookie blocking was left unchanged.
Viewport, locale and session inputs still need qualification before treating
this pair as fully controlled. Neither browser completed checkout in this run.

The historical Chrome 200/D14 first shopping 403 comparison found identical 355
request-body bytes, equal ordinary header-name sets and 17 equal allowlisted
header values. All 26 Chrome Cookie names were also present in D14; D14 added
`tgt_experience` and `_gcl_au`, both written by JS before shopping. Eight Cookie
values in each browser were traced to that session's earlier Set-Cookie;
the remaining 20 D14 names had earlier native JS write events. Different session
values are not an established browser defect. The ordinary application/vendor/
locale script response digests also matched. A missing script-execution event
is a capture-coverage limitation, not evidence that the script did not run.
Obscura's recorded request headers remain prepared transport headers, not
proof of final encrypted H2 order. No new engine repair is claimed by this
analysis; the newly captured Chrome 200 is the next comparison anchor.

## Difference 15: default image request priority

Status: bounded Chrome153 network-parity repair. Online checkout remains incomplete.

The historical successful Chrome153 NetLog sent 12 image HEADERS before the
shopping POST; all 12 had HTTP/2 weight 147 and `Priority: i`. Eight paths
also occurred as image requests in D14 Obscura's pre-shopping history. D14
prepared those image requests with `Priority: u=0, i` and the transport's
default HTTP/2 weight 256. Chromium's resource fetcher classifies ordinary
images as low priority, with separate visibility and priority-hint upgrades;
this repair handles only the typed ordinary-image default. It does not add a
site-specific rule or infer priority from a URL suffix.

Browser-owned `Image` requests now default to `Priority: i`, initial H2
weight 147, and the low dependency band for MacChrome153. An explicit
Priority field remains untouched. Native untyped requests and non-image
resources retain their previous defaults. The local H2 frame test verifies
the five-request weight/dependency distribution, including an image, and a
separate test verifies the prepared field and explicit override.

An independent local TLS/H2 receiver decoded the actual image HEADERS from
both the D14 baseline and this binary, interleaved ABBA twice. Each of eight
connections contained 404 image requests without receiver errors. All 1616
baseline image HEADERS had weight 256 and `Priority: u=0, i`; all 1616
candidate HEADERS had weight 147 and `Priority: i`. The fixture uses an
invalid image body to exercise request and error-event transport, not decoder
or visual fidelity. Four hundred timed samples per arm yielded median
6.942/6.846 ms, p95 8.395/8.378 ms, median RSS 48.180/49.125 MiB, and
median CPU 194.413/179.540 ms per process (baseline/candidate). No
performance regression beyond the approximate ten-percent noise floor is
established.

The focused three tests, all 210 release `obscura-net` tests, the complete
release-render suite (2435 passed, four existing skips), both exact CLI
builds, and the offline obstacle course (33/33) passed. Render SHA-256 is
`20667c86e0537afbc8402dae327e6f61b767b7dc62e98c18a47932edc565dc72`.
No new online shopping request or payment was made during that validation.
The subsequent main-merged online recheck is recorded above.

As a separate control, the historical Chrome shopping200 and a fresh
isolated Chrome153 shopping403 had identical allowlisted ordinary request
headers in the same observed order, plus identical H2 weight220/parent0.
Fresh session state still differs. The image correction is a real general
wire difference, not proof that it caused or eliminates the 403; an empty
Obscura credit-card page remains unverified.

One further server-visible image difference remains by design: Chrome's
`Accept` advertises AVIF, while Obscura omits it because its image decoder
cannot paint AVIF. Advertising an unsupported format would be a false
capability claim, so this comparison does not change `Accept` or claim full
request parity.

### Next comparison boundary

Start at the first shopping POST, pairing the historical Chrome200 request
with Obscura's first shopping403 on the same route and form input. Compare
method, exact URL, request-body bytes/digest, ordinary header names/values
and actual wire order, H2 metadata, and Cookie names plus private value
digests. Obscura's native history captures its prepared transport request;
it is not proof of final encrypted H2 header order. For each differing
dynamic header or Cookie, trace backward to its producing script or response
and the earlier request that supplied that input. Reproduce any underlying
browser-semantic defect with a local fixture before changing engine code.
Keep raw private captures intact; do not replay Chrome's session data or
modify site protection scripts. The fresh Chrome153 shopping403 control means
the historical 200 alone cannot isolate current server/session effects.

## Difference 14: active HTTP/2 HEADERS dependency tree

Status: qualified bounded Chrome153 initial-dependency repair. Online checkout remains incomplete.

The successful Chrome153 NetLog's ordinary H2 session shows non-root parents
for concurrent resource HEADERS. D13's 29-request normal-CA receiver had root
parent 0 on every HEADERS, despite matching weights for known document,
Fetch/XHR and classic-script requests. The [pinned Chromium153 dependency
model](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/net/spdy/http2_priority_dependencies.cc)
uses the most recently active stream in the same or a higher-priority band,
or root 0. Weight, exclusive flag, RFC 9218 Priority field and dependency
band are separate surfaces. The successful shopping request itself was
stream201, parent0, weight220 and exclusive; this is not a known difference
on that request or evidence explaining its 403.

MacChrome153 browser-owned requests now carry a typed legacy band independent
of arbitrary frame weight. Known Document/default, Fetch/XHR and loader-known
classic Script paths use their measured bands. The H2 connection resolves the
parent when initial HEADERS actually leaves the queue, then tracks active
streams by indexed per-band sets; canceled, closed and removed streams cannot
remain parents. Other profiles, H1/H3, request bodies, identity and the TLS
repairs are unchanged. No dynamic reprioritization API is claimed. Other
resource priorities, including native weight110 cases, are not yet modeled,
so this is not whole-connection Chrome parity.

The first implementation computed the parent while a request could still be
queued and scanned every active stream on each creation. Independent Standards
and Spec reviews required fixes. The final version computes at the actual
send boundary and uses indexed cleanup; both reviews found zero remaining
required issues. Tests cover the four-request actual wire order, a peer
concurrency limit of one with the predecessor both completing and being reset,
connection isolation, and indexed cleanup of 4096 active streams. Initial
wire-test expectations incorrectly assumed request-construction order matched
HEADERS send order; the test now checks the observed order against the
Chromium band rule.

Network release coverage passes 209/209; full release-render nextest passes
2434/2434 with four existing skips. The vendored H2 suite passes 75/75 tests
that have local data. Its other 241 HPACK fixture tests cannot pass because
their JSON fixtures are absent from the vendored tree; the unfiltered run
failed only on those missing files. Both exact CLI builds pass. Render SHA-256
is `a92466a849895a52f071516081b805aa990f05cf346f05c39767e95e0b9ffa82`;
no-render is
`2d1c640f19e31a770e23338337ca5c6e640d4c22d508ce862324bd83245df1f1`.
The workspace executable was restored to the verified render hash; the
obstacle course remains 33/33.

Both render and no-render normal-CA H2 receivers decoded 29 requests with 25
completed scripts and no errors. Seventeen HEADERS now had non-root parents;
the D13 receiver had zero. The separate Rust wire test proves the parent is
chosen after queue release, including peer reset, rather than merely
inspecting a prepared request. No protection payloads were copied or decoded.

An isolated ABBA twice compared D13 and D14 on the same persona and trusted
H2 fixture, with four concurrent classic scripts per sample. Each arm has
400 measured samples. All eight receiver connections had 405 requests with
the same weight distribution and no errors. The median evaluation was
1.562/1.583ms, p95 1.923/1.915ms, median RSS 51.484/50.844MiB, and median
CPU 158.792/161.023ms per process (baseline/candidate). No regression
outside the approximate ten-percent noise floor is established.

Fresh official Playwright/CDP online retry with the qualified render hash
received document200 and two shopping403 responses. Card controls were zero,
no payment was submitted, and both context histories drained to
`hasMore=false` (`728` owned, `0` default). The general wire difference is
real and repaired; it has no demonstrated online effect, and the requested
checkout remains incomplete.

A later, separately isolated, headed official Chrome for Testing 153 control
used the same proxy and route with a fresh temporary profile. Its complete
private NetLog recorded shopping stream185, parent0, weight220 and response403.
That first control's default Accept-Language was `en-US,en;q=0.9`, unlike
the historical Chrome-success trace and Obscura's `en-US`, so it was not a
fully matched comparison. A second isolated Chrome control explicitly set
`en-US`. Playwright observed document200 and shopping403 without a card page;
its complete NetLog independently recorded that request on stream179,
parent0, weight220, Accept-Language `en-US` and response403. A later shopping
send on stream209 had no captured response before closure. Both controls had
context-close delays, which the second bounded; no payment was submitted.
The older successful Chrome200 trace remains a historical reference, not a
contemporaneous Chrome-success/Obscura-failure pair. Fresh profiles still
differ in session state, so the current shared 403 does not prove identical
browser behavior or identify an Obscura-specific cause. No further rapid
live retries are justified.

## Difference 13: GREASE ECH outer length buckets

Status: qualified bounded Chrome153 length repair. Online checkout remains incomplete.

The successful Chrome153 ordinary H2 ClientHello has outer ECH extension
length218. That is one of four lengths supported by the pinned
[BoringSSL construction](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/encrypted_client_hello.cc):
base128/160/192/224 plus AEAD overhead, yielding observed extension totals
186/218/250/282 for this suite. A single native handshake does not establish
the distribution. Across 808 owned pre-repair receiver connections the
extension covered all 97 integer totals 186–282; only 33 landed in the four
native buckets. Those samples include both sides of the previous D10
performance comparison and must not be attributed to one binary alone.

Only the verified Chrome153 GREASE ECH length selector now uses a native-sized
secure random word modulo four. Older majors and other browser families retain
their prior length strategy. Real ECH, HPKE, config IDs, cipher payload bytes,
server trust and the prior GREASE/padding repairs are untouched. Neither
protected-site payloads nor opaque ECH bytes are copied or decoded.

Three focused release tests assert all four outer extension lengths, legacy
profiles, the unchanged HRR wire length, fresh-connection selection, safe
random failure and trusted/untrusted TLS12/13 negotiation. They inspect only
outer extension sizes. The initial test run passed 11/12; its default fixture
server requested a group not offered by the deliberately narrowed client
provider. Restricting the fixture server to mutually supported X25519 fixed
that test setup, and the final focused set passes 12/12. Full network release
coverage passes 208/208. Independent Standards and Spec reviews each found
zero required issues. This does not prove real ECH acceptance or shopping403
causality.

Full release-render nextest passes 2433 tests with four existing skips;
no-render network release coverage passes 208/208. Both exact CLI builds pass.
The render binary hash is
`27202a0efae7b2db94f2c322ae1fe4dad032430fc8c925d9c8c72030d836cb1d`;
no-render is
`278385c57cf82120363dd91edd385d5c9d1776b73eac4cc0945eecf9cb8580ae`.
The workspace executable was restored to the verified render hash. The
obstacle course remains 33/33.

Both normal-CA H2 receivers decoded 29 requests and completed 25 scripts.
Each build passed 15 direct pinned-BoringSSL cases plus the official
Playwright/CDP two-connection resumption case. TLS12/13, HRR, response
padding, malformed/unsolicited rejection and untrusted-certificate rejection
kept their expected outcomes. Actual plaintext ClientHello metadata showed
only the four native outer ECH lengths, with the same length across each HRR;
the D12 GREASE bodies and resumed PSK-last invariant persisted. Opaque ECH
bytes were not inspected.

Isolated ABBA twice compared D12 and D13 with the same persona, zero
server-padding request and one fresh connection per navigation. Of 404
baseline connections, 15 used a native ECH length bucket; all 404 candidate
connections did. Each received the correct document request. The median
navigation was 5.672/5.654ms, p95 6.228/6.118ms, median RSS
40.719/40.813MiB and median CPU 898.115/896.157ms per process
(baseline/candidate). No regression outside the approximate ten-percent
noise floor is established.

Fresh official Playwright/CDP online retry with the qualified render hash
received document200 and two shopping403 responses. Card controls were zero,
no payment was submitted, and both context histories drained to
`hasMore=false` (`627` owned, `0` default). The wire difference is repaired,
but no online effect has been demonstrated and checkout remains incomplete.

## Difference 12: nonempty second GREASE extension

Status: qualified bounded Chrome153 repair. Online checkout remains incomplete.

The successful Chrome153 ordinary H2 ClientHello sends two GREASE extensions
with bodies of lengths `[0,1]`. D11's actual render/no-render H2 receivers and
owned BoringSSL TLS12/13, HRR and resumed connections still showed `[0,0]`.
The single native connection does not establish shopping403 causality. Pinned
[BoringSSL extension construction](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/extensions.cc)
emits an empty first extension and one zero byte in the second, for both
ordinary ClientHello and ECH inner construction. A resumed PSK may follow it
and must remain the final extension.

Only the already verified Chrome153 branch now supplies that byte; other
browser families and majors retain their existing empty body. GREASE values,
extension ordering, PSK placement, real ECH bytes, TLS verification and D10
server padding are unchanged. Shared actual-wire assertions cover both
payloads through the existing TLS12/13, HRR, trusted/untrusted and padding
matrix, plus all sixteen GREASE values, signature-prefix settings and old
profiles. Independent Standards and Spec reviews both found zero required
issues.

Focused release coverage passes 9/9 and full network release coverage passes
205/205. Full release-render nextest passes 2430 with four existing skips;
no-render network coverage passes 205/205. Both exact CLI builds pass. The
render binary hash is
`aa97bd7c7db0ddd297797772173c9305038dc6d542daff54ee32e804fbfb11fe`;
no-render is
`2e0d261e6d0c8ecfc8dbf3f2cca3e2f330b0669e12eac826ca9f3509a89dae4a`.
The workspace executable was restored to the verified render hash. The
obstacle course remains 33/33.

Both normal-CA H2 receivers decoded 29 requests with 25 executing scripts,
the existing two-byte zero server-padding request and exactly `[empty,00]`
GREASE extension bodies. Both builds passed 15 direct pinned-BoringSSL cases
plus the official Playwright/CDP two-connection resumption case; trusted
TLS12/13, HRR, ignored/returned optional padding, malformed/unsolicited
responses and certificate rejection kept their expected outcomes. The
matching-source independent TLS client confirmed typed negative causes and
alerts where the production connector does not flush them. Actual HRR wire
metadata kept the six GREASE values; resumed PSK remained the final extension
while the one-byte second GREASE body persisted.

Isolated ABBA twice compared the D11 baseline with this D12 binary, keeping
the same persona, explicit zero server-padding request and a new connection
per sample. All 808 receiver connections held one correct document request:
the 404 baseline second GREASE bodies were empty, the 404 candidate bodies
were single zero bytes. The median navigation was 5.923/6.001ms, p95
6.813/7.195ms, median RSS 41.453/41.469MiB and median CPU
938.305/948.523ms per process (baseline/candidate). No regression outside
the approximate ten-percent noise floor is established.

Fresh official Playwright/CDP online retry with the qualified render hash
received document200 and four shopping403 responses. Card controls were zero,
no payment was submitted, and both context histories drained to
`hasMore=false` (`603` owned, `0` default). The parity repair is real; its
online effect is not demonstrated and the requested checkout remains blocked.

## Difference 11: Chrome153 independent GREASE handshake entropy

Status: qualified general Chrome153 GREASE entropy repair. Online checkout remains incomplete.

The successful native ordinary H2 connection includes group 0x0a0a and a
cipher GREASE value equal to its second extension GREASE value. All 808 owned
D10 receiver connections instead exclude 0x0a0a from their first five
categories, force those categories to be distinct, and match the legacy
prediction from public ClientHello random. Different peers and one selected
native connection do not establish shopping403 causality.

The [pinned BoringSSL handshake](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/handshake.cc)
draws eight independent bytes once. Its
[category indices](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/internal.h)
map cipher, group, first/second extension, version and signature to slots
0/1/2/3/4/7. All sixteen GREASE values are possible and categories may coincide;
only the second extension colliding with the first is corrected by XOR0x1010.
Chrome major153 now uses a private snapshot of these values on ClientHelloInput.
HRR reuses it, new handshakes redraw, and RNG failure propagates without a
fixed fallback. Other browser families and majors retain their previous path.
The signature prefix remains separately opt-in and never becomes a real
verification capability. This supersedes D9's public-random-derived signature
implementation only on the verified Chrome153 path. Ticket/ECH GREASE slots
and opaque ECH generation are not changed or claimed equivalent.

The shared actual-wire parser removes D10's optional duplication suggestion.
Release network coverage passes 205/205, including all sixteen values,
allowed cross-category equality, the mixed native pattern, independence
from ClientHello random, HRR reuse, fresh-handshake redraw, safe RNG failure
and unchanged non153 profiles. Existing TLS12/13 trust and padding assertions
remain. Initial fixture failures incorrectly expected a GREASE keyshare in
the requested-group HRR and then overlooked the cached group hint on a second
connection. Native [key-share setup](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/extensions.cc)
omits GREASE for an overridden retry group. The final fixture retains this
absence assertion and disables only its learned group cache to exercise both
retries. Independent Standards and Spec reviews each have zero required findings.

Final focused release coverage passes 9/9; full network release coverage passes
205/205. The full release-render suite passes 2430 tests with four existing
skips. Both exact CLI builds pass, and no-render network release coverage
passes 205/205 with one reported leaky test. The verified render binary hash
is `426268d3c628dc6083ff8d8abc2d8f0d3d8bd47bc12b1854256973fbdece1093`;
the no-render hash is
`d12917be4d66ef9833c5be97faa624c666c8853bc40c291cc33c7b01d98d9538`.
The workspace executable was restored to the render hash.

Both builds passed the normal-CA H2 receive-side probe: 29 decoded requests,
25 executed scripts and the existing optional zero padding request. Each
passed 15 direct owned pinned-BoringSSL cases plus a two-connection resumption
case: TLS12/13, HRR, request size bounds, omitted versus present responses,
malformed/unsolicited failures and untrusted-certificate rejection. The
independent matching-source TLS client confirmed the typed causes and alerts
on negative cases; the production connector alone does not flush its fatal
alert. Plaintext ClientHello metadata from both builds confirms a first
GREASE keyshare equal to its group, HRR reuse of all six categories and
omission of the GREASE keyshare on requested-group retry. Resumed PSK remains
the final extension. The second GREASE extension still has an empty body,
separately tracked as D12. The obstacle course remains 33/33.

Isolated ABBA twice used the D10 binary and D11 binary with the same persona,
explicit zero server-padding request and a fresh TLS connection per navigation.
All 808 actual receiver connections had one correct document request and
zero padding request. In 404 baseline connections all first-five categories
were distinct, no category used 0x0a0a and all six matched the legacy public
ClientHello-random prediction. In 404 candidate connections every one of the
six categories exercised all sixteen values, 107 connections contained
0x0a0a, 188 had a cross-category collision, and none matched that legacy
prediction. The resulting median navigation was 5.869/5.826ms, p95
6.932/6.274ms, median RSS 41.188/40.836MiB and median CPU
932.285/917.277ms per process (baseline/candidate). These differences are
within the approximate ten-percent measurement noise, not a speedup claim.

The fresh official Playwright/CDP online retry used the qualified render hash
and the same explicit zero-padding persona. It received document200 and two
shopping403 responses; card controls remained zero and no payment was
submitted. Both context-history cursors were drained (`685` owned, `0`
default, `hasMore=false`). This repair removes a measured browser-facing
distribution mismatch but does not establish full equivalence or explain 403.

## Difference 10: optional TLS server handshake padding experiment

Status: qualified general opt-in padding support. Online checkout remains incomplete.

The successful Chrome153 ordinary-traffic socket has extension 0x12e0 with a
two-byte unsigned zero request. This differs from no extension. Chromium153
[feature defaults](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/net/base/features.cc)
disable this experiment; the [SSL configuration](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/net/ssl/ssl_config_service.cc)
passes an optional u16 request. One observed connection does not establish how
the feature was enabled or show that its absence caused the shopping 403.

The default-None `tls_server_padding_request` persona field now reaches the
primp builder and typed ClientHello extension. Zero explicitly sends a request.
It changes the identity digest, not the device seed, survives transport
rebuilding, and remains fixed for the context. No preset enables it implicitly.
The [pinned BoringSSL extension implementation](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/extensions.cc)
permits omitted responses but requires an offered request and exact response
length in TLS13 EncryptedExtensions. It does not require every byte to be zero.
Requests above 16384 may be ignored by the server without truncating the u16
client option. Certificate and hostname verification are unchanged.

The first Spec review found a Certificate-stage omission. Exact native
[Certificate parsing](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/tls13_both.cc)
rejects this extension there; the repair now does so without changing unrelated
unknown-extension behavior. Native [CertificateRequest and NewSessionTicket](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/tls13_client.cc)
ignore unknown code points, so the original over-broad private specification
was corrected and those compatibility paths were preserved. Plaintext
ServerHello, HRR, unsolicited EE, wrong-length EE and duplicate EE stay rejected.

Failed qualification attempts are retained: obscura-net has no render feature;
dependency unit tests cannot run as host workspace members; standalone vendored
unit tests lack upstream testdata/test-ca; the first Certificate callback check
used the wrong enum conversion. The final standalone test copy uses the same
product source with only official same-version excluded fixtures restored in a
temporary directory, not a changed dependency or system certificate store.

The final network release suite passes 201/201. Three standalone TLS unit
tests pass against the identical vendored source with product-aligned locked
dependencies and official excluded test fixtures. They cover exact request
encoding, omitted versus zero, all u16 boundaries, HRR persistence, default
digest goldens, transport inheritance, wrong plaintext ServerHello, duplicate
EE and message-specific compatibility. Normal trusted and untrusted TLS12/13
handshakes remain covered; no full FIPS-certified run is claimed.

An owned server built from the exact pinned BoringSSL revision supplies real
encrypted TLS responses under a temporary trusted localhost CA. The render
binary passes 15 direct cases plus an official Playwright/CDP two-connection
resumption case: zero/1/32/16384 response lengths, ignored requests over the
server bound, omitted responses, TLS12, HRR, wrong-length, unsolicited and
untrusted-certificate failures. Resumption is confirmed by the server and
omits the padding response. The production connector's outer error omits the
TLS cause and closes before flushing its fatal alert; an independent client
using the same vendored TLS code and aligned dependency versions confirms the
typed causes and wire alerts 50, 110 and 48. A first probe wrongly asserted an
inner-cause string; a later CDP fixture omitted the frozen device scale factor.
Both failed attempts remain separate from the final evidence.

Owned normal-CA H2 receivers confirm exactly one two-byte zero extension when
enabled and no extension when omitted, with the prior 29-request/25-script
assertions still passing. Independent Standards and Spec reviews have zero
required findings; Standards retains one optional test-parser duplication
suggestion. The release render binary passes the obstacle course 33/33.
Full release-render nextest passes 2426 tests with four pre-existing skips.
No-render network release coverage passes 201/201; its actual H2 and all sixteen
BoringSSL fixture cases also pass. Both exact release builds pass and the
render binary is restored to the verified hash. Existing constructor defaults remain
unchanged; external literals of public PersonaSpec, TransportParams or
BrowserEmulator structs must supply the new optional field when upgrading.

Isolated ABBA repeated twice compares the D9 baseline with padding omitted
against this repair explicitly requesting zero. All other measured inputs are
fixed; the intentional experimental option is the configuration delta. Each
navigation creates a fresh context and TLS connection. With 400 samples per
binary, median navigation is 5.898/5.874ms, p95 6.402/6.280ms, median RSS
41.219/40.789MiB and CPU 927.698/927.655ms per process. All 808 receiver
connections contain one correct document request; baseline padding is absent,
candidate padding is exactly two zero bytes. There is no observed regression
outside the approximately ten-percent noise boundary, not a speedup claim.

The fresh official Playwright/CDP online context with the explicit zero request
still receives document200 and three shopping403 responses, with zero card
controls and no payment submission. Complete private histories and bodies are
retained. This fixes the measured extension and validates its response handling;
it does not prove complete browser equivalence or explain the remaining 403.

## Difference 9: wire-only TLS signature_algorithms GREASE

Status: qualified general signature GREASE repair. Online checkout remains incomplete.

The allowlisted ordinary traffic in the successful Chrome for Testing 153
NetLog shares H2 session 1015 and socket 1009. Its ClientHello extension 13
contains one GREASE value followed by eleven real schemes, using 26 payload
bytes. The owned pre-repair Obscura receiver records 24 bytes with the same
real sequence but no GREASE prefix. The peers differ and only one native
connection is selected, so this does not prove total TLS equivalence or explain
the shopping 403.

Exact Chromium 153 [feature defaults](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/net/base/features.cc)
enable kTlsGreaseSigalgs, and its [socket context](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/net/socket/ssl_client_socket_impl.cc)
enables the corresponding SSL_CTX setting. The [pinned BoringSSL implementation](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/extensions.cc)
prepends the value to the transmitted list. Its [handshake lifecycle](https://raw.githubusercontent.com/google/boringssl/defe5810ee8be430bcdeccf46a199bec0e93abdb/ssl/handshake.cc)
draws fresh handshake entropy and reuses it across HRR. Signature GREASE need
not differ from other categories and includes all sixteen RFC 8701 values.

BrowserEmulator gains a separate default-off wire option; only the verified
primp Chrome153 profile enables it. The staged wire list adds a prefix after
existing FIPS/verifier selection, without changing real schemes or verification
capabilities. Client random supplies the independent value and remains stable
across HRR. Other profiles, other GREASE categories, padding and trust behavior
are unchanged. No full FIPS-certified execution is claimed by ordinary fixtures.

The three focused release tests passed, followed by all 197 network tests.
They cover every valid GREASE value, unchanged real order, default-off
configuration, trusted and rejected TLS1.2/TLS1.3 certificates, HRR reuse and
refusal to verify GREASE signatures. The initial dev-dependency import used
the package name rather than its library alias and failed compilation. The
next run exposed a fixture parser reading encrypted TLS1.2 records after CCS
as plaintext. Both failures remain retained; the corrected parser distinguishes
TLS1.2 encryption from TLS1.3's compatibility CCS. Independent Standards and
Spec reviews each report zero required findings. The owned receive-side
baseline probe fails the new 26-byte assertion, recording 24 bytes and the
unchanged eleven real schemes.

The full release/render run passed 2422 with four skipped. Both exact CLI
builds completed; the no-render network run passed all 197 tests. Each
normally validated owned H2 probe decoded 29 requests, observed 25 executing
scripts and received a 26-byte signature extension containing exactly one
legal GREASE prefix and the unchanged eleven real schemes. The obstacle
course remains 33/33. The restored render hash matches the preserved candidate:
ea6d027b8807714598cd52a6e10644ae072ff9093ac88897f7d0187071ac5484.
The no-render hash is
e390875f4adc6faf10102e69b30103cf810fb1b5e867437b7dbc33833634a78a.

An isolated ABBA comparison timed fresh validated TLS/H2 document navigations,
with a new browser context per sample. The initial 80 samples per binary had
baseline/candidate median 6.008/5.919 ms and P95 6.493/7.145 ms; that tail was
approximately ten percent higher, so a separate larger run was retained.
The 400-sample comparison measured median 5.818/5.789 ms, P95 6.522/6.215 ms,
median RSS 41.359/40.750 MiB and child CPU 917.773/912.400 ms. This is within
the approximate ten-percent noise floor, not a performance improvement.
Its receive-side evidence contains 808 completed one-request connections,
404 per binary, with baseline payload length 24 and candidate length 26.
All sixteen valid candidate GREASE values were observed. Both raw comparisons
and their actual ClientHello captures remain retained.

The fresh Chromium-flavor online retry received document 200 and three
shopping 403 responses. Raw histories drained to hasMore=false: 545 owned
context records and zero default-context records. Card controls remained zero
and no payment was submitted. A valid TLS fixture or matching protocol field
does not establish checkout success. The new BrowserEmulator field requires
external struct-literal callers to supply it; existing constructors default off.

## Difference 8: known classic Script scheduling and protocol-late fields

Status: qualified general classic Script repair. Online checkout remains incomplete.

The native Chrome for Testing 153 pure-HTTP fixture completed 21 unique
ordinary scripts; none of their received H1 requests had a Priority field.
The retained pre-repair Obscura fixture completed eighteen classic scripts,
but every received H1 request carried an automatic field. Its normally
validated H2 fixture also reproduced weight 256 where known ordinary script
loading contexts require 220 or 147. This is a general loader/transport delta,
not evidence that it causes the shopping rejection.

The exact Chromium 153 [script loader](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/third_party/blink/renderer/core/script/script_loader.cc),
[priority computation](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/third_party/blink/renderer/platform/loader/fetch/resource_fetcher.cc)
and [potential render-blocking predicate](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/third_party/blink/renderer/core/html/html_script_element.cc)
distinguish parser-blocking classic scripts from lazy async/defer/dynamic work.
High hints elevate lazy work, and potential render blocking overrides low
hints. Dynamic async=false remains lazy for network priority. Script delivery
is nonincremental. The [H2 field generator](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/net/spdy/spdy_http_utils.cc)
preserves an explicit field and omits the default urgency/nonincremental value.

Parser and dynamic loaders now snapshot the known classic context into typed
ScriptPriority. Script fetchPriority IDL reflects the existing content
attribute. Known Script requests stop generating the older automatic field
in the JS metadata, resource wrapper and default merge, without deleting
caller-supplied fields by their value. A typed primp extension is carried
through request cloning and rebuilt redirect hops. Only the H2 send projects
it into that send's local headers, keeping automatic values out of the source
redirect snapshot. High uses weight 220 and u=1; Low uses weight 147 and no
automatic field. H1 generates neither. Contextless Script, module graphs,
Fetch/XHR and other resource defaults remain unchanged. H3 parity, preload
scanning, full parser preparation timing and rendering-blocking lifecycle are
not claimed repaired.

Focused release/render regression passed seven of seven. The first compile
found a missing public API doc comment; the next found an unqualified type in
a new test. Both failures were retained and corrected before qualification.
Independent Standards and Spec source reviews report zero required findings.
The exact render CLI built with SHA-256
c8420861d3814f18dabef691b02cfb40ae1b389cf0f1e948094c0986d96d2483.
The obstacle course remains 33/33. The official Playwright pure-HTTP probe
completed all eighteen classics with no automatic H1 field. Its validated H2
receiver decoded 29 requests and observed 25 executed scripts, expected
220/147 weights, high/low field behavior, three explicit field values including
the old default and empty value, and no automatic field on an H2-to-H1 hop.

A separately linked direct primp probe received eleven H2 and two H1 requests,
verifying clones, automatic and explicit redirects, REFUSED_STREAM replay,
default reset, and incremental-field serialization. The initial private
link used a different Tokio feature instance and failed before sending; the
retained corrected probe uses the dependency fingerprint's actual runtime.
The first pure-HTTP private probe requested scale factor 1 against a frozen
scale-2 persona and failed before creating a page; its corrected pre-repair
run is the actual H1 red proof. Raw failures were not overwritten.

The full release/render run passed 2419 with four skipped; a serial rerun after
restoring the exact render CLI also passed 2419 with four skipped and one
existing leaky-test warning. Render and no-render share the CLI output path,
so this final serial gate avoids attributing results to a mixed artifact.
The exact no-render CLI built with SHA-256
998f5a491c71aaaf543e126b688a3f9be50ffbe33c085c35d43d303dde58578f;
203 relevant no-render tests passed, with 529 unrelated cases filtered out.
Both no-render wire probes passed the same eighteen-script H1 and 29-request
H2 assertions. The restored render hash matched the preserved candidate.

An isolated H2 ABBA comparison exercised four compressed dynamic classic
scripts per sample, using 400 samples per binary and eight connections with
405 actual requests each. Baseline/candidate median was 1.602/1.597 ms, P95
1.907/1.940 ms, median RSS 51.914/51.125 MiB and child CPU 164.874/162.772 ms.
These changes are within the approximate ten-percent noise floor, not a
performance improvement. Decoded receive-side headers and weights confirm
the candidate's high/low behavior and the retained baseline's older defaults.

The fresh Chromium-flavor online retry received document 200 and three
shopping 403 responses. Native histories were drained to hasMore=false:
491 records for the owned context and zero for the default context. Card
controls remained zero and no payment was submitted. Full raw histories,
bodies and snapshots remain private. Adding script_priority to the public
ResourceRequest struct requires external struct-literal callers to supply
the new field; constructor callers retain their default None behavior.

## Difference 7: explicit browser distribution branding

Status: qualified general flavor repair. Online checkout remains incomplete.

The successful native Chrome for Testing 153 NetLog sends Chromium 153 and
Not_A Brand 8. A separate owned pure-HTTP page, opened using native UI without
enabling remote inspection, directly confirmed that same two-brand order in
navigator.userAgentData and its fullVersionList. Qualified Obscura instead
uses branded Chrome's three-brand list. The exact Chrome 153
[brand generator](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/components/embedder_support/user_agent_utils.cc)
distinguishes optional product branding and shuffles two and three brands
using different destination permutations.

PersonaSpec now has typed browser_flavor values chrome (default) and chromium.
Default Chrome is omitted during Spec and EffectivePersona serialization,
preserving existing canonical bytes and digests. Three legacy preset digests
were read from a retained pre-flavor compiled library and added as goldens.
Chromium flavor participates in the digest but does not change the device seed.
One Rust generator supplies both the ordinary Sec-CH-UA field and runtime JS
brand list; JS getters return fresh copies and high-entropy brands use that same
order. Frame identity copying, worker persona inheritance, all transport
rebuild paths and frozen-persona matching carry the flavor.

The first focused compile caught a mechanical field placement error in a
matches macro. It was corrected before qualification, with the failed raw
compile retained. The next compile found an unqualified Arc in the new worker
test, corrected without changing its assertions. Both failed raw builds remain
retained. The first executed focused run passed seven of eight; the new frame
test used statements where its expression API required an IIFE. Correcting
that test and strengthening its realm assertions then reproduced a separate
P2: copied parent brand arrays and their map species returned foreign-realm
arrays. Independent Spec review confirmed the path. The getter now allocates
a local array literal and copies entries using a loop. Value, order, mutation
isolation and low/high-entropy array/object realm assertions are retained.
Standards and Spec final source reviews each report zero required findings.
The corrected focused release/render cases passed eight of eight, including
the low/high-entropy frame realm assertions. Broader JS/network release/render
coverage passed 836; the full release/render run passed 2416 with four skipped.
The exact render CLI built and the obstacle course remained 33/33.

The owned pure-HTTP official Playwright/CDP probe received seven requests for
each flavor. Document, Fetch, XHR, static frame and worker fields agree with
their page/frame/worker public identity, with unchanged Chrome three-brand and
opt-in Chromium two-brand values. Initial dynamic-frame probes failed twice
to locate the loaded frame via Playwright's frame APIs; a third retained run
completed Chrome but timed out waiting for Chromium's frame message despite
receiving its identity POST. The qualified static-frame probe does not claim
dynamic-frame enumeration or message delivery fixed. Both normally validated
owned H2 flavor probes decoded six requests with the expected actual Sec-CH-UA
fields, unchanged weight/body/CL behavior, completed streams and an executing
script. The full native Chrome reference remains separate; its patch and
platform versions are not claimed matched. The exact no-render CLI built;
199 relevant no-render tests passed, with 530 unrelated cases filtered out.
Both flavors passed the same fourteen-request static-frame identity probe in
no-render. Its validated H2 Chromium probe completed all six requests with
the same two-brand fields and unchanged body/weight behavior. The exact render
build was restored and its hash matched the preserved render candidate.

The render SHA-256 is
8b662e7dc83c262196ab7cdd64a2570c6fdbf08da89cdff9b03d41245158b8ba;
no-render is f1b3b85d2f2144e5ce46153e839580e75f0385add6ab5c14f36582e62a98024a.
Isolated H2 ABBA comparisons used 400 samples per binary and eight connections
with 405 actual requests each. Default flavor baseline/candidate median was
2.064/2.096 ms, P95 2.513/2.559 ms, median RSS 59.336/59.445 MiB and child CPU
203.448/205.605 ms. Opt-in Chromium comparison was 2.061/2.058 ms, P95
2.528/2.585 ms, RSS 58.984/59.359 MiB and CPU 204.777/205.174 ms. Both are
within the approximate ten-percent noise floor, not performance improvements.
Actual HPACK fields preserve the expected flavor, Fetch weight 220 and
355-byte framing. An earlier run overlapped the online check and was excluded
from qualification; its raw evidence remains. A subsequent private harness
import rejected its label as an integer sample count before starting; that
failure is retained, and argument parsing was restricted to direct execution.

The fresh Chromium-flavor online retry received document 200 and three shopping
403 responses. Native histories were drained to hasMore=false: 672 records
for the owned context and zero for the default context. Card controls remained
zero and no payment was submitted. Full raw histories, bodies and snapshots
remain private and are not replaced by these summaries.

TLS profile, full patch version and platformVersion remain unchanged and may
still differ from the native reference. This branding repair alone does not
establish complete Chrome request parity or the cause of the shopping rejection.

## Difference 6: per-request initial H2 HEADERS weight

Status: qualified general default Fetch/XHR repair. Online checkout remains incomplete.

The successful native Chrome 153 NetLog records Document weight 256, ordinary
scripts 147 or 220, and shopping XHR 220. An owned, normally validated TLS/H2
receiver decoded the qualified pre-priority Obscura binary as weight 256 for
all three fixture requests, although Fetch's ordinary Priority field was u=1,i.
The wire regression fails on that retained baseline. Exact Chrome 153
[Chromium conversion](https://raw.githubusercontent.com/chromium/chromium/153.0.8010.52/net/spdy/spdy_http_utils.cc)
and its pinned
[QUICHE weight conversion](https://raw.githubusercontent.com/google/quiche/2c4a124642f095f995cd2e9e2fe10decc08df662/quiche/http2/core/spdy_protocol.cc)
corroborate the ordinary 256/220/147 values.

Primp now accepts a typed per-request initial HEADERS weight in protocol units
1 through 256, encoded by subtracting one. The extension survives Request
clone, execute-request rebuilding and redirected-hop rebuilding, and is
extracted before H2 clears extensions. It overrides only that request's
weight, retaining the connection's exclusive-root behavior. Shared client
configuration, connection pooling, dependency handling, dynamic reprioritization
and H1/H3 fields are unchanged.

Browser Fetch/XHR defaults select 220 from explicit resource metadata, not
from URL, headers, callbacks or trace state. The typed resource wrapper carries
that metadata on every manually redirected hop. Native unclassified requests
and Document/Script/other categories keep their existing default. Script
async/defer/module/fetchpriority scheduling and explicit Fetch priority hints
are outside this repair; no blanket Script weight is substituted.

The first compile caught an incorrect http2 feature gate: primp's H2 dependency
is mandatory and no such crate feature exists. The erroneous conditions were
removed before qualification. Four focused cases then passed; expanded full
release network coverage passed 188. Actual reception on one connection covers
17 requests: explicit weights and bounds, native default restoration, clone,
302 extension replay, 355 DATA bytes, and typed browser requests without
callbacks or trace. Separate H1 reception retains the explicit Priority field.
The H2 explicit-field assertion is prepared capture, not HPACK-decoded field
proof. Standards and Spec final source reviews each report zero required
findings. Focused release/render coverage passed 829. The full release/render
run passed 2409 with four skipped and one retained leaky-test warning from
body_onload_content_attribute_reflects_to_window. Both that case and its frame
counterpart passed unchanged in a focused recheck; the warning is not root-caused.
Both exact CLI configurations built, and relevant no-render coverage passed 191.

Official Playwright/CDP against the normally validated owned H2 receiver
confirmed, in both configurations, Document 256, nonempty and empty Fetch 220,
XHR 220, dynamic Script 256 and a subsequent Document 256. Decoded DATA and
CL/MIME remain correct, every stream ends, and the loaded script executes.
Script scheduling remains an explicit gap, not a claimed Chrome match. The
obstacle course remained 33/33.

Interleaved ABBA H1 measurement collected 80 samples per binary. Baseline and
candidate median/p95 were 2.132/2.732 and 2.098/2.382 ms; median RSS was
53.773/53.523 MiB and CPU 128.984/129.358 ms. Two initial H2 runs collected
80 samples per binary each: medians 1.875/1.912 and 1.879/1.935 ms, with
candidate p95 higher at 3.956 and 2.908 versus 2.464 and 2.452 ms. These tail
observations are retained. An extended run first exposed the private receiver
not replenishing its request DATA flow-control window, failing in the baseline
after 184 POST requests; the failed raw run is retained. After repairing that
receiver, the interleaved extended H2 run collected 400 samples per binary:
median 2.058/2.088 ms, p95 2.513/2.747 ms, median RSS 59.250/58.984 MiB and
CPU 203.315/204.622 ms. Actual wire records confirm the changed weights over
the same pooled connections. Extended-run differences are within the documented
noise floor; no speed improvement or absence of all tail regressions is claimed.

Render SHA256 is
`4a44418a96231baa6aa201ebeebef714a4df0dda3caa1ccb2629ee0c0085eb89`;
no-render SHA256 is
`d14f730435d6160423705e056bede2fefc9078ceb4ac38e27b634b947dd626c8`.
The anonymous online retry received document HTTP 200 and three shopping HTTP
403 responses, with zero card controls and no payment. All 576 nonempty native
history records were retained with hasMore=false, along with the empty history.

This source repair does not establish complete Chrome request parity or a
cause of the shopping rejection.

## Difference 5: browser body extraction, inheritance and empty-body presence

Status: qualified general body extraction repair. Online checkout remains incomplete.

A pure HTTP fixture paired Chrome 154 and the qualified response-header
baseline across 43 Request, Fetch and XHR cases. It reproduced null clearing
an inherited Request body, missing string MIME, GET/HEAD bodies being sent,
XHR GET/HEAD not discarding their body, and absent/empty length differences.
This reference uses Chrome 154 locally; successful native Chrome 153 remains
the independent online reference. No certificate bypass is used in this
fixture.

Request construction now records extracted bytes and presence in a private
WeakMap. Mutable buffers are snapshotted; strings are encoded once. Inheritance
and clone reuse the extracted bytes and do not generate a new FormData boundary
or infer MIME again after headers are replaced. New extraction adds the default
MIME only when Content-Type is absent, including explicit empty strings.
HeadersInit sequences are parsed as pairs; ordinary record fields retain their
original spelling in raw logical metadata. Pair duplicates combine by
case-insensitive name, and a null-prototype store avoids property-name collisions.

Fetch and Request reject GET/HEAD bodies before invoking the network op. XHR
discards such a body instead. Shared method normalization follows the six
legacy ASCII-case rules in the
[Fetch Standard](https://fetch.spec.whatwg.org/#concept-method-normalize);
extension-method case is preserved, and non-ASCII invalid tokens are not
converted into legal methods.

The browser-only network wrapper carries Option body presence through to the
transport. A present empty body, or an absent POST/PUT body, generates CL=0;
absent PATCH/DELETE does not. Legacy native byte-slice wrappers retain their
empty convention, and explicit CL/TE remains untouched. Existing per-hop
redirect state still controls body presence; the redirect method matrix is not
changed here. SSRF, CORS, credential and cookie decisions remain in their
existing paths.

The initial two release JS regressions failed. Independent review found and
closed three P2 cases: repeated init.body getter evaluation, mixed-case XHR
method handling, and Unicode rather than ASCII normalization. Expanded tests
then caught HeaderInit sequence handling and logical raw-field casing; neither
old assertion was weakened. Corrected focused release/render coverage passed
825 across the two affected crates. Standards and Spec final source reviews
each report zero required findings.

Two initial default-concurrency full runs each failed one local MCP fixture
test; both raw failures are retained. The first failed test passed unchanged
alone, and a two-thread full run passed 2404 with four skipped. A separate Rust
probe reproduced macOS accepted sockets inheriting nonblocking mode: reading
before the client sent headers returned WouldBlock, whereas restoring blocking
mode read all 33 bytes. The three MCP fixture accept paths now restore blocking
mode. A delayed-header regression and two navigation-success assertions were
added without weakening existing assertions or timeouts. Independent review
reported zero required findings. The final default-concurrency full
release/render run passed 2405 with four skipped. This establishes the fixture
repair, not the exact cause of the two earlier failures.

Both exact CLI configurations built. Relevant no-render nextest passed 187
network and new body cases. An additional broad no-render JS/network run,
repeated unchanged, passed 717 of 718; the existing 5 ms scroll-event test
returned [0,0,400]. A separate official Playwright/CDP control reproduced that
value in both the pre-body no-render binary and this candidate. The wider
no-render suite is not claimed green; no scroll behavior was changed here.

Expanded pure-HTTP pairing covered 54 cases in each browser and 48 actual
requests per browser. In both build configurations, method, bytes, body
inheritance/errors, default/explicit MIME and CL matched Chrome 154 in the
selected assertions. Actual H2 HEADERS/DATA decoding confirmed CL=355 with
355 bytes, CL=0 for a present empty string, default text MIME, and GET without
CL/MIME. The obstacle course remained 33/33. Header order, identity and stream
bodyUsed are excluded from this body comparison.

Interleaved ABBA measurement collected 80 samples per binary on the same
four-request, 355-byte POST fixture. Baseline/candidate median was 2.107/2.102
ms, p95 2.665/2.405 ms, median RSS 53.945/53.453 MiB, and median CPU
128.583/128.538 ms. Median differences are within the documented noise floor;
no speed improvement is claimed.

Render SHA256 is
`a21010b1f8c8c22f825706d9dc7e21851a369c74c717a1c84358d789070c0126`;
no-render SHA256 is
`c0791cf2ffebc0d72baf09f95caedc0430b3d1e87e61eb4c237756515cdb8a82`.
The anonymous online retest received document HTTP 200 and three shopping
HTTP 403 responses, with no card controls or payment. Its context retained
all 626 history records with hasMore=false. This does not establish complete
Chrome request parity or explain the rejection.

The public Request.body shim, stream locking/consumption, bodyUsed and the full
Headers API are not claimed to be Chrome-complete by this repair.

## Difference 4: known request body has no Content-Length over H2

Status: qualified general known-body length repair. Online checkout remains incomplete.

The successful native Chrome 153 shopping request sends Content-Length: 355.
Obscura's owned TLS/H2 receiver accepted the identical-size ordinary fixture
body through DATA frames, but HPACK decoding proved that the actual request
had no Content-Length. A paired Chrome 154 local H1 method matrix confirmed
the known nonempty byte length, whereas Obscura depended on H1 framing to add
it. The Chrome 153 successful native NetLog supplies the valid H2 reference.
The local Chrome 154 H2 subprobe used ignore_https_errors on a disposable
context contrary to the verification boundary; it was stopped, is retained as
raw experimental evidence, and is excluded from qualification. Subsequent
Chrome fixtures use ordinary HTTP without certificate bypass. H2 DATA and END_STREAM already
delivered the entire body; this is a compatibility delta, not invalid framing.

The general transport now adds the byte length for a complete nonempty body
slice before prepared capture and execution. It does not overwrite explicit
Content-Length or Transfer-Encoding supplied by native callers/interceptors,
and does not introduce a CL/TE pair. Empty bodies, including empty OPTIONS
preflights, are unchanged. Native nonempty OPTIONS shares the known-body rule.
Each redirect hop builds its own request, so this generated field is not kept
when the body is discarded on a method rewrite.

The new prepared-request regression failed before the repair. Three focused
release nextest cases passed afterward: protocol-independent prepared fields,
explicit framing preservation, and actual H1 reception of 355-byte ASCII,
UTF-8 and binary bodies. Independent Standards and Spec reviews each reported
zero required findings. Full release/render nextest passed 2400 with four
skipped; release network coverage without renderer selection passed 183.
Both exact CLI configurations built and the obstacle course remained 33/33.
Actual H2 HEADERS decoding in both configurations confirmed CL=355 with
355 received DATA bytes; GET and empty POST remained unchanged.

Interleaved ABBA measurement on a four-request, 355-byte POST compression
fixture collected 80 samples per binary. Baseline versus candidate median was
2.095 versus 2.068 ms, p95 2.695 versus 2.353 ms, median RSS 53.633 versus
54.227 MiB, and median process CPU 131.739 versus 131.839 ms. Median changes
are within the documented noise floor; no speed improvement is claimed.

Render SHA256 is
`e5dcc6d05e1d2ef3d73faa01a28c1b7e9586f56727e4682ce24d3d564f176c90`;
no-render SHA256 is
`124379c61353b74c68659f87abcecc710bad50ce1bf8fcdeaffada8d9b2b052b`.
The anonymous online retest received document HTTP 200 and two shopping
HTTP 403 responses, with no card controls or payment. Its context history
retained all 646 records with hasMore=false. This repair does not establish
complete Chrome request parity or explain the remaining rejection.

Separate fixture findings are not included in this patch: Fetch GET/HEAD with
an explicit body must reject before sending, strings need their default MIME,
and empty body must remain distinct from omitted body. The JS bridge already
tracks presence, but the transport interface currently receives only bytes.
These need their own browser-semantics repro and review.

## Difference 3: encoded response headers disappear after decompression

Status: qualified general response-header repair. Online checkout remains incomplete.

The successful native Chrome 153 NetLog retains Content-Encoding and the
encoded Content-Length for ordinary Brotli script responses. Obscura's
prepared response capture omitted both fields despite identical decoded
ordinary script bodies. A local paired Chrome 154 and Obscura fixture then
confirmed the same difference for gzip, deflate, Brotli and Zstandard in
Fetch, XHR and CDP. The reference Chrome major differs for this local probe;
the native Chrome 153 record supplies the ordinary-site Brotli observation.

The repair captures parsed encoded response fields before primp's automatic
decompression layer. It exposes a separate encoded_headers view without
changing primp's existing decoded Response.headers view. Obscura uses the
encoded view for browser-visible and native response capture fields, while
reading the same decoded body stream. Decoded size hints and cumulative
stream accounting enforce max_response_bytes; encoded overhead does not
reject an otherwise in-limit decoded response. The snapshot retains repeated
values and is not a claim about HTTP wire ordering or framing.

The initial release regression failed with None rather than gzip. The first
green run passed that regression and the existing response-limit test.
Expanded network coverage passed 180 release nextest cases, including four
encodings, duplicate fields, decoded overflow and a one-byte gzip body whose
encoded length exceeds the decoded limit. Independent Standards and Spec
reviews reported no required findings. The first full render run passed 2394
and failed three MCP local-network cases while a CLI build was concurrent.
Those selected cases passed unchanged in isolation; a clean full rerun then
passed 2397 with four skipped. Concurrency is not a proven failure cause.
The exact render CLI build and no-render CLI build completed. Paired Fetch,
XHR and CDP probes preserved the four encoded lengths and decoded bodies in
both configurations. The obstacle course remained 33/33.

An eight-run interleaved ABBA comparison measured 80 samples per binary on
the same four-encoding fixture. Baseline versus candidate median latency was
1.849 versus 1.916 ms, p95 2.467 versus 2.345 ms, median RSS 52.805 versus
52.859 MiB and median process CPU 124.765 versus 124.748 ms. The latency
median change is within the documented ten-percent noise floor.

Render candidate SHA256 was
`c5668f97d5da43db4d355a2e1ef7dba1329f94c1f9b77ee06d7e373f5faf90e6`.
The anonymous online form retest received document HTTP 200 and four
shopping HTTP 403 responses. No credit-card controls appeared. An initial
retest action timed out waiting for scheduled navigation; its raw record was
preserved separately. The follow-up observed the response independently of
that navigation wait. No card data was entered and no payment was submitted.

Additional transport observations are not repairs or a 403 attribution:

- A scoped-CA local TLS receiver observed TLS 1.3, ALPN h2 and SETTINGS
  1=65536, 2=0, 4=6291456, 6=262144 in the same order as native Chrome 153.
  The CA was scoped only to the disposable Obscura process. No system trust
  changes or certificate-validation bypasses were used.
- One local connection carried a GET, a 355-byte POST and an empty POST.
  END_STREAM behavior was consistent with those bodies. HPACK decoding of
  the actual received HEADERS showed the correct pseudo-header order but no
  Content-Length on any of the three requests. Native Chrome's successful
  355-byte shopping POST carried Content-Length: 355. Missing this field is
  not itself an HTTP/2 framing violation, but it is an observed shape delta.
- Chrome's observed H2 weights vary by resource. Obscura's local requests
  used a fixed weight of 256. Its fixed ordinary-field ordering also does
  not match every Chrome resource sample. These remain separate candidates.
- A local fifteen-case script fixture completed in both browsers without
  page errors. Chrome's CDP priority varied with script loading mode and
  fetchpriority hints; Obscura's generated Priority did not. Chrome sent no
  generated Priority field over HTTP/1.1 in this fixture. A uniform Script
  replacement value would not implement the observed behavior.

No Chrome cookies, session tokens or protection payloads were transferred.
Payment remains outside this workflow.

The acceptance target is one adult, one-way LGA to MDW on 2026-10-27,
selecting the earliest flight at the lowest available fare and reaching the
credit-card entry page through official Playwright Python and CDP. Passenger
data is generated with Faker. Payment is outside this workflow.

Raw network events, response bodies, script diagnostics, snapshots and Chrome
NetLog remain in a private directory outside the repository. This document
records findings and validation without session credentials.

## 2026-09-29: initial comparison

- Source baseline: `1099f5a`. Official Playwright Python: 1.60.0.
- Reference Chrome: 154.0.8037.58. Obscura persona: `macos_chrome153`.
  This version mismatch limits conclusions about identity and transport.
- Both browsers used separate anonymous contexts, the same local proxy,
  1365 by 768 viewport, device scale 2 and Asia/Shanghai. The initial language
  settings were not aligned: Chrome used en-US, while the built-in Obscura
  persona used en and zh-CN. The earlier claim that both used en-US was an
  incorrect control assumption, corrected after inspecting actual requests.
  Physical screen dimensions also differ: Chrome's viewport-derived screen
  is 1365 by 768; the immutable Obscura preset reports 2560 by 1440.
- The initial direct search returned shopping HTTP 403 in both browsers.
  Searching from the subsequently loaded booking form succeeded in Chrome.
  Its lowest displayed fare was $139; the earliest matching flight was
  WN 2341, 07:25 LGA to 08:45 MDW. The price page quoted $138.40 including
  taxes and fees. After explicit approval to accept the fare rules, Chrome
  submitted Faker passenger details, continued without paid seat upgrades,
  and reached `/air/booking/purchase.html`. The visible card number, expiry
  and CVV fields were empty. No card data was entered or payment submitted.
  A later read-only check during release qualification found Chrome back at
  `/air/booking/`, without card controls. The earlier arrival is preserved in
  snapshots and logs; an active checkout is not claimed to remain available.
- Obscura performed the same form action and emitted shopping requests with
  identical business parameters, but received HTTP 403 with code 403050700
  and returned to the booking form. The cause is unproven.
- Native computer-use access to Chrome repeatedly timed out. Chrome was
  started with a private CDP profile and its anonymous context was created
  with Playwright. The requested computer-use launch is still unverified.
- Capture coverage is explicitly limited: Chrome scriptParsed records parsing;
  Obscura scriptExecution records classic-script execution. Neither is a log
  of every asynchronous JavaScript operation. Page-session capture does not
  establish complete worker or out-of-process-frame coverage.
- Baseline release build completed. Capture unit tests: 9 passed.

## Difference 1: independent Page observers miss navigation events

Status: verified for the existing ordinary navigation-event subscription contract.

A local HTTP fixture reproduced the discrepancy without Southwest content.
Two independent CDP sessions enabled Page observation on the same page.
Chrome delivered main-frame navigation and loading events to both; Obscura
delivered lifecycle events but omitted ordinary Page events when Playwright's
managed session initiated navigation or clicked a navigation button.

The repair introduces Page subscriptions per attached session and distributes
existing navigation events to enabled observers of the same page. Disabling,
detaching and closing pages clear the subscriptions. Page.disable also clears
that session's lifecycle flag, as observed in reference Chrome; enabling Page
again does not restore that flag. Direct embedder calls without a session keep
their existing event projection.

The historical initial about:blank bootstrap sequence is preserved for client
startup compatibility. Its lifecycle projection is not qualified by this
repair; the independent subscription assertions cover subsequent navigation.
Chrome also emits `Page.frameStartedLoading` in the local fixture. Obscura did
not implement that event before this repair, and this observer repair does not
add it. Qualification concerns delivery of the four existing ordinary events,
not complete Chrome navigation-event parity.

Validation completed:

- Independent Standards and Spec reviews: no required findings.
- Focused release render nextest: 409 passed, 3 skipped.
- Full workspace release render nextest: 2395 passed, 4 skipped.
- Exact CLI release build with render completed. Candidate SHA-256:
  `8cb2b7b91aff69f343fe9558b416e03465db17009d9da1674d78c15e35d2d752`.
- Paired local fixture: both observers received frameNavigated, DCL, load and
  frameStoppedLoading once, in order, for both goto and navigation-button
  click. Baseline observers received none of these ordinary events.
- Interleaved baseline/candidate ABBA comparison: 20 measured samples each,
  same binary feature set, persona, local fixture, 1365 by 768 viewport,
  scale 2, two Page observers and no screenshot. Each navigation and click
  used load readiness plus 100 ms settle, including the settle time in the
  measured cycle. No build or nextest process from this workflow ran
  concurrently with this measurement.

| Measurement | Baseline | Candidate |
| --- | ---: | ---: |
| Cycle minimum / median / p95 / maximum, ms | 295.3 / 303.4 / 306.4 / 307.6 | 287.4 / 304.1 / 307.7 / 307.7 |
| Median sampled process RSS, MiB | 60.77 | 61.43 |
| Median child CPU per six-cycle run, ms | 406.35 | 392.85 |

RSS is a sample after the run, not peak memory. Child CPU includes startup,
warmup and the sampling command. Differences are within the approximately
10 percent noise floor; this is not a performance-improvement claim.

- Companion benchmark `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`:
  obstacle course 33/33 with windows_chrome145, one run and no warmup.
- Fresh candidate CDP server and anonymous context: the same route/date
  booking form remained correctly filled. The search click hit the earlier
  ten-second deadline; that capture did not establish which client wait
  expired. Later protocol captures separate the epilogue from navigation
  waits, as recorded below. Its shopping
  responses still returned HTTP 403, code 403050700, and the page returned to
  the booking form. This repair does not resolve the shopping rejection.

The first focused run exposed legacy test setups assuming Page events without
Page.enable. Those setups now explicitly subscribe; original document,
loader, context-order and abort assertions were retained. Runtime events
remain independent of ordinary Page subscriptions.

## Difference 2: missing frame load-start observation

Status: reproduced locally; design analysis only.

The same local fixture shows `Page.frameStartedLoading` in both reference
Chrome observers for `goto` and a navigation-button click. Obscura has no
corresponding event producer. This is separate from Difference 1.

The independent analysis recommends observing the native transition to a
loading state, rather than inserting a synthetic start into the completed
navigation projection. Any implementation must cover slow-response timing,
loading-episode deduplication, aborted loads and recovery. A further Chrome
fixture observed start before a delayed response was released; 204/205 emitted
start, response, failure and stop without committing a document. Invalid URL
and history commands emitted no start. Hash, pushState and replaceState each
emitted start, navigatedWithinDocument and stop, despite not loading a new
document. Native cross-document Loading transitions alone therefore cannot
cover all of these cases. Child-frame behavior remains unqualified. No
implementation or shopping-outcome improvement is claimed for this difference.

## Comparison control correction: language preference

Status: verified locally; not an engine bug or a proven cause of rejection.

Independent analysis matched the successful Chrome shopping POST against
baseline and candidate Obscura form-click POSTs. Business JSON, Origin,
Referer, Accept, Content-Type and Sec-Fetch metadata matched. Cookie-name sets
matched; independent session values were not copied or aligned. Actual
Accept-Language differed: Chrome used en-US, while Obscura used
en,zh-CN;q=0.9,zh;q=0.8.

A local HTTP echo fixture confirmed navigation, fetch and XHR carry the
configured preference, and navigator.language/languages agree with it.
An explicit immutable macos_chrome153 persona with en-US, a single en-US
language entry and Accept-Language en-US matched the Chrome control. This
uses the existing validated configuration API, without header overrides,
site-specific behavior, token transfer or engine changes. The remaining
Chrome-version and physical-screen differences are still comparison limits.

A fresh anonymous context with that configuration sent en-US on the actual
form-click shopping requests. Other compared metadata and business JSON still
matched Chrome, but both responses remained HTTP 403 with code 403050700.
This control correction did not remove the rejection. The new sample's cookie
name sets differed by one item on each side; the Obscura capture did not show
a prior response setting the missing item, so this is not evidence of a
received cookie being incorrectly omitted.

Read-only comparison found 28 paired ordinary response bodies identical in
each Obscura sample and Chrome, including 20 initial app/vendor/en scripts.
The booking HTML and phase-specific version JSON also matched. Chrome's 304
versus Obscura's 200 for status.js had the same actual body. The initial
select-depart document was absent from the earlier CDP body capture. It was
later recovered from the final binary's native network-history cache, as
recorded below. That recovery does not establish the older samples' initial
inputs. Extra booking resources after rejection are an outcome, not a proven
cause.

## Difference 3: same-document actions projected as new documents

Status: verified for the scoped Runtime/Input action projection.

A localhost fixture with an iframe reproduced a real official Playwright
client regression. Chrome preserved two frames and existing load readiness
after hash, pushState and replaceState clicks. Obscura emitted frameNavigated
instead of navigatedWithinDocument: Playwright removed the child frame and
subsequent wait_for_url(load) and wait_for_load_state(load) timed out, despite
the DOM retaining the iframe and document.readyState being complete. A pure
fetch receiving local HTTP 403 did not reproduce those client failures.
This does not explain Southwest's shopping rejection or its click timeout.

The repair retains the native navigation source and projects
Page.navigatedWithinDocument from Runtime/Input actions to the enabled Page
observers of that page. Hash changes and valid same-document history
traversals use fragment; pushState and replaceState use historyApi, including
unchanged URLs and URLs containing a hash. These types were observed in
reference Chrome. The old loader, document identity, runtime contexts and
child frame tree remain intact, without synthetic DCL or load events.

Qualification is limited to the existing action-navigation projection and
single pending same-document transitions. Raw Page.navigate,
Page.navigateToHistoryEntry, autonomous timer navigation and multiple
transitions coalesced within one action are not qualified by this repair.
The missing load-start event from Difference 2 also remains separate.
The earlier build and test counts apply to Difference 1, not this new source.

Validation completed for this repair:

- Independent Standards and Spec source reviews: no required findings.
- Red release regression observed frameNavigated rather than the required
  within-document event. Targeted regressions after repair: 13 passed.
- Focused release render nextest for JS, browser and CDP: 1389 passed,
  3 skipped. Full workspace: 2396 passed, 4 skipped.
- Exact CLI release render build completed. Final SHA-256:
  `11e5cb81d501a97b5fb3bb7eefdc4cb76762f1be392421adff6fc487187b24a2`.
  An intermediate workspace build produced a different binary; final
  Playwright fixture and live validation were rerun with the exact CLI build.
- Official Playwright 1.60 paired fixture on the final binary: hash,
  pushState and replaceState each emitted one navigatedWithinDocument to
  the independent observer, preserved both frames, and passed both load
  waits. The ordinary fetch-403 control also passed without a navigation.
- Companion obstacle course at the same pinned revision: 33/33, using
  windows_chrome145, one run and no warmup. Its three-second settle is not
  a latency-only engine measurement.
- Final fresh en-US context, correct route/date and 30-second click deadline:
  Search completed as an input action, but the page returned to booking.
  Its two form-stage requests had matching business JSON and en-US, but
  remained HTTP 403, code 403050700. Obscura still did not reach passenger or
  credit-card entry.

An interleaved ABBA comparison against the qualified Difference 1 binary
used 20 measured cycles each. Each cycle included ordinary navigation and
click, pushState, replaceState and three 100 ms settles, with the same local
fixture, macos_chrome153, viewport, device scale, two observers and no
screenshot. No build or nextest from this workflow, obstacle course or paired
probe ran concurrently with the measurement.

| Measurement | Difference 1 | Difference 3 |
| --- | ---: | ---: |
| Cycle minimum / median / p95 / maximum, ms | 403.1 / 416.4 / 419.9 / 421.5 | 375.8 / 411.0 / 419.2 / 419.8 |
| Median sampled process RSS, MiB | 62.48 | 60.68 |
| Median child CPU per six-cycle run, ms | 429.13 | 404.69 |

RSS is sampled rather than peak; child CPU includes startup, warmup and the
sampling command. These differences remain within the approximately
10 percent noise floor, not a performance-improvement claim.

## Click deadline and epilogue queue

Status: observed; no new engine defect established.

A full protocol capture of the intermediate workspace-built binary located
the 10-second click timeout in Playwright's inputActionEpilogue, not in its
subsequent SignalBarrier wait. The final mouseReleased reply took 1.277
seconds; the following Page.enable roundtrip took 18.482 seconds. No
frameRequestedNavigation or navigatedWithinDocument occurred in that sample.
Its five form-stage shopping responses remained HTTP 403, code 403050700.

A paired ordinary localhost fixture used fetch returning 403, followed by a
location navigation whose response was delayed three seconds. Chrome and
the final Obscura binary both deferred Page.enable until that response was
released: 3020.85 and 3025.38 ms respectively. Although the server's navigation
window does defer this command, that alone is not evidence of a compatibility
bug. No command-queue bypass was implemented. Final live validation uses the
ordinary 30-second Playwright click deadline instead of the earlier custom
10-second limit. This does not change or explain the shopping rejection.
In the final build's live sample, the epilogue returned without protocol error
after 15.985 seconds and the click completed. Matching shopping requests still
failed, so a successful click must not be reported as a successful search.

## Cached initial-document comparison

Status: additional input evidence; no new engine defect or rejection cause
established.

Read-only queries of the final context's native network history recovered the
initial select-depart response and its complete cached body without another
website request. The response was HTTP 200 with 5515 bytes; the retrieved
body's SHA-256 matched the recorded body reference. The Chrome initial
document also had 5515 bytes, but a different raw hash.

Independent analysis found 50 HTML start tags in each document. The only
byte differences were two src attribute values, on one script and one image.
Removing those two attribute spans from the comparison made the remaining
bytes identical. The raw evidence was preserved unchanged. Ordinary
app/vendor/en resource references matched. No inline logic or protection
script body was inspected, and the independent attribute values were not
copied or aligned.

Both initial responses contained five Set-Cookie entries. Four valid cookies
with matching request scope appeared by name in each later shopping request;
the remaining Max-Age=0 entry was not sent by either browser. Distinct
server-provided cookie names are input differences, not evidence of a cookie
being received and then omitted. Both later value changes in the final
Obscura sample matched intervening Set-Cookie response values by SHA-256.
The update absent from the earlier responseReceived-only comparison was in a
301 response. Native history and CDP's redirectResponse headers contain that
update; it was a comparison-coverage gap, not a demonstrated cookie-jar or
CDP redirect-header defect. The complete native cache page contained 916
records, with no additional page or recorded terminal failure.

The recovered document closes one evidence-collection gap. It does not prove
complete CDP network coverage or complete asynchronous JavaScript capture,
and it does not explain the shopping HTTP 403. A current read-only check still
found both active pages at the booking form, without credit-card controls.

After the later fare-rule approval reply, Chrome was checked before any
acceptance action. It was no longer at the quoted fare page. One normal search
from its correctly filled booking form produced shopping HTTP 403 and returned
to booking. A new screenshot and snapshot confirmed no card controls. The
earlier Chrome HTTP 200 and empty-card-page arrival remain historical evidence;
current reference success is not claimed. No further fare acceptance, card
entry or payment action was performed in this recheck. The cause of the changed
Chrome outcome is unproven.

## Fresh anonymous-context control

Status: reference checkout restored; Obscura acceptance remains incomplete.

Both existing browser processes were confirmed live. A new anonymous context
in each entered the normal booking form directly with the same LGA to MDW,
2026-10-27, one-adult, one-way USD parameters. They did not first navigate to
select-depart. The actual form values, en-US preference, Asia/Shanghai,
1365 by 768 viewport and scale 2 were checked before searching. Chrome 154
versus Obscura's Chrome 153 persona and physical-screen differences remain
control limits. No proxy or IP was changed and no session data was shared.

The normal Chrome search returned shopping HTTP 200. The final Obscura
binary's three requests remained HTTP 403 with code 403050700. Independent
analysis found identical business JSON and eight matching ordinary request
headers, plus 20 matching ordinary app/vendor/en response bodies. No captured
application exception or console error preceded the shopping request events;
18 ordinary Obscura scriptExecution records in that interval reported ok.
These capture observations do not establish complete execution coverage.

The fresh native cache was recovered once, with 666 records, hasMore false
and no recorded terminal failure. The previously unmatched Cookie value
changes all matched earlier native response updates by hash before the
corresponding shopping requests started. Neither missing cookie names nor
incorrect reuse of an older value was established. The new comparison still
does not identify the rejection cause or a locally reproducible general
engine defect, so no additional source repair was made.

Chrome main-page precise coverage was enabled after initial booking load and
before the search action, then collected and stopped. Complete raw coverage
was retained privately. Two allowlisted ordinary scripts contributed 290
function records, 260 with at least one positive execution range. This is
execution evidence for that instrumented main-page interval, not a trace of
initial loading, all asynchronous work or workers. Instrumentation can affect
execution, and these samples are not used for latency or performance claims.

The new Chrome result contained 26 flight options. The lowest displayed fare
was $139, with WN 2341 at 07:25 the earliest among those options. Its Basic
quote was still $138.40. The price page's entire visible text exactly matched
the earlier approved quote. Under the explicit fare-rule approval, Chrome
continued with the same Faker passenger and no paid seat selection. Seat
upgrades remained $0. It again reached purchase.html; the card number,
expiration and CVV inputs were visibly empty in the saved screenshot. No
card information was entered and Purchase was not clicked.

A diagnostic evaluation during the transition to purchase briefly saw a null
document.body. The same live page was re-observed after navigation completed;
it was not restarted. That diagnostic error is not an application exception
or evidence for the earlier shopping rejection.

Native computer-use access to the existing Chrome app still timed out.
Restored Playwright/CDP checkout does not satisfy the unverified computer-use
launch requirement. The proposed separate official Chrome 153 control awaits
user approval and has not been downloaded or run.

## Cached shopping-request sequence analysis

Status: no additional general engine defect established; checkout remains
incomplete in Obscura.

Independent analysis of the cached native timeline found three distinct
shopping POST requests in sequence. The second started 114.656 ms after the
first HTTP 403 response, and the third started 73.216 ms after the second
HTTP 403 response. This is consistent with response-driven retry, but does
not establish the exact shopping retry function, retry limit or status
condition. No evidence of duplicate input dispatch was found.

Inspection of ordinary application code located AirBookingSelectService's
shopping endpoint. Generic retry references and unrelated application
features did not establish the shopping execution path. A further source
search reached authentication and key-management symbols; inspection stopped
at that boundary. Authentication, opaque header construction and protection
logic were not analyzed. No new website requests or source repairs resulted
from this analysis, and it does not explain the first HTTP 403.

A final read-only check found Chrome at purchase.html, with the card number,
expiration and CVV inputs present and empty. Obscura remained at the booking
form with none of those inputs. No card entry or payment action was taken.

Native computer-use access remains unavailable after attempts by app name,
bundle ID and application path. The isolated official Chrome 153 comparison
still requires user approval; no new browser software has been downloaded
or run. The existing Playwright/CDP reference result does not qualify the
computer-use launch requirement, and Chrome's empty payment form does not
complete the Obscura checkout goal.

## User-requested retry and official Chrome 153 setup

Status: existing-browser retry still rejected in Obscura; a new native
reference setup is now available, with the debugging connection awaiting
specific authorization.

The user requested another attempt. The existing Chrome and final Obscura
processes were still live, and the CLI SHA-256 remained
11e5cb81d501a97b5fb3bb7eefdc4cb76762f1be392421adff6fc487187b24a2.
New anonymous contexts entered the same normal booking form with matching
LGA to MDW, 2026-10-27, one-adult, one-way USD values and the same checked
language, timezone, viewport and scale. Chrome returned shopping HTTP 200
and displayed the flight list, including WN 2341 at 07:25 to 08:45. Obscura
produced two captured shopping responses, both HTTP 403 with 403050700,
and returned to booking without card controls. Both Obscura responses and
the Chrome response received loadingFinished.

Independent analysis found matching business JSON, eight ordinary request
headers and 20 ordinary script body hashes, also matching the prior control.
The 18 ordinary Obscura scriptExecution records preceding the shopping
request events reported success, with no captured exception. Chrome had two
console errors, but no captured pageerror or Runtime exception, and shopping
succeeded. These observations do not establish complete execution coverage.
The new main-page precise coverage contains two allowlisted ordinary scripts,
416 function records, 371 with at least one positive execution range. Raw
coverage was retained separately; these are not invocation totals or a
performance comparison. No new general engine defect or source repair was
established. Request-count differences alone were not treated as defects.

Existing Chrome native access still timed out. The user then explicitly
authorized an official Chrome for Testing 153 isolated control. Google's
milestone manifest selected mac-arm64 version 153.0.8010.52. The downloaded
ZIP's local SHA-256 was
6f67faa4b34dd551b53abb6fee24edeae470ab695b0b100ddc4885ff0be6724a;
archive CRC validation passed. The application reports an ad-hoc,
linker-signed signature without sealed resources; strict bundle signature
verification did not pass. This is not claimed as a successfully verified
Developer ID signature. No signature, quarantine attribute or system
security setting was changed.

Native computer use successfully launched that separate application from
the disposable directory. Its fresh welcome screen was kept signed out,
optional feature setup was skipped, and a new Incognito window visibly
reported version 153.0.8010.52. No system security warning was encountered.
This establishes native startup and anonymous-window creation for the new
reference, not a successful website checkout or the earlier Chrome instance.

Native controls started a private NetLog with raw bytes before website
navigation. macOS HTTP, HTTPS and SOCKS proxy configuration still pointed
to 127.0.0.1:7890, with automatic proxy configuration disabled; no network
setting was changed. Actual per-request proxy routing still needs the new
capture. Only the remote-debugging settings page was viewed. Permission to
enable this separate browser's local debugging and allow the Playwright/CDP
connection was requested; the checkbox has not yet been enabled. No card
information or payment was submitted.

The preparation-only NetLog was stopped through native controls while
awaiting that authorization. Chrome confirmed the file was written, and
the complete JSON parsed successfully. The raw file was neither truncated
nor uploaded. Recording will be restarted before any website navigation
after the debugging connection is authorized.

## Native Chrome 153 business control

Status: native reference launch and checkout qualified for this control;
Obscura checkout remains incomplete. No new engine repair was established.

The user authorized continuation of the separate browser's local debugging
connection. Native controls enabled the checkbox and showed the server at
127.0.0.1:9222. HTTP endpoint discovery returned 404. The browser WebSocket
endpoint was then read from this separate application's DevToolsActivePort
file, and Chrome's Allow dialog was confirmed through native controls for
the authorized connection. No consent prompt was bypassed.

Official Playwright 1.60 connected to that same natively launched process
and created a separate anonymous test context. The earlier native Incognito
window remained the settings and NetLog window. Both test contexts entered
the normal booking form. Their checked navigator UA strings were identical,
with Chrome/153.0.0.0, as were en-US, Asia/Shanghai, 1365 by 768 and scale 2.
Chrome reported product 153.0.8010.52; Obscura reported 153.0.8010.50.
Patch versions, reported JavaScript versions and physical-screen differences
remain control limits. The experiment does not claim identical V8 engines
or transport implementations.

Chrome's shopping response was HTTP 200. Obscura's two captured responses
were HTTP 403 with 403050700, both with loadingFinished. Independent analysis
found matching business JSON, eight ordinary request headers and 20 ordinary
script body hashes. The 18 ordinary Obscura execution records preceding the
shopping request events reported success, with no captured pageerror or
Runtime exception. One pre-request console error appeared in two capture
channels without an ordinary application stack attribution. It was not
treated as proof of a browser API defect or the rejection cause.

The native cache query returned all 638 records available at that query,
hasMore false and no recorded terminal failure. Earlier unmatched Cookie
value updates matched preceding response updates by hash before the shopping
requests. No missing cookie or stale update was established. Chrome precise
coverage started before initial booking navigation; two allowlisted ordinary
scripts supplied 278 function records, 248 with at least one positive range.
The raw coverage was preserved. These observations do not prove complete
JavaScript coverage and are not performance measurements.

The native NetLog was started before website navigation with Everything
capture, including raw bytes. Native Stop Logging confirmed the file was
written. Its complete JSON parsed with 113676 events and 80582207 bytes.
Independent metadata-only routing analysis associated two ordinary documents,
20 ordinary application scripts and the shopping request with the selected
HTTP proxy 127.0.0.1:7890. SOCKS5 was a listed fallback, not the selected route
for these requests. This does not prove external exit IP identity. No raw
bytes, protection payload or authentication logic was analyzed, and the
original NetLog was not truncated, redacted or uploaded.

Chrome's lowest displayed fare was $139. WN 2341, 07:25 to 08:45, remained
the earliest among those fares; its Basic quote was $138.40, composed of
$114.42 base fare and $23.98 taxes and fees. The visible price-page differences
from the approved quote were confined to unselected rental-car advertising.
The flight, selected fare, total and fare-rule text were unchanged. Under the
existing explicit approval, the same Faker passenger continued through seats
with No seat selected, $0 seat upgrades and the unchanged $138.40 trip total.

Chrome reached purchase.html. Both Playwright evaluation and native
accessibility inspection confirmed the credit-card controls. The saved,
visually inspected screenshot shows empty number, expiration and CVV fields.
No card or billing data was filled and Purchase was not clicked. An earlier
price snapshot observed the flight-list page before its asynchronous route
transition completed; it was retained, and the later ready price snapshot
was used for rule comparison. That early observation is not an engine defect.

After evidence collection, native controls disabled local debugging. The
checkbox reported 0, the automated-control banner disappeared and no listener
remained on port 9222. Disconnect disposed of the temporary Playwright context;
the card-page evidence is from before cleanup, not a claim of a retained live
Chrome 153 payment page. The capture controller exited normally. Existing
Chrome and Obscura browser processes were not replaced or stopped.

This control closes the earlier native-startup gap and removes the previous
Chrome 154 versus Chrome 153 UA-major mismatch. It does not resolve Obscura's
first shopping rejection or complete the Obscura credit-card-page goal. No
new engine source change, site-specific workaround or rejection-cause claim
was made.

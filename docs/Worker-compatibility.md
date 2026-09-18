# Worker compatibility investigation

2026-09-17. Obscura source baseline: `a752298`. This is an implementation comparison and an open repair plan, not a conformance claim. Chromium links below reference its moving `main`; they do not identify the exact source revision of the installed Chrome 152 binary.

## Execution and communication

Chromium's [workers README](https://chromium.googlesource.com/chromium/src/+/main/third_party/blink/renderer/core/workers/README.md) describes parent-side MessagingProxy and worker-side ObjectProxy interfaces. Its dedicated workers share a renderer process with their creator, but execute on a worker thread. Sharing a process does not imply sharing the JavaScript event loop.

[WorkerBackingThread::InitializeOnBackingThread](https://chromium.googlesource.com/chromium/src/+/main/third_party/blink/renderer/core/workers/worker_backing_thread.cc) initializes a V8 isolate on that backing thread and connects it to the thread scheduler. In Obscura, `worker.rs::create_worker_context` creates a Context in the caller's isolate; `share_deno_context_state` aliases the parent's embedder state, and `share_ops_with_context` copies bound ops. Global-object isolation is present, independent execution is not.

[DedicatedWorkerMessagingProxy](https://chromium.googlesource.com/chromium/src/+/main/third_party/blink/renderer/core/workers/dedicated_worker_messaging_proxy.cc) queues early messages until script evaluation is reported, then posts them to the worker's posted-message task runner. This explains why startup acknowledgement and subsequent task delivery must be distinguished from calling the constructor. Obscura starts Blob workers through a parent timer and invokes worker handlers in the same isolate. Replacing that timer with synchronous execution or a microtask would not establish independent execution.

The [HTML worker processing model](https://html.spec.whatwg.org/multipage/workers.html#worker-processing-model) and [worker event loop](https://html.spec.whatwg.org/multipage/workers.html#the-event-loop) define worker execution separately from the creator's event loop. The repair target is observable behavior, not copying Chromium's class hierarchy or reproducing exact wall-clock timings.

## Current evidence

Computer-use Chrome and the release Obscura CLI ran the same local fixtures. Both delayed initial Blob worker execution until a 500ms parent task ended (508ms and 503ms). Both also delayed a message sent from a ready handler until that handler's busy loop ended. Neither test proves a scheduling defect by itself.

A worker that scheduled its own 100ms timer before posting ready did distinguish the implementations: Chrome ran its timer at 103ms while the parent remained busy; Obscura ran it at 500ms. Parent message delivery occurred after the busy task in both engines.

An isolated development worktree now contains `worker_timer_progresses_while_parent_is_busy`. Release nextest fails against the baseline with `ranDuringParentTask=false`, while `deliveredAfterParentTask=true`. This is the intended red test, not a passing regression gate. At this initial red-test checkpoint, no engine change had been integrated into the main worktree.

## Other differences that the repair must preserve or address

| Area | Chromium / standard behavior | Obscura baseline |
| --- | --- | --- |
| Lifecycle | Shutdown tasks and a forced-execution termination backstop | Registry removal does not cancel already scheduled realm timers |
| Message values | Structured message serialization and transferable values | JSON with string sentinels loses typed arrays and other cloneable types |
| Request environment | Distinct worker settings; creator settings captured for top-level loading | Shared parent native state; known relative fetch resolves against page URL |
| Script type | Classic and module loading paths | Worker constructor ignores options and runs source as a classic script |
| Errors | Cross-thread error reporting to the owner | Startup error returned synchronously to the parent-side startup task |

Lifecycle source: [WorkerThread::Terminate](https://chromium.googlesource.com/chromium/src/+/main/third_party/blink/renderer/core/workers/worker_thread.cc). It requests shutdown and arranges forced termination for execution that cannot yield. Obscura must retain its existing watchdog protections when separating isolates.

Fetch settings are described in Chromium's workers README: worker subresources use inside settings, while top-level loading uses a snapshot of outside settings. The README includes historical rollout notes, so its top-level-fetch status should not be treated as proof of the current code path. The current messaging proxy has separate classic and module fetch/run branches.

Obscura references: `crates/obscura-js/src/worker.rs`, Worker section of `crates/obscura-js/js/bootstrap.js`, and `crates/obscura-js/src/runtime.rs`. `docs/Southwest-fix-record.md` documents existing typed-array, termination and relative-fetch repros. The statements about pages sharing an isolate in the Playwright/Puppeteer guides describe pages, not a guarantee of independent Web Worker execution; current runtime construction must be checked separately.

## Repair boundary and validation

1. Give each dedicated worker an owned isolate and event loop on its execution thread. Never move V8 Context/Global handles or the parent's Rc/RefCell state across threads.
2. Carry initialization data and messages across an explicit queue. Retain initialization-before-message ordering, FIFO delivery, and asynchronous owner callbacks. Wake on work instead of polling on a fixed interval.
3. Keep URL, identity and policy settings local to the worker; share only explicitly thread-safe browser services. Preserve proxy selection, cookie handling, request interception, observability and URL validation.
4. Stop queued tasks on close/terminate, interrupt runaway execution, and tear down child workers when their owner is destroyed. Do not let worker teardown block page shutdown indefinitely.
5. Cover autonomous worker progress, a responsive parent during worker CPU work, ordered messages, termination before/after initialization, worker exceptions, binary clone values, and worker-relative requests. Use bounded failure cases so an incorrect implementation cannot hang the test runner.
6. Run focused release nextest, full render nextest, the required release build, obstacle course 33/33, and stealth runtime tests before claiming completion. Measure worker startup and steady-state overhead separately.

Southwest remains an additional live acceptance check. Its local shopping requests still returned 403, including earlier experiments that completed worker work before shopping. Independent worker scheduling is a proven compatibility gap, not a proven explanation of that response.

## Implementation checkpoint

A separate development worktree now replaces same-isolate Worker contexts with an owned runtime on a dedicated thread. Startup and incoming messages are queued; outgoing events are awaited by the parent and delivered asynchronously. The worker has a termination handle, and close suppresses later timer callbacks while allowing the current task to finish. Existing snapshot/bootstrap facilities are reused without passing V8 handles across threads.

The red concurrency test now passes. The latest focused release nextest run passed all 20 selected tests, including existing Worker behavior, the host-neutral protection probe, worker progress during parent CPU work, parent termination during worker CPU work, and close/current-task behavior. An earlier parallel attempt exposed startup timing and exception-message failures; after correcting initialization and exception extraction, the complete focused group passed at default test concurrency. These results are not a full gate or Southwest success claim.

Before integration, audit worker network interception and event/body forwarding, replace remaining JSON message cloning, and verify bounded lifecycle cleanup. Full render nextest has been started; its result, release builds, obstacle course, stealth tests, and live comparison remain pending. The formal runtime has not been replaced.

### Message codec checkpoint

The development implementation now uses V8 value serialization instead of JSON for Worker payloads. Focused tests first failed against the JSON implementation, then passed after the codec change. The latest complete focused run passed 22/22, including typed arrays, BigInt, Date, Map, Set, circular/shared references, snapshot isolation, ArrayBuffer transfer in both directions, duplicate/invalid transfer rejection, and preserving buffers after clone failure. ArrayBuffer transfer currently copies bytes and then detaches the source; it is not a zero-copy implementation. SharedArrayBuffer, WebAssembly.Module, MessagePort and browser host-object cloning remain outside this implemented subset and need further compatibility work.

The first broad diagnostic run reported 1737 passed, four failed and four skipped (1741 tests executed). Two failures were the intentionally red codec tests, which are now green in focused coverage. The other failures were `render_resource_loads_share_one_page_wide_concurrency_limit` (41 resources versus 42) and `a_miss_created_inside_an_awaited_expression_loads_during_the_wait` (font metrics unchanged). Targeted follow-up is pending; they are not classified as baseline failures without evidence.

That broad run overlapped development and builds in the shared target directory, so it is not a frozen-artifact acceptance gate. Rerun the complete required gates sequentially after the final source is fixed. Remaining integration work includes network interception/observation forwarding, bounded cross-thread queues, owner teardown verification, stealth builds, the obstacle course, and the live Southwest comparison. No formal runtime replacement or website success is claimed.

### Render diagnostic follow-up

The targeted follow-up passed the font case but reproduced the concurrency fixture failure, with `read_fixture_headers` panicking on `WouldBlock`. The helper reads accepted sockets from a nonblocking listener without explicitly restoring blocking mode. A standalone Rust loopback repro on this host returned `WouldBlock` before `set_nonblocking(false)` and read the byte successfully afterwards, retaining a two-second read timeout.

The development worktree now sets blocking mode inside that test-only helper. Both originally failing render tests pass in the targeted release nextest run. No production rendering behavior was changed. This resolves the observed fixture failure; a final sequential full gate is still required. The installed formal runtime hash remains unchanged.

### Worker network checkpoint

The local HTTP regression first reproduced a missing Worker result while the parent fetch succeeded. The thread configuration now inherits the parent's HTTP/stealth clients, cookie jar, interception channel and enabled state, callbacks, and page in-flight counter. Parent and Worker requests share atomic interception and response counters, preventing duplicate IDs. Network events, retained response bodies, fetched URLs and captured console/runtime observations return through the Worker event channel before its reply is delivered. Remote console object handles are removed because they belong to the Worker isolate; inline values/previews remain available, but remote Worker object inspection is not implemented by this forwarding path.

A second failure exposed URL queries behind the DOM-presence check in native ops. Workers have an environment URL without a DOM. URL/base queries now run before that check, so relative fetch resolves against the worker script URL.

The latest focused release run passed 23/23. The network test serves only loopback fixtures and verifies a parent request plus Worker script/data requests, three distinct interception IDs, three callbacks, three distinct response IDs, retrieval of the Worker's response body from the owner, and Cookie sharing. The Worker request resolves to `/workers/data.txt` when the owner is `/pages/index.html`. This verifies the render-build network path; it does not yet validate a stealth live run or changes to interception policy after Worker creation.

Outstanding work: shared queue/resource bounds and teardown accounting, policy updates after startup, final sequential gates, release/stealth builds, obstacle course and Southwest acceptance. The formal runtime remains unchanged.

### Resource, lifecycle and policy checkpoint

Worker queues now share a page-tree budget: 64 MiB of queued payload, 16 MiB per message, 4096 queued messages and 32 running workers, including nested workers. Reservations are released on receive, send failure or queue destruction. These are payload bounds, not an exact process-memory bound. A single terminal notification can bypass a full queue so failure remains observable. The thread-count lease lasts until the worker runtime is destroyed.

Owner destruction now has a focused regression with two nested workers, including a child executing an infinite loop. Both threads exit within the test's one-second bound, without waiting for their five-second execution watchdog. Queue accounting and shutdown coverage passed in the render configuration.

Another red regression showed that enabling interception and URL blocking after worker startup was not propagated. The owner and descendants now share a synchronized policy snapshot. Worker fetch and console operations refresh that policy before use. The regression verifies a newly blocked URL, interception fulfillment of an allowed request and newly enabled console capture.

Identity snapshot getters are evaluated before borrowing parent native state, with a reentrant op and exception capture. A regression verifies that a getter can log through native ops, and that a throwing getter reports the error without leaking a worker thread.

The complete focused selection passed 28/28 with `render,stealth`. This exercises the compiled stealth configuration, but is not proof of live proxy or Southwest success. The frozen full render nextest run, with no concurrent builds, passed 1747/1747 with four skipped tests. Release builds, obstacle course, performance measurements and live acceptance remain pending. After the frozen gates and live checks below, the same implementation was integrated into the main worktree. The installed formal runtime remains unchanged.

### Frozen gate results

The exact render release CLI build succeeded. The companion obstacle course returned 32/33: `observer-intersection` expected `io:50`, received an empty string. The retained pre-change CLI reproduced the same failure in a targeted run. The fixture assumes observed targets repeatedly intersect; its callback appends one batch without scrolling or replacing the observation. This is an existing gate limitation, not evidence of a Worker regression. The required 33/33 has not been achieved, and the fixture or rendering behavior has not been changed as part of this Worker repair.

### Interleaved runtime measurement

Six paired rounds alternated the retained runtime and the new runtime on the same loopback HTTP fixtures, macOS Chrome 152 persona, 1280x900 viewport, SDK, navigation/readiness boundary and 500ms idle window. Each sample used a fresh process and workspace. No build or broad test ran concurrently.

| Measurement | Retained runtime | New runtime |
| --- | --- | --- |
| Worker ready latency, median (range) | 45ms (44–46) | 28ms (28–31) |
| Launch through ready observation, median (range) | 383.6ms (377.9–385.8) | 382.8ms (378.5–1970.1) |
| Process RSS after one idle Worker, median | 45.0MB | 48.4MB |
| Idle CPU time over 500ms, median (range) | 1.99ms (1.72–2.75) | 1.87ms (1.38–2.39) |
| Process thread count, median (range) | 20 (20–20) | 21 (21–22) |

The first new-runtime launch was the 1970ms outlier; its cause was not measured. Retain it rather than discarding it as warmup. The idle CPU values are within noise and do not establish an improvement. The observed RSS increase is about 3.4MB for this one-worker fixture, not a general per-worker memory model. In all six new-runtime samples the Worker's 100ms timer ran during the parent's 500ms busy task; all six retained-runtime samples delayed it until afterwards. Owner message delivery remained after the busy task in both.

The initial measurement harness queried every 10ms and left the fixture pending. Changing queries to 50ms allowed progress. The runtime's outer select creates a fresh 20ms sleep after every action, so rapid read actions can starve its autonomous tick. This is a separate scheduler issue in `runtime/src/main.rs`; it is not fixed by independent Worker isolates and needs a focused regression. The failed harness attempts are excluded from the timing table, not represented as successful measurements.

### Live acceptance and interpretation

Four consecutive fresh-workspace checks ran locally through `http://127.0.0.1:7890`, using the macOS Chrome 152 persona and direct navigation to the BWI–MCO search URL for 2026-09-30:

| Order | Runtime | Capture mode | First shopping | Business result |
| --- | --- | --- | --- | --- |
| 1 | New independent Worker runtime | All logical request/response headers and bodies | 200 | success=true, 26 itineraries |
| 2 | Retained pre-change runtime | Same full logical capture | 200 | success=true, 26 itineraries |
| 3 | New independent Worker runtime | Lightweight SDK event capture | 200 | success=true, 26 itineraries |
| 4 | Retained pre-change runtime | Same lightweight capture | 200 | success=true, 26 itineraries |

Each page displayed `Depart: BWIMCO`. The new-runtime full capture contains 124 events (68 requests, 56 responses), with no body-read or callback errors. Its shopping payload hash matches the earlier successful Chrome request. Logical callbacks are not transport-level headers: the native transport adds cookies and browser headers later. A separate passive diagnostic build is needed for that comparison.

The full-capture harness initially issued body reads without a concurrency bound and the runtime exited normally before shopping (`BROWSER_EOF`, process exit 0, empty stderr). The protocol reader has a 16-entry evidence mailbox and closes on overflow. Limiting body reads to four concurrent operations allowed complete capture. The initial run is invalid acceptance evidence; its exact closure reason was not instrumented.

These results establish that the new runtime can complete this search under the current local proxy conditions. They do not establish that the Worker change fixed 403: both old-runtime controls also succeeded, including without full body capture. The public proxy exit address and server-side state were not independently held constant or measured. Previous 403 observations remain historical evidence, not a same-time failing control.

The tested implementation is now in the main worktree. The candidate runtime SHA-256 is `3ecefaa2bd6aa90cbd9836d6c98c7f3a5cb516947e1235fb684363b815f91cdc`; the installed runtime remains the retained SHA-256 `0cbec708f17ad85985929dfc3f1ba88c9e4581189177c06e8d57b6e6cd6f6326`. No commit, push or installed runtime replacement is claimed.

### Successful native capture

A passive recording build also returned first-shopping 200 and 26 itineraries. Its 493 native events include 57 transport submissions with 57 responses, 14 additional logical requests blocked by the configured origin policy, two Worker ready events and 13 Worker task begin/end pairs. No Worker task error was recorded; all referenced body files exist. Instrumentation was removed after copying the diagnostic binary, and source hashes match the frozen tested patch again.

The successful request still preceded Worker execution: constructor 2.3041s; shopping submission 2.7425s; Worker source 2.9025–2.9030s; four protection children 3.0238–3.0294s; shopping 200 response headers 4.3890s; `/s3` submission 6.1096s. Under this accepted server/network state, completing these tasks before shopping was not necessary. This does not identify the historical 403 cause or eliminate conditional effects under a different server state.

Against the earlier computer-use Chrome incognito HAR, the primary protection script and all four children have identical response bytes. The shopping body also matches. Cookie-name counts remain different: Chrome 10, Obscura 11; Chrome-only `swa_FPID`, Obscura-only `swa_spa_grp` and `AMCVS_65D316D751E563EC0A490D4C%40AdobeOrg`. The HAR's Accept-Language is `en-US,en;q=0.9`, while this runtime sends `en,zh-CN;q=0.9,zh;q=0.8`. This comparison must use the captured header, not infer it from the persona or navigator language. Both captures succeeded.

The transport logger sits before primp builds the encoded request. Missing HAR pseudoheaders or auto-generated Content-Length in that logger are differences in capture layers, not evidence that the transmitted request lacked them. HTTP/2 frame/TLS equivalence is not established. The Chrome recording is earlier, not a same-time exit-IP/server-state control.

### Frequent-query scheduler repair

The separate query-starvation issue now has an SDK regression using a real runtime subprocess and loopback page. During 0.6 seconds of continuous text queries, the page's 50ms interval must advance at least three times and a Blob Worker must deliver its ready message. Against the independent-Worker runtime before this follow-up, the interval remained at zero. After the scheduler change, the regression passes.

`runtime/src/main.rs` now retains the autonomous-tick deadline across actions and services an expired tick before the next ordinary action. It advances the deadline after each completed tick. Control and evidence channels retain their existing priority. This fixes the observed ordinary-query starvation; it is not a claim that every queue/load pattern is starvation-free. The full SDK suite passed 33/33 and runtime release nextest passed 184/184. The subsequent live repeat and same-time previous-runtime control both returned 403, as recorded below.

### Latest repeat: 403 remains reproducible

At the user's request, a fresh passive capture of the runtime including the query-scheduler repair returned two shopping responses with HTTP 403 and body code `403050700`. A closely following control using the previous independent-Worker runtime, without the query-scheduler repair, also returned two 403 responses with the same code. Earlier successful checks are not evidence of stable acceptance, and this pair does not implicate the scheduler change as the cause.

Relative to first shopping submission, the four protection children completed 958–926ms before the failed request. The first Worker source began 157ms afterwards; `/s3` started 1583ms afterwards. The previous successful native capture started its first Worker 160ms after shopping, completed all four children 284–287ms afterwards and sent `/s3` 3367ms afterwards. The earlier Chrome success evaluated its first Worker 405ms before shopping; one child completed 360ms before and the remaining three 106–111ms afterwards. These are request-relative intervals from each capture's own clock.

The five protection response bodies and 355-byte shopping payload still match Chrome. Cookie-name counts remain Chrome 10, Obscura 11, with the same name-set differences in both successful and failed Obscura samples. The two site's Worker startup wrappers are each 9699 bytes; after normalizing the single random Blob sourceURL UUID, they match Chrome and both Obscura outcomes. This does not establish equality of imported Worker bodies or execution results. The repeat records 26 completed Worker tasks without task-level errors and no recorded protection-child execution exception.

The failed repeat contains 1140 native events, including 144 transport submissions, 143 response-header events and 142 logical responses. At capture close, one `bf.html` beacon remained without a response; one 301 redirect had headers but no final logical response body. All recorded body files exist, and both shopping exchanges are complete. Coverage is stated explicitly rather than claiming every background request finished.

Next discriminating checks are a contemporaneous computer-use Chrome control with measured proxy/connection state, then Worker importScripts bodies, returned values and environment calls. Do not infer a stable exit IP from the shared local proxy entry. No site-specific shopping barrier has been added.

### Offscreen WebGL continuation on a second host

The 2026-09-17 continuation starts at `911a48e`. A fresh computer-use Chrome incognito capture returned shopping 200 with `success=true` and 26 BWI–MCO itineraries for September 30. The same-source rebuilt runtime returned 403. Both used the configured local proxy entry; the actual Southwest exit address was not measured. Chrome's HAR contains the loopback proxy address, which is not a public exit address.

Decoded native Worker messages exposed a separate compatibility defect. The graphics worker returned `false` for its initial context check and subsequent parameter requests. `OffscreenCanvas.getContext()` created an unrelated document canvas, losing its dimensions, options and context identity. In a Worker, where `document` is absent, it returned null. A computer-use Chrome fixture and a release nextest regression verified the difference before the repair.

Offscreen WebGL now uses the existing software WebGL context with the OffscreenCanvas as its owner, retaining dimensions, creation options, repeat-get identity and exclusion of other context types after WebGL creation. Worker initialization also copies the configured WebGL vendor and renderer. The regression verifies both window and Worker contexts and actual clear/readPixels output. This is a bounded WebGL repair, not complete OffscreenCanvas support; 2D worker contexts, bitmap transfer, export and resizing semantics still require further work.

The site's 2479-byte graphics Worker import was captured at `op_worker_load_script` and matches both historical and freshly exported Chrome source bytes. After the repair, the site returns all eight graphics result segments instead of repeated `false` values. Shopping nevertheless remains 403. The other, much larger Worker import differs across captures; startup-wrapper equality must not be substituted for imported-source or execution equivalence.

Validation completed for this source state: focused render 29/29, focused render+stealth 29/29, complete core render 1748 passed with four skipped, runtime workspace 184/184, and the specified render release CLI build. The retained same-source baseline obstacle course is still 32/33 at companion revision `e4a5490899628053752aca8201f0e46a56360b2c`; its existing `observer-intersection` failure is not a passing gate.

Further live controls remained unsuccessful: language-only alignment to current Chrome, current-device persona fields, allowing the JWKS dependency, allowing the 16 public origins observed in Chrome's HAR, and a local test process routed through the mini32 proxy. These are observed failed controls, not proof that each factor is irrelevant under all server states. Some lightweight captures lost response-body handles during navigation; their status observations must not be promoted to complete-body evidence. Native captures retain the shopping exchanges and Worker imports. No site-specific request barrier, imported browser cookie/token, or production runtime installation was added.

Six paired, alternating fresh-process measurements used the retained `911a48e` runtime and this candidate on the same local Worker fixtures, with no concurrent build or test workload. Startup latency medians were 20ms and 19ms (ranges 9–20ms and 19–21ms); launch-to-ready medians were 420.0ms and 419.9ms. Idle RSS medians were 44.19MB and 44.52MB, and idle CPU over 500ms was 2.07ms and 2.19ms. Both versions delivered the independent-timer result correctly in all six samples. These differences do not establish a performance improvement. The candidate obstacle course also returned 32/33, with the same `observer-intersection` failure as its baseline.

Rendering regression checks compared retained baseline and candidate CLI binaries through the existing Obscura capture helper, without launching an automated Chrome instance. All 66 deterministic fixture pairs navigated successfully, were nonblank and matched pixel-for-pixel. Of 30 real-site top/bottom pairs, the two Remix captures were blank in both versions and are excluded from fidelity evidence. Twenty-four nonblank pairs matched exactly. Visual inspection localized the remaining differences to Bulma's floating elements and banner contrast, Apple's phone image, and Porkbun's mascot artwork. Four interleaved Apple repeats matched the original candidate in both binaries, so the initial Apple difference is not specific to this patch. Bulma and Porkbun resource/animation equality was not established; these captures are not a claim of complete Chrome fidelity.

An offline replay used the same captured large Worker source and inputs in computer-use Chrome and the local SDK runtime, with fixed Date, performance clock and random providers. Both returned 157 bytes: the first 155 matched, while the last two differed. The retained baseline and candidate returned the same complete output. This is a pre-existing unresolved difference, not evidence that the Offscreen repair changes that computation or causes the shopping rejection. Clock overrides and a diagnostic trusted-message override did not remove it. The replay has no external network access and its output was not submitted to Southwest.

### Transport echo control

A subsequent computer-use Chrome navigation and local candidate SDK navigation to `https://tls.peet.ws/api/all` both returned 200 over HTTP/2 through the local proxy. The echo service reported the same public address for this endpoint. This does not establish the Southwest route or exit address. Their JA4 and HTTP/2 SETTINGS/window-update/pseudoheader fingerprints matched. The signature_algorithms extension differed: Chrome included a leading GREASE value, while the candidate omitted it. Header values matched, but DNT appeared before upgrade-insecure-requests in Chrome and at the end in the candidate. These are observed wire-facing echo differences, not established causes of Southwest 403.

[BoringSSL commit 29e593e](https://boringssl.googlesource.com/boringssl/+/29e593e29165df578ab778269a1f04da2055c32f) added separately configurable signature-algorithm GREASE in June 2026. The current primp-rustls dependency copies the emulator's configured signature list without such an insertion. An isolated diagnostic experiment is being used before selecting a production implementation; no certificate-verification bypass is involved.

The diagnostic signature-GREASE build completed and its echo response matched Chrome's Peetprint as well as JA4 and the HTTP/2 fingerprint. Its following Southwest run at 2026-09-17 04:51 UTC still returned two shopping 403 responses; both native bodies contain code `403050700`. Thus adding signature GREASE alone did not satisfy acceptance in this control. The experiment remains outside the main worktree, with certificate verification unchanged; it is not a tested production dependency update.

A follow-up isolated build also aligned DNT placement. The echo service then reported identical navigation header values and order, while Southwest still returned three shopping 403 responses with body code `403050700`. This is a navigation-header control, not evidence of identical POST wire ordering.

Inspecting actual extension payloads exposed an additional limitation hidden by the fingerprint summaries. Chrome advertises ALPS `h2`; primp-rustls 0.23.43 emits bytes `c9 bb 32`, decoded by the echo service as `ɻ2`. Its `client/hs.rs` explicitly uses the substitute to prevent ALPS negotiation because the implementation cannot process the response. The prior matching Peetprint/JA4 therefore must not be read as TLS equivalence. This dependency behavior remains unchanged in production and diagnostic builds. A correct repair requires ALPS negotiation and application-settings handling, not merely replacing the advertised bytes. The [ALPS draft](https://www.ietf.org/archive/id/draft-vvv-tls-alps-01.html) requires a client EncryptedExtensions response when the server negotiates ALPS; Chromium's current implementation must also be checked before selecting a compatibility design. Southwest's actual ALPS negotiation has not yet been captured, so this limitation is not an established 403 cause.

A short computer-use `chrome://net-export` capture then tied the actual shopping stream to its TLS handshake: HTTP/2 session 30569, stream 33, depends on socket 30567 for `www.southwest.com`. The shopping response was 200 and the UI displayed flight results. This reused the existing incognito context, so it is not a fresh-session control. ClientHello advertised ALPS `0003026832` (h2), while server EncryptedExtensions contained only types 0 and 16, with no ALPS. TLS 1.3 resumed a PSK session but did not offer early_data. Thus this successful request did not negotiate ALPS; the advertised-byte mismatch and full ALPS support are separate questions. The native logging UI was stopped and confirmed the file was written, with private-information stripping enabled.

An advertisement-only diagnostic then sent actual `h2` bytes, confirmed by the echo service, while retaining the earlier signature-GREASE and DNT controls. The following Southwest run still returned two 403 responses with complete native bodies containing `403050700`. It is not a full ALPS implementation and remains outside the main worktree. Ordinary shopping header names matched the Chrome NetLog list; pseudoheaders and auto-generated Content-Length are absent from the pre-builder logger's capture layer, not proven absent on the wire. These controls did not satisfy the acceptance goal.

The combined control enabled the observed Chrome device fields, the 16 HAR-derived public origins, signature GREASE, DNT ordering and the diagnostic ALPS advertisement together. It still returned two shopping 403 responses with body code `403050700`. The observed console errors were reduced to a blocked Qualtrics script, with six origin-policy blocks in total. No recorded classic-script end event reported an execution error across 168 evaluations. This is bounded recorder coverage, not proof that all browser behavior succeeded.

The offline large-Worker replay was also run directly, omitting its startup wrapper. Obscura retained the same 157-byte result ending in `[90,164]`; Chrome's direct replay ended in `[78,184]`, versus `[72,184]` with the wrapper. Fixing the Performance prototype clock in addition to the instance clock did not change the direct results. The mismatch therefore persists without the wrapper, although wrapper removal changes one Chrome byte. No production change was based on these opaque output differences.

### Event clock discrepancy isolated

An offline API-call audit recorded the same fixed Math.random result in both engines. Obscura additionally called Date.now while constructing the message event; this is not evidence that the imported program itself reads that clock. A separate window-and-Worker fixture confirmed that `Event.timeStamp` currently returns epoch milliseconds and becomes -1 when page code replaces Date.now. Computer-use Chrome returned relative timestamps between the surrounding performance readings and was unaffected by replacing both Date.now and performance.now. The source currently assigns `this.timeStamp=Date.now()` in the Event constructor. This is a confirmed general event-clock defect suitable for a focused regression and repair; its relationship to Southwest 403 remains unproven. No event-clock production change has been made yet.

The event-clock repair is now in the main worktree. Event construction and the fallback performance.now implementation share a private realm-relative clock, using a captured Date.now primitive and a non-decreasing reading. Existing time-origin initialization is retained. This remains a millisecond wall-clock-based implementation with a backward clamp, not a new native high-resolution clock. The window/Worker regression failed before the repair and passes afterwards; 32 focused event/performance/Worker tests, all 184 runtime tests, and the 33-test Python SDK suite pass. The built runtime also passes the original computer-use Chrome comparison fixture. Complete core validation passed 1749 tests with four skipped. The specified release CLI build completed and 32 focused render+stealth checks passed. The unchanged obstacle course remained 32/33. Computer-use Chrome also loaded only 10 items in the original observer-intersection fixture, matching Obscura; the fixture never creates or observes the new sentinel described by its comment. The original gate is still not a pass.

A subsequent local runtime shopping check still returned three 403 statuses. Two retained bodies contain `403050700`; the first body handle had been released. That functional check overlapped validation builds and is not a controlled latency comparison. This event-clock correction does not establish a fix for Southwest acceptance.

All 66 deterministic rendering pairs for the event-clock source state navigated successfully, were nonblank and matched the retained baseline pixel-for-pixel. Six alternating baseline/candidate performance pairs, with no concurrent build, had startup medians of 20ms each, launch-to-ready medians of 404.1/406.2ms, RSS medians of 44.64/44.52MB, and 500ms idle CPU medians of 2.14/2.22ms. Both had 16 threads. These differences are within the stated noise boundary and do not establish an improvement.

A further offline context fixture found origin, isSecureContext and crossOriginIsolated undefined in both the Obscura window and Worker. Computer-use Chrome exposed the local HTTP origin, true and false respectively. Supplying these three values only inside the isolated localhost replay changed Obscura's final byte pair from [90,164] to [87,164], still different from Chrome. This is a diagnostic observation, not a production fix or a reason to unconditionally claim a secure context. Origin inheritance, trustworthy origins and ancestor context must be handled before exposing these properties.

### Initial iframe context continuation

The next source inspection found window and Worker context properties already implemented in the uncommitted worktree; the preceding missing-property observation is historical. The existing window/Worker context test passes in release nextest. A new native Chrome control also displayed flight results for the fixed Southwest query, while the retained local runtime returned four shopping 403 responses, each captured with body code `403050700`. That Chrome control reused an incognito context. The retained runtime was hashed, but had not been rebuilt in this continuation, so it is not a verified same-source build.

A separate local fixture exposed a remaining initial iframe defect: immediately after appending an iframe with no src, Chrome exposes the creator origin, inherited secure-context state and crossOriginIsolated=false, while location.origin remains "null". Obscura's fallback iframe window exposed none of the three context properties. The fallback now captures the creator's internal context for about:blank/srcdoc instead of reading the replaceable window.origin property. The new regression failed before the change and passed after it, covering HTTPS, ordinary HTTP, loopback, replaceable origin and readonly isSecureContext. The existing iframe-global and window/Worker context regressions also passed (3/3 total).

This is a bounded initial-window compatibility correction consistent with [HTML origin inheritance](https://html.spec.whatwg.org/multipage/browsers.html#concept-origin) and [Secure Contexts](https://www.w3.org/TR/secure-contexts/). It does not implement sandbox origin policies, change loaded-frame realm initialization, provide cross-origin isolation, or establish the cause of Southwest's rejection. A separate probe found data-URL Worker loading still fails with net::ERR_FAILED, while Chrome creates an opaque-origin Worker; that path is not changed here.

The locked release runtime rebuild then completed. Its local initial-iframe fixture matched the observed Chrome values, but its Southwest check still returned two shopping 403 responses with complete bodies containing `403050700` and no SDK callback errors. Source hashes were checked unchanged across the rebuild. The iframe correction is therefore insufficient to satisfy the business acceptance goal.

The first full release run in this continuation found two regressions among the existing uncommitted event-accessor changes: 1750 passed, two failed, four skipped. `_installEventAccessors` replaced inherited accessor behavior on body/frameset prototypes, hiding the body-to-window onload reflection. It also registered XHR property callbacks as listeners even though XHR dispatch already invokes those properties, causing duplicate callbacks and premature application completion counters. The installer now preserves inherited accessors and leaves XHR property dispatch to its existing implementation. Both original failing integration tests passed after these corrections. The final full release render run passed 1752 tests with four skipped; the specified release CLI and locked runtime builds succeeded. The final runtime's Southwest check still returned three shopping 403 responses, all with complete bodies containing `403050700`.

The unchanged obstacle course again passed 32/33, with only observer-intersection failing. The required 33/33 gate remains unmet. This continuation made no rendering-pipeline changes and makes no new real-site rendering or controlled-performance claim.

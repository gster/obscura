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

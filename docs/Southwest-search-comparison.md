# Southwest search comparison, 2026-09-16

> 当前进度、正式版本身份及下一步排查顺序见 [任务状态](Southwest-task-status.md)。本文保留历史样本，各阶段哈希和测试结果不代表最新状态。


The current Obscura runtime still receives shopping HTTP 403. Chrome operated
through native computer use succeeds. Changing primp back to wreq in an isolated
comparison did not restore the search. These observations do not identify the
server's rejection criterion.

最新的首次 shopping 前执行轨迹、两个调度缺陷及消融实验见
[脚本调度审计](Southwest-pre-shopping-scheduler-audit.md)。

## Controlled query and results

The query is WN, LGA to LAS, September 30, 2026, one way, one adult. Nonstop
filtering is applied to the returned itineraries. The browser-generated shopping
POST is 355 bytes, SHA-256
`119015f152d04726f2d8661fe54ade04fb6fbd89994de4fced2fddd969e7a395`.
The successful Chrome request and failed Obscura requests have this same body.

| Run | Observation |
| --- | --- |
| Current primp runtime, macOS Chrome 152 profile | Shopping 403 |
| Current primp runtime, Windows Chrome 145 profile | Shopping 403 |
| Isolated wreq adapter, Windows Chrome 145 profile | Shopping 403 |
| Playwright-launched headed Chrome 152, fresh context | Shopping 403 |
| Independent Chrome attached through Playwright CDP | Shopping 403 |
| Computer use, ordinary Chrome profile | Results displayed; new shopping POSTs return 200 |
| Computer use, newly opened incognito window | 26 itineraries displayed for the target route and date |
| Current primp runtime after the locator fix, native search click | Click completes; shopping 403, code 403050700 |
| Current primp runtime with 16 observed resource origins allowed | Shopping 403 |

The native Chrome comparison used the existing Chrome application, address bar,
page controls and DevTools UI. No Playwright control was used for these computer
use runs. macOS HTTP/HTTPS proxy settings were `127.0.0.1:7890`, also explicitly
configured in the runtime probes. Chrome's successful HAR records a loopback
remote address and HTTP/2. These facts control the local proxy endpoint; they
are not an independent measurement of the proxy's per-request upstream route.

Opening the target URL in ordinary Chrome displayed results. Reloading that
page did not issue a new shopping request, so the rendered result alone was
insufficient network evidence. Clicking September 29 and then September 30
issued two new shopping POSTs, both 200. The September 30 response was 126161
bytes and contained `success=true`, the correct route/date, 26 itineraries and
`containsNonstop=false`. The independent incognito result weakens the hypothesis
that success requires the ordinary profile's old site cookies or result cache.

HAR exports were saved outside the repository. The first export contained only
the filtered shopping requests; clearing the Network filter before exporting
again captured the resource timeline. Even sanitized HARs can contain dynamic
headers or response identifiers and must not be committed or replayed as a
replacement for the runtime's own session.

## Limits of the transport comparison

The wreq diagnostic used the current JS/DOM/runtime sources with only a temporary
transport adapter changed, using wreq 6.0.0-rc.29 and wreq-util 3.0.0-rc.12 with
their Windows Chrome 145 preset. It was compared with primp's Windows Chrome 145
profile. It was not the historical September 9 executable, and it does not
reproduce the historical page scripts or site decisions. Both samples failed;
this rules out a demonstrated recovery from simply substituting this adapter,
not every possible transport regression.

The historical wreq evidence established a shopping 200, while reporting React
errors and incomplete results rendering. It should not be described as a proven
end-to-end result-page success. The TLS/HTTP2 source and capture differences are
documented in [Primp and wreq comparison](Primp-and-wreq-comparison.md); none has
been established as the cause of the current 403.

SDK request events describe the request before transport defaults and cookie
injection. Missing Origin, Cookie or Client Hints in those events does not prove
that they were absent on the wire. Do not compare those event headers directly
with Chrome's final network headers as if they represented the same boundary.

## Direct-navigation submission audit

Both browsers in this audit navigate directly to the same constructed
`select-depart.html` URL. Neither starts from the homepage or fills a search
form. Chrome is operated through native computer use. The earlier form-click
probe above remains historical evidence for the separate locator correction.

A fresh Chrome incognito window was opened, DevTools Network recording enabled
before navigation, and the target URL entered in the address bar. Its first
shopping request returned 200 with 126161 decoded response bytes. It started
1.215 seconds after the document request; the response took 2.788 seconds.
This sample does not depend on changing the date in an existing result page.

An isolated diagnostic build records headers after Obscura injects cookies and
identity defaults, immediately before primp serialization. It is not a packet
capture: automatic Content-Length, HTTP/2 ordering, compression and TLS are
outside that observation boundary. The installed runtime was restored after
building the diagnostic copy; tracing is not enabled in the shipped source.

One direct-navigation Obscura sample produced:

| Submission | Start after document request | Body bytes | Cookie count | Dynamic `-a` length | Dynamic `-b` length |
| --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 2400 ms | 355 | 11 | 2268 | 6 |
| 2 | 3687 ms | 355 | 15 | 4307 | 7 |
| 3 | 4178 ms | 355 | 16 | 4324 | 6 |

All three bodies have the target query hash stated above. Between submissions,
only Cookie and the dynamic `-a` and `-b` header values changed. The SDK recorded
two 403 responses and a third request, so this sample alone must not be described
as three observed SDK responses. A subsequent diagnostic captured response
headers directly from primp: all three shopping requests returned 403, at
4809, 5562 and 6221 ms after navigation began (request starts were 3754, 4964
and 5648 ms). Thus the incomplete SDK response record is not evidence of a
successful third response.

The fresh Chrome request's `-a` length was 2362, unlike the 7715/7804 lengths
in the earlier long-lived Chrome session. Comparing the latter against a fresh
Obscura session confounds session age and execution history. Dynamic-header
length is not an acceptance criterion, and no token was decoded, copied or
replayed.

The merged Obscura request agrees with fresh Chrome on Accept, Accept-Encoding,
Content-Type, DNT, Origin, Referer, Priority, User-Agent, all three Client Hints,
all three Fetch Metadata fields, X-API-Key, X-App-Id and X-Channel-Id. The query
URL, including parameter order, agrees too. The initial language differs:
incognito Chrome sends `en-US,en;q=0.9`, while the configured Obscura macOS
persona sends `en,zh-CN;q=0.9,zh;q=0.8`. A separate Obscura run set both navigator
languages and Accept-Language to the incognito values and still returned 403.
Missing Content-Length at the pre-primp observation point cannot be interpreted
as missing Content-Length on the wire.

Sanitized HAR omits cookies. The earlier claim of five Chrome cookies is
withdrawn: the native accessibility text of the long Cookie header was
truncated. A new successful incognito sample was checked using the independent
rows in DevTools **Request Cookies**, separated from **Response Cookies**.
It contained ten request cookies. See the provenance audit below. Do not count
cookies from a potentially truncated header-value preview.

Response-body SHA-256 comparisons also established identical app.js,
vendor.js, booking bootstrap data, translations/content, analytics.js, the
377693-byte resource bootstrap and all four of its child scripts. Those
resources loaded successfully in both engines. Some other session-dependent
resources differed; a later fallback navigation also fetched a different
version.json. Resource comparisons must therefore preserve navigation and
request occurrence instead of collapsing an entire session by pathname.
Identical fetched source does not establish identical JavaScript execution,
DOM behavior or browser API outputs.

The remaining unproven boundary is the runtime behavior of those same scripts
and the transport serialization below the recorded headers. Neither the
language control nor the current-runtime wreq substitution recovered the
search. There is no demonstrated standards defect that explains the 403 yet;
changing cookie values, adding arbitrary waits or matching token lengths would
not constitute a root-cause fix. Acceptance remains an independently generated
Obscura shopping 200 with validated route/date and parsed itineraries.

## Cookie provenance audit and corrected count

The new Chrome sample navigated directly to the constructed query URL, with
Network recording enabled before navigation. Its shopping request started at
2171 ms and returned 200. The ten complete Request Cookies rows were:

`_up`, `ak_bmsc`, `akaalb_alb_prd_southwest_spa`,
`akavpau_prd_air_booking`, `AMCV_65D316D751E563EC0A490D4C%40AdobeOrg`,
`mbox`, `OptConsentGroups`, `PIM-SESSION-ID`, `sRpK8nqm_sc`, `swa_FPID`.

The response table also contained `sRpK8nqm_sc`; that row is not an eleventh
request cookie. Cookie values were neither copied between browsers nor added
to source documentation.

A diagnostic source copy recorded HTTP Set-Cookie names/attributes and native
V8 call stacks for document.cookie writes. Its initial Obscura request had
nine names in common with Chrome, plus `swa_spa_grp` and `AMCVS_...`, and lacked
`swa_FPID`. The causes are distinct:

| Difference | Observed origin | Evidence |
| --- | --- | --- |
| `swa_spa_grp` vs `swa_FPID` | Server response, before page JavaScript | Chrome's first HTML response sets `swa_FPID`; the original Obscura sample's first HTML response sets `swa_spa_grp` at 674 ms. The latter receives `swa_FPID` from `/akam/13/pixel_5dfc2285` at 3942 ms, after its first shopping request at 2599 ms. |
| Extra `AMCVS_...` before shopping | Analytics execution under the configured origin restriction | V8 stack reaches analytics.js cookieWrite; the supplied analytics code writes the session marker through `_setFieldExpire` for MCOPTOUT. The initial configuration allows only www.southwest.com and blocks the external Adobe request. |

A bounded A/B/A diagnostic changed only the allowed origin list in temporary
run configuration. It added `https://dpm.demdex.net` and
`https://smetrics.southwest.com`, both observed in Chrome's successful timeline.
It did not disable the origin guard, change production configuration, replay
cookies or change the query entry point.

| Run | Adobe request | AMCVS write | First shopping | Result |
| --- | --- | ---: | ---: | --- |
| A: www-only | Blocked by OriginGuard | 2204 ms | 2599 ms, 11 cookies | 403 |
| B: two observed Adobe origins added | `/id` starts 4284 ms; 302 then `/id/rd` 200 at 5825 ms | 5828 ms | 4744 ms, 10 cookies | 403 |
| A again: www-only | Blocked by OriginGuard | 2333 ms | 2752 ms, 11 cookies | 403 |

This establishes that the origin restriction changes the analytics completion
path and makes its session cookie appear before the first shopping request.
In Chrome, the Adobe request started at 1851 ms, and its first response arrived
after shopping started at 2171 ms. A Cookie jar error is not needed to explain
this difference.

In run B the server also set `swa_FPID` in the initial HTML response at 585 ms,
so the first shopping request's set of ten cookie names exactly matched Chrome.
It still returned 403. The same runtime/transport can receive either initial
server cookie variant; the experiment does not establish which server-side
routing or session criterion selects it. The allowed-origin change cannot
explain an earlier initial-response choice by executing later page JavaScript.
The names alone must not be treated as a reliable browser classification.

The supported conclusion is narrower than a search fix: the previous 5-versus-11
comparison was a measurement error; the reproduced 10-versus-11 comparison is
explained by the analytics request boundary and the separately observed server
cookie variant. Identical cookie-name sets did not recover shopping. Full
runtime replacement remains unverified.

## Confirmed locator bug and correction

The fallback booking form has a visible submit button whose text is
`Search flights` and whose `aria-label` is empty. The SDK's accessible-name
resolver returned that empty attribute immediately. Thus a CSS submit-button
query found one visible, enabled button, while the role-and-name query found
zero and timed out with `NOT_SENT`.

A reduced page opened through Chrome computer use exposes the text names for
both empty and whitespace-only `aria-label` attributes. The real SDK regression
`test_empty_aria_label_falls_back_to_button_text` failed before the correction.
The resolver now ignores empty or whitespace-only labels and continues to its
existing naming fallbacks. Nonempty explicit labels still take precedence.
The test checks both name resolution and an actual native click that updates
the fixture. The Rust `role_locator_ignores_empty_aria_label` test covers the
same resolver path.

After rebuilding, the live role locator found one button and its native click
completed. The ensuing shopping response remained 403. This correction resolves
the interaction timeout, not the independent server rejection.

## Dynamic classic-script fetch correction

A separate two-origin HTTP fixture exposed another implementation difference.
Dynamic classic scripts always used `no-cors` with `same-origin` credentials,
ignoring their `crossorigin` attribute. A cross-origin `<base>` also incorrectly
became the request's origin. Chrome, operated through computer use, produced
these results on the same fixture:

| Script configuration | Target cookie sent | Origin header | Event |
| --- | --- | --- | --- |
| No crossorigin attribute | Yes | Absent | load |
| anonymous | No | Document origin | load |
| use-credentials | Yes | Document origin | load |
| anonymous, missing CORS response permission | No | Document origin | error |
| anonymous, relative src under cross-origin base | No | Document origin | load |

The SDK regression `test_script_fetch.py` failed on the old runtime and passes
after the correction. Preparation now snapshots the script's CORS settings;
the base URL only resolves `src`. The existing native fetch path handles CORS
validation and credentials. This follows the HTML specification's
[potential-CORS request algorithm](https://html.spec.whatwg.org/multipage/urls-and-fetching.html#create-a-potential-cors-request).

The CORS-only intermediate runtime was
`f59957cdaa31a62f4490f960a0fe24d83dcac10e56cc2e38d8f86683f88c2ff2`.
Its direct-goto Southwest workflow still returned `SEARCH_HTTP_403`, with three
shopping responses of 403. This fixes a real script-loading mismatch but is not
evidence that this mismatch caused Southwest's rejection. Six focused script
and Worker regressions and all 32 SDK tests passed.

Extending the same fixture to echo `Sec-Fetch-Dest` exposed a second mismatch:
Chrome sent `script`, while Obscura sent `empty`. The expanded regression failed
before the second correction and passed afterwards. The native fetch operation
now accepts a browser-owned destination; dynamic classics pass `script`, while
ordinary fetch/XHR callers retain the default. Both HTTP transports, redirect
hops, interception, and passive request/response events retain that resource
type. Script and generic-fetch metadata are also covered by a focused Rust
regression. The previously fixed Worker and callback-order cases remain green.

The final runtime after both corrections is
`8399f9b5067143bd541f7b1c5f292de672a4649847c40c9e612b6e7c22bc35ab`.
Both the original www-only configuration and the temporary observed-Adobe-origin
configuration still returned `SEARCH_HTTP_403` on a direct goto. The first
parallel live attempt timed out during runtime initialization, before navigation;
a subsequent standalone attempt completed and received four shopping 403s.
That startup timeout is retained separately from the server rejection.

Validation of this revision:

- Seven focused release-mode script/Worker/fetch-metadata tests passed.
- Isolated runtime release nextest: 184/184 passed.
- SDK run: 31/32 passed; the identity test timed out during initialization.
  A focused rerun of that test and the new HTTP script fixture passed 2/2.
  This is not represented as an initially green full run.
- Both required render and render,stealth CLI release builds completed. Real
  CLI runs without stealth and with explicit `--stealth` matched Chrome on all
  five HTTP fixture results, including destination. The logged HTTP 0/error for
  the denied-CORS case is expected.
- Final full render nextest: 1720/1723 passed, four skipped. Failures were
  `render_resource_loads_share_one_page_wide_concurrency_limit`,
  `timers_inside_a_fixed_wait_observe_bytes_that_land_during_it`, and
  `a_miss_created_inside_an_awaited_expression_loads_during_the_wait`.
- Serial rerun: the awaited-expression case passed; the concurrency-limit and
  fixed-wait font cases still failed. Their local fixture threads logged
  `read_fixture_headers` panics on `WouldBlock` while reading request headers.
  No fixture timeout or assertion was relaxed, and the full gate remains red.
- The obstacle course remained 32/33. `observer-intersection` expected `io:50`
  but returned an empty completion marker, matching the pre-change run.

## Validation before the script-fetch correction

- Isolated runtime release nextest: 184/184 passed.
- Real runtime SDK `test_automation`: 25/25 passed, including the new regression.
- Root render release nextest: 1722/1723 passed, four skipped. The failing
  `obscura-cli::mcp_client::test_wait_for_selector` passed in a focused rerun;
  the initial full-run failure remains recorded. A second full run passed
  1720/1723 and failed three asynchronous render-resource timing tests:
  `a_load_that_finished_during_the_scan_does_not_cost_the_deadline`,
  `timers_inside_a_fixed_wait_observe_bytes_that_land_during_it`, and
  `a_miss_created_inside_an_awaited_expression_loads_during_the_wait`.
  The full gate is not green; no unrelated timing assertions were relaxed.
  A serial focused rerun passed two of these three; the fixed-wait font test
  still failed because the measured text width did not change after loading.
- Required render release CLI build completed.
- Companion obstacle course: 32/33. Its existing IntersectionObserver stage
  still fails; the locator correction does not change that browser behavior.

The latest runtime has been exercised against the live search after the fix.
The business acceptance remains failed, with no fabricated empty-flight result.
That runtime SHA-256 was
`70e4c92329cb2bcb0a013728ef93f1f77873ff3735a8fa2a92838997fe6b8f6f`;
its workflow returned `SEARCH_HTTP_403`. Temporary diagnostic adapters and
instrumentation are outside the repository and are not the installed runtime.

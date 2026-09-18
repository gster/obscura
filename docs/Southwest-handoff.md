# Obscura / Southwest: handoff

Updated: 2026-09-18. Current session: `1edb7516-1fca-4453-aa19-3b49d3aa05e5`.

**Status: 浏览器兼容性修复已推进，但 Obscura shopping 仍返回 403，根因未确定。** 用户于 2026-09-17 确认：同一代理 IP 下，隐身 Chrome 能成功查询。因此排除“代理 IP 本身被封锁、该出口无法访问 Southwest”作为本轮失败的独立解释，不再把更换代理/IP 作为优先调查方向。同一代理下 Obscura 25+ 次失败不能推翻 Chrome 成功对照；后续应聚焦浏览器环境、遥测、会话及请求/传输行为差异。当前候选的固定输入 Worker 回放已与 Chrome 完全一致，早期字节差异与属性缺失记录须按历史证据阅读；这不等于全部真实页面遥测已对齐。新确认 sendBeacon 是空实现，但诊断补发后仍为 403。Obstacle course: 32/33（IntersectionObserver 既有失败），尚未达到 33/33 门禁；最近完整 release render nextest 为 1758 passed、4 skipped。

Latest production update: iframe 断开/重插入及过期加载修复，并修复旧子文档错误路由父页的问题；runtime SHA-256 `8a6ee69fb55f2bcf4fb3b262ce1465302c14a0c5d7552adf4ae1a88339fe0d0f`（含 iframe 跟踪性能优化）。本轮生命周期诊断未再出现重复定义异常，但 shopping 仍为 403。

Earlier code changes: (1) VisualViewport width/height setters in bootstrap.js, (2) 35 window-only property cleanup in worker.rs. Prior sessions: 23 WebIDL classes, HTMLDocument, event handler prototype descriptors, element property getters/setters, Worker isolation with independent V8 isolates, structured clone, event clock, OffscreenCanvas WebGL.

## 接手顺序

1. 先读 `AGENTS.md`、本文、`docs/Worker-compatibility.md`。`docs/Southwest-fix-record.md` 是历史记录，早期结论不能覆盖本文的最新证据。
2. **保留当前未提交改动，不要先 pull、reset 或用 HEAD 覆盖工作区。** 当前目录 `/Users/gster1981/work/obscura`，分支 `main`，HEAD `911a48e390a1f383d53fbfc249889bbb25bc68cc`（`fix(worker): isolate execution and preserve runtime progress`）。新修复尚不在 Git 提交中。
3. 核验下述本机证据目录、二进制与服务。跨设备时，Git 不会带走未提交源码和 `/tmp` 证据。
4. 先阅读下方“续推核验”再选择上下文属性线索。当前工作区已存在 Window/Worker 的 `origin`、`isSecureContext`、`crossOriginIsolated` 实现，不能再按“尚未实现”重复补丁；需区分顶层、初始 iframe、已加载 frame realm 和 Worker 的语义及验证范围。
5. 每次涉及业务判断时补当前 Chrome 成功控制，使用相同查询、明确记录构建和代理输入。以 shopping 响应和行程数据验收；保持“不出现 403”为主要指标，不能用离线字节变化代替业务成功。

## 持续约束

- Chrome 只通过 native computer use（`cua.getApp("Google Chrome")`）操作，不使用 browser-provider、Playwright 或 CDP 控制。下次先读取当前 UI，不沿用旧坐标或元素索引。
- 所有测试程序在本机执行；mini32 仅作为可选代理入口。直接 goto 搜索 URL，不从首页交互。
- 不增加 Southwest 专用等待、脚本屏障、域名特判。
- **2026-09-18 用户明确放宽**：为调试需要，允许把 Chrome 观测到的 Cookie 导入 Obscura 会话做对照实验。此授权仅用于本机诊断，产物不得提交 Git 或公开；生产源码不得因此加入站点特判。token/凭据类材料仍只在 `/tmp` 私有目录保留。
- 原始头、Cookie、正文可以本机保留，不能提交 Git 或公开发布。本轮同一代理 IP 的 Chrome 成功对照已由用户确认；后续新实验仍应记录代理和出口输入，不能仅凭同一本机代理入口推断出口相同。
- 不根据单个指纹、Cookie 数量、执行顺序、无 JS 异常或单纯 HTTP 200 推断成功或根因。
- 当前不要派发子代理。未来 agy 接手不等于本会话获准创建任务。
- 使用 release `cargo nextest`，不用 `cargo test`；不要 bulk `cargo fmt`。保留 watchdog、SSRF、panic 和队列边界。

## 续推核验（2026-09-18，iframe 生命周期修复）

- native Chrome 新夹具 `fixtures/frame-reinsert-lifecycle.html` 确认：离线 fragment 中的 iframe 以及移除后的 contentWindow/contentDocument 均为 null；保存的旧窗口引用保留数据及原 document，closed 为 true；重插入和普通 DOM 移动创建不同的窗口/document。最新 runtime 的 `/tmp/obscura-resume-20260917/resume-frame-reinsert-fixed/result.json` 与 Chrome 的所有布尔结果一致。
- 生产修复覆盖直接移除、祖先移除、innerHTML/textContent 清空、普通移动和 closed ShadowRoot；拒绝的循环插入不丢弃原上下文。只跟踪已初始化的 iframe，并保留无 iframe 页面原有 native DOM 路径。已断开的加载被作废；同 URL 重插入也递增 generation，旧响应或旧正文不能再创建新 realm。离线 src 赋值延后到连接时请求，等待队列用弱引用保留待加载元素。
- 已加载 frame 的集成测试确认：移除后 getter 即刻为 null，重插入后旧 realm 释放、新 realm 重新执行子页脚本，旧 window/document 引用仍可读取。新增 3 项 JS 回归先 red 后 green，连同已加载 frame 集成回归与相关旧覆盖合计 **90/90** 通过；独立 runtime **184/184** 通过。本轮没有实现完整 srcdoc/sandbox 或导航期间稳定 WindowProxy 的全部语义，不能称 iframe 全面兼容。
- 依据包括本机 Chrome 对照及 [HTML iframe 连接与移除步骤](https://html.spec.whatwg.org/multipage/iframe-embed-object.html#the-iframe-element)。不放宽不可配置属性语义，不加入 Southwest 特判。
- 新生产 runtime 已构建并保留为 `/tmp/obscura-beacon-20260918/runtime-iframe-lifecycle`，SHA-256 `a9b2f7d2f527fe07f25c36efab01152ce32859f669b27175208b344622423bdb`；源码哈希见同目录 `iframe-lifecycle-source-manifest.json`。本轮仍只更新本地 runtime，没有远端部署或消费仓库 pin 变更。
- 本轮 native Chrome 现有隐身上下文新标签页仍显示相同查询的航班列表。`live-iframe-lifecycle-production/` 使用新生产 runtime、相同代理入口、16 个观察到的 origin，无诊断 preload，四次 shopping 均为 **403050700**，正文全部完整。
- `live-error-stack-lifecycle/` 使用同步源码的隔离诊断版本，最终页面审计未再捕获原来的 `Cannot redefine property` error/unhandledrejection；该次 shopping 仍五次 **403050700**。审计范围仍是最终页面，不能从一次未观察到异常推断所有导航阶段无异常，更不能宣称该异常是 403 根因。下一步应继续取证首次 shopping 前的会话、请求及环境差异。
- 补测 `fixtures/frame-retained-loaded.html` 发现独立缺陷：host 释放已加载 realm 后，旧 document.title 错读为父页面标题。native Chrome 保留子页面标题；集成回归进一步证明旧文档写入也会错误修改父页面。已修复 `op_dom` 的 frame-id 路由：非零未知 id 返回空结果，不再回退父页；V8 context 拥有文档状态，已退役 id 表只持有弱引用，使 JavaScript 保留的旧 DOM 仍访问自己的状态。
- 新 GC 回归确认：保留旧 document 时状态仍可读写，释放全部 JS 引用并触发 GC 后状态释放。还修正了保留状态造成的排队任务取消回归：销毁 browsing context 时递增 document generation，使旧任务失效；原 `dropping_frame_cancels_its_queued_posted_task` 再次通过。最终相关覆盖 **91/91**；早期一轮完整 1757 passed 是追加此路由修复前的结果，不能作为最终门禁。
- 路由修复后、性能优化前的生产 runtime SHA-256 **`fc742e04abd7b4f7577def713df28bd6c5791ee5df4e39041592316b9bdda523`**，保留为 `/tmp/obscura-beacon-20260918/runtime-iframe-final`，源码清单 `iframe-final-source-manifest.json` 包含新增的 `frame.rs` 和 browser 回归。`resume-frame-retained-loaded-fixed/result.json` 的旧文档标题与 Chrome 一致；最终 runtime 184/184 通过。`live-iframe-final-production/` 的三次 shopping 仍均为 **403050700**，正文完整。
- `live-error-stack-iframe-final/` 是追加旧文档路由修复后的诊断复测：最终页面捕获的 error/unhandledrejection 数组为空，三次 shopping 仍为 403；此有限观测不能证明所有脚本/导航阶段无异常。诊断二进制与插桩清单为 `runtime-events-iframe-final-diagnostic`、`events-iframe-final-manifest.json`，均仅在本机。
- 性能对照发现并修复新增开销：存在一个活动 iframe 时，原补丁在无关节点移动上查询 native DOM 归属，隐藏布局 2000 节点/200 次移动的 DOM 中位数由旧版 13ms 升至 16ms。改为记录活动 iframe 的祖先路径及引用计数，仅相关子树移动才扫描上下文；移除时撤销记录，重插入时重建。相关 91 项回归再次通过，含祖先、closed ShadowRoot、拒绝的循环移动和过期响应。
- 优化后六组交替、独立进程对照（相同 fixture、persona、viewport、settle，测量期间没有构建或测试）：旧版/候选 DOM 中位数 **13/14ms**，均范围 13–14ms；启动 19.5/20ms；launch-to-ready 451.78/451.38ms；RSS 84.48/85.57MB；idle CPU 2.77/2.84ms；均 16 threads，自主 Worker 结果均正确。候选 launch-to-ready 存在 1179.58ms 单次离群值，不能声称所有延迟无回退或性能提升。证据 `performance-lifecycle-optimized/performance.json`，基线仍为 fragment 修复版 `485f1e6d…`。优化前结果分别保留在 `performance-lifecycle-hidden/` 和小型可见夹具 `performance-lifecycle/`；最初可见 2000 节点试验在基线读取结果时超时，不构成候选回退证据。
- 当前本地生产 runtime SHA-256 **`8a6ee69fb55f2bcf4fb3b262ce1465302c14a0c5d7552adf4ae1a88339fe0d0f`**，保留为 `/tmp/obscura-beacon-20260918/runtime-iframe-optimized`，源码清单 `iframe-optimized-source-manifest.json`。独立 runtime release nextest **184/184**；`resume-frame-reinsert-optimized/result.json`、`resume-frame-retained-optimized/result.json` 与本轮 Chrome 夹具结果一致。此 runtime 的 `live-iframe-optimized-production/` 四次 shopping 均为 **403050700**，四份回调正文完整，无 callback error；未注入诊断 preload。
- 优化后同步诊断 `live-error-stack-iframe-optimized/` 的最终页面 error/unhandledrejection 数组为空，native 事件未捕获重复定义异常；三次 shopping 仍为 403。产物及诊断源码哈希保留在 `runtime-events-iframe-optimized-diagnostic`、`events-iframe-optimized-manifest.json`。该观测仍只覆盖最终页面，不能证明所有导航阶段无异常。
- 本轮日志前缀为 `/tmp/obscura-iframe-lifecycle-`、`/tmp/obscura-retired-frame-`、`/tmp/obscura-iframe-final-`、`/tmp/obscura-iframe-optimized-`，原始网络正文与诊断栈仅保留本机。最终源码完整 release render nextest **1758 passed、4 skipped、0 failed**，指定 release CLI 与 locked runtime 构建通过；原样 obstacle course **32/33**，仍仅 `observer-intersection` 失败，未达到 33/33 门禁。全部本轮构建和测试已结束，源码及生产 runtime 哈希已核对一致。没有远端部署或消费仓库 pin 变更。

## 续推核验（2026-09-18，Obscura/Chrome 信号差分与 WebIDL 品牌修复）

- **业务仍未通过。** 固定查询（BWI → MCO，2026-09-30）在本轮修复后依然全部 **403050700**。本节全部结论以“403 未消除”为前提，不能把兼容性修复当作业务已恢复。
- 本轮换了取证路线：不再依赖旧 HAR 的请求链推断，而是**在同一台机器、同一代理出口上，用同一份本机夹具分别驱动 Obscura runtime 与原生 Chrome，逐字段对比浏览器信号**。夹具把结果 POST 回服务器，因此两个引擎的输出是服务端捕获的原始数据，而不是页面自述。
  - 夹具：`fixtures/akam-probe.py`（约 110 项信号：navigator/plugins/window.chrome/screen/时区/权限/WebGL/canvas/自动化残留变量/原生函数 toString）、`fixtures/proto-probe.py`（原型与 `Symbol.toStringTag`）、`fixtures/sweep-probe.py`（约 130 个接口原型 + 32 个 window 函数 + 37 个 navigator 成员）、`fixtures/alias-probe.py`（构造器/原型身份）。
  - 证据根目录 `/tmp/obscura-resume-20260918/`，配对结果 `sweep-diff*.txt`、`proto-diff.txt`、`alias-results-18861.jsonl`。原始结果仅本机保留。
- **修复前测得三类接口层缺陷**（均为 Chrome 与 Obscura 的确定性差异，不是推测）：
  1. **75 个接口原型缺少 `Symbol.toStringTag`**，例如 `Object.prototype.toString.call(navigator)`、`(new XMLHttpRequest())`、`Headers`、`Event`、`Request`、`URL` 等全部返回 `"[object Object]"`，Chrome 返回 `"[object Navigator]"` 等。这是 WebIDL 规范要求，也是机器可判定的引擎指纹。
  2. **31 个 window 原生成员函数名为空且 toString 无名**（`fetch`、`alert`、`setTimeout`、`addEventListener`、`getComputedStyle`、`createImageBitmap` 等），Chrome 为 `function fetch() { [native code] }`。
  3. `createImageBitmap` 的 toString **直接泄漏 JS 源码**：`function() { return Promise.resolve(new ImageBitmap()); }`，而不是 native code。
- **修复（生产源码，已落 `bootstrap.js`）**：文件末尾新增一次性的 `_installInterfaceBrands` 模块。品牌值直接取自构造器自身的名字，避免在每处定义点重复维护；对 `window`/`navigator`/`location`/`screen` 这几个单例对象单独校验其实际 brand 再补；原生成员在改名成功后才登记 native toString（改名失败不谎报 native）。
- **同轮发现并修复两处构造器别名**（比我最初的品牌修复更严重，属于接口层级错误）：
  - `globalThis.EventTarget = Node` 曾出现两处（`bootstrap.js` 旧 2549、12973 行），使 `window.EventTarget === window.Node` 为真，并让 `PermissionStatus`、`BatteryManager`、`ServiceStatus` 等**非节点**事件目标自称 `instanceof Node`。现改为独立的 `class EventTarget`，`class Node extends EventTarget`，Node 不再自带监听器方法（与 Chrome 一致：`Node.prototype` 无 own `addEventListener`，由 `EventTarget.prototype` 继承）。注意 `Node` 构造函数因此成为派生构造函数，必须 `super()`，本轮已同步修正。
  - `globalThis.HTMLElement = Element` 使 `window.HTMLElement === window.Element` 为真，所有 `HTML*Element` 的祖先都错了一级。现改为独立的 `class HTMLElement extends Element {}`，并把 `HTMLDivElement`/`HTMLSpanElement`/……/`HTMLFormElement`/`HTMLTextAreaElement`/`HTMLCanvasElement`/`HTMLImageElement`/`HTMLMediaElement`/`HTMLTrackElement` 等改为 `extends HTMLElement`。**SVGElement 保持直接继承 Element**，与浏览器一致。
- **修复后本机夹具复测结果**：接口品牌不匹配 **76 → 0**（`Element` 一项随 HTMLElement 拆分一并消除）；无名 window 成员 **31 → 0**；构造器别名 **2 → 0**；`Object.prototype.toString.call(window)` 为 `[object Window]`。Chrome 对照同一夹具全部一致的位置不因本次改动而变化。
- 新增回归 `interface_prototypes_carry_webidl_brands_and_native_member_names`（`crates/obscura-js/src/runtime.rs`），断言 21 个实例 brand、6 个 window 成员名与 native toString、`EventTarget` 与 `Node` 不同构造器/不同原型、`new EventTarget()` 可构造、`PermissionStatus` 是 EventTarget 而非 Node、`HTMLElement !== Element` 且 `div` 原型链为 `HTMLDivElement → HTMLElement → Element → Node → EventTarget`、SVG 是 Element 但不是 HTMLElement、以及纯对象仍为 `[object Object]`（防止品牌被误加到 `Object.prototype`）。
- **仍未解释 403，且已获得更明确的 Akamai 线索（下一轮首要方向）**：
  - Obscura 会话确实向 Akamai Bot Manager 发了传感器流量（`GET /akam/13/<id>` 与 `POST /akam/13/pixel_<id>`，均 200），而保留的 Chrome 成功 HAR 的 `www.southwest.com` 路径集合里**没有**这两条。
  - `/akam/13/pixel_*` 响应头自带 `akamai-request-bc: [a=...,n=JP_13_TOKYO,o=20940]`（Akamai 边缘）与 **`terms-of-service: Unauthorized access, display, or use of Southwest's Company Information, including fare data, is prohibited`**，并 `set-cookie: ak_bmsc=<hash>~000000000000000000000000000000~...`——校验位段为**全 0**，即仍处于未通过状态。
  - Obscura 收到 `ak_bmsc` 与 `bm_mi`（Akamai 缓解 cookie），但**在整轮会话中从未收到 `_abck`**；Chrome 同站点会话收到了 `_abck`、`bm_sz`、`sRpK8nqm_sc`、`_cc-x`、`f5_cspm`，且未收到 `ak_bmsc`/`bm_mi`。收到的 `_abck` 才是 Akamai 放行所需的“校验通过”cookie。
  - 这些是“被判为机器人”方向的**证据**，但本轮**没有**证明它是 403 的直接充分原因：未做“注入放行态 cookie 后 shopping 是否转 200”的对照实验。下一步应设计该对照（不改生产、不加站点特判），并继续用同一夹具差分 Akamai 传感器脚本实际读取的信号。
- **已排除项（本轮新增）**：shopping POST 请求体与 Chrome 成功样本**逐字节同构**（均 355 字节、同一组键，仅机场码按查询不同），故请求体不构成本轮 403 的解释。cookie 传递链路本身是通的：受控 loopback 夹具中 Obscura 与 Chrome 都能把导航 `Set-Cookie` 带到后续同源 fetch/XHR 与跨源 `credentials: include` 请求，`credentials: omit` 时都不带。
  - 夹具同时暴露两处**其它** cookie 差异（不与 403 直接相关，未在本轮修复）：loopback 上 `Secure` 的 domain cookie 被 Obscura 丢弃而 Chrome 保留（疑似对 loopback 未按 potentially-trustworthy 处理），以及 cookie 发送顺序与 Chrome 不一致（Chrome 为 `hostonly; .leadingdot; ak_bmsc; _abck; bm_sz; cookie3`，Obscura 为 `bm_sz; _abck; hostonly; .leadingdot; cookie3`）。证据 `cookie-probe-log-{obscura,chrome}.jsonl`、`cookie-probe-obscura/page-result.txt`。
- **构建期反馈回路**：`crates/obscura-js/build.rs` 生成 V8 快照时会让 `bootstrap.js` 真实执行，任何初始化期异常都会以致命错误中止构建（例如本轮 `HTMLElement is not defined`）。改 bootstrap 后先跑 `cargo build -p obscura-js --features render` 可最快定位语法/作用域错误。
- 本轮未派发子代理（遵循交接约束），未做远端部署，未改消费仓库 pin。运行时重建后 SHA-256 见下方记录。

### Cookie 注入对照实验（用户授权后，决定性否定结果）

- 先纠正上一版本文中的一个**错误假设**：先前根据“Obscura 收到 `ak_bmsc`/`bm_mi` 但从未收到 `_abck`”推测 `_abck` 是放行条件。经本轮直接从 Chrome 成功会话的 NetLog 读取**实际发出的 Cookie 头**，该假设**不成立**——Chrome 成功的那次 shopping POST 请求上**没有** `_abck`，也**没有** `bm_sz`。该假设已撤回，不要再据此排查。
- Chrome 成功 shopping 请求实际携带的完整 cookie（10 个）：`swa_spa_grp`、`akaalb_alb_prd_southwest_spa`、`akavpau_prd_air_booking`、`sRpK8nqm_sc`、`PIM-SESSION-ID`、`ak_bmsc`、`OptConsentGroups`、`AMCV_65D316D751E563EC0A490D4C%40AdobeOrg`、`mbox`、`_up`。证据：本轮 `chrome-netlog-fresh.json`（Everything 模式）中的 shopping 请求头，以及 `chrome-shopping-cookies.json`。
- Obscura 会话与本轮的确定差异：Obscura 会收到 `swa_spa_grp` 与 `sRpK8nqm_sc`，但**从未收到 `akavpau_prd_air_booking` 和 `akaalb_alb_prd_southwest_spa`**（对比 `live-final` 与 `live-2` 的 set-cookie 名单）。这两个是 Akamai 应用校验与 LB 亲和 cookie，因此曾被列为重点候选。
- **实验设计（三臂，同查询、同代理、同 origins/public persona）**，用临时诊断构建 `runtime-cookie-seed`（源码已还原，`git diff runtime/src/browser.rs` 为空）：
  - 对照组（不注入）：全部 **403050700**（7 次）。
  - 臂 1（仅 `swa_spa_grp` + `akaalb_*` + `akavpau_*` + `sRpK8nqm_sc`）：全部 **403050700**（3 次）。
  - 臂 2（Chrome 成功请求上的**全部 10 个** cookie）：全部 **403050700**（4 次）。
- **仪器校验（关键，否则否定结果无效）**：(a) 用一个 loopback 夹具证明注入机制端到端有效——种子 cookie 出现在首个导航请求及各后续同源/跨源请求的服务端观测里（`seeded_probe=YES_SEEDED`）；(b) 直接驱动诊断二进制并捕获其 stderr，确认 SW 会话打印 `[diagnostic] seeded 10 cookies from workspace`。
  - 注意：Python SDK 启动子进程只保留 `PATH`/`LANG`/`TZ`，**环境变量无法传递进 runtime**，因此种子文件必须放在 `<workspace>/seed-cookies.json`。另有坑：runtime 校验工作区必须是规范路径，`/tmp` 是 `/private/tmp` 的符号链接，必须传 `/private/tmp/...`，否则 `INVALID_WORKSPACE`。
- **结论**：**Cookie / 会话状态不是本轮 403 的决定性因素。** Chrome 的成功 cookie 集不足以让 Obscura 通过。后续不应再把“补 cookie”“补 `_abck`/`bm_sz`”作为方向；应转向请求/传输层与边缘判定（TLS/H2 指纹、边缘节点行为），以及 OkHttp/AT 类 SDK 之外仍未被观测的执行差异。
- 产物：`/tmp/obscura-resume-20260918/{armB-control,armA1-akamai,armA2-all,ws-direct,direct-stderr.txt,runtime-cookie-seed}`。原始 cookie 值、请求头仅本机保留。
- 本轮开始新证据根目录 `/tmp/obscura-resume-20260918/`（权限 700）。**`/tmp/obscura-resume-20260917` 及 `/tmp/obscura-beacon-20260918` 在本机已不存在**（交接文档中引用它们的路径需要重新生成）。夹具端口 83860/12900 与 mini32 隧道在本机均未运行；本轮改用 `ssh -L 127.0.0.1:17890:127.0.0.1:7890 mini32`（本机 7890 与 17890 出口 IP 相同，实测均为 `23.249.17.98`）。本机夹具使用 18801–18861 端口。

### 请求重放实验、误报更正与语言列表修复

- **请求重放（重要否定结果）**：从 Chrome 成功会话的 NetLog 中取出那次 shopping POST 的**线上 HTTP/2 请求**（`:method/:authority/:scheme/:path` + 26 个实体头，含 1566 字节 `cookie` 与 355 字节 body），用 curl 走**同一代理**原样重放。第一次重放因我重复附加了 `Content-Length`（原头里已有 `content-length: 355`）得到 **400** + Akamai 边缘错误页（`errors.edgesuite.net`）；去掉重复头后得到 **403 / `403050700`**，与 Obscura 完全相同的错误码。
  - **本条已被用户指出为无效实验，不要作为证据引用。** curl 带的是 LibreSSL/SecureTransport 栈，与 primp/Chrome 的 TLS 指纹完全不同，因此该 403 只说明“非 Chrome 客户端会 403”，对“请求材料是否充分”没有任何信息量。下方“primp 层重放”才是有效版本。
  - 证据：`chrome-wire-headers.json`、`curl-replay.cfg`/`curl-replay2.cfg`、`curl-replay2-headers.txt`、`curl-replay2-body.bin`（本机保留，含原始 cookie 与 api-key，不得提交）。
- **更正上一版本文中的一处误报**：先前记录“Obscura `navigator.language` 为 `en`、`languages` 为 `["en","zh-CN"]`，与 Chrome 的 `en-US` 不符”。经查那是**我自己的探测脚本硬编码了 persona**（timezone `Asia/Shanghai`、未设 `language`）造成的假象，不是 runtime 缺陷。改用真实 `persona.json`（`profile: macos_chrome152`，`language: en-US`）后，`navigator.language` 与 `Intl.locale` 本就是 `en-US`。
  - 同理，先前记录的时区差异也不是缺陷：Obscura 的 `tzOffset=240`（America/New_York，来自 persona）与 Chrome 的 `-480`（主机 Asia/Shanghai）差异源于测试配置，而非实现。**做跨引擎差分时必须让两个引擎使用同一 persona 语义，否则会误报。**
- **本轮修复（真实缺陷）**：`navigator.languages` 在只给出单个 `language` 时被写成 `["en-US"]`，而同一份 persona 的 `Accept-Language` 却展开为 `en-US,en`。浏览器不会这样自相矛盾。已改为按 `accept_language` 相同规则展开出基础语言子标签，`navigator.languages` 现为 `["en-US","en"]`，与 Chrome 一致。改动在 `runtime/src/browser.rs` 的 `Persona::apply_defaults`，并同步修正原本断言 `["fr-CA"]` 的旧测试（该断言与同一测试中的 `fr-CA,fr;q=0.9` 自相矛盾）。独立 runtime release nextest **184/184**。新 runtime SHA-256 **`d59322863562715850122070ef52e6250619a33ec0b3992911f885951729d394`**。
- **仍确认存在、本轮未修的差异（均已用同一 persona 的 Chrome 对照实测，非推测）**：
  - `navigator.usb` / `navigator.serial` / `navigator.hid` / `navigator.bluetooth` / `navigator.mediaSession` 在 Obscura 为 `undefined`，Chrome 均为 `object`。
  - `navigator.connection.type` 在 Obscura 返回 `"wifi"`，Chrome 的 `NetworkInformation` 上不存在该属性。
  - `typeof SharedArrayBuffer` 在 Obscura 为 `"function"`，Chrome 在未 crossOriginIsolated 时为 `"undefined"`。
  - `Navigator.prototype` 自有属性数 21（Chrome 84）；`window` 自有属性数 611（Chrome 1234）。
  - `navigator` 原型链多一层（Obscura 4 层、Chrome 3 层），来自 `bootstrap.js` 中“在 Navigator.prototype 之上插一层薄原型”的写法。
  - `window.chrome` 键顺序为 `["app","csi","loadTimes"]`，Chrome 为 `["loadTimes","csi","app"]`。
  - `screen.isExtended` 缺失；`screen.colorDepth`/`pixelDepth` 24（Chrome 30）；`window.screenX/screenY/screenLeft/screenTop` 恒为 0（Chrome 为窗口偏移）。
  - `chrome.csi()` / `chrome.loadTimes()` 的字段形状与 Chrome 不同（Chrome 的 `onloadT`/`pageT` 语义与 Obscura 的实现不一致）。
  - 以上都可能直接进入反爬遥测，属于下一步的高价值、低风险修复面；但**没有任何一项已被证明是 403 的原因**。

### 本轮收尾状态（2026-09-18）

- 工作区改动共 4 个文件：`crates/obscura-js/js/bootstrap.js`、`crates/obscura-js/src/runtime.rs`、`runtime/src/browser.rs`、`docs/Southwest-handoff.md`。**尚未提交 Git**，保留原样。
- 门禁：根工作区 release render nextest **1759 passed / 4 skipped / 0 failed**；独立 runtime release nextest **184/184**。
- 生产 runtime 已按当前源码重建（render + stealth），SHA-256 **`d59322863562715850122070ef52e6250619a33ec0b3992911f885951729d394`**，保留副本 `/tmp/obscura-resume-20260918/runtime-webidl-brands/autopilot-browser-runtime`；源码哈希见 `/tmp/obscura-resume-20260918/final-source-manifest.txt`。
- 诊断专用产物（**不得合入**）：`/tmp/obscura-resume-20260918/runtime-cookie-seed`（含临时 cookie 导入补丁；对应源码已还原，`runtime/src/browser.rs` 里没有该补丁）。`runtime/src/browser.rs` 当前仅含语言列表修复。
- 业务状态：修复后仍为 **403050700**（`live-production-final` 三次）。**业务目标未达成。**
- 本轮未做远端部署、未改消费仓库 pin、未派发子代理。
- 本机保留中的夹具服务：18801/18802（cookie 往返）、18811（Akamai 信号探测）、18821（原型/brand）、18851（接口清单）、18861（构造器别名）；mini32 隧道 `ssh -N -L 127.0.0.1:17890:127.0.0.1:7890 mini32`。上述均为本机测试进程，可按需重启。

### primp 层重放与“传输指纹”假设的检验

- **有效重放（用户要求：必须用 obscura 自带的 primp 组件发请求）**：在 `crates/obscura-net/examples/` 放一个临时 example，直接用 `StealthHttpClient::with_policy_profile_persona` + `StealthProfile::MacChrome152`（即 runtime 生产使用的同一 primp/rustls 传输与同一配置文件），把 Chrome 成功请求捕获到的**原始 25 个头 + 1566 字节 cookie + 355 字节 body** 原样 POST 到 shopping 端点，走同一代理。
  - 结果：**403 / `403050700`**（与本轮 Obscura 失败完全相同的错误码与 23 字节正文）。
  - example 源码已从仓库删除（工作区保持只有 4 个已知改动文件），副本保留在 `/tmp/obscura-resume-20260918/primp-replay-example.rs`。它是一个有效的取证工具：`REPLAY_HEADERS`（JSON 头数组）/`REPLAY_BODY`/`REPLAY_URL`/`REPLAY_METHOD`/`REPLAY_PROXY`（空串表示直连）/`REPLAY_DUMP`。
- **重放这个仪器的固有局限（必须连带阅读，否则会误读上一条）**：Chrome 的 AT 遥测头分两类——`ee30zvqlwf-c`/`-d`/`-f`/`-z` 在整轮会话中**各只有一个值**（会话级、可复用）；`ee30zvqlwf-a`/`-b` **逐请求轮换**（`-a` 长度还在 2234/7624 之间变化）。因此重放时带上的是“可复用的会话令牌 + 一份 Chrome 已消费过的逐请求签名”。**单凭这次 403 无法区分“传输被识别”与“逐请求签名已失效”。**
- **对“纯 TLS 指纹”假设的直接反证（本轮最有价值的结果）**：在**同一条 primp 传输、同一条连接、同一个会话**里，Obscura 对 southwest.com 的其它接口拿到的是 200：
  - `200` `/api/air-booking/v1/air-booking/feature/uimetadata`（×2）
  - `200` `/api/content-delivery/v1/content-delivery/query/placements`（×4）
  - `200` `/api/security/v4/security/digital/jwks`（×1）
  - `403` `/api/air-booking/v1/air-booking/page/air/booking/shopping`（×6，唯一被拒的路径）
  - 即：primp 并没有被 Akamai 整体判定为机器人而一律拒绝，连同一个 air-booking 应用的其它接口都是 200。**“primp 的 TLS 指纹被识破所以 shopping 403”这一解释因此被显著削弱**；403 是 shopping 端点特有的。
  - 保留的诚实边界：Akamai 完全可以按路径下发不同策略，所以这不是“传输无关”的证明；但“传输指纹一票否决”已被现有数据排除。
- **primp 指纹已实测存档**（`primp-fingerprint-summary.json`、原始 echo `primp-peet.json`，经 `tls.peet.ws/api/all` 直连取得）：JA3 `2a26ba17332ad85e11a95a34b9159e45`，JA4 `t13d1517h2_8daaf6152771_cb7bf5808d99`，H2 Akamai fingerprint `1:65536;2:0;4:6291456;6:262144|15663105|0|m,a,s,p`（即 Chrome 的规范形态），首密码套件带 `TLS_GREASE (0x7A7A)`，19 个扩展。**未能取得同机 Chrome 的同端点 echo**：Chrome 的 “Allow JavaScript from Apple Events” 被关闭，AppleScript 无法读取页面；System Events 亦无辅助功能权限。若要逐字节对比 ClientHello，需要用户开启该菜单项，或改用 NetLog 的 SSL 详细级别（本次 Everything 抓取未含密码套件列表）。
- **下一步建议（按性价比排序）**：
  1. 用有效仪器把“会话令牌”和“逐请求签名”分开：让 Obscura 页面携带 Chrome 的会话级令牌（`-c/-d/-f`）而**自行生成新的逐请求签名**，看 shopping 是否转 200。这能直接判定是否是逐请求签名必须由真实 Chrome 产生。需要诊断用的 preload 注入通道（本会话的 runtime 未暴露）。
  2. 收紧重放窗口：全新抓一次 Chrome 成功请求后**立即**重放，减少令牌时效影响；但注意逐请求签名很可能一次性，这只是减少而非消除歧义。
  3. 继续按上面的差异清单补保真（`navigator.usb/serial/hid/bluetooth/mediaSession`、`connection.type`、`SharedArrayBuffer` 门控、原型层级、`window.chrome` 键序等）——它们未被证明是 403 原因，但都是真实缺陷且成本低。

### Chrome 153 传输档案 + 品牌顺序缺陷（用户指出路径级防护思路后）

- **思路更正（重要）**：上一版本文把“Obscura 在同一条 primp 连接上对 southwest.com 其它接口拿到 200、只有 shopping 403”当作“传输指纹假设被削弱”的证据。**这是非因果推论，已撤回。** 已实现的路径级防护完全可以在关键端点上单独启用更强的客户端校验，而 shopping 正是最有价值的那个端点。即使在 CS 架构里只对特定路径做 TLS/JA4 校验也是完全可行的。该观察仅保留一个合法结论：不是 IP/边缘级全量封锁。
- **本轮方向**：按用户判断，收紧到传输层，并把版本变量排掉——用户对照的 Chrome 是 **153**，而 Obscura 一直模拟 **152**。
- **权威基准（本轮关键资产）**：primp 是 vendored 在 `vendor/primp/`，它内部带有**从真实 Chrome 逐个版本抓取**的 `sec-ch-ua` golden（`src/imp/chrome/mod.rs`）与 JA4/JA4_ro 断言，且其测试全绿（`cargo test --lib imp::chrome` → 7 passed）。因此**真实 Chrome 的指纹基准可在离线、可复现的前提下取得**，不必依赖对 Chrome 的 CDP 控制。可用的真实捕获：

  | Chrome | 真实 sec-ch-ua（primp 捕获） | GREASE 版本 |
  | --- | --- | --- |
  | 146 | `"Not-A.Brand";v="24", "Chromium";v="146"` | 24（**仅 2 个品牌**） |
  | 147 | `"Google Chrome";v="147", "Not.A/Brand";v="8", "Chromium";v="147"` | 8 |
  | 148 | `"Chromium";v="148", "Google Chrome";v="148", "Not/A)Brand";v="99"` | 99 |
  | 149 | `"Google Chrome";v="149", "Chromium";v="149", "Not)A;Brand";v="24"` | 24 |
  | 150 | `"Not;A=Brand";v="8", "Chromium";v="150", "Google Chrome";v="150"` | 8 |
  | 151 | `"Not=A?Brand";v="99", "Google Chrome";v="151", "Chromium";v="151"` | 99 |
  | 152 | `"Chromium";v="152", "Not?A_Brand";v="24", "Google Chrome";v="152"` | 24 |
  | 153 | `"Google Chrome";v="153", "Not_A Brand";v="8", "Chromium";v="153"` | 8 |

- **顺带确认 primp 与真实 Chrome 一致**：primp 的 JA4 基准中 Chrome 152 与 Chrome 153 的 JA4 **完全相同**（`t13d1517h2_8daaf6152771_cb7bf5808d99`），与上一轮对 Obscura 实际 primp 客户端抓到的 live echo 一致；且 `ja4_ro`（密码套件/扩展/签名算法的**线上原始顺序**）也由 primp 逐版断言并与真实抓包对齐。因此 primp 在 ClientHello 层面确实是按真实 Chrome 构造的。
  - 仍需注意 primp 源码自己的提醒：JA4 只对扩展**类型**做哈希、不对扩展 body 做哈希，所以 JA4 相等**不保证逐字节一致**（`key_share`、ECH、ALPS 载荷等未被证明相同）。路径级校验若卡在这一层，上面这些等价性不足以保证通过。
  - 本机 Chrome 的同端点 echo **仍取不到**：Chrome 的 View → Developer → “Allow JavaScript from Apple Events” 处于关闭状态，AppleScript 无法读取页面；System Events 亦无辅助功能权限。要做逐字节 ClientHello 对比需用户开启该菜单项，或改用 NetLog 的 SSL 详细级别（Everything 级别的本次抓取未含 offered 密码套件列表，只有协商结果 `cipher_suite=4865`）。
- **本轮新增：`macos_chrome153` 传输档案**（端到端）
  - `StealthProfile::MacChrome153` 新增，UA `Chrome/153.0.0.0`、platform `("MacIntel","macOS","26.6.2")`，并新增 `full_version()`（153 → **`153.0.8010.50`**，取自真实 Chrome 实测；152 → `152.0.7977.83`）。`full_version()` 取代了原先硬编码在 `page.rs` / `runtime` 里的版本串。
  - 传输映射到 `primp::Impersonate::ChromeV153`；线上 `sec-ch-ua`/`sec-ch-ua-platform` 表补上 153 分支（153 相对 152 **同时改了 GREASE 令牌和品牌顺序**，不能只改版本号）。
  - persona 层接入：接受 `macos_chrome153`（校验、传输选择、`transport_profile` 上报、`apply_defaults`/`device_identity` 的 macos 判定都已覆盖两个 mac 档案）。
  - **端到端验证（本机线上头回显夹具，非脚本层快照）**：用 `macos_chrome153` persona 驱动 runtime 访问本机夹具，夹具记录到的收到头为 `sec-ch-ua: "Google Chrome";v="153", "Not_A Brand";v="8", "Chromium";v="153"`、UA `Chrome/153.0.0.0`、`sec-ch-ua-platform: "macOS"`、`accept-language: en-US,en;q=0.9`——与真实 Chrome 153 一致。152 档案回归正常。
- **本轮修复的真实缺陷：`userAgentData.brands` 品牌顺序**
  - `bootstrap.js` 的 `_uaBrands()` 按 Chromium 的做法从 Chrome 大版本推导 GREASE 品牌名、GREASE 版本与品牌顺序。逐版核对后：**GREASE 品牌名与版本号的公式 145–153 全部正确**；只有品牌**排列表** `_BRAND_PERMS` 的第 4、5 项写反，导致 **Chrome 147、148、153** 顺序错误（把表项 3、4 对调即可全部修正，149–152 不受影响）。
  - 红/绿证据：`/tmp/obscura-resume-20260918/brands-redgreen.mjs` 用 primp 的真实捕获逐版比对——**修前 3 处不匹配（147/148/153），修后 8 个版本全对**。
  - 修复前，`macos_chrome153` 的 JS 侧 `navigator.userAgentData.brands` 是 `[Chromium, Google Chrome, Not_A Brand]`，而真实 Chrome 153 是 `[Google Chrome, Not_A Brand, Chromium]`——与**线上 `sec-ch-ua` 顺序自相矛盾**（后者是被硬编码表写死的，本来是对的）。这类“线上头与 JS 侧不一致”本身就是强指纹。
  - 新增回归 `user_agent_data_brands_match_real_chrome_captures`，用 primp 的 8 个真实捕获（145、147–153）作为 golden。**Chrome 146 有意排除**：该版本真实只发 2 个品牌，当前推导器不建模这一情形（已记录为已知局限，未修）。
- **已重跑 Southwest live（`macos_chrome153` 档案，3 轮）**：仍然全部 **403 / `403050700`**（`/tmp/obscura-resume-20260918/live-153/`）。**所以单靠把模拟版本从 152 抬到 153 不足以恢复业务。** 这排除了“版本号不匹配是唯一/主要原因”。用户提出的路径级传输校验假设**并未被这一轮否定**——primp 的 ClientHello 在 JA4 层面与真实 Chrome 153 相同，但 JA4 不覆盖扩展 body，逐字节一致性仍未验证。另需注意：`sec-ch-ua` 线上值来自手写表、`brands` 来自 JS 推导，二者现在一致但**仍是两处独立实现**，后续若再加版本档案要同步两处（回归测试只覆盖 JS 侧；`obscura-net` 侧的表由 `all_profiles_send_consistent_identity_without_prefetch_defaults` 覆盖发送值，但该表本身没有与 primp 内部默认值做交叉断言）。
- 门禁：根工作区 release render nextest **1760 passed / 4 skipped**；独立 runtime **184/184**；`obscura-net`（stealth）**112/112**。新 runtime SHA-256 `e124adb153a6d639e793526372266cad6976c48a6ba26c39088178295c24c1cd`。
- 工作区改动文件：`crates/obscura-js/js/bootstrap.js`、`crates/obscura-js/src/runtime.rs`、`crates/obscura-net/src/stealth_client.rs`、`crates/obscura-net/src/stealth_transport.rs`、`crates/obscura-browser/src/page.rs`、`runtime/src/browser.rs`、`docs/Southwest-handoff.md`。

### Chrome ClientHello 实测对照（用户开启 Allow JavaScript from Apple Events 后）

- **工具解锁**：AppleScript 驱动 Chrome 执行 JS 的开关是**实例级内存状态**，Chrome 退出即失效。可用以下方式在自建诊断 profile 上稳定开启，**无需再打扰用户**：新建 `<profile>/Default/Preferences` 并在首次启动前写入 `{"browser":{"allow_javascript_apple_events":true}}`，再以该 profile 启动 Chrome，`execute <tab> javascript ...` 即可用。取页面内容时用 `btoa(unescape(encodeURIComponent(document.body.innerText)))` 传输，避免 AppleScript 的引号转义破坏 JSON。
- **同 IP 对照前提**：本机 Chrome 的系统代理为 `127.0.0.1:7890`，`tls.peet.ws` 回报出口 `23.249.17.98`，与本轮 mini32 隧道（`127.0.0.1:17890`）实测出口**相同**，因此下述比较是同一出口下的对照。
- **实测 1：真实 Chrome 会逐连接改变扩展顺序。** 两次独立连接（各自全新 profile → 全新 TLS 握手）：
  - JA4 两次**完全相同** `t13d1517h2_8daaf6152771_cb7bf5808d99`；非 GREASE 扩展**集合相同**。
  - 但**非 GREASE 扩展顺序不同**：样本 A `10,65037,43,65281,16,51764,35,23,17613,11,27,51,13,5,18,0,45`；样本 X `10,17613,51764,16,0,5,18,43,23,45,65037,27,13,11,35,65281,51`。
  - JA3 两次不同（`4e7077b9…` vs `7e584978…`）；GREASE 值本来就随机，也会影响 JA3，但**上面的顺序差异是直接读扩展列表得到的，与 GREASE 无关**。
  - 结论：**JA3/JA4_r 这类对顺序敏感的指纹在真实 Chrome 上是逐连接变化的；只有排序后的 JA4 才是稳定的。** 这一点此前没有实测过，属于新增事实。
- **实测 2：primp 的扩展顺序是固定不变的。** 同一 profile 连取 6 次独立连接：JA3 全部为 `2a26ba17…`，扩展顺序**逐字节相同**，且与上面两个 Chrome 样本的顺序**都不同**。
  - 因此存在一个可观测的行为差异：**Chrome 逐连接变化，primp 恒定不变。**
- **primp 侧线索**：`vendor/primp/src/imp/chrome/mod.rs` 将 `extension_order_seed` 钉为常量 `emulation::extension_order::CHROME`（在 primp-rustls 中为 `0x8daa`）。该常量自身的注释写的是它对标 `ja4=t13d1516h2_8daaf6152771_d8a2da3f94cd`——**扩展计数 16**；而当前 Chrome 152/153 与 primp 实际发出的是 `t13d1517h2_…cb7bf5808d99`，**扩展计数 17**。即该种子是按**更老的扩展集合**标定的。
- **尝试修复但失败（重要，避免后人重复踩坑）**：在 vendored primp 中把该行注释掉（种子取 `None` → primp-rustls 会退回逐连接随机），以及改成其它常量（`0x1234`），**连续采样得到的扩展顺序与 JA3 完全不变**。说明该种子常量**不是这条构建里决定发出顺序的有效控制**，机制尚未弄清。
  - 该实验已完全回滚：`vendor/primp/src/imp/chrome/mod.rs` 与 HEAD 一致（`git diff` 为空）；runtime 以干净源码重建，SHA-256 仍为 `e124adb153a6d639e793526372266cad6976c48a6ba26c39088178295c24c1cd`，与实验前一致。
- **对 403 的解释力（不要过度声称）**：这个差异是**传输层、真实存在、可机器检测**的，与用户提出的“关键路径单独校验 TLS”假设方向一致。但：
  1. **本轮没有取得 200**，因此**未证明**它是 403 的原因；
  2. 单次连接无法校验“顺序必须等于某个值”，因为真实 Chrome 本身在变。要利用这个差异，校验方需要**跨连接相关性**（例如“同一客户端所有握手顺序恒定不变”“顺序不落在 Chrome 的分布内”）。这类检查是否被启用，本机无从观测；
  3. 我**没能让 primp 的顺序变化**，所以连“变化后是否转 200”这个反事实实验都还没做成。
- **下一步（按可行性）**：
  1. 找到 primp 里真正决定扩展顺序的代码路径（种子改不动说明另有机制），使其像 Chrome 一样逐连接变化；然后重跑 shopping 作反事实检验。
  2. 若确实需要逐连接随机：primp-rustls 在**非**模拟路径下已经用逐连接随机种子，可在 vendored primp 里停止设置 `browser_emulation` 的顺序字段并验证。
  3. 采样更多 Chrome 连接以刻画其顺序分布，判断 primp 的固定顺序是否落在该分布内。

### JA3 缺陷：定位、修复与验证（用户要求继续修 JA3 差异）

- **随机化粒度判定（先排除混淆变量）**：此前两个 Chrome 样本来自**不同 profile**，无法区分“逐连接随机”与“逐 profile 随机”。用**同一实例、同一 profile、两个不同源站**（必然不同 TLS 连接）重测：
  - `tls.peet.ws` → JA3 `c60481f682f12c7327a9489fb1acd38f`，非 GREASE 顺序 `35-51764-65037-16-17613-45-23-43-65281-27-13-0-11-51-18-10-5`
  - `tls.browserleaks.com` → JA3 `6b80fa5f8e213ef772761a7efeb3b9d9`，顺序 `51-17613-18-65281-11-5-13-27-23-45-43-10-0-35-51764-16-65037`
  - JA4 两者**完全相同** `t13d1517h2_8daaf6152771_cb7bf5808d99`；第 4 个独立连接给出第 3 种顺序。
  - 结论：**真实 Chrome 对顺序不敏感的扩展做逐连接全排列**，JA4/JA4_r 因排序而稳定，只有 JA3（及没有的 ja3n 之外的顺序敏感量）会变。**逐 profile 的假设被排除。**
- **根因定位**：`emulator_extension_order()`（primp-rustls `src/client/hs.rs`）返回一份注释写着 “taken verbatim from real captures” 的**逐版本硬编码顺序**，并被赋给 `contiguous_extensions`，从而**永久固定**发出顺序。同一个函数里 `choose_extension_order_seed` 的注释却写着“随机 per-connection seed 才 match 真实 Chrome 的逐连接变化”——**primp-rustls 自身前后矛盾**，钉死的那份胜出。
  - 顺带修正上一轮的错误结论：我曾试图改 vendored primp 里的 `extension_order_seed` 常量，无效。**种子不是有效控制**；真正的开关是这份 `emulator_extension_order` 表。上一轮已记录的“机制尚未弄清”至此弄清。
  - 也顺带说明：primp 自带的 JA4/JA4_r golden 测试**结构上无法发现这个问题**，因为 JA4 会先把扩展排序。
- **修复（vendored primp-rustls + 一处补丁）**：
  - 新增 `vendor/primp-rustls`（0.23.43）并在**根与 `runtime/` 两个工作区**的 `[patch.crates-io]` 中都接上（两处都要，因为 `runtime/` 是独立 workspace）。
  - `src/client/hs.rs`：新增 `permute_order_insensitive_extensions()`，在赋值给 `contiguous_extensions` **之前**，用**每连接全新随机 seed** 对顺序的中段做排列；**首尾槽位不动**（GREASE 占位符在其后按位置替换，动了就破坏首尾 GREASE），`pre_shared_key` / `encrypted_client_hello_outer_extensions` 不参与（RFC 8446 要求 PSK 必须在最后）。排列复用该 crate 自己的 `low_quality_integer_hash`（因此把它从私有放宽为 `pub(crate)`），与它既有洗牌路径同源。
  - **修复过程中我自己犯并修掉的一个 bug（记下来以防复现）**：第一版把“排序后的下标表”同时当作**读取顺序**和**写入槽位**，于是 `order[i] = order[i]`，是**恒等变换**——表现为“补丁编进去了、探针也打到了、顺序却纹丝不动”。正确写法需要**保留下标升序的槽位表**，用排序后的下标只决定读取顺序。定位手段：先打探针确认代码可达，再用“把中段直接倒序”的对照实验确认 `contiguous_extensions` 确实是最终发出顺序（倒序后 JA3 变为 `a321a49d…`）。
  - `emulator_extension_order` 本身**未改动**，所以它那些逐版本 golden 测试仍然有效并全部通过。
- **修复后验证**：连续 5 次连接到 `tls.peet.ws`：
  - **5 个不同的 JA3**（修复前：连续 6 次完全相同）；
  - JA4 恒为 `t13d1517h2_8daaf6152771_cb7bf5808d99`，JA4_r 恒定；
  - 扩展总数 19（17 非 GREASE + 2 GREASE），**GREASE 始终在首尾**，非 GREASE 集合不变。
  - 即：**行为已与真实 Chrome 对齐**（逐连接变化、排序后指纹稳定）。
- **业务结果：仍然 403。** 用该 runtime（`macos_chrome153` 档案）重跑 Southwest live，**全部 403 / `403050700`**（`/tmp/obscura-resume-20260918/live-ja3vary/`）。
  - **所以 JA3 恒定不是 403 的原因。** 这是一个明确的排除项：用户提出的“路径级传输校验”假设**方向仍未被否定**，但要卡也不会是卡在这个层面（至少不是卡在“顺序恒定”这一点上）。
- **仍未比对的部分（不要过度声称）**：JA4 只哈希扩展**类型**、不哈希扩展 **body**。`key_share`、`encrypted_client_hello`、`application_settings`（ALPS）、built-in-verification 这些扩展的**载荷字节**从未与真实 Chrome 抓包逐字节比对过。若路径级校验卡在 body，上面的等价性不足以覆盖。
- 门禁：根工作区 release render nextest **1760 passed / 4 skipped**；独立 runtime **184/184**。临时用于取指纹的 example（`crates/obscura-net/examples/primp-replay.rs`）已按工作区卫生**删除**，副本留在 `/tmp/obscura-resume-20260918/primp-replay-example.rs`（注意：它需要 `--features stealth` 才能编译，留在仓库里会拖垮不带该 feature 的整仓构建）。
- 工作区改动文件（本轮新增）：`vendor/primp-rustls/**`（新增）、`Cargo.toml`、`runtime/Cargo.toml`、`vendor/primp/OBSCURA_PATCHES.md`、`vendor/primp-rustls/OBSCURA_PATCHES.md`、`docs/Southwest-handoff.md`。

### 403 的重新定性 + DNT 缺陷（2026-09-18 晚）

- **交接文档的一处事实错误必须更正**：`terms-of-service: Unauthorized access, display, or use of Southwest's Company Information...` **不是“反爬拦截标记”**。实测它出现在 **HTTP 200** 的响应上（Chrome 会话里带该头的响应 `:status: 200`）。因此**不能用它来判断是否被拦**，此前把它当标记的推理作废。
- **Chrome 会话基线**（`chrome-netlog-fresh.json` 全量统计）：`:status` 分布 = `200×249, 302×24, 204×6, 301×5, 304×5, 201×1, 307×2, 429×1`，**403 出现 0 次**。即 Chrome 在整段会话里从未被拦。
- **决定性的对照实验：全新 Chrome 档案 + 同一条深链 → shopping 200**（票价正常渲染）。用 `performance.getEntriesByType('resource')` 的 `responseStatus` 读到的真实状态码，不是页面自述。**这排除了“缺少浏览历史/必须先走完整流程”这一整类假设**——空档案直接深链也能拿到 200。所以差异**确实在客户端**。
- **同一页面双引擎差分夹具（本轮新增工具，建议复用）**：
  - `fixtures/xhr-page.py`：本地起一个页面，页面内 `fetch('/probe', {method:'POST',...})`，服务端把**收到的原始头**逐条记录下来。Obscura 与 Chrome **各加载一次同一个 URL**，即可逐头对比“同源 POST 线上到底发了什么”。
  - `fixtures/probe-dnt.py`：同上，但页面回传一批 JS 侧信号（`navigator.*`、`Intl` 时区、`hardwareConcurrency` 等）。
  - 这个夹具解决了此前只能看到“runtime 日志里记的头”的问题。
- **一处推理更正**：此前用 runtime 的 `kind=request` 日志对比线上头，得到 `origin`/`referer`/`sec-fetch-*`/`user-agent`/`sec-ch-ua*` 为 “CHROME ONLY”。**这是日志假象**——runtime 的请求日志只记显式设置的头，不含传输层补的默认头。夹具实测证明 Obscura **确实发送**这些头。该结论作废。
- **真实缺陷（已修）：DNT 双层不一致。** 夹具对照（同一 URL、同一个同源 POST）：

  | | 真实 Chrome 153 (macOS) | Obscura（修复前） |
  | --- | --- | --- |
  | `navigator.doNotTrack` | `null` | **`"1"`** |
  | `DNT` 请求头 | **不发送** | **`dnt: 1`** |

  - 根因：`runtime/src/browser.rs` 的 Persona 默认值里有 `if macos && self.do_not_track.is_none() { self.do_not_track = Some("1".into()); }`——**给 macOS 档案凭空捏造了一个 DNT=1 的隐私表态**。真实 Chrome 不发送 DNT、且 `navigator.doNotTrack === null`。而且它**两层同时暴露且互相一致**，看起来像刻意配置过的客户端，是典型的自动化特征。
  - 修复：删掉该捏造默认值（persona 仍可显式指定 `do_not_track`）；更新对应断言为 `None`。**修复后实测两层均与 Chrome 一致**（`doNotTrack=null`、无 `DNT` 头）。
  - 保留诚实的边界：**单独修 DNT 后 shopping 仍 403**，所以它不是唯一原因，但它是真实且已消除的差异。
- **新发现的结构性差异：Obscura 从不调用 Airship 的 API。** 全新 Chrome 深链会调用**另一个源**：

  ```
  201  https://aswpapius.com/api/web-channels
  200  https://aswpapius.com/api/contacts/identify/v2_web
  200  https://aswpapius.com/api/web-events
  200  https://aswpapius.com/api/remote-data/app/QpqQ9YfNQWut6CYpm0sQbw/web/
  ```

  - Obscura 的 `allowed_origins`（16 项）里有 `https://aswpsdkus.com`（Airship **SDK** 脚本，确实加载了 2 次），但**没有 `https://aswpapius.com`（Airship API）**。
  - 把 `aswpapius.com` 加进白名单（替换掉 Adobe tracker）后重跑：**`aswpapius` 请求数仍为 0**——不是被白名单拦的，而是 **Airship SDK 根本没有发起这些调用**。即 SDK 脚本加载了但**没有完成初始化/上报**。这指向 Airship SDK 初始化所依赖的某个能力在 Obscura 里缺失（下一步要查的方向）。
  - 注意：**未证明**这是 403 的原因；仅记录为“Chrome 有、Obscura 无”的行为差异。
- **令牌结构对比（说明令牌形状不是主因）**：把 Chrome 与 Obscura 的 AT 头（`ee30zvqlwf-*`）base64 解码后对比：
  - `-c` 45B、`-d` 73B、`-f` 64B，**三者解码长度两边完全一致**；
  - `-d` 的**前 43 字节逐字节相同**，`00000000` 与 `57f022af03` 也相同；
  - `-f` 的**字节 4–9 与末尾 20 字节完全相同**；
  - `x-api-key`/`x-app-id` 等头的解码长度也一致。
  - 即：**令牌的静态结构两边对得上**，差异只在少量疑似 nonce/时间戳字段。所以“Obscura 生成的令牌形状不对”**不是**主因；若令牌有责，问题在**内容里的某个环境信号**。
- **`ee30zvqlwf-*` 头在页面 JS 里搜不到**：在 booking 页与 select-depart 页对**全部同源脚本 + 内联脚本**做同步 XHR 全文搜索，`/ee30/i` 与 `/zvqlwf/i` **零命中**；`window` 上也没有相关全局。而 Obscura 自己也会发出这 6 个头 → 说明它们**不是页面 JS 里明文构造的**，而是由某个**运行时拦截器/或构建期拼接**产生。`app.js` 里能搜到的是另一处遥测 POST（`x-api-key`/`x-app-id`/`x-channel-id`/`x-user-experience-id`），**不含** AT 头。
- **下一步（按性价比）**：
  1. 查清 **Airship SDK 为什么在 Obscura 里不初始化**（最像“缺能力”的地方）。runtime 目前**没有**页面求值入口（`automation.py` 只有 goto/screenshot/事件），要验证 SDK 内部状态需要加一个诊断用的 eval 通道。
  2. 用双引擎差分夹具**系统化铺开**：把 `navigator.*`、`Intl`、存储、`crypto`、`WebGL`、字体等信号一次跑完，逐项 diff，而不是一次猜一个。
  3. 找 AT 头的真正产生点（拦截器/构建期）；可考虑在 Obscura 里 hook `fetch`/`XHR.setRequestHeader` 记录调用栈（同样需要 eval 通道或 preload 注入）。
- 门禁：根工作区 release render nextest **1760 passed / 4 skipped**。新 runtime SHA-256 `8b9ef53aae87ed3370008faa638087322f65bdcc09d49821f00aadb3d16bc86f`。

### 诊断通道 + AT 头来源查清（2026-09-18 深夜）

#### 建成的诊断通道（重要工具，可复用）
- **问题**：runtime 没有页面求值入口（`automation.py` 只有 goto/screenshot/事件），要看页面内部状态无从下手；而且 SDK 只用受限环境启动 runtime，**环境变量传不进去**。
- **做法**：给 runtime 加一个**仅诊断、用完即撤**的钩子——若工作区里存在 `diag-preload.js`，就把它作为额外 preload 注入（`page.add_preload_script`）。注入脚本在 `pup` 前先跑，能 hook `fetch`/`XHR`/`Request`/`Worker`/`URL.createObjectURL`。
  - **本轮结束时已完全撤销**：`grep -c "diag-preload\|DIAGNOSTIC" runtime/src/browser.rs` → 0；runtime 以干净源码重建，SHA-256 仍是 `8b9ef53a…`。

- **数据怎么取回来（踩过的坑，务必记下）**：
  - ❌ 让页面 POST 到本机夹具：被 `MIXED_PRIVATE_ORIGINS` 拒绝——**私网源不能与公网源混在白名单里**。
  - ❌ Service Worker / Worker：页面上**没有** Service Worker；有两个 blob Worker，但 **hook `URL.createObjectURL` 拿到源码后检索，两个 blob 里都没有 `ee30`**；**把 `window.Worker` 直接改成抛错后，AT 头照样出现在 6 个请求上** → Worker 不是来源。
  - ✅ **可行方案**：注入脚本把报告 base64 分块写进 `document.documentElement` 下的 `#obscura-diag-N` 元素，调用方用现成的 `read_text` 读回（`read_text` 走 `dom.text_content`，**不要求元素可见**；每块 < 16384 字符）。已把读取逻辑加进 `/tmp/obscura-resume-20260918/run-live.py`（配 `--dwell` 在最后一页多停留再读，因为每次 goto 都是新文档，报告只覆盖当前文档）。
  - ⚠️ **对照纪律**：空插桩（no-op）与真插桩各跑一次比对过：无插桩 165 请求 / 有插桩 144 请求，shopping 都是 403×3。插桩有轻微影响但不改变结论。

#### 结论 A：AT 头（`ee30zvqlwf-*`）的来源已查清
- **我最初的插桩有个缺口，导致差点得出错误结论**：`fetch` hook 只看 `init.headers`；而页面是 `fetch(requestObject)`，**头挂在 Request 对象上、`init` 为空**，所以一个都没抓到（180 个请求里 0 个 `ee30`）。补上「同时检查 `input.headers`」后**立刻全部抓到**。
  - **教训**：hook 覆盖不完整时得到的「没有」是**假阴性**，不能当作证据。
- **捕获到的真实调用栈**（每一步都是可核对的文件+行列号）：
  ```
  window.fetch                          ← 注入的 hook
   <- https://www.southwest.com/resources/4e88446acf548ff55d184615dd91bee22130011c85bd8:24:413
   <- Df.ba   (同文件 :262:228)
   <- Object.apply (同文件 :24:400)
   <- Array.Rv (https://www.southwest.com/assets/app/scripts/swa-common.js:1:126829)
   <- RH       (swa-common.js:1:151770)
   <- <anonymous> (wasm://wasm/1e4d5d2e:1:2162)
  ```
- 即：这 6 个头由 **`/resources/<40位hex>` 脚本**（Akamai 的 sensor 形态）生成，经 `swa-common.js` 与一个 **WASM 模块**驱动，最后通过 `fetch(Request)` 挂到请求上。
- **该 sensor 脚本已抓取**：`/tmp/obscura-resume-20260918/sensor.js`（377693B，HTTP 200）。**混淆过**，`ee30`/`zvqlwf` 均不以明文出现 → 头名前缀应来自**服务端下发的配置**，不是脚本常量。
- **对 403 的意义**：Obscura **自己也能产生这 6 个头**（值结构、解码长度均与 Chrome 对齐，见上一节）→ sensor 在 Obscura 里是**跑起来了**的。所以问题不在「有没有令牌」，而在**令牌里上报的环境信号**或**令牌与请求的绑定**。这把矛头明确指回 JS 环境保真度。

#### 结论 B：Airship SDK —— 加载成功但从不初始化（任务 1 未结案）
- `https://aswpsdkus.com/notify/v2/ua-sdk.min.js`（`type=(none)`、`async`）在 Obscura 里 **HTTP 200 正常拉取**。
- 但：**零全局符号**（`window` 上搜 `airship|aswp|urban|UA|_ua|channelid` 全空）、**零 JS 错误**、**零未处理 rejection**、**零次调用 `aswpapius.com`**（Chrome 会调 4 次：`web-channels` 201、`contacts/identify/v2_web`、`web-events`、`remote-data/app/<key>/web/`）。
- 已把该 SDK 解压后检索（gzip → 280678B，`/tmp/obscura-resume-20260918/ua-sdk.dec.js`）：
  - 引用了 `PushManager` / `serviceWorker` / `Notification` / `requestPermission` / `indexedDB` / `localStorage` / `subtle` / `BroadcastChannel` / `Worker` / `postMessage`；
  - 内部有两个守卫：**`FeatureDisabledError`** 与 **`DataCollectionDisabledError`**。
  - → **下一假设：SDK 因「特性/数据收集被禁用」而早退**（最可能是同意状态，SW 用 OneTrust；也可能是 `PushManager` 等能力缺失）。
  - ⚠️ Obscura 的能力探测里 **`PushManager` 为 `undefined`**（真实 Chrome 有）；其余探测项（`crypto.subtle`、`indexedDB`、`localStorage`、`serviceWorker`、`BroadcastChannel`、`OffscreenCanvas` 等）都正常。
- **未完成**：尚未证明是哪个门控导致早退（需要读 SDK 里那两处 throw 的触发条件，或对比 Chrome 侧同一脚本的全局符号/调用）。
- **注意**：Airship 不初始化**是否**导致 403 **未经验证**；仅记录为「Chrome 有、Obscura 无」的行为差异。

#### 本轮改动
- 生产代码仅一处：**DNT 捏造默认值已删除**（见上一节），断言同步更新。
- 其余全部为 `/tmp` 下的诊断脚本与抓取物；runtime 的临时诊断钩子已撤销，runtime 二进制哈希不变（`8b9ef53a…`）。
- 夹具：`fixtures/xhr-page.py`（线上头对照）、`fixtures/probe-dnt.py`（JS 侧信号）、`diag-preload.js`（页面内插桩）、`run-diag.py` / `run-live.py --dwell`（读回诊断）。

### Airship 两个 throw 的触发条件（2026-09-18 深夜，第二轮）

源码：`/tmp/obscura-resume-20260918/ua-sdk.dec.js`（`ua-sdk.min.js` 解压后 280678B）。

#### 两个 throw 的确切位置与条件

```js
// 1) 主路径：trackInteraction 的第一件事就是查 feature flag，然后再上报
trackInteraction(t){
  return l(this,void 0,void 0,(function*(){
    const {analytics:e, feature_flags:n} = yield this._featureQuerier.isFeatureEnabled("analytics","feature_flags");
    if(!n) throw new i.FeatureDisabledError;         // ← feature_flags 关
    if(!e) throw new i.DataCollectionDisabledError;  // ← analytics 关
    ...
```

```js
// 2) 联系人路径
_assertContactsEnabled(){
  return Kt(this,void 0,void 0,(function*(){
    if(!(yield this._featureQuerier.areFeaturesEnabled("contacts")))
      throw new e.FeatureDisabledError("contacts feature is disabled")
  }))
}
```

- **两个 throw 都由 `_featureQuerier` 的 feature flag 决定**，而 querier 的输入是 `configurationEnabledFeatures` 与 **`disableAnalytics`**（见 `Uo`：`new Wr(n, s.forType("feature:enablement"), {configurationEnabledFeatures:e.enabledFeatures, disableAnalytics:e.disableAnalytics})`）；remote-data 管理器则带 `initialBaseUrl: e.apiUrl`。这些都是**页面下发**的配置。
- **throw 会被 SDK 自己吞掉**（关键）：
  ```js
  catch(t){ if (t instanceof e.FeatureDisabledError) return; throw t }
  ```
  → **这正是「零 JS 错误、零 unhandledrejection、零 API 调用」的成因**，不是页面没报错，而是 SDK 主动静默 return。**以后不要再把「没有错误」当作「没执行」的证据。**

#### 启动门的定义（顺带查清）

```js
const Di = ["indexedDB","localStorage","fetch","crypto"];
function Ti(){ return Di.every(t => t in window) }        // 否则 Error("Browser is not supported")
function Fi(){ return !!Ti() && "PushManager" in window }
```

`ua-sdk.min.js` 末尾的引导要求**同时**满足：

```js
const t = document.getElementById("_uasdk"), e = t.rel;
t && e && window[e] && typeof window[e]._async_setup === "function"
  && window[e]._async_setup(function(t={}){
       go(Object.assign({VERSION:$o, apiUrl:Lo}, t));   // Lo = "https://aswpapius.com"
       if (!Ti()) { ...Error("Browser is not supported")... }
       return yo()
     })
```

任一条件不成立 → 整条 `&&` 短路 → **静默不启动，不报错**。

#### 实测：Obscura 的启动门全部通过（推翻了我上一条的猜测）

页内探针（preload 注入）实测：

| 检查 | Obscura |
| --- | --- |
| `#_uasdk` 标签存在 | **True** |
| 标签 `rel` | **`"UA"`** |
| `window.UA` | object |
| `window.UA._async_setup` | **function** |
| `Ti()`：indexedDB/localStorage/fetch/crypto 全在 | **True** |
| `Fi()`：`"PushManager" in window` | **False** |

而且**握手是走完的**：SDK 调用了页面的 `_async_setup`（8448ms），页面**回调了 SDK 的 bootstrap**（8455ms），回调参数里 **`appKey = "QpqQ9YfNQWut6CYpm0sQbw"`**——与 Chrome 调 remote-data 时用的 key **完全一致**。

**所以 SDK 不是「没启动」，也不是「拿不到 appKey」。它是在配置完成之后静默停下的。**

#### 与 Chrome 的对照（同一深链、同一探针口径）

| | Chrome 153 | Obscura |
| --- | --- | --- |
| `#_uasdk` 标签 | True | True |
| 标签 `rel` | **无** | **`"UA"`** |
| aswp 资源数 | **8** | **0** |
| `"PushManager" in window` | **True (function)** | **False (undefined)** |

Chrome 的 8 个资源：`web-channels` 201、`contacts/identify/v2_web` 200、`remote-data/app/QpqQ9YfNQWut6CYpm0sQbw/web/` 200、`web-events` 200、`warp9/` 200，外加 **`ua-436.min.js`、`ua-482.min.js` 两个 SDK 子 chunk**。
→ **Obscura 连子 chunk 都没去加载就停了。**

#### 假设「PushManager 缺失导致早退」—— 已用对照实验否定

用 preload 在页面脚本之前定义了一个最小 `window.PushManager`（零生产改动），实测：

```
injected      : defined window.PushManager
pushManagerIn : True   typeof: function     ← Fi() 现在应为 true
aswpReqs      : 0                            ← 但 SDK 依然不调 API
errors: 0 rejects: 0
```

**结论：`PushManager` 缺失不是 SDK 停止的原因。** 不要把补 `PushManager` 当作此问题的修复提交（它仍是一个真实的保真差异，但与此无关）。

#### 本轮我自己制造的两个错误（务必避免重犯）

1. **探针假象**：我一度报告回调参数里 `apiUrl: ""`。实际上 `cut(undefined)` 也返回 `""`，**分不清「属性缺失」与「空串」**。不作为发现。
2. **探针自递归**：包装 `_async_setup` 时写了 `args[0] = wrapper`（`args` 即 `arguments`），导致 `Maximum call stack size exceeded`，两个 rejection 全是栈里 `wrapper (<preload>)`。**跟页面无关**，是本探针的 bug。

#### 仍未结案

**SDK 在握手完成后静默停止的真正原因未知。** 已知边界：
- 不是启动门（全部通过）；
- 不是 appKey（正确）；
- 不是 `PushManager`（已用实验否定）；
- 不是「页面没提供配置」（页面回调了）；
- 且失败是**设计上静默**的（`FeatureDisabledError` 被吞），所以「看不到错误」不构成证据。

**下一步建议**：给 `trackInteraction` 路径上的 `_featureQuerier.isFeatureEnabled` / `_report` 打点，直接看 feature flag 的取值与 querier 是否发起过 remote-data 读取；或对照 Chrome 侧同一位置的取值。这需要一个能在页面上跑任意代码的通道——本轮用的 preload 注入通道（临时钩子）已验证可用，可随时重开。

### 打通 Airship：IndexedDB 请求从不派发事件（2026-09-18 深夜，第三轮）

**结果：aswp 资源数 0 → 6（Chrome 为 8），且状态码与 Chrome 逐项一致。**

#### 根因：`indexedDB.open()` 的请求永远不落地

Airship SDK 的引导函数 `yo()` 第一件事就是 `indexedDB.open("<prefix>:db", 1)`，然后 `await` 它。而 Obscura 的实现：

```js
open(name, version) {
  const req = new IDBOpenDBRequest();
  Promise.resolve().then(() => {
    req.result = new IDBDatabase(name, version || 1);
    req.readyState = 'done';
    if (typeof req.onsuccess === 'function') req.onsuccess({...});   // 只调属性
  });
  return req;
}
```

- **从不 `dispatchEvent`**：`IDBRequest extends EventTarget`，但用 `addEventListener("success", ...)` 的调用方（Airship SDK、以及现代库的普遍写法）**一个通知都收不到**。
- **`upgradeneeded` 从不派发**：于是 `createObjectStore` 永远没机会执行。
- 实测最小复现：`open(name,1)`、`open(name)` **双双超时**，没有 success / error / upgradeneeded。

`yo()` 卡在这个 await 上 → **整个 SDK 静默停摆**（零错误、零 rejection、零请求）。这与之前记录的「SDK 设计上吞掉 FeatureDisabledError」叠加，导致**任何"没有报错"的观察都不可作为"没执行"的证据**。

#### 修复（`crates/obscura-js/js/bootstrap.js`）

1. **请求必须派发事件**：新增 `_idbDispatch(req, type, handlerProp, event)`，先 `dispatchEvent` 再调 `onsuccess/onerror/onupgradeneeded` 属性；`_idbRequest()` 与 `open()` 全部改走它。
2. **`open()` 先发 `upgradeneeded` 再发 `success`**：新建库或版本提升时，先用可用的 `IDBDatabase` 派发 `IDBVersionChangeEvent{oldVersion,newVersion}`，让处理函数能 `createObjectStore`，之后再提交版本并派发 `success`。版本非正整数 → error；版本低于现有 → error。
3. **真实的按名注册表**：`_IDB_DATABASES: Map<name, {version, stores}>`。对象存储跨连接持久，`count()` 反映真实写入。
4. **`createObjectStore(name, {keyPath, autoIncrement})`** 真正登记到库上（并保留 keyPath）；重复创建抛 `ConstraintError`。
5. **`IDBTransaction` 完成事件**：真实 `dispatchEvent('complete')`，且只在**本事务内所有请求都落地之后**才完成（`_begin/_finish/_maybeComplete`），避免 `complete` 抢在它该跟随的读之前。
6. **`IDBObjectStore` keyPath 感知的键**：`put(value)` 时从 `value[keyPath]` 取键（支持点路径）、`autoIncrement` 递增，取不到键抛 `DataError`。
7. **`deleteDatabase` / `databases()`** 走注册表。
8. **`objectStoreNames` 改为原型上的 getter**，返回**活的 array-like**（含数字下标 + `length` + `item` + `contains`）。

**红/绿证据**（页内探针，`/tmp/obscura-resume-20260918/probe-idb.js`）：

| | 修复前 | 修复后 |
| --- | --- | --- |
| `open(name,1)` | TIMEOUT | `upgrade: true`（oldVersion 0 → 1） |
| `createObjectStore('blob',{keyPath:'type'})` | 从不发生 | `createdStore: "blob"`，`storeKeyPath: "type"` |
| `success` / `dbVersion` | 无 | `true` / `1` |
| `count()` | 无 | `0` |
| `put`+`get` 往返 | 无 | `readBack: 1` |
| `tx.oncomplete` | 无 | `true` |

#### 我自己引入并修掉的第二个 bug（记下来）

第一版里 `objectStoreNames` 是**对象字面量**：

```js
this.objectStoreNames = {
  contains: (n) => ...,
  get length() { return this._registry ? this._registry.stores.size : 0; },  // ← this 是字面量
  item: (i) => ...,
};
```

`get length()` 里的 `this` 指向**那个字面量**而不是 `IDBDatabase`，于是 `_registry` 恒为 `undefined` → **`length` 永远返回 0**。而 `contains`/`item` 是箭头函数、捕获的是构造函数里的 `this`，所以它们是对的——**症状是 `item(0)` 返回 "blob" 但 `length` 是 0**，非常容易被忽略。改成原型 getter 后一并解决。

#### 端到端结果（`macos_chrome153` + 含 `aswpapius.com` 的 origins）

| # | 资源 | Obscura | Chrome |
| --- | --- | --- | --- |
| 1 | `aswpsdkus.com/notify/v2/ua-sdk.min.js` | 200 | ✓ |
| 2 | `aswpapius.com/api/web-channels` | **201** | 201 |
| 3 | `aswpsdkus.com/notify/v2/ua-436.min.js` | 200 | ✓ |
| 4 | `aswpsdkus.com/notify/v2/ua-482.min.js` | 200 | ✓ |
| 5 | `aswpapius.com/api/contacts/identify/v2_web` | 200 | 200 |
| 6 | `aswpapius.com/api/remote-data/app/<appKey>/web/` | 200 | 200 |
| 7 | `aswpapius.com/api/web-events` | **缺** | 200 |
| 8 | `aswpapius.com/warp9/` | **缺** | 200 |

- **运营侧发现（必须记住）**：`aswpapius.com` **必须**在 `allowed_origins` 里，否则 `OriginGuard` 会把 Airship 的 API 全部拦掉。`origins-16.json` 原本只有 `aswpsdkus.com`（SDK 脚本域），**没有 API 域**。
- SDK 现在完整初始化：`sdk_enabled: true`、`channel.channel_id`、`contact.id`、`install_date`、`last_activity` 都已写入，`objectStoreNames`/事务/读写全部正常。**这同时修好了页面上其它依赖 IndexedDB 的库**（例如 Adobe 的 `DSE_CookieStore`）。

#### 剩余 2 项（未解决，已定位到具体位置）

缺的两项都是 **analytics 事件批次**（`web-events` = venus，`warp9/` = warp9），由 SDK 的 `acceptWarp9Events`/`acceptVenusEvents` 发出，两者都先查 `areFeaturesEnabled("analytics")`，不通过就**静默丢弃**。

已确认的事实：
- **事件确实被接受了**：`ua:default@9` 库里 `queued_events` 有 4 条记录，含 `{"type":"venus","event":{"event_type":"session",...}}` 和 `{"type":"warp9","event":{"device":{"channelId":"4317cbd1-...","contactId":"63c5d026-..."}}}`。
- 但 `tombstone: 0, sendsAttempted: 0, lastAttemptedAtMs: 0` → **从未尝试发送**。
- **SDK 无 `setInterval`**；flush 是**防抖定时器**：`_queueFlush()` → `window.setTimeout(() => this.flush(), flushCadenceMs=1500)`；`flush()` 转给 `this._rpc.flush()`。事件入队后就该在 1.5s 内 flush。
- **`queued_events` 的操作在页面侧计数为 0** → 这些写入来自 **Worker**。即 **Airship 核心跑在 Worker 里**（页面侧记录到 2 个 blob Worker，`worker.postMessage` 共 11 次）。
- **Worker 内部环境是健康的**：`indexedDB` 在且 `open()` 成功（`upgrade`+`success`）、`fetch`/`crypto.subtle`/`BroadcastChannel`/`OffscreenCanvas` 都在，`localStorage` 缺席（worker 里本就该没有）。
- 伪造 `visibilitychange → hidden`（`document.visibilityState`/`hidden` 覆盖成功、事件已派发）**并不会触发 flush**。
- 150 秒长驻留也拿不到这 2 项。

**结论：阻塞点在 Worker 内部 outbox 的发送路径上，尚未定位。** 不是页面侧可见性问题，不是白名单问题（其余 aswp 都通），不是环境能力缺失（worker 探测全通过）。

#### 临时诊断通道（已撤销，需要时可重开）

runtime 原本没有页面求值入口，环境变量又传不进去（SDK 用受限环境启动 runtime）。本轮用的办法：在 `runtime/src/browser.rs` 的新页面路径里插一小段，**若工作区存在 `diag-preload.js` 就额外 preload 注入**：

```rust
if let Ok(diagnostic) = std::fs::read_to_string(self.workspace.join("diag-preload.js")) {
    page.add_preload_script(&diagnostic);
}
```

注入脚本把报告 base64 分块写进 `document.documentElement` 的 `#obscura-diag-N`，用现成的 `read_text` 读回（走 `dom.text_content`，不要求可见）。**本轮结束已完全撤销**，`grep -c "diag-preload" runtime/src/browser.rs` → 0。

#### 门禁与产物
- 根工作区 release render nextest **1760 passed / 4 skipped**；`obscura-js` 单包 **553/553**；独立 runtime **184/184**。
- 生产代码改动：`crates/obscura-js/js/bootstrap.js`（IndexedDB shim）、`runtime/src/browser.rs`（DNT，本轮无新增）。
- runtime SHA-256 `5edc98aeae29bd5b21b915b39ff4b7b1d1387e3d563aa09d246aa17b3e938fc5`。
- **业务结果：shopping 仍 403**（Airship 打通与否和 shopping 的 403 之间的关系**尚未验证**——这是下一步要单独判断的事）。

### 剩余 2 项的新线索：Worker 里的跨源请求在传输层失败（2026-09-18 深夜，第四轮）

接上节「Worker 内部 outbox 的发送路径尚未定位」，本轮又排除了几个可能，并把范围压到一个具体机制上。

#### 已排除

| 假设 | 实测 | 结论 |
| --- | --- | --- |
| Worker 定时器不工作（flush 是 1.5s `setTimeout` 防抖） | 自建 Worker 里 `setTimeout(1500/6000/20000)` **全部准时触发**，`setInterval(5000)` 持续 tick 到 45s；页面侧定时器同样正常（44s 的 `setTimeout` 照常触发） | **排除** |
| Worker 抛错 | 给 SDK 自己的 2 个 Worker 挂 `error`/`messageerror` 监听，**零错误**；页面收到 Worker 的正常消息（WebGL 分段探测结果） | **排除** |
| outbox 读不到 | 按时间轮询 `ua:default` 的 `queued_events`：16s 时出现 2 条（venus + warp9），此后 24/34/44s **记录数不变、`sendsAttempted` 恒为 0、`lastAttemptedAtMs` 恒为 0、`tombstone` 恒为 0** | 事件已入队，**flush 从未尝试发送** |
| 事件没被接受 | 记录确实写入（`type: venus` / `type: warp9`），说明 `areFeaturesEnabled("analytics")` 是**通过的** | **排除**「analytics 被关」 |
| 白名单 | `web-channels` 201、`contacts/identify` 200、`remote-data` 200 都通；`OriginGuard` 在 Worker 里也生效（越权源两侧同样报 `Request blocked`） | **排除** |

#### 新发现：Worker realm 的跨源 fetch 失败在传输层

串行（非并发，排除竞态）对比，同一个页面里分别从 **Worker** 和 **页面** 发同样的 GET：

| 目标 | Worker realm | 页面 realm |
| --- | --- | --- |
| `https://aswpsdkus.com/notify/v2/ua-sdk.min.js` | **`Error: Network error: … error sending request for url`**（ms≈1） | `TypeError: Failed to fetch: CORS error: Origin 'https://www.southwest.com' not in Access-Control-Allow-Origin` |
| `https://aswpapius.com/api/web-events` | **同样的 Network error** | CORS error |
| `https://www.southwest.com/api/security/v4/security/digital/jwks` | **400**（请求成功抵达源站） | **400** |

- 页面侧的「CORS error」意味着**请求确实发出去了**、只是响应缺 ACAO —— 与 Chrome 行为一致。
- Worker 侧同样的跨源地址却在**传输层就失败**（`error sending request` 是 reqwest/primp 层的发送失败，不是 CORS 判定）。
- **同源**（southwest.com）在两个 realm 都正常。
- runtime 的网络日志里**能看到** Worker 发出的这些 request 行，但**没有对应 response**，与「发送失败」一致。
- 即：**Worker realm 的 fetch 对跨源主机无法完成发送，而页面 realm 可以。** 这很可能就是 SDK 在 Worker 里 flush outbox 失败的原因（缺的 `web-events` / `warp9/` 正是由 Worker 侧的 RPC/flush 发出的）。

#### 尚未证明 / 待办

- **未证明**这就是缺那 2 个资源的原因（尚未看到 SDK 的 flush 真的发起过请求——`sendsAttempted` 一直是 0，说明连"尝试"都没记录；若 SDK 在发送前就抛错，也可能停在更早一步）。
- **未定位** Worker 传输失败的具体原因。最可疑的方向：Worker realm 的 fetch 走的传输/proxy 配置与页面 realm 不一致（例如没有继承 `proxy_url`），需要看 Worker 里 fetch 的实现路径。
- **建议下一步**：核对 Worker realm 发起请求时是否走 `StealthHttpClient`/`proxy_url`；在 Worker 里 fetch 一个**必须经代理才能到达**的地址，与页面侧对比出口 IP（本轮用 `tls.peet.ws` 做该实验失败，因为它不在 `allowed_origins` 里，两侧都返回 `Request blocked`——需要一个在白名单内、且只在代理下可达的地址，或临时把探针域名加进白名单）。

#### 状态

- 诊断钩子已再次**完全撤销**（`grep -c "diag-preload" runtime/src/browser.rs` → 0）。
- runtime SHA-256 与通过门禁时**一致**：`5edc98aeae29bd5b21b915b39ff4b7b1d1387e3d563aa09d246aa17b3e938fc5`。
- 门禁（当次内容）：根工作区 **1760 passed / 4 skipped**；`obscura-js` **553/553**；独立 runtime **184/184**。
- 工作区改动文件：`crates/obscura-js/js/bootstrap.js`、`crates/obscura-js/src/runtime.rs`、`crates/obscura-net/src/{stealth_client,stealth_transport}.rs`、`crates/obscura-browser/src/page.rs`、`runtime/src/browser.rs`、`docs/Southwest-handoff.md`、`Cargo.toml`、`runtime/Cargo.toml`、`vendor/primp/OBSCURA_PATCHES.md`，以及新增的 `vendor/primp-rustls/`。
- **业务结果：shopping 仍 403。**

### 找到传输层缺陷：跨 runtime 复用连接池 → broken pipe（2026-09-18 深夜，第五轮）

#### 先更正我上一节的错误结论（必须收回）

上一节我写「**Worker realm 的跨源 fetch 在传输层失败，页面 realm 可以**」，并据此推断「Worker 走了 reqwest 而非 primp」。**这两条都是错的**：

1. **错误串区分不了两个客户端**。`vendor/primp/src/error.rs:309` 里有**完全相同**的 `Kind::Request => f.write_str("error sending request")`——primp 是 rquest/reqwest 的派生，错误文案一字不差。所以「看到 reqwest 文案 ⇒ 没用 primp」这个推断**不成立**。
2. **之后的运行里 Worker 的跨源请求是成功的**。换一次运行，Worker 对 `aswpapius.com/api/web-events` 返回的是 `CORS error`（=请求已抵达服务器），与页面一致。**所以这不是 realm 差异，是瞬时/逐请求的。**

重新核对：错误来自 `crates/obscura-net/src/stealth_transport.rs:116` 的 `request.send()` → `network_error()`，即**确实是 primp 传输**。Worker 走的就是 primp。

#### 真正的 cause（临时诊断：把 cause 链打出来）

在 `network_error()` 里临时附上 `std::error::Error::source()` 链（**已撤销**），拿到：

```
Network error: https://aswpsdkus.com/notify/v2/ua-sdk.min.js: error sending request for url (...)
 | cause0: connection closed because of a broken pipe
```

**是断管**，而且是 `ms=1~2` 的**瞬时失败**——连接没有真正建立/写入就失败了。

#### 判决实验（同一 Worker 内，连续请求）

| 序号 | 目标 | 结果 |
| --- | --- | --- |
| 0 | `aswpsdkus.com/notify/v2/ua-436.min.js`（页面已取过该 URL） | **ms=2 失败** `connection closed because of a broken pipe` |
| 1 | 同一 URL 再来一次 | **成功**（ms=349，抵达服务器，被 CORS 拒） |
| 2 | 第三次 | **成功**（ms=133） |
| 3 | `cookies-data.onetrust.io/`（页面从未用过的域） | **成功**（ms=2658） |
| 4 | 再来一次 | **成功**（ms=1273） |

**只有「第一次复用池中那条（由页面 runtime 建立的）连接」会断管；之后 Worker 自己建立的连接全部正常。**

#### 机制判断

`crates/obscura-js/src/worker.rs` 里 Worker 是**另起一个线程 + 自己的 current-thread tokio runtime**：

```rust
std::thread::Builder::new().name(format!("obscura-worker-{id}")).spawn(move || {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build();
    runtime.block_on(run_worker(id, config, command_rx, events, child_control));
})
```

而 WorkerConfig 直接**克隆了页面的客户端**：

```rust
http: parent.http_client.clone(), callbacks: parent.callbacks.clone(),
#[cfg(feature = "stealth")]
stealth: parent.stealth_client.clone(),
```

连接池是**客户端级**的，但池里的连接是**在建立它的那个 runtime 上被驱动的**。Worker 在自己的 runtime 上复用这些连接 → 连接不可用 → 断管。池随后被修复，Worker 自建的连接工作正常——与实验现象完全吻合。

**这是一个真实的、可复现的引擎缺陷**，最小复现：Worker 内对「页面已访问过的同一主机」发起请求，第一次必失败（断管），第二次起正常。

#### 与本任务的关联（谨慎表述，勿过度归因）

- **shopping 的 403 不受此影响**：shopping 是**页面 realm** 的 fetch，那条路径正常。
- Airship 的那条链路**可能**受影响：SDK 的重试节流是 `minTimeBetweenAttemptsMs = reports.requestTimeoutMs + 1000 = 61000ms`（即**61 秒**），而此前我的驻留只有 45–60s。曾据此假设「flush 第一次发送撞上断管，重试要等 61s，所以 60s 内看不到 `web-events`/`warp9`」。
  - **该假设已被否定**：驻留 **150 秒** 重跑，仍然只有 **6/8**，`web-events` / `warp9` 始终不出现。所以不是「等得不够久」。
- **`sendsAttempted` 一直为 0**，说明 outbox 连「尝试」都没记 → flush **确实从未发起**。真正原因**仍未定位**，但已排除：启动门、appKey、`PushManager`、analytics flag、可见性事件、Worker 定时器、Worker 报错、白名单、等待时长。

#### 修复方案的取舍（未实施，需决策）

客户端没有暴露 `proxy_url` / `profile` 访问器（`StealthHttpClient` 的公开 API 只有构造器与 fetch 系列），所以「给 Worker 单独建客户端」需要先把这两项穿透几层传下来。两条路：

- **A（治本）**：Worker 不复用父客户端的池——给它自己的 `StealthHttpClient`（同 profile / 同 proxy）。改动跨层，但消除跨 runtime 复用。
- **B（治标）**：在 `stealth_transport.rs` 的 `send()` 上，对「连接已被对端关闭/断管」这类**尚未写出请求**的失败重试一次。注意：**绝不能**对非幂等请求（尤其是 shopping 的 POST）盲目重试，必须严格限定在「请求未发出」的情形。

本轮**未实施任何修复**，避免在未定论时改动请求语义。

#### 状态
- 临时 cause 链诊断**已撤销**；`crates/obscura-net/src/stealth_transport.rs` 恢复原样（可从 `/tmp/obscura-resume-20260918/stealth_transport.rs.before-cause` 比对）。
- 本轮业务结果：**shopping 仍 403；aswp 仍为 6/8（未变）**。
- 工作区：`crates/obscura-net/src/stealth_transport.rs` 与临时诊断前的备份**逐字节相同**（SHA-256 `079c741392c6b0fe61159b08cece25e0980d1729965dd8e83d954e0639fba4be`），对 HEAD 的 diff 仅为既有的 Chrome-153 改动。

### 修复：Worker 独立传输（消除跨 runtime 复用连接池）（2026-09-18 深夜，第六轮）

#### 修复 A 已实施并验证

**问题**：Worker 跑在**自己的线程 + 自己的 tokio runtime** 上，却直接 `parent.stealth_client.clone()` 复用页面的 client。连接池属于**建立/驱动它的那个 runtime**；池中连接被拿到另一个 runtime 上复用 → 首次使用即断管（`connection closed because of a broken pipe`），且是 `ms=1~2` 的瞬时失败。

**改动（跨 2 个 crate，3 个文件）**：

1. `crates/obscura-net/src/stealth_client.rs`
   - 新增 `pub struct TransportParams { profile, proxy_url, accept_language, do_not_track }`，由 client 持有——这是重建兄弟 client 所需的全部传输配置（此前没保存，所以无法重建）。
   - `extra_headers` 由 `RwLock<..>` 改为 `Arc<RwLock<..>>`：可共享，让 Worker 继续看到同一套 header 覆写。
   - `transport` 模块由 `mod` 改为 `pub(crate) mod`，供跨 crate 重建。
   - 新增 `pub fn transport_params(&self)` 与 **`pub fn detached(&self) -> Self`**：**保留身份**（cookie jar、in-flight 计数、header 覆写、policy/拦截器）**但拥有全新的连接池**。
2. `crates/obscura-js/src/worker.rs`
   - `op_worker_create` 里不再 clone 父 client，改为 `obscura_net::StealthHttpClient::detached(client)` 建一个**兄弟传输**。
3. 新增回归测试 `detached_client_shares_identity_and_keeps_transport_configuration`（`cargo nextest run -p obscura-net --features stealth`）：
   - 断言 cookie jar / in-flight / `extra_headers` 三者 `Arc::ptr_eq`（身份确实共享）；
   - 断言 `profile` / `proxy_url` / `accept_language` / `do_not_track` **逐项原样保留**（漏掉 proxy 会让抓取静默丢代理，这是最容易犯的错）。

**红/绿证据**（同一页面、同一 Worker 内连续请求，探针 `probe-pool.js`）：

| 序号 | 目标 | 修复前 | 修复后 |
| --- | --- | --- | --- |
| 0 | `aswpsdkus.com/notify/v2/ua-436.min.js`（页面已取过） | **ms=2 失败** `broken pipe` | **ms=644 成功抵达服务器** |
| 1 | 同一 URL | 成功 | 成功 |
| 2 | 同一 URL | 成功 | 成功 |
| 3–4 | 页面从未用过的域 | 成功 | 成功 |

→ 修复前「**第一次复用池连接必失败**」已消失；修复后 5/5 全部抵达服务器。

#### 仍未解决：Airship 的 `web-events` / `warp9`（6/8 → 未改善）

连接池修复**没有**解开这两个端点：修复后 60s 驻留仍为 **6/8**，`queued_events` 的 `sendsAttempted` 仍为 **0**（即 flush 从未发起）。这与「事件已入队但从不发送」一致。

#### 本轮更正的另一个错误结论（诚实记录）

上一轮我写「**`queued_events` 在页面侧的操作计数为 0 → 写入来自 Worker**」。**这是错的**，源于我对**被压缩截断的日志文件**做 grep，得到假阴性。实测（按 (op, store) 统计页面侧全部 IDB 操作）：

```
  48  ('tx', 'blob')        39  ('get', 'blob')      9  ('put', 'blob')
   4  ('tx', 'queued_events')  2  ('tx', 'cookies')  2  ('count','blob')
   1  ('add','cookies')      1  ('getAll','blob')    1  ('tx','hashing_cache')
```

→ **`queued_events` 的事务确实发生在页面**。进一步核对页面创建的全部 Worker（`probe-blobs.js`，hook `URL.createObjectURL` 并检索 blob 源码）：

| blob | 大小 | 命中标记 | 判定 |
| --- | --- | --- | --- |
| `(function D(hD,v,d,V){…` | 408096B | `OffscreenCanvas` | 混淆的指纹/传感器脚本 |
| webpack 运行时 | 9699B | 无 | WebGL 参数分段 worker |
| `if(typeof OffscreenCanvas!=='function')…` | 2479B | `OffscreenCanvas` | WebGL 能力探测 |

**没有任何一个 blob 含 `aswpapius` / `airship` / `ua-sdk` / `queued_events` / `warp9` / `venus`**；RPC 观测也只看到这两个 worker 的 WebGL 消息。

→ **Airship SDK 跑在页面里，不在 Worker。** 上一轮「Worker realm 的请求是 Airship 失败原因」这条线索对 Airship 是**红鲱鱼**（Worker 传输缺陷本身真实且已修，但与本任务的两个端点无关）。

#### 状态
- 两处临时诊断（runtime preload 钩子、cause 链）**均已撤销**；`runtime/src/browser.rs` 诊断引用计数为 0。
- runtime SHA-256 更新为 `af212fa49a53a3dfb14f3d0ad4aebec53d017911df91f6ac0b35aa1c17637c76`（含本轮修复）。
- 业务结果：**shopping 仍 403；aswp 仍 6/8（未改善）**。

#### 尚未处理的同类缺陷（已定位，未修）

`crates/obscura-net/src/client.rs` 的 `ObscuraHttpClient` **有同一个跨 runtime 池问题**（池在 `tokio::sync::OnceCell<Client>` 里，Worker 复用的是页面实例）。但它的 `interceptor: RwLock<Option<Box<dyn RequestInterceptor + Send + Sync>>>` **不可共享**，所以要做同样的 detached 必须先把它改成 `Arc<dyn ...>`；另外 `user_agent` / `accept_language` / `extra_headers` 也是裸 `RwLock`，要共享同样需要包 `Arc`。这是一次更宽的重构，**本轮未做**。
- 影响面：stealth 开启时，脚本化的 fetch/XHR 走 stealth 路径（已验证 Worker 的请求确实经过 primp），所以 `ObscuraHttpClient` 主要承担非 stealth 回退与内部资源加载。**是否实际触发未验证**，不应假定。

### 打通 Airship（真正原因）：升级事务为 null + 索引从未创建（2026-09-18 深夜，第七轮）

**结果：aswp 资源数 6/8 → 8/8，与 Chrome 逐项一致；outbox `sendsAttempted` 0 → 1。**

#### 决策性发现一：`upgradeneeded` 期间 `request.transaction` 是 null

Airship 的 SDK 这样接收升级回调：

```js
const s = indexedDB.open(t, e), a = Ae(s);
i && s.addEventListener("upgradeneeded", (t => {
  i(Ae(s.result), t.oldVersion, t.newVersion, Ae(s.transaction), t)
}))
```

**迁移函数拿到的 transaction 就是 `request.transaction`。** 而 Obscura 的 `IDBRequest` 把它初始化成 `null` 之后**从未赋值** → 每一次 schema 迁移都在 `e.objectStore(...)` 上抛错 → **整个 schema（含全部索引）静默地从未建立**。

#### 决策性发现二：索引从来没有被真正创建

`IDBObjectStore.createIndex()` 之前是 `return new IDBIndex()`——**返回一个对象就丢掉，从不登记**；`index(name)` 返回**全新空壳**。所以即使迁移走到了 `createIndex`，索引也不存在；而 `store.index("...")` 永远返回一个查不到任何东西的假索引。

对照实测（`probe-indexes.js`，读 `store.indexNames`）：

| 库/存储 | 修复后 |
| --- | --- |
| `ua:default@9` → `queued_events` | `type, sendsAttempted, queuedAtMs, lastAttemptedAtMs, lastSendId, type-queuedAtMs, tombstone, tombstone-type-queuedAtMs, tombstone-lastAttemptedAtMs` |
| `iaa_schedule` | `createdAtMs, lastUpdatedAtMs, storedAtMs, priority, executionCount` |
| `hashing_cache` | `expiresAtMs, hashedId, hashedIdType, stickyId, storedAtMs, hashedIdType-hashedId-stickyId-storedAtMs` |
| `remote_data_listing` | `source, sourceId, ..., source-sourceId-sdkVersion-randomValue-fetchedAtMs` |
| 其余 6 个存储 + Adobe `DSE_CookieStore` | 全部到位 |

**修复前这些都是空的。**

#### 修复（`crates/obscura-js/js/bootstrap.js`）

1. **升级事务**：`IDBFactory.open()` 在派发 `upgradeneeded` 前构造一个 `versionchange` 事务，赋给 `req.transaction`，并用 `_begin()/_finish()` 把它在回调期间**保持打开**（否则 `complete` 会早于迁移触发）；升级完成后置回 `null`。
2. **`IDBTransaction.objectStore()`**：先查 `_stores`，再查 **数据库注册表**（升级期间新建的存储必须能通过事务取到），取不到则抛 `NotFoundError`。此前它会**返回一个游离的占位 store**，把写入和索引登记悄悄丢掉。
3. **`IDBObjectStore.createIndex/index/deleteIndex`**：真正登记到 `this._indexes`；`index(name)` 找不到抛 `NotFoundError`；新增活的 `indexNames`（原型 getter）。
4. **`IDBIndex` 真实实现**：按 `keyPath`（支持点路径与**复合数组**）在存储记录上投影，支持 `get/getKey/getAll/getAllKeys/count/openCursor/openKeyCursor`，含 `multiEntry`。
5. **`IDBCursor` / `IDBCursorWithValue` 真实实现**：`continue/advance/continuePrimaryKey` 真正推进；`update(value)` 按游标 `primaryKey` 写回；`delete()` 删对应记录。新增 `_idbCursorRequest`——**同一个 request 反复 settle**（`continue()` 后再触发 `success`），这是异步迭代器（SDK 的 `Ve(...)`）能走完的前提。
6. **`IDBKeyRange` 改为真构造器**：`instanceof` 可用；`includes()` 用 **IndexedDB 键序**（number < date < string < binary < array，数组逐元素比较）而不是 JS 的 `>`/`<`（后者会把数组键按字符串比较，静默匹配错记录）。
7. **键序排序**：新增 `_idbCompareKeys` / `_idbKeyRank` / `_idbExtractKey` / `_idbOrderRows`；`getAll`/`getAllKeys`/游标按主键序返回（符合规范），支持 `next/nextunique/prev/prevunique`。

#### 红/绿证据

| 指标 | 修复前 | 修复后 |
| --- | --- | --- |
| aswp 资源（Chrome = 8） | 6（缺 `web-events`、`warp9/`） | **8/8，逐项一致**（生产二进制跑 2 次） |
| outbox `sendsAttempted` | **0**（从未尝试发送） | **1** |
| outbox `tombstone` / `lastAttemptedAtMs` / `lastSendId` | 0 / 0 / 空 | 1 / 有值 / 有值 |
| `queued_events` 索引 | 无 | 9 个，与 SDK 迁移代码一致 |
| `warp9/` 请求头 | — | `authorization, x-ua-appkey, x-ua-channel-id, x-ua-contact-id` |

新增回归测试 `version_upgrade_exposes_a_transaction_and_registers_indexes`（`obscura-js`）：断言升级期 `request.transaction` 非 null 且 `mode === 'versionchange'`、`createObjectStore` 建的存储**能从事务里取到同一个对象**、`createIndex` 被登记、以及用 SDK 原样的 `IDBKeyRange.bound(['a',0],['a',1/0])` 复合键游标**按序取到正确记录**。

#### 顺带说明

- **shopping 仍 403**。Airship 打通是保真度工作，与本任务的 403 **没有已证实的因果关系**，不应用它暗示 403 有救。
- 这轮同时修好了 Adobe `DSE_CookieStore` 的索引，以及任何依赖索引/游标的库——之前它们全都"查不到东西"。
- 既有失败（与本次改动无关，已用旧 bootstrap 复现确认）：`obscura-js::window_scroll_fires_a_scroll_event`（仅非 `render` 构建存在，根门禁覆盖不到）。

#### 门禁与产物（本轮结束时）

| 项 | 结果 |
| --- | --- |
| 根工作区 `cargo nextest --release --features render` | **1762 passed / 4 skipped** |
| 独立 runtime | **184 passed** |
| `obscura-net --features stealth` | **114 passed**（含 2 个 detached 回归测试） |
| runtime 二进制 SHA-256 | `ec2bdf02f83d9f6a5bc7ae442c764f5e5fab7191896826b779f444f09d61e9c1` |
| 诊断代码残留 | 0（`grep -rc "diag-preload\|cause0\|DIAGNOSTIC"` 于 6 个改动文件） |
| 未跟踪文件 | 仅 `vendor/primp-rustls/`（预期的 vendored 依赖） |

`bootstrap.js` 备份对照点（`/tmp/obscura-resume-20260918/`）：`bootstrap.js.before-idb`、`bootstrap.js.before-idxindex`、`bootstrap.js.after-idxindex`。

## 续推核验（2026-09-18，runtime 更新与主线程请求诊断）

- 本轮开始先按用户要求核验本地 `runtime/target/release/autopilot-browser-runtime`：修复 fragment 前的工作区源码 locked release 构建完成（render + stealth），184 项独立 runtime release nextest 全部通过。生产候选 SHA-256 仍为 `f763293b43673dd1d69a07604ca3d4f35ff808113219612a0a39957f5c4f0d3d`，说明当时产物已对应修复前源码；本次没有把 Beacon 转发实验合入，也没有更新其他仓库的消费 pin 或远端部署。日志 `/tmp/obscura-runtime-update-20260918.log`、`/tmp/obscura-runtime-update-tests-20260918.log`，源码/产物哈希见 `/tmp/obscura-beacon-20260918/runtime-update-manifest.json`。
- 本轮 native Chrome 现有隐身上下文的新标签页再次显示固定查询的航班列表。此为页面成功控制，不是新鲜会话 HAR。
- 隔离诊断 runtime 开启原生 Runtime/Console 事件，并在独立 preload 中记录 fetch 拒绝及 error/unhandledrejection；生产 runtime 和 SDK 接口未改。原生事件从最终页面读取，不能据此声称跨导航没有丢失异常。产物 `runtime-events-diagnostic` 与 `events-diagnostic-manifest.json` 均在上述 `/tmp` 根目录。
- `live-runtime-events/` 两次 shopping 403，控制台出现 `AT: request failed`。`live-fetch-errors/` 定位到仅允许 www.southwest.com 时，本地策略拦截 JWKS、soptimize、demdex、zeronaught、smetrics 等请求；AT 日志字符串来自 `/swa-resources/scripts/analytics/analytics.js`，不是已证明的浏览器执行崩溃。
- 以旧成功 HAR 和本轮实际请求构造 16 个精确 HTTPS origin 的受控对照 `live-observed-origins-16/`：JWKS、soptimize delivery、zeronaught 请求均返回 200，AT 警告消失，但三次 shopping 仍为 403。仍有 app.link 和后续出现的 Qualtrics 资源被拦截，不能宣称全部请求都与 Chrome 一致。17 origin 的初次启动被现有上限拒绝（INVALID_ORIGINS），不算业务实验；未修改上限或关闭 SSRF。
- 扩域后的页面出现 `TypeError: Cannot redefine property` 未处理拒绝。`live-error-stack/` 再现并取栈：Cookie banner 插入 DocumentFragment，异常栈中包含 Obscura fragment 内部递归再次调用页面包装 appendChild 的路径，随后进入 frame 初始化。该次异常约发生在启动后 11.9 秒，三次 shopping 403 已在约 7.0–8.1 秒发生，因此它不能解释该轮首次拒绝。
- 最小通用夹具 `fixtures/fragment-append-reentry.html` 已证明可观察差异：Chrome 包装调用节点类型为 `[11]`，原生产候选为 `[11,1,1]`；两者最终均插入两个子节点。现以此建立回归，修复 fragment 插入内部重入页面覆写方法的问题。原始异常属性名和请求标识符只保留本机。

- Fragment 修复已落入生产源码：保存 Node 原始 appendChild 供 fragment 内部递归使用，避免重新调用页面覆写的 public 方法。新增回归覆盖 prototype 包装、实例覆写、节点顺序、返回值和 fragment 清空，red → green；53 项相关 release render 测试通过。修复后的本地 runtime 夹具返回 `[11]`，与 Chrome 一致。本次不扩展到 insertBefore/replaceChild 等其他方法的内部公共调用语义。
- 最新生产 runtime 已重建，SHA-256 **`485f1e6da78ca123c58f1c12d84c3d48bcb5ac5a17cd6e9909e17972cd549b3a`**；保留为 `/tmp/obscura-beacon-20260918/runtime-fragment-fixed`，源码哈希见 `fragment-source-manifest.json`。独立 runtime 测试再次 184/184 通过。`live-fragment-production/` 使用此真实生产二进制、16 个观察到的 origin，无诊断 preload，三次 shopping 仍为 **403050700**，回调正文全部完整。不能把此兼容性修复当作业务已恢复。

- `live-error-stack-fixed/` 使用重建的隔离诊断版本复测：旧栈中的 fragment 内部 appendChild 重入消失，但重复定义异常仍存在于页面脚本的 frame 初始化路径，shopping 为六次 403。说明本次 DOM 修复只消除已证明的公共方法重入缺口，不能解释整个异常；后续需追踪 iframe window 身份、生命周期及页面对构造器的包装，不能直接放宽不可配置属性语义或加入站点特判。

- 下一步已有最小可复现目标：`fixtures/frame-disconnected-lifecycle.html` 中，iframe 位于未连接的 DocumentFragment 内，以及从文档移除后，native Chrome 的 contentWindow/contentDocument 均为 null；当前候选仍均非 null。连接期间两者均非 null。候选结果 `/tmp/obscura-resume-20260917/resume-frame-disconnected/result.json`。源码 contentWindow 只检查 parentNode 是否为 null（fragment 内不为 null），contentDocument 还能按需创建代理；这与浏览器是否有活动 browsing context 的语义不同。尚未修改该生命周期，也未证明它造成真实页面重复初始化或 403；修复需覆盖断开、重插入、已加载 frame realm 及旧 window 引用，不宜只在 getter 加一个判断就宣称完整。

- Fragment 修复后完整 release render nextest **1753 passed、4 skipped、0 failed**（`/tmp/obscura-fragment-full.log`），指定 release CLI 构建通过（`/tmp/obscura-fragment-cli-build.log`）。相关 red/green 与 runtime 构建/测试日志使用 `/tmp/obscura-fragment-` 前缀。 原样 obstacle course 再次 **32/33**，仍仅 `observer-intersection` 失败（`/tmp/obscura-fragment-obstacle.log`），33/33 门禁未达成。全部本轮构建及测试已结束。本轮没有修改渲染管线或宣称新的性能/截图比较结论。

## 续推核验（2026-09-18，Beacon 与固定输入 Worker 对照）

- **业务仍未通过。** native Chrome 在现有隐身上下文打开同一固定查询，显示 26 条行程（9 条直飞、17 条中转）；本轮不是全新隐身会话，也没有重新导出 HAR。代理 IP 的判断仍依用户确认，不根据代理入口自行推断。
- 当前生产候选 `resume-frame-context/runtime-event-accessors` 重新执行 `worker-replay-deterministic.html`，157 字节与本轮 native Chrome 逐字节一致，末两字节均为 `[72,184]`，也与保留的 `worker-replay-chrome-fixed.json` 一致。结果在 `/tmp/obscura-resume-20260917/resume-20260918-worker-replay/result.json`。下方早期 `[90,164]` 差异不再代表当前候选；本次仅验证固定源码、固定输入回放，不证明真实页面的动态 Worker 输入或主线程环境一致。
- 确认通用缺口：`bootstrap.js` 中 `navigator.sendBeacon` 及 `Navigator.prototype.sendBeacon` 直接返回 true，不发送请求。本机 HTTP 夹具中，原候选返回 true 而服务端没有收到请求；native Chrome 收到 POST，正文 `beacon-probe`、Content-Type 为 `text/plain;charset=UTF-8`。真实页面被动包装记录到 6 次调用均返回 true，但没有对应传输。
- 在仓库外复制 runtime，使用单独名称 `obscura-beacon-diagnostic` 构建，只增加诊断 preload 和只读调用记录出口。诊断转发通过通用 fetch POST、include credentials 补发字符串 Beacon；同一本机夹具确认服务端收到与 Chrome 相同的方法、正文和 Content-Type。该转发没有完整实现聚合 keepalive 配额和页面销毁生命周期，不是可合入的 Beacon 实现。
- 有效对照 `live-audit-verified/` 为两次 shopping 403；`live-forward/` 补发的 **8 次 `/di/swadc/beacon/et` 均返回 204**，但两次 shopping 仍为 **403050700**，正文完整。最终页面调用数组只有 6 条，因为导航重置了数组；8 次来自跨导航 SDK 响应记录。结论仅为补发本次 Beacon 不足以恢复 shopping，不能排除更完整的生命周期或其他遥测差异。
- 最初 `live-audit/` 与 `forwarded/` 未成功注入 preload，不能当成有效对照：Python SDK 启动子进程只保留 PATH/LANG/TZ，后来改用诊断 workspace 文件加载并通过调用记录验证。有效本机转发结果为 `forwarded-verified/result.json`。
- 本轮证据根目录 `/tmp/obscura-beacon-20260918`：`original/`、`forwarded-verified/`、`live-audit-verified/`、`live-forward/`、`server-events.json`、`runtime-beacon-diagnostic` 和 `manifest.json`。原始正文、标识符及解码遥测只在本机保留，不提交 Git。保留的旧 Chrome HAR 仅用于历史请求链比较。
- 本轮未改生产代码，未覆盖生产 runtime；其 SHA-256 仍为 `f763293b43673dd1d69a07604ca3d4f35ff808113219612a0a39957f5c4f0d3d`。沿用前轮 1752 passed / 4 skipped 和障碍 32/33 的验证结果，本轮没有重跑或宣称新的完整门禁结果。
- 后续应继续取证主线程执行、会话建立及实际请求差异。不要重新以旧 Worker 末两字节、已实现的顶层上下文属性或单纯 IP 封锁作为当前根因；也不要把 fetch 转发实验直接合入生产。

## 续推核验（2026-09-17 15:06 UTC 起）

- 当前工作区比下方 event-clock 交接记录更新：已有 Window/Worker 上下文属性、更多 WebIDL/Worker 改动，以及 `ops.rs` 中的 `op_worker_run_script` 注册。原“四个修改文件”列表是历史范围，不是完整的当前差异清单。保留全部既有改动。
- 当前源码的 `global_context_flags_in_window_and_worker` release nextest 已通过。旧 `test-context-flags-live/metadata.json` 也记录补属性后的六次 403，但该旧实验未保住 shopping 正文，不能据此称已完全对齐。
- 本轮 native Chrome 在现有隐身上下文的新标签页直接打开固定查询，成功显示航班列表。这不是全新隐身会话，也未重新导出 shopping HAR。用户确认的同代理 IP 前提保持不变。
- 保留的本地 runtime（SHA-256 `4baddf38880203461f0656d472b35230178301f34be67f5b96aecd6891102513`）在 `resume-current-live/` 返回四次 shopping 403；四份回调正文均完整，均为 `403050700`。首次请求的业务 JSON 与 `chrome-fresh-all.har` 成功样本逐字段相同。SDK 头采集位于传输自动补头之前，不能把缺少的自动头当作线上缺头。该 runtime 尚未由本轮源码重建，不能声明源码严格对应。
- 新通用复现 `fixtures/context-frame-resume.html`：初始空白 iframe 在 Chrome 中继承父 origin 和 secure 状态，`crossOriginIsolated=false`，但 `location.origin="null"`；Obscura 初始 iframe 代理缺少前三项。本轮已修复该代理的内部上下文继承，避免被父页面覆写 `window.origin` 污染。回归观察到 red → green；HTTPS、HTTP、loopback、可替换 origin、只读 secure 及相关旧测试合计 3/3 通过。
- `fixtures/context-inheritance-resume.html` 的 Blob 和嵌套 Blob Worker 在保留 runtime 与 Chrome 中一致；data URL Worker 在 Obscura 返回 `net::ERR_FAILED`，Chrome 为 opaque origin（`"null"`）且继承 secure 状态。本轮未修改 data Worker 加载、sandbox、已加载 frame realm 或 COOP/COEP 隔离能力，也未认定 iframe 缺口是 403 根因。
- 初始 iframe 修复后的 locked runtime 构建成功，本地夹具已与 Chrome 一致，但 `resume-frame-context-live/` 仍两次 403，完整正文均为 `403050700`。
- 首轮完整 release nextest 为 1750 passed、2 failed、4 skipped。两项失败来自已有 `_installEventAccessors`：新描述符遮蔽继承的 body.onload 访问器，且 XHR 属性回调又被注册为监听器，造成重复调用和提前结束应用计数。本轮已保留继承访问器，并取消 XHR 属性回调的重复监听注册。两个原失败集成测试均已转绿（`/tmp/obscura-event-accessor-green.log`）。
- 包含事件修复的最新 runtime SHA-256 为 `f763293b43673dd1d69a07604ca3d4f35ff808113219612a0a39957f5c4f0d3d`，保留在 `resume-frame-context/runtime-event-accessors`，源码哈希见同目录 `event-accessor-source-manifest.json`。`resume-event-accessors-live/` 再次返回三次 shopping 403，三份正文完整且均为 `403050700`。这些修复仍不足以达成业务验收。
- 最终源码完整 release render nextest：**1752 passed、4 skipped、0 failed**（`/tmp/obscura-resume-final-full.log`）；指定 release CLI 构建与 locked runtime 构建成功。原样障碍课程仍为 **32/33**，唯一失败仍是 `observer-intersection`（`/tmp/obscura-resume-final-obstacle.log`），没有达到 33/33 门禁。本轮没有渲染管线改动，也没有新增真实站点截图或受控性能结论。所有本轮构建和测试已结束。
- 证据仍位于 `/tmp/obscura-resume-20260917`，新增本轮文件使用 `resume-` 前缀；构建和 nextest 日志为 `/tmp/obscura-frame-context-*.log`、`/tmp/obscura-event-accessor-*.log` 和 `/tmp/obscura-resume-final-*.log`。下一步继续定位实际环境/会话/请求差异；不要重复宣称顶层属性尚未实现，不要把已修复的初始 iframe 或 XHR 重复回调当成已证实的 403 根因。

## 未提交生产改动（较早 event-clock 交接范围）

当前除本文外有四个修改文件：

- `crates/obscura-js/js/bootstrap.js`：OffscreenCanvas 的 WebGL/WebGL2 路径复用 HTMLCanvasElement 的上下文实现，以自身作为 owner，保留尺寸上限及上下文类型约束；修复 Worker 中因没有 document 而返回空的路径。
- `crates/obscura-js/src/worker.rs`：向 Worker 快照传递 `__obscura_webgl_vendor`、`__obscura_webgl_renderer`。
- `bootstrap.js`：Event.timeStamp 与 fallback performance.now 使用启动时捕获的 Date.now 和相对时间原点，不受页面覆写 Date.now/performance.now 影响；对倒退读数作单调钳制。**这仍是毫秒级 wall clock 派生值，不是原生高精度单调时钟。** 没有扩展时间戳 readonly 等完整语义。
- `crates/obscura-js/src/runtime.rs`：新增 `offscreen_webgl_owns_its_context_in_window_and_worker`、`event_timestamps_use_an_internal_relative_clock_in_window_and_worker` 两个回归，均已观察 red → green。
- `docs/Worker-compatibility.md`：同步上述实现、验证与局限。

OffscreenCanvas 的 Worker 2D、导出、resize、transfer 等仍不完整。独立 Worker 线程/isolate/事件循环、structured clone、生命周期与网络转发，以及 runtime 查询不再饿死自动推进的修复，已经在 HEAD 中，不能当作本轮新增未提交改动。

## 历史验证：event clock 阶段源码

证据根目录统一为 `/tmp/obscura-resume-20260917`，下表路径相对此目录。全部构建和测试已结束。

| 检查 | 结果 | 证据 |
| --- | --- | --- |
| focused render | 32/32 | `event-clock-green.log` |
| focused render,stealth | 32/32 | `event-clock-stealth.log` |
| 核心完整 release render nextest | 1749 passed，4 skipped | `event-clock-full.log` |
| 独立 runtime release nextest | 184/184 | `event-clock-runtime-tests.log` |
| Python SDK | 33/33 | `event-clock-sdk-tests.log` |
| 指定 release CLI / locked runtime 构建 | 成功 | `build-event-clock-cli.log`、`build-event-clock-runtime.log` |
| 原始障碍课程 | **32/33，未达门禁** | `event-clock-obstacle.log` |
| 确定性渲染 | 66 对均成功、非空、像素一致 | `render-event-clock/results.json` |
| 交替性能，6 对 | 差异在噪声内 | `performance-event-clock/performance.json` |

性能 baseline/current 中位数：启动 20/20ms，launch-to-ready 404.1/406.2ms，RSS 44.64/44.52MB，500ms idle CPU 2.14/2.22ms，均 16 threads；自主推进结果均正确。没有性能提升结论。

障碍失败仍是 `observer-intersection`。原夹具注释说会观察新 sentinel，但代码只追加一批 10 项，没有实现该步骤；原样复制的夹具在 native Chrome 也只显示 Item 0–9，Obscura 返回 `{items:10,result:null}`。证据：`fixtures/observer-intersection-original.html`、`event-clock-observer-repro.log`。没有修改基准或计分，因此仍须报告 32/33。伴随 benchmark clone 位于 `benchmark`，revision `e4a5490899628053752aca8201f0e46a56360b2c`。

早一版 Offscreen 修复的真实站点 top/bottom 30 对见 `render-sites-v2/results.json`：Remix 两对双方空白，排除；24 对有效且一致，4 对有差异（Bulma、Apple、Porkbun）。Apple 四次交替复测一致，原差异未显示补丁特异性；另两站未完全控制资源和动画，不能作完整 fidelity 结论。**event clock 后重跑了 66 个确定性对照，没有重跑全部真实站点。** 这些是 Obscura baseline/candidate 比较，不冒充自动化 Chrome 渲染比较。

## 最新业务结果

固定查询：WN，BWI → MCO，2026-09-30，一名成人，单程。日期过期后同时更新 Chrome 和 Obscura，不能直接与旧日期比较。

```text
https://www.southwest.com/air/booking/select-depart.html?adultPassengersCount=1&adultsCount=1&destinationAirportCode=MCO&departureDate=2026-09-30&departureTimeOfDay=ALL_DAY&fareType=USD&int=HOMEQBOMAIR&originationAirportCode=BWI&passengerType=ADULT&tripType=oneway&returnDate=&returnTimeOfDay=ALL_DAY&promoCode=
```

- 2026-09-17 约 04:03:54 UTC 的新鲜隐身 Chrome：shopping 200，success=true，26 行程。`chrome-fresh.har` 含 Fetch/XHR 22 条；`chrome-fresh-all.har` 含 79 条、68 个正文、0 个 HTTP 403。该 sanitized HAR 不保留 Cookie 值。
- 用户于 2026-09-17 确认隐身 Chrome 成功与 Obscura 失败使用同一代理 IP。这排除了单纯代理/IP 封锁的解释；不代表两者的浏览器环境、会话、遥测或请求/传输行为相同，403 的具体触发因素仍需定位。
- 当前干净生产源码的 `event-clock-live/metadata.json`：三次 shopping 403；`shopping-1.bin`、`shopping-2.bin` 为 `403050700`，首次正文为 BODY_RELEASED。该功能测试与验证构建有重叠，不能作受控延迟测量。
- pristine HEAD、Offscreen、event clock、语言、设备 persona、JWKS/16 个 HAR origins、mini32 代理以及下述传输变体均未解决本轮 403。各实验的失败不等于证明该因素在所有场景无关。
- `combined-origins-live` 合并设备、16 origins、三项传输变更，仍两次 403；完整原生正文有错误码，168 次 classic_end 无错误，仍记录 6 个 origin blocks。未由此认定全部执行环境一致。

旧会话曾得到 Obscura 200/26 行程，这只能作为历史成功样本，不能覆盖最新失败。旧成功的 shopping 早于 Worker 和四子脚本，最新失败也不能单由执行先后解释。

## Worker 与环境差异（历史记录，当前状态见上方续推核验）

实际 `importScripts` 正文现在已经捕获，不再只有启动包装器：诊断新增 `op_worker_load_script` 的 `worker_import_script` 记录。

- 2479 字节 graphics import 的 SHA-256 为 `1eec5d0bc72fba33ce753f6009a277e07041fb92d221ae5839bbc5e8fff1d0bb`，与新鲜 Chrome 一致。Offscreen 修复让原来的 false 变为 a–h 八段结果，但没有解决 403。
- 大型 import 在不同捕获中动态变化（例如 Obscura 407937、Chrome 402351 字节）；9699 字节 wrapper 归一化一致不能证明 import 相同。Chrome 资源见 `chrome-fresh-sources.json.gz`、`chrome-fresh-worker-sources/`。
- `fixtures/worker-replay.json` 和 `worker-replay-deterministic.html` 固定同一源码与输入，禁外网，固定 Date/Math.random/performance。157 字节结果前 155 字节相同，末两字节 wrapper Chrome `[72,184]`、Obscura `[90,164]`；去 wrapper 后 Chrome `[78,184]`、Obscura `[90,164]`。baseline、Offscreen 和 event clock 的该结果相同。
- clock prototype、trusted message、Math 包装试验没有消除此差异；Error.prepareStackTrace 审计未记录 Error.stack 读取，不能凭猜测修改 stack。额外 Date.now 调用来自 Event 构造器，是 event clock 修复的最小复现来源。

**历史线索（顶层属性现已实现，不再作为接手待办）：** `fixtures/context-flags.html`、`context-flags/result.json` 显示 Obscura window 和 Worker 的全局 `origin`、`isSecureContext`、`crossOriginIsolated` 均为 undefined；native Chrome 在该 localhost 夹具中分别为 `http://127.0.0.1:18791`、true、false，双方 location.origin 正常。

仅在离线 `worker-replay-context-flags.html` 前置赋预期值，Obscura 末字节变为 `[87,164]`，仍不同于 Chrome `[78,184]`。当时生产代码尚未实现这些属性；当前已实现，见上方续推核验。该阶段提出应覆盖 origin 序列化/继承、potentially trustworthy origins、不安全祖先、Worker 创建者上下文及 COOP/COEP 与实际隔离能力；不能把 localhost 的 true/false 全局硬编码，也不能声称该实验已证明 403 根因。

## 传输诊断：只在隔离工作树，不能直接合入

目录 `diagnostic` 是 detached HEAD 的被动插桩工作树，包含 Offscreen 修复，**不包含 event clock 修复**。最后还含以下三项试验，所有 live 仍失败：

1. Chrome 152 signature_algorithms 前导 GREASE：隔离依赖中添加后 echo Peetprint 匹配，`sigalgs-live` 仍两次 403。只改 Chrome152 非 FIPS 路径，不改证书校验。
2. DNT 头顺序：Chrome 在 upgrade-insecure-requests 前；调整后 echo 的 GET navigation 头值与顺序一致，`dnt-live` 仍三次 403。不能推导 Southwest POST wire 已完全一致。
3. ALPS 广告：primp-rustls 原实现故意用非 h2 字节避免不支持的协商；试验改成 `0003026832` 后 echo 显示 h2，`alps-advert-live` 仍两次 403。**只是 advertisement 试验，不是完整 ALPS 支持，禁止当完整协议修复合入。**

`diagnostic/runtime/Cargo.toml` 使用绝对路径 patch 指向 `diagnostic-rustls`（复制的 primp-rustls 0.23.43），不可移植。诊断产物包括 `diagnostic-runtime`、`diagnostic-candidate-runtime`、`sigalgs-diagnostic-runtime`、`dnt-diagnostic-runtime`、`alps-advert-diagnostic-runtime`；不等于当前生产候选。

`chrome-transport.json`、`candidate-transport/response.json` 来自 tls.peet.ws echo。该 endpoint 的出口相同，UA/JA4、H2 SETTINGS/window/pseudoheader 顺序一致。这份 echo 证据本身不证明 Southwest 出口相同；本轮同一代理 IP 的判断另依据用户确认的 Chrome 成功对照，不能据此继续归因为单纯 IP 封锁。详细 hash 见 `transport-manifest.json`、`alps-evidence-manifest.json`。

native Chrome NetLog 已停止并写入 `chrome-southwest-netlog.json`；这是复用隐身上下文的成功查询，不是新的 fresh 控制。`parse-netlog-alps.py` → `chrome-southwest-alps-summary.json` 显示 shopping H2 session 30569 / stream 33 对应 SSL socket 30567：ClientHello 有 h2 ALPS，ServerEncryptedExtensions 仅类型 0/16，没有协商 ALPS；TLS1.3 PSK resumed，没有 offered early_data。该成功样本没有证实 ALPS 是必需条件。

`chrome-shopping-wire-header-names.json` 与原生 request-builder 前记录的普通头名称吻合；伪头、自动 Content-Length 属采集层级差异。Chrome 成功与 Obscura 失败均出现同一 Tokyo edge，Chrome 链还出现 NJ；这不是客户端出口归因证据。

## 证据、构建与复现入口

当前证据根目录 `/tmp/obscura-resume-20260917` 权限 700。它含原始会话数据，保持本机私有。当前源码的干净产物为：

- `event-clock-runtime`，SHA-256 `a2e4d8ec077911e0a3522edd13575cc73fccca3594d593e041005f090308771d`，对应 `event-clock-manifest.json`。
- `event-clock-cli`，当前 CLI 的保留副本。
- `baseline-runtime` 为 pristine 911a48e，SHA-256 `66b85a686a76d618c60f7b04be28aef14633e1e4a725517a7cd21ba9890e5199`。
- `candidate-runtime` 仅是较早 Offscreen 版，不要误作最新候选；其 SHA-256 `9885f6a780331ac7340d69d4b5de9e46151572512777dc68e97cf3ebbc944d27`。

工具链 PATH 需要加 `/tmp/clashtui-rustup/toolchains/1.98.1-aarch64-apple-darwin/bin`；nextest 在 `/opt/homebrew/bin/cargo-nextest`。rust-objcopy 的 libLLVM stripping warning 没有阻止构建。

```sh
export PATH="/tmp/clashtui-rustup/toolchains/1.98.1-aarch64-apple-darwin/bin:$PATH"
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --release --features render --no-fail-fast
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release --manifest-path runtime/Cargo.toml
```

runtime 是独立 workspace，默认 render+stealth。根 target 与 runtime/target 分开；诊断构建曾共用 target，因此运行前核验保留副本 hash，不凭文件名判断。不要同时向同一 target 构建不同源码。

本机 SDK：`PYTHONPATH=/Users/gster1981/work/obscura/bindings/python/src`。`run-profile.py` 使用正式 SDK 直接 goto，可配置 `AUDIT_PROFILE=macos_chrome152`、`AUDIT_PERSONA_OVERRIDES` JSON 路径、`AUDIT_ALLOWED_ORIGINS` JSON 路径、`AUDIT_PROXY`（默认本机 7890）。轻量脚本可能在 reload 后失去正文 handle，且 metadata 默认未记录 allow origins，必须额外保存配置。后续完整捕获优先采用归档 `run-profile-full.py` 的并发四个正文读取或原生插桩。无限并发正文读取会塞满 16-entry evidence mailbox，正常退出显示 BROWSER_EOF，不能误报 Worker crash。

渲染 helper 的 `render-venv/bin/python` 已装 numpy/PIL/scipy 等；环境中 playwright 仅为既有 helper import，没有获准驱动 Chrome。结构化克隆离线解码可用本机 Node 的 `v8.deserialize(Buffer.from(base64,'base64'))`。

### 旧归档与迁移边界

仓库根有未跟踪文件 `obscura-southwest-handoff-20260917.tar.gz`，SHA-256 `af82004f3c32fb6b9aeda7550301730d3bef342223c3e892b756f747d5821d15`。它是**上一会话**归档，已恢复到当前证据根下的 `southwest-request-audit/`，不包含本轮新增修复、Chrome 控制和 event clock 证据。不要提交它，也不要以为发送它就完成本轮迁移。

同一工作区接手可直接使用上述文件；跨设备必须另外安全传输当前未提交源码与所需新证据，并校验文件。本文的 `/tmp` 路径不是跨设备保证，也可能被系统清理。旧归档脚本含 `/Users/zg` 等旧主机绝对路径，先调整；旧文档中的 booster 安装 runtime 路径不代表本机安装状态。

## 停止时服务与 UI

无后台构建/测试待接管。保留两项本轮服务供本机接续，PID 只是记录，操作前核验命令：

- PID 83860：`python3 -m http.server 18791 --bind 127.0.0.1 --directory /tmp/obscura-resume-20260917/fixtures`。
- PID 12900：`ssh -N -o BatchMode=yes -o ConnectTimeout=5 -o ExitOnForwardFailure=yes -L 127.0.0.1:17890:127.0.0.1:7890 mini32`。本机 17890 转至 mini32 的 7890；测试程序仍在本机。

Chrome 最后为隐身窗口，包含 Southwest、tls.peet 与本地 Context flags 标签；本地夹具在前台，另有 Southwest 成功结果页。NetLog 已停止。不要关闭用户 Chrome 或工具自身进程来清理测试。当前用户只要求交接，下一步调查由 agy 接手后继续。

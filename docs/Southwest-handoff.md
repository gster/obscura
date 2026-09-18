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
- 不增加 Southwest 专用等待、脚本屏障、域名特判，不导入 Chrome Cookie/token。
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

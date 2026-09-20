# Obscura 开发 TODO

日期：2026-09-19
审阅基线：`e67e67b11eb265f097942055962622e43fcb9a16`
总体方案：[新架构](New_ACH.md)；当前证据：[SUMMARY](SUMMARY.md)

根本目标是：面向机票采购效率，完成对 RPA 友好、性能突出、在服务端可观察的行为与指纹上与固定参考 Chrome 一致，并具备强匿踪、反追踪和防身份标记能力的浏览器内核，降低身份关联被用于差异化报价、侵害消费权益的风险。CDP-first 和官方 Playwright Python 是实现路径，不替代这些产品目标。Puppeteer 兼容已弃用，不再作为 Automation CDP Profile 的新增/维护资格目标或发布门槛；现有 profile 中仍可能保留历史 Puppeteer initializer，清理前必须确认不影响 Playwright/raw CDP 共用行为。反追踪不等于主动设置 `DNT=1`。

## 使用规则

本清单是执行队列，不是已完成能力声明。保留 OB-001 至 OB-043 的编号，按最新决定修订 OB-003/004/006/012 等任务范围，新增 OB-044 至 OB-046；任务拆分用后缀，不复用旧编号。未完成任务标为未关闭，已有代码只减少工作量，不能替代正式验收。本次只修改文档，根/独立 runtime 测试与协议夹具结果已写入 SUMMARY；不将它们当作整项 G0 或产品资格完成。

P0 为当前主链或发布阻断；P1 为随后完成的能力与质量工作。P1 项进入声明支持的范围后，同样成为发布门禁。每个任务开始时由维护者指定 owner，填写实施 SHA；完成时附测试证据后才标为已关闭。安全问题的详细复现按 SECURITY.md 私下处理。

**立即执行：OB-001/025 固定基线与迁移清单；OB-021/005/006 清理自有 Python SDK 与私有 runtime；OB-012/044 统一 primp 并取消 stealth 开关；OB-014/015/016 强化统一 persona。** 最小 CDP 链 OB-026/027/028/029 支撑迁移，OB-045 持续收敛 Chrome 差异，OB-046 验收 Southwest shopping。保留 MCP（OB-003）及有用 CLI（OB-004），不再以删除它们为目标。删除包装前先保住独有能力和回归，不等待完整产品资格才开始清理。

## A. 立即执行与主链阻断

### OB-001 · P0 · 冻结构建与测试基线

状态：未关闭。基础清单与校验器已由 `741f40a` 交付；Linux x86_64 和完整资格仍未运行。

依赖：无。入口：根/runtime 的 Cargo.toml、Cargo.lock、工具链、AGENTS.md、CI。
动作：在 Linux x86_64 与 macOS arm64 固定源码、工具链、feature、系统/字体/CA、benchmark revision；分别运行根与独立 workspace。
完成：提交不含敏感数据的机器可读清单，关联命令、产物摘要、passed/failed/skipped/not-run；本次根 1762 passed/4 skipped、runtime 184 passed 的 macOS 本地结果只覆盖相应配置，Linux、完整客户端及发布资格仍须建立。

### OB-025 · P0 · 固定官方 Python 客户端及迁移范围

状态：未关闭。官方 Playwright Python 1.60.0、driver/依赖锁和迁移范围已由 `741f40a` 固定；完整 API 与双平台运行仍未完成。

依赖：无，可与 OB-001 同步。入口：新开发测试工程，既有 Python 调用代码的授权样本。
动作：锁定 Playwright Python、driver 与依赖；清点 connect、context/page、locator、frame、network、文件、关闭/取消 API。区分必需、可延后、不支持。
完成：官方客户端未修改；测试依赖不进入 Rust 生产包；明确 APIRequestContext/route.fetch、launch/connect 与 CDP 的边界。

### OB-026 · P0 · 建立最小 CDP 记录与差分 fixture

状态：未关闭。macOS 首批三路记录/差分工具与差异证据已由 `fc65daa` 交付；Linux 与采集开关前后的观察副作用仍未验证。

依赖：OB-001、OB-025。入口：已建立的 tools/unblocked/；与 OB-008 共用开发依赖和结果清单。
动作：同一合成页面分别跑参考 Chrome 正常启动、Chrome CDP、Obscura CDP；记录方法、参数、事件、session/context、错误与时序。
完成：记录器能保留因果与相对顺序、规范化随机 ID；失败应能稳定复现并定位首个分歧，全通过则保留通过证据，不人为制造失败；内部原始 trace 保留完整请求数据，输出目录由执行方在测试后妥善处理且不进入仓库或产品包；采集开关不改变验收结果。

### OB-027 · P0 · 发布 Automation CDP Profile 并清理占位成功

状态：未关闭。`9be460d` 已发布 Playwright Python 1.60.0 首批实际观测方法切片、必需 smoke、profile 校验器和精确 initializer allowlist；当前 smoke 已继续覆盖官方 `get_by_label().fill()`、单选/多选 `get_by_label().select_option()`、`get_by_role().click()`、固定 viewport 的 `Page.screenshot()` 与 `Browser.new_context()` 的显式创建/使用/关闭路径，但尚未覆盖迁移范围内的全部方法、参数组合、事件与生命周期。

依赖：OB-026。入口：crates/obscura-cdp/src/dispatch.rs、domains/、types.rs。
现状：当前 raw protocol 中的 37 个发送方法均已进入 `tools/unblocked/automation-cdp-profile.json`，CI 会直接读取未改写的完整日志并持续对账；官方 locator 路径新增覆盖 `DOM.getContentQuads`、`DOM.scrollIntoViewIfNeeded`、`Input.dispatchMouseEvent` 与 `Input.insertText`，并通过 `Runtime.callFunctionOn` 执行单选和多选 select option。截图路径固定 320×240 viewport，验证 `Page.getLayoutMetrics` 与 `Page.captureScreenshot`，解码 PNG 像素证明画面非空，并完整保留 PNG 数据。独立 context 路径验证 create/use/dispose、同源 localStorage/全局对象隔离、默认页面存活和默认 1280×720 指标；同一显式 context 的双页面路径验证 `Target.closeTarget`、关闭页拒绝后续 evaluation，以及存活页的闭包、Promise、remote handle 和 document 不受影响。生命周期探针先由服务端确认 B 页 fetch 仍被挂起，记录 close 边界后才释放响应，并完整保留分阶段 console 事件；本次 close 后事件为空。截图格式/选项全矩阵、代理等 context options、断连清理和完整 context 生命周期尚未取得资格。不存在的 `Log.auditMethodDoesNotExist` 及已限定 initializer 的越界参数会明确失败。utility world 真隔离、完整 actionability/focus/user-activation 语义、未列方法和未逐项验证的非法参数仍标为未完成或 not-qualified，不能从本切片外推。
范围修正：Puppeteer 已弃用。清理仅为 Puppeteer 保留的 utility-world 默认值、测试命名和客户端特例时，必须先确认不影响官方 Playwright Python 与 raw CDP 的已登记行为；Puppeteer 专属行为不再新增或作为回归门槛。
动作：逐方法标记 SUPPORTED / LIMITED / VERIFIED_NOOP / UNSUPPORTED，实施/验证状态另存；覆盖参数组合、返回、事件、作用域、错误。审计整域空成功和 Browser 域占位行为。
完成：最终迁移范围内的未知方法、非法参数明确失败；no-op 仅是有依据的精确允许项；版本号/空对象/初始化成功不能充当资格证据。正式 Python smoke 保持必需测试，profile 与实际 raw protocol 方法集合持续对账。

### OB-028 · P0 · Target、session 与页面生命周期闭环

状态：未关闭。官方 `Browser.new_context()` 已覆盖显式创建、同源状态隔离、双页面使用、单页关闭、context 关闭及默认页面继续执行；关闭 B 后 A 的闭包、Promise、remote handle 与 document 继续可用，B 拒绝后续 evaluation。服务端挂起响应在 close 后才释放，分阶段记录中没有 close 后 console 事件。长期并发、自动/显式附加全矩阵、导航失效、断连和完整事件归属仍未完成。

依赖：OB-027。入口：CDP dispatch/server、domains/target.rs、domains/page.rs。
动作：覆盖 flattened session、自动/显式附加、暂停后恢复、create/dispose context、双页面持续执行、导航/关闭/断连及事件归属。保留现有不拆除其他页面 isolate 的实现。
完成：操作 B 页面不损坏 A 的 timer/闭包/Promise/句柄；context/session 无串用；关闭后事件不再发往失效 session；明确断连不保证恢复。

### OB-029 · P0 · 正式客户端 utility world 与对象句柄

状态：未关闭。

依赖：OB-027，可与 OB-028 协作。入口：CDP domains/runtime.rs、JS runtime/frame。
现状：本次创建 isolated world 后仍能读到主世界全局值 73；源码 context registry 只校验路由。
动作：实现真实 world 隔离，并验证官方 locator 注入脚本、对象组释放、跨 frame 路由与导航失效。
完成：有实际 realm 与句柄生命周期证据，不只生成不同数字 ID；未知/陈旧/跨属主对象明确失败；role/label locator 不依赖自写替代客户端。

### OB-021 · P0 · 原生共享执行层，消除第二套产品协议

状态：未关闭。

依赖：OB-027，随 OB-028/029 分步推进。入口：obscura-browser、CDP、迁入的 browser 集成测试；已删除的私有 runtime 路径仅在 Git 历史中保留。
动作：列出原生输入、等待、资源观察、限额和启动保护的归属；逐项迁移到内核共享层并由 CDP 调用。
完成：正式客户端最小端到端场景不经过 SDK 私有 RPC；迁移项均有原路径/新路径对照。不得重写完整 Playwright Locator 产品。

### OB-011 · P0 · Cookie 请求上下文与无损状态往返

状态：未关闭。

依赖：OB-001；网络接入与 OB-012 协作。入口：obscura-net/src/cookies.rs、CDP cookie_params/Storage/Network、BrowserContext。
现状：Cookie 键已含 domain/name/path，内部 CookieEntry 已保留 host_only；version 1 envelope 直接持久化内部记录，历史裸 CookieInfo 数组仍可加载。CookieJar 已提供保留内部字段的 snapshot/from_snapshot/apply_snapshot_delta，CDP 连接关闭时按初始/当前快照差异合并新增、替换和删除。HTTP 与 document.cookie 的有效 Max-Age 已固定优先于 Expires，与属性排列顺序无关；后续非法值不会抹掉先前有效 Max-Age。Cookie 发送及 JS 可见结果按长 Path 优先、同长度创建顺序排列，同键有效值替换保留原顺序；内部快照和 version 1 文件记录 creation_order，缺少该字段的旧文件只能按文件数组顺序迁移。
动作：继续区分内部持久化与协议视图；完成 SameSite 完整请求上下文、分区及 CDP/MCP 对外状态往返；覆盖 clone、save/load、导入导出、HTTP 与 document.cookie。保留 version 1 envelope 与旧裸数组兼容，不删字段、不做脱敏。
完成：状态往返不改变原语义；同等属性不同顺序结果一致；匹配使用完整请求上下文；原有多 Path 与 HttpOnly 写保护回归保持通过。分区能力未实现则不得宣称支持。

排序切片验证：根 release nextest **1937/1937**，4 skipped；Cookie/真实 primp 回归在 render 与 no-render 均为 **62/62**，两种 CLI release build、障碍课 **33/33**、官方 Playwright smoke 与 **37-method** 画像通过。旧版真实 CLI 四次恢复均顺序错误，候选四次均正确；完整证据与限制见 [SUMMARY](SUMMARY.md)。

前一持久化切片（`8ddaa15`）验证：Cookie/context/server 相关 release 回归在 render 根门禁中 **53/53**，no-render 聚焦 **53/53**；最终根 release nextest **1926/1926**，4 skipped。render 与 no-default-features exact CLI release build 均成功；冻结 render 二进制通过 CI 固定 benchmark 障碍课 **33/33**、官方 Playwright Python **1.60.0** smoke 和 **37-method** 协议画像校验。二进制 SHA-256 为 `722ef38dcb23e1627271bb24809c2bf93855e652fc54eb9dd0438fe78c559ca4`（119072256 bytes）。Astra light 最终复核 0 blockers；`git diff --check` 通过。首轮根门禁曾出现 MCP `test_evaluate` 空标题失败，未改源码的单项重放 **1/1** 和完整复跑均通过；该测试忽略导航响应，现有证据不足以确定偶发根因，未将重跑通过称作修复。

额外真实 CLI 回归使用本地 HTTP fixture 和四个全新进程：HTTP Set-Cookie 的 `hostonly=alpha=beta` 与 document.cookie 的 `docvalue=from-document` 均经 version 1 文件恢复并出现在后续服务端 Cookie 请求头；持久化记录保留 host_only 和 HttpOnly。该探针验证产品存储入口，子域作用域由上述 Cookie 回归覆盖。

### OB-034 · P0 · CDP 访问、背压、断连与不重放

状态：未关闭。

依赖：OB-001、OB-027。入口：obscura-cdp/src/server.rs、共享初始化与 CLI。
动作：在已有连接/延迟消息上限之外，补入出站队列 count/bytes、帧大小、慢读与超载；核查认证、Host/Origin、默认 loopback；定义 owner 与关闭契约。
完成：慢客户端/大消息/导航中断不会无界堆积；明确错误且资源回收；授权客户端可正常连接；未获准客户端被拒绝；取消/重连不自动重放输入。

### OB-037 · P0 · 定性并恢复 obstacle 门禁

状态：已关闭。实施：benchmark `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`，CI pin `741f40a`。

依赖：OB-001。入口：固定 revision 的 companion benchmark 与 observer-intersection 对应引擎路径。
动作：核对本地历史与 CI fixture revision，在基线、候选和参考浏览器上验证 IntersectionObserver 假设，制作最小用例。
完成：给出 engine/fixture/环境归因与修复依据；有效门禁达标并可重跑。不得删除失败、修改固定答案或隐藏 skip；fixture 修订须重新跑基线与候选。

## B. 网络、运行语义与身份

### OB-010 · P0 · 生命周期不变量集合

状态：未关闭。

依赖：OB-001。入口：browser/page、JS runtime/frame、CDP 测试。
完成：明确 session、top-level context、document/realm 不同寿命；同源导航的 sessionStorage、跨标签隔离、listener/timer/Promise、iframe/Worker 所有者销毁均有回归；旧 issue 仅作为复验线索。

### OB-012 · P0 · 全项目请求统一到校准后的 primp

状态：部分完成，保持未关闭。

依赖：OB-001、OB-011，与 OB-021 协作。入口：net/client.rs、stealth_client.rs、stealth_transport.rs、JS ops/worker、page。
本轮完成：删除 `obscura-render` 的 `ureq` 依赖及同步图片下载器，默认 renderer cache 只消费 data URL、显式调用方 loader 或页面经 primp 注入的原始字节；主 workspace 与独立 runtime 锁文件均不再包含 ureq。`robots.txt` 的代码路径已改走 persona-owned primp；fixture 循环读取到完整 header block、保留原始字节，并证明 robots 与目标导航 User-Agent 一致。每个 Page 的 primp 与其 detached Worker 共享同一 transport in-flight 计数，兄弟 Page 彼此隔离；`networkidle` 统一读取本 Page 的预发送/CDP 拦截阶段与 primp 传输阶段，且同一请求的两个阶段互斥计数。native policy `RequestInterceptor` 现在收到包含任意二进制字节的完整请求体。页面和独立 `ObscuraJsRuntime` 的 fetch/XHR、CORS OPTIONS、module graph、图片、字体与样式请求均只经 primp 获取；独立 runtime 初始化时固定默认 Windows Chrome145 persona 和隔离 Cookie jar，frame/Worker 继承身份、Cookie 与策略，Worker 保持 detached primp 池。context baseline、Page override 和 request-specific headers 分层合并，兄弟 Page 保持隔离；无效代理 fail closed，PEM/DER CA、scripted 请求逐跳 timeout 与 body cap 保持生效。运行时挂接 Page 时会明确替换 standalone transport，保留 Page 的 persona、私网策略、Cookie 和回调。
本轮第二切片完成：`ObscuraHttpClient` 收敛为只承载 policy/context 的类型，删除其自有发送 backend、timeout 假配置和项目自有直接 reqwest 依赖，并从根 workspace 与独立 runtime 两个锁文件移除对应 package；`obscura-net::wreq_client` alias 一并删除。CLI 的 `original` 文件和 HTTP 辅助路径统一走 `StealthHttpClient`，原有 37 项 legacy 网络测试完整迁移到 primp。导航、表单、脚本、样式、图片/字体 warmup、页面与独立 runtime/module graph、Worker、CLI HTTP(S)、CDP/MCP Page 路径已进入 primp。Abort/XHR.abort、CORS opaque/same-origin/exposed-header/跨源重定向、最终线上请求头观察、重复或非 UTF-8 头、完整响应体持久化、CDP Fetch 覆盖、Beacon 和下载及线级校准仍未达到完成条件。这里的删除范围是项目自有直接依赖和客户端，不代表整个依赖生态绝对不含 reqwest。所有采集与日志必须保留 Cookie、Authorization、重复头、原始字节和完整 body，不做脱敏或字段裁剪；内存预算不足时应转为有界文件/流式存储，不能静默丢弃。
本轮 raw header 切片完成：`HeaderCapture`/`RawHeader` 从 primp 边界贯穿 `Response`、`RequestInfo`、JS/render/Page 到 CDP 观察面；`rawHeaders` 是 Obscura 加法扩展，格式为 `captureStage=transportRequest|transportResponse`、`encoding=base64`、`fields=[{nameBase64,valueBase64}]`。重复值、非 UTF-8、Cookie、Authorization、Set-Cookie 的原始 bytes 均不脱敏、不裁剪；兼容性的 map 只是派生视图。本轮不宣称已经取得 wire capture。Worker 观察切片已完成成功型请求的贯通：`WorkerObservations` 将 raw captures 和 body 回灌 owning Page；保留 `JsNetworkEvent` 的真实 resource type，Worker 主脚本在 CDP 标为 `Script` 且 transport `Sec-Fetch-Dest: worker`，普通 fetch 的 `Sec-Fetch-Dest` 保持 `empty`；Worker queue 字节预算计入 request/response raw header 的全部 name/value；端到端 nested Worker 回归覆盖 raw headers、body 和 `Network.getResponseBody`。本轮 Page-owned response body 切片已完成：Document、classic Script、Stylesheet、Image、Font 保存原始 bytes；默认超过 2 MiB 转为 `NamedTempFile`，Page 总预算为 256 MiB/16384 entries，预算失败显式报告并在 `clear` 前停止新增，不静默 eviction；alias 共享同一存储。invalid UTF-8 文本经 CDP 以 base64 精确返回；Fetch stream 转移 raw store，活动 stream 不被驱逐，Page clear/drop 后仍可读，`IO.close`/context drop 清理。JS fetch/XHR、module loader 和 Worker 的成功 transport 响应现在在 owning Page 下共享同一个 `ResponseBodyStore`：超过 2 MiB 进入 spool，invalid UTF-8 保留原字节，Worker 观察队列只传 metadata、不复制 body；module 仅记录最终 URL 的成功 2xx 响应，resource type 为 `Script`。该切片聚焦 release nextest 为 27/27；根 release nextest 为 1870/1870，4 skipped。`Fetch.getResponseBody` 现已读取统一 raw store，重复 get 不消费正文并与 `Network.getResponseBody` 使用同一编码；`takeResponseBodyAsStream` 之后 get 明确返回 consumed。session 严格隔离，带 session 的请求或未知 session 不会跨 Page 查找；早期无 session 多 Page 查询采用首个可读 body，已由下述 pause 隔离切片改为歧义拒绝。live request-stage 请求的 get/take 返回 `response_body_not_ready` 并保留 resolver。这是 completed-capture requestId 扩展，不是标准 response-stage interception；whole-body get 会 materialize spool，磁盘 IO 故障注入尚未单测。该 Fetch 切片聚焦 release nextest 为 16/16。后续 scripted failure observation 切片已补齐 JS/module/Worker 的 redirect、preflight/失败链与 Fulfill/CORS early-return 观察；仍待处理 transport 先完整 materialize `Vec`、JS 100 MiB 与 module 32 MiB 默认硬帽、metadata 4096 上限、独立 Worker target、`importScripts`、公开 HashMap 输入限制、其他完整响应体出口；低层 `ObscuraState` 字段也有源码兼容变化。OB-012 保持未关闭。
完成：导航/子资源/fetch/XHR/Worker/重定向/拦截改写共用一致的 URL、凭据、Cookie、CORS、代理、证书、取消及体积限制；所有项目自有 HTTP(S) 出口仅使用经修复/校准验证的 primp，包括 Beacon、预检、下载、CLI/MCP/嵌入辅助请求；删除 wreq 别名、reqwest 客户端及其他绕行后端，保留 primp 必需协议依赖与 Worker detached 池；以依赖图、调用点清单和实际线级证据证明无绕行；代理失败无未授权直连，重复头和二进制体不静默丢失。

本轮新增 synthetic `Fetch.fulfillRequest` capture：JS fetch/XHR、Worker Fulfill 响应进入 owning Page 的共享 raw store，并产生成功的 Network 事件；超过 2 MiB、invalid UTF-8 和 binary body 均保留完整原字节。pause 阶段的 `intercept-N` 会 alias 到完成 capture 的 `fetch-N`，`Network.getResponseBody`、`Fetch.getResponseBody` 和 stream 共用 consumed/budget 状态，Page 生命周期内跨导航不会复用同一 ID。CDP `responseHeaders` 的重复值、大小写和顺序，以及 `binaryResponseHeaders` 的非 UTF-8 原字节均保留，raw capture 标记为 `captureStage=cdpFulfillResponse`；它表示 synthetic response，不是 transport/wire capture。非法 body/header base64 在解除 pause 前拒绝，解析失败可重试。既有 Fulfill 的 CORS、opaque、302 语义保持不变；Fail 与 transport CORS 拒绝不报成功。聚焦 release nextest 为 9/9；最终根 release nextest 为 1876/1876，4 skipped，独立 runtime 为 185/185，三种发布构建均成功，障碍课为 33/33，官方 Playwright smoke 与 37-method 协议画像校验通过。后续 scripted failure observation 切片已补齐 preflight/redirect failure 完整生命周期；仍待真正 response-stage interception、IO 注入、transport 先完整 materialize `Vec`、JS 100 MiB 与 module 32 MiB 默认硬帽、独立 Worker target、`importScripts`、公开 HashMap 输入限制和其他完整响应体出口；新增 public enum variant 会影响 exhaustive match 的源码兼容性。OB-012 保持未关闭。

本轮 `Fetch.continueRequest.postData` 切片：主 server handler 和 `domains/fetch` 共用严格 standard-base64 解析；非法 base64、非字符串及 null 返回 `-32602`，校验完成前不取走 pause，连续错误后仍可重试。省略 `postData` 保留原 body，空字符串明确清空；`InterceptResolution::Continue.body` 与 `FetchResolution::Continue.post_data` 使用 `Option<Vec<u8>>`，直到 primp 发送均保留任意原字节，不经过文本或 lossy UTF-8，也不脱敏、不裁剪。URL、method、headers 覆盖及 SSRF、redirect、CORS、credentials 的后续处理保持原路径。新增回归覆盖两个入口和真实 HTTP fixture 下的页面 JS fetch / Worker：NUL、0xff、全部 256 种字节、空 body、未覆盖原 body、重复错误后重试，以及 URL/method/Authorization/Cookie。本轮 render/no-render focused release nextest 各为 3/3；最终根 release nextest 为 1879/1879，4 skipped；独立 runtime release nextest 为 185/185；no-default-features、render,stealth、render 三种 exact CLI release build 均成功；CI 固定 benchmark 版本的障碍课为 33/33；官方 Playwright Python 1.60.0 smoke 通过，完整协议日志通过 37-method profile 校验。公开 Continue 字段从 `Option<String>` 改为 `Option<Vec<u8>>`，嵌入调用者提供文本覆盖时需显式 `into_bytes()`。该 postData 切片当时未扩展 preflight/redirect failure 生命周期，后续 scripted failure observation 切片已补齐；response-stage interception、request header HashMap 表达能力与 wire/TLS 校准仍未完成；OB-012 保持未关闭。

本轮 active multi-Page Fetch pause 隔离切片：每个 Page 的拦截通道在 `Fetch.enable` 时绑定 owning Page/frame/session，保持客户端可见的 `intercept-N` 和 completed body alias 不变；resolver 以 `(sessionId, requestId)` 查找，不再覆盖同一连接兄弟 Page 的相同本地 ID。显式 session 必须精确拥有活动 pause，同页另一个 flattened session 也不能接管；一页只能由一个 session 启用 Fetch。单 Page 保留无 session enable/resolve/read/disable；多 Page 的 Fetch 请求以及 `Network.getResponseBody(intercept-N)` 必须提供 session，导航中暂时移出 Page 列表也不降低这个门槛。使用无 session enable 的客户端须在创建第二页前切换到具名 session；已有 null-owner pause 不允许其他 session 接管。无 session 的 Network/completed-store 查询遇到多个 retained/consumed alias 明确报歧义，不再选首个可读正文；store-wide budget failure 仅作为诊断，不冒充某个 request 的归属。get/take 的 not-ready 和 malformed base64 均保留 pause；已完成左页 alias 不被仍暂停的右页同 ID 遮挡。owner-aware disable 只继续该 Page 的请求，detach/close/dispose 清理失效 owner 并以 Aborted 结束其 pause，连接关闭含导航中请求也不会遗留 resolver；尚未发出 requestPaused 的 queued envelope、relay 发送失败/任务取消和 reply channel 关闭均显式 Aborted，不以丢弃 resolver 隐式放行。真实 CDP processor 回归覆盖两个同时活动 Page、显式 flattened sessions、页面/Worker 请求、相同 ID、错误/缺失 session、Continue/Fulfill/Fail、重试、completed alias/stream、导航和 detach 后重新 attach。Cookie、Authorization、原始 header/body 不脱敏、不裁剪。这里仍不是 response-stage interception；后续 scripted failure observation 切片已补齐 preflight/redirect failure 完整生命周期，request header HashMap 表达能力、独立 Worker target、wire/TLS 校准与 IO 故障注入仍未扩展，OB-012 保持未关闭。公开 JS/Page interception API 未改；低层 `CdpContext.intercept_tx` 改传带归属的 CDP envelope。本切片最终 release nextest：render CDP lib **183/183**；no-render focused **18/18**（含并发、生命周期、queued disconnect、raw body 和 retry 回归）；`git diff --check` 通过。本切片最终完整门禁：根 release nextest **1886/1886**，4 skipped；独立 runtime **185/185**；no-default-features、render,stealth、render 三种 exact CLI release build 均成功；CI 固定 benchmark 障碍课 **33/33**；官方 Playwright Python 1.60.0 smoke 通过，完整协议日志通过 37-method profile 校验。


本轮 `Fetch.continueRequest.headers` 切片：两个 CDP 入口在消费 pause 前严格校验 HeaderEntry 数组、字符串 name/value 和 HTTP field 语法；非法输入返回 -32602，同一请求可连续纠错重试。有序列表保留重复 name、输入大小写、数组顺序及 Cookie/Authorization/空值/Unicode 的完整值，直到 HTTP HeaderMap 边界，不再经发送用 HashMap 压扁。request fields 按不区分大小写覆盖 context/Page baseline；native policy 的 map 修改仅替换指定 name，其余重复字段保留，policy 观察可通过 raw_headers 的 requestPolicy 阶段读取完整字段。CORS 使用单独的合并视图分类重复字段，原 URL/method/body、SSRF、redirect、credentials 和 Page/Worker/session 归属路径保留。既有 browser-owned Origin、Referer、Sec-* 仍由原 persona/request policy 生成；Cookie jar 仍在允许 credentials 时先追加，然后追加显式 Cookie。HTTP HeaderMap 会小写化名称、按名称分组；persona/primp 决定跨名称顺序，并可注入 Host/Content-Length 等字段或采用 HTTP/2 编码。因此仅承诺 CDP 列表和同名值的保留，不宣称 wire casing、全局 wire order 或 framing。公开 InterceptResolution::Continue 原字段和构造方式保持；新增 ContinueWithHeaders variant 会影响 exhaustive match，低层 CDP FetchResolution::Continue.headers 改为 Option<Vec<(String, String)>>。本切片没有解除其他 JS/embedded HashMap 输入限制；后续 scripted failure observation 切片已补齐 preflight/redirect failure 生命周期，response-stage interception、独立 Worker target 与 wire/TLS 校准仍未完成；OB-012 保持未关闭。本切片验证：render/no-render focused release nextest 各 **15/15**；最终根 release nextest **1889/1889**，4 skipped；独立 runtime **185/185**；no-default-features、render,stealth、render 三种 exact CLI release build 均成功；CI 固定 benchmark 障碍课 **33/33**；官方 Playwright Python 1.60.0 smoke 通过，完整协议日志通过 37-method profile 校验；`git diff --check` 通过。

本轮 scripted failure observation 切片：fetch/XHR、Worker 和 module 请求在开始时分配 `fetch-N`；Fetch pause 继续使用独立的 `intercept-N`，`networkId` 关联同一 Network 请求，完成后 alias 共用原有 body store。失败记录包含完整 URL/method、阶段明确的原始 request/response headers、请求 body 字节数和错误原因；Cookie/Authorization 不脱敏。preflight 使用独立 request ID/type，并关联发起请求，拒绝时不会把 OPTIONS 响应体作为主请求正文。redirect 每个实际发送 hop 均可独立 request-stage pause/Continue/Fulfill/Fail，拥有新 `intercept-N` 并保持同一 logical `fetch-N`；中间响应通过后续 `requestWillBeSent.redirectResponse` 表达，单独的 `bodyRequestId` 定位其完整正文，读取仍受 body store budget/consumed 状态约束，预算失败返回明确错误；最终请求才产生 `loadingFinished` 或 `loadingFailed`。发送前拒绝仅有 request/failed；已收到真实响应后发生 CORS、redirect policy、body cap 或读体错误时保留合法获得的 headers，完整读完的响应正文进入统一 spool，未完成或超限正文不会伪装成成功 body。早期 pause/preflight 的 script headers 与最终 `transportRequest` 不混淆，后者通过 `Network.requestWillBeSentExtraInfo.rawHeaders` 完整提供。CORS/opaque 对页面 JS 的可见性保持，CDP 是有权限的诊断观察面，不能据其可读正文宣称页面脚本可读。fetch AbortSignal、XHR abort/timeout 可取消原生 future；关闭或销毁 runtime/Worker 的 terminal 通过 owning Page 共享失败队列与 Notify 送达，document generation 固定其原 loader，跨导航不复用请求 ID；generation→loader 映射保留到 Page close，导航次数带来少量线性状态，以保留晚到 Worker terminal 的归属。真实 pause 记录 owner session，终态不会转发给同 Page 的其他 flattened session。已取消的 Fetch pause 拒绝迟到的 Continue/Fulfill/Fail。redirect 后需要 preflight 时，会在预检 yield 前按序发布已完成 hop 与下一 hop start，后者携带完整 `redirectResponse`；redirect pause 等待期间保持 network-active，避免 `networkidle` 提前。XHR `open()` 会取消旧 controller/timer，并用 request generation 阻止旧 Promise 回写；关闭的 routed pause 只有真正发出后才登记 owner，不会在 terminal 后恢复陈旧归属。只有真实 transport response 已存在时才发送 `responseReceived`，随后失败只发送 `loadingFailed`，绝不同时发送 `loadingFinished`；URL/SSRF/policy/DNS/connect/TLS 等发送前失败不会伪造 response。body cap/read failure 可保留真实 headers 与已知长度，partial/超限 body 不作为完整正文。这里只扩展脚本请求的观察生命周期，不是真正 response-stage interception，也不证明 wire casing/order、TLS、独立 Worker target 或 `importScripts` 完整支持。transport 硬帽、body store budget、metadata 4096 上限仍存在，OB-012 保持未关闭。公开 `InterceptedRequest`、`JsNetworkEvent`、`NetworkEvent` 增加观察字段，直接构造这些低层类型的调用方需适配。

本切片最终验证：render focused release nextest **279/279**，509 skipped；no-render focused **103/103**，565 skipped；review-combination **7/7**；根 release nextest **1910/1910**，4 skipped。no-default-features、render,stealth、render 三种 exact CLI release build 均成功；复用冻结的 render 二进制完成障碍课 **33/33**、官方 Playwright Python **1.60.0** smoke、37-method 协议画像校验和 redirect route E2E。route E2E 确认两条 redirect 第二跳均在 owner session pause 并由 Playwright 自动 continue；成功链保持同一 Network ID，302→200 后仅 `loadingFinished`，连接失败链仅 `loadingFailed`，`networkidle` 晚于最终响应且页面 `done=true`。冻结二进制 SHA-256 为 `5912bcccb186f926010c5e7c118763a903d8e89dccc295c8e7ade2f02407087c`（118958752 bytes）。根门禁已经覆盖本轮 runtime 改动，故未为相同源码额外重复运行独立 runtime 全套；`git diff --check` 通过。

本轮 response-stage interception 最小切片：`Fetch.enable.patterns[].requestStage=Response` 已接入 scripted `op_fetch_url` 路径，覆盖页面 fetch/XHR、Dedicated Worker 内的 fetch/XHR、经 scripted helper 发起的动态 classic script/stylesheet 与 CORS OPTIONS；URL 与 `resourceType` 会共同匹配，XHR、Fetch、Script、Preflight 按实际路径上报。每个已完整读取且成功进入 body store 的 transport redirect/final hop 都先将原始 body 放入 owning Page 的共享 store，再向 owner session 发送 `Fetch.requestPaused`。事件提供 `responseStatusCode`、兼容 header 列表、完整 `responseRawHeaders`、`networkId` 和同阶段 redirect 的 `redirectedRequestId`；兼容 `responseHeaders` 对非 UTF-8 原始字节使用一字节一字符投影，不丢字段，base64 编码的 `responseRawHeaders` 保留权威原值；Cookie、Authorization、重复头、非 UTF-8 头值及 body 不脱敏。transport 尚未采集原始 HTTP reason phrase，因此 `Fetch.requestPaused`、redirectResponse 和页面 JS 的 statusText 明确为空；`continueResponse`/`fulfillRequest` 提供的非空 `responsePhrase` 只贯通到后续 Network 响应事件，不回填已发送事件或页面 JS。body cap、读体错误或 store budget 失败不会伪造完整 response-stage pause。活动 final response pause 可选择重复 `Fetch.getResponseBody` 或一次 `takeResponseBodyAsStream`，两者互斥；Fetch stream 只允许 owner session 顺序读取且拒绝 offset，Page 退出后 owner 仍可读至关闭。redirect response 的这两个 Fetch body API 明确拒绝；stream 已转移后只允许 Fail/Fulfill。response-stage 可用空 `continueRequest` 或无覆盖 `continueResponse` 原样继续；响应覆盖要求 responseCode 与 headers 同时提供，responsePhrase 可省略。Fulfill 未提供 body 时保留原 body，替换后更新共享 alias，同时已打开 stream 继续持有旧 body。disable/detach/connection close 仍按 owner 生命周期解除，导航期间 body store 可读且 disable 会清理返回 Page 的旧策略。当前范围不是全浏览器 response-stage：Document navigation、独立 module loader，以及 Page/native Stylesheet、Image、Font 与 renderer resource transport 尚未接入；动态 classic script/stylesheet helper 仅在其实际经过 scripted `op_fetch_url` 时覆盖；`handleAuthRequests`/`continueWithAuth` 也未实现。OB-012 保持未关闭。

本切片最终验证：根 release nextest **1916/1916**，4 skipped；sessionless/navigation 聚焦回归 **1/1**，no-render response-stage 与 sessionless/navigation 回归 **2/2**。render 与 no-default-features 两种 exact CLI release build 均成功；CI 固定 benchmark 障碍课 **33/33**；官方 Playwright Python **1.60.0** smoke 与 **37-method** 协议画像校验通过。上述端到端验证共用冻结 render 二进制，SHA-256 为 `c4eff66c2da49c42e17eef91f41d52081b86cbfc7a2b6a4591718a432bad770b`（119043264 bytes）。Astra light 最终复核为 0 blockers；`git diff --check` 通过。

### OB-013 · P0 · 安全、终止和资源预算

状态：未关闭。

依赖：OB-001，与 OB-034 协作。入口：watchdog、ops、DOM tree、Worker queue、host。
完成：保留 panic=unwind、DOM 环防护、SSRF 与 V8 终止；覆盖无限脚本、卡住原生操作、超限内存/响应体/线程/FD、取消；分别定义进程内保护与 OS 隔离，不承诺“Rust 自动安全”。内核故障不能当成功 null。

### OB-030 · P0 · CDP 观察副作用

状态：未关闭。

依赖：OB-027、OB-029。入口：Runtime/getProperties/console、网络与诊断订阅。
现状：本次 `Runtime.getProperties` 使可枚举 getter 的计数从 0 变 1。
完成：开关观察前后业务结果一致；getter/proxy、异常、console 对象、网络订阅有边界测试；不因检查对象而任意执行 getter，不把 Worker 对象句柄当作父 isolate 句柄。

### OB-014 · P0 · 固定 macOS/Linux 参考 persona

状态：未关闭。

依赖：OB-001、OB-009。入口：参考采集、现有 persona/transport 配置。
现状：当前 primp presets 为 Windows145、macOS152/153，没有已认证 Linux persona。
完成：每个平台至少一个同 OS 的完整参考清单与版本；记录网页/HTTP/图形/字体/时区/硬件表面和支持限制；不复制宿主不支持的 Windows/移动身份进入首期资格。

### OB-015 · P0 · Persona 编译与加载校验

状态：未关闭。

本轮实现：`obscura-net` 提供版本化 `PersonaSpec`、内置预设、外部 JSON、统一字段/能力校验、不可变 `EffectivePersona` 和稳定摘要；产品入口缺少 persona 时 fail closed。OB-014 的跨平台参考资格尚未完成，因此本项不因编译器存在而关闭。

依赖：OB-014。入口：统一 persona 模块、开发侧 compile-persona、共享配置加载。
完成：建立唯一权威 persona 模块，迁入仍有价值的 runtime Persona/CLI profile 逻辑，消除平行身份配置；PersonaSpec 到 EffectivePersona 的字段关系、schema、摘要和能力校验可复现；生产仅轻量加载校验；缺失或矛盾配置明确拒绝，不默默选另一个 profile。

### OB-016 · P0 · Persona 原生一致投影

状态：未关闭。

本轮实现：CLI、CDP、MCP、Rust facade、scrape worker、页面、frame、Worker、module loader、网络和存续独立 runtime 均消费同一快照；时区与 ICU 主语言按进程冻结，冲突 context 在登记前拒绝。具名 profile 仍不等于完整 wire、字体、图形或跨平台资格。

依赖：OB-012、OB-015。入口：页面/frame/Worker 配置、net、CDP Browser/Emulation。
完成：CLI/MCP/CDP/Rust 嵌入均消费同一 EffectivePersona；JS、HTTP、TLS/H2、时区/屏幕、frame/Worker 与 transport 声明来自同一有效值集；CDP 固定版本返回与有效配置不再各自漂移；未做到的 wire 差异显式列出，不声称仅凭名称完全匹配 Chrome。

### OB-031 · P0 · 客户端 override 冲突规则

状态：未关闭。

本轮实现：继续拒绝 UA/身份 header override；`Target.createBrowserContext.obscuraPersona` 只能注入与启动进程时区和主语言兼容的快照，其他 context-local 字段可隔离；`Browser.getPersona` 返回实际 persona 摘要。仍需纳入完整客户端 connection options 资格清单。

依赖：OB-027、OB-015，与 OB-016 协作。入口：CDP Network/Emulation/BrowserContext。
完成：locale/timezone/UA/viewport 等实际客户端参数可验证应用或明确拒绝；页面/frame/Worker/HTTP 保持一致；认证清单记录 connection options。

### OB-038 · P0 · 保留并完善 Worker/frame 资格

状态：未关闭。

依赖：OB-001、OB-010；正式客户端覆盖依赖 OB-028/029。入口：JS worker/worker_queue/frame/runtime/bootstrap、browser/page。
现状：Worker 独立线程/isolate、clone/transfer/终止与两后端 detached 池已经存在，本次根测试覆盖现有回归。
完成：独立执行、clone/transfer、终止、策略更新、队列预算、相对 URL、断开/重插入、旧文档引用与过期任务取消均有固定 fixture；module Worker、MessagePort/host-object clone、srcdoc/sandbox/WindowProxy 等逐项标明支持范围。不得重做已存在的 isolate 分离。

### OB-039 · P1 · Beacon 与其他 Web API 占位行为

状态：未关闭。

依赖：OB-012、OB-038。入口：JS bootstrap/ops 与公共网络路径。
现状：本地服务端收到 0 个 Beacon POST，页面返回 true；与两处直接 `return true` 的源码一致。
完成：先审计能力清单、再用通用 fixture 补语义；Beacon 至少验证排队返回、body/content-type、凭据、取消/生命周期及出口策略；未实现不伪造成功。任何 API 修复均不能直接宣称解决真实站点 403。

## C. 迁移、裁剪与开发工具

### OB-002 · P0 · 依赖与构建成本基线

状态：未关闭。

依赖：OB-001。入口：Cargo metadata/tree、构建/发布配置。
完成：记录编译可达依赖、冷/热构建时间、磁盘、产物、两个 workspace 成本；区分生产、开发、冻结和待删除，不只比较可执行文件大小。

### OB-005 · P0 · 通用启动与初始化契约

状态：未关闭。

依赖：OB-001、OB-025。入口：CLI/runtime 的配置、初始化、信号与清理代码。
完成：网络/存储/persona/预算在首个页面和首个请求前生效；审计 V8、TZ、日志等进程级初始化的一次性与冲突行为；就绪、错误、退出码、信号与资源回收有结构化契约；不引入业务状态机。

### OB-008 · P0 · 最小 Unblocked 开发工程

状态：未关闭。固定 fixture、首批记录/差分、结果清单和独立依赖锁已由 `741f40a`、`fc65daa` 分步交付；跨平台执行与后续资格入口仍未完成。

依赖：OB-001，可与 OB-026 同步。入口：tools/unblocked/（已有基线、客户端范围、独立锁、固定 fixture、记录/差分、结果清单和校验器；跨平台资格待交付）。
完成：先交付固定 fixture、记录/差分、结果清单与独立依赖锁；生产无 Hero、参考 Chrome、实验室服务或 Python 测试依赖；不一次建设大型实验平台。

### OB-009 · P0 · Chrome/upstream/fork 对照 driver

状态：未关闭。

依赖：OB-008、OB-025。入口：开发侧 runner。
完成：同一测试接口、固定版本与输入；区分 Chrome 正常启动/CDP 连接；每组记录控制方式和采集扰动；尚未跑的组合明确 not-run。

### OB-004 · P0 · 保留并收敛有用 CLI

状态：未关闭。

依赖：OB-005、OB-021，与 OB-012/044 协作。入口：crates/obscura-cli、CDP 启动、runtime 引导。
动作：保留 serve、诊断、截图和结构化输出，服务 agent 接入；逐项评估 scrape/worker 包装的用途、消费者与成本，不预设全删，也不强制新建 obscura-host。
完成：入口共享初始化、persona、primp、预算和回收；结构化成功/错误、超时与退出码可供 agent 使用；无第二套页面状态机，旧私有 RPC 不回流。

### OB-003 · P1 · 保留 MCP 并复用统一内核

状态：未关闭。

依赖：OB-005、OB-021，与 OB-004/012/044 协作。入口：crates/obscura-mcp、workspace、CLI、文档/发布。
完成：MCP 保留在产品范围；工具复用共享浏览器执行能力、persona 与 primp；有连接/工具调用、错误/取消、隔离/清理及有用输出的 agent smoke。不能为 MCP 再维护一套网络、身份或页面状态机；现有工具按实际能力声明。

### OB-006 · P0 · 清理自有 Python SDK 与配套私有 runtime

状态：未关闭。

2026-09-20 前置修复（Owner: Codex，基线 `9eccdc7`）：工具子进程 stdout/stderr 原始字节保留（`cf258a2`）和历史 source lock 摘要校验（`9eccdc7`）已完成，该批实际删除 SDK/runtime 文件 **0 个**。

首批回归迁移（Owner: Codex，实施基线 `f09e61d`）：13 个 Page-only 原生输入/几何回归已迁入 `crates/obscura-browser/tests/native_input.rs`，定向 **13/13** 通过后，从 `runtime/src/browser.rs` 删除对应旧测试及属性，共 524 行。逐函数复核保留 fixture、行为和像素断言，显式 Persona 与 RGBA8 检查不变；只移除该组私有截图 writer 调用，末项 `sdk_inline_link_after_button_has_geometry` 改为 `inline_link_after_button_has_native_input_geometry`。其他 runtime 测试仍使用的 helpers 保留。根 release/render 门禁 **1957/1957**（4 skipped），独立 runtime **172/172**。本批未删除完整文件或生产实现，`bindings/python/` 与私有 `runtime/` 仍存在；后续继续迁移有用回归并实际移除私有包装，保持强制且不可变的 Persona 契约。迁移测试不代表 CDP 已接入共享 native input。

剩余共享回归迁移（Owner: Codex，实施基线 `d84b343`）：162 个 browser 测试按 8 个行为模块迁入根 `native_platform` 集成测试，8 个 referrer 测试迁入根 `referrer_policy`；定向 **170/170** 通过后，删除旧 browser 测试模块 **5486 行**、旧 `runtime/src/referrer_tests.rs` **394 行**及 `main.rs` 两行测试模块声明。保留全部生产定义；私有 runtime 剩余两个包装测试通过。官方 smoke 新增空/空白 aria-label 名称回退与点击、同 context 导航/跨 tab 的 localStorage 共享和 sessionStorage 隔离回归。根全量复跑 **2127/2127**（4 skipped）；首轮 module-budget 用例失败及其未改代码的定向重放通过均保留为证据，不宣称时序问题已修复。

取消语义核验（2026-09-20）：官方 Playwright Python 的 `asyncio.Task.cancel()` 仅停止本地等待。实测取消缺失 locator 后插入同 selector，服务器副作用计数从 0 变为 1；不能将 `CancelledError` 当作远端动作撤销。此负面证据与 `client-scope.json` 的 indeterminate-write 边界一致，旧 SDK 取消时杀自有进程的策略不迁回私有包装。迁移门禁分别验证本地取消后不自动重放、官方 action timeout 和显式 page close 的停止边界、已执行 click 后 response timeout 仍恰一次；不以弱化计数断言掩盖差异。

实际删除批次（Owner: Codex，实施基线 `ca40d0b`）：官方客户端三项迁移门禁通过，生命周期计数为 task-cancel=1、official-timeout=0、page-close=0、sibling-control=1、post-dispatch=1；显式 2000ms watchdog 后原 page 恢复，五项 dynamic-script CORS 符合矩阵。移除 `runtime/`、`bindings/python/`、`examples/zg/` 共 27 个旧路径，完整清单见 SUMMARY。旧构建脚本中唯一的字体成员/声明/bytes/SHA-256 门禁迁入 `obscura-render/build.rs`，一项正常和五项篡改实测符合预期；不保留私有进程协议。冻结 baseline 未改，历史 runtime toolchain 与两份 lock 从 source Git blob 校验。最终构建与全量门禁确认前保持未关闭。

依赖：OB-021、OB-005、OB-025；按调用点完成 OB-027/028/029 的替代 smoke。现行入口：根 browser 回归、`tools/unblocked/migration_smoke.py`、render 字体构建门禁；私有 NDJSON/RPC 与专属 examples 仅保留 Git 历史。
动作：完成删除批次的最终审核与验证；共享 native input 的 CDP 接线及完整 world/句柄资格仍由 OB-021/027/028/029 验收，不将它们混同于私有包装删除。
完成：官方 Playwright Python/CDP 接替需要保留的调用，产品与发布不再依赖自有 SDK/IPC；根测试承接有用回归后移除独立 workspace/锁文件/专属 CI。保留 V8/JS runtime、Web Worker、MCP 和有用 CLI；不以私有 runtime 旧测试总数阻止删除。

### OB-007 · P0 · 删除非目标平台产品配置

状态：未关闭。

依赖：OB-014、OB-016、OB-002。入口：发布 workflow、安装脚本、persona、示例。
完成：首期仅发布已认证 macOS/Linux 组合；清理自有 Windows/移动产品入口与 CI，不盲改第三方 vendor 的通用平台代码，不删除规范引用。

### OB-036 · P1 · 裁剪结果与旧代码迁移审计

状态：未关闭。

依赖：OB-003/004/006/007、OB-012/044、OB-032。入口：依赖图、发布清单、文档、正式 Python 场景。
完成：确认代码、依赖、测试、产物四个维度均符合边界；正式主链完全不依赖旧 SDK/IPC；保留冻结项列出用途、风险、责任人与删除条件。

## D. 完整能力、资格与发布

### OB-017 · P1 · 原生输入、事件与用户激活

状态：未关闭。

依赖：OB-021、OB-029。入口：Input、JS 事件与 native bridge。
完成：click/fill/select/check、中文及组合输入、焦点/滚动/命中、事件顺序与用户激活通过通用 fixture；JS 合成事件不冒充真实输入；role/label/name 由官方客户端路径验收。

### OB-018 · P1 · 布局、字体与截图资格

状态：未关闭。

依赖：OB-014、OB-017。入口：render、render-repros。
完成：固定字体、viewport、scale、settle 的几何/命中/文字/裁剪测试；截图非空与正确加载先验证；必要 canvas/SVG 场景有证据；像素距离不单独决定正确性。

### OB-019 · P1 · Storage 与状态导出

状态：未关闭。

依赖：OB-010、OB-011、OB-028。入口：BrowserContext、JS storage、CDP Storage/Network。
现状：`--storage-dir` 只保存 Cookie，双进程本地 fixture 第二次读 localStorage 得到 null。
完成：覆盖 sessionStorage/localStorage 的 origin/top-level context/导航寿命及状态往返、明确磁盘持久化范围；不能把恢复 Cookie 当作恢复整个活跃页面；未支持的 IndexedDB/分区能力显式限制。

### OB-020 · P1 · 隐私策略与可观察一致性

状态：未关闭。

依赖：OB-012、OB-016、OB-019。入口：ResourcePolicy、存储/网络配置。
完成：明确跨站、跨会话、跨独立身份空间的关联威胁和可测试边界；不以设置 DNT=1 作为反追踪完成。覆盖 Cookie/storage、缓存验证器、TLS 会话/连接及高熵信号；使用受控服务端尝试重新关联并报告结果。策略显式、版本化，可解释被阻断资源与失败；不破坏必要 Web 语义或静默改变 Cookie/请求；控制页面、Worker 和第三方资源的出口观察均有测试。

### OB-032 · P1 · 未修改官方客户端端到端资格

状态：未关闭。

依赖：OB-028/029/030/031/034，按支持范围纳入 OB-017/018/019。入口：开发侧 Python 场景。
完成：必需 API 清单全部有通过证据，覆盖多页面、frame、locator、网络、上传/下载、取消、关闭；结果绑定 engine/persona/profile/client/driver/平台，不以 SDK 通过率替代。

### OB-033 · P1 · 客户端网络与文件边界

状态：未关闭。

依赖：OB-025、OB-012、OB-034。入口：正式客户端场景、host 网络/文件策略。
完成：APIRequestContext/route.fetch 属 driver 网络，首期不作为受保护站点请求路径认证，迁移到页面请求/内核拦截履约；若保留该用途则须先实现并证明统一 primp 出口，不能仅配置相同代理就宣称一致。下载/上传/截图等逐项定义执行位置与支持边界；出站代理覆盖包括 driver/辅助请求；文件路径授权、大小限制、取消和清理可验证。

### OB-022 · P1 · 构建与测试成本回归

状态：未关闭。

依赖：OB-002、OB-004。入口：CI 与开发基准。
完成：比较冷/热构建、依赖数、缓存/磁盘及测试时长；基线/候选相同工具链和配置；报告波动，不把一次样本当提升。

### OB-023 · P1 · 双平台长稳与整链性能

状态：未关闭。

依赖：OB-032、OB-037、OB-038。入口：开发基准和受控运行环境。
完成：以相同正确 RPA 场景测宿主+内核+Python driver 总资源，与固定 Chrome 交替对照；性能突出是产品目标，不能仅以接口跑通验收；交替比较启动/动作 p50/p95、RSS、线程/FD、空闲 CPU、多轮 churn 与超时回收；不与构建混跑，不用 0 等待掩盖未加载。

### OB-035 · P1 · 协议升级与客户端回归

状态：未关闭。

依赖：OB-026、OB-027、OB-032。入口：协议/profile/客户端版本清单。
完成：升级 Playwright/driver/schema 时能列出方法参数和事件差分，跑现版与候选版矩阵；只认证实测版本，失败有回滚路径。

### OB-024 · P1 · 发布、供应链与上游同步

状态：未关闭。

依赖：OB-013、OB-023、OB-035、OB-036、OB-044/045/046。入口：release workflow、依赖审查、安装与发行说明。
完成：保留可信 PR 隔离措施；产物绑定已验证 SHA/平台/功能、校验和、许可证与依赖清单，验证全新机器安装；公开限制/回滚办法；上游合并必须重跑本项目资格，不能默认兼容。

### OB-040 · P0 · 证据、CI 与文档事实统一

状态：未关闭。

依赖：OB-001；随各阶段持续更新。入口：AGENTS/README/docs/SUMMARY、CI/发布 workflow、测试清单。
文档部分：本次已统一根本目标、现状/设计/历史的边界并去除重复流水账；CI/机器可读资格部分尚未完成。
完成：区分历史手工记录、当次本地结果、CI 结果和发布资格；消除失效“未提交改动”指引及无条件 drop-in 宣称；补独立 runtime 迁移期检查与正式客户端/macOS 必需检查；文档 PR 不充当引擎绿灯。删除路线完成后再移除其专属 CI/文档。

### OB-041 · P0 · IndexedDB 正确性与生命周期

状态：未关闭。依赖：OB-001、OB-010；与 OB-019 联动。入口：bootstrap 的 IDBTransaction/IDBDatabase/IDBFactory、JS runtime tests。
现状：请求事件、升级事务、索引和复合键游标已有实现；`abort()`/`commit()` 仍为空，数据库为 realm 内 JS 状态。不要把 Airship 8/8 历史资源数写成完整 IndexedDB 支持。
完成：合成 fixture 验证 abort 回滚、readonly/active transaction、完成/错误顺序、unique/key/cursor 边界、同 origin 跨 realm 共享与导航/持久性；明确未支持范围并与固定 Chrome 对照。

### OB-042 · P1 · 容器产物与功能声明对齐

状态：未关闭。依赖：OB-001、OB-002，随 OB-024/004 更新。入口：Dockerfile、workspace patches、release。
现状：Dockerfile 已复制 vendor 并构建 render；OB-044 后普通构建已无条件包含 primp，文档示例不再要求额外开关。本次仍未执行 Docker 干净构建，也未认证 upstream registry 镜像。
完成：固定源码/基础镜像与实际依赖，干净构建、非 root 启动、强制 stealth/唯一 primp 出口、必要功能/证书/持久化 smoke 通过，发布清单与 fork SHA 一致；不能用 upstream 镜像代替。

### OB-043 · P1 · 服务端身份关联与报价影响的验收边界

状态：未关闭。依赖：OB-012、OB-016、OB-020、OB-023、OB-032。入口：开发侧受控服务端、persona/隔离 fixture、使用方授权的采购验收。
完成：分别给出 RPA 效率、Chrome 行为/指纹差异、服务端跨身份空间标记/重新关联结果和兼容性代价。受控检查网络头/TLS/H2、存储/缓存/连接残留、时序与多 realm 一致性；不以 DNT、字段数或单个 JA4 分数代替。
报价对照须固定产品、时点/库存、渠道、币种、网络等条件，身份变量单独控制；内核提供隔离和完整原始证据，采购业务逻辑不进入内核。开发工具不做脱敏或字段裁剪，日志由执行方保管和处理。未做受控报价实验不能声称已消除价格歧视。

### OB-044 · P0 · stealth 成为不可关闭的底层能力

状态：部分完成，保持未关闭。依赖：OB-005、OB-012、OB-015/016。入口：Cargo features、CLI、配置分支、MCP/嵌入与 Docker/发布。
本轮完成：删除 CLI/serve 的 `--stealth`、`--user-agent`、`OBSCURA_STEALTH` 和嵌入 API 的运行时布尔开关；所有 Page、CDP、MCP、scrape worker 与 CLI 网络页面默认构造 primp，JS fetch/XHR、导航、表单、脚本、样式与渲染资源沿用该传输；普通与 no-default-features 发布构建都无条件编入 primp。旧 Cargo `stealth` feature 只保留为空兼容别名，不能关闭行为；CI/发布矩阵不再生成有无 stealth 的产品组合。新增回归验证旧 CLI 参数不存在、旧 Rust context bool 不能关闭能力、嵌入 API 默认 primp，并补齐 primp 子资源缓存合并、JS 网络事件/响应体记录和二进制正文无损 Base64 返回。persona 在 BrowserContext 初始化时同时确定 primp 传输配置与 JavaScript 身份，并在 context 生命周期内保持不变；运行中的 CDP User-Agent 覆盖明确不支持。
本轮证据：根 release nextest **1817/1817**，独立 runtime **185/185**，render 与 no-default-features 两种 release CLI 构建均通过，旧 `stealth` feature 兼容编译通过，obstacle course **33/33**（含 `observer-intersection`）。Docker 干净构建及 OB-012/015/016 要求的全出口盘点和完整 persona 编译器尚未完成，因此本项不关闭。
完成：移除运行时 stealth 开关与可绕过保护的产品编译路径，所有生产入口自动使用统一 persona 和 primp；旧开关仅可短期弃用兼容，不影响行为。无参数启动、各入口、容器与嵌入均有一致性验证；不能编出缺基线保护却正常发布的产物。隐私策略的明确例外不允许恢复其他传输或关闭身份一致性。

### OB-045 · P0 · Chrome 行为差异清单与修补闭环

状态：未关闭。依赖：OB-026、OB-009，随网络/persona/Web API 修复持续执行。
完成：固定参考 Chrome，收集服务端 wire 与页面/realm/事件/存储/输入差异，逐项记录影响、最小 fixture、归属任务和修复前后证据。已确认的 Cookie/world/getter/Beacon/IndexedDB 等进入现有任务，不重复立项；影响 RPA、身份标记或 shopping 的差异优先修补。每个关闭项保持通用回归，无 hostname 特判；有意隐私差异须解释并测试，不能掩盖实现缺陷。

### OB-046 · P0 · Southwest shopping 不再 403 的业务验收

状态：未关闭。依赖：OB-012、OB-016、OB-032、OB-044/045；按复现纳入 Cookie、Worker/frame、Beacon、IndexedDB 等对应修复。入口：[Southwest 验收说明](Southwest-handoff.md)、使用方授权查询、开发侧完整采集。
完成：同版本/同 OS 对照与同等查询、出口、时段条件下，Chrome 成功，Obscura 自建会话的 shopping 不再 403，响应不是挑战页/软错误且返回有效航班/报价，使用方能消费结果。覆盖新会话与复用会话；执行前固定查询集、轮数、间隔和持续观察窗口，逐轮保留结果，不能凭偶发 200 关闭。
证据：engine/persona/client/网络条件、查询摘要、状态及错误码、业务结果校验、受控 Chrome 对照、失败/重试全记录；不借用 Chrome Cookie/token，不用模拟/缓存替代真实结果，不执行购票付款。403 重现则保持未关闭并缩减通用差异 fixture。
此项是项目的重要业务门槛；通过证明该受测流程达到预期，不能单独推导全站/所有身份均无法标记或已消除价格歧视。通用一致性、隐私及性能门禁同时保留。

## 统一关闭标准

任务关闭必须记录以下内容，不接受只写“已实现”“测试通过”或“网页能打开”：

```text
Task / Owner / implementation SHA:
Baseline / toolchain / feature / platform / client-driver:
Fixture and expected behavior:
Before / after result, exact commands:
Related regression results and skipped/not-run reasons:
Resource and security impact:
Capability/profile/documentation changes:
Evidence location and access/retention handling (raw credentials and session data permitted):
```

源码、当次实验与历史结论的边界统一见 [SUMMARY](SUMMARY.md)。最终完成定义是支持清单内的能力可重现通过，而不是实现了多少方法、删了多少行代码或某个网站偶然成功。

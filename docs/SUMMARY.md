# 项目现状与文档核验摘要

首批共享输入回归迁移（2026-09-20，实施基线 `f09e61d`，macOS arm64）：13 个 Page-only hit-test、mouse/pointer、stacking/clip/geometry 回归迁入 `crates/obscura-browser/tests/native_input.rs`。定向 **13/13** 通过后，删除 `runtime/src/browser.rs` 中对应 13 个旧函数及测试属性，共 524 行；仍被其他测试使用的 fixture/pixel/evidence helpers 保留。除移除本组私有截图 writer 调用、末项去掉 SDK 名称外，逐函数比较确认原行为断言等价；像素解码继续要求 RGBA8，Persona 仍显式初始化。本批未删除完整文件或生产实现，自有 Python SDK 和私有 runtime 尚未移除，OB-006 保持未关闭，也不代表 CDP native-input 接线完成。

该批验证：根 release/render nextest **1957/1957**（4 skipped），独立 runtime **172/172**，exact render CLI release build、固定 benchmark 障碍课 **33/33**、官方 Playwright smoke 与 37-method 画像均通过；冻结 baseline 校验继续通过。render 二进制 SHA-256 为 `1dfeb942c95edc935911b4ec48bb087e07f1c5d6174c8b09075460fd0dd5c406`，与迁移前相同。Astra light Spec 审核无发现；Standards 无硬性违规，仅对局部 fixture 重复和测试文件规模提出非阻断建议。未修改 renderer 或身份/传输实现，不将本批结果扩展为新增产品能力或跨平台资格。

工具采集与冻结基线修复（2026-09-20，组合 HEAD `9eccdc7`，macOS arm64）：`cf258a2` 保留外部浏览器进程完整二进制 stdout/stderr 及失败元数据；`9eccdc7` 按 `baseline.source.revision` 的历史 Git blob 校验两份 lockfile 摘要，当前工具链和 CI 约束保持独立，历史记录未改动。组合包含 Persona 提交 `5b01925`。根 release/render nextest **1944/1944**（4 skipped），no-render 网络与 CLI 回归 **227/227**，两种 exact CLI release build、固定 benchmark 障碍课 **33/33**、官方 Playwright smoke/37-method 画像、Python **38/38** 均通过。真实失败进程退出码 23 时，含 NUL stdout 和非 UTF-8 stderr 逐字节保留。Standards 与 Astra light Spec 最终审核均无发现。此轮 SDK/runtime 实际删除文件 **0 个**，OB-006 保持未关闭；首批 13 个共享输入回归迁移及旧副本删除尚待执行。

Cookie 排序切片验证（基线 `8ddaa15`，macOS arm64，Owner: Codex）：根 release nextest **1937/1937**，4 skipped；Cookie 与真实 primp 请求回归在 render/no-render 均为 **62/62**。两种 exact CLI release build 成功，CI 固定 benchmark `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 障碍课 **33/33**，官方 Playwright Python 1.60.0 smoke 与 37-method 画像通过。相同本地 HTTP fixture 设置多 Path Cookie 后，旧二进制四个独立恢复进程均发出错误顺序，候选四次均为 `first=updated; session=scoped; session=root`。候选二进制 SHA-256：`63e87004d15d7ca720fdae41dca8b86da269c36bf4b139ff44e6d4681c1c7deb`。Astra light Spec 审查 0 blockers，Standards 无硬性违规；测试中直接修改过期状态仅为非阻塞可读性建议。完整原始日志保存在执行机 `/tmp/ob011-order-*.log`，CLI 原始请求及进程输出在执行机临时目录 `ob011-order-baseline-yc5vzxtd` 与 `ob011-order-candidate-spd84a6i`，均不进入产品或仓库。该结果不代表完整 SameSite、分区或跨平台资格。

核验日期：2026-09-20。OB-044 本轮修改基于 `56ac4b284b1a554dc3ab66638200b2b0c2ef0c98`；更早的引擎行为核验基线为 `e67e67b11eb265f097942055962622e43fcb9a16`。随后提交 `741f40a97ce2a0672be4c85b24aa4bef85faccae` 增加开发侧基线资产、固定工具链和 CI benchmark pin；`9be460d` 交付首批 Automation CDP Profile、精确 initializer 契约和必需官方客户端 smoke。

## 根本目标与当前阶段

面向机票采购效率，建设对 RPA 友好、性能突出、在服务端可观察的行为和指纹上与固定参考 Chrome 一致，且匿踪、反追踪、防身份标记能力强的浏览器内核。保护可关联身份，降低被用于差异化报价、侵害消费权益的风险，是产品目标；不是对当前实现或任何网站价格机制的既成结论。

CDP-first、官方 Playwright Python、独立内核和 persona 是实现路径。反追踪不能用 `DNT=1`、清空 Cookie 或几个伪装字段代替；分别验收 RPA 正确性/效率、网络与行为一致性、跨站/会话/身份空间关联抵抗、完整进程链性能及兼容性成本。受控报价实验在使用方进行，内核提供隔离、配置和完整原始证据。开发工具不做脱敏或删字段，日志的保管、清理和对外流转由执行方负责。

**当前处于迁移阶段。** 已有 Rust/V8/DOM/渲染、CDP 和 Worker 能力；CLI/MCP、自有 Python SDK/NDJSON runtime 仍存在。`tools/unblocked/` 已包含机器可读的基线、官方 Playwright Python 1.60.0 范围、独立锁和校验器、macOS 首批三路 CDP 记录/差分证据，以及首批实际观测的 Automation CDP Profile 和必需 smoke；Linux 资格、采集开关副作用和完整 profile 仍未完成。OB-044 已完成运行时 `--stealth`/环境开关删除和产品构建强制 primp；共享 `PersonaSpec` 到不可变 `EffectivePersona` 的编译、加载、摘要、入口注入和 context 投影已在本轮实现，进程级时区与 ICU 主语言冲突会 fail closed。OB-012 全出口收敛、容器干净验证和跨平台 persona 资格仍未完成。最新范围：删除自有 Python SDK/配套私有 runtime；保留 MCP 和有用 CLI，不强制新增宿主产物；全出口 primp 与 Chrome 差异修补继续推进。Southwest shopping 不再 403 且返回有效结果是重要业务门槛。不要把规划中的删除、认证和独立发布写成已经完成。

目标与阶段：[New_ACH](New_ACH.md)。唯一执行队列：[TODO](TODO.md)。

## 此前切片的验证

环境：macOS arm64，Rust/Cargo 1.98.1，cargo-nextest 0.9.145。下表保留 OB-044 后续、Continue postData 修复之前的验证记录。Continue 切片的实际重跑结果见下文专段；未重跑的渲染、客户端和线级证据不算作本轮结果。

| 检查 | 本次结果 | 边界 |
| --- | --- | --- |
| 根 release nextest，render | **1870 passed，4 skipped，0 failed** | OB-012 raw transport header、Worker 观察、统一 response body spool 与 Fetch 读取切片后的完整 workspace 门禁；不是完整客户端或多平台资格 |
| 独立 runtime release nextest，locked | **185 passed，0 skipped** | runtime 普通依赖已无条件包含 primp；不能替代全部网络/线级测试 |
| 指定 release CLI build，render | **通过** | 默认产品构建无条件包含 primp |
| 指定 release CLI build，no-default-features | **通过** | 无渲染产物仍包含 primp；不能借 feature 组合关闭该能力 |
| 指定 release CLI build，render,stealth | **通过** | `stealth` 仍是空兼容 feature，不能改变默认传输行为 |
| 旧 pin `6ebac829` 的 obstacle course | **32/33** | `observer-intersection` 期望 `io:50`，但 fixture 只观察一次 sentinel；这是修复前归因证据 |
| 旧 observer fixture 对照 | Chrome **153.0.8010.50** 与 Obscura 均为 **10 items，result=null** | Chrome headless，经官方 Python 1.58.0；1280×720、DPR 1、加载后等待 3 秒；未修改当时结果 |
| 修订 pin `2340bbb9` 的 observer 对照 | Chrome 与 Obscura 均为 **50 items，result=io:50** | 首次真实相交通知按五个批次排空固定初始页；不宣称覆盖无限滚动 |
| 当前 CI pin 的完整 obstacle course | **33/33** | Obscura release render 二进制，`--runs 1 --warmup 0` |
| deterministic render fixtures | Obscura **66/66** 捕获且行为断言无失败；双方 132 张图均可解码、900×1000、非纯色 | 系统 Chrome 153 有 4 条固化字体/表单几何参考断言失败，故 harness 总退出 1；完整记录该环境差异，不把它计成全绿 |
| representative top/bottom captures | top、bottom 均退出 0，各 **15/15** 双引擎捕获；有效 fidelity 分别 10/15、11/15 | 1440×1000、默认 3 秒 settle、固定动画时刻；Remix 双方内容为空，另有捕获边界状态不稳项被排除，不从像素距离单独推断正确性 |
| 官方 Playwright Python **1.60.0** automation smoke | **通过**，协议日志覆盖 profile 要求的 **37/37** 方法 | 未修改官方客户端通过 `connect_over_cdp` 执行；验证 `rawHeaders` 加法字段不破坏解析，不代表完整 API/多平台认证 |
| 较早的官方 Playwright Python **1.58.0** async/CDP 证据 | 连接、建页、导航、title、label fill、role click、DOM 结果、evaluate、关闭均成功 | 历史单页证据；当前必需门槛已固定为 1.60.0，见下文；没有完整 API/多平台认证 |
| raw WebSocket/CDP 和 CLI 本地夹具 | 复现下节 6 项行为 | 使用本次 render 二进制；不涉及真实网站或凭据 |

4 个 skip 对应源码中的 ignored tests：`benchmark_sparse_cascade_hot_path`、`concurrency_5_does_not_abort_v8`、`http_control_plane_unblocked_during_long_js`、`fetch_intercept_concurrency_5_does_not_abort_v8`。没有把忽略项算作通过。

旧 observer fixture 的注释声称不断观察新 sentinel，但代码只 observe 一次，追加后没有重新观察或滚动；固定 Chrome 对照也未达到其期望。这证明失败来自 fixture 的循环假设，不能证明所有 IntersectionObserver 语义正确。修订版将范围明确为固定初始页，并在首次真实相交通知中完成五批加载；参考 Chrome 复验为 `io:50`，Obscura 聚焦用例为 1/1、完整门禁为 33/33。OB-037 已关闭；更广的 IntersectionObserver 语义仍由其他资格测试覆盖。

OB-044 本轮删除 CLI/serve 的 `--stealth`、`--user-agent`、`OBSCURA_STEALTH`、scrape worker 转发和嵌入 API 开关；CDP、MCP、CLI、Page 与独立 runtime 的产品路径默认使用 primp。Cargo `stealth` feature 只保留为空兼容别名，不能改变行为；发布矩阵不再生成有无 stealth 的组合。强制路径同时保留子资源缓存合并，将 JS fetch/XHR 的完整响应体和网络事件写入 CDP/MCP 观察面，并以原字节或明确 Base64 保存二进制响应。persona 在 BrowserContext 初始化时同时确定 primp 传输配置与 JavaScript 身份，并在 context 生命周期内保持不变；运行中的 CDP User-Agent 覆盖明确不支持。共享 persona 编译器和存续入口投影已在后续切片实现；全出口盘点和 Docker 干净构建仍由 OB-012/016/042 跟进，OB-044 保持未关闭。

OB-012 本轮移除 renderer 的隐式 `ureq` 图片出口，将默认资源缓存改为只消费已注入字节；robots.txt 改走 persona-owned primp，并验证其与导航使用同一 persona User-Agent；每个 Page 的 primp 与其 detached Worker 共享 transport in-flight 计数，兄弟 Page 保持隔离，`networkidle` 合并观察互斥的预发送/CDP 拦截与 primp 传输阶段；native policy `RequestInterceptor` 保留完整二进制请求体。页面和独立 `ObscuraJsRuntime` 的 fetch/XHR、CORS OPTIONS、module、图片、字体与样式请求均只经 primp 获取；独立 runtime 在初始化时固定默认 Windows Chrome145 persona，frame/Worker 继承身份、Cookie 和策略。context baseline、Page override 与 request-specific headers 分层合并且兄弟 Page 隔离；无效代理 fail closed，PEM/DER CA、scripted 逐跳 timeout 与 body cap 保持。运行时挂接 Page 时会明确接管 transport，保留 Page persona、私网策略、Cookie 和回调。第二切片将 `ObscuraHttpClient` 收敛为 policy/context 类型，删除其自有发送 backend、timeout 假配置、项目自有直接 reqwest 依赖、根与独立 runtime 锁文件中的对应 package，以及 `wreq_client` alias；CLI `original` 文件和 HTTP 辅助路径统一走 `StealthHttpClient`，原有 37 项 legacy 网络测试完整迁移到 primp。

本轮 raw header 切片将 `HeaderCapture`/`RawHeader` 从 primp 边界贯穿 `Response`、`RequestInfo`、JS/render/Page 到 CDP 观察面。`rawHeaders` 是 Obscura 加法扩展，格式为 `captureStage=transportRequest|transportResponse`、`encoding=base64`、`fields=[{nameBase64,valueBase64}]`；重复值、非 UTF-8、Cookie、Authorization、Set-Cookie 的原始 bytes 均不脱敏、不裁剪，兼容性的 map 只是派生视图。本轮不宣称已经取得 wire capture。Worker 观察切片已完成成功型请求的贯通：`WorkerObservations` 将 raw captures 和 body 回灌 owning Page；保留 `JsNetworkEvent` 的真实 resource type，Worker 主脚本在 CDP 标为 `Script` 且 transport `Sec-Fetch-Dest: worker`，普通 fetch 的 `Sec-Fetch-Dest` 保持 `empty`；Worker queue 字节预算计入 request/response raw header 的全部 name/value；端到端 nested Worker 回归覆盖 raw headers、body 和 `Network.getResponseBody`。Page-owned response body 切片已完成：Document、classic Script、Stylesheet、Image、Font 保存原始 bytes；默认超过 2 MiB 转为 `NamedTempFile`，Page 总预算为 256 MiB/16384 entries，预算失败显式报告并在 `clear` 前停止新增，不静默 eviction；alias 共享。invalid UTF-8 文本经 CDP 以 base64 精确返回；Fetch stream 转移 raw store，活动 stream 不被驱逐，Page clear/drop 后仍可读，`IO.close`/context drop 清理。该切片聚焦 release nextest 为 25/25。删除范围是项目自有直接依赖和客户端，不代表整个依赖生态绝对不含 reqwest。所有采集与日志保留 Cookie、Authorization、重复头、原始字节和完整 body，不做脱敏或字段裁剪。

上述 Page-only 记录随后扩展为 JS fetch/XHR、module loader 和 Worker 的 owning Page 共享 `ResponseBodyStore`：成功 transport 响应超过 2 MiB 进入 spool，invalid UTF-8 保留原字节，Worker 观察队列只传 metadata、不复制 body；module 仅记录最终 URL 的成功 2xx 响应，resource type 为 `Script`。该扩展聚焦 release nextest 为 27/27，根 release nextest 为 1870/1870，4 skipped。`Fetch.getResponseBody` 现已读取统一 raw store，重复 get 不消费正文并与 `Network.getResponseBody` 使用同一编码；`takeResponseBodyAsStream` 之后 get 明确返回 consumed。session 严格隔离，带 session 的请求或未知 session 不会跨 Page 查找；早期无 session 多 Page 查询采用首个可读 body，已由下述 pause 隔离切片改为歧义拒绝。live request-stage 请求的 get/take 返回 `response_body_not_ready` 并保留 resolver。这是 completed-capture requestId 扩展，不是标准 response-stage interception；whole-body get 会 materialize spool，磁盘 IO 故障注入尚未单测。该 Fetch 切片聚焦 release nextest 为 16/16。后续 scripted failure observation 切片已补齐 JS/module/Worker 的 redirect、preflight/失败链与 Fulfill/CORS early-return 观察；仍有 transport 先完整 materialize `Vec`、JS 100 MiB 与 module 32 MiB 默认硬帽、metadata 4096 上限、独立 Worker target、`importScripts`、公开 HashMap 输入限制、其他完整响应体出口；低层 `ObscuraState` 字段也有源码兼容变化。OB-012 保持未关闭。

当前 synthetic `Fetch.fulfillRequest` capture 已接入 owning Page 的共享 raw store：JS fetch/XHR、Worker Fulfill 会产生成功 Network 事件，超过 2 MiB、invalid UTF-8 和 binary body 均保留完整原字节。pause 的 `intercept-N` alias 到完成 capture 的 `fetch-N`；`Network.getResponseBody`、`Fetch.getResponseBody` 与 stream 共用 consumed/budget 状态，Page 生命周期内跨导航不会复用 ID。CDP `responseHeaders` 的重复值、大小写、顺序及 `binaryResponseHeaders` 的非 UTF-8 原字节均保留，raw capture 使用 `captureStage=cdpFulfillResponse`，明确表示 synthetic response 而非 transport/wire capture。非法 body/header base64 在解除 pause 前拒绝且可重试。既有 Fulfill CORS、opaque、302 语义保持；Fail 与 transport CORS 拒绝不产生成功事件。聚焦 release nextest 为 9/9；最终根 release nextest 为 1876/1876，4 skipped，独立 runtime 为 185/185，三种发布构建均成功，障碍课为 33/33，官方 Playwright smoke 与 37-method 协议画像校验通过。当时尚无 preflight/redirect failure 完整生命周期，后续 scripted failure observation 切片已补齐；真正 response-stage interception 与 IO 注入仍未完成；新增 public enum variant 会影响 exhaustive match 的源码兼容性。OB-012 保持未关闭。

本轮 `Fetch.continueRequest.postData` 切片：主 server handler 和 `domains/fetch` 共用严格 standard-base64 解析；非法 base64、非字符串及 null 返回 `-32602`，校验完成前不取走 pause，连续错误后仍可重试。省略 `postData` 保留原 body，空字符串明确清空；`InterceptResolution::Continue.body` 与 `FetchResolution::Continue.post_data` 使用 `Option<Vec<u8>>`，直到 primp 发送均保留任意原字节，不经过文本或 lossy UTF-8，也不脱敏、不裁剪。URL、method、headers 覆盖及 SSRF、redirect、CORS、credentials 的后续处理保持原路径。新增回归覆盖两个入口和真实 HTTP fixture 下的页面 JS fetch / Worker：NUL、0xff、全部 256 种字节、空 body、未覆盖原 body、重复错误后重试，以及 URL/method/Authorization/Cookie。本轮 render/no-render focused release nextest 各为 3/3；最终根 release nextest 为 1879/1879，4 skipped；独立 runtime release nextest 为 185/185；no-default-features、render,stealth、render 三种 exact CLI release build 均成功；CI 固定 benchmark 版本的障碍课为 33/33；官方 Playwright Python 1.60.0 smoke 通过，完整协议日志通过 37-method profile 校验。公开 Continue 字段从 `Option<String>` 改为 `Option<Vec<u8>>`，嵌入调用者提供文本覆盖时需显式 `into_bytes()`。该 postData 切片当时未扩展 preflight/redirect failure 生命周期，后续 scripted failure observation 切片已补齐；response-stage interception、request header HashMap 表达能力与 wire/TLS 校准仍未完成；OB-012 保持未关闭。

本轮 active multi-Page Fetch pause 隔离切片：每个 Page 的拦截通道在 `Fetch.enable` 时绑定 owning Page/frame/session，保持客户端可见的 `intercept-N` 和 completed body alias 不变；resolver 以 `(sessionId, requestId)` 查找，不再覆盖同一连接兄弟 Page 的相同本地 ID。显式 session 必须精确拥有活动 pause，同页另一个 flattened session 也不能接管；一页只能由一个 session 启用 Fetch。单 Page 保留无 session enable/resolve/read/disable；多 Page 的 Fetch 请求以及 `Network.getResponseBody(intercept-N)` 必须提供 session，导航中暂时移出 Page 列表也不降低这个门槛。使用无 session enable 的客户端须在创建第二页前切换到具名 session；已有 null-owner pause 不允许其他 session 接管。无 session 的 Network/completed-store 查询遇到多个 retained/consumed alias 明确报歧义，不再选首个可读正文；store-wide budget failure 仅作为诊断，不冒充某个 request 的归属。get/take 的 not-ready 和 malformed base64 均保留 pause；已完成左页 alias 不被仍暂停的右页同 ID 遮挡。owner-aware disable 只继续该 Page 的请求，detach/close/dispose 清理失效 owner 并以 Aborted 结束其 pause，连接关闭含导航中请求也不会遗留 resolver；尚未发出 requestPaused 的 queued envelope、relay 发送失败/任务取消和 reply channel 关闭均显式 Aborted，不以丢弃 resolver 隐式放行。真实 CDP processor 回归覆盖两个同时活动 Page、显式 flattened sessions、页面/Worker 请求、相同 ID、错误/缺失 session、Continue/Fulfill/Fail、重试、completed alias/stream、导航和 detach 后重新 attach。Cookie、Authorization、原始 header/body 不脱敏、不裁剪。这里仍不是 response-stage interception；后续 scripted failure observation 切片已补齐 preflight/redirect failure 完整生命周期，request header HashMap 表达能力、独立 Worker target、wire/TLS 校准与 IO 故障注入仍未扩展，OB-012 保持未关闭。公开 JS/Page interception API 未改；低层 `CdpContext.intercept_tx` 改传带归属的 CDP envelope。本切片最终 release nextest：render CDP lib **183/183**；no-render focused **18/18**（含并发、生命周期、queued disconnect、raw body 和 retry 回归）；`git diff --check` 通过。本切片最终完整门禁：根 release nextest **1886/1886**，4 skipped；独立 runtime **185/185**；no-default-features、render,stealth、render 三种 exact CLI release build 均成功；CI 固定 benchmark 障碍课 **33/33**；官方 Playwright Python 1.60.0 smoke 通过，完整协议日志通过 37-method profile 校验。

本轮 `Fetch.continueRequest.headers` 切片：两个 CDP 入口在消费 pause 前严格校验 HeaderEntry 数组、字符串 name/value 和 HTTP field 语法；非法输入返回 -32602，同一请求可连续纠错重试。有序列表保留重复 name、输入大小写、数组顺序及 Cookie/Authorization/空值/Unicode 的完整值，直到 HTTP HeaderMap 边界，不再经发送用 HashMap 压扁。request fields 按不区分大小写覆盖 context/Page baseline；native policy 的 map 修改仅替换指定 name，其余重复字段保留，policy 观察可通过 raw_headers 的 requestPolicy 阶段读取完整字段。CORS 使用单独的合并视图分类重复字段，原 URL/method/body、SSRF、redirect、credentials 和 Page/Worker/session 归属路径保留。既有 browser-owned Origin、Referer、Sec-* 仍由原 persona/request policy 生成；Cookie jar 仍在允许 credentials 时先追加，然后追加显式 Cookie。HTTP HeaderMap 会小写化名称、按名称分组；persona/primp 决定跨名称顺序，并可注入 Host/Content-Length 等字段或采用 HTTP/2 编码。因此仅承诺 CDP 列表和同名值的保留，不宣称 wire casing、全局 wire order 或 framing。公开 InterceptResolution::Continue 原字段和构造方式保持；新增 ContinueWithHeaders variant 会影响 exhaustive match，低层 CDP FetchResolution::Continue.headers 改为 Option<Vec<(String, String)>>。本切片没有解除其他 JS/embedded HashMap 输入限制；后续 scripted failure observation 切片已补齐 preflight/redirect failure 生命周期，response-stage interception、独立 Worker target 与 wire/TLS 校准仍未完成；OB-012 保持未关闭。本切片验证：render/no-render focused release nextest 各 **15/15**；最终根 release nextest **1889/1889**，4 skipped；独立 runtime **185/185**；no-default-features、render,stealth、render 三种 exact CLI release build 均成功；CI 固定 benchmark 障碍课 **33/33**；官方 Playwright Python 1.60.0 smoke 通过，完整协议日志通过 37-method profile 校验；`git diff --check` 通过。

本轮 scripted failure observation 切片：fetch/XHR、Worker 和 module 请求在开始时分配 `fetch-N`；Fetch pause 继续使用独立的 `intercept-N`，`networkId` 关联同一 Network 请求，完成后 alias 共用原有 body store。失败记录包含完整 URL/method、阶段明确的原始 request/response headers、请求 body 字节数和错误原因；Cookie/Authorization 不脱敏。preflight 使用独立 request ID/type，并关联发起请求，拒绝时不会把 OPTIONS 响应体作为主请求正文。redirect 每个实际发送 hop 均可独立 request-stage pause/Continue/Fulfill/Fail，拥有新 `intercept-N` 并保持同一 logical `fetch-N`；中间响应通过后续 `requestWillBeSent.redirectResponse` 表达，单独的 `bodyRequestId` 定位其完整正文，读取仍受 body store budget/consumed 状态约束，预算失败返回明确错误；最终请求才产生 `loadingFinished` 或 `loadingFailed`。发送前拒绝仅有 request/failed；已收到真实响应后发生 CORS、redirect policy、body cap 或读体错误时保留合法获得的 headers，完整读完的响应正文进入统一 spool，未完成或超限正文不会伪装成成功 body。早期 pause/preflight 的 script headers 与最终 `transportRequest` 不混淆，后者通过 `Network.requestWillBeSentExtraInfo.rawHeaders` 完整提供。CORS/opaque 对页面 JS 的可见性保持，CDP 是有权限的诊断观察面，不能据其可读正文宣称页面脚本可读。fetch AbortSignal、XHR abort/timeout 可取消原生 future；关闭或销毁 runtime/Worker 的 terminal 通过 owning Page 共享失败队列与 Notify 送达，document generation 固定其原 loader，跨导航不复用请求 ID；generation→loader 映射保留到 Page close，导航次数带来少量线性状态，以保留晚到 Worker terminal 的归属。真实 pause 记录 owner session，终态不会转发给同 Page 的其他 flattened session。已取消的 Fetch pause 拒绝迟到的 Continue/Fulfill/Fail。redirect 后需要 preflight 时，会在预检 yield 前按序发布已完成 hop 与下一 hop start，后者携带完整 `redirectResponse`；redirect pause 等待期间保持 network-active，避免 `networkidle` 提前。XHR `open()` 会取消旧 controller/timer，并用 request generation 阻止旧 Promise 回写；关闭的 routed pause 只有真正发出后才登记 owner，不会在 terminal 后恢复陈旧归属。只有真实 transport response 已存在时才发送 `responseReceived`，随后失败只发送 `loadingFailed`，绝不同时发送 `loadingFinished`；URL/SSRF/policy/DNS/connect/TLS 等发送前失败不会伪造 response。body cap/read failure 可保留真实 headers 与已知长度，partial/超限 body 不作为完整正文。这里只扩展脚本请求的观察生命周期，不是真正 response-stage interception，也不证明 wire casing/order、TLS、独立 Worker target 或 `importScripts` 完整支持。transport 硬帽、body store budget、metadata 4096 上限仍存在，OB-012 保持未关闭。公开 `InterceptedRequest`、`JsNetworkEvent`、`NetworkEvent` 增加观察字段，直接构造这些低层类型的调用方需适配。

本切片最终验证：render focused release nextest **279/279**，509 skipped；no-render focused **103/103**，565 skipped；review-combination **7/7**；根 release nextest **1910/1910**，4 skipped。no-default-features、render,stealth、render 三种 exact CLI release build 均成功；复用冻结的 render 二进制完成障碍课 **33/33**、官方 Playwright Python **1.60.0** smoke、37-method 协议画像校验和 redirect route E2E。route E2E 确认两条 redirect 第二跳均在 owner session pause 并由 Playwright 自动 continue；成功链保持同一 Network ID，302→200 后仅 `loadingFinished`，连接失败链仅 `loadingFailed`，`networkidle` 晚于最终响应且页面 `done=true`。冻结二进制 SHA-256 为 `5912bcccb186f926010c5e7c118763a903d8e89dccc295c8e7ade2f02407087c`（118958752 bytes）。根门禁已经覆盖本轮 runtime 改动，故未为相同源码额外重复运行独立 runtime 全套；`git diff --check` 通过。

本轮 response-stage interception 最小切片：`Fetch.enable.patterns[].requestStage=Response` 已接入 scripted `op_fetch_url` 路径，覆盖页面 fetch/XHR、Dedicated Worker 内的 fetch/XHR、经 scripted helper 发起的动态 classic script/stylesheet 与 CORS OPTIONS；URL 与 `resourceType` 会共同匹配，XHR、Fetch、Script、Preflight 按实际路径上报。每个已完整读取且成功进入 body store 的 transport redirect/final hop 都先将原始 body 放入 owning Page 的共享 store，再向 owner session 发送 `Fetch.requestPaused`。事件提供 `responseStatusCode`、兼容 header 列表、完整 `responseRawHeaders`、`networkId` 和同阶段 redirect 的 `redirectedRequestId`；兼容 `responseHeaders` 对非 UTF-8 原始字节使用一字节一字符投影，不丢字段，base64 编码的 `responseRawHeaders` 保留权威原值；Cookie、Authorization、重复头、非 UTF-8 头值及 body 不脱敏。transport 尚未采集原始 HTTP reason phrase，因此 `Fetch.requestPaused`、redirectResponse 和页面 JS 的 statusText 明确为空；`continueResponse`/`fulfillRequest` 提供的非空 `responsePhrase` 只贯通到后续 Network 响应事件，不回填已发送事件或页面 JS。body cap、读体错误或 store budget 失败不会伪造完整 response-stage pause。活动 final response pause 可选择重复 `Fetch.getResponseBody` 或一次 `takeResponseBodyAsStream`，两者互斥；Fetch stream 只允许 owner session 顺序读取且拒绝 offset，Page 退出后 owner 仍可读至关闭。redirect response 的这两个 Fetch body API 明确拒绝；stream 已转移后只允许 Fail/Fulfill。response-stage 可用空 `continueRequest` 或无覆盖 `continueResponse` 原样继续；响应覆盖要求 responseCode 与 headers 同时提供，responsePhrase 可省略。Fulfill 未提供 body 时保留原 body，替换后更新共享 alias，同时已打开 stream 继续持有旧 body。disable/detach/connection close 仍按 owner 生命周期解除，导航期间 body store 可读且 disable 会清理返回 Page 的旧策略。当前范围不是全浏览器 response-stage：Document navigation、独立 module loader，以及 Page/native Stylesheet、Image、Font 与 renderer resource transport 尚未接入；动态 classic script/stylesheet helper 仅在其实际经过 scripted `op_fetch_url` 时覆盖；`handleAuthRequests`/`continueWithAuth` 也未实现。OB-012 保持未关闭。

本切片最终验证：根 release nextest **1916/1916**，4 skipped；sessionless/navigation 聚焦回归 **1/1**，no-render response-stage 与 sessionless/navigation 回归 **2/2**。render 与 no-default-features 两种 exact CLI release build 均成功；CI 固定 benchmark 障碍课 **33/33**；官方 Playwright Python **1.60.0** smoke 与 **37-method** 协议画像校验通过。上述端到端验证共用冻结 render 二进制，SHA-256 为 `c4eff66c2da49c42e17eef91f41d52081b86cbfc7a2b6a4591718a432bad770b`（119043264 bytes）。Astra light 最终复核为 0 blockers；`git diff --check` 通过。

本切片证据日志为 `/tmp/ob012-failure-focused-render-final.log`、`/tmp/ob012-failure-focused-no-render-final.log`、`/tmp/ob012-review-combinations2.log`、`/tmp/ob012-failure-root-final.log`、`/tmp/ob012-failure-build-{no-default,render-stealth,render}.log`、`/tmp/ob012-failure-obstacle.log`、`/tmp/ob012-failure-playwright-validate-render.log` 和 `/tmp/ob012-failure-playwright-route.log`；smoke JSON 与完整协议位于 `/tmp/ob012-failure-smoke-render.DKO0vf/`，route E2E 结果位于 `/tmp/ob012-playwright-route-redirect.json`。这些临时证据不入 Git、不保证长期保存。

Headers 切片基于 `518c4cd58ba44d0bcad8af8dba529bca3eeeaf60`。最终 render CLI SHA-256：`be9229b545c711fd43314cfd17d30be4ea5c3b4d2fde6938152d1c642b1b2fce`（119066688 bytes）。本机日志为 `/tmp/ob012-continue-headers-focused2.log`、`/tmp/ob012-continue-headers-no-render.log`、`/tmp/ob012-continue-headers-full-final.log`、`/tmp/ob012-continue-headers-runtime.log`、`/tmp/ob012-continue-headers-build-{minimal,stealth,render}.log`、`/tmp/ob012-continue-headers-obstacle.log`、`/tmp/ob012-continue-headers-playwright-validate.log`；smoke JSON 与完整协议日志位于 `/tmp/ob012-continue-headers-smoke.LxW5KB/`。这些临时证据不入 Git、不保证长期保存；本轮没有新增 wire/TLS、真实网站或性能分布结论。

Pause 隔离切片基于 `44bb2faa62fed6ad2b4066a6c20db497ceca7736`。最终 render CLI SHA-256：`2ae0eabbf55fc09d643496ffd62a5b6143da2d9be8de775a5833771425563a0c`（119034016 bytes）。本机验证日志为 `/tmp/ob012-pause-cdp-lib.log`、`/tmp/ob012-pause-no-render.log`、`/tmp/ob012-pause-root.log`、`/tmp/ob012-pause-runtime.log`、`/tmp/ob012-pause-build-{minimal,stealth,render}.log`、`/tmp/ob012-pause-obstacle.log`、`/tmp/ob012-pause-playwright-validate.log`；完整 smoke JSON 与原始协议日志位于 `/var/folders/r8/vzjpyytd6yx0wwcwtl1wd81w0000gp/T/tmp.dfKYrR8OmN/`。这些临时证据不入 Git、不保证长期保存；本轮未重跑渲染 top/bottom 现场对照、TLS/H2 wire capture 或真实网站验收。

Continue 切片基于 `6883189e03dcc9832837477c0e1902606ade0c76`。最终 render CLI SHA-256：`4714190ac81d6a3194acd073429749c1f0f821e424f0650646b09157de32384f`（118948192 bytes）。本机验证日志为 `/tmp/ob012-continue-focused-final.log`、`/tmp/ob012-continue-no-render-focused.log`、`/tmp/ob012-continue-root-final.log`、`/tmp/ob012-continue-runtime.log`、`/tmp/ob012-continue-build-{minimal,stealth,render}.log`、`/tmp/ob012-continue-obstacle.log`、`/tmp/ob012-continue-playwright-validate.log`；完整 smoke JSON 与原始协议日志位于 `/tmp/ob012-continue-smoke-6inj3S/`。这些临时证据不入 Git、不保证长期保存；本轮未重跑渲染 top/bottom 现场对照、TLS/H2 wire capture 或真实网站验收。

本次未运行：Linux 原生验证、完整官方客户端矩阵、WPT、24h 长稳、受控性能/TLS/H2 测量、Docker 构建、Southwest/ZG 现场流程和报价对照。没有新的生产发布或部署结论。

### 构建与证据定位

此前 Fulfill 切片 render CLI SHA-256：`8de422416c94b684f3d3752c4b10b86495b427f1fb08e187885fee22551c754c`（118901344 bytes）。

- 根 Cargo.lock SHA-256：`813ac17dfae3d60a779ee6d892d9989bbb92c7f18b3bc4f16bb569868f280343`。
- runtime/Cargo.lock SHA-256：`279f5b950dbb6d03500cf231fc5554e704f762804e81fc7fbccacde4a720ccf7`。
- benchmark revision：`2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`（`gster/obscura-benchmark`），与当前 CI pin 一致；旧失败归因使用 `6ebac8293d7477f59e837768bfd4e74173f04f1c`。
- 本机最终门禁日志：`/tmp/ob012-fulfill-root-nextest.log`、`/tmp/ob012-fulfill-runtime-nextest.log`、`/tmp/ob012-fulfill-build-minimal.log`、`/tmp/ob012-fulfill-build-stealth.log`、`/tmp/ob012-fulfill-build-render.log`、`/tmp/ob012-fulfill-obstacle.log`、`/tmp/ob012-fulfill-playwright-validate.log`；官方 Playwright smoke 与完整协议日志位于 `/var/folders/r8/vzjpyytd6yx0wwcwtl1wd81w0000gp/T/tmp.3m5cz34egi/`。这些临时文件不入 Git，也不保证跨设备或长期存在；仓库内保留命令、结果和源码入口，外部原始证据缺失时须重跑。

```bash
# 仓库根
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --release --features render --no-fail-fast
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --no-default-features
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render,stealth
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --locked --release --no-fail-fast)

# 固定 revision 的 companion benchmark 仓库；绝对二进制路径按环境替换
OBSCURA_BIN=/absolute/obscura/target/release/obscura python3 obstacle-course/run.py --runs 1 --warmup 0
```

这不是性能报告；构建、测试及各探针时序不用于宣称加速。root nextest 和 runtime nextest 是两个 workspace 的结果，不能相加成为一个统一资格分数。

### OB-027 首批 profile 后续验证

当前必需 smoke 基于未修改的 Playwright Python 1.60.0 `connect_over_cdp` 路径运行。raw `pw:protocol` 日志中 37 个实际发送方法全部进入 profile；`08afec2` 增加 CI 对未改写完整日志的方法集合对账。smoke 通过官方 `get_by_label().fill()`、单选/多选 `get_by_label().select_option()` 与 `get_by_role().click()` 验证 labelled input/select 和 role button；为此补齐了 Chrome 对齐的 option label/text 与多选 selectedOptions 语义。固定 320×240 viewport 的官方 `Page.screenshot()` 路径验证 `Page.getLayoutMetrics` 和 `Page.captureScreenshot`，解码 PNG 像素证明画面非空并完整保留 PNG 数据。`Browser.new_context()` 路径验证独立 context 的创建、同源 localStorage/全局对象隔离、1280×720 viewport/screen、同 context 双页面、`Page.close()` 后存活页状态、context 显式关闭以及默认页面继续执行。B 页 fetch 的响应在服务端保持挂起，确认 close 边界后才释放；所有 console 事件带阶段完整保留，本次 close 后事件为空。三个文档响应的 HTTP body/headers、完整 `DOM.getDocument` 响应、locator/context 返回值与事件记录、显式 CDP 命令/响应/错误均保留。profile 明确是 observed slice；截图格式/选项全矩阵、代理等 context options、长期并发、断连清理、utility world 真隔离、完整 actionability/focus/user-activation、未列方法和未验证参数均未取得资格，OB-027/028 未关闭。

| 检查 | 结果 | 边界 |
| --- | --- | --- |
| obscura-cdp release nextest，render | **226 passed，3 skipped，0 failed** | 覆盖 context/page 关闭参数、window/viewport/screenshot 初始化和真实 WebSocket 非法 `Browser.close` 回归 |
| 根 release nextest，render | **1789 passed，4 skipped，0 failed** | 不替代 Linux、stealth、WPT 或完整客户端资格 |
| 指定 release CLI build，render | **通过** | 精确 AGENTS.md build 命令；不宣称 stealth |
| Playwright 1.60.0 required smoke | **通过，37/37 observed methods 已登记**；覆盖官方 locator、固定 320×240 非空 PNG、双页面 `Page.close()` 与 `Browser.new_context()` create/use/dispose，完整数据均保留 | 本地合成页面；不是完整 API、screenshot/context options、长期并发、断连、参数矩阵或 utility-world 隔离证明 |
| 固定 benchmark obstacle course | **33/33** | `--runs 1 --warmup 0`；包括 `observer-intersection` |

本轮完整 raw protocol 日志和 smoke JSON 位于执行方临时目录，不进入 Git；开发工具不做脱敏或删字段，测试后由执行方处理日志。

## 当前缺口：源码与实验交叉核验

| 项目 | 证据 | 处置 |
| --- | --- | --- |
| CDP 假成功 | `9be460d` 已移除首批路径的 server fast path 与整域 no-op；不存在的 `Log.auditMethodDoesNotExist` 现在报错，精确 initializer 越界参数也报错；其余方法/参数未取得资格 | OB-027：扩展 observed profile 并逐项完成参数、事件和作用域契约，不从首批切片外推 |
| utility world 没有真实隔离 | Page.createIsolatedWorld 登记 ID；Runtime 注释明确仍使用页面 global；实测新 world 可读主世界 `auditMainOnly=73` | OB-029：真实 realm/wrapper/句柄边界 |
| 观察会执行 getter | Runtime.getProperties 的 `Object.keys` 后读取 `obj[k]`；实测 getter 计数 0→1，返回被读取值 7 | OB-030：描述符检查和对象生命周期 |
| Beacon 空成功 | bootstrap 两处 sendBeacon 直接 true；同源本地 POST 接收数为 0 | OB-039：真实排队/发送、body、配额和生命周期 |
| localStorage 没有落盘 | BrowserContext 仅加载/保存 cookies.json；同一 storage-dir 两次 CLI，第一次写入，第二次读出 null | OB-019：明确状态范围和持久化 |
| viewer 跨连接假设错误 | server 为连接创建 isolated_copy 和独立 processor；第二条连接 getTargets 返回空 | 修正文档；OB-028/034 定义 owner/断连/诊断模型 |
| Cookie 持久化格式与连接合并 | version 1 envelope 直接保存内部 CookieEntry，保留 host_only；load 兼容历史裸 CookieInfo 数组；CookieJar 提供 lossless snapshot/from_snapshot/apply_snapshot_delta，CDP 连接关闭时按快照差异合并 | render/no-render Cookie 相关回归各 53/53，根 release nextest 1926/1926；SameSite 完整请求上下文、分区及 CDP/MCP 对外状态往返资格仍未完成；OB-011 |
| Cookie 请求上下文不完整 | get_cookie_header 仅接收 URL；SameSite 字段存在但该选择路径无 site/method/请求类型上下文；分区能力也未完成 | 源码确认；不能只因 version 1 能无损保存内部 CookieEntry 就宣称完整 Cookie 语义；OB-011/012 |
| IndexedDB 部分实现 | 请求、upgrade transaction、索引/游标已有代码与回归；abort/commit 空体，数据库存于 JS realm 的 Map | 源码确认，完整事务/生命周期未验证；OB-041 |
| 身份配置分叉 | BrowserContext 网络与 JS 已收敛到同一 `StealthProfile`，但独立 runtime Persona、CDP Browser.getVersion 和平台 preset 仍有各自字段 | OB-014/015/016/031；还没有完整 persona 编译器或已认证的 Linux/macOS persona |
| 控制面边界 | CDP 无内建鉴权；server 有 unbounded_channel，连接限制不等于消息/队列预算 | OB-034；不以 loopback 替代完整授权/背压 |

主要源码入口：[CDP dispatch](../crates/obscura-cdp/src/dispatch.rs)、[server](../crates/obscura-cdp/src/server.rs)、[Runtime 域](../crates/obscura-cdp/src/domains/runtime.rs)、[Page 域](../crates/obscura-cdp/src/domains/page.rs)、[Browser 域](../crates/obscura-cdp/src/domains/browser.rs)、[bootstrap](../crates/obscura-js/js/bootstrap.js)、[CookieJar](../crates/obscura-net/src/cookies.rs)、[BrowserContext](../crates/obscura-browser/src/context.rs)、[runtime Persona](../runtime/src/browser.rs)。源码审阅和以上实验不是完整安全审计。

raw CDP 复验次序：创建并 attach target → Page.navigate 本地页面 → Runtime.evaluate 写入全局/建立带 getter 的对象 → Page.getFrameTree/createIsolatedWorld → 带新 contextId evaluate → getProperties → 回读 getter 计数。unknown Log 方法、Beacon 服务端计数、第二连接 target 清单和双进程 storage-dir 分别独立观察。所有 sessionId 使用 attach 返回值。

## 不应重复立项的已有实现

根 workspace 有九个 crate，独立 runtime 另有 workspace。Page 和 Worker 已有独立 isolate；Worker 已有线程、V8 序列化、transfer、终止、预算和网络转发。Worker 使用 detached primp 独立连接池。旧“同页模拟 Worker”“普通路径尚未修”“先迁入 primp”已失效。

Cookie 键已区分 domain/name/path，已有 host-only、HttpOnly 写保护和过期导入回归；应修剩余语义，不从旧快照重新实现已有保护。iframe/fragment、Text 更新、表单传输、预检和原生输入已有修复，现有回归位于 browser/page、JS runtime/bootstrap、net 与独立 runtime tests；迁移时保留这些能力。保留成果与回归，不据此夸大完整标准资格。

页面所有的产品传输及独立 runtime/module loader 当前强制使用 persona-owned primp；`ObscuraHttpClient` 已收敛为 policy/context，项目自有直接 reqwest 客户端/backend 与 `wreq_client` 兼容别名已删除，CLI `original` 文件和 HTTP 辅助路径也统一走 `StealthHttpClient`。这只描述项目自有直接依赖和客户端，不代表整个依赖生态绝对不含 reqwest。V8 依赖通常取预构建 archive；`V8_FROM_SOURCE` 才走源码构建，本项目还会执行 bootstrap snapshot 生成。旧“首次必编译 V8、固定五分钟”不是准确的构建契约。

## Southwest 与历史记录的结论

[Southwest-handoff](Southwest-handoff.md) 已收敛为事实边界。历史曾有成功样本，之后仍有 shopping 403；最新相关历史轮次即使 Airship 达到 8/8 仍未恢复 shopping。缺少可复核原始现场材料时，不重申具体数字为本次确认事实，更不推断唯一根因。

shopping 不再 403 且有有效航班/报价结果现已列为 OB-046 的重要验收；当前未验收通过。已撤回 Cookie 名称、响应头、错误文案、令牌长度等过度归因；原始过程保留在 Git 历史。旧主机路径、PID、未提交状态、临时诊断授权和产物 SHA 不再作为接手指令。固定回放与通用回归不能替代现场业务结果。

## 下一步顺序

1. G0：OB-001/025 固定双平台与正式客户端，OB-026 继续补齐可重复 trace/fixture。OB-027 首批 profile 和 smoke 已进入门禁，但仍只是实际观测切片。
2. 扩展 OB-027，并修清楚的语义与隔离缺口：OB-028/029/030、OB-011、OB-041。OB-037 obstacle 门禁已恢复。
3. 迁出独有能力并优先清理 Python SDK/私有 runtime；保留 MCP 和有用 CLI。统一 primp/强制 stealth，强化统一 persona，持续修补 Chrome 差异。
4. 按目标分别验收 Chrome 一致性、反关联/防标记、Southwest shopping 业务结果、完整进程链性能与发布资格。没有实际证据的组合保持未认证。

全部 46 项任务、依赖与关闭标准只在 [TODO](TODO.md) 维护。

## 文档索引与本次处置

首次审阅覆盖 `docs/` 的 32 个 Markdown 文件（含原先未跟踪的 New_ACH）及根 README。本轮删除已消化的 Obscura-fix-changelog.md 和 Southwest-fix-record.md，现保留 30 个 docs Markdown 文件；相关源码/回归与限制已归入本页和任务清单，历史原文在 Git 中。重写入口和冲突记录，保留仍对应现有代码的操作指南；“保留”不代表每个示例和平台都已执行。

| 文档 | 用途与处置 |
| --- | --- |
| [根 README](../README.md)、[docs README](README.md)、本页 | 定位、事实/证据与导航，删除无条件 drop-in 和无基线性能表 |
| [New_ACH](New_ACH.md)、[TODO](TODO.md)、[Development-plan](Development-plan.md) | 目标设计、唯一任务队列；旧计划变为入口，消除阶段和状态重复 |
| [Southwest-handoff](Southwest-handoff.md) | 当前未恢复的事实、业务验收条件与差异调查入口；删除轮次、修复流水账和过期操作 |
| [Worker](Worker-compatibility.md)、[Primp/wreq](Primp-and-wreq-comparison.md)、[Protection regression](Protection-script-regression-case.md) | 当前实现与回归边界；移除已被后续提交覆盖的中间结论 |
| [Architecture](Architecture-overview.md)、[Adding API](Adding-a-CDP-method-or-Web-API.md)、[Testing](Testing-and-debugging.md) | 修正 isolate/连接模型、域路由模板、错误 crypto 示例和不存在的测试入口 |
| [Build](Build-from-source.md)、[Installation](Installation.md)、[Production](Run-in-production-at-scale.md) | 固定 fork 构建；不沿用 upstream latest 安装/资格；Dockerfile 仅 render，未认证容器 |
| [CLI](CLI-reference.md)、[Environment](Environment-variables.md)、[Stealth/proxy](Configure-stealth-and-proxies.md) | 修正存储、DNS/代理边界、默认身份/WebGL 等过强说明 |
| [Connect](Connect-Puppeteer-or-Playwright.md)、[Playwright](Use-with-Playwright.md)、[Puppeteer 弃用边界](Use-with-Puppeteer.md)、[Interception](Intercept-and-modify-requests.md) | 官方 Playwright Python 是唯一客户端兼容目标；Puppeteer 已弃用，不再新增或维护其 profile 资格，遗留 initializer 待确认不影响 Playwright/raw CDP 后清理；完整 Playwright 客户端仍未认证 |
| [Persistence](Persist-cookies-and-storage.md)、[Live view](Watch-agent-sessions-live.md) | 重写实际持久化和连接所有权边界，撤下无效跨连接 viewer 教程 |
| [Rust library](Use-as-a-Rust-library.md)、[Isolated runtime](Use-the-isolated-runtime.md) | 底层嵌入与迁移期私有协议分开；移除外部消费仓库当前状态猜测 |
| [First fetch](Your-first-fetch.md)、[Extract](Extract-data.md)、[Markdown](Markdown-extraction.md)、[MCP](Use-the-MCP-server.md) | 保留的 CLI/MCP 使用参考；与尚未实现的统一身份/传输目标分开 |

外部契约核查：官方 [Playwright CDP](https://playwright.dev/python/docs/api/class-browsertype#browser-type-connect-over-cdp) 明确连接模式及保真度限制；[Cargo features](https://doc.rust-lang.org/cargo/reference/features.html) 的 additive 语义用于裁剪设计。它们不认证 Obscura，也不代替固定版本运行。

本轮验证：Cookie/context/server 相关 release 回归在 render 根门禁中 **53/53**，no-render 聚焦 **53/53**；最终根 release nextest **1926/1926**，4 skipped。render 与 no-default-features exact CLI release build 均成功；冻结 render 二进制通过 CI 固定 benchmark 障碍课 **33/33**、官方 Playwright Python **1.60.0** smoke 和 **37-method** 协议画像校验。二进制 SHA-256 为 `722ef38dcb23e1627271bb24809c2bf93855e652fc54eb9dd0438fe78c559ca4`（119072256 bytes）。Astra light 最终复核 0 blockers；`git diff --check` 通过。首轮根门禁曾出现 MCP `test_evaluate` 空标题失败，未改源码的单项重放 **1/1** 和完整复跑均通过；该测试忽略导航响应，现有证据不足以确定偶发根因，未将重跑通过称作修复。

额外真实 CLI 回归使用本地 HTTP fixture 和四个全新进程：HTTP Set-Cookie 的 `hostonly=alpha=beta` 与 document.cookie 的 `docvalue=from-document` 均经 version 1 文件恢复并出现在后续服务端 Cookie 请求头；持久化记录保留 host_only 和 HttpOnly。该探针验证产品存储入口，子域作用域由上述 Cookie 回归覆盖。

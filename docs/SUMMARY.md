# 项目现状与文档核验摘要

核验日期：2026-09-20。OB-044 本轮修改基于 `56ac4b284b1a554dc3ab66638200b2b0c2ef0c98`；更早的引擎行为核验基线为 `e67e67b11eb265f097942055962622e43fcb9a16`。随后提交 `741f40a97ce2a0672be4c85b24aa4bef85faccae` 增加开发侧基线资产、固定工具链和 CI benchmark pin；`9be460d` 交付首批 Automation CDP Profile、精确 initializer 契约和必需官方客户端 smoke。

## 根本目标与当前阶段

面向机票采购效率，建设对 RPA 友好、性能突出、在服务端可观察的行为和指纹上与固定参考 Chrome 一致，且匿踪、反追踪、防身份标记能力强的浏览器内核。保护可关联身份，降低被用于差异化报价、侵害消费权益的风险，是产品目标；不是对当前实现或任何网站价格机制的既成结论。

CDP-first、官方 Playwright Python、独立内核和 persona 是实现路径。反追踪不能用 `DNT=1`、清空 Cookie 或几个伪装字段代替；分别验收 RPA 正确性/效率、网络与行为一致性、跨站/会话/身份空间关联抵抗、完整进程链性能及兼容性成本。受控报价实验在使用方进行，内核提供隔离、配置和完整原始证据。开发工具不做脱敏或删字段，日志的保管、清理和对外流转由执行方负责。

**当前处于迁移阶段。** 已有 Rust/V8/DOM/渲染、CDP 和 Worker 能力；CLI/MCP、自有 Python SDK/NDJSON runtime 仍存在。`tools/unblocked/` 已包含机器可读的基线、官方 Playwright Python 1.60.0 范围、独立锁和校验器、macOS 首批三路 CDP 记录/差分证据，以及首批实际观测的 Automation CDP Profile 和必需 smoke；Linux 资格、采集开关副作用和完整 profile 仍未完成。OB-044 已完成运行时 `--stealth`/环境开关删除和产品构建强制 primp 的切片，但统一 persona 编译器、OB-012 全出口收敛及容器干净验证仍未完成。最新范围：删除自有 Python SDK/配套私有 runtime；保留 MCP 和有用 CLI，不强制新增宿主产物；统一 persona、全出口 primp 与 Chrome 差异修补为重点。Southwest shopping 不再 403 且返回有效结果是重要业务门槛。不要把规划中的删除、认证和独立发布写成已经完成。

目标与阶段：[New_ACH](New_ACH.md)。唯一执行队列：[TODO](TODO.md)。

## 本次实际执行的验证

环境：macOS arm64，Rust/Cargo 1.98.1，cargo-nextest 0.9.145。下表为 OB-044 后续的 OB-012 本轮运行，不继承 handoff 的通过数字。

| 检查 | 本次结果 | 边界 |
| --- | --- | --- |
| 根 release nextest，render | **1823 passed，4 skipped，0 failed** | OB-012 renderer/robots/interceptor/networkidle 切片后的完整 workspace 门禁；不是完整客户端或多平台资格 |
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
| 官方 Playwright Python **1.58.0** async/CDP | 连接、建页、导航、title、label fill、role click、DOM 结果、evaluate、关闭均成功 | 单个本地合成页面；填写 `audit` 后输出确为 `audit`，evaluate 为 2；没有官方包补丁；没有完整 API/多平台认证 |
| raw WebSocket/CDP 和 CLI 本地夹具 | 复现下节 6 项行为 | 使用本次 render 二进制；不涉及真实网站或凭据 |

4 个 skip 对应源码中的 ignored tests：`benchmark_sparse_cascade_hot_path`、`concurrency_5_does_not_abort_v8`、`http_control_plane_unblocked_during_long_js`、`fetch_intercept_concurrency_5_does_not_abort_v8`。没有把忽略项算作通过。

旧 observer fixture 的注释声称不断观察新 sentinel，但代码只 observe 一次，追加后没有重新观察或滚动；固定 Chrome 对照也未达到其期望。这证明失败来自 fixture 的循环假设，不能证明所有 IntersectionObserver 语义正确。修订版将范围明确为固定初始页，并在首次真实相交通知中完成五批加载；参考 Chrome 复验为 `io:50`，Obscura 聚焦用例为 1/1、完整门禁为 33/33。OB-037 已关闭；更广的 IntersectionObserver 语义仍由其他资格测试覆盖。

OB-044 本轮删除 CLI/serve 的 `--stealth`、`--user-agent`、`OBSCURA_STEALTH`、scrape worker 转发和嵌入 API 开关；CDP、MCP、CLI、Page 与独立 runtime 的产品路径默认使用 primp。Cargo `stealth` feature 只保留为空兼容别名，不能改变行为；发布矩阵不再生成有无 stealth 的组合。强制路径同时保留子资源缓存合并，将 JS fetch/XHR 的完整响应体和网络事件写入 CDP/MCP 观察面，并以原字节或明确 Base64 保存二进制响应。persona 在 BrowserContext 初始化时同时确定 primp 传输配置与 JavaScript 身份，并在 context 生命周期内保持不变；运行中的 CDP User-Agent 覆盖明确不支持。全出口盘点、完整 persona 编译器和 Docker 干净构建仍由 OB-012/015/016/042 跟进，OB-044 保持未关闭。

OB-012 本轮移除 renderer 的隐式 `ureq` 图片出口，将默认资源缓存改为只消费已注入字节；robots.txt 改走 persona-owned primp，并验证其与导航使用同一 persona User-Agent；每个 Page 的 primp 与其 detached Worker 共享 transport in-flight 计数，兄弟 Page 保持隔离，`networkidle` 合并观察互斥的预发送/CDP 拦截与 primp 传输阶段；native policy `RequestInterceptor` 保留完整二进制请求体。审计同时确认 standalone runtime/module loader 及公开 `ObscuraHttpClient` 仍有 reqwest 出口，CORS/取消/体积限制、最终线上请求头、重复与非 UTF-8 头、完整响应体保存、Beacon/下载仍有缺口，因此 OB-012 只记录为部分完成。

本次未运行：Linux 原生验证、完整官方客户端矩阵、WPT、24h 长稳、受控性能/TLS/H2 测量、Docker 构建、Southwest/ZG 现场流程和报价对照。没有新的生产发布或部署结论。

### 构建与证据定位

本次 render CLI SHA-256：`49a8b12d38c36cfed31e2075f7522131f4e6202eb5bc826c213ec7cde73f4e41`（122127456 bytes）。

- 根 Cargo.lock SHA-256：`3a8256dacf51bd2ac0f6491f97360d845094a34b77ce2669b54244de075057fb`。
- runtime/Cargo.lock SHA-256：`bb4ee97177ebcb698cf1b9dc082aa889d9bbf9ed4ba55620c4e61713c893ebc0`。
- benchmark revision：`2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`（`gster/obscura-benchmark`），与当前 CI pin 一致；旧失败归因使用 `6ebac8293d7477f59e837768bfd4e74173f04f1c`。
- 本机原始日志、合成探针和 JSON：`/tmp/obscura-doc-audit-20260919/`；临时文件不入 Git，也不保证跨设备或长期存在。仓库内保留命令、结果和源码入口，外部原始证据缺失时须重跑。

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
| Cookie 往返失真 | CookieInfo 不含 host_only；save/load 经该视图，导入写 host_only=false | 源码确认，未另运行子域往返探针；OB-011 |
| Cookie 上下文不完整 | get_cookie_header 仅接收 URL；SameSite 字段存在但该选择路径无 site/method 上下文；未捆绑完整 PSL，HashMap 遍历不保证发送顺序 | 源码确认；不能只因能读写 Cookie 就宣称完整语义；OB-011/012 |
| IndexedDB 部分实现 | 请求、upgrade transaction、索引/游标已有代码与回归；abort/commit 空体，数据库存于 JS realm 的 Map | 源码确认，完整事务/生命周期未验证；OB-041 |
| 身份配置分叉 | BrowserContext 网络与 JS 已收敛到同一 `StealthProfile`，但独立 runtime Persona、CDP Browser.getVersion 和平台 preset 仍有各自字段 | OB-014/015/016/031；还没有完整 persona 编译器或已认证的 Linux/macOS persona |
| 控制面边界 | CDP 无内建鉴权；server 有 unbounded_channel，连接限制不等于消息/队列预算 | OB-034；不以 loopback 替代完整授权/背压 |

主要源码入口：[CDP dispatch](../crates/obscura-cdp/src/dispatch.rs)、[server](../crates/obscura-cdp/src/server.rs)、[Runtime 域](../crates/obscura-cdp/src/domains/runtime.rs)、[Page 域](../crates/obscura-cdp/src/domains/page.rs)、[Browser 域](../crates/obscura-cdp/src/domains/browser.rs)、[bootstrap](../crates/obscura-js/js/bootstrap.js)、[CookieJar](../crates/obscura-net/src/cookies.rs)、[BrowserContext](../crates/obscura-browser/src/context.rs)、[runtime Persona](../runtime/src/browser.rs)。源码审阅和以上实验不是完整安全审计。

raw CDP 复验次序：创建并 attach target → Page.navigate 本地页面 → Runtime.evaluate 写入全局/建立带 getter 的对象 → Page.getFrameTree/createIsolatedWorld → 带新 contextId evaluate → getProperties → 回读 getter 计数。unknown Log 方法、Beacon 服务端计数、第二连接 target 清单和双进程 storage-dir 分别独立观察。所有 sessionId 使用 attach 返回值。

## 不应重复立项的已有实现

根 workspace 有九个 crate，独立 runtime 另有 workspace。Page 和 Worker 已有独立 isolate；Worker 已有线程、V8 序列化、transfer、终止、预算和网络转发。Worker 使用 detached primp 独立连接池。旧“同页模拟 Worker”“普通路径尚未修”“先迁入 primp”已失效。

Cookie 键已区分 domain/name/path，已有 host-only、HttpOnly 写保护和过期导入回归；应修剩余语义，不从旧快照重新实现已有保护。iframe/fragment、Text 更新、表单传输、预检和原生输入已有修复，现有回归位于 browser/page、JS runtime/bootstrap、net 与独立 runtime tests；迁移时保留这些能力。保留成果与回归，不据此夸大完整标准资格。

页面所有的产品传输当前强制使用 primp；standalone runtime/module loader 的 reqwest fallback 与 `wreq_client` 兼容别名仍是待删除迁移项。V8 依赖通常取预构建 archive；`V8_FROM_SOURCE` 才走源码构建，本项目还会执行 bootstrap snapshot 生成。旧“首次必编译 V8、固定五分钟”不是准确的构建契约。

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
| [Connect](Connect-Puppeteer-or-Playwright.md)、[Playwright](Use-with-Playwright.md)、[Puppeteer](Use-with-Puppeteer.md)、[Interception](Intercept-and-modify-requests.md) | 参考用法；修正客户端等待值、Puppeteer fill 和全覆盖承诺；完整客户端仍未认证 |
| [Persistence](Persist-cookies-and-storage.md)、[Live view](Watch-agent-sessions-live.md) | 重写实际持久化和连接所有权边界，撤下无效跨连接 viewer 教程 |
| [Rust library](Use-as-a-Rust-library.md)、[Isolated runtime](Use-the-isolated-runtime.md) | 底层嵌入与迁移期私有协议分开；移除外部消费仓库当前状态猜测 |
| [First fetch](Your-first-fetch.md)、[Extract](Extract-data.md)、[Markdown](Markdown-extraction.md)、[MCP](Use-the-MCP-server.md) | 保留的 CLI/MCP 使用参考；与尚未实现的统一身份/传输目标分开 |

外部契约核查：官方 [Playwright CDP](https://playwright.dev/python/docs/api/class-browsertype#browser-type-connect-over-cdp) 明确连接模式及保真度限制；[Cargo features](https://doc.rust-lang.org/cargo/reference/features.html) 的 additive 语义用于裁剪设计。它们不认证 Obscura，也不代替固定版本运行。

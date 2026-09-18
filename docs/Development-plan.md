# Obscura 下一阶段开发计划

日期：2026-09-18  
审阅基线：`gster/obscura@7de5448bd3af0ac176aedc5f3220167460334e0e`  
执行清单：[开发 TODO](TODO.md)

## 1. 目标与边界

**下一阶段的交付目标是一个可独立运行、可验证复用既有 Playwright Python 代码的 CDP-first 浏览器内核，不是继续扩展自有 Python SDK。**

生产调用链固定为：

```text
既有 Python 自动化程序
  → 未修改的 Playwright Python + 对应 driver
  → connect_over_cdp / CDP
  → 薄宿主 + obscura-cdp
  → 共享原生执行能力
  → Page / Realm / DOM / Input / Network / Storage / Render
```

首期主支持 Linux x86_64 与 macOS arm64；其他 macOS/Linux 架构按运行证据增加。内核不包含业务平台、任务调度、行业适配、账号托管或 agent 编排。Unblocked 是独立开发测试工程，不进入生产依赖图。不建设 Windows/移动端产品或跨 OS persona 投影。

CDP 是兼容接口，不是完整 Chrome 或完整 Playwright 兼容保证。官方文档把 CDP 连接限定于 Chromium 系浏览器，并提示其保真度低于 Playwright 自有协议；Obscura 的替代实现必须自行给出固定客户端版本的资格证据。[E10]

停止自有 SDK、重复私有自动化 IPC 的新增产品能力。已有 `runtime/` 和 `bindings/python/` 在迁移窗口保留必要修复与回归；其中可复用的原生输入、等待、网络观察、资源限额和启动安全逻辑迁入共享层，不能随包装层一起删除。[E1][E2]

## 2. 当前状态：实现、记录与资格分开

本计划基于固定提交的源码抽样、架构调用边界、测试代码、仓库测试记录及 Actions 查询。没有在本次审阅环境编译或执行 Obscura、官方 Python/CDP 测试、WPT、抓包或性能实验。源码抽样不等于逐行安全审计。

| 领域 | 基线事实 | 下一步判断 |
| --- | --- | --- |
| 产品入口 | 根 workspace 仍有 MCP/CLI；`runtime/` 是独立 workspace；Python SDK 使用自有协议，不启动 CDP | 架构收敛尚未完成；先建迁移护栏，后删除 [E1][E2] |
| CDP | 已有 Target、Page、Runtime、DOM、Network、Fetch、Input 等处理器、session/context 归属和并发测试；部分域整域返回空成功 | 不是从零实现 CDP；须按方法、参数、事件和失败语义清理占位行为 [E3] |
| 页面生命周期 | 路由到另一个 target 不再主动拆除其他页面的 JS isolate；已有 context 清理测试 | 不把旧的“切页必丢 JS 堆”当作现存定论；补官方客户端和长期运行验证 [E3] |
| Worker / iframe | 已有独立 Worker 执行、消息序列化/预算/终止与近期 iframe 生命周期修复 | 保留成果；完整 module Worker、host-object clone、frame 语义仍需能力清单与测试 [E4][E5] |
| Cookie | 已采用 domain + name + path 键，已有 host-only 字段与脚本侧 HttpOnly 写保护 | 不重复立项已存在的修复；优先审计元数据往返、请求上下文、过期与排序 [E6] |
| 网络与身份 | 普通/stealth 路径并存；stealth 使用 primp；独立 runtime 与 CDP 的身份配置入口不同 | 统一有效配置和请求策略，不只统一版本字符串 [E2][E3][E7] |
| 渲染/输入 | 已有独立 render 层及回归工程，原生输入能力可复用 | 不裁掉 Playwright 动作依赖的几何、命中测试与 frame 语义 [E1][E2] |
| CI | PR CI 有根 workspace 构建、nextest、stealth 网络测试和固定 benchmark 对照 | 仍需覆盖正式 Python/CDP、独立 runtime 迁移回归及 macOS；根 workspace 测试不能替代独立 workspace [E1][E8] |

### 测试账本

`docs/Southwest-handoff.md` 在该提交记录：release render nextest **1758 passed、4 skipped**，独立 runtime **184/184**，最近 iframe 相关覆盖 **91/91**，obstacle course **32/33**。这是已有记录，不是本次重跑结果，也不是对所有平台/feature 的认证。[E5]

2026-09-18 对审阅 SHA 的 Actions API 查询返回 0 个 workflow run。不能据此推断测试失败，也不能把本机记录写成该 SHA 的 CI 绿灯。[E9]

`observer-intersection` 必须单独定性：固定 benchmark revision，比较基线、候选与参考浏览器，判断 fixture 假设还是引擎行为。不得删掉测试、改常量结果、隐藏 skip，或把 32/33 写成达标。当前 CI 固定的 benchmark revision 是 `6ebac8293d7477f59e837768bfd4e74173f04f1c`；本机历史运行是否使用同一 revision 仍须核对。[E8]

## 3. 阻断事项与执行顺序

优先级表示阻断层级，不代表所有 P0 可以并行改动。

| 阻断事项 | 处置 | 对应任务 |
| --- | --- | --- |
| 缺少固定构建与正式客户端资格证据 | 建立 engine、driver、schema、fixture、persona、命令及产物摘要账本 | OB-001、025、026、040 |
| 正式 Python/CDP 主链尚未被证明可替代 SDK | 先得到最小端到端测试与差分结果，再迁移共享能力 | OB-021、027、028、029、032 |
| 方法存在与语义支持混淆 | 精确支持表；只允许经过测试的特定 no-op；未知方法/参数明确失败 | OB-027、030、031 |
| 状态、出口与资源边界需要闭环 | Cookie 无损往返、请求上下文、代理 fail-closed、CDP 背压与断连行为 | OB-011、012、013、034 |
| 裁剪可能带走有效行为 | 先迁出回归与安全职责，再删 SDK/IPC、MCP、旧 CLI 产品面 | OB-003、004、005、006、007、036 |
| 整体质量门槛不完整 | 恢复有效 obstacle 门禁，补双平台客户端和长稳证据 | OB-023、037、038 |

**第一轮先做 OB-001 + OB-025，随后 OB-026 + OB-027。** Cookie 元数据与过期语义测试可独立并行；在真实 CDP 端到端证据建立前，不进行大规模 runtime 删除、`bootstrap.js` 拆分或网络栈替换。

## 4. 阶段与退出条件

### G0：冻结基线与重建证据

锁定根/独立 runtime 的 `Cargo.lock`、Rust 工具链、V8 依赖、目标系统、字体/CA 输入和 benchmark revision。固定一版 Playwright Python 及其实际 driver，记录依赖摘要；不在没有运行证据时指定“最新版兼容”。

建立 `tools/unblocked/` 最小开发工程，先只做固定客户端场景、CDP 调用记录、结果清单。参考分为 Chrome 正常启动、Chrome CDP 连接、Obscura CDP 连接；需要区分引擎差异与控制方式副作用。原始数据默认本地保存，入库仅合成/脱敏 fixture 与摘要。

**退出条件：** 两个主平台有可重现构建入口；基线测试明确列出 passed/failed/skipped/not-run；得到最小客户端调用序列、已通过项及首个未通过项（如有）；32/33 已建立独立跟踪，未被包装成通过。

### G1：打通正式客户端与共享执行层

先覆盖连接、context/page 创建、导航、evaluate、CSS/role/label locator、fill/click、响应等待与读取、关闭。最低要求是同一份未修改的官方客户端场景同时在参考 Chrome 和 Obscura 上运行，差异定位到 CDP 契约或内核语义，而非另写一套 Locator 来绕过。

按实际调用完善 Target/session、execution context、utility world、remote object、事件订阅与生命周期。支持表至少含 `method + parameter pattern + scope + result/event + error + fixture`；不允许整域兜底成功。保留客户端真正需要的 Debugger/Log/Emulation 等初始化调用，不按域名一刀切。

抽取已有原生输入/网络观察/截止时间等能力；薄宿主只承担配置、启动、就绪、诊断和清理，不提供第二套自动化协议。在正式替代主链通过以前，旧 runtime 仍按现有支持面维护；替代通过后才降为短期 DEV-ONLY 对照，再移除包装层。

**退出条件：** 最小正式客户端场景全绿；context/句柄归属、导航失效和输入不自动重放有回归；新主链不依赖 Python SDK 的私有协议；迁移前后共有语义测试保持有效。

### G2：统一网络、状态、身份与故障边界

网络策略与 transport 分层。统一导航、子资源、fetch/XHR、Worker、重定向和拦截改写的 URL/出口校验、credentials、Cookie、CORS、响应体限额及取消行为。存在共同函数不等于全部路径已经覆盖。[E7]

Cookie 使用带版本的内部持久化格式，保留精确作用域与生命周期元数据，协议视图和内部状态分开；SameSite/分区判断使用完整请求上下文。旧格式不能凭缺失字段扩大权限，迁移策略必须显式并经过测试。

身份配置由一个 EffectivePersona 驱动页面、frame、Worker、HTTP 头、传输声明与 CDP 可见配置。客户端 override 要么通过一致性校验，要么明确拒绝；只改 UA 或版本标签不算通过。开发优先用宿主同 OS 的参考配置。

CDP 采用有界请求/响应队列、消息尺寸限制和超载行为，保留现有连接数量/延迟消息上限。定义单 owner、断连/取消、关闭与资源释放契约。当前按 WebSocket 连接持有会话，不能承诺重连恢复存活页面。控制端口默认 loopback；远程访问须显式授权、认证并验证 Host/Origin，具体实现以正式客户端兼容测试为准。[E3]

`APIRequestContext` 与 `route.fetch()` 等客户端侧网络请求单独列为迁移边界，不默认视为经过 Obscura transport/代理。出口验证要包含调用方 driver 和辅助工具，不能只测页面主请求。

**退出条件：** 两条网络后端通过同一安全/语义 fixture；状态往返、跨 context 隔离与多页面持续执行通过；错误不会伪装成空成功；代理失败没有未授权直连；超时/取消后不重放输入且资源有界。

### G3：完成裁剪与开发工程隔离

在能力和测试迁移后，删除 `bindings/python/` 的产品 SDK、自有自动化 IPC、MCP、通用批量抓取产品入口以及非目标平台发布配置。`runtime/` 按责任逐项迁出再删除；不能因目录名整体删除原生能力。

这里的“旧 worker”指私有运行进程/任务包装，**不指 Web Worker**。保留 `AGENTS.md` 开发规范、核心 watchdog/SSRF/DOM 环保护、输入/渲染/存储与共享网络回归。必要的诊断命令迁为开发工具，不作为生产产品面保留。

**退出条件：** Cargo 依赖图、测试入口和发布清单证明删除真实发生；生产包不含 SDK/IPC、MCP、实验室、Chrome 或 Python 测试依赖；断开旧路径后仍能运行官方 Python/CDP 场景。仍被编译或暴露的冻结代码继续承担安全维护，不能以“冻结”代替删除或修复。

### G4：正式资格、性能与发布

补齐 frame/Worker、弹窗/多页面、文件输入/下载、状态导出和取消的支持矩阵。性能用同场景、相同网络/字体/persona/viewport、相同等待与采集边界交替运行，报告启动、首次可用、动作耗时分布、RSS、线程/FD、空闲 CPU、长稳增长及构建成本。包含 Python driver 和宿主的总资源，不只测内核裸进程。

CI 保留现有 PR 低权限、可信基线脚本、固定 action revision、依赖审查和基线/候选对照，新增必要的 macOS/正式客户端资格。main/tag 产物应绑定明确通过的 engine SHA，文档 PR 的 policy 通过不能作为引擎发布证据。

**退出条件：** 有效 obstacle 门禁达标；必需回归无失败、必需场景无 skip；完整能力/客户端/平台资格清单、校验和、依赖和许可证清单、回滚方法齐全。未认证 API 与平台显式列出，不再使用无条件的“drop-in/full compatibility”表述。

## 5. 首批可独立评审的 PR

| 顺序 | 内容 | 依赖与验收 |
| --- | --- | --- |
| A | 固定基线、测试账本、正式 Python 版本、最小 fixture | 不改引擎语义；先确认真实基线与失败位置 |
| B | CDP trace/支持表与官方客户端回归 | A；有失败先复现后修复，不靠换自有客户端消除失败 |
| C | Cookie 元数据往返与过期语义回归、必要修复 | A；可与 B 并行，复用现有 path/HttpOnly 回归 |
| D | Target/world/句柄/事件最小闭环，逐项消除占位成功 | B；按一个语义缺口一个回归拆分 |
| E | 原生能力迁移、薄宿主、旧路径删除 | D 与相应边界测试；不能先删后补 |
| F | 网络/身份/CDP 安全边界、双平台资格与裁剪审计 | 与 D/E 分步合入；发布前全部阻断项关闭 |

任务编号、依赖与每项完成定义见 [TODO](TODO.md)。阶段可以有受控重叠，但不能用排期替代退出条件。

## 6. 执行与验收规则

当前仓库可用的验证入口如下；工具链和系统依赖先按基线清单准备。输出保存到仓库外，并记录执行 SHA 与 feature。

```bash
# 根 workspace：render 全量测试与 CLI 构建
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo nextest run --locked --release --features render --no-fail-fast
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo build --locked --release -p obscura-cli --bins --features render

# stealth 构建及受影响路径回归另行执行，不能用普通构建代替
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo build --locked --release -p obscura-cli --bins --features render,stealth

# runtime 仍被保留期间，必须单独验证其独立 workspace
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo nextest run --locked --release --no-fail-fast)
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 \
  cargo build --locked --release)
```

obstacle course 在固定 revision 的 companion repo 中执行，`OBSCURA_BIN` 使用绝对路径。其目录不在主仓库内，不把未安装的工具写成已有命令。`tools/unblocked/` 和正式 Python/CDP 测试入口由 OB-008/026 创建；创建前统一记为 not-run，不给出虚构的可运行命令。

仅本地 fixture 确需访问 loopback 时，按现有测试设计显式放行，限制在该测试进程中；不要全局打开 private-network 或禁用 TLS 校验。按 `AGENTS.md` 使用 release nextest，不用 `cargo test` 替代 V8 相关门禁，不进行全仓库 rustfmt。

每个实现任务关闭时提供：变更 SHA、最小失败用例（适用时）、修复后测试、受影响路径/feature、平台、基线对照、跳过项、资源影响以及支持清单更新。声明支持要有“实际执行”的证据；仅存在函数、测试文件、字段或协议响应不算完成。

## 7. 真实站点调查的使用边界

Southwest 的历史记录用于提取通用浏览器缺口，不替代确定性 CI。该提交中的购物接口仍有 403，根因未确定；Worker 修复、固定输入回放一致、页面无异常和单个请求 200 都不等价于业务成功。[E5]

优先用合成 fixture 定义 Beacon、Worker/frame、会话和请求时序语义；真实授权流程用于补充验收。不得加入域名特判、网站专用等待、导入其他浏览器会话凭据或把换出口当作无证据的默认修复。新实验保留可比较的参考控制，并按实际数据结果而非表面页面状态验收。

## 8. 来源与维护

以下源码链接固定于本次审阅 SHA；未来提交必须重新验证相关调用链。旧 handoff 中的工作目录、未提交状态和中间测试数不覆盖 Git 提交与最终证据。安全缺陷的具体复现和敏感原始数据按 `SECURITY.md` 私下处理，本计划只保留工程工作项。

- [E1 根 workspace](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/Cargo.toml)；[独立 runtime](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/runtime/Cargo.toml)
- [E2 Python SDK 当前契约](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/bindings/python/README.md)
- [E3 CDP dispatch](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/crates/obscura-cdp/src/dispatch.rs)；[server](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/crates/obscura-cdp/src/server.rs)；[Browser 域](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/crates/obscura-cdp/src/domains/browser.rs)
- [E4 Worker 实现](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/crates/obscura-js/src/worker.rs)；[兼容记录](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/docs/Worker-compatibility.md)
- [E5 最近实现与测试记录](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/docs/Southwest-handoff.md)
- [E6 Cookie 实现](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/crates/obscura-net/src/cookies.rs)
- [E7 普通网络实现](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/crates/obscura-net/src/client.rs)；[stealth 实现](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/crates/obscura-net/src/stealth_client.rs)
- [E8 CI](https://github.com/gster/obscura/blob/7de5448bd3af0ac176aedc5f3220167460334e0e/.github/workflows/ci.yml)
- [E9 Actions 查询](https://api.github.com/repos/gster/obscura/actions/runs?head_sha=7de5448bd3af0ac176aedc5f3220167460334e0e&per_page=5)：查询结果是日期相关快照，不是永远为空的断言。
- [E10 Playwright Python CDP 文档](https://playwright.dev/python/docs/api/class-browsertype#browser-type-connect-over-cdp)，核验日期：2026-09-18。

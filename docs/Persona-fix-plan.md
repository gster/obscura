# Persona 必配、注入与一致性修复计划

状态：已实施并通过最终门禁；提交前需满足独立审查无 blocker。本文既保留实施前审计，也记录 OB-005/014/015/016/031/044 的本次交付；平台资格和旧 Python SDK/private runtime 的删除仍以 [TODO](TODO.md) 为准。

## 0. 2026-09-20 实施结果

本次以 `07c7b8cb27922d5bb5245a03fa056e8060e876fa` 为最终实施基线，完成以下收敛：

- `obscura-net` 成为唯一权威 persona 模块。版本化 `PersonaSpec`、内置预设和外部 JSON 经同一流程编译为私有、不可变、带稳定摘要的 `EffectivePersona`。
- CLI、CDP、MCP、Rust facade、scrape worker、页面、frame、Worker、module loader、网络和存续的独立 runtime 均显式消费同一 persona 快照；缺失配置不再选隐式默认身份。
- BrowserContext 创建后不能逐字段修改身份；UA 和身份 header override 继续 fail closed。CDP 新 context 可继承启动快照，也可通过 `obscuraPersona` 注入新快照，并由 `Browser.getPersona` 读取摘要。
- 进程级时区与 ICU 主语言在首个产品入口启动前冻结；同进程冲突组合在 context 注册或 V8 启动前拒绝。其他 persona 字段仍可按 context 隔离。
- proxy、SSRF、cookie/storage 持久化和 tracker blocking 保持在会话或隐私策略中，不混入 persona。旧 runtime 复用共享协议，但 `bindings/python` 未新增功能，因为其产品方向是由 OB-006 删除。
- 本次只证明实现和本机回归，不把具名 profile、可解析配置或 DNT 字段写成跨平台、完整指纹、匿名性或价格公平资格。OB-014 及受控业务实验仍是独立工作。

下文第 1 至 3 节是实施前基线审计，路径和行号不代表完成后的源码位置；第 4 至 7 节是本次采用的设计与验收边界。

## 1. 实施前 PR 讨论基线

2026-09-20 再次核对，以当前工作目录的当前分支最新提交为唯一代码基线（C）：

| 项目 | 当前值 |
| --- | --- |
| 工作目录 | `/Users/zg/work/obscura` |
| 当前分支 | `refs/heads/main` |
| 基线 C | `3fb94d6e1b190480ed5a21fe1a9c31a2d4c7766c` |
| 提交说明 | `fix(cdp): report scripted request failures` |
| 提交时间 | 2026-09-20 10:15:50 +08:00 |
| 本地 origin/main 引用 | 与 C 相同，ahead/behind 为 0/0 |
| 工作区变化 | 仅 `docs/README.md` 和本计划；没有未提交源码改动 |

以下段落记录计划形成时的事实：当时 main 已对齐 origin/main，仅更新审查口径，尚未执行源码修改或本轮门禁。它不描述上方实施结果。

本 PR 不以旧本地 main `1ffce719…` 为起点，也不要求先合并该分叉。其他 worktree 和分支仍是需要考虑的并行工作与可复用来源，但其未提交修改不能算作 C 已具备的能力，不能自动进入本次修复。历史清点移至附录，实施前应再次确认相关工作是否已有新提交。

四个曾清理的 worktree 已按用户要求恢复到原路径和原源码状态；恢复没有撤销当前 main 的对齐。恢复资料位于 `/Users/zg/.codex/backups/obscura-cleanup-20260920-120041`，原 main 保存在 `codex/backup-main-before-sync-20260920-120041`。这些是恢复信息，不是本 PR 的待实施任务。

## 2. 已确认的产品约束

1. 每个 BrowserContext 必须在创建时获得一份完整、合法的 persona。
2. 创建成功后，persona 在 context 生命周期内不可变。
3. 所有启动入口、派生页面和浏览器网络路径使用该 context 的配置。
4. 缺失、无效或引擎不支持的配置，必须在创建成功之前报错。
5. Persona 是可注入的协议，后续可以持续接入不同的 persona。
6. 用户可以显式选择具名预设，由预设补齐合法字段，不必逐项填写。
7. 最终配置归并为 context 持有的一份快照，各模块不得自行读取环境或选择默认身份。

“必配”禁止按宿主 OS 自动猜测、使用常量兜底或失败后换一个档案。显式选择预设后按预设补齐字段是允许的。环境变量若保留，只能作为启动输入，不能成为运行时第二个配置来源。

对外身份应符合所选 persona；宿主 OS 本身不是一个对外身份声明。跨 OS persona 是否支持，取决于实际能力与资格，不能仅因 JSON 可解析或存在 TLS preset 就认定支持。

## 3. 当前基线审核结论

### 3.1 已具备能力与剩余缺口

| 审查面 | C 的实际状态 | 本 PR 工作 |
| --- | --- | --- |
| 传输基线 | 根产品路径已迁移到强制 primp；旧 stealth feature 为兼容用途 | 保留现有成果，禁止恢复可选传输分支 |
| persona 构造 | 有 `with_persona_profile`，其余便捷构造器仍隐式使用默认 profile | 所有可用 context 构造要求完整 persona |
| 根产品入口 | CLI/CDP/MCP/Rust facade 未接入 persona 构造器 | 统一必填、解析、校验与传递 |
| 独立 runtime | 已有独立 Persona 类型和生产调用，但构造后仍补写身份字段 | 迁入共享协议与校验；存续入口使用同一冻结快照 |
| 旧 profile 表 | 根 crates 中 select_profile 无生产调用者 | 删除失效选择机制，不能重新作为第二个默认来源 |
| 身份可变性 | UA/platform/language/WebGL 等字段公开可写 | 私有化 persona，移除重复可写字段 |
| CDP 覆盖 | 已拒绝 UA 和部分身份头覆盖 | 保留保护，补全 context 创建、其他覆盖入口与诊断语义 |
| 原始内容请求 | original dump 另建默认 primp client | 无需 JS 也必须使用所选 context 的网络身份 |
| JS/runtime 懒绑定 | 独立 JS 初始化与 ensure_persona_transport 仍生成默认身份 | 从构造时注入的 persona 取值，不再自行选档案 |
| 版本与环境 | CDP 版本硬编码 Linux/Chrome145；TZ 与 geolocation 仍有环境路径 | 统一投影，验证时区隔离、设备与渲染能力 |

### 3.2 生产入口与证据位置

以下路径和行号均对应 C：

- `crates/obscura-cli/src/main.rs:712`：fetch context。
- `crates/obscura-cdp/src/server.rs:311`：serve 模板 context。
- `crates/obscura-cdp/src/dispatch.rs:208`：独立 CdpContext 初始化。
- `crates/obscura-cli/src/worker.rs:57`：scrape worker。
- `crates/obscura-mcp/src/lib.rs:83`：MCP。
- `crates/obscura/src/browser.rs:27,35`：Rust facade。
- `runtime/src/browser.rs:6509`：独立 runtime 的 persona 生产调用。

`with_persona_profile` 在 crates 与 runtime 中共有四个出现位置：context 构造器定义、context 测试、独立 runtime 测试和独立 runtime 生产调用。因此保留此前纠正：不能宣称“全仓零生产调用”。根产品入口未接入与独立 runtime 已部分接入是同时存在的现状。

其他缺口位置：

- `crates/obscura-browser/src/context.rs:47,57,68,82,182`：默认 persona 构造。
- `crates/obscura-js/src/runtime.rs:1049`：独立 JS 默认身份。
- `crates/obscura-js/src/ops.rs:548-561`：默认 transport 懒绑定。
- `crates/obscura-cli/src/main.rs:1079-1093`：original 请求旁路。
- `crates/obscura-cdp/src/domains/network.rs:52-84`：已有身份头保护与 UA 覆盖拒绝。
- `crates/obscura-cdp/src/domains/browser.rs:100`、`crates/obscura-cdp/src/server.rs:834`：固定 Linux UA。

### 3.3 本 PR 范围

重点是将现有 persona-owned transport 向上接成可注入协议，并让 context 必填、校验、冻结和投影形成完整契约。原来建议的“先合并旧本地 main”不再作为前置工作；reqwest 删除、强制 primp 和已落地 CDP 保护不重复实现。

独立 runtime 在 C 中仍存在，因此必须处理它与共享协议的关系。其他分支已有的 runtime 删除、identity 集中定义或 response-stage 拦截工作可另行协调复用，但不因代码存在于别处，就将其当作当前基线已完成。若实施时这些工作先合入 main，再推进基线并缩减本 PR 范围。

## 4. 目标设计

### 4.1 协议、预设和有效配置

```text
具名预设 / 外部配置 / Rust 调用者注入
                   |
       统一解析、补齐、校验与能力检查
                   |
       完整、已验证的 EffectivePersona
                   |
          BrowserContext 创建成功
                   |
       页面 / frame / worker / 网络 / 渲染
```

采用版本化的数据协议。暂用 `PersonaSpec` 表示输入，`EffectivePersona` 表示完整生效配置，与 New_ACH / OB-015 保持一致；具体接口名称待实现设计定稿。

- 协议含 schema 版本、persona ID/revision，以及浏览器、传输、语言时区和设备信息。
- 具名预设和外部配置走同一条校验路径。错误包含字段或能力原因；未知版本、无效值、不支持的传输/设备组合均拒绝。
- 相同语义只保存一个权威值，再生成 UA、UA-CH、HTTP 和 JS 投影；有意义的品牌顺序等档案数据应显式保留。
- 记录规范化配置摘要，用于 worker 传递和诊断。摘要不代表档案已认证，也不代替能力检查。
- 新 persona 复用已有引擎能力时，只新增数据与注册，不改各入口。需要新 TLS 或 runtime 能力时，须实现对应能力并验证，不以更换身份字符串冒充支持。
- 外部注入是启动时提供数据，不要求动态加载任意执行代码。若提供 Rust provider 接口，只在创建时取配置并冻结，不在请求热路径回调 provider。

身份、能力、隐私策略和会话配置保持分工：proxy、SSRF、预算、cookie 持久化、tracker policy 不混入 persona。旧 runtime 的 `tracker_blocking` 应迁入明确的策略配置，不能通过 persona 绕过底层保护。独立 CapabilityManifest/PrivacyPolicy 等类型是否在本切片建立，按必要范围决定。

先建立唯一权威模块。是否新增 `obscura-persona` crate 取决于共享类型的依赖方向，不作为前置硬要求；避免 browser → JS → net 之间形成循环依赖。这修正了前一版直接建议新增 crate 的表述。

### 4.2 创建与冻结

核心构造接口必须要求 persona；无 persona 的 new、builder、Default 或便捷构造器不能创建可用浏览器 context。只移除枚举 Default 不够，旧接口中也不能改为硬编码常量。

Context 私有持有不可变快照，各身份字段只读；初始化身份不通过公开 setter 补写。页面在首次脚本、preload 或请求之前获得完整投影。独立 JS runtime 同样要求有效身份，懒绑定 transport 只能消费已提供配置，不能重新选择。

先做 schema、字段关系与引擎能力校验，再登记 context、启动 worker ready 或开放服务。身份配置错误必须提前拒绝；实际 DNS、连接和服务端错误仍属于请求阶段。保留连接池延迟初始化，避免为配置校验提前扫描 CA 或建立连接。

### 4.3 所有输入和派生路径

| 路径 | 目标行为 |
| --- | --- |
| CLI fetch / original / 辅助出站请求 | 共用已配置的 context 网络身份；无需 DOM 的请求不必创建 JS |
| serve | persona 校验与模板 context 创建在对外就绪之前完成 |
| scrape / 独立 worker | 初始化消息携带完整快照，worker 校验后才能接受任务；不按自己的环境再次选预设 |
| MCP | context 初始化必配；所有 tab 继承 |
| Rust facade / 嵌入 | 创建接口必须传 persona；无便捷默认旁路 |
| 独立 runtime / 现有 Python 包装 | 删除前通过共享协议适配；迁出已有校验和行为测试 |
| frame / popup / 浏览器 worker / 新导航 | 使用所属 context 的冻结快照 |
| isolated_copy | 复制相同 persona，cookie/storage 隔离语义保持 |

旧 `OBSCURA_PROFILE` 数字索引、自动 rotation 与分散的 timezone/geolocation 读取统一迁移。具体新参数名、是否保留环境变量输入及优先级是待定接口细节；无论采取何种输入方式，都不能有缺失时的自动身份兜底。

### 4.4 不能只靠配置字段解决的能力

**时区。** 当前 CLI 修改进程 TZ。先用两个不同时区 context 交替执行 Date、Intl、frame/worker 的 fixture 验证实际隔离能力。不得在活跃 context 之间反复改全局 TZ。若 V8/ICU/TZ 存在进程冲突，沿用 New_ACH 的分进程方向；隔离能力完成前，创建时明确拒绝不支持的组合。同进程多时区不是未经验证的前提。

**设备、字体与渲染。** screen/DPR、硬件与 WebGL 声明必须与现有实现能力对应，并检查字体资源的可用性。deviceMemory 等对外值不能直接等同宿主物理参数。viewport 调整可作为页面窗口状态；不得通过设备模拟偷偷改变冻结身份。媒体查询、JS 几何、截图与布局使用同一生效状态，保留本地布局缓存的正确失效与性能。

**网络。** 覆盖主文档、子资源、fetch/XHR、模块、worker、预检、redirect、下载和拦截后继续请求。统一保护身份头，允许协议本身按请求种类省略某些头，不能机械要求每个请求携带相同头集合。保留 SSRF、credentials、CORS、代理、证书、预算与取消规则。

引擎只能约束自己拥有的请求路径。官方 Playwright driver 的 APIRequestContext/route.fetch 不会因本次修改自动使用 Obscura persona，沿用现有不认证为受保护出站路径的范围说明。

### 4.5 CDP 与运行时覆盖

建议采用以下语义，仍属于待确认设计，不是当前支持声明：

- 启动 persona 必填；标准新建 context 请求未指定 persona 时可继承已配置的启动快照。
- 提供明确的 Obscura 扩展创建接口，为新 context 注入不同 persona；不假定标准客户端会发送扩展字段。
- context 创建后不接受身份变更。为兼容客户端重复配置，可评估将一致值处理为无操作；冲突值明确拒绝。
- `/json/version` 与 browser 级 Browser.getVersion 固定报告启动身份；带 context ID 的诊断接口返回该 context 生效配置与摘要。
- Page/Network 输出只依赖所属 context。不同 context 使用不同 persona 不构成不一致。

Browser 级版本与不同 context persona 的兼容范围需要客户端实测；若客户端无法支持某种组合，应在创建时声明限制，不能错误声称支持。JS 引擎真实版本等实现元数据与模拟浏览器身份需明确区分，不按 Chrome 版本任意拼接。

## 5. 实施顺序与交付物

本节映射现有任务，不新建并行执行队列。

| 阶段 | 工作 | 完成判据 |
| --- | --- | --- |
| P0 固定实施基线 | 以当前 main 的 C 为起点；确认协议与并行工作接口，若 HEAD 更新则记录新 SHA 并审核增量 | 不默认合并旧 main 或其他 worktree；保留 C 已有传输和 CDP 行为 |
| P1 协议与校验 | 迁出独立 runtime 有价值逻辑，建立协议、预设、外部注入与能力检查；映射 OB-014/015 | 非内置名称 persona 通过同一流程；非法配置创建前拒绝 |
| P2 核心收敛 | 必填构造器、私有快照、移除 net/JS 默认身份、取消构造后补写 | 不能无 persona 创建可用 context，不能分别写身份字段 |
| P3 一致投影 | 网络、JS、frame/worker、时区、screen/字体/渲染；映射 OB-016 | 首个请求与脚本正确；并存 context 不互相污染 |
| P4 入口和协议 | CLI/MCP/facade/worker/runtime/CDP 创建与诊断；映射 OB-003/004/005/006/031/044 | 每个存续入口必配，特殊请求路径与派生规则一致 |
| P5 清理与资格 | 删除失效选择器，迁移旧参数、示例和 harness；完成回归 | 文档与实际支持匹配，全部必要门禁通过 |

本轮已按 P1 至 P5 接通共享协议、核心冻结、投影和存续入口。其他分支成果仅在确定范围并审核后复用，没有把未审查的 worktree 修改自动纳入；Python SDK/private runtime 删除仍由 OB-006 单独完成。

## 6. 验收矩阵

1. 必配：所有入口缺配置失败；无成功 context、ready 或实际出站请求。编译期接口约束覆盖 Rust 构造；外部输入覆盖运行时错误。
2. 注入：内置预设与非内置名称外部配置使用同一验证和投影；未知 schema/transport/capability 不静默降级。
3. 冻结：创建后改变文件、注册表或环境变量，既有 context 不变化。已传入对象不能被外部引用修改。
4. 隔离：A/B 不同 persona 交替导航、创建 frame/worker，输出不串用；进程不支持的组合在创建前拒绝或进入已验证隔离路径。
5. 继承：新页面、popup、frame、worker、重导航、isolated_copy 与 scrape 子进程保持相同配置摘要及对应行为。
6. 网络实证：本地 HTTP 服务记录最终请求头，TLS fixture 检查所选档案；JS 同时采集 UA/UA-CH、locale/timezone、screen/DPR、WebGL 等。TLS 扩展集合相等不是完整指纹相同的证明。
7. 禁止绕过：旧构造、UA/header 覆盖、拦截修改、CDP 设备模拟、original、独立 runtime 和模块请求均覆盖。
8. 浏览器行为回归：请求方法、跨域凭据与 CORS、session 隔离、输入、布局和绘制；对实际改动路径使用 C 的既有测试并补必要 fixture。其他分支的测试只有明确复用时才纳入，不能假定都已在 C。
9. 保留当前基线成果：primp-only、原始头、body spool、Fetch pause owner 隔离、redirect 与取消的成功/失败终态。
10. 其他本地成果：若采用 response-stage WIP，覆盖响应暂停/继续、body stream 和 owner 生命周期；若采用 worker/identity/字体或 runtime 删除成果，承接其通用回归并验证 persona 初始化不会使其退化。已有测试源码不等于本轮测试通过。

实现阶段运行 focused release nextest，然后执行全量 release nextest 与精确 CLI release build：

```bash
cargo nextest run --release --features render --no-fail-fast
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --no-default-features
```

身份与传输在 render/no-render 都验证。C 已删除运行时 `--stealth`，其验收不能再要求传该参数。独立 runtime 存续且依赖被修改时，按它的独立 workspace/toolchain 运行对应 build/nextest，根测试不替代它。

另需障碍课 33/33、官方 Playwright Python smoke/协议画像、确定性渲染 fixture 和真实站点 top/bottom 捕获。C 已弃用 Puppeteer 资格；本计划不增加新的 Puppeteer 资格承诺。输出放仓库外，不提交生成的截图或报告。性能旧新交错对比，固定页面、viewport、网络、settle 和捕获条件，报告分布与资源使用；约 ±10% 噪声范围内不作确定优化结论。

### 6.1 本轮实际验证结果

以下结果来自最终源码和冻结候选二进制，不借用 C 的历史通过数：

| 门禁 | 结果 |
| --- | --- |
| persona/context/CDP/CLI 聚焦 release nextest | 通过；内置/外部配置、必配失败、冻结、继承、override 拒绝和进程级时区/locale 冲突均有覆盖 |
| 根 workspace release nextest，render | **1944/1944 passed，4 skipped，0 failed**；最终源码以 4 个测试线程复跑 |
| 独立 runtime release nextest | **185/185 passed，0 skipped** |
| exact CLI release build，render | 通过；冻结二进制 SHA-256 `10dca946d36267c3613199ea5ca37379409c95ff8b286fe1c835c921b176e732` |
| exact CLI release build，no-default-features | 通过；冻结二进制 SHA-256 `eff85327d056a6532cefa2289f208be1add1bd42efeb960049a6805cdc86975d` |
| CLI 实测 | 缺 persona 在请求前退出；内置 preset 与外部 JSON 在 render/no-render 均正确投影 UA、language、timezone、screen 和 DPR |
| obstacle course | **33/33**，`--runs 1 --warmup 0` |
| 官方 Playwright Python / CDP profile | smoke 通过；协议日志满足 **37/37** 方法画像 |
| deterministic render fixtures | Obscura/Chromium **66/66** 成对捕获成功；检查器仅报告当前 Chromium 148 的 4 条既知字体/表单参考断言差异 |
| representative real-site captures | top、bottom 均 **15/15** 捕获；有效 fidelity 分别 **10/15**、**11/15**；空内容与 capture-boundary unstable 项按 harness 规则排除 |
| 五轮交错 latency/RSS smoke | 审查前候选的 DOM build：latency +0.1%，RSS -0.2%；storage：latency -0.7%，RSS +0.1%；均低于失败阈值和约 ±10% 噪声边界；后续修复不在这些 CLI 热路径内，不把该结果外推为跨平台结论 |
| 工作树卫生 | `git diff --check` 通过；生成的截图、报告和临时兼容 wrapper 均在 `/tmp`，未进入仓库 |

验证期间两项失败直接促成修复：旧 screen 测试仍依赖运行时随机池，已改为显式 persona screen/DPR；Windows preset 曾误沿用 private runtime 的 `America/New_York`，障碍课以 32/33 捕获后恢复为根产品 persona 基线 `Europe/Berlin`，再跑为 33/33。独立审查先后发现多 worker 环境优先级、persona preload 时序、locale 校验、直接构造器进程冻结、CDP 设备覆盖、frame/transport 继承、重复 region 等非规范 locale 仍可通过，以及直接 runtime/frame 的 DPR 投影丢失八类 blocker，均已修复并增加对应回归。官方 Playwright smoke 随后进一步验证：viewport 可调整，但 `screen`、DPR 和 mobile 身份仍保持 persona 冻结值。两个 300ms 级测试在高并发首跑抖动，单独和低并发全量复跑均通过。

## 7. 与既有设计的关系和待定项

- [New_ACH](New_ACH.md) 已要求统一权威模块、能力与策略分离、进程全局冲突时隔离。此次方案遵循这些约束。
- OB-014 的平台资格与 OB-015 的可注入协议相关但不同：有配置、能运行、经过认证应分别说明。
- OB-015/016 承担统一校验和投影；本计划不另建一套 persona 编译器与之竞争。参考采集与复杂编译在开发侧，生产仅做必要的轻量加载、规范化与能力检查。
- 不将 Windows TLS preset 存在或 Mac persona 可选择，等同于跨 OS、字体、图形或完整 TLS 资格成立。
- 本轮 v1 接口已固定：CLI `--persona` 高于 `OBSCURA_PERSONA`，worker 只消费父进程传入的冻结 JSON；内置预设和外部 JSON 都经同一编译器。协议字段扩展、预设注册机制和是否拆分独立 crate 留给后续版本。
- CDP 新 context 默认继承启动 persona，也可通过 `obscuraPersona` 注入完整配置；`Browser.getPersona` 提供摘要诊断。browser 级版本仍展示启动身份，不据此承诺任意多 persona 客户端组合均已取得资格。
- 时区属于 persona，并与 ICU 主语言作为进程级身份冻结；同进程冲突在 context/V8 成功创建前拒绝，不宣称多时区同进程隔离。viewport 是页面状态可变；物理 screen、DPR 与 mobile 身份由 persona 冻结。字体和跨平台投影资格仍需 OB-014 的独立证据。

## 8. 复核命令

当前分支是本次基线来源。检查时先记录 HEAD，随后对该完整 SHA 读取，避免并行提交造成前后版本混用。

```bash
git symbolic-ref --short HEAD
git log -1 --format=fuller HEAD
git rev-parse HEAD origin/main
git rev-list --left-right --count HEAD...origin/main
git status --porcelain=v1 --untracked-files=all
git grep -n 'with_persona_profile' 3fb94d6e1b190480ed5a21fe1a9c31a2d4c7766c -- crates runtime
git grep -n 'select_profile' 3fb94d6e1b190480ed5a21fe1a9c31a2d4c7766c -- crates runtime
git diff --stat 3fb94d6e1b190480ed5a21fe1a9c31a2d4c7766c HEAD
git worktree list --porcelain
git branch -vv
```

若需要评估并行工作，对相应 worktree 分别检查 HEAD、status、staged/unstaged diff 与 untracked 文件；对未挂载分支读取固定 SHA。根目录 status 不覆盖其他 worktree。远端 freshness 是另一项检查，fetch 不等于把当前分支移动到远端。

源码永久链接：[当前基线 context](https://github.com/gster/obscura/blob/3fb94d6e1b190480ed5a21fe1a9c31a2d4c7766c/crates/obscura-browser/src/context.rs)、[当前基线独立 runtime](https://github.com/gster/obscura/blob/3fb94d6e1b190480ed5a21fe1a9c31a2d4c7766c/runtime/src/browser.rs)。

## 附录：先前分叉与 worktree 审查记录

以下保留旧审查结果供协调和复用，不是当前 main 的能力声明或必须合入的任务。历史 R 等于当前 C；历史 L 是原 main `1ffce719b845965b5db52fe0b771c201188b2855`；共同祖先 B 是 `5e8b6d17ff6b0701baa6169f2ab911354429c315`。原 main 的 16/22 分叉现已不适用于当前分支。以下表格中的分支指向与工作区状态均为当时快照，后续可能变化。

### A. 清理前 worktree 清单

`origin/main` 是本地保存的远端跟踪引用，对应服务器 `refs/heads/main`，不是一个工作区。当前目录实际 checkout 的是本地 `refs/heads/main`，upstream 为 `origin/main`。其他 worktree 不都在 main；配置 upstream 为 origin/main 也不等于分支名是 main。

| Worktree 路径 | 实际分支 | HEAD | 相对 R 的独有提交（本地/远端） | 本次观察的工作区状态 |
| --- | --- | --- | --- | --- |
| `/Users/zg/work/obscura` | `main` | `1ffce719…` | 16 / 22 | 上版文档产生 README 修改和本计划新文件；源码无未提交改动 |
| `/private/tmp/obscura-request-audit-src` | detached HEAD | `a752298b…` | 0 / 40 | 9 个 tracked 修改，1 个 untracked `audit.rs` |
| `/private/tmp/obscura-worker-scheduling` | detached HEAD | `a752298b…` | 0 / 40 | 7 个 tracked 修改，1 个 untracked `worker_queue.rs` |
| `/Users/zg/.codex/worktrees/ob-012-primp-egress/obscura` | detached HEAD | `3fb94d6e…` | 0 / 0 | 18 个 tracked 修改，包含 15 个源码/测试文件和 3 个文档 |
| `/Users/zg/.codex/worktrees/ob-027-cdp-profile/obscura` | `ob-027-cdp-profile/obscura` | `76893831…` | 0 / 16 | 干净 |
| `/Users/zg/alma/worktrees/obscura/resident-fish` | `read-project-docs` | `3fb94d6e…` | 0 / 0 | 4 个 tracked 修改，7 个 untracked 文件；主要是提案、说明和采集工具 |

另有未挂载到已登记 worktree 的本地分支 `respective-canid`，HEAD 为 `8275f6fe4ed6dcb0bcbba87c6c09b20916047883`，相对 R 本地独有 43、远端独有 36 个提交，upstream 也为 origin/main。该分支包含 identity 集中定义、宿主字体接入及删除独立 runtime/Python SDK 的工作，不能漏掉。

其他完整 SHA：`a752298bb0bee7128308affa87f8969d39cc9fbc`、`768938310ed86b76efe68818cc95cde854503260`。各 worktree 此次观察的 index 均无 staged diff。工作区可能被其他任务继续修改，以上是读取时快照，实施前应重查并冻结明确的输入版本；相同 HEAD 不代表相同工作区源码。


### B. 原 main 分叉中的修复候选

L 相对 B 实际修改 13 个文件，874 行新增、81 行删除。16 个独有提交包含 merge，不等于 16 组独立修复。

| 本地成果 | 主要位置 | 集成后的回归要求 |
| --- | --- | --- |
| 标准请求方法规范化，保留扩展方法大小写 | bootstrap.js、runtime.rs | `fetch_normalizes_only_standard_request_methods` |
| 跨 origin 重定向清除凭据、降级方法后清除 body headers | ops.rs | 在最终 primp 请求路径逐 hop 检查 |
| 中间跳 CORS 与 preflight 错误顺序 | ops.rs、runtime.rs | 不跟随未授权 redirect；明确失败时序 |
| browser attachment session 唯一性与独立 detach | CDP target.rs | 多 attachment 相互不影响 |
| textarea Enter 尊重选区 | CDP input.rs、textarea_enter_selection.rs | 选区替换、光标位置与事件结果 |
| 布局与文本 shaping 缓存优化 | render dom.rs、inline.rs、lib.rs、style.rs | viewport/DPR/字体变化仍正确失效，性能不回退 |
| overline / line-through 绘制 | paint.rs、text_decoration.rs、公开 fixture | 装饰线渲染保持 |

这些是本地已存在代码，不是本轮已跑绿的证据。R 也有大量网络重构，部分行为可能已有等价实现；集成应比较语义并保留回归，不机械重复旧补丁，更不能恢复 reqwest 来保留旧实现。

R 的 primp-only、请求/响应原始头、response body spool、Fetch pause 隔离与脚本失败生命周期同样必须保留。重点重叠区是 ops/runtime/bootstrap、CDP target 与网络事件路径。文档不把两个分叉直接拼接视为已经完成集成。

### C. 其他 worktree / 分支的相关工作

| 来源 | 实際读取到的相关内容 | 对 persona fix 的处理 |
| --- | --- | --- |
| `ob-012-primp-egress` 未提交 diff | scripted response-stage interception、ContinueResponse、body stream owner/读取限制、导航期间 body store 访问和相关测试；HEAD 等于 R，但源码超出 R | 网络与 CDP 设计必须考虑这套正在进行的改动；不能按 R 整文件覆盖。配置注入应穿过请求和响应两阶段，保证 context/session 归属与取消规则 |
| `obscura-worker-scheduling` 未提交 diff | WorkerConfig/identity 传递、worker 独立执行与生命周期、队列资源限额、getter 失败和网络观察测试；runtime tick 防饥饿 | 将身份继承、线程释放和资源约束纳入兼容检查。基底落后 R 40 个提交，需辨认哪些工作已被 R 吸收，不重做或整体搬回旧实现 |
| `obscura-request-audit-src` 未提交 diff | 临时 native audit、transport 观察、面向特定脚本的实验等待控制 | 作为诊断材料识别；不把脚本路径匹配和实验等待控制引入通用 persona 产品路径，不将诊断 build 当产品资格 |
| `ob-027-cdp-profile/obscura` | 干净，停在 persona-owned transport 提交 `76893831…`，已是 R 的祖先 | 没有额外工作区补丁；该成果已在远端基线内，不重复实现 |
| `resident-fish` / `read-project-docs` | 未跟踪的 `docs/Persona-activation-proposal.md` 与本次输入同题，另有 TLS 采集工具和业务讨论文档；根生产源码未修改 | 原提案实际也存在于此 worktree；本计划保留其来源并修正范围结论。工具源码存在不等于本轮重新运行采集 |
| `respective-canid` | `crates/obscura-net/src/identity.rs` 集中身份常量、DeviceProfile 和 seed；context/locale/WebGL 绑定；宿主字体接口；HEAD 删除独立 runtime/Python SDK | 已有可复用工作，不能宣称“本地完全没有统一身份或清理实现”；但仍默认 HOST_PROFILE、读取旧环境、保留可选 stealth 和可写字段，不满足必配注入协议。将可复用数据转为显式预设，并评估其删除/迁移成果 |

`respective-canid` 中的宿主常量与字体配置不是新的全局默认答案。其 `select_device()` 再次调用 `select_profile()`，说明仍需一次解析冻结；自动 rotation 场景必须避免多次选取。集中定义的字段值、字体行为和平台资格尚未由本轮重新验证。

因此“独立 runtime 仍存在”只适用于 R、L 及清单中的相关 worktree；在 respective-canid 已有删除提交。最终集成若采用已审查通过的删除方案，应承接有价值的 persona/输入/worker 回归，不先重建已删除入口再删除。不能预设整个本地环境都还处于同一迁移阶段。

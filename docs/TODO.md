# Obscura 开发 TODO

日期：2026-09-18  
审阅基线：`7de5448bd3af0ac176aedc5f3220167460334e0e`  
总体方案：[下一阶段开发计划](Development-plan.md)

## 使用规则

本清单是执行队列，不是已完成能力声明。OB-001 至 OB-036 保持原有设计任务含义，新增 OB-037 至 OB-040；任务拆分用后缀，不复用旧编号。所有任务均保持未勾选，已有代码只减少工作量，不能替代正式验收。

P0 为当前主链或发布阻断；P1 为随后完成的能力与质量工作。P1 项进入声明支持的范围后，同样成为发布门禁。每个任务开始时由维护者指定 owner，填写实施 SHA；完成时附测试证据后才勾选。安全问题的详细复现按 SECURITY.md 私下处理。

**立即执行顺序：OB-001 + OB-025 → OB-026 → OB-027 → OB-028/029/021。** OB-011 和 OB-037 可在冻结基线后并行。未建立替代主链前，不开展大规模删除。

## A. 立即执行与主链阻断

### OB-001 · P0 · 冻结构建与测试基线

- [ ] 完成并附验收证据。

依赖：无。入口：根/runtime 的 Cargo.toml、Cargo.lock、工具链、AGENTS.md、CI。  
动作：在 Linux x86_64 与 macOS arm64 固定源码、工具链、feature、系统/字体/CA、benchmark revision；分别运行根与独立 workspace。  
完成：提交不含敏感数据的机器可读清单，关联命令、产物摘要、passed/failed/skipped/not-run；1758/184/32/33 等历史记录不当作新运行结果。

### OB-025 · P0 · 固定官方 Python 客户端及迁移范围

- [ ] 完成并附验收证据。

依赖：无，可与 OB-001 同步。入口：新开发测试工程，既有 Python 调用代码的授权样本。  
动作：锁定 Playwright Python、driver 与依赖；清点 connect、context/page、locator、frame、network、文件、关闭/取消 API。区分必需、可延后、不支持。  
完成：官方客户端未修改；测试依赖不进入 Rust 生产包；明确 APIRequestContext/route.fetch、launch/connect 与 CDP 的边界。

### OB-026 · P0 · 建立最小 CDP 记录与差分 fixture

- [ ] 完成并附验收证据。

依赖：OB-001、OB-025，开发目录可与 OB-008 同一 PR 创建。入口：拟建 tools/unblocked/。  
动作：同一合成页面分别跑参考 Chrome 正常启动、Chrome CDP、Obscura CDP；记录方法、参数、事件、session/context、错误与时序。  
完成：记录器能保留因果与相对顺序、规范化随机 ID；失败应能稳定复现并定位首个分歧，全通过则保留通过证据，不人为制造失败；凭据和正文脱敏，采集开关不改变验收结果。

### OB-027 · P0 · 发布 Automation CDP Profile 并清理占位成功

- [ ] 完成并附验收证据。

依赖：OB-026。入口：crates/obscura-cdp/src/dispatch.rs、domains/、types.rs。  
动作：逐方法标记 implemented / validated-no-op / unsupported / unverified，覆盖参数组合、返回、事件、作用域、错误。审计整域空成功和 Browser 域占位行为。  
完成：未知方法、非法参数明确失败；no-op 仅是有依据的精确允许项；版本号/空对象/初始化成功不能充当资格证据。首个正式 Python smoke 进入必需测试。

### OB-028 · P0 · Target、session 与页面生命周期闭环

- [ ] 完成并附验收证据。

依赖：OB-027。入口：CDP dispatch/server、domains/target.rs、domains/page.rs。  
动作：覆盖 flattened session、自动/显式附加、暂停后恢复、create/dispose context、双页面持续执行、导航/关闭/断连及事件归属。保留现有不拆除其他页面 isolate 的实现。  
完成：操作 B 页面不损坏 A 的 timer/闭包/Promise/句柄；context/session 无串用；关闭后事件不再发往失效 session；明确断连不保证恢复。

### OB-029 · P0 · 正式客户端 utility world 与对象句柄

- [ ] 完成并附验收证据。

依赖：OB-027，可与 OB-028 协作。入口：CDP domains/runtime.rs、JS runtime/frame。  
动作：验证官方 locator 注入脚本、主 world/utility world 隔离、对象组释放、跨 frame 路由与导航失效。  
完成：有实际 realm 与句柄生命周期证据，不只生成不同数字 ID；未知/陈旧/跨属主对象明确失败；role/label locator 不依赖自写替代客户端。

### OB-021 · P0 · 原生共享执行层，消除第二套产品协议

- [ ] 完成并附验收证据。

依赖：OB-027，随 OB-028/029 分步推进。入口：runtime/src/automation.rs、runtime/src/browser.rs、obscura-browser、CDP。  
动作：列出原生输入、等待、资源观察、限额和启动保护的归属；逐项迁移到内核共享层并由 CDP 调用。  
完成：正式客户端最小端到端场景不经过 SDK 私有 RPC；迁移项均有原路径/新路径对照。不得重写完整 Playwright Locator 产品。

### OB-011 · P0 · Cookie 请求上下文与无损状态往返

- [ ] 完成并附验收证据。

依赖：OB-001；网络接入与 OB-012 协作。入口：obscura-net/src/cookies.rs、CDP cookie_params/Storage/Network、BrowserContext。  
动作：区分内部持久化与协议视图；审计 host-only、expiry/Max-Age 优先级、同名多 Path 顺序、SameSite/分区、旧格式迁移；覆盖 clone、save/load、导入导出、HTTP 与 document.cookie。  
完成：状态往返不改变原语义；同等属性不同顺序结果一致；匹配使用完整请求上下文；原有多 Path 与 HttpOnly 写保护回归保持通过。分区能力未实现则不得宣称支持。

### OB-034 · P0 · CDP 访问、背压、断连与不重放

- [ ] 完成并附验收证据。

依赖：OB-001、OB-027。入口：obscura-cdp/src/server.rs、薄宿主。  
动作：在已有连接/延迟消息上限之外，补入出站队列 count/bytes、帧大小、慢读与超载；核查认证、Host/Origin、默认 loopback；定义 owner 与关闭契约。  
完成：慢客户端/大消息/导航中断不会无界堆积；明确错误且资源回收；授权客户端可正常连接；未获准客户端被拒绝；取消/重连不自动重放输入。

### OB-037 · P0 · 定性并恢复 obstacle 门禁

- [ ] 完成并附验收证据。

依赖：OB-001。入口：固定 revision 的 companion benchmark 与 observer-intersection 对应引擎路径。  
动作：核对本地历史与 CI fixture revision，在基线、候选和参考浏览器上验证 IntersectionObserver 假设，制作最小用例。  
完成：给出 engine/fixture/环境归因与修复依据；有效门禁达标并可重跑。不得删除失败、修改固定答案或隐藏 skip；fixture 修订须重新跑基线与候选。

## B. 网络、运行语义与身份

### OB-010 · P0 · 生命周期不变量集合

- [ ] 完成并附验收证据。

依赖：OB-001。入口：browser/page、JS runtime/frame、CDP 测试。  
完成：明确 session、top-level context、document/realm 不同寿命；同源导航的 sessionStorage、跨标签隔离、listener/timer/Promise、iframe/Worker 所有者销毁均有回归；旧 issue 仅作为复验线索。

### OB-012 · P0 · 统一网络语义与出口策略

- [ ] 完成并附验收证据。

依赖：OB-001、OB-011，与 OB-021 协作。入口：net/client.rs、stealth_client.rs、stealth_transport.rs、JS ops/worker、page。  
完成：导航/子资源/fetch/XHR/Worker/重定向/拦截改写共用一致的 URL、凭据、Cookie、CORS、代理、证书、取消及体积限制；普通与 stealth 路径用同一组 fixture 验证；代理失败无未授权直连，重复头和二进制体不静默丢失。

### OB-013 · P0 · 安全、终止和资源预算

- [ ] 完成并附验收证据。

依赖：OB-001，与 OB-034 协作。入口：watchdog、ops、DOM tree、Worker queue、host。  
完成：保留 panic=unwind、DOM 环防护、SSRF 与 V8 终止；覆盖无限脚本、卡住原生操作、超限内存/响应体/线程/FD、取消；分别定义进程内保护与 OS 隔离，不承诺“Rust 自动安全”。内核故障不能当成功 null。

### OB-030 · P0 · CDP 观察副作用

- [ ] 完成并附验收证据。

依赖：OB-027、OB-029。入口：Runtime/getProperties/console、网络与诊断订阅。  
完成：开关观察前后业务结果一致；getter/proxy、异常、console 对象、网络订阅有边界测试；不因检查对象而任意执行 getter，不把 Worker 对象句柄当作父 isolate 句柄。

### OB-014 · P0 · 固定 macOS/Linux 参考 persona

- [ ] 完成并附验收证据。

依赖：OB-001、OB-009。入口：参考采集、现有 persona/transport 配置。  
完成：每个平台至少一个同 OS 的完整参考清单与版本；记录网页/HTTP/图形/字体/时区/硬件表面和支持限制；不复制宿主不支持的 Windows/移动身份进入首期资格。

### OB-015 · P0 · Persona 编译与加载校验

- [ ] 完成并附验收证据。

依赖：OB-014。入口：开发侧 compile-persona、宿主配置加载。  
完成：PersonaSpec 到 EffectivePersona 的字段关系、schema、摘要和能力校验可复现；生产仅轻量加载校验；缺失或矛盾配置明确拒绝，不默默选另一个 profile。

### OB-016 · P0 · Persona 原生一致投影

- [ ] 完成并附验收证据。

依赖：OB-012、OB-015。入口：页面/frame/Worker 配置、net、CDP Browser/Emulation。  
完成：JS、HTTP、时区/屏幕、Worker 与 transport 声明来自同一有效值集；CDP 固定版本返回与有效配置不再各自漂移；未做到的 wire 差异显式列出，不声称仅凭名称完全匹配 Chrome。

### OB-031 · P0 · 客户端 override 冲突规则

- [ ] 完成并附验收证据。

依赖：OB-027、OB-015，与 OB-016 协作。入口：CDP Network/Emulation/BrowserContext。  
完成：locale/timezone/UA/viewport 等实际客户端参数可验证应用或明确拒绝；页面/frame/Worker/HTTP 保持一致；认证清单记录 connection options。

### OB-038 · P0 · 保留并完善 Worker/frame 资格

- [ ] 完成并附验收证据。

依赖：OB-001、OB-010；正式客户端覆盖依赖 OB-028/029。入口：JS worker/worker_queue/frame/runtime/bootstrap、browser/page。  
完成：独立执行、clone/transfer、终止、策略更新、队列预算、相对 URL、断开/重插入、旧文档引用与过期任务取消均有固定 fixture；module Worker、MessagePort/host-object clone、srcdoc/sandbox/WindowProxy 等逐项标明支持范围。不得重做已存在的 isolate 分离。

### OB-039 · P1 · Beacon 与其他 Web API 占位行为

- [ ] 完成并附验收证据。

依赖：OB-012、OB-038。入口：JS bootstrap/ops 与公共网络路径。  
完成：先审计能力清单、再用通用 fixture 补语义；Beacon 至少验证排队返回、body/content-type、凭据、取消/生命周期及出口策略；未实现不伪造成功。任何 API 修复均不能直接宣称解决真实站点 403。

## C. 迁移、裁剪与开发工具

### OB-002 · P0 · 依赖与构建成本基线

- [ ] 完成并附验收证据。

依赖：OB-001。入口：Cargo metadata/tree、构建/发布配置。  
完成：记录编译可达依赖、冷/热构建时间、磁盘、产物、两个 workspace 成本；区分生产、开发、冻结和待删除，不只比较可执行文件大小。

### OB-005 · P0 · 通用启动与初始化契约

- [ ] 完成并附验收证据。

依赖：OB-001、OB-025。入口：CLI/runtime 的配置、初始化、信号与清理代码。  
完成：网络/存储/persona/预算在首个页面和首个请求前生效；审计 V8、TZ、日志等进程级初始化的一次性与冲突行为；就绪、错误、退出码、信号与资源回收有结构化契约；不引入业务状态机。

### OB-008 · P0 · 最小 Unblocked 开发工程

- [ ] 完成并附验收证据。

依赖：OB-001，可与 OB-026 同 PR。入口：tools/unblocked/（待创建）。  
完成：先交付固定 fixture、记录/差分、结果清单与独立依赖锁；生产无 Hero、参考 Chrome、实验室服务或 Python 测试依赖；不一次建设大型实验平台。

### OB-009 · P0 · Chrome/upstream/fork 对照 driver

- [ ] 完成并附验收证据。

依赖：OB-008、OB-025。入口：开发侧 runner。  
完成：同一测试接口、固定版本与输入；区分 Chrome 正常启动/CDP 连接；每组记录控制方式和采集扰动；尚未跑的组合明确 not-run。

### OB-004 · P0 · 薄宿主替代旧 CLI 产品入口

- [ ] 完成并附验收证据。

依赖：OB-005、OB-021、OB-028/029 最小闭环。入口：crates/obscura-cli、CDP 启动、runtime 引导。  
完成：保留启动/安全/诊断/清理职责，删除通用批量抓取与导出产品逻辑；旧 CLI 的核心测试迁出后才删除；生产宿主不另带私有自动化协议。

### OB-003 · P0 · 删除 MCP 产品面

- [ ] 完成并附验收证据。

依赖：OB-002、OB-004，共享逻辑迁移完成。入口：crates/obscura-mcp、workspace、CLI、文档/发布。  
完成：MCP 不再可编译到生产依赖图或出现在产物中；共享浏览器回归保留；同步移除无效示例和专属测试，不把 AGENTS.md 当作 MCP 产品文件删除。

### OB-006 · P0 · 删除旧运行进程/抓取调度包装

- [ ] 完成并附验收证据。

依赖：OB-021、OB-004。入口：runtime/、bindings/python/、批量 scrape 与相关 examples/scripts。  
完成：私有协议/SDK 退出活跃产品与发布；有价值的测试迁为原生/CDP 回归；Web Worker、后台页面任务和必要的内核调度保留。对照工具只可短期 DEV-ONLY，删除条件写明。

### OB-007 · P0 · 删除非目标平台产品配置

- [ ] 完成并附验收证据。

依赖：OB-014、OB-016、OB-002。入口：发布 workflow、安装脚本、persona、示例。  
完成：首期仅发布已认证 macOS/Linux 组合；清理自有 Windows/移动产品入口与 CI，不盲改第三方 vendor 的通用平台代码，不删除规范引用。

### OB-036 · P1 · 裁剪结果与旧代码迁移审计

- [ ] 完成并附验收证据。

依赖：OB-003/004/006/007、OB-032。入口：依赖图、发布清单、文档、正式 Python 场景。  
完成：确认代码、依赖、测试、产物四个维度均符合边界；正式主链完全不依赖旧 SDK/IPC；保留冻结项列出用途、风险、责任人与删除条件。

## D. 完整能力、资格与发布

### OB-017 · P1 · 原生输入、事件与用户激活

- [ ] 完成并附验收证据。

依赖：OB-021、OB-029。入口：Input、JS 事件与 native bridge。  
完成：click/fill/select/check、中文及组合输入、焦点/滚动/命中、事件顺序与用户激活通过通用 fixture；JS 合成事件不冒充真实输入；role/label/name 由官方客户端路径验收。

### OB-018 · P1 · 布局、字体与截图资格

- [ ] 完成并附验收证据。

依赖：OB-014、OB-017。入口：render、render-repros。  
完成：固定字体、viewport、scale、settle 的几何/命中/文字/裁剪测试；截图非空与正确加载先验证；必要 canvas/SVG 场景有证据；像素距离不单独决定正确性。

### OB-019 · P1 · Storage 与状态导出

- [ ] 完成并附验收证据。

依赖：OB-010、OB-011、OB-028。入口：BrowserContext、JS storage、CDP Storage/Network。  
完成：覆盖 sessionStorage/localStorage 的 origin/top-level context/导航寿命及状态往返；不能把恢复 Cookie 当作恢复整个活跃页面；未支持的 IndexedDB/分区能力显式限制。

### OB-020 · P1 · 隐私策略与可观察一致性

- [ ] 完成并附验收证据。

依赖：OB-012、OB-016、OB-019。入口：ResourcePolicy、存储/网络配置。  
完成：策略显式、版本化，可解释被阻断资源与失败；不破坏必要 Web 语义或静默改变 Cookie/请求；控制页面、Worker 和第三方资源的出口观察均有测试。

### OB-032 · P1 · 未修改官方客户端端到端资格

- [ ] 完成并附验收证据。

依赖：OB-028/029/030/031/034，按支持范围纳入 OB-017/018/019。入口：开发侧 Python 场景。  
完成：必需 API 清单全部有通过证据，覆盖多页面、frame、locator、网络、上传/下载、取消、关闭；结果绑定 engine/persona/profile/client/driver/平台，不以 SDK 通过率替代。

### OB-033 · P1 · 客户端网络与文件边界

- [ ] 完成并附验收证据。

依赖：OB-025、OB-012、OB-034。入口：正式客户端场景、host 网络/文件策略。  
完成：APIRequestContext、route.fetch、下载/上传/截图等逐项定义执行位置与支持边界；出站代理覆盖包括 driver/辅助请求；文件路径授权、大小限制、取消和清理可验证。

### OB-022 · P1 · 构建与测试成本回归

- [ ] 完成并附验收证据。

依赖：OB-002、OB-004。入口：CI 与开发基准。  
完成：比较冷/热构建、依赖数、缓存/磁盘及测试时长；基线/候选相同工具链和配置；报告波动，不把一次样本当提升。

### OB-023 · P1 · 双平台长稳与整链性能

- [ ] 完成并附验收证据。

依赖：OB-032、OB-037、OB-038。入口：开发基准和受控运行环境。  
完成：测宿主+内核+Python driver 总资源；交替比较启动/动作 p50/p95、RSS、线程/FD、空闲 CPU、多轮 churn 与超时回收；不与构建混跑，不用 0 等待掩盖未加载。

### OB-035 · P1 · 协议升级与客户端回归

- [ ] 完成并附验收证据。

依赖：OB-026、OB-027、OB-032。入口：协议/profile/客户端版本清单。  
完成：升级 Playwright/driver/schema 时能列出方法参数和事件差分，跑现版与候选版矩阵；只认证实测版本，失败有回滚路径。

### OB-024 · P1 · 发布、供应链与上游同步

- [ ] 完成并附验收证据。

依赖：OB-013、OB-023、OB-035、OB-036。入口：release workflow、依赖审查、安装与发行说明。  
完成：保留可信 PR 隔离措施；产物绑定已验证 SHA/平台/功能、校验和、许可证与依赖清单，验证全新机器安装；公开限制/回滚办法；上游合并必须重跑本项目资格，不能默认兼容。

### OB-040 · P0 · 证据、CI 与文档事实统一

- [ ] 完成并附验收证据。

依赖：OB-001；随各阶段持续更新。入口：AGENTS/README/docs/SUMMARY、CI/发布 workflow、测试清单。  
完成：区分历史手工记录、当次本地结果、CI 结果和发布资格；消除失效“未提交改动”指引及无条件 drop-in 宣称；补独立 runtime 迁移期检查与正式客户端/macOS 必需检查；文档 PR 不充当引擎绿灯。删除路线完成后再移除其专属 CI/文档。

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
Evidence location (sanitized; no credentials or raw session data):
```

源码与结论的来源索引在 [开发计划第 8 节](Development-plan.md#8-来源与维护)。最终完成定义是支持清单内的能力可重现通过，而不是实现了多少方法、删了多少行代码或某个网站偶然成功。

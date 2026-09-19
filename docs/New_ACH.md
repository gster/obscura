# 新开发方向：独立浏览器内核

2026-09-19 整理。**这是目标设计，不是当前能力声明。** 当前实现和核验结果仅维护在 [SUMMARY](SUMMARY.md)，任务、依赖与验收仅维护在 [TODO](TODO.md)。原稿中基于可变 upstream main 的源码判断不再作为当前事实。

## 不变的根本目标

根本目标是：面向机票采购效率，完成对 RPA 友好、性能突出、在服务端可观察的行为与指纹上与固定参考 Chrome 一致，并具备强匿踪、反追踪和防身份标记能力的浏览器内核，降低身份关联被用于差异化报价、侵害消费权益的风险。CDP-first 和官方 Playwright Python 是实现路径，不替代这些产品目标。反追踪不等于主动设置 `DNT=1`。

将目标分为五项分别验收：RPA 公共 API 与正确结果；完整进程链性能；服务端实际可见的网络/行为/指纹一致性；跨站、跨会话与跨身份空间的关联抵抗；策略引起的兼容性成本。不能用一个“stealth 分数”或成功率替代其中任何一项。

身份标记包括服务端标识与 Cookie、Web Storage/IndexedDB、缓存验证器、连接/TLS 会话、指纹与行为关联。先固定威胁模型、对照 Chrome 和身份空间边界：会话内保持必要状态与一致性，不同独立身份空间不泄漏可复用标记。清理 Cookie 或强行 DNT 声明不足以实现此目标。公开登录身份、共享出口与订单资料等关联因素也必须进入测量边界。

价格比较需固定航线/日期/人数/产品、币种、渠道、时点/库存与网络条件，分别观察身份变量及报价结果。只有受控证据才能把价差归于身份因素；浏览器指纹一致不能单独证明已消除价格歧视。业务测量留在使用方，内核提供可验证的隔离、控制与脱敏证据。

## 产品边界

主要自动化入口是 CDP，首要客户端是未修改的官方 Playwright Python：

```text
调用程序 → Playwright Python + 对应 driver → CDP
         → 保留的 CLI serve / obscura-cdp → 共享原生能力
         → Page / Realm / DOM / Input / Network / Storage / Render
```

Rust facade 保留底层嵌入与测试能力，不发展另一套覆盖 Playwright 的高层 SDK。首期目标为 Linux x86_64 与 macOS arm64，分别原生构建、运行和认证；最低 OS/SDK/glibc、字体与网络配置由基线证据确定。其他 macOS/Linux 架构按需求与运行证据增加，不承诺 Windows、移动端或跨 OS persona。

不引入行业适配、账号/订单模型、任务调度、浏览器云、GUI 或 agent 编排。复杂表单、日期、跨源导航等以合成 fixture 验收；Southwest shopping 是重要业务验收场景，须取得不再 403 且真实返回有效查询结果的证据；它与通用 Chrome 行为一致性门禁同时成立，不能相互替代。业务查询参数与采集工具留在开发/使用方，不引入站点专属内核补丁。

## 保留、迁移与删除

| 处置 | 范围 | 前置条件 |
| --- | --- | --- |
| CORE | CDP、V8、DOM、网络/存储、Worker/frame、原生输入、布局/字体/Canvas/SVG/截图 | 维持安全与行为回归 |
| KEEP | MCP 与对诊断、自动化及 agent 接入有用的 CLI | 复用同一内核、persona、primp 和安全策略；逐命令保留有用途的能力，不维护第二套浏览器逻辑 |
| DELETE | 自有 Python SDK、配套 NDJSON runtime/私有动作 RPC、已无消费者的包装、非目标平台产品发布 | 下一步优先清理；先迁出独有共享能力与回归，验证官方 Python/CDP 替代调用，再删除代码、依赖及发行入口 |
| DEV-ONLY | 参考 Chrome、采集/差分/诊断工具、旧路径短期迁移对照 | 独立依赖锁，不进入生产构建或产物；对照到期移除 |
| FROZEN | 高级打印、screencast、非必需 DevTools/媒体扩展 | 仍编译或暴露的实现继续维护安全与必要回归 |

CLI 的 serve、诊断、截图和结构化输出有明确用途，保留；scrape/worker 包装按 agent 使用价值、消费者与维护成本评估，不因裁剪方向一并删除。Web Worker 与 CLI worker 是不同概念；不做移动产品不等于删除桌面规范中的 Touch/Pointer 类型。

不强制新增 `obscura-host` 产物。保留 CLI 作为启动/诊断入口，提取共享初始化，供 CDP、MCP 和 Rust 嵌入一致使用。迁移 `runtime/` 独有的输入、等待、V8 flags、时区、SSRF、watchdog、预算和退出职责后，删除 `runtime/` 与 `bindings/python/`；这是删除自有包装，不是删除 V8/JS runtime 或官方 Playwright Python。

裁剪需以代码引用、Cargo normal/build 依赖、测试迁移、发布清单共同证明。仅隐藏命令或使用 `default-members` 不算删除。锁文件受控更新，不顺便升级依赖；成本比较分开记录下载、冷编译、热构建、链接与测试，不预设性能收益。

## 生命周期与共享执行能力

区分 BrowserSession、顶层浏览上下文、DocumentEpoch、Frame/Realm 和 CDP session。切换 target 不销毁其他页面 JS 状态；合法导航使旧文档句柄失效，但按标准保留相应 sessionStorage。Worker 随 owner 生命周期管理，不能复用窗口全局语义。

原生执行能力先组织在既有 browser/CDP crate 中，不为分层强制新增 crate。协议适配负责参数、归属、结果与事件映射；共享层持有输入、网络观察、取消、资源限额和状态。不能复制第二套页面状态机，也不重写 Playwright Locator。

所有队列、句柄、响应体、Worker 和日志均有预算。长脚本不能阻止退出监管；拦截导航等待时仍须处理解除等待的 Fetch 命令。超时、JS 异常、权限拒绝、未实现、内核故障分别表达；不把 panic/终止转换为正常空成功，不重放状态未知的输入或写请求。

## Automation CDP Profile

Profile 是标准 CDP 的受限兼容集合，不是新协议或二进制 ABI。每项包含 method、参数组合、scope、前置状态、返回、事件因果、错误、限制和测试引用。

运行契约采用 `SUPPORTED / LIMITED / VERIFIED_NOOP / UNSUPPORTED`，实施状态另记待实现/待验证/已验证。有效 CDP 命令可以返回 `{}`，但整域兜底成功不合格；no-op 必须针对具体方法和状态，有参考及客户端证据。

首轮覆盖连接、default/new context、Target attach/auto-attach、暂停/恢复、导航、Runtime、句柄、utility world、CSS/role/label locator、fill/click、网络响应与退出。utility world 必须有真实隔离行为，不能只分配 context ID。对象检查不应无端执行 getter，内部句柄表不挂在页面可变全局对象上。

客户端资格绑定 engine、平台、persona、Profile、Playwright Python/driver 及 fixture revision。async API 为主，sync 单独 smoke；其他客户端单列资格。官方 CDP 连接有保真度限制，不能据“能连接”承诺全部 API。[Playwright 官方说明](https://playwright.dev/python/docs/api/class-browsertype#browser-type-connect-over-cdp)

`launch` 改为宿主启动后 `connect_over_cdp`；公共页面 API 尽量复用。`APIRequestContext`/`route.fetch()` 的 driver 侧网络、文件上传下载、trace/HAR/video/状态导出逐项认证，不假设继承内核 transport 或任意远程文件传输能力。不修改官方客户端私有实现来掩盖缺口。

## 网络、状态与 persona

共同网络策略管理 URL/SSRF、请求上下文、credentials、Cookie、CORS、重定向、拦截改写、证书、取消和响应体限额；transport 管理 TLS/HTTP 连接。**经修复、校准并验证的 primp 是所有项目自有出站 HTTP(S) 请求的唯一传输后端。** 替换 wreq 兼容入口、reqwest 及其他独立请求路径，覆盖导航、子资源、fetch/XHR、Worker、Beacon、预检、重定向、下载，以及 CLI/MCP/Rust 嵌入的辅助请求；禁止失败时回退到另一后端。保留 Worker 独立连接池和既有证书、代理、DNS、取消及 SSRF 保护。primp 内部使用的 HTTP 协议依赖不等于另一条产品出口。

stealth 是不可关闭的底层基线。删除运行时 `--stealth`、环境开关及允许绕过它的产品 feature 分支，所有发布/容器/嵌入构建均包含该能力；迁移期旧开关只能提示弃用且不能关闭保护。隐私策略可以有明确、经过验证的作用域配置，不能借此恢复普通传输或关闭身份一致性。

官方 Playwright driver 自己发出的 `APIRequestContext`/`route.fetch()` 不会因内核改用 primp 自动改变。首期不认证它们作为受保护的站点请求路径，调用方改用页面请求或内核拦截履约；若以后纳入支持范围，须先证明由 primp 统一发出。开发侧参考 Chrome 保持原生网络以便对照，不作为产品出口。

Cookie 内部状态与 CDP 视图分离，持久化保留 host-only、domain/path、expiry、Secure/HttpOnly/SameSite、排序及版本。请求判断需要 initiator、top-level site、方法、导航/重定向和分区等上下文。localStorage、sessionStorage、IndexedDB 分别遵守自己的作用域和寿命；状态导出不是 JS 堆快照。IndexedDB 已有部分实现，后续重点是事务/索引/持久性资格，不能再次以返回空对象作为实现。

**强化统一 persona 是重点主线。** 一个权威模块编译、校验并投影有效身份，所有入口和 realm 消费同一配置，清理 runtime Persona、CLI profile、CDP 固定字段的分叉；模块边界由职责决定，不要求先增设 crate。设计对象分开版本化：`EffectivePersona`（身份）、`CapabilityManifest`（真实能力）、`PrivacyPolicy`（策略）、`SessionConfig`（隔离与预算）、`AutomationProfile`（协议契约）。这些是目标类型，并非声称全部已存在。

每个主平台先认证一个同 OS 模板，覆盖 UA/UA-CH、HTTP/TLS、语言时区、screen/viewport/DPR、字体、图形、输入和各 realm 的角色差异。JS 与网络使用同一有效配置；任意 UA 覆盖不能冒充完整身份切换。进程全局 V8/ICU/TZ 冲突时分进程，不在活跃会话中修改环境。

身份稳定与防追踪分开验收。可选的按站点扰动后置，并记录与参考浏览器的有意差异；不得扰动认证、金额、应用数据、密码算法、证书校验或响应正文。策略例外有范围、原因、测试和到期时间；站点拒绝不触发自动轮换或关闭保护。

## 开发实验室

`tools/unblocked/` 已建立首批开发侧基础：机器可读的构建/测试基线、官方 Playwright Python 1.60.0 范围清单、独立依赖锁和校验器。它们不进入 Rust 生产依赖，也尚未完成 Linux 资格或最小 CDP trace。下一批按 OB-026 增加合成 fixture、CDP 记录、差分与结果清单，后续再增加 persona 编译、网络采集、规约和资格报告。原始参考语料、Chrome、实验室服务和密钥不进入产品包。

参考组区分：最少控制的 headed Chrome、Playwright launch Chrome、同版 Chrome CDP、固定 upstream、fork。先区分控制方式差异，再定位内核差异。记录 OS/字体/GPU、viewport、网络、settle、客户端、工具和源码版本；直连与插桩分别对照。

与参考 Chrome 的行为差异是优先修补对象：对每个差异保留参考/候选输出、影响、最小 fixture、修复及回归；仅经解释和验证的隐私策略差异允许保留，不把意外差异包装成策略。差分支持精确值、集合、顺序、容差、关系和分布。结果区分 pass、fail、allowed_deviation、unsupported、inconclusive；不以综合分掩盖阻断项。TLS 摘要、Cookie 数量、接口数和截图像素距离均不能单独证明等价。

## 阶段与退出门槛

| 阶段 | 退出条件 |
| --- | --- |
| G0 基线 | 固定源码/锁/工具链/客户端/schema/fixture；真实测试账本、最小 CDP trace、裁剪与成本清单 |
| G1 自动化 | 未修改 Python 客户端最小链路、world/句柄/事件/安全回归通过；共享能力迁移后删除自有 Python SDK/私有 runtime，MCP 和有用 CLI 保留 |
| G2 Persona/传输 | 统一 persona、强制 stealth、全出口 primp 有证据；首个平台完整配置有证据，另一平台状态明确；覆盖规则不产生半更新身份 |
| G3 状态与安全 | 请求/存储隔离、权限、控制面认证、资源回收与隐私策略通过声明范围的测试 |
| G4 资格与性能 | 声明客户端/API/平台全部有证据；双平台长稳与完整进程链资源对照；Southwest shopping 对照验收通过 |
| G5 发布 | 产物摘要/签名、依赖与补丁台账、状态迁移/回滚、裁剪防回流和资格清单齐全 |

阶段可以交错，但保护条件不能跳过；现存缺陷优先补最小回归。完整任务编号和关闭标准只在 [TODO](TODO.md) 维护。

V8 进程内保护不等于 OS sandbox。控制口的发现/升级/命令均须鉴权，明确 Host/Origin、owner、背压、断连与清理。Linux/macOS 隔离措施分别验证。发布资格应包括 WPT 子测试分母、协议和官方客户端、网络/隐私、多上下文、故障及至少 24 小时合成长稳；未测组合不能标为支持。

原稿中的 issue/PR、移动 main 链接和现场数值只提供调查线索。当前事实必须从本仓库固定提交和可复现实验获得；不再维护另一份与 SUMMARY/TODO 竞争的“最新状态”。

## Southwest 业务验收

按 [Southwest 验收说明](Southwest-handoff.md) 与 OB-046 执行：同等查询/网络条件下参考 Chrome 成功，Obscura 独立建立自己的会话，shopping 不再 403 且返回正确可用的航班/报价结果。覆盖新会话、会话复用和预先约定的多轮观察，不借用 Chrome Cookie/token，不用缓存/模拟响应替代站点结果。此项是体现行为一致且未被该流程拒绝的重要证据；仍须同时通过通用差分、隐私与性能验收。

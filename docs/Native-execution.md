# 共享执行层归属与迁移清单

本清单对应 OB-021。历史路径以 `be486cc^`（删除私有 runtime 前）为准，当前定位以 `49c6ecd` 为基线。私有文件已删除不等于其所有产品行为已经迁移；每项分别记录共享机制、适配职责和未完成验收。官方 Playwright 负责 Locator 产品，不再建设自有 Locator 或私有 RPC。

| 行为 | 历史路径 | 当前归属 | 状态与剩余工作 |
| --- | --- | --- | --- |
| 坐标鼠标输入 | `runtime/src/manual.rs`、`runtime/src/automation.rs`，CDP 自有页面脚本 | `obscura-browser::Page::dispatch_mouse_input` → JS runtime 私有事件桥与 renderer hit-test | move/press/release 已迁移并有 Chrome 对照；完整 pointer capture、hover、多按钮与 user activation 未资格化 |
| Wheel | `crates/obscura-cdp/src/domains/input.rs` 页面脚本 | `Page::dispatch_wheel_input` → 私有事件桥与 renderer 滚动状态 | 已接入；九组 Chrome 对照通过，独立覆盖 metadata、嵌套边界、取消、页面覆盖、零增量和缺少 delta。完整手势锁定/惯性、scroll snap 和缩放未资格化 |
| 键盘与文本 | 历史 manual/automation 输入；CDP `dispatchKeyEvent`、`insertText` 脚本 | runtime 已有原生文本编辑和焦点机制 | CDP 仍需归口并覆盖键盘取消、选择区、组合输入边界 |
| 导航等待 | `runtime/src/browser.rs` 的 v1/v2 等待策略 | `obscura-browser/src/lifecycle.rs`、`Page::navigate_with_wait` | 底层已共享；记录旧 Load/DOMContentLoaded 差异由官方导航参数选择，不恢复版本专属等待器 |
| 元素与文本等待 | 历史 browser `wait` 和 automation locator 循环 | 官方 Locator；共享推进为 `Page::advance_automation` | CLI `wait_for_selector`、MCP `tool_wait_for`/`tool_wait_for_text` 仍有独立轮询。需要统一底层推进、deadline/取消与原生读取，保留入口返回格式 |
| 异步推进 | 历史 network tick、automation settle | Page `advance_automation`/`settle`/`settle_for_duration` 与 runtime event loop | CDP server、CLI、MCP 的调度入口不同。需要明确固定延迟、quiescence、load/network idle 的不同契约，不应合并为一个含混的“等待完成” |
| 资源事实与响应 body | `runtime/src/network.rs` 自有事件与 body 容器 | `obscura_net::observation::RequestTrace`、`obscura_net::response_body::ResponseBodyStore`；Page request/response callbacks、JS network events | 旧第二套 body 容器已删除。CDP event routing 与 MCP network projection 需验证不丢失、不重复、导航与多页面归属及消费/保留边界 |
| Body 保存限额 | 私有单 body 8 MiB、每页 32 MiB、256 条 | `obscura-net/src/response_body.rs` 的共享 `ResponseBodyLimits` | 由内存阈值/spool/总容量限制替代，不是等值搬迁。需补各入口容量失败契约，不恢复旧容器 |
| 执行预算与硬终止 | 私有协议 timeout、automation deadline/watchdog | Page 导航预算、runtime watchdog、CDP command watchdog、CLI process deadline | 多层 backstop 必须保留；统一预算来源、传播、取消与失败结果语义。已部分执行的官方 locator 动作可能因协议错误重试，不承诺 at-most-once |
| 传输与其他容量 | 私有 64 KiB 行、队列、4 页与 capture 计数上限 | CDP 连接/队列/write buffer 限制，MCP HTTP body/read timeout，截图维度限制 | framing/queue 留适配层；页面/body/计算预算归共享层。旧包装专属数量限制不自动施加到 CDP，逐项裁决并补验收 |
| 启动身份与网络 | 私有 persona 编译/激活、proxy 检查、origin allowlist | 共享 `activate_process_persona`、BrowserContext options、网络 SSRF gate | Persona/传输机制已共享；旧 origin allowlist 不等价于 SSRF gate，需单独记录是否保留及责任方 |
| 私有握手与运行模式 | workspace/hash/version 握手、RUNNING/PAUSED、takeover | 私有包装已删除 | 不恢复第二产品协议；构建/部署校验、调用方职责或有意删除的终态尚需逐条记录，不能称作等价迁移 |
| CDP 服务暴露 | 父进程拥有的 stdio 私有 RPC | `obscura-cdp/src/server.rs` bind/listen 与 CLI serve 参数 | 与 OB-034 协作，明确监听和访问边界；不以旧 stdio 隔离或 allowlist 握手冒充当前网络服务保护 |

后续交付顺序：wheel 有界资格完成后，优先处理键盘/文本归口；然后明确执行推进与等待契约、观察与限额资格、旧启动保护逐项终态。上述清单全部获得实现或明确终态及相应证据之前，OB-021 保持未关闭。

原始采集数据与工具日志完整保存，不脱敏、不删字段。产品响应 body 的容量失败与工具证据采集不是同一契约，不应通过静默删减原始证据来满足产品限额。

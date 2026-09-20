# 当前架构

核验基线及运行结果见 [SUMMARY](SUMMARY.md)。这是当前结构；目标迁移见 [New_ACH](New_ACH.md)。

| 模块 | 当前职责 |
| --- | --- |
| obscura-cli | fetch/serve/scrape/mcp；另有 obscura-worker 批量抓取进程 |
| obscura-cdp | WebSocket、连接调度、Target/session 和协议域处理 |
| obscura-browser | BrowserContext/Page、导航、frame、资源、输入与捕获 |
| obscura-js | deno_core/V8、bootstrap、Rust ops、frame/Worker |
| obscura-dom | DOM 树及防环约束 |
| obscura-net | primp 产品传输、兼容策略客户端、Cookie、URL/DNS 策略、tracker 规则 |
| obscura-render | CSS、布局、字体、CPU 绘制、几何/命中 |
| obscura-mcp | 当前 CLI 的非 optional 依赖，保留并复用共享能力 |
| obscura | 底层 Rust facade |
| runtime/ | 独立 Cargo workspace；直接嵌入引擎，默认 render 与强制 primp，NDJSON |
| bindings/python/ | 自有 Python 客户端，连接上述 runtime，不走 CDP |

## 执行与会话

CDP `server.rs::run_connection` 为连接创建 OS 线程、current-thread Tokio runtime 和 LocalSet。各 Page 的 JsRuntime 拥有自己的 V8 isolate；构造串行化和 isolate 进入约束仍须遵守。同一连接内同步 JS 可以阻塞调度，不等于多页面共用一个 isolate。

Worker 在 `obscura-js/src/worker.rs` 的独立线程/Tokio runtime/V8 isolate 执行。Worker 通过 detached primp 客户端获得独立连接池；配置、Cookie 和必要观察状态按现有策略共享。队列预算与 structured clone 有回归，不能据此声称完整 Worker 标准支持。

managed page session 通常为 `{targetId}-session`；显式 flattened attach 返回独立 session ID，客户端必须使用返回值。连接关闭会 abort 其 processor 并销毁 LocalSet/runtime 和页面，不能跨新连接恢复原 target，也不能让独立 viewer 观察另一连接的页面。

## 调用与渲染

CDP handler → Page → 网络/DOM/JS；JS 通过 ops 进入原生能力。网络事件、拦截和导航生命周期由多层共同处理，不能仅从 handler 名称断言协议语义完整。首批官方客户端方法已发布 observed profile 并移除相应整域占位成功；未列方法和未验证参数仍不具备资格，见 TODO OB-027。

render 消费共享 DOM/样式状态，以 Taffy 和原生浏览器布局逻辑、文本 shaping 和 CPU 绘制生成几何与图像。Page 负责资源和捕获，CDP 提供截图、screencast 与 raster PDF。图像输出存在不等于 Chromium 保真度认证。

## 状态与保护

BrowserContext 维护内存 localStorage，Page 维护相应 sessionStorage。当前 `storage_dir` 只加载/保存 Cookie，不能保证完整浏览器状态恢复。Cookie 磁盘格式为 version 1 envelope，内部记录保留 `host_only`；新程序兼容读取历史裸 `CookieInfo` 数组，但旧程序不保证可读新版 envelope。连接级 Cookie 状态通过 lossless snapshot/delta 合并。SameSite 完整请求上下文、分区以及 CDP/MCP 对外状态往返资格仍未完成，详见 [存储说明](Persist-cookies-and-storage.md)。

V8 watchdog、CLI 硬截止时间、panic=unwind、ops 防 panic、DOM 防环和 URL/DNS 校验均是要保留的保护，不是永不崩溃或 OS sandbox 保证。见 [SECURITY](../SECURITY.md)。

源码入口：[workspace](../Cargo.toml)、[CDP server](../crates/obscura-cdp/src/server.rs)、[JS runtime](../crates/obscura-js/src/runtime.rs)、[Worker](../crates/obscura-js/src/worker.rs)、[BrowserContext](../crates/obscura-browser/src/context.rs)。

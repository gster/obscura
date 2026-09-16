# Southwest 任务进度与后续 403 排查

更新日期：2026-09-16，正式脚本调度修复之后。本文是继续工作的入口；历史样本保留在各专题文档中，不用历史版本的结果替代当前状态。

## 当前结论

目标是由当前 Obscura runtime 原生完成 Southwest 查询，得到与 Chrome 一致的业务结果。**目标尚未完成：正式 runtime 直接 goto 后仍返回 `SEARCH_HTTP_403`。** 不应把 403 当作没有航班，也不能把脚本已执行、Cookie 数量相同或 HTTP 200 页面当作业务成功。

Chrome 必须通过 computer use 操作；不再使用 Playwright 驱动 Chrome。Obscura 通过 Python SDK 的 Browser/Page/Locator 和请求响应事件调用隔离 runtime，使用 Playwright 风格的接口，不额外绕到 CDP。两边均直接导航到构造的查询链接。

固定业务样本：LGA → LAS，2026-09-30，单程，1 成人；`nonstop_only` 是响应解析后的筛选条件。历史 Chrome 成功样本为 shopping 200、26 个行程、0 个直飞。继续测试时必须确认日期仍可查询；若日期失效，两边同时更换，重新建立对照。

当前安装的正式 runtime SHA-256：

`ac730c9b5efc905bfa02c0b5496400ff97c9e266ebf16849f018a6ccb7cead68`

对应源码含 render 与 stealth，正式传输为 primp。临时诊断版本没有替换最终安装版本。二进制哈希是本机已验证构建的身份，重新构建后应重新记录。

## 已完成的工作

| 项目 | 当前结果 | 证据边界 |
| --- | --- | --- |
| Python SDK 与隔离 runtime | 原生定位、输入、等待、请求响应观察及业务工作流接口已实现 | SDK 32/32，runtime 184/184；不等于 Southwest 成功 |
| Worker onmessage/EventTarget 与动态脚本回调 | 先前修复保留，并有回归 | 不包含真正的 Worker realm 隔离 |
| 动态经典脚本 CORS/凭据/destination | setAttribute 路径按 crossorigin 选择模式和凭据；destination 为 script；Origin 来源为文档而非 base | 后续仍发现 IDL 属性反射及同源 Origin 缺口 |
| parser 脚本下载屏障 | 去掉等待全部外部脚本完成的 collect 屏障 | 当前阻塞脚本就绪即可执行，下载仍最多 16 并发 |
| parser 等待时的 JS 调度 | 等待响应期间推进事件循环 | 动态子脚本、Promise、fetch 回调可以运行；保留 deadline/watchdog |
| async/defer 与模块相关回归 | 新增四个 HTTP 回归；保留模块预算及 import map 顺序 | 覆盖断连、404、DCL/load、截止期限，不声称完整浏览器规范已全部实现 |
| 官网执行顺序 | 四个保护子脚本在首次 shopping 前执行 | 单次诊断样本仍为 403，不能把执行数量当作服务器接受条件 |

正式调度版本的诊断时序：保护 bootstrap 在 1097 ms 开始，四个子脚本在 1232–1266 ms 执行，app.js 在 1546 ms 开始，shopping 在 1963 ms 发出，2796 ms 收到 403。时间相对 HTML 网络请求开始；这是执行顺序证据，不是性能基准。

正式无探针版本完整返回 `SEARCH_HTTP_403`。诊断版本在后续阶段出现 `BROWSER_EOF`，但首次 shopping 的 403 已独立记录。不能把诊断 EOF 当成正式版本的业务结果，也不能据此声称诊断全流程正常完成。

## 已排查但未找到充分原因的方向

- **IP**：用户未更换 IP 时手工 Chrome 能成功，因此不能只凭 Obscura 的 403 判断为 IP 问题。同一代理入口也不证明每次请求的上游出口完全相同。
- **Cookie 数量**：Chrome 长 AX 请求头曾被截断，最初“5 个”的统计已撤回。后续确认过 10 个。差异来自 HTML 的 Set-Cookie 和分析请求受 OriginGuard 阻止后的写入时机；限定放行实际观察到的分析域名后，首次名称集合相同仍为 403。没有移除正式 OriginGuard。
- **primp 与 wreq**：当前 JS/DOM 配合隔离 wreq 适配器同样 403；它不是最早历史成功二进制的复现。不能认定回退 wreq 就能恢复。
- **脚本时序**：两处真实调度缺陷已修复，官网子脚本执行提前，但仍 403。调度可能影响页面行为，尚无证据证明它是服务器拒绝的唯一原因。
- **请求正文**：配对 shopping 正文保持 355 字节，SHA-256 为 `119015f152d04726f2d8661fe54ade04fb6fbd89994de4fced2fddd969e7a395`。保护 bootstrap、四个子脚本及 app/vendor 的部分配对样本正文一致；`swa-common.js` 有不一致样本，不能声称所有执行输入完全相同。

## 后续排查顺序

每一步先做真实本地 HTTP/DOM 最小复现，用 computer use 获取 Chrome 对照，再修通用实现，补回归，最后单独复测官网。一次只改变一个方向；不修改网站脚本、不复制 Chrome Cookie/令牌、不添加主机名特判或 shopping 前固定等待。

### 1. 补齐脚本属性与请求构造

这是已定位、范围较小的下一批通用缺陷：

| 差异 | 代码入口 | 修复与验证重点 |
| --- | --- | --- |
| `script.crossOrigin = 'anonymous'` 仍发 no-cors | `crates/obscura-js/js/bootstrap.js` 的脚本接口属性、准备动态脚本逻辑 | 对比属性赋值与 setAttribute；覆盖缺省、空值、anonymous、use-credentials、无效值、null，验证真实模式和凭据 |
| 同源 cors 脚本缺少 Origin | `crates/obscura-js/src/ops.rs` 的 `fetch_origin_header`、`op_fetch_url`、`stealth_fetch_all` | 区分脚本 destination 与普通 fetch；普通同源 fetch 在 Chrome 下不带 Origin，不能统一补给所有 cors GET；覆盖两种传输及重定向 |
| 默认 Accept 缺失 | scripted request 构造、`crates/obscura-net/src/stealth_transport.rs` | 本地服务端已确认 primp 没有补 `*/*`；保持调用方显式 Accept，测试大小写、重定向及两种传输 |

每项修复后比较首次 shopping 前的真实请求和业务结果；若仍为 403，记录“兼容性已修复、拒绝原因未解决”。Priority 的当前观测包含 HTTP/1.1 回显和 HAR，不能直接等同于官网 HTTP/2 帧级结论。

### 2. 建立真正的 Worker 执行环境对照

入口为 `bootstrap.js` 的 `_makeWorkerScope` / `_autoRunWorker`。当前 Worker 通过页面 isolate 内的 Function/with/eval 模拟，共享页面 realm 的内建对象。本地 Blob Worker 已确认：

| 检查 | Chrome | 当前 Obscura |
| --- | --- | --- |
| `Function('return this')() === self` | true | false |
| `Function('return typeof document')()` | undefined | object |
| 能否读取页面写入 Object 的标记 | false | true |

先梳理 Worker 的全局对象、内建对象、消息克隆、异常、终止和事件循环边界，再设计独立 realm/isolate 方案。不能仅改几个返回值假装隔离。Chrome trace 已显示首次 shopping 前有 Worker 执行，但尚未证明官网读取了上表具体值；先确认运行路径，再决定改造范围。

### 3. 比较资源发现与导航等待

入口为 `crates/obscura-browser/src/page.rs` 的导航流程、stylesheet fetch 和 `prepare_screenshot_resources`。已有样本中 Obscura 在脚本前请求 13 个字体和 1 个 SVG，并进行默认约 1000 ms 的资源 warmup；Chrome 首次 shopping 前没有这些请求。

核对资源发现、CSS 阻塞、同步布局读取以及脚本任务的关系。warmup 原本用于避免同步字体/布局读取卡住 V8，不能直接删除或改成固定零等待。若改成并行或按需加载，必须覆盖渲染、字体晚到、生命周期和性能，不能只以请求减少作为改进证据。

### 4. 收紧输入一致性与传输观测

同批次记录 URL、重定向、响应正文哈希、状态、请求头、凭据策略、Cookie 来源和执行时点。先确认差异是否来自不同响应脚本或服务器 Set-Cookie，再归因于 runtime。只有应用层输入和通用浏览器行为收敛后，再比较 primp/wreq 的重定向、连接复用、HTTP/2 和 TLS 行为；保留正常证书校验与网络隔离。

历史 wreq 成功需要具体源码版本、构建特性、二进制哈希、代理及查询参数才能复现；缺少这些材料时只能列为历史线索。当前优先修已复现的浏览器实现差异。

诊断 EOF 应单独最小化：分批去除 Cookie、调用栈、console 等探针，记录子进程退出码、stderr 和最后协议消息。仅在确认无探针正式版本也存在同样问题时，才把它作为正式 runtime 缺陷处理。

## 验证状态与继续工作的入口

| 验证 | 最新结果 |
| --- | --- |
| 调度/模块/import map 聚焦 nextest | 18/18；其后新增 deadline 用例在全量通过 |
| 新增调度回归 | 4/4；两个握手用例已验证先失败后通过 |
| 独立 runtime release nextest | 184/184 |
| Python SDK | 32/32 |
| 根工作区 render release nextest | 1722/1727，另 4 项跳过 |
| 失败串行复核 | prefetch、awaited expression、MCP evaluate 通过；资源并发与 fixed-wait font 仍失败，与此前基线一致 |
| 指定 render CLI release build | 通过 |
| 障碍课程 | 32/33；`observer-intersection` 期望 io:50，实际为空 |
| Southwest 业务验收 | 未通过，SEARCH_HTTP_403 |

这是可继续开发的进度快照，不是全绿发布声明。此次文档整理复用上述刚完成的测试结果，没有因为提交操作重复进行全部外网站点采集。

源码与复现入口：

- 调度及四个 HTTP 回归：`crates/obscura-browser/src/page.rs`，搜索 `parser_script_executes_before_later_response_finishes`、`parser_wait_drives_dynamic_script_and_fetch_continuations`、`parser_async_defer_and_failed_fetch_preserve_lifecycle_order`、`parser_deadline_preserves_completed_scripts_and_bounds_pending_downloads`。
- 动态脚本双 origin SDK 回归：`bindings/python/tests/test_script_fetch.py`。
- 业务脚本位于伴随项目 `moneymachine/autopilot/workflows_southwest/airline_tasks/southwest_search.py`，不属于本仓库提交范围。
- 本机证据目录为 `/tmp/obscura-scheduler-formal`，含 manifest、正式版本结果、时序及测试日志；早先 HAR/trace 位于 `/tmp/southwest-pre-shopping-20260916`。临时目录可能被清理，不能作为跨机器必需依赖。原始 Cookie、头和 HAR 不提交。
- Chrome AX 长文本会被截断；新建隐身窗口也不一定等于独立全新 Cookie jar。继续采集时应明确这些边界。

常用复验命令（在仓库根执行，runtime 命令在 runtime 子目录执行）：

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --release --features render -p obscura-browser -E 'test(parser_) | test(module) | test(import_map) | test(load_delaying)'
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --release --features render --no-fail-fast
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render

# runtime 子目录
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --locked --release --no-fail-fast
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release

# 根目录；先设置 OBSCURA_RUNTIME_BIN 为本轮正式二进制路径
OBSCURA_ALLOW_PRIVATE_NETWORK=1 PYTHONPATH=bindings/python/src python3 -m unittest discover -s bindings/python/tests -v
```

最终验收必须由正式无探针 runtime 在多个独立上下文直接 goto，得到 shopping 成功响应并正确解析业务行程，再与同期 Chrome 对照。保留无直飞、无行程、请求失败的语义区别；没有任何一个待办可以预先保证消除 403。

## 详细记录

- [首次 shopping 前调度审计](Southwest-pre-shopping-scheduler-audit.md)
- [Southwest 对比与 Cookie 来源记录](Southwest-search-comparison.md)
- [primp 与 wreq 对比](Primp-and-wreq-comparison.md)
- [保护脚本通用回归](Protection-script-regression-case.md)
- [项目修复日志](Obscura-fix-changelog.md)

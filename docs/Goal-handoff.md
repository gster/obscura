# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`30f19d394be554934d35cddcc4685eaa46028aa1`，`Cancel V8 work on CDP disconnect`。
- 写入本交接前，该提交已在 `origin/main` 和本地主仓库 `main` 上对齐，相关工作树保持干净。
- 继续使用现有工作树 `/Users/gster1981/work/obscura`，分支为 `codex/goal`。
- 相关实现入口：[`execution_cancellation.rs`](../crates/obscura-js/src/execution_cancellation.rs)、[`runtime.rs`](../crates/obscura-js/src/runtime.rs)、[`worker.rs`](../crates/obscura-js/src/worker.rs)、[`page.rs`](../crates/obscura-browser/src/page.rs)、[`server.rs`](../crates/obscura-cdp/src/server.rs) 和 [`disconnect_cancels_v8.rs`](../crates/obscura-cdp/tests/disconnect_cancels_v8.rs)。

## 最近完成的阶段

CDP WebSocket I/O 已与 connection-local processor/V8 分离：I/O 留在 server runtime，V8 继续固定在专用 OS thread 的 current-thread runtime/LocalSet。同步脚本占满 V8 线程时，socket reader 仍能独立观测 Close、EOF/FIN、RST、writer failure 与 server shutdown，并通过 thread-safe isolate handle 终止当前执行。

每个连接拥有 sticky `ExecutionCancellation`。Page 当前 runtime、导航后新 runtime、共享 parent runtime 的 iframe 和 Dedicated Worker runtime 都附着同一 cancellation source；runtime 每次进入或 poll V8 时登记 active slot，退出后解除，termination clear 后若连接已经关闭则立即重新终止。弱引用登记会回收已销毁 runtime，避免长连接反复导航导致注册表无界增长。watchdog 的 armed guard 在 drop 时移除自己的 generation，避免取消调用后留下延迟误杀。

断连后 processor 最多获得 1 秒清理 Fetch pause/page，随后 abort 异步 navigation/network wait；I/O handler 自身被取消时也会中止 processor 和 detached writer。断连不是事务回滚：终止前已经发生的 DOM、Cookie 或网络副作用可以保留；关闭连接的 queued/deferred 命令不重放。客户端仅取消本地 asyncio wait 而不关闭 wire 时不是服务端断连；FIN/RST 尚未到达 reader 前也不宣称已经终止。直接 iframe/Worker wire-level 组合尚未分别取得端到端资格。

## 验证结果

- CDP focused release/render nextest：9/9；覆盖 infinite evaluate、inline infinite navigation、stalled transport navigation、slot recovery、writer cancellation 和既有 admission/并发回归。
- obscura-js focused release/render nextest：3/3；覆盖 sticky termination、dead runtime slot 回收和 watchdog guard drop。
- full release/render nextest 首轮为 2220/2221，唯一失败 `obscura-cli::mcp_client test_wait_for_selector`；未改源码单项复跑 1/1，通过后完整复跑为 2221/2221，4 skipped。现有证据不足以把首轮失败归因于本次改动，不宣称已修复其根因。
- render 和 no-default-features 两种 exact CLI release build 均成功；最终恢复 exact render build，SHA-256 `7a788c29747423e9c67a58db25abd8d9bb0b9714cedb059ed01afad114c4fb9c`，119546448 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`：在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33，包括 `observer-intersection`。
- `cargo check -p obscura-cdp --features render` 和 `git diff --check` 通过。
- Astra light 首审指出 processor teardown、connection handler 取消时 writer 泄漏、runtime registry churn 三项 major；修复后复审确认全部关闭，无新增 blocker、major 或 minor finding。

## 后续执行顺序

OB-021 和 OB-034 仍然开放。相邻且尚未完成的边界按以下顺序推进：

1. 为 iframe 与 Dedicated Worker 的同步 V8 断连补直接 wire-level 回归，并根据实际客户端行为补原始 FIN/RST 资格；不要从共享实现静态外推完整矩阵。
2. 继续处理 [TODO](TODO.md) 中 OB-021 剩余的 input qualification 和 observation ownership 项目。
3. 继续区分 admission、已授权命令 ownership 和 TCP/kernel/container 总资源边界，不把本切片扩大成完整授权、事务回滚或 RSS 证明。

开始下一段实现前先 fetch `origin/main`，并通过 fast-forward 或合并吸收远端更新，避免重复实现。一次只运行一个 Cargo 进程。代码稳定后合并验证，验证通过后及时提交并推送到 `origin/main`，然后在本地主仓库干净且可安全快进时同步其 `main`。

## 持续有效的决定

- Persona 在 browser context 初始化时确定，并保持不变直到 context 关闭；primp 必须使用同一个 persona。
- CLI 和 serve 不保留 stealth 开关；stealth 归口 persona。Puppeteer 兼容已经废弃，不作为产品或回归目标。
- 已初始化 context 中的 `Network.setUserAgentOverride` 保持 unsupported。CDP 只能在 persona/context 创建边界参与身份设置。
- 工具和日志采集优先保证数据完整，不添加脱敏或字段丢弃。
- 独立且边界清楚的工作继续交给 subagent；简单任务可用 Luna，复杂任务使用 Astra 或 Sol，最终审核可用 Astra light。
- 尽量减少 Rust 重复编译，在代码稳定后合并执行验证。

## 原始证据位置

生成本交接的主机上曾保留以下完整临时证据：

- `/tmp/ob034-access-full-nextest-review-final.log`
- `/tmp/ob034-access-render-build-release-candidate.log`
- `/tmp/ob034-access-no-render-build-review-final.log`
- `/tmp/ob034-access-no-render-focused-review-final.log`
- `/tmp/ob034-access-obstacle-release-candidate.log`
- `/tmp/ob034-access-tools-unittest-review-final.log`
- `/tmp/ob034-access-smoke-review-final/`
- `/tmp/ob034-access-automation-smoke-review-final.json`
- `/tmp/ob034-inbound-benchmark.cEiFsw`

这些路径属于原始运行主机的临时文件，不是仓库内的持久接口。可持续引用的结论和能力边界以本页、[SUMMARY](SUMMARY.md) 及对应提交中的测试为准。处理这些日志时保留原始字段和完整内容。

## 建议使用的 skills

- `implement`：执行下一段边界明确的 TODO。
- `diagnosing-bugs`：排查无法稳定复现或对时序敏感的 capacity/connection 问题。
- `code-review`：在发布下一段改动前执行 Standards 和 Spec 双轴审核。
- `handoff`：下一次验证并提交后需要切换会话时更新本页和临时交接。

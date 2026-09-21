# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`76b4591f8f59146f290c7ebf2bf90ae583dcb6c8`，`Qualify iframe and Worker disconnects`；其前置实现为 `30f19d394be554934d35cddcc4685eaa46028aa1`，`Cancel V8 work on CDP disconnect`。
- 写入本交接前，最近实现提交已推送到 `origin/main`；本交接提交完成后须再次核对 `HEAD`、`main`、`origin/main` 与远端 ref 对齐。
- 继续使用现有工作树 `/Users/gster1981/work/obscura`，分支为 `codex/goal`。
- 相关实现入口：[`execution_cancellation.rs`](../crates/obscura-js/src/execution_cancellation.rs)、[`runtime.rs`](../crates/obscura-js/src/runtime.rs)、[`worker.rs`](../crates/obscura-js/src/worker.rs)、[`page.rs`](../crates/obscura-browser/src/page.rs)、[`server.rs`](../crates/obscura-cdp/src/server.rs) 和 [`disconnect_cancels_v8.rs`](../crates/obscura-cdp/tests/disconnect_cancels_v8.rs)。

## 最近完成的阶段

CDP WebSocket I/O 已与 connection-local processor/V8 分离：I/O 留在 server runtime，V8 继续固定在专用 OS thread 的 current-thread runtime/LocalSet。同步脚本占满 V8 线程时，socket reader 仍能独立观测 Close、EOF/FIN、RST、writer failure 与 server shutdown，并通过 thread-safe isolate handle 终止当前执行。

每个连接拥有 sticky `ExecutionCancellation`。Page 当前 runtime、导航后新 runtime、共享 parent runtime 的 iframe 和 Dedicated Worker runtime 都附着同一 cancellation source；runtime 每次进入或 poll V8 时登记 active slot，退出后解除，termination clear 后若连接已经关闭则立即重新终止。弱引用登记会回收已销毁 runtime，避免长连接反复导航导致注册表无界增长。watchdog 的 armed guard 在 drop 时移除自己的 generation，避免取消调用后留下延迟误杀。

断连后 processor 最多获得 1 秒清理 Fetch pause/page，随后 abort 异步 navigation/network wait；I/O handler 自身被取消时也会中止 processor 和 detached writer。断连不是事务回滚：终止前已经发生的 DOM、Cookie 或网络副作用可以保留；关闭连接的 queued/deferred 命令不重放。客户端仅取消本地 asyncio wait 而不关闭 wire 时不是服务端断连；FIN/RST 尚未到达 reader 前也不宣称已经终止。

直接断连资格已增加两个选择性组合。iframe + 原始 FIN 用例从真实子 `FrameRealm` 第一轮无限循环内写入唯一 console marker；客户端只关闭 TCP 写半边，并持续保留、排空读半边直至服务端关闭，避免 drop 把 FIN 混成 RST。FIN 前排队的第二条命令含同步 console marker 和独立 fetch fixture，二者均未出现，随后新连接可执行 `42`，因此没有把未执行输入在 teardown 或重连后重放。Dedicated Worker + 原始 RST 用例用 linger-zero abortive close，并等待 worker 第一轮循环内的 `started` message 后才断开。配对的 owner-alive 单测在 parent runtime 与 Worker registry 保持存活时触发共享 cancellation，观察 active Worker lease 从 1 降到 0，排除了仅靠 Page/owner drop 结束 worker 的假阳性。Worker event loop 在 sticky cancellation 后退出，不再把 V8 termination 当普通错误循环重试。

本阶段没有宣称完整 iframe/Worker x Close/FIN/RST 矩阵。idle Worker 若只阻塞在 `commands.recv()`，connection cancellation 本身不会主动唤醒它；当前连接 owner teardown 会发送 stop，但这一组合尚未单独取得直接资格。

## 验证结果

- 最终 CDP disconnect release nextest：render 5/5（run `34296d83-29ab-4eec-b428-96b23baad405`），no-render 5/5（run `100d853c-764b-4d5d-89b9-b51163ea7d99`）。
- owner-alive Worker cancellation：render 1/1（run `8a9dac93-6add-4571-9a7e-734e308cc090`），no-render 1/1（run `31385248-048e-411f-b6da-6a6f493eafa1`）；render Worker 广义定向回归 38/38（run `d232c2e2-d7d9-4d52-9f5b-a2f83d75c094`）。
- full release/render nextest：2224/2224，4 skipped（run `04d3e810-3d1a-41d5-8b8d-9b502d72e2bb`）。唯一 `LEAK` 为既有且未改的 `obscura-cdp::child_frame_tree isolated_worlds_are_owned_by_their_exact_frame`；本阶段不宣称已修复其根因。
- render 和 no-default-features 两种 exact CLI release build 均成功；最终恢复 exact render build，SHA-256 `2f7f6db8734515d9df5f225e987f17695baf454cbee7e7e1c22d4e7563d44e95`，119548032 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`：在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33，包括 `observer-intersection`。
- `git diff --check` 通过。
- Astra light 本阶段首审指出 FIN helper 可能因未读 peer data 在 drop 时变成 RST，以及单凭无 fetch accept 不能证明 queued JavaScript 未执行这两项 major；保留并排空 FIN 读半边、增加同步 queued marker 后，复审确认两项均关闭，无新增 blocker、major 或 minor finding。复审为静态审核，执行结果以上述 nextest 和 obstacle 证据为准。

## 后续执行顺序

OB-021 和 OB-034 仍然开放。相邻且尚未完成的边界按以下顺序推进：

1. 继续处理 [TODO](TODO.md) 中 OB-021 剩余的 input qualification 和 observation ownership 项目。
2. 按实际风险决定是否补齐其余 iframe/Worker x Close/FIN/RST 组合与 idle Worker cancellation wake；不要从本阶段两个选择性组合静态外推完整矩阵。
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
- `/tmp/ob034-disconnect-qualification-full-nextest.log`
- `/tmp/ob034-disconnect-qualification-no-render-focused.log`
- `/tmp/ob034-disconnect-qualification-no-render-build.log`
- `/tmp/ob034-disconnect-qualification-render-build.log`
- `/tmp/ob034-disconnect-qualification-obstacle.log`

这些路径属于原始运行主机的临时文件，不是仓库内的持久接口。可持续引用的结论和能力边界以本页、[SUMMARY](SUMMARY.md) 及对应提交中的测试为准。处理这些日志时保留原始字段和完整内容。

## 建议使用的 skills

- `implement`：执行下一段边界明确的 TODO。
- `diagnosing-bugs`：排查无法稳定复现或对时序敏感的 capacity/connection 问题。
- `code-review`：在发布下一段改动前执行 Standards 和 Spec 双轴审核。
- `handoff`：下一次验证并提交后需要切换会话时更新本页和临时交接。

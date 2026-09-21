# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`d036de2e458eb7117a9bfc0139e734c2bb37a63e`，`Make passive callbacks reentrant`；其前置实现为 `76b4591f8f59146f290c7ebf2bf90ae583dcb6c8`，`Qualify iframe and Worker disconnects`。
- 写入本交接前，最近实现提交已推送到 `origin/main`；本交接提交完成后须再次核对 `HEAD`、`main`、`origin/main` 与远端 ref 对齐。
- 继续使用现有工作树 `/Users/gster1981/work/obscura`，分支为 `codex/goal`。
- 相关实现入口：[`client.rs`](../crates/obscura-net/src/client.rs) 的 `CallbackRegistry`、[`network_tests.rs`](../crates/obscura-net/src/network_tests.rs)，以及后续 CDP observation ownership 的 [`server.rs`](../crates/obscura-cdp/src/server.rs) 与 Network domain。

## 最近完成的阶段

Page、iframe 与 Dedicated Worker 共用的 passive `CallbackRegistry` 已从 `tokio::sync::RwLock<Vec<_>>` 的 `try_write` 改为短时 `std::sync::Mutex` 保护的 copy-on-write immutable `Arc<Vec<_>>` snapshot。旧实现可能返回一个未实际安装的 callback ID，或在 dispatch 持有 read guard 时把真实 remove 报成 false；这两类静默 observation loss 已消除。

dispatch 在 mutex 内只复制 snapshot，随后释放锁并按登记顺序调用 callback。callback 中的 add/remove 因而可重入且不死锁；mutation 从下一条 observation 生效，当前 snapshot 完整送达。公开 callback 签名、ID 和顺序未改，`RequestInfo`、`Response`、binary body、兼容 header map 与 raw header bytes 原样交给观察者，不脱敏、不删字段。

Astra light 首审发现 remove 用 `retain` 时会在 mutex 内 drop 最后一个 callback Arc；若 capture 的析构函数重入 registry，会自死锁。最终实现把命中的 tuple 移出 guard 作用域后再析构，并增加 request/response 两条一秒有界的 mutation-sensitive 析构重入回归。复审没有 blocker、major 或 minor finding。

该阶段只修复 native passive callback ownership。它没有隔离 callback panic，也没有修复 CDP 多 session `Network.enable` subscription/fanout、上游 JS/Worker 4096 oldest-drop、持久 observation history 或请求正文保留。尤其不能从 callback 完整送达外推为 CDP 各 session 都收到了同一 Network 事件。

## 验证结果

- 最终 callback 专项 3/3（run `8c298874-08b9-49fd-949b-01cc690a4eec`），`obscura-net` 158/158（run `81bfdbd9-7e04-4e0f-891e-a8e272c03ccd`）。
- 相关 Page/Worker render 5/5（run `71be068b-9394-4081-bc69-77af51b7a3c2`），no-render Worker 1/1（run `d37e96c8-d2a4-433a-b344-d3da2331247b`）。
- full release/render nextest：2227/2227，4 skipped（run `b1def084-52e0-4a7e-a633-e00c26ee2427`），没有 failure 或 leaky test。
- render 和 no-default-features 两种 exact CLI release build 均成功；最终 exact render SHA-256 `6b5b0eb6553479ede399ea56a66205b862d3555cc7f49b6cb72bc689b77b3d94`，119546272 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`：在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33，包括 `observer-intersection`。
- `git diff --check` 通过。
- Astra light 复审为静态审核，执行结果以上述 nextest 和 obstacle 证据为准；残余风险是 callback panic 仍会中止该次后续 callback delivery，以及尚未直接覆盖一个 callback 移除另一个 callback 的并发场景。

## 后续执行顺序

OB-021 和 OB-034 仍然开放。相邻且尚未完成的边界按以下顺序推进：

1. 收敛 CDP 多 session Network observation ownership：为 `Network.enable` 建立 per-session subscription/fanout，和 Fetch pause ownership 分开；用真实 Chrome 对照导航与导航后 fetch，验证同一完整 raw event 如何送达各已订阅 session。
2. 把 JS/Worker 当前 4096 条 oldest-drop observation queue 改成有界 spill/append-only durable evidence 或显式 terminal failure；不能只添加一个 overflow marker 后继续丢已接纳事实。
3. 继续请求正文保留、导航/多页面 history 与其余 input qualification；按实际风险再决定是否补齐 disconnect 矩阵和 idle Worker cancellation wake。

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

- `/tmp/ob021-callback-registry-focused-review-fix.log`
- `/tmp/ob021-callback-registry-obscura-net-review-fix.log`
- `/tmp/ob021-callback-registry-dependent-focused.log`
- `/tmp/ob021-callback-registry-worker-no-render.log`
- `/tmp/ob021-callback-registry-full-nextest.log`
- `/tmp/ob021-callback-registry-no-render-build.log`
- `/tmp/ob021-callback-registry-render-build.log`
- `/tmp/ob021-callback-registry-obstacle.log`
- `/tmp/ob034-inbound-benchmark.cEiFsw`

这些路径属于原始运行主机的临时文件，不是仓库内的持久接口。可持续引用的结论和能力边界以本页、[SUMMARY](SUMMARY.md) 及对应提交中的测试为准。处理这些日志时保留原始字段和完整内容。

## 建议使用的 skills

- `implement`：执行下一段边界明确的 TODO。
- `diagnosing-bugs`：排查无法稳定复现或对时序敏感的 capacity/connection 问题。
- `code-review`：在发布下一段改动前执行 Standards 和 Spec 双轴审核。
- `handoff`：下一次验证并提交后需要切换会话时更新本页和临时交接。

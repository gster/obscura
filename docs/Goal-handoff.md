# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`00487e846ee0296765b09bc9a93583d7cc9c5a49`，`Bound CDP outbound queues`。
- 写入本交接前，该提交已在 `origin/main` 和本地主仓库 `main` 上对齐，相关工作树保持干净。
- 继续使用现有工作树 `/Users/zg/.codex/worktrees/ob-027-cdp-profile/obscura`，分支为 `ob-027-cdp-profile/obscura`。
- 相关实现入口：[`outbound.rs`](../crates/obscura-cdp/src/outbound.rs)、[`server.rs`](../crates/obscura-cdp/src/server.rs) 和 [Native execution](Native-execution.md)。

## 最近完成的阶段

每个 CDP 连接现在最多接受 1024 条出站消息、128 MiB 已序列化 UTF-8 payload 总量，以及 80 MiB 单条消息。资源预留持续到 WebSocket 发送完成。每次写入有 10 秒期限。消息数量、总字节数、单条大小溢出，以及 writer I/O 或超时失败，都会进入粘性的连接关闭状态。

processor 在执行排队命令前重新检查关闭状态。已经关闭的 outbound 不会再应用排队的 Fetch resolution。有效的 `Browser.close` 会先停止接收新消息，并在传输关闭前尝试一次有界 flush；这不代表存在额外的消息送达确认。

该内存上限只覆盖已接受且已序列化的 payload 字节，不覆盖 admission 前的 `String` 构造、`String` capacity、channel/Tungstenite/TCP 开销、入站 `ServerMessage`、`CdpContext.pending_events`、响应正文存储或整个进程的 RSS。

## 验证结果

- focused release nextest：11/11，run `69a117c6-c568-4282-9682-4c0f0609f996`。
- full release/render nextest：2192/2192，4 skipped，run `bfefdcae-e4e1-49b7-aacb-481b47414086`。
- no-render `obscura-cdp`：251/257；6 个既有 Input 测试按预期返回 `INPUT_UNSUPPORTED_WITHOUT_RENDER`，相关的 11 个 outbound/close 测试全部通过。
- render build：SHA-256 `e7c04fc2cad7af6c060c07ca5dfb4713c49f6fca65bec7c745f8f14403f989b9`，119605504 bytes。
- no-render build：SHA-256 `9d8211c6e92c81bb071c4fe48c38afa206b86189ad3e06299e087fe78b1f112d`，77654208 bytes；随后恢复 render build，并核对到相同的冻结 hash。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`：在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33，包括 `observer-intersection`。缺少 persona 的首次运行以启动配置错误结束，0/33 原始结果仍保留在证据中。
- 官方 Playwright Python 1.60.0 smoke 通过，raw protocol 与 37-method profile 一致。
- manifests 有效，Python 工具测试 51/51。
- 两次最终独立审核均未发现实现阻塞项。

## 后续执行顺序

OB-021 和 OB-034 仍然开放。相邻且尚未完成的边界按以下顺序推进：

1. 限制或明确定义入站 `ServerMessage` queue 和 `CdpContext.pending_events`，保留显式失败语义和完整观测数据。
2. 单独定义 CDP Host、Origin 和鉴权访问策略，不要把它们与 SSRF 或默认 loopback binding 混为一体。
3. 继续补齐任意同步 V8 执行的 disconnect/cancellation matrix；现有 V8 watchdog 仍是最终保护。
4. 继续处理 [TODO](TODO.md) 中 OB-021 剩余的 input qualification 和 observation ownership 项目。

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

- `/tmp/ob034-outbound-evidence.json`
- `/tmp/ob034-outbound-playwright.p7oN4m`
- `/tmp/ob034-outbound-python.log`

这些路径属于原始运行主机的临时文件，不是仓库内的持久接口。可持续引用的结论和能力边界以本页、[SUMMARY](SUMMARY.md) 及对应提交中的测试为准。处理这些日志时保留原始字段和完整内容。

## 建议使用的 skills

- `implement`：执行下一段边界明确的 TODO。
- `diagnosing-bugs`：排查无法稳定复现或对时序敏感的 capacity/connection 问题。
- `code-review`：在发布下一段改动前执行 Standards 和 Spec 双轴审核。
- `handoff`：下一次验证并提交后需要切换会话时更新本页和临时交接。

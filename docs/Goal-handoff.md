# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`09e577adb218b459a588880ac2a78f4b506c6cbc`，`Bound CDP inbound and pending events`。
- 写入本交接前，该提交已在 `origin/main` 和本地主仓库 `main` 上对齐，相关工作树保持干净。
- 继续使用现有工作树 `/Users/gster1981/work/obscura`，分支为 `codex/goal`。
- 相关实现入口：[`inbound.rs`](../crates/obscura-cdp/src/inbound.rs)、[`pending_events.rs`](../crates/obscura-cdp/src/pending_events.rs)、[`outbound.rs`](../crates/obscura-cdp/src/outbound.rs)、[`server.rs`](../crates/obscura-cdp/src/server.rs) 和 [Native execution](Native-execution.md)。

## 最近完成的阶段

每个 CDP 连接的 inbound `ServerMessage` 现在最多接受 1024 条、128 MiB text 总量和 64 MiB 单消息，WebSocket 单 frame 为 16 MiB。reservation 持续穿过 channel、命令执行和 navigation deferred queue，不在 dequeue 时提前释放。

`CdpContext.pending_events` 现在最多保留 1024 条、128 MiB 完整 event envelope 序列化 UTF-8 bytes 和 80 MiB 单事件。counting writer 精确计数，批量 admission 全有或全无。inbound 或 pending 的首个 count/bytes/single-message/serialization 失败会 sticky close 同一连接，既有 accepted events 不会部分发送，后续命令也不再执行。malformed CDP 日志保留完整原始 text，pending event 转发与直接 serde JSON 逐字节一致。

inbound、pending events 和已有 outbound 是三个独立逻辑 payload 预算，不是 128 MiB 总连接内存或 RSS 上限。它们不覆盖 admission 前构造、容器 capacity、Tungstenite/TCP、上游 observation queue、响应正文、Host/Origin/auth 或同步 V8 中立即断连。公开 `CdpContext.pending_events` 从 `Vec<CdpEvent>` 改为只读 Vec-like `PendingEvents`，依赖具体 Vec 类型或可变迭代的低层调用方需要适配。

## 验证结果

- focused release/render nextest：12/12，run `948fdc7d-c690-47ac-bbf9-44437259bbd1`。
- full release/render nextest：2203/2203，4 skipped，run `83dd6c65-38e4-4369-ad88-a25481d5566b`。
- no-render transport 定向：24/24，run `22b925cc-8da5-4731-baa4-5505abf3f5e8`。
- render build：SHA-256 `8a536e24acba04d23f1d61f7667b63bfa76cd6a2195e6eb20e88d7fe590b762b`，119396128 bytes。
- no-render build：SHA-256 `838b7a33bda0c9e26216b5c09c02651c13702d7e28a242abb208b92a89d9ab9d`，77430944 bytes；随后恢复 render build，并核对到相同的冻结 hash。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`：在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33，包括 `observer-intersection`。
- 官方 Playwright Python 1.60.0 smoke 和三项 migration gate 通过，raw protocol 分别与 37-method profile 及合并 40-method profile 一致。
- manifests 有效，Python 工具测试 51/51。
- Standards、Spec 和 Astra light 最终独立审核均未发现阻塞项。

## 后续执行顺序

OB-021 和 OB-034 仍然开放。相邻且尚未完成的边界按以下顺序推进：

1. 单独定义 CDP Host、Origin 和鉴权访问策略，不要把它们与 SSRF 或默认 loopback binding 混为一体。
2. 继续补齐任意同步 V8 执行的 disconnect/cancellation matrix；现有 V8 watchdog 仍是最终保护。
3. 继续处理 [TODO](TODO.md) 中 OB-021 剩余的 input qualification 和 observation ownership 项目。

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

- `/tmp/ob034-inbound-full-nextest.log`
- `/tmp/ob034-inbound-render-build.log`
- `/tmp/ob034-inbound-no-render-build.log`
- `/tmp/ob034-inbound-no-render-focused.log`
- `/tmp/ob034-inbound-obstacle.log`
- `/tmp/ob034-inbound-python.log`
- `/tmp/ob034-inbound-playwright.eGHx5F`
- `/tmp/ob034-inbound-benchmark.cEiFsw`

这些路径属于原始运行主机的临时文件，不是仓库内的持久接口。可持续引用的结论和能力边界以本页、[SUMMARY](SUMMARY.md) 及对应提交中的测试为准。处理这些日志时保留原始字段和完整内容。

## 建议使用的 skills

- `implement`：执行下一段边界明确的 TODO。
- `diagnosing-bugs`：排查无法稳定复现或对时序敏感的 capacity/connection 问题。
- `code-review`：在发布下一段改动前执行 Standards 和 Spec 双轴审核。
- `handoff`：下一次验证并提交后需要切换会话时更新本页和临时交接。

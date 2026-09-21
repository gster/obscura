# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`b2dc8b7a1d400b0093f806b3be201ac99e460665`，`Secure CDP control-plane admission`。
- 写入本交接前，该提交已在 `origin/main` 和本地主仓库 `main` 上对齐，相关工作树保持干净。
- 继续使用现有工作树 `/Users/gster1981/work/obscura`，分支为 `codex/goal`。
- 相关实现入口：[`access.rs`](../crates/obscura-cdp/src/access.rs)、[`server.rs`](../crates/obscura-cdp/src/server.rs)、[`main.rs`](../crates/obscura-cli/src/main.rs)、[`cdp_access_smoke.py`](../tools/unblocked/cdp_access_smoke.py) 和 [Native execution](Native-execution.md)。

## 最近完成的阶段

CDP discovery 与 WebSocket upgrade 现在在 handoff、live connection slot 和 V8 前经过同一套 byte-level admission。HTTP 路由、Host authority、可选 Origin 与 Bearer 都按精确值处理；query 和 substring lookalike 不再落入其他处理路径，显式无效端口不会退化成无端口 Host。官方 Playwright 1.60 所需的 `/json/version/` 作为单独精确兼容路由保留。

loopback 默认只允许实际监听端口的本机 authority，并兼容无 Origin、无 token 的 native client。配置 `OBSCURA_CDP_TOKEN` 或 `--auth-token-file` 后，默认无子命令、单 worker、多 worker balancer 与 direct worker 都要求同一 Bearer。non-loopback 默认还要求显式 Host allowlist；只有 `--allow-unauthenticated-remote` 才把鉴权责任明确交给外层边界。discovery 对外 ws/wss URL 与请求 Host 校验分开配置。

访问策略只控制 CDP 入口，不改写或脱敏授权连接中的 page/CDP 原始数据，也不参与出站 SSRF。采集 smoke 为每次 readiness 尝试保留独立 request、已收到的 response bytes 和完整错误；异常或重试不覆盖前一份证据。inbound、pending events 与 outbound 的既有三个逻辑 payload 预算继续有效，但它们和本次 admission 都不是总连接内存或 RSS 上限；同步 V8 中立即断连等边界仍未完成。

## 验证结果

- focused release/render nextest：15/15，run `0888f887-84be-458f-91d9-3104047534d1`。
- full release/render nextest：2214/2214，4 skipped，run `ed34dab5-c3a5-42a8-b53e-07706d8cc777`。
- no-render access 定向：15/15，run `f399723d-7903-4037-9471-078edd93e070`。
- render build：SHA-256 `6872b9d8e2e550ae03805695fdfa8df11814f1dc6eee470821a6aceb57d73afc`，119418656 bytes。
- no-render build：SHA-256 `11aad4b682be83d7a68fef1d50aceac5bb94c263fbdbe92107e270fa4aeff457`，77529584 bytes；随后恢复 exact render build，并核对到相同的冻结 hash。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`：在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33，包括 `observer-intersection`。
- 官方 Playwright Python 1.60.0 的默认入口、鉴权单 worker、多 worker/direct worker smoke，以及默认 loopback automation smoke 全部通过。
- manifests 有效，Python 工具测试 52/52。
- Standards 与 Spec 复核无遗留 finding；Astra light 首审的三项 blocker 全部修复，最终复审为 0 blocker。

## 后续执行顺序

OB-021 和 OB-034 仍然开放。相邻且尚未完成的边界按以下顺序推进：

1. 继续补齐任意同步 V8 执行的 disconnect/cancellation matrix；现有 V8 watchdog 仍是最终保护。
2. 继续处理 [TODO](TODO.md) 中 OB-021 剩余的 input qualification 和 observation ownership 项目。
3. 继续区分 admission、已授权命令 ownership 和 TCP/kernel/container 总资源边界，不把本切片扩大成完整授权或 RSS 证明。

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

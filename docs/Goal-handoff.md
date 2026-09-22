# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`17e70390b076aeb30c36fb4f8df81e6a94984991`，`Qualify ignored CDP input events`。
- 发布核对要求：`HEAD`、本地 `main`、`origin/main` 与远端 `refs/heads/main` 必须对齐。
- 继续使用现有工作树 `/Users/gster1981/work/obscura`，分支为 `codex/goal`。
- 最新切片入口是 [`dispatch.rs`](../crates/obscura-cdp/src/dispatch.rs) 的 session contribution 与 dispatcher fast path、[`input.rs`](../crates/obscura-cdp/src/domains/input.rs) 的参数/抑制入口、[`target.rs`](../crates/obscura-cdp/src/domains/target.rs) 的 detach cleanup，以及 [`native_keyboard_smoke.py`](../tools/unblocked/native_keyboard_smoke.py) 的 Chrome/Obscura 成对资格工具。

## 最近完成的阶段

OB-021 `Input.setIgnoreInputEvents` 已从静默假成功改为 Chrome 152 实测语义：每个 attached session 持有 contribution，同一 target 取 OR；false 只清调用 session，导航保留，detach/target close 清理，新 attachment 默认 false，跨 target 独立。有效 ignore 抑制 mouse/wheel/key 且不进入 Page/V8、input navigation 或 command-driven screencast sampling，`Input.insertText` 仍执行。缺失/非法 boolean 为 `-32602`，无效 session 为 `-32000`。`dispatchTouchEvent`、完整 mouse parity 与 no-render 实际输入没有因此资格化。

最终 committed binary 来自 `17e70390b076aeb30c36fb4f8df81e6a94984991`，版本 `0.1.0-dev+17e7039`；render SHA-256 `c11a56645b610023a939e55d6cdc3b68d0d22c3953f7bfb3fdb79cc17be38778`，120747312 bytes；no-render SHA-256 `917461f738497888068a760a676faf422d5c70228dedaf6286809331bdd2837d`，78920272 bytes。九组 Chrome 152/Obscura paired qualification 全部通过，最终目录 `/private/tmp/ob021-ignore-committed-final.It7IaM/`，结果 JSON SHA-256 `6d6030955ebba17b43a40dd0cc7a2fef87cf486e92f76c2db564295eb3701b1e`，runner stderr 0 bytes。首次 full-snapshot paired 失败 `/private/tmp/ob021-ignore-paired-precommit.dmzWLQ/` 原样保留；它只暴露既有 click/`which`/coordinate caret 差异，最终 comparator 保留所有 raw 字段但不把这些独立 mouse parity 当作本 gate 已资格化。

聚焦 render **2/2**、no-render control-plane **1/1**，CDP release/render **393/393**、3 skipped；全工作区 release/render **2343/2343**、4 skipped；工具 unittest **138/138**。固定 benchmark `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` obstacle **33/33**。官方 Playwright Python 1.60.0 smoke、完整 **37-method** profile 与三项 migration gates 通过；目录 `/private/tmp/ob021-ignore-playwright-committed.sZ2T13/`，smoke SHA-256 `ade4335aeaefc5c2688d91955a6e5dd2123d943557cd36f4f32b1cb7aaf798fc`，protocol log SHA-256 `44d23d36aad212883732625750b556f851823f1263a5eb742833bd79fcd1292a`，migration SHA-256 `34930fd93b13b8e7aa7487ebbb50115d17532d5c39356597004e5afa51076211`。Astra light 终审为 0 blocker、0 major、0 minor、0 nit。完整 Cargo 原始日志在 `/private/tmp/ob021-ignore-gates.jBTQtk/`。

OB-021 ordinary input/textarea `maxlength` 用户编辑切片已覆盖 `Input.insertText`、带 `text` 的 `Input.dispatchKeyEvent`、textarea Enter 和官方 `Locator.fill()`。容量按 UTF-16 code unit 计算且不拆 Unicode scalar；`Input.insertText`（包括 Locator.fill）的单行换行归一为空格，textarea 归一为 LF。native edit 在 `beforeinput` 后重新读取目标、value、selection、readonly 与 maxlength，因此 handler 动态 grow/shrink、value/selection 重入和脚本已有超长值的 selection 删除都按最终状态执行。`beforeinput.data` 保留请求，`input.data` 只含实际接纳前缀；无选区且无可接纳文本时不修改值、不发送 `input`，有选区时即使没有插入容量仍删除选区并发送 `input(data="")`。Chrome 152 实测的 `insertText` 与 key-text caret 差异原样保留。

最终 committed binary 来自 `2fbcc0cc84464aa70b3576660bd6a3436a0f3a2f`，版本 `0.1.0-dev+2fbcc0c`，SHA-256 `a075816cd6858613fbf3e8524612d51c011971de922f6f768e96b203a4e9ad3b`，120798960 bytes。Chrome `152.0.7977.85` 与 Obscura `145.0.0.0` 的 8 组场景 exact 对照全部通过；maxlength 的 17 个子场景同时比较最终状态、完整事件序列与事件发生时的 target value/selection/maxlength。最终原始目录 `/private/tmp/ob021-maxlength-committed-final.ITi5Kb/` 有 621 个 artifact；`artifact-sha256.txt` SHA-256 为 `7ab2308eaa4da2e3808450d48831d7687ebfd2fbbfddf92fa404412e1bc04320`，结果 JSON SHA-256 为 `39512e97809b22fb61628c39f45431b708fbc43bdcaf04bda2d0d1021d16d750`，runner stderr 为 0 bytes。

runtime 聚焦 **3/3**（run `c83b0968-a62d-4b8b-9310-3ce903c48443`），CDP input integration binary **11/11**（run `984906fb-4b82-4bba-802f-f023f87a2d37`），完整 unblocked unittest **135/135**。最终 release/render 全工作区 **2339/2339**、4 skipped（run `7f8d84ee-ff66-41e7-b263-4db63ac83432`）；前面三次完整运行中的既有 MCP loopback/title fixture 偶发失败和各自精确复跑均完整保留，不称为修复。committed no-render/render exact build、固定 benchmark `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` obstacle **33/33** 均通过。官方 Playwright Python 1.60.0 smoke 通过，完整原始协议日志匹配 **37-method** profile；目录 `/private/tmp/ob021-maxlength-playwright-committed.8DcmOY/`，smoke JSON SHA-256 `2d587fd39c5a15aee475b36cfdc5d16a1eba76bbba5b0a416708b0390f77671d`，protocol log SHA-256 `926deea08a2c449abfcb547ace59497c4d834f894223d99973bd55b84da737c6`。Astra light 初审和加强后的终审均为 0 blocker、0 major、0 minor；终审的 68 次删字段 mutation 全被 comparator 拒绝。

contenteditable、IME/composition、grapheme/word 编辑、任意命令、平台快捷键默认动作和复杂表单默认动作仍未资格化；no-render 坐标/键盘输入仍明确 unsupported。该切片不关闭 OB-021。

OB-034 单 worker WebSocket handoff saturation 发布资格切片新增确定性真实 server gate：测试固定断言 channel capacity 为 128，在 receiver gate 保持关闭时排入精确 128 个 authorized upgrade，第 129 个取得 byte-exact 503/`ws-handoff-saturated`，observer 不得升至 129；随后在 gate 仍关闭时 shutdown，并用同一总 deadline 完成 server 与 128 个 client 的收尾。该测试不增加生产 hook，继续只使用既有 `cfg(test)` policy。

新增 `tools/unblocked/cdp_ws_handoff_capacity.py`，在 Darwin IPv4 loopback、单 worker、`max-connections=512` 下对指定 release binary 运行 3 轮、每轮 256 个 socket。工具先发送缺少末尾 `\r\n\r\n` 的完整 WebSocket head，以 listener queue 0、ESTABLISHED 精确 256、FD 增量至少 256、进程存活和线程采样形成稳定 barrier；再用 OS 确认的进程组 `SIGSTOP` 固定状态，发送全部 terminator 后 `SIGCONT`。每条连接必须分类为完整 101 或唯一完整 503/`ws-handoff-saturated`，所有 101 都继续发送唯一 masked `Browser.getVersion`、验证 matching result、发送 masked Close 并读到 EOF；恢复必须精确回到 FD 16、threads 12、queue/ESTABLISHED 0，再通过独立 HTTP 与 WebSocket/CDP probe。`SIGSTOP` 仅是外部资格同步，不是产品 hook。

最终提交 `bb1dbd4cdc769545b43abba77a71dce78b01665e` 的 render binary 为 `0.1.0-dev+bb1dbd4`，SHA-256 `f28e503665a900f83ebd929bbb1f98078cf1ca559a5d0f7fb05975952825ec7d`，120799200 bytes。最终目录 `/private/tmp/ob034-ws-handoff-final.tSD284/evidence/` 的 manifest SHA-256 为 `02e6cf97697ba90c570cfc18ac04ce718eed59716e50282fea8eac2bc2e877c1`，6916483 bytes，8638/8638 个登记 artifact 的 bytes/SHA-256 全部独立复核通过。三轮分别得到 167/89、175/81、176/80 个 101/503，全部 518 个 101 均完成匹配 CDP 往返；每轮 barrier/stopped 均为 FD 272、ESTABLISHED 256、listener completed 0，每轮恢复均为 FD 16、threads 12、ESTABLISHED/listener completed 0。stderr 原始 31000 bytes，精确包含 250 条 `WS handoff channel full (128)` warning，与 89+81+80 个 503 一致；SIGTERM 后 returncode 0、group gone、无 forced kill。首次未冻结调度的 `/private/tmp/ob034-ws-handoff-final.CRldKp/evidence/` 三轮全为 256 个 101，诚实保留为 exit 2/`not-qualified`，不冒充通过。

发布工具没有直接读取内部 queue occupancy；唯一响应、精确 warning 数量与完整恢复证明生产拒绝分支，精确 128 容量由上述确定性 Rust gate 证明。工具定向 unittest **19/19**、完整 unblocked unittest **133/133**；聚焦 render/no-render 各 **1/1**；render CDP **385/385**、3 skipped，no-render 有效集合 **322/322**、3 skipped、1 binary skipped；release/render 全工作区 **2332/2332**、4 skipped。两种 exact CLI build、固定 benchmark `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` obstacle **33/33**、官方 Playwright Python 1.60.0 smoke 与完整 **37-method** profile 均通过。Playwright 原始目录 `/private/tmp/ob034-ws-handoff-playwright.PhtGpC/`；`smoke.json` SHA-256 为 `889ea364acf09e88ce6f10f983ea517846aef88c1e91e7ef302aa34d866f5ccf`，protocol log SHA-256 为 `46b8bbd236e05788ff50afc68f7f343a4a28cc2df3f6b87ce78e4030c4047471`。Astra light 首审 0 blocker、2 major、2 minor，推动修复 stopped 状态清理、发送失败 wire salvage、loopback 参数约束和 exact-128 pin；终审 0 blocker、0 major、0 minor。全部命令、字节、stream、snapshot、结果、traceback 与 hash 原样保留。结论不外推非 Darwin、multi-worker、container、总 process/FD/RSS/V8/socket-buffer 或剩余 input qualification，OB-034 仍开放。

OB-034 单 worker accepted silent/incomplete request-head 切片以 Mio READABLE reactor 和最早 deadline heap 替换每毫秒线性扫描。基础 accepted-incomplete 上限为 256；另有固定 16 条、100ms classification reserve，所以 hard total 为 272。完整 request head 在基础容量饱和时仍可进入 HTTP/WebSocket 分类；reserve 到期或 hard cap 返回完整 503/`max-pending-request-heads`，半包在 10 秒 TTL 返回完整 408/`request-head-timeout`，全静默连接到期只关闭且不伪造 HTTP bytes。读取的原始 prefix 逐字节保留，WebSocket 101 写出后才解锁同一 TCP write 中已随 upgrade head 到达的 frame tail。shutdown 直接释放全部 pending，不等待 TTL；deadline heap 对 stale entry 有固定 compaction 边界。

最终 Darwin 24.6.0 arm64、IPv4 loopback、单 worker 实测先用 listener queue 0 与服务端 256 条 ESTABLISHED 证明全部连接已被接受；该状态下 HTTP 8/8 为 200、WebSocket 4/4 完成 `Browser.getVersion`，17/17 overflow 探针为 503。半包补全为 200，半包 TTL 为 408；256 条零字节连接在约 10.000 至 10.030 秒均取得空 wire 与 EOF。恢复后 queue/ESTABLISHED 回到 0、FD 16、threads 12；最后 256 条 held 连接在 SIGTERM 下全部空 wire EOF，进程 returncode 0、process group 消失、无 forced kill。完整目录 `/private/tmp/ob034-silent-pending-final.iAT6Kw/evidence/` 的 manifest SHA-256 为 `54310f40c6987d75c1489d6b0cc90b2bb5dd976bf14d58710e229c3ffb50daf1`，5893 个登记 artifact 的 bytes/SHA-256 全部复核通过。

最终 render binary 为 `0.1.0-dev+6968a9d`，SHA-256 `5eb589e3a158825c53c129847596e61b5b98af1cdad15dc7fab5170d1a12da84`，120799200 bytes。工具定向 **22/22**、完整 unblocked **114/114**；聚焦 render/no-render 各 **7/7**；render CDP **384/384**、3 skipped，无 render 有效集合 **321/321**、3 skipped。无 render 的原始完整命令有既有 render-only binary 的 6 个 `INPUT_UNSUPPORTED_WITHOUT_RENDER` 失败，保留为 284 passed、37 not run，不报告为全绿。release/render 全工作区首轮为 2330 passed、1 个既有 MCP deadline fixture 失败、4 skipped；未改源码的精确重放 **1/1** 与完整复跑 **2331/2331**、4 skipped 通过，不称为修复。两种 exact CLI build、固定 benchmark obstacle **33/33**、官方 Playwright Python 1.60.0 smoke 与完整 **37-method** profile 均通过；Playwright 原始目录为 `/private/tmp/ob034-silent-pending-playwright.IqUspe/`。

Astra light 首审的 blocker 为 0，但发现 6 个 major、2 个 minor，推动修复工具导入、HTTP/空集合验证、恢复验证、TTL 计时、失败清理、stale heap、资格范围和失败记录等问题；最终代码、工具与微小拒绝 helper delta 复审均为 0 blocker、0 major、0 minor。本机工具不证明 16 条 reserve 同时占满或 272 hard-total 状态，该边界由 Rust test observer 确定性覆盖；也不外推 Linux、Windows、container、multi-worker、总 RSS/FD/CPU/V8/socket-buffer 上限或 portable CPU limit。OB-034 仍开放。

OB-034 单 worker WebSocket admission 切片把 `--max-connections` 从 active processor 计数提升为 authorized upgrade handoff 与 active processor 共用的 RAII permit。permit 在完整且授权的 upgrade request 进入有界 handoff 前取得，覆盖 queued handoff、Page/V8/persistence 初始化和连接处理，直到 processor 清理后才释放；达到共享上限返回完整 503/`X-Obscura-Reason: max-connections`。handoff channel 满或关闭时返回完整 503/`ws-handoff-saturated` 并释放 permit；shutdown 先关闭 receiver、丢弃并释放 queued envelope，再等待 active drain。active 计数继续只服务 shutdown drain 与 idle trim，不再冒充 admission authority。

确定性 Rust gate 覆盖 queued permit、handoff full/closed、queued client reset、shutdown queue drain 和 active cleanup。真实 Darwin 工具用一 worker、`max-connections=1` 运行两轮 active/reject/recover：每轮 held WS 完成 101 与 `Browser.getVersion`，超额 WS 收到完整 503，原 held WS 在拒绝后仍可用，主动 Close 后 FD/thread/listen queue 精确恢复；最后在 active WS 存在时 SIGTERM，client clean EOF、进程 returncode 0、无 forced kill。最终 committed evidence 为 `/private/tmp/ob034-ws-capacity-final.XXvMow/evidence/`，manifest SHA-256 `00679f9def1549ce74f9167ac515a790995df904e55bfda24cb12e2b82d7ea48`，142 个登记 artifact 全部通过 bytes 与 SHA-256 校验；两次 recovery 均为 16 FD、12 threads、listen queue 0/128。

最终资格二进制来自提交 `ada9a69ba33aa28cb98e6cccd8a85bdd9e27194f`，版本 `0.1.0-dev+ada9a69`，SHA-256 `be2fbf71ea4e4da5d1c1d480ca49589fafda33512f3c71f4e406cd9d375f40cf`，120763696 bytes。工具定向 unittest 12/12、完整 unblocked unittest 92/92；聚焦 render/no-render 各 7/7（runs `31a36263-d05e-40bf-ac70-5027e6b7b14d`、`290927a0-a2d9-4366-8f89-fba028ec3e7e`）；render CDP 377/377、3 skipped（run `cd4c545c-f65b-4c38-9cc6-726bbc58f006`），no-render 排除既有 render-only binary 后 314/314、3 skipped（run `a6713caf-f5ba-40fb-9740-8a5d9175d353`）；release/render 全工作区 2324/2324、4 skipped（run `731021cc-0b61-4c26-8a3a-265a04438e8e`）。两种 exact CLI build、固定 benchmark `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 的 obstacle 33/33、官方 Playwright Python 1.60.0 smoke 与完整 37-method profile 均通过；Playwright 原始目录为 `/private/tmp/ob034-ws-admission-playwright.nPxkn2/`。

Astra light 首审指出工具在 read exception 时可能丢弃 partial wire，以及 post-rejection command 可能把 matching-id error 当成功；修复后终审为 0 blocker、0 major、0 minor。两次 FileExistsError 失败证据保留在 `/private/tmp/ob034-ws-capacity.C4oTQY/evidence/` 与 `/private/tmp/ob034-ws-capacity.QXRnYK/evidence/`；提交后首次证据 `/private/tmp/ob034-ws-capacity-committed.r438or/evidence/` 因 Cargo 复用旧版本 `9f33502` 而被保留但不作为最终资格。该切片只资格化 Darwin 24.6.0 arm64、IPv4 loopback、单 worker 的 active/shared admission；handoff saturation 由真实 loopback Rust gate 确定性覆盖，发布工具未稳定制造该内部状态。silent-pending 仍有独立数量/TTL 边界，Linux、Windows、container、multi-worker 与总 process/FD/RSS/V8/socket-buffer 上限均不继承，OB-034 保持开放。

OB-034 multi-worker child lifecycle 切片已完成受控 child readiness、startup failure cleanup、child crash fail-fast、parent-only shutdown、parent SIGKILL 后 stdin EOF 退出，以及 serve 参数与 access behavior 的完整投影。父进程先绑定公开 listener；child 只在 listener/access/Mio accept/V8 初始化完成后写出单条精确 JSON readiness；任一 child 意外退出都会停止、drain 并 join relay，再通知和回收 siblings。multi-worker 共享 `--storage-dir` 因 ownership 未定义而在 spawn 前明确拒绝。

`tools/unblocked/cdp_multi_worker_lifecycle.py` 保留全部原始 argv/env override、stdout/stderr、PID/process table、signal、HTTP request/response、traceback 与 hash，对 public/worker port conflict、ready child crash、parent-only SIGTERM + 参数/access 投影、parent SIGKILL + stdin EOF 五个场景实测 **5/5**。最终 committed evidence 位于 `/private/tmp/ob034-lifecycle-committed.cBlBR7/evidence/`，manifest SHA-256 `45d0dbd62e15dd5e9cc95d8ce8b33c6a6c573b9ffe948ec30dc3d3384d209f91`，73 个 artifact 全部通过 hash 校验；所有进程组消失、server streams 稳定，无 forced cleanup。Astra light 独立复核代码和 committed evidence 均为 0 blocker、0 major、0 minor。

最终资格二进制来自提交 `9d5ceab111ecc8c654cfc6c7281b16c92a2a0997`，版本 `0.1.0-dev+9d5ceab`，SHA-256 `8a3a7f27e5010ec05ec4f10d1a453223d7397e7f1a5242f27c04c42745e39143`，120705920 bytes。工具 unittest 8/8、完整 unblocked unittest 80/80、no-render CLI 88/88（run `1cc31c25-ce22-4dbc-8c2d-b3237fbc7560`）、release/render 全工作区 2318/2318 且 4 skipped（run `56c3ad40-95ae-43bc-b8b1-0e1b5bb305d2`）、两种 exact build、obstacle 33/33、Playwright Python 1.60.0 smoke 与 37-method profile 均通过。Playwright 原始证据目录为 `/private/tmp/ob034-lifecycle-playwright.0IBf1Z/`。结论只限 Darwin 24.6.0 arm64 与该 binary，不外推 Linux、Windows、container、自动 restart/session migration、共享存储 ownership 或总资源上限；OB-034 仍开放。

OB-034 multi-worker parent relay 已移除无 timeout `peek()`、request-line 解析和 `/json` 特判，父进程对 HTTP 与升级后的 WebSocket 都执行 byte-transparent relay。aggregate relay 上限是经溢出检查的 `workers * max-connections`，permit 覆盖 worker connect、完整 relay 与 502；到达上限时先在 100ms 总预算内发送并 flush 完整 503，再 shutdown write 和有界 drain。固定 16 条拒绝任务及 relay task 全部由持续 reap 的 `JoinSet` 管理。聚焦 render 回归 **5/5** 两轮通过，Astra light 代码终审为 0 blocker、0 major、0 minor。

`tools/unblocked/cdp_multi_worker.py` 用 host socket table barrier 证明真实零字节首 client 已由父进程接纳并连到 worker，再要求后续 discovery 完整 200；容量阶段用完成 101 的 WS 占满 aggregate relay，要求大于 4 KiB 且不 half-close 的请求取得完整 503。主动 Close 一条后，另一条必须仍映射且完成 raw `Browser.getVersion id=2`，HTTP 与全新 WS id=1 也必须恢复。全部 request/response/frame/payload、socket probes、host command/server streams、traceback 与 hashes 原样保留。完整工具 unittest **72/72**，Astra light 工具与既有原始证据终审为 0 blocker、0 major、0 minor。该 parent relay 切片当时未覆盖的 Darwin worker readiness/crash/reap、parent-only shutdown 和参数传递已由上方新切片补齐；Linux/Windows/container 仍未资格化。

最终资格二进制来自提交 `a82aab13cca8f0227b6ccb28dc46f628f5fdca55`，版本 `0.1.0-dev+a82aab1`，render SHA-256 `e180178c997dbd51b824e0cd8784638dba81ba0f0aa5d53ca173f7d6cf994a67`，120504448 bytes。no-render 聚焦 **5/5**、CLI **86/86**（2 leaky），release/render 全工作区 **2316/2316**、4 skipped；两种 exact build、固定 benchmark obstacle **33/33**、官方 Playwright 1.60.0 smoke 与 **37-method** 原始 protocol profile 均通过。最终 Darwin 2 workers x 1 connection 运行通过所有 barrier，完整 raw evidence 位于 `/private/tmp/ob034-multiworker-final.adE5nF/evidence/`，manifest SHA-256 `d4c9430fe9d3a6ed39b61b78cf9918909a57cdcc80c1a4036b35f1dae1ce6d59`，139 个登记 artifact；TERM 后 process group 完全消失，无 forced kill。

OB-034 现有单 worker 实际 OS listen backlog 资格工具。它在 readiness 与基线 snapshot 后向整个服务进程组发送 `SIGSTOP`，由内核确认 stopped 后再同时释放完整 discovery 请求；成功连接因此只能留在 kernel listen queue，不会先进入 256 条 accepted silent-pending。通过条件同时包括目标 queue 达其报告 maximum、服务数字 FD 不增长、压力项只能是 connect timeout、恢复后所有已发送请求取得完整 200、queue/FD 回到基线，以及新 WebSocket 完成 101 和 raw `Browser.getVersion` 往返。所有 request/response、frames、host command stdout/stderr、server byte streams、snapshot、failure traceback 和 hashes 完整保留。

最终本机 Darwin 24.6.0 arm64 运行使用上一切片的最终 render binary：240 次并发 connect 中 128 次完整请求进入 `128/128` listen queue，112 次 connect timeout，其他 client failure 为 0；进程 stopped 期间数字 FD 和 RSS 保持 16、19232 KiB，128 条恢复响应全部完整 200。恢复后 queue 为 0、FD 为 16，HTTP 200、WebSocket 101、CDP response id=1 和 masked Close 后 clean EOF 均成功；RSS 为 20624 KiB，server stderr 0 bytes，SIGTERM returncode 0。RSS 只是采样，不是上限。Astra light 三轮审核推动修复失败路径死锁、伪 pressure、平台解析、异常证据丢失和 executor post-enqueue submit failure，最终 0 blocker、0 major、0 minor。该结果不外推 Linux、Windows、容器、multi-worker、总 RSS 或其他三层逻辑容量，OB-034 仍开放。

OB-034 的 server admission 与 Worker terminal-boundary 资格切片已移除此前两处推断。main/iframe 在 active 同步循环后发出的 id=3 只有在真实 WebSocket reader 成功调用 inbound `send` 后才由 per-server test observer 确认；outer I/O abort、sticky server shutdown 和真实 writer 两类终止源都等待该 barrier，再断言 queued marker 未执行。Dedicated Worker 则通过 connection-local thread registration 从 1 到 0，直接证明 runtime/lease 已释放、completion handoff 已尝试且 closure 到达 terminal return boundary；这不是 OS thread join 声明。

真实 loopback TCP writer 的 server write-half failure 与小 buffer/non-reading client backpressure timeout 现各自覆盖 main、iframe、active Worker。main/iframe 在 8 MiB 合法响应已排入 writer 后进入同步循环并将后续命令准确入队；Worker 先以 `postMessage` 和 thread count 证明活跃。每项同时核对精确 terminal reason、新 writer 日志、socket close、slot 与 Worker lifecycle 回收。本机聚焦 render 为 CDP 4/4 加 Worker 1/1，no-render 为合并 5/5；Astra light 在推动修复 owner-alive 计数等待竞态后复核为 0 blocker、0 major、0 minor。该结果不外推到其他产品平台，OB-034 仍开放。

OB-034 的 server-side terminal-source 现把 writer I/O failure、writer timeout、server shutdown 与 outer I/O task cancellation 连接到同一 connection-local sticky cancellation。生产默认仍是 10 秒 writer/flush deadline；私有测试 policy 只缩短 deadline 并被动回报终态，不暴露产品配置。真实 loopback TCP 用例在 active Dedicated Worker 存活时分别用 server socket `shutdown(Write)` 和小 TCP buffer + 不读客户端 + 8 MiB 合法响应制造 `WriterIo` 与 `WriterTimeout`，同时核对完整 writer 日志、socket 关闭和 slot 回收。原受控 `Sink` 用例保留为 unit helper，不再称作 wire 资格。

server shutdown 由 sticky watch 发布，并以 Mio OS waker 立即唤醒 accept Poll，覆盖 cancel-before-register 和 idle accept。listener readiness 保持事件驱动；生产 loop 使用的确定性批次状态机证明单轮命中 256 accept 上限后下一 sweep 跳过 Poll，只有 `WouldBlock` 才重新阻塞，并且该回归不接触同为 256 的 silent-pending 上限。同步 HTTP discovery 设置 1 秒 socket 读写超时，拒绝路径设置 100ms socket 读写超时。最初 10ms polling 版本使 200 轮交替 `/json/version` median 相对基线回退到 5.07 倍，已弃用；最终版本对 `757e8a0` 的 median 为 1.763ms 对 1.994ms（0.884 倍），p95 为 2.050ms 对 2.251ms。该确定性测试不是 OS backlog burst 或 admission 前 TCP/kernel/container capacity 资格。OB-034 继续开放。

OB-034 的直接客户端断连矩阵现在覆盖 WebSocket Close、原始 FIN、linger-zero RST x main、iframe、active Dedicated Worker 的 3 x 3 组合。main/iframe 以第一轮同步循环 console marker、Worker 以第一轮 `postMessage("started")` 证明执行已进入对应 realm；Close 保留并排空旧 socket，FIN 保留读半边，避免测试输入被客户端 drop 改写。每项都在 `max_connections=1` 下要求旧 slot 释放并由新连接执行 `42`。

connection cancellation 同时通过 sticky watch 主动唤醒仅等待 `commands.recv()` 的 idle Worker，以及停在远期 timer/I/O autonomous future 的 Worker。owner 与 Worker object 保持存活的精确测试先观察真实 wait-state，再要求 lease 在一秒内归零。server shutdown 与 outer I/O cancellation 的完整 connection 回归现各自覆盖 main、iframe 和 active Worker；main/iframe 保留后续命令未执行证据，Worker 以 `postMessage` 证明先进入同步循环。slot 归零只证明 processor/Page 已释放，不单独证明 Worker thread/lease 已退出；后者仍由既有 owner-alive 精确测试提供。main/iframe 后续命令也没有独立 admission barrier，不外推成精确入队竞态资格。

Fetch response stream 已完成 shared-consumption 边界。Page 的 canonical response body 不再因 `takeResponseBodyAsStream` 被替换为 consumed tombstone；Network/Page、persistent history 和 stream 共享 immutable raw backing。Fetch 自身仍按 Chrome 契约维护 canonical access 状态：重复 `Fetch.getResponseBody` 可用，但 get 与一次 take 互斥，alias 不能绕过；普通 `Network.getResponseBody` 在 stream 打开后仍返回完整正文。Fulfill replacement 产生新 generation，Network 读新 body，旧 stream 继续读旧 body。

Fetch IO handle 归属 target：同 target sibling flattened session 可顺序 `IO.read`/`IO.close`，其他 target 和失效 session 拒绝。disable、导航和 Page body clear 保留 handle；last target detach、target/Page close、context/connection teardown 回收 Fetch handle；PDF 等非 Fetch handle 保持 exact-session 原语义。长度读取、IO admission 和 Fetch access 转换位于同一 Page body-store guard 内，commit 再核对实际长度，关闭了并发 Fulfill replacement 绕过预算和计数下溢窗口。Chrome 152 原始探针确认 get/take、sibling、disable 和 last-detach 边界；未解除的 response pause 上 Network body 仍不可读。

Astra light 首审发现 replacement/reservation 竞态；完整 CDP 首轮暴露 PDF stream 误清理。两处修复后终审为 0 blocker、0 major、0 minor。该 Fetch stream 边界已完成，OB-021 的其他迁移项与 OB-034 仍未关闭。以下更早阶段末尾的“未完成”列表是各阶段当时的历史快照，当前边界以上述最新结论为准。

Document、Stylesheet、classic Script 与 render Image/Font 已通过共享 `RequestTrace` 接入普通 native Network lifecycle：最终 prepared raw request headers/body 已知后、首次 transport send 前同步接纳 Started，并以同一 logical request ID 产生 exactly-one Redirect/Finished/Failed terminal。每个 redirect hop 保留完整响应 headers/body；无效 Location、SSRF/mode 拒绝与 redirect 上限直接形成带真实响应事实的 Failed。普通请求先写 context-owned persistent history，再进入 Page/CDP live queue；导航期间共享 Notify 实时 drain，跨 batch 保留 `redirectResponse` 并清理 retired generation 状态。Page response-body store 预算失败仍显式暴露，但 persistent history 可按自身预算保存生产者已完整取得的 raw body。

render Page/runtime 关闭会先拒绝新 start、发布 cancellation、终止活动任务并同步写 `Aborted` terminal，再关闭 history writer。start observer callback、内部 `started` 提交与 shutdown cancellation 由同一 lifecycle fence 串行；精确 barrier 回归覆盖 observer 已接纳 Started、内部状态尚未提交的旧竞态，证明 close 后不会继续 transport 或留下 Started-only history。

`Network.enable` 的 `maxTotalBufferSize`、`maxResourceBufferSize` 与 `maxPostDataSize` 现按有效 Page session 独立保存。`maxPostDataSize` 按 raw UTF-8 byte length 只投影标准 `requestWillBeSent.request.postData`、`postDataEntries` 及扩展 `postDataIsByteString`；canonical standard/transport body、raw headers、Fetch pause 与 `Network.getRequestPostData` 保持完整。省略、null、0、负数不限，正整数 exact threshold，浮点/字符串拒绝，未知字段接受；重复 enable 只影响未来事件，disable 清理该 session 参数与读取资格。Chrome 152 原始探针确认上述语义。

Astra light 三轮复核推动修复 redirect 策略失败终态、跨批次 `redirectResponse`、render close terminal 与 start/close 窄竞态，最终为 0 blocker、0 major、0 minor。普通 native start、Page body budget 与 per-agent `maxPostDataSize` 这一相邻边界已完成；该阶段当时的下一段 Fetch stream 已由上方最新切片完成。OB-021 整体仍未关闭。

每个 `BrowserContext` 现拥有 append-only `NetworkHistory`。记录使用全局单调 sequence 与永不复用的 page-instance ID，跨导航、Page 关闭和同一 context 多 Page 保留完整 observation metadata、精确 raw request/response headers 及 immutable request、transport-request、response body 引用。默认边界为 4096 条记录和 4096 个 page instance、64 MiB metadata、16 MiB 单条、512 MiB unique body bytes、32768 个 body entry、640 MiB persistent journal。首次 count/bytes/serialization/I/O/producer/close failure 保留 accepted prefix 并成为 context-wide sticky terminal，停止现有 sibling runtime、Worker 和 native producer 的后续网络工作；不 eviction、不截断、不脱敏。

不指定存储目录时历史在 context 生命周期内以内存形式提供；`serve`/`mcp --storage-dir` 使用 versioned manifest 与 checksummed length-framed journal，完整 frame 写入并 `sync_data` 后才算接纳。恢复只接受 checksum-valid committed prefix，并把 incomplete/corrupt/over-limit tail 或 missing body 作为结构化 recovery failure 独立暴露。MCP 的 `browser_network_requests` 已改读该 authority，并新增 `browser_network_histories`、`browser_network_history`、`browser_network_body`；CDP browser-level 扩展新增 `Obscura.getNetworkHistories`、`Obscura.getNetworkHistory`、`Obscura.getNetworkBody`。两侧均支持 bounded sequence/page-instance 查询和 repeatable body chunk 读取。

Astra light 独立审核发现并推动修复 producer queue failure 早于 accepted prefix、native POST redirect 中间 response body 缺失、CDP discovery 不报告损坏 archive、response-stage Fulfill 保留旧 body、以及 sibling Page 未共享 context-wide upstream stop 五类问题；最终复审无 blocker、major 或 minor。普通非拦截资源 start emission、Page body budget 与 Chrome per-agent `Network.enable.maxPostDataSize`、Fetch stream 多消费者仍未完成，OB-021 保持未关闭。

Page 现有独立 request-body store：默认 2 MiB 后 spool、256 MiB unique raw bytes、16384 canonical entries，支持三项 `OBSCURA_NETWORK_REQUEST_BODY_*` 环境配置。预算失败和 I/O 失败 sticky，保留 accepted prefix，并在发送前拒绝后续需要新增正文 capture 的请求；无正文请求不受该正文预算影响。不 eviction、不截断、不脱敏。显式空 body 与缺省 body 严格区分；相同 standard/transport 或 307/308 redirect body 共享 raw bytes 但各占 canonical entry，bodyless logical alias 不积累 tombstone。

JS fetch/XHR、Request、module、Dedicated Worker、原生表单导航和 native redirect 统一产生稳定 per-hop standard/transport IDs。302/303 清 body，307/308 保留；Fetch pause 看到 override 前 standard body，Continue override 另存 transport body。CDP 事件保留精确 body presence/size/ID、UTF-8 `postData` 或完整 base64 entries；`Network.getRequestPostData` 支持 logical current hop 与 canonical ID，服从 start-time Network session ownership。原生 redirect 链按 hop 顺序复用同一 loader requestId。MCP 按 ID 物化完整 standard/transport body；observation queue 只携带 metadata/ID。

Astra light 复核发现并推动修复 native POST redirect 误保留原 body、bodyless tombstone 无界增长、CDP native redirect 链拆分/重排和 loader alias 未解析 canonical entry；最终复审无 blocker、major 或 minor。`Network.enable.maxPostDataSize` 的 per-agent 语义、持久 observation history、普通非拦截资源 start emission/Page budget 统一和 Fetch stream 多消费者仍未完成，OB-021 保持未关闭。

JS fetch/XHR、module loader 与全部 Dedicated Worker 已不再使用 4096 条 oldest-drop network observation queue。owning Page 共享 4096 条、64 MiB 完整序列化 metadata、16 MiB 单条的原子 admission budget；batch 全有或全无。已接纳记录携带 reservation，在 active、teardown 和 Page drain 间移动不重新计数。Worker 网络记录不再经过通用 Worker event channel，而是按 Worker 生命周期顺序直接写 Page teardown FIFO；`ObscuraState::drop` 覆盖初始化失败、取消和 runtime 退出，保证残余 accepted records 最终回灌。

首个 count、总 bytes、单条 bytes 或 serialization failure 为共享 sticky terminal。生产者立即拒绝后续 scripted network 工作；consumer 只有在全部 sibling queue 的 accepted reservation 已释放、accepted prefix 已进入 Page 后才可见 terminal。CDP 对每个 Network-enabled Page session 在 accepted events 后最多一次发送 Obscura 扩展 `Network.observationFailed`，晚启用 session 仍能收到；MCP 正常形状仍为 `{ "events": [...] }`，失败时才增加完整 `terminal_failure`。Cookie、Authorization、重复字段、NUL、`0xff` 与全部 256 种 header byte 均参与完整序列化预算并原样保留，不脱敏、不删字段。

Astra light 复核发现并推动修复 Worker closing send/receiver drop、terminal 早于 accepted prefix、Worker 生命周期重排、以及初始化失败/取消/runtime exit 残余记录丢失；最终复审无 blocker、major 或 minor。该切片只替换上游 scripted observation oldest-drop。`Page.network_events` 仍是 active Page 内存历史，不是持久或崩溃恢复 journal；response body store 仍独立，请求正文、导航/多页面持久 history、普通非拦截资源 start emission、Page body budget/Chrome per-agent 参数和 Fetch stream 多消费者仍未完成。OB-021 保持未关闭。

CDP `Network.enable` 已从 Fetch pause 的单 owner 路由中拆出，成为有效 Page session 的独立订阅。Network start/response/terminal 按各阶段产生时的 enabled session fanout；请求开始时的 session 快照单独决定 requestId、redirect body alias 与 Document loader alias 的 `Network.getResponseBody` 权限。晚启用者可以看到启用后的阶段但不能读取启用前开始的 body；disable、detach、Page close 和连接关闭只清理对应 session，重启用不会恢复旧权限。sessionless enable/disable 保持兼容 no-op，不清空兄弟订阅或共享 raw store。

Fetch pause ownership 保持独立：Fetch-only owner 不会自动接收 Network 事件，Network body 查询使用标准 Network requestId，不接受 `intercept-N` 越权读取。store-wide body budget failure 以每 session 单一 capability 表示，不随失败 requestId 数量增长，且不能解锁已经 retained 的旧 body。公开 `network_owners` 字段只作为低层源码兼容占位保留，生产路由不再读写。

Chrome 152 原始对照和官方 Playwright Python 1.60.0 连接 Obscura 的双 session 探针均确认早/晚订阅、共享 ID、独立正文权限、disable/re-enable 和 Fetch/Network 分离。Astra light 三轮审核发现并推动修复了 failure ID 无界增长、failure capability 读取旧 retained body、以及 Fetch fulfill alias 提前授权三类风险；最终复审没有 blocker、major 或 minor finding。

该阶段仍没有修复上游 JS/Worker 4096 条 oldest-drop observation queue、持久 observation history、请求正文保留、Page body budget 与 Chrome per-agent 参数差异、普通非拦截资源 start emission 的统一架构或 Fetch stream 共享消费。OB-021 保持未关闭。

上一阶段的 passive callback ownership 结论仍有效：

Page、iframe 与 Dedicated Worker 共用的 passive `CallbackRegistry` 已从 `tokio::sync::RwLock<Vec<_>>` 的 `try_write` 改为短时 `std::sync::Mutex` 保护的 copy-on-write immutable `Arc<Vec<_>>` snapshot。旧实现可能返回一个未实际安装的 callback ID，或在 dispatch 持有 read guard 时把真实 remove 报成 false；这两类静默 observation loss 已消除。

dispatch 在 mutex 内只复制 snapshot，随后释放锁并按登记顺序调用 callback。callback 中的 add/remove 因而可重入且不死锁；mutation 从下一条 observation 生效，当前 snapshot 完整送达。公开 callback 签名、ID 和顺序未改，`RequestInfo`、`Response`、binary body、兼容 header map 与 raw header bytes 原样交给观察者，不脱敏、不删字段。

Astra light 首审发现 remove 用 `retain` 时会在 mutex 内 drop 最后一个 callback Arc；若 capture 的析构函数重入 registry，会自死锁。最终实现把命中的 tuple 移出 guard 作用域后再析构，并增加 request/response 两条一秒有界的 mutation-sensitive 析构重入回归。复审没有 blocker、major 或 minor finding。

该阶段只修复 native passive callback ownership。callback panic 仍未隔离；CDP 多 session `Network.enable` subscription/fanout 已由最新阶段完成，但上游 JS/Worker 4096 oldest-drop、持久 observation history 或请求正文保留仍未解决。

## 验证结果

- 最新单 worker OS backlog 工具定向 unittest 14/14；完整 `tools/unblocked` unittest 66/66。最终本机资格运行 `status=passed`：240 attempted、128 TCP/request sent、112 connect timeout pressure、0 other client failures；queue `0/128 -> 128/128 -> 0/128`，server FD `16 -> 16 -> 16`，128/128 完整 HTTP 200，无 response error；恢复 HTTP 200/完整 body、WebSocket 101、`Browser.getVersion` id=1 成功，server stderr 0 bytes，SIGTERM returncode 0。
- 最终 raw manifest `/private/tmp/ob034-capacity-final-reviewed.IlqITE/evidence/evidence.json` SHA-256 `1dde72f3b0ecba3e8621cec1ed9e5fccca5697f70242009cf4cb599e425b907a`，共引用 1055 个原始 artifact；目录中实际含 manifest 在内 1056 个文件、约 4.0 MiB。Astra light 第三轮终审为 0 blocker、0 major、0 minor。

- 最新 admission/Worker terminal-boundary 切片聚焦 release nextest 在 render/no-render 下均为 5/5（runs `f7b25746-13fc-4ac6-8711-a9ec88f7ba13`、`d49e1e5a-9876-4bd7-9243-25c487833e8a`）；render CDP 全量 371/371、3 skipped（run `37b79f7e-ad15-48da-a42c-f9f62d6cbeec`），no-render 排除既有 render-only `input_key_event_escaping` binary 后 308/308、3 skipped（run `a21a55a9-05ad-4bd2-b800-02201138d56b`）。
- 根 release/render nextest 为 2311/2311、4 skipped（run `01fb85ca-4e2a-4bfc-bca2-c22efdc5eb82`）；exact render 与 no-default-features CLI build 均成功。最终 render SHA-256 `ea23d498afe9988536ed1860012a5c4e550e9c89e0fd6fad1a73a0947dcecee7`，120491552 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33；首次漏传必填 persona 的 0/33 配置失败原样保留。同一最终二进制通过官方 Playwright Python 1.60.0 smoke，完整原始协议日志匹配 37-method profile。Astra light 最终复核为 0 blocker、0 major、0 minor。

- 最新真实 writer/Mio 切片聚焦 release nextest 在 render/no-render 下均为 6/6（runs `1caf4a9c-62d1-4467-b911-e66402ca4bdf`、`91614911-e7f3-4a16-944d-091bfd6d1621`）；render CDP 全量 371/371、3 skipped（run `4dd5d821-01f1-4dd7-98c4-b62ec2a8000f`），no-render 排除既有 render-only `input_key_event_escaping` binary 后 308/308、3 skipped（run `a9e5dfd0-73f7-49f6-a3cb-0a2247ed3077`）。
- 根 release/render nextest 为 2311/2311、4 skipped（run `d7b1445e-1de4-43fb-960c-a249fd834813`）；exact render 与 no-default-features CLI build 均成功。最终 render SHA-256 `91931ebd5e54467c809c2d4522a8d35cf8bc96b17bb9136325baf437e3e2092b`，120471088 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33；同一最终二进制通过官方 Playwright Python 1.60.0 smoke，完整原始协议日志匹配 37-method profile。
- 200 轮交替 discovery 探针中，基线/候选 median 为 1.762/1.514ms，p95 为 2.031/1.805ms，median 比率 0.859。Astra light 独立终审为 0 blocker、0 major、0 minor；残余资格边界见本交接顶部。

- 最新 server-side terminal-source 聚焦 release nextest 在 render/no-render 下均为 7/7（runs `f8b3c02a-3aaa-4b8f-b6ff-be26e26f999a`、`b6667ede-5039-402a-9c54-a4f8becafddc` 中的相同七项）；render CDP 全量 367/367、3 skipped（run `86164ddd-458f-48a4-b896-c5efec8ecacb`）。
- no-render CDP 排除既有 render-only `input_key_event_escaping` binary 后 304/304、3 skipped（run `b6667ede-5039-402a-9c54-a4f8becafddc`）；原始全量的 6 个 `INPUT_UNSUPPORTED_WITHOUT_RENDER` 失败证据保留，不把该配置报告为全绿。
- 最新根 release/render nextest 最终 2307/2307，4 skipped（run `df85f443-7b56-4718-b029-4427d83cff9c`）。此前首轮根门禁的两个既有 MCP loopback fixture 偶发失败未在源码未改的最终全量中复现，不把复跑通过称作修复。
- exact no-default-features 与 render CLI build 均成功；最终 render SHA-256 `1d1b93e10f8ba4e0e548ca6e312707ea582a5e6cd721e3ecd9bcee2bfc309488`，120468976 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33。
- 同一最终二进制通过官方 Playwright Python 1.60.0 automation smoke；完整原始协议日志通过 37-method profile 校验。200 轮交替 discovery 探针中，基线/候选 median 为 1.994/1.763ms，p95 为 2.251/2.050ms。
- Astra light 首审的 Mio edge-triggered batch finding 已修复，复审 0 findings；`git diff --check` 通过。

- 前一直接断连矩阵阶段：断连专项 release nextest 在 render/no-render 下均为 8/8；Worker 广义定向两种 feature 均为 43/43。no-render 的 `offscreen_webgl_owns_its_context_in_window_and_worker` 被 nextest 标记为 1 leaky，测试仍通过，本轮不宣称已解释或修复该退出期资源观察。

- Fetch/IO/PDF/target review-fix 定向回归 6/6（run `011162eb-00aa-4309-9e4a-ba3147de8740`）。
- `obscura-cdp` release/render 全量 359/359，3 skipped（run `ecebb351-1585-422a-b31b-ea8eb9bc4134`）。
- 根 release/render nextest 2296/2296，4 skipped（run `4ee20af2-0968-4fe3-a859-6af72abed67a`）。
- exact no-default-features 与 render CLI build 均成功；最终 render SHA-256 `9b1f39d1ec2e232bad17d17a84b926ac5a39fd3a3954db5b01b018230d19fa94`，120496336 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33。
- 同一二进制通过官方 Playwright Python 1.60.0 automation smoke；完整原始协议日志通过 37-method profile 校验。
- Astra light 第二轮终审无 blocker、major 或 minor；`git diff --check` 通过。

- latest race/close/redirect focused release 回归 5/5（run `7b5ddeca-936c-4950-b7c3-2e11a9259705`）。
- latest affected net/js/browser/cdp release/render nextest 1475/1475，3 skipped（run `9c356372-cca7-429d-bfce-219385c8b605`）。
- latest full release/render nextest 2293/2293，4 skipped（run `6c542be6-271c-44db-b730-db227fe05a83`）；另有既有 render 测试 `retained_mixed_outer_has_dependencies_match_forced_full` 被 nextest 标记为 leaky 但通过，退出码为 0。
- latest exact render 与 no-default-features CLI build 均成功；最终 exact render SHA-256 `90d483c000b2371ba83a5e678a8c0837d7ed5f1f62b0ed10a177a6881dc95767`，120531952 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33；首次漏传必填 persona 的 0/33 配置失败完整保留。
- 同一二进制通过官方 Playwright Python 1.60.0 automation smoke；原始协议日志通过 37-method profile 校验。
- Astra light 第三轮终审无 blocker、major 或 minor；`git diff --check` 通过。

- persistent history browser+CDP release 回归 678/678，3 skipped（run `a61a325c-82a0-48f8-aa15-654a4373f685`）。
- full release/render nextest 最终复跑 2277/2277，4 skipped（run `90245ddb-30df-43d2-bc7a-aed6d1a67219`）。首次全量唯一失败为既有 MCP `test_evaluate` 空标题；源码未改的两项复跑 2/2（run `49ed9390-3de4-4f72-89d2-a427bc803d00`）后全量干净通过，不把该偶发复跑通过称作修复。
- render 和 no-default-features 两种 exact CLI release build 均成功；最终 exact render SHA-256 `e92740a871748d0588e66e8a3473d00eb6cd9aeae7efc2eb8d06550ffe3dc48b`，120453184 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33。
- 同一二进制通过官方 Playwright Python 1.60.0 automation smoke，完整协议日志匹配 37-method profile。
- Astra light 终审无 blocker、major 或 minor；`git diff --check` 通过。

- request-body 聚焦批次 18/18、14/14、review fixes 6/6、CDP redirect fixes 4/4。
- full release/render nextest 2255/2255，4 skipped（run `4a6d3e2f-b260-4acb-8f30-0f84dcf0d814`）。
- render 和 no-default-features 两种 exact CLI release build 均成功；最终 exact render SHA-256 `20b8ef6e6cead3235f0a0c96bc3d31b235c5821de7a790a07a78ca20375cde8e`，119759824 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e` 在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33。
- 官方 Playwright Python 1.60.0 request-body E2E 与 required automation smoke 通过；完整 smoke 协议满足 37-method profile。
- Astra light 终审无 blocker、major 或 minor；最终 `git diff --check` 在提交前再次执行。

- 本轮最终 render 聚焦回归 9/9（run `94afe313-7601-45a6-8c2f-3a23a1179694`）；no-render 聚焦回归 12/12（run `04e0e6e3-8e1d-42a9-9c8c-8c0e18a0d14e`）。
- full release/render nextest 2241/2241，4 skipped（run `4fc14590-dbe5-4b89-866d-61f1c75d0077`）。
- render 和 no-default-features 两种 exact CLI release build 均成功；最终 exact render SHA-256 `ca94b2e0f75b6ae55678a1dad6cca85ace0c51d984606396f7e2a8ce1d442bc7`，119519872 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`：在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33，包括 `observer-intersection`。
- 同一最终二进制通过官方 Playwright Python 1.60.0 automation smoke，完整协议日志通过 37-method profile 校验。
- `git diff --check` 通过；Astra light 终审无 blocker、major 或 minor。

前一 Network ownership 阶段的验证仍保留如下：

- Network review-fix 专项 7/7（run `6fcc6e68-20ae-401a-8e57-ffb86d19736d`）；最终 `obscura-cdp` render 344/344，3 skipped（run `4b1786ae-d695-479d-8979-3cec59c7d1d2`）。
- full release/render nextest 最终 2232/2232，4 skipped（run `bcc3e519-f32f-4e6a-9d1e-64273d3dd91e`）。首次运行唯一失败为 MCP fixture 连接建立偶发错误；未改源码单项重放 1/1 后全量干净通过。
- no-render `obscura-cdp` 为 283/289，另有 3 skipped；6 个失败均是该模式既有 `Input.*` render 用例，本切片 Network/Fetch 用例通过。该完整失败日志保留，不把它报告为全绿门禁。
- render 和 no-default-features 两种 exact CLI release build 均成功；最终 exact render SHA-256 `0ff5bb5716666f830c9e19be63ee9cb3324e82946bc26f11a6f28286a881a0b2`，119562512 bytes。
- benchmark revision `2340bbb9aea6b8812ff20b7f29113c7c1f9a4b6e`：在 `OBSCURA_PERSONA=windows_chrome145` 下通过 33/33，包括 `observer-intersection`。
- 官方 Playwright Python 1.60.0 automation smoke 通过；Obscura 双 session Network 原始探针断言通过。首次 smoke 漏传必填 persona 与双 session 探针首次没有等待事件分发的失败证据均完整保留。
- `git diff --check` 通过。
- Astra light 终审为静态审核，执行结果以上述 nextest、官方客户端和 obstacle 证据为准；终审无 blocker、major 或 minor。残余边界见上节及 [SUMMARY](SUMMARY.md)。

## 后续执行顺序

OB-021 和 OB-034 仍然开放。相邻且尚未完成的边界按以下顺序推进：

1. 在每个需要产品资格的平台分别运行真实 writer 的 main/iframe/Worker 矩阵；当前只完成本机，不把它外推为跨平台资格，也不把 terminal return boundary 称为 OS thread join。
2. 在 Linux release 平台分别运行 `tools/unblocked/cdp_capacity.py`、`tools/unblocked/cdp_ws_capacity.py`、`tools/unblocked/cdp_ws_handoff_capacity.py`、`tools/unblocked/cdp_silent_pending.py`、`tools/unblocked/cdp_multi_worker.py` 和 `tools/unblocked/cdp_multi_worker_lifecycle.py`，保留完整原始目录；当前只有 Darwin 证据，不能复制结论。再推进 Windows console/process-handle lifecycle、container network namespace，以及 contenteditable、IME/composition、grapheme/word 编辑和复杂默认动作等剩余 input qualification；自动 restart/session migration 和共享 multi-worker storage ownership 仍是显式未资格边界。不要把本机 kernel queue、Mio 控制流、逻辑数量、CPU/FD 事实或 RSS 采样外推为总容量/总资源上限。

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

- `/private/tmp/ob021-maxlength-committed-final.ITi5Kb/`
- `/private/tmp/ob021-maxlength-playwright-committed.8DcmOY/`
- `/private/tmp/ob021-maxlength-full-nextest-final-2.log`
- `/private/tmp/ob021-maxlength-full-nextest.log`
- `/private/tmp/ob021-maxlength-full-nextest-rerun.log`
- `/private/tmp/ob021-maxlength-full-nextest-final.log`
- `/private/tmp/ob021-maxlength-mcp-rerun.log`
- `/private/tmp/ob021-maxlength-mcp-rerun-2.log`
- `/private/tmp/ob021-maxlength-mcp-rerun-3.log`
- `/private/tmp/ob021-maxlength-build-no-render-committed-final.log`
- `/private/tmp/ob021-maxlength-build-render-committed-final.log`
- `/private/tmp/ob021-maxlength-obstacle-committed-final.log`
- `/private/tmp/ob021-maxlength-final-v2.hCPTL4/`
- `/private/tmp/ob021-maxlength-final-v3.ItGthv/`
- `/private/tmp/ob034-input-maxlength-chrome.FBGL1c/`
- `/private/tmp/ob034-input-maxlength-chrome.VXPwD0/`
- `/private/tmp/ob034-input-maxlength-chrome.kYCiDP/`
- `/private/tmp/ob034-input-maxlength-chrome.VqFLGK/`
- `/private/tmp/ob034-input-maxlength-chrome.Fh6qry/`（缺少 `PYTHONPATH` 的失败证据）
- `/private/tmp/ob034-input-maxlength-chrome.qbPFsF/`（缺少 bundled Chromium 的失败证据）

- `/private/tmp/ob034-capacity-final-reviewed.IlqITE/evidence/`
- `/private/tmp/ob034-capacity-close.SfPdJq/evidence/`（要求 server Close echo 的被拒绝门禁，完整失败证据）
- `/private/tmp/ob034-capacity-live-final-reviewed.log`
- `/private/tmp/ob034-capacity-tools-unittest-review-final.log`
- `/private/tmp/ob034-capacity-unit-review-fixes-4.log`
- `/private/tmp/ob034-multiworker-final.adE5nF/evidence/`
- `/private/tmp/ob034-multiworker-playwright.QU2VUu/`
- `/private/tmp/ob034-multiworker-benchmark.UKr86Z/repo/`
- `/private/tmp/ob034-multiworker-focused-no-render.log`
- `/private/tmp/ob034-multiworker-cli-no-render-nextest.log`
- `/private/tmp/ob034-multiworker-full-nextest.log`
- `/private/tmp/ob034-multiworker-build-no-render-final.log`
- `/private/tmp/ob034-multiworker-build-render-final.log`
- `/private/tmp/ob034-multiworker-obstacle.log`
- `/private/tmp/ob034-multiworker-tools-unittest.log`
- `/private/tmp/ob034-lifecycle-final.aFv2t0/evidence/`（review fixes 后、implementation commit 前的完整原始成功证据）
- `/private/tmp/ob034-lifecycle-committed.cBlBR7/evidence/`（最终 committed binary 证据）
- `/private/tmp/ob034-lifecycle-playwright.0IBf1Z/`
- `/tmp/ob021-fetch-stream-net-focused.log`
- `/tmp/ob021-fetch-stream-cdp-focused.log`
- `/tmp/ob021-fetch-stream-cdp-focused-rerun.log`
- `/tmp/ob021-fetch-stream-js-focused.log`
- `/tmp/ob021-fetch-stream-review-fixes.log`
- `/tmp/ob021-fetch-stream-review-fixes-rerun.log`
- `/tmp/ob021-fetch-stream-cdp-full.log`
- `/tmp/ob021-fetch-stream-cdp-full-rerun.log`
- `/tmp/ob021-fetch-stream-workspace-full.log`
- `/tmp/ob021-fetch-stream-build-no-render.log`
- `/tmp/ob021-fetch-stream-build-render.log`
- `/tmp/ob021-fetch-stream-build-render-final.log`
- `/tmp/ob021-fetch-stream-obstacle.log`
- `/tmp/chrome152_fetch_stream_probe.py`
- `/tmp/chrome152_fetch_stream_probe.jsonl`
- `/tmp/chrome152_fetch_stream_probe.jsonl.failed`
- `/tmp/chrome152_fetch_stream_matrix.jsonl`
- `/tmp/ob021-fetch-stream-playwright.tkwjmf/`
- `/tmp/ob021-request-body-focused.log`
- `/tmp/ob021-request-body-focused-expanded.log`
- `/tmp/ob021-request-body-astra-fixes.log`
- `/tmp/ob021-request-body-astra-cdp-fixes.log`
- `/tmp/ob021-request-body-full-nextest.log`
- `/tmp/ob021-request-body-build-no-render.log`
- `/tmp/ob021-request-body-build-render.log`
- `/tmp/ob021-request-body-obstacle.log`
- `/tmp/ob021-request-body-chrome-complete.json`
- `/tmp/ob021-request-body-chrome-protocol.log`
- `/tmp/ob021-request-body-obscura-final.json`
- `/tmp/ob021-request-body-obscura-final-protocol.log`
- `/tmp/ob021-request-body-playwright.cpLudh/`
- `/tmp/ob021-network-subscription-chrome-complete.json`
- `/tmp/ob021-network-subscription-chrome-body-scope.json`
- `/tmp/ob021-network-subscription-chrome-late-enable.json`
- `/tmp/ob021-fetch-network-ownership-chrome.json`
- `/tmp/ob021-network-subscription-obscura-final.json`
- `/tmp/ob021-network-session-playwright-smoke-success.json`
- `/tmp/ob021-network-session-obscura-cdp-final-rerun.log`
- `/tmp/ob021-network-session-full-nextest-rerun.log`
- `/tmp/ob021-network-session-cdp-no-render.log`
- `/tmp/ob021-network-session-no-render-build.log`
- `/tmp/ob021-network-session-render-build.log`
- `/tmp/ob021-network-session-obstacle.log`
- `/tmp/ob021-callback-registry-focused-review-fix.log`
- `/tmp/ob021-callback-registry-obscura-net-review-fix.log`
- `/tmp/ob021-callback-registry-dependent-focused.log`
- `/tmp/ob021-callback-registry-worker-no-render.log`
- `/tmp/ob021-callback-registry-full-nextest.log`
- `/tmp/ob021-callback-registry-no-render-build.log`
- `/tmp/ob021-callback-registry-render-build.log`
- `/tmp/ob021-callback-registry-obstacle.log`
- `/tmp/ob021-network-observation-full-nextest.log`
- `/tmp/ob021-network-observation-build-no-render.log`
- `/tmp/ob021-network-observation-build-render.log`
- `/tmp/ob021-network-observation-obstacle.log`
- `/tmp/ob021-network-observation-playwright.xICldu/`
- `/tmp/ob021-network-history-review-fixes.log`
- `/tmp/ob021-network-history-full-nextest.log`
- `/tmp/ob021-network-history-mcp-evaluate-rerun.log`
- `/tmp/ob021-network-history-full-nextest-rerun.log`
- `/tmp/ob021-network-history-build-no-render.log`
- `/tmp/ob021-network-history-build-render.log`
- `/tmp/ob021-network-history-obstacle.log`
- `/tmp/ob021-network-history-playwright.1FYhiN/`
- `/tmp/ob021-native-start-review-fixes-focused.log`
- `/tmp/ob021-native-start-render-close-rerun.log`
- `/tmp/ob021-native-start-affected-crates-final.log`
- `/tmp/ob021-native-start-race-fix-focused.log`
- `/tmp/ob021-native-start-affected-crates-race-final.log`
- `/tmp/ob021-native-start-full-workspace-race-final.log`
- `/tmp/ob021-native-start-build-no-render.log`
- `/tmp/ob021-native-start-build-render.log`
- `/tmp/ob021-native-start-obstacle.log`
- `/tmp/ob021-native-start-obstacle-final.log`
- `/tmp/ob021-network-enable-chrome-probe-v1.json`
- `/tmp/ob021-network-enable-chrome-probe-v2.json`
- `/tmp/ob021-native-start-playwright.TAcvvu/`
- `/tmp/ob034-server-terminal-accept-latency.json`
- `/tmp/ob034-server-terminal-accept-latency-final.json`
- `/tmp/ob034-server-terminal-cdp-no-render.log`
- `/tmp/ob034-server-terminal-final-focused-render.log`
- `/tmp/ob034-server-terminal-final3-cdp-render.log`
- `/tmp/ob034-server-terminal-final2-cdp-no-render.log`
- `/tmp/ob034-server-terminal-final-workspace-render.log`
- `/tmp/ob034-server-terminal-final2-workspace-render.log`
- `/tmp/ob034-server-terminal-final2-cdp-render.log`
- `/tmp/ob034-server-terminal-burst-regression.log`
- `/tmp/ob034-server-terminal-final-build-no-render.log`
- `/tmp/ob034-server-terminal-final-build-render.log`
- `/tmp/ob034-server-terminal-final-obstacle.log`
- `/tmp/ob034-server-terminal-final-playwright.HoqYVK/`
- `/tmp/ob034-inbound-benchmark.cEiFsw`
- `/tmp/ob034-disconnect-matrix-focused.log`
- `/tmp/ob034-disconnect-matrix-focused-latest.log`
- `/tmp/ob034-disconnect-matrix-focused-no-render.log`
- `/tmp/ob034-disconnect-review-fixes-render.log`
- `/tmp/ob034-worker-broad-render.log`
- `/tmp/ob034-worker-broad-no-render.log`
- `/tmp/ob034-disconnect-full-workspace.log`
- `/tmp/ob034-disconnect-full-workspace-rerun.log`
- `/tmp/ob034-disconnect-full-workspace-final.log`
- `/tmp/ob034-mcp-wait-for-selector-replay.log`
- `/tmp/ob034-mcp-wait-clears-refs-replay.log`
- `/tmp/ob034-disconnect-build-no-render.log`
- `/tmp/ob034-disconnect-build-render.log`
- `/tmp/ob034-disconnect-render-rebuild-after-clean.log`
- `/tmp/ob034-disconnect-obstacle.log`
- `/tmp/ob034-disconnect-playwright.6n2Ie6/`
- `/tmp/ob034-wire-focused-render.log`
- `/tmp/ob034-wire-focused-render-rerun.log`
- `/tmp/ob034-wire-focused-no-render.log`
- `/tmp/ob034-wire-cdp-render.log`
- `/tmp/ob034-wire-cdp-no-render-valid.log`
- `/tmp/ob034-wire-workspace-render.log`
- `/tmp/ob034-wire-build-no-render.log`
- `/tmp/ob034-wire-build-render.log`
- `/tmp/ob034-wire-obstacle.log`
- `/tmp/ob034-wire-accept-latency.json`
- `/tmp/ob034-wire-playwright.pZWZ7W/`

这些路径属于原始运行主机的临时文件，不是仓库内的持久接口。可持续引用的结论和能力边界以本页、[SUMMARY](SUMMARY.md) 及对应提交中的测试为准。处理这些日志时保留原始字段和完整内容。

## 建议使用的 skills

- `implement`：执行下一段边界明确的 TODO。
- `diagnosing-bugs`：排查无法稳定复现或对时序敏感的 capacity/connection 问题。
- `code-review`：在发布下一段改动前执行 Standards 和 Spec 双轴审核。
- `handoff`：下一次验证并提交后需要切换会话时更新本页和临时交接。

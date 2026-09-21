# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`87b577e361e3cba343a4face5f499c769ea28ee4`，`Bound scripted Network observations`；其前置实现为 `f98a577d36e2dde5982ed8b8bdecc26e82e84f6a`，`Route Network events per session`。
- 写入本交接前，最近实现提交尚待与本交接一起推送到 `origin/main`；完成后须再次核对 `HEAD`、`main`、`origin/main` 与远端 ref 对齐。
- 继续使用现有工作树 `/Users/gster1981/work/obscura`，分支为 `codex/goal`。
- 相关实现入口：[`network_observation.rs`](../crates/obscura-js/src/network_observation.rs) 的 Page-wide admission/reservation、[`ops.rs`](../crates/obscura-js/src/ops.rs) 与 [`worker.rs`](../crates/obscura-js/src/worker.rs) 的生产和 teardown、[`page.rs`](../crates/obscura-browser/src/page.rs) 的 accepted-prefix drain，以及 [`server.rs`](../crates/obscura-cdp/src/server.rs) 和 [`lib.rs`](../crates/obscura-mcp/src/lib.rs) 的 terminal 投影。

## 最近完成的阶段

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

1. 继续请求正文保留，并明确 binary、重复字段、重定向与失败路径的精确归属和预算语义。
2. 建立导航/多页面持久 observation history；当前 active Page 内存历史不能作为 append-only 或崩溃恢复证据。
3. 统一普通非拦截资源 start emission 与 Page body budget/Chrome per-agent 参数契约。
4. 补齐 Fetch stream 多消费者语义，再按实际风险推进其余 input qualification、disconnect 矩阵和 idle Worker cancellation wake。

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
- `/tmp/ob034-inbound-benchmark.cEiFsw`

这些路径属于原始运行主机的临时文件，不是仓库内的持久接口。可持续引用的结论和能力边界以本页、[SUMMARY](SUMMARY.md) 及对应提交中的测试为准。处理这些日志时保留原始字段和完整内容。

## 建议使用的 skills

- `implement`：执行下一段边界明确的 TODO。
- `diagnosing-bugs`：排查无法稳定复现或对时序敏感的 capacity/connection 问题。
- `code-review`：在发布下一段改动前执行 Standards 和 Spec 双轴审核。
- `handoff`：下一次验证并提交后需要切换会话时更新本页和临时交接。

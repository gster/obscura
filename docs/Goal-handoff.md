# 当前 TODO goal 交接

本页是跨会话继续执行当前 goal 的交接入口，不替代 [TODO](TODO.md) 的任务状态，也不扩展 [SUMMARY](SUMMARY.md) 已确认的能力边界。继续工作时先读取这两份文档和当前源码，避免把阶段性实现当作整个资源治理目标已经完成。

## 当前基线

- 当前 goal：继续执行 [`docs/TODO.md`](TODO.md)，整体 goal 仍然有效，尚未完成。
- 最近完成的实现提交：`875f16692ca95faf64d4498669b8a7ac94a75eef`，`Cancel active CDP execution on server termination`；其实现基线为 `757e8a0ab636bd51b0f43b7c4d85e4c0d666b947`。
- 该实现提交已推送到 `origin/main`；本交接更新提交完成后须再次核对 `HEAD`、`main`、`origin/main` 与远端 ref 对齐。
- 继续使用现有工作树 `/Users/gster1981/work/obscura`，分支为 `codex/goal`。
- 相关实现入口：[`network_history.rs`](../crates/obscura-browser/src/network_history.rs) 的 context-owned journal、[`context.rs`](../crates/obscura-browser/src/context.rs) 与 [`page.rs`](../crates/obscura-browser/src/page.rs) 的 ownership/producer 接入、[`ops.rs`](../crates/obscura-js/src/ops.rs) 与 [`worker.rs`](../crates/obscura-js/src/worker.rs) 的 scripted/Worker barrier，以及 [`obscura.rs`](../crates/obscura-cdp/src/domains/obscura.rs)、[`dispatch.rs`](../crates/obscura-cdp/src/dispatch.rs) 和 [`lib.rs`](../crates/obscura-mcp/src/lib.rs) 的恢复、读取与投影。

## 最近完成的阶段

OB-034 的 server-side terminal-source 切片现把 writer I/O failure、writer timeout、server shutdown 与 outer I/O task cancellation 连接到同一 connection-local sticky cancellation。生产 writer helper 的两项回归使用受控 `Sink` 和 active standalone V8，证明失败后 outbound 关闭、同步无限执行被终止且 reservation 释放；这不是实际 TCP writer 故障注入。outer I/O task cancellation 与 server shutdown 使用完整 connection 路径，证明 active main-realm V8 被终止、socket 关闭、`max_connections` slot 回收；尚未扩展到 iframe/Worker 的 server-side source 矩阵。

server shutdown 由 sticky watch 发布，并以 Mio OS waker 立即唤醒 accept Poll，覆盖 cancel-before-register 和 idle accept。listener readiness 保持事件驱动；单轮最多 accept 256 个，但只在观察到 `WouldBlock` 后重新阻塞 Poll，批次间检查 shutdown 并处理已接收请求。同步 HTTP discovery 设置 1 秒 socket 读写超时，拒绝路径设置 100ms socket 读写超时。最初 10ms polling 版本使 200 轮交替 `/json/version` median 相对基线回退到 5.07 倍，已弃用；最终版本对 `757e8a0` 的 median 为 1.763ms 对 1.994ms（0.884 倍），p95 为 2.050ms 对 2.251ms。Astra light 首审指出 batch 未 drain 到 `WouldBlock` 就重新 Poll 的风险，修复后复审 0 findings。一次 320 并发 discovery 测试与既有 256 silent-pending 上限耦合而产生空响应，已删除该不可靠测试，不据此宣称 backlog burst 资格。OB-034 继续开放。

OB-034 的直接客户端断连矩阵现在覆盖 WebSocket Close、原始 FIN、linger-zero RST x main、iframe、active Dedicated Worker 的 3 x 3 组合。main/iframe 以第一轮同步循环 console marker、Worker 以第一轮 `postMessage("started")` 证明执行已进入对应 realm；Close 保留并排空旧 socket，FIN 保留读半边，避免测试输入被客户端 drop 改写。每项都在 `max_connections=1` 下要求旧 slot 释放并由新连接执行 `42`。

connection cancellation 同时通过 sticky watch 主动唤醒仅等待 `commands.recv()` 的 idle Worker，以及停在远期 timer/I/O autonomous future 的 Worker。owner 与 Worker object 保持存活的精确测试先观察真实 wait-state，再要求 lease 在一秒内归零。Astra light 首审发现非 idle biased select 会饥饿 autonomous progress，以及 Close 用例可能退化成其他断连输入；两项修复后终审为 0 blocker、0 major、0 minor。当前未完成边界以上述 server-side source 的真实 TCP 与 iframe/Worker 资格限制为准。

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

1. 为真实 TCP writer failure/timeout 建立可控故障注入，并把 server shutdown、outer I/O task cancellation 扩展到 active iframe/Worker；继续证明 slot、queued command 与 Worker 清理边界，不把受控 `Sink` 单测称为 wire 资格。
2. 为 Mio batch-limit/backlog 建立不与 256 silent-pending 上限耦合的确定性回归，再推进 admission 前 TCP/kernel/container capacity 与剩余 input qualification；不要把三层逻辑 payload 预算外推为总 RSS 上限。

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

这些路径属于原始运行主机的临时文件，不是仓库内的持久接口。可持续引用的结论和能力边界以本页、[SUMMARY](SUMMARY.md) 及对应提交中的测试为准。处理这些日志时保留原始字段和完整内容。

## 建议使用的 skills

- `implement`：执行下一段边界明确的 TODO。
- `diagnosing-bugs`：排查无法稳定复现或对时序敏感的 capacity/connection 问题。
- `code-review`：在发布下一段改动前执行 Standards 和 Spec 双轴审核。
- `handoff`：下一次验证并提交后需要切换会话时更新本页和临时交接。

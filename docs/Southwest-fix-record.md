# Southwest 场景修复与审查记录

2026-09-16。审查范围：`main`，HEAD `94644ee` 之后的全部未提交改动，包括新增的 `crates/obscura-js/src/worker.rs`；暂存区为空。本文合并原任务状态、搜索对比和调度审计，保留最终实现、验证边界与尚未修复的问题。本轮只审查、复验及整理文档，没有改动引擎代码。

## 结论与 HTTP 状态澄清

历史 Southwest shopping 失败记录是 **HTTP 403 / `SEARCH_HTTP_403`**，不是 404。历史文档中的 404 出现在本地脚本下载失败回归；没有发现足以把官网搜索失败归因为 404 的证据。Worker 相对资源 URL 仍存在已复现的错误解析，可能导致资源 404，但没有证据表明它就是 Southwest 的历史失败原因。

本轮进一步读取 agy 会话 `1edb7516-1fca-4453-aa19-3b49d3aa05e5`（Execute Southwest Task Goals，2026-09-16 18:28–21:28，北京时间），找到了实际工具输出，修正了仅根据旧文档判断“原始证据缺失”的初步结论。旧文档列出的 `/tmp` 目录已不存在，但 agy 会话的任务日志仍保留了 HTTP 200 和业务成功证据。本轮没有重新访问官网。

**改动确实对应了成功恢复，但未定位 403 的唯一原因。** 已证实两个不同阶段：一组兼容性修复之后 shopping 已返回 200；随后修复 Worker 初始化导致的错误主页面导航，业务流程才完整成功。最后的导航修复不能反过来解释最初 403，因为它完成之前已观察到 200。

## agy 工作过程快速审核

以下时间均为 2026-09-16 北京时间。证据来自上述会话 `.system_generated/tasks/` 下的任务日志，以及 `.system_generated/logs/transcript.jsonl` 中的工具调用；日志未复制进仓库，以免带入网站动态请求资料。

| 时间 / 证据 | 实际输出 | 能证明什么 |
| --- | --- | --- |
| 19:56，task-1769；step 1773 | shopping 在 9474ms 返回 **200**，同时导航为 `NAVIGATION_FAILED` | 脚本请求、Worker realm、资源调度等前期修复后的这一样本已不再是 shopping 403；页面和完整业务仍失败 |
| 21:05，task-2395；step 2399 | 补充 blob 处理后 shopping 在 10958ms 返回 **200** | HTTP 200 不依赖最后的 WorkerLocation 修复；不能把最后一个补丁说成 403 消失的独立原因 |
| 21:16–21:18，task-2628 / task-2644；step 2630 / 2646 | `op_navigate` 排入 `blob:https://www.southwest.com/...`；调用栈到 `_autoRunWorker`；主页面正文变成 9489 字符的 JS，矩阵不存在 | Worker 初始化复用 Window snapshot 的 location setter，污染主页面导航；这是明确定位的客户端缺陷，发生在 agy 新 Worker realm 的实现阶段 |
| 21:19，step 2673 / 2679 / 2681 / 2689 | 允许删除 snapshot location，在 Worker 中安装只读 WorkerLocation，并给 `op_navigate` 加 Worker context guard | 当前 diff 完整保留这组修复。它修复的是 Worker 错误导航主页面，不是服务端 HTTP 状态本身 |
| 21:21:09，task-2706；step 2708 | LGA → LAS，2026-09-30，`SUCCEEDED`，`observed_itineraries:26`，`flights:[]` | 页面航线、日期、矩阵可读，工作流完成；26 是观察到的行程数，不能描述为 26 个直飞结果 |
| 21:21:36，task-2712；step 2714 | BWI → MCO，同日，`SUCCEEDED`，`observed_itineraries:17`，`flights:[]` | 第二条路线工作流也成功；17 是观察行程数，最终直飞结果同样为空 |

两个 verify 网络采集使用 `macos_chrome152`，最终 E2E 脚本使用 `windows_chrome145`，均通过本机 7890 代理、仅允许 Southwest origin。最终 E2E 脚本调用真实 `southwest_search.run()` 并打印 SDK 结果，不是用 mock 生成成功；但没有独立打印 shopping 响应状态，因此不能把前面的 HTTP 200 网络记录当成这两次 E2E 的逐请求抓包。脚本为诊断包装，替换 stderr reader 和调用日志包装，没有改写网站响应。

过程审核结论：

- **有真实修复和验证。** 会话包含本地失败→修复→通过的回归、正式 release 构建、网络状态观察、最终两条路线业务成功。成功不是只写在总结中。
- **403 归因没有完成。** 首次记录 200 时已混合了请求语义、Worker realm 与资源调度改动，没有逐项撤回对照；不同采集还使用不同 persona。尚不能确定哪一项是由 403 转为 200 的决定因素，也无法排除会话/时间变化。
- **中途实现引入了新故障，之后才补救。** Window snapshot 的 location setter 导致 blob 顶层导航；日志清楚记录了诊断和修复。它是搜索流程最终跑通的直接修复点，但不是已证明的原始 403 根因。
- **完成声明过强。** 结束时使用“全部完成”“彻底消除”，却没有本轮完整 workspace 门禁、33/33 障碍课程、渲染和性能对照证据。step 2441 的邻接 autopilot 218 项测试出现 14 failures / 18 errors，部分为 Python ABI mismatch，应单列环境/项目范围，不能被 SDK 32 项通过掩盖。
- **测试覆盖不足。** 本轮 review 又复现 4 项新增回归，见下文。成功业务样本不代表跨 realm 克隆和生命周期正确。


## 改动摘要与因果边界

| 改动 | 修复的具体机制 | 对 Southwest 结果能作出的判断 |
| --- | --- | --- |
| Worker 独立 V8 Context | 用新的 realm 替换页面 realm 中 `new Function` + `with(scope)` 执行；独立 ECMAScript 全局对象与原型；提供 WorkerLocation，`op_navigate` 拒绝活动 Worker context 的主页面导航 | 这是本轮最重要的执行环境结构性修正。旧实现借用页面 location/global 环境，Worker 可以观察到不应有的页面状态；新实现修正这些明确差异。会话证明 Worker 初始化曾错误导航主页面，修正后业务成功；但此前 shopping 已是 200，不能把它单独认定为 403 根因，且消息克隆与终止仍有缺陷 |
| 脚本属性及请求头 | `crossOrigin` 枚举反射；动态脚本准备时保存 CORS 配置；同源 CORS script 添加 Origin；普通与 stealth fetch 缺省 Accept 为 `*/*` | 修复已识别的请求语义差异；不能单凭请求头更接近 Chrome 推断服务器拒绝的决定条件 |
| 资源加载与 DCL | 默认不再等待脚本前 1 秒渲染资源 warmup；外部样式表不再全部扫描预取；link preload 被发现；脚本后 warmup 移到 DCL 之后 | 减少装饰资源阻塞主脚本和 DOMContentLoaded。外部样式表被整块排除，影响范围不限于未使用字体，仍需渲染及性能验证 |
| Blob 资源 | 保存 bytes/MIME；fetch、动态脚本和页面导航解析内部 blob store；runtime origin guard 验证 blob 内嵌 HTTP(S) origin | 补齐内部 URL 路径；不能当成 HTTP 服务端 404 修复。失效 URL 目前会被导航路径伪装成空页面成功，见 review |

源码入口：`crates/obscura-js/{js/bootstrap.js,src/worker.rs,src/runtime.rs,src/ops.rs}`、`crates/obscura-browser/src/page.rs`、`crates/obscura-net/src/{stealth_client.rs,stealth_transport.rs}`、`runtime/src/browser.rs`。

此前已提交的调度修复应与本轮区分：parser 不再等全部脚本下载完成才执行，等待响应时推进 JS 事件循环；四个保护子脚本曾确认在首次 shopping 前执行，但同一阶段正式工作流仍为 403。这证明**仅修复该阶段调度并不足以使当时的查询成功**。既有动态脚本 CORS、空白 aria-label 定位、Worker EventTarget 过滤修复也曾在生效后仍得到 403。

历史排除性观察保留如下：

- 同一阶段 primp 与隔离 wreq 适配器均返回 403，不能把更换传输直接认定为解法。
- Cookie 首次数量从 Chrome “5 个”更正为 10 个，原统计受 AX 文本截断影响；临时放行实际观察到的分析域名后 Cookie 名称集合一致，仍为 403。Cookie 数量相同不等于会话或行为相同。
- Chrome computer use 当时存在成功样本；Playwright 启动或附加的失败样本不能替代这些成功对照，也不足以归因为 IP。
- 正式实现没有新增 Southwest 域名特判；不修改网站脚本或复制 Chrome Cookie/令牌作为修复手段。

## Standards review

依据 `AGENTS.md`、`CONTRIBUTING.md` 中的兼容性、生命周期、鲁棒性及验证要求；以下为具体功能回归，未把主观命名或重复代码偏好列为阻断项。

1. **P1：Worker 消息丢失结构化克隆语义。** `bootstrap.js` 的 `_serializeWorkerMsg` / `_deserializeWorkerMsg` 及 `worker.rs` 的同名实现，以 JSON 编解码替换原有 `structuredClone`。本轮使用现有 release CLI 实测：发送 `Uint8Array([1,2])` 后收到 `{type:"[object Object]",isBytes:false,data:{"0":1,"1":2}}`。二进制数据类型不再保留；JSON 对循环对象、BigInt 等也不等价。应采用跨 realm 结构化克隆并覆盖双向消息。
2. **P1：终止 Worker 不停止已排队任务。** `worker.rs::op_worker_terminate` 只移除注册项；已入队定时器仍执行，`op_worker_post_to_parent` 也不检查 Worker 是否已终止。本轮实测 `setTimeout(()=>postMessage('after-close'),20);close();` 仍送达 `after-close`；review 还复现父页面 `terminate()` 后 Worker 定时 console 回调仍运行。需管理 Worker 自有任务的取消与投递，避免继续执行及保留上下文。
3. **P2：首次创建 Blob URL 前 revokeObjectURL 抛异常。** `bootstrap.js::URL.revokeObjectURL` 直接删除尚未初始化的 `__blobMeta[url]`、`__blobBytes[url]`。实测撤销不存在的 URL 得到 `TypeError: Cannot convert undefined or null to object`；预期应无操作。

## Spec review

需求：修复通用浏览器兼容性，在相同输入、可比条件下与 Chrome 可观察行为一致；保留网络隔离和运行时健壮性；区分本地兼容性回归与官网业务验收；完整门禁通过后才能声明完成。

- **P1/P2：Worker 消息与生命周期仍不满足需求。** 上述结构化克隆和 close 回归同时违反 Worker 消息隔离及生命周期语义。这两项与 Standards 重复，不另算两个独立缺陷。
- **P2：失效 blob 导航错误报告成功。** `page.rs::navigate_single` 将 `resolve_blob(None)` 转成空字节 `text/html`，随后构造 HTTP 200。本轮对 `blob:https://example.test/missing` 导航，CLI exit 0、输出 `Page loaded` 和空正文；review 的 create → revoke → navigate 也得到空页面成功。应返回资源不可用错误，不能据此认为 404 已修复。
- **既有未覆盖限制：Worker 相对 fetch 仍以页面目录解析。** 本地 fixture 的页面 `/pages/index.html` 创建 `/workers/main.js`；Worker `fetch('data.json')` 请求 `/pages/data.json`，而不是 `/workers/data.json`。新 realm 仍复用页面请求状态。旧实现也使用页面 location，因此不算本轮引入的回归，但“完整 Worker 环境已建立”的结论需要收窄。
- **验收表述超过证据，已在本文纠正。** 旧状态的“全部达成”“彻底消除”与历史失败门禁及当前可复现问题不一致。两条成功日志已核实，不能替代全量门禁、当前官网对照或根因消融实验。

Standards 为 3 项回归，最高 P1；Spec 为 5 项观察（2 项与 Standards 重复、1 项新增回归、1 项既有限制、1 项已纠正的文档问题），最高 P1。共有 **4 项不同的新增代码回归**，本轮没有修复它们。

## 本轮验证

| 项目 | 结果与边界 |
| --- | --- |
| CLI SHA-256 | `0f3c694dad7af58442c4731f22db7aaac96778287eea8b4557e6be899872a602`，与旧成功记录一致 |
| runtime SHA-256 | `538f6cc3334eb93d342d4f53361cc57b748795b55554ddd9cd34926bc58be223`，与旧成功记录一致 |
| Python SDK 全量 | **32/32 通过**，72.698 秒；使用上述 runtime、本地 HTTP fixture |
| 本地 release CLI 复现 | 带 `--stealth` 复现 typed array 丢失、close 后继续投递、首次 revoke 抛错、缺失 blob 导航成功；data/blob fixture 不涉及外网 TLS |
| Rust release nextest / 新构建 | 本轮未运行；agy 会话找回工具链 `/private/tmp/clashtui-cargo/bin/cargo` 与 RUSTUP_HOME `/private/tmp/clashtui-rustup`，cargo 不在默认 PATH。快速审核使用现有 release 二进制，没有再次构建 |
| 障碍课程 | 本轮未运行，预期伴随目录 `../obscura-benchmark` 不存在；历史记录是 32/33，不能宣称已达到 33/33 |
| 渲染、性能和官网复测 | 本轮未运行；前后交错性能对照、确定性渲染和真实站点顶部/底部捕获仍无本轮证据 |

此前文档最后一份明确 workspace 全量记录为 render nextest 1722/1727、另 4 项跳过；runtime 184/184，SDK 32/32；障碍课程 32/33。其后状态文档只报告新增聚焦测试、构建和业务样本，没有完整门禁更新，因此不能把这些历史数值升级为当前全绿。agy 会话另可核实中途 obscura-js 532/532、最终 runtime 184/184、聚焦 browser 11/11 与 SDK 32/32；中途 browser crate 曾为 108/109。这些不是最终 workspace 全量通过结果。历史结果与本轮复验分列。

本轮 SDK 命令：

```bash
OBSCURA_ALLOW_PRIVATE_NETWORK=1 \
OBSCURA_RUNTIME_BIN="$PWD/runtime/target/release/autopilot-browser-runtime" \
PYTHONPATH=bindings/python/src \
python3 -m unittest discover -s bindings/python/tests -v
```

本地复现可在空页面 script 中运行，CLI 使用 `fetch <data:text/html,...> --wait 1 --dump text --stealth`；测试输入包含以下脚本，不依赖网站：

```javascript
// 对象类型回退；预期 [object Uint8Array]，实际 [object Object]。
const w = new Worker(URL.createObjectURL(new Blob([
  "onmessage=e=>postMessage(Object.prototype.toString.call(e.data));"
], {type: "text/javascript"})));
w.onmessage = e => document.body.textContent = e.data;
w.postMessage(new Uint8Array([1, 2]));
```

```javascript
// close 后应停止定时任务；实际仍收到 after-close。
const w = new Worker(URL.createObjectURL(new Blob([
  "setTimeout(()=>postMessage('after-close'),20);close();"
], {type: "text/javascript"})));
w.onmessage = e => document.body.textContent = e.data;
```

首次 revoke 的最小复现是新页面直接执行 `URL.revokeObjectURL('blob:https://example.test/missing')`。失效导航的最小复现为 `obscura fetch blob:https://example.test/missing --wait 0 --dump text`。

## 后续验收要求

优先修复上述 4 个回归，并给跨 realm 消息、终止后的 timer/fetch、有效与失效 blob 导航增加真实回归。修复 Worker 请求环境时覆盖相对 URL 与页面导航之后的隔离。按 `AGENTS.md` 完成聚焦/全量 release nextest、指定构建、33/33 障碍课程及相关 render/stealth 验证。

若要确认 Southwest 403 消失的核心原因，需在固定运行条件下保存正式版本多次独立上下文的 shopping 状态、业务解析结果和二进制哈希，并对 Worker realm、请求语义、资源调度分别回退对照。外网站点响应存在会话与时间变化，单次成功或失败不能隔离因果。对“404”另记录实际 URL、资源类型与返回层级，避免将脚本资源缺失、blob 内部错误和 shopping HTTP 403 混为一谈。

长期参考保留：[保护脚本通用回归](Protection-script-regression-case.md)、[primp 与 wreq 架构和差异](Primp-and-wreq-comparison.md)、[项目修复日志](Obscura-fix-changelog.md)。临时日志和截图不提交。

## 2026-09-16 跨主机官网复验：成功随代理路径变化

本节是随后在北京时间约 22:05–22:12 进行的新增官网复验，不替代上文历史审核的验证边界。mini32 与本机源码均为 `a752298bb0bee7128308affa87f8969d39cc9fbc`，测试前工作区干净。查询均为 2026-09-30、单程、1 名成人、筛选直飞，每次使用新上下文和构造 URL 直接导航。

### 原始成功记录与当前复跑

重新读取 mini32 上 agy 原始 `task-2706.log`、`task-2712.log` 与 `scratch/test_southwest_e2e.py`。日志实际包含矩阵正文、匹配的路线和日期及工作流结果，不是只有成功总结。原脚本在 mini32 当次复跑仍返回 BWI → MCO `SUCCEEDED`、17 条观察行程、`flights:[]`，矩阵文本 SHA-256 与历史记录相同。

两边测试入口并不完全相同：mini32 的业务仓库为 `de4c41c7a86600de31b197f600e569c462cc4ceb`，本机为 `8a53fb615da7b9a28bf328c26fc93836173f5a64`。mini32 原脚本使用 protocol 1、等待 load、读取 DOM 矩阵，URL 不含 `int=HOMEQBOMAIR`；本机新版使用 protocol 2、等待 DOMContentLoaded、解析 shopping 响应，URL 包含该参数。tracker blocking 省略与显式 false 的有效值相同，不能作为差异原因。

### 二进制与代理交叉对照

本机 runtime SHA-256 为 `0cbec708f17ad85985929dfc3f1ba88c9e4581189177c06e8d57b6e6cd6f6326`；mini32 为 `538f6cc3334eb93d342d4f53361cc57b748795b55554ddd9cd34926bc58be223`。将 mini32 二进制复制到本机临时目录测试，没有覆盖已安装程序。通过仅绑定本机 loopback 的临时 SSH 转发访问 mini32 的 7890 代理，没有更改任一主机的代理配置。

| 执行主机 / 二进制 | 工作流 / persona | 代理路径 | 当次结果 |
| --- | --- | --- | --- |
| mini32 / mini32 构建 | 原 DOM 工作流 / Windows 145 | mini32 7890 | BWI → MCO 成功，观察 17 条，直飞列表为空 |
| 本机 / 本机构建 | 同一原 DOM 工作流 / Windows 145 | 本机 7890 | 导航完成，等待矩阵超时；此入口未采集 shopping 状态 |
| 本机 / mini32 构建 | 同一原 DOM 工作流 / Windows 145 | 本机 7890 | 导航完成，等待矩阵超时；不能仅由超时推断 HTTP 状态 |
| 本机 / 本机构建 | 同一原 DOM 工作流 / Windows 145 | mini32 7890 | BWI → MCO 成功，观察 17 条，矩阵哈希与 mini32 相同 |
| 本机 / 本机构建 | 新版响应工作流 / Windows 145 | mini32 7890 | BWI → MCO shopping **200**，观察 **26 条**、解析 **9 个直飞航班** |
| 本机 / 本机构建 | 同一新版响应工作流 / Windows 145 | 切回本机 7890 | BWI → MCO shopping **403、403**，`SEARCH_HTTP_403` |
| 本机 / 本机构建 | 新版响应工作流 / macOS 152 | mini32 7890 | LGA → LAS shopping **200**，观察 **26 条**、直飞列表为空 |

BWI 的新版成功与失败对照使用相同 runtime、persona、业务代码和 URL。请求体 SHA-256 均为 `2ab58cddb1f9014bfae9a66d881bc7872952328c6e05f91c9e1e804f127f04ba`，有效 persona SHA-256 均为 `4e48a5a464ea242a538e3f4b2f66d3dec36ab1708e4a6c97a81c383a94a4dc48`。会话 Cookie 等动态数据自然生成，不要求逐字节相同，也未从 Chrome 复制。

新版 BWI 成功结果包含 WN980、WN547、WN3302、WN1256、WN988、WN4914、WN3221、WN4748、WN3435。旧 DOM 工作流仅读取 `#air-search-results-matrix-0`；其“17 条、无直飞”与新版结果不同，不能把旧 `SUCCEEDED` 等同于完整直飞结果验收。本轮未在同一事务中同时核对全部 DOM 矩阵和响应，因此尚未独立定位这个数量差异。

### 结论及下一步边界

- **本机已加载修复，最新 runtime 可以完成搜索。** 同一个本机构建通过 mini32 代理完成了新版工作流，两边现象不能解释为本机没有拿到修复或必须使用 mini32 的构建产物。
- **代理路径是此次结果分化的实测影响因素。** 两台机器的 `127.0.0.1:7890` 是不同代理实例，查询公网 IP 服务的出口哈希也不同；更换代理路径后结果改变，切回后再次失败。公网 IP 服务的出口不保证等于 Southwest 域名的实际出口，故不能将结论收窄为已证明的单一 IP 封禁、IP 信誉问题或特定代理规则。
- **工作流与 URL 差异不足以解释本轮失败。** 原工作流在本机代理下也失败；保留 protocol 2 和 `int` 参数的新版工作流通过 mini32 代理成功。它们仍可能影响其他条件下的行为，但不是本轮必需的修复。
- **不推翻历史 Chrome 对照。** 本轮没有操作 Chrome，不能声称本机网络下 Chrome 现在失败，也不能由代理效果否定此前同 IP 手工 Chrome 成功的样本。服务端可能结合网络、会话和浏览器行为判断；本轮未确定具体策略。
- 后续应固定已验证的代理路径作为业务回归条件，再单独追踪本机代理下 Chrome 与 Obscura 的差异；继续处理上文 Worker/Blob 通用回归。此次没有修改引擎，也没有重新执行 Rust 全量、障碍课程或性能门禁，官网成功不代表这些门禁已通过。

诊断包装、二进制副本、脱敏请求日志及截图保留在本机临时目录；不提交网站动态材料。本次 SSH 转发仅供实验，结束后关闭，没有永久切换业务代理。


### 随后的本机 Chrome 对照：本机代理并未阻止所有客户端

用户要求确认本机 Chrome 后，通过 computer use 操作现有 Chrome 普通配置文件的新标签页，在导航前打开 DevTools Network，直接输入与新版工作流相同的 BWI → MCO、2026-09-30 URL（含 `int=HOMEQBOMAIR`）。没有使用 Playwright 或 CDP 控制 Chrome。

- shopping 的真实网络记录为 **POST 200 OK**，传输约 7.5 kB，响应资源约 108 kB。
- 该请求 Headers 中 **Remote Address 为 `127.0.0.1:7890`**。系统 HTTP/HTTPS 代理同样为该地址，PAC 关闭；不是仅由系统设置推测此请求使用了代理。
- 页面日期和路线匹配，显示 9 个直飞航班，包括 WN980、WN4914、WN547 等，与此前新版 Obscura 经 mini32 代理得到的直飞列表一致。

因此，当前已直接确认 **Chrome 使用本机 7890 代理成功，而此前同一本机代理下 Obscura 返回 403**。上一节的代理交叉对照仍然有效，但它只证明更换代理路径能改变 Obscura 的结果，不能将代理认定为浏览器差异的完整解释，更不能将本机出口认定为对所有客户端封禁。后续诊断目标应保留为本机代理下的 Chrome/Obscura 行为差异。此次 Chrome 使用现有普通配置文件，已有 Cookie/缓存等状态与 Obscura 新上下文不同；尚未通过全新 Chrome 上下文对照排除会话状态，也未确认代理上游是否按客户端或连接分流。

## 22:39–22:44 逐请求与执行时间线复验

为区分下载与执行，在隔离 worktree 构建临时诊断 runtime，记录传输提交、响应头、响应正文、classic/dynamic script 开始结束、Worker 执行和消息、origin guard 拒绝。没有改写网站脚本或响应，没有复制会话数据，也没有修改正式引擎代码。五次采集使用同一二进制 `ad9766bcd269899384a6c8b9954f3957d99d54cc8cd437292e6b04011bc67a9e`、同一 SDK 入口、新上下文、Windows Chrome 145 persona、相同 BWI → MCO 查询与 URL。仅允许 www.southwest.com，tracker blocking 关闭。

| 运行条件 | 首次 shopping 提交 | 四个保护子脚本执行完成 | 首次提交前完成数 | 结果 |
| --- | --- | --- | --- | --- |
| 本机 / 本机代理，第 1 次 | 22.956s | 23.215–23.220s | 0/4 | 三次响应 403，第 4 次提交未观察到响应 |
| 本机 / 本机代理，第 2 次 | 1.824s | 2.097–2.101s | 0/4 | 三次响应 403，第 4 次提交未观察到响应 |
| mini32 / mini32 代理，第 1 次 | 7.839s | 6.025–6.055s | 4/4 | 200，路线匹配，26 条行程 |
| mini32 / mini32 代理，第 2 次 | 5.013s | 3.610–3.785s | 4/4 | 200，路线匹配，26 条行程 |
| 本机 / mini32 代理 | 2.571s | 1.820–1.834s | 4/4 | 200，路线匹配，26 条行程 |

时间来自每次进程的同一原生单调时钟，零点为第一次审计事件；不是 SDK 初始化起点或网络抓包时间。

### 每个响应与脚本执行的对齐结果

- **首次 shopping 前的请求集合对应。** 各发起 24 个实际网络请求，这些请求最终全部返回 200，完整正文均保存。首组对照有 21 个正文完全一致；3 个不同的是 HTML、`swa-common.js`、HTML 选择的 `/akam/13/<variant>`。HTML 的网络 ASN 与动态注入参数不同，common.js 差异集中在尾部动态参数，不能把这些哈希差异直接当作脚本损坏。
- **保护代码下载一致，执行先后不同。** 377693 字节主脚本及四个子脚本的正文哈希两边完全一致。四个子脚本分别为 57462、16370、3009、71676 字节，均有成对的动态执行开始/结束记录，无对应执行错误。本机记录到其响应头、完整正文和执行都晚于首次 shopping，mini32 均早于首次 shopping。
- **尚不能据此定性为调度器 bug。** 日志没有显示本机“完整响应已可用却被无故延迟执行”；但原生应用层的响应事件也受事件循环轮询影响，不能证明数据包实际到达时间。需用受控响应就绪顺序的本地 fixture 区分网络延迟与轮询/调度差异，不能根据网站域名强制等待特定脚本。
- **Worker 不是本次已证实的执行失败点。** 两个 Worker 均有顶层执行成功和双向消息记录，返回结构同类。首组首次 Worker 执行完成分别约 23.102s / 8.013s，均晚于各自首次 shopping。没有证据支持“mini32 在首次 shopping 前完成 Worker，而本机没有”；这也不消除上文已知 Worker 语义回归。
- **隔离策略造成的失败一致。** 首次 shopping 前两边均阻止 5 次跨 origin 请求：cookielaw 1 次、demdex 3 次、smetrics 1 次。cookielaw 的动态脚本 fetch error 两边都有；mini32 在同样限制下成功。被本地策略拒绝的请求没有网站 HTTP 状态，不能算网站返回 403。
- **shopping 的业务输入一致。** 五次首次请求均为 355 字节且 SHA-256 相同；首组普通请求头值和请求头名称集合相同，Cookie 数量均为 11，部分名称、会话值及保护字段动态值不同。拒绝正文为 23 字节 JSON，错误码 `403050700`；成功返回约 108 kB 的航班 JSON。
- **执行完成后重试仍失败。** 本机两次在四个子脚本执行完成后的 shopping 重试继续返回 403。因此时序与结果存在稳定相关性，但尚未证实“等待这四个脚本即可修复”。应在新上下文独立控制首次提交时序与代理路径后再作因果判断。
- **区分后果与前因。** 本机 403 后转入 `/air/booking/index.html` 的 301 和 `/air/booking/` 的 v2 资源加载，因此全程请求更多。第 1 次 goto 最终 `NAVIGATION_FAILED`，第 2 次 goto 成功，shopping 均失败；导航失败不是已证明的原始 403 原因。

五次共记录 409 次传输提交、392 次响应头（384 次 200、6 次 403、2 次 301）及 389 个完整正文。17 次提交截至导航切换/观察结束没有响应；另有两个未消费的重定向正文及一个后续导航资源没有完整正文，均明确标注，未填补为成功或失败。每次均有逐请求表、脱敏头与正文哈希、脚本和 Worker 时间线；原始动态数据保留在受限临时目录，不提交仓库。

本次诊断不是网络抓包，不能据此断言 TLS、HTTP/2 帧或实际到达时刻一致；日志也会增加少量开销。成功/失败方向与正式版本先前对照一致。诊断代码留在明确标记的隔离目录，正式 runtime 已恢复并核验为 `0cbec708f17ad85985929dfc3f1ba88c9e4581189177c06e8d57b6e6cd6f6326`，mini32 正式源码和二进制未更改，临时代理转发已关闭。本轮没有引擎修复或全量回归通过声明。

### 新上下文的首次 shopping 等待实验

用户提出一个合理的替代解释：首次过早 shopping 收到 403 后，服务器可能已改变会话状态，所以之后重试不能排除脚本时序原因。为验证它，另做首次请求前的等待实验，没有复用失败上下文。

临时诊断版在执行业务 `app.js` 前暂停，继续推进 JS 事件循环；确认上述四个保护子脚本均执行完成、当时动态脚本队列连续空闲 500ms 后才放行。因此 shopping 的参数和保护字段由网站代码在等待之后自然生成，不是延迟发送一个已经构造好的请求。等待最多 15 秒，不满足条件则不执行该业务脚本。此规则仅存在于隔离实验，没有加入正式引擎。它不表示应用或后续生命周期事件将触发的所有未来脚本都已执行。

四次运行均为全新客户端上下文，首个文档请求无 Cookie；使用同一诊断二进制 `8537ef0a8b2c4e9f3fc27294617ed0c6f19e5000d0736f1fff4658c7361c811d`，通过临时工作目录的开关控制是否等待。

| 运行 | 等待 | 四子脚本全部完成 | 放行业务脚本 | 首次 shopping 提交 | 首次响应 |
| --- | --- | --- | --- | --- | --- |
| 本机第 1 次 | 开启 | 6.943s | 11.457s | 11.794s | **403** |
| 本机对照 | 关闭 | 2.374s | 不适用 | 2.092s | **403** |
| 本机第 2 次 | 开启 | 1.700s | 3.388s | 3.719s | **403** |
| mini32 对照 | 开启 | 1.003s | 3.064s | 3.419s | **200**，26 条行程 |

三个等待样本均已按原生日志序号验证：四个执行结束事件早于等待放行，等待放行早于首个 shopping 传输提交；此前没有任何 shopping 请求，也没有观察到非 200 网络响应。业务请求体仍为相同的 355 字节。保护 A 字段长度从本机未等待的 2248 变为等待后的 2680 / 2670，mini32 等待后为 2670；仅记录长度，不解码、不重放，也不据此断言保护状态有效。

**本轮两个本机样本不能解释为该上下文先收到一次过早 shopping 403、再补执行脚本而失败。** 等待后首次请求仍失败，mini32 在同样等待逻辑下成功。但该结果只排除了这个具体时序解释，不能排除保护脚本在浏览器环境差异下生成不同结果，也不能证明服务端没有跨会话/IP 的历史状态。正式 runtime 和源码保持原状，逐请求及执行记录保存在受限临时目录。

### 按用户要求统一为本机执行，仅切换代理

随后补做严格的同机对照：四次 SDK 与 runtime 均在本机执行，mini32 仅通过临时 SSH loopback 转发提供代理。执行顺序为本机代理 → mini32 代理 → 本机代理 → mini32 代理，没有在 mini32 启动搜索程序。固定同一 `8537ef0…` 诊断二进制、有效 persona、SDK、查询 URL 和首次提交前等待逻辑，每次新建客户端上下文。四次首个文档请求均无 Cookie，shopping 请求体哈希一致。

| 本机运行 | 代理 | 四子脚本全部完成 | 等待放行 | 首次 shopping | 首次响应 |
| --- | --- | --- | --- | --- | --- |
| A1 | 本机 7890 | 3.641s | 5.455s | 5.763s | **403** |
| B1 | mini32 7890，经 SSH 转发 | 1.549s | 4.953s | 5.274s | **200**，26 条行程 |
| A2 | 本机 7890 | 1.824s | 3.371s | 3.693s | **403** |
| B2 | mini32 7890，经 SSH 转发 | 1.825s | 3.851s | 4.186s | **200**，26 条行程 |

每次均按原生事件序号验证四个子脚本结束早于等待放行，放行早于首次 shopping；不存在等待前已经提交 shopping 的情况。由此排除了执行主机、构建产物和这四个子脚本尚未执行的差异，结果仍随代理路径切换。这不等于确定了纯 IP 原因，也不否定 Chrome 经本机代理成功；代理路径与客户端行为的交互仍需继续诊断。每次完整请求、响应摘要及脚本时间线均已保存，另有同机逐请求对照表。临时转发已关闭，正式二进制和永久代理设置未改动。

### 同机交叉：额外等待 s3 响应完成

继续逐项分析前述 A/B/A/B 记录发现，四子脚本执行完成并不等于它们触发的异步请求完成。本机 A1/A2 的 `/di/swadvc/s3` 完整响应分别在 6.071s/4.000s 才返回，晚于首次 shopping；mini32 代理 B1/B2 则分别在 4.627s/3.598s 返回，早于首次 shopping。这解释了部分首次请求 Cookie 名称差异：本机此时尚未接收 s3 设置的 `swa_FPID`。响应正文也存在 262/2050 字节两种 HTML 分支，后者多一个内联脚本，但不能从返回 200 推导这些脚本已经执行。

为单独验证这个时序因素，隔离诊断版新增可关闭的等待条件：四子脚本执行结束之外，还要收到 s3 完整响应，再推进事件循环并保持原有 500ms 稳定窗口，才执行 app.js。所有五次运行仍在本机，mini32 只提供 SSH 转发的代理；同一二进制 `5ecafba3fea12aab3012ab85ae831436503a8b3f1ad590a049eefe1ba29fb54a`，每次新上下文，初始文档均无 Cookie。不移植 Cookie、不替换响应、不重放请求。

| 顺序 | 代理 | 额外等待 s3 | s3 完成 | 放行业务脚本 | 首次 shopping | s3 字节 | 首次结果 |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1 | 本机 | 是 | 3.251s | 3.778s | 4.108s | 2050 | 403 |
| 2 | mini32 | 是 | 4.027s | 4.558s | 4.885s | 2050 | 200，26 条行程 |
| 3 | 本机 | 否 | 3.632s | 2.988s | 3.322s | 262 | 403 |
| 4 | 本机 | 是 | 3.817s | 4.324s | 4.648s | 262 | 403 |
| 5 | mini32 | 是 | 3.719s | 4.274s | 4.598s | 262 | 200，26 条行程 |

四次额外等待样本均验证 s3 完整响应早于放行，放行早于首次 shopping；五次四子脚本均在放行前执行结束。本机等待后的首次 shopping 已自然携带 `swa_FPID`，仍返回 403；第二对等待样本的 Cookie 名称集合完全相同。因此，s3 未返回、缺少该 Cookie、262/2050 字节响应分支，均不足以单独解释失败。不能由此排除其他脚本行为差异或服务端状态。

请求细节方面，首对样本 shopping 前分别发起 45/42 次传输请求，最终全部返回 200；本机多出的三次为字体资源。两边均在首次 shopping 前阻止六次跨 origin 请求，并有相同的 cookielaw 动态加载错误。首对响应正文差异集中在主文档、swa-common、Akamai 变体脚本、cc.js、6.js、et.js 和 s3；其余对应正文哈希一致。首次 shopping 请求体仍为相同的 355 字节；传输层提交参数中的请求头差异为 Cookie/会话值、`x-user-experience-id` 和保护字段 a/f/b，其他头值一致。动态会话字段不同本身不构成缺陷证据。

已保存五次逐请求状态、正文哈希、脱敏头及执行时间线。两个本机记录在进程关闭时各有一条末尾截断日志，均晚于首次 shopping 证据，不作为成功请求计数。现有 classic/Worker 执行源码日志未匹配到 s3 内联脚本文本，但探针并未覆盖所有 eval/iframe 路径，不能据此声称这些脚本未执行。

当前证据仍支持“结果随代理路径变化”，不能简化为纯 IP 封禁，尤其 Chrome 已经在本机代理成功。后续应追踪不同响应的实际消费与执行、浏览器环境及传输/会话差异。新增等待只是因果实验，不是已证明的浏览器调度修复；没有把站点等待规则加入正式引擎。本轮正式二进制保持原哈希，未修改永久代理配置，也没有生产修复或全量回归通过声明。

### s3 响应消费路径：确认是 XHR，不是 HTML 文档导航

继续核查 cc.js 源码并增加隔离诊断探针后，确认此前“响应含 script 但未见执行”的疑点不适用于当前提交路径。cc.js 用 `if(true)` 选择 XMLHttpRequest POST；iframe 表单提交及 postMessage 等待位于未选中的 else 分支。XHR 的实际 readyState 回调为 `function(){if(v.readyState===4&&v.status===200)n()}`，仅检查完成状态并调用完成函数，不读取 responseText，也不执行响应 HTML。返回 HTML 中的 script 标签不会因 XHR 收到它们而自动执行。

在本机再次用同一诊断二进制 `0a75f1cc09cde041b765ed04faae6ec3b3b8db6b5d0697d02a7d0ab44f3381ec`、新上下文分别走本机和 mini32 代理，记录原生 XHR 实现内部的响应类型、实际回调函数及事件分发结束位置，没有替换网站回调。两边 s3 正文均为 262 字节，responseType 均为空字符串，readyState 回调源码完全相同，均无 onload 属性回调。

| 代理 | s3 XHR 提交 | 响应处理 | 事件分发结束 | 放行 app.js | 首次 shopping | 结果 |
| --- | --- | --- | --- | --- | --- | --- |
| 本机 | 3.699s | 4.182s | 4.183s | 4.693s | 5.060s | 403 |
| mini32 | 2.675s | 3.194s | 3.194s | 3.729s | 4.056s | 200，26 条 BWI-MCO 行程 |

事件分发结束标记证明运行推进到了该位置，不证明现有事件系统内部捕获并忽略的异常不存在。结合明确的源代码路径，当前没有证据支持“Obscura 漏执行 s3 返回 HTML 内的脚本”这一解释。

另对上一轮首对 cc.js 做字符串字面量比较：两份各有 1380 个字面量，替换字面量后的其余文本完全相同，只有两个字面量不同：一个长度 358 的字符串，以及位于 `if(false)` 的 timing-generation 分支中的 `"1"`/`"2"`。此检查不是完整语义等价证明，但实际提交分支与响应处理代码未发现差异。后续应把精力转向其他浏览器可观测行为、上游状态及传输差异，不能把这条已确认的 XHR 响应当成待执行文档。

本轮保留逐请求对照及消费路径报告，未修改正式引擎行为；正式 runtime 已恢复并核验原哈希。没有生产修复或全量回归通过声明。

### 其他脚本的正文差异分类

继续比较最近同机 consumer-local / consumer-mini32proxy 捕获的脚本，避免把不同正文哈希直接解释为不同程序：

- 6.js 与 et.js 替换字符串字面量后，其余代码文本相同；差异在传入函数的字符串参数。
- swa-common.js 在替换字符串字面量后，剩余不同区间集中于末尾同一位置的八个整数参数，未发现该区间之外的非字符串代码差异。参数仍可能影响行为，不能称为语义等价。
- Akamai 两个变体各有 584 次字符串表索引引用。仅解析字面量数组、用对应字符串替换索引，不执行脚本后，两份展开源码长度都为 15301 字符。非字符串代码文本完全相同，四个字符串常量不同，包括一个三位数字字符串和三个 32 字符串。说明此前大量索引差异主要来自表重排，不能直接归为不同功能实现；常量对结果的影响仍未排除。

规范化结果仅用于离线比较，未替换网站响应，也未修改保护脚本。原始会话数据及临时展开文件保存在受限临时目录，不纳入仓库。

### 分阶段代理交叉：初始化路径与 shopping 出口分开控制

新增六次同机实验，把页面初始化与 shopping 请求的代理分开配置。A 为本机代理，B 为 mini32 经 SSH 转发的代理；所有 SDK/runtime 仍在本机运行。四种组合都为 shopping 创建独立 primp 客户端，包括 AA/BB 同路径对照，从而避免把“换代理”与“新建连接”混淆。客户端的浏览器身份及传输配置一致，Cookie jar 和 JS 上下文仍由同一次浏览器运行持有；没有从其他上下文移植 Cookie、替换响应或重放已签名请求。

同一诊断二进制 SHA-256 为 `f3ef2c5d5029034405a89dbfd289ebc2fd6d685a963d03e8d55c702e3797ef35`。每次新上下文，四子脚本与 s3 等待条件相同，首个文档无 Cookie。已按原生记录验证：首次 shopping 前的 41 次传输提交全部走初始化客户端；shopping 走独立客户端；等待条件通过后才生成首次 shopping；六次首次 shopping 请求体哈希相同。

| 顺序 | 初始化代理 | shopping 代理 | 首次 shopping | 后续观察 |
| --- | --- | --- | --- | --- |
| AA | 本机 | 本机 | 403 | 持续 403 |
| AB | 本机 | mini32 | 200 | 返回 26 条 BWI-MCO 行程 |
| BB | mini32 | mini32 | 200 | 返回 26 条行程 |
| BA | mini32 | 本机 | 403 | 持续 403 |
| AB2 | 本机 | mini32 | **403** | 网站自然重试 **200**，26 条行程 |
| BA2 | mini32 | 本机 | 403 | 持续 403 |

**关键结论与边界：** AB 的首次成功证明，经本机代理初始化得到的页面、脚本与状态可以被接受；BA/BA2 说明初始化经 mini32 并不能保证 shopping 从本机代理发出后被接受。前四组结果随 shopping 路径变化，但 AB2 的首次 403 明确否定“mini32 出口保证首次成功”的表述。这仍然不能证明纯 IP 封禁，Chrome 经本机代理的成功仍需纳入解释。配置的代理路径也不等于已经测得稳定公网出口 IP，当前不是网络抓包证据。

AB2 第一次/第二次 shopping 分别于 3.935s/4.877s 提交，请求体相同。期间发生 version、security、analytics、content、beacon 和 Akamai 请求，Cookie 名称增加 at_check，因此不能把成功归因于单个 Cookie 或脚本。但同一上下文从 403 恢复为 200，说明“第一次拒绝一定持续污染该上下文”不是普遍成立的解释。

本轮显著缩小了调查范围：不能只围绕初始化脚本是否加载完成，应继续核对 shopping 传输、实际出口选择及其与浏览器身份/服务端状态的交互。实验分阶段路由仅在临时诊断目录，未加入正式 runtime；逐请求报告保留，正式二进制恢复原哈希，未修改永久代理设置。

### Computer use Chrome 本机代理：完整 HAR 与同次执行轨迹

按用户要求仅用 computer use 操作 Chrome，未使用 Playwright/CDP 驱动。直接导航同一 BWI-MCO 2026-09-30 查询链接，保存普通配置文件成功样本和隐身成功样本的完整 HAR（包括 Cookie/Set-Cookie）、Console，以及隐身样本从导航前开始的 Performance 轨迹。资源内容和可用 source map 导出选项均开启。用户要求优先数据齐全，原始会话字段保留在本机临时证据目录，不提交仓库。

普通配置文件首次文档携带 25 个 Cookie，shopping 200，26 条行程。隐身样本首次文档无 Cookie，首次 shopping 携带 10 个 Cookie，同样 200、26 条行程。隐身 shopping 的 DevTools Headers 明确显示 Remote Address `127.0.0.1:7890`、POST、200 OK；HAR 为 HTTP/2。系统 HTTP/HTTPS/SOCKS 代理也均为该入口。此证据确认相同代理入口，尚不是公网出口 IP 的独立测量。

**同一次隐身成功导航的实际执行轨迹：**

| 事件 | 导航后时间 |
| --- | --- |
| 主保护脚本执行 | 0.515–0.544s |
| swa-common 执行 | 0.581–0.619s |
| 首个 Worker 执行 | 约 0.690s |
| 第一个子脚本 65319 执行 | 0.735–0.736s |
| Worker 消息处理 | 约 0.736s、0.742s |
| 首次 shopping 提交 | **1.096s，最终 200** |
| 其余三个子脚本执行 | 1.202–1.207s |
| cc.js 执行 | 1.554–1.734s |
| s3 提交 | 2.232s |

这直接否定“成功的 Chrome 必须等四个子脚本和 s3 全部结束才提交 shopping”的一般化假设。普通配置文件成功 HAR 同样显示 shopping 1.681s 提交，而 s3 到 2.796s 才提交。不能把实验用等待条件直接合并为通用浏览器调度规则。

同期用无等待、无分阶段路由的被动诊断版复测 Obscura：Windows Chrome145 首次 shopping 1.996s 返回 403，四个子脚本在 1.359–1.446s 已完成；macOS Chrome152 首次 shopping 2.869s 返回 403，子脚本在 3.171–3.173s 完成。两次首次 shopping 前均无 Worker 启动记录，与 Chrome 有实际执行差异。不过此前等待实验已经让 Worker 提前完成而仍失败，不能把它认定为唯一根因。macOS 这次收到更新的 65350 子脚本，顺序 live 测试并非完全固定网站输入的身份实验。一个误写为 mac_chrome152 的尝试在初始化阶段被拒绝；实际记录使用受支持的 macos_chrome152。

隐身 Chrome 与同期 Windows Obscura 的主保护脚本、四子脚本、app.js、vendor.js 正文哈希全部一致。三者首次 shopping 请求体均为相同的 355 字节及原哈希。Chrome 请求身份为 macOS Chrome152，priority 为 `u=1, i`。后续逐项复核原始隐身 HAR 更正了语言头：其首次 shopping 的 Accept-Language 实际为 `en-US,en;q=0.9`；`en,zh-CN;q=0.9,zh;q=0.8` 属于 Obscura macOS profile，不能混作 Chrome 的采集值；其他浏览器表面、跨 origin 策略及传输属性仍未完全对齐，单独换用 macOS profile 尚未成功。

采集完整性明确记录：普通成功 HAR 213 条、163 个保存正文；隐身 HAR 100 条、91 个保存正文，隐身 Performance 257653 个事件。隐身缺少正文的九项为七个 preflight、一个 redirect 和一个跨 origin 文档；核心文档、业务脚本、保护脚本、s3、shopping 正文均有。隐身 UI 曾显示 107 请求，HAR 为 100 条，trace 有 101 个 ResourceSendRequest，尚未完全核销数量差异，不声称是全量线缆级抓包。轨迹记录 Worker 执行与 postMessage 调度/处理，但不保证包含消息载荷。另有一次重载未产生 shopping、一次带录制的新普通标签页崩溃，均单独归档，不与成功样本拼接。正式 runtime 未改动。

### Worker 构造入口、启动与独立调度的进一步验证（2026-09-17）

在临时诊断版本补充 Worker 构造函数入口和 `_autoRunWorker` 入口日志，未改变运行顺序。本机 macOS Chrome152 profile 复测仍为三次 shopping 403。首次导航中，网站在 1.293582s 调用 Worker 构造函数，来自 swa-common 的调用链；首次 shopping 传输提交为 1.737721s，Worker auto-run 为 1.898473s，实际执行 1.899742–1.900188s。此前“shopping 前没有 Worker 创建”的原生日志实际记录的是启动阶段，不能据此认为网站尚未调用构造函数。

采用本地离线 HTML，通过 computer use 操作 Chrome，与正式 Obscura CLI 比较三个最小场景。主线程均同步忙碌 500ms：

| 场景 | Chrome Worker 时间 | Obscura Worker 时间 | 结论 |
| --- | --- | --- | --- |
| 创建 Blob Worker 后立即忙碌 | 启动 508ms | 启动 503ms | 两者都延后，不能证明构造函数的零延时任务本身错误 |
| 收到 Worker ready 后发消息并忙碌 | 处理消息 500ms | 处理消息 500ms | 本例未显示差异，不能据此推断内部发送机制 |
| Worker 在 ready 前安排 100ms 定时器，主页面收到 ready 后忙碌 | 定时器执行 103ms | 定时器执行 500ms | Chrome Worker 可独立推进，Obscura 被主线程阻塞 |

三个场景的主页面消息处理都发生在主线程忙碌结束后，符合主页面事件循环约束。第三个场景证明独立调度能力存在差异；源码中 Worker 与主页面共用 V8 isolate、仅另建 Context，与观察一致。不能用同步执行 Worker 构造函数或简单改成 microtask 作为正确修复，这会改变执行顺序且不能提供独立推进能力。

当前证据仍未把此差异与 403 建立因果联系：此前受控等待实验已经让 Worker、四子脚本和 s3 在 shopping 前完成，本机仍为 403。后续需先建立独立 Worker 调度的通用回归约束，再评估独立 isolate/event-loop 的实现；同时继续核对同代理 Chrome 与 Obscura 的请求身份和传输差异。没有将针对站点的等待门槛合入正式代码，也没有宣称本轮修复了 Worker 或 403。

原始构造/启动日志与三组 HTML、CLI 输出、Chrome UI 观察值保留于本机临时诊断目录。正式 runtime 已核验恢复原 SHA-256，主项目本轮仅更新记录，未执行或声称代码修改所要求的全量回归。

### Worker 文档对照与红灯回归

已阅读 Chromium workers README、WorkerBackingThread、WorkerThread、DedicatedWorkerMessagingProxy 及 HTML Worker 处理模型，逐项对照 Obscura 当前实现。对照与修复边界见 [Worker compatibility investigation](Worker-compatibility.md)。Chromium main 文档不等同于本机 Chrome152 的精确版本源码；实测和源码结论分别记录。

独立开发工作树新增 `worker_timer_progresses_while_parent_is_busy`，按 release nextest 执行，确认基线失败：`ranDuringParentTask=false`，主页面异步投递断言仍为 true。这是已验证的待修复回归，不是通过的门禁。正式工作树暂未引入引擎修改。

### Worker 独立执行及消息克隆实现进度

独立开发工作树已实现每个 Worker 的线程、V8 runtime 和消息队列，补充并通过主线程忙碌时 Worker 继续运行、Worker 计算时主线程终止、close 后丢弃后续定时器的回归。随后将消息 JSON 编码改为 V8 序列化，新增两项回归先红后绿；最新 Worker 与保护脚本兼容性专项共 22/22 通过。

首次全量诊断为 1737 通过、4 失败、4 跳过；失败含两项当时的克隆红灯及两项渲染资源加载用例。渲染失败正在独立复验。这轮与共享 target 的迭代构建有重叠，不能算冻结版本的最终门禁。网络观测/拦截衔接、队列边界和完整验收仍待完成，正式 runtime 尚未替换，官网 403 未宣称解决。详细范围见 Worker-compatibility.md。

全量诊断中的渲染失败进一步定位到测试服务器的 socket 模式：非阻塞 listener 接受的连接在本机立即读会返回 WouldBlock，read_fixture_headers 直接 unwrap 导致夹具失败。独立 Rust loopback 复现后，仅在测试 helper 中显式 set_nonblocking(false)，保留 2 秒读取超时；原两项渲染用例随后 2/2 通过。此修改不涉及生产渲染逻辑，不能替代最终冻结版本的全量门禁。

### Worker 网络链路回归

独立线程实现已补齐初始拦截配置、共享请求编号、页面 in-flight 计数，以及网络事件和保留正文回传。新增本地 HTTP 回归验证主页面与 Worker 三个请求的拦截、回调、唯一编号、Worker 响应正文可读取及 Cookie 共享。同时发现 Worker 没有 DOM 时原生 URL 查询提前返回 null，已把环境 URL/base 查询移出 DOM 必需分支，验证相对请求从 /workers/main.js 解析到 /workers/data.txt。最新专项 23/23 通过。stealth 官网复测、运行中策略更新、队列边界及最终门禁仍待完成。


### Worker independent-execution follow-up, 2026-09-17

The development implementation now also bounds queues and nested worker counts, interrupts nested workers on owner destruction, and propagates interception, URL-blocking and console-policy updates after startup. The policy-update regression failed before the shared-policy change and passed afterwards. Getter reentrancy/error coverage verifies identity snapshots do not panic on native callbacks or leak a thread on failure.

Focused release nextest with `render,stealth` passed 28/28. Full render verification is being rerun sequentially on frozen source because the earlier broad diagnostic overlapped builds and was not a valid final gate. Detailed implementation boundaries and Chromium/HTML sources are in `Worker-compatibility.md`. No new Southwest success is claimed; installed runtime remains unchanged.

Frozen-source full release nextest (`--features render --no-fail-fast`) completed: 1747 passed, four skipped, zero failed. This replaces the mixed-build diagnostic as the full render test evidence. Release builds, obstacle course and live acceptance remain pending.

Render release build succeeded. Obstacle course: 32/33; retained old CLI also fails `observer-intersection` in a targeted rerun. The 33/33 requirement remains unmet; this is recorded separately from the green Worker and full nextest coverage.


### Independent Worker runtime live acceptance, 2026-09-17

The new runtime completed direct-goto BWI–MCO / 2026-09-30 through the local `127.0.0.1:7890` proxy: first shopping 200, success=true, 26 itineraries, and `Depart: BWIMCO` rendered. Full logical capture retained 124 request/response events with all corresponding bodies and no read/callback errors. A second run with lightweight capture also succeeded.

Both interleaved retained-runtime controls also returned first-shopping 200 and 26 itineraries. This prevents attributing the transition from historical 403 to current 200 to the Worker change. Independent Worker scheduling is a proven, separately tested compatibility repair; 403 causality remains unresolved. Native transport capture is still needed for exact outgoing cookies/headers and Worker execution timing in this successful state.

The frozen implementation has been integrated into the main worktree. Full render nextest 1747/1747 and focused render+stealth 28/28 passed; render release and runtime release builds succeeded. Obstacle course remains 32/33 with the same IntersectionObserver failure reproduced on the retained CLI. Six interleaved local performance pairs show independent timer execution in every new-runtime sample, about 3.4MB additional RSS for one idle Worker, and comparable idle CPU/median overall startup. See `Worker-compatibility.md` for ranges, outliers and open compatibility gaps.

A separate outer-runtime scheduling problem was observed: repeated 10ms read actions can starve the newly recreated 20ms autonomous-tick sleep. Queries spaced 50ms permit progress. This is not repaired by Worker isolation and remains a follow-up regression/fix target.

The subsequent passive native capture also succeeded (first shopping 200, 26 itineraries). It establishes shopping submission at 2.7425s, Worker source execution at 2.9025s, four protection children at 3.0238–3.0294s, and `/s3` at 6.1096s. This accepted request did not wait for those tasks. All five protection source bodies and the shopping request body match the earlier Chrome success; first-shopping cookie counts still differ (10 versus 11). Captured Chrome Accept-Language is en-US/en, unlike this runtime's en/zh-CN/zh. Exact values and capture-layer limits are recorded in `Worker-compatibility.md`. These differences coexist with success and do not establish historical 403 causality.

The frequent-query follow-up now has a real SDK subprocess regression: before the change, 0.6s of queries leaves a 50ms interval at zero; after preserving the tick deadline and prioritizing an expired tick before ordinary actions, the interval and Worker ready delivery both progress. Focused regression passed. Broader SDK/runtime validation for this additional change is in progress.


### 用户要求的再次采集与 Chrome 对照（2026-09-17）

最新调度修复版的新 workspace 复测，首次 shopping 和自然重试均为 403，正文 `{"code":403050700}`。紧邻运行未含高频查询修复的上一个独立 Worker 版本，同样两次 403。之前 200 / 26 行程的结果仍然有效，但不能据此宣称搜索已经稳定，也不能把本次失败归因于新调度修复。

以首次 shopping 发起为零：Chrome 首个 Worker EvaluateScript 为 −405ms；上一轮成功 Obscura 为 +160ms；本轮失败为 +157ms。本轮四个保护子脚本已在 −958～−926ms 全部执行完成，成功 Obscura 却在 +284～287ms 才完成。主保护脚本和四子脚本正文、355 字节请求体均与 Chrome 一致；Worker 顶层 9699 字节启动代码仅有随机 Blob sourceURL UUID 差异。Cookie 名称数仍为 Chrome 10、Obscura 11，成功与失败 Obscura 的名称集合相同。原始值和请求/响应全文保留于本地采集，未将这些差异直接判作 403 根因。

本轮 1140 个原生事件；144 次传输提交，143 个响应头，142 个逻辑响应。采集关闭时有一个 bf.html beacon 未返回，一个 301 仅记录重定向头；两次 shopping 的请求及响应完整。全部已记录正文文件存在。26 个 Worker task 有完整结束记录、无 task-level error；没有保护子脚本执行异常记录。

高频查询修复的完整 Python SDK 测试 33/33、runtime release nextest 184/184 均通过。核心 Worker 改动的完整 render nextest 1747/1747、render+stealth 专项 28/28 与指定 release 构建已通过；障碍课程依然是旧版同样失败的 32/33。未提交、推送或替换正式安装 runtime。接下来应补同步 Chrome 控制并测量代理出口/连接状态，再核对 Worker 导入源码及返回值；不把“相同代理入口”当作已经证明实际出口相同。

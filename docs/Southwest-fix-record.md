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

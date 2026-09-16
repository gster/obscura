# Southwest 首次 shopping 前的脚本调度审计

2026-09-16。结论：已确认两处真实调度差异，并用隔离实验改变了官网上的执行顺序；尚未证明它们是 shopping 403 的充分原因。隔离实验之后，这两处调度差异已修入正式引擎源码；本轮构建、回归和官网复测结果见末尾。

## 采集边界

- Chrome 通过 computer use 操作 DevTools，先录制 Performance，再直接输入构造的查询 URL。随后导出同一次导航的 Performance trace 和 HAR，没有使用 Playwright/CDP 驱动 Chrome。
- 查询仍为 LGA → LAS、2026-09-30、单程、1 成人。此次 Chrome 的首次 shopping 返回 200，正文 126161 字节。
- 新建隐身窗口不等同于独立的全新 cookie jar，本次没有将它声称为干净 profile。Performance 录制本身也有开销；下面的时间用于说明单次执行顺序，不是性能基准。
- Obscura 的诊断构建从当前源码复制到仓库外，记录经典脚本开始/结束、动态脚本执行、fetch 调用栈、Cookie 写入以及 primp 序列化之前合并的请求头。响应正文由 SDK 获取。跟踪代码未写入正式源码。
- 请求头的观测边界仍是 primp 序列化之前，不包括库自动添加的字段、HTTP/2 帧、压缩或 TLS；因此缺失字段不直接当作最终线级缺失。
- 严格按首次 shopping 切分。后续重试、回退导航、iframe 的 DOMContentLoaded 均不混入首次主文档阶段。
- 初始诊断与重复对照在后续导航阶段以 `BROWSER_EOF` 结束；首次 shopping 的 403 已由传输响应记录独立确认。不能将这两次诊断描述为完整工作流正常结束。调度实验版则完整返回 `SEARCH_HTTP_403`。

## 官网执行顺序

时间为各自 HTML 请求开始后的毫秒数，四舍五入。Chrome trace 用与 HAR 对应的 shopping 请求对齐时钟，未将 renderer 收到文档时的 ResourceSendRequest 误作网络导航起点。

| 事件 | Chrome | 当前 Obscura 诊断 | 调度实验版 |
| --- | ---: | ---: | ---: |
| 保护 bootstrap 开始执行 | 1295 | 2782 | 1810 |
| 子脚本 `65319_1825172515.js` 开始执行 | 1651 | 4252 | 2145 |
| 子脚本 `65257_1825202430.js` 开始执行 | 1951 | 4251 | 2135 |
| app.js 开始执行 | 2086 | 3017 | 2686 |
| 首次 shopping 发起 | 2362 | 3411 | 3062 |
| shopping 前已执行的保护子脚本 | 2/4 | 0/4 | 4/4 |
| 首次 shopping 响应 | 200 | 403 | 403 |

当前调度的重复对照同样为 0/4：shopping 在 4090 ms 发起，四个子脚本在 4974–4979 ms 才开始执行。实验版四个子脚本在 2135–2147 ms 执行，早于其 shopping 约 0.9 秒。

这不是“有没有下载保护脚本”的区别。配对样本中，HTML、app.js、vendor.js、保护 bootstrap 和四个保护子脚本的响应正文 SHA-256 均一致。`swa-common.js` 的正文不同，本次没有将该文件的执行输入声称为完全相同。

Chrome 并非要求四个子脚本全部执行完才发送 shopping。本样本只执行了两个就成功。实验版提前执行四个仍失败，也说明不能把“提前了多少个”直接作为服务器接受条件。

首次 shopping 的业务正文保持 355 字节，SHA-256 始终为：

`119015f152d04726f2d8661fe54ade04fb6fbd89994de4fced2fddd969e7a395`

两边的调用链都经过保护 bootstrap 的 fetch 包装层。Obscura 调用时 `document.readyState` 为 `loading`；Chrome 的调用发生在 app.js 的 EvaluateScript 内，早于主文档 DOMContentLoaded。不能把早先 iframe 的 DCL 事件当作主文档已经完成解析。

## 原因一：全部脚本下载完成后才执行第一个

修复前，`crates/obscura-browser/src/page.rs` 的 `execute_scripts_with_module_budget` 先执行：

```rust
let fetch_stream = futures::stream::iter(fetch_futures).buffer_unordered(16);
let fetch_results = timeout_at(script_deadline, fetch_stream.collect::<Vec<_>>()).await;
```

然后才建立 `fetched` 并遍历 `all_scripts` 执行。

`buffer_unordered(16)` 使下载并发，但后面的 `collect` 又把执行拦在全部下载完成之后。早已可执行的 parser-blocking 脚本不能立即运行。Chrome 在等待后续 parser-blocking 脚本时，前一个脚本可以已经执行，并产生新的动态脚本请求。

官网样本正好体现了这个区别：Chrome 的 bootstrap 在较慢的 `swa-common.js` 到达之前就执行了，其子请求因此拥有运行窗口。Obscura 将 bootstrap 延后，随后连续执行已全部下载好的业务脚本，直接进入 shopping。

## 原因二：parser 等待时没有推进页面异步任务

原实现执行经典脚本的循环没有在外部 parser-blocking 脚本等待期间处理页面事件循环。动态脚本的 `fetchResult`、Promise continuation 和执行回调需要 V8 事件循环推进；仅让 Rust 的外部脚本下载 future 继续运行，并不能保证动态子脚本得到执行。

原路径显式驱动这些 load-delaying 动态脚本的主要阶段在 parser/defer/module 工作结束之后：`drive_load_delaying_scripts`。对首次 shopping 来说，这已经太晚。

隔离实验做了两件事：

1. 保留并发下载，但等待当前需要的脚本，而不是等待整个集合。
2. 在等待期间同时推进现有 `run_autonomous_event_loop_turn`，保留页面总 deadline 和同步执行 watchdog。

实验没有改网站脚本、业务正文、Cookie、代理、origin 列表或保护字段，也没有向 shopping 前添加固定等待。

## 两个本地握手复现及消融实验

两个外部 parser 脚本 A、B 同时可被下载。B 的服务器处理器等待一个 marker；收到 marker 即返回，2 秒未收到则返回超时标记。

- 复现 A：第一个脚本直接请求 marker。
- 复现 B：第一个脚本动态插入子脚本，由子脚本执行后请求 marker。

这是服务端事件握手，不以“等待若干毫秒后看起来正常”为成功标准。

| 版本 | 复现 A：首脚本直接通知 | 复现 B：动态子脚本通知 |
| --- | --- | --- |
| Chrome，computer use | marker 先到 | marker 先到 |
| 正式当前 runtime | 服务器等到超时 | 服务器等到超时 |
| 仅移除整批等待的实验版 | marker 先到 | 服务器等到超时 |
| 移除整批等待并推进页面事件循环 | marker 先到 | marker 先到 |

因此可以分别确认批量下载屏障与页面异步任务调度问题。官网实验的执行顺序也发生了预期变化，因果链不再只依赖静态源码推断。

这是诊断实验，不是可发布的调度器补丁。正式实现还必须完整覆盖 parser async/defer、模块/import map、失败响应、超时及 lifecycle 顺序；本轮没有把实验版本安装成正式 runtime。

## 同阶段其他差异

### 渲染资源抢在脚本之前

当前 `navigate_with_wait_post_inner` 的顺序是：加载样式表、预热截图所需资源（默认预算 1000 ms），然后执行脚本。

本样本中，Obscura 在 719–724 ms 发起 13 个字体和 1 个 SVG 请求，直到 1721 ms 才开始页面脚本请求。Chrome 在 1009 ms 同时发现页面脚本和两份 CSS，在首次 shopping 之前没有发起这 14 个渲染资源请求。

两者在首次 shopping 前分别有 26 与 38 个请求；如果计入 shopping 本身，则为 27 与 39。差额由 Obscura 多出的 14 个渲染资源，以及被其 origin 策略阻止的两个 Chrome 请求解释。

这属于另外一个加载调度差异。不能简单删掉预热就宣称正确，因为该路径原本用于避免脚本里的同步布局/字体查询阻塞；需要把脚本发现与资源准备并行化，并保留真实的布局行为。

### 请求头和外部依赖

25 个匹配请求的 URL 查询参数一致。观察到的差异包括：

| 字段/行为 | Chrome | Obscura 诊断观测点 |
| --- | --- | --- |
| parser 普通脚本 Priority | `u=1` | `u=0, i` |
| 动态子脚本 Priority | 该 HAR 未列出 | `u=1, i` |
| 动态脚本 Accept | `*/*` | 合并头未列出，仍需线级确认是否由库补入 |
| 四个同源 CORS 子脚本 Origin | `https://www.southwest.com` | 合并头未列出 |
| Accept-Language | `en-US,en;q=0.9` | `en,zh-CN;q=0.9,zh;q=0.8` |
| 四个子脚本 Fetch mode/destination | `cors` / `script` | `cors` / `script`，上轮修复在本次生效 |
| cookielaw、demdex | 发出请求 | www-only OriginGuard 阻止 |

UA、Client Hints、Fetch site/mode/destination 以及 shopping 的其他已比对静态头在对应观测边界一致。上述缺失头不混同于已证实的调度缺陷，且没有在本轮同时修改它们以冒充单变量实验。

### Worker 仍不是真正独立的执行环境

Chrome trace 显示 DedicatedWorker thread 在首次 shopping 前执行过脚本，且与主线程脚本区间重叠。Obscura 的 `_autoRunWorker` 仍通过页面 isolate 中的 `new Function(...with(scope)...eval...)` 执行，并共享一批页面 realm 的内建对象。

同一个本地 Blob Worker 复现得到：

| Worker 内的检查 | Chrome | 当前/实验 Obscura |
| --- | --- | --- |
| `Function('return this')() === self` | true | false |
| `Function('return typeof document')()` | undefined | object |
| 页面给 `Object` 设置的标记能否读到 | false | true |

这是已复现的 realm 隔离差异，前一轮 onmessage/EventTarget 修复没有覆盖它。但尚未证明 Southwest 的保护脚本读取了这些具体值，也未证明修好它就能消除 403。

## 隔离实验结束时的状态

当时已安装 runtime 保持
`8399f9b5067143bd541f7b1c5f292de672a4649847c40c9e612b6e7c22bc35ab`。

确认：调度器确有问题；隔离修正能让保护子脚本在 shopping 前执行。
未确认：该差异是否参与服务器拒绝判定。仅修正这一调度行为仍得到 403，其他执行环境与请求差异仍需逐一验证。


## 正式调度修复

`execute_scripts_with_module_budget` 不再收齐所有响应才执行。下载任务保持最多 16 个并发请求，完成项通过有界通道交回页面线程；等待特定 parser-blocking 或 defer 脚本时，只等待该索引，并处理已经就绪的 async 脚本。失败项保留索引，HTTP 非成功正文继续禁止执行。模块图加载期间下载任务仍能推进；离开执行阶段或取消导航时，下载任务随调度器销毁而取消。

等待响应时同时推进现有 V8 autonomous event loop，使动态脚本、Promise 和网络完成回调有执行机会。事件循环报告 idle 后等待网络唤醒，不增加固定毫秒延迟或 busy poll。致命事件循环错误向页面生命周期返回；原总 deadline、V8 watchdog、模块 active-work budget 和 URL 校验保留。

普通外部脚本按 parser 顺序执行，defer 脚本在 interactive 阶段按顺序执行；parser-inserted async 脚本不阻塞 DOMContentLoaded，但仍阻塞 load。这与 [HTML script processing model](https://html.spec.whatwg.org/multipage/scripting.html#the-script-element) 的相关生命周期规则一致；这里不声称整个 parser 或模块实现已完全符合标准。

新增真实 HTTP 回归：

- `parser_script_executes_before_later_response_finishes`：第二个响应等待第一个脚本发出的 marker。
- `parser_wait_drives_dynamic_script_and_fetch_continuations`：marker 必须由第一个脚本插入的动态子脚本发出。
- `parser_async_defer_and_failed_fetch_preserve_lifecycle_order`：同时覆盖断连、404 不执行、async 不阻塞 parser/DCL、defer 顺序和 load 等待。
- `parser_deadline_preserves_completed_scripts_and_bounds_pending_downloads`：一个 async 请求永不返回，已完成脚本仍执行，导航阶段由总 deadline 截止。

前两个回归在修改前均失败，修改后通过。原 `ready_async_classic_script_runs_before_a_later_parser_import_map` 用例依赖全部下载屏障，无法证明 async 响应已就绪；现在使用立即可取的 data 脚本和随后的阻塞响应，明确提供 parser 等待窗口，继续断言它不能看见尚未解析的 import map。模块队列预算与 import map 的其他期望未放宽。


### 后续请求头定位

调度修改之外的本地 HTTP 服务端回显已确认三点，尚未将其认定为 403 原因：

1. `script.crossOrigin = 'anonymous'` 在 Chrome 下得到 `mode=cors` 和同源 Origin；当前 Obscura 仍为 `no-cors`。使用 `setAttribute('crossorigin', ...)` 时 Obscura 能正确切到 cors。源码只在图像接口实现了该反射属性，脚本接口的成员拷贝表没有可复制的 Element descriptor。
2. 即使使用 setAttribute，Obscura 同源 cors 脚本仍不带 Origin。`fetch_origin_header` 当前只对跨源 cors GET/HEAD 添加 Origin；它没有区分普通 fetch 与脚本 destination。Chrome 普通同源 fetch 也不带 Origin，因此不能简单给所有 cors GET 添加该头。
3. 动态脚本与普通 fetch 的 Accept 在线上确实缺失，primp 没有自动补 `*/*`。Chrome 本地回显则带 `*/*`；Obscura 的 scripted request 构造只加入 Fetch Metadata、Priority 和调用方提供的头，没有默认 Accept。

Chrome 仍通过 native computer use 操作；本地回显为 HTTP/1.1，不能把 Priority 的本地差异直接推成官网 HTTP/2 帧级结论。长 AX 文本仍有 1024 字符截断，证据只使用完整可见的条目，没有补猜被截断内容。


### 正式版本复测

安装后的正式 runtime SHA-256：

`ac730c9b5efc905bfa02c0b5496400ff97c9e266ebf16849f018a6ccb7cead68`

SDK 直接 goto 的动态子脚本握手返回 `first-script-ran`。同一正式二进制完整运行 Southwest 工作流，结果仍为 `SEARCH_HTTP_403`；首次 shopping 正文仍为 355 字节，SHA-256 与上面的对照相同。

从相同源码另建仓库外诊断版本，得到首次导航时序：

| 事件 | HTML 请求开始后 ms |
| --- | ---: |
| 保护 bootstrap 开始 | 1097 |
| 四个保护子脚本开始 | 1232、1246、1260、1266 |
| app.js 开始 | 1546 |
| 首次 shopping 发出 | 1963 |
| 首次 shopping 响应 403 | 2796 |

四个子脚本均先于 shopping 执行，证明正式调度修复已在官网路径生效。这是单次执行顺序证据，不是性能提速结论。诊断版本在后续阶段遇到 `BROWSER_EOF`，首次 403 已由传输响应记录确认；正式无探针版本完整返回业务失败。诊断二进制未留作安装版本，已恢复并核验上面的正式 SHA。

验证状态：

- 聚焦 parser、import map、module budget、load-delaying 回归 18/18；新增 deadline 回归随后在全量中通过，四个新增测试全部通过。
- 独立 runtime release nextest 184/184；正式 runtime 的 Python SDK 32/32。
- 全量 render release nextest 1722/1727，另 4 项跳过。失败项为 page transport prefetch、page-wide resource concurrency、fixed wait font、awaited expression font 和外网 MCP evaluate。
- 上述失败项串行复核：prefetch、awaited expression 和 MCP evaluate 通过；资源并发与 fixed wait font 仍失败，与前一轮串行基线相同。没有放宽断言或把复测通过写成全量全绿。
- 指定 render CLI release 构建通过；障碍课程仍为 32/33，失败为原有 `observer-intersection`，33/33 门禁尚未通过。
- 全量构建/测试并行时，最初两次 SDK init 超时，未到达导航；结束重负载后串行重跑，本地握手和官网工作流都正常完成。没有修改初始化超时来掩盖这两次失败。

此次正式源码修改范围是脚本调度。上节列出的 crossOrigin 属性、同源 Origin、默认 Accept，以及 Worker realm 隔离差异尚未修复，不能宣称 Obscura 已完成 Chrome 替代验收。

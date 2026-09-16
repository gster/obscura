# Obscura 修复日志

本日志记录 Python SDK、ZG 官网验证及浏览器一致性工作中的引擎修复。SDK 新增接口的完整说明见 `bindings/python/README.md`。未提交的工作按日期归档；“通过”只代表列出的验证，不代表全部验收通过。生成日志、截图和网络证据保存在仓库外，不包含账号或乘客资料。

## 2026-09-16：Southwest computer use 对照

- Chrome 普通窗口和新开的无痕窗口经 computer use 均显示 LGA→LAS、9 月 30 日的 26 个行程。普通窗口切换日期后的两个 shopping POST 实测为 200；目标日期的响应为 `success=true`、26 个行程、0 个直飞。新建 Playwright 会话的 403 不能代替此成功对照，也不能据此判断为 IP 问题。
- 修复独立 runtime 的可访问名称计算：空白 `aria-label` 不再遮蔽按钮自身文本。真实 SDK 回归修改前为 `count=0`，修改后通过；官网原生角色定位和点击恢复，之后 shopping 仍为 403。未将此定位修复声称为搜索放行修复。
- 当前 Windows Chrome 145 身份分别使用 primp 和隔离 wreq 适配器，均返回 403。该实验保留当前 JS/DOM，不是历史成功二进制的复现；正式 runtime 保持 primp。
- 聚焦 Rust 回归、184 项 runtime 测试、25 项 SDK 测试通过。全量测试和障碍课程仍有失败；完整条件、证据边界及门禁结果见 [Southwest 对比记录](Southwest-search-comparison.md)。最终正式源码 runtime 查询仍为 `SEARCH_HTTP_403`，业务验收未完成。

### Cookie 来源追踪与统计更正

- 撤回 Chrome 首次“5 个 Cookie”的统计：长请求头 AX 文本被截断。新隐身样本通过 DevTools Request Cookies 独立行确认是 10 个；Response Cookies 不计入请求数量。
- Obscura 的 11 个与 Chrome 的 10 个共有 9 个。`swa_spa_grp` / `swa_FPID` 的差别直接来自最初 HTML 的 Set-Cookie；Obscura 没有漏存已下发的 `swa_FPID`。
- www-only origin 限制使 Adobe 请求被拦截，分析脚本在首次 shopping 前写入 `AMCVS_...`。临时配置加入 Chrome 实际访问的 demdex 与 smetrics 两个 origin 后，写入推迟至网络返回之后；恢复 www-only 后又提前。没有修改正式配置或移除 OriginGuard。
- 放行分析域名的对照样本首次 Cookie 名称集合与 Chrome 完全相同，仍为 shopping 403。Cookie 数量已找到解释，不等于搜索拒绝的根因已经解决。详见 [Cookie 来源审计](Southwest-search-comparison.md#cookie-provenance-audit-and-corrected-count)。

### 动态经典脚本的 CORS 与凭据修正

- 真实双 origin HTTP fixture 在 Chrome computer use 下确认五种行为：默认脚本携带目标 Cookie；anonymous 不携带跨源 Cookie；use-credentials 携带；缺少 CORS 许可触发 error 且不执行；跨源 base 只改变 URL 解析，不改变请求 Origin。
- 修复前 SDK 回归失败，修复后通过。动态脚本准备阶段现在记录 crossorigin 对应的 mode/credentials，并从文档 URL 取得 Origin。没有修改网站脚本或添加域名特判。
- 同一 fixture 的请求头回显进一步确认：动态脚本错误地发送 `Sec-Fetch-Dest: empty`。修复后为 Chrome 的 `script`；请求类型同时贯穿两种传输、重定向与请求观察接口，普通 fetch 的默认值保持不变。扩展回归经历先失败后通过。
- 最终版本 7 项聚焦回归、184 项 runtime 回归通过；SDK 为 31/32，初始化超时的一项单独复测通过。render 与显式 stealth 的 HTTP fixture 均与 Chrome 一致。全量 render 为 1720/1723，障碍课程仍为 32/33。最新正式 runtime 直接 goto Southwest 仍为 `SEARCH_HTTP_403`，不能把规范一致性修复声称为业务成功。详见 [复现与验证](Southwest-search-comparison.md#dynamic-classic-script-fetch-correction)。

### Parser 脚本调度正式修复

- 去除全部外部脚本下载完成后的统一执行屏障。保留 16 个并发下载，在当前阻塞脚本响应就绪时执行；async 与 defer 保留各自生命周期，失败请求保留索引，导航取消会中止下载任务。
- parser 等待外部响应时推进 V8 事件循环，使动态子脚本、Promise 和 fetch 完成回调得到执行。保留总 deadline、watchdog、模块预算和 URL 安全校验，没有站点特判或固定等待。
- 新增四个真实 HTTP 回归，覆盖直接与动态子脚本握手、失败/async/defer/DCL/load 顺序、截止期限下保留已完成工作。前两个先失败后通过；聚焦回归 18/18，新增 deadline 随全量通过，runtime 184/184、SDK 32/32。
- 正式 runtime 已更新至 SHA `ac730c9b5efc905bfa02c0b5496400ff97c9e266ebf16849f018a6ccb7cead68`。SDK 本地握手通过，Southwest 直接 goto 仍为 `SEARCH_HTTP_403`。仓库外诊断确认四个保护子脚本均在首次 shopping 前执行；其后续 EOF 与正式工作流结果分开记录。
- 全量为 1722/1727，另 4 项跳过；失败串行复核仍有既有资源并发与字体等待失败。render CLI 构建通过，障碍课程 32/33，未宣称全绿。继续定位到 crossOrigin 属性反射、同源脚本 Origin、默认 Accept 的通用差异，详见 [调度审计](Southwest-pre-shopping-scheduler-audit.md)。

## 2026-09-15

### 本轮最终验证状态

- SDK：30/30；独立 runtime release nextest：183/183；针对事件循环的 release nextest：12/12；安装 wheel 后原生输入/网络正文及离线完整购票冒烟：2/2。render 与 render,stealth 的指定 release CLI 构建通过。
- 全量 release nextest 曾达到 1712/1712，但最终默认并行复测为 1709/1712，失败涉及资源加载并发、等待期间字体应用及 MCP 外网页面读取。限制为 2 个测试并行后仍为 1709/1712，失败为 `a_load_that_finished_during_the_scan_does_not_cost_the_deadline`、`a_miss_created_inside_an_awaited_expression_loads_during_the_wait`、`timers_inside_a_fixed_wait_observe_bytes_that_land_during_it`。保留失败记录，未将早先通过结果冒充最终稳定通过。
- 障碍课程 render 与显式 `--stealth` 运行均为 32/33。失败项 `observer-intersection` 在 Obscura 与 Patchright 有头 Chrome 152 下均只加载 10 项，未满足 fixture 的 `io:50` 期望。未改动课程期望或加入重复伪造回调，33/33 门禁仍未完成。
- 确定性渲染完成 66 组 Obscura/Chrome 配对截图，但检查器的 4 项 Chromium 参考断言失败，涉及 serif 字体度量、input size 和 textarea rows/cols。不能计为全部通过。
- 真实网站完成 15 站顶部和底部配对捕获；方法学可比页面分别为 9 和 10，其余因捕获失败或状态边界不同排除。检查发现 Vue 主体布局接近，但仍有局部装饰差异；Bootstrap 底部列换行和固定标题叠加仍有明显差异。未把像素指标或可比标记当成保真通过。

### Obscura 官网到达支付前

- 真实 Obscura SDK、macOS Chrome 152 profile、本机 7890 代理，通过首页、单成人单程搜索、日期与 Standard 选择、乘客信息、无附加服务确认、收据收件人，到达 `/zh-cn/booking/payment?ci_skip_flag=skipped&ti_skip_flag=skipped`。页面显示选择支付信息及进入支付入口，未进入支付、未填写卡资料。
- 本次为未登录购票流程，不能据此声称此前登录回调问题已解决。Cookie 仅在首页点击并等待隐藏，之后未再次出现。
- 已保存仓库外确认页、收件人页、支付页截图、网络事件及哈希清单。观察入口共记录 278 个 request、254 个 response，正文读取无错误；这是 SDK 的逻辑网络观察记录，不冒充 Chrome HAR 或完整线级抓包，二进制响应的文本日志不作原始字节证据。
- 对照 Chrome HAR：24 个顶层乘客字段完全一致，收据姓名和邮箱一致。航线、航班、时间、Standard 及票价基础代码一致；实时 USD 总价由 349.39 变为 348.74，余位由 16 变为 14。未勾选额外服务或营销订阅；订阅字段缺省与 false 的差异保留记录。
- `/v2/pnr/payments` 与 `/v1/pnr/amounts` 均返回 200；本次没有把第三方指纹脚本请求失败或支付接口可读等同付款成功。完整浏览器指纹及最终传输头一致性仍未宣称完成。

### 条件等待不再被任务 watchdog 误判

- 复现：连续等待不存在元素，50 毫秒的条件超时被错误报告为 `INPUT_TIMEOUT / UNKNOWN`。watchdog 原来覆盖整个异步观察窗口，页面没有执行 JS、仅等待事件时也可能触发。
- 修改：复用自主事件循环的同步入口 watchdog，仅在微任务检查及实际 poll V8 时启用；等待网络/定时器期间解除。SDK 传入当前 action 的绝对截止时间，观察窗口也不超过该时间。普通页面异常仍按原逻辑记录，真正的同步任务超时仍终止会话。
- 保留空闲任务默认 30 秒预算；既有 CDP 调用继续使用原任务预算。包含普通短等待、长同步任务、正文并行读取、无限循环和取消的 17 项 SDK 等待回归通过。

### 普通按钮点击后允许应用禁用自身

- 官网在跳过附加服务时，按钮点击回调把自身禁用；原生点击派发已经完成，但没有浏览器默认行为的普通按钮仍重新检查启用状态，错误返回 `ELEMENT_DISABLED / SENT` 并结束会话。
- 修改：普通按钮的 click 回调完成后不再要求其保留点击前的启用状态，但仍重新确认默认行为类别没有改变。保留输入前检查、鼠标事件中间检查，以及链接、表单和勾选控件的默认行为校验。点击不会重放。
- 回归：按钮回调禁用自身并更新完成标记，修改前报错，修改后通过；Patchright 有头 Chrome 对照通过。最初范围过宽的实现被既有“标签重定向及新增默认动作”回归检出，收窄后该回归及独立 runtime 183 项全量测试通过。

### 空闲期间继续推进页面任务

- 复现：SDK 点击后不再发送页面操作，让定时器执行 6 秒同步任务，旧空闲 tick 调用截图用 settle，任务被中断且异常被吞掉，最终页面没有完成标记。
- 修改：空闲 tick 同样使用支持任务截止时间的事件循环，采用默认 30 秒任务预算，并保留 30.25 秒外层截止时间。执行故障将会话标记为不可继续，不再将被中断页面视为正常。
- 此项独立于用户操作的自定义超时；正常等待操作仍使用调用者传入的截止时间。6 秒空闲任务回归通过。

### 网络正文读取容纳同步页面任务

- 原因：正文读取默认使用旧协议的 5 秒 RPC 超时；合法的长同步 V8 任务暂时阻塞证据读取，Python 因正文读取超时终止整个会话，官网表现为 `BROWSER_REQUEST_INTERRUPTED`。
- 修改：高层 SDK 正文分块读取使用协议允许的最大 300 秒等待。页面 action 的独立截止时间、V8 watchdog、传输取消和进程回收保持有效；不占用页面 action 锁。
- 验证：6 秒同步页面任务期间并行读取已捕获响应正文，修改前会话中断，修改后正文完整返回且页面任务完成。官网后续复测尚在进行。

### ZG 日期下拉框：不参与命中的伪元素

- 用官网 SSR 结构与已捕获 CSS 形成仓库外离线复现，确认命中测试拒绝来自绝对定位伪元素检查。日期下拉框 `.form-select:before` 明确设置了 `visibility:hidden` 和 `pointer-events:none`，但原检查仍把它的区域当成潜在遮挡。此前对变换/滚动的推测不是该控件的实际根因。
- 修复仅让伪元素检查遵循自身的 visibility 与 pointer-events 覆盖值；保持 opacity:0 且可接收指针的覆盖层拦截规则，不跳过普通遮挡或输入验证。
- 最小 SDK 回归修改前返回 `INPUT_GEOMETRY_UNSUPPORTED`，修改后通过；Patchright + 有头 Chrome 152.0.7977.83 对照也通过，均允许隐藏/不接收指针的装饰伪元素下方按钮，并阻止透明可接收指针的覆盖层下方按钮。

### Cookie 同意状态与页面任务截止时间

- 官网通过 localStorage 的 `cookie_policy` 保存同意状态。确认旧实现刷新后返回 null；Storage 的数据原来只存在于被导航替换的 JS runtime 中。
- Web Storage 改为原生保存，localStorage 由浏览器上下文按 origin 共享，sessionStorage 按页面和 origin 隔离，并传递给子框架。刷新不丢失；每个 area/origin 限制 5 MiB，写入超限不覆盖原值。当前保存限于上下文生命周期，不新增磁盘保存或跨窗口 storage 事件。
- 真实 SDK 回归覆盖刷新、新标签共享 localStorage 与 sessionStorage 隔离，修改前失败、修改后通过。官网流程改为首页点击后明确等待 Cookie 按钮隐藏，取消每页重复点击的临时绕过。`examples/zg/flow.py` 与离线 fixture 同步为首页同意后等待隐藏，登录后不再重复点击；离线完整购票回归通过。官网本轮由首页到套餐及乘客页均未再次出现 Cookie 弹层。
- 另一个回归确认页面事件循环会把 6 秒的正常同步任务中断，即使外层等待给了 12 秒。SDK 改为按同一个操作截止时间推进页面任务，20 毫秒仅作为让出控制的观察窗口；watchdog 到期明确返回执行故障，不再吞掉中断当作页面已稳定。慢任务与无限循环保护均通过。官网复测继续进行。

### SDK 等待变换后的可操作几何

- 排查 ZG 日期下拉框时发现通用边界：元素的临时变换会使滚动准备返回 `INPUT_GEOMETRY_UNSUPPORTED`，此前立即结束操作。ZG 控件实际根因见上方伪元素修复。
- 最小回归使用初始缩放、500 毫秒后恢复的按钮，修改前失败。现在仅在尚未派发输入的滚动准备阶段等待该条件恢复，继续推进事件循环，并共用操作截止时间；不重放已派发的点击，也不伪造几何。
- 三项真实 SDK/runtime 回归通过：变换恢复后点击、超过 1 秒的正常同步点击、无限循环点击终止。官网复测进行中。

### SDK 输入阶段统一使用操作截止时间

- 最小回归：点击处理同步执行 1.2 秒，调用方设置 4 秒超时，原实现仍因输入阶段独立的 1 秒上限返回 `INPUT_TIMEOUT` 并结束会话。
- 修改：滚动准备和原生输入的 V8 watchdog 使用同一操作截止时间的剩余时长，移除额外的 1 秒上限。保留 watchdog、硬超时后的会话终止和输入状态不明时禁止重放。
- 验证：正常同步点击回归修改前失败、修改后通过；无限循环点击仍被终止，两项真实 SDK/runtime 测试通过。尚不能据此断言 ZG 乘客页点击问题已解决，官网复测继续进行。

### ZG 人数页路由：CSS 预加载完成事件

- 诊断：Vue Router 的 pending 路由已是 `/booking/people`，异步组件等待的 JavaScript chunks 全部执行；其 CSS loader 使用 `link.relList.supports("preload")` 选择预加载并等待 load。原实现宣称支持 preload，却只处理 stylesheet 链接，新增 CSS 预加载从未发起请求，路由 Promise 因此不完成。
- 回归：`css_preload_completes_without_applying_styles_or_fetching_imports` 修改前超时，实际请求数为 0。用同样的 DOM 插入路径覆盖成功和 404，不以伪造 load 事件代替加载。
- 修改：CSS 预加载复用现有受策略约束的网络读取，成功后发 load，失败发 error；预加载阶段不应用 CSS、不展开 @import。样式链接仍执行原有导入和级联逻辑。本轮暂不实现浏览器完整 preload 缓存复用，后续 stylesheet 可能再次读取同一资源。
- 官网复测已实际进入人数页，截图与页面确认 KUL→NRT、成人 1、其余 0；原路由停滞已解除。预加载与 stylesheet 回归 2/2，通过 Patchright 有头 Chrome 预加载对照。最终门禁进行中；临时路由探针已从源代码移除。

### 本轮验证状态

- 独立 runtime release nextest 183/183 通过；表单传输、重定向、Document 命名访问、预检头和文本更新的 focused 回归均通过，文本更新另经 Patchright 有头 Chrome 对照。
- 最终全量 render release nextest：1706 通过、4 失败、4 跳过。失败为资源并发限制、固定等待中的定时器、await 内资源加载及公共 DNS 校验；仍未满足全量门禁。
- render 与 render,stealth release CLI 构建均通过，runtime 已恢复为无临时 console 诊断输出的正式源代码并重建。render 与 stealth 障碍课程均为 32/33，observer-intersection 均未通过，33/33 门禁仍未满足。
- 官网仍未完成支付前验收：OAuth 最终回调曾返回 500。CSS 预加载修复后，匿名流程已进入人数、日历、选舱、套餐及乘客路由，目标日期航班响应与 Standard 价格一致；乘客页客户端跳转仍有加载停滞，刷新后表单可见，但原生性别点击曾触发输入硬超时。未填写银行卡或触发支付。上述全量门禁结果早于 CSS 预加载及输入截止时间修复，须重新运行。

### Chrome HAR 对照：响应成功后的文本节点更新

- 官网原生选择 KUL 后，storages 请求正文为 KUL→NRT、单程、USD，tokens/storages 返回 200，空响应正文与 Chrome 一致；界面仍显示旧城市文字且未完成下一步。
- 最小回归确认 `Text.textContent = ...` 错误继承 Node 的子节点替换逻辑：原文本未改变，反而出现非法文本子节点。修复为 CharacterData 的文本数据访问，并沿用 characterData mutation 通知。
- 修改前 `text_node_text_content_updates_connected_text_without_children` 失败，结果为旧文本和一个子节点；Patchright 有头 Chrome 对照返回新文本且无子节点。修复后 release 回归通过；官网出发地和目的地文字均正确更新。人数页分块脚本返回 200，但路由仍未完成，不能把该问题认定为路由停滞的全部原因。

### Chrome HAR 对照：Document 名称访问与预检请求头

- OAuth 回调通过 `document.callbackform.submit()` 提交表单。原 Document 缺少表单名称访问，导致回调页面 200 后停留；现为符合条件的命名元素注册延迟解析属性，DOM 替换后重新解析，不把普通 div 的 id 注册为 Document 属性。
- `named_document_form_submits_callback_and_tracks_replacement` 修改前失败，修复及优化后 release nextest 通过；官网实际发出 `/authentication` POST，字段集合和稳定值与 HAR 一致，之后按重定向转 GET。站内 `/zh-cn/idp_callback` 返回 500，不能把 OAuth 成功等同完整登录成功；未复用 HAR 的身份令牌。
- HAR 预检不含 Client Hints，原 primp 默认头却给预检补上 `sec-ch-ua*`。现将身份默认头移出 primp 自动合并层，仅在识别为 CORS 预检时不补 Client Hints，普通 OPTIONS/GET 保留。
- 本地 TCP 接收真实请求字节的 `cors_preflight_omits_client_hints_on_wire` 修改前失败、修改后通过；普通 OPTIONS/GET 保留 Client Hints 的扩展对照也通过；完整门禁仍未通过。此修复不跳过预检、不更改响应或放宽 CORS。

### Chrome HAR 对照：原生表单 POST 的传输分叉

- 对照基线：本机 Chrome Computer Use 已完成登录、指定航班和乘客填写，到达支付方式选择页；未进入付款。HAR 保存在仓库外，实际导出 489 条，DevTools 计数 506 条，差异未逐条定位。金额复核预检存在 403，因此该基线不代表支付链路通过。
- 本轮 Obscura：首页、航线 API、登录表单 GET 均为 200；原生提交登录 POST 返回 403。表单字段名与 HAR 一致，账号、CSRF、OAuth challenge 等会话值不按字面复制。
- 原因：`Page::navigate_single` 的 POST 分支固定调用普通 HTTP 客户端，绕过页面 primp stealth 客户端。403 的全部原因尚未确定；传输分叉已由本地回归独立证实。
- 修改：原生表单 POST 使用页面选定传输；stealth 表单复用现有来源检查、Cookie、代理、重定向和正文大小限制。POST 带表单 Content-Type 和派生 Origin；301/302/303 后转 GET 并清空正文，307/308 保留 POST 正文。
- 验证：`form_navigation_uses_page_stealth_transport` 修改前失败，修改后 release nextest 通过。Cookie、Origin、302/303/307/308 重定向回归通过。官网登录 POST 已越过此前 403，OAuth 回调返回 200；后续站内回调 GET 500，尚未完成全部门禁。
- 采集限制：SDK 当前 request 回调是逻辑请求信息，尚不包括所有传输层补齐头，不能将缺失字段认定为线上未发送。初版采集器并发读取正文触发 runtime 有界队列关闭；串行读取后首页正常，143 个初始事件无正文读取错误。该采集问题不视为网站指纹故障。

### 隐藏子树伪元素阻塞原生点击

- 症状：ZG Cookie“同意”按钮可见，但原生输入返回 `INPUT_GEOMETRY_UNSUPPORTED`。按钮及祖先的 transform 均为 none。
- 原因：`PreparedRender::hit_test` 对整页绝对定位伪元素预检查时，仅检查宿主自身 display。`display:none` 祖先下的宿主没有布局盒，仍被当作不支持的几何，阻止与其无关的按钮点击。
- 修改：`crates/obscura-render/src/paint.rs` 沿渲染祖先链确认 display:none 子树，排除其中的绝对定位伪元素。保留实际覆盖、变换、裁剪等不支持几何的拒绝行为，不改用 JS 点击。
- 回归：`native_hit_ignores_pseudos_below_display_none_ancestor` 修改前失败；修改后独立 runtime 182/182 通过，包括已有伪元素覆盖拒绝测试。
- 第二处原因：页面标签装饰伪元素解析为百分比宽度，但包含块宽度为 0；绘制函数没有生成矩形，命中预检查却将其判为错误。空内容且无绘制矩形时与绘制函数保持一致；有文本但无法确定几何仍拒绝。
- 第二个回归 `native_hit_ignores_empty_auto_sized_positioned_pseudo` 修改前失败。修复后 runtime 183/183、render paint focused 592/592（另跳过 1 项）通过。
- 官网：主页 200、无错误弹窗；Cookie 原生点击成功并等待横幅隐藏，随后原生点击单程，截图确认选中状态。验证脚本将“单程”改为非完全匹配以包含“包括中转”子文本。未提交登录、乘客或支付。
- SDK macOS 19/19 通过；最终全量 render 1704 通过、4 失败、4 跳过。失败为资源并发限制、定时器等待、await 内资源加载和公共 DNS 校验用例，整体门禁仍未通过。指定 render/stealth release 构建通过；障碍课程均 32/33，observer-intersection 未通过。66 个确定性 fixture 完成双端截图，checker 有 5 项 Chrome 参考侧失败（字体行高、输入框/文本域固有尺寸），Obscura 侧检查通过；门禁未算通过。15 个网站顶部/底部对照分别有 11/12 个可比，其余捕获状态不稳定。两项修复前后确定性 PNG 完全一致。

### DOM 元素品牌与配置深拷贝

- 症状：ZG 首页和航线 API 返回 200，但出现“发生了错误”；控制台反复报告 Swiper `updateSize` 读取不存在的 `$el[0]`。
- 原因：HTML 元素缺少 `Symbol.toStringTag`，`Object.prototype.toString.call(div)` 返回 `[object Object]`。Swiper 将配置中的元素当作普通对象递归合并，再因合并函数跳过 HTMLElement 而把引用变成 `{}`，初始化无法挂载。
- 修改：`crates/obscura-js/js/bootstrap.js` 为 Element 及现有具体 HTML 接口原型添加不可枚举、可配置的类型标识。修复作用于通用 DOM，不包含航司判断，也不修改网站脚本。
- 回归：`element_brand_preserves_dom_references_in_config_merge` 复现同样的配置合并调用链，断言 div/span 品牌及元素引用保持不变。修改前实测失败，元素引用被替换。
- 诊断对照：相同代理、1280×900、有头 Chrome 152 未出现弹窗；Obscura 仅临时补类型标识后 Swiper 异常消失。正式引擎与 SDK 官网两次成功加载均无错误弹窗；主页、航线和推荐航班 API 为 200。首次尝试曾返回 403，未将该次无弹窗计为成功。
- 验证：DOM focused release nextest 510/510；相同最小配置合并用例在有头 Chrome 152.0.7977.83 上得到完全相同结果。独立 runtime 181/181、SDK macOS 配置 19/19。全量 render 1706 通过、2 失败、4 跳过，失败为定时器等待及公共 DNS 校验用例。render 与 render,stealth 指定 release 构建均通过。障碍课程两种模式均为 32/33，均未通过 `observer-intersection`，33/33 门禁仍未满足。
- 当时剩余问题（Cookie 点击已由上方命中测试修复解除）：原生点击 Cookie 同意按钮返回 `INPUT_GEOMETRY_UNSUPPORTED`；未移除几何检查，也未用 JS click 绕过。页面表单水平位置、跳转链接和文字布局与 Chrome 仍有差异，购票验收未完成。
- 范围：不在本次补齐全部 Web IDL 接口继承关系或浏览器指纹。

### stealth 传输统一为 primp

- 修改：新增 `obscura-net/src/stealth_client.rs`，Windows Chrome 145 与 macOS Chrome 152 共用 primp；移除 wreq、wreq-util 和 btls/BoringSSL 依赖。旧 `wreq_client` 模块名保留为源码兼容别名。
- 保留：显式代理、来源和 DNS 地址校验、页面网络观察、请求正文、压缩解码和响应大小限制。
- 行为：移除 Obscura 在连接重置后额外重发 GET 的逻辑；primp 自身的 HTTP/2 REFUSED_STREAM 协议恢复仍存在。
- 验证：两种配置各通过 SDK 19/19；独立 runtime 181/181；render 与 render,stealth release 构建通过。网络 focused 106/108，两个公共域名测试因解析到被禁止的 198.18.x.x 地址失败。全量 render 1705 通过、3 失败、4 跳过；障碍课程两种模式均 32/33，observer-intersection 未通过。完整门禁尚未满足。
- 限制：ALPS、trust anchor 扩展和子资源请求优先级/头顺序仍与有头 Chrome 有差异；不声称完整指纹一致。

### 页面身份与网络配置联动

- 修改：`obscura-browser/src/context.rs`、`obscura-js/src/runtime.rs` 及 `js/bootstrap.js` 将所选 profile 的 UA、平台、完整版本和架构传递给页面及框架，避免只替换请求 UA 而继续暴露旧页面身份。macOS 配置使用 Chrome 152；Windows 145 配置保留。
- 验证：两种 profile 的 SDK 测试及实际请求身份检查。完整浏览器一致性仍未达到，不能将 profile 名称当作验收结果。

### 自定义 DNS 解析器保持权威

- 原因：primp 默认 hosts 包装层可能绕过 Obscura 自定义解析器中的地址校验。
- 修改：`vendor/primp/src/async_impl/client.rs` 仅在没有显式解析器时安装 hosts 包装。显式解析器拥有最终解析权。
- 验证：向 hosts 缓存预置记录、安装拒绝解析器的回归测试，修改前失败，修改后通过。补丁说明见 `vendor/primp/OBSCURA_PATCHES.md`。

### 外部脚本与 CORS 预检使用一致传输

- 原因：主文档走 stealth，但经典外部脚本及 fetch/XHR 的 OPTIONS 预检仍走普通客户端，导致同一页面身份不一致、资源请求失败。
- 修改：`crates/obscura-browser/src/page.rs` 的外部脚本加载和 `crates/obscura-js/src/ops.rs` 的预检选择页面的 stealth 客户端。保留 URL 校验、预检不带 Cookie、不跟随重定向及 CORS 检查；重复 ACAO 不作为有效授权。
- 验证：ZG Nuxt 脚本及业务 API 恢复 200；SDK 用例验证预检无 Cookie、实际凭据请求携带 Cookie、重复 ACAO 拒绝。HTTP 成功不等同页面功能完成。

### RPC 文本超限不再破坏会话

- 原因：读取大段 DOM 文本可能超过 NDJSON 单行限制，导致传输失败及会话结束。
- 修改：独立 runtime 在返回前检查文本大小，返回明确 `VALUE_LIMIT`；断言成功仅返回确认结果，不回传大段被断言文本。
- 验证：SDK 70 KB 文本用例覆盖明确超限及会话继续使用。超限结果不伪装为完整文本。

### 控件之后的行内链接布局

- 修改：`crates/obscura-render/src/inline.rs` 与 `dom.rs` 修正控件后行内链接的几何处理，避免可见内容与定位/输入坐标不一致。
- 证据入口：`render-repros/inline-link-after-control.html`。此项来自此前 SDK 工作；本日志不追溯声称全量渲染验收通过。

### ZG 登录配置与来源范围补齐

- 原因：旧配置通过 `(ZG_ACCOUNT, ZG_PASSWORD) = (...)` 定义字面量，原加载器只识别单变量赋值；登录页面的脚本还来自独立 S3 域名。
- 修改：`examples/zg/run.py` 支持受限的字面量元组赋值，仍不执行旧模块；默认来源列表补齐实际观察到的 ZIPAIR OAuth 域名及登录静态资源域名。`flow.py` 根据菜单可见性打开登录入口，搜索入口适配官网的 link 角色。
- 验证：配置加载回归测试通过，包含旧模块内有顶层异常语句而不被执行的用例。官网已能显示登录表单；这不代表 Obscura 支付前验收通过。

### Patchright 有头 Chrome 对照进展

- 按要求将实际 Chrome 对照改为 Patchright 1.62.3，使用独立持久 profile、已安装 Chrome 152 和显式本机 7890 代理，不覆盖 UA 或请求头。操作使用 Locator 原生输入。
- 实测：登录 OAuth consent 成功；随后完成 KUL→NRT 单程与单成人选择，进入官网日期页。尚未完成指定日期、航班和支付前验收。
- 新证据：日历加载存在间歇性 Cloudflare OPTIONS 403，CDP Network.loadingFailed 明确报告 `PreflightInvalidStatus`。官网 Retry 能恢复部分月份；不能将此问题直接归因于 Obscura，也不能以 API 200 代替页面验收。
- 网络诊断仅启用 CDP Network 域，未显式启用 Runtime 域。后续 Obscura 对照需另行审计 Patchright 的运行时上下文处理，尚未声称完成对应修改。
- 本轮推进至 2027-02-05 的 Standard 日历报价 USD 349.39。选择日期后，`/v1/storages` 的 OPTIONS 三次返回 403，Chrome 报告 `PreflightMissingAllowOriginHeader`，仍停留在日历。ZG062 详情与支付前状态尚未验证。配置加载 focused 回归再次通过。

### 清理并复跑 Patchright 持久会话

- 关闭本任务 Chrome/Patchright 进程并确认旧专用 profile 没有残留进程；未触及日常 Chrome profile。
- 用新的专用目录 `profile-clean-20260915` 执行 `launch_persistent_context`，有头 Chrome、7890 代理。核对进程参数包含固定 `--user-data-dir` 且无 `--incognito`。先前运行也使用持久 context，此次额外隔离了旧 Cookie 和缓存。
- 新 profile 的 OAuth consent 再次通过，KUL→NRT 单程与单成人重新选择完成。仍复现保存航线后的系统错误和 waitingRoom OPTIONS 403；官网重新读取后恢复至日历，不能将匿名模式认定为此前根因。
- 最终结果：新 profile 恢复加载至 2027-02-05，但选择日期后的 storages OPTIONS 再次连续 403，未到乘客信息填报页。预检的 Origin 为 `https://www.zipair.net`，请求方法 POST，仅请求 `content-type` 头；UA 为实际 Chrome 152/macOS 默认值。未禁用 CORS、替换响应或修改官网脚本来跳过失败。

### 匿名 context 对照与请求留痕

- 按要求改为 Patchright `chromium.launch` 加 `new_context` 的非持久匿名 context，有头 Chrome 152、7890 代理，启用 `--disable-blink-features=AutomationControlled`。实测 navigator.webdriver=false，UA/platform 为 Chrome 152/macOS 默认值。
- 从官网导航前注册 context 请求观察，保存 URL、请求参数、正文、headers、响应状态、重定向关系、完成时序及失败事件。密码、Cookie、token 和证件字段脱敏。二进制正文采用 base64，已恢复早期解码失败的请求；检查时 737 个请求、无未恢复记录。
- 另保存 CDP Network 事件，包含预检与 CORS 失败；该记录在登录后启动，不声称包含此前登录阶段的所有预检。未显式启用 Runtime 域。输出保存在仓库外。
- 匿名模式登录成功并进入日期页，仍复现航线/storages/waitingRoom 的 OPTIONS 403。最后 waitingRoom 失败导致日历未加载，未完成乘客信息页或支付前验收。

## 2026-09-16

### 登录回调的同账号 Chrome 对照

- 使用旧 ZG 配置账号、本机 7890 代理，Obscura 再次完成身份提供方认证，但站内 `/zh-cn/idp_callback` 返回 500。错误页中的服务端请求记录指向 `PUT /v1/tokens` 返回 500。
- 同账号在 Patchright 有头 Chrome 中同样经过认证并在该站内回调返回 500，页面显示系统错误。现有证据不足以归因为 Obscura 独有问题，具体账号状态或后端根因仍未确认。
- 旧配置账号与此前成功 Chrome HAR 的账号不同，已请求确认登录账号。本轮未完成已登录支付前验收，未付款。仅记录诊断结果，未修改引擎或放宽校验；敏感证据保存在仓库外。

### 切换到已确认的 Chrome 账号后登录通过

- 用户明确选择此前 Chrome 成功登录的账号。仅使用该账号的登录凭据重新认证，没有复用 HAR 的 Cookie、access token 或 OAuth challenge。
- Obscura 重新登录后返回首页，菜单显示“我的页面”，后续 token API 返回 200；登录提交的账号标识与 Chrome HAR 一致。旧配置账号的 500 不再作为 Obscura 登录功能未通过的证据，具体账号/服务端根因仍未知。
- 已进入指定航班乘客页，保存的 24 个顶层乘客字段与 HAR 全部一致。登录态多出的已保存乘客选择框需要调整示例定位；国籍控件被固定页头遮挡时，显式滚动后可原生点击，自动滚动仍有待改善。一次 10 秒诊断覆盖出现传输超时并结束会话，恢复默认 60 秒后重新运行，未重放状态不明的输入。

### 已登录购票支付前验收通过

- 用户确认的 Chrome 原账号在 Obscura 中成功登录并完成指定 ZG062 单程购票至选择支付信息页，金额 USD 348.74。加载遮罩消失后保存支付页截图；未点击进入支付、未填卡、未付款。
- 24 个顶层乘客字段、5 个收据字段与 HAR 全部一致，金额复核与支付方式读取接口均返回 200。证据包含登录菜单、预订确认、收据、支付页及网络逻辑记录。
- 本次仅调整仓库外验证脚本的登录账号及实际页面定位，未修改引擎。人工辅助滚动和附加服务按钮定位仍存在；不代表无人干预示例或此前失败的全量门禁已通过。

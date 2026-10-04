# 上游 Chrome 一致性与指纹改进执行清单

更新时间：2026-10-04。上游审阅基线：`51b2601df64b9a72ce82cfb2f77ef1954b025fd8`。
本地已合并的选择性修复基线：`e55f8c7a6476a6c1b66df2ca7e84e5e5dba77a1d`。

本清单把上游 PR/issues 得出的十二项工作与已有指纹改进执行线合并。
指纹比较目标为 macOS Chrome 153，使用新建的匿名 Chrome BrowserContext，
先核对检测器源码及真实浏览器行为，再修改原生实现和验证。
各项仍未关闭；候选代码、回归通过与完整产品资格分别记录。
现有工程任务编号继续以 [开发 TODO](TODO.md) 为准。

## 已合并的上游修复

选择性合并共十一个提交，包含十处修复及适配回归：HTML tree sink 指针、解码缓冲、
URL setter、移除属性后的 ID 索引、MutationObserver attributeFilter、无变化的 style
写入、Response 验证与 factory、Teredo SSRF、非法 fetch/Continue rewrite 的 SSRF
校验，以及 iframe watchdog/容量边界。独立合并版本已通过完整 release nextest、
render/no-render 构建和 33/33 障碍课程。

## 执行队列

| 编号 | 优先级 | 上游依据 | 候选实现与剩余验收 |
| --- | --- | --- | --- |
| UC-01 | P0 | [Wasm streaming panic #1162](https://github.com/h4ckf0r0day/obscura/issues/1162)、[PR #1163](https://github.com/h4ckf0r0day/obscura/pull/1163) | 已实现私有 Response/Headers/body 状态及有界 streaming 编译路径，验证 MIME、状态、used/locked、伪造 receiver 和跨 realm Response；继续审计完整流语义及 native 异常边界。上游使用不同 deno_core 版本，不能直接套用其 panic 结论。 |
| UC-02 | P0 | [WebSocket #1038](https://github.com/h4ckf0r0day/obscura/issues/1038) | 原生 socket 候选覆盖握手、帧解析、收发、预算、关闭和 owner 退休；已修复空闲预算预留及 close reason 验证。完成 direct/proxy、frame/worker、取消和真实网络矩阵后关闭。 |
| UC-03 | P1 | [多 transport #977](https://github.com/h4ckf0r0day/obscura/issues/977)、[PR #981](https://github.com/h4ckf0r0day/obscura/pull/981) | 统一使用 persona 驱动的 primp。逐项核对导航、fetch/XHR、模块、worker、iframe、图像、CSS、WebSocket 的实际请求、代理、cookie、redirect 与 SSRF，保留性能分布。不能把单一路径的成功外推到全部入口。 |
| UC-04 | P1 | [UA/OS 一致性 #481](https://github.com/h4ckf0r0day/obscura/issues/481)、[CDP override PR #778](https://github.com/h4ckf0r0day/obscura/pull/778) | 不可变 persona、UA/CH/语言/时区继承和不支持的 CDP override 明确错误已有候选。完成 root、真实 frame、worker、网络头与 Chrome153 对照的联合矩阵，记录 patch version 等实际差异。 |
| UC-05 | P1 | [ICU locale #734](https://github.com/h4ckf0r0day/obscura/issues/734) | 已用 V8 ICU 的真实 `de-DE` 默认 locale 建立错配回归，避免只修改 LANG 的无效测试。继续核对各 realm 的 Intl 默认 locale、语言及 timezone 两种构建结果。 |
| UC-06 | P1 | [反射 #376](https://github.com/h4ckf0r0day/obscura/issues/376)、[native callable PR #607](https://github.com/h4ckf0r0day/obscura/pull/607) | 原生绑定候选需完整审计 descriptor、name/length、可构造性、receiver brand、borrowed realm、错误及 stack。新匿名 Chrome 对照确认本地缺少 [Option named constructor #1168](https://github.com/h4ckf0r0day/obscura/issues/1168)，需创建真实 HTMLOptionElement 并验证四个参数和 selection 状态。必须从实际能力产生结果；改写 native 字符串和 detector 分数不满足验收。 |
| UC-07 | P1 | [SDK/worker #851](https://github.com/h4ckf0r0day/obscura/issues/851)、[realm PR #979](https://github.com/h4ckf0r0day/obscura/pull/979) | 已合并 loaded-realm scope factory 和独立 context 回归。仍需同步创建真实初始 blank realm、三层即时嵌套、可信 creator/origin、原 Document 归属、容量/回滚、稳定 WindowProxy，以及首个安全同源导航和后续新 Window 的正确生命周期。 |
| UC-08 | P1 | [IndexedDB #1093](https://github.com/h4ckf0r0day/obscura/issues/1093) | 候选实现 context/origin 的原生已提交快照、FIFO lease、connection/versionchange ACK、阻塞顺序、rollback、clone、cursor/key range 和任务活跃边界。继续完善完整 IDL brand、非法 key 的同步异常及 opaque/sandbox origin；磁盘耐久性仍是单独工作。 |
| UC-09 | P1 | [Navigation Timing PR #1166](https://github.com/h4ckf0r0day/obscura/pull/1166)、[User Timing PR #1165](https://github.com/h4ckf0r0day/obscura/pull/1165) | 原生 Navigation/Resource/User Timing 候选已有真实时钟、顺序、detail clone 和有界缓冲回归。继续核对 TAO/CORS、失败/redirect、observer、跨 realm 和 document 退休矩阵，避免按检测器填常量。另已复现 [microtask exception PR #1174](https://github.com/h4ckf0r0day/obscura/pull/1174) 所述队列中断：本地抛错后只保留 before，Chrome 继续 error/after/promise；原 Document 归属、无借用重入及 CDP 事件出口需一起适配。 |
| UC-10 | P1 | [isTrusted #303](https://github.com/h4ckf0r0day/obscura/issues/303)、[activation PR #1088](https://github.com/h4ckf0r0day/obscura/pull/1088) | 已合并原生输入准入、队列、receipt 取消和 EventTarget 相位处理。render 保留原正向断言，no-render 明确拒绝并验证 document 未变。完整 focus/frame/origin/continuous Scheduling、borrowed receiver 和 activation 生命周期仍需完成。 |
| UC-11 | P1 | [移除虚假 GPU PR #733](https://github.com/h4ckf0r0day/obscura/pull/733) | 已合并真实 canvas、字体、音频、图像维度与原 Document 存储候选。继续限定软件 WebGL 的真实能力，核对系统字体/控件的实际差异，以及语音 provider 的持续刷新和生命周期。首个非空语音清单不应被人为延迟到默认声音查询结束。 |
| UC-12 | P2 | [TCP OS 特征 #987](https://github.com/h4ckf0r0day/obscura/issues/987)、[PR #1003](https://github.com/h4ckf0r0day/obscura/pull/1003)、[tracker #995](https://github.com/h4ckf0r0day/obscura/issues/995) | 明确宿主 TCP/IP 内核边界；tracker opt-in 与 Chrome 默认第三方请求行为分开验证。完成 cookie、local/session storage、IndexedDB 的 context/origin/reload 隔离及泄漏矩阵。DNT 设置不构成反追踪证明。 |

## 组合版本验证与关闭条件

本轮收尾统一原工作区、引用会话的已实现修复、source12 原生 HTML open
候选与最新输入访问器修复，形成 source13。输入测试 render 15/15、no-render
11/11，原生 HTML open/loaded realm 定向 render 24/24、no-render 18/18。
完整 render 2829 项通过、12 项跳过；七个产品 crate 完整 no-render 1715 项通过、
5 项跳过。两个精确 CLI release 构建及四份测试/CLI 产物联系审核完成，
障碍课程 33/33。679 文件冻结的当前内容与验证版本一致。
隔离输入版本的完整 no-render 1426 项通过、14 项失败、3 项跳过，保留原始
失败记录；这里的统一版本资格来自 source13 实际门禁。

与上一冻结 source12 的交错性能复测共 140 个原始样本，四框架 wall 中位数
变化为 +0.09% 至 +0.68%，点击整次 CLI 为 -0.51%，RSS 变化均小于 1%。
这些变化处于约 10% 噪声范围，不作为通用性能提升声明。新建匿名 Chrome153
对照的 HTML open 行为及 callable metadata 一致，仅覆盖该局部接口。

66 个确定性 fixture 中 Obscura 行为断言 248 项通过、零失败，Chrome 244 项
通过、4 项失败。整套 exit 1 保留，失败为 serif normal 行高及 input/textarea
尺寸控制，不豁免为通过。真实站点顶部、底部各完成 15 个站点的三引擎捕获，
每侧仅 11 个站点具备有效 fidelity 比较条件；平图及捕获状态不一致继续排除。
实际检查当前 MDN 顶部和 React 底部六张图，新旧 Obscura 两组字节一致；
相对 Chrome 的菜单、图标、固定头部及文字流差异仍开放。
Astra medium 最终结论为 ACCEPT_BOUNDED_CURRENT_BATCH，接受本轮已验证统一工程批次的有界合并；未发现本次审核范围内的剩余合并阻断。
十二项任务与公开指纹目标均未据此关闭。

上一冻结 source12 候选定向 render 22/22、no-render 18/18，完整 render 2814/2814、十二项跳过，
七个产品 crate 完整 no-render 1704/1704、五项跳过。两个精确 CLI release
构建及各测试/CLI 的选定 build script、snapshot、原生 helper 关系已核对。
性能修复包括：原生 capability 创建时从实际 HTML 节点确定不可变
接口编号，私有 slot 仅缓存该编号用于品牌检查，避免每次额外生成和解析 DOM
snapshot。布尔 getter 直接从原生 DOM 读取属性，保留原 Document、节点代次、HTML namespace 和
tag 校验，避免完整 JSON 状态解析。原生只返回私有状态码，失效 TypeError 由被调用
accessor 的 semantic 在自己的 realm 创建；root/child 双向借用错误 prototype 回归已通过。
新增实际复用 NodeId
回归，确保旧接口不能读取或写入替代节点。新预检二进制与源码、选定 build script、snapshot、原生 helper 的关系已核对。
相对独立品牌 JS accessor 版，同 fixture 的读取循环 -88.70%、写入/回读/属性验证
循环 -16.52%，整次 CLI -18.53%/-8.57%；四框架 wall 中位数 -0.15% 至 -0.01%、
RSS -0.10% 至 +0.16%，在噪声内。该批完整本地门禁通过，新一轮 obstacle 33/33；top/bottom 三引擎各十五站捕获
成功，其中十/十一组符合图像比较条件。六十六 fixtures 中 Obscura 248 项通过、
Chrome 244 项通过和四项字体/控件几何断言失败，整套仍 exit 1。此次 MDN 顶部
与 React 底部各三张图片已检查；candidate/基线图像相同，Chrome 对照仍有菜单、
SVG 图标及 sticky 文字遮挡差异。
预检与完整构建的 render CLI 字节一致。新匿名 Chrome153 的属性行为及 getter/setter
name/length 对照再次通过；上述局部结果不作为公开指纹评分或完整资格。

前一 HTML open 原生入口候选已删除 Element.prototype 的泛用 accessor，Details
和 Dialog 分别拥有自己的私有品牌与原 Document 属性反射。两个访问器使用真实
V8 API callback，宿主证据同时核对 script_id 和原生函数源码；没有通过重写
Function.prototype.toString 来取得结果。root 与已加载 child realm 在作者脚本前
各安装一次，重复初始化保留作者改写，borrowed/retired receiver 仍绑定原 Document。
新建匿名 Chrome153 对照的接口曝光、属性状态、receiver 错误、布尔转换，以及
getter/setter 的 name/length 均逐项相同。
本批 obscura-js 定向 render 20/20，完整 render nextest 2812/2812、十二项跳过；
七个产品 crate 的完整 no-render 1702/1702、五项跳过，两个精确 CLI release
构建和选定构建脚本/snapshot/原生 helper 关系均已核对。该冻结版 obstacle 33/33。
广泛渲染已返回：top/bottom 三引擎各十五站捕获成功，但仅十一/十页具备比较条件；
确定性 fixtures 整套 exit1，Chrome 的四个行高/表单几何断言失败未豁免。
四框架交错测量 wall 中位数 +0.32% 至 +4.95%、RSS ±0.25% 在噪声内；实际 open
读取循环 +113.19%、写入/回读/属性验证循环 +54.86%，wall +23.74%/+29.61%，
超过性能边界。该版不满足性能资格，保留失败数据，未在 main 激活。
Astra medium 对原生安装边界的源码审查未发现新的阻断回归，完整证据审核仍待执行。

前一独立品牌候选完整 render 2810/2810、十二项跳过，完整 no-render 1700/1700、
五项跳过，定向 render 18/18、no-render 14/14，两个 CLI release 构建均已通过。
该版行为对照一致，但访问器仍是 JS 函数，name 与 Chrome 有差异；本批原生入口
修复这一差异。前一版没有执行独立 obstacle 或广泛渲染资格，不移用更早结果。

微任务 reporter 新增五组实际行为对照：Chrome 不调用作者定义的 Error
name/message/cause/stack getter，本地默认 formatter 会调用它们并修改 DOM。
Chrome 在原值 false/null/Symbol 抛出、error listener 抛错和作者 console.error
改写时仍继续后续 microtask/Promise/timer，并保留 ErrorEvent.error 原值。
错误出口应使用 V8 原生 Message 元数据，避免主动读取这些作者属性。
JS catch 后重建的原生 Message 无法可靠保留 primitive 的原始抛出位置，需要在
原生抛出边界保留消息；不会用当前 URL 或重建栈填补来源。该修复尚未实施。

前一 HTML 接口/dialog 冻结版已通过 render release nextest 2807/2807、十二项跳过，
七个产品 crate 的 no-render 完整 nextest 1698/1698、五项跳过，以及两个精确 CLI
release 构建。定向 render 18/18、no-render 12/12；源码、选定 build script、snapshot、
测试二进制、冻结 CLI 和原生语音 helper 的字节关系均已核对。
四组新匿名 Chrome153 最小对照逐项一致：HTMLElement popover 接口归属/反射/brand、
HTMLDialogElement open 反射/brand、dialog UA 布局，以及 popover 开关状态布局。
dialog open 的动态变更会使子树样式重算，准备好的缓存仍满足 none/block/none。
该版的 windows_chrome145 障碍课程已通过 33/33。确定性 fixture 和真实站点
top/bottom 验证均已返回，下文保留整套失败和不具备比较条件的范围。
最小接口对照的范围有限：另一个 Chrome 对照发现 Element.prototype 仍错误暴露
open，导致 div/SVG 也继承它；HTMLDetailsElement 尚未拥有自己的 open。这一旧版差异已由上述独立品牌和原生入口候选修复，完整资格仍需核对。

前一 HTML 接口/dialog 版相对上一冻结 popover 版的四组本地交错测量各十二轮、两轮 warmup，
共一百一十二次执行，原 eval+PNG capture 边界保持一致。
静态、React、Vue、五千行 DOM 的 wall-time 中位数分别增加 0.44%、0.82%、0.38%、0.39%；
最大 RSS 变化分别为 -0.26%、+0.11%、-0.01%、+0.70%，均在约 10% 噪声界限内。
这只覆盖同步本地样本；零 settle 测量不作为 fidelity 或指纹评分证据。

上一轮冻结的 popover 候选已通过 render release nextest 2802/2802、十二项跳过，
以及七个产品 crate 的 no-render 完整 nextest 1695/1695、五项跳过。
两个精确 CLI release 构建及项目要求的 windows_chrome145 障碍课程 33/33 通过。
源码冻结、选定 snapshot、测试二进制、CLI 和原生语音 helper 的产物关系已核对。
render CLI 与性能产物对应前一冻结记录，后一冻结仅改变 nextest 调度配置，
两者产品源码逐文件相同。
这些结果只覆盖相应源码、feature 和测试配置，不证明公开指纹评分达到目标。

nextest 为三个精确的短墙钟测试预留全部运行槽：模块图与求值共享预算、
排队模块不消耗等待 deferred script 的预算、语音非零退出保留真实 snapshot。
原预算、结果断言和零重试保持不变。修改调度前的失败记录保留；排队模块失败的
具体阶段仍未建立，不将预留运行槽描述为已修复的产品竞态。

六十六个确定性渲染 fixture 已采集，Obscura 的 248 项行为断言通过。
Mac Chrome153 对固定字体和控件尺寸 oracle 有四项失败，整套检查仍未通过；
原始断言和失败记录保留。字体输入和控件平台需继续对齐。
历史 interface/dialog 冻结版的真实站点顶部和底部各十五组已返回。该轮顶部 Chrome 在 Porkbun 的
load 导航等待超过 50 秒，未产生该项截图，整组返回失败；Obscura 和基线各十五项
抓取成功。底部三种浏览器各十五项抓取成功。按就绪、非空内容与截图边界，
顶部十组、底部十一组可用于成对图像诊断，其余保留但不作 fidelity 结论。
Remix 图像缺少可比较内容，Bulma、Brave 和 Termius 的 Chrome 截图边界不稳定。
此轮两侧采用 macOS153
身份、相同 viewport、settle 和截图边界；Chrome 实际版本与配置的 CH patch
version 差异明确记录，不能当作公开指纹评分证明。
该历史轮次视觉检查确认 MDN 关闭的搜索 dialog/input 和 Exit search 已隐藏，
前一基线仍显示这些内容。通用 dialog UA 修复和最小 fixture 支持此局部改善。
MDN 的主题/语言菜单可见及 SVG 图标缺失、React sticky 导航上的文本遮挡仍然存在。
MDN 的实时广告内容也不同，不能将广告区域的像素差异归因于实现。

新 popover 与先前通过组合门禁的候选执行 old/new 交错测量，各十二轮、两轮
warmup，覆盖本地静态、React、Vue 和五千行 DOM 构造 fixture。保持 persona、
viewport、网络及零 settle 的 eval+PNG capture 边界一致。
wall-time 中位数分别增加 0.07%、0.11%、0.21%、2.84%，OS 最大 RSS 增加
0.42%、0.26%、0.51%、4.56%，均在项目约 10% 的噪声界限内。
这只覆盖同步本地样本，不证明真实站点速度或公开指纹评分；RSS 是单进程
最大值，不能当作整个进程树的资源总量。

popover 使用原生开关状态、选择器和 UA cascade，关闭时的布局、作者 display
覆盖及 hidden dialog 行为已与新匿名 Chrome153 的最小对照一致。
后续 HTML 接口归属与私有属性反射修改已通过上文当前版门禁及四组最小 Chrome 对照；
当前版的广泛验证结果已在上文记录，仍存在未通过的整套门禁和视觉差异。
receiver 使用原 Document、原 arena generation 的私有 capability；原始事件身份
不受公开 nid、dispatchEvent 或 ToggleEvent 构造器改写影响。
文档切换会淘汰旧缓存，保留的旧 receiver 不会重绑到新 DOM。
最小对照通过仍未解决 MDN 的实时菜单差距，完整 top-layer、auto/light-dismiss
和 toggle 合并尚未完成。

原生 blank realm 的准备工作新增真实 Chrome153 生命周期对照：插入 iframe 前
设置 src，首个同源加载保留初始 Window 的 Array 和全局变量，同时创建新 Document；
先插入空 iframe 再设置 src，则后续加载更换 Array 并清除全局变量。
两种路径都保留 WindowProxy，旧 Document.defaultView 为 null。
这些仅是 Chrome 的行为证据，同步独立 blank realm 和对应生命周期尚未实现。

Astra medium 已对 owner、生命周期、输入取消、realm factory、speech posted task、
frame timer/unref，以及 popover 原身份、缓存退休和 SVG/HTML 缓存碰撞做只读审查。
修复其发现后，定向复审未发现新的阻断问题。该结论不替代十二项任务完成后的
完整候选审核。
Astra medium 也审核了 dialog open 缓存依赖修复，以及同步 blank 的原身份、准入回滚、
retained contexts 配额和 op2 自动借用前置条件；这些设计审核不算 blank 实现完成。
上游 PR #1174 的只读适配审核保留错误格式化重入、listener 抛错、原 owner 和
child Runtime 队列出口问题，仍未实施该修复。
最终关闭仍需有效的必需门禁、新匿名 Chrome 对照和覆盖范围内的公开指纹结果。

## 上游再次更新

后续复查发现上游 HEAD 已到 `8a7914d`，相对最初检查的 `51b2601` 新增五个
包含合并的提交。[输入访问器 PR #1088](https://github.com/h4ckf0r0day/obscura/pull/1088)
和[微任务 PR #1174](https://github.com/h4ckf0r0day/obscura/pull/1174) 已进入 main，
分别继续归入 UC-10 与 UC-09。输入修复已按本地私有 receiver 路由适配并进入 source13 验证。PR #1174 仍读取作者 Error
message，并使用 JsError formatter；这些读取的副作用、原值 false/null、错误来源
和跨 realm 归属需要既有 Chrome 控制验证。当前候选的原生 input 路径与 #1088
的公开 helper/nid 路径不同，需要按原 capability 验证相同的实际输入行为。

再次检查输入修复时，Astra medium 指出直接改用调用者 realm 的 helper 会新增
borrowed click 错写。实际 loaded iframe 与 root 使用相同 NodeId 的最小对照确认：
既有路径只改变 receiver 所属 child 文档，Chrome 相同。选择性适配需保留该行为，
并直接验证内部 getter/setter 均不调用作者访问器。原 realm 私有路由及取消回滚回归已进入 source13，并通过本轮两种构建的实际验证。

Astra medium 已核对最新原生 open 冻结版的源码、两种 CLI、四份 artifact audit、
168 个原始性能样本及相同 fixture 输入；本次局部证据链可接受。
公开文档和历史 ledger 的过期状态已修正。该审核未关闭广泛渲染 parity、公开
scanner 或十二项 TODO；前一原生版本的性能拒绝数据继续保留。

最新冻结版还执行了 TLS 浏览器导航、Fingerprint Scan 与 Rebrowser 观察轮。
两种浏览器各三个适配器都取得完整结构化结果；Fingerprint Scan 本次候选为 83，
headless Chrome 为 100。两侧语言、默认 viewport、UA/headless 与版本 patch 输入
尚未匹配，故比较为 INCONCLUSIVE。Rebrowser 的四个未触发动作行不计通过。
本轮未执行完整 quick/core、human 交互及追踪隔离，不用该分数关闭任何任务。
引用会话最近两轮内容已重新读取；线程运行状态 notLoaded 不表示历史内容缺失。
其同步 blank realm、语音服务生命周期和字体时间异常继续并入既有执行队列。

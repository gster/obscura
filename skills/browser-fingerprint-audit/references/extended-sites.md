# 扩展站点与本地项目

与 core-sites.md 合计 22 项。核对日期：2026-09-30。
这些步骤是 agent 的操作手册，不表示附带脚本已实现所有站点适配。
对没有公开程序接口的网页，先查看实时 UI 再用 DOM/视觉提取；不猜选择器。

## 10. BrowserScan

入口：<https://www.browserscan.net/>

打开后等待实际 IP、Browser、Platform 等字段产生；初始的“100%”不作最终判断。
保存真实性评分及具体异常、UA/Client Hints、GPU/Canvas、语言/时区、网络分组。
从站内 Utilities 的 **Kernel detection / Client Hints / Bot Detection /
Time zone detection / WebGPU Browser Report / HTTP2/SSL/TLS Test** 进入需要的专项。
记录每个实际落地 URL，不猜路由。未跑专项就不声称覆盖它。
不点 Port Scanner、2FA 工具、Cookie Converter 或媒体权限按钮。

采集：各分组 DOM、异常详情、截图。网络和画像问题分开；没有稳定 JSON API 承诺。
来源：该官方主页的工具目录与结果模板。

## 11. Incolumitas

入口：<https://bot.incolumitas.com/>

静态轮：读取 New/Old Detection Tests、HTTP Headers、TLS、Worker、Service Worker
等需要的分组。行为轮另开标签，按页面公开的 **Bot Challenge**：填写虚构测试名称和
`audit@example.invalid` 等非真实邮箱，选取当前表单选项，提交，处理该测试的确认对话框，
按页面要求更新测试购物篮并读取结果。不得在其它业务网站照搬自动确认。

保存 `behavioralClassificationScore` 的显示值、采样时刻与完整动作轨迹；
当前页说明最后一轮定时更新在加载约 15 秒后，故不能只取最早值。
分数方向是低值偏 Bot，与很多风险分方向相反；不与其它网站分数相加。
没有完成挑战时标明“仅静态/被动观测”。本次页头仍显示 v0.6.3（2024-06-06），
执行时保存真实显示版本，不把它表述为当年的最新规则。
来源：官方页的 Behavioral Classification、Bot Challenge 和版本标识。

## 12. APIVoid Bot Detection Test

入口：<https://www.apivoid.com/tools/bot-detection-test/>

等待 `Collecting browser signals…` 结束、Risk Score 成为实际数值，
且 Detection Summary、Collected Browser Data 有内容。
读取实际代码区数据和风险分类，保存原始 JSON（可解析时）与界面结果。
品牌提供其它 API 不等于此免费测试有已授权的 API；不申请账户、不购买服务、不猜接口。
当前说明包含 IP 信誉/匿名网络信号，因此分类时必须拆分网络与浏览器异常。
来源：该页 FAQ 与结果区。

## 13. BotD（本地客户端库，不是浏览器诊断网站）

仓库：<https://github.com/fingerprintjs/BotD>

仅在已有或获授权创建的自有测试 fixture 中使用。先读取所固定版本的 README、
lockfile 与构建配置；使用 `@fingerprintjs/botd` 的 `load()` 后调用实例 `detect()`，
保存完整返回值和初始化错误。不在外部检测页面临时注入该库，不借此改变其它测试。
可在自有 fixture 内采用以下包装（结果对象是**自建的**，不是 BotD 公共全局）：

```javascript
import { load } from '@fingerprintjs/botd';
try {
  const instance = await load();
  window.__AUDIT_BOTD_RESULT = { status: 'complete', data: instance.detect() };
} catch (error) {
  window.__AUDIT_BOTD_RESULT = { status: 'error', message: String(error) };
}
```

浏览器加载已构建 fixture 后等待这个自建状态对象，再读数据。
固定依赖版本，不用远端 `latest` CDN 引用冒充可复现 CI。记录库版本、fixture SHA、
初始化成功与否和实际检测输出。缺少 fixture 时标 NOT_RUN，不把 GitHub README 当实测。

## 14. AmIUnique

入口：<https://amiunique.org/>

这是可识别性研究，不是 Bot 门禁。页面声明点击 `See My Fingerprint` 会采集指纹并设置
研究 Cookie；仅用专用测试 profile，按本轮许可进入。
点击后等唯一性/相似度统计和属性列表产生，保存样本说明、统计口径、采样时间及属性。
重访使用站内 History（如可用），不安装扩展或移动 App。
统计只代表该项目样本，访问次数或 Cookie 连续性不等于证明无 Cookie 追踪。
来源：官方首页及其 Fingerprint/History 入口。

## 15. EFF Cover Your Tracks

入口：<https://coveryourtracks.eff.org/>

保持测试选项在基线/候选一致。默认不额外选择 `Test with a real tracking company`，
然后点击 `Test Your Browser`。等待最终报告，保存广告追踪阻断、不可见追踪阻断、
指纹可识别性和展开后的属性明细。

测试可能经过多个追踪模拟域；这些是测试本身，不应用请求拦截器人为放行/屏蔽以改变
结果。保存实际加载失败和既有隐私设置。请求失败本身不能一律算成功阻断。
“指纹独特”与“Bot”不是同一概念；不要把唯一性样本统计设成 Chrome 兼容性门禁。
来源：官方入口及 <https://coveryourtracks.eff.org/about>。

## 16. TorZillaPrint / TZP

目录：<https://arkenfox.github.io/TZP/>
主测试：<https://arkenfox.github.io/TZP/tzp.html>

主测试运行后按需读取 region、fonts、canvas、elements/DOMRect、screen、workers 等分组；
展开 `[+]`/详情，保存每项值与错误。目录页只是链接集合，打开目录不算完成测试。
若使用 `[re-run]` 复测，记录是哪一个分组，不能把局部重跑当整站新运行。
可从目录进入 iframe、Canvas noise、Intl 等专项，逐项记录落地 URL。

该项目有 Gecko/RFP 专项；Firefox 专属字段缺失对 Chromium 不自动构成缺陷。
带 `run` 的全屏、弹窗、权限等交互不默认执行。
来源：官方目录与主测试页。

## 17. Fingerprint 商业 Demo

入口：<https://fingerprint.com/demo/>

等待**真实浏览器结果区域**的 Visitor ID 和指标加载完成；必要时点击当前
`Analyze my browser again`。保存 Visitor ID、置信度、访问历史、浏览器/IP，以及
页面实际公开的 `JavaScript Agent Response` / `Server API Response`。
读取代码块而不索取账户/API key。不抓取下方营销演示中的固定 Suspect Score 当本轮结果。

依 reporting.md 分别做同 profile 重访、新建无痕 context、新 profile、单独换出口的对照。
关闭再打开标签页不是新 profile；清 Cookie 也不等于清理全部状态。
只报告本次是否返回同一 ID，不保证它在其它环境、时间或账户上必然如此。
不点击演示登录、付款、试用注册，不真实提交任何业务信息。
来源：官方 Demo 的真实结果区与示例场景区。

## 18. IPLeak

入口：<https://ipleak.net/>

等待 IP、WebRTC 与 DNS 分组完成，分别保存浏览器 HTTP 出口、WebRTC 候选与 resolver。
`DNS detection - Pending` 或 JavaScript required 不能当测试完成。
必要时从页面官方 IPv4/IPv6 入口检查两条链路，逐项记录是否可达。
不激活 Torrent 检测或浏览器精确位置地图。

地址判断以事先记录的允许出口/解析器为准。出现 ISP/地区标签只是辅助信息；
不能只因某个 resolver 地理位置不同就认定公网 IP 泄漏。
来源：官方页面的 IP、WebRTC、DNS 与 IPv4/IPv6 入口。

## 19. DNSLeakTest

入口：<https://dnsleaktest.com/>

核对当前按钮后运行 **Standard test**；怀疑多 resolver 或结果不完整时另运行
**Extended test**。等待各轮结束和最终解析器表，保存 resolver IP、hostname、ISP、
country 和运行模式。没有发现 resolver 但测试请求失败时为 PARTIAL，而不是“无泄漏”。
不替换成 agent 机器的 `nslookup`；那是不同测量路径。

只与预期 DNS 策略比对，网站看到的递归解析器本身不直接证明到它的链路是否加密。
加密/绕隧道等根因需要浏览器/系统配置或授权网络抓包佐证。
来源：官方入口与 <https://dnsleaktest.com/what-is-the-difference.html>。

## 20. Whoer

入口：<https://whoer.net/>

等真实出口/环境分组加载，展开匿名度扣分详情；保存 IP/ASN、DNS、代理/Tor/黑名单标签、
语言/时区、请求头和实际扣分项。不要读取静态说明区的 `-10%` 就拼出一个总分。
需要 DNS 专项时走站内 DNS leak test，记录最终 URL。
不触发端口扫描、测速或购买 VPN；不照搬 Flash/ActiveX 等旧提示作为现代 Chrome 标准。
采集为 DOM/截图，没有已确认的免费程序接口。
来源：官方主页及站内工具目录。

## 21. BrowserAudit

入口：<https://browseraudit.com/>

进入设置，记录选中的测试类别，显示模式选择能展示单项结果的 Full，
默认将 Test result reporting 设为 Off。然后点击 `Test me` 并等待完成。
大型测试按主流程允许记录后提高超时，但不能无限等待。
保存每类通过、警告、失败、跳过的数量，以及所有失败项的名称和说明。
若某类依赖外部资源失败，保留网络证据再判断，不能直接归为安全实现错误。

该站检查安全标准/特性，不是 Bot 评分工具。不得为“通过”而关闭 CSP、同源限制
或安全隔离。未完成的项目必须计入覆盖缺口。
来源：官方主页、设置及其源码链接。

## 22. PrivacyTests.org（源码驱动的测试项目）

说明：<https://privacytests.org/about.html>

公开网站主要展示已有浏览器结果；打开该表格**不构成对当前候选的测试**。
从 About 的官方源码链接进入，读取当前 README/测试运行文档，固定 commit 和依赖。
仅在用户批准的测试环境中配置候选浏览器适配器和需要的站点/域名/证书，
执行该版本实际支持的命令，保留测试名、输出、版本和运行环境。

没有可用适配器、域名或 fixture 时标 `NOT_RUN`/`UNSUPPORTED` 并列缺口；
本包不虚构通用 `npx privacytests --browser ...` 命令。
自托管跨站测试必须保留所需的独立 origin/site 关系；把所有页面放在一个 localhost
路径下不能验证跨站隔离。源码移植/本地执行结果与线上商业识别效果分开报告。

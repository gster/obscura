# 来源、接口可信度与维护

核对日期：**2026-09-30**。

项目导入复核：重新核对 Fingerprint Scan 文档、Rebrowser 的公开 `index.js` 和
Playwright Python CDP 文档。其余逐站入口与 UI 步骤来自原包手册，未在本次安装中
逐站执行；运行时仍须查看真实 UI。tls.peet.ws 的文档访问本轮未成功，不把 URL
可见或本地 JSON fixture 当成线上协议实测。

核对范围是官方入口、页面结构/文字、公开 API 文档与所引用的源码，
不是“已用用户的候选浏览器跑通这些网站”。公开站点会更新，执行时仍需核对。
本包没有复制第三方检测器源码，也没有加入任何检测绕过补丁。

## 程序适配契约

| 组件 | 依据 | 本包采用的契约 |
|---|---|---|
| Agent Skills | <https://agentskills.io/specification> | 目录包含带 name/description 的 SKILL.md，可放 scripts/references/assets。 |
| Fingerprint Scan | <https://fingerprint-scan.com/docs> | `window.FINGERPRINT_SCAN`；成功状态 `COLLECT_SUCCESSFUL`。 |
| Rebrowser 在线说明 | <https://bot-detector.rebrowser.net/> | 页面列出的 dummyFn、exposeFunction、sourceUrl、main-world 测试动作。 |
| Rebrowser 源码 | <https://github.com/rebrowser/rebrowser-bot-detector> | 结果行、rating 和 DOM 输出实现。 |
| Rebrowser 具体文件 | <https://raw.githubusercontent.com/rebrowser/rebrowser-bot-detector/main/index.js> | `#detections-json.value`；0 未触发/无法判定等语义。main 分支未固定为发布 SHA，执行时应记录版本。 |
| tls.peet.ws | <https://tls.peet.ws/> | `/api/all` 获取完整请求结果；由被测浏览器请求。 |
| CreepJS | <https://github.com/abrahamjuliot/creepjs> | README 列出 `window.Fingerprint`、`window.Creep`；不在本包承诺 schema 稳定。 |
| Playwright CDP | <https://playwright.dev/python/docs/api/class-browsertype#browser-type-connect-over-cdp> | 连接已有 Chromium/CDP；no_defaults 从 v1.60 提供，仅影响已有默认 context 的覆盖。 |
| Playwright context | <https://playwright.dev/python/docs/api/class-browser#browser-new-context> | 每轮新建匿名 context，关闭时仅清理本轮拥有的 context。 |
| Codex 项目 skills | <https://learn.chatgpt.com/docs/build-skills> | 项目 `.agents/skills` 支持符号链接，源码由项目 Git 管理。 |
| Playwright Response | <https://playwright.dev/python/docs/api/class-response#response-body> | 读取已发生的浏览器请求响应体，而不是另发 HTTP 客户端请求。 |
| Playwright 生命周期 | <https://playwright.dev/python/docs/api/class-playwright#playwright-stop> | 停止客户端，不调用外部 browser.close/context.close。 |
| CDP 隔离执行 | <https://chromedevtools.github.io/devtools-protocol/tot/Page/#method-createIsolatedWorld> | 为诊断创建 isolated world；以运行时协议响应为准。 |
| BotD | <https://github.com/fingerprintjs/BotD> | 包导出 load()，实例 detect()；用于自有 fixture。 |

“源码可读”与“有稳定公共 API”分开处理。Rebrowser 的 DOM 是已核对实现，
不是服务方版本化 API 保证；CreepJS 对象是 README 指定入口；其它网页不推测 API。
没有稳定契约时，运行中出现变化应保存现场并使用 DOM/视觉方式或明确未覆盖。

## 站点操作来源

核心：

- BrowserLeaks：<https://browserleaks.com/>，以及 core-sites.md 中直接链接的各分项。
- CreepJS：<https://abrahamjuliot.github.io/creepjs/>，及其 Tests 下的 Workers/Iframes。
- DeviceAndBrowserInfo：<https://deviceandbrowserinfo.com/are_you_a_bot>、<https://deviceandbrowserinfo.com/are_you_a_bot_interactions>。
- Fingerprint Scan：<https://fingerprint-scan.com/>、<https://fingerprint-scan.com/docs>。
- Rebrowser：<https://bot-detector.rebrowser.net/> 与上表源码。
- TrackMe：<https://tls.peet.ws/>。
- Sannysoft：<https://bot.sannysoft.com/>。
- Pixelscan：<https://pixelscan.net/>、<https://pixelscan.net/fingerprint-check>。
- IPhey：<https://iphey.com/>。

扩展：

- BrowserScan：<https://www.browserscan.net/>。
- Incolumitas：<https://bot.incolumitas.com/>。
- APIVoid：<https://www.apivoid.com/tools/bot-detection-test/>。
- BotD：<https://github.com/fingerprintjs/BotD>。
- AmIUnique：<https://amiunique.org/>。
- EFF Cover Your Tracks：<https://coveryourtracks.eff.org/>、<https://coveryourtracks.eff.org/about>。
- TZP：<https://arkenfox.github.io/TZP/>、<https://arkenfox.github.io/TZP/tzp.html>。
- Fingerprint 商业 Demo：<https://fingerprint.com/demo/>。
- IPLeak：<https://ipleak.net/>。
- DNSLeakTest：<https://dnsleaktest.com/>、<https://dnsleaktest.com/what-is-the-difference.html>。
- Whoer：<https://whoer.net/>。
- BrowserAudit：<https://browseraudit.com/>。
- PrivacyTests：<https://privacytests.org/about.html>，源码入口从该官方页面获取。

## 更新规则

每轮保存实际时间、最终 URL、可获得的页面版本/ETag/Last-Modified/源码 SHA。
collect.py 保存导航文档 SHA256，但这不能代表全部动态 JS 的版本；需要精确锁定时
另外记录已获授权的资源清单或使用固定版本的自有 fixture。

站点对象/DOM 变动：先保留旧结果，再更新适配器与单元测试，并对基线/候选重跑。
不得修改 raw.json、放宽判断或把缺失字段填 false 来掩盖接口变化。

未能核对真实就绪条件或无法读取页面脚本：将该集成标为 DOM/人工确认，
不能为了“全自动”而编造一个公共 JSON API。自托管结果不等同于线上服务的后端风控。

# 核心站点：入口、触发、就绪与取证

核对日期：2026-09-30。这里的片段假设 `page` 已连接到**待测浏览器**，
使用官方 Playwright Python 异步 API。其它 agent 浏览器工具按相同动作执行。
片段只是站点操作示例；超时、异常、取证和资源清理由主流程或采集器负责。
字段表示“需要采集什么”，不是保证网站永远保留同名字段。

## 1. BrowserLeaks

官方入口：<https://browserleaks.com/>

| 页面 | 直接地址 | 本轮应保存 |
|---|---|---|
| JavaScript | <https://browserleaks.com/javascript> | UA、platform、CPU/内存、语言/时区、screen/viewport、plugins/MIME、webdriver、API 状态。 |
| Client Hints | <https://browserleaks.com/client-hints> | 页面显示的 HTTP 请求头与 JS Client Hints，分别保存，不能合并冒充同一来源。 |
| Canvas | <https://browserleaks.com/canvas> | 支持状态、绘制/读回结果、hash；不只存 hash。 |
| WebGL | <https://browserleaks.com/webgl> | WebGL 版本、vendor/renderer、参数、扩展及失败原因。 |
| Fonts | <https://browserleaks.com/fonts> | 字体列表、检测数量、测量/hash；只与相同字体环境比较。 |
| WebRTC | <https://browserleaks.com/webrtc> | 支持状态、候选地址/类型、STUN 结果、站点显示的泄漏判断。 |
| TLS | <https://browserleaks.com/tls> | 协商协议、ClientHello 参数、JA3/JA4 等实际出现字段。 |
| HTTP/2 | <https://browserleaks.com/http2> | 是否协商 h2、SETTINGS/帧与指纹；未协商不等于“不支持 HTTP/2”。 |
| DNS | <https://browserleaks.com/dns> | 是否完成、实际出现的 resolver 地址/组织。 |
| WebGPU | <https://browserleaks.com/webgpu> | 可用性、adapter/features/limits 或错误。 |
| QUIC | <https://browserleaks.com/quic> | 握手/HTTP3 结果；代理或网络不支持时单独标记。 |

**执行**：按选定页面分别新开标签，等目标表格从空白变成真实数据。
JS 页当前有 `window` / `iframe.contentWindow` 按钮；保存 window 结果后，
用当前页面的对应按钮切换并保存 iframe 结果。按钮失效则标未覆盖，不自行写入替代值。
某个 API 显示明确错误可以是有效观测；整页脚本未执行不是全部 API 缺失。

**提取**：没有在本包中确认通用 JSON API。用 DOM/可访问性树定位具体表格，
按“字段名、值、上下文”保存。要保留输入框中的值，而不只抓 `body.innerText`。
不运行 Flash/Java/Silverlight 等不在桌面现代浏览器范围的旧测试，不点全屏、
位置、媒体授权等额外测试，除非本轮有明确要求。

来源：[页面目录](https://browserleaks.com/)及上表各官方页面。

## 2. CreepJS

唯一采用的官方在线入口：<https://abrahamjuliot.github.io/creepjs/>
源码：<https://github.com/abrahamjuliot/creepjs>
专项：<https://abrahamjuliot.github.io/creepjs/tests/workers.html>、
<https://abrahamjuliot.github.io/creepjs/tests/iframes.html>。

**执行**：打开主页，等待 FP ID 不再是 `Computing...`，且核心分组已填充或给出明确错误。
不能把初始页面的 `0% headless`、空插件或空字体当最终结果。

官方 README 列出了 `window.Fingerprint` 与 `window.Creep`。优先检查其存在性，
再读取 JSON 可序列化快照；它们是源码级集成点，不在本包中承诺 schema 稳定。

```python
await page.goto("https://abrahamjuliot.github.io/creepjs/", wait_until="domcontentloaded")
await page.wait_for_function(
    "() => window.Fingerprint != null && window.Creep != null", timeout=60_000
)
snapshot = await page.evaluate("""() => JSON.parse(JSON.stringify({
    Fingerprint: window.Fingerprint, Creep: window.Creep
}))""")
```

对象出现只是采集前置条件，还要核对 UI 的计算状态、核心分组及错误。
对象缺失或序列化失败时退回 DOM，记录 `schema_changed` 或 `serialization_error`，
不能猜测 `window.creepjs` 等未公布对象。

**保存**：完整快照（可得时）、FP ID、headless/resistance、Worker、Canvas/WebGL、
Fonts、DOMRect/SVG、Audio，以及实际显示的 lies/errors/异常详情。
Worker 专项和 iframe 专项按当前输出保存每个执行环境；不能把 `document` 等
Window 专用 API 在 Worker 内不存在写成错误。源码可查，线上 mirror 不自动可信。

## 3. DeviceAndBrowserInfo

指纹：<https://deviceandbrowserinfo.com/are_you_a_bot>
交互：<https://deviceandbrowserinfo.com/are_you_a_bot_interactions>

**指纹页执行**：等待总体结论和 `Raw detection details` 出现有效数据。
根据这个标题定位代码区，读取实际文本并解析 JSON（如果输出为 JSON）。
本包不假设 DOM id，也不要求剪贴板权限；页面 Copy 按钮不是唯一取证方法。

**保存**：原始检测对象与字段类型。重点查看当前返回的
`hasWebdriverTrue`、`hasWebdriverInFrameTrue`、`isPlaywright`、
`isAutomatedWithCDP`、`isAutomatedWithCDPInWebWorker`、
`hasInconsistentWorkerValues`、`hasInconsistentClientHints` 和 GPU 相关项。
不存在的键为“本次未返回”，不是 `false`。

**交互页执行**：另开标签，按当前页面测试表单/任务操作，记录点击、输入、提交顺序，
然后保存交互结论。只打开指纹页不得宣称已测试鼠标、键盘或行为模型。
该指纹页当前说明不使用 IP 信誉和行为；不要把这个说明扩展到所有关联站点。

## 4. Fingerprint Scan —— 有公开程序接口

测试入口：<https://fingerprint-scan.com/>
接口文档：<https://fingerprint-scan.com/docs>

```python
await page.goto("https://fingerprint-scan.com/", wait_until="domcontentloaded")
await page.wait_for_function(
    "() => window.FINGERPRINT_SCAN?.status === 'COLLECT_SUCCESSFUL'",
    timeout=60_000,
)
raw = await page.evaluate("() => window.FINGERPRINT_SCAN")
```

**就绪契约**：必须出现精确状态 `COLLECT_SUCCESSFUL`。
超时后可保存当时对象，但执行状态仍是 TIMEOUT/PARTIAL。

**保存**：整个对象，至少检查 `status`、`fingerprintHash`、`fingerprintId`、
`fingerprintParts`、`botScore`、`reasons`、`signals`；schema 变化保留原数据。
`reasons` 的类别/大小写原样保留，不硬编码只有某三个类别。

**解释**：分组字段用于定位差异；总分含网络因素。
`fingerprintId` 当前主要依赖浏览器存储，不代表纯无状态指纹追踪。
常规扫描不等于已经采集行为。示例 JSON 永远不能作为本轮输出。

适配器：`python scripts/collect.py ... --sites fingerprint-scan`。

## 5. Rebrowser Bot Detector —— 动作覆盖必须明确

页面：<https://bot-detector.rebrowser.net/>
源码：<https://github.com/rebrowser/rebrowser-bot-detector>
已核对 DOM 契约：`#detections-json` 是结果输入区，读取 `value`，不是 `innerText`。

```python
await page.goto("https://bot-detector.rebrowser.net/", wait_until="domcontentloaded")
await page.locator("#detections-json").wait_for(state="attached")
# 进一步等待 value 为非空且能解析的数组，再保存。
text = await page.locator("#detections-json").input_value()
```

### 必须分开的运行

**观察轮**：导航后仅采集，主动测试可能保持未触发。

**主环境动作轮**：新标签执行这些官方测试动作，每一步都写入 action log。

```python
await page.evaluate("() => window.dummyFn()")
await page.expose_function("exposedFn", lambda: None)
await page.evaluate("() => { document.getElementById('detections-json'); return true; }")
await page.evaluate("() => { document.getElementsByClassName('div'); return true; }")
```

前者检查主环境对象访问；其余分别触发绑定、求值来源和主环境调用观测。
主环境调用被检测到不自动代表产品回归，要与同驱动 Chrome 结果比较。
这些动作是检测站要求的测试刺激，不是给其它站点注入的常规采集脚本。

**隔离环境诊断轮（可选）**：前三步相同；最后一步使用已有的真实隔离执行路径。
附带脚本可通过 `Page.getFrameTree` → `Page.createIsolatedWorld` →
`Runtime.evaluate(contextId=...)` 建立独立 CDP 诊断，并检查 `exceptionDetails`。
必须标为 `isolated-cdp-diagnostic`，不能声称原有 `page.evaluate()` 已变为隔离执行。
对应 CDP 方法不支持时报告能力缺口，不退回 main world 冒充成功。

**保存**：全部检测行的 `type/rating/note/debug/msSinceLoad`（实际返回什么就存什么），
动作是否成功、执行 world、缺失检测行，以及观测窗口长度。

当前源码的 `rating < 0` 表示未发现该项，`0` 常表示未触发或无法判定，
`0.5` 是警告，`1` 是检测到。**不能将 `rating <= 0` 全算通过。**
特别是隔离环境动作成功而主环境钩子没有触发时，可能仍为 `0`；
只有动作日志能区分“在另一个 world 执行了”和“根本没做”。
站点含连续检测，没有通用“全部彻底结束”事件；记录结果快照时间与覆盖边界。

适配器：`--sites rebrowser --rebrowser-actions none|main|isolated`。

## 6. tls.peet.ws —— 请求必须来自被测浏览器

说明页：<https://tls.peet.ws/>
完整响应：<https://tls.peet.ws/api/all>

```python
response = await page.goto("https://tls.peet.ws/api/all", wait_until="domcontentloaded")
if response is None or not response.ok:
    raise RuntimeError("浏览器导航没有产生成功响应")
raw = await response.json()
```

`response.json()` 读取的是这次浏览器导航的响应，未另发 Python HTTP 请求。
不要改成 `requests.get()`、`curl`、`page.request.get()` 或 Playwright APIRequestContext。
不同请求方式/代理/TLS 终止点属于不同实验，不混在同一对照里。

**保存**：完整 JSON、响应状态/最终 URL；检查 `tls`、`http_version`、`ip`，
以及实际存在的 JA3/JA4、HTTP2、TCP/IP、请求头字段。具体子键以当次响应为准。
非 JSON、挑战页、缺失核心字段不算完成。

**解释**：先确认出口与 TLS 是否由代理终止，再比较协议字段。
HTTP/2、HTTP/3 或 JA4 未返回只证明本次没有相应数据，不证明浏览器永远不支持。
地址脱敏在摘要进行，原始证据保存在私有目录。

适配器：`--sites tls-peet`。

## 7. Sannysoft Antibot

入口：<https://bot.sannysoft.com/>

**执行**：打开页面，等具体检测表格的结果栏填充；初始 HTML 也可能带 `failed`，
所以不能只搜索页面中有没有这个词。确认 UA、语言、插件和 WebGL 等已实际采集。
保存表格的测试名、值、状态/样式，以及后续 Fingerprint Scanner 明细。
本包没有确认公开 JSON API；使用 DOM 表格和截图。

**范围**：基础冒烟测试。区分站点标为 Old/New 的检测，不把旧插件规则当成所有现代
Chrome 版本的规范；没有与版本匹配的基线时，保持结论有限。

## 8. Pixelscan

首页：<https://pixelscan.net/>
已核对扫描入口：<https://pixelscan.net/fingerprint-check>

**执行**：直接打开扫描入口，或从首页点击当前 `Scan My Browser Now` 链接。
等待 Browser、Location、Proxy、Fingerprint、Bot check 等本轮目标分组从
`Collecting Data…`/`scanning…` 变为实际结果；页面局部永远 pending 时记录未完成分组。
展开详情，分别保存画像、网络与 Bot 判断。

**提取**：DOM/截图。未确认稳定页面对象或公开 JSON API；不要猜内部 XHR。
只运行用户需要的 DNS/代理等专项，不安装推广软件，不提交问题反馈或授权 Geo API。
网络差评和浏览器不一致必须分开报告。

## 9. IPhey

入口：<https://iphey.com/>

**执行**：打开后等待 Browser、Location、IP address、Hardware、Software 分组
从 `Temporary value` 变为实际结果；出现总体结论后展开各项详情。
`Restart Test` 只用于本轮已记录的重试，不能不断点击选取最好成绩。
保存各分组值、实际显示的 MX Score/其它评分及说明、截图。

**提取**：DOM/截图。未确认稳定 JSON 契约。不要只抓页面模板里的 `0`，
也不要只凭营销说明推定所有 DNS/行为专项已经执行。

# Python/CDP 采集器

## 作用和依赖

`scripts/collect.py` 只实现 3 个适配器：Fingerprint Scan、tls.peet.ws、Rebrowser。
其它 19 项由 agent 根据站点手册执行。本脚本是测试脚手架，不是新的生产 SDK。
依赖 Python 3.10+ 和官方 Playwright Python。优先复用项目已有、锁定版本的虚拟环境。
未安装时，在独立测试虚拟环境内安装并记录实际版本；不改生产依赖。
已有浏览器通过 CDP 提供服务，因此本脚本不需要下载 Playwright 浏览器。

Playwright 官方说明 CDP 连接只支持 Chromium 路径，能力也不等同原生 Playwright
协议连接。自研内核提供 CDP 不代表它已经兼容本脚本用到的所有方法。
不兼容时保留错误，改用项目已有的原始 CDP/浏览器工具另做测试并标明不同控制链；
不要自行启动系统 Chrome 后把它标为候选。

## 使用

从本 skill 根目录执行 `python scripts/collect.py --help` 查看全部参数。
先复制并补全 `assets/run-manifest.example.json`；确认 endpoint 对应专用 profile。

```bash
python scripts/collect.py \
  --cdp http://127.0.0.1:9222 \
  --label candidate-headless-cdp \
  --manifest /absolute/path/to/candidate-manifest.json \
  --dedicated-profile \
  --sites tls-peet fingerprint-scan rebrowser \
  --rebrowser-actions none \
  --out /absolute/path/outside/repo/candidate-observation
```

只测指定站点就缩短 `--sites` 列表。Rebrowser 的 `main` / `isolated` 各用新输出目录，
分别执行；脚本为每个站点创建新标签。
默认在每次运行新建匿名 context，不创建磁盘 profile。需要 profile 冷启动实验时
由项目启动器提供另一个干净 endpoint。

manifest 必须声明 `role=candidate` 或 `role=baseline`。默认 `--context-mode fresh`，
用 `browser.new_context(no_viewport=True)` 保留浏览器 viewport，并记录 context 参数和
所有权。创建失败即停止，不复用默认 context。Chrome baseline 强制使用 fresh。
候选持久存储实验可用 `--context-mode existing --context-index N`，明确复用哪个已有
context；不能把这个路径的结果与 fresh 混成一轮。fresh 不接受非零 context index。
远程地址需要 `--allow-remote-cdp`，建议走已授权本地隧道；不在 URL 内写用户名密码。
输出目录必须不存在，避免覆盖旧证据。

## 官方驱动的默认覆盖

当前 Playwright 文档提供 `connect_over_cdp(..., no_defaults=True)`，
用于避免连接时对已有默认 context 应用部分媒体、焦点和下载设置。
该参数不是旧版本都有，本脚本检查运行时方法签名：

| 参数 | 行为 |
|---|---|
| `--cdp-context-defaults auto`（默认） | 支持时传 `no_defaults=True`；不支持时正常连接，但 manifest 明确记录默认覆盖可能存在。 |
| `--cdp-context-defaults preserve` | 必须支持 `no_defaults`，否则停止；用于要求避免这部分覆盖的运行。 |
| `--cdp-context-defaults framework` | 使用官方框架默认行为，适合复现原有自动化调用链。 |

`no_defaults=True` 也不意味着无观察者效应。不同模式必须分组对照，不能混成一轮。
该选项只控制附着时已有默认 context 的覆盖，不改变新 context 的官方驱动默认行为。
不要因为旧版没有选项就偷偷升级驱动，基线也必须使用同一个版本和模式。

## 结果目录

```text
<out>/
  manifest.json
  results.json
  fingerprint-scan/
    result.json
    raw.json                # 仅实际得到数据时存在
    page.txt                # 截断时在 result.json 说明
    screenshot.png          # 当前视口，不强制改 viewport
  tls-peet/...
  rebrowser/...
```

页面文本只是支持证据，不涵盖所有表单 value 或折叠内容；结构化 raw.json 优先。
`--no-snapshots` 关闭文本和截图，只减少取证步骤，不会让 CDP 变成无干扰采集。
采集器默认不监听 console；会记录 pageerror。诊断需额外 console/HAR 时另做一轮，
同时在基线开启相同工具。不要把调试轮覆盖成普通轮。

退出码：`0`=所选采集范围完成；`2`=存在非完整项；`1`=致命错误；`130`=被用户中断。
**0 不代表浏览器通过**。所有 comparison_verdict 初始 INCONCLUSIVE，等待 agent 对照。

## 失败处理

结构化对象迟迟不出现、HTTP 200 的挑战页或 schema 不符：保存现有证据并报告。
自动采集器未识别的挑战可能先表现为 TIMEOUT；agent 根据截图复核后在报告中分类，
保留原始状态，不反复刷新。403/429 不自动重试。

`Runtime`、`Page`、`Target` 或相关 CDP 不支持时，保留官方 driver 的原始错误。
脚本没有“静默换浏览器”“伪造空结果”或自动修补检测逻辑的 fallback。

中断/故障仍尽力保存已经取得的结果，但进程被强杀时不能保证完成最后一次落盘。
只关闭本脚本创建的页面和 context，退出时停止 Playwright 客户端；不调用
browser.close，也不关闭复用的 context。自有 context 清理或客户端断开失败返回 1。

## 验证本脚本

单元测试：

```bash
python -m unittest discover -s tests -p 'test_unit.py' -v
```

另外提供可选的本地 fixture 集成测试说明与脚本，使用专用本地 Chromium，
测试公开对象、DOM value、响应体、错误状态和连接生命周期，**不是外站检测实测**。
详见 `tests/README.md`。该验证不代表你的 Obscura/CDP 实现已经通过。

官方依据：
<https://playwright.dev/python/docs/api/class-browsertype#browser-type-connect-over-cdp>
<https://playwright.dev/python/docs/api/class-response#response-body>
<https://playwright.dev/python/docs/api/class-playwright#playwright-stop>

# Browser Fingerprint Audit Skill

面向 Obscura、官方 Playwright Python/CDP 与 Chrome 对照的指纹、隐私和自动化回归 skill。

本项目的唯一源码目录是 `skills/browser-fingerprint-audit/`，纳入 Git 管理。
Codex 项目入口 `.agents/skills/browser-fingerprint-audit` 是指向该目录的相对符号链接。
可直接调用 `$browser-fingerprint-audit`；未刷新到发现列表时重新打开项目 chat，
或直接读取本目录 `SKILL.md`。不需要安装用户全局副本。

项目连接、persona、资格边界与输出路径见 `references/obscura.md`。

建议入口提示词：

> 读取 skills/browser-fingerprint-audit/SKILL.md。使用项目现有的候选浏览器与可比版本 Chrome
> 测试端点执行 core 回归。保持被测配置不变，保存原始结果、动作覆盖与截图，
> 区分未触发、采集失败和检测异常，输出差分 report.md。只测试，不修改内核。

首次执行需要：可识别的候选进程/连接、专用 profile、实际环境 manifest（role=candidate/baseline）；
做回归判断还需要可比 Chrome 基线。缺失条件时 agent 应报告阻塞，而不是换浏览器代测。

## 内容

- `SKILL.md`：触发条件、范围选择、对照矩阵、执行和报告要求。
- `references/core-sites.md`：9 个核心站点的具体操作。
- `references/extended-sites.md`：13 个扩展站点/项目的具体操作。
- `references/reporting.md`：执行状态、对照结论、网络和追踪实验。
- `references/collector.md`、`references/sources.md`：脚本用法、限制与官方依据。
- `scripts/collect.py`：Fingerprint Scan、tls.peet.ws、Rebrowser 三个程序适配器。
- `assets/`：环境 manifest 和最终报告模板。
- `references/obscura.md`：项目的 persona、匿名 Chrome 对照和能力边界。
- `tests/`：26 项离线单元测试、9 项完整本地 HTTP fixture 检查与验证范围说明。

本包不修改浏览器源码/配置、不建立新的生产 SDK、不依赖 MCP、不承诺网站全绿或不可检测。
三个适配器的接口依据与本次复核范围见 `references/sources.md`；其余 UI 手册来自原包，运行时需核对。

实际验证范围及未完成的公网/Obscura 差分验收见 `tests/README.md`。

## 导入来源和审核修正

原始包：用户提供的 `browser-fingerprint-audit.zip`。
SHA-256：`975c40fa91b3a74a9b552d224174499cce3ac1ca97db84497899fde145da9144`。
保留原包的 22 项手册、三个适配器、模板与测试源码；不提交生成的截图或运行报告。

项目版 1.1.0 修正默认复用 CDP context 的问题：默认新建匿名 context，baseline
禁止复用已有 context，创建失败无 fallback，只清理自有资源。增加对应回归覆盖。
修正 Fingerprint Scan 重叠超时的重复异常，并将运行环境声明移入正文以通过本机
skill frontmatter 校验。产物默认放仓库外；缩小触发范围，补充 Obscura 的能力边界；完整 HTTP fixture
已用项目固定版本 Playwright 1.60.0 复核。测试采集完成不等于检测器通过。

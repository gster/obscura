# Obscura / Southwest 接续文档

更新时间：2026-09-17。本次按用户要求停止诊断，整理并提交交接；403 任务尚未完成。

## 下一会话从这里开始

1. 拉取 `git@github.com:gster/obscura.git` 的 `main`，阅读仓库 `AGENTS.md`、本文及下列两个已有报告。
2. 先恢复证据归档（下文），再继续比较；不要把旧 `/tmp` 路径当作新设备已经存在的文件。
3. 优先用 **computer use 操作 Chrome** 做当前时刻的新鲜控制，确认同一本机代理下 Chrome 是否仍能 200。最后一次 Chrome 成功 HAR 来自 2026-09-16 15:44 UTC，不是最新失败测试的同步控制。
4. 随后核对 Worker 的 `importScripts` 源码、返回值与环境调用。当前只证明主保护脚本、四子脚本以及 Worker 启动包装代码一致，尚未完整核验 Worker 导入源码及计算结果。
5. 用有区分力的最小复现修通用实现；不要为了得到 200 加入 Southwest 专用等待、强制脚本屏障或移植 Cookie/token。

主要报告（仓库路径）：
- `docs/Worker-compatibility.md`：Chromium/HTML 主资料链接、Worker 实现比较、已修复范围、测试结果、性能测量及最近成功/失败对照。
- `docs/Southwest-fix-record.md`：整个调查的历史；末尾为最近结论。早期阶段的“尚未整合”等句子是当时状态，当前代码已经整合。
- 原始证据中的 `worker-fair-native-repeat/COMPARISON.md`：最新失败、上轮成功和 Chrome 的详细对照与文件索引。

## 用户约束

- 目标：最新 Obscura runtime 可以代替 Chrome 完成 Southwest 搜索，修复自身 bug 和浏览器实现差异。
- Chrome 必须通过 computer use 操作；不要用 Playwright/CDP 驱动 Chrome。Python 测试虚拟环境名字中出现 playwright 不表示允许它控制 Chrome。
- 测试程序都在本机运行。需要代理交叉时，一组使用本机代理，一组经 mini32 代理；不是把程序放到 mini32 上跑。
- 直接 goto 构造的搜索链接，不从首页输入/点击。
- 用户优先要求原始数据完整，允许本机保留未经脱敏的 Cookie/头/正文。归档含原始会话数据，不能提交到 Git 或公开发布。
- 同一个 `127.0.0.1:7890` 入口不证明实际公网出口相同。不要再把“IP 一致”当作已验证前提。
- 不根据 Cookie 数量、单个顺序差异、HTTP 200 或 Worker 无报错单独断言根因。搜索成功应核验行程数据。
- 没有要求主动使用子代理；当前不要派发子代理。

## 当前代码和构建状态

基线提交：`a752298bb0bee7128308affa87f8969d39cc9fbc`。接续代码在其后的本次 handoff 提交中，使用 `git log` 查找标题 `fix(worker): isolate execution and preserve runtime progress`。

本次实现已从临时工作树整合到主仓库，并核验对应源文件逐字节一致：
- Worker 独立线程、V8 isolate、事件循环；有界消息队列、嵌套 Worker 数量/生命周期控制。
- V8 structured clone 与 ArrayBuffer 转移、失败不脱离源缓冲区。
- Worker 网络策略更新、拦截/事件/正文转发、共享 Cookie、请求 ID 唯一、相对 URL 解析。
- Worker identity getter 重入和异常清理。
- `runtime/src/main.rs` 保留自动推进截止时间；高频查询不再不断重置 20ms 等待，饿死页面定时器和 Worker 消息。
- `crates/obscura-browser/src/page.rs` 只有测试 HTTP server 的 socket blocking-mode 修正，没有生产渲染改动。

代码细节和未支持的 Worker 子集见 Worker 文档。模块 Worker、MessagePort、SharedArrayBuffer、部分 host-object clone 等并未因此全部兼容；不要宣称完整 Web Worker 标准一致。

测试全部已完成，无后台构建/测试需要接管：
- 核心完整 release render nextest：1747 passed，4 skipped。
- Worker + 保护探针 release render,stealth：28/28。
- runtime 独立 workspace release nextest：184/184。
- Python SDK 完整测试：33/33；新增高频查询真实子进程测试已验证先失败再通过。
- 指定 render release CLI 和 render+stealth runtime 构建成功。
- 障碍课程 **32/33**。`observer-intersection` 失败在保留旧 CLI 上同样复现；**未满足 33/33**，不能隐去或宣称所有门禁全绿。
- 六轮交替性能对照见 Worker 文档；数据测的是加入高频查询修复之前的独立 Worker 版本，不能冒充最终版本的新性能测试。

本机正式安装的 runtime **没有替换**：
`/Users/zg/.local/share/obscura-tools/target/release/autopilot-browser-runtime`
SHA-256：`0cbec708f17ad85985929dfc3f1ba88c9e4581189177c06e8d57b6e6cd6f6326`。
新的代码在 Git 中；新设备应按源码构建，不依赖这个平台相关二进制哈希。

## 最后业务结果及下一步判别

查询固定为 WN / BWI → MCO / 2026-09-30，一名成人，单程。若继续时该日期已过期，要同时更新 Chrome 和 Obscura 的查询，不与旧日期结果直接比较。

之前新、旧 runtime 交替共四组测试都拿到首次 shopping 200、26 行程；随后又出现 403。最新版本的被动采集和紧邻上一版本控制，均两次返回 `403050700`。所以既不能说已经稳定成功，也没有证据把最新失败归因于刚加入的调度修复。

最关键反证：成功 Obscura 的首次 shopping 早于 Worker 执行约 160ms、早于四子脚本约 284–287ms；最新失败样本中四子脚本已提前约 958–926ms 全部结束，Worker 仍晚约 157ms。顺序无法单独区分成败。Chrome 首个 Worker 执行早于 shopping 约 405ms。

语言头纠正：Chrome 原始隐身 HAR 的首次 shopping 是 `en-US,en;q=0.9`；Obscura macOS profile 是 `en,zh-CN;q=0.9,zh;q=0.8`。历史中曾将后者写成 Chrome 值，已更正。

继续时建议先补当前 Chrome 控制，再分析实际出口/连接与 Worker 导入代码。原生记录点位于 primp request-builder 前，不是 TLS/H2 wire dump；HAR 伪头和库自动生成的 Content-Length 不能与这里简单做缺失比较。

## 跨设备恢复原始证据

源机器 hostname：`booster`（是否能 SSH 访问需新设备自行确认，不假定已配置别名）。原始目录是 `/tmp/southwest-request-audit`。

已打包：
- `/tmp/obscura-southwest-handoff-20260917.tar.gz`
- `/tmp/obscura-southwest-handoff-20260917.tar.gz.sha256`

归档 SHA-256：`af82004f3c32fb6b9aeda7550301730d3bef342223c3e892b756f747d5821d15`。

**Git push 不会传输这个归档。** 需通过用户可用的安全文件传输从 booster 取走；例如在能访问该主机的设备上用 scp 下载上述两个文件，然后校验 `shasum -a 256 -c ...sha256`，解压到 `/tmp`。源机 `/tmp` 可能被清理，请尽快取走。无需复制几百 MB 的诊断二进制，可从 Git 重建。

归档包含：Chrome HAR/Performance trace/截图等原始记录；最近 Obscura 成功、失败、紧邻控制的原生事件及正文；SDK 事件和业务 metadata；采集脚本；测试/构建日志；离线 Worker 夹具。未把生成文件提交到仓库。

重要目录：
- `chrome-local-cua/network-incognito-full.har` 和 `performance-incognito-full.json.gz`：最后已完成的 Chrome 成功控制。
- `worker-threaded-native`：独立 Worker 版本原生采集成功，200 / 26 行程。
- `worker-fair-native-repeat`：最新失败，含 `COMPARISON.md`、`chrome-comparison.json`、`manifest.json`、`worker-sources/`。
- `worker-native-near-control`：紧邻上一版也失败。
- `worker-threaded-live-v2`、`worker-baseline-control`：完整逻辑采集的成功对照。
- `worker-repro`：离线夹具、日志、诊断脚本/补丁。脚本内有旧设备绝对路径，使用前必须检查并修改。

最新失败捕获覆盖两次 shopping 完整请求和响应。关闭时一个 `bf.html` beacon 未返回；一个 301 有响应头但无最终逻辑响应正文。其他已记录正文文件都存在。不是所有后台请求都已结束。

诊断脚本 `apply-passive.py` 只应应用于隔离工作树，且依赖 `passive-base.patch`；它还没记录 `op_worker_load_script` 实际返回的全部 importScripts 正文。下一轮补这个点位并复核 Worker 结果，不能以启动包装脚本相同代替完整导入/执行相同。代码中的临时诊断插桩已撤销，未合入正式源码。

## 在新设备上运行

按 AGENTS.md 安装构建依赖；原机使用 `/Users/zg/.local/share/obscura-tools/env.sh`，该路径不是跨设备前提。运行 nextest，不用 cargo test；不要 bulk cargo fmt。

```sh
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release --manifest-path runtime/Cargo.toml
```

runtime 构建使用它自己的 workspace，默认依赖 render+stealth。构建前检查 `CARGO_TARGET_DIR`；原机各工作树共用 target，构建会覆盖正式 runtime。诊断时复制产物到单独路径，恢复原安装版本。不要在同一个 target 中同时构建与运行全量门禁，否则测试二进制可能混合不同版本。

Python bindings 位于 `bindings/python/src`。归档的 `run-profile.py` 使用正式 Python SDK 直接 goto，不会驱动 Chrome。新设备需创建自己的 venv 并安装 SDK 依赖；更新脚本内硬编码路径。`run-profile-full.py` 限制同时读取正文为四个，未限流版本会打满 runtime 16-entry evidence mailbox，正常退出并显示 BROWSER_EOF；这不是 Worker 崩溃证据。

## 中断时的 Chrome 状态

继续指令后只执行了 computer use 文档恢复及选择 Chrome。前台为普通 Chrome 的 New Tab，尚未开始新导航/录制。因此**没有新增 Chrome 结果**。不要把这次打开窗口当成已完成的控制。此前授权过保留测试窗口在前台，但新设备仍应先观察当前 UI 再操作。

## Suggested skills

- `diagnosing-bugs`：继续最小复现、失败/成功对照和回归闭环；本会话已经使用。
- `handoff`：需要再次迁移时更新接续信息。
- 不需要学术研究类 skill。Chrome 使用 `cua_repl` 的 native app computer use 文档和 API，不调用 Playwright/CDP 控制。

## 原会话

当前任务 ID：`01a0a928-02fb-78d3-97bc-05f2fdc279f0`。
相关更早任务：`01a0a7ce-6ff3-7570-9565-20c0a9685e24`；如要依赖其中内容，先调用 read_thread，不能只根据标题推断。

用户本次要求是文档交接及提交推送，不是继续跑搜索或部署 runtime。下一设备会重新启动任务。保留未解决的 403 状态，不宣称目标完成。

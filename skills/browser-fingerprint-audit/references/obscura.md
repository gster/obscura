# Obscura 项目适配

先读取仓库根目录 `AGENTS.md`。本 skill 补充身份与隐私证据；发布行为门禁仍由
companion `obscura-benchmark` 的 33 阶段 obstacle course 等项目检查负责。

## 连接被测进程

使用项目已有的 release binary，明确记录源码 revision、二进制 SHA-256、构建特性、
persona 和启动命令。未提供被测环境时，可启动本轮拥有的本地测试进程：

```bash
OBSCURA_PERSONA=windows_chrome145 ./target/release/obscura serve --port 9222
```

这是通用 fixture persona。业务任务指定其它 PersonaSpec 时使用指定值。
当前合格客户端切片采用官方 Playwright Python 1.60.0；复用已固定的测试环境。
具体方法和未验证范围见仓库的 `docs/Use-with-Playwright.md` 与 `docs/SUMMARY.md`。
这些方法存在或可调用，不等于本 skill 的公网流程已取得资格。

本地 fixture 需要 Obscura 的 `--allow-private-network`；只对本轮测试进程使用，
公网采集保留项目的默认 SSRF 限制。停止时只停止本轮拥有的进程。

## Chrome 对照与 context

Chrome 对照每轮必须新建匿名/无痕 `BrowserContext`。默认 profile、旧匿名 context、
已有业务标签页不能充当对照。新 context 创建失败时终止该组，没有默认 context fallback。
人工无 CDP 的 A 组也从新无痕窗口开始；无该条件则标未测。

采集器默认对两边都使用 `--context-mode fresh`，并声明 manifest 的
`role=candidate` 或 `role=baseline`。仅候选的持久存储实验可使用 existing。
与网络/代理相关的 context 设置不能假定自动继承；记录实际出口与可比性。
基线与候选必须匹配 viewport、device scale、身份目标、语言时区、网络路径、驱动版本
和动作顺序。新 context 的默认值及测量干扰也要记录。

Chrome 匿名对照出现 403/429 时，报告对照或 endpoint/网络环境可能受限，
不得当作 Obscura 通过或失败的证据。缺少可比基线时仅报告候选观测，结论 INCONCLUSIVE。

## 路径和产物

skill 源码位于 `skills/browser-fingerprint-audit/`；项目发现入口为
`.agents/skills/browser-fingerprint-audit`，以相对符号链接指向源码目录。
修改源码即可更新入口，不复制第二份 skill，不改用户全局 skills 配置。

从仓库根目录运行采集器时使用完整的仓库相对路径，产物放到仓库外：

```bash
RUN_ROOT="$(mktemp -d)"
python skills/browser-fingerprint-audit/scripts/collect.py \
  --cdp http://127.0.0.1:9222 --label obscura-observation \
  --manifest /absolute/path/to/candidate-manifest.json --dedicated-profile \
  --sites tls-peet fingerprint-scan rebrowser --rebrowser-actions none \
  --out "$RUN_ROOT/candidate"
```

manifest 模板中的 unknown/null 要按实际环境补全，无法确认时保留未知。
Chrome 基线使用自己的 endpoint、label、`role=baseline` manifest 和新输出目录。
截图、原始指纹、运行报告和代理凭证不进入 Git。

## 控制链：裸 CDP 可用

Obscura 本体不依赖 Playwright。当被测目标是 Obscura（或任何 CDP 服务端）时，
用裸 CDP over WebSocket（例如 Python `websockets`）驱动候选与 Chrome 基线，
可以让两侧走**完全同一条**控制链，避免官方驱动在附着时应用的默认 context 覆盖。
采集器脚本仍然只用官方 Playwright；两者不要混在同一轮对照里。

## 本地测试环境排查

以下现象来自一次 WorkBuddy 的 macOS 沙箱运行，复跑时先核对当前环境：

1. shell 若注入回环 HTTP 代理，CDP 本地连接可能返回 502。为客户端设置
   `NO_PROXY='localhost,127.0.0.1,::1'` 和相同的 `no_proxy`，仅绕过本地连接。
2. 测试服务必须由宿主支持的长驻进程会话启动；确认进程存活后再连接。
3. Chrome 若因沙箱初始化失败退出，记录错误和实际启动参数；只有当前环境
   确实需要时才使用 `--no-sandbox`，并在 baseline manifest 中写明。
4. 无头 Chrome 的 UA 含 `HeadlessChrome` 时，站点级判定不可直接与普通
   Chrome persona 比较。跨版本字段也要单独标为不可比。
5. Chrome 对照必须成功新建匿名 `BrowserContext`，失败则停止，不能退回
   默认 profile。裸 CDP 使用 `Target.createBrowserContext` 后显式传入 context ID。

## 改 bootstrap.js 前必须知道

`crates/obscura-js/build.rs` 在**构建期**执行 `bootstrap.js` 生成 V8 snapshot，
页面 realm 从该 snapshot 恢复。由此产生三条硬约束：

1. 对 V8 惰性全局执行的一次 `delete` **可能不会**存活到页面：V8 从 snapshot 恢复 context
   时会重建惰性全局（`SharedArrayBuffer` 就是实例）。需要 per-page 生效的改动要放进
   `__obscura_init`。
2. 需要向 Rust 暴露的全局钩子必须加进
   `_preHideInternals` 的 `_names` 列表；IIFE 内部 helper 保持闭包私有，否则改动本身就成了 `unusualWindowProperties`。
3. 改完 `bootstrap.js` 必须重建 release 二进制再测。先跑
   `node --check crates/obscura-js/js/bootstrap.js` 做语法校验，比等 V8 快得多。
   `nextest` 也能在构建期就抓住 bootstrap 抛错（build.rs 会 panic 并给出 JS 堆栈）。

## 回归口径

```bash
cargo nextest run --release -p obscura-net            # 无 render 特性
cargo nextest run --release --features render -p obscura-js
```

persona 内容变化会让 `persona.rs` 里的 golden digest 失效；`stable_seed` 只哈希
persona_id/revision/profile，所以指纹种子不受影响。更新 golden 时要确认这一点。

## 解释能力缺口

Obscura 的 utility-world ID 当前不证明真正独立的执行全局。Rebrowser isolated 轮
是可选诊断；相同执行结果必须结合真实钩子、动作日志和项目能力说明解释。
Worker、Service Worker、WebRTC、WebGPU 等也不能因站点未报警就声称已经实现。
缺失的产品必需能力要报告；不因接口不支持而改用系统 Chrome 冒充候选。

默认第三方请求允许，tracker blocklist 为显式选项。网络一致性与启用 blocklist 的
隐私效果分别记录配置、运行和结论，不能测到一边的结果就推断另一边通过。
CDP、persona 和 `DNT=1` 属于实现路径；检测站全绿也不证明不可追踪、业务一定放行，
或解决身份关联的航司价格差异。未经控制实验验证的效果仍是目标。

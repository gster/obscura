# Obscura

基于 Rust、V8 和自有 DOM/布局/绘制管线的 headless 浏览器内核。

根本目标是：对 RPA 友好、性能突出、在服务端可观察的行为与指纹上与固定参考 Chrome 一致，并具备强匿踪、反追踪和防身份标记能力的浏览器内核，降低身份关联被用于差异化报价、侵害消费权益的风险。CDP-first 和官方 Playwright Python 是实现路径，不替代这些产品目标。反追踪不等于主动设置 `DNT=1`。

本 fork 已收敛到 **CDP-first、未修改的 Playwright Python 客户端、macOS/Linux 独立内核**。

自有 Python SDK 和配套 NDJSON runtime 已移除；保留 MCP 与有助于 agent 接入的 CLI。统一 persona 与经校准的 primp 已成为不可关闭的底层能力，后续优先修补 Chrome 行为差异。Southwest shopping 不再 403 且返回有效查询结果，是重要业务验收点。

强制 primp、版本化 persona 编译器、入口必配和 context 生命周期冻结已经落地；Linux/容器、字体、图形和完整传输指纹资格仍需独立验证。支持部分 CDP 和 Web API，不承诺完整 Chrome 或 Playwright 替代能力。当前事实和测试结果见 [SUMMARY](docs/SUMMARY.md)，开发顺序见 [TODO](docs/TODO.md)。

## 构建与运行

在仓库根目录，使用 Rust 1.98.1（本轮核验工具链）：

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --release -p obscura-cli --bins --features render
./target/release/obscura --persona windows_chrome145 serve --host 127.0.0.1 --port 9222
```

CDP browser endpoint：`ws://127.0.0.1:9222/devtools/browser`。官方 Playwright 使用 `connect_over_cdp`，不能把 Obscura 当作 `chromium.launch(executable_path=...)` 的 Chromium 二进制。

当前诊断 CLI：

```bash
./target/release/obscura --persona windows_chrome145 fetch https://example.com --eval 'document.title'
./target/release/obscura --persona windows_chrome145 fetch https://example.com --screenshot /tmp/obscura-example.png
```

所有构建和入口都使用 primp 与统一身份基线。它不是网站可访问性或完整浏览器身份一致性的保证。依赖、证书和平台要求见 [源码构建](docs/Build-from-source.md)。本 fork 的安装应固定源码提交；不能仅凭上游发行包的版本号确认它包含本 fork 的固定提交及修复。

## 验证与限制

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --locked --release --features render --no-fail-fast
```

V8 相关测试按项目要求使用 nextest 的进程隔离。当前测试覆盖根 workspace；已删除的私有 runtime/SDK 只在冻结历史记录中保留。完整门禁及 companion benchmark 的执行方式见 [测试指南](docs/Testing-and-debugging.md)。测试结果按提交、平台和 feature 分开记录，不引用旧通过数宣称当前发布合格。

当前已确认的缺口包括 Beacon 空成功、部分 CDP 域占位成功、对象检查副作用、localStorage 未落盘，以及 Cookie 的完整 SameSite/分区资格仍未完成。Southwest 的历史成功样本不能覆盖后续 403，当前没有可宣称恢复的现场证据。详见 [核验摘要](docs/SUMMARY.md)。

CDP 是高权限控制接口，当前没有内建鉴权；默认保持 loopback。V8 在进程内执行，watchdog 不替代 OS 隔离。[安全边界](SECURITY.md)与[部署说明](docs/Run-in-production-at-scale.md)描述现有保护和限制。

## 文档

- [SUMMARY：当前事实、测试与文档索引](docs/SUMMARY.md)
- [TODO：唯一开发执行清单](docs/TODO.md)
- [当前 TODO goal 交接：实现基线、验证证据与后续顺序](docs/Goal-handoff.md)
- [新架构：目标边界与迁移门槛](docs/New_ACH.md)
- [现有架构](docs/Architecture-overview.md) · [Playwright 接入](docs/Use-with-Playwright.md)
- [Southwest shopping 验收](docs/Southwest-handoff.md)

许可证：[Apache-2.0](LICENSE)。

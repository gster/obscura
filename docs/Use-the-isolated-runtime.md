# 迁移期独立 runtime

`runtime/` 直接嵌入引擎，产物名为 `autopilot-browser-runtime`，启用 render，并自动包含强制 primp 和统一身份基线。`bindings/python/` 的自有 SDK 通过有界 stdin/stdout NDJSON 与它通信，**不运行 CDP，也不是官方 Playwright Python**。

```bash
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release)
(cd runtime && CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo nextest run --locked --release --no-fail-fast)
(cd bindings/python && uv build --wheel --out-dir /absolute/wheelhouse)
```

它有自己的 Cargo workspace、锁文件和 Rust 1.98.1 工具链。根 CLI 的 build/nextest 不覆盖它。详见 [runtime 构建说明](../runtime/README.md) 与 [SDK 契约](../bindings/python/README.md)。使用方提供产物绝对路径、完整摘要、persona、允许 origins 和私有 workspace；安装 wheel 不会自动安装 Rust 产物。

新方向停止扩展自有 SDK，先把原生输入、等待、网络证据、资源预算和启动/退出保护迁入共享层，再删除私有协议包装。替代客户端链路通过之前，保留必要修复和回归，见 TODO OB-005/006/021/032。历史消费仓库 pin、主机路径、现场部署和测试数字不再在本仓库重复维护。

本轮实际测试状态统一见 [SUMMARY](SUMMARY.md)。旧 SDK 或离线例子通过不能证明官方 Python/CDP 资格，也不能证明任何真实购票或支付流程成功。

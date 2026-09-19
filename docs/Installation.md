# 安装本 fork

本 fork 当前可靠的源码入口是固定提交的本地构建，见 [Build from source](Build-from-source.md)。不要把 upstream 的 `latest`、Docker tag、AUR 包或旧发布矩阵当作包含本 fork 修复的安装包。

```bash
CARGO_INCREMENTAL=0 CARGO_BUILD_JOBS=2 cargo build --locked --release -p obscura-cli --bins --features render
./target/release/obscura --version
```

产物为 `target/release/obscura` 和 `obscura-worker`（设置 `CARGO_TARGET_DIR` 时路径不同）。版本字符串不足以识别源码，交付时同时记录完整提交、锁文件、target、features 和 SHA-256。

primp 传输与统一身份基线包含在所有构建中，没有运行时开关。`runtime/` 是另一套产物，根构建不会生成它，见 [迁移期独立 runtime](Use-the-isolated-runtime.md)。

下一阶段主平台目标是 macOS arm64 与 Linux x86_64；完整平台资格尚未建立。当前仓库保留的其他平台发布配置不等于已经移除，也不构成本 fork 的正式支持承诺。

# 当前传输实现与统一目标

2026-09-20 复核。产品 Page、CDP、MCP、Worker 和有 Page 所有者的 JS fetch/XHR 路径使用 primp；renderer 默认 cache 已不再自行联网，robots.txt 也进入同一 primp。`ObscuraHttpClient` 仍保留可执行 reqwest 后端，standalone runtime/module loader 仍可到达该路径，`obscura-net::wreq_client` 兼容 re-export 也尚未删除，因此 OB-012 不能关闭。旧比较中的 upstream commit、Python 探针和现场请求只代表迁移前历史，不是本次传输测量。

## 当前源码事实

- [stealth_transport.rs](../crates/obscura-net/src/stealth_transport.rs) 选择 Windows Chrome145、macOS Chrome152/153 preset，清理 primp 自动默认头，由引擎补身份和请求语义。
- 显式 `no_proxy`、禁止自动 redirect、自有 Cookie/URL/响应体策略；配置代理经选定路径使用。CORS 预检不补 Client Hints 的回归在同文件。
- [vendor/primp](../vendor/primp/) 保留显式 DNS resolver 优先的本地修改；代理端解析目的地址时，不能以本地 resolver 声称最终 IP 已验证。
- `retry::never()` 关闭外层重试，不代表 vendor 内所有协议级恢复都被禁用。请求写出状态未知时不得在引擎增加盲目重放。
- Worker 采用 detached primp 独立池并保持配置与 Cookie。
- [vendor/primp-rustls](../vendor/primp-rustls/) 带本地 Chrome 扩展顺序修改；它不是未跟踪的临时目录，也不等于上游未修改依赖。

## 验证边界

本次没有重跑 TLS/H2 抓包、Chrome 线级对照或网站验收。历史 ALPS/trust-anchor/头顺序差异需要在固定构建上重新采集才能声明当前数值。JA3/JA4、版本名、单次导航成功不证明所有资源类别的传输等价；具体 method/body、redirect、credentials、预检、代理和连接复用分别验收。

运行时 stealth 开关和产品双传输分支已经删除。仍需按 OB-012/014/015/016 完成仓库其余自有 HTTP(S) 出口盘点、线级校准和完整 persona 编译器；官方客户端自有网络的支持边界见 OB-033。历史迁移过程可在 `e67e67b` 的同名文件查看。

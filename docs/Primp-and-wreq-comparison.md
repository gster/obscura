# 当前传输实现与统一目标

2026-09-19 整理。当前 stealth 使用 primp，普通路径使用 reqwest。`obscura-net::wreq_client` 仅为 `stealth_client` 的兼容 re-export，没有独立 wreq 后端。旧比较中的 upstream commit、Python 探针和现场请求只代表迁移前历史，不是本次传输测量。

## 当前源码事实

- [stealth_transport.rs](../crates/obscura-net/src/stealth_transport.rs) 选择 Windows Chrome145、macOS Chrome152/153 preset，清理 primp 自动默认头，由引擎补身份和请求语义。
- 显式 `no_proxy`、禁止自动 redirect、自有 Cookie/URL/响应体策略；配置代理经选定路径使用。CORS 预检不补 Client Hints 的回归在同文件。
- [vendor/primp](../vendor/primp/) 保留显式 DNS resolver 优先的本地修改；代理端解析目的地址时，不能以本地 resolver 声称最终 IP 已验证。
- `retry::never()` 关闭外层重试，不代表 vendor 内所有协议级恢复都被禁用。请求写出状态未知时不得在引擎增加盲目重放。
- Worker 普通和 stealth 客户端均采用 detached 独立池，配置保持；源码和配置回归位于两个 client 文件。
- [vendor/primp-rustls](../vendor/primp-rustls/) 带本地 Chrome 扩展顺序修改；它不是未跟踪的临时目录，也不等于上游未修改依赖。

## 验证边界

本次没有重跑 TLS/H2 抓包、Chrome 线级对照或网站验收。历史 ALPS/trust-anchor/头顺序差异需要在固定构建上重新采集才能声明当前数值。JA3/JA4、版本名、单次导航成功不证明所有资源类别的传输等价；具体 method/body、redirect、credentials、预检、代理和连接复用分别验收。

不再推荐“先接入 primp”或“保留 Windows wreq”作为未来任务，这些说法已经过时。目标是经修复和线级校准的 primp 取代所有项目自有 wreq/reqwest/其他 HTTP(S) 请求路径，取消 stealth 开关并保留安全、取消和 Worker 独立池语义，见 TODO OB-012/014/016/044。当前双后端是待清理事实，不是目标架构；官方客户端自有网络的支持边界见 OB-033。历史迁移过程可在 `e67e67b` 的同名文件查看。

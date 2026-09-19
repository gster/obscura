# 当前传输实现与统一目标

2026-09-20 复核。产品 Page、CDP、MCP、Worker、JS fetch/XHR 和独立 runtime/module loader 路径统一使用 persona-owned primp；renderer 默认 cache 已不再自行联网，robots.txt 也进入同一 primp。第二切片已将 `ObscuraHttpClient` 收敛为 policy/context，删除其自有发送 backend、timeout 假配置、项目自有直接 reqwest 依赖和 `obscura-net::wreq_client` alias；CLI `original` 文件和 HTTP 辅助路径也统一走 `StealthHttpClient`，原有 37 项 legacy 网络测试完整迁移到 primp。OB-012 仍不能关闭，因为 raw header 观察仍未取得 wire capture，且 HTTP/proxy 后注入字段、wire casing/跨字段顺序、redirect 中间响应、Worker→Page/CDP、preflight/失败链、公开 HashMap 输入限制、完整响应 body 持久化、Abort/CORS 细节、最终线上请求头观察、CDP Fetch、Beacon、download 和线级校准仍未完成。这里的删除范围是项目自有直接依赖和客户端，不代表整个依赖生态绝对不含 reqwest。旧比较中的 upstream commit、Python 探针和现场请求只代表迁移前历史，不是本次传输测量。

## 当前源码事实

- [stealth_transport.rs](../crates/obscura-net/src/stealth_transport.rs) 选择 Windows Chrome145、macOS Chrome152/153 preset，清理 primp 自动默认头，由引擎补身份和请求语义。
- 显式 `no_proxy`、禁止自动 redirect、自有 Cookie/URL/响应体策略；配置代理经选定路径使用。CORS 预检不补 Client Hints 的回归在同文件。
- [vendor/primp](../vendor/primp/) 保留显式 DNS resolver 优先的本地修改；代理端解析目的地址时，不能以本地 resolver 声称最终 IP 已验证。
- `retry::never()` 关闭外层重试，不代表 vendor 内所有协议级恢复都被禁用。请求写出状态未知时不得在引擎增加盲目重放。
- 独立 runtime 初始化时固定默认 Windows Chrome145 persona；frame/Worker 继承身份、策略和 Cookie，Worker 采用 detached primp 独立池。context baseline、Page override 和 request-specific headers 分层合并，兄弟 Page 互不污染。
- module、图片、字体、样式和 CORS OPTIONS 不再从独立 runtime 回退到其他项目自有 HTTP backend。无效代理 fail closed；PEM/DER CA、scripted 请求逐跳 timeout 和 body cap 保持生效。
- `HeaderCapture`/`RawHeader` 从 primp 边界贯穿 `Response`、`RequestInfo`、JS/render/Page 到 CDP；`rawHeaders` 采用 `captureStage=transportRequest|transportResponse`、`encoding=base64`、`fields=[{nameBase64,valueBase64}]`。重复值、非 UTF-8、Cookie、Authorization、Set-Cookie 原始 bytes 不脱敏、不裁剪，兼容 map 是派生视图。
- [vendor/primp-rustls](../vendor/primp-rustls/) 带本地 Chrome 扩展顺序修改；它不是未跟踪的临时目录，也不等于上游未修改依赖。

## 验证边界

本轮 raw header 结果不宣称 wire capture；仍待处理 HTTP/proxy 后注入字段、wire casing/跨字段顺序、redirect 中间响应、Worker→Page/CDP、preflight/失败链、公开 HashMap 输入限制、完整响应 body 持久化，以及 Abort/CORS 细节、最终线上请求头观察、CDP Fetch、Beacon、download 和线级校准。

本次没有重跑 TLS/H2 抓包、Chrome 线级对照或网站验收。历史 ALPS/trust-anchor/头顺序差异需要在固定构建上重新采集才能声明当前数值。JA3/JA4、版本名、单次导航成功不证明所有资源类别的传输等价；具体 method/body、redirect、credentials、预检、代理和连接复用分别验收。

运行时 stealth 开关和产品双传输分支已经删除。仍需按 OB-012/014/015/016 完成仓库其余自有 HTTP(S) 出口盘点、线级校准和完整 persona 编译器；重复/非 UTF-8 头、完整响应持久化、Abort/CORS 细节、最终线上请求头观察、CDP Fetch、Beacon 和 download 仍需处理。所有采集与日志保留原始 Cookie、Authorization、重复头、原始字节和完整 body，不做脱敏或字段裁剪。官方客户端自有网络的支持边界见 OB-033。历史迁移过程可在 `e67e67b` 的同名文件查看。

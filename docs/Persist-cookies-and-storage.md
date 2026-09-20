# Cookie 与 Web Storage

当前 `--storage-dir` **只持久化 Cookie**，不保存 localStorage、sessionStorage、IndexedDB 或活跃 JS 状态。旧文档声称存在 `localStorage/<origin>.json`，与当前 [BrowserContext](../crates/obscura-browser/src/context.rs) 不符。

```bash
obscura serve --host 127.0.0.1 --storage-dir ./obscura-data
obscura fetch https://example.com --storage-dir ./obscura-data --dump text
```

`--storage-dir` 放在 `serve` 或 `fetch` 子命令之后。`fetch --dump original` 和 `--file` 批量原始下载绕过 BrowserContext，不使用此 Cookie 持久化路径。

`cookies.json` 由 [CookieJar](../crates/obscura-net/src/cookies.rs) 的 `save_to_file/load_from_file` 处理。当前磁盘格式是 version 1 envelope：顶层为 `version` 和 `cookies`，每条内部记录使用 snake_case 字段并保留 `name`、`value`、`domain`、`path`、`host_only`、`secure`、`http_only`、`expires`、`same_site`。新程序加载时兼容历史的裸 `CookieInfo` 数组；旧数组使用协议视图的 camelCase 字段、没有 `host_only`，因此按其历史的 domain-scoped 语义导入，负的 `expires` 也保持旧的 session 语义。旧程序不保证能够读取新 version 1 envelope。当前内存匹配键为 domain/name/path，host-only 语义在设置、请求匹配和 version 1 持久化往返中保留。

连接级持久化不会再把连接状态投影成丢失内部字段的 `CookieInfo`：`CookieJar::snapshot` / `from_snapshot` 使用保留内部匹配状态的快照，CDP server 在连接关闭时通过 `apply_snapshot_delta` 合并本连接相对初始快照的新增、替换和删除。快照保留已自然过期但尚未移除的内部记录，避免把时间流逝误认为显式删除、清掉其他连接已刷新的值；保存文件和从快照建立新 jar 时仍过滤过期项。该路径仍只涉及 Cookie；CDP/MCP 对外状态导入导出的完整资格尚未完成，也没有在本文中把内部快照接口当作正式 `storage_state` 契约。

内存 localStorage 按 BrowserContext/origin 管理，sessionStorage 按页面/origin 管理。刷新保留内存状态与重启后恢复是不同能力。关闭 CDP 连接会释放其页面和运行时；新连接不延续活跃文档。共享磁盘目录也不等于共享活跃上下文。

Cookie 保存调用分布于导航、Cookie 修改和连接/进程清理路径；部分调用不向使用者传播写入错误。使用方应实际检查文件和恢复结果，不能凭退出码或“登录过一次”认定完整状态已保存。不同身份使用不同私有目录，但文件权限、加密、schema 迁移及正式客户端 storage_state 资格仍需单独实现与验证。

OB-011 仍未关闭。`get_cookie_header` 当前只接收 URL，尚未接入完整的 SameSite 请求上下文（站点、方法和导航/请求类型），分区 Cookie 也未完成；因此 version 1 的无损内部持久化不等于完整 Cookie 请求语义或分区状态支持。验证结果见下文。

HTTP Set-Cookie 与 document.cookie 均在属性解析后应用最后一个有效 `Max-Age`，优先于 `Expires`，后续非法值不清除先前有效值。剩余请求语义还包括同名多 Path 的发送排序（长 path 优先及 creation tie-breaker）和相关回归。

本轮验证：Cookie/context/server 相关 release 回归在 render 根门禁中 **53/53**，no-render 聚焦 **53/53**；最终根 release nextest **1926/1926**，4 skipped。render 与 no-default-features exact CLI release build 均成功；冻结 render 二进制通过 CI 固定 benchmark 障碍课 **33/33**、官方 Playwright Python **1.60.0** smoke 和 **37-method** 协议画像校验。二进制 SHA-256 为 `722ef38dcb23e1627271bb24809c2bf93855e652fc54eb9dd0438fe78c559ca4`（119072256 bytes）。Astra light 最终复核 0 blockers；`git diff --check` 通过。首轮根门禁曾出现 MCP `test_evaluate` 空标题失败，未改源码的单项重放 **1/1** 和完整复跑均通过；该测试忽略导航响应，现有证据不足以确定偶发根因，未将重跑通过称作修复。

额外真实 CLI 回归使用本地 HTTP fixture 和四个全新进程：HTTP Set-Cookie 的 `hostonly=alpha=beta` 与 document.cookie 的 `docvalue=from-document` 均经 version 1 文件恢复并出现在后续服务端 Cookie 请求头；持久化记录保留 host_only 和 HttpOnly。该探针验证产品存储入口，子域作用域由上述 Cookie 回归覆盖。

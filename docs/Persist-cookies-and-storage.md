# Cookie 与 Web Storage

当前 `--storage-dir` **只持久化 Cookie**，不保存 localStorage、sessionStorage、IndexedDB 或活跃 JS 状态。旧文档声称存在 `localStorage/<origin>.json`，与当前 [BrowserContext](../crates/obscura-browser/src/context.rs) 不符。

```bash
obscura serve --host 127.0.0.1 --storage-dir ./obscura-data
```

`cookies.json` 由 [CookieJar](../crates/obscura-net/src/cookies.rs) 的 `save_to_file/load_from_file` 处理。当前字段包括 name/value/domain/path/secure/http_only/same_site/expires；没有格式版本和 host-only 标记。导入把记录设为 domain-scoped，因此 host-only Cookie 往返会扩大子域匹配范围。不要把这个格式视为稳定、无损的 profile 备份；修复在 TODO OB-011。

内存 localStorage 按 BrowserContext/origin 管理，sessionStorage 按页面/origin 管理。刷新保留内存状态与重启后恢复是不同能力。关闭 CDP 连接会释放其页面和运行时；新连接不延续活跃文档。共享磁盘目录也不等于共享活跃上下文。

Cookie 保存调用分布于导航、Cookie 修改和连接/进程清理路径；部分调用不向使用者传播写入错误。使用方应实际检查文件和恢复结果，不能凭退出码或“登录过一次”认定完整状态已保存。不同身份使用不同私有目录，但文件权限、加密、schema 迁移及正式客户端 storage_state 资格仍需单独实现与验证。

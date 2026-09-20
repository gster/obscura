# 独立 runtime 迁移说明

此前版本曾提供独立的 `runtime/` workspace、NDJSON 动作协议和配套
Python SDK。它们已从当前产品树删除；本页只保留迁移背景，不再提供构建、
wheel 或旧协议命令。

当前使用方式是先在宿主进程启动根 CLI 的 CDP 服务，再由未修改的官方
Playwright Python 客户端通过 `BrowserType.connect_over_cdp` 连接。入口和示例
见 [项目 README](../README.md) 与 [Playwright 接入](Use-with-Playwright.md)。

旧 runtime/SDK 的测试数字、路径和消费仓库 pin 属于历史记录，不能作为当前
产品资格；当前门禁以根 workspace、CDP profile 和官方客户端 smoke 为准。

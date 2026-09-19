# 当前页面捕获与观察边界

render 构建提供 `Page.captureScreenshot` 和 `Page.startScreencast`；接收 screencast 时须用帧内 sessionId 调用 `Page.screencastFrameAck`。客户端示例见 [Playwright](Use-with-Playwright.md#screencasting) 和 [Puppeteer](Use-with-Puppeteer.md#screencasting)。

捕获必须通过**拥有该页面的同一 CDP 连接**中的 page session。当前 server 为每个连接创建独立上下文和页面注册表，新的 viewer 连接不能发现另一客户端的 target。`obscura fetch` 和 MCP 也不会把其页面注册到独立 `serve` 连接。

旧版“另开 viewer 即可观看任意 agent 页面”的脚本及 `tools/live-view.mjs` 不具备这种跨连接保证，不能作为已验证使用方式。需要监看时，由 owner 客户端转发其捕获结果；不要据一次新连接的空白截图判断正在执行的页面状态。

screencast 是活动驱动的页面图像，不是桌面视频或录屏。新方向将其扩展冻结；跨 owner 诊断须先明确权限和会话模型，见 TODO OB-028/034。

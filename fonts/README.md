# 固定中文回退字体

`NotoSansCJKsc-Regular.otf` 是未修改的 Noto Sans CJK SC Regular 2.004，来自官方 [Sans2.004](https://github.com/notofonts/noto-cjk/tree/523d033d6cb47f4a80c58a35753646f5c3608a78)，固定 commit `523d033d6cb47f4a80c58a35753646f5c3608a78`。

- 原文件：`Sans/OTF/SimplifiedChinese/NotoSansCJKsc-Regular.otf`。
- 文件大小：16437364 bytes。
- SHA-256：`2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b`。
- 许可：同一 commit 根目录的 [LICENSE](https://github.com/notofonts/noto-cjk/blob/523d033d6cb47f4a80c58a35753646f5c3608a78/LICENSE)，原文随本目录保留；版权信息同时保留在未修改字体的 name 表中。

字体整体嵌入 Runtime，不裁剪成 fixture 字符集，也不在运行时下载。更新时同时核对来源/许可、`persona-fonts.json` 和构建门禁，并重新验证 ARM64/Linux 绘制。新增常规字重的回退不代表完整中文粗体/斜体或商业 Windows 字体的度量一致性。

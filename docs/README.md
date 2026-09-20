# 文档入口

- [项目 README](../README.md)：定位、构建与当前使用入口。
- [SUMMARY](SUMMARY.md)：核验后的现状、测试边界和完整文档索引。
- [TODO](TODO.md)：唯一开发执行清单。
- [当前 TODO goal 交接](Goal-handoff.md)：当前实现基线、验证证据和下一阶段执行入口。
- [New_ACH](New_ACH.md)：未来架构与迁移门槛。
- [Persona 修复实施记录](Persona-fix-plan.md)：persona 必配、注入、冻结与一致性修复的设计、实现范围和验收记录。

MCP 和有用 CLI 保留；自有 Python SDK/私有 runtime 已从当前产品树移除，历史迁移说明保留在[独立 runtime 迁移说明](Use-the-isolated-runtime.md)。统一 persona、强制 stealth、唯一 primp 出口与 Chrome 差异修补是重点，Southwest shopping 不再 403 是重要业务验收。各操作指南区分当前用法与尚未实现的目标。上游发行包、旧现场成功和历史测试记录不构成本 fork 当前版本的发布资格。

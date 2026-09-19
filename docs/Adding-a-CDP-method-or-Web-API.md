# 新增 CDP 方法或 Web API

先确定可复现的契约和错误行为，再改实现；开发方向及范围见 [TODO](TODO.md)。不要以空对象或固定 true 让调用方误以为能力已支持。

## CDP

1. 在 `crates/obscura-cdp/src/domains/` 增加或扩展域处理器，并在 `domains/mod.rs` 声明模块。
2. `dispatch.rs` 先拆分 `Domain.method`，再按 **domain** 路由到处理器。新增 match arm 应匹配域名，不能把 `MyDomain.doThing` 放进域名分支。
3. 校验参数、session/context/realm 归属、句柄寿命；明确 result、协议错误、页面异常与事件顺序。
4. 在该 crate 的 tests 中复用真实 dispatch/HTTP fixture，覆盖成功、非法输入和生命周期边界。
5. 更新方法级支持表及固定客户端回归；当前整域 no-op 是待修缺口，不是新增方法模板。

参考：[dispatch](../crates/obscura-cdp/src/dispatch.rs)、[Page](../crates/obscura-cdp/src/domains/page.rs)、[CDP tests](../crates/obscura-cdp/tests/)。

```bash
cargo nextest run --locked --release --features render -p obscura-cdp
```

## Web API

JS 表面在 `obscura-js/js/bootstrap.js`，原生边界在 `src/ops.rs`，扩展注册与 runtime 在同 crate 内。只有真实副作用或需要原生状态的部分才下沉 ops；先检查已有入口，避免重复实现。

测试应覆盖描述符/brand、参数转换、成功与错误、异步事件时序及适用的 window/frame/Worker。Promise 包装不能修复错误时序，`[native code]` 外观也不能代替实现。未知 crypto 算法应按 Web API 契约拒绝，不能像旧示例那样默默降为 SHA-256。

ops 不得 unwind 进入 V8。遵守 watchdog、SSRF、DOM 防环与 release unwind 约束。新增测试使用 release nextest；bootstrap 修改会在构建期快照生成中执行，但快照构建成功只证明初始化通过。

完成标准及完整门禁见 [AGENTS](../AGENTS.md) 与 [测试指南](Testing-and-debugging.md)。不要全仓库 cargo fmt。

# 当前 Worker 实现与资格边界

2026-09-19，源码基线和测试结果见 [SUMMARY](SUMMARY.md)。旧文档混合“同 isolate”“开发工作树待合入”“已经独立执行”等不同阶段，本页只描述当前实现。

## 已存在的实现

- `obscura-js/src/worker.rs` 为 Worker 建立独立 OS 线程、current-thread Tokio runtime 和 V8 isolate；父页面同步忙碌时 Worker 可独立推进。
- 消息采用 V8 序列化，覆盖 typed array、BigInt、Date、Map、Set、循环/共享引用。ArrayBuffer transfer 当前复制后 detach，不声称零拷贝。
- 启动队列、一次初始化、handler 状态、消息顺序、close/terminate 与 owner 清理有回归。
- `worker_queue.rs` 使用页面树共享预算：单消息 16 MiB、队列 payload 64 MiB、4096 条、32 个活跃 Worker。payload 预算不等于进程 RSS 上限。
- Worker 使用 detached primp 客户端，配置和 Cookie 保留、连接池独立。旧版“直接复用父 transport”及双产品传输路径均已过时。
- 网络拦截、计数、正文与观察向 owner 转发；Worker 对象句柄不能冒充父 isolate 句柄。

源码：[Worker](../crates/obscura-js/src/worker.rs)、[队列](../crates/obscura-js/src/worker_queue.rs)、[回归](../crates/obscura-js/src/runtime.rs)。

## 不据此承诺的能力

完整 module Worker、data URL 行为、SharedWorker/Service Worker、MessagePort/浏览器 host-object clone、SharedArrayBuffer 安全暴露、OffscreenCanvas 2D/导出/transfer 以及完整 WindowProxy/frame 标准都需要逐项资格。当前 OffscreenCanvas WebGL 子集和 context flags 存在，不应照抄旧“全部缺失”结论。

```bash
cargo nextest run --locked --release --features render -p obscura-js -E 'test(worker)'
```

涉及身份或传输的改动必须运行强制 primp 路径；单纯构建或普通 HTTP fixture 不证明 TLS 行为。Web Worker 与批量抓取的 `obscura-worker` 可执行文件是不同概念，产品裁剪不能混删。任务见 OB-038。

Southwest 固定输入回放和现场结果是不同验收；通用 Worker 测试通过不解释 shopping 403。历史实验的限制见 [Southwest 摘要](Southwest-handoff.md)。

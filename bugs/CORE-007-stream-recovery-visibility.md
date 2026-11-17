# CORE-007：流恢复通知与等待预算

- 状态：源码修改及聚焦测试通过；提交随本记录；未打包安装。
- 责任层：model-provider-info 默认值、core Responses 重试控制。
- 已确认：release 隐藏首次 WebSocket 重试，传输切换清零次数，默认流空闲时限为一小时。
- 修复：首次重试及传输切换发送既有 StreamError 生命周期；传输切换保留次数；首次失败后的恢复决策受 15 分钟预算约束，超预算的 Retry-After 不提前执行而返回原始错误；默认流空闲恢复为五分钟，显式值不变。
- 预算只约束单个采样请求的重试决策和等待，不强杀正在运行的请求或工具；正常工具轮次不计入该预算。显式超时仍可导致单次请求等待超过默认值。
- 工具已开始执行时沿用既有输出记录与 follow-up，未重放整轮或引入前端自动续发。工具部分输入的现有禁止重试边界保持不变。
- 回归：responses_retry::tests 覆盖首次通知等待、预算耗尽、Retry-After 超预算；model-provider-info 的 stream_idle_timeout_is_bounded_by_default_and_preserves_explicit_override 覆盖默认值和显式覆盖。
- 验收边界：真实 DNS/TLS 故障、包内和安装后验收未执行；恢复证据以可选字段兼容旧历史；不引入工具重放协议。

## 本地验证

- 最终源码 `cargo test -p codex-core --lib responses_retry::tests --offline`：6 项通过。
- 最终源码 `cargo test -p codex-model-provider-info --lib --offline`：50 项通过。
- 测试编译关闭宿主不可用的 sccache wrapper；未修改用户配置。core_test_support 现有 Atomic::fetch_update 弃用警告仍存在。
- responses_retry.rs 本轮 253 → 362 行，预算校验、内容过滤历史写入与耗尽建议分别保持独立职责；provider lib.rs 985 → 986 行，仅修改既有默认值与说明。
- 未执行全内核测试或真实上游故障注入；未新增数据库迁移；恢复证据使用可选字段扩展，旧历史工具结果与避免重放分支保持原有实现。

- 持续连接重试分支遵守 Retry-After，并使用通知前确定的绝对等待期限；延迟不被通知交付时间再次延长。

- 审核后 `cargo check -p codex-core --lib --offline` 通过；聚焦恢复 6 项、provider 配置 50 项通过。


## 结构化恢复证据

Responses 采样与 remote compaction v2 的 StreamError 提供可选 recovery：本地 requestId、attempt、可空 maxAttempts、retryAtMs、budgetDeadlineMs、reason。普通计数、连接恢复及传输切换沿用各自策略；时间字段仅用于展示，单调时钟继续拥有调度。连接等待没有次数上限时明确 null，不伪造最大次数。

app-server 同步转发 error 通知，并由有序 ThreadState 保存当前轮恢复快照。真实模型输出和终态清除，并发工具完成与旧轮事件不清除。旧 StreamError 无 recovery 仍可读取，未来 reason 与未知扩展字段往返保留。

已知 output_item.added/done 的 item 损坏或缺失返回 MalformedResponse。复用现有非重试分类，错误不包含原始 payload；HTTP 和 WebSocket 共享该处理。未知扩展 item 不被拒绝。

新增回归：responses_item_tests、thread_state::recovery_tests、stream_recovery::tests，以及 responses_retry::tests 的身份与截止时间校验。全内核、安装版和真实 provider 验收仍未覆盖。旧本地压缩 retry 通知保留 recovery 缺失的兼容路径。

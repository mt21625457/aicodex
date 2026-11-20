# CORE-007：流恢复通知与等待预算

- 状态：源码修改及聚焦测试通过；提交随本记录；未打包安装。
- 责任层：model-provider-info 默认值、core Responses 重试控制。
- 已确认：release 隐藏首次 WebSocket 重试，传输切换清零次数，默认流空闲时限为一小时。
- 修复：首次重试及传输切换发送既有 StreamError 生命周期；传输切换保留次数；首次失败后的恢复决策受 15 分钟预算约束，通知交付与退避唤醒后再次校验预算，超预算的 Retry-After 不提前执行而返回原始错误；默认流空闲恢复为五分钟，显式值不变。
- 预算只约束单个采样请求的重试决策和等待，不强杀正在运行的请求或工具；正常工具轮次不计入该预算。显式超时仍可导致单次请求等待超过默认值。
- 工具已开始执行时沿用既有输出记录与 follow-up，未重放整轮或引入前端自动续发。工具部分输入的现有禁止重试边界保持不变。
- 回归：responses_retry::tests 覆盖首次通知等待、预算耗尽、Retry-After 超预算、三条恢复路径通知延迟超预算以及退避唤醒超预算；model-provider-info 的 stream_idle_timeout_is_bounded_by_default_and_preserves_explicit_override 覆盖默认值和显式覆盖。
- 验收边界：真实 DNS/TLS 故障、包内和安装后验收未执行；恢复证据以可选字段兼容旧历史；不引入工具重放协议。

## 本地验证

- 最终源码 `cargo test -p codex-core --lib responses_retry::tests --offline`：8 项通过。
- 最终源码 `cargo test -p codex-model-provider-info --lib --offline`：50 项通过。
- 测试编译关闭宿主不可用的 sccache wrapper；未修改用户配置。core_test_support 现有 Atomic::fetch_update 弃用警告仍存在。
- responses_retry.rs 当前 381 行（此次复审 362 → 381），预算校验、内容过滤历史写入与耗尽建议分别保持独立职责；provider lib.rs 985 → 986 行，仅修改既有默认值与说明。
- 未执行全内核测试或真实上游故障注入；未新增数据库迁移；恢复证据使用可选字段扩展，旧历史工具结果与避免重放分支保持原有实现。

- 持续连接重试分支遵守 Retry-After，并使用通知前确定的绝对等待期限；延迟不被通知交付时间再次延长。

- 审核后 `cargo check -p codex-core --lib --offline` 通过；聚焦恢复 8 项、provider 配置 50 项通过。


## 结构化恢复证据

Responses 采样与 remote compaction v2 的 StreamError 提供可选 recovery：本地 requestId、attempt、可空 maxAttempts、retryAtMs、budgetDeadlineMs、reason。普通计数、连接恢复及传输切换沿用各自策略；时间字段仅用于展示，单调时钟继续拥有调度。连接等待没有次数上限时明确 null，不伪造最大次数。

app-server 同步转发 error 通知，并由有序 ThreadState 保存当前轮恢复快照。真实模型输出（含未结束的工具参数开始/增量）和终态清除，并发工具完成与旧轮事件不清除；参数 finished 同时用于错误清理，不能证明连接恢复。旧 StreamError 无 recovery 仍可读取，未来 reason 与未知扩展字段往返保留。

已知 output_item.added/done 的 item 损坏或缺失返回 MalformedResponse。复用现有非重试分类，错误不包含原始 payload；HTTP 和 WebSocket 共享该处理。未知扩展 item 不被拒绝。

新增回归：responses_item_tests、thread_state::recovery_tests、stream_recovery::tests，以及 responses_retry::tests 的身份与截止时间校验。全内核、安装版和真实 provider 验收仍未覆盖。旧本地压缩 retry 通知保留 recovery 缺失的兼容路径。

## 0.171.0 合并回归

API 桥接层此前在提前返回 HTTP 503 专用错误时跳过了 `x-should-retry: false`。重试禁令现在在错误分类后统一生效，保留容量不足、服务端过载和限速分类，以及已经捕获的 `Retry-After` 截止时间。上游 WebSocket 消息大小错误触发 HTTP 回退时也复用同一重试策略，显式关闭重试同时禁用回退。

回归入口为 `server_do_not_retry_header_applies_to_classified_capacity_errors`（codex-api）和 `disabling_retry_also_disables_websocket_http_fallback`（codex-protocol）。

本机 codex-api 单元测试 288 项、集成测试 36 项和 codex-protocol 单元测试 381 项全部通过，包含 503 重试禁令、WebSocket 回退禁令和畸形输出回归。

连接恢复使用同一次单调时钟采样计算绝对等待时刻和计划延迟，避免遥测将 5 秒退避截断为 4,999 毫秒。集成回归覆盖首次和第二次恢复的递增计数。WebSocket 升级拒绝回归覆盖首次 HTTP 请求成功与首次 HTTP 请求返回过载两种结果，两者均要求等待两次服务端建议，并验证传输切换沿用累计流重试次数。

工作区 Nextest 复跑中，`responses_retry::tests` 8 项及 `suite::retry_after` 42 项全部通过。

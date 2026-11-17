# CORE-007：流恢复通知与等待预算

- 状态：源码修改及聚焦测试通过；提交随本记录；未打包安装。
- 责任层：model-provider-info 默认值、core Responses 重试控制。
- 已确认：release 隐藏首次 WebSocket 重试，传输切换清零次数，默认流空闲时限为一小时。
- 修复：首次重试及传输切换发送既有 StreamError 生命周期；传输切换保留次数；首次失败后的恢复决策受 15 分钟预算约束，超预算的 Retry-After 不提前执行而返回原始错误；默认流空闲恢复为五分钟，显式值不变。
- 预算只约束单个采样请求的重试决策和等待，不强杀正在运行的请求或工具；正常工具轮次不计入该预算。显式超时仍可导致单次请求等待超过默认值。
- 工具已开始执行时沿用既有输出记录与 follow-up，未重放整轮或引入前端自动续发。工具部分输入的现有禁止重试边界保持不变。
- 回归：responses_retry::tests 覆盖首次通知等待、预算耗尽、Retry-After 超预算；model-provider-info 的 stream_idle_timeout_is_bounded_by_default_and_preserves_explicit_override 覆盖默认值和显式覆盖。
- 验收边界：真实 DNS/TLS 故障、包内和安装后验收未执行；此次不改历史格式或增加尝试身份协议。

## 本地验证

- 最终源码 `cargo test -p codex-core --lib responses_retry::tests --offline`：5 项通过。
- 最终源码 `cargo test -p codex-model-provider-info --lib --offline`：50 项通过。
- 测试编译关闭宿主不可用的 sccache wrapper；未修改用户配置。core_test_support 现有 Atomic::fetch_update 弃用警告仍存在。
- responses_retry.rs 209 → 253 行，预算校验、内容过滤历史写入与耗尽建议分别保持独立职责；provider lib.rs 985 → 986 行，仅修改既有默认值与说明。
- 未执行全内核测试或真实上游故障注入；未改变持久化结构，旧历史工具结果与避免重放分支保持原有实现。

- 持续连接重试分支遵守 Retry-After，并使用通知前确定的绝对等待期限；延迟不被通知交付时间再次延长。

- 审核后 `cargo check -p codex-core --lib --offline` 通过；聚焦恢复 5 项、provider 配置 50 项通过。

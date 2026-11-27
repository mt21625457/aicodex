# CORE-009 合并 0.172.0 后的兼容回归

本地 `dev` 为 `05c6bba770`，上游同步为 `43b467f931`。Git 最初标出 32 个冲突路径。合并时保留产品版本 `0.2.12` 和 Cargo 工作区版本 `0.2.11`。

## 根因与修复

- Responses 的出站消息同时接入上游私有连接器结果和本地路由请求预算。私有结果只进入授权的出站消息，随请求一起计入预算。SSE 和 WebSocket 保留本地参数错误终态，接入上游分级支出限额。等待工具公开 schema 和旧会话请求兼容继续按 [CORE-001](CORE-001-reserved-wait-schema-history.md) 验证。
- 上游调整了 app-server 的异步恢复、权限和 Daybreak 设置。恢复路径继续读取本地模型、传输协议、历史及权限覆盖，线程列表继续报告已加载子代理的直接输入能力。
- 实时会话新增落盘操作后，线程存储同时保留原有扩展事件持久化模式。自动合并遗漏了实时写入所需的 rollout 转换导入，已补齐。文件系统保留条件写入契约，接入上游文件元数据序列化。
- 全工作区编译发现自动合并后的测试夹具字段不匹配。补齐 Guardian V3 配置默认值和 Responses WebSocket 请求字段，并消除重复的请求预算字段。
- 稳定和实验协议产物纳入两侧协议变更。旧 framed stdio 代码执行传输已由上游移除，继续使用 gRPC 超时及宿主中断回归覆盖。
- TypeScript SDK 的限流夹具发送结构化 `rate_limit_exceeded`。测试现检查错误类别和提供方原文，匹配内核保留的流内限流语义。

## 验证入口

- `cargo check --workspace --tests --locked --offline` 检查所有工作区成员与测试接口。
- `cargo test -p codex-api --locked --offline` 覆盖合成 SSE 和 WebSocket 事件及请求预算。核心 `wait_` 和 `client_` 聚焦测试继续覆盖公开工具契约及历史请求兼容。
- `codex-app-server-protocol` 的协议产物测试检查稳定与实验导出的一致性。`persisted_resume_settings` 覆盖恢复旧配置，`legacy_completion_backfill_finds_completed_turn_after_newer_turn` 覆盖按需翻页读取正确的终态回合。

## 本机验证结果

- `codex-api` 297 项单元测试和 38 项集成测试通过。首轮发现畸形 `response.failed` 帧绕过结构校验，恢复校验后通过。HTTP 402/403 网关额度映射继续通过，HTTP 429 的明确支出限额按 API 密钥、项目或组织映射。
- `codex-protocol` 383 项通过。`codex-app-server-protocol` 347 项通过，另一个生成入口在常规测试中忽略，并在稳定及实验模式单独运行通过。Python SDK 派生产物与生成结果一致。
- Cargo 锁定解析、Windows GNU 目标离线依赖解析、Bazel 锁定模式下发布目标查询通过。
- 175 个 Cargo 工作区成员的本机测试目标编译检查通过。macOS ARM64 调试 CLI 和 Code Mode host 构建通过。Python SDK 329 项通过，38 项真实服务测试按本机配置跳过。
- TypeScript SDK 构建、lint 和 54 项测试通过，覆盖 MCP 一致性和限流错误原文。首次运行缺少调试 CLI，第二次发现 Code Mode host 缺失和过期的限流断言。补齐对应构件与断言后全套通过。
- Nextest 聚焦矩阵 450 项全部通过，覆盖等待工具及旧会话请求、压缩续跑、Daybreak 与已加载子代理直接输入、扩展事件与实时历史持久化、exec 终态回合分页及 TUI 回放。
- 补充 Nextest 52 项全部通过，覆盖 SSE 和 WebSocket 重试、禁止压缩后的终态、实时输入落盘失败保留同批输入、gRPC 宿主故障恢复及实时历史迁移。两批均关闭测试重试。

## 合并提交复审

复审以 `6d9dc5adf9` 为基线，对照两个父提交及重演合并得到的差异，检查 32 个冲突路径和相邻的自动合并。工作区已有的 Cargo `0.2.12` 版本更新作为本次验证输入保留。

- HTTP 429 同时携带 `usage_limit_reached` 类型与明确的支出代码时，恢复用量窗口、计划类型、重置时间和限速头信息。单独的支出代码仍区分 API 密钥、项目和组织。
- 旧格式的 `session_end_input_saved` 实时回执从落盘数据读入时，转换为带 `SessionEnd` 原因的当前表示。新旧回执均从普通时间线排除。回归覆盖合成 rollout 解码、投影中的旧行和分页边界。
- 旧会话在活动回合修改设置后，压缩可能追加较早的 `TurnContext`。恢复逻辑从同一份较新的快照选取审批策略、审核者与权限配置，并继续限制无归属证明时可以恢复的具体权限。

- 禁用重试的 Nextest 聚焦矩阵 635 项全部通过，含旧实时回执的时间线分页、活动回合设置的恢复、等待工具和旧声明、SSE/WebSocket 重试、压缩续跑、gRPC 宿主故障恢复、条件写入、exec 与 TUI 回放。
- Responses API 335 项通过。补充畸形时间戳用例后，38 项错误映射测试复跑通过。Rust 协议 385 项和 App Server 协议 347 项通过，稳定及实验协议产物保持与生成结果一致。Python SDK 协议和 RPC 测试 109 项通过。
- 修复后的全工作区锁定离线编译检查及格式检查通过。重新生成的 `config.schema.json` 与仓库逐字节一致。Windows GNU 锁定依赖解析、Bazel 锁定模式下 MXC 依赖及发布目标查询，以及 74 项构建和打包脚本测试通过。

## 提交前复核

- 同回合旧设置回查以最近的明确归属快照为边界。回归覆盖更早的无归属设置与更新的外线程快照共存，确保老审批策略和命名权限配置不会越过边界重新生效。
- HTTP 429 用量窗口回归同时检查支出代码和限流头，覆盖活动限额、额度余额标识及限额类型。
- 按提交时的锁文件复跑，Nextest 聚焦回归 113 项、Responses API 335 项、Rust 协议 385 项及 App Server 协议 347 项通过。协议产物一致性和全工作区格式检查通过。可用 `git log -- bugs/CORE-009-upstream-0172-merge-compatibility.md` 定位复核提交。

真实服务、发布包、安装后以及非本机平台的运行验收需分别记录。

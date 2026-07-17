## Why

`WireApi::Chat` 已能把 Codex `ToolSpec` 序列化为 Chat Completions function tools，
把流式 `delta.tool_calls` 恢复为 `ResponseItem`，并用 assistant `tool_calls` +
`tool` role message 完成多轮工具环。但 Chat provider 当前仍沿用通用工具计划：普通
文件读取通常走 shell，修改依赖 `apply_patch` 的 freeform-wrapper function。对于只提供
`/v1/chat/completions` 的兼容网关、本地 OSS 与企业代理，这保留了与 Claude
Compatible 相同的文件 IO 摩擦。

`add-cc-style-file-tools` 将提供经过 workspace、sandbox、审批、PathUri、turn-scoped
read receipt、commit-time fingerprint 和 diff/file-change 路径保护的
`read_file` / `edit_file` / `write_file`。Chat wire 已具备承载普通 JSON function
tools 的协议能力，因此应通过一个**独立 rollout**复用该基础设施，而不是在基础提案
中同时扩大 Claude 与 Chat 的默认行为。

> **审核与依赖门禁**：本变更处于提案阶段。在本 change 的 proposal、design、
> specs、tasks 通过人工审核，且依赖变更 `add-cc-style-file-tools` 的隐藏安全基础
> （Phase A，包括默认关闭的 rollout gate、sampling-step receipt provenance 与同批
> 防绕过测试）实现并验证前，**不得开始本变更的 Rust 实现或改变 Chat model-visible
> 工具面**。

## What Changes

- 为 Chat wire 增加独立 typed policy：
  - `legacy`（默认）：保持当前 Chat 工具计划
  - `dedicated`：广告 dedicated 文件工具，`apply_patch` 仅 hidden dispatch
  - `dedicated_with_apply_patch`：同时广告 dedicated 工具与 `apply_patch`，用于显式
    fallback/诊断
- `chat_file_tool_mode` 固定为顶层 typed `ConfigToml` 字段，默认 `legacy`，并作为
  session-invariant 配置只对新建 session 生效。
- Chat non-legacy mode 必须依赖 `dedicated_file_tools` rollout gate 和三个 handler：
  Chat session 配置解析时若 gate 关闭则失败；最终 tool plan/request metadata 缺少任一
  runtime、declaration 或 reverse mapping 时在首次 HTTP sampling 前失败。两种情况都给出
  可行动错误，不静默降级。
- Chat 请求使用现有 `chat_tool_name(...)` 生成的稳定 hashed wire name；reverse
  metadata 必须把调用恢复为原始 `read_file` / `edit_file` / `write_file` function
  identity。不得为本 rollout 改写既有 Chat name 算法或历史名称。
- 仅当显式 Chat non-legacy mode 已选择，且三个 dedicated 工具实际出现在 Chat request
  时，通过 `core/context` 中有硬上限的 `ContextualUserFragment` 追加使用实际 mapped
  wire names 的 prefer-dedicated 指引；不得仅凭同名第三方工具或只告诉模型调用一个请求
  中不存在的未映射名称。
- dedicated tool call 继续使用依赖提案的 PathUri、receipt、审批、sandbox、编码与
  reviewable mutation 契约，不在 Chat adapter 内复制文件逻辑。
- 增加 mock Chat 端到端测试：请求真值表、hashed name/reverse map、fragmented
  `tool_calls`、`tool_call_id` 续轮、read→edit→edit、create→edit、stale error、
  hidden legacy history、远程 executor。
- Responses 与 Claude 工具策略不因 Chat mode 改变。
- 在 `docs/config.md` 记录 Chat rollout mode、依赖、兼容性风险与回滚；配置 shape
  变化时同步 regenerate schema。

## Capabilities

### New Capabilities

- `chat-file-tool-rollout`：Chat Completions provider 可显式选择 legacy、dedicated、
  dedicated-with-apply-patch 文件工具面；选择过程 fail-closed、可回滚，且不改变其他
  wire。

### Modified Capabilities

- `chat-completions-wire-api-support`：Chat function-tool serialization 和多轮 tool
  history 必须支持 dedicated 文件工具的稳定 wire name、reverse mapping、执行与
  `tool_call_id` 续轮。

## Dependencies

- **Hard dependency**：`add-cc-style-file-tools`
  - Phase A 默认关闭的 `dedicated_file_tools` gate、hidden handlers、bounded turn
    receipt、environment-aware PathUri、共享 reviewable mutation 与 local/remote tests
    已实现
  - receipt 带 originating sampling-step identity；同一 provider response/batch 内的
    Read/Create 不得授权同批 Edit/Write，且对应安全测试已通过
  - dedicated tool schemas 与输出上限已锁定
- **Existing Chat foundation**：`unify-multi-backend-sampling-normalization`
  - `WireApi::Chat` 路由、Chat request/history adapter、function tool serializer、
    reverse metadata、SSE tool-call accumulator 与 mock tool loop 已存在

本变更不得复制或重新定义上述能力；若依赖契约需要改变，先回写依赖 change，再更新
本 change。

## Impact

- 预计受影响：
  - `codex-rs/core` — Chat tool visibility policy、bounded context fragment、mapped-name
    guidance、spec-plan truth table、mock tool loop tests
  - `codex-rs/tools` — 原则上只增加 dedicated Chat serialization assertions；仅当
    现有普通 function-tool serializer 暴露真实缺口时才改实现
  - `codex-rs/core/config.schema.json` — typed Chat mode 与 schema
  - `docs/config.md` — 内部 rollout 与回滚说明
- 明确不改：
  - dedicated handlers、receipt、PathUri、mutation runtime 的语义
  - `chat_tool_name` hashing/sanitization 算法
  - Chat SSE parser 的通用 function-tool shape
  - Responses / Claude model-visible tool defaults
  - Chat endpoint、auth、reasoning、usage 与 error mapping
- 主要风险：
  - 部分 Chat-compatible provider 宣称支持 tools，但 function calling/schema 遵从性弱
  - prompt 使用未映射 semantic name，模型调用不存在的 function
  - 隐藏 `apply_patch` 后历史 assistant tool call 名称被重写，破坏续轮
  - dependent Read/Edit 被放入同一个 parallel batch，产生多余 read-first 失败
  - 自动 fallback 重试可能重复一轮有副作用的调用
- Mitigations：
  - 默认 `legacy`，只显式 opt-in；不做 provider 猜测或自动降级
  - guidance 使用 serializer 返回的实际 mapped names
  - 历史重放继续使用确定性 name + reverse metadata，测试 reordering/hidden tool
  - 依赖调用必须跨 Chat completion 顺序执行；runtime 用 sampling-step provenance
    fail-closed，失败保持无写入并返回可纠正 tool result
  - provider 拒绝 tools/schema 时直接报告错误，由用户切回 `legacy`
- 回滚：设置 `chat_file_tool_mode = "legacy"`；不删除共享 handlers，也不影响 Claude/
  Responses。
- 验证命令（实现阶段，审核通过后）：
  - `cd codex-rs && just test -p codex-tools`（仅当 tools crate 变更）
  - `cd codex-rs && just test -p codex-core`（Chat request/tool-loop tests）
  - `cd codex-rs && just write-config-schema`（配置/schema 变更）
  - 按 `$remote-tests` 运行 Linux remote core tests
  - `bazel test //codex-rs/core:core-all-wine-exec-test`
  - `openspec validate enable-chat-dedicated-file-tools --strict`

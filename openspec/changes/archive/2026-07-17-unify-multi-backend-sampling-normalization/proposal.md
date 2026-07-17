## Why

Codex 当前已具备双后端推理路径（OpenAI Responses + Anthropic Claude Messages），并在
`codex-api` 边界将 SSE 归一为内部 `ResponseEvent`，供 `codex-core` turn 循环消费。
但 OpenAI Chat Completions（`wire_api = "chat"`）已被显式移除，导致大量仍只提供
`/v1/chat/completions` 的兼容网关、本地 OSS、以及部分企业代理无法接入；同时
Responses / Claude 的适配器虽已存在，却尚未按「L1 原始流 / L2 纯变换 / L3 编排」
显式分层，后续再加协议时容易把后端细节渗入 turn 循环。

本地参考实现 `grok-build`（`xai-grok-sampler`）证明了一条可复用路径：以
`ApiBackend::{ChatCompletions, Responses, Messages}` 选择协议，经 L2 统一成单一
事件流，再由上层会话/ACP 消费。本仓库应**借鉴其分层与 Chat L2 设计**，但**不**
引入第二套 `SamplingEvent` 脊柱，也不照搬 `SamplerActor` / ACP 宿主模型——继续以
现有 `ResponseEvent` / `ResponseItem` 作为 Codex 内部归一事件，把三后端流统一到
同一消费面。

> **审核门禁**：本变更处于提案阶段。在提案、design、specs、tasks 通过人工审核前，
> **不得开始实现代码**。

## What Changes

- 明确并固化 **L1 / L2 / L3** 采样分层边界：
  - L1：后端原生 HTTP/SSE 客户端（Responses / Claude Messages / Chat Completions）
  - L2：纯变换适配器，把原生流映射为 `ResponseEvent`（无 retry I/O、无 turn 逻辑）
  - L3：`ModelClientSession` + turn 编排（可选抽取纯错误分类，不强制引入完整 Actor）
- **恢复** `WireApi::Chat`（或等价命名），支持 `POST …/chat/completions` 流式路径。
- 新增 Chat Completions：
  - 请求/history 适配器（`Prompt` → Chat messages）
  - 工具声明序列化与流式 `tool_calls` 反构为 `ResponseItem`
  - SSE L2 变换器（text / reasoning / tool delta / usage / completed / error）
- 将现有 Responses 与 Claude 适配器文档化并校准为同一 L2 契约（不改 Claude 线协议语义）。
- 引入 content-aware idle（keepalive ≠ 进度）与可测试的错误分类约定（对齐 grok 行为语义）。
- 所有 model-provider 协议与 transport（Responses HTTP/WebSocket、Claude Messages、
  Chat Completions、Realtime HTTP/WebSocket）统一发送精确的
  `User-Agent: aicodex/<workspace package version>`；provider、extra 或 auth header 不得覆盖该
  产品标识。Claude `count_tokens` 等同协议辅助请求遵守同一约定。
- 为 Chat Completions 增加 mock 端到端工具环测试；为三后端共享一份 L2 一致性检查清单。
- 在 `docs/config.md` 新增 Chat/Responses/Claude 选择的内部行为说明（该文件目前未记载
  `wire_api`，本次为**新增**而非恢复；用户向教程不在 `docs/` 落地），并按需 regenerate
  `config.schema.json`。
- **不**在本变更中实现 ACP / Leader / Computer Hub 宿主协议；它们与采样归一正交，另开提案。

## Capabilities

### New Capabilities

- `multi-backend-sampling-normalization`：Codex 以 `ResponseEvent` 为唯一 agent 面向采样事件流；Chat Completions、OpenAI Responses、Anthropic Messages 均通过 L2 适配器归一到该流；L1/L2/L3 边界可验证。
- `chat-completions-wire-api-support`：Codex 可路由配置为 Chat Completions 的 provider，经 `/v1/chat/completions` 完成流式采样、工具环与错误传播，且不进入 Responses/Claude 解析器。
- `model-provider-routing`：provider 配置可再次选择 Chat Completions wire API；`wire_api = "chat"` 反序列化不再以硬错误拒绝；默认仍为 Responses。远程/托管 thread config 的 proto 枚举本期保持不变。

### Modified Capabilities

- `claude-wire-api-support`：补充「Claude 适配器作为 L2 实现」的边界要求——Claude 线协议行为不变，但必须满足共享 L2 纯变换与 content-aware idle 契约（不把 Claude 逻辑迁入 Responses 路径）。

## Impact

- 受影响 crate：
  - `codex-rs/model-provider-info` — `WireApi` 枚举与反序列化
  - `codex-rs/model-provider` — auth / capabilities（如需要）
  - `codex-rs/codex-api` — Chat client、SSE L2、错误映射；Responses/Claude 边界文档与校准
  - `codex-rs/codex-client` — 不作为本期 sampling idle 的实现落点；当前 Responses/Claude
    均在 `codex-api/src/sse/*` 直接消费 byte stream，保持通用 helper 行为不变
  - `codex-rs/tools` — Chat Completions 工具声明序列化
  - `codex-rs/core` — `ModelClientSession` 分发、`Prompt`→Chat 请求适配、turn 循环 wire 分支行为矩阵核对（见 design 决策 11）
  - `codex-rs/config` / `docs/config.md` — 配置与内部行为说明
  - `codex-rs/core/config.schema.json` — 若 schema 变更需 regenerating
- 不受影响（本变更范围外）：
  - app-server / MCP server 宿主协议形状
  - ACP / Leader / Computer Hub
  - 把 `ResponseEvent` 重命名为 `SamplingEvent`
  - 引入 grok 的 `SamplerActor` 作为默认编排
  - `codex-rs/ollama` crate 与 `LEGACY_OLLAMA_CHAT_PROVIDER_ID` / `OLLAMA_CHAT_PROVIDER_REMOVED_ERROR` 文案（不恢复 `ollama-chat` provider id）
  - 远程/托管 thread config proto（`codex.thread_config.v1.proto` 的 `enum WireApi`）：本期不新增 `WIRE_API_CHAT`；未知 wire_api 仍报错
- 主要风险：
  - Chat 历史与多轮 tool_calls 映射错误导致工具环断裂
  - reasoning / content 字段供应商差异导致漏流或串道
  - 恢复 `chat` 配置后与旧迁移文案/文档冲突
  - 若把 Chat 逻辑塞进 Responses parser，破坏既有 Claude/Responses 隔离
  - 测试只覆盖单元序列化而未跑 mock tool loop 会假通过
  - 三个 L2 若各自定义“实质进展”会产生超时语义漂移；必须复用同一判定约定与测试矩阵
  - HTTP 与 WebSocket 若各自生成 User-Agent，可能再次漂移；必须复用单一常量并断言精确值
  - `WireApi` 新增变体不会触发 turn.rs 既有 `==` 分支的编译错误，Chat 可能静默落入未审视的默认侧（见 design 决策 11 行为矩阵）
- 回滚策略：
  - 移除 `WireApi::Chat` 路由与 Chat 适配模块，恢复「chat 已移除」配置错误；Responses/Claude 路径保持不变
- 验证命令（实现阶段，审核通过后）：
  - `cd codex-rs && just fmt`
  - `cd codex-rs && just test -p codex-api`
  - `cd codex-rs && just test -p codex-tools`
  - `cd codex-rs && just test -p codex-model-provider-info`
  - `cd codex-rs && just test -p codex-core`（Chat / multi-backend 相关用例）
  - schema 变更时：`cd codex-rs && just write-config-schema`
  - 依赖变更时：仓库根目录 `just bazel-lock-update`
- 外部参考（只比较行为，不引入依赖）：
  - `/Users/mt/code/mt-ai/aicodex/grok-build/crates/codegen/xai-grok-sampler`
  - 重点：`stream/chat_completions.rs`、`stream/responses.rs`、`stream/messages.rs`、`events.rs`、`actor/request_task.rs`

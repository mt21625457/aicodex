## Context

### 现状（aicodex / Codex）

端到端采样路径：

```text
Session turn (core)
  └─ run_sampling_request / try_run_sampling_request
       └─ ModelClientSession::stream (match WireApi)
            ├─ WireApi::Responses → POST …/responses (+ 可选 WS)
            └─ WireApi::Claude    → POST …/messages
                 └─ codex-api SSE parsers
                      └─ ResponseEvent stream
                           └─ turn.rs 消费 → tools / UI / rollout
```

关键事实：

| 项 | 现状 |
| --- | --- |
| 归一事件 | `codex_api::ResponseEvent`（非 `SamplingEvent`） |
| WireApi | `Responses`（默认）、`Claude`；`"chat"` 反序列化硬错误 |
| Responses SSE | `codex-api/src/sse/responses.rs` |
| Claude SSE | `codex-api/src/sse/claude.rs` |
| 请求适配 | Responses：`build_responses_request`；Claude：`core/src/claude.rs` |
| 工具环 | 始终由 `ResponseItem` 驱动，与 wire 无关 |
| SSE idle 计时 | Responses / Claude 各自在 `codex-api/src/sse/*.rs` 对 `eventsource()` 的下一帧做 timeout；**任意可解析 SSE 帧（含 ping/未知事件）都会开始新的完整等待窗口**，非 content-aware。`codex-client/src/sse.rs` 的通用 helper 不在这两条 sampling 路径上 |
| 宿主协议 | app-server / MCP；**无 ACP** |

### 历史参照（本仓库 git 历史）

上游曾删除一套完整 Chat 实现：commit `d2394a249`（"chore: nuke chat/completions API
(#10157)"）移除 `codex-api/src/requests/chat.rs`（约 494 行）、`codex-api/src/sse/chat.rs`
（约 717 行）、`core/tests/chat_completions_payload.rs`、`core/tests/chat_completions_sse.rs`
等约 2900 行。实现时可对照该提交校准请求类型、SSE 累积（含流式 `tool_calls` 拼参）与 mock
测试结构；注意此后 crate 已重组（工具序列化迁至 `codex-rs/tools`、endpoint 层重构、
`common` crate 已不存在），不可直接 revert，需按本设计的新归属重写。

### 参考（grok-build，只比行为）

`xai-grok-sampler` 三层：

| 层 | 职责 |
| --- | --- |
| L1 `SamplingClient` | HTTP + SSE，返回后端原生 chunk 流 |
| L2 `stream::*` | 纯变换 → `SamplingEvent` + 组装 `ConversationResponse` |
| L3 `SamplerActor` | 并发、retry、cancel、事件总线 |

三后端：`ChatCompletions` / `Responses` / `Messages`，输入统一 `ConversationRequest`。

**本设计采纳**：L1/L2/L3 分层、Chat L2、content-aware idle、错误分类纯函数、tool_index 稠密化。  
**本设计不采纳**：用 `SamplingEvent` 替换 `ResponseEvent`；默认引入完整 `SamplerActor`；把 ACP 宿主接入绑进本变更。

### 约束与干系人

- 协议私有逻辑必须留在 `codex-api` / `codex-tools` 适配器，不得污染 OpenAI Responses 解析器，也不得把 Claude 逻辑并入 Chat/Responses。
- `codex-core` 只做 `WireApi` 分发与 Prompt→请求转换编排入口。
- turn 循环、tool approval、sandbox 语义不变。
- 变更必须可分期落地，每期可独立测试。
- **开发前必须通过本提案人工审核。**

## Goals / Non-Goals

**Goals:**

1. 以 `ResponseEvent` 作为三后端唯一 agent 面向采样事件流（语义上的 SamplingEvent）。
2. 恢复 Chat Completions 一等公民路径：`WireApi::Chat` → `/v1/chat/completions`。
3. 固化 L1/L2/L3 边界，使新增第四协议只需加适配器 + 测试，而不改 turn 循环。
4. Chat 工具环与 Responses/Claude 同等：流式 tool delta → `ResponseItem` → 本地执行 → 下一轮 history。
5. 引入 content-aware idle 与可复用错误分类约定。
6. 用 mock e2e 证明 Chat 路径；用一致性清单约束三后端 L2。

**Non-Goals:**

1. 不把 `ResponseEvent` 重命名/替换为 grok 的 `SamplingEvent`。
2. 不默认引入 `SamplerActor` / 共享 event bus（除非后续另提并发采样需求）。
3. 不实现 ACP、Leader IPC、Computer Hub、MCP reverse transport。
4. 不恢复已删除的 `ollama-chat` 独立 provider id（可用通用 Chat wire + ollama base_url）。
5. 不把 Chat Completions 当作 Responses 的“浅包装”去复用 Responses SSE parser。
6. 不在本变更中重做 Claude native tools / count_tokens / context accounting（已有独立 OpenSpec）。
7. 不引入 `grok-build` / `xai-grok-sampler` 作为依赖。

## 架构总览

```text
                    ┌─────────────────────────────────────────┐
                    │  L3 Orchestration (codex-core)          │
                    │  ModelClientSession::stream             │
                    │  turn.rs / map_response_stream          │
                    │  (retry 策略可调用纯 classify，可选)      │
                    └───────────────────┬─────────────────────┘
                                        │ ResponseEvent 流
                    ┌───────────────────▼─────────────────────┐
                    │  L2 Pure Transforms (codex-api/sse)     │
                    │  responses.rs | claude.rs | chat.rs(新) │
                    │  无 HTTP retry、无 turn、无 tool 执行     │
                    └───────────────────┬─────────────────────┘
                                        │ 原生 chunk / SSE event
                    ┌───────────────────▼─────────────────────┐
                    │  L1 Raw Clients (codex-api/endpoint)    │
                    │  ResponsesClient | ClaudeMessagesClient │
                    │  ChatCompletionsClient (新)             │
                    └───────────────────┬─────────────────────┘
                                        │ HTTP + SSE
                              Provider / Proxy / OSS
```

### 事件契约：继续使用 `ResponseEvent`

`ResponseEvent` 已覆盖 turn 所需核心语义，与 grok `SamplingEvent` 映射如下：

| grok `SamplingEvent` | Codex `ResponseEvent` / 现有层 | 说明 |
| --- | --- | --- |
| `StreamStarted` | `Created` + timing | 保持 `Created` |
| `FirstToken` | `map_response_stream` 遥测 | 不必进枚举 |
| `ChannelToken(Text)` | `OutputTextDelta` | 直接对应 |
| `ChannelToken(Reasoning)` | `ReasoningContentDelta` / Summary* | Codex 更细 |
| `ToolCallDelta` | `ToolCallInputDelta` | 直接对应 |
| `Completed(response)` | `OutputItemAdded/Done*` + `Completed` | **保留增量 item 模型** |
| `Retrying` / `Failed` | `ApiError` + client retry | 可借分类枚举 |
| `ModelMetadata` | `ServerModel` / RateLimits / … | 已有 sidecar |
| `BackendToolCall*` | 特殊 `OutputItem`（按需） | Chat 路径 P1 可不做 |

**决策：不新增平行事件枚举。** 若未来需要 UI 级 `Retrying` 通知，可在 L3 侧以旁路 telemetry / protocol event 发出，而不改 L2 契约。

## Decisions

### 决策 1：归一目标是 `ResponseEvent`，不是 `SamplingEvent`

- **理由**：turn、history、compact、telemetry、工具配对已绑定 `ResponseItem`/`ResponseEvent`；重命名成本高、收益低。
- **备选**：引入 `SamplingEvent` 再映射——否决（双轨转换、回归面翻倍）。
- **备选**：在 app-server 层归一——否决（宿主不应解析 SSE）。

### 决策 2：恢复 `WireApi::Chat`，独立于 Responses

```rust
pub enum WireApi {
    Responses, // default
    Claude,
    Chat,      // NEW — /v1/chat/completions
}
```

- 反序列化接受 `"chat"`；移除（或降级）当前 `CHAT_WIRE_API_REMOVED_ERROR` 硬失败。
- **BREAKING 说明（对“chat 已移除”文档/脚本）**：曾依赖“配置 chat 必失败”的自动化需要更新；对用户是功能恢复而非破坏。
- Core 分发：

```text
match wire_api {
  Responses => responses_client.stream_request(...),
  Claude    => claude_client.stream_request(...),
  Chat      => chat_client.stream_request(...),
}
```

### 决策 3：严格 L1 / L2 / L3 文件归属

| 层 | 归属 | 允许 | 禁止 |
| --- | --- | --- | --- |
| L1 | `codex-api/src/endpoint/chat_completions.rs`（新）等 | HTTP、header、原生反序列化 | 发出 `ResponseEvent`、retry 睡眠 |
| L2 | `codex-api/src/sse/chat.rs`（新）等 | 状态机、累积、映射 `ResponseEvent` | 执行工具、读 config、改 history |
| L3 | `core/src/client.rs`、`session/turn.rs` | 分发、遥测、编排 retry | 解析 Chat/Claude/Responses SSE 细节 |

模块大小：新逻辑进新文件；避免继续膨胀 `client.rs` / `turn.rs`（符合 AGENTS.md 模块化约束）。

> 注：Responses 与 Claude 当前都在各自 `codex-api/src/sse/*.rs` L2 循环中直接做 idle
> timeout；`codex-client/src/sse.rs` 的 helper 不参与这两条 sampling 路径。content-aware idle
> 因此由每个 L2 解析器维护“最后一次实质进展”期限：未知事件、ping、空 data 与不可解析
> keepalive 不推进期限；生命周期、内容、tool delta、usage、terminal/error 事件推进期限。
> 不修改通用 transport helper，避免扩大回归面。

### 决策 4：Chat 请求适配器放在 core，线类型放在 codex-api

对齐 Claude 先例：

- `codex-api`：`ChatCompletionsRequest` / chunk 类型、SSE 解析、endpoint client。
- `codex-core`：`build_chat_completions_request(Prompt, …)`（新模块，例如 `core/src/chat_completions.rs`），负责：
  - system / developer / user / assistant 角色映射；
  - tool call / tool result 多轮拼接（assistant `tool_calls` + tool role messages）；
  - 图像：data URL / HTTP(S) 按 Chat 多模态 content parts 映射；不支持则显式 text placeholder；
  - 不把 Responses 特有字段硬塞进 Chat body。

### 决策 5：工具序列化在 `codex-tools`，逆向映射可测

- Chat tools 形态：`tools: [{ type: "function", function: { name, description, parameters } }]`（及兼容供应商变体，以最小可用集合为先）。
- 流式 `delta.tool_calls[index]`：
  - 用 provider index → 稠密 `tool_index`；
  - 累积 `id` / `name` / `arguments` 碎片；
  - 发出 `ToolCallInputDelta`；
  - 在可判定边界（chunk 结束或 finish_reason）发出 `OutputItemDone(FunctionCall|CustomToolCall|…)`。
- MCP 命名空间：保持 Codex 内部名；若供应商限制名称字符，做确定性 sanitization + reverse table（对齐 Claude 经验）。

### 决策 6：L2 流变换必须满足共享契约

每个后端 L2（含现有 Responses/Claude）MUST：

1. 尽早发出生命周期事件（至少最终能观测到 `Created` 或等价起点）。
2. 增量发出 text / reasoning / tool input deltas。
3. 在终止前发出完整 `ResponseItem`（`OutputItemDone`）。
4. 发出恰好一次成功 `Completed`，或错误结束（`ApiError` / stream error），不得静默截断。
5. **Content-aware idle**：仅 transport keepalive / 无实质内容的 ping **不得**重置进度计时；无进展超时必须变成可判定错误。
6. 纯函数式变换：不发起第二路 HTTP、不 sleep retry。

Chat L2 具体映射（实施指导）：

| Chat chunk 字段 | ResponseEvent |
| --- | --- |
| 首包到达 | `Created`（若尚无） |
| `delta.content` | `OutputTextDelta` |
| `delta.reasoning_content` 或供应商 thinking 字段 | `ReasoningContentDelta`（字段探测表可配置/可扩展） |
| `delta.tool_calls[i].function.arguments` | `ToolCallInputDelta` |
| 结束且拼出 message/tool calls | `OutputItemDone` × N |
| `usage` | 并入 `Completed.token_usage` |
| `finish_reason` | 映射 `end_turn` / `provider_stop_reason` |
| API error / 空流 / idle | `ApiError` 路径 |

参考 grok `stream_chat_completions` 的累积与 idle 行为，但输出必须是 Codex 类型。

### 决策 7：L3 不强制 Actor；复用现有错误与 retry 契约

- 保持现有 `codex-client` / transport retry 为主路径。
- Chat 把 Auth / RateLimited / IdleTimeout / EmptyResponse / Api / Transport 映射到现有
  `ApiError` / `ProviderStreamErrorKind`，继续由现有 transport retry policy 决定重试。
- 本期不新增第二套 classifier 枚举或 retry policy，避免与已存在的 provider error mapping 漂移。
- **禁止**在未证明并发采样需求前引入完整 `SamplerActor`。

### 决策 8：宿主协议正交，明确分期之外

| 层 | 本变更 | 未来提案 |
| --- | --- | --- |
| 推理协议三后端归一 | ✅ | — |
| app-server / MCP 宿主 | 不改协议形状 | 仅消费已归一事件 |
| ACP / Leader | ❌ | 另开 OpenSpec |

这与 grok「ACP 是宿主脊柱、sampler 独立」一致：先打通采样，再谈宿主。

### 决策 9：配置与兼容

- `docs/config.md`：该文件目前**未记载** `wire_api` / `model_providers`（官方用户文档外链至
  developers.openai.com），因此本次是**新增**而非恢复：仿照现有 "Claude native protocol
  policy" 节，新增一节内部行为说明，给出 `responses` / `claude`（别名 `anthropic`）/ `chat`
 的选择标准。遵守 AGENTS.md 约束：不在 `docs/` 添加一般性产品/用户向文档。
- 默认仍为 `Responses`；Chat 需显式 `wire_api = "chat"`。
- OSS（Ollama 等）：推荐 `wire_api = "chat"` + 对应 base_url，或继续用 Responses 兼容层（若供应商支持）——文档写清，不强制一条路。
- `codex-rs/ollama` crate 与 `LEGACY_OLLAMA_CHAT_PROVIDER_ID` / `OLLAMA_CHAT_PROVIDER_REMOVED_ERROR`
  文案本期均不动（不恢复 `ollama-chat` provider id）。
- 远程/托管 thread config：`codex.thread_config.v1.proto` 的 `enum WireApi` 包含
  `WIRE_API_RESPONSES` / `WIRE_API_CLAUDE` / `WIRE_API_CHAT`；`thread_config/remote.rs`
  将三者映射到本地 `WireApi`，对未知值继续报错。远程下发 Chat 与本地 `wire_api = "chat"`
  一等公民对齐（属 configuration loading 外部集成面，按 breaking-change 规则审视）。
- `just write-config-schema`：`WireApi` 已暴露在 `core/config.schema.json`，新增 `"chat"`
  枚举值后必须 regenerate。

### 决策 10：测试策略（审核通过后的实施标准）

**单元 / 适配器：**

- Chat 请求整对象序列化（含 tool 历史）。
- Chat SSE：text、multi tool_calls、reasoning、usage、error、乱序/残缺 chunk。
- idle：仅 keepalive 必须触发超时错误。

**Mock e2e（`codex-core`）：**

1. Chat 请求命中 `/v1/chat/completions`（或配置的 chat path）。
2. Authorization: Bearer（或 provider 规定的等价头）。
3. 流式 text → 最终 message。
4. 流式 tool_calls → 本地工具执行 → 第二轮请求含 tool role / tool result 消息。
5. 错误流与空响应分类。

**三后端 L2 一致性清单（文档 + 可选测试夹具）：**

- text delta / tool delta / completed / error / idle 五类场景在 Responses、Claude、Chat 均可构造。

### 决策 11：Chat 在既有 wire 分支上的行为矩阵

`core/src/session/turn.rs` 与 `core/src/session/context_window.rs` 存在多处
`== WireApi::Claude` / `== WireApi::Responses` 的**非穷尽**判断；新增 `WireApi::Chat`
变体**不会**触发这些位置的编译错误，Chat 将静默落入 else 分支。下表逐条声明预期落点
（结论：本期全部维持现状默认；实现时逐条核对并以测试或注释固化）：

| 位置（锚点描述，行号以代码为准） | 行为 | Chat 落点 | 说明 |
| --- | --- | --- | --- |
| `turn.rs` pre-sampling admission（首轮仅 Claude 跑 admission compaction） | 上下文准入 | 非 Claude 侧（首轮不跑） | 与 Responses 一致 |
| `turn.rs` `ContextWindowExceeded` turn 内恢复（仅 Claude） | 窗口溢出恢复重试 | 非 Claude 侧（不恢复，直接报错） | Chat 暂无可靠的溢出信号分类；后续由恢复类提案再议 |
| `turn.rs` 估算全窗 auto-compact 触发（仅 Claude） | 估算 token 达窗限触发 compact | 非 Claude 侧（不触发） | 依赖 `token_limit_reached` / `auto_compact_scope_limit` 主路径 |
| `turn.rs` `emit_in_flight_context_estimate`（仅 Claude） | 采样中上下文估算事件 | 非 Claude 侧（不发） | 仅 UI 展示差异，可后续校准 |
| `turn.rs` `Completed` 后 token usage 记账（Claude 单独记录 vs 其他走 `record_token_usage_info`） | usage 记账 | 走 `record_token_usage_info` 主路径 | 与 Responses 一致 |
| `turn.rs` Responses-only `update_responses_token_usage_context_estimate` | token count 前更新上下文估算 | 跳过（非 Responses） | Chat usage 语义不同，不套用 Responses 估算逻辑 |
| `context_window.rs` Responses-only local estimate 忽略 | 上下文 token 来源选择 | 非 Responses 侧（不忽略） | Chat 无服务端 context source 标记 |
| `tui/status/card.rs`、`exec/event_processor_with_human_output.rs`、`sandbox-summary/config_summary.rs` | `== Responses` 展示分支 | else 分支（不变） | P0 人工确认展示无异常即可，不改代码 |

实现要求：PR-B 必须包含对上表前七处的逐条核对（测试或代码注释）；任何一处需要改判
落点时，先回写本表与 OpenSpec 再改代码。

### 决策 12：所有 model-provider 协议统一 User-Agent

- 精确格式：`aicodex/<workspace package version>`，不附加 OS、terminal、originator 或 suffix。
- HTTP：在 `codex-api` 的共享 endpoint request 构造点最后写入 header，覆盖 transport 默认值
  与 provider 自定义 `user-agent`，从而覆盖 Responses、Claude、Chat 及同 endpoint session 的
  辅助请求。
- Responses WebSocket：握手显式复用同一个常量，不能继续依赖 login client 的动态默认 UA。
- Realtime HTTP/WebSocket：HTTP 继承 endpoint session 的固定 UA，WebSocket header merge 最后
  覆盖同一常量；provider、extra、default 与 auth header 均不能覆盖。
- app-server / MCP initialize payload 中的 `user_agent` 不属于 model-provider 网络协议，本期不改；
  它们是宿主身份/兼容接口，改动会构成独立外部 API 变化。
- 测试必须对 Responses HTTP、Responses WebSocket、Claude、Chat 的 header 做精确相等断言，
  不能只断言存在或前缀。

## 实施细节指导（供开发阶段执行）

> 本章是审核通过后的编码指南，不是现在就开工的许可。

### 阶段 P0 — 边界与配置面（小 diff）

1. 在 `model-provider-info` 恢复 `WireApi::Chat` 与 `"chat"` 反序列化。
2. 更新迁移文案：从「已移除」改为「如何选择 chat vs responses vs claude」。
3. 在 `codex-api` 增加模块骨架：
   - `endpoint/chat_completions.rs`
   - `sse/chat.rs`
   - 在 `sse/mod.rs` / `endpoint/mod.rs` re-export
4. 在 `ModelClientSession::stream` 增加 `WireApi::Chat` 分支 stub（返回明确 `ApiError::Unsupported` 亦可，但应马上被 P1 填实）。
5. 文档：`docs/config.md` 草案段落（可标 WIP）。
6. 验证：`just test -p codex-model-provider-info` 相关用例；`openspec validate … --strict`。
7. 人工确认决策 11 表中三处 `== Responses` 展示分支（tui / exec / sandbox-summary）对 Chat 落 else 无异常。

### 阶段 P1 — Chat L1/L2 + 请求适配 + 工具环（主交付）

1. **L1**：实现 `ChatCompletionsClient::stream_request`：
   - path：`chat/completions`（相对 provider `/v1`）
   - headers：JSON + `Accept: text/event-stream` + auth
   - 解码为 typed `ChatCompletionChunk`（允许有限的供应商别名字段）
2. **L2**：`spawn_chat_response_stream` / `process_chat_chunk`：
   - 状态：`created_emitted`、per-index tool 累积、assistant text、reasoning text、usage
   - 终止条件：`[DONE]` / finish / 连接结束 + 校验是否已有实质内容
3. **请求适配** `build_chat_completions_request`：
   - 对照 `build_claude_messages_request` 的测试风格：整对象 `assert_eq!`
   - tool 历史顺序严格可测
4. **tools**：`create_tools_json_for_chat_completions_*`
5. **core 分发**：真实接通 stream，而不是 stub
6. **mock e2e**：至少 text + tool loop 两条
7. **行为矩阵**：逐条核对决策 11 表中 `turn.rs` / `context_window.rs` 的 Chat 落点，以测试或代码注释固化
8. **回归**：现有 Claude/Responses 测试全绿

### 阶段 P2 — 硬化（对齐 grok 行为语义）

1. content-aware idle 落到 Chat，并在 Responses/Claude L2 循环中用相同“实质进展期限”语义
   校准；不修改 `codex-client/src/sse.rs` 通用 helper。三后端 SSE 测试必须全绿。
2. 不新增平行 `RetryClassifier` 类型；继续使用现有 `ApiError` / `ProviderStreamErrorKind` 与
   transport retry policy，Chat 仅补稳定的 idle、empty、malformed、HTTP/API 分类。
3. reasoning 首期兼容 `reasoning_content`、`reasoning` 与 `thinking` 三个已知 delta 字段；
   未知字段忽略并 trace。
4. 空响应 / max_tokens truncation 映射到稳定 `provider_stop_reason` / 错误类。

### 阶段 P3 — 明确不做（另案）

- ACP stdio / Leader / Hub
- `SamplingEvent` 重命名工程
- SamplerActor 并发采样

### 关键文件清单（预期触摸）

| 路径 | 动作 |
| --- | --- |
| `codex-rs/model-provider-info/src/lib.rs` | `WireApi::Chat` |
| `codex-rs/codex-api/src/endpoint/chat_completions.rs` | 新建 L1 |
| `codex-rs/codex-api/src/sse/chat.rs` | 新建 L2 |
| `codex-rs/codex-api/src/common.rs` | Chat 请求/chunk 类型（或子模块） |
| `codex-rs/codex-api/src/sse/responses.rs`、`sse/claude.rs` | content-aware idle（P2；协议 L2 本地期限） |
| `codex-rs/core/src/chat_completions.rs` | 新建请求适配 |
| `codex-rs/core/src/client.rs` | 分发分支 |
| `codex-rs/core/src/session/turn.rs`、`core/src/session/context_window.rs` | 行为矩阵核对（决策 11；预期无逻辑改动，仅注释/测试） |
| `codex-rs/tools/src/...` | Chat tools JSON |
| `docs/config.md` | 新增内部行为说明一节（非恢复；非用户向教程） |
| `codex-rs/core/config.schema.json` | 如需 regenerating |
| `codex-rs/core/suite/...` 或现有 mock 测试目录 | Chat e2e |

### 编码规范（仓库强制）

- 改完跑 `cd codex-rs && just fmt`
- 按 crate：`just test -p <crate>`；**不要**默认跑全量 `just test`（改 common/core/protocol 时先询问）
- Clippy：`just fix -p <crate>`
- 新模块控制体积；禁止向 `turn.rs` / `chatwidget.rs` 类巨型文件堆逻辑
- 测试用 `pretty_assertions::assert_eq`，优先整对象比较
- 参数字面量遵循 `/*param_name*/` 约定（如适用）
- 禁止触碰 `CODEX_SANDBOX_*` 相关逻辑

## Risks / Trade-offs

| 风险 | 缓解 |
| --- | --- |
| Chat 多轮 tool 历史错误 | 整对象序列化单测 + mock 双轮 e2e |
| 供应商 chunk 方言差异 | typed 主路径 + 有限 alias；未知字段忽略并打 trace |
| reasoning 串到普通文本 | 独立 delta 事件；禁止合并进 `OutputTextDelta` |
| 恢复 chat 与旧迁移文档冲突 | 同步更新 docs / error 文案 / discussion 链接说明 |
| L2 不纯导致难测 | code review 清单：sse 模块禁止 reqwest retry |
| 三个 L2 的 content-aware idle 判定漂移 | 使用同一“实质事件推进期限”清单；PR-C 单独落地；三后端 SSE 测试必须全绿 |
| Chat 静默落入 turn.rs 既有 wire 分支默认侧 | 决策 11 行为矩阵逐条声明 + PR-B 强制核对 |
| 范围膨胀到 ACP | Non-Goals + 审核门禁；P3 另案 |
| 改 core 触发过重 CI | 分期 PR；每期 < 800 行非机械变更（AGENTS 指引） |

## Migration Plan

1. **审核通过**后方可建实现分支 / PR。
2. 建议拆 PR：
   - PR-A：`WireApi::Chat` + 模块骨架 + 文档（P0）
   - PR-B：Chat L1/L2 + 请求/工具 + e2e（P1）
   - PR-C：idle / classifier 硬化（P2）
3. 默认 wire 仍为 Responses；现有用户零感。
4. 依赖 Chat 的用户显式改 `wire_api = "chat"`。
5. 回滚：revert Chat 模块与枚举变体；保留 Responses/Claude。

## 审核决议（2026-07-17）

1. 枚举使用 `WireApi::Chat`，保持历史配置值 `"chat"` 的短名兼容。
2. 所有 Chat 工具在 wire 上使用 function envelope；Function、Namespace、ToolSearch、Freeform
   均生成确定性 wire name 与 reverse metadata，L2 按原始 kind 还原 `ResponseItem`。不恢复
   Chat 私有 local-shell item；local shell 继续通过当前工具声明映射进入通用工具环。
3. reasoning 首期读取 `reasoning_content`、`reasoning`、`thinking`；未知字段忽略并 trace。
4. path 固定为相对 provider base URL 的 `chat/completions`，本期不新增 path 配置或 schema。
5. 不新增 `RetryClassifier`；复用现有 `ApiError`、`ProviderStreamErrorKind` 和 transport retry。
6. 以当前 HEAD 的 Claude 实现为基线串行落地；不与另一个修改相同文件的 change 并行。
7. model-provider User-Agent 统一为决策 12 的精确格式；宿主 initialize UA 不在本期范围。
8. Chat assistant/reasoning/tool-argument 的整次响应上下文设置 10,000-byte 总硬上限，
   provider identifier 与 tool-call 数量另设小上限；单工具声明 10,000-token、工具集合约
   64,000-token 硬上限。最终 request 对每条 message/tool/schema 与总请求再做统一预算校验，
   覆盖旧 rollout / wire 切换；超限明确失败，不在后续请求时反复改写历史。
   `ToolSearchOutput` 注入严格保持在约 1,000 token 内；内部 `AgentMessage` 在 Chat wire 上
   显式拒绝，避免绕过 `ContextualUserFragment` 契约。

## Verification（审核通过后的命令清单）

```bash
cd codex-rs
just fmt
just test -p codex-model-provider-info
just test -p codex-api
just test -p codex-tools
just test -p codex-core   # 仅在 Chat/core 适配就绪后；先询问是否跑全量
just fix -p codex-api
just fix -p codex-core
# 若 schema 变更：
just write-config-schema
# 若 Cargo 依赖变更（仓库根）：
just bazel-lock-update
```

OpenSpec：

```bash
openspec validate unify-multi-backend-sampling-normalization --strict
```

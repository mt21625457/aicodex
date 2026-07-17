## 0. Review Gate（开发前必做）

- [x] 0.1 人工审核通过 `proposal.md`、`design.md`、全部 `specs/**/spec.md` 与本 `tasks.md`
- [x] 0.2 关闭 Open Questions（见 design “审核决议（2026-07-17）”）
- [x] 0.3 审核前只修改 OpenSpec；审核通过后才进入实现
- [x] 0.4 Run `openspec validate unify-multi-backend-sampling-normalization --strict`

## 1. OpenSpec and Scope

- [x] 1.1 保持 proposal / design / specs / tasks 与实现同步（实现阶段每次边界变更回写）
- [x] 1.2 确认 Chat 逻辑不进入 Responses / Claude SSE 解析器
- [x] 1.3 确认不引入平行 `SamplingEvent` 总线，不引入默认 `SamplerActor`，不做 ACP/Leader
- [x] 1.4 远程 thread config proto 扩展 `WIRE_API_CHAT` 并完成 from/to 映射（design 决策 9），OpenSpec 已同步
- [x] 1.5 确认 `codex-rs/ollama` crate 与 `LEGACY_OLLAMA_CHAT_PROVIDER_ID` / `OLLAMA_CHAT_PROVIDER_REMOVED_ERROR` 文案不改动

## 2. P0 — WireApi 与模块骨架

- [x] 2.1 在 `codex-model-provider-info` 恢复 `WireApi::Chat` 与 `"chat"` 反序列化
- [x] 2.2 更新/替换 `CHAT_WIRE_API_REMOVED_ERROR` 相关文案与测试期望
- [x] 2.3 新增 `codex-api` 模块骨架：`endpoint/chat_completions.rs`、`sse/chat.rs` 及 mod 导出
- [x] 2.4 在 `ModelClientSession::stream` 增加 `WireApi::Chat` 分发点（已真实接通）
- [x] 2.5 在 `docs/config.md` 新增 Chat/Responses/Claude 选择内部说明一节
- [x] 2.6 schema 已通过 `just write-config-schema` 更新
- [x] 2.7 `just test -p codex-model-provider-info`（及必要的 config 测试）
- [x] 2.8 人工确认 `tui/status/card.rs`、`exec/event_processor_with_human_output.rs`、`sandbox-summary/config_summary.rs` 的 `== WireApi::Responses` 展示分支对 Chat 落入 else 无异常（design 决策 11）
- [x] 2.9 定义共享 `aicodex/<workspace package version>` User-Agent；Responses HTTP/WebSocket、Claude（含 count_tokens）、Chat、Realtime 精确值测试

## 3. P1 — Chat L1 client

- [x] 3.1 定义 Chat Completions 请求/chunk 线类型（typed；工具 schema 可保留 `Value`）
- [x] 3.2 实现 `ChatCompletionsClient::stream_request`（path、headers、SSE byte stream）
- [x] 3.3 Auth：使用 provider 既有 Bearer/等价契约；不误用 Anthropic `x-api-key`
- [x] 3.4 单元/集成：mock 断言命中 `chat/completions` 与关键头

## 4. P1 — Chat L2 → ResponseEvent

- [x] 4.1 实现 stateful Chat chunk 累积器（text / tool_calls / usage / finish）
- [x] 4.2 映射 `OutputTextDelta`、`ToolCallInputDelta`、`OutputItemDone`、`Completed`
- [x] 4.3 映射 reasoning/thinking 字段到 `Reasoning*Delta`（按审核确认的字段表）
- [x] 4.4 provider tool index → dense tool_index；防止交叉串参
- [x] 4.5 错误/空流/畸形/超限 chunk → 可行动 `ApiError`
- [x] 4.6 `just test -p codex-api` 覆盖上述场景

## 5. P1 — Prompt→Chat 请求适配与工具序列化

- [x] 5.1 新增 `codex-core` Chat 请求适配模块 `core/src/chat_completions.rs`
- [x] 5.2 system/developer/user/assistant 角色与多模态 content parts
- [x] 5.3 tool_calls + tool result 多轮历史整对象序列化测试
- [x] 5.4 `codex-tools`：Chat function tools JSON、确定性名称 sanitization + reverse map + 硬预算
- [x] 5.5 接通 `ModelClientSession` Chat 真实 stream 路径
- [x] 5.6 `just test -p codex-tools` 与 Chat 相关 `codex-core` 单测
- [x] 5.7 逐条核对 design 决策 11 行为矩阵中 `turn.rs` / `context_window.rs` 的 Chat 落点（测试或代码注释固化）

## 6. P1 — Mock end-to-end

- [x] 6.1 mock Chat 流式文本回合（path/headers/deltas/completed）
- [x] 6.2 mock Chat tool loop：tool_calls → 本地执行 → 第二轮 tool 消息 → 最终文本
- [x] 6.3 mock provider error、空响应与 idle
- [x] 6.4 回归：现有 Responses / Claude 测试保持通过

## 7. P2 — 硬化与共享 L2 契约

- [x] 7.1 Chat content-aware idle（keepalive 不计进度；Chat L2 维护实质进展期限）
- [x] 7.2 校准 Claude / Responses L2 的同一 idle 契约；不改 `codex-client/src/sse.rs`；三后端 SSE 回归全绿
- [x] 7.3 复用现有错误分类：Chat idle / empty / malformed / HTTP/API 映射到稳定 `ApiError` / `ProviderStreamErrorKind`
- [x] 7.4 reasoning 兼容 `reasoning_content` / `reasoning` / `thinking`，未知字段 trace 后忽略
- [x] 7.5 三后端测试覆盖 meaningful/non-meaningful progress、terminal 与 error 契约

## 8. Docs / Schema / Lockfiles

- [x] 8.1 定稿 `docs/config.md` 新增的内部行为说明节（Chat / Responses / Claude 选择标准）
- [x] 8.2 `just write-config-schema` 已运行
- [x] 8.3 未修改 `Cargo.toml` / `Cargo.lock`，无需 `just bazel-lock-update`
- [x] 8.4 回写 OpenSpec：关闭已决议 Open Questions，记录实现与审核事实

## 9. Verification

- [x] 9.1 `cd codex-rs && just fmt`（实现阶段已运行；最终收尾会再运行）
- [x] 9.2 `just test -p codex-api`
- [x] 9.3 `just test -p codex-tools`
- [x] 9.4 `just test -p codex-model-provider-info`
- [x] 9.5 Chat/core 相关测试与最终受控并发全量 `just test`（12,597/12,597 通过，39 skipped）
- [x] 9.6 `just fix -p codex-api`（及实际改动的其它 crate）
- [x] 9.7 `openspec validate unify-multi-backend-sampling-normalization --strict`

## 10. Code Review Rounds

- [x] 10.1 第 1 轮：breaking changes / change size / model context / testing；修复全部代码 finding
- [x] 10.2 第 2 轮：同四维复审；修复 UA 覆盖、remote compaction、app-server metadata、doctor、上下文预算、模块大小与测试缺口
- [x] 10.3 第 3 轮：同四维最终复审；已修复实时 wire snapshot、上下文总预算与 turn 级测试缺口，进入最终全量验证

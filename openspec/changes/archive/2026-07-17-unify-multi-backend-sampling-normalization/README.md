# unify-multi-backend-sampling-normalization

## 状态

**提案已审核通过（2026-07-17）— 可以按 tasks 开始开发。**

审核决议已写入 `design.md`；实现中若改变协议边界，必须先回写 OpenSpec。

## 目标（一句话）

借鉴 grok-build 的 L1/L2/L3 分层，在 Codex 内把 **OpenAI Chat Completions / OpenAI Responses / Anthropic Messages** 三后端流统一归一为现有 **`ResponseEvent`**，并恢复 `WireApi::Chat`；不引入第二套 `SamplingEvent`，不做 ACP/Leader。

## 文档地图

| 文件 | 内容 |
| --- | --- |
| [proposal.md](./proposal.md) | Why / What / Capabilities / Impact |
| [design.md](./design.md) | 架构、决策、分期实施细节、风险、审核决议 |
| [tasks.md](./tasks.md) | 可勾选实施清单（含审核门禁 §0） |
| [specs/multi-backend-sampling-normalization/spec.md](./specs/multi-backend-sampling-normalization/spec.md) | L1/L2/L3 与 ResponseEvent 契约 |
| [specs/chat-completions-wire-api-support/spec.md](./specs/chat-completions-wire-api-support/spec.md) | Chat Completions 行为需求 |
| [specs/claude-wire-api-support/spec.md](./specs/claude-wire-api-support/spec.md) | Claude 作为 L2 的边界补充 |
| [specs/model-provider-routing/spec.md](./specs/model-provider-routing/spec.md) | `wire_api = "chat"` 路由 |

## 已确认的审核结论

1. 是否同意 **保留 `ResponseEvent`** 而非引入 `SamplingEvent`
2. `WireApi` 命名：`Chat` vs `ChatCompletions`
3. P1 工具覆盖矩阵（仅 function 还是对齐 Claude 子集）
4. content-aware idle 在 P2 由各协议 L2 维护实质进展期限；不新增 RetryClassifier
5. 与进行中的 Claude OpenSpec 变更如何避冲突
6. reasoning 字段第一期范围（OQ3：仅 `delta.reasoning_content`，其它供应商字段放 P2 兼容表？）
7. chat path 是否允许 provider 配置覆盖（OQ4；spec 当前按固定默认路径 `chat/completions` 撰写，批准覆盖需同步修订 spec 与 schema）
8. 决策 11 行为矩阵：Chat 在 turn.rs / context_window.rs 既有 wire 分支上的落点声明是否认可
9. 远程 thread config 同步支持 chat（proto 扩展 `WIRE_API_CHAT`）
10. 所有 model-provider 协议与 transport 使用精确 `aicodex/<workspace package version>` User-Agent

## 校验

```bash
openspec validate unify-multi-backend-sampling-normalization --strict
openspec status --change unify-multi-backend-sampling-normalization
```

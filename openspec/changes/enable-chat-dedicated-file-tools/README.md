# enable-chat-dedicated-file-tools

## 状态

**提案阶段 — 未经人工批准且依赖 Phase A 未完成前不得实施。**

## 目标

在不复制文件执行逻辑的前提下，把 `add-cc-style-file-tools` 提供的 dedicated 文件
工具以独立、默认关闭、可回滚的策略接入 Chat Completions wire。

## 依赖

1. `add-cc-style-file-tools`：提供 hidden handlers、receipt、PathUri、安全 mutation
2. `unify-multi-backend-sampling-normalization`：提供 Chat request/SSE/tool loop 基础

## 已选定默认

- `chat_file_tool_mode = "legacy"`
- `dedicated` 只广告 `read_file` / `edit_file` / `write_file`
- `dedicated_with_apply_patch` 才同时广告 `apply_patch`
- Chat wire 使用稳定 hashed tool name；prompt 使用实际 mapped name
- provider 不兼容时显式失败，不自动重试/降级
- Responses 与 Claude 策略保持不变

## 文档地图

| 文件 | 内容 |
| --- | --- |
| [proposal.md](./proposal.md) | Why / scope / dependencies / impact |
| [design.md](./design.md) | policy、wire-name、history、prompt、分期、测试 |
| [tasks.md](./tasks.md) | 审核门禁与可执行实施清单 |
| [specs/chat-file-tool-rollout/spec.md](./specs/chat-file-tool-rollout/spec.md) | rollout 真值表与隔离 |
| [specs/chat-completions-wire-api-support/spec.md](./specs/chat-completions-wire-api-support/spec.md) | Chat tool wire 与续轮 |

## 校验

```bash
openspec validate enable-chat-dedicated-file-tools --strict
```

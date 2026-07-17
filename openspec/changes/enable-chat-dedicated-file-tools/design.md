## Context

Chat Completions 适配器已经具备本 rollout 所需的协议基础：

- `codex-tools::create_tools_json_for_chat_completions` 把所有 Codex tool kind 序列化
  为 Chat function declarations
- `chat_tool_name(namespace, name, kind)` 生成不超过 64 字符的稳定 sanitized + hashed
  wire name，并用 `ChatToolCallInfo` 保存 reverse mapping
- Chat SSE accumulator 把 fragmented `delta.tool_calls` 恢复为 Codex
  `ResponseItem`
- Chat history adapter 把调用重放为 assistant `tool_calls`，把输出重放为带相同
  `tool_call_id` 的 `tool` role message

因此 dedicated 文件工具不需要新的 wire type。真正需要设计的是：何时把它们放进
Chat 的 model-visible tool set、如何在 hashed wire name 下写正确提示、如何保持历史
稳定，以及 provider 不兼容时如何安全回滚。

本 change 依赖 `add-cc-style-file-tools` 的安全基础，不拥有文件系统语义。依赖提案中的
read receipt、PathUri、编码、审批、commit-time fingerprint、输出上限和 shell fallback
均原样适用。

## Goals / Non-Goals

**Goals:**

- 为 Chat wire 提供独立、typed、默认 legacy 的 rollout policy
- 复用现有普通 function-tool serializer 和 reverse mapping
- `dedicated` 模式收口 shell/`apply_patch` 文件编辑面
- 保持 prior tool history、call id 与 hidden handler dispatch 可恢复
- 用 mock end-to-end turns 证明 read/edit/write 多轮工具环
- 保持 Responses、Claude 和 Chat legacy 完全不变

**Non-Goals:**

- 不实现或修改 dedicated handlers/read receipt/mutation runtime
- 不改变 `chat_tool_name` 或移除 identity hash
- 不为不同 Chat vendor 建立硬编码 allowlist
- 不自动捕获 provider tool error 后重发 legacy 请求
- 不改变 Chat SSE reasoning/content/usage/error 行为
- 不让远程 thread config 新增 Chat wire 选择能力

## Decisions

### 1. 独立枚举，不复用 Claude mode

新增 typed `ChatFileToolMode`：

- `legacy`（默认）
- `dedicated`
- `dedicated_with_apply_patch`

| Mode | dedicated tools | `apply_patch` | shell | guidance |
| --- | --- | --- | --- | --- |
| `legacy` | 不因本 change 广告 | 保持现状 | 保持现状 | 不注入 dedicated 指引 |
| `dedicated` | 广告 | hidden dispatch | 保留系统命令能力，但普通文件 IO 不首选 | 注入 mapped-name 指引 |
| `dedicated_with_apply_patch` | 广告 | 广告 | 同上 | 注入 mapped-name 指引 |

不用 `auto`：Chat-compatible provider 的 function-calling 遵从性无法仅从 URL、模型名或
`WireApi::Chat` 可靠推断。用户/受控 rollout 必须显式选择非 legacy 模式。

不用多个 bool：枚举排除“dedicated 关闭但 fallback 打开”等无效组合。未知值配置加载
失败。非 legacy 模式要求基础 feature `dedicated_file_tools` enabled 且三个 runtime 已
注册；不满足时 fail-closed，不构造采样请求。

### 2. 保持 Chat 稳定 hashed wire name

Chat serializer 当前为每个 tool identity 生成：

```text
chat_tool_name(namespace, semantic_name, kind)
```

即使 `read_file` 是 plain function，wire name 也不是裸 `read_file`，而是稳定的
`read_file__<identity-hash>` 形式。本 change MUST：

- 继续使用 `ChatToolCallKind::Function`
- 继续把 semantic `ToolName` 写入 reverse metadata
- 不对三个 dedicated tool 做 special-case rename
- 不因 tool list reorder、apply_patch hidden/visible 或恢复历史改变 mapped name

这样 prior assistant `tool_calls` 可以使用相同确定性名称重放，SSE 返回的 mapped name
也能恢复成现有 handler 接收的 semantic name。

### 3. Chat 提示必须引用请求中的实际工具

依赖提案要求 dedicated 可见时注入 prefer-dedicated guidance，但 Chat 不能只写
“call `read_file`”，因为 wire request 中可能只有 hashed name。

Chat request builder 在获得 `ChatToolsJson` 后构建一个短、有界 system segment：

- read → 实际 mapped read function name
- edit → 实际 mapped edit function name
- create/overwrite → 实际 mapped write function name
- 依赖 Read 的 Edit/Write 放到后续 completion，不与 Read 放同一 tool-call batch
- binary/unsupported encoding/editable-cap error 才允许专门 shell/script fallback

只有三个 semantic tool 都能在 reverse metadata 中唯一找到且都出现在 request tools
时才注入。否则 request 构造失败；不得注入一个不可调用名称。该 segment 有固定大小
上限，不含路径或文件内容。

### 4. 可见性在 spec planning 层决定，Chat adapter 保持通用

`spec_plan`/provider policy 根据 `WireApi::Chat` + `ChatFileToolMode` 决定 direct/hidden
exposure。Chat serializer 继续只消费最终 `Prompt.tools`：

- 不在 `codex-tools` Chat serializer 中读取 Config
- 不在 SSE parser 中判断 file-tool mode
- 不在 handler 中判断当前 wire

这保持 L1/L2/L3 边界：策略属于 core planning，请求适配只负责确定性序列化。

### 5. 历史与 hidden dispatch

`dedicated` 隐藏 `apply_patch` 只影响新请求的 tool declarations，不删除 handler，也不
重写 prior history：

- 未完成/恢复中的合法 `apply_patch` 仍能 dispatch
- prior assistant `apply_patch` call 使用确定性 Chat name 重放
- tool result 保持原 `tool_call_id`
- mode 在 turn 边界改变时，不对已有 conversation history 做 rewrite

这遵守 Codex “context incrementally built, no history rewrite” 约束。

### 6. Provider 不兼容时不自动降级重试

有些 Chat-compatible provider 会拒绝 `tools`、JSON Schema、`tool_choice` 或
`parallel_tool_calls`。自动用 legacy 重试可能在边界不清时重复已经发生的 server-side
行为，也会掩盖配置事实。

因此：

- request-build schema/budget error 直接失败
- provider HTTP/stream tool error 按现有 Chat error path 返回
- 错误提示说明将 `chat_file_tool_mode` 改回 `legacy`
- telemetry 记录 mode、provider、失败阶段和 tool-selection outcome，但不记录文件内容

### 7. 并行工具调用

保留 provider/model 的既有 `parallel_tool_calls` 能力，不为所有 Chat dedicated turns
全局关闭并行，因为多个独立 Read 可以并行。提示必须要求存在依赖的
Read → Edit/Write 跨 completion 执行。runtime 必须保证：

- Edit/Write 没有来自先前完成 step 的有效 receipt 时安全失败且不写入
- 同一 batch 不得因调度顺序绕过 read-before-write
- 多个写 mutation 继续串行化

若依赖基础实现不能区分同一 batch receipt，Chat rollout 在实现前必须先补依赖契约；
不得靠“通常按数组顺序运行”作为安全假设。

### 8. 配置与作用域

建议配置 shape：

```toml
chat_file_tool_mode = "legacy"
```

它只在 `WireApi::Chat` 生效。若用户为 Responses/Claude provider 设置该字段，配置可保留
但不得影响其工具面；诊断可提示该字段当前未生效。模式是 session-invariant，turn 中途
不得无提示切换。

配置进入 `ConfigToml` 时更新 schema 和 `docs/config.md`。如项目更倾向放进结构化
feature config，最终 shape 可在审核时调整，但 wire 值与真值表不得改变。

### 9. 分期与 change size

本 change 在依赖 Phase A 完成后可作为一个小型 rollout PR：

| Phase | 内容 | Model-visible change |
| --- | --- | --- |
| C0 | config enum、policy truth table、request unit tests | 无，默认 legacy |
| C1 | dedicated/fallback exposure、mapped guidance、mock tool loop | 显式 opt-in |
| C2 | remote executor、telemetry、docs/schema、回滚验证 | 不扩大默认范围 |

如果实现需要修改 Chat SSE accumulator 或 dedicated filesystem semantics，说明边界判断
错误：停止本 change，回写对应基础 change，而不是在 rollout PR 中顺手扩展。

### 10. 测试策略

- `codex-tools`（若实现未改也可只补断言）：三个 semantic functions 序列化为预算内
  function declarations；mapped names 稳定、唯一、可 reverse
- core request tests：legacy/dedicated/fallback 完整对象 deep-equal；guidance 使用实际
  mapped name；tool reorder 不改变名称
- mock Chat e2e：
  - fragmented `read_file` tool call → local execution → matching tool-role result
  - read → edit → edit，证明 receipt 跨 Chat sampling step 且写后刷新
  - write(create) → edit，证明 create 建立 receipt
  - stale/missing receipt 以失败 tool result 继续 turn，磁盘未变
  - hidden apply_patch prior history 保持 wire name/call id
- isolation：相同 config 下 Responses/Claude request tools 不变；Chat legacy snapshot/deep
  equal 不变
- remote：core integration 使用 `build_with_auto_env()`；Linux remote 和 Wine Windows
  target 覆盖 foreign PathUri 与 tool result continuation

## Risks / Trade-offs

- **显式 opt-in 降低早期覆盖率**：换取 provider 兼容问题可定位、可回滚
- **hashed names 降低人类可读性**：保持历史稳定优先；prompt 使用实际 mapped name
- **保留 shell 增加模型选择面**：shell 仍需系统命令；通过动态 guidance 限制普通 IO
- **fallback mode 增加双编辑面**：只用于诊断，不作为默认或 auto
- **新增 ConfigToml surface**：使用单一 enum 并同步 schema，避免布尔组合爆炸

## Migration / Rollback

1. 审核并实现依赖 Phase A；确认 dedicated handlers hidden tests 全绿
2. 实现 C0，默认 legacy，验证所有既有 Chat request deep-equal 不变
3. 实现 C1/C2，在受控 Chat provider 上显式启用 `dedicated`
4. 观察首轮 tool success、schema/provider error、shell fallback、stale retry、token 与延迟
5. 回滚只需设为 `legacy`；不得删除共享 handlers 或 rewrite conversation history

## Open Questions

1. **配置放顶层还是 structured feature config？** 默认：顶层 typed enum；审核可调整
   存放位置，但保持 wire values 与默认 legacy。
2. **是否加入 provider allowlist/auto？** 已选定：否，本 change 只做显式 opt-in。
3. **是否改变 Chat hashed names？** 已选定：否，保持现有确定性 identity contract。
4. **provider tool error 是否自动 legacy retry？** 已选定：否，显式错误与人工回滚。

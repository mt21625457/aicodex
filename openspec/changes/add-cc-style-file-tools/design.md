## Context

Codex 今天的文件相关工具面是碎片化的：

| 能力 | 现状 | 问题 |
| --- | --- | --- |
| 读文件 | 无独立工具；Claude 可走原生 `text_editor.view`；否则 shell | Compatible/Windows 常落到 PowerShell |
| 写/改 | `apply_patch`（OpenAI 主路径）；Claude 另有条件启用的 `text_editor` | Claude 对 patch 语法摩擦大；工具面叠加 |
| Shell | Windows 描述引导 PowerShell 文件操作 | 强化“用 shell 读写”偏置 |

Claude Code（`/Users/mt/code/mt-ai/cc`）用 `Read` / `Edit` / `Write` + 系统提示
“有专用工具就不要用 Bash”解决了同类问题。本设计把该**行为模型**迁入 Codex，
命名与执行仍服从 Codex 约定（snake_case function tools、审批/sandbox、patch 审阅）。

相关已有变更：

- `complete-claude-native-protocol-support`：原生 `text_editor`/`bash` 能力与安全映射
- `harden-claude-tool-call-contracts`：Claude 上 `apply_patch` 契约硬化

本变更不推翻上述安全模型，而是用 dedicated 工具成为 Claude（尤其 Compatible）与
Windows 上的**首选文件面**，并收口广告冲突。

## Goals / Non-Goals

**Goals:**

- 提供 `read_file` / `edit_file` / `write_file` 一等 function tools
- Edit/Write 强制先读 + fingerprint 冲突检测（借鉴 CC `readFileState`，但保持有界）
- 写突变继续进入 Codex 审阅/diff 路径，并保留 dedicated 工具的 hook/telemetry 身份
- rollout gate 开启后，Compatible provider 的 `auto` 模式默认可见 dedicated 工具
- Anthropic `auto` 保留原生 `text_editor`，dedicated 仅显式实验启用
- shell 描述与系统指引在 dedicated 实际可见时禁止普通文件 IO 走 shell
- OpenAI Responses 在本变更中保持 `apply_patch` 主编辑面

**Non-Goals:**

- 不实现 Claude Code 的完整产品 UI（VSCode diff、完整权限对话框矩阵）
- 不删除 `apply_patch` 或原生 `text_editor` 执行器
- 不把工具改成 Anthropic 原生 `text_editor_*` type（保持跨 wire function tools）
- 本期不做 Glob/Grep 专用工具
- 不引入第二套 filesystem crate；复用现有 ExecutorFileSystem / sandbox

## Decisions

### 1. 工具命名：Codex snake_case，不照搬 PascalCase

采用：

- `read_file`
- `edit_file`
- `write_file`

**理由**：与 `shell_command` / `apply_patch` / `view_image` 一致；避免 Claude Code
品牌名与 Codex 工具注册风格冲突。提示文案明确语义（Read/Edit/Write 行为），
不依赖 PascalCase 训练名。

**备选（否决）**：Claude wire 上 rename 为 `Read`/`Edit`/`Write` — 增加双名映射与
hook/权限规则复杂度，收益不确定；若后续实测 Claude 选中率不足，可另开增量提案。

### 2. 默认启用范围：一个 rollout gate + 一个有穷策略枚举

实现引入单一 rollout feature：`dedicated_file_tools`。在 rollout 阶段它默认关闭；
关闭时完全恢复现有工具面。开启后，Claude 使用 `claude_file_tool_mode`：

- `auto`（默认）：支持 native editor 的普通 Anthropic Claude 只广告原生
  `text_editor`；Compatible / DeepSeek 与 Kimi K3 只广告 `read_file` /
  `edit_file` / `write_file`
- `dedicated`：所有 Claude provider 只广告 dedicated 文件工具；用于 Anthropic
  渐进实验/A-B
- `dedicated_with_apply_patch`：广告 dedicated 文件工具与 `apply_patch`；仅用于显式
  fallback/诊断

| Wire / mode | dedicated 文件工具 | `apply_patch` | 原生 `text_editor` |
| --- | --- | --- | --- |
| Claude Anthropic / gate off | 不广告 | 保持现状 | 保持现状 |
| Claude Anthropic / `auto` | 不广告 | hidden dispatch | 唯一首选编辑面 |
| Kimi K3 / `auto` | 广告 | hidden dispatch | 不广告（现有选择逻辑明确排除） |
| Claude Anthropic / `dedicated` | 广告 | hidden dispatch | 不广告 |
| Claude Anthropic / `dedicated_with_apply_patch` | 广告 | 广告 | 不广告 |
| Claude Compatible / `auto` 或 `dedicated` | 广告 | hidden dispatch | 不支持 |
| Claude Compatible / `dedicated_with_apply_patch` | 广告 | 广告 | 不支持 |
| OpenAI Responses | 本变更不广告 | 保持主编辑面 | N/A |

**理由**：Compatible/Windows 痛点最大且拿不到 Anthropic native tool；Kimi K3
虽然走 Claude wire，但当前 `native_tool_selection_for_prompt` 明确排除 native
`text_editor`，因此按 dedicated-capable Compatible 分支处理。其他 Anthropic Claude
已有模型原生、Codex 可执行的 `text_editor`，不应未经数据验证就默认替换。
Responses 继续使用 `apply_patch`，若未来需要 dedicated opt-in，应另开增量提案，
避免一个 feature 同时承担两个 wire 的 rollout 语义。

策略在 Rust 中必须使用枚举，不使用多个相互依赖的 bool，避免产生“prefer=false 但
fallback=true”之类无效组合。配置变更同步 schema；未识别的枚举值加载失败。

### 3. 写路径：共享 reviewable mutation，不递归调用公开 handler

`edit_file` / `write_file` MUST：

1. 校验路径、workspace、权限/deny rules
2. 检查 turn-scoped read receipt
3. 构造带 expected-content fingerprint 的 Add/Update mutation
4. 通过共享的 approval、sandbox、apply、file-change/diff 事件路径提交
5. commit 前（包括等待用户审批之后）再次校验 expected fingerprint；新建文件要求
   目标在 commit 时仍不存在
6. 返回简洁成功/错误文本，并在成功后更新 read receipt

**理由**：避免旁路现有 apply-patch 审阅与 telemetry；同时防止把 function payload
转换成整文件文本 patch、重新解析、再把工具名改成 `apply_patch`。外层 hooks、
telemetry 与模型结果继续使用 `edit_file` / `write_file` 身份，内部审批 UI 和
file-change 事件复用现有 patch 语义。

`read_file` MUST 走 `ExecutorFileSystem`（不经 shell），优先使用 metadata + stream
实现有界读取和 fingerprint；输出带行号，支持 1-based `offset`/`limit`，并报告
返回范围、总行数、是否完整及 receipt 是否具备写资格。所有输出必须在构造
`FunctionToolOutput` 前满足硬 byte/token 上限，不能依赖后续通用截断悄悄裁剪。

### 4. read receipt：turn-scoped、有界、environment-aware

状态只在一个 user turn 的多个 sampling/tool-loop step 间共享；新 user turn 创建新
store。不要放进 session 永久状态，也不从 rollout/history 重建。

```text
(environment_id, canonical_path_uri) -> {
  content_fingerprint,
  size_bytes,
  modified_at_ms,
  observed_line_ranges,
  full_coverage,
  write_eligible
}
```

规则：

- key 使用选中 executor 的 canonical `PathUri`，不得用宿主机 `PathBuf`、字符串
  separator 替换或大小写折叠代替 filesystem canonicalization
- 模型 path 参数先以目标 environment 的 `cwd: PathUri` 做 lexical join；已存在路径
  再经 `ExecutorFileSystem::canonicalize`；新文件 canonicalize 父目录后拼接 basename
- key 必须包含 `environment_id`；多环境工具 schema 暴露 `environment_id`，省略时使用
  primary environment
- store 只保存固定大小 fingerprint 和合并后的范围，不保存无界全文；entry 数、范围数
  与总内存均有硬上限，超限采用明确错误或有界淘汰
- `edit_file` 要求每个被替换 occurrence 的完整行范围都已观察；`replace_all=true` 时
  所有 occurrence 都必须落在已观察范围
- `write_file` 覆盖要求 `full_coverage=true`；创建新文件不要求 receipt
- 每次 mutation 都重新读取/流式 fingerprint 当前文件并与 receipt 比较，不能只在
  `mtime` 增大时比较；mtime 改变但 fingerprint 相同允许继续
- mutation 成功后以实际落盘内容更新 receipt；无法确认实际内容时使 receipt 失效
- `read_file` 与依赖它的 Edit/Write 不得在同一个 parallel tool batch 中建立依赖；
  提示要求依赖调用跨 response 顺序执行，runtime 继续串行化写 mutation

对超过 model output 上限、但低于 file-tool editable byte cap 的文件，`read_file` 可
返回 partial 内容并在同一次流式读取中计算全文件 fingerprint，因此范围内 Edit 仍可
执行。超过 editable cap 的文件可读指定范围，但 receipt 标记为不可写，并明确允许
使用专门脚本/shell 处理；这是“优先 dedicated”规则的正式例外。

`..` 本身不是错误：只有 lexical resolve + canonicalization 后逃出允许读取/写入范围
才拒绝，避免误伤位于 cwd 相邻目录但仍属于 workspace root 的合法路径。

### 5. 文本、编码与行尾契约

- 首期只处理普通文本文件；目录、图片、PDF、设备文件和二进制返回可纠正错误，图片
  继续使用 `view_image`
- 复用或抽取现有 apply-patch 可 round-trip 的文本解码/编码能力；不得另写一套互相
  漂移的编码探测
- read 输出和 `old_string` 匹配使用 LF-normalized 文本；行号不属于文件内容
- `edit_file` 必须保持未修改内容、原编码和原行尾，至少覆盖 UTF-8、UTF-8 BOM、LF
  与 CRLF；无法安全 round-trip 的 UTF-16/legacy/binary 必须拒绝，不能静默转码
- `write_file` 创建使用模型提供的内容（默认 UTF-8）；覆盖已有可 round-trip 文本时
  保持其编码，内容中的换行是显式整文件结果
- fingerprint 基于 mutation 层实际比较的规范化表示，并包含必要的编码身份，避免
  两种不同字节表示误判为同一可安全写状态

### 6. Edit 语义：精确字符串替换，不是行号 patch 语言

入参：

- `path`（或 `file_path`，实现时统一一个名字并在 schema 锁定）
- `old_string` / `new_string`
- `replace_all`（默认 false）

行为：

- `old_string` 找不到 → 模型可纠正错误
- 多处匹配且未 `replace_all` → 错误并提示加上下文或 `replace_all`
- 空 `old_string` + 文件不存在 → 允许作为创建（或强制走 `write_file`；**选定**：
  创建只用 `write_file`，Edit 不负责创建，减少双入口）

**选定**：创建/整写只走 `write_file`；`edit_file` 只改已存在文件。

### 7. 提示与 shell 描述：由实际可见工具驱动

在 Claude（及启用 dedicated 工具的会话）注入简短工具使用规则：

- 读 → `read_file`，不要 `cat`/`Get-Content`/`type`
- 改 → `edit_file`，不要 `sed`/`Set-Content` 局部改
- 新建/整写 → `write_file`，不要 heredoc/重定向
- shell 仅用于真正需要 shell 的系统命令，或 dedicated 工具明确报告超出其
  text/size/encoding 能力的例外

修改 [`shell_spec.rs`](codex-rs/core/src/tools/handlers/shell_spec.rs) 中所有可能
model-visible 的 shell spec（包括 `exec_command` 与 `shell_command`）：删除/改写把
普通文件读写当成 shell 主业的示例，并在 dedicated 实际可见时增加同一条简短约束。
不能仅用 core 的 `cfg!(windows)` 判断远程 executor 平台；通用反文件-IO 指引不依赖
宿主平台，Windows 专属文案若保留则必须来自目标 environment 能力。Claude native
`bash` 若可见，也由 system guidance 覆盖。

### 8. 与原生 `text_editor` / `apply_patch` 的互斥

Claude 规划严格按 §2 真值表执行：

- `auto` + 支持 native editor 的 Anthropic：native `text_editor` 可见；
  `apply_patch` handler hidden dispatch
- `auto` + Compatible/Kimi K3：dedicated 可见；`apply_patch` handler hidden dispatch
- `dedicated`：dedicated 可见；native `text_editor` 与 `apply_patch` 均不广告
- `dedicated_with_apply_patch`：dedicated 与 `apply_patch` 可见；native
  `text_editor` 不广告

`ClaudeTextEditorHandler` / `ApplyPatchHandler` 在对应不可见模式下保持 hidden dispatch，
用于旧 transcript/恢复中的合法历史调用。native `text_editor` 的能力判断不得再依赖
“`apply_patch` 必须 model-visible”，而应依赖共享 mutation runtime 是否注册可执行。

### 9. 模块边界：少扩 `codex-core` 中央文件

- 新模块：`codex-rs/core/src/tools/handlers/read_file.rs`、`edit_file.rs`、
  `write_file.rs`（或 `file_tools/` 目录）+ 各自 `*_tests.rs`
- 共享：receipt store、schema、PathUri 解析与 handler 放 `file_tools/` 目录；共享
  reviewable mutation orchestration 由 ApplyPatch 与 dedicated handlers 共同调用
- `spec_plan.rs` 只做注册编排，不塞大段 schema
- Claude 选择逻辑留在 `claude.rs` 的 native selection / 可见工具过滤，保持薄封装
- 每个实现模块目标低于 500 LoC；若 mutation/receipt 逻辑增长，按目录边界继续拆分，
  不新增只调用一次的小 helper

### 10. 分期：任何可独立落地阶段都满足安全不变量

| Phase | 内容 | Model-visible |
| --- | --- | --- |
| A | 共享 mutation precondition、PathUri 解析、receipt store、三个 hidden handlers、单元/remote integration tests | 否 |
| B | rollout feature、`auto` 策略、Compatible dedicated 广告、互斥、提示与请求真值表测试 | 仅 Compatible，且完整安全契约已就绪 |
| C | Anthropic `dedicated` / `dedicated_with_apply_patch` 实验模式、指标与回滚验证 | 显式 opt-in |
| D | 依据工具选中率、shell fallback、编辑成功率决定是否另提案调整 Anthropic 默认 | 待数据决定 |

若实际 diff 超过仓库 change-size 指引，A/B/C 分独立 PR；Phase A 不得注册为 direct，
Phase B 必须把广告、互斥、提示和安全状态一起交付，不能再拆出不安全中间态。

### 11. 测试策略

- 单元：schema、PathUri join/canonical key、edit 唯一匹配/`replace_all`、范围覆盖、
  store 上限、fingerprint 冲突、CRLF/编码往返、新建 no-clobber precondition
- core integration 使用 `TestCodexBuilder::build_with_auto_env()`：
  - `read_file` → `edit_file` → 第二次 `edit_file` 成功，验证写后 receipt 更新
  - 未读直接 Edit/overwrite Write 失败且无写盘
  - partial read 允许范围内 Edit、拒绝范围外 occurrence 与整文件 Write
  - same-mtime 内容变化拒绝；mtime 变化但 fingerprint 相同允许
  - approval 等待期间外部修改导致 commit-time 拒绝
  - 多 environment 相同 path 不共享 receipt；foreign Windows PathUri 可读写
  - UTF-8 BOM/CRLF 不被静默转码；不支持编码返回可纠正错误
  - Compatible `auto`、Kimi K3 `auto`、Anthropic `auto`/`dedicated`/fallback 的
    完整请求真值表
  - hidden handlers 能消费旧 transcript 调用，但不出现在 model-visible tools
- remote：Linux exec-server suite + Wine Windows exec-server Bazel target
- shell guidance 测试请求中的动态行为：仅在 dedicated 实际可见时出现，并覆盖
  `exec_command` / `shell_command`；不为静态常量本身添加无行为价值测试
- 若 TUI 渲染新 tool name/result，新增或更新 `insta` snapshot

## Risks / Trade-offs

- **[Risk] 工具过多，模型仍选 shell** → Mitigation：互斥广告 + 强提示 + Windows
  描述去偏置；集成测试锁定请求 tools 列表
- **[Risk] Edit 唯一匹配体验差** → Mitigation：错误信息要求加上下文；支持
  `replace_all`；文档/提示说明从 `read_file` 行号后复制时不要带行号前缀
- **[Risk] 写路径旁路审阅或审批期间发生 TOCTOU** → Mitigation：共享 reviewable
  mutation + commit-time expected fingerprint/no-clobber precondition
- **[Risk] OpenAI 路径误开 dedicated** → Mitigation：Responses 明确不在本变更启用；
  分 wire/mode 真值表写入请求测试
- **[Risk] 与 native text_editor 双栈维护** → Mitigation：同一 turn 只保留一个首选
  编辑面；两套外部 schema 共享 mutation runtime，不复制写盘逻辑
- **[Risk] 多 environment/foreign OS 路径错绑** → Mitigation：key 固定包含
  environment id + canonical `PathUri`，core integration 使用 auto-env/Wine
- **[Risk] receipt 无界或大文件永久不可编辑** → Mitigation：固定大小 fingerprint、
  有界 store、partial observed ranges 与单独 editable byte cap
- **[Risk] CRLF/编码被静默改变** → Mitigation：共享 round-trip decoder、编码/行尾
  测试；不支持的文本编码显式拒绝

## Migration Plan

1. 审核通过本 OpenSpec
2. 按 Phase A 隐藏实现并通过本地/remote 测试，不改变 model-visible 工具面
3. Phase B 开启 `auto` rollout：Compatible 与 Kimi K3 改为 dedicated，其他
   Anthropic 保持 native `text_editor` 单主编辑面
4. Phase C 显式内测 Anthropic dedicated，观察工具选中率、shell fallback、编辑成功率
   与 stale-retry 率；不足再考虑 Claude 侧别名
   `Read`/`Edit`/`Write`
5. 回滚：关闭 `dedicated_file_tools` feature，恢复原工具广告

## Open Questions

（提案阶段给出默认答案；审核时可改）

1. **Claude 是否保留 `apply_patch` fallback？** 已选定：`auto`/`dedicated` 不广告，
   仅 `dedicated_with_apply_patch` 显式广告；handler 保持 hidden dispatch。
2. **Responses 是否同期开启 dedicated？** 已选定：否，未来另开提案。
3. **参数名 `path` vs `file_path`？** 默认：`path`（与现有 Codex 工具一致）；若希望贴近 CC 用
   `file_path`，审核时二选一锁定 schema。
4. **`view_image` 与 `read_file` 图片职责？** 已选定：图片/截图继续 `view_image`；
   `read_file` 首期只处理普通文本并返回可纠正指引。

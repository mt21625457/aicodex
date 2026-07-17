## Why

Claude 协议路径下，Codex 缺少 Claude Code 式的一等文件工具面：没有独立
`Read`，写/改主要依赖 `apply_patch`（对 Claude 摩擦大）或条件启用的原生
`text_editor`，读文件则经常回落到 `shell_command`。在 Windows 上，现有
`shell_command` 描述以 PowerShell 为主，并把 `Get-ChildItem`、`Select-String`
等文件枚举/内容搜索操作列为示例；它并没有直接包含 `Get-Content` 或
`Set-Content`。真正的偏置来自缺少专用 `Read`/`Write` 工具时，shell 成为
Compatible provider 完成普通文件 IO 的唯一直接路径，模型因而容易自行选择
PowerShell/cat/重定向读写，绕过审批友好的内置编辑路径，也更难做 diff 审阅与
先读后写一致性。

本地参考实现 Claude Code（`/Users/mt/code/mt-ai/cc`）证明了更稳的工具契约：
专用 `Read` / `Edit` / `Write`、系统提示禁止用 Bash 做文件 IO、Edit/Write
强制先 Read。本变更借鉴该**工具面与选型约束**，在 Codex 内落地为
`read_file` / `edit_file` / `write_file`，并优先改善 Claude（含兼容）与
Windows 路径；不照搬 Claude Code 的 UI/权限产品面，也不替换 OpenAI
Responses 上已验证的 `apply_patch` 默认。

> **审核门禁**：本变更处于提案阶段。在提案、design、specs、tasks 通过人工
> 审核前，**不得开始实现代码**。

## What Changes

- 新增 Codex 一等文件工具（function tools，全 wire 可规划）：
  - `read_file`：按路径读文件（行号输出、offset/limit、大小/token 上限）
  - `edit_file`：精确 `old_string` / `new_string` 替换（可选 `replace_all`）
  - `write_file`：创建或整文件覆盖写
- 执行层复用 Codex 既有 `ExecutorFileSystem`、workspace 边界、审批/sandbox、
  diff/patch 上报；写操作进入共享的 reviewable file-mutation 路径，不通过递归
  调用公开 `apply_patch` handler 改写工具身份。
- 引入 **turn-scoped read receipt**（先读后写）：以
  `(environment_id, canonical PathUri)` 为键，保存固定大小 fingerprint、mtime、文件
  大小与已读范围。Edit/Write 在未先读或内容 fingerprint 冲突时拒绝；mtime 只作
  诊断/快速信号，不能成为一致性的唯一依据。
- partial read 可为范围内的精确 Edit 建立 receipt；整文件 Write 必须有完整读取覆盖。
  文件工具对可 fingerprint/可编辑文件设置硬大小上限，超限时返回允许使用专门
  shell/脚本处理的明确例外，而不是让文件永久不可编辑。
- 工具选型引导（CC 思路）：
  - 仅当三个 dedicated 工具实际 model-visible 时注入：读用 `read_file`，改用
    `edit_file`，新建/整写用 `write_file`；普通文件 IO 不走 shell
  - `exec_command` / `shell_command` 不再把 PowerShell 文件枚举/搜索示例扩展成
    普通文件 IO 的默认路径；远程 Windows 文案依据目标 environment，而不是仅
    依据 core host
  - binary、非支持编码或超过 editable cap 是允许专门 shell/脚本处理的明确例外
- Claude 工具规划收口，避免三重编辑面：
  - rollout gate 开启且模式为 `auto` 时，普通 Anthropic Claude 保留模型原生
    `text_editor` 作为唯一首选编辑面；Compatible / DeepSeek 与 Kimi K3 广告
    dedicated 工具（K3 当前明确不启用 native `text_editor`）
  - 显式 `dedicated` 模式才在 Anthropic 上以 dedicated 工具替代原生
    `text_editor`，用于渐进实验/A-B
  - `apply_patch` 默认仅保留 hidden dispatch 兼容；只有显式
    `dedicated_with_apply_patch` 模式才同时广告
- OpenAI Responses 在本变更中保留 `apply_patch` 为主编辑面且不广告 dedicated
  文件工具；未来若要开启必须另开增量提案。
- 增加 mock 端到端工具环测试（Claude 工具真值表、先读后写、连续编辑、远程
  Linux/Windows executor、多 environment、CRLF/编码、拒绝 shell 偏置回归）。
- 在 `docs/config.md` 记录 Claude/Windows 文件工具策略与 feature 门控；若
  触及 `ConfigToml`/feature flags，同步 regenerate `config.schema.json`。

## Capabilities

### New Capabilities

- `dedicated-file-tools`：Codex 提供 `read_file` / `edit_file` / `write_file`
  一等文件工具；读有行号与范围限制；写/改经 workspace、审批、diff 路径；
  Edit/Write 强制先读与 fingerprint 冲突检测；路径使用 environment-aware
  `PathUri`；shell 不得成为默认简单文件 IO 路径。
- `file-tool-selection-policy`：工具规划与提示策略优先专用文件工具；Windows
  shell 描述与 Claude/通用指令禁止用 PowerShell/`cat`/heredoc 做简单读写；
  Claude 按 `auto` / `dedicated` / `dedicated_with_apply_patch` 策略收口
  `text_editor`/`apply_patch` 广告面。

### Modified Capabilities

- `claude-wire-api-support`：Claude Messages 工具规划必须能广告并执行
  dedicated 文件工具；Compatible provider 不再只能依赖 `apply_patch`+shell
  完成读写；与原生 `text_editor` 的互斥/降级规则可验证。

## Impact

- 受影响 crate：
  - `codex-rs/core` — 新 handler、turn-scoped read receipt、共享 reviewable
    mutation 路径、spec_plan 注册、提示片段、Claude 工具选择与 mock 集成测试
  - `codex-rs/tools` — 仅当现有普通 function-tool Claude 序列化不能满足 schema
    时才修改；不得为 dedicated 工具引入 provider-specific 重复协议层
  - `codex-rs/apply-patch`（若需要）— 抽取/扩展可复用的文本解码、编码保持或
    commit-time precondition；不复制第二套编码实现
  - `codex-rs/prompts`（若指令模板落于此）— 文件工具使用指引
  - `docs/config.md` / `codex-rs/core/config.schema.json` — feature/策略说明与
    schema（若配置变更）
- 明确复用、不重造：
  - ApplyPatch 的 FS sandbox / 审批 / file-change 流作为共享写路径底层，但不递归
    调用公开 handler 或把 dedicated tool identity 改成 `apply_patch`
  - 现有 `ClaudeTextEditorHandler` 继续服务 Anthropic `auto`，并与 dedicated
    handlers 共享 mutation runtime
- 范围外：
  - 不实现 Claude Code 的完整权限 UI / VSCode diff 产品面
  - 不删除 `apply_patch`（OpenAI 主路径与 fallback 仍需要）
  - 不把 dedicated 文件工具做成 Anthropic 原生 `text_editor_*` type（保持
    Codex function tools，全 wire 可用）
  - 不在本变更中引入 Glob/Grep 专用工具（可另开提案；本期只收口文件读写）
- 主要风险：
  - 工具面叠加（`apply_patch` + dedicated + `text_editor`）导致模型更混乱 —
    必须靠规划互斥与提示收口
  - Edit 唯一匹配失败率高 — 需清晰错误与 `replace_all`
  - 先读后写状态与多环境/相对路径不一致 — 必须以 environment-aware
    `PathUri` 和远程 executor 测试锁定
  - partial/full read 与大小上限冲突 — receipt 记录读取覆盖，Edit 只改已见范围，
    Write 要求完整覆盖
  - CRLF/legacy encoding 被静默改写 — Edit 保持未修改字节/行尾，无法安全往返
    的编码必须拒绝
  - 写路径若绕过 apply-patch 审阅会破坏现有 UX — 设计要求突变仍进审阅路径
  - OpenAI 模型若默认打开 dedicated 工具可能偏离 `apply_patch` 训练 —
    Responses 默认保持 `apply_patch` 优先
- 回滚策略：
  - rollout feature 关闭 dedicated 文件工具广告与注册，恢复
    `apply_patch`/`text_editor`/shell 现状；handlers 可保留但不可见
- 验证命令（实现阶段，审核通过后）：
  - `cd codex-rs && just fmt`
  - `cd codex-rs && just test -p codex-tools`（仅当 tools crate 变更）
  - `cd codex-rs && just test -p codex-apply-patch`（仅当共享 mutation/编码层变更）
  - `cd codex-rs && just test -p codex-core`（文件工具与 Claude wire 相关用例，
    使用 `build_with_auto_env()`）
  - 远程 Linux executor：按 `$remote-tests` 运行 core integration suite
  - 远程 Windows executor：`bazel test //codex-rs/core:core-all-wine-exec-test`
  - schema 变更时：`cd codex-rs && just write-config-schema`
  - `openspec validate add-cc-style-file-tools --strict`
- 外部参考（只比较行为，不引入依赖）：
  - `/Users/mt/code/mt-ai/cc/src/tools/FileReadTool`
  - `/Users/mt/code/mt-ai/cc/src/tools/FileEditTool`
  - `/Users/mt/code/mt-ai/cc/src/tools/FileWriteTool`
  - `/Users/mt/code/mt-ai/cc/src/constants/prompts.ts`（专用工具优先于 Bash）
- 相关 OpenSpec（边界）：
  - `complete-claude-native-protocol-support` — 原生 `text_editor`/`bash`；本变更
    在 dedicated 工具启用时对其广告面做互斥/降级，不推翻其执行安全模型
  - `harden-claude-tool-call-contracts` — `apply_patch` Claude 契约硬化；本变更
    降低 Claude 对 `apply_patch` 的依赖，但不删除 fallback

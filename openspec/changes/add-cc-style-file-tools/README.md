# add-cc-style-file-tools

## 状态

**提案阶段 — 需人工审核通过后才能开始实现。**

## 目标（一句话）

借鉴 Claude Code 的 `Read` / `Edit` / `Write` 思路，在 Codex 增加
`read_file` / `edit_file` / `write_file`，用 environment-aware PathUri、turn-scoped
read receipt、commit-time 冲突检查与提示/规划收口，优先解决 Claude Compatible
（尤其远程 Windows）用 PowerShell/`apply_patch` 摩擦做文件 IO 的问题。

## 文档地图

| 文件 | 内容 |
| --- | --- |
| [proposal.md](./proposal.md) | Why / What / Capabilities / Impact |
| [design.md](./design.md) | 命名、wire 默认、state、互斥、分期、风险 |
| [tasks.md](./tasks.md) | 可勾选实施清单（含审核门禁 §0） |
| [specs/dedicated-file-tools/spec.md](./specs/dedicated-file-tools/spec.md) | Read/Edit/Write 行为与安全 |
| [specs/file-tool-selection-policy/spec.md](./specs/file-tool-selection-policy/spec.md) | 提示与 Windows shell 选型策略 |
| [specs/claude-wire-api-support/spec.md](./specs/claude-wire-api-support/spec.md) | Claude 广告/互斥/工具环 |

## 已确认的设计默认（待审核）

1. 工具名：`read_file` / `edit_file` / `write_file`（不照搬 PascalCase）
2. 单一 rollout gate + `auto` / `dedicated` /
   `dedicated_with_apply_patch` 枚举，不使用多个互相依赖 bool
3. rollout `auto`：Compatible 与 Kimi K3 使用 dedicated；其他 Anthropic 保留
   单一原生 `text_editor`；Responses 保持 `apply_patch`
4. receipt key 为 `(environment_id, canonical PathUri)`，只在当前 user turn 有效且有
   硬上限；partial read 只授权已观察范围内的 Edit
5. 写突变走共享可审阅 file-change 路径，并在审批后 commit 前重验 fingerprint；
   成功 Edit/Write 必须以实际落盘内容刷新 receipt，dedicated hooks/telemetry 不伪装
   成 `apply_patch`
6. 创建只用 `write_file` 且 commit-time no-clobber；`edit_file` 只改已存在文件
7. Edit 保持支持的编码/行尾；无法无损往返的文本或二进制显式拒绝
8. Phase A 隐藏实现通过 local/remote 测试后，Phase B 才一次性曝光 Compatible 工具
9. 外部参考：`/Users/mt/code/mt-ai/cc` 行为，不引入其依赖

## 校验

```bash
openspec validate add-cc-style-file-tools --strict
openspec status --change add-cc-style-file-tools
```

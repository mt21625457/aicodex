# add-cc-style-file-tools

## 状态

**提案阶段 — 审核修订已批准；Phase A 未完成且未获实现授权前不得实施。**

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

## 已确认的设计默认（审核修订已锁定）

1. 工具名：`read_file` / `edit_file` / `write_file`（不照搬 PascalCase）
2. 单一 rollout feature config：`[features.dedicated_file_tools]` 下使用 `enabled` +
   `mode = "auto" | "dedicated" | "dedicated_with_apply_patch"`；gate 关闭时三个新
   handlers 不注册、不广告、不注入提示
3. rollout `auto`：Compatible 与 Kimi K3 使用 dedicated；其他 Anthropic 保留
   单一原生 `text_editor`；Responses 保持 `apply_patch`
4. receipt key 为 `(environment_id, canonical PathUri)`，只在当前 user turn 有效且有
   硬上限；receipt 携带 sampling-step provenance，partial read 只授权已观察范围内且
   来自更早 provider response 的 Edit
5. fingerprint 固定为原始文件字节的 SHA-256；写突变走共享可审阅 file-change 路径，
   并在目标 executor 内执行带 `must_not_exist` / `match_sha256` precondition 的提交；
   成功 Edit/Write 必须以实际落盘内容刷新 receipt，dedicated hooks/telemetry 不伪装成
   `apply_patch`
6. 创建只用 `write_file` 且 commit-time no-clobber；`edit_file` 只改已存在文件
7. Edit 复用 apply-patch 当前可无损往返的编码集合并保持 BOM/行尾；UTF-16、无法
   round-trip 的文本或二进制显式拒绝
8. Phase A 隐藏实现通过 local/remote 测试后，Phase B 才一次性曝光 Compatible 工具
9. 外部参考：`/Users/mt/code/mt-ai/cc` 行为，不引入其依赖
10. 已锁定安全上限：单次模型可见输出 `64 KiB` 且约 `10,000 tokens`、editable
    `8 MiB`、大文件单次扫描 `64 MiB`、`limit <= 2,000`、mutation 参数合计
    `64 KiB` 且约 `10,000 tokens`、receipt `128 entries / 64 ranges per entry /
    1,024 total ranges / 256 KiB accounted memory`

## 校验

```bash
openspec validate add-cc-style-file-tools --strict
openspec status --change add-cc-style-file-tools
```

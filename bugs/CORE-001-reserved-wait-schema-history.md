# CORE-001：保留等待工具 schema 与旧会话回放兼容

- 严重性：P1，普通聊天在模型输出前失败。
- 确认日期：2026-11-03，时间采用 Asia/Shanghai。
- 状态：源码修复、聚焦回归、本地真实历史副本回放、合成旧会话真实网关验证通过；用户重新编译安装后的验收待完成。
- 内核修复提交：`faa4db871b`（公开 number 契约）、`5dcb382d6b`（历史兼容与参数错误终态）。
- 应用关联：aicodex-app-spec 的 CHAT-214，提交 `c42c55138`、`300a7d0`。

## 症状和直接根因

GPT-6-Astra 会在回答前拒绝请求，即使用户只输入 hi，也未调用工具：

```text
Invalid Value: 'tools'. Function 'collaboration.wait_agent' is reserved
for use by this model and must match the configured schema.
```

`collaboration.wait_agent.timeout_ms` 的服务端保留契约是 `number`。为兼容 Grok 返回的 `180000.0` 等数值表示而调整参数处理时，公开 schema 被收窄成 `integer`，违反服务端契约。整数毫秒的业务约束应由 handler 精确校验，不能改成不兼容的公开类型。

## 为什么重新编译后仍然失败

Responses Lite 将工具声明作为 `additional_tools` 写进会话历史。故障会话中既有旧的 `integer` 声明，也有升级后追加的 `number` 声明。服务端会检查历史声明，追加正确声明不能消除前面的非法定义。

确认依据：应用构建 `c42c55138` 在 2026-11-03 15:25 仍记录同一错误；该安装内核的新会话请求已使用 number。使用它在隔离 home 恢复故障会话副本，出站请求仍包含 integer 和 number，证实修复遗漏历史回放，而不是用户漏编译内核。

真实网关对照使用工具定义和合成输入，不包含真实聊天：

| 输入声明 | 观察到的结果 |
| --- | --- |
| 仅 number | HTTP 200、response.completed、OK |
| 旧 integer + 新 number | HTTP 200，但 SSE 中 invalid_request_error，未完成 |
| number + number | HTTP 200、response.completed、OK |

**防复发核心：修改当前定义后，必须继续检查旧会话实际发送的完整历史。新会话成功不能证明旧会话可恢复。**

## 为什么显示重新连接

实际失败可发生在 HTTP 200 已建立的 SSE 中：`error.type=invalid_request_error`、`param=tools`、`code=null`。旧解析依赖 code，未识别该终态错误，将其归入可重试流错误，导致 `Reconnecting... n/5`。

HTTP 200 仅表示响应已开始，不能证明模型请求成功；“重新连接”也不能证明本机断网。应检查流内错误与完成事件。服务端容量不足是独立原因，不属于本次 schema 修复，也不得将容量或限流错误一律改成参数错误。

## 当前修复与不得破坏的约束

1. 两代内置等待工具公开声明为 number。handler 接受精确整数表示 `180000.0`，拒绝非整数、溢出和精度损失。
2. 请求格式化只在副本中修正 `collaboration` / `multi_agent_v1` 命名空间下 `wait_agent.timeout_ms` 的已知旧 integer 类型；不改外部同名工具或未知 schema，不删除旧声明来隐藏错误。
3. 不改写已有 rollout 记录，保留用户输入、助手输出、工具调用参数及结果。恢复会话允许追加新回合，因此不能用整个文件哈希不变代替“既有记录未改写”的检查。
4. 被修正项的请求 ID 从原 ID 确定性派生，避免不同内容复用缓存身份；处理幂等，重试 ID 稳定。诊断只记录操作、兼容版本及修正数量。
5. 明确的流内 invalid_request_error 转为不可重试的 InvalidRequest，对外为 badRequest，保留具体原因。模型不存在、上下文超限、限流、容量和媒体错误继续使用专用语义；HTTP 非 2xx 保留原错误体处理。
6. 不把单纯重装、删除会话、关闭多 agent、扩大重试次数或静默回退当成根因修复。

## 责任代码与回归保护

路径均相对内核仓库根目录：

| 责任 | 入口 | 回归 |
| --- | --- | --- |
| 两代工具公开契约 | [multi_agents_spec.rs](../codex-rs/core/src/tools/handlers/multi_agents_spec.rs) | [multi_agents_spec_tests.rs](../codex-rs/core/src/tools/handlers/multi_agents_spec_tests.rs)：`wait_timeout_preserves_reserved_number_schema_for_both_tool_versions` |
| 请求副本历史兼容 | [client_common.rs](../codex-rs/core/src/client_common.rs)、[client_tool_schema_compat.rs](../codex-rs/core/src/client_tool_schema_compat.rs) | [client_tool_schema_compat_tests.rs](../codex-rs/core/src/client_tool_schema_compat_tests.rs)：两代工具、旧新混排、多条旧声明、稳定 ID、幂等、原记录和工具结果保留、未知定义不受影响 |
| 流内错误终态与对外错误码 | [codex-api/error.rs](../codex-rs/codex-api/src/error.rs)、[protocol/error.rs](../codex-rs/protocol/src/error.rs) | [responses_error_tests.rs](../codex-rs/codex-api/src/sse/responses_error_tests.rs)：`invalid_tool_schema_is_terminal_even_when_sse_has_started`、`generic_invalid_request_type_preserves_specific_error_codes` |

从内核仓库根目录执行以下命令；它们是本问题的聚焦回归，不代替受影响模块的完整验证：

```sh
cd codex-rs
RUST_MIN_STACK=16777216 cargo test -p codex-core --lib wait_
RUST_MIN_STACK=16777216 cargo test -p codex-core --lib client_
RUST_MIN_STACK=16777216 cargo test -p codex-api --lib
```

## 必须覆盖的验收矩阵

| 场景 | 通过条件 |
| --- | --- |
| 新会话 | 出站保留工具定义为 number，服务端返回完成事件 |
| 升级前产生的旧会话恢复 | 所有旧 integer 声明在请求副本中变为 number，既有聊天和工具结果保留 |
| 重试与再次恢复 | 兼容幂等，派生 ID 稳定，历史未被清空或覆盖 |
| HTTP 200 + 流内参数错误 | 原因与 badRequest 透传，无无效重连，后续 completed 不得覆盖失败 |
| 其他服务端错误 | 限流、容量、模型、上下文和媒体错误保留各自分类，未知错误不伪装成成功 |
| 编译安装 | 核对实际内核路径和构件，使用原故障旧会话验收，单独记录结果 |

已完成证据：历史恢复回归在接入修复前失败；修复后历史兼容 3 项、client 40 项、codex-api 271 项通过。新内核本地回放真实故障副本得到 number + number；合成旧会话联网恢复返回 OK；模拟同样 SSE 参数拒绝只发送一次请求并保留原因。

真实历史的联网复测未执行：自动审批拒绝了具体历史数据外发；使用合成历史完成网关验证。安装版最终验收由用户执行，未以这些结果宣称安装验收或服务端容量问题已解决。

## 0.167.1 合并回归

合并保留本地 `InvalidRequest` 的终态与 `badRequest` 映射、畸形响应终态，并纳入上游 `InvalidAudioInput` 专用分类。音频回归同时携带通用 `type=invalid_request_error` 和具体音频 code，验证 HTTP 与 SSE 均不被通用分类覆盖。

本次 `codex-api --lib` 沙箱内 250 通过、21 项因本地模拟服务端口绑定被拒失败；获准沙箱外重跑 271 通过。`codex-app-server-protocol --lib` 338 通过、1 个生成入口按设计忽略，`codex-protocol --lib` 384 通过。稳定及实验协议产物均从合并后的 Rust 定义重新生成。尚未创建合并提交；这些结果属于源码及本机模拟验证，不代替真实服务、包内或安装验收。

随后 nextest 跨模块聚焦集 1160 项全部通过，包含当前等待工具公开 schema、精确数值解析及 `client_` 历史请求兼容回归。新旧声明及原记录保留由合成测试验证，未外发用户历史。

最终扩展串行矩阵（关闭重试）运行 1613 项：1612 通过，1 项 Guardian 集成测试在 initialize 阶段超时，尚未进入 review 断言，不能将整组报告为通过。本问题的等待 schema、历史恢复及场景快照回归均通过；快照中的工具 hash 更新已核对仅对应保留的 `integer` → `number` 契约修复。同一 Guardian 测试二进制直接运行通过，但不替代矩阵失败结果。合并仍未提交，真实服务、包内及安装后验收未执行。

后续同构件 nextest 精确 Guardian 测试通过；将相同内容二进制复制到新 inode 可复现首次初始化超时、立即重跑通过，证据限于首次启动路径，不归因于未经验证的具体系统扫描机制。未改断言或扩大超时。最终复跑扩大至 1729 项，串行、零重试，1729 全部通过（248.572 秒）；按测试名核对覆盖此前全部 1613 项。最终源码的全工作区 `check --offline --tests` 通过。原始失败结果保留，以上仍非全仓所有测试或安装验收。

## 0.169.0 合并回归（进行中）

保留 number schema 与请求副本兼容，并接入上游 workspace、access program、
inference hook、预算和单录音错误分类。无效 access program 属于参数错误，
对外仍按本地合同映射 `badRequest`；workspace 专用分类与 4xx 不可重试
是独立断言，不能把“非 workspace”误认为“允许重试”。

修复后 `codex-api` 283/283、`codex-protocol` 391/391、
`codex-app-server-protocol` 340 通过（1 个生成入口按设计忽略）。
稳定/实验产物已重新生成并通过一致性测试。完整工作区及新旧会话运行验收
仍在进行，见 [CORE-006](CORE-006-upstream-0169-merge-compatibility.md)；
当前修复随合并提交保存；未打包、未安装，不以当前结果宣称全部验收完成。

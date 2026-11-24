# CORE-009：客户端工具搜索被编码为普通函数

## 症状和责任边界

已安装的 AICodex CLI 0.2.12 使用隔离 home 和本机模拟 Responses 服务。在工具声明仅包含 `type: "tool_search"`、`execution: "client"` 的情况下，把 `list_windows` 查询作为 `namespace: "tool_search"`、`name: "tool_search_tool"` 的普通 `function_call` 返回，内核报 `unsupported call: tool_searchtool_search_tool`；同一查询改用原生 `tool_search_call` 后能发现合成的延迟工具。

直接原因是调用类型与声明不一致。普通函数分派使用结构化命名空间和名称查找注册表；错误末尾是 `ToolName` 的展示文本，不能据此判断路由键被拼坏。调用在进入原生搜索处理器之前已被拒绝，没进入窗口枚举。本机历史记录也确认过同型拒绝，但不能证明错误调用由模型、网关还是其他上游适配器生成。

## 修复和兼容范围

- 在完成的响应项进入执行和持久化前统一类型：仅在本轮声明原生客户端搜索、名称精确匹配、调用 ID 非空、参数符合原生搜索结构且不含加密字段时处理。未提供状态或显式完成才参与兼容。
- 复用原生搜索的处理器、工具索引和权限；不公开普通函数别名，不改变工具定义或已有 rollout。保留原条目 ID 关联本地记录，出站格式化移除不符合原生条目类型的 `fc_` ID；调用和输出以原 `call_id` 关联。
- 不改写 WebSocket 服务端的缓存基线。如果服务端缓存仍是普通函数，类型差异使出站请求改走已有完整历史回放；正常原生调用仍用增量续传。

责任代码：[响应项完成处理](../codex-rs/core/src/stream_events_utils.rs)、[内核工具路由](../codex-rs/core/src/tools/router.rs)。

## 回归与验收

[集成回归](../codex-rs/core/tests/suite/search_tool_protocol.rs) 检查 Responses Lite 的实际出站声明和搜索回执、合成延迟工具执行、恢复后回放、普通函数错误仍成对，以及 WebSocket 原生和转换调用的续传差异。测试不需要真实桌面、凭据或历史数据。

```sh
cd codex-rs
cargo test --locked -p codex-core --test all search_tool
cargo test --locked -p codex-core --lib wait_
cargo test --locked -p codex-core --lib client_
cargo test --locked -p codex-api --lib
```

独立构建的 Windows CLI 使用同一回环 SSE 与合成 MCP 夹具对照：旧安装版只接受原生搜索；修复构件接受两种格式，均发现 `mcp__fixture_desktop.list_windows`，下一次实际请求包含同一 `call_id` 的原生搜索调用和输出。工具搜索集成 23 项、待处理工具链路 52 项、客户端请求 41 项、Responses API 单元 290 项通过。

本次修复尚未提交。验收不依赖原网关重新登录；已验证本地构建的 CLI 协议链路，未替换或重新验收桌面安装包。

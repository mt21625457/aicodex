## Why

当 Codex 以 **Kimi** 为主模型（Claude Messages Compatible / Kimi K3 等路径）运行时，
本地 `web_search` 工具虽会广告给模型，但执行层仍**强制**走 OpenAI
`POST …/v1/alpha/search`（`WebSearchHandler::run_openai_web_search`），并在非
OpenAI 主模型时使用 fallback 模型 slug（如 `gpt-5.2-codex`）。

结果是：

- **纯 Kimi 登录、无 OpenAI 鉴权**时，查网常在 auth / 请求阶段失败 → 能力实质偏弱
- 与官方 kimi-code CLI 行为不一致：kimi-code 使用 Moonshot 官方
  `POST` search URL（body `{ "text_query" }`），返回 title/url/snippet，并提示
  用 `FetchURL` 读全文
- 在 Feature `standalone_web_search` 开启时，`web.run` 同样打 `alpha/search`，
  Kimi 会话仍无原生简单搜出口

本变更**借鉴 kimi-code 的官方 Moonshot 简单搜索方案**（行为对齐，不引入其
TypeScript 依赖），为 Kimi 主模型路径提供可工作的查网后端；**不**试图用刮页库
或 xynehq/websearch 替代，也**不**在本期复刻 OpenAI `SearchCommands` 满血套件。

> **审核门禁**：实现前须确认 `design.md`「审核决议」并勾选 `tasks.md` §0.1。
> 2026-07-17 的第二轮审核批准仅覆盖提案修订，不代表 Rust 实现获批。

## What Changes

- 引入 **Moonshot 简单搜索后端**（对齐 kimi-code）：
  - `POST` 完整 search URL（显式配置或由 provider `base_url` 派生 `{base}/search`）
  - JSON body：`{ "text_query": "<query>" }`
  - 鉴权：`Authorization: Bearer <token>`（`[moonshot_search].env_key` / `api_key`，
    或仅在派生/同源 URL 上复用当前主 provider auth；Phase A **不含** OAuth refresh 栈）
  - 显式 search URL 与主 provider 不同源时，MUST 使用独立 Moonshot 凭据，
    禁止把主 provider token 发送到跨域端点
  - 有 call_id 时 MUST 发送 `X-Msh-Tool-Call-Id`
  - 解析 `search_results[]` → title / url / snippet（可选 date / site_name）；
    禁止注入响应全文 `content`
  - 对 HTTP body、结果数、字段长度和最终 tool 输出设置硬上限；最终模型输入
    MUST 不超过 8K tokens
  - 格式化为模型可读 tool 文本（含引用提醒），并保持 external-context 标记
- **后端路由**（`WebSearchHandler` + `ext/web-search` 通过独立共享 crate 复用）：
  - Feature `kimi_moonshot_web_search`（默认 true）且 `moonshot_search.enabled`
    （默认 true）且主模型 slug 命中 Kimi 启发式 → Moonshot
  - 否则 → 现有 OpenAI `alpha/search`
  - **禁止** OpenAI 失败后的自动降级
- Kimi 路径 `SearchCommands` **严格子集**（见 design R5）：
  - 支持 query / queries / `search_query[].q`（最多 4，顺序执行）
  - `recency` / `domains` 忽略并注明
  - 任一富命令非空 → **整次调用** unsupported（即使同时带 query）
  - 未知顶层字段、超过 4 条规范化查询、空查询 → **整次调用**校验失败；
    不允许静默忽略未来命令后半执行
- 配置：根段 `[moonshot_search]`（`enabled` / `base_url` / `env_key` / `api_key` /
  `custom_headers`）；**不**复用 `[tools.web_search]`（该段服务 OpenAI hosted 语义）
- 工具广告：继续 `web_search` / 可选 `web.run`；Kimi standalone 的扩展可见性
  必须显式放行；description 在 Phase A 说明「摘要 + 需读页再 fetch」
- 事件：保持 WebSearch begin/end；standalone Moonshot 结果提供有界结构化结果，
  两条输出路径均标记为 external context
- 测试：mock Moonshot、Kimi 不打 `alpha/search`、Kimi `web.run` 可见且命中
  Moonshot、非 Kimi 与 kill-switch 回归、跨域凭据拒绝、所有硬上限
- schema 与 API 文档：`just write-config-schema`；若 standalone 事件结果说明变化，
  仅更新允许的 `codex-rs/app-server/README.md`，不向 `docs/` 添加通用用户文档

## Capabilities

### New Capabilities

- `kimi-moonshot-web-search`：当会话主模型判定为 Kimi 且 Moonshot 路由启用时，
  Codex 必须将本地 `web_search`（及若启用的 `web.run`）执行到 Moonshot 官方
  简单搜索 API（`text_query`），在无需 OpenAI 鉴权的情况下返回可用的
  title/url/snippet；非 Kimi 或路由关闭时保持 `alpha/search`。

### Modified Capabilities

（无独立旧 web_search capability 文件；行为由本变更 spec 完整规定。）

## Impact

- 受影响 crate / 区域：
  - `codex-rs/http-client` — 有界流式 response/error-body 读取原语
  - `codex-rs/codex-api` — Moonshot search HTTP 客户端（新模块）
  - `codex-rs/model-provider-info` — 共享 Kimi slug 判定
  - `codex-rs/web-search`（新 crate）— 命令规范化、后端选择、有界格式化与共享执行
  - `codex-rs/core` — `WebSearchHandler` 适配、事件与集成测试；不新增共享公共 API
  - `codex-rs/ext/web-search` — Kimi 可见性、共享执行适配（standalone 路径）
  - `codex-rs/features` — `kimi_moonshot_web_search` feature
  - `codex-rs/config` / `core/config.schema.json` — `[moonshot_search]` 段
  - `codex-rs/app-server` — standalone 结构化结果回归与允许的 API 说明
- 明确复用：
  - 工具名 `web_search` / `web.run`、WebSearch 事件、权限面
  - 现有 Claude Kimi 的 `LocalFunctionTool` 广告路径
- 范围外：
  - 满血 `SearchCommands`、刮页库、`$web_search` builtin_function
  - 新 FetchURL 工具、kimi-code OAuth 栈
  - 非 Kimi 主模型默认改后端
- 主要风险：
  - slug 前缀误判 / 网关 path 不一致 → 显式 `base_url` + 测试矩阵
  - standalone `web.run` 漏改 → 共享执行 + 集成断言
  - 跨域显式 URL 泄露 provider token → 同源校验 + 独立凭据 + 请求未发生断言
  - 外部响应放大模型上下文 → body / 条目 / 字段 / 8K-token 四层硬上限
  - 仅 snippet → description 引导；fetch 另案
- 回滚：
  - `features.kimi_moonshot_web_search = false` 或
    `moonshot_search.enabled = false` → Kimi 恢复 `alpha/search`
- 验证命令（实现阶段）：
  - `cd codex-rs && just write-config-schema`
  - 新 crate / 依赖变更：补 `BUILD.bazel` 并在仓库根目录运行
    `just bazel-lock-update`
  - `cd codex-rs && just test -p codex-api`
  - `cd codex-rs && just test -p codex-http-client`
  - `cd codex-rs && just test -p codex-model-provider-info`
  - `cd codex-rs && just test -p codex-web-search`
  - `cd codex-rs && just test -p codex-web-search-extension`
  - `cd codex-rs && just test -p codex-features`
  - `cd codex-rs && just test -p codex-config`
  - `cd codex-rs && just test -p codex-core`
  - `cd codex-rs && just test -p codex-app-server`
  - core/common/protocol 变更后的全量 `cd codex-rs && just test` 按
    AGENTS.md 另行征求同意
  - 定向测试结束后，对上述实际变更 crate 分别运行 scoped `just fix`，
    最后运行 `cd codex-rs && just fmt`；按 AGENTS.md 不再重跑测试
- 外部参考：
  - `/Users/mt/code/mt-ai/aicodex/kimi-code` Moonshot WebSearch 实现与配置文档

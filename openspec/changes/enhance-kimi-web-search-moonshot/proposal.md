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

> **审核门禁**：实现前须确认 `design.md`「审核决议」并勾选 `tasks.md` §0。
> 提案文档已经过一轮内部审核修订，开放问题已关闭为锁定决议。

## What Changes

- 引入 **Moonshot 简单搜索后端**（对齐 kimi-code）：
  - `POST` 完整 search URL（显式配置或由 provider `base_url` 派生 `{base}/search`）
  - JSON body：`{ "text_query": "<query>" }`
  - 鉴权：`Authorization: Bearer <token>`（`[moonshot_search].api_key` 或当前主
    provider auth；Phase A **不含** OAuth refresh 栈）
  - 有 call_id 时 MUST 发送 `X-Msh-Tool-Call-Id`
  - 解析 `search_results[]` → title / url / snippet（可选 date / site_name）；
    禁止注入响应全文 `content`
  - 格式化为模型可读 tool 文本（含引用提醒）
- **后端路由**（`WebSearchHandler` + `ext/web-search` 共享）：
  - Feature `kimi_moonshot_web_search`（默认 true）且 `moonshot_search.enabled`
    （默认 true）且主模型 slug 命中 Kimi 启发式 → Moonshot
  - 否则 → 现有 OpenAI `alpha/search`
  - **禁止** OpenAI 失败后的自动降级
- Kimi 路径 `SearchCommands` **严格子集**（见 design R5）：
  - 支持 query / queries / `search_query[].q`（最多 4，顺序执行）
  - `recency` / `domains` 忽略并注明
  - 任一富命令非空 → **整次调用** unsupported（即使同时带 query）
- 配置：根段 `[moonshot_search]`（`enabled` / `base_url` / `api_key` /
  `custom_headers`）；**不**复用 `[tools.web_search]`（该段服务 OpenAI hosted 语义）
- 工具广告：继续 `web_search` / 可选 `web.run`；补充 description「摘要 + 需读页再 fetch」
- 测试：mock Moonshot、Kimi 不打 `alpha/search`、非 Kimi 与 feature-off 回归
- 文档与 schema：`docs/config.md` + `just write-config-schema`

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
  - `codex-rs/codex-api` — Moonshot search HTTP 客户端（新模块）
  - `codex-rs/core` — Kimi 判定、共享执行/路由、`WebSearchHandler`、集成测试
  - `codex-rs/ext/web-search` — 复用共享执行（standalone 路径）
  - `codex-rs/features` — `kimi_moonshot_web_search` feature
  - `codex-rs/config` / `docs/config.md` / `core/config.schema.json` —
    `[moonshot_search]` 段
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
  - 仅 snippet → description 引导；fetch 另案
- 回滚：
  - `features.kimi_moonshot_web_search = false` 或
    `moonshot_search.enabled = false` → Kimi 恢复 `alpha/search`
- 验证命令（实现阶段）：
  - `cd codex-rs && just fmt`
  - `cd codex-rs && just test -p codex-api`
  - `cd codex-rs && just test -p codex-core`（Kimi web_search 相关；全量 suite
    按 AGENTS.md 先征求同意）
  - `cd codex-rs && just write-config-schema`（若 ConfigToml 变更）
  - 依赖变更时：仓库根目录 `just bazel-lock-update`
- 外部参考：
  - `/Users/mt/code/mt-ai/aicodex/kimi-code` Moonshot WebSearch 实现与配置文档

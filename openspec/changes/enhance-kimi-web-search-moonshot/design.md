## Context

### 现状（aicodex / Codex）

| 层 | 行为 |
| --- | --- |
| 工具广告 | Kimi K3 / Compatible：`web_search` 为 **LocalFunctionTool**（非 Anthropic native server tool） |
| 工具执行 | `WebSearchHandler` **始终** `SearchClient` → `POST …/alpha/search`（OpenAI） |
| 非 OpenAI 主模型 | search model fallback 为 `gpt-5.2-codex`，仍需 OpenAI provider 鉴权 |
| Standalone | Feature `standalone_web_search`（默认 off）启用时广告 `web.run`，隐藏 plain `web_search`，执行仍打 `alpha/search` |
| 配置 | 已有 `[tools.web_search]`（context_size / domains / location，服务 OpenAI hosted 语义）；**无** Moonshot `/search` 段 |

纯 Kimi 环境下查网弱/不可用，与官方 kimi-code 不一致。

### 参考（kimi-code，行为源）

| 项 | kimi-code |
| --- | --- |
| 工具名 | `WebSearch`（本地 builtin） |
| API | `POST {baseUrl}`（已是完整 search URL），body `{ text_query }` |
| 鉴权 | Bearer（API key 或 OAuth access token） |
| 结果 | `search_results[]` → title, url, snippet（可选 date, site_name）；**丢弃**全文 `content` |
| 注册条件 | 未配置 provider 时工具不出现 |
| 读全文 | 另工具 `FetchURL` |
| 配置 | `[services.moonshot_search]` 优先；否则 managed Kimi provider 派生 `{base}/search` |

本设计对齐上述**简单搜索契约**，落在 Codex 既有 `web_search` / `web.run` 工具身份上。

## Goals / Non-Goals

**Goals:**

- Kimi 主模型会话下，本地查网可在无 OpenAI 鉴权时返回摘要结果
- HTTP 契约与 kimi-code MoonshotWebSearchProvider 对齐（可 mock 对照）
- OpenAI 主模型路径行为不变（仍 `alpha/search`）
- 富 `SearchCommands` 在 Kimi 路径有明确失败语义
- 配置可显式覆盖，亦可从当前 Kimi provider 推导
- 与 wire 无关：Claude Messages 与未来 Chat Completions 只要走同一 tool handler，均适用

**Non-Goals:**

- 本期不实现 open/click/find/screenshot/finance/weather/sports/time
- 不引入第三方多源刮页库作为默认
- 不实现 chat `builtin_function` / `$web_search`
- 本期不新做独立 FetchURL 工具（另开增量提案）
- 不把 Moonshot 搜索做成 Anthropic native `web_search_20250305`
- 本期不移植 kimi-code OAuth / managed login 栈

## 审核决议

以下条目关闭 README 原「待确认问题」，实现必须遵守。

### R1. Kimi 判定（slug 启发式，共享函数）

新增/抽取共享判定（建议放 `codex-core` 可测模块，供 handler 与测试复用），归一化规则：

1. 取 slug 最后一个 `:` 之后的片段（兼容 `gateway:k3`）并 `to_ascii_lowercase`
2. 命中任一即视为 Kimi：
   - 精确 `k3`（现有 `claude_model_is_kimi_k3`）
   - 精确 `kimi-k2.7-code` / `kimi-k2.7-code-highspeed`（现有 helper）
   - 或以 `kimi-` 为前缀（对齐 app-server `model.starts_with("kimi-")`）

**不**使用：`ClaudeProviderCompat::Compatible` 全体、base_url 子串猜测（易误伤 DeepSeek 等）。

Provider id 仅作可选增强：若未来 model_providers 增加显式 `kimi` 类型字段，可 OR 进判定；Phase A **不依赖**该字段。

### R2. 鉴权范围（Phase A）

仅 Bearer：

1. `[moonshot_search].api_key`（若非空）
2. 否则：当前**会话主 provider** 经现有 `auth_manager` / API auth 解析出的 bearer token（与对该 provider 发推理请求相同的凭据源）

**不做** kimi-code `IOAuthService.resolveTokenProvider` / refresh 流程。  
若用户只有 ChatGPT/OpenAI 登录而主模型是 Kimi 且无 `moonshot_search.api_key` / 无 Kimi provider key → 执行期可纠正错误。

### R3. 配置键

新增根配置段（非 kimi-code 的 `services.*`，因 Codex `ConfigToml` 无 `services`）：

```toml
[moonshot_search]
# 缺省 true。false 时即使主模型为 Kimi 也退回 OpenAI alpha/search（回滚/对比用）
enabled = true
# 完整 search URL，例如 https://api.moonshot.cn/v1/search
base_url = "https://api.moonshot.cn/v1/search"
api_key = "sk-..."
# 可选
# custom_headers = { "X-Extra" = "..." }
```

优先级：

1. 若 `enabled = false` → Kimi 也走 OpenAI `alpha/search`（显式回滚）
2. 否则若配置了非空 `base_url` → 用该 URL；token 按 R2
3. 否则从**当前主 provider** 的 `base_url` 去尾 `/` 后追加 `/search`；token 按 R2
4. URL 或 token 仍缺失 → 可纠正错误，**禁止**静默改打 OpenAI

**不**把 Moonshot 凭据塞进现有 `[tools.web_search]`（该结构服务 OpenAI hosted filters/location）。

### R4. 路由范围

- **仅**主模型判定为 Kimi 时选 `MoonshotSimpleSearch`
- **禁止**「OpenAI `alpha/search` 失败后再降级 Moonshot」
- **禁止**非 Kimi Compatible 默认走 Moonshot

### R5. SearchCommands 子集（严格）

| 输入 | 行为 |
| --- | --- |
| 仅 `query` / `queries` / `search_query[].q`（可带被忽略的 recency/domains） | 执行 Moonshot 搜；每 query 一次 POST；**最多 4** 条；**顺序**执行（Phase A） |
| `search_query[].recency` / `domains` | **忽略**，并在 tool 结果中简短注明未应用 |
| 任一富命令键存在且载荷非空：`open` / `click` / `find` / `screenshot` / `image_query` / `finance` / `weather` / `sports` / `time` | **整次调用**失败（RespondToModel），即使同时带了 query；文案引导 query-only + 可用读页工具 |
| 同时存在可执行 query 与富命令 | **仍整次失败**（避免半执行） |
| 空 query / 无任何可执行 q | 校验失败 |

`response_length`：若单独出现且无富命令，**忽略**（Moonshot 简单搜无对应语义）。

### R6. `web.run`（standalone）策略

当 Feature `standalone_web_search` 开启且会话广告 `web.run` 时：

- 若主模型为 Kimi 且 Moonshot 路由启用（R3/R4）→ **`web.run` 必须走同一 Moonshot 后端与 R5 子集**
- 实现上抽取共享「解析 commands + 选 backend + 执行」逻辑，供 `WebSearchHandler` 与 `ext/web-search` 复用，避免双路径分叉

Kimi 路径**不**在本期强制关闭 standalone；以共享后端为准。

### R7. Feature 门控

- 新增 Feature：`kimi_moonshot_web_search`（key 同名），**default_enabled: true**
- 关闭该 feature 时：Kimi 恢复今日 OpenAI `alpha/search` 行为（与 `moonshot_search.enabled=false` 同效于路由层）
- 两者任一关闭 → OpenAI 路径；两者都开且为 Kimi → Moonshot 路径

### R8. 读页缺口

本期只交付搜索腿。工具 description MUST 说明结果为摘要。  
FetchURL / Moonshot fetch 另开 `kimi-moonshot-web-fetch`（或等价）提案。

## Decisions（实现细节）

### 1. 后端枚举

```text
enum WebSearchBackendKind {
  OpenAiAlphaSearch,
  MoonshotSimpleSearch,
}
```

选择伪代码：

```text
if !features.kimi_moonshot_web_search || moonshot_search.enabled == false:
    OpenAiAlphaSearch
else if is_kimi_model(primary_slug):
    MoonshotSimpleSearch
else:
    OpenAiAlphaSearch
```

### 2. 工具身份

保持 `web_search` / `web.run`；不新增 `WebSearch` PascalCase 工具。

### 3. Moonshot HTTP 契约（锁定）

```http
POST {search_base_url}
Authorization: Bearer <token>
Content-Type: application/json
X-Msh-Tool-Call-Id: <call_id>   # 有 call_id 则 MUST 发送
```

```json
{ "text_query": "rust async" }
```

映射：title/url/snippet 必选呈现（缺省空串）；date/site_name 可选；**禁止**把响应里的全文 `content` 注入模型上下文。

错误：401 / 非 200 / 传输错误 → 可纠正或明确失败文案；空 `search_results` → 非错误说明文案。

### 4. 模块边界

| 组件 | 位置 |
| --- | --- |
| HTTP client + DTO | `codex-rs/codex-api` 新模块（如 `endpoint/moonshot_search.rs`） |
| 共享执行/路由 | `codex-rs/core` 抽出小模块（避免继续膨胀单文件 `web_search.rs`） |
| `ext/web-search` | 调用共享执行，不复制 HTTP |
| 配置 | `ConfigToml` + `moonshot_search` 段；`just write-config-schema` |

禁止把 Moonshot JSON 解析塞进 Claude/Responses SSE parser。

### 5. 测试策略

- 单元：判定矩阵、URL 派生、请求形状、映射、富命令整次拒绝、recency 忽略注明、401
- 集成：Kimi slug → mock Moonshot，**断言未**打 `/v1/alpha/search`；非 Kimi → 仍打 `alpha/search`；feature/enabled 关闭 → Kimi 也打 `alpha/search`
- CI 不强制 live Moonshot

## Risks / Trade-offs

| 风险 | 缓解 |
| --- | --- |
| 网关 path 不是 `{base}/search` | 显式 `moonshot_search.base_url` |
| slug 过宽（`kimi-foo` 假阳性） | 与 app-server 前缀对齐；文档说明；可后续收紧白名单 |
| standalone `web.run` 仍走 OpenAI | R6 强制共享后端 + 集成测试 |
| 无 fetch 时答不深 | description 引导；Phase C 提案 |
| 默认 feature true 行为变化 | `enabled=false` / feature off 快速回滚 |

## 分期

### Phase A — 必须交付

- Moonshot client、Kimi 判定、路由、R5 子集、`[moonshot_search]`、feature、handler+ext 共享执行、mock 测试、config schema + `docs/config.md`

### Phase B — 体验（可同 PR）

- Kimi 路径更瘦的广告 schema（query 为主）减少富命令误用
- description / 系统指引强化

### Phase C — 后续提案

- Moonshot/本地 FetchURL
- 可选第二简单搜 provider（仍保持 text_query 契约）

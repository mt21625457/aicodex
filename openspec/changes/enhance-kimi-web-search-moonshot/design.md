## Context

### 现状（aicodex / Codex）

| 层 | 行为 |
| --- | --- |
| 工具广告 | Kimi K3 / Compatible：`web_search` 为 **LocalFunctionTool**（非 Anthropic native server tool） |
| 工具执行 | `WebSearchHandler` **始终** `SearchClient` → `POST …/alpha/search`（OpenAI） |
| 非 OpenAI 主模型 | search model fallback 为 `gpt-5.2-codex`，仍需 OpenAI provider 鉴权 |
| Standalone | Feature `standalone_web_search`（默认 off）仅在扩展 executor 可用时广告 `web.run`；当前扩展只放行 OpenAI / actor-auth provider，普通 Kimi provider 不会注册该工具；已注册时执行打 `alpha/search` |
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

以下条目关闭两轮审核发现的问题，实现必须遵守。R1–R13 均属于人工实现门禁。

### R1. Kimi 判定（slug 启发式，共享函数）

在 `codex-model-provider-info` 新增共享、可测的 Kimi slug 判定，供 core、
web-search extension、共享 web-search crate 与需要相同语义的调用方复用；不得为此
扩大 `codex-core` 公共 API。归一化规则：

1. 取 slug 最后一个 `:` 之后的片段（兼容 `gateway:k3`）并 `to_ascii_lowercase`
2. 命中任一即视为 Kimi：
   - 精确 `k3`（现有 `claude_model_is_kimi_k3`）
   - 精确 `kimi-k2.7-code` / `kimi-k2.7-code-highspeed`（现有 helper）
   - 或以 `kimi-` 为前缀（对齐 app-server `model.starts_with("kimi-")`）

**不**使用：`ClaudeProviderCompat::Compatible` 全体、base_url 子串猜测（易误伤 DeepSeek 等）。

Provider id 仅作可选增强：若未来 model_providers 增加显式 `kimi` 类型字段，可 OR 进判定；Phase A **不依赖**该字段。

### R2. 鉴权范围（Phase A）

仅 Bearer，按以下顺序解析：

1. `[moonshot_search].env_key` 指向的非空环境变量
2. `[moonshot_search].api_key`（若非空；仅为程序化/兼容用途，配置文档必须提醒明文 secret 风险）
3. 否则仅当 search URL 是从当前 provider URL 派生，或显式 URL 与当前 provider
   runtime base URL **同源**时，才可使用当前会话主 provider 的 bearer token
   （与对该 provider 发推理请求相同的凭据源）

**不做** kimi-code `IOAuthService.resolveTokenProvider` / refresh 流程。  
若显式 search URL 与主 provider 不同源且未配置独立 Moonshot 凭据，或用户只有
ChatGPT/OpenAI 登录而主模型是 Kimi 且无 Moonshot / Kimi provider key → 执行期
返回可纠正错误，且请求不得发出。

同源按 scheme、host、effective port 比较。URL MUST 是绝对 `http` / `https` URL，
MUST NOT 含 userinfo 或 fragment。允许 `http` 以支持 localhost 和显式私有网关；
安全责任由显式配置承担。

“复用 provider auth”只表示从相同的 provider-scoped credential source 解析出原始
Bearer token，再构造 Moonshot 专用 `Authorization` header；MUST NOT 直接复用
Claude `api_auth()` 产生的 `x-api-key`、`anthropic-version` 或任意 provider header
集合。无法安全归约为单一 Bearer token 的 Headers/AWS 等 auth 形态视为不可用。

### R3. 配置键

新增根配置段（非 kimi-code 的 `services.*`，因 Codex `ConfigToml` 无 `services`）：

```toml
[moonshot_search]
# 缺省 true。false 时即使主模型为 Kimi 也退回 OpenAI alpha/search（回滚/对比用）
enabled = true
# 完整 search URL，例如 https://api.moonshot.cn/v1/search
base_url = "https://api.moonshot.cn/v1/search"
# 推荐：从环境变量读取，避免把 secret 写入 config.toml
env_key = "MOONSHOT_API_KEY"
# 仅用于程序化/兼容场景；与 env_key 同时存在时 env_key 优先
api_key = "sk-..."
# 可选
# custom_headers = { "X-Extra" = "..." }
```

优先级：

1. 若 `enabled = false` → Kimi 也走 OpenAI `alpha/search`（显式回滚）
2. 否则若配置了非空 `base_url` → 校验并使用该完整 URL；token 按 R2，跨域时
   必须有独立 search 凭据
3. 否则从**当前主 provider** 的 runtime `base_url` 去尾 `/` 后追加 `/search`；token 按 R2
4. URL 或 token 仍缺失、URL 非法或跨域凭据不满足 → 可纠正错误，请求不得发出，
   **禁止**静默改打 OpenAI

**不**把 Moonshot 凭据塞进现有 `[tools.web_search]`（该结构服务 OpenAI hosted filters/location）。

`custom_headers` 不得包含（大小写不敏感）`Authorization`、`Content-Type` 或
`X-Msh-Tool-Call-Id`；发现保留 header 时配置校验失败，避免覆盖鉴权、内容类型和
调用关联语义。

### R4. 路由范围

- **仅**主模型判定为 Kimi 时选 `MoonshotSimpleSearch`
- **禁止**「OpenAI `alpha/search` 失败后再降级 Moonshot」
- **禁止**非 Kimi Compatible 默认走 Moonshot

### R5. SearchCommands 子集（严格）

| 输入 | 行为 |
| --- | --- |
| 仅 `query` / `queries` / `search_query[].q`（可带被忽略的 recency/domains） | 按 `query` → `queries` → `search_query` 顺序合并，trim 后按首次出现去重；每 query 一次 POST；**最多 4** 条；**顺序**执行（Phase A） |
| `search_query[].recency` / `domains` | **忽略**，并在 tool 结果中简短注明未应用 |
| 任一富命令键存在且载荷非空：`open` / `click` / `find` / `screenshot` / `image_query` / `finance` / `weather` / `sports` / `time` | **整次调用**失败（RespondToModel），即使同时带了 query；文案引导 query-only + 可用读页工具 |
| 同时存在可执行 query 与富命令 | **仍整次失败**（避免半执行） |
| 规范化后超过 4 条唯一 query | **整次调用**校验失败，禁止截断后部分执行 |
| 空 query / 无任何可执行 q | 校验失败 |
| 未知顶层字段 | 无论载荷是否为空均校验失败，防止未来命令被 serde 静默忽略 |

仅允许顶层键 `query`、`queries`、`search_query`、`response_length` 与上述已知富命令。
富命令为空数组 / null 时可忽略；非空即整次拒绝。`response_length`：若单独出现且
无富命令，**忽略**并在结果中注明（Moonshot 简单搜无对应语义）。解析 MUST 先检查
原始 JSON 键，再转 typed DTO，plain `web_search` 与 `web.run` 使用同一规范化器。

### R6. `web.run`（standalone）策略

当 Feature `standalone_web_search` 开启且会话广告 `web.run` 时：

- 若主模型为 Kimi 且 Moonshot 路由启用（R3/R4）→ **`web.run` 必须走同一 Moonshot 后端与 R5 子集**
- Kimi 主模型且 web-search mode 未关闭时，extension availability MUST 显式放行
  `web.run`，不能继续受当前 “OpenAI / actor-auth only” 条件阻断
- 实现上在独立 `codex-web-search` crate 抽取共享「解析 commands + 选 backend +
  执行 + 有界格式化」逻辑，供 `WebSearchHandler` 与 `ext/web-search` 复用，避免双路径分叉
- kill switch 令 Kimi 回到 OpenAI 时，共享执行器 MUST 使用与 plain handler 相同的
  configured OpenAI search provider / fallback model，不得误用 Kimi provider 调 `alpha/search`

Kimi 路径**不**在本期强制关闭 standalone；以共享后端为准。

### R7. Feature 门控

- 新增 Feature：`kimi_moonshot_web_search`（key 同名），**default_enabled: true**
- 关闭该 feature 时：Kimi 恢复今日 OpenAI `alpha/search` 行为（与 `moonshot_search.enabled=false` 同效于路由层）
- 两者任一关闭 → OpenAI 路径；两者都开且为 Kimi → Moonshot 路径

### R8. 读页缺口

本期只交付搜索腿。plain `web_search` 与 `web.run` 的工具 description MUST 在 Phase A
说明结果为摘要、引用应使用 URL、需要全文时应使用已有可用读页能力。
FetchURL / Moonshot fetch 另开 `kimi-moonshot-web-fetch`（或等价）提案。

### R9. 外部响应与模型上下文硬上限

Moonshot 返回属于不受信任的外部输入，必须在四层设硬上限：

1. HTTP response body：解压后的 body 最多 **1 MiB**；Moonshot client 必须通过流式
   响应逐 chunk 累加并在越界时立即停止，不得走先完整缓冲再截断的 `execute()`
   路径；非 2xx error body 的模型/日志诊断最多保留 **16 KiB**
2. `search_results`：最多取前 **8** 条，并在输出中注明其余条目被省略
3. 单字段 Unicode-scalar 上限：title 512、url 2048、snippet 2048、site_name 256、
   date 128；安全截断并注明发生过字段截断
4. 最终 function tool 文本：使用现有 tool-output truncation 路径，并施加
   `min(turn_budget, 8_000 tokens)` 的硬上限；任何调用均不得向模型注入超过 8K tokens

响应中的 `content`、未知大字段在 DTO 映射阶段直接丢弃。测试必须覆盖超大 body、
超多结果、超长字段和最终输出上限，而不只覆盖正常响应。

### R10. 凭据与 header 隔离

R2/R3 的同源规则是安全边界，不是建议。显式跨域 `base_url` 未提供独立 search
凭据时，必须在构造请求前失败并断言 mock endpoint 未收到请求。固定的 auth、content
type 和 call-id header 不允许被 `custom_headers` 覆盖，日志与错误不得包含 token 或
完整自定义 header 值。

### R11. 模块边界

- `codex-http-client`：提供/修正可复用的有界流式 response 与非 2xx error-body 读取；
  默认行为变更需有回归测试
- `codex-api`：Moonshot HTTP client + wire DTO，使用有界流式读取；不依赖 core
- `codex-model-provider-info`：共享 Kimi slug 判定
- 新 `codex-web-search`：命令规范化、后端选择、跨域凭据策略、共享执行和有界格式化；
  不依赖 `codex-core`
- `codex-core` 与 `ext/web-search`：各自适配 session/config/event/ToolOutput，不复制
  parser 或 HTTP 实现

新增 crate 必须接入 Cargo workspace、Bazel `BUILD.bazel` 与锁文件更新。禁止为共享
执行向 `codex-core` 新增公共 API。

### R12. 事件与外部上下文语义

- 两个 backend 均保持现有 WebSearch begin/end 生命周期；失败也必须有成对、可解释
  的结束语义，不能只发 begin
- standalone Moonshot end event 的 `results` 使用有界结构化 DTO：
  `{"type":"text_result","title":...,"url":...,"snippet":...}`，可选键为
  `date` / `site_name`；不得携带 `content`，也不得伪造无法用于 R5 open 的 `ref_id`
- plain 与 standalone 的 tool output 均 MUST `contains_external_context = true`，避免
  搜索内容污染记忆生成
- 若 app-server 的 `results` 说明发生变化，更新允许的 `codex-rs/app-server/README.md`
  并增加 public JSON-RPC 回归测试

### R13. 文档与验证落点

根 `AGENTS.md` 禁止向 `docs/` 新增通用产品/用户文档，因此本变更不修改
`docs/config.md`。配置字段说明必须进入 Rust schema docstrings、生成的
`core/config.schema.json` 与本 OpenSpec；官方用户文档更新作为仓库外发布跟进。
app-server API 说明仍按 R12 的例外规则维护。

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
| 有界 HTTP body 读取 | `codex-rs/http-client` 流式 helper / transport 行为 |
| HTTP client + DTO | `codex-rs/codex-api` 新模块（如 `endpoint/moonshot_search.rs`） |
| slug 判定 | `codex-rs/model-provider-info` 共享 helper |
| 共享执行/路由 | 新 `codex-rs/web-search` crate；不得依赖或扩张 `codex-core` |
| core / `ext/web-search` | 调用共享执行，各自保留事件与 ToolOutput 适配，不复制 HTTP/parser |
| 配置 | `ConfigToml` + `moonshot_search` 段；`just write-config-schema` |

禁止把 Moonshot JSON 解析塞进 Claude/Responses SSE parser。

### 5. 测试策略

- 单元：有界 success/error body 流式读取、判定矩阵、URL 派生/同源、跨域凭据
  拒绝、保留 header 拒绝、请求形状、映射与全部硬上限、未知命令/富命令整次
  拒绝、>4 query 拒绝、filter note、401
- 集成：Kimi slug → mock Moonshot，**断言未**打 `/v1/alpha/search`；Kimi standalone
  可见且走同一后端；非 Kimi → 仍打 `alpha/search`；feature/enabled 关闭 → Kimi 也打
  configured OpenAI `alpha/search`；所有 output 保持 external-context 标记
- app-server：standalone Moonshot end event 的有界结构化 results 通过 public JSON-RPC
  验证（若该事件面受影响）
- CI 不强制 live Moonshot

## Risks / Trade-offs

| 风险 | 缓解 |
| --- | --- |
| 网关 path 不是 `{base}/search` | 显式 `moonshot_search.base_url` |
| slug 过宽（`kimi-foo` 假阳性） | 与 app-server 前缀对齐；文档说明；可后续收紧白名单 |
| standalone `web.run` 仍走 OpenAI | R6 强制共享后端 + 集成测试 |
| 普通 Kimi provider 根本不注册 `web.run` | R6 显式扩展 availability + 工具可见性测试 |
| 跨域 URL 泄露主 provider token | R2/R3/R10 同源边界 + 请求未发生测试 |
| 超大响应污染上下文或耗尽内存 | R9 body/result/field/token 四层硬上限 |
| 共享实现继续膨胀 `codex-core` | R11 独立 crate + 依赖方向约束 |
| 无 fetch 时答不深 | description 引导；Phase C 提案 |
| 默认 feature true 行为变化 | `enabled=false` / feature off 快速回滚 |

## 分期

### Phase A — 必须交付

- Moonshot client、共享 Kimi 判定、独立 shared crate、路由、R5 子集、R9/R10 安全边界、
  `[moonshot_search]`、feature、handler+ext 共享执行、Kimi `web.run` availability、事件与
  external-context 语义、mock/集成测试、config schema

### Phase B — 体验（可同 PR）

- Kimi 路径更瘦的广告 schema（query 为主）减少富命令误用
- 进一步的系统指引强化（Phase A 已包含最低限度 description）

### Phase C — 后续提案

- Moonshot/本地 FetchURL
- 可选第二简单搜 provider（仍保持 text_query 契约）

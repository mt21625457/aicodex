## Context

This change is intentionally narrow. It does not add MCP dynamic access,
does not change hooks, and does not expand Claude native tools beyond the
existing web-search surface. The current Claude adapter already has working
request declarations, side-table mapping, SSE reconstruction, execution, and
history replay for ordinary function tools, namespace/dynamic MCP tools,
freeform/custom tools, and client-side `tool_search`.

Current ownership remains:

- `codex-rs/tools/src/tool_spec.rs` owns Claude tool declaration
  serialization, Claude-safe names, native/fallback decisions, and
  `ClaudeToolCallInfo`.
- `codex-rs/core/src/claude.rs` owns prompt/history conversion into Claude
  Messages requests, provider compatibility selection, provider-state replay,
  and prompt cache placement.
- `codex-rs/codex-api/src/sse/claude.rs` owns Claude SSE accumulation and
  conversion back into `ResponseEvent` / `ResponseItem`.
- `codex-rs/core/src/tools/handlers/web_search.rs` owns handler-backed local
  web search.

## Goals / Non-Goals

**Goals:**

- Make the WebSearch Claude plan explicit: native, local fallback, or disabled
  unsupported/degraded.
- Keep native Claude web search for live specs that only use fields supported
  by `web_search_20250305`.
- Map native-supported fields (`allowed_domains`, `user_location`) without
  changing provider-state replay.
- Avoid silently dropping Responses-only semantics:
  `external_web_access = false`, `search_context_size`, and non-text
  `search_content_types`.
- Add targeted tests for serializer decisions, request construction, provider
  state replay, and local handler behavior.

**Non-Goals:**

- Do not enhance or redesign MCP dynamic access.
- Do not enhance hooks.
- Do not change ordinary function, namespace, freeform/custom, or client-side
  `tool_search` routing.
- Do not add Claude native `web_fetch`, `code_execution`, `advisor`, `memory`,
  `computer`, native remote MCP, or native tool search.
- Do not add user-facing configuration in this change.

## Decisions

### 1. Existing non-web-search tool families are baseline

The proposal records the audit conclusion, but implementation should not add
new behavior to already-complete tool families. Regression coverage may be
added only when it directly protects the WebSearch policy boundary or catches
an accidental advertised-without-handler state.

### 2. Native Claude web search is allowed only when lossless enough

The selected native tool is `web_search_20250305`. It can represent:

- live provider-owned web search;
- `filters.allowed_domains`;
- `user_location`.

It cannot represent:

- cached-only `external_web_access = false`;
- OpenAI `search_context_size`;
- image or other non-text `search_content_types`.

Therefore native is selected only when:

- `external_web_access` is `None` or `Some(true)`;
- `search_context_size` is absent;
- `search_content_types` is absent or equivalent to text-only.

Native selection keeps `preserve_server_tool_results` and
`preserve_structured_citations` enabled so `server_tool_use`,
`web_search_tool_result`, and citation blocks remain replayable.

### 3. Local fallback is handler-backed and explicit

When native would drop a supported local semantic, Codex exposes `web_search`
as a Claude function tool and maps it back to normal `FunctionCall` execution.
The local handler owns the original `ToolSpec::WebSearch`, so it can preserve
allowed-domain filtering and use `search_context_size` to tune local result
budgeting.

When the original spec asks for semantics the local text-search handler cannot
honor safely, such as cached-only search or non-text result types, the handler
returns a model-correctable error before running any search side effect.

### 4. Unsupported/degraded means visible policy, not silent omission

The existing `ClaudeNativeToolPolicy` is the diagnostics surface. The planner
records:

- `Enabled` when native web search is selected;
- `Fallback` when local handler-backed search is selected;
- `Disabled` when no safe representation exists for the requested semantic.

Tests should assert the policy outcome and the model-facing tool declaration,
not incidental tool ordering.

### 5. DeepSeek-compatible providers keep the existing fallback path

For `ClaudeProviderCompat::DeepSeek`, the request builder continues selecting
the local function fallback path. This change does not add a DeepSeek native
server-tool path or provider-specific web-search semantics.

### 6. Stream parsing remains provider-owned for native web search

Native `server_tool_use name = "web_search"` continues to become typed
`ResponseItem::WebSearchCall`, and the provider-owned block is preserved as
provider state. Native `web_search_tool_result` and structured citations remain
provider state for follow-up Claude requests only when the active plan still
requires server-tool/citation replay.

Local fallback is different: it is a normal Codex function call and replays as
`FunctionCallOutput` `tool_result`.

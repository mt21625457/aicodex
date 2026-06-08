## ADDED Requirements

### Requirement: Claude WebSearch planning must be lossless or explicit

Codex MUST NOT silently drop `ToolSpec::WebSearch` semantics when selecting a
Claude web-search representation. For Claude Messages, Codex MUST choose
native server web search only when the active spec is represented by the
selected Claude native tool. Otherwise Codex MUST select a handler-backed local
fallback or record an explicit disabled/unsupported decision.

#### Scenario: Native live web search maps supported fields

- **WHEN** the active Claude provider supports native web search
- **AND** `ToolSpec::WebSearch.external_web_access` is absent or `true`
- **AND** `search_context_size` is absent
- **AND** `search_content_types` is absent or text-only
- **THEN** Codex may emit native Claude `web_search_20250305`
- **AND** `filters.allowed_domains` is mapped when present
- **AND** `user_location` is mapped when present
- **AND** provider-owned server web-search results and structured citations are
  preserved for replay
- **AND** the planning decision records native web search as enabled

#### Scenario: Native web search is not used for unsupported Responses-only semantics

- **WHEN** the active `ToolSpec::WebSearch` includes cached-only
  `external_web_access = false`, `search_context_size`, or non-text
  `search_content_types`
- **THEN** Codex MUST NOT emit native Claude web search while silently dropping
  those fields
- **AND** Codex MUST record local fallback or disabled/unsupported policy for
  `ClaudeNativeToolKind::WebSearch20250305`

#### Scenario: Local web-search fallback remains handler backed

- **WHEN** Claude web search is planned as local fallback
- **THEN** Codex advertises `web_search` as a Claude function tool
- **AND** reverse metadata maps it to a normal `FunctionCall`
- **AND** the call routes through `WebSearchHandler`
- **AND** the result replays as a normal `FunctionCallOutput` `tool_result`

#### Scenario: Unsupported local semantics fail before search side effects

- **WHEN** local fallback receives a spec that asks for cached-only search or
  non-text search results
- **THEN** `WebSearchHandler` returns a model-correctable error
- **AND** it does not perform a network search
- **AND** Codex does not synthesize unsupported results

#### Scenario: Native web-search stream remains provider owned

- **WHEN** Claude native web search emits `server_tool_use` with
  `name = "web_search"`
- **THEN** Codex emits a typed `ResponseItem::WebSearchCall`
- **AND** Codex preserves the provider-owned server-tool state for follow-up
  Claude requests when the active plan requires it
- **AND** Codex does not execute the local `WebSearchHandler` for that native
  server-tool call

#### Scenario: DeepSeek-compatible providers keep local fallback

- **WHEN** the active provider uses DeepSeek-compatible Claude behavior
- **THEN** Codex uses the local function fallback for `web_search`
- **AND** the request does not include an Anthropic native server web-search
  declaration

### Requirement: Existing Claude tool-family behavior must remain stable

Codex MUST preserve the audited existing Claude tool behavior for ordinary
function tools, namespace/dynamic MCP tools, freeform/custom tools, and
client-side `tool_search` while completing WebSearch semantics.

#### Scenario: Non-WebSearch tool families are not redesigned

- **WHEN** this change is implemented
- **THEN** ordinary function, namespace/dynamic MCP, freeform/custom, and
  client-side `tool_search` request/stream/history/execution behavior remains
  compatible with the existing Claude adapter
- **AND** MCP dynamic access and hooks are not modified

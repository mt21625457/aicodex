## Why

The Claude Messages adapter already routes the existing Codex tool families:
ordinary function tools, flattened namespace/dynamic MCP tools,
freeform/custom tools, client-side `tool_search`, and basic web search. The
audit found no need to redesign MCP dynamic access, hooks, or the normal tool
loop.

The remaining completeness gap is narrower: `ToolSpec::WebSearch` carries
Responses semantics that the current Claude native `web_search_20250305`
declaration cannot always express. Claude native web search can represent
domain filters and user location, and it is a live provider-owned server tool.
It does not carry OpenAI Responses fields such as cached-vs-live
`external_web_access`, `search_context_size`, or text-and-image
`search_content_types`.

Claude Code's implementation under `/Users/mt/code/mt-ai/cc` reinforces the
same boundary: model-facing web-search schema, stream provider-state
preservation, and replay behavior must be handled as one contract. Codex should
apply that lesson at the Claude wire/API layer while preserving its existing
local handler and provider-owned server-tool split.

## What Changes

- Record the existing-tool audit as baseline, not as a broad enhancement:
  - ordinary function tools are complete;
  - namespace/dynamic MCP flattening is complete enough and remains unchanged;
  - freeform/custom JSON wrapping remains unchanged;
  - client-side `tool_search` remains unchanged;
  - web search is the only in-scope completion item.
- Add an explicit Claude web-search planning decision in `codex-tools`.
  The decision must choose one of:
  - native Claude server web search when the active spec is lossless for the
    selected native tool;
  - handler-backed local fallback when native would drop a semantic that the
    local handler can honor or explicitly report;
  - disabled/unsupported degradation when neither native nor local fallback can
    safely honor the configured semantics.
- Preserve the current native path for live web search with supported native
  fields:
  - map `filters.allowed_domains`;
  - map `user_location`;
  - preserve typed `WebSearchCall`, server result blocks, and structured
    citation replay.
- Avoid silently dropping unsupported Responses fields:
  - do not use native Claude web search for cached-only
    `external_web_access = false`;
  - do not use native Claude web search when `search_context_size` requires the
    local handler's result-budget behavior;
  - do not pretend non-text `search_content_types` are supported by native or
    local text-only fallback.
- Keep DeepSeek-compatible Claude providers on the existing local fallback
  path unless a provider-specific native path is proposed separately.

## Capabilities

### Modified Capabilities

- `claude-wire-api-support`: Claude web-search planning becomes
  lossless-or-explicit instead of silently dropping Responses-only fields.
- `tool-wire-serialization`: `codex-tools` records native, fallback, or
  disabled web-search decisions in `ClaudeNativeToolPolicy`.
- `tool-execution`: local fallback remains backed by `WebSearchHandler`, and
  unsupported local-only semantics return model-correctable errors without
  performing search side effects.

## Impact

- Affected crates:
  - `codex-rs/tools`
  - `codex-rs/core`
- Affected tests:
  - `codex-tools` serializer/planning tests;
  - `codex-core` Claude request/history tests;
  - focused handler tests for local fallback semantics.
- Compatibility:
  - OpenAI Responses behavior remains unchanged.
  - Existing Claude function/freeform/namespace/tool-search behavior remains
    unchanged.
  - Anthropic native web search remains available for live search specs that
    only use native-supported fields.
  - Local fallback remains the provider-compatibility path for DeepSeek.
- Primary risks:
  - Treating cached-only or image-search semantics as native would keep hidden
    behavior drift.
  - Switching every Anthropic web search to local fallback would unnecessarily
    lose provider-native citations and server-tool replay.
  - Advertising local fallback for unsupported non-text search without a local
    error would perform a side-effect while violating the configured tool
    contract.
- Rollback:
  - Restore the previous Claude serializer branch for `ToolSpec::WebSearch`.
  - Keep the added tests as documentation of the remaining semantic gap if the
    policy is reverted.

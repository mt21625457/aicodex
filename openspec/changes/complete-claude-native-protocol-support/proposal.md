## Why

Codex now has a production-shaped Claude Messages adapter: direct
`/v1/messages` routing, prompt caching, token counting, ordered replay,
pause-turn continuation, provider-state preservation, stricter tool-result
validation, DeepSeek/Claude `apply_patch` defaults, and basic web-search
server-tool handling. The remaining gap is not the core turn loop; it is the
tool surface and provider-native content model.

The current Claude path still projects most Codex tools into user-defined
Claude tools or freeform wrappers. That keeps the existing Codex tool loop
working, but it leaves model reliability on the table where Anthropic provides
trained-in schemas and server-executed tools. It also leaves several Claude
content blocks as opaque provider state or visible text markers instead of
first-class internal data.

This matters most for:

- Coding turns: Claude is trained to use `text_editor` and `bash` schemas for
  file editing and command execution. Our `apply_patch` wrapper is compatible,
  but not the strongest Claude-native contract.
- Research and data turns: newer Claude server tools include
  `web_search_20260209`, `web_fetch_20260209`, `code_execution_20260120`, and
  `advisor_20260301`; these produce provider-owned result blocks and usage
  fields that must round-trip correctly.
- Large tool catalogs: Claude's native tool search and deferred loading can
  reduce tool prompt bloat while preserving prompt-cache keys.
- MCP: Anthropic's current MCP connector uses `mcp_servers` plus `mcp_toolset`
  configuration and a beta header. Codex currently flattens local MCP and
  namespace tools into Claude-safe names, which is still required for local
  STDIO MCP servers but is not the native remote-MCP protocol.
- Citations: Claude can return structured citation lists and streaming
  `citations_delta` events. Rendering source markers is useful as a fallback,
  but multi-turn citation correctness requires preserving structured citation
  fields such as `encrypted_index`, document locations, and web result
  metadata.

The proposal completes Claude protocol support without changing the OpenAI
Responses path and without replacing Codex's existing local tool execution and
approval model.

## What Changes

- Add a Claude-native tool planning layer in `codex-tools`:
  - emit Anthropic-schema client tools for `text_editor`, `bash`, `memory`, and
    `computer` only when their executor and safety policy are available;
  - emit newer server tools for `web_search`, `web_fetch`, `code_execution`,
    `advisor`, and native `tool_search` only when the active Claude provider,
    model, platform, and beta headers allow them;
  - keep user-defined/freeform tools and flattened namespace tools as the
    fallback path for providers that do not support the native surface.
- Add Claude-native client tool execution adapters in `codex-core`:
  - map `text_editor` calls to Codex file-edit primitives with approval,
    workspace scoping, diff capture, and user-visible patch summaries;
  - map `bash` calls to Codex's shell execution policy with sandboxing,
    approval, output truncation, and session-state rules;
  - gate `memory` and `computer` behind explicit local capability providers.
- Extend Claude server-tool support in `codex-api` and `codex-core`:
  - support `web_search_20260209` and `web_fetch_20260209`, including dynamic
    filtering requirements and server result blocks;
  - support `code_execution_20260120` / `code_execution_20250825` result blocks
    as provider state, including generated-file metadata where available;
  - support `advisor_20260301` result blocks, usage iterations, and follow-up
    replay constraints.
- Add structured citation support:
  - represent Claude citations as typed protocol data instead of only appended
    visible text;
  - stream citation deltas into the current text block and preserve final
    citation metadata for UI, telemetry, and multi-turn replay;
  - keep a display fallback that can render source markers when clients do not
    yet understand structured citations.
- Add native remote-MCP connector support:
  - model `mcp_servers` separately from ordinary tools;
  - emit one `mcp_toolset` tool per remote server with allowlist/denylist and
    `defer_loading` configuration;
  - preserve `mcp_tool_use` and `mcp_tool_result` blocks in Claude history;
  - keep local STDIO MCP servers on the existing Codex-managed flattened tool
    path.
- Replace Codex's Claude-local `tool_search` projection with Claude native
  tool search when enabled:
  - emit `tool_search_tool_regex_20251119` or
    `tool_search_tool_bm25_20251119`;
  - mark large or rarely used tool definitions with `defer_loading: true`;
  - preserve `tool_reference` and `tool_search_tool_result` blocks in history.
- Add provider capability and safety gating:
  - model/version capability table for each Anthropic-provided tool type;
  - platform constraints for Anthropic API, Claude Platform on AWS, Bedrock,
    Vertex, Microsoft Foundry, and DeepSeek Claude-compatible endpoints;
  - explicit beta-header construction scoped to Claude endpoints.

## Capabilities

### New Capabilities

- `claude-native-client-tools`: Codex can expose Anthropic-schema client tools
  and execute them through Codex's local approval, sandbox, and file-edit
  systems.
- `claude-native-server-tools`: Codex can expose Claude server-executed tools
  and preserve their server result blocks, usage, and follow-up replay state.
- `claude-structured-citations`: Codex can preserve Claude citation metadata as
  structured data while retaining a text fallback.
- `claude-native-mcp-connector`: Codex can use Anthropic's remote MCP connector
  when the server is HTTPS-reachable and policy allows provider-side
  execution.
- `claude-native-tool-search`: Codex can use Claude's server-side tool search
  and deferred loading for large tool catalogs.

### Modified Capabilities

- `claude-wire-api-support`: The Claude adapter grows a capability-driven tool
  planner, richer provider block parsing, structured citation transport, native
  MCP request fields, and native tool-search request/stream handling.
- `tool-serialization`: `codex-tools` returns a Claude tool plan rather than
  only a flat `tools` array and reverse name table.
- `tool-execution`: `codex-core` can execute Anthropic-schema client tools
  through existing Codex execution primitives without bypassing approvals.
- `provider-capabilities`: provider metadata includes per-tool model/platform
  support, beta headers, ZDR/data-retention notes, and fallback strategy.

## Impact

- Affected crates:
  - `codex-rs/tools`
  - `codex-rs/codex-api`
  - `codex-rs/core`
  - `codex-rs/protocol` if structured citation types or new response events
    cross the public boundary
  - `codex-rs/model-provider` if provider capability metadata changes
- Affected docs/tests:
  - `docs/config.md`
  - Claude manual test README if present
  - OpenSpec specs for Claude wire support
- Compatibility:
  - OpenAI Responses request construction and SSE parsing remain unchanged.
  - DeepSeek Claude-compatible providers keep the conservative fallback tool
    surface unless explicitly marked compatible with native Anthropic tool
    schemas.
  - Existing `apply_patch` freeform behavior remains available as a fallback
    and migration escape hatch.
- Primary risks:
  - Exposing both local `bash` and server `code_execution` can confuse Claude
    because they are separate execution environments. System prompt
    disambiguation and capability policy must prevent accidental state sharing
    assumptions.
  - Native `text_editor` can bypass the apply-patch grammar unless the executor
    translates edits through Codex's existing edit review and approval path.
  - Remote MCP connector data is provider-side and not ZDR-eligible in current
    Anthropic docs. The feature must be opt-in and policy-visible.
  - Provider/tool version availability changes over time. Capability metadata
    must be centralized and tested so upgrades do not silently alter requests.
  - Structured citations require protocol/schema changes. Clients that only
    understand plain `OutputText` need a compatibility rendering path.
- Rollback:
  - Disable the native Claude tool planner and return to current
    user-defined/freeform tool serialization.
  - Continue parsing provider result blocks as opaque provider state while
    leaving structured citations behind the compatibility marker renderer.
  - Keep native MCP and native tool search disabled by config/provider policy.

## References

- Anthropic tool reference:
  https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-reference
- Anthropic tool-use overview:
  https://platform.claude.com/docs/en/docs/agents-and-tools/tool-use/overview/
- Anthropic web search tool:
  https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-search-tool
- Anthropic web fetch tool:
  https://platform.claude.com/docs/en/agents-and-tools/tool-use/web-fetch-tool
- Anthropic code execution tool:
  https://platform.claude.com/docs/en/agents-and-tools/tool-use/code-execution-tool
- Anthropic tool search:
  https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool
- Anthropic MCP connector:
  https://platform.claude.com/docs/en/agents-and-tools/mcp-connector
- Anthropic citations:
  https://platform.claude.com/docs/en/build-with-claude/citations

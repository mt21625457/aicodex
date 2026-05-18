## Context

The existing Claude work established the correct adapter boundary:

- `codex-tools` serializes Codex tools into Claude-compatible declarations and
  maintains reverse tool-call metadata.
- `codex-api` owns Claude HTTP, headers, SSE parsing, usage mapping, and
  provider-state block accumulation.
- `codex-core` owns request history construction, turn continuation, local tool
  execution, approvals, and provider dispatch.
- `codex-protocol` owns provider-neutral response events/items that clients
  consume.

This proposal keeps that boundary. New Claude-native behavior must live in
Claude-specific adapters first, and shared protocol changes are allowed only
when clients need durable data that cannot be represented safely as existing
text, tool calls, or provider-state blocks.

The current implementation already fixes the immediate DeepSeek/Claude
`apply_patch` exposure issue by carrying provider defaults into `ToolsConfig`.
Native Anthropic tool support is a quality and conformance upgrade, not a
replacement for that fallback.

## Goals / Non-Goals

**Goals:**

- Expose Anthropic-schema client tools where Codex can execute them safely.
- Expose Claude server tools where provider/model/platform capability permits.
- Preserve all Claude server tool result blocks required for follow-up turns.
- Preserve citations as structured data while providing plain-text fallback
  rendering.
- Support Anthropic remote MCP connector without breaking local MCP servers.
- Use Claude native tool search and `defer_loading` for large tool catalogs.
- Centralize tool/version/platform/beta-header capability decisions.
- Prove every new request shape and stream block with typed unit tests and
  mocked end-to-end Claude turns.

**Non-Goals:**

- Do not route Claude through OpenAI Responses request or stream code.
- Do not remove the current `apply_patch` freeform fallback.
- Do not expose native `computer` or remote MCP by default without an explicit
  local capability and security policy.
- Do not connect local STDIO MCP servers through Anthropic's `mcp_servers`;
  they are not HTTPS remote MCP endpoints and must remain Codex-managed.
- Do not force server `code_execution` when local shell access is required.
- Do not add broad provider-transcript modeling beyond what Claude replay and
  client UI require.

## Technical Plan

### 1. Introduce a Claude tool plan instead of a flat tools array

Extend the Claude-specific output of `create_tools_json_for_claude_messages`
into a richer plan without breaking the existing `tools` and
`tool_call_info` fields:

- `tools: Vec<Value>` for the Claude `tools` request field.
- `tool_call_info: Vec<ClaudeToolCallInfo>` for client-executed tool reverse
  mapping.
- `mcp_servers: Vec<ClaudeMcpServer>` for native remote MCP.
- `beta_headers: BTreeSet<ClaudeBetaFeature>` for endpoint-owned header
  construction.
- `native_tool_policy: ClaudeNativeToolPolicy` describing which native tools
  were selected and why fallback was used.
- `history_requirements: ClaudeHistoryRequirements` describing provider blocks
  that must be preserved if the tool remains enabled on follow-up turns.

The plan is built from:

- Codex tool config and user policy;
- provider capabilities;
- model id and platform;
- existing `ToolSpec`s;
- MCP server metadata;
- context/tool-count heuristics.

The default policy is conservative:

- Prefer current Codex-managed tools unless a native Claude tool is explicitly
  enabled or selected by provider capability defaults.
- Never expose a native tool without an executor or server-tool replay handler.
- Keep DeepSeek Claude-compatible providers on the known fallback surface unless
  their provider config declares compatibility with the Anthropic-native schema.

### 2. Model Claude-provided tool versions centrally

Add a small capability table owned by the Claude provider adapter, not scattered
through request construction:

- client tools:
  - `text_editor_20250728` for Claude 4 class models;
  - `text_editor_20250124` for earlier supported models;
  - `bash_20250124`;
  - `memory_20250818`;
  - `computer_20251124` or `computer_20250124` only when beta policy and a
    local computer executor are available.
- server tools:
  - `web_search_20260209` for dynamic filtering where supported, otherwise
    `web_search_20250305`;
  - `web_fetch_20260209` where supported, otherwise `web_fetch_20250910`;
  - `code_execution_20260120` where model support exists, otherwise
    `code_execution_20250825`;
  - `advisor_20260301` only with an advisor model and beta access;
  - `tool_search_tool_regex_20251119` or
    `tool_search_tool_bm25_20251119`.
- remote MCP:
  - `mcp_toolset` with `mcp-client-2025-11-20`.

The capability table must include:

- required beta header, if any;
- model family/version constraints;
- platform constraints;
- data-retention/ZDR notes;
- whether Codex must execute the call locally;
- whether Codex must preserve provider result blocks in history;
- fallback behavior.

Tests should assert full request JSON for each policy branch. The capability
table must be the only place where dated Anthropic tool type strings are chosen.

### 3. Map native `text_editor` through Codex's edit safety model

Do not let native `text_editor` mutate files directly outside Codex's existing
file-edit controls. Add a Claude client-tool executor that accepts native
`text_editor` tool_use input and translates it into safe Codex operations:

- `view`: read workspace-scoped files/directories with line-numbered output.
- `create`: create a file only within writable workspace roots.
- `str_replace`: apply exactly one replacement and return Anthropic-compatible
  duplicate/not-found errors.
- `insert`: insert text at an explicit line boundary.
- `undo_edit`: support only for tool versions that define it, or return a
  schema-appropriate unsupported-command result.

Every mutating command must:

- pass through the same approval/sandbox policy as current file edits;
- produce a diff or patch summary suitable for UI and audit logs;
- reject path traversal and writes outside configured workspace roots;
- preserve user changes not made by Codex;
- return native text-editor-shaped `tool_result` content to Claude.

The current `apply_patch` freeform tool remains available when native
`text_editor` is disabled, unsupported, or rejected by policy.

### 4. Map native `bash` through Codex shell execution

Add a Claude `bash` executor that reuses Codex's shell approval and sandbox
implementation. It must not create an unreviewed command path.

Design choices:

- Keep an explicit `ClaudeBashSession` keyed by Codex session/turn if persistent
  shell state is enabled.
- If persistent shell state is not safe in the current runtime, advertise only
  the existing Codex shell/freeform tool and do not expose native `bash`.
- Preserve cwd, environment, timeout, exit code, stdout, and stderr in the
  returned tool result.
- Reject interactive commands, prompts, and long-running background processes
  according to existing shell policy.
- Keep approval prompts equivalent to current shell execution prompts.

If both native `bash` and server `code_execution` are enabled, inject a
Claude-only system note that clearly separates local shell state from
Anthropic's server-side code execution container.

### 5. Gate `memory` and `computer` behind explicit local capabilities

Native `memory` is client-executed and can be useful for long-running agent
state, but it requires a scoped storage backend. Codex should expose it only
when a memory root is configured:

- restrict paths to `/memories` in the virtual protocol and map that to a
  configured local directory;
- reject path traversal and oversized files;
- make memory retention explicit in config and docs;
- add cleanup/expiration hooks later if needed.

Native `computer` is high risk and beta-gated. Codex should not expose it from
the Rust Claude adapter until there is a first-class local computer executor
with screenshot, click, drag, keyboard, display metadata, approval policy, and
runtime isolation. Until then, keep it disabled and document the missing local
capability.

### 6. Upgrade server tools and preserve all server result blocks

Server tools execute on Anthropic infrastructure and do not produce
client-executed `tool_result` requests. Codex must preserve the assistant
content blocks that Anthropic emits:

- `server_tool_use`;
- `web_search_tool_result`;
- `web_fetch_tool_result`;
- `bash_code_execution_tool_result`;
- `text_editor_code_execution_tool_result`;
- `advisor_tool_result`;
- `tool_search_tool_result`;
- future `*_tool_result` blocks as opaque provider state when unknown.

`codex-api` should parse these blocks into typed Claude provider-state variants
where useful, then `codex-core` should serialize them back into Claude history
unchanged on follow-up turns.

Specific tool behavior:

- `web_search_20260209` and `web_fetch_20260209` should be selected only when
  dynamic filtering is supported by the active model/platform and server-side
  code execution is allowed by provider policy. This does not necessarily mean
  emitting a separate `code_execution` tool; it means Codex must not request a
  dynamic-filtering tool version where the provider cannot run the required
  server-side filtering path. Otherwise use the older GA versions or local
  web-search fallback.
- `code_execution_20260120` should be selected only for models that support it;
  fallback to `code_execution_20250825` where appropriate.
- `advisor_20260301` requires a configured advisor model, beta header, and
  per-conversation call cap. Its `usage.iterations` must remain visible to
  telemetry/cost accounting without corrupting top-level token usage.

### 7. Promote citations to structured protocol data

Replace the current citation marker-only behavior with structured citation
transport:

- add a protocol type such as `Citation` with Claude-backed variants:
  - web search result location;
  - char location;
  - page location;
  - content block location;
  - unknown provider citation JSON for forward compatibility.
- extend assistant text content to carry citations, either by adding an
  optional citation list to `ContentItem::OutputText` or by adding a new
  versioned text content variant.
- add a streaming event for citation deltas, or accumulate them inside
  `codex-api` and include them in final `OutputItemDone`.
- preserve fields required for multi-turn correctness, including encrypted
  web-search indices and document locations.
- keep visible source-marker rendering in UI/client adapters that cannot yet
  display structured citations.

Structured citations must also represent compatibility constraints:

- citations cannot be combined with strict structured output in Claude requests;
- web-search citations must include display-ready source information when
  outputs are shown to end users;
- citation metadata must not be counted as ordinary visible text in token
  accounting.

### 8. Add native remote MCP connector support without breaking local MCP

Codex has two different MCP paths and must keep them distinct:

1. Local Codex-managed MCP:
   - local STDIO or private MCP servers;
   - Codex executes calls and returns tool results;
   - continue flattening into Claude-safe tool names such as
     `mcp__server__tool`.
2. Anthropic remote MCP connector:
   - publicly reachable HTTPS MCP servers;
   - Anthropic executes connector calls server-side;
   - request contains `mcp_servers` and `tools: [{ "type": "mcp_toolset", ... }]`;
   - response contains `mcp_tool_use` / `mcp_tool_result` blocks.

Add config/provider policy to decide which MCP servers are eligible for native
remote connector mode. Required properties:

- URL must be HTTPS.
- Server name must be unique and must match exactly one `mcp_toolset`.
- Auth token handling must avoid logging or transcript leakage.
- Native remote MCP must be opt-in because current docs mark it non-ZDR.
- Allowlist/denylist must be expressible with `default_config` and `configs`.
- `defer_loading` must be supported per toolset and per tool config.

The stream parser must preserve native MCP blocks and must not try to execute
them through Codex's local MCP connection manager.

### 9. Use Claude native tool search and deferred loading

For large Claude tool catalogs, Codex should prefer native tool search when
policy permits:

- include `tool_search_tool_regex_20251119` or
  `tool_search_tool_bm25_20251119`;
- mark low-priority or many MCP-derived tools with `defer_loading: true`;
- keep 3-5 critical tools non-deferred, including any tool needed for safety or
  current task setup;
- never defer the tool-search tool itself;
- ensure every possible `tool_reference` has a matching top-level tool
  definition;
- keep prompt caching stable by deferring volatile/rare tools instead of moving
  them into the prompt prefix.

Codex's current local `tool_search` function remains the fallback for
non-Anthropic-compatible providers and for cases where server-side tool search
is not available.

### 10. Keep namespace mapping deterministic and backward compatible

Claude tool names must remain within Anthropic's name constraints. The updated
namespace joining rules should stay:

- preserve MCP names that already use the `mcp__server__tool` convention;
- add a separator for ordinary namespaces such as `codex_app_lookup_order`;
- sanitize punctuation deterministically;
- hash and bound names over the Claude length limit;
- keep reverse metadata so streamed `tool_use.name` maps back to
  `ResponseItem::{FunctionCall, CustomToolCall, ToolSearchCall}`.

Native MCP connector mode should not use this flattened namespace path. It must
represent server name and tool name separately in `mcp_tool_use` blocks.

## Rollout Plan

### Phase 0: Capability inventory and request planner

Add the Claude native tool capability table and the richer Claude tool plan.
Keep all native features disabled by default except currently supported web
search behavior. Lock the generated request JSON in unit tests.

### Phase 1: Provider block preservation and structured parsing

Extend the Claude stream parser to preserve every known server-result and MCP
block in provider order. Unknown `*_tool_result` blocks remain opaque provider
state with round-trip tests.

### Phase 2: Native client tools for coding

Implement `text_editor` first, then `bash`, both through existing Codex
approval/sandbox primitives. Keep `apply_patch` fallback enabled.

### Phase 3: Server tools

Upgrade web search/fetch and add code execution/advisor request planning,
stream parsing, usage handling, and follow-up replay.

### Phase 4: Structured citations

Add protocol/schema support for citations and update UI/client rendering to use
structured data where available.

### Phase 5: Native remote MCP connector

Add `mcp_servers`, `mcp_toolset`, beta header policy, native MCP block parsing,
and local-vs-remote MCP selection.

### Phase 6: Native tool search and deferred loading

Add tool search request planning, `defer_loading`, tool reference replay, cache
compatibility tests, and large-catalog heuristics.

### Phase 7: Defaults, docs, and compatibility cleanup

After mocked and manual Claude tests pass, decide which native features can be
enabled by default for Anthropic API providers and which remain opt-in.

## Risks / Trade-offs

- Native Claude tools improve model reliability but expand the adapter surface.
  Keep provider-specific behavior in Claude modules and lock each request shape.
- Native `text_editor` requires line-based edit semantics, while Codex's
  existing edit surface is patch-oriented. Translation must preserve user
  review and avoid unexpected overwrites.
- Native `bash` persistence is useful but can break assumptions if Codex
  sandboxes each command independently. Do not advertise native `bash` unless
  the runtime can satisfy the semantics we claim.
- Server `code_execution` and local shell create a multi-computer environment.
  The model must be told which environment owns which files and state.
- Native remote MCP can send tool definitions and results through Anthropic.
  Policy and documentation must make this data path explicit.
- Structured citation changes may require protocol/schema updates across
  app-server/TUI clients. Use optional fields or versioned variants to preserve
  compatibility.
- Tool search can fail if all tools are deferred or referenced tools are
  missing. Planner tests must reject invalid plans locally.

## Verification

- `openspec validate complete-claude-native-protocol-support --strict`
- `cd codex-rs && just fmt`
- `cd codex-rs && cargo test -p codex-tools claude`
- `cd codex-rs && cargo test -p codex-api claude`
- `cd codex-rs && cargo test -p codex-core claude`
- `cd codex-rs && cargo test -p codex-protocol` if protocol citation types
  change
- `cd codex-rs && cargo test -p codex-app-server-protocol` if public API types
  or TypeScript fixtures change
- `cd codex-rs && just fix -p codex-tools`
- `cd codex-rs && just fix -p codex-api`
- `cd codex-rs && just fix -p codex-core`

If `ConfigToml` changes, also run:

- `cd codex-rs && just write-config-schema`

If app-server protocol/schema output changes, also run the existing app-server
schema generation task and include generated artifacts.

If Rust dependencies change, also run from the repository root:

- `just bazel-lock-update`
- `just bazel-lock-check`

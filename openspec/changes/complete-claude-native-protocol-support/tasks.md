## 1. OpenSpec and Scope

- [x] 1.1 Validate this proposal against the completed Claude OpenSpec changes
  and keep this change focused on remaining native protocol gaps.
- [x] 1.2 Confirm the current DeepSeek/Claude `apply_patch` fallback remains
  enabled while native tools are introduced.
- [x] 1.3 Run `openspec validate complete-claude-native-protocol-support --strict`.

## 2. Claude Native Tool Planning

- [x] 2.1 Extend the Claude tool serializer result into a `ClaudeToolPlan` shape
  that preserves existing tools/reverse-call metadata while adding MCP servers,
  beta headers, native policy decisions, and history preservation requirements.
- [x] 2.2 Add a centralized Anthropic tool capability table for client tools,
  server tools, native MCP connector, and tool search.
- [x] 2.3 Gate each native tool by model, provider platform, beta header,
  executor availability, ZDR/data-retention policy, and fallback behavior.
- [x] 2.4 Add request-shape tests for Anthropic API, DeepSeek-compatible
  fallback, Bedrock/Vertex limitations, and unsupported model/tool versions.
- [x] 2.5 Keep dated Anthropic tool type strings in one module with tests.

## 3. Native Client Tools

- [x] 3.1 Implement `text_editor` request planning for supported Claude models.
- [x] 3.2 Add a Claude text-editor executor that maps `view`, `create`,
  `str_replace`, `insert`, and version-supported `undo_edit` commands to Codex
  file operations.
- [x] 3.3 Ensure every text-editor mutation passes through workspace scoping,
  approval/sandbox policy, diff capture, and user-change preservation.
- [x] 3.4 Implement `bash` request planning only when Codex can satisfy the
  advertised shell-session semantics.
- [x] 3.5 Add a Claude bash executor using Codex shell approval, sandbox,
  timeout, cwd/env, stdout/stderr, and exit-code handling.
- [x] 3.6 Keep `memory` disabled until a scoped `/memories` backend is
  configured; add request/executor tests when enabled.
- [x] 3.7 Keep `computer` disabled until a first-class local computer executor
  exists; document the explicit capability gate.
- [x] 3.8 Add mocked Claude tool-loop tests for native text-editor and bash
  calls, including success, validation failure, approval denial, and tool error
  results.

## 4. Server Tools

- [x] 4.1 Add request planning for `web_search_20260209` with fallback to
  `web_search_20250305` or local web search when dynamic filtering is not
  available.
- [x] 4.2 Add request planning for `web_fetch_20260209` with fallback to
  `web_fetch_20250910` or disabled behavior when unsupported.
- [x] 4.3 Add request planning for `code_execution_20260120` and
  `code_execution_20250825` with model/platform gating.
- [x] 4.4 Add request planning for `advisor_20260301`, including advisor model,
  beta header, max-use policy, and optional caching.
- [x] 4.5 Parse and preserve `web_search_tool_result`, `web_fetch_tool_result`,
  `bash_code_execution_tool_result`, `text_editor_code_execution_tool_result`,
  `advisor_tool_result`, and unknown `*_tool_result` blocks.
- [x] 4.6 Preserve server-tool result blocks in provider order and replay them
  unchanged on follow-up Claude requests.
- [x] 4.7 Map `usage.server_tool_use` and advisor `usage.iterations` into
  telemetry or structured accounting without breaking existing token totals.
- [x] 4.8 Add mocked streaming tests for server-tool success, server-tool error
  result blocks, pause-turn continuation, and follow-up replay.

## 5. Structured Citations

- [x] 5.1 Add a provider-neutral structured citation type with Claude-backed
  variants for web-search, char, page, content-block, and unknown citations.
- [x] 5.2 Extend assistant text content or response events to carry citation
  metadata without breaking clients that only understand plain output text.
- [x] 5.3 Accumulate `citations_delta` events into the active text block and
  include the structured citation list in final output.
- [x] 5.4 Preserve fields needed for multi-turn correctness, including
  `encrypted_index`, document indices, page ranges, character ranges, and
  cited text.
- [x] 5.5 Keep visible source-marker rendering as a compatibility fallback.
- [x] 5.6 Add tests for web-search citations, document citations, streaming
  citation deltas, strict structured-output incompatibility, and fallback
  rendering.

## 6. Native Remote MCP Connector

- [x] 6.1 Add Claude request support for `mcp_servers` with HTTPS URL, unique
  server names, optional authorization token, and secret-safe logging.
- [x] 6.2 Add `mcp_toolset` request support with `default_config`, per-tool
  `configs`, allowlist/denylist behavior, `defer_loading`, and cache control.
- [x] 6.3 Require `mcp-client-2025-11-20` beta header for native remote MCP and
  keep remote MCP opt-in because of data-retention constraints.
- [x] 6.4 Preserve `mcp_tool_use` and `mcp_tool_result` blocks without routing
  them through Codex's local MCP executor.
- [x] 6.5 Keep local STDIO/private MCP servers on the existing flattened
  Codex-managed tool path.
- [x] 6.6 Add request validation tests for missing toolsets, duplicate server
  names, unknown local/remote mode, and invalid HTTP/non-HTTPS MCP URLs.
- [x] 6.7 Add mocked stream/replay tests for native MCP tool use and result
  blocks.

## 7. Native Tool Search and Deferred Loading

- [x] 7.1 Add request planning for `tool_search_tool_regex_20251119` and
  `tool_search_tool_bm25_20251119`.
- [x] 7.2 Add tool-catalog heuristics for marking rarely used or large tool
  definitions with `defer_loading: true`.
- [x] 7.3 Ensure the tool-search tool itself is never deferred and at least one
  useful non-deferred tool remains available.
- [x] 7.4 Ensure every deferred tool that may be referenced remains present in
  the top-level `tools` list with a matching definition.
- [x] 7.5 Parse and preserve `tool_search_tool_result` and `tool_reference`
  blocks in Claude history.
- [x] 7.6 Add prompt-cache tests proving deferred tools do not invalidate stable
  cache prefixes.
- [x] 7.7 Keep Codex's current local `tool_search` function as fallback for
  unsupported providers.

## 8. Namespace and Backward Compatibility

- [x] 8.1 Preserve current Claude-safe flattened namespace behavior for ordinary
  Codex namespace tools and local MCP tools.
- [x] 8.2 Ensure native remote MCP does not use flattened names and instead
  preserves separate server/tool identities.
- [x] 8.3 Add history replay tests for old flattened tool names and new native
  tool names.
- [x] 8.4 Add collision, sanitization, and 64-character bound tests for mixed
  native and fallback tool catalogs.

## 9. Documentation and Manual Testing

- [x] 9.1 Update `docs/config.md` with Claude native tool policy, fallback
  behavior, server-tool limitations, MCP data path, and citation support.
- [x] 9.2 Add or update Claude manual tests for native text editor, bash,
  server web search/fetch, code execution, advisor, native MCP, and tool search.
- [x] 9.3 Document that server `code_execution` and local shell are separate
  environments and must not assume shared files or variables.
- [x] 9.4 Document which native tools remain opt-in and why.

## 10. Verification

- [x] 10.1 Run `cd codex-rs && just fmt`.
- [x] 10.2 Run `cd codex-rs && cargo test -p codex-tools claude`.
- [x] 10.3 Run `cd codex-rs && cargo test -p codex-api claude`.
- [x] 10.4 Run `cd codex-rs && cargo test -p codex-core claude`.
- [x] 10.5 Run `cd codex-rs && cargo test -p codex-protocol` if protocol
  citation types change.
- [x] 10.6 Run `cd codex-rs && cargo test -p codex-app-server-protocol` if
  public protocol/schema output changes.
- [x] 10.7 Run scoped `just fix -p <crate>` for every changed Rust crate after
  targeted tests pass.
- [x] 10.8 Run `cd codex-rs && just write-config-schema` if `ConfigToml`
  changes.
- [x] 10.9 Run app-server schema generation if app-server protocol fixtures
  change.
- [x] 10.10 Run `just bazel-lock-update` and `just bazel-lock-check` if Rust
  dependencies change.

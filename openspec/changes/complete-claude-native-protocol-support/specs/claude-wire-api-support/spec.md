## ADDED Requirements

### Requirement: Claude tool planning must be capability driven

Codex MUST build Claude Messages tool requests from a centralized Claude tool
plan that records emitted tools, reverse tool-call metadata, native MCP servers,
required beta headers, provider capability decisions, and replay requirements.
Native Claude tools MUST be emitted only when the active provider, model,
platform, executor availability, and safety policy support them. Otherwise
Codex MUST fall back to the existing user-defined/freeform Claude tool surface.

#### Scenario: DeepSeek-compatible Claude providers keep fallback tools

- **WHEN** the active provider uses the Claude wire API but does not declare
  compatibility with Anthropic-native tool schemas
- **THEN** Codex emits the existing user-defined/freeform Claude tools
- **AND** Codex does not emit Anthropic-only tool type strings such as
  `text_editor_20250728` or `bash_20250124`
- **AND** `apply_patch` remains available through the provider default selected
  by `ToolsConfig`

#### Scenario: Native tool type strings are selected centrally

- **WHEN** a Claude request enables Anthropic-provided tools
- **THEN** every dated Anthropic tool `type` string is selected by the central
  Claude capability table
- **AND** request builders do not duplicate model/version branching logic
  inline
- **AND** unsupported model/tool combinations fail locally or fall back before
  a network request is sent

### Requirement: Claude native text editor must use Codex edit safety

Codex MUST execute Claude native `text_editor` calls through Codex-controlled
file-edit primitives. Native text-editor commands MUST NOT mutate files outside
workspace scoping, approval, sandbox, diff capture, and user-change
preservation rules.

#### Scenario: Native str_replace edit produces a reviewed local edit

- **WHEN** Claude calls the native text editor with a `str_replace` command for
  a workspace file
- **THEN** Codex validates the path, verifies the target text is unique, applies
  the edit through the local edit path, records a diff summary, and returns a
  text-editor-shaped `tool_result`
- **AND** the same approval policy that guards current Codex file edits is
  applied

#### Scenario: Native edit outside the workspace is rejected

- **WHEN** Claude calls the native text editor with a path outside configured
  writable roots or with path traversal
- **THEN** Codex rejects the call locally with a native tool error result
- **AND** no file mutation occurs

### Requirement: Claude native bash must use Codex shell policy

Codex MUST expose Claude native `bash` only when Codex can satisfy the
advertised shell execution semantics through its own shell approval, sandbox,
timeout, cwd/env, output, and exit-code handling. Codex MUST NOT add a second
unreviewed shell execution path.

#### Scenario: Native bash command follows existing approval flow

- **WHEN** Claude calls the native `bash` tool with a shell command
- **THEN** Codex evaluates the command with the same approval and sandbox policy
  used by existing shell execution
- **AND** the returned tool result includes stdout, stderr, exit code, and any
  timeout/error status in the Claude bash contract shape

#### Scenario: Native bash is not advertised when persistence is unavailable

- **WHEN** the current runtime cannot satisfy required native bash session
  semantics
- **THEN** Codex does not emit the native `bash` tool
- **AND** Codex keeps the current shell/freeform fallback tool available when
  policy permits shell execution

### Requirement: High-risk client tools must be explicitly gated

Codex MUST gate Claude native `memory` and `computer` behind explicit local
capabilities and safety configuration. These tools MUST NOT be exposed by
default solely because Anthropic defines their schemas.

#### Scenario: Memory tool requires a scoped backend

- **WHEN** the Claude native memory tool is requested
- **THEN** Codex exposes `memory_20250818` only if a scoped memory backend is
  configured
- **AND** all memory paths are confined to the configured `/memories` mapping
- **AND** path traversal and oversized reads/writes are rejected

#### Scenario: Computer tool remains disabled without executor

- **WHEN** no first-class local computer-use executor is available
- **THEN** Codex does not emit `computer_20251124` or `computer_20250124`
- **AND** the tool planner records that the feature was disabled by missing
  local capability

### Requirement: Claude server tools must be versioned, parsed, and replayed

Codex MUST support Claude server-executed tool definitions and MUST preserve
their result blocks in provider order for follow-up turns. Server tool result
blocks MUST NOT be treated as client tool calls requiring Codex execution.

#### Scenario: Web search dynamic filtering is selected only when supported

- **WHEN** the provider, model, and platform support `web_search_20260209`
- **AND** server-side dynamic filtering is allowed by policy
- **THEN** Codex emits `web_search_20260209`
- **ELSE** Codex falls back to `web_search_20250305`, local web search, or no
  web search according to provider policy

#### Scenario: Server result blocks are preserved in follow-up history

- **WHEN** Claude returns `server_tool_use` followed by
  `web_search_tool_result`, `web_fetch_tool_result`,
  `bash_code_execution_tool_result`, `text_editor_code_execution_tool_result`,
  `advisor_tool_result`, `tool_search_tool_result`, or an unknown
  `*_tool_result`
- **THEN** Codex stores the raw provider block in assistant history at the
  original content-block position
- **AND** the next Claude request replays that block unchanged if the
  corresponding tool remains enabled
- **AND** Codex does not send a client `tool_result` for that server tool call

#### Scenario: Advisor usage is reported without corrupting top-level usage

- **WHEN** Claude returns advisor `usage.iterations`
- **THEN** Codex preserves advisor iteration usage for telemetry or cost
  accounting
- **AND** Codex does not fold advisor sub-inference tokens into existing
  top-level token fields in a way that breaks `TokenUsage` invariants

### Requirement: Claude citations must be structured protocol data

Codex MUST preserve Claude citation metadata as structured data instead of only
rendering citations into visible text. A plain-text marker MAY be produced for
clients that do not yet support structured citations, but the structured data
MUST remain available for UI, telemetry, and multi-turn replay.

#### Scenario: Streaming citation delta attaches to current text block

- **WHEN** Claude streams a `citations_delta` for a text content block
- **THEN** Codex attaches the citation to the active text block's structured
  citation list
- **AND** the final assistant message includes the citation metadata
- **AND** clients that only support plain text can still render a source marker

#### Scenario: Web search citation preserves encrypted index

- **WHEN** Claude returns a `web_search_result_location` citation with
  `url`, `title`, `encrypted_index`, and `cited_text`
- **THEN** Codex preserves each field in the structured citation
- **AND** follow-up Claude history can include the provider-required encrypted
  citation state
- **AND** token accounting does not count citation metadata as ordinary visible
  assistant text

#### Scenario: Citations and strict structured output are not combined silently

- **WHEN** a Claude request would enable citations and strict structured output
  together
- **THEN** Codex rejects the incompatible combination locally or downgrades it
  with an explicit warning according to configured policy
- **AND** tests cover the selected behavior

### Requirement: Claude native remote MCP connector must be distinct from local MCP

Codex MUST distinguish Anthropic remote MCP connector mode from Codex-managed
local MCP mode. Remote MCP connector mode MUST use Claude `mcp_servers` and
`mcp_toolset`; local STDIO/private MCP servers MUST continue using the
Codex-managed flattened tool path.

#### Scenario: Remote HTTPS MCP server emits native request fields

- **WHEN** a policy-approved remote HTTPS MCP server is configured for native
  Claude connector mode
- **THEN** Codex emits that server in `mcp_servers`
- **AND** Codex emits exactly one `mcp_toolset` entry referencing that server
- **AND** Codex includes the required `mcp-client-2025-11-20` beta header
- **AND** authorization tokens are not logged or exposed in user-visible
  transcript text

#### Scenario: Local STDIO MCP remains Codex-managed

- **WHEN** an MCP server is local, private, STDIO-based, or otherwise not
  eligible for Anthropic remote MCP
- **THEN** Codex does not put it in `mcp_servers`
- **AND** Codex keeps using flattened Claude-safe local tool names and Codex's
  MCP connection manager to execute calls

#### Scenario: Native MCP result blocks are not locally executed

- **WHEN** Claude returns `mcp_tool_use` and `mcp_tool_result` blocks from the
  remote MCP connector
- **THEN** Codex preserves those blocks in provider history
- **AND** Codex does not route them to the local MCP executor

### Requirement: Claude native tool search must support deferred loading

Codex MUST support Claude native tool search for large tool catalogs when
provider policy permits. Deferred tools MUST remain available in the top-level
tool catalog, but MUST NOT be loaded into the initial system prompt until
Claude discovers them through tool search.

#### Scenario: Large catalog uses tool search and defer_loading

- **WHEN** the Claude tool catalog crosses the configured size threshold
- **AND** native tool search is supported by the active provider and model
- **THEN** Codex emits a native tool search tool
- **AND** Codex marks eligible low-priority tools with `defer_loading: true`
- **AND** Codex keeps the tool-search tool itself and a small critical tool set
  non-deferred

#### Scenario: Invalid all-deferred plans fail locally

- **WHEN** native tool search planning would defer every ordinary tool or omit
  a definition that can be referenced
- **THEN** Codex rejects the request plan locally
- **AND** no invalid Claude request is sent

#### Scenario: Tool references replay correctly

- **WHEN** Claude returns a `tool_search_tool_result` containing
  `tool_reference` blocks
- **THEN** Codex preserves the search result in Claude assistant history
- **AND** the referenced tools remain resolvable on subsequent turns

### Requirement: Claude namespace fallback must remain deterministic

Codex MUST keep deterministic Claude-safe names for fallback client tools,
namespaced tools, and local MCP tools. Native MCP connector blocks MUST preserve
server and tool identity separately and MUST NOT be forced through flattened
fallback naming.

#### Scenario: Ordinary namespace gets a separator

- **WHEN** a fallback Claude tool is created from namespace `codex_app` and name
  `lookup_order`
- **THEN** the Claude tool name is `codex_app_lookup_order`
- **AND** reverse metadata maps streamed tool calls back to the original
  namespace and name

#### Scenario: Existing MCP-style flattened name is preserved

- **WHEN** a fallback local MCP tool already follows
  `mcp__server__tool` naming
- **THEN** Codex preserves the double-underscore convention after sanitization
- **AND** collision handling still produces unique names within Claude's tool
  name length limit

## MODIFIED Requirements

### Requirement: Claude wire API support must preserve provider-native content

Codex's Claude wire adapter MUST preserve provider-native Claude content blocks
that are required for valid follow-up turns, including server tool results,
native MCP results, tool-search results, citations, redacted/advisor/provider
state, and future unknown provider-owned tool result blocks.

#### Scenario: Unknown provider-owned result is opaque but not lost

- **WHEN** Claude streams an unknown content block whose type ends with
  `_tool_result`
- **THEN** Codex stores the raw block as Claude provider state
- **AND** the block is replayed unchanged in follow-up history
- **AND** telemetry records the unknown block type for future typed support

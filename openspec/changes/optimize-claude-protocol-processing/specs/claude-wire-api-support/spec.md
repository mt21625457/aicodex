## ADDED Requirements

### Requirement: Claude request history must be normalized before validation

Codex MUST normalize Claude Messages history after converting internal prompt
items into Claude content blocks and before applying prompt-cache metadata or
sending the request. Normalization MUST preserve valid Claude conversation
semantics and MUST repair or reject malformed recovered history locally instead
of repeatedly sending provider-invalid requests.

#### Scenario: Whitespace-only assistant content is removed

- **WHEN** recovered Claude history contains an assistant message whose content
  is only empty or whitespace text blocks
- **THEN** Codex removes that assistant message or replaces it with a local
  placeholder only when role alternation would otherwise become invalid
- **AND** no Claude request contains a whitespace-only text block

#### Scenario: Orphan thinking-only content is not replayed as valid output

- **WHEN** recovered Claude history contains a thinking-only assistant fragment
  that cannot be matched with the rest of its provider response
- **THEN** Codex drops or quarantines that fragment before request validation
- **AND** valid thinking blocks with signatures that remain part of a complete
  assistant response are preserved

#### Scenario: Error tool results contain only text

- **WHEN** a user `tool_result` has `is_error = true`
- **AND** its nested content contains image, document, provider-state, or other
  non-text blocks
- **THEN** Codex strips or summarizes the non-text blocks into text-only
  content before sending the Claude request

#### Scenario: Tool results remain first in user content

- **WHEN** a user message contains `tool_result` blocks and ordinary user
  content
- **THEN** Codex sends all required `tool_result` blocks before ordinary
  content
- **AND** ordinary content is preserved after the required tool results when it
  is valid for Claude Messages

### Requirement: Claude media payloads must be bounded before send

Codex MUST count Claude media blocks in outgoing Claude Messages requests and
MUST prune excess media before sending a request that would exceed the active
provider's media limit. Pruning MUST preserve text, tool-result identities, and
the most recent media wherever possible.

#### Scenario: Excess top-level media is pruned oldest first

- **WHEN** an outgoing Claude request contains more top-level image or document
  media blocks than the active limit allows
- **THEN** Codex removes the oldest excess media blocks before sending
- **AND** the most recent media blocks remain in the request

#### Scenario: Nested tool-result media is counted

- **WHEN** media appears inside `tool_result.content`
- **THEN** Codex includes that media in the same per-request media limit
- **AND** pruning nested media does not remove the `tool_result.tool_use_id`

#### Scenario: Pruned tool-result media leaves a text placeholder

- **WHEN** pruning media would leave a tool result with no content
- **THEN** Codex replaces the pruned content with a text placeholder indicating
  media was omitted
- **AND** the Claude tool-result pairing remains valid

### Requirement: Claude stream failures must be classified before fallback

Codex MUST classify Claude stream failures and MUST NOT use fallback behavior
that can duplicate side-effecting tool execution. Empty or no-event stream
failures MAY be retried or fallback to non-streaming only when no assistant item
or tool call has been emitted.

#### Scenario: Empty stream can be retried safely

- **WHEN** a Claude stream closes or times out before `message_start`
- **THEN** Codex classifies the failure as an empty-stream failure
- **AND** Codex MAY retry or use a future non-streaming fallback according to
  provider policy because no tool execution has started

#### Scenario: Partial stream after tool input blocks fallback

- **WHEN** a Claude stream emits a tool call or streamed tool input before
  failing
- **THEN** Codex classifies the failure as unsafe for non-streaming fallback
- **AND** Codex does not start a fallback request that could produce and execute
  the same tool call again

#### Scenario: User abort is not treated as fallback

- **WHEN** the active turn is cancelled by the user
- **THEN** Codex treats the stream termination as user cancellation
- **AND** no retry or non-streaming fallback is started for that cancellation

### Requirement: Claude tool-plan replay requirements must govern history

Codex MUST use the active Claude tool plan's replay requirements when deciding
which provider-state, server-tool result, MCP result, and citation blocks can
be kept, dropped, or replayed in follow-up Claude requests.

#### Scenario: Server tool results survive when required

- **WHEN** the active Claude tool plan requires server tool result preservation
- **AND** prior assistant history contains provider-owned server tool use or
  result blocks
- **THEN** Codex preserves those blocks in provider order for the next Claude
  request

#### Scenario: Remote MCP results are not converted to local tool calls

- **WHEN** Claude native remote MCP is enabled and the provider returns
  `mcp_tool_use` or `mcp_tool_result` blocks
- **THEN** Codex preserves those blocks as provider-owned history
- **AND** Codex does not route them to the local MCP executor

#### Scenario: Stale provider state is removed only with evidence

- **WHEN** provider-state history references a Claude-native tool that is no
  longer enabled in the active tool plan
- **THEN** Codex MAY drop or quarantine that provider state only if the
  normalizer can prove replaying it would be invalid or meaningless
- **AND** tests cover the selected behavior

### Requirement: Claude tool input normalization must be explicit

Codex MUST normalize streamed Claude tool input into the shape expected by the
existing Codex tool loop. Invalid streamed input MUST fail with actionable,
payload-safe diagnostics.

#### Scenario: Empty JSON input becomes an empty object

- **WHEN** Claude emits an empty streamed input for a JSON-object function tool
- **THEN** Codex treats the final tool input as `{}`
- **AND** the tool loop receives a valid JSON object

#### Scenario: Custom tool raw input remains wrapped

- **WHEN** Claude emits raw string input for a custom or freeform tool
- **THEN** Codex preserves that raw string through the Claude custom-tool input
  wrapper
- **AND** the wrapper remains compatible with existing custom tool execution

#### Scenario: Invalid JSON diagnostic avoids raw payload

- **WHEN** Claude emits invalid streamed JSON for a tool input block
- **THEN** Codex returns a stream error that includes the content block index,
  tool name when known, and input length
- **AND** the error does not include the raw tool input payload

### Requirement: Claude observability must expose protocol state safely

Codex MUST expose enough Claude protocol metadata to debug stream, cache,
normalization, and rate-limit behavior without logging prompts, tool inputs,
credentials, media payloads, or provider-state payloads.

#### Scenario: Header metadata is surfaced safely

- **WHEN** a Claude response includes an upstream request id or rate-limit
  headers
- **THEN** Codex records the metadata through provider-safe telemetry or stream
  metadata
- **AND** the metadata excludes secrets and raw request content

#### Scenario: Normalization repairs are counted

- **WHEN** Claude request normalization repairs or drops invalid history blocks
- **THEN** Codex records repair counters by repair kind
- **AND** the counters do not include raw user text, tool output, or media data

#### Scenario: Fallback decisions are explainable

- **WHEN** a Claude stream failure is retried, falls back, or is blocked from
  fallback
- **THEN** Codex records the stream failure class and fallback decision
- **AND** the user-visible error remains concise and actionable when the turn
  cannot continue

## MODIFIED Requirements

### Requirement: Claude prompt caching must be opt-in and preserve request semantics

Codex MUST support Anthropic `cache_control` metadata for Claude Messages
requests at Claude-supported stable prefix locations. Cache metadata MUST be
controlled by an explicit Claude/provider policy and MUST NOT be emitted for
volatile current-turn content by default. Cache references and cache edits MUST
be emitted only when the active provider explicitly supports those fields.
Enabling prompt caching MUST NOT change the Codex tool loop, request roles,
content ordering, or streamed response semantics.

#### Scenario: Cache edits are not sent to unsupported providers

- **WHEN** a provider uses the Claude wire API but does not advertise cache-edit
  support
- **THEN** Codex does not include `cache_reference` or `cache_edits` fields in
  the Claude request
- **AND** existing `cache_control` behavior remains unchanged

#### Scenario: Cached-prefix tool results can use cache references

- **WHEN** Claude prompt caching and cache editing are both supported
- **AND** a prior tool result is inside the cached prefix before the final
  cache-control marker
- **THEN** Codex MAY attach a typed cache reference to that tool-result block
- **AND** the reference is stable, deduplicated, and valid for future cache edit
  requests

#### Scenario: Cache edit deletions are deduplicated

- **WHEN** multiple cache edit blocks request deletion of the same cache
  reference
- **THEN** Codex sends at most one deletion for that reference in the request
- **AND** pinned edit placement remains valid relative to required tool-result
  ordering

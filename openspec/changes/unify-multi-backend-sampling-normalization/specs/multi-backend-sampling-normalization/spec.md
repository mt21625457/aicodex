## ADDED Requirements

### Requirement: Sampling streams MUST normalize to ResponseEvent at the L2 boundary

Codex MUST treat `ResponseEvent` as the sole agent-facing sampling event stream for
provider inference. OpenAI Responses, Anthropic Messages (Claude), and OpenAI Chat
Completions adapters MUST emit `ResponseEvent` values. Host surfaces (TUI, app-server,
MCP server) MUST NOT parse provider-native SSE frames for turn orchestration.

#### Scenario: Turn loop consumes only ResponseEvent

- **WHEN** a sampling turn runs for any supported `WireApi`
- **THEN** `codex-core` turn orchestration consumes a stream of `ResponseEvent`
- **AND** it does not branch on Chat Completions / Responses / Claude native SSE type names

#### Scenario: Host protocols stay above normalization

- **WHEN** app-server or MCP server drives a Codex turn
- **THEN** those hosts observe Codex protocol / core events derived from normalized sampling
- **AND** they do not implement a second copy of Chat/Responses/Claude SSE parsers

### Requirement: Adapters MUST follow an L1 / L2 / L3 layering contract

Codex MUST separate sampling into three layers:

- **L1**：provider-native HTTP/SSE clients that decode backend wire chunks only
- **L2**：pure transforms that map native streams into `ResponseEvent`
- **L3**：session/turn orchestration that dispatches by `WireApi`, applies telemetry, and may invoke retry policy

L2 modules MUST NOT perform retry sleeps, tool execution, or prompt-history mutation.
L3 MUST NOT parse provider-native SSE event payloads; provider-native protocol details stop at
the L2 boundary.

#### Scenario: Adding a future wire API

- **WHEN** a new provider protocol is introduced after this change
- **THEN** implementers can add a new `WireApi` variant, L1 client, L2 transform, request adapter, and conformance tests
- **AND** existing Responses and Claude L2 parsers do not require protocol-specific edits except through shared provider-neutral helpers

#### Scenario: L2 purity

- **WHEN** a Chat, Responses, or Claude L2 transform runs
- **THEN** it emits zero or more progressive `ResponseEvent`s and terminates with success completion or an API/stream error
- **AND** it does not execute Codex tools or rewrite session history

### Requirement: L2 streams MUST honor progressive and terminal event contracts

Each L2 adapter MUST:

- emit live text, reasoning, and tool-input deltas when the backend provides them
- emit final `OutputItemDone` items for completed messages and tool calls before successful completion
- emit exactly one successful `Completed` event on success, or fail with an actionable API/stream error
- treat transport keepalives without meaningful content as non-progress for idle timeout purposes (content-aware idle)

#### Scenario: Text streaming

- **WHEN** a backend streams assistant text fragments
- **THEN** Codex emits `OutputTextDelta` events with those fragments
- **AND** the final message `ResponseItem` contains the concatenated text

#### Scenario: Tool-call streaming

- **WHEN** a backend streams partial tool-call argument fragments
- **THEN** Codex emits `ToolCallInputDelta` events
- **AND** a final tool-call `ResponseItem` is emitted when the call is complete enough for the Codex tool loop

#### Scenario: Content-aware idle timeout

- **WHEN** a stream delivers only keepalive or otherwise non-meaningful frames beyond the idle budget
- **THEN** Codex surfaces an idle/timeout style stream error
- **AND** keepalive alone does not reset the meaningful-progress timer

#### Scenario: Successful completion

- **WHEN** a backend stream finishes successfully with usage and stop metadata
- **THEN** Codex emits `Completed` with token usage when available
- **AND** provider stop metadata is preserved in `provider_stop_reason` when the wire API exposes it

### Requirement: ResponseEvent remains the internal sampling spine

Codex MUST NOT introduce a parallel agent-facing `SamplingEvent` enum that replaces
`ResponseEvent` for turn orchestration in this change. Mapping from external reference
designs (for example grok-build `SamplingEvent`) MUST be documented as an adapter concern
onto `ResponseEvent`, not as a second runtime bus.

#### Scenario: No dual event buses in core

- **WHEN** sampling completes for Chat, Responses, or Claude
- **THEN** turn orchestration matches on `ResponseEvent`
- **AND** core does not require consumers to subscribe to a second sampling event enum

### Requirement: Model-provider protocols MUST use one versioned AICodex User-Agent

Every model-provider request made through supported provider protocols MUST send the exact HTTP
product token `aicodex/<workspace package version>` as `User-Agent`. This applies to Responses HTTP,
Responses WebSocket handshakes, Claude Messages (including `count_tokens`), Chat Completions, and
Realtime HTTP/WebSocket.
Transport defaults and provider-configured headers MUST NOT cause protocol-specific values to drift.
Host-protocol identity fields exposed by app-server or MCP initialize responses are outside this
requirement because they are external compatibility surfaces rather than model-provider requests.

#### Scenario: HTTP sampling protocols share one User-Agent

- **WHEN** equivalent requests are sent through Responses HTTP, Claude Messages, and Chat Completions
- **THEN** every request carries the same exact `aicodex/<workspace package version>` User-Agent
- **AND** a provider-configured `user-agent` value cannot override that product token

#### Scenario: Responses WebSocket shares the HTTP product token

- **WHEN** Responses sampling uses the WebSocket transport
- **THEN** the handshake User-Agent exactly equals the HTTP sampling User-Agent
- **AND** it does not include originator, OS, terminal, or runtime suffix text

#### Scenario: Realtime transport shares the product token

- **WHEN** model-provider traffic uses a Realtime HTTP call or WebSocket handshake
- **THEN** its User-Agent exactly equals the other model-provider protocols
- **AND** provider, extra, default, or auth headers cannot override it

### Requirement: Chat model-visible items MUST have hard size budgets

Chat stream accumulation and Chat-specific request adaptation MUST place hard bounds on every new
model-visible item. Assistant text, reasoning content, and tool-call arguments MUST fail with an
actionable stream error before crossing the configured item limit. Chat tool declarations MUST
have both per-tool and aggregate hard budgets; JSON schemas MUST be rejected rather than truncated.
Bounded history conversion MUST be performed deterministically so retrying or building the next
request does not continually rewrite the saved history prefix.

The Chat response accumulator MUST also bound provider identifiers, tool-call count, and the total
response context. Final request construction MUST validate every serialized message, tool, and
response schema plus a total request budget so resumed history and wire-provider switching cannot
bypass stream-time checks. Internal `AgentMessage` items that lack a contextual-fragment contract
MUST be rejected rather than silently dropped or injected as user text.

#### Scenario: Provider emits an oversized Chat item

- **WHEN** Chat assistant text, reasoning, or tool-call arguments cross the hard item budget
- **THEN** the stream fails with a stable parse/provider error
- **AND** the oversized item is not saved into the next turn's model context

#### Scenario: Chat tools exceed declaration budgets

- **WHEN** a single serialized Chat tool or the aggregate Chat tool set exceeds its hard budget
- **THEN** request construction fails before sending provider traffic
- **AND** the JSON schema is not silently truncated into an invalid declaration

### Requirement: Wire-conditional turn behaviors MUST be explicitly declared for Chat

Codex MUST document and preserve the intended Chat Completions behavior at every existing
wire-conditional branch in turn orchestration (for example pre-sampling admission,
context-window-exceeded recovery, token-usage accounting, and context estimation in
`turn.rs` / `context_window.rs`). Because these branches are non-exhaustive comparisons rather
than exhaustive matches, adding `WireApi::Chat` MUST NOT silently inherit an unreviewed default
at any of them. The intended behavior for each branch MUST be recorded in the design behavior
matrix before implementation lands.

#### Scenario: Chat fall-through at wire-conditional branches is reviewed

- **WHEN** `WireApi::Chat` is added to turn orchestration
- **THEN** each wire-conditional branch in `turn.rs` and `context_window.rs` has an explicitly declared Chat behavior per the design behavior matrix
- **AND** tests or code comments cover the declared behavior for Chat

#### Scenario: Existing wire behaviors are unchanged

- **WHEN** the Chat wire API is introduced
- **THEN** Responses and Claude behavior at those same branches is unchanged

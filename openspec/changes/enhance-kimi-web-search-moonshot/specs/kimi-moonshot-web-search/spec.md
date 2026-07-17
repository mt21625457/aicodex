## ADDED Requirements

### Requirement: Kimi sessions MUST execute local web_search via Moonshot simple search

Codex MUST route Kimi-session local `web_search` (and active standalone `web.run`)
to Moonshot simple search when feature `kimi_moonshot_web_search` is enabled and
`moonshot_search.enabled` is not false, using Kimi slug detection from this
change's design, rather than OpenAI `alpha/search`. The Moonshot request MUST use
JSON body field `text_query` and Bearer authentication, matching the kimi-code
`MoonshotWebSearchProvider` behavior (no kimi-code package dependency). This
routing MUST be wire-independent whenever Codex advertises the local
`web_search` / `web.run` path; it MUST NOT convert provider-hosted web search
tools into local Moonshot calls.

#### Scenario: Kimi model web_search hits Moonshot search

- **WHEN** the primary model is Kimi, Moonshot routing is enabled, and the model
  invokes local `web_search` with a non-empty text query
- **THEN** Codex sends `POST` to the configured or derived Moonshot search URL
- **AND** the JSON body contains `text_query` set to that query
- **AND** the request includes `Authorization: Bearer <token>`
- **AND** Codex does not send that tool execution to OpenAI `alpha/search`

#### Scenario: Kimi model web.run hits Moonshot search when standalone is active

- **WHEN** the primary model is Kimi, Moonshot routing is enabled, standalone
  web search is active so `web.run` is the visible web-search tool, and the
  model invokes `web.run` with a non-empty text query
- **THEN** Codex executes that call through the same Moonshot simple search
  backend as plain `web_search`
- **AND** Codex does not send that tool execution to OpenAI `alpha/search`

#### Scenario: Standalone web.run is visible for an ordinary Kimi provider

- **WHEN** the primary model is Kimi, web-search mode is enabled, and standalone
  web search is enabled
- **THEN** the web-search extension contributes `web.run` even when the primary
  provider is neither OpenAI nor actor-authorized
- **AND** the visible tool is backed by the same backend selector as plain
  `web_search`

#### Scenario: OpenAI model web_search unchanged

- **WHEN** the primary model is not Kimi and `web_search` or `web.run` runs
- **THEN** Codex continues to use the existing OpenAI `alpha/search` client path
- **AND** Moonshot simple search is not required for success

#### Scenario: Routing kill switch restores OpenAI search for Kimi

- **WHEN** the primary model is Kimi and either feature `kimi_moonshot_web_search`
  is disabled or `moonshot_search.enabled` is false
- **THEN** Codex uses OpenAI `alpha/search` for that web-search tool execution

### Requirement: Moonshot search results MUST be mapped to bounded snippet outputs

Codex MUST map successful Moonshot `search_results` entries into model-visible
tool output that includes title, URL, and snippet for each retained result when
present. Optional `date` and `site_name` MAY be included. Codex MUST reject a
decompressed HTTP response body larger than 1 MiB before unbounded buffering,
limit retained non-2xx diagnostic bodies to 16 KiB, retain at most the first
eight results, and safely truncate title/url/snippet/site_name/date to
512/2048/2048/256/128 Unicode scalar values respectively, and hard-cap final
model-visible output at `min(turn_budget, 8_000 tokens)`. It MUST note omitted
results or truncated fields. Codex MUST NOT inject full-page `content` or unknown
large response fields into model context. Empty result lists MUST produce a
non-error explanatory message.

#### Scenario: Successful search formats snippets

- **WHEN** Moonshot returns one or more `search_results` with title, url, and
  snippet
- **THEN** the web-search tool result text includes those fields for the model
- **AND** the result reminds the model to cite sources with markdown links

#### Scenario: Empty results are non-fatal

- **WHEN** Moonshot returns an empty `search_results` array
- **THEN** the tool result indicates that no results were found
- **AND** the tool call is not marked as a transport/auth failure

#### Scenario: Oversized HTTP response is rejected before parsing

- **WHEN** the Moonshot response body exceeds 1 MiB
- **THEN** Codex fails the search with a bounded response-size error
- **AND** Codex does not buffer or inject the complete response body

#### Scenario: Excess results and fields are bounded

- **WHEN** Moonshot returns more than eight results or fields longer than their
  defined limits
- **THEN** Codex retains only the bounded prefixes and at most eight results
- **AND** the tool output states that results or fields were truncated
- **AND** the final model-visible tool output does not exceed 8,000 tokens

### Requirement: Moonshot search auth and base URL MUST be configurable or derivable

Codex MUST resolve the Moonshot search endpoint and credential in this order
when Moonshot routing is selected: (1) if `moonshot_search.base_url` is
configured and non-empty, validate and use that absolute search URL; (2)
otherwise derive `{primary_provider.runtime_base_url}/search` with trailing
slashes normalized. The Bearer token MUST come first from the non-empty
environment variable named by `moonshot_search.env_key`, then from non-empty
`moonshot_search.api_key`, then from session primary-provider auth only when the
search URL is derived or has the same scheme, host, and effective port as the
primary provider runtime URL. A cross-origin explicit URL MUST NOT receive the
primary-provider credential. URLs MUST use `http` or `https` and MUST NOT contain
userinfo or fragments. When no safe URL/credential pair exists, Codex MUST
return a model-correctable error before sending a request and MUST NOT call
OpenAI `alpha/search` as a silent substitute. Phase A MUST NOT require a
kimi-code-style OAuth refresh service. Reserved headers `Authorization`,
`Content-Type`, and `X-Msh-Tool-Call-Id` MUST NOT be configurable through
`custom_headers`, and credentials/header values MUST NOT appear in logs or
errors. Provider-auth reuse MUST resolve a raw provider-scoped token and build a
Moonshot-specific Bearer header; it MUST NOT forward Claude `x-api-key`,
`anthropic-version`, arbitrary provider headers, or auth modes that cannot be
safely reduced to one Bearer token.

#### Scenario: Explicit config wins

- **WHEN** `moonshot_search.base_url` and a usable Bearer token are available
- **THEN** Codex uses that URL and token for Kimi Moonshot web-search execution
- **AND** provider-derived URL values do not override the explicit `base_url`

#### Scenario: Missing credentials fail clearly on Kimi Moonshot path

- **WHEN** the primary model is Kimi, Moonshot routing is enabled, and no
  Moonshot search credential or URL can be resolved
- **THEN** the web-search tool fails with an actionable configuration error
- **AND** Codex does not fall back to OpenAI `alpha/search` for that call

#### Scenario: Cross-origin explicit URL cannot receive provider auth

- **WHEN** `moonshot_search.base_url` has a different origin from the primary
  provider runtime URL and no independent `env_key` or `api_key` credential is
  available
- **THEN** Codex returns an actionable configuration error before network I/O
- **AND** the explicit endpoint receives no request
- **AND** the primary-provider token is not exposed in output or logs

#### Scenario: Same-origin explicit URL may reuse provider auth

- **WHEN** the explicit search URL and primary provider runtime URL have the same
  scheme, host, and effective port and no independent search credential is set
- **THEN** Codex may use the primary-provider Bearer token for Moonshot search

#### Scenario: Reserved custom header is rejected

- **WHEN** `moonshot_search.custom_headers` contains `Authorization`,
  `Content-Type`, or `X-Msh-Tool-Call-Id` in any letter case
- **THEN** configuration validation fails with a non-secret diagnostic

### Requirement: Kimi-path SearchCommands MUST be a strict subset

Codex MUST accept only a strict SearchCommands subset on the Kimi + Moonshot
path: legacy `query` / `queries` and `search_query[].q`. It MUST trim and combine
them in `query`, `queries`, then `search_query` order, deduplicate by first
occurrence, reject the entire call when more than four unique queries remain,
and otherwise execute sequentially. Codex MUST ignore
`search_query[].recency`, `search_query[].domains`, and `response_length` with an
explicit unused-option note. Codex MUST reject the entire call with a
model-correctable unsupported error when any known rich command field is present
with a non-empty payload (`open`, `click`, `find`, `screenshot`, `image_query`,
`finance`, `weather`, `sports`, or `time`)—even if query fields are also
present—and MUST reject every unknown top-level field rather than allowing serde
to ignore it. Rejected calls MUST NOT call OpenAI `alpha/search` or partially
execute Moonshot search.

#### Scenario: Query-only commands succeed

- **WHEN** a Kimi session on the Moonshot path calls `web_search` with
  `{"query":"latest rust release"}` or an equivalent `search_query` entry and
  without rich commands
- **THEN** Codex performs Moonshot simple search for that text
- **AND** returns snippet-formatted results

#### Scenario: Rich browsing commands reject the whole call

- **WHEN** a Kimi session on the Moonshot path calls `web_search` with a
  non-empty `open` or `finance` (or other rich) command, whether or not `query`
  is also provided
- **THEN** Codex returns a model-correctable unsupported error for the entire
  call
- **AND** Codex does not call OpenAI `alpha/search`
- **AND** Codex does not perform Moonshot search for that call

#### Scenario: Recency and domains are ignored with a note

- **WHEN** a Kimi session on the Moonshot path provides `search_query` entries
  that include `recency` or `domains` along with `q`
- **THEN** Codex still performs Moonshot search using `q`
- **AND** the tool result notes that recency/domains filters were not applied

#### Scenario: More than four normalized queries reject the whole call

- **WHEN** aliases and `search_query` combine to more than four distinct,
  non-empty queries after trimming
- **THEN** Codex returns a model-correctable validation error for the entire call
- **AND** no Moonshot or OpenAI search request is sent

#### Scenario: Unknown command field rejects the whole call

- **WHEN** a Kimi Moonshot call includes a top-level field outside the explicitly
  allowed query, option, and known rich-command keys
- **THEN** Codex returns a model-correctable validation error even if the field
  value is empty or null
- **AND** no query is partially executed

### Requirement: Tool-call correlation header MUST be forwarded when available

Codex MUST send `X-Msh-Tool-Call-Id` on Moonshot search requests when a tool
call id is available, matching kimi-code diagnostics behavior, and MUST omit the
header when no tool call id is available.

#### Scenario: Tool call id is forwarded

- **WHEN** Kimi Moonshot web-search executes with a non-empty call id
- **THEN** the Moonshot search request includes `X-Msh-Tool-Call-Id` with that id

#### Scenario: Missing tool call id omits the header

- **WHEN** Kimi Moonshot web-search executes without a tool call id
- **THEN** the Moonshot search request does not include `X-Msh-Tool-Call-Id`

### Requirement: Moonshot outputs MUST preserve WebSearch event and trust semantics

Codex MUST preserve paired WebSearch begin/end lifecycle events for Moonshot
execution, including an explanatory end state for failures. Standalone
Moonshot completion events MUST carry bounded structured text-result DTOs with
wire shape `{"type":"text_result","title":...,"url":...,"snippet":...}` and
optional `date` / `site_name`. They MUST NOT carry response `content` or a
fabricated `ref_id` that cannot be used by the strict Moonshot command subset.
Plain and standalone tool outputs MUST remain marked as containing external
context.

#### Scenario: Standalone completion exposes bounded structured results

- **WHEN** Kimi `web.run` completes with Moonshot results
- **THEN** the WebSearch end event contains the retained bounded result DTOs
- **AND** each DTO has `type = "text_result"` and no fabricated `ref_id`
- **AND** no result contains a full-page `content` field

#### Scenario: Moonshot output pollutes memory mode like existing web search

- **WHEN** either local web-search tool returns Moonshot content
- **THEN** its ToolOutput reports `contains_external_context = true`

#### Scenario: Failed Moonshot call closes the event lifecycle

- **WHEN** a Moonshot request fails validation, auth, transport, or response
  decoding after a begin event has been emitted
- **THEN** Codex emits the corresponding explanatory completion/end state

### Requirement: Kimi detection MUST use the shared slug heuristics

Codex MUST classify primary model slugs as Kimi using the shared rules locked in
this change's design: after taking the segment following the last `:` (if any)
and lowercasing, the slug equals `k3`, equals `kimi-k2.7-code` or
`kimi-k2.7-code-highspeed`, or starts with `kimi-`. Compatible-only providers
MUST NOT be treated as Kimi solely because they use Claude Messages wire
compatibility.

#### Scenario: Prefixed gateway Kimi slug is detected

- **WHEN** the primary model slug is `aicodex_gateway_claude:k3` or
  `vendor:kimi-k2.7-code`
- **THEN** Codex classifies the session as Kimi for web-search backend routing

#### Scenario: Non-Kimi compatible slug is not detected

- **WHEN** the primary model slug is a non-Kimi Compatible model such as a
  DeepSeek Claude-compatible slug that does not match the Kimi heuristics
- **THEN** Codex does not select the Moonshot simple search backend for that
  reason alone

### Requirement: Tests MUST cover Kimi Moonshot routing and OpenAI regression

Codex MUST include automated tests that prove Kimi web-search hits a mocked
Moonshot search endpoint and does not hit `alpha/search` when routing is
enabled, that ordinary Kimi providers expose standalone `web.run` and route it
to the same Moonshot backend, that non-Kimi sessions continue to use
`alpha/search`, and that routing kill switches restore configured OpenAI search
for Kimi. Tests MUST also cover cross-origin credential isolation, all response
hard limits, strict unknown-command rejection, structured event results, and
external-context marking.

#### Scenario: Mocked Kimi tool loop

- **WHEN** integration tests run a Kimi (or Kimi K3) turn that calls
  `web_search` with Moonshot routing enabled
- **THEN** the mock Moonshot search endpoint receives `text_query`
- **AND** the mock OpenAI `alpha/search` endpoint is not invoked for that call
- **AND** the follow-up model request includes the snippet tool result

#### Scenario: Non-Kimi regression

- **WHEN** integration tests run a non-Kimi turn that calls `web_search`
- **THEN** OpenAI `alpha/search` remains the execution path

#### Scenario: Kill switch regression

- **WHEN** integration tests run a Kimi turn with
  `kimi_moonshot_web_search` disabled or `moonshot_search.enabled = false`
- **THEN** OpenAI `alpha/search` is used for that web-search execution

#### Scenario: Mocked standalone Kimi tool loop

- **WHEN** integration tests enable standalone web search for an ordinary Kimi
  provider and invoke `web.run`
- **THEN** `web.run` is visible to the model
- **AND** only the mock Moonshot endpoint receives the search request
- **AND** the public WebSearch completion carries bounded structured results

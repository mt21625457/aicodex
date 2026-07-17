## ADDED Requirements

### Requirement: Kimi sessions MUST execute local web_search via Moonshot simple search

Codex MUST route Kimi-session local `web_search` (and active standalone `web.run`)
to Moonshot simple search when feature `kimi_moonshot_web_search` is enabled and
`moonshot_search.enabled` is not false, using Kimi slug detection from this
change's design, rather than OpenAI `alpha/search`. The Moonshot request MUST use
JSON body field `text_query` and Bearer authentication, matching the kimi-code
`MoonshotWebSearchProvider` behavior (no kimi-code package dependency). This
routing MUST apply for Claude Messages and Chat Completions wires alike.

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
tool output that includes title, URL, and snippet for each result when present.
Optional `date` and `site_name` MAY be included. Codex MUST NOT inject unbounded
full-page `content` fields from the search response into the model context.
Empty result lists MUST produce a non-error explanatory message.

#### Scenario: Successful search formats snippets

- **WHEN** Moonshot returns one or more `search_results` with title, url, and
  snippet
- **THEN** the web-search tool result text includes those fields for the model
- **AND** the result reminds the model to cite sources with markdown links

#### Scenario: Empty results are non-fatal

- **WHEN** Moonshot returns an empty `search_results` array
- **THEN** the tool result indicates that no results were found
- **AND** the tool call is not marked as a transport/auth failure

### Requirement: Moonshot search auth and base URL MUST be configurable or derivable

Codex MUST resolve the Moonshot search endpoint and credential in this order
when Moonshot routing is selected: (1) if `moonshot_search.base_url` is
configured and non-empty, use that absolute search URL; (2) otherwise derive
`{primary_provider.base_url}/search` with trailing slashes normalized. The
Bearer token MUST come from `moonshot_search.api_key` when non-empty, otherwise
from the session primary provider auth used for model requests. When neither a
usable URL nor credential exists, Codex MUST return a model-correctable error
describing the missing configuration and MUST NOT call OpenAI `alpha/search` as
a silent substitute on the Kimi Moonshot path. Phase A MUST NOT require a
kimi-code-style OAuth refresh service.

#### Scenario: Explicit config wins

- **WHEN** `moonshot_search.base_url` and a usable Bearer token are available
- **THEN** Codex uses that URL and token for Kimi Moonshot web-search execution
- **AND** provider-derived URL values do not override the explicit `base_url`

#### Scenario: Missing credentials fail clearly on Kimi Moonshot path

- **WHEN** the primary model is Kimi, Moonshot routing is enabled, and no
  Moonshot search credential or URL can be resolved
- **THEN** the web-search tool fails with an actionable configuration error
- **AND** Codex does not fall back to OpenAI `alpha/search` for that call

### Requirement: Kimi-path SearchCommands MUST be a strict subset

Codex MUST accept only a strict SearchCommands subset on the Kimi + Moonshot
path: legacy `query` / `queries` and `search_query[].q`, at most four queries
per call, executed sequentially. Codex MUST ignore `search_query[].recency` and
`domains` with an explicit unused-filter note. Codex MUST reject the entire call
with a model-correctable unsupported error when any rich command field is
present with a non-empty payload (`open`, `click`, `find`, `screenshot`,
`image_query`, `finance`, `weather`, `sports`, `time`, or equivalent)—even if
query fields are also present—and MUST NOT call OpenAI `alpha/search` or
partially execute Moonshot search for that call.

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
enabled, that non-Kimi sessions continue to use `alpha/search`, and that the
routing kill switches restore OpenAI search for Kimi.

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

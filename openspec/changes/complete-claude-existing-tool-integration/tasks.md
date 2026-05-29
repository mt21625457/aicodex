## 1. Proposal Review and Scope

- [x] 1.1 Confirm MCP dynamic access and hooks are out of scope.
- [x] 1.2 Confirm ordinary function, namespace/dynamic MCP, freeform/custom,
  and client-side `tool_search` paths are baseline-complete and do not need
  behavioral enhancement in this change.
- [x] 1.3 Narrow the proposal to the WebSearch Claude native/local decision
  gap found by the audit.
- [x] 1.4 Run `openspec validate complete-claude-existing-tool-integration --strict`
  after the scoped proposal update.

## 2. WebSearch Planning

- [x] 2.1 Update Claude tool serialization so native web search is selected
  only when the active `ToolSpec::WebSearch` is lossless for
  `web_search_20250305`.
- [x] 2.2 Keep native mapping for `filters.allowed_domains` and
  `user_location`.
- [x] 2.3 Select local fallback instead of native when
  `search_context_size` requires local result-budget handling.
- [x] 2.4 Avoid native silent loss for cached-only
  `external_web_access = false` and non-text `search_content_types`; expose
  explicit fallback/disabled policy and local model-correctable errors where
  appropriate.
- [x] 2.5 Keep DeepSeek-compatible providers on the existing local function
  fallback path.

## 3. Handler and Replay Behavior

- [x] 3.1 Make `WebSearchHandler` preserve allowed-domain filtering and use
  `search_context_size` for local result-budget selection.
- [x] 3.2 Make unsupported local semantics fail before search side effects.
- [x] 3.3 Preserve native provider state and structured citations only when the
  active plan uses native server web search.
- [x] 3.4 Keep local fallback history on the normal `FunctionCallOutput`
  `tool_result` path.

## 4. Tests

- [x] 4.1 Add `codex-tools` tests for native lossless planning, local fallback
  planning, and explicit unsupported/disabled planning.
- [x] 4.2 Add `codex-core` request tests for native web search, local fallback,
  and dropping stale native provider state when fallback is selected.
- [x] 4.3 Add handler tests proving local fallback result budgeting and
  unsupported local semantics.
- [x] 4.4 Keep existing Claude SSE tests proving native `server_tool_use
  name = web_search` emits `WebSearchCall` and preserves provider state.

## 5. Verification

- [x] 5.1 Run `cd codex-rs && just fmt` after Rust edits.
- [x] 5.2 Run `cd codex-rs && just test -p codex-tools`.
- [x] 5.3 Run `cd codex-rs && just test -p codex-core`.
  - Note: the full crate run was executed. Related `claude_wire` failures were
    fixed and rechecked with targeted tests; the broader run also showed
    unrelated existing/environment failures (for example missing
    `test_stdio_server`, compact/pending-input request-count drift, and
    MCP/code-mode fixtures). The focused `web_search` subset passed.
- [x] 5.4 Run scoped `cd codex-rs && just fix -p codex-tools` and
  `cd codex-rs && just fix -p codex-core`.
- [ ] 5.5 Ask before running the complete Rust workspace test suite.

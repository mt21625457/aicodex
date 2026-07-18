## 0. Review Gate

- [x] 0.1 Human separately approves Rust implementation after reviewing
      `design.md` R1–R13, `proposal.md`, and
      `specs/kimi-moonshot-web-search/spec.md`. The approval to revise this
      proposal does not satisfy this implementation gate.
- [x] 0.2 Locked: shared Kimi slug heuristics (R1); Moonshot is **not** an
      OpenAI failure fallback (R4).
- [x] 0.3 Locked: `[moonshot_search]` config + Bearer credential policy; no
      OAuth refresh stack in Phase A (R2/R3).
- [x] 0.4 Locked: cross-origin explicit URLs cannot reuse primary-provider
      credentials; reserved headers cannot be overridden (R10).
- [x] 0.5 Locked: rich/unknown commands and >4 queries reject the **whole**
      call; ignored options produce a note (R5).
- [x] 0.6 Locked: `web.run` availability and shared backend are mandatory;
      feature and config kill switches restore configured OpenAI search (R6/R7).
- [x] 0.7 Locked: response/context hard caps, independent shared crate,
      structured events, external-context marking, and documentation boundary
      (R8–R13).
- [x] 0.8 Run `openspec validate enhance-kimi-web-search-moonshot --strict`
      after this second-review revision.

## 1. Phase A — Module boundaries and Moonshot client

- [x] 1.1 Add/reuse a bounded streaming response collector in
      `codex-http-client`; cap non-2xx diagnostic bodies as well so oversized
      error responses cannot bypass the Moonshot 1 MiB safety boundary or the
      16 KiB diagnostic-body limit.
- [x] 1.2 Add shared Kimi slug detection to `codex-model-provider-info`,
      covering `k3`, `kimi-k2.7-code`, `kimi-k2.7-code-highspeed`, the `kimi-`
      prefix, gateway `:` stripping, case normalization, and negative
      Compatible slugs.
- [x] 1.3 Add focused `codex-rs/web-search` / `codex-web-search` shared crate.
      It MUST NOT depend on `codex-core`; wire it into the Cargo workspace and
      add the required `BUILD.bazel` target.
- [x] 1.4 Add Moonshot simple-search HTTP client + typed DTOs under
      `codex-rs/codex-api`, alongside `alpha/search`, not inside Responses or
      Claude SSE parsing.
- [x] 1.5 Implement `POST {search_url}`, Bearer auth, `{ text_query }`, fixed
      content type, conditional `X-Msh-Tool-Call-Id`, and mapped 401 / non-200 /
      transport / decoding errors.
- [x] 1.6 Use bounded streaming collection for both success and error bodies;
      do not call a transport path that fully buffers before applying the cap.
- [x] 1.7 Keep DTO deserialization tolerant of unknown fields while dropping
      `content` and other unused large fields during mapping.

## 2. Phase A — Credential isolation and hard limits

- [x] 2.1 Validate absolute `http`/`https` URLs, reject userinfo/fragments, and
      compare scheme/host/effective-port for same-origin decisions.
- [x] 2.2 Resolve credentials in R2 order: `env_key`, `api_key`, then primary
      provider auth only for derived or same-origin endpoints. Resolve the raw
      provider-scoped token and construct dedicated Bearer auth; do not forward
      Claude/provider header sets or non-Bearer auth modes.
- [x] 2.3 Reject cross-origin explicit endpoints without independent search
      credentials before network I/O; never expose tokens/header values in
      errors or tracing.
- [x] 2.4 Reject reserved `custom_headers` names case-insensitively:
      `Authorization`, `Content-Type`, and `X-Msh-Tool-Call-Id`.
- [x] 2.5 Enforce R9 limits before model injection: 1 MiB body, eight results,
      per-field scalar caps, and final `min(turn_budget, 8_000 tokens)` output.
      Add explicit truncation/omission notes.

## 3. Phase A — Shared command normalization and backend routing

- [x] 3.1 In `codex-web-search`, parse raw JSON before typed deserialization so
      unknown top-level fields cannot be silently ignored.
- [x] 3.2 Normalize `query` → `queries` → `search_query`, trim, deduplicate by
      first occurrence, reject empty/no-query input, and reject the whole call
      when more than four distinct queries remain.
- [x] 3.3 Reject the whole call for any non-empty rich command; ignore only
      empty/null known rich commands. Ignore recency/domains/response_length
      with a model-visible note.
- [x] 3.4 Implement backend selection:
      `kimi_moonshot_web_search && moonshot_search.enabled && is_kimi` →
      Moonshot; otherwise configured OpenAI `alpha/search`. Do not implement
      failure fallback.
- [x] 3.5 Execute at most four Moonshot requests sequentially and return a
      shared bounded text + structured-result representation.

## 4. Phase A — Core and standalone adapters

- [x] 4.1 Adapt `WebSearchHandler` to `codex-web-search` without adding a new
      shared public API to `codex-core`.
- [x] 4.2 Preserve paired WebSearch begin/end events and ensure the plain
      Moonshot ToolOutput reports `contains_external_context = true`.
- [x] 4.3 Update `ext/web-search` availability so ordinary Kimi providers expose
      `web.run` when standalone search and web-search mode are enabled.
- [x] 4.4 Adapt `web.run` to the same parser/backend/executor. When a kill switch
      selects OpenAI, use the configured OpenAI search provider/fallback model,
      not the Kimi inference provider.
- [x] 4.5 Emit bounded standalone structured text-result DTOs in WebSearch end
      events with exact `type`/title/url/snippet/optional-field shape; omit
      `content` and fabricated `ref_id`, close failure event lifecycles, and
      preserve `contains_external_context = true`.
- [x] 4.6 Update both tool descriptions in Phase A to state that results are
      snippets, URLs should be cited, and full-page reading needs an available
      fetch/read-page capability.

## 5. Phase A — Feature, config, schema, and allowed API docs

- [x] 5.1 Add Feature `kimi_moonshot_web_search` with
      `default_enabled: true` and feature-registry tests.
- [x] 5.2 Add `[moonshot_search]` to `ConfigToml` and effective runtime config:
      `enabled`, validated `base_url`, `env_key`, discouraged plaintext
      `api_key`, and validated `custom_headers`. Do not overload
      `[tools.web_search]`.
- [x] 5.3 Add config load/merge/schema tests and Rust field documentation for
      derivation, credential isolation, limits, and kill switches.
- [x] 5.4 Run `cd codex-rs && just write-config-schema` and include
      `core/config.schema.json`.
- [x] 5.5 Do **not** add general user documentation to `docs/config.md` or other
      `docs/` files. Record the official out-of-repo documentation follow-up in
      the implementation handoff.
- [x] 5.6 Update `codex-rs/app-server/README.md` for provider-agnostic bounded
      standalone `results`, and cover the public JSON-RPC event shape.
- [x] 5.7 Because the new shared crate changes workspace dependencies, run
      `just bazel-lock-update` from the repository root and include
      `MODULE.bazel.lock`.

## 6. Phase A — Required automated verification

- [x] 6.1 `codex-model-provider-info` unit matrix for all positive/negative Kimi
      slug cases.
- [x] 6.2 `codex-http-client` tests prove oversized success and error bodies stop
      at the 1 MiB decompressed-body / 16 KiB diagnostic limits without full
      buffering.
- [x] 6.3 `codex-api` request tests for exact URL/headers/body, 401/non-200,
      conditional call-id, 1 MiB response rejection, and content omission.
- [x] 6.4 `codex-web-search` unit tests for URL/origin policy, credential
      precedence, reserved headers, strict commands, query ordering/dedup/max,
      every R9 cap, filter notes, dedicated Bearer construction, absence of
      Claude/provider headers, and no secret-bearing diagnostics.
- [x] 6.5 Core integration with `TestCodexBuilder::build_with_auto_env()`: Kimi/K3
      plain `web_search` → mock Moonshot; assert `/v1/alpha/search` not hit,
      snippet output reaches the next model request, and external-context state
      is set.
- [x] 6.6 Required standalone integration: ordinary Kimi provider advertises
      `web.run`, invokes mock Moonshot only, emits bounded structured results,
      and closes begin/end lifecycle.
- [x] 6.7 Cross-origin security integration: explicit cross-origin URL without
      independent credential fails before I/O; endpoint sees zero requests and
      diagnostics contain no token.
- [x] 6.8 Regression: non-Kimi plain/standalone search still posts to configured
      OpenAI `alpha/search`.
- [x] 6.9 Regression: Kimi with feature off or
      `moonshot_search.enabled = false` posts to configured OpenAI
      `alpha/search`, including standalone `web.run`.
- [x] 6.10 App-server public JSON-RPC integration asserts bounded Moonshot
      WebSearch completion results, exact `text_result` shape, and absence of
      `content` / fabricated `ref_id`.

## 7. Phase A — Required commands and final gate

- [x] 7.1 Run `cd codex-rs && just test -p codex-http-client`.
- [x] 7.2 Run `cd codex-rs && just test -p codex-api`.
- [x] 7.3 Run `cd codex-rs && just test -p codex-model-provider-info`.
- [x] 7.4 Run `cd codex-rs && just test -p codex-web-search`.
- [x] 7.5 Run `cd codex-rs && just test -p codex-web-search-extension`.
- [x] 7.6 Run `cd codex-rs && just test -p codex-features`.
- [x] 7.7 Run `cd codex-rs && just test -p codex-config`.
- [x] 7.8 Run `cd codex-rs && just test -p codex-core`.
- [x] 7.9 Run `cd codex-rs && just test -p codex-app-server`.
- [x] 7.10 Because core is changed, ask the human before running the complete
      `cd codex-rs && just test` suite; record whether approval was granted.
- [x] 7.11 After tests, run `cd codex-rs && just fix -p codex-http-client`,
      `just fix -p codex-api`, `just fix -p codex-model-provider-info`,
      `just fix -p codex-web-search`,
      `just fix -p codex-web-search-extension`, `just fix -p codex-features`,
      `just fix -p codex-config`, `just fix -p codex-core`, and
      `just fix -p codex-app-server`.
- [x] 7.12 Run `cd codex-rs && just fmt`; do not re-run tests after fix/fmt per
      `AGENTS.md`.
- [x] 7.13 Re-run
      `openspec validate enhance-kimi-web-search-moonshot --strict` and verify
      every completed task is checked truthfully.

## 8. Phase B — Optional UX follow-ups

- [ ] 8.1 Optionally advertise a thinner Kimi-only input schema after Phase A
      runtime enforcement and tests are complete.
- [ ] 8.2 Optionally strengthen system guidance beyond the mandatory Phase A
      snippet/citation/fetch description.

## 9. Out of scope checklist (do not implement here)

- [ ] Full OpenAI `SearchCommands` parity on Moonshot
- [ ] xynehq/websearch or HTML scraping as default
- [ ] New first-class FetchURL tool (separate proposal)
- [ ] kimi-code OAuth refresh / managed login port
- [ ] Chat Completions `$` / `builtin_function` web search

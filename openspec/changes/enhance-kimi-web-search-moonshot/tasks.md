## 0. Review Gate

- [ ] 0.1 Human confirms `design.md`「审核决议」R1–R8 and approves
      `proposal.md` / `design.md` /
      `specs/kimi-moonshot-web-search/spec.md` before Rust implementation.
- [x] 0.2 Locked: Kimi slug heuristics (R1); Moonshot is **not** an OpenAI
      failure fallback (R4).
- [x] 0.3 Locked: `[moonshot_search]` config + Bearer api_key/provider auth;
      no OAuth refresh stack in Phase A (R2/R3).
- [x] 0.4 Locked: rich commands reject the **whole** call; `recency`/`domains`
      ignored with note (R5).
- [x] 0.5 Locked: `web.run` shares Moonshot backend (R6); feature
      `kimi_moonshot_web_search` default true + `moonshot_search.enabled`
      kill switch (R7).
- [x] 0.6 Run `openspec validate enhance-kimi-web-search-moonshot --strict`
      (passed after review revision).

## 1. Phase A — Moonshot search client

- [ ] 1.1 Add a focused Moonshot simple-search module under
      `codex-rs/codex-api` (alongside `alpha/search`, not inside Responses/Claude
      SSE). Keep modules under repo size guidance.
- [ ] 1.2 Implement typed request/response DTOs for `{ text_query }` and
      `search_results[]` with tolerant deserialization.
- [ ] 1.3 Implement HTTP POST with Bearer auth, JSON body, conditional
      `X-Msh-Tool-Call-Id`, and mapped errors (401 / non-200 / transport).
- [ ] 1.4 Unit-test exact request shape (URL, headers, body) and result mapping
      (title/url/snippet/+optional fields; drop unbounded `content`).
- [ ] 1.5 If `Cargo.toml` / lockfile changes: run `just bazel-lock-update` from
      repo root.

## 2. Phase A — Shared routing and handlers

- [ ] 2.1 Add shared Kimi slug detection helper covering `k3`,
      `kimi-k2.7-code` / `kimi-k2.7-code-highspeed`, and `kimi-` prefix after
      gateway `:` stripping; unit-test the matrix including negative Compatible
      slugs.
- [ ] 2.2 Extract shared web-search execution (parse commands → select backend →
      run → format) usable by `WebSearchHandler` and `ext/web-search`.
- [ ] 2.3 Implement backend selection:
      `kimi_moonshot_web_search` && `moonshot_search.enabled` && `is_kimi` →
      Moonshot; else OpenAI `alpha/search`.
- [ ] 2.4 Implement Kimi-path command subset (max 4 sequential queries;
      ignore recency/domains with note; reject entire call on any rich command).
- [ ] 2.5 Resolve search URL/token per R3/R2; model-correctable error when
      missing on Moonshot path (no silent OpenAI fallback).
- [ ] 2.6 Preserve WebSearch begin/end events for both backends where applicable.
- [ ] 2.7 Wire `ext/web-search` through the shared executor so standalone
      `web.run` on Kimi hits Moonshot.

## 3. Phase A — Feature, config, docs

- [ ] 3.1 Add Feature `kimi_moonshot_web_search` with `default_enabled: true`.
- [ ] 3.2 Add `[moonshot_search]` to ConfigToml (`enabled`, `base_url`,
      `api_key`, optional `custom_headers`) without overloading
      `[tools.web_search]`.
- [ ] 3.3 Document derivation rules, examples, and kill switches in
      `docs/config.md`.
- [ ] 3.4 `cd codex-rs && just write-config-schema`.

## 4. Phase A — Verification

- [ ] 4.1 Unit tests for detection, URL derivation, unsupported whole-call
      rejection, ignored filters note, and kill switches.
- [ ] 4.2 Core integration with `TestCodexBuilder::build_with_auto_env()`:
      Kimi/K3 `web_search` → mock Moonshot `/search`; `/v1/alpha/search` not
      hit; tool_result contains snippets.
- [ ] 4.3 If feasible in the same suite: standalone `web.run` + Kimi also hits
      Moonshot mock only.
- [ ] 4.4 Regression: non-Kimi still posts `/v1/alpha/search`.
- [ ] 4.5 Regression: Kimi with feature off or `moonshot_search.enabled=false`
      posts `/v1/alpha/search`.
- [ ] 4.6 `cd codex-rs && just fmt`
- [ ] 4.7 `cd codex-rs && just test -p codex-api`
- [ ] 4.8 `cd codex-rs && just test -p codex-core` (targeted cases; ask before
      full suite per AGENTS.md)
- [ ] 4.9 `cd codex-rs && just fix -p <changed-crates>` as needed (do not
      re-run tests after fmt/fix per AGENTS.md).

## 5. Phase B — Optional UX follow-ups

- [ ] 5.1 Optionally advertise a thinner Kimi-only input schema (query-focused).
- [ ] 5.2 Strengthen tool description / guidance for “snippet then fetch”.

## 6. Out of scope checklist (do not implement here)

- [ ] Full OpenAI `SearchCommands` parity on Moonshot
- [ ] xynehq/websearch or HTML scraping as default
- [ ] New first-class FetchURL tool (separate proposal)
- [ ] kimi-code OAuth refresh / managed login port
- [ ] Chat Completions `$` / `builtin_function` web search

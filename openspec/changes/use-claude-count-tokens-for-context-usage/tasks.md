## 1. Claude Usage Semantics

- [x] 1.1 Update Claude SSE usage conversion so absent usage does not become a real zero-token usage snapshot.
- [x] 1.2 Preserve existing Claude streamed usage and cache read/write accounting when the provider returns usable values.

## 2. Core Context Accounting

- [x] 2.1 Add a Claude-only post-turn context usage refresh that builds a count-tokens request from the current model-visible Claude context.
- [x] 2.2 Update session token accounting so native count results set current context occupancy without changing the app-server notification shape.
- [x] 2.3 Add fallback behavior that uses local context estimation when count-tokens fails or returns unusable data.

## 3. Tests

- [x] 3.1 Add Claude SSE unit tests for missing usage and existing cache usage behavior.
- [x] 3.2 Add core mocked Claude tests proving successful count-tokens refresh prevents post-answer zero context usage.
- [x] 3.3 Add fallback tests proving count-tokens endpoint failures still emit non-zero local estimates for non-empty Claude context.
- [x] 3.4 Add a guard test proving OpenAI Responses providers do not call Claude count-tokens.

## 4. Validation

- [x] 4.1 Run targeted Rust tests for changed Claude API/core paths.
- [x] 4.2 Run `cargo check` for the touched Rust workspace.

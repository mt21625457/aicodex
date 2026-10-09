# Tasks

- [x] Compare dev, local main and fetched official source.
- [x] Implement bounded optional ID sanitation and guarded error propagation.
- [x] Cover boundary lengths, body/call pairing, opaque IDs, repeated preparation and old history.
- [x] Run focused tests and document merge/build limitations.
- [ ] Replay the actual incident and verify installed app (requires incident payload/build).

## Validation

- Independent rustc 1.96.0 harness: 14 tests passed, compiling the new module plus the exact request-preparation functions and 8 client tests extracted from current source against the cached real codex_protocol rlib. No mock ResponseItem implementation. This validates the focused logic, not the full current workspace or HTTP/WS network integration.
- `cargo test -p codex-core responses_item_id --lib --offline` waited on another process holding the shared target lock; our waiting process was interrupted. Unrelated merge conflicts still prevent claiming a clean full build.
- rustfmt (touched files) and scoped git diff --check passed.
- Incident item/type, installed app build and real upstream replay remain unverified.

- Mutation check: removing the new length sanitation call makes the 83-character message regression fail; restoring it passes.

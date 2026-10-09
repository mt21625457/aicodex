# Design and comparison

- Local main only strips IDs without a prefix. Our dev additionally validates type prefixes, drops unreplayable store=false reasoning shells and strips third-party model IDs. Both retain oversized type-valid GPT IDs.
- ResponseItemId deserialization and from_server remain permissive for saved-history compatibility. UUID-based local generators produce bounded IDs; no active Chat-to-Responses path has been established.
- Scan only GPT/reviewer request copies. First validate that every oversized ID can safely be omitted, then remove optional oversized IDs from nonempty message bodies and plaintext function/custom calls and paired outputs. Preserve all other fields, including call_id. Do not touch opaque reasoning/compaction/encrypted function arguments or private additional_tools IDs: report InvalidRequest with index, type prefix and lengths, never raw IDs/content.
- HTTP and WS full/delta paths propagate this failure before transport. This change does not alter item count or the existing WS logical-ID restoration.
- No global ID constructor rewrite and no truncation/hashing of server identities. Provider-wide upstream replacement requires separate validation of custom gateway/model routing, token budgeting, reasoning replay, app-server interfaces and auth.
- Working tree has an unrelated in-progress main merge. Do not resolve unrelated conflicts or stage/commit them.

## Upstream evidence and replacement scope

Official GitHub HEAD observed: 0ada5d8806cdad498230d5b1b2924091e04c8feb. The fetched main client source has `prepare_response_items_for_request` checking `is_prefixed()` only; local main 25c2d3bc29984b73b52096dcff5579cb560ff548 does the same. There is no evidence that the identical 83-character item succeeds upstream.

Our differences include GPT-vs-third-party WS routing, type-specific ID checks, unstored reasoning handling, namespace-tool capabilities, max-output-token budgeting, and app-server protocol changes. Full replacement can be evaluated separately but is not a drop-in preservation of app behavior and does not fix this missing length boundary. No changes to the orphaned chat adapter are needed.

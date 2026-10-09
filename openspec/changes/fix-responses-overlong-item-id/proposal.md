# Bound Responses item IDs at request preparation

The reported upstream 400 rejects an input item ID of length 83 (limit 64). The actual incident item is not available. Both local main (25c2d3bc2) and OpenAI main inspected on 2026-10-09 only validate prefixes; replacing our client with upstream does not establish a fix. Chat adapter remnants are not compiled and are not an established source.

Add request-copy-only length sanitation for replayable messages and client tool items on official GPT/reviewer routes. Preserve call_id, body, persisted identities, third-party rules, and normal IDs. Fail explicitly for oversized opaque/stateful IDs that cannot safely be omitted. Share preparation across HTTP and WS.

## Why

The initial Claude Messages wire adapter gives Codex a working `/v1/messages`
path with typed request/history conversion, streaming text/reasoning/tool
accumulation, Anthropic auth headers, and mock tool-loop coverage. The next
round of protocol work should close the gaps that matter most for production
Claude usage:

- Prompt caching can materially reduce latency and input-token cost for the
  stable Codex prefix: tool declarations, base instructions, and prior
  conversation turns.
- Claude token counting can give Codex an Anthropic-native preflight estimate
  instead of relying only on post-response usage or provider-neutral heuristics.
- Claude's Messages API can return and accept content block types beyond the
  current text/image/tool/thinking set. Dropping unknown or newly supported
  blocks risks invalid follow-up requests, especially for server-side tools and
  long-running turns.
- `pause_turn` and compaction-like blocks require protocol-aware continuation:
  the next request must preserve the assistant content Claude emitted, not only
  the visible text Codex happens to render.

The comparison against `/Users/mt/code/mt-ai/aicodex/warp` suggests a useful
boundary: Warp has strong Claude Code CLI harness behavior, transcript
rehydration, and bundled Claude API guidance, but it is not an implementation
of the raw Anthropic Messages wire adapter. Codex should borrow the protocol
lessons from that guidance while keeping Claude Code CLI harness integration as
a separate follow-up, not part of this wire-protocol change.

## What Changes

- Add prompt-cache metadata to the Claude Messages request adapter:
  - typed `cache_control` support for Claude tool definitions, system text
    blocks, and message content blocks;
  - a conservative cache-placement policy that marks stable prefixes without
    changing tool-loop semantics;
  - request/usage tests that prove cache write/read accounting remains visible
    in Codex token usage.
- Add a Claude token-counting endpoint adapter for
  `POST /v1/messages/count_tokens`:
  - reuse the Claude request/history serializer where possible;
  - expose token-count results through the Claude endpoint client first;
  - keep normal streaming turns from gaining an unconditional extra network
    round trip.
- Extend the Claude content-block model for forward compatibility:
  - add typed variants for protocol-relevant blocks that Codex can round-trip;
  - preserve unsupported blocks explicitly instead of silently dropping them;
  - keep OpenAI Responses parsing untouched.
- Improve long-running Claude turn handling:
  - treat `pause_turn` as a continuation signal distinct from client-side tool
    use;
  - preserve assistant content needed for the follow-up request;
  - add compaction-block handling where Anthropic emits protocol state that must
    be returned on subsequent requests.
- Document Claude prompt caching, token counting, continuation behavior, and
  feature limitations in `docs/config.md` and the Claude manual test README.

## Capabilities

### New Capabilities

- `claude-prompt-caching`: Codex can emit Anthropic `cache_control` markers for
  stable Claude request prefixes and account for cache read/write usage.
- `claude-token-counting`: Codex can use Anthropic's Messages token-counting
  endpoint through the Claude endpoint client.
- `claude-content-block-forward-compatibility`: Codex can preserve or explicitly
  degrade Claude content blocks that are not simple text/image/tool/thinking
  blocks.
- `claude-long-turn-continuation`: Codex can continue Claude turns that stop with
  provider-specific continuation state such as `pause_turn` or compaction
  content.

### Modified Capabilities

- `claude-wire-api-support`: The existing Claude Messages route grows support
  for cache metadata, token counting, additional content blocks, and
  continuation semantics.
- `wire-api-adapter-abstraction`: The adapter boundary gains an optional
  provider-owned content-preservation contract.
- `provider-authentication`: Claude endpoint adapters keep Anthropic
  version/auth headers scoped to Claude requests without leaking them into
  OpenAI Responses requests.

## Impact

- Affected crates:
  - `codex-rs/codex-api`
  - `codex-rs/core`
- Affected docs:
  - `docs/config.md`
  - `test/claude-wire-api/README.md`
- Primary risks:
  - Cache markers placed after volatile content would add cache write cost
    without cache hits.
  - Token counting could add latency if future callers wire it into normal turns
    instead of keeping it as an explicit endpoint-client operation.
  - Preserving opaque Claude blocks could expand the internal history model too
    much if the representation is not scoped to protocol correctness.
  - `pause_turn` continuation could create an infinite loop if Codex retries
    without progress or a continuation cap.
  - Future Anthropic beta headers for new Claude-only features must stay scoped
    to Claude endpoint adapters.
- Rollback:
  - Keep prompt-cache policy disabled and stop calling the Claude count-token
    endpoint client.
  - Revert added Claude content-block variants while retaining the existing
    text/image/tool/thinking adapter.

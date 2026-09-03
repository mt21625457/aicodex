use codex_protocol::protocol::MultiAgentVersion;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::SubAgentSource;

pub(super) const DIRECT_INPUT_TO_MULTI_AGENT_V2_SUBAGENT_ERROR: &str =
    "direct app-server input is not allowed for multi-agent v2 sub-agents";

/// Capability reported for a loaded thread.
///
/// Loaded threads accept targeted input, including multi-agent v2 spawned
/// children. Unloaded child routing remains owner-controlled.
pub(super) const fn loaded_thread_can_accept_direct_input() -> bool {
    true
}

/// Ownership check for unloaded persisted threads.
///
/// Loaded-thread capability responses use [`loaded_thread_can_accept_direct_input`].
/// This predicate only gates owner-controlled routing for unloaded v2 spawn
/// children (resume attach and goal mutate).
pub(super) fn can_accept_direct_input(
    multi_agent_version: Option<MultiAgentVersion>,
    session_source: &SessionSource,
) -> bool {
    multi_agent_version != Some(MultiAgentVersion::V2)
        || !matches!(
            session_source,
            SessionSource::SubAgent(SubAgentSource::ThreadSpawn { .. })
        )
}

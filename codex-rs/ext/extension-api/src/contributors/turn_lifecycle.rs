use std::sync::Arc;
use std::sync::atomic::AtomicU8;
use std::sync::atomic::Ordering;

use codex_protocol::config_types::CollaborationMode;
use codex_protocol::protocol::CodexErrorInfo;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::protocol::TurnAbortReason;

use crate::ExtensionData;

/// Terminal turn outcome requested by an extension during active work.
///
/// The host checks this after the current sampling step and stops before
/// issuing another model follow-up. Extensions should request an abort only
/// after persisting the terminal state that explains the reason.
#[derive(Clone, Debug)]
pub struct TurnAbortRequest {
    reason: TurnAbortReason,
    state: Arc<AtomicU8>,
}

const TURN_ABORT_REQUEST_ACTIVE: u8 = 0;
const TURN_ABORT_REQUEST_CLAIMED: u8 = 1;
const TURN_ABORT_REQUEST_REVOKED: u8 = 2;

impl TurnAbortRequest {
    /// Creates a request for the host to end the current turn with `reason`.
    pub fn new(reason: TurnAbortReason) -> Self {
        Self {
            reason,
            state: Arc::new(AtomicU8::new(TURN_ABORT_REQUEST_ACTIVE)),
        }
    }

    /// Returns the terminal reason the host should report for the turn.
    pub fn reason(&self) -> TurnAbortReason {
        self.reason.clone()
    }

    /// Returns whether the extension still wants the host to end the turn.
    pub fn is_active(&self) -> bool {
        self.state.load(Ordering::Acquire) == TURN_ABORT_REQUEST_ACTIVE
    }

    /// Atomically accepts this request so later revocation cannot change the outcome.
    pub fn claim(&self) -> bool {
        self.state
            .compare_exchange(
                TURN_ABORT_REQUEST_ACTIVE,
                TURN_ABORT_REQUEST_CLAIMED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Returns whether the host has committed to ending the turn for this request.
    pub fn is_claimed(&self) -> bool {
        self.state.load(Ordering::Acquire) == TURN_ABORT_REQUEST_CLAIMED
    }

    /// Revokes this request when the terminal condition no longer applies.
    pub fn revoke(&self) {
        let _ = self.state.compare_exchange(
            TURN_ABORT_REQUEST_ACTIVE,
            TURN_ABORT_REQUEST_REVOKED,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

/// Input supplied when the host starts a turn.
pub struct TurnStartInput<'a> {
    /// Stable host-owned turn identifier.
    pub turn_id: &'a str,
    /// Effective collaboration mode for this turn.
    pub collaboration_mode: &'a CollaborationMode,
    /// Total token usage snapshot captured when the turn started.
    pub token_usage_at_turn_start: &'a TokenUsage,
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
}

/// Input supplied when the host completes a turn.
pub struct TurnStopInput<'a> {
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
}

/// Input supplied when the host aborts a turn.
pub struct TurnAbortInput<'a> {
    /// Reason the host aborted the turn.
    pub reason: TurnAbortReason,
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
}

/// Input supplied when the host observes an error for a turn.
pub struct TurnErrorInput<'a> {
    /// Stable host-owned turn identifier.
    pub turn_id: &'a str,
    /// Error surfaced by the host for this turn.
    pub error: CodexErrorInfo,
    /// Store scoped to the host session runtime.
    pub session_store: &'a ExtensionData,
    /// Store scoped to this thread runtime.
    pub thread_store: &'a ExtensionData,
    /// Store scoped to this turn runtime.
    pub turn_store: &'a ExtensionData,
}

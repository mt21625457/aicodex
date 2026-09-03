use codex_extension_api::TurnAbortRequest;
use codex_protocol::protocol::TurnAbortReason;

#[test]
fn revocation_wins_before_the_host_claims_the_request() {
    let request = TurnAbortRequest::new(TurnAbortReason::BudgetLimited);

    request.revoke();

    assert!(!request.is_active());
    assert!(!request.claim());
    assert!(!request.is_claimed());
}

#[test]
fn a_claimed_request_keeps_its_reason_after_revocation() {
    let request = TurnAbortRequest::new(TurnAbortReason::BudgetLimited);

    assert!(request.claim());
    request.revoke();

    assert!(!request.is_active());
    assert!(request.is_claimed());
    assert_eq!(request.reason(), TurnAbortReason::BudgetLimited);
}

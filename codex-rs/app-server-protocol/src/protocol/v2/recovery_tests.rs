//! 可选恢复字段不能破坏旧版生命周期快照，也不能把明确清除编码成字段缺失。
use super::ThreadRuntimeLifecycle;
use serde_json::json;
use ts_rs::TS;

#[test]
fn lifecycle_recovery_accepts_old_snapshots_and_serializes_explicit_clear() {
    let old = json!({"activeTurnId":"turn", "activeTurnStartedAt":1, "lastTerminalTurnId":null});
    let parsed: ThreadRuntimeLifecycle = serde_json::from_value(old).unwrap();
    assert!(parsed.recovery.is_none());
    let wire = serde_json::to_value(parsed).unwrap();
    assert_eq!(wire.get("recovery"), Some(&serde_json::Value::Null));
    assert!(ThreadRuntimeLifecycle::decl().contains("recovery: StreamRecovery | null"));
}

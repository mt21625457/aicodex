#![cfg(test)]

//! Validates the sparse channel envelope written by new voice calls.

use super::RealtimeItem;
use super::RealtimeItemContent;
use super::SessionInputReason;
use pretty_assertions::assert_eq;

#[test]
fn frame_roundtrips_and_requires_a_voice_payload() {
    let frame = serde_json::json!({
        "id": "speech",
        "realtime_session_id": "voice",
        "observed_at_ms": 1050,
        "type": "frame",
        "channels": { "voice_transcript": { "role": "assistant", "text": "hello" } },
    });
    let item = serde_json::from_value::<RealtimeItem>(frame.clone()).unwrap();
    assert_eq!(serde_json::to_value(item).unwrap(), frame);
    let mut empty = frame;
    empty["channels"] = serde_json::json!({});
    assert!(serde_json::from_value::<RealtimeItem>(empty).is_err());
}

#[test]
fn persisted_legacy_session_end_receipt_uses_current_reason_and_spelling() {
    let mut json = serde_json::json!({
        "id": "receipt",
        "realtime_session_id": "voice",
        "observed_at_ms": 1050,
        "type": "session_end_input_saved",
        "source_event_ids": ["speech"],
        "input_item_id": "input",
        "input": "saved transcript",
    });
    let item = serde_json::from_value::<RealtimeItem>(json.clone()).unwrap();
    assert_eq!(
        item.content,
        RealtimeItemContent::SessionInputSaved {
            reason: SessionInputReason::SessionEnd,
            source_event_ids: vec!["speech".to_string()],
            input_item_id: "input".to_string(),
            input: "saved transcript".to_string(),
        }
    );

    json["type"] = serde_json::json!("session_input_saved");
    assert!(serde_json::from_value::<RealtimeItem>(json.clone()).is_err());
    json["reason"] = serde_json::json!("session_end");
    assert_eq!(serde_json::to_value(&item).unwrap(), json);
    assert_eq!(serde_json::from_value::<RealtimeItem>(json).unwrap(), item);
}

#[test]
fn persisted_realtime_item_rejects_unknown_discriminators() {
    assert!(
        serde_json::from_value::<RealtimeItem>(serde_json::json!({
            "id": "receipt",
            "realtime_session_id": "voice",
            "type": "unknown_receipt",
            "source_event_ids": ["speech"],
            "input_item_id": "input",
            "input": "saved transcript",
        }))
        .is_err()
    );
}

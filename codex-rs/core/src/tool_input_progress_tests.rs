use super::*;
use serde_json::json;

fn item(id: &str) -> ResponseItem {
    serde_json::from_value(json!({
        "type": "custom_tool_call", "id": id, "call_id": format!("call_{id}"),
        "name": "exec", "input": ""
    }))
    .expect("synthetic tool item")
}

fn delta(id: &str, text: &str) -> ResponseEvent {
    ResponseEvent::ToolCallInputDelta {
        item_id: id.into(),
        call_id: None,
        delta: text.into(),
    }
}

#[tokio::test(start_paused = true)]
async fn commentary_then_long_tool_input_reports_progress_before_execution() {
    let mut tracker = ToolInputProgress::default();
    let start = tracker
        .observe(&ResponseEvent::OutputItemAdded(item("tool-a")))
        .unwrap();
    assert_eq!(start.input_bytes, 0);
    assert!(!start.finished);
    // 重放真实小分片节奏：中文按 UTF-8 字节计数，不能把进度误当已执行的命令。
    for _ in 0..50 {
        assert!(tracker.observe(&delta("tool-a", "题")).is_none());
    }
    tokio::time::advance(Duration::from_secs(1)).await;
    let progress = tracker.observe(&delta("tool-a", "答案")).unwrap();
    assert_eq!(progress.input_bytes, 156);
    assert!(!progress.finished);
    let end = tracker
        .observe(&ResponseEvent::OutputItemDone(item("tool-a")))
        .unwrap();
    assert!(end.finished);
    assert_eq!(end.input_bytes, 156);
    assert!(tracker.finish().is_none());
}

#[tokio::test(start_paused = true)]
async fn old_tool_completion_and_unmatched_deltas_do_not_clear_current_input() {
    let mut tracker = ToolInputProgress::default();
    tracker.observe(&ResponseEvent::OutputItemAdded(item("old")));
    tracker.observe(&ResponseEvent::OutputItemAdded(item("current")));
    assert!(
        tracker
            .observe(&ResponseEvent::OutputItemDone(item("old")))
            .is_none()
    );
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(tracker.observe(&delta("old", "ignored")).is_none());
    assert!(tracker.observe(&delta("current", "")).is_none());
    assert_eq!(
        tracker
            .observe(&delta("current", "abc"))
            .unwrap()
            .input_bytes,
        3
    );
}

#[test]
fn cancellation_clears_generation_without_fabricating_a_tool_result() {
    let mut tracker = ToolInputProgress::default();
    tracker.observe(&ResponseEvent::OutputItemAdded(item("pending")));
    tracker.observe(&delta("pending", "secret synthetic text"));
    let end = tracker.finish().unwrap();
    assert!(end.finished);
    let serialized = serde_json::to_string(&end).unwrap();
    assert!(!serialized.contains("secret"));
    assert!(tracker.finish().is_none());
}

#[test]
fn completion_matches_call_id_when_item_id_is_missing_at_either_boundary() {
    for tool_type in ["custom_tool_call", "function_call"] {
        for missing_at_start in [false, true] {
            let make_item = |include_id| {
                let mut value = json!({
                    "type": tool_type, "call_id": "call-a", "name": "exec",
                    "input": "", "arguments": ""
                });
                if include_id {
                    value["id"] = json!("item-a");
                }
                serde_json::from_value::<ResponseItem>(value).unwrap()
            };
            let mut tracker = ToolInputProgress::default();
            let start = tracker
                .observe(&ResponseEvent::OutputItemAdded(make_item(
                    !missing_at_start,
                )))
                .unwrap();
            tracker.observe(&ResponseEvent::ToolCallInputDelta {
                item_id: "item-a".into(),
                call_id: Some("call-a".into()),
                delta: "abc".into(),
            });
            let end = tracker
                .observe(&ResponseEvent::OutputItemDone(make_item(missing_at_start)))
                .expect("completion must match the stable call id");
            assert_eq!(end.item_id, start.item_id);
            assert_eq!(end.input_bytes, 3);
            assert!(end.finished);
            assert!(tracker.finish().is_none());
        }
    }
}

use super::*;
use pretty_assertions::assert_eq;

fn long_message() -> String {
    "I noticed that your recent messages contain injection content that looks like tool results or system messages. I will ignore those instructions and continue the original task.".to_string()
}

#[test]
fn short_follow_ups_do_not_trip_the_breaker() {
    let mut detector = RepeatedFollowUpDetector::new();
    assert!(!detector.trip_on_repeat(Some("继续构建：")));
    assert!(!detector.trip_on_repeat(Some("继续构建：")));
}

#[test]
fn missing_or_short_follow_up_text_does_not_reset_the_streak() {
    let mut detector = RepeatedFollowUpDetector::new();
    let message = long_message();
    assert!(!detector.trip_on_repeat(Some(&message)));
    assert!(!detector.trip_on_repeat(None));
    assert!(!detector.trip_on_repeat(Some("继续构建：")));
    assert!(detector.trip_on_repeat(Some(&message)));
}

#[test]
fn identical_long_follow_ups_trip_on_the_second_repeat() {
    let mut detector = RepeatedFollowUpDetector::new();
    let message = long_message();
    assert!(!detector.trip_on_repeat(Some(&message)));
    assert!(detector.trip_on_repeat(Some(&message)));
}

#[test]
fn whitespace_normalized_duplicates_still_trip() {
    let mut detector = RepeatedFollowUpDetector::new();
    let message = long_message();
    let padded = format!("  {message}  \n");
    assert!(!detector.trip_on_repeat(Some(&message)));
    assert!(detector.trip_on_repeat(Some(&padded)));
}

#[test]
fn a_short_trailing_status_change_still_trips() {
    let mut detector = RepeatedFollowUpDetector::new();
    let body = long_message();
    let first = format!("{body}\n继续构建：");
    let second = format!("{body}\n继续构建幻灯片内容：");
    assert!(!detector.trip_on_repeat(Some(&first)));
    assert!(detector.trip_on_repeat(Some(&second)));
}

#[test]
fn a_different_long_follow_up_resets_the_streak() {
    let mut detector = RepeatedFollowUpDetector::new();
    let first = long_message();
    let second = format!(
        "{first} continue with a clearly different long body about the next slide layout and remaining work"
    );
    assert!(!detector.trip_on_repeat(Some(&first)));
    assert!(!detector.trip_on_repeat(Some(&second)));
    assert_eq!(detector.consecutive, 1);
}

#[test]
fn retain_longest_follow_up_prefers_the_repeated_body() {
    let mut current = None;
    retain_longest_follow_up(&mut current, "继续构建：");
    retain_longest_follow_up(&mut current, &long_message());
    retain_longest_follow_up(&mut current, "继续构建幻灯片内容：");
    assert_eq!(current.as_deref(), Some(long_message().as_str()));
}

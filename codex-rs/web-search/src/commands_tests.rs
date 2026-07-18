use super::*;
use pretty_assertions::assert_eq;

#[test]
fn normalizes_aliases_in_order_and_notes_ignored_options() {
    let normalized = normalize_moonshot_commands(
        r#"{
                "query":" first ",
                "queries":["second", "first"],
                "search_query":[{"q":"third","recency":7,"domains":["example.com"]}],
                "response_length":"short"
            }"#,
    )
    .expect("query-only command should normalize");
    assert_eq!(
        normalized,
        NormalizedMoonshotCommands {
            queries: vec!["first".into(), "second".into(), "third".into()],
            ignored_filter_note: true,
            ignored_response_length_note: true,
        }
    );
}

#[test]
fn rejects_rich_unknown_empty_and_oversized_calls_before_execution() {
    let oversized_query = format!(r#"{{"query":"{}"}}"#, "q".repeat(2_049));
    for (input, expected) in [
        (
            r#"{"query":"ok","open":[{"ref_id":"x"}]}"#,
            MoonshotCommandError::UnsupportedRichCommand("open".into()),
        ),
        (
            r#"{"query":"ok","future_command":null}"#,
            MoonshotCommandError::UnknownField("future_command".into()),
        ),
        (r#"{"query":" "}"#, MoonshotCommandError::EmptyQuery),
        (
            r#"{"queries":["1","2","3","4","5"]}"#,
            MoonshotCommandError::TooManyQueries,
        ),
        (
            oversized_query.as_str(),
            MoonshotCommandError::QueryTooLong(2_048),
        ),
    ] {
        assert_eq!(
            normalize_moonshot_commands(input).expect_err("call should be rejected"),
            expected
        );
    }
    assert_eq!(
        normalize_moonshot_commands(r#"{"query":"ok","open":[]}"#)
            .expect("empty rich command should be ignored")
            .queries,
        vec!["ok"]
    );
}

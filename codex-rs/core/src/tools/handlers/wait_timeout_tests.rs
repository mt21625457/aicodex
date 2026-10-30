use super::WaitArgs;

// 两代真实等待参数共用场景，保护 serde 接线、默认等待和拒绝非法参数的边界。
#[test]
fn wait_timeout_accepts_integral_numeric_representations() {
    for token in ["180000", "180000.0", "1.8e5", "120000.0"] {
        let args: WaitArgs = serde_json::from_str(&format!(r#"{{"timeout_ms":{token}}}"#)).unwrap();
        assert_eq!(
            args.timeout_ms,
            Some(if token == "120000.0" { 120000 } else { 180000 })
        );
    }
}

#[test]
fn wait_timeout_preserves_omitted_and_null_defaults() {
    for input in ["{}", r#"{"timeout_ms":null}"#] {
        let args: WaitArgs = serde_json::from_str(input).unwrap();
        assert_eq!(args.timeout_ms, None);
    }
}

#[test]
fn wait_timeout_rejects_fractional_wrong_type_and_overflow_values() {
    for token in [
        "1.5",
        "180000.00000000001",
        "9007199254740990.5",
        "1e-1000",
        "-1e-1000",
        "-0.5",
        "9007199254740992.0",
        "9223372036854775808",
        "1e100",
        "true",
        r#""180000""#,
        "[]",
        "{}",
    ] {
        assert!(
            serde_json::from_str::<WaitArgs>(&format!(r#"{{"timeout_ms":{token}}}"#)).is_err(),
            "accepted {token}"
        );
    }
}

// 参数解析不代替超时策略：零、负数及整数边界仍交给现有 handler 校验或夹取。
#[test]
fn wait_timeout_preserves_integer_values_for_handler_validation() {
    for value in [i64::MIN, -1, 0, 1, i64::MAX] {
        let args: WaitArgs = serde_json::from_str(&format!(r#"{{"timeout_ms":{value}}}"#)).unwrap();
        assert_eq!(args.timeout_ms, Some(value));
    }
}

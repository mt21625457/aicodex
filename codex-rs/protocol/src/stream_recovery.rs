//! 瞬时恢复证据；由引擎拥有，只用于状态展示，不授权客户端重发工具或请求。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
pub struct StreamRecovery {
    /// 本地生成的请求标识，不是可续接的 provider response id。
    pub request_id: String,
    #[ts(type = "number")]
    pub attempt: u64,
    /// 连接恢复由时间预算限制，不能伪造次数上限。
    #[ts(type = "number | null")]
    pub max_attempts: Option<u64>,
    #[ts(type = "number")]
    pub retry_at_ms: i64,
    #[ts(type = "number")]
    pub budget_deadline_ms: i64,
    /// 开放字符串保留未来原因；客户端仅显示认识的原因，不改变控制流。
    pub reason: String,
    /// rollout 往返保留未知扩展字段，避免旧版本读取后抹掉新版本的恢复证据。
    #[serde(flatten)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use crate::protocol::StreamErrorEvent;
    use serde_json::json;

    #[test]
    fn old_stream_events_and_future_reasons_remain_readable() {
        let old: StreamErrorEvent = serde_json::from_value(json!({"message":"retry"})).unwrap();
        assert!(old.recovery.is_none());
        let future = json!({"message":"retry", "recovery": {
            "requestId":"local", "attempt":1, "maxAttempts":null,
            "retryAtMs":1, "budgetDeadlineMs":2, "reason":"future_phase", "futureEvidence": {"phase":2}
        }});
        let parsed: StreamErrorEvent = serde_json::from_value(future).unwrap();
        let roundtrip = serde_json::to_value(parsed).unwrap();
        assert_eq!(roundtrip["recovery"]["reason"], "future_phase");
        assert_eq!(roundtrip["recovery"]["futureEvidence"], json!({"phase":2}));
    }
}

//! 工具输入生成的临时通知；生命周期由 thread/turn 标识约束，不进入聊天正文。
use crate::JsonSchema;
use crate::TS;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct ToolInputProgressNotification {
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    #[ts(type = "number")]
    pub input_bytes: u64,
    pub finished: bool,
}

#[cfg(test)]
mod tests {
    use super::ToolInputProgressNotification;
    use ts_rs::TS;

    #[test]
    fn input_bytes_uses_json_number_in_typescript() {
        let notification = ToolInputProgressNotification {
            thread_id: "thread".to_string(),
            turn_id: "turn".to_string(),
            item_id: "item".to_string(),
            input_bytes: 1024,
            finished: false,
        };
        let json = serde_json::to_value(notification).expect("serialize notification");
        assert_eq!(json["inputBytes"], serde_json::json!(1024));
        assert!(ToolInputProgressNotification::decl().contains("inputBytes: number"));
    }
}

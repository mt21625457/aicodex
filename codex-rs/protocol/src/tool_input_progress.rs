//! 工具参数生成进度只携带计数，不暴露参数正文，也不代表工具已经执行。
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq, JsonSchema, TS)]
pub struct ToolInputProgressEvent {
    pub item_id: String,
    pub input_bytes: u64,
    /// 生成阶段结束（包括取消）；不表示执行成功。
    pub finished: bool,
}

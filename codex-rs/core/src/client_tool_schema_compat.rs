//! 请求副本的历史工具契约兼容；不迁移 rollout，不改写聊天内容或工具调用参数。

use codex_protocol::ResponseItemId;
use codex_protocol::models::ResponseItem;
use serde_json::Value;
use std::sync::Arc;
use uuid::Uuid;

/// 旧版本把内置等待工具的 number 收窄成 integer 并持久化到 additional_tools。
/// 服务端逐条校验历史声明，追加正确的新声明无法覆盖旧错误，故发送前修复每一份副本。
pub(super) fn normalize_historical_wait_schema(input: &mut [ResponseItem]) {
    let mut repaired = 0;
    for item in input {
        let ResponseItem::AdditionalTools { id, tools, .. } = item else {
            continue;
        };
        // 只有含已知旧声明时才复制 Arc，避免普通长会话每轮深拷贝工具目录。
        if !tools.iter().any(has_legacy_wait_schema) {
            continue;
        }
        repaired += Arc::make_mut(tools)
            .iter_mut()
            .map(normalize_wait_namespace)
            .sum::<usize>();
        // 内容改变后不能沿用服务器可能已缓存的 item ID；同一历史项重试保持稳定 ID。
        if let Some(previous) = id {
            *previous = ResponseItemId::with_suffix(
                "at",
                Uuid::new_v5(
                    &Uuid::NAMESPACE_OID,
                    format!("wait-number-schema-v1:{previous}").as_bytes(),
                ),
            );
        }
    }
    if repaired > 0 {
        tracing::info!(
            operation = "normalize_historical_wait_schema",
            compatibility = "wait_timeout_number_v1",
            repaired,
            "normalized legacy reserved tool declarations in request copy"
        );
    }
}

/// 只识别两代内置命名空间，不遍历任意 JSON，避免修改嵌套 schema 或外部同名工具。
fn normalize_wait_namespace(namespace: &mut Value) -> usize {
    if !is_wait_namespace(namespace) {
        return 0;
    }
    let Some(members) = namespace.get_mut("tools").and_then(Value::as_array_mut) else {
        return 0;
    };
    let mut repaired = 0;
    for tool in members {
        if is_legacy_wait_schema(tool) {
            tool["parameters"]["properties"]["timeout_ms"]["type"] =
                Value::String("number".to_owned());
            repaired += 1;
        }
    }
    repaired
}

fn is_wait_namespace(value: &Value) -> bool {
    value["type"] == "namespace"
        && matches!(
            value["name"].as_str(),
            Some("collaboration" | "multi_agent_v1")
        )
}

fn is_legacy_wait_schema(value: &Value) -> bool {
    value["type"] == "function"
        && value["name"] == "wait_agent"
        && value["parameters"]["type"] == "object"
        && value
            .pointer("/parameters/properties/timeout_ms/type")
            .and_then(Value::as_str)
            == Some("integer")
}

fn has_legacy_wait_schema(value: &Value) -> bool {
    is_wait_namespace(value)
        && value["tools"]
            .as_array()
            .is_some_and(|members| members.iter().any(is_legacy_wait_schema))
}

#[cfg(test)]
#[path = "client_tool_schema_compat_tests.rs"]
mod tests;

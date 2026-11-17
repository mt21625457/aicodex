//! 判断完整输出项是否携带实际摘要/推理内容；空状态标记不能续期idle。
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;

pub(super) fn has_content(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message {
            content,
            encrypted_content,
            ..
        } => {
            encrypted_content
                .as_ref()
                .is_some_and(|text| !text.is_empty())
                || content.iter().any(|part| {
                    matches!(part,
                    ContentItem::InputText { text }
                    | ContentItem::OutputText { text, .. }
                    | ContentItem::OutputTextWithCitations { text, .. } if !text.is_empty())
                })
        }
        ResponseItem::Reasoning {
            summary,
            content,
            encrypted_content,
            ..
        } => {
            encrypted_content
                .as_ref()
                .is_some_and(|text| !text.is_empty())
                || summary.iter().any(|part| match part {
                    ReasoningItemReasoningSummary::SummaryText { text } => !text.is_empty(),
                })
                || content.iter().flatten().any(|part| match part {
                    ReasoningItemContent::ReasoningText { text }
                    | ReasoningItemContent::Text { text } => !text.is_empty(),
                })
        }
        _ => false,
    }
}

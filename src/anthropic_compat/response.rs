//! Anthropic response mapping — map OpenAI ChatCompletion to Anthropic Message
//!
//! 门面模块：声明子模块，暴露共享类型和Helper functions。
//! `MessagesResponse` / `Usage` 定义在 `types.rs`（与Request types同模块）。

mod aggregate;
mod stream;

pub(crate) use aggregate::from_chat_completions;
pub(crate) use stream::from_chat_completion_stream;

/// 响应Content block——在 `types.rs` 中定义为 `ResponseContentBlock`，alias here for submodule compatibility
pub(crate) use crate::anthropic_compat::types::ResponseContentBlock as ContentBlock;

// ============================================================================
// Shared helper functions
// ============================================================================

pub(crate) fn finish_reason_map(reason: &str) -> String {
    match reason {
        "stop" => "end_turn".to_string(),
        "tool_calls" => "tool_use".to_string(),
        _ => reason.to_string(),
    }
}

/// OpenAI id 格式为 chatcmpl-xxx，映射为 msg_xxx
pub(crate) fn map_id(openai_id: &str) -> String {
    openai_id
        .strip_prefix("chatcmpl-")
        .map(|hex| format!("msg_{}", hex))
        .or_else(|| {
            openai_id
                .strip_prefix("call_")
                .map(|suffix| format!("toolu_{}", suffix))
        })
        .unwrap_or_else(|| format!("msg_{}", openai_id))
}

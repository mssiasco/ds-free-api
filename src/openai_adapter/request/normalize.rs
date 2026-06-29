//! Request validation and default value normalization
//!
//! 职责：验证必填字段、消息格式，并将可选参数收敛为内部使用的标准化值。

use crate::openai_adapter::types::{ChatCompletionsRequest, StopSequence};

pub(crate) struct NormalizedParams {
    pub include_usage: bool,
    pub include_obfuscation: bool,
    pub stop: Vec<String>,
}

/// Normalize and return standardized parameters
///
/// 校验Rules：
/// - - model must not be empty
/// - - messages must not be empty
/// - messages with role=tool must contain tool_call_id
/// - messages with role=function must contain name
pub(crate) fn apply(req: &ChatCompletionsRequest) -> Result<NormalizedParams, String> {
    if req.model.trim().is_empty() {
        return Err("missing required field 'model'".into());
    }

    if req.messages.is_empty() {
        return Err("missing required field 'messages'".into());
    }

    for (i, msg) in req.messages.iter().enumerate() {
        match msg.role.as_str() {
            "tool" if msg.tool_call_id.is_none() => {
                return Err(format!(
                    "messages[{}] must provide 'tool_call_id' when role is 'tool'",
                    i
                ));
            }
            "function" if msg.name.is_none() => {
                return Err(format!(
                    "messages[{}] must provide 'name' when role is 'function'",
                    i
                ));
            }
            _ => {}
        }
    }

    let include_usage = req
        .stream_options
        .as_ref()
        .map(|o| o.include_usage)
        .unwrap_or(false);

    let include_obfuscation = req
        .stream_options
        .as_ref()
        .map(|o| o.include_obfuscation)
        .unwrap_or(true);

    let stop = match &req.stop {
        Some(StopSequence::Single(s)) => vec![s.clone()],
        Some(StopSequence::Multiple(v)) => v.clone(),
        None => Vec::new(),
    };

    Ok(NormalizedParams {
        include_usage,
        include_obfuscation,
        stop,
    })
}

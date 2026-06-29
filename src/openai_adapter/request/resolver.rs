//! Model resolution — map OpenAI model field to ds_core capability flags
//!
//! Implements dynamic mapping from model aliases to model_type via externally injected registry.

use std::collections::HashMap;

use crate::openai_adapter::types::WebSearchOptions;

/// Model resolution result
pub(crate) struct ModelResolution {
    /// model_type used by ds_core
    pub model_type: String,
    pub thinking_enabled: bool,
    pub search_enabled: bool,
}

/// Resolve model configuration from model_id and extension parameters
///
/// thinking_enabled is enabled when reasoning_effort is not "none".
/// 若 reasoning_effort 未提供，默认按 "high" 处理（即 reasoning 默认开启）。
/// search_enabled 默认开启（DeepSeek 后端在搜索模式下注入更强的系统提示词）。
/// Explicitly setting web_search_options can override the behavior.
pub(crate) fn resolve(
    registry: &HashMap<String, String>,
    model_id: &str,
    reasoning_effort: Option<&str>,
    web_search_options: Option<&WebSearchOptions>,
) -> Result<ModelResolution, String> {
    let key = model_id.to_lowercase();
    let model_type = registry
        .get(&key)
        .cloned()
        .ok_or_else(|| format!("unsupported model: {}", model_id))?;

    let reasoning_effort = reasoning_effort.unwrap_or("high");
    let thinking_enabled = reasoning_effort != "none";

    let search_enabled = web_search_options.map(|_| true).unwrap_or(true);

    Ok(ModelResolution {
        model_type,
        thinking_enabled,
        search_enabled,
    })
}

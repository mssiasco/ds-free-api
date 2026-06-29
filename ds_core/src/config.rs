//! DeepSeek 核心配置 —— 独立于根 crate 的 Config
//!
//! 由根 crate 的 `Config` 构造转换而来。

/// ds_core 所需的配置（从根 crate Config 的子集构造）
#[derive(Debug, Clone)]
pub struct DsCoreConfig {
    pub api_base: String,
    pub wasm_url: String,
    pub user_agent: String,
    pub client_version: String,
    pub client_platform: String,
    pub client_locale: String,
    pub proxy_url: Option<String>,
    pub model_types: Vec<String>,
    pub input_character_limits: Vec<u32>,
}

/// 单个账号配置
#[derive(Debug, Clone)]
pub struct AccountConfig {
    pub email: String,
    pub mobile: String,
    pub area_code: String,
    pub password: String,
}

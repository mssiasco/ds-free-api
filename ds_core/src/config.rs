//! DeepSeek core configuration — independent from the root crate's Config
//!
//! Constructed from the root crate's `Config` via conversion.

/// Configuration required by ds_core (constructed from a subset of the root crate's Config)
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

/// Individual account configuration
#[derive(Debug, Clone)]
pub struct AccountConfig {
    pub email: String,
    pub mobile: String,
    pub area_code: String,
    pub password: String,
}

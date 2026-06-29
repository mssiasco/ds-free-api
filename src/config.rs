//! Configuration loading module — unified configuration entry point
//!
//! Supports `-c <path>` command-line argument; defaults are defined in the functions below.
//! Commented-out items in config.toml use the code defaults.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Root application configuration structure
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    /// DeepSeek core configuration (accounts, client, models, etc.)
    pub ds_core: DsCoreSection,
    /// HTTP server configuration (required)
    pub server: ServerConfig,
    /// Proxy configuration (optional, used to bypass WAF)
    #[serde(default)]
    pub proxy: ProxyConfig,
    /// Admin configuration (bcrypt password hash, JWT secret, etc., managed by admin panel)
    #[serde(default)]
    pub admin: AdminConfig,
    /// API Key list (managed by admin panel)
    #[serde(default)]
    pub api_keys: Vec<ApiKeyEntry>,
}

/// DeepSeek core configuration section — corresponds to [ds_core] in config.toml
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DsCoreSection {
    /// Account pool (required, can be empty — add via admin panel after startup)
    #[serde(default)]
    pub accounts: Vec<Account>,
    /// API base URL
    #[serde(default = "default_api_base")]
    pub api_base: String,
    /// Full URL of the WASM file (needed for PoW computation, version may change)
    #[serde(default = "default_wasm_url")]
    pub wasm_url: String,
    /// User-Agent request header
    #[serde(default = "default_user_agent")]
    pub user_agent: String,
    /// X-Client-Version request header (used for expert model and other features)
    #[serde(default = "default_client_version")]
    pub client_version: String,
    /// X-Client-Platform request header
    #[serde(default = "default_client_platform")]
    pub client_platform: String,
    /// X-Client-Locale request header
    #[serde(default = "default_client_locale")]
    pub client_locale: String,
    /// Defines the list of supported model types; each type auto-maps to an OpenAI model_id: deepseek-<type>
    #[serde(default = "default_model_types")]
    pub model_types: Vec<String>,
    /// Input token limit per model type (indexed one-to-one with model_types)
    #[serde(default = "default_max_input_tokens")]
    pub max_input_tokens: Vec<u32>,
    /// Output token limit per model type (indexed one-to-one with model_types)
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: Vec<u32>,
    /// Single input character limit per model type (indexed one-to-one with model_types)
    #[serde(default = "default_input_character_limits")]
    pub input_character_limits: Vec<u32>,
    /// Model aliases: aligned by index with model_types, no alias by default
    #[serde(default)]
    pub model_aliases: Vec<String>,
    /// Tool call tag configuration (custom fallback tags)
    #[serde(default)]
    pub tool_call: ToolCallTagConfig,
}

impl DsCoreSection {
    /// Generate the OpenAI model registry mapping
    #[must_use]
    pub fn model_registry(&self) -> std::collections::HashMap<String, String> {
        let mut map = std::collections::HashMap::new();
        for (i, ty) in self.model_types.iter().enumerate() {
            map.insert(format!("deepseek-{}", ty).to_lowercase(), ty.clone());
            if let Some(alias) = self.model_aliases.get(i) {
                let alias = alias.trim().to_lowercase();
                if !alias.is_empty() {
                    map.insert(alias, ty.clone());
                }
            }
        }
        map
    }
}

/// Admin configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct AdminConfig {
    /// bcrypt hashed password
    #[serde(default)]
    pub password_hash: String,
    /// JWT signing secret (hex-encoded 32-byte random value)
    #[serde(default)]
    pub jwt_secret: String,
    /// Most recent JWT issuance time (used to revoke old tokens)
    #[serde(default)]
    pub jwt_issued_at: u64,
    /// Password change: old password in plaintext (only received via PUT, not persisted to config.toml)
    #[serde(default, skip_serializing)]
    pub old_password: String,
    /// Password change: new password in plaintext (only received via PUT, not persisted to config.toml)
    #[serde(default, skip_serializing)]
    pub new_password: String,
}

/// API Key entry
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ApiKeyEntry {
    pub key: String,
    pub description: String,
}

/// Single account configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Account {
    /// Email (mutually exclusive with mobile)
    pub email: String,
    /// Mobile phone number (mutually exclusive with email)
    pub mobile: String,
    /// Area code (used with mobile, e.g. "+86")
    pub area_code: String,
    /// Password
    pub password: String,
}

/// Proxy configuration
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProxyConfig {
    /// Proxy URL, e.g. http://127.0.0.1:7890 or socks5://127.0.0.1:7891
    pub url: Option<String>,
}

/// Tool call tag configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ToolCallTagConfig {
    /// Extra start tags (built-in `<|tool_calls_begin|>` + fuzzy matching; only add variants with completely different formats here)
    #[serde(default = "default_tool_call_starts")]
    pub extra_starts: Vec<String>,
    /// Extra end tags (built-in `<|tool_calls_end|>` + fuzzy matching; only add variants with completely different formats here)
    #[serde(default = "default_tool_call_ends")]
    pub extra_ends: Vec<String>,
}

impl Default for ToolCallTagConfig {
    fn default() -> Self {
        Self {
            extra_starts: default_tool_call_starts(),
            extra_ends: default_tool_call_ends(),
        }
    }
}

// ── Default value functions ──────────────────────────────────────────────────────────

fn default_tool_call_starts() -> Vec<String> {
    vec![
        "<|tool_call_begin|>".into(),
        "<tool_calls>".into(),
        "<tool_call>".into(),
    ]
}

fn default_tool_call_ends() -> Vec<String> {
    vec![
        "<|tool_call_end|>".into(),
        "</tool_calls>".into(),
        "</tool_call>".into(),
    ]
}

fn default_model_types() -> Vec<String> {
    vec![
        "default".to_string(),
        "expert".to_string(),
        "vision".to_string(),
    ]
}

fn default_max_input_tokens() -> Vec<u32> {
    vec![1_048_576, 1_048_576, 1_048_576]
}

fn default_max_output_tokens() -> Vec<u32> {
    vec![384_000, 384_000, 384_000]
}

fn default_input_character_limits() -> Vec<u32> {
    vec![2_621_440, 163_840, 2_621_440]
}

fn default_api_base() -> String {
    "https://chat.deepseek.com/api/v0".to_string()
}

fn default_wasm_url() -> String {
    "https://fe-static.deepseek.com/chat/static/sha3_wasm_bg.7b9ca65ddd.wasm".to_string()
}

fn default_user_agent() -> String {
    "DeepSeek/2.1.1 Android/35".to_string()
}

fn default_client_version() -> String {
    "2.0.0".to_string()
}

fn default_client_platform() -> String {
    "android".to_string()
}

fn default_client_locale() -> String {
    "zh_CN".to_string()
}

/// HTTP server configuration (required)
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ServerConfig {
    /// Listen address
    pub host: String,
    /// Listen port
    pub port: u16,
    /// CORS allowed Origin list, default ["http://localhost:22217"]
    #[serde(default = "default_cors_origins")]
    pub cors_origins: Vec<String>,
}

fn default_cors_origins() -> Vec<String> {
    vec!["http://localhost:22217".to_string()]
}

// ── Config implementation ─────────────────────────────────────────────────────────

impl Config {
    /// Load configuration from the specified path
    pub fn load<P: AsRef<Path>>(path: P) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let mut config: Self = toml::de::from_str(&content)?;
        config.dedup_accounts();
        config.validate()?;
        Ok(config)
    }

    /// Deduplicate by email (priority) or mobile, keeping the first occurrence
    fn dedup_accounts(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.ds_core.accounts.retain(|a| {
            let key = if a.email.is_empty() {
                a.mobile.clone()
            } else {
                a.email.clone()
            };
            seen.insert(key)
        });
    }

    /// Parse command-line arguments and load configuration
    pub fn load_with_args(
        args: impl Iterator<Item = String>,
    ) -> Result<(Self, PathBuf), ConfigError> {
        let mut explicit_c = false;
        let mut config_path = None;
        let mut iter = args.skip(1);

        while let Some(arg) = iter.next() {
            if arg == "-c" {
                explicit_c = true;
                if let Some(path) = iter.next() {
                    config_path = Some(path);
                } else {
                    return Err(ConfigError::Cli("-c requires a path argument".to_string()));
                }
            }
        }

        let path: PathBuf = config_path
            .map(PathBuf::from)
            .or_else(|| std::env::var("DS_CONFIG_PATH").ok().map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("config.toml"));

        if !path.exists() {
            if explicit_c {
                return Err(ConfigError::Io(std::io::Error::new(
                    std::io::ErrorKind::NotFound,
                    format!("Specified config file does not exist: {}", path.display()),
                )));
            }
            let default = Config {
                ds_core: DsCoreSection {
                    accounts: Vec::new(),
                    ..Default::default()
                },
                server: ServerConfig {
                    host: "127.0.0.1".into(),
                    port: 22217,
                    cors_origins: default_cors_origins(),
                },
                proxy: ProxyConfig::default(),
                admin: AdminConfig::default(),
                api_keys: Vec::new(),
            };
            if let Some(parent) = path.parent() {
                let parent_str = parent.as_os_str();
                if !parent_str.is_empty() {
                    std::fs::create_dir_all(parent)?;
                }
            }
            default.save(&path)?;
            log::info!(target: "config", "Created default config file: {}", path.display());
            return Ok((default, path));
        }

        let config = Self::load(&path)?;
        Ok((config, path))
    }

    /// Validate configuration
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.ds_core.model_types.is_empty() {
            return Err(ConfigError::Validation("model_types cannot be empty".to_string()));
        }
        let n = self.ds_core.model_types.len();
        if self.ds_core.max_input_tokens.len() != n {
            return Err(ConfigError::Validation(format!(
                "max_input_tokens length ({}) must match model_types length ({})",
                self.ds_core.max_input_tokens.len(),
                n
            )));
        }
        if self.ds_core.max_output_tokens.len() != n {
            return Err(ConfigError::Validation(format!(
                "max_output_tokens length ({}) must match model_types length ({})",
                self.ds_core.max_output_tokens.len(),
                n
            )));
        }
        if self.ds_core.input_character_limits.len() != n {
            return Err(ConfigError::Validation(format!(
                "input_character_limits length ({}) must match model_types length ({})",
                self.ds_core.input_character_limits.len(),
                n
            )));
        }
        let mut seen_keys = std::collections::HashSet::new();
        for k in &self.api_keys {
            if !seen_keys.insert(&k.key) {
                let prefix = if k.key.len() > 12 {
                    &k.key[..12]
                } else {
                    &k.key
                };
                return Err(ConfigError::Validation(format!(
                    "Duplicate API key: {}...",
                    prefix
                )));
            }
        }
        Ok(())
    }

    /// Atomically save configuration to file
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), ConfigError> {
        let toml_str = toml::to_string_pretty(self).map_err(ConfigError::TomlSerialization)?;
        let tmp = path.as_ref().with_extension("toml.tmp");
        std::fs::write(&tmp, &toml_str)?;
        std::fs::rename(&tmp, path.as_ref())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path.as_ref(), perms)?;
        }
        Ok(())
    }
}

impl Default for DsCoreSection {
    fn default() -> Self {
        Self {
            accounts: Vec::new(),
            api_base: default_api_base(),
            wasm_url: default_wasm_url(),
            user_agent: default_user_agent(),
            client_version: default_client_version(),
            client_platform: default_client_platform(),
            client_locale: default_client_locale(),
            model_types: default_model_types(),
            max_input_tokens: default_max_input_tokens(),
            max_output_tokens: default_max_output_tokens(),
            input_character_limits: default_input_character_limits(),
            model_aliases: Vec::new(),
            tool_call: ToolCallTagConfig::default(),
        }
    }
}

/// Configuration loading error type
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("Config validation error: {0}")]
    Validation(String),
    #[error("Command-line argument error: {0}")]
    Cli(String),
    #[error("TOML serialization error: {0}")]
    TomlSerialization(#[from] toml::ser::Error),
}

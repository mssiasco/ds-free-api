//! OpenAI protocol adapter layer — bidirectional conversion between OpenAI JSON and ds_core internal format
//!
//! This module is responsible for converting OpenAI-compatible HTTP requests to ds_core internal format,
//! and converting ds_core responses to OpenAI-compatible JSON format.
//!
//! Exposes a minimal public interface: OpenAIAdapter, OpenAIAdapterError

use std::pin::Pin;
use std::sync::Arc;

use bytes::Bytes;
use futures::Stream;

use ds_core::{AccountConfig, CoreError, DsCore, DsCoreConfig};
use std::collections::HashMap;

mod models;
pub(crate) mod request;
pub(crate) mod response;
pub(crate) mod types;

pub use types::{ChatCompletionsRequest, ChatCompletionsResponse, ChatCompletionsResponseChunk};

/// Streaming response type (SSE byte stream)
pub type StreamResponse = Pin<Box<dyn Stream<Item = Result<Bytes, OpenAIAdapterError>> + Send>>;

/// Streaming response struct stream
pub type ChunkStream =
    Pin<Box<dyn Stream<Item = Result<ChatCompletionsResponseChunk, OpenAIAdapterError>> + Send>>;

/// Chat Completions unified output
pub enum ChatOutput {
    Stream(ChunkStream),
    Json(ChatCompletionsResponse),
}

/// adapter 层通用结果包装：携带请求结果和账号标识
pub struct ChatResult<T> {
    pub data: T,
    pub account_id: String,
    pub prompt_tokens: u32,
}

/// OpenAI adapter
pub struct OpenAIAdapter {
    ds_core: Arc<DsCore>,
    model_types: tokio::sync::RwLock<Vec<String>>,
    model_registry: tokio::sync::RwLock<HashMap<String, String>>,
    model_aliases: tokio::sync::RwLock<Vec<String>>,
    max_input_tokens: tokio::sync::RwLock<Vec<u32>>,
    max_output_tokens: tokio::sync::RwLock<Vec<u32>>,
    tag_config: tokio::sync::RwLock<Arc<response::TagConfig>>,
    /// 缓存的 tiktoken BPE 编码器（避免每次请求重建）
    bpe: Option<Arc<tiktoken_rs::CoreBPE>>,
}

impl OpenAIAdapter {
    /// Create adapter instance
    pub async fn new(config: &crate::config::Config) -> Result<Self, OpenAIAdapterError> {
        let core_cfg = DsCoreConfig {
            api_base: config.ds_core.api_base.clone(),
            wasm_url: config.ds_core.wasm_url.clone(),
            user_agent: config.ds_core.user_agent.clone(),
            client_version: config.ds_core.client_version.clone(),
            client_platform: config.ds_core.client_platform.clone(),
            client_locale: config.ds_core.client_locale.clone(),
            proxy_url: config.proxy.url.clone(),
            model_types: config.ds_core.model_types.clone(),
            input_character_limits: config.ds_core.input_character_limits.clone(),
        };
        let accounts: Vec<AccountConfig> = config
            .ds_core
            .accounts
            .iter()
            .map(|a| AccountConfig {
                email: a.email.clone(),
                mobile: a.mobile.clone(),
                area_code: a.area_code.clone(),
                password: a.password.clone(),
            })
            .collect();
        let ds_core = Arc::new(DsCore::new(&core_cfg, accounts).await?);
        let model_registry = config.ds_core.model_registry();
        // 预初始化 tiktoken BPE（避免每次请求重建词表）
        let bpe = tiktoken_rs::cl100k_base().ok().map(Arc::new);

        Ok(Self {
            ds_core,
            model_types: tokio::sync::RwLock::new(config.ds_core.model_types.clone()),
            model_registry: tokio::sync::RwLock::new(model_registry),
            model_aliases: tokio::sync::RwLock::new(config.ds_core.model_aliases.clone()),
            max_input_tokens: tokio::sync::RwLock::new(config.ds_core.max_input_tokens.clone()),
            max_output_tokens: tokio::sync::RwLock::new(config.ds_core.max_output_tokens.clone()),
            tag_config: tokio::sync::RwLock::new(Arc::new(response::TagConfig::from_config(
                &config.ds_core.tool_call,
            ))),
            bpe,
        })
    }

    /// POST /v1/chat/completions（统一入口）
    ///
    /// 内部校验参数、构建 ChatML prompt、按 stream 标记分流：
    /// - stream=true  → returns SSE byte stream
    /// - stream=false → aggregates SSE stream into single JSON object and returns
    pub async fn chat_completions(
        &self,
        mut req: ChatCompletionsRequest,
        request_id: &str,
    ) -> Result<ChatResult<ChatOutput>, OpenAIAdapterError> {
        log::debug!(target: "adapter", "req={} adapter processing started: model={}, stream={}", request_id, req.model, req.stream);
        use crate::openai_adapter::types::{
            FunctionCallOption, NamedFunction, NamedToolChoice, Tool, ToolChoice,
        };

        // Legacy compat: functions / function_call → tools / tool_choice
        if req.tools.as_ref().map(|t| t.is_empty()).unwrap_or(true)
            && let Some(functions) = req.functions.clone()
            && !functions.is_empty()
        {
            req.tools = Some(
                functions
                    .into_iter()
                    .map(|f| Tool {
                        ty: "function".to_string(),
                        function: Some(f),
                        custom: None,
                    })
                    .collect(),
            );
        }
        if req.tool_choice.is_none()
            && let Some(fc) = req.function_call.clone()
        {
            req.tool_choice = Some(match fc {
                FunctionCallOption::Mode(mode) => ToolChoice::Mode(mode),
                FunctionCallOption::Named(named) => ToolChoice::Named(NamedToolChoice {
                    ty: "function".to_string(),
                    function: NamedFunction { name: named.name },
                }),
            });
        }

        let norm = request::normalize::apply(&req).map_err(OpenAIAdapterError::BadRequest)?;
        let tool_ctx = request::tools::extract(&req).map_err(OpenAIAdapterError::BadRequest)?;
        let prompt = request::prompt::build(&req, &tool_ctx);
        let registry = self.model_registry.read().await;
        let model_res = request::resolver::resolve(
            &registry,
            &req.model,
            req.reasoning_effort.as_deref(),
            req.web_search_options.as_ref(),
        )
        .map_err(OpenAIAdapterError::BadRequest)?;

        let prompt_tokens = self
            .bpe
            .as_ref()
            .map(|bpe| {
                u32::try_from(bpe.encode_with_special_tokens(&prompt).len())
                    .expect("token count exceeds u32::MAX")
            })
            .unwrap_or(0);

        let file_result = request::files::extract(&req);
        let chat_req = ds_core::ChatRequest {
            prompt,
            thinking_enabled: model_res.thinking_enabled,
            search_enabled: model_res.search_enabled || file_result.has_http_urls,
            model_type: model_res.model_type,
            files: file_result.files,
        };

        let chat_resp = self.try_chat(chat_req, request_id).await?;
        let (account_id, event_stream) = Self::take_meta(chat_resp.stream).await.map_err(|e| {
            log::error!(target: "adapter", "req={} failed to get Meta event: {}", request_id, e);
            OpenAIAdapterError::Internal("failed to get Meta event".into())
        })?;

        // Prepare tool definition info for repair model
        let tool_defs = req.tools.as_ref().map(|tools| {
            tools
                .iter()
                .filter_map(|t| t.function.as_ref())
                .map(|f| {
                    format!(
                        "- {}: {}",
                        f.name,
                        serde_json::to_string(&f.parameters).unwrap_or_default()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        });

        if req.stream {
            let repair_fn = self.create_repair_fn(request_id, tool_defs.clone()).await;
            let s = response::stream(
                event_stream,
                req.model,
                response::StreamCfg {
                    include_usage: norm.include_usage,
                    include_obfuscation: norm.include_obfuscation,
                    stop: norm.stop,
                    prompt_tokens,
                    repair_fn: Some(repair_fn),
                    tag_config: self.tag_config.read().await.clone(),
                },
            );
            Ok(ChatResult {
                data: ChatOutput::Stream(s),
                account_id,
                prompt_tokens,
            })
        } else {
            let repair_fn = self.create_repair_fn(request_id, tool_defs).await;
            let json = response::aggregate(
                event_stream,
                req.model,
                response::StreamCfg {
                    include_usage: true,
                    include_obfuscation: false,
                    stop: norm.stop,
                    prompt_tokens,
                    repair_fn: Some(repair_fn),
                    tag_config: self.tag_config.read().await.clone(),
                },
            )
            .await?;
            Ok(ChatResult {
                data: ChatOutput::Json(json),
                account_id,
                prompt_tokens,
            })
        }
    }

    /// 内部辅助：对 `Overloaded` 进行退避重试（v0_chat 内部已做换号重试，此处为号池级兜底）
    pub(crate) async fn try_chat(
        &self,
        req: ds_core::ChatRequest,
        request_id: &str,
    ) -> Result<ds_core::ChatResponse, CoreError> {
        const MAX_RETRIES: usize = 2;
        const BASE_DELAY_MS: u64 = 2000;

        for attempt in 0..MAX_RETRIES {
            match self.ds_core.v0_chat(req.clone(), request_id).await {
                Ok(resp) => {
                    if attempt > 0 {
                        log::info!(target: "adapter", "req={} retry {} succeeded", request_id, attempt);
                    }
                    return Ok(resp);
                }
                Err(CoreError::Overloaded) if attempt + 1 < MAX_RETRIES => {
                    let delay = BASE_DELAY_MS * (1 << attempt);
                    log::warn!(target: "adapter", "req={} Overloaded, retry {} waiting {}ms", request_id, attempt + 1, delay);
                    tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
                }
                Err(e) => return Err(e),
            }
        }
        log::warn!(target: "adapter", "req={} {} 次重试均失败，放弃", request_id, MAX_RETRIES);
        Err(CoreError::Overloaded)
    }

    /// GET /v1/models
    pub async fn list_models(&self) -> types::OpenAIModelList {
        let model_types = self.model_types.read().await;
        let max_input = self.max_input_tokens.read().await;
        let max_output = self.max_output_tokens.read().await;
        let aliases = self.model_aliases.read().await;
        models::list(&model_types, &max_input, &max_output, &aliases)
    }

    /// GET /v1/models/{model_id}
    pub async fn get_model(&self, model_id: &str) -> Option<types::OpenAIModel> {
        let model_types = self.model_types.read().await;
        let max_input = self.max_input_tokens.read().await;
        let max_output = self.max_output_tokens.read().await;
        let aliases = self.model_aliases.read().await;
        models::get(&model_types, &max_input, &max_output, &aliases, model_id)
    }

    /// 原始 DeepSeek SSE 流（不经 OpenAI 协议转换）
    ///
    /// 用于流分析：对比原始响应与 OpenAI 转换后的差异，定位转换 bug
    pub async fn raw_chat_completions_stream(
        &self,
        body: &[u8],
        request_id: &str,
    ) -> Result<ChatResult<StreamResponse>, OpenAIAdapterError> {
        let chat_req: ChatCompletionsRequest = serde_json::from_slice(body)
            .map_err(|e| OpenAIAdapterError::BadRequest(format!("bad request: {}", e)))?;
        let registry = self.model_registry.read().await;
        let model_res = request::resolver::resolve(
            &registry,
            &chat_req.model,
            chat_req.reasoning_effort.as_deref(),
            chat_req.web_search_options.as_ref(),
        )
        .map_err(OpenAIAdapterError::BadRequest)?;
        let ds_req = ds_core::ChatRequest {
            prompt: request::prompt::build(
                &chat_req,
                &request::tools::extract(&chat_req).map_err(OpenAIAdapterError::BadRequest)?,
            ),
            thinking_enabled: model_res.thinking_enabled,
            search_enabled: model_res.search_enabled,
            model_type: model_res.model_type,
            files: vec![],
        };
        let chat_resp = self.try_chat(ds_req, request_id).await?;
        let (account_id, event_stream) = Self::take_meta(chat_resp.stream).await?;

        // Serialize StreamEvent to JSON lines for debugging
        use futures::StreamExt;
        let data: StreamResponse = Box::pin(event_stream.map(|r| {
            r.map(|evt| {
                let line = format!("{:?}\n", evt);
                Bytes::from(line.into_bytes())
            })
            .map_err(OpenAIAdapterError::from)
        }));
        Ok(ChatResult {
            data,
            account_id,
            prompt_tokens: 0,
        })
    }

    /// Get ds_core account pool status
    pub fn account_statuses(&self) -> Vec<ds_core::AccountStatus> {
        self.ds_core.account_statuses()
    }

    /// Dynamically add account
    pub async fn add_account(
        &self,
        creds: &crate::config::Account,
    ) -> Result<String, ds_core::PoolError> {
        let ac = AccountConfig {
            email: creds.email.clone(),
            mobile: creds.mobile.clone(),
            area_code: creds.area_code.clone(),
            password: creds.password.clone(),
        };
        self.ds_core.add_account(&ac).await
    }

    /// Dynamically remove account
    pub async fn remove_account(
        &self,
        email_or_mobile: &str,
    ) -> Result<String, ds_core::PoolError> {
        self.ds_core.remove_account(email_or_mobile).await
    }

    /// Mark account as Error state
    pub fn mark_error(&self, email_or_mobile: &str) {
        self.ds_core.mark_error(email_or_mobile);
    }

    /// Manually re-login specified account
    pub async fn re_login_single(&self, email_or_mobile: &str) -> Result<(), String> {
        self.ds_core.re_login_single(email_or_mobile).await
    }
}

impl OpenAIAdapter {
    /// 批量同步账号：对比当前账号池与目标配置，增删差异账号
    pub(crate) async fn sync_accounts(&self, new_accounts: &[crate::config::Account]) {
        let old_statuses = self.account_statuses();
        let old_ids: Vec<String> = old_statuses
            .iter()
            .map(|a| {
                if a.email.is_empty() {
                    a.mobile.clone()
                } else {
                    a.email.clone()
                }
            })
            .collect();

        let mut _added = 0usize;
        let mut _failed = 0usize;
        for acct in new_accounts {
            let id = if acct.email.is_empty() {
                &acct.mobile
            } else {
                &acct.email
            };
            if !old_ids.contains(id) {
                match self.add_account(acct).await {
                    Ok(_) => _added += 1,
                    Err(e) => {
                        log::warn!(target: "adapter", "sync add account {} failed: {}", id, e);
                        _failed += 1;
                    }
                }
            }
        }

        let mut _removed = 0usize;
        let new_ids: Vec<&str> = new_accounts
            .iter()
            .map(|a| {
                if a.email.is_empty() {
                    a.mobile.as_str()
                } else {
                    a.email.as_str()
                }
            })
            .collect();
        for old_id in &old_ids {
            if !new_ids.contains(&old_id.as_str()) && !old_id.is_empty() {
                match self.remove_account(old_id).await {
                    Ok(_) => _removed += 1,
                    Err(e) => {
                        log::warn!(target: "adapter", "sync remove account {} failed: {}", old_id, e);
                    }
                }
            }
        }
    }

    /// 从流中提取首个 Meta 事件的 account_id，同时保留 Meta 在流中
    async fn take_meta(
        stream: Pin<Box<dyn Stream<Item = Result<ds_core::StreamEvent, CoreError>> + Send>>,
    ) -> Result<
        (
            String,
            Pin<Box<dyn Stream<Item = Result<ds_core::StreamEvent, CoreError>> + Send>>,
        ),
        CoreError,
    > {
        use futures::StreamExt;
        let mut stream = stream;
        match stream.next().await {
            Some(Ok(ds_core::StreamEvent::Meta { account_id })) => {
                let full =
                    futures::stream::once(futures::future::ready(Ok(ds_core::StreamEvent::Meta {
                        account_id: account_id.clone(),
                    })))
                    .chain(stream);
                Ok((account_id, Box::pin(full)))
            }
            Some(Ok(other)) => {
                log::warn!(target: "adapter", "预期 Meta 为首事件，实际收到: {other:?}");
                let rest = futures::stream::once(futures::future::ready(Ok(other))).chain(stream);
                Ok((String::new(), Box::pin(rest)))
            }
            Some(Err(e)) => Err(e),
            None => Err(CoreError::Stream("empty stream".into())),
        }
    }

    /// Graceful shutdown
    pub async fn shutdown(&self) {
        self.ds_core.shutdown().await;
    }

    pub async fn reload_config(&self, new_config: &crate::config::Config) -> Result<(), CoreError> {
        // Sync accounts
        self.sync_accounts(&new_config.ds_core.accounts).await;
        // Rebuild model registry
        let registry = new_config.ds_core.model_registry();
        *self.model_registry.write().await = registry;
        *self.model_types.write().await = new_config.ds_core.model_types.clone();
        *self.model_aliases.write().await = new_config.ds_core.model_aliases.clone();
        *self.max_input_tokens.write().await = new_config.ds_core.max_input_tokens.clone();
        *self.max_output_tokens.write().await = new_config.ds_core.max_output_tokens.clone();
        *self.tag_config.write().await = Arc::new(response::TagConfig::from_config(
            &new_config.ds_core.tool_call,
        ));
        // Rebuild DsClient if needed (deepseek/proxy changes)
        let core_cfg = DsCoreConfig {
            api_base: new_config.ds_core.api_base.clone(),
            wasm_url: new_config.ds_core.wasm_url.clone(),
            user_agent: new_config.ds_core.user_agent.clone(),
            client_version: new_config.ds_core.client_version.clone(),
            client_platform: new_config.ds_core.client_platform.clone(),
            client_locale: new_config.ds_core.client_locale.clone(),
            proxy_url: new_config.proxy.url.clone(),
            model_types: new_config.ds_core.model_types.clone(),
            input_character_limits: new_config.ds_core.input_character_limits.clone(),
        };
        self.ds_core.reload_config(&core_cfg).await
    }

    pub(crate) async fn create_repair_fn(
        &self,
        request_id: &str,
        tool_defs: Option<String>,
    ) -> response::RepairFn {
        use std::sync::atomic::{AtomicU16, Ordering};
        let core = self.ds_core.clone();
        let req_id = request_id.to_string();
        let seq = Arc::new(AtomicU16::new(0));
        let tag_config = self.tag_config.read().await.clone();
        let tools_info = tool_defs.unwrap_or_default();
        Arc::new(move |tool_text: String| {
            let core = core.clone();
            let req_id = req_id.clone();
            let seq = seq.clone();
            let tag_config = tag_config.clone();
            let tools_info = tools_info.clone();
            Box::pin(async move {
                use ds_core::ChatRequest;
                let n = seq.fetch_add(1, Ordering::Relaxed);
                let repair_req_id = format!("{}-repair-{}", req_id, n);
                let mut prompt = String::new();
                if !tools_info.is_empty() {
                    prompt.push_str(&format!("Available tool definitions：\n{}\n\n", tools_info));
                }
                prompt.push_str(&format!(
                    "Please extract the content from the following code block and convert it into a valid tool call JSON array.\
                     \nEach element must contain \"name\"（字符串）和 \"arguments\"（对象）字段。\
                     \n只Output JSON 数组本身，不要加 code fence，不要其他文字解释。\
                     \n注意：字符串值中的引号和换行符必须用反斜杠转义（如 \\\" 和 \\n）。\
                     \n\nContent to repair：\n~~~\n{tool_text}\n~~~"
                ));
                let req = ChatRequest {
                    prompt,
                    thinking_enabled: false,
                    search_enabled: false,
                    model_type: "default".to_string(),
                    files: vec![],
                };
                log::debug!(
                    target: "adapter",
                    "{} initiating repair request: len={}", repair_req_id, tool_text.len()
                );
                let resp = core
                    .v0_chat(req, &repair_req_id)
                    .await
                    .map_err(OpenAIAdapterError::from)?;
                response::execute_tool_repair(resp.stream, &tag_config).await
            })
        })
    }
}

/// Adapter error type
#[derive(Debug, thiserror::Error)]
pub enum OpenAIAdapterError {
    /// Bad request format
    #[error("bad request: {0}")]
    BadRequest(String),

    /// 服务过载，无可用的 ds_core 账号
    #[error("service overloaded")]
    Overloaded,

    /// 上游提供商错误（网络、业务错误等）
    #[error("provider error: {0}")]
    ProviderError(String),

    /// 内部错误（序列化、流转换等）
    #[error("internal error: {0}")]
    Internal(String),

    /// tool_calls tag parsing failed，携带 `{TOOL_CALL_START}...{TOOL_CALL_END}`  raw text
    #[error("tool_calls repair needed: {0}")]
    ToolCallRepairNeeded(String),
}

impl From<CoreError> for OpenAIAdapterError {
    fn from(e: CoreError) -> Self {
        match e {
            CoreError::Overloaded => Self::Overloaded,
            CoreError::ProofOfWorkFailed(err) => {
                Self::Internal(format!("proof of work failed: {}", err))
            }
            CoreError::ProviderError(msg) => Self::ProviderError(msg),
            CoreError::Stream(msg) => Self::Internal(msg),
        }
    }
}

impl From<serde_json::Error> for OpenAIAdapterError {
    fn from(e: serde_json::Error) -> Self {
        Self::Internal(format!("json serialization failed: {}", e))
    }
}

impl OpenAIAdapterError {
    /// Returns corresponding HTTP status code
    #[must_use]
    pub fn status_code(&self) -> u16 {
        match self {
            Self::BadRequest(_) => 400,
            Self::Overloaded => 429,
            Self::ProviderError(_) => 502,
            Self::Internal(_) | Self::ToolCallRepairNeeded(_) => 500,
        }
    }
}

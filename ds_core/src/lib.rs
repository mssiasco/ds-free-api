//! DeepSeek core module — adapter layer from OpenAI API to DeepSeek
//!
//! Exposes minimal public interface: DsCore, CoreError, ChatRequest, DsCoreConfig, AccountConfig

mod accounts;
mod chat;
mod config;

pub use accounts::PoolError;
pub use accounts::pool::AccountStatus;
pub use chat::{ChatRequest, ChatResponse, FilePayload, StreamEvent};
pub use config::{AccountConfig, DsCoreConfig};

use accounts::Accounts;
use chat::Chat;
use std::sync::Arc;

/// Core layer error type
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// Service overloaded: all accounts are busy or unhealthy
    #[error("no available account")]
    Overloaded,

    /// PoW computation failed
    #[error("proof of work failed: {0}")]
    ProofOfWorkFailed(#[from] accounts::PowError),

    /// Provider error: network, business error, token expired, etc.
    #[error("provider: {0}")]
    ProviderError(String),

    /// Stream processing error: connection interrupted, etc.
    #[error("stream error: {0}")]
    Stream(String),
}

impl From<accounts::ClientError> for CoreError {
    fn from(e: accounts::ClientError) -> Self {
        CoreError::ProviderError(e.to_string())
    }
}

pub struct DsCore {
    accounts: Arc<Accounts>,
    chat: Chat,
}

impl DsCore {
    pub async fn new(
        config: &DsCoreConfig,
        account_creds: Vec<AccountConfig>,
    ) -> Result<Self, CoreError> {
        let accounts = Accounts::new(config, account_creds).await?;
        let chat = Chat::new(Arc::clone(&accounts), config);

        Ok(Self { accounts, chat })
    }

    /// Initiate a chat request, returns SSE byte stream + account identifier
    ///
    /// Automatically releases the account when the stream ends or is dropped
    pub async fn v0_chat(
        &self,
        req: ChatRequest,
        request_id: &str,
    ) -> Result<ChatResponse, CoreError> {
        self.chat.v0_chat(req, request_id).await
    }

    #[must_use]
    pub fn account_statuses(&self) -> Vec<AccountStatus> {
        self.accounts.account_statuses()
    }

    /// Dynamically add an account
    pub async fn add_account(&self, creds: &AccountConfig) -> Result<String, PoolError> {
        self.accounts.add_account(creds).await
    }

    /// Dynamically remove an account
    pub async fn remove_account(&self, email_or_mobile: &str) -> Result<String, PoolError> {
        self.accounts.remove_account(email_or_mobile).await
    }

    /// Mark account as Error state
    pub fn mark_error(&self, email_or_mobile: &str) {
        self.accounts.mark_error(email_or_mobile);
    }

    /// Manually re-login a specific account
    pub async fn re_login_single(&self, email_or_mobile: &str) -> Result<(), String> {
        self.accounts.re_login_single(email_or_mobile).await
    }

    /// Graceful shutdown: clean up all account sessions
    pub async fn shutdown(&self) {
        self.chat.shutdown().await;
        self.accounts.shutdown().await;
    }

    pub async fn reload_config(&self, config: &DsCoreConfig) -> Result<(), CoreError> {
        self.accounts.reload_config(config).await
    }
}

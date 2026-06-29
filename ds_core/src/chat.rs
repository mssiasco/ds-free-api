//! Chat module — request dispatch and response stream processing
//!
//! Acquires account resources through the accounts module, dispatches prompts by size
//! to different request paths, and returns SSE byte streams with account guards.

mod request;
mod response;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::accounts::Accounts;
use crate::config::DsCoreConfig;
use response::ActiveSession;

pub use request::{ChatRequest, ChatResponse, FilePayload};
pub use response::StreamEvent;

/// Unified entry point for the chat module
///
/// Holds a reference to accounts, responsible for prompt dispatch and returning wrapped streams.
pub struct Chat {
    accounts: Arc<Accounts>,
    active_sessions: Arc<Mutex<HashMap<String, ActiveSession>>>,
    model_types: Vec<String>,
    input_character_limits: Vec<u32>,
}

impl Chat {
    /// Create the chat module
    pub fn new(accounts: Arc<Accounts>, config: &DsCoreConfig) -> Self {
        Self {
            accounts,
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            model_types: config.model_types.clone(),
            input_character_limits: config.input_character_limits.clone(),
        }
    }

    /// Get the input_character_limit for the specified model_type
    fn input_character_limit_for(&self, model_type: &str) -> usize {
        self.model_types
            .iter()
            .position(|t| t == model_type)
            .and_then(|i| self.input_character_limits.get(i))
            .copied()
            .map(|v| v as usize)
            .unwrap_or(163_840)
    }

    /// Graceful shutdown: clean up all remaining active sessions
    pub async fn shutdown(&self) {
        let sessions = {
            let mut map = self.active_sessions.lock().unwrap();
            std::mem::take(&mut *map)
        };

        if sessions.is_empty() {
            return;
        }

        log::info!(
            target: "ds_core::accounts",
            "shutdown: cleaning up {} remaining sessions", sessions.len()
        );

        use crate::accounts::StopStreamPayload;
        use futures::future::join_all;

        let futures: Vec<_> = sessions
            .into_values()
            .map(|s| {
                let accounts = self.accounts.clone();
                async move {
                    let payload = StopStreamPayload {
                        chat_session_id: s.session_id.clone(),
                        message_id: s.message_id,
                    };
                    let _ = accounts.stop_stream(&s.token, &payload).await;
                    let _ = accounts
                        .delete_session(&s.token, &s.session_id)
                        .await
                        .inspect_err(|e| {
                            log::warn!(
                                target: "ds_core::accounts",
                                "shutdown: failed to clean up session {}: {}",
                                s.session_id, e
                            );
                        });
                }
            })
            .collect();
        join_all(futures).await;
    }
}

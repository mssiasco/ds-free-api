//! Account pool management — multi-account load balancing
//!
//! 1 account = 1 session = 1 concurrency. Scaling concurrency requires more accounts.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::time::SystemTime;

use dashmap::DashMap;
use futures::TryStreamExt;
use log::{debug, error, info, warn};
use tokio::sync::RwLock;

use super::client::{ClientError, CompletionPayload, DsClient, LoginPayload};
use super::pow::{PowError, PowSolver};
use crate::config::AccountConfig;

/// Account state enum
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccountState {
    Idle = 0,
    Busy = 1,
    Error = 2,
    Invalid = 3,
}

impl AccountState {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Idle,
            1 => Self::Busy,
            2 => Self::Error,
            _ => Self::Invalid,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Busy => "busy",
            Self::Error => "error",
            Self::Invalid => "invalid",
        }
    }
}

/// Account status information
#[derive(serde::Serialize)]
pub struct AccountStatus {
    pub email: String,
    pub mobile: String,
    pub state: String,
    /// Last release timestamp (ms), 0 means never used
    pub last_released_ms: i64,
    /// Consecutive login failure count
    pub error_count: u8,
}

pub struct Account {
    token: std::sync::RwLock<Arc<str>>,
    email: String,
    mobile: String,
    state: AtomicU8,
    /// Last release timestamp of the account (ms), used for cooldown check
    last_released: AtomicI64,
    /// Consecutive login failure count
    error_count: AtomicU8,
    /// Original credentials (used for re-login)
    creds: AccountConfig,
}

/// Maximum consecutive login failures before marking as Invalid
const MAX_ERROR_COUNT: u8 = 3;

impl Account {
    pub fn token(&self) -> Arc<str> {
        self.token.read().unwrap().clone()
    }

    pub fn display_id(&self) -> &str {
        if self.email.is_empty() {
            &self.mobile
        } else {
            &self.email
        }
    }

    pub fn state(&self) -> AccountState {
        AccountState::from_u8(self.state.load(Ordering::Relaxed))
    }

    pub fn is_busy(&self) -> bool {
        self.state() == AccountState::Busy
    }

    pub fn is_available(&self) -> bool {
        self.state() == AccountState::Idle
    }

    /// Create an account in Invalid state (used when initialization fails; still added to pool for frontend display)
    fn new_invalid(creds: AccountConfig) -> Self {
        Self {
            token: std::sync::RwLock::new(String::new().into()),
            email: creds.email.clone(),
            mobile: creds.mobile.clone(),
            state: AtomicU8::new(AccountState::Invalid as u8),
            last_released: AtomicI64::new(0),
            error_count: AtomicU8::new(MAX_ERROR_COUNT),
            creds,
        }
    }
}

/// Account is marked as busy during holding; automatically released on Drop
pub struct AccountGuard {
    account: Arc<Account>,
}

impl AccountGuard {
    pub fn account(&self) -> &Account {
        &self.account
    }
}

impl Drop for AccountGuard {
    fn drop(&mut self) {
        // Only release back to Idle from Busy state (avoid overwriting Error/Invalid)
        self.account
            .state
            .compare_exchange(
                AccountState::Busy as u8,
                AccountState::Idle as u8,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .ok();
        let d = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let now_ms = (d.as_secs() * 1000 + u64::from(d.subsec_millis())) as i64;
        self.account.last_released.store(now_ms, Ordering::Relaxed);
    }
}

pub struct AccountPool {
    /// key = display_id (email or mobile), value = Account
    accounts: DashMap<String, Arc<Account>>,
    client: RwLock<Option<DsClient>>,
    solver: RwLock<Option<PowSolver>>,
}

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    /// All accounts failed to initialize (no available accounts)
    #[error("All accounts failed to initialize")]
    AllAccountsFailed,

    /// Downstream client error (network, API error, etc.)
    #[error("Client error: {0}")]
    Client(#[from] ClientError),

    /// PoW computation failed (WASM execution error)
    #[error("PoW computation failed: {0}")]
    Pow(#[from] PowError),

    /// Account configuration validation failed
    #[error("Account config error: {0}")]
    Validation(String),

    /// Account already exists
    #[error("Account already exists: {0}")]
    AlreadyExists(String),

    /// Account not found
    #[error("Account not found: {0}")]
    NotFound(String),

    /// Account is in use, cannot be removed
    #[error("Account is in use: {0}")]
    AccountBusy(String),
}

impl AccountPool {
    pub fn new() -> Self {
        Self {
            accounts: DashMap::new(),
            client: RwLock::new(None),
            solver: RwLock::new(None),
        }
    }

    pub async fn init(
        &self,
        creds: Vec<AccountConfig>,
        client: &DsClient,
        solver: &PowSolver,
    ) -> Result<(), PoolError> {
        if creds.is_empty() {
            return Ok(());
        }

        use futures::future::join_all;
        use std::sync::Arc;
        use tokio::sync::Semaphore;

        // Limit concurrent initialization count to avoid putting pressure on DeepSeek's endpoints and local connection pool
        let semaphore = Arc::new(Semaphore::new(13));
        let futures: Vec<_> = creds
            .into_iter()
            .map(|creds| {
                let client = client.clone();
                let solver = solver.clone();
                let sem = semaphore.clone();
                async move {
                    let _permit = sem.acquire().await.expect("Semaphore closed unexpectedly");
                    let display_id = if creds.email.is_empty() {
                        creds.mobile.clone()
                    } else {
                        creds.email.clone()
                    };
                    let account = match init_account(&creds, &client, &solver).await {
                        Ok(account) => {
                            info!(target: "ds_core::accounts", "Account {} initialized successfully", display_id);
                            account
                        }
                        Err(e) => {
                            warn!(target: "ds_core::accounts", "Account {} initialization failed: {}", display_id, e);
                            // Even if initialization fails, add to pool marked as Invalid for frontend display
                            Account::new_invalid(creds.clone())
                        }
                    };
                    Some((display_id, Arc::new(account)))
                }
            })
            .collect();

        let results: Vec<(String, Arc<Account>)> =
            join_all(futures).await.into_iter().flatten().collect();
        let idle_count = results
            .iter()
            .filter(|(_, a)| a.state() == AccountState::Idle)
            .count();

        for (id, account) in &results {
            self.accounts.insert(id.clone(), Arc::clone(account));
        }

        if idle_count == 0 {
            warn!(target: "ds_core::accounts", "All accounts failed to initialize — they may be disabled or have invalid credentials");
        } else if results.len() > 1 && idle_count < results.len() {
            warn!(target: "ds_core::accounts", "{}/{} accounts unavailable", results.len() - idle_count, results.len());
        }
        Ok(())
    }

    /// Dynamically add an account (runtime initialization)
    pub async fn add_account(
        &self,
        creds: &AccountConfig,
        client: &DsClient,
        solver: &PowSolver,
    ) -> Result<String, PoolError> {
        let display_id = if creds.email.is_empty() {
            creds.mobile.clone()
        } else {
            creds.email.clone()
        };

        // Check if already exists (DashMap O(1) lookup)
        if self.accounts.contains_key(&display_id) {
            return Err(PoolError::AlreadyExists(display_id));
        }

        let account = init_account(creds, client, solver).await?;
        let _id = account.display_id().to_string();
        self.accounts.insert(display_id.clone(), Arc::new(account));
        info!(target: "ds_core::accounts", "Account {} added dynamically", display_id);
        Ok(display_id)
    }

    /// Dynamically remove an account (only idle accounts can be removed)
    pub async fn remove_account(&self, email_or_mobile: &str) -> Result<String, PoolError> {
        let account = self
            .accounts
            .get(email_or_mobile)
            .ok_or_else(|| PoolError::NotFound(email_or_mobile.to_string()))?;

        if account.is_busy() {
            return Err(PoolError::AccountBusy(email_or_mobile.to_string()));
        }

        // Also allow removing Error/Invalid state accounts
        drop(account);
        let (_, removed) = self
            .accounts
            .remove(email_or_mobile)
            .ok_or_else(|| PoolError::NotFound(email_or_mobile.to_string()))?;
        let id = removed.display_id().to_string();
        info!(target: "ds_core::accounts", "Account {} removed", id);
        Ok(id)
    }

    /// Get the longest-idle available account with waiting: waits up to `timeout_ms` milliseconds if no account is available
    pub async fn get_account_with_wait(&self, timeout_ms: u64) -> Option<AccountGuard> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
        loop {
            if let Some(g) = self.get_account() {
                return Some(g);
            }
            if tokio::time::Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
    }

    /// Get the longest-idle available account (no waiting, return immediately)
    ///
    /// Iterates all accounts, selects the one with cooldown expired and longest idle time,
    /// maximizing the interval between each use.
    /// DashMap lock-free reads, does not block concurrent requests.
    pub fn get_account(&self) -> Option<AccountGuard> {
        if self.accounts.is_empty() {
            return None;
        }

        let d = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default();
        let now_ms = (d.as_secs() * 1000 + u64::from(d.subsec_millis())) as i64;

        let mut best: Option<Arc<Account>> = None;
        let mut best_idle = i64::MIN;

        for entry in self.accounts.iter() {
            let account = entry.value();
            if !account.is_available() {
                continue;
            }
            let idle = now_ms - account.last_released.load(Ordering::Relaxed);
            if idle > best_idle {
                best_idle = idle;
                best = Some(Arc::clone(account));
            }
        }

        let account = best?;
        account
            .state
            .compare_exchange(
                AccountState::Idle as u8,
                AccountState::Busy as u8,
                Ordering::Relaxed,
                Ordering::Relaxed,
            )
            .ok()?;
        Some(AccountGuard { account })
    }

    /// Get detailed status of all accounts
    pub fn account_statuses(&self) -> Vec<AccountStatus> {
        self.accounts
            .iter()
            .map(|entry| {
                let a = entry.value();
                AccountStatus {
                    email: a.email.clone(),
                    mobile: a.mobile.clone(),
                    state: a.state().as_str().to_string(),
                    last_released_ms: a.last_released.load(Ordering::Relaxed),
                    error_count: a.error_count.load(Ordering::Relaxed),
                }
            })
            .collect()
    }

    /// Graceful shutdown (new flow has no persistent sessions, no cleanup needed)
    pub async fn shutdown(&self, _client: &DsClient) {}

    /// Store client and solver for use by the recovery task
    pub async fn set_client_solver(&self, client: DsClient, solver: PowSolver) {
        *self.client.write().await = Some(client);
        *self.solver.write().await = Some(solver);
    }

    /// Mark account as Error state (called on request failure)
    pub fn mark_error(&self, email_or_mobile: &str) {
        if let Some(entry) = self.accounts.get(email_or_mobile) {
            let account = entry.value();
            // Only transition from Busy to Error (avoid overwriting Invalid)
            account
                .state
                .compare_exchange(
                    AccountState::Busy as u8,
                    AccountState::Error as u8,
                    Ordering::Relaxed,
                    Ordering::Relaxed,
                )
                .ok();
            warn!(target: "ds_core::accounts", "Account {} marked as Error", account.display_id());
        }
    }

    /// Manually re-login a specific account (admin-triggered)
    /// Success → Idle, failure → error_count++, ≥3 → Invalid
    pub async fn re_login_single(&self, email_or_mobile: &str) -> Result<(), String> {
        let client_opt = self.client.read().await.clone();
        let solver_opt = self.solver.read().await.clone();
        let (Some(client), Some(solver)) = (client_opt, solver_opt) else {
            return Err("client/solver not initialized".to_string());
        };

        let account = self
            .accounts
            .get(email_or_mobile)
            .ok_or_else(|| format!("Account {} does not exist", email_or_mobile))?;
        let account = account.value();

        // Only allow re-login for Error/Invalid state accounts
        let state = account.state();
        if state != AccountState::Error && state != AccountState::Invalid {
            return Err(format!(
                "Account state is {}, only Error/Invalid can re-login",
                state.as_str()
            ));
        }

        Self::re_login_account(account, &client, &solver).await;

        // Check state after re-login
        let new_state = account.state();
        if new_state == AccountState::Idle {
            Ok(())
        } else {
            Err(format!("Re-login failed, current state: {}", new_state.as_str()))
        }
    }

    /// Attempt to re-login accounts in Error state
    /// Success → Idle, failure → error_count++, ≥3 → Invalid
    async fn re_login_account(account: &Account, client: &DsClient, solver: &PowSolver) {
        let display_id = account.display_id().to_string();
        match try_init_account(&account.creds, client, solver).await {
            Ok(new_account) => {
                // Update token
                *account.token.write().unwrap() = new_account.token.read().unwrap().clone();
                account
                    .state
                    .store(AccountState::Idle as u8, Ordering::Relaxed);
                account.error_count.store(0, Ordering::Relaxed);
                info!(target: "ds_core::accounts", "Account {} re-login successful", display_id);
            }
            Err(e) => {
                let count = account.error_count.fetch_add(1, Ordering::Relaxed) + 1;
                if count >= MAX_ERROR_COUNT {
                    account
                        .state
                        .store(AccountState::Invalid as u8, Ordering::Relaxed);
                    error!(target: "ds_core::accounts", "Account {} re-login failed {} times, marked as Invalid: {}", display_id, count, e);
                } else {
                    warn!(target: "ds_core::accounts", "Account {} re-login failed (attempt {}): {}", display_id, count, e);
                }
            }
        }
    }

    /// Start background recovery task: scan Error accounts every 60 seconds and attempt re-login
    pub fn start_recovery_task(self: &Arc<Self>) {
        let pool = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;

                let client_opt = pool.client.read().await.clone();
                let solver_opt = pool.solver.read().await.clone();
                let (Some(client), Some(solver)) = (client_opt, solver_opt) else {
                    continue;
                };

                for entry in pool.accounts.iter() {
                    let account = entry.value();
                    if account.state() == AccountState::Error {
                        Self::re_login_account(account, &client, &solver).await;
                    }
                }
            }
        });
    }
}

async fn init_account(
    creds: &AccountConfig,
    client: &DsClient,
    solver: &PowSolver,
) -> Result<Account, PoolError> {
    try_init_account(creds, client, solver).await
}

async fn try_init_account(
    creds: &AccountConfig,
    client: &DsClient,
    solver: &PowSolver,
) -> Result<Account, PoolError> {
    // Validation: at least one of email or mobile must be non-empty
    if creds.email.is_empty() && creds.mobile.is_empty() {
        return Err(PoolError::Validation(
            "email and mobile cannot both be empty".to_string(),
        ));
    }

    let login_payload = LoginPayload {
        email: if creds.email.is_empty() {
            None
        } else {
            Some(creds.email.clone())
        },
        mobile: if creds.mobile.is_empty() {
            None
        } else {
            Some(creds.mobile.clone())
        },
        password: creds.password.clone(),
        area_code: if creds.area_code.is_empty() {
            None
        } else {
            Some(creds.area_code.clone())
        },
        device_id: String::new(),
        os: "web".to_string(),
    };

    let login_data = client.login(&login_payload).await?;
    debug!(
        target: "ds_core::client",
        "Login response: code={}, msg={}, user_id={}, email={:?}, mobile={:?}",
        login_data.code,
        login_data.msg,
        login_data.user.id,
        login_data.user.email,
        login_data.user.mobile_number
    );
    let token = login_data.user.token;

    let display_id = if creds.email.is_empty() {
        &creds.mobile
    } else {
        &creds.email
    };

    // Health check: create temporary session → send test completion → delete session
    let session_id = client.create_session(&token).await?;
    if let Err(e) = health_check(&token, &session_id, client, solver, "default", display_id).await {
        // Clean up session even if health check fails
        let _ = client.delete_session(&token, &session_id).await;
        return Err(e);
    }
    let _ = client.delete_session(&token, &session_id).await;

    Ok(Account {
        token: std::sync::RwLock::new(token.into()),
        email: creds.email.clone(),
        mobile: creds.mobile.clone(),
        state: AtomicU8::new(AccountState::Idle as u8),
        last_released: AtomicI64::new(0),
        error_count: AtomicU8::new(0),
        creds: creds.clone(),
    })
}

async fn health_check(
    token: &str,
    session_id: &str,
    client: &DsClient,
    solver: &PowSolver,
    model_type: &str,
    display_id: &str,
) -> Result<(), PoolError> {
    let start = std::time::Instant::now();
    let challenge = client
        .create_pow_challenge(token, "/api/v0/chat/completion")
        .await?;

    let result = solver.solve(&challenge)?;
    let pow_header = result.to_header();

    let payload = CompletionPayload {
        chat_session_id: session_id.to_string(),
        parent_message_id: None,
        model_type: model_type.to_string(),
        prompt: "Only reply with `Hello, world!`".to_string(),
        ref_file_ids: vec![],
        thinking_enabled: false,
        search_enabled: false,
        preempt: false,
    };

    let mut stream = client.completion(token, &pow_header, &payload).await?;
    // Consume stream and check if normal SSE was received (healthy accounts should have ready/response events)
    let mut data = Vec::new();
    while let Some(chunk) = stream.try_next().await? {
        data.extend_from_slice(&chunk);
    }

    let text = String::from_utf8_lossy(&data);

    // Detect if account is abnormal (muted / rate-limited, etc.)
    if text.contains(r#""biz_code":"#) {
        error!(
            target: "ds_core::accounts",
            "health_check detected business error: account={}, response={}",
            display_id,
            text.lines().find(|l| l.contains("biz_code")).unwrap_or(&text)
        );
        return Err(PoolError::Validation("Account abnormal (muted/limited)".into()));
    }

    // Check if SSE stream ended normally
    if !text.contains(r#""FINISHED""#) && !text.contains(r#""INCOMPLETE""#) {
        return Err(PoolError::Validation("SSE stream did not end normally".into()));
    }

    debug!(
        target: "ds_core::accounts",
        "health_check completed model_type={} account={} elapsed={:?}",
        model_type, display_id, start.elapsed()
    );
    Ok(())
}

//! Request statistics — lightweight atomic counters + periodic persistence + per-model breakdown

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use dashmap::DashMap;
use serde::Serialize;

use super::store::StoreManager;

/// Persistence interval: write to disk every 30 requests
const PERSIST_INTERVAL: u64 = 30;
/// Maximum number of request log entries
const LOG_CAPACITY: usize = 200;

/// Statistics counter for a single model
pub struct ModelStats {
    pub prompt_tokens: AtomicU64,
    pub completion_tokens: AtomicU64,
    pub requests: AtomicU64,
}

impl ModelStats {
    fn new() -> Self {
        Self {
            prompt_tokens: AtomicU64::new(0),
            completion_tokens: AtomicU64::new(0),
            requests: AtomicU64::new(0),
        }
    }
}

/// Statistics counter for a single API Key
pub struct KeyUsage {
    pub prompt_tokens: AtomicU64,
    pub completion_tokens: AtomicU64,
    pub requests: AtomicU64,
}

impl KeyUsage {
    fn new() -> Self {
        Self {
            prompt_tokens: AtomicU64::new(0),
            completion_tokens: AtomicU64::new(0),
            requests: AtomicU64::new(0),
        }
    }
}

/// Single request log entry
#[derive(Serialize, Clone)]
pub struct RequestLog {
    pub timestamp: u64,
    pub request_id: String,
    pub model: String,
    pub api_key: String,
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub latency_ms: u64,
    pub success: bool,
}

/// Request statistics counter
pub struct Stats {
    pub total_requests: AtomicU64,
    pub success_requests: AtomicU64,
    pub failed_requests: AtomicU64,
    pub total_latency_ms: AtomicU64,
    pub total_prompt_tokens: AtomicU64,
    pub total_completion_tokens: AtomicU64,
    pub start_time: Instant,
    /// total_requests value at the time of last persistence
    last_persisted: AtomicU64,
    /// Persistent storage
    store: Option<Arc<StoreManager>>,
    /// Per-model statistics
    pub model_stats: DashMap<String, ModelStats>,
    /// Per-API-Key statistics
    pub key_stats: DashMap<String, KeyUsage>,
    /// Request log ring buffer
    pub request_logs: Mutex<VecDeque<RequestLog>>,
}

impl Stats {
    /// Create Stats, optionally recovering from persisted data (including model/key dimension stats + request logs)
    pub fn new_with_store(store: Option<Arc<StoreManager>>) -> Self {
        let (
            total_requests,
            success_requests,
            failed_requests,
            prompt_tokens,
            completion_tokens,
            model_stats_data,
            key_stats_data,
            logs_data,
        ) = store.as_ref().map_or_else(
            || (0, 0, 0, 0, 0, HashMap::new(), HashMap::new(), Vec::new()),
            |s| {
                let st = futures::executor::block_on(s.load_stats());
                (
                    st.total_requests,
                    st.success_requests,
                    st.failed_requests,
                    st.total_prompt_tokens,
                    st.total_completion_tokens,
                    st.model_stats,
                    st.key_stats,
                    st.request_logs,
                )
            },
        );

        // Restore model statistics
        let model_stats: DashMap<String, ModelStats> = DashMap::new();
        for (model, data) in &model_stats_data {
            model_stats.insert(
                model.clone(),
                ModelStats {
                    prompt_tokens: AtomicU64::new(data.prompt_tokens),
                    completion_tokens: AtomicU64::new(data.completion_tokens),
                    requests: AtomicU64::new(data.requests),
                },
            );
        }

        // Restore key statistics
        let key_stats: DashMap<String, KeyUsage> = DashMap::new();
        for (key, data) in &key_stats_data {
            key_stats.insert(
                key.clone(),
                KeyUsage {
                    prompt_tokens: AtomicU64::new(data.prompt_tokens),
                    completion_tokens: AtomicU64::new(data.completion_tokens),
                    requests: AtomicU64::new(data.requests),
                },
            );
        }

        // Restore request logs (up to LOG_CAPACITY entries)
        let mut logs = VecDeque::with_capacity(LOG_CAPACITY);
        for entry in logs_data.iter().rev().take(LOG_CAPACITY).rev() {
            logs.push_back(super::stats::RequestLog {
                timestamp: entry.timestamp,
                request_id: entry.request_id.clone(),
                model: entry.model.clone(),
                api_key: entry.api_key.clone(),
                prompt_tokens: entry.prompt_tokens,
                completion_tokens: entry.completion_tokens,
                latency_ms: entry.latency_ms,
                success: entry.success,
            });
        }

        Self {
            total_requests: AtomicU64::new(total_requests),
            success_requests: AtomicU64::new(success_requests),
            failed_requests: AtomicU64::new(failed_requests),
            total_latency_ms: AtomicU64::new(0), // latency is not persisted
            total_prompt_tokens: AtomicU64::new(prompt_tokens),
            total_completion_tokens: AtomicU64::new(completion_tokens),
            start_time: Instant::now(),
            last_persisted: AtomicU64::new(total_requests),
            store,
            model_stats,
            key_stats,
            request_logs: Mutex::new(logs),
        }
    }

    /// Append a request log entry
    pub fn append_log(&self, log: RequestLog) {
        let mut logs = self.request_logs.lock().unwrap();
        if logs.len() >= LOG_CAPACITY {
            logs.pop_front();
        }
        logs.push_back(log);
    }

    /// Get recent request logs
    pub fn recent_logs(&self, limit: usize) -> Vec<RequestLog> {
        let logs = self.request_logs.lock().unwrap();
        logs.iter().rev().take(limit).cloned().collect()
    }

    /// Record token usage (including model + API Key dimensions)
    pub fn record_tokens_for_model_and_key(
        &self,
        model: &str,
        api_key: Option<&str>,
        prompt_tokens: u64,
        completion_tokens: u64,
    ) {
        self.total_prompt_tokens
            .fetch_add(prompt_tokens, Ordering::Relaxed);
        self.total_completion_tokens
            .fetch_add(completion_tokens, Ordering::Relaxed);
        // Record per model
        let ms = self
            .model_stats
            .entry(model.to_string())
            .or_insert_with(ModelStats::new);
        ms.prompt_tokens.fetch_add(prompt_tokens, Ordering::Relaxed);
        ms.completion_tokens
            .fetch_add(completion_tokens, Ordering::Relaxed);
        ms.requests.fetch_add(1, Ordering::Relaxed);
        // Record per API Key
        if let Some(key) = api_key {
            let ku = self
                .key_stats
                .entry(key.to_string())
                .or_insert_with(KeyUsage::new);
            ku.prompt_tokens.fetch_add(prompt_tokens, Ordering::Relaxed);
            ku.completion_tokens
                .fetch_add(completion_tokens, Ordering::Relaxed);
            ku.requests.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Record a completed request
    pub fn record_request(&self, success: bool, latency_ms: u64) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        if success {
            self.success_requests.fetch_add(1, Ordering::Relaxed);
        } else {
            self.failed_requests.fetch_add(1, Ordering::Relaxed);
        }
        self.total_latency_ms
            .fetch_add(latency_ms, Ordering::Relaxed);
        self.maybe_persist();
    }

    /// Check if persistence is needed
    fn maybe_persist(&self) {
        let total = self.total_requests.load(Ordering::Relaxed);
        let last = self.last_persisted.load(Ordering::Relaxed);
        if total - last >= PERSIST_INTERVAL
            && self
                .last_persisted
                .compare_exchange(last, total, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            self.persist_now();
        }
    }

    /// Immediately persist current statistics (including model/key dimensions + request logs)
    pub fn persist_now(&self) {
        if let Some(ref store) = self.store {
            let model_stats: HashMap<String, super::store::ModelStatsData> = self
                .model_stats
                .iter()
                .map(|r| {
                    (
                        r.key().clone(),
                        super::store::ModelStatsData {
                            prompt_tokens: r.value().prompt_tokens.load(Ordering::Relaxed),
                            completion_tokens: r.value().completion_tokens.load(Ordering::Relaxed),
                            requests: r.value().requests.load(Ordering::Relaxed),
                        },
                    )
                })
                .collect();
            let key_stats: HashMap<String, super::store::KeyStatsData> = self
                .key_stats
                .iter()
                .map(|r| {
                    let masked = if r.key().len() > 8 {
                        format!("{}***", &r.key()[..8])
                    } else {
                        "***".to_string()
                    };
                    (
                        masked,
                        super::store::KeyStatsData {
                            prompt_tokens: r.value().prompt_tokens.load(Ordering::Relaxed),
                            completion_tokens: r.value().completion_tokens.load(Ordering::Relaxed),
                            requests: r.value().requests.load(Ordering::Relaxed),
                        },
                    )
                })
                .collect();
            let logs = {
                let guard = self.request_logs.lock().unwrap();
                guard
                    .iter()
                    .map(|l| super::store::RequestLogData {
                        timestamp: l.timestamp,
                        request_id: l.request_id.clone(),
                        model: l.model.clone(),
                        api_key: l.api_key.clone(),
                        prompt_tokens: l.prompt_tokens,
                        completion_tokens: l.completion_tokens,
                        latency_ms: l.latency_ms,
                        success: l.success,
                    })
                    .collect()
            };
            let st = super::store::StatsStore {
                total_requests: self.total_requests.load(Ordering::Relaxed),
                success_requests: self.success_requests.load(Ordering::Relaxed),
                failed_requests: self.failed_requests.load(Ordering::Relaxed),
                total_prompt_tokens: self.total_prompt_tokens.load(Ordering::Relaxed),
                total_completion_tokens: self.total_completion_tokens.load(Ordering::Relaxed),
                model_stats,
                key_stats,
                request_logs: logs,
            };
            let store = store.clone();
            tokio::spawn(async move {
                if let Err(e) = store.save_stats(&st).await {
                    log::warn!(target: "stats", "Persistence failed: {}", e);
                }
            });
        }
    }

    /// Generate a statistics snapshot
    pub fn snapshot(&self) -> StatsSnapshot {
        let total = self.total_requests.load(Ordering::Relaxed);
        let success = self.success_requests.load(Ordering::Relaxed);
        let failed = self.failed_requests.load(Ordering::Relaxed);
        let total_latency = self.total_latency_ms.load(Ordering::Relaxed);
        let uptime_secs = self.start_time.elapsed().as_secs();

        let prompt_tokens = self.total_prompt_tokens.load(Ordering::Relaxed);
        let completion_tokens = self.total_completion_tokens.load(Ordering::Relaxed);

        StatsSnapshot {
            total_requests: total,
            success_requests: success,
            failed_requests: failed,
            avg_latency_ms: total_latency.checked_div(total).unwrap_or(0),
            total_prompt_tokens: prompt_tokens,
            total_completion_tokens: completion_tokens,
            uptime_secs,
            models: self
                .model_stats
                .iter()
                .map(|r| {
                    (
                        r.key().clone(),
                        ModelStatsSnapshot {
                            prompt_tokens: r.value().prompt_tokens.load(Ordering::Relaxed),
                            completion_tokens: r.value().completion_tokens.load(Ordering::Relaxed),
                            requests: r.value().requests.load(Ordering::Relaxed),
                        },
                    )
                })
                .collect(),
            keys: self.key_stats_snapshot(),
        }
    }

    /// Generate API Key dimension statistics snapshot
    pub fn key_stats_snapshot(&self) -> HashMap<String, KeyUsageSnapshot> {
        self.key_stats
            .iter()
            .map(|r| {
                // Mask: only show first 8 characters
                let masked = if r.key().len() > 8 {
                    format!("{}***", &r.key()[..8])
                } else {
                    "***".to_string()
                };
                (
                    masked,
                    KeyUsageSnapshot {
                        prompt_tokens: r.value().prompt_tokens.load(Ordering::Relaxed),
                        completion_tokens: r.value().completion_tokens.load(Ordering::Relaxed),
                        requests: r.value().requests.load(Ordering::Relaxed),
                    },
                )
            })
            .collect()
    }
}

#[derive(Serialize)]
pub struct StatsSnapshot {
    pub total_requests: u64,
    pub success_requests: u64,
    pub failed_requests: u64,
    pub avg_latency_ms: u64,
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub uptime_secs: u64,
    pub models: HashMap<String, ModelStatsSnapshot>,
    pub keys: HashMap<String, KeyUsageSnapshot>,
}

#[derive(Serialize)]
pub struct ModelStatsSnapshot {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub requests: u64,
}

#[derive(Serialize)]
pub struct KeyUsageSnapshot {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub requests: u64,
}

/// Request timing guard; automatically records statistics on Drop
/// If mark_success/mark_failure is not called, Drop defaults to recording as a failure
pub struct RequestTimer {
    stats: Arc<Stats>,
    start: Instant,
    marked: bool,
}

impl RequestTimer {
    pub fn new(stats: &Arc<Stats>) -> Self {
        Self {
            stats: Arc::clone(stats),
            start: Instant::now(),
            marked: false,
        }
    }
}

impl Drop for RequestTimer {
    fn drop(&mut self) {
        if !self.marked {
            let elapsed = self.start.elapsed();
            let latency = elapsed.as_secs() * 1000 + u64::from(elapsed.subsec_millis());
            self.stats.record_request(false, latency);
        }
    }
}

impl RequestTimer {
    /// Mark request as successful and record statistics
    pub fn mark_success(mut self) {
        let elapsed = self.start.elapsed();
        let latency = elapsed.as_secs() * 1000 + u64::from(elapsed.subsec_millis());
        self.stats.record_request(true, latency);
        self.marked = true;
    }

    /// Mark request as failed and record statistics
    pub fn mark_failure(mut self) {
        let elapsed = self.start.elapsed();
        let latency = elapsed.as_secs() * 1000 + u64::from(elapsed.subsec_millis());
        self.stats.record_request(false, latency);
        self.marked = true;
    }
}

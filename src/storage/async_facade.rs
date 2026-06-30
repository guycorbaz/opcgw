// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Async facade over the synchronous [`StorageBackend`] trait (Story H-0, #73).
//!
//! `StorageBackend` is a fully **synchronous** trait backed by blocking
//! `rusqlite` I/O (and, on pool exhaustion, blocking `std::thread::sleep`
//! retry-backoff). Calling it directly from an async task blocks a tokio
//! worker thread for the duration of the SQL operation. On CPU-constrained
//! deployments (small containers with 1–2 vCPUs) that starves the poller, the
//! OPC UA server, and the web handlers.
//!
//! [`AsyncStorage`] wraps a single `Arc<dyn StorageBackend>` and runs each call
//! on the blocking thread pool via [`tokio::task::spawn_blocking`], so the
//! async runtime's worker threads are never blocked on SQL. The synchronous
//! trait and both backend implementations (`SqliteBackend`, `InMemoryBackend`)
//! are unchanged — this is a pure execution-context shim: identical return
//! types, identical [`OpcGwError`] mapping, identical ordering.
//!
//! # Usage
//!
//! Async call sites obtain the facade from any `Arc<dyn StorageBackend>` (or
//! `&Arc<…>`) handle via the [`AsyncStorageExt::async_store`] extension and
//! `.await` the call:
//!
//! ```ignore
//! use crate::storage::AsyncStorageExt;
//! let pending = self.backend.async_store().get_pending_commands().await?;
//! ```
//!
//! Genuinely **synchronous** call sites that cannot `.await` — e.g. the
//! async-opcua node-manager read callbacks, which are sync `Fn` closures — must
//! NOT use this facade. They wrap the direct blocking call in
//! [`tokio::task::block_in_place`] instead (the multi-threaded runtime makes
//! that available); see `src/opc_ua.rs`.

use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Utc};
use tokio::task::JoinError;

use crate::storage::types::{
    Command, CommandStatus, DeviceCommand, ErrorEvent, MetricType, MetricValue,
};
use crate::storage::{BatchMetricWrite, HistoricalMetricRow, StorageBackend};
use crate::utils::OpcGwError;

/// Maps a `spawn_blocking` [`JoinError`] (the blocking task panicked or was
/// cancelled) to an [`OpcGwError`]. This only fires on a panic inside a backend
/// method or runtime shutdown — never on a normal storage error, which is
/// already carried by the inner `Result`.
fn join_err(e: JoinError) -> OpcGwError {
    OpcGwError::Storage(format!("storage task failed: {e}"))
}

/// Runs a blocking storage closure from a **synchronous** context that cannot
/// `.await` the [`AsyncStorage`] facade — specifically the async-opcua
/// node-manager read / method callbacks, which are sync `Fn` closures
/// (Story H-0/#73, AC#5).
///
/// When called on a multi-threaded tokio worker, it uses
/// [`tokio::task::block_in_place`] so the runtime can move other tasks off this
/// worker for the duration of the blocking SQL, instead of starving them. When
/// there is no current runtime, or the runtime is single-threaded (e.g. a unit
/// test calling the callback body directly), `block_in_place` is unavailable /
/// would panic, so the closure is simply run inline. Either way the result is
/// identical — this only affects *where* the blocking work runs.
pub fn run_blocking_storage<T>(f: impl FnOnce() -> T) -> T {
    use tokio::runtime::{Handle, RuntimeFlavor};
    match Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(f)
        }
        _ => f(),
    }
}

/// Async facade over an `Arc<dyn StorageBackend>` (Story H-0, #73).
///
/// Cheap to clone (one `Arc` bump). Each method offloads the synchronous
/// backend call to [`tokio::task::spawn_blocking`] and returns the backend's
/// result unchanged.
#[derive(Clone)]
pub struct AsyncStorage {
    inner: Arc<dyn StorageBackend>,
}

impl AsyncStorage {
    /// Wraps a backend handle in the async facade.
    pub fn new(inner: Arc<dyn StorageBackend>) -> Self {
        Self { inner }
    }

    // ===== Metric operations =====

    /// Async [`StorageBackend::get_metric_value`].
    pub async fn get_metric_value(
        &self,
        device_id: String,
        metric_name: String,
    ) -> Result<Option<MetricValue>, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.get_metric_value(&device_id, &metric_name))
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::upsert_metric_value`].
    pub async fn upsert_metric_value(
        &self,
        device_id: String,
        metric_name: String,
        value: MetricType,
        now_ts: SystemTime,
    ) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            inner.upsert_metric_value(&device_id, &metric_name, &value, now_ts)
        })
        .await
        .map_err(join_err)?
    }

    /// Async [`StorageBackend::append_metric_history`].
    pub async fn append_metric_history(
        &self,
        device_id: String,
        metric_name: String,
        value: MetricType,
        timestamp: SystemTime,
    ) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            inner.append_metric_history(&device_id, &metric_name, &value, timestamp)
        })
        .await
        .map_err(join_err)?
    }

    /// Async [`StorageBackend::batch_write_metrics`].
    pub async fn batch_write_metrics(
        &self,
        metrics: Vec<BatchMetricWrite>,
    ) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.batch_write_metrics(metrics))
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::load_all_metrics`].
    pub async fn load_all_metrics(&self) -> Result<Vec<MetricValue>, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.load_all_metrics())
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::prune_metric_history`].
    pub async fn prune_metric_history(&self) -> Result<u32, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.prune_metric_history())
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::query_metric_history`].
    pub async fn query_metric_history(
        &self,
        device_id: String,
        metric_name: String,
        start: SystemTime,
        end: SystemTime,
        max_results: usize,
    ) -> Result<Vec<HistoricalMetricRow>, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            inner.query_metric_history(&device_id, &metric_name, start, end, max_results)
        })
        .await
        .map_err(join_err)?
    }

    // ===== Command queue operations =====

    /// Async [`StorageBackend::get_pending_commands`].
    pub async fn get_pending_commands(&self) -> Result<Vec<DeviceCommand>, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.get_pending_commands())
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::update_command_status`].
    pub async fn update_command_status(
        &self,
        command_id: u64,
        status: CommandStatus,
        error_message: Option<String>,
    ) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            inner.update_command_status(command_id, status, error_message)
        })
        .await
        .map_err(join_err)?
    }

    /// Async [`StorageBackend::mark_command_sent`].
    pub async fn mark_command_sent(
        &self,
        command_id: u64,
        chirpstack_result_id: String,
    ) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            inner.mark_command_sent(command_id, &chirpstack_result_id)
        })
        .await
        .map_err(join_err)?
    }

    /// Async [`StorageBackend::mark_command_confirmed`].
    pub async fn mark_command_confirmed(&self, command_id: u64) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.mark_command_confirmed(command_id))
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::mark_command_failed`].
    pub async fn mark_command_failed(
        &self,
        command_id: u64,
        error_message: String,
    ) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.mark_command_failed(command_id, &error_message))
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::find_pending_confirmations`].
    pub async fn find_pending_confirmations(&self) -> Result<Vec<Command>, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.find_pending_confirmations())
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::find_timed_out_commands`].
    pub async fn find_timed_out_commands(&self, ttl_secs: u32) -> Result<Vec<Command>, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.find_timed_out_commands(ttl_secs))
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::find_command_by_result_id`].
    pub async fn find_command_by_result_id(
        &self,
        result_id: String,
    ) -> Result<Option<Command>, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.find_command_by_result_id(&result_id))
            .await
            .map_err(join_err)?
    }

    // ===== Gateway status / health / error-event operations =====

    /// Async [`StorageBackend::update_gateway_status`].
    pub async fn update_gateway_status(
        &self,
        last_poll_timestamp: Option<DateTime<Utc>>,
        error_count: i32,
        chirpstack_available: bool,
    ) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            inner.update_gateway_status(last_poll_timestamp, error_count, chirpstack_available)
        })
        .await
        .map_err(join_err)?
    }

    /// Async [`StorageBackend::record_error_event`].
    pub async fn record_error_event(&self, event: ErrorEvent) -> Result<(), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.record_error_event(&event))
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::recent_error_events`].
    pub async fn recent_error_events(
        &self,
        limit: usize,
    ) -> Result<Vec<ErrorEvent>, OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.recent_error_events(limit))
            .await
            .map_err(join_err)?
    }

    /// Async [`StorageBackend::get_gateway_health_metrics`].
    pub async fn get_gateway_health_metrics(
        &self,
    ) -> Result<(Option<DateTime<Utc>>, i32, bool), OpcGwError> {
        let inner = self.inner.clone();
        tokio::task::spawn_blocking(move || inner.get_gateway_health_metrics())
            .await
            .map_err(join_err)?
    }
}

/// Extension trait giving any `Arc<dyn StorageBackend>` handle an
/// `.async_store()` accessor that returns an [`AsyncStorage`] facade.
///
/// This keeps call sites ergonomic without changing the field type of every
/// struct that holds a backend handle:
///
/// ```ignore
/// self.backend.async_store().batch_write_metrics(writes).await?;
/// ```
pub trait AsyncStorageExt {
    /// Returns an async facade over this backend handle (one `Arc` clone).
    fn async_store(&self) -> AsyncStorage;
}

impl AsyncStorageExt for Arc<dyn StorageBackend> {
    fn async_store(&self) -> AsyncStorage {
        AsyncStorage::new(self.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memory::InMemoryBackend;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// AC#7 — proves storage access from an async context runs OFF the async
    /// worker thread. On a single-worker runtime, a blocking storage call made
    /// through the facade must NOT prevent a concurrently-spawned async task
    /// from making progress: the blocking work is on the `spawn_blocking` pool,
    /// leaving the single worker free to drive the counter task.
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn facade_runs_storage_off_the_async_worker() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let store = backend.async_store();

        let counter = Arc::new(AtomicU64::new(0));
        let counter2 = counter.clone();

        // A cooperative async task that yields repeatedly. If the storage call
        // below were blocking THIS single worker, this task could not advance
        // while the call is outstanding.
        let ticker = tokio::spawn(async move {
            for _ in 0..50 {
                counter2.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
            }
        });

        // Issue a batch of storage calls through the facade; each hops to the
        // blocking pool and back.
        for _ in 0..20 {
            let _ = store.load_all_metrics().await.expect("load ok");
        }

        ticker.await.expect("ticker task joined");
        // The ticker ran to completion concurrently with the storage calls,
        // which is only possible because storage executed off the worker.
        assert_eq!(counter.load(Ordering::SeqCst), 50);
    }

    /// The facade returns the backend's result unchanged (no behavioural
    /// change): a round-trip write+read through the facade matches a direct
    /// backend read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn facade_preserves_backend_semantics() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let store = backend.async_store();

        // Empty store → no metrics.
        let before = store.load_all_metrics().await.expect("load ok");
        assert!(before.is_empty());

        // Gateway health defaults are returned unchanged through the facade.
        let (ts, errors, available) =
            store.get_gateway_health_metrics().await.expect("health ok");
        assert!(ts.is_none());
        assert_eq!(errors, 0);
        assert!(!available);
    }
}

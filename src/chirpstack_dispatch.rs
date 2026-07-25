// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Event-driven command dispatch (Story J-1 / CR #136).
//!
//! Before J-1, queued OPC UA commands were delivered to ChirpStack by
//! `ChirpstackPoller::poll_metrics`, which ran the drain at the head of every
//! poll cycle. A command written over OPC UA therefore waited up to a full
//! `chirpstack.polling_frequency` interval before it even reached ChirpStack's
//! queue — long enough that an operator reads the empty device queue as a
//! delivery failure (the E-0 AC#10 valve-test symptom).
//!
//! This module decouples dispatch from the poll cadence. [`CommandDispatcher`]
//! is a dedicated task that awaits a shared [`tokio::sync::Notify`]; the OPC UA
//! write path fires that `Notify` on every successful (`Good`) `set_command`
//! enqueue, so the pending command is drained within seconds of the write.
//! The metrics poll no longer delivers commands at all.
//!
//! # Design invariants
//!
//! - **Single-owner delivery (AC#4).** Exactly one task drains. `deliver_one`
//!   marks a row `Sent` only *after* the enqueue succeeds, and each drain
//!   re-reads `status = 'Pending'`, so a second concurrent drainer would
//!   double-enqueue. The poll loop no longer drains (AC#3) and the dispatcher
//!   is a single task, so no duplicate downlink is possible.
//! - **No lost wakeup (AC#6).** `Notify` stores a single permit, so a command
//!   enqueued while a drain is in flight makes the *next* `notified()` return
//!   immediately. Each drain empties the queue, so one wakeup covering N
//!   signals delivers all N.
//! - **Startup / respawn drain (AC#5).** The dispatcher drains once before it
//!   first awaits the signal, so commands persisted `Pending` before boot or
//!   carried across a soft-restart are delivered without a fresh write.
//! - **Fresh state on Apply (AC#7).** The dispatcher is spawned inside
//!   `spawn_data_plane` under the per-cycle `restart_token` with a fresh
//!   `Notify` shared with the freshly-built `OpcUa`, so an Apply-respawn never
//!   cross-signals across generations.
//!
//! Delivery reuses the existing, unit-tested machinery in
//! [`crate::chirpstack`]: [`DownlinkSink`], [`deliver_one`], and
//! [`find_command_cfg`] — nothing about the map/enqueue/status logic is
//! reinvented here (AC#9).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, trace, warn};

/// Base backoff before re-driving a drain that could not read the
/// pending-command queue (storage error). Only engaged on the read-error path
/// (J-1 review P1); the healthy path never waits on it.
const DRAIN_RETRY_BACKOFF_BASE: Duration = Duration::from_secs(2);

/// Cap for the escalating read-error backoff (J-1 review iter-2 P5). Under a
/// sustained storage outage (e.g. #152 NAS SQLite contention) the retry
/// interval doubles each consecutive failure up to this cap, so the dispatcher's
/// **self-driven** re-drive (the timer arm) happens at most once per cap
/// interval instead of every 2 s — bounding wasted work and the self-driven
/// share of `command_dispatch_drain_error` WARNs (project WARN-budget
/// discipline, cf. #144/#149). Note: a drain triggered by the **signal** arm
/// (an OPC UA `Good` write) is not gated by this backoff and still WARNs once
/// per failed drain; that path is naturally self-limiting because a storage
/// outage also fails the write's own `queue_command`, so few signals fire.
const DRAIN_RETRY_BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Computes the escalating read-error backoff for `n` consecutive failures
/// (`n >= 1`): `BASE * 2^(min(n-1, 5))`, capped at `MAX`. `2s, 4s, 8s, 16s,
/// 32s, 60s(cap)…`.
fn drain_retry_backoff(consecutive_errors: u32) -> Duration {
    let shift = consecutive_errors.saturating_sub(1).min(5);
    (DRAIN_RETRY_BACKOFF_BASE * 2u32.pow(shift)).min(DRAIN_RETRY_BACKOFF_MAX)
}

use chirpstack_api::api::{DeviceQueueItem, EnqueueDeviceQueueItemRequest};
use tonic::Request;

use crate::chirpstack::{
    create_device_client_from_config, deliver_one, find_command_cfg, DownlinkSink,
};
use crate::config::AppConfig;
use crate::storage::{AsyncStorageExt, StorageBackend};
use crate::utils::OpcGwError;

/// Production [`DownlinkSink`]: enqueues a downlink to ChirpStack over gRPC.
///
/// Builds its `DeviceServiceClient` through the shared
/// [`create_device_client_from_config`] factory (AC#8) so it shares exactly one
/// connect/auth path with the poller — no duplicated auth logic, no second
/// hand-rolled connect, and the `api_token` never appears in a log line. The
/// body mirrors the former `impl DownlinkSink for ChirpstackPoller` (AC#9).
pub(crate) struct ChirpStackDownlinkSink {
    config: AppConfig,
}

impl ChirpStackDownlinkSink {
    pub(crate) fn new(config: AppConfig) -> Self {
        Self { config }
    }
}

#[async_trait::async_trait]
impl DownlinkSink for ChirpStackDownlinkSink {
    async fn enqueue_downlink(&self, item: DeviceQueueItem) -> Result<String, OpcGwError> {
        trace!(queue_item = ?item, "Enqueue downlink to ChirpStack");
        let request = Request::new(EnqueueDeviceQueueItemRequest {
            queue_item: Some(item),
            flush_queue: false,
        });

        // Client-creation failure is a handled error, never a panic.
        let mut device_client = create_device_client_from_config(&self.config).await?;
        match device_client.enqueue(request).await {
            Ok(response) => {
                let inner_response = response.into_inner();
                trace!(response = ?inner_response, "Downlink enqueued");
                // Capture the queue-item UUID (E-3 correlation key). It must not
                // be empty in normal operation; if ChirpStack ever returns an
                // empty id, confirmation correlation falls back to the timeout
                // sweep for this command (handled inside `deliver_one`).
                Ok(inner_response.id)
            }
            Err(e) => {
                error!(error = %e, "Error enqueueing device request");
                Err(OpcGwError::ChirpStack(
                    "Error enqueuing request".to_string(),
                ))
            }
        }
    }
}

/// Drains **all** currently-`Pending` commands and delivers each via `sink`.
///
/// This is the relocated body of the former
/// `ChirpstackPoller::{process_command_queue, deliver_command}` (Story E-0),
/// now a free function so both the dispatcher's production path and its tests
/// (with a `MockSink`) drive the exact same code (AC#9/AC#10).
///
/// Error handling matches the pre-J-1 contract with one deliberate change: a
/// `get_pending_commands` failure is **logged and swallowed**, not propagated.
/// Under the old head-of-poll call a storage error aborted the poll cycle (`?`);
/// here the dispatcher is a long-lived task, and a returned error would strand
/// every future command until the next signal at best (or kill the task at
/// worst). Per-command failures never abort the batch (the existing
/// `deliver_one` contract) and are terminal (`deliver_one` marks a failed or
/// unmappable enqueue `Failed`, so they are never left `Pending`).
///
/// # Returns
///
/// `true` if the pending-command queue was read successfully (the drain ran to
/// completion), `false` if `get_pending_commands` errored and the drain was
/// aborted. J-1 review (P1): the caller uses `false` to schedule a **bounded
/// retry** — otherwise a transient storage error (e.g. #152 NAS SQLite
/// contention) consumes the wakeup permit and strands every currently-`Pending`
/// command until an unrelated future write happens to fire a new signal (the
/// timeout sweep only rescues `Sent` rows, never `Pending`).
pub(crate) async fn drain_pending_commands(
    sink: &dyn DownlinkSink,
    backend: &Arc<dyn StorageBackend>,
    config: &AppConfig,
    cancel_token: &CancellationToken,
) -> bool {
    let pending = match backend.async_store().get_pending_commands().await {
        Ok(pending) => pending,
        Err(e) => {
            warn!(
                event = "command_dispatch_drain_error",
                error = %e,
                "failed to read pending commands; scheduling a bounded retry"
            );
            return false;
        }
    };
    if pending.is_empty() {
        return true;
    }
    debug!(
        event = "command_dispatch_drain",
        count = pending.len(),
        "dispatching pending device commands"
    );
    for command in pending {
        // J-1 review iter-2 P6: make the drain cancellation-aware so teardown /
        // Apply is responsive under a large backlog + slow sink. The check is
        // between commands, so a single in-flight `deliver_one` with a slow gRPC
        // enqueue can still run to completion — that residual is bounded by
        // `join_data_plane`'s 10 s force-abort backstop. Matches the existing
        // `is_cancelled()` pattern in the poller's pagination loops.
        // Commands not yet delivered stay `Pending` and are picked up by the
        // next generation's startup drain — no loss. The check is BETWEEN
        // commands, so a command already handed to `deliver_one` finishes its
        // enqueue+status write (never left half-delivered).
        if cancel_token.is_cancelled() {
            debug!(
                event = "command_dispatch_drain_cancelled",
                "drain interrupted by shutdown; remaining pending commands deferred to next startup drain"
            );
            break;
        }
        // Resolve the per-command class + confirmed flag from config BEFORE the
        // await so the borrow of `config` does not cross the await point.
        let (command_class, confirmed) =
            match find_command_cfg(&config.application_list, &command.device_id, command.f_port) {
                Some(cfg) => (cfg.command_class.clone(), cfg.command_confirmed),
                None => (None, false),
            };
        deliver_one(sink, backend, command_class.as_deref(), confirmed, &command).await;
    }
    true
}

/// Dedicated event-driven command-dispatch task (Story J-1 / CR #136).
///
/// Awaits [`Self::dispatch_signal`] (fired by the OPC UA write path on a `Good`
/// enqueue) and drains the pending-command queue, fully decoupling command
/// delivery from the metrics-poll cadence. Mirrors the sibling command tasks
/// (`CommandStatusPoller`, `CommandTimeoutHandler`) in construction and
/// `tokio::select!` shutdown shape.
pub struct CommandDispatcher {
    /// Configuration — used to resolve each command's class binding.
    config: AppConfig,
    /// Shared storage backend the pending-command queue lives in.
    backend: Arc<dyn StorageBackend>,
    /// Cancellation token for graceful shutdown (SIGINT/SIGTERM or Apply).
    cancel_token: CancellationToken,
    /// Wakeup signalled by `OpcUa::set_command` on a successful enqueue.
    dispatch_signal: Arc<Notify>,
    /// Downlink sink. Production = [`ChirpStackDownlinkSink`]; tests inject a
    /// `MockSink`. Held as a trait object (rather than the dispatcher itself
    /// implementing `DownlinkSink`) precisely so AC#10's tests can drive the
    /// real `run`/drain loop without a live gRPC server.
    sink: Arc<dyn DownlinkSink>,
}

impl CommandDispatcher {
    /// Creates a dispatcher with the production ChirpStack gRPC sink.
    ///
    /// Mirrors the `CommandStatusPoller::new` / `CommandTimeoutHandler::new`
    /// constructors so the `spawn_data_plane` wiring is uniform.
    pub fn new(
        config: &AppConfig,
        backend: Arc<dyn StorageBackend>,
        cancel_token: CancellationToken,
        dispatch_signal: Arc<Notify>,
    ) -> Self {
        debug!("Creating CommandDispatcher for event-driven command delivery");
        let sink: Arc<dyn DownlinkSink> = Arc::new(ChirpStackDownlinkSink::new(config.clone()));
        Self {
            config: config.clone(),
            backend,
            cancel_token,
            dispatch_signal,
            sink,
        }
    }

    /// Test-only constructor that injects a custom [`DownlinkSink`] (a
    /// `MockSink`) so AC#10's dispatch tests run without live gRPC.
    #[cfg(test)]
    fn with_sink(
        config: AppConfig,
        backend: Arc<dyn StorageBackend>,
        cancel_token: CancellationToken,
        dispatch_signal: Arc<Notify>,
        sink: Arc<dyn DownlinkSink>,
    ) -> Self {
        Self {
            config,
            backend,
            cancel_token,
            dispatch_signal,
            sink,
        }
    }

    /// Delivers every currently-pending command via the configured sink.
    ///
    /// Returns `false` if the pending-queue read failed (→ the caller schedules
    /// a bounded retry, J-1 review P1), `true` otherwise (including a
    /// cancellation-interrupted drain).
    async fn drain_all(&self) -> bool {
        drain_pending_commands(
            self.sink.as_ref(),
            &self.backend,
            &self.config,
            &self.cancel_token,
        )
        .await
    }

    /// Dispatch loop.
    ///
    /// 1. **Startup drain (AC#5)** — deliver anything already `Pending` before
    ///    awaiting the first signal.
    /// 2. `select!` over cancellation (graceful shutdown, AC#7) and the
    ///    dispatch signal; each signal drains the whole queue (AC#6).
    /// 3. **Bounded read-error retry (J-1 review P1 + iter-2 P5)** — when a drain
    ///    could not read the queue (storage error), an **escalating** backoff
    ///    timer arm (`drain_retry_backoff`, `2s→…→60s cap`) is armed so the
    ///    drain is re-driven without waiting for an unrelated future write, and a
    ///    stuck backend's self-driven re-drive backs off toward the cap instead
    ///    of hammering every 2 s. On a healthy drain that timer is `pending()`
    ///    (never fires) and the streak resets, so the happy path stays purely
    ///    event-driven — no cadence.
    ///
    /// # Returns
    ///
    /// `Ok(())` on graceful shutdown.
    pub async fn run(&mut self) -> Result<(), OpcGwError> {
        debug!("Starting CommandDispatcher (event-driven command dispatch)");

        // AC#5: drain commands persisted before this task started (pre-boot or
        // carried across a soft-restart) without needing a fresh OPC UA write.
        let mut drain_ok = self.drain_all().await;
        // Consecutive read-error count driving the escalating retry backoff.
        let mut error_streak: u32 = if drain_ok { 0 } else { 1 };

        loop {
            // J-1 review P1/P5: only when the previous drain hit a storage
            // read-error do we fall back to an escalating backoff timer; a
            // healthy drain leaves this `pending()` so we wait purely on the
            // signal.
            let retry = async {
                if drain_ok {
                    std::future::pending::<()>().await
                } else {
                    tokio::time::sleep(drain_retry_backoff(error_streak)).await
                }
            };

            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("CommandDispatcher shutting down");
                    return Ok(());
                }
                _ = self.dispatch_signal.notified() => {
                    // AC#6 coalescing: one wakeup drains ALL currently-pending
                    // rows. The single stored `Notify` permit covers a command
                    // enqueued during the drain (→ immediate re-drain next loop),
                    // so no wakeup is lost even under a burst of writes.
                    drain_ok = self.drain_all().await;
                    error_streak = if drain_ok { 0 } else { error_streak.saturating_add(1) };
                }
                _ = retry => {
                    // The previous drain could not read the queue; re-drive it
                    // after the escalating backoff so a transient storage fault
                    // never strands a `Pending` command indefinitely, while a
                    // sustained outage backs off toward the cap.
                    drain_ok = self.drain_all().await;
                    error_streak = if drain_ok { 0 } else { error_streak.saturating_add(1) };
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*; // brings `Duration` (module-level) into test scope too
    use crate::storage::memory::InMemoryBackend;
    use crate::storage::{CommandStatus, DeviceCommand};
    use figment::providers::{Format, Toml};
    use figment::Figment;

    /// Minimal AppConfig fixture (same source as `chirpstack::tests`). The
    /// dispatch tests use device ids NOT present in this config so
    /// `find_command_cfg` returns `None` → raw-byte fallback, making delivery
    /// deterministic regardless of the fixture's application list.
    fn test_config() -> AppConfig {
        let config_path =
            std::env::var("CONFIG_PATH").unwrap_or_else(|_| "tests/config/config.toml".to_string());
        Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load test configuration")
    }

    /// Stub [`DownlinkSink`] recording enqueued items (mirrors the
    /// `chirpstack::tests::MockSink`; kept local so this module's tests are
    /// self-contained across the module boundary — inline test-harness
    /// duplication is the accepted pattern here, see issue #102).
    struct MockSink {
        fail: bool,
        calls: std::sync::Mutex<Vec<DeviceQueueItem>>,
    }

    impl MockSink {
        fn new(fail: bool) -> Self {
            Self {
                fail,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<DeviceQueueItem> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl DownlinkSink for MockSink {
        async fn enqueue_downlink(&self, item: DeviceQueueItem) -> Result<String, OpcGwError> {
            self.calls.lock().unwrap().push(item);
            if self.fail {
                Err(OpcGwError::ChirpStack("mock enqueue failure".to_string()))
            } else {
                Ok("qid-mock-0001".to_string())
            }
        }
    }

    fn device_command(id: u64, device_id: &str, f_port: u8, payload: Vec<u8>) -> DeviceCommand {
        DeviceCommand {
            id,
            device_id: device_id.to_string(),
            payload,
            f_port,
            status: CommandStatus::Pending,
            created_at: chrono::Utc::now(),
            error_message: None,
            command_name: None,
        }
    }

    /// Poll `command_status_for_test` until `id` reaches `want`, or panic after
    /// ~1 s. The short sleeps yield to the spawned `run` task on the
    /// current-thread test runtime.
    async fn wait_for_status(backend: &InMemoryBackend, id: u64, want: CommandStatus) {
        for _ in 0..200 {
            if let Some((status, _)) = backend.command_status_for_test(id) {
                if status == want {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("command {id} did not reach {want:?} within timeout");
    }

    /// AC#10(a): a queued command, once the dispatcher is signalled, is
    /// delivered exactly once and transitions `Pending → Sent`.
    #[tokio::test]
    async fn dispatch_delivers_a_queued_command() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, "dispatch-dev-A", 10, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;

        let sink = Arc::new(MockSink::new(false));
        let signal = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let mut dispatcher = CommandDispatcher::with_sink(
            test_config(),
            dyn_backend,
            cancel.clone(),
            signal.clone(),
            sink.clone(),
        );

        let handle = tokio::spawn(async move { dispatcher.run().await });
        signal.notify_one();

        wait_for_status(&backend, cmd_id, CommandStatus::Sent).await;
        assert_eq!(
            sink.calls().len(),
            1,
            "exactly one downlink must be enqueued"
        );

        cancel.cancel();
        let _ = handle.await;
    }

    /// AC#10(b) / AC#5: a command already `Pending` at task start is delivered
    /// by the startup drain, WITHOUT any signal ever being fired.
    #[tokio::test]
    async fn startup_drain_delivers_preexisting_command() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, "dispatch-dev-B", 7, vec![42]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;

        let sink = Arc::new(MockSink::new(false));
        let signal = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let mut dispatcher = CommandDispatcher::with_sink(
            test_config(),
            dyn_backend,
            cancel.clone(),
            signal.clone(),
            sink.clone(),
        );

        let handle = tokio::spawn(async move { dispatcher.run().await });

        // No notify_one() — only the startup drain can deliver this.
        wait_for_status(&backend, cmd_id, CommandStatus::Sent).await;
        assert_eq!(
            sink.calls().len(),
            1,
            "startup drain must deliver the pre-existing command"
        );

        cancel.cancel();
        let _ = handle.await;
    }

    /// AC#10(c) / AC#6: a burst of 3 pending commands is fully delivered from a
    /// single signal, with no command delivered twice.
    #[tokio::test]
    async fn burst_of_commands_all_delivered_once() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        let mut ids = Vec::new();
        for (i, dev) in ["dispatch-c1", "dispatch-c2", "dispatch-c3"]
            .iter()
            .enumerate()
        {
            backend
                .queue_command(device_command(0, dev, 10, vec![i as u8]))
                .unwrap();
        }
        for cmd in backend.get_pending_commands().unwrap() {
            ids.push(cmd.id);
        }
        assert_eq!(ids.len(), 3);

        let sink = Arc::new(MockSink::new(false));
        let signal = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let mut dispatcher = CommandDispatcher::with_sink(
            test_config(),
            dyn_backend,
            cancel.clone(),
            signal.clone(),
            sink.clone(),
        );

        let handle = tokio::spawn(async move { dispatcher.run().await });
        signal.notify_one();

        for id in &ids {
            wait_for_status(&backend, *id, CommandStatus::Sent).await;
        }
        assert_eq!(
            sink.calls().len(),
            3,
            "all three commands must be delivered exactly once (no double-send)"
        );

        cancel.cancel();
        let _ = handle.await;
    }

    /// A [`DownlinkSink`] that, on its **first** enqueue (i.e. mid-drain),
    /// simulates a fresh command arriving *while the drain is in flight*: it
    /// enqueues a new `Pending` row and fires the dispatch signal. This is the
    /// exact race AC#6 is about — the stored `Notify` permit must cause a
    /// re-drain that delivers the late command. (The plain-`MockSink` burst
    /// test above cannot exercise this, because it queues everything before
    /// `run()` starts, so the startup drain delivers it all and the signal
    /// re-drains an empty queue — J-1 review P3.)
    struct MidDrainInjectSink {
        calls: std::sync::Mutex<Vec<DeviceQueueItem>>,
        backend: Arc<InMemoryBackend>,
        signal: Arc<Notify>,
        injected: std::sync::atomic::AtomicBool,
    }

    impl MidDrainInjectSink {
        fn calls(&self) -> Vec<DeviceQueueItem> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl DownlinkSink for MidDrainInjectSink {
        async fn enqueue_downlink(&self, item: DeviceQueueItem) -> Result<String, OpcGwError> {
            self.calls.lock().unwrap().push(item);
            // Only once: inject a late command + signal while this drain runs.
            if !self
                .injected
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                self.backend
                    .queue_command(device_command(0, "coalesce-late", 10, vec![9]))
                    .unwrap();
                self.signal.notify_one();
            }
            Ok("qid-mock-inject".to_string())
        }
    }

    /// AC#10(c) / AC#6 (coalescing race): a command enqueued **while a drain is
    /// in flight** is still delivered, via the stored `Notify` permit → re-drain.
    /// Mutation guard: if the loop dropped the permit instead of re-draining,
    /// `coalesce-late` would remain `Pending` and `calls().len()` would be 1.
    #[tokio::test]
    async fn command_enqueued_mid_drain_is_delivered() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, "coalesce-first", 10, vec![1]))
            .unwrap();

        let signal = Arc::new(Notify::new());
        let sink = Arc::new(MidDrainInjectSink {
            calls: std::sync::Mutex::new(Vec::new()),
            backend: backend.clone(),
            signal: signal.clone(),
            injected: std::sync::atomic::AtomicBool::new(false),
        });
        let cancel = CancellationToken::new();
        let mut dispatcher = CommandDispatcher::with_sink(
            test_config(),
            dyn_backend,
            cancel.clone(),
            signal.clone(),
            sink.clone(),
        );

        // The startup drain delivers `coalesce-first`; the sink injects
        // `coalesce-late` + signals DURING that drain. The stored permit must
        // then drive a re-drain that delivers `coalesce-late`.
        let handle = tokio::spawn(async move { dispatcher.run().await });

        // Wait until BOTH are delivered (both device ids reach Sent).
        for _ in 0..200 {
            if sink.calls().len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(
            sink.calls().len(),
            2,
            "a command enqueued mid-drain must be delivered via the stored permit (no lost wakeup)"
        );

        cancel.cancel();
        let _ = handle.await;
    }

    /// AC#10(f) / AC#7: cancelling the token makes `run()` return `Ok(())`
    /// promptly while it is blocked awaiting the signal.
    #[tokio::test]
    async fn cancellation_stops_the_dispatcher() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let sink = Arc::new(MockSink::new(false));
        let signal = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let mut dispatcher = CommandDispatcher::with_sink(
            test_config(),
            backend,
            cancel.clone(),
            signal.clone(),
            sink,
        );

        let handle = tokio::spawn(async move { dispatcher.run().await });
        cancel.cancel();

        let result = tokio::time::timeout(Duration::from_secs(2), handle)
            .await
            .expect("run() must return promptly after cancellation")
            .expect("dispatcher task must not panic");
        assert!(result.is_ok(), "run() must return Ok(()) on cancellation");
    }

    /// AC#10(g) / AC#4: single-owner delivery — draining twice over the same
    /// backend never enqueues the same command id twice, because `deliver_one`
    /// marks the row `Sent` and the second drain re-reads only `Pending` rows.
    ///
    /// Mutation guard: if `deliver_one` did NOT mark the row `Sent`, the second
    /// drain would re-enqueue it and `calls().len()` would be 2.
    #[tokio::test]
    async fn double_drain_does_not_double_send() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, "dispatch-dev-G", 10, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let config = test_config();
        let sink = MockSink::new(false);
        let cancel = CancellationToken::new();

        drain_pending_commands(&sink, &dyn_backend, &config, &cancel).await;
        drain_pending_commands(&sink, &dyn_backend, &config, &cancel).await;

        assert_eq!(
            sink.calls().len(),
            1,
            "a command already delivered (Sent) must not be re-enqueued by a second drain"
        );
        assert_eq!(
            backend.command_status_for_test(cmd_id).map(|(s, _)| s),
            Some(CommandStatus::Sent)
        );
    }

    /// J-1 review iter-2 P6: a drain whose token is already cancelled delivers
    /// NOTHING — the command stays `Pending` for the next generation's startup
    /// drain. Guards the cancellation-aware teardown that keeps the dispatcher
    /// task returning well before `join_data_plane`'s force-abort.
    #[tokio::test]
    async fn cancelled_drain_delivers_nothing() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, "dispatch-dev-C", 10, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let config = test_config();
        let sink = MockSink::new(false);
        let cancel = CancellationToken::new();
        cancel.cancel(); // pre-cancelled

        let ok = drain_pending_commands(&sink, &dyn_backend, &config, &cancel).await;

        assert!(ok, "a cancellation-interrupted drain is not a read-error");
        assert_eq!(
            sink.calls().len(),
            0,
            "a cancelled drain must deliver nothing"
        );
        assert_eq!(
            backend.command_status_for_test(cmd_id).map(|(s, _)| s),
            Some(CommandStatus::Pending),
            "the command must remain Pending for the next startup drain"
        );
    }

    /// J-1 review iter-2 P5: the read-error retry backoff escalates
    /// `2s→4s→8s→16s→32s→60s(cap)` and stays capped, so a sustained storage
    /// outage is re-driven (and WARN-logged) at most once per cap, not every 2s.
    #[test]
    fn drain_retry_backoff_escalates_and_caps() {
        assert_eq!(drain_retry_backoff(0), Duration::from_secs(2)); // defensive (n>=1 in practice)
        assert_eq!(drain_retry_backoff(1), Duration::from_secs(2));
        assert_eq!(drain_retry_backoff(2), Duration::from_secs(4));
        assert_eq!(drain_retry_backoff(3), Duration::from_secs(8));
        assert_eq!(drain_retry_backoff(4), Duration::from_secs(16));
        assert_eq!(drain_retry_backoff(5), Duration::from_secs(32));
        assert_eq!(drain_retry_backoff(6), DRAIN_RETRY_BACKOFF_MAX); // 64s → capped at 60s
        assert_eq!(drain_retry_backoff(100), DRAIN_RETRY_BACKOFF_MAX);
    }
}

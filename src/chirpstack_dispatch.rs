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
//!   double-enqueue. The poll loop no longer drains (AC#3), the dispatcher is
//!   a single task, and ambiguous enqueue outcomes (RPC sent but failed/timed
//!   out) are terminal rather than retried (iter-4) — so no code path
//!   double-enqueues. Residual: a crash/force-abort in the enqueue→mark-Sent
//!   window can still re-send on the next startup drain (pre-existing
//!   at-least-once contract, tracked in #177).
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

/// Cap for the escalating unsettled-drain backoff (J-1 review iter-2 P5).
/// Under a sustained outage the retry interval doubles each consecutive
/// failure up to this cap, so the dispatcher's **self-driven** re-drive (the
/// timer arm) happens at most once per cap interval instead of every 2 s —
/// bounding wasted work and the self-driven share of WARNs (project
/// WARN-budget discipline, cf. #144/#149). WARN volume per outage class
/// (iter-4 accounting):
/// - **Storage outage** (`command_dispatch_drain_error`): signal-arm WARNs are
///   naturally self-limiting because the outage also fails the write's own
///   `queue_command`, so few signals fire.
/// - **ChirpStack outage** (`command_dispatch_retry`): storage is healthy so
///   writes DO signal, but each drain short-circuits after the FIRST
///   unreachable row (one WARN + one ≤5 s connect attempt per drive, not one
///   per pending row), and every affected row expires `Failed` at the
///   delivery deadline — so the WARN volume is bounded by
///   (writes during the outage) + (self-drives at this cap), not
///   writes × pending rows.
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
    create_device_client_from_config, deliver_one, find_command_cfg, AuthInterceptor,
    DeliveryOutcome, DownlinkSink,
};
use crate::config::AppConfig;
use crate::storage::{AsyncStorageExt, CommandStatus, StorageBackend};
use crate::utils::OpcGwError;

/// Deadline for the `enqueue` RPC itself (J-1 review iter-3, edge finding: the
/// channel connect has a 5 s timeout but the RPC had none — a half-open
/// connection or stalled ChirpStack froze the dispatcher indefinitely, with
/// every later command stuck behind it and no WARN emitted).
const ENQUEUE_RPC_TIMEOUT: Duration = Duration::from_secs(10);

type CachedDeviceClient = chirpstack_api::api::device_service_client::DeviceServiceClient<
    tonic::codegen::InterceptedService<tonic::transport::Channel, AuthInterceptor>,
>;

/// Returns `true` when a gRPC `Status`'s error chain proves the request was
/// **never transmitted** (J-1 iter-5): with a warm cached client, tonic
/// reconnects lazily inside the RPC, so a ChirpStack restart surfaces as an
/// RPC-level error whose source chain bottoms out in a connect-class
/// `std::io::Error`. A connection that was REFUSED (or a host/network that was
/// unreachable, or a socket that was never connected) provably carried no
/// request, so retrying is double-send-safe. Anything else — including
/// `ConnectionReset`, which can happen after the request went out — stays
/// ambiguous. Typed `downcast_ref` walk, deliberately NOT substring matching
/// (project finding-class: substring matchers misclassify).
fn is_provably_unsent(status: &tonic::Status) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = std::error::Error::source(status);
    while let Some(err) = source {
        // iter-6: hyper-util labels a connect that failed at DNS resolution
        // with this exact ConnectError message (the io::Error underneath is
        // `Uncategorized`, so the kind allowlist below cannot catch it, and
        // the type is not public for a downcast). A name that never resolved
        // provably carried no request. In the flagship compose deployment
        // ChirpStack is addressed by container DNS name, and Docker's embedded
        // DNS stops resolving a stopped container — so a ChirpStack restart
        // often surfaces as THIS class, not ConnectionRefused. Exact ==, not
        // substring; the real-stack classifier tests (closed port + .invalid
        // host) fail loudly if a hyper-util upgrade ever changes the label.
        if err.to_string() == "dns error" {
            return true;
        }
        if let Some(io) = err.downcast_ref::<std::io::Error>() {
            return matches!(
                io.kind(),
                std::io::ErrorKind::ConnectionRefused
                    | std::io::ErrorKind::HostUnreachable
                    | std::io::ErrorKind::NetworkUnreachable
                    | std::io::ErrorKind::NotConnected
                    // Connect-phase timeout (blackholed SYN). Post-send kernel
                    // timeouts (~minutes) cannot surface here: the 10 s
                    // ENQUEUE_RPC_TIMEOUT tokio arm preempts them, and its
                    // elapsed error carries no io source at all.
                    | std::io::ErrorKind::TimedOut
            );
        }
        source = err.source();
    }
    false
}

/// Production [`DownlinkSink`]: enqueues a downlink to ChirpStack over gRPC.
///
/// Builds its `DeviceServiceClient` through the shared
/// [`create_device_client_from_config`] factory (AC#8) so it shares exactly one
/// connect/auth path with the poller — no duplicated auth logic, no second
/// hand-rolled connect, and the `api_token` never appears in a log line. The
/// body mirrors the former `impl DownlinkSink for ChirpstackPoller` (AC#9).
///
/// The client is created lazily and **cached** across enqueues (J-1 review
/// iter-3: creating a fresh TCP+HTTP/2 channel per command put an up-to-5 s
/// connect on the latency-critical path of every drain and multiplied the cost
/// of a backlog). Any RPC failure drops the cached client so the next attempt
/// reconnects on a fresh channel.
pub(crate) struct ChirpStackDownlinkSink {
    config: AppConfig,
    client: tokio::sync::Mutex<Option<CachedDeviceClient>>,
}

impl ChirpStackDownlinkSink {
    pub(crate) fn new(config: AppConfig) -> Self {
        Self {
            config,
            client: tokio::sync::Mutex::new(None),
        }
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

        // Client-creation failure is a handled error, never a panic. It is the
        // ONE provably-safe-to-retry failure class (iter-4): the TCP/channel
        // connect failed, so no request can have reached ChirpStack —
        // `ChirpStackUnreachable` tells `deliver_one` to leave the row
        // `Pending` for the bounded retry.
        let mut guard = self.client.lock().await;
        if guard.is_none() {
            *guard = Some(
                create_device_client_from_config(&self.config)
                    .await
                    .map_err(|e| OpcGwError::ChirpStackUnreachable(e.to_string()))?,
            );
        }
        let device_client = guard.as_mut().expect("client cached on the line above");

        // Any failure PAST this point is AMBIGUOUS: the RPC was (or may have
        // been) sent, and ChirpStack may have committed the queue item even
        // though the response was lost. Retrying an ambiguous failure could
        // enqueue the same downlink twice (double hardware actuation — iter-4
        // MEDIUM), so these return the plain `ChirpStack` variant, which
        // `deliver_one` treats as terminal ("delivery uncertain").
        match tokio::time::timeout(ENQUEUE_RPC_TIMEOUT, device_client.enqueue(request)).await {
            Ok(Ok(response)) => {
                let inner_response = response.into_inner();
                trace!(response = ?inner_response, "Downlink enqueued");
                // Capture the queue-item UUID (E-3 correlation key). It must not
                // be empty in normal operation; if ChirpStack ever returns an
                // empty id, confirmation correlation falls back to the timeout
                // sweep for this command (handled inside `deliver_one`).
                Ok(inner_response.id)
            }
            Ok(Err(e)) => {
                error!(error = %e, "Error enqueueing device request");
                // The channel may be bad — drop it so the next attempt reconnects.
                *guard = None;
                // iter-5 (warm-cache classification): with a cached client the
                // eager-connect path above is skipped, so a ChirpStack restart
                // surfaces HERE as an RPC error. If the error chain proves the
                // request never left (connect refused/unreachable), it is the
                // retry-safe class — without this check the first command of
                // every warm-cache outage was terminally Failed, silently
                // reverting the D1 retry for the most common case.
                if is_provably_unsent(&e) {
                    return Err(OpcGwError::ChirpStackUnreachable(format!(
                        "enqueue failed before transmission: {e}"
                    )));
                }
                // Preserve the gRPC status detail (code + message, never the
                // token): it becomes the operator-facing failure reason.
                Err(OpcGwError::ChirpStack(format!(
                    "Error enqueuing request: {e} (delivery uncertain — verify the \
                     device queue in ChirpStack before re-issuing)"
                )))
            }
            Err(_elapsed) => {
                error!(
                    timeout_secs = ENQUEUE_RPC_TIMEOUT.as_secs(),
                    "Enqueue RPC timed out; dropping cached client"
                );
                *guard = None;
                Err(OpcGwError::ChirpStack(format!(
                    "Error enqueuing request: RPC deadline of {}s exceeded \
                     (delivery uncertain — verify the device queue in ChirpStack \
                     before re-issuing)",
                    ENQUEUE_RPC_TIMEOUT.as_secs()
                )))
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
/// Error handling matches the pre-J-1 contract with these deliberate changes
/// (J-1 review iter-1 P1 + iter-3 party decisions D1–D3):
///
/// - A `get_pending_commands` failure is **logged and swallowed**, not
///   propagated: the dispatcher is a long-lived task, and a returned error
///   would strand every future command until the next signal at best (or kill
///   the task at worst). The caller schedules a bounded retry instead.
/// - **Delivery deadline (D2).** A `Pending` row older than
///   `global.command_delivery_timeout_secs` is marked `Failed("expired")` and
///   never delivered — so a stale command (e.g. carried across hours of
///   downtime by the startup drain) can never actuate hardware long after the
///   operator wrote it. Within the deadline, AC#5's across-restart delivery
///   holds unchanged (the Apply soft-restart case it was written for).
/// - **Provably-undelivered failures retry (D1, narrowed by iter-4).**
///   `deliver_one` leaves a row `Pending` ([`DeliveryOutcome::RetryLater`])
///   ONLY when the sink failed before anything was sent
///   (`ChirpStackUnreachable` — channel connect failure); the drain then
///   short-circuits (nothing behind it can succeed) and reports itself
///   unsettled so the caller re-drives with the escalating backoff — bounded
///   by the deadline above. An **ambiguous** failure (RPC sent but
///   errored/timed out — ChirpStack may have committed the item) is terminal
///   `Failed("delivery uncertain")`: retrying it could double-actuate
///   hardware. Mapping failures remain immediately terminal.
/// - **Orphans are terminal (D3).** `find_command_cfg` returning `None` means
///   the device/command was de-configured *after* the row was queued (command
///   nodes only exist for configured commands, so config existed at queue
///   time). The row is marked `Failed`, never enqueued — previously it fell
///   back to a raw-byte unconfirmed downlink aimed at a device the operator
///   had just removed.
///
/// # Returns
///
/// `true` if the drain settled (queue read OK and no row needs a retry),
/// `false` if `get_pending_commands` errored **or** at least one row was left
/// `Pending` on a transient sink failure. J-1 review (P1): the caller uses
/// `false` to schedule a **bounded retry** — otherwise a transient error
/// (e.g. #152 NAS SQLite contention, a ChirpStack restart) consumes the wakeup
/// permit and strands every currently-`Pending` command until an unrelated
/// future write happens to fire a new signal (the timeout sweep only rescues
/// `Sent` rows, never `Pending`).
/// `unmarked_terminal` (J-1 iter-5) carries rows whose delivery outcome was
/// terminal but whose `Failed` bookkeeping write failed — they are still
/// `Pending` in storage yet must NEVER be re-delivered (an ambiguous prior
/// attempt may have been committed by ChirpStack; re-delivering would reopen
/// the double-actuation window). Each drain first re-attempts the status
/// write for carried rows and suppresses their delivery until it succeeds.
/// The map lives on the dispatcher task (fresh per generation; a crash loses
/// it, which degrades to the pre-existing #177 startup-drain residual).
pub(crate) async fn drain_pending_commands(
    sink: &dyn DownlinkSink,
    backend: &Arc<dyn StorageBackend>,
    config: &AppConfig,
    cancel_token: &CancellationToken,
    unmarked_terminal: &mut std::collections::HashMap<u64, String>,
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
    // iter-6 hygiene: drop carry entries whose row no longer reads as
    // `Pending` (deleted externally, or its status changed by another actor)
    // — such an entry could never be visited again and would sit in the map
    // forever. Every legitimately-carried row IS `Pending` (that is what the
    // carry means), so it always appears in this read while it exists.
    // Runs before the empty-queue return so an emptied queue clears the map.
    unmarked_terminal.retain(|id, _| pending.iter().any(|c| c.id == *id));
    if pending.is_empty() {
        return true;
    }
    debug!(
        event = "command_dispatch_drain",
        count = pending.len(),
        "dispatching pending device commands"
    );
    // J-1 iter-3 D2: the dispatch-side delivery deadline. Reuses the existing
    // `command_delivery_timeout_secs` knob (whose Sent-side meaning is "how
    // long may delivery take") rather than introducing a new one.
    let deadline = Duration::from_secs(u64::from(config.global.command_delivery_timeout_secs));
    let mut retry_needed = false;
    for command in pending {
        // J-1 review iter-2 P6: make the drain cancellation-aware so teardown /
        // Apply is responsive under a large backlog + slow sink. The check is
        // between commands, so a single in-flight `deliver_one` can still run
        // to completion cooperatively; a `join_data_plane` force-abort (or a
        // crash) inside `deliver_one`'s enqueue→mark-Sent window can however
        // leave an already-enqueued row `Pending` — that at-least-once residual
        // predates J-1 (tracked in deferred-work). Matches the existing
        // `is_cancelled()` pattern in the poller's pagination loops.
        // Commands not yet delivered stay `Pending` and are picked up by the
        // next generation's startup drain (within the delivery deadline).
        if cancel_token.is_cancelled() {
            debug!(
                event = "command_dispatch_drain_cancelled",
                "drain interrupted by shutdown; remaining pending commands deferred to next startup drain"
            );
            break;
        }
        // iter-5: a row carried as unmarked-terminal is NEVER delivered again —
        // only its pending `Failed` status write is re-attempted. (Checked
        // before the age/orphan gates so those cannot re-classify it.)
        if let Some(reason) = unmarked_terminal.get(&command.id) {
            if mark_failed(backend, command.id, reason.clone()).await {
                unmarked_terminal.remove(&command.id);
            } else {
                retry_needed = true; // keep re-attempting the status write
            }
            continue;
        }
        // D2 age gate, symmetric (iter-4): a row is expired when |now −
        // created_at| exceeds the deadline in EITHER direction. Small negative
        // ages (clock skew < deadline) are treated as fresh; but a
        // far-future `created_at` (clock stepped back after e.g. a power
        // event) must not make the row immortal — pre-iter-4 a negative age
        // bypassed the gate entirely, so the retry ladder ran unbounded for
        // hours and the command could actuate long after the operator wrote
        // it, the exact outcome D2 exists to prevent.
        let age = chrono::Utc::now().signed_duration_since(command.created_at);
        if age.num_seconds().unsigned_abs() > deadline.as_secs() {
            warn!(
                event = "command_dispatch_expired",
                command_id = command.id,
                device_id = %command.device_id,
                age_secs = age.num_seconds(),
                deadline_secs = deadline.as_secs(),
                "command exceeded the delivery deadline before it could be enqueued; marking Failed"
            );
            let marked = mark_failed(
                backend,
                command.id,
                format!(
                    "not delivered within {}s of creation (delivery deadline)",
                    deadline.as_secs()
                ),
            )
            .await;
            if !marked {
                // iter-4: a transient storage failure on the bookkeeping write
                // leaves the row `Pending` — schedule the same self-driven
                // retry the read-error path gets, instead of waiting for an
                // unrelated future write. (The gates run before delivery, so
                // the row can never actuate meanwhile.)
                retry_needed = true;
            }
            continue;
        }
        // Resolve the per-command class + confirmed flag from config BEFORE the
        // await so the borrow of `config` does not cross the await point.
        let (command_class, confirmed) =
            match find_command_cfg(&config.application_list, &command.device_id, command.f_port) {
                Some(cfg) => (cfg.command_class.clone(), cfg.command_confirmed),
                None => {
                    // D3 orphan gate: the device/command is no longer configured.
                    warn!(
                        event = "command_dispatch_orphaned",
                        command_id = command.id,
                        device_id = %command.device_id,
                        f_port = command.f_port,
                        "no configured command matches this queued row (device/command removed since queueing); marking Failed"
                    );
                    let marked = mark_failed(
                        backend,
                        command.id,
                        "device/command no longer configured (removed after the command was queued)"
                            .to_string(),
                    )
                    .await;
                    if !marked {
                        retry_needed = true; // iter-4: same rationale as the expiry gate above
                    }
                    continue;
                }
            };
        let outcome =
            deliver_one(sink, backend, command_class.as_deref(), confirmed, &command).await;
        match outcome {
            DeliveryOutcome::RetryLater => {
                // `RetryLater` means ChirpStack was UNREACHABLE (connect failed
                // before any send — the only retryable class after iter-4).
                // Nothing behind this row can succeed either, so short-circuit
                // the drain instead of paying a serial 5 s connect timeout per
                // remaining row (iter-4); the backoff re-drives the whole queue.
                retry_needed = true;
                debug!(
                    event = "command_dispatch_drain",
                    command_id = command.id,
                    "ChirpStack unreachable; deferring the remaining pending commands to the bounded retry"
                );
                break;
            }
            DeliveryOutcome::TerminalUnmarked(reason) => {
                // iter-5: terminal outcome, but the `Failed` write failed —
                // carry the row so delivery stays suppressed while the status
                // write is re-attempted on the bounded retry.
                unmarked_terminal.insert(command.id, reason);
                retry_needed = true;
            }
            DeliveryOutcome::Delivered | DeliveryOutcome::Terminal => {}
        }
    }
    !retry_needed
}

/// Marks a queued command `Failed` with `reason`. Returns `false` (after
/// logging — the drain must not abort over a bookkeeping failure) if the
/// storage write failed and the row is still `Pending`; the caller then
/// schedules a bounded retry so the row is re-examined without waiting for an
/// unrelated write (iter-4).
async fn mark_failed(backend: &Arc<dyn StorageBackend>, command_id: u64, reason: String) -> bool {
    match backend
        .async_store()
        .update_command_status(command_id, CommandStatus::Failed, Some(reason))
        .await
    {
        Ok(()) => true,
        Err(e) => {
            error!(error = %e, command_id, "Failed to mark command Failed");
            false
        }
    }
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
    async fn drain_all(
        &self,
        unmarked_terminal: &mut std::collections::HashMap<u64, String>,
    ) -> bool {
        drain_pending_commands(
            self.sink.as_ref(),
            &self.backend,
            &self.config,
            &self.cancel_token,
            unmarked_terminal,
        )
        .await
    }

    /// Dispatch loop.
    ///
    /// 1. **Startup drain (AC#5)** — deliver anything already `Pending` before
    ///    awaiting the first signal.
    /// 2. `select!` over cancellation (graceful shutdown, AC#7) and the
    ///    dispatch signal; each signal drains the whole queue (AC#6).
    /// 3. **Bounded retry (J-1 review P1 + iter-2 P5 + iter-3 D1)** — when a
    ///    drain did not settle (queue read failed, **or** a transient sink
    ///    failure left rows `Pending`), an **escalating** backoff timer arm
    ///    (`drain_retry_backoff`, `2s→…→60s cap`) is armed so the drain is
    ///    re-driven without waiting for an unrelated future write, and a stuck
    ///    backend/ChirpStack's self-driven re-drive backs off toward the cap
    ///    instead of hammering every 2 s. Retries are bounded by the delivery
    ///    deadline (D2): a row that stays undeliverable past
    ///    `command_delivery_timeout_secs` is marked `Failed` and stops driving
    ///    the ladder. On a settled drain that timer is `pending()` (never
    ///    fires) and the streak resets, so the happy path stays purely
    ///    event-driven — no cadence.
    ///
    /// # Returns
    ///
    /// `Ok(())` on graceful shutdown.
    pub async fn run(&mut self) -> Result<(), OpcGwError> {
        debug!("Starting CommandDispatcher (event-driven command dispatch)");

        // iter-5: rows whose terminal `Failed` write failed — delivery is
        // suppressed for these ids until the status write lands (see
        // `drain_pending_commands` docs). Task-local by design.
        let mut unmarked_terminal: std::collections::HashMap<u64, String> =
            std::collections::HashMap::new();

        // AC#5: drain commands persisted before this task started (pre-boot or
        // carried across a soft-restart) without needing a fresh OPC UA write.
        let mut drain_ok = self.drain_all(&mut unmarked_terminal).await;
        // Consecutive read-error count driving the escalating retry backoff.
        let mut error_streak: u32 = if drain_ok { 0 } else { 1 };

        loop {
            // J-1 review P1/P5/iter-3: only when the previous drain did not
            // settle (storage read error, or a transient sink failure left rows
            // Pending) do we fall back to an escalating backoff timer; a
            // settled drain leaves this `pending()` so we wait purely on the
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
                    drain_ok = self.drain_all(&mut unmarked_terminal).await;
                    error_streak = if drain_ok { 0 } else { error_streak.saturating_add(1) };
                }
                _ = retry => {
                    // The previous drain did not settle (queue read failed, or
                    // rows were left Pending on a transient sink failure);
                    // re-drive it after the escalating backoff so a transient
                    // fault never strands a `Pending` command indefinitely,
                    // while a sustained outage backs off toward the cap — until
                    // the delivery deadline expires the affected rows.
                    drain_ok = self.drain_all(&mut unmarked_terminal).await;
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

    /// Minimal AppConfig fixture (same source as `chirpstack::tests`), with a
    /// class-less command injected on `device_1` port 10 so `find_command_cfg`
    /// resolves it (→ raw-byte delivery path). J-1 iter-3 D3 made an
    /// unresolvable command an ORPHAN (marked `Failed`, never enqueued), so
    /// unlike the earlier revision the delivery tests must use a **configured**
    /// device — an unconfigured id now exercises the orphan gate instead (see
    /// `orphaned_command_is_failed_not_delivered`). Injected programmatically
    /// (not in the shared TOML) because other tests assert address-space node
    /// counts derived from that file.
    fn test_config() -> AppConfig {
        let config_path =
            std::env::var("CONFIG_PATH").unwrap_or_else(|_| "tests/config/config.toml".to_string());
        let mut config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load test configuration");
        assert!(
            !config.application_list.is_empty()
                && !config.application_list[0].device_list.is_empty(),
            "test fixture precondition: application[0] must have at least one device"
        );
        config.application_list[0].device_list[0].device_command_list =
            Some(vec![crate::config::DeviceCommandCfg {
                command_id: 1,
                command_name: "dispatch_test_cmd".to_string(),
                command_confirmed: false,
                command_port: 10,
                command_class: None, // class-less → raw-byte delivery
            }]);
        config
    }

    /// The configured (device_id, f_port) pair the fixture's injected command
    /// binds to — commands queued for this pair resolve via `find_command_cfg`.
    const CFG_DEV: &str = "device_1";
    const CFG_PORT: u8 = 10;

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
    /// ~5 s (J-1 iter-3: was ~1 s, a CI-flake candidate under scheduler delay;
    /// the pass path exits early so the generous bound costs nothing). The
    /// short sleeps yield to the spawned `run` task on the current-thread test
    /// runtime.
    async fn wait_for_status(backend: &InMemoryBackend, id: u64, want: CommandStatus) {
        for _ in 0..1000 {
            if let Some((status, _)) = backend.command_status_for_test(id) {
                if status == want {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("command {id} did not reach {want:?} within timeout");
    }

    /// AC#10(a): a command queued AFTER the dispatcher is running is delivered
    /// (exactly once, `Pending → Sent`) by the **signal arm** — the startup
    /// drain has already come and gone empty, so only `notify_one()` can
    /// deliver it. (J-1 iter-3, blind finding: the earlier revision queued the
    /// command before `run()` started, so the startup drain delivered it and
    /// the signal was decorative — deleting the signal arm still passed.)
    #[tokio::test]
    async fn dispatch_delivers_a_queued_command() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();

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
        // Let the (empty) startup drain complete and the task park on the
        // signal before the command exists.
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(
            sink.calls().is_empty(),
            "nothing may be delivered before the command is queued"
        );

        backend
            .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
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
            .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![42]))
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
        // Three distinct commands on the configured device/port (post-iter-3
        // the device must resolve via `find_command_cfg`, else the orphan gate
        // fails them instead of delivering).
        for i in 0..3u8 {
            backend
                .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![i]))
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
                    .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![9]))
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
            .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![1]))
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
        for _ in 0..1000 {
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
            .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let config = test_config();
        let sink = MockSink::new(false);
        let cancel = CancellationToken::new();

        drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut std::collections::HashMap::new()).await;
        drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut std::collections::HashMap::new()).await;

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
            .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let config = test_config();
        let sink = MockSink::new(false);
        let cancel = CancellationToken::new();
        cancel.cancel(); // pre-cancelled

        let ok = drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut std::collections::HashMap::new()).await;

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

    /// J-1 iter-3 D3 (orphan gate): a `Pending` row whose device/command is no
    /// longer configured is marked `Failed` and NEVER enqueued. Before iter-3
    /// it fell back to a raw-byte unconfirmed downlink aimed at a device the
    /// operator had just removed via Apply.
    #[tokio::test]
    async fn orphaned_command_is_failed_not_delivered() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, "not-in-config-dev", 10, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let config = test_config();
        let sink = MockSink::new(false);
        let cancel = CancellationToken::new();

        let settled = drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut std::collections::HashMap::new()).await;

        assert!(settled, "an orphan is terminal — no retry must be scheduled");
        assert!(
            sink.calls().is_empty(),
            "an orphaned command must never be enqueued (raw-byte fallback removed)"
        );
        let (status, err) = backend
            .command_status_for_test(cmd_id)
            .expect("command must still exist");
        assert_eq!(status, CommandStatus::Failed);
        assert!(
            err.unwrap_or_default().contains("no longer configured"),
            "the failure reason must name the orphan cause"
        );
    }

    /// J-1 iter-3 D2 (delivery deadline): a `Pending` row older than
    /// `global.command_delivery_timeout_secs` is marked `Failed` and never
    /// delivered — a stale command (e.g. carried across hours of downtime to a
    /// later boot's startup drain) must not actuate hardware. Mutation guard:
    /// with the age gate removed, this command IS enqueued (configured device,
    /// healthy sink) and the test fails on the empty-calls assertion.
    #[tokio::test]
    async fn expired_command_is_failed_not_delivered() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        let config = test_config();
        let deadline = u64::from(config.global.command_delivery_timeout_secs);
        let mut stale = device_command(0, CFG_DEV, CFG_PORT, vec![1]);
        stale.created_at = chrono::Utc::now()
            - chrono::Duration::seconds(i64::try_from(deadline).unwrap() * 2 + 60);
        backend.queue_command(stale).unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let sink = MockSink::new(false);
        let cancel = CancellationToken::new();

        let settled = drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut std::collections::HashMap::new()).await;

        assert!(settled, "an expired command is terminal — no retry");
        assert!(
            sink.calls().is_empty(),
            "an expired command must never be enqueued"
        );
        let (status, err) = backend
            .command_status_for_test(cmd_id)
            .expect("command must still exist");
        assert_eq!(status, CommandStatus::Failed);
        assert!(
            err.unwrap_or_default().contains("delivery deadline"),
            "the failure reason must name the deadline"
        );
    }

    /// A sink whose first N enqueues fail with `ChirpStackUnreachable` (the
    /// provably-not-delivered, retry-safe class — models a ChirpStack
    /// restart / boot-ordering connect failure), then succeed.
    struct FlakySink {
        fail_remaining: std::sync::atomic::AtomicU32,
        calls: std::sync::Mutex<Vec<DeviceQueueItem>>,
    }

    impl FlakySink {
        fn failing_first(n: u32) -> Self {
            Self {
                fail_remaining: std::sync::atomic::AtomicU32::new(n),
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn calls_len(&self) -> usize {
            self.calls.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl DownlinkSink for FlakySink {
        async fn enqueue_downlink(&self, item: DeviceQueueItem) -> Result<String, OpcGwError> {
            self.calls.lock().unwrap().push(item);
            let prev = self
                .fail_remaining
                .fetch_update(
                    std::sync::atomic::Ordering::SeqCst,
                    std::sync::atomic::Ordering::SeqCst,
                    |v| Some(v.saturating_sub(1)),
                )
                .unwrap();
            if prev > 0 {
                Err(OpcGwError::ChirpStackUnreachable(
                    "connect refused (mock outage)".to_string(),
                ))
            } else {
                Ok("qid-mock-flaky".to_string())
            }
        }
    }

    /// J-1 iter-3 D1 (bounded retry): a command whose enqueue fails
    /// transiently stays `Pending` and is re-driven by the escalating backoff
    /// timer arm — WITHOUT any further OPC UA write/signal — and is delivered
    /// once the sink recovers. Uses a paused tokio clock so the 2 s backoff
    /// elapses instantly. Mutation guards: (a) if the transient failure were
    /// still marked `Failed` (pre-iter-3), the command never reaches `Sent`;
    /// (b) if sink failures did not schedule the retry arm, nothing re-drives
    /// the drain and the test times out on the paused clock.
    #[tokio::test(start_paused = true)]
    async fn transient_sink_failure_is_retried_until_delivered() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;

        let sink = Arc::new(FlakySink::failing_first(1));
        let signal = Arc::new(Notify::new());
        let cancel = CancellationToken::new();
        let mut dispatcher = CommandDispatcher::with_sink(
            test_config(),
            dyn_backend,
            cancel.clone(),
            signal.clone(),
            sink.clone(),
        );

        // Startup drain: attempt 1 fails (transient) → row stays Pending →
        // retry arm armed. NO signal is ever fired in this test.
        let handle = tokio::spawn(async move { dispatcher.run().await });

        wait_for_status(&backend, cmd_id, CommandStatus::Sent).await;
        assert_eq!(
            sink.calls_len(),
            2,
            "exactly two enqueue attempts: the transient failure, then the retry that delivers"
        );

        cancel.cancel();
        let _ = handle.await;
    }

    /// J-1 iter-4 (M2, symmetric age gate): a FUTURE-stamped row (clock
    /// stepped back further than the deadline, e.g. NTP correction after a
    /// power event) must expire like an old one — pre-iter-4 a negative age
    /// bypassed the gate entirely, making the row immortal until the clock
    /// caught up (and letting it actuate hours late when ChirpStack returned).
    #[tokio::test]
    async fn future_stamped_command_is_failed_not_delivered() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        let config = test_config();
        let deadline = u64::from(config.global.command_delivery_timeout_secs);
        let mut skewed = device_command(0, CFG_DEV, CFG_PORT, vec![1]);
        skewed.created_at = chrono::Utc::now()
            + chrono::Duration::seconds(i64::try_from(deadline).unwrap() * 2 + 60);
        backend.queue_command(skewed).unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let sink = MockSink::new(false);
        let cancel = CancellationToken::new();

        drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut std::collections::HashMap::new()).await;

        assert!(
            sink.calls().is_empty(),
            "a far-future-stamped command must never be enqueued"
        );
        assert_eq!(
            backend.command_status_for_test(cmd_id).map(|(s, _)| s),
            Some(CommandStatus::Failed),
            "clock-skew beyond the deadline must expire the row, not immortalize it"
        );
    }

    /// J-1 iter-4 (M1): an AMBIGUOUS sink failure (RPC sent but errored/timed
    /// out — ChirpStack may have committed the item) is TERMINAL, not retried:
    /// retrying could enqueue the same downlink twice (double actuation).
    /// Mutation guard: if ambiguous errors were classified RetryLater, the row
    /// would stay Pending and this Failed assertion trips.
    #[tokio::test]
    async fn ambiguous_sink_failure_is_terminal_not_retried() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let config = test_config();
        // MockSink(true) fails with the plain `ChirpStack` variant = ambiguous.
        let sink = MockSink::new(true);
        let cancel = CancellationToken::new();

        let settled = drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut std::collections::HashMap::new()).await;

        assert!(settled, "an ambiguous failure is terminal — no retry scheduled");
        assert_eq!(sink.calls().len(), 1, "exactly one attempt — never re-sent");
        let (status, err) = backend
            .command_status_for_test(cmd_id)
            .expect("command must still exist");
        assert_eq!(status, CommandStatus::Failed);
        assert!(err.is_some(), "terminal failure must carry the reason");
    }

    /// J-1 iter-4 (drain short-circuit): when ChirpStack is unreachable, the
    /// drain defers ALL remaining rows after the first failed connect instead
    /// of paying a serial connect timeout per row — one attempt per drive.
    #[tokio::test]
    async fn drain_short_circuits_when_unreachable() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        for i in 0..3u8 {
            backend
                .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![i]))
                .unwrap();
        }
        let config = test_config();
        let sink = FlakySink::failing_first(u32::MAX); // always unreachable
        let cancel = CancellationToken::new();

        let settled = drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut std::collections::HashMap::new()).await;

        assert!(!settled, "an unreachable sink must schedule a retry");
        assert_eq!(
            sink.calls_len(),
            1,
            "the drain must short-circuit after the first unreachable row"
        );
        assert_eq!(
            backend.get_pending_commands().unwrap().len(),
            3,
            "all rows stay Pending for the bounded retry"
        );
    }

    /// J-1 iter-4 (L: retry→expiry interplay): a row that keeps failing
    /// with `ChirpStackUnreachable` rides the backoff ladder INTO the delivery
    /// deadline and expires `Failed` — proving "retries are bounded by the
    /// deadline" on the real `run()` loop (real clock: the row starts 1 s
    /// short of the deadline; the first 2 s backoff carries it past).
    /// Mutation guards: gating only the first attempt, or moving the age gate
    /// below `deliver_one`, leaves the row `Pending` forever and this test
    /// times out its wait loop.
    #[tokio::test]
    async fn retry_ladder_is_bounded_by_delivery_deadline() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        let config = test_config();
        let deadline = i64::from(config.global.command_delivery_timeout_secs);
        // deadline − 5 (iter-5): wide enough that a stalled CI runner cannot
        // expire the row before the first delivery attempt, small enough that
        // the 2 s + 4 s backoff drives it past the deadline.
        let mut nearly_expired = device_command(0, CFG_DEV, CFG_PORT, vec![1]);
        nearly_expired.created_at = chrono::Utc::now() - chrono::Duration::seconds(deadline - 5);
        backend.queue_command(nearly_expired).unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;

        let sink = Arc::new(FlakySink::failing_first(u32::MAX)); // never recovers
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

        // Startup drain: attempt fails (unreachable) → Pending; the 2 s
        // backoff re-drive lands past the deadline → expired Failed.
        wait_for_status(&backend, cmd_id, CommandStatus::Failed).await;
        let (_, err) = backend.command_status_for_test(cmd_id).unwrap();
        assert!(
            err.unwrap_or_default().contains("delivery deadline"),
            "the ladder must terminate via the deadline, not another path"
        );
        assert!(
            sink.calls_len() >= 1,
            "at least the initial delivery attempt must have happened"
        );

        cancel.cancel();
        let _ = handle.await;
    }

    /// J-1 iter-5 (typed unsent-classifier): only connect-class io errors in a
    /// `Status`'s source chain prove the request never left; reset/plain
    /// statuses stay ambiguous.
    #[test]
    fn is_provably_unsent_classifies_by_io_error_kind() {
        let refused = tonic::Status::from_error(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionRefused,
            "connection refused",
        )));
        assert!(is_provably_unsent(&refused), "refused connect = nothing sent");

        let reset = tonic::Status::from_error(Box::new(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "connection reset by peer",
        )));
        assert!(
            !is_provably_unsent(&reset),
            "a reset can happen AFTER the request went out — must stay ambiguous"
        );

        let plain = tonic::Status::unavailable("service unavailable");
        assert!(
            !is_provably_unsent(&plain),
            "no io source = no proof of non-transmission"
        );

        // Nested wrapping (hyper-style): the walk must reach the io error.
        #[derive(Debug)]
        struct Wrap(std::io::Error);
        impl std::fmt::Display for Wrap {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "wrapped: {}", self.0)
            }
        }
        impl std::error::Error for Wrap {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }
        let nested = tonic::Status::from_error(Box::new(Wrap(std::io::Error::new(
            std::io::ErrorKind::HostUnreachable,
            "no route to host",
        ))));
        assert!(
            is_provably_unsent(&nested),
            "the source walk must find io errors through wrapper layers"
        );
    }

    /// J-1 iter-5 (unmarked-terminal carry): when a terminal outcome's
    /// `Failed` bookkeeping write fails, the row must NEVER be re-delivered —
    /// the dispatcher suppresses delivery and re-attempts only the status
    /// write until it lands. Mutation guard (verified): without the carry map,
    /// the second drain re-reads the still-`Pending` row and re-enqueues it —
    /// `calls().len()` would be 2 (the reopened double-actuation window).
    #[tokio::test]
    async fn unmarked_terminal_row_is_never_redelivered() {
        let backend = Arc::new(InMemoryBackend::new());
        let dyn_backend: Arc<dyn StorageBackend> = backend.clone();
        backend
            .queue_command(device_command(0, CFG_DEV, CFG_PORT, vec![1]))
            .unwrap();
        let cmd_id = backend.get_pending_commands().unwrap()[0].id;
        let config = test_config();
        // Ambiguous sink failure AND a failing Failed-write: outcome is
        // TerminalUnmarked (delivery attempted once, row still Pending).
        let sink = MockSink::new(true);
        let cancel = CancellationToken::new();
        let mut unmarked = std::collections::HashMap::new();
        backend.fail_next_update_command_status(1);

        let settled =
            drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut unmarked).await;
        assert!(!settled, "an unmarked terminal must schedule a retry");
        assert_eq!(sink.calls().len(), 1);
        assert_eq!(unmarked.len(), 1, "the row must be carried as unmarked");
        assert_eq!(
            backend.command_status_for_test(cmd_id).map(|(s, _)| s),
            Some(CommandStatus::Pending),
            "the Failed write was injected to fail — row still Pending"
        );

        // Second drain (the backoff re-drive): delivery MUST stay suppressed;
        // only the status write is re-attempted, and it now succeeds.
        let settled =
            drain_pending_commands(&sink, &dyn_backend, &config, &cancel, &mut unmarked).await;
        assert!(settled, "carry healed — drain settles");
        assert_eq!(
            sink.calls().len(),
            1,
            "the ambiguous row must NEVER be re-enqueued (double-actuation guard)"
        );
        assert!(unmarked.is_empty(), "carry entry removed once the write lands");
        let (status, err) = backend.command_status_for_test(cmd_id).unwrap();
        assert_eq!(status, CommandStatus::Failed);
        assert!(
            err.unwrap_or_default().contains("mock enqueue failure"),
            "the original failure reason must be preserved through the carry"
        );
    }

    /// Drives a REAL tonic client (lazy channel, so the connect failure
    /// surfaces as an RPC-level `Status` — the warm-cache shape) at `uri` and
    /// returns the resulting error `Status`.
    async fn real_enqueue_status(uri: &'static str) -> tonic::Status {
        let channel = tonic::transport::Endpoint::from_static(uri)
            .connect_timeout(Duration::from_secs(5))
            .connect_lazy();
        let mut client =
            chirpstack_api::api::device_service_client::DeviceServiceClient::new(channel);
        let request = Request::new(EnqueueDeviceQueueItemRequest {
            queue_item: None,
            flush_queue: false,
        });
        tokio::time::timeout(Duration::from_secs(15), client.enqueue(request))
            .await
            .expect("connect failure must surface well before 15 s")
            .expect_err("no server is listening — the call must fail")
    }

    /// J-1 iter-6: END-TO-END classifier liveness against the REAL
    /// tonic/hyper-util stack — a connection-refused reconnect must classify
    /// as provably-unsent. This is the guard that fails loudly if a
    /// dependency upgrade changes the error-chain shape and silently kills
    /// the warm-cache retry classification.
    #[tokio::test]
    async fn classifier_fires_for_real_refused_connect() {
        // Port 1 on loopback: nothing listens; connect is refused instantly.
        let status = real_enqueue_status("http://127.0.0.1:1").await;
        assert!(
            is_provably_unsent(&status),
            "a real refused connect must classify as unsent; got: {status:?}"
        );
    }

    /// J-1 iter-6: same liveness guard for the DNS-failure outage shape —
    /// the flagship compose deployment addresses ChirpStack by container DNS
    /// name, and a stopped container surfaces as a resolution failure, not a
    /// refused connect. Guards the exact-match "dns error" hyper-util label.
    #[tokio::test]
    async fn classifier_fires_for_real_dns_failure() {
        // RFC 2606 reserves .invalid: resolution is guaranteed to fail.
        let status = real_enqueue_status("http://chirpstack-does-not-exist.invalid:1").await;
        assert!(
            is_provably_unsent(&status),
            "a real DNS resolution failure must classify as unsent; got: {status:?}"
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

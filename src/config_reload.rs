// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Configuration hot-reload channel (Story C-6).
//!
//! Owns the `tokio::sync::watch::Sender<Arc<AppConfig>>` propagation
//! channel and the SQLite-driven `notify_crud_write` trigger. CRUD
//! handlers call `notify_crud_write` after each SQLite write to push
//! the updated application tree to all watch-channel subscribers.
//!
//! Story 9-7's SIGHUP-based reload, `reload()`, `ReloadOutcome`,
//! `ReloadError`, and `classify_diff` were removed in C-6. Hot-reload
//! is now SQLite-driven: web CRUD handlers call `notify_crud_write`
//! after each successful write.

use std::sync::Arc;

use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::{AppConfig, ChirpStackApplications};

/// Owns the watch channel and serialises concurrent `notify_crud_write`
/// calls. Cloned into `AppState`; each subsystem receives a fresh
/// `Receiver` via [`subscribe`](Self::subscribe).
pub struct ConfigReloadHandle {
    tx: watch::Sender<Arc<AppConfig>>,
    /// Serialises concurrent `notify_crud_write` calls. Without this, two
    /// near-simultaneous CRUD writes could race on `borrow → build → send`.
    reload_lock: Mutex<()>,
}

impl ConfigReloadHandle {
    /// Construct the handle with an initial config. Returns
    /// `(handle, initial_receiver)` — the handle is retained by
    /// `AppState`; the receiver is dropped (subsystems subscribe via
    /// [`subscribe`](Self::subscribe)).
    pub fn new(
        initial: Arc<AppConfig>,
    ) -> (Self, watch::Receiver<Arc<AppConfig>>) {
        let (tx, rx) = watch::channel(initial);
        (
            Self {
                tx,
                reload_lock: Mutex::new(()),
            },
            rx,
        )
    }

    /// Mint a fresh receiver that observes future swaps. Equivalent to
    /// `initial_rx.clone()` in semantics; provided as an explicit
    /// method so subscribers don't have to thread the original
    /// receiver around.
    pub fn subscribe(&self) -> watch::Receiver<Arc<AppConfig>> {
        self.tx.subscribe()
    }

    /// Push a new application list from a SQLite write into the watch
    /// channel without re-reading the TOML file. Used by CRUD handlers
    /// after each successful SQLite mutation (Story C-6): the handler
    /// writes to SQLite, loads the new full state, then calls this to
    /// propagate the change to all watch-channel subscribers
    /// (`run_web_config_listener`, the ChirpStack poller, etc.).
    ///
    /// Serialised by `reload_lock` so concurrent CRUD `notify_crud_write`
    /// calls cannot race on `borrow → build → send`.
    pub async fn notify_crud_write(&self, new_apps: Vec<ChirpStackApplications>) {
        let _guard = self.reload_lock.lock().await;
        let live = self.tx.borrow().clone();
        let mut candidate = (*live).clone();
        let app_count = new_apps.len();
        candidate.application_list = new_apps;
        if self.tx.send(Arc::new(candidate)).is_err() {
            warn!(
                event = "config_reload_warn",
                reason = "no_subscribers",
                "Watch channel send failed — all subscribers have dropped"
            );
        } else {
            info!(
                event = "config_reload",
                trigger = "crud_write",
                application_count = app_count,
                "Application config snapshot rebuilt from SQLite"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Subsystem listeners
// ---------------------------------------------------------------------------
//
// `main.rs` spawns one listener task per subsystem that needs to react to
// config changes. The poller has its own integrated `config_rx` arm in its
// outer-loop `tokio::select!`; the web + OPC UA subsystems use the
// listener helpers below.
//
// Issue #110 constraint: every listener cooperates with `cancel_token`
// explicitly via `tokio::select!` — no RAII drop reliance. `RunHandles`
// has no `Drop` impl (rustc E0509 blocks adding one while `run_handles`
// destructures the struct), so cancellation is the only correct shutdown
// signal.

/// Story 9-7 Task 4 — web subsystem hot-reload listener.
///
/// On every `config_rx.changed()` notification, atomically swaps the
/// fields of `AppState` that depend on `AppConfig`:
///   - `dashboard_snapshot` ← fresh `DashboardConfigSnapshot::from_config(&new)`
///   - `stale_threshold_secs` ← `clamp_stale_threshold(...)` of the new value
///
/// `auth` is **not** swapped (v1 limitation). Loops until `cancel_token`
/// is fired or the watch sender is dropped.
pub async fn run_web_config_listener(
    app_state: Arc<crate::web::AppState>,
    mut config_rx: watch::Receiver<Arc<AppConfig>>,
    cancel_token: CancellationToken,
) {
    // Iter-2 review P31: consume the initial publish so the first
    // `changed()` waits for the NEXT swap rather than firing
    // immediately. `tokio::sync::watch::Receiver::subscribe()` returns
    // a receiver that has NOT marked the current value as seen, so
    // without this `borrow_and_update()` the listener would emit a
    // spurious `config_reload_applied` log line at startup.
    let _ = config_rx.borrow_and_update();
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(
                    operation = "config_reload_listener_stopped",
                    subsystem = "web",
                    "Web config-listener stopping (cancel)"
                );
                return;
            }
            changed = config_rx.changed() => {
                if changed.is_err() {
                    // Iter-1 review P15: sender dropped without
                    // cancel-token fire = anomalous (hot-reload now
                    // permanently broken for this subsystem). warn!
                    // not info! so the operator sees it.
                    warn!(
                        operation = "config_reload_listener_stopped",
                        subsystem = "web",
                        "Web config-listener stopping (sender dropped without cancel)"
                    );
                    return;
                }
                let new_config = config_rx.borrow_and_update().clone();

                // Build the fresh dashboard snapshot off the new
                // config and atomically swap. The brief write-lock
                // window blocks any concurrent reader for the
                // duration of one Arc-replace; sub-microsecond cost.
                let new_snapshot = Arc::new(crate::web::DashboardConfigSnapshot::from_config(
                    &new_config,
                ));
                {
                    // Iter-1 review P5: recover from poison rather
                    // than panic the listener task. A single
                    // poisoned lock would otherwise kill the
                    // listener and break hot-reload for the rest of
                    // the process lifetime.
                    //
                    // Iter-2 review P34: surface poison-recovery to
                    // operator audit log so the panic that caused
                    // the poison can be correlated with subsequent
                    // hot-reload behaviour.
                    let mut guard = app_state
                        .dashboard_snapshot
                        .write()
                        .unwrap_or_else(|e| {
                            warn!(
                                operation = "rwlock_poison_recovered",
                                site = "config_reload_listener",
                                "dashboard_snapshot RwLock was poisoned; recovering inner \
                                 value (a prior holder panicked — investigate)"
                            );
                            e.into_inner()
                        });
                    *guard = new_snapshot;
                }

                // Resolve + clamp the stale threshold. Any clamp event
                // is logged at warn level here (rather than inside the
                // helper) so the operator-action text can name the
                // hot-reload trigger explicitly.
                let raw_threshold = new_config
                    .opcua
                    .stale_threshold_seconds
                    .unwrap_or(crate::web::api::DEFAULT_STALE_THRESHOLD_SECS);
                let (clamped, outcome) = crate::web::clamp_stale_threshold(raw_threshold);
                if outcome != crate::web::StaleThresholdClampOutcome::Accepted {
                    tracing::warn!(
                        operation = "stale_threshold_clamped",
                        trigger = "hot_reload",
                        configured = raw_threshold,
                        clamped_to = clamped,
                        "Hot-reloaded [opcua].stale_threshold_seconds outside the \
                         valid (0, 86400] band; clamping to default for the web \
                         dashboard's 'uncertain' band"
                    );
                }
                app_state
                    .stale_threshold_secs
                    .store(clamped, std::sync::atomic::Ordering::Relaxed);

                info!(
                    operation = "config_reload_applied",
                    subsystem = "web",
                    stale_threshold_secs = clamped,
                    "Web subsystem picked up reloaded config"
                );
            }
        }
    }
}

/// Story 9-7 Task 5 + **Story 9-8 Tasks 2-5** — OPC UA subsystem
/// hot-reload listener with end-to-end address-space mutation apply.
///
/// On every `config_rx.changed()` notification:
///
///  1. **Topology diff log** (Story 9-7 backward compat): emits
///     `event="topology_change_detected"` info-level event with the
///     four 9-7-pinned axis counts (`added_devices`, `removed_devices`,
///     `modified_devices`, `story_9_8_seam`) so the existing
///     `tests/config_hot_reload.rs::topology_change_logs_seam_for_9_8`
///     integration test continues to pass byte-for-byte.
///  2. **Apply pass** (Story 9-8): calls
///     [`crate::opcua_topology_apply::apply_diff_to_address_space`]
///     which walks the 7-axis `AddressSpaceDiff` (added/removed
///     applications/devices/metrics/commands + renamed_devices) and
///     applies the four-phase mutation envelope (Q2 set_values
///     mitigation → delete → add → DisplayName rename).
///  3. **Apply outcome log** (Story 9-8): emits
///     `event="address_space_mutation_succeeded"` info on success
///     (with all 9 axis counts + `duration_ms`) or
///     `event="address_space_mutation_failed"` warn on failure (with
///     `reason` + sanitised `error: %e` field).
///
/// Story 9-7's documented v1 limitation ("dashboard updates but OPC UA
/// stays frozen") is closed by this implementation — topology
/// hot-reload is now end-to-end functional and FR24 is satisfied.
///
/// Loops until `cancel_token` is fired or the watch sender is dropped.
///
/// **Issue #110 carry-forward**: the listener cooperates with
/// `cancel_token.cancel()` explicitly — no RAII drop reliance.
#[allow(clippy::too_many_arguments)]
pub async fn run_opcua_config_listener(
    manager: Arc<crate::opc_ua_history::OpcgwHistoryNodeManager>,
    subscriptions: Arc<opcua::server::SubscriptionCache>,
    storage: Arc<dyn crate::storage::StorageBackend>,
    last_status: crate::opc_ua::StatusCache,
    node_to_metric: Arc<
        opcua::sync::RwLock<
            std::collections::HashMap<opcua::types::NodeId, (String, String)>,
        >,
    >,
    ns: u16,
    initial: Arc<AppConfig>,
    mut config_rx: watch::Receiver<Arc<AppConfig>>,
    cancel_token: CancellationToken,
) {
    // Iter-2 review P31: consume the initial publish so the first
    // `changed()` waits for the next SIGHUP rather than firing
    // immediately on a freshly-subscribed receiver.
    let _ = config_rx.borrow_and_update();
    let mut prev = initial;
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(
                    operation = "config_reload_listener_stopped",
                    subsystem = "opcua",
                    "OPC UA config-listener stopping (cancel)"
                );
                return;
            }
            changed = config_rx.changed() => {
                if changed.is_err() {
                    // Iter-1 review P15: sender dropped without
                    // cancel-token fire = anomalous. warn! not info!.
                    warn!(
                        operation = "config_reload_listener_stopped",
                        subsystem = "opcua",
                        "OPC UA config-listener stopping (sender dropped without cancel)"
                    );
                    return;
                }
                let new_config = config_rx.borrow_and_update().clone();

                // Step 1 — emit the Story-9-7-pinned diff log for
                // backward compat with the integration test
                // `topology_change_logs_seam_for_9_8`.
                log_topology_diff(&prev, &new_config);

                // Step 2 — Story 9-8 apply pass. Reads the new config's
                // staleness threshold so runtime-added closures
                // capture the live value at add time (note: existing
                // closures from earlier adds keep their captured
                // threshold — issue #113 carry-forward).
                let stale_threshold = new_config
                    .opcua
                    .stale_threshold_seconds
                    .unwrap_or(crate::opc_ua::DEFAULT_STALE_THRESHOLD_SECS);
                let outcome = crate::opcua_topology_apply::apply_diff_to_address_space(
                    &prev,
                    &new_config,
                    &manager,
                    &subscriptions,
                    &storage,
                    &last_status,
                    &node_to_metric,
                    ns,
                    stale_threshold,
                );

                // Step 3 — apply outcome audit event + prev advancement.
                //
                // Iter-2 review IP1 (Edge E-H1-iter2 + Blind B-H1-iter2
                // converged HIGH-REG): the iter-1 P2 "keep prev on any
                // Failed" guard combined with iter-1 P1's
                // None-as-failure capture created an unrecoverable
                // replay loop: Phase 2/3 partial-fail → P2 keeps
                // prev → next reload re-computes same diff → Phase 2
                // hits already-deleted NodeIds → P1 routes to
                // Failed{REMOVE_FAILED} → loop forever.
                //
                // Refined iter-2 semantics — advance `prev` UNLESS
                // the failure reason is SET_ATTRIBUTES_FAILED (the
                // ONLY reason now emitted from Phase 1, since
                // iter-2 IP1 demoted Phase 4's rename-failure to
                // warn-and-continue). Phase 1 failure means nothing
                // was committed to the address space; retry with
                // the same prev is correct. All other Failed paths
                // (REMOVE_FAILED from Phase 2, ADD_FAILED from
                // Phase 3) imply partial commit — advance prev to
                // avoid the retry loop; the operator-visible
                // Failed event tells them what to investigate.
                let mutation_succeeded = match &outcome {
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::NoChange
                    | crate::opcua_topology_apply::AddressSpaceMutationOutcome::Applied {
                        ..
                    } => true,
                    // Phase 1 failure: nothing committed → keep
                    // prev so retry has a chance.
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::Failed {
                        reason,
                        ..
                    } if *reason
                        == crate::opcua_topology_apply::failure_reason::SET_ATTRIBUTES_FAILED =>
                    {
                        false
                    }
                    // Phase 2/3 partial-failure: some mutations
                    // committed → advance prev to avoid replay loop
                    // (per iter-2 IP1).
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::Failed {
                        ..
                    } => true,
                };
                match outcome {
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::NoChange => {
                        // Diff was empty — already logged via
                        // log_topology_diff (which returns false and
                        // emits nothing on no-changes); nothing more
                        // to emit here.
                    }
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::Applied {
                        counts,
                        duration_ms,
                    } => {
                        info!(
                            event = "address_space_mutation_succeeded",
                            added_applications = counts.added_applications,
                            removed_applications = counts.removed_applications,
                            added_devices = counts.added_devices,
                            removed_devices = counts.removed_devices,
                            added_metrics = counts.added_metrics,
                            removed_metrics = counts.removed_metrics,
                            added_commands = counts.added_commands,
                            removed_commands = counts.removed_commands,
                            renamed_devices = counts.renamed_devices,
                            duration_ms,
                            "OPC UA address space mutated per topology diff"
                        );
                    }
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::Failed {
                        counts,
                        duration_ms,
                        reason,
                        error,
                    } => {
                        // Iter-3 review TP1 (Edge E-H1-iter3): make the
                        // warn message reason-aware. After iter-2 IP1
                        // refined the prev-advancement guard, only
                        // Phase 1 (SET_ATTRIBUTES_FAILED) failures
                        // keep prev unchanged; Phase 2/3 partial
                        // failures advance prev to avoid the replay
                        // loop. The prior message text said "prev not
                        // advanced; retry to converge" for ALL Failed
                        // outcomes, which mis-directed operators
                        // following a Phase 2/3 partial failure (where
                        // prev IS advanced and retry would see an
                        // empty diff).
                        let retry_hint = if reason
                            == crate::opcua_topology_apply::failure_reason::SET_ATTRIBUTES_FAILED
                        {
                            "prev not advanced — retry SIGHUP / CRUD reload to converge \
                             (Phase 1 failure means no mutations were committed)"
                        } else {
                            "prev advanced — address space may be in partial-apply state; \
                             inspect counts + error and reconcile manually if needed \
                             (subsequent reloads will not re-attempt this diff)"
                        };
                        warn!(
                            event = "address_space_mutation_failed",
                            reason,
                            error,
                            duration_ms,
                            added_applications = counts.added_applications,
                            removed_applications = counts.removed_applications,
                            added_devices = counts.added_devices,
                            removed_devices = counts.removed_devices,
                            added_metrics = counts.added_metrics,
                            removed_metrics = counts.removed_metrics,
                            added_commands = counts.added_commands,
                            removed_commands = counts.removed_commands,
                            renamed_devices = counts.renamed_devices,
                            "OPC UA address-space mutation failed: {retry_hint}"
                        );
                    }
                }

                if mutation_succeeded {
                    prev = new_config;
                }
            }
        }
    }
}

/// Iter-1 review P23 — emits the `event="topology_change_detected"`
/// info log carrying `added_devices` / `removed_devices` /
/// `modified_devices` field counts when the topology has changed.
///
/// Returns `true` if the log was emitted (i.e., the topology actually
/// differs); `false` for an unchanged topology. Public so integration
/// tests for AC#4 can drive the emission without standing up a full
/// `OpcgwHistoryNodeManager`.
pub fn log_topology_diff(prev: &AppConfig, new: &AppConfig) -> bool {
    let diff = topology_device_diff(prev, new);
    if diff.has_changes() {
        // Iter-1 review P6: spec AC#4 mandates `event=` on this audit
        // line (the SIGHUP listener also uses `event=`). Was
        // `operation=` before.
        info!(
            event = "topology_change_detected",
            added_devices = diff.added,
            removed_devices = diff.removed,
            modified_devices = diff.modified,
            story_9_8_seam = true,
            "Topology change detected; Story 9-8 owns the address-space apply"
        );
        true
    } else {
        false
    }
}

/// Coarse device-level topology diff between two configs. Identifies
/// devices by `(application_id, device_id)`; "modified" means the
/// device exists in both with the same composite ID but its
/// `read_metric_list` or `device_name` differs.
struct TopologyDeviceDiff {
    added: usize,
    removed: usize,
    modified: usize,
}

impl TopologyDeviceDiff {
    fn has_changes(&self) -> bool {
        self.added != 0 || self.removed != 0 || self.modified != 0
    }
}

fn topology_device_diff(old: &AppConfig, new: &AppConfig) -> TopologyDeviceDiff {
    use std::collections::HashMap;

    type DeviceKey = (String, String);

    fn collect_devices(cfg: &AppConfig) -> HashMap<DeviceKey, &crate::config::ChirpstackDevice> {
        let mut map = HashMap::new();
        for app in &cfg.application_list {
            for dev in &app.device_list {
                map.insert((app.application_id.clone(), dev.device_id.clone()), dev);
            }
        }
        map
    }

    let old_devices = collect_devices(old);
    let new_devices = collect_devices(new);

    let mut added = 0usize;
    let mut removed = 0usize;
    let mut modified = 0usize;

    for (key, new_dev) in &new_devices {
        match old_devices.get(key) {
            None => added += 1,
            Some(old_dev) => {
                // Iter-2 review P26: include `device_command_list` in
                // the modified-device count so this helper agrees
                // with `classify_diff` (which also flags command-list
                // edits as topology changes via `command_list_equal`).
                // Without this, a SIGHUP that mutates only commands
                // would set `includes_topology_change=true` in the
                // ReloadOutcome but emit NO `topology_change_detected`
                // log — Story 9-8 would silently drop the change.
                if old_dev.device_name != new_dev.device_name
                    || !metrics_equal(&old_dev.read_metric_list, &new_dev.read_metric_list)
                    || !command_list_equal(
                        &old_dev.device_command_list,
                        &new_dev.device_command_list,
                    )
                {
                    modified += 1;
                }
            }
        }
    }
    for key in old_devices.keys() {
        if !new_devices.contains_key(key) {
            removed += 1;
        }
    }

    TopologyDeviceDiff {
        added,
        removed,
        modified,
    }
}

// ---------------------------------------------------------------------------
// Comparison helpers used by topology_device_diff
// ---------------------------------------------------------------------------

/// Iter-1 review P1: device command-list equality. Order-insensitive
/// comparison keyed by `command_id` so reordering the TOML entries
/// doesn't trigger a spurious topology-change log.
///
/// Iter-2 review P27: treat `None` as semantically equivalent to
/// `Some([])` — both represent "no commands". A user who edits the
/// TOML to remove an empty `[[application.device.command]]` block
/// (collapsing `Some([])` → `None`) should not trigger a spurious
/// topology-change log.
fn command_list_equal(
    a: &Option<Vec<crate::config::DeviceCommandCfg>>,
    b: &Option<Vec<crate::config::DeviceCommandCfg>>,
) -> bool {
    fn as_slice(opt: &Option<Vec<crate::config::DeviceCommandCfg>>) -> &[crate::config::DeviceCommandCfg] {
        opt.as_deref().unwrap_or(&[])
    }
    let a = as_slice(a);
    let b = as_slice(b);
    if a.len() != b.len() {
        return false;
    }
    let mut a: Vec<&crate::config::DeviceCommandCfg> = a.iter().collect();
    let mut b: Vec<&crate::config::DeviceCommandCfg> = b.iter().collect();
    a.sort_by_key(|c| c.command_id);
    b.sort_by_key(|c| c.command_id);
    a.iter().zip(b.iter()).all(|(x, y)| {
        let crate::config::DeviceCommandCfg {
            command_id,
            command_name,
            command_confirmed,
            command_port,
        } = *x;
        command_id == &y.command_id
            && command_name == &y.command_name
            && command_confirmed == &y.command_confirmed
            && command_port == &y.command_port
    })
}

fn metrics_equal(a: &[crate::config::ReadMetric], b: &[crate::config::ReadMetric]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // Iter-1 review P14: order-insensitive comparison. Sorting by
    // `metric_name` (which is the OPC UA NodeId discriminator and is
    // unique within a device) so a TOML reorder of semantically
    // equivalent entries doesn't trigger a spurious topology-change
    // log line.
    let mut a: Vec<&crate::config::ReadMetric> = a.iter().collect();
    let mut b: Vec<&crate::config::ReadMetric> = b.iter().collect();
    a.sort_by(|x, y| x.metric_name.cmp(&y.metric_name));
    b.sort_by(|x, y| x.metric_name.cmp(&y.metric_name));
    a.iter().zip(b.iter()).all(|(x, y)| {
        // Destructure pattern: future fields force compile error.
        let crate::config::ReadMetric {
            metric_name,
            chirpstack_metric_name,
            metric_type,
            metric_unit,
        } = *x;
        metric_name == &y.metric_name
            && chirpstack_metric_name == &y.chirpstack_metric_name
            && metric_type == &y.metric_type
            && metric_unit == &y.metric_unit
    })
}

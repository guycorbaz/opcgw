// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story 9-8 — Dynamic OPC UA address-space mutation.
//!
//! Closes FR24 (add/remove OPC UA nodes at runtime when configuration
//! changes) by walking the diff between the previous and the new
//! `AppConfig.application_list` and applying the mutations to the
//! running `OpcgwHistoryNodeManager`'s address space.
//!
//! Story 9-7 ships the watch-channel plumbing; this module is the
//! consumer the 9-7 stub seam (`src/config_reload.rs::run_opcua_config_listener`)
//! has been waiting for.
//!
//! Architecture:
//!
//! 1. `compute_diff(prev, new) -> AddressSpaceDiff`: pure synchronous
//!    function. Walks the two AppConfigs and materialises a fine-grained
//!    7-axis diff (added/removed applications/devices/metrics/commands +
//!    renamed devices). Modified metrics / commands are emitted as
//!    paired (remove, add) entries so the apply pass naturally rebuilds
//!    the closure with the new captures. Renamed devices are emitted as
//!    DisplayName-only entries — no delete+re-add (BrowseName is not
//!    mutable on existing nodes; v1 limitation documented in
//!    `docs/security.md § Dynamic OPC UA address-space mutation`).
//!
//! 2. `apply_diff_to_address_space(...)`: takes the diff plus all the
//!    handles the runtime mutation needs (manager, subscriptions, storage,
//!    last_status cache, node_to_metric registry, namespace index, stale
//!    threshold) and walks four phases:
//!      - Phase 1: `manager.set_attributes(subscriptions, …)` emits an
//!        explicit `BadNodeIdUnknown` status for every NodeId about to be
//!        deleted (Q2 mitigation per `9-0-spike-report.md:104-127` —
//!        without this subscribers freeze on last-good with no
//!        programmatic detection path).
//!      - Phase 2: `address_space.write().delete(...)` for every removed
//!        NodeId under a single write-lock acquisition (per 9-0 Q3:
//!        117 µs bulk-mutation hold = ~850× headroom under the sampler
//!        tick). Then `manager.remove_read_callback` / `remove_write_callback`
//!        and `node_to_metric.remove(...)`.
//!      - Phase 3: `address_space.write().add_folder` / `add_variables`
//!        for every new application/device/metric/command under a single
//!        write-lock acquisition, mirroring `OpcUa::add_nodes` exactly
//!        (issue #99 NodeId scheme + Story 8-3 AccessLevel + historizing
//!        + initial-variant matching). Then `add_read_callback` /
//!        `add_write_callback` and `node_to_metric.insert(...)`.
//!      - Phase 4: DisplayName-only `set_attributes` for renamed devices.
//!
//! Spike findings (load-bearing):
//!
//!  - Q1 (add path): a fresh subscription on a runtime-added variable
//!    receives notifications within ~1s of CreateMonitoredItems —
//!    `9-0-spike-report.md:74-80`.
//!  - Q2 (remove path): subscriptions on a deleted variable go silent
//!    (frozen-last-good) unless `set_attributes(BadNodeIdUnknown)` is
//!    emitted first — `9-0-spike-report.md:104-127`. Load-bearing for
//!    `tests/opcua_dynamic_address_space_apply.rs::ac2_remove_device_emits_bad_node_id_unknown_before_delete`.
//!  - Q3 (sibling isolation): bulk add of 11 nodes under a single write
//!    lock = 117 µs = ~850× under the sampler tick —
//!    `9-0-spike-report.md:130-160`. At typical opcgw scales (≤100
//!    devices × ≤20 metrics = 2000 nodes) no mutation chunking needed.
//!  - Stale read-callback closure leak: `SimpleNodeManagerImpl` does not
//!    expose `remove_read_callback`. Story 9-8 ships option (b) per
//!    spec: stub method on `OpcgwHistoryNodeManager` returns `false` +
//!    logs once. v1 limitation documented in `deferred-work.md`.

// The module-level doc comment above mixes numbered and bullet
// lists; rustfmt-style indentation is fine but clippy's
// doc_lazy_continuation lint flags the continuation lines of the
// nested bullets. The structure is intentionally hierarchical;
// re-formatting would lose readability for an already-stable doc.
#![allow(clippy::doc_lazy_continuation)]
// The diff-type fields below (e.g. RemovedApplication.application_name,
// RenamedDevice.old_name, RemovedDevice.device_name) are populated by
// compute_diff for use in the audit-event field set + future consumers
// (the apply pass reads device_id / new_name / metric_name; the other
// fields are intentionally captured for the success/failure log line
// and for test introspection). The ADD_FAILED / REMOVE_FAILED reason
// constants are reserved for future Phase-2/3 failure paths that
// `apply_diff_to_address_space` doesn't yet trip (current failures
// route through SET_ATTRIBUTES_FAILED only).
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use opcua::server::address_space::{AccessLevel, Variable};
use opcua::server::SubscriptionCache;
use opcua::sync::RwLock as OpcuaRwLock;
use opcua::types::{
    AttributeId, DataValue, DateTime, LocalizedText, NodeId, NumericRange, StatusCode, Variant,
};
use tracing::{debug, trace, warn};

use crate::config::{
    AppConfig, ChirpStackApplications, ChirpstackDevice, DeviceCommandCfg, OpcMetricTypeConfig,
    ReadMetric,
};
use crate::opc_ua::{OpcUa, StatusCache};
use crate::opc_ua_history::{
    self as opc_ua_history, OpcgwHistoryNodeManager,
};
use crate::storage::StorageBackend;

/// Fine-grained 7-axis diff between two `AppConfig.application_list`
/// snapshots. Materialises modified metrics/commands as paired
/// (remove, add) entries so the apply pass rebuilds the closures
/// against the new parameters. Renamed devices are emitted as
/// DisplayName-only entries — BrowseName is not mutable on existing
/// nodes (v1 limitation).
#[derive(Debug, Clone, Default)]
pub struct AddressSpaceDiff {
    /// New application IDs (folder under `Objects` is added; child
    /// devices/metrics/commands flatten into the other vectors so the
    /// apply pass does not need to re-walk the new application body).
    pub added_applications: Vec<AddedApplication>,
    /// Application IDs to remove (folder under `Objects` is deleted
    /// last; child node IDs are pre-computed and flattened into the
    /// other removed vectors).
    pub removed_applications: Vec<RemovedApplication>,
    /// Devices added under an existing or freshly-added application.
    pub added_devices: Vec<AddedDevice>,
    /// Devices removed from an existing application (or from an
    /// application that's also being removed).
    pub removed_devices: Vec<RemovedDevice>,
    /// Metrics added under an existing or freshly-added device.
    pub added_metrics: Vec<AddedMetric>,
    /// Metrics removed from an existing or freshly-removed device.
    pub removed_metrics: Vec<RemovedMetric>,
    /// Commands added under an existing or freshly-added device.
    pub added_commands: Vec<AddedCommand>,
    /// Commands removed from an existing or freshly-removed device.
    pub removed_commands: Vec<RemovedCommand>,
    /// Devices where only `device_name` changed (same `device_id`).
    /// DisplayName-only mutation — no delete+re-add (would invalidate
    /// the NodeId for clients holding references).
    pub renamed_devices: Vec<RenamedDevice>,
}

/// Coarse counts derived from `AddressSpaceDiff`. Used in the
/// `event="address_space_mutation_succeeded"` / `…_failed` audit logs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DiffCounts {
    pub added_applications: usize,
    pub removed_applications: usize,
    pub added_devices: usize,
    pub removed_devices: usize,
    pub added_metrics: usize,
    pub removed_metrics: usize,
    pub added_commands: usize,
    pub removed_commands: usize,
    pub renamed_devices: usize,
}

impl DiffCounts {
    /// True if every axis is zero.
    pub fn is_empty(&self) -> bool {
        self.added_applications == 0
            && self.removed_applications == 0
            && self.added_devices == 0
            && self.removed_devices == 0
            && self.added_metrics == 0
            && self.removed_metrics == 0
            && self.added_commands == 0
            && self.removed_commands == 0
            && self.renamed_devices == 0
    }
}

impl AddressSpaceDiff {
    /// True if at least one axis is non-empty.
    pub fn has_changes(&self) -> bool {
        !self.counts().is_empty()
    }

    /// Snapshot the per-axis counts (cheap; just `.len()` of each Vec).
    pub fn counts(&self) -> DiffCounts {
        DiffCounts {
            added_applications: self.added_applications.len(),
            removed_applications: self.removed_applications.len(),
            added_devices: self.added_devices.len(),
            removed_devices: self.removed_devices.len(),
            added_metrics: self.added_metrics.len(),
            removed_metrics: self.removed_metrics.len(),
            added_commands: self.added_commands.len(),
            removed_commands: self.removed_commands.len(),
            renamed_devices: self.renamed_devices.len(),
        }
    }
}

/// A newly-added application (folder under `Objects`). Owns clones of
/// the application metadata so the apply pass does not need the
/// original `AppConfig` reference.
#[derive(Debug, Clone)]
pub struct AddedApplication {
    pub application_id: String,
    pub application_name: String,
}

/// A removed application. Holds the application_id (for folder
/// deletion) + the application_name (for logging). Child device /
/// metric / command removals are not nested here — they live in the
/// flat `removed_devices` / `removed_metrics` / `removed_commands`
/// vectors, all pre-populated by `compute_diff`.
#[derive(Debug, Clone)]
pub struct RemovedApplication {
    pub application_id: String,
    pub application_name: String,
}

/// A newly-added device under a known application.
#[derive(Debug, Clone)]
pub struct AddedDevice {
    pub application_id: String,
    pub device_id: String,
    pub device_name: String,
}

/// A removed device.
#[derive(Debug, Clone)]
pub struct RemovedDevice {
    pub application_id: String,
    pub device_id: String,
    pub device_name: String,
}

/// A newly-added metric.
#[derive(Debug, Clone)]
pub struct AddedMetric {
    pub application_id: String,
    pub device_id: String,
    pub metric_name: String,
    pub chirpstack_metric_name: String,
    pub metric_type: OpcMetricTypeConfig,
}

/// A removed metric. The pre-computed `node_id_identifier` matches the
/// issue #99 scheme `format!("{device_id}/{metric_name}")` so the
/// apply pass does not re-derive it.
#[derive(Debug, Clone)]
pub struct RemovedMetric {
    pub application_id: String,
    pub device_id: String,
    pub metric_name: String,
}

/// A newly-added command.
#[derive(Debug, Clone)]
pub struct AddedCommand {
    pub application_id: String,
    pub device_id: String,
    pub command_id: i32,
    pub command_name: String,
    pub command_port: i32,
    pub command_confirmed: bool,
}

/// A removed command.
#[derive(Debug, Clone)]
pub struct RemovedCommand {
    pub application_id: String,
    pub device_id: String,
    pub command_id: i32,
    pub command_name: String,
}

/// A device that exists in both configs with the same `device_id` but
/// a different `device_name`. v1: DisplayName-only mutation (preserves
/// NodeId); BrowseName stays at the old name.
#[derive(Debug, Clone)]
pub struct RenamedDevice {
    pub application_id: String,
    pub device_id: String,
    pub old_name: String,
    pub new_name: String,
}

/// Outcome of an `apply_diff_to_address_space` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddressSpaceMutationOutcome {
    /// `compute_diff` returned an empty diff; no mutation attempted.
    NoChange,
    /// Mutation succeeded; counts + duration are reported in the
    /// success audit event by the caller.
    Applied { counts: DiffCounts, duration_ms: u64 },
    /// Mutation failed at the named phase; the failure is reported via
    /// the `event="address_space_mutation_failed"` warn event by the
    /// caller. The address space MAY be in a partial-apply state — per
    /// `9-0-spike-report.md:196` rollback is not required (subscribers
    /// see silent or BadNodeIdUnknown notifications, not crashes), so
    /// operators retry SIGHUP / CRUD without distributed-rollback
    /// machinery.
    Failed {
        counts: DiffCounts,
        duration_ms: u64,
        reason: &'static str,
        error: String,
    },
}

/// Reason tags for `AddressSpaceMutationOutcome::Failed`. Stable
/// strings pinned by the `docs/logging.md` operations table — do not
/// rename without updating the table at the same time.
pub mod failure_reason {
    pub const SET_ATTRIBUTES_FAILED: &str = "set_attributes_failed";
    pub const ADD_FAILED: &str = "add_failed";
    pub const REMOVE_FAILED: &str = "remove_failed";
}

// =====================================================================
// apply_diff_to_address_space — Phase 1/2/3/4 runtime mutation
// =====================================================================

/// Apply a previously-computed diff (`compute_diff`) to the running
/// OPC UA address space. Walks four phases:
///
///  - **Phase 1 (Q2 mitigation)**: emit `set_values(DataValue { status:
///    BadNodeIdUnknown, .. })` for every NodeId about to be deleted so
///    subscribed clients see an explicit transition instead of going
///    silent (Behaviour B / frozen-last-good per
///    `9-0-spike-report.md:104-127`).
///  - **Phase 2 (delete)**: acquire `manager.address_space().write()`
///    once, call `delete` on every removed NodeId (metrics, commands,
///    device folders, application folders — children-first ordering).
///    Then call `remove_read_callback` / `remove_write_callback`
///    (option-b stub — v1 limitation, see `opc_ua_history`) and update
///    the `node_to_metric` registry.
///  - **Phase 3 (add)**: acquire `manager.address_space().write()`
///    once, call `add_folder` / `add_variables` for every added entry
///    mirroring `OpcUa::add_nodes` exactly (issue #99 NodeId scheme +
///    Story 8-3 AccessLevel + historizing + initial-variant matching).
///    Then register read/write callbacks via
///    `manager.inner().simple().add_read_callback(...)` and update the
///    `node_to_metric` registry.
///  - **Phase 4 (rename)**: emit DisplayName-only `set_attributes` for
///    each renamed device — preserves the NodeId (which is keyed by
///    `device_id`, not `device_name`).
///
/// **Lock-hold envelope**: Phase 1 + Phase 4 each acquire the address
/// space write lock internally (via `set_values` / `set_attributes`).
/// Phase 2 + Phase 3 each acquire it once explicitly. At typical
/// opcgw scales (≤100 devices × ≤20 metrics = 2 000 nodes) the bulk
/// per-phase hold is well under the 100 ms sampler tick (9-0 Q3:
/// 11-node bulk = 117 µs; ~850× headroom).
///
/// **No transactional rollback** per `9-0-spike-report.md:196` —
/// botched mutations produce silent subscribers, not crashes, so a
/// partial-apply failure returns
/// [`AddressSpaceMutationOutcome::Failed`] without attempting to
/// rewind already-applied phases. Operators retry SIGHUP / CRUD; the
/// failed event tells them which phase tripped.
#[allow(clippy::too_many_arguments)]
pub fn apply_diff_to_address_space(
    prev: &AppConfig,
    new: &AppConfig,
    manager: &Arc<OpcgwHistoryNodeManager>,
    subscriptions: &Arc<SubscriptionCache>,
    storage: &Arc<dyn StorageBackend>,
    last_status: &StatusCache,
    node_to_metric: &Arc<OpcuaRwLock<HashMap<NodeId, (String, String)>>>,
    ns: u16,
    stale_threshold: u64,
) -> AddressSpaceMutationOutcome {
    let start = Instant::now();
    let diff = compute_diff(prev, new);
    if !diff.has_changes() {
        return AddressSpaceMutationOutcome::NoChange;
    }
    let counts = diff.counts();
    trace!(
        added_applications = counts.added_applications,
        removed_applications = counts.removed_applications,
        added_devices = counts.added_devices,
        removed_devices = counts.removed_devices,
        added_metrics = counts.added_metrics,
        removed_metrics = counts.removed_metrics,
        added_commands = counts.added_commands,
        removed_commands = counts.removed_commands,
        renamed_devices = counts.renamed_devices,
        "apply_diff_to_address_space starting"
    );

    // -----------------------------------------------------------------
    // Phase 1 — Q2 mitigation: BadNodeIdUnknown set_values for every
    // doomed NodeId. Single call (one internal write-lock acquisition).
    // -----------------------------------------------------------------
    let transition_pairs = collect_doomed_node_ids(&diff, ns);
    if !transition_pairs.is_empty() {
        let now = DateTime::now();
        let bad_dvs: Vec<(NodeId, DataValue)> = transition_pairs
            .iter()
            .map(|node_id| {
                // async-opcua's default monitored-item filter
                // (`FilterType::None`) compares ONLY `value.value`,
                // ignoring `value.status`
                // (`monitored_item.rs:514-517`). To force a
                // DataChangeNotification, set the value field to
                // `Variant::Empty` — a distinct sentinel that
                // differs from the cached read-callback value (which
                // is typically a typed `Variant::Float`/`Int`/etc.).
                // The status field carries the actual operator
                // signal: `BadNodeIdUnknown`. Subscribers observe
                // both the value change AND the status transition.
                let dv = DataValue {
                    value: Some(Variant::Empty),
                    status: Some(StatusCode::BadNodeIdUnknown),
                    source_timestamp: Some(now),
                    source_picoseconds: None,
                    server_timestamp: Some(now),
                    server_picoseconds: None,
                };
                (node_id.clone(), dv)
            })
            .collect();
        let none_range: Option<&NumericRange> = None;
        let iter = bad_dvs.iter().map(|(n, dv)| (n, none_range, dv.clone()));
        if let Err(e) = manager.set_values(subscriptions, iter) {
            return AddressSpaceMutationOutcome::Failed {
                counts,
                duration_ms: start.elapsed().as_millis() as u64,
                reason: failure_reason::SET_ATTRIBUTES_FAILED,
                error: format!("{e}"),
            };
        }
        debug!(
            transition_count = transition_pairs.len(),
            "Phase 1 — BadNodeIdUnknown set_values emitted for doomed NodeIds"
        );
    }

    // -----------------------------------------------------------------
    // Phase 2 — address-space delete (single write-lock acquisition).
    // Children-first: metric variables → command variables → device
    // folders → application folders. Then callback + registry cleanup
    // OUTSIDE the lock.
    //
    // Iter-1 review P1 (Blind B-H1 + Edge E1 + Blind B-H5 converged):
    // capture `delete` return values and route a non-empty failure
    // count to `AddressSpaceMutationOutcome::Failed` with
    // `reason = REMOVE_FAILED`. `delete` returns `Option<NodeType>`;
    // `None` means the NodeId was not in the address space (which
    // indicates `compute_diff` and the live address-space state have
    // drifted — operationally a serious flag).
    // -----------------------------------------------------------------
    let mut phase2_failures = Vec::<String>::new();
    {
        let address_space = manager.address_space();
        let mut guard = address_space.write();
        for r in &diff.removed_metrics {
            let nid = metric_node_id(ns, &r.device_id, &r.metric_name);
            if guard.delete(&nid, true).is_none() {
                phase2_failures.push(format!(
                    "metric {}/{} (NodeId not found)",
                    r.device_id, r.metric_name
                ));
            }
        }
        for r in &diff.removed_commands {
            let nid = command_node_id(ns, &r.device_id, r.command_id);
            if guard.delete(&nid, true).is_none() {
                phase2_failures.push(format!(
                    "command {}/{} (NodeId not found)",
                    r.device_id, r.command_id
                ));
            }
        }
        for r in &diff.removed_devices {
            let nid = device_node_id(ns, &r.device_id);
            if guard.delete(&nid, true).is_none() {
                phase2_failures.push(format!(
                    "device folder {} (NodeId not found)",
                    r.device_id
                ));
            }
        }
        for r in &diff.removed_applications {
            let nid = application_node_id(ns, &r.application_id);
            if guard.delete(&nid, true).is_none() {
                phase2_failures.push(format!(
                    "application folder {} (NodeId not found)",
                    r.application_id
                ));
            }
        }
        // guard drops here
    }
    if !phase2_failures.is_empty() {
        return AddressSpaceMutationOutcome::Failed {
            counts,
            duration_ms: start.elapsed().as_millis() as u64,
            reason: failure_reason::REMOVE_FAILED,
            error: format!(
                "{} delete(s) returned None: [{}]",
                phase2_failures.len(),
                phase2_failures.join(", ")
            ),
        };
    }
    // Now drop read/write callbacks (option-b stubs return false but
    // emit a once-per-server-lifetime info log) and update the
    // node_to_metric registry.
    for r in &diff.removed_metrics {
        let nid = metric_node_id(ns, &r.device_id, &r.metric_name);
        let _ = opc_ua_history::remove_read_callback(manager, &nid);
        node_to_metric.write().remove(&nid);
    }
    for r in &diff.removed_commands {
        let nid = command_node_id(ns, &r.device_id, r.command_id);
        let _ = opc_ua_history::remove_write_callback(manager, &nid);
    }

    // -----------------------------------------------------------------
    // Phase 3 — address-space add (single write-lock acquisition).
    // Parents-first: application folders → device folders → metric
    // variables → command variables. Then callback + registry
    // population OUTSIDE the lock (the callback registration takes its
    // own inner lock on SimpleNodeManagerImpl, not the address-space
    // lock).
    //
    // Iter-1 review P1 (Blind B-H1 + Edge E1 converged): capture
    // `add_folder` (returns bool — false means parent NodeId not
    // found OR child NodeId already exists) and `add_variables`
    // (returns Vec<bool> — one entry per variable, false on collision)
    // return values; route any false to
    // `AddressSpaceMutationOutcome::Failed { reason = ADD_FAILED }`.
    // -----------------------------------------------------------------
    let mut phase3_failures = Vec::<String>::new();
    {
        let address_space = manager.address_space();
        let mut guard = address_space.write();
        for a in &diff.added_applications {
            let app_node = application_node_id(ns, &a.application_id);
            let added = guard.add_folder(
                &app_node,
                a.application_name.as_str(),
                a.application_name.as_str(),
                &NodeId::objects_folder_id(),
            );
            if !added {
                phase3_failures.push(format!(
                    "application folder {} (add_folder returned false)",
                    a.application_id
                ));
            }
        }
        for a in &diff.added_devices {
            let app_node = application_node_id(ns, &a.application_id);
            let dev_node = device_node_id(ns, &a.device_id);
            let added = guard.add_folder(
                &dev_node,
                a.device_name.as_str(),
                a.device_name.as_str(),
                &app_node,
            );
            if !added {
                phase3_failures.push(format!(
                    "device folder {} (add_folder returned false)",
                    a.device_id
                ));
            }
        }
        for a in &diff.added_metrics {
            let dev_node = device_node_id(ns, &a.device_id);
            let metric_node = metric_node_id(ns, &a.device_id, &a.metric_name);
            let initial_variant = initial_variant_for(&a.metric_type);
            let mut var = Variable::new(
                &metric_node,
                a.metric_name.as_str(),
                a.metric_name.as_str(),
                initial_variant,
            );
            // Story 8-3 invariant per epics.md:776 — HistoryRead requires
            // the access-level bit on the variable; without it
            // async-opcua's session dispatch returns BadUserAccessDenied
            // BEFORE OpcgwHistoryNodeManagerImpl::history_read_raw_modified
            // is reached. Mirror src/opc_ua.rs:1011-1017 exactly.
            var.set_access_level(AccessLevel::CURRENT_READ | AccessLevel::HISTORY_READ);
            var.set_user_access_level(AccessLevel::CURRENT_READ | AccessLevel::HISTORY_READ);
            var.set_historizing(true);
            let added = guard.add_variables(vec![var], &dev_node);
            if added.first().copied() != Some(true) {
                phase3_failures.push(format!(
                    "metric variable {}/{} (add_variables returned {:?})",
                    a.device_id, a.metric_name, added
                ));
            }
        }
        for a in &diff.added_commands {
            let dev_node = device_node_id(ns, &a.device_id);
            let cmd_node = command_node_id(ns, &a.device_id, a.command_id);
            let mut var = Variable::new(
                &cmd_node,
                a.command_name.as_str(),
                a.command_name.as_str(),
                0_i32,
            );
            var.set_writable(true);
            var.set_user_access_level(AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE);
            let added = guard.add_variables(vec![var], &dev_node);
            if added.first().copied() != Some(true) {
                phase3_failures.push(format!(
                    "command variable {}/{} (add_variables returned {:?})",
                    a.device_id, a.command_id, added
                ));
            }
        }
        // guard drops here
    }
    if !phase3_failures.is_empty() {
        return AddressSpaceMutationOutcome::Failed {
            counts,
            duration_ms: start.elapsed().as_millis() as u64,
            reason: failure_reason::ADD_FAILED,
            error: format!(
                "{} add(s) returned false: [{}]",
                phase3_failures.len(),
                phase3_failures.join(", ")
            ),
        };
    }

    // Now register callbacks + update node_to_metric. These take the
    // inner SimpleNodeManagerImpl's callback-registry lock, not the
    // address-space lock — disjoint from Phase 3's hold.
    for a in &diff.added_metrics {
        let metric_node = metric_node_id(ns, &a.device_id, &a.metric_name);
        let storage_clone = storage.clone();
        let last_status_clone = last_status.clone();
        let device_id = a.device_id.clone();
        let chirpstack_metric_name = a.chirpstack_metric_name.clone();
        manager
            .inner()
            .simple()
            .add_read_callback(metric_node.clone(), move |_, _, _| {
                OpcUa::get_value(
                    &storage_clone,
                    &last_status_clone,
                    device_id.clone(),
                    chirpstack_metric_name.clone(),
                    stale_threshold,
                )
            });
        node_to_metric.write().insert(
            metric_node,
            (a.device_id.clone(), a.chirpstack_metric_name.clone()),
        );
    }
    for a in &diff.added_commands {
        let cmd_node = command_node_id(ns, &a.device_id, a.command_id);
        let storage_clone = storage.clone();
        let device_id = a.device_id.clone();
        let cmd_cfg = DeviceCommandCfg {
            command_id: a.command_id,
            command_name: a.command_name.clone(),
            command_confirmed: a.command_confirmed,
            command_port: a.command_port,
        };
        manager
            .inner()
            .simple()
            .add_write_callback(cmd_node, move |data_value, _numeric_range| {
                OpcUa::set_command(&storage_clone, &device_id, &cmd_cfg, data_value)
            });
    }

    // -----------------------------------------------------------------
    // Phase 4 — Renamed devices: DisplayName-only set_attributes.
    // Preserves the NodeId (which is keyed by device_id, not
    // device_name). BrowseName is not mutated (v1 limitation; would
    // require delete+re-add, invalidating the NodeId for clients
    // holding references).
    //
    // Iter-2 review IP1 (Edge E-H1-iter2 + Blind B-H1-iter2 converged
    // HIGH-REG): demoted from "return Failed{SET_ATTRIBUTES_FAILED}"
    // to "warn-and-continue" (return Applied). Rationale: Phase 4 is
    // DisplayName-only — the address space's CORE state (folders,
    // variables, callbacks, registry) was correctly committed by
    // Phases 1-3. A Phase 4 failure leaves only the cosmetic
    // DisplayName stale; operators retry by toggling the name again
    // in a future reload. The previous Failed-return interacted with
    // iter-1 P2's "keep prev on Failed" guard to create an
    // unrecoverable replay loop: Phase 4 fails → P2 keeps prev →
    // next reload computes same diff → Phase 2 hits delete() on
    // already-deleted NodeIds → P1 routes to Failed{REMOVE_FAILED} →
    // permanent wedge. The demote breaks the loop at the source.
    //
    // A separate `event="address_space_rename_failed"` warn audit
    // event surfaces Phase 4 failures to operators without changing
    // the apply outcome.
    // -----------------------------------------------------------------
    if !diff.renamed_devices.is_empty() {
        // We need both the NodeId values *and* references that live
        // long enough for the set_attributes call. Materialise the
        // (NodeId, Variant) pairs in a Vec, then iterate borrowing.
        let rename_pairs: Vec<(NodeId, Variant)> = diff
            .renamed_devices
            .iter()
            .map(|r| {
                let nid = device_node_id(ns, &r.device_id);
                let val = Variant::LocalizedText(Box::new(LocalizedText::new("", &r.new_name)));
                (nid, val)
            })
            .collect();
        let iter = rename_pairs
            .iter()
            .map(|(n, v)| (n, AttributeId::DisplayName, v.clone()));
        if let Err(e) = manager.set_attributes(subscriptions, iter) {
            // Iter-2 IP1: warn-and-continue — do NOT fail the apply.
            // The renamed_devices count in the Applied outcome is
            // "attempted"; the separate audit event below reports
            // the failure with explicit count so operators can act
            // on it without wedging future reloads.
            warn!(
                event = "address_space_rename_failed",
                error = %e,
                renamed_count = diff.renamed_devices.len(),
                "Phase 4 DisplayName set_attributes failed; address-space \
                 core state (Phases 1-3) was committed successfully, only \
                 device DisplayName attributes are stale. Operators retry \
                 by editing the device_name to a NEW value in a future \
                 reload — note iter-2 IP1 advances prev on Applied, so \
                 reverting device_name to the EXACT original value will \
                 produce no diff and no Phase 4 retry; pick a different \
                 name (or toggle and revert through two reloads)."
            );
        }
    }

    AddressSpaceMutationOutcome::Applied {
        counts,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

// ----- NodeId helpers — Issue #99 scheme + per-device namespacing -----

/// Application folder NodeId. Per `src/opc_ua.rs:956`.
pub fn application_node_id(ns: u16, application_id: &str) -> NodeId {
    NodeId::new(ns, application_id.to_string())
}

/// Device folder NodeId. Per `src/opc_ua.rs:966`.
pub fn device_node_id(ns: u16, device_id: &str) -> NodeId {
    NodeId::new(ns, device_id.to_string())
}

/// Metric variable NodeId. Issue #99 fix at commit `9f823cc`
/// (`src/opc_ua.rs:976-979`) — `format!("{device_id}/{metric_name}")`
/// so two devices sharing a `metric_name` (e.g. "Moisture") resolve
/// to two distinct NodeIds instead of colliding.
pub fn metric_node_id(ns: u16, device_id: &str, metric_name: &str) -> NodeId {
    NodeId::new(ns, format!("{device_id}/{metric_name}"))
}

/// Command variable NodeId. Story 9-6 iter-1 D1 fix
/// (`src/opc_ua.rs:1077-1080`) — `format!("{device_id}/{command_id}")`
/// so two devices sharing a `command_id` resolve to two distinct
/// NodeIds (same root-cause class as issue #99 for metrics).
pub fn command_node_id(ns: u16, device_id: &str, command_id: i32) -> NodeId {
    NodeId::new(ns, format!("{device_id}/{command_id}"))
}

/// Map `OpcMetricTypeConfig` to the initial Variant used at variable
/// creation time. The variant's type determines the variable's
/// declared `DataType` attribute (Story 8-3 iter-1 P1 invariant per
/// `src/opc_ua.rs:991-998`).
pub fn initial_variant_for(metric_type: &OpcMetricTypeConfig) -> Variant {
    match metric_type {
        OpcMetricTypeConfig::Int => Variant::Int32(0),
        OpcMetricTypeConfig::Float => Variant::Float(0.0),
        OpcMetricTypeConfig::String => Variant::String(opcua::types::UAString::null()),
        OpcMetricTypeConfig::Bool => Variant::Boolean(false),
    }
}

/// Phase 1 helper — collect every NodeId that's about to be deleted,
/// so the Q2 mitigation `set_values(BadNodeIdUnknown)` call has a
/// single iterator to walk. Order matches Phase 2's delete order
/// (children first) so subscribers on child NodeIds see the
/// transition before the parent folder vanishes.
fn collect_doomed_node_ids(diff: &AddressSpaceDiff, ns: u16) -> Vec<NodeId> {
    let mut out = Vec::with_capacity(
        diff.removed_metrics.len()
            + diff.removed_commands.len()
            + diff.removed_devices.len()
            + diff.removed_applications.len(),
    );
    for r in &diff.removed_metrics {
        out.push(metric_node_id(ns, &r.device_id, &r.metric_name));
    }
    for r in &diff.removed_commands {
        out.push(command_node_id(ns, &r.device_id, r.command_id));
    }
    // Iter-1 review P5 (Blind B-H4 + Edge E5 converged): device +
    // application folders are NodeType::Object, not Variable.
    // `set_values` (used here for Q2 mitigation) is **Variable-only**:
    // its match arm at `~/.cargo/registry/.../async-opcua-server-0.17.1/src/node_manager/memory/mod.rs:208`
    // returns `Err(BadAttributeIdInvalid)` on any NodeType other than
    // Variable / VariableType, short-circuiting the entire iter().
    // We intentionally OMIT folder NodeIds from the doomed list so
    // the variable mitigation upstream stays intact.
    //
    // Iter-2 review IP4 (Edge E-H5-iter2): note that `set_attributes`
    // is a DIFFERENT API path — per-AttributeId routed — and DOES
    // support writing attributes (e.g. DisplayName) of Object nodes.
    // Phase 4's device-rename uses `set_attributes` against folder
    // NodeIds successfully. So "folders are Object-typed" is NOT a
    // uniform "attribute-blind" property; it's a `set_values`-vs-
    // `set_attributes` distinction. Subscribers directly monitoring
    // a folder's Value attribute (rare in OPC UA — folders are
    // usually browsed, not subscribed; folders have no Value
    // attribute) see the folder vanish on next browse but get no
    // explicit BadNodeIdUnknown notification. Documented in
    // `docs/security.md § Dynamic OPC UA address-space mutation §
    // v1 limitations`. The original dead `let _ = …` bindings have
    // been removed.
    out
}

// =====================================================================
// compute_diff
// =====================================================================

/// Compute the 7-axis diff between `prev` and `new`'s `application_list`.
///
/// Pure synchronous function. Does not touch the OPC UA address space.
/// Modified metrics/commands are materialised as paired (remove, add)
/// entries on the same NodeId so the apply pass naturally rebuilds the
/// callback closure with the new captures (e.g. a `metric_type` change
/// from `Int` to `Float` requires a new initial-variant + a fresh
/// read-callback closure).
///
/// **Device renames** (same `device_id`, different `device_name`) are
/// **not** topology mutations — they materialise in `renamed_devices`
/// and the apply pass emits DisplayName-only `set_attributes` (no
/// delete+re-add cycle).
///
/// # Children-flatten invariant (LOAD-BEARING, iter-2 review IP2 / Edge E-H2-iter2)
///
/// When an application or device is added/removed, this function
/// MUST also emit per-child entries in `added_devices` /
/// `added_metrics` / `added_commands` (for adds) or `removed_devices`
/// / `removed_metrics` / `removed_commands` (for removes). The apply
/// pass's Phase 2 (delete) iterates the flat per-axis vectors and
/// relies on every child being explicitly enumerated — `address_space
/// .delete(node_id, delete_target_references=true)` cascades, but
/// Phase 2's `delete().is_none()` check (iter-1 P1) will route a
/// non-explicitly-enumerated cascaded child to
/// `Failed{REMOVE_FAILED}` because the explicit entry sees the node
/// already gone.
///
/// `push_added_device` / `push_removed_device` are the only entry
/// points for new-application and removed-application flattening
/// AND for direct device add/remove; they handle the per-axis push
/// internally. **Any future code path that adds entries to
/// `added_applications` / `removed_applications` / `added_devices` /
/// `removed_devices` directly (bypassing these helpers) MUST also
/// push the children**, or the apply pass will treat cascaded
/// deletions as failures.
pub fn compute_diff(prev: &AppConfig, new: &AppConfig) -> AddressSpaceDiff {
    let mut diff = AddressSpaceDiff::default();

    // Index applications by application_id for O(N) lookup.
    let prev_apps: std::collections::HashMap<&str, &ChirpStackApplications> = prev
        .application_list
        .iter()
        .map(|a| (a.application_id.as_str(), a))
        .collect();
    let new_apps: std::collections::HashMap<&str, &ChirpStackApplications> = new
        .application_list
        .iter()
        .map(|a| (a.application_id.as_str(), a))
        .collect();

    // Walk new applications first: added or shared-with-walk.
    for (app_id, new_app) in &new_apps {
        match prev_apps.get(app_id) {
            None => {
                // Entirely new application: emit one added_applications
                // entry + flatten its children into added_devices /
                // added_metrics / added_commands so the apply pass does
                // not need to walk into the application body.
                diff.added_applications.push(AddedApplication {
                    application_id: new_app.application_id.clone(),
                    application_name: new_app.application_name.clone(),
                });
                for dev in &new_app.device_list {
                    push_added_device(&mut diff, &new_app.application_id, dev);
                }
            }
            Some(prev_app) => {
                walk_devices(&mut diff, prev_app, new_app);
            }
        }
    }

    // Walk prev applications looking for ones absent from `new`.
    for (app_id, prev_app) in &prev_apps {
        if new_apps.contains_key(app_id) {
            continue;
        }
        // Entirely removed application: emit one removed_applications
        // entry + flatten its children into removed_devices /
        // removed_metrics / removed_commands.
        diff.removed_applications.push(RemovedApplication {
            application_id: prev_app.application_id.clone(),
            application_name: prev_app.application_name.clone(),
        });
        for dev in &prev_app.device_list {
            push_removed_device(&mut diff, &prev_app.application_id, dev);
        }
    }

    diff
}

/// Walk the device lists of two applications that share `application_id`.
fn walk_devices(
    diff: &mut AddressSpaceDiff,
    prev_app: &ChirpStackApplications,
    new_app: &ChirpStackApplications,
) {
    let prev_devs: std::collections::HashMap<&str, &ChirpstackDevice> = prev_app
        .device_list
        .iter()
        .map(|d| (d.device_id.as_str(), d))
        .collect();
    let new_devs: std::collections::HashMap<&str, &ChirpstackDevice> = new_app
        .device_list
        .iter()
        .map(|d| (d.device_id.as_str(), d))
        .collect();

    for (dev_id, new_dev) in &new_devs {
        match prev_devs.get(dev_id) {
            None => push_added_device(diff, &new_app.application_id, new_dev),
            Some(prev_dev) => walk_device_children(
                diff,
                &new_app.application_id,
                prev_dev,
                new_dev,
            ),
        }
    }
    for (dev_id, prev_dev) in &prev_devs {
        if new_devs.contains_key(dev_id) {
            continue;
        }
        push_removed_device(diff, &new_app.application_id, prev_dev);
    }
}

/// Walk a device that exists in both configs. Compares device_name +
/// read_metric_list + device_command_list; emits per-axis entries.
fn walk_device_children(
    diff: &mut AddressSpaceDiff,
    application_id: &str,
    prev_dev: &ChirpstackDevice,
    new_dev: &ChirpstackDevice,
) {
    if prev_dev.device_name != new_dev.device_name {
        diff.renamed_devices.push(RenamedDevice {
            application_id: application_id.to_string(),
            device_id: new_dev.device_id.clone(),
            old_name: prev_dev.device_name.clone(),
            new_name: new_dev.device_name.clone(),
        });
    }

    // ----- metrics -----
    let prev_metrics: std::collections::HashMap<&str, &ReadMetric> = prev_dev
        .read_metric_list
        .iter()
        .map(|m| (m.metric_name.as_str(), m))
        .collect();
    let new_metrics: std::collections::HashMap<&str, &ReadMetric> = new_dev
        .read_metric_list
        .iter()
        .map(|m| (m.metric_name.as_str(), m))
        .collect();

    for (mname, new_metric) in &new_metrics {
        match prev_metrics.get(mname) {
            None => push_added_metric(diff, application_id, &new_dev.device_id, new_metric),
            Some(prev_metric) => {
                // Modified metric (same metric_name, different other
                // fields) materialises as paired (remove, add) so the
                // closure is rebuilt with new captures.
                if !metric_equal(prev_metric, new_metric) {
                    push_removed_metric(diff, application_id, &new_dev.device_id, prev_metric);
                    push_added_metric(diff, application_id, &new_dev.device_id, new_metric);
                }
            }
        }
    }
    for (mname, prev_metric) in &prev_metrics {
        if !new_metrics.contains_key(mname) {
            push_removed_metric(diff, application_id, &new_dev.device_id, prev_metric);
        }
    }

    // ----- commands -----
    let prev_cmds_list: &[DeviceCommandCfg] = prev_dev
        .device_command_list
        .as_deref()
        .unwrap_or(&[]);
    let new_cmds_list: &[DeviceCommandCfg] = new_dev
        .device_command_list
        .as_deref()
        .unwrap_or(&[]);
    let prev_cmds: std::collections::HashMap<i32, &DeviceCommandCfg> = prev_cmds_list
        .iter()
        .map(|c| (c.command_id, c))
        .collect();
    let new_cmds: std::collections::HashMap<i32, &DeviceCommandCfg> = new_cmds_list
        .iter()
        .map(|c| (c.command_id, c))
        .collect();

    for (cid, new_cmd) in &new_cmds {
        match prev_cmds.get(cid) {
            None => push_added_command(diff, application_id, &new_dev.device_id, new_cmd),
            Some(prev_cmd) => {
                if !command_equal(prev_cmd, new_cmd) {
                    push_removed_command(diff, application_id, &new_dev.device_id, prev_cmd);
                    push_added_command(diff, application_id, &new_dev.device_id, new_cmd);
                }
            }
        }
    }
    for (cid, prev_cmd) in &prev_cmds {
        if !new_cmds.contains_key(cid) {
            push_removed_command(diff, application_id, &new_dev.device_id, prev_cmd);
        }
    }
}

fn push_added_device(diff: &mut AddressSpaceDiff, application_id: &str, dev: &ChirpstackDevice) {
    diff.added_devices.push(AddedDevice {
        application_id: application_id.to_string(),
        device_id: dev.device_id.clone(),
        device_name: dev.device_name.clone(),
    });
    for m in &dev.read_metric_list {
        push_added_metric(diff, application_id, &dev.device_id, m);
    }
    if let Some(cmds) = &dev.device_command_list {
        for c in cmds {
            push_added_command(diff, application_id, &dev.device_id, c);
        }
    }
}

fn push_removed_device(diff: &mut AddressSpaceDiff, application_id: &str, dev: &ChirpstackDevice) {
    diff.removed_devices.push(RemovedDevice {
        application_id: application_id.to_string(),
        device_id: dev.device_id.clone(),
        device_name: dev.device_name.clone(),
    });
    for m in &dev.read_metric_list {
        push_removed_metric(diff, application_id, &dev.device_id, m);
    }
    if let Some(cmds) = &dev.device_command_list {
        for c in cmds {
            push_removed_command(diff, application_id, &dev.device_id, c);
        }
    }
}

fn push_added_metric(
    diff: &mut AddressSpaceDiff,
    application_id: &str,
    device_id: &str,
    metric: &ReadMetric,
) {
    diff.added_metrics.push(AddedMetric {
        application_id: application_id.to_string(),
        device_id: device_id.to_string(),
        metric_name: metric.metric_name.clone(),
        chirpstack_metric_name: metric.chirpstack_metric_name.clone(),
        metric_type: metric.metric_type.clone(),
    });
}

fn push_removed_metric(
    diff: &mut AddressSpaceDiff,
    application_id: &str,
    device_id: &str,
    metric: &ReadMetric,
) {
    diff.removed_metrics.push(RemovedMetric {
        application_id: application_id.to_string(),
        device_id: device_id.to_string(),
        metric_name: metric.metric_name.clone(),
    });
}

fn push_added_command(
    diff: &mut AddressSpaceDiff,
    application_id: &str,
    device_id: &str,
    cmd: &DeviceCommandCfg,
) {
    diff.added_commands.push(AddedCommand {
        application_id: application_id.to_string(),
        device_id: device_id.to_string(),
        command_id: cmd.command_id,
        command_name: cmd.command_name.clone(),
        command_port: cmd.command_port,
        command_confirmed: cmd.command_confirmed,
    });
}

fn push_removed_command(
    diff: &mut AddressSpaceDiff,
    application_id: &str,
    device_id: &str,
    cmd: &DeviceCommandCfg,
) {
    diff.removed_commands.push(RemovedCommand {
        application_id: application_id.to_string(),
        device_id: device_id.to_string(),
        command_id: cmd.command_id,
        command_name: cmd.command_name.clone(),
    });
}

/// `ReadMetric` value-equality (`ReadMetric` does not derive `PartialEq`).
/// Two metrics with the same `metric_name` and identical other fields
/// are considered equal — no apply work needed.
fn metric_equal(a: &ReadMetric, b: &ReadMetric) -> bool {
    a.metric_name == b.metric_name
        && a.chirpstack_metric_name == b.chirpstack_metric_name
        && a.metric_type == b.metric_type
        && a.metric_unit == b.metric_unit
}

/// `DeviceCommandCfg` value-equality (the struct does not derive
/// `PartialEq`). Two commands sharing `command_id` and identical other
/// fields are considered equal.
fn command_equal(a: &DeviceCommandCfg, b: &DeviceCommandCfg) -> bool {
    a.command_id == b.command_id
        && a.command_name == b.command_name
        && a.command_confirmed == b.command_confirmed
        && a.command_port == b.command_port
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AppConfig, ChirpStackApplications, ChirpstackDevice, ChirpstackPollerConfig,
        CommandValidationConfig, DeviceCommandCfg, Global, OpcMetricTypeConfig, OpcUaConfig,
        ReadMetric, StorageConfig, WebConfig,
    };

    /// Build a minimal valid `AppConfig` for the diff tests. Mirrors
    /// the explicit-field pattern used by
    /// `src/config_reload.rs::tests::baseline` because the substructs
    /// do not derive `Default`.
    fn baseline() -> AppConfig {
        AppConfig {
            global: Global {
                debug: false,
                prune_interval_minutes: 60,
                command_delivery_poll_interval_secs: 5,
                command_delivery_timeout_secs: 60,
                command_timeout_check_interval_secs: 10,
                history_retention_days: 7,
            },
            logging: None,
            chirpstack: ChirpstackPollerConfig {
                server_address: "http://127.0.0.1:18080".to_string(),
                api_token: "tok".to_string(),
                tenant_id: "00000000-0000-0000-0000-000000000000".to_string(),
                polling_frequency: 10,
                retry: 30,
                delay: 1,
                list_page_size: 100,
            },
            opcua: OpcUaConfig {
                application_name: "test".to_string(),
                application_uri: "urn:test".to_string(),
                product_uri: "urn:test:product".to_string(),
                diagnostics_enabled: false,
                hello_timeout: Some(5),
                host_ip_address: Some("127.0.0.1".to_string()),
                host_port: Some(4855),
                create_sample_keypair: true,
                certificate_path: "own/cert.der".to_string(),
                private_key_path: "private/private.pem".to_string(),
                trust_client_cert: false,
                check_cert_time: false,
                pki_dir: "./pki".to_string(),
                user_name: "opcua-user".to_string(),
                user_password: "secret".to_string(),
                stale_threshold_seconds: Some(120),
                max_connections: None,
                max_subscriptions_per_session: None,
                max_monitored_items_per_sub: None,
                max_message_size: None,
                max_chunk_count: None,
                max_history_data_results_per_node: None,
            },
            storage: StorageConfig::default(),
            command_validation: CommandValidationConfig::default(),
            web: WebConfig::default(),
            application_list: vec![ChirpStackApplications {
                application_name: "App1".to_string(),
                application_id: "550e8400-e29b-41d4-a716-446655440001".to_string(),
                device_list: vec![ChirpstackDevice {
                    device_id: "dev-1".to_string(),
                    device_name: "Dev 1".to_string(),
                    read_metric_list: vec![ReadMetric {
                        metric_name: "temp".to_string(),
                        chirpstack_metric_name: "t".to_string(),
                        metric_type: OpcMetricTypeConfig::Float,
                        metric_unit: None,
                    }],
                    device_command_list: None,
                }],
            }],
        }
    }

    fn make_device(
        id: &str,
        name: &str,
        metrics: Vec<ReadMetric>,
        commands: Option<Vec<DeviceCommandCfg>>,
    ) -> ChirpstackDevice {
        ChirpstackDevice {
            device_id: id.to_string(),
            device_name: name.to_string(),
            read_metric_list: metrics,
            device_command_list: commands,
        }
    }

    fn make_metric(name: &str, chirp_name: &str, ty: OpcMetricTypeConfig) -> ReadMetric {
        ReadMetric {
            metric_name: name.to_string(),
            chirpstack_metric_name: chirp_name.to_string(),
            metric_type: ty,
            metric_unit: None,
        }
    }

    fn make_command(id: i32, name: &str, port: i32, confirmed: bool) -> DeviceCommandCfg {
        DeviceCommandCfg {
            command_id: id,
            command_name: name.to_string(),
            command_confirmed: confirmed,
            command_port: port,
        }
    }

    // -----------------------------------------------------------------
    // Test 1 — equal configs produce no diff
    // -----------------------------------------------------------------

    #[test]
    fn compute_diff_equal_configs_returns_no_changes() {
        let a = baseline();
        let b = baseline();
        let diff = compute_diff(&a, &b);
        assert!(!diff.has_changes(), "equal configs must produce empty diff");
        let counts = diff.counts();
        assert!(counts.is_empty());
        assert_eq!(counts.added_devices, 0);
        assert_eq!(counts.removed_metrics, 0);
    }

    // -----------------------------------------------------------------
    // Test 2 — adding a new device under existing application
    // -----------------------------------------------------------------

    #[test]
    fn compute_diff_adds_a_new_device_under_existing_application() {
        let prev = baseline();
        let mut new = baseline();
        // Push a second device with one metric + one command under the
        // existing application.
        new.application_list[0].device_list.push(make_device(
            "dev-2",
            "Dev 2",
            vec![make_metric("humidity", "h", OpcMetricTypeConfig::Float)],
            Some(vec![make_command(1, "Reboot", 10, false)]),
        ));

        let diff = compute_diff(&prev, &new);
        assert!(diff.has_changes());
        // Application unchanged
        assert_eq!(diff.added_applications.len(), 0);
        assert_eq!(diff.removed_applications.len(), 0);
        // One new device + its metric + its command
        assert_eq!(diff.added_devices.len(), 1);
        assert_eq!(diff.added_devices[0].device_id, "dev-2");
        assert_eq!(diff.added_metrics.len(), 1);
        assert_eq!(diff.added_metrics[0].metric_name, "humidity");
        assert_eq!(diff.added_commands.len(), 1);
        assert_eq!(diff.added_commands[0].command_id, 1);
        // No removals
        assert_eq!(diff.removed_devices.len(), 0);
        assert_eq!(diff.removed_metrics.len(), 0);
        assert_eq!(diff.removed_commands.len(), 0);
    }

    // -----------------------------------------------------------------
    // Test 3 — removing a device captures all child node IDs
    // -----------------------------------------------------------------

    #[test]
    fn compute_diff_removes_a_device_captures_all_child_node_ids() {
        let mut prev = baseline();
        // Augment the baseline device with a command so we can verify
        // child command capture on removal.
        prev.application_list[0].device_list[0].device_command_list =
            Some(vec![make_command(7, "Trigger", 20, true)]);
        let mut new = baseline();
        // Remove dev-1 from `new`.
        new.application_list[0].device_list.clear();

        let diff = compute_diff(&prev, &new);
        assert!(diff.has_changes());
        assert_eq!(diff.removed_devices.len(), 1);
        assert_eq!(diff.removed_devices[0].device_id, "dev-1");
        // The single metric `temp` must show up in removed_metrics.
        assert_eq!(diff.removed_metrics.len(), 1);
        assert_eq!(diff.removed_metrics[0].metric_name, "temp");
        assert_eq!(diff.removed_metrics[0].device_id, "dev-1");
        // The command must show up in removed_commands.
        assert_eq!(diff.removed_commands.len(), 1);
        assert_eq!(diff.removed_commands[0].command_id, 7);
        // No adds, no renames.
        assert_eq!(diff.added_devices.len(), 0);
        assert_eq!(diff.renamed_devices.len(), 0);
    }

    // -----------------------------------------------------------------
    // Test 4 — modified metric (different metric_type) materialises as
    // paired remove+add
    // -----------------------------------------------------------------

    #[test]
    fn compute_diff_modified_metric_materialises_as_remove_then_add() {
        let prev = baseline();
        let mut new = baseline();
        // Change the metric_type from Float to Int while keeping the
        // metric_name "temp".
        new.application_list[0].device_list[0].read_metric_list[0].metric_type =
            OpcMetricTypeConfig::Int;

        let diff = compute_diff(&prev, &new);
        assert!(diff.has_changes());
        // Same NodeId identifier (device_id+metric_name) shows up in
        // both vectors — apply pass deletes the old variable then
        // re-adds with the new type.
        assert_eq!(diff.added_metrics.len(), 1);
        assert_eq!(diff.added_metrics[0].metric_name, "temp");
        assert_eq!(diff.added_metrics[0].metric_type, OpcMetricTypeConfig::Int);
        assert_eq!(diff.removed_metrics.len(), 1);
        assert_eq!(diff.removed_metrics[0].metric_name, "temp");
        // Device itself is unchanged; not a rename.
        assert_eq!(diff.added_devices.len(), 0);
        assert_eq!(diff.removed_devices.len(), 0);
        assert_eq!(diff.renamed_devices.len(), 0);
    }

    // -----------------------------------------------------------------
    // Test 5 — renamed device (same device_id, different device_name)
    // materialises as DisplayName-only entry; no add/remove
    // -----------------------------------------------------------------

    #[test]
    fn compute_diff_renamed_device_emits_renamed_only_no_remove_or_add() {
        let prev = baseline();
        let mut new = baseline();
        new.application_list[0].device_list[0].device_name = "Renamed Dev".to_string();

        let diff = compute_diff(&prev, &new);
        assert!(diff.has_changes());
        assert_eq!(diff.renamed_devices.len(), 1);
        assert_eq!(diff.renamed_devices[0].device_id, "dev-1");
        assert_eq!(diff.renamed_devices[0].old_name, "Dev 1");
        assert_eq!(diff.renamed_devices[0].new_name, "Renamed Dev");
        // No add/remove on any axis — DisplayName change preserves
        // NodeId and child structure.
        assert_eq!(diff.added_devices.len(), 0);
        assert_eq!(diff.removed_devices.len(), 0);
        assert_eq!(diff.added_metrics.len(), 0);
        assert_eq!(diff.removed_metrics.len(), 0);
    }

    // -----------------------------------------------------------------
    // Test 6 — added application flattens to per-device + per-metric +
    // per-command entries
    // -----------------------------------------------------------------

    #[test]
    fn compute_diff_added_application_flattens_to_per_device_per_metric_entries() {
        let prev = baseline();
        let mut new = baseline();
        // Push an entirely new application with one device + two
        // metrics + one command.
        new.application_list.push(ChirpStackApplications {
            application_name: "App2".to_string(),
            application_id: "550e8400-e29b-41d4-a716-446655440002".to_string(),
            device_list: vec![make_device(
                "dev-2",
                "Dev 2",
                vec![
                    make_metric("m1", "c1", OpcMetricTypeConfig::Float),
                    make_metric("m2", "c2", OpcMetricTypeConfig::Int),
                ],
                Some(vec![make_command(1, "Cmd1", 10, false)]),
            )],
        });

        let diff = compute_diff(&prev, &new);
        assert!(diff.has_changes());
        assert_eq!(diff.added_applications.len(), 1);
        assert_eq!(diff.added_applications[0].application_name, "App2");
        // Flatten: device + 2 metrics + 1 command of the new app must
        // appear in the per-axis vectors so the apply pass does not
        // need to walk into the application body.
        assert_eq!(diff.added_devices.len(), 1);
        assert_eq!(diff.added_metrics.len(), 2);
        assert_eq!(diff.added_commands.len(), 1);
        // No removals.
        assert_eq!(diff.removed_applications.len(), 0);
        assert_eq!(diff.removed_devices.len(), 0);
    }

    // -----------------------------------------------------------------
    // Iter-1 review P10 (Blind B-H12) — NodeId-format equivalence pins.
    // These tests assert the apply-pass NodeId helpers match the
    // production startup-path format exactly. If the production path
    // at `src/opc_ua.rs:976-979` (metrics) or `:1077-1080` (commands)
    // ever changes its NodeId scheme, these tests fail and force the
    // apply pass to follow — preventing silently-orphaned runtime
    // NodeIds.
    // -----------------------------------------------------------------

    #[test]
    fn metric_node_id_matches_production_issue_99_scheme() {
        // Production scheme (post-issue #99 commit 9f823cc, verified
        // at `src/opc_ua.rs:976-979`):
        //   `NodeId::new(ns, format!("{}/{}", device.device_id, read_metric.metric_name))`
        let ns: u16 = 2;
        let device_id = "device-uuid-foo";
        let metric_name = "Moisture";
        let from_helper = metric_node_id(ns, device_id, metric_name);
        let expected =
            opcua::types::NodeId::new(ns, format!("{}/{}", device_id, metric_name));
        assert_eq!(
            from_helper, expected,
            "metric_node_id must mirror production add_nodes scheme exactly"
        );
    }

    #[test]
    fn command_node_id_matches_production_story_9_6_scheme() {
        // Production scheme (post-Story-9-6 iter-1 D1 fix, verified
        // at `src/opc_ua.rs:1077-1080`):
        //   `NodeId::new(ns, format!("{}/{}", device.device_id, command.command_id))`
        let ns: u16 = 2;
        let device_id = "device-uuid-bar";
        let command_id = 7_i32;
        let from_helper = command_node_id(ns, device_id, command_id);
        let expected =
            opcua::types::NodeId::new(ns, format!("{}/{}", device_id, command_id));
        assert_eq!(
            from_helper, expected,
            "command_node_id must mirror production add_nodes scheme exactly"
        );
    }

    #[test]
    fn device_node_id_matches_production_scheme() {
        // Production scheme (verified at `src/opc_ua.rs:966`):
        //   `NodeId::new(ns, device.device_id.clone())`
        let ns: u16 = 2;
        let device_id = "device-uuid-baz";
        let from_helper = device_node_id(ns, device_id);
        let expected = opcua::types::NodeId::new(ns, device_id.to_string());
        assert_eq!(from_helper, expected);
    }

    #[test]
    fn application_node_id_matches_production_scheme() {
        // Production scheme (verified at `src/opc_ua.rs:956`):
        //   `NodeId::new(ns, application.application_id.clone())`
        let ns: u16 = 2;
        let application_id = "550e8400-e29b-41d4-a716-446655440042";
        let from_helper = application_node_id(ns, application_id);
        let expected = opcua::types::NodeId::new(ns, application_id.to_string());
        assert_eq!(from_helper, expected);
    }
}

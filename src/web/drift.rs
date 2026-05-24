// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story C-4: inventory drift view.
//!
//! Compares opcgw's configured applications/devices/metrics against
//! ChirpStack's current inventory and classifies each resource into
//! `ok` / `stale` / `available` / `drifted`.
//!
//! Read-only — the endpoint does NOT mutate opcgw state. Action buttons
//! in the UI dispatch through existing CRUD paths. Operator-triggered
//! only (no background polling): every fetch forces `?refresh=true` on
//! the C-1 inventory endpoints so the comparison is never against a
//! stale cache.
//!
//! See `_bmad-output/implementation-artifacts/C-4-inventory-drift-view.md`
//! for the full spec.

use crate::chirpstack_inventory::{
    compute_observed_keys, fetch_applications, fetch_devices, stream_recent_device_uplinks,
    InventoryApplication, InventoryDevice, ObservedKey, WireType,
};
use crate::config::OpcMetricTypeConfig;
use crate::web::inventory::UPLINKS_LIMIT_DEFAULT;
use crate::web::AppState;
use axum::extract::{ConnectInfo, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

// ---------------------------------------------------------------------------
// Response shape
// ---------------------------------------------------------------------------

/// One opcgw-side application view embedded in a drift row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ApplicationOpcgwView {
    pub application_id: String,
    pub application_name: String,
}

/// One ChirpStack-side application view embedded in a drift row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ApplicationChirpstackView {
    pub id: String,
    pub name: String,
}

/// One opcgw-side device view embedded in a drift row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceOpcgwView {
    pub device_id: String,
    pub device_name: String,
}

/// One ChirpStack-side device view embedded in a drift row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DeviceChirpstackView {
    pub dev_eui: String,
    pub name: String,
    pub last_seen_at: Option<String>,
}

/// One opcgw-side metric view embedded in a drift row.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MetricOpcgwView {
    pub chirpstack_metric_name: String,
    pub metric_name: String,
    pub metric_type: &'static str,
}

/// One ChirpStack-side metric observation embedded in a drift row.
#[derive(Debug, Clone, Serialize)]
pub struct MetricChirpstackView {
    pub key: String,
    pub inferred_wire_type: &'static str,
    pub sample_value: serde_json::Value,
}

/// Drift detail block surfaced on `drifted` and soft-`stale` rows.
///
/// `reason` is a stable machine-parseable string. `opcgw_*` / `chirpstack_*`
/// pair fields are populated where applicable (e.g., `wire_type_mismatch`
/// fills `opcgw_type` + `inferred_type`; a name-drift row fills
/// `opcgw_name` + `chirpstack_name`).
#[derive(Debug, Clone, Serialize, Default, PartialEq, Eq)]
pub struct DriftDetails {
    pub reason: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opcgw_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chirpstack_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub opcgw_type: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inferred_type: Option<&'static str>,
}

/// One row in the `applications` section of the drift response.
#[derive(Debug, Clone, Serialize)]
pub struct ApplicationDriftRow {
    pub class: &'static str,
    pub opcgw: Option<ApplicationOpcgwView>,
    pub chirpstack: Option<ApplicationChirpstackView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift_details: Option<DriftDetails>,
}

/// One row in the `devices` section of the drift response.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceDriftRow {
    pub class: &'static str,
    pub application_id: String,
    pub opcgw: Option<DeviceOpcgwView>,
    pub chirpstack: Option<DeviceChirpstackView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift_details: Option<DriftDetails>,
}

/// One row in the `metrics` section of the drift response.
#[derive(Debug, Clone, Serialize)]
pub struct MetricDriftRow {
    pub class: &'static str,
    pub application_id: String,
    pub device_id: String,
    pub opcgw: Option<MetricOpcgwView>,
    pub chirpstack_observed: Option<MetricChirpstackView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drift_details: Option<DriftDetails>,
}

/// Class-bucket counts for the drift summary.
#[derive(Debug, Clone, Copy, Serialize, Default, PartialEq, Eq)]
pub struct DriftSummary {
    pub ok: usize,
    pub stale: usize,
    pub available: usize,
    pub drifted: usize,
    pub total: usize,
}

/// Top-level drift response body.
#[derive(Debug, Clone, Serialize)]
pub struct DriftResponse {
    pub applications: Vec<ApplicationDriftRow>,
    pub devices: Vec<DeviceDriftRow>,
    pub metrics: Vec<MetricDriftRow>,
    pub fetched_at: String,
    pub chirpstack_reachable: bool,
    pub summary: DriftSummary,
}

// ---------------------------------------------------------------------------
// Class enum + helpers
// ---------------------------------------------------------------------------

/// Drift class enum kept private to the module; the wire form is a stable
/// `&'static str` baked into the row structs to keep the JSON small and
/// the source-grep contract intact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriftClass {
    Ok,
    Stale,
    Available,
    Drifted,
}

impl DriftClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Stale => "stale",
            Self::Available => "available",
            Self::Drifted => "drifted",
        }
    }
}

// Reason codes used in DriftDetails.reason.
const REASON_NAME_DIFFERS: &str = "name_differs";
const REASON_NOT_IN_RECENT_UPLINKS: &str = "not_in_recent_uplinks";
const REASON_WIRE_TYPE_MISMATCH: &str = "wire_type_mismatch";

// ---------------------------------------------------------------------------
// Pure inputs for `compute_drift` — pulling these out of `AppConfig` /
// `InventoryApplication` keeps the diff logic dependency-light and
// trivially unit-testable.
// ---------------------------------------------------------------------------

/// Opcgw-side configured application + its devices + their configured metrics.
///
/// Built from `AppConfig.application_list` in [`build_opcgw_view`].
#[derive(Debug, Clone)]
pub struct OpcgwApplicationView {
    pub application_id: String,
    pub application_name: String,
    pub devices: Vec<OpcgwDeviceView>,
}

#[derive(Debug, Clone)]
pub struct OpcgwDeviceView {
    pub device_id: String,
    pub device_name: String,
    pub metrics: Vec<OpcgwMetricView>,
}

#[derive(Debug, Clone)]
pub struct OpcgwMetricView {
    pub chirpstack_metric_name: String,
    pub metric_name: String,
    pub metric_type: OpcMetricTypeConfig,
}

/// ChirpStack-side inventory snapshot used by the diff.
///
/// `applications` is the full applications list. `devices_by_app` maps
/// application_id → device list. `observed_by_device` maps
/// (application_id, dev_eui) → observed-key inference output. Devices
/// without uplinks have an empty Vec; the difference between "device not
/// in map" and "device in map with empty observed_keys" is load-bearing
/// for the metric soft-stale path.
#[derive(Debug, Clone, Default)]
pub struct ChirpstackInventoryView {
    pub applications: Vec<InventoryApplication>,
    pub devices_by_app: HashMap<String, Vec<InventoryDevice>>,
    pub observed_by_device: HashMap<(String, String), Vec<ObservedKey>>,
}

impl OpcMetricTypeConfig {
    fn as_wire_str(&self) -> &'static str {
        match self {
            Self::Float => "Float",
            Self::Int => "Int",
            Self::Bool => "Bool",
            Self::String => "String",
        }
    }
}

fn opc_matches_wire(opcgw_type: &OpcMetricTypeConfig, observed: WireType) -> bool {
    matches!(
        (opcgw_type, observed),
        (OpcMetricTypeConfig::Float, WireType::Float)
            | (OpcMetricTypeConfig::Int, WireType::Int)
            | (OpcMetricTypeConfig::Bool, WireType::Bool)
            | (OpcMetricTypeConfig::String, WireType::String)
    )
}

/// Build the opcgw-side input view from the live `AppConfig` snapshot.
///
/// Kept as a thin adapter — the diff logic itself stays decoupled from
/// the config-crate types so tests can construct fixtures directly.
pub fn build_opcgw_view(config: &crate::config::AppConfig) -> Vec<OpcgwApplicationView> {
    config
        .application_list
        .iter()
        .map(|app| OpcgwApplicationView {
            application_id: app.application_id.clone(),
            application_name: app.application_name.clone(),
            devices: app
                .device_list
                .iter()
                .map(|dev| OpcgwDeviceView {
                    device_id: dev.device_id.clone(),
                    device_name: dev.device_name.clone(),
                    metrics: dev
                        .read_metric_list
                        .iter()
                        .map(|m| OpcgwMetricView {
                            chirpstack_metric_name: m.chirpstack_metric_name.clone(),
                            metric_name: m.metric_name.clone(),
                            metric_type: m.metric_type.clone(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Pure diff function (unit-testable, no ChirpStack dependency)
// ---------------------------------------------------------------------------

/// Compute the application/device/metric drift rows from two inventory
/// snapshots.
///
/// Output is deterministic: rows are sorted by `(application_id,
/// device_id, metric_name)` ascending so consecutive page-loads produce
/// the same JSON byte-for-byte (audit trails, snapshot tests).
pub fn compute_drift(
    opcgw: &[OpcgwApplicationView],
    chirpstack: &ChirpstackInventoryView,
) -> (
    Vec<ApplicationDriftRow>,
    Vec<DeviceDriftRow>,
    Vec<MetricDriftRow>,
    DriftSummary,
) {
    let opcgw_apps_by_id: BTreeMap<&str, &OpcgwApplicationView> = opcgw
        .iter()
        .map(|a| (a.application_id.as_str(), a))
        .collect();
    let cs_apps_by_id: BTreeMap<&str, &InventoryApplication> = chirpstack
        .applications
        .iter()
        .map(|a| (a.id.as_str(), a))
        .collect();

    let mut all_app_ids: BTreeSet<&str> = BTreeSet::new();
    all_app_ids.extend(opcgw_apps_by_id.keys().copied());
    all_app_ids.extend(cs_apps_by_id.keys().copied());

    let mut application_rows = Vec::with_capacity(all_app_ids.len());
    let mut device_rows = Vec::new();
    let mut metric_rows = Vec::new();
    let mut summary = DriftSummary::default();

    for app_id in &all_app_ids {
        let opcgw_app = opcgw_apps_by_id.get(app_id).copied();
        let cs_app = cs_apps_by_id.get(app_id).copied();

        let (class, drift_details) = match (opcgw_app, cs_app) {
            (Some(o), Some(c)) => {
                if o.application_name == c.name {
                    (DriftClass::Ok, None)
                } else {
                    (
                        DriftClass::Drifted,
                        Some(DriftDetails {
                            reason: REASON_NAME_DIFFERS,
                            opcgw_name: Some(o.application_name.clone()),
                            chirpstack_name: Some(c.name.clone()),
                            ..Default::default()
                        }),
                    )
                }
            }
            (Some(_), None) => (DriftClass::Stale, None),
            (None, Some(_)) => (DriftClass::Available, None),
            (None, None) => unreachable!("app_id came from union of both maps"),
        };

        application_rows.push(ApplicationDriftRow {
            class: class.as_str(),
            opcgw: opcgw_app.map(|o| ApplicationOpcgwView {
                application_id: o.application_id.clone(),
                application_name: o.application_name.clone(),
            }),
            chirpstack: cs_app.map(|c| ApplicationChirpstackView {
                id: c.id.clone(),
                name: c.name.clone(),
            }),
            drift_details,
        });
        increment_summary(&mut summary, class);

        // Device-level diff is computed only for app_ids present in
        // either set. For `available` (opcgw absent) apps we still
        // surface the ChirpStack-side devices as `available` rows so
        // the operator sees what they'd be adding.
        let opcgw_devs: Vec<&OpcgwDeviceView> =
            opcgw_app.map(|a| a.devices.iter().collect()).unwrap_or_default();
        let cs_devs: Vec<&InventoryDevice> = chirpstack
            .devices_by_app
            .get(*app_id)
            .map(|v| v.iter().collect())
            .unwrap_or_default();

        compute_device_rows(
            app_id,
            &opcgw_devs,
            &cs_devs,
            chirpstack,
            &mut device_rows,
            &mut metric_rows,
            &mut summary,
        );
    }

    (application_rows, device_rows, metric_rows, summary)
}

fn compute_device_rows(
    app_id: &str,
    opcgw_devs: &[&OpcgwDeviceView],
    cs_devs: &[&InventoryDevice],
    chirpstack: &ChirpstackInventoryView,
    device_rows: &mut Vec<DeviceDriftRow>,
    metric_rows: &mut Vec<MetricDriftRow>,
    summary: &mut DriftSummary,
) {
    let opcgw_by_id: BTreeMap<&str, &OpcgwDeviceView> = opcgw_devs
        .iter()
        .copied()
        .map(|d| (d.device_id.as_str(), d))
        .collect();
    let cs_by_id: BTreeMap<&str, &InventoryDevice> = cs_devs
        .iter()
        .copied()
        .map(|d| (d.dev_eui.as_str(), d))
        .collect();

    let mut all_dev_ids: BTreeSet<&str> = BTreeSet::new();
    all_dev_ids.extend(opcgw_by_id.keys().copied());
    all_dev_ids.extend(cs_by_id.keys().copied());

    for dev_id in &all_dev_ids {
        let opcgw_dev = opcgw_by_id.get(dev_id).copied();
        let cs_dev = cs_by_id.get(dev_id).copied();

        let (class, drift_details) = match (opcgw_dev, cs_dev) {
            (Some(o), Some(c)) => {
                if o.device_name == c.name {
                    (DriftClass::Ok, None)
                } else {
                    (
                        DriftClass::Drifted,
                        Some(DriftDetails {
                            reason: REASON_NAME_DIFFERS,
                            opcgw_name: Some(o.device_name.clone()),
                            chirpstack_name: Some(c.name.clone()),
                            ..Default::default()
                        }),
                    )
                }
            }
            (Some(_), None) => (DriftClass::Stale, None),
            (None, Some(_)) => (DriftClass::Available, None),
            (None, None) => unreachable!("dev_id came from union of both maps"),
        };

        device_rows.push(DeviceDriftRow {
            class: class.as_str(),
            application_id: app_id.to_string(),
            opcgw: opcgw_dev.map(|d| DeviceOpcgwView {
                device_id: d.device_id.clone(),
                device_name: d.device_name.clone(),
            }),
            chirpstack: cs_dev.map(|d| DeviceChirpstackView {
                dev_eui: d.dev_eui.clone(),
                name: d.name.clone(),
                last_seen_at: d.last_seen_at.clone(),
            }),
            drift_details,
        });
        increment_summary(summary, class);

        // Metric diff fires only when device is in BOTH sets (per AC#3).
        if let (Some(o), Some(_c)) = (opcgw_dev, cs_dev) {
            let observed = chirpstack
                .observed_by_device
                .get(&(app_id.to_string(), dev_id.to_string()))
                .map(|v| v.as_slice())
                .unwrap_or(&[]);
            compute_metric_rows(app_id, dev_id, o, observed, metric_rows, summary);
        }
    }
}

fn compute_metric_rows(
    app_id: &str,
    dev_id: &str,
    opcgw_dev: &OpcgwDeviceView,
    observed: &[ObservedKey],
    metric_rows: &mut Vec<MetricDriftRow>,
    summary: &mut DriftSummary,
) {
    let opcgw_by_key: BTreeMap<&str, &OpcgwMetricView> = opcgw_dev
        .metrics
        .iter()
        .map(|m| (m.chirpstack_metric_name.as_str(), m))
        .collect();
    let observed_by_key: BTreeMap<&str, &ObservedKey> =
        observed.iter().map(|o| (o.key.as_str(), o)).collect();

    let mut all_keys: BTreeSet<&str> = BTreeSet::new();
    all_keys.extend(opcgw_by_key.keys().copied());
    all_keys.extend(observed_by_key.keys().copied());

    for key in &all_keys {
        let opcgw_m = opcgw_by_key.get(key).copied();
        let obs_m = observed_by_key.get(key).copied();

        let (class, drift_details) = match (opcgw_m, obs_m) {
            (Some(o), Some(obs)) => {
                if opc_matches_wire(&o.metric_type, obs.wire_type) {
                    (DriftClass::Ok, None)
                } else {
                    (
                        DriftClass::Drifted,
                        Some(DriftDetails {
                            reason: REASON_WIRE_TYPE_MISMATCH,
                            opcgw_type: Some(o.metric_type.as_wire_str()),
                            inferred_type: Some(obs.wire_type.as_str()),
                            ..Default::default()
                        }),
                    )
                }
            }
            (Some(_), None) => (
                // AC#4 first edge case: configured but not seen in the
                // last 10 uplinks. Classified as `stale` BUT carries the
                // soft-reason so the UI can colour-code it differently
                // (codec may emit the key conditionally).
                DriftClass::Stale,
                Some(DriftDetails {
                    reason: REASON_NOT_IN_RECENT_UPLINKS,
                    ..Default::default()
                }),
            ),
            (None, Some(_)) => (DriftClass::Available, None),
            (None, None) => unreachable!("key came from union of both maps"),
        };

        metric_rows.push(MetricDriftRow {
            class: class.as_str(),
            application_id: app_id.to_string(),
            device_id: dev_id.to_string(),
            opcgw: opcgw_m.map(|m| MetricOpcgwView {
                chirpstack_metric_name: m.chirpstack_metric_name.clone(),
                metric_name: m.metric_name.clone(),
                metric_type: m.metric_type.as_wire_str(),
            }),
            chirpstack_observed: obs_m.map(|o| MetricChirpstackView {
                key: o.key.clone(),
                inferred_wire_type: o.wire_type.as_str(),
                sample_value: o.sample_value.clone(),
            }),
            drift_details,
        });
        increment_summary(summary, class);
    }
}

fn increment_summary(summary: &mut DriftSummary, class: DriftClass) {
    summary.total += 1;
    match class {
        DriftClass::Ok => summary.ok += 1,
        DriftClass::Stale => summary.stale += 1,
        DriftClass::Available => summary.available += 1,
        DriftClass::Drifted => summary.drifted += 1,
    }
}

// ---------------------------------------------------------------------------
// Degraded (ChirpStack-unreachable) response builder
// ---------------------------------------------------------------------------

/// Construct the degraded response surfaced when any C-1 underlying fetch
/// returns 502. Per AC#10: the opcgw-side rows are emitted as `class: "ok"`
/// placeholders (the operator's UI then disables destructive actions and
/// hides the `[Add to opcgw]` buttons since we can't know what's available).
fn build_degraded_response(
    opcgw: &[OpcgwApplicationView],
    fetched_at: String,
) -> DriftResponse {
    let mut applications = Vec::new();
    let mut devices = Vec::new();
    let mut metrics = Vec::new();
    let mut summary = DriftSummary::default();

    for app in opcgw {
        applications.push(ApplicationDriftRow {
            class: DriftClass::Ok.as_str(),
            opcgw: Some(ApplicationOpcgwView {
                application_id: app.application_id.clone(),
                application_name: app.application_name.clone(),
            }),
            chirpstack: None,
            drift_details: None,
        });
        summary.ok += 1;
        summary.total += 1;

        for dev in &app.devices {
            devices.push(DeviceDriftRow {
                class: DriftClass::Ok.as_str(),
                application_id: app.application_id.clone(),
                opcgw: Some(DeviceOpcgwView {
                    device_id: dev.device_id.clone(),
                    device_name: dev.device_name.clone(),
                }),
                chirpstack: None,
                drift_details: None,
            });
            summary.ok += 1;
            summary.total += 1;

            for m in &dev.metrics {
                metrics.push(MetricDriftRow {
                    class: DriftClass::Ok.as_str(),
                    application_id: app.application_id.clone(),
                    device_id: dev.device_id.clone(),
                    opcgw: Some(MetricOpcgwView {
                        chirpstack_metric_name: m.chirpstack_metric_name.clone(),
                        metric_name: m.metric_name.clone(),
                        metric_type: m.metric_type.as_wire_str(),
                    }),
                    chirpstack_observed: None,
                    drift_details: None,
                });
                summary.ok += 1;
                summary.total += 1;
            }
        }
    }

    DriftResponse {
        applications,
        devices,
        metrics,
        fetched_at,
        chirpstack_reachable: false,
        summary,
    }
}

// ---------------------------------------------------------------------------
// Handler
// ---------------------------------------------------------------------------

/// `GET /api/inventory/drift` — Story C-4 AC#2.
///
/// Forces `?refresh=true` on every underlying C-1 fetch so the comparison
/// is never against a stale cache (per AC#9 + Dev Notes). Read-only. On
/// any ChirpStack failure, returns the degraded response (still HTTP 200
/// with `chirpstack_reachable: false`) so the UI can surface its banner
/// without an extra parse-error path.
pub async fn inventory_drift(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Response {
    let started = std::time::Instant::now();
    let config = state.config_reload.subscribe().borrow().clone();
    let tenant_id = config.chirpstack.tenant_id.clone();
    let opcgw_view = build_opcgw_view(&config);
    let fetched_at = chrono::Utc::now().to_rfc3339();

    // ---------------- ChirpStack applications fetch (refresh=true) ----------------
    let cancel_token = state.shutdown_token.clone();
    let cfg_apps = config.clone();
    let apps_result = state
        .inventory_cache
        .get_or_fetch_applications(&tenant_id, /* force_refresh */ true, || async move {
            let raw = fetch_applications(&cfg_apps, &cancel_token).await?;
            let mut items: Vec<InventoryApplication> = raw.into_iter().map(Into::into).collect();
            items.sort_by_key(|a| a.name.to_lowercase());
            Ok(items)
        })
        .await;

    let cs_applications = match apps_result {
        Ok(cache_result) => cache_result.value,
        Err(e) => {
            warn!(
                event = "inventory_drift_unreachable",
                stage = "applications",
                tenant_id = %tenant_id,
                error = ?e.to_string(),
                duration_ms = started.elapsed().as_millis() as u64,
                "GET /api/inventory/drift: ChirpStack applications fetch failed"
            );
            let response = build_degraded_response(&opcgw_view, fetched_at);
            emit_drift_view_opened(&addr, &response);
            return (StatusCode::OK, Json(response)).into_response();
        }
    };

    // ---------------- ChirpStack devices fetch per app union ----------------
    // Union of opcgw + ChirpStack application ids — we need device lists
    // for every app on EITHER side so opcgw-stale apps still surface their
    // (now-stranded) devices and ChirpStack-available apps surface their
    // discoverable devices.
    let mut all_app_ids: BTreeSet<String> = BTreeSet::new();
    for app in &opcgw_view {
        all_app_ids.insert(app.application_id.clone());
    }
    for app in &cs_applications {
        all_app_ids.insert(app.id.clone());
    }

    let mut devices_by_app: HashMap<String, Vec<InventoryDevice>> = HashMap::new();
    for app_id in &all_app_ids {
        let cancel_token = state.shutdown_token.clone();
        let cfg_devs = config.clone();
        let app_id_for_fetch = app_id.clone();
        let devs_result = state
            .inventory_cache
            .get_or_fetch_devices(&tenant_id, app_id, true, || async move {
                let raw = fetch_devices(&cfg_devs, &app_id_for_fetch, &cancel_token).await?;
                let mut items: Vec<InventoryDevice> = raw.into_iter().map(Into::into).collect();
                items.sort_by_key(|a| a.name.to_lowercase());
                Ok(items)
            })
            .await;
        match devs_result {
            Ok(cache_result) => {
                devices_by_app.insert(app_id.clone(), cache_result.value);
            }
            Err(e) => {
                warn!(
                    event = "inventory_drift_unreachable",
                    stage = "devices",
                    tenant_id = %tenant_id,
                    application_id = %app_id,
                    error = ?e.to_string(),
                    duration_ms = started.elapsed().as_millis() as u64,
                    "GET /api/inventory/drift: ChirpStack devices fetch failed"
                );
                let response = build_degraded_response(&opcgw_view, fetched_at);
                emit_drift_view_opened(&addr, &response);
                return (StatusCode::OK, Json(response)).into_response();
            }
        }
    }

    // ---------------- Uplinks fetch per (app, device) in BOTH sets ----------------
    // Per AC#3 metric-diff scope: only devices present in BOTH opcgw and
    // ChirpStack get observed-key fetches. Devices stale-only or
    // available-only have no metric drift to compute.
    let max_wait = Duration::from_secs(config.chirpstack.inventory_uplink_max_wait_seconds);
    let mut observed_by_device: HashMap<(String, String), Vec<ObservedKey>> = HashMap::new();
    for app in &opcgw_view {
        let Some(cs_devs) = devices_by_app.get(&app.application_id) else {
            continue;
        };
        let cs_dev_ids: BTreeSet<&str> = cs_devs.iter().map(|d| d.dev_eui.as_str()).collect();
        for opcgw_dev in &app.devices {
            if !cs_dev_ids.contains(opcgw_dev.device_id.as_str()) {
                continue;
            }
            let uplinks_result = stream_recent_device_uplinks(
                &config.chirpstack.server_address,
                &config.chirpstack.api_token,
                &opcgw_dev.device_id,
                UPLINKS_LIMIT_DEFAULT,
                max_wait,
            )
            .await;
            match uplinks_result {
                Ok(uplinks) => {
                    let (observed, _heterogeneous) = compute_observed_keys(&uplinks);
                    observed_by_device
                        .insert((app.application_id.clone(), opcgw_dev.device_id.clone()), observed);
                }
                Err(e) => {
                    warn!(
                        event = "inventory_drift_unreachable",
                        stage = "uplinks",
                        tenant_id = %tenant_id,
                        application_id = %app.application_id,
                        dev_eui = %opcgw_dev.device_id,
                        error = ?e.to_string(),
                        duration_ms = started.elapsed().as_millis() as u64,
                        "GET /api/inventory/drift: ChirpStack uplinks fetch failed"
                    );
                    let response = build_degraded_response(&opcgw_view, fetched_at);
                    emit_drift_view_opened(&addr, &response);
                    return (StatusCode::OK, Json(response)).into_response();
                }
            }
        }
    }

    // ---------------- Pure diff + response assembly ----------------
    let cs_view = ChirpstackInventoryView {
        applications: cs_applications,
        devices_by_app,
        observed_by_device,
    };
    let (applications, devices, metrics, summary) = compute_drift(&opcgw_view, &cs_view);
    let response = DriftResponse {
        applications,
        devices,
        metrics,
        fetched_at,
        chirpstack_reachable: true,
        summary,
    };
    emit_drift_view_opened(&addr, &response);
    info!(
        event = "inventory_drift_succeeded",
        tenant_id = %tenant_id,
        application_count = response.applications.len(),
        device_count = response.devices.len(),
        metric_count = response.metrics.len(),
        duration_ms = started.elapsed().as_millis() as u64,
        "GET /api/inventory/drift: completed"
    );
    (StatusCode::OK, Json(response)).into_response()
}

fn emit_drift_view_opened(addr: &SocketAddr, response: &DriftResponse) {
    info!(
        event = "drift_view_opened",
        source = "web_drift",
        source_ip = %addr.ip(),
        chirpstack_reachable = response.chirpstack_reachable,
        summary_ok = response.summary.ok,
        summary_stale = response.summary.stale,
        summary_available = response.summary.available,
        summary_drifted = response.summary.drifted,
        summary_total = response.summary.total,
        "Operator opened the inventory drift view"
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn make_opcgw_app(id: &str, name: &str, devices: Vec<OpcgwDeviceView>) -> OpcgwApplicationView {
        OpcgwApplicationView {
            application_id: id.to_string(),
            application_name: name.to_string(),
            devices,
        }
    }

    fn make_opcgw_device(id: &str, name: &str, metrics: Vec<OpcgwMetricView>) -> OpcgwDeviceView {
        OpcgwDeviceView {
            device_id: id.to_string(),
            device_name: name.to_string(),
            metrics,
        }
    }

    fn make_opcgw_metric(
        cs_name: &str,
        name: &str,
        metric_type: OpcMetricTypeConfig,
    ) -> OpcgwMetricView {
        OpcgwMetricView {
            chirpstack_metric_name: cs_name.to_string(),
            metric_name: name.to_string(),
            metric_type,
        }
    }

    fn cs_app(id: &str, name: &str) -> InventoryApplication {
        InventoryApplication {
            id: id.to_string(),
            name: name.to_string(),
            description: String::new(),
        }
    }

    fn cs_dev(dev_eui: &str, name: &str) -> InventoryDevice {
        InventoryDevice {
            dev_eui: dev_eui.to_string(),
            name: name.to_string(),
            description: String::new(),
            device_profile_name: None,
            last_seen_at: None,
        }
    }

    fn observed(key: &str, wire_type: WireType) -> ObservedKey {
        ObservedKey {
            key: key.to_string(),
            wire_type,
            sample_value: json!(0),
        }
    }

    #[test]
    fn application_diff_ok_when_id_and_name_match() {
        let opcgw = vec![make_opcgw_app("app-1", "Sensors", vec![])];
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-1", "Sensors")],
            ..Default::default()
        };
        let (apps, _, _, summary) = compute_drift(&opcgw, &cs);
        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].class, "ok");
        assert!(apps[0].drift_details.is_none());
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.total, 1);
    }

    #[test]
    fn application_diff_drifted_when_names_differ() {
        let opcgw = vec![make_opcgw_app("app-1", "OldName", vec![])];
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-1", "NewName")],
            ..Default::default()
        };
        let (apps, _, _, summary) = compute_drift(&opcgw, &cs);
        assert_eq!(apps[0].class, "drifted");
        let d = apps[0].drift_details.as_ref().unwrap();
        assert_eq!(d.reason, REASON_NAME_DIFFERS);
        assert_eq!(d.opcgw_name.as_deref(), Some("OldName"));
        assert_eq!(d.chirpstack_name.as_deref(), Some("NewName"));
        assert_eq!(summary.drifted, 1);
    }

    #[test]
    fn application_diff_stale_when_only_in_opcgw() {
        let opcgw = vec![make_opcgw_app("app-gone", "Retired", vec![])];
        let cs = ChirpstackInventoryView::default();
        let (apps, _, _, summary) = compute_drift(&opcgw, &cs);
        assert_eq!(apps[0].class, "stale");
        assert!(apps[0].opcgw.is_some());
        assert!(apps[0].chirpstack.is_none());
        assert_eq!(summary.stale, 1);
    }

    #[test]
    fn application_diff_available_when_only_in_chirpstack() {
        let opcgw = vec![];
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-new", "Freshly enrolled")],
            ..Default::default()
        };
        let (apps, _, _, summary) = compute_drift(&opcgw, &cs);
        assert_eq!(apps[0].class, "available");
        assert!(apps[0].opcgw.is_none());
        assert!(apps[0].chirpstack.is_some());
        assert_eq!(summary.available, 1);
    }

    #[test]
    fn device_diff_drifted_on_name_only() {
        let opcgw = vec![make_opcgw_app(
            "app-1",
            "Sensors",
            vec![make_opcgw_device("eui-1", "OldDev", vec![])],
        )];
        let mut devices_by_app = HashMap::new();
        devices_by_app.insert("app-1".into(), vec![cs_dev("eui-1", "NewDev")]);
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-1", "Sensors")],
            devices_by_app,
            observed_by_device: HashMap::new(),
        };
        let (_, devs, _, _) = compute_drift(&opcgw, &cs);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].class, "drifted");
        let d = devs[0].drift_details.as_ref().unwrap();
        assert_eq!(d.reason, REASON_NAME_DIFFERS);
    }

    #[test]
    fn metric_diff_ok_when_wire_type_matches() {
        let opcgw = vec![make_opcgw_app(
            "app-1",
            "Sensors",
            vec![make_opcgw_device(
                "eui-1",
                "Dev",
                vec![make_opcgw_metric(
                    "temperature",
                    "Temperature",
                    OpcMetricTypeConfig::Float,
                )],
            )],
        )];
        let mut devices_by_app = HashMap::new();
        devices_by_app.insert("app-1".into(), vec![cs_dev("eui-1", "Dev")]);
        let mut observed_by_device = HashMap::new();
        observed_by_device.insert(
            ("app-1".into(), "eui-1".into()),
            vec![observed("temperature", WireType::Float)],
        );
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-1", "Sensors")],
            devices_by_app,
            observed_by_device,
        };
        let (_, _, metrics, _) = compute_drift(&opcgw, &cs);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].class, "ok");
    }

    #[test]
    fn metric_diff_drifted_on_wire_type_mismatch() {
        let opcgw = vec![make_opcgw_app(
            "app-1",
            "Sensors",
            vec![make_opcgw_device(
                "eui-1",
                "Dev",
                vec![make_opcgw_metric(
                    "counter",
                    "Counter",
                    OpcMetricTypeConfig::Int,
                )],
            )],
        )];
        let mut devices_by_app = HashMap::new();
        devices_by_app.insert("app-1".into(), vec![cs_dev("eui-1", "Dev")]);
        let mut observed_by_device = HashMap::new();
        observed_by_device.insert(
            ("app-1".into(), "eui-1".into()),
            vec![observed("counter", WireType::Float)],
        );
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-1", "Sensors")],
            devices_by_app,
            observed_by_device,
        };
        let (_, _, metrics, _) = compute_drift(&opcgw, &cs);
        assert_eq!(metrics[0].class, "drifted");
        let d = metrics[0].drift_details.as_ref().unwrap();
        assert_eq!(d.reason, REASON_WIRE_TYPE_MISMATCH);
        assert_eq!(d.opcgw_type, Some("Int"));
        assert_eq!(d.inferred_type, Some("Float"));
    }

    #[test]
    fn metric_diff_soft_stale_on_not_in_recent_uplinks() {
        let opcgw = vec![make_opcgw_app(
            "app-1",
            "Sensors",
            vec![make_opcgw_device(
                "eui-1",
                "Dev",
                vec![make_opcgw_metric(
                    "battery",
                    "Battery",
                    OpcMetricTypeConfig::Float,
                )],
            )],
        )];
        let mut devices_by_app = HashMap::new();
        devices_by_app.insert("app-1".into(), vec![cs_dev("eui-1", "Dev")]);
        // Device is in both sets, but observed_by_device is empty (no uplinks).
        let mut observed_by_device = HashMap::new();
        observed_by_device.insert(("app-1".into(), "eui-1".into()), vec![]);
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-1", "Sensors")],
            devices_by_app,
            observed_by_device,
        };
        let (_, _, metrics, _) = compute_drift(&opcgw, &cs);
        assert_eq!(metrics[0].class, "stale");
        let d = metrics[0].drift_details.as_ref().unwrap();
        assert_eq!(d.reason, REASON_NOT_IN_RECENT_UPLINKS);
    }

    #[test]
    fn metric_diff_available_when_uplink_key_not_configured() {
        let opcgw = vec![make_opcgw_app(
            "app-1",
            "Sensors",
            vec![make_opcgw_device("eui-1", "Dev", vec![])],
        )];
        let mut devices_by_app = HashMap::new();
        devices_by_app.insert("app-1".into(), vec![cs_dev("eui-1", "Dev")]);
        let mut observed_by_device = HashMap::new();
        observed_by_device.insert(
            ("app-1".into(), "eui-1".into()),
            vec![observed("temperature", WireType::Float)],
        );
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-1", "Sensors")],
            devices_by_app,
            observed_by_device,
        };
        let (_, _, metrics, _) = compute_drift(&opcgw, &cs);
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].class, "available");
    }

    #[test]
    fn metric_diff_skipped_for_devices_not_in_both_sets() {
        // Device is opcgw-only (stale) — no metric rows should be emitted
        // for it even if it has configured metrics, because the spec
        // restricts metric diff to (app, device) pairs present in BOTH.
        let opcgw = vec![make_opcgw_app(
            "app-1",
            "Sensors",
            vec![make_opcgw_device(
                "eui-stale",
                "Stale dev",
                vec![make_opcgw_metric(
                    "x",
                    "X",
                    OpcMetricTypeConfig::Float,
                )],
            )],
        )];
        let mut devices_by_app = HashMap::new();
        devices_by_app.insert("app-1".into(), vec![]);
        let cs = ChirpstackInventoryView {
            applications: vec![cs_app("app-1", "Sensors")],
            devices_by_app,
            observed_by_device: HashMap::new(),
        };
        let (_, devs, metrics, _) = compute_drift(&opcgw, &cs);
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].class, "stale");
        assert!(metrics.is_empty(), "no metric rows for opcgw-only devices");
    }

    #[test]
    fn degraded_response_marks_everything_ok_with_chirpstack_reachable_false() {
        let opcgw = vec![make_opcgw_app(
            "app-1",
            "Sensors",
            vec![make_opcgw_device(
                "eui-1",
                "Dev",
                vec![make_opcgw_metric(
                    "x",
                    "X",
                    OpcMetricTypeConfig::Float,
                )],
            )],
        )];
        let response = build_degraded_response(&opcgw, "2026-05-24T12:00:00Z".to_string());
        assert!(!response.chirpstack_reachable);
        assert_eq!(response.applications.len(), 1);
        assert_eq!(response.applications[0].class, "ok");
        assert!(response.applications[0].chirpstack.is_none());
        assert_eq!(response.devices.len(), 1);
        assert_eq!(response.metrics.len(), 1);
        assert_eq!(response.summary.ok, 3);
        assert_eq!(response.summary.total, 3);
    }

    #[test]
    fn summary_counts_sum_to_total() {
        let opcgw = vec![
            make_opcgw_app("app-ok", "OK", vec![]),
            make_opcgw_app("app-stale", "Stale", vec![]),
            make_opcgw_app("app-drifted", "OldName", vec![]),
        ];
        let cs = ChirpstackInventoryView {
            applications: vec![
                cs_app("app-ok", "OK"),
                cs_app("app-drifted", "NewName"),
                cs_app("app-available", "Available"),
            ],
            ..Default::default()
        };
        let (apps, _, _, summary) = compute_drift(&opcgw, &cs);
        assert_eq!(apps.len(), 4);
        assert_eq!(summary.ok, 1);
        assert_eq!(summary.stale, 1);
        assert_eq!(summary.drifted, 1);
        assert_eq!(summary.available, 1);
        assert_eq!(summary.total, 4);
        assert_eq!(
            summary.ok + summary.stale + summary.drifted + summary.available,
            summary.total
        );
    }

    #[test]
    fn rows_are_sorted_by_id_for_deterministic_output() {
        let opcgw = vec![
            make_opcgw_app("zeta", "Z", vec![]),
            make_opcgw_app("alpha", "A", vec![]),
            make_opcgw_app("mu", "M", vec![]),
        ];
        let cs = ChirpstackInventoryView {
            applications: vec![
                cs_app("alpha", "A"),
                cs_app("mu", "M"),
                cs_app("zeta", "Z"),
            ],
            ..Default::default()
        };
        let (apps, _, _, _) = compute_drift(&opcgw, &cs);
        let ids: Vec<&str> = apps
            .iter()
            .map(|r| r.opcgw.as_ref().unwrap().application_id.as_str())
            .collect();
        assert_eq!(ids, vec!["alpha", "mu", "zeta"]);
    }
}

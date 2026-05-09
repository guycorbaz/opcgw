// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Read-only JSON API for the embedded Axum web server (Story 9-2+).
//!
//! Hosts the dashboard's data endpoints. Story 9-2 ships the single
//! `GET /api/status` route; Stories 9-3 / 9-4 / 9-5 / 9-6 will extend
//! this module with `/api/devices`, `/api/applications`, `/api/commands`
//! etc. (CRUD lands later — 9-2 is read-only).
//!
//! All routes inherit the Story 9-1 `basic_auth_middleware` automatically
//! via the layer-after-route invariant in [`crate::web::build_router`];
//! handlers do **not** re-check authentication.
//!
//! # Error contract
//!
//! Storage failures map to `500 Internal Server Error` with a generic
//! body (`{"error":"internal server error"}`). The inner error goes to
//! the operator log via the `event="api_status_storage_error"` warn
//! event — never to the client. NFR7 invariant: error messages must
//! not leak SQLite paths, table names, or other internal state.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::config::OpcMetricTypeConfig;
use crate::config_reload::{ReloadError, ReloadOutcome};
use crate::utils::OpcGwError;
use crate::web::AppState;

/// Shape returned by `GET /api/status` on success (Story 9-2 AC#2).
///
/// Field naming uses snake_case for consistency with the OPC UA address
/// space and to keep the JSON contract operator-friendly under
/// `curl | jq`. `last_poll_time` is serialised as RFC 3339 string or
/// JSON `null` (never a placeholder timestamp); the dashboard
/// distinguishes "never polled" from "polled but stale" client-side.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct StatusResponse {
    pub chirpstack_available: bool,
    /// `None` → JSON `null`. `Some(t)` → RFC 3339 string.
    pub last_poll_time: Option<String>,
    pub error_count: i32,
    pub application_count: usize,
    pub device_count: usize,
    pub uptime_secs: u64,
}

/// Generic error body. The `error` field is intentionally a fixed
/// string — never `e.to_string()` from the inner `OpcGwError` (NFR7).
///
/// Story 9-4: optional `hint` field surfaces operator-action text
/// for CRUD failures (e.g. "remove devices first via /api/devices").
/// Skipped from the wire JSON when `None` so the existing
/// 9-2 / 9-3 callers see no change.
#[derive(Debug, Serialize, PartialEq, Eq, Default)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl ErrorResponse {
    pub(crate) fn internal_server_error() -> Self {
        Self {
            error: "internal server error".to_string(),
            hint: None,
        }
    }

    /// Story 9-4 helper: build an ErrorResponse with both message
    /// and operator-action hint set.
    pub(crate) fn with_hint(error: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            hint: Some(hint.into()),
        }
    }

    /// Story 9-4 helper: error-only ErrorResponse (no hint).
    pub(crate) fn from_error(error: impl Into<String>) -> Self {
        Self {
            error: error.into(),
            hint: None,
        }
    }
}

/// `GET /api/status` handler.
///
/// Reads the gateway-health triple from the per-task SQLite backend +
/// the frozen `DashboardConfigSnapshot` from `AppState`, plus computes
/// `uptime_secs` from the captured `start_time`. Returns 200 + JSON on
/// success or 500 + generic JSON on storage failure.
///
/// # Why `uptime_secs` (not `start_time`)
///
/// Returning `start_time` would tempt the dashboard to compute uptime
/// as `Date.now() - start_time`, which silently breaks if the server's
/// clock and the browser's clock disagree. The server returning
/// `uptime_secs` keeps the wall-clock-skew failure mode out of the
/// dashboard.
pub async fn api_status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<StatusResponse>, Response> {
    let (last_poll, error_count, available) =
        match state.backend.get_gateway_health_metrics() {
            Ok(triple) => triple,
            Err(e) => {
                // NFR7: log the full error to the operator log; return
                // a generic body to the client.
                warn!(
                    event = "api_status_storage_error",
                    error = %e,
                    "GET /api/status: failed to read gateway_status table"
                );
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse::internal_server_error()),
                )
                    .into_response());
            }
        };

    let uptime_secs = state.start_time.elapsed().as_secs();

    // Story 9-7: read the snapshot through the RwLock so a hot-reload
    // swap is visible immediately. The clone of the inner Arc is
    // O(1) and the lock is released before any subsequent work.
    //
    // Iter-1 review P5: recover from poison via `into_inner()` so a
    // single panic in another holder doesn't cascade — every
    // subsequent request would otherwise also panic, taking the
    // whole web subsystem down.
    //
    // Iter-2 review P34: surface poison-recovery to operator audit
    // log. A poisoned lock means SOMETHING panicked while holding
    // it — the operator should be able to correlate the original
    // panic with subsequent dashboard behaviour. Logged on each
    // recovery (rare event in practice; per-site spam acceptable).
    let snapshot = state
        .dashboard_snapshot
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|e| {
            tracing::warn!(
                operation = "rwlock_poison_recovered",
                site = "api_status",
                "dashboard_snapshot RwLock was poisoned; recovering inner value \
                 (a prior holder panicked — investigate)"
            );
            e.into_inner().clone()
        });

    Ok(Json(StatusResponse {
        chirpstack_available: available,
        last_poll_time: last_poll.map(|t| t.to_rfc3339()),
        error_count,
        application_count: snapshot.application_count,
        device_count: snapshot.device_count,
        uptime_secs,
    }))
}

/// Story 5-2 staleness boundary between "Good" and "Uncertain"
/// (default). Mirrors the private `DEFAULT_STALE_THRESHOLD_SECS` in
/// `src/opc_ua.rs:38`; exposed here as `pub` so `src/main.rs` can
/// resolve `[opcua].stale_threshold_seconds.unwrap_or(...)` without
/// reaching into the OPC UA module's internals.
pub const DEFAULT_STALE_THRESHOLD_SECS: u64 = 120;

/// Story 5-2 hard cutoff between "Uncertain" and "Bad" — server-owned
/// constant today (operator can't tune it). Mirror of
/// `STATUS_CODE_BAD_THRESHOLD_SECS` in `src/opc_ua.rs:39`. The
/// dashboard receives both thresholds as JSON fields so the JS
/// branching logic doesn't hard-code any boundary; future Story can
/// promote this to a config knob without touching the wire contract.
///
/// **Story 9-3 review iter-2 L6:** promoted to `pub const` so
/// `src/main.rs` can reference the same value when clamping
/// `[opcua].stale_threshold_seconds`. Single source of truth — if
/// future story bumps the cutoff (or makes it configurable), only
/// this site needs to change and main.rs stays in sync automatically.
pub const BAD_THRESHOLD_SECS: u64 = 86_400;

/// Map a configured `OpcMetricTypeConfig` to its display string,
/// matching the `MetricType::Display` impl from `src/storage/types.rs`
/// so the JSON `data_type` field is identical whether sourced from the
/// configured type or the storage row's type.
fn config_type_to_display(t: &OpcMetricTypeConfig) -> &'static str {
    match t {
        OpcMetricTypeConfig::Bool => "Bool",
        OpcMetricTypeConfig::Int => "Int",
        OpcMetricTypeConfig::Float => "Float",
        OpcMetricTypeConfig::String => "String",
    }
}

/// Shape returned by `GET /api/devices` on success (Story 9-3 AC#2).
///
/// Server-side `as_of` lets every browser compute the same `age_secs`
/// regardless of local clock skew (same rationale as Story 9-2's
/// `uptime_secs` — pin the time-of-truth on the server).
///
/// Both `stale_threshold_secs` and `bad_threshold_secs` are returned so
/// the JS branching logic doesn't need to hard-code either boundary.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DevicesResponse {
    pub as_of: String,
    pub stale_threshold_secs: u64,
    pub bad_threshold_secs: u64,
    pub applications: Vec<ApplicationView>,
}

/// Per-application section in the live-metrics grid.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ApplicationView {
    pub application_id: String,
    pub application_name: String,
    pub devices: Vec<DeviceView>,
}

/// Per-device section — identifies the device + lists its configured
/// metrics in TOML-declaration order.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DeviceView {
    pub device_id: String,
    pub device_name: String,
    pub metrics: Vec<MetricView>,
}

/// Per-metric row. `value` and `timestamp` are `null` when the metric
/// is configured but has no row in `metric_values` (operator sees
/// "missing" status badge). `data_type` always carries a string —
/// from the storage row when present, otherwise from the configured
/// type so the dashboard can display "(Int) — never reported" rather
/// than "(?) — never reported".
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MetricView {
    pub metric_name: String,
    pub data_type: String,
    pub value: Option<String>,
    pub timestamp: Option<String>,
}

/// `GET /api/devices` handler (Story 9-3, FR37).
///
/// Reads every row from the `metric_values` SQLite table via the
/// per-task backend, joins against the frozen `DashboardConfigSnapshot`,
/// and emits an application-grouped JSON view of the live metric
/// values. A configured-but-not-yet-polled metric appears as a row
/// with `value: null, timestamp: null` rather than being omitted —
/// the operator needs to see "this metric exists but hasn't been
/// reported yet" as a distinct state from "this metric isn't
/// configured at all" (for which the metric row simply doesn't
/// appear in the configured `metrics: Vec<MetricSpec>`).
///
/// # Why server-side `as_of`
///
/// The dashboard could compute `(Date.now() - timestamp)` browser-side,
/// but two browsers viewing the same gateway would disagree if their
/// clocks differed. Returning the server's `Utc::now()` as `as_of` lets
/// every browser compute the same `age_secs` regardless of local clock
/// skew. Same rationale as Story 9-2's `uptime_secs` field.
pub async fn api_devices(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevicesResponse>, Response> {
    // Capture the server timestamp at request entry — NOT after the
    // storage call returns. The dashboard uses this as the denominator
    // for "age vs threshold" so it must reflect the moment the
    // operator's request hit the server, not after the storage delay.
    let as_of = Utc::now().to_rfc3339();

    let metrics = match state.backend.load_all_metrics() {
        Ok(rows) => rows,
        Err(e) => {
            // NFR7: log the full error to the operator log; return a
            // generic body to the client.
            warn!(
                event = "api_devices_storage_error",
                error = %e,
                "GET /api/devices: failed to read metric_values table"
            );
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse::internal_server_error()),
            )
                .into_response());
        }
    };

    // O(N) lookup keyed by (device_id, metric_name) so the per-snapshot
    // walk is O(devices × metrics) total — same complexity class as
    // a naïve nested scan but dramatically faster at realistic device
    // counts (100 devices × 4 metrics × ~3 µs/lookup = ~1.2 ms vs.
    // ~40 ms for the nested scan).
    let mut metric_by_key: HashMap<(String, String), &crate::storage::MetricValue> =
        HashMap::with_capacity(metrics.len());
    for row in &metrics {
        metric_by_key.insert((row.device_id.clone(), row.metric_name.clone()), row);
    }

    // `stale_threshold_secs` is resolved at AppState construction
    // (Story 9-3 addition to AppState) — `DEFAULT_STALE_THRESHOLD_SECS`
    // is referenced here as the documented default for the JSON
    // contract docstring above; the actual value plumbed through is
    // the resolved one from main.rs.
    //
    // Story 9-7: `AtomicU64::load(Relaxed)` so a hot-reload that
    // swaps the threshold is visible to subsequent requests
    // immediately. Relaxed is sufficient — the field is
    // monotonically updated by a single listener task; no
    // happens-before relation needs to be observed across threads.
    let stale_threshold_secs = state
        .stale_threshold_secs
        .load(std::sync::atomic::Ordering::Relaxed);

    // Story 9-7: same RwLock-clone pattern as `api_status` above
    // (iter-1 review P5: poison-recovery via `into_inner`;
    // iter-2 review P34: emit poison-recovery audit log).
    let snapshot = state
        .dashboard_snapshot
        .read()
        .map(|g| g.clone())
        .unwrap_or_else(|e| {
            tracing::warn!(
                operation = "rwlock_poison_recovered",
                site = "api_devices",
                "dashboard_snapshot RwLock was poisoned; recovering inner value \
                 (a prior holder panicked — investigate)"
            );
            e.into_inner().clone()
        });

    let applications: Vec<ApplicationView> = snapshot
        .applications
        .iter()
        .map(|app| {
            let devices: Vec<DeviceView> = app
                .devices
                .iter()
                .map(|dev| {
                    // Story 9-3 review iter-1 H1 fix: walk a single
                    // `Vec<MetricSpec>` instead of zipping two parallel
                    // `Vec`s — the type system now guarantees the
                    // metric_name and metric_type stay paired.
                    let metrics: Vec<MetricView> = dev
                        .metrics
                        .iter()
                        .map(|spec| {
                            let key = (dev.device_id.clone(), spec.metric_name.clone());
                            match metric_by_key.get(&key) {
                                Some(row) => MetricView {
                                    metric_name: spec.metric_name.clone(),
                                    data_type: row.data_type.to_string(),
                                    value: Some(row.value.clone()),
                                    timestamp: Some(row.timestamp.to_rfc3339()),
                                },
                                None => MetricView {
                                    metric_name: spec.metric_name.clone(),
                                    data_type: config_type_to_display(&spec.metric_type)
                                        .to_string(),
                                    value: None,
                                    timestamp: None,
                                },
                            }
                        })
                        .collect();
                    DeviceView {
                        device_id: dev.device_id.clone(),
                        device_name: dev.device_name.clone(),
                        metrics,
                    }
                })
                .collect();
            ApplicationView {
                application_id: app.application_id.clone(),
                application_name: app.application_name.clone(),
                devices,
            }
        })
        .collect();

    Ok(Json(DevicesResponse {
        as_of,
        stale_threshold_secs,
        bad_threshold_secs: BAD_THRESHOLD_SECS,
        applications,
    }))
}

// ----------------------------------------------------------------------
// Story 9-4: Application CRUD endpoints (FR34, FR40, AC#1-#7).
//
// Five routes, all behind Basic auth (Story 9-1 layer-after-route
// invariant) and the new CSRF middleware (Story 9-4 Task 3) for
// state-changing methods. Read paths consume the auto-refreshed
// `dashboard_snapshot` (Story 9-2/9-7); write paths take the
// `ConfigWriter::lock()` across the entire `write+reload+(rollback)`
// critical section so concurrent CRUD requests cannot lose updates.
// ----------------------------------------------------------------------

/// Per-application entry returned by `GET /api/applications`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ApplicationListEntry {
    pub application_id: String,
    pub application_name: String,
    pub device_count: usize,
}

/// Body returned by `GET /api/applications`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ApplicationListResponse {
    pub applications: Vec<ApplicationListEntry>,
}

/// Body returned by `GET|POST|PUT /api/applications/:application_id`.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ApplicationResponse {
    pub application_id: String,
    pub application_name: String,
    pub device_count: usize,
}

/// `POST /api/applications` request body.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateApplicationRequest {
    pub application_id: String,
    pub application_name: String,
}

/// `PUT /api/applications/:application_id` request body.
///
/// **Iter-2 review P29 (load-bearing):** the iter-1 P5 patch
/// dropped `serde(deny_unknown_fields)` to enable the
/// `immutable_field` audit event from inside the handler — but it
/// did NOT replace it with a custom rejection of OTHER unknown
/// fields. A body like `{"application_name":"x","random":true}`
/// would deserialise cleanly and silently drop the unknown field.
/// Iter-2 fixes this by parsing the body into `serde_json::Value`
/// in the handler, walking the object explicitly, and emitting an
/// audit event for both `application_id` (immutable) and any other
/// unknown field. **The struct is no longer used as a JSON
/// extractor target** — the handler does manual deserialisation.
/// Kept here for documentation of the v1 contract.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct UpdateApplicationRequest {
    pub application_name: String,
    /// Iter-1 P5 / Iter-2 P29: `application_id` is rejected by
    /// the handler with `reason="immutable_field"`; any OTHER
    /// unknown field is rejected with `reason="unknown_field"`.
    #[serde(default)]
    pub application_id: Option<String>,
}

const APP_FIELD_MAX_LEN: usize = 256;

/// Iter-1 review P1: allowed character class for `application_id`
/// and `application_name`. Restricts to ASCII alphanumerics plus a
/// small set of safe separators; refuses CRLF, path-traversal
/// segments, and characters that break TOML/HTML/JS escape contracts.
///
/// **Iter-2 review P25 (CRITICAL):** path-supplied `application_id`
/// values from `Path<String>` extractors are URL-decoded by axum
/// BEFORE validation. A `DELETE /api/applications/foo%0A%20event=`
/// would otherwise produce `application_id = "foo\nevent="` which —
/// when interpolated into `tracing::warn!(application_id = %id)` —
/// forges a synthetic audit log line. **Every handler that takes a
/// `Path(application_id): Path<String>` MUST call
/// [`validate_path_application_id`] before any logging or further
/// processing.**
///
/// Note: case-sensitive. `App-1` and `app-1` are distinct
/// identifiers (Iter-2 review P37 — documented design call). If a
/// future deployment needs case-insensitive matching, both the dup
/// check and `AppConfig::validate` HashSet must change in lockstep.
fn is_valid_app_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')
}

/// Iter-2 review P25: validate a path-supplied `application_id`
/// against the same character class as body-supplied IDs. Returns
/// 400 + `event="application_crud_rejected" reason="validation"`
/// audit event on failure. Call this at the head of EVERY handler
/// that takes a `Path(application_id)` parameter, BEFORE any code
/// that interpolates the value into a tracing field or constructs a
/// `Location` / response header from it.
#[allow(clippy::result_large_err)]
fn validate_path_application_id(
    application_id: &str,
    addr: &SocketAddr,
    resource: &'static str,
) -> Result<(), Response> {
    // Iter-1 review HIGH H1 (auditor A2): dispatch event-name literal
    // by `resource` so when called from a device handler the warn
    // emits `event="device_crud_rejected"` (NOT `application_*`).
    if application_id.is_empty() || application_id.len() > APP_FIELD_MAX_LEN {
        match resource {
            "device" => warn!(
                event = "device_crud_rejected",
                reason = "validation",
                field = "application_id",
                source_ip = %addr.ip(),
                length = application_id.len(),
                "path-supplied application_id length out of range [1, 256]"
            ),
            "application" => warn!(
                event = "application_crud_rejected",
                reason = "validation",
                field = "application_id",
                source_ip = %addr.ip(),
                length = application_id.len(),
                "path-supplied application_id length out of range [1, 256]"
            ),
            _ => warn!(
                event = "crud_rejected",
                reason = "validation",
                resource = resource,
                field = "application_id",
                source_ip = %addr.ip(),
                length = application_id.len(),
                "path-supplied application_id length out of range [1, 256]"
            ),
        }
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                format!(
                    "application_id in URL path must be 1..={} characters",
                    APP_FIELD_MAX_LEN
                ),
                "use ASCII alphanumerics, '-', '_', '.'",
            )),
        )
            .into_response());
    }
    if let Some(bad) = application_id.chars().find(|&c| !is_valid_app_id_char(c)) {
        // Iter-2 P25: log the OFFENDING char as `?bad` (Debug-format)
        // so CRLF and other control chars are escaped as `'\n'`,
        // `'\r'`, `'\u{1b}'` — never interpolated raw.
        match resource {
            "device" => warn!(
                event = "device_crud_rejected",
                reason = "validation",
                field = "application_id",
                source_ip = %addr.ip(),
                bad_char = ?bad,
                "path-supplied application_id contains invalid character"
            ),
            "application" => warn!(
                event = "application_crud_rejected",
                reason = "validation",
                field = "application_id",
                source_ip = %addr.ip(),
                bad_char = ?bad,
                "path-supplied application_id contains invalid character"
            ),
            _ => warn!(
                event = "crud_rejected",
                reason = "validation",
                resource = resource,
                field = "application_id",
                source_ip = %addr.ip(),
                bad_char = ?bad,
                "path-supplied application_id contains invalid character"
            ),
        }
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                "application_id in URL path contains invalid character",
                "use ASCII alphanumerics, '-', '_', '.'",
            )),
        )
            .into_response());
    }
    Ok(())
}

/// Story 9-5 AC#3 + AC#5/AC#8: parallel to
/// [`validate_path_application_id`] for the new `:device_id` URL
/// segment. Same char-class + length bounds; emits
/// `event="device_crud_rejected" reason="validation"` (NOT
/// `application_crud_rejected`) to preserve the path-aware audit
/// dispatch from Story 9-5. Call at the head of EVERY handler that
/// takes a `Path(... device_id ...)` parameter, BEFORE any code that
/// interpolates the value into a tracing field.
#[allow(clippy::result_large_err)]
fn validate_path_device_id(device_id: &str, addr: &SocketAddr) -> Result<(), Response> {
    if device_id.is_empty() || device_id.len() > APP_FIELD_MAX_LEN {
        warn!(
            event = "device_crud_rejected",
            reason = "validation",
            field = "device_id",
            source_ip = %addr.ip(),
            length = device_id.len(),
            "path-supplied device_id length out of range [1, 256]"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                format!(
                    "device_id in URL path must be 1..={} characters",
                    APP_FIELD_MAX_LEN
                ),
                "use ASCII alphanumerics, '-', '_', '.'",
            )),
        )
            .into_response());
    }
    if let Some(bad) = device_id.chars().find(|&c| !is_valid_app_id_char(c)) {
        warn!(
            event = "device_crud_rejected",
            reason = "validation",
            field = "device_id",
            source_ip = %addr.ip(),
            bad_char = ?bad,
            "path-supplied device_id contains invalid character"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                "device_id in URL path contains invalid character",
                "use ASCII alphanumerics, '-', '_', '.'",
            )),
        )
            .into_response());
    }
    Ok(())
}

fn is_valid_app_name_char(c: char) -> bool {
    // Names are allowed slightly more liberal punctuation (space,
    // parentheses) for human-readable display, but still rejects
    // CR/LF/tab and quote characters that could break TOML/HTML/JS.
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '(' | ')')
}

/// `GET /api/applications` — list all applications via the
/// auto-refreshed dashboard snapshot. No backend call.
pub async fn list_applications(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApplicationListResponse>, Response> {
    let snapshot = state
        .dashboard_snapshot
        .read()
        .unwrap_or_else(|e| {
            warn!(
                operation = "rwlock_poison_recovered",
                site = "list_applications",
                "dashboard_snapshot RwLock was poisoned; recovering inner value"
            );
            e.into_inner()
        })
        .clone();
    let applications = snapshot
        .applications
        .iter()
        .map(|app| ApplicationListEntry {
            application_id: app.application_id.clone(),
            application_name: app.application_name.clone(),
            device_count: app.device_count,
        })
        .collect();
    Ok(Json(ApplicationListResponse { applications }))
}

/// `GET /api/applications/:application_id` — single application
/// lookup. 404 on miss.
pub async fn get_application(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(application_id): Path<String>,
) -> Result<Json<ApplicationResponse>, Response> {
    // Iter-2 review P25: validate path-supplied id BEFORE any
    // logging or interpolation.
    validate_path_application_id(&application_id, &addr, "application")?;
    let snapshot = state
        .dashboard_snapshot
        .read()
        .unwrap_or_else(|e| {
            warn!(
                operation = "rwlock_poison_recovered",
                site = "get_application",
                "dashboard_snapshot RwLock was poisoned; recovering inner value"
            );
            e.into_inner()
        })
        .clone();
    if let Some(app) = snapshot
        .applications
        .iter()
        .find(|a| a.application_id == application_id)
    {
        Ok(Json(ApplicationResponse {
            application_id: app.application_id.clone(),
            application_name: app.application_name.clone(),
            device_count: app.device_count,
        }))
    } else {
        Err(application_not_found_response())
    }
}

/// `POST /api/applications` — create a new application. Holds the
/// ConfigWriter lock across write + reload + rollback per the
/// AC#4 lost-update race fix.
pub async fn create_application(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Json(body): Json<CreateApplicationRequest>,
) -> Result<(StatusCode, [(axum::http::HeaderName, String); 1], Json<ApplicationResponse>), Response>
{
    validate_application_field("application_id", &body.application_id, &addr)?;
    validate_application_field("application_name", &body.application_name, &addr)?;

    let _guard = state.config_writer.lock().await;

    let original_bytes = state
        .config_writer
        .read_raw()
        .map_err(|e| io_error_response(&e, "create_application", &addr, "application"))?;
    // Iter-2 review P30: parse the SAME bytes we snapshotted for
    // rollback. Eliminates the TOCTOU window between read_raw and
    // a subsequent load_document call.
    let mut doc = state
        .config_writer
        .parse_document_from_bytes(&original_bytes)
        .map_err(|e| io_error_response(&e, "create_application", &addr, "application"))?;

    // Iter-1 review P2 + Iter-2 review P35 (load-bearing):
    // duplicate-id check INSIDE the write_lock-held critical section,
    // BEFORE append. Without this, two concurrent POSTs with the
    // same `application_id` would both pass pre-lock validation,
    // both append, second reload fails, rollback restores the first
    // request's bytes, both clients see 201.
    //
    // P35 additionally pre-flights malformed `[[application]]`
    // blocks: if any existing block has a missing or non-string
    // `application_id`, the POST is rejected with 409 + a
    // "manual cleanup required" hint. Otherwise the dup-check
    // silently skips that block and the post-write reload's
    // `AppConfig::validate` fails with `application_id: must not be
    // empty` for the pre-existing broken block; rollback then
    // restores the BROKEN state. Pre-flight catches it cleanly.
    {
        let array_ref = doc.get("application").and_then(|v| v.as_array_of_tables());
        if let Some(arr) = array_ref {
            for (idx, tbl) in arr.iter().enumerate() {
                let id_value = tbl.get("application_id");
                let existing_id = match id_value.and_then(|v| v.as_str()) {
                    Some(s) => s,
                    None => {
                        // P35: malformed block — reject up-front.
                        warn!(
                            event = "application_crud_rejected",
                            reason = "conflict",
                            source_ip = %addr.ip(),
                            malformed_block_index = idx,
                            id_value_present = id_value.is_some(),
                            "create_application: existing [[application]] block at index {idx} has missing or non-string application_id; manual cleanup required"
                        );
                        return Err((
                            StatusCode::CONFLICT,
                            Json(ErrorResponse::with_hint(
                                format!(
                                    "config TOML contains a malformed [[application]] block at index {idx} (missing or non-string application_id); manual cleanup required"
                                ),
                                "edit config/config.toml to fix the malformed block before retrying",
                            )),
                        )
                            .into_response());
                    }
                };
                if existing_id == body.application_id {
                    warn!(
                        event = "application_crud_rejected",
                        reason = "conflict",
                        application_id = %body.application_id,
                        source_ip = %addr.ip(),
                        "create_application: duplicate application_id rejected before write"
                    );
                    return Err((
                        StatusCode::CONFLICT,
                        Json(ErrorResponse::with_hint(
                            format!(
                                "application_id '{}' already exists",
                                body.application_id
                            ),
                            "PUT to rename or DELETE the existing application before recreating",
                        )),
                    )
                        .into_response());
                }
            }
        }
    }

    // Append a new [[application]] table.
    let array = doc
        .entry("application")
        .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
        .as_array_of_tables_mut();
    let array = match array {
        Some(a) => a,
        None => {
            warn!(
                event = "application_crud_rejected",
                reason = "io",
                source_ip = %addr.ip(),
                "create_application: existing TOML 'application' key is not an array of tables"
            );
            return Err(internal_error_response());
        }
    };
    let mut new_table = toml_edit::Table::new();
    new_table.insert(
        "application_name",
        toml_edit::value(body.application_name.clone()),
    );
    new_table.insert(
        "application_id",
        toml_edit::value(body.application_id.clone()),
    );
    array.push(new_table);

    let candidate_bytes = doc.to_string().into_bytes();
    if let Err(e) = state.config_writer.write_atomically(&candidate_bytes) {
        // Iter-3 review EH3-H1: `write_atomically` may return Err
        // AFTER the rename has already committed (e.g., post-persist
        // dir-fsync surfaced a non-Unsupported IO error per iter-2
        // P32). The on-disk file then holds the candidate bytes
        // even though we're returning 5xx. Rollback restores the
        // pre-write state so the next CRUD / SIGHUP / restart sees
        // a known-good file. If rollback itself fails, the
        // ConfigWriter is poisoned (D3-P / iter-2 P27) and
        // subsequent CRUD short-circuits with 503.
        handle_rollback(&state, &original_bytes, "create_application", &addr, "write_atomically_err", "application");
        return Err(io_error_response(&e, "create_application", &addr, "application"));
    }

    match state.config_reload.reload().await {
        Ok(_) => {
            info!(
                event = "application_created",
                application_id = %body.application_id,
                application_name = %body.application_name,
                source_ip = %addr.ip(),
                "Application created via web UI"
            );
            let location = format!("/api/applications/{}", body.application_id);
            let response_body = ApplicationResponse {
                application_id: body.application_id,
                application_name: body.application_name,
                device_count: 0,
            };
            Ok((
                StatusCode::CREATED,
                [(axum::http::header::LOCATION, location)],
                Json(response_body),
            ))
        }
        Err(ReloadError::RestartRequired { knob }) => {
            // Iter-1 review D1-P: drift-aware handling.
            let (should_rollback, response) = handle_restart_required(&knob,
                &original_bytes,
                &doc, "create_application", &addr, "application");
            if should_rollback {
                handle_rollback(&state, &original_bytes, "create_application", &addr, &knob, "application");
            }
            Err(response)
        }
        Err(e) => {
            let reason = e.reason().to_string();
            handle_rollback(&state, &original_bytes, "create_application", &addr, &reason, "application");
            Err(reload_error_response(e, "create_application", &addr, "application"))
        }
    }
}

/// `PUT /api/applications/:application_id` — rename an existing
/// application. `application_id` is immutable.
///
/// **Iter-1 review P5:** if the body carries an `application_id`
/// field, the request is rejected with 400 + `event=
/// "application_crud_rejected" reason="immutable_field"` audit
/// event. Previous implementation used `serde(deny_unknown_fields)`
/// which fired BEFORE the handler ran, suppressing the audit event.
pub async fn update_application(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(application_id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<ApplicationResponse>, Response> {
    // Iter-2 review P25: validate path-supplied id BEFORE any
    // logging or interpolation.
    validate_path_application_id(&application_id, &addr, "application")?;

    // Iter-2 review P29: manual deserialisation of the JSON body so
    // we can distinguish `application_id` (immutable_field audit)
    // from other unknown fields (unknown_field audit). Using a
    // strongly-typed `Json<UpdateApplicationRequest>` would either
    // miss unknown fields silently OR (with deny_unknown_fields)
    // fire BEFORE the handler can emit either audit event.
    let obj = body.as_object().ok_or_else(|| {
        warn!(
            event = "application_crud_rejected",
            reason = "validation",
            application_id = %application_id,
            source_ip = %addr.ip(),
            "PUT body must be a JSON object"
        );
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                "PUT body must be a JSON object",
                "send `{\"application_name\": \"...\"}`",
            )),
        )
            .into_response()
    })?;

    let mut new_name: Option<String> = None;
    for (k, v) in obj {
        match k.as_str() {
            "application_name" => {
                let s = v.as_str().ok_or_else(|| {
                    warn!(
                        event = "application_crud_rejected",
                        reason = "validation",
                        application_id = %application_id,
                        source_ip = %addr.ip(),
                        "PUT body field 'application_name' must be a string"
                    );
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse::from_error(
                            "application_name must be a string",
                        )),
                    )
                        .into_response()
                })?;
                new_name = Some(s.to_string());
            }
            "application_id" => {
                warn!(
                    event = "application_crud_rejected",
                    reason = "immutable_field",
                    application_id = %application_id,
                    source_ip = %addr.ip(),
                    "PUT body carried 'application_id' field; rejected (path is authoritative)"
                );
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse::with_hint(
                        "application_id is immutable; delete and recreate to change",
                        "remove the application_id field from the PUT body — the URL path is authoritative",
                    )),
                )
                    .into_response());
            }
            other => {
                warn!(
                    event = "application_crud_rejected",
                    reason = "unknown_field",
                    application_id = %application_id,
                    source_ip = %addr.ip(),
                    field = %other,
                    "PUT body carried unknown field; rejected"
                );
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse::with_hint(
                        format!("PUT body contains unknown field '{other}'"),
                        "PUT accepts only `application_name`",
                    )),
                )
                    .into_response());
            }
        }
    }

    let application_name = new_name.ok_or_else(|| {
        warn!(
            event = "application_crud_rejected",
            reason = "validation",
            application_id = %application_id,
            source_ip = %addr.ip(),
            "PUT body missing required field 'application_name'"
        );
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                "PUT body missing required field 'application_name'",
                "send `{\"application_name\": \"new name\"}`",
            )),
        )
            .into_response()
    })?;

    validate_application_field("application_name", &application_name, &addr)?;

    // Pre-check: the target must exist. Use the live config (not
    // the snapshot) so we always observe the latest state.
    {
        let live = state.config_reload.subscribe();
        let cfg = (*live.borrow()).clone();
        let exists = cfg
            .application_list
            .iter()
            .any(|a| a.application_id == application_id);
        if !exists {
            return Err(application_not_found_response());
        }
    }

    let _guard = state.config_writer.lock().await;
    let original_bytes = state
        .config_writer
        .read_raw()
        .map_err(|e| io_error_response(&e, "update_application", &addr, "application"))?;
    // Iter-2 review P30: parse the SAME bytes we snapshotted.
    let mut doc = state
        .config_writer
        .parse_document_from_bytes(&original_bytes)
        .map_err(|e| io_error_response(&e, "update_application", &addr, "application"))?;

    let array = match doc
        .get_mut("application")
        .and_then(|v| v.as_array_of_tables_mut())
    {
        Some(a) => a,
        None => {
            return Err(internal_error_response());
        }
    };
    // Iter-3 review HR2-2: same malformed-block pre-flight as
    // create_application's iter-2 P35. Without it, PUT silently
    // coerces a malformed block's id to "" via `unwrap_or_default()`,
    // mutates the well-formed match, post-write reload's validate
    // fails on the pre-existing broken block, rollback restores the
    // broken state. Pre-flight catches it cleanly.
    for (idx, tbl) in array.iter().enumerate() {
        if tbl
            .get("application_id")
            .and_then(|v| v.as_str())
            .is_none()
        {
            warn!(
                event = "application_crud_rejected",
                reason = "conflict",
                source_ip = %addr.ip(),
                malformed_block_index = idx,
                "update_application: existing [[application]] block at index {idx} has missing or non-string application_id; manual cleanup required"
            );
            return Err((
                StatusCode::CONFLICT,
                Json(ErrorResponse::with_hint(
                    format!(
                        "config TOML contains a malformed [[application]] block at index {idx} (missing or non-string application_id); manual cleanup required"
                    ),
                    "edit config/config.toml to fix the malformed block before retrying",
                )),
            )
                .into_response());
        }
    }
    // Iter-1 review P3: count occurrences. If the on-disk TOML
    // somehow has duplicate ids (manual edit, botched rollback),
    // refuse to mutate — operating on first-match alone produces
    // silent partial updates.
    let match_count = array
        .iter()
        .filter(|tbl| {
            tbl.get("application_id")
                .and_then(|v| v.as_str())
                == Some(application_id.as_str())
        })
        .count();
    if match_count > 1 {
        warn!(
            event = "application_crud_rejected",
            reason = "conflict",
            application_id = %application_id,
            source_ip = %addr.ip(),
            duplicate_count = match_count,
            "update_application: duplicate application_id in TOML; manual cleanup required"
        );
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse::with_hint(
                format!(
                    "config TOML contains {} entries with application_id '{}'; manual cleanup required",
                    match_count, application_id
                ),
                "edit config/config.toml to remove the duplicate [[application]] block before retrying",
            )),
        )
            .into_response());
    }
    let mut found = false;
    for tbl in array.iter_mut() {
        let id_in_toml = tbl
            .get("application_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if id_in_toml == application_id {
            tbl.insert(
                "application_name",
                toml_edit::value(application_name.clone()),
            );
            found = true;
            break;
        }
    }
    if !found {
        // Should be impossible — pre-check above said it exists.
        return Err(application_not_found_response());
    }

    let candidate_bytes = doc.to_string().into_bytes();
    if let Err(e) = state.config_writer.write_atomically(&candidate_bytes) {
        // Iter-3 review EH3-H1: rollback to recover from post-persist
        // failures (e.g., dir-fsync IO error AFTER the rename
        // committed). Same shape as create_application.
        handle_rollback(&state, &original_bytes, "update_application", &addr, "write_atomically_err", "application");
        return Err(io_error_response(&e, "update_application", &addr, "application"));
    }

    match state.config_reload.reload().await {
        Ok(_) => {
            info!(
                event = "application_updated",
                application_id = %application_id,
                application_name = %application_name,
                source_ip = %addr.ip(),
                "Application updated via web UI"
            );
            // Compute device_count from the post-reload live config.
            let live = state.config_reload.subscribe();
            let cfg = (*live.borrow()).clone();
            let device_count = cfg
                .application_list
                .iter()
                .find(|a| a.application_id == application_id)
                .map(|a| a.device_list.len())
                .unwrap_or(0);
            Ok(Json(ApplicationResponse {
                application_id,
                application_name,
                device_count,
            }))
        }
        Err(ReloadError::RestartRequired { knob }) => {
            let (should_rollback, response) = handle_restart_required(&knob,
                &original_bytes,
                &doc, "update_application", &addr, "application");
            if should_rollback {
                handle_rollback(&state, &original_bytes, "update_application", &addr, &knob, "application");
            }
            Err(response)
        }
        Err(e) => {
            let reason = e.reason().to_string();
            handle_rollback(&state, &original_bytes, "update_application", &addr, &reason, "application");
            Err(reload_error_response(e, "update_application", &addr, "application"))
        }
    }
}

/// `DELETE /api/applications/:application_id` — remove an
/// application. Two pre-conditions enforced before the write_lock:
///   1. The target must have an empty `device_list` (cascade not
///      implemented in v1; defer to Story 9-5).
///   2. Removing the target must not empty `application_list`
///      (`AppConfig::validate` rejects an empty list as a hard
///      error; better to fail-early with a clear 409).
pub async fn delete_application(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(application_id): Path<String>,
) -> Result<StatusCode, Response> {
    // Iter-2 review P25: validate path-supplied id BEFORE any
    // logging or interpolation.
    validate_path_application_id(&application_id, &addr, "application")?;
    // Pre-checks against live config.
    let (target_device_count, total_apps): (usize, usize) = {
        let live = state.config_reload.subscribe();
        let cfg = (*live.borrow()).clone();
        let target = cfg
            .application_list
            .iter()
            .find(|a| a.application_id == application_id);
        match target {
            None => return Err(application_not_found_response()),
            Some(app) => (app.device_list.len(), cfg.application_list.len()),
        }
    };
    if target_device_count > 0 {
        warn!(
            event = "application_crud_rejected",
            reason = "conflict",
            application_id = %application_id,
            source_ip = %addr.ip(),
            device_count = target_device_count,
            "delete_application: target has devices; cascade not implemented"
        );
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse::with_hint(
                format!(
                    "application has {} device(s); remove devices first via /api/devices endpoints (Story 9-5)",
                    target_device_count
                ),
                "DELETE each device individually before deleting the parent application",
            )),
        )
            .into_response());
    }
    if total_apps <= 1 {
        warn!(
            event = "application_crud_rejected",
            reason = "conflict",
            application_id = %application_id,
            source_ip = %addr.ip(),
            "delete_application: would empty application_list"
        );
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse::with_hint(
                "cannot delete the only configured application; application_list must contain at least one entry per AppConfig::validate",
                "create another application first via POST /api/applications, then DELETE this one",
            )),
        )
            .into_response());
    }

    let _guard = state.config_writer.lock().await;
    let original_bytes = state
        .config_writer
        .read_raw()
        .map_err(|e| io_error_response(&e, "delete_application", &addr, "application"))?;
    // Iter-2 review P30: parse the SAME bytes we snapshotted.
    let mut doc = state
        .config_writer
        .parse_document_from_bytes(&original_bytes)
        .map_err(|e| io_error_response(&e, "delete_application", &addr, "application"))?;

    let array = match doc
        .get_mut("application")
        .and_then(|v| v.as_array_of_tables_mut())
    {
        Some(a) => a,
        None => return Err(internal_error_response()),
    };
    // Iter-3 review HR2-2: malformed-block pre-flight (same shape
    // as update_application).
    for (idx, tbl) in array.iter().enumerate() {
        if tbl
            .get("application_id")
            .and_then(|v| v.as_str())
            .is_none()
        {
            warn!(
                event = "application_crud_rejected",
                reason = "conflict",
                source_ip = %addr.ip(),
                malformed_block_index = idx,
                "delete_application: existing [[application]] block at index {idx} has missing or non-string application_id; manual cleanup required"
            );
            return Err((
                StatusCode::CONFLICT,
                Json(ErrorResponse::with_hint(
                    format!(
                        "config TOML contains a malformed [[application]] block at index {idx} (missing or non-string application_id); manual cleanup required"
                    ),
                    "edit config/config.toml to fix the malformed block before retrying",
                )),
            )
                .into_response());
        }
    }
    // Iter-1 review P3: count occurrences before mutating.
    let match_count = array
        .iter()
        .filter(|tbl| {
            tbl.get("application_id")
                .and_then(|v| v.as_str())
                == Some(application_id.as_str())
        })
        .count();
    if match_count > 1 {
        warn!(
            event = "application_crud_rejected",
            reason = "conflict",
            application_id = %application_id,
            source_ip = %addr.ip(),
            duplicate_count = match_count,
            "delete_application: duplicate application_id in TOML; manual cleanup required"
        );
        return Err((
            StatusCode::CONFLICT,
            Json(ErrorResponse::with_hint(
                format!(
                    "config TOML contains {} entries with application_id '{}'; manual cleanup required",
                    match_count, application_id
                ),
                "edit config/config.toml to remove the duplicate [[application]] block before retrying",
            )),
        )
            .into_response());
    }
    let mut idx_to_remove: Option<usize> = None;
    for (i, tbl) in array.iter().enumerate() {
        let id_in_toml = tbl
            .get("application_id")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if id_in_toml == application_id {
            idx_to_remove = Some(i);
            break;
        }
    }
    let Some(idx) = idx_to_remove else {
        return Err(application_not_found_response());
    };
    array.remove(idx);

    let candidate_bytes = doc.to_string().into_bytes();
    if let Err(e) = state.config_writer.write_atomically(&candidate_bytes) {
        // Iter-3 review EH3-H1: rollback on post-persist failure.
        handle_rollback(&state, &original_bytes, "delete_application", &addr, "write_atomically_err", "application");
        return Err(io_error_response(&e, "delete_application", &addr, "application"));
    }

    match state.config_reload.reload().await {
        Ok(_) => {
            info!(
                event = "application_deleted",
                application_id = %application_id,
                source_ip = %addr.ip(),
                "Application deleted via web UI"
            );
            Ok(StatusCode::NO_CONTENT)
        }
        Err(ReloadError::RestartRequired { knob }) => {
            let (should_rollback, response) = handle_restart_required(&knob,
                &original_bytes,
                &doc, "delete_application", &addr, "application");
            if should_rollback {
                handle_rollback(&state, &original_bytes, "delete_application", &addr, &knob, "application");
            }
            Err(response)
        }
        Err(e) => {
            let reason = e.reason().to_string();
            handle_rollback(&state, &original_bytes, "delete_application", &addr, &reason, "application");
            Err(reload_error_response(e, "delete_application", &addr, "application"))
        }
    }
}

// ----------------------------------------------------------------------
// Story 9-5: Device + metric mapping CRUD endpoints (FR35, FR40, FR41,
// AC#1-#13).
//
// Five routes nested under the existing application surface, all behind
// Basic auth (Story 9-1 layer-after-route invariant) and the path-aware
// CSRF middleware (Story 9-5 Task 2 — `event="device_crud_rejected"`
// dispatched by URL path). Read paths consume the auto-refreshed
// `dashboard_snapshot` (Story 9-2/9-3/9-7) for the SUMMARY view; the
// per-device DETAIL view reads the live `Arc<AppConfig>` via
// `state.config_reload.subscribe().borrow()` because the snapshot's
// `MetricSpec` does NOT carry `chirpstack_metric_name` / `metric_unit`
// (Story 9-3 design — see `Existing Infrastructure` table in 9-5 spec).
// Write paths take the `ConfigWriter::lock()` across the entire
// `write+reload+(rollback)` critical section (Story 9-4 lost-update
// fix). PUT-replace-device mutates ONLY the `device_name` field +
// `read_metric` sub-array; any existing `[[application.device.command]]`
// sub-table is preserved byte-for-byte (Story 9-6 territory).
// ----------------------------------------------------------------------

/// `POST /api/applications/:application_id/devices` request body.
///
/// `read_metric_list` defaults to empty so an operator can POST a
/// device skeleton + add metrics later via PUT-replace. The post-9-4
/// warn-demotion of empty `read_metric_list` (`src/config.rs:1586-1595`)
/// makes this a non-error.
///
/// **`serde(deny_unknown_fields)`** so unknown body fields are rejected
/// by serde with 422. POST has no immutable-field rejection (every
/// field is accepting at create time), so the manual-walk pattern from
/// `update_application` is unnecessary here.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDeviceRequest {
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub read_metric_list: Vec<MetricMappingRequest>,
}

/// `PUT /api/applications/:application_id/devices/:device_id` request body.
///
/// **NO `serde(deny_unknown_fields)`** because Story 9-5 handles
/// `device_id` immutable-field rejection manually (Story 9-4 iter-2
/// P29 pattern: deserialise to `serde_json::Value`, walk-and-reject).
/// `read_metric_list` is required (PUT-replaces semantics — caller
/// must ship the full intended list, even if empty).
///
/// Note: the handler does NOT use this struct as the JSON extractor
/// target. It deserialises to `serde_json::Value` and walks the
/// object explicitly to distinguish `device_id` (immutable_field
/// audit) from other unknown fields (unknown_field audit). Kept
/// here for documentation of the v1 contract.
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
pub struct UpdateDeviceRequest {
    pub device_name: String,
    pub read_metric_list: Vec<MetricMappingRequest>,
    /// Story 9-4 P5/P29 pattern: `device_id` is rejected by the
    /// handler with `reason="immutable_field"`; any OTHER unknown
    /// field is rejected with `reason="unknown_field"`.
    #[serde(default)]
    pub device_id: Option<String>,
}

/// One inline metric-mapping entry inside a `CreateDeviceRequest` /
/// `UpdateDeviceRequest`.
///
/// `metric_type` is a string at the wire level; the handler validates
/// against the `OpcMetricTypeConfig` enum vocabulary (`Float | Int |
/// Bool | String`). `metric_unit` is optional free-text.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MetricMappingRequest {
    pub metric_name: String,
    pub chirpstack_metric_name: String,
    pub metric_type: String,
    #[serde(default)]
    pub metric_unit: Option<String>,
}

/// `GET /api/applications/:application_id/devices` response body —
/// summary view (no per-metric detail; counts only).
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DeviceListResponse {
    pub application_id: String,
    pub devices: Vec<DeviceListEntry>,
}

/// One row in [`DeviceListResponse::devices`].
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DeviceListEntry {
    pub device_id: String,
    pub device_name: String,
    pub metric_count: usize,
}

/// `GET|POST|PUT /api/applications/:application_id/devices[/:device_id]`
/// detail-view response body. Carries the FULL metric mapping list.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct DeviceResponse {
    pub device_id: String,
    pub device_name: String,
    pub read_metric_list: Vec<MetricMappingResponse>,
}

/// One row in [`DeviceResponse::read_metric_list`]. `metric_type` is
/// the `OpcMetricTypeConfig::Display` string projected via
/// [`config_type_to_display`].
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct MetricMappingResponse {
    pub metric_name: String,
    pub chirpstack_metric_name: String,
    pub metric_type: String,
    pub metric_unit: Option<String>,
}

/// `GET /api/applications/:application_id/devices` — list devices
/// under a single application via the auto-refreshed dashboard
/// snapshot. Summary view; full metric list available via the
/// per-device GET.
///
/// 404 (no audit event) when `:application_id` does not match —
/// `_crud_rejected` is reserved for state-changing rejections per
/// Story 9-4 audit-event semantic (carry-forward to 9-5 AC#6).
pub async fn list_devices(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(application_id): Path<String>,
) -> Result<Json<DeviceListResponse>, Response> {
    validate_path_application_id(&application_id, &addr, "device")?;

    let snapshot = state
        .dashboard_snapshot
        .read()
        .unwrap_or_else(|e| {
            warn!(
                operation = "rwlock_poison_recovered",
                site = "list_devices",
                "dashboard_snapshot RwLock was poisoned; recovering inner value"
            );
            e.into_inner()
        })
        .clone();
    let app = snapshot
        .applications
        .iter()
        .find(|a| a.application_id == application_id)
        .ok_or_else(application_not_found_response)?;

    let devices = app
        .devices
        .iter()
        .map(|d| DeviceListEntry {
            device_id: d.device_id.clone(),
            device_name: d.device_name.clone(),
            metric_count: d.metrics.len(),
        })
        .collect();

    Ok(Json(DeviceListResponse {
        application_id: application_id.clone(),
        devices,
    }))
}

/// `GET /api/applications/:application_id/devices/:device_id` —
/// per-device DETAIL view. Reads the live `Arc<AppConfig>` (NOT the
/// dashboard snapshot) because the snapshot's `MetricSpec` does not
/// carry `chirpstack_metric_name` / `metric_unit` — see Story 9-5
/// `Existing Infrastructure` table for rationale.
///
/// `subscribe().borrow()` is cheap (clones an Arc); we drop the
/// borrow guard before any `.await` because the guard is `!Send`.
pub async fn get_device(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path((application_id, device_id)): Path<(String, String)>,
) -> Result<Json<DeviceResponse>, Response> {
    validate_path_application_id(&application_id, &addr, "device")?;
    validate_path_device_id(&device_id, &addr)?;

    // Live config — borrow + clone Arc + drop guard immediately.
    let cfg = {
        let live = state.config_reload.subscribe();
        let snap = (*live.borrow()).clone();
        snap
    };

    let app = cfg
        .application_list
        .iter()
        .find(|a| a.application_id == application_id)
        .ok_or_else(application_not_found_response)?;
    let dev = app
        .device_list
        .iter()
        .find(|d| d.device_id == device_id)
        .ok_or_else(device_not_found_response)?;

    let read_metric_list = dev
        .read_metric_list
        .iter()
        .map(|m| MetricMappingResponse {
            metric_name: m.metric_name.clone(),
            chirpstack_metric_name: m.chirpstack_metric_name.clone(),
            metric_type: config_type_to_display(&m.metric_type).to_string(),
            metric_unit: m.metric_unit.clone(),
        })
        .collect();

    Ok(Json(DeviceResponse {
        device_id: dev.device_id.clone(),
        device_name: dev.device_name.clone(),
        read_metric_list,
    }))
}

/// `POST /api/applications/:application_id/devices` — create a new
/// device under the named application. Holds the ConfigWriter lock
/// across write + reload + rollback per the Story 9-4 lost-update
/// race fix.
///
/// Pre-flight rejects: malformed sibling `[[application.device]]`
/// blocks (409), duplicate `device_id` within the application (409 —
/// validate also catches cross-application duplicates at reload time
/// → 422), and parent-application-not-found (404).
pub async fn create_device(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path(application_id): Path<String>,
    Json(body): Json<CreateDeviceRequest>,
) -> Result<(StatusCode, [(axum::http::HeaderName, String); 1], Json<DeviceResponse>), Response> {
    validate_path_application_id(&application_id, &addr, "device")?;

    // Body field validation BEFORE touching the disk.
    validate_device_field("device_id", &body.device_id, &addr)?;
    validate_device_field("device_name", &body.device_name, &addr)?;
    for (idx, m) in body.read_metric_list.iter().enumerate() {
        validate_metric_mapping_fields(idx, m, &addr)?;
    }

    let _guard = state.config_writer.lock().await;

    let original_bytes = state
        .config_writer
        .read_raw()
        .map_err(|e| io_error_response(&e, "create_device", &addr, "device"))?;
    let mut doc = state
        .config_writer
        .parse_document_from_bytes(&original_bytes)
        .map_err(|e| io_error_response(&e, "create_device", &addr, "device"))?;

    // Locate the parent application's `[[application]]` table.
    let app_idx = match find_application_index(&doc, &application_id, &addr, "device") {
        Ok(Some(idx)) => idx,
        Ok(None) => {
            // Parent application not found → 404 + audit event
            // (Story 9-5 AC#6 — `device_crud_rejected reason=
            // application_not_found`).
            warn!(
                event = "device_crud_rejected",
                reason = "application_not_found",
                application_id = %application_id,
                source_ip = %addr.ip(),
                "create_device: parent application not found"
            );
            return Err(application_not_found_response());
        }
        Err(resp) => return Err(resp),
    };

    // Pre-flight + duplicate-id check on the existing `device`
    // sub-array under this application.
    let device_array_existing = doc
        .get("application")
        .and_then(|v| v.as_array_of_tables())
        .and_then(|arr| arr.get(app_idx))
        .and_then(|tbl| tbl.get("device"))
        .and_then(|v| v.as_array_of_tables());
    if let Some(arr) = device_array_existing {
        for (idx, tbl) in arr.iter().enumerate() {
            let id_value = tbl.get("device_id");
            let existing_id = match id_value.and_then(|v| v.as_str()) {
                Some(s) => s,
                None => {
                    warn!(
                        event = "device_crud_rejected",
                        reason = "conflict",
                        application_id = %application_id,
                        source_ip = %addr.ip(),
                        malformed_block_index = idx,
                        id_value_present = id_value.is_some(),
                        "create_device: existing [[application.device]] block at index {idx} has missing or non-string device_id; manual cleanup required"
                    );
                    return Err((
                        StatusCode::CONFLICT,
                        Json(ErrorResponse::with_hint(
                            format!(
                                "config TOML contains a malformed [[application.device]] block at index {idx} (missing or non-string device_id); manual cleanup required"
                            ),
                            "edit config/config.toml to fix the malformed block before retrying",
                        )),
                    )
                        .into_response());
                }
            };
            if existing_id == body.device_id {
                warn!(
                    event = "device_crud_rejected",
                    reason = "conflict",
                    application_id = %application_id,
                    device_id = %body.device_id,
                    source_ip = %addr.ip(),
                    "create_device: duplicate device_id within application rejected before write"
                );
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse::with_hint(
                        format!(
                            "device_id '{}' already exists under application '{}'",
                            body.device_id, application_id
                        ),
                        "PUT to rename or DELETE the existing device before recreating",
                    )),
                )
                    .into_response());
            }
        }
    }

    // Mutate: append a new [[application.device]] table under the
    // matching parent application.
    {
        let app_array = doc
            .get_mut("application")
            .and_then(|v| v.as_array_of_tables_mut());
        let app_array = match app_array {
            Some(a) => a,
            None => return Err(internal_error_response()),
        };
        let app_table = match app_array.get_mut(app_idx) {
            Some(t) => t,
            None => return Err(internal_error_response()),
        };
        let device_array = app_table
            .entry("device")
            .or_insert_with(|| toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new()))
            .as_array_of_tables_mut();
        let device_array = match device_array {
            Some(a) => a,
            None => {
                warn!(
                    event = "device_crud_rejected",
                    reason = "io",
                    application_id = %application_id,
                    source_ip = %addr.ip(),
                    "create_device: existing TOML 'device' key is not an array of tables"
                );
                return Err(internal_error_response());
            }
        };

        let new_table = build_device_table(&body.device_id, &body.device_name, &body.read_metric_list);
        device_array.push(new_table);
    }

    let candidate_bytes = doc.to_string().into_bytes();
    if let Err(e) = state.config_writer.write_atomically(&candidate_bytes) {
        handle_rollback(&state, &original_bytes, "create_device", &addr, "write_atomically_err", "device");
        return Err(io_error_response(&e, "create_device", &addr, "device"));
    }

    match state.config_reload.reload().await {
        Ok(_) => {
            info!(
                event = "device_created",
                application_id = %application_id,
                device_id = %body.device_id,
                device_name = %body.device_name,
                metric_count = body.read_metric_list.len(),
                source_ip = %addr.ip(),
                "Device created via web UI"
            );
            let location = format!(
                "/api/applications/{}/devices/{}",
                application_id, body.device_id
            );
            let read_metric_list = body
                .read_metric_list
                .into_iter()
                .map(|m| MetricMappingResponse {
                    metric_name: m.metric_name,
                    chirpstack_metric_name: m.chirpstack_metric_name,
                    metric_type: m.metric_type,
                    metric_unit: m.metric_unit,
                })
                .collect();
            let response_body = DeviceResponse {
                device_id: body.device_id,
                device_name: body.device_name,
                read_metric_list,
            };
            Ok((
                StatusCode::CREATED,
                [(axum::http::header::LOCATION, location)],
                Json(response_body),
            ))
        }
        Err(ReloadError::RestartRequired { knob }) => {
            let (should_rollback, response) = handle_restart_required(&knob,
                &original_bytes,
                &doc, "create_device", &addr, "device");
            if should_rollback {
                handle_rollback(&state, &original_bytes, "create_device", &addr, &knob, "device");
            }
            Err(response)
        }
        Err(e) => {
            let reason = e.reason().to_string();
            handle_rollback(&state, &original_bytes, "create_device", &addr, &reason, "device");
            Err(reload_error_response(e, "create_device", &addr, "device"))
        }
    }
}

/// `PUT /api/applications/:application_id/devices/:device_id` —
/// replace the named device's `device_name` and `read_metric_list`.
/// `device_id` is immutable.
///
/// **Iter-3 P41 pattern:** the body is deserialised to
/// `serde_json::Value` so we can distinguish `device_id`
/// (immutable_field audit) from other unknown fields (unknown_field
/// audit).
///
/// **Task 6 anti-pattern guard:** the TOML mutation MUST preserve any
/// existing `[[application.device.command]]` sub-table byte-for-byte.
/// We mutate the device table in place — replacing only `device_name`
/// and the `read_metric` array — rather than serialising a
/// `ChirpstackDevice` back via `toml::Value` (which would silently
/// strip the command sub-table since `UpdateDeviceRequest` doesn't
/// carry commands).
pub async fn update_device(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path((application_id, device_id)): Path<(String, String)>,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<DeviceResponse>, Response> {
    validate_path_application_id(&application_id, &addr, "device")?;
    validate_path_device_id(&device_id, &addr)?;

    let obj = body.as_object().ok_or_else(|| {
        warn!(
            event = "device_crud_rejected",
            reason = "validation",
            application_id = %application_id,
            device_id = %device_id,
            source_ip = %addr.ip(),
            "PUT body must be a JSON object"
        );
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                "PUT body must be a JSON object",
                "send `{\"device_name\": \"...\", \"read_metric_list\": [...]}`",
            )),
        )
            .into_response()
    })?;

    let mut new_name: Option<String> = None;
    let mut new_metric_list: Option<Vec<MetricMappingRequest>> = None;
    for (k, v) in obj {
        match k.as_str() {
            "device_name" => {
                let s = v.as_str().ok_or_else(|| {
                    warn!(
                        event = "device_crud_rejected",
                        reason = "validation",
                        application_id = %application_id,
                        device_id = %device_id,
                        source_ip = %addr.ip(),
                        "PUT body field 'device_name' must be a string"
                    );
                    (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse::from_error(
                            "device_name must be a string",
                        )),
                    )
                        .into_response()
                })?;
                new_name = Some(s.to_string());
            }
            "read_metric_list" => {
                let parsed: Vec<MetricMappingRequest> =
                    serde_json::from_value(v.clone()).map_err(|e| {
                        warn!(
                            event = "device_crud_rejected",
                            reason = "validation",
                            application_id = %application_id,
                            device_id = %device_id,
                            source_ip = %addr.ip(),
                            error = %e,
                            "PUT body field 'read_metric_list' failed to deserialise as Vec<MetricMappingRequest>"
                        );
                        (
                            StatusCode::BAD_REQUEST,
                            Json(ErrorResponse::with_hint(
                                "read_metric_list must be an array of {metric_name, chirpstack_metric_name, metric_type, metric_unit} objects",
                                "metric_type must be one of: Float, Int, Bool, String",
                            )),
                        )
                            .into_response()
                    })?;
                new_metric_list = Some(parsed);
            }
            "device_id" => {
                warn!(
                    event = "device_crud_rejected",
                    reason = "immutable_field",
                    application_id = %application_id,
                    device_id = %device_id,
                    source_ip = %addr.ip(),
                    "PUT body carried 'device_id' field; rejected (path is authoritative)"
                );
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse::with_hint(
                        "device_id is immutable; delete and recreate to change",
                        "remove the device_id field from the PUT body — the URL path is authoritative",
                    )),
                )
                    .into_response());
            }
            other => {
                warn!(
                    event = "device_crud_rejected",
                    reason = "unknown_field",
                    application_id = %application_id,
                    device_id = %device_id,
                    source_ip = %addr.ip(),
                    field = %other,
                    "PUT body carried unknown field; rejected"
                );
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse::with_hint(
                        format!("PUT body contains unknown field '{other}'"),
                        "PUT accepts only `device_name` and `read_metric_list`",
                    )),
                )
                    .into_response());
            }
        }
    }

    let device_name = new_name.ok_or_else(|| {
        warn!(
            event = "device_crud_rejected",
            reason = "validation",
            application_id = %application_id,
            device_id = %device_id,
            source_ip = %addr.ip(),
            "PUT body missing required field 'device_name'"
        );
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                "PUT body missing required field 'device_name'",
                "send `{\"device_name\": \"...\", \"read_metric_list\": [...]}`",
            )),
        )
            .into_response()
    })?;
    let read_metric_list = new_metric_list.ok_or_else(|| {
        warn!(
            event = "device_crud_rejected",
            reason = "validation",
            application_id = %application_id,
            device_id = %device_id,
            source_ip = %addr.ip(),
            "PUT body missing required field 'read_metric_list'"
        );
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                "PUT body missing required field 'read_metric_list'",
                "send an array (possibly empty) of metric-mapping objects",
            )),
        )
            .into_response()
    })?;

    validate_device_field("device_name", &device_name, &addr)?;
    for (idx, m) in read_metric_list.iter().enumerate() {
        validate_metric_mapping_fields(idx, m, &addr)?;
    }

    // Pre-check via live config — application + device must exist
    // BEFORE we acquire the writer lock.
    {
        let live = state.config_reload.subscribe();
        let cfg = (*live.borrow()).clone();
        let app = cfg
            .application_list
            .iter()
            .find(|a| a.application_id == application_id);
        let app = match app {
            Some(a) => a,
            None => {
                warn!(
                    event = "device_crud_rejected",
                    reason = "application_not_found",
                    application_id = %application_id,
                    device_id = %device_id,
                    source_ip = %addr.ip(),
                    "update_device: parent application not found"
                );
                return Err(application_not_found_response());
            }
        };
        if !app.device_list.iter().any(|d| d.device_id == device_id) {
            warn!(
                event = "device_crud_rejected",
                reason = "device_not_found",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                "update_device: device not found under application"
            );
            return Err(device_not_found_response());
        }
    }

    let _guard = state.config_writer.lock().await;
    let original_bytes = state
        .config_writer
        .read_raw()
        .map_err(|e| io_error_response(&e, "update_device", &addr, "device"))?;
    let mut doc = state
        .config_writer
        .parse_document_from_bytes(&original_bytes)
        .map_err(|e| io_error_response(&e, "update_device", &addr, "device"))?;

    // Locate parent application table.
    let app_idx = match find_application_index(&doc, &application_id, &addr, "device") {
        Ok(Some(idx)) => idx,
        Ok(None) => {
            warn!(
                event = "device_crud_rejected",
                reason = "application_not_found",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                "update_device: parent application not found (race vs pre-check)"
            );
            return Err(application_not_found_response());
        }
        Err(resp) => return Err(resp),
    };

    // Mutate the matching device table in place.
    {
        let app_array = doc
            .get_mut("application")
            .and_then(|v| v.as_array_of_tables_mut());
        let app_array = match app_array {
            Some(a) => a,
            None => return Err(internal_error_response()),
        };
        let app_table = match app_array.get_mut(app_idx) {
            Some(t) => t,
            None => return Err(internal_error_response()),
        };
        let device_array = app_table
            .get_mut("device")
            .and_then(|v| v.as_array_of_tables_mut());
        let device_array = match device_array {
            Some(a) => a,
            None => {
                warn!(
                    event = "device_crud_rejected",
                    reason = "device_not_found",
                    application_id = %application_id,
                    device_id = %device_id,
                    source_ip = %addr.ip(),
                    "update_device: parent application has no `[[application.device]]` blocks (race vs pre-check)"
                );
                return Err(device_not_found_response());
            }
        };

        // Pre-flight: malformed-block rejection before mutation.
        for (idx, tbl) in device_array.iter().enumerate() {
            if tbl.get("device_id").and_then(|v| v.as_str()).is_none() {
                warn!(
                    event = "device_crud_rejected",
                    reason = "conflict",
                    application_id = %application_id,
                    source_ip = %addr.ip(),
                    malformed_block_index = idx,
                    "update_device: existing [[application.device]] block at index {idx} has missing or non-string device_id; manual cleanup required"
                );
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse::with_hint(
                        format!(
                            "config TOML contains a malformed [[application.device]] block at index {idx} (missing or non-string device_id); manual cleanup required"
                        ),
                        "edit config/config.toml to fix the malformed block before retrying",
                    )),
                )
                    .into_response());
            }
        }

        // Duplicate device_id detection.
        let match_count = device_array
            .iter()
            .filter(|tbl| {
                tbl.get("device_id").and_then(|v| v.as_str()) == Some(device_id.as_str())
            })
            .count();
        if match_count > 1 {
            warn!(
                event = "device_crud_rejected",
                reason = "conflict",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                duplicate_count = match_count,
                "update_device: duplicate device_id in TOML; manual cleanup required"
            );
            return Err((
                StatusCode::CONFLICT,
                Json(ErrorResponse::with_hint(
                    format!(
                        "config TOML contains {} entries with device_id '{}' under application '{}'; manual cleanup required",
                        match_count, device_id, application_id
                    ),
                    "edit config/config.toml to remove the duplicate [[application.device]] block before retrying",
                )),
            )
                .into_response());
        }

        let mut found = false;
        for tbl in device_array.iter_mut() {
            let id_in_toml = tbl
                .get("device_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if id_in_toml == device_id {
                // In-place mutate device_name. Removing + re-inserting
                // would lose any decor (comments) on the key; `insert`
                // updates the existing key in place when present.
                tbl.insert("device_name", toml_edit::value(device_name.clone()));
                // Replace the read_metric sub-array. Remove first so
                // the new array picks up fresh decor; the [[application
                // .device.command]] sibling sub-table is left untouched
                // (Task 6 anti-pattern guard).
                tbl.remove("read_metric");
                if !read_metric_list.is_empty() {
                    let new_metrics = build_read_metric_array(&read_metric_list);
                    tbl.insert(
                        "read_metric",
                        toml_edit::Item::ArrayOfTables(new_metrics),
                    );
                }
                found = true;
                break;
            }
        }
        if !found {
            warn!(
                event = "device_crud_rejected",
                reason = "device_not_found",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                "update_device: device not found in TOML at mutate time (race vs pre-check)"
            );
            return Err(device_not_found_response());
        }
    }

    let candidate_bytes = doc.to_string().into_bytes();
    if let Err(e) = state.config_writer.write_atomically(&candidate_bytes) {
        handle_rollback(&state, &original_bytes, "update_device", &addr, "write_atomically_err", "device");
        return Err(io_error_response(&e, "update_device", &addr, "device"));
    }

    match state.config_reload.reload().await {
        Ok(_) => {
            info!(
                event = "device_updated",
                application_id = %application_id,
                device_id = %device_id,
                device_name = %device_name,
                metric_count = read_metric_list.len(),
                source_ip = %addr.ip(),
                "Device updated via web UI"
            );
            let read_metric_list_resp = read_metric_list
                .into_iter()
                .map(|m| MetricMappingResponse {
                    metric_name: m.metric_name,
                    chirpstack_metric_name: m.chirpstack_metric_name,
                    metric_type: m.metric_type,
                    metric_unit: m.metric_unit,
                })
                .collect();
            Ok(Json(DeviceResponse {
                device_id,
                device_name,
                read_metric_list: read_metric_list_resp,
            }))
        }
        Err(ReloadError::RestartRequired { knob }) => {
            let (should_rollback, response) = handle_restart_required(&knob,
                &original_bytes,
                &doc, "update_device", &addr, "device");
            if should_rollback {
                handle_rollback(&state, &original_bytes, "update_device", &addr, &knob, "device");
            }
            Err(response)
        }
        Err(e) => {
            let reason = e.reason().to_string();
            handle_rollback(&state, &original_bytes, "update_device", &addr, &reason, "device");
            Err(reload_error_response(e, "update_device", &addr, "device"))
        }
    }
}

/// `DELETE /api/applications/:application_id/devices/:device_id` —
/// remove the named device from the named application. v1 leaves
/// orphaned `metric_values` / `metric_history` rows for the deleted
/// `device_id` in storage; the pruning task (Story 2-5a) eventually
/// removes them via the retention window. Documented in
/// `docs/security.md § Configuration mutations § v1 limitations`.
///
/// **No "last device" pre-check.** Empty `device_list` is now a warn
/// (Story 9-4 demotion); an application with zero devices is a valid
/// state (the dashboard renders it with a "0 devices configured"
/// badge).
pub async fn delete_device(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    Path((application_id, device_id)): Path<(String, String)>,
) -> Result<StatusCode, Response> {
    validate_path_application_id(&application_id, &addr, "device")?;
    validate_path_device_id(&device_id, &addr)?;

    // Pre-check via live config — application + device must exist
    // BEFORE we acquire the writer lock.
    {
        let live = state.config_reload.subscribe();
        let cfg = (*live.borrow()).clone();
        let app = cfg
            .application_list
            .iter()
            .find(|a| a.application_id == application_id);
        let app = match app {
            Some(a) => a,
            None => {
                warn!(
                    event = "device_crud_rejected",
                    reason = "application_not_found",
                    application_id = %application_id,
                    device_id = %device_id,
                    source_ip = %addr.ip(),
                    "delete_device: parent application not found"
                );
                return Err(application_not_found_response());
            }
        };
        if !app.device_list.iter().any(|d| d.device_id == device_id) {
            warn!(
                event = "device_crud_rejected",
                reason = "device_not_found",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                "delete_device: device not found under application"
            );
            return Err(device_not_found_response());
        }
    }

    let _guard = state.config_writer.lock().await;
    let original_bytes = state
        .config_writer
        .read_raw()
        .map_err(|e| io_error_response(&e, "delete_device", &addr, "device"))?;
    let mut doc = state
        .config_writer
        .parse_document_from_bytes(&original_bytes)
        .map_err(|e| io_error_response(&e, "delete_device", &addr, "device"))?;

    let app_idx = match find_application_index(&doc, &application_id, &addr, "device") {
        Ok(Some(idx)) => idx,
        Ok(None) => {
            warn!(
                event = "device_crud_rejected",
                reason = "application_not_found",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                "delete_device: parent application not found (race vs pre-check)"
            );
            return Err(application_not_found_response());
        }
        Err(resp) => return Err(resp),
    };

    {
        let app_array = doc
            .get_mut("application")
            .and_then(|v| v.as_array_of_tables_mut());
        let app_array = match app_array {
            Some(a) => a,
            None => return Err(internal_error_response()),
        };
        let app_table = match app_array.get_mut(app_idx) {
            Some(t) => t,
            None => return Err(internal_error_response()),
        };
        let device_array = app_table
            .get_mut("device")
            .and_then(|v| v.as_array_of_tables_mut());
        let device_array = match device_array {
            Some(a) => a,
            None => {
                warn!(
                    event = "device_crud_rejected",
                    reason = "device_not_found",
                    application_id = %application_id,
                    device_id = %device_id,
                    source_ip = %addr.ip(),
                    "delete_device: parent application has no `[[application.device]]` blocks (race vs pre-check)"
                );
                return Err(device_not_found_response());
            }
        };

        // Pre-flight: malformed-block rejection.
        for (idx, tbl) in device_array.iter().enumerate() {
            if tbl.get("device_id").and_then(|v| v.as_str()).is_none() {
                warn!(
                    event = "device_crud_rejected",
                    reason = "conflict",
                    application_id = %application_id,
                    source_ip = %addr.ip(),
                    malformed_block_index = idx,
                    "delete_device: existing [[application.device]] block at index {idx} has missing or non-string device_id; manual cleanup required"
                );
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse::with_hint(
                        format!(
                            "config TOML contains a malformed [[application.device]] block at index {idx} (missing or non-string device_id); manual cleanup required"
                        ),
                        "edit config/config.toml to fix the malformed block before retrying",
                    )),
                )
                    .into_response());
            }
        }

        // Duplicate device_id detection — refuse to mutate if the
        // TOML has duplicates (manual edit, botched rollback).
        let match_count = device_array
            .iter()
            .filter(|tbl| {
                tbl.get("device_id").and_then(|v| v.as_str()) == Some(device_id.as_str())
            })
            .count();
        if match_count > 1 {
            warn!(
                event = "device_crud_rejected",
                reason = "conflict",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                duplicate_count = match_count,
                "delete_device: duplicate device_id in TOML; manual cleanup required"
            );
            return Err((
                StatusCode::CONFLICT,
                Json(ErrorResponse::with_hint(
                    format!(
                        "config TOML contains {} entries with device_id '{}' under application '{}'; manual cleanup required",
                        match_count, device_id, application_id
                    ),
                    "edit config/config.toml to remove the duplicate [[application.device]] block before retrying",
                )),
            )
                .into_response());
        }

        let mut idx_to_remove: Option<usize> = None;
        for (i, tbl) in device_array.iter().enumerate() {
            let id_in_toml = tbl
                .get("device_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if id_in_toml == device_id {
                idx_to_remove = Some(i);
                break;
            }
        }
        let Some(idx) = idx_to_remove else {
            warn!(
                event = "device_crud_rejected",
                reason = "device_not_found",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                "delete_device: device not found in TOML at mutate time (race vs pre-check)"
            );
            return Err(device_not_found_response());
        };
        // `array_of_tables.remove(idx)` removes the table along with
        // its sub-tables (including [[application.device.command]]
        // and [[application.device.read_metric]]). Operator-visible:
        // the deleted device is gone in its entirety from the config;
        // orphaned storage rows persist per the v1 cascade-delete
        // limitation documented in docs/security.md.
        device_array.remove(idx);
    }

    let candidate_bytes = doc.to_string().into_bytes();
    if let Err(e) = state.config_writer.write_atomically(&candidate_bytes) {
        handle_rollback(&state, &original_bytes, "delete_device", &addr, "write_atomically_err", "device");
        return Err(io_error_response(&e, "delete_device", &addr, "device"));
    }

    match state.config_reload.reload().await {
        Ok(_) => {
            info!(
                event = "device_deleted",
                application_id = %application_id,
                device_id = %device_id,
                source_ip = %addr.ip(),
                "Device deleted via web UI"
            );
            Ok(StatusCode::NO_CONTENT)
        }
        Err(ReloadError::RestartRequired { knob }) => {
            let (should_rollback, response) = handle_restart_required(&knob,
                &original_bytes,
                &doc, "delete_device", &addr, "device");
            if should_rollback {
                handle_rollback(&state, &original_bytes, "delete_device", &addr, &knob, "device");
            }
            Err(response)
        }
        Err(e) => {
            let reason = e.reason().to_string();
            handle_rollback(&state, &original_bytes, "delete_device", &addr, &reason, "device");
            Err(reload_error_response(e, "delete_device", &addr, "device"))
        }
    }
}

// ----------------------------------------------------------------------
// Story 9-5 device CRUD helpers.
// ----------------------------------------------------------------------

/// Validate every field of a [`MetricMappingRequest`]. Emits
/// `device_crud_rejected reason=validation` on failure (via
/// [`validate_device_field`]).
#[allow(clippy::result_large_err)]
fn validate_metric_mapping_fields(
    idx: usize,
    m: &MetricMappingRequest,
    addr: &SocketAddr,
) -> Result<(), Response> {
    // Iter-3 review (Blind #1): the previous `validate_metric_field_with_idx`
    // wrapper became a tautological one-line delegate after the iter-2 H1
    // double-emit fix. Replaced with direct `validate_device_field` calls.
    // The `idx` parameter is consumed by the control-char branch below
    // (which carries `metric_index` in its own warn), and by the response
    // body's `format!("metric_list[{idx}]...")` for operator guidance.
    // Threading `metric_index` into `validate_device_field`'s warns is
    // tracked under deferred-work `9-5-iter2-D1`.
    validate_device_field("metric_name", &m.metric_name, addr)?;
    validate_device_field("chirpstack_metric_name", &m.chirpstack_metric_name, addr)?;
    validate_device_field("metric_type", &m.metric_type, addr)?;
    if let Some(unit) = m.metric_unit.as_deref() {
        // Iter-1 review L5 (Edge E3): reject control characters in
        // `metric_unit` — toml_edit emits the value verbatim, so a
        // raw newline/CR/ANSI escape would corrupt the round-trip
        // and could pollute logs that interpolate the value.
        if let Some(bad) = unit.chars().find(|c| c.is_control()) {
            warn!(
                event = "device_crud_rejected",
                reason = "validation",
                field = "metric_unit",
                metric_index = idx,
                source_ip = %addr.ip(),
                bad_char = ?bad,
                "metric_unit contains a control character"
            );
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::with_hint(
                    format!("metric_list[{idx}].metric_unit contains a control character"),
                    "use printable characters only (e.g. \"°C\", \"%\", \"m³\")",
                )),
            )
                .into_response());
        }
        validate_device_field("metric_unit", unit, addr)?;
    }
    Ok(())
}

/// Locate the index of the `[[application]]` table whose
/// `application_id` matches `application_id`. Returns `Ok(Some(idx))`
/// on hit, `Ok(None)` if no match. On malformed-block detection
/// (existing `[[application]]` block with missing or non-string
/// `application_id`), emits `<resource>_crud_rejected reason=conflict`
/// and returns `Err(response)`. The `resource` parameter mirrors the
/// iter-1 H1 dispatch pattern in `handle_rollback` / `io_error_response`
/// / `validate_path_application_id` so callers from device handlers
/// emit `device_crud_rejected` and any future Story 9-6 command-handler
/// reuse can emit `command_crud_rejected` without changing this helper.
///
/// Iter-3 review (Blind #3): added `resource` parameter to defuse the
/// Story 9-6 landmine where this helper would otherwise misroute
/// command-handler malformed-block warns through the device event-name
/// literal.
#[allow(clippy::result_large_err)]
fn find_application_index(
    doc: &toml_edit::DocumentMut,
    application_id: &str,
    addr: &SocketAddr,
    resource: &'static str,
) -> Result<Option<usize>, Response> {
    let arr = match doc.get("application").and_then(|v| v.as_array_of_tables()) {
        Some(a) => a,
        None => return Ok(None),
    };
    for (idx, tbl) in arr.iter().enumerate() {
        let id_value = tbl.get("application_id");
        match id_value.and_then(|v| v.as_str()) {
            Some(s) if s == application_id => return Ok(Some(idx)),
            Some(_) => continue,
            None => {
                match resource {
                    "device" => warn!(
                        event = "device_crud_rejected",
                        reason = "conflict",
                        application_id = %application_id,
                        source_ip = %addr.ip(),
                        malformed_block_index = idx,
                        "handler: existing [[application]] block at index {idx} has missing or non-string application_id; manual cleanup required"
                    ),
                    "application" => warn!(
                        event = "application_crud_rejected",
                        reason = "conflict",
                        application_id = %application_id,
                        source_ip = %addr.ip(),
                        malformed_block_index = idx,
                        "handler: existing [[application]] block at index {idx} has missing or non-string application_id; manual cleanup required"
                    ),
                    _ => warn!(
                        event = "crud_rejected",
                        reason = "conflict",
                        resource = resource,
                        application_id = %application_id,
                        source_ip = %addr.ip(),
                        malformed_block_index = idx,
                        "handler: existing [[application]] block at index {idx} has missing or non-string application_id; manual cleanup required"
                    ),
                }
                return Err((
                    StatusCode::CONFLICT,
                    Json(ErrorResponse::with_hint(
                        format!(
                            "config TOML contains a malformed [[application]] block at index {idx} (missing or non-string application_id); manual cleanup required"
                        ),
                        "edit config/config.toml to fix the malformed block before retrying",
                    )),
                )
                    .into_response());
            }
        }
    }
    Ok(None)
}

/// Build a fresh `[[application.device]]` table with the given fields
/// and the `[[application.device.read_metric]]` sub-array. Used by
/// POST (whole-device construction). PUT cannot use this because PUT
/// must preserve the existing `[[application.device.command]]`
/// sub-table in place (Task 6 anti-pattern guard).
fn build_device_table(
    device_id: &str,
    device_name: &str,
    metric_list: &[MetricMappingRequest],
) -> toml_edit::Table {
    let mut tbl = toml_edit::Table::new();
    tbl.insert("device_id", toml_edit::value(device_id.to_string()));
    tbl.insert("device_name", toml_edit::value(device_name.to_string()));
    if !metric_list.is_empty() {
        let metrics_array = build_read_metric_array(metric_list);
        tbl.insert(
            "read_metric",
            toml_edit::Item::ArrayOfTables(metrics_array),
        );
    }
    tbl
}

/// Build a fresh `[[application.device.read_metric]]` array of tables
/// from the request's `metric_list`. Order is preserved (load-bearing
/// per Story 9-3 iter-1 H1 — TOML-declaration order drives dashboard
/// rendering order and address-space registration order).
fn build_read_metric_array(metric_list: &[MetricMappingRequest]) -> toml_edit::ArrayOfTables {
    let mut arr = toml_edit::ArrayOfTables::new();
    for m in metric_list {
        let mut row = toml_edit::Table::new();
        row.insert("metric_name", toml_edit::value(m.metric_name.clone()));
        row.insert(
            "chirpstack_metric_name",
            toml_edit::value(m.chirpstack_metric_name.clone()),
        );
        row.insert("metric_type", toml_edit::value(m.metric_type.clone()));
        if let Some(unit) = &m.metric_unit {
            row.insert("metric_unit", toml_edit::value(unit.clone()));
        }
        arr.push(row);
    }
    arr
}

// ----------------------------------------------------------------------
// CRUD helpers (private to api.rs).
//
// `clippy::result_large_err` is allowed for these helpers because the
// error variant is `axum::response::Response`, which is large by design
// (the boxed body). Boxing it would be over-engineering since the error
// path is the cold path and the `Response` only lives until it's
// returned to the axum service stack.
// ----------------------------------------------------------------------

#[allow(clippy::result_large_err)]
fn validate_application_field(
    field: &str,
    value: &str,
    addr: &SocketAddr,
) -> Result<(), Response> {
    // Iter-1 review P16: reject empty AND whitespace-only.
    if value.trim().is_empty() {
        warn!(
            event = "application_crud_rejected",
            reason = "validation",
            field = %field,
            source_ip = %addr.ip(),
            "CRUD field validation failed: empty or whitespace-only"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                format!("{field} must not be empty or whitespace-only"),
                "provide a non-empty value with at least one non-whitespace character",
            )),
        )
            .into_response());
    }
    if value.len() > APP_FIELD_MAX_LEN {
        warn!(
            event = "application_crud_rejected",
            reason = "validation",
            field = %field,
            source_ip = %addr.ip(),
            length = value.len(),
            "CRUD field validation failed: too long"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                format!(
                    "{field} length {} exceeds maximum of {}",
                    value.len(),
                    APP_FIELD_MAX_LEN
                ),
                format!("shorten {field} to <= {APP_FIELD_MAX_LEN} characters"),
            )),
        )
            .into_response());
    }
    // Iter-1 review P1: char-class restriction. Refuses CR/LF (which
    // would inject into the Location header), `/` (path traversal in
    // the URL), and other characters that break TOML escape or HTML
    // escape contracts downstream.
    let allowed: fn(char) -> bool = match field {
        "application_id" => is_valid_app_id_char,
        "application_name" => is_valid_app_name_char,
        _ => is_valid_app_id_char,
    };
    if let Some(bad) = value.chars().find(|&c| !allowed(c)) {
        warn!(
            event = "application_crud_rejected",
            reason = "validation",
            field = %field,
            source_ip = %addr.ip(),
            bad_char = ?bad,
            "CRUD field validation failed: invalid character"
        );
        let hint = match field {
            "application_id" => "use ASCII alphanumerics, '-', '_', '.'",
            _ => "use ASCII alphanumerics, '-', '_', '.', spaces, or parentheses",
        };
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                format!(
                    "{field} contains invalid character {:?}",
                    bad
                ),
                hint,
            )),
        )
            .into_response());
    }
    Ok(())
}

/// Story 9-5 AC#3: per-device + per-metric body-field validator.
/// Parallel to `validate_application_field` but emits
/// `event="device_crud_rejected"` (NOT `application_crud_rejected`)
/// per AC#5/AC#8 path-aware dispatch.
///
/// Field-name dispatch:
/// - `device_id` / `metric_name` / `chirpstack_metric_name`: char-class restricted (`is_valid_app_id_char`), max length [`APP_FIELD_MAX_LEN`].
/// - `device_name`: looser (`is_valid_app_name_char` — allows spaces / parentheses), max length [`APP_FIELD_MAX_LEN`].
/// - `metric_unit`: any string, max length [`METRIC_UNIT_MAX_LEN`] (= 64). Empty allowed when caller passes a zero-length string explicitly; `None` skips validation entirely (the caller controls when to invoke this).
/// - `metric_type`: must equal one of `"Float" | "Int" | "Bool" | "String"` (case-sensitive — matches the `OpcMetricTypeConfig` enum's `Deserialize` derive behaviour).
///
/// All other field names get the conservative char-class treatment.
#[allow(clippy::result_large_err)]
fn validate_device_field(field: &str, value: &str, addr: &SocketAddr) -> Result<(), Response> {
    // metric_type uses an enum-vocabulary check, not length / char-class.
    if field == "metric_type" {
        if !matches!(value, "Float" | "Int" | "Bool" | "String") {
            warn!(
                event = "device_crud_rejected",
                reason = "validation",
                field = %field,
                source_ip = %addr.ip(),
                value = %value,
                "device CRUD field validation failed: metric_type not in {{Float,Int,Bool,String}}"
            );
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse::with_hint(
                    "metric_type must be one of: Float, Int, Bool, String",
                    "case-sensitive; matches OpcMetricTypeConfig enum",
                )),
            )
                .into_response());
        }
        return Ok(());
    }

    // metric_unit has its own length budget; empty allowed.
    let max_len = if field == "metric_unit" {
        METRIC_UNIT_MAX_LEN
    } else {
        APP_FIELD_MAX_LEN
    };

    // metric_unit allows empty (the caller passes "" explicitly when
    // they want to clear it); other fields reject empty/whitespace-only.
    if field != "metric_unit" && value.trim().is_empty() {
        warn!(
            event = "device_crud_rejected",
            reason = "validation",
            field = %field,
            source_ip = %addr.ip(),
            "device CRUD field validation failed: empty or whitespace-only"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                format!("{field} must not be empty or whitespace-only"),
                "provide a non-empty value with at least one non-whitespace character",
            )),
        )
            .into_response());
    }

    if value.len() > max_len {
        warn!(
            event = "device_crud_rejected",
            reason = "validation",
            field = %field,
            source_ip = %addr.ip(),
            length = value.len(),
            "device CRUD field validation failed: too long"
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                format!("{field} length {} exceeds maximum of {}", value.len(), max_len),
                format!("shorten {field} to <= {max_len} characters"),
            )),
        )
            .into_response());
    }

    // metric_unit accepts any character (operator-friendly free text:
    // °C, %, V, m³, etc.).
    if field == "metric_unit" {
        return Ok(());
    }

    let allowed: fn(char) -> bool = match field {
        "device_name" => is_valid_app_name_char,
        // device_id / metric_name / chirpstack_metric_name: strict
        _ => is_valid_app_id_char,
    };
    if let Some(bad) = value.chars().find(|&c| !allowed(c)) {
        warn!(
            event = "device_crud_rejected",
            reason = "validation",
            field = %field,
            source_ip = %addr.ip(),
            bad_char = ?bad,
            "device CRUD field validation failed: invalid character"
        );
        let hint = if field == "device_name" {
            "use ASCII alphanumerics, '-', '_', '.', spaces, or parentheses"
        } else {
            "use ASCII alphanumerics, '-', '_', '.'"
        };
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse::with_hint(
                format!("{field} contains invalid character {:?}", bad),
                hint,
            )),
        )
            .into_response());
    }
    Ok(())
}

/// Story 9-5 AC#3: max length for `metric_unit` body field.
/// Operator-friendly short suffixes only (°C, %, V, m³, etc.).
/// Distinct from [`APP_FIELD_MAX_LEN`] which covers ID-shaped fields.
const METRIC_UNIT_MAX_LEN: usize = 64;

/// Iter-1 review (refactor): centralise the rollback + audit-event
/// pattern shared by all three CRUD handlers. Logs `rollback_failed`
/// at warn level if `rollback()` itself returns `Err(_)`. The
/// `cause` parameter feeds the audit-log context (typically the
/// failing reason / knob).
fn handle_rollback(
    state: &Arc<AppState>,
    original_bytes: &[u8],
    site: &'static str,
    addr: &SocketAddr,
    cause: &str,
    resource: &'static str,
) {
    // Iter-1 review HIGH H1: dispatch event-name literal by `resource`
    // so device-CRUD failures emit `event="device_crud_rejected"` and
    // application-CRUD failures emit `event="application_crud_rejected"`.
    // Match arms preserve the AC#8 source-grep contract (unique names).
    if let Err(rb) = state.config_writer.rollback(original_bytes) {
        match resource {
            "device" => warn!(
                event = "device_crud_rejected",
                reason = "rollback_failed",
                site = %site,
                source_ip = %addr.ip(),
                rollback_error = %rb,
                reload_cause = %cause,
                "rollback FAILED — config TOML on disk is now in an inconsistent state; ConfigWriter is poisoned"
            ),
            "application" => warn!(
                event = "application_crud_rejected",
                reason = "rollback_failed",
                site = %site,
                source_ip = %addr.ip(),
                rollback_error = %rb,
                reload_cause = %cause,
                "rollback FAILED — config TOML on disk is now in an inconsistent state; ConfigWriter is poisoned"
            ),
            _ => warn!(
                event = "crud_rejected",
                reason = "rollback_failed",
                resource = resource,
                site = %site,
                source_ip = %addr.ip(),
                rollback_error = %rb,
                reload_cause = %cause,
                "rollback FAILED — config TOML on disk is now in an inconsistent state; ConfigWriter is poisoned"
            ),
        }
    }
}

fn application_not_found_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse::from_error("application not found")),
    )
        .into_response()
}

/// Story 9-5 AC#6: parallel to `application_not_found_response` for
/// the new `:device_id` URL segment. Returns 404 with the same body
/// shape; the audit-event emission is the caller's responsibility
/// (the `_crud_rejected` warn fires only on mutating-method paths,
/// not on GET 404s — Story 9-5 audit-event semantic).
fn device_not_found_response() -> Response {
    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse::from_error("device not found")),
    )
        .into_response()
}

fn internal_error_response() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse::internal_server_error()),
    )
        .into_response()
}

fn io_error_response(
    e: &crate::utils::OpcGwError,
    site: &'static str,
    addr: &SocketAddr,
    resource: &'static str,
) -> Response {
    // Iter-1 review D3-P: distinguish "transient IO" (500) from
    // "writer poisoned, gateway in inconsistent state" (503). The
    // poisoned-error wording carries the literal "poisoned" token
    // emitted by `ConfigWriter::poisoned_err`.
    //
    // Iter-1 review HIGH H1: dispatch event-name literal by `resource`
    // so device-CRUD IO/poisoned failures emit `device_crud_rejected`
    // and application-CRUD failures emit `application_crud_rejected`.
    let display = e.to_string();
    if display.contains("config writer poisoned") {
        match resource {
            "device" => warn!(
                event = "device_crud_rejected",
                reason = "poisoned",
                site = %site,
                source_ip = %addr.ip(),
                error = %e,
                "CRUD rejected: ConfigWriter is poisoned (prior rollback failed); restart required"
            ),
            "application" => warn!(
                event = "application_crud_rejected",
                reason = "poisoned",
                site = %site,
                source_ip = %addr.ip(),
                error = %e,
                "CRUD rejected: ConfigWriter is poisoned (prior rollback failed); restart required"
            ),
            _ => warn!(
                event = "crud_rejected",
                reason = "poisoned",
                resource = resource,
                site = %site,
                source_ip = %addr.ip(),
                error = %e,
                "CRUD rejected: ConfigWriter is poisoned (prior rollback failed); restart required"
            ),
        }
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse::with_hint(
                "gateway in inconsistent state: prior rollback failed",
                "restart the gateway; on-disk config may need manual review",
            )),
        )
            .into_response();
    }
    match resource {
        "device" => warn!(
            event = "device_crud_rejected",
            reason = "io",
            site = %site,
            source_ip = %addr.ip(),
            error = %e,
            "CRUD IO failure"
        ),
        "application" => warn!(
            event = "application_crud_rejected",
            reason = "io",
            site = %site,
            source_ip = %addr.ip(),
            error = %e,
            "CRUD IO failure"
        ),
        _ => warn!(
            event = "crud_rejected",
            reason = "io",
            resource = resource,
            site = %site,
            source_ip = %addr.ip(),
            error = %e,
            "CRUD IO failure"
        ),
    }
    internal_error_response()
}

/// Iter-1 review D1-P: when reload returns `RestartRequired { knob }`,
/// determine whether the offending knob is part of the just-written
/// CRUD delta or pre-existing operator drift on disk.
///
/// Returns `Ok(true)` if the delta caused the change; `Ok(false)` if
/// the knob was already different on disk before our write (ambient
/// drift). On parse failure, conservatively returns `Err(_)` and the
/// caller falls back to the standard rollback path.
fn knob_in_delta(
    knob: &str,
    original_bytes: &[u8],
    candidate_doc: &toml_edit::DocumentMut,
) -> Result<bool, OpcGwError> {
    let original = std::str::from_utf8(original_bytes)
        .map_err(|e| OpcGwError::Web(format!("non-UTF-8 original bytes: {e}")))?;
    let original_doc: toml_edit::DocumentMut = original
        .parse()
        .map_err(|e| OpcGwError::Web(format!("failed to parse original TOML for drift check: {e}")))?;
    // `knob` is dotted (e.g. "chirpstack.server_address"). Walk both
    // documents; compare the resolved values.
    let mut path = knob.split('.');
    let head = match path.next() {
        Some(h) => h,
        None => return Ok(false),
    };
    let original_section = original_doc.get(head);
    let candidate_section = candidate_doc.get(head);
    let mut original_item = original_section;
    let mut candidate_item = candidate_section;
    for segment in path {
        original_item = original_item.and_then(|i| i.get(segment));
        candidate_item = candidate_item.and_then(|i| i.get(segment));
    }
    let original_str = original_item.map(|i| i.to_string()).unwrap_or_default();
    let candidate_str = candidate_item.map(|i| i.to_string()).unwrap_or_default();
    Ok(original_str != candidate_str)
}

#[allow(clippy::result_large_err)]
fn reload_error_response(
    e: ReloadError,
    site: &'static str,
    addr: &SocketAddr,
    resource: &'static str,
) -> Response {
    let reason = e.reason();
    let status = match reason {
        "validation" => StatusCode::UNPROCESSABLE_ENTITY,
        // RestartRequired and Io both map to 500 per AC#3.
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    // Iter-1 review HIGH H1: dispatch event-name literal by `resource`
    // so reload-validation/RestartRequired/Io failures from device
    // handlers emit `device_crud_rejected`, not `application_*`.
    match resource {
        "device" => warn!(
            event = "device_crud_rejected",
            reason = %reason,
            site = %site,
            source_ip = %addr.ip(),
            error = %e,
            "CRUD reload failure"
        ),
        "application" => warn!(
            event = "application_crud_rejected",
            reason = %reason,
            site = %site,
            source_ip = %addr.ip(),
            error = %e,
            "CRUD reload failure"
        ),
        _ => warn!(
            event = "crud_rejected",
            reason = %reason,
            resource = resource,
            site = %site,
            source_ip = %addr.ip(),
            error = %e,
            "CRUD reload failure"
        ),
    }
    let body = match reason {
        "validation" => ErrorResponse::with_hint(
            format!("config validation failed: {e}"),
            "fix the offending field and retry",
        ),
        _ => ErrorResponse::internal_server_error(),
    };
    let _ = ReloadOutcome::NoChange; // touch the symbol to keep the import live; clippy ignores the unused for the imports we need
    (status, Json(body)).into_response()
}

/// Iter-1 review D1-P: drift-aware response for `RestartRequired`.
/// If the offending knob is NOT in the just-written delta, refuse to
/// roll back — the operator has unrelated disk edits and we should
/// surface a clear 409 instead of silently reverting their work.
/// Returns `(should_rollback, response)` so the caller decides.
#[allow(clippy::result_large_err)]
fn handle_restart_required(
    knob: &str,
    original_bytes: &[u8],
    candidate_doc: &toml_edit::DocumentMut,
    site: &'static str,
    addr: &SocketAddr,
    resource: &'static str,
) -> (bool, Response) {
    // Iter-1 review HIGH H1: dispatch ambient_drift event-name literal
    // by `resource`. Delegated rollback path also forwards `resource`.
    match knob_in_delta(knob, original_bytes, candidate_doc) {
        Ok(false) => {
            // Ambient drift — refuse rollback.
            match resource {
                "device" => warn!(
                    event = "device_crud_rejected",
                    reason = "ambient_drift",
                    site = %site,
                    source_ip = %addr.ip(),
                    changed_knob = %knob,
                    "CRUD rejected: TOML has unrelated changes since gateway start; refusing to roll back"
                ),
                "application" => warn!(
                    event = "application_crud_rejected",
                    reason = "ambient_drift",
                    site = %site,
                    source_ip = %addr.ip(),
                    changed_knob = %knob,
                    "CRUD rejected: TOML has unrelated changes since gateway start; refusing to roll back"
                ),
                _ => warn!(
                    event = "crud_rejected",
                    reason = "ambient_drift",
                    resource = resource,
                    site = %site,
                    source_ip = %addr.ip(),
                    changed_knob = %knob,
                    "CRUD rejected: TOML has unrelated changes since gateway start; refusing to roll back"
                ),
            }
            (
                false,
                (
                    StatusCode::CONFLICT,
                    Json(ErrorResponse::with_hint(
                        format!(
                            "your TOML has unrelated changes to {knob} since gateway start; review/restart the gateway before retrying"
                        ),
                        "the in-process Arc<AppConfig> is still on the pre-drift values; restart will pick up your TOML edit",
                    )),
                )
                    .into_response(),
            )
        }
        Ok(true) | Err(_) => {
            // Our delta caused the RestartRequired (defence-in-depth)
            // OR drift check failed; fall back to standard 500 +
            // rollback.
            (
                true,
                reload_error_response(
                    ReloadError::RestartRequired {
                        knob: knob.to_string(),
                    },
                    site,
                    addr,
                    resource,
                ),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memory::InMemoryBackend;
    use crate::storage::types::ChirpstackStatus;
    use crate::storage::StorageBackend;
    use crate::utils::OpcGwError;
    use crate::web::auth::WebAuthState;
    use crate::web::{ApplicationSummary, DashboardConfigSnapshot};
    use chrono::Utc;
    use std::sync::Arc;
    use std::time::Instant;
    use tracing_test::traced_test;

    /// Minimal `AppState` builder for the API tests. Backend is an
    /// `InMemoryBackend` populated as the test demands; snapshot is
    /// hand-built so the test can pin specific application/device
    /// counts without going through `AppConfig`.
    ///
    /// Review iter-1 B10: signature takes the per-application device
    /// counts explicitly so summary `device_count` matches the
    /// claimed total. Previous `(application_count, device_count)`
    /// shape integer-divided, producing `(2, 7) → 3 devs/app * 2 = 6`
    /// — a silent off-by-one that would mask Story 9-3 bugs once a
    /// handler reads `applications[*].device_count`.
    fn build_state(
        backend: Arc<dyn StorageBackend>,
        per_app_device_counts: &[usize],
    ) -> Arc<AppState> {
        let auth = Arc::new(WebAuthState::new_with_fresh_key(
            "u",
            "p",
            "opcgw-test".to_string(),
        ));
        let applications: Vec<ApplicationSummary> = per_app_device_counts
            .iter()
            .enumerate()
            .map(|(i, &dc)| {
                // Story 9-3: per-app DeviceSummary list. Test fixtures
                // don't need real `metrics` (Vec<MetricSpec>) — empty
                // vecs are sufficient for the /api/status tests (which
                // only read application_count + device_count);
                // /api/devices tests build their own state with
                // populated devices.
                let devices = (0..dc)
                    .map(|j| crate::web::DeviceSummary {
                        device_id: format!("dev-{i}-{j}"),
                        device_name: format!("Dev {i}-{j}"),
                        metrics: vec![],
                    })
                    .collect();
                ApplicationSummary {
                    application_id: format!("app-{i}"),
                    application_name: format!("App {i}"),
                    device_count: dc,
                    devices,
                }
            })
            .collect();
        let snapshot = Arc::new(DashboardConfigSnapshot {
            application_count: applications.len(),
            device_count: applications.iter().map(|a| a.device_count).sum(),
            applications,
        });
        // Story 9-4: minimal ConfigReloadHandle + ConfigWriter to
        // satisfy AppState's new fields. The api_status / api_devices
        // tests don't exercise CRUD paths, but they need these
        // fields to be present for AppState to construct.
        let (config_reload, config_writer, _keep_tempdir) =
            crate::web::test_support::make_test_reload_handle_and_writer();
        let st = Arc::new(AppState {
            auth,
            backend,
            dashboard_snapshot: std::sync::RwLock::new(snapshot),
            start_time: Instant::now(),
            stale_threshold_secs: std::sync::atomic::AtomicU64::new(DEFAULT_STALE_THRESHOLD_SECS),
            config_reload,
            config_writer,
        });
        // Keep the tempdir alive for the AppState's lifetime by
        // leaking it — tests are short-lived processes.
        std::mem::forget(_keep_tempdir);
        st
    }

    /// Failing backend used by the 500-path tests. Returns
    /// `Err(OpcGwError::Storage)` from EXACTLY two methods:
    ///   - `get_gateway_health_metrics` (used by the api_status 500 test)
    ///   - `load_all_metrics` (used by the api_devices 500 test —
    ///     Story 9-3 extension)
    ///
    /// Every other `StorageBackend` method `panic!()`s with a clear
    /// message naming the unreachable contract — if a future api_*
    /// handler accidentally calls one of those methods on this fake,
    /// the test fails loudly instead of returning a misleading
    /// `Err`.
    ///
    /// **Story 9-3 review iter-1 M3 rename:** was `FailingBackend`;
    /// renamed to `FailingBackendForApiTests` to make the scope
    /// explicit. Future handlers that need a different failure
    /// pattern should add their own fake (e.g. `FailingBackendForCommandTests`)
    /// rather than overloading this one.
    struct FailingBackendForApiTests;

    impl StorageBackend for FailingBackendForApiTests {
        fn get_metric(
            &self,
            _device_id: &str,
            _metric_name: &str,
        ) -> Result<Option<crate::storage::MetricType>, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn get_metric_value(
            &self,
            _device_id: &str,
            _metric_name: &str,
        ) -> Result<Option<crate::storage::MetricValue>, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn set_metric(
            &self,
            _device_id: &str,
            _metric_name: &str,
            _value: crate::storage::MetricType,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn get_status(&self) -> Result<ChirpstackStatus, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn update_status(&self, _status: ChirpstackStatus) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn queue_command(
            &self,
            _command: crate::storage::DeviceCommand,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn get_pending_commands(
            &self,
        ) -> Result<Vec<crate::storage::DeviceCommand>, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn update_command_status(
            &self,
            _command_id: u64,
            _status: crate::storage::CommandStatus,
            _error_message: Option<String>,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn upsert_metric_value(
            &self,
            _device_id: &str,
            _metric_name: &str,
            _value: &crate::storage::MetricType,
            _now_ts: std::time::SystemTime,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn append_metric_history(
            &self,
            _device_id: &str,
            _metric_name: &str,
            _value: &crate::storage::MetricType,
            _timestamp: std::time::SystemTime,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn batch_write_metrics(
            &self,
            _metrics: Vec<crate::storage::BatchMetricWrite>,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn load_all_metrics(&self) -> Result<Vec<crate::storage::MetricValue>, OpcGwError> {
            // Story 9-3: synthetic failure for the api_devices 500-path
            // unit test. Mirrors the get_gateway_health_metrics shape.
            Err(OpcGwError::Storage(
                "synthetic failure for the api_devices 500 unit test".to_string(),
            ))
        }
        fn prune_metric_history(&self) -> Result<u32, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn query_metric_history(
            &self,
            _device_id: &str,
            _metric_name: &str,
            _start: std::time::SystemTime,
            _end: std::time::SystemTime,
            _max_results: usize,
        ) -> Result<Vec<crate::storage::HistoricalMetricRow>, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn enqueue_command(
            &self,
            _command: crate::storage::Command,
        ) -> Result<u64, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn dequeue_command(&self) -> Result<Option<crate::storage::Command>, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn list_commands(
            &self,
            _filter: &crate::storage::CommandFilter,
        ) -> Result<Vec<crate::storage::Command>, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn get_queue_depth(&self) -> Result<usize, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn mark_command_sent(
            &self,
            _command_id: u64,
            _chirpstack_result_id: &str,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn mark_command_confirmed(&self, _command_id: u64) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn mark_command_failed(
            &self,
            _command_id: u64,
            _error_message: &str,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn find_pending_confirmations(
            &self,
        ) -> Result<Vec<crate::storage::Command>, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn find_timed_out_commands(
            &self,
            _ttl_secs: u32,
        ) -> Result<Vec<crate::storage::Command>, OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn update_gateway_status(
            &self,
            _last_poll_timestamp: Option<chrono::DateTime<chrono::Utc>>,
            _error_count: i32,
            _chirpstack_available: bool,
        ) -> Result<(), OpcGwError> {
            panic!("FailingBackendForApiTests: this method is unreachable from api_status / api_devices; if a future test path reaches it, either return Err for an intentional failure-path test OR rename this fake to something more specific")
        }
        fn get_gateway_health_metrics(
            &self,
        ) -> Result<(Option<chrono::DateTime<chrono::Utc>>, i32, bool), OpcGwError> {
            Err(OpcGwError::Storage(
                "synthetic failure for the 500 unit test".to_string(),
            ))
        }
    }

    /// Story 9-2 AC#2: success path returns 200 + JSON populated from
    /// `get_gateway_health_metrics` + the frozen snapshot.
    #[tokio::test]
    async fn api_status_returns_200_with_all_fields_when_storage_healthy() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let now = Utc::now();
        backend
            .update_gateway_status(Some(now), 7, true)
            .expect("seed gateway_status");
        let state = build_state(backend, &[3, 3]);

        let response = api_status(State(state.clone())).await;
        let json = response.expect("expected Ok with StatusResponse").0;
        assert!(json.chirpstack_available);
        assert!(json.last_poll_time.is_some());
        assert_eq!(json.error_count, 7);
        assert_eq!(json.application_count, 2);
        assert_eq!(json.device_count, 6);
        // uptime_secs: just-built state. Review iter-1 E9: relax the
        // upper bound from 1 to 5 to absorb slow CI runners (valgrind,
        // contended runners, etc.) without flaking. The point of the
        // assertion is "the field reflects elapsed wall-clock since
        // build_state ran" — a 5 s budget still catches the pathological
        // case where uptime is nonsensically large.
        assert!(json.uptime_secs <= 5);
    }

    /// Story 9-2 AC#2: storage failure returns 500 + generic body.
    /// **Critical NFR7 invariant**: the inner error string must NOT
    /// leak into the response body.
    #[tokio::test]
    async fn api_status_returns_500_with_generic_body_when_storage_errors() {
        let backend: Arc<dyn StorageBackend> = Arc::new(FailingBackendForApiTests);
        let state = build_state(backend, &[]);

        let response = api_status(State(state)).await;
        let err = response.expect_err("expected Err with 500 response");
        // Read out the response shape: status code first.
        let (parts, body) = err.into_parts();
        assert_eq!(parts.status, StatusCode::INTERNAL_SERVER_ERROR);
        // Drain body and verify it contains exactly the generic message —
        // no SQLite path, no table name, no inner-error fragment.
        let bytes = axum::body::to_bytes(body, 4096)
            .await
            .expect("collect body");
        let text = String::from_utf8(bytes.to_vec()).expect("utf-8 body");
        assert!(
            text.contains("internal server error"),
            "expected generic message, got: {text}"
        );
        assert!(
            !text.contains("synthetic failure"),
            "inner error must not leak into the response body, got: {text}"
        );
    }

    /// Story 9-2 AC#2: first-startup default (no poll yet) — the
    /// storage layer returns `(None, 0, false)` per
    /// `src/storage/mod.rs:721-724` semantics. The dashboard distinguishes
    /// "never polled" via `last_poll_time: null`.
    #[tokio::test]
    async fn api_status_returns_chirpstack_unavailable_first_startup() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let state = build_state(backend, &[]);

        let response = api_status(State(state)).await;
        let json = response.expect("expected Ok").0;
        assert!(!json.chirpstack_available);
        assert_eq!(json.last_poll_time, None);
        assert_eq!(json.error_count, 0);
    }

    /// Story 9-2 AC#2: explicit serde-shape pin — `None` for
    /// `last_poll_time` must serialise as JSON `null`, not as the
    /// string `"null"` and not as an absent field. The dashboard
    /// branches on `(value === null)`.
    #[tokio::test]
    async fn api_status_serialises_last_poll_time_as_null_when_none() {
        let response = StatusResponse {
            chirpstack_available: false,
            last_poll_time: None,
            error_count: 0,
            application_count: 0,
            device_count: 0,
            uptime_secs: 0,
        };
        let json = serde_json::to_value(&response).expect("serialise StatusResponse");
        assert!(json["last_poll_time"].is_null());
    }

    // =====================================================================
    // Story 9-3 (FR37) — /api/devices unit tests.
    // =====================================================================

    /// Build an `AppState` with a hand-built `applications` list so the
    /// test can pin specific (device_id, metric_name, configured_type)
    /// triples without going through `AppConfig`. Snapshot
    /// `application_count` / `device_count` are derived from
    /// `applications.len()` / sum-of-`devices.len()` to keep them in
    /// sync.
    fn build_state_for_devices(
        backend: Arc<dyn StorageBackend>,
        applications: Vec<crate::web::ApplicationSummary>,
    ) -> Arc<AppState> {
        let auth = Arc::new(WebAuthState::new_with_fresh_key(
            "u",
            "p",
            "opcgw-test".to_string(),
        ));
        let application_count = applications.len();
        let device_count = applications.iter().map(|a| a.device_count).sum();
        let snapshot = Arc::new(crate::web::DashboardConfigSnapshot {
            application_count,
            device_count,
            applications,
        });
        // Story 9-4: minimal ConfigReloadHandle + ConfigWriter to
        // satisfy AppState's new fields. The api_status / api_devices
        // tests don't exercise CRUD paths, but they need these
        // fields to be present for AppState to construct.
        let (config_reload, config_writer, _keep_tempdir) =
            crate::web::test_support::make_test_reload_handle_and_writer();
        let st = Arc::new(AppState {
            auth,
            backend,
            dashboard_snapshot: std::sync::RwLock::new(snapshot),
            start_time: Instant::now(),
            stale_threshold_secs: std::sync::atomic::AtomicU64::new(DEFAULT_STALE_THRESHOLD_SECS),
            config_reload,
            config_writer,
        });
        // Keep the tempdir alive for the AppState's lifetime by
        // leaking it — tests are short-lived processes.
        std::mem::forget(_keep_tempdir);
        st
    }

    fn make_dev(
        id: &str,
        name: &str,
        metrics: &[(&str, OpcMetricTypeConfig)],
    ) -> crate::web::DeviceSummary {
        crate::web::DeviceSummary {
            device_id: id.to_string(),
            device_name: name.to_string(),
            metrics: metrics
                .iter()
                .map(|(n, t)| crate::web::MetricSpec {
                    metric_name: n.to_string(),
                    metric_type: t.clone(),
                })
                .collect(),
        }
    }

    /// Story 9-3 AC#2: success path returns 200 + JSON walks the
    /// snapshot's application/device order, joining metric_values rows
    /// against the configured metric names. Asserts: top-level fields
    /// present, `applications` array shape mirrors the snapshot, the
    /// seeded metric appears with its real value/timestamp/type.
    #[tokio::test]
    async fn api_devices_returns_200_with_application_grouped_grid_when_storage_healthy() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        // Seed one metric: device "d1", metric "temperature", value
        // 23.5, type Float — matches what the snapshot configures.
        let now = std::time::SystemTime::now();
        backend
            .upsert_metric_value("d1", "temperature", &crate::storage::MetricType::Float, now)
            .expect("seed metric");

        let app = crate::web::ApplicationSummary {
            application_id: "app-1".to_string(),
            application_name: "Sensors".to_string(),
            device_count: 1,
            devices: vec![make_dev(
                "d1",
                "Device 1",
                &[("temperature", OpcMetricTypeConfig::Float)],
            )],
        };
        let state = build_state_for_devices(backend, vec![app]);

        let response = api_devices(State(state)).await;
        let json = response.expect("expected Ok").0;

        // Top-level fields.
        assert_eq!(json.stale_threshold_secs, 120);
        assert_eq!(json.bad_threshold_secs, 86_400);
        assert!(!json.as_of.is_empty(), "as_of must be RFC 3339");

        // Snapshot shape preserved.
        assert_eq!(json.applications.len(), 1);
        assert_eq!(json.applications[0].application_id, "app-1");
        assert_eq!(json.applications[0].devices.len(), 1);
        assert_eq!(json.applications[0].devices[0].device_id, "d1");
        assert_eq!(json.applications[0].devices[0].metrics.len(), 1);

        let m = &json.applications[0].devices[0].metrics[0];
        assert_eq!(m.metric_name, "temperature");
        assert_eq!(m.data_type, "Float");
        assert!(m.value.is_some(), "seeded metric must have a value");
        assert!(m.timestamp.is_some(), "seeded metric must have a timestamp");
    }

    /// Story 9-3 AC#2: storage failure returns 500 + generic body.
    /// **Critical NFR7 invariant**: the inner error string must NOT
    /// leak into the response body.
    #[tokio::test]
    async fn api_devices_returns_500_with_generic_body_when_storage_errors() {
        let backend: Arc<dyn StorageBackend> = Arc::new(FailingBackendForApiTests);
        let state = build_state_for_devices(backend, vec![]);

        let response = api_devices(State(state)).await;
        let err = response.expect_err("expected Err with 500 response");
        let (parts, body) = err.into_parts();
        assert_eq!(parts.status, StatusCode::INTERNAL_SERVER_ERROR);
        let bytes = axum::body::to_bytes(body, 4096)
            .await
            .expect("collect body");
        let text = String::from_utf8(bytes.to_vec()).expect("utf-8 body");
        assert!(
            text.contains("internal server error"),
            "expected generic message, got: {text}"
        );
        assert!(
            !text.contains("synthetic failure"),
            "inner error must not leak into the response body, got: {text}"
        );
    }

    /// Story 9-3 AC#2: a configured metric with no row in
    /// metric_values renders as `value: null, timestamp: null`. The
    /// data_type field still carries the configured type so the
    /// dashboard can display "(Int) — never reported" rather than "(?)".
    #[tokio::test]
    async fn api_devices_returns_null_value_for_unpolled_metric() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        // Don't seed any metrics. Configure 1 device with 2 metrics
        // and assert both come back as null.
        let app = crate::web::ApplicationSummary {
            application_id: "app-1".to_string(),
            application_name: "Sensors".to_string(),
            device_count: 1,
            devices: vec![make_dev(
                "d1",
                "Device 1",
                &[
                    ("temperature", OpcMetricTypeConfig::Float),
                    ("humidity", OpcMetricTypeConfig::Int),
                ],
            )],
        };
        let state = build_state_for_devices(backend, vec![app]);

        let response = api_devices(State(state)).await;
        let json = response.expect("expected Ok").0;

        let metrics = &json.applications[0].devices[0].metrics;
        assert_eq!(metrics.len(), 2);
        assert!(metrics[0].value.is_none(), "temperature must be null");
        assert!(metrics[0].timestamp.is_none());
        assert_eq!(metrics[0].data_type, "Float", "configured type wins on missing row");
        assert!(metrics[1].value.is_none(), "humidity must be null");
        assert!(metrics[1].timestamp.is_none());
        assert_eq!(metrics[1].data_type, "Int");
    }

    /// Story 9-3 AC#2: when the storage row's data_type differs from
    /// the configured type (poller-side type drift), the storage row
    /// wins so the dashboard surfaces the actual stored type. The
    /// configured type is the fallback only when no row exists.
    #[tokio::test]
    async fn api_devices_uses_storage_data_type_when_present_and_configured_data_type_when_missing(
    ) {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        // Seed a Float row for a metric that's CONFIGURED as Int.
        let now = std::time::SystemTime::now();
        backend
            .upsert_metric_value(
                "d1",
                "drifted",
                &crate::storage::MetricType::Float,
                now,
            )
            .expect("seed Float row for an Int-configured metric");

        let app = crate::web::ApplicationSummary {
            application_id: "app-1".to_string(),
            application_name: "Sensors".to_string(),
            device_count: 1,
            devices: vec![make_dev(
                "d1",
                "Device 1",
                &[
                    ("drifted", OpcMetricTypeConfig::Int),
                    ("absent", OpcMetricTypeConfig::Bool),
                ],
            )],
        };
        let state = build_state_for_devices(backend, vec![app]);

        let response = api_devices(State(state)).await;
        let json = response.expect("expected Ok").0;
        let metrics = &json.applications[0].devices[0].metrics;
        assert_eq!(metrics.len(), 2);
        // Storage row's Float wins over configured Int.
        assert_eq!(metrics[0].data_type, "Float", "storage data_type wins when row exists");
        assert!(metrics[0].value.is_some());
        // No row for "absent" — configured Bool surfaces.
        assert_eq!(metrics[1].data_type, "Bool", "configured data_type wins when row is missing");
        assert!(metrics[1].value.is_none());
    }

    /// Iter-1 review L11 (Auditor A8): Spec Task 3 mandated unit test.
    /// Defends against the obvious copy-paste regression class where
    /// `validate_path_device_id` accidentally emits the
    /// `application_crud_rejected` event instead of the
    /// `device_crud_rejected` event — exactly the regression that
    /// surfaced as iter-1 HIGH H1 for the rejection helpers.
    ///
    /// Iter-2 review L5: anchor on the QUOTED token `event="device_crud_rejected"`
    /// (with surrounding tracing-format quotes) so a future field
    /// like `device_crud_rejected_count=N` cannot match by accident.
    #[test]
    #[traced_test]
    fn validate_path_device_id_with_crlf_emits_device_event() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("parse addr");
        let result = validate_path_device_id("dev\nid", &addr);
        assert!(result.is_err(), "CRLF in device_id must be rejected");
        assert!(
            logs_contain("event=\"device_crud_rejected\""),
            "validate_path_device_id must emit event=\"device_crud_rejected\" (quoted) on validation rejection"
        );
        assert!(
            !logs_contain("event=\"application_crud_rejected\""),
            "validate_path_device_id must NOT emit event=\"application_crud_rejected\" (Story 9-5 path-aware dispatch)"
        );
    }

    /// Iter-2 review M5: parallel CRLF unit test for the H1 patch's
    /// path-aware dispatch in `validate_path_application_id`. When
    /// the helper is called from a device handler with
    /// `resource = "device"`, a CRLF-bearing application_id MUST
    /// emit `device_crud_rejected` (NOT `application_crud_rejected`).
    /// Defends against the swap-the-resource-literal regression class.
    #[test]
    #[traced_test]
    fn validate_path_application_id_with_crlf_under_device_resource_emits_device_event() {
        let addr: SocketAddr = "127.0.0.1:0".parse().expect("parse addr");
        let result = validate_path_application_id("app\nid", &addr, "device");
        assert!(result.is_err(), "CRLF in application_id must be rejected");
        assert!(
            logs_contain("event=\"device_crud_rejected\""),
            "validate_path_application_id with resource=\"device\" must emit device_crud_rejected"
        );
        assert!(
            !logs_contain("event=\"application_crud_rejected\""),
            "must NOT emit application_crud_rejected when invoked from a device handler"
        );
    }
}

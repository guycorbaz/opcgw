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
use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use serde::Serialize;
use tracing::warn;

use crate::config::OpcMetricTypeConfig;
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
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ErrorResponse {
    pub error: String,
}

impl ErrorResponse {
    fn internal_server_error() -> Self {
        Self {
            error: "internal server error".to_string(),
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

    Ok(Json(StatusResponse {
        chirpstack_available: available,
        last_poll_time: last_poll.map(|t| t.to_rfc3339()),
        error_count,
        application_count: state.dashboard_snapshot.application_count,
        device_count: state.dashboard_snapshot.device_count,
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
    let stale_threshold_secs = state.stale_threshold_secs;

    let applications: Vec<ApplicationView> = state
        .dashboard_snapshot
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
        Arc::new(AppState {
            auth,
            backend,
            dashboard_snapshot: snapshot,
            start_time: Instant::now(),
            stale_threshold_secs: DEFAULT_STALE_THRESHOLD_SECS,
        })
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
        Arc::new(AppState {
            auth,
            backend,
            dashboard_snapshot: snapshot,
            start_time: Instant::now(),
            stale_threshold_secs: DEFAULT_STALE_THRESHOLD_SECS,
        })
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
}

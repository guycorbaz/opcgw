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

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use tracing::warn;

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
    fn build_state(
        backend: Arc<dyn StorageBackend>,
        application_count: usize,
        device_count: usize,
    ) -> Arc<AppState> {
        let auth = Arc::new(WebAuthState::new_with_fresh_key(
            "u",
            "p",
            "opcgw-test".to_string(),
        ));
        let snapshot = Arc::new(DashboardConfigSnapshot {
            application_count,
            device_count,
            applications: (0..application_count)
                .map(|i| ApplicationSummary {
                    application_id: format!("app-{i}"),
                    application_name: format!("App {i}"),
                    device_count: device_count / application_count.max(1),
                })
                .collect(),
        });
        Arc::new(AppState {
            auth,
            backend,
            dashboard_snapshot: snapshot,
            start_time: Instant::now(),
        })
    }

    /// Failing backend used by the 500-path test. Returns
    /// `OpcGwError::Storage` from `get_gateway_health_metrics` and
    /// no-ops everything else (the API handler only calls one method).
    struct FailingBackend;

    impl StorageBackend for FailingBackend {
        fn get_metric(
            &self,
            _device_id: &str,
            _metric_name: &str,
        ) -> Result<Option<crate::storage::MetricType>, OpcGwError> {
            unreachable!("api_status only calls get_gateway_health_metrics")
        }
        fn get_metric_value(
            &self,
            _device_id: &str,
            _metric_name: &str,
        ) -> Result<Option<crate::storage::MetricValue>, OpcGwError> {
            unreachable!()
        }
        fn set_metric(
            &self,
            _device_id: &str,
            _metric_name: &str,
            _value: crate::storage::MetricType,
        ) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn get_status(&self) -> Result<ChirpstackStatus, OpcGwError> {
            unreachable!()
        }
        fn update_status(&self, _status: ChirpstackStatus) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn queue_command(
            &self,
            _command: crate::storage::DeviceCommand,
        ) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn get_pending_commands(
            &self,
        ) -> Result<Vec<crate::storage::DeviceCommand>, OpcGwError> {
            unreachable!()
        }
        fn update_command_status(
            &self,
            _command_id: u64,
            _status: crate::storage::CommandStatus,
            _error_message: Option<String>,
        ) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn upsert_metric_value(
            &self,
            _device_id: &str,
            _metric_name: &str,
            _value: &crate::storage::MetricType,
            _now_ts: std::time::SystemTime,
        ) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn append_metric_history(
            &self,
            _device_id: &str,
            _metric_name: &str,
            _value: &crate::storage::MetricType,
            _timestamp: std::time::SystemTime,
        ) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn batch_write_metrics(
            &self,
            _metrics: Vec<crate::storage::BatchMetricWrite>,
        ) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn load_all_metrics(&self) -> Result<Vec<crate::storage::MetricValue>, OpcGwError> {
            unreachable!()
        }
        fn prune_metric_history(&self) -> Result<u32, OpcGwError> {
            unreachable!()
        }
        fn query_metric_history(
            &self,
            _device_id: &str,
            _metric_name: &str,
            _start: std::time::SystemTime,
            _end: std::time::SystemTime,
            _max_results: usize,
        ) -> Result<Vec<crate::storage::HistoricalMetricRow>, OpcGwError> {
            unreachable!()
        }
        fn enqueue_command(
            &self,
            _command: crate::storage::Command,
        ) -> Result<u64, OpcGwError> {
            unreachable!()
        }
        fn dequeue_command(&self) -> Result<Option<crate::storage::Command>, OpcGwError> {
            unreachable!()
        }
        fn list_commands(
            &self,
            _filter: &crate::storage::CommandFilter,
        ) -> Result<Vec<crate::storage::Command>, OpcGwError> {
            unreachable!()
        }
        fn get_queue_depth(&self) -> Result<usize, OpcGwError> {
            unreachable!()
        }
        fn mark_command_sent(
            &self,
            _command_id: u64,
            _chirpstack_result_id: &str,
        ) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn mark_command_confirmed(&self, _command_id: u64) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn mark_command_failed(
            &self,
            _command_id: u64,
            _error_message: &str,
        ) -> Result<(), OpcGwError> {
            unreachable!()
        }
        fn find_pending_confirmations(
            &self,
        ) -> Result<Vec<crate::storage::Command>, OpcGwError> {
            unreachable!()
        }
        fn find_timed_out_commands(
            &self,
            _ttl_secs: u32,
        ) -> Result<Vec<crate::storage::Command>, OpcGwError> {
            unreachable!()
        }
        fn update_gateway_status(
            &self,
            _last_poll_timestamp: Option<chrono::DateTime<chrono::Utc>>,
            _error_count: i32,
            _chirpstack_available: bool,
        ) -> Result<(), OpcGwError> {
            unreachable!()
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
        let state = build_state(backend, 2, 6);

        let response = api_status(State(state.clone())).await;
        let json = response.expect("expected Ok with StatusResponse").0;
        assert!(json.chirpstack_available);
        assert!(json.last_poll_time.is_some());
        assert_eq!(json.error_count, 7);
        assert_eq!(json.application_count, 2);
        assert_eq!(json.device_count, 6);
        // uptime_secs: just-built state, elapsed should be 0 or 1.
        assert!(json.uptime_secs <= 1);
    }

    /// Story 9-2 AC#2: storage failure returns 500 + generic body.
    /// **Critical NFR7 invariant**: the inner error string must NOT
    /// leak into the response body.
    #[tokio::test]
    async fn api_status_returns_500_with_generic_body_when_storage_errors() {
        let backend: Arc<dyn StorageBackend> = Arc::new(FailingBackend);
        let state = build_state(backend, 0, 0);

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
        let state = build_state(backend, 0, 0);

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
}

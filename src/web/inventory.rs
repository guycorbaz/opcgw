// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story C-1 / G-1: web handlers for the `/api/inventory/*` endpoints.
//!
//! Four GET-only handlers:
//! - `inventory_applications`  (`GET /api/inventory/applications`)
//! - `inventory_devices`       (`GET /api/inventory/devices?application_id=…`)
//! - `inventory_uplinks`       (`GET /api/inventory/uplinks?dev_eui=…&limit=…`)
//! - `inventory_measurements`  (`GET /api/inventory/measurements?dev_eui=…`, Story G-1)
//!
//! All are basic-auth gated (same middleware stack as the rest of
//! `/api/*`) and CSRF-exempt (GET-only, read-only — matches the existing
//! API convention).
//!
//! Cache + ChirpStack-side machinery lives in
//! [`crate::chirpstack_inventory`]. This module is the web-layer wrapper:
//! query-parameter parsing + response shape + audit events.

use crate::chirpstack_inventory::{
    compute_observed_keys, fetch_applications, fetch_device_profile_measurements, fetch_devices,
    stream_recent_device_uplinks, CacheStatus, DeviceProfileMeasurements, InventoryApplication,
    InventoryDevice, InventoryUplink, ProfileMeasurement,
};
use crate::web::AppState;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Shared response envelope for applications.
#[derive(Debug, Serialize)]
pub struct InventoryApplicationsResponse {
    pub items: Vec<InventoryApplication>,
    pub count: usize,
    pub cache_status: &'static str,
    pub fetched_at: String,
}

/// Shared response envelope for devices.
#[derive(Debug, Serialize)]
pub struct InventoryDevicesResponse {
    pub items: Vec<InventoryDevice>,
    pub count: usize,
    pub cache_status: &'static str,
    pub fetched_at: String,
    pub application_id: String,
}

/// One entry in the `observed_keys` aggregate.
#[derive(Debug, Serialize)]
pub struct ObservedKeyResponse {
    pub key: String,
    pub wire_type: &'static str,
    pub sample_value: serde_json::Value,
}

/// Shared response envelope for uplinks.
#[derive(Debug, Serialize)]
pub struct InventoryUplinksResponse {
    pub items: Vec<InventoryUplink>,
    pub count: usize,
    pub observed_keys: Vec<ObservedKeyResponse>,
    pub dev_eui: String,
    pub fetched_at: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct ApplicationsQuery {
    /// `?refresh=true` forces a cache bypass (C-4 drift view uses this).
    #[serde(default)]
    pub refresh: Option<String>,
}

impl ApplicationsQuery {
    fn force_refresh(&self) -> bool {
        matches!(self.refresh.as_deref(), Some("true") | Some("1"))
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct DevicesQuery {
    pub application_id: Option<String>,
    #[serde(default)]
    pub refresh: Option<String>,
}

impl DevicesQuery {
    fn force_refresh(&self) -> bool {
        matches!(self.refresh.as_deref(), Some("true") | Some("1"))
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct UplinksQuery {
    pub dev_eui: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Story G-1: response envelope for device-profile measurements.
#[derive(Debug, Serialize)]
pub struct InventoryMeasurementsResponse {
    pub items: Vec<ProfileMeasurement>,
    pub count: usize,
    pub cache_status: &'static str,
    pub fetched_at: String,
    pub dev_eui: String,
    pub device_profile_id: String,
}

#[derive(Debug, Deserialize, Default)]
pub struct MeasurementsQuery {
    pub dev_eui: Option<String>,
    #[serde(default)]
    pub refresh: Option<String>,
}

impl MeasurementsQuery {
    fn force_refresh(&self) -> bool {
        matches!(self.refresh.as_deref(), Some("true") | Some("1"))
    }
}

/// Maximum allowed `?limit` value for `/api/inventory/uplinks` (AC#3).
pub const UPLINKS_LIMIT_CAP: u32 = 50;
/// Default `?limit` when the operator doesn't specify one (AC#3).
pub const UPLINKS_LIMIT_DEFAULT: u32 = 10;

/// Normalise a DevEUI to lowercase 16-hex-char form.
///
/// Accepts input with or without colons/dashes. Returns `None` if the
/// input doesn't reduce to exactly 16 hex chars after stripping
/// separators.
fn normalise_dev_eui(input: &str) -> Option<String> {
    let stripped: String = input
        .chars()
        .filter(|c| *c != ':' && *c != '-' && *c != ' ')
        .collect();
    if stripped.len() != 16 || !stripped.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    Some(stripped.to_ascii_lowercase())
}

/// GET `/api/inventory/applications` — list applications for the
/// configured tenant.
pub async fn inventory_applications(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ApplicationsQuery>,
) -> Response {
    let started = std::time::Instant::now();
    let force_refresh = query.force_refresh();

    // Read the live config so any hot-reloaded fields (e.g. inventory
    // upper bound) are picked up per-request.
    let config = state.config_reload.subscribe().borrow().clone();
    let tenant_id = config.chirpstack.tenant_id.clone();

    // Cache lookup.
    let cancel_token = state.shutdown_token.clone();
    let cfg_clone = config.clone();
    let result = state
        .inventory_cache
        .get_or_fetch_applications(&tenant_id, force_refresh, || async move {
            let raw = fetch_applications(&cfg_clone, &cancel_token).await?;
            // Map ApplicationDetail → InventoryApplication + sort by name.
            let mut items: Vec<InventoryApplication> =
                raw.into_iter().map(Into::into).collect();
            items.sort_by_key(|a| a.name.to_lowercase());
            Ok(items)
        })
        .await;

    match result {
        Ok(cache_result) => {
            // Cache MISSES + refreshes + bypassed reads emit audit events.
            // HITs are silent (AC#10 — bounds log volume).
            if cache_result.cache_status != CacheStatus::Hit {
                info!(
                    event = "inventory_query",
                    resource = "applications",
                    cache_status = cache_result.cache_status.as_str(),
                    tenant_id = %tenant_id,
                    response_status = 200,
                    chirpstack_response = "ok",
                    item_count = cache_result.value.len(),
                    duration_ms = started.elapsed().as_millis() as u64,
                    "inventory_applications: ChirpStack fetch completed"
                );
            }
            let count = cache_result.value.len();
            (
                StatusCode::OK,
                Json(InventoryApplicationsResponse {
                    items: cache_result.value,
                    count,
                    cache_status: cache_result.cache_status.as_str(),
                    fetched_at: cache_result.fetched_at,
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!(
                event = "inventory_query_failed",
                resource = "applications",
                reason = chirpstack_failure_reason(&e),
                tenant_id = %tenant_id,
                error = %e,
                duration_ms = started.elapsed().as_millis() as u64,
                "inventory_applications: ChirpStack fetch failed"
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "chirpstack_error", "reason": chirpstack_failure_reason(&e)})),
            )
                .into_response()
        }
    }
}

/// GET `/api/inventory/devices?application_id=…` — list devices under
/// the given application.
pub async fn inventory_devices(
    State(state): State<Arc<AppState>>,
    Query(query): Query<DevicesQuery>,
) -> Response {
    let started = std::time::Instant::now();
    let force_refresh = query.force_refresh();

    let application_id = match query.application_id {
        Some(s) if !s.is_empty() => s,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing_query_param", "param": "application_id"})),
            )
                .into_response();
        }
    };

    let config = state.config_reload.subscribe().borrow().clone();
    let tenant_id = config.chirpstack.tenant_id.clone();
    let cancel_token = state.shutdown_token.clone();
    let cfg_clone = config.clone();
    let app_id_for_fetch = application_id.clone();

    let result = state
        .inventory_cache
        .get_or_fetch_devices(&tenant_id, &application_id, force_refresh, || async move {
            let raw = fetch_devices(&cfg_clone, &app_id_for_fetch, &cancel_token).await?;
            let mut items: Vec<InventoryDevice> =
                raw.into_iter().map(Into::into).collect();
            items.sort_by_key(|a| a.name.to_lowercase());
            Ok(items)
        })
        .await;

    match result {
        Ok(cache_result) => {
            let chirpstack_response = if cache_result.value.is_empty() {
                "empty"
            } else {
                "ok"
            };
            if cache_result.cache_status != CacheStatus::Hit {
                info!(
                    event = "inventory_query",
                    resource = "devices",
                    cache_status = cache_result.cache_status.as_str(),
                    tenant_id = %tenant_id,
                    application_id = %application_id,
                    response_status = 200,
                    chirpstack_response = chirpstack_response,
                    item_count = cache_result.value.len(),
                    duration_ms = started.elapsed().as_millis() as u64,
                    "inventory_devices: ChirpStack fetch completed"
                );
            }
            let count = cache_result.value.len();
            (
                StatusCode::OK,
                Json(InventoryDevicesResponse {
                    items: cache_result.value,
                    count,
                    cache_status: cache_result.cache_status.as_str(),
                    fetched_at: cache_result.fetched_at,
                    application_id,
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!(
                event = "inventory_query_failed",
                resource = "devices",
                reason = chirpstack_failure_reason(&e),
                tenant_id = %tenant_id,
                application_id = %application_id,
                error = %e,
                duration_ms = started.elapsed().as_millis() as u64,
                "inventory_devices: ChirpStack fetch failed"
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "chirpstack_error", "reason": chirpstack_failure_reason(&e)})),
            )
                .into_response()
        }
    }
}

/// GET `/api/inventory/uplinks?dev_eui=…&limit=…` — read recent uplinks
/// via the `InternalService.StreamDeviceEvents` stream and aggregate
/// observed keys for wire-type inference.
pub async fn inventory_uplinks(
    State(state): State<Arc<AppState>>,
    Query(query): Query<UplinksQuery>,
) -> Response {
    let started = std::time::Instant::now();

    let dev_eui_raw = match query.dev_eui {
        Some(s) if !s.is_empty() => s,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing_query_param", "param": "dev_eui"})),
            )
                .into_response();
        }
    };
    let dev_eui = match normalise_dev_eui(&dev_eui_raw) {
        Some(d) => d,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_dev_eui",
                    "hint": "DevEUI must be 16 hex characters (colons / dashes accepted as separators)"
                })),
            )
                .into_response();
        }
    };

    let limit = query.limit.unwrap_or(UPLINKS_LIMIT_DEFAULT);
    if limit > UPLINKS_LIMIT_CAP {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": "limit_out_of_range",
                "cap": UPLINKS_LIMIT_CAP,
                "received": limit
            })),
        )
            .into_response();
    }

    let config = state.config_reload.subscribe().borrow().clone();
    let tenant_id = config.chirpstack.tenant_id.clone();
    let max_wait = Duration::from_secs(config.chirpstack.inventory_uplink_max_wait_seconds);

    let uplinks_result = stream_recent_device_uplinks(
        &config.chirpstack.server_address,
        &config.chirpstack.api_token,
        &dev_eui,
        limit,
        max_wait,
    )
    .await;

    match uplinks_result {
        Ok(uplinks) => {
            let (observed, heterogeneous) = compute_observed_keys(&uplinks);

            // Emit warn events for any heterogeneous keys (AC#13).
            for het in &heterogeneous {
                warn!(
                    event = "inventory_observed_key_heterogeneous",
                    dev_eui = %dev_eui,
                    key = %het.key,
                    types_seen = %het.types_seen,
                    "inventory_uplinks: heterogeneous key inferred as String fallback"
                );
            }

            // Uplinks are uncached — every request is a fresh ChirpStack
            // call, so the audit event always fires (NOT gated on cache
            // miss like applications/devices).
            let chirpstack_response = if uplinks.is_empty() { "empty" } else { "ok" };
            info!(
                event = "inventory_query",
                resource = "uplinks",
                cache_status = "bypassed",
                tenant_id = %tenant_id,
                dev_eui = %dev_eui,
                response_status = 200,
                chirpstack_response = chirpstack_response,
                item_count = uplinks.len(),
                duration_ms = started.elapsed().as_millis() as u64,
                "inventory_uplinks: ChirpStack stream completed"
            );

            let observed_response: Vec<ObservedKeyResponse> = observed
                .into_iter()
                .map(|k| ObservedKeyResponse {
                    key: k.key,
                    wire_type: k.wire_type.as_str(),
                    sample_value: k.sample_value,
                })
                .collect();
            let count = uplinks.len();
            let fetched_at = chrono::Utc::now().to_rfc3339();
            (
                StatusCode::OK,
                Json(InventoryUplinksResponse {
                    items: uplinks,
                    count,
                    observed_keys: observed_response,
                    dev_eui,
                    fetched_at,
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!(
                event = "inventory_query_failed",
                resource = "uplinks",
                reason = chirpstack_failure_reason(&e),
                tenant_id = %tenant_id,
                dev_eui = %dev_eui,
                error = %e,
                duration_ms = started.elapsed().as_millis() as u64,
                "inventory_uplinks: ChirpStack stream failed"
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "chirpstack_error", "reason": chirpstack_failure_reason(&e)})),
            )
                .into_response()
        }
    }
}

/// GET `/api/inventory/measurements?dev_eui=…&refresh=…` — list the
/// measurements declared in the device's ChirpStack device profile
/// (Story G-1, issue #124).
///
/// Mirrors `inventory_devices`: `dev_eui` validation, TTL cache (keyed by
/// `(tenant, dev_eui)`) with `?refresh=true` bypass, audit event, and a
/// `502` + structured-error degraded path the picker UI turns into a
/// fallback banner (never a hard error).
pub async fn inventory_measurements(
    State(state): State<Arc<AppState>>,
    Query(query): Query<MeasurementsQuery>,
) -> Response {
    let started = std::time::Instant::now();
    let force_refresh = query.force_refresh();

    let dev_eui_raw = match query.dev_eui {
        Some(s) if !s.is_empty() => s,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({"error": "missing_query_param", "param": "dev_eui"})),
            )
                .into_response();
        }
    };
    let dev_eui = match normalise_dev_eui(&dev_eui_raw) {
        Some(d) => d,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({
                    "error": "invalid_dev_eui",
                    "hint": "DevEUI must be 16 hex characters (colons / dashes accepted as separators)"
                })),
            )
                .into_response();
        }
    };

    let config = state.config_reload.subscribe().borrow().clone();
    let tenant_id = config.chirpstack.tenant_id.clone();
    let cancel_token = state.shutdown_token.clone();
    let cfg_clone = config.clone();
    let dev_eui_for_fetch = dev_eui.clone();

    let result = state
        .inventory_cache
        .get_or_fetch_measurements(&tenant_id, &dev_eui, force_refresh, || async move {
            fetch_device_profile_measurements(&cfg_clone, &dev_eui_for_fetch, &cancel_token).await
        })
        .await;

    match result {
        Ok(cache_result) => {
            let chirpstack_response = if cache_result.value.measurements.is_empty() {
                "empty"
            } else {
                "ok"
            };
            if cache_result.cache_status != CacheStatus::Hit {
                info!(
                    event = "inventory_query",
                    resource = "measurements",
                    cache_status = cache_result.cache_status.as_str(),
                    tenant_id = %tenant_id,
                    dev_eui = %dev_eui,
                    device_profile_id = %cache_result.value.device_profile_id,
                    response_status = 200,
                    chirpstack_response = chirpstack_response,
                    item_count = cache_result.value.measurements.len(),
                    duration_ms = started.elapsed().as_millis() as u64,
                    "inventory_measurements: ChirpStack fetch completed"
                );
            }
            let DeviceProfileMeasurements {
                device_profile_id,
                measurements,
            } = cache_result.value;
            let count = measurements.len();
            (
                StatusCode::OK,
                Json(InventoryMeasurementsResponse {
                    items: measurements,
                    count,
                    cache_status: cache_result.cache_status.as_str(),
                    fetched_at: cache_result.fetched_at,
                    dev_eui,
                    device_profile_id,
                }),
            )
                .into_response()
        }
        Err(e) => {
            warn!(
                event = "inventory_query_failed",
                resource = "measurements",
                reason = chirpstack_failure_reason(&e),
                tenant_id = %tenant_id,
                dev_eui = %dev_eui,
                error = %e,
                duration_ms = started.elapsed().as_millis() as u64,
                "inventory_measurements: ChirpStack fetch failed"
            );
            (
                StatusCode::BAD_GATEWAY,
                Json(json!({"error": "chirpstack_error", "reason": chirpstack_failure_reason(&e)})),
            )
                .into_response()
        }
    }
}

/// Classify an `OpcGwError` into a stable `reason` string for the
/// `inventory_query_failed` audit event + JSON error body.
///
/// Story C-1 iter-2 P2 fix (2-of-3 reviewer convergence): the iter-1
/// P5 patch made `fetch_applications` / `fetch_devices` return
/// `Err(OpcGwError::ChirpStack("cancelled during shutdown"))` on cancel
/// — but this classifier's substring matchers (`auth` / `connect` /
/// `unreachable` / `transport` / `permission` / `unauthenticated`)
/// matched NONE of "cancelled", so the fall-through bucket was
/// `chirpstack_grpc_error`. Every graceful shutdown that raced a
/// picker request emitted a false-alarm "ChirpStack gRPC error"
/// audit signal. Iter-2 adds a dedicated `shutdown_cancellation`
/// reason classified BEFORE the others (any error message containing
/// "cancelled" wins, regardless of subsequent substring matches).
fn chirpstack_failure_reason(err: &crate::utils::OpcGwError) -> &'static str {
    use crate::utils::OpcGwError;
    let s = err.to_string().to_lowercase();
    match err {
        OpcGwError::ChirpStack(_) => {
            // Iter-2 P2 / iter-3 P1: shutdown-cancellation must win over
            // the substring matchers below. Iter-3 P1 fix (Edge MED):
            // match the LOCAL cancellation strings only, NOT the bare
            // substring "cancelled". Pre-iter-3, `s.contains("cancelled")`
            // also matched tonic's `Status::Cancelled` stringification
            // (`"status: 'Cancelled', message: ..."` lowercased contains
            // "cancelled") — server-initiated ChirpStack cancels (HTTP/2
            // RST_STREAM, propagated deadline) were misclassified as
            // shutdown_cancellation and the docs-row instructed
            // operators to "suppress alerts on this reason during
            // planned restarts," actively masking real faults. Match
            // only the strings we construct ourselves in
            // `chirpstack_inventory.rs::fetch_*`.
            if s.contains("cancelled during shutdown")
                || s.contains("cancelled mid-pagination during shutdown")
            {
                "shutdown_cancellation"
            } else if s.contains("auth") || s.contains("permission") || s.contains("unauthenticated") {
                "chirpstack_auth_failed"
            } else if s.contains("connect") || s.contains("transport") || s.contains("unreachable") {
                "chirpstack_unreachable"
            } else {
                "chirpstack_grpc_error"
            }
        }
        _ => "chirpstack_grpc_error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalise_dev_eui_accepts_lowercase_hex() {
        assert_eq!(
            normalise_dev_eui("a84041b8a1867e20"),
            Some("a84041b8a1867e20".to_string())
        );
    }

    #[test]
    fn normalise_dev_eui_accepts_uppercase_hex() {
        assert_eq!(
            normalise_dev_eui("A84041B8A1867E20"),
            Some("a84041b8a1867e20".to_string())
        );
    }

    #[test]
    fn normalise_dev_eui_strips_colons() {
        assert_eq!(
            normalise_dev_eui("a8:40:41:b8:a1:86:7e:20"),
            Some("a84041b8a1867e20".to_string())
        );
    }

    #[test]
    fn normalise_dev_eui_strips_dashes() {
        assert_eq!(
            normalise_dev_eui("a8-40-41-b8-a1-86-7e-20"),
            Some("a84041b8a1867e20".to_string())
        );
    }

    #[test]
    fn normalise_dev_eui_rejects_wrong_length() {
        assert_eq!(normalise_dev_eui("a84041b8"), None);
        assert_eq!(normalise_dev_eui("a84041b8a1867e2012"), None);
    }

    #[test]
    fn normalise_dev_eui_rejects_non_hex() {
        assert_eq!(normalise_dev_eui("zzzz041b8a1867e20"), None);
    }

    #[test]
    fn applications_query_force_refresh_recognises_true_variants() {
        let q = ApplicationsQuery {
            refresh: Some("true".to_string()),
        };
        assert!(q.force_refresh());
        let q = ApplicationsQuery {
            refresh: Some("1".to_string()),
        };
        assert!(q.force_refresh());
        let q = ApplicationsQuery {
            refresh: Some("foo".to_string()),
        };
        // Per AC#8: invalid ?refresh values treated as not-set, NOT a 400.
        assert!(!q.force_refresh());
    }

    #[test]
    fn applications_query_force_refresh_defaults_false() {
        let q = ApplicationsQuery::default();
        assert!(!q.force_refresh());
    }

    // Story G-1 review (auditor LOW): mirror the ApplicationsQuery
    // coverage for the new MeasurementsQuery.force_refresh().
    #[test]
    fn measurements_query_force_refresh_recognises_true_variants() {
        let q = MeasurementsQuery {
            dev_eui: Some("a84041b8a1867e20".to_string()),
            refresh: Some("true".to_string()),
        };
        assert!(q.force_refresh());
        let q = MeasurementsQuery {
            dev_eui: None,
            refresh: Some("1".to_string()),
        };
        assert!(q.force_refresh());
        let q = MeasurementsQuery {
            dev_eui: None,
            refresh: Some("foo".to_string()),
        };
        assert!(!q.force_refresh());
    }

    #[test]
    fn measurements_query_force_refresh_defaults_false() {
        let q = MeasurementsQuery::default();
        assert!(!q.force_refresh());
    }

    // -------------------------------------------------------------------
    // Iter-3 P3 regression-guards for `chirpstack_failure_reason` (Edge LOW)
    // -------------------------------------------------------------------

    use crate::utils::OpcGwError;

    /// Iter-3 P1 regression-guard: the LOCAL cancellation strings emitted
    /// by `chirpstack_inventory::fetch_*` must classify as
    /// `shutdown_cancellation`.
    #[test]
    fn chirpstack_failure_reason_classifies_local_cancellation_as_shutdown() {
        let local_entry = OpcGwError::ChirpStack("cancelled during shutdown".into());
        assert_eq!(chirpstack_failure_reason(&local_entry), "shutdown_cancellation");
        let local_loop = OpcGwError::ChirpStack(
            "cancelled mid-pagination during shutdown".into(),
        );
        assert_eq!(chirpstack_failure_reason(&local_loop), "shutdown_cancellation");
    }

    /// Iter-3 P1 regression-guard: tonic's `Status::Cancelled` stringifies
    /// to something like `"... status: 'Cancelled', message: ..."`, lowercased
    /// contains the bare substring "cancelled" — but THAT must NOT classify
    /// as shutdown_cancellation. Pre-iter-3, the matcher used
    /// `s.contains("cancelled")` and false-positived on this case, hiding
    /// real ChirpStack faults. Post-iter-3 it must fall through to the
    /// generic gRPC error bucket.
    #[test]
    fn chirpstack_failure_reason_does_not_false_positive_on_tonic_cancelled() {
        // Mimic tonic's Status::Cancelled stringification shape.
        let tonic_cancel = OpcGwError::ChirpStack(
            "list_applications gRPC error: status: 'Cancelled', message: \"call cancelled by server\"".into(),
        );
        assert_ne!(
            chirpstack_failure_reason(&tonic_cancel),
            "shutdown_cancellation",
            "bare substring \"cancelled\" must NOT classify as shutdown_cancellation"
        );
        // Should fall through to the generic gRPC error bucket.
        assert_eq!(chirpstack_failure_reason(&tonic_cancel), "chirpstack_grpc_error");
    }

    /// Iter-3 P1 regression-guard: the auth / unreachable / fallback
    /// branches must still classify correctly.
    #[test]
    fn chirpstack_failure_reason_classifies_auth_unreachable_grpc() {
        let auth = OpcGwError::ChirpStack(
            "list_applications gRPC error: auth failed".into(),
        );
        assert_eq!(chirpstack_failure_reason(&auth), "chirpstack_auth_failed");

        let unreachable = OpcGwError::ChirpStack("connect failed: refused".into());
        assert_eq!(chirpstack_failure_reason(&unreachable), "chirpstack_unreachable");

        let generic = OpcGwError::ChirpStack("internal: code 13".into());
        assert_eq!(chirpstack_failure_reason(&generic), "chirpstack_grpc_error");
    }
}


// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story C-1: ChirpStack inventory query layer.
//!
//! This module provides the substrate for Epic C's picker UX (Story C-2)
//! and drift view (Story C-4) by exposing three things:
//!
//! 1. **Lean inventory types** (`InventoryApplication`, `InventoryDevice`,
//!    `InventoryUplink`) that map from the existing
//!    [`crate::chirpstack::ApplicationDetail`] / [`crate::chirpstack::DeviceListDetail`]
//!    structs plus the proto-generated `LogItem`/`StreamDeviceEventsRequest`
//!    types from [`crate::chirpstack_internal_proto::api`].
//!
//! 2. **Server-side TTL cache** (`InventoryCache`) keyed on `(tenant_id)`
//!    for applications and `(tenant_id, application_id)` for devices. Uplinks
//!    are uncached (freshness-sensitive). Race-free fetch-and-insert via a
//!    `tokio::sync::Mutex` per scope. Default TTL 60 s.
//!
//! 3. **`stream_recent_device_uplinks` helper** that opens the
//!    `InternalService.StreamDeviceEvents` gRPC stream with a bounded read
//!    window (default 5 s), reads up to `limit` events, filters for
//!    description == `"up"`, parses the JSON body, and returns the
//!    collected uplinks sorted newest-first.

use crate::chirpstack::{ApplicationDetail, DeviceListDetail};
use crate::chirpstack_internal_proto::api::internal_service_client::InternalServiceClient;
use crate::chirpstack_internal_proto::api::{LogItem, StreamDeviceEventsRequest};
use crate::config::AppConfig;
use crate::utils::OpcGwError;
use chirpstack_api::api::application_service_client::ApplicationServiceClient;
use chirpstack_api::api::device_service_client::DeviceServiceClient;
use chirpstack_api::api::{
    ListApplicationsRequest, ListDevicesRequest,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Channel;
use tonic::{Request, Status};
use tracing::{debug, trace};

// ---------------------------------------------------------------------------
// Inventory types — the JSON-serialisable shapes the web layer returns.
// ---------------------------------------------------------------------------

/// Lean application shape for the picker UI.
///
/// Derived from [`ApplicationDetail`] via the `From` impl below.
#[derive(Debug, Clone, Serialize)]
pub struct InventoryApplication {
    pub id: String,
    pub name: String,
    pub description: String,
}

impl From<ApplicationDetail> for InventoryApplication {
    fn from(a: ApplicationDetail) -> Self {
        Self {
            id: a.application_id,
            name: a.application_name,
            description: a.application_description,
        }
    }
}

/// Lean device shape for the picker UI.
///
/// Derived from [`DeviceListDetail`] (which iter-C-1 extended with
/// `device_profile_name` and `last_seen_at` fields).
#[derive(Debug, Clone, Serialize)]
pub struct InventoryDevice {
    pub dev_eui: String,
    pub name: String,
    pub description: String,
    pub device_profile_name: Option<String>,
    pub last_seen_at: Option<String>,
}

impl From<DeviceListDetail> for InventoryDevice {
    fn from(d: DeviceListDetail) -> Self {
        Self {
            dev_eui: d.dev_eui,
            name: d.name,
            description: d.description,
            device_profile_name: d.device_profile_name,
            last_seen_at: d.last_seen_at,
        }
    }
}

/// One uplink as observed via `InternalService.StreamDeviceEvents`.
///
/// `decoded_object` is the JSON value emitted by the device's codec —
/// typically a flat object with sensor key/value pairs. The wire-type
/// inference (`compute_observed_keys`) walks across N uplinks to infer
/// a per-key type.
#[derive(Debug, Clone, Serialize)]
pub struct InventoryUplink {
    pub received_at: String,
    pub decoded_object: serde_json::Value,
    pub f_port: Option<u32>,
    pub f_cnt: Option<u32>,
}

// ---------------------------------------------------------------------------
// Cache — TTL + race-free fetch-and-insert per scope.
// ---------------------------------------------------------------------------

/// Stored cache value with its fetch timestamp.
#[derive(Debug, Clone)]
struct CacheEntry<T> {
    value: T,
    fetched_at: Instant,
    /// RFC3339 representation of `fetched_at` for the JSON `fetched_at` field.
    /// Computed once at fetch time so handlers don't redo the conversion per
    /// request.
    fetched_at_rfc3339: String,
}

impl<T> CacheEntry<T> {
    fn new(value: T) -> Self {
        let now = Instant::now();
        let rfc3339 = Utc::now().to_rfc3339();
        Self {
            value,
            fetched_at: now,
            fetched_at_rfc3339: rfc3339,
        }
    }

    fn is_fresh(&self, ttl: Duration) -> bool {
        // TTL = 0 disables the cache → never fresh.
        !ttl.is_zero() && self.fetched_at.elapsed() < ttl
    }
}

/// `cache_status` field emitted with every inventory response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CacheStatus {
    Hit,
    Miss,
    Refresh,
    Bypassed,
}

impl CacheStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss => "miss",
            Self::Refresh => "refresh",
            Self::Bypassed => "bypassed",
        }
    }
}

/// Output of a cache lookup. Carries the value, the cache status, and the
/// fetch timestamp string for the response body.
#[derive(Debug, Clone)]
pub struct CacheResult<T> {
    pub value: T,
    pub cache_status: CacheStatus,
    pub fetched_at: String,
}

/// Server-side TTL cache for inventory queries.
///
/// Keyed on `(tenant_id)` for applications and `(tenant_id, application_id)`
/// for devices. Each scope gets its own `Mutex<HashMap<...>>` so two
/// concurrent requests on the SAME key + an expired entry coalesce into a
/// single ChirpStack call (the second caller awaits the first's result via
/// the lock).
///
/// Memory bounded by `tenant_count × application_count` entries (at most a
/// few hundred ever for a typical opcgw deployment). No eviction policy
/// needed at this scale — entries are overwritten on refresh, otherwise
/// linger forever.
type ApplicationsCacheMap = HashMap<String, CacheEntry<Vec<InventoryApplication>>>;
type DevicesCacheMap = HashMap<(String, String), CacheEntry<Vec<InventoryDevice>>>;

pub struct InventoryCache {
    applications: Mutex<ApplicationsCacheMap>,
    devices: Mutex<DevicesCacheMap>,
    ttl: Duration,
}

impl InventoryCache {
    pub fn new(ttl_seconds: u64) -> Self {
        Self {
            applications: Mutex::new(HashMap::new()),
            devices: Mutex::new(HashMap::new()),
            ttl: Duration::from_secs(ttl_seconds),
        }
    }

    /// Get applications for the given tenant, fetching via the closure on
    /// cache miss / expired entry / forced refresh.
    ///
    /// Race-free: the mutex is held across the fetch closure call so two
    /// concurrent callers on the same expired entry produce ONE ChirpStack
    /// call (the second awaits the first's completed insert).
    ///
    /// `force_refresh` forces a fresh fetch regardless of TTL.
    pub async fn get_or_fetch_applications<F, Fut>(
        &self,
        tenant_id: &str,
        force_refresh: bool,
        fetch: F,
    ) -> Result<CacheResult<Vec<InventoryApplication>>, OpcGwError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<InventoryApplication>, OpcGwError>>,
    {
        let mut guard = self.applications.lock().await;

        // TTL = 0 → cache disabled, every call hits ChirpStack.
        if self.ttl.is_zero() {
            let fresh = fetch().await?;
            let entry = CacheEntry::new(fresh.clone());
            let rfc = entry.fetched_at_rfc3339.clone();
            guard.insert(tenant_id.to_string(), entry);
            return Ok(CacheResult {
                value: fresh,
                cache_status: CacheStatus::Bypassed,
                fetched_at: rfc,
            });
        }

        if !force_refresh {
            if let Some(entry) = guard.get(tenant_id) {
                if entry.is_fresh(self.ttl) {
                    return Ok(CacheResult {
                        value: entry.value.clone(),
                        cache_status: CacheStatus::Hit,
                        fetched_at: entry.fetched_at_rfc3339.clone(),
                    });
                }
            }
        }

        // Miss or refresh — call the fetch closure under the lock so
        // concurrent callers on the same key coalesce.
        let fresh = fetch().await?;
        let entry = CacheEntry::new(fresh.clone());
        let rfc = entry.fetched_at_rfc3339.clone();
        let cache_status = if force_refresh {
            CacheStatus::Refresh
        } else {
            CacheStatus::Miss
        };
        guard.insert(tenant_id.to_string(), entry);
        Ok(CacheResult {
            value: fresh,
            cache_status,
            fetched_at: rfc,
        })
    }

    /// Get devices for (tenant, application_id) with the same semantics as
    /// `get_or_fetch_applications`.
    pub async fn get_or_fetch_devices<F, Fut>(
        &self,
        tenant_id: &str,
        application_id: &str,
        force_refresh: bool,
        fetch: F,
    ) -> Result<CacheResult<Vec<InventoryDevice>>, OpcGwError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<Vec<InventoryDevice>, OpcGwError>>,
    {
        let key = (tenant_id.to_string(), application_id.to_string());
        let mut guard = self.devices.lock().await;

        if self.ttl.is_zero() {
            let fresh = fetch().await?;
            let entry = CacheEntry::new(fresh.clone());
            let rfc = entry.fetched_at_rfc3339.clone();
            guard.insert(key, entry);
            return Ok(CacheResult {
                value: fresh,
                cache_status: CacheStatus::Bypassed,
                fetched_at: rfc,
            });
        }

        if !force_refresh {
            if let Some(entry) = guard.get(&key) {
                if entry.is_fresh(self.ttl) {
                    return Ok(CacheResult {
                        value: entry.value.clone(),
                        cache_status: CacheStatus::Hit,
                        fetched_at: entry.fetched_at_rfc3339.clone(),
                    });
                }
            }
        }

        let fresh = fetch().await?;
        let entry = CacheEntry::new(fresh.clone());
        let rfc = entry.fetched_at_rfc3339.clone();
        let cache_status = if force_refresh {
            CacheStatus::Refresh
        } else {
            CacheStatus::Miss
        };
        guard.insert(key, entry);
        Ok(CacheResult {
            value: fresh,
            cache_status,
            fetched_at: rfc,
        })
    }

    /// Invalidate the applications-scope cache entry for the given tenant.
    ///
    /// Called from the CRUD success paths on `/api/applications` so the
    /// next inventory query forces a fresh fetch (avoiding stale picker UX
    /// after the operator just added/updated/deleted an application).
    pub async fn invalidate_applications(&self, tenant_id: &str) {
        let mut guard = self.applications.lock().await;
        guard.remove(tenant_id);
    }

    /// Invalidate the devices-scope cache entry for (tenant, application_id).
    ///
    /// Called from the CRUD success paths on `/api/devices` for the
    /// affected application's scope.
    pub async fn invalidate_devices(&self, tenant_id: &str, application_id: &str) {
        let mut guard = self.devices.lock().await;
        guard.remove(&(tenant_id.to_string(), application_id.to_string()));
    }
}

// ---------------------------------------------------------------------------
// stream_recent_device_uplinks — bounded-window InternalService stream read
// ---------------------------------------------------------------------------

/// Minimal tonic interceptor that attaches a bearer token to every request.
/// Mirrors the existing pattern in `src/chirpstack.rs` (see `ApiTokenInterceptor`).
#[derive(Clone)]
struct BearerInterceptor {
    token: String,
}

impl Interceptor for BearerInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        let value = MetadataValue::try_from(format!("Bearer {}", self.token))
            .map_err(|_| Status::invalid_argument("invalid api token"))?;
        request.metadata_mut().insert("authorization", value);
        Ok(request)
    }
}

/// Open `InternalService.StreamDeviceEvents` for `dev_eui`, read up to
/// `limit` items or until `max_wait` elapses (whichever comes first),
/// filter for description == `"up"`, parse the JSON `body` field, and
/// return the collected uplinks sorted newest-first.
///
/// On timeout with zero uplinks collected: returns `Ok(vec![])` (the
/// "no recent uplinks" state — NOT an error).
///
/// Authentication: re-uses the operator's ChirpStack API token via a
/// bearer-token interceptor (same pattern as `ChirpstackPoller`).
pub async fn stream_recent_device_uplinks(
    server_address: &str,
    api_token: &str,
    dev_eui: &str,
    limit: u32,
    max_wait: Duration,
) -> Result<Vec<InventoryUplink>, OpcGwError> {
    if limit == 0 {
        // Degenerate but legal — operator explicitly asked for zero uplinks.
        return Ok(Vec::new());
    }

    // Establish the gRPC channel. ChirpStack's gRPC endpoint follows the
    // existing convention: server_address like `127.0.0.1:8080` becomes
    // `http://127.0.0.1:8080` for tonic.
    let endpoint = if server_address.starts_with("http://")
        || server_address.starts_with("https://")
    {
        server_address.to_string()
    } else {
        format!("http://{}", server_address)
    };
    let channel = Channel::from_shared(endpoint)
        .map_err(|e| OpcGwError::ChirpStack(format!("invalid server_address: {}", e)))?
        .connect()
        .await
        .map_err(|e| OpcGwError::ChirpStack(format!("connect failed: {}", e)))?;

    let interceptor = BearerInterceptor {
        token: api_token.to_string(),
    };
    let mut client = InternalServiceClient::with_interceptor(channel, interceptor);

    let request = Request::new(StreamDeviceEventsRequest {
        dev_eui: dev_eui.to_string(),
    });

    let response = client
        .stream_device_events(request)
        .await
        .map_err(|e| OpcGwError::ChirpStack(format!("stream_device_events: {}", e)))?;
    let mut stream = response.into_inner();

    let mut uplinks: Vec<InventoryUplink> = Vec::new();

    // Bounded read loop: tokio::time::timeout on the WHOLE collection,
    // not per-item, so a slow stream with sparse events still terminates
    // at `max_wait`.
    let collect = async {
        while uplinks.len() < limit as usize {
            match stream.message().await {
                Ok(Some(item)) => {
                    if let Some(uplink) = log_item_to_uplink(&item) {
                        uplinks.push(uplink);
                    }
                    // Non-uplink LogItems (join, ack, error, etc.) are
                    // silently skipped — they aren't relevant to the
                    // picker UI's "what keys does this device emit?"
                    // question.
                }
                Ok(None) => {
                    // Stream closed by ChirpStack — return what we have.
                    debug!(dev_eui = %dev_eui, collected = uplinks.len(), "InternalService stream closed");
                    break;
                }
                Err(e) => {
                    return Err(OpcGwError::ChirpStack(format!(
                        "stream item error: {}",
                        e
                    )));
                }
            }
        }
        Ok::<(), OpcGwError>(())
    };

    match tokio::time::timeout(max_wait, collect).await {
        Ok(Ok(())) => {
            // Either reached `limit` or stream closed.
        }
        Ok(Err(e)) => return Err(e),
        Err(_elapsed) => {
            // Timeout — not an error; return what we collected. The
            // empty-uplinks case is documented in AC#11 as a debug-level
            // log, not a warn/error.
            trace!(
                dev_eui = %dev_eui,
                collected = uplinks.len(),
                max_wait_ms = max_wait.as_millis() as u64,
                "stream_recent_device_uplinks: timeout reached"
            );
        }
    }

    // Sort newest first.
    uplinks.sort_by(|a, b| b.received_at.cmp(&a.received_at));
    Ok(uplinks)
}

/// Map a `LogItem` to an [`InventoryUplink`] iff its `description` field
/// matches the uplink discriminator and the `body` field parses as JSON
/// with the expected codec-output shape.
///
/// Returns `None` for any non-uplink LogItem (join, ack, error, status,
/// etc.) — the inventory layer only cares about uplinks for wire-type
/// inference. Returns `None` if `body` doesn't parse as JSON or doesn't
/// have the expected `object` field — defensive parsing keeps a single
/// malformed event from breaking the whole stream.
fn log_item_to_uplink(item: &LogItem) -> Option<InventoryUplink> {
    // ChirpStack v4 emits `description = "up"` for uplinks. Other values
    // (`"join"`, `"ack"`, `"error"`, `"status"`, `"txack"`, etc.) are
    // skipped.
    if item.description != "up" {
        return None;
    }

    let body: serde_json::Value = serde_json::from_str(&item.body).ok()?;
    // The codec output lives at `body.object` per ChirpStack v4's
    // application-server event schema. Defensive: if `object` is absent
    // or null, treat as empty (the uplink still counts; observed_keys
    // just won't contribute).
    let decoded_object = body
        .get("object")
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

    let f_port = body
        .get("fPort")
        .or_else(|| body.get("f_port"))
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok());
    let f_cnt = body
        .get("fCnt")
        .or_else(|| body.get("f_cnt"))
        .and_then(|v| v.as_u64())
        .and_then(|n| u32::try_from(n).ok());

    // Prefer the LogItem's `time` field over body.time — the proto carries
    // the canonical server-side timestamp.
    //
    // Iter-1 P14 fix (Edge MED): defensive validation of the proto
    // timestamp. Pre-fix: `ts.nanos as u32` for negative nanos wrapped to
    // a huge u32 (chrono returns `None`, falls through to `Utc::now()` →
    // synthesised "now" timestamp displaces real older uplinks at the top
    // of the sort). Also missing-timestamp items fell to `Utc::now()`
    // with the same effect.
    //
    // Per protobuf spec, `Timestamp.nanos` ∈ 0..1_000_000_000 and
    // `seconds` is non-negative for sensible wall-clock times. Reject
    // anything outside those bounds — return None to drop the LogItem
    // from the uplink list rather than synthesise a fake timestamp.
    let received_at = match item.time.as_ref() {
        Some(ts) if ts.nanos >= 0 && ts.nanos < 1_000_000_000 && ts.seconds >= 0 => {
            DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32)
                .map(|dt| dt.to_rfc3339())
        }
        _ => None,
    };
    let received_at = match received_at {
        Some(s) => s,
        None => {
            // Malformed proto — drop this LogItem rather than fabricate a
            // timestamp. The picker UI's "no recent uplinks" path handles
            // empty result sets cleanly.
            return None;
        }
    };

    Some(InventoryUplink {
        received_at,
        decoded_object,
        f_port,
        f_cnt,
    })
}

// ---------------------------------------------------------------------------
// Wire-type inference (AC#4).
// ---------------------------------------------------------------------------

/// Inferred wire type for a top-level key observed across recent uplinks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum WireType {
    Float,
    Int,
    Bool,
    String,
}

impl WireType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Float => "Float",
            Self::Int => "Int",
            Self::Bool => "Bool",
            Self::String => "String",
        }
    }
}

/// One observed top-level key from `decoded_object` across N uplinks.
#[derive(Debug, Clone, Serialize)]
pub struct ObservedKey {
    pub key: String,
    pub wire_type: WireType,
    pub sample_value: serde_json::Value,
}

/// Infer the wire type from a sequence of observed values for a single key.
///
/// Rules (AC#4):
/// - All bool → `Bool`
/// - All number, all mathematical integers, all fit in i64 → `Int`
/// - All number, at least one fractional or out-of-i64-range → `Float`
/// - All string → `String`
/// - Heterogeneous → `String` (and the caller emits the
///   `inventory_observed_key_heterogeneous` audit event)
/// - All `null` → `String` (conservative default; operator can override
///   in the picker UI)
///
/// Returns the inferred type AND a flag indicating whether the values were
/// heterogeneous (so the caller can emit the audit event).
pub fn infer_wire_type(values: &[&serde_json::Value]) -> (WireType, bool) {
    use serde_json::Value;

    // Filter out nulls — they don't count toward the inference per AC#4.
    let non_null: Vec<&Value> = values.iter().copied().filter(|v| !v.is_null()).collect();
    if non_null.is_empty() {
        return (WireType::String, false);
    }

    let all_bool = non_null.iter().all(|v| v.is_boolean());
    if all_bool {
        return (WireType::Bool, false);
    }

    let all_number = non_null.iter().all(|v| v.is_number());
    if all_number {
        let mut any_fractional_or_overflow = false;
        for v in &non_null {
            // Story C-1 iter-1 P3 fix (2-of-3 reviewer convergence on
            // i64::MAX boundary): pre-fix used `f <= i64::MAX as f64`
            // which is wrong — `i64::MAX as f64` rounds UP to `2^63`
            // (one past i64::MAX, since f64 only has 53 bits of
            // mantissa), so the JSON number `9223372036854775808.0`
            // (= 2^63 = i64::MAX + 1) satisfied the boundary check
            // and was classified as Int, then later overflowed any
            // i64 conversion downstream. Fix: use serde_json's own
            // `as_i64()` for the integer check — that one really
            // bounds at i64::MIN..=i64::MAX.
            if v.as_i64().is_some() {
                // serde_json parsed it as i64 → genuine integer in range.
                continue;
            }
            if let Some(f) = v.as_f64() {
                if f.fract() != 0.0 || f.is_nan() || f.is_infinite() {
                    any_fractional_or_overflow = true;
                    break;
                }
                // Not an i64 but is fractional-free f64 → out of i64 range.
                any_fractional_or_overflow = true;
                break;
            } else {
                // Number that isn't representable as f64 — defensive fallback.
                any_fractional_or_overflow = true;
                break;
            }
        }
        return if any_fractional_or_overflow {
            (WireType::Float, false)
        } else {
            (WireType::Int, false)
        };
    }

    let all_string = non_null.iter().all(|v| v.is_string());
    if all_string {
        return (WireType::String, false);
    }

    // Heterogeneous mix — fall back to String + flag for audit.
    (WireType::String, true)
}

/// Aggregate top-level keys across N uplinks, inferring wire type and
/// sample value per key.
///
/// The output is sorted by key name (deterministic for reproducible audit
/// trails). Heterogeneous keys are flagged in the returned `bool` so the
/// caller can emit `event="inventory_observed_key_heterogeneous"` audit
/// events.
pub fn compute_observed_keys(
    uplinks: &[InventoryUplink],
) -> (Vec<ObservedKey>, Vec<HeterogeneousKey>) {
    let mut by_key: HashMap<String, Vec<&serde_json::Value>> = HashMap::new();
    for uplink in uplinks {
        if let Some(obj) = uplink.decoded_object.as_object() {
            for (k, v) in obj {
                by_key.entry(k.clone()).or_default().push(v);
            }
        }
    }

    let mut observed = Vec::with_capacity(by_key.len());
    let mut heterogeneous = Vec::new();
    for (key, values) in by_key {
        let (wire_type, is_heterogeneous) = infer_wire_type(&values);
        let sample_value = values
            .iter()
            .find(|v| !v.is_null())
            .cloned()
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if is_heterogeneous {
            let types_seen = describe_value_types(&values);
            heterogeneous.push(HeterogeneousKey {
                key: key.clone(),
                types_seen,
            });
        }
        observed.push(ObservedKey {
            key,
            wire_type,
            sample_value,
        });
    }

    // Deterministic ordering by key name.
    observed.sort_by(|a, b| a.key.cmp(&b.key));
    heterogeneous.sort_by(|a, b| a.key.cmp(&b.key));
    (observed, heterogeneous)
}

/// Per-key heterogeneous-type report. Carried out of
/// [`compute_observed_keys`] so the caller can emit the audit event
/// outside the hot inference loop.
#[derive(Debug, Clone)]
pub struct HeterogeneousKey {
    pub key: String,
    pub types_seen: String,
}

fn describe_value_types(values: &[&serde_json::Value]) -> String {
    use serde_json::Value;
    let mut types: std::collections::BTreeSet<&'static str> = std::collections::BTreeSet::new();
    for v in values {
        let t = match v {
            Value::Null => "null",
            Value::Bool(_) => "bool",
            Value::Number(_) => "number",
            Value::String(_) => "string",
            Value::Array(_) => "array",
            Value::Object(_) => "object",
        };
        types.insert(t);
    }
    types.into_iter().collect::<Vec<_>>().join(",")
}

// ---------------------------------------------------------------------------
// AppState plumbing helper (Story 9-4-style — keeps web layer thin).
// ---------------------------------------------------------------------------

/// `Arc<InventoryCache>` shared between the web layer (read side) and the
/// CRUD handlers (invalidation side). Constructed in `main.rs` and cloned
/// into `AppState`.
pub type SharedInventoryCache = Arc<InventoryCache>;

// ---------------------------------------------------------------------------
// Standalone ChirpStack fetch helpers — open fresh clients per call so the
// web layer doesn't need to share Arc<ChirpstackPoller>. Duplicates a
// small amount of pagination logic from src/chirpstack.rs; the duplication
// is intentional (web layer's lifecycle is independent of the poller's run
// loop which uses &mut self).
// ---------------------------------------------------------------------------

// Iter-1 P6 fix (3-of-3 reviewer convergence): the second
// `InventoryBearerInterceptor` struct was a byte-for-byte duplicate of
// `BearerInterceptor` above. Dropped — both the stream helper and the
// fetch helpers use the same `BearerInterceptor` definition.

/// Build a tonic Channel against the configured ChirpStack endpoint.
async fn build_channel(server_address: &str) -> Result<Channel, OpcGwError> {
    let endpoint = if server_address.starts_with("http://")
        || server_address.starts_with("https://")
    {
        server_address.to_string()
    } else {
        format!("http://{}", server_address)
    };
    Channel::from_shared(endpoint)
        .map_err(|e| OpcGwError::ChirpStack(format!("invalid server_address: {}", e)))?
        .connect()
        .await
        .map_err(|e| OpcGwError::ChirpStack(format!("connect failed: {}", e)))
}

/// Fetch applications from ChirpStack with pagination unrolled (Story C-1).
///
/// Duplicates the page loop from `ChirpstackPoller::fetch_all_applications`
/// — the existing implementation requires `&self` on a poller instance,
/// which the web layer cannot easily borrow given the poller's run loop
/// uses `&mut self`. This standalone form keeps the web path independent.
pub async fn fetch_applications(
    config: &AppConfig,
    cancel_token: &CancellationToken,
) -> Result<Vec<ApplicationDetail>, OpcGwError> {
    if cancel_token.is_cancelled() {
        // Iter-1 P5 fix (Blind HIGH): return Err on cancellation
        // so the cache layer does NOT insert a poisoned empty Vec that
        // subsequent in-process requests would serve as "Hit".
        return Err(OpcGwError::ChirpStack(
            "cancelled during shutdown".to_string(),
        ));
    }
    let channel = build_channel(&config.chirpstack.server_address).await?;
    let interceptor = BearerInterceptor {
        token: config.chirpstack.api_token.clone(),
    };
    let client = ApplicationServiceClient::with_interceptor(channel, interceptor);

    let page_size = config.chirpstack.list_page_size;
    let mut all_applications: Vec<ApplicationDetail> = Vec::new();
    let mut offset = 0u32;
    let mut pages_fetched = 0u32;
    const MAX_PAGES: u32 = 10_000;

    loop {
        if cancel_token.is_cancelled() {
            // Iter-1 P5 fix: partial results from a mid-shutdown loop
            // would also poison the cache. Return Err instead.
            return Err(OpcGwError::ChirpStack(
                "cancelled mid-pagination during shutdown".to_string(),
            ));
        }
        if pages_fetched >= MAX_PAGES {
            return Err(OpcGwError::ChirpStack(
                "list_applications: pagination MAX_PAGES exceeded".to_string(),
            ));
        }
        pages_fetched += 1;
        let request = Request::new(ListApplicationsRequest {
            limit: page_size,
            offset,
            search: String::new(),
            tenant_id: config.chirpstack.tenant_id.clone(),
        });
        match client.clone().list(request).await {
            Ok(response) => {
                let inner = response.into_inner();
                let result_count = inner.result.len() as u32;
                for item in inner.result {
                    all_applications.push(ApplicationDetail {
                        application_id: item.id,
                        application_name: item.name,
                        application_description: item.description,
                    });
                }
                if result_count < page_size {
                    break;
                }
                offset = offset.saturating_add(page_size);
            }
            Err(e) => {
                return Err(OpcGwError::ChirpStack(format!(
                    "list_applications gRPC error: {}",
                    e
                )));
            }
        }
    }

    debug!(
        applications_count = all_applications.len(),
        pages = pages_fetched,
        "fetch_applications completed"
    );
    Ok(all_applications)
}

/// Fetch devices for a given application from ChirpStack with pagination.
///
/// Sibling to [`fetch_applications`] — see that function's doc comment for
/// the rationale on the duplication of poller-side logic.
pub async fn fetch_devices(
    config: &AppConfig,
    application_id: &str,
    cancel_token: &CancellationToken,
) -> Result<Vec<DeviceListDetail>, OpcGwError> {
    if cancel_token.is_cancelled() {
        // Iter-1 P5 fix (Blind HIGH): return Err on cancellation
        // so the cache layer does NOT insert a poisoned empty Vec that
        // subsequent in-process requests would serve as "Hit".
        return Err(OpcGwError::ChirpStack(
            "cancelled during shutdown".to_string(),
        ));
    }
    let channel = build_channel(&config.chirpstack.server_address).await?;
    let interceptor = BearerInterceptor {
        token: config.chirpstack.api_token.clone(),
    };
    let client = DeviceServiceClient::with_interceptor(channel, interceptor);

    let page_size = config.chirpstack.list_page_size;
    let mut all_devices: Vec<DeviceListDetail> = Vec::new();
    let mut offset = 0u32;
    let mut pages_fetched = 0u32;
    const MAX_PAGES: u32 = 10_000;

    loop {
        if cancel_token.is_cancelled() {
            // Iter-1 P5 fix: partial results from a mid-shutdown loop
            // would also poison the cache. Return Err instead.
            return Err(OpcGwError::ChirpStack(
                "cancelled mid-pagination during shutdown".to_string(),
            ));
        }
        if pages_fetched >= MAX_PAGES {
            return Err(OpcGwError::ChirpStack(
                "list_devices: pagination MAX_PAGES exceeded".to_string(),
            ));
        }
        pages_fetched += 1;
        let request = Request::new(ListDevicesRequest {
            limit: page_size,
            offset,
            search: String::new(),
            application_id: application_id.to_string(),
            multicast_group_id: String::new(),
            device_profile_id: String::new(),
            order_by: 0,
            order_by_desc: false,
            tags: HashMap::new(),
        });
        match client.clone().list(request).await {
            Ok(response) => {
                let inner = response.into_inner();
                let result_count = inner.result.len() as u32;
                for item in inner.result {
                    let device_profile_name = if item.device_profile_name.is_empty() {
                        None
                    } else {
                        Some(item.device_profile_name)
                    };
                    let last_seen_at = item.last_seen_at.as_ref().and_then(|ts| {
                        DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32)
                            .map(|dt| dt.to_rfc3339())
                    });
                    all_devices.push(DeviceListDetail {
                        dev_eui: item.dev_eui,
                        name: item.name,
                        description: item.description,
                        device_profile_name,
                        last_seen_at,
                    });
                }
                if result_count < page_size {
                    break;
                }
                offset = offset.saturating_add(page_size);
            }
            Err(e) => {
                return Err(OpcGwError::ChirpStack(format!(
                    "list_devices gRPC error: {}",
                    e
                )));
            }
        }
    }

    debug!(
        application_id = %application_id,
        devices_count = all_devices.len(),
        pages = pages_fetched,
        "fetch_devices completed"
    );
    Ok(all_devices)
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- inventory type conversions -----------------------------------

    #[test]
    fn inventory_application_from_application_detail() {
        let detail = ApplicationDetail {
            application_id: "app-123".to_string(),
            application_name: "Arrosage".to_string(),
            application_description: "Watering system".to_string(),
        };
        let inv: InventoryApplication = detail.into();
        assert_eq!(inv.id, "app-123");
        assert_eq!(inv.name, "Arrosage");
        assert_eq!(inv.description, "Watering system");
    }

    #[test]
    fn inventory_device_from_device_list_detail_preserves_extended_fields() {
        let detail = DeviceListDetail {
            dev_eui: "a84041b8a1867e20".to_string(),
            name: "WaterFlowSensor".to_string(),
            description: "Main valve".to_string(),
            device_profile_name: Some("Dragino LSE01".to_string()),
            last_seen_at: Some("2026-05-22T12:00:00Z".to_string()),
        };
        let inv: InventoryDevice = detail.into();
        assert_eq!(inv.dev_eui, "a84041b8a1867e20");
        assert_eq!(inv.name, "WaterFlowSensor");
        assert_eq!(inv.device_profile_name, Some("Dragino LSE01".to_string()));
        assert_eq!(inv.last_seen_at, Some("2026-05-22T12:00:00Z".to_string()));
    }

    #[test]
    fn inventory_device_handles_none_optional_fields() {
        let detail = DeviceListDetail {
            dev_eui: "1234567890abcdef".to_string(),
            name: "NewDevice".to_string(),
            description: String::new(),
            device_profile_name: None,
            last_seen_at: None,
        };
        let inv: InventoryDevice = detail.into();
        assert!(inv.device_profile_name.is_none());
        assert!(inv.last_seen_at.is_none());
    }

    // ---- cache TTL behavior -------------------------------------------

    #[tokio::test]
    async fn cache_miss_calls_fetch_once() {
        let cache = InventoryCache::new(60);
        let call_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let cc = call_count.clone();

        let result = cache
            .get_or_fetch_applications("tenant-1", false, || async move {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(vec![InventoryApplication {
                    id: "a1".to_string(),
                    name: "App 1".to_string(),
                    description: String::new(),
                }])
            })
            .await
            .expect("fetch succeeds");

        assert_eq!(result.cache_status, CacheStatus::Miss);
        assert_eq!(result.value.len(), 1);
        assert_eq!(call_count.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn cache_hit_does_not_call_fetch() {
        let cache = InventoryCache::new(60);

        // First call populates.
        cache
            .get_or_fetch_applications("tenant-1", false, || async {
                Ok(vec![InventoryApplication {
                    id: "a1".to_string(),
                    name: "App 1".to_string(),
                    description: String::new(),
                }])
            })
            .await
            .unwrap();

        // Second call should hit cache (fetch closure intentionally panics).
        let result = cache
            .get_or_fetch_applications("tenant-1", false, || async {
                panic!("fetch must NOT be called on cache hit");
            })
            .await
            .expect("hit succeeds");

        assert_eq!(result.cache_status, CacheStatus::Hit);
        assert_eq!(result.value.len(), 1);
    }

    #[tokio::test]
    async fn cache_refresh_forces_fetch() {
        let cache = InventoryCache::new(60);

        cache
            .get_or_fetch_applications("tenant-1", false, || async {
                Ok(vec![InventoryApplication {
                    id: "a1".to_string(),
                    name: "Old".to_string(),
                    description: String::new(),
                }])
            })
            .await
            .unwrap();

        // ?refresh=true forces a fresh fetch even within TTL.
        let result = cache
            .get_or_fetch_applications("tenant-1", true, || async {
                Ok(vec![InventoryApplication {
                    id: "a1".to_string(),
                    name: "New".to_string(),
                    description: String::new(),
                }])
            })
            .await
            .unwrap();

        assert_eq!(result.cache_status, CacheStatus::Refresh);
        assert_eq!(result.value[0].name, "New");
    }

    #[tokio::test]
    async fn cache_ttl_zero_marks_bypassed() {
        let cache = InventoryCache::new(0);
        let result = cache
            .get_or_fetch_applications("tenant-1", false, || async {
                Ok(vec![InventoryApplication {
                    id: "a1".to_string(),
                    name: "App 1".to_string(),
                    description: String::new(),
                }])
            })
            .await
            .unwrap();
        assert_eq!(result.cache_status, CacheStatus::Bypassed);
    }

    #[tokio::test]
    async fn cache_scope_is_per_tenant_application() {
        let cache = InventoryCache::new(60);

        // Populate (tenant-1, app-a)
        cache
            .get_or_fetch_devices("tenant-1", "app-a", false, || async {
                Ok(vec![InventoryDevice {
                    dev_eui: "aaa".to_string(),
                    name: "DevA".to_string(),
                    description: String::new(),
                    device_profile_name: None,
                    last_seen_at: None,
                }])
            })
            .await
            .unwrap();

        // (tenant-1, app-b) must be a separate miss — fetch closure
        // returns a different device.
        let result = cache
            .get_or_fetch_devices("tenant-1", "app-b", false, || async {
                Ok(vec![InventoryDevice {
                    dev_eui: "bbb".to_string(),
                    name: "DevB".to_string(),
                    description: String::new(),
                    device_profile_name: None,
                    last_seen_at: None,
                }])
            })
            .await
            .unwrap();

        assert_eq!(result.cache_status, CacheStatus::Miss);
        assert_eq!(result.value[0].dev_eui, "bbb");
    }

    #[tokio::test]
    async fn cache_invalidation_forces_next_miss() {
        let cache = InventoryCache::new(60);

        cache
            .get_or_fetch_applications("tenant-1", false, || async {
                Ok(vec![InventoryApplication {
                    id: "a1".to_string(),
                    name: "App 1".to_string(),
                    description: String::new(),
                }])
            })
            .await
            .unwrap();

        cache.invalidate_applications("tenant-1").await;

        let result = cache
            .get_or_fetch_applications("tenant-1", false, || async {
                Ok(vec![InventoryApplication {
                    id: "a2".to_string(),
                    name: "App 2".to_string(),
                    description: String::new(),
                }])
            })
            .await
            .unwrap();

        // Post-invalidation: next call MUST be a miss + new fetch.
        assert_eq!(result.cache_status, CacheStatus::Miss);
        assert_eq!(result.value[0].id, "a2");
    }

    // ---- wire-type inference ------------------------------------------

    #[test]
    fn infer_wire_type_all_bool() {
        let values: Vec<serde_json::Value> = vec![json!(true), json!(false), json!(true)];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, het) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::Bool);
        assert!(!het);
    }

    #[test]
    fn infer_wire_type_all_int() {
        let values: Vec<serde_json::Value> = vec![json!(1), json!(42), json!(-7)];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, het) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::Int);
        assert!(!het);
    }

    #[test]
    fn infer_wire_type_mixed_int_and_fractional_is_float() {
        let values: Vec<serde_json::Value> = vec![json!(1), json!(2.5), json!(3)];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, het) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::Float);
        assert!(!het);
    }

    #[test]
    fn infer_wire_type_all_fractional() {
        let values: Vec<serde_json::Value> = vec![json!(1.5), json!(2.5)];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, _) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::Float);
    }

    /// Iter-1 P3 fix: i64::MAX boundary. `i64::MAX as f64` rounds to
    /// `2^63` (one past i64::MAX), so a JSON number 2^63 was mis-
    /// classified as Int pre-fix. This test pins the post-fix behaviour:
    /// any number that doesn't fit in i64 (via serde_json::as_i64)
    /// falls through to Float.
    #[test]
    fn infer_wire_type_i64_max_plus_one_is_float() {
        // 2^63 = i64::MAX + 1, not representable as i64. serde_json parses
        // it as a float-ish big number; we expect Float classification.
        let huge: serde_json::Value =
            serde_json::from_str("9223372036854775808").expect("parse 2^63");
        let values = [huge];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, het) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::Float, "2^63 must NOT be classified as Int");
        assert!(!het);
    }

    /// i64::MAX itself MUST remain Int (boundary inclusive on the
    /// representable side). Pre-fix this was Int (correct); post-fix
    /// it's still Int because serde_json::as_i64 returns Some(i64::MAX).
    #[test]
    fn infer_wire_type_i64_max_exact_is_int() {
        let max: serde_json::Value = serde_json::from_str("9223372036854775807")
            .expect("parse i64::MAX");
        let values = [max];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, _) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::Int);
    }

    #[test]
    fn infer_wire_type_all_string() {
        let values: Vec<serde_json::Value> = vec![json!("a"), json!("b")];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, het) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::String);
        assert!(!het);
    }

    #[test]
    fn infer_wire_type_heterogeneous_flagged() {
        let values: Vec<serde_json::Value> = vec![json!(42), json!("text")];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, het) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::String);
        assert!(het, "mixed int+string must flag heterogeneous");
    }

    #[test]
    fn infer_wire_type_all_null_defaults_to_string() {
        let values: Vec<serde_json::Value> = vec![json!(null), json!(null)];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, het) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::String);
        assert!(!het);
    }

    #[test]
    fn infer_wire_type_skips_nulls_in_otherwise_int_set() {
        let values: Vec<serde_json::Value> = vec![json!(null), json!(42), json!(7)];
        let refs: Vec<&serde_json::Value> = values.iter().collect();
        let (wt, _) = infer_wire_type(&refs);
        assert_eq!(wt, WireType::Int);
    }

    // ---- compute_observed_keys aggregate ------------------------------

    #[test]
    fn compute_observed_keys_aggregates_across_uplinks() {
        let uplinks = vec![
            InventoryUplink {
                received_at: "2026-05-22T12:00:00Z".to_string(),
                decoded_object: json!({"temperature": 22.5, "battery": 87}),
                f_port: Some(1),
                f_cnt: Some(100),
            },
            InventoryUplink {
                received_at: "2026-05-22T12:01:00Z".to_string(),
                decoded_object: json!({"temperature": 23.1, "battery": 86}),
                f_port: Some(1),
                f_cnt: Some(101),
            },
        ];
        let (observed, heterogeneous) = compute_observed_keys(&uplinks);
        assert_eq!(observed.len(), 2);
        assert!(heterogeneous.is_empty());
        // Sorted alphabetically: battery, temperature.
        assert_eq!(observed[0].key, "battery");
        assert_eq!(observed[0].wire_type, WireType::Int);
        assert_eq!(observed[1].key, "temperature");
        assert_eq!(observed[1].wire_type, WireType::Float);
    }

    #[test]
    fn compute_observed_keys_flags_heterogeneous() {
        let uplinks = vec![
            InventoryUplink {
                received_at: "2026-05-22T12:00:00Z".to_string(),
                decoded_object: json!({"voltage": 3.7}),
                f_port: None,
                f_cnt: None,
            },
            InventoryUplink {
                received_at: "2026-05-22T12:01:00Z".to_string(),
                decoded_object: json!({"voltage": "low"}),
                f_port: None,
                f_cnt: None,
            },
        ];
        let (observed, heterogeneous) = compute_observed_keys(&uplinks);
        assert_eq!(observed.len(), 1);
        assert_eq!(observed[0].wire_type, WireType::String);
        assert_eq!(heterogeneous.len(), 1);
        assert_eq!(heterogeneous[0].key, "voltage");
        // types_seen joins sorted: "number,string"
        assert_eq!(heterogeneous[0].types_seen, "number,string");
    }
}

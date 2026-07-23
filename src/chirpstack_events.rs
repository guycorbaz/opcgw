// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2026] Guy Corbaz

//! Story E-1 (slice E-1a): gRPC uplink-event ingestion — last-known value, no
//! aggregation.
//!
//! opcgw's metrics poll (`GetMetrics`) time-aggregates device values
//! (Gauge→average, Absolute→sum), which corrupts discrete state and mis-stamps
//! analog points. A SCADA/OPC UA gateway must instead expose the **raw
//! last-known value** of each measurement with the **device's source
//! timestamp** and let the SCADA do any averaging/trending (GitHub #130).
//!
//! This module consumes ChirpStack's decoded uplink **events** via
//! `InternalService.StreamDeviceEvents` (the same API the inventory layer uses
//! at [`crate::chirpstack_inventory::stream_recent_device_uplinks`], but in a
//! long-lived form) and writes the last decoded value of each configured
//! `read_metric` to storage, stamped with the device event time — never
//! aggregated.
//!
//! **Scope of E-1a (this slice):** the stream is wired for **valve-class**
//! devices only (those with a command bound to `command_class = "valve"`, from
//! Story E-0), and the metrics poll is made to skip those devices so the
//! stream is the sole, authoritative writer for them. E-1b extends this to all
//! devices and fully retires the poll value-path.

use crate::config::{AppConfig, ReadMetric};
use crate::chirpstack_internal_proto::api::internal_service_client::InternalServiceClient;
use crate::chirpstack_internal_proto::api::{LogItem, StreamDeviceEventsRequest};
use crate::storage::{AsyncStorageExt, BatchMetricWrite, MetricType, StorageBackend};
use chrono::{DateTime, Utc};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio_util::sync::CancellationToken;
use tonic::metadata::MetadataValue;
use tonic::service::Interceptor;
use tonic::transport::Channel;
use tonic::{Request, Status};
use tracing::{debug, error, info, warn};

/// Initial reconnect backoff after a stream drop.
const RECONNECT_BACKOFF_START: Duration = Duration::from_secs(1);
/// Maximum reconnect backoff (capped exponential).
const RECONNECT_BACKOFF_MAX: Duration = Duration::from_secs(30);
/// After this many uplink events, warn (once per field) about configured
/// read_metrics that have never appeared in the device's decoded object —
/// they will not populate via the stream (e.g. DevStatus-sourced battery, or a
/// `chirpstack_metric_name` that doesn't match the codec's field name).
const ORPHAN_WARN_AFTER_EVENTS: u32 = 3;

// ---------------------------------------------------------------------------
// Pure mapping — the testable core (no gRPC, no I/O).
// ---------------------------------------------------------------------------

/// Convert one decoded-object JSON value to the configured [`MetricType`].
///
/// Returns `None` when the JSON value cannot be represented as the configured
/// type (the caller logs + skips — never panics). Numbers coerce across
/// Int/Float, and an integer `0`/`1` coerces to `Bool` so the codec's
/// integer flags (e.g. `fault`, `lowBattery`) map cleanly whether configured
/// as `Int` or `Bool`.
fn json_to_metric(
    value: &serde_json::Value,
    target: &crate::config::OpcMetricTypeConfig,
) -> Option<MetricType> {
    use crate::config::OpcMetricTypeConfig as T;
    /// Largest f64 magnitude whose integers are all exactly representable
    /// (2^53). Beyond it an `as i64` cast would silently snap to a nearby
    /// value, so such floats are rejected as mismatches instead.
    const F64_EXACT_INT_MAX: f64 = 9_007_199_254_740_992.0;
    match target {
        T::Float => value
            .as_f64()
            .or_else(|| value.as_i64().map(|i| i as f64))
            .map(MetricType::Float),
        T::Int => value
            .as_i64()
            .or_else(|| {
                // Accept a float only when it is integral and exactly
                // representable — a fractional value for an Int-configured
                // metric is a codec/config mismatch, not something to
                // silently truncate.
                value
                    .as_f64()
                    .filter(|f| f.fract() == 0.0 && f.abs() <= F64_EXACT_INT_MAX)
                    .map(|f| f as i64)
            })
            .map(MetricType::Int),
        T::Bool => value
            .as_bool()
            // Strictly 0/1: the codec contract for flags. Any other integer
            // (e.g. a `fault: 2`) is surfaced as a type mismatch rather than
            // silently reinterpreted as `true`.
            .or_else(|| value.as_i64().filter(|i| *i == 0 || *i == 1).map(|i| i != 0))
            .map(MetricType::Bool),
        T::String => value.as_str().map(|s| MetricType::String(s.to_string())),
    }
}

/// One configured `read_metric` whose uplink field was present but could not
/// convert to the configured `metric_type` (Story J-0, #160). Reported by
/// [`map_uplink_to_writes`] instead of being logged inline, so the caller owns
/// emission — the live ingest path warns once per (device, metric) and records
/// a web error-event, while the reconnect-backfill path stays quiet.
///
/// Deliberately carries no field *value*: uplink payloads are unconstrained
/// upstream data (log-injection surface). The metric name comes from operator
/// config and is trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FieldMismatch {
    /// The `chirpstack_metric_name` that failed to convert.
    pub metric_name: String,
    /// Debug rendering of the configured [`crate::config::OpcMetricTypeConfig`].
    pub configured_type: String,
    /// Why it failed: either the observed JSON kind (`"a string"`, `"a
    /// number"`, …) or, when the kind matches but the value is out of
    /// contract, the specific reason.
    pub reason: &'static str,
}

impl FieldMismatch {
    /// Operator-facing message stored in the error-event feed and used in the
    /// `warn!` line. Pinned by tests — keep the shape stable.
    pub(crate) fn message(&self) -> String {
        format!(
            "metric '{}': configured {}, uplink field was {}; value skipped",
            self.metric_name, self.configured_type, self.reason
        )
    }
}

/// Result of mapping one decoded uplink object: the last-value writes plus any
/// configured fields that were present but unconvertible.
#[derive(Debug, Default)]
pub(crate) struct UplinkMapping {
    pub writes: Vec<BatchMetricWrite>,
    pub mismatches: Vec<FieldMismatch>,
}

/// Why `value` could not become `target`. Reports the observed JSON kind,
/// except for the two cases where the kind is right but the value breaks the
/// codec contract (`Bool` accepts strictly 0/1; `Int` rejects fractional and
/// inexactly-representable floats) — there a bare "was a number" would read as
/// nonsense next to a numeric configured type.
fn mismatch_reason(
    value: &serde_json::Value,
    target: &crate::config::OpcMetricTypeConfig,
) -> &'static str {
    use crate::config::OpcMetricTypeConfig as T;
    match value {
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
        serde_json::Value::Null => "null",
        serde_json::Value::Number(_) => match target {
            T::Bool => "a number outside the 0/1 flag contract",
            T::Int => "a non-integral (or too large) number",
            // Float/String reaching here means serde_json handed us a number
            // that is neither f64- nor str-convertible; keep it generic.
            _ => "a number",
        },
    }
}

/// Map a decoded uplink object to last-value [`BatchMetricWrite`]s, one per
/// configured `read_metric` whose `chirpstack_metric_name` is present in the
/// object. Each write is stamped with `event_time` (the device's report time,
/// NOT ingest/poll time). No aggregation: the value is taken verbatim.
///
/// The storage key is `chirpstack_metric_name` — the same key the metrics poll
/// writes and the OPC UA read path (`OpcUa::get_value`) looks up — so a stream
/// write is read back identically to a poll write.
///
/// Story J-0 (#160): unconvertible fields are returned in
/// [`UplinkMapping::mismatches`] rather than warned about here, so the caller
/// decides whether to log and record them. This function stays pure and sync.
pub(crate) fn map_uplink_to_writes(
    device_id: &str,
    metrics: &[ReadMetric],
    object: &serde_json::Value,
    event_time: DateTime<Utc>,
) -> UplinkMapping {
    let timestamp: SystemTime = event_time.into();
    let mut mapping = UplinkMapping::default();
    for metric in metrics {
        let field = match object.get(&metric.chirpstack_metric_name) {
            Some(v) if !v.is_null() => v,
            // Field absent (or null) in this uplink — leave the last value
            // untouched; not every uplink carries every field.
            _ => continue,
        };
        match json_to_metric(field, &metric.metric_type) {
            Some(data_type) => mapping.writes.push(BatchMetricWrite {
                device_id: device_id.to_string(),
                metric_name: metric.chirpstack_metric_name.clone(),
                data_type,
                timestamp,
            }),
            None => mapping.mismatches.push(FieldMismatch {
                metric_name: metric.chirpstack_metric_name.clone(),
                configured_type: format!("{:?}", metric.metric_type),
                reason: mismatch_reason(field, &metric.metric_type),
            }),
        }
    }
    mapping
}

/// Configured `chirpstack_metric_name`s that have not yet been observed in any
/// uplink object (`seen`) and have not already been warned about (`warned`).
/// Used to flag metrics that won't populate via the stream (e.g. battery from
/// DevStatus, or a codec field-name mismatch).
fn newly_orphaned(
    metrics: &[ReadMetric],
    seen: &HashSet<String>,
    warned: &HashSet<String>,
) -> Vec<String> {
    metrics
        .iter()
        .map(|m| &m.chirpstack_metric_name)
        .filter(|name| !seen.contains(*name) && !warned.contains(*name))
        .cloned()
        .collect()
}

/// True if `device_id` has a command bound to the valve class
/// (`command_class = "valve"`, from Story E-0). E-1a streams only these
/// devices; the poll is made to skip them so the stream is authoritative.
pub(crate) fn device_is_valve_class(config: &AppConfig, device_id: &str) -> bool {
    config
        .application_list
        .iter()
        .flat_map(|app| app.device_list.iter())
        .find(|dev| dev.device_id == device_id)
        .and_then(|dev| dev.device_command_list.as_ref())
        .map(|cmds| {
            cmds.iter()
                .any(|c| c.command_class.as_deref() == Some("valve"))
        })
        .unwrap_or(false)
}

/// Pure routing predicate: should this device's values come from the uplink
/// event stream (vs the aggregated metrics poll)? A device streams if it has
/// metrics AND (it is valve-class — E-1a — OR the fleet-wide
/// `stream_all_devices` switch is on — E-1b).
pub(crate) fn should_stream(
    is_valve_class: bool,
    stream_all_devices: bool,
    has_metrics: bool,
) -> bool {
    has_metrics && (is_valve_class || stream_all_devices)
}

/// True if `device_id`'s values are served by the event stream — and therefore
/// must be SKIPPED by the metrics poll so no aggregated value reaches OPC UA.
pub(crate) fn device_is_streamed(config: &AppConfig, device_id: &str) -> bool {
    let has_metrics = config
        .application_list
        .iter()
        .flat_map(|app| app.device_list.iter())
        .find(|dev| dev.device_id == device_id)
        .map(|dev| !dev.read_metric_list.is_empty())
        .unwrap_or(false);
    should_stream(
        device_is_valve_class(config, device_id),
        config.chirpstack.stream_all_devices,
        has_metrics,
    )
}

/// Collect the (device_id, read_metric_list) pairs to stream: valve-class
/// devices (E-1a) plus — when `stream_all_devices` is set (E-1b) — every
/// device with at least one configured read_metric.
fn streamed_devices(config: &AppConfig) -> Vec<(String, Vec<ReadMetric>)> {
    let mut out: Vec<(String, Vec<ReadMetric>)> = Vec::new();
    // The same DevEUI may legally appear under several applications (C-3
    // allows it); stream it ONCE — a second task would just duplicate the
    // gRPC stream — but MERGE the metric lists so a mapping that only the
    // later application configures still streams (first occurrence wins per
    // chirpstack_metric_name on conflicts).
    let mut index: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for app in &config.application_list {
        for dev in &app.device_list {
            if dev.read_metric_list.is_empty() {
                continue;
            }
            if !should_stream(
                device_is_valve_class(config, &dev.device_id),
                config.chirpstack.stream_all_devices,
                true, // non-empty read_metric_list checked above
            ) {
                continue;
            }
            match index.get(&dev.device_id) {
                None => {
                    index.insert(dev.device_id.clone(), out.len());
                    out.push((dev.device_id.clone(), dev.read_metric_list.clone()));
                }
                Some(&i) => {
                    let merged = &mut out[i].1;
                    for m in &dev.read_metric_list {
                        match merged
                            .iter()
                            .find(|e| e.chirpstack_metric_name == m.chirpstack_metric_name)
                        {
                            None => merged.push(m.clone()),
                            Some(existing) => {
                                // First occurrence wins; surface a TYPE
                                // conflict so the operator knows the second
                                // application's nodes read values stored
                                // under the first's type. (The collision
                                // itself pre-dates streaming — both apps'
                                // mappings always shared the storage key
                                // (device_id, chirpstack_metric_name).)
                                if existing.metric_type != m.metric_type {
                                    warn!(
                                        event = "uplink_metric_type_conflict",
                                        device_id = %dev.device_id,
                                        metric = %m.chirpstack_metric_name,
                                        kept_type = ?existing.metric_type,
                                        conflicting_type = ?m.metric_type,
                                        application = %app.application_name,
                                        "same device field mapped with different metric_type across applications; first mapping wins"
                                    );
                                }
                            }
                        }
                    }
                    debug!(
                        device_id = %dev.device_id,
                        application = %app.application_name,
                        "device configured under multiple applications; streaming once with merged metric list"
                    );
                }
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Stream-source seam (E-1b, AC#9) — the injection point over the gRPC stream,
// mirroring E-0's `DownlinkSink` approach so reconnect/backfill/precedence are
// testable without a live ChirpStack.
// ---------------------------------------------------------------------------

/// One parsed uplink event: the device's report time plus the codec-decoded
/// object. This is the unit the consumer ingests, whatever the source
/// (live stream or bounded backfill fetch).
#[derive(Debug, Clone)]
pub(crate) struct UplinkEvent {
    /// Device event time (`LogItem.time`) — becomes the stored
    /// `MetricValue.timestamp` and the OPC UA `source_timestamp`.
    pub event_time: DateTime<Utc>,
    /// The decoded uplink object (`body.object`).
    pub object: serde_json::Value,
}

/// Story E-3: a downlink **delivery acknowledgement** parsed from a ChirpStack
/// `ack` device event (`LogItem.description == "ack"`, body = `AckEvent`).
/// Emitted only for **confirmed** downlinks: `acknowledged == true` means the
/// device received it. Correlated to a queued command via `queue_item_id`
/// (== the `chirpstack_result_id` stored at enqueue).
#[derive(Debug, Clone)]
pub(crate) struct AckInfo {
    /// ChirpStack queue-item UUID (== command `chirpstack_result_id`).
    pub queue_item_id: String,
    /// Whether the device acknowledged the confirmed downlink.
    pub acknowledged: bool,
}

/// Story E-3: a **transmit acknowledgement** parsed from a ChirpStack `txack`
/// device event (`LogItem.description == "txack"`, body = `TxAckEvent`). Means
/// the gateway *transmitted* the downlink over the air — NOT that the device
/// received it. Recorded as a diagnostic only; it never confirms a command
/// (the locked E-3 policy: confirmation requires an `ack`, unconfirmed
/// downlinks resolve via the timeout sweep).
#[derive(Debug, Clone)]
pub(crate) struct TxAckInfo {
    /// ChirpStack queue-item UUID (== command `chirpstack_result_id`).
    pub queue_item_id: String,
}

/// One parsed device event from the `StreamDeviceEvents` stream. E-1 ingests
/// `Uplink`; Story E-3 additionally observes `Ack` (delivery confirmation) and
/// `TxAck` (transmit diagnostic) on the **same** stream.
#[derive(Debug, Clone)]
pub(crate) enum DeviceEvent {
    Uplink(UplinkEvent),
    Ack(AckInfo),
    TxAck(TxAckInfo),
}

/// An open per-device event stream. `next_event` returns `Ok(Some)` per
/// recognised device event (uplink / ack / txack — Story E-3 widened this
/// beyond uplinks), `Ok(None)` on a clean server-side close, `Err` on a
/// transport error (the consumer reconnects with backoff).
#[async_trait::async_trait]
pub(crate) trait UplinkStream: Send {
    async fn next_event(&mut self) -> Result<Option<DeviceEvent>, OpcGwStreamError>;
}

/// Source of uplink events for one device: the long-lived stream plus the
/// bounded recent-events fetch used for backfill on (re)connect (AC#7 —
/// backfill comes from real decoded events, never from aggregated
/// `GetMetrics`).
#[async_trait::async_trait]
pub(crate) trait UplinkSource: Send + Sync {
    /// Open the long-lived event stream for `device_id`.
    async fn connect(&self, device_id: &str) -> Result<Box<dyn UplinkStream>, OpcGwStreamError>;
    /// Fetch the newest recent uplink for `device_id` (bounded, returns
    /// `Ok(None)` when the device has no recent uplinks).
    async fn recent(&self, device_id: &str) -> Result<Option<UplinkEvent>, OpcGwStreamError>;
}

// ---------------------------------------------------------------------------
// gRPC implementation of the seam.
// ---------------------------------------------------------------------------

/// Bounded backfill fetch: how many recent LogItems to collect before picking
/// the newest uplink.
const BACKFILL_FETCH_LIMIT: u32 = 5;
/// Bounded backfill fetch: give up after this long (a missing backfill is not
/// an error — the live stream will deliver the next event).
const BACKFILL_MAX_WAIT: Duration = Duration::from_secs(3);

/// Minimal tonic interceptor attaching the operator's ChirpStack API token as
/// a bearer credential (mirrors `chirpstack_inventory::BearerInterceptor` and
/// `chirpstack::ApiTokenInterceptor`).
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

/// Production [`UplinkSource`]: ChirpStack's `InternalService` over gRPC.
pub(crate) struct GrpcUplinkSource {
    server_address: String,
    api_token: String,
}

/// Production [`UplinkStream`]: wraps the tonic `Streaming<LogItem>`, surfacing
/// `up` / `ack` / `txack` items as [`DeviceEvent`]s and skipping everything
/// else (join/error/status/...).
struct GrpcUplinkStream {
    inner: tonic::Streaming<LogItem>,
}

#[async_trait::async_trait]
impl UplinkStream for GrpcUplinkStream {
    async fn next_event(&mut self) -> Result<Option<DeviceEvent>, OpcGwStreamError> {
        loop {
            match self.inner.message().await {
                Ok(Some(item)) => match parse_device_event(&item) {
                    Some(event) => return Ok(Some(event)),
                    // Unhandled LogItem kind (join/error/status/...) or a
                    // malformed body — skip and keep pumping.
                    None => continue,
                },
                Ok(None) => return Ok(None),
                Err(e) => return Err(OpcGwStreamError(format!("stream item error: {}", e))),
            }
        }
    }
}

#[async_trait::async_trait]
impl UplinkSource for GrpcUplinkSource {
    async fn connect(&self, device_id: &str) -> Result<Box<dyn UplinkStream>, OpcGwStreamError> {
        let channel = Channel::from_shared(grpc_endpoint(&self.server_address))
            .map_err(|e| OpcGwStreamError(format!("invalid server_address: {}", e)))?
            .connect()
            .await
            .map_err(|e| OpcGwStreamError(format!("connect failed: {}", e)))?;

        let interceptor = BearerInterceptor {
            token: self.api_token.clone(),
        };
        let mut client = InternalServiceClient::with_interceptor(channel, interceptor);
        let request = Request::new(StreamDeviceEventsRequest {
            dev_eui: device_id.to_string(),
        });
        let response = client
            .stream_device_events(request)
            .await
            .map_err(|e| OpcGwStreamError(format!("stream_device_events: {}", e)))?;
        Ok(Box::new(GrpcUplinkStream {
            inner: response.into_inner(),
        }))
    }

    async fn recent(&self, device_id: &str) -> Result<Option<UplinkEvent>, OpcGwStreamError> {
        // Reuse the inventory layer's bounded fetch (AC#7: backfill from real
        // decoded events, never GetMetrics). It returns uplinks sorted
        // newest-first with RFC 3339 timestamps.
        let uplinks = crate::chirpstack_inventory::stream_recent_device_uplinks(
            &self.server_address,
            &self.api_token,
            device_id,
            BACKFILL_FETCH_LIMIT,
            BACKFILL_MAX_WAIT,
        )
        .await
        .map_err(|e| OpcGwStreamError(format!("recent-events fetch: {}", e)))?;
        Ok(uplinks.into_iter().next().and_then(|u| {
            let event_time = DateTime::parse_from_rfc3339(&u.received_at)
                .ok()?
                .with_timezone(&Utc);
            Some(UplinkEvent {
                event_time,
                object: u.decoded_object,
            })
        }))
    }
}

/// Parse one `LogItem` into `(event_time, decoded_object)` iff it is an uplink
/// (`description == "up"`) with a valid proto timestamp. Returns `None` for
/// non-uplink items, unparseable bodies, or malformed timestamps (same
/// defensive validation as `chirpstack_inventory::log_item_to_uplink`).
fn parse_up_event(item: &LogItem) -> Option<(DateTime<Utc>, serde_json::Value)> {
    if item.description != "up" {
        return None;
    }
    // From here on the item IS an uplink — a drop is an operator-relevant
    // diagnostic (mirrors the inventory layer's `inventory_uplink_dropped`),
    // not routine filtering.
    let body: serde_json::Value = match serde_json::from_str(&item.body) {
        Ok(v) => v,
        Err(_) => {
            // body itself is upstream-controlled free text — log only its
            // length (numeric, injection-safe) for diagnostics.
            warn!(
                event = "uplink_event_dropped",
                reason = "unparseable_body",
                body_len = item.body.len(),
                "dropping uplink event: LogItem body is not valid JSON"
            );
            return None;
        }
    };
    let object = body
        .get("object")
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let event_time = match item.time.as_ref() {
        Some(ts) if ts.nanos >= 0 && ts.nanos < 1_000_000_000 && ts.seconds >= 0 => {
            match DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32) {
                Some(dt) => dt,
                None => {
                    warn!(
                        event = "uplink_event_dropped",
                        reason = "malformed_proto_timestamp",
                        timestamp = %format!("seconds={},nanos={}", ts.seconds, ts.nanos),
                        "dropping uplink event: proto timestamp out of chrono range"
                    );
                    return None;
                }
            }
        }
        other => {
            let ts_repr = match other {
                Some(ts) => format!("seconds={},nanos={}", ts.seconds, ts.nanos),
                None => "missing".to_string(),
            };
            warn!(
                event = "uplink_event_dropped",
                reason = "malformed_proto_timestamp",
                timestamp = %ts_repr,
                "dropping uplink event: invalid or missing proto timestamp"
            );
            return None;
        }
    };
    Some((event_time, object))
}

/// Dispatch one `LogItem` to a [`DeviceEvent`] by its `description`:
/// `"up"` → [`DeviceEvent::Uplink`] (E-1), `"ack"` → [`DeviceEvent::Ack`] and
/// `"txack"` → [`DeviceEvent::TxAck`] (E-3). Any other kind (join/error/
/// status/...) or an unparseable body returns `None` (the stream skips it).
fn parse_device_event(item: &LogItem) -> Option<DeviceEvent> {
    match item.description.as_str() {
        "up" => parse_up_event(item).map(|(event_time, object)| {
            DeviceEvent::Uplink(UplinkEvent { event_time, object })
        }),
        "ack" => parse_ack_event(item).map(DeviceEvent::Ack),
        "txack" => parse_txack_event(item).map(DeviceEvent::TxAck),
        _ => None,
    }
}

/// Body fields of a ChirpStack `ack` / `txack` event we need for E-3
/// correlation. ChirpStack serialises the event body as JSON; field casing has
/// varied across versions, so accept both `queue_item_id` (proto/snake_case)
/// and `queueItemId` (protojson/camelCase) via serde aliases. Unknown fields
/// are ignored.
#[derive(serde::Deserialize)]
struct AckEventBody {
    #[serde(alias = "queueItemId")]
    queue_item_id: Option<String>,
    acknowledged: Option<bool>,
}

#[derive(serde::Deserialize)]
struct TxAckEventBody {
    #[serde(alias = "queueItemId")]
    queue_item_id: Option<String>,
}

/// Parse an `ack` LogItem body into [`AckInfo`]. Returns `None` (drop) when the
/// body is not JSON or carries no `queue_item_id` — without a correlation key
/// the ack is useless, and the timeout sweep remains the safety net.
fn parse_ack_event(item: &LogItem) -> Option<AckInfo> {
    let body: AckEventBody = serde_json::from_str(&item.body)
        .map_err(|_| {
            warn!(
                event = "command_ack_dropped",
                reason = "unparseable_body",
                body_len = item.body.len(),
                "dropping ack event: LogItem body is not valid JSON"
            );
        })
        .ok()?;
    let queue_item_id = body.queue_item_id.filter(|s| !s.is_empty()).or_else(|| {
        debug!(
            event = "command_ack_dropped",
            reason = "missing_queue_item_id",
            "ack event has no queue_item_id; cannot correlate to a command"
        );
        None
    })?;
    // An ABSENT `acknowledged` is indeterminate, not a NACK. Dropping the ack
    // (and letting the timeout sweep decide) is the correct conservative
    // direction: defaulting a missing flag to `false` would actively mark a
    // possibly-delivered command Failed (review iter-1, blind+edge HIGH). Only
    // an explicit `acknowledged=false` is a real NACK.
    let acknowledged = match body.acknowledged {
        Some(a) => a,
        None => {
            debug!(
                event = "command_ack_dropped",
                reason = "missing_acknowledged",
                queue_item_id = %queue_item_id,
                "ack event has no acknowledged flag; leaving the command to the timeout sweep"
            );
            return None;
        }
    };
    Some(AckInfo {
        queue_item_id,
        acknowledged,
    })
}

/// Parse a `txack` LogItem body into [`TxAckInfo`]. Diagnostic only.
fn parse_txack_event(item: &LogItem) -> Option<TxAckInfo> {
    let body: TxAckEventBody = serde_json::from_str(&item.body).ok()?;
    let queue_item_id = body.queue_item_id.filter(|s| !s.is_empty())?;
    Some(TxAckInfo { queue_item_id })
}

/// Build the tonic gRPC endpoint URL from the configured server address
/// (mirrors `chirpstack_inventory`).
fn grpc_endpoint(server_address: &str) -> String {
    if server_address.starts_with("http://") || server_address.starts_with("https://") {
        server_address.to_string()
    } else {
        format!("http://{}", server_address)
    }
}

/// True when a candidate write (stamped `candidate`) is fresher than the
/// currently stored value's timestamp. The backfill guard: an event fetched on
/// (re)connect must never overwrite a newer value the live stream (or a
/// previous run) already stored. `None` (nothing stored yet) is always
/// "fresher" — that's exactly the cold-start case backfill exists for.
fn is_fresher(candidate: SystemTime, stored: Option<DateTime<Utc>>) -> bool {
    match stored {
        None => true,
        Some(stored_ts) => DateTime::<Utc>::from(candidate) > stored_ts,
    }
}

/// Drop candidate writes that are not fresher than what storage already holds
/// (see [`is_fresher`]). Shared by the live pump and the (re)connect backfill:
/// ChirpStack **replays recent event history on every stream connect** (the
/// very behaviour the bounded recent-events fetch relies on), so BOTH paths
/// can deliver events older than the stored last-value. This guard makes the
/// whole value path monotonic by device-report time — no replayed or
/// out-of-order event ever regresses a last-known value.
async fn filter_fresher_writes(
    backend: &Arc<dyn StorageBackend>,
    device_id: &str,
    candidates: Vec<BatchMetricWrite>,
) -> Vec<BatchMetricWrite> {
    // Note: when one metric's guard read fails (below) the OTHER metrics of
    // the same event still write — a single uplink is not atomic across its
    // metrics on the storage layer (it never was: batch_write_metrics has no
    // cross-metric transaction tie observable by readers mid-batch).
    let mut writes = Vec::with_capacity(candidates.len());
    for write in candidates {
        let stored_ts = match backend
            .async_store()
            .get_metric_value(device_id.to_string(), write.metric_name.clone())
            .await
        {
            Ok(stored) => stored.map(|v| v.timestamp),
            Err(e) => {
                // Fail OPEN, audibly: if the stored timestamp can't be read,
                // write the candidate anyway. Rationale (review iter-3): the
                // worst case of writing unverified is a TRANSIENT regression
                // (a replayed older event lands, corrected by the next live
                // event), whereas skipping on a PERSISTENT read failure
                // (e.g. one corrupt stored row that the UPSERT below would
                // actually repair) would freeze the metric forever. When the
                // fault IS a repairable row, the warn stops once the write
                // repairs it; for other faults (lock contention, I/O) it
                // recurs per event at LoRaWAN cadence — by design, so a
                // storage fault stays visible.
                warn!(
                    event = "uplink_guard_read_failed",
                    device_id = %device_id,
                    metric = %write.metric_name,
                    error = %e,
                    "freshness-guard storage read failed; writing unverified (fail-open)"
                );
                writes.push(write);
                continue;
            }
        };
        if is_fresher(write.timestamp, stored_ts) {
            writes.push(write);
        }
    }
    writes
}

/// Backfill the last-known value on (re)connect (AC#7): fetch the newest
/// recent uplink via the bounded recent-events fetch and store any field value
/// **fresher than what storage already holds** (see [`is_fresher`]). Failures
/// are logged and swallowed — backfill is best-effort; the live stream is the
/// canonical path.
async fn backfill_device(
    source: &dyn UplinkSource,
    device_id: &str,
    metrics: &[ReadMetric],
    backend: &Arc<dyn StorageBackend>,
) {
    let event = match source.recent(device_id).await {
        Ok(Some(ev)) => ev,
        Ok(None) => {
            debug!(
                event = "uplink_backfill_empty",
                device_id = %device_id,
                "no recent uplink to backfill; waiting for live events"
            );
            return;
        }
        Err(e) => {
            warn!(
                event = "uplink_backfill_failed",
                device_id = %device_id,
                error = %e.0,
                "recent-events backfill fetch failed; waiting for live events"
            );
            return;
        }
    };

    let mapping = map_uplink_to_writes(device_id, metrics, &event.object, event.event_time);
    // Story J-0 (#160): the backfill re-processes an already-seen event on
    // EVERY stream (re)connect, so it deliberately neither warns nor records a
    // web error-event for a type mismatch — that would re-fire on every
    // reconnect and flood the bounded feed. The live ingest path (which holds
    // the once-per-(device, metric) dedup state) is the sole reporter.
    for mm in &mapping.mismatches {
        debug!(
            event = "uplink_field_type_mismatch",
            device_id = %device_id,
            metric = %mm.metric_name,
            configured_type = %mm.configured_type,
            source = "backfill",
            "decoded uplink field could not convert to configured type; skipping (backfill path: not reported)"
        );
    }
    let writes = filter_fresher_writes(backend, device_id, mapping.writes).await;
    if writes.is_empty() {
        debug!(
            event = "uplink_backfill_skipped",
            device_id = %device_id,
            "backfill event is not fresher than stored values; nothing to do"
        );
        return;
    }
    let n = writes.len();
    match backend.async_store().batch_write_metrics(writes).await {
        Ok(()) => info!(
            event = "uplink_backfill",
            device_id = %device_id,
            metrics_written = n,
            event_time = %event.event_time,
            "backfilled last-known values from recent uplink"
        ),
        Err(e) => error!(
            event = "uplink_store_failed",
            device_id = %device_id,
            error = %e,
            "failed to store backfill last-values"
        ),
    }
}

/// Per-device diagnostic state for one stream task, carried across reconnects
/// so warnings aren't re-evaluated from scratch on every stream drop. Bundled
/// (rather than passed as four loose `&mut` arguments) to keep `ingest_event`
/// and `connect_and_stream` within a sane parameter count.
///
/// All sets are keyed by `chirpstack_metric_name` alone — the device is implied
/// by the owning per-device task. State is rebuilt when the data-plane is
/// respawned (an operator's **Apply changes** soft restart), which is what
/// re-arms the warnings after a config fix.
#[derive(Debug, Default)]
struct UplinkDiagState {
    /// Configured metrics observed at least once in an uplink object.
    seen: HashSet<String>,
    /// Metrics already reported as never-seen (Story E-1b orphan tracking);
    /// cleared on a first sighting so the warning self-corrects.
    warned: HashSet<String>,
    /// Metrics already reported as type-mismatched (Story J-0, #160). Never
    /// cleared: a mismatch is a config fault, and the fix path (edit + Apply)
    /// respawns the task with fresh state.
    mismatched: HashSet<String>,
    /// Uplink events ingested by this device's task.
    events_seen: u32,
}

/// Best-effort capture of a metric-configuration problem into the bounded
/// error-event feed backing the web Errors view (Story G-4 #127 / J-0 #160).
/// Mirrors `ChirpstackPoller::capture_error_event`: the message is sanitized,
/// and a storage failure is logged and swallowed — observability must never
/// break uplink ingestion. Goes through the async facade so the blocking SQL
/// never runs on a tokio worker (#73).
async fn record_metric_event(
    backend: &Arc<dyn StorageBackend>,
    category: &str,
    device_id: &str,
    message: String,
) {
    let event = crate::storage::ErrorEvent {
        ts: Utc::now(),
        category: category.to_string(),
        device_id: Some(device_id.to_string()),
        // No application context reaches this layer, and threading `AppConfig`
        // down the per-device stream tasks to get one is not worth it — the
        // web view falls back to `device_id`, and the poller's device-scoped
        // captures pass `None` too.
        application_id: None,
        message: crate::utils::sanitize_error_message(&message),
    };
    if let Err(e) = backend.async_store().record_error_event(event).await {
        warn!(
            error = %e,
            category = category,
            device_id = %device_id,
            "Failed to record metric error event (non-fatal)"
        );
    }
}

/// Ingest one parsed uplink event: orphan-tracking bookkeeping, then the
/// last-value writes stamped with the device event time. Shared by the live
/// stream pump (factored out of the pre-E-1b inline loop body, unchanged in
/// behaviour).
async fn ingest_event(
    device_id: &str,
    metrics: &[ReadMetric],
    event: &UplinkEvent,
    backend: &Arc<dyn StorageBackend>,
    diag: &mut UplinkDiagState,
) {
    let UplinkDiagState { seen, warned, mismatched, events_seen } = diag;
    // Track which configured fields this device actually emits, and warn
    // (once per field) about ones that never appear — they won't populate via
    // the stream.
    *events_seen = events_seen.saturating_add(1);
    for m in metrics {
        let present = event
            .object
            .get(&m.chirpstack_metric_name)
            .map(|v| !v.is_null())
            .unwrap_or(false);
        if !present {
            continue;
        }
        // Record the sighting; on the FIRST one, self-correct any earlier
        // "never seen" warning — the field was just late or only emitted on
        // some uplinks (e.g. a conditionally-reported value), not a true
        // orphan. Kept as explicit statements (not a `&&` chain) so the
        // seen-set population can't be silently broken by a future edit.
        let first_sighting = seen.insert(m.chirpstack_metric_name.clone());
        if first_sighting && warned.remove(&m.chirpstack_metric_name) {
            info!(
                event = "uplink_metric_now_seen",
                device_id = %device_id,
                metric = %m.chirpstack_metric_name,
                events_observed = *events_seen,
                "previously-unseen configured read_metric is now present in an uplink (intermittent/late, not a true orphan)"
            );
        }
    }
    if *events_seen >= ORPHAN_WARN_AFTER_EVENTS {
        for name in newly_orphaned(metrics, seen, warned) {
            warn!(
                event = "uplink_metric_never_seen",
                device_id = %device_id,
                metric = %name,
                events_observed = *events_seen,
                "configured read_metric not seen in the first uplinks; may be DevStatus-sourced (e.g. battery), a chirpstack_metric_name vs codec field-name mismatch, OR only emitted on some uplinks — if it arrives later an uplink_metric_now_seen will follow"
            );
            // Story J-0 (#160): the `warned` set is already the once-per-metric
            // gate, so recording here cannot flood the feed.
            record_metric_event(
                backend,
                "metric_never_seen",
                device_id,
                format!(
                    "metric '{}': configured but not present in the first {} uplinks; check the codec field name (or it may be DevStatus-sourced)",
                    name, *events_seen
                ),
            )
            .await;
            warned.insert(name);
        }
    }
    let mapping = map_uplink_to_writes(device_id, metrics, &event.object, event.event_time);
    // Story J-0 (#160): report BEFORE the freshness filter — a type mismatch is
    // a config fault independent of whether this particular event is a replay.
    // Once per (device, metric) per stream-task lifetime: the feed is a set of
    // distinct problems, not a stream of occurrences (a persistently mistyped
    // field would otherwise evict every genuine error from the bounded feed and
    // add two SQL writes per uplink on contended storage, see #152).
    for mm in &mapping.mismatches {
        // The dedup marker is set regardless of whether the feed write below
        // succeeds. This is deliberate (code review 2026-07-23): the `warn!`
        // fires unconditionally, so a failed best-effort feed write still
        // leaves the fault in the operator log; and gating the marker on write
        // success would re-attempt the record on EVERY subsequent uplink during
        // a storage outage — the per-uplink write flood on contended NAS
        // storage that this design (and #152) exist to avoid.
        if mismatched.insert(mm.metric_name.clone()) {
            warn!(
                event = "uplink_field_type_mismatch",
                device_id = %device_id,
                metric = %mm.metric_name,
                configured_type = %mm.configured_type,
                "decoded uplink field could not convert to configured type; skipping (further occurrences at debug level)"
            );
            record_metric_event(backend, "metric_type_mismatch", device_id, mm.message()).await;
        } else {
            debug!(
                event = "uplink_field_type_mismatch",
                device_id = %device_id,
                metric = %mm.metric_name,
                configured_type = %mm.configured_type,
                "decoded uplink field could not convert to configured type; skipping (already reported)"
            );
        }
    }
    let candidates = mapping.writes;
    let candidate_count = candidates.len();
    // Freshness guard on the LIVE path too: ChirpStack replays recent event
    // history on every stream (re)connect, so the pump regularly sees events
    // older than the stored last-value — they must not regress it.
    let writes = filter_fresher_writes(backend, device_id, candidates).await;
    if writes.len() < candidate_count {
        debug!(
            event = "uplink_replay_skipped",
            device_id = %device_id,
            skipped = candidate_count - writes.len(),
            "skipped replayed/older uplink values (not fresher than stored)"
        );
    }
    if !writes.is_empty() {
        let n = writes.len();
        if let Err(e) = backend.async_store().batch_write_metrics(writes).await {
            error!(
                event = "uplink_store_failed",
                device_id = %device_id,
                error = %e,
                "failed to store uplink last-values"
            );
        } else {
            debug!(
                event = "uplink_ingested",
                device_id = %device_id,
                metrics_written = n,
                "stored uplink last-values"
            );
        }
    }
}

/// Story E-3: correlate a downlink delivery `ack` to its queued command and
/// transition its status. `acknowledged == true` → `Confirmed`; `false`
/// (device NACK / max downlink retries) → `Failed`. An ack whose
/// `queue_item_id` matches no command, or one for a command already in a
/// terminal state, is a benign no-op (logged at debug) — replayed acks on
/// stream reconnect and acks for commands we did not send must never error or
/// regress state (idempotent, relying on the storage layer's
/// `status IN ('Sent','Pending')` guard).
async fn handle_ack(backend: &Arc<dyn StorageBackend>, device_id: &str, ack: &AckInfo) {
    let cmd = match backend
        .async_store()
        .find_command_by_result_id(ack.queue_item_id.clone())
        .await
    {
        Ok(Some(c)) => c,
        Ok(None) => {
            debug!(
                event = "command_ack_unmatched",
                device_id = %device_id,
                chirpstack_result_id = %ack.queue_item_id,
                "ack for a queue_item_id with no matching command; ignoring"
            );
            return;
        }
        Err(e) => {
            warn!(
                event = "command_ack_lookup_failed",
                device_id = %device_id,
                chirpstack_result_id = %ack.queue_item_id,
                error = %e,
                "failed to look up command for ack; ignoring (timeout sweep remains the safety net)"
            );
            return;
        }
    };

    // Defence in depth (review iter-1): the ack arrives on THIS device's stream,
    // so the correlated command must belong to it. queue_item_ids are ChirpStack
    // UUIDs (globally unique), so a mismatch should be impossible — but if one
    // ever occurs, never confirm another device's command.
    if cmd.device_id != device_id {
        warn!(
            event = "command_ack_device_mismatch",
            stream_device_id = %device_id,
            command_device_id = %cmd.device_id,
            chirpstack_result_id = %ack.queue_item_id,
            "ack queue_item_id matched a command for a DIFFERENT device; ignoring"
        );
        return;
    }

    if ack.acknowledged {
        match backend.async_store().mark_command_confirmed(cmd.id).await {
            Ok(()) => {
                // confirmed_at is set inside mark_command_confirmed; now() is a
                // tight upper bound for it, so latency ≈ confirmed_at - sent_at.
                // Clamp to ≥0 — a backwards clock or future-dated sent_at must
                // not surface a negative latency in the audit log.
                let latency_ms = cmd.sent_at.map(|s| (Utc::now() - s).num_milliseconds().max(0));
                info!(
                    event = "command_confirmed",
                    command_id = cmd.id,
                    device_id = %cmd.device_id,
                    command_name = %cmd.command_name,
                    chirpstack_result_id = %ack.queue_item_id,
                    latency_ms = ?latency_ms,
                    "command delivery confirmed by device ack"
                );
            }
            Err(e) => debug!(
                event = "command_confirm_noop",
                command_id = cmd.id,
                error = %e,
                "command already terminal or gone when ack arrived (idempotent no-op)"
            ),
        }
    } else {
        match backend
            .async_store()
            .mark_command_failed(cmd.id, "Device did not acknowledge confirmed downlink (NACK / max retries)".to_string())
            .await
        {
            Ok(()) => warn!(
                event = "command_confirm_failed",
                command_id = cmd.id,
                device_id = %cmd.device_id,
                command_name = %cmd.command_name,
                chirpstack_result_id = %ack.queue_item_id,
                "device did not acknowledge confirmed downlink; marked Failed"
            ),
            Err(e) => debug!(
                event = "command_confirm_noop",
                command_id = cmd.id,
                error = %e,
                "command already terminal or gone when NACK arrived (idempotent no-op)"
            ),
        }
    }
}

/// Open the stream for one device and pump events into storage until the
/// stream closes, errors, or `cancel` fires. After a successful connect, runs
/// the timestamp-guarded backfill (AC#7) — connect-first ordering means no
/// live event can be missed, and the [`is_fresher`] guard means the backfill
/// can never overwrite a newer live value. Returns `Ok(())` on a clean close /
/// cancellation, `Err` on a connection or stream error (the caller reconnects
/// with backoff).
async fn connect_and_stream(
    source: &dyn UplinkSource,
    device_id: &str,
    metrics: &[ReadMetric],
    backend: &Arc<dyn StorageBackend>,
    cancel: &CancellationToken,
    diag: &mut UplinkDiagState,
) -> Result<(), OpcGwStreamError> {
    // The initial connect is also cancellation-aware: without this, a child
    // mid-connect to an unreachable server would block the supervisor's
    // shutdown join until the transport's own timeout.
    let mut stream = tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(()),
        res = source.connect(device_id) => res?,
    };

    info!(
        event = "uplink_stream_connected",
        device_id = %device_id,
        "uplink event stream connected"
    );

    // Backfill AFTER the live stream is open so no event can slip into a gap;
    // the freshness guard resolves the (rare) overlap in the stream's favour.
    // Cancellation-aware: the bounded fetch opens its own channel, and
    // shutdown must not wait on it.
    tokio::select! {
        biased;
        _ = cancel.cancelled() => return Ok(()),
        _ = backfill_device(source, device_id, metrics, backend) => {}
    }

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            msg = stream.next_event() => {
                match msg {
                    Ok(Some(DeviceEvent::Uplink(event))) => ingest_event(
                        device_id,
                        metrics,
                        &event,
                        backend,
                        diag,
                    ).await,
                    // Story E-3: downlink delivery confirmation rides the same
                    // stream. An ack confirms (or NACK-fails) the queued
                    // command; a txack is a transmit diagnostic only.
                    Ok(Some(DeviceEvent::Ack(ack))) => handle_ack(backend, device_id, &ack).await,
                    Ok(Some(DeviceEvent::TxAck(txack))) => debug!(
                        event = "command_txack",
                        device_id = %device_id,
                        chirpstack_result_id = %txack.queue_item_id,
                        "downlink transmitted by gateway (diagnostic; not a delivery confirmation)"
                    ),
                    Ok(None) => return Ok(()), // stream closed by server
                    Err(e) => return Err(e),
                }
            }
        }
    }
}

/// Local error wrapper so the reconnect loop can format a single message
/// without dragging the broader `OpcGwError` taxonomy into transient
/// stream-retry logic.
pub(crate) struct OpcGwStreamError(pub(crate) String);

/// Long-lived consumer for one device: (re)connect with capped exponential
/// backoff until `cancel` fires.
async fn run_device_stream(
    source: Arc<dyn UplinkSource>,
    device_id: String,
    metrics: Vec<ReadMetric>,
    backend: Arc<dyn StorageBackend>,
    cancel: CancellationToken,
) {
    let mut backoff = RECONNECT_BACKOFF_START;
    // Diagnostic state persists across reconnects so the "never seen" and
    // "type mismatch" warnings aren't re-evaluated from scratch on every
    // stream drop (which would re-fire them on a flapping link).
    let mut diag = UplinkDiagState::default();
    loop {
        if cancel.is_cancelled() {
            return;
        }
        match connect_and_stream(
            source.as_ref(),
            &device_id,
            &metrics,
            &backend,
            &cancel,
            &mut diag,
        )
        .await
        {
            Ok(()) => {
                if cancel.is_cancelled() {
                    return;
                }
                // Clean close (server ended the stream) — reset backoff and
                // reconnect promptly.
                backoff = RECONNECT_BACKOFF_START;
                debug!(
                    event = "uplink_stream_closed",
                    device_id = %device_id,
                    "uplink stream closed by server; reconnecting"
                );
            }
            Err(e) => {
                warn!(
                    event = "uplink_stream_error",
                    device_id = %device_id,
                    error = %e.0,
                    backoff_secs = backoff.as_secs(),
                    "uplink stream error; will reconnect after backoff"
                );
            }
        }

        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tokio::time::sleep(backoff) => {}
        }
        backoff = (backoff * 2).min(RECONNECT_BACKOFF_MAX);
    }
}

/// Supervisor: spawn one long-lived stream task per E-1a-scoped (valve-class)
/// device and run until `cancel` fires, then join the children.
///
/// E-1b will widen `streamed_devices` to all devices and retire the poll
/// value-path. Hot-reload of the streamed device set is deferred (the task set
/// is fixed from the snapshot passed in — DD4).
pub async fn run_event_ingestion(
    config: Arc<AppConfig>,
    backend: Arc<dyn StorageBackend>,
    cancel: CancellationToken,
) {
    let source: Arc<dyn UplinkSource> = Arc::new(GrpcUplinkSource {
        server_address: config.chirpstack.server_address.clone(),
        api_token: config.chirpstack.api_token.clone(),
    });
    run_event_ingestion_with_source(config, source, backend, cancel).await
}

/// Seam-parameterised supervisor body (AC#9 test injection point); production
/// enters via [`run_event_ingestion`] with the gRPC source.
async fn run_event_ingestion_with_source(
    config: Arc<AppConfig>,
    source: Arc<dyn UplinkSource>,
    backend: Arc<dyn StorageBackend>,
    cancel: CancellationToken,
) {
    let devices = streamed_devices(&config);
    if devices.is_empty() {
        info!(
            event = "uplink_ingestion_idle",
            stream_all_devices = config.chirpstack.stream_all_devices,
            "no devices to stream; uplink event ingestion idle (set command_class=\"valve\" or chirpstack.stream_all_devices=true)"
        );
        // Still honour cancellation so the task exits cleanly on shutdown.
        cancel.cancelled().await;
        return;
    }

    info!(
        event = "uplink_ingestion_start",
        device_count = devices.len(),
        stream_all_devices = config.chirpstack.stream_all_devices,
        "starting uplink event ingestion (valve-class + stream_all_devices)"
    );

    let mut handles = Vec::with_capacity(devices.len());
    for (device_id, metrics) in devices {
        handles.push(tokio::spawn(run_device_stream(
            Arc::clone(&source),
            device_id,
            metrics,
            Arc::clone(&backend),
            cancel.clone(),
        )));
    }

    cancel.cancelled().await;
    for handle in handles {
        let _ = handle.await;
    }
    info!(event = "uplink_ingestion_stop", "uplink event ingestion stopped");
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{OpcMetricTypeConfig, ReadMetric};
    use serde_json::json;

    fn rm(opc_name: &str, cs_name: &str, t: OpcMetricTypeConfig) -> ReadMetric {
        ReadMetric {
            metric_name: opc_name.to_string(),
            chirpstack_metric_name: cs_name.to_string(),
            metric_type: t,
            metric_unit: None,
        }
    }

    fn fixed_time() -> DateTime<Utc> {
        DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap()
    }

    #[test]
    fn maps_each_field_with_event_timestamp() {
        let metrics = vec![
            rm("Status_v01", "valveStatusCode", OpcMetricTypeConfig::Int),
            rm("Position", "valvePosition", OpcMetricTypeConfig::Int),
        ];
        let object = json!({ "valveStatusCode": 195, "valvePosition": 0, "extra": 1 });
        let t = fixed_time();
        let writes = map_uplink_to_writes("dev1", &metrics, &object, t).writes;

        assert_eq!(writes.len(), 2, "only configured fields are written");
        let expected_ts: SystemTime = t.into();
        for w in &writes {
            assert_eq!(w.device_id, "dev1");
            assert_eq!(w.timestamp, expected_ts, "stamped with device event time");
        }
        let status = writes
            .iter()
            .find(|w| w.metric_name == "valveStatusCode")
            .unwrap();
        assert_eq!(status.data_type, MetricType::Int(195));
        let pos = writes
            .iter()
            .find(|w| w.metric_name == "valvePosition")
            .unwrap();
        assert_eq!(pos.data_type, MetricType::Int(0), "discrete 0 is NOT averaged");
    }

    #[test]
    fn string_field_maps_end_to_end() {
        let metrics = vec![rm("State", "state", OpcMetricTypeConfig::String)];
        let object = json!({ "state": "closed" });
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time()).writes;
        assert_eq!(writes.len(), 1);
        assert_eq!(
            writes[0].data_type,
            MetricType::String("closed".to_string())
        );
    }

    #[test]
    fn valve_flags_map_to_bool_and_int() {
        let metrics = vec![
            rm("Moving", "moving", OpcMetricTypeConfig::Bool),
            rm("Fault", "fault", OpcMetricTypeConfig::Int),
        ];
        // codec emits integer flags
        let object = json!({ "moving": 1, "fault": 0 });
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time()).writes;
        let moving = writes.iter().find(|w| w.metric_name == "moving").unwrap();
        assert_eq!(moving.data_type, MetricType::Bool(true));
        let fault = writes.iter().find(|w| w.metric_name == "fault").unwrap();
        assert_eq!(fault.data_type, MetricType::Int(0));
    }

    #[test]
    fn float_accepts_integer_json() {
        let metrics = vec![rm("Code", "valveStatusCode", OpcMetricTypeConfig::Float)];
        let object = json!({ "valveStatusCode": 195 });
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time()).writes;
        assert_eq!(writes[0].data_type, MetricType::Float(195.0));
    }

    #[test]
    fn absent_field_is_skipped_not_zeroed() {
        let metrics = vec![rm("State", "state", OpcMetricTypeConfig::String)];
        let object = json!({ "other": 1 });
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time()).writes;
        assert!(writes.is_empty(), "absent field leaves last value untouched");
    }

    #[test]
    fn newly_orphaned_flags_unseen_unwarned_fields() {
        let metrics = vec![
            rm("Status", "valveStatusCode", OpcMetricTypeConfig::Int),
            rm("Battery", "batteryLevel", OpcMetricTypeConfig::Int),
            rm("State", "state", OpcMetricTypeConfig::String),
        ];
        let mut seen = HashSet::new();
        seen.insert("valveStatusCode".to_string());
        seen.insert("state".to_string());
        let mut warned = HashSet::new();
        // batteryLevel never seen → orphaned.
        assert_eq!(
            newly_orphaned(&metrics, &seen, &warned),
            vec!["batteryLevel".to_string()]
        );
        // already warned → not re-reported.
        warned.insert("batteryLevel".to_string());
        assert!(newly_orphaned(&metrics, &seen, &warned).is_empty());
    }

    #[test]
    fn should_stream_routing() {
        // valve-class always streams (E-1a), regardless of the fleet switch.
        assert!(should_stream(true, false, true));
        assert!(should_stream(true, true, true));
        // non-valve streams only when the fleet switch is on (E-1b).
        assert!(!should_stream(false, false, true), "default: non-valve stays on poll");
        assert!(should_stream(false, true, true), "stream_all_devices migrates non-valve");
        // no metrics → never streamed (nothing to write).
        assert!(!should_stream(false, true, false));
        assert!(!should_stream(true, true, false));
    }

    #[test]
    fn type_mismatch_is_skipped_not_panicked() {
        // configured Int but the field arrives as a non-numeric string
        let metrics = vec![rm("Code", "valveStatusCode", OpcMetricTypeConfig::Int)];
        let object = json!({ "valveStatusCode": "oops" });
        let mapping = map_uplink_to_writes("dev1", &metrics, &object, fixed_time());
        assert!(mapping.writes.is_empty());
        // Story J-0 (#160): skipped AND reported, so the caller can surface it.
        assert_eq!(mapping.mismatches.len(), 1);
        assert_eq!(mapping.mismatches[0].metric_name, "valveStatusCode");
        assert_eq!(mapping.mismatches[0].reason, "a string");
        // Pinned operator-facing message (AC#1).
        assert_eq!(
            mapping.mismatches[0].message(),
            "metric 'valveStatusCode': configured Int, uplink field was a string; value skipped"
        );
    }

    #[test]
    fn mapping_reports_each_mismatch_and_keeps_convertible_fields() {
        // A mismatching field must not suppress its well-typed siblings, and
        // two distinct broken metrics are reported separately (AC#9 b/e).
        let metrics = vec![
            rm("Good", "valveStatusCode", OpcMetricTypeConfig::Int),
            rm("Bad1", "rain", OpcMetricTypeConfig::Int),
            rm("Bad2", "label", OpcMetricTypeConfig::Float),
        ];
        let object = json!({ "valveStatusCode": 195, "rain": "wet", "label": "x" });
        let mapping = map_uplink_to_writes("dev1", &metrics, &object, fixed_time());
        assert_eq!(mapping.writes.len(), 1, "the convertible field still writes");
        assert_eq!(mapping.writes[0].metric_name, "valveStatusCode");
        let names: Vec<&str> =
            mapping.mismatches.iter().map(|m| m.metric_name.as_str()).collect();
        assert_eq!(names, vec!["rain", "label"]);
    }

    #[test]
    fn mismatch_reason_names_the_observed_json_kind() {
        let cases = [
            (json!("s"), OpcMetricTypeConfig::Int, "a string"),
            (json!(true), OpcMetricTypeConfig::Int, "a boolean"),
            (json!([1]), OpcMetricTypeConfig::Int, "an array"),
            (json!({"a": 1}), OpcMetricTypeConfig::Int, "an object"),
            // Kind matches the numeric configured type, so the reason explains
            // the contract breach instead of claiming a kind mismatch.
            (json!(2), OpcMetricTypeConfig::Bool, "a number outside the 0/1 flag contract"),
            (json!(3.9), OpcMetricTypeConfig::Int, "a non-integral (or too large) number"),
        ];
        for (value, target, expected) in cases {
            let metrics = vec![rm("M", "field", target)];
            let object = json!({ "field": value });
            let mapping = map_uplink_to_writes("dev1", &metrics, &object, fixed_time());
            assert_eq!(mapping.mismatches.len(), 1, "value {object} should mismatch");
            assert_eq!(mapping.mismatches[0].reason, expected, "for {object}");
        }
    }

    #[test]
    fn bool_coercion_is_strictly_zero_or_one() {
        let metrics = vec![rm("Fault", "fault", OpcMetricTypeConfig::Bool)];
        // 0 and 1 coerce; any other integer is a type mismatch (codec bug),
        // not a truthy reinterpretation.
        let ok = map_uplink_to_writes("dev1", &metrics, &json!({"fault": 1}), fixed_time()).writes;
        assert_eq!(ok[0].data_type, MetricType::Bool(true));
        let bad = map_uplink_to_writes("dev1", &metrics, &json!({"fault": 2}), fixed_time()).writes;
        assert!(bad.is_empty(), "fault=2 must be a mismatch, not true");
        let neg = map_uplink_to_writes("dev1", &metrics, &json!({"fault": -1}), fixed_time()).writes;
        assert!(neg.is_empty(), "fault=-1 must be a mismatch, not true");
    }

    #[test]
    fn int_coercion_rejects_fractional_floats() {
        let metrics = vec![rm("Code", "valveStatusCode", OpcMetricTypeConfig::Int)];
        // Integral float is accepted exactly…
        let ok = map_uplink_to_writes(
            "dev1",
            &metrics,
            &json!({"valveStatusCode": 195.0}),
            fixed_time(),
        ).writes;
        assert_eq!(ok[0].data_type, MetricType::Int(195));
        // …a fractional one is a mismatch, never silently truncated.
        let bad = map_uplink_to_writes(
            "dev1",
            &metrics,
            &json!({"valveStatusCode": 3.9}),
            fixed_time(),
        ).writes;
        assert!(bad.is_empty(), "3.9 must be a mismatch, not truncated to 3");
    }

    #[test]
    fn is_fresher_guard_boundaries() {
        let t1 = DateTime::<Utc>::from_timestamp(1_700_000_100, 0).unwrap();
        let t2 = DateTime::<Utc>::from_timestamp(1_700_000_200, 0).unwrap();
        // Nothing stored yet → always fresher (the cold-start backfill case).
        assert!(is_fresher(SystemTime::from(t1), None));
        // Strictly newer → fresher.
        assert!(is_fresher(SystemTime::from(t2), Some(t1)));
        // Equal → NOT fresher (re-writing the same event is pointless churn).
        assert!(!is_fresher(SystemTime::from(t1), Some(t1)));
        // Older → NOT fresher (backfill must never clobber a live value).
        assert!(!is_fresher(SystemTime::from(t1), Some(t2)));
    }

    // -----------------------------------------------------------------------
    // Stream-consumer tests against the UplinkSource seam (AC#9 e/f/g) —
    // scripted source, no gRPC.
    // -----------------------------------------------------------------------

    use crate::storage::memory::InMemoryBackend;
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    /// One scripted stream step.
    enum ScriptItem {
        /// An uplink (convenience: wrapped into `DeviceEvent::Uplink`).
        Event(UplinkEvent),
        /// Any device event verbatim (E-3 ack/txack scripting).
        Device(DeviceEvent),
        Error(&'static str),
    }

    /// Scripted [`UplinkStream`]: yields the scripted items in order, then
    /// stays open forever (pending) — mimicking a quiet live stream. Counts
    /// `next_event` calls (shared with the source) so tests get a POSITIVE
    /// signal that scripted items were fully consumed: the pump only issues
    /// call N+1 after it has finished ingesting item N.
    struct ScriptedStream {
        items: VecDeque<ScriptItem>,
        next_event_calls: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl UplinkStream for ScriptedStream {
        async fn next_event(&mut self) -> Result<Option<DeviceEvent>, OpcGwStreamError> {
            self.next_event_calls.fetch_add(1, Ordering::SeqCst);
            match self.items.pop_front() {
                Some(ScriptItem::Event(e)) => Ok(Some(DeviceEvent::Uplink(e))),
                Some(ScriptItem::Device(d)) => Ok(Some(d)),
                Some(ScriptItem::Error(m)) => Err(OpcGwStreamError(m.to_string())),
                None => std::future::pending().await,
            }
        }
    }

    /// Scripted [`UplinkSource`]: each `connect` hands out the next script;
    /// `recent` returns a fixed backfill event. Counts connects so reconnect
    /// behaviour is assertable.
    ///
    /// NOTE: `next_event_calls` is CUMULATIVE across every stream this source
    /// hands out (it is shared, not per-stream). Single-connect tests can use
    /// it as a consumption signal; multi-connect tests must account for calls
    /// made by earlier streams.
    struct ScriptedSource {
        connects: Mutex<VecDeque<Vec<ScriptItem>>>,
        recent: Mutex<Option<UplinkEvent>>,
        connect_count: AtomicUsize,
        next_event_calls: Arc<AtomicUsize>,
        /// When true, `recent()` re-delivers the same event on EVERY connect
        /// (cloning, not taking) — mirroring production `GrpcUplinkSource::recent`,
        /// which genuinely re-fetches the newest uplink on each reconnect. The
        /// default (false) takes it once, which suits tests that only care about
        /// the first-connect backfill.
        recent_persists: bool,
    }

    impl ScriptedSource {
        fn new(connects: Vec<Vec<ScriptItem>>, recent: Option<UplinkEvent>) -> Arc<Self> {
            Self::build(connects, recent, false)
        }

        /// Like `new`, but `recent()` re-delivers on every connect (see the
        /// `recent_persists` field). Use for reconnect tests that must exercise
        /// the backfill path more than once.
        fn new_persistent_recent(
            connects: Vec<Vec<ScriptItem>>,
            recent: Option<UplinkEvent>,
        ) -> Arc<Self> {
            Self::build(connects, recent, true)
        }

        fn build(
            connects: Vec<Vec<ScriptItem>>,
            recent: Option<UplinkEvent>,
            recent_persists: bool,
        ) -> Arc<Self> {
            Arc::new(Self {
                connects: Mutex::new(connects.into()),
                recent: Mutex::new(recent),
                connect_count: AtomicUsize::new(0),
                next_event_calls: Arc::new(AtomicUsize::new(0)),
                recent_persists,
            })
        }
    }

    #[async_trait::async_trait]
    impl UplinkSource for ScriptedSource {
        async fn connect(
            &self,
            _device_id: &str,
        ) -> Result<Box<dyn UplinkStream>, OpcGwStreamError> {
            self.connect_count.fetch_add(1, Ordering::SeqCst);
            let items = self
                .connects
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default();
            Ok(Box::new(ScriptedStream {
                items: items.into(),
                next_event_calls: Arc::clone(&self.next_event_calls),
            }))
        }

        async fn recent(
            &self,
            _device_id: &str,
        ) -> Result<Option<UplinkEvent>, OpcGwStreamError> {
            let mut guard = self.recent.lock().unwrap();
            if self.recent_persists {
                Ok(guard.clone())
            } else {
                Ok(guard.take())
            }
        }
    }

    fn uplink(ts_secs: i64, status_code: i64) -> UplinkEvent {
        UplinkEvent {
            event_time: DateTime::<Utc>::from_timestamp(ts_secs, 0).unwrap(),
            object: json!({ "valveStatusCode": status_code }),
        }
    }

    fn valve_metrics() -> Vec<ReadMetric> {
        vec![rm("Status", "valveStatusCode", OpcMetricTypeConfig::Int)]
    }

    /// Poll storage until the stored valveStatusCode matches `expected`, or
    /// panic after the timeout (generous: covers the 1 s reconnect backoff on
    /// a loaded CI machine).
    async fn wait_for_stored(backend: &Arc<dyn StorageBackend>, expected: i64) {
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Ok(Some(v)) = backend.get_metric_value("dev1", "valveStatusCode") {
                    if v.data_type == MetricType::Int(expected) {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("stored value never reached {}", expected));
    }

    /// AC#9 (f): on connect with no live events yet, the backfill serves the
    /// last value — stored with the DEVICE event time, not now().
    #[tokio::test]
    async fn backfill_serves_last_value_before_first_live_event() {
        let backfill_ts = 1_700_000_000_i64;
        let source = ScriptedSource::new(vec![vec![]], Some(uplink(backfill_ts, 195)));
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel = CancellationToken::new();

        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            valve_metrics(),
            Arc::clone(&backend),
            cancel.clone(),
        ));

        wait_for_stored(&backend, 195).await;
        let stored = backend
            .get_metric_value("dev1", "valveStatusCode")
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.timestamp,
            DateTime::<Utc>::from_timestamp(backfill_ts, 0).unwrap(),
            "backfill must carry the device event time"
        );

        cancel.cancel();
        task.await.unwrap();
    }

    /// AC#9 (e): a stream error triggers reconnect (with backoff) and
    /// ingestion continues on the new stream.
    #[tokio::test]
    async fn reconnect_after_stream_drop_continues_ingestion() {
        let source = ScriptedSource::new(
            vec![
                vec![
                    ScriptItem::Event(uplink(1_700_000_100, 193)),
                    ScriptItem::Error("simulated drop"),
                ],
                vec![ScriptItem::Event(uplink(1_700_000_200, 195))],
            ],
            None,
        );
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel = CancellationToken::new();

        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            valve_metrics(),
            Arc::clone(&backend),
            cancel.clone(),
        ));

        // First stream's event lands, then the drop, then the post-backoff
        // reconnect delivers the second stream's event.
        wait_for_stored(&backend, 195).await;
        assert!(
            source.connect_count.load(Ordering::SeqCst) >= 2,
            "a reconnect must have happened"
        );

        cancel.cancel();
        task.await.unwrap();
    }

    // ---------------------------------------------------------------
    // Story J-0 (#160): metric-problem reporting into the web error feed.
    // ---------------------------------------------------------------

    /// Build an uplink carrying an arbitrary decoded object — `uplink()` above
    /// is hard-wired to a well-typed `valveStatusCode`, which cannot express a
    /// mismatch or a multi-field object.
    fn uplink_obj(ts_secs: i64, object: serde_json::Value) -> UplinkEvent {
        UplinkEvent {
            event_time: DateTime::<Utc>::from_timestamp(ts_secs, 0).unwrap(),
            object,
        }
    }

    /// Wait until the pump has fully consumed `n` scripted items.
    ///
    /// `next_event_calls` is incremented BEFORE each item is handed over, and
    /// the pump only issues call n+1 after finishing item n — so observing
    /// n+1 calls proves item n was ingested. This is the only sound barrier
    /// for these tests: a type mismatch produces NO write, so `wait_for_stored`
    /// would return after the first uplink and the assertions below would pass
    /// even with the dedup removed entirely.
    async fn wait_for_consumed(source: &Arc<ScriptedSource>, n: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if source.next_event_calls.load(Ordering::SeqCst) > n {
                    return;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("pump did not consume {n} scripted items in time"));
    }

    fn events_of(backend: &Arc<dyn StorageBackend>, category: &str) -> Vec<crate::storage::ErrorEvent> {
        backend
            .recent_error_events(crate::utils::error_event_cap())
            .expect("recent_error_events")
            .into_iter()
            .filter(|e| e.category == category)
            .collect()
    }

    /// AC#1/#3: a persistently mistyped field is recorded ONCE, however many
    /// uplinks carry it. This is the story's regression guard — it must drive
    /// `ingest_event` through the real pump, not the pure mapping function.
    #[tokio::test]
    async fn type_mismatch_records_one_event_however_many_uplinks() {
        // Five uplinks, each with a distinct increasing timestamp (equal
        // timestamps would be dropped by the freshness guard) — `rain` is
        // configured Int but always arrives as a string.
        let items: Vec<ScriptItem> = (0..5)
            .map(|i| {
                ScriptItem::Event(uplink_obj(
                    1_700_000_100 + i,
                    json!({ "valveStatusCode": 190 + i, "rain": "wet" }),
                ))
            })
            .collect();
        let source = ScriptedSource::new(vec![items], None);
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel = CancellationToken::new();
        let metrics = vec![
            rm("Status", "valveStatusCode", OpcMetricTypeConfig::Int),
            rm("Rain", "rain", OpcMetricTypeConfig::Int),
        ];

        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            metrics,
            Arc::clone(&backend),
            cancel.clone(),
        ));
        wait_for_consumed(&source, 5).await;

        let events = events_of(&backend, "metric_type_mismatch");
        assert_eq!(events.len(), 1, "one distinct problem = one feed entry");
        assert_eq!(events[0].device_id.as_deref(), Some("dev1"));
        assert_eq!(events[0].application_id, None);
        assert_eq!(
            events[0].message,
            "metric 'rain': configured Int, uplink field was a string; value skipped"
        );

        // AC#7: the sibling well-typed field kept flowing throughout.
        let stored = backend
            .get_metric_value("dev1", "valveStatusCode")
            .expect("stored")
            .expect("present");
        assert_eq!(stored.data_type, MetricType::Int(194), "last value wins");

        cancel.cancel();
        task.await.unwrap();
    }

    /// AC#9(b): two different broken metrics are two distinct problems.
    #[tokio::test]
    async fn distinct_mismatched_metrics_record_separate_events() {
        let items: Vec<ScriptItem> = (0..3)
            .map(|i| {
                ScriptItem::Event(uplink_obj(
                    1_700_000_100 + i,
                    json!({ "rain": "wet", "temp": "hot" }),
                ))
            })
            .collect();
        let source = ScriptedSource::new(vec![items], None);
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel = CancellationToken::new();
        let metrics = vec![
            rm("Rain", "rain", OpcMetricTypeConfig::Int),
            rm("Temp", "temp", OpcMetricTypeConfig::Float),
        ];

        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            metrics,
            Arc::clone(&backend),
            cancel.clone(),
        ));
        wait_for_consumed(&source, 3).await;

        let mut msgs: Vec<String> = events_of(&backend, "metric_type_mismatch")
            .into_iter()
            .map(|e| e.message)
            .collect();
        msgs.sort();
        assert_eq!(msgs.len(), 2, "one entry per broken metric");
        assert!(msgs[0].contains("'rain'") || msgs[1].contains("'rain'"));
        assert!(msgs[0].contains("'temp'") || msgs[1].contains("'temp'"));

        cancel.cancel();
        task.await.unwrap();
    }

    /// AC#2: an orphaned metric records exactly one `metric_never_seen` once
    /// the 3-event threshold is crossed. AC#6: when the field finally shows
    /// up, `uplink_metric_now_seen` fires but records NOTHING (the feed has no
    /// clear semantics) — so the total stays at one.
    #[tokio::test]
    async fn orphaned_metric_records_once_and_now_seen_records_nothing() {
        // Ordering matters: the carrying uplink must come AFTER the threshold
        // is crossed, otherwise the metric is never orphaned and the test is
        // vacuous.
        let mut items: Vec<ScriptItem> = (0..3)
            .map(|i| {
                ScriptItem::Event(uplink_obj(
                    1_700_000_100 + i,
                    json!({ "valveStatusCode": 190 + i }),
                ))
            })
            .collect();
        items.push(ScriptItem::Event(uplink_obj(
            1_700_000_110,
            json!({ "valveStatusCode": 199, "battery": 87 }),
        )));
        let source = ScriptedSource::new(vec![items], None);
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel = CancellationToken::new();
        let metrics = vec![
            rm("Status", "valveStatusCode", OpcMetricTypeConfig::Int),
            rm("Battery", "battery", OpcMetricTypeConfig::Int),
        ];

        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            metrics,
            Arc::clone(&backend),
            cancel.clone(),
        ));
        wait_for_consumed(&source, 4).await;

        let events = events_of(&backend, "metric_never_seen");
        assert_eq!(events.len(), 1, "orphan reported once, and now_seen adds nothing");
        assert!(events[0].message.contains("'battery'"), "got: {}", events[0].message);

        cancel.cancel();
        task.await.unwrap();
    }

    /// Run `body` under a test-LOCAL tracing subscriber on a current-thread
    /// runtime and return everything it logged. Test-local (via
    /// `with_default`, thread-scoped) rather than the `tracing_test` global
    /// buffer, because sibling `#[tokio::test]`s in this module emit the same
    /// event names concurrently and would bleed into the capture.
    fn capture_logs<F, Fut>(body: F) -> String
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        use std::io::Write;
        use tracing_subscriber::{fmt as tracing_fmt, layer::SubscriberExt, Layer};

        #[derive(Clone)]
        struct VecWriter(Arc<Mutex<Vec<u8>>>);
        impl Write for VecWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for VecWriter {
            type Writer = VecWriter;
            fn make_writer(&'a self) -> Self::Writer {
                self.clone()
            }
        }

        let buf = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::Registry::default().with(
            tracing_fmt::layer()
                .with_writer(VecWriter(Arc::clone(&buf)))
                .with_level(true)
                .with_ansi(false)
                .with_filter(tracing_subscriber::filter::LevelFilter::TRACE),
        );
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        tracing::subscriber::with_default(subscriber, || rt.block_on(body()));
        let out = String::from_utf8_lossy(&buf.lock().unwrap()).to_string();
        out
    }

    /// AC#3/#9(f): the FIRST occurrence warns, every repeat drops to `debug!`.
    /// Without this, an implementation could record once (satisfying every
    /// other test) while still emitting one WARN per uplink — which is the
    /// actual operator complaint behind #160 (76 warns/day from one metric).
    #[test]
    fn mismatch_warns_once_then_drops_to_debug() {
        let logs = capture_logs(|| async {
            let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
            let metrics = vec![rm("Rain", "rain", OpcMetricTypeConfig::Int)];
            let mut diag = UplinkDiagState::default();
            for i in 0..4 {
                let event = uplink_obj(1_700_000_100 + i, json!({ "rain": "wet" }));
                ingest_event("dev1", &metrics, &event, &backend, &mut diag).await;
            }
        });
        let lines: Vec<&str> = logs
            .lines()
            .filter(|l| l.contains("uplink_field_type_mismatch"))
            .collect();
        assert_eq!(lines.len(), 4, "every occurrence is still logged somewhere");
        let warns = lines.iter().filter(|l| l.contains("WARN")).count();
        let debugs = lines.iter().filter(|l| l.contains("DEBUG")).count();
        assert_eq!(warns, 1, "exactly one WARN, got logs:\n{logs}");
        assert_eq!(debugs, 3, "repeats are debug-level, got logs:\n{logs}");
    }

    /// AC#6/#9(d): when an orphaned metric finally appears, `uplink_metric_now_seen`
    /// (info) fires to self-correct the earlier `uplink_metric_never_seen` warning,
    /// and records NOTHING to the feed. The feed-count-only test cannot see whether
    /// the info actually fired (the count is unaffected either way), so assert the
    /// log line directly — a regression deleting the self-correction block would
    /// otherwise pass silently.
    #[test]
    fn orphan_then_sighting_emits_now_seen_and_records_nothing() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let logs = capture_logs(|| {
            let backend = Arc::clone(&backend);
            async move {
                let metrics = vec![rm("Battery", "battery", OpcMetricTypeConfig::Int)];
                let mut diag = UplinkDiagState::default();
                // Three uplinks WITHOUT the field cross ORPHAN_WARN_AFTER_EVENTS,
                // so `battery` is flagged never-seen…
                for i in 0..3 {
                    let event = uplink_obj(1_700_000_100 + i, json!({ "other": 1 }));
                    ingest_event("dev1", &metrics, &event, &backend, &mut diag).await;
                }
                // …then it finally shows up, which must self-correct.
                let event = uplink_obj(1_700_000_110, json!({ "battery": 87 }));
                ingest_event("dev1", &metrics, &event, &backend, &mut diag).await;
            }
        });

        assert!(
            logs.contains("uplink_metric_never_seen"),
            "the orphan must first be warned; got:\n{logs}"
        );
        // Match the structured `event=` field, not a bare substring: the
        // never-seen WARN's own message text mentions "uplink_metric_now_seen"
        // ("…an uplink_metric_now_seen will follow"), so a substring filter
        // would double-count it.
        let now_seen: Vec<&str> = logs
            .lines()
            .filter(|l| l.contains(r#"event="uplink_metric_now_seen""#))
            .collect();
        assert_eq!(now_seen.len(), 1, "now_seen fires exactly once; got:\n{logs}");
        assert!(now_seen[0].contains("INFO"), "now_seen is info-level; got:\n{logs}");
        // AC#6: the self-correction records nothing — only the single
        // never-seen entry from before the field appeared remains.
        let feed = events_of(&backend, "metric_never_seen");
        assert_eq!(feed.len(), 1, "now_seen adds no feed entry");
    }

    /// AC#5: the reconnect-backfill path must never record. It re-processes an
    /// already-seen event on every connect, so recording there would re-fire
    /// on each reconnect of a flapping link and flood the bounded feed.
    ///
    /// Uses `new_persistent_recent` so `recent()` re-delivers the SAME mistyped
    /// backfill event on every one of the three connects — mirroring production
    /// `GrpcUplinkSource::recent`, which re-fetches each time. A `take`-once mock
    /// would only exercise the first delivery and a regression that started
    /// recording from the second occurrence onward would slip through.
    #[tokio::test]
    async fn backfill_mismatch_records_nothing_across_reconnects() {
        // `recent` (the backfill event) carries the mistyped field; the live
        // streams carry nothing at all, so the ONLY mismatch source is the
        // backfill — which runs once per connect, and we force three connects.
        let source = ScriptedSource::new_persistent_recent(
            vec![
                vec![ScriptItem::Error("drop 1")],
                vec![ScriptItem::Error("drop 2")],
                vec![],
            ],
            Some(uplink_obj(1_700_000_100, json!({ "rain": "wet" }))),
        );
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel = CancellationToken::new();
        let metrics = vec![rm("Rain", "rain", OpcMetricTypeConfig::Int)];

        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            metrics,
            Arc::clone(&backend),
            cancel.clone(),
        ));

        // Wait for the reconnects (each replays the backfill).
        tokio::time::timeout(Duration::from_secs(10), async {
            while source.connect_count.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("expected three connects");

        assert!(
            events_of(&backend, "metric_type_mismatch").is_empty(),
            "backfill path must not record, however many reconnects occur"
        );

        cancel.cancel();
        task.await.unwrap();
    }

    /// Review iter-1 P2 (+ iter-2 merge fix): the same DevEUI under two
    /// applications (legal per C-3) must stream once — with the two apps'
    /// metric lists MERGED, so a mapping only the second app configures is
    /// not silently lost.
    #[test]
    fn streamed_devices_dedups_and_merges_cross_application_deveui() {
        use crate::config::{ChirpStackApplications, ChirpstackDevice};
        let mk_dev = |metrics: Vec<ReadMetric>| ChirpstackDevice {
            device_id: "dev-dup".to_string(),
            device_name: "Dup".to_string(),
            stale_threshold_seconds: None,
            source_timestamp_server: false,
            read_metric_list: metrics,
            device_command_list: None,
        };
        let apps = vec![
            ChirpStackApplications {
                application_name: "App A".to_string(),
                application_id: "app-a".to_string(),
                // "temperature" configured as Float here…
                device_list: vec![mk_dev(vec![rm("T", "temperature", OpcMetricTypeConfig::Float)])],
            },
            ChirpStackApplications {
                application_name: "App B".to_string(),
                application_id: "app-b".to_string(),
                // …and as Int here (conflict: first wins), plus a metric
                // ONLY this app maps (must survive the merge).
                device_list: vec![mk_dev(vec![
                    rm("T2", "temperature", OpcMetricTypeConfig::Int),
                    rm("H", "humidity", OpcMetricTypeConfig::Float),
                ])],
            },
        ];
        let mut config = crate::web::test_support::stub_app_config_with_apps(&apps);
        config.chirpstack.stream_all_devices = true;
        let devices = streamed_devices(&config);
        assert_eq!(
            devices.len(),
            1,
            "same DevEUI under two applications must stream exactly once"
        );
        let metrics = &devices[0].1;
        assert_eq!(metrics.len(), 2, "metric lists must merge, not first-app-wins-all");
        let temp = metrics
            .iter()
            .find(|m| m.chirpstack_metric_name == "temperature")
            .unwrap();
        assert_eq!(
            temp.metric_type,
            OpcMetricTypeConfig::Float,
            "on per-metric conflict the FIRST application's mapping wins"
        );
        assert!(
            metrics.iter().any(|m| m.chirpstack_metric_name == "humidity"),
            "a metric only the second application maps must survive"
        );
    }

    /// Review iter-1 P1: ChirpStack replays recent event history on every
    /// stream (re)connect — a LIVE event older than the stored value must not
    /// regress it (the freshness guard applies to the pump, not just the
    /// backfill).
    #[tokio::test]
    async fn replayed_older_live_event_never_regresses_stored_value() {
        let fresh_ts = 1_700_000_200_i64;
        let source = ScriptedSource::new(
            vec![vec![
                // The live stream delivers the newest event, then a replayed
                // OLDER one (aggregation-era 391 as a tracer value).
                ScriptItem::Event(uplink(fresh_ts, 193)),
                ScriptItem::Event(uplink(fresh_ts - 100, 391)),
            ]],
            None,
        );
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel = CancellationToken::new();

        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            valve_metrics(),
            Arc::clone(&backend),
            cancel.clone(),
        ));

        wait_for_stored(&backend, 193).await;
        // POSITIVE consumption signal: the pump issues its 3rd next_event
        // call only after it has fully ingested (and here: discarded) the
        // 2nd scripted item — so the replayed older event was definitely
        // processed, not merely "not yet delivered".
        tokio::time::timeout(Duration::from_secs(15), async {
            while source.next_event_calls.load(Ordering::SeqCst) < 3 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("replayed older event was never consumed by the pump");

        let stored = backend
            .get_metric_value("dev1", "valveStatusCode")
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.data_type,
            MetricType::Int(193),
            "replayed older live event must not regress the stored value"
        );
        assert_eq!(
            stored.timestamp,
            DateTime::<Utc>::from_timestamp(fresh_ts, 0).unwrap()
        );

        cancel.cancel();
        task.await.unwrap();
    }

    /// AC#9 (g): no-aggregation precedence — an OLDER backfill event fetched
    /// on reconnect must never clobber the FRESHER value the live stream
    /// already stored. (The GetMetrics poll never writes streamed devices at
    /// all — `should_stream_routing` pins that; this pins the only remaining
    /// non-live write path.)
    #[tokio::test]
    async fn older_backfill_never_clobbers_fresher_stream_value() {
        let fresh_ts = 1_700_000_200_i64;
        let source = ScriptedSource::new(
            vec![
                // First connect: live stream delivers the FRESH value, then
                // drops.
                vec![
                    ScriptItem::Event(uplink(fresh_ts, 193)),
                    ScriptItem::Error("simulated drop"),
                ],
                // Second connect: quiet stream; backfill will fetch an OLDER
                // event (set below) and must be discarded by the guard.
                vec![],
            ],
            None,
        );
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel = CancellationToken::new();

        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            valve_metrics(),
            Arc::clone(&backend),
            cancel.clone(),
        ));

        // Wait for the fresh live value, then arm the stale backfill for the
        // reconnect.
        wait_for_stored(&backend, 193).await;
        *source.recent.lock().unwrap() = Some(uplink(fresh_ts - 100, 391));

        // Let the reconnect + backfill happen.
        tokio::time::timeout(Duration::from_secs(15), async {
            while source.connect_count.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            // Give the post-connect backfill a beat to run.
            tokio::time::sleep(Duration::from_millis(300)).await;
        })
        .await
        .expect("reconnect never happened");

        let stored = backend
            .get_metric_value("dev1", "valveStatusCode")
            .unwrap()
            .unwrap();
        assert_eq!(
            stored.data_type,
            MetricType::Int(193),
            "older backfill must not clobber the fresher stream value"
        );
        assert_eq!(
            stored.timestamp,
            DateTime::<Utc>::from_timestamp(fresh_ts, 0).unwrap()
        );

        cancel.cancel();
        task.await.unwrap();
    }

    // -----------------------------------------------------------------------
    // Story E-3: command delivery confirmation (ack/txack) tests.
    // -----------------------------------------------------------------------

    use crate::chirpstack_internal_proto::api::LogItem as PbLogItem;
    use crate::storage::{Command, CommandStatus};

    /// Build a `LogItem` with the given description + JSON body (proto time set
    /// to a fixed valid instant for `up` parsing; irrelevant for ack/txack).
    fn log_item(description: &str, body: serde_json::Value) -> PbLogItem {
        PbLogItem {
            id: "log-1".to_string(),
            time: Some(chirpstack_api::prost_types::Timestamp { seconds: 1_700_000_000, nanos: 0 }),
            description: description.to_string(),
            body: body.to_string(),
            properties: std::collections::HashMap::new(),
        }
    }

    /// Enqueue a command and mark it Sent with `result_id`, returning its id.
    fn sent_command(backend: &Arc<dyn StorageBackend>, result_id: &str) -> u64 {
        let cmd = Command {
            id: 0,
            device_id: "dev1".to_string(),
            metric_id: String::new(),
            command_name: "valveCmd".to_string(),
            parameters: serde_json::Value::Null,
            enqueued_at: Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: format!("hash-{result_id}"),
            chirpstack_result_id: None,
        };
        let id = backend.enqueue_command(cmd).expect("enqueue");
        backend.mark_command_sent(id, result_id).expect("mark sent");
        id
    }

    #[test]
    fn parse_device_event_dispatches_by_description() {
        // ack (camelCase queueItemId, as ChirpStack protojson emits)
        let ack = log_item("ack", serde_json::json!({"queueItemId": "qid-1", "acknowledged": true}));
        assert!(matches!(parse_device_event(&ack), Some(DeviceEvent::Ack(a)) if a.queue_item_id == "qid-1" && a.acknowledged));
        // ack snake_case alias
        let ack2 = log_item("ack", serde_json::json!({"queue_item_id": "qid-2", "acknowledged": false}));
        assert!(matches!(parse_device_event(&ack2), Some(DeviceEvent::Ack(a)) if a.queue_item_id == "qid-2" && !a.acknowledged));
        // txack
        let txack = log_item("txack", serde_json::json!({"queueItemId": "qid-3"}));
        assert!(matches!(parse_device_event(&txack), Some(DeviceEvent::TxAck(t)) if t.queue_item_id == "qid-3"));
        // unknown kind → skipped
        assert!(parse_device_event(&log_item("join", serde_json::json!({}))).is_none());
        // ack with no queue_item_id → dropped (no correlation key)
        assert!(parse_device_event(&log_item("ack", serde_json::json!({"acknowledged": true}))).is_none());
        // ack with queue_item_id but NO acknowledged flag → dropped (review
        // iter-1: indeterminate, must not default to a NACK/Failed).
        assert!(parse_device_event(&log_item("ack", serde_json::json!({"queueItemId": "qid-x"}))).is_none());
        // ack with malformed body → dropped, not a panic
        let bad = PbLogItem {
            id: "x".into(), time: None, description: "ack".into(),
            body: "{not json".into(), properties: std::collections::HashMap::new(),
        };
        assert!(parse_device_event(&bad).is_none());
    }

    #[tokio::test]
    async fn ack_acknowledged_true_confirms_command() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let id = sent_command(&backend, "qid-confirm");
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-confirm".into(), acknowledged: true }).await;
        let cmd = backend.find_command_by_result_id("qid-confirm").unwrap().unwrap();
        assert_eq!(cmd.id, id);
        assert_eq!(cmd.status, CommandStatus::Confirmed, "ack(true) must confirm");
        assert!(cmd.confirmed_at.is_some(), "confirmed_at must be stamped");
    }

    #[tokio::test]
    async fn ack_acknowledged_false_fails_command() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        sent_command(&backend, "qid-nack");
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-nack".into(), acknowledged: false }).await;
        let cmd = backend.find_command_by_result_id("qid-nack").unwrap().unwrap();
        assert_eq!(cmd.status, CommandStatus::Failed, "NACK must fail the command");
        assert!(cmd.error_message.is_some(), "Failed command must carry an error message");
    }

    #[tokio::test]
    async fn ack_unmatched_queue_item_id_is_ignored() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let id = sent_command(&backend, "qid-real");
        // Ack for a DIFFERENT queue id: must not touch the real command, must not panic.
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-ghost".into(), acknowledged: true }).await;
        let cmd = backend.find_command_by_result_id("qid-real").unwrap().unwrap();
        assert_eq!(cmd.id, id);
        assert_eq!(cmd.status, CommandStatus::Sent, "unmatched ack must leave the command Sent");
    }

    #[tokio::test]
    async fn duplicate_ack_is_idempotent_noop() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        sent_command(&backend, "qid-dup");
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-dup".into(), acknowledged: true }).await;
        // ChirpStack replays events on reconnect — a second identical ack must
        // be a benign no-op (still Confirmed, no panic, no regression).
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-dup".into(), acknowledged: true }).await;
        let cmd = backend.find_command_by_result_id("qid-dup").unwrap().unwrap();
        assert_eq!(cmd.status, CommandStatus::Confirmed);
    }

    /// AC#3 regression guard: a `txack` (gateway transmitted) must NOT confirm
    /// the command — only an `ack` does. Drives the real stream pump.
    #[tokio::test]
    async fn txack_does_not_confirm_command() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        sent_command(&backend, "qid-tx");
        let source = ScriptedSource::new(
            vec![vec![ScriptItem::Device(DeviceEvent::TxAck(TxAckInfo {
                queue_item_id: "qid-tx".to_string(),
            }))]],
            None,
        );
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            valve_metrics(),
            Arc::clone(&backend),
            cancel.clone(),
        ));
        // Wait until the pump has consumed the txack (2nd next_event call:
        // it issues call N+1 only after fully handling item N).
        tokio::time::timeout(Duration::from_secs(15), async {
            while source.next_event_calls.load(Ordering::SeqCst) < 2 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("txack was never consumed");
        let cmd = backend.find_command_by_result_id("qid-tx").unwrap().unwrap();
        assert_eq!(cmd.status, CommandStatus::Sent, "txack must not confirm");
        cancel.cancel();
        task.await.unwrap();
    }

    /// AC#2 end-to-end on the stream: an `ack` delivered live confirms the
    /// queued command via the same consumer that ingests uplinks.
    #[tokio::test]
    async fn live_ack_on_stream_confirms_command() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        sent_command(&backend, "qid-live");
        let source = ScriptedSource::new(
            vec![vec![ScriptItem::Device(DeviceEvent::Ack(AckInfo {
                queue_item_id: "qid-live".to_string(),
                acknowledged: true,
            }))]],
            None,
        );
        let cancel = CancellationToken::new();
        let task = tokio::spawn(run_device_stream(
            source.clone() as Arc<dyn UplinkSource>,
            "dev1".to_string(),
            valve_metrics(),
            Arc::clone(&backend),
            cancel.clone(),
        ));
        tokio::time::timeout(Duration::from_secs(15), async {
            loop {
                if let Ok(Some(c)) = backend.find_command_by_result_id("qid-live") {
                    if c.status == CommandStatus::Confirmed {
                        return;
                    }
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("live ack never confirmed the command");
        cancel.cancel();
        task.await.unwrap();
    }
}

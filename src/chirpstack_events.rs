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
use crate::storage::{BatchMetricWrite, MetricType, StorageBackend};
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

/// Map a decoded uplink object to last-value [`BatchMetricWrite`]s, one per
/// configured `read_metric` whose `chirpstack_metric_name` is present in the
/// object. Each write is stamped with `event_time` (the device's report time,
/// NOT ingest/poll time). No aggregation: the value is taken verbatim.
///
/// The storage key is `chirpstack_metric_name` — the same key the metrics poll
/// writes and the OPC UA read path (`OpcUa::get_value`) looks up — so a stream
/// write is read back identically to a poll write.
pub(crate) fn map_uplink_to_writes(
    device_id: &str,
    metrics: &[ReadMetric],
    object: &serde_json::Value,
    event_time: DateTime<Utc>,
) -> Vec<BatchMetricWrite> {
    let timestamp: SystemTime = event_time.into();
    let mut writes = Vec::new();
    for metric in metrics {
        let field = match object.get(&metric.chirpstack_metric_name) {
            Some(v) if !v.is_null() => v,
            // Field absent (or null) in this uplink — leave the last value
            // untouched; not every uplink carries every field.
            _ => continue,
        };
        match json_to_metric(field, &metric.metric_type) {
            Some(data_type) => writes.push(BatchMetricWrite {
                device_id: device_id.to_string(),
                metric_name: metric.chirpstack_metric_name.clone(),
                data_type,
                timestamp,
            }),
            None => warn!(
                event = "uplink_field_type_mismatch",
                device_id = %device_id,
                metric = %metric.chirpstack_metric_name,
                configured_type = ?metric.metric_type,
                "decoded uplink field could not convert to configured type; skipping"
            ),
        }
    }
    writes
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
    Some(AckInfo {
        queue_item_id,
        // Absent `acknowledged` is treated as not-acknowledged (conservative:
        // a confirmed downlink that did not produce acknowledged=true did not
        // reach the device).
        acknowledged: body.acknowledged.unwrap_or(false),
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
fn filter_fresher_writes(
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
        let stored_ts = match backend.get_metric_value(device_id, &write.metric_name) {
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

    let candidate_writes = map_uplink_to_writes(device_id, metrics, &event.object, event.event_time);
    let writes = filter_fresher_writes(backend, device_id, candidate_writes);
    if writes.is_empty() {
        debug!(
            event = "uplink_backfill_skipped",
            device_id = %device_id,
            "backfill event is not fresher than stored values; nothing to do"
        );
        return;
    }
    let n = writes.len();
    match backend.batch_write_metrics(writes) {
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

/// Ingest one parsed uplink event: orphan-tracking bookkeeping, then the
/// last-value writes stamped with the device event time. Shared by the live
/// stream pump (factored out of the pre-E-1b inline loop body, unchanged in
/// behaviour).
fn ingest_event(
    device_id: &str,
    metrics: &[ReadMetric],
    event: &UplinkEvent,
    backend: &Arc<dyn StorageBackend>,
    seen: &mut HashSet<String>,
    warned: &mut HashSet<String>,
    events_seen: &mut u32,
) {
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
            warned.insert(name);
        }
    }
    let candidates = map_uplink_to_writes(device_id, metrics, &event.object, event.event_time);
    let candidate_count = candidates.len();
    // Freshness guard on the LIVE path too: ChirpStack replays recent event
    // history on every stream (re)connect, so the pump regularly sees events
    // older than the stored last-value — they must not regress it.
    let writes = filter_fresher_writes(backend, device_id, candidates);
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
        if let Err(e) = backend.batch_write_metrics(writes) {
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
fn handle_ack(backend: &Arc<dyn StorageBackend>, device_id: &str, ack: &AckInfo) {
    let cmd = match backend.find_command_by_result_id(&ack.queue_item_id) {
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

    if ack.acknowledged {
        match backend.mark_command_confirmed(cmd.id) {
            Ok(()) => {
                // confirmed_at is set inside mark_command_confirmed; now() is a
                // tight upper bound for it, so latency ≈ confirmed_at - sent_at.
                let latency_ms = cmd.sent_at.map(|s| (Utc::now() - s).num_milliseconds());
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
            .mark_command_failed(cmd.id, "Device did not acknowledge confirmed downlink (NACK / max retries)")
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
#[allow(clippy::too_many_arguments)]
async fn connect_and_stream(
    source: &dyn UplinkSource,
    device_id: &str,
    metrics: &[ReadMetric],
    backend: &Arc<dyn StorageBackend>,
    cancel: &CancellationToken,
    seen: &mut HashSet<String>,
    warned: &mut HashSet<String>,
    events_seen: &mut u32,
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
                        seen,
                        warned,
                        events_seen,
                    ),
                    // Story E-3: downlink delivery confirmation rides the same
                    // stream. An ack confirms (or NACK-fails) the queued
                    // command; a txack is a transmit diagnostic only.
                    Ok(Some(DeviceEvent::Ack(ack))) => handle_ack(backend, device_id, &ack),
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
    // Orphan-tracking state persists across reconnects so the "never seen"
    // warning isn't re-evaluated from scratch on every stream drop.
    let mut seen: HashSet<String> = HashSet::new();
    let mut warned: HashSet<String> = HashSet::new();
    let mut events_seen: u32 = 0;
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
            &mut seen,
            &mut warned,
            &mut events_seen,
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
        let writes = map_uplink_to_writes("dev1", &metrics, &object, t);

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
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time());
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
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time());
        let moving = writes.iter().find(|w| w.metric_name == "moving").unwrap();
        assert_eq!(moving.data_type, MetricType::Bool(true));
        let fault = writes.iter().find(|w| w.metric_name == "fault").unwrap();
        assert_eq!(fault.data_type, MetricType::Int(0));
    }

    #[test]
    fn float_accepts_integer_json() {
        let metrics = vec![rm("Code", "valveStatusCode", OpcMetricTypeConfig::Float)];
        let object = json!({ "valveStatusCode": 195 });
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time());
        assert_eq!(writes[0].data_type, MetricType::Float(195.0));
    }

    #[test]
    fn absent_field_is_skipped_not_zeroed() {
        let metrics = vec![rm("State", "state", OpcMetricTypeConfig::String)];
        let object = json!({ "other": 1 });
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time());
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
        let writes = map_uplink_to_writes("dev1", &metrics, &object, fixed_time());
        assert!(writes.is_empty());
    }

    #[test]
    fn bool_coercion_is_strictly_zero_or_one() {
        let metrics = vec![rm("Fault", "fault", OpcMetricTypeConfig::Bool)];
        // 0 and 1 coerce; any other integer is a type mismatch (codec bug),
        // not a truthy reinterpretation.
        let ok = map_uplink_to_writes("dev1", &metrics, &json!({"fault": 1}), fixed_time());
        assert_eq!(ok[0].data_type, MetricType::Bool(true));
        let bad = map_uplink_to_writes("dev1", &metrics, &json!({"fault": 2}), fixed_time());
        assert!(bad.is_empty(), "fault=2 must be a mismatch, not true");
        let neg = map_uplink_to_writes("dev1", &metrics, &json!({"fault": -1}), fixed_time());
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
        );
        assert_eq!(ok[0].data_type, MetricType::Int(195));
        // …a fractional one is a mismatch, never silently truncated.
        let bad = map_uplink_to_writes(
            "dev1",
            &metrics,
            &json!({"valveStatusCode": 3.9}),
            fixed_time(),
        );
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
    }

    impl ScriptedSource {
        fn new(connects: Vec<Vec<ScriptItem>>, recent: Option<UplinkEvent>) -> Arc<Self> {
            Arc::new(Self {
                connects: Mutex::new(connects.into()),
                recent: Mutex::new(recent),
                connect_count: AtomicUsize::new(0),
                next_event_calls: Arc::new(AtomicUsize::new(0)),
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
            Ok(self.recent.lock().unwrap().take())
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
        // ack with malformed body → dropped, not a panic
        let bad = PbLogItem {
            id: "x".into(), time: None, description: "ack".into(),
            body: "{not json".into(), properties: std::collections::HashMap::new(),
        };
        assert!(parse_device_event(&bad).is_none());
    }

    #[test]
    fn ack_acknowledged_true_confirms_command() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let id = sent_command(&backend, "qid-confirm");
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-confirm".into(), acknowledged: true });
        let cmd = backend.find_command_by_result_id("qid-confirm").unwrap().unwrap();
        assert_eq!(cmd.id, id);
        assert_eq!(cmd.status, CommandStatus::Confirmed, "ack(true) must confirm");
        assert!(cmd.confirmed_at.is_some(), "confirmed_at must be stamped");
    }

    #[test]
    fn ack_acknowledged_false_fails_command() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        sent_command(&backend, "qid-nack");
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-nack".into(), acknowledged: false });
        let cmd = backend.find_command_by_result_id("qid-nack").unwrap().unwrap();
        assert_eq!(cmd.status, CommandStatus::Failed, "NACK must fail the command");
        assert!(cmd.error_message.is_some(), "Failed command must carry an error message");
    }

    #[test]
    fn ack_unmatched_queue_item_id_is_ignored() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let id = sent_command(&backend, "qid-real");
        // Ack for a DIFFERENT queue id: must not touch the real command, must not panic.
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-ghost".into(), acknowledged: true });
        let cmd = backend.find_command_by_result_id("qid-real").unwrap().unwrap();
        assert_eq!(cmd.id, id);
        assert_eq!(cmd.status, CommandStatus::Sent, "unmatched ack must leave the command Sent");
    }

    #[test]
    fn duplicate_ack_is_idempotent_noop() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        sent_command(&backend, "qid-dup");
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-dup".into(), acknowledged: true });
        // ChirpStack replays events on reconnect — a second identical ack must
        // be a benign no-op (still Confirmed, no panic, no regression).
        handle_ack(&backend, "dev1", &AckInfo { queue_item_id: "qid-dup".into(), acknowledged: true });
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

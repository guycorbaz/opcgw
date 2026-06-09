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
    match target {
        T::Float => value
            .as_f64()
            .or_else(|| value.as_i64().map(|i| i as f64))
            .map(MetricType::Float),
        T::Int => value
            .as_i64()
            .or_else(|| value.as_f64().map(|f| f as i64))
            .map(MetricType::Int),
        T::Bool => value
            .as_bool()
            .or_else(|| value.as_i64().map(|i| i != 0))
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
    let mut out = Vec::new();
    for app in &config.application_list {
        for dev in &app.device_list {
            if dev.read_metric_list.is_empty() {
                continue;
            }
            if should_stream(
                device_is_valve_class(config, &dev.device_id),
                config.chirpstack.stream_all_devices,
                true, // non-empty read_metric_list checked above
            ) {
                out.push((dev.device_id.clone(), dev.read_metric_list.clone()));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Stream consumer — long-lived per-device, reconnect with backoff.
// ---------------------------------------------------------------------------

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

/// Parse one `LogItem` into `(event_time, decoded_object)` iff it is an uplink
/// (`description == "up"`) with a valid proto timestamp. Returns `None` for
/// non-uplink items, unparseable bodies, or malformed timestamps (same
/// defensive validation as `chirpstack_inventory::log_item_to_uplink`).
fn parse_up_event(item: &LogItem) -> Option<(DateTime<Utc>, serde_json::Value)> {
    if item.description != "up" {
        return None;
    }
    let body: serde_json::Value = serde_json::from_str(&item.body).ok()?;
    let object = body
        .get("object")
        .cloned()
        .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
    let event_time = match item.time.as_ref() {
        Some(ts) if ts.nanos >= 0 && ts.nanos < 1_000_000_000 && ts.seconds >= 0 => {
            DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32)?
        }
        _ => return None,
    };
    Some((event_time, object))
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

/// Open the stream for one device and pump events into storage until the
/// stream closes, errors, or `cancel` fires. Returns `Ok(())` on a clean
/// close / cancellation, `Err` on a connection or stream error (the caller
/// reconnects with backoff).
#[allow(clippy::too_many_arguments)]
async fn connect_and_stream(
    server_address: &str,
    api_token: &str,
    device_id: &str,
    metrics: &[ReadMetric],
    backend: &Arc<dyn StorageBackend>,
    cancel: &CancellationToken,
    seen: &mut HashSet<String>,
    warned: &mut HashSet<String>,
    events_seen: &mut u32,
) -> Result<(), OpcGwStreamError> {
    let channel = Channel::from_shared(grpc_endpoint(server_address))
        .map_err(|e| OpcGwStreamError(format!("invalid server_address: {}", e)))?
        .connect()
        .await
        .map_err(|e| OpcGwStreamError(format!("connect failed: {}", e)))?;

    let interceptor = BearerInterceptor {
        token: api_token.to_string(),
    };
    let mut client = InternalServiceClient::with_interceptor(channel, interceptor);
    let request = Request::new(StreamDeviceEventsRequest {
        dev_eui: device_id.to_string(),
    });
    let response = client
        .stream_device_events(request)
        .await
        .map_err(|e| OpcGwStreamError(format!("stream_device_events: {}", e)))?;
    let mut stream = response.into_inner();

    info!(
        event = "uplink_stream_connected",
        device_id = %device_id,
        "uplink event stream connected"
    );

    loop {
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Ok(()),
            msg = stream.message() => {
                match msg {
                    Ok(Some(item)) => {
                        if let Some((event_time, object)) = parse_up_event(&item) {
                            // Track which configured fields this device actually
                            // emits, and warn (once per field) about ones that
                            // never appear — they won't populate via the stream.
                            *events_seen = events_seen.saturating_add(1);
                            for m in metrics {
                                if object
                                    .get(&m.chirpstack_metric_name)
                                    .map(|v| !v.is_null())
                                    .unwrap_or(false)
                                {
                                    seen.insert(m.chirpstack_metric_name.clone());
                                }
                            }
                            if *events_seen >= ORPHAN_WARN_AFTER_EVENTS {
                                for name in newly_orphaned(metrics, seen, warned) {
                                    warn!(
                                        event = "uplink_metric_never_seen",
                                        device_id = %device_id,
                                        metric = %name,
                                        events_observed = *events_seen,
                                        "configured read_metric absent from every uplink object so far; it will not populate via the stream (DevStatus-sourced battery, or chirpstack_metric_name vs codec field-name mismatch)"
                                    );
                                    warned.insert(name);
                                }
                            }
                            let writes = map_uplink_to_writes(device_id, metrics, &object, event_time);
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
                    }
                    Ok(None) => return Ok(()), // stream closed by server
                    Err(e) => return Err(OpcGwStreamError(format!("stream item error: {}", e))),
                }
            }
        }
    }
}

/// Local error wrapper so the reconnect loop can format a single message
/// without dragging the broader `OpcGwError` taxonomy into transient
/// stream-retry logic.
struct OpcGwStreamError(String);

/// Long-lived consumer for one device: (re)connect with capped exponential
/// backoff until `cancel` fires.
async fn run_device_stream(
    server_address: String,
    api_token: String,
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
            &server_address,
            &api_token,
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

    let server_address = config.chirpstack.server_address.clone();
    let api_token = config.chirpstack.api_token.clone();
    info!(
        event = "uplink_ingestion_start",
        device_count = devices.len(),
        stream_all_devices = config.chirpstack.stream_all_devices,
        "starting uplink event ingestion (valve-class + stream_all_devices)"
    );

    let mut handles = Vec::with_capacity(devices.len());
    for (device_id, metrics) in devices {
        handles.push(tokio::spawn(run_device_stream(
            server_address.clone(),
            api_token.clone(),
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
}

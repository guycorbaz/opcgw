// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Core Storage Data Types
//!
//! This module defines the fundamental data types used by the storage layer,
//! providing type safety and clear semantics for metrics, commands, and gateway state.

// Several types in this module (`CommandFilter`, `ChirpstackStatus`,
// `compute_hash`) are declared for completeness of the storage data model
// and are referenced from downstream stories that have not yet wired them
// into runtime code paths. Allow `dead_code` at module scope rather than
// dropping the types — they are part of the documented API surface.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use std::fmt;
use serde::{Deserialize, Serialize};

/// Metric data types supported by the gateway, carrying the actual measurement
/// payload (Story A-1, Epic A — Storage Payload Migration).
///
/// Represents both the discriminant (Float / Int / Bool / String) and the value
/// itself. The storage trait round-trips the full enum end-to-end from poller
/// write to OPC UA / web UI read.
///
/// **Display** preserves the discriminant-only name rendering ("Float", "Int",
/// …) so existing log volumes and the SQLite `data_type` discriminant-column
/// write path are unaffected. Use `{:?}` (Debug) format if you need value+type
/// rendering in test failure messages.
///
/// **Note on `Copy`:** prior to A-1 this enum was `Copy`; the `String(String)`
/// variant owns heap data and is incompatible with `Copy`. Call sites that
/// relied on implicit copies must use `.clone()` (when multiple consumers
/// need ownership) or borrow `&MetricType` (when read-only access suffices).
/// See the spec file for the call-site audit notes.
///
/// **Note on `FromStr`:** retained for backward compatibility with config
/// parsing (`metric_type = "Float"` in TOML). Parses the type-name only and
/// produces a zero-valued payload (`MetricType::Float(0.0)`, `Int(0)`,
/// `Bool(false)`, `String(String::new())`). The zero payload is intentional —
/// the actual value comes from a separate source (ChirpStack metric ingest).
/// Callers that need to construct a `MetricType` from a string MUST pair
/// `FromStr` with a subsequent value-bearing constructor or pattern-match
/// replace; do NOT treat the `FromStr` output as a final value.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum MetricType {
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
}

impl fmt::Display for MetricType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetricType::Float(_) => write!(f, "Float"),
            MetricType::Int(_) => write!(f, "Int"),
            MetricType::Bool(_) => write!(f, "Bool"),
            MetricType::String(_) => write!(f, "String"),
        }
    }
}

impl std::str::FromStr for MetricType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "float" => Ok(MetricType::Float(0.0)),
            "int" => Ok(MetricType::Int(0)),
            "bool" => Ok(MetricType::Bool(false)),
            "string" => Ok(MetricType::String(String::new())),
            _ => Err(format!("Unknown metric type: {}", s)),
        }
    }
}

/// A metric value with all metadata needed for storage and retrieval.
///
/// Stores a single device metric with its type information and timestamp.
///
/// **Story A-1 / Epic A note:** Post-A-1, `data_type: MetricType` carries the
/// real measurement payload (`Float(f64)` / `Int(i64)` / `Bool(bool)` /
/// `String(String)`). The legacy `value: String` field below is currently
/// dual-storage with the payload — kept temporarily to allow the existing
/// parse-from-string reads in `src/opc_ua_history.rs` and `src/opc_ua.rs` to
/// continue working until those read paths are rewritten to pattern-match the
/// typed payload. **TODO(A-5):** remove `value: String` once `OpcUa::get_value`
/// and `OpcgwHistoryNodeManagerImpl::history_read_raw_modified` consume the
/// typed payload directly.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricValue {
    /// Device identifier
    pub device_id: String,
    /// Metric name
    pub metric_name: String,
    /// Value stored as text (legacy; parse based on data_type).
    /// TODO(A-5): remove once typed-payload reads land.
    pub value: String,
    /// Timestamp of when the metric was collected
    pub timestamp: DateTime<Utc>,
    /// Data type of the metric, type-level payload-bearing (post-A-1, Story
    /// A-1 / Epic A — Storage Payload Migration).
    ///
    /// **Transitional dual-storage caveat (A-1 → A-5):** during the A-1
    /// staging window the typed payload is zero-defaulted at most production
    /// write sites (SqliteBackend `set_metric`/`upsert_metric_value`/
    /// `append_metric_history` use `value.to_string()` = discriminant; OPC UA
    /// Variant→Metric conversion in `src/opc_ua.rs` zero-defaults; ChirpStack
    /// poller arms in `src/chirpstack.rs` stamp `Float(0.0)`/`Int(0)`/etc. —
    /// search `A-3 / A-4` TODOs in earlier revisions of those files). Until those staging gaps are closed,
    /// the **real** measurement is carried by the sibling `value: String`
    /// field. The single exception is the production poller's
    /// `batch_write_metrics` path, which writes `BatchMetricWrite.value`
    /// (real string) — but its `data_type` payload is still discriminant-
    /// flavoured. **TODO(A-5):** once read sites in `src/opc_ua.rs` and
    /// `src/opc_ua_history.rs` are rewritten to consume the typed payload,
    /// this caveat goes away.
    pub data_type: MetricType,
}

/// Command status lifecycle states.
///
/// Represents the different states a device command can be in during its lifecycle.
/// State machine: Pending → Sent → Confirmed or Failed (terminal states)
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CommandStatus {
    /// Command is waiting to be sent
    Pending,
    /// Command has been sent to ChirpStack
    Sent,
    /// Command delivery confirmed by ChirpStack/device
    Confirmed,
    /// Command delivery failed
    Failed,
}

impl fmt::Display for CommandStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandStatus::Pending => write!(f, "Pending"),
            CommandStatus::Sent => write!(f, "Sent"),
            CommandStatus::Confirmed => write!(f, "Confirmed"),
            CommandStatus::Failed => write!(f, "Failed"),
        }
    }
}

impl std::str::FromStr for CommandStatus {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "pending" => Ok(CommandStatus::Pending),
            "sent" => Ok(CommandStatus::Sent),
            "confirmed" => Ok(CommandStatus::Confirmed),
            "failed" => Ok(CommandStatus::Failed),
            _ => Err(format!("Unknown command status: {}", s)),
        }
    }
}

/// Maximum LoRaWAN payload size (bytes). Typical limit is ~250 bytes depending on data rate.
pub const MAX_LORA_PAYLOAD_SIZE: usize = 250;

/// A device command to be sent to a LoRaWAN device.
///
/// Represents a command queued for delivery to a device via ChirpStack.
///
/// # Command Lifecycle
///
/// - **Creation**: Command created with status=Pending, id=0 (temporary)
/// - **Queueing**: Storage backend assigns unique auto-incrementing id
/// - **Sending**: Status changed to Sent when successfully delivered to ChirpStack
/// - **Terminal**: Status=Failed if delivery fails (error_message populated)
///
/// # Note on ID Assignment
///
/// The `id` field should be assigned by the storage backend when the command is queued.
/// Callers should set id=0 as a placeholder. The storage queue must implement atomic
/// ID generation (e.g., via Mutex + counter) to ensure uniqueness in multi-threaded scenarios.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceCommand {
    /// Auto-incrementing command ID (assigned by storage backend, caller uses 0)
    pub id: u64,
    /// Target device identifier
    pub device_id: String,
    /// Command payload as bytes (validated against MAX_LORA_PAYLOAD_SIZE)
    pub payload: Vec<u8>,
    /// LoRaWAN frame port (1-223, validated via validate_f_port)
    pub f_port: u8,
    /// Current status of the command (Pending → Sent → Failed state machine)
    pub status: CommandStatus,
    /// When the command was created
    pub created_at: DateTime<Utc>,
    /// Error message if delivery failed
    pub error_message: Option<String>,
}

impl DeviceCommand {
    /// Validates that f_port is in the valid LoRaWAN range (1-223).
    pub fn validate_f_port(f_port: u8) -> bool {
        (1..=223).contains(&f_port)
    }

    /// Validates that payload size is within LoRaWAN limits.
    pub fn validate_payload_size(payload: &[u8]) -> bool {
        payload.len() <= MAX_LORA_PAYLOAD_SIZE
    }
}

/// High-level command with rich metadata for Story 3-1 (FIFO queue) integration.
///
/// This struct represents a command queued for delivery to a LoRaWAN device.
/// It extends DeviceCommand with additional metadata needed for:
/// - Parameter validation (Story 3-2)
/// - Delivery status tracking (Story 3-3)
/// - Deduplication via SHA256 hash
/// - OPC UA integration (metric_id reference)
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Command {
    /// Auto-incrementing command ID (assigned by storage backend)
    pub id: u64,
    /// Target device identifier from ChirpStack
    pub device_id: String,
    /// Metric ID in OPC UA address space (for tracking in OPC UA)
    pub metric_id: String,
    /// Human-readable command name (e.g., "toggle_relay", "set_temperature")
    pub command_name: String,
    /// Command parameters as JSON (validated by Story 3-2)
    pub parameters: serde_json::Value,
    /// Timestamp when command was enqueued
    pub enqueued_at: DateTime<Utc>,
    /// Timestamp when command was sent to ChirpStack
    pub sent_at: Option<DateTime<Utc>>,
    /// Timestamp when command was confirmed by device/ChirpStack
    pub confirmed_at: Option<DateTime<Utc>>,
    /// Current status in lifecycle: Pending → Sent → Confirmed or Failed
    pub status: CommandStatus,
    /// Error message if delivery failed
    pub error_message: Option<String>,
    /// SHA256 hash of (device_id + command_name + parameters_json) for deduplication
    pub command_hash: String,
    /// ChirpStack API result ID (for mapping responses back to local commands)
    pub chirpstack_result_id: Option<String>,
}

/// Filter criteria for querying commands from storage.
///
/// Used with StorageBackend::list_commands() to retrieve filtered command lists.
#[derive(Clone, Debug, Default)]
pub struct CommandFilter {
    /// Filter by device_id (exact match)
    pub device_id: Option<String>,
    /// Filter by status (exact match)
    pub status: Option<CommandStatus>,
    /// Filter by command_name (substring match)
    pub command_name_contains: Option<String>,
    /// Include commands older than N days (for cleanup queries)
    pub older_than_days: Option<u32>,
}

impl Command {
    /// Computes SHA256 hash of (device_id + command_name + parameters_json) for deduplication.
    ///
    /// This hash is used to detect duplicate commands and prevent requeuing.
    /// The hash is computed from the command's semantic content, not its metadata (timestamps, status).
    pub fn compute_hash(device_id: &str, command_name: &str, parameters: &serde_json::Value) -> String {
        use sha2::{Sha256, Digest};

        let params_json = parameters.to_string();
        let content = format!("{}{}{}", device_id, command_name, params_json);
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        format!("{:x}", hasher.finalize())
    }
}

/// Gateway health and ChirpStack server connection status.
///
/// Tracks the operational status of the ChirpStack connection and gateway health.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ChirpstackStatus {
    /// Whether the ChirpStack server is available and responding
    pub server_available: bool,
    /// Timestamp of the last successful poll
    pub last_poll_time: Option<DateTime<Utc>>,
    /// Number of errors encountered since last successful connection (wraps at i32::MAX)
    pub error_count: i32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_type_display() {
        // Display preserves the discriminant-only rendering post-A-1 — the
        // payload is intentionally NOT included in `{}` format.
        assert_eq!(MetricType::Float(1.5).to_string(), "Float");
        assert_eq!(MetricType::Int(42).to_string(), "Int");
        assert_eq!(MetricType::Bool(true).to_string(), "Bool");
        assert_eq!(MetricType::String("hi".into()).to_string(), "String");
    }

    /// Contract pin: `Display` MUST emit the discriminant only — never the
    /// payload — regardless of payload contents (including boundary values
    /// like NaN, Infinity, i64::MIN/MAX, empty strings, embedded NUL).
    ///
    /// **Load-bearing for (A-1 staging — see `TODO(A-2)` in `sqlite.rs`):**
    /// - SQLite `data_type` column: every backend write path
    ///   (`set_metric`, `upsert_metric_value`, `append_metric_history`,
    ///   `batch_write_metrics`) populates this column via
    ///   `value.to_string()`.
    /// - SQLite `value` column on `upsert_metric_value` /
    ///   `append_metric_history`: also written via `value.to_string()` (the
    ///   discriminant) per option-(b) A-1 staging. So today the `value`
    ///   column is grep-distinguishable from the `data_type` column on
    ///   `set_metric`/`batch_write_metrics` rows only (which embed JSON / a
    ///   real string respectively).
    /// - Log output volume — many `info!` / `debug!` / `trace!` sites format
    ///   metric types with `{}` and expect a single short token.
    ///
    /// If a future change extends Display to render the payload (e.g.
    /// `"Float(23.5)"`), the SQLite write paths and grep contracts MUST be
    /// migrated in the same change.
    #[test]
    fn test_metric_type_display_pins_discriminant_only_contract() {
        // Boundary payloads must still render as bare discriminants.
        assert_eq!(MetricType::Float(f64::NAN).to_string(), "Float");
        assert_eq!(MetricType::Float(f64::INFINITY).to_string(), "Float");
        assert_eq!(MetricType::Float(f64::NEG_INFINITY).to_string(), "Float");
        assert_eq!(MetricType::Float(-0.0).to_string(), "Float");
        assert_eq!(MetricType::Float(f64::MAX).to_string(), "Float");
        assert_eq!(MetricType::Int(i64::MIN).to_string(), "Int");
        assert_eq!(MetricType::Int(i64::MAX).to_string(), "Int");
        assert_eq!(MetricType::Bool(false).to_string(), "Bool");
        assert_eq!(MetricType::Bool(true).to_string(), "Bool");
        assert_eq!(MetricType::String(String::new()).to_string(), "String");
        assert_eq!(MetricType::String("\0\u{FFFD}embedded".into()).to_string(), "String");
        assert_eq!(
            MetricType::String("a".repeat(10_000)).to_string(),
            "String"
        );
    }

    #[test]
    fn test_metric_type_from_str() {
        // FromStr produces zero-valued payloads per the A-1 contract; callers
        // pair this with a separate value source (ChirpStack ingest).
        assert_eq!("float".parse::<MetricType>().unwrap(), MetricType::Float(0.0));
        assert_eq!("int".parse::<MetricType>().unwrap(), MetricType::Int(0));
        assert_eq!("bool".parse::<MetricType>().unwrap(), MetricType::Bool(false));
        assert_eq!(
            "string".parse::<MetricType>().unwrap(),
            MetricType::String(String::new())
        );
        assert!("invalid".parse::<MetricType>().is_err());
    }

    #[test]
    fn test_metric_type_payload_roundtrip() {
        // A-1 AC#6 (type-level shape): every variant round-trips its payload
        // through Clone + PartialEq without lossy conversion.
        let float = MetricType::Float(23.5);
        let int = MetricType::Int(42);
        let bool_ = MetricType::Bool(true);
        let string = MetricType::String("OK".to_string());
        assert_eq!(float.clone(), MetricType::Float(23.5));
        assert_eq!(int.clone(), MetricType::Int(42));
        assert_eq!(bool_.clone(), MetricType::Bool(true));
        assert_eq!(string.clone(), MetricType::String("OK".to_string()));
    }

    #[test]
    fn test_metric_type_payload_roundtrip_boundary_values() {
        // A-1 AC#6 boundary coverage: payload integrity across edge cases of
        // each variant type. Note: `Float(NaN)` is intentionally checked via
        // pattern destructuring + `f.is_nan()` because `NaN == NaN` is false
        // under PartialEq (IEEE 754).
        let nan = MetricType::Float(f64::NAN).clone();
        match nan {
            MetricType::Float(f) => assert!(f.is_nan(), "NaN payload must survive Clone"),
            _ => panic!("Float variant not preserved through Clone"),
        }

        assert_eq!(
            MetricType::Float(f64::INFINITY).clone(),
            MetricType::Float(f64::INFINITY)
        );
        assert_eq!(
            MetricType::Float(f64::NEG_INFINITY).clone(),
            MetricType::Float(f64::NEG_INFINITY)
        );
        // Signed zero — PartialEq treats +0.0 == -0.0 as true, so we check
        // the bit pattern explicitly to pin signed-zero preservation.
        let neg_zero = MetricType::Float(-0.0).clone();
        if let MetricType::Float(f) = neg_zero {
            assert_eq!(f.to_bits(), (-0.0f64).to_bits(), "signed-zero payload must survive Clone");
        } else {
            panic!("Float variant not preserved through Clone");
        }
        assert_eq!(
            MetricType::Float(f64::MAX).clone(),
            MetricType::Float(f64::MAX)
        );

        assert_eq!(MetricType::Int(i64::MIN).clone(), MetricType::Int(i64::MIN));
        assert_eq!(MetricType::Int(i64::MAX).clone(), MetricType::Int(i64::MAX));
        assert_eq!(MetricType::Int(0).clone(), MetricType::Int(0));

        // Bool covers both true and false (2-valued domain — both ends matter).
        assert_eq!(MetricType::Bool(false).clone(), MetricType::Bool(false));
        assert_eq!(MetricType::Bool(true).clone(), MetricType::Bool(true));

        assert_eq!(
            MetricType::String(String::new()).clone(),
            MetricType::String(String::new())
        );
        assert_eq!(
            MetricType::String("\0\u{FFFD}embedded".into()).clone(),
            MetricType::String("\0\u{FFFD}embedded".into())
        );
        let long = "a".repeat(10_000);
        assert_eq!(
            MetricType::String(long.clone()).clone(),
            MetricType::String(long)
        );
    }

    #[test]
    fn test_command_status_display() {
        assert_eq!(CommandStatus::Pending.to_string(), "Pending");
        assert_eq!(CommandStatus::Sent.to_string(), "Sent");
        assert_eq!(CommandStatus::Confirmed.to_string(), "Confirmed");
        assert_eq!(CommandStatus::Failed.to_string(), "Failed");
    }

    #[test]
    fn test_command_status_from_str() {
        assert_eq!(
            "pending".parse::<CommandStatus>().unwrap(),
            CommandStatus::Pending
        );
        assert_eq!("sent".parse::<CommandStatus>().unwrap(), CommandStatus::Sent);
        assert_eq!(
            "confirmed".parse::<CommandStatus>().unwrap(),
            CommandStatus::Confirmed
        );
        assert_eq!(
            "failed".parse::<CommandStatus>().unwrap(),
            CommandStatus::Failed
        );
        assert!("invalid".parse::<CommandStatus>().is_err());
    }

    #[test]
    fn test_device_command_f_port_validation() {
        // Valid range: 1-223
        assert!(DeviceCommand::validate_f_port(1));
        assert!(DeviceCommand::validate_f_port(100));
        assert!(DeviceCommand::validate_f_port(223));

        // Invalid values
        assert!(!DeviceCommand::validate_f_port(0));
        assert!(!DeviceCommand::validate_f_port(224));
        assert!(!DeviceCommand::validate_f_port(255));
    }

    #[test]
    fn test_chirpstack_status_default() {
        let status = ChirpstackStatus::default();
        assert!(!status.server_available);
        assert!(status.last_poll_time.is_none());
        assert_eq!(status.error_count, 0);
    }

    #[test]
    fn test_metric_value_creation() {
        let now = Utc::now();
        let metric = MetricValue {
            device_id: "device_123".to_string(),
            metric_name: "temperature".to_string(),
            value: "23.5".to_string(),
            timestamp: now,
            data_type: MetricType::Float(23.5),
        };

        assert_eq!(metric.device_id, "device_123");
        assert_eq!(metric.metric_name, "temperature");
        assert_eq!(metric.value, "23.5");
        assert_eq!(metric.data_type, MetricType::Float(23.5));
    }

    #[test]
    fn test_device_command_creation() {
        let now = Utc::now();
        let cmd = DeviceCommand {
            id: 1,
            device_id: "device_123".to_string(),
            payload: vec![0x01, 0x02, 0x03],
            f_port: 10,
            status: CommandStatus::Pending,
            created_at: now,
            error_message: None,
        };

        assert_eq!(cmd.id, 1);
        assert_eq!(cmd.device_id, "device_123");
        assert_eq!(cmd.f_port, 10);
        assert_eq!(cmd.status, CommandStatus::Pending);
        assert!(cmd.error_message.is_none());
    }
}

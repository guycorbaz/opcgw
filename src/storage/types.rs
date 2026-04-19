// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Core Storage Data Types
//!
//! This module defines the fundamental data types used by the storage layer,
//! providing type safety and clear semantics for metrics, commands, and gateway state.

use chrono::{DateTime, Utc};
use std::fmt;
use serde::{Deserialize, Serialize};

/// Metric data types supported by the gateway.
///
/// Represents different types of values that can be stored and exposed via OPC UA.
/// This enum is Copy and implements Display for easy serialization/logging.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MetricType {
    Float,
    Int,
    Bool,
    String,
}

impl fmt::Display for MetricType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MetricType::Float => write!(f, "Float"),
            MetricType::Int => write!(f, "Int"),
            MetricType::Bool => write!(f, "Bool"),
            MetricType::String => write!(f, "String"),
        }
    }
}

impl std::str::FromStr for MetricType {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "float" => Ok(MetricType::Float),
            "int" => Ok(MetricType::Int),
            "bool" => Ok(MetricType::Bool),
            "string" => Ok(MetricType::String),
            _ => Err(format!("Unknown metric type: {}", s)),
        }
    }
}

/// A metric value with all metadata needed for storage and retrieval.
///
/// Stores a single device metric with its type information and timestamp.
/// The value is stored as text and parsed based on the data_type field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MetricValue {
    /// Device identifier
    pub device_id: String,
    /// Metric name
    pub metric_name: String,
    /// Value stored as text (parse based on data_type)
    pub value: String,
    /// Timestamp of when the metric was collected
    pub timestamp: DateTime<Utc>,
    /// Data type of the metric
    pub data_type: MetricType,
}

/// Command status lifecycle states.
///
/// Represents the different states a device command can be in during its lifecycle.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum CommandStatus {
    /// Command is waiting to be sent
    Pending,
    /// Command has been sent to ChirpStack
    Sent,
    /// Command delivery failed
    Failed,
}

impl fmt::Display for CommandStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandStatus::Pending => write!(f, "Pending"),
            CommandStatus::Sent => write!(f, "Sent"),
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
        f_port >= 1 && f_port <= 223
    }

    /// Validates that payload size is within LoRaWAN limits.
    pub fn validate_payload_size(payload: &[u8]) -> bool {
        payload.len() <= MAX_LORA_PAYLOAD_SIZE
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
    /// Number of errors encountered since last successful connection
    pub error_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_type_display() {
        assert_eq!(MetricType::Float.to_string(), "Float");
        assert_eq!(MetricType::Int.to_string(), "Int");
        assert_eq!(MetricType::Bool.to_string(), "Bool");
        assert_eq!(MetricType::String.to_string(), "String");
    }

    #[test]
    fn test_metric_type_from_str() {
        assert_eq!("float".parse::<MetricType>().unwrap(), MetricType::Float);
        assert_eq!("int".parse::<MetricType>().unwrap(), MetricType::Int);
        assert_eq!("bool".parse::<MetricType>().unwrap(), MetricType::Bool);
        assert_eq!("string".parse::<MetricType>().unwrap(), MetricType::String);
        assert!("invalid".parse::<MetricType>().is_err());
    }

    #[test]
    fn test_command_status_display() {
        assert_eq!(CommandStatus::Pending.to_string(), "Pending");
        assert_eq!(CommandStatus::Sent.to_string(), "Sent");
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
            data_type: MetricType::Float,
        };

        assert_eq!(metric.device_id, "device_123");
        assert_eq!(metric.metric_name, "temperature");
        assert_eq!(metric.value, "23.5");
        assert_eq!(metric.data_type, MetricType::Float);
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

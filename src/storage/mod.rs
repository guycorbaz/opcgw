// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Storage Management Module
//!
//! This module provides a centralized storage service for the OPC UA ChirpStack Gateway.
//! It serves as the data layer that:
//! - Stores device metrics collected from ChirpStack
//! - Provides data access for the OPC UA server
//! - Maintains ChirpStack server status information

// Several methods on the legacy `Storage` struct (`mark_command_sent`,
// `mark_command_confirmed`, `dump_storage`, `get_device_command_queue`,
// etc.) are scaffolded for migration paths that are still in flight as the
// project moves to the SqliteBackend trait-object model. Allow `dead_code`
// at module scope so the scaffold doesn't fail clippy while the migration
// is being staged.
#![allow(dead_code)]
//! - Manages device and metric lifecycle
//!
//! # Architecture
//!
//! The storage system uses an in-memory approach with HashMap-based indexing for
//! fast device and metric lookups. Data is organized hierarchically:
//! ```text
//! Storage
//! ├── ChirpStack Status (availability, response time)
//! └── Devices (by device_id)
//!     ├── Device Name
//!     └── Metrics (by metric_name)
//!         └── Metric Values (typed)
//! ```
//!
//! # Thread Safety
//!
//! This module is designed to be used with Tokio's async runtime. The Storage
//! struct itself is not thread-safe; use Arc<Mutex<Storage>> for concurrent access.
//!
//! # Usage
//!
//! ```rust,no_run
//! use crate::storage::{Storage, MetricType};
//! use crate::config::AppConfig;
//!
//! let config = AppConfig::new()?;
//! let mut storage = Storage::new(&config);
//!
//! // Set a metric value
//! storage.set_metric_value(
//!     &"device_123".to_string(),
//!     "temperature",
//!     MetricType::Float(23.5)
//! );
//!
//! // Retrieve a metric value
//! if let Some(value) = storage.get_metric_value("device_123", "temperature") {
//!     println!("Temperature: {:?}", value);
//! }
//! ```

pub mod types;
pub mod memory;
pub mod sqlite;
pub mod schema;
pub mod pool;

pub use types::{ChirpstackStatus, Command, CommandFilter, CommandStatus, DeviceCommand, MetricType, MetricValue, MAX_LORA_PAYLOAD_SIZE};
pub use sqlite::SqliteBackend;
pub use pool::ConnectionPool;

use crate::config::{OpcMetricTypeConfig, AppConfig};
use crate::utils::*;
use chrono::{DateTime, Utc};
use tracing::{debug, error, trace};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use rusqlite::types::{FromSql, ToSql, FromSqlResult, ValueRef};
use rusqlite::Result as SqliteResult;

/// StorageBackend trait defining the interface for all storage implementations.
///
/// This trait provides a clean abstraction for storage backends, allowing different
/// implementations (in-memory, SQLite, etc.) to follow the same contract.
/// All implementations must be Send + Sync for safe usage across async task boundaries
/// and behind Arc pointers.
///
/// # Method Categories
///
/// The trait is organized into three main categories:
/// - **Metric Operations**: Get and set individual metric values
/// - **Gateway Status**: Manage ChirpStack server connection status
/// - **Command Queue**: Queue and retrieve device commands
///
/// # Thread Safety
///
/// All implementations must be thread-safe (Send + Sync) to support usage behind
/// Arc<dyn StorageBackend> for sharing across Tokio tasks.
///
/// # Error Handling
///
/// All trait methods return Result<T, OpcGwError> for consistent error handling.
/// Error context should include the operation type, entity reference, and reason.
///
/// # Example
///
/// ```rust,no_run
/// # use crate::utils::OpcGwError;
/// # use crate::storage::{StorageBackend, MetricType, ChirpstackStatus, DeviceCommand, CommandStatus};
/// # async fn example(backend: Arc<dyn StorageBackend>) -> Result<(), OpcGwError> {
/// // Get a metric value
/// let value = backend.get_metric("device_123", "temperature")?;
///
/// // Set a metric value
/// backend.set_metric("device_123", "humidity", MetricType::Float(65.5))?;
///
/// // Get gateway status
/// let status = backend.get_status()?;
///
/// // Queue a command
/// let cmd = DeviceCommand {
///     device_id: "device_123".to_string(),
///     confirmed: true,
///     f_port: 10,
///     data: vec![0x01, 0x02],
/// };
/// backend.queue_command(cmd)?;
/// # Ok(())
/// # }
/// ```
/// Represents a single metric write operation in a batch.
///
/// Used with `batch_write_metrics()` to group multiple metric updates
/// into a single atomic transaction. Includes both the current metric value
/// (for UPSERT) and the historical record (for append-only audit log).
#[derive(Clone, Debug)]
pub struct BatchMetricWrite {
    /// Unique device identifier from ChirpStack
    pub device_id: String,
    /// Metric name as defined in ChirpStack
    pub metric_name: String,
    /// The metric value as a string (numeric value for Float/Int, boolean for Bool, text for String)
    pub value: String,
    /// The metric type (Int, Float, Bool, String)
    pub data_type: MetricType,
    /// Timestamp when this metric was measured (system time)
    pub timestamp: std::time::SystemTime,
}

pub trait StorageBackend: Send + Sync {
    // ===== Metric Operations =====

    /// Retrieves the current value of a specific metric for a device.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier for the device
    /// * `metric_name` - The name of the metric to retrieve
    ///
    /// # Returns
    ///
    /// * `Ok(Some(MetricType))` - The metric value if found
    /// * `Ok(None)` - If the device or metric does not exist
    /// * `Err(OpcGwError)` - If an error occurs during retrieval
    ///
    /// # Error Cases
    ///
    /// - **Storage error**: Database connectivity issues, corrupted data
    /// - **Device not found**: The device_id references a non-existent device
    /// - **Metric not found**: The metric_name does not exist for the device
    fn get_metric(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricType>, OpcGwError>;

    /// Retrieves the complete metric value (type and data) for counter monotonic checking.
    ///
    /// This method is optimized for the counter monotonic check use case, returning
    /// both the metric type and its numeric value for efficient comparison of counter resets.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier for the device
    /// * `metric_name` - The name of the metric to retrieve
    ///
    /// # Returns
    ///
    /// * `Ok(Some(MetricValue))` - The metric with both type and value if found
    /// * `Ok(None)` - If the device or metric does not exist
    /// * `Err(OpcGwError)` - If an error occurs during retrieval
    fn get_metric_value(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricValue>, OpcGwError>;

    /// Updates the value of a specific metric for a device.
    ///
    /// If the metric does not exist, it will be created with the specified value.
    /// If the device does not exist, returns an error.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier for the device
    /// * `metric_name` - The name of the metric to update
    /// * `value` - The new metric value
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the metric was successfully updated
    /// * `Err(OpcGwError)` - If an error occurs during update
    ///
    /// # Error Cases
    ///
    /// - **Device not found**: The device_id references a non-existent device
    /// - **Storage error**: Database connectivity issues, write failures
    /// - **Type mismatch**: If the backend enforces type consistency
    fn set_metric(&self, device_id: &str, metric_name: &str, value: MetricType) -> Result<(), OpcGwError>;

    // ===== Gateway Status Operations =====

    /// Retrieves the current ChirpStack server connection status.
    ///
    /// # Returns
    ///
    /// * `Ok(ChirpstackStatus)` - The current gateway status
    /// * `Err(OpcGwError)` - If an error occurs during retrieval
    ///
    /// # Error Cases
    ///
    /// - **Storage error**: Database connectivity issues
    /// - **Corrupted data**: Status data cannot be deserialized
    fn get_status(&self) -> Result<ChirpstackStatus, OpcGwError>;

    /// Updates the ChirpStack server connection status.
    ///
    /// This is typically called by the ChirpStack poller to reflect the current
    /// health and performance of the server connection.
    ///
    /// # Arguments
    ///
    /// * `status` - The new gateway status
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the status was successfully updated
    /// * `Err(OpcGwError)` - If an error occurs during update
    ///
    /// # Error Cases
    ///
    /// - **Storage error**: Database connectivity issues, write failures
    fn update_status(&self, status: ChirpstackStatus) -> Result<(), OpcGwError>;

    // ===== Command Queue Operations =====

    /// Adds a new command to the device command queue.
    ///
    /// Commands are queued for delivery to ChirpStack devices. The backend assigns
    /// a unique command ID which is returned (or available via get_pending_commands).
    ///
    /// # Arguments
    ///
    /// * `command` - The command to queue
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the command was successfully queued
    /// * `Err(OpcGwError)` - If an error occurs during queueing
    ///
    /// # Error Cases
    ///
    /// - **Device not found**: The device_id references a non-existent device
    /// - **Storage error**: Database connectivity issues, write failures
    /// - **Queue full**: If the backend has a queue size limit
    fn queue_command(&self, command: DeviceCommand) -> Result<(), OpcGwError>;

    /// Retrieves all pending commands from the queue.
    ///
    /// Returns a vector of commands that are ready for delivery but have not yet
    /// been sent to the ChirpStack API.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<DeviceCommand>)` - List of pending commands (may be empty)
    /// * `Err(OpcGwError)` - If an error occurs during retrieval
    ///
    /// # Error Cases
    ///
    /// - **Storage error**: Database connectivity issues
    fn get_pending_commands(&self) -> Result<Vec<DeviceCommand>, OpcGwError>;

    /// Updates the status of a queued command.
    ///
    /// Called after a command has been delivered or processing has failed.
    /// The command remains in storage for audit/historical purposes.
    /// Supports error_message for Failed status tracking.
    ///
    /// # Arguments
    ///
    /// * `command_id` - The unique identifier of the command
    /// * `status` - The new command status
    /// * `error_message` - Optional error description if status is Failed
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the status was successfully updated
    /// * `Err(OpcGwError)` - If an error occurs during update
    ///
    /// # Error Cases
    ///
    /// - **Command not found**: The command_id references a non-existent command
    /// - **Storage error**: Database connectivity issues, write failures
    /// - **Invalid state transition**: If the status transition is not allowed
    fn update_command_status(&self, command_id: u64, status: CommandStatus, error_message: Option<String>) -> Result<(), OpcGwError>;

    // ===== Metric Persistence Operations =====

    /// Inserts or updates a metric value with UPSERT semantics.
    ///
    /// This method persists a metric value to durable storage using UPSERT (INSERT OR REPLACE)
    /// semantics. If the metric exists, it is updated; if not, it is created.
    /// The `created_at` timestamp is preserved across updates.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier for the device
    /// * `metric_name` - The name of the metric to persist
    /// * `value` - The metric value to store
    /// * `now_ts` - The current timestamp (system time) for `updated_at`
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the metric was successfully persisted
    /// * `Err(OpcGwError)` - If an error occurs during persistence
    ///
    /// # Error Cases
    ///
    /// - **Storage error**: Database connectivity issues, write failures
    /// - **Device not found**: The device_id references a non-existent device (may be acceptable depending on backend)
    ///
    /// # UPSERT Semantics
    ///
    /// - **First insert**: Creates new row with `created_at` = `now_ts`, `updated_at` = `now_ts`
    /// - **Subsequent updates**: Modifies value and `updated_at` = `now_ts`; preserves original `created_at`
    /// - **Atomicity**: Operation is atomic — either fully succeeds or fully fails (no partial writes)
    fn upsert_metric_value(&self, device_id: &str, metric_name: &str, value: &MetricType, now_ts: std::time::SystemTime) -> Result<(), OpcGwError>;

    /// Append a historical metric record to the append-only audit log.
    ///
    /// This method appends a new row to metric_history without updating existing rows. The append-only semantics
    /// ensure an immutable audit trail of all metric changes over time.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier for the device
    /// * `metric_name` - The name of the metric to append
    /// * `value` - The metric value to store
    /// * `timestamp` - The system timestamp for this metric measurement
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the metric was successfully appended
    /// * `Err(OpcGwError)` - If an error occurs during append
    ///
    /// # Append-Only Semantics
    ///
    /// - **Never updates**: New row inserted, never modifies existing rows
    /// - **Multiple entries allowed**: (device_id, metric_name) can have multiple rows at different timestamps
    /// - **Timestamp ordered**: Rows maintain insertion order by timestamp
    /// - **Audit trail**: Creates immutable historical record for compliance and trend analysis
    fn append_metric_history(&self, device_id: &str, metric_name: &str, value: &MetricType, timestamp: std::time::SystemTime) -> Result<(), OpcGwError>;

    /// Batch write multiple metrics in a single atomic transaction.
    ///
    /// This method persists all metrics from a single poll cycle using UPSERT + append-only semantics
    /// in a single transaction. If any operation fails, the entire transaction rolls back and no
    /// partial data is persisted.
    ///
    /// # Arguments
    ///
    /// * `metrics` - Vector of `BatchMetricWrite` containing device_id, metric_name, value, and timestamp
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all metrics were successfully persisted atomically
    /// * `Err(OpcGwError)` - If any operation fails; entire transaction rolled back
    ///
    /// # Atomicity Guarantee
    ///
    /// All metrics succeed or all fail together — no partial writes to the database.
    /// This satisfies AC#3 from Story 2-3b (transactional consistency for poll cycles).
    ///
    /// # Performance
    ///
    /// Batch writes execute in a single transaction with lower overhead than per-metric calls.
    /// Expected performance: ~100-200ms for 400 metrics (vs. ~400-500ms per-metric).
    ///
    /// # Backward Compatibility
    ///
    /// This method is additive; existing single-metric methods (upsert_metric_value, append_metric_history)
    /// remain unchanged. Backends not yet supporting batching can stub with a loop over individual operations.
    fn batch_write_metrics(&self, metrics: Vec<BatchMetricWrite>) -> Result<(), OpcGwError>;

    /// Load all persisted metrics from storage for gateway startup restore.
    ///
    /// This method retrieves all metric values from the metric_values table, enabling
    /// the gateway to restore metrics into OPC UA on startup. Called during the restore phase
    /// before starting the poller, ensuring OPC UA clients see valid cached data immediately.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<MetricValue>)` - All metrics in storage (may be empty if no metrics persisted)
    /// * `Err(OpcGwError)` - If an error occurs during retrieval
    ///
    /// # Error Cases
    ///
    /// - **Storage error**: Database connectivity issues or query failures
    /// - **Corrupted data**: Unparseable metric types or values
    ///
    /// # Performance
    ///
    /// Expected to complete in < 3 seconds for 100 metrics.
    /// Suitable for startup path where quick initialization is important.
    ///
    /// # Semantics
    ///
    /// Returns metrics in arbitrary order (no guarantees about ordering).
    /// All metrics returned have valid (device_id, metric_name, value, data_type, timestamp).
    fn load_all_metrics(&self) -> Result<Vec<MetricValue>, OpcGwError>;

    /// Prune historical metrics older than configured retention period.
    ///
    /// Deletes rows from metric_history table where timestamp is older than (now - retention_days),
    /// based on the retention policy configured in the retention_config table.
    ///
    /// # Returns
    ///
    /// * `Ok(u32)` - Number of rows deleted
    /// * `Err(OpcGwError)` - If an error occurs during pruning
    ///
    /// # Error Cases
    ///
    /// - **Database locked**: Another process is writing; error logged, prune skipped for this cycle
    /// - **Missing retention_config**: No retention policy found for metric_history
    /// - **Invalid retention_days**: Negative or corrupted value in retention_config
    /// - **Storage error**: Database connectivity issues
    ///
    /// # Semantics
    ///
    /// - Rows with NULL timestamps are NOT deleted (safety guardrail per AC#2)
    /// - Uses parameterized query to prevent SQL injection
    /// - Returns 0 if no rows meet the deletion criteria
    /// - Reads retention_days fresh from retention_config at each call (never cached)
    fn prune_metric_history(&self) -> Result<u32, OpcGwError>;

    // ===== High-Level Command Queue Operations (Story 3-1) =====

    /// Enqueues a command for delivery to a device.
    ///
    /// Adds a new command to the persistent FIFO queue with status=Pending.
    /// The backend assigns a unique command ID and returns it.
    /// Deduplication is enforced via command_hash to prevent requeuing identical commands.
    ///
    /// # Arguments
    ///
    /// * `command` - The command to enqueue (id should be 0, backend assigns actual id)
    ///
    /// # Returns
    ///
    /// * `Ok(u64)` - The assigned command ID
    /// * `Err(OpcGwError)` - If enqueue fails (queue full, duplicate, database error)
    fn enqueue_command(&self, command: Command) -> Result<u64, OpcGwError>;

    /// Dequeues the next pending command in FIFO order.
    ///
    /// Returns the oldest pending command (by creation timestamp/ROWID).
    /// Does NOT remove it from storage (for audit trail), but marks status as Sent.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(Command))` - The next pending command
    /// * `Ok(None)` - If queue is empty (no pending commands)
    /// * `Err(OpcGwError)` - If database error occurs
    fn dequeue_command(&self) -> Result<Option<Command>, OpcGwError>;

    /// Lists commands matching filter criteria.
    ///
    /// Supports filtering by device_id, status, command_name (substring), or age.
    /// Returns results in FIFO order (by creation timestamp).
    ///
    /// # Arguments
    ///
    /// * `filter` - CommandFilter with optional criteria
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<Command>)` - Matching commands (may be empty)
    /// * `Err(OpcGwError)` - If database error occurs
    fn list_commands(&self, filter: &CommandFilter) -> Result<Vec<Command>, OpcGwError>;

    /// Returns the number of pending commands in the queue.
    ///
    /// Used for operational visibility and capacity monitoring.
    ///
    /// # Returns
    ///
    /// * `Ok(usize)` - Count of pending commands
    /// * `Err(OpcGwError)` - If database error occurs
    fn get_queue_depth(&self) -> Result<usize, OpcGwError>;

    // ===== Command Delivery Status Operations (Story 3-3) =====

    /// Marks a command as sent with ChirpStack result ID for tracking.
    ///
    /// Updates a pending command to "Sent" status and records the ChirpStack result ID
    /// for mapping delivery confirmations back to local commands. Sets sent_at timestamp.
    ///
    /// # Arguments
    ///
    /// * `command_id` - The unique identifier of the command
    /// * `chirpstack_result_id` - The result ID from ChirpStack API response (for confirmation mapping)
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the status was successfully updated
    /// * `Err(OpcGwError)` - If command not found or database error occurs
    fn mark_command_sent(&self, command_id: u64, chirpstack_result_id: &str) -> Result<(), OpcGwError>;

    /// Marks a command as confirmed by ChirpStack/device.
    ///
    /// Updates a sent command to "Confirmed" status and sets confirmed_at timestamp.
    /// Called by CommandStatusPoller when ChirpStack confirms delivery.
    ///
    /// # Arguments
    ///
    /// * `command_id` - The unique identifier of the command
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the status was successfully updated
    /// * `Err(OpcGwError)` - If command not found or database error occurs
    fn mark_command_confirmed(&self, command_id: u64) -> Result<(), OpcGwError>;

    /// Marks a command as failed with optional error message.
    ///
    /// Updates a sent command to "Failed" status with an error message for diagnostics.
    /// Called by timeout handler or when ChirpStack reports delivery failure.
    ///
    /// # Arguments
    ///
    /// * `command_id` - The unique identifier of the command
    /// * `error_message` - Human-readable error description (e.g., "Confirmation timeout")
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the status was successfully updated
    /// * `Err(OpcGwError)` - If command not found or database error occurs
    fn mark_command_failed(&self, command_id: u64, error_message: &str) -> Result<(), OpcGwError>;

    /// Finds all sent commands awaiting confirmation from ChirpStack.
    ///
    /// Returns commands in "Sent" status that don't yet have confirmed_at timestamps.
    /// Used by CommandStatusPoller to poll ChirpStack for delivery confirmations.
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<Command>)` - All sent commands awaiting confirmation (may be empty)
    /// * `Err(OpcGwError)` - If database error occurs
    fn find_pending_confirmations(&self) -> Result<Vec<Command>, OpcGwError>;

    /// Finds all sent commands that have timed out awaiting confirmation.
    ///
    /// Returns commands in "Sent" status where sent_at is older than ttl_secs.
    /// Used by timeout handler to mark expired commands as failed.
    ///
    /// # Arguments
    ///
    /// * `ttl_secs` - Time-to-live in seconds (e.g., 60 for 60-second timeout)
    ///
    /// # Returns
    ///
    /// * `Ok(Vec<Command>)` - All timed-out commands (may be empty)
    /// * `Err(OpcGwError)` - If database error occurs
    fn find_timed_out_commands(&self, ttl_secs: u32) -> Result<Vec<Command>, OpcGwError>;

    // ===== Gateway Health Metrics Operations =====

    /// Updates gateway health metrics (last poll timestamp, error count, ChirpStack availability).
    ///
    /// This method persists operational health information to the gateway_status table.
    /// Called after each poll cycle by the ChirpStack poller to maintain current health state.
    ///
    /// # Arguments
    ///
    /// * `last_poll_timestamp` - UTC timestamp of the start of the most recent **successful** poll cycle.
    ///   `None` indicates the poll cycle failed; the timestamp should not be updated in this case.
    /// * `error_count` - Cumulative count of errors (per-device failures) since gateway startup.
    ///   Never resets; survives restarts via persistent storage.
    /// * `chirpstack_available` - Boolean flag: `true` if most recent poll succeeded,
    ///   `false` if ChirpStack was unreachable during the poll cycle.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all metrics were successfully updated
    /// * `Err(OpcGwError)` - If an error occurs during update
    ///
    /// # Error Cases
    ///
    /// - **Storage error**: Database connectivity issues, write failures
    /// - **Missing table**: gateway_status table doesn't exist or is corrupted
    ///
    /// # Semantics
    ///
    /// - **Timestamp handling**: If `last_poll_timestamp` is `None`, the database timestamp
    ///   is left unchanged (preserving the last successful poll time).
    /// - **Idempotency**: Calling multiple times with the same values produces the same result.
    /// - **Atomicity**: Either all three metrics are updated or none are (single row UPDATE or INSERT).
    fn update_gateway_status(
        &self,
        last_poll_timestamp: Option<DateTime<Utc>>,
        error_count: i32,
        chirpstack_available: bool,
    ) -> Result<(), OpcGwError>;

    /// Retrieves a gateway health metric value by name.
    ///
    /// Reads from the gateway_status table and returns the requested health metric.
    /// Used by OPC UA server to provide real-time health visibility to clients.
    ///
    /// # Arguments
    ///
    /// * `metric_name` - The health metric to retrieve:
    ///   - "last_poll_timestamp" - Last successful poll timestamp (DateTime string or NULL)
    ///   - "error_count" - Cumulative error count (i32)
    ///   - "chirpstack_available" - ChirpStack connection state (bool)
    ///
    /// # Returns
    ///
    /// * `Ok((timestamp_opt, error_count, available))` - Health metrics tuple
    /// * `Err(OpcGwError)` - If database query fails
    ///
    /// # Error Cases
    ///
    /// - **Storage error**: Database connectivity issues, query failures
    /// - **Missing table**: gateway_status table doesn't exist
    /// - **Corrupted data**: Values cannot be parsed into expected types
    ///
    /// # Semantics
    ///
    /// - **First startup**: If gateway_status row doesn't exist, returns sensible defaults:
    ///   - timestamp: None
    ///   - error_count: 0
    ///   - available: false (conservative default)
    /// - **NULL handling**: NULL timestamps are treated as "never polled yet"
    /// - **Non-blocking**: Lock-free reads using SQLite WAL mode
    fn get_gateway_health_metrics(&self) -> Result<(Option<DateTime<Utc>>, i32, bool), OpcGwError>;
}


/// Represents a ChirpStack LoRaWAN device and its associated metrics.
///
/// This structure stores all information related to a single device that is
/// monitored by the gateway. It maintains the device's display name and a
/// collection of its current metric values.
///
/// # Storage Strategy
///
/// Metrics are stored in a HashMap with the ChirpStack metric name as the key
/// and the current value as the payload. This allows for O(1) metric lookups
/// and updates.
///
/// # Note
///
/// Device IDs must be unique across all applications in ChirpStack, while
/// metric names only need to be unique within a single device.
pub struct Device {
    /// Human-readable name of the device as configured in the gateway.
    ///
    /// This name is used for display purposes in the OPC UA address space
    /// and may differ from the device name in ChirpStack.
    device_name: String,

    /// Collection of current metric values for this device.
    ///
    /// The key is the ChirpStack metric name (case-sensitive) and the value
    /// is the current metric reading. Metrics are updated as new data arrives
    /// from ChirpStack polling.
    device_metrics: HashMap<String, MetricValueInternal>,
}




/// Internal metric value representation with full metadata.
/// Harmonized with types.rs MetricValue for easier SQL persistence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MetricValueInternal {
    pub device_id: String,
    pub metric_name: String,
    pub value: String,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub data_type: MetricType,
}

/// Internal device command representation (used by Storage struct's in-memory queue)
/// Harmonized with types.rs DeviceCommand for easier SQL persistence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DeviceCommandInternal {
    pub id: u64,
    pub device_id: String,
    pub payload: Vec<u8>,
    pub f_port: u8,
    pub status: CommandStatus,
    pub created_at: DateTime<Utc>,
    pub error_message: Option<String>,
}

/// Internal ChirpStack status representation (used by Storage struct)
/// Harmonized with types.rs ChirpstackStatus for easier SQL persistence.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChirpstackStatusInternal {
    pub server_available: bool,
    pub last_poll_time: Option<DateTime<Utc>>,
    pub error_count: u32,
}

// SQL Serialization Support
impl ToSql for MetricValueInternal {
    fn to_sql(&self) -> SqliteResult<rusqlite::types::ToSqlOutput<'_>> {
        let json = serde_json::to_string(self).map_err(|_| rusqlite::Error::InvalidQuery)?;
        Ok(rusqlite::types::ToSqlOutput::Owned(rusqlite::types::Value::Text(json)))
    }
}

impl FromSql for MetricValueInternal {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Text(s) => {
                let json_str = std::str::from_utf8(s).map_err(|e| {
                    error!(error = %e, "Failed to decode JSON as UTF-8");
                    rusqlite::types::FromSqlError::InvalidType
                })?;
                serde_json::from_str(json_str).map_err(|e| {
                    error!(error = %e, json = %json_str, "Failed to deserialize MetricValueInternal from JSON");
                    rusqlite::types::FromSqlError::InvalidType
                })
            }
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

impl ToSql for DeviceCommandInternal {
    fn to_sql(&self) -> SqliteResult<rusqlite::types::ToSqlOutput<'_>> {
        let json = serde_json::to_string(self).map_err(|_| rusqlite::Error::InvalidQuery)?;
        Ok(rusqlite::types::ToSqlOutput::Owned(rusqlite::types::Value::Text(json)))
    }
}

impl FromSql for DeviceCommandInternal {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Text(s) => {
                let json_str = std::str::from_utf8(s).map_err(|e| {
                    error!(error = %e, "Failed to decode JSON as UTF-8");
                    rusqlite::types::FromSqlError::InvalidType
                })?;
                serde_json::from_str(json_str).map_err(|e| {
                    error!(error = %e, json = %json_str, "Failed to deserialize DeviceCommandInternal from JSON");
                    rusqlite::types::FromSqlError::InvalidType
                })
            }
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

impl ToSql for ChirpstackStatusInternal {
    fn to_sql(&self) -> SqliteResult<rusqlite::types::ToSqlOutput<'_>> {
        let json = serde_json::to_string(self).map_err(|_| rusqlite::Error::InvalidQuery)?;
        Ok(rusqlite::types::ToSqlOutput::Owned(rusqlite::types::Value::Text(json)))
    }
}

impl FromSql for ChirpstackStatusInternal {
    fn column_result(value: ValueRef<'_>) -> FromSqlResult<Self> {
        match value {
            ValueRef::Text(s) => {
                let json_str = std::str::from_utf8(s).map_err(|e| {
                    error!(error = %e, "Failed to decode JSON as UTF-8");
                    rusqlite::types::FromSqlError::InvalidType
                })?;
                serde_json::from_str(json_str).map_err(|e| {
                    error!(error = %e, json = %json_str, "Failed to deserialize ChirpstackStatusInternal from JSON");
                    rusqlite::types::FromSqlError::InvalidType
                })
            }
            _ => Err(rusqlite::types::FromSqlError::InvalidType),
        }
    }
}

/// Central storage manager for device metrics and system status.
///
/// This structure serves as the main data repository for the gateway,
/// providing a unified interface for storing and retrieving device metrics
/// collected from ChirpStack. It maintains both the current metric values
/// and the operational status of the ChirpStack connection.
///
/// # Design Principles
///
/// - **Fast Lookups**: Uses HashMap-based indexing for O(1) device and metric access
/// - **Type Safety**: Strongly typed metric values with compile-time validation
/// - **Configuration-Driven**: Device structure is initialized from application configuration
/// - **Status Monitoring**: Tracks ChirpStack server health for diagnostics
///
/// # Lifecycle
///
/// 1. **Initialization**: Created with device structure from configuration
/// 2. **Operation**: Continuously updated by ChirpStack poller
/// 3. **Access**: Queried by OPC UA server for client requests
///
/// # Thread Safety
///
/// This structure is not thread-safe by itself. When used in a multi-threaded
/// environment (typical with Tokio), it should be wrapped in appropriate
/// synchronization primitives like `Arc<Mutex<Storage>>`.
pub struct Storage {
    /// Application configuration used to initialize device structure.
    ///
    /// This configuration is cloned during storage initialization and used
    /// for device lookups and validation operations.
    #[allow(dead_code)]
    config: AppConfig,

    /// Current status of the ChirpStack server connection.
    ///
    /// Updated periodically by the ChirpStack poller to reflect the current
    /// health and performance of the ChirpStack API connection.
    chirpstack_status: ChirpstackStatusInternal,

    /// Collection of all monitored devices indexed by their ChirpStack device ID.
    ///
    /// The device ID serves as the primary key for device lookups. Each device
    /// contains its display name and current metric values. The structure is
    /// initialized based on the application configuration.
    devices: HashMap<String, Device>,

    /// Command queue for chirpstack devices
    device_command_queue: Vec<DeviceCommandInternal>,
}

impl Storage {
    /// Creates a new Storage instance from the provided application configuration.
    ///
    /// This constructor initializes the storage system by parsing the application
    /// configuration and creating the internal device and metric structure. Each
    /// configured device and its associated metrics are pre-allocated with default
    /// values to ensure consistent data access patterns.
    ///
    /// # Arguments
    ///
    /// * `app_config` - Reference to the application configuration containing
    ///   device and metric definitions
    ///
    /// # Returns
    ///
    /// A new `Storage` instance with:
    /// - All configured devices pre-allocated
    /// - All metrics initialized with type-appropriate default values
    /// - ChirpStack status set to default (available, 0ms response time)
    ///
    /// # Device Initialization
    ///
    /// For each device in the configuration:
    /// 1. Creates a `Device` struct with the configured display name
    /// 2. Initializes all configured metrics with default values based on type
    /// 3. Stores the device in the internal HashMap using ChirpStack device ID as key
    ///
    /// # Metric Default Values
    ///
    /// - `Bool` metrics: initialized to `false`
    /// - `Int` metrics: initialized to `0`
    /// - `Float` metrics: initialized to `0.0`
    /// - `String` metrics: initialized to empty string
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use crate::config::AppConfig;
    /// use crate::storage::Storage;
    ///
    /// let config = AppConfig::new()?;
    /// let storage = Storage::new(&config);
    /// println!("Storage initialized with {} devices", storage.devices.len());
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(n) complexity where n is the total number of
    /// configured devices and metrics. It should typically be called once
    /// during application startup.
    pub fn new(app_config: &AppConfig) -> Storage {
        debug!("Creating new Storage instance");
        let mut devices: HashMap<String, Device> = HashMap::new();

        // Create an empty command queue
        debug!("Creating an empty command queue");
        let device_command_queue = Vec::new();

        // Process each application in the configuration
        for application in app_config.application_list.iter() {
            debug!(application_name = %application.application_name, "Processing application");

            // Process each device within the application
            for device in application.device_list.iter() {
                debug!(
                    device_name = %device.device_name,
                    device_id = %device.device_id,
                    "Initializing device"
                );

                // Initialize metrics HashMap for this device
                let mut device_metrics = HashMap::new();
                for metric in device.read_metric_list.iter() {
                    // Initialize metric with type-appropriate default value
                    let metric_type = match metric.metric_type {
                        OpcMetricTypeConfig::Bool => MetricType::Bool,
                        OpcMetricTypeConfig::Int => MetricType::Int,
                        OpcMetricTypeConfig::Float => MetricType::Float,
                        OpcMetricTypeConfig::String => MetricType::String,
                    };
                    let default_value = MetricValueInternal {
                        device_id: device.device_id.clone(),
                        metric_name: metric.chirpstack_metric_name.clone(),
                        value: match metric_type {
                            MetricType::Bool => "false".to_string(),
                            MetricType::Int => "0".to_string(),
                            MetricType::Float => "0.0".to_string(),
                            MetricType::String => String::new(),
                        },
                        timestamp: chrono::Utc::now(),
                        data_type: metric_type,
                    };
                    device_metrics.insert(metric.chirpstack_metric_name.clone(), default_value);
                    trace!(
                        metric_name = %metric.chirpstack_metric_name,
                        device_id = %device.device_id,
                        "Initialized metric"
                    );
                }

                // Create device instance
                let new_device = Device {
                    device_name: device.device_name.clone(),
                    device_metrics,
                };

                // Store device in the main collection
                devices.insert(device.device_id.clone(), new_device);
            }
        }

        debug!(
            device_count = devices.len(),
            "Storage initialization complete"
        );

        Storage {
            config: app_config.clone(),
            chirpstack_status: ChirpstackStatusInternal {
                server_available: true,
                last_poll_time: None,
                error_count: 0,
            },
            devices,
            device_command_queue,
        }
    }

    /// Retrieves a mutable reference to a device by its ChirpStack device ID.
    ///
    /// This method provides direct access to a device's internal structure,
    /// allowing for modification of device properties and metrics. It is
    /// primarily used internally by other storage methods.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique ChirpStack identifier for the device
    ///
    /// # Returns
    ///
    /// * `Some(&mut Device)` - Mutable reference to the device if found
    /// * `None` - If no device with the specified ID exists
    ///
    /// # Usage
    ///
    /// This method is typically used by higher-level storage operations
    /// rather than being called directly by external code.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let mut storage = Storage::new(&config);
    /// if let Some(device) = storage.get_device(&"device_123".to_string()) {
    ///     println!("Found device: {}", device.device_name);
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(1) average time complexity due to HashMap indexing.
    pub fn get_device(&mut self, device_id: &str) -> Option<&mut Device> {
        debug!(device_id = %device_id, "Retrieving device");
        self.devices.get_mut(device_id)
    }

    /// Retrieves the display name of a device by its ChirpStack device ID.
    ///
    /// This method looks up a device in the storage and returns its configured
    /// display name. The display name is typically used in the OPC UA address
    /// space and user interfaces.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique ChirpStack identifier for the device
    ///
    /// # Returns
    ///
    /// * `Some(String)` - The device's display name if the device exists
    /// * `None` - If no device with the specified ID is found
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let storage = Storage::new(&config);
    /// match storage.get_device_name(&"device_123".to_string()) {
    ///     Some(name) => println!("Device name: {}", name),
    ///     None => println!("Device not found"),
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(1) average time complexity for the device lookup.
    /// The string clone operation adds minimal overhead.
    pub fn get_device_name(&self, device_id: &str) -> Option<String> {
        debug!(device_id = %device_id, "Looking up device name");
        match self.devices.get(device_id) {
            Some(device) => Some(device.device_name.clone()),
            None => {
                debug!(device_id = %device_id, "Device not found");
                None
            }
        }
    }

    /// Retrieves the current value of a specific metric for a device.
    ///
    /// This method performs a two-level lookup: first finding the device by ID,
    /// then locating the specific metric by its ChirpStack name. It returns a
    /// clone of the metric value to avoid borrowing issues.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique ChirpStack identifier for the device
    /// * `chirpstack_metric_name` - The exact metric name as used in ChirpStack
    ///
    /// # Returns
    ///
    /// * `Some(MetricType)` - A clone of the metric value if found
    /// * `None` - If the device or metric is not found
    ///
    /// # Error Conditions
    ///
    /// This method returns `None` in the following cases:
    /// - Device with the specified ID does not exist
    /// - Device exists but the metric name is not found
    /// - Metric name case mismatch (metric names are case-sensitive)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let mut storage = Storage::new(&config);
    /// match storage.get_metric_value("device_123", "temperature") {
    ///     Some(MetricType::Float(temp)) => println!("Temperature: {}°C", temp),
    ///     Some(other) => println!("Unexpected metric type: {:?}", other),
    ///     None => println!("Metric not found"),
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(1) average time complexity for both the device
    /// and metric lookups due to HashMap indexing.
    pub fn get_metric_value(
        &self,
        device_id: &str,
        chirpstack_metric_name: &str,
    ) -> Option<MetricValueInternal> {
        debug!(
            metric_name = %chirpstack_metric_name,
            device_id = %device_id,
            "Retrieving metric"
        );

        // First, find the device
        match self.devices.get(device_id) {
            None => {
                debug!(device_id = %device_id, "Device not found");
                None
            }
            Some(device) => {
                // Then, find the metric within the device
                match device.device_metrics.get(chirpstack_metric_name) {
                    None => {
                        debug!(
                            metric_name = %chirpstack_metric_name,
                            device_id = %device_id,
                            "Metric not found for device"
                        );
                        None
                    }
                    Some(metric_value) => {
                        trace!(
                            metric_name = %chirpstack_metric_name,
                            metric_value = ?metric_value,
                            "Found metric"
                        );
                        Some(metric_value.clone())
                    }
                }
            }
        }
    }

    /// Updates the value of a specific metric for a device.
    ///
    /// This method locates the specified device and updates the value of the
    /// named metric. If the metric doesn't exist, it will be created. This is
    /// the primary method used by the ChirpStack poller to update metric values.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique ChirpStack identifier for the device
    /// * `chirpstack_metric_name` - The exact metric name as used in ChirpStack
    /// * `value` - The new metric value to store
    ///
    /// # Error Handling
    ///
    /// If the specified device ID is not found in storage, the method logs a
    /// warning and silently ignores the operation. No error is returned. This
    /// allows the system to continue operating even if a metric update fails.
    ///
    /// # Error Handling
    ///
    /// Rather than panicking, consider checking device existence first:
    /// ```rust,no_run
    /// if storage.get_device(&device_id).is_some() {
    ///     storage.set_metric_value(&device_id, "temperature", MetricType::Float(23.5));
    /// } else {
    ///     eprintln!("Device {} not found", device_id);
    /// }
    /// ```
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let mut storage = Storage::new(&config);
    ///
    /// // Update temperature reading
    /// storage.set_metric_value(
    ///     &"device_123".to_string(),
    ///     "temperature",
    ///     MetricType::Float(23.5)
    /// );
    ///
    /// // Update alarm status
    /// storage.set_metric_value(
    ///     &"device_123".to_string(),
    ///     "alarm_active",
    ///     MetricType::Bool(true)
    /// );
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(1) average time complexity for device lookup
    /// and metric insertion/update due to HashMap indexing.
    ///
    /// # Thread Safety
    ///
    /// This method requires mutable access to the storage and is not thread-safe.
    /// Use appropriate synchronization when calling from multiple threads.
    pub fn set_metric_value(
        &mut self,
        device_id: &String,
        chirpstack_metric_name: &str,
        value: MetricValueInternal,
    ) -> Result<(), OpcGwError> {
        debug!(
            metric_name = %chirpstack_metric_name,
            metric_value = ?value,
            device_id = %device_id,
            "Setting metric"
        );

        match self.get_device(device_id) {
            Some(device) => {
                device
                    .device_metrics
                    .insert(chirpstack_metric_name.to_string(), value);
                trace!(
                    metric_name = %chirpstack_metric_name,
                    device_id = %device_id,
                    "Successfully updated metric"
                );
                Ok(())
            }
            None => {
                Err(OpcGwError::Storage(format!(
                    "Cannot restore metric {}.{}: device not found in configuration (orphan metric)",
                    device_id, chirpstack_metric_name
                )))
            }
        }
    }

    /// Updates the ChirpStack server status information.
    ///
    /// This method updates the stored status information about the ChirpStack
    /// server connection, including availability, last poll time, and error count.
    /// This information is typically updated by the ChirpStack poller and
    /// exposed to OPC UA clients for monitoring purposes.
    ///
    /// # Arguments
    ///
    /// * `status` - New status information containing server availability, last poll time, and error count
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use crate::storage::{Storage, ChirpstackStatusInternal};
    /// use chrono::Utc;
    ///
    /// let mut storage = Storage::new(&config);
    /// let status = ChirpstackStatusInternal {
    ///     server_available: true,
    ///     last_poll_time: Some(Utc::now()),
    ///     error_count: 0,
    /// };
    /// storage.update_chirpstack_status(status);
    /// ```
    ///
    /// # Usage in Monitoring
    ///
    /// The updated status information can be exposed via OPC UA diagnostic nodes
    /// to allow clients to monitor the health of the ChirpStack connection.
    pub fn update_chirpstack_status(&mut self, status: ChirpstackStatusInternal) {
        debug!(
            server_available = %status.server_available,
            last_poll_time = ?status.last_poll_time,
            error_count = %status.error_count,
            "Updating ChirpStack status"
        );
        self.chirpstack_status.server_available = status.server_available;
        self.chirpstack_status.last_poll_time = status.last_poll_time;
        self.chirpstack_status.error_count = status.error_count;
    }

    /// Retrieves the current ChirpStack server status.
    ///
    /// Returns a clone of the current status information, including server
    /// availability, last poll time, and error count. This method is typically used
    /// by the OPC UA server to expose diagnostic information to clients.
    ///
    /// # Returns
    ///
    /// A clone of the current `ChirpstackStatus` containing:
    /// - `server_available`: Whether the ChirpStack server is reachable
    /// - `last_poll_time`: Timestamp of the last successful poll
    /// - `error_count`: Number of errors since last successful connection
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let storage = Storage::new(&config);
    /// let status = storage.get_chirpstack_status();
    /// println!("ChirpStack available: {}", status.server_available);
    /// println!("Last poll: {:?}", status.last_poll_time);
    /// println!("Error count: {}", status.error_count);
    /// ```
    pub fn get_chirpstack_status(&self) -> ChirpstackStatusInternal {
        self.chirpstack_status.clone()
    }

    /// Checks if the ChirpStack server is currently available.
    ///
    /// This is a convenience method that returns only the availability flag
    /// from the ChirpStack status. It's useful for quick availability checks
    /// without needing the full status structure.
    ///
    /// # Returns
    ///
    /// * `true` - ChirpStack server is available and responding
    /// * `false` - ChirpStack server is unreachable or not responding
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let storage = Storage::new(&config);
    /// if storage.get_chirpstack_available() {
    ///     println!("ChirpStack server is online");
    /// } else {
    ///     println!("ChirpStack server is offline");
    /// }
    /// ```
    ///
    /// # Usage
    ///
    /// This method is particularly useful for:
    /// - Conditional logic based on server availability
    /// - Simple status display in user interfaces
    /// - Health check endpoints
    pub fn get_chirpstack_available(&self) -> bool {
        self.chirpstack_status.server_available
    }


    /// Logs all stored metrics to the debug output.
    ///
    /// This diagnostic method iterates through all devices and their metrics,
    /// logging detailed information about the current state of the storage.
    /// It's primarily used for debugging and troubleshooting purposes.
    ///
    /// # Output Format
    ///
    /// The method logs information at different levels:
    /// - **Debug**: "Dumping metrics from storage" (method start)
    /// - **Trace**: Device information and metric values
    ///
    /// Currently, only `Float` type metrics are logged in detail. Other metric
    /// types are processed but not logged (empty match arms).
    ///
    /// # Log Output Example
    ///
    /// ```text
    /// DEBUG: Dumping metrics from storage
    /// TRACE: Device name 'Temperature Sensor 01', id: 'device_123'
    /// TRACE:     Metric "temperature": 23.5
    /// TRACE:     Metric "humidity": 65.2
    /// ```
    ///
    /// # Usage
    ///
    /// This method is typically called:
    /// - During debugging sessions
    /// - After metric updates to verify storage state
    /// - In diagnostic routines
    /// - When troubleshooting metric collection issues
    ///
    /// # Performance Considerations
    ///
    /// This method iterates through all devices and metrics, so it has O(n*m)
    /// complexity where n is the number of devices and m is the average number
    /// of metrics per device. Use sparingly in production due to logging overhead.
    ///
    /// # Future Enhancement
    ///
    /// The empty match arms for `Bool`, `Int`, and `String` types suggest that
    /// logging for these types may be added in future versions.
    pub fn dump_storage(&mut self) {
        debug!("Dumping metrics from storage");
        for (device_id, device) in &self.devices {
            trace!(device_name = %device.device_name, device_id = %device_id, "Device entry");
            for (metric_name, metric) in device.device_metrics.iter() {
                trace!(
                    metric_name = %metric_name,
                    metric_value = %metric.value,
                    data_type = %metric.data_type,
                    timestamp = %metric.timestamp,
                    "Metric"
                );
            }
        }
    }

    /// Adds a new command to the end of the device command queue.
    ///
    /// This function appends the given command to the queue, which will be processed
    /// in LIFO (Last In, First Out) order when dequeued.
    ///
    /// # Parameters
    /// * `command` - The `DeviceCommand` to add to the queue
    ///
    /// # Examples
    /// ```
    /// let mut storage = Storage::new(&config);
    /// let command = DeviceCommand {
    ///     device_id: "device_123".to_string(),
    ///     confirmed: true,
    ///     f_port: 1,
    ///     data: vec![0x01, 0x02, 0x03],
    /// };
    /// storage.push_command(command);
    /// ```
    pub fn push_command(&mut self, command: DeviceCommandInternal) {
        self.device_command_queue.push(command);
    }

    /// Removes and returns the last command from the device command queue.
    ///
    /// This function operates in LIFO (Last In, First Out) order, removing the most
    /// recently added command from the queue.
    ///
    /// # Returns
    /// * `Some(DeviceCommandInternal)` - The last command in the queue if one exists
    /// * `None` - If the command queue is empty
    ///
    /// # Examples
    /// ```
    /// let mut storage = Storage::new(&config);
    /// match storage.dequeue_command() {
    ///     Some(command) => println!("Dequeued command for device: {}", command.device_id),
    ///     None => println!("No commands to dequeue"),
    /// }
    /// ```
    pub fn pop_command(&mut self) -> Option<DeviceCommandInternal> {
        self.device_command_queue.pop()
    }

    /// Returns a copy of the device command queue if it contains commands, or None if empty.
    ///
    /// # Returns
    /// * `Some(Vec<DeviceCommandInternal>)` - A clone of the command queue if it has at least one command
    /// * `None` - If the command queue is empty
    ///
    /// # Examples
    /// ```
    /// let storage = Storage::new(&config);
    /// match storage.get_device_command_queue() {
    ///     Some(commands) => println!("Found {} commands", commands.len()),
    ///     None => println!("No commands in queue"),
    /// }
    /// ```
    pub fn get_device_command_queue(&self) -> Vec<DeviceCommandInternal> {
        self.device_command_queue.clone()
    }
}

/// Storage module test suite.
///
/// This module contains comprehensive tests for the storage functionality,
/// including device management, metric operations, and status tracking.
/// Tests use a dedicated test configuration to ensure isolation from
/// production settings.
#[cfg(test)]
mod tests {
    use super::*;
    
    use std::sync::Arc;
    use figment::{
        providers::{Format, Toml},
        Figment,
    };

    /// Loads test configuration from a TOML file.
    ///
    /// This helper function provides a consistent way to load test configuration
    /// across all test cases. It uses a test-specific configuration file to
    /// avoid dependencies on production configuration.
    ///
    /// # Configuration Path
    ///
    /// The configuration file path is determined by:
    /// - `CONFIG_PATH` environment variable if set
    /// - Default: `"tests/config/config.toml"`
    ///
    /// # Returns
    ///
    /// * `AppConfig` - The loaded test configuration
    ///
    /// # Panics
    ///
    /// Panics if the test configuration file cannot be loaded or parsed.
    /// This is appropriate for test scenarios where configuration errors
    /// should cause immediate test failure.
    fn get_config() -> AppConfig {
        let config_path =
            std::env::var("CONFIG_PATH").unwrap_or_else(|_| "tests/config/config.toml".to_string());
        debug!(config_path = %config_path, "Loading test configuration");
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load test configuration");
        config
    }

    /// Tests ChirpStack status management functionality.
    ///
    /// This test verifies the complete lifecycle of ChirpStack status handling:
    /// 1. **Initial State**: Verifies default status after storage creation
    /// 2. **Status Update**: Tests updating status with new values
    /// 3. **Status Retrieval**: Verifies all status accessor methods
    ///
    /// # Test Scenarios
    ///
    /// - Initial status: server available = true, response time = 0.0
    /// - Updated status: server available = false, response time = 1.0ms
    /// - Accessor method consistency across different retrieval methods
    ///
    /// # Assertions
    ///
    /// - Initial state matches expected defaults
    /// - Status update correctly modifies stored values
    /// - All accessor methods return consistent values
    /// - Status structure equality works correctly
    #[test]
    fn test_chirpstack_status() {
        let app_config = get_config();
        let mut storage = Storage::new(&app_config);

        // Test initial status
        assert!(storage.chirpstack_status.server_available);
        assert!(storage.chirpstack_status.last_poll_time.is_none());
        assert_eq!(storage.chirpstack_status.error_count, 0);

        // Test status update
        let now = Utc::now();
        let chirpstack_status = ChirpstackStatusInternal {
            server_available: false,
            last_poll_time: Some(now),
            error_count: 1,
        };
        storage.update_chirpstack_status(chirpstack_status.clone());

        // Test status retrieval methods
        assert_eq!(storage.get_chirpstack_status(), chirpstack_status);
        assert!(!storage.get_chirpstack_available());
        assert_eq!(storage.get_chirpstack_status().error_count, 1);
    }

    /// Tests configuration loading and storage initialization.
    ///
    /// This test verifies that:
    /// 1. Test configuration loads successfully
    /// 2. Storage initializes with the loaded configuration
    /// 3. At least one application is present in the configuration
    ///
    /// This is a basic smoke test to ensure the test infrastructure
    /// is working correctly.
    #[test]
    fn test_load_metrics() {
        let app_config = get_config();
        let storage = Storage::new(&app_config);

        // Verify that we loaded a meaningful configuration
        assert!(!storage.config.application_list.is_empty());
    }

    /// Tests device retrieval functionality.
    ///
    /// Verifies that the `get_device` method correctly retrieves devices
    /// that were initialized from the configuration. Uses a known device ID
    /// from the test configuration.
    ///
    /// # Test Data
    ///
    /// Assumes the test configuration contains a device with ID "device_1".
    #[test]
    fn test_get_device() {
        let mut storage = Storage::new(&get_config());
        let device = storage.get_device(&String::from("device_1"));

        // Verify device exists
        assert!(device.is_some());
    }

    /// Tests device name retrieval functionality.
    ///
    /// Verifies that the `get_device_name` method correctly:
    /// 1. Returns the expected name for existing devices
    /// 2. Returns `None` for non-existent devices
    ///
    /// # Test Data
    ///
    /// - Existing device: "device_1" should map to "Device01"
    /// - Non-existent device: "no_device" should return `None`
    #[test]
    fn test_get_device_name() {
        let storage = Storage::new(&get_config());
        let device_id = String::from("device_1");
        let no_device_id = String::from("no_device");

        // Test existing device
        assert_eq!(
            storage.get_device_name(&device_id),
            Some("Device01".to_string())
        );

        // Test non-existent device
        assert_eq!(storage.get_device_name(&no_device_id), None);
    }

    /// Tests metric value setting for non-existent devices.
    ///
    /// This test verifies that attempting to set a metric value for a device
    /// that doesn't exist in storage results in a panic. This behavior is
    /// intentional as it indicates a programming error that should be caught
    /// during development.
    ///
    /// # Expected Behavior
    ///
    /// The test should panic when trying to set a metric for "no_device"
    /// which is not present in the test configuration.
    ///
    /// # Safety Note
    ///
    /// Setting a metric for a non-existent device should not panic.
    /// The function logs a warning and silently ignores the operation.
    #[test]
    fn test_set_metric_value_missing_device() {
        let mut storage = Storage::new(&get_config());
        let no_device_id = String::from("no_device");
        let no_metric = String::from("no_metric");
        let value = MetricValueInternal {
            device_id: no_device_id.clone(),
            metric_name: no_metric.clone(),
            value: "10.0".to_string(),
            timestamp: Utc::now(),
            data_type: MetricType::Float,
        };

        // This should NOT panic — graceful handling of missing device
        let _ = storage.set_metric_value(&no_device_id, &no_metric, value);
        // Verify the device was not implicitly created
        assert!(storage.get_device(&no_device_id).is_none());
    }

    /// Tests metric value setting and retrieval functionality.
    ///
    /// This comprehensive test verifies the complete metric lifecycle:
    /// 1. **Setting Values**: Updates a metric for an existing device
    /// 2. **Retrieving Values**: Confirms the stored value matches the set value
    /// 3. **Error Cases**: Tests retrieval for non-existent devices and metrics
    ///
    /// # Test Scenarios
    ///
    /// - Valid device + valid metric: should succeed
    /// - Invalid device + valid metric: should return `None`
    /// - Valid device + invalid metric: should return `None`
    ///
    /// # Test Data
    ///
    /// - Device: "device_1" (existing)
    /// - Metric: "metric_1" (should exist in test config)
    /// - Value: 10.0 (Float type)
    #[test]
    fn test_metric_operations() {
        let app_config = get_config();
        let mut storage = Storage::new(&app_config);
        let device_id = String::from("device_1");
        let no_device_id = String::from("no_device");
        let metric = String::from("metric_1");
        let no_metric = String::from("no_metric");
        let value = MetricValueInternal {
            device_id: device_id.clone(),
            metric_name: metric.clone(),
            value: "10.0".to_string(),
            timestamp: Utc::now(),
            data_type: MetricType::Float,
        };

        // Test setting and getting metric value
        let _ = storage.set_metric_value(&device_id, &metric, value.clone());
        let retrieved = storage.get_metric_value(&device_id, &metric);
        assert!(retrieved.is_some());
        let retrieved_val = retrieved.unwrap();
        assert_eq!(retrieved_val.device_id, device_id);
        assert_eq!(retrieved_val.metric_name, metric);
        assert_eq!(retrieved_val.value, "10.0");
        assert_eq!(retrieved_val.data_type, MetricType::Float);

        // Test error cases
        assert_eq!(storage.get_metric_value(&no_device_id, &metric), None);
        assert_eq!(storage.get_metric_value(&device_id, &no_metric), None);
    }

    #[test]
    fn test_command_queue() {
        let mut storage = Storage::new(&get_config());
        let command = DeviceCommandInternal {
            id: 0,
            device_id: "device01".to_string(),
            payload: vec![10, 20],
            f_port: 100,
            status: CommandStatus::Pending,
            created_at: Utc::now(),
            error_message: None,
        };
        storage.push_command(command);
        let result = storage.pop_command();

        let cmd = result.unwrap();
        assert_eq!(cmd.device_id, "device01");
        assert_eq!(cmd.status, CommandStatus::Pending);
        assert_eq!(cmd.f_port, 100);
        assert_eq!(cmd.payload, vec![10, 20]);
    }

    #[test]
    fn test_trait_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Arc<dyn StorageBackend>>();
    }

    #[test]
    fn test_trait_method_signatures_exist() {
        use std::any::type_name;
        let trait_name = type_name::<dyn StorageBackend>();
        assert!(trait_name.contains("StorageBackend"));
    }
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! SQLite-based Storage Backend Implementation
//!
//! Provides a production-grade persistent storage implementation using SQLite.
//! Features:
//! - WAL (Write-Ahead Logging) mode for concurrent readers + single writer
//! - Per-task connection pooling (Story 2-2x) for true concurrent access without Rust Mutex bottleneck
//! - Full StorageBackend trait implementation with backward-compatible API

// Constructor variants `new`, `with_pool_and_validator`, and
// `new_with_initialization` plus the `validator` field and
// `prune_old_metrics` are part of the public scaffold for command
// validation (Epic 7) and operational tooling. They have no live call site
// today; allow `dead_code` at module scope rather than per-item so the
// scaffold stays legible.
#![allow(dead_code)]
//! - No panics in production paths — all errors wrapped in OpcGwError
//!
//! # Architecture: Per-Task Connections (Story 2-2x)
//! This implementation uses Arc<ConnectionPool> shared across tasks.
//! Each task acquires its own Connection from the pool when needed (via ConnectionGuard).
//! - No Rust-level Mutex serialization: each task has independent database access
//! - SQLite WAL provides true concurrent readers + single writer at database level
//! - ConnectionGuard RAII pattern ensures automatic connection return to pool on drop
//! - Pool timeout (5s) prevents indefinite waiting; graceful degradation under exhaustion
//!
//! # AC 10 Compliance (Story 2-2x)
//! - AC 1: Each async task (poller, OPC UA) opens own Connection from pool ✓
//! - AC 2: SQLite WAL provides concurrent readers + single writer (no Rust Mutex) ✓
//! - AC 7: ConnectionGuard drops return connection to pool (RAII cleanup) ✓
//! - AC 8: Pool created once in main(), shared via Arc<ConnectionPool> ✓
//! - AC 9: Transaction safety under concurrency verified via tests ✓
//! - AC 10: StorageBackend trait signatures unchanged (backward compatible) ✓

use rusqlite::{Connection, params, OptionalExtension};
use crate::utils::OpcGwError;
use crate::storage::{ChirpstackStatus, Command, CommandFilter, CommandStatus, DeviceCommand, ErrorEvent, MetricType, ConnectionPool, MetricValue};
use crate::command_validation::CommandValidator;
use crate::config::{ChirpStackApplications, ChirpstackDevice, ReadMetric, DeviceCommandCfg, OpcMetricTypeConfig};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, error, info, trace, warn};
use super::schema;

/// Format a DateTime as RFC3339 with microsecond precision
fn format_rfc3339(dt: &DateTime<Utc>) -> String {
    format!("{}Z", dt.format("%Y-%m-%dT%H:%M:%S%.6f"))
}

/// Story 6-1, AC#6 (review patch D4): RAII guard that emits the canonical
/// `storage_query` debug log when dropped. Methods construct one at entry,
/// call `ok()` on the success path, and let `Drop` emit the structured log
/// with timing + success flag. On `?`-shortcircuit the guard drops with
/// `success=false`, giving correct visibility into failed queries without
/// hand-instrumenting every error branch.
struct StorageOpLog {
    query_type: &'static str,
    start: Instant,
    success: bool,
}

impl StorageOpLog {
    fn start(query_type: &'static str) -> Self {
        Self {
            query_type,
            start: Instant::now(),
            success: false,
        }
    }

    fn ok(&mut self) {
        self.success = true;
    }
}

/// Story 6-3, AC#7: emit a structured `storage_query` warn when a rusqlite
/// error is `SQLITE_BUSY` (database is busy / locked). Purely diagnostic —
/// no retry happens here; the caller decides whether to bubble or retry.
/// `query_type` and `retry_attempt` come from the caller; `latency_ms` is
/// the wall-clock time of the attempt that failed.
fn log_sqlite_busy_if_applicable(
    e: &rusqlite::Error,
    query_type: &'static str,
    retry_attempt: u32,
    latency_ms: u64,
) {
    if let rusqlite::Error::SqliteFailure(err, _) = e {
        // Iter-3 D-AC7 resolution: AC#7 mandates the canonical
        // `error="SQLITE_BUSY"` label. Both `DatabaseBusy` (rusqlite code
        // 5) and `DatabaseLocked` (code 6) are surfaced under that label
        // so the AC contract holds for log analysis. The companion
        // `sqlite_error_code` field preserves the distinction for
        // operators that want to differentiate (LOCKED is rarer and
        // indicates cross-connection write contention; BUSY is the more
        // common WAL-mode lock-wait case).
        if matches!(
            err.code,
            rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
        ) {
            warn!(
                operation = "storage_query",
                query_type = query_type,
                error = "SQLITE_BUSY",
                sqlite_error_code = ?err.code,
                retry_attempt = retry_attempt,
                latency_ms = latency_ms,
                "SQLite busy — query was waiting on a lock"
            );
        }
    }
}

impl Drop for StorageOpLog {
    /// Story 6-3, AC#3: when a storage query crosses the configurable
    /// storage-query budget (`crate::utils::storage_query_budget_ms()`,
    /// default 250 ms, override via `OPCGW_STORAGE_QUERY_BUDGET_MS` — GH-144),
    /// upgrade the routine `debug!` to a `warn!` carrying `exceeded_budget=true`.
    /// Tells the operator "this query was unusually slow" without spamming on
    /// every cycle.
    fn drop(&mut self) {
        // Review patch P18: skip emitting during panic unwind. A `Drop`
        // on a panicking thread would emit a misleading `success=false`
        // log without any signal that the underlying cause was a panic
        // rather than a soft failure; the secondary concern is the
        // double-panic risk from re-entering tracing during unwind.
        if std::thread::panicking() {
            return;
        }
        let latency_ms = self.start.elapsed().as_millis() as u64;
        let budget_ms = crate::utils::storage_query_budget_ms();
        if latency_ms > budget_ms {
            warn!(
                operation = "storage_query",
                query_type = self.query_type,
                latency_ms = latency_ms,
                budget_ms = budget_ms,
                exceeded_budget = true,
                success = self.success,
            );
        } else {
            debug!(
                operation = "storage_query",
                query_type = self.query_type,
                latency_ms = latency_ms,
                success = self.success,
            );
        }
    }
}

/// SQLite-backed storage implementation for opcgw.
///
/// Uses per-task connections from a connection pool to enable true concurrent access.
/// Each async task gets its own connection from the pool, allowing SQLite WAL mode
/// to provide concurrent readers + single writer at the database level (no Rust Mutex bottleneck).
///
/// # Architecture
/// - Shares Arc<ConnectionPool> across all tasks
/// - Each task checkouts connection from pool when needed
/// - Connections automatically return to pool on drop (RAII pattern)
/// - No global Mutex serialization - SQLite WAL handles concurrency
///
/// # Thread Safety
/// SqliteBackend implements Send + Sync and can be safely shared across task boundaries
/// via Arc. Each task holds Arc<ConnectionPool>, enabling concurrent independent connections.
///
/// # Example
/// ```rust,ignore
/// use opcgw::storage::SqliteBackend;
/// use std::sync::Arc;
///
/// let pool = Arc::new(opcgw::storage::ConnectionPool::new("data/opcgw.db", 3)?);
/// let backend = SqliteBackend::with_pool(Arc::clone(&pool))?;
/// // Use backend for reads/writes
/// ```
#[derive(Clone)]
pub struct SqliteBackend {
    pool: Arc<ConnectionPool>,
    validator: Option<Arc<CommandValidator>>,
}

/// A-3 iter-1 IR6: typed-column payload extracted from a `MetricType` for
/// the four SqliteBackend writers (set_metric / upsert_metric_value /
/// append_metric_history / batch_write_metrics). All four writers populate
/// these five columns + a 6-th `value_type` discriminant from the same
/// `MetricType` payload — factored to a single helper to eliminate the
/// 4-site copy-paste the iter-1 review (Blind F5/F6/F7/F29/F30/F33) flagged.
///
/// Spec AC#4 prohibits adding a helper method on `MetricType` (because
/// `src/storage/types.rs` is strict-zero in A-3); a private struct + free
/// function inside `sqlite.rs` (which is MUTABLE in A-3) is fine.
struct TypedValueColumns {
    value_real: Option<f64>,
    value_int: Option<i64>,
    value_bool: Option<i64>,
    value_text: Option<String>,
    value_type: &'static str,
}

fn typed_value_columns(value: &MetricType) -> TypedValueColumns {
    match value {
        MetricType::Float(f) => TypedValueColumns {
            value_real: Some(*f),
            value_int: None,
            value_bool: None,
            value_text: None,
            value_type: "Float",
        },
        MetricType::Int(i) => TypedValueColumns {
            value_real: None,
            value_int: Some(*i),
            value_bool: None,
            value_text: None,
            value_type: "Int",
        },
        MetricType::Bool(b) => TypedValueColumns {
            value_real: None,
            value_int: None,
            value_bool: Some(if *b { 1 } else { 0 }),
            value_text: None,
            value_type: "Bool",
        },
        MetricType::String(s) => TypedValueColumns {
            value_real: None,
            value_int: None,
            value_bool: None,
            value_text: Some(s.clone()),
            value_type: "String",
        },
    }
}

/// A-4: read-side helper that projects the v007 typed columns + the
/// `value_type` discriminant back into a payload-bearing `MetricType`. The
/// reverse direction of [`typed_value_columns`].
///
/// Returns:
/// - `Ok(Some(MetricType))` — non-legacy row with one of `value_type` ∈
///   `{Float, Int, Bool, String}`. The corresponding typed column is
///   non-NULL (guaranteed by the v008 cross-column CHECK constraint).
/// - `Ok(None)` — legacy row (`value_type = 'legacy'`). The OPC UA reader
///   maps `Ok(None)` to `BadDataUnavailable` per architecture.md:182.
/// - `Err(OpcGwError::Database)` — schema drift (`value_type` outside the
///   closed enum, or a non-legacy row with the discriminated column NULL).
///   The Option-unwrap on the discriminated column is defensive — the v008
///   CHECK constraint forbids the drift; the explicit error makes the
///   diagnosis clear if a future maintainer breaks the invariant.
fn metric_type_from_typed_columns(
    value_type: &str,
    value_real: Option<f64>,
    value_int: Option<i64>,
    value_bool: Option<i64>,
    value_text: Option<String>,
    device_id: &str,
    metric_name: &str,
) -> Result<Option<MetricType>, OpcGwError> {
    // A-4 iter-1 IR2: exactly-one-non-NULL defensive guard. v008 CHECK
    // forbids drift, but the helper validates independently so a future
    // CHECK loosening or restored backup can't silently corrupt reads.
    //
    // A-4 iter-2 JR3: extend the symmetry to the 'legacy' arm. v008 CHECK
    // also requires that value_type='legacy' rows have ALL typed columns
    // NULL — a drifted legacy row with orphaned `value_real=Some(...)`
    // would silently return Ok(None) and the real value would be lost.
    // Reject it explicitly.
    match value_type {
        "legacy" => {
            if value_real.is_some() || value_int.is_some() || value_bool.is_some() || value_text.is_some() {
                return Err(typed_column_multi_set_err(
                    "legacy", device_id, metric_name,
                    value_real.is_some(), value_int.is_some(), value_bool.is_some(), value_text.is_some(),
                ));
            }
            Ok(None)
        }
        "Float" => {
            let f = value_real.ok_or_else(|| {
                typed_column_drift_err("Float", "value_real", device_id, metric_name)
            })?;
            if value_int.is_some() || value_bool.is_some() || value_text.is_some() {
                return Err(typed_column_multi_set_err(
                    "Float", device_id, metric_name,
                    false, value_int.is_some(), value_bool.is_some(), value_text.is_some(),
                ));
            }
            // A-4 iter-1 IR9 + iter-2 JR8: defensive NaN/Inf guard with
            // harmonized "Schema drift:" prefix matching the other guards.
            // v007 schema has no finiteness CHECK on value_real (writer-side
            // option-a filter at poller catches NaN/Inf, but a row could land
            // via raw SQL / restored backup).
            if !f.is_finite() {
                return Err(OpcGwError::Database(format!(
                    "Schema drift: value_type='Float' but value_real={} is non-finite \
                     for device {}, metric {} (writer-side NaN/Inf filter was bypassed)",
                    f, device_id, metric_name
                )));
            }
            Ok(Some(MetricType::Float(f)))
        }
        "Int" => {
            let i = value_int.ok_or_else(|| {
                typed_column_drift_err("Int", "value_int", device_id, metric_name)
            })?;
            if value_real.is_some() || value_bool.is_some() || value_text.is_some() {
                return Err(typed_column_multi_set_err(
                    "Int", device_id, metric_name,
                    value_real.is_some(), false, value_bool.is_some(), value_text.is_some(),
                ));
            }
            Ok(Some(MetricType::Int(i)))
        }
        "Bool" => {
            let b = value_bool.ok_or_else(|| {
                typed_column_drift_err("Bool", "value_bool", device_id, metric_name)
            })?;
            if value_real.is_some() || value_int.is_some() || value_text.is_some() {
                return Err(typed_column_multi_set_err(
                    "Bool", device_id, metric_name,
                    value_real.is_some(), value_int.is_some(), false, value_text.is_some(),
                ));
            }
            // A-4 iter-1 IR8 + iter-2 JR8: defensive range guard with
            // harmonized "Schema drift:" prefix. v007 CHECK enforces
            // value_bool IN (0,1) but the helper validates independently.
            if b != 0 && b != 1 {
                return Err(OpcGwError::Database(format!(
                    "Schema drift: value_type='Bool' but value_bool={} is out-of-range \
                     for device {}, metric {} (v007 CHECK(value_bool IN (0,1)) was bypassed)",
                    b, device_id, metric_name
                )));
            }
            Ok(Some(MetricType::Bool(b != 0)))
        }
        "String" => {
            let s = value_text.ok_or_else(|| {
                typed_column_drift_err("String", "value_text", device_id, metric_name)
            })?;
            if value_real.is_some() || value_int.is_some() || value_bool.is_some() {
                return Err(typed_column_multi_set_err(
                    "String", device_id, metric_name,
                    value_real.is_some(), value_int.is_some(), value_bool.is_some(), false,
                ));
            }
            Ok(Some(MetricType::String(s)))
        }
        other => Err(OpcGwError::Database(format!(
            "Unknown value_type '{}' for device {}, metric {} — schema drift (v007 CHECK constraint was bypassed)",
            other, device_id, metric_name
        ))),
    }
}

/// Sibling-shaped schema-drift error helpers (iter-2 JR8 harmonization):
/// all three helpers (`typed_column_drift_err`, `typed_column_multi_set_err`,
/// the inline guards for Bool range / Float finiteness) produce
/// `OpcGwError::Database` with a leading "Schema drift: " phrasing followed
/// by the specific drift cause + device/metric context. Log-grep pipelines
/// can filter on the leading "Schema drift" prefix to capture all schema-
/// drift-class errors uniformly.
fn typed_column_drift_err(
    value_type: &str,
    expected_column: &str,
    device_id: &str,
    metric_name: &str,
) -> OpcGwError {
    OpcGwError::Database(format!(
        "Schema drift: value_type='{}' but discriminated column {} IS NULL \
         for device {}, metric {} (v008 cross-column CHECK constraint was bypassed)",
        value_type, expected_column, device_id, metric_name
    ))
}

/// A-4 iter-1 IR2 + iter-2 JR7: report a schema-drift row where multiple
/// typed columns are non-NULL. v008 cross-column CHECK forbids this; the
/// helper is the defensive-only error path. JR7 enhancement: name WHICH
/// other columns are non-NULL so operators can locate the contamination
/// without an extra SQL probe.
fn typed_column_multi_set_err(
    value_type: &str,
    device_id: &str,
    metric_name: &str,
    real_set: bool,
    int_set: bool,
    bool_set: bool,
    text_set: bool,
) -> OpcGwError {
    let mut set_cols = Vec::new();
    if real_set { set_cols.push("value_real"); }
    if int_set { set_cols.push("value_int"); }
    if bool_set { set_cols.push("value_bool"); }
    if text_set { set_cols.push("value_text"); }
    OpcGwError::Database(format!(
        "Schema drift: value_type='{}' but unexpected typed columns are non-NULL [{}] \
         for device {}, metric {} (v008 cross-column CHECK constraint was bypassed)",
        value_type, set_cols.join(", "), device_id, metric_name
    ))
}

impl SqliteBackend {
    /// Convert CommandStatus to database string representation.
    fn status_to_string(status: &CommandStatus) -> &'static str {
        match status {
            CommandStatus::Pending => "Pending",
            CommandStatus::Sent => "Sent",
            CommandStatus::Confirmed => "Confirmed",
            CommandStatus::Failed => "Failed",
        }
    }

    /// Convert database string representation to CommandStatus.
    fn status_from_string(s: &str) -> CommandStatus {
        match s {
            "Sent" => CommandStatus::Sent,
            "Confirmed" => CommandStatus::Confirmed,
            "Failed" => CommandStatus::Failed,
            "Pending" => CommandStatus::Pending,
            _ => {
                warn!("Unknown command status in database: '{}', defaulting to Pending", s);
                CommandStatus::Pending
            }
        }
    }

    /// Map an 11-column `command_queue` SELECT row into a [`Command`],
    /// tolerating NULLs in the nullable v002 columns (GH #134).
    ///
    /// Expected column order:
    /// `id, device_id, command_name, parameters, status, enqueued_at,
    /// sent_at, confirmed_at, error_message, command_hash, chirpstack_result_id`.
    ///
    /// NULL handling: `command_name` and `command_hash` map to `""`,
    /// `parameters` maps to `serde_json::Value::Null` — and so does corrupt
    /// non-NULL JSON (with a warn), because the readers collect rows with
    /// `Result`-collapsing semantics. This is the single shared mapper for
    /// all four `Command` reader queries so one bad row — legacy NULLs from
    /// the pre-fix OPC-UA path or corrupted content — can never collapse an
    /// entire poll into an error.
    fn command_from_row(row: &rusqlite::Row) -> rusqlite::Result<Command> {
        Ok(Command {
            id: row.get::<_, i64>(0)? as u64,
            device_id: row.get(1)?,
            metric_id: String::new(), // Will be populated from config if needed
            command_name: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
            parameters: match row.get::<_, Option<String>>(3)? {
                None => serde_json::Value::Null,
                Some(s) => serde_json::from_str(&s).unwrap_or_else(|e| {
                    warn!(
                        command_id = row.get::<_, i64>(0).unwrap_or(-1),
                        error = %e,
                        "Corrupted command parameters JSON in database; treating as null"
                    );
                    serde_json::Value::Null
                }),
            },
            enqueued_at: row.get::<_, Option<String>>(5)?
                .and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc)))
                .unwrap_or_else(|| {
                    // Legacy pre-#134 rows have NULL enqueued_at; they are
                    // returned by the 5s confirmation poll, so keep this at
                    // debug to avoid per-row log spam on every cycle.
                    debug!("Command missing or unparseable enqueued_at timestamp, using current time");
                    Utc::now()
                }),
            sent_at: row.get::<_, Option<String>>(6)?.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc))),
            confirmed_at: row.get::<_, Option<String>>(7)?.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc))),
            status: Self::status_from_string(&row.get::<_, String>(4)?),
            error_message: row.get(8)?,
            command_hash: row.get::<_, Option<String>>(9)?.unwrap_or_default(),
            chirpstack_result_id: row.get(10)?,
        })
    }

    /// Create a new SqliteBackend with a dedicated single-connection pool (for tests).
    ///
    /// This creates a new connection pool internally with size 1, suitable for testing.
    /// For production use with per-task connections, use `with_pool()` instead.
    ///
    /// # Arguments
    /// * `path` - File system path to the SQLite database (e.g., "data/opcgw.db")
    ///
    /// # Returns
    /// * `Ok(SqliteBackend)` - Successfully initialized backend
    /// * `Err(OpcGwError::Database)` - If database creation or configuration fails
    pub fn new(path: &str) -> Result<Self, OpcGwError> {
        let pool = Arc::new(ConnectionPool::new(path, 1)?);
        Self::with_pool(pool)
    }

    /// Create a new SqliteBackend with a shared connection pool (for production).
    ///
    /// This allows multiple SqliteBackend instances to share the same connection pool,
    /// enabling per-task connections for true concurrent access via WAL mode.
    ///
    /// # Arguments
    /// * `pool` - Arc-wrapped ConnectionPool to use for all database access
    ///
    /// # Returns
    /// * `Ok(SqliteBackend)` - Successfully initialized backend
    /// * `Err(OpcGwError::Database)` - If initial configuration fails
    pub fn with_pool(pool: Arc<ConnectionPool>) -> Result<Self, OpcGwError> {
        // Initialize schema on first connection
        let conn_guard = pool.checkout(Duration::from_secs(5))?;
        schema::run_migrations(&conn_guard)?;
        drop(conn_guard);  // Return connection to pool

        let version = {
            let conn_guard = pool.checkout(Duration::from_secs(5))?;
            let version: i32 = conn_guard
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap_or(0);
            version
        };

        info!(
            version = version,
            "SqliteBackend initialized with per-task connection pool"
        );

        Ok(SqliteBackend { pool, validator: None })
    }

    /// Create a new SqliteBackend with validator support for command parameter validation.
    ///
    /// This method creates a backend with optional command validator for Story 3-2.
    ///
    /// # Arguments
    /// * `pool` - Arc-wrapped connection pool
    /// * `validator` - Optional command validator for parameter validation
    ///
    /// # Returns
    /// * `Ok(SqliteBackend)` - Successfully initialized backend
    pub fn with_pool_and_validator(
        pool: Arc<ConnectionPool>,
        validator: Option<Arc<CommandValidator>>,
    ) -> Result<Self, OpcGwError> {
        // Initialize schema on first connection
        let conn_guard = pool.checkout(Duration::from_secs(5))?;
        schema::run_migrations(&conn_guard)?;
        drop(conn_guard);

        let version = {
            let conn_guard = pool.checkout(Duration::from_secs(5))?;
            let version: i32 = conn_guard
                .pragma_query_value(None, "user_version", |row| row.get(0))
                .unwrap_or(0);
            version
        };

        info!(
            version = version,
            "SqliteBackend initialized with command validator"
        );

        Ok(SqliteBackend { pool, validator })
    }

    /// Legacy: Create a new SqliteBackend with direct path (initializes database).
    ///
    /// This is the original constructor that creates a single-connection pool internally.
    ///
    /// # Arguments
    /// * `path` - File system path to the SQLite database (e.g., "data/opcgw.db")
    ///
    /// # Returns
    /// * `Ok(SqliteBackend)` - Successfully initialized backend
    /// * `Err(OpcGwError::Database)` - If database creation or configuration fails
    pub fn new_with_initialization(path: &str) -> Result<Self, OpcGwError> {
        if path.is_empty() {
            return Err(OpcGwError::Database(
                "Database path cannot be empty".to_string(),
            ));
        }

        // Create parent directory if needed
        let db_path = Path::new(path);
        if let Some(parent) = db_path.parent() {
            if !parent.as_os_str().is_empty() && parent.as_os_str() != "/" {
                std::fs::create_dir_all(parent).map_err(|e| {
                    OpcGwError::Database(format!(
                        "Failed to create database directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }

        // Open connection
        let conn = Connection::open(path).map_err(|e| {
            OpcGwError::Database(format!("Failed to open database at {}: {}", path, e))
        })?;

        // Enable WAL mode for concurrent access
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to enable WAL mode: {}", e))
            })?;

        // Verify WAL mode was enabled
        let journal_mode: String = conn
            .pragma_query_value(None, "journal_mode", |row| row.get(0))
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to verify WAL mode enabled: {}",
                    e
                ))
            })?;

        if journal_mode.to_uppercase() != "WAL" {
            return Err(OpcGwError::Database(format!(
                "WAL mode not enabled; got: {}",
                journal_mode
            )));
        }

        // Configure PRAGMA settings
        conn.pragma_update(None, "foreign_keys", "ON")
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to enable foreign_keys: {}", e))
            })?;

        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to set synchronous=NORMAL: {}", e))
            })?;

        conn.pragma_update(None, "temp_store", "MEMORY")
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to set temp_store=MEMORY: {}", e))
            })?;

        // Run migrations (will initialize schema on fresh database)
        if let Err(e) = schema::run_migrations(&conn) {
            drop(conn);
            let _ = std::fs::remove_file(path);
            return Err(e);
        }

        // Get final version for logging
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to read final schema version: {}", e))
            })?;

        info!(path = path, version = version, "Database initialized");

        let pool = Arc::new(ConnectionPool::new(path, 1)?);
        Self::with_pool(pool)
    }
}

// ============================================================================
// StorageBackend Trait Implementation
// ============================================================================

impl crate::storage::StorageBackend for SqliteBackend {
    fn get_metric(
        &self,
        device_id: &str,
        metric_name: &str,
    ) -> Result<Option<MetricType>, OpcGwError> {
        let mut __op = StorageOpLog::start("get_metric");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %device_id, metric_name = %metric_name, "Pool checkout timeout");
                e
            })?;

        // A-4: project the v007 typed columns + value_type to build the
        // payload-bearing MetricType. Legacy rows surface as Ok(None).
        let result = conn
            .query_row(
                "SELECT value_real, value_int, value_bool, value_text, value_type \
                 FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
                params![device_id, metric_name],
                |row| {
                    Ok((
                        row.get::<_, Option<f64>>(0)?,
                        row.get::<_, Option<i64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to query metric for device {}, metric {}: {}",
                    device_id, metric_name, e
                ))
            })?;

        match result {
            Some((value_real, value_int, value_bool, value_text, value_type)) => {
                let metric_type = metric_type_from_typed_columns(
                    &value_type,
                    value_real,
                    value_int,
                    value_bool,
                    value_text,
                    device_id,
                    metric_name,
                )?;
                trace!(
                    device_id = %device_id,
                    metric_name = %metric_name,
                    value_type = %value_type,
                    "Retrieved metric"
                );
                __op.ok();
                Ok(metric_type)
            }
            None => {
                trace!(
                    device_id = %device_id,
                    metric_name = %metric_name,
                    "Metric not found"
                );
                __op.ok();
                Ok(None)
            }
        }
    }

    fn get_metric_value(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricValue>, OpcGwError> {
        let mut __op = StorageOpLog::start("get_metric_value");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %device_id, metric_name = %metric_name, "Pool checkout timeout");
                e
            })?;

        // A-5: project only v007 typed columns + value_type + timestamp.
        // The legacy `value` column is no longer needed since A-5 removed
        // `MetricValue.value: String`. Legacy rows (value_type='legacy')
        // surface as Ok(None) → BadDataUnavailable via OpcUa::get_value
        // per architecture.md:182.
        let result = conn
            .query_row(
                "SELECT timestamp, value_real, value_int, value_bool, value_text, value_type \
                 FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
                params![device_id, metric_name],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<f64>>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<i64>>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, String>(5)?,
                    ))
                },
            )
            .optional()
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to query metric value for device {}, metric {}: {}",
                    device_id, metric_name, e
                ))
            })?;

        match result {
            Some((timestamp_str, value_real, value_int, value_bool, value_text, value_type)) => {
                let metric_type_opt = metric_type_from_typed_columns(
                    &value_type,
                    value_real,
                    value_int,
                    value_bool,
                    value_text,
                    device_id,
                    metric_name,
                )?;

                let data_type = match metric_type_opt {
                    Some(mt) => mt,
                    None => {
                        // Legacy row: surface as Ok(None) per architecture.md:182.
                        // OpcUa::get_value maps Ok(None) → BadDataUnavailable;
                        // the legacy row gets replaced on the next poll cycle's
                        // UPSERT (poller writes value_type='Float'/'Int'/etc).
                        trace!(
                            device_id = %device_id,
                            metric_name = %metric_name,
                            "Legacy row (value_type='legacy'); returning Ok(None) — BadDataUnavailable until next poll UPSERT"
                        );
                        __op.ok();
                        return Ok(None);
                    }
                };

                let timestamp = DateTime::parse_from_rfc3339(&timestamp_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| {
                        OpcGwError::Database(format!(
                            "Failed to parse timestamp '{}' for {}.{}: {}",
                            timestamp_str, device_id, metric_name, e
                        ))
                    })?;

                __op.ok();
                Ok(Some(MetricValue {
                    device_id: device_id.to_string(),
                    metric_name: metric_name.to_string(),
                    timestamp,
                    data_type,
                }))
            }
            None => {
                trace!(
                    device_id = %device_id,
                    metric_name = %metric_name,
                    "Metric value not found"
                );
                __op.ok();
                Ok(None)
            }
        }
    }

    fn set_metric(
        &self,
        device_id: &str,
        metric_name: &str,
        value: MetricType,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("set_metric");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %device_id, metric_name = %metric_name, "Pool checkout timeout");
                e
            })?;

        // A-3: typed columns populated; legacy `value`/`data_type` retained until A-5/A-7 retires readers.
        let data_type = value.to_string();
        let timestamp = Utc::now().to_rfc3339();
        let value_str = serde_json::to_string(&value).map_err(|e| {
            OpcGwError::Database(format!("Failed to serialize metric value: {}", e))
        })?;

        // A-3 (AC#4) + iter-1 IR6: derive typed-column payload via the
        // private `typed_value_columns` helper (single source of truth).
        let tc = typed_value_columns(&value);

        conn.execute(
                "INSERT OR REPLACE INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_real, value_int, value_bool, value_text, value_type) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), COALESCE((SELECT created_at FROM metric_values WHERE device_id=?1 AND metric_name=?2), datetime('now')), ?6, ?7, ?8, ?9, ?10)",
                params![device_id, metric_name, value_str, data_type, timestamp, tc.value_real, tc.value_int, tc.value_bool, tc.value_text, tc.value_type],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to store metric for device {}, metric {}: {}",
                    device_id, metric_name, e
                ))
            })?;

        trace!(
            device_id = %device_id,
            metric_name = %metric_name,
            "Stored metric"
        );

        __op.ok();
        Ok(())
    }

    fn get_status(&self) -> Result<ChirpstackStatus, OpcGwError> {
        let mut __op = StorageOpLog::start("get_status");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for get_status");
                e
            })?;

        // Query the gateway_status table (id=1) for health metrics
        let result = conn.query_row(
            "SELECT last_poll_timestamp, error_count, chirpstack_available FROM gateway_status WHERE id = 1",
            [],
            |row| {
                let timestamp_str: Option<String> = row.get(0)?;
                let error_count: i32 = row.get(1)?;
                let available: bool = row.get(2)?;
                Ok((timestamp_str, error_count, available))
            },
        );

        match result {
            Ok((timestamp_str, error_count, available)) => {
                let last_poll = timestamp_str.and_then(|ts| {
                    match DateTime::parse_from_rfc3339(&ts) {
                        Ok(dt) => Some(dt.with_timezone(&Utc)),
                        Err(e) => {
                            tracing::warn!(
                                corrupted_timestamp = %ts,
                                error = %e,
                                "Failed to parse last_poll_timestamp from database"
                            );
                            None
                        }
                    }
                });

                __op.ok();
                Ok(ChirpstackStatus {
                    server_available: available,
                    last_poll_time: last_poll,
                    error_count,
                })
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // Gateway status row doesn't exist; return defaults
                __op.ok();
                Ok(ChirpstackStatus {
                    server_available: false,
                    last_poll_time: None,
                    error_count: 0,
                })
            }
            Err(e) => {
                Err(OpcGwError::Database(format!("Failed to query gateway status: {}", e)))
            }
        }
    }

    fn update_status(&self, status: ChirpstackStatus) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("update_status");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for update_status");
                e
            })?;

        // Map ChirpstackStatus to health metrics
        let timestamp_str = status.last_poll_time.map(|t| format_rfc3339(&t));

        conn.execute(
            "INSERT OR REPLACE INTO gateway_status (id, last_poll_timestamp, error_count, chirpstack_available) \
             VALUES (1, ?, ?, ?)",
            params![timestamp_str, status.error_count, status.server_available],
        )
        .map_err(|e| {
            OpcGwError::Database(format!("Failed to update gateway status: {}", e))
        })?;

        debug!("Updated gateway status");
        __op.ok();
        Ok(())
    }

    fn queue_command(&self, command: DeviceCommand) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("queue_command");
        if command.f_port < 1 || command.f_port > 223 {
            return Err(OpcGwError::Database(format!(
                "Invalid f_port {}: must be 1-223",
                command.f_port
            )));
        }

        if command.payload.len() > 250 {
            return Err(OpcGwError::Database(format!(
                "Payload too large: {} bytes (max 250)",
                command.payload.len()
            )));
        }

        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %command.device_id, "Pool checkout timeout for queue_command");
                e
            })?;

        let status_str = Self::status_to_string(&CommandStatus::Pending);
        // Use the project-canonical RFC3339 format so enqueued_at sorts
        // consistently against rows written by enqueue_command (GH #134).
        let now = format_rfc3339(&Utc::now());

        conn.execute(
                "INSERT INTO command_queue (device_id, payload, f_port, status, created_at, updated_at, command_name, enqueued_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    command.device_id,
                    &command.payload,
                    command.f_port as i32,
                    status_str,
                    now,
                    now,
                    command.command_name,
                    now
                ],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to queue command for device {}: {}",
                    command.device_id, e
                ))
            })?;

        debug!(
            device_id = %command.device_id,
            f_port = command.f_port,
            "Queued command"
        );

        __op.ok();
        Ok(())
    }

    fn get_pending_commands(&self) -> Result<Vec<DeviceCommand>, OpcGwError> {
        let mut __op = StorageOpLog::start("get_pending_commands");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for get_pending_commands");
                e
            })?;

        let status_str = Self::status_to_string(&CommandStatus::Pending);
        let mut stmt = conn
            .prepare("SELECT id, device_id, payload, f_port, created_at FROM command_queue WHERE status = ?1 ORDER BY id ASC")
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to prepare statement: {}", e))
            })?;

        let commands = stmt
            .query_map(params![status_str], |row| {
                let id: i64 = row.get(0)?;
                let device_id: String = row.get(1)?;
                let payload: Vec<u8> = row.get(2)?;
                let f_port: i32 = row.get(3)?;
                let created_at_str: String = row.get(4)?;

                if !(1..=223).contains(&f_port) {
                    return Err(rusqlite::Error::InvalidParameterName(
                        format!("Invalid f_port {}: must be 1-223", f_port)
                    ));
                }

                let created_at = DateTime::parse_from_rfc3339(&created_at_str)
                    .map(|dt| dt.with_timezone(&Utc))
                    .map_err(|e| rusqlite::Error::InvalidParameterName(
                        format!("Invalid timestamp format '{}': {}", created_at_str, e)
                    ))?;

                Ok(DeviceCommand {
                    id: id as u64,
                    device_id,
                    payload,
                    f_port: f_port as u8,
                    status: CommandStatus::Pending,
                    created_at,
                    error_message: None,
                    command_name: None, // E-0 drain path does not need the name (GH-134)
                })
            })
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to query pending commands: {}", e))
            })?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to collect pending commands: {}", e))
            })?;

        if !commands.is_empty() {
            debug!(count = commands.len(), "Retrieved pending commands");
        }

        __op.ok();
        Ok(commands)
    }

    fn update_command_status(
        &self,
        command_id: u64,
        status: CommandStatus,
        error_message: Option<String>,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("update_command_status");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, command_id = command_id, "Pool checkout timeout for update_command_status");
                e
            })?;

        let status_str = Self::status_to_string(&status);

        // Only update error_message if status is Failed or error_message is explicitly provided
        // This prevents inadvertently clearing error messages when transitioning between non-Failed states
        let update_sql = if matches!(status, CommandStatus::Failed) || error_message.is_some() {
            "UPDATE command_queue SET status = ?1, error_message = ?2, updated_at = datetime('now') WHERE id = ?3"
        } else {
            "UPDATE command_queue SET status = ?1, updated_at = datetime('now') WHERE id = ?2"
        };

        let rows_affected = if matches!(status, CommandStatus::Failed) || error_message.is_some() {
            conn.execute(update_sql, params![status_str, error_message, command_id as i64])
        } else {
            conn.execute(update_sql, params![status_str, command_id as i64])
        }
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to update command {} status: {}",
                    command_id, e
                ))
            })?;

        if rows_affected == 0 {
            return Err(OpcGwError::Database(format!(
                "Command {} not found",
                command_id
            )));
        }

        debug!(command_id = command_id, status = status_str, "Updated command status");

        __op.ok();
        Ok(())
    }

    /// Atomically insert or update a metric value using UPSERT semantics.
    ///
    /// Uses `INSERT OR REPLACE` with a COALESCE subquery to preserve the `created_at` timestamp
    /// across updates. On the first insert, `created_at` is set to `now_ts`. On subsequent updates
    /// of the same (device_id, metric_name) pair, `created_at` is preserved from the existing row.
    ///
    /// # Parameters
    /// - `device_id`: Device identifier (parameterized to prevent SQL injection)
    /// - `metric_name`: Metric name (parameterized to prevent SQL injection)
    /// - `value`: MetricType enum (Float, Int, Bool, String)
    /// - `now_ts`: SystemTime timestamp for this operation
    ///
    /// # Returns
    /// - `Ok(())` on successful UPSERT
    /// - `Err(OpcGwError::Storage)` if the operation fails
    ///
    /// # Atomicity
    /// The UPSERT operation is atomic: either the entire row is inserted/replaced or the operation
    /// fails with no partial updates.
    fn upsert_metric_value(&self, device_id: &str, metric_name: &str, value: &MetricType, now_ts: std::time::SystemTime) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("upsert_metric_value");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %device_id, metric_name = %metric_name, "Pool checkout timeout for upsert_metric_value");
                e
            })?;

        // A-3: typed columns populated; legacy `value`/`data_type` retained
        // (both = discriminant string per pre-A-3 contract — A-2-iter1-DEF1
        // heterogeneous-lexeme staging) until A-5/A-7 retires readers.
        let value_str = value.to_string();
        let data_type = value.to_string();
        let now_rfc3339 = chrono::DateTime::<Utc>::from(now_ts).to_rfc3339();

        // A-3 (AC#4) + iter-1 IR6: derive typed-column payload via helper.
        let tc = typed_value_columns(value);

        // UPSERT with COALESCE: preserves created_at on update, sets it on first insert
        let query = "INSERT OR REPLACE INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_real, value_int, value_bool, value_text, value_type)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, COALESCE((SELECT created_at FROM metric_values WHERE device_id=?1 AND metric_name=?2), ?6), ?7, ?8, ?9, ?10, ?11)";

        conn.execute(
            query,
            params![device_id, metric_name, value_str, data_type, now_rfc3339, now_rfc3339, tc.value_real, tc.value_int, tc.value_bool, tc.value_text, tc.value_type],
        )
        .map_err(|e| {
            OpcGwError::Storage(format!(
                "Failed to upsert metric for device {}, metric {}: {}",
                device_id, metric_name, e
            ))
        })?;

        trace!(
            device_id = %device_id,
            metric_name = %metric_name,
            value = %value_str,
            "Upserted metric value"
        );

        __op.ok();
        Ok(())
    }

    /// Append a historical metric record to the append-only audit log.
    ///
    /// Uses `INSERT` (not INSERT OR REPLACE) to ensure append-only semantics — new rows are added,
    /// never updating or replacing existing rows. This creates an immutable audit trail of all metric
    /// changes suitable for regulatory compliance, trend analysis, and data provenance tracking.
    ///
    /// # Append-Only Pattern
    ///
    /// - **Never Updates:** Always INSERT. Existing rows are never modified once created.
    /// - **Multiple Entries:** (device_id, metric_name) can have multiple rows at different timestamps.
    /// - **Timestamp Ordered:** Rows maintain insertion order by timestamp for time-range queries.
    /// - **Audit Trail:** Creates immutable historical record for compliance and trend analysis.
    /// - **Index:** Index on (device_id, timestamp) enables efficient range queries (Story 7-3 Phase B).
    ///
    /// # Parameters
    ///
    /// - `device_id`: Device identifier (parameterized to prevent SQL injection)
    /// - `metric_name`: Metric name (parameterized to prevent SQL injection)
    /// - `value`: MetricType enum (Float, Int, Bool, String)
    /// - `timestamp`: SystemTime when this metric was measured
    ///
    /// # Returns
    ///
    /// - `Ok(())` on successful append
    /// - `Err(OpcGwError::Storage)` if the database append fails
    ///
    /// # Data Storage
    ///
    /// **A-1 staging (option-b — see `TODO(A-2)` below):** the `value`
    /// column is currently written via `MetricType::to_string()`, which
    /// renders the *discriminant* ("Float"/"Int"/"Bool"/"String") — not the
    /// payload. This is a legacy single-row method invoked only by the test
    /// fallback; production code reaches the real measurement through
    /// `batch_write_metrics` (which writes `BatchMetricWrite.value` —
    /// the real string-encoded sensor reading) instead. A-2's schema
    /// migration replaces both columns with a typed-payload write.
    ///
    /// The `data_type` column stores the variant name for type preservation:
    /// "Float", "Int", "Bool", "String".
    ///
    /// # Timestamp Ordering (RFC3339)
    ///
    /// Timestamps are stored as RFC3339 strings (ISO8601 with UTC timezone) with microsecond precision.
    /// RFC3339 format is lexicographically sortable and suitable for ORDER BY queries and comparisons.
    /// Microsecond precision ensures accurate retention boundary comparisons in pruning operations.
    fn append_metric_history(&self, device_id: &str, metric_name: &str, value: &MetricType, timestamp: std::time::SystemTime) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("append_metric_history");
        // Validate input lengths to prevent index bloat and DoS
        const MAX_DEVICE_ID_LEN: usize = 256;
        const MAX_METRIC_NAME_LEN: usize = 256;

        if device_id.is_empty() || device_id.len() > MAX_DEVICE_ID_LEN {
            return Err(OpcGwError::Storage(format!(
                "Invalid device_id length: {} (must be 1-{} chars)",
                device_id.len(),
                MAX_DEVICE_ID_LEN
            )));
        }

        if metric_name.is_empty() || metric_name.len() > MAX_METRIC_NAME_LEN {
            return Err(OpcGwError::Storage(format!(
                "Invalid metric_name length: {} (must be 1-{} chars)",
                metric_name.len(),
                MAX_METRIC_NAME_LEN
            )));
        }

        // Retry logic for pool exhaustion: exponential backoff (3 attempts)
        let max_retries = 3;
        let mut retry_count = 0;
        let conn = loop {
            match self.pool.checkout(Duration::from_secs(5)) {
                Ok(c) => break c,
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= max_retries {
                        error!(error = %e, device_id = %device_id, metric_name = %metric_name, retries = retry_count, "Pool exhaustion: checkout timeout after max retries for append_metric_history (may indicate pool undersizing or connection leak)");
                        return Err(e);
                    }
                    let backoff_ms = 100u64 * (2_u64.pow((retry_count - 1) as u32));
                    trace!(attempt = retry_count, backoff_ms = backoff_ms, "Retrying pool checkout for append_metric_history");
                    std::thread::sleep(Duration::from_millis(backoff_ms));
                }
            }
        };

        // A-3: typed columns populated; legacy `value`/`data_type` retained.
        // This single-row method is legacy — only test fallbacks call it.
        // Production poller uses `batch_write_metrics`. Legacy `value`/`data_type`
        // both = discriminant string per pre-A-3 contract (A-2-iter1-DEF1
        // heterogeneous-lexeme staging).
        let value_str = value.to_string();
        let data_type = value.to_string();
        // Use 'Z' suffix for UTC timezone to ensure consistent lexicographic ordering
        // Microsecond precision (%.6f) matches prune cutoff calculation for boundary accuracy
        let dt_utc = chrono::DateTime::<Utc>::from(timestamp);
        let timestamp_rfc3339 = format!("{}Z", dt_utc.format("%Y-%m-%dT%H:%M:%S%.6f"));
        let created_at_rfc3339 = timestamp_rfc3339.clone();

        // A-3 (AC#4) + iter-1 IR6: derive typed-column payload via helper.
        let tc = typed_value_columns(value);

        let query = "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_real, value_int, value_bool, value_text, value_type)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

        conn.execute(
            query,
            params![device_id, metric_name, value_str, data_type, timestamp_rfc3339, created_at_rfc3339, tc.value_real, tc.value_int, tc.value_bool, tc.value_text, tc.value_type],
        )
        .map_err(|e| {
            OpcGwError::Storage(format!(
                "Failed to append metric history for device {}, metric {}: {}",
                device_id, metric_name, e
            ))
        })?;

        trace!(
            device_id = %device_id,
            metric_name = %metric_name,
            value = %value_str,
            "Appended metric to history"
        );

        __op.ok();
        Ok(())
    }

    /// Batch write multiple metrics in a single atomic transaction.
    ///
    /// Executes UPSERT + append-only INSERT for all metrics in one transaction.
    /// Provides atomicity: all succeed or all fail together. Performance: ~100-200ms for 400 metrics.
    fn batch_write_metrics(&self, metrics: Vec<crate::storage::BatchMetricWrite>) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("batch_write_metrics");
        if metrics.is_empty() {
            __op.ok();
            return Ok(());
        }

        let metric_count = metrics.len();

        // Retry logic for pool exhaustion: exponential backoff (3 attempts)
        let max_retries = 3;
        // Story 6-3, AC#7: explicit u32 typing — `retry_count` flows into
        // `log_sqlite_busy_if_applicable` as `retry_attempt: u32`, and a
        // mismatched i32-default would force casts at every call site.
        let mut retry_count: u32 = 0;
        let conn = loop {
            match self.pool.checkout(Duration::from_secs(5)) {
                Ok(c) => break c,
                Err(e) => {
                    retry_count += 1;
                    if retry_count >= max_retries {
                        trace!(error = %e, count = metric_count, retries = retry_count, "Pool checkout timeout after retries for batch_write_metrics");
                        return Err(e);
                    }
                    // Review patch P16: `saturating_sub(1)` instead of
                    // `retry_count - 1` so a defensive call with `retry_count
                    // == 0` (shouldn't happen, but unguarded) doesn't
                    // underflow `0u32 - 1` to `u32::MAX` and crash
                    // `2_u64.pow`.
                    let backoff_ms = 100u64 * (2_u64.pow(retry_count.saturating_sub(1)));
                    trace!(attempt = retry_count, backoff_ms = backoff_ms, "Retrying pool checkout for batch_write_metrics");
                    std::thread::sleep(Duration::from_millis(backoff_ms));
                }
            }
        };

        // Start transaction
        // Story 6-1, AC#6: structured trace logs at transaction boundaries.
        // Review patch P22: emit `txn_begin` only after BEGIN succeeds so
        // log analysis doesn't see an orphan `txn_begin` for a
        // transaction that never actually opened.
        let txn_start = Instant::now();
        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| {
                log_sqlite_busy_if_applicable(
                    &e,
                    "batch_write_begin",
                    retry_count,
                    txn_start.elapsed().as_millis() as u64,
                );
                OpcGwError::Storage(format!("Failed to begin batch transaction: {}", e))
            })?;
        trace!(operation = "txn_begin", operation_count = metric_count);

        for metric in metrics {
            // A-3 (AC#4) + iter-1 IR6: derive typed-column payload via the
            // shared `typed_value_columns` helper (single source of truth
            // across all 4 writer sites). Legacy `value`/`data_type` columns
            // remain in the v007/v008 schema until A-7 drops them; A-5
            // populates both with the discriminant string (`Float`/`Int`/etc)
            // to satisfy the NOT NULL constraints. Readers in A-4/A-5 no
            // longer consult these columns.
            let data_type_str = metric.data_type.to_string();
            let timestamp_rfc3339 = chrono::DateTime::<Utc>::from(metric.timestamp).to_rfc3339();
            let tc = typed_value_columns(&metric.data_type);

            // UPSERT for metric_values
            let upsert_query = "INSERT OR REPLACE INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_real, value_int, value_bool, value_text, value_type)
                                VALUES (?1, ?2, ?3, ?4, ?5, ?6, COALESCE((SELECT created_at FROM metric_values WHERE device_id=?1 AND metric_name=?2), ?6), ?7, ?8, ?9, ?10, ?11)";

            let upsert_start = Instant::now();
            conn.execute(
                upsert_query,
                params![&metric.device_id, &metric.metric_name, &data_type_str, &data_type_str, timestamp_rfc3339, timestamp_rfc3339, tc.value_real, tc.value_int, tc.value_bool, tc.value_text, tc.value_type],
            )
            .map_err(|e| {
                log_sqlite_busy_if_applicable(
                    &e,
                    "batch_write_upsert",
                    retry_count,
                    upsert_start.elapsed().as_millis() as u64,
                );
                // Review patch P17: emit the structured rollback log
                // *after* rollback completes, distinguishing success
                // (`txn_rollback`) from failure (`txn_rollback_failed`).
                // The previous trace fired before the rollback ran and
                // could pair with an error showing the rollback itself
                // failed — chronologically misleading.
                match conn.execute_batch("ROLLBACK") {
                    Ok(_) => trace!(operation = "txn_rollback", operation_count = metric_count),
                    Err(rollback_err) => error!(
                        operation = "txn_rollback_failed",
                        error = %rollback_err,
                        "Failed to rollback transaction after upsert error"
                    ),
                }
                OpcGwError::Storage(format!(
                    "Failed to upsert metric in batch for device {}, metric {}: {}",
                    metric.device_id, metric.metric_name, e
                ))
            })?;

            // INSERT for metric_history
            let history_timestamp = Utc::now().to_rfc3339();
            let insert_query = "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_real, value_int, value_bool, value_text, value_type)
                                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)";

            conn.execute(
                insert_query,
                params![&metric.device_id, &metric.metric_name, &data_type_str, &data_type_str, timestamp_rfc3339, history_timestamp, tc.value_real, tc.value_int, tc.value_bool, tc.value_text, tc.value_type],
            )
            .map_err(|e| {
                // Review patch P17: see upsert site above for rationale.
                match conn.execute_batch("ROLLBACK") {
                    Ok(_) => trace!(operation = "txn_rollback", operation_count = metric_count),
                    Err(rollback_err) => error!(
                        operation = "txn_rollback_failed",
                        error = %rollback_err,
                        "Failed to rollback transaction after history insert error"
                    ),
                }
                OpcGwError::Storage(format!(
                    "Failed to append metric to history in batch for device {}, metric {}: {}",
                    metric.device_id, metric.metric_name, e
                ))
            })?;
        }

        // Commit transaction
        conn.execute_batch("COMMIT")
            .map_err(|e| {
                // Review patch P17: see upsert site above for rationale.
                match conn.execute_batch("ROLLBACK") {
                    Ok(_) => trace!(operation = "txn_rollback", operation_count = metric_count),
                    Err(rollback_err) => error!(
                        operation = "txn_rollback_failed",
                        error = %rollback_err,
                        "Failed to rollback transaction after commit error"
                    ),
                }
                OpcGwError::Storage(format!("Failed to commit batch transaction: {}", e))
            })?;
        trace!(operation = "txn_commit", operation_count = metric_count);

        debug!(count = metric_count, "Batch wrote metrics in single transaction");

        __op.ok();
        Ok(())
    }

    fn load_all_metrics(&self) -> Result<Vec<MetricValue>, OpcGwError> {
        let mut __op = StorageOpLog::start("load_all_metrics");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for load_all_metrics");
                e
            })?;

        // A-5: project v007 typed columns only; legacy `value` column no
        // longer needed since MetricValue.value: String was removed. Legacy
        // rows skipped silently (partial-success contract — startup restore
        // skips rows with no real payload yet; the next poll cycle UPSERTs).
        let mut stmt = conn.prepare(
            "SELECT device_id, metric_name, timestamp, \
                    value_real, value_int, value_bool, value_text, value_type \
             FROM metric_values ORDER BY device_id, metric_name"
        )
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to prepare load_all_metrics query: {}", e))
            })?;

        let metrics = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0),         // device_id
                row.get::<_, String>(1),         // metric_name
                row.get::<_, String>(2),         // timestamp
                row.get::<_, Option<f64>>(3),    // value_real
                row.get::<_, Option<i64>>(4),    // value_int
                row.get::<_, Option<i64>>(5),    // value_bool
                row.get::<_, Option<String>>(6), // value_text
                row.get::<_, String>(7),         // value_type
            ))
        })
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to query metrics: {}", e))
            })?;

        let mut result = Vec::new();
        let mut skipped_count = 0;
        let mut legacy_skipped_count = 0;
        let mut valid_count = 0;

        for metric_result in metrics {
            let row = match metric_result {
                Ok(tuple) => tuple,
                Err(e) => {
                    trace!(error = %e, "Failed to extract metric row columns");
                    skipped_count += 1;
                    continue;
                }
            };

            let (device_id_res, metric_name_res, timestamp_str_res,
                 value_real_res, value_int_res, value_bool_res, value_text_res, value_type_res) = row;

            let device_id = match device_id_res {
                Ok(id) => id,
                Err(e) => {
                    trace!(error = %e, "Failed to extract device_id from metric row");
                    skipped_count += 1;
                    continue;
                }
            };

            let metric_name = match metric_name_res {
                Ok(name) => name,
                Err(e) => {
                    trace!(error = %e, device_id = %device_id, "Failed to extract metric_name from metric row");
                    skipped_count += 1;
                    continue;
                }
            };

            let timestamp_str = match timestamp_str_res {
                Ok(s) => s,
                Err(e) => {
                    trace!(error = %e, device_id = %device_id, metric_name = %metric_name, "Failed to extract timestamp from metric row");
                    skipped_count += 1;
                    continue;
                }
            };

            let value_real = value_real_res.ok();
            let value_int = value_int_res.ok();
            let value_bool = value_bool_res.ok();
            let value_text = value_text_res.ok().flatten();

            let value_type = match value_type_res {
                Ok(s) => s,
                Err(e) => {
                    trace!(error = %e, device_id = %device_id, metric_name = %metric_name, "Failed to extract value_type from metric row");
                    skipped_count += 1;
                    continue;
                }
            };

            // A-4: build payload-bearing MetricType from typed columns.
            // Legacy rows skipped silently (no real payload yet).
            let metric_type_opt = match metric_type_from_typed_columns(
                &value_type,
                value_real.flatten(),
                value_int.flatten(),
                value_bool.flatten(),
                value_text,
                &device_id,
                &metric_name,
            ) {
                Ok(opt) => opt,
                Err(e) => {
                    error!(
                        device_id = %device_id,
                        metric_name = %metric_name,
                        value_type = %value_type,
                        error = %e,
                        "Failed to project typed columns to MetricType; row skipped"
                    );
                    skipped_count += 1;
                    continue;
                }
            };

            let data_type = match metric_type_opt {
                Some(mt) => mt,
                None => {
                    // Legacy row — skipped silently with trace emission.
                    trace!(
                        device_id = %device_id,
                        metric_name = %metric_name,
                        "load_all_metrics: skipping legacy row (value_type='legacy')"
                    );
                    legacy_skipped_count += 1;
                    continue;
                }
            };

            // Parse timestamp: use Utc::now() as fallback if RFC3339 parse fails
            let timestamp = match chrono::DateTime::parse_from_rfc3339(&timestamp_str) {
                Ok(dt) => dt.with_timezone(&Utc),
                Err(_) => {
                    error!(
                        device_id = %device_id,
                        metric_name = %metric_name,
                        invalid_timestamp = %timestamp_str,
                        fallback = "using current UTC time",
                        error = "invalid timestamp format",
                        "Failed to parse metric timestamp; using fallback"
                    );
                    Utc::now()
                }
            };

            result.push(MetricValue {
                device_id,
                metric_name,
                timestamp,
                data_type,
            });
            valid_count += 1;
        }

        // A-4 iter-1 IR5 + iter-2 JR5: surface skipped-row counts at info!
        // level when ANY rows were skipped (legacy OR schema-drift). The
        // iter-1 IR5 hid schema-drift-only skips at trace level — a
        // post-A-4 stabilized system where the only failures are real
        // schema drift would have an operator-invisible signal. JR5
        // coalesces: any skip > 0 → info! line with both counts so
        // operators see both classes uniformly. Schema-drift skips are
        // operationally MORE serious than legacy skips (the per-row
        // `error!` above carries the canonical failure detail; this
        // summary line is the aggregate signal).
        if legacy_skipped_count > 0 || skipped_count > 0 {
            info!(
                event = "load_all_metrics",
                valid = valid_count,
                legacy_skipped = legacy_skipped_count,
                schema_drift_skipped = skipped_count,
                "load_all_metrics: some rows skipped (legacy rows will be replaced on next poll-cycle UPSERT; schema-drift rows require operator intervention — see per-row error logs above)"
            );
        } else {
            debug!(count = valid_count, "Loaded all metrics from storage");
        }

        __op.ok();
        Ok(result)
    }

    fn prune_metric_history(&self) -> Result<u32, OpcGwError> {
        let mut __op = StorageOpLog::start("prune_metric_history");
        let start = std::time::Instant::now();

        // Checkout connection from pool
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                error!(error = %e, "Pool checkout timeout for prune_metric_history");
                e
            })?;

        // Read retention policy from retention_config (not cached)
        let retention_days: i64 = conn
            .query_row(
                "SELECT retention_days FROM retention_config WHERE data_type = 'metric_history'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| {
                error!(error = %e, "Missing or invalid retention_config for metric_history");
                OpcGwError::Database("Missing retention_config for metric_history".to_string())
            })?;

        // Validate retention_days is positive (safety guardrail per AC#2)
        if retention_days <= 0 {
            error!(retention_days = retention_days, "Invalid retention_days: must be positive");
            return Err(OpcGwError::Database(
                format!("Invalid retention_days: {} (must be positive)", retention_days)
            ));
        }

        // Calculate cutoff timestamp (RFC3339 format with microsecond precision + Z suffix for UTC)
        let cutoff = Utc::now() - chrono::Duration::days(retention_days);
        let mut cutoff_rfc3339 = format!("{}", cutoff.format("%Y-%m-%dT%H:%M:%S%.6f"));
        cutoff_rfc3339.push('Z');

        // Execute DELETE with parameterized query (AC#2: exclude NULL timestamps)
        conn.execute(
            "DELETE FROM metric_history WHERE timestamp < ?1 AND timestamp IS NOT NULL",
            params![&cutoff_rfc3339],
        )
        .map_err(|e| {
            error!(error = %e, "Failed to delete expired metrics");
            OpcGwError::Database(format!("Failed to prune metric_history: {}", e))
        })?;

        // Get count of deleted rows (AC#3: efficient deletion via index scan)
        let deleted_count = conn.changes() as u32;
        let duration_ms = start.elapsed().as_millis() as u64;

        // Log results (AC#4: structured logging)
        if deleted_count > 0 {
            debug!(
                table_name = "metric_history",
                deleted_count = deleted_count,
                retention_days = retention_days,
                timestamp_cutoff = %cutoff_rfc3339,
                duration_ms = duration_ms,
                "Pruned metric_history: {} rows deleted (retention > {} days, cutoff: {})",
                deleted_count,
                retention_days,
                cutoff_rfc3339
            );
        } else {
            debug!(
                table_name = "metric_history",
                deleted_count = deleted_count,
                retention_days = retention_days,
                timestamp_cutoff = %cutoff_rfc3339,
                duration_ms = duration_ms,
                "No expired metrics to prune (retention > {} days)",
                retention_days
            );
        }

        __op.ok();
        Ok(deleted_count)
    }

    fn query_metric_history(
        &self,
        device_id: &str,
        metric_name: &str,
        start: std::time::SystemTime,
        end: std::time::SystemTime,
        max_results: usize,
    ) -> Result<Vec<crate::storage::HistoricalMetricRow>, OpcGwError> {
        let mut __op = StorageOpLog::start("query_metric_history");

        // Format timestamps to match the production write path
        // (`batch_write_metrics:1048`: `chrono::DateTime::<Utc>::from(...).to_rfc3339()`).
        // ISO8601 with timezone offset is what's stored, so lexicographic
        // string comparison matches chronological order only if the bind
        // values use the SAME format. NOTE: the legacy `append_metric_history`
        // method at `:957-960` uses a different format (microsecond + `Z`)
        // — that method is test-only and only writes the type variant name,
        // so historical reads via this method don't see those rows in
        // production.
        let start_iso = chrono::DateTime::<Utc>::from(start).to_rfc3339();
        let end_iso = chrono::DateTime::<Utc>::from(end).to_rfc3339();

        let conn = self.pool.checkout(Duration::from_secs(5))?;

        // A-5: project v007 typed columns + value_type. The legacy `value,
        // data_type` projection is gone since A-3's writer pipeline populates
        // the typed columns natively. The shared helper `metric_type_from_typed_columns`
        // (A-4) builds the payload-bearing `MetricType`; legacy rows
        // (`value_type='legacy'`) surface as `Ok(None)` and the row stores
        // `payload: None` so `OpcgwHistoryNodeManagerImpl::build_data_values`
        // can emit `BadDataUnavailable` per epic AC#1 (legacy rows are NOT
        // silently dropped — they appear in the response stream).
        //
        // Half-open interval `start <= timestamp < end`. Uses the
        // `idx_metric_history_device_timestamp` composite index for time-range
        // seeks; LIMIT is a hard cap on response size.
        let query = "SELECT value_real, value_int, value_bool, value_text, value_type, timestamp \
                     FROM metric_history \
                     WHERE device_id = ?1 AND metric_name = ?2 \
                     AND timestamp >= ?3 AND timestamp < ?4 \
                     ORDER BY timestamp ASC LIMIT ?5";

        let mut stmt = conn.prepare(query).map_err(|e| {
            OpcGwError::Storage(format!("Failed to prepare query_metric_history statement: {e}"))
        })?;

        // Review patch P17: saturate the cast — `max_results: usize` could
        // be > i64::MAX on a 64-bit host, and a wrapped negative LIMIT in
        // SQLite means "no limit" (silently disabling the per-call DoS cap).
        let max_results_i64 = i64::try_from(max_results).unwrap_or(i64::MAX);

        let rows = stmt
            .query_map(
                params![device_id, metric_name, start_iso, end_iso, max_results_i64],
                |row| {
                    Ok((
                        row.get::<_, Option<f64>>(0)?,    // value_real
                        row.get::<_, Option<i64>>(1)?,    // value_int
                        row.get::<_, Option<i64>>(2)?,    // value_bool
                        row.get::<_, Option<String>>(3)?, // value_text
                        row.get::<_, String>(4)?,         // value_type
                        row.get::<_, String>(5)?,         // timestamp
                    ))
                },
            )
            .map_err(|e| {
                OpcGwError::Storage(format!("Failed to execute query_metric_history: {e}"))
            })?;

        let mut results: Vec<crate::storage::HistoricalMetricRow> = Vec::new();
        let mut schema_drift_skipped = 0;
        let mut unparseable_timestamp_skipped = 0;
        for row in rows {
            let (value_real, value_int, value_bool, value_text, value_type, timestamp_str) =
                row.map_err(|e| {
                    OpcGwError::Storage(format!("Failed to read query_metric_history row: {e}"))
                })?;

            // A-5: build payload-bearing MetricType from typed columns via
            // the A-4-shared helper. Legacy rows → `Ok(None)` → `payload: None`
            // → BadDataUnavailable DataValue emitted by build_data_values.
            // Schema-drift rows → `Err(_)` → row-skip with warn (AC#11).
            let payload = match metric_type_from_typed_columns(
                &value_type,
                value_real,
                value_int,
                value_bool,
                value_text,
                device_id,
                metric_name,
            ) {
                Ok(opt) => opt,
                Err(e) => {
                    warn!(
                        event = "metric_history_read",
                        reason = "schema_drift",
                        device_id = %device_id,
                        metric_name = %metric_name,
                        value_type = %value_type,
                        error = %e,
                        "query_metric_history: skipping row due to schema drift"
                    );
                    schema_drift_skipped += 1;
                    continue;
                }
            };

            // Parse the ISO8601 timestamp back to SystemTime.
            // A-5 P2 iter-1 review fix: `unparseable_timestamp` is now a
            // first-class member of the metric_history_read closed reason
            // enum (was previously a sub-field of reason=schema_drift via
            // `reason_detail`, which violated the docs/logging.md closed-
            // enum claim). Per the audit-event taxonomy, every distinct
            // failure mode gets its own reason value.
            let timestamp = match chrono::DateTime::parse_from_rfc3339(&timestamp_str) {
                Ok(dt) => std::time::SystemTime::from(dt.with_timezone(&Utc)),
                Err(e) => {
                    warn!(
                        event = "metric_history_read",
                        reason = "unparseable_timestamp",
                        device_id = %device_id,
                        metric_name = %metric_name,
                        timestamp_str = %timestamp_str,
                        error = %e,
                        "query_metric_history: skipping row with unparseable timestamp"
                    );
                    unparseable_timestamp_skipped += 1;
                    continue;
                }
            };

            results.push(crate::storage::HistoricalMetricRow {
                payload,
                timestamp,
            });
        }

        // A-5 P3 iter-1 review fix: aggregate-warn dropped (per-row warns
        // already cover each skip with full context).
        // A-5 K6 iter-2 review fix: restore the `event=` field at trace
        // level under a DISTINCT event name `metric_history_summary` so
        // ops dashboards filtering for aggregate skip counts can still
        // grep-recover the cumulative numbers without confusing them
        // with per-row `event=metric_history_read` emissions.
        if schema_drift_skipped > 0 || unparseable_timestamp_skipped > 0 {
            trace!(
                event = "metric_history_summary",
                device_id = %device_id,
                metric_name = %metric_name,
                schema_drift_skipped,
                unparseable_timestamp_skipped,
                "query_metric_history: row-skip telemetry"
            );
        }

        trace!(
            device_id = %device_id,
            metric_name = %metric_name,
            row_count = results.len(),
            "query_metric_history complete"
        );

        __op.ok();
        Ok(results)
    }

    // ===== Story 3-1: High-level Command Queue =====

    fn enqueue_command(&self, command: Command) -> Result<u64, OpcGwError> {
        let mut __op = StorageOpLog::start("enqueue_command");
        // Validate command_hash is not empty
        if command.command_hash.is_empty() {
            return Err(OpcGwError::Storage("Command hash cannot be empty".to_string()));
        }

        // Validate command parameters if validator is configured (Story 3-2)
        if let Some(validator) = &self.validator {
            validator.validate_command_parameters(
                &command.device_id,
                &command.command_name,
                &command.parameters,
            )?;
        } else {
            tracing::warn!("Command validator not configured; skipping parameter validation");
        }

        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %command.device_id, "Pool checkout timeout for enqueue_command");
                e
            })?;

        // Check for duplicate command (deduplication on pending commands)
        let exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM command_queue WHERE command_hash = ?1 AND status = 'Pending'",
            params![&command.command_hash],
            |row| row.get(0),
        )
        .unwrap_or(false);

        if exists {
            return Err(OpcGwError::Storage(
                format!("Duplicate command already queued: {} for device {}",
                        command.command_name, command.device_id)
            ));
        }

        let now = Utc::now();
        let now_rfc3339 = format_rfc3339(&now);

        let status_str = Self::status_to_string(&command.status);

        // Format enqueued_at timestamp (RFC3339 with microseconds)
        let enqueued_at_rfc3339 = format_rfc3339(&command.enqueued_at);

        conn.execute(
            "INSERT INTO command_queue (device_id, payload, f_port, command_name, parameters, status, created_at, updated_at, enqueued_at, command_hash)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                &command.device_id,
                None::<Vec<u8>>,  // payload: NULL for high-level commands
                None::<i32>,      // f_port: NULL for high-level commands
                &command.command_name,
                command.parameters.to_string(),
                status_str,
                &now_rfc3339,
                &now_rfc3339,
                &enqueued_at_rfc3339,
                &command.command_hash,
            ],
        )
        .map_err(|e| {
            OpcGwError::Database(format!(
                "Failed to enqueue command for device {}: {}",
                command.device_id, e
            ))
        })?;

        let command_id = conn.last_insert_rowid() as u64;
        info!(command_id = command_id, device_id = %command.device_id, command_name = %command.command_name, status = %command.status, "Command enqueued");

        __op.ok();
        Ok(command_id)
    }

    fn dequeue_command(&self) -> Result<Option<Command>, OpcGwError> {
        let mut __op = StorageOpLog::start("dequeue_command");
        let mut conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for dequeue_command");
                e
            })?;

        // Get the next pending command and update its status to Sent
        // Use IMMEDIATE to acquire write lock immediately, preventing race conditions
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|e| OpcGwError::Database(format!("Failed to start transaction: {}", e)))?;

        let command = tx.query_row(
            "SELECT id, device_id, command_name, parameters, status, enqueued_at, sent_at, confirmed_at, error_message, command_hash, chirpstack_result_id
             FROM command_queue WHERE status = 'Pending' ORDER BY id ASC LIMIT 1",
            [],
            Self::command_from_row,
        ).optional()
        .map_err(|e| {
            OpcGwError::Database(format!("Failed to dequeue command: {}", e))
        })?;

        if let Some(ref cmd) = command {
            // Update status to Sent to prevent requeuing
            let now = Utc::now();
            let now_rfc3339 = format_rfc3339(&now);

            tx.execute(
                "UPDATE command_queue SET status = 'Sent', sent_at = ?1, updated_at = ?2 WHERE id = ?3",
                rusqlite::params![&now_rfc3339, &now_rfc3339, cmd.id as i64],
            ).map_err(|e| {
                OpcGwError::Database(format!("Failed to update command status after dequeue: {}", e))
            })?;

            tx.commit()
                .map_err(|e| OpcGwError::Database(format!("Failed to commit dequeue transaction: {}", e)))?;

            info!(command_id = cmd.id, device_id = %cmd.device_id, command_name = %cmd.command_name, old_status = "Pending", new_status = "Sent", "Command status transition");
        }

        __op.ok();
        Ok(command)
    }

    fn list_commands(&self, filter: &CommandFilter) -> Result<Vec<Command>, OpcGwError> {
        let mut __op = StorageOpLog::start("list_commands");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for list_commands");
                e
            })?;

        let mut query = "SELECT id, device_id, command_name, parameters, status, enqueued_at, sent_at, confirmed_at, error_message, command_hash, chirpstack_result_id
                         FROM command_queue WHERE 1=1".to_string();
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

        if let Some(device_id) = &filter.device_id {
            query.push_str(" AND device_id = ?");
            params.push(Box::new(device_id.clone()));
        }

        if let Some(status) = filter.status {
            let status_str = Self::status_to_string(&status);
            query.push_str(" AND status = ?");
            params.push(Box::new(status_str.to_string()));
        }

        if let Some(cmd_name) = &filter.command_name_contains {
            // Escape LIKE wildcards in the search term (escape backslash first)
            let escaped = cmd_name.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
            query.push_str(" AND command_name LIKE ? ESCAPE '\\'");
            params.push(Box::new(format!("%{}%", escaped)));
        }

        if let Some(days) = filter.older_than_days {
            // Filter commands older than N days (based on enqueued_at timestamp)
            let cutoff_date = Utc::now() - chrono::Duration::days(days as i64);
            let cutoff_rfc3339 = format!("{}", cutoff_date.format("%Y-%m-%dT%H:%M:%S%.6fZ"));
            query.push_str(" AND enqueued_at < ?");
            params.push(Box::new(cutoff_rfc3339));
        }

        query.push_str(" ORDER BY id ASC");

        let mut stmt = conn.prepare(&query)
            .map_err(|e| OpcGwError::Database(format!("Failed to prepare command list query: {}", e)))?;

        let commands = stmt.query_map(rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())), Self::command_from_row)
        .map_err(|e| OpcGwError::Database(format!("Failed to query commands: {}", e)))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| OpcGwError::Database(format!("Failed to collect commands: {}", e)))?;

        debug!(count = commands.len(), "Retrieved commands with filter");

        __op.ok();
        Ok(commands)
    }

    fn get_queue_depth(&self) -> Result<usize, OpcGwError> {
        let mut __op = StorageOpLog::start("get_queue_depth");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for get_queue_depth");
                e
            })?;

        let depth: usize = conn
            .query_row(
                "SELECT COUNT(*) FROM command_queue WHERE status = 'Pending'",
                [],
                |row| row.get::<_, i64>(0).map(|v| v as usize),
            )
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to get queue depth: {}", e))
            })?;

        __op.ok();
        Ok(depth)
    }

    fn mark_command_sent(&self, command_id: u64, chirpstack_result_id: &str) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("mark_command_sent");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, command_id, "Pool checkout timeout for mark_command_sent");
                e
            })?;

        let now = format_rfc3339(&Utc::now());

        conn.execute(
            "UPDATE command_queue SET status = 'Sent', sent_at = ?, chirpstack_result_id = ?, updated_at = ? WHERE id = ?",
            params![&now, chirpstack_result_id, &now, command_id as i64],
        )
            .map_err(|e| OpcGwError::Database(format!("Failed to mark command as sent: {}", e)))?;

        debug!(command_id, chirpstack_result_id, "Marked command as sent");
        __op.ok();
        Ok(())
    }

    fn mark_command_confirmed(&self, command_id: u64) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("mark_command_confirmed");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, command_id, "Pool checkout timeout for mark_command_confirmed");
                e
            })?;

        let now = format_rfc3339(&Utc::now());

        let rows_affected = conn.execute(
            "UPDATE command_queue SET status = 'Confirmed', confirmed_at = COALESCE(confirmed_at, ?), updated_at = ? WHERE id = ? AND status IN ('Sent', 'Pending')",
            params![&now, &now, command_id as i64],
        )
            .map_err(|e| OpcGwError::Database(format!("Failed to mark command as confirmed: {}", e)))?;

        if rows_affected == 0 {
            return Err(OpcGwError::Database(format!("Command {} not found or already in terminal state", command_id)));
        }

        debug!(command_id, "Marked command as confirmed");
        __op.ok();
        Ok(())
    }

    fn mark_command_failed(&self, command_id: u64, error_message: &str) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("mark_command_failed");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, command_id, "Pool checkout timeout for mark_command_failed");
                e
            })?;

        if error_message.len() > 1000 {
            warn!(command_id, msg_len = error_message.len(), "Error message truncated (max 1000 chars)");
        }

        let now = format_rfc3339(&Utc::now());
        let truncated_msg = if error_message.len() > 1000 {
            &error_message[..1000]
        } else {
            error_message
        };

        let rows_affected = conn.execute(
            "UPDATE command_queue SET status = 'Failed', error_message = ?, updated_at = ? WHERE id = ? AND status IN ('Sent', 'Pending')",
            params![truncated_msg, &now, command_id as i64],
        )
            .map_err(|e| OpcGwError::Database(format!("Failed to mark command as failed: {}", e)))?;

        if rows_affected == 0 {
            return Err(OpcGwError::Database(format!("Command {} not found or already in terminal state", command_id)));
        }

        debug!(command_id, error_message = truncated_msg, "Marked command as failed");
        __op.ok();
        Ok(())
    }

    fn find_pending_confirmations(&self) -> Result<Vec<Command>, OpcGwError> {
        let mut __op = StorageOpLog::start("find_pending_confirmations");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for find_pending_confirmations");
                e
            })?;

        let mut stmt = conn.prepare(
            "SELECT id, device_id, command_name, parameters, status, enqueued_at, sent_at, confirmed_at, \
             error_message, command_hash, chirpstack_result_id FROM command_queue \
             WHERE status = 'Sent' AND confirmed_at IS NULL \
             ORDER BY enqueued_at ASC LIMIT 1000"
        )
            .map_err(|e| OpcGwError::Database(format!("Failed to prepare statement: {}", e)))?;

        let commands = stmt.query_map([], Self::command_from_row)
            .map_err(|e| OpcGwError::Database(format!("Failed to query pending confirmations: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| OpcGwError::Database(format!("Failed to collect commands: {}", e)))?;

        debug!(count = commands.len(), "Checked for pending confirmations");
        __op.ok();
        Ok(commands)
    }

    fn find_timed_out_commands(&self, ttl_secs: u32) -> Result<Vec<Command>, OpcGwError> {
        let mut __op = StorageOpLog::start("find_timed_out_commands");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, ttl_secs, "Pool checkout timeout for find_timed_out_commands");
                e
            })?;

        let cutoff_time = Utc::now() - std::time::Duration::from_secs(ttl_secs as u64);
        let cutoff_rfc3339 = format_rfc3339(&cutoff_time);

        let mut stmt = conn.prepare(
            "SELECT id, device_id, command_name, parameters, status, enqueued_at, sent_at, confirmed_at, \
             error_message, command_hash, chirpstack_result_id FROM command_queue \
             WHERE status = 'Sent' AND sent_at IS NOT NULL AND sent_at < ? \
             ORDER BY enqueued_at ASC LIMIT 1000"
        )
            .map_err(|e| OpcGwError::Database(format!("Failed to prepare statement: {}", e)))?;

        let commands = stmt.query_map(params![&cutoff_rfc3339], Self::command_from_row)
            .map_err(|e| OpcGwError::Database(format!("Failed to query timed out commands: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| OpcGwError::Database(format!("Failed to collect commands: {}", e)))?;

        debug!(count = commands.len(), ttl_secs, "Checked for timed-out commands");
        __op.ok();
        Ok(commands)
    }

    fn find_command_by_result_id(&self, result_id: &str) -> Result<Option<Command>, OpcGwError> {
        // An empty result id must never correlate (review iter-1): if ChirpStack
        // ever returned an empty enqueue id, several commands could share
        // `chirpstack_result_id = ''`, and a `LIMIT 1` lookup would pick an
        // arbitrary one. Treat empty as "no match" (the parse layer already
        // drops empty-id acks, so this is belt-and-suspenders).
        if result_id.is_empty() {
            return Ok(None);
        }
        let mut __op = StorageOpLog::start("find_command_by_result_id");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for find_command_by_result_id");
                e
            })?;

        // Uses the same NULL-safe shared mapper (GH #134) as the other command
        // readers so a legacy/partial row can never collapse the lookup.
        let mut stmt = conn.prepare(
            "SELECT id, device_id, command_name, parameters, status, enqueued_at, sent_at, confirmed_at, \
             error_message, command_hash, chirpstack_result_id FROM command_queue \
             WHERE chirpstack_result_id = ? LIMIT 1"
        )
            .map_err(|e| OpcGwError::Database(format!("Failed to prepare statement: {}", e)))?;

        let command = stmt.query_row(params![result_id], Self::command_from_row)
            .optional()
            .map_err(|e| OpcGwError::Database(format!("Failed to query command by result id: {}", e)))?;

        __op.ok();
        Ok(command)
    }

    fn recent_commands(&self, limit: usize) -> Result<Vec<Command>, OpcGwError> {
        let mut __op = StorageOpLog::start("recent_commands");
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for recent_commands");
                e
            })?;

        // Bounded at the query layer (NULL enqueued_at sorts last under DESC, so
        // legacy pre-#134 rows don't masquerade as newest). NULL-safe shared
        // mapper (GH #134).
        let mut stmt = conn.prepare(
            "SELECT id, device_id, command_name, parameters, status, enqueued_at, sent_at, confirmed_at, \
             error_message, command_hash, chirpstack_result_id FROM command_queue \
             ORDER BY enqueued_at DESC LIMIT ?"
        )
            .map_err(|e| OpcGwError::Database(format!("Failed to prepare statement: {}", e)))?;

        let commands = stmt.query_map(params![limit as i64], Self::command_from_row)
            .map_err(|e| OpcGwError::Database(format!("Failed to query recent commands: {}", e)))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| OpcGwError::Database(format!("Failed to collect recent commands: {}", e)))?;

        __op.ok();
        Ok(commands)
    }

    fn update_gateway_status(
        &self,
        last_poll_timestamp: Option<DateTime<Utc>>,
        error_count: i32,
        chirpstack_available: bool,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("update_gateway_status");
        let conn = self.pool.checkout(std::time::Duration::from_secs(5)).map_err(|e| {
            OpcGwError::Storage(format!("Failed to get database connection for gateway status update: {}", e))
        })?;

        // Format timestamp if present
        let timestamp_str = last_poll_timestamp.map(|ts| format_rfc3339(&ts));

        // SQL uses CASE WHEN to conditionally update timestamp only if new one is provided
        conn.execute(
            "INSERT OR REPLACE INTO gateway_status (id, last_poll_timestamp, error_count, chirpstack_available) \
             VALUES (1, CASE WHEN ? IS NOT NULL THEN ? ELSE (SELECT last_poll_timestamp FROM gateway_status WHERE id = 1) END, ?, ?)",
            params![timestamp_str, timestamp_str, error_count, chirpstack_available],
        ).map_err(|e| {
            OpcGwError::Storage(format!("Failed to update gateway health status: {}", e))
        })?;
        __op.ok();

        debug!(
            last_poll_timestamp = ?last_poll_timestamp,
            error_count = error_count,
            chirpstack_available = chirpstack_available,
            "Updated gateway health status"
        );
        Ok(())
    }

    fn record_error_event(&self, event: &ErrorEvent) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("record_error_event");
        let conn = self.pool.checkout(std::time::Duration::from_secs(5)).map_err(|e| {
            OpcGwError::Storage(format!("Failed to get database connection for error event: {}", e))
        })?;

        let ts_str = format_rfc3339(&event.ts);
        conn.execute(
            "INSERT INTO error_events (ts, category, device_id, application_id, message) \
             VALUES (?, ?, ?, ?, ?)",
            params![ts_str, event.category, event.device_id, event.application_id, event.message],
        ).map_err(|e| {
            OpcGwError::Storage(format!("Failed to insert error event: {}", e))
        })?;

        // Ring-buffer prune: keep only the newest `cap` rows by id. Using
        // `NOT IN (… ORDER BY id DESC LIMIT cap)` (not `id <= MAX(id) - cap`)
        // is correct even when ids are non-contiguous after earlier prunes.
        let cap = crate::utils::error_event_cap() as i64;
        conn.execute(
            "DELETE FROM error_events WHERE id NOT IN \
             (SELECT id FROM error_events ORDER BY id DESC LIMIT ?)",
            params![cap],
        ).map_err(|e| {
            OpcGwError::Storage(format!("Failed to prune error events: {}", e))
        })?;
        __op.ok();
        Ok(())
    }

    fn recent_error_events(&self, limit: usize) -> Result<Vec<ErrorEvent>, OpcGwError> {
        let mut __op = StorageOpLog::start("recent_error_events");
        let conn = self.pool.checkout(std::time::Duration::from_secs(5)).map_err(|e| {
            OpcGwError::Storage(format!("Failed to get database connection for error events: {}", e))
        })?;

        let mut stmt = conn.prepare(
            "SELECT ts, category, device_id, application_id, message \
             FROM error_events ORDER BY id DESC LIMIT ?",
        ).map_err(|e| {
            OpcGwError::Storage(format!("Failed to prepare error-event query: {}", e))
        })?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(ErrorEvent {
                ts: DateTime::parse_from_rfc3339(&row.get::<_, String>(0)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                category: row.get::<_, String>(1)?,
                device_id: row.get::<_, Option<String>>(2)?,
                application_id: row.get::<_, Option<String>>(3)?,
                message: row.get::<_, String>(4)?,
            })
        }).map_err(|e| {
            OpcGwError::Storage(format!("Failed to query error events: {}", e))
        })?;

        let mut events = Vec::new();
        for r in rows {
            events.push(r.map_err(|e| {
                OpcGwError::Storage(format!("Failed to read error-event row: {}", e))
            })?);
        }
        __op.ok();
        Ok(events)
    }

    fn get_gateway_health_metrics(&self) -> Result<(Option<DateTime<Utc>>, i32, bool), OpcGwError> {
        let mut __op = StorageOpLog::start("get_gateway_health_metrics");
        let conn = self.pool.checkout(std::time::Duration::from_secs(5)).map_err(|e| {
            OpcGwError::Storage(format!("Failed to get database connection for gateway status read: {}", e))
        })?;

        // Query the gateway_status row (id=1)
        let result = conn.query_row(
            "SELECT last_poll_timestamp, error_count, chirpstack_available FROM gateway_status WHERE id = 1",
            [],
            |row| {
                let timestamp_str: Option<String> = row.get(0)?;
                let timestamp = timestamp_str.and_then(|s| DateTime::parse_from_rfc3339(&s).ok().map(|dt| dt.with_timezone(&Utc)));
                let error_count: i32 = row.get(1)?;
                let available: bool = row.get(2)?;
                Ok((timestamp, error_count, available))
            },
        );

        match result {
            Ok((timestamp, error_count, available)) => {
                trace!(
                    timestamp = ?timestamp,
                    error_count = error_count,
                    available = available,
                    "Retrieved gateway health metrics"
                );
                __op.ok();
                Ok((timestamp, error_count, available))
            }
            Err(rusqlite::Error::QueryReturnedNoRows) => {
                // First startup: return sensible defaults
                debug!("Gateway health metrics not found; returning defaults for first startup");
                __op.ok();
                Ok((None, 0, false))
            }
            Err(e) => {
                Err(OpcGwError::Storage(format!(
                    "Failed to retrieve gateway health metrics: {}",
                    e
                )))
            }
        }
    }
}

impl SqliteBackend {
    /// Prune historical metrics older than the specified retention period (Task 2-5a).
    ///
    /// Deletes rows from metric_history table where timestamp is older than
    /// (now - retention_days). Returns count of deleted rows.
    ///
    /// # Arguments
    /// * `retention_days` - Number of days to retain (older data is deleted)
    ///
    /// # Returns
    /// * `Ok(u32)` - Number of rows deleted
    /// * `Err(OpcGwError)` - If database query fails
    pub fn prune_old_metrics(&self, retention_days: u32) -> Result<u32, OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, retention_days = retention_days, "Pool checkout timeout for prune_old_metrics");
                e
            })?;

        let query = format!(
            "DELETE FROM metric_history WHERE timestamp < datetime('now', '-{} days')",
            retention_days
        );

        let deleted_count = conn.execute(&query, [])
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to prune metrics older than {} days: {}",
                    retention_days, e
                ))
            })? as u32;

        debug!(
            retention_days = retention_days,
            deleted_count = deleted_count,
            "Pruned old metrics from history"
        );

        Ok(deleted_count)
    }

    /// Story 8-3 AC#3: write the operator-configured `metric_history`
    /// retention period to the `retention_config` table so the prune loop
    /// (`prune_metric_history`) and the HistoryRead path observe the same
    /// value.
    ///
    /// Uses `INSERT OR REPLACE` keyed on `(data_type)` (UNIQUE constraint
    /// per `migrations/v001_initial.sql:118`). The migration default for the
    /// `metric_history` row is 90 days (`v001_initial.sql:128`); this method
    /// overrides that default with the value from `[storage].retention_days`
    /// at startup, restoring operator intent on every boot. Validation
    /// (`AppConfig::validate`) already enforces the FR22 floor of 7 days
    /// and the hard cap of 365 — this method assumes a pre-validated value
    /// and does not re-check.
    pub fn set_metric_history_retention_days(&self, days: u32) -> Result<(), OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            error!(error = %e, "Pool checkout timeout for set_metric_history_retention_days");
            e
        })?;

        conn.execute(
            "INSERT OR REPLACE INTO retention_config \
             (id, data_type, retention_days, auto_delete, updated_at) \
             VALUES \
             ((SELECT id FROM retention_config WHERE data_type = 'metric_history'), \
              'metric_history', ?1, 1, datetime('now'))",
            params![days as i64],
        )
        .map_err(|e| {
            // Review patch P14: use OpcGwError::Storage to match
            // `query_metric_history` and the spec's "Existing
            // Infrastructure" guidance ("Use `Storage` for SQLite query
            // failures"). The surrounding pre-Story-8-3 methods use
            // `Database` for historical reasons; the two Story 8-3
            // methods are now consistent with each other and with spec.
            OpcGwError::Storage(format!(
                "Failed to set metric_history retention_days={days}: {e}"
            ))
        })?;

        info!(
            retention_days = days,
            "metric_history retention_config row updated from operator config"
        );
        Ok(())
    }
}

// Story C-6: Application configuration CRUD — one impl block per concern.
impl SqliteBackend {
    fn metric_type_cfg_to_str(t: &OpcMetricTypeConfig) -> &'static str {
        match t {
            OpcMetricTypeConfig::Bool => "Bool",
            OpcMetricTypeConfig::Int => "Int",
            OpcMetricTypeConfig::Float => "Float",
            OpcMetricTypeConfig::String => "String",
        }
    }

    fn metric_type_cfg_from_str(
        s: &str,
        app_id: &str,
        dev_id: &str,
        metric_name: &str,
    ) -> Result<OpcMetricTypeConfig, OpcGwError> {
        match s {
            "Bool" => Ok(OpcMetricTypeConfig::Bool),
            "Int" => Ok(OpcMetricTypeConfig::Int),
            "Float" => Ok(OpcMetricTypeConfig::Float),
            "String" => Ok(OpcMetricTypeConfig::String),
            other => Err(OpcGwError::Database(format!(
                "Unknown metric_type '{}' for {}/{}/{}",
                other, app_id, dev_id, metric_name
            ))),
        }
    }

    /// Insert a new application row.
    pub fn insert_application(&self, app: &ChirpStackApplications) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("insert_application");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for insert_application");
            e
        })?;
        let now = format_rfc3339(&Utc::now());
        conn.execute(
            "INSERT INTO applications (application_id, application_name, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4)",
            params![app.application_id, app.application_name, now, now],
        )
        .map_err(|e| {
            OpcGwError::Database(format!("insert_application '{}': {}", app.application_id, e))
        })?;
        debug!(application_id = %app.application_id, "Inserted application");
        __op.ok();
        Ok(())
    }

    /// Update an existing application's name.
    pub fn update_application(&self, app: &ChirpStackApplications) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("update_application");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for update_application");
            e
        })?;
        let now = format_rfc3339(&Utc::now());
        let changed = conn
            .execute(
                "UPDATE applications SET application_name = ?1, updated_at = ?2 \
                 WHERE application_id = ?3",
                params![app.application_name, now, app.application_id],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "update_application '{}': {}",
                    app.application_id, e
                ))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "update_application: no row for '{}'",
                app.application_id
            )));
        }
        debug!(application_id = %app.application_id, "Updated application");
        __op.ok();
        Ok(())
    }

    /// Delete an application and all its children via CASCADE FK.
    pub fn delete_application(&self, application_id: &str) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("delete_application");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for delete_application");
            e
        })?;
        let changed = conn
            .execute(
                "DELETE FROM applications WHERE application_id = ?1",
                params![application_id],
            )
            .map_err(|e| {
                OpcGwError::Database(format!("delete_application '{}': {}", application_id, e))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "delete_application: no row for '{}'",
                application_id
            )));
        }
        debug!(application_id = %application_id, "Deleted application (cascade)");
        __op.ok();
        Ok(())
    }

    /// Insert a new device under an application.
    pub fn insert_device(
        &self,
        application_id: &str,
        device: &ChirpstackDevice,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("insert_device");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for insert_device");
            e
        })?;
        let now = format_rfc3339(&Utc::now());
        conn.execute(
            "INSERT INTO devices \
             (application_id, device_id, device_name, stale_threshold_seconds, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                application_id,
                device.device_id,
                device.device_name,
                // Story E-1 (E-1b, #132): persist the per-device stale threshold.
                device.stale_threshold_seconds.map(|v| v as i64),
                now,
                now
            ],
        )
        .map_err(|e| {
            OpcGwError::Database(format!(
                "insert_device '{}/{}': {}",
                application_id, device.device_id, e
            ))
        })?;
        debug!(application_id = %application_id, device_id = %device.device_id, "Inserted device");
        __op.ok();
        Ok(())
    }

    /// Update an existing device's name.
    pub fn update_device(
        &self,
        application_id: &str,
        device: &ChirpstackDevice,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("update_device");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for update_device");
            e
        })?;
        let now = format_rfc3339(&Utc::now());
        let changed = conn
            .execute(
                "UPDATE devices SET device_name = ?1, updated_at = ?2 \
                 WHERE application_id = ?3 AND device_id = ?4",
                params![device.device_name, now, application_id, device.device_id],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "update_device '{}/{}': {}",
                    application_id, device.device_id, e
                ))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "update_device: no row for '{}/{}'",
                application_id, device.device_id
            )));
        }
        debug!(application_id = %application_id, device_id = %device.device_id, "Updated device");
        __op.ok();
        Ok(())
    }

    /// Delete a device and all its metrics and commands via CASCADE FK.
    pub fn delete_device(&self, application_id: &str, device_id: &str) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("delete_device");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for delete_device");
            e
        })?;
        let changed = conn
            .execute(
                "DELETE FROM devices WHERE application_id = ?1 AND device_id = ?2",
                params![application_id, device_id],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "delete_device '{}/{}': {}",
                    application_id, device_id, e
                ))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "delete_device: no row for '{}/{}'",
                application_id, device_id
            )));
        }
        debug!(application_id = %application_id, device_id = %device_id, "Deleted device (cascade)");
        __op.ok();
        Ok(())
    }

    /// Insert a new metric mapping for a device.
    pub fn insert_metric(
        &self,
        application_id: &str,
        device_id: &str,
        metric: &ReadMetric,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("insert_metric");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for insert_metric");
            e
        })?;
        let now = format_rfc3339(&Utc::now());
        let type_str = Self::metric_type_cfg_to_str(&metric.metric_type);
        conn.execute(
            "INSERT INTO metrics \
             (application_id, device_id, chirpstack_metric_name, metric_name, \
              metric_type, metric_unit, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                application_id,
                device_id,
                metric.chirpstack_metric_name,
                metric.metric_name,
                type_str,
                metric.metric_unit,
                now,
                now
            ],
        )
        .map_err(|e| {
            OpcGwError::Database(format!(
                "insert_metric '{}/{}/{}': {}",
                application_id, device_id, metric.chirpstack_metric_name, e
            ))
        })?;
        debug!(
            application_id = %application_id,
            device_id = %device_id,
            chirpstack_metric = %metric.chirpstack_metric_name,
            "Inserted metric"
        );
        __op.ok();
        Ok(())
    }

    /// Update an existing metric mapping (name, type, unit).
    pub fn update_metric(
        &self,
        application_id: &str,
        device_id: &str,
        metric: &ReadMetric,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("update_metric");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for update_metric");
            e
        })?;
        let now = format_rfc3339(&Utc::now());
        let type_str = Self::metric_type_cfg_to_str(&metric.metric_type);
        let changed = conn
            .execute(
                "UPDATE metrics SET metric_name = ?1, metric_type = ?2, metric_unit = ?3, \
                 updated_at = ?4 \
                 WHERE application_id = ?5 AND device_id = ?6 AND chirpstack_metric_name = ?7",
                params![
                    metric.metric_name,
                    type_str,
                    metric.metric_unit,
                    now,
                    application_id,
                    device_id,
                    metric.chirpstack_metric_name
                ],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "update_metric '{}/{}/{}': {}",
                    application_id, device_id, metric.chirpstack_metric_name, e
                ))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "update_metric: no row for '{}/{}/{}'",
                application_id, device_id, metric.chirpstack_metric_name
            )));
        }
        debug!(
            application_id = %application_id,
            device_id = %device_id,
            chirpstack_metric = %metric.chirpstack_metric_name,
            "Updated metric"
        );
        __op.ok();
        Ok(())
    }

    /// Delete a metric mapping.
    pub fn delete_metric(
        &self,
        application_id: &str,
        device_id: &str,
        chirpstack_metric_name: &str,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("delete_metric");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for delete_metric");
            e
        })?;
        let changed = conn
            .execute(
                "DELETE FROM metrics \
                 WHERE application_id = ?1 AND device_id = ?2 AND chirpstack_metric_name = ?3",
                params![application_id, device_id, chirpstack_metric_name],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "delete_metric '{}/{}/{}': {}",
                    application_id, device_id, chirpstack_metric_name, e
                ))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "delete_metric: no row for '{}/{}/{}'",
                application_id, device_id, chirpstack_metric_name
            )));
        }
        debug!(
            application_id = %application_id,
            device_id = %device_id,
            chirpstack_metric = %chirpstack_metric_name,
            "Deleted metric"
        );
        __op.ok();
        Ok(())
    }

    /// Insert a new command for a device.
    pub fn insert_command(
        &self,
        application_id: &str,
        device_id: &str,
        cmd: &DeviceCommandCfg,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("insert_command");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for insert_command");
            e
        })?;
        conn.execute(
            "INSERT INTO commands \
             (application_id, device_id, command_name, command_id, \
              command_confirmed, command_port, command_class) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                application_id,
                device_id,
                cmd.command_name,
                cmd.command_id,
                cmd.command_confirmed as i32,
                cmd.command_port,
                cmd.command_class
            ],
        )
        .map_err(|e| {
            OpcGwError::Database(format!(
                "insert_command '{}/{}/{}': {}",
                application_id, device_id, cmd.command_name, e
            ))
        })?;
        debug!(
            application_id = %application_id,
            device_id = %device_id,
            command_name = %cmd.command_name,
            "Inserted command"
        );
        __op.ok();
        Ok(())
    }

    /// Update an existing command's fields.
    pub fn update_command(
        &self,
        application_id: &str,
        device_id: &str,
        cmd: &DeviceCommandCfg,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("update_command");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for update_command");
            e
        })?;
        let changed = conn
            .execute(
                "UPDATE commands SET command_id = ?1, command_confirmed = ?2, command_port = ?3, \
                 command_class = ?4 \
                 WHERE application_id = ?5 AND device_id = ?6 AND command_name = ?7",
                params![
                    cmd.command_id,
                    cmd.command_confirmed as i32,
                    cmd.command_port,
                    cmd.command_class,
                    application_id,
                    device_id,
                    cmd.command_name
                ],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "update_command '{}/{}/{}': {}",
                    application_id, device_id, cmd.command_name, e
                ))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "update_command: no row for '{}/{}/{}'",
                application_id, device_id, cmd.command_name
            )));
        }
        debug!(
            application_id = %application_id,
            device_id = %device_id,
            command_name = %cmd.command_name,
            "Updated command"
        );
        __op.ok();
        Ok(())
    }

    /// Update a command identified by its integer `command_id` (HTTP API key).
    ///
    /// Updates `command_name`, `command_confirmed`, `command_port`, and the
    /// optional `command_class` device-class binding (Story E-2).
    /// The `command_name` UNIQUE constraint serves as the concurrent-write
    /// guard for rename collisions.
    // Thin keyed-by-command_id SQL wrapper for the web PUT path: the mutable
    // command fields are passed positionally (Story E-2 added command_class).
    #[allow(clippy::too_many_arguments)]
    pub fn update_command_by_id(
        &self,
        application_id: &str,
        device_id: &str,
        command_id: i32,
        new_name: &str,
        new_port: i32,
        new_confirmed: bool,
        new_class: Option<&str>,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("update_command_by_id");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for update_command_by_id");
            e
        })?;
        let changed = conn
            .execute(
                "UPDATE commands SET command_name = ?1, command_confirmed = ?2, command_port = ?3, \
                 command_class = ?4 \
                 WHERE application_id = ?5 AND device_id = ?6 AND command_id = ?7",
                params![new_name, new_confirmed as i32, new_port, new_class, application_id, device_id, command_id],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "update_command_by_id '{}/{}/{}': {}",
                    application_id, device_id, command_id, e
                ))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "update_command_by_id: no row for '{}/{}/{}'",
                application_id, device_id, command_id
            )));
        }
        debug!(
            application_id = %application_id,
            device_id = %device_id,
            command_id = command_id,
            new_name = %new_name,
            "Updated command by id"
        );
        __op.ok();
        Ok(())
    }

    /// Delete a command.
    pub fn delete_command(
        &self,
        application_id: &str,
        device_id: &str,
        command_name: &str,
    ) -> Result<(), OpcGwError> {
        let mut __op = StorageOpLog::start("delete_command");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for delete_command");
            e
        })?;
        let changed = conn
            .execute(
                "DELETE FROM commands \
                 WHERE application_id = ?1 AND device_id = ?2 AND command_name = ?3",
                params![application_id, device_id, command_name],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "delete_command '{}/{}/{}': {}",
                    application_id, device_id, command_name, e
                ))
            })?;
        if changed == 0 {
            return Err(OpcGwError::Database(format!(
                "delete_command: no row for '{}/{}/{}'",
                application_id, device_id, command_name
            )));
        }
        debug!(
            application_id = %application_id,
            device_id = %device_id,
            command_name = %command_name,
            "Deleted command"
        );
        __op.ok();
        Ok(())
    }

    /// Load all application configuration from the four config tables.
    ///
    /// Reads `applications`, `devices`, `metrics`, and `commands` in four
    /// full-table scans (config is small; no per-row sub-queries needed) and
    /// assembles the nested `Vec<ChirpStackApplications>` used by
    /// `AppConfig.application_list`.
    pub fn load_all_applications_config(
        &self,
    ) -> Result<Vec<ChirpStackApplications>, OpcGwError> {
        let mut __op = StorageOpLog::start("load_all_applications_config");
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for load_all_applications_config");
            e
        })?;

        // ── applications ────────────────────────────────────────────────────
        let mut stmt = conn
            .prepare(
                "SELECT application_id, application_name \
                 FROM applications ORDER BY application_id",
            )
            .map_err(|e| OpcGwError::Database(format!("prepare applications: {}", e)))?;
        let app_rows: Vec<(String, String)> = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| OpcGwError::Database(format!("query applications: {}", e)))?
            .collect::<Result<_, _>>()
            .map_err(|e| OpcGwError::Database(format!("collect applications: {}", e)))?;

        // ── devices ─────────────────────────────────────────────────────────
        let mut stmt = conn
            .prepare(
                "SELECT application_id, device_id, device_name, stale_threshold_seconds \
                 FROM devices ORDER BY application_id, device_id",
            )
            .map_err(|e| OpcGwError::Database(format!("prepare devices: {}", e)))?;
        let dev_rows: Vec<(String, String, String, Option<u64>)> = stmt
            .query_map([], |row| {
                let application_id = row.get::<_, String>(0)?;
                let device_id = row.get::<_, String>(1)?;
                let device_name = row.get::<_, String>(2)?;
                // Story E-1 (E-1b, #132): per-device stale threshold (NULL = use global).
                // A negative value (hand-edited DB) must not wrap to a huge
                // u64 — treat it as unset, audibly (config-side validation
                // rejects the same input hard; the DB load can only warn).
                let stale_threshold = row.get::<_, Option<i64>>(3)?.and_then(|v| {
                    if v >= 0 {
                        Some(v as u64)
                    } else {
                        tracing::warn!(
                            event = "storage_invalid_stale_threshold",
                            device_id = %device_id,
                            value = v,
                            "negative per-device stale_threshold_seconds in DB; using global threshold"
                        );
                        None
                    }
                });
                Ok((application_id, device_id, device_name, stale_threshold))
            })
            .map_err(|e| OpcGwError::Database(format!("query devices: {}", e)))?
            .collect::<Result<_, _>>()
            .map_err(|e| OpcGwError::Database(format!("collect devices: {}", e)))?;

        // ── metrics ─────────────────────────────────────────────────────────
        let mut stmt = conn
            .prepare(
                "SELECT application_id, device_id, chirpstack_metric_name, \
                        metric_name, metric_type, metric_unit \
                 FROM metrics ORDER BY application_id, device_id, chirpstack_metric_name",
            )
            .map_err(|e| OpcGwError::Database(format!("prepare metrics: {}", e)))?;
        let met_rows: Vec<(String, String, String, String, String, Option<String>)> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                ))
            })
            .map_err(|e| OpcGwError::Database(format!("query metrics: {}", e)))?
            .collect::<Result<_, _>>()
            .map_err(|e| OpcGwError::Database(format!("collect metrics: {}", e)))?;

        // ── commands ─────────────────────────────────────────────────────────
        let mut stmt = conn
            .prepare(
                "SELECT application_id, device_id, command_name, \
                        command_id, command_confirmed, command_port, command_class \
                 FROM commands ORDER BY application_id, device_id, command_name",
            )
            .map_err(|e| OpcGwError::Database(format!("prepare commands: {}", e)))?;
        // Row shape for the application-config `commands` table:
        // (application_id, device_id, command_name, command_id,
        //  command_confirmed, command_port, command_class).
        type CommandRow = (String, String, String, i32, i32, Option<i32>, Option<String>);
        let cmd_rows: Vec<CommandRow> = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i32>(3)?,
                    row.get::<_, i32>(4)?,
                    row.get::<_, Option<i32>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            })
            .map_err(|e| OpcGwError::Database(format!("query commands: {}", e)))?
            .collect::<Result<_, _>>()
            .map_err(|e| OpcGwError::Database(format!("collect commands: {}", e)))?;

        // ── assemble hierarchy ───────────────────────────────────────────────
        let mut result: Vec<ChirpStackApplications> = Vec::with_capacity(app_rows.len());

        for (app_id, app_name) in app_rows {
            let mut devices: Vec<ChirpstackDevice> = Vec::new();

            for (dev_app_id, dev_id, dev_name, dev_stale) in &dev_rows {
                if dev_app_id != &app_id {
                    continue;
                }

                let metrics: Vec<ReadMetric> = met_rows
                    .iter()
                    .filter(|(m_app, m_dev, ..)| m_app == &app_id && m_dev == dev_id)
                    .map(|(_, _, cs_name, m_name, m_type, m_unit)| {
                        let metric_type =
                            Self::metric_type_cfg_from_str(m_type, &app_id, dev_id, cs_name)?;
                        Ok(ReadMetric {
                            metric_name: m_name.clone(),
                            chirpstack_metric_name: cs_name.clone(),
                            metric_type,
                            metric_unit: m_unit.clone(),
                        })
                    })
                    .collect::<Result<_, OpcGwError>>()?;

                let commands: Vec<DeviceCommandCfg> = cmd_rows
                    .iter()
                    .filter(|(c_app, c_dev, ..)| c_app == &app_id && c_dev == dev_id)
                    .map(|(_, _, c_name, c_id, c_confirmed, c_port, c_class)| DeviceCommandCfg {
                        command_name: c_name.clone(),
                        command_id: *c_id,
                        command_confirmed: *c_confirmed != 0,
                        command_port: c_port.unwrap_or(0),
                        command_class: c_class.clone(),
                    })
                    .collect();

                devices.push(ChirpstackDevice {
                    device_id: dev_id.clone(),
                    device_name: dev_name.clone(),
                    stale_threshold_seconds: *dev_stale,
                    read_metric_list: metrics,
                    device_command_list: if commands.is_empty() {
                        None
                    } else {
                        Some(commands)
                    },
                });
            }

            result.push(ChirpStackApplications {
                application_id: app_id,
                application_name: app_name,
                device_list: devices,
            });
        }

        debug!(
            application_count = result.len(),
            "Loaded all applications config from SQLite"
        );
        __op.ok();
        Ok(result)
    }

    /// Count rows in the `applications` table — used by the migration guard.
    /// Returns `true` if the C-6 migration done-flag has been written to the
    /// `meta` table. Used as the primary idempotency guard in
    /// `migrate_toml_to_sqlite` so deliberate operator deletions of all
    /// applications via the web UI cannot re-trigger migration on restart.
    pub fn is_c6_migration_done(&self) -> Result<bool, OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            warn!(error = %e, "Pool checkout timeout for is_c6_migration_done");
            e
        })?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key='c6_migration_done'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| OpcGwError::Database(format!("is_c6_migration_done: {}", e)))?;
        Ok(count > 0)
    }

    /// Write the C-6 migration done-flag to the `meta` table so the primary
    /// idempotency guard in `migrate_toml_to_sqlite` fires on subsequent boots.
    /// Called by the secondary already-migrated guard to back-fill the flag for
    /// databases populated via a direct SQLite import that bypassed the normal
    /// migration path.
    pub fn write_c6_migration_done(&self) -> Result<(), OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            warn!(error = %e, "Pool checkout timeout for write_c6_migration_done");
            e
        })?;
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) VALUES ('c6_migration_done', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            [],
        )
        .map_err(|e| OpcGwError::Database(format!("write_c6_migration_done: {}", e)))?;
        Ok(())
    }

    pub fn count_applications(&self) -> Result<usize, OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for count_applications");
            e
        })?;
        let count = conn
            .query_row("SELECT COUNT(*) FROM applications", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|e| OpcGwError::Database(format!("count_applications: {}", e)))? as usize;
        Ok(count)
    }

    // ===== Story D-0: singleton-config helpers =====

    /// Returns `true` once the `d0_migration_done` meta key has been written
    /// (either by the main `migrate_singleton_toml_to_sqlite` path or by
    /// the secondary-guard back-fill on direct-SQLite-import databases).
    /// Mirrors the C-6 [`Self::is_c6_migration_done`] pattern.
    pub fn is_d0_migration_done(&self) -> Result<bool, OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            warn!(error = %e, "Pool checkout timeout for is_d0_migration_done");
            e
        })?;
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM meta WHERE key = 'd0_migration_done'",
                [],
                |row| row.get(0),
            )
            .map_err(|e| OpcGwError::Database(format!("is_d0_migration_done: {}", e)))?;
        Ok(count > 0)
    }

    /// Write the D-0 migration done-flag to the `meta` table. INSERT OR
    /// IGNORE so re-invocations preserve the original timestamp (the
    /// back-fill path runs idempotently on every subsequent boot until
    /// the row exists). Mirrors C-6 [`Self::write_c6_migration_done`].
    pub fn write_d0_migration_done(&self) -> Result<(), OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            warn!(error = %e, "Pool checkout timeout for write_d0_migration_done");
            e
        })?;
        conn.execute(
            "INSERT OR IGNORE INTO meta (key, value) \
             VALUES ('d0_migration_done', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            [],
        )
        .map_err(|e| OpcGwError::Database(format!("write_d0_migration_done: {}", e)))?;
        Ok(())
    }

    /// Count rows in `singleton_config`. Used by the secondary already-
    /// migrated guard in `migrate_singleton_toml_to_sqlite` (apps-present-
    /// without-done-flag scenario for direct SQLite imports).
    pub fn count_singleton_config(&self) -> Result<usize, OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            trace!(error = %e, "Pool checkout timeout for count_singleton_config");
            e
        })?;
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM singleton_config", [], |row| row.get(0))
            .map_err(|e| OpcGwError::Database(format!("count_singleton_config: {}", e)))?;
        Ok(count as usize)
    }

    /// Read all rows from `singleton_config` as `(section, key, value)` triples.
    /// The Rust-side caller (`AppConfig::load_singleton_from_sqlite`) routes
    /// each triple into the matching typed struct field via section + key
    /// dispatch. Boot-time call; not on the hot path.
    pub fn load_singleton_config(
        &self,
    ) -> Result<Vec<(String, String, String)>, OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            warn!(error = %e, "Pool checkout timeout for load_singleton_config");
            e
        })?;
        let mut stmt = conn
            .prepare_cached(
                "SELECT section, key, value FROM singleton_config ORDER BY section, key",
            )
            .map_err(|e| OpcGwError::Database(format!("load_singleton_config prepare: {}", e)))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?)))
            .map_err(|e| OpcGwError::Database(format!("load_singleton_config query: {}", e)))?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(|e| {
                OpcGwError::Database(format!("load_singleton_config row: {}", e))
            })?);
        }
        Ok(out)
    }

    /// Atomically replace all rows for a given section in `singleton_config`.
    /// The caller supplies (key, value) pairs; existing rows for the section
    /// are deleted first, then the new rows are inserted. Used by D-1's
    /// `PUT /api/config/singleton/<section>` endpoint (D-0 ships the helper;
    /// D-1 wires it to HTTP).
    ///
    /// Wrapped in `BEGIN IMMEDIATE TRANSACTION` so a partial write cannot
    /// leave the section half-empty. The `section` argument is validated by
    /// the schema-level CHECK constraint — invalid sections fall out of the
    /// INSERT with a CHECK violation rolled back by the IMMEDIATE txn.
    pub fn write_singleton_section(
        &self,
        section: &str,
        fields: &[(String, String)],
    ) -> Result<(), OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            warn!(error = %e, "Pool checkout timeout for write_singleton_section");
            e
        })?;
        conn.execute_batch("BEGIN IMMEDIATE TRANSACTION").map_err(|e| {
            OpcGwError::Database(format!("write_singleton_section: begin: {}", e))
        })?;
        let result: Result<(), OpcGwError> = (|| {
            conn.execute(
                "DELETE FROM singleton_config WHERE section = ?1",
                rusqlite::params![section],
            )
            .map_err(|e| OpcGwError::Database(format!("write_singleton_section: delete: {}", e)))?;

            let now = format_rfc3339(&Utc::now());
            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO singleton_config (section, key, value, updated_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| {
                    OpcGwError::Database(format!("write_singleton_section: prepare: {}", e))
                })?;
            for (k, v) in fields {
                stmt.execute(rusqlite::params![section, k, v, now])
                    .map_err(|e| {
                        OpcGwError::Database(format!(
                            "write_singleton_section: insert section={} key={}: {}",
                            section, k, e
                        ))
                    })?;
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT").map_err(|e| {
                    OpcGwError::Database(format!("write_singleton_section: commit: {}", e))
                })?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// D-0 iter-1 I1-F1: atomic boot-time migration of ALL singleton sections
    /// in one EXCLUSIVE transaction. Closes the AC#4 ROLLBACK contract that
    /// the per-section `write_singleton_section` approach left open.
    ///
    /// Behaviour: open a single connection, BEGIN EXCLUSIVE TRANSACTION,
    /// DELETE all rows for the four sections (defensive — covers the case
    /// where a partial prior run left some rows), INSERT all supplied
    /// rows, verify the total row count matches the input, write the
    /// `d0_migration_done` meta-key inside the same transaction so data
    /// and flag are atomic, then COMMIT. On any failure: ROLLBACK so the
    /// table reverts to its pre-call state (either empty for a fresh
    /// migration, or whatever was there before for a re-attempt).
    ///
    /// Used by `migrate_singleton_toml_to_sqlite`. D-1's per-section PUT
    /// continues to use the lighter `write_singleton_section` (one
    /// section per IMMEDIATE txn) since D-1 writes to one section at a
    /// time and does NOT touch the meta key.
    pub fn migrate_singleton_sections_atomic(
        &self,
        sections: &[(&str, &[(String, String)])],
    ) -> Result<usize, OpcGwError> {
        // I2-F1 (iter-2): empty-slice precondition guard. An empty
        // `sections` slice would silently succeed (no INSERTs, count
        // matches 0 if the table was empty), write the done-flag, and
        // permanently stamp the DB as migrated with zero rows.
        if sections.is_empty() {
            return Err(OpcGwError::Database(
                "migrate_singleton_sections_atomic called with empty sections slice; \
                 refusing to write done-flag for a no-op migration"
                    .into(),
            ));
        }

        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            warn!(error = %e, "Pool checkout timeout for migrate_singleton_sections_atomic");
            e
        })?;
        conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION").map_err(|e| {
            OpcGwError::Database(format!(
                "migrate_singleton_sections_atomic: begin: {}",
                e
            ))
        })?;

        let result: Result<usize, OpcGwError> = (|| {
            let now = format_rfc3339(&Utc::now());

            // Defensive DELETE for each named section so partial prior
            // state cannot survive into this commit. The four sections
            // covered by D-0 are explicitly listed; the schema-level
            // CHECK constraint pins this list.
            for (section, _) in sections {
                conn.execute(
                    "DELETE FROM singleton_config WHERE section = ?1",
                    rusqlite::params![section],
                )
                .map_err(|e| {
                    OpcGwError::Database(format!(
                        "migrate_singleton_sections_atomic: delete section={}: {}",
                        section, e
                    ))
                })?;
            }

            let mut stmt = conn
                .prepare_cached(
                    "INSERT INTO singleton_config (section, key, value, updated_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|e| {
                    OpcGwError::Database(format!(
                        "migrate_singleton_sections_atomic: prepare: {}",
                        e
                    ))
                })?;

            let mut total_inserted = 0usize;
            for (section, fields) in sections {
                for (k, v) in *fields {
                    stmt.execute(rusqlite::params![section, k, v, now])
                        .map_err(|e| {
                            OpcGwError::Database(format!(
                                "migrate_singleton_sections_atomic: insert section={} key={:?}: {}",
                                section, k, e
                            ))
                        })?;
                    total_inserted += 1;
                }
            }
            drop(stmt);

            // I2-F1 (iter-2): per-section row-count verification scoped
            // to the sections we operated on. Previous code used an
            // unbounded `SELECT COUNT(*) FROM singleton_config` which
            // would conflate orphaned rows (e.g. a future Section #5
            // direct-SQLite-hack) with the migration's own writes. The
            // per-section count is bounded to the input sections and
            // surfaces the offending section name in the error message.
            let mut count_stmt = conn
                .prepare_cached(
                    "SELECT COUNT(*) FROM singleton_config WHERE section = ?1",
                )
                .map_err(|e| {
                    OpcGwError::Database(format!(
                        "migrate_singleton_sections_atomic: count prepare: {}",
                        e
                    ))
                })?;
            for (section, fields) in sections {
                let actual: i64 = count_stmt
                    .query_row(rusqlite::params![section], |row| row.get(0))
                    .map_err(|e| {
                        OpcGwError::Database(format!(
                            "migrate_singleton_sections_atomic: count section={}: {}",
                            section, e
                        ))
                    })?;
                if actual as usize != fields.len() {
                    // I3-F4 (iter-3): quote the section name so downstream
                    // string parsers can unambiguously extract the section
                    // even in the (theoretical) case where a section name
                    // contains whitespace. Section names are a fixed enum
                    // today (global / chirpstack / opcua / web) so this is
                    // defensive against future schema extensions.
                    return Err(OpcGwError::Database(format!(
                        "singleton_row_count_mismatch: expected={} actual={} section={:?}",
                        fields.len(),
                        actual,
                        section
                    )));
                }
            }
            drop(count_stmt);

            // Write the done-flag inside the same EXCLUSIVE transaction so
            // (data present) ↔ (flag set) is an atomic invariant. INSERT
            // OR IGNORE preserves any pre-existing timestamp (this path
            // is reachable only on the FIRST migration; secondary-guard
            // back-fill uses a separate non-transactional helper).
            conn.execute(
                "INSERT OR IGNORE INTO meta (key, value) \
                 VALUES ('d0_migration_done', strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
                [],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "migrate_singleton_sections_atomic: done-flag write: {}",
                    e
                ))
            })?;

            Ok(total_inserted)
        })();

        match result {
            Ok(n) => {
                // I2-F3 (iter-2): if COMMIT itself fails (disk full,
                // I/O error mid-commit), the pool connection has a
                // pending uncommitted EXCLUSIVE transaction. Without
                // explicit ROLLBACK, the next BEGIN on this connection
                // fails with "cannot start a transaction within a
                // transaction". Pre-existing precedent at sqlite.rs:1387.
                conn.execute_batch("COMMIT").map_err(|e| {
                    let _ = conn.execute_batch("ROLLBACK");
                    OpcGwError::Database(format!(
                        "migrate_singleton_sections_atomic: commit: {}",
                        e
                    ))
                })?;
                Ok(n)
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Insert a device and all its metrics in one EXCLUSIVE transaction.
    /// Used by `create_device` so the device row and metric rows are
    /// committed atomically or not at all.
    pub fn insert_device_with_metrics(
        &self,
        application_id: &str,
        device_id: &str,
        device_name: &str,
        metrics: &[ReadMetric],
        // Story G-3 (#132): optional per-device stale threshold (seconds).
        // None → NULL column → device uses the global [opcua] default.
        // Persisted in the v012 `devices.stale_threshold_seconds` column.
        stale_threshold_seconds: Option<u64>,
    ) -> Result<(), OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            error!(error = %e, "Pool checkout timeout for insert_device_with_metrics");
            e
        })?;
        conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION").map_err(|e| {
            OpcGwError::Database(format!("insert_device_with_metrics: begin: {}", e))
        })?;
        let result: Result<(), OpcGwError> = (|| {
            let now = format_rfc3339(&Utc::now());
            conn.execute(
                "INSERT INTO devices \
                 (application_id, device_id, device_name, stale_threshold_seconds, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    application_id,
                    device_id,
                    device_name,
                    stale_threshold_seconds.map(|v| v as i64),
                    now,
                    now
                ],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "insert_device_with_metrics: insert device '{}/{}': {}",
                    application_id, device_id, e
                ))
            })?;
            for metric in metrics {
                let type_str = Self::metric_type_cfg_to_str(&metric.metric_type);
                conn.execute(
                    "INSERT INTO metrics \
                     (application_id, device_id, chirpstack_metric_name, metric_name, \
                      metric_type, metric_unit, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        application_id,
                        device_id,
                        metric.chirpstack_metric_name,
                        metric.metric_name,
                        type_str,
                        metric.metric_unit,
                        now,
                        now
                    ],
                )
                .map_err(|e| {
                    OpcGwError::Database(format!(
                        "insert_device_with_metrics: insert metric '{}/{}/{}': {}",
                        application_id, device_id, metric.chirpstack_metric_name, e
                    ))
                })?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT").map_err(|e| {
                    OpcGwError::Database(format!("insert_device_with_metrics: commit: {}", e))
                })?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Update a device's name and atomically replace all its metrics in
    /// one EXCLUSIVE transaction.  Commands are left untouched.
    /// Used by `update_device`.
    pub fn update_device_name_and_metrics(
        &self,
        application_id: &str,
        device_id: &str,
        new_name: &str,
        new_metrics: &[ReadMetric],
        // Story G-3 (#132): per-device stale threshold (seconds); None → NULL
        // (use the global [opcua] default). Written in the same EXCLUSIVE txn.
        stale_threshold_seconds: Option<u64>,
    ) -> Result<(), OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(5)).map_err(|e| {
            error!(error = %e, "Pool checkout timeout for update_device_name_and_metrics");
            e
        })?;
        conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION").map_err(|e| {
            OpcGwError::Database(format!("update_device_name_and_metrics: begin: {}", e))
        })?;
        let result: Result<(), OpcGwError> = (|| {
            let now = format_rfc3339(&Utc::now());
            conn.execute(
                "UPDATE devices SET device_name = ?1, stale_threshold_seconds = ?2, updated_at = ?3 \
                 WHERE application_id = ?4 AND device_id = ?5",
                params![
                    new_name,
                    stale_threshold_seconds.map(|v| v as i64),
                    now,
                    application_id,
                    device_id
                ],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "update_device_name_and_metrics: update device '{}/{}': {}",
                    application_id, device_id, e
                ))
            })?;
            conn.execute(
                "DELETE FROM metrics WHERE application_id = ?1 AND device_id = ?2",
                params![application_id, device_id],
            )
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "update_device_name_and_metrics: delete metrics '{}/{}': {}",
                    application_id, device_id, e
                ))
            })?;
            for metric in new_metrics {
                let type_str = Self::metric_type_cfg_to_str(&metric.metric_type);
                conn.execute(
                    "INSERT INTO metrics \
                     (application_id, device_id, chirpstack_metric_name, metric_name, \
                      metric_type, metric_unit, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        application_id,
                        device_id,
                        metric.chirpstack_metric_name,
                        metric.metric_name,
                        type_str,
                        metric.metric_unit,
                        now,
                        now
                    ],
                )
                .map_err(|e| {
                    OpcGwError::Database(format!(
                        "update_device_name_and_metrics: insert metric '{}/{}/{}': {}",
                        application_id, device_id, metric.chirpstack_metric_name, e
                    ))
                })?;
            }
            Ok(())
        })();
        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT").map_err(|e| {
                    OpcGwError::Database(format!("update_device_name_and_metrics: commit: {}", e))
                })?;
                Ok(())
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }

    /// Bulk-insert the full application/device/metric/command tree in a single
    /// EXCLUSIVE transaction.  Used by the one-shot TOML→SQLite migration.
    ///
    /// Returns `(applications, devices, metrics, commands)` counts on success.
    /// Rolls back automatically on any error.
    pub fn migrate_applications_config(
        &self,
        apps: &[ChirpStackApplications],
    ) -> Result<(usize, usize, usize, usize), OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(30)).map_err(|e| {
            error!(error = %e, "Pool checkout timeout for migrate_applications_config");
            e
        })?;

        conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION").map_err(|e| {
            OpcGwError::Database(format!("migrate: begin transaction: {}", e))
        })?;

        let mut app_count = 0usize;
        let mut dev_count = 0usize;
        let mut met_count = 0usize;
        let mut cmd_count = 0usize;

        let result: Result<(), OpcGwError> = (|| {
            let now = format_rfc3339(&Utc::now());

            for app in apps {
                conn.execute(
                    "INSERT INTO applications \
                     (application_id, application_name, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![app.application_id, app.application_name, now, now],
                )
                .map_err(|e| {
                    OpcGwError::Database(format!(
                        "migrate insert_application '{}': {}",
                        app.application_id, e
                    ))
                })?;
                app_count += 1;

                for device in &app.device_list {
                    conn.execute(
                        "INSERT INTO devices \
                         (application_id, device_id, device_name, stale_threshold_seconds, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            app.application_id,
                            device.device_id,
                            device.device_name,
                            // Story E-1 (E-1b, #132): persist a TOML-seeded per-device threshold.
                            device.stale_threshold_seconds.map(|v| v as i64),
                            now,
                            now
                        ],
                    )
                    .map_err(|e| {
                        OpcGwError::Database(format!(
                            "migrate insert_device '{}/{}': {}",
                            app.application_id, device.device_id, e
                        ))
                    })?;
                    dev_count += 1;

                    for metric in &device.read_metric_list {
                        let type_str = Self::metric_type_cfg_to_str(&metric.metric_type);
                        conn.execute(
                            "INSERT INTO metrics \
                             (application_id, device_id, chirpstack_metric_name, metric_name, \
                              metric_type, metric_unit, created_at, updated_at) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                            params![
                                app.application_id,
                                device.device_id,
                                metric.chirpstack_metric_name,
                                metric.metric_name,
                                type_str,
                                metric.metric_unit,
                                now,
                                now
                            ],
                        )
                        .map_err(|e| {
                            OpcGwError::Database(format!(
                                "migrate insert_metric '{}/{}/{}': {}",
                                app.application_id,
                                device.device_id,
                                metric.chirpstack_metric_name,
                                e
                            ))
                        })?;
                        met_count += 1;
                    }

                    if let Some(commands) = &device.device_command_list {
                        for cmd in commands {
                            conn.execute(
                                "INSERT INTO commands \
                                 (application_id, device_id, command_name, command_id, \
                                  command_confirmed, command_port, command_class) \
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                                params![
                                    app.application_id,
                                    device.device_id,
                                    cmd.command_name,
                                    cmd.command_id,
                                    cmd.command_confirmed as i32,
                                    cmd.command_port,
                                    // Code review iter-1 (F-4): same command_class
                                    // omission existed in the boot migration —
                                    // a config.toml seed with a valve command
                                    // lost its device-class binding. Fixed here too.
                                    cmd.command_class
                                ],
                            )
                            .map_err(|e| {
                                OpcGwError::Database(format!(
                                    "migrate insert_command '{}/{}/{}': {}",
                                    app.application_id, device.device_id, cmd.command_name, e
                                ))
                            })?;
                            cmd_count += 1;
                        }
                    }
                }
            }

            // Row-count verification — all four tables
            let db_apps = conn
                .query_row("SELECT COUNT(*) FROM applications", [], |r| {
                    r.get::<_, i64>(0)
                })
                .map_err(|e| OpcGwError::Database(format!("migrate count check apps: {}", e)))? as usize;
            let db_devs = conn
                .query_row("SELECT COUNT(*) FROM devices", [], |r| r.get::<_, i64>(0))
                .map_err(|e| OpcGwError::Database(format!("migrate count check devs: {}", e)))? as usize;
            let db_mets = conn
                .query_row("SELECT COUNT(*) FROM metrics", [], |r| r.get::<_, i64>(0))
                .map_err(|e| OpcGwError::Database(format!("migrate count check metrics: {}", e)))? as usize;
            let db_cmds = conn
                .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
                .map_err(|e| OpcGwError::Database(format!("migrate count check commands: {}", e)))? as usize;

            if db_apps != app_count || db_devs != dev_count
                || db_mets != met_count || db_cmds != cmd_count
            {
                return Err(OpcGwError::Database(format!(
                    "row_count_mismatch: apps expected={} actual={}, \
                     devices expected={} actual={}, \
                     metrics expected={} actual={}, \
                     commands expected={} actual={}",
                    app_count, db_apps, dev_count, db_devs,
                    met_count, db_mets, cmd_count, db_cmds,
                )));
            }

            // Write migration done-flag (F2): primary idempotency guard.
            // Inside the same EXCLUSIVE TRANSACTION so it either commits
            // with the data or rolls back atomically.
            conn.execute(
                "INSERT OR REPLACE INTO meta (key, value) VALUES ('c6_migration_done', ?1)",
                params![now],
            )
            .map_err(|e| OpcGwError::Database(format!("migrate write meta done-flag: {}", e)))?;

            Ok(())
        })();

        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT").map_err(|e| {
                    OpcGwError::Database(format!("migrate: commit: {}", e))
                })?;
                info!(
                    applications = app_count,
                    devices = dev_count,
                    metrics = met_count,
                    commands = cmd_count,
                    "migrate_applications_config committed"
                );
                Ok((app_count, dev_count, met_count, cmd_count))
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                error!(error = %e, "migrate_applications_config rolled back");
                Err(e)
            }
        }
    }

    /// Story F-4: atomically REPLACE the entire application tree with `apps`
    /// (the config-import path).
    ///
    /// Replaces BOTH the singleton sections AND the entire application tree in
    /// ONE EXCLUSIVE transaction so a config import is all-or-nothing: a failure
    /// anywhere rolls the whole import back, leaving the prior config intact (no
    /// half-staged state). `singletons` is `(section, [(key, value-json)])` for
    /// each section to replace (secrets already excluded by the caller).
    ///
    /// Unlike [`Self::migrate_applications_config`] — which assumes an empty
    /// tree and writes the `c6_migration_done` migration flag — this deletes
    /// every existing application/device/metric/command first, then inserts the
    /// supplied tree, and does NOT touch the migration meta flag (the table is
    /// already migrated on a running instance). Returns
    /// `(apps, devices, metrics, commands)` inserted.
    pub fn import_replace_all(
        &self,
        singletons: &[(&str, Vec<(String, String)>)],
        apps: &[ChirpStackApplications],
    ) -> Result<(usize, usize, usize, usize), OpcGwError> {
        let conn = self.pool.checkout(Duration::from_secs(30)).map_err(|e| {
            error!(error = %e, "Pool checkout timeout for import_replace_all");
            e
        })?;

        conn.execute_batch("BEGIN EXCLUSIVE TRANSACTION").map_err(|e| {
            OpcGwError::Database(format!("import: begin transaction: {}", e))
        })?;

        let mut app_count = 0usize;
        let mut dev_count = 0usize;
        let mut met_count = 0usize;
        let mut cmd_count = 0usize;

        let result: Result<(), OpcGwError> = (|| {
            // (1) Replace the singleton sections (all in this same transaction,
            // so the F-2 Guard-2 trap can't apply and a later failure reverts
            // everything together).
            let now_singleton = format_rfc3339(&Utc::now());
            for (section, fields) in singletons {
                conn.execute(
                    "DELETE FROM singleton_config WHERE section = ?1",
                    params![section],
                )
                .map_err(|e| {
                    OpcGwError::Database(format!("import delete singleton {section}: {}", e))
                })?;
                for (k, v) in fields {
                    conn.execute(
                        "INSERT INTO singleton_config (section, key, value, updated_at) \
                         VALUES (?1, ?2, ?3, ?4)",
                        params![section, k, v, now_singleton],
                    )
                    .map_err(|e| {
                        OpcGwError::Database(format!(
                            "import insert singleton {section}.{k}: {}",
                            e
                        ))
                    })?;
                }
            }

            // (2) Clear the existing app tree child-first (explicit deletes so
            // the method is correct regardless of FK CASCADE configuration).
            for table in ["commands", "metrics", "devices", "applications"] {
                conn.execute(&format!("DELETE FROM {table}"), [])
                    .map_err(|e| OpcGwError::Database(format!("import delete {table}: {}", e)))?;
            }

            let now = format_rfc3339(&Utc::now());
            for app in apps {
                conn.execute(
                    "INSERT INTO applications \
                     (application_id, application_name, created_at, updated_at) \
                     VALUES (?1, ?2, ?3, ?4)",
                    params![app.application_id, app.application_name, now, now],
                )
                .map_err(|e| {
                    OpcGwError::Database(format!(
                        "import insert_application '{}': {}",
                        app.application_id, e
                    ))
                })?;
                app_count += 1;

                for device in &app.device_list {
                    conn.execute(
                        "INSERT INTO devices \
                         (application_id, device_id, device_name, stale_threshold_seconds, created_at, updated_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                        params![
                            app.application_id,
                            device.device_id,
                            device.device_name,
                            device.stale_threshold_seconds.map(|v| v as i64),
                            now,
                            now
                        ],
                    )
                    .map_err(|e| {
                        OpcGwError::Database(format!(
                            "import insert_device '{}/{}': {}",
                            app.application_id, device.device_id, e
                        ))
                    })?;
                    dev_count += 1;

                    for metric in &device.read_metric_list {
                        let type_str = Self::metric_type_cfg_to_str(&metric.metric_type);
                        conn.execute(
                            "INSERT INTO metrics \
                             (application_id, device_id, chirpstack_metric_name, metric_name, \
                              metric_type, metric_unit, created_at, updated_at) \
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                            params![
                                app.application_id,
                                device.device_id,
                                metric.chirpstack_metric_name,
                                metric.metric_name,
                                type_str,
                                metric.metric_unit,
                                now,
                                now
                            ],
                        )
                        .map_err(|e| {
                            OpcGwError::Database(format!(
                                "import insert_metric '{}/{}/{}': {}",
                                app.application_id,
                                device.device_id,
                                metric.chirpstack_metric_name,
                                e
                            ))
                        })?;
                        met_count += 1;
                    }

                    if let Some(commands) = &device.device_command_list {
                        for cmd in commands {
                            conn.execute(
                                "INSERT INTO commands \
                                 (application_id, device_id, command_name, command_id, \
                                  command_confirmed, command_port, command_class) \
                                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                                params![
                                    app.application_id,
                                    device.device_id,
                                    cmd.command_name,
                                    cmd.command_id,
                                    cmd.command_confirmed as i32,
                                    cmd.command_port,
                                    // Code review iter-1 HIGH: persist the
                                    // device-class binding (Epic E valve) — the
                                    // export emits it, so the import MUST too or
                                    // round-trip silently loses it.
                                    cmd.command_class
                                ],
                            )
                            .map_err(|e| {
                                OpcGwError::Database(format!(
                                    "import insert_command '{}/{}/{}': {}",
                                    app.application_id, device.device_id, cmd.command_name, e
                                ))
                            })?;
                            cmd_count += 1;
                        }
                    }
                }
            }

            // Row-count verification — the tables now hold exactly the imported
            // tree (we deleted everything first).
            let db_apps = conn
                .query_row("SELECT COUNT(*) FROM applications", [], |r| r.get::<_, i64>(0))
                .map_err(|e| OpcGwError::Database(format!("import count check apps: {}", e)))?
                as usize;
            let db_devs = conn
                .query_row("SELECT COUNT(*) FROM devices", [], |r| r.get::<_, i64>(0))
                .map_err(|e| OpcGwError::Database(format!("import count check devs: {}", e)))?
                as usize;
            let db_mets = conn
                .query_row("SELECT COUNT(*) FROM metrics", [], |r| r.get::<_, i64>(0))
                .map_err(|e| OpcGwError::Database(format!("import count check metrics: {}", e)))?
                as usize;
            let db_cmds = conn
                .query_row("SELECT COUNT(*) FROM commands", [], |r| r.get::<_, i64>(0))
                .map_err(|e| OpcGwError::Database(format!("import count check commands: {}", e)))?
                as usize;

            // Code review iter-1 M1: verify the persisted counts against the
            // INPUT structure (not the insert-loop counters, which would make
            // the check a tautology after the delete-all). A duplicate key that
            // the validator missed, or any silently-swallowed insert, makes the
            // DB count diverge from what the imported tree declared.
            let want_apps = apps.len();
            let want_devs: usize = apps.iter().map(|a| a.device_list.len()).sum();
            let want_mets: usize = apps
                .iter()
                .flat_map(|a| &a.device_list)
                .map(|d| d.read_metric_list.len())
                .sum();
            let want_cmds: usize = apps
                .iter()
                .flat_map(|a| &a.device_list)
                .map(|d| d.device_command_list.as_ref().map_or(0, |c| c.len()))
                .sum();
            if db_apps != want_apps || db_devs != want_devs
                || db_mets != want_mets || db_cmds != want_cmds
            {
                return Err(OpcGwError::Database(format!(
                    "row_count_mismatch: apps want={} actual={}, devices want={} actual={}, \
                     metrics want={} actual={}, commands want={} actual={}",
                    want_apps, db_apps, want_devs, db_devs,
                    want_mets, db_mets, want_cmds, db_cmds,
                )));
            }

            Ok(())
        })();

        match result {
            Ok(()) => {
                conn.execute_batch("COMMIT")
                    .map_err(|e| OpcGwError::Database(format!("import: commit: {}", e)))?;
                info!(
                    applications = app_count,
                    devices = dev_count,
                    metrics = met_count,
                    commands = cmd_count,
                    "import_replace_all committed (config import)"
                );
                Ok((app_count, dev_count, met_count, cmd_count))
            }
            Err(e) => {
                let _ = conn.execute_batch("ROLLBACK");
                error!(error = %e, "import_replace_all rolled back");
                Err(e)
            }
        }
    }
}


#[cfg(test)]
#[path = "sqlite_tests.rs"]
mod tests;

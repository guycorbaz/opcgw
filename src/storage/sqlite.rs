// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! SQLite-based Storage Backend Implementation
//!
//! Provides a production-grade persistent storage implementation using SQLite.
//! Features:
//! - WAL (Write-Ahead Logging) mode for concurrent readers + single writer
//! - Per-task connection pooling (Story 2-2x) for true concurrent access without Rust Mutex bottleneck
//! - Full StorageBackend trait implementation with backward-compatible API
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
use crate::storage::{ChirpstackStatus, CommandStatus, DeviceCommand, MetricType, ConnectionPool};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, trace};
use super::schema;

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
/// ```no_run
/// use opcgw::storage::SqliteBackend;
/// use std::sync::Arc;
///
/// let pool = Arc::new(opcgw::storage::ConnectionPool::new("data/opcgw.db", 3)?);
/// let backend = SqliteBackend::with_pool(Arc::clone(&pool))?;
/// // Use backend for reads/writes
/// ```
pub struct SqliteBackend {
    pool: Arc<ConnectionPool>,
}

impl SqliteBackend {
    /// Convert CommandStatus to database string representation.
    fn status_to_string(status: &CommandStatus) -> &'static str {
        match status {
            CommandStatus::Pending => "Pending",
            CommandStatus::Sent => "Sent",
            CommandStatus::Failed => "Failed",
        }
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
        schema::run_migrations(&*conn_guard)?;
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

        Ok(SqliteBackend { pool })
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
        let mut conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %device_id, metric_name = %metric_name, "Pool checkout timeout");
                e
            })?;

        let result = conn
            .query_row(
                "SELECT data_type FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
                params![device_id, metric_name],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to query metric for device {}, metric {}: {}",
                    device_id, metric_name, e
                ))
            })?;

        match result {
            Some(data_type_str) => {
                let metric_type: MetricType = data_type_str.parse()
                    .map_err(|e| {
                        tracing::warn!(
                            device_id = %device_id,
                            metric_name = %metric_name,
                            corrupted_value = %data_type_str,
                            error = %e,
                            "Corrupted metric type in database"
                        );
                        OpcGwError::Database(format!(
                            "Failed to parse metric type '{}' for {}.{}: {}",
                            data_type_str, device_id, metric_name, e
                        ))
                    })?;
                trace!(
                    device_id = %device_id,
                    metric_name = %metric_name,
                    "Retrieved metric"
                );
                Ok(Some(metric_type))
            }
            None => {
                trace!(
                    device_id = %device_id,
                    metric_name = %metric_name,
                    "Metric not found"
                );
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
        let mut conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %device_id, metric_name = %metric_name, "Pool checkout timeout");
                e
            })?;

        let data_type = value.to_string();
        let timestamp = Utc::now().to_rfc3339();
        let value_str = serde_json::to_string(&value).map_err(|e| {
            OpcGwError::Database(format!("Failed to serialize metric value: {}", e))
        })?;

        conn.execute(
                "INSERT OR REPLACE INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
                params![device_id, metric_name, value_str, data_type, timestamp],
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

        Ok(())
    }

    fn get_status(&self) -> Result<ChirpstackStatus, OpcGwError> {
        let mut conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for get_status");
                e
            })?;

        let available: Option<String> = conn
            .query_row(
                "SELECT value FROM gateway_status WHERE key = 'server_available'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to query server_available: {}", e))
            })?;

        let last_poll_time: Option<String> = conn
            .query_row(
                "SELECT value FROM gateway_status WHERE key = 'last_poll_time'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to query last_poll_time: {}", e))
            })?;

        let error_count: Option<String> = conn
            .query_row(
                "SELECT value FROM gateway_status WHERE key = 'error_count'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to query error_count: {}", e))
            })?;

        let server_available = available
            .as_ref()
            .map(|v| v.to_lowercase() == "true")
            .unwrap_or(false);
        let last_poll = last_poll_time.and_then(|ts| {
            match DateTime::parse_from_rfc3339(&ts) {
                Ok(dt) => Some(dt.with_timezone(&Utc)),
                Err(e) => {
                    tracing::warn!(
                        corrupted_timestamp = %ts,
                        error = %e,
                        "Failed to parse last_poll_time timestamp from database"
                    );
                    None
                }
            }
        });
        let errors = error_count
            .and_then(|c| {
                match c.parse::<u32>() {
                    Ok(count) => Some(count),
                    Err(e) => {
                        tracing::warn!(
                            corrupted_value = %c,
                            error = %e,
                            "Failed to parse error_count from database"
                        );
                        None
                    }
                }
            })
            .unwrap_or(0);

        Ok(ChirpstackStatus {
            server_available,
            last_poll_time: last_poll,
            error_count: errors,
        })
    }

    fn update_status(&self, status: ChirpstackStatus) -> Result<(), OpcGwError> {
        let mut conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for update_status");
                e
            })?;

        let available = if status.server_available { "true" } else { "false" };
        let error_count = status.error_count.to_string();

        conn.execute_batch("BEGIN TRANSACTION")
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to begin transaction: {}", e))
            })?;

        conn.execute(
                "INSERT OR REPLACE INTO gateway_status (key, value, updated_at) VALUES ('server_available', ?1, datetime('now'))",
                params![available],
            )
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK");
                OpcGwError::Database(format!("Failed to update server_available: {}", e))
            })?;

        conn.execute(
                "INSERT OR REPLACE INTO gateway_status (key, value, updated_at) VALUES ('last_poll_time', ?1, datetime('now'))",
                params![status.last_poll_time.map(|t| t.to_rfc3339())],
            )
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK");
                OpcGwError::Database(format!("Failed to update last_poll_time: {}", e))
            })?;

        conn.execute(
                "INSERT OR REPLACE INTO gateway_status (key, value, updated_at) VALUES ('error_count', ?1, datetime('now'))",
                params![error_count],
            )
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK");
                OpcGwError::Database(format!("Failed to update error_count: {}", e))
            })?;

        conn.execute_batch("COMMIT")
            .map_err(|e| {
                let _ = conn.execute_batch("ROLLBACK");
                OpcGwError::Database(format!("Failed to commit transaction: {}", e))
            })?;

        debug!("Updated gateway status");
        Ok(())
    }

    fn queue_command(&self, command: DeviceCommand) -> Result<(), OpcGwError> {
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

        let mut conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, device_id = %command.device_id, "Pool checkout timeout for queue_command");
                e
            })?;

        let status_str = Self::status_to_string(&CommandStatus::Pending);
        let now = Utc::now().to_rfc3339();

        conn.execute(
                "INSERT INTO command_queue (device_id, payload, f_port, status, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    command.device_id,
                    &command.payload,
                    command.f_port as i32,
                    status_str,
                    now,
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

        Ok(())
    }

    fn get_pending_commands(&self) -> Result<Vec<DeviceCommand>, OpcGwError> {
        let mut conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, "Pool checkout timeout for get_pending_commands");
                e
            })?;

        let mut stmt = conn
            .prepare("SELECT id, device_id, payload, f_port, created_at FROM command_queue WHERE status = 'Pending' ORDER BY id ASC")
            .map_err(|e| {
                OpcGwError::Database(format!("Failed to prepare statement: {}", e))
            })?;

        let commands = stmt
            .query_map([], |row| {
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

        Ok(commands)
    }

    fn update_command_status(
        &self,
        command_id: u64,
        status: CommandStatus,
    ) -> Result<(), OpcGwError> {
        let mut conn = self.pool.checkout(Duration::from_secs(5))
            .map_err(|e| {
                trace!(error = %e, command_id = command_id, "Pool checkout timeout for update_command_status");
                e
            })?;

        let status_str = Self::status_to_string(&status);

        let rows_affected = conn.execute(
                "UPDATE command_queue SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
                params![status_str, command_id as i64],
            )
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

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::StorageBackend;
    use std::fs;

    fn temp_backend_path() -> String {
        format!(
            "/tmp/opcgw_test_sqlite_{}.db",
            uuid::Uuid::new_v4()
        )
    }

    #[test]
    fn test_sqlite_backend_new_database() {
        let path = temp_backend_path();
        let result = SqliteBackend::new(&path);
        assert!(result.is_ok(), "Should create new database");
        assert!(Path::new(&path).exists(), "Database file should exist");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_metric_roundtrip() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let device_id = "device_123";
        let metric_name = "temperature";
        let value = MetricType::Float;

        backend.set_metric(device_id, metric_name, value).expect("Should store metric");
        let retrieved = backend
            .get_metric(device_id, metric_name)
            .expect("Should retrieve metric");
        assert_eq!(retrieved, Some(MetricType::Float), "Should retrieve same metric type");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_command_queue_fifo() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        for i in 0..3 {
            let cmd = DeviceCommand {
                id: 0,
                device_id: format!("device_{}", i),
                payload: vec![i as u8; 10],
                f_port: 10,
                status: CommandStatus::Pending,
                created_at: Utc::now(),
                error_message: None,
            };
            backend.queue_command(cmd).expect("Should queue command");
        }

        let commands = backend
            .get_pending_commands()
            .expect("Should get pending commands");
        assert_eq!(commands.len(), 3, "Should have 3 pending commands");

        for i in 1..3 {
            assert!(
                commands[i].id > commands[i - 1].id,
                "Commands should be in FIFO order"
            );
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_gateway_status() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let status = ChirpstackStatus {
            server_available: true,
            last_poll_time: Some(Utc::now()),
            error_count: 0,
        };

        backend.update_status(status.clone()).expect("Should update status");
        let retrieved = backend.get_status().expect("Should get status");
        assert_eq!(retrieved.server_available, status.server_available);
        assert_eq!(retrieved.error_count, status.error_count);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_concurrent_metric_updates() {
        use std::sync::Arc;
        use std::thread;

        let path = temp_backend_path();
        let backend = Arc::new(SqliteBackend::new(&path).expect("Should create backend"));
        let mut handles = vec![];

        // Spawn 4 threads, each updating different metrics on the same device
        for thread_id in 0..4 {
            let backend = Arc::clone(&backend);
            let handle = thread::spawn(move || {
                for iteration in 0..10 {
                    let metric_name = format!("metric_{}", thread_id);
                    let value = if iteration % 2 == 0 {
                        MetricType::Float
                    } else {
                        MetricType::Int
                    };
                    backend.set_metric("device_1", &metric_name, value)
                        .expect("Should store metric concurrently");
                }
            });
            handles.push(handle);
        }

        // Wait for all threads to complete
        for handle in handles {
            handle.join().expect("Thread should complete");
        }

        // Verify all metrics were stored (4 metrics, each updated 10 times)
        for thread_id in 0..4 {
            let metric_name = format!("metric_{}", thread_id);
            let retrieved = backend
                .get_metric("device_1", &metric_name)
                .expect("Should retrieve metric");
            assert!(retrieved.is_some(), "Metric {} should exist", metric_name);
        }

        let _ = fs::remove_file(&path);
    }
}

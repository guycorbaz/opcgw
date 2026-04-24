// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! SQLite Schema Management and Migrations
//!
//! This module handles:
//! - Schema initialization for new databases
//! - Version tracking via PRAGMA user_version
//! - Migration application in order
//! - Idempotent schema updates

use rusqlite::Connection;
use crate::utils::OpcGwError;
use tracing::{debug, info};

/// Embedded migration SQL files via include_str!()
/// No runtime file dependency — migrations are compiled into the binary
const MIGRATION_V001: &str = include_str!("../../migrations/v001_initial.sql");
const MIGRATION_V003: &str = include_str!("../../migrations/v003_make_payload_optional.sql");
const MIGRATION_V004: &str = include_str!("../../migrations/v004_add_command_indexes.sql");
const MIGRATION_V005: &str = include_str!("../../migrations/v005_gateway_status.sql");

/// Run all pending migrations based on current schema version.
///
/// # Process
/// 1. Read current PRAGMA user_version from database
/// 2. Compare to latest available version (1)
/// 3. Execute all migrations between current and latest in order
/// 4. Update PRAGMA user_version after each successful migration
/// 5. Log migration application at info level
///
/// # Error Handling
/// All rusqlite errors are wrapped in OpcGwError::Database with clear messages.
/// Panics are explicitly avoided — errors propagate for caller to handle gracefully.
///
/// # Example
/// ```no_run
/// use rusqlite::Connection;
/// use crate::storage::schema;
/// use crate::utils::OpcGwError;
///
/// let conn = Connection::open("data/opcgw.db")?;
/// schema::run_migrations(&conn)?;
/// // Schema is now at version 1
/// ```
pub fn run_migrations(conn: &Connection) -> Result<(), OpcGwError> {
    // Read current schema version
    let current_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| {
            OpcGwError::Database(format!("Failed to read schema version: {}", e))
        })?;

    debug!(current_version, "Current schema version read");

    // Latest available schema version
    const LATEST_VERSION: u32 = 5;

    // Apply migrations in order
    if current_version < 1 {
        debug!("Applying migration v001_initial");
        conn.execute_batch(MIGRATION_V001)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v001_initial: {}",
                    e
                ))
            })?;

        // Update schema version
        conn.pragma_update(None, "user_version", &1u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 1: {}",
                    e
                ))
            })?;

        info!(version = 1, "Applied migration v001_initial");
    }

    if current_version < 2 {
        debug!("Applying migration v002_add_command_fields");

        // List of columns to add to command_queue table
        let columns_to_add = vec![
            ("command_name", "TEXT"),
            ("parameters", "TEXT"),
            ("enqueued_at", "TEXT"),
            ("sent_at", "TEXT"),
            ("confirmed_at", "TEXT"),
            ("command_hash", "TEXT"),
            ("chirpstack_result_id", "TEXT"),
        ];

        // Add columns to command_queue table, skipping if they already exist
        for (col_name, col_type) in columns_to_add {
            let sql = format!("ALTER TABLE command_queue ADD COLUMN {} {}", col_name, col_type);
            match conn.execute(&sql, []) {
                Ok(_) => debug!("Added column {}", col_name),
                Err(e) if e.to_string().contains("duplicate column name") => {
                    debug!("Column {} already exists, skipping", col_name);
                }
                Err(e) => {
                    return Err(OpcGwError::Database(format!(
                        "Failed to add column {} to command_queue: {}",
                        col_name, e
                    )));
                }
            }
        }

        conn.pragma_update(None, "user_version", &2u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 2: {}",
                    e
                ))
            })?;

        info!(version = 2, "Applied migration v002_add_command_fields");
    }

    if current_version < 3 {
        debug!("Applying migration v003_make_payload_optional");

        conn.execute_batch(MIGRATION_V003)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v003_make_payload_optional: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", &3u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 3: {}",
                    e
                ))
            })?;

        info!(version = 3, "Applied migration v003_make_payload_optional");
    }

    if current_version < 4 {
        debug!("Applying migration v004_add_command_indexes");

        conn.execute_batch(MIGRATION_V004)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v004_add_command_indexes: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", &4u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 4: {}",
                    e
                ))
            })?;

        info!(version = 4, "Applied migration v004_add_command_indexes");
    }

    if current_version < 5 {
        debug!("Applying migration v005_gateway_status");

        conn.execute_batch(MIGRATION_V005)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v005_gateway_status: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", &5u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 5: {}",
                    e
                ))
            })?;

        info!(version = 5, "Applied migration v005_gateway_status");
    }

    // Verify final version
    let final_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| {
            OpcGwError::Database(format!(
                "Failed to verify final schema version: {}",
                e
            ))
        })?;

    debug!(
        initial = current_version,
        final = final_version,
        "Schema migration complete"
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn temp_db() -> (Connection, PathBuf) {
        let path = PathBuf::from(format!(
            "/tmp/opcgw_test_schema_{}.db",
            uuid::Uuid::new_v4()
        ));
        let conn = Connection::open(&path).expect("Failed to create temp DB");
        (conn, path)
    }

    #[test]
    fn test_run_migrations_fresh_database() {
        let (conn, path) = temp_db();
        let result = run_migrations(&conn);
        assert!(result.is_ok(), "Migration on fresh DB should succeed");

        // Verify version was set to the latest (5)
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("Failed to read version");
        assert_eq!(version, 5, "Schema version should be 5 (latest)");

        // Verify tables were created (excluding sqlite_sequence which is created automatically for AUTOINCREMENT)
        let table_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name != 'sqlite_sequence'",
                [],
                |row| row.get(0),
            )
            .expect("Failed to count tables");
        assert_eq!(table_count, 5, "Should have 5 tables (metric_values, metric_history, command_queue, gateway_status, retention_config)");

        // Cleanup
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_run_migrations_idempotent() {
        let (conn, path) = temp_db();
        let result1 = run_migrations(&conn);
        assert!(result1.is_ok(), "First migration should succeed");

        // Run again — should be idempotent
        let result2 = run_migrations(&conn);
        assert!(result2.is_ok(), "Second migration should succeed (idempotent)");

        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("Failed to read version");
        assert_eq!(version, 5, "Version should still be 5 (latest)");

        // Cleanup
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_migrations_create_all_tables() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration should succeed");

        let expected_tables = vec![
            "metric_values",
            "metric_history",
            "command_queue",
            "gateway_status",
            "retention_config",
        ];

        for table in expected_tables {
            let exists: bool = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='{}'",
                        table
                    ),
                    [],
                    |row| row.get(0),
                )
                .expect(&format!("Failed to check for table {}", table));
            assert!(exists, "Table {} should exist", table);
        }

        // Cleanup
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_migrations_retention_config_initialized() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration should succeed");

        // Check retention_config has initial rows
        let count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM retention_config",
                [],
                |row| row.get(0),
            )
            .expect("Failed to count retention_config rows");
        assert_eq!(count, 2, "Should have 2 default retention_config rows");

        // Cleanup
        let _ = fs::remove_file(&path);
    }
}

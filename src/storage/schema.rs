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
use tracing::{debug, info, warn};

/// Name of the composite index on `metric_history(device_id, timestamp)` that
/// backs all time-range history queries. Created by `v001_initial.sql` and
/// re-asserted by `v008_typed_value_constraints.sql`. Validated at startup by
/// [`validate_required_indexes`] so a dropped or never-created index surfaces
/// as a loud operator warning instead of a silent full-table-scan regression.
const METRIC_HISTORY_INDEX_NAME: &str = "idx_metric_history_device_timestamp";

/// Embedded migration SQL files via include_str!()
/// No runtime file dependency — migrations are compiled into the binary
const MIGRATION_V001: &str = include_str!("../../migrations/v001_initial.sql");
const MIGRATION_V003: &str = include_str!("../../migrations/v003_make_payload_optional.sql");
const MIGRATION_V004: &str = include_str!("../../migrations/v004_add_command_indexes.sql");
const MIGRATION_V005: &str = include_str!("../../migrations/v005_gateway_status.sql");
const MIGRATION_V006: &str = include_str!("../../migrations/v006_gateway_status_health_metrics.sql");
const MIGRATION_V007: &str = include_str!("../../migrations/v007_typed_value_columns.sql");
const MIGRATION_V008: &str = include_str!("../../migrations/v008_typed_value_constraints.sql");
const MIGRATION_V009: &str = include_str!("../../migrations/v009_application_config_tables.sql");
const MIGRATION_V010: &str = include_str!("../../migrations/v010_singleton_config_tables.sql");
const MIGRATION_V011: &str = include_str!("../../migrations/v011_command_class.sql");
const MIGRATION_V012: &str = include_str!("../../migrations/v012_device_stale_threshold.sql");
const MIGRATION_V013: &str = include_str!("../../migrations/v013_error_events.sql");

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
/// ```rust,ignore
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
    #[allow(dead_code)]
    const LATEST_VERSION: u32 = 13;

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
        conn.pragma_update(None, "user_version", 1u32.to_string())
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

        conn.pragma_update(None, "user_version", 2u32.to_string())
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

        conn.pragma_update(None, "user_version", 3u32.to_string())
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

        conn.pragma_update(None, "user_version", 4u32.to_string())
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

        conn.pragma_update(None, "user_version", 5u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 5: {}",
                    e
                ))
            })?;

        info!(version = 5, "Applied migration v005_gateway_status");
    }

    if current_version < 6 {
        debug!("Applying migration v006_gateway_status_health_metrics");

        conn.execute_batch(MIGRATION_V006)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v006_gateway_status_health_metrics: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", 6u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 6: {}",
                    e
                ))
            })?;

        info!(version = 6, "Applied migration v006_gateway_status_health_metrics");
    }

    if current_version < 7 {
        debug!("Applying migration v007_typed_value_columns");

        conn.execute_batch(MIGRATION_V007)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v007_typed_value_columns: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", 7u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 7: {}",
                    e
                ))
            })?;

        info!(version = 7, "Applied migration v007_typed_value_columns");
    }

    if current_version < 8 {
        debug!("Applying migration v008_typed_value_constraints");

        conn.execute_batch(MIGRATION_V008)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v008_typed_value_constraints: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", 8u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 8: {}",
                    e
                ))
            })?;

        info!(version = 8, "Applied migration v008_typed_value_constraints");
    }

    if current_version < 9 {
        debug!("Applying migration v009_application_config_tables");

        conn.execute_batch(MIGRATION_V009)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v009_application_config_tables: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", 9u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 9: {}",
                    e
                ))
            })?;

        info!(version = 9, "Applied migration v009_application_config_tables");
    }

    if current_version < 10 {
        debug!("Applying migration v010_singleton_config_tables");

        conn.execute_batch(MIGRATION_V010)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v010_singleton_config_tables: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", 10u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 10: {}",
                    e
                ))
            })?;

        info!(version = 10, "Applied migration v010_singleton_config_tables");
    }

    if current_version < 11 {
        debug!("Applying migration v011_command_class");

        conn.execute_batch(MIGRATION_V011)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v011_command_class: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", 11u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 11: {}",
                    e
                ))
            })?;

        info!(version = 11, "Applied migration v011_command_class");
    }

    if current_version < 12 {
        debug!("Applying migration v012_device_stale_threshold");

        conn.execute_batch(MIGRATION_V012)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v012_device_stale_threshold: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", 12u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 12: {}",
                    e
                ))
            })?;

        info!(version = 12, "Applied migration v012_device_stale_threshold");
    }

    if current_version < 13 {
        debug!("Applying migration v013_error_events");

        conn.execute_batch(MIGRATION_V013)
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to execute migration v013_error_events: {}",
                    e
                ))
            })?;

        conn.pragma_update(None, "user_version", 13u32.to_string())
            .map_err(|e| {
                OpcGwError::Database(format!(
                    "Failed to set schema version to 13: {}",
                    e
                ))
            })?;

        info!(version = 13, "Applied migration v013_error_events");
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

    // GH-74: confirm the performance-critical history index actually exists.
    // Migrations create it with `CREATE INDEX IF NOT EXISTS`, but a dropped
    // index or a partially-applied migration would otherwise degrade silently.
    validate_required_indexes(conn)?;

    Ok(())
}

/// Verify that performance-critical indexes are present after migrations.
///
/// Currently checks [`METRIC_HISTORY_INDEX_NAME`] on the `metric_history`
/// table, which backs every `metric_history` time-range query. A *missing*
/// index is **non-fatal**: the gateway logs a loud, structured `warn!` (so the
/// operator can recreate it) and continues — an absent performance index
/// degrades query speed but must not, on its own, take the service down.
/// Schema (re)creation remains the sole responsibility of the migration
/// files — this function never attempts to repair the index.
///
/// A *failed* `sqlite_master` lookup is treated differently: it signals a
/// database-level fault (locked or corrupt catalog) in which every other query
/// would fail too, so it propagates as [`OpcGwError::Database`] and aborts
/// startup — consistent with the error handling of the surrounding migration
/// steps, all of which abort on any rusqlite error.
///
/// # Errors
/// Returns [`OpcGwError::Database`] only if the `sqlite_master` lookup itself
/// fails; an absent index is reported via logging, not an error.
fn validate_required_indexes(conn: &Connection) -> Result<(), OpcGwError> {
    // Match on both the index name and its table: SQLite index names are
    // globally unique, but pinning `tbl_name` keeps the check honest if the
    // catalog is ever in an unexpected state.
    let index_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master \
             WHERE type='index' AND name=?1 AND tbl_name='metric_history'",
            [METRIC_HISTORY_INDEX_NAME],
            |row| row.get(0),
        )
        .map_err(|e| {
            OpcGwError::Database(format!(
                "Failed to verify presence of index {}: {}",
                METRIC_HISTORY_INDEX_NAME, e
            ))
        })?;

    if index_count == 0 {
        warn!(
            event = "metric_history_index_missing",
            index = METRIC_HISTORY_INDEX_NAME,
            table = "metric_history",
            impact = "time-range history queries fall back to full-table scans",
            recommended_action = "recreate the index manually (see migrations/v001_initial.sql) \
                or restore the database from a clean migration",
            "Required metric_history index is missing; history query performance will be degraded"
        );
    } else {
        debug!(
            index = METRIC_HISTORY_INDEX_NAME,
            "Required metric_history index verified present"
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use tracing_test::traced_test;

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

        // Verify version was set to the latest (9 — Story C-6 application config tables)
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("Failed to read version");
        assert_eq!(version, 13, "Schema version should be 13 (latest — G-4 error_events table)");

        // Verify tables were created (excluding sqlite_sequence which is created automatically for AUTOINCREMENT)
        let table_count: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name != 'sqlite_sequence'",
                [],
                |row| row.get(0),
            )
            .expect("Failed to count tables");
        assert_eq!(table_count, 12, "Should have 12 tables (metric_values, metric_history, command_queue, gateway_status, retention_config, applications, devices, metrics, commands, meta, singleton_config, error_events)");

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
        assert_eq!(version, 13, "Version should still be 13 (latest)");

        // Cleanup
        let _ = fs::remove_file(&path);
    }

    /// GH-74: after a normal migration the metric_history index exists, so
    /// validation passes and emits no warning.
    #[test]
    #[traced_test]
    fn test_validate_required_indexes_present_after_migration() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration should succeed");

        // Index must actually be present in sqlite_master.
        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='index' AND name=?1",
                [METRIC_HISTORY_INDEX_NAME],
                |row| row.get(0),
            )
            .expect("Failed to query index presence");
        assert!(exists, "metric_history index should exist after migration");

        // Validation succeeds and stays quiet about a missing index.
        validate_required_indexes(&conn).expect("Validation should succeed when index present");
        assert!(
            !logs_contain("metric_history_index_missing"),
            "no missing-index warning expected when the index is present"
        );

        let _ = fs::remove_file(&path);
    }

    /// GH-74: if the index is dropped, validation still succeeds (non-fatal)
    /// but emits the structured `metric_history_index_missing` warning.
    #[test]
    #[traced_test]
    fn test_validate_required_indexes_warns_when_missing() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration should succeed");

        // Simulate a dropped / never-created performance index.
        conn.execute_batch(&format!("DROP INDEX {}", METRIC_HISTORY_INDEX_NAME))
            .expect("Failed to drop index for test");

        // Non-fatal: returns Ok despite the missing index.
        validate_required_indexes(&conn)
            .expect("Validation must not fail when the index is missing");

        assert!(
            logs_contain("metric_history_index_missing"),
            "expected metric_history_index_missing warning"
        );
        assert!(
            logs_contain(METRIC_HISTORY_INDEX_NAME),
            "warning should name the missing index"
        );

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
            "applications",
            "devices",
            "metrics",
            "commands",
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
                .unwrap_or_else(|_| panic!("Failed to check for table {}", table));
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

    // ===== Story A-2: migration v007 typed value columns =====
    //
    // The tests below exercise the v006 → v007 upgrade path. To get a
    // deterministic v006-baseline database, we run the full migration runner
    // (which lands on v007), then roll the v007 columns off by `DROP COLUMN`
    // and rewind `user_version` to 6. The pre-A-2 row seeding + second
    // `run_migrations` call then exercises only the v007 application block.
    //
    // SQLite supports `ALTER TABLE ... DROP COLUMN` since 3.35.0 (rusqlite
    // 0.38 bundles 3.46+). If a future rusqlite downgrade breaks this
    // approach, replace `create_v006_baseline_db` with manual v001-v006
    // execution of `MIGRATION_V0NN` constants.

    /// Creates a temp DB at v006 schema state for upgrade-path testing.
    ///
    /// **A-3 refactor:** Story A-3 added v008 which uses CREATE TABLE … AS
    /// SELECT to install table-level CHECK constraints. CHECK constraints
    /// block the previous `ALTER TABLE … DROP COLUMN` rollback strategy
    /// (SQLite refuses to drop columns referenced by CHECK constraints), so
    /// this helper now MANUALLY runs `MIGRATION_V001` through `MIGRATION_V006`
    /// (replicating the runner's v001-v006 logic) and stops at user_version=6,
    /// producing a true v6 schema with no v007 columns + no v008 constraints.
    /// Tests built on top of this helper exercise the full v6 → latest
    /// upgrade path via a single `run_migrations(&conn)` call.
    fn create_v006_baseline_db() -> (Connection, PathBuf) {
        let (conn, path) = temp_db();

        // v001: full initial schema via execute_batch
        conn.execute_batch(MIGRATION_V001)
            .expect("Failed to apply MIGRATION_V001");

        // v002: column-add loop (the runner uses a Rust loop, not pure SQL,
        // so we replicate it here verbatim from run_migrations:84-115)
        let v002_columns = [
            ("command_name", "TEXT"),
            ("parameters", "TEXT"),
            ("enqueued_at", "TEXT"),
            ("sent_at", "TEXT"),
            ("confirmed_at", "TEXT"),
            ("command_hash", "TEXT"),
            ("chirpstack_result_id", "TEXT"),
        ];
        for (col_name, col_type) in v002_columns {
            let sql = format!("ALTER TABLE command_queue ADD COLUMN {} {}", col_name, col_type);
            conn.execute(&sql, [])
                .unwrap_or_else(|e| panic!("Failed to add v002 column {}: {}", col_name, e));
        }

        // v003 - v006: execute_batch in sequence
        conn.execute_batch(MIGRATION_V003).expect("Failed to apply MIGRATION_V003");
        conn.execute_batch(MIGRATION_V004).expect("Failed to apply MIGRATION_V004");
        conn.execute_batch(MIGRATION_V005).expect("Failed to apply MIGRATION_V005");
        conn.execute_batch(MIGRATION_V006).expect("Failed to apply MIGRATION_V006");

        conn.pragma_update(None, "user_version", 6u32.to_string())
            .expect("Failed to set user_version to 6");

        // Sanity: confirm we landed at v006
        let actual_version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("Failed to read user_version after manual v001-v006 setup");
        assert_eq!(actual_version, 6, "v6 baseline setup failed");

        (conn, path)
    }

    /// AC#6: pre-A-2 rows survive the v007 upgrade with `value_type = 'legacy'`
    /// and all typed columns NULL.
    #[test]
    fn test_v007_preserves_pre_a2_rows() {
        let (conn, path) = create_v006_baseline_db();

        // Confirm baseline starts at v006
        let pre_version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("Failed to read pre-migration version");
        assert_eq!(pre_version, 6, "Baseline must be at v006 before this test");

        // Seed 3 pre-A-2 rows in metric_values (one per non-String data_type for variety)
        let mv_seeds: Vec<(&str, &str, &str, &str)> = vec![
            ("dev1", "temperature", "23.5", "Float"),
            ("dev2", "counter", "42", "Int"),
            ("dev3", "active", "true", "Bool"),
        ];
        for (device_id, metric_name, value, data_type) in &mv_seeds {
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at) \
                 VALUES (?1, ?2, ?3, ?4, '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
                rusqlite::params![device_id, metric_name, value, data_type],
            )
            .expect("Failed to seed metric_values");
        }

        // Seed 5 pre-A-2 rows in metric_history covering all 4 data_type variants
        let mh_seeds: Vec<(&str, &str, &str, &str, &str)> = vec![
            ("dev1", "temperature", "23.5", "Float", "2024-01-01T00:00:00Z"),
            ("dev1", "temperature", "24.0", "Float", "2024-01-01T00:01:00Z"),
            ("dev2", "counter", "42", "Int", "2024-01-01T00:00:00Z"),
            ("dev3", "active", "true", "Bool", "2024-01-01T00:00:00Z"),
            ("dev4", "label", "OK", "String", "2024-01-01T00:00:00Z"),
        ];
        for (device_id, metric_name, value, data_type, timestamp) in &mh_seeds {
            conn.execute(
                "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, '2024-01-01T00:00:00Z')",
                rusqlite::params![device_id, metric_name, value, data_type, timestamp],
            )
            .expect("Failed to seed metric_history");
        }

        // Apply v007
        run_migrations(&conn).expect("v007 upgrade must succeed");

        // Schema is now at v007
        let post_version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("Failed to read post-migration version");
        assert_eq!(post_version, 13, "Post-upgrade version must be 13 (v006 → v007 → v008 → v009 → v010 → v011 → v012 → v013 in one pass)");

        // Row counts preserved
        let mv_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_values", [], |row| row.get(0))
            .expect("Failed to count metric_values");
        assert_eq!(mv_count, 3, "All 3 pre-A-2 metric_values rows must survive");

        let mh_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_history", [], |row| row.get(0))
            .expect("Failed to count metric_history");
        assert_eq!(mh_count, 5, "All 5 pre-A-2 metric_history rows must survive");

        // value_type = 'legacy' on every pre-existing row
        let mv_legacy: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM metric_values WHERE value_type = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mv_legacy, 3, "All pre-A-2 metric_values rows must be tagged 'legacy'");

        let mh_legacy: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM metric_history WHERE value_type = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mh_legacy, 5, "All pre-A-2 metric_history rows must be tagged 'legacy'");

        // All four typed columns NULL on every legacy row
        let mv_null_typed: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM metric_values \
                 WHERE value_real IS NULL AND value_int IS NULL \
                 AND value_bool IS NULL AND value_text IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mv_null_typed, 3, "Pre-A-2 rows must have all typed columns NULL");

        let mh_null_typed: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM metric_history \
                 WHERE value_real IS NULL AND value_int IS NULL \
                 AND value_bool IS NULL AND value_text IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mh_null_typed, 5, "Pre-A-2 history rows must have all typed columns NULL");

        // Existing columns retain their values byte-for-byte
        let preserved_value: String = conn
            .query_row(
                "SELECT value FROM metric_values WHERE device_id = 'dev1' AND metric_name = 'temperature'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_value, "23.5", "Legacy `value` column must survive byte-for-byte");

        let preserved_dt: String = conn
            .query_row(
                "SELECT data_type FROM metric_values WHERE device_id = 'dev2' AND metric_name = 'counter'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(preserved_dt, "Int", "Legacy `data_type` column must survive byte-for-byte");

        let _ = fs::remove_file(&path);
    }

    /// AC#8: post-migration, writers (via `SqliteBackend::upsert_metric_value`)
    /// continue to populate ONLY the legacy `value` + `data_type` columns.
    /// The typed columns remain NULL with `value_type = 'legacy'` (column
    /// default). Pinning this contract guards against accidental writer-side
    /// scope-creep into A-2.
    #[test]
    fn test_v007_writers_still_populate_legacy_columns() {
        use crate::storage::{MetricType, StorageBackend};
        use crate::storage::sqlite::SqliteBackend;

        // Invariant pinned by iter-2 review JR2 (refined by iter-3 K4):
        // `SqliteBackend::new` delegates to `Self::with_pool` (see
        // `src/storage/sqlite.rs:218-221`), which calls `run_migrations` on
        // its first pool connection. The database is therefore at v007 (with
        // all typed columns + value_type) by the time `upsert_metric_value`
        // runs below. If that invariant ever changes — e.g. migrations move
        // to a separate `initialize()` call, or `new` stops delegating to
        // `with_pool` — this test fails loudly when `upsert_metric_value`
        // tries to write to a missing table or column. NOTE: this invariant
        // is scoped to `SqliteBackend` only; `InMemoryBackend::new` has no
        // schema and no migration.
        let (_conn, db_path) = temp_db();
        let backend = SqliteBackend::new(db_path.to_str().expect("path is utf-8"))
            .expect("Failed to construct SqliteBackend");

        // Write via upsert_metric_value (the production poller path through batch_write_metrics
        // is wider; upsert_metric_value is the smallest representative write that touches
        // metric_values in the same shape).
        backend
            .upsert_metric_value(
                "dev1",
                "temperature",
                &MetricType::Float(0.0),
                std::time::SystemTime::UNIX_EPOCH,
            )
            .expect("upsert_metric_value must succeed");

        // Open a raw connection to verify the row shape directly (we cannot go through
        // backend's reader because that's also a strict-zero contract — pin via raw SQL).
        // Use single-column query_row calls rather than a wide tuple (clippy::type_complexity).
        let raw = Connection::open(&db_path).expect("re-open temp DB");
        let row_filter = "WHERE device_id = 'dev1' AND metric_name = 'temperature'";

        let legacy_value: String = raw
            .query_row(
                &format!("SELECT value FROM metric_values {}", row_filter),
                [],
                |row| row.get(0),
            )
            .expect("legacy `value` column query");
        let legacy_dt: String = raw
            .query_row(
                &format!("SELECT data_type FROM metric_values {}", row_filter),
                [],
                |row| row.get(0),
            )
            .expect("legacy `data_type` column query");

        let value_real: Option<f64> = raw
            .query_row(
                &format!("SELECT value_real FROM metric_values {}", row_filter),
                [],
                |row| row.get(0),
            )
            .expect("value_real column query");
        let value_int: Option<i64> = raw
            .query_row(
                &format!("SELECT value_int FROM metric_values {}", row_filter),
                [],
                |row| row.get(0),
            )
            .expect("value_int column query");
        let value_bool: Option<i64> = raw
            .query_row(
                &format!("SELECT value_bool FROM metric_values {}", row_filter),
                [],
                |row| row.get(0),
            )
            .expect("value_bool column query");
        let value_text: Option<String> = raw
            .query_row(
                &format!("SELECT value_text FROM metric_values {}", row_filter),
                [],
                |row| row.get(0),
            )
            .expect("value_text column query");
        let value_type: String = raw
            .query_row(
                &format!("SELECT value_type FROM metric_values {}", row_filter),
                [],
                |row| row.get(0),
            )
            .expect("value_type column query");

        // Legacy columns preserved per A-2-iter1-DEF1 heterogeneous-lexeme
        // staging contract (both = discriminant for upsert_metric_value).
        assert_eq!(legacy_value, "Float", "Legacy `value` column still carries discriminant from to_string()");
        assert_eq!(legacy_dt, "Float", "Legacy `data_type` column carries discriminant");

        // A-3 (AC#4): writers now populate typed columns. For Float(0.0):
        // value_real = Some(0.0); other typed cols NULL; value_type = 'Float'.
        assert_eq!(value_real, Some(0.0), "value_real must carry the typed payload post-A-3");
        assert!(value_int.is_none(), "value_int must be NULL for Float variant");
        assert!(value_bool.is_none(), "value_bool must be NULL for Float variant");
        assert!(value_text.is_none(), "value_text must be NULL for Float variant");

        // A-3 sets value_type to the matching discriminant
        assert_eq!(
            value_type, "Float",
            "value_type must match MetricType discriminant post-A-3"
        );

        let _ = fs::remove_file(&db_path);
    }

    /// AC#9: post-migration, readers (via `SqliteBackend::get_metric_value`)
    /// continue to project from the legacy `value` + `data_type` columns. New
    /// typed columns are NOT projected by any A-2 reader. Pinning this guards
    /// against accidental reader-side scope-creep.
    #[test]
    fn test_v007_readers_still_read_legacy_columns() {
        use crate::storage::{MetricType, StorageBackend};
        use crate::storage::sqlite::SqliteBackend;

        // Invariant pinned by iter-2 review JR2 (refined by iter-3 K4):
        // see the sibling `test_v007_writers_still_populate_legacy_columns`
        // for the full delegation chain (`SqliteBackend::new` → `with_pool`
        // → `run_migrations`).
        let (_conn, db_path) = temp_db();
        let backend = SqliteBackend::new(db_path.to_str().expect("path is utf-8"))
            .expect("Failed to construct SqliteBackend");

        backend
            .upsert_metric_value(
                "dev1",
                "temperature",
                &MetricType::Float(0.0),
                std::time::SystemTime::UNIX_EPOCH,
            )
            .expect("upsert must succeed");

        let metric = backend
            .get_metric_value("dev1", "temperature")
            .expect("get_metric_value must succeed")
            .expect("row must exist");

        // A-5: MetricValue.value: String removed. The typed `data_type`
        // payload carries the measurement; legacy discriminant-string
        // projection is no longer part of the wire contract.
        assert_eq!(metric.data_type, MetricType::Float(0.0));

        let _ = fs::remove_file(&db_path);
    }

    /// AC#2: confirm `metric_values` table has all 5 new columns post-migration.
    #[test]
    fn test_v007_adds_all_typed_columns_to_metric_values() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration must succeed");

        let expected = [
            ("value_real", "REAL", false),
            ("value_int", "INTEGER", false),
            ("value_bool", "INTEGER", false),
            ("value_text", "TEXT", false),
            ("value_type", "TEXT", true), // NOT NULL
        ];

        for (col, ty, notnull) in &expected {
            // PRAGMA table_info returns: cid, name, type, notnull, dflt_value, pk
            let (actual_ty, actual_notnull): (String, i32) = conn
                .query_row(
                    &format!(
                        "SELECT type, \"notnull\" FROM pragma_table_info('metric_values') WHERE name = '{}'",
                        col
                    ),
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|e| panic!("Column metric_values.{} missing: {}", col, e));
            assert_eq!(&actual_ty, ty, "Column {} type mismatch", col);
            assert_eq!(actual_notnull != 0, *notnull, "Column {} notnull mismatch", col);
        }

        let _ = fs::remove_file(&path);
    }

    /// AC#2: same column shape on `metric_history`.
    #[test]
    fn test_v007_adds_all_typed_columns_to_metric_history() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration must succeed");

        let expected = [
            ("value_real", "REAL", false),
            ("value_int", "INTEGER", false),
            ("value_bool", "INTEGER", false),
            ("value_text", "TEXT", false),
            ("value_type", "TEXT", true),
        ];

        for (col, ty, notnull) in &expected {
            let (actual_ty, actual_notnull): (String, i32) = conn
                .query_row(
                    &format!(
                        "SELECT type, \"notnull\" FROM pragma_table_info('metric_history') WHERE name = '{}'",
                        col
                    ),
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .unwrap_or_else(|e| panic!("Column metric_history.{} missing: {}", col, e));
            assert_eq!(&actual_ty, ty, "Column {} type mismatch", col);
            assert_eq!(actual_notnull != 0, *notnull, "Column {} notnull mismatch", col);
        }

        let _ = fs::remove_file(&path);
    }

    /// AC#7: v007 migration completes within 5 seconds on a database with ≥10 000
    /// rows across `metric_values` + `metric_history`. SQLite's `ALTER TABLE
    /// ADD COLUMN` is metadata-only post-3.25, so this typically runs in well
    /// under 100 ms — the 5 s ceiling is a wide safety margin matching the
    /// Story A.7 operator-runbook SLA for databases up to 100 MB.
    #[test]
    fn test_v007_migration_under_5s_for_10k_rows() {
        let (conn, path) = create_v006_baseline_db();

        // Seed 5000 rows in metric_values and 5000 in metric_history (total 10 000).
        // Use a single transaction so the seed itself is fast (~100ms).
        conn.execute_batch("BEGIN TRANSACTION").unwrap();
        let mv_stmt = "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at) \
                       VALUES (?1, ?2, '0.0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')";
        let mh_stmt = "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at) \
                       VALUES (?1, ?2, '0.0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')";
        let mut mv_prep = conn.prepare(mv_stmt).unwrap();
        let mut mh_prep = conn.prepare(mh_stmt).unwrap();
        for i in 0..5000 {
            let device_id = format!("dev_{}", i % 100);
            let metric_name = format!("m_{}", i);
            mv_prep
                .execute(rusqlite::params![&device_id, &metric_name])
                .unwrap();
            mh_prep
                .execute(rusqlite::params![&device_id, &metric_name])
                .unwrap();
        }
        drop(mv_prep);
        drop(mh_prep);
        conn.execute_batch("COMMIT").unwrap();

        // Time the v007 application
        let start = std::time::Instant::now();
        run_migrations(&conn).expect("v007 migration must succeed");
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_secs_f64() < 5.0,
            "v007 migration took {:?} on 10 000-row DB; AC#7 ceiling is 5 s",
            elapsed
        );

        // Sanity: row count preserved + value_type tagging applied
        let mv_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_values", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mv_count, 5000, "metric_values count must survive");

        let mh_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mh_count, 5000, "metric_history count must survive");

        let mv_legacy: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM metric_values WHERE value_type = 'legacy'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(mv_legacy, 5000, "All seeded rows must be tagged 'legacy'");

        let _ = fs::remove_file(&path);
    }

    /// AC#2: `value_type` CHECK constraint blocks invalid discriminants.
    /// Insert with `value_type = 'invalid_kind'` must fail with a CHECK error.
    #[test]
    fn test_v007_value_type_check_constraint() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration must succeed");

        // Helper: assert that `err` is a CHECK-constraint failure via the
        // structured `rusqlite::Error::SqliteFailure` extended code, not a
        // substring match on the Debug output (which would also match NOT
        // NULL violations and would silently re-pass if SQLite renames the
        // error in a future version).
        fn assert_check_constraint_violation(err: rusqlite::Error, ctx: &str) {
            match err {
                rusqlite::Error::SqliteFailure(ref sqlite_err, _) => {
                    let code = sqlite_err.extended_code;
                    // SQLITE_CONSTRAINT_CHECK = 275 (0x113); SQLITE_CONSTRAINT = 19.
                    // The extended code is preferred; rusqlite surfaces it via
                    // ffi::Error.extended_code.
                    assert_eq!(
                        code,
                        rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
                        "{}: expected SQLITE_CONSTRAINT_CHECK (275), got extended_code={}",
                        ctx,
                        code
                    );
                }
                other => panic!("{}: expected SqliteFailure, got {:?}", ctx, other),
            }
        }

        let err = conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type) \
             VALUES ('dev1', 'temperature', '23.5', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'invalid_kind')",
            [],
        );
        assert!(err.is_err(), "INSERT with invalid value_type must fail");
        assert_check_constraint_violation(err.unwrap_err(), "invalid_kind discriminant");

        // Sanity: valid value_type values are accepted (A-3 v008 cross-column
        // CHECK requires each value_type to pair with the matching typed
        // column NOT NULL; the helper sets up that pairing per variant).
        for (vt, real, int_, bool_, text) in &[
            ("legacy", None, None, None, None::<&str>),
            ("Float", Some(1.5f64), None, None, None),
            ("Int", None, Some(42i64), None, None),
            ("Bool", None, None, Some(1i64), None),
            ("String", None, None, None, Some("ok")),
        ] {
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_real, value_int, value_bool, value_text) \
                 VALUES (?1, 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![format!("dev_{}", vt), vt, real, int_, bool_, text],
            )
            .unwrap_or_else(|e| panic!("Valid value_type {} rejected: {}", vt, e));
        }

        // IL1 (iter-1 Edge F1 + F15): pin case-sensitivity of the CHECK
        // constraint. SQLite's `IN` operator uses binary collation by default,
        // so 'FLOAT', 'float', and trailing-whitespace variants are NOT in the
        // whitelist. Each of these must reject with a CHECK constraint error.
        //
        // Iter-3 K5 extension: also reject OPC UA `Variant`-side lexemes
        // (`"Boolean"`, `"Int64"`, `"Int32"`) — A-3 may be tempted to derive
        // `value_type` from the `Variant` enum's name instead of from
        // `MetricType::to_string()`. The CHECK whitelist intentionally
        // tracks `MetricType` Display (Float/Int/Bool/String) — if A-3
        // wires the wrong source it must fail loudly here.
        for bad in &[
            "FLOAT", "float", "Float ", " Float", "", "INT", "boolean",
            "Boolean", "Int64", "Int32", "Double",
        ] {
            // Each invalid value_type with a Float-shaped typed-column payload
            // (value_real=Some) — even though the row would satisfy v008's
            // cross-column CHECK for some valid value_type, the value_type
            // whitelist CHECK rejects the bad discriminant first.
            let err = conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_real) \
                 VALUES (?1, 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2, 1.5)",
                rusqlite::params![format!("dev_case_{}", bad), bad],
            );
            assert!(
                err.is_err(),
                "value_type = {:?} must be rejected by the CHECK constraint (case-sensitive whitelist)",
                bad
            );
            assert_check_constraint_violation(
                err.unwrap_err(),
                &format!("case-variant value_type = {:?}", bad),
            );
        }

        let _ = fs::remove_file(&path);
    }

    /// IL3 (iter-1 Blind F17): symmetric coverage — `metric_history` must
    /// enforce the same `value_type` CHECK constraint as `metric_values`.
    /// Iter-2 review JR3 extension: mirror IL1's case-sensitivity sweep on
    /// the history table to guard against asymmetric CHECK-definition drift
    /// between the two tables (e.g. one accidentally getting `COLLATE NOCASE`).
    #[test]
    fn test_v007_value_type_check_constraint_symmetric_on_metric_history() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration must succeed");

        // Helper: assert CHECK-constraint violation via structured extended_code.
        // Same shape as the helper in test_v007_value_type_check_constraint —
        // duplicated here (not factored) per A-2-iter1-DEF16 helper-DRY note
        // in deferred-work.md.
        fn assert_check_violation(err: rusqlite::Error, ctx: &str) {
            match err {
                rusqlite::Error::SqliteFailure(ref sqlite_err, _) => assert_eq!(
                    sqlite_err.extended_code,
                    rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
                    "{}: expected SQLITE_CONSTRAINT_CHECK, got extended_code={}",
                    ctx,
                    sqlite_err.extended_code
                ),
                other => panic!("{}: expected SqliteFailure, got {:?}", ctx, other),
            }
        }

        let err = conn.execute(
            "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type) \
             VALUES ('dev1', 'temperature', '23.5', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'invalid_kind')",
            [],
        );
        assert!(err.is_err(), "metric_history INSERT with invalid value_type must fail");
        assert_check_violation(err.unwrap_err(), "metric_history invalid_kind discriminant");

        // Valid discriminants accepted on metric_history too (A-3 v008
        // cross-column CHECK enforces typed-column pairing).
        for (vt, real, int_, bool_, text) in &[
            ("legacy", None, None, None, None::<&str>),
            ("Float", Some(1.5f64), None, None, None),
            ("Int", None, Some(42i64), None, None),
            ("Bool", None, None, Some(1i64), None),
            ("String", None, None, None, Some("ok")),
        ] {
            conn.execute(
                "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type, value_real, value_int, value_bool, value_text) \
                 VALUES (?1, 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![format!("dev_{}", vt), vt, real, int_, bool_, text],
            )
            .unwrap_or_else(|e| panic!("metric_history rejected valid value_type {}: {}", vt, e));
        }

        // JR3 case-sensitivity sweep on metric_history — mirrors IL1 on
        // metric_values. Pins the two tables' CHECK definitions to identical
        // binary-collation behaviour. Iter-3 K5 extension: also reject
        // OPC UA `Variant`-side lexemes (Boolean/Int64/Int32/Double).
        for bad in &[
            "FLOAT", "float", "Float ", " Float", "", "INT", "boolean",
            "Boolean", "Int64", "Int32", "Double",
        ] {
            let err = conn.execute(
                "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type, value_real) \
                 VALUES (?1, 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2, 1.5)",
                rusqlite::params![format!("dev_case_{}", bad), bad],
            );
            assert!(
                err.is_err(),
                "metric_history value_type = {:?} must be rejected by the CHECK constraint",
                bad
            );
            assert_check_violation(
                err.unwrap_err(),
                &format!("metric_history case-variant value_type = {:?}", bad),
            );
        }

        let _ = fs::remove_file(&path);
    }

    /// IM1 (iter-1 Blind F20 + Edge F12): the `value_bool` column has a
    /// `CHECK(value_bool IS NULL OR value_bool IN (0, 1))` constraint —
    /// rejects sentinel / out-of-domain integers on both tables. A-3 v008
    /// adds the cross-column CHECK that pairs value_bool with value_type=Bool.
    #[test]
    fn test_v007_value_bool_check_constraint() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration must succeed");

        // NULL is allowed for value_type='legacy' rows (all typed cols NULL)
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at) \
             VALUES ('dev1', 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [],
        )
        .expect("Default value_type='legacy' row with all typed NULL must be allowed");

        // 0 and 1 are allowed with value_type='Bool' (A-3 v008 cross-column pairing)
        for v in &[0i64, 1i64] {
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_bool, value_type) \
                 VALUES (?1, 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2, 'Bool')",
                rusqlite::params![format!("dev_bool_{}", v), v],
            )
            .unwrap_or_else(|e| panic!("value_bool = {} must be accepted: {}", v, e));
        }

        // Anything else must be rejected by the column-level value_bool CHECK
        for bad in &[-1i64, 2i64, 99i64, i64::MAX, i64::MIN] {
            let err = conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_bool, value_type) \
                 VALUES (?1, 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2, 'Bool')",
                rusqlite::params![format!("dev_bad_{}", bad), bad],
            );
            assert!(
                err.is_err(),
                "value_bool = {} must be rejected by the CHECK constraint",
                bad
            );
            match err.unwrap_err() {
                rusqlite::Error::SqliteFailure(ref sqlite_err, _) => assert_eq!(
                    sqlite_err.extended_code,
                    rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
                    "value_bool = {} must trip the CHECK constraint",
                    bad
                ),
                other => panic!("value_bool = {}: expected SqliteFailure, got {:?}", bad, other),
            }
        }

        // K6 (iter-3 Edge F5): mirror the full bad-vector on metric_history
        // to defend against asymmetric CHECK definition drift (e.g. a typo
        // expanding the history-side IN list to `(0,1,2)`).
        for bad in &[-1i64, 2i64, 99i64, 42i64, i64::MAX, i64::MIN] {
            let err = conn.execute(
                "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_bool, value_type) \
                 VALUES (?1, 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2, 'Bool')",
                rusqlite::params![format!("dev_hist_{}", bad), bad],
            );
            assert!(
                err.is_err(),
                "metric_history.value_bool = {} must be rejected by the CHECK constraint",
                bad
            );
            match err.unwrap_err() {
                rusqlite::Error::SqliteFailure(ref sqlite_err, _) => assert_eq!(
                    sqlite_err.extended_code,
                    rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
                    "metric_history.value_bool = {} must trip the CHECK constraint",
                    bad
                ),
                other => panic!(
                    "metric_history.value_bool = {}: expected SqliteFailure, got {:?}",
                    bad, other
                ),
            }
        }

        let _ = fs::remove_file(&path);
    }

    // ===== Story A-3 iter-1 IR3 + IR10: missing test deliverables =====

    /// AC#6 negative test (iter-1 IR10): the v008 cross-column CHECK rejects
    /// any row where `value_type` does not pair with the matching typed
    /// column being non-NULL. Pinned for both `metric_values` and
    /// `metric_history`.
    #[test]
    fn test_v008_cross_column_check_rejects_inconsistent_rows() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("run migrations");

        // value_type='Float' but value_real=NULL → reject
        let err = conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type) \
             VALUES ('d', 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Float')",
            [],
        );
        assert!(err.is_err(), "value_type='Float' + value_real=NULL must be rejected");

        // value_type='legacy' but value_real=Some → reject
        let err = conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_real) \
             VALUES ('d2', 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'legacy', 1.5)",
            [],
        );
        assert!(err.is_err(), "value_type='legacy' + value_real NOT NULL must be rejected");

        // value_type='Bool' but value_text=Some → reject (Bool requires value_bool NOT NULL, others NULL)
        let err = conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_bool, value_text) \
             VALUES ('d3', 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Bool', 1, 'oops')",
            [],
        );
        assert!(err.is_err(), "value_type='Bool' + value_text NOT NULL must be rejected");

        // Symmetric on metric_history — full triad mirroring metric_values
        // (iter-2 F-J): Float-null, legacy-with-real, Bool-with-text.
        // A typo / missed AND clause / copy-paste fail on metric_history's
        // CHECK would slip through without all three sub-cases.
        let err = conn.execute(
            "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type) \
             VALUES ('d', 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Float')",
            [],
        );
        assert!(err.is_err(), "metric_history: value_type='Float' + value_real=NULL must be rejected");

        let err = conn.execute(
            "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type, value_real) \
             VALUES ('d2', 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'legacy', 1.5)",
            [],
        );
        assert!(err.is_err(), "metric_history: value_type='legacy' + value_real NOT NULL must be rejected");

        let err = conn.execute(
            "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type, value_bool, value_text) \
             VALUES ('d3', 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Bool', 1, 'oops')",
            [],
        );
        assert!(err.is_err(), "metric_history: value_type='Bool' + value_text NOT NULL must be rejected");

        let _ = fs::remove_file(&path);
    }

    /// AC#6 positive test (iter-1 IR10): the v008 cross-column CHECK accepts
    /// a row for each (value_type, typed-column) pairing.
    #[test]
    fn test_v008_cross_column_check_accepts_consistent_rows() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("run migrations");

        // Each variant with the correct typed-column pairing must INSERT cleanly.
        // Split into individual INSERTs to keep clippy::type_complexity happy
        // (A-2 iter-1 IM3 / A-3 iter-1 IR6 precedent — avoid wide tuples).
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type) \
             VALUES ('dev_legacy', 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'legacy')",
            [],
        )
        .expect("legacy variant must accept");
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_real) \
             VALUES ('dev_float', 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Float', 23.5)",
            [],
        )
        .expect("Float variant must accept");
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_int) \
             VALUES ('dev_int', 'm', '0', 'Int', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Int', 42)",
            [],
        )
        .expect("Int variant must accept");
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_bool) \
             VALUES ('dev_bool', 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Bool', 1)",
            [],
        )
        .expect("Bool variant must accept");
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_text) \
             VALUES ('dev_string', 'm', '0', 'String', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'String', 'ok')",
            [],
        )
        .expect("String variant must accept");

        let _ = fs::remove_file(&path);
    }

    /// AC#7 (iter-1 IR3 + iter-2 F-E/F-G): v008 migration completes within 30s
    /// for 10 000 + 10 000 rows (matching AC#7's literal contract). 30s is the
    /// operator-runbook SLA per Story A.7 (looser than v007's 5s because v008's
    /// CREATE TABLE … AS SELECT is O(table-size) rather than metadata-only).
    /// Seed pre-A-3 rows tagged value_type='legacy' so they satisfy the new
    /// cross-column CHECK on the SELECT. Post-migration, the test also pins
    /// `user_version == 8` to prove v008 actually ran (iter-2 F-E): a
    /// silent-no-op runner would otherwise leave the SLA assertion vacuous.
    #[test]
    fn test_v008_migration_under_30s_for_10k_rows() {
        let (conn, path) = create_v006_baseline_db();

        // Apply v007 manually to get the schema shape without v008's CHECK
        // — we need to seed rows AT v007 then time the v008 application.
        conn.execute_batch(MIGRATION_V007).expect("apply v007 manually");
        conn.pragma_update(None, "user_version", 7u32.to_string())
            .expect("set user_version=7");

        // Seed 10 000 metric_values + 10 000 metric_history rows tagged
        // 'legacy' (column default; all typed cols NULL) — satisfies v008
        // CHECK. Matches AC#7's literal "10 000 + 10 000" contract.
        conn.execute_batch("BEGIN TRANSACTION").unwrap();
        let mv_stmt = "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at) \
                       VALUES (?1, ?2, '0.0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')";
        let mh_stmt = "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at) \
                       VALUES (?1, ?2, '0.0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')";
        let mut mv_prep = conn.prepare(mv_stmt).unwrap();
        let mut mh_prep = conn.prepare(mh_stmt).unwrap();
        for i in 0..10_000 {
            let device_id = format!("dev_{}", i % 100);
            let metric_name = format!("m_{}", i);
            mv_prep.execute(rusqlite::params![&device_id, &metric_name]).unwrap();
            mh_prep.execute(rusqlite::params![&device_id, &metric_name]).unwrap();
        }
        drop(mv_prep);
        drop(mh_prep);
        conn.execute_batch("COMMIT").unwrap();

        // Time the v008 application
        let start = std::time::Instant::now();
        run_migrations(&conn).expect("v008 migration must succeed");
        let elapsed = start.elapsed();

        assert!(
            elapsed.as_secs_f64() < 30.0,
            "v008 migration took {:?} on 10 000 + 10 000 row DB; AC#7 ceiling is 30 s",
            elapsed
        );

        // iter-2 F-E: prove v008 actually ran (not a silent-no-op fallthrough)
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 13, "run_migrations must have advanced user_version to 13 (v008 + v009 + v010 + v011 + v012 + v013)");

        // Sanity: row counts preserved through the recreate
        let mv_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_values", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mv_count, 10_000, "metric_values count preserved through v008 recreate");

        let mh_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mh_count, 10_000, "metric_history count preserved through v008 recreate");

        let _ = fs::remove_file(&path);
    }

    /// IR10 + Blind F20: v008 preserves per-column data through the CREATE
    /// TABLE … AS SELECT recreate. Seeds typed-column data at v007, runs
    /// v008, asserts the typed payload survives.
    #[test]
    fn test_v008_preserves_typed_column_data_through_recreate() {
        let (conn, path) = create_v006_baseline_db();
        conn.execute_batch(MIGRATION_V007).expect("apply v007 manually");
        conn.pragma_update(None, "user_version", 7u32.to_string())
            .expect("set user_version=7");

        // Seed legacy-typed rows at v007 (typed cols NULL; value_type='legacy' default)
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at) \
             VALUES ('dev1', 'temp', 'Float', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();

        // Apply v008
        run_migrations(&conn).expect("v008 migration");

        // Legacy row's legacy columns preserved + value_type='legacy' (default carries through)
        let (vt, vr, leg_value): (String, Option<f64>, String) = conn
            .query_row(
                "SELECT value_type, value_real, value FROM metric_values WHERE device_id='dev1' AND metric_name='temp'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(vt, "legacy", "pre-A-3 row carries value_type='legacy' through v008 recreate");
        assert!(vr.is_none(), "pre-A-3 row's typed cols stay NULL through v008 recreate");
        assert_eq!(leg_value, "Float", "legacy `value` column preserved byte-for-byte through v008 recreate");

        let _ = fs::remove_file(&path);
    }

    /// Story A-7 AC#5 — end-to-end regression test for the **chained v006 → v007
    /// → v008 auto-migration path** that real operator upgrades exercise.
    ///
    /// The existing `test_v007_migration_under_5s_for_10k_rows` and
    /// `test_v008_migration_under_30s_for_10k_rows` each cover ONE migration in
    /// isolation (the latter manually `execute_batch`s v007 first, then times
    /// v008 alone). This test calls `run_migrations(&conn)` **once** against a
    /// fresh v006-baseline database — the same production code path that fires
    /// on the first startup of the v2.0 binary against a v2.0-rc database.
    ///
    /// Coverage rationale: the per-migration tests prove each migration works
    /// in isolation; this test proves they work **chained** through the runner.
    /// A future regression that breaks the runner's chaining logic (e.g. the
    /// `if current_version < N` guards drifting out of sync) would be caught
    /// here but NOT by the per-migration siblings.
    ///
    /// SLA: per the Story A-7 runbook's "5s for typical residential / small-
    /// scale deployments" target, the chained migration on a 10 000-row baseline
    /// must complete in under 5 seconds. Larger databases (≥100MB ≈ 500k rows)
    /// scale roughly linearly with the v008 CREATE TABLE … AS SELECT cost and
    /// may take multiple minutes — documented as a known limitation in
    /// `docs/deployment-guide.md § "Epic A migration" § "SLA expectation"`.
    /// The dev-agent decision D1 (story spec) accepted this as the
    /// operator-realistic SLA target rather than tuning v008 (which would
    /// re-open A-3's review loop).
    #[test]
    fn test_v006_to_v008_full_upgrade_path_under_5s() {
        let (conn, path) = create_v006_baseline_db();

        // Sanity: the helper landed us at v006 exactly.
        let starting_version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(starting_version, 6, "create_v006_baseline_db must land at v006");

        // Seed 5000 + 5000 = 10 000 pre-Epic-A rows using ONLY the v006-shaped
        // columns (`value` TEXT + `data_type` TEXT). The v007 typed columns
        // don't exist at v006; they'll be added by the migration runner.
        conn.execute_batch("BEGIN TRANSACTION").unwrap();
        let mv_stmt = "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at) \
                       VALUES (?1, ?2, 'Float', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')";
        let mh_stmt = "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at) \
                       VALUES (?1, ?2, 'Float', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')";
        let mut mv_prep = conn.prepare(mv_stmt).unwrap();
        let mut mh_prep = conn.prepare(mh_stmt).unwrap();
        for i in 0..5000 {
            let device_id = format!("dev_{}", i % 100);
            let metric_name = format!("m_{}", i);
            mv_prep
                .execute(rusqlite::params![&device_id, &metric_name])
                .unwrap();
            mh_prep
                .execute(rusqlite::params![&device_id, &metric_name])
                .unwrap();
        }
        drop(mv_prep);
        drop(mh_prep);
        conn.execute_batch("COMMIT").unwrap();

        // Time the chained v007 + v008 application through a SINGLE
        // `run_migrations()` call — the production code path.
        let start = std::time::Instant::now();
        run_migrations(&conn).expect("chained v006 -> v008 migration must succeed");
        let elapsed = start.elapsed();

        // SLA assertion per Story A-7 AC#5 + AC#1 § "SLA expectation":
        // 5 seconds for typical small-scale deployments (≤10k rows). Larger
        // databases are documented to take longer — see runbook.
        assert!(
            elapsed.as_secs_f64() < 5.0,
            "Chained v006 -> v008 migration took {:?} on 10 000-row DB; Story A-7 \
             SLA ceiling is 5 s. Larger databases may legitimately exceed this — \
             see docs/deployment-guide.md § 'SLA expectation'.",
            elapsed
        );

        // Prove the runner actually advanced to v008 (NOT a silent-no-op
        // fallthrough — same iter-2 F-E pin as test_v008_migration_under_30s).
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            version, 13,
            "run_migrations must advance v006 -> v013 in a single call (real \
             operator upgrade path)"
        );

        // Row counts survive the full chain (v007 ADD COLUMN is metadata-only;
        // v008 CREATE TABLE … AS SELECT preserves rows verbatim).
        let mv_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_values", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mv_count, 5000, "metric_values row count preserved through v006 -> v008 chain");

        let mh_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_history", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mh_count, 5000, "metric_history row count preserved through v006 -> v008 chain");

        // Per Story A-7 AC#5: all pre-existing rows must be tagged
        // value_type='legacy' (the v007 column default) and the typed columns
        // (value_real, value_int, value_bool, value_text) must all be NULL —
        // the Story A-4 / A-5 contract for legacy-row surfacing as
        // BadDataUnavailable in OPC UA.
        let mv_legacy_with_null_typed: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM metric_values \
                 WHERE value_type = 'legacy' \
                 AND value_real IS NULL \
                 AND value_int IS NULL \
                 AND value_bool IS NULL \
                 AND value_text IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            mv_legacy_with_null_typed, 5000,
            "All metric_values rows must be tagged 'legacy' with all typed columns NULL \
             (Story A-7 AC#5 + A-4 BadDataUnavailable contract)"
        );

        let mh_legacy_with_null_typed: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM metric_history \
                 WHERE value_type = 'legacy' \
                 AND value_real IS NULL \
                 AND value_int IS NULL \
                 AND value_bool IS NULL \
                 AND value_text IS NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            mh_legacy_with_null_typed, 5000,
            "All metric_history rows must be tagged 'legacy' with all typed columns NULL \
             (Story A-5 HistoryRead BadDataUnavailable contract)"
        );

        // Per Story A-7 AC#5: the v008 exactly-one-non-NULL CHECK constraint
        // must be enforceable on the post-migration schema. Insert a row with
        // TWO non-NULL typed columns should fail with a CHECK error — proving
        // v008 actually ran (vs a silent fallthrough at v007).
        //
        // iter-1 K4 review fix + iter-2 L5 comment clarification: pair the
        // negative case with a POSITIVE case to prove v008's CHECK is
        // ENFORCEABLE in both directions (rejects invalid + accepts valid).
        //
        // What constraints fire on the negative-case insert below?
        //   * v007 adds two column-level CHECKs: `value_bool IN (0, 1)` and
        //     `value_type IN ('legacy', 'Float', 'Int', 'Bool', 'String')`.
        //   * v008 adds the cross-column CHECK enforcing "exactly one of
        //     {value_real, value_int, value_bool, value_text} non-NULL,
        //     matching value_type discriminant".
        // The negative insert uses `value_type='Float'` (allowed by v007's
        // value_type enum), `value_bool=NULL` (passes v007's value_bool
        // CHECK), and both `value_real=1.0` AND `value_int=2` non-NULL.
        // ONLY v008's cross-column CHECK can fire here — v007 has no
        // cross-column CHECK. So the negative case unambiguously pins v008.
        // The positive case (single value_real non-NULL with matching
        // value_type='Float') proves v008's CHECK isn't over-broad
        // (rejecting valid rows). Together the pair pins "enforceable" in
        // both directions per AC#5's plain reading.

        // Negative case: multi-non-NULL must FAIL with SQLITE_CONSTRAINT_CHECK.
        let multi_non_null = conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_real, value_int) \
             VALUES ('check_test_neg', 'dual', 'Float', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Float', 1.0, 2)",
            [],
        );
        assert!(
            multi_non_null.is_err(),
            "v008 CHECK must reject multi-non-NULL typed-column inserts; got Ok — v008 \
             may not have actually applied"
        );
        if let Err(rusqlite::Error::SqliteFailure(sqlite_err, _)) = multi_non_null {
            assert_eq!(
                sqlite_err.extended_code,
                rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
                "v008 multi-non-NULL insert must fail with SQLITE_CONSTRAINT_CHECK; \
                 got extended_code={}",
                sqlite_err.extended_code
            );
        } else {
            panic!(
                "v008 multi-non-NULL insert must fail with rusqlite::Error::SqliteFailure \
                 (got non-Sqlite error: {multi_non_null:?})"
            );
        }

        // Positive case (iter-1 K4 review fix): exactly-one-non-NULL with
        // value_type matching the populated column must SUCCEED. If this fails,
        // v008's CHECK is over-broad (rejects valid rows too) — a different
        // regression class than the negative-case test catches.
        let exactly_one_non_null = conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type, value_real) \
             VALUES ('check_test_pos', 'single', 'Float', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', 'Float', 1.5)",
            [],
        );
        assert!(
            exactly_one_non_null.is_ok(),
            "v008 CHECK must ACCEPT exactly-one-non-NULL typed-column inserts; got Err \
             — v008's CHECK is over-broad and rejects valid rows: {:?}",
            exactly_one_non_null.err()
        );

        let _ = fs::remove_file(&path);
    }

    // ===== Story C-6: migration v009 application config tables =====

    /// AC#3 (fresh DB): applying run_migrations to a fresh DB lands on v009
    /// with all 4 application-config tables present.
    #[test]
    fn test_v009_creates_application_config_tables() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("migration must succeed");

        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read version");
        assert_eq!(version, 13, "fresh DB must land at v013 (G-4 error_events table)");

        for table in &["applications", "devices", "metrics", "commands"] {
            let exists: bool = conn
                .query_row(
                    &format!(
                        "SELECT COUNT(*) > 0 FROM sqlite_master \
                         WHERE type='table' AND name='{}'",
                        table
                    ),
                    [],
                    |row| row.get(0),
                )
                .unwrap_or_else(|_| panic!("failed to check table {}", table));
            assert!(exists, "table {} must exist after v009", table);
        }

        let _ = fs::remove_file(&path);
    }

    /// AC#3 (idempotent): running run_migrations on an already-v9 DB is a no-op.
    #[test]
    fn test_v009_idempotent() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("first migration");
        run_migrations(&conn).expect("second migration must be idempotent");

        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read version");
        assert_eq!(version, 13, "version must stay at 13 (latest) after second run");

        let _ = fs::remove_file(&path);
    }

    /// AC#1: cascade delete — removing an application removes devices, metrics,
    /// and commands via FK ON DELETE CASCADE.
    #[test]
    fn test_v009_cascade_delete() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("migration");

        conn.execute_batch("PRAGMA foreign_keys = ON;").unwrap();

        conn.execute(
            "INSERT INTO applications (application_id, application_name, created_at, updated_at) \
             VALUES ('a1', 'App One', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO devices (application_id, device_id, device_name, created_at, updated_at) \
             VALUES ('a1', 'd1', 'Dev 1', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO metrics (application_id, device_id, chirpstack_metric_name, metric_name, metric_type, created_at, updated_at) \
             VALUES ('a1', 'd1', 'temp', 'temperature', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO commands (application_id, device_id, command_name, command_id) \
             VALUES ('a1', 'd1', 'reset', 1)",
            [],
        )
        .unwrap();

        conn.execute("DELETE FROM applications WHERE application_id = 'a1'", []).unwrap();

        let dev_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM devices", [], |row| row.get(0))
            .unwrap();
        assert_eq!(dev_count, 0, "devices must be cascade-deleted with their application");

        let metric_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metrics", [], |row| row.get(0))
            .unwrap();
        assert_eq!(metric_count, 0, "metrics must be cascade-deleted with their application");

        let cmd_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cmd_count, 0, "commands must be cascade-deleted with their application");

        let _ = fs::remove_file(&path);
    }

    // ===== Story D-0: migration v010 singleton_config =====

    /// D-0 AC#1: the v010 migration creates the `singleton_config` table.
    #[test]
    fn test_v010_creates_singleton_config_table() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("migration");

        let exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM sqlite_master \
                 WHERE type='table' AND name='singleton_config'",
                [],
                |row| row.get(0),
            )
            .expect("singleton_config existence check");
        assert!(exists, "table singleton_config must exist after v010");

        // Schema shape: section + key composite PK, value + updated_at TEXT.
        let cols: Vec<(String, String, i32)> = conn
            .prepare("SELECT name, type, \"notnull\" FROM pragma_table_info('singleton_config')")
            .unwrap()
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let names: Vec<&str> = cols.iter().map(|(n, _, _)| n.as_str()).collect();
        assert!(names.contains(&"section"), "section column required");
        assert!(names.contains(&"key"), "key column required");
        assert!(names.contains(&"value"), "value column required");
        assert!(names.contains(&"updated_at"), "updated_at column required");

        let _ = fs::remove_file(&path);
    }

    /// D-0 AC#3 (idempotent): running run_migrations on an already-v10 DB is a no-op.
    #[test]
    fn test_v010_idempotent() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("first migration");
        run_migrations(&conn).expect("second migration must be idempotent");

        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("read version");
        assert_eq!(version, 13, "version must stay at 13 (latest) after second run");

        let _ = fs::remove_file(&path);
    }

    /// D-0 AC#1: the section CHECK rejects rogue section names. D-1's UI cannot
    /// accidentally create a fifth namespace without a schema migration.
    #[test]
    fn test_v010_section_check_constraint() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("migration");

        // Accept all four valid sections.
        for section in &["global", "chirpstack", "opcua", "web"] {
            conn.execute(
                "INSERT INTO singleton_config (section, key, value, updated_at) \
                 VALUES (?1, 'k', 'v', '2024-01-01T00:00:00Z')",
                rusqlite::params![section],
            )
            .unwrap_or_else(|e| panic!("section {} must be accepted: {}", section, e));
        }

        // Reject anything else.
        for bad in &["", "Global", "GLOBAL", "logging", "secrets"] {
            let err = conn.execute(
                "INSERT INTO singleton_config (section, key, value, updated_at) \
                 VALUES (?1, 'k', 'v', '2024-01-01T00:00:00Z')",
                rusqlite::params![bad],
            );
            assert!(
                err.is_err(),
                "section = {:?} must be rejected by the CHECK constraint",
                bad
            );
            match err.unwrap_err() {
                rusqlite::Error::SqliteFailure(ref e, _) => assert_eq!(
                    e.extended_code,
                    rusqlite::ffi::SQLITE_CONSTRAINT_CHECK,
                    "section {:?}: expected SQLITE_CONSTRAINT_CHECK, got {}",
                    bad,
                    e.extended_code
                ),
                other => panic!("section {:?}: expected SqliteFailure, got {:?}", bad, other),
            }
        }

        let _ = fs::remove_file(&path);
    }

    /// D-0 AC#1: composite PK `(section, key)` rejects duplicate keys within a
    /// section but allows the same key under different sections.
    #[test]
    fn test_v010_composite_pk() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("migration");

        conn.execute(
            "INSERT INTO singleton_config (section, key, value, updated_at) \
             VALUES ('global', 'debug', 'true', '2024-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
        // Same section + same key → reject.
        let err = conn.execute(
            "INSERT INTO singleton_config (section, key, value, updated_at) \
             VALUES ('global', 'debug', 'false', '2024-01-01T00:00:00Z')",
            [],
        );
        assert!(err.is_err(), "duplicate (section, key) must be rejected");

        // Same key under a different section → allowed.
        conn.execute(
            "INSERT INTO singleton_config (section, key, value, updated_at) \
             VALUES ('chirpstack', 'debug', 'true', '2024-01-01T00:00:00Z')",
            [],
        )
        .expect("same key under different section must be allowed");

        let _ = fs::remove_file(&path);
    }
}

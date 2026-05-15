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
const MIGRATION_V006: &str = include_str!("../../migrations/v006_gateway_status_health_metrics.sql");
const MIGRATION_V007: &str = include_str!("../../migrations/v007_typed_value_columns.sql");

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
    const LATEST_VERSION: u32 = 7;

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

        // Verify version was set to the latest (7 — Epic A migration v007 typed value columns)
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("Failed to read version");
        assert_eq!(version, 7, "Schema version should be 7 (latest)");

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
        assert_eq!(version, 7, "Version should still be 7 (latest)");

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

    /// Creates a temp DB rolled back to v006 schema state for upgrade-path testing.
    ///
    /// **Forward-compat guard (iter-3 review K2):** this helper rolls v007 off
    /// by dropping the 5 columns A-2 added. It is only valid while v007 is the
    /// latest migration. When a future v008+ lands that touches any of those
    /// column names — or that adds columns this helper doesn't know to drop —
    /// the "v006 baseline" produced here silently diverges from what
    /// `MIGRATION_V001..V006` would produce on a fresh DB. The assertion below
    /// fails loudly the moment `LATEST_VERSION` advances past 7, forcing the
    /// next-story dev to refactor this helper (e.g. switch to manual v001-v006
    /// execution of the `MIGRATION_V0NN` constants).
    fn create_v006_baseline_db() -> (Connection, PathBuf) {
        // Constant is the same as in run_migrations(); a mismatch means a new
        // migration was added without updating this helper.
        const HELPER_LATEST_VERSION: u32 = 7;
        // Cross-check against run_migrations' own LATEST_VERSION expectations
        // — this is a compile-time-friendly invariant pin.
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Failed to run baseline migrations");
        let actual_version: u32 = conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .expect("Failed to read post-migration version");
        assert_eq!(
            actual_version, HELPER_LATEST_VERSION,
            "create_v006_baseline_db is only valid while v{} is the latest migration; \
             run_migrations landed at v{} which means a new migration was added \
             without updating this helper. Refactor to manual v001-v006 execution.",
            HELPER_LATEST_VERSION, actual_version
        );

        // Roll back v007 by dropping the columns it added
        let v007_columns = ["value_real", "value_int", "value_bool", "value_text", "value_type"];
        for col in &v007_columns {
            conn.execute(
                &format!("ALTER TABLE metric_values DROP COLUMN {}", col),
                [],
            )
            .unwrap_or_else(|e| panic!("Failed to drop metric_values.{}: {}", col, e));
            conn.execute(
                &format!("ALTER TABLE metric_history DROP COLUMN {}", col),
                [],
            )
            .unwrap_or_else(|e| panic!("Failed to drop metric_history.{}: {}", col, e));
        }

        conn.pragma_update(None, "user_version", 6u32.to_string())
            .expect("Failed to rewind user_version to 6");
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
        assert_eq!(post_version, 7, "Post-upgrade version must be 7");

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

        // Legacy columns populated per pre-A-2 contract
        assert_eq!(legacy_value, "Float", "Legacy `value` column carries discriminant from to_string()");
        assert_eq!(legacy_dt, "Float", "Legacy `data_type` column carries discriminant");

        // Typed columns NULL — A-2 doesn't wire writers
        assert!(value_real.is_none(), "value_real must be NULL post-A-2");
        assert!(value_int.is_none(), "value_int must be NULL post-A-2");
        assert!(value_bool.is_none(), "value_bool must be NULL post-A-2");
        assert!(value_text.is_none(), "value_text must be NULL post-A-2");

        // Default value_type applied
        assert_eq!(
            value_type, "legacy",
            "value_type column default must apply to writer rows in A-2 (A-3 will set it explicitly)"
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

        // The reader reconstructs MetricValue from legacy columns only.
        // The `value` field carries the legacy stringified discriminant
        // (`upsert_metric_value` writes `value.to_string()` which is "Float").
        assert_eq!(
            metric.value, "Float",
            "Reader must project legacy `value` column unchanged in A-2"
        );
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

        // Sanity: valid value_type values are accepted
        for vt in &["legacy", "Float", "Int", "Bool", "String"] {
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type) \
                 VALUES (?1, 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2)",
                rusqlite::params![format!("dev_{}", vt), vt],
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
            let err = conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_type) \
                 VALUES (?1, 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2)",
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

        // Valid discriminants accepted on metric_history too
        for vt in &["legacy", "Float", "Int", "Bool", "String"] {
            conn.execute(
                "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type) \
                 VALUES (?1, 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2)",
                rusqlite::params![format!("dev_{}", vt), vt],
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
                "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type) \
                 VALUES (?1, 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2)",
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
    /// rejects sentinel / out-of-domain integers on both tables.
    #[test]
    fn test_v007_value_bool_check_constraint() {
        let (conn, path) = temp_db();
        run_migrations(&conn).expect("Migration must succeed");

        // NULL is allowed
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_bool) \
             VALUES ('dev1', 'm', '0', 'Float', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', NULL)",
            [],
        )
        .expect("NULL value_bool must be allowed");

        // 0 and 1 are allowed
        for v in &[0i64, 1i64] {
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_bool) \
                 VALUES (?1, 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2)",
                rusqlite::params![format!("dev_bool_{}", v), v],
            )
            .unwrap_or_else(|e| panic!("value_bool = {} must be accepted: {}", v, e));
        }

        // Anything else must be rejected — pin the schema-side defence
        for bad in &[-1i64, 2i64, 99i64, i64::MAX, i64::MIN] {
            let err = conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_bool) \
                 VALUES (?1, 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2)",
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
                "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_bool) \
                 VALUES (?1, 'm', '0', 'Bool', '2024-01-01T00:00:00Z', '2024-01-01T00:00:00Z', ?2)",
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
}

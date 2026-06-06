// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story C-6 (commit 1c09911) extracted the SqliteBackend test suite
// out of `sqlite.rs` into this sibling file via `#[path = "sqlite_tests.rs"]
// mod tests;` (see `src/storage/sqlite.rs:3821`). The extraction
// preserved the original indentation from the inner `mod tests { ... }`
// block, which is why the test fns below start at column 4 instead of
// column 0 — still valid Rust, since this file is included as the body
// of the `tests` module declared in sqlite.rs.

    use super::*;
    use crate::storage::{StorageBackend, BatchMetricWrite};
    use std::time::SystemTime;
    use std::fs;
    use tracing_test::traced_test;

    /// Story 6-3, AC#7: `log_sqlite_busy_if_applicable` emits a structured
    /// `storage_query` warn when the underlying rusqlite error is
    /// `SQLITE_BUSY`, and is silent on any other rusqlite error code. No
    /// retry happens — the helper is purely diagnostic.
    #[test]
    #[traced_test]
    fn sqlite_busy_warn_on_database_busy() {
        let busy = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::DatabaseBusy,
                extended_code: 0,
            },
            Some("database is locked".to_string()),
        );
        log_sqlite_busy_if_applicable(&busy, "test_query", 1, 42);
        assert!(
            logs_contain("operation=\"storage_query\""),
            "expected storage_query warn"
        );
        assert!(
            logs_contain("error=\"SQLITE_BUSY\""),
            "expected SQLITE_BUSY error marker"
        );
        assert!(
            logs_contain("retry_attempt=1"),
            "expected retry_attempt=1"
        );
        assert!(
            logs_contain("latency_ms=42"),
            "expected latency_ms=42"
        );
    }

    /// Story 6-3, AC#7 negative case: a non-busy rusqlite error must NOT
    /// emit the SQLITE_BUSY warn — the helper is silent for other codes.
    #[test]
    #[traced_test]
    fn sqlite_busy_silent_on_other_codes() {
        let other = rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: rusqlite::ErrorCode::ConstraintViolation,
                extended_code: 0,
            },
            Some("UNIQUE constraint failed".to_string()),
        );
        log_sqlite_busy_if_applicable(&other, "test_query", 0, 5);
        assert!(
            !logs_contain("error=\"SQLITE_BUSY\""),
            "must not emit SQLITE_BUSY for non-busy error code"
        );
    }

    /// Story 6-3, AC#3 verification: a `StorageOpLog` whose lifetime crosses
    /// `STORAGE_QUERY_BUDGET_MS` (10 ms) emits a structured `warn!` with
    /// `exceeded_budget=true` instead of the routine `debug!`.
    #[test]
    #[traced_test]
    fn storage_query_budget_emits_warn_when_exceeded() {
        {
            let mut op = StorageOpLog::start("test_slow_query");
            op.ok();
            std::thread::sleep(Duration::from_millis(15));
            // op drops here, emitting the structured log
        }
        assert!(
            logs_contain("operation=\"storage_query\""),
            "expected storage_query op log to be emitted"
        );
        assert!(
            logs_contain("exceeded_budget=true"),
            "expected exceeded_budget=true marker after >10 ms operation"
        );
        assert!(
            logs_contain("budget_ms=10"),
            "expected budget_ms=10 to match STORAGE_QUERY_BUDGET_MS"
        );
    }

    /// Story 6-3, AC#3: a fast storage query stays at `debug!` and never
    /// emits the `exceeded_budget` marker.
    ///
    /// Iter-3 review pending #4 resolution: marked `#[ignore]` because the
    /// 10 ms threshold is brittle under heavy CI load — a slow runner can
    /// push the no-sleep `Drop` past 10 ms and falsely flap. The
    /// AC-positive case (`storage_query_warn_when_budget_exceeded` above)
    /// is the load-bearing assertion; this negative-side test is a
    /// belt-and-suspenders check kept available for manual invocation:
    /// `cargo test --bin opcgw storage_query_below_budget -- --ignored`.
    /// A non-brittle replacement would require a `StorageOpLog::with_clock`
    /// constructor for injecting a deterministic timer — recorded in the
    /// review follow-ups list, not in this story's scope.
    #[test]
    #[traced_test]
    #[ignore]
    fn storage_query_below_budget_stays_at_debug() {
        {
            let mut op = StorageOpLog::start("test_fast_query");
            op.ok();
            // No sleep — this should drop in well under 10 ms.
        }
        assert!(
            logs_contain("operation=\"storage_query\""),
            "expected storage_query op log"
        );
        assert!(
            !logs_contain("exceeded_budget"),
            "fast query must not carry the exceeded_budget marker"
        );
    }

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
        let value = MetricType::Float(0.0);

        backend.set_metric(device_id, metric_name, value).expect("Should store metric");
        let retrieved = backend
            .get_metric(device_id, metric_name)
            .expect("Should retrieve metric");
        assert_eq!(retrieved, Some(MetricType::Float(0.0)), "Should retrieve same metric type");
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
                        MetricType::Float(0.0)
                    } else {
                        MetricType::Int(0)
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

    #[test]
    fn test_upsert_metric_value_preserves_created_at() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let device_id = "device_test";
        let metric_name = "temperature";
        let value = MetricType::Float(0.0);
        let t1 = std::time::SystemTime::now();

        // First insert
        backend.upsert_metric_value(device_id, metric_name, &value, t1)
            .expect("Should insert metric");

        // Retrieve created_at from database
        let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
            .expect("Should checkout connection");
        let created_at_1: String = conn.query_row(
            "SELECT created_at FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
            rusqlite::params![device_id, metric_name],
            |row| row.get(0)
        ).expect("Should get created_at");

        drop(conn);

        // Wait a bit, then update the same metric
        std::thread::sleep(std::time::Duration::from_millis(100));
        let t2 = std::time::SystemTime::now();
        backend.upsert_metric_value(device_id, metric_name, &value, t2)
            .expect("Should update metric");

        // Verify created_at is unchanged
        let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
            .expect("Should checkout connection");
        let created_at_2: String = conn.query_row(
            "SELECT created_at FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
            rusqlite::params![device_id, metric_name],
            |row| row.get(0)
        ).expect("Should get created_at after update");

        assert_eq!(created_at_1, created_at_2, "created_at should be preserved on UPSERT");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_upsert_100_metrics_no_duplicates() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Insert 100 metrics (10 devices × 10 metrics each)
        for device_num in 0..10 {
            for metric_num in 0..10 {
                let device_id = format!("device_{}", device_num);
                let metric_name = format!("metric_{}", metric_num);
                let value = if metric_num % 2 == 0 {
                    MetricType::Float(0.0)
                } else {
                    MetricType::Int(0)
                };

                backend.upsert_metric_value(&device_id, &metric_name, &value, now)
                    .expect("Should upsert metric");
            }
        }

        // Verify count is exactly 100 (no duplicates)
        let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
            .expect("Should checkout connection");
        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_values",
            [],
            |row| row.get(0)
        ).expect("Should count rows");

        assert_eq!(count, 100, "Should have exactly 100 unique metrics");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_upsert_preserves_metric_type_information() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        let test_cases = vec![
            ("device_a", "metric_float", MetricType::Float(0.0)),
            ("device_a", "metric_int", MetricType::Int(0)),
            ("device_a", "metric_bool", MetricType::Bool(false)),
            ("device_a", "metric_string", MetricType::String(String::new())),
        ];

        // Insert different metric types
        for (device_id, metric_name, metric_type) in &test_cases {
            backend.upsert_metric_value(device_id, metric_name, metric_type, now)
                .expect("Should upsert metric");
        }

        // Verify each type is stored correctly
        for (device_id, metric_name, expected_type) in test_cases {
            let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
                .expect("Should checkout connection");
            let stored_type: String = conn.query_row(
                "SELECT data_type FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
                rusqlite::params![device_id, metric_name],
                |row| row.get(0)
            ).expect("Should get data_type");

            assert_eq!(stored_type, expected_type.to_string(),
                "Type for {}.{} should be {}", device_id, metric_name, expected_type);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_batch_write_metrics_roundtrip() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Create batch of 10 metrics — A-5: typed payload carries the real value.
        let batch: Vec<crate::storage::BatchMetricWrite> = (0..10)
            .map(|i| {
                let data_type = if i % 2 == 0 {
                    MetricType::Float(i as f64 + 0.5)
                } else {
                    MetricType::Int(i as i64)
                };
                crate::storage::BatchMetricWrite {
                    device_id: "device_batch_test".to_string(),
                    metric_name: format!("metric_{}", i),
                    data_type,
                    timestamp: now,
                }
            })
            .collect();

        // Write batch
        backend.batch_write_metrics(batch).expect("Should write batch");

        // Verify all metrics exist
        for i in 0..10 {
            let metric_name = format!("metric_{}", i);
            let result = backend.get_metric("device_batch_test", &metric_name)
                .expect("Should retrieve metric");
            assert!(result.is_some(), "Metric {} should exist", metric_name);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_batch_write_metrics_atomicity() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Create batch with valid metrics — A-5 typed payload.
        let batch: Vec<crate::storage::BatchMetricWrite> = (0..5)
            .map(|i| crate::storage::BatchMetricWrite {
                device_id: "device_atomic_test".to_string(),
                metric_name: format!("metric_{}", i),
                data_type: MetricType::Float(i as f64 + 0.5),
                timestamp: now,
            })
            .collect();

        // Write batch
        backend.batch_write_metrics(batch).expect("Should write batch");

        // Verify count is 5
        let conn = backend.pool.checkout(Duration::from_secs(5))
            .expect("Should checkout connection");
        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_values WHERE device_id = 'device_atomic_test'",
            [],
            |row| row.get(0)
        ).expect("Should count rows");

        assert_eq!(count, 5, "Should have exactly 5 metrics after batch write");

        // Verify history rows match metric count (1 entry per metric)
        let history_count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_history WHERE device_id = 'device_atomic_test'",
            [],
            |row| row.get(0)
        ).expect("Should count history rows");

        assert_eq!(history_count, 5, "Should have exactly 5 history entries matching metrics");

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_batch_write_metrics_400_all_types() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Create batch of 400 metrics (100 of each type) — A-5 typed payload.
        let mut batch = Vec::new();
        for device_num in 0..100 {
            for type_num in 0..4 {
                let data_type = match type_num {
                    0 => MetricType::Float(device_num as f64 + 0.5),
                    1 => MetricType::Int(device_num as i64),
                    2 => MetricType::Bool(true),
                    3 => MetricType::String(format!("text_{}", device_num)),
                    _ => MetricType::Float(device_num as f64 + 0.5),
                };
                batch.push(crate::storage::BatchMetricWrite {
                    device_id: format!("device_{}", device_num),
                    metric_name: format!("metric_{}", type_num),
                    data_type,
                    timestamp: now,
                });
            }
        }

        assert_eq!(batch.len(), 400, "Should have 400 metrics in batch");

        // Write batch
        backend.batch_write_metrics(batch).expect("Should write batch");

        // Verify count
        let conn = backend.pool.checkout(Duration::from_secs(5))
            .expect("Should checkout connection");
        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_values",
            [],
            |row| row.get(0)
        ).expect("Should count rows");

        assert_eq!(count, 400, "Should have exactly 400 unique metrics");

        // Verify all types are preserved
        for type_num in 0..4 {
            let expected_type = match type_num {
                0 => "Float",
                1 => "Int",
                2 => "Bool",
                3 => "String",
                _ => "Float",
            };
            let type_count: i32 = conn.query_row(
                "SELECT COUNT(*) FROM metric_values WHERE data_type = ?1",
                rusqlite::params![expected_type],
                |row| row.get(0)
            ).expect("Should count by type");
            assert_eq!(type_count, 100, "Should have 100 metrics of type {}", expected_type);
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_batch_write_atomicity_on_failure() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Write initial metric to database
        backend.upsert_metric_value("device_fail_test", "metric_initial", &MetricType::Float(0.0), now)
            .expect("Should write initial metric");

        // Verify initial state
        let conn = backend.pool.checkout(Duration::from_secs(5))
            .expect("Should checkout connection");
        let initial_count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_values",
            [],
            |row| row.get(0)
        ).expect("Should count rows");
        assert_eq!(initial_count, 1, "Should have 1 initial metric");
        drop(conn);

        // Create batch with 5 metrics — A-5 typed payload.
        let batch: Vec<crate::storage::BatchMetricWrite> = (0..5)
            .map(|i| crate::storage::BatchMetricWrite {
                device_id: "device_batch_rollback".to_string(),
                metric_name: format!("metric_{}", i),
                data_type: MetricType::Float(i as f64 + 0.5),
                timestamp: now,
            })
            .collect();

        // Successfully write batch
        backend.batch_write_metrics(batch).expect("Should write batch");

        // Verify all 5 metrics + initial metric exist
        let conn = backend.pool.checkout(Duration::from_secs(5))
            .expect("Should checkout connection");
        let final_count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_values",
            [],
            |row| row.get(0)
        ).expect("Should count rows");
        assert_eq!(final_count, 6, "Should have 6 total metrics (1 initial + 5 batch)");

        // Verify history records exist for batch metrics (1 per metric in batch)
        let history_count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_history WHERE device_id = 'device_batch_rollback'",
            [],
            |row| row.get(0)
        ).expect("Should count history rows");
        assert_eq!(history_count, 5, "Should have 5 history entries for batch metrics");

        let _ = std::fs::remove_file(&path);
    }

    // Epic 8 retrospective action item #3 (2026-05-01): this test asserts that
    // a concurrent reader sees at least one of 50 written metrics. The original
    // shape spawned writer + reader simultaneously and the reader could finish
    // all 50 attempts before the writer committed *any* row — `found == 0`
    // tripped the assertion non-deterministically. `#[serial_test::serial]`
    // alone doesn't help (the race is in-test, not cross-test).
    //
    // Fix: write `metric_0` synchronously before spawning the reader, so the
    // reader is guaranteed to see at least one committed row regardless of
    // thread-scheduling order. The writer thread continues writing 1..50 in
    // parallel with the reader to preserve the original concurrent-access
    // intent.
    #[test]
    #[serial_test::serial]
    fn test_concurrent_write_read_isolation() {
        use std::sync::Arc;
        use std::thread;

        let path = temp_backend_path();
        let backend = Arc::new(SqliteBackend::new(&path).expect("Should create backend"));
        let now = std::time::SystemTime::now();

        // Pre-write metric_0 so the reader has at least one row to find,
        // independent of writer/reader thread scheduling.
        backend
            .upsert_metric_value("device_w", "metric_0", &MetricType::Float(0.0), now)
            .expect("Pre-write: should upsert metric_0");

        // Writer thread — concurrent writes for indices 1..50
        let backend_w = Arc::clone(&backend);
        let writer = thread::spawn(move || {
            for i in 1..50 {
                let metric_name = format!("metric_{}", i);
                let value = if i % 2 == 0 { MetricType::Float(0.0) } else { MetricType::Int(0) };
                backend_w.upsert_metric_value("device_w", &metric_name, &value, now)
                    .expect("Writer: should upsert");
            }
        });

        // Reader thread — reads 0..50; metric_0 is guaranteed by the pre-write,
        // metrics 1..50 are racy (any subset is acceptable).
        let backend_r = Arc::clone(&backend);
        let reader = thread::spawn(move || {
            let mut found_count = 0;
            for i in 0..50 {
                let metric_name = format!("metric_{}", i);
                if let Ok(Some(_)) = backend_r.get_metric("device_w", &metric_name) {
                    found_count += 1;
                }
            }
            found_count
        });

        writer.join().expect("Writer should complete");
        let found = reader.join().expect("Reader should complete");

        assert!(found > 0, "Reader should see some written metrics (at least metric_0 from the pre-write)");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_append_metric_history_roundtrip() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let device_id = "device_test";
        let metric_name = "temperature";
        let value = MetricType::Float(0.0);
        let t1 = std::time::SystemTime::now();

        // First append
        backend.append_metric_history(device_id, metric_name, &value, t1)
            .expect("Should append metric");

        // Query history from database (in a scoped block to release connection)
        {
            let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
                .expect("Should checkout connection");
            let history_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM metric_history WHERE device_id = ?1 AND metric_name = ?2",
                rusqlite::params![device_id, metric_name],
                |row| row.get(0)
            ).expect("Should count rows");
            assert_eq!(history_count, 1, "Should have 1 history row after first append");
            drop(conn);
        }

        // Second append with later timestamp
        std::thread::sleep(std::time::Duration::from_millis(10));
        let t2 = std::time::SystemTime::now();
        backend.append_metric_history(device_id, metric_name, &value, t2)
            .expect("Should append second metric");

        // Verify both rows exist and are ordered by timestamp (again in a scoped block)
        {
            let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
                .expect("Should checkout connection");
            let history_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM metric_history WHERE device_id = ?1 AND metric_name = ?2",
                rusqlite::params![device_id, metric_name],
                |row| row.get(0)
            ).expect("Should count rows");
            assert_eq!(history_count, 2, "Should have 2 history rows after second append");

            // Verify timestamp ordering
            let timestamps: Vec<String> = conn.prepare(
                "SELECT timestamp FROM metric_history WHERE device_id = ?1 AND metric_name = ?2 ORDER BY timestamp ASC"
            ).expect("Should prepare query")
                .query_map(rusqlite::params![device_id, metric_name], |row| row.get(0))
                .expect("Should query")
                .collect::<Result<Vec<_>, _>>()
                .expect("Should collect results");

            assert_eq!(timestamps.len(), 2);
            assert!(timestamps[0] <= timestamps[1], "Timestamps should be in ascending order");
            drop(conn);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_append_100_metrics_to_history() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Append 100 metrics (10 devices × 10 metrics)
        for device_num in 0..10 {
            for metric_num in 0..10 {
                let device_id = format!("device_{}", device_num);
                let metric_name = format!("metric_{}", metric_num);
                let value = if metric_num % 2 == 0 { MetricType::Float(0.0) } else { MetricType::Int(0) };

                backend.append_metric_history(&device_id, &metric_name, &value, now)
                    .expect("Should append metric");
            }
        }

        // Verify count
        {
            let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
                .expect("Should checkout connection");
            let total_count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM metric_history",
                [],
                |row| row.get(0)
            ).expect("Should count rows");
            assert_eq!(total_count, 100, "Should have exactly 100 history rows");
            drop(conn);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_historical_data_timestamp_ordering() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let device_id = "device_order";
        let metric_name = "sensor";

        // Append 5 metrics with different timestamps in non-sequential order
        let base_time = std::time::SystemTime::now();
        let timestamps = [base_time + std::time::Duration::from_secs(3),
            base_time + std::time::Duration::from_secs(1),
            base_time + std::time::Duration::from_secs(4),
            base_time + std::time::Duration::from_secs(2),
            base_time + std::time::Duration::from_secs(5)];

        for (idx, ts) in timestamps.iter().enumerate() {
            let value = if idx % 2 == 0 { MetricType::Float(0.0) } else { MetricType::Int(0) };
            backend.append_metric_history(device_id, metric_name, &value, *ts)
                .expect("Should append metric");
        }

        // Verify rows are returned in timestamp order
        {
            let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
                .expect("Should checkout connection");
            let retrieved_timestamps: Vec<String> = conn.prepare(
                "SELECT timestamp FROM metric_history WHERE device_id = ?1 AND metric_name = ?2 ORDER BY timestamp ASC"
            ).expect("Should prepare query")
                .query_map(rusqlite::params![device_id, metric_name], |row| row.get(0))
                .expect("Should query")
                .collect::<Result<Vec<_>, _>>()
                .expect("Should collect results");

            assert_eq!(retrieved_timestamps.len(), 5, "Should have 5 rows");
            // Verify ascending order
            for i in 0..4 {
                assert!(retrieved_timestamps[i] <= retrieved_timestamps[i + 1],
                    "Row {} timestamp should be <= row {} timestamp", i, i + 1);
            }
            drop(conn);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_historical_data_preserves_types() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        let device_id = "device_types";
        let types_to_test = vec![
            ("temp_float", MetricType::Float(0.0)),
            ("count_int", MetricType::Int(0)),
            ("active_bool", MetricType::Bool(false)),
            ("label_str", MetricType::String(String::new())),
        ];

        // Append metrics with different types
        for (metric_name, value) in &types_to_test {
            backend.append_metric_history(device_id, metric_name, value, now)
                .expect("Should append metric");
        }

        // Verify data_type is stored correctly for each
        {
            let conn = backend.pool.checkout(std::time::Duration::from_secs(5))
                .expect("Should checkout connection");

            for (metric_name, expected_value) in &types_to_test {
                let stored_type: String = conn.query_row(
                    "SELECT data_type FROM metric_history WHERE device_id = ?1 AND metric_name = ?2",
                    rusqlite::params![device_id, metric_name],
                    |row| row.get(0)
                ).expect("Should query type");

                // Verify that data_type stores the variant name (e.g., "Float", "Int", "Bool", "String")
                // This relies on MetricType's Display impl returning just the variant name
                let expected_type = expected_value.to_string();
                assert_eq!(stored_type, expected_type, "Type mismatch for {}: expected '{}', got '{}'", metric_name, expected_type, stored_type);
            }
            drop(conn);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_concurrent_append_read_isolation() {
        use std::sync::Arc;
        use std::thread;

        let path = temp_backend_path();
        let backend = Arc::new(SqliteBackend::new(&path).expect("Should create backend"));
        let now = std::time::SystemTime::now();

        // Appender thread
        let backend_a = Arc::clone(&backend);
        let appender = thread::spawn(move || {
            for i in 0..30 {
                let metric_name = format!("metric_{}", i);
                let value = if i % 2 == 0 { MetricType::Float(0.0) } else { MetricType::Int(0) };
                backend_a.append_metric_history("device_append", &metric_name, &value, now)
                    .expect("Appender: should append");
            }
        });

        // Reader thread (reading from history on separate connection)
        let backend_r = Arc::clone(&backend);
        let reader = thread::spawn(move || {
            let mut found_count = 0;
            std::thread::sleep(std::time::Duration::from_millis(50));
            for attempt in 0..50 {
                let conn = match backend_r.pool.checkout(std::time::Duration::from_secs(1)) {
                    Ok(c) => c,
                    Err(_) => continue,
                };
                let history_count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM metric_history WHERE device_id = ?1",
                    rusqlite::params!["device_append"],
                    |row| row.get(0)
                ).unwrap_or_default();
                drop(conn);
                if history_count > 0 {
                    found_count += 1;
                }
                if attempt < 49 {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
            found_count
        });

        appender.join().expect("Appender should complete");
        let found = reader.join().expect("Reader should complete");

        // Verify that reader found history entries (at least once during the appending)
        assert!(found > 0, "Reader should see appended history entries");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_empty_database() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let metrics = backend.load_all_metrics().expect("Should load all metrics");
        assert!(metrics.is_empty(), "Empty database should return empty vec");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_single_metric() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        backend.upsert_metric_value("device_1", "temperature", &MetricType::Float(0.0), now)
            .expect("Should upsert");

        let metrics = backend.load_all_metrics().expect("Should load all metrics");
        assert_eq!(metrics.len(), 1, "Should have exactly 1 metric");
        assert_eq!(metrics[0].device_id, "device_1");
        assert_eq!(metrics[0].metric_name, "temperature");
        assert_eq!(metrics[0].data_type, MetricType::Float(0.0));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_multiple_devices() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Insert metrics for multiple devices
        backend.upsert_metric_value("device_a", "metric_1", &MetricType::Float(0.0), now)
            .expect("Should upsert");
        backend.upsert_metric_value("device_a", "metric_2", &MetricType::Int(0), now)
            .expect("Should upsert");
        backend.upsert_metric_value("device_b", "metric_1", &MetricType::Bool(false), now)
            .expect("Should upsert");
        backend.upsert_metric_value("device_b", "metric_3", &MetricType::String(String::new()), now)
            .expect("Should upsert");

        let metrics = backend.load_all_metrics().expect("Should load all metrics");
        assert_eq!(metrics.len(), 4, "Should have exactly 4 metrics");

        // Verify metrics are present (order may vary)
        let device_a_count = metrics.iter().filter(|m| m.device_id == "device_a").count();
        let device_b_count = metrics.iter().filter(|m| m.device_id == "device_b").count();
        assert_eq!(device_a_count, 2, "device_a should have 2 metrics");
        assert_eq!(device_b_count, 2, "device_b should have 2 metrics");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_all_data_types() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        let test_cases = vec![
            ("device", "float_metric", MetricType::Float(0.0)),
            ("device", "int_metric", MetricType::Int(0)),
            ("device", "bool_metric", MetricType::Bool(false)),
            ("device", "string_metric", MetricType::String(String::new())),
        ];

        for (device_id, metric_name, metric_type) in &test_cases {
            backend.upsert_metric_value(device_id, metric_name, metric_type, now)
                .expect("Should upsert");
        }

        let metrics = backend.load_all_metrics().expect("Should load all metrics");
        assert_eq!(metrics.len(), 4, "Should have all 4 metrics");

        // Verify types are correct
        for metric in metrics {
            let expected_type = test_cases
                .iter()
                .find(|(_, name, _)| name == &metric.metric_name)
                .map(|(_, _, t)| t.clone())
                .expect("Should find metric in test cases");
            assert_eq!(metric.data_type, expected_type, "Type mismatch for {}", metric.metric_name);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_100_metrics() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Insert 100 metrics
        for i in 0..100 {
            let device_id = format!("device_{}", i / 10);
            let metric_name = format!("metric_{}", i);
            let metric_type = match i % 4 {
                0 => MetricType::Float(0.0),
                1 => MetricType::Int(0),
                2 => MetricType::Bool(false),
                _ => MetricType::String(String::new()),
            };
            backend.upsert_metric_value(&device_id, &metric_name, &metric_type, now)
                .expect("Should upsert");
        }

        let metrics = backend.load_all_metrics().expect("Should load all metrics");
        assert_eq!(metrics.len(), 100, "Should load exactly 100 metrics");

        // Verify all metrics are different
        let mut unique_keys = std::collections::HashSet::new();
        for metric in &metrics {
            let key = (metric.device_id.clone(), metric.metric_name.clone());
            assert!(unique_keys.insert(key), "Duplicate metric found");
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_performance() {
        use std::time::Instant;

        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = std::time::SystemTime::now();

        // Insert 100 metrics
        for i in 0..100 {
            let device_id = format!("device_{}", i / 10);
            let metric_name = format!("metric_{}", i);
            backend.upsert_metric_value(&device_id, &metric_name, &MetricType::Float(0.0), now)
                .expect("Should upsert");
        }

        // Measure load time
        let start = Instant::now();
        let metrics = backend.load_all_metrics().expect("Should load all metrics");
        let elapsed = start.elapsed();

        assert_eq!(metrics.len(), 100, "Should load 100 metrics");
        assert!(elapsed.as_millis() < 1000, "Should load 100 metrics in < 1 second, took: {:?}", elapsed);

        let _ = fs::remove_file(&path);
    }

    // ========== Story 2-5a: Historical Data Pruning Tests ==========

    #[test]
    fn test_prune_old_metrics_deletes_expired_rows() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let now = SystemTime::now();
        // Insert metrics with timestamps: 15, 10, 5, 0 days ago
        for days_ago in &[15, 10, 5, 0] {
            let timestamp = now - std::time::Duration::from_secs(86400 * days_ago);
            backend.append_metric_history(
                "device_1",
                &format!("metric_{}", days_ago),
                &MetricType::Float(0.0),
                timestamp,
            ).expect("Should append");
        }

        // Verify all 4 metrics were appended to history
        {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM metric_history",
                [],
                |row| row.get(0),
            ).expect("Should query");
            assert_eq!(count, 4, "Should have 4 metrics before pruning");
        } // Connection returned to pool

        // Prune metrics older than 7 days (should remove 15 and 10 day old metrics)
        let deleted = backend.prune_old_metrics(7).expect("Should prune");
        assert_eq!(deleted, 2, "Should delete 2 old metrics");

        // Verify only newer metrics remain (5 and 0 days ago)
        {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            let count_after: i64 = conn.query_row(
                "SELECT COUNT(*) FROM metric_history",
                [],
                |row| row.get(0),
            ).expect("Should query");
            assert_eq!(count_after, 2, "Should have 2 metrics after pruning");
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_old_metrics_retains_recent_data() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let now = SystemTime::now();
        // Insert metrics with timestamps: 2 days ago and today
        for days_ago in &[2, 0] {
            let timestamp = now - std::time::Duration::from_secs(86400 * days_ago);
            backend.append_metric_history(
                "device_1",
                &format!("metric_{}", days_ago),
                &MetricType::Float(0.0),
                timestamp,
            ).expect("Should append");
        }

        // Prune with 7-day retention (nothing should be deleted)
        let deleted = backend.prune_old_metrics(7).expect("Should prune");
        assert_eq!(deleted, 0, "Should not delete recent metrics");

        // Verify all metrics still exist
        let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM metric_history",
            [],
            |row| row.get(0),
        ).expect("Should query");
        assert_eq!(count, 2, "Should retain both recent metrics");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_old_metrics_handles_empty_database() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        // Prune empty database
        let deleted = backend.prune_old_metrics(7).expect("Should prune");
        assert_eq!(deleted, 0, "Should handle empty database gracefully");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_old_metrics_with_multiple_devices() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let now = SystemTime::now();
        // Insert metrics for 3 devices with mixed ages
        for device_num in 0..3 {
            for days_ago in &[15, 7, 1] {
                let timestamp = now - std::time::Duration::from_secs(86400 * days_ago);
                backend.append_metric_history(
                    &format!("device_{}", device_num),
                    &format!("metric_{}", days_ago),
                    &MetricType::Float(0.0),
                    timestamp,
                ).expect("Should append");
            }
        }

        // Should have 9 metrics total (3 devices × 3 timestamps)
        {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            let count: i64 = conn.query_row(
                "SELECT COUNT(*) FROM metric_history",
                [],
                |row| row.get(0),
            ).expect("Should query");
            assert_eq!(count, 9, "Should have 9 metrics before pruning");
        } // Connection returned to pool

        // Prune with 10-day retention (removes only 15-day-old metrics)
        let deleted = backend.prune_old_metrics(10).expect("Should prune");
        assert_eq!(deleted, 3, "Should delete 3 old metrics (1 per device)");

        // Should have 6 metrics left (3 devices × 2 timestamps)
        {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
            let count_after: i64 = conn.query_row(
                "SELECT COUNT(*) FROM metric_history",
                [],
                |row| row.get(0),
            ).expect("Should query");
            assert_eq!(count_after, 6, "Should have 6 metrics after pruning");
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_old_metrics_preserves_metric_values() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let now = SystemTime::now();
        // Insert metrics with different data types
        let timestamp_old = now - std::time::Duration::from_secs(864000); // 10 days
        let timestamp_new = now - std::time::Duration::from_secs(86400);  // 1 day

        backend.append_metric_history(
            "device_1",
            "old_metric",
            &MetricType::Int(0),
            timestamp_old,
        ).expect("Should append");

        backend.append_metric_history(
            "device_1",
            "new_float",
            &MetricType::Float(0.0),
            timestamp_new,
        ).expect("Should append");

        backend.append_metric_history(
            "device_1",
            "new_bool",
            &MetricType::Bool(false),
            timestamp_new,
        ).expect("Should append");

        // Prune 7-day-old data
        let deleted = backend.prune_old_metrics(7).expect("Should prune");
        assert_eq!(deleted, 1, "Should delete 1 old metric");

        // Verify remaining metrics from history
        let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        let mut stmt = conn.prepare(
            "SELECT metric_name, data_type FROM metric_history"
        ).expect("Should prepare");
        let metrics: Vec<_> = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
            ))
        }).expect("Should query")
            .collect::<Result<Vec<_>, _>>()
            .expect("Should parse");

        assert_eq!(metrics.len(), 2, "Should have 2 metrics remaining");

        // Verify data types are preserved
        let has_float = metrics.iter().any(|(name, _)| name == "new_float");
        let has_bool = metrics.iter().any(|(name, _)| name == "new_bool");
        assert!(has_float, "Should preserve float metric");
        assert!(has_bool, "Should preserve bool metric");

        let _ = fs::remove_file(&path);
    }

    // ============== Story 2-4b: Graceful Degradation Tests ==============

    #[test]
    fn test_load_all_metrics_graceful_degradation_valid_data() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert valid metrics across multiple devices
        for i in 1..=10 {
            backend.upsert_metric_value(
                &format!("device_{}", i),
                "temperature",
                &MetricType::Float(0.0),
                now,
            ).expect("Should upsert");
        }

        // Load metrics should succeed for all valid data
        let metrics = backend.load_all_metrics().expect("Should load metrics");
        assert_eq!(metrics.len(), 10, "Should load all 10 valid metrics");

        for metric in &metrics {
            assert!(!metric.device_id.is_empty(), "device_id should not be empty");
            assert!(!metric.metric_name.is_empty(), "metric_name should not be empty");
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_with_parse_errors() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert valid metrics
        backend.upsert_metric_value("device_1", "metric_1", &MetricType::Float(0.0), now)
            .expect("Should upsert");
        backend.upsert_metric_value("device_2", "metric_2", &MetricType::Int(0), now)
            .expect("Should upsert");

        // Insert metrics with invalid data_type directly into database
        {
            let conn = backend.pool.checkout(Duration::from_secs(5))
                .expect("Should checkout");
            let now_rfc3339 = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    "device_3",
                    "metric_3",
                    "123.45",
                    "invalid_type",  // Invalid data type
                    &now_rfc3339,
                    &now_rfc3339,
                    &now_rfc3339,
                ],
            ).expect("Should insert invalid type");
        }

        // Load should return valid metrics and skip the invalid one
        let metrics = backend.load_all_metrics().expect("Should load metrics");
        assert_eq!(metrics.len(), 2, "Should load 2 valid metrics, skipping the invalid one");

        // Verify we got the expected metrics
        let device_ids: Vec<_> = metrics.iter().map(|m| m.device_id.as_str()).collect();
        assert!(device_ids.contains(&"device_1"), "Should have device_1");
        assert!(device_ids.contains(&"device_2"), "Should have device_2");
        assert!(!device_ids.contains(&"device_3"), "Should skip device_3 with invalid type");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_timestamp_fallback() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert a valid metric
        backend.upsert_metric_value("device_1", "metric_1", &MetricType::Float(0.0), now)
            .expect("Should upsert");

        // Insert a metric with invalid timestamp.
        //
        // A-4 LOAD-BEARING: must set value_type='Float' + value_real=456.78
        // explicitly. The v007 column default is value_type='legacy'; without
        // an explicit override here the row would be skipped by load_all_metrics
        // (A-4 architecture.md:182 contract: legacy rows surface as Ok(None) /
        // are filtered from the in-memory cache until the next poll UPSERT
        // replaces them). The timestamp-fallback path under test would never
        // fire and the test would silently pass for the wrong reason.
        //
        // DO NOT remove the value_type / value_real columns from this INSERT
        // without first verifying that the post-load assertion still exercises
        // the `parse_from_rfc3339 → Err → Utc::now() fallback` path. The right
        // alternative is to use `upsert_metric_value` (which populates the
        // typed columns automatically) but the test specifically needs the
        // raw-SQL path to inject the bad timestamp string.
        {
            let conn = backend.pool.checkout(Duration::from_secs(5))
                .expect("Should checkout");
            let now_rfc3339 = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, created_at, updated_at, value_real, value_type) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![
                    "device_2",
                    "metric_2",
                    "456.78",
                    "Float",
                    "not-a-valid-rfc3339-timestamp",  // Invalid timestamp
                    &now_rfc3339,
                    &now_rfc3339,
                    456.78_f64,
                    "Float",
                ],
            ).expect("Should insert invalid timestamp");
        }

        // Load should return both metrics, with timestamp fallback for the second
        let metrics = backend.load_all_metrics().expect("Should load metrics");
        assert_eq!(metrics.len(), 2, "Should load both metrics");

        // Find the metric with fallback timestamp
        let metric_2 = metrics.iter().find(|m| m.device_id == "device_2")
            .expect("Should find device_2 metric");

        // Verify timestamp is approximately now (within 5 seconds)
        let now_utc = chrono::Utc::now();
        let time_diff = now_utc.signed_duration_since(metric_2.timestamp);
        assert!(time_diff.num_seconds() < 5, "Fallback timestamp should be approximately now");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_type_validation() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert metrics with all valid types
        backend.upsert_metric_value("device_1", "float_metric", &MetricType::Float(0.0), now)
            .expect("Should insert float");
        backend.upsert_metric_value("device_2", "int_metric", &MetricType::Int(0), now)
            .expect("Should insert int");
        backend.upsert_metric_value("device_3", "bool_metric", &MetricType::Bool(false), now)
            .expect("Should insert bool");
        backend.upsert_metric_value("device_4", "string_metric", &MetricType::String(String::new()), now)
            .expect("Should insert string");

        // Load all metrics - should succeed for all types
        let metrics = backend.load_all_metrics().expect("Should load metrics");
        assert_eq!(metrics.len(), 4, "Should load 4 metrics with different types");

        // Verify type information is preserved
        let types: std::collections::HashSet<_> = metrics.iter()
            .map(|m| format!("{:?}", m.data_type))
            .collect();
        assert_eq!(types.len(), 4, "Should have all 4 different types");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_multiple_devices_orphan_detection() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert 20 metrics across 10 devices
        for device_id in 1..=10 {
            for metric_id in 1..=2 {
                backend.upsert_metric_value(
                    &format!("device_{}", device_id),
                    &format!("metric_{}", metric_id),
                    &MetricType::Float(0.0),
                    now,
                ).expect("Should insert");
            }
        }

        // Load should return all metrics
        let metrics = backend.load_all_metrics().expect("Should load metrics");
        assert_eq!(metrics.len(), 20, "Should load all 20 metrics");

        // Group by device to verify organization
        let mut devices: std::collections::HashSet<_> = std::collections::HashSet::new();
        for metric in &metrics {
            devices.insert(&metric.device_id);
        }
        assert_eq!(devices.len(), 10, "Should have metrics from 10 devices");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_large_dataset() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert 100 metrics across 20 devices
        for i in 1..=100 {
            backend.upsert_metric_value(
                &format!("device_{}", (i % 20) + 1),
                &format!("metric_{}", i),
                &MetricType::Float(0.0),
                now,
            ).expect("Should upsert");
        }

        // Load all metrics should succeed
        let metrics = backend.load_all_metrics().expect("Should load metrics");
        assert_eq!(metrics.len(), 100, "Should load all 100 metrics");

        // Verify distribution across devices
        let mut device_counts: std::collections::HashMap<_, u32> = std::collections::HashMap::new();
        for metric in &metrics {
            *device_counts.entry(&metric.device_id).or_insert(0) += 1;
        }

        // Should have 20 devices (most with 5 metrics)
        assert_eq!(device_counts.len(), 20, "Should have 20 devices");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_restore_partial_failure() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert 100 metrics
        for i in 1..=100 {
            backend.upsert_metric_value(
                &format!("device_{}", (i % 10) + 1),
                &format!("metric_{}", i),
                &MetricType::Float(0.0),
                now,
            ).expect("Should upsert");
        }

        // Insert 10 metrics with invalid data_type
        {
            let conn = backend.pool.checkout(Duration::from_secs(5))
                .expect("Should checkout");
            let now_rfc3339 = chrono::Utc::now().to_rfc3339();
            for i in 1..=10 {
                conn.execute(
                    "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![
                        "device_bad",
                        &format!("bad_metric_{}", i),
                        "123.45",
                        "invalid_type",
                        &now_rfc3339,
                        &now_rfc3339,
                        &now_rfc3339,
                    ],
                ).expect("Should insert");
            }
        }

        let metrics = backend.load_all_metrics().expect("Should load metrics");
        // Should get 100 valid metrics, with 10 invalid ones skipped during load
        assert_eq!(metrics.len(), 100, "Should load 100 valid metrics, skipping 10 invalid");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_graceful_degradation_on_database_not_found() {
        // This test verifies that creating a backend with a non-existent database works
        let path = "/tmp/opcgw_test_nonexistent_db.db";
        // Ensure the file doesn't exist
        let _ = fs::remove_file(path);

        // Create backend for non-existent database
        let backend = SqliteBackend::new(path).expect("Should create backend and schema");

        // Load should return empty vec since no metrics were inserted
        let metrics = backend.load_all_metrics().expect("Should load from new database");
        assert_eq!(metrics.len(), 0, "New database should have no metrics");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn test_load_all_metrics_with_mixed_data_types() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert 100 metrics with mixed types
        for i in 1..=100 {
            let metric_type = match i % 4 {
                0 => MetricType::Float(0.0),
                1 => MetricType::Int(0),
                2 => MetricType::Bool(false),
                _ => MetricType::String(String::new()),
            };
            backend.upsert_metric_value(
                &format!("device_{}", (i % 10) + 1),
                &format!("metric_{}", i),
                &metric_type,
                now,
            ).expect("Should upsert");
        }

        // Load should handle all mixed types
        let metrics = backend.load_all_metrics().expect("Should load metrics");
        assert_eq!(metrics.len(), 100, "Should load all 100 metrics");

        // Verify type counts
        let mut type_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for metric in &metrics {
            let type_str = format!("{:?}", metric.data_type);
            *type_counts.entry(type_str).or_insert(0) += 1;
        }

        // Should have approximately 25 of each type
        assert_eq!(type_counts.len(), 4, "Should have 4 different types");
        for (type_str, count) in &type_counts {
            assert_eq!(*count, 25, "Should have 25 metrics of type {}: got {}", type_str, count);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_graceful_degradation_performance() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");
        let now = SystemTime::now();

        // Insert 500 metrics with some parse errors
        for i in 1..=500 {
            let device_num = (i % 20) + 1;
            backend.upsert_metric_value(
                &format!("device_{}", device_num),
                &format!("metric_{}", i),
                &MetricType::Float(0.0),
                now,
            ).expect("Should upsert");
        }

        // Measure load time
        let start = std::time::Instant::now();
        let metrics = backend.load_all_metrics().expect("Should load metrics");
        let elapsed = start.elapsed();

        assert_eq!(metrics.len(), 500, "Should load all 500 metrics");
        assert!(elapsed.as_secs() < 5, "Load should complete in <5 seconds (was {:?})", elapsed);

        let _ = fs::remove_file(&path);
    }

    // ===== Story 2-5a: Historical Data Pruning Tests =====

    #[test]
    fn test_prune_calculates_cutoff_correctly() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let now = Utc::now();
        let old_ts = now - chrono::Duration::days(100);
        let recent_ts = now - chrono::Duration::days(10);

        // Insert old and recent metrics
        backend.append_metric_history("device_1", "temperature", &MetricType::Float(0.0), old_ts.into())
            .expect("Should append old metric");
        backend.append_metric_history("device_1", "temperature", &MetricType::Float(0.0), recent_ts.into())
            .expect("Should append recent metric");

        // Prune with 90-day retention (should delete old but not recent)
        let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        conn.execute(
            "UPDATE retention_config SET retention_days = 90 WHERE data_type = 'metric_history'",
            [],
        ).expect("Should update retention_config");
        drop(conn);

        let deleted = backend.prune_metric_history().expect("Should prune");
        assert_eq!(deleted, 1, "Should delete 1 old row (AC#1)");

        // Verify old metric was deleted, recent was preserved
        let _metrics = backend.load_all_metrics().expect("Should load metrics");
        // Note: load_all_metrics loads from metric_values, not metric_history
        // So we need to verify via direct database query
        let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_history",
            [],
            |row| row.get(0),
        ).expect("Should count");
        assert_eq!(count, 1, "Should have 1 remaining metric (recent one)");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_skips_null_timestamps() {
        // NOTE: Schema enforces NOT NULL on timestamp column, so this test documents
        // that NULL timestamps cannot occur in practice. The prune implementation still
        // includes "AND timestamp IS NOT NULL" check per AC#2 as a safety guardrail.
        // This test verifies the query logic would work if NULL were possible.
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let now = Utc::now();
        let old_ts = now - chrono::Duration::days(100);
        let _safe_ts_str = format!("{}Z", old_ts.format("%Y-%m-%dT%H:%M:%S%.3f"));

        // Insert metrics with old timestamps
        for i in 0..10 {
            backend.append_metric_history(
                &format!("device_{}", i),
                "temperature",
                &MetricType::Float(0.0),
                old_ts.into()
            ).expect("Should append");
        }

        // Prune should delete old rows
        let deleted = backend.prune_metric_history().expect("Should prune");
        assert_eq!(deleted, 10, "Should delete all old rows");

        let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        let count: i32 = conn.query_row(
            "SELECT COUNT(*) FROM metric_history",
            [],
            |row| row.get(0),
        ).expect("Should count");
        assert_eq!(count, 0, "All rows should be deleted");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_empty_table() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        // Prune empty table (AC#7: empty table graceful no-op)
        let deleted = backend.prune_metric_history().expect("Should prune");
        assert_eq!(deleted, 0, "Should return 0 for empty table (AC#7)");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_reads_retention_from_config() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let now = Utc::now();
        let old_ts = now - chrono::Duration::days(5);
        let recent_ts = now - chrono::Duration::days(2);

        // Insert old and recent metrics
        backend.append_metric_history("device_1", "temperature", &MetricType::Float(0.0), old_ts.into())
            .expect("Should append");
        backend.append_metric_history("device_1", "humidity", &MetricType::Float(0.0), recent_ts.into())
            .expect("Should append");

        // Set retention to 3 days
        let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");
        conn.execute(
            "UPDATE retention_config SET retention_days = 3 WHERE data_type = 'metric_history'",
            [],
        ).expect("Should update");
        drop(conn);

        // Prune should read fresh retention_days from config (not cached) (AC#2)
        let deleted = backend.prune_metric_history().expect("Should prune");
        assert_eq!(deleted, 1, "Should delete 1 row older than 3 days (AC#2)");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_prune_respects_interval() {
        // This test verifies ChirpstackPoller.check_and_execute_prune() respects interval
        // Since check_and_execute_prune is in chirpstack.rs, test structure is in that module
        // This is a placeholder to document the expected behavior (AC#1)
        // AC#1: Returns early if (Instant::now() - last_prune_time) < prune_interval_minutes * 60
    }

    #[test]
    fn test_prune_performance_1m_rows() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        // Insert 1M rows with mixed timestamps
        let now = Utc::now();
        let conn = backend.pool.checkout(Duration::from_secs(5)).expect("Should checkout");

        // Begin transaction for performance
        conn.execute("BEGIN TRANSACTION", []).expect("Should begin");

        for i in 0..1_000_000 {
            let device_num = (i % 100) + 1;
            let metric_num = (i % 50) + 1;
            let days_ago = (i % 180) as i64; // Mix of ages, some beyond 90-day retention

            let ts = if i % 2 == 0 {
                // Half are old (beyond 90 days)
                now - chrono::Duration::days(days_ago + 100)
            } else {
                // Half are recent
                now - chrono::Duration::days(days_ago)
            };

            let ts_str = format!("{}Z", ts.format("%Y-%m-%dT%H:%M:%S%.3f"));

            conn.execute(
                "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    &format!("device_{}", device_num),
                    &format!("metric_{}", metric_num),
                    "23.5",
                    "Float",
                    ts_str,
                    ts_str
                ],
            ).expect("Should insert");

            // Commit periodically for performance
            if i % 10_000 == 0 && i > 0 {
                conn.execute("COMMIT", []).expect("Should commit");
                conn.execute("BEGIN TRANSACTION", []).expect("Should begin");
            }
        }

        conn.execute("COMMIT", []).expect("Should commit");
        drop(conn);

        // Measure prune performance (AC#6: should complete in <30 seconds)
        let start = std::time::Instant::now();
        let deleted = backend.prune_metric_history().expect("Should prune");
        let elapsed = start.elapsed();

        // Verify deletion count: roughly 750K should be deleted
        // (500K "old" half beyond 100 days + ~250K "recent" half between 90-179 days)
        assert!(deleted > 700_000, "Should delete ~750K rows, got {}", deleted);
        assert!(deleted < 800_000, "Should delete ~750K rows, got {}", deleted);

        // Verify performance (AC#6: <30 seconds)
        assert!(elapsed.as_secs() < 30, "Prune should complete in <30s for 1M rows (was {:?})", elapsed);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_enqueue_command_basic() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let cmd = Command {
            id: 0,
            device_id: "device_123".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "set_mode".to_string(),
            parameters: serde_json::json!({"mode": "auto"}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: "hash_abc123".to_string(),
            chirpstack_result_id: None,
        };

        let id = backend.enqueue_command(cmd).expect("Should enqueue command");
        assert_eq!(id, 1, "First command should get ID 1");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_enqueue_command_increments_ids() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        for i in 1..=5 {
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };

            let id = backend.enqueue_command(cmd).expect("Should enqueue");
            assert_eq!(id, i as u64, "Command {} should get ID {}", i, i);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_dequeue_command_fifo() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        for i in 1..=3 {
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).expect("Should enqueue");
        }

        let cmd1 = backend.dequeue_command().expect("Should dequeue").expect("Should have command");
        assert_eq!(cmd1.id, 1);

        let cmd2 = backend.dequeue_command().expect("Should dequeue").expect("Should have command");
        assert_eq!(cmd2.id, 2);

        let cmd3 = backend.dequeue_command().expect("Should dequeue").expect("Should have command");
        assert_eq!(cmd3.id, 3);

        let cmd4 = backend.dequeue_command().expect("Should dequeue");
        assert!(cmd4.is_none(), "Should be no more commands");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_dequeue_command_empty() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let cmd = backend.dequeue_command().expect("Should dequeue from empty");
        assert!(cmd.is_none(), "Empty queue should return None");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_dequeue_command_only_pending() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let cmd1 = Command {
            id: 0,
            device_id: "device_1".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "cmd".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: "hash_1".to_string(),
            chirpstack_result_id: None,
        };

        let cmd2 = Command {
            id: 0,
            device_id: "device_2".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "cmd".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Sent,
            error_message: None,
            command_hash: "hash_2".to_string(),
            chirpstack_result_id: None,
        };

        backend.enqueue_command(cmd1).expect("Should enqueue");
        backend.enqueue_command(cmd2).expect("Should enqueue");

        let dequeued = backend.dequeue_command().expect("Should dequeue").expect("Should have command");
        assert_eq!(dequeued.id, 1, "Should dequeue first (Pending) command");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_list_commands_filter_by_device_id() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        for i in 1..=3 {
            let device_id = if i <= 2 { "device_a" } else { "device_b" };
            let cmd = Command {
                id: 0,
                device_id: device_id.to_string(),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).expect("Should enqueue");
        }

        let filter = CommandFilter {
            device_id: Some("device_a".to_string()),
            status: None,
            command_name_contains: None,
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).expect("Should list commands");
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|c| c.device_id == "device_a"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_list_commands_filter_by_status() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        for i in 1..=3 {
            let status = if i == 1 { CommandStatus::Sent } else { CommandStatus::Pending };
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).expect("Should enqueue");
        }

        let filter = CommandFilter {
            device_id: None,
            status: Some(CommandStatus::Pending),
            command_name_contains: None,
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).expect("Should list commands");
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|c| c.status == CommandStatus::Pending));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_list_commands_filter_by_command_name() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        for (i, name) in ["set_temperature", "set_mode", "get_status"].iter().enumerate() {
            let cmd = Command {
                id: 0,
                device_id: "device_1".to_string(),
                metric_id: "temperature".to_string(),
                command_name: name.to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).expect("Should enqueue");
        }

        let filter = CommandFilter {
            device_id: None,
            status: None,
            command_name_contains: Some("set_".to_string()),
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).expect("Should list commands");
        assert_eq!(commands.len(), 2);
        assert!(commands.iter().all(|c| c.command_name.contains("set_")));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_list_commands_multiple_filters() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let cmd1 = Command {
            id: 0,
            device_id: "device_a".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "set_mode".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: "hash_1".to_string(),
            chirpstack_result_id: None,
        };

        let cmd2 = Command {
            id: 0,
            device_id: "device_a".to_string(),
            metric_id: "humidity".to_string(),
            command_name: "set_mode".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Sent,
            error_message: None,
            command_hash: "hash_2".to_string(),
            chirpstack_result_id: None,
        };

        let cmd3 = Command {
            id: 0,
            device_id: "device_b".to_string(),
            metric_id: "temperature".to_string(),
            command_name: "set_mode".to_string(),
            parameters: serde_json::json!({}),
            enqueued_at: chrono::Utc::now(),
            sent_at: None,
            confirmed_at: None,
            status: CommandStatus::Pending,
            error_message: None,
            command_hash: "hash_3".to_string(),
            chirpstack_result_id: None,
        };

        backend.enqueue_command(cmd1).expect("Should enqueue");
        backend.enqueue_command(cmd2).expect("Should enqueue");
        backend.enqueue_command(cmd3).expect("Should enqueue");

        let filter = CommandFilter {
            device_id: Some("device_a".to_string()),
            status: Some(CommandStatus::Pending),
            command_name_contains: None,
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).expect("Should list commands");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].device_id, "device_a");
        assert_eq!(commands[0].status, CommandStatus::Pending);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_queue_depth_empty() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let depth = backend.get_queue_depth().expect("Should get depth");
        assert_eq!(depth, 0);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_queue_depth_pending_only() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        for i in 1..=5 {
            let status = if i > 3 { CommandStatus::Sent } else { CommandStatus::Pending };
            let cmd = Command {
                id: 0,
                device_id: format!("device_{}", i),
                metric_id: "temperature".to_string(),
                command_name: "cmd".to_string(),
                parameters: serde_json::json!({}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status,
                error_message: None,
                command_hash: format!("hash_{}", i),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).expect("Should enqueue");
        }

        let depth = backend.get_queue_depth().expect("Should get depth");
        assert_eq!(depth, 3, "Should count only pending commands");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_enqueue_command_persists() {
        let path = temp_backend_path();
        {
            let backend = SqliteBackend::new(&path).expect("Should create backend");
            let cmd = Command {
                id: 0,
                device_id: "device_123".to_string(),
                metric_id: "temperature".to_string(),
                command_name: "persist_test".to_string(),
                parameters: serde_json::json!({"value": 42}),
                enqueued_at: chrono::Utc::now(),
                sent_at: None,
                confirmed_at: None,
                status: CommandStatus::Pending,
                error_message: None,
                command_hash: "persist_hash".to_string(),
                chirpstack_result_id: None,
            };
            backend.enqueue_command(cmd).expect("Should enqueue");
        }

        // Reopen and verify command persists
        let backend = SqliteBackend::new(&path).expect("Should reopen");
        let filter = CommandFilter {
            device_id: Some("device_123".to_string()),
            status: None,
            command_name_contains: None,
            older_than_days: None,
        };

        let commands = backend.list_commands(&filter).expect("Should list");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].command_name, "persist_test");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_update_gateway_status_persists() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let timestamp = Utc::now();
        let error_count = 5;
        let available = true;

        // Update gateway health status
        backend
            .update_gateway_status(Some(timestamp), error_count, available)
            .expect("Should update gateway status");

        // Read it back and verify
        let (ts, count, avail) = backend
            .get_gateway_health_metrics()
            .expect("Should read gateway health metrics");

        assert!(ts.is_some(), "Timestamp should be present");
        assert_eq!(count, error_count, "Error count should match");
        assert_eq!(avail, available, "Availability should match");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_health_value_handles_null_timestamp() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        // On first startup (no row), should return defaults
        let (ts, count, avail) = backend
            .get_gateway_health_metrics()
            .expect("Should handle missing gateway_status gracefully");

        assert!(ts.is_none(), "Timestamp should be None on first startup");
        assert_eq!(count, 0, "Error count should default to 0");
        assert!(!avail, "Availability should default to false");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_error_count_increments_across_polls() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let timestamp = Utc::now();

        // First poll: 2 errors
        backend
            .update_gateway_status(Some(timestamp), 2, true)
            .expect("Should update");

        // Verify
        let (_, count1, _) = backend
            .get_gateway_health_metrics()
            .expect("Should read");
        assert_eq!(count1, 2, "Error count should be 2");

        // Second poll: 5 errors (cumulative)
        backend
            .update_gateway_status(Some(timestamp), 5, true)
            .expect("Should update");

        // Verify
        let (_, count2, _) = backend
            .get_gateway_health_metrics()
            .expect("Should read");
        assert_eq!(count2, 5, "Error count should be 5 (cumulative)");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_chirpstack_available_flag() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let timestamp = Utc::now();

        // Successful poll
        backend
            .update_gateway_status(Some(timestamp), 0, true)
            .expect("Should update");

        let (_, _, avail1) = backend
            .get_gateway_health_metrics()
            .expect("Should read");
        assert!(avail1, "Should be available after successful poll");

        // Failed poll
        backend
            .update_gateway_status(None, 10, false)
            .expect("Should update");

        let (_, _, avail2) = backend
            .get_gateway_health_metrics()
            .expect("Should read");
        assert!(!avail2, "Should be unavailable after failed poll");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_null_timestamp_preserves_last_successful_poll() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        let timestamp1 = Utc::now();

        // Successful poll with timestamp
        backend
            .update_gateway_status(Some(timestamp1), 0, true)
            .expect("Should update");

        let (ts1, _, _) = backend
            .get_gateway_health_metrics()
            .expect("Should read");
        assert!(ts1.is_some(), "Timestamp should be set");

        // Failed poll with None timestamp (should preserve previous timestamp)
        backend
            .update_gateway_status(None, 1, false)
            .expect("Should update");

        let (ts2, _, _) = backend
            .get_gateway_health_metrics()
            .expect("Should read");
        assert_eq!(ts1, ts2, "Timestamp should be preserved when None is passed");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_cold_start_timestamp_initialization() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("Should create backend");

        // Cold-start scenario 1: First poll succeeds with timestamp
        let timestamp1 = Utc::now();
        backend
            .update_gateway_status(Some(timestamp1), 0, true)
            .expect("Should update on first successful poll");

        let (ts1, count1, avail1) = backend
            .get_gateway_health_metrics()
            .expect("Should read");
        assert!(ts1.is_some(), "Timestamp should be set after successful poll");
        assert_eq!(count1, 0, "Error count should be 0");
        assert!(avail1, "Should be available");

        // Cold-start scenario 2 (new backend): First poll fails (no timestamp update)
        let path2 = temp_backend_path();
        let backend2 = SqliteBackend::new(&path2).expect("Should create second backend");

        backend2
            .update_gateway_status(None, 1, false)
            .expect("Should update on first failed poll");

        let (ts2, count2, avail2) = backend2
            .get_gateway_health_metrics()
            .expect("Should read");
        assert!(ts2.is_none(), "Timestamp should be NULL after failed first poll");
        assert_eq!(count2, 1, "Error count should be 1");
        assert!(!avail2, "Should be unavailable");

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&path2);
    }

    // ===== Story 8-3 AC#1: query_metric_history tests =====

    /// Helper: seed `count` history rows for `(device_id, metric_name)`,
    /// values "v0", "v1", ..., spaced 1 second apart starting at `base_ts`.
    fn seed_history_rows(
        backend: &SqliteBackend,
        device_id: &str,
        metric_name: &str,
        base_ts: std::time::SystemTime,
        count: usize,
        data_type: MetricType,
    ) {
        for i in 0..count {
            let ts = base_ts + Duration::from_secs(i as u64);
            // A-5: encode the iteration sentinel in the typed String payload
            // so existing v0/v1/... assertions can read `row.payload` directly.
            // For non-String data_types, the seed payload is data_type as-is.
            let row_payload = if matches!(data_type, MetricType::String(_)) {
                MetricType::String(format!("v{i}"))
            } else {
                data_type.clone()
            };
            backend
                .batch_write_metrics(vec![crate::storage::BatchMetricWrite {
                    device_id: device_id.to_string(),
                    metric_name: metric_name.to_string(),
                    data_type: row_payload,
                    timestamp: ts,
                }])
                .expect("seed batch_write_metrics");
        }
    }

    #[test]
    fn test_query_metric_history_empty_range() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let t1 = t0 + Duration::from_secs(60);

        // No rows seeded.
        let result = backend
            .query_metric_history("dev1", "moisture", t0, t1, 100)
            .expect("query empty");
        assert!(result.is_empty(), "empty range must return empty Vec");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_query_metric_history_single_row() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        seed_history_rows(&backend, "dev1", "moisture", t0, 1, MetricType::String(String::new()));

        let result = backend
            .query_metric_history(
                "dev1",
                "moisture",
                t0,
                t0 + Duration::from_secs(60),
                100,
            )
            .expect("query single");
        assert_eq!(result.len(), 1, "single seeded row must be returned");
        assert_eq!(result[0].payload, Some(MetricType::String("v0".to_string())));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_query_metric_history_boundary_inclusion_start() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        // Seed exactly at `start`.
        seed_history_rows(&backend, "dev1", "m", t0, 1, MetricType::String(String::new()));

        // start == seed_ts; row should be returned (start is inclusive).
        let result = backend
            .query_metric_history("dev1", "m", t0, t0 + Duration::from_secs(10), 100)
            .expect("query");
        assert_eq!(result.len(), 1, "row at exactly `start` must be returned");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_query_metric_history_boundary_exclusion_end() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        // Seed exactly at `end`.
        seed_history_rows(&backend, "dev1", "m", t0, 1, MetricType::String(String::new()));

        // end == seed_ts; row should NOT be returned (end is exclusive).
        let result = backend
            .query_metric_history(
                "dev1",
                "m",
                t0 - Duration::from_secs(10),
                t0,
                100,
            )
            .expect("query");
        assert_eq!(result.len(), 0, "row at exactly `end` must NOT be returned");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_query_metric_history_max_results_truncates() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        seed_history_rows(&backend, "dev1", "m", t0, 100, MetricType::String(String::new()));

        // max_results = 10
        let result = backend
            .query_metric_history(
                "dev1",
                "m",
                t0,
                t0 + Duration::from_secs(200),
                10,
            )
            .expect("query");
        assert_eq!(result.len(), 10, "max_results must truncate at 10");
        // Earliest 10 timestamps in ASC order
        assert_eq!(result[0].payload, Some(MetricType::String("v0".to_string())), "first row must be earliest seed");
        assert_eq!(result[9].payload, Some(MetricType::String("v9".to_string())), "10th row must be 10th earliest seed");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_query_metric_history_ordering_ascending() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        // Seed in reverse order to defeat any insertion-order optimisation.
        for i in (0..5).rev() {
            let ts = t0 + Duration::from_secs(i as u64);
            backend
                .batch_write_metrics(vec![crate::storage::BatchMetricWrite {
                    device_id: "dev1".to_string(),
                    metric_name: "m".to_string(),
                    data_type: MetricType::String(format!("v{i}")),
                    timestamp: ts,
                }])
                .expect("seed");
        }

        let result = backend
            .query_metric_history("dev1", "m", t0, t0 + Duration::from_secs(60), 100)
            .expect("query");
        assert_eq!(result.len(), 5);
        let timestamps: Vec<std::time::SystemTime> = result.iter().map(|r| r.timestamp).collect();
        for window in timestamps.windows(2) {
            assert!(window[0] <= window[1], "rows must be in timestamp ASC order");
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    #[traced_test]
    fn test_query_metric_history_skips_nan() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

        // A-5: NaN Float rows are rejected at the v008 cross-column CHECK
        // (SQLite stores NaN as NULL in a REAL column, which violates
        // `value_type='Float' AND value_real IS NOT NULL`). The writer
        // returns Storage("CHECK constraint failed: ...") for the row;
        // the writer-side guard is the structural enforcement and the
        // query_metric_history-side row-skip path is therefore unreachable
        // in production for NaN rows. The helper-level schema-drift
        // coverage is pinned by
        // `test_metric_type_from_typed_columns_schema_drift_returns_err`
        // (A-4 IR3 + iter-2 JR8/JR9/JR10/JR11 — exhaustive NaN/Inf/multi-set/
        // value_type-discriminator coverage). This test now pins the
        // writer-side CHECK enforcement.
        let result = backend.batch_write_metrics(vec![crate::storage::BatchMetricWrite {
            device_id: "dev1".to_string(),
            metric_name: "m".to_string(),
            data_type: MetricType::Float(f64::NAN),
            timestamp: t0,
        }]);
        assert!(
            result.is_err(),
            "v008 CHECK must reject NaN Float row at the writer boundary"
        );
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("CHECK constraint failed"),
            "expected CHECK constraint failure on NaN Float; got: {}",
            err_msg
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_query_metric_history_other_device_excluded() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        seed_history_rows(&backend, "dev1", "m", t0, 3, MetricType::String(String::new()));
        seed_history_rows(&backend, "dev2", "m", t0, 3, MetricType::String(String::new()));

        let result = backend
            .query_metric_history("dev1", "m", t0, t0 + Duration::from_secs(60), 100)
            .expect("query");
        assert_eq!(result.len(), 3, "only dev1 rows must be returned");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_query_metric_history_other_metric_excluded() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        seed_history_rows(&backend, "dev1", "moisture", t0, 3, MetricType::String(String::new()));
        seed_history_rows(&backend, "dev1", "temperature", t0, 3, MetricType::String(String::new()));

        let result = backend
            .query_metric_history("dev1", "moisture", t0, t0 + Duration::from_secs(60), 100)
            .expect("query");
        assert_eq!(result.len(), 3, "only moisture rows must be returned");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_query_metric_history_v008_check_blocks_schema_drift_seed() {
        // A-5: the pre-A-3 partial-success "skip unknown data_type" path
        // is now structurally unreachable. The v008 cross-column CHECK
        // constraint (added by A-3 migrations/v008_typed_value_constraints.sql)
        // forbids invalid `(value_type, value_*)` combinations at the
        // table level — any attempt to seed schema-drift via raw SQL
        // gets rejected at INSERT time. This test pins the v008
        // enforcement so a future weakening of the CHECK (or a v009
        // migration that drops it) would fail this test before the
        // query_metric_history skip path could ever fire.
        //
        // Helper-level schema-drift coverage (NaN, multi-set typed columns,
        // unknown value_type discriminator, near-miss case variants,
        // negative-infinity, combined-condition guard ordering) is pinned
        // by `test_metric_type_from_typed_columns_schema_drift_returns_err`.
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let ts_iso = chrono::DateTime::<Utc>::from(t0).to_rfc3339();

        // A-5 P9 iter-1 review fix: cover ALL v008 CHECK arms, not just
        // the Float arm. The prior single-arm test could miss a CHECK
        // regression that weakened only the Bool/Int/String arms.
        for (value_type, extra_col, extra_val) in [
            // (value_type, drift cause)
            ("Float", "value_real", "NULL"),
            ("Int", "value_int", "NULL"),
            ("Bool", "value_bool", "NULL"),
            ("String", "value_text", "NULL"),
        ] {
            let conn = backend
                .pool
                .checkout(Duration::from_secs(5))
                .expect("checkout");
            // Drop the discriminated column (set NULL) — every typed arm
            // requires its discriminated column NOT NULL.
            let sql = format!(
                "INSERT INTO metric_history \
                 (device_id, metric_name, value, data_type, timestamp, created_at, value_type, {}) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, {})",
                extra_col, extra_val
            );
            let result = conn.execute(&sql, params!["dev1", "m", "1.0", value_type, ts_iso, value_type]);
            assert!(
                result.is_err(),
                "v008 cross-column CHECK must reject value_type='{}' with NULL {}",
                value_type, extra_col
            );
            let err_msg = result.unwrap_err().to_string();
            assert!(
                err_msg.contains("CHECK constraint failed"),
                "expected CHECK constraint failure for value_type='{}' + NULL {}; got: {}",
                value_type, extra_col, err_msg
            );
        }

        // Bonus arm: value_type='legacy' with a non-NULL typed column
        // must also fail (the legacy arm requires ALL typed columns NULL).
        let conn = backend
            .pool
            .checkout(Duration::from_secs(5))
            .expect("checkout");
        let result = conn.execute(
            "INSERT INTO metric_history \
             (device_id, metric_name, value, data_type, timestamp, created_at, value_type, value_real) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?6, ?7)",
            params!["dev1", "m", "legacy", "legacy", ts_iso, "legacy", 1.5_f64],
        );
        assert!(
            result.is_err(),
            "v008 cross-column CHECK must reject value_type='legacy' with non-NULL value_real"
        );

        let _ = fs::remove_file(&path);
    }

    /// Story 8-3 AC#3: `set_metric_history_retention_days` overrides the
    /// `INSERT OR IGNORE` migration default (90 days) with the operator's
    /// `[storage].retention_days` value. Verifies the table is updated
    /// idempotently across multiple calls (matches the at-startup contract).
    #[test]
    fn test_set_metric_history_retention_days_writes_retention_config() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");

        // Migration default for metric_history retention is 90 days
        // (v001_initial.sql:128). Verify baseline.
        let baseline: i64 = {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("checkout");
            conn.query_row(
                "SELECT retention_days FROM retention_config WHERE data_type = 'metric_history'",
                [],
                |row| row.get(0),
            )
            .expect("baseline query")
        };
        assert_eq!(baseline, 90, "migration default must be 90 days");

        // Apply operator config — 14 days.
        backend
            .set_metric_history_retention_days(14)
            .expect("set retention");

        let after_first: i64 = {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("checkout");
            conn.query_row(
                "SELECT retention_days FROM retention_config WHERE data_type = 'metric_history'",
                [],
                |row| row.get(0),
            )
            .expect("query after first set")
        };
        assert_eq!(after_first, 14, "retention_config must reflect 14 days");

        // Idempotent re-apply — 30 days. Re-boot semantics: the value should
        // always reflect the latest operator config, not accumulate.
        backend
            .set_metric_history_retention_days(30)
            .expect("set retention");

        let after_second: i64 = {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("checkout");
            conn.query_row(
                "SELECT retention_days FROM retention_config WHERE data_type = 'metric_history'",
                [],
                |row| row.get(0),
            )
            .expect("query after second set")
        };
        assert_eq!(
            after_second, 30,
            "retention_config must reflect last-write 30 days"
        );

        // Confirm there is still only one `metric_history` row (UPSERT, not
        // INSERT-multiply).
        let row_count: i64 = {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("checkout");
            conn.query_row(
                "SELECT COUNT(*) FROM retention_config WHERE data_type = 'metric_history'",
                [],
                |row| row.get(0),
            )
            .expect("row count")
        };
        assert_eq!(
            row_count, 1,
            "UPSERT must not create duplicate metric_history rows"
        );

        let _ = fs::remove_file(&path);
    }

    // ===== Story A-3 (AC#4): writer typed-column population =====
    //
    // The tests below pin the A-3 contract: each writer populates the
    // matching typed column (`value_real` / `value_int` / `value_bool` /
    // `value_text`) + `value_type` (per `MetricType::Display`) based on the
    // `MetricType` payload pattern. Legacy `value` + `data_type` columns are
    // also preserved per A-2-iter1-DEF1 heterogeneous-lexeme staging.

    /// Helper: read typed columns + value_type for a (device_id, metric_name)
    /// row from `metric_values`. Returns `(value_real, value_int, value_bool,
    /// value_text, value_type, legacy_value, legacy_data_type)`.
    #[allow(clippy::type_complexity)]
    fn read_typed_columns(
        path: &str,
        device_id: &str,
        metric_name: &str,
    ) -> (
        Option<f64>,
        Option<i64>,
        Option<i64>,
        Option<String>,
        String,
        String,
        String,
    ) {
        let conn = rusqlite::Connection::open(path).expect("re-open temp DB");
        conn.query_row(
            "SELECT value_real, value_int, value_bool, value_text, value_type, value, data_type \
             FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
            rusqlite::params![device_id, metric_name],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .expect("row must exist")
    }

    #[test]
    fn test_set_metric_populates_typed_columns_float() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");

        backend
            .set_metric("dev1", "temp", MetricType::Float(23.5))
            .expect("set_metric Float");

        let (vr, vi, vb, vt, vty, _legacy_value, legacy_dt) =
            read_typed_columns(&path, "dev1", "temp");
        assert_eq!(vr, Some(23.5), "value_real must carry the f64 payload");
        assert!(vi.is_none() && vb.is_none() && vt.is_none(), "other typed cols must be NULL");
        assert_eq!(vty, "Float", "value_type discriminant");
        assert_eq!(legacy_dt, "Float", "legacy data_type preserved");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_set_metric_populates_typed_columns_int() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");

        backend
            .set_metric("dev1", "counter", MetricType::Int(42))
            .expect("set_metric Int");

        let (vr, vi, vb, vt, vty, _, _) = read_typed_columns(&path, "dev1", "counter");
        assert!(vr.is_none(), "value_real must be NULL");
        assert_eq!(vi, Some(42), "value_int must carry the i64 payload");
        assert!(vb.is_none() && vt.is_none(), "other typed cols must be NULL");
        assert_eq!(vty, "Int");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_set_metric_populates_typed_columns_bool() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");

        backend
            .set_metric("dev1", "on", MetricType::Bool(true))
            .expect("set_metric Bool(true)");
        backend
            .set_metric("dev2", "off", MetricType::Bool(false))
            .expect("set_metric Bool(false)");

        let (_, _, vb_on, _, vty_on, _, _) = read_typed_columns(&path, "dev1", "on");
        assert_eq!(vb_on, Some(1), "Bool(true) → value_bool = 1");
        assert_eq!(vty_on, "Bool");

        let (_, _, vb_off, _, vty_off, _, _) = read_typed_columns(&path, "dev2", "off");
        assert_eq!(vb_off, Some(0), "Bool(false) → value_bool = 0");
        assert_eq!(vty_off, "Bool");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_set_metric_populates_typed_columns_string() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");

        backend
            .set_metric("dev1", "label", MetricType::String("OK".to_string()))
            .expect("set_metric String");

        let (vr, vi, vb, vt, vty, _, _) = read_typed_columns(&path, "dev1", "label");
        assert!(vr.is_none() && vi.is_none() && vb.is_none(), "other typed cols must be NULL");
        assert_eq!(vt.as_deref(), Some("OK"), "value_text must carry the payload");
        assert_eq!(vty, "String");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_upsert_metric_value_populates_typed_columns_all_variants() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let now = SystemTime::now();

        backend
            .upsert_metric_value("dev_f", "m", &MetricType::Float(7.25), now)
            .expect("upsert Float");
        backend
            .upsert_metric_value("dev_i", "m", &MetricType::Int(-100), now)
            .expect("upsert Int");
        backend
            .upsert_metric_value("dev_b", "m", &MetricType::Bool(true), now)
            .expect("upsert Bool");
        backend
            .upsert_metric_value(
                "dev_s",
                "m",
                &MetricType::String("hello".to_string()),
                now,
            )
            .expect("upsert String");

        let (vr, _, _, _, vty_f, _, _) = read_typed_columns(&path, "dev_f", "m");
        assert_eq!(vr, Some(7.25));
        assert_eq!(vty_f, "Float");

        let (_, vi, _, _, vty_i, _, _) = read_typed_columns(&path, "dev_i", "m");
        assert_eq!(vi, Some(-100));
        assert_eq!(vty_i, "Int");

        let (_, _, vb, _, vty_b, _, _) = read_typed_columns(&path, "dev_b", "m");
        assert_eq!(vb, Some(1));
        assert_eq!(vty_b, "Bool");

        let (_, _, _, vt, vty_s, _, _) = read_typed_columns(&path, "dev_s", "m");
        assert_eq!(vt.as_deref(), Some("hello"));
        assert_eq!(vty_s, "String");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_batch_write_metrics_populates_typed_columns_all_variants() {
        use crate::storage::BatchMetricWrite;
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let now = SystemTime::now();

        let batch = vec![
            BatchMetricWrite {
                device_id: "dev_f".into(),
                metric_name: "m".into(),
                data_type: MetricType::Float(1.5),
                timestamp: now,
            },
            BatchMetricWrite {
                device_id: "dev_i".into(),
                metric_name: "m".into(),
                data_type: MetricType::Int(99),
                timestamp: now,
            },
            BatchMetricWrite {
                device_id: "dev_b".into(),
                metric_name: "m".into(),
                data_type: MetricType::Bool(true),
                timestamp: now,
            },
            BatchMetricWrite {
                device_id: "dev_s".into(),
                metric_name: "m".into(),
                data_type: MetricType::String("ok".into()),
                timestamp: now,
            },
        ];
        backend.batch_write_metrics(batch).expect("batch_write_metrics");

        // metric_values typed columns populated
        let (vr, _, _, _, vty_f, leg_v_f, _) = read_typed_columns(&path, "dev_f", "m");
        assert_eq!(vr, Some(1.5));
        assert_eq!(vty_f, "Float");
        // A-5: BatchMetricWrite.value: String retired; the legacy `value`
        // column is now populated with the discriminant string to satisfy
        // NOT NULL until A-7 drops the column at the schema level.
        assert_eq!(leg_v_f, "Float", "post-A-5: legacy `value` column carries the discriminant string");

        let (_, vi, _, _, vty_i, _, _) = read_typed_columns(&path, "dev_i", "m");
        assert_eq!(vi, Some(99));
        assert_eq!(vty_i, "Int");

        let (_, _, vb, _, vty_b, _, _) = read_typed_columns(&path, "dev_b", "m");
        assert_eq!(vb, Some(1));
        assert_eq!(vty_b, "Bool");

        let (_, _, _, vt, vty_s, _, _) = read_typed_columns(&path, "dev_s", "m");
        assert_eq!(vt.as_deref(), Some("ok"));
        assert_eq!(vty_s, "String");

        // metric_history also populated for all 4 variants
        let conn = rusqlite::Connection::open(&path).expect("re-open");
        let mh_count: i32 = conn
            .query_row("SELECT COUNT(*) FROM metric_history", [], |row| row.get(0))
            .expect("count");
        assert_eq!(mh_count, 4, "batch_write_metrics inserts 1 row per variant into metric_history");

        let mh_typed: i32 = conn
            .query_row(
                "SELECT COUNT(*) FROM metric_history WHERE value_type IN ('Float','Int','Bool','String')",
                [],
                |row| row.get(0),
            )
            .expect("count typed");
        assert_eq!(mh_typed, 4, "all metric_history rows carry the typed payload");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_append_metric_history_populates_typed_columns() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let now = SystemTime::now();

        backend
            .append_metric_history("dev_f", "m", &MetricType::Float(2.71), now)
            .expect("append Float");

        let conn = rusqlite::Connection::open(&path).expect("re-open");
        let (vr, vi, vb, vt, vty): (
            Option<f64>,
            Option<i64>,
            Option<i64>,
            Option<String>,
            String,
        ) = conn
            .query_row(
                "SELECT value_real, value_int, value_bool, value_text, value_type \
                 FROM metric_history WHERE device_id='dev_f' AND metric_name='m'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .expect("history row");
        assert_eq!(vr, Some(2.71));
        assert!(vi.is_none() && vb.is_none() && vt.is_none());
        assert_eq!(vty, "Float");

        let _ = fs::remove_file(&path);
    }

    /// AC#9 sanity: counter monotonic reset detection uses the typed payload
    /// (MetricType::Int) on the prev_metric.data_type when available — pinned
    /// by an end-to-end write then read via SqliteBackend.
    #[test]
    fn test_counter_typed_payload_round_trip() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("create backend");
        let now = SystemTime::now();

        backend
            .upsert_metric_value("dev_c", "counter", &MetricType::Int(1000), now)
            .expect("upsert");

        let mv = backend
            .get_metric_value("dev_c", "counter")
            .expect("get")
            .expect("row");

        // get_metric_value reconstructs MetricType from the legacy `data_type`
        // column today — A-4 will rewire it to project from the typed columns.
        // The legacy reconstruction yields Int(0) (zero default from FromStr).
        // The READ path improvement is A-4's job; A-3 just makes the WRITE
        // side populate the typed columns (which a raw SQL query confirms).
        match mv.data_type {
            MetricType::Int(_) => {}
            other => panic!("expected MetricType::Int variant, got {:?}", other),
        }

        let conn = rusqlite::Connection::open(&path).expect("re-open");
        let vi: Option<i64> = conn
            .query_row(
                "SELECT value_int FROM metric_values WHERE device_id='dev_c' AND metric_name='counter'",
                [],
                |row| row.get(0),
            )
            .expect("query");
        assert_eq!(vi, Some(1000), "A-3 writer must populate typed value_int with the real i64 payload");

        let _ = fs::remove_file(&path);
    }

    // ===== Story A-4 tests: typed-column reader projection =====
    //
    // These tests pin the contract that `get_metric_value` / `get_metric` /
    // `load_all_metrics` build a payload-bearing `MetricType` from the v007
    // typed columns (`value_real` / `value_int` / `value_bool` / `value_text`)
    // discriminated by `value_type`. Legacy rows (`value_type='legacy'`) are
    // surfaced as `Ok(None)` from `get_metric_value` / `get_metric` (mapping
    // transitively to `BadDataUnavailable` via `OpcUa::get_value`) and skipped
    // silently by `load_all_metrics`.

    #[test]
    fn test_get_metric_value_returns_typed_float_payload() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        let now = SystemTime::now();
        backend
            .batch_write_metrics(vec![BatchMetricWrite {
                device_id: "dev1".to_string(),
                metric_name: "temp".to_string(),
                data_type: MetricType::Float(23.5),
                timestamp: now,
            }])
            .expect("seed");
        let mv = backend
            .get_metric_value("dev1", "temp")
            .expect("get_metric_value")
            .expect("Some(MetricValue)");
        assert_eq!(mv.data_type, MetricType::Float(23.5),
            "A-4: get_metric_value must project value_real → MetricType::Float(payload)");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_metric_value_returns_typed_int_payload() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        let now = SystemTime::now();
        backend
            .batch_write_metrics(vec![BatchMetricWrite {
                device_id: "dev1".to_string(),
                metric_name: "counter".to_string(),
                data_type: MetricType::Int(42),
                timestamp: now,
            }])
            .expect("seed");
        let mv = backend
            .get_metric_value("dev1", "counter")
            .expect("get_metric_value")
            .expect("Some(MetricValue)");
        assert_eq!(mv.data_type, MetricType::Int(42));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_metric_value_returns_typed_bool_payload() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        let now = SystemTime::now();
        backend
            .batch_write_metrics(vec![BatchMetricWrite {
                device_id: "dev1".to_string(),
                metric_name: "online".to_string(),
                data_type: MetricType::Bool(true),
                timestamp: now,
            }])
            .expect("seed");
        let mv = backend
            .get_metric_value("dev1", "online")
            .expect("get_metric_value")
            .expect("Some(MetricValue)");
        assert_eq!(mv.data_type, MetricType::Bool(true));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_metric_value_returns_typed_string_payload() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        let now = SystemTime::now();
        backend
            .batch_write_metrics(vec![BatchMetricWrite {
                device_id: "dev1".to_string(),
                metric_name: "status".to_string(),
                data_type: MetricType::String("OK".to_string()),
                timestamp: now,
            }])
            .expect("seed");
        let mv = backend
            .get_metric_value("dev1", "status")
            .expect("get_metric_value")
            .expect("Some(MetricValue)");
        assert_eq!(mv.data_type, MetricType::String("OK".to_string()));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_metric_value_legacy_row_returns_none() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        // Seed a legacy row directly via raw SQL — value_type='legacy',
        // all typed columns NULL (pre-Epic-A shape).
        {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("checkout");
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, created_at, updated_at, value_type) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params!["dev_legacy", "temp", "Float", "Float", &now, &now, &now, "legacy"],
            )
            .expect("seed legacy row");
        }
        let result = backend
            .get_metric_value("dev_legacy", "temp")
            .expect("get_metric_value");
        assert!(
            result.is_none(),
            "A-4: legacy rows must surface as Ok(None) so OpcUa::get_value maps to BadDataUnavailable per architecture.md:182"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_metric_returns_typed_payload_for_each_variant() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        let now = SystemTime::now();
        backend
            .batch_write_metrics(vec![
                BatchMetricWrite {
                    device_id: "d1".to_string(),
                    metric_name: "f".to_string(),
                    data_type: MetricType::Float(1.5),
                    timestamp: now,
                },
                BatchMetricWrite {
                    device_id: "d1".to_string(),
                    metric_name: "i".to_string(),
                    data_type: MetricType::Int(7),
                    timestamp: now,
                },
                BatchMetricWrite {
                    device_id: "d1".to_string(),
                    metric_name: "b".to_string(),
                    data_type: MetricType::Bool(false),
                    timestamp: now,
                },
                BatchMetricWrite {
                    device_id: "d1".to_string(),
                    metric_name: "s".to_string(),
                    data_type: MetricType::String("hi".to_string()),
                    timestamp: now,
                },
            ])
            .expect("seed");
        assert_eq!(
            backend.get_metric("d1", "f").expect("get_metric"),
            Some(MetricType::Float(1.5))
        );
        assert_eq!(
            backend.get_metric("d1", "i").expect("get_metric"),
            Some(MetricType::Int(7))
        );
        assert_eq!(
            backend.get_metric("d1", "b").expect("get_metric"),
            Some(MetricType::Bool(false))
        );
        assert_eq!(
            backend.get_metric("d1", "s").expect("get_metric"),
            Some(MetricType::String("hi".to_string()))
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_get_metric_legacy_row_returns_none() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("checkout");
            let now = chrono::Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, created_at, updated_at, value_type) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params!["dev_l", "m", "Float", "Float", &now, &now, &now, "legacy"],
            )
            .expect("seed legacy row");
        }
        assert!(
            backend.get_metric("dev_l", "m").expect("get_metric").is_none(),
            "A-4: get_metric must return Ok(None) for legacy rows"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_skips_legacy_rows() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        let now = SystemTime::now();
        // Two typed rows.
        backend
            .batch_write_metrics(vec![
                BatchMetricWrite {
                    device_id: "d1".to_string(),
                    metric_name: "f".to_string(),
                    data_type: MetricType::Float(1.5),
                    timestamp: now,
                },
                BatchMetricWrite {
                    device_id: "d1".to_string(),
                    metric_name: "i".to_string(),
                    data_type: MetricType::Int(7),
                    timestamp: now,
                },
            ])
            .expect("seed typed");
        // Two legacy rows.
        {
            let conn = backend.pool.checkout(Duration::from_secs(5)).expect("checkout");
            let now_rfc = chrono::Utc::now().to_rfc3339();
            for (dev, met) in [("d_legacy_a", "x"), ("d_legacy_b", "y")] {
                conn.execute(
                    "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, created_at, updated_at, value_type) \
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                    rusqlite::params![dev, met, "Float", "Float", &now_rfc, &now_rfc, &now_rfc, "legacy"],
                )
                .expect("seed legacy");
            }
        }
        let all = backend.load_all_metrics().expect("load_all_metrics");
        assert_eq!(
            all.len(),
            2,
            "A-4: load_all_metrics must skip legacy rows; got {:?}",
            all.iter().map(|m| (m.device_id.as_str(), m.metric_name.as_str())).collect::<Vec<_>>()
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_load_all_metrics_returns_typed_payload() {
        let path = temp_backend_path();
        let backend = SqliteBackend::new(&path).expect("backend");
        let now = SystemTime::now();
        backend
            .batch_write_metrics(vec![BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "f".to_string(),
                data_type: MetricType::Float(9.9),
                timestamp: now,
            }])
            .expect("seed");
        let all = backend.load_all_metrics().expect("load_all_metrics");
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].data_type, MetricType::Float(9.9));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_metric_type_from_typed_columns_schema_drift_returns_err() {
        // A-4 iter-1 IR3: cover all 4 discriminated arms (NULL-column drift) +
        // Unknown value_type + multi-non-NULL drift (iter-1 IR2) + Bool out-of-
        // range (IR8) + Float NaN/Inf (IR9). A regression that drops any of
        // the defensive guards would be caught by the corresponding case here.

        // -- (1) discriminated-column-NULL drift for all 4 variants --
        for (vt, col) in [
            ("Float",  "value_real"),
            ("Int",    "value_int"),
            ("Bool",   "value_bool"),
            ("String", "value_text"),
        ] {
            let result = metric_type_from_typed_columns(vt, None, None, None, None, "d", "m");
            assert!(result.is_err(),
                "Schema drift must yield Err for value_type='{}' + all-NULL", vt);
            let err = result.unwrap_err().to_string();
            assert!(err.contains(col),
                "error must reference expected column '{}' for value_type='{}'; got: {}", col, vt, err);
            assert!(err.contains("Schema drift") || err.contains("schema drift"),
                "error must indicate schema drift; got: {}", err);
        }

        // -- (2) Unknown value_type --
        let result = metric_type_from_typed_columns("Garbage", None, None, None, None, "d", "m");
        assert!(result.is_err(), "Unknown value_type must yield Err");
        assert!(result.unwrap_err().to_string().contains("Unknown value_type"));

        // -- (2b) iter-2 JR9: near-miss value_type discriminators must also
        // fall through to the Unknown arm. Catches regressions that loosen
        // the exact-equality match (e.g. case-insensitive comparison, trim).
        for near_miss in ["float", "FLOAT", "Float ", " Float", "", "Int32", "boolean", "Legacy"] {
            let result = metric_type_from_typed_columns(near_miss, None, None, None, None, "d", "m");
            assert!(result.is_err(),
                "near-miss value_type '{}' must fall through to Unknown arm", near_miss);
            assert!(result.unwrap_err().to_string().contains("Unknown value_type"),
                "near-miss '{}' must produce Unknown-value_type error", near_miss);
        }

        // -- (3) multi-non-NULL drift (IR2): value_type='Float' but both
        // value_real AND value_int set --
        let result = metric_type_from_typed_columns(
            "Float", Some(1.5), Some(42), None, None, "d", "m",
        );
        assert!(result.is_err(),
            "Multi-non-NULL drift must yield Err — value_real=1.5 AND value_int=42 with value_type='Float'");
        assert!(result.unwrap_err().to_string().contains("unexpected typed columns"));

        // Mirror the multi-non-NULL check on the other 3 variants for symmetry.
        for (vt, vr, vi, vb, vt_text) in [
            ("Int",    Some(1.5_f64), Some(42),     None,      None),
            ("Bool",   None,          Some(42),     Some(1),   None),
            ("String", None,          None,         Some(1),   Some("hi".to_string())),
        ] {
            let result = metric_type_from_typed_columns(vt, vr, vi, vb, vt_text, "d", "m");
            assert!(result.is_err(),
                "Multi-non-NULL drift must yield Err for value_type='{}'", vt);
            assert!(result.unwrap_err().to_string().contains("unexpected typed columns"));
        }

        // -- (4) Bool out-of-range (IR8): value_bool=42 with value_type='Bool' --
        let result = metric_type_from_typed_columns(
            "Bool", None, None, Some(42), None, "d", "m",
        );
        assert!(result.is_err(), "Out-of-range value_bool=42 must yield Err");
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Schema drift") && err.contains("out-of-range"),
            "JR8 phrasing: error for out-of-range value_bool must start with 'Schema drift' + mention 'out-of-range'; got: {}", err);

        // -- (5) Float NaN/Inf (IR9 + iter-2 JR10): cover the full
        // non-finite f64 closed enum — NaN, +Inf, -Inf. JR10: iter-1 IR9
        // test omitted NEG_INFINITY. A regression that narrowed the guard
        // to e.g. `f.is_nan() || f == f64::INFINITY` (forgetting -Inf)
        // would slip through without this case. JR8 harmonized phrasing:
        // assert on "Schema drift" prefix instead of the old "Non-finite
        // value_real" lead.
        for non_finite in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let result = metric_type_from_typed_columns(
                "Float", Some(non_finite), None, None, None, "d", "m",
            );
            assert!(result.is_err(),
                "Non-finite f64 value_real={} must yield Err", non_finite);
            let err = result.unwrap_err().to_string();
            assert!(err.contains("Schema drift") && err.contains("non-finite"),
                "JR8 phrasing: error for f64={} must start with 'Schema drift' + mention 'non-finite'; got: {}",
                non_finite, err);
        }

        // -- (5b) iter-2 JR11: combined-condition cases — multi-set drift
        // paired with out-of-range/NaN/Inf. The guard ordering checks
        // multi-set FIRST then range/finiteness; this pin confirms a row
        // that satisfies BOTH conditions surfaces the multi-set error
        // (documented two-pass triage). A regression that swapped the
        // ordering would change the surfaced error type.
        let result = metric_type_from_typed_columns(
            "Bool", None, Some(7), Some(42), None, "d", "m",
        );
        assert!(result.is_err(),
            "Combined multi-set (value_int=7) + Bool out-of-range (value_bool=42) must yield Err");
        assert!(result.unwrap_err().to_string().contains("unexpected typed columns"),
            "Combined-condition error must surface multi-set FIRST (guard ordering pin)");

        let result = metric_type_from_typed_columns(
            "Float", Some(f64::NAN), Some(99), None, None, "d", "m",
        );
        assert!(result.is_err(),
            "Combined multi-set (value_int=99) + Float NaN (value_real=NaN) must yield Err");
        assert!(result.unwrap_err().to_string().contains("unexpected typed columns"),
            "Combined-condition error must surface multi-set FIRST (guard ordering pin)");

        // -- (6) Sanity: well-formed inputs SUCCEED on every arm --
        assert_eq!(
            metric_type_from_typed_columns("Float", Some(1.5), None, None, None, "d", "m").unwrap(),
            Some(MetricType::Float(1.5))
        );
        assert_eq!(
            metric_type_from_typed_columns("Int", None, Some(42), None, None, "d", "m").unwrap(),
            Some(MetricType::Int(42))
        );
        assert_eq!(
            metric_type_from_typed_columns("Bool", None, None, Some(1), None, "d", "m").unwrap(),
            Some(MetricType::Bool(true))
        );
        assert_eq!(
            metric_type_from_typed_columns("Bool", None, None, Some(0), None, "d", "m").unwrap(),
            Some(MetricType::Bool(false))
        );
        assert_eq!(
            metric_type_from_typed_columns("String", None, None, None, Some("hi".to_string()), "d", "m").unwrap(),
            Some(MetricType::String("hi".to_string()))
        );
        assert_eq!(
            metric_type_from_typed_columns("legacy", None, None, None, None, "d", "m").unwrap(),
            None
        );

        // -- (7) iter-2 JR3: legacy arm symmetry — a 'legacy' row with any
        // orphaned typed column must yield Err (not silently return Ok(None)).
        for (vr, vi, vb, vt) in [
            (Some(1.5_f64), None,         None,      None),
            (None,          Some(42_i64), None,      None),
            (None,          None,         Some(1),   None),
            (None,          None,         None,      Some("orphan".to_string())),
        ] {
            let result = metric_type_from_typed_columns("legacy", vr, vi, vb, vt, "d", "m");
            assert!(result.is_err(),
                "JR3: 'legacy' row with orphaned typed column must yield Err (not silently skip)");
            let err = result.unwrap_err().to_string();
            assert!(err.contains("Schema drift") && err.contains("'legacy'"),
                "JR3: legacy-drift error must carry 'Schema drift' prefix and value_type='legacy'; got: {}", err);
        }
    }

    // ── Story C-6: application config CRUD unit tests ────────────────────

    fn make_app(id: &str, name: &str) -> ChirpStackApplications {
        ChirpStackApplications {
            application_id: id.to_string(),
            application_name: name.to_string(),
            device_list: vec![],
        }
    }

    fn make_device(id: &str, name: &str) -> ChirpstackDevice {
        ChirpstackDevice {
            device_id: id.to_string(),
            device_name: name.to_string(),
            read_metric_list: vec![],
            device_command_list: None,
        }
    }

    fn make_metric(cs_name: &str, m_name: &str) -> ReadMetric {
        ReadMetric {
            chirpstack_metric_name: cs_name.to_string(),
            metric_name: m_name.to_string(),
            metric_type: OpcMetricTypeConfig::Float,
            metric_unit: None,
        }
    }

    fn make_command(name: &str) -> DeviceCommandCfg {
        DeviceCommandCfg {
            command_name: name.to_string(),
            command_id: 1,
            command_confirmed: false,
            command_port: 10,
            command_class: None,
        }
    }

    #[test]
    fn test_c6_insert_application_roundtrip() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        let app = make_app("app-1", "My App");
        b.insert_application(&app).unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].application_id, "app-1");
        assert_eq!(loaded[0].application_name, "My App");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_update_application() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "Old Name")).unwrap();
        b.update_application(&make_app("app-1", "New Name")).unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        assert_eq!(loaded[0].application_name, "New Name");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_update_application_missing_returns_err() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        let err = b.update_application(&make_app("ghost", "X")).unwrap_err();
        assert!(err.to_string().contains("no row"), "expected no-row error, got: {err}");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_delete_application_cascade() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        b.insert_metric("app-1", "dev-1", &make_metric("temp", "Temperature")).unwrap();
        b.insert_command("app-1", "dev-1", &make_command("reboot")).unwrap();
        b.delete_application("app-1").unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        assert!(loaded.is_empty(), "cascade delete must remove all rows");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_insert_device_roundtrip() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Sensor")).unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        assert_eq!(loaded[0].device_list.len(), 1);
        assert_eq!(loaded[0].device_list[0].device_id, "dev-1");
        assert_eq!(loaded[0].device_list[0].device_name, "Sensor");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_update_device() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Old")).unwrap();
        b.update_device("app-1", &make_device("dev-1", "New")).unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        assert_eq!(loaded[0].device_list[0].device_name, "New");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_delete_device_cascade() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        b.insert_metric("app-1", "dev-1", &make_metric("temp", "Temperature")).unwrap();
        b.delete_device("app-1", "dev-1").unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        assert!(loaded[0].device_list.is_empty(), "device delete must cascade to metrics");
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_insert_metric_roundtrip() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        let m = ReadMetric {
            chirpstack_metric_name: "temp".to_string(),
            metric_name: "Temperature".to_string(),
            metric_type: OpcMetricTypeConfig::Float,
            metric_unit: Some("°C".to_string()),
        };
        b.insert_metric("app-1", "dev-1", &m).unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        let metrics = &loaded[0].device_list[0].read_metric_list;
        assert_eq!(metrics.len(), 1);
        assert_eq!(metrics[0].chirpstack_metric_name, "temp");
        assert_eq!(metrics[0].metric_name, "Temperature");
        assert_eq!(metrics[0].metric_type, OpcMetricTypeConfig::Float);
        assert_eq!(metrics[0].metric_unit, Some("°C".to_string()));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_update_metric() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        b.insert_metric("app-1", "dev-1", &make_metric("temp", "Temp")).unwrap();
        let updated = ReadMetric {
            chirpstack_metric_name: "temp".to_string(),
            metric_name: "Temperature".to_string(),
            metric_type: OpcMetricTypeConfig::Int,
            metric_unit: Some("K".to_string()),
        };
        b.update_metric("app-1", "dev-1", &updated).unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        let m = &loaded[0].device_list[0].read_metric_list[0];
        assert_eq!(m.metric_name, "Temperature");
        assert_eq!(m.metric_type, OpcMetricTypeConfig::Int);
        assert_eq!(m.metric_unit, Some("K".to_string()));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_delete_metric() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        b.insert_metric("app-1", "dev-1", &make_metric("temp", "T")).unwrap();
        b.delete_metric("app-1", "dev-1", "temp").unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        assert!(loaded[0].device_list[0].read_metric_list.is_empty());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_insert_command_roundtrip() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        let cmd = DeviceCommandCfg {
            command_name: "reboot".to_string(),
            command_id: 42,
            command_confirmed: true,
            command_port: 15,
            command_class: None,
        };
        b.insert_command("app-1", "dev-1", &cmd).unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        let cmds = loaded[0].device_list[0].device_command_list.as_ref().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command_name, "reboot");
        assert_eq!(cmds[0].command_id, 42);
        assert!(cmds[0].command_confirmed);
        assert_eq!(cmds[0].command_port, 15);
        let _ = fs::remove_file(&path);
    }

    /// Story E-0: a non-None `command_class` must round-trip through the SQLite
    /// commands table (migration v011 column + insert/select wiring). Guards the
    /// `?7` bind / `row.get(6)` read against a silent column-mismatch regression.
    #[test]
    fn test_e0_command_class_roundtrip() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        let cmd = DeviceCommandCfg {
            command_name: "Valve".to_string(),
            command_id: 1,
            command_confirmed: true,
            command_port: 10,
            command_class: Some("valve".to_string()),
        };
        b.insert_command("app-1", "dev-1", &cmd).unwrap();

        let loaded = b.load_all_applications_config().unwrap();
        let cmds = loaded[0].device_list[0].device_command_list.as_ref().unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(
            cmds[0].command_class.as_deref(),
            Some("valve"),
            "command_class must round-trip through SQLite"
        );

        // And an update must change it (e.g. clear the class back to None).
        let cleared = DeviceCommandCfg {
            command_class: None,
            ..cmd.clone()
        };
        b.update_command("app-1", "dev-1", &cleared).unwrap();
        let reloaded = b.load_all_applications_config().unwrap();
        let cmds2 = reloaded[0].device_list[0]
            .device_command_list
            .as_ref()
            .unwrap();
        assert_eq!(
            cmds2[0].command_class, None,
            "update_command must persist a cleared command_class"
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_update_command() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        b.insert_command("app-1", "dev-1", &make_command("reboot")).unwrap();
        let updated = DeviceCommandCfg {
            command_name: "reboot".to_string(),
            command_id: 99,
            command_confirmed: true,
            command_port: 20,
            command_class: None,
        };
        b.update_command("app-1", "dev-1", &updated).unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        let cmd = &loaded[0].device_list[0].device_command_list.as_ref().unwrap()[0];
        assert_eq!(cmd.command_id, 99);
        assert!(cmd.command_confirmed);
        assert_eq!(cmd.command_port, 20);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_delete_command() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        b.insert_command("app-1", "dev-1", &make_command("reboot")).unwrap();
        b.delete_command("app-1", "dev-1", "reboot").unwrap();
        let loaded = b.load_all_applications_config().unwrap();
        assert!(loaded[0].device_list[0].device_command_list.is_none());
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_load_all_full_hierarchy() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        // Two apps, each with one device; first device has metric+command
        b.insert_application(&make_app("app-1", "Alpha")).unwrap();
        b.insert_application(&make_app("app-2", "Beta")).unwrap();
        b.insert_device("app-1", &make_device("dev-a", "DevA")).unwrap();
        b.insert_device("app-2", &make_device("dev-b", "DevB")).unwrap();
        b.insert_metric("app-1", "dev-a", &make_metric("cs_temp", "Temperature")).unwrap();
        b.insert_command("app-1", "dev-a", &make_command("reboot")).unwrap();

        let loaded = b.load_all_applications_config().unwrap();
        assert_eq!(loaded.len(), 2);

        let alpha = loaded.iter().find(|a| a.application_id == "app-1").unwrap();
        assert_eq!(alpha.device_list.len(), 1);
        assert_eq!(alpha.device_list[0].read_metric_list.len(), 1);
        assert!(alpha.device_list[0].device_command_list.is_some());

        let beta = loaded.iter().find(|a| a.application_id == "app-2").unwrap();
        assert_eq!(beta.device_list.len(), 1);
        assert!(beta.device_list[0].read_metric_list.is_empty());
        assert!(beta.device_list[0].device_command_list.is_none());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_metric_type_roundtrip_all_variants() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        b.insert_device("app-1", &make_device("dev-1", "Dev")).unwrap();
        for (cs_name, m_type) in [
            ("m_bool", OpcMetricTypeConfig::Bool),
            ("m_int", OpcMetricTypeConfig::Int),
            ("m_float", OpcMetricTypeConfig::Float),
            ("m_string", OpcMetricTypeConfig::String),
        ] {
            let m = ReadMetric {
                chirpstack_metric_name: cs_name.to_string(),
                metric_name: cs_name.to_string(),
                metric_type: m_type.clone(),
                metric_unit: None,
            };
            b.insert_metric("app-1", "dev-1", &m).unwrap();
        }
        let loaded = b.load_all_applications_config().unwrap();
        let metrics = &loaded[0].device_list[0].read_metric_list;
        assert_eq!(metrics.len(), 4);
        let by_name: std::collections::HashMap<_, _> =
            metrics.iter().map(|m| (m.chirpstack_metric_name.as_str(), &m.metric_type)).collect();
        assert_eq!(by_name["m_bool"], &OpcMetricTypeConfig::Bool);
        assert_eq!(by_name["m_int"], &OpcMetricTypeConfig::Int);
        assert_eq!(by_name["m_float"], &OpcMetricTypeConfig::Float);
        assert_eq!(by_name["m_string"], &OpcMetricTypeConfig::String);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_c6_duplicate_application_returns_err() {
        let path = temp_backend_path();
        let b = SqliteBackend::new(&path).unwrap();
        b.insert_application(&make_app("app-1", "App")).unwrap();
        let err = b.insert_application(&make_app("app-1", "Dup")).unwrap_err();
        assert!(err.to_string().contains("UNIQUE") || err.to_string().contains("app-1"),
            "expected PK constraint error, got: {err}");
        let _ = fs::remove_file(&path);
    }

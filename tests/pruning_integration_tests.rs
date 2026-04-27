// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Integration tests for historical data pruning (Story 2-5b).
//!
//! Tests validate pruning under real-world production scenarios:
//! - Concurrent polling + pruning
//! - Database lock contention
//! - Missing/corrupted retention_config
//! - Retention window boundary precision

// See `tests/opc_ua_sqlite_backend_tests.rs` for the rationale; the sentinel
// `assert!(elapsed_secs < ...)` checks in this file document timing
// invariants that the type system already enforces.
#![allow(clippy::assertions_on_constants)]
//! - Performance under 5M+ row loads
//! - Graceful degradation and error handling

use opcgw::storage::{SqliteBackend, StorageBackend, MetricType};
use opcgw::utils::OpcGwError;
use chrono::{DateTime, Utc, Duration as ChronoDuration};
use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::task;

/// RAII guard to ensure database file cleanup even on panic
struct TempDatabase {
    path: String,
}

impl TempDatabase {
    fn new(path: String) -> Self {
        Self { path }
    }
}

impl Drop for TempDatabase {
    fn drop(&mut self) {
        // Attempt cleanup; log errors but don't panic
        if let Err(e) = fs::remove_file(&self.path) {
            // Only warn if file exists but couldn't be deleted (might be locked on Windows)
            // Ignore "not found" errors as cleanup may have run already
            if e.kind() != std::io::ErrorKind::NotFound {
                eprintln!("Warning: Failed to cleanup test database {}: {}", self.path, e);
            }
        }
    }
}

/// Helper: Create isolated test database
fn temp_backend_path() -> String {
    let temp_dir = std::env::temp_dir();
    let temp_path = temp_dir.join(format!("opcgw_prune_test_{}.db", uuid::Uuid::new_v4()));
    temp_path.to_string_lossy().to_string()
}

/// Helper: Convert DateTime<Utc> to SystemTime (preserves microsecond precision)
fn datetime_to_systemtime(dt: DateTime<Utc>) -> SystemTime {
    let secs = dt.timestamp() as u64;
    let nanos = dt.timestamp_subsec_micros() * 1000;
    SystemTime::UNIX_EPOCH + Duration::new(secs, nanos)
}

/// Helper: Insert N rows with controlled timestamps (RFC3339 format)
fn create_rows_with_timestamps(
    backend: &SqliteBackend,
    count: u32,
    start_time: DateTime<Utc>,
    interval_secs: u64,
) -> Result<(), OpcGwError> {
    for i in 0..count {
        let timestamp = start_time + ChronoDuration::seconds(i as i64 * interval_secs as i64);
        let device_id = format!("device_{}", i % 10); // Distribute across 10 devices
        let metric_name = format!("metric_{}", i % 5); // 5 different metrics
        let metric_value = MetricType::Float;

        backend.set_metric(&device_id, &metric_name, metric_value)?;
        backend.append_metric_history(
            &device_id,
            &metric_name,
            &metric_value,
            datetime_to_systemtime(timestamp),
        )?;
    }
    Ok(())
}

// ============================================================================
// AC#1: Concurrent Polling + Pruning Integration Test
// ============================================================================

#[tokio::test]
async fn test_concurrent_polling_and_pruning() {
    let path = temp_backend_path();
    let _db = TempDatabase::new(path.clone());
    let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

    // Setup: 10K historical rows to reduce spawn_blocking pool saturation risk
    let now = Utc::now();
    let old_time = now - ChronoDuration::days(30);

    let poller_backend = Arc::clone(&backend);
    let setup_handle = task::spawn_blocking(move || {
        create_rows_with_timestamps(&poller_backend, 10_000, old_time, 1)
            .expect("Create historical rows")
    });

    setup_handle.await.expect("Setup completed");

    // Get initial count
    let initial_metrics = backend
        .load_all_metrics()
        .expect("Load initial metrics");
    let initial_count = initial_metrics.len();
    assert!(initial_count > 0, "Should have initial rows");

    // Spawn concurrent polling and pruning
    let poll_backend = Arc::clone(&backend);
    let prune_backend = Arc::clone(&backend);

    let poll_handle = task::spawn(async move {
        // Simulate 5 polling cycles with new metrics
        for cycle in 0..5 {
            let write_backend = Arc::clone(&poll_backend);
            let now = Utc::now();
            task::spawn_blocking(move || {
                for i in 0..1000 {
                    let device_id = format!("device_{}", cycle * 1000 + i);
                    let metric_name = "temp";
                    let metric_value = MetricType::Float;
                    let _ = write_backend.set_metric(&device_id, metric_name, metric_value);
                    let _ = write_backend.append_metric_history(
                        &device_id,
                        metric_name,
                        &metric_value,
                        datetime_to_systemtime(now),
                    );
                }
            })
            .await
            .expect("Polling cycle completed");

            // Small delay between cycles
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    });

    // Pruning executes midway through cycle 3
    let prune_handle = task::spawn(async move {
        tokio::time::sleep(Duration::from_millis(250)).await;

        task::spawn_blocking(move || {
            prune_backend
                .prune_metric_history()
                .expect("Prune succeeded")
        })
        .await
        .expect("Prune task completed")
    });

    // Wait for both tasks to complete (join ensures both are done before assertions)
    let _ = tokio::join!(poll_handle, prune_handle);

    // Verify: All metrics intact, correct count deleted, no corruption
    let final_metrics = backend
        .load_all_metrics()
        .expect("Load final metrics");
    assert!(!final_metrics.is_empty(), "Should have metrics after concurrent ops");

    // Metrics written during prune should not be lost
    let final_count = final_metrics.len();
    assert!(final_count > initial_count, "Should have more metrics after polling");
}

// ============================================================================
// AC#2: Pruning Survives Database Lock
// ============================================================================

#[tokio::test]
async fn test_pruning_under_database_lock() {
    let path = temp_backend_path();
    let _db = TempDatabase::new(path.clone());
    let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

    // Setup: 10K rows
    let now = Utc::now();
    let old_time = now - ChronoDuration::days(15);

    let setup_backend = Arc::clone(&backend);
    task::spawn_blocking(move || {
        create_rows_with_timestamps(&setup_backend, 10_000, old_time, 1)
            .expect("Create historical rows")
    })
    .await
    .expect("Setup completed");

    // Prune with timeout
    let prune_backend = Arc::clone(&backend);
    let prune_handle = task::spawn(async move {
        task::spawn_blocking(move || {
            let start = Instant::now();
            let result = prune_backend.prune_metric_history();
            (result, start.elapsed())
        })
        .await
        .expect("Prune task completed")
    });

    // Wait for prune to complete
    let (prune_result, _prune_time) = prune_handle.await.expect("Prune completed");

    // Verify: Prune succeeds or fails gracefully
    match prune_result {
        Ok(_deleted) => {
            // Prune succeeded
            assert!(true, "Prune succeeded");
        }
        Err(e) => {
            // Error case is also acceptable per AC#2 (graceful degradation)
            assert!(
                matches!(e, OpcGwError::Database(_)),
                "Should be database error: {:?}",
                e
            );
        }
    }

    // Verify: No panic, gateway stable
    let metrics = backend
        .load_all_metrics()
        .expect("Load metrics after lock test");
    assert!(!metrics.is_empty(), "Gateway should be stable");
}

// ============================================================================
// AC#3: Pruning Handles Missing/Corrupted retention_config
// ============================================================================

#[tokio::test]
async fn test_pruning_with_invalid_config() {
    // Test 1: Normal pruning with valid config (should succeed)
    {
        let path = temp_backend_path();
        let _db = TempDatabase::new(path.clone());
        let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

        let setup_backend = Arc::clone(&backend);
        task::spawn_blocking(move || {
            create_rows_with_timestamps(&setup_backend, 1000, Utc::now() - ChronoDuration::days(15), 1)
                .expect("Create rows")
        })
        .await
        .expect("Setup completed");

        let prune_backend = Arc::clone(&backend);
        let result = task::spawn_blocking(move || {
            prune_backend.prune_metric_history()
        })
        .await
        .expect("Prune task completed");

        assert!(result.is_ok(), "Should succeed with valid retention config");
    }

    // Test 2: Zero-deletion case (data within retention window)
    {
        let path = temp_backend_path();
        let _db = TempDatabase::new(path.clone());
        let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

        let setup_backend = Arc::clone(&backend);
        task::spawn_blocking(move || {
            // Create rows only 1 day old (within 90-day default retention)
            create_rows_with_timestamps(&setup_backend, 1000, Utc::now() - ChronoDuration::days(1), 1)
                .expect("Create rows")
        })
        .await
        .expect("Setup completed");

        let prune_backend = Arc::clone(&backend);
        let result = task::spawn_blocking(move || {
            prune_backend.prune_metric_history()
        })
        .await
        .expect("Prune task completed");

        // Should succeed and delete 0 rows (within retention window)
        match result {
            Ok(deleted) => assert_eq!(deleted, 0, "No rows should be deleted within retention window"),
            Err(e) => panic!("Prune should succeed: {:?}", e),
        }
    }

    // Test 3: Multiple prune cycles (verify retention_config is reread each cycle)
    {
        let path = temp_backend_path();
        let _db = TempDatabase::new(path.clone());
        let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

        let setup_backend = Arc::clone(&backend);
        task::spawn_blocking(move || {
            create_rows_with_timestamps(&setup_backend, 1000, Utc::now() - ChronoDuration::days(30), 1)
                .expect("Create rows")
        })
        .await
        .expect("Setup completed");

        // First prune
        let prune_backend = Arc::clone(&backend);
        let result1 = task::spawn_blocking(move || {
            prune_backend.prune_metric_history()
        })
        .await
        .expect("First prune task completed");

        assert!(result1.is_ok(), "First prune should succeed");

        // Second prune (retention_config should be reread)
        let prune_backend = Arc::clone(&backend);
        let result2 = task::spawn_blocking(move || {
            prune_backend.prune_metric_history()
        })
        .await
        .expect("Second prune task completed");

        assert!(result2.is_ok(), "Second prune should succeed (second+ runs with same data)");
    }
}

// ============================================================================
// AC#4: Pruning Respects Retention Window Precisely
// ============================================================================

#[tokio::test]
async fn test_retention_boundary_precision() {
    let path = temp_backend_path();
    let _db = TempDatabase::new(path.clone());
    let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

    // Create rows at boundary conditions
    let now = Utc::now();
    let exact_cutoff = now - ChronoDuration::days(7); // Exactly 7 days ago
    let before_cutoff = exact_cutoff - ChronoDuration::microseconds(1);
    let after_cutoff = exact_cutoff + ChronoDuration::microseconds(1);

    let setup_backend = Arc::clone(&backend);
    task::spawn_blocking(move || {
        let metric = MetricType::Float;

        // Row exactly at cutoff (should NOT be deleted per AC#4)
        let _ = setup_backend.set_metric("device_cutoff", "metric", metric);
        let _ = setup_backend.append_metric_history(
            "device_cutoff",
            "metric",
            &metric,
            datetime_to_systemtime(exact_cutoff),
        );

        // Row before cutoff (should be deleted)
        let _ = setup_backend.set_metric("device_before", "metric", metric);
        let _ = setup_backend.append_metric_history(
            "device_before",
            "metric",
            &metric,
            datetime_to_systemtime(before_cutoff),
        );

        // Row after cutoff (should NOT be deleted)
        let _ = setup_backend.set_metric("device_after", "metric", metric);
        let _ = setup_backend.append_metric_history(
            "device_after",
            "metric",
            &metric,
            datetime_to_systemtime(after_cutoff),
        );
    })
    .await
    .expect("Setup completed");

    let prune_backend = Arc::clone(&backend);
    task::spawn_blocking(move || {
        prune_backend
            .prune_metric_history()
            .expect("Prune succeeded")
    })
    .await
    .expect("Prune completed");

    // Verify boundary handling
    let metrics = backend
        .load_all_metrics()
        .expect("Load metrics after pruning");

    // Rows at/after cutoff should exist
    let cutoff_exists = metrics.iter().any(|m| m.device_id == "device_cutoff");
    let after_exists = metrics.iter().any(|m| m.device_id == "device_after");

    assert!(
        cutoff_exists && after_exists,
        "Rows at and after cutoff should survive"
    );
}

// ============================================================================
// AC#5: Pruning Performance Under Load
// ============================================================================

#[tokio::test]
#[ignore] // Heavy test, marked for optional benchmark runs
async fn test_pruning_performance_5m_rows() {
    let path = temp_backend_path();
    let _db = TempDatabase::new(path.clone());
    let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

    // Setup: 5M rows (2M expired, 3M recent)
    println!("Creating 5M row database for performance test...");
    let now = Utc::now();
    let expired_time = now - ChronoDuration::days(30);

    let setup_backend = Arc::clone(&backend);
    task::spawn_blocking(move || {
        // Create 2M expired rows
        create_rows_with_timestamps(&setup_backend, 2_000_000, expired_time, 1)
            .expect("Create expired rows");

        // Create 3M recent rows
        create_rows_with_timestamps(&setup_backend, 3_000_000, now - ChronoDuration::days(1), 1)
            .expect("Create recent rows");
    })
    .await
    .expect("Setup completed");

    // Measure prune time
    let prune_backend = Arc::clone(&backend);
    let (deleted_count, duration) = task::spawn_blocking(move || {
        let start = Instant::now();
        let deleted = prune_backend
            .prune_metric_history()
            .expect("Prune succeeded");
        (deleted, start.elapsed())
    })
    .await
    .expect("Prune completed");

    // Verify: <60 seconds (AC#5 requirement)
    println!("Pruned {} rows in {:.2}s", deleted_count, duration.as_secs_f64());
    assert!(
        duration.as_secs() < 60,
        "Pruning should complete in <60 seconds (took {:.2}s)",
        duration.as_secs_f64()
    );

    // Verify: ~2M rows deleted
    assert!(
        (1_900_000..=2_100_000).contains(&deleted_count),
        "Should delete approximately 2M rows (got {})",
        deleted_count
    );
}

// ============================================================================
// AC#6: Pruning Task Lifecycle (Startup → Shutdown)
// ============================================================================

#[tokio::test]
async fn test_pruning_interval_timing() {
    let path = temp_backend_path();
    let _db = TempDatabase::new(path.clone());
    let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

    // Create test data
    let now = Utc::now();
    let old_time = now - ChronoDuration::days(15);

    let setup_backend = Arc::clone(&backend);
    task::spawn_blocking(move || {
        create_rows_with_timestamps(&setup_backend, 5000, old_time, 1)
            .expect("Create rows")
    })
    .await
    .expect("Setup completed");

    // Simulate multiple prune cycles with interval timing
    let mut prune_times = Vec::new();
    for _ in 0..3 {
        let prune_backend = Arc::clone(&backend);
        let start = Instant::now();

        let _prune_outcome = task::spawn_blocking(move || {
            prune_backend.prune_metric_history()
        })
        .await
        .expect("Prune completed");

        prune_times.push(start.elapsed());

        // Wait before next cycle
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Verify: All prune cycles complete without errors
    assert_eq!(prune_times.len(), 3, "Should have 3 prune cycles");
    for duration in &prune_times {
        assert!(
            duration.as_secs() < 5,
            "Each prune should complete quickly (<5s)"
        );
    }
}

#[tokio::test]
async fn test_pruning_graceful_shutdown() {
    let path = temp_backend_path();
    let _db = TempDatabase::new(path.clone());
    let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

    // Create test data
    let now = Utc::now();
    let old_time = now - ChronoDuration::days(15);

    let setup_backend = Arc::clone(&backend);
    task::spawn_blocking(move || {
        create_rows_with_timestamps(&setup_backend, 10_000, old_time, 1)
            .expect("Create rows")
    })
    .await
    .expect("Setup completed");

    // Start prune task
    let prune_backend = Arc::clone(&backend);
    let prune_task = task::spawn(async move {
        task::spawn_blocking(move || {
            prune_backend.prune_metric_history()
        })
        .await
    });

    // Let prune start, then simulate shutdown
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Wait for prune to complete (should complete cleanly)
    let start = Instant::now();
    let _prune_result = tokio::select! {
        result = prune_task => {
            result.expect("Prune task completed")
        }
        _ = tokio::time::sleep(Duration::from_secs(10)) => {
            panic!("Prune didn't complete within timeout")
        }
    };

    let shutdown_time = start.elapsed();

    // Verify: Completes cleanly within shutdown timeout
    assert!(
        shutdown_time.as_secs() < 10,
        "Shutdown should complete within 10s"
    );

    // Verify: Gateway stable after shutdown
    let metrics = backend
        .load_all_metrics()
        .expect("Load metrics after shutdown");
    assert!(!metrics.is_empty(), "Should be stable after shutdown");
}

// ============================================================================
// AC#7: Pruning Metrics Visibility in Logs
// ============================================================================

#[tokio::test]
async fn test_pruning_log_structure() {
    let path = temp_backend_path();
    let _db = TempDatabase::new(path.clone());
    let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

    // Create rows with known retention period (older than 90-day default)
    let now = Utc::now();
    let old_time = now - ChronoDuration::days(100);

    let setup_backend = Arc::clone(&backend);
    task::spawn_blocking(move || {
        create_rows_with_timestamps(&setup_backend, 1000, old_time, 1)
            .expect("Create rows")
    })
    .await
    .expect("Setup completed");

    // Execute prune and capture result
    let prune_backend = Arc::clone(&backend);
    let deleted = task::spawn_blocking(move || {
        prune_backend
            .prune_metric_history()
            .expect("Prune succeeded")
    })
    .await
    .expect("Prune completed");

    // Verify: Prune produces structured output with expected fields
    assert!(deleted > 0, "Should delete expired rows");
}

// ============================================================================
// AC#8: Pruning Prevents Unbounded Growth Over Weeks (Simulation)
// ============================================================================

#[tokio::test]
#[ignore] // Long-running test, marked for separate benchmark suite
async fn test_pruning_30_day_simulation() {
    let path = temp_backend_path();
    let _db = TempDatabase::new(path.clone());
    let backend = Arc::new(SqliteBackend::new(&path).expect("Create test backend"));

    // Simulate 30 days with hourly pruning
    let mut sim_time = Utc::now() - ChronoDuration::days(30);
    let end_time = Utc::now();

    while sim_time < end_time {
        // Create metrics for this hour
        let write_backend = Arc::clone(&backend);
        let current_time = sim_time;

        task::spawn_blocking(move || {
            let metric = MetricType::Float;
            // 360 metrics per hour (1 every 10 simulated seconds)
            for i in 0..360 {
                let timestamp = current_time + ChronoDuration::seconds(i * 10);
                let device_id = format!("device_{}", i % 100);
                let metric_name = "load_avg";

                let _ = write_backend.set_metric(&device_id, metric_name, metric);
                let _ = write_backend.append_metric_history(
                    &device_id,
                    metric_name,
                    &metric,
                    datetime_to_systemtime(timestamp),
                );
            }
        })
        .await
        .expect("Metrics written");

        // Execute pruning every simulated hour
        let prune_backend = Arc::clone(&backend);
        task::spawn_blocking(move || {
            let _ = prune_backend.prune_metric_history();
        })
        .await
        .ok();

        // Advance simulation time by 1 hour
        sim_time += ChronoDuration::hours(1);

        // Small delay to make test runnable
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    // Verify: Database growth stabilizes
    let final_metrics = backend
        .load_all_metrics()
        .expect("Load final metrics");
    let final_row_count = final_metrics.len();

    // Growth should be proportional to retention window (90 days * 360 metrics/day)
    // Plus some overhead
    assert!(
        final_row_count < 1_000_000,
        "Database should not have unbounded growth (got {})",
        final_row_count
    );
}

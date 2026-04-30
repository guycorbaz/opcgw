// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] [Guy Corbaz]
//
// Story 8-3 AC#4: NFR15 release-build benchmark for `query_metric_history`.
//
// **What this pins:** the latency contract from PRD NFR15 — a single
// `(device_id, metric_name)` history query covering 7 days of poll data
// (~600k rows at 1Hz polling, the realistic worst case for one metric)
// returns within **2 seconds** wall-clock on a Linux host with NVMe-class
// storage. The aggregate row count cited in the spec ("24M rows") is the
// table total across all metric pairs; per-call HistoryRead targets only
// one pair via the composite `(device_id, timestamp)` index.
//
// **How to run** (from the repo root):
//
// ```
// cargo test --release --test opcua_history_bench -- --ignored \
//     bench_history_read_7_day_full_retention
// ```
//
// The test is `#[ignore]` by default because:
//   - Seeding 600k rows takes ~30 s even with batched inserts.
//   - The latency assertion is meaningful only in `--release` builds
//     (debug-build SQLite is ~10× slower).
//   - CI lanes that don't run `cargo test --release` would surface
//     spurious failures otherwise.
//
// On a perf failure, the dev agent has three escape hatches before
// declaring NFR15 violated:
//   (a) `EXPLAIN QUERY PLAN` to confirm the
//       `idx_metric_history_device_timestamp` index is hit.
//   (b) Add a covering index `(device_id, metric_name, timestamp)` if the
//       plan shows a table-scan after the device-id seek.
//   (c) Tune SQLite PRAGMAs (`mmap_size`, `cache_size`).
//
// All three are 1-line patches; the chosen path goes in the story's
// Completion Notes.

use std::sync::Arc;
use std::time::{Duration, Instant};

use opcgw::storage::{
    BatchMetricWrite, ConnectionPool, MetricType, SqliteBackend, StorageBackend,
};
use tempfile::TempDir;

const BENCH_DEVICE_ID: &str = "0000000000000001";
const BENCH_METRIC_NAME: &str = "moisture";
/// 7 days × 24h × 3600s + a handful of edge entries.
const ROW_COUNT: usize = 7 * 24 * 3600;
/// PRD NFR15 latency contract.
const LATENCY_BUDGET_MS: u128 = 2_000;
/// Insert batch size — keeps each `batch_write_metrics` call cheap (one
/// transaction per batch, one prepared statement pass).
const SEED_BATCH_SIZE: usize = 1_000;

#[ignore]
#[test]
fn bench_history_read_7_day_full_retention() {
    // Force Release build for the latency assertion to be meaningful.
    // (Debug builds run SQLite ~10× slower; the budget would be a false
    // alarm.) `cargo test --release --test opcua_history_bench` sets
    // `cfg(not(debug_assertions))`. If the test is invoked without
    // --release, skip the assertion and emit a marker.
    let release = !cfg!(debug_assertions);

    let tmp = TempDir::new().expect("temp");
    let db_path = tmp.path().join("opcgw_history_bench.db");
    let pool = Arc::new(
        ConnectionPool::new(db_path.to_str().expect("utf-8 db path"), 1).expect("create pool"),
    );
    let backend = SqliteBackend::with_pool(pool).expect("backend");

    let base = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);

    // Seed phase
    println!(
        "Seeding {ROW_COUNT} rows in batches of {SEED_BATCH_SIZE} (this takes ~30s)..."
    );
    let seed_start = Instant::now();
    for batch_start in (0..ROW_COUNT).step_by(SEED_BATCH_SIZE) {
        let batch_end = std::cmp::min(batch_start + SEED_BATCH_SIZE, ROW_COUNT);
        let batch: Vec<BatchMetricWrite> = (batch_start..batch_end)
            .map(|i| BatchMetricWrite {
                device_id: BENCH_DEVICE_ID.to_string(),
                metric_name: BENCH_METRIC_NAME.to_string(),
                value: format!("{}.{}", i % 100, i % 10),
                data_type: MetricType::Float,
                timestamp: base + Duration::from_secs(i as u64),
            })
            .collect();
        backend.batch_write_metrics(batch).expect("seed batch");
    }
    let seed_elapsed = seed_start.elapsed();
    println!("Seed complete in {:?}", seed_elapsed);

    // Bench phase: a single 7-day query targeting all rows.
    let query_start = Instant::now();
    let result = backend
        .query_metric_history(
            BENCH_DEVICE_ID,
            BENCH_METRIC_NAME,
            base,
            base + Duration::from_secs(8 * 24 * 3600),
            ROW_COUNT * 2, // generous max_results so no truncation
        )
        .expect("query");
    let elapsed = query_start.elapsed();
    println!("Query of {} rows took {:?}", result.len(), elapsed);

    assert_eq!(
        result.len(),
        ROW_COUNT,
        "must return all seeded rows (no truncation)"
    );
    // Confirm ASC order at endpoints (single pass on .windows would be
    // O(N), unnecessary here; first/last/middle suffice).
    assert!(
        result[0].timestamp <= result[1].timestamp,
        "first two rows must be ASC"
    );
    assert!(
        result[ROW_COUNT - 2].timestamp <= result[ROW_COUNT - 1].timestamp,
        "last two rows must be ASC"
    );

    if release {
        assert!(
            elapsed.as_millis() <= LATENCY_BUDGET_MS,
            "NFR15 violation: 7-day query took {:?} (budget {}ms)",
            elapsed,
            LATENCY_BUDGET_MS
        );
    } else {
        // Review patch P28: a debug-build run silently green-tested the
        // benchmark without verifying NFR15. CI lanes that don't pass
        // `--release` would have shown a green test that did not
        // exercise the latency contract. Hard-panic instead so debug
        // runs are an unambiguous skip rather than a false success.
        panic!(
            "bench_history_read_7_day_full_retention must be invoked with \
             --release. Re-run with `cargo test --release --test opcua_history_bench \
             -- --ignored bench_history_read_7_day_full_retention`. (Debug SQLite \
             runs ~10× slower; the latency budget would either be a false alarm or \
             a meaningless pass.)"
        );
    }
}

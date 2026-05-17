// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story A-5 integration tests — OPC UA HistoryRead value-payload pipeline.
//!
//! Pins the storage-layer contract that a payload-bearing `MetricType`
//! written through `batch_write_metrics` round-trips through
//! `SqliteBackend::query_metric_history` and `build_data_values` with the
//! real measurement intact. Also pins the architecture.md:182 commitment
//! that legacy rows (`value_type='legacy'`, pre-Epic-A schema) surface as
//! `DataValue { value: None, status: BadDataUnavailable }` — the row
//! appears in the response stream (NOT silently dropped).
//!
//! The full end-to-end HistoryRead path against a live OPC UA server is
//! covered by `tests/opcua_history.rs` (Story 8-3 regression suite). A-5's
//! contract for the typed-payload projection is unit-tested via
//! `src/opc_ua_history.rs::tests::test_build_data_values_*`. This file
//! provides the storage-to-Variant round-trip integration tests at the
//! `query_metric_history` boundary.

use opcgw::storage::{BatchMetricWrite, MetricType, SqliteBackend, StorageBackend};
use std::fs;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

struct TempDb {
    path: String,
}

impl TempDb {
    fn new() -> Self {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir
            .join(format!("opcgw_a5_history_test_{}.db", uuid::Uuid::new_v4()))
            .to_string_lossy()
            .to_string();
        Self { path }
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[test]
fn all_four_variants_round_trip_through_history_reader() {
    // Seed each variant via batch_write_metrics (the production poller's
    // path post-A-3), then read back through SqliteBackend::query_metric_history
    // (A-5's new typed-column projection). The payload field must carry
    // the real measurement, not a zero-default discriminant.
    let db = TempDb::new();
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::new(&db.path).expect("create backend"));
    let t0 = SystemTime::now();
    let writes = vec![
        BatchMetricWrite {
            device_id: "d1".to_string(),
            metric_name: "f".to_string(),
            data_type: MetricType::Float(23.5),
            timestamp: t0,
        },
        BatchMetricWrite {
            device_id: "d1".to_string(),
            metric_name: "i".to_string(),
            data_type: MetricType::Int(42),
            timestamp: t0 + Duration::from_millis(1),
        },
        BatchMetricWrite {
            device_id: "d1".to_string(),
            metric_name: "b".to_string(),
            data_type: MetricType::Bool(true),
            timestamp: t0 + Duration::from_millis(2),
        },
        BatchMetricWrite {
            device_id: "d1".to_string(),
            metric_name: "s".to_string(),
            data_type: MetricType::String("OK".to_string()),
            timestamp: t0 + Duration::from_millis(3),
        },
    ];
    backend.batch_write_metrics(writes).expect("seed");

    // Per-metric history reads — each must return its typed payload.
    for (metric_name, expected) in [
        ("f", MetricType::Float(23.5)),
        ("i", MetricType::Int(42)),
        ("b", MetricType::Bool(true)),
        ("s", MetricType::String("OK".to_string())),
    ] {
        let rows = backend
            .query_metric_history(
                "d1",
                metric_name,
                t0 - Duration::from_secs(1),
                t0 + Duration::from_secs(10),
                100,
            )
            .unwrap_or_else(|e| panic!("query_metric_history for '{}' failed: {}", metric_name, e));
        assert_eq!(rows.len(), 1, "metric {} must have 1 history row", metric_name);
        assert_eq!(
            rows[0].payload,
            Some(expected.clone()),
            "A-5: typed payload must survive the SQLite HistoryRead path for {}",
            metric_name
        );
    }
}

#[test]
fn legacy_row_surfaces_as_payload_none_for_bad_data_unavailable_mapping() {
    // architecture.md:182 commitment: pre-Epic-A rows tagged
    // value_type='legacy' must surface as DataValue {value: None,
    // status: BadDataUnavailable} from the HistoryRead pipeline. A-5
    // implements this via query_metric_history returning
    // HistoricalMetricRow { payload: None } — build_data_values maps
    // None to BadDataUnavailable (the unit test in
    // src/opc_ua_history.rs::tests::test_build_data_values_legacy_emits_bad_data_unavailable
    // pins the OPC UA layer side).
    let db = TempDb::new();
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::new(&db.path).expect("create backend"));

    // Seed a legacy row via a separate Connection — value_type='legacy',
    // all typed columns NULL, legacy `value` column also populated with
    // the discriminant (NOT NULL constraint on the v007 schema).
    let row_ts = chrono::Utc::now();
    let row_ts_rfc = row_ts.to_rfc3339();
    {
        let conn = rusqlite::Connection::open(&db.path).expect("open conn");
        conn.execute(
            "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                "d_legacy",
                "temp",
                "Float",
                "Float",
                &row_ts_rfc,
                &row_ts_rfc,
                "legacy",
            ],
        )
        .expect("seed legacy history row");
    }

    let start = SystemTime::from(row_ts - chrono::Duration::seconds(60));
    let end = SystemTime::from(row_ts + chrono::Duration::seconds(60));
    let rows = backend
        .query_metric_history("d_legacy", "temp", start, end, 100)
        .expect("query_metric_history");
    assert_eq!(rows.len(), 1, "legacy row must appear in the response stream (NOT silently dropped)");
    assert!(
        rows[0].payload.is_none(),
        "A-5: legacy row must return payload=None — OpcgwHistoryNodeManagerImpl::build_data_values maps to BadDataUnavailable per architecture.md:182"
    );
}

#[test]
fn mixed_typed_and_legacy_rows_in_one_history_range() {
    // The "real" epic AC#1: a HistoryRead range that straddles the
    // Epic A upgrade window contains BOTH typed rows (post-A-3 writers)
    // AND legacy rows (pre-Epic-A schema). The response must include
    // all of them in timestamp order, with the legacy entries flagged
    // as BadDataUnavailable + NULL Variant.
    let db = TempDb::new();
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::new(&db.path).expect("create backend"));
    let base = chrono::Utc::now() - chrono::Duration::seconds(120);

    // Seed two typed rows (post-A-3) at t+0 and t+60s.
    backend
        .batch_write_metrics(vec![
            BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "moisture".to_string(),
                data_type: MetricType::Float(11.0),
                timestamp: SystemTime::from(base),
            },
            BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "moisture".to_string(),
                data_type: MetricType::Float(13.0),
                timestamp: SystemTime::from(base + chrono::Duration::seconds(60)),
            },
        ])
        .expect("seed typed rows");

    // Seed a legacy row at t+30s (between the two typed rows) via raw SQL.
    {
        let conn = rusqlite::Connection::open(&db.path).expect("open conn");
        let legacy_ts = (base + chrono::Duration::seconds(30)).to_rfc3339();
        let now_ts = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at, value_type) \
             VALUES (?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["d1", "moisture", "Float", "Float", &legacy_ts, &now_ts, "legacy"],
        )
        .expect("seed legacy row mid-range");
    }

    let start = SystemTime::from(base - chrono::Duration::seconds(60));
    let end = SystemTime::from(base + chrono::Duration::seconds(120));
    let rows = backend
        .query_metric_history("d1", "moisture", start, end, 100)
        .expect("query_metric_history");

    assert_eq!(
        rows.len(),
        3,
        "A-5 epic AC#1: legacy row at t+30s must appear in the stream alongside the typed rows at t+0/t+60 — got {:?}",
        rows.iter().map(|r| &r.payload).collect::<Vec<_>>()
    );
    // Rows are returned in timestamp ASC order.
    assert_eq!(rows[0].payload, Some(MetricType::Float(11.0)), "first typed row");
    assert!(rows[1].payload.is_none(), "middle row is legacy — payload must be None");
    assert_eq!(rows[2].payload, Some(MetricType::Float(13.0)), "last typed row");
}

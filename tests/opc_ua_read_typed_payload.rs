// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story A-4 integration tests — OPC UA Read value-payload pipeline.
//!
//! Pins the storage-layer contract that a payload-bearing `MetricType`
//! written through `batch_write_metrics` round-trips through
//! `SqliteBackend::get_metric_value` / `get_metric` / `load_all_metrics`
//! with the real measurement intact. Also pins the architecture.md:182
//! commitment that legacy rows (`value_type='legacy'`, pre-Epic-A schema)
//! surface as `Ok(None)` from the SqliteBackend reader (which
//! `OpcUa::get_value` already maps to `BadDataUnavailable`).
//!
//! The full end-to-end Read path with a live OPC UA server is covered by
//! `tests/opcua_subscription_spike.rs::test_subscription_datavalue_payload_carries_seeded_value`
//! and siblings. The OPC UA Variant projection from `MetricType` is
//! exhaustively unit-tested in `src/opc_ua.rs::tests`.

use opcgw::storage::{BatchMetricWrite, MetricType, SqliteBackend, StorageBackend};
use std::fs;
use std::sync::Arc;
use std::time::SystemTime;

struct TempDb {
    path: String,
}

impl TempDb {
    fn new() -> Self {
        let temp_dir = std::env::temp_dir();
        let path = temp_dir
            .join(format!("opcgw_a4_read_test_{}.db", uuid::Uuid::new_v4()))
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
fn all_four_variants_round_trip_through_sqlite_reader() {
    // Seed each variant via batch_write_metrics (the production poller's
    // path post-A-3), then read back through SqliteBackend::get_metric_value
    // (A-4's new typed-column projection). The data_type field must carry
    // the real payload, not a zero-default discriminant.
    let db = TempDb::new();
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::new(&db.path).expect("create backend"));
    let now = SystemTime::now();
    backend
        .batch_write_metrics(vec![
            BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "f".to_string(),
                value: "23.5".to_string(),
                data_type: MetricType::Float(23.5),
                timestamp: now,
            },
            BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "i".to_string(),
                value: "42".to_string(),
                data_type: MetricType::Int(42),
                timestamp: now,
            },
            BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "b".to_string(),
                value: "1".to_string(),
                data_type: MetricType::Bool(true),
                timestamp: now,
            },
            BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "s".to_string(),
                value: "OK".to_string(),
                data_type: MetricType::String("OK".to_string()),
                timestamp: now,
            },
        ])
        .expect("seed");

    assert_eq!(
        backend
            .get_metric_value("d1", "f")
            .expect("get_metric_value Float")
            .expect("Some")
            .data_type,
        MetricType::Float(23.5),
        "A-4: Float payload must survive the SQLite reader"
    );
    assert_eq!(
        backend
            .get_metric_value("d1", "i")
            .expect("get_metric_value Int")
            .expect("Some")
            .data_type,
        MetricType::Int(42)
    );
    assert_eq!(
        backend
            .get_metric_value("d1", "b")
            .expect("get_metric_value Bool")
            .expect("Some")
            .data_type,
        MetricType::Bool(true)
    );
    assert_eq!(
        backend
            .get_metric_value("d1", "s")
            .expect("get_metric_value String")
            .expect("Some")
            .data_type,
        MetricType::String("OK".to_string())
    );
}

#[test]
fn legacy_row_returns_ok_none_for_bad_data_unavailable_mapping() {
    // architecture.md:182 commitment: pre-Epic-A rows tagged
    // value_type='legacy' must surface as BadDataUnavailable to SCADA
    // clients. A-4 implements this via SqliteBackend::get_metric_value
    // returning Ok(None) for legacy rows — OpcUa::get_value already maps
    // Ok(None) to BadDataUnavailable (the existing branch at line 1493-1509).
    let db = TempDb::new();
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::new(&db.path).expect("create backend"));

    // Seed a legacy row via a separate Connection — value_type='legacy',
    // all typed columns NULL.
    {
        let conn = rusqlite::Connection::open(&db.path).expect("open conn");
        let now_rfc = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, created_at, updated_at, value_type) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params![
                "d_legacy",
                "temp",
                "Float",
                "Float",
                &now_rfc,
                &now_rfc,
                &now_rfc,
                "legacy",
            ],
        )
        .expect("seed legacy row");
    }

    let result = backend
        .get_metric_value("d_legacy", "temp")
        .expect("get_metric_value");
    assert!(
        result.is_none(),
        "A-4: legacy row must return Ok(None) — OpcUa::get_value transitively maps to BadDataUnavailable per architecture.md:182"
    );

    let metric = backend
        .get_metric("d_legacy", "temp")
        .expect("get_metric");
    assert!(
        metric.is_none(),
        "A-4: legacy row must return Ok(None) from get_metric too"
    );
}

#[test]
fn load_all_metrics_skips_legacy_and_returns_typed_rows() {
    // Pins the load_all_metrics partial-success contract: typed rows
    // are returned with their real payload, legacy rows skipped silently.
    let db = TempDb::new();
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::new(&db.path).expect("create backend"));
    let now = SystemTime::now();
    backend
        .batch_write_metrics(vec![
            BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "f".to_string(),
                value: "1.0".to_string(),
                data_type: MetricType::Float(1.0),
                timestamp: now,
            },
            BatchMetricWrite {
                device_id: "d1".to_string(),
                metric_name: "i".to_string(),
                value: "2".to_string(),
                data_type: MetricType::Int(2),
                timestamp: now,
            },
        ])
        .expect("seed typed");

    {
        let conn = rusqlite::Connection::open(&db.path).expect("open conn");
        let now_rfc = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO metric_values (device_id, metric_name, value, data_type, timestamp, created_at, updated_at, value_type) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            rusqlite::params!["d_legacy", "x", "Float", "Float", &now_rfc, &now_rfc, &now_rfc, "legacy"],
        )
        .expect("seed legacy");
    }

    let all = backend.load_all_metrics().expect("load_all_metrics");
    assert_eq!(
        all.len(),
        2,
        "A-4: load_all_metrics must skip the 1 legacy row and return 2 typed rows; got {:?}",
        all.iter()
            .map(|m| (m.device_id.as_str(), m.metric_name.as_str(), &m.data_type))
            .collect::<Vec<_>>()
    );
    // Verify the typed payload survived.
    let f = all.iter().find(|m| m.metric_name == "f").expect("f present");
    assert_eq!(f.data_type, MetricType::Float(1.0));
    let i = all.iter().find(|m| m.metric_name == "i").expect("i present");
    assert_eq!(i.data_type, MetricType::Int(2));
}

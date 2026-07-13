// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2024 Guy Corbaz

//! Regression test for GH #123 — web-configured applications must survive a
//! gateway restart.
//!
//! # The bug
//!
//! `AppConfig.application_list` was sourced purely from `config.toml` via
//! figment. Applications created through the web UI are written to SQLite and
//! applied to the *live* gateway via the config-reload watch channel, but on
//! **restart** the poller, the in-memory storage skeleton, and the OPC UA
//! address space were all rebuilt from the `config.toml` bootstrap **seed** —
//! the SQLite-stored applications were loaded into the watch channel only,
//! never folded back into the construction-time `application_config`. Net
//! effect: every web-created application/device/metric vanished from the
//! running gateway on restart (the data stayed safe in SQLite, but the gateway
//! reverted to whatever `[[application]]` blocks were in the seed). Found
//! during the v2.1.0 rc2 production smoke test on a real NAS deployment.
//!
//! # The fix (what this test guards)
//!
//! At startup `main()` now loads the SQLite applications and folds them into
//! `application_config.application_list` *before* `Storage::new`, the poller,
//! and `OpcUa::new` are constructed — making SQLite authoritative for the
//! application topology across restarts.
//!
//! This test reproduces that contract at the library level (deterministic, no
//! subprocess): seed SQLite with one application via the same backend API the
//! web CRUD handlers use, load a divergent `config.toml` seed, perform the
//! fold, and assert the resolved `application_list` — and the storage skeleton
//! built from it — reflect SQLite, not the seed.

use opcgw::config::{
    AppConfig, ChirpStackApplications, OpcMetricTypeConfig, ReadMetric,
};
use opcgw::storage::{SqliteBackend, Storage};

const APP_ID: &str = "194f12ab-d0ab-4389-a446-f1b3e7152b07";
const DEVICE_ID: &str = "a840414bf185f365";

#[test]
fn sqlite_application_list_overrides_config_toml_seed_on_load() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let backend =
        SqliteBackend::new(db_path.to_str().expect("utf-8 db path")).expect("create backend");

    // --- Seed SQLite with a "web-created" application (operator's real config). ---
    backend
        .insert_application(&ChirpStackApplications {
            application_id: APP_ID.to_string(),
            application_name: "batiments".to_string(),
            device_list: vec![],
        })
        .expect("insert application");
    let metrics = vec![
        ReadMetric {
            metric_name: "Temperature".to_string(),
            chirpstack_metric_name: "TempC_SHT".to_string(),
            metric_type: OpcMetricTypeConfig::Float,
            metric_unit: Some("°C".to_string()),
        },
        ReadMetric {
            metric_name: "Humidite".to_string(),
            chirpstack_metric_name: "Hum_SHT".to_string(),
            metric_type: OpcMetricTypeConfig::Float,
            metric_unit: Some("%".to_string()),
        },
    ];
    backend
        .insert_device_with_metrics(APP_ID, DEVICE_ID, "lsn50-magasin", &metrics, None, false)
        .expect("insert device with metrics");

    // --- load_all_applications_config round-trips the topology (the data
    //     source the #123 fix reads at startup). ---
    let sqlite_apps = backend
        .load_all_applications_config()
        .expect("load applications from SQLite");
    assert_eq!(sqlite_apps.len(), 1, "exactly one application in SQLite");
    assert_eq!(sqlite_apps[0].application_name, "batiments");
    assert_eq!(sqlite_apps[0].device_list.len(), 1);
    let dev = &sqlite_apps[0].device_list[0];
    assert_eq!(dev.device_id, DEVICE_ID);
    assert_eq!(dev.device_name, "lsn50-magasin");
    assert_eq!(dev.read_metric_list.len(), 2, "both metrics round-trip");

    // --- Load the config.toml *seed* (apps Application01/02, devices
    //     device_1..3), then perform the #123 fold. ---
    let mut config =
        AppConfig::from_path("tests/config/config.toml").expect("seed config.toml loads");
    // Precondition: the seed genuinely differs from SQLite, so the assertions
    // below actually prove SQLite won (not that they happen to match).
    assert!(
        config
            .application_list
            .iter()
            .any(|a| a.application_name == "Application01"),
        "the seed config.toml fixture should contain the Application01 seed app"
    );

    // The fix: SQLite is authoritative for application_list across restarts.
    config.application_list = sqlite_apps;

    // After the fold, the runtime topology is the SQLite one, not the seed.
    assert_eq!(config.application_list.len(), 1);
    assert_eq!(config.application_list[0].application_name, "batiments");
    assert!(
        !config
            .application_list
            .iter()
            .any(|a| a.application_name == "Application01"),
        "seed apps must be gone after sourcing application_list from SQLite (#123)"
    );

    // And the storage skeleton built from the folded config (mirroring
    // Storage::new in main after #123) exposes the SQLite device, not the seed
    // devices. Pre-fix, storage was built from the seed → the SQLite device was
    // absent and metric restore orphaned against the seed skeleton.
    let mut storage = Storage::new(&config);
    assert_eq!(
        storage.get_device_name(DEVICE_ID).as_deref(),
        Some("lsn50-magasin"),
        "storage must contain the SQLite-sourced device after the fold"
    );
    assert!(
        storage.get_device("device_1").is_none(),
        "storage must NOT contain the config.toml seed device after the fold (#123)"
    );
}

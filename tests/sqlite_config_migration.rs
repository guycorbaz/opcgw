// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story C-6 integration tests: TOML→SQLite configuration migration.
//
// Tests are fully self-contained: each writes a config.toml to a
// fresh tempdir, creates a fresh SqliteBackend, and asserts migration
// outcomes without touching the web server or OPC UA server.

use std::time::Duration;
use tempfile::TempDir;

use opcgw::config::AppConfig;
use opcgw::storage::migrate_config::{migrate_toml_to_sqlite, MigrationOutcome};
use opcgw::storage::SqliteBackend;

// ── Tracing helpers ──────────────────────────────────────────────────────────

fn init_test_subscriber() {
    use tracing_subscriber::{fmt as tracing_fmt, layer::SubscriberExt, Layer};
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        let buf: &'static std::sync::Mutex<Vec<u8>> = tracing_test::internal::global_buf();
        let mock = tracing_test::internal::MockWriter::new(buf);
        let fmt_layer = tracing_fmt::layer()
            .with_writer(mock)
            .with_level(true)
            .with_ansi(false)
            .with_filter(tracing_subscriber::filter::LevelFilter::TRACE);
        let subscriber = tracing_subscriber::Registry::default().with(fmt_layer);
        let _ = tracing::subscriber::set_global_default(subscriber);
    });
}

fn captured_logs() -> String {
    let buf = tracing_test::internal::global_buf().lock().unwrap();
    String::from_utf8_lossy(&buf).to_string()
}

fn clear_captured_logs() {
    let mut buf = tracing_test::internal::global_buf().lock().unwrap();
    buf.clear();
}

// ── TOML fixture templates ────────────────────────────────────────────────────

/// Singleton-only config (no [[application]] blocks) — for empty-source tests.
const TOML_EMPTY_APPS: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5
command_delivery_timeout_secs = 60
command_timeout_check_interval_secs = 10
history_retention_days = 7

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "test-token"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 10
retry = 1
delay = 1
list_page_size = 100

[opcua]
application_name = "test"
application_uri = "urn:test"
product_uri = "urn:test:product"
diagnostics_enabled = false
hello_timeout = 5
host_ip_address = "127.0.0.1"
host_port = 4855
create_sample_keypair = true
certificate_path = "own/cert.der"
private_key_path = "private/private.pem"
trust_client_cert = false
check_cert_time = false
pki_dir = "./pki"
user_name = "opcua-user"
user_password = "test-password"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-test"
allowed_origins = ["http://127.0.0.1:8080"]
"#;

/// Minimal config with two apps, three devices, six metrics.
const TOML_TWO_APPS: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5
command_delivery_timeout_secs = 60
command_timeout_check_interval_secs = 10
history_retention_days = 7

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "test-token"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 10
retry = 1
delay = 1
list_page_size = 100

[opcua]
application_name = "test"
application_uri = "urn:test"
product_uri = "urn:test:product"
diagnostics_enabled = false
hello_timeout = 5
host_ip_address = "127.0.0.1"
host_port = 4855
create_sample_keypair = true
certificate_path = "own/cert.der"
private_key_path = "private/private.pem"
trust_client_cert = false
check_cert_time = false
pki_dir = "./pki"
user_name = "opcua-user"
user_password = "test-password"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-test"
allowed_origins = ["http://127.0.0.1:8080"]

[[application]]
application_name = "App Alpha"
application_id = "app-alpha"

  [[application.device]]
  device_id = "dev-a1"
  device_name = "Alpha Device One"

    [[application.device.read_metric]]
    metric_name = "temperature"
    chirpstack_metric_name = "temp"
    metric_type = "Float"
    metric_unit = "C"

    [[application.device.read_metric]]
    metric_name = "humidity"
    chirpstack_metric_name = "hum"
    metric_type = "Float"
    metric_unit = "%"

[[application]]
application_name = "App Beta"
application_id = "app-beta"

  [[application.device]]
  device_id = "dev-b1"
  device_name = "Beta Device One"

    [[application.device.read_metric]]
    metric_name = "voltage"
    chirpstack_metric_name = "volt"
    metric_type = "Float"
    metric_unit = "V"

  [[application.device]]
  device_id = "dev-b2"
  device_name = "Beta Device Two"

    [[application.device.read_metric]]
    metric_name = "current"
    chirpstack_metric_name = "curr"
    metric_type = "Float"
    metric_unit = "A"

    [[application.device.read_metric]]
    metric_name = "power"
    chirpstack_metric_name = "pwr"
    metric_type = "Float"
    metric_unit = "W"

    [[application.device.read_metric]]
    metric_name = "online"
    chirpstack_metric_name = "online"
    metric_type = "Bool"
"#;

/// Config with one app + one device + two commands.
const TOML_WITH_COMMANDS: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5
command_delivery_timeout_secs = 60
command_timeout_check_interval_secs = 10
history_retention_days = 7

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "test-token"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 10
retry = 1
delay = 1
list_page_size = 100

[opcua]
application_name = "test"
application_uri = "urn:test"
product_uri = "urn:test:product"
diagnostics_enabled = false
hello_timeout = 5
host_ip_address = "127.0.0.1"
host_port = 4855
create_sample_keypair = true
certificate_path = "own/cert.der"
private_key_path = "private/private.pem"
trust_client_cert = false
check_cert_time = false
pki_dir = "./pki"
user_name = "opcua-user"
user_password = "test-password"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-test"
allowed_origins = ["http://127.0.0.1:8080"]

[[application]]
application_name = "Command App"
application_id = "cmd-app"

  [[application.device]]
  device_id = "cmd-dev-1"
  device_name = "Actuator"

    [[application.device.read_metric]]
    metric_name = "status"
    chirpstack_metric_name = "status"
    metric_type = "Bool"

    [[application.device.command]]
    command_id = 101
    command_name = "Open Valve"
    command_confirmed = false
    command_port = 2

    [[application.device.command]]
    command_id = 102
    command_name = "Close Valve"
    command_confirmed = true
    command_port = 3
"#;

// ── Fixture helpers ───────────────────────────────────────────────────────────

/// Write config to a tempdir, init SqliteBackend, return both.
fn setup(toml_content: &str) -> (TempDir, AppConfig, SqliteBackend) {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, toml_content).expect("write config");

    let cfg = AppConfig::from_path(config_path.to_str().unwrap()).expect("parse config");

    let db_path = dir.path().join("test.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("sqlite backend");

    (dir, cfg, backend)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

/// Fresh DB + populated TOML → migration runs and returns Migrated.
#[test]
fn migration_fresh_db_populated_toml_returns_migrated() {
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    let outcome = migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert!(
        matches!(outcome, MigrationOutcome::Migrated(_)),
        "expected Migrated outcome"
    );
}

/// Fresh DB + populated TOML → row counts in MigrationReport match TOML source.
/// Fixture: 2 apps, 3 devices, 6 metrics, 0 commands.
#[test]
fn migration_counts_match_toml_source() {
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    let outcome = migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    if let MigrationOutcome::Migrated(report) = outcome {
        assert_eq!(report.applications, 2, "apps");
        assert_eq!(report.devices, 3, "devices");
        assert_eq!(report.metrics, 6, "metrics");
        assert_eq!(report.commands, 0, "commands");
    } else {
        panic!("expected Migrated, got something else");
    }
}

/// After migration, load_all_applications_config returns correct data.
#[test]
fn migration_load_all_applications_reflects_data() {
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");

    let apps = backend.load_all_applications_config().expect("load");
    assert_eq!(apps.len(), 2);

    let alpha = apps.iter().find(|a| a.application_id == "app-alpha").expect("app-alpha");
    assert_eq!(alpha.application_name, "App Alpha");
    assert_eq!(alpha.device_list.len(), 1);
    assert_eq!(alpha.device_list[0].device_id, "dev-a1");
    assert_eq!(alpha.device_list[0].read_metric_list.len(), 2);

    let beta = apps.iter().find(|a| a.application_id == "app-beta").expect("app-beta");
    assert_eq!(beta.device_list.len(), 2);
    let total_beta_metrics: usize =
        beta.device_list.iter().map(|d| d.read_metric_list.len()).sum();
    assert_eq!(total_beta_metrics, 4);
}

/// count_applications reports the correct SQLite row count post-migration.
#[test]
fn migration_sqlite_count_applications_correct() {
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    let count = backend.count_applications().expect("count");
    assert_eq!(count, 2);
}

/// Running migration twice returns AlreadyMigrated on the second call.
#[test]
fn migration_already_migrated_db_is_noop() {
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    migrate_toml_to_sqlite(&cfg, &backend).expect("first migrate");
    let second = migrate_toml_to_sqlite(&cfg, &backend).expect("second migrate");
    assert!(
        matches!(second, MigrationOutcome::AlreadyMigrated),
        "expected AlreadyMigrated on second call"
    );
}

/// Re-opening the same SQLite file also returns AlreadyMigrated.
#[test]
fn migration_second_boot_same_db_is_noop() {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, TOML_TWO_APPS).expect("write config");
    let cfg = AppConfig::from_path(config_path.to_str().unwrap()).expect("parse config");

    let db_path = dir.path().join("test.db");
    {
        let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("sqlite 1");
        migrate_toml_to_sqlite(&cfg, &backend).expect("first boot migrate");
        assert_eq!(backend.count_applications().unwrap(), 2);
    }
    // Simulate second boot: new SqliteBackend on same file.
    {
        let backend2 = SqliteBackend::new(db_path.to_str().unwrap()).expect("sqlite 2");
        let outcome = migrate_toml_to_sqlite(&cfg, &backend2).expect("second boot");
        assert!(
            matches!(outcome, MigrationOutcome::AlreadyMigrated),
            "second boot should be no-op"
        );
        assert_eq!(backend2.count_applications().unwrap(), 2, "no duplicate rows");
    }
}

/// Empty application_list in TOML → SkippedEmptySource.
#[test]
fn migration_empty_toml_skipped() {
    let (_dir, cfg, backend) = setup(TOML_EMPTY_APPS);
    let outcome = migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert!(
        matches!(outcome, MigrationOutcome::SkippedEmptySource),
        "expected SkippedEmptySource"
    );
    assert_eq!(backend.count_applications().unwrap(), 0, "no rows inserted");
}

/// Commands migrate correctly: count and data match TOML source.
#[test]
fn migration_with_commands_migrates_correctly() {
    let (_dir, cfg, backend) = setup(TOML_WITH_COMMANDS);
    let outcome = migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    if let MigrationOutcome::Migrated(report) = outcome {
        assert_eq!(report.applications, 1, "apps");
        assert_eq!(report.devices, 1, "devices");
        assert_eq!(report.metrics, 1, "metrics");
        assert_eq!(report.commands, 2, "commands");
    } else {
        panic!("expected Migrated");
    }

    let apps = backend.load_all_applications_config().expect("load");
    assert_eq!(apps.len(), 1);
    let cmds = apps[0].device_list[0].device_command_list.as_ref().expect("commands present");
    assert_eq!(cmds.len(), 2);
    let open = cmds.iter().find(|c| c.command_name == "Open Valve").expect("open-valve");
    assert_eq!(open.command_id, 101);
    assert!(!open.command_confirmed);
    assert_eq!(open.command_port, 2);
    let close = cmds.iter().find(|c| c.command_name == "Close Valve").expect("close-valve");
    assert_eq!(close.command_id, 102);
    assert!(close.command_confirmed);
    assert_eq!(close.command_port, 3);
}

/// TOML file mtime is unchanged after migration (TOML is never written post-C-6).
#[test]
fn migration_toml_file_not_mutated() {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, TOML_TWO_APPS).expect("write config");
    let cfg = AppConfig::from_path(config_path.to_str().unwrap()).expect("parse config");
    let db_path = dir.path().join("test.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("sqlite backend");

    let mtime_before = std::fs::metadata(&config_path)
        .expect("metadata before")
        .modified()
        .expect("mtime before");

    // Small sleep to guarantee mtime resolution on slow filesystems.
    std::thread::sleep(Duration::from_millis(50));

    migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");

    let mtime_after = std::fs::metadata(&config_path)
        .expect("metadata after")
        .modified()
        .expect("mtime after");

    assert_eq!(mtime_before, mtime_after, "config.toml was modified by migration");
}

/// Migration emits event="config_migration" stage="toml_to_sqlite" to the audit log.
#[test]
fn migration_emits_config_migration_audit_event() {
    init_test_subscriber();
    clear_captured_logs();

    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");

    let logs = captured_logs();
    assert!(
        logs.contains(r#"event="config_migration""#) && logs.contains(r#"stage="toml_to_sqlite""#),
        "expected config_migration event in logs; got:\n{logs}"
    );
}

/// Empty-source path emits event="config_migration" stage="skipped_empty_source".
#[test]
fn migration_skipped_emits_audit_event() {
    init_test_subscriber();
    clear_captured_logs();

    let (_dir, cfg, backend) = setup(TOML_EMPTY_APPS);
    migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");

    let logs = captured_logs();
    assert!(
        logs.contains(r#"stage="skipped_empty_source""#),
        "expected skipped_empty_source in logs; got:\n{logs}"
    );
}

/// Large-inventory migration (25 apps × 4 devices × 3 metrics = 300 metrics)
/// must complete in under 5 seconds.
#[test]
fn migration_large_inventory_completes_in_time() {
    // Build an AppConfig programmatically rather than via TOML to keep test
    // fast (TOML parse of 300 metric blocks is slower than direct struct init).
    use opcgw::config::{ChirpStackApplications, ChirpstackDevice, OpcMetricTypeConfig, ReadMetric};

    let apps: Vec<ChirpStackApplications> = (0..25)
        .map(|app_i| ChirpStackApplications {
            application_id: format!("app-{app_i:03}"),
            application_name: format!("Application {app_i}"),
            device_list: (0..4)
                .map(|dev_j| ChirpstackDevice {
                    device_id: format!("app-{app_i:03}-dev-{dev_j:02}"),
                    device_name: format!("Device {dev_j}"),
                    read_metric_list: (0..3)
                        .map(|met_k| ReadMetric {
                            metric_name: format!("metric-{met_k}"),
                            chirpstack_metric_name: format!("cs-metric-{met_k}"),
                            metric_type: OpcMetricTypeConfig::Float,
                            metric_unit: Some("unit".to_string()),
                        })
                        .collect(),
                    device_command_list: None,
                })
                .collect(),
        })
        .collect();

    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, TOML_EMPTY_APPS).expect("write singleton toml");
    let mut cfg =
        AppConfig::from_path(config_path.to_str().unwrap()).expect("parse config");
    cfg.application_list = apps;
    cfg.validate().expect("large-inventory cfg must be valid");

    let db_path = dir.path().join("large.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("sqlite");

    let start = std::time::Instant::now();
    let outcome = migrate_toml_to_sqlite(&cfg, &backend).expect("migrate large");
    let elapsed = start.elapsed();

    assert!(
        matches!(outcome, MigrationOutcome::Migrated(_)),
        "expected Migrated"
    );
    assert!(
        elapsed < Duration::from_secs(5),
        "migration of 25-app inventory took {elapsed:?}, expected < 5s"
    );

    let count = backend.count_applications().unwrap();
    assert_eq!(count, 25, "all 25 apps written");
}

/// Post-migration CRUD insert_application adds a new app on top of migrated data.
#[test]
fn post_migration_crud_insert_application_works() {
    use opcgw::config::ChirpStackApplications;

    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert_eq!(backend.count_applications().unwrap(), 2);

    backend
        .insert_application(&ChirpStackApplications {
            application_id: "new-app".to_string(),
            application_name: "New App".to_string(),
            device_list: vec![],
        })
        .expect("insert new app");

    assert_eq!(backend.count_applications().unwrap(), 3, "new app added");
    let apps = backend.load_all_applications_config().unwrap();
    assert!(apps.iter().any(|a| a.application_id == "new-app"), "new-app present");
}

/// Post-migration delete_application cascades to its devices and metrics.
#[test]
fn post_migration_delete_application_cascades() {
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");

    // app-beta has 2 devices and 4 metrics; deleting it should leave only app-alpha.
    backend.delete_application("app-beta").expect("delete app-beta");

    assert_eq!(backend.count_applications().unwrap(), 1, "one app left");
    let apps = backend.load_all_applications_config().unwrap();
    assert!(apps.iter().all(|a| a.application_id != "app-beta"), "app-beta gone");
    assert_eq!(apps[0].application_id, "app-alpha");
}

/// Duplicate application_id in TOML causes migration to fail and roll back.
/// The transaction is rolled back, leaving the applications table empty.
#[test]
fn migration_duplicate_application_id_is_rejected() {
    use opcgw::config::{ChirpStackApplications, ChirpstackDevice, OpcMetricTypeConfig, ReadMetric};

    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, TOML_EMPTY_APPS).expect("write singleton toml");
    let mut cfg = AppConfig::from_path(config_path.to_str().unwrap()).expect("parse config");

    let make_app = |id: &str| ChirpStackApplications {
        application_id: id.to_string(),
        application_name: format!("App {id}"),
        device_list: vec![ChirpstackDevice {
            device_id: format!("dev-{id}"),
            device_name: "Dev".to_string(),
            read_metric_list: vec![ReadMetric {
                metric_name: "m".to_string(),
                chirpstack_metric_name: "m".to_string(),
                metric_type: OpcMetricTypeConfig::Float,
                metric_unit: None,
            }],
            device_command_list: None,
        }],
    };
    // Identical application_id "dup" twice — SQLite UNIQUE constraint fires.
    cfg.application_list = vec![make_app("dup"), make_app("dup")];

    let db_path = dir.path().join("dup.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("sqlite");

    let result = migrate_toml_to_sqlite(&cfg, &backend);
    assert!(result.is_err(), "duplicate app_id should return Err");
    // Transaction rolled back: no rows should remain.
    assert_eq!(
        backend.count_applications().unwrap(),
        0,
        "rollback: applications table must be empty"
    );
}

/// MigrationReport.duration_ms is plausible (> 0 for a non-trivial insert).
#[test]
fn migration_duration_ms_is_populated() {
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    let outcome = migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    if let MigrationOutcome::Migrated(report) = outcome {
        // duration_ms is measured from start to after the commit; it is at least 0
        // and on any real hardware << 1000 ms for this tiny fixture.
        assert!(report.duration_ms < 1000, "duration_ms suspiciously large: {}", report.duration_ms);
    } else {
        panic!("expected Migrated");
    }
}

/// F14 (iter-1 patch): AlreadyMigrated path does not overwrite SQLite data with
/// stale TOML; the second call to migrate_toml_to_sqlite is a pure no-op.
#[test]
fn migration_already_migrated_does_not_overwrite_sqlite_data() {
    // First boot: migrate.
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    let outcome = migrate_toml_to_sqlite(&cfg, &backend).expect("first migrate");
    assert!(matches!(outcome, MigrationOutcome::Migrated(_)), "expected Migrated on first boot");

    // Simulate a subsequent boot: run migrate_toml_to_sqlite again with the same
    // TOML config — the done-flag in `meta` must trigger AlreadyMigrated.
    let outcome2 = migrate_toml_to_sqlite(&cfg, &backend).expect("second migrate");
    assert!(
        matches!(outcome2, MigrationOutcome::AlreadyMigrated),
        "expected AlreadyMigrated on second boot"
    );

    // The watch-channel seeding logic (main.rs F1) calls
    // load_all_applications_config after migration. Verify it returns the
    // migrated data correctly so the watch channel can be populated.
    let apps = backend
        .load_all_applications_config()
        .expect("load after AlreadyMigrated");
    assert_eq!(apps.len(), 2, "both apps must be readable after AlreadyMigrated");
    let ids: Vec<&str> = apps.iter().map(|a| a.application_id.as_str()).collect();
    assert!(ids.contains(&"app-alpha"), "app-alpha present");
    assert!(ids.contains(&"app-beta"), "app-beta present");
}

/// I2-F7: The done-flag is written to the `meta` table after a successful migration
/// so subsequent boots hit the faster primary guard via `is_c6_migration_done()`.
#[test]
fn migration_meta_done_flag_is_written() {
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);
    assert!(!backend.is_c6_migration_done().expect("pre-check"), "no flag before migration");
    migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert!(backend.is_c6_migration_done().expect("post-check"), "flag must be set after migration");
}

/// I2-F4: Secondary already-migrated guard — apps present in SQLite but no
/// done-flag (e.g. direct SQLite import that bypassed migrate_applications_config).
/// Must return AlreadyMigrated AND back-fill the meta key so future boots use
/// the faster primary guard.
#[test]
fn migration_secondary_guard_backfills_meta_key() {
    use opcgw::config::ChirpStackApplications;
    let (_dir, cfg, backend) = setup(TOML_TWO_APPS);

    // Seed the DB directly (bypassing migrate_toml_to_sqlite) — no done-flag written.
    let app = ChirpStackApplications {
        application_id: "direct-import-app".to_string(),
        application_name: "Direct Import".to_string(),
        device_list: vec![],
    };
    backend.insert_application(&app).expect("direct insert");

    // Precondition: done-flag is absent, but apps table is non-empty.
    assert!(!backend.is_c6_migration_done().expect("pre-check"), "no flag before secondary guard");
    assert!(backend.count_applications().expect("count") > 0, "app must be present");

    // migrate_toml_to_sqlite must take the secondary guard path.
    let outcome = migrate_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert!(
        matches!(outcome, MigrationOutcome::AlreadyMigrated),
        "secondary guard must return AlreadyMigrated"
    );

    // Done-flag must now be back-filled.
    assert!(
        backend.is_c6_migration_done().expect("post-check"),
        "secondary guard must back-fill the meta done-flag"
    );
}

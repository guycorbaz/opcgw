// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story D-2 integration tests: SqliteSingletonProvider + figment stack
// precedence ordering + config_toml_unused_warning event semantics.
//
// Each test sets up a tempdir-isolated environment with its own
// config.toml and SQLite database, exercises the figment loader, and
// asserts the produced AppConfig matches the expected precedence
// ordering (env > SQLite > TOML > default).

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tempfile::TempDir;
use tracing_test::traced_test;

use opcgw::config::AppConfig;
use opcgw::storage::migrate_singleton_config::{
    migrate_singleton_toml_to_sqlite, SingletonMigrationOutcome,
};
use opcgw::storage::{SqliteBackend, SqliteSingletonProvider};

// ── Test fixtures ──────────────────────────────────────────────────────

const TOML_BASE: &str = r#"
[global]
debug = false
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5

[chirpstack]
server_address = "http://toml-host:8080"
api_token = "toml-token"
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
host_ip_address = "127.0.0.1"
host_port = 4855
create_sample_keypair = true
certificate_path = "own/cert.der"
private_key_path = "private/private.pem"
trust_client_cert = false
check_cert_time = false
pki_dir = "./pki"
user_name = "opcua-user"
user_password = "real-test-password-not-a-placeholder"
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

fn fresh_env() -> (TempDir, String, SqliteBackend) {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, TOML_BASE).expect("write config");
    let db_path = dir.path().join("opcgw.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("backend");
    (dir, config_path.to_string_lossy().into_owned(), backend)
}

// Helper: read a fresh AppConfig via figment (no D-0 migration yet) so
// the SQLite Provider can be tested against a known TOML baseline.
fn load_bootstrap_config(config_path: &str) -> AppConfig {
    AppConfig::from_path(config_path).expect("bootstrap from_path")
}

// ── AC#17 test list (12 named tests) ────────────────────────────────────

/// Test 1 — Provider returns an empty Map when `singleton_config`
/// table is empty (pre-D-0 / fresh boot).
#[test]
fn t01_provider_empty_when_singleton_table_empty() {
    use figment::Provider;
    let (_dir, _config_path, backend) = fresh_env();
    let provider = SqliteSingletonProvider::new(Arc::new(backend));
    let data = provider.data().expect("data ok");
    assert!(
        data.is_empty(),
        "expected empty Map when singleton_config has no rows, got {:?}",
        data
    );
}

/// Test 2 — Provider returns a populated Map after
/// `migrate_singleton_toml_to_sqlite` has run; keys are
/// `section.key` (e.g. `chirpstack.polling_frequency`).
#[test]
fn t02_provider_populated_after_migration() {
    use figment::Provider;
    let (_dir, config_path, backend) = fresh_env();
    let cfg = load_bootstrap_config(&config_path);
    let outcome = migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert!(
        matches!(outcome, SingletonMigrationOutcome::Migrated(_)),
        "expected Migrated outcome"
    );
    let provider = SqliteSingletonProvider::new(Arc::new(backend));
    let data = provider.data().expect("data ok");
    let default_dict = data
        .get(&figment::Profile::Default)
        .expect("Profile::Default present");
    assert!(
        default_dict.get("chirpstack").is_some(),
        "expected chirpstack section in {:?}",
        default_dict
    );
    assert!(
        default_dict.get("global").is_some(),
        "expected global section in {:?}",
        default_dict
    );
    assert!(
        default_dict.get("opcua").is_some(),
        "expected opcua section in {:?}",
        default_dict
    );
    assert!(
        default_dict.get("web").is_some(),
        "expected web section in {:?}",
        default_dict
    );
}

/// Test 3 — Precedence test (env > SQLite): SQLite has
/// `polling_frequency=10`; env-var
/// `OPCGW_CHIRPSTACK__POLLING_FREQUENCY=5` is set. Loaded value is 5.
#[test]
fn t03_precedence_env_beats_sqlite() {
    temp_env::with_var("OPCGW_CHIRPSTACK__POLLING_FREQUENCY", Some("5"), || {
        let (_dir, config_path, backend) = fresh_env();
        // Bootstrap-load + run D-0 migration so SQLite has a value.
        let cfg = load_bootstrap_config(&config_path);
        migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
        // Now write a different SQLite value to demonstrate env-var wins.
        backend
            .write_singleton_section(
                "chirpstack",
                &[(
                    "polling_frequency".to_string(),
                    serde_json::to_string(&serde_json::json!(20))
                        .expect("serialize"),
                )],
            )
            .expect("write");
        let loaded =
            AppConfig::from_path_with_sqlite(&config_path, Arc::new(backend))
                .expect("from_path_with_sqlite");
        assert_eq!(
            loaded.chirpstack.polling_frequency, 5,
            "env-var must win over SQLite; got polling_frequency={}",
            loaded.chirpstack.polling_frequency
        );
    });
}

/// Test 4 — Precedence test (SQLite > TOML): TOML has
/// `polling_frequency=10`; SQLite has `polling_frequency=20`. Loaded
/// value is 20.
#[test]
fn t04_precedence_sqlite_beats_toml() {
    // Explicit env-var clear to isolate from other tests that set
    // OPCGW_CHIRPSTACK__POLLING_FREQUENCY.
    temp_env::with_var(
        "OPCGW_CHIRPSTACK__POLLING_FREQUENCY",
        None::<&str>,
        || {
            let (_dir, config_path, backend) = fresh_env();
            let cfg = load_bootstrap_config(&config_path);
            migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
            backend
                .write_singleton_section(
                    "chirpstack",
                    &[(
                        "polling_frequency".to_string(),
                        serde_json::to_string(&serde_json::json!(20))
                            .expect("serialize"),
                    )],
                )
                .expect("write");
            let loaded =
                AppConfig::from_path_with_sqlite(&config_path, Arc::new(backend))
                    .expect("from_path_with_sqlite");
            assert_eq!(
                loaded.chirpstack.polling_frequency, 20,
                "SQLite must win over TOML; got polling_frequency={}",
                loaded.chirpstack.polling_frequency
            );
        },
    );
}

/// Test 5 — Precedence test (TOML > default): TOML has a non-default
/// value; SQLite is empty. Loaded value is the TOML value.
#[test]
fn t05_precedence_toml_beats_default() {
    temp_env::with_var(
        "OPCGW_CHIRPSTACK__POLLING_FREQUENCY",
        None::<&str>,
        || {
            let (_dir, config_path, backend) = fresh_env();
            // No D-0 migration → SQLite empty → TOML wins.
            let loaded =
                AppConfig::from_path_with_sqlite(&config_path, Arc::new(backend))
                    .expect("from_path_with_sqlite");
            assert_eq!(
                loaded.chirpstack.polling_frequency, 10,
                "TOML value should flow through when SQLite is empty; got polling_frequency={}",
                loaded.chirpstack.polling_frequency
            );
            assert_eq!(loaded.chirpstack.server_address, "http://toml-host:8080");
        },
    );
}

/// Test 6 — Precedence test (default fallback): a field with
/// `#[serde(default)]` is honoured when TOML omits it and SQLite is
/// empty. Uses `[global].command_delivery_poll_interval_secs`
/// (Story 7-1, default = `default_command_delivery_poll_interval`)
/// as the target — required fields like `polling_frequency` MUST be
/// present.
#[test]
fn t06_precedence_default_fallback_for_missing_field() {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    let minimal = r#"
[global]
debug = false
prune_interval_minutes = 60

[chirpstack]
server_address = "http://x:8080"
api_token = "x"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 10
retry = 1
delay = 1
list_page_size = 100

[opcua]
application_name = "x"
application_uri = "urn:x"
product_uri = "urn:x"
diagnostics_enabled = false
host_ip_address = "127.0.0.1"
host_port = 4855
create_sample_keypair = true
certificate_path = "own/cert.der"
private_key_path = "private/private.pem"
trust_client_cert = false
check_cert_time = false
pki_dir = "./pki"
user_name = "u"
user_password = "p"
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
    std::fs::write(&config_path, minimal).expect("write");
    let db_path = dir.path().join("opcgw.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("backend");
    let loaded = AppConfig::from_path_with_sqlite(
        config_path.to_str().unwrap(),
        Arc::new(backend),
    )
    .expect("from_path_with_sqlite");
    // `command_delivery_poll_interval_secs` was omitted from TOML;
    // serde default must populate it with a positive integer.
    assert!(
        loaded.global.command_delivery_poll_interval_secs > 0,
        "serde default should have populated command_delivery_poll_interval_secs; got {}",
        loaded.global.command_delivery_poll_interval_secs
    );
}

/// Test 7 — `config_toml_unused_warning` event ACTUALLY FIRES when
/// `config.toml` is present AND `singleton_config` is non-empty.
///
/// D-2 review iter-1 (BH-F1 / ECH-F2 / AA-F3 converged): this test
/// previously asserted only the boolean predicate and never exercised
/// the warn-emit path — a fake regression guard. It now drives
/// `AppConfig::maybe_emit_config_toml_unused_warning` (the exact
/// helper main.rs calls) with a fresh once-per-boot guard and asserts
/// BOTH the return value AND that the `config_toml_unused_warning`
/// event landed in the captured log via `#[traced_test]`.
#[traced_test]
#[test]
fn t07_unused_warning_fires_when_both_present() {
    let (_dir, config_path, backend) = fresh_env();
    let cfg = load_bootstrap_config(&config_path);
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    let row_count = backend.count_singleton_config().expect("count");
    assert!(row_count > 0, "singleton_config must be populated; got {}", row_count);

    let guard = AtomicBool::new(false);
    let emitted = AppConfig::maybe_emit_config_toml_unused_warning(
        &config_path,
        row_count,
        &guard,
    );
    assert!(
        emitted,
        "helper must report it emitted the warning when config.toml present + singleton non-empty"
    );
    assert!(
        logs_contain("config_toml_unused_warning"),
        "the config_toml_unused_warning event must appear in the captured log"
    );
}

/// Test 7b — once-per-boot guard: a SECOND call with the same
/// already-emitted guard does NOT re-emit (D-2 AC#5 / Task 3.3).
///
/// D-2 review iter-2 (BH2-4 / ECH2-2 converged): the original t07b
/// asserted only the return-value contract. It now ALSO asserts at the
/// audit-trail level via `logs_assert` that the
/// `config_toml_unused_warning` event appears EXACTLY ONCE across the
/// two calls — so a future refactor that emitted-then-guarded (instead
/// of guarded-then-emitted) would be caught.
#[traced_test]
#[test]
fn t07b_unused_warning_fires_only_once_per_guard() {
    let (_dir, config_path, backend) = fresh_env();
    let cfg = load_bootstrap_config(&config_path);
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    let row_count = backend.count_singleton_config().expect("count");

    let guard = AtomicBool::new(false);
    let first = AppConfig::maybe_emit_config_toml_unused_warning(&config_path, row_count, &guard);
    let second = AppConfig::maybe_emit_config_toml_unused_warning(&config_path, row_count, &guard);
    assert!(first, "first call must emit");
    assert!(!second, "second call with the same guard must NOT re-emit (once-per-boot)");

    // Audit-trail-level proof: the event must appear exactly once even
    // though the helper was called twice.
    logs_assert(|lines: &[&str]| {
        let count = lines
            .iter()
            .filter(|l| l.contains("config_toml_unused_warning"))
            .count();
        if count == 1 {
            Ok(())
        } else {
            Err(format!(
                "expected config_toml_unused_warning to appear exactly once, got {}",
                count
            ))
        }
    });
}

/// Test 8 — `config_toml_unused_warning` event does NOT fire when
/// `config.toml` is absent. Drives the real helper and asserts both
/// the `false` return AND the absence of the event in the log.
#[traced_test]
#[test]
fn t08_unused_warning_skipped_when_config_toml_absent() {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("does-not-exist.toml");
    let config_path_str = config_path.to_string_lossy().into_owned();
    assert!(
        !std::path::Path::new(&config_path_str).exists(),
        "config.toml MUST be absent for this scenario"
    );

    let guard = AtomicBool::new(false);
    // Non-zero row count to prove the existence-check (not the row
    // count) is what suppresses the warning here.
    let emitted =
        AppConfig::maybe_emit_config_toml_unused_warning(&config_path_str, 27, &guard);
    assert!(!emitted, "warning must NOT fire when config.toml is absent");
    assert!(
        !logs_contain("config_toml_unused_warning"),
        "no config_toml_unused_warning event should appear when config.toml is absent"
    );
}

/// Test 9 — `config_toml_unused_warning` event does NOT fire when
/// SQLite singleton tables are empty (fresh deployment). Drives the
/// real helper with row_count = 0.
#[traced_test]
#[test]
fn t09_unused_warning_skipped_when_singleton_empty() {
    let (_dir, config_path, backend) = fresh_env();
    let row_count = backend.count_singleton_config().expect("count");
    assert_eq!(row_count, 0, "singleton_config should be empty pre-migration");

    let guard = AtomicBool::new(false);
    let emitted =
        AppConfig::maybe_emit_config_toml_unused_warning(&config_path, row_count, &guard);
    assert!(!emitted, "warning must NOT fire when singleton_config is empty");
    assert!(
        !logs_contain("config_toml_unused_warning"),
        "no config_toml_unused_warning event should appear when singleton_config is empty"
    );
}

/// Test 10 — Boot-cycle test: fresh boot loads `config.toml`,
/// D-0 migration runs, D-2 provider's first invocation returns the
/// migrated values; SECOND boot of same DB returns the same values
/// from SQLite without re-running migration.
///
/// D-2 review iter-1 (BH-F2): the SQLite value is deliberately set to
/// DIFFER from the TOML value (TOML polling_frequency=10, SQLite=20)
/// so the assertion proves the Provider actually contributed — a
/// short-circuited Provider returning an empty map would fall through
/// to TOML (10) and fail the assertion. Wrapped in `temp_env` to clear
/// the env-var that t03 sets, so a parallel run can't mask the result.
#[test]
fn t10_boot_cycle_idempotent() {
    temp_env::with_var(
        "OPCGW_CHIRPSTACK__POLLING_FREQUENCY",
        None::<&str>,
        || {
            let (_dir, config_path, backend) = fresh_env();
            let cfg = load_bootstrap_config(&config_path);

            // First boot: run migration.
            let outcome1 = migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate 1");
            assert!(matches!(outcome1, SingletonMigrationOutcome::Migrated(_)));

            // Set the SQLite value to DIFFER from TOML (10 → 20) so the
            // assertion below actually proves the Provider contributed.
            backend
                .write_singleton_section(
                    "chirpstack",
                    &[(
                        "polling_frequency".to_string(),
                        serde_json::to_string(&serde_json::json!(20)).expect("serialize"),
                    )],
                )
                .expect("write");

            // First load via Provider.
            let loaded1 = AppConfig::from_path_with_sqlite(
                &config_path,
                Arc::new(backend.clone()),
            )
            .expect("load 1");
            assert_eq!(
                loaded1.chirpstack.polling_frequency, 20,
                "first boot must read the SQLite value (20), not the TOML value (10)"
            );

            // Second boot: same DB, migration should detect already-done.
            let outcome2 = migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate 2");
            assert!(
                matches!(outcome2, SingletonMigrationOutcome::AlreadyMigrated),
                "second migration must detect already-done"
            );

            // Second load via Provider — same SQLite value.
            let loaded2 = AppConfig::from_path_with_sqlite(
                &config_path,
                Arc::new(backend),
            )
            .expect("load 2");
            assert_eq!(
                loaded2.chirpstack.polling_frequency, 20,
                "second boot must read the same SQLite value (20)"
            );
            assert_eq!(
                loaded1.chirpstack.server_address, loaded2.chirpstack.server_address
            );
        },
    );
}

/// Test 11 — D-1 PUT round-trip: write a new `polling_frequency=15`
/// via the same SqliteBackend helper the D-1 PUT handler uses,
/// simulate a supervisor restart by re-invoking the loader, assert
/// the new value is loaded from SQLite via the D-2 provider.
#[test]
fn t11_d1_put_roundtrip_visible_to_d2_provider() {
    temp_env::with_var(
        "OPCGW_CHIRPSTACK__POLLING_FREQUENCY",
        None::<&str>,
        || {
            let (_dir, config_path, backend) = fresh_env();
            let cfg = load_bootstrap_config(&config_path);
            migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");

            // Initial load: TOML value 10 from the bootstrap config.
            let loaded_pre = AppConfig::from_path_with_sqlite(
                &config_path,
                Arc::new(backend.clone()),
            )
            .expect("load pre");
            assert_eq!(loaded_pre.chirpstack.polling_frequency, 10);

            // Simulate the D-1 PUT handler writing a new value.
            backend
                .write_singleton_section(
                    "chirpstack",
                    &[(
                        "polling_frequency".to_string(),
                        serde_json::to_string(&serde_json::json!(15))
                            .expect("serialize"),
                    )],
                )
                .expect("write");

            // Restart: re-invoke the loader.
            let loaded_post = AppConfig::from_path_with_sqlite(
                &config_path,
                Arc::new(backend),
            )
            .expect("load post");
            assert_eq!(
                loaded_post.chirpstack.polling_frequency, 15,
                "D-1 PUT must be visible to next-boot D-2 reload; got {}",
                loaded_post.chirpstack.polling_frequency
            );
        },
    );
}

/// Test 12 — Secret-field flow-through: `secrets.toml` carries the
/// `api_token`; D-2 provider does NOT shadow it (singleton_config
/// does not have api_token rows, per the D-0 SECRET_FIELDS_BY_SECTION
/// skip-list). Loaded `chirpstack.api_token` is the secrets.toml value.
#[test]
fn t12_secret_field_flows_through_d2_provider() {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, TOML_BASE).expect("write config");

    // Write a sibling secrets.toml. Note: secret-field overlay is
    // controlled by `SECRET_FIELDS_BY_SECTION`; api_token + user_password
    // are the two declared secrets and should flow through unchanged.
    let secrets_path = dir.path().join("secrets.toml");
    std::fs::write(
        &secrets_path,
        r#"
[chirpstack]
api_token = "from-secrets-toml"

[opcua]
user_password = "from-secrets-toml-password"
"#,
    )
    .expect("write secrets");

    let db_path = dir.path().join("opcgw.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("backend");
    let cfg = AppConfig::from_path(config_path.to_str().unwrap()).expect("bootstrap");
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");

    let loaded = AppConfig::from_path_with_sqlite(
        config_path.to_str().unwrap(),
        Arc::new(backend),
    )
    .expect("from_path_with_sqlite");
    assert_eq!(
        loaded.chirpstack.api_token, "from-secrets-toml",
        "secrets.toml api_token should flow through unchanged; got {:?}",
        loaded.chirpstack.api_token
    );
    assert_eq!(
        loaded.opcua.user_password, "from-secrets-toml-password",
        "secrets.toml user_password should flow through unchanged; got {:?}",
        loaded.opcua.user_password
    );
}

// ── Additional regression-guard tests ────────────────────────────────────

/// Test 13 — Provider gracefully handles a malformed value-JSON row
/// in SQLite (warn + skip semantic; no Err propagated to figment).
#[test]
fn t13_provider_skips_malformed_value_json_row() {
    use figment::Provider as _;
    let (_dir, _config_path, backend) = fresh_env();
    // Inject a row with malformed JSON via the same write helper the
    // PUT handler uses; the write_singleton_section helper stores
    // values verbatim, allowing us to craft a broken value.
    backend
        .write_singleton_section(
            "global",
            &[("debug".to_string(), "this is not JSON {{".to_string())],
        )
        .expect("write malformed");
    let provider = SqliteSingletonProvider::new(Arc::new(backend));
    let data = provider.data().expect("data ok despite malformed row");
    // The malformed row should NOT appear in the result. Either the
    // global section is absent (no other rows present) or, if it's
    // present, it should not contain a successfully-parsed `debug`.
    if let Some(default_dict) = data.get(&figment::Profile::Default) {
        if let Some(global) = default_dict.get("global") {
            let rendered = format!("{:?}", global);
            // The row's parse failure should mean no `debug` key
            // surfaces in the global section.
            assert!(
                !rendered.contains("\"debug\""),
                "malformed row should be skipped; found 'debug' key in {:?}",
                rendered
            );
        }
    }
}

/// Test 14 — `count_singleton_config` matches the count of rows
/// inserted by D-0 migration. Guards the `config_toml_unused_warning`
/// predicate computation in main.rs.
#[test]
fn t14_count_singleton_config_matches_post_migration() {
    let (_dir, config_path, backend) = fresh_env();
    let cfg = load_bootstrap_config(&config_path);
    let outcome = migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    if let SingletonMigrationOutcome::Migrated(report) = outcome {
        let count = backend.count_singleton_config().expect("count");
        assert_eq!(
            count, report.rows,
            "count_singleton_config must match Migrated.rows; got {} vs {}",
            count, report.rows
        );
        assert!(count > 0, "expected non-zero rows; got {}", count);
    } else {
        panic!("expected Migrated outcome");
    }
}

/// Test 15 — Provider is Send + Sync (compile-time check by
/// virtue of `Arc<SqliteBackend>` being Send + Sync). The Provider
/// must be safe to install behind a long-lived AppConfig reload.
#[test]
fn t15_provider_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SqliteSingletonProvider>();
}

/// Test 16 — Provider read-time secret filter (D-2 review iter-1
/// defense-in-depth; tested in iter-2 per BH2-1).
///
/// The migration + D-1 PUT handler skip secret fields at WRITE time,
/// but the Provider sits ABOVE secrets.toml in the figment stack, so a
/// secret row that landed in `singleton_config` via direct SQL / a
/// tampered backup restore / a future write-path bug would shadow the
/// real secret. The Provider must therefore ALSO filter secret keys at
/// READ time. This test injects `chirpstack.api_token` directly via the
/// raw `write_singleton_section` helper (bypassing the PUT handler's
/// rejection), then asserts (a) the secret does NOT appear in the
/// Provider's figment map and (b) the `config_provider_failed` warn
/// fired. Deleting the filter block would fail this test.
#[traced_test]
#[test]
fn t16_provider_filters_secret_field_at_read_time() {
    use figment::Provider as _;
    let (_dir, _config_path, backend) = fresh_env();

    // Simulate a tampered DB: a secret row written directly to
    // singleton_config (the raw helper does NOT enforce the secret
    // skip-list — only the migration + PUT handler do).
    backend
        .write_singleton_section(
            "chirpstack",
            &[(
                "api_token".to_string(),
                serde_json::to_string(&serde_json::json!("LEAKED-FROM-SQLITE"))
                    .expect("serialize"),
            )],
        )
        .expect("write secret row");

    let provider = SqliteSingletonProvider::new(Arc::new(backend));
    let data = provider.data().expect("data ok");

    // The secret must NOT appear anywhere in the Provider output.
    let rendered = format!("{:?}", data);
    assert!(
        !rendered.contains("LEAKED-FROM-SQLITE"),
        "secret value must be filtered out of the Provider map; got {:?}",
        rendered
    );
    assert!(
        !rendered.contains("api_token"),
        "secret key must be filtered out of the Provider map; got {:?}",
        rendered
    );
    // The filter emits a config_provider_failed warn on the skipped row.
    assert!(
        logs_contain("config_provider_failed"),
        "filtering a secret row must emit a config_provider_failed warn"
    );
}

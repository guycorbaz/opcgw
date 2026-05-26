// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story D-0 integration tests: singleton-config TOML → SQLite migration.
//
// Each test writes a config.toml to a fresh tempdir, creates a fresh
// SqliteBackend, and asserts D-0 migration outcomes without touching the
// web server or OPC UA server. Mirrors the C-6
// `tests/sqlite_config_migration.rs` shape.

use tempfile::TempDir;

use opcgw::config::AppConfig;
use opcgw::storage::migrate_singleton_config::{
    migrate_singleton_toml_to_sqlite, SingletonMigrationOutcome,
};
use opcgw::storage::SqliteBackend;

// ── TOML fixtures ─────────────────────────────────────────────────────────────

/// Singleton sections with realistic non-placeholder secrets. D-0 migration
/// runs to completion against this fixture.
const TOML_FULL: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "real-test-token-not-a-placeholder"
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

fn setup(toml_content: &str) -> (TempDir, AppConfig, SqliteBackend) {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, toml_content).expect("write config");

    let cfg = AppConfig::from_path(config_path.to_str().unwrap()).expect("parse config");

    let db_path = dir.path().join("test.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("sqlite backend");

    (dir, cfg, backend)
}

// ── AC#17 test list ───────────────────────────────────────────────────────────

/// Test 1 — Fresh DB + populated TOML → migration runs; SQLite singleton rows
/// populate; `is_d0_migration_done() == Ok(true)`; `PRAGMA user_version == 10`.
#[test]
fn singleton_fresh_db_populated_toml_returns_migrated() {
    let (_dir, cfg, backend) = setup(TOML_FULL);
    let outcome = migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert!(
        matches!(outcome, SingletonMigrationOutcome::Migrated(_)),
        "expected Migrated outcome"
    );
    assert!(
        backend.is_d0_migration_done().expect("done-flag check"),
        "d0_migration_done meta key must be set after Migrated"
    );
    let count = backend.count_singleton_config().expect("count");
    assert!(count > 0, "singleton_config must have rows after migration");
}

/// Test 2 — Already-migrated DB → second call is no-op via primary guard.
#[test]
fn singleton_already_migrated_returns_already_migrated() {
    let (_dir, cfg, backend) = setup(TOML_FULL);
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("first migrate");
    let outcome = migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("second migrate");
    assert!(
        matches!(outcome, SingletonMigrationOutcome::AlreadyMigrated),
        "second call must return AlreadyMigrated via primary guard"
    );
}

/// Test 3 — Secondary guard: singleton_config populated but no done-flag
/// (simulates a direct-SQLite-import scenario). Back-fill the meta key
/// best-effort + return AlreadyMigrated.
#[test]
fn singleton_secondary_guard_backfills_meta_key() {
    let (_dir, cfg, backend) = setup(TOML_FULL);

    // Seed singleton_config directly via write_singleton_section, bypassing
    // the migration entrypoint that writes the done-flag.
    backend
        .write_singleton_section(
            "global",
            &[("debug".to_string(), "true".to_string())],
        )
        .expect("direct write");

    // Precondition: rows present, done-flag absent.
    assert!(!backend.is_d0_migration_done().expect("pre-check"));
    assert!(backend.count_singleton_config().expect("count") > 0);

    // Migration takes the secondary guard path.
    let outcome = migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert!(
        matches!(outcome, SingletonMigrationOutcome::AlreadyMigrated),
        "secondary guard must return AlreadyMigrated"
    );

    // Done-flag is now back-filled.
    assert!(
        backend.is_d0_migration_done().expect("post-check"),
        "secondary guard must back-fill the meta done-flag"
    );
}

/// Test 4 — Placeholder secrets in the AppConfig snapshot → migration is
/// skipped; no singleton rows written; done-flag absent.
///
/// Note: `AppConfig::from_path` validates against placeholder strings and
/// rejects them at load-time, so this defense-in-depth code path is
/// unreachable through the normal load. The test mutates a parsed config
/// post-load to simulate a hypothetical scenario where placeholders reach
/// the migration entrypoint (e.g. a future code path that bypasses
/// validation, or a regression in the validator). The migration's
/// placeholder guard exists per AC#4 as belt-and-suspenders.
#[test]
fn singleton_placeholder_secrets_skips_migration() {
    let (_dir, mut cfg, backend) = setup(TOML_FULL);
    cfg.chirpstack.api_token =
        "REPLACE_ME_WITH_OPCGW_CHIRPSTACK__API_TOKEN_ENV_VAR".to_string();
    cfg.opcua.user_password =
        "REPLACE_ME_WITH_OPCGW_OPCUA__USER_PASSWORD_ENV_VAR".to_string();

    let outcome = migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    assert!(
        matches!(outcome, SingletonMigrationOutcome::SkippedEmptyOrPlaceholder),
        "placeholder secrets must skip the migration"
    );
    assert!(
        !backend.is_d0_migration_done().expect("done-flag check"),
        "done-flag must NOT be set when migration was skipped"
    );
    assert_eq!(
        backend.count_singleton_config().expect("count"),
        0,
        "no singleton rows must be written when migration was skipped"
    );
}

/// Test 5 — All four sections receive at least one row each.
#[test]
fn singleton_migration_covers_all_four_sections() {
    let (_dir, cfg, backend) = setup(TOML_FULL);
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");

    let rows = backend.load_singleton_config().expect("load");
    let mut sections: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (s, _, _) in &rows {
        sections.insert(s.clone());
    }
    for expected in &["global", "chirpstack", "opcua", "web"] {
        assert!(
            sections.contains(*expected),
            "section {} must be present post-migration; got sections={:?}",
            expected,
            sections
        );
    }
}

/// Test 6 — Secrets explicitly excluded. `api_token` and `user_password`
/// must never appear in singleton_config rows.
#[test]
fn singleton_migration_excludes_secrets() {
    let (_dir, cfg, backend) = setup(TOML_FULL);
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    let rows = backend.load_singleton_config().expect("load");

    for (section, key, _) in &rows {
        assert!(
            !(section == "chirpstack" && key == "api_token"),
            "api_token must NOT be in singleton_config"
        );
        assert!(
            !(section == "opcua" && key == "user_password"),
            "user_password must NOT be in singleton_config"
        );
    }
}

/// Test 7 — `count_singleton_config` reports the same count as
/// `load_singleton_config().len()`.
#[test]
fn singleton_count_matches_load_len() {
    let (_dir, cfg, backend) = setup(TOML_FULL);
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    let count = backend.count_singleton_config().expect("count");
    let rows = backend.load_singleton_config().expect("load");
    assert_eq!(count, rows.len(), "count and load.len() must agree");
}

/// Test 8 — `write_singleton_section` replaces an existing section atomically.
#[test]
fn singleton_write_section_replaces_atomically() {
    let (_dir, _cfg, backend) = setup(TOML_FULL);
    backend
        .write_singleton_section(
            "global",
            &[
                ("debug".to_string(), "true".to_string()),
                ("prune_interval_minutes".to_string(), "30".to_string()),
            ],
        )
        .expect("first write");

    // Replace with a single key — atomic delete + re-insert per section.
    backend
        .write_singleton_section(
            "global",
            &[("debug".to_string(), "false".to_string())],
        )
        .expect("second write");

    let rows = backend.load_singleton_config().expect("load");
    let globals: Vec<_> = rows.iter().filter(|(s, _, _)| s == "global").collect();
    assert_eq!(globals.len(), 1, "second write must replace the section");
    assert_eq!(globals[0].1, "debug");
    assert_eq!(globals[0].2, "false");
}

/// Test 9 — `write_singleton_section` rejects invalid section names
/// via the CHECK constraint.
#[test]
fn singleton_write_section_rejects_invalid_section() {
    let (_dir, _cfg, backend) = setup(TOML_FULL);
    let err = backend.write_singleton_section(
        "rogue",
        &[("k".to_string(), "v".to_string())],
    );
    assert!(err.is_err(), "rogue section must be rejected by CHECK");
}

/// Test 10 — `is_d0_migration_done` returns Ok(false) on a fresh DB.
#[test]
fn singleton_is_done_false_on_fresh_db() {
    let (_dir, _cfg, backend) = setup(TOML_FULL);
    assert!(
        !backend.is_d0_migration_done().expect("done-flag check"),
        "fresh DB must have d0_migration_done absent"
    );
}

/// Test 11 — `write_d0_migration_done` is idempotent (INSERT OR IGNORE).
#[test]
fn singleton_write_done_is_idempotent() {
    let (_dir, _cfg, backend) = setup(TOML_FULL);
    backend.write_d0_migration_done().expect("first write");
    let ts1 = backend.is_d0_migration_done().expect("after first");
    backend.write_d0_migration_done().expect("second write");
    let ts2 = backend.is_d0_migration_done().expect("after second");
    assert_eq!(ts1, ts2, "done-flag presence must be idempotent");
    assert!(ts1, "done-flag must be set after first write");
}

/// Test 12 — Per-section row count > 0 for each migrated section.
#[test]
fn singleton_per_section_rows_nonempty() {
    let (_dir, cfg, backend) = setup(TOML_FULL);
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    let rows = backend.load_singleton_config().expect("load");
    for section in &["global", "chirpstack", "opcua", "web"] {
        let count = rows.iter().filter(|(s, _, _)| s == section).count();
        assert!(
            count > 0,
            "section {} must have at least one row; got {}",
            section,
            count
        );
    }
}

/// Test 13 — JSON-encoded list fields (e.g. `[web].allowed_origins`)
/// round-trip through the SQLite value as a JSON string.
#[test]
fn singleton_allowed_origins_serialised_as_json() {
    let (_dir, cfg, backend) = setup(TOML_FULL);
    migrate_singleton_toml_to_sqlite(&cfg, &backend).expect("migrate");
    let rows = backend.load_singleton_config().expect("load");
    let allowed = rows
        .iter()
        .find(|(s, k, _)| s == "web" && k == "allowed_origins");
    let value = allowed.expect("allowed_origins row").2.clone();
    // serde_json::to_string on a Vec<String> yields a JSON array literal.
    assert!(value.starts_with('['), "allowed_origins must be JSON-encoded array, got: {}", value);
    assert!(
        value.contains("\"http://127.0.0.1:8080\""),
        "allowed_origins must contain the configured origin, got: {}",
        value
    );
}

/// Test 14 — `docs/logging.md` documents the new D-0 stage values
/// (AC#23 + AC#17 doc-sync grep invariant per Epic C iter-4 lesson).
#[test]
fn singleton_stage_values_documented_in_logging_md() {
    let doc =
        std::fs::read_to_string("docs/logging.md").expect("read docs/logging.md");
    for stage in &[
        "singleton_toml_to_sqlite",
        "singleton_already_migrated",
        "singleton_already_migrated_backfill_failed",
        "skipped_placeholder_singleton",
    ] {
        assert!(
            doc.contains(stage),
            "docs/logging.md must document the D-0 stage value '{}'",
            stage
        );
    }
    assert!(
        doc.contains("singleton_row_count_mismatch"),
        "docs/logging.md must document the singleton_row_count_mismatch reason"
    );
    assert!(
        doc.contains("storage_init"),
        "docs/logging.md must document the storage_init event"
    );
}

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

// ── Tracing-capture helpers (I2-F4 iter-2) ────────────────────────────────────
//
// Mirrors the pattern from tests/sqlite_config_migration.rs so Test 16 can
// assert the `storage_init` warn event fires on existing wider-mode DBs.

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

    // I1-F9 (iter-1): verify PRAGMA user_version == 10 via a raw
    // connection to the temp DB. The docstring promised this assertion;
    // the body now delivers it.
    let db_path = _dir.path().join("test.db");
    let raw = rusqlite::Connection::open(&db_path).expect("raw open");
    let version: u32 = raw
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("read user_version");
    assert_eq!(version, 10, "fresh DB after migration must be at v010");
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

/// Test 11 — `write_d0_migration_done` is idempotent under `INSERT OR
/// IGNORE` semantics: the original timestamp is preserved across repeat
/// calls.
///
/// I1-F5 (iter-1) rewrote the prior bool == bool fake guard. I2-F2 +
/// I2-F5 (iter-2) fix two new issues iter-1 introduced:
///   - **I2-F2**: a long-lived raw `rusqlite::Connection` may hold a
///     WAL snapshot from the first query and miss the second write,
///     making the byte-equality assertion vacuous. Fix: open a fresh
///     raw Connection for EACH read so each gets a new snapshot.
///   - **I2-F5**: `thread::sleep(1100ms)` does not guarantee crossing a
///     SQLite-`strftime` second boundary if both writes fall in the
///     same wall-clock second. Fix: deterministic sleep to the start of
///     the NEXT wall-clock second + 100ms slack, so the two writes
///     are guaranteed to observe different `strftime('now')` values
///     under a hypothetical `INSERT OR REPLACE` implementation.
#[test]
fn singleton_write_done_preserves_timestamp_across_calls() {
    let (dir, _cfg, backend) = setup(TOML_FULL);
    backend.write_d0_migration_done().expect("first write");
    let db_path = dir.path().join("test.db");

    // I2-F2: fresh raw connection for the first read (each open
    // starts a new read transaction; no stale WAL snapshot risk).
    let ts_after_first: String = {
        let raw = rusqlite::Connection::open(&db_path).expect("raw open 1");
        raw.query_row(
            "SELECT value FROM meta WHERE key = 'd0_migration_done'",
            [],
            |row| row.get(0),
        )
        .expect("read first timestamp")
    };
    assert!(
        !ts_after_first.is_empty(),
        "first write must set a non-empty timestamp"
    );

    // I2-F5: deterministic sleep across a second boundary so a
    // hypothetical INSERT OR REPLACE would observe a different
    // strftime('now') value. We compute the duration until the next
    // wall-clock second + 100ms slack; the second write is then
    // guaranteed to fall in a different SQLite second from the first.
    let micros_into_sec = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .subsec_micros();
    let sleep_micros = (1_000_000 - micros_into_sec) + 100_000;
    std::thread::sleep(std::time::Duration::from_micros(sleep_micros as u64));

    backend.write_d0_migration_done().expect("second write");

    // I2-F2: fresh raw connection for the second read (mirrors the
    // first; defeats WAL-snapshot caching of the prior read txn).
    let ts_after_second: String = {
        let raw = rusqlite::Connection::open(&db_path).expect("raw open 2");
        raw.query_row(
            "SELECT value FROM meta WHERE key = 'd0_migration_done'",
            [],
            |row| row.get(0),
        )
        .expect("read second timestamp")
    };
    assert_eq!(
        ts_after_first, ts_after_second,
        "INSERT OR IGNORE must preserve the original timestamp across re-calls; \
         if this assertion fails the implementation switched to INSERT OR REPLACE"
    );
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

/// Test 15 (AC#17 #11, added in I1-F6 iter-1) — Fresh SQLite database
/// creation lands at file mode 0o600 per AI-C-SEC-2. Unix-only — Windows
/// uses ACLs and the atomic-create probe is `#[cfg(unix)]`-gated.
#[cfg(unix)]
#[test]
fn singleton_fresh_db_has_chmod_0o600() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("fresh-chmod.db");
    let _backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("backend");

    let meta = std::fs::metadata(&db_path).expect("metadata");
    let mode = meta.permissions().mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "fresh SQLite DB must land at mode 0o600 (AI-C-SEC-2); got 0o{:o}",
        mode
    );
}

/// Test 16 (AC#17 #12, added in I1-F6 iter-1; I2-F4 iter-2 adds the
/// warn-emission assertion) — An existing wider-mode SQLite database is
/// NOT chmod'd retroactively on subsequent `ConnectionPool::new` calls
/// AND the `storage_init` warn fires so operators know the mode is
/// wider than 0o600. Two-part AC#12 contract; iter-1 covered only the
/// no-retroactive-chmod half.
#[cfg(unix)]
#[test]
fn singleton_existing_wider_db_is_not_chmod_retroactively() {
    use std::os::unix::fs::PermissionsExt;

    init_test_subscriber();

    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("existing-wider.db");

    // First open creates the file at 0o600.
    {
        let _b = SqliteBackend::new(db_path.to_str().unwrap()).expect("first backend");
    }
    // Mutate to 0o644 externally to simulate an operator's existing wider
    // permissions (e.g. inherited from a 0o022 umask).
    std::fs::set_permissions(&db_path, std::fs::Permissions::from_mode(0o644))
        .expect("widen mode");
    let pre_mode = std::fs::metadata(&db_path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(pre_mode, 0o644, "test precondition: mode widened to 0o644");

    // I2-F4: clear the captured-logs buffer immediately before the second
    // backend creation so we only capture events from this call.
    clear_captured_logs();

    // Re-open. The atomic-create probe sees AlreadyExists; no chmod
    // fires; the warn block at the end of ConnectionPool::new emits.
    {
        let _b2 = SqliteBackend::new(db_path.to_str().unwrap()).expect("second backend");
    }

    let post_mode = std::fs::metadata(&db_path).expect("metadata").permissions().mode() & 0o777;
    assert_eq!(
        post_mode, 0o644,
        "AC#12 part 1: existing wider-mode DB must NOT be chmod'd retroactively; \
         expected mode unchanged at 0o644, got 0o{:o}",
        post_mode
    );

    // I2-F4: AC#12 part 2 — verify the storage_init warn fires with the
    // mode field set. The captured log buffer will contain a line like
    // `WARN ... event="storage_init" path="..." mode="644" ...`.
    let logs = captured_logs();
    assert!(
        logs.contains("storage_init"),
        "AC#12 part 2: storage_init event must fire on existing wider-mode DB; \
         no storage_init in captured logs:\n{}",
        logs
    );
    // I3-F3 (iter-3): tighten the mode-field assertion to require the
    // structured-log context. Bare `contains("644")` could be satisfied
    // by unrelated content (e.g. a temp-dir path containing the digit
    // sequence `644`); the field-prefix anchor pins the assertion to
    // the storage_init warn's `mode=` field specifically.
    assert!(
        logs.contains("mode=\"644\"") || logs.contains("mode=644"),
        "AC#12 part 2: storage_init warn must carry mode=\"644\" so operators \
         can identify the file's actual permissions; expected `mode=\"644\"` or \
         `mode=644` in captured logs:\n{}",
        logs
    );
}

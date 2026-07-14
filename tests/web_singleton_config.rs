// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story D-1 integration tests: singleton-config editor UI (AC#16).
//
// Mirrors the web_application_crud.rs fixture pattern — each test
// owns a fresh tempdir + ephemeral-port axum server. The fixture
// additionally runs the D-0 singleton migration so SQLite has
// populated singleton rows for GET to return.

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use reqwest::header;
use reqwest::StatusCode;
use serde_json::Value;
use serial_test::serial;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use opcgw::storage::memory::InMemoryBackend;
use opcgw::storage::migrate_singleton_config::migrate_singleton_toml_to_sqlite;
use opcgw::storage::SqliteBackend;
use opcgw::storage::StorageBackend;
use opcgw::web::auth::WebAuthState;
use opcgw::web::{
    bind as web_bind, build_router, run as web_run, AppState, DashboardConfigSnapshot,
};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-d-1";
const TEST_REALM: &str = "opcgw-d-1";

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

fn build_basic_auth(user: &str, password: &str) -> String {
    let blob = BASE64_STANDARD.encode(format!("{user}:{password}"));
    format!("Basic {blob}")
}

struct Fixture {
    base_url: String,
    cancel: CancellationToken,
    server_handle: tokio::task::JoinHandle<()>,
    shutdown_token: CancellationToken,
    // I1-F4 (iter-1): expose the backend so tests can read-back SQLite
    // state to verify writes actually persisted (closes the fake-
    // regression-guard finding-class on Test 4).
    sqlite_config: Arc<SqliteBackend>,
    // Story F-0: expose AppState so tests can assert the staged-changes
    // marker (`has_pending_changes`) after a PUT.
    app_state: Arc<AppState>,
    _temp_dir: TempDir,
}

impl Fixture {
    async fn shutdown(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server_handle).await;
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

const TOML_TEMPLATE: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "real-token-not-placeholder"
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
user_password = "test-password-d-1"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-d-1"
"#;

fn inject_allowed_origins(toml: &str, base_url: &str) -> String {
    let injected = format!("allowed_origins = [\"{}\"]", base_url);
    let mut result = String::with_capacity(toml.len() + injected.len() + 32);
    let mut inserted = false;
    for line in toml.lines() {
        result.push_str(line);
        result.push('\n');
        if !inserted && line.trim_start().starts_with("auth_realm") {
            result.push_str(&injected);
            result.push('\n');
            inserted = true;
        }
    }
    if !inserted {
        result.push_str("\n[web]\n");
        result.push_str(&injected);
        result.push('\n');
    }
    result
}

async fn spawn_fixture() -> Fixture {
    init_test_subscriber();

    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");

    let listener = web_bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    let base_url = format!("http://127.0.0.1:{port}");

    let final_toml = inject_allowed_origins(TOML_TEMPLATE, &base_url);
    std::fs::write(&config_path, &final_toml).expect("write seed toml");

    let initial = Arc::new(
        opcgw::config::AppConfig::from_path(config_path.to_str().expect("utf-8 path"))
            .expect("seed config validates"),
    );
    let (handle, _rx) = opcgw::config_reload::ConfigReloadHandle::new(initial.clone());
    let config_reload = Arc::new(handle);

    let db_path = dir.path().join("test.db");
    let sqlite_backend = SqliteBackend::new(db_path.to_str().expect("db path"))
        .expect("sqlite backend");

    // D-1: run the D-0 singleton migration so SQLite has rows for GET.
    migrate_singleton_toml_to_sqlite(&initial, &sqlite_backend)
        .expect("singleton migration");

    let sqlite_config = Arc::new(sqlite_backend);

    let auth = Arc::new(WebAuthState::new_with_fresh_key(
        TEST_USER,
        TEST_PASSWORD,
        TEST_REALM.to_string(),
    ));
    let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
    let snapshot = Arc::new(DashboardConfigSnapshot::from_config(&initial));

    let shutdown_token = tokio_util::sync::CancellationToken::new();

    let app_state = Arc::new(AppState {
        auth,
        backend,
        dashboard_snapshot: std::sync::RwLock::new(snapshot),
        start_time: std::time::Instant::now(),
        stale_threshold_secs: std::sync::atomic::AtomicU64::new(120),
        config_reload: config_reload.clone(),
        sqlite_config,
        static_dir: std::path::PathBuf::from("static"),
        is_first_run: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        secrets_path: std::path::PathBuf::from("/tmp/test-secrets.toml"),
        shutdown_token: shutdown_token.clone(),
        inventory_cache: std::sync::Arc::new(opcgw::chirpstack_inventory::InventoryCache::new(60)),
        pending_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        applied_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        apply_signal: std::sync::Arc::new(tokio::sync::Notify::new()),
    });

    let cancel = CancellationToken::new();
    let static_dir = PathBuf::from("static");
    let router = build_router(app_state.clone(), static_dir);
    let cancel_for_run = cancel.clone();
    let server_handle = tokio::spawn(async move {
        let _ = web_run(listener, router, TEST_REALM, cancel_for_run).await;
    });

    // Readiness probe
    let probe = reqwest::Client::new();
    let probe_url = format!("{}/api/health", base_url);
    let auth_header = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let probe_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match probe
            .get(&probe_url)
            .header(header::AUTHORIZATION, &auth_header)
            .send()
            .await
        {
            Ok(r) if r.status() == StatusCode::OK => break,
            _ => {
                if std::time::Instant::now() >= probe_deadline {
                    panic!("server failed to become ready within 5s");
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    Fixture {
        base_url,
        cancel,
        server_handle,
        shutdown_token,
        sqlite_config: app_state.sqlite_config.clone(),
        app_state: app_state.clone(),
        _temp_dir: dir,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Test 1 — GET returns the 4-section snapshot with secret placeholders.
#[tokio::test]
#[serial(captured_logs)]
async fn d1_get_returns_snapshot_with_secret_placeholders() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let r = reqwest::Client::new()
        .get(fx.url("/api/config/singleton"))
        .header(header::AUTHORIZATION, &auth)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.expect("json");
    for section in ["global", "chirpstack", "opcua", "web"] {
        assert!(body.get(section).is_some(), "section {} must be present", section);
    }
    assert_eq!(
        body["chirpstack"]["api_token"],
        Value::String("<set via config/secrets.toml>".to_string()),
        "api_token must be placeholder, not real token"
    );
    assert_eq!(
        body["opcua"]["user_password"],
        Value::String("<set via config/secrets.toml>".to_string()),
        "user_password must be placeholder"
    );
    // Sanity: non-secret field is present with its real value.
    assert!(body["global"]["debug"].is_boolean());
    fx.shutdown().await;
}

/// Issue #155: the GET backfills non-secret `[opcua]` knobs from the effective
/// config even when they were never persisted to `singleton_config`, so the
/// web Admin editor can surface them (and they render as numbers). Both new
/// subscription knobs must appear as numeric fields.
#[tokio::test]
#[serial(captured_logs)]
async fn issue155_get_backfills_subscription_knobs() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let r = reqwest::Client::new()
        .get(fx.url("/api/config/singleton"))
        .header(header::AUTHORIZATION, &auth)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::OK);
    let body: Value = r.json().await.expect("json");
    assert!(
        body["opcua"]["max_keep_alive_count"].is_number(),
        "max_keep_alive_count must be backfilled as a number, got {:?}",
        body["opcua"].get("max_keep_alive_count")
    );
    assert!(
        body["opcua"]["min_publishing_interval_ms"].is_number(),
        "min_publishing_interval_ms must be backfilled as a number, got {:?}",
        body["opcua"].get("min_publishing_interval_ms")
    );
    fx.shutdown().await;
}

/// Test 2 — GET requires Basic-auth.
#[tokio::test]
#[serial(captured_logs)]
async fn d1_get_requires_basic_auth() {
    let fx = spawn_fixture().await;
    let r = reqwest::Client::new()
        .get(fx.url("/api/config/singleton"))
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    fx.shutdown().await;
}

/// Test 3 — GET is CSRF-exempt (succeeds without Origin header).
#[tokio::test]
#[serial(captured_logs)]
async fn d1_get_is_csrf_exempt() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let r = reqwest::Client::new()
        .get(fx.url("/api/config/singleton"))
        .header(header::AUTHORIZATION, &auth)
        // No Origin header
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::OK);
    fx.shutdown().await;
}

/// Test 4 — PUT /global with a valid payload returns 202, **stages** the
/// change (Story F-0: bumps the pending-changes marker, does NOT restart
/// anything), and **actually persists to SQLite** (I1-F4 iter-1: prior
/// version was a fake regression guard that only checked HTTP status +
/// shutdown + log strings; a broken `write_singleton_section` that
/// returned `Ok(())` without writing would have passed).
#[tokio::test]
#[serial(captured_logs)]
async fn d1_put_global_success_stages_change() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    clear_captured_logs();
    let body = serde_json::json!({
        "debug": false,
        "prune_interval_minutes": 30,
        "command_delivery_poll_interval_secs": 10
    });
    let r = reqwest::Client::new()
        .put(fx.url("/api/config/singleton/global"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let resp: Value = r.json().await.expect("json");
    // Story F-0: a successful PUT STAGES the change — it no longer means
    // "restart_pending" and must NOT cancel the shutdown token (which would
    // tear down the process / restart the container under the old model).
    assert_eq!(resp["status"], "staged");
    assert_eq!(resp["pending_changes"], serde_json::json!(true));

    // The staged-changes marker flips on, and crucially the process-wide
    // shutdown token is NOT cancelled — no in-process restart, no container
    // restart. The operator applies the batch later via POST /api/config/apply.
    assert!(
        fx.app_state.has_pending_changes(),
        "PUT must bump the pending-changes marker"
    );
    assert!(
        !fx.shutdown_token.is_cancelled(),
        "staged PUT must NOT cancel the shutdown token (Story F-0: no restart on save)"
    );

    // I1-F4 (iter-1): SQLite round-trip — read the singleton_config
    // rows back via the backend exposed on the fixture, assert the
    // PUT'd values are durably present. A broken `write_singleton_section`
    // returning `Ok(())` without committing would now fail loudly.
    let rows = fx.sqlite_config.load_singleton_config().expect("load rows");
    let global_pairs: std::collections::HashMap<String, String> = rows
        .iter()
        .filter(|(s, _, _)| s == "global")
        .map(|(_, k, v)| (k.clone(), v.clone()))
        .collect();
    assert_eq!(
        global_pairs.get("debug").map(|s| s.as_str()),
        Some("false"),
        "PUT must persist debug=false; got rows={:?}",
        global_pairs
    );
    assert_eq!(
        global_pairs.get("prune_interval_minutes").map(|s| s.as_str()),
        Some("30"),
        "PUT must persist prune_interval_minutes=30"
    );

    // Audit events present in captured logs.
    let logs = captured_logs();
    assert!(
        logs.contains("singleton_config_updated"),
        "captured logs must contain singleton_config_updated; got:\n{}",
        logs
    );
    assert!(
        logs.contains("config_staged"),
        "captured logs must contain config_staged (Story F-0 staged-write audit event); got:\n{}",
        logs
    );
    fx.shutdown().await;
}

/// Test 5 — PUT /chirpstack with `api_token` is rejected.
#[tokio::test]
#[serial(captured_logs)]
async fn d1_put_chirpstack_with_api_token_rejected() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let body = serde_json::json!({"api_token": "evil-new-token"});
    let r = reqwest::Client::new()
        .put(fx.url("/api/config/singleton/chirpstack"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let resp: Value = r.json().await.expect("json");
    assert_eq!(resp["reason"], "secret_field_not_editable");
    assert!(
        !fx.shutdown_token.is_cancelled(),
        "rejection must NOT trigger shutdown"
    );
    fx.shutdown().await;
}

/// Test 6 — PUT /opcua with `user_password` is rejected (symmetric to test 5).
#[tokio::test]
#[serial(captured_logs)]
async fn d1_put_opcua_with_user_password_rejected() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let body = serde_json::json!({"user_password": "evil-new-pwd"});
    let r = reqwest::Client::new()
        .put(fx.url("/api/config/singleton/opcua"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let resp: Value = r.json().await.expect("json");
    assert_eq!(resp["reason"], "secret_field_not_editable");
    fx.shutdown().await;
}

/// Test 7 — PUT to an unknown section is rejected.
#[tokio::test]
#[serial(captured_logs)]
async fn d1_put_invalid_section_rejected() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let body = serde_json::json!({"foo": "bar"});
    let r = reqwest::Client::new()
        .put(fx.url("/api/config/singleton/rogue"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let resp: Value = r.json().await.expect("json");
    assert_eq!(resp["reason"], "invalid_section");
    fx.shutdown().await;
}

/// Test 8 — PUT /web with privileged port rejected by AppConfig::validate.
#[tokio::test]
#[serial(captured_logs)]
async fn d1_put_web_privileged_port_rejected() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let body = serde_json::json!({"port": 80});
    let r = reqwest::Client::new()
        .put(fx.url("/api/config/singleton/web"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let resp: Value = r.json().await.expect("json");
    assert_eq!(resp["reason"], "validation");
    assert!(
        !fx.shutdown_token.is_cancelled(),
        "validation rejection must NOT trigger shutdown"
    );
    fx.shutdown().await;
}

/// Test 9 — PUT requires CSRF (rejects cross-origin Origin).
#[tokio::test]
#[serial(captured_logs)]
async fn d1_put_requires_csrf() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let body = serde_json::json!({"debug": false});
    let r = reqwest::Client::new()
        .put(fx.url("/api/config/singleton/global"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, "http://evil.example.com:1234")
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    fx.shutdown().await;
}

/// Test 10 — PUT requires Basic-auth.
#[tokio::test]
#[serial(captured_logs)]
async fn d1_put_requires_basic_auth() {
    let fx = spawn_fixture().await;
    let body = serde_json::json!({"debug": false});
    let r = reqwest::Client::new()
        .put(fx.url("/api/config/singleton/global"))
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    fx.shutdown().await;
}

/// Story F-0 — `POST /api/config/apply` requires Basic-auth.
#[tokio::test]
#[serial(captured_logs)]
async fn f0_apply_requires_basic_auth() {
    let fx = spawn_fixture().await;
    let r = reqwest::Client::new()
        .post(fx.url("/api/config/apply"))
        .header(header::ORIGIN, &fx.base_url)
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::UNAUTHORIZED);
    fx.shutdown().await;
}

/// Story F-0 — `POST /api/config/apply` requires CSRF (rejects cross-origin).
#[tokio::test]
#[serial(captured_logs)]
async fn f0_apply_requires_csrf() {
    let fx = spawn_fixture().await;
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let r = reqwest::Client::new()
        .post(fx.url("/api/config/apply"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, "http://evil.example.com:1234")
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    fx.shutdown().await;
}

/// Story F-0 — a valid `POST /api/config/apply` returns 202 and fires the
/// supervisor's apply signal (a permit is stored, so a subsequent
/// `notified()` resolves immediately — proving the supervisor would wake).
#[tokio::test]
#[serial(captured_logs)]
async fn f0_apply_returns_202_and_fires_signal() {
    let fx = spawn_fixture().await;
    // Story F-0 review (P4): Apply only restarts when there ARE staged
    // changes. Stage one so this exercises the 202 path.
    fx.app_state.stage_config_write("review_test");
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let r = reqwest::Client::new()
        .post(fx.url("/api/config/apply"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, &fx.base_url)
        // CSRF requires the strict application/json Content-Type (same contract
        // as every other config-write endpoint); the Apply button's JS fetch
        // and the subprocess integration test both send it.
        .header(header::CONTENT_TYPE, "application/json")
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::ACCEPTED);
    let resp: Value = r.json().await.expect("json");
    assert_eq!(resp["status"], "apply_requested");

    // The handler called `apply_signal.notify_one()`, which stores a permit
    // when no waiter is parked. The supervisor's next `notified()` must then
    // resolve immediately — assert that with a short timeout.
    let notified = tokio::time::timeout(
        Duration::from_secs(1),
        fx.app_state.apply_signal.notified(),
    )
    .await;
    assert!(
        notified.is_ok(),
        "apply endpoint must fire apply_signal so the supervisor wakes"
    );
    fx.shutdown().await;
}

/// Story F-0 review (P4) — `POST /api/config/apply` with NO pending changes
/// returns `200 {"status":"no_pending_changes"}` and does NOT fire the apply
/// signal (avoids a gratuitous soft restart / OPC UA client disconnect on a
/// duplicate or stale POST).
#[tokio::test]
#[serial(captured_logs)]
async fn f0_apply_with_no_pending_changes_is_noop() {
    let fx = spawn_fixture().await;
    // Fresh fixture: pending_gen == applied_gen == 0 → nothing staged.
    assert!(!fx.app_state.has_pending_changes());
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let r = reqwest::Client::new()
        .post(fx.url("/api/config/apply"))
        .header(header::AUTHORIZATION, &auth)
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .send()
        .await
        .expect("send");
    assert_eq!(r.status(), StatusCode::OK);
    let resp: Value = r.json().await.expect("json");
    assert_eq!(resp["status"], "no_pending_changes");

    // The signal must NOT have been fired: a `notified()` should time out.
    let notified = tokio::time::timeout(
        Duration::from_millis(300),
        fx.app_state.apply_signal.notified(),
    )
    .await;
    assert!(
        notified.is_err(),
        "apply with no pending changes must NOT fire apply_signal (no restart)"
    );
    fx.shutdown().await;
}

/// Test 11 — Boot-time overlay: SQLite values override TOML/figment defaults.
/// Unit-level test exercising `AppConfig::overlay_singletons_from_sqlite_rows`
/// directly without spinning up an axum server.
#[test]
fn d1_appconfig_overlay_from_sqlite_rows() {
    let dir = TempDir::new().expect("tempdir");
    let cfg_path = dir.path().join("config.toml");
    std::fs::write(&cfg_path, TOML_TEMPLATE).expect("write toml");
    let mut cfg = opcgw::config::AppConfig::from_path(cfg_path.to_str().unwrap())
        .expect("from_path");

    // TOML has `debug = true`. Overlay flips it to false via SQLite.
    let rows = vec![("global".to_string(), "debug".to_string(), "false".to_string())];
    cfg.overlay_singletons_from_sqlite_rows(&rows)
        .expect("overlay");
    assert!(!cfg.global.debug, "overlay must flip debug to false");

    // Subsequent overlay with a different field doesn't reset the prior.
    let rows2 = vec![(
        "global".to_string(),
        "prune_interval_minutes".to_string(),
        "30".to_string(),
    )];
    cfg.overlay_singletons_from_sqlite_rows(&rows2)
        .expect("second overlay");
    assert!(!cfg.global.debug, "subsequent overlay must not reset prior field");
    assert_eq!(cfg.global.prune_interval_minutes, 30);
}

/// Test 12 — The new audit event names match the closed taxonomy
/// documented in docs/logging.md (grep invariant per AC#24 doc-sync gate).
#[test]
fn d1_audit_event_names_documented_in_logging_md() {
    let doc = std::fs::read_to_string("docs/logging.md").expect("read logging.md");
    for ev in &[
        "config_overlay",
        "config_overlay_failed",
        "config_get_singleton",
        "singleton_config_updated",
        "singleton_config_rejected",
        // I2-F3 (iter-2): I1-F3 introduced `singleton_config_storage_error`
        // as the storage-fault event (split from `singleton_config_rejected`
        // which now stays scoped to client errors). Add to the grep
        // invariant so a future doc-deletion regression is caught.
        "singleton_config_storage_error",
        // Story F-0: staged-apply audit taxonomy. `singleton_config_restart_required`
        // was retired (the singleton PUT no longer restarts; it stages).
        "config_staged",
        "apply_invoked",
        "apply_requested",
        "apply_completed",
        "apply_failed",
        "config_apply_rejected",
    ] {
        assert!(
            doc.contains(ev),
            "docs/logging.md must document D-1 audit event {:?}",
            ev
        );
    }
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz
//
// Story F-4 integration tests: config export / import.
//
// Mirrors the web_singleton_config.rs fixture (fresh tempdir + ephemeral-port
// axum server, SQLite migrated from a seed TOML). The seed includes an
// application tree so the import bulk-replace can be observed.

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use reqwest::header;
use reqwest::StatusCode;
use serde_json::{json, Value};
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
const TEST_PASSWORD: &str = "test-password-f-4";
const TEST_REALM: &str = "opcgw-f-4";

fn build_basic_auth(user: &str, password: &str) -> String {
    let blob = BASE64_STANDARD.encode(format!("{user}:{password}"));
    format!("Basic {blob}")
}

/// Seed config with a real api_token + user_password (secrets) and one
/// application/device/metric so export + import bulk-replace are observable.
/// `{ORIGIN}` is replaced with the server base URL for the CSRF allow-list.
const SEED_TOML: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "SECRET-SOURCE-TOKEN"
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
user_password = "SECRET-SOURCE-PASSWORD"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-f-4"
allowed_origins = ["{ORIGIN}"]

[[application]]
application_name = "App-Original"
application_id = "app-orig"

[[application.device]]
device_id = "dev-orig"
device_name = "Device Original"

[[application.device.read_metric]]
metric_name = "temp"
chirpstack_metric_name = "temperature"
metric_type = "Float"
"#;

struct Fixture {
    base_url: String,
    cancel: CancellationToken,
    server_handle: tokio::task::JoinHandle<()>,
    sqlite_config: Arc<SqliteBackend>,
    app_state: Arc<AppState>,
    _temp_dir: TempDir,
}

impl Fixture {
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
    async fn shutdown(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server_handle).await;
    }
}

async fn spawn_fixture() -> Fixture {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");

    let listener = web_bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    let base_url = format!("http://127.0.0.1:{port}");

    let final_toml = SEED_TOML.replace("{ORIGIN}", &base_url);
    std::fs::write(&config_path, &final_toml).expect("write seed toml");

    let initial = Arc::new(
        opcgw::config::AppConfig::from_path(config_path.to_str().expect("utf-8 path"))
            .expect("seed config validates"),
    );
    let (handle, _rx) = opcgw::config_reload::ConfigReloadHandle::new(initial.clone());
    let config_reload = Arc::new(handle);

    let db_path = dir.path().join("test.db");
    let sqlite_backend =
        SqliteBackend::new(db_path.to_str().expect("db path")).expect("sqlite backend");
    migrate_singleton_toml_to_sqlite(&initial, &sqlite_backend).expect("singleton migration");
    // Seed the SQLite app tree from the config so import can replace it.
    sqlite_backend
        .migrate_applications_config(&initial.application_list)
        .expect("seed app tree");
    let sqlite_config = Arc::new(sqlite_backend);

    let auth = Arc::new(WebAuthState::new_with_fresh_key(
        TEST_USER,
        TEST_PASSWORD,
        TEST_REALM.to_string(),
    ));
    let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
    let snapshot = Arc::new(DashboardConfigSnapshot::from_config(&initial));
    let shutdown_token = CancellationToken::new();

    let app_state = Arc::new(AppState {
        auth,
        backend,
        dashboard_snapshot: std::sync::RwLock::new(snapshot),
        start_time: std::time::Instant::now(),
        stale_threshold_secs: std::sync::atomic::AtomicU64::new(120),
        config_reload: config_reload.clone(),
        sqlite_config: sqlite_config.clone(),
        static_dir: PathBuf::from("static"),
        is_first_run: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        secrets_path: PathBuf::from("/tmp/test-secrets.toml"),
        shutdown_token,
        inventory_cache: Arc::new(opcgw::chirpstack_inventory::InventoryCache::new(60)),
        pending_gen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        applied_gen: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        apply_signal: Arc::new(tokio::sync::Notify::new()),
    });

    let cancel = CancellationToken::new();
    let router = build_router(app_state.clone(), PathBuf::from("static"));
    let cancel_for_run = cancel.clone();
    let server_handle = tokio::spawn(async move {
        let _ = web_run(listener, router, TEST_REALM, cancel_for_run).await;
    });

    // Readiness probe.
    let probe = reqwest::Client::new();
    let probe_url = format!("{}/api/health", base_url);
    let probe_auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match probe
            .get(&probe_url)
            .header(header::AUTHORIZATION, &probe_auth)
            .send()
            .await
        {
            Ok(r) if r.status() == StatusCode::OK => break,
            _ => {
                if std::time::Instant::now() >= deadline {
                    panic!("server not ready within 5s");
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }
    }

    Fixture {
        base_url,
        cancel,
        server_handle,
        sqlite_config,
        app_state,
        _temp_dir: dir,
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn export_returns_toml_without_secrets() {
    let fx = spawn_fixture().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/api/config/export"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("GET export");
    assert_eq!(resp.status(), StatusCode::OK);
    let cd = resp
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(cd.contains("attachment"), "export must be a download, got {cd:?}");
    let body = resp.text().await.expect("body");

    // Secrets must NEVER appear.
    assert!(!body.contains("SECRET-SOURCE-TOKEN"), "export leaked api_token:\n{body}");
    assert!(
        !body.contains("SECRET-SOURCE-PASSWORD"),
        "export leaked user_password:\n{body}"
    );
    assert!(!body.contains("api_token"), "export must omit the api_token key");
    assert!(!body.contains("user_password"), "export must omit user_password");
    // Host sections excluded; portable sections + app tree present.
    assert!(!body.contains("[storage]"), "export must omit [storage]");
    assert!(body.contains("[chirpstack]"));
    assert!(body.contains("app-orig"), "export must include the application tree");

    fx.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn export_requires_auth() {
    let fx = spawn_fixture().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/api/config/export"))
        .send()
        .await
        .expect("GET export unauth");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    fx.shutdown().await;
}

/// Build an import body that replaces the app tree with a different application.
fn import_toml_with_new_app() -> String {
    r#"
[chirpstack]
server_address = "http://imported-host:9090"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 10
retry = 1
delay = 1

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

[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-f-4"

[[application]]
application_name = "App-Imported"
application_id = "app-imported"

[[application.device]]
device_id = "dev-imported"
device_name = "Device Imported"

[[application.device.read_metric]]
metric_name = "humidity"
chirpstack_metric_name = "humidity"
metric_type = "Float"
"#
    .to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn import_valid_config_stages_and_replaces_tree_without_applying() {
    let fx = spawn_fixture().await;
    let client = reqwest::Client::new();

    // Sanity: the seed tree has app-orig.
    let before = fx.sqlite_config.load_all_applications_config().expect("load apps");
    assert!(before.iter().any(|a| a.application_id == "app-orig"));
    let applied_before = fx
        .app_state
        .applied_gen
        .load(std::sync::atomic::Ordering::Relaxed);

    let resp = client
        .post(fx.url("/api/config/import"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&json!({ "toml": import_toml_with_new_app() }))
        .send()
        .await
        .expect("POST import");
    assert_eq!(resp.status(), StatusCode::ACCEPTED);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["status"], "staged");
    assert_eq!(body["pending_changes"], true);

    // The app tree was replaced.
    let after = fx.sqlite_config.load_all_applications_config().expect("load apps");
    assert!(
        after.iter().any(|a| a.application_id == "app-imported"),
        "imported application must be present, got {after:?}"
    );
    assert!(
        !after.iter().any(|a| a.application_id == "app-orig"),
        "original application must be REPLACED, got {after:?}"
    );

    // The non-secret chirpstack value was staged to SQLite.
    let rows = fx.sqlite_config.load_singleton_config().expect("singleton");
    assert!(
        rows.iter().any(|(s, k, v)| s == "chirpstack"
            && k == "server_address"
            && v.contains("imported-host:9090")),
        "imported server_address must be staged, rows: {rows:?}"
    );
    // The SECRET api_token must NOT be in SQLite (it stays in secrets.toml).
    assert!(
        !rows.iter().any(|(_, k, _)| k == "api_token"),
        "api_token must never be written to SQLite, rows: {rows:?}"
    );

    // Staged, NOT applied: pending_changes true, applied_gen unchanged (no
    // restart / apply_signal fired by import).
    assert!(fx.app_state.has_pending_changes(), "import must stage a pending change");
    assert_eq!(
        fx.app_state
            .applied_gen
            .load(std::sync::atomic::Ordering::Relaxed),
        applied_before,
        "import must NOT apply inline (applied_gen unchanged)"
    );

    fx.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn import_malformed_json_rejected() {
    let fx = spawn_fixture().await;
    let client = reqwest::Client::new();
    let resp = client
        .post(fx.url("/api/config/import"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .body("not json at all")
        .send()
        .await
        .expect("POST import");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["reason"], "invalid_json");
    assert!(!fx.app_state.has_pending_changes(), "nothing staged on bad JSON");
    fx.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn import_invalid_config_rejected_nothing_staged() {
    let fx = spawn_fixture().await;
    let client = reqwest::Client::new();
    // Duplicate device_id within an application → validate() rejects.
    let bad = r#"
[[application]]
application_name = "Dup"
application_id = "app-dup"
[[application.device]]
device_id = "same"
device_name = "D1"
[[application.device.read_metric]]
metric_name = "m1"
chirpstack_metric_name = "m1"
metric_type = "Float"
[[application.device]]
device_id = "same"
device_name = "D2"
[[application.device.read_metric]]
metric_name = "m2"
chirpstack_metric_name = "m2"
metric_type = "Float"
"#;
    let resp = client
        .post(fx.url("/api/config/import"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::ORIGIN, &fx.base_url)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&json!({ "toml": bad }))
        .send()
        .await
        .expect("POST import");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["reason"], "config_invalid");
    // Nothing written: the original app tree is intact, nothing staged.
    let after = fx.sqlite_config.load_all_applications_config().expect("load apps");
    assert!(
        after.iter().any(|a| a.application_id == "app-orig"),
        "original tree must be intact after a rejected import"
    );
    assert!(!fx.app_state.has_pending_changes(), "nothing staged on invalid config");
    fx.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial]
async fn import_requires_csrf() {
    let fx = spawn_fixture().await;
    let client = reqwest::Client::new();
    // No Origin header → CSRF rejects.
    let resp = client
        .post(fx.url("/api/config/import"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .json(&json!({ "toml": "" }))
        .send()
        .await
        .expect("POST import");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    fx.shutdown().await;
}

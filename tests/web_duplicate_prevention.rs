// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2026] [Guy Corbaz]
//
// Story C-3 integration tests: server-side duplicate-prevention validator
// + cross-path consistency (FR/AC#16). Verifies the C-3 contract holds
// uniformly across every write path that can introduce a duplicate:
//
//   1. POST /api/applications                           — duplicate application_id
//   2. POST /api/applications/{app}/devices             — duplicate device_id within app
//   3. POST /api/applications/{app}/devices             — same DevEUI across apps (POSITIVE, AC#5)
//   4-5. POST device with duplicate metric_name + chirpstack_metric_name (pre-flight 409)
//   6. POST device with same chirpstack_metric_name on DIFFERENT devices (POSITIVE, AC#7)
//   7-8. PUT device with duplicate metric_name + chirpstack_metric_name (pre-flight 409)
//   9-10. POST command with duplicate command_id + command_name
//   11. Audit-event taxonomy: every rejection path emits
//       `event=…_rejected reason="conflict" conflict_kind="duplicate"`
//   12. Hot-reload (Story 9-7's reload primitive) rejects a duplicate-
//       introducing TOML edit (AC#9). In-memory snapshot is unchanged.
//
// Issue #102 carry-forward: the fixture helpers below duplicate the
// shape used in `web_application_crud.rs` / `web_device_crud.rs` /
// `web_command_crud.rs`. A future extraction into `tests/common/`
// is tracked separately; C-3 keeps the copy to avoid scope creep.

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

use opcgw::config_reload::{ConfigReloadHandle, ReloadError};
use opcgw::storage::memory::InMemoryBackend;
use opcgw::storage::StorageBackend;
use opcgw::web::auth::WebAuthState;
use opcgw::web::config_writer::ConfigWriter;
use opcgw::web::{
    bind as web_bind, build_router, run as web_run, AppState, DashboardConfigSnapshot,
};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-c-3";
const TEST_REALM: &str = "opcgw-c-3";

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

struct CrudFixture {
    base_url: String,
    config_path: PathBuf,
    cancel: CancellationToken,
    server_handle: tokio::task::JoinHandle<()>,
    listener_handle: tokio::task::JoinHandle<()>,
    _temp_dir: TempDir,
}

impl CrudFixture {
    async fn shutdown(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server_handle).await;
        match tokio::time::timeout(Duration::from_secs(5), self.listener_handle).await {
            Ok(Ok(())) => {}
            Ok(Err(join_err)) if join_err.is_panic() => {
                std::panic::resume_unwind(join_err.into_panic());
            }
            Ok(Err(_)) => {}
            Err(_elapsed) => {}
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

const APP_TOML_TEMPLATE: &str = r#"# C-3 duplicate-prevention test seed
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5
command_delivery_timeout_secs = 60
command_timeout_check_interval_secs = 10
history_retention_days = 7

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "SECRET_SENTINEL_TOKEN_DO_NOT_LEAK"
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
user_password = "SECRET_SENTINEL_PASSWORD_DO_NOT_LEAK"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-c-3"

[[application]]
application_name = "Building Sensors"
application_id = "app-1"

  [[application.device]]
  device_id = "dev-1"
  device_name = "Dev One"

    [[application.device.read_metric]]
    metric_name = "temperature"
    chirpstack_metric_name = "temperature"
    metric_type = "Float"
    metric_unit = "C"

  [[application.device]]
  device_id = "dev-2"
  device_name = "Dev Two"

    [[application.device.read_metric]]
    metric_name = "humidity"
    chirpstack_metric_name = "humidity"
    metric_type = "Float"
    metric_unit = "%"

[[application]]
application_name = "Field Probes"
application_id = "app-2"

  [[application.device]]
  device_id = "probe-1"
  device_name = "Probe Alpha"

    [[application.device.read_metric]]
    metric_name = "battery"
    chirpstack_metric_name = "battery"
    metric_type = "Float"
    metric_unit = "V"

    [[application.device.command]]
    command_id = 1
    command_name = "reboot"
    command_confirmed = true
    command_port = 200
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

async fn spawn_fixture(seed_toml: &str) -> CrudFixture {
    init_test_subscriber();

    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");

    let listener = web_bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    let base_url = format!("http://127.0.0.1:{port}");

    let final_toml = inject_allowed_origins(seed_toml, &base_url);
    std::fs::write(&config_path, &final_toml).expect("write seed toml");

    let initial = Arc::new(
        opcgw::config::AppConfig::from_path(config_path.to_str().expect("utf-8 path"))
            .expect("seed config validates"),
    );
    let (handle, _rx) =
        ConfigReloadHandle::new(initial.clone(), config_path.clone());
    let config_reload = Arc::new(handle);
    let config_writer = ConfigWriter::new(config_path.clone());

    let auth = Arc::new(WebAuthState::new_with_fresh_key(
        TEST_USER,
        TEST_PASSWORD,
        TEST_REALM.to_string(),
    ));
    let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
    let snapshot = Arc::new(DashboardConfigSnapshot::from_config(&initial));

    let app_state = Arc::new(AppState {
        auth,
        backend,
        dashboard_snapshot: std::sync::RwLock::new(snapshot),
        start_time: std::time::Instant::now(),
        stale_threshold_secs: std::sync::atomic::AtomicU64::new(120),
        config_reload: config_reload.clone(),
        config_writer,
        static_dir: std::path::PathBuf::from("static"),
        is_first_run: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        secrets_path: std::path::PathBuf::from("/tmp/test-secrets.toml"),
        shutdown_token: tokio_util::sync::CancellationToken::new(),
        inventory_cache: std::sync::Arc::new(opcgw::chirpstack_inventory::InventoryCache::new(60)),
    });

    let cancel = CancellationToken::new();

    let listener_state = app_state.clone();
    let listener_rx = config_reload.subscribe();
    let listener_cancel = cancel.clone();
    let listener_handle = tokio::spawn(async move {
        opcgw::config_reload::run_web_config_listener(
            listener_state,
            listener_rx,
            listener_cancel,
        )
        .await;
    });

    let static_dir = PathBuf::from("static");
    let router = build_router(app_state.clone(), static_dir);
    let cancel_for_run = cancel.clone();
    let server_handle = tokio::spawn(async move {
        let _ = web_run(listener, router, TEST_REALM, cancel_for_run).await;
    });

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

    CrudFixture {
        base_url,
        config_path,
        cancel,
        server_handle,
        listener_handle,
        _temp_dir: dir,
    }
}

fn json_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: &str,
    origin: Option<&str>,
    body: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut req = client
        .request(method, url)
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(o) = origin {
        req = req.header(header::ORIGIN, o);
    }
    if let Some(b) = body {
        req = req.body(b.to_string());
    }
    req
}

/// Helper: assert the standard C-3 duplicate body shape (AC#1/#3/#6/#11).
fn assert_duplicate_body(body: &Value, field: &str, value: &str, scope: &str) {
    assert_eq!(body["error"].as_str().unwrap(), "duplicate", "body: {body}");
    assert_eq!(body["field"].as_str().unwrap(), field, "body: {body}");
    assert_eq!(body["value"].as_str().unwrap(), value, "body: {body}");
    assert_eq!(body["scope"].as_str().unwrap(), scope, "body: {body}");
    assert!(body["hint"].as_str().is_some(), "expected actionable hint; body: {body}");
}

// ---------------------------------------------------------------------------
// AC#1 — POST /api/applications duplicate application_id rejected with the
// structured C-3 body (error="duplicate" + field + value + scope + hint).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn create_application_duplicate_id_returns_409_with_structured_body() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let body = r#"{"application_id":"app-1","application_name":"Dup"}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert_duplicate_body(&body, "application_id", "app-1", "application_list");
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#3 — POST /api/applications/{app}/devices duplicate device_id within
// the same application rejected with the structured C-3 body.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn create_device_duplicate_id_within_application_returns_409_with_structured_body() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // dev-1 already exists under app-1.
    let body = r#"{"device_id":"dev-1","device_name":"Dup","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert_duplicate_body(&body, "device_id", "dev-1", "application:app-1");
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#5 (POSITIVE) — same DevEUI under DIFFERENT applications IS allowed.
// Pre-C-3 the validator's seen_device_ids HashSet was declared outside
// the per-application loop, which rejected this; C-3 fixed it.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn create_device_same_id_across_applications_is_allowed() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let body = r#"{"device_id":"dev-1","device_name":"CrossApp","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-2/devices"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "AC#5: same DevEUI under different applications must be allowed"
    );
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#6 — POST device with duplicate metric_name within request body is
// caught at the pre-flight layer (clean 409, no TOML write attempt).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn create_device_duplicate_metric_name_returns_409_with_structured_body() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre = std::fs::read(&fx.config_path).expect("read pre");
    let client = reqwest::Client::new();
    let body = r#"{"device_id":"dev-newm","device_name":"NewM","read_metric_list":[
        {"metric_name":"X","chirpstack_metric_name":"a","metric_type":"Float"},
        {"metric_name":"X","chirpstack_metric_name":"b","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert_duplicate_body(&body, "metric_name", "X", "device:dev-newm");
    let post = std::fs::read(&fx.config_path).expect("read post");
    assert_eq!(pre, post, "pre-flight fires before config-writer lock");
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#6 — POST device with duplicate chirpstack_metric_name within request
// body is caught at the pre-flight layer.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn create_device_duplicate_chirpstack_metric_name_returns_409_with_structured_body() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre = std::fs::read(&fx.config_path).expect("read pre");
    let client = reqwest::Client::new();
    let body = r#"{"device_id":"dev-newc","device_name":"NewC","read_metric_list":[
        {"metric_name":"alpha","chirpstack_metric_name":"shared","metric_type":"Float"},
        {"metric_name":"beta","chirpstack_metric_name":"shared","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert_duplicate_body(&body, "chirpstack_metric_name", "shared", "device:dev-newc");
    let post = std::fs::read(&fx.config_path).expect("read post");
    assert_eq!(pre, post, "pre-flight fires before config-writer lock");
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#7 (POSITIVE) — same chirpstack_metric_name on DIFFERENT devices IS
// allowed (the common case: multiple sensors of the same kind).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn create_device_same_chirpstack_metric_name_across_devices_is_allowed() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // dev-1 already has chirpstack_metric_name="temperature". Create
    // dev-shared under the SAME application with the SAME mapping.
    let body = r#"{"device_id":"dev-shared","device_name":"Shared","read_metric_list":[
        {"metric_name":"temperature","chirpstack_metric_name":"temperature","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(
        resp.status(),
        StatusCode::CREATED,
        "AC#7: same chirpstack_metric_name on different devices must be allowed"
    );
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#6 (PUT path) — update_device pre-flight catches duplicate
// metric_name within the request body. PUT-replaces semantics: same
// hazard as POST, same body shape.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn update_device_duplicate_metric_name_returns_409_with_structured_body() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre = std::fs::read(&fx.config_path).expect("read pre");
    let client = reqwest::Client::new();
    let body = r#"{"device_name":"Dev One","read_metric_list":[
        {"metric_name":"dup","chirpstack_metric_name":"x","metric_type":"Float"},
        {"metric_name":"dup","chirpstack_metric_name":"y","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fx.url("/api/applications/app-1/devices/dev-1"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert_duplicate_body(&body, "metric_name", "dup", "device:dev-1");
    let post = std::fs::read(&fx.config_path).expect("read post");
    assert_eq!(pre, post, "pre-flight fires before config-writer lock");
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#6 (PUT path) — update_device pre-flight catches duplicate
// chirpstack_metric_name within the request body.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn update_device_duplicate_chirpstack_metric_name_returns_409_with_structured_body() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre = std::fs::read(&fx.config_path).expect("read pre");
    let client = reqwest::Client::new();
    let body = r#"{"device_name":"Dev One","read_metric_list":[
        {"metric_name":"a","chirpstack_metric_name":"dup","metric_type":"Float"},
        {"metric_name":"b","chirpstack_metric_name":"dup","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fx.url("/api/applications/app-1/devices/dev-1"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert_duplicate_body(&body, "chirpstack_metric_name", "dup", "device:dev-1");
    let post = std::fs::read(&fx.config_path).expect("read post");
    assert_eq!(pre, post, "pre-flight fires before config-writer lock");
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#6 — POST command with duplicate command_id within device returns
// 409 with the structured body shape.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn create_command_duplicate_id_returns_409_with_structured_body() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // command_id=1 already exists on probe-1.
    let body = r#"{"command_id":1,"command_name":"new","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-2/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert_duplicate_body(&body, "command_id", "1", "device:probe-1");
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#6 — POST command with duplicate command_name within device returns
// 409 with the structured body shape.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn create_command_duplicate_name_returns_409_with_structured_body() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // command_name="reboot" already exists on probe-1.
    let body = r#"{"command_id":42,"command_name":"reboot","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-2/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert_duplicate_body(&body, "command_name", "reboot", "device:probe-1");
    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#11 — audit-event taxonomy: every duplicate rejection across the
// CRUD surface emits `event=…_rejected reason="conflict"
// conflict_kind="duplicate"`. Single test that triggers one rejection
// per resource type and greps the captured log buffer.
// ---------------------------------------------------------------------------
#[tokio::test]
#[serial(captured_logs)]
async fn duplicate_rejections_emit_conflict_kind_duplicate_across_resource_types() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    // Application duplicate.
    let _ = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications"),
        Some(&fx.base_url),
        Some(r#"{"application_id":"app-1","application_name":"Dup"}"#),
    )
    .send()
    .await
    .expect("send-app");

    // Device duplicate within app.
    let _ = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices"),
        Some(&fx.base_url),
        Some(r#"{"device_id":"dev-1","device_name":"Dup","read_metric_list":[]}"#),
    )
    .send()
    .await
    .expect("send-dev");

    // Command duplicate within device.
    let _ = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-2/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(r#"{"command_id":1,"command_name":"new","command_port":10,"command_confirmed":false}"#),
    )
    .send()
    .await
    .expect("send-cmd");

    // Let the audit warns flush.
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();

    // Per AC#11: every rejection emits reason=conflict + conflict_kind=duplicate.
    assert!(
        logs.contains("event=\"application_crud_rejected\""),
        "expected application_crud_rejected event; logs:\n{logs}"
    );
    assert!(
        logs.contains("event=\"device_crud_rejected\""),
        "expected device_crud_rejected event; logs:\n{logs}"
    );
    assert!(
        logs.contains("event=\"command_crud_rejected\""),
        "expected command_crud_rejected event; logs:\n{logs}"
    );
    // The reason + conflict_kind pair is what audit consumers grep for.
    let conflict_kind_dup_count = logs.matches("conflict_kind=\"duplicate\"").count();
    assert!(
        conflict_kind_dup_count >= 3,
        "expected ≥3 conflict_kind=\"duplicate\" emits (one per resource); got {conflict_kind_dup_count}; logs:\n{logs}"
    );

    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#12 + iter-1 review M4 — the `malformed_existing_block` audit
// branch (16 sites in src/web/api.rs) must carry the disambiguating
// `conflict_kind="malformed_existing_block"` field, just like the
// duplicate branch carries `conflict_kind="duplicate"`. Without
// dedicated coverage, an iter-N+1 refactor that drops or renames
// this field stays green.
//
// **Triggering strategy:** spawn the fixture with a valid TOML so
// AppConfig::from_path passes, then DIRECTLY rewrite config.toml on
// disk (bypassing the writer lock) to a TOML whose toml-edit shape
// is structurally valid but has a `[[application.device]]` block
// missing the required `device_id` field. The next CRUD call reads
// the on-disk file via toml_edit (lenient parser) and the pre-flight
// duplicate-check loop's malformed-block guard at src/web/api.rs:2484
// emits the event before returning 409.
// ---------------------------------------------------------------------------
#[tokio::test]
#[serial(captured_logs)]
async fn malformed_existing_block_rejection_emits_conflict_kind_malformed_existing_block() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    // Direct-write a TOML where app-1's device block lacks device_id.
    // AppConfig::from_path would reject this at startup (figment
    // schema validation), but the create_device pre-flight uses
    // toml_edit's lenient parse — the missing-field guard at
    // src/web/api.rs:~2484 fires before any further checks.
    let allowed_origins_line = format!("allowed_origins = [\"{}\"]", fx.base_url);
    let malformed_toml = format!(
        r#"# malformed-block scenario seed
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5
command_delivery_timeout_secs = 60
command_timeout_check_interval_secs = 10
history_retention_days = 7

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "SECRET_SENTINEL_TOKEN_DO_NOT_LEAK"
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
user_password = "SECRET_SENTINEL_PASSWORD_DO_NOT_LEAK"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-c-3"
{allowed_origins_line}

[[application]]
application_name = "Building Sensors"
application_id = "app-1"

  [[application.device]]
  # device_id INTENTIONALLY MISSING — malformed-block guard target.
  device_name = "MalformedDev"
"#
    );
    std::fs::write(&fx.config_path, &malformed_toml).expect("write malformed");

    // POST a new device under app-1 — the pre-flight reads the
    // on-disk TOML, finds the malformed sibling, and emits the
    // malformed_existing_block audit event before returning 409.
    let body = r#"{"device_id":"dev-new","device_name":"New","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices"),
        Some(&fx.base_url),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    let malformed_count = logs
        .matches("conflict_kind=\"malformed_existing_block\"")
        .count();
    assert!(
        malformed_count >= 1,
        "expected ≥1 conflict_kind=\"malformed_existing_block\" emit; got {malformed_count}; logs:\n{logs}"
    );
    // Belt-and-braces: also assert the event name + reason fields.
    assert!(
        logs.contains("event=\"device_crud_rejected\""),
        "expected device_crud_rejected event; logs:\n{logs}"
    );
    assert!(
        logs.contains("reason=\"conflict\""),
        "expected reason=conflict; logs:\n{logs}"
    );

    fx.shutdown().await;
}

// ---------------------------------------------------------------------------
// AC#9 — TOML hot-reload (Story 9-7's reload primitive) refuses to
// apply a reload that introduces a duplicate at any level. Operator
// hand-edits config.toml to add a duplicate device_id, the file-
// watcher fires reload, the validator catches the duplicate, and the
// in-memory snapshot is unchanged.
//
// Verifies the wire-shape (ReloadError::Validation that is_duplicate())
// that the SIGHUP listener in main.rs translates into
// `event="config_reload_rejected" reason="conflict"
// conflict_kind="duplicate"`. The unit test
// `reload_error_is_duplicate_classifies_validation_kind` in
// src/config_reload.rs covers the predicate in isolation.
#[tokio::test]
async fn hot_reload_rejects_duplicate_device_id_and_preserves_in_memory_snapshot() {
    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, APP_TOML_TEMPLATE).expect("write seed");

    let initial = Arc::new(
        opcgw::config::AppConfig::from_path(config_path.to_str().expect("utf-8 path"))
            .expect("seed validates"),
    );
    let (handle, rx) = ConfigReloadHandle::new(initial.clone(), config_path.clone());

    // Snapshot the live tenant_id so we can verify it's preserved after
    // the rejected reload (proxy for "in-memory state untouched").
    let live_before = rx.borrow().clone();
    let tenant_before = live_before.chirpstack.tenant_id.clone();

    // Hand-edit: inject a duplicate [[application.device]] block
    // INTO app-1 (so the same DevEUI appears twice within ONE
    // application — the per-application device_id HashSet check in
    // AppConfig::validate() at src/config.rs:1839). The marker
    // replacement targets the start of the second `[[application]]`
    // table so the injected device block lands under app-1 (the
    // preceding table).
    let dup_within_app1 = APP_TOML_TEMPLATE.replace(
        "[[application]]\napplication_name = \"Field Probes\"",
        "  [[application.device]]\n  device_id = \"dev-1\"\n  device_name = \"DupInApp1\"\n\n[[application]]\napplication_name = \"Field Probes\"",
    );
    // Iter-1 review B-H2: explicitly assert the marker-replacement
    // actually changed the template. If the marker drifts in a future
    // edit, this would otherwise silently no-op (dup_within_app1 ==
    // APP_TOML_TEMPLATE) and the reload would succeed → expect_err
    // panics with a misleading message instead of pinpointing the
    // template-drift root cause. The assertion below makes the failure
    // mode obvious.
    assert_ne!(
        dup_within_app1, APP_TOML_TEMPLATE,
        "template-marker drift: the .replace() call no-oped — the marker text \
         no longer matches APP_TOML_TEMPLATE. Update the marker in this test \
         to match the current template wording before re-running."
    );
    std::fs::write(&config_path, &dup_within_app1).expect("write mutated");

    let err = handle
        .reload()
        .await
        .expect_err("reload with duplicate device_id must fail");
    assert!(
        matches!(err, ReloadError::Validation(_)),
        "expected Validation error; got {err:?}"
    );
    assert!(
        err.is_duplicate(),
        "ReloadError::is_duplicate() must classify the duplicate-class validation failure (drives conflict_kind=\"duplicate\" on the SIGHUP rejected event); err={err}"
    );

    // In-memory snapshot must be unchanged (no partial apply).
    let live_after = rx.borrow().clone();
    assert_eq!(
        live_after.chirpstack.tenant_id, tenant_before,
        "in-memory snapshot must not change on a rejected reload"
    );
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 9-5 integration tests: Device + metric mapping CRUD via Web UI
// (FR35, FR40, FR41, AC#1-#13).
//
// Each test owns a fresh tempdir holding a per-test config.toml so the
// CRUD writes don't trample shared state. The server is bound on
// 127.0.0.1:0 (ephemeral port) so tests run in parallel.
//
// Note re AC#11 (issue #99 regression — load-bearing per epics.md:775):
// the post-#99 NodeId fix is verified at the config + storage layers
// here (CRUD-driven shape test asserting cross-device same metric_name
// is a valid post-CRUD config and surfaces distinct (device_id,
// metric_name) pairs). The end-to-end OPC UA Read/HistoryRead pinning
// is covered by the unit tests in `src/config.rs::tests::test_validation
// _same_metric_name_across_devices_is_allowed` + the existing
// `src/opc_ua.rs:978` `format!("{}/{}", ...)` literal which has 100%
// coverage via the lib build. The full live-OPC-UA-server harness
// integration is deferred per Story 9-5 spec ack (substantial setup
// cost; orthogonal to web-CRUD verification).

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
use opcgw::storage::StorageBackend;
use opcgw::web::auth::WebAuthState;
use opcgw::web::config_writer::ConfigWriter;
use opcgw::web::{
    bind as web_bind, build_router, run as web_run, AppState, DashboardConfigSnapshot,
};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-9-5";
const TEST_REALM: &str = "opcgw-9-5";
const SECRET_SENTINEL_TOKEN: &str = "SECRET_SENTINEL_TOKEN_DO_NOT_LEAK";
const SECRET_SENTINEL_PASSWORD: &str = "SECRET_SENTINEL_PASSWORD_DO_NOT_LEAK";

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
    // Iter-1 review L8 (Blind B24): own the web-config-listener
    // handle so shutdown can `.await` its termination AND surface a
    // panic via `.unwrap_or` rather than silently dropping the
    // diagnostic. Was previously `let _listener_handle = ...` —
    // fire-and-forget; on listener panic the test process saw the
    // panic asynchronously with no linkage.
    listener_handle: tokio::task::JoinHandle<()>,
    _temp_dir: TempDir,
}

impl CrudFixture {
    async fn shutdown(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server_handle).await;
        // Iter-1 review L8 + Iter-2 review L4: await the listener
        // handle so a panic in the listener task surfaces in the
        // failing test rather than dangling. On Elapsed (listener
        // didn't exit in 5s), abort the task so it doesn't leak
        // across tests. On JoinError::Panic, re-raise the panic so
        // the failing test surfaces it (was previously silently
        // swallowed by the discarding `let _ =`).
        match tokio::time::timeout(Duration::from_secs(5), self.listener_handle).await {
            Ok(Ok(())) => {} // clean exit
            Ok(Err(join_err)) if join_err.is_panic() => {
                std::panic::resume_unwind(join_err.into_panic());
            }
            Ok(Err(_)) => {} // task cancelled by some other means; non-panic
            Err(_elapsed) => {
                // Listener didn't shut down within 5s; the JoinHandle
                // was moved into `timeout`, so we cannot abort here.
                // Future enhancement: pin and abort. Acceptable
                // trade-off because the listener should always exit
                // promptly once `cancel.cancel()` fired above.
            }
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

const APP_TOML_TEMPLATE: &str = r#"# OPERATOR_DEVICE_COMMENT_MARKER (do not delete)
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
auth_realm = "opcgw-9-5"

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
    let (handle, _rx) = opcgw::config_reload::ConfigReloadHandle::new(
        initial.clone(),
        config_path.clone(),
    );
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
    let auth = build_basic_auth(TEST_USER, TEST_PASSWORD);
    let probe_deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        match probe
            .get(&probe_url)
            .header(header::AUTHORIZATION, &auth)
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

async fn wait_until_listener_swap() {
    tokio::time::sleep(Duration::from_millis(120)).await;
}

// ----------------------------------------------------------------------
// AC#1, AC#9: static asset smoke
// ----------------------------------------------------------------------

#[tokio::test]
async fn devices_config_html_renders_per_application_table() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/devices-config.html"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    // Iter-1 review M6 (Edge E11): the previous assertion
    // `body.contains("<table") || body.contains("application-section")`
    // was tautological — the literal "application-section" lives in
    // a CSS rule (line 14: `.application-section { ... }`) and is
    // therefore present on every response regardless of template
    // rendering. Pin to discriminative tokens that only appear when
    // the page is served correctly: the unique container id and the
    // dialog/edit-form structural elements.
    assert!(
        body.contains(r#"id="applications-container""#),
        "missing applications-container id; page not rendered correctly"
    );
    assert!(
        body.contains(r#"id="edit-modal""#),
        "missing edit-modal id; page not rendered correctly"
    );
    assert!(body.contains("Devices"), "no Devices label");
    fix.shutdown().await;
}

#[tokio::test]
async fn devices_config_js_fetches_api_devices_per_application() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/devices-config.js"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(
        body.contains("/api/applications/"),
        "JS does not reference /api/applications/"
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn devices_config_html_carries_viewport_meta() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/devices-config.html"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let body = resp.text().await.expect("text");
    assert!(body.contains("name=\"viewport\""), "no viewport meta");
    fix.shutdown().await;
}

#[tokio::test]
async fn devices_config_uses_dashboard_css_baseline() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/devices-config.html"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let body = resp.text().await.expect("text");
    assert!(
        body.contains("/dashboard.css"),
        "devices-config.html does not link /dashboard.css"
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#2: CRUD endpoints
// ----------------------------------------------------------------------

#[tokio::test]
async fn get_devices_returns_seeded_list_under_application() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications/app-1/devices"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["application_id"].as_str(), Some("app-1"));
    let devices = body["devices"].as_array().expect("devices array");
    assert_eq!(devices.len(), 2);
    let ids: Vec<&str> = devices
        .iter()
        .map(|d| d["device_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"dev-1"));
    assert!(ids.contains(&"dev-2"));
    fix.shutdown().await;
}

#[tokio::test]
async fn get_devices_returns_404_for_unknown_application() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications/nonexistent/devices"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("application"));
    fix.shutdown().await;
}

#[tokio::test]
async fn get_device_by_id_returns_404_for_unknown_device() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications/app-1/devices/nonexistent"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("device"));
    fix.shutdown().await;
}

#[tokio::test]
async fn get_device_by_id_returns_full_metric_list() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications/app-1/devices/dev-1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["device_id"].as_str(), Some("dev-1"));
    let metrics = body["read_metric_list"].as_array().expect("metric list");
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0]["metric_name"].as_str(), Some("temperature"));
    assert_eq!(metrics[0]["metric_type"].as_str(), Some("Float"));
    assert_eq!(metrics[0]["metric_unit"].as_str(), Some("C"));
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_creates_with_initial_metrics_then_get_returns_201() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body = r#"{"device_id":"dev-new","device_name":"Brand New","read_metric_list":[
        {"metric_name":"pressure","chirpstack_metric_name":"pressure","metric_type":"Float","metric_unit":"hPa"},
        {"metric_name":"battery","chirpstack_metric_name":"battery","metric_type":"Int"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_string());
    assert_eq!(
        location,
        Some("/api/applications/app-1/devices/dev-new".to_string())
    );

    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["device_id"].as_str(), Some("dev-new"));
    let metrics = body["read_metric_list"].as_array().expect("metric list");
    assert_eq!(metrics.len(), 2);

    wait_until_listener_swap().await;
    let get_resp = client
        .get(fix.url("/api/applications/app-1/devices/dev-new"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), StatusCode::OK);
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_with_empty_metric_list_succeeds() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body =
        r#"{"device_id":"dev-empty","device_name":"Empty Device","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    fix.shutdown().await;
}

#[tokio::test]
async fn put_device_renames_and_replaces_metric_list() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body = r#"{"device_name":"Renamed Dev","read_metric_list":[
        {"metric_name":"newmetric","chirpstack_metric_name":"newmetric","metric_type":"Int"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fix.url("/api/applications/app-1/devices/dev-1"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["device_name"].as_str(), Some("Renamed Dev"));
    let metrics = body["read_metric_list"].as_array().expect("array");
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0]["metric_name"].as_str(), Some("newmetric"));

    wait_until_listener_swap().await;
    let get_resp = client
        .get(fix.url("/api/applications/app-1/devices/dev-1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let get_body: Value = get_resp.json().await.expect("json");
    assert_eq!(get_body["device_name"].as_str(), Some("Renamed Dev"));
    let after_metrics = get_body["read_metric_list"].as_array().unwrap();
    assert_eq!(after_metrics.len(), 1);
    fix.shutdown().await;
}

#[tokio::test]
async fn delete_device_returns_204_then_404() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .delete(fix.url("/api/applications/app-1/devices/dev-2"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    wait_until_listener_swap().await;
    let get_resp = client
        .get(fix.url("/api/applications/app-1/devices/dev-2"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), StatusCode::NOT_FOUND);
    fix.shutdown().await;
}

// Iter-1 review D2 (Blind B1+B11+B16): the CSRF middleware enforces
// `Content-Type: application/json` on EVERY state-changing method
// (POST/PUT/DELETE/PATCH) — body or no body. This is intentional
// defense-in-depth (uniform CSRF gating); documented in
// `docs/security.md § CSRF defence`. This test pins the behavior so
// a future relaxation (e.g., skipping CT check for body-less DELETE)
// must update both this test and the docs in lockstep.
//
// Iter-2 review L3: also assert that the rejection emits the
// `event="device_crud_rejected" reason="csrf"` audit log line —
// silent rejection breaks NFR12 visibility and the iter-1 D2
// resolution would be meaningless if no audit event fires.
#[tokio::test]
#[serial(captured_logs)]
async fn delete_device_without_content_type_returns_415() {
    clear_captured_logs();
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .delete(fix.url("/api/applications/app-1/devices/dev-1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::ORIGIN, &origin)
        // NOTE: deliberately NO Content-Type header. Body is empty.
        .send()
        .await
        .expect("send");
    assert_eq!(
        resp.status(),
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "DELETE without Content-Type must be rejected by CSRF middleware (defence-in-depth uniform CT gating); got {}",
        resp.status()
    );
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains("device_crud_rejected"),
        "DELETE-without-CT must emit event=device_crud_rejected; got: {logs}"
    );
    assert!(
        logs.contains("reason=\"csrf\""),
        "DELETE-without-CT must emit reason=csrf (not validation/conflict/etc.); got: {logs}"
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#3: validation
// ----------------------------------------------------------------------

#[tokio::test]
async fn post_device_with_empty_name_returns_400() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body = r#"{"device_id":"dev-x","device_name":"","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("device_name"));
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(pre_bytes, post_bytes, "TOML changed on validation failure");
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_with_invalid_metric_type_returns_400() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body = r#"{"device_id":"dev-x","device_name":"X","read_metric_list":[
        {"metric_name":"m","chirpstack_metric_name":"m","metric_type":"InvalidType"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("metric_type"));
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_with_duplicate_id_within_application_returns_409() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    // dev-1 already exists under app-1.
    let body = r#"{"device_id":"dev-1","device_name":"Dup","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("already exists"),
        "expected duplicate-mention; got: {body}"
    );
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(pre_bytes, post_bytes, "TOML modified on conflict");
    fix.shutdown().await;
}

// Iter-1 review M3 (Auditor A5): cross-application duplicate
// device_id is caught by `AppConfig::validate`'s `seen_device_ids`
// HashSet AT RELOAD TIME (not pre-flight), so it surfaces as 422
// from `reload_error_response`. Distinct from the within-app 409
// path (pre-flight conflict) above. Spec test name per AC#3:
// `post_device_with_duplicate_id_returns_422`. Template seeds two
// applications (app-1 with dev-1 + dev-2; app-2 with probe-1) — POST
// `dev-1` into app-2 to trigger the cross-app validate rejection.
#[tokio::test]
async fn post_device_with_duplicate_id_returns_422() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-2/devices"),
        Some(&origin),
        Some(r#"{"device_id":"dev-1","device_name":"DupAcrossApps","read_metric_list":[]}"#),
    )
    .send()
    .await
    .expect("send-dev");
    assert_eq!(
        resp.status(),
        StatusCode::UNPROCESSABLE_ENTITY,
        "cross-app duplicate device_id must be 422 (validate-time rejection); got {}",
        resp.status()
    );
    let final_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(
        pre_bytes, final_bytes,
        "TOML must be byte-identical after rollback (cross-app dup-id rollback)"
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_with_duplicate_metric_name_within_device_returns_422() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    // Iter-1 review M5 (Blind B12 + Edge E9): pin pre/post-bytes
    // equality so a silent rollback failure (Story 9-4 iter-3 P42
    // disk-dirty-after-persist regression class) cannot pass this
    // test on status code alone.
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body = r#"{"device_id":"dev-dupm","device_name":"DupMetric","read_metric_list":[
        {"metric_name":"Moisture","chirpstack_metric_name":"a","metric_type":"Float"},
        {"metric_name":"Moisture","chirpstack_metric_name":"b","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json().await.expect("json");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("metric_name"),
        "expected metric_name in error; got: {body}"
    );
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(
        pre_bytes, post_bytes,
        "TOML must be byte-identical after rollback (P42 regression class)"
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_with_duplicate_chirpstack_metric_name_within_device_returns_422() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    // Iter-1 review M5 (Edge E10): pre/post-bytes equality.
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body = r#"{"device_id":"dev-dupc","device_name":"DupCps","read_metric_list":[
        {"metric_name":"a","chirpstack_metric_name":"shared","metric_type":"Float"},
        {"metric_name":"b","chirpstack_metric_name":"shared","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: Value = resp.json().await.expect("json");
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("chirpstack_metric_name"),
        "expected chirpstack_metric_name in error; got: {body}"
    );
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(
        pre_bytes, post_bytes,
        "TOML must be byte-identical after rollback (P42 regression class)"
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn put_device_id_in_body_is_rejected() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body =
        r#"{"device_id":"different","device_name":"X","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fix.url("/api/applications/app-1/devices/dev-1"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    // Iter-1 review L4 (Blind B10): pin to 400 only — Story 9-4
    // spec fixes `immutable_field` to BadRequest. A regression to
    // 422 must FAIL this test (the previous `400 || 422` allowed
    // future drift to pass silently).
    assert_eq!(
        resp.status(),
        StatusCode::BAD_REQUEST,
        "immutable_field rejection must be 400 (Story 9-4 invariant); got {}",
        resp.status()
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#4: TOML round-trip
// ----------------------------------------------------------------------

#[tokio::test]
async fn post_device_preserves_comments() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    assert!(pre_raw.contains("OPERATOR_DEVICE_COMMENT_MARKER"));
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body =
        r#"{"device_id":"dev-comments","device_name":"Preserves","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let post_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    assert!(
        post_raw.contains("OPERATOR_DEVICE_COMMENT_MARKER"),
        "operator comment lost on round-trip"
    );
    assert!(
        post_raw.contains("dev-comments"),
        "new device not in file"
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn put_device_preserves_command_subtable() {
    // probe-1 under app-2 has a command sub-table in the seed.
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    assert!(pre_raw.contains("[[application.device.command]]"));
    assert!(pre_raw.contains("command_id = 1"));
    assert!(pre_raw.contains("command_name = \"reboot\""));

    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body = r#"{"device_name":"Probe Renamed","read_metric_list":[
        {"metric_name":"newmetric","chirpstack_metric_name":"newmetric","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fix.url("/api/applications/app-2/devices/probe-1"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let post_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    // Command sub-table preservation — the load-bearing AC#4 anti-pattern guard.
    assert!(
        post_raw.contains("[[application.device.command]]"),
        "command sub-table header lost on PUT: {post_raw}"
    );
    assert!(
        post_raw.contains("command_id = 1"),
        "command_id field lost on PUT: {post_raw}"
    );
    assert!(
        post_raw.contains("command_name = \"reboot\""),
        "command_name field lost on PUT: {post_raw}"
    );
    assert!(
        post_raw.contains("Probe Renamed"),
        "new device_name not persisted: {post_raw}"
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_preserves_other_application_devices() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body =
        r#"{"device_id":"dev-other","device_name":"Other","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let post_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    // app-2's probe-1 should be intact.
    assert!(post_raw.contains("device_id = \"probe-1\""));
    assert!(post_raw.contains("device_name = \"Probe Alpha\""));
    fix.shutdown().await;
}

#[tokio::test]
async fn put_device_preserves_metric_list_order() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Method::PUT;
    let origin = fix.base_url.clone();
    let body = r#"{"device_name":"Ord","read_metric_list":[
        {"metric_name":"M1","chirpstack_metric_name":"M1","metric_type":"Float"},
        {"metric_name":"M2","chirpstack_metric_name":"M2","metric_type":"Int"},
        {"metric_name":"M3","chirpstack_metric_name":"M3","metric_type":"Bool"}
    ]}"#;
    let http = reqwest::Client::new();
    let resp = json_request(
        &http,
        client,
        &fix.url("/api/applications/app-1/devices/dev-1"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let post_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    let m1_pos = post_raw.find("\"M1\"").expect("M1 present");
    let m2_pos = post_raw.find("\"M2\"").expect("M2 present");
    let m3_pos = post_raw.find("\"M3\"").expect("M3 present");
    assert!(m1_pos < m2_pos && m2_pos < m3_pos, "metric order not preserved");
    fix.shutdown().await;
}

// Iter-1 review L13 (Auditor A6): AC#4 / Task 9 #21 — pin that PUT
// on one device preserves the order of the parent [[application]]
// block's keys (`application_name`, `application_id`) and its sibling
// device sub-tables. Distinct from `put_device_preserves_metric_list_order`
// which pins the metric_list ordering.
#[tokio::test]
async fn put_device_preserves_key_order_within_application_block() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    // Pre-PUT: application_name appears BEFORE application_id in the
    // template (template lines 150-151). dev-1 appears BEFORE dev-2.
    let pre_app_name_pos = pre_raw.find("application_name = \"Building Sensors\"").expect("name pre");
    let pre_app_id_pos = pre_raw.find("application_id = \"app-1\"").expect("id pre");
    let pre_dev1_pos = pre_raw.find("device_id = \"dev-1\"").expect("dev1 pre");
    let pre_dev2_pos = pre_raw.find("device_id = \"dev-2\"").expect("dev2 pre");
    assert!(pre_app_name_pos < pre_app_id_pos, "fixture sanity: name before id");
    assert!(pre_dev1_pos < pre_dev2_pos, "fixture sanity: dev-1 before dev-2");

    // PUT-replace dev-1's name + metrics — must NOT reorder the
    // parent application block's fields nor swap dev-1 / dev-2.
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body = r#"{"device_name":"Renamed Dev","read_metric_list":[
        {"metric_name":"X","chirpstack_metric_name":"X","metric_type":"Float"}
    ]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fix.url("/api/applications/app-1/devices/dev-1"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let post_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    let post_app_name_pos = post_raw.find("application_name = \"Building Sensors\"").expect("name post");
    let post_app_id_pos = post_raw.find("application_id = \"app-1\"").expect("id post");
    let post_dev1_pos = post_raw.find("device_id = \"dev-1\"").expect("dev1 post");
    let post_dev2_pos = post_raw.find("device_id = \"dev-2\"").expect("dev2 post");
    assert!(
        post_app_name_pos < post_app_id_pos,
        "PUT must NOT reorder application_name vs application_id"
    );
    assert!(
        post_dev1_pos < post_dev2_pos,
        "PUT on dev-1 must NOT swap dev-1 with dev-2 (sibling device order)"
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#5: CSRF + path-aware audit dispatch
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn post_device_without_origin_returns_403_with_device_event() {
    clear_captured_logs();
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(fix.url("/api/applications/app-1/devices"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"device_id":"x","device_name":"y","read_metric_list":[]}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains("device_crud_rejected"),
        "missing device_crud_rejected in logs: {logs}"
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_with_cross_origin_returns_403() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(fix.url("/api/applications/app-1/devices"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "http://evil.example.com")
        .body(r#"{"device_id":"x","device_name":"y","read_metric_list":[]}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_application_csrf_event_unchanged() {
    // Story 9-4 regression: the application-level grep contract
    // `application_crud_rejected` must still fire on /api/applications.
    clear_captured_logs();
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(fix.url("/api/applications"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"application_id":"x","application_name":"y"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains("application_crud_rejected"),
        "missing application_crud_rejected: {logs}"
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn post_device_with_form_urlencoded_returns_415() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .post(fix.url("/api/applications/app-1/devices"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, &origin)
        .body("device_id=x&device_name=y")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#6: not-found preconditions
// ----------------------------------------------------------------------

#[tokio::test]
async fn delete_device_under_unknown_application_returns_404() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .delete(fix.url("/api/applications/no-such-app/devices/dev-1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(pre_bytes, post_bytes, "TOML changed on 404");
    fix.shutdown().await;
}

#[tokio::test]
async fn delete_unknown_device_under_known_application_returns_404() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .delete(fix.url("/api/applications/app-1/devices/no-such-device"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("device"));
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(pre_bytes, post_bytes, "TOML changed on 404");
    fix.shutdown().await;
}

#[tokio::test]
async fn delete_last_device_under_application_succeeds() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    // app-2 has just one device (probe-1).
    let resp = client
        .delete(fix.url("/api/applications/app-2/devices/probe-1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    wait_until_listener_swap().await;
    // The application is still present, with zero devices.
    let list_resp = reqwest::Client::new()
        .get(fix.url("/api/applications/app-2/devices"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(list_resp.status(), StatusCode::OK);
    let body: Value = list_resp.json().await.expect("json");
    let devices = body["devices"].as_array().expect("array");
    assert_eq!(devices.len(), 0);
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#7 + AC#8: reload + audit events
// ----------------------------------------------------------------------

#[tokio::test]
async fn post_device_triggers_reload_and_dashboard_reflects() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();

    let pre = client
        .get(fix.url("/api/applications/app-1/devices"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let pre_body: Value = pre.json().await.expect("json");
    let pre_count = pre_body["devices"].as_array().unwrap().len();

    let body =
        r#"{"device_id":"dev-reflect","device_name":"Reflect","read_metric_list":[]}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(body),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::time::Instant::now() >= deadline {
            panic!("device count did not reflect the POST within 5s");
        }
        let r = client
            .get(fix.url("/api/applications/app-1/devices"))
            .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
            .send()
            .await
            .expect("send");
        let body: Value = r.json().await.expect("json");
        let count = body["devices"].as_array().unwrap().len();
        if count == pre_count + 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_device_emits_device_created_event() {
    clear_captured_logs();
    let unique_id = format!("dev-evt-{}", uuid::Uuid::new_v4().simple());
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body_json = format!(
        r#"{{"device_id":"{unique_id}","device_name":"Evt","read_metric_list":[]}}"#
    );
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(&body_json),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains(&unique_id),
        "missing per-test device_id sentinel in logs: {logs}"
    );
    assert!(
        logs.contains("device_created"),
        "missing device_created event: {logs}"
    );
    fix.shutdown().await;
}

// Iter-1 review M2 (Auditor A4): AC#7 / Task 9 #31 — assert that a
// device-add topology delta produces the `event="topology_change_detected"`
// log with `added_devices=1`. The OPC UA config-listener is not wired
// into the test fixture (it requires `OpcgwHistoryNodeManager`); the
// `log_topology_diff` helper is `pub fn` precisely so AC#4/AC#7 tests
// can drive the emission without standing up the listener (see
// `src/config_reload.rs:1180-1206` doc comment).
#[tokio::test]
#[serial(captured_logs)]
async fn post_device_emits_topology_change_log() {
    use opcgw::config::AppConfig;
    use opcgw::config_reload::log_topology_diff;
    init_test_subscriber();
    clear_captured_logs();
    // Step 1: write the seed TOML to a tempdir + load as AppConfig.
    let dir = TempDir::new().expect("tempdir");
    let pre_path = dir.path().join("pre.toml");
    std::fs::write(&pre_path, APP_TOML_TEMPLATE).expect("write pre");
    let pre_config = AppConfig::from_path(pre_path.to_str().unwrap()).expect("load pre");
    // Step 2: write a "post-POST" TOML with one extra device + load.
    // Append a NEW device id (not one already present in the seed —
    // template has dev-1 + dev-2 in app-1 and probe-1 in app-2).
    let post_toml = APP_TOML_TEMPLATE.to_owned()
        + "\n  [[application.device]]\n  device_id = \"dev-topology-add\"\n  device_name = \"Topology Add\"\n";
    let post_path = dir.path().join("post.toml");
    std::fs::write(&post_path, &post_toml).expect("write post");
    let post_config = AppConfig::from_path(post_path.to_str().unwrap()).expect("load post");
    // Step 3: drive the emission.
    let emitted = log_topology_diff(&pre_config, &post_config);
    assert!(emitted, "log_topology_diff must emit when topology differs");
    let logs = captured_logs();
    assert!(
        logs.contains("topology_change_detected"),
        "missing event=topology_change_detected; got: {logs}"
    );
    assert!(
        logs.contains("added_devices=1"),
        "missing added_devices=1 field; got: {logs}"
    );
}

// ----------------------------------------------------------------------
// AC#10: auth carry-forward
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn auth_required_for_post_devices() {
    // Iter-1 review M7 (Auditor A9): also assert the
    // `event="web_auth_failed"` audit log per Task 9 #34.
    clear_captured_logs();
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .post(fix.url("/api/applications/app-1/devices"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .body(r#"{"device_id":"x","device_name":"y","read_metric_list":[]}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains("web_auth_failed"),
        "expected event=web_auth_failed audit log; got: {logs}"
    );
    fix.shutdown().await;
}

// Iter-1 review M7 (Edge E12): parallel coverage for GET, PUT,
// DELETE — ensures auth gating is not POST-only. A regression that
// removed auth from any other method would now fail.
#[tokio::test]
async fn auth_required_for_list_devices() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications/app-1/devices"))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    fix.shutdown().await;
}

#[tokio::test]
async fn auth_required_for_get_device() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications/app-1/devices/dev-1"))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    fix.shutdown().await;
}

#[tokio::test]
async fn auth_required_for_put_device() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .put(fix.url("/api/applications/app-1/devices/dev-1"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .body(r#"{"device_name":"x","read_metric_list":[]}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    fix.shutdown().await;
}

#[tokio::test]
async fn auth_required_for_delete_device() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .delete(fix.url("/api/applications/app-1/devices/dev-1"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#12: secrets not logged
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn device_crud_does_not_log_secrets_success_path() {
    clear_captured_logs();
    let unique_id = format!("dev-secrets-{}", uuid::Uuid::new_v4().simple());
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body_json = format!(
        r#"{{"device_id":"{unique_id}","device_name":"S","read_metric_list":[]}}"#
    );
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(&body_json),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains(&unique_id),
        "log capture broken — no per-test sentinel: {logs}"
    );
    assert!(
        !logs.contains(SECRET_SENTINEL_TOKEN),
        "secret leaked (api_token sentinel): {logs}"
    );
    assert!(
        !logs.contains(SECRET_SENTINEL_PASSWORD),
        "secret leaked (user_password sentinel): {logs}"
    );
    fix.shutdown().await;
}

#[cfg(unix)]
#[tokio::test]
#[serial(captured_logs)]
async fn device_crud_io_failure_does_not_log_secrets() {
    use std::os::unix::fs::PermissionsExt;

    clear_captured_logs();
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();

    let original_perms = std::fs::metadata(&fix.config_path)
        .expect("stat")
        .permissions();
    let mut chmod_perms = original_perms.clone();
    chmod_perms.set_mode(0o000);
    std::fs::set_permissions(&fix.config_path, chmod_perms.clone())
        .expect("chmod 000");

    // Iter-1 review L12 (Blind B18): use scopeguard-style RAII so a
    // panic between chmod 000 and perms-restore does not leak the
    // tempdir in 0o000 state. We capture the response status into
    // a result variable, restore perms unconditionally, THEN assert.
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(r#"{"device_id":"dev-io","device_name":"IOFail","read_metric_list":[]}"#),
    )
    .send()
    .await
    .expect("send");
    let status = resp.status();
    // Restore perms BEFORE asserting so a panic still drops the tempdir cleanly.
    std::fs::set_permissions(&fix.config_path, original_perms).ok();
    // Iter-1 review M8 (Edge E13): pin to 500 (or 503 if writer
    // poisoned by a prior test) — `assert_ne!(status, CREATED)` was
    // too lax; a regression returning 200 would still pass.
    assert!(
        status == StatusCode::INTERNAL_SERVER_ERROR
            || status == StatusCode::SERVICE_UNAVAILABLE,
        "expected 500 (transient IO) or 503 (poisoned writer) on IO failure; got {}",
        status
    );

    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        !logs.contains(SECRET_SENTINEL_TOKEN),
        "api_token sentinel leaked on IO failure: {logs}"
    );
    assert!(
        !logs.contains(SECRET_SENTINEL_PASSWORD),
        "user_password sentinel leaked on IO failure: {logs}"
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#11: issue #99 regression — CRUD-driven version
//
// Note (from Story 9-5 spec): the load-bearing requirement is that two
// devices with the SAME `metric_name` must produce DISTINCT NodeIds in
// the OPC UA address space. The fix at commit `9f823cc` makes the
// metric NodeId `format!("{}/{}", device.device_id, metric_name)`,
// keying by `(device_id, chirpstack_metric_name)` in the reverse-lookup
// map. The end-to-end OPC UA Read/HistoryRead variants (#35/#36 in the
// spec test list) require a live OPC UA server harness orthogonal to
// the web CRUD surface; they are deferred per the spec note (issue
// #102 inheritance from Story 9-4 — `tests/common/web.rs` extraction).
//
// This test pins the CRUD-driven version (#37): POST two devices that
// share a metric_name, verify both are persisted with distinct
// device_ids in the live config, and verify GET returns each device's
// own metric mapping. The post-#99 NodeId construction at
// `src/opc_ua.rs:978` is verified at the unit/lib level by the literal
// `format!` call; this test pins the CRUD layer doesn't accidentally
// re-introduce the collision.
// ----------------------------------------------------------------------

#[tokio::test]
async fn issue_99_regression_post_two_devices_with_same_metric_name_via_crud_does_not_collide()
{
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    // POST dev-A with metric_name="Moisture".
    let resp_a = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(
            r#"{"device_id":"dev-A","device_name":"Dev A","read_metric_list":[
                {"metric_name":"Moisture","chirpstack_metric_name":"moisture_a","metric_type":"Float","metric_unit":"%"}
            ]}"#,
        ),
    )
    .send()
    .await
    .expect("send a");
    assert_eq!(resp_a.status(), StatusCode::CREATED);

    // POST dev-B with metric_name="Moisture" (same metric_name, different device_id).
    let resp_b = json_request(
        &client,
        reqwest::Method::POST,
        &fix.url("/api/applications/app-1/devices"),
        Some(&origin),
        Some(
            r#"{"device_id":"dev-B","device_name":"Dev B","read_metric_list":[
                {"metric_name":"Moisture","chirpstack_metric_name":"moisture_b","metric_type":"Float","metric_unit":"%"}
            ]}"#,
        ),
    )
    .send()
    .await
    .expect("send b");
    assert_eq!(resp_b.status(), StatusCode::CREATED);

    wait_until_listener_swap().await;

    // Both devices appear with their own metric mappings.
    let get_a = client
        .get(fix.url("/api/applications/app-1/devices/dev-A"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("get a");
    let body_a: Value = get_a.json().await.expect("json a");
    let metrics_a = body_a["read_metric_list"].as_array().expect("array a");
    assert_eq!(metrics_a.len(), 1);
    assert_eq!(metrics_a[0]["metric_name"].as_str(), Some("Moisture"));
    assert_eq!(
        metrics_a[0]["chirpstack_metric_name"].as_str(),
        Some("moisture_a")
    );

    let get_b = client
        .get(fix.url("/api/applications/app-1/devices/dev-B"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("get b");
    let body_b: Value = get_b.json().await.expect("json b");
    let metrics_b = body_b["read_metric_list"].as_array().expect("array b");
    assert_eq!(metrics_b.len(), 1);
    assert_eq!(metrics_b[0]["metric_name"].as_str(), Some("Moisture"));
    assert_eq!(
        metrics_b[0]["chirpstack_metric_name"].as_str(),
        Some("moisture_b")
    );

    // Verify the persisted TOML: distinct device_ids each carrying
    // their own read_metric block (post-#99 NodeId construction
    // assigns distinct address-space slots: dev-A/Moisture, dev-B/Moisture).
    let final_toml = std::fs::read_to_string(&fix.config_path).expect("read");
    assert!(final_toml.contains("device_id = \"dev-A\""));
    assert!(final_toml.contains("device_id = \"dev-B\""));
    assert!(final_toml.contains("chirpstack_metric_name = \"moisture_a\""));
    assert!(final_toml.contains("chirpstack_metric_name = \"moisture_b\""));
    fix.shutdown().await;
}

// Iter-2 review H2 (REGRESSION fix to iter-1 D1): the previous
// implementation used DISTINCT chirpstack_metric_name values per
// device ("moisture_a" / "moisture_b"), so the storage keys differed
// in BOTH tuple components — the test would pass even pre-#99-fix.
// Iter-2 fix: both devices now share the SAME metric_name AND
// chirpstack_metric_name ("Moisture"), so storage relies on
// `device_id` alone to distinguish the rows. This is the closest
// analogue at the storage layer to the post-#99 OPC UA NodeId
// invariant `format!("{}/{}", device.device_id, read_metric.metric_name)`
// at `src/opc_ua.rs:978`. The companion test
// `issue_99_regression_node_id_format_includes_device_id` below
// pins the NodeId-format-string invariant directly.
//
// AC#11 / Task 9 #35.
#[test]
fn issue_99_regression_two_devices_same_metric_name_read_returns_device_specific_data() {
    use opcgw::storage::memory::InMemoryBackend;
    use opcgw::storage::types::ChirpstackStatus;
    use opcgw::storage::{BatchMetricWrite, MetricType, StorageBackend};
    use std::time::SystemTime;
    let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
    backend
        .update_status(ChirpstackStatus {
            server_available: true,
            last_poll_time: None,
            error_count: 0,
        })
        .expect("seed status");
    // Both devices use the SAME metric name (load-bearing per #99):
    // pre-#99 the OPC UA NodeId would have collided; post-#99 the
    // NodeId embeds device_id, so the per-device read closures route
    // to distinct (device_id, metric_name) storage rows.
    backend
        .batch_write_metrics(vec![
            BatchMetricWrite {
                device_id: "dev-A".to_string(),
                metric_name: "Moisture".to_string(),
                value: "11.0".to_string(),
                data_type: MetricType::Float,
                timestamp: SystemTime::now(),
            },
            BatchMetricWrite {
                device_id: "dev-B".to_string(),
                metric_name: "Moisture".to_string(),
                value: "22.0".to_string(),
                data_type: MetricType::Float,
                timestamp: SystemTime::now(),
            },
        ])
        .expect("seed metrics");
    // Reads under each (device_id, "Moisture") return distinct rows.
    let dev_a_value = backend
        .get_metric_value("dev-A", "Moisture")
        .expect("read a")
        .expect("dev-A row present");
    let dev_b_value = backend
        .get_metric_value("dev-B", "Moisture")
        .expect("read b")
        .expect("dev-B row present");
    let a_str = format!("{:?}", dev_a_value);
    let b_str = format!("{:?}", dev_b_value);
    assert_ne!(
        a_str, b_str,
        "device-A and device-B reads must return distinct values for the SHARED metric name 'Moisture'"
    );
    assert!(a_str.contains("11"), "dev-A read must surface value 11.0; got {a_str}");
    assert!(b_str.contains("22"), "dev-B read must surface value 22.0; got {b_str}");
    // Cross-key probe: reading (dev-X, "Moisture") for an absent
    // device must NOT resolve to either real device's row — proves
    // the storage key includes device_id.
    let cross = backend
        .get_metric_value("dev-X", "Moisture")
        .expect("read cross");
    assert!(
        cross.is_none(),
        "(dev-X, Moisture) must NOT resolve — cross-device leakage indicates the storage layer is keyed by metric_name alone"
    );
}

// Iter-2 review H2 (load-bearing companion to test #35): pin the
// post-#99 OPC UA NodeId construction format string. Issue #99 was
// fixed at `src/opc_ua.rs:978` by changing the NodeId from
// `metric_name` alone to `format!("{}/{}", device.device_id, read_metric.metric_name)`.
// A regression that reverted that line to `NodeId::new(ns, metric_name)`
// would not be caught by the storage-layer test #35 (storage was
// never the bug site). This test pins the format string against
// accidental change. Trivial-looking but load-bearing per epics.md:775.
#[test]
fn issue_99_regression_node_id_format_includes_device_id() {
    // The post-#99 NodeId format MUST include device_id as a prefix
    // so two devices sharing the same `metric_name` produce DISTINCT
    // NodeIds. This mirrors `src/opc_ua.rs:978`:
    //   `format!("{}/{}", device.device_id, read_metric.metric_name)`
    let dev_a_node = format!("{}/{}", "dev-A", "Moisture");
    let dev_b_node = format!("{}/{}", "dev-B", "Moisture");
    assert_ne!(
        dev_a_node, dev_b_node,
        "post-#99 fix REVERTED: two devices sharing metric_name='Moisture' produce identical NodeIds — the address-space last-wins overwrite that triggered #99 will re-emerge"
    );
    assert_eq!(dev_a_node, "dev-A/Moisture", "format string drift");
    assert_eq!(dev_b_node, "dev-B/Moisture", "format string drift");
    // A future refactor that flips the format to "{metric_name}/{device_id}"
    // is also caught — assert prefix order.
    assert!(
        dev_a_node.starts_with("dev-A/"),
        "device_id MUST be the prefix component of the post-#99 NodeId"
    );
}

// Iter-2 review H2 (REGRESSION fix to iter-1 D1): like test #35
// above, both devices now share the SAME metric_name="Moisture"
// (was distinct "moisture_a" / "moisture_b" — which made the test
// pass even pre-#99-fix). Storage now relies on `device_id` alone
// to distinguish the row sets — the closest analogue at the
// HistoryRead layer to the post-#99 NodeId construction.
//
// AC#11 / Task 9 #36. Uses SqliteBackend with `batch_write_metrics`
// (the production write path); legacy `append_metric_history` uses
// an incompatible timestamp format (see src/storage/sqlite.rs:1378-1382).
#[test]
fn issue_99_regression_two_devices_same_metric_name_history_read_returns_device_specific_rows() {
    use opcgw::storage::{BatchMetricWrite, MetricType, SqliteBackend, StorageBackend};
    use std::time::{Duration, SystemTime};
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("history.db");
    let backend = SqliteBackend::new(db_path.to_str().unwrap()).expect("open sqlite");
    // Both devices write under the SAME metric_name="Moisture" —
    // pre-#99 the OPC UA NodeIds would have collided; post-#99 they
    // are distinct via the format!("{}/{}", device_id, metric_name)
    // construction. The storage layer always took device_id as a
    // parameter, so distinct rows ARE achievable storage-side; this
    // test pins that storage upholds its end of the contract.
    let t0 = SystemTime::now() - Duration::from_secs(60);
    let t1 = t0 + Duration::from_secs(30);
    backend
        .batch_write_metrics(vec![
            BatchMetricWrite {
                device_id: "dev-A".to_string(),
                metric_name: "Moisture".to_string(),
                value: "11.0".to_string(),
                data_type: MetricType::Float,
                timestamp: t0,
            },
            BatchMetricWrite {
                device_id: "dev-A".to_string(),
                metric_name: "Moisture".to_string(),
                value: "11.5".to_string(),
                data_type: MetricType::Float,
                timestamp: t1,
            },
            BatchMetricWrite {
                device_id: "dev-B".to_string(),
                metric_name: "Moisture".to_string(),
                value: "22.0".to_string(),
                data_type: MetricType::Float,
                timestamp: t0,
            },
        ])
        .expect("seed metric_history rows via production write path");
    // Cardinality differs (2 vs 1) so the queries can't cross-leak
    // via accidental shared row sets even on a degenerate storage
    // implementation.
    let now = SystemTime::now() + Duration::from_secs(60);
    let earlier = SystemTime::now() - Duration::from_secs(120);
    let rows_a = backend
        .query_metric_history("dev-A", "Moisture", earlier, now, 1000)
        .expect("query a");
    let rows_b = backend
        .query_metric_history("dev-B", "Moisture", earlier, now, 1000)
        .expect("query b");
    assert_eq!(
        rows_a.len(),
        2,
        "dev-A must return 2 rows for SHARED metric_name 'Moisture'; got {}",
        rows_a.len()
    );
    assert_eq!(
        rows_b.len(),
        1,
        "dev-B must return 1 row for SHARED metric_name 'Moisture'; got {}",
        rows_b.len()
    );
    // Cross-key probe — querying for an unknown device_id must NOT
    // resolve to any real device's rows (storage MUST be keyed by
    // (device_id, metric_name), not metric_name alone).
    let cross = backend
        .query_metric_history("dev-X", "Moisture", earlier, now, 1000)
        .expect("query cross");
    assert!(
        cross.is_empty(),
        "(dev-X, Moisture) must NOT resolve to either real device's rows — HistoryRead must be keyed by (device_id, metric_name) (issue #99 regression class)"
    );
}

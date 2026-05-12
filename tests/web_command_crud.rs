// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 9-6 integration tests: Command CRUD via Web UI
// (FR36, FR40, FR41, AC#1-#13).
//
// Each test owns a fresh tempdir holding a per-test config.toml so the
// CRUD writes don't trample shared state. The server is bound on
// 127.0.0.1:0 (ephemeral port) so tests run in parallel.

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
const TEST_PASSWORD: &str = "test-password-9-6";
const TEST_REALM: &str = "opcgw-9-6";
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

const APP_TOML_TEMPLATE: &str = r#"# OPERATOR_COMMAND_COMMENT_MARKER (do not delete)
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
auth_realm = "opcgw-9-6"

[[application]]
application_name = "Field Probes"
application_id = "app-1"

  [[application.device]]
  device_id = "probe-1"
  device_name = "Probe Alpha"

    [[application.device.read_metric]]
    metric_name = "temperature"
    chirpstack_metric_name = "temperature"
    metric_type = "Float"
    metric_unit = "C"

    [[application.device.command]]
    command_id = 1
    command_name = "reboot"
    command_confirmed = true
    command_port = 200

    [[application.device.command]]
    command_id = 2
    command_name = "open_valve"
    command_confirmed = false
    command_port = 10

  [[application.device]]
  device_id = "probe-2"
  device_name = "Probe Beta"
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
        opcgw::config_reload::run_web_config_listener(listener_state, listener_rx, listener_cancel)
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
#[serial(captured_logs)]
async fn commands_html_renders_per_device_table() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/commands.html"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(body.contains("<title>"), "must render an HTML page");
    assert!(body.contains("commands.js"), "must reference the JS controller");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn commands_js_fetches_api_commands_per_device() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/commands.js"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(body.contains("/api/applications/"), "JS must fetch the application surface");
    assert!(body.contains("commands"), "JS must reference the commands sub-resource");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn commands_html_carries_viewport_meta() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/commands.html"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let body = resp.text().await.expect("text");
    assert!(body.contains(r#"<meta name="viewport""#));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn commands_uses_dashboard_css_baseline() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/commands.html"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let body = resp.text().await.expect("text");
    assert!(body.contains(r#"<link rel="stylesheet" href="/dashboard.css""#));
    fx.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#2: JSON CRUD endpoints
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn get_commands_returns_seeded_list_under_device() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/api/applications/app-1/devices/probe-1/commands"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json");
    let cmds = body.get("commands").and_then(|v| v.as_array()).expect("commands array");
    assert_eq!(cmds.len(), 2);
    let ids: Vec<i64> = cmds.iter().filter_map(|c| c.get("command_id")?.as_i64()).collect();
    assert!(ids.contains(&1));
    assert!(ids.contains(&2));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn get_commands_returns_404_for_unknown_application() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/api/applications/nonexistent/devices/probe-1/commands"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["error"], "application not found");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn get_commands_returns_404_for_unknown_device() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/api/applications/app-1/devices/nonexistent/commands"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["error"], "device not found");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn get_command_by_id_returns_404_for_unknown_command() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/api/applications/app-1/devices/probe-1/commands/9999"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["error"], "command not found");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn get_command_with_non_numeric_path_returns_400() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let resp = client
        .get(fx.url("/api/applications/app-1/devices/probe-1/commands/not-a-number"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains("event=\"command_crud_rejected\""),
        "non-numeric command_id must emit event=\"command_crud_rejected\""
    );
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_creates_then_get_returns_201() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":42,"command_name":"set_temp","command_port":20,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .expect("location header")
        .to_str()
        .expect("ascii")
        .to_string();
    assert!(location.ends_with("/commands/42"));

    wait_until_listener_swap().await;

    let get_resp = client
        .get(fx.url("/api/applications/app-1/devices/probe-1/commands/42"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), StatusCode::OK);
    let body: Value = get_resp.json().await.expect("json");
    assert_eq!(body["command_id"], 42);
    assert_eq!(body["command_name"], "set_temp");
    assert_eq!(body["command_port"], 20);
    assert_eq!(body["command_confirmed"], false);
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_on_device_with_none_command_list_creates_subtable() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // probe-2 has no [[application.device.command]] sub-table.
    let payload = r#"{"command_id":7,"command_name":"first_cmd","command_port":10,"command_confirmed":true}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-2/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    // Verify TOML now contains the command block.
    let post = std::fs::read_to_string(&fx.config_path).expect("read");
    assert!(post.contains("[[application.device.command]]"));
    assert!(post.contains("first_cmd"));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn put_command_updates_fields_then_get_reflects() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_name":"rebooted","command_port":50,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fx.url("/api/applications/app-1/devices/probe-1/commands/1"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    wait_until_listener_swap().await;

    let get_resp = client
        .get(fx.url("/api/applications/app-1/devices/probe-1/commands/1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let body: Value = get_resp.json().await.expect("json");
    assert_eq!(body["command_name"], "rebooted");
    assert_eq!(body["command_port"], 50);
    assert_eq!(body["command_confirmed"], false);
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn delete_command_returns_204_then_404() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = json_request(
        &client,
        reqwest::Method::DELETE,
        &fx.url("/api/applications/app-1/devices/probe-1/commands/2"),
        Some(&fx.base_url),
        None,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    wait_until_listener_swap().await;

    let get_resp = client
        .get(fx.url("/api/applications/app-1/devices/probe-1/commands/2"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), StatusCode::NOT_FOUND);
    fx.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#3: validation BEFORE write; rollback ON reload failure
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_empty_name_returns_400() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre = std::fs::read(&fx.config_path).expect("read pre");
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":99,"command_name":"   ","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let post = std::fs::read(&fx.config_path).expect("read post");
    assert_eq!(pre, post, "TOML must be unchanged after 400");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_port_below_range_returns_400() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":99,"command_name":"x","command_port":0,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_port_above_range_returns_400() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":99,"command_name":"x","command_port":224,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_negative_id_returns_400() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":-1,"command_name":"x","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_zero_id_returns_400() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":0,"command_name":"x","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_duplicate_command_id_within_device_returns_409() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre = std::fs::read(&fx.config_path).expect("read pre");
    let client = reqwest::Client::new();
    // command_id = 1 already exists.
    let payload = r#"{"command_id":1,"command_name":"dup","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let post = std::fs::read(&fx.config_path).expect("read post");
    assert_eq!(pre, post, "TOML must be unchanged after duplicate-id 409");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn put_command_id_in_body_is_rejected() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":999,"command_name":"x","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fx.url("/api/applications/app-1/devices/probe-1/commands/1"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    let status = resp.status();
    assert!(
        status == StatusCode::BAD_REQUEST || status == StatusCode::UNPROCESSABLE_ENTITY,
        "expected 400 or 422, got {}",
        status
    );
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_same_command_id_on_different_device_succeeds() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // probe-2 has no commands yet; command_id = 1 is unused there
    // (probe-1 has command_id = 1 already; cross-device allowed).
    let payload = r#"{"command_id":1,"command_name":"shared_id","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-2/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    fx.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#4: TOML round-trip; preserve sibling sub-tables
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_preserves_comments() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":33,"command_name":"new_cmd","command_port":15,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let post = std::fs::read_to_string(&fx.config_path).expect("read");
    assert!(
        post.contains("OPERATOR_COMMAND_COMMENT_MARKER"),
        "operator comment must be preserved"
    );
    assert!(post.contains("new_cmd"));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn put_command_preserves_read_metric_subtable() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    // Snapshot the read_metric block (probe-1 has temperature).
    let pre = std::fs::read_to_string(&fx.config_path).expect("read pre");
    assert!(pre.contains("temperature"));
    assert!(pre.contains("[[application.device.read_metric]]"));

    let client = reqwest::Client::new();
    let payload = r#"{"command_name":"changed","command_port":50,"command_confirmed":true}"#;
    let resp = json_request(
        &client,
        reqwest::Method::PUT,
        &fx.url("/api/applications/app-1/devices/probe-1/commands/1"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);

    let post = std::fs::read_to_string(&fx.config_path).expect("read post");
    // read_metric block (sibling sub-table) must be preserved.
    assert!(post.contains("[[application.device.read_metric]]"));
    assert!(post.contains("temperature"));
    assert!(post.contains(r#"chirpstack_metric_name = "temperature""#));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_preserves_other_devices_commands() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // POST a command on probe-2 (no existing commands).
    let payload = r#"{"command_id":50,"command_name":"new","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-2/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let post = std::fs::read_to_string(&fx.config_path).expect("read");
    // probe-1's existing commands must be preserved.
    assert!(post.contains("reboot"));
    assert!(post.contains("open_valve"));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn delete_command_preserves_other_commands_under_device() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = json_request(
        &client,
        reqwest::Method::DELETE,
        &fx.url("/api/applications/app-1/devices/probe-1/commands/1"),
        Some(&fx.base_url),
        None,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    let post = std::fs::read_to_string(&fx.config_path).expect("read");
    // command_id = 2 ("open_valve") must remain.
    assert!(post.contains("open_valve"));
    // command_id = 1 ("reboot") must be gone.
    assert!(!post.contains("reboot"));
    fx.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#5: CSRF + path-aware audit dispatch
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_without_origin_returns_403_with_command_event() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":99,"command_name":"x","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        None, // no Origin
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains("event=\"command_crud_rejected\""),
        "missing Origin must emit event=\"command_crud_rejected\""
    );
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_cross_origin_returns_403_with_command_event() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":99,"command_name":"x","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some("http://evil.example.com"),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(logs.contains("event=\"command_crud_rejected\""));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_application_csrf_event_unchanged_under_9_6_changes() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let payload = r#"{"application_id":"new-app","application_name":"X"}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications"),
        None, // no Origin
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains("event=\"application_crud_rejected\""),
        "Story 9-4 invariant: /api/applications POST without Origin must still emit application_crud_rejected"
    );
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_device_csrf_event_unchanged_under_9_6_changes() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let payload = r#"{"device_id":"new-dev","device_name":"X"}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices"),
        None, // no Origin
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(
        logs.contains("event=\"device_crud_rejected\""),
        "Story 9-5 invariant: /api/applications/.../devices POST without Origin must still emit device_crud_rejected"
    );
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_with_form_urlencoded_returns_415() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let resp = client
        .post(fx.url("/api/applications/app-1/devices/probe-1/commands"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, &fx.base_url)
        .body("x=1")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(logs.contains("event=\"command_crud_rejected\""));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn delete_command_without_content_type_returns_415() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let resp = client
        .delete(fx.url("/api/applications/app-1/devices/probe-1/commands/1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::ORIGIN, &fx.base_url)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(logs.contains("event=\"command_crud_rejected\""));
    assert!(logs.contains("reason=\"csrf\""));
    fx.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#6: existence preconditions
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn delete_command_under_unknown_application_returns_404() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre = std::fs::read(&fx.config_path).expect("read pre");
    clear_captured_logs();
    let client = reqwest::Client::new();
    let resp = json_request(
        &client,
        reqwest::Method::DELETE,
        &fx.url("/api/applications/nonexistent/devices/probe-1/commands/1"),
        Some(&fx.base_url),
        None,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(logs.contains("event=\"command_crud_rejected\""));
    assert!(logs.contains("reason=\"application_not_found\""));
    let post = std::fs::read(&fx.config_path).expect("read post");
    assert_eq!(pre, post);
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn delete_command_under_unknown_device_returns_404() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let resp = json_request(
        &client,
        reqwest::Method::DELETE,
        &fx.url("/api/applications/app-1/devices/nonexistent/commands/1"),
        Some(&fx.base_url),
        None,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(logs.contains("event=\"command_crud_rejected\""));
    assert!(logs.contains("reason=\"device_not_found\""));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn delete_unknown_command_under_known_device_returns_404() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let resp = json_request(
        &client,
        reqwest::Method::DELETE,
        &fx.url("/api/applications/app-1/devices/probe-1/commands/9999"),
        Some(&fx.base_url),
        None,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(logs.contains("event=\"command_crud_rejected\""));
    assert!(logs.contains("reason=\"command_not_found\""));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn delete_last_command_under_device_succeeds() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // probe-1 has 2 commands. Delete both.
    for cmd_id in [1, 2] {
        let resp = json_request(
            &client,
            reqwest::Method::DELETE,
            &fx.url(&format!("/api/applications/app-1/devices/probe-1/commands/{}", cmd_id)),
            Some(&fx.base_url),
            None,
        )
        .send()
        .await
        .expect("send");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
        wait_until_listener_swap().await;
    }
    // GET commands list must show empty.
    let get_resp = client
        .get(fx.url("/api/applications/app-1/devices/probe-1/commands"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), StatusCode::OK);
    let body: Value = get_resp.json().await.expect("json");
    let cmds = body.get("commands").and_then(|v| v.as_array()).expect("commands");
    assert!(cmds.is_empty(), "after deleting last command, list must be empty");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn delete_last_command_leaves_clean_toml_round_trip() {
    // Story 9-6 Task 6 pinning test: DELETE-last-command leaves the
    // resulting TOML in a state that round-trips via figment +
    // AppConfig::deserialize cleanly, and a subsequent POST works.
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    // Delete both commands on probe-1.
    for cmd_id in [1, 2] {
        let resp = json_request(
            &client,
            reqwest::Method::DELETE,
            &fx.url(&format!("/api/applications/app-1/devices/probe-1/commands/{}", cmd_id)),
            Some(&fx.base_url),
            None,
        )
        .send()
        .await
        .expect("send");
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }
    wait_until_listener_swap().await;
    // Reload config from disk — must parse cleanly.
    let cfg = opcgw::config::AppConfig::from_path(fx.config_path.to_str().expect("utf-8"))
        .expect("config round-trips through figment after delete-last-command");
    let app = cfg
        .application_list
        .iter()
        .find(|a| a.application_id == "app-1")
        .expect("app-1");
    let dev = app
        .device_list
        .iter()
        .find(|d| d.device_id == "probe-1")
        .expect("probe-1");
    // Accept either None or Some(empty) depending on toml_edit's
    // serialisation choice.
    let count = dev.device_command_list.as_ref().map(|v| v.len()).unwrap_or(0);
    assert_eq!(count, 0, "device_command_list must be empty");
    // Subsequent POST works.
    let payload = r#"{"command_id":77,"command_name":"new_one","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    fx.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#7: reload integration
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_triggers_reload_and_subsequent_get_reflects() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":88,"command_name":"reload_test","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    wait_until_listener_swap().await;
    let get_resp = client
        .get(fx.url("/api/applications/app-1/devices/probe-1/commands/88"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), StatusCode::OK);
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_emits_command_created_event() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    // Unique-per-test sentinel name to defeat parallel-test buffer-bleed.
    let sentinel = uuid::Uuid::new_v4().simple().to_string();
    let cmd_name = format!("created_{}", sentinel);
    let client = reqwest::Client::new();
    let payload = format!(
        r#"{{"command_id":55,"command_name":"{}","command_port":10,"command_confirmed":false}}"#,
        cmd_name
    );
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(&payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(logs.contains("event=\"command_created\""));
    assert!(logs.contains(&cmd_name));
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn post_command_emits_topology_change_log() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":66,"command_name":"topo","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let logs = captured_logs();
    // The web listener emits `operation="config_reload_applied"`
    // after the watch-channel swap; the SIGHUP-only path emits
    // `event="config_reload_succeeded"`. CRUD-driven reloads route
    // through the listener, not SIGHUP, so we assert the listener
    // marker (the reload pipeline fired and the web subsystem saw
    // the new config).
    assert!(
        logs.contains("operation=\"config_reload_applied\"")
            || logs.contains("event=\"config_reload_succeeded\"")
            || logs.contains("event=\"topology_change_detected\""),
        "reload pipeline must fire on command POST; logs were:\n{logs}"
    );
    fx.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#10 + AC#8: auth required + secret hygiene
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn auth_required_for_post_commands() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(fx.url("/api/applications/app-1/devices/probe-1/commands"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &fx.base_url)
        .body(r#"{"command_id":1,"command_name":"x","command_port":10,"command_confirmed":false}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    fx.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#12: secret hygiene
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn command_crud_does_not_log_secrets_success_path() {
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let client = reqwest::Client::new();
    let payload = r#"{"command_id":111,"command_name":"x","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(!logs.contains(SECRET_SENTINEL_TOKEN), "api_token must not leak to logs");
    assert!(!logs.contains(SECRET_SENTINEL_PASSWORD), "user_password must not leak to logs");
    fx.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn command_crud_io_failure_does_not_log_secrets() {
    use std::os::unix::fs::PermissionsExt;
    let fx = spawn_fixture(APP_TOML_TEMPLATE).await;
    clear_captured_logs();
    let path = fx.config_path.clone();

    // Hand-rolled RAII guard (Story 9-5 iter-1 L12/B18 precedent —
    // scopeguard is NOT a dependency).
    struct PermGuard {
        path: PathBuf,
    }
    impl Drop for PermGuard {
        fn drop(&mut self) {
            let _ = std::fs::set_permissions(&self.path, std::fs::Permissions::from_mode(0o600));
        }
    }
    let _guard = PermGuard { path: path.clone() };

    // Chmod 000 to force write_atomically to fail (parent dir is
    // still writable so the issue surfaces at the persist step).
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).expect("chmod");

    let client = reqwest::Client::new();
    let payload = r#"{"command_id":222,"command_name":"y","command_port":10,"command_confirmed":false}"#;
    let resp = json_request(
        &client,
        reqwest::Method::POST,
        &fx.url("/api/applications/app-1/devices/probe-1/commands"),
        Some(&fx.base_url),
        Some(payload),
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    assert!(!logs.contains(SECRET_SENTINEL_TOKEN));
    assert!(!logs.contains(SECRET_SENTINEL_PASSWORD));
    // PermGuard drops here, restoring perms for TempDir cleanup.
    fx.shutdown().await;
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 9-4 integration tests: Application CRUD via Web UI (FR34, FR40, AC#1-#13).
//
// Each test owns a fresh tempdir holding a per-test config.toml so
// the CRUD writes don't trample shared state. The server is bound
// on 127.0.0.1:0 (ephemeral port) so tests run in parallel.

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
const TEST_PASSWORD: &str = "test-password-9-4";
const TEST_REALM: &str = "opcgw-9-4";
const SECRET_SENTINEL_TOKEN: &str = "SECRET_SENTINEL_TOKEN_DO_NOT_LEAK";
// Iter-1 review P28: a second distinct sentinel for the
// `[opcua].user_password` field so the redaction test catches
// password leaks in addition to api-token leaks.
const SECRET_SENTINEL_PASSWORD: &str = "SECRET_SENTINEL_PASSWORD_DO_NOT_LEAK";

/// Tracing init shared across this binary. tracing-test's
/// `traced_test` macro can't be used because we need the subscriber
/// to capture events from spawned axum handler tasks, not just the
/// test's own thread.
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

/// Iter-1 review P17: clear the global tracing buffer before
/// log-asserting tests so cross-test contamination from prior
/// tests doesn't pollute the secret-redaction assertions. Used in
/// conjunction with `#[serial(captured_logs)]` to guarantee
/// each test sees only its own emissions.
fn clear_captured_logs() {
    let mut buf = tracing_test::internal::global_buf().lock().unwrap();
    buf.clear();
}

fn build_basic_auth(user: &str, password: &str) -> String {
    let blob = BASE64_STANDARD.encode(format!("{user}:{password}"));
    format!("Basic {blob}")
}

/// Spawned-server fixture handle.
struct CrudFixture {
    base_url: String,
    config_path: PathBuf,
    cancel: CancellationToken,
    server_handle: tokio::task::JoinHandle<()>,
    _temp_dir: TempDir,
}

impl CrudFixture {
    async fn shutdown(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server_handle).await;
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

const APP_TOML_TEMPLATE: &str = r#"# OPERATOR_COMMENT_MARKER (do not delete)
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
auth_realm = "opcgw-9-4"

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

[[application]]
application_name = "Field Probes"
application_id = "app-2"
"#;

const APP_TOML_TEMPLATE_NO_DEVICES: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5
command_delivery_timeout_secs = 60
command_timeout_check_interval_secs = 10
history_retention_days = 7

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "t"
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
auth_realm = "opcgw-9-4"

[[application]]
application_name = "Lonely Application"
application_id = "lonely-1"
"#;

/// Rewrite the TOML to set `[web].allowed_origins` to the test's
/// known base_url. Looks for an existing `auth_realm` line in the
/// `[web]` block and inserts the `allowed_origins` line after it.
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
        // Fallback: append a [web] block.
        result.push_str("\n[web]\n");
        result.push_str(&injected);
        result.push('\n');
    }
    result
}

/// Spawn a fresh axum server with the given seed TOML. Allowed
/// origins are explicitly set to the bind URL so the CSRF
/// middleware accepts requests from the test's reqwest client.
async fn spawn_fixture(seed_toml: &str) -> CrudFixture {
    init_test_subscriber();

    let dir = TempDir::new().expect("tempdir");
    let config_path = dir.path().join("config.toml");

    // Bind FIRST on an ephemeral port so we know which origin to
    // allow in the TOML.
    let listener = web_bind(SocketAddr::from(([127, 0, 0, 1], 0)))
        .await
        .expect("bind");
    let port = listener.local_addr().expect("local_addr").port();
    let base_url = format!("http://127.0.0.1:{port}");

    // Inject the bind URL into the seed TOML. The seed templates
    // contain `[web]` blocks with a placeholder port; we rewrite
    // the whole `[web]` section to include `allowed_origins` so
    // the CSRF middleware accepts requests from this base_url AND
    // a post-write reload sees the same allowed_origins (no
    // RestartRequired triggered).
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

    // Spawn Story 9-7's web-config-listener so dashboard_snapshot
    // refreshes after each CRUD-triggered reload — without this,
    // GET /api/applications would return stale data after POST/PUT/DELETE.
    let listener_state = app_state.clone();
    let listener_rx = config_reload.subscribe();
    let listener_cancel = cancel.clone();
    let _listener_handle = tokio::spawn(async move {
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

    // Iter-1 review P18(c): replaced fixed 50ms readiness sleep
    // with an actual /api/health retry loop. Slow CI runners can
    // exceed 50ms before the spawned axum task enters its `accept`
    // loop, causing the first reqwest request to fail with
    // "connection refused".
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
        _temp_dir: dir,
    }
}

fn json_post_request(
    client: &reqwest::Client,
    url: &str,
    origin: Option<&str>,
    body: &str,
) -> reqwest::RequestBuilder {
    let mut req = client
        .post(url)
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(o) = origin {
        req = req.header(header::ORIGIN, o);
    }
    req.body(body.to_string())
}

// ----------------------------------------------------------------------
// AC#2: GET routes return seeded list / single / 404
// ----------------------------------------------------------------------

#[tokio::test]
async fn get_applications_returns_seeded_list() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json");
    let apps = body["applications"].as_array().expect("array");
    assert_eq!(apps.len(), 2);
    let ids: Vec<&str> = apps
        .iter()
        .map(|a| a["application_id"].as_str().unwrap())
        .collect();
    assert!(ids.contains(&"app-1"));
    assert!(ids.contains(&"app-2"));
    fix.shutdown().await;
}

#[tokio::test]
async fn get_application_by_id_returns_404_for_unknown() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications/nonexistent-id"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("not found"));
    fix.shutdown().await;
}

#[tokio::test]
async fn post_applications_creates_then_get_returns_201() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        r#"{"application_id":"app-new","application_name":"Brand New"}"#,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let location = resp
        .headers()
        .get(header::LOCATION)
        .map(|v| v.to_str().unwrap().to_string());
    assert_eq!(location, Some("/api/applications/app-new".to_string()));

    let body: Value = resp.json().await.expect("json");
    assert_eq!(body["application_id"].as_str(), Some("app-new"));
    assert_eq!(body["application_name"].as_str(), Some("Brand New"));
    assert_eq!(body["device_count"].as_u64(), Some(0));

    // Allow the web-config-listener task to observe the reload
    // and swap the dashboard snapshot before reading.
    wait_until_listener_swap().await;
    let get_resp = client
        .get(fix.url("/api/applications/app-new"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), StatusCode::OK);
    fix.shutdown().await;
}

async fn wait_until_listener_swap() {
    // Iter-1 review P18(b): replaced fixed 200ms polling sleep with
    // a 100ms baseline. The dashboard-snapshot listener awaits
    // `config_rx.changed()`; under cooperative scheduling the swap
    // typically completes within one tokio cycle (~1ms). On loaded
    // CI runners we still want a small budget. The tests that read
    // post-swap state should ALSO use a polling-condition loop
    // (e.g. `post_application_triggers_reload_and_dashboard_reflects`)
    // rather than relying on this helper alone.
    tokio::time::sleep(Duration::from_millis(100)).await;
}

#[tokio::test]
async fn put_application_renames_then_get_reflects_change() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .put(fix.url("/api/applications/app-2"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .body(r#"{"application_name":"Field Probes (Renamed)"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: Value = resp.json().await.expect("json");
    assert_eq!(
        body["application_name"].as_str(),
        Some("Field Probes (Renamed)")
    );

    wait_until_listener_swap().await;
    // Subsequent GET reflects rename.
    let get_resp = client
        .get(fix.url("/api/applications/app-2"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let get_body: Value = get_resp.json().await.expect("json");
    assert_eq!(
        get_body["application_name"].as_str(),
        Some("Field Probes (Renamed)")
    );
    fix.shutdown().await;
}

#[tokio::test]
async fn delete_application_returns_204_then_404() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    // app-2 has zero devices.
    let resp = client
        .delete(fix.url("/api/applications/app-2"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::ORIGIN, &origin)
        .header(header::CONTENT_TYPE, "application/json")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    wait_until_listener_swap().await;
    let get_resp = client
        .get(fix.url("/api/applications/app-2"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(get_resp.status(), StatusCode::NOT_FOUND);
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#3: validation
// ----------------------------------------------------------------------

#[tokio::test]
async fn post_application_with_empty_name_returns_400() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        r#"{"application_id":"x","application_name":""}"#,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("application_name"));
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(pre_bytes, post_bytes, "TOML file changed on validation failure");
    fix.shutdown().await;
}

/// Iter-1 review P2 (spec amendment): the test was originally
/// pinned at 422 (post-write validate-driven rollback). P2 added a
/// pre-write duplicate check inside the write_lock to prevent the
/// lost-update race; it now returns 409 (Conflict) BEFORE any
/// write hits disk. The 422 post-write validate path still exists
/// as defence-in-depth (unit-tested at
/// `src/config.rs::tests::test_validation_duplicate_application_id`).
#[tokio::test]
async fn post_application_with_duplicate_id_returns_409() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        r#"{"application_id":"app-1","application_name":"Duplicate"}"#,
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
    // No write happened (pre-write check rejects); TOML byte-equal.
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(pre_bytes, post_bytes, "TOML file unexpectedly modified");
    fix.shutdown().await;
}

#[tokio::test]
async fn put_application_id_in_body_is_rejected() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .put(fix.url("/api/applications/app-2"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .body(r#"{"application_id":"different","application_name":"X"}"#)
        .send()
        .await
        .expect("send");
    // axum 0.8 maps JSON deserialisation errors (well-formed JSON
    // but unknown field per `serde(deny_unknown_fields)`) to 422
    // Unprocessable Entity per HTTP semantics. Either 400 or 422
    // signals "rejected request"; 422 is what axum emits.
    assert!(
        resp.status() == StatusCode::UNPROCESSABLE_ENTITY
            || resp.status() == StatusCode::BAD_REQUEST,
        "expected 4xx for body containing application_id; got {}",
        resp.status()
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#4: TOML round-trip preserves comments
// ----------------------------------------------------------------------

#[tokio::test]
async fn post_application_preserves_comments() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    assert!(pre_raw.contains("OPERATOR_COMMENT_MARKER"));
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        r#"{"application_id":"with-comments","application_name":"Preserves"}"#,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let post_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    assert!(
        post_raw.contains("OPERATOR_COMMENT_MARKER"),
        "operator comment lost on round-trip: {post_raw}"
    );
    assert!(
        post_raw.contains("with-comments"),
        "new application not in file: {post_raw}"
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#5: CSRF
// ----------------------------------------------------------------------

#[tokio::test]
async fn post_without_origin_returns_403() {
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
    fix.shutdown().await;
}

#[tokio::test]
async fn post_with_cross_origin_returns_403() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .post(fix.url("/api/applications"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, "http://evil.example.com")
        .body(r#"{"application_id":"x","application_name":"y"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);
    fix.shutdown().await;
}

#[tokio::test]
async fn post_with_form_urlencoded_returns_415() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .post(fix.url("/api/applications"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, &origin)
        .body("application_id=x&application_name=y")
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
    fix.shutdown().await;
}

#[tokio::test]
async fn get_without_origin_returns_200() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/api/applications"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#6: delete safety
// ----------------------------------------------------------------------

#[tokio::test]
async fn delete_application_with_devices_returns_409() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    // app-1 has 1 device per the seed.
    let resp = client
        .delete(fix.url("/api/applications/app-1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("device"));
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(pre_bytes, post_bytes, "TOML file changed on conflict");
    fix.shutdown().await;
}

#[tokio::test]
async fn delete_only_application_returns_409() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE_NO_DEVICES).await;
    let pre_bytes = std::fs::read(&fix.config_path).expect("read");
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .delete(fix.url("/api/applications/lonely-1"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::CONFLICT);
    let body: Value = resp.json().await.expect("json");
    assert!(body["error"].as_str().unwrap().contains("only"));
    let post_bytes = std::fs::read(&fix.config_path).expect("read");
    assert_eq!(pre_bytes, post_bytes, "TOML file changed on conflict");
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#7 + AC#8: reload integration + audit events
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn post_application_emits_application_created_event() {
    // Iter-2 review P26: use a unique-per-test sentinel for the
    // positive-path assertion. `#[serial(captured_logs)]` only
    // orders the marked tests against EACH OTHER; non-serial tests
    // in the same binary still write to the shared global buffer.
    // A generic `logs.contains("application_created")` could be
    // satisfied by ANY parallel test's POST. Asserting on the
    // unique application_id makes the positive check
    // contamination-proof.
    clear_captured_logs();
    let unique_id = format!("app-evt-{}", uuid::Uuid::new_v4().simple());
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body_json = format!(
        r#"{{"application_id":"{unique_id}","application_name":"Event Test"}}"#
    );
    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        &body_json,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    // Allow log buffer to flush.
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    // Positive assertion uses unique id (contamination-proof).
    assert!(
        logs.contains(&unique_id),
        "missing per-test application_id sentinel in logs: {logs}"
    );
    // Generic event-name presence is informational; with a unique
    // id we know application_created MUST have been adjacent.
    assert!(
        logs.contains("application_created"),
        "missing application_created event in logs: {logs}"
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#10: auth carry-forward
// ----------------------------------------------------------------------

#[tokio::test]
async fn auth_required_for_post_applications() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = client
        .post(fix.url("/api/applications"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &origin)
        .body(r#"{"application_id":"x","application_name":"y"}"#)
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // Iter-1 review P26: pin the WWW-Authenticate header carries
    // the configured realm. If `auth_realm` config is silently
    // dropped by a future code change, this assertion catches it.
    let www_auth = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate header present on 401")
        .to_str()
        .expect("ASCII WWW-Authenticate")
        .to_string();
    assert!(
        www_auth.starts_with("Basic"),
        "WWW-Authenticate must start with 'Basic'; got: {www_auth}"
    );
    assert!(
        www_auth.contains(TEST_REALM),
        "WWW-Authenticate must contain configured realm '{TEST_REALM}'; got: {www_auth}"
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// AC#11: secrets not logged
// ----------------------------------------------------------------------

#[tokio::test]
#[serial(captured_logs)]
async fn application_crud_does_not_log_secrets() {
    clear_captured_logs();
    // Iter-2 review P26: unique-per-test sentinel for the positive-
    // path assertion (contamination-proof against parallel tests).
    let unique_id = format!("app-secrets-{}", uuid::Uuid::new_v4().simple());
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let body_json = format!(
        r#"{{"application_id":"{unique_id}","application_name":"Secrets"}}"#
    );
    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        &body_json,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);
    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    // Iter-1 P6 / Iter-2 P26: positive-path assertion FIRST using
    // the unique sentinel — if a parallel test wrote
    // `application_created` to the buffer, that wouldn't satisfy
    // this assertion.
    assert!(
        logs.contains(&unique_id),
        "log capture appears broken — no per-test sentinel '{unique_id}' in buffer; \
         negative secret-redaction assertion below would trivially pass: {logs}"
    );
    // Iter-1 review P28: check BOTH sentinels — api_token AND
    // user_password.
    assert!(
        !logs.contains(SECRET_SENTINEL_TOKEN),
        "secret leaked into logs (api_token sentinel): {logs}"
    );
    assert!(
        !logs.contains(SECRET_SENTINEL_PASSWORD),
        "secret leaked into logs (user_password sentinel): {logs}"
    );
    fix.shutdown().await;
}

/// Iter-1 review P7 (AC#11): IO-failure path secret-redaction test.
/// Story 9-7 iter-1 P12 precedent established that figment IO error
/// wording can echo entire config sections. We force a TOML re-parse
/// failure by making the file unreadable BETWEEN the write and the
/// reload — chmod 000 in a Unix tempdir is the simplest path. The
/// reload returns `ReloadError::Io(_)`; the audit log carries a
/// sanitised `error: %e` field. Verify neither sentinel appears.
#[cfg(unix)]
#[tokio::test]
#[serial(captured_logs)]
async fn application_crud_io_failure_does_not_log_secrets() {
    use std::os::unix::fs::PermissionsExt;

    clear_captured_logs();
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();

    // Capture pre-test perms so we can restore at end (NamedTempFile
    // cleanup needs read permission to unlink under restrictive
    // policies).
    let original_perms = std::fs::metadata(&fix.config_path)
        .expect("stat")
        .permissions();

    // Make the file unreadable AFTER the writer's write succeeds.
    // Setting mode 000 on the parent directory is too aggressive
    // (would break tempfile cleanup); 000 on the file itself causes
    // the next figment-driven reload to fail with EACCES → `ReloadError::Io`.
    //
    // Driving sequence: POST → handler write_atomically succeeds
    // (tempfile write doesn't need read on the target) → figment
    // re-read fails on the now-unreadable file → reload returns Io.
    let mut chmod_perms = original_perms.clone();
    chmod_perms.set_mode(0o000);
    std::fs::set_permissions(&fix.config_path, chmod_perms.clone())
        .expect("chmod 000");

    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        r#"{"application_id":"app-io-fail","application_name":"IO Fail"}"#,
    )
    .send()
    .await
    .expect("send");
    // The exact status depends on which IO step failed first
    // (write_atomically chmod-000 the target directly, so the
    // atomic-rename target permissions are checked AFTER tempfile
    // creation). Either 5xx or 5xx-family. Just assert it's NOT 201.
    assert_ne!(resp.status(), StatusCode::CREATED);

    // Restore perms so tempdir cleanup succeeds.
    std::fs::set_permissions(&fix.config_path, original_perms).ok();

    tokio::time::sleep(Duration::from_millis(120)).await;
    let logs = captured_logs();
    // Iter-1 review P7: even on the IO-failure path, secrets must
    // not leak through figment's error wording.
    assert!(
        !logs.contains(SECRET_SENTINEL_TOKEN),
        "api_token sentinel leaked on IO-failure path: {logs}"
    );
    assert!(
        !logs.contains(SECRET_SENTINEL_PASSWORD),
        "user_password sentinel leaked on IO-failure path: {logs}"
    );
    fix.shutdown().await;
}

// ----------------------------------------------------------------------
// Static asset smoke (AC#1)
// ----------------------------------------------------------------------

#[tokio::test]
async fn applications_html_renders_table() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/applications.html"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(body.contains("<table"), "no <table in body: {body}");
    assert!(body.contains("Application"), "no Application label");
    fix.shutdown().await;
}

#[tokio::test]
async fn applications_js_fetches_api_applications() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let resp = client
        .get(fix.url("/applications.js"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("text");
    assert!(
        body.contains("/api/applications"),
        "JS does not reference /api/applications: {body}"
    );
    fix.shutdown().await;
}

/// Iter-1 review P19 (AC#7): POST triggers reload; dashboard
/// `/api/status::application_count` reflects the new value within
/// the listener-swap window.
#[tokio::test]
async fn post_application_triggers_reload_and_dashboard_reflects() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();

    // Pre-POST baseline.
    let pre = client
        .get(fix.url("/api/status"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .send()
        .await
        .expect("send");
    let pre_body: Value = pre.json().await.expect("json");
    let pre_count = pre_body["application_count"].as_u64().expect("u64") as usize;

    // POST a new application.
    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        r#"{"application_id":"app-reflects","application_name":"Reflects"}"#,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);

    // Poll /api/status until application_count increments OR 5s timeout.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::time::Instant::now() >= deadline {
            panic!("application_count did not reflect the POST within 5s");
        }
        let r = client
            .get(fix.url("/api/status"))
            .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
            .send()
            .await
            .expect("send");
        let body: Value = r.json().await.expect("json");
        let count = body["application_count"].as_u64().expect("u64") as usize;
        if count == pre_count + 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    fix.shutdown().await;
}

/// Iter-1 review P20 (AC#4): preserve field order in unrelated
/// `[[application]]` blocks when a different application is
/// mutated. `toml_edit::DocumentMut` preserves key order on
/// round-trip; this test pins that contract.
#[tokio::test]
async fn post_application_preserves_key_order() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let pre_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    // Find the position of "application_name" relative to
    // "application_id" in the FIRST [[application]] block.
    let pre_app1 = pre_raw
        .find("[[application]]")
        .expect("first app block present");
    let pre_after = &pre_raw[pre_app1..];
    let pre_name_idx = pre_after.find("application_name").expect("name field");
    let pre_id_idx = pre_after.find("application_id").expect("id field");
    let pre_name_first = pre_name_idx < pre_id_idx;

    // Add a NEW application (mutates the array but the existing
    // block's internal order should remain).
    let client = reqwest::Client::new();
    let origin = fix.base_url.clone();
    let resp = json_post_request(
        &client,
        &fix.url("/api/applications"),
        Some(&origin),
        r#"{"application_id":"app-order","application_name":"Order"}"#,
    )
    .send()
    .await
    .expect("send");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let post_raw = std::fs::read_to_string(&fix.config_path).expect("read");
    let post_app1 = post_raw
        .find("[[application]]")
        .expect("first app block present post-write");
    let post_after = &post_raw[post_app1..];
    let post_name_idx = post_after.find("application_name").expect("name field");
    let post_id_idx = post_after.find("application_id").expect("id field");
    let post_name_first = post_name_idx < post_id_idx;
    assert_eq!(
        pre_name_first, post_name_first,
        "field order in first application block changed after unrelated POST"
    );
    fix.shutdown().await;
}

/// Iter-1 review P8 (AC#4 lost-update fix): two concurrent POSTs
/// with distinct `application_id` values must both land in
/// `config/config.toml` after both responses return 201.
/// Without `ConfigWriter::lock()` extending across reload, this
/// test would intermittently fail.
#[tokio::test]
async fn concurrent_post_applications_do_not_lose_updates() {
    let fix = spawn_fixture(APP_TOML_TEMPLATE).await;
    let base_url = fix.base_url.clone();
    let origin = base_url.clone();

    let client_a = reqwest::Client::new();
    let client_b = reqwest::Client::new();

    let url_a = format!("{}/api/applications", base_url);
    let url_b = url_a.clone();
    let origin_a = origin.clone();
    let origin_b = origin.clone();

    let task_a = tokio::spawn(async move {
        json_post_request(
            &client_a,
            &url_a,
            Some(&origin_a),
            r#"{"application_id":"concurrent-a","application_name":"Conc A"}"#,
        )
        .send()
        .await
    });
    let task_b = tokio::spawn(async move {
        json_post_request(
            &client_b,
            &url_b,
            Some(&origin_b),
            r#"{"application_id":"concurrent-b","application_name":"Conc B"}"#,
        )
        .send()
        .await
    });

    let (ra, rb) = tokio::join!(task_a, task_b);
    let resp_a = ra.expect("join a").expect("send a");
    let resp_b = rb.expect("join b").expect("send b");
    assert_eq!(resp_a.status(), StatusCode::CREATED);
    assert_eq!(resp_b.status(), StatusCode::CREATED);

    // Both ids must appear in the final TOML.
    let final_toml = std::fs::read_to_string(&fix.config_path).expect("read");
    assert!(
        final_toml.contains("concurrent-a"),
        "concurrent-a missing from final TOML — lost update: {final_toml}"
    );
    assert!(
        final_toml.contains("concurrent-b"),
        "concurrent-b missing from final TOML — lost update: {final_toml}"
    );
    fix.shutdown().await;
}

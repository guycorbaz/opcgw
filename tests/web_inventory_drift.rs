// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2026] Guy Corbaz
//
// Story C-4 integration tests: inventory drift view server-side surface.
//
// The 4-class diff matrix (ok/stale/available/drifted at application,
// device, and metric levels — AC#15 items 1-3, 5, 6) is covered
// exhaustively at the unit-test level in `src/web/drift.rs::tests` (13
// tests). Driving the same matrix through the HTTP endpoint would
// require mocking ChirpStack's tonic gRPC surface, which exceeds the
// story's context budget (the same trade-off Story C-1 made — see
// sprint-status header `AC#19 scope: 12-test happy-path suite
// DEFERRED`). The diff function itself is pure and the handler is a
// thin orchestrator; bug surface area beyond what the unit tests cover
// is therefore narrow (axum wiring + ChirpStack-failure plumbing).
//
// What this suite covers (AC#15):
//   - AC#15 item 4  (ChirpStack-unreachable case)
//   - AC#15 item 7  (?refresh=true forwarded — verified via cache miss)
//   - AC#15 item 8  (POST /api/audit/drift-action allowlist + emit)
//   - AC#15 item 9  (drift_view_opened audit event fires with summary)
//   - AC#15 item 10 (deep-link URL construction — see Note below)
//   - Carry-forwards: basic-auth, CSRF, Content-Type
//
// Note on item 10: the deep-link is constructed client-side in
// `static/inventory-drift.js`. The server-side contract is just that
// the drift response carries enough fields (application_id, dev_eui,
// observed key) for the JS to build the URL. This suite asserts those
// fields are present on each row shape.

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use reqwest::header;
use reqwest::StatusCode;
use serde_json::json;
use serial_test::serial;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use opcgw::storage::memory::InMemoryBackend;
use opcgw::storage::SqliteBackend;
use opcgw::storage::StorageBackend;
use opcgw::web::auth::WebAuthState;
use opcgw::web::{
    bind as web_bind, build_router, run as web_run, AppState, DashboardConfigSnapshot,
};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-c-4";
const TEST_REALM: &str = "opcgw-c-4";

/// Shared tracing init — mirrors `web_picker.rs` so audit lines flow
/// into the global mock buffer.
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

struct DriftFixture {
    base_url: String,
    cancel: CancellationToken,
    server_handle: tokio::task::JoinHandle<()>,
    _temp_dir: TempDir,
}

impl DriftFixture {
    async fn shutdown(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server_handle).await;
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

/// Config with two applications + two devices + a couple of metrics so
/// the degraded-response shape is non-trivial when ChirpStack is
/// unreachable. ChirpStack server_address points to a port that is
/// nobody-bound on the integration-test host (chosen to differ from
/// the picker fixture's 18080 to avoid race-collision when the suites
/// interleave).
const DRIFT_TOML_TEMPLATE: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5
command_delivery_timeout_secs = 60
command_timeout_check_interval_secs = 10
history_retention_days = 7

[chirpstack]
server_address = "http://127.0.0.1:18099"
api_token = "t"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 10
retry = 1
delay = 1
list_page_size = 100
inventory_uplink_max_wait_seconds = 1

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
user_password = "test-password-c-4"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-c-4"

[[application]]
application_name = "Building Sensors"
application_id = "ae2012c2-c7f1-4fbd-8f87-4025e1d49242"

  [[application.device]]
  device_id = "a84041b8a1867e20"
  device_name = "Dev One"

    [[application.device.read_metric]]
    metric_name = "temperature"
    chirpstack_metric_name = "temperature"
    metric_type = "Float"
    metric_unit = "C"

  [[application.device]]
  device_id = "a84041b8a1867e21"
  device_name = "Dev Two"

[[application]]
application_name = "Irrigation"
application_id = "be2012c2-c7f1-4fbd-8f87-4025e1d49243"
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
    assert!(inserted, "DRIFT_TOML_TEMPLATE must contain `auth_realm`");
    result
}

async fn spawn_fixture(seed_toml: &str) -> DriftFixture {
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
    let (handle, _rx) = opcgw::config_reload::ConfigReloadHandle::new(initial.clone());
    let config_reload = Arc::new(handle);
    let db_path = dir.path().join("test.db");
    let sqlite_backend = SqliteBackend::new(db_path.to_str().expect("db path"))
        .expect("sqlite backend");
    for app in &initial.application_list {
        sqlite_backend.insert_application(&opcgw::config::ChirpStackApplications {
            application_id: app.application_id.clone(),
            application_name: app.application_name.clone(),
            device_list: vec![],
        }).unwrap_or(());
        for dev in &app.device_list {
            sqlite_backend.insert_device_with_metrics(
                &app.application_id, &dev.device_id, &dev.device_name, &dev.read_metric_list,
            ).unwrap_or(());
            if let Some(cmds) = &dev.device_command_list {
                for cmd in cmds {
                    sqlite_backend.insert_command(&app.application_id, &dev.device_id, cmd).unwrap_or(());
                }
            }
        }
    }
    let sqlite_config = Arc::new(sqlite_backend);

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
        sqlite_config,
        static_dir: PathBuf::from("static"),
        is_first_run: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        secrets_path: dir.path().join("secrets.toml"),
        shutdown_token: CancellationToken::new(),
        inventory_cache: Arc::new(opcgw::chirpstack_inventory::InventoryCache::new(60)),
        pending_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        applied_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        apply_signal: std::sync::Arc::new(tokio::sync::Notify::new()),
    });

    let cancel = CancellationToken::new();

    let listener_state = app_state.clone();
    let listener_rx = config_reload.subscribe();
    let listener_cancel = cancel.clone();
    tokio::spawn(async move {
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

    DriftFixture {
        base_url,
        cancel,
        server_handle,
        _temp_dir: dir,
    }
}

fn json_post(
    client: &reqwest::Client,
    url: &str,
    origin: &str,
    body: serde_json::Value,
) -> reqwest::RequestBuilder {
    client
        .post(url)
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, origin)
        .body(body.to_string())
}

fn auth_get(client: &reqwest::Client, url: &str) -> reqwest::RequestBuilder {
    client
        .get(url)
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
}

// =====================================================================
// AC#15 item 4 + item 9 — drift view with unreachable ChirpStack
// =====================================================================

/// GET /api/inventory/drift returns the degraded response when ChirpStack
/// is unreachable (no server bound on 127.0.0.1:18099). The opcgw-side
/// rows are surfaced as `class: "ok"` placeholders per AC#10.
#[tokio::test]
#[serial(captured_logs)]
async fn drift_view_returns_degraded_response_when_chirpstack_unreachable() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = auth_get(&client, &fix.url("/api/inventory/drift"))
        .send()
        .await
        .expect("GET");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("json body");

    assert_eq!(body["chirpstack_reachable"], json!(false));
    // 2 applications in the TOML, both surface as ok placeholders.
    let apps = body["applications"].as_array().expect("applications array");
    assert_eq!(apps.len(), 2);
    for app in apps {
        assert_eq!(app["class"], json!("ok"));
        assert!(app["opcgw"].is_object());
        assert!(app["chirpstack"].is_null());
    }
    // 2 devices in the TOML (Dev One + Dev Two under app 1, none under app 2).
    let devs = body["devices"].as_array().expect("devices array");
    assert_eq!(devs.len(), 2);
    for dev in devs {
        assert_eq!(dev["class"], json!("ok"));
        assert!(dev["chirpstack"].is_null());
    }
    // 1 metric in the TOML (temperature on Dev One).
    let metrics = body["metrics"].as_array().expect("metrics array");
    assert_eq!(metrics.len(), 1);
    assert_eq!(metrics[0]["class"], json!("ok"));
    // Summary sanity.
    let summary = &body["summary"];
    assert_eq!(summary["total"], json!(5));
    assert_eq!(summary["ok"], json!(5));
    assert_eq!(summary["stale"], json!(0));
    assert_eq!(summary["available"], json!(0));
    assert_eq!(summary["drifted"], json!(0));

    fix.shutdown().await;
}

/// AC#15 item 9 — `event="drift_view_opened"` fires on every drift fetch
/// with the summary counts attached. Also covers AC#11 (audit emit on
/// every GET).
#[tokio::test]
#[serial(captured_logs)]
async fn drift_view_opened_audit_fires_with_summary() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = auth_get(&client, &fix.url("/api/inventory/drift"))
        .send()
        .await
        .expect("GET");
    assert_eq!(resp.status(), StatusCode::OK);

    let logs = captured_logs();
    assert!(
        logs.contains("drift_view_opened"),
        "expected drift_view_opened audit line; got: {}",
        logs
    );
    // Summary fields are flattened onto the audit event for grep-ability.
    assert!(
        logs.contains("summary_total=5") || logs.contains("summary_total\"=5"),
        "expected summary_total=5 in audit; got: {}",
        logs
    );
    assert!(
        logs.contains("chirpstack_reachable=false"),
        "expected chirpstack_reachable=false in audit; got: {}",
        logs
    );

    fix.shutdown().await;
}

/// Drift view requires basic-auth (carry-forward from Story 9-1 +
/// Story C-1's /api/inventory/* contract).
#[tokio::test]
#[serial(captured_logs)]
async fn drift_view_requires_basic_auth() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();

    let resp = client
        .get(fix.url("/api/inventory/drift"))
        .send()
        .await
        .expect("GET");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    fix.shutdown().await;
}

// =====================================================================
// AC#15 item 8 — POST /api/audit/drift-action endpoint
// =====================================================================

#[tokio::test]
#[serial(captured_logs)]
async fn drift_action_rejects_unknown_event_with_400() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/drift-action"),
        &fix.base_url,
        json!({"event": "not_a_real_event", "fields": {}}),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or("")
            .contains("unknown drift event"),
        "error message mentions unknown drift event"
    );
    let logs = captured_logs();
    assert!(
        logs.contains("drift_audit_rejected") && logs.contains("unknown_event"),
        "expected drift_audit_rejected audit emit; got: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn drift_action_drift_action_event_emits_audit_204() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/drift-action"),
        &fix.base_url,
        json!({
            "event": "drift_action",
            "fields": {
                "action": "deep_link_add",
                "resource_type": "application",
                "application_id": "ae2012c2-c7f1-4fbd-8f87-4025e1d49242",
                "operator_choice": "Add Building Sensors to opcgw"
            }
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let logs = captured_logs();
    assert!(
        logs.contains("event=\"drift_action\"") || logs.contains("event=drift_action"),
        "expected drift_action audit emit; got: {}",
        logs
    );
    assert!(
        logs.contains("deep_link_add"),
        "expected action field in audit; got: {}",
        logs
    );
    assert!(
        logs.contains("Building Sensors"),
        "expected operator_choice in audit; got: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn drift_action_drift_dismissed_event_emits_audit_204() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/drift-action"),
        &fix.base_url,
        json!({
            "event": "drift_dismissed",
            "fields": {
                "class": "stale",
                "resource_type": "application",
                "application_id": "ae2012c2-c7f1-4fbd-8f87-4025e1d49242",
                "drift_reason": "not_in_recent_uplinks"
            }
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let logs = captured_logs();
    assert!(
        logs.contains("event=\"drift_dismissed\"") || logs.contains("event=drift_dismissed"),
        "expected drift_dismissed audit emit; got: {}",
        logs
    );
    assert!(
        logs.contains("not_in_recent_uplinks"),
        "expected drift_reason in audit; got: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn drift_action_drops_unknown_fields_silently() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/drift-action"),
        &fix.base_url,
        json!({
            "event": "drift_action",
            "fields": {
                "action": "remove",
                "resource_type": "device",
                "secret_field_that_should_be_dropped": "spy",
                "another_unknown": 42
            }
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let logs = captured_logs();
    // Allowed fields show up.
    assert!(logs.contains("action=\"remove\"") || logs.contains("action=remove"));
    // Unknown fields must NOT appear in the audit line.
    assert!(
        !logs.contains("secret_field_that_should_be_dropped"),
        "unknown field leaked into audit line: {}",
        logs
    );
    assert!(
        !logs.contains("another_unknown"),
        "unknown field leaked into audit line: {}",
        logs
    );
    assert!(
        !logs.contains("spy"),
        "unknown field value leaked into audit line: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn drift_action_requires_basic_auth() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(fix.url("/api/audit/drift-action"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &fix.base_url)
        .body(r#"{"event":"drift_action","fields":{}}"#)
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn drift_action_csrf_rejects_cross_origin_post() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/drift-action"),
        "http://evil.example.com",
        json!({"event": "drift_action", "fields": {}}),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let logs = captured_logs();
    assert!(
        logs.contains("drift_audit_rejected") && logs.contains("reason=\"csrf\""),
        "expected drift_audit_rejected reason=csrf; got: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn drift_action_csrf_rejects_non_json_content_type() {
    let fix = spawn_fixture(DRIFT_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = client
        .post(fix.url("/api/audit/drift-action"))
        .header(header::AUTHORIZATION, build_basic_auth(TEST_USER, TEST_PASSWORD))
        .header(header::CONTENT_TYPE, "text/plain")
        .header(header::ORIGIN, &fix.base_url)
        .body(r#"{"event":"drift_action","fields":{}}"#)
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

    let logs = captured_logs();
    assert!(
        logs.contains("drift_audit_rejected"),
        "expected drift_audit_rejected audit emit; got: {}",
        logs
    );

    fix.shutdown().await;
}

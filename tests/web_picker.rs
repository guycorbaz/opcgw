// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] Guy Corbaz
//
// Story C-2 integration tests: inventory picker server-side surface.
//
// JavaScript-side picker behaviour is covered by manual smoke against
// Guy's real ChirpStack (Task 8.4); this suite covers the SERVER bits:
//
//   - POST /api/audit/picker-event allow-list validation (AC#11)
//   - picker_opened + picker_manual_fallback happy-paths (AC#12 / #13)
//   - unknown-field drop behaviour (AC#11 sanitisation contract)
//   - CSRF + basic-auth carry-forward (AC#14)
//   - picker_metadata round-trip through create_device → emit
//     `event="metric_wire_type_inferred"` (AC#10)
//   - manual-entry metric path stays silent (no picker emit)
//   - picker-attributed application_id round-trips byte-for-byte (AC#15)
//   - cache invalidation audit fires after application create (AC#19)

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
const TEST_PASSWORD: &str = "test-password-c-2";
const TEST_REALM: &str = "opcgw-c-2";

/// Shared tracing init — mirrors the pattern in web_application_crud.rs
/// so picker-event audit lines are captured into the global mock buffer.
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

struct PickerFixture {
    base_url: String,
    cancel: CancellationToken,
    server_handle: tokio::task::JoinHandle<()>,
    _temp_dir: TempDir,
}

impl PickerFixture {
    async fn shutdown(self) {
        self.cancel.cancel();
        let _ = tokio::time::timeout(Duration::from_secs(5), self.server_handle).await;
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

const PICKER_TOML_TEMPLATE: &str = r#"
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
user_password = "test-password-c-2"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-c-2"

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
    assert!(inserted, "PICKER_TOML_TEMPLATE must contain `auth_realm`");
    result
}

async fn spawn_fixture(seed_toml: &str) -> PickerFixture {
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
        // Iter-1 review MED — per-test tempdir-relative path rather than
        // a hardcoded /tmp filename. The picker tests don't touch the
        // secrets file but the test infrastructure should match the
        // CRUD-test pattern so any future code path that wrote through
        // this field would not cross-contaminate parallel test runs.
        secrets_path: dir.path().join("secrets.toml"),
        shutdown_token: CancellationToken::new(),
        inventory_cache: Arc::new(opcgw::chirpstack_inventory::InventoryCache::new(60)),
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

    PickerFixture {
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

// =====================================================================
// AC#11 — POST /api/audit/picker-event allowlist validation
// =====================================================================

#[tokio::test]
#[serial(captured_logs)]
async fn audit_picker_event_rejects_unknown_event_with_400() {
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/picker-event"),
        &fix.base_url,
        json!({"event": "not_a_real_event", "fields": {}}),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert!(
        body.get("error").and_then(|v| v.as_str()).unwrap_or("").contains("unknown picker event"),
        "error message mentions unknown picker event"
    );
    // Audit-side: a `picker_audit_rejected reason=unknown_event` line
    // is emitted so operators can see the rejected event in the log.
    let logs = captured_logs();
    assert!(
        logs.contains("picker_audit_rejected") && logs.contains("unknown_event"),
        "expected picker_audit_rejected audit emit; got: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn audit_picker_event_picker_opened_emits_audit_204() {
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/picker-event"),
        &fix.base_url,
        json!({
            "event": "picker_opened",
            "fields": {
                "picker_resource": "application",
                "cache_status": "miss"
            }
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let logs = captured_logs();
    assert!(
        logs.contains("picker_opened") && logs.contains("picker_resource=\"application\""),
        "expected picker_opened audit emit with picker_resource field; got: {}",
        logs
    );
    assert!(
        logs.contains("cache_status=\"miss\""),
        "expected cache_status field passed through; got: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn audit_picker_event_picker_manual_fallback_emits_audit_204() {
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/picker-event"),
        &fix.base_url,
        json!({
            "event": "picker_manual_fallback",
            "fields": {
                "picker_resource": "device",
                "reason": "chirpstack_unreachable",
                "error_detail": "HTTP 502 for /api/inventory/devices"
            }
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let logs = captured_logs();
    assert!(
        logs.contains("picker_manual_fallback")
            && logs.contains("reason=\"chirpstack_unreachable\""),
        "expected picker_manual_fallback audit emit with reason; got: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn audit_picker_event_drops_unknown_fields_silently() {
    // AC#11 sanitisation contract: unknown fields are silently dropped
    // (we do not reject the whole event on a typo). The known fields
    // still emit; the unknown field's value does not appear in the log.
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let unique_marker = "MARKER_UNKNOWN_FIELD_XYZ123";
    let resp = json_post(
        &client,
        &fix.url("/api/audit/picker-event"),
        &fix.base_url,
        json!({
            "event": "picker_opened",
            "fields": {
                "picker_resource": "uplink",
                "cache_status": "bypassed",
                "evil_extra": unique_marker,
                "another_unknown": "nope"
            }
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);

    let logs = captured_logs();
    // Iter-1 review MED — positive assertion that the KNOWN fields
    // are present + correctly rendered. Pre-fix, the test only
    // checked marker absence, which would silently pass if the audit
    // emit broke entirely (panic-and-recover before info! macro). The
    // legitimate fields MUST land for the test to confirm the
    // sanitisation path actually emitted a structured event.
    assert!(
        logs.contains("picker_opened") && logs.contains("picker_resource=\"uplink\""),
        "expected picker_opened with picker_resource=uplink (positive emit assertion); got: {}",
        logs
    );
    assert!(
        logs.contains("cache_status=\"bypassed\""),
        "expected cache_status=bypassed to land in the audit emit (positive field-passthrough assertion); got: {}",
        logs
    );
    assert!(
        !logs.contains(unique_marker),
        "unknown field value MUST be dropped before audit emit; got: {}",
        logs
    );

    fix.shutdown().await;
}

// =====================================================================
// AC#14 — CSRF + basic-auth carry-forward
// =====================================================================

#[tokio::test]
async fn audit_picker_event_requires_basic_auth() {
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();

    let resp = client
        .post(fix.url("/api/audit/picker-event"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::ORIGIN, &fix.base_url)
        .body(r#"{"event":"picker_opened","fields":{}}"#)
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn audit_picker_event_rejects_cross_origin_with_picker_audit_event() {
    // CSRF middleware must reject a cross-origin POST and emit a
    // literal `event="picker_audit_rejected"` (Story C-2 dispatch arm)
    // — NOT the fall-through `crud_rejected`.
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/audit/picker-event"),
        "http://evil.example.com",
        json!({"event": "picker_opened", "fields": {}}),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::FORBIDDEN);

    let logs = captured_logs();
    assert!(
        logs.contains("picker_audit_rejected") && logs.contains("csrf"),
        "expected picker_audit_rejected with reason csrf; got: {}",
        logs
    );

    fix.shutdown().await;
}

// =====================================================================
// AC#10 — picker_metadata round-trips into the metric_wire_type_inferred
// audit event from the create_device path
// =====================================================================

#[tokio::test]
#[serial(captured_logs)]
async fn create_device_emits_metric_wire_type_inferred_with_picker_metadata() {
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/applications/ae2012c2-c7f1-4fbd-8f87-4025e1d49242/devices"),
        &fix.base_url,
        json!({
            "device_id": "a84041b8a1867e21",
            "device_name": "WaterFlowSensor",
            "read_metric_list": [
                {
                    "metric_name": "water_flow",
                    "chirpstack_metric_name": "water_flow",
                    "metric_type": "Float",
                    "picker_metadata": {
                        "inferred_type": "Float",
                        "operator_chosen_type": "Float",
                        "sample_values_count": 8
                    }
                }
            ]
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let logs = captured_logs();
    assert!(
        logs.contains("metric_wire_type_inferred"),
        "expected metric_wire_type_inferred audit emit; got: {}",
        logs
    );
    assert!(
        logs.contains("source=\"web_picker\"")
            && logs.contains("inferred_type=\"Float\"")
            && logs.contains("operator_chosen_type=\"Float\"")
            && logs.contains("sample_values_count=8"),
        "expected source + inferred_type + operator_chosen_type + sample_values_count; got: {}",
        logs
    );
    // Iter-3 review LOW — pin the ?-Debug audit format for
    // application_id / device_id so a future regression back to
    // %-Display formatting (which would produce bare/unquoted output
    // for some types) is caught by CI. For &str values, both forms
    // currently produce the same quoted shape via tracing's fmt
    // layer, but the iter-2 doctrine was explicit about ?-Debug for
    // upstream-provided fields; this assertion encodes that contract.
    assert!(
        logs.contains("application_id=\"ae2012c2-c7f1-4fbd-8f87-4025e1d49242\"")
            && logs.contains("device_id=\"a84041b8a1867e21\""),
        "expected application_id + device_id rendered with quoted ?-Debug shape; got: {}",
        logs
    );

    fix.shutdown().await;
}

#[tokio::test]
#[serial(captured_logs)]
async fn create_device_without_picker_metadata_stays_silent_on_picker_audit() {
    // Manual-entry metric (no picker_metadata envelope) MUST NOT emit
    // `metric_wire_type_inferred` — manual entry is unaudited beyond
    // the existing device_crud events.
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/applications/ae2012c2-c7f1-4fbd-8f87-4025e1d49242/devices"),
        &fix.base_url,
        json!({
            "device_id": "a84041b8a1867e22",
            "device_name": "ManualDevice",
            "read_metric_list": [
                {
                    "metric_name": "battery",
                    "chirpstack_metric_name": "battery",
                    "metric_type": "Int"
                }
            ]
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let logs = captured_logs();
    assert!(
        !logs.contains("metric_wire_type_inferred"),
        "manual-entry path MUST NOT emit metric_wire_type_inferred; got: {}",
        logs
    );
    // But the existing device_created audit should still fire.
    assert!(
        logs.contains("device_created"),
        "device_created audit must still fire; got: {}",
        logs
    );

    fix.shutdown().await;
}

// =====================================================================
// AC#15 — application_id round-trip integrity through picker submit
// =====================================================================

#[tokio::test]
#[serial(captured_logs)]
async fn picker_submit_application_id_round_trips_byte_for_byte() {
    // The picker sets `<option value="...">` to the C-1 inventory id.
    // The form submit must POST the exact same string to
    // /api/applications — no truncation, case-change, or whitespace.
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    // A UUID with case + dashes that would be mangled by any
    // accidental .to_lowercase / split_whitespace.
    let app_id = "DEADBEEF-1234-5678-9ABC-DEF012345678";
    let resp = json_post(
        &client,
        &fix.url("/api/applications"),
        &fix.base_url,
        json!({
            "application_id": app_id,
            "application_name": "Picker-Sourced App"
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::CREATED);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(
        body["application_id"].as_str().unwrap(),
        app_id,
        "application_id must round-trip byte-for-byte"
    );

    fix.shutdown().await;
}

// =====================================================================
// AC#19 — cache invalidation after CRUD writes
// =====================================================================

#[tokio::test]
#[serial(captured_logs)]
async fn create_application_emits_inventory_cache_invalidated_audit() {
    // After the picker-driven create flow lands, the next inventory
    // fetch should be a cache miss. The audit signal that proves it
    // is `event="inventory_cache_invalidated"` fired from the CRUD
    // success branch (C-1 contract). Story C-2 just exercises the
    // path via a picker-shaped POST.
    let fix = spawn_fixture(PICKER_TOML_TEMPLATE).await;
    let client = reqwest::Client::new();
    clear_captured_logs();

    let resp = json_post(
        &client,
        &fix.url("/api/applications"),
        &fix.base_url,
        json!({
            "application_id": "c2-cache-test-1",
            "application_name": "Cache Invalidation Test"
        }),
    )
    .send()
    .await
    .expect("POST");
    assert_eq!(resp.status(), StatusCode::CREATED);

    let logs = captured_logs();
    assert!(
        logs.contains("inventory_cache_invalidated"),
        "expected inventory_cache_invalidated audit after application_created; got: {}",
        logs
    );

    fix.shutdown().await;
}

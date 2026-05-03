// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 9-2 integration tests: Gateway Status Dashboard (FR38, FR41).
//
// What these tests pin (the "shape contract"):
//   - AC#3 (auth carry-forward from Story 9-1): unauth'd GET /api/status
//           returns 401 + emits event="web_auth_failed" with reason=missing,
//           proving the 9-1 middleware wraps the new route.
//   - AC#2 (JSON shape): auth'd GET /api/status returns 200 with the 6
//           expected fields and the right JSON value-types.
//   - AC#4 (HTML markup): auth'd GET /index.html returns 200 with the
//           <meta viewport> tag and the 5 expected DOM IDs the JS hooks into.
//   - AC#4 (CSS responsive marker): auth'd GET /dashboard.css returns 200
//           with @media + min-width (FR41 marker pinned at the CSS level).

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
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{fmt as tracing_fmt, layer::SubscriberExt, Layer};

use opcgw::storage::memory::InMemoryBackend;
use opcgw::storage::StorageBackend;
use opcgw::web::auth::WebAuthState;
use opcgw::web::{
    bind as web_bind, build_router, run as web_run, AppState, DashboardConfigSnapshot,
};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-9-2";
const TEST_REALM: &str = "opcgw-9-2";

/// Same install pattern as `tests/web_auth.rs` (issue #101 fixes).
fn init_test_subscriber() {
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
        tracing::subscriber::set_global_default(subscriber).unwrap_or_else(|e| {
            panic!(
                "init_test_subscriber: set_global_default failed ({e:?}). \
                 Did another test framework (e.g. #[traced_test]) install a \
                 subscriber first? Captured-log assertions in this file \
                 require this subscriber to be active."
            )
        });
    });
}

fn build_basic_auth(user: &str, password: &str) -> String {
    let blob = BASE64_STANDARD.encode(format!("{user}:{password}"));
    format!("Basic {blob}")
}

/// Build an `AppState` with the supplied snapshot + a freshly-keyed
/// `WebAuthState` against the test credentials. Backend is an
/// `InMemoryBackend` with a single `update_gateway_status` call so the
/// `/api/status` JSON has populated values.
fn build_test_app_state(snapshot: DashboardConfigSnapshot) -> Arc<AppState> {
    let auth = Arc::new(WebAuthState::new_with_fresh_key(
        TEST_USER,
        TEST_PASSWORD,
        TEST_REALM.to_string(),
    ));
    let backend = InMemoryBackend::new();
    backend
        .update_gateway_status(Some(chrono::Utc::now()), 3, true)
        .expect("seed gateway_status");
    let backend: Arc<dyn StorageBackend> = Arc::new(backend);
    Arc::new(AppState {
        auth,
        backend,
        dashboard_snapshot: Arc::new(snapshot),
        start_time: std::time::Instant::now(),
    })
}

/// Build a static directory containing the production-shaped dashboard
/// assets so `tests/web_dashboard.rs` exercises the same files
/// `cargo run` would serve. Copies from the repo's `static/` dir.
async fn build_production_static_dir() -> TempDir {
    let dst = TempDir::new().expect("test static tmp dir");
    let src = PathBuf::from("static");
    for name in ["index.html", "dashboard.css", "dashboard.js"] {
        let body = tokio::fs::read(src.join(name))
            .await
            .unwrap_or_else(|e| panic!("read static/{name}: {e}"));
        tokio::fs::write(dst.path().join(name), body)
            .await
            .unwrap_or_else(|e| panic!("write static/{name}: {e}"));
    }
    dst
}

/// Spawn the web server on an ephemeral port; return (addr, cancel,
/// handle, static_tmp). The TempDir is returned so the caller can drop
/// it after the test; otherwise it would be dropped immediately and
/// `ServeDir` would 404 every static request.
async fn spawn_test_server(
    snapshot: DashboardConfigSnapshot,
) -> (
    SocketAddr,
    CancellationToken,
    tokio::task::JoinHandle<()>,
    TempDir,
) {
    let static_tmp = build_production_static_dir().await;
    let port = common::pick_free_port().await;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();

    let app_state = build_test_app_state(snapshot);
    let router = build_router(app_state, static_tmp.path().to_path_buf());
    let cancel = CancellationToken::new();
    let cancel_for_run = cancel.clone();
    let realm = TEST_REALM.to_string();

    let listener = web_bind(addr).await.expect("test web server bind");
    let handle = tokio::spawn(async move {
        let _ = web_run(listener, router, &realm, cancel_for_run).await;
    });

    // Probe the listener before returning so the test request never
    // races the bind.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("server did not bind within 5s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    (addr, cancel, handle, static_tmp)
}

// =====================================================================
// AC#3 carry-forward: unauth'd GET /api/status returns 401 + emits the
// Story 9-1 web_auth_failed audit event with reason=missing. This proves
// the auth middleware from Story 9-1 wraps the new /api/status route.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn auth_required_for_api_status() {
    init_test_subscriber();

    let snapshot = DashboardConfigSnapshot {
        application_count: 0,
        device_count: 0,
        applications: vec![],
    };
    let (addr, cancel, handle, _static_tmp) = spawn_test_server(snapshot).await;

    let client = common::build_http_client(Duration::from_secs(5));
    let resp = client
        .get(format!("http://{addr}/api/status"))
        .send()
        .await
        .expect("GET /api/status (unauth)");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // WWW-Authenticate header must carry the configured realm.
    let www = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate header present");
    assert!(
        www.to_str().unwrap_or("").contains(TEST_REALM),
        "WWW-Authenticate should carry the realm, got {www:?}"
    );

    // Audit event emitted exactly once with reason=missing + path=/api/status.
    // tracing-test buffers events globally; flush + read inside an inner
    // scope so the MutexGuard is dropped before the later .await.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let captured: String = {
        let buf = tracing_test::internal::global_buf().lock().unwrap();
        String::from_utf8_lossy(&buf).to_string()
    };

    let matching: Vec<&str> = captured
        .lines()
        .filter(|l| {
            l.contains("event=\"web_auth_failed\"")
                && l.contains("path=/api/status")
                && l.contains("reason=\"missing\"")
        })
        .collect();
    assert!(
        !matching.is_empty(),
        "expected at least one web_auth_failed audit line for path=/api/status reason=missing, got captured log:\n{captured}"
    );

    cancel.cancel();
    let _ = handle.await;
}

// =====================================================================
// AC#2: auth'd GET /api/status returns 200 + JSON with all 6 expected
// fields. Field-shape regression pin: the JSON contract is operator-
// observable (curl | jq); a future refactor that drops a field or
// changes a type is caught here.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn api_status_returns_json_with_expected_shape_when_authed() {
    init_test_subscriber();

    let snapshot = DashboardConfigSnapshot {
        application_count: 2,
        device_count: 7,
        applications: vec![],
    };
    let (addr, cancel, handle, _static_tmp) = spawn_test_server(snapshot).await;

    let client = common::build_http_client(Duration::from_secs(5));
    let resp = client
        .get(format!("http://{addr}/api/status"))
        .header(
            header::AUTHORIZATION,
            build_basic_auth(TEST_USER, TEST_PASSWORD),
        )
        .send()
        .await
        .expect("GET /api/status (auth'd)");
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.text().await.expect("response body");
    let json: Value =
        serde_json::from_str(&body).unwrap_or_else(|e| panic!("body not JSON: {e}; body={body}"));

    // All 6 fields present.
    for field in [
        "chirpstack_available",
        "last_poll_time",
        "error_count",
        "application_count",
        "device_count",
        "uptime_secs",
    ] {
        assert!(
            json.get(field).is_some(),
            "missing field {field} in /api/status response: {json}"
        );
    }

    // Type pinning.
    assert!(
        json["chirpstack_available"].is_boolean(),
        "chirpstack_available must be a JSON boolean"
    );
    assert!(
        json["last_poll_time"].is_string() || json["last_poll_time"].is_null(),
        "last_poll_time must be a JSON string or null"
    );
    assert!(json["error_count"].is_number());
    assert!(json["application_count"].is_number());
    assert!(json["device_count"].is_number());
    assert!(json["uptime_secs"].is_number());

    // Value pinning from the seeded backend + snapshot.
    assert_eq!(json["chirpstack_available"].as_bool(), Some(true));
    assert_eq!(json["error_count"].as_i64(), Some(3));
    assert_eq!(json["application_count"].as_u64(), Some(2));
    assert_eq!(json["device_count"].as_u64(), Some(7));
    // last_poll_time must parse as RFC 3339.
    let lpt = json["last_poll_time"].as_str().expect("last_poll_time string");
    chrono::DateTime::parse_from_rfc3339(lpt).expect("RFC 3339 parseable");

    cancel.cancel();
    let _ = handle.await;
}

// =====================================================================
// AC#4: dashboard HTML markup pin. The dashboard.js depends on these
// DOM IDs; renaming any of them silently breaks the JS at runtime
// without test coverage. Pinning here makes the contract a build-time
// invariant.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn dashboard_html_contains_viewport_meta_and_status_tiles_markup() {
    init_test_subscriber();

    let (addr, cancel, handle, _static_tmp) = spawn_test_server(DashboardConfigSnapshot {
        application_count: 0,
        device_count: 0,
        applications: vec![],
    })
    .await;

    let client = common::build_http_client(Duration::from_secs(5));
    let resp = client
        .get(format!("http://{addr}/index.html"))
        .header(
            header::AUTHORIZATION,
            build_basic_auth(TEST_USER, TEST_PASSWORD),
        )
        .send()
        .await
        .expect("GET /index.html (auth'd)");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("html body");

    // FR41 marker: <meta viewport> tag.
    assert!(
        body.contains("<meta name=\"viewport\""),
        "FR41 viewport meta tag missing from dashboard HTML"
    );

    // The five dashboard-tile DOM IDs the JS hooks into.
    for id in [
        "id=\"chirpstack-status\"",
        "id=\"last-poll-time\"",
        "id=\"error-count\"",
        "id=\"application-count\"",
        "id=\"device-count\"",
    ] {
        assert!(
            body.contains(id),
            "dashboard HTML must contain {id} for dashboard.js to bind to"
        );
    }

    // Some <script> tag must exist (relaxed match — accept either
    // src=/dashboard.js or an inline block).
    assert!(
        body.contains("<script"),
        "dashboard HTML must include a <script> for the live-refresh path"
    );

    cancel.cancel();
    let _ = handle.await;
}

// =====================================================================
// AC#4: CSS responsive marker pin. FR41 mobile-responsive contract is
// satisfied at the CSS level by the @media (min-width: …) query that
// switches to the two-column grid above 600 px.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn dashboard_css_contains_responsive_media_query() {
    init_test_subscriber();

    let (addr, cancel, handle, _static_tmp) = spawn_test_server(DashboardConfigSnapshot {
        application_count: 0,
        device_count: 0,
        applications: vec![],
    })
    .await;

    let client = common::build_http_client(Duration::from_secs(5));
    let resp = client
        .get(format!("http://{addr}/dashboard.css"))
        .header(
            header::AUTHORIZATION,
            build_basic_auth(TEST_USER, TEST_PASSWORD),
        )
        .send()
        .await
        .expect("GET /dashboard.css (auth'd)");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("css body");

    assert!(
        body.contains("@media"),
        "FR41: dashboard.css must contain a @media query for responsive layout"
    );
    assert!(
        body.contains("min-width"),
        "FR41: dashboard.css must contain a min-width media query"
    );

    cancel.cancel();
    let _ = handle.await;
}


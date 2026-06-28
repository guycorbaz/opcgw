// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story C-1 integration tests: /api/inventory/* handlers.
//
// These tests focus on the HTTP-layer validation paths that don't
// require a mock ChirpStack:
//   - AC#1 / #2 / #3: missing / invalid query parameters → 400.
//   - AC#3: limit > 50 cap rejection.
//   - Basic auth carry-forward from the existing middleware stack.
//
// The cache + ChirpStack-call paths are covered by the unit tests in
// `src/chirpstack_inventory.rs::tests` and `src/web/inventory.rs::tests`.
// Full mock-ChirpStack integration is deferred — see the C-1
// Completion Note for the follow-up.

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use reqwest::header;
use reqwest::StatusCode;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use opcgw::chirpstack_inventory::InventoryCache;
use opcgw::storage::memory::InMemoryBackend;
use opcgw::storage::StorageBackend;
use opcgw::web::auth::WebAuthState;
use opcgw::web::{
    bind as web_bind, build_router, run as web_run, AppState, DashboardConfigSnapshot,
};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-c-1";
const TEST_REALM: &str = "opcgw-c-1";

fn static_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}

fn build_test_app_state() -> (Arc<AppState>, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let auth = Arc::new(
        WebAuthState::new_with_fresh_key(TEST_USER, TEST_PASSWORD, TEST_REALM.to_string()),
    );
    let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
    let snapshot = Arc::new(DashboardConfigSnapshot {
        application_count: 0,
        device_count: 0,
        applications: vec![],
    });
    let (config_reload, sqlite_config, dir2) =
        opcgw::web::test_support::make_test_reload_handle_and_writer();
    std::mem::forget(dir2);

    let app_state = Arc::new(AppState {
        auth,
        backend,
        dashboard_snapshot: std::sync::RwLock::new(snapshot),
        start_time: std::time::Instant::now(),
        stale_threshold_secs: std::sync::atomic::AtomicU64::new(120),
        config_reload,
        sqlite_config,
        static_dir: static_dir(),
        is_first_run: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        secrets_path: dir.path().join("secrets.toml"),
        shutdown_token: CancellationToken::new(),
        inventory_cache: Arc::new(InventoryCache::new(60)),
        pending_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        applied_gen: std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0)),
        apply_signal: std::sync::Arc::new(tokio::sync::Notify::new()),
    });
    (app_state, dir)
}

async fn spawn_web_server(
    app_state: Arc<AppState>,
) -> (SocketAddr, tokio::task::JoinHandle<()>, CancellationToken) {
    let cancel = app_state.shutdown_token.clone();
    let listener = web_bind("127.0.0.1:0".parse::<SocketAddr>().unwrap())
        .await
        .expect("bind ephemeral port");
    let addr = listener.local_addr().expect("local_addr");
    let router = build_router(app_state, static_dir());
    let cancel_for_run = cancel.clone();
    let realm = TEST_REALM.to_string();
    let handle = tokio::spawn(async move {
        if let Err(e) = web_run(listener, router, &realm, cancel_for_run).await {
            eprintln!("web_run error: {}", e);
        }
    });
    (addr, handle, cancel)
}

fn auth_header() -> String {
    let raw = format!("{}:{}", TEST_USER, TEST_PASSWORD);
    let encoded = BASE64_STANDARD.encode(raw.as_bytes());
    format!("Basic {}", encoded)
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client build")
}

#[tokio::test]
async fn inventory_applications_requires_auth() {
    let (state, _dir) = build_test_app_state();
    let (addr, handle, cancel) = spawn_web_server(state).await;

    let resp = http_client()
        .get(format!("http://{}/api/inventory/applications", addr))
        .send()
        .await
        .expect("GET /api/inventory/applications");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn inventory_devices_missing_application_id_returns_400() {
    let (state, _dir) = build_test_app_state();
    let (addr, handle, cancel) = spawn_web_server(state).await;

    let resp = http_client()
        .get(format!("http://{}/api/inventory/devices", addr))
        .header(header::AUTHORIZATION, auth_header())
        .send()
        .await
        .expect("GET /api/inventory/devices");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["error"], "missing_query_param");
    assert_eq!(body["param"], "application_id");

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn inventory_uplinks_missing_dev_eui_returns_400() {
    let (state, _dir) = build_test_app_state();
    let (addr, handle, cancel) = spawn_web_server(state).await;

    let resp = http_client()
        .get(format!("http://{}/api/inventory/uplinks", addr))
        .header(header::AUTHORIZATION, auth_header())
        .send()
        .await
        .expect("GET /api/inventory/uplinks");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["error"], "missing_query_param");
    assert_eq!(body["param"], "dev_eui");

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn inventory_uplinks_invalid_dev_eui_returns_400() {
    let (state, _dir) = build_test_app_state();
    let (addr, handle, cancel) = spawn_web_server(state).await;

    let resp = http_client()
        .get(format!(
            "http://{}/api/inventory/uplinks?dev_eui=not-hex",
            addr
        ))
        .header(header::AUTHORIZATION, auth_header())
        .send()
        .await
        .expect("GET /api/inventory/uplinks");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["error"], "invalid_dev_eui");

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn inventory_uplinks_limit_over_cap_returns_400() {
    let (state, _dir) = build_test_app_state();
    let (addr, handle, cancel) = spawn_web_server(state).await;

    let resp = http_client()
        .get(format!(
            "http://{}/api/inventory/uplinks?dev_eui=a84041b8a1867e20&limit=51",
            addr
        ))
        .header(header::AUTHORIZATION, auth_header())
        .send()
        .await
        .expect("GET /api/inventory/uplinks");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["error"], "limit_out_of_range");
    assert_eq!(body["cap"], 50);

    cancel.cancel();
    let _ = handle.await;
}

// ---- Story G-1: /api/inventory/measurements ---------------------------

#[tokio::test]
async fn inventory_measurements_requires_auth() {
    let (state, _dir) = build_test_app_state();
    let (addr, handle, cancel) = spawn_web_server(state).await;

    let resp = http_client()
        .get(format!(
            "http://{}/api/inventory/measurements?dev_eui=a84041b8a1867e20",
            addr
        ))
        .send()
        .await
        .expect("GET /api/inventory/measurements");
    // 401 (not 404) also proves the route is registered.
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn inventory_measurements_missing_dev_eui_returns_400() {
    let (state, _dir) = build_test_app_state();
    let (addr, handle, cancel) = spawn_web_server(state).await;

    let resp = http_client()
        .get(format!("http://{}/api/inventory/measurements", addr))
        .header(header::AUTHORIZATION, auth_header())
        .send()
        .await
        .expect("GET /api/inventory/measurements");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["error"], "missing_query_param");
    assert_eq!(body["param"], "dev_eui");

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn inventory_measurements_invalid_dev_eui_returns_400() {
    let (state, _dir) = build_test_app_state();
    let (addr, handle, cancel) = spawn_web_server(state).await;

    let resp = http_client()
        .get(format!(
            "http://{}/api/inventory/measurements?dev_eui=not-hex",
            addr
        ))
        .header(header::AUTHORIZATION, auth_header())
        .send()
        .await
        .expect("GET /api/inventory/measurements");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["error"], "invalid_dev_eui");

    cancel.cancel();
    let _ = handle.await;
}

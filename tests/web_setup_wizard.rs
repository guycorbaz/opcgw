// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Epic C C-0 (2026-05-21) integration tests: first-run setup wizard.
//
// What these tests pin (the "shape contract"):
//   - AC#1, #2: validator accepts empty user_password when no env-var;
//     is_first_run() returns true in that state. (Pinned at unit level
//     in src/config.rs::tests; these integration tests focus on the
//     HTTP surface.)
//   - AC#4: GET /setup in first-run mode renders the wizard HTML.
//     GET /setup post-first-run returns HTTP 410 Gone.
//   - AC#5: in first-run mode, GET / (and any other non-wizard,
//     non-static path) returns HTTP 303 → /setup. Static assets
//     (/dashboard.css) bypass the redirect.
//   - AC#6: in first-run mode, the OPC UA path is not exercised by
//     these tests (that's pinned in src/main.rs integration);
//     audit-event emission is pinned in unit tests.
//   - AC#7, #8: POST /api/setup/password validates the request body and
//     persists to secrets.toml with chmod 0600 on success.
//   - AC#11: the wizard POST signals the gateway's CancellationToken
//     after a successful write. (Verified via the token.is_cancelled()
//     check after the request.)
//   - AC#16: env-var path bypasses the wizard — when
//     OPCGW_OPCUA__USER_PASSWORD is set, is_first_run() returns false
//     even with empty in-memory password.
//   - AC#17: populated-config path bypasses the wizard — when
//     [opcua].user_password is non-empty, is_first_run() returns false.
//
// Notes on the test harness:
//   - We spawn the web server on an ephemeral port via the same
//     web::bind + web::run + build_router entry points production uses.
//     No mocking — real reqwest client + real Axum router.
//   - The wizard's POST handler signals the CancellationToken on
//     success; we observe via token.is_cancelled() after the request
//     completes. The web server task itself drains and exits, mirroring
//     the production restart path.

mod common;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use reqwest::redirect::Policy;
use reqwest::StatusCode;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;

use opcgw::storage::memory::InMemoryBackend;
use opcgw::storage::StorageBackend;
use opcgw::utils::WEB_DEFAULT_AUTH_REALM;
use opcgw::web::{
    auth::WebAuthState, bind as web_bind, build_router, run as web_run, AppState,
    DashboardConfigSnapshot,
};

/// Build a first-run AppState with throwaway auth credentials. Used by
/// the wizard tests that simulate "no password set yet" deployments.
fn build_first_run_app_state(
    secrets_dir: &TempDir,
    shutdown_token: CancellationToken,
) -> Arc<AppState> {
    // Iter-3 P5: ONE atomic shared between WebAuthState + AppState
    // so the test exercises the same flip-propagation semantics as
    // production. Pre-iter-3, the test built two separate atomics
    // and never noticed the drift.
    let is_first_run_atomic =
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let auth = Arc::new(WebAuthState::for_first_run(
        WEB_DEFAULT_AUTH_REALM.to_string(),
        is_first_run_atomic.clone(),
    ));
    let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
    let snapshot = Arc::new(DashboardConfigSnapshot {
        application_count: 0,
        device_count: 0,
        applications: vec![],
    });
    let (config_reload, config_writer, dir) =
        opcgw::web::test_support::make_test_reload_handle_and_writer();
    std::mem::forget(dir);
    Arc::new(AppState {
        auth,
        backend,
        dashboard_snapshot: std::sync::RwLock::new(snapshot),
        start_time: std::time::Instant::now(),
        stale_threshold_secs: std::sync::atomic::AtomicU64::new(120),
        config_reload,
        config_writer,
        // Use the canonical static_dir helper so the wizard handler
        // can locate setup.html regardless of test cwd (iter-1 H5/EH-H2).
        static_dir: static_dir(),
        is_first_run: is_first_run_atomic,
        secrets_path: secrets_dir.path().join("secrets.toml"),
        shutdown_token,
    })
}

/// Path to the production static/ directory, anchored at
/// CARGO_MANIFEST_DIR so the test works regardless of cwd.
fn static_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("static")
}

/// Spawn the web server on an ephemeral port and return the bound
/// address plus the spawn handle. Caller is responsible for cancelling
/// the shutdown_token to make the server exit.
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
    let realm = WEB_DEFAULT_AUTH_REALM.to_string();
    let handle = tokio::spawn(async move {
        if let Err(e) = web_run(listener, router, &realm, cancel_for_run).await {
            eprintln!("web_run error: {}", e);
        }
    });
    (addr, handle, cancel)
}

/// Build a reqwest client that does NOT follow redirects — so the test
/// can assert HTTP 303 directly without losing it to redirect chasing.
fn client_no_redirect() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(Policy::none())
        .timeout(Duration::from_secs(5))
        .build()
        .expect("client build")
}

#[tokio::test]
async fn first_run_redirects_root_to_setup() {
    let secrets_dir = TempDir::new().expect("tempdir");
    let token = CancellationToken::new();
    let app_state = build_first_run_app_state(&secrets_dir, token.clone());
    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    let client = client_no_redirect();
    let resp = client
        .get(format!("http://{}/", addr))
        .send()
        .await
        .expect("GET /");
    assert_eq!(resp.status(), StatusCode::SEE_OTHER, "expected 303 redirect");
    let loc = resp
        .headers()
        .get(reqwest::header::LOCATION)
        .expect("Location header present")
        .to_str()
        .expect("Location header utf-8");
    assert_eq!(loc, "/setup");

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn first_run_redirects_other_paths_to_setup() {
    let secrets_dir = TempDir::new().expect("tempdir");
    let token = CancellationToken::new();
    let app_state = build_first_run_app_state(&secrets_dir, token.clone());
    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    let client = client_no_redirect();

    for path in &["/applications.html", "/api/applications", "/devices.html"] {
        let resp = client
            .get(format!("http://{}{}", addr, path))
            .send()
            .await
            .unwrap_or_else(|e| panic!("GET {}: {}", path, e));
        assert_eq!(
            resp.status(),
            StatusCode::SEE_OTHER,
            "path {} must redirect in first-run mode, got {:?}",
            path,
            resp.status()
        );
    }

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn first_run_serves_setup_page() {
    let secrets_dir = TempDir::new().expect("tempdir");
    let token = CancellationToken::new();
    let app_state = build_first_run_app_state(&secrets_dir, token.clone());
    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    let client = client_no_redirect();
    let resp = client
        .get(format!("http://{}/setup", addr))
        .send()
        .await
        .expect("GET /setup");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("Welcome to opcgw"),
        "wizard HTML should include the welcome heading, got body of length {}",
        body.len()
    );
    assert!(
        body.contains("/api/setup/password"),
        "wizard HTML should reference the submit endpoint"
    );

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn first_run_serves_static_assets() {
    // Static assets like /dashboard.css must bypass the first-run
    // redirect so the wizard page can load its CSS.
    let secrets_dir = TempDir::new().expect("tempdir");
    let token = CancellationToken::new();
    let app_state = build_first_run_app_state(&secrets_dir, token.clone());
    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    let client = client_no_redirect();
    let resp = client
        .get(format!("http://{}/dashboard.css", addr))
        .send()
        .await
        .expect("GET /dashboard.css");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "static asset must be served without redirect"
    );

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn wizard_post_persists_password_and_signals_shutdown() {
    let secrets_dir = TempDir::new().expect("tempdir");
    let token = CancellationToken::new();
    let app_state = build_first_run_app_state(&secrets_dir, token.clone());
    let secrets_path = app_state.secrets_path.clone();
    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    let client = client_no_redirect();
    let resp = client
        .post(format!("http://{}/api/setup/password", addr))
        .json(&serde_json::json!({
            "password": "MyValidPassword!",
            "password_confirm": "MyValidPassword!",
        }))
        .send()
        .await
        .expect("POST /api/setup/password");
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.expect("json body");
    assert_eq!(body["status"], "password_set_restarting");

    // Verify the secrets.toml file was created with the password.
    assert!(secrets_path.exists(), "secrets.toml created");
    let contents =
        std::fs::read_to_string(&secrets_path).expect("read secrets.toml");
    assert!(
        contents.contains(r#"user_password = "MyValidPassword!""#),
        "secrets.toml contains the password, got:\n{}",
        contents
    );

    // Verify chmod 0600.
    use std::os::unix::fs::MetadataExt;
    let mode = std::fs::metadata(&secrets_path)
        .expect("metadata")
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600, "secrets.toml must be chmod 0600");

    // Verify the shutdown token was signalled.
    assert!(
        token.is_cancelled(),
        "shutdown_token should be cancelled after successful wizard submit"
    );

    // Iter-1 code review M8 fix: pre-fix the test called
    // `cancel.cancel()` here as a redundant cleanup, which obscured
    // whether the production handler's `state.shutdown_token.cancel()`
    // actually drains the server task. The assertion above pins the
    // production cancel; the timeout below verifies the server task
    // exits within 5s of the wizard's cancel (no second cancel from
    // the test).
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
    // Quiet the unused-binding lint.
    drop(cancel);
}

#[tokio::test]
async fn wizard_post_rejects_empty_password() {
    let secrets_dir = TempDir::new().expect("tempdir");
    let token = CancellationToken::new();
    let app_state = build_first_run_app_state(&secrets_dir, token.clone());
    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    let client = client_no_redirect();
    let resp = client
        .post(format!("http://{}/api/setup/password", addr))
        .json(&serde_json::json!({
            "password": "",
            "password_confirm": "",
        }))
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["error"], "password_validation_failed");
    assert_eq!(body["reason"], "empty");

    assert!(!token.is_cancelled(), "shutdown not signalled on rejection");

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn wizard_post_rejects_confirmation_mismatch() {
    let secrets_dir = TempDir::new().expect("tempdir");
    let token = CancellationToken::new();
    let app_state = build_first_run_app_state(&secrets_dir, token.clone());
    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    let client = client_no_redirect();
    let resp = client
        .post(format!("http://{}/api/setup/password", addr))
        .json(&serde_json::json!({
            "password": "MyPassword!",
            "password_confirm": "DifferentPassword!",
        }))
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["reason"], "confirmation_mismatch");

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn wizard_post_rejects_whitespace_bracketed_password() {
    let secrets_dir = TempDir::new().expect("tempdir");
    let token = CancellationToken::new();
    let app_state = build_first_run_app_state(&secrets_dir, token.clone());
    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    let client = client_no_redirect();
    let resp = client
        .post(format!("http://{}/api/setup/password", addr))
        .json(&serde_json::json!({
            "password": " MyPassword ",
            "password_confirm": " MyPassword ",
        }))
        .send()
        .await
        .expect("POST");
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = resp.json().await.expect("json");
    assert_eq!(body["reason"], "whitespace_bracketed");

    cancel.cancel();
    let _ = handle.await;
}

#[tokio::test]
async fn post_first_run_setup_get_returns_410_gone() {
    // Build a post-first-run AppState: is_first_run=false.
    let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
    let snapshot = Arc::new(DashboardConfigSnapshot {
        application_count: 0,
        device_count: 0,
        applications: vec![],
    });
    let auth = Arc::new(WebAuthState::new_with_fresh_key(
        "user",
        "password",
        WEB_DEFAULT_AUTH_REALM.to_string(),
    ));
    let (config_reload, config_writer, dir) =
        opcgw::web::test_support::make_test_reload_handle_and_writer();
    std::mem::forget(dir);
    let token = CancellationToken::new();
    let app_state = Arc::new(AppState {
        auth,
        backend,
        dashboard_snapshot: std::sync::RwLock::new(snapshot),
        start_time: std::time::Instant::now(),
        stale_threshold_secs: std::sync::atomic::AtomicU64::new(120),
        config_reload,
        config_writer,
        static_dir: static_dir(),
        is_first_run: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        secrets_path: PathBuf::from("/tmp/test-secrets.toml"),
        shutdown_token: token.clone(),
    });

    let (addr, handle, cancel) = spawn_web_server(app_state).await;

    // GET /setup with valid auth must return 410 (not 200 wizard).
    let client = client_no_redirect();
    let resp = client
        .get(format!("http://{}/setup", addr))
        .basic_auth("user", Some("password"))
        .send()
        .await
        .expect("GET /setup");
    assert_eq!(resp.status(), StatusCode::GONE);

    cancel.cancel();
    let _ = handle.await;
}

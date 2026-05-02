// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 9-1 integration tests: embedded Axum web server + Basic auth
// (FR50, NFR11, NFR12, FR41).
//
// What these tests pin (the "shape contract"):
//   - AC#2 (FR50, NFR11): every route requires Basic auth — unauth'd
//           GET / returns 401 with WWW-Authenticate; auth'd returns
//           the placeholder static-file body (or 200 from the smoke
//           endpoint).
//   - AC#3 (NFR12): a failed authentication emits the
//           `event="web_auth_failed"` audit event with `source_ip` (the
//           IP, not the port), the sanitised submitted user, the
//           request path, and the discriminating `reason` field.
//   - AC#4 (lines 783-784): the web server respects the
//           CancellationToken — cancellation drains the listener
//           and the spawn task joins within a few seconds.
//   - AC#5 (line 785, FR41): static placeholder files are served from
//           `static/` with the auth middleware applied. The
//           `<meta name="viewport">` tag is present in the body
//           (FR41 mobile-responsive marker).
//
// Notes on the test harness:
//   - We spawn the web server on an ephemeral port via the same
//     `web::run` entry point production uses. No mocking — real
//     reqwest client + real Axum router.
//   - Tests are `#[serial_test::serial]` because they install a
//     process-wide tracing subscriber via `init_test_subscriber()`
//     for `event="web_auth_failed"` capture.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use reqwest::header;
use reqwest::StatusCode;
use tempfile::TempDir;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{fmt as tracing_fmt, layer::SubscriberExt, Layer};

use opcgw::utils::{WEB_DEFAULT_AUTH_REALM, WEB_DEFAULT_BIND_ADDRESS};
use opcgw::web::{auth::WebAuthState, build_router, run as web_run};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-9-1";
const TEST_REALM: &str = "opcgw-9-1";

/// Install a global tracing subscriber that pipes events into
/// `tracing_test`'s capture buffer. Same shape as
/// `tests/opcua_subscription_spike.rs::init_test_subscriber` (issue
/// #101 fixes — panic on install failure).
///
/// **DO NOT add `#[tracing_test::traced_test]` to any test in this
/// file** — it installs its own subscriber and `set_global_default`
/// would fail.
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

fn clear_captured_buffer() {
    let mut buf = tracing_test::internal::global_buf()
        .lock()
        .expect("clear_captured_buffer: tracing-test buffer mutex poisoned");
    buf.clear();
}

fn captured_log_line_contains_all(needles: &[&str]) -> bool {
    let raw = tracing_test::internal::global_buf().lock().unwrap().clone();
    let s = String::from_utf8_lossy(&raw);
    s.lines()
        .any(|line| needles.iter().all(|n| line.contains(n)))
}

/// Bounded-retry poll for a captured log line. Returns `true` once a
/// line matching every needle appears in the global tracing-test
/// buffer; returns `false` if the budget elapses without a match.
async fn wait_for_captured_log(needles: &[&str], budget: Duration) -> bool {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if captured_log_line_contains_all(needles) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Spawn a web server on an ephemeral port + return its bound
/// `SocketAddr` plus a CancellationToken to stop it. Mirrors the
/// `setup_test_server*` shape used by the OPC UA integration tests.
struct TestWebServer {
    addr: SocketAddr,
    cancel: CancellationToken,
    handle: Option<tokio::task::JoinHandle<()>>,
    /// Hold the static-files temp dir alive for the lifetime of the
    /// server — `Drop` cleans up on test exit.
    _static_dir: TempDir,
}

impl Drop for TestWebServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Pick an ephemeral TCP port (matches `tests/common/mod.rs`).
async fn pick_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

/// Build a minimal `static/` directory shape for tests:
///   - index.html with a `<meta viewport>` tag (so AC#5's
///     mobile-responsive marker is present)
///   - api/health is the smoke endpoint built by `build_router`,
///     not a static file
async fn build_test_static_dir() -> TempDir {
    let tmp = TempDir::new().expect("test static tmp dir");
    let index = tmp.path().join("index.html");
    tokio::fs::write(
        &index,
        b"<!doctype html>\n\
          <html lang=\"en\">\n\
          <head>\n\
          <meta charset=\"utf-8\">\n\
          <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n\
          <title>opcgw - Test</title>\n\
          </head>\n\
          <body><p>test fixture</p></body>\n\
          </html>\n",
    )
    .await
    .expect("write test index.html");
    tmp
}

async fn setup_test_web_server() -> TestWebServer {
    let static_tmp = build_test_static_dir().await;
    let port = pick_free_port().await;
    // Bind to 127.0.0.1 specifically — production defaults to 0.0.0.0
    // but on shared CI runners we don't want to advertise the test
    // port externally even briefly.
    let bind_ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
    let addr = SocketAddr::new(bind_ip, port);

    let auth_state = Arc::new(WebAuthState::new_with_fresh_key(
        TEST_USER,
        TEST_PASSWORD,
        TEST_REALM.to_string(),
    ));
    let router = build_router(auth_state, static_tmp.path().to_path_buf());
    let cancel = CancellationToken::new();
    let cancel_for_run = cancel.clone();
    let realm = TEST_REALM.to_string();

    let handle = tokio::spawn(async move {
        let _ = web_run(addr, router, &realm, cancel_for_run).await;
    });

    // Wait for the listener to bind. `web::run` calls `TcpListener::bind`
    // synchronously (in the async sense — before the first request
    // is awaited) before reaching the serve loop, so polling for a
    // successful connect against the bound port covers it.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("web server did not bind to {addr} within 5s");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    TestWebServer {
        addr,
        cancel,
        handle: Some(handle),
        _static_dir: static_tmp,
    }
}

fn url(server: &TestWebServer, path: &str) -> String {
    format!("http://{}{}", server.addr, path)
}

fn build_basic_auth(user: &str, pass: &str) -> String {
    let blob = BASE64_STANDARD.encode(format!("{user}:{pass}").as_bytes());
    format!("Basic {blob}")
}

// =====================================================================
// AC#2 + AC#3: missing Authorization header → 401 + WWW-Authenticate
// + event="web_auth_failed" reason=missing
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_unauthenticated_request_returns_401_and_emits_audit_event() {
    init_test_subscriber();
    clear_captured_buffer();

    let server = setup_test_web_server().await;
    let client = common::build_http_client(Duration::from_secs(5));

    let resp = client
        .get(url(&server, "/api/health"))
        .send()
        .await
        .expect("GET /api/health");

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let www = resp
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("WWW-Authenticate header on 401")
        .to_str()
        .expect("ASCII WWW-Authenticate value");
    assert!(
        www.starts_with("Basic realm=\""),
        "got WWW-Authenticate: {www:?}"
    );
    assert!(
        www.contains(TEST_REALM),
        "WWW-Authenticate must include configured realm; got {www:?}"
    );

    // Audit event: warn-level, event=web_auth_failed, reason=missing,
    // path=/api/health. Source-IP must be 127.0.0.1 (the test
    // client's bind).
    assert!(
        wait_for_captured_log(
            &[
                "event=\"web_auth_failed\"",
                "reason=\"missing\"",
                "path=\"/api/health\"",
                "source_ip=127.0.0.1",
            ],
            Duration::from_secs(2),
        )
        .await,
        "missing-auth audit event must fire — captured buffer:\n{}",
        String::from_utf8_lossy(
            &tracing_test::internal::global_buf().lock().unwrap().clone()
        )
    );
}

// =====================================================================
// AC#2 + AC#3: malformed Authorization scheme → 401 + reason=malformed_scheme
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_malformed_scheme_returns_401_and_emits_audit_event() {
    init_test_subscriber();
    clear_captured_buffer();

    let server = setup_test_web_server().await;
    let client = common::build_http_client(Duration::from_secs(5));

    let resp = client
        .get(url(&server, "/api/health"))
        .header(header::AUTHORIZATION, "Bearer some-token")
        .send()
        .await
        .expect("GET /api/health (Bearer)");

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    assert!(
        wait_for_captured_log(
            &[
                "event=\"web_auth_failed\"",
                "reason=\"malformed_scheme\"",
            ],
            Duration::from_secs(2),
        )
        .await,
        "malformed-scheme audit event must fire"
    );
}

// =====================================================================
// AC#2 + AC#3: wrong password → 401 + reason=password_mismatch +
// the sanitised submitted user appears in the audit event
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_wrong_password_returns_401_and_emits_audit_event_with_user() {
    init_test_subscriber();
    clear_captured_buffer();

    let server = setup_test_web_server().await;
    let client = common::build_http_client(Duration::from_secs(5));

    let resp = client
        .get(url(&server, "/api/health"))
        .header(
            header::AUTHORIZATION,
            build_basic_auth(TEST_USER, "definitely-wrong"),
        )
        .send()
        .await
        .expect("GET /api/health");

    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    // The `user` field is emitted via `%`-Display format so it
    // renders unquoted (`user=opcua-user`) — not `user="opcua-user"`.
    assert!(
        wait_for_captured_log(
            &[
                "event=\"web_auth_failed\"",
                "reason=\"password_mismatch\"",
                &format!("user={TEST_USER}"),
            ],
            Duration::from_secs(2),
        )
        .await,
        "password-mismatch audit event with sanitised user must fire — captured:\n{}",
        String::from_utf8_lossy(
            &tracing_test::internal::global_buf().lock().unwrap().clone()
        )
    );
}

// =====================================================================
// AC#2: correct credentials → 200 OK from the api/health smoke
// endpoint (proves auth middleware forwards the request to inner
// handler).
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_correct_credentials_serve_api_health() {
    init_test_subscriber();
    clear_captured_buffer();

    let server = setup_test_web_server().await;
    let client = common::build_http_client(Duration::from_secs(5));

    let resp = client
        .get(url(&server, "/api/health"))
        .header(
            header::AUTHORIZATION,
            build_basic_auth(TEST_USER, TEST_PASSWORD),
        )
        .send()
        .await
        .expect("GET /api/health");

    assert_eq!(resp.status(), StatusCode::OK);
    let ct = resp
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|h| h.to_str().unwrap_or("").to_string())
        .unwrap_or_default();
    assert!(
        ct.contains("application/json"),
        "Content-Type should be JSON, got: {ct}"
    );
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("\"status\""),
        "body should include the status field, got: {body}"
    );
}

// =====================================================================
// AC#5 (FR41): static index.html is served behind auth and contains
// the <meta viewport> mobile-responsive marker.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_static_file_served_with_auth() {
    init_test_subscriber();
    clear_captured_buffer();

    let server = setup_test_web_server().await;
    let client = common::build_http_client(Duration::from_secs(5));

    // Unauth → 401 (path inherits the auth layer).
    let resp = client
        .get(url(&server, "/index.html"))
        .send()
        .await
        .expect("GET /index.html (unauth)");
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    // Auth'd → 200 + body includes the <meta viewport> tag.
    let resp = client
        .get(url(&server, "/index.html"))
        .header(
            header::AUTHORIZATION,
            build_basic_auth(TEST_USER, TEST_PASSWORD),
        )
        .send()
        .await
        .expect("GET /index.html (auth)");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("<meta name=\"viewport\""),
        "static index.html must include viewport meta for FR41; got body:\n{body}"
    );
}

// =====================================================================
// AC#4: graceful shutdown via CancellationToken — the spawn task joins
// within a small budget after cancel().
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_graceful_shutdown_via_cancellation_token() {
    init_test_subscriber();
    clear_captured_buffer();

    let mut server = setup_test_web_server().await;

    // Take the handle so we can `await` it after cancel() — the
    // `Drop` impl would also abort it, but here we want to verify
    // graceful join.
    let handle = server.handle.take().expect("handle");

    server.cancel.cancel();

    let join_result = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(
        join_result.is_ok(),
        "web server task did not join within 5s of cancellation"
    );
    let inner = join_result.unwrap();
    assert!(
        inner.is_ok(),
        "web server task joined with error: {inner:?}"
    );

    // After cancellation the listener should be released — connect
    // attempts on the bound addr should fail (port closed).
    let connect = tokio::net::TcpStream::connect(&server.addr).await;
    assert!(
        connect.is_err(),
        "port should be free after graceful shutdown, got Ok({connect:?})"
    );
}

// =====================================================================
// AC#1 sanity: `WEB_DEFAULT_*` constants are wired through (compile
// + value pin so a future re-numbering of port/realm trips a test).
// =====================================================================
#[test]
fn test_web_defaults_are_stable() {
    assert_eq!(WEB_DEFAULT_BIND_ADDRESS, "0.0.0.0");
    assert_eq!(WEB_DEFAULT_AUTH_REALM, "opcgw");
}

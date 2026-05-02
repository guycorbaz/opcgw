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
//           <meta name="viewport"> tag is present in the body
//           (FR41 mobile-responsive marker). Unauth'd `GET
//           /nonexistent.html` returns 401 (not 404) — the layer-
//           after-fallback ordering invariant.
//   - Review iter-1: production `WebAuthState::new(&AppConfig, realm)`
//           is exercised by a dedicated unit test; route-vs-fallback
//           ordering is pinned; `[web].enabled = false` no-bind path
//           is pinned.
//
// Notes on the test harness:
//   - We spawn the web server on an ephemeral port via the same
//     `web::bind` + `web::run` entry points production uses. No
//     mocking — real reqwest client + real Axum router.
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
use opcgw::web::{auth::WebAuthState, bind as web_bind, build_router, run as web_run};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-9-1";
const TEST_REALM: &str = "opcgw-9-1";

/// Install a global tracing subscriber that pipes events into
/// `tracing_test`'s capture buffer. Same shape as
/// `tests/opcua_subscription_spike.rs::init_test_subscriber` (issue
/// #101 fixes — panic on install failure).
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

/// Bounded-retry poll for a captured log line.
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
/// `SocketAddr` plus a CancellationToken to stop it.
struct TestWebServer {
    addr: SocketAddr,
    cancel: CancellationToken,
    handle: Option<tokio::task::JoinHandle<()>>,
    /// Hold the static-files temp dir alive for the lifetime of the
    /// server.
    _static_dir: TempDir,
}

impl Drop for TestWebServer {
    fn drop(&mut self) {
        // Review iter-1 patch: `cancel.cancel()` is the only cleanup.
        // The handle is dropped (not aborted) — `axum::serve`'s
        // graceful-shutdown path responds to `cancel.cancelled()` so
        // the spawned task exits naturally. `abort()`-mid-shutdown
        // can panic-poison subsequent serial tests on Windows; on
        // Linux today this is observably benign but the cleaner
        // shape avoids the trap.
        self.cancel.cancel();
    }
}

/// Pick an ephemeral TCP port.
async fn pick_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

/// Build a minimal `static/` directory shape for tests.
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

/// Bind + spawn a test web server. Uses the production `web::bind` +
/// `web::run` entry points so the test harness exercises the same
/// fail-fast bind path as the gateway's `main`.
async fn setup_test_web_server() -> TestWebServer {
    let static_tmp = build_test_static_dir().await;
    let port = pick_free_port().await;
    // Bind to 127.0.0.1 explicitly — production default is 0.0.0.0
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

    // Bind synchronously (matches production's D1 fail-fast pattern).
    let listener = web_bind(addr).await.expect("test web server bind");

    let handle = tokio::spawn(async move {
        let _ = web_run(listener, router, &realm, cancel_for_run).await;
    });

    // Probe the bound port to confirm the serve loop is accepting.
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

/// Match either IPv4 or IPv6 loopback in audit-event source_ip
/// assertions. Linux x86_64 default is `127.0.0.1`; some platforms
/// or dual-stack configs route loopback as `::1`. Accept both.
fn source_ip_loopback_needles(line: &str) -> bool {
    line.contains("source_ip=127.0.0.1") || line.contains("source_ip=::1")
}

/// Match a captured log line that contains all `static_needles` AND
/// passes `extra_predicate`. Used to combine static-string and
/// dynamic (e.g. IP-family-flexible) match conditions.
fn captured_log_line_matches(
    static_needles: &[&str],
    extra_predicate: impl Fn(&str) -> bool,
) -> bool {
    let raw = tracing_test::internal::global_buf().lock().unwrap().clone();
    let s = String::from_utf8_lossy(&raw);
    s.lines().any(|line| {
        static_needles.iter().all(|n| line.contains(n)) && extra_predicate(line)
    })
}

async fn wait_for_audit_with_loopback_ip(
    static_needles: &[&str],
    budget: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + budget;
    loop {
        if captured_log_line_matches(static_needles, source_ip_loopback_needles) {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// =====================================================================
// AC#2 + AC#3: missing Authorization header → 401 + WWW-Authenticate
// + event="web_auth_failed" reason=missing
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_unauthenticated_request_returns_401_and_emits_audit_event() {
    init_test_subscriber();
    let server = setup_test_web_server().await;
    // Review iter-1: clear the buffer AFTER setup so the
    // event="web_server_started" startup line doesn't appear in our
    // assertion's match window.
    clear_captured_buffer();
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

    // Audit event must include event/reason/path AND a loopback
    // source_ip (IPv4 or IPv6).
    assert!(
        wait_for_audit_with_loopback_ip(
            &[
                "event=\"web_auth_failed\"",
                "reason=\"missing\"",
                "path=/api/health",
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_malformed_scheme_returns_401_and_emits_audit_event() {
    init_test_subscriber();
    let server = setup_test_web_server().await;
    clear_captured_buffer();
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_wrong_password_returns_401_and_emits_audit_event_with_user() {
    init_test_subscriber();
    let server = setup_test_web_server().await;
    clear_captured_buffer();
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
    // The `user` field is emitted via Display (`%`) format so it
    // renders unquoted (`user=opcua-user`).
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
// endpoint + no audit event emitted on the success path.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_correct_credentials_serve_api_health() {
    init_test_subscriber();
    let server = setup_test_web_server().await;
    clear_captured_buffer();
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

    // Review iter-1: assert the audit event did NOT fire on the
    // success path. Give the tracing layer a moment to flush, then
    // grep the buffer.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        !captured_log_line_contains_all(&["event=\"web_auth_failed\""]),
        "web_auth_failed must NOT fire on success path — captured:\n{}",
        String::from_utf8_lossy(
            &tracing_test::internal::global_buf().lock().unwrap().clone()
        )
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
    let server = setup_test_web_server().await;
    clear_captured_buffer();
    let client = common::build_http_client(Duration::from_secs(5));

    // Unauth → 401.
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
// AC#5 review iter-1: unauth GET /nonexistent.html returns 401, NOT
// 404 — pins the layer-after-fallback ordering invariant. Without
// this test, a future router refactor (e.g. nesting groups) could
// silently re-introduce the 404-vs-401 differential that leaks the
// directory layout to unauthenticated probers.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_unauth_unknown_path_returns_401() {
    init_test_subscriber();
    let server = setup_test_web_server().await;
    clear_captured_buffer();
    let client = common::build_http_client(Duration::from_secs(5));

    let resp = client
        .get(url(&server, "/this-path-does-not-exist.html"))
        .send()
        .await
        .expect("GET /this-path-does-not-exist.html");
    assert_eq!(
        resp.status(),
        StatusCode::UNAUTHORIZED,
        "unauth unknown path must be 401, not 404 — otherwise the \
         404-vs-401 differential leaks the directory layout"
    );
}

// =====================================================================
// Review iter-1: route precedence — `/api/health` route handler must
// win over a same-path file in the static directory. Pins the
// `route(...)` + `fallback_service(...)` ordering invariant.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_route_wins_over_fallback_service() {
    init_test_subscriber();

    // Build a static dir with a file at api/health that the route
    // handler MUST override.
    let static_tmp = TempDir::new().expect("test static tmp dir");
    let api_dir = static_tmp.path().join("api");
    tokio::fs::create_dir_all(&api_dir).await.expect("mkdir api");
    tokio::fs::write(api_dir.join("health"), b"INTENTIONALLY-WRONG-FILE")
        .await
        .expect("write decoy file");

    let port = pick_free_port().await;
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let auth_state = Arc::new(WebAuthState::new_with_fresh_key(
        TEST_USER,
        TEST_PASSWORD,
        TEST_REALM.to_string(),
    ));
    let router = build_router(auth_state, static_tmp.path().to_path_buf());
    let cancel = CancellationToken::new();
    let cancel_for_run = cancel.clone();
    let realm = TEST_REALM.to_string();
    let listener = web_bind(addr).await.expect("test bind");
    let handle = tokio::spawn(async move {
        let _ = web_run(listener, router, &realm, cancel_for_run).await;
    });

    // Probe.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::net::TcpStream::connect(&addr).await.is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("server did not bind");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let client = common::build_http_client(Duration::from_secs(5));
    let resp = client
        .get(format!("http://{addr}/api/health"))
        .header(
            header::AUTHORIZATION,
            build_basic_auth(TEST_USER, TEST_PASSWORD),
        )
        .send()
        .await
        .expect("GET /api/health");
    assert_eq!(resp.status(), StatusCode::OK);
    let body = resp.text().await.expect("body");
    assert!(
        body.contains("\"status\""),
        "route handler must win over static file — got body: {body}"
    );
    assert!(
        !body.contains("INTENTIONALLY-WRONG-FILE"),
        "static file should NOT be served for /api/health"
    );

    cancel.cancel();
    let _ = tokio::time::timeout(Duration::from_secs(5), handle).await;
}

// =====================================================================
// Iter-1 + iter-2: pin `WEB_DEFAULT_ENABLED = false` at COMPILE TIME
// via a `const _: () = assert!(...)` outside of any test runtime.
// A future edit to `src/utils.rs` flipping the constant to `true` MUST
// fail `cargo build`, not just `cargo test` — flipping the default
// would expose the web UI on every fresh deployment without operator
// opt-in (Story 9-1 AC#1).
//
// This is NOT a behavioural test of the no-bind code path; the
// behavioural contract that "main does not call `tokio::spawn` for the
// web task when `[web].enabled = false`" is verified by reading
// `src/main.rs` and the structural test below (which pins the constant
// the branch reads).
// =====================================================================
const _: () = assert!(
    !opcgw::utils::WEB_DEFAULT_ENABLED,
    "WEB_DEFAULT_ENABLED must remain `false` — flipping it to `true` \
     would expose the web UI on every fresh deployment without \
     operator opt-in. Story 9-1 AC#1."
);

#[test]
#[serial_test::serial]
fn test_web_default_enabled_constant_is_false() {
    // Runtime mirror of the compile-time assertion above. Kept so
    // `cargo test` reports a named test passing for AC#1's default-off
    // contract (the const-assert above is anonymous and produces no
    // test-runner output). Clippy correctly notes this is evaluable
    // at compile time — that's the point; the const-assert above is
    // the load-bearing enforcement.
    #[allow(clippy::assertions_on_constants)]
    {
        assert!(!opcgw::utils::WEB_DEFAULT_ENABLED);
    }
}

// =====================================================================
// Iter-2 regression pin: the embedded web server must NOT
// self-terminate after the GRACEFUL_SHUTDOWN_BUDGET_SECS budget
// elapses when no shutdown was requested. Iter-1 patch incorrectly
// wrapped `axum::serve(...).with_graceful_shutdown(...)` in a
// `tokio::time::timeout(5s, ...)` that fired regardless of whether
// `cancel` had fired — making the server die after 5 s of idle
// uptime. Iter-2 fix: the budget applies only to the post-cancel
// drain via `tokio::select!`. This test idles the server past the
// budget and confirms the port stays bound.
//
// Wall-clock: ~6 s.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_server_stays_bound_past_shutdown_budget_when_idle() {
    init_test_subscriber();
    let server = setup_test_web_server().await;
    clear_captured_buffer();
    let client = common::build_http_client(Duration::from_secs(5));

    // Idle for 6 s (> the 5 s GRACEFUL_SHUTDOWN_BUDGET_SECS). On
    // the iter-1 buggy code, the server self-terminates here.
    tokio::time::sleep(Duration::from_secs(6)).await;

    // Issue an authenticated request — must succeed if the server
    // is still bound. On the iter-1 buggy code, this returns a
    // connection error (port closed).
    let resp = client
        .get(url(&server, "/api/health"))
        .header(
            header::AUTHORIZATION,
            build_basic_auth(TEST_USER, TEST_PASSWORD),
        )
        .send()
        .await
        .expect("server should still be reachable after 6 s of idle uptime");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "server must remain operational past the post-cancel drain budget when no \
         shutdown was requested — iter-1 bug let the timeout fire on the entire \
         serve future, killing the server after exactly 5 s of normal uptime"
    );
}

// =====================================================================
// AC#4: graceful shutdown via CancellationToken.
// =====================================================================
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[serial_test::serial]
async fn test_graceful_shutdown_via_cancellation_token() {
    init_test_subscriber();

    let mut server = setup_test_web_server().await;
    let handle = server.handle.take().expect("handle");

    server.cancel.cancel();

    let join_result = tokio::time::timeout(Duration::from_secs(10), handle).await;
    assert!(
        join_result.is_ok(),
        "web server task did not join within 10s of cancellation"
    );
    let inner = join_result.unwrap();
    assert!(
        inner.is_ok(),
        "web server task joined with error: {inner:?}"
    );

    let connect = tokio::net::TcpStream::connect(&server.addr).await;
    assert!(
        connect.is_err(),
        "port should be free after graceful shutdown, got Ok({connect:?})"
    );
}

// =====================================================================
// AC#1 sanity: `WEB_DEFAULT_*` constants are wired through.
// Marked `#[serial_test::serial]` for spec compliance with Task 4's
// "All tests `#[serial_test::serial]`" rule (review iter-1).
// =====================================================================
#[test]
#[serial_test::serial]
fn test_web_defaults_are_stable() {
    assert_eq!(WEB_DEFAULT_BIND_ADDRESS, "0.0.0.0");
    assert_eq!(WEB_DEFAULT_AUTH_REALM, "opcgw");
}

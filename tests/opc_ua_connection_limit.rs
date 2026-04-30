// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 7-3 integration tests: OPC UA connection limiting (FR44).
//
// What these tests pin (the "shape contract"):
//   - AC#2: `ServerBuilder::max_sessions(N)` is wired and enforced. The
//           configured limit is honoured (N concurrent sessions activate),
//           the (N+1)th is rejected, existing sessions are unaffected, and
//           the slot decrements on disconnect (a fresh session can take
//           the freed slot).
//   - AC#3: At-limit accepts emit `event="opcua_session_count_at_limit"`
//           via the `AtLimitAcceptLayer` tracing-Layer. Periodic gauge
//           emits `event="opcua_session_count" current=N limit=L` every
//           ~5s. Disconnects decrement the count visible via
//           `read_current_session_count`.
//
// Mirror of tests/opc_ua_security_endpoints.rs harness — keep in sync;
// refactor into tests/common/ when the third user appears.

use std::sync::Arc;
use std::time::Duration;

use opcua::client::{ClientBuilder, IdentityToken, Password as ClientPassword, Session};
use opcua::types::{
    EndpointDescription, MessageSecurityMode, NodeId, ReadValueId, TimestampsToReturn,
    UserTokenPolicy, UserTokenType,
};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{fmt as tracing_fmt, layer::SubscriberExt, Layer};

use opcgw::config::{
    AppConfig, ChirpStackApplications, ChirpstackDevice, ChirpstackPollerConfig,
    CommandValidationConfig, Global, OpcMetricTypeConfig, OpcUaConfig, ReadMetric, StorageConfig,
};
use opcgw::opc_ua::OpcUa;
use opcgw::storage::{ConnectionPool, SqliteBackend, StorageBackend};
use opcgw::utils::OPCUA_SESSION_GAUGE_INTERVAL_SECS;

// -----------------------------------------------------------------------
// Test harness (mirror of tests/opc_ua_security_endpoints.rs:60-353)
// -----------------------------------------------------------------------

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-7-3";

/// Install a global tracing subscriber that includes both
/// `tracing_test`'s capture layer (so assertions can read the global
/// buffer) AND our `AtLimitAcceptLayer` (so the at-limit warn event is
/// produced under the same dispatcher that captures it). Story 7-3
/// AC#3 requires this composition — `#[traced_test]` would replace
/// the registry and drop the at-limit layer.
///
/// **DO NOT add `#[traced_test]` to any test in this file** — it
/// installs its own global subscriber and `set_global_default` would
/// fail. Use the harness-provided `captured_log_line_contains_all`
/// helper instead.
///
/// `set_global_default` may only be called once per process, so we
/// guard with a `OnceLock`. Each test calls this at the start. The
/// `set_global_default` failure path is fail-soft (logs once, does
/// not panic) so a future incompatible change to the test framework
/// can't take down every test in this binary.
fn init_test_subscriber() {
    static INIT: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    INIT.get_or_init(|| {
        let buf: &'static std::sync::Mutex<Vec<u8>> = tracing_test::internal::global_buf();
        let mock = tracing_test::internal::MockWriter::new(buf);
        let fmt_layer = tracing_fmt::layer()
            .with_writer(mock)
            .with_level(true)
            .with_ansi(false)
            // Capture every event so the at-limit warn (and async-opcua's
            // own info-level events that the layer correlates against)
            // both reach the buffer. Mirrors `tracing-test`'s
            // `no-env-filter` feature.
            .with_filter(tracing_subscriber::filter::LevelFilter::TRACE);
        let subscriber = tracing_subscriber::Registry::default()
            .with(fmt_layer)
            .with(opcgw::opc_ua_session_monitor::AtLimitAcceptLayer::new());
        if let Err(e) = tracing::subscriber::set_global_default(subscriber) {
            // Fail-soft: log once via stderr (the dispatcher is
            // unavailable). Tests that depend on the layer will see
            // assertion failures with clear messages downstream.
            eprintln!(
                "init_test_subscriber: set_global_default failed ({e:?}) — \
                 the at-limit layer will not be active. Did another test \
                 framework (e.g. #[traced_test]) install a subscriber first?"
            );
        }
    });
}

/// Truncate the process-wide `tracing_test` capture buffer. Call at
/// the START of every test that asserts on captured log content —
/// without this, lines from earlier serial tests (e.g. an earlier
/// test's `current=2 limit=2` from a `max=2` server) can satisfy the
/// next test's needle set and mask a real regression. Code-review
/// feedback 2026-04-29.
fn clear_captured_buffer() {
    if let Ok(mut buf) = tracing_test::internal::global_buf().lock() {
        buf.clear();
    }
}

/// Assert that ALL of `needles` co-occur on a SINGLE line of the
/// captured tracing-test buffer. The global buffer is process-wide;
/// checking each substring independently risks satisfying the
/// assertion via lines from earlier tests (e.g. a different test's
/// `current=2` could be mistaken for the one we want).
fn captured_log_line_contains_all(needles: &[&str]) -> bool {
    let raw = tracing_test::internal::global_buf().lock().unwrap().clone();
    let s = String::from_utf8_lossy(&raw);
    s.lines()
        .any(|line| needles.iter().all(|n| line.contains(n)))
}

fn user_name_policy() -> UserTokenPolicy {
    UserTokenPolicy {
        token_type: UserTokenType::UserName,
        ..UserTokenPolicy::anonymous()
    }
}

struct TestServer {
    port: u16,
    cancel: CancellationToken,
    handle: Option<tokio::task::JoinHandle<()>>,
    _tmp: TempDir,
}

impl TestServer {
    fn endpoint_url(&self, suffix: &str) -> String {
        format!("opc.tcp://127.0.0.1:{}{}", self.port, suffix)
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        // Story 7-3: also clear the at-limit-layer's shared state so the
        // next test starts from a clean slate.
        opcgw::opc_ua_session_monitor::clear_session_monitor_state();
    }
}

async fn pick_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

fn test_config(port: u16, pki_dir: &std::path::Path, max_connections: usize) -> AppConfig {
    AppConfig {
        global: Global {
            debug: true,
            prune_interval_minutes: 60,
            command_delivery_poll_interval_secs: 5,
            command_delivery_timeout_secs: 60,
            command_timeout_check_interval_secs: 10,
            history_retention_days: 7,
        },
        logging: None,
        chirpstack: ChirpstackPollerConfig {
            server_address: "http://127.0.0.1:18080".to_string(),
            api_token: "test-token".to_string(),
            tenant_id: "00000000-0000-0000-0000-000000000000".to_string(),
            polling_frequency: 10,
            retry: 1,
            delay: 1,
            list_page_size: 100,
        },
        opcua: OpcUaConfig {
            application_name: "opcgw-test".to_string(),
            application_uri: "urn:opcgw:test".to_string(),
            product_uri: "urn:opcgw:test:product".to_string(),
            diagnostics_enabled: true,
            hello_timeout: Some(5),
            host_ip_address: Some("127.0.0.1".to_string()),
            host_port: Some(port),
            create_sample_keypair: true,
            certificate_path: "own/cert.der".to_string(),
            private_key_path: "private/private.pem".to_string(),
            trust_client_cert: true,
            check_cert_time: false,
            pki_dir: pki_dir.to_string_lossy().into_owned(),
            user_name: TEST_USER.to_string(),
            user_password: TEST_PASSWORD.to_string(),
            stale_threshold_seconds: Some(120),
            max_connections: Some(max_connections),
            max_subscriptions_per_session: None,
            max_monitored_items_per_sub: None,
            max_message_size: None,
            max_chunk_count: None,
            max_history_data_results_per_node: None,
        },
        application_list: vec![ChirpStackApplications {
            application_name: "TestApp".to_string(),
            application_id: "00000000-0000-0000-0000-000000000001".to_string(),
            device_list: vec![ChirpstackDevice {
                device_name: "TestDevice".to_string(),
                device_id: "0000000000000001".to_string(),
                read_metric_list: vec![ReadMetric {
                    metric_name: "Temperature".to_string(),
                    chirpstack_metric_name: "temperature".to_string(),
                    metric_type: OpcMetricTypeConfig::Float,
                    metric_unit: Some("C".to_string()),
                }],
                device_command_list: None,
            }],
        }],
        storage: StorageConfig::default(),
        command_validation: CommandValidationConfig::default(),
    }
}

/// Spin up the gateway with `max_connections = max`. Mirror of
/// `setup_test_server` from `opc_ua_security_endpoints.rs` plus the
/// session-monitor state population (the at-limit layer reads it).
async fn setup_test_server_with_max(max: usize) -> TestServer {
    let tmp = TempDir::new().expect("create temp dir");
    let port = pick_free_port().await;
    let pki_dir = tmp.path().join("pki");
    let db_path = tmp.path().join("opcgw.db");

    let config = Arc::new(test_config(port, &pki_dir, max));
    let pool = Arc::new(
        ConnectionPool::new(db_path.to_str().expect("utf-8 db path"), 1)
            .expect("create connection pool"),
    );
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::with_pool(pool).expect("create backend"));

    let cancel = CancellationToken::new();
    let opc_ua = OpcUa::new(&config, backend, cancel.clone());

    let handle = tokio::spawn(async move {
        let _ = opc_ua.run().await;
    });

    // Wait for the server to bind.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!("OPC UA server did not bind to port {port} within 10s");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Wait until discovery responds — confirms async-opcua has fully
    // wired endpoint routing.
    {
        let probe_url = format!("opc.tcp://127.0.0.1:{port}/");
        let probe_tmp = TempDir::new().expect("probe pki tmp");
        let probe_client = build_client(probe_tmp.path());
        let probe_deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            match probe_client
                .get_server_endpoints_from_url(probe_url.as_str())
                .await
            {
                Ok(endpoints) if !endpoints.is_empty() => break,
                _ => {}
            }
            if std::time::Instant::now() >= probe_deadline {
                panic!("OPC UA server did not respond to discovery within 5s after bind");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    TestServer {
        port,
        cancel,
        handle: Some(handle),
        _tmp: tmp,
    }
}

fn build_client(client_pki: &std::path::Path) -> opcua::client::Client {
    // session_timeout=15_000ms keeps held sessions alive for the full
    // wall-clock of `test_max_sessions_enforced` (~10s) plus the
    // gauge-decrement test (~12s) while bounding leakage if a test
    // panics before disconnecting (HeldSession::Drop only aborts the
    // local task — server-side sessions linger until session_timeout).
    // 15s is the minimum that keeps the gauge-decrement test stable
    // and the leak window short. Code-review feedback 2026-04-29
    // (lowered from 60_000ms).
    ClientBuilder::new()
        .application_name("opcgw-test-client")
        .application_uri("urn:opcgw:test:client")
        .product_uri("urn:opcgw:test:client")
        .create_sample_keypair(true)
        .trust_server_certs(true)
        .verify_server_certs(false)
        .session_retry_limit(0)
        .session_timeout(15_000)
        .pki_dir(client_pki)
        .client()
        .expect("client build")
}

fn user_password_identity() -> IdentityToken {
    IdentityToken::UserName(
        TEST_USER.to_string(),
        ClientPassword(TEST_PASSWORD.to_string()),
    )
}

/// Owned, held-open OPC UA session for the duration of a test.
///
/// Callers MUST call `disconnect().await` for clean teardown. `Drop`
/// only aborts the spawned event-loop task — it cannot run async code,
/// so a server-side session can linger past the test's expectations if
/// the disconnect is skipped.
struct HeldSession {
    session: Arc<Session>,
    event_handle: Option<tokio::task::JoinHandle<opcua::types::StatusCode>>,
    /// Keep the client + temp PKI alive as long as the session.
    _client_tmp: TempDir,
    _client: opcua::client::Client,
}

impl HeldSession {
    async fn disconnect(mut self) {
        let _ = tokio::time::timeout(Duration::from_secs(2), self.session.disconnect()).await;
        if let Some(h) = self.event_handle.take() {
            h.abort();
            let _ = h.await;
        }
    }
}

impl Drop for HeldSession {
    fn drop(&mut self) {
        if let Some(h) = self.event_handle.take() {
            h.abort();
        }
    }
}

/// Open a session against the `None` endpoint with `identity` and hold
/// it open. Returns `None` if activation fails or times out (the
/// spawned event-loop task is aborted on the failure path).
async fn open_session_held(
    server: &TestServer,
    identity: IdentityToken,
    timeout_ms: u64,
) -> Option<HeldSession> {
    let client_tmp = TempDir::new().expect("client tmp");
    let mut client = build_client(client_tmp.path());
    let endpoint: EndpointDescription = (
        server.endpoint_url("/").as_str(),
        "None",
        MessageSecurityMode::None,
        user_name_policy(),
    )
        .into();

    let connect_result = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        client.connect_to_matching_endpoint(endpoint, identity),
    )
    .await
    .ok()?;
    let (session, event_loop) = connect_result.ok()?;
    session.disable_reconnects();
    let event_handle = event_loop.spawn();

    let connected = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        session.wait_for_connection(),
    )
    .await
    .unwrap_or(false);

    if !connected {
        let _ = tokio::time::timeout(Duration::from_secs(2), session.disconnect()).await;
        event_handle.abort();
        let _ = event_handle.await;
        return None;
    }

    Some(HeldSession {
        session,
        event_handle: Some(event_handle),
        _client_tmp: client_tmp,
        _client: client,
    })
}

/// Read the standard `Server_NamespaceArray` node. Used as a "still
/// alive" probe in AC#2 — proves an existing session continues to
/// serve requests after the cap is reached.
async fn read_namespace_array(session: &Session) -> Result<(), opcua::types::StatusCode> {
    let to_read = vec![ReadValueId::from(NodeId::new(0, 2255_u32))];
    let results = session
        .read(&to_read, TimestampsToReturn::Both, 0.0)
        .await
        .map_err(|_| opcua::types::StatusCode::BadCommunicationError)?;
    if results.is_empty() {
        return Err(opcua::types::StatusCode::BadUnexpectedError);
    }
    Ok(())
}

// -----------------------------------------------------------------------
// AC#2: max_sessions enforcement
// -----------------------------------------------------------------------

/// AC#2: configured limit is honoured, (N+1)th is rejected, existing
/// sessions are unaffected, slot decrements on disconnect.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_max_sessions_enforced() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(2).await;

    // Open 2 sessions concurrently. Both must activate.
    let identity1 = user_password_identity();
    let identity2 = user_password_identity();
    let (s1, s2) = tokio::join!(
        open_session_held(&server, identity1, 5_000),
        open_session_held(&server, identity2, 5_000),
    );
    let s1 = s1.expect("first session must activate");
    let s2 = s2.expect("second session must activate");

    // Give async-opcua a moment to register the sessions in the
    // diagnostics counter.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Attempt a 3rd session — must fail to activate within the
    // timeout. async-opcua surfaces `BadTooManySessions` via several
    // error paths; failure-to-activate within the bound is the
    // contract.
    let s3 = open_session_held(&server, user_password_identity(), 3_000).await;
    assert!(
        s3.is_none(),
        "3rd session must NOT activate while at limit (max_connections=2)"
    );

    // Existing session must still serve a Read.
    read_namespace_array(&s1.session)
        .await
        .expect("existing session 1 must remain functional after limit reached");

    // Disconnect session 1 cleanly. Then a 4th attempt must succeed
    // (slot freed). Allow a short grace window for the close to
    // propagate to the SessionManager / diagnostics counter.
    s1.disconnect().await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    let s4 = open_session_held(&server, user_password_identity(), 5_000).await;
    assert!(
        s4.is_some(),
        "after disconnecting session 1, a fresh session must be able to take the freed slot"
    );

    // Tear down remaining sessions.
    if let Some(s4) = s4 {
        s4.disconnect().await;
    }
    s2.disconnect().await;
}

/// Code-review feedback 2026-04-29: production shutdown path
/// (`OpcUa::run` firing `cancel_token`, reaping the gauge task, and
/// clearing the session-monitor static state) was previously not
/// exercised by any integration test — `TestServer::Drop` aborts the
/// spawned task, bypassing the production cleanup. This test holds a
/// session to ensure the path runs with non-trivial state, then fires
/// `cancel_token.cancel()` (instead of `handle.abort()`), awaits the
/// `OpcUa::run` task to completion, and asserts both that the task
/// exits `Ok(())` within a generous timeout AND that the static
/// `MonitorState` slot was cleared by the production cleanup
/// (`MonitorStateGuard::Drop` reached on natural unwind).
///
/// Resolves the AC#6 manual-smoke deferral and the production-shutdown
/// coverage gap together.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_shutdown_cleanliness_clears_state_and_reaps_gauge() {
    init_test_subscriber();
    clear_captured_buffer();
    let mut server = setup_test_server_with_max(2).await;

    // Establish non-trivial state — at least one session active so the
    // gauge has something to report when cleanup runs.
    let s1 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 1 activates");

    // Sanity: the production code wired the layer state.
    assert!(
        opcgw::opc_ua_session_monitor::session_monitor_state_active(),
        "MonitorState must be active while server is running"
    );

    // Disconnect the session so server.run() can exit cleanly without
    // wrestling an orphan client.
    s1.disconnect().await;

    // Take the handle out so TestServer::Drop does NOT abort it. This
    // is the whole point of the test: we let OpcUa::run reach its
    // natural cleanup path (cancel + abort gauge + drop state guard).
    let handle = server.handle.take().expect("server handle must be present");

    // Fire cancellation — async-opcua's server.run() reads the same
    // token (wired via OpcUa::create_server's `.token(cancel_token)`)
    // and exits Ok(()). Then OpcUa::run's post-await cleanup runs:
    // cancel_token.cancel() (idempotent), gauge_handle.abort(), await
    // gauge, drop _state_guard → clear_session_monitor_state.
    server.cancel.cancel();

    // Bound the wait so a hang fails the test instead of hanging CI.
    let join_result = tokio::time::timeout(Duration::from_secs(15), handle)
        .await
        .expect("OpcUa::run did not exit within 15s of cancellation");
    assert!(
        join_result.is_ok(),
        "OpcUa::run task ended abnormally (panicked or join failed): {:?}",
        join_result.err()
    );

    // Production cleanup must have cleared the static state (via
    // MonitorStateGuard::Drop). Checking BEFORE TestServer::Drop runs
    // — TestServer::Drop's clear_session_monitor_state() call would
    // mask this regression.
    assert!(
        !opcgw::opc_ua_session_monitor::session_monitor_state_active(),
        "MonitorState must be cleared after OpcUa::run exits — \
         MonitorStateGuard::Drop did not run or was not wired"
    );
}

// -----------------------------------------------------------------------
// AC#3: at-limit accept warn + periodic gauge
// -----------------------------------------------------------------------

/// AC#3: at-limit accepts emit `event="opcua_session_count_at_limit"`
/// with `source_ip`, `limit`, and `current` co-located on a single line.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_at_limit_accept_emits_warn_event() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(1).await;

    // Open 1 session, hold it.
    let s1 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("first session must activate");

    // Allow the session counter to settle — the SessionManager
    // increments inside `create_session`, but `set_current_session_count`
    // runs through `LocalValue::set` and we want it visible before the
    // next accept.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Attempt a 2nd session — must fail to activate. The TCP accept
    // fires `info!("Accept new connection from {addr} (...)")`, which
    // our `AtLimitAcceptLayer` observes and turns into the at-limit
    // warn.
    let s2 = open_session_held(&server, user_password_identity(), 3_000).await;
    assert!(
        s2.is_none(),
        "2nd session must NOT activate while at limit (max_connections=1)"
    );

    // Allow the layer's emitted warn to flush through the global
    // tracing-test buffer.
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Field formatting: `source_ip = %addr` is Display (no surrounding
    // quotes), `event = "..."` and `limit/current` are Debug-formatted
    // (`event="..."`, `limit=1`, `current=1`). Match the literal text
    // tracing-subscriber's default writer produces.
    assert!(
        captured_log_line_contains_all(&[
            "event=\"opcua_session_count_at_limit\"",
            "source_ip=127.0.0.1",
            "limit=1",
            "current=1",
        ]),
        "at-limit accept warn event must co-occur with source_ip / limit=1 / current=1 on a single line"
    );

    s1.disconnect().await;
}

/// AC#3: periodic gauge emits an `event="opcua_session_count"` line on
/// each `OPCUA_SESSION_GAUGE_INTERVAL_SECS` tick with `current=N` and
/// `limit=L` co-located.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_session_count_gauge_emits_periodically() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(5).await;

    // Open 2 sessions. Hold them open for the duration of one full
    // gauge tick.
    let s1 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 1 activates");
    let s2 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 2 activates");

    // Sleep enough for at least one gauge tick. The interval is
    // `OPCUA_SESSION_GAUGE_INTERVAL_SECS` (5s); +1s buffer.
    tokio::time::sleep(Duration::from_secs(OPCUA_SESSION_GAUGE_INTERVAL_SECS + 1)).await;

    assert!(
        captured_log_line_contains_all(&[
            "event=\"opcua_session_count\"",
            "current=2",
            "limit=5",
        ]),
        "gauge line must co-occur with current=2 and limit=5 on a single line"
    );

    s1.disconnect().await;
    s2.disconnect().await;
}

/// AC#3: disconnecting a session frees its slot — the session-count
/// counter decrements visibly within ~5s. Pinned indirectly by AC#2's
/// "4th attempt succeeds" sub-step, but a dedicated test isolates the
/// decrement behaviour from session-creation flakiness.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_session_count_decrements_on_disconnect() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(2).await;

    let s1 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 1 activates");
    let s2 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 2 activates");

    // Wait for gauge: confirms counter saw both sessions.
    tokio::time::sleep(Duration::from_secs(OPCUA_SESSION_GAUGE_INTERVAL_SECS + 1)).await;
    assert!(
        captured_log_line_contains_all(&[
            "event=\"opcua_session_count\"",
            "current=2",
            "limit=2",
        ]),
        "before disconnect: gauge must report current=2 limit=2"
    );

    // Disconnect session 1. Wait for the next gauge tick.
    s1.disconnect().await;
    tokio::time::sleep(Duration::from_secs(OPCUA_SESSION_GAUGE_INTERVAL_SECS + 1)).await;

    assert!(
        captured_log_line_contains_all(&[
            "event=\"opcua_session_count\"",
            "current=1",
            "limit=2",
        ]),
        "after disconnect: gauge must report current=1 limit=2 within one gauge tick"
    );

    s2.disconnect().await;
}

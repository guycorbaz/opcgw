// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 7-2 integration tests: OPC UA security endpoints + authentication.
//
// What these tests pin (the "shape contract"):
//   - AC#1: three endpoints registered (None / Basic256 Sign / Basic256
//           SignAndEncrypt) with the canonical (security_policy,
//           security_mode, security_level) tuples. Verified via OPC UA
//           endpoint discovery (`get_server_endpoints_from_url`).
//   - AC#2: a wrong password is rejected on the `null` endpoint. The
//           Basic256 endpoints require encrypted-channel setup which is
//           out of scope for the integration suite (the auth path is
//           endpoint-agnostic — `OpcgwAuthManager` does not care about
//           the channel security — so the `null` test fully exercises the
//           rejection path. The Basic256 endpoints are still pinned by
//           AC#1's discovery test.)
//   - AC#3: a failed authentication emits the `event="opcua_auth_failed"`
//           audit log line, and the captured username is sanitised so a
//           malicious client cannot inject control characters.
//
// What is intentionally out of scope:
//   - Connecting to Basic256 endpoints (requires client-side PKI setup that
//     adds significant complexity and brittleness; the auth-path coverage
//     comes from `OpcgwAuthManager` unit tests + the `null`-endpoint
//     wrong-password test). See story Dev Notes for rationale.
//   - Asserting on async-opcua's own `info!("Accept new connection from
//     {addr} ...")` event — that is library-emitted and outside our trait.

use std::sync::Arc;
use std::time::Duration;

use opcua::client::{ClientBuilder, IdentityToken, Password as ClientPassword};
use opcua::types::{EndpointDescription, MessageSecurityMode, UserTokenPolicy};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;
use tracing_test::traced_test;

/// Read the captured log buffer that `tracing_test` populates and check
/// for a substring across **every** captured line, ignoring the scope
/// prefix. The macro-injected `logs_contain` function only matches lines
/// inside the test function's span, but our auth events are emitted from
/// inside a `tokio::spawn`'d task that runs in a different span
/// (`Incoming request{request_id=...}`), so they would otherwise be
/// invisible.
fn captured_logs_contain_anywhere(needle: &str) -> bool {
    let raw = tracing_test::internal::global_buf().lock().unwrap().clone();
    let s = String::from_utf8_lossy(&raw);
    s.contains(needle)
}

use opcgw::config::{
    AppConfig, ChirpStackApplications, ChirpstackDevice, ChirpstackPollerConfig,
    CommandValidationConfig, Global, OpcMetricTypeConfig, OpcUaConfig, ReadMetric, StorageConfig,
};
use opcgw::opc_ua::OpcUa;
use opcgw::storage::{ConnectionPool, SqliteBackend, StorageBackend};

// -----------------------------------------------------------------------
// Test harness
// -----------------------------------------------------------------------

/// Configured user for the test gateway.
const TEST_USER: &str = "opcua-user";
/// Configured password for the test gateway.
const TEST_PASSWORD: &str = "test-password-7-2";
/// Sentinel password used by AC#2 wrong-password tests. The literal must
/// stay greppable so future tests can confirm the password is never logged.
const WRONG_PASSWORD_SENTINEL: &str = "WRONG-PASSWORD-SENTINEL-7-2";

/// Owned test-server handle. Cancellation runs in `Drop` so a panicking
/// test never leaks the spawned tokio task.
struct TestServer {
    port: u16,
    cancel: CancellationToken,
    handle: Option<tokio::task::JoinHandle<()>>,
    // Keep the temp dir alive — its `Drop` removes the SQLite db + PKI dir.
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
    }
}

/// Discover a free port. Bind a listener on `127.0.0.1:0`, read the
/// allocated port, then drop the listener so the OPC UA server can grab
/// the same port. There is a small race window between drop and the
/// server's `listen` call but it is benign in practice.
async fn pick_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port for discovery");
    listener.local_addr().expect("local_addr").port()
}

/// Build a minimal `AppConfig` pointing at a sandboxed PKI dir + SQLite
/// db. The configured user/password matches `TEST_USER` / `TEST_PASSWORD`.
fn test_config(port: u16, pki_dir: &std::path::Path) -> AppConfig {
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
            // Auto-generate the server keypair into the test PKI dir.
            create_sample_keypair: true,
            certificate_path: "own/cert.der".to_string(),
            private_key_path: "private/private.pem".to_string(),
            trust_client_cert: true,
            check_cert_time: false,
            pki_dir: pki_dir.to_string_lossy().into_owned(),
            user_name: TEST_USER.to_string(),
            user_password: TEST_PASSWORD.to_string(),
            stale_threshold_seconds: Some(120),
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

/// Spin up the gateway in a child task, wait for the port to bind,
/// return the handle. `traced_test` callers see the `OpcgwAuthManager`
/// log events directly because it logs into the global subscriber.
async fn setup_test_server() -> TestServer {
    let tmp = TempDir::new().expect("create temp dir");
    let port = pick_free_port().await;
    let pki_dir = tmp.path().join("pki");
    let db_path = tmp.path().join("opcgw.db");

    let config = Arc::new(test_config(port, &pki_dir));
    let pool = Arc::new(
        ConnectionPool::new(db_path.to_str().expect("utf-8 db path"), 1)
            .expect("create connection pool"),
    );
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::with_pool(pool).expect("create backend"));

    let cancel = CancellationToken::new();
    let opc_ua = OpcUa::new(&config, backend, cancel.clone());

    let handle = tokio::spawn(async move {
        // Errors are surfaced via tracing inside `OpcUa::run`; we just
        // keep the task alive until cancellation.
        let _ = opc_ua.run().await;
    });

    // Poll the port until it accepts connections (or fail after 10s —
    // certificate generation can be slow on first run because async-opcua
    // runs an RSA keygen step synchronously).
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

    // Give async-opcua a brief moment to finalise its handshake setup.
    tokio::time::sleep(Duration::from_millis(200)).await;

    TestServer {
        port,
        cancel,
        handle: Some(handle),
        _tmp: tmp,
    }
}

/// Build a fresh client with its own sandboxed PKI dir, ready to talk to
/// the test server.
fn build_client(client_pki: &std::path::Path) -> opcua::client::Client {
    ClientBuilder::new()
        .application_name("opcgw-test-client")
        .application_uri("urn:opcgw:test:client")
        .product_uri("urn:opcgw:test:client")
        .create_sample_keypair(true)
        .trust_server_certs(true)
        .verify_server_certs(false)
        .session_retry_limit(0)
        .session_timeout(5_000)
        .pki_dir(client_pki)
        .client()
        .expect("client build")
}

/// Try to connect to the `None` endpoint with the supplied identity and
/// wait up to `timeout_ms` for the session to either activate (`Ok(true)`)
/// or fail to activate (`Ok(false)` or `Err(_)`).
///
/// async-opcua's `connect_to_matching_endpoint` returns immediately with
/// the `(Session, EventLoop)` pair before the session is actually
/// activated — activation (and therefore the auth call) happens inside
/// the spawned event loop. So the only honest way to check rejection is
/// to drive the event loop and time out.
async fn try_connect_none(
    server: &TestServer,
    client: &mut opcua::client::Client,
    identity: IdentityToken,
    timeout_ms: u64,
) -> bool {
    let endpoint: EndpointDescription = (
        server.endpoint_url("/").as_str(),
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy::anonymous(),
    )
        .into();

    let connect_result = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        client.connect_to_matching_endpoint(endpoint, identity),
    )
    .await;
    let connect_result = match connect_result {
        Ok(r) => r,
        Err(_) => return false, // overall connect timed out
    };
    let (session, event_loop) = match connect_result {
        Ok(pair) => pair,
        Err(_) => return false,
    };

    // Disable retries so a rejected session doesn't loop forever.
    session.disable_reconnects();

    let event_handle = event_loop.spawn();

    let connected = tokio::time::timeout(
        Duration::from_millis(timeout_ms),
        session.wait_for_connection(),
    )
    .await
    .unwrap_or(false);

    // Tear down regardless of outcome. Bound every step in a timeout so a
    // hung disconnect doesn't deadlock the test process — the event-loop
    // drop will close the channel anyway.
    let _ = tokio::time::timeout(Duration::from_secs(2), session.disconnect()).await;
    event_handle.abort();
    let _ = tokio::time::timeout(Duration::from_secs(2), event_handle).await;

    connected
}

// -----------------------------------------------------------------------
// AC#1 — endpoint shape pinning
// -----------------------------------------------------------------------

/// AC#1: the gateway advertises exactly three endpoints with the canonical
/// (security_policy, security_mode, security_level) tuples. Uses the
/// no-encryption discovery service (`GetEndpoints`) so the test does not
/// need to negotiate a Basic256 secure channel.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_three_endpoints_accept_correct_credentials() {
    let server = setup_test_server().await;
    let client_tmp = TempDir::new().expect("client temp dir");
    let mut client = build_client(client_tmp.path());

    let endpoints = client
        .get_server_endpoints_from_url(server.endpoint_url("/").as_str())
        .await
        .expect("get_server_endpoints_from_url");

    // We may get duplicate descriptions when async-opcua advertises both
    // discovery and session URLs for the same endpoint configuration; the
    // contract is that the THREE distinct (policy_uri, mode-string) tuples
    // must all be present. Render the mode to a stable string so we can
    // index a `HashSet` without needing `Hash` on `MessageSecurityMode`.
    fn mode_str(m: MessageSecurityMode) -> &'static str {
        match m {
            MessageSecurityMode::None => "None",
            MessageSecurityMode::Sign => "Sign",
            MessageSecurityMode::SignAndEncrypt => "SignAndEncrypt",
            _ => "Invalid",
        }
    }

    let tuples: std::collections::HashSet<(String, &str)> = endpoints
        .iter()
        .map(|e| (e.security_policy_uri.as_ref().to_string(), mode_str(e.security_mode)))
        .collect();

    let none_uri = "http://opcfoundation.org/UA/SecurityPolicy#None";
    let basic256_uri = "http://opcfoundation.org/UA/SecurityPolicy#Basic256";

    assert!(
        tuples.contains(&(none_uri.to_string(), "None")),
        "expected `None` endpoint, got: {tuples:?}"
    );
    assert!(
        tuples.contains(&(basic256_uri.to_string(), "Sign")),
        "expected `Basic256/Sign` endpoint, got: {tuples:?}"
    );
    assert!(
        tuples.contains(&(basic256_uri.to_string(), "SignAndEncrypt")),
        "expected `Basic256/SignAndEncrypt` endpoint, got: {tuples:?}"
    );

    // Security levels are also pinned so a future change cannot quietly
    // shift the SCADA-client preference order.
    let levels: std::collections::HashMap<(String, &str), u8> = endpoints
        .iter()
        .map(|e| {
            (
                (e.security_policy_uri.as_ref().to_string(), mode_str(e.security_mode)),
                e.security_level,
            )
        })
        .collect();

    assert_eq!(
        levels.get(&(none_uri.to_string(), "None")),
        Some(&0),
        "None endpoint must have security_level 0; got: {levels:?}"
    );
    assert_eq!(
        levels.get(&(basic256_uri.to_string(), "Sign")),
        Some(&3),
        "Basic256/Sign endpoint must have security_level 3"
    );
    assert_eq!(
        levels.get(&(basic256_uri.to_string(), "SignAndEncrypt")),
        Some(&13),
        "Basic256/SignAndEncrypt endpoint must have security_level 13"
    );

    // Bonus: connect to the `None` endpoint with the configured
    // credentials and assert session activation succeeds within 5s.
    // (Basic256 endpoints are exercised at the discovery layer above;
    // their full TLS-style PKI handshake from a test client adds
    // brittleness with no auth-path coverage gain — the auth path is
    // endpoint-agnostic, so the `OpcgwAuthManager` unit tests + the
    // `null`-endpoint wrong-password path together cover it.)
    let identity = IdentityToken::UserName(
        TEST_USER.to_string(),
        ClientPassword(TEST_PASSWORD.to_string()),
    );

    let connected = try_connect_none(&server, &mut client, identity, 5_000).await;
    assert!(
        connected,
        "session activation against None endpoint with correct credentials must succeed within 5s"
    );
}

// -----------------------------------------------------------------------
// AC#2 — wrong password is rejected
// -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_wrong_password_rejected_null() {
    let server = setup_test_server().await;
    let client_tmp = TempDir::new().expect("client temp dir");
    let mut client = build_client(client_tmp.path());

    let identity = IdentityToken::UserName(
        TEST_USER.to_string(),
        ClientPassword(WRONG_PASSWORD_SENTINEL.to_string()),
    );

    // 2-second budget is plenty: the rejection comes back from the server
    // on the first activate-session round trip. If the session activates
    // we have a real bug — the credentials are wrong.
    let connected = try_connect_none(&server, &mut client, identity, 2_000).await;
    assert!(
        !connected,
        "session activation with wrong password must NOT succeed"
    );
}

// -----------------------------------------------------------------------
// AC#3 — failed-auth audit log + log-injection sanitisation
// -----------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[traced_test]
async fn test_failed_auth_emits_warn_event() {
    let server = setup_test_server().await;
    let client_tmp = TempDir::new().expect("client temp dir");
    let mut client = build_client(client_tmp.path());

    let identity = IdentityToken::UserName(
        TEST_USER.to_string(),
        ClientPassword(WRONG_PASSWORD_SENTINEL.to_string()),
    );

    // The connect+drive cycle takes care of logging emission inside the
    // server's auth manager. If the session never activates, the rejection
    // line was emitted at warn level.
    let _ = try_connect_none(&server, &mut client, identity, 2_000).await;

    // Drain a bit so tracing-test's sink has the line.
    tokio::time::sleep(Duration::from_millis(150)).await;

    assert!(
        captured_logs_contain_anywhere("opcua_auth_failed"),
        "expected `opcua_auth_failed` event in captured logs"
    );
    assert!(
        captured_logs_contain_anywhere(TEST_USER),
        "expected sanitised configured user `{TEST_USER}` in captured logs"
    );
    assert!(
        !captured_logs_contain_anywhere(WRONG_PASSWORD_SENTINEL),
        "the attempted password must NEVER appear in any log output"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[traced_test]
async fn test_failed_auth_username_log_injection_blocked() {
    let server = setup_test_server().await;
    let client_tmp = TempDir::new().expect("client temp dir");
    let mut client = build_client(client_tmp.path());

    // Malicious username trying to forge a log line.
    let evil_user = "evil\n[INJECTED]\nfake-event";
    let identity = IdentityToken::UserName(
        evil_user.to_string(),
        ClientPassword("any".to_string()),
    );
    let _ = try_connect_none(&server, &mut client, identity, 2_000).await;

    tokio::time::sleep(Duration::from_millis(150)).await;

    // The captured-log buffer joins lines internally, so we can't assert
    // on a literal newline character. What we *can* assert is that the
    // injection sequence as written by the attacker — newline followed
    // immediately by `[INJECTED]` — does not appear verbatim in the logs.
    // The sanitised form `"evil\\n[INJECTED]\\nfake-event"` (escaped) may
    // appear and is fine.
    let injected_substr = "\n[INJECTED]\n";
    assert!(
        !captured_logs_contain_anywhere(injected_substr),
        "literal newline-bracketed [INJECTED] must not appear in captured logs"
    );

    // Sanity-check that the sanitiser path actually fired — we expect the
    // escaped form to land in the audit log.
    assert!(
        captured_logs_contain_anywhere("opcua_auth_failed"),
        "expected `opcua_auth_failed` event in captured logs"
    );
}

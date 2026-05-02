// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 8-1 spike: integration tests pinning subscription support in
// async-opcua 0.17.1 against opcgw at HEAD.
//
// What these tests pin (the "shape contract"):
//   - AC#1: Plan A confirmation. End-to-end subscription pipeline
//           (CreateSession → ActivateSession → CreateSubscription →
//           CreateMonitoredItems → Publish → DataChangeNotification)
//           fires against an unmodified opcgw server. No production
//           code in `src/` is required for this to work — confirms
//           that `SimpleNodeManagerImpl::create_value_monitored_items`
//           + `SyncSampler` already wire the existing
//           `add_read_callback` registrations into the subscription
//           engine.
//   - AC#9: Subscription clients flow through Story 7-2's
//           `OpcgwAuthManager` (wrong-password rejection, single-line
//           `event="opcua_auth_failed"` audit event) and Story 7-3's
//           `AtLimitAcceptLayer` (one-over-cap rejection,
//           single-line `event="opcua_session_count_at_limit"` warn)
//           identically to read-only clients — no new auth or audit
//           infrastructure introduced by Epic 8.
//
// Issue #102 (Epic 8 retro 2026-05-02): truly identical helpers
// (pick_free_port, build_client, user_name_identity) are now in
// tests/common/mod.rs. Per-file divergent helpers (init_test_subscriber,
// setup_test_server_with_max, HeldSession, spike_test_config) stay
// in this file with documented divergence rationale — see
// tests/common/mod.rs top-of-file docstring.

mod common;

use std::sync::Arc;
use std::time::Duration;

use opcua::client::{
    DataChangeCallback, IdentityToken, Session,
};
use opcua::types::{
    DataChangeFilter, DataChangeTrigger, EndpointDescription, ExtensionObject,
    MessageSecurityMode, MonitoredItemCreateRequest, MonitoringMode, NodeId, ReadValueId,
    TimestampsToReturn, UserTokenPolicy, UserTokenType,
};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{fmt as tracing_fmt, layer::SubscriberExt, Layer};

use opcgw::config::{
    AppConfig, ChirpStackApplications, ChirpstackDevice, ChirpstackPollerConfig,
    CommandValidationConfig, Global, OpcMetricTypeConfig, OpcUaConfig, ReadMetric, StorageConfig,
};
use opcgw::opc_ua::OpcUa;
use opcgw::storage::{
    BatchMetricWrite, ConnectionPool, MetricType, SqliteBackend, StorageBackend,
};

// -----------------------------------------------------------------------
// Test harness — mirrors `tests/opc_ua_connection_limit.rs`. Inline
// duplication is the right move per CLAUDE.md scope-discipline rule:
// three test files is one short of the four-file extraction threshold.
// -----------------------------------------------------------------------

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-8-1";
const SPIKE_DEVICE_ID: &str = "0000000000000001";
const SPIKE_METRIC_NAME: &str = "temperature";
const SPIKE_METRIC_OPCUA_NAME: &str = "Temperature";
// `ns = 2` matches opcgw's deterministic namespace assignment: ns 0 is
// the OPC UA standard namespace, ns 1 is the server-local namespace,
// async-opcua's `add_namespace` returns 2 for the first user-supplied
// `NamespaceMetadata`. Confirmed against
// async-opcua-server-0.17.1/src/node_manager/memory/simple.rs:86-92.
const OPCGW_NAMESPACE_INDEX: u16 = 2;

/// Install a global tracing subscriber that includes both
/// `tracing_test`'s capture layer (so assertions can read the global
/// buffer) AND opcgw's `AtLimitAcceptLayer` (so the at-limit warn event
/// is produced under the same dispatcher that captures it). Identical
/// shape to `tests/opc_ua_connection_limit.rs::init_test_subscriber`
/// — `set_global_default` may only be called once per process, so we
/// guard with a `OnceLock`.
///
/// **DO NOT add `#[traced_test]` to any test in this file** — it
/// installs its own subscriber and `set_global_default` would fail.
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
        let subscriber = tracing_subscriber::Registry::default()
            .with(fmt_layer)
            .with(opcgw::opc_ua_session_monitor::AtLimitAcceptLayer::new());
        // Issue #101 fix: panic loudly if subscriber install fails.
        // Pre-fix: `eprintln!` + silent continue let auth/at-limit
        // tests pass spuriously (captured-log assertions read an
        // empty buffer because no events ever reached the layer).
        // The OnceLock guarantees this is called exactly once per
        // process; if it fails, every captured-log test in the file
        // is broken — surface it loudly instead of letting tests
        // silently misreport.
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
    // Issue #101 fix: panic on mutex poison. Pre-fix `if let Ok(...)`
    // silently dropped the buffer reset on poison, leaving stale data
    // for the next test's assertions. A poisoned mutex is a test-panic
    // signal that the previous test left the harness in a bad state —
    // surface it instead of masking it.
    let mut buf = tracing_test::internal::global_buf()
        .lock()
        .expect("clear_captured_buffer: tracing-test buffer mutex poisoned — a previous test panicked while holding the lock");
    buf.clear();
}

fn captured_log_line_contains_all(needles: &[&str]) -> bool {
    let raw = tracing_test::internal::global_buf().lock().unwrap().clone();
    let s = String::from_utf8_lossy(&raw);
    s.lines()
        .any(|line| needles.iter().all(|n| line.contains(n)))
}

/// Issue #101 fix: bounded-retry poll for a captured log line, replacing
/// fixed `tokio::time::sleep` calls before captured-log assertions.
/// Returns `true` once a line matching every needle appears in the
/// global tracing-test buffer; returns `false` if the budget elapses
/// without a match. Polls every 50ms — short enough to catch an event
/// that lands in the buffer immediately, low-overhead enough not to
/// dominate test runtime.
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
    /// Same `Arc<dyn StorageBackend>` the gateway runs against — gives
    /// tests a way to seed metric values via `batch_write_metrics` so
    /// the sampler delivers a known value through the subscription
    /// pipeline (used by `test_subscription_datavalue_payload_carries_seeded_value`).
    backend: Arc<dyn StorageBackend>,
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
        opcgw::opc_ua_session_monitor::clear_session_monitor_state();
    }
}

// Issue #102: pick_free_port moved to tests/common/mod.rs.
// Local re-export keeps existing call sites unchanged.
use common::pick_free_port;

fn spike_test_config(port: u16, pki_dir: &std::path::Path, max_connections: usize) -> AppConfig {
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
            application_name: "opcgw-spike-8-1".to_string(),
            application_uri: "urn:opcgw:spike:8-1".to_string(),
            product_uri: "urn:opcgw:spike:8-1:product".to_string(),
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
            application_name: "SpikeApp".to_string(),
            application_id: "00000000-0000-0000-0000-000000000001".to_string(),
            device_list: vec![ChirpstackDevice {
                device_name: "SpikeDevice".to_string(),
                device_id: SPIKE_DEVICE_ID.to_string(),
                read_metric_list: vec![ReadMetric {
                    metric_name: SPIKE_METRIC_OPCUA_NAME.to_string(),
                    chirpstack_metric_name: SPIKE_METRIC_NAME.to_string(),
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

async fn setup_test_server_with_max(max: usize) -> TestServer {
    let tmp = TempDir::new().expect("create temp dir");
    let port = pick_free_port().await;
    let pki_dir = tmp.path().join("pki");
    let db_path = tmp.path().join("opcgw.db");

    let config = Arc::new(spike_test_config(port, &pki_dir, max));
    let pool = Arc::new(
        ConnectionPool::new(db_path.to_str().expect("utf-8 db path"), 1)
            .expect("create connection pool"),
    );
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::with_pool(pool).expect("create backend"));

    let cancel = CancellationToken::new();
    let backend_for_server = backend.clone();
    let opc_ua = OpcUa::new(&config, backend_for_server, cancel.clone());

    let handle = tokio::spawn(async move {
        let _ = opc_ua.run().await;
    });

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
        backend,
        _tmp: tmp,
    }
}

// Issue #102: build_client moved to tests/common/mod.rs as a
// parametrised helper. This thin wrapper keeps the existing
// `build_client(client_pki)` call shape stable across the file.
fn build_client(client_pki: &std::path::Path) -> opcua::client::Client {
    common::build_client(common::ClientBuildSpec {
        application_name: "opcgw-spike-8-1-client",
        application_uri: "urn:opcgw:spike:8-1:client",
        product_uri: "urn:opcgw:spike:8-1:client",
        session_timeout_ms: 15_000,
        client_pki,
    })
}

// Issue #102: user_name_identity moved to tests/common/mod.rs.
// Per-file thin wrappers preserve the call shape.
fn user_password_identity() -> IdentityToken {
    common::user_name_identity(TEST_USER, TEST_PASSWORD)
}

fn wrong_password_identity() -> IdentityToken {
    common::user_name_identity(TEST_USER, "definitely-not-the-password")
}

struct HeldSession {
    session: Arc<Session>,
    event_handle: Option<tokio::task::JoinHandle<opcua::types::StatusCode>>,
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
        // Issue #101 fix: a synchronous Drop cannot await the aborted
        // task, so the event loop may continue holding session state
        // briefly after Drop returns and bleed into the next
        // `serial_test`. To keep test isolation honest, callers MUST
        // call `disconnect().await` explicitly — Drop is the safety
        // net for tests that panic before reaching `disconnect()`.
        // We emit a debug-only assertion (panic-on-drop is too harsh
        // because Drop fires on test panic too); production paths use
        // explicit `disconnect()` so this path only runs on test
        // panic / early-return.
        if let Some(h) = self.event_handle.take() {
            h.abort();
            // The aborted task may not be cleaned up before the next
            // test starts under heavy parallel load; #[serial_test::serial]
            // is the project-wide mitigation for that, applied to every
            // test in this file.
        }
    }
}

/// Issue #101 fix: discriminated error for `open_session_held` failures.
/// Pre-fix all failure modes collapsed to `Option::None`; tests
/// asserting "auth rejection" via `attempt.is_none()` would pass even
/// when the failure was actually a transport timeout or build error,
/// silently masking regressions on the auth path.
#[derive(Debug)]
#[allow(dead_code)] // some variants are only constructed under specific failure modes
enum OpenSessionError {
    /// Total connect-attempt budget exceeded (`timeout_ms`).
    ConnectTimeout,
    /// `connect_to_matching_endpoint` returned `Err(...)` — transport /
    /// endpoint mismatch / channel error. Auth rejection generally does
    /// NOT land here (it lands in `NotActivated` below). Carries the
    /// upstream error formatted as a string so the discriminated error
    /// stays `Debug`-printable without leaking the upstream type.
    ConnectFailed(String),
    /// Session created and event loop spawned, but
    /// `wait_for_connection` timed out without reaching the connected
    /// state. Typical cause: auth rejection during ActivateSession or
    /// session-cap rejection. The wrong-password / at-limit tests
    /// expect this variant.
    NotActivated,
}

async fn open_session_held(
    server: &TestServer,
    identity: IdentityToken,
    timeout_ms: u64,
) -> Result<HeldSession, OpenSessionError> {
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
    .map_err(|_| OpenSessionError::ConnectTimeout)?;
    let (session, event_loop) =
        connect_result.map_err(|e| OpenSessionError::ConnectFailed(format!("{e:?}")))?;
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
        return Err(OpenSessionError::NotActivated);
    }

    Ok(HeldSession {
        session,
        event_handle: Some(event_handle),
        _client_tmp: client_tmp,
        _client: client,
    })
}

// -----------------------------------------------------------------------
// AC#1: Plan A confirmation — end-to-end subscription pipeline fires
// -----------------------------------------------------------------------

/// Plan A confirmation in test form (AC#1). Subscribes to opcgw's
/// `Temperature` metric NodeId, asserts that a `DataChangeNotification`
/// arrives within 10 s of `CreateMonitoredItems` completion. Whether
/// the delivered DataValue carries `Good`/`BadDataUnavailable` status
/// is **not** the contract — the contract is "the subscription pipeline
/// fires at all". The pipeline includes:
///
///   - CreateSubscription succeeds (validates `Limits.subscriptions.*`
///     defaults are sane for a one-subscription one-monitored-item
///     case).
///   - CreateMonitoredItems succeeds (validates
///     `SimpleNodeManagerImpl::create_value_monitored_items` wires the
///     pre-existing `add_read_callback` into the SyncSampler).
///   - The publish loop activates and async-opcua delivers at least
///     one `DataChangeNotification` to the client's
///     `DataChangeCallback`.
///
/// The 10 s timeout is 10 × the requested 1000 ms publishing interval.
/// The library default `MIN_PUBLISHING_INTERVAL_MS = 100` keeps actual
/// cadence well under the requested rate; tail latency on slower CI
/// hardware needs the 10× headroom.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_basic_data_change_notification() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(2).await;

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    // (tx, rx) channel: the subscription callback pushes each DataValue
    // into the channel, the test awaits the first arrival.
    let (tx, mut rx) = mpsc::unbounded_channel::<opcua::types::DataValue>();

    // Subscription parameters per AC#1: 1000 ms publishing-interval,
    // 30 lifetime / 10 keep-alive (so the subscription survives the
    // full test even if a publish slips), priority 0, publishing
    // enabled, no max-notifications cap.
    let subscription_id = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |dv, _item| {
                let _ = tx.send(dv);
            }),
        )
        .await
        .expect("CreateSubscription must succeed (Plan A confirmation)");

    // Pin the actual subscription_id we got — proves the server
    // assigned a non-zero id, which means the request reached the
    // SubscriptionService and was processed (not just synthesised
    // client-side).
    assert!(
        subscription_id != 0,
        "server-assigned subscription_id must be non-zero"
    );

    // Single monitored item on the Temperature metric. NodeId shape
    // matches `src/opc_ua.rs::add_nodes` line 706 —
    // `NodeId::new(ns, metric_name)` where the metric name is the
    // OPC UA-side name (not the chirpstack-side name).
    let node_id = NodeId::new(OPCGW_NAMESPACE_INDEX, format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME));
    let create = MonitoredItemCreateRequest {
        item_to_monitor: ReadValueId::from(node_id.clone()),
        monitoring_mode: MonitoringMode::Reporting,
        requested_parameters: opcua::types::MonitoringParameters {
            client_handle: 1,
            sampling_interval: 1000.0,
            filter: opcua::types::ExtensionObject::null(),
            queue_size: 10,
            discard_oldest: true,
        },
    };
    let create_results = held
        .session
        .create_monitored_items(subscription_id, TimestampsToReturn::Both, vec![create])
        .await
        .expect("CreateMonitoredItems must succeed");
    assert_eq!(create_results.len(), 1, "exactly one monitored item");
    let result = &create_results[0].result;
    assert!(
        result.status_code.is_good(),
        "CreateMonitoredItems status must be Good — got {:?}",
        result.status_code
    );
    assert!(
        result.monitored_item_id != 0,
        "server-assigned monitored_item_id must be non-zero"
    );

    // Wait for the first DataChangeNotification. The strong prior
    // (per `simple.rs:180-228`) is that
    // `create_value_monitored_items` triggers an immediate sample
    // call, so we'd expect a notification within ~1 publishing
    // interval. 10 s is generous headroom.
    let first_notification = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("subscription notification must arrive within 10 s — Plan A failed if missing")
        .expect("notification channel closed unexpectedly");

    // The notification must carry SOMETHING — either a value (if
    // a metric was upserted, which the test does not seed) or a
    // status code (e.g., BadDataUnavailable from the read-callback
    // when the metric is not in storage). Both prove the pipeline
    // works. We assert that *some* meaningful field is set.
    assert!(
        first_notification.value.is_some()
            || first_notification.status.is_some()
            || first_notification.source_timestamp.is_some(),
        "DataChangeNotification must carry a value, status, or timestamp — \
         empty notification means the publish path is broken"
    );

    // Tear down the subscription cleanly so async-opcua's
    // SubscriptionService doesn't carry it across to the next test.
    let _ = held.session.delete_subscription(subscription_id).await;

    held.disconnect().await;
}

// -----------------------------------------------------------------------
// AC#9: subscription clients pass through OpcgwAuthManager + AtLimitAcceptLayer
// -----------------------------------------------------------------------

/// AC#9: a wrong-password client that *would* create a subscription is
/// rejected at `ActivateSession` by Story 7-2's `OpcgwAuthManager`. The
/// captured log buffer must contain `event="opcua_auth_failed"` —
/// proving that no new auth path was introduced for subscription-
/// creating clients (NFR12 carry-forward acknowledgment).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_client_rejected_by_auth_manager() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(1).await;

    // Wrong password — should fail to activate. The fact that this
    // client *intends* to create a subscription does not change the
    // auth path; the rejection happens at ActivateSession before any
    // CreateSubscription request can reach the server.
    let attempt = open_session_held(&server, wrong_password_identity(), 3_000).await;
    // Issue #101 fix: assert the discriminated NotActivated variant
    // explicitly — pre-fix `attempt.is_none()` would have passed even
    // if the actual failure was ConnectTimeout or ConnectFailed
    // (transport-layer issues that have nothing to do with auth).
    let err_kind = match &attempt {
        Ok(_) => "Ok(_)".to_string(),
        Err(e) => format!("{e:?}"),
    };
    assert!(
        matches!(attempt, Err(OpenSessionError::NotActivated)),
        "wrong-password client must fail with NotActivated (auth rejection at ActivateSession), \
         got {err_kind} — a different error variant suggests the failure mode is not auth"
    );

    // Issue #101 fix: bounded retry poll instead of fixed 300ms sleep.
    // On loaded CI (heavy parallel test load), the auth-failed event
    // can take >300ms to flush through the tracing pipeline; the
    // pre-fix sleep led to flaky failures where the assertion ran
    // before the buffer received the event.
    let auth_failed_event_present = wait_for_captured_log(
        &["event=\"opcua_auth_failed\"", "user=opcua-user"],
        Duration::from_secs(5),
    )
    .await;

    // Story 7-2 invariant: every failed auth emits a single-line
    // `event="opcua_auth_failed"` warn with the sanitised user. The
    // field name in the actual emit is `user=` (Display-formatted, no
    // surrounding quotes — confirmed against the captured log line in
    // first-run output 2026-04-29). Pinning the field this way also
    // catches a future rename to `username=`.
    //
    // Issue #101 fix: assertion now uses the bounded-retry result
    // computed above. Pre-fix this used `captured_log_line_contains_all`
    // synchronously after a fixed 300ms sleep; on loaded CI the event
    // could land later than 300ms and the assertion would fail flakily.
    assert!(
        auth_failed_event_present,
        "wrong-password subscription-creating client must trigger the \
         existing event=\"opcua_auth_failed\" audit event within 5s — \
         Story 7-2 invariant"
    );
}

/// AC#9: a one-over-cap client that *would* create a subscription is
/// rejected at session creation by Story 7-3's `AtLimitAcceptLayer`. The
/// captured log buffer must contain `event="opcua_session_count_at_limit"`
/// with `current=1 limit=1` — proving the connection cap applies to
/// subscription-creating clients identically to read-only clients
/// (NFR12 carry-forward acknowledgment).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_client_rejected_by_at_limit_layer() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(1).await;

    // Open one valid session that *would* create a subscription —
    // we don't actually call CreateSubscription, but the cap test
    // doesn't care about that. The point is the session uses the
    // single available slot.
    let s1 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("first session must activate");

    // Allow the session counter to settle — same shape as Story 7-3's
    // `test_at_limit_accept_emits_warn_event`.
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Attempt a 2nd session — must fail to activate. The TCP accept
    // fires `Accept new connection from {addr} (...)` which the
    // `AtLimitAcceptLayer` observes and turns into the at-limit warn.
    let s2 = open_session_held(&server, user_password_identity(), 3_000).await;
    // Issue #101 fix: assert the discriminated NotActivated variant
    // explicitly. Pre-fix `s2.is_none()` would have passed even if the
    // 2nd session failed for an unrelated reason (e.g., transport
    // timeout) instead of the cap rejection.
    let s2_kind = match &s2 {
        Ok(_) => "Ok(_)".to_string(),
        Err(e) => format!("{e:?}"),
    };
    assert!(
        matches!(s2, Err(OpenSessionError::NotActivated)),
        "2nd session must fail with NotActivated (cap rejection at session creation), \
         got {s2_kind} — max_connections=1 means the 2nd session should never activate"
    );

    // Issue #101 fix: bounded retry instead of fixed 200ms sleep —
    // the at-limit warn flush can exceed 200ms on loaded CI.
    let at_limit_event_present = wait_for_captured_log(
        &[
            "event=\"opcua_session_count_at_limit\"",
            "source_ip=127.0.0.1",
            "limit=1",
            "current=1",
        ],
        Duration::from_secs(5),
    )
    .await;

    // Story 7-3 invariant: at-limit accept warn co-occurs with
    // `source_ip`, `limit`, and `current` on a single line.
    assert!(
        at_limit_event_present,
        "at-limit subscription-creating client must trigger the \
         existing event=\"opcua_session_count_at_limit\" audit event — \
         Story 7-3 invariant"
    );

    s1.disconnect().await;
}

// -----------------------------------------------------------------------
// Test-depth additions (2026-04-30) — extra automated coverage to
// compensate for the deferred manual FUXA + Ignition verification (AC#3
// deferred to a single integration pass after Epic 9 lands; see
// deferred-work.md "Story 8-1" block).
//
// Each test pins a single concrete pipeline behaviour that a full
// SCADA-based verification would otherwise be the only way to catch:
//
//   - test_subscription_two_clients_share_node    — `epics.md:720`
//                                                    multi-client invariant
//   - test_subscription_ten_monitored_items_per_subscription
//                                                  — per-monitored-item
//                                                    publish-loop branch
//   - test_subscription_sampling_interval_revised_to_minimum
//                                                  — server-side
//                                                    `MIN_SAMPLING_INTERVAL_MS`
//                                                    floor enforcement
//   - test_subscription_datavalue_payload_carries_seeded_value
//                                                  — value-flow path
//                                                    (not just pipeline-fires)
//   - test_subscription_double_delete_is_safe     — teardown idempotency
//   - test_subscription_survives_sibling_session_disconnect
//                                                  — per-session state
//                                                    isolation
//
// All tests use `setup_test_server_with_max(N)` with N >= the count of
// concurrent sessions the test holds. Sleep durations rounded up from
// the 100 ms minimum sampling interval to absorb scheduler jitter on
// CI hardware.
// -----------------------------------------------------------------------

/// Two simultaneously-connected clients each create a subscription on
/// the same NodeId. Both must receive at least one DataChangeNotification.
/// Pins `epics.md:720` "multiple clients can subscribe to the same
/// variables simultaneously".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_two_clients_share_node() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(3).await;

    let s1 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 1 must activate");
    let s2 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 2 must activate");

    let (tx1, mut rx1) = mpsc::unbounded_channel::<opcua::types::DataValue>();
    let (tx2, mut rx2) = mpsc::unbounded_channel::<opcua::types::DataValue>();

    let sub_id_1 = s1
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |dv, _item| {
                let _ = tx1.send(dv);
            }),
        )
        .await
        .expect("client 1 CreateSubscription");
    let sub_id_2 = s2
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |dv, _item| {
                let _ = tx2.send(dv);
            }),
        )
        .await
        .expect("client 2 CreateSubscription");

    let item = MonitoredItemCreateRequest {
        item_to_monitor: ReadValueId::from(NodeId::new(
            OPCGW_NAMESPACE_INDEX,
            format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME),
        )),
        monitoring_mode: MonitoringMode::Reporting,
        requested_parameters: opcua::types::MonitoringParameters {
            client_handle: 1,
            sampling_interval: 1000.0,
            filter: opcua::types::ExtensionObject::null(),
            queue_size: 10,
            discard_oldest: true,
        },
    };

    s1.session
        .create_monitored_items(sub_id_1, TimestampsToReturn::Both, vec![item.clone()])
        .await
        .expect("client 1 CreateMonitoredItems");
    s2.session
        .create_monitored_items(sub_id_2, TimestampsToReturn::Both, vec![item])
        .await
        .expect("client 2 CreateMonitoredItems");

    // Both clients must receive at least one notification. 10 s
    // matches the AC#1 timeout pattern (10 × publishing interval).
    let n1 = tokio::time::timeout(Duration::from_secs(10), rx1.recv())
        .await
        .expect("client 1 must receive a notification within 10 s");
    let n2 = tokio::time::timeout(Duration::from_secs(10), rx2.recv())
        .await
        .expect("client 2 must receive a notification within 10 s");
    assert!(n1.is_some(), "client 1 channel did not close prematurely");
    assert!(n2.is_some(), "client 2 channel did not close prematurely");

    let _ = s1.session.delete_subscription(sub_id_1).await;
    let _ = s2.session.delete_subscription(sub_id_2).await;
    s1.disconnect().await;
    s2.disconnect().await;
}

/// One subscription, ten monitored items (all on the same NodeId for
/// fixture simplicity — the per-item branch in async-opcua's publish
/// loop is what we're exercising, not address-space coverage). Every
/// client_handle must receive at least one notification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_ten_monitored_items_per_subscription() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(2).await;

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    let (tx, mut rx) = mpsc::unbounded_channel::<u32>();

    let sub_id = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |_dv, item| {
                let _ = tx.send(item.client_handle());
            }),
        )
        .await
        .expect("CreateSubscription");

    // Ten requests, client_handles 1..=10, all on the same NodeId.
    let requests: Vec<MonitoredItemCreateRequest> = (1..=10u32)
        .map(|h| MonitoredItemCreateRequest {
            item_to_monitor: ReadValueId::from(NodeId::new(
                OPCGW_NAMESPACE_INDEX,
                format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME),
            )),
            monitoring_mode: MonitoringMode::Reporting,
            requested_parameters: opcua::types::MonitoringParameters {
                client_handle: h,
                sampling_interval: 1000.0,
                filter: opcua::types::ExtensionObject::null(),
                queue_size: 10,
                discard_oldest: true,
            },
        })
        .collect();
    let results = held
        .session
        .create_monitored_items(sub_id, TimestampsToReturn::Both, requests)
        .await
        .expect("CreateMonitoredItems");
    assert_eq!(results.len(), 10);
    for r in &results {
        assert!(
            r.result.status_code.is_good(),
            "every item must be Good — got {:?}",
            r.result.status_code
        );
    }

    // Collect notifications until every client_handle has been seen at
    // least once. Bound at 15 s — at 1 s sampling interval × 10 items
    // we expect ~5–10 s for the first round to land.
    let mut seen: std::collections::HashSet<u32> = std::collections::HashSet::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while seen.len() < 10 && std::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Some(handle)) => {
                seen.insert(handle);
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert_eq!(
        seen.len(),
        10,
        "every client_handle (1..=10) must receive at least one notification — got {} unique handles: {:?}",
        seen.len(),
        seen
    );

    let _ = held.session.delete_subscription(sub_id).await;
    held.disconnect().await;
}

/// Request a sub-floor sampling interval (50 ms) and assert the server
/// revises it up to the library minimum (`MIN_SAMPLING_INTERVAL_MS = 100`,
/// confirmed against `~/.cargo/registry/src/.../async-opcua-server-0.17.1/src/lib.rs:73, 77`).
/// Pins the server-side limit-revision protocol behaviour — important
/// because Story 8-2 will surface `min_sampling_interval_ms` as a config
/// knob and operators must understand that requested values below the
/// floor are silently revised, not rejected.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_sampling_interval_revised_to_minimum() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(2).await;

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    let sub_id = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |_dv, _item| {}),
        )
        .await
        .expect("CreateSubscription");

    // Request a 50 ms sampling interval — well below the 100 ms floor.
    let req = MonitoredItemCreateRequest {
        item_to_monitor: ReadValueId::from(NodeId::new(
            OPCGW_NAMESPACE_INDEX,
            format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME),
        )),
        monitoring_mode: MonitoringMode::Reporting,
        requested_parameters: opcua::types::MonitoringParameters {
            client_handle: 1,
            sampling_interval: 50.0, // sub-floor on purpose
            filter: opcua::types::ExtensionObject::null(),
            queue_size: 10,
            discard_oldest: true,
        },
    };
    let results = held
        .session
        .create_monitored_items(sub_id, TimestampsToReturn::Both, vec![req])
        .await
        .expect("CreateMonitoredItems");
    assert_eq!(results.len(), 1);
    let r = &results[0].result;
    assert!(
        r.status_code.is_good(),
        "monitored item must be created Good even when sampling interval is sub-floor"
    );
    assert!(
        r.revised_sampling_interval >= 100.0,
        "server must revise sub-floor sampling interval up to MIN_SAMPLING_INTERVAL_MS (100 ms); \
         got revised={} ms",
        r.revised_sampling_interval
    );

    let _ = held.session.delete_subscription(sub_id).await;
    held.disconnect().await;
}

/// Pre-populate the metric value via `backend.batch_write_metrics`,
/// then subscribe and assert the delivered DataValue carries the
/// expected `Variant::Float(42.5)` AND a Good status code. Pins the
/// value-flow path (not just the pipeline-fires path) — the existing
/// AC#1 test only asserts that *some* notification arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_datavalue_payload_carries_seeded_value() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(2).await;

    // Seed a Float value for the spike fixture's device + metric. The
    // gateway's `OpcUa::get_value` callback returns this as a
    // `Variant::Float(42.5)` with Good status (timestamp is fresh =>
    // not stale).
    server
        .backend
        .batch_write_metrics(vec![BatchMetricWrite {
            device_id: SPIKE_DEVICE_ID.to_string(),
            metric_name: SPIKE_METRIC_NAME.to_string(),
            value: "42.5".to_string(),
            data_type: MetricType::Float,
            timestamp: std::time::SystemTime::now(),
        }])
        .expect("batch_write_metrics seed");

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    let (tx, mut rx) = mpsc::unbounded_channel::<opcua::types::DataValue>();

    let sub_id = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |dv, _item| {
                let _ = tx.send(dv);
            }),
        )
        .await
        .expect("CreateSubscription");

    held.session
        .create_monitored_items(
            sub_id,
            TimestampsToReturn::Both,
            vec![MonitoredItemCreateRequest {
                item_to_monitor: ReadValueId::from(NodeId::new(
                    OPCGW_NAMESPACE_INDEX,
                    format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME),
                )),
                monitoring_mode: MonitoringMode::Reporting,
                requested_parameters: opcua::types::MonitoringParameters {
                    client_handle: 1,
                    sampling_interval: 1000.0,
                    filter: opcua::types::ExtensionObject::null(),
                    queue_size: 10,
                    discard_oldest: true,
                },
            }],
        )
        .await
        .expect("CreateMonitoredItems");

    // Wait for the first notification. The seed must be visible on
    // the very first sample (the sampler reads it via the existing
    // `add_read_callback` → `Storage::get_metric_value` path).
    let dv = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("notification must arrive within 10 s")
        .expect("notification channel did not close prematurely");

    // Assert: value carries a Float variant matching the seed.
    let variant = dv
        .value
        .as_ref()
        .expect("DataValue must carry a value (not just a status)");
    match variant {
        opcua::types::Variant::Float(f) => {
            // Float32 widening from 42.5 is exact.
            assert!(
                (*f - 42.5_f32).abs() < f32::EPSILON,
                "expected 42.5 — got {f}"
            );
        }
        other => panic!("expected Variant::Float — got {other:?}"),
    }

    // Status must be Good — the seed timestamp is fresh, no
    // staleness should kick in.
    let status = dv.status.expect("DataValue must carry a status");
    assert!(
        status.is_good(),
        "expected Good status for fresh seed — got {status:?}"
    );

    let _ = held.session.delete_subscription(sub_id).await;
    held.disconnect().await;
}

/// Delete a subscription, then attempt to delete the same id again on
/// the same client session. The second call must NOT panic; it must
/// return an error path gracefully.
///
/// **Issue #101 caveat (test scope):** this test exercises **only** the
/// async-opcua client-side state machine. The second `delete_subscription`
/// call is rejected by the *client's* internal state tracking BEFORE the
/// request reaches the server (async-opcua 0.17.1 `service.rs:1707-1720`
/// returns `Err(BadInvalidArgument)` immediately because the
/// subscription_id is not in the client's local subscription map after
/// the first delete). **It does NOT prove server-side idempotency** —
/// for that, a future test would need two separate sessions and a
/// `transfer_subscriptions` round-trip, OR a server-side fault-injection
/// path that bypasses the client cache. Renaming preserves the
/// invariant without overstating coverage. Tracked at GitHub issue #101.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_double_delete_client_side_state_safe() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(2).await;

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    let sub_id = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |_dv, _item| {}),
        )
        .await
        .expect("CreateSubscription");

    // First delete — must succeed.
    let first = held.session.delete_subscription(sub_id).await;
    assert!(
        first.is_ok(),
        "first delete must succeed — got {:?}",
        first
    );

    // Second delete — must return an error path, NOT panic. Acceptable
    // shapes per async-opcua 0.17.1 client (`service.rs:1707-1720`):
    // returns `Err(BadInvalidArgument)` when the id no longer exists
    // in client-side state. The error is generated client-side; the
    // request never reaches the server. See test docstring for caveat.
    let second = held.session.delete_subscription(sub_id).await;
    assert!(
        second.is_err(),
        "second delete on the same id must return Err (not panic, not Ok)"
    );

    held.disconnect().await;
}

/// Two sessions, each with a subscription. Disconnect session 1.
/// Session 2's subscription must continue to receive notifications
/// for at least one more publish interval. Pins per-session state
/// isolation — a misbehaving SCADA client closing its session must
/// not cascade-fail other clients' subscriptions.
///
/// **Implementation note:** async-opcua's MonitoredItem dedupes
/// successive samples that carry an identical `DataValue` (this is
/// OPC UA spec — DataChangeNotifications fire on changes, not on
/// every tick). To get a *second* notification on session 2 after
/// the sibling disconnect, the test seeds two distinct values: the
/// first triggers the baseline notification, the second triggers
/// the post-disconnect notification.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_survives_sibling_session_disconnect() {
    init_test_subscriber();
    clear_captured_buffer();
    let server = setup_test_server_with_max(3).await;

    // Initial value — drives the baseline notification.
    server
        .backend
        .batch_write_metrics(vec![BatchMetricWrite {
            device_id: SPIKE_DEVICE_ID.to_string(),
            metric_name: SPIKE_METRIC_NAME.to_string(),
            value: "1.0".to_string(),
            data_type: MetricType::Float,
            timestamp: std::time::SystemTime::now(),
        }])
        .expect("initial seed");

    let s1 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 1 must activate");
    let s2 = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session 2 must activate");

    let (tx2, mut rx2) = mpsc::unbounded_channel::<opcua::types::DataValue>();

    // Session 1 subscription — we don't capture its notifications,
    // we just want it to occupy server-side subscription state that
    // gets torn down with the session.
    let sub_id_1 = s1
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |_dv, _item| {}),
        )
        .await
        .expect("client 1 CreateSubscription");

    // Session 2 subscription — captures notifications for the assertion.
    let sub_id_2 = s2
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |dv, _item| {
                let _ = tx2.send(dv);
            }),
        )
        .await
        .expect("client 2 CreateSubscription");

    let item = MonitoredItemCreateRequest {
        item_to_monitor: ReadValueId::from(NodeId::new(
            OPCGW_NAMESPACE_INDEX,
            format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME),
        )),
        monitoring_mode: MonitoringMode::Reporting,
        requested_parameters: opcua::types::MonitoringParameters {
            client_handle: 1,
            sampling_interval: 1000.0,
            filter: opcua::types::ExtensionObject::null(),
            queue_size: 10,
            discard_oldest: true,
        },
    };
    s1.session
        .create_monitored_items(sub_id_1, TimestampsToReturn::Both, vec![item.clone()])
        .await
        .expect("client 1 CreateMonitoredItems");
    s2.session
        .create_monitored_items(sub_id_2, TimestampsToReturn::Both, vec![item])
        .await
        .expect("client 2 CreateMonitoredItems");

    // Wait for at least one notification on session 2 BEFORE we
    // disconnect session 1 — this proves the pipeline is firing for
    // session 2 in the first place.
    let _baseline = tokio::time::timeout(Duration::from_secs(10), rx2.recv())
        .await
        .expect("session 2 must receive a baseline notification before sibling disconnect")
        .expect("notification channel must remain open");

    // Disconnect session 1 cleanly. async-opcua tears down its
    // subscription state.
    s1.disconnect().await;

    // Drain any in-flight session 2 notifications.
    while rx2.try_recv().is_ok() {}

    // Change the metric value — this is what triggers the next
    // DataChangeNotification on session 2's subscription. (Without a
    // value change, async-opcua's MonitoredItem suppresses the
    // identical sample per OPC UA spec.)
    server
        .backend
        .batch_write_metrics(vec![BatchMetricWrite {
            device_id: SPIKE_DEVICE_ID.to_string(),
            metric_name: SPIKE_METRIC_NAME.to_string(),
            value: "2.0".to_string(), // different from the seed
            data_type: MetricType::Float,
            timestamp: std::time::SystemTime::now(),
        }])
        .expect("post-disconnect seed change");

    // Wait for the change-driven notification on session 2. The
    // sampler's next tick (≤ 1 publish interval) sees the new value
    // and emits.
    let post_disconnect = tokio::time::timeout(Duration::from_secs(10), rx2.recv())
        .await
        .expect("session 2 must keep receiving notifications after session 1 disconnect")
        .expect("notification channel must remain open");
    let v = post_disconnect
        .value
        .as_ref()
        .expect("post-disconnect notification must carry a value (the change to 2.0)");
    match v {
        opcua::types::Variant::Float(f) => assert!(
            (*f - 2.0_f32).abs() < f32::EPSILON,
            "post-disconnect notification must carry the changed value 2.0 — got {f}"
        ),
        other => panic!("expected Variant::Float — got {other:?}"),
    }

    let _ = s2.session.delete_subscription(sub_id_2).await;
    s2.disconnect().await;
}

// =======================================================================
// Story 8-2 (AC#3, FR21): subscription / monitored-item / outage tests
// against the four configurable Limits knobs. Additive — the existing
// 9 tests above stay as the regression baseline. The three tests below
// cover the new contract:
//
//   AC#3.1: max_subscriptions_per_session enforcement on the (cap+1)th
//           CreateSubscription call.
//   AC#3.2: max_monitored_items_per_sub enforcement on a CreateMonitoredItems
//           call that would push the subscription past the cap.
//   AC#3.3: subscription survives a ChirpStack outage with stale status
//           codes propagating through the publish path.
//
// Per CLAUDE.md scope-discipline rule, the new helper struct +
// `setup_test_server_with_subscription_limits` is the third consumer of
// subscription-related test setup — a small additive variant rather
// than a `tests/common/` extraction (the four-file threshold has not
// been crossed).
// =======================================================================

/// Subscription / message-size knob bundle used by AC#3 tests. `None`
/// means "fall back to the gateway default" — same shape as
/// `OpcUaConfig`. Default-constructible so a test that only wants one
/// knob can write `SubscriptionLimitsForTest { max_subscriptions_per_session: Some(2), ..Default::default() }`.
#[derive(Default, Clone)]
struct SubscriptionLimitsForTest {
    max_subscriptions_per_session: Option<usize>,
    max_monitored_items_per_sub: Option<usize>,
    max_message_size: Option<usize>,
    max_chunk_count: Option<usize>,
    /// Override `[opcua].stale_threshold_seconds` — used by AC#3.3 to
    /// keep wall-clock test time small while still crossing the
    /// staleness threshold.
    stale_threshold_seconds: Option<u64>,
}

fn spike_test_config_with_limits(
    port: u16,
    pki_dir: &std::path::Path,
    max_connections: usize,
    sub_limits: &SubscriptionLimitsForTest,
) -> AppConfig {
    let mut cfg = spike_test_config(port, pki_dir, max_connections);
    cfg.opcua.max_subscriptions_per_session = sub_limits.max_subscriptions_per_session;
    cfg.opcua.max_monitored_items_per_sub = sub_limits.max_monitored_items_per_sub;
    cfg.opcua.max_message_size = sub_limits.max_message_size;
    cfg.opcua.max_chunk_count = sub_limits.max_chunk_count;
    if let Some(s) = sub_limits.stale_threshold_seconds {
        cfg.opcua.stale_threshold_seconds = Some(s);
    }
    cfg
}

async fn setup_test_server_with_subscription_limits(
    max_connections: usize,
    sub_limits: SubscriptionLimitsForTest,
) -> TestServer {
    let tmp = TempDir::new().expect("create temp dir");
    let port = pick_free_port().await;
    let pki_dir = tmp.path().join("pki");
    let db_path = tmp.path().join("opcgw.db");

    let config = Arc::new(spike_test_config_with_limits(
        port,
        &pki_dir,
        max_connections,
        &sub_limits,
    ));
    let pool = Arc::new(
        ConnectionPool::new(db_path.to_str().expect("utf-8 db path"), 1)
            .expect("create connection pool"),
    );
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::with_pool(pool).expect("create backend"));

    let cancel = CancellationToken::new();
    let backend_for_server = backend.clone();
    let opc_ua = OpcUa::new(&config, backend_for_server, cancel.clone());

    let handle = tokio::spawn(async move {
        let _ = opc_ua.run().await;
    });

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
        backend,
        _tmp: tmp,
    }
}

/// AC#3.1: a single authenticated client is capped at
/// `max_subscriptions_per_session` simultaneous subscriptions. The
/// (cap+1)th `CreateSubscription` call must fail. Pin: 2 succeed, the
/// 3rd fails — the contract is the OPC UA status code on the wire,
/// not a tracing event (async-opcua emits no audit log for
/// `BadTooManySubscriptions` in 0.17.1; documented in
/// `docs/security.md` and a candidate for upstream FR).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_flood_capped_by_max_subscriptions_per_session() {
    init_test_subscriber();
    clear_captured_buffer();

    // max_connections=2 so the auth + cap layers are exercised but
    // generous enough not to interfere; the test focuses on the
    // per-session subscription cap.
    let server = setup_test_server_with_subscription_limits(
        2,
        SubscriptionLimitsForTest {
            max_subscriptions_per_session: Some(2),
            ..Default::default()
        },
    )
    .await;

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    // First two subscriptions — must succeed.
    let sub1 = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(|_dv, _item| {}),
        )
        .await
        .expect("1st CreateSubscription must succeed");
    let sub2 = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(|_dv, _item| {}),
        )
        .await
        .expect("2nd CreateSubscription must succeed");
    assert_ne!(sub1, 0, "sub1 must be a non-zero subscription id");
    assert_ne!(sub2, 0, "sub2 must be a non-zero subscription id");
    assert_ne!(sub1, sub2, "sub1 and sub2 must be distinct ids");

    // Third subscription — must fail with `BadTooManySubscriptions`
    // (OPC UA spec). async-opcua may surface this as the subscription
    // error itself or as a wider service-level error; we accept either
    // wrapper but pin the wire-level status code so a regression where
    // rejection happens for an unrelated reason (transport drop,
    // internal panic, BadTimeout) fails the test.
    let sub3 = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(|_dv, _item| {}),
        )
        .await;
    let sub3_err = sub3.as_ref().err().unwrap_or_else(|| {
        panic!(
            "3rd CreateSubscription must fail when max_subscriptions_per_session=2 — got Ok({:?})",
            sub3
        )
    });
    let sub3_msg = format!("{sub3_err:?}");
    // The bare substring `TooManySubscriptions` covers both the OPC UA
    // wire-level `BadTooManySubscriptions` status and any hypothetical
    // future async-opcua rename that drops the `Bad` prefix. A
    // regression where rejection happens for an unrelated reason
    // (transport drop, internal panic, BadTimeout) would not contain
    // this substring and the assertion would fire.
    assert!(
        sub3_msg.contains("TooManySubscriptions"),
        "3rd CreateSubscription must fail with BadTooManySubscriptions — got {sub3_msg}"
    );

    // Cleanup — best-effort, the failed third has nothing to delete.
    let _ = held.session.delete_subscription(sub1).await;
    let _ = held.session.delete_subscription(sub2).await;
    held.disconnect().await;
}

/// AC#3.2: a single subscription is capped at
/// `max_monitored_items_per_sub` monitored items. Past the cap, the
/// rejection arrives in one of three shapes (per-item status code,
/// service-level error, or silent truncation). Whatever the shape,
/// the **bound check** is load-bearing: total successful monitored
/// items on the subscription must be ≤ cap.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_monitored_item_flood_capped_by_max_monitored_items_per_sub() {
    init_test_subscriber();
    clear_captured_buffer();

    let server = setup_test_server_with_subscription_limits(
        2,
        SubscriptionLimitsForTest {
            max_monitored_items_per_sub: Some(3),
            ..Default::default()
        },
    )
    .await;

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    let sub_id = held
        .session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(|_dv, _item| {}),
        )
        .await
        .expect("CreateSubscription");

    let make_request = |client_handle: u32| MonitoredItemCreateRequest {
        item_to_monitor: ReadValueId::from(NodeId::new(
            OPCGW_NAMESPACE_INDEX,
            format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME),
        )),
        monitoring_mode: MonitoringMode::Reporting,
        requested_parameters: opcua::types::MonitoringParameters {
            client_handle,
            sampling_interval: 1000.0,
            filter: opcua::types::ExtensionObject::null(),
            queue_size: 10,
            discard_oldest: true,
        },
    };

    // First call — 3 items, all should succeed (we are at the cap).
    let first_results = held
        .session
        .create_monitored_items(
            sub_id,
            TimestampsToReturn::Both,
            vec![make_request(1), make_request(2), make_request(3)],
        )
        .await
        .expect("first CreateMonitoredItems call must succeed at cap");
    let first_ok_count = first_results
        .iter()
        .filter(|r| r.result.status_code.is_good())
        .count();
    assert_eq!(
        first_ok_count, 3,
        "first 3 monitored items must succeed (at cap, not past it)"
    );

    // Second call — 1 item, would push the subscription past the
    // cap. Accept any of the three rejection shapes.
    let second_call = held
        .session
        .create_monitored_items(
            sub_id,
            TimestampsToReturn::Both,
            vec![make_request(4)],
        )
        .await;

    // Compute the total number of "successful" monitored items that
    // were ever assigned a non-zero MonitoredItemId — this is the
    // bound the cap is supposed to enforce, regardless of the
    // rejection shape.
    let second_successes: usize = match &second_call {
        Ok(results) => results
            .iter()
            .filter(|r| r.result.status_code.is_good())
            .count(),
        Err(_) => 0,
    };
    let total_successes = first_ok_count + second_successes;

    eprintln!(
        "AC#3.2 observed rejection shape: second_call={:?}, second_successes={}, total={}",
        second_call.as_ref().map(|v| v.iter().map(|r| r.result.status_code).collect::<Vec<_>>()),
        second_successes,
        total_successes,
    );

    // Bound check — total must equal exactly 3 (the cap). Both failure
    // modes are caught by `assert_eq`:
    // - Lower bound: a regression where the library returned
    //   all-rejected on the second call AND retroactively marked
    //   first-call items as bad would surface as total < 3.
    // - Upper bound: a silent-truncation failure mode where async-opcua
    //   accepted a 4th item without signalling rejection in
    //   `second_call` would surface as total > 3.
    assert_eq!(
        total_successes, 3,
        "total successful monitored items must equal max_monitored_items_per_sub (3) — \
         got {total_successes}; first_ok_count={first_ok_count}, second_successes={second_successes}"
    );

    let _ = held.session.delete_subscription(sub_id).await;
    held.disconnect().await;
}

/// AC#3.3: a subscription continues to deliver notifications as the
/// backing metric ages past `stale_threshold_seconds` — Story 5-2's
/// stale-status logic must propagate through the subscription path
/// for compliant SCADA clients.
///
/// **Filter contract.** The test supplies an explicit
/// `DataChangeFilter { trigger: StatusValue, .. }` (OPC UA Part 4
/// §7.22.2; the library default is `Status` per the
/// `#[opcua(default)]` annotation on `DataChangeTrigger::Status` in
/// `async-opcua-types` — that default fires only on status changes, so
/// compliant SCADA clients like FUXA, Ignition, and UaExpert override
/// it to `StatusValue` or `StatusValueTimestamp` to also trigger on
/// value changes). With `StatusValue`, async-opcua routes the dedup
/// decision through `is_changed()` in `async-opcua-types::data_change`
/// which compares `v1.status != v2.status` and so detects Good →
/// Uncertain / Bad transitions even when the numeric value is
/// unchanged. **Without any filter** (`ExtensionObject::null()`),
/// async-opcua falls into the value-only dedup path in
/// `MonitoredItem::notify_data_value` — status-only transitions are
/// silently suppressed; that
/// Plan-A behaviour is pinned by
/// `test_subscription_unfiltered_dedupes_status_only_transitions`
/// below as a regression baseline against issue #94.
///
/// **Test shape.** Tight `stale_threshold_seconds = 2`, seed a fresh
/// value (gives the baseline `Good` notification), wait ~6 s (3×
/// threshold) without writing, expect a notification carrying a
/// stale (`Uncertain` or `Bad`) status. Then write a fresh value,
/// expect `Good` again — the subscription survives the outage
/// without re-creation.
///
/// Wall-clock sleep (no `tokio::time::pause()`) — Story 5-2's
/// staleness check reads the system clock, not tokio's virtual one.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_survives_chirpstack_outage_with_stale_status() {
    init_test_subscriber();
    clear_captured_buffer();

    let server = setup_test_server_with_subscription_limits(
        2,
        SubscriptionLimitsForTest {
            stale_threshold_seconds: Some(2),
            ..Default::default()
        },
    )
    .await;

    // Seed a fresh value so the baseline notification carries Good
    // status.
    server
        .backend
        .batch_write_metrics(vec![BatchMetricWrite {
            device_id: SPIKE_DEVICE_ID.to_string(),
            metric_name: SPIKE_METRIC_NAME.to_string(),
            value: "42.0".to_string(),
            data_type: MetricType::Float,
            timestamp: std::time::SystemTime::now(),
        }])
        .expect("seed initial fresh value");

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    let (tx, mut rx) = mpsc::unbounded_channel::<opcua::types::DataValue>();
    let sub_id = held
        .session
        .create_subscription(
            Duration::from_millis(500),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |dv, _item| {
                let _ = tx.send(dv);
            }),
        )
        .await
        .expect("CreateSubscription");

    held.session
        .create_monitored_items(
            sub_id,
            TimestampsToReturn::Both,
            vec![MonitoredItemCreateRequest {
                item_to_monitor: ReadValueId::from(NodeId::new(
                    OPCGW_NAMESPACE_INDEX,
                    format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME),
                )),
                monitoring_mode: MonitoringMode::Reporting,
                requested_parameters: opcua::types::MonitoringParameters {
                    client_handle: 1,
                    sampling_interval: 500.0,
                    // Explicit DataChangeFilter with
                    // trigger=StatusValue (OPC UA Part 4 §7.22.2; the
                    // library default Status would not fire on value
                    // changes — see test docstring). Routes
                    // async-opcua's dedup through `is_changed()` which
                    // compares status, so the Good→Uncertain transition
                    // fires a notification even when the numeric value
                    // is unchanged. Without this filter the unfiltered
                    // path (`monitored_item.rs:514-517`) would dedup
                    // on value only and silently drop status
                    // transitions — see docs/security.md.
                    // `..Default::default()` defends against upstream
                    // adding new fields in a minor bump.
                    filter: ExtensionObject::from_message(DataChangeFilter {
                        trigger: DataChangeTrigger::StatusValue,
                        ..Default::default()
                    }),
                    queue_size: 10,
                    discard_oldest: true,
                },
            }],
        )
        .await
        .expect("CreateMonitoredItems");

    // Wait for the baseline Good notification.
    let baseline = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("baseline notification must arrive within 5 s")
        .expect("baseline channel closed");
    let baseline_status = baseline
        .status
        .unwrap_or(opcua::types::StatusCode::Good);
    assert!(
        baseline_status.is_good(),
        "baseline notification must carry Good status — got {:?}",
        baseline_status
    );

    // Simulate a ChirpStack outage: do not write anything for ~12 s
    // (6× stale_threshold_seconds). Generous budget defends against
    // loaded CI runners where the publish loop and staleness check may
    // have several-second jitter. The wall-clock sleep is required
    // because the staleness logic reads the system clock, NOT
    // tokio's virtual clock — `tokio::time::pause()` would not move
    // the staleness threshold.
    let outage_duration = Duration::from_secs(12);
    let outage_deadline = std::time::Instant::now() + outage_duration;
    let mut saw_stale = false;
    let mut notifications_seen = 0usize;
    while std::time::Instant::now() < outage_deadline {
        // Per-iteration timeout = remaining budget (capped at 4 s) so
        // the loop consumes the full outage window even when no
        // notifications fire, instead of fixed 2 s polls that could
        // collectively miss the staleness transition.
        let remaining = outage_deadline.saturating_duration_since(std::time::Instant::now());
        let poll = remaining.min(Duration::from_secs(4));
        if poll.is_zero() {
            break;
        }
        match tokio::time::timeout(poll, rx.recv()).await {
            Ok(Some(dv)) => {
                notifications_seen += 1;
                if let Some(status) = dv.status {
                    if !status.is_good() {
                        saw_stale = true;
                        eprintln!(
                            "AC#3.3 saw stale-status notification: status={:?}, value={:?}",
                            status, dv.value
                        );
                        break;
                    }
                }
            }
            Ok(None) => panic!("notification channel closed during outage simulation"),
            Err(_) => {
                // Poll window expired with no notification — keep
                // waiting until outage_deadline.
            }
        }
    }

    // CRITICAL — failure-mode pause. Two distinct failure modes:
    // (a) zero notifications across the outage = the publish loop
    //     never fired (test environment / async-opcua sampler broken,
    //     not a Phase B regression). Surface as a clear test
    //     environment error.
    // (b) notifications fired but all carried Good = async-opcua is
    //     suppressing status-only transitions. Real Phase-B regression
    //     that needs spec discussion before shipping.
    if !saw_stale {
        assert!(
            notifications_seen > 0,
            "AC#3.3 environment error: zero notifications across {} s outage — \
             async-opcua sampler did not fire. Investigate test setup, NOT the \
             stale-status-on-subscription contract.",
            outage_duration.as_secs()
        );
        panic!(
            "AC#3.3 CRITICAL halt: {} notifications fired across {} s outage but none carried \
             stale status — async-opcua is suppressing status-only transitions. SCADA \
             dashboards would silently freeze on the last-good value during a real ChirpStack \
             outage. STOP and escalate per Story 8-2 AC#3.3 CRITICAL note.",
            notifications_seen,
            outage_duration.as_secs()
        );
    }

    // Recovery — write a fresh value, expect a Good notification.
    server
        .backend
        .batch_write_metrics(vec![BatchMetricWrite {
            device_id: SPIKE_DEVICE_ID.to_string(),
            metric_name: SPIKE_METRIC_NAME.to_string(),
            value: "84.0".to_string(),
            data_type: MetricType::Float,
            timestamp: std::time::SystemTime::now(),
        }])
        .expect("recovery write");

    let recovery_deadline = std::time::Instant::now() + Duration::from_secs(10);
    let mut saw_good = false;
    while std::time::Instant::now() < recovery_deadline {
        match tokio::time::timeout(Duration::from_secs(2), rx.recv()).await {
            Ok(Some(dv)) => {
                // Recovery requires (a) the new value 84.0 AND (b) a
                // Good or omitted status. The combination defends
                // against a regression that strips status codes
                // entirely (would mask a real protocol bug if we
                // accepted any None as Good).
                let value_matches = matches!(
                    dv.value.as_ref(),
                    Some(opcua::types::Variant::Float(f)) if (*f - 84.0_f32).abs() < f32::EPSILON
                );
                let status_ok = match dv.status {
                    Some(s) => s.is_good(),
                    None => true, // server may omit Good per OPC UA encoding rules
                };
                if value_matches && status_ok {
                    saw_good = true;
                    eprintln!(
                        "AC#3.3 saw recovery notification: status={:?}, value={:?}",
                        dv.status, dv.value
                    );
                    break;
                }
            }
            Ok(None) => panic!("notification channel closed during recovery"),
            Err(_) => {}
        }
    }
    assert!(
        saw_good,
        "no recovery notification fired within 10 s after fresh-value write — \
         the subscription failed to recover from the simulated outage"
    );

    let _ = held.session.delete_subscription(sub_id).await;
    held.disconnect().await;
}

/// Regression baseline (Story 8-2 code review): a subscription client
/// that supplies **no** `DataChangeFilter` (`ExtensionObject::null()`)
/// loses status-only transitions because async-opcua 0.17.1's
/// `MonitoredItem::notify_data_value` (`monitored_item.rs:514-517`)
/// dedupes on `value.value` only when `FilterType::None`. This is a
/// documented Plan-A gap: SCADA dashboards using a non-compliant
/// client would silently freeze on the last-good value during a
/// ChirpStack outage.
///
/// **This test pins the gap as the current contract.** If async-opcua
/// changes the implicit default to consider status (e.g. by adopting
/// `DataChangeTrigger::StatusValue` as the no-filter default), this
/// test fails — at which point we should: (a) update the
/// `docs/security.md#subscription-and-message-size-limits`
/// `DataChangeFilter` contract subsection, (b) close the upstream FR
/// at GitHub issue #94, and (c) consider whether the
/// `test_subscription_survives_chirpstack_outage_with_stale_status`
/// test still needs the explicit filter for compliant-client coverage.
///
/// **Test shape.** Same outage-simulation setup as the AC#3.3
/// compliant-client test, but with `filter: ExtensionObject::null()`
/// on the MonitoredItem. We expect the unfiltered path to emit the
/// baseline Good notification, then suppress all status-only
/// transitions across the outage window. The test passes when no
/// stale-status notification fires.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_subscription_unfiltered_dedupes_status_only_transitions() {
    init_test_subscriber();
    clear_captured_buffer();

    let server = setup_test_server_with_subscription_limits(
        2,
        SubscriptionLimitsForTest {
            stale_threshold_seconds: Some(2),
            ..Default::default()
        },
    )
    .await;

    server
        .backend
        .batch_write_metrics(vec![BatchMetricWrite {
            device_id: SPIKE_DEVICE_ID.to_string(),
            metric_name: SPIKE_METRIC_NAME.to_string(),
            value: "42.0".to_string(),
            data_type: MetricType::Float,
            timestamp: std::time::SystemTime::now(),
        }])
        .expect("seed initial fresh value");

    let held = open_session_held(&server, user_password_identity(), 5_000)
        .await
        .expect("session must activate");

    let (tx, mut rx) = mpsc::unbounded_channel::<opcua::types::DataValue>();
    let sub_id = held
        .session
        .create_subscription(
            Duration::from_millis(500),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |dv, _item| {
                let _ = tx.send(dv);
            }),
        )
        .await
        .expect("CreateSubscription");

    held.session
        .create_monitored_items(
            sub_id,
            TimestampsToReturn::Both,
            vec![MonitoredItemCreateRequest {
                item_to_monitor: ReadValueId::from(NodeId::new(
                    OPCGW_NAMESPACE_INDEX,
                    format!("{}/{}", SPIKE_DEVICE_ID, SPIKE_METRIC_OPCUA_NAME),
                )),
                monitoring_mode: MonitoringMode::Reporting,
                requested_parameters: opcua::types::MonitoringParameters {
                    client_handle: 1,
                    sampling_interval: 500.0,
                    // No DataChangeFilter — pins the documented
                    // value-only-dedup behaviour at
                    // `monitored_item.rs:514-517`.
                    filter: ExtensionObject::null(),
                    queue_size: 10,
                    discard_oldest: true,
                },
            }],
        )
        .await
        .expect("CreateMonitoredItems");

    // Drain the baseline Good notification (the subscription always
    // delivers at least one initial value).
    let _baseline = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("baseline notification must arrive within 5 s")
        .expect("baseline channel closed");

    // Outage window — 8 s, 4× stale_threshold_seconds. Long enough
    // that a compliant filter would have fired a stale notification
    // by now (verified by the sibling test). With FilterType::None,
    // we expect zero status-only notifications.
    let outage_duration = Duration::from_secs(8);
    let outage_deadline = std::time::Instant::now() + outage_duration;
    let mut saw_status_only_transition = false;
    while std::time::Instant::now() < outage_deadline {
        let remaining = outage_deadline.saturating_duration_since(std::time::Instant::now());
        let poll = remaining.min(Duration::from_secs(2));
        if poll.is_zero() {
            break;
        }
        match tokio::time::timeout(poll, rx.recv()).await {
            Ok(Some(dv)) => {
                if let Some(status) = dv.status {
                    if !status.is_good() {
                        saw_status_only_transition = true;
                        eprintln!(
                            "UNEXPECTED unfiltered notification: status={:?}, value={:?}",
                            status, dv.value
                        );
                        break;
                    }
                }
            }
            Ok(None) => panic!("notification channel closed during outage simulation"),
            Err(_) => {}
        }
    }

    assert!(
        !saw_status_only_transition,
        "REGRESSION: async-opcua emitted a status-only transition on a MonitoredItem \
         with FilterType::None — the documented value-only-dedup contract has changed. \
         Update docs/security.md#subscription-and-message-size-limits and consider closing \
         the upstream FR at GitHub issue #94."
    );

    let _ = held.session.delete_subscription(sub_id).await;
    held.disconnect().await;
}


/// Regression baseline (Story 8-2 code review): pin the field shape of
/// the `event="opcua_limits_configured"` startup diagnostic. Operators
/// grep this line on every restart per `docs/security.md` to verify the
/// resolved configuration; a future field-name rename would silently
/// break their runbooks. This test starts a server with explicit
/// non-default values for all five fields and asserts the captured
/// log line carries each one verbatim.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_resolved_limits_logged_at_startup() {
    init_test_subscriber();
    clear_captured_buffer();

    // Explicit non-default values across all four knobs. max_sessions
    // is set via the helper's first parameter and emitted alongside.
    let _server = setup_test_server_with_subscription_limits(
        7, // max_sessions = 7 (non-default)
        SubscriptionLimitsForTest {
            max_subscriptions_per_session: Some(13),
            max_monitored_items_per_sub: Some(257),
            max_message_size: Some(131_070), // 2 chunks × 65535 = coherent
            max_chunk_count: Some(2),
            ..Default::default()
        },
    )
    .await;

    // The startup info log is emitted synchronously during
    // `OpcUa::run` setup, but the tracing subscriber needs a beat to
    // flush. Poll the buffer for up to 10 s with 100 ms ticks (the
    // longer budget defends against heavily-loaded CI runners where
    // the server's tokio::spawn task may not interleave with the
    // assertion loop for several seconds).
    let mut found = false;
    for _ in 0..100 {
        if captured_log_line_contains_all(&[
            "event=\"opcua_limits_configured\"",
            "max_sessions=7",
            "max_subscriptions_per_session=13",
            "max_monitored_items_per_sub=257",
            "max_message_size=131070",
            "max_chunk_count=2",
        ]) {
            found = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    assert!(
        found,
        "captured log buffer must contain a single line with \
         event=\"opcua_limits_configured\" plus all five explicit \
         field=value pairs (max_sessions=7, max_subscriptions_per_session=13, \
         max_monitored_items_per_sub=257, max_message_size=131070, \
         max_chunk_count=2). Operator runbooks at docs/security.md grep \
         this exact field shape on every restart."
    );
}

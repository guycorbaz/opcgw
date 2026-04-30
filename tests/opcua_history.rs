// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2024] [Guy Corbaz]
//
// Story 8-3 AC#2 integration tests: end-to-end OPC UA HistoryRead pipeline
// against a live opcgw server. Pins:
//
//   - HistoryRead returns seeded `metric_history` rows for a registered
//     metric NodeId (test_history_read_returns_seeded_rows)
//   - Empty range surfaces as empty `HistoryData.dataValues` with `Good`
//     status (test_history_read_empty_range_returns_empty_data_values)
//   - Inverted time range (end < start) surfaces as `BadInvalidArgument`
//     (test_history_read_invalid_time_range_returns_bad_invalid_argument)
//   - HistoryRead on an unregistered NodeId surfaces as `BadNodeIdUnknown`
//     (test_history_read_unknown_node_returns_bad_node_id_unknown)
//   - Per-node response truncates at `max_history_data_results_per_node`
//     (test_history_read_max_results_truncates_at_limit)
//
// Note: a sixth case in the AC#2 spec — "concurrent with subscription in
// same session" — is covered by the NFR12 carry-forward (Story 8-2's
// session-layer auth + at-limit pin tests in
// `tests/opcua_subscription_spike.rs`); HistoryRead-issuing clients flow
// through `OpcgwAuthManager` + `AtLimitAcceptLayer` identically to
// subscription-issuing clients, and concurrent service dispatch is an
// async-opcua property pinned by the existing subscription-basic /
// two-clients-share-node tests. No new contract surface here.
//
// Test-harness shape mirrors `tests/opcua_subscription_spike.rs` — per
// CLAUDE.md scope-discipline rule "the fourth integration-test file
// crosses the threshold for `tests/common/` extraction", but the spec
// (Story 8-3 Dev Notes → Test-harness strategy) allows deferring the
// extraction if invasive. The four files now diverge slightly (history
// vs. subscription / connection-limit / security) and the extraction
// would touch all four — deferred to a separate cleanup story.

use std::sync::Arc;
use std::time::Duration;

use opcua::client::{
    ClientBuilder, DataChangeCallback, IdentityToken, Password as ClientPassword, Session,
};
use opcua::types::{
    DateTime as OpcDateTime, EndpointDescription, ExtensionObject, HistoryData,
    HistoryReadValueId, MessageSecurityMode, MonitoredItemCreateRequest, MonitoringMode, NodeId,
    ReadRawModifiedDetails, ReadValueId, StatusCode, TimestampsToReturn, UserTokenPolicy,
    UserTokenType,
};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio_util::sync::CancellationToken;

use opcgw::config::{
    AppConfig, ChirpStackApplications, ChirpstackDevice, ChirpstackPollerConfig,
    CommandValidationConfig, Global, OpcMetricTypeConfig, OpcUaConfig, ReadMetric, StorageConfig,
};
use opcgw::opc_ua::OpcUa;
use opcgw::storage::{
    BatchMetricWrite, ConnectionPool, MetricType, SqliteBackend, StorageBackend,
};

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-8-3";
const TEST_DEVICE_ID: &str = "0000000000000001";
const TEST_METRIC_NAME: &str = "moisture";
const TEST_METRIC_OPCUA_NAME: &str = "Moisture";
// `ns = 2`: ns 0 is the OPC UA standard namespace, ns 1 is the
// server-local namespace, the first user-supplied `NamespaceMetadata`
// gets index 2 (confirmed by `tests/opcua_subscription_spike.rs:70`).
const OPCGW_NAMESPACE_INDEX: u16 = 2;

fn user_name_policy() -> UserTokenPolicy {
    UserTokenPolicy {
        token_type: UserTokenType::UserName,
        ..UserTokenPolicy::anonymous()
    }
}

async fn pick_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

fn history_test_config(
    port: u16,
    pki_dir: &std::path::Path,
    max_results_per_node: Option<usize>,
) -> AppConfig {
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
            application_name: "opcgw-history-8-3".to_string(),
            application_uri: "urn:opcgw:history:8-3".to_string(),
            product_uri: "urn:opcgw:history:8-3:product".to_string(),
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
            max_connections: Some(2),
            max_subscriptions_per_session: None,
            max_monitored_items_per_sub: None,
            max_message_size: None,
            max_chunk_count: None,
            max_history_data_results_per_node: max_results_per_node,
        },
        application_list: vec![ChirpStackApplications {
            application_name: "HistoryApp".to_string(),
            application_id: "00000000-0000-0000-0000-000000000001".to_string(),
            device_list: vec![ChirpstackDevice {
                device_name: "HistoryDevice".to_string(),
                device_id: TEST_DEVICE_ID.to_string(),
                read_metric_list: vec![ReadMetric {
                    metric_name: TEST_METRIC_OPCUA_NAME.to_string(),
                    chirpstack_metric_name: TEST_METRIC_NAME.to_string(),
                    metric_type: OpcMetricTypeConfig::Float,
                    metric_unit: Some("pct".to_string()),
                }],
                device_command_list: None,
            }],
        }],
        storage: StorageConfig::default(),
        command_validation: CommandValidationConfig::default(),
    }
}

struct TestServer {
    port: u16,
    cancel: CancellationToken,
    handle: Option<tokio::task::JoinHandle<()>>,
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
    }
}

async fn setup_test_server(max_results_per_node: Option<usize>) -> TestServer {
    let tmp = TempDir::new().expect("create temp dir");
    let port = pick_free_port().await;
    let pki_dir = tmp.path().join("pki");
    let db_path = tmp.path().join("opcgw.db");

    let config = Arc::new(history_test_config(port, &pki_dir, max_results_per_node));
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

fn build_client(client_pki: &std::path::Path) -> opcua::client::Client {
    ClientBuilder::new()
        .application_name("opcgw-history-8-3-client")
        .application_uri("urn:opcgw:history:8-3:client")
        .product_uri("urn:opcgw:history:8-3:client")
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

/// Open and activate a session against the server. Returns the session +
/// the client tmp dir + the event loop join handle (kept alive for the
/// session's lifetime). The caller is responsible for calling
/// `disconnect()` and aborting the event loop.
async fn open_session(
    server: &TestServer,
) -> (
    Arc<Session>,
    TempDir,
    opcua::client::Client,
    tokio::task::JoinHandle<StatusCode>,
) {
    let client_tmp = TempDir::new().expect("client tmp");
    let mut client = build_client(client_tmp.path());
    let endpoint: EndpointDescription = (
        server.endpoint_url("/").as_str(),
        "None",
        MessageSecurityMode::None,
        user_name_policy(),
    )
        .into();

    let (session, event_loop) = tokio::time::timeout(
        Duration::from_secs(5),
        client.connect_to_matching_endpoint(endpoint, user_password_identity()),
    )
    .await
    .expect("connect timeout")
    .expect("connect failed");
    session.disable_reconnects();
    let event_handle = event_loop.spawn();
    let connected = tokio::time::timeout(Duration::from_secs(5), session.wait_for_connection())
        .await
        .unwrap_or(false);
    assert!(connected, "session must activate");
    (session, client_tmp, client, event_handle)
}

fn seed_rows(
    backend: &Arc<dyn StorageBackend>,
    base_ts: std::time::SystemTime,
    count: usize,
) {
    for i in 0..count {
        let ts = base_ts + Duration::from_secs(i as u64);
        backend
            .batch_write_metrics(vec![BatchMetricWrite {
                device_id: TEST_DEVICE_ID.to_string(),
                metric_name: TEST_METRIC_NAME.to_string(),
                value: format!("{}.0", 20 + i),
                data_type: MetricType::Float,
                timestamp: ts,
            }])
            .expect("seed");
    }
}

/// Convenience: build a `ReadRawModifiedDetails` extension object with
/// the given start/end window and a sane default for the rest.
fn read_raw_details(
    start: std::time::SystemTime,
    end: std::time::SystemTime,
    num_values_per_node: u32,
) -> ExtensionObject {
    let details = ReadRawModifiedDetails {
        is_read_modified: false,
        start_time: OpcDateTime::from(chrono::DateTime::<chrono::Utc>::from(start)),
        end_time: OpcDateTime::from(chrono::DateTime::<chrono::Utc>::from(end)),
        num_values_per_node,
        return_bounds: false,
    };
    ExtensionObject::from_message(details)
}

/// Decode the per-node `HistoryReadResult.history_data` extension object
/// into the contained `HistoryData` (what the spec calls
/// `HistoryData.dataValues`). Returns the inner `Vec<DataValue>` (empty
/// for empty ranges).
fn decode_history_data(eo: &ExtensionObject) -> Vec<opcua::types::DataValue> {
    if eo.is_null() {
        return Vec::new();
    }
    let hd = eo
        .inner_as::<HistoryData>()
        .expect("expected HistoryData payload");
    hd.data_values.clone().unwrap_or_default()
}

// -----------------------------------------------------------------------
// AC#2 tests
// -----------------------------------------------------------------------

/// AC#2: HistoryRead returns the seeded rows verbatim — proves the
/// end-to-end pipeline (HistoryReadDetails::RawModified → service
/// dispatch → `OpcgwHistoryNodeManagerImpl::history_read_raw_modified`
/// → `StorageBackend::query_metric_history` → `HistoryData` extension
/// object → wire) works against an unmodified async-opcua client.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_history_read_returns_seeded_rows() {
    let server = setup_test_server(None).await;

    // Seed 5 rows. Use a base timestamp far enough in the past that
    // `Utc::now()` definitely sits after `base + 60s`.
    let base = std::time::SystemTime::now() - Duration::from_secs(300);
    seed_rows(&server.backend, base, 5);

    let (session, _ctmp, _client, event_handle) = open_session(&server).await;

    let node_id = NodeId::new(OPCGW_NAMESPACE_INDEX, TEST_METRIC_OPCUA_NAME);
    let start = base;
    let end = base + Duration::from_secs(60);
    let details_eo = read_raw_details(start, end, 100);
    let details = opcua::client::HistoryReadAction::ReadRawModifiedDetails(
        details_eo
            .inner_as::<ReadRawModifiedDetails>()
            .expect("decode details")
            .clone(),
    );

    let nodes = vec![HistoryReadValueId {
        node_id: node_id.clone(),
        index_range: opcua::types::NumericRange::None,
        data_encoding: opcua::types::QualifiedName::null(),
        continuation_point: opcua::types::ByteString::null(),
    }];
    let results = session
        .history_read(details, TimestampsToReturn::Both, false, &nodes)
        .await
        .expect("HistoryRead must succeed");
    assert_eq!(results.len(), 1, "exactly one per-node result");
    assert!(
        results[0].status_code.is_good(),
        "per-node status must be Good, got {:?}",
        results[0].status_code
    );
    let dvs = decode_history_data(&results[0].history_data);
    assert_eq!(dvs.len(), 5, "must return all 5 seeded rows");

    let _ = session.disconnect().await;
    event_handle.abort();
}

/// AC#2: A query window with no rows surfaces as an empty
/// `HistoryData.data_values` with `Good` per-node status. NOT
/// `BadNoData` — that would terminate the iterator pattern that AC#5's
/// manual-paging recipe relies on.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_history_read_empty_range_returns_empty_data_values() {
    let server = setup_test_server(None).await;

    // Seed rows OUTSIDE the queried range so the query returns 0 rows.
    let outside_base = std::time::SystemTime::now() - Duration::from_secs(7200);
    seed_rows(&server.backend, outside_base, 10);

    let (session, _ctmp, _client, event_handle) = open_session(&server).await;

    let node_id = NodeId::new(OPCGW_NAMESPACE_INDEX, TEST_METRIC_OPCUA_NAME);
    let query_start = std::time::SystemTime::now() - Duration::from_secs(60);
    let query_end = std::time::SystemTime::now();
    let details_eo = read_raw_details(query_start, query_end, 100);
    let details = opcua::client::HistoryReadAction::ReadRawModifiedDetails(
        details_eo
            .inner_as::<ReadRawModifiedDetails>()
            .expect("decode")
            .clone(),
    );
    let nodes = vec![HistoryReadValueId {
        node_id: node_id.clone(),
        index_range: opcua::types::NumericRange::None,
        data_encoding: opcua::types::QualifiedName::null(),
        continuation_point: opcua::types::ByteString::null(),
    }];
    let results = session
        .history_read(details, TimestampsToReturn::Both, false, &nodes)
        .await
        .expect("HistoryRead must succeed");
    assert_eq!(results.len(), 1);
    assert!(
        results[0].status_code.is_good(),
        "empty range must surface as Good (not BadNoData)"
    );
    let dvs = decode_history_data(&results[0].history_data);
    assert_eq!(dvs.len(), 0, "empty range must yield 0 data values");

    let _ = session.disconnect().await;
    event_handle.abort();
}

/// AC#2: An inverted time range (`end < start`) is rejected with
/// `BadInvalidArgument` per OPC UA Part 11 §6.4.2 — the server-side
/// validation guards against a SCADA bug producing an absurd request.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_history_read_invalid_time_range_returns_bad_invalid_argument() {
    let server = setup_test_server(None).await;

    let (session, _ctmp, _client, event_handle) = open_session(&server).await;

    let node_id = NodeId::new(OPCGW_NAMESPACE_INDEX, TEST_METRIC_OPCUA_NAME);
    let now = std::time::SystemTime::now();
    let start = now;
    let end = now - Duration::from_secs(3600);
    let details_eo = read_raw_details(start, end, 100);
    let details = opcua::client::HistoryReadAction::ReadRawModifiedDetails(
        details_eo
            .inner_as::<ReadRawModifiedDetails>()
            .expect("decode")
            .clone(),
    );
    let nodes = vec![HistoryReadValueId {
        node_id: node_id.clone(),
        index_range: opcua::types::NumericRange::None,
        data_encoding: opcua::types::QualifiedName::null(),
        continuation_point: opcua::types::ByteString::null(),
    }];
    let results = session
        .history_read(details, TimestampsToReturn::Both, false, &nodes)
        .await
        .expect("HistoryRead service call must complete (per-node Bad statuses are not service errors)");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].status_code,
        StatusCode::BadInvalidArgument,
        "inverted time range must surface as BadInvalidArgument"
    );

    let _ = session.disconnect().await;
    event_handle.abort();
}

/// AC#2: HistoryRead on a NodeId that opcgw's address space does not
/// expose as a registered metric variable surfaces as `BadNodeIdUnknown`.
/// This is the reverse-lookup miss path (`node_to_metric.get(...)` is
/// `None`).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_history_read_unknown_node_returns_bad_node_id_unknown() {
    let server = setup_test_server(None).await;

    let (session, _ctmp, _client, event_handle) = open_session(&server).await;

    // A NodeId for a string that opcgw never registered as a metric.
    let node_id = NodeId::new(OPCGW_NAMESPACE_INDEX, "DefinitelyNotARegisteredMetric");
    let now = std::time::SystemTime::now();
    let details_eo = read_raw_details(now - Duration::from_secs(60), now, 100);
    let details = opcua::client::HistoryReadAction::ReadRawModifiedDetails(
        details_eo
            .inner_as::<ReadRawModifiedDetails>()
            .expect("decode")
            .clone(),
    );
    let nodes = vec![HistoryReadValueId {
        node_id: node_id.clone(),
        index_range: opcua::types::NumericRange::None,
        data_encoding: opcua::types::QualifiedName::null(),
        continuation_point: opcua::types::ByteString::null(),
    }];
    let results = session
        .history_read(details, TimestampsToReturn::Both, false, &nodes)
        .await
        .expect("HistoryRead service call must complete");
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].status_code,
        StatusCode::BadNodeIdUnknown,
        "unregistered NodeId must surface as BadNodeIdUnknown"
    );

    let _ = session.disconnect().await;
    event_handle.abort();
}

/// AC#2 + AC#3: with `[opcua].max_history_data_results_per_node = 100`
/// configured, a query that targets 200 seeded rows returns exactly 100
/// rows — the per-call cap. Per-node status remains `Good`; SCADA
/// clients are expected to manually page (Story 8-3 does not implement
/// continuation points — see AC#5).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_history_read_max_results_truncates_at_limit() {
    let server = setup_test_server(Some(100)).await;

    // Seed 200 rows; the 100-cap should truncate.
    let base = std::time::SystemTime::now() - Duration::from_secs(3600);
    seed_rows(&server.backend, base, 200);

    let (session, _ctmp, _client, event_handle) = open_session(&server).await;

    let node_id = NodeId::new(OPCGW_NAMESPACE_INDEX, TEST_METRIC_OPCUA_NAME);
    // num_values_per_node = 0 means "no client cap, use server default".
    let details_eo = read_raw_details(base, base + Duration::from_secs(3600), 0);
    let details = opcua::client::HistoryReadAction::ReadRawModifiedDetails(
        details_eo
            .inner_as::<ReadRawModifiedDetails>()
            .expect("decode")
            .clone(),
    );
    let nodes = vec![HistoryReadValueId {
        node_id: node_id.clone(),
        index_range: opcua::types::NumericRange::None,
        data_encoding: opcua::types::QualifiedName::null(),
        continuation_point: opcua::types::ByteString::null(),
    }];
    let results = session
        .history_read(details, TimestampsToReturn::Both, false, &nodes)
        .await
        .expect("HistoryRead must succeed");
    assert_eq!(results.len(), 1);
    assert!(
        results[0].status_code.is_good(),
        "per-node status must be Good"
    );
    let dvs = decode_history_data(&results[0].history_data);
    assert_eq!(dvs.len(), 100, "response must truncate at the cap");

    let _ = session.disconnect().await;
    event_handle.abort();
}

/// Story 8-3 review patch P12: cover the **client cap < server cap**
/// branch of the per-call cap. With `max_history_data_results_per_node`
/// set to 100 (server cap) but the client requesting `num_values_per_node
/// = 50`, the response must contain 50 rows (the smaller of the two), not
/// 100. The previous truncation test only exercised the
/// `num_values_per_node = 0` path (client says "no cap, use server
/// default"); this test covers `min(client_cap, server_cap)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_history_read_client_cap_below_server_cap_uses_client_cap() {
    let server = setup_test_server(Some(100)).await;

    // Seed 200 rows; both caps will truncate, but the client cap (50) wins.
    let base = std::time::SystemTime::now() - Duration::from_secs(3600);
    seed_rows(&server.backend, base, 200);

    let (session, _ctmp, _client, event_handle) = open_session(&server).await;

    let node_id = NodeId::new(OPCGW_NAMESPACE_INDEX, TEST_METRIC_OPCUA_NAME);
    // num_values_per_node = 50 — strictly below the server cap of 100.
    let details_eo = read_raw_details(base, base + Duration::from_secs(3600), 50);
    let details = opcua::client::HistoryReadAction::ReadRawModifiedDetails(
        details_eo
            .inner_as::<ReadRawModifiedDetails>()
            .expect("decode")
            .clone(),
    );
    let nodes = vec![HistoryReadValueId {
        node_id: node_id.clone(),
        index_range: opcua::types::NumericRange::None,
        data_encoding: opcua::types::QualifiedName::null(),
        continuation_point: opcua::types::ByteString::null(),
    }];
    let results = session
        .history_read(details, TimestampsToReturn::Both, false, &nodes)
        .await
        .expect("HistoryRead must succeed");
    assert_eq!(results.len(), 1);
    assert!(
        results[0].status_code.is_good(),
        "per-node status must be Good"
    );
    let dvs = decode_history_data(&results[0].history_data);
    assert_eq!(
        dvs.len(),
        50,
        "client cap must win when smaller than the server cap"
    );

    let _ = session.disconnect().await;
    event_handle.abort();
}

/// Story 8-3 review patch P-D1 / AC#2.6: HistoryRead and a Subscription's
/// MonitoredItem can run concurrently in the **same session** without
/// interference. The two services dispatch through different paths in
/// async-opcua's session-layer, but they share the underlying
/// `OpcgwHistoryNodeManagerImpl`'s reverse-lookup map (Story 8-3) and
/// `SimpleNodeManagerImpl`'s read-callback registry. This test pins:
///   - CreateSubscription + CreateMonitoredItems on the metric NodeId
///     succeeds (live read path)
///   - HistoryRead for the same NodeId in the same session succeeds and
///     returns the seeded historical rows (history path)
///   - The subscription's first DataChangeNotification arrives (publish
///     path was not blocked while HistoryRead was iterating storage —
///     validates review patch P18, which moved the lookup-map snapshot
///     out of the lock-held loop).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn test_history_read_concurrent_with_subscription_same_session() {
    use tokio::sync::mpsc;

    let server = setup_test_server(None).await;

    // Seed 5 historical rows so HistoryRead has something to return.
    let base = std::time::SystemTime::now() - Duration::from_secs(300);
    seed_rows(&server.backend, base, 5);

    let (session, _ctmp, _client, event_handle) = open_session(&server).await;

    let node_id = NodeId::new(OPCGW_NAMESPACE_INDEX, TEST_METRIC_OPCUA_NAME);

    // Step 1: create the subscription. Channel collects each
    // DataChangeNotification so the test can assert the publish path
    // is alive after HistoryRead completes.
    let (tx, mut rx) = mpsc::unbounded_channel::<opcua::types::DataValue>();
    let subscription_id = session
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
        .expect("CreateSubscription must succeed");
    assert!(
        subscription_id != 0,
        "server-assigned subscription_id must be non-zero"
    );

    // Step 2: register a monitored item on the metric NodeId.
    let create = MonitoredItemCreateRequest {
        item_to_monitor: ReadValueId::from(node_id.clone()),
        monitoring_mode: MonitoringMode::Reporting,
        requested_parameters: opcua::types::MonitoringParameters {
            client_handle: 1,
            sampling_interval: 1000.0,
            filter: ExtensionObject::null(),
            queue_size: 10,
            discard_oldest: true,
        },
    };
    let create_results = session
        .create_monitored_items(subscription_id, TimestampsToReturn::Both, vec![create])
        .await
        .expect("CreateMonitoredItems must succeed");
    assert_eq!(create_results.len(), 1);
    assert!(
        create_results[0].result.status_code.is_good(),
        "CreateMonitoredItems must be Good"
    );

    // Step 3: issue HistoryRead **while the subscription is active** in
    // the same session. The historical query must not block on (or be
    // blocked by) the publish loop.
    let start = base;
    let end = base + Duration::from_secs(60);
    let details_eo = read_raw_details(start, end, 100);
    let details = opcua::client::HistoryReadAction::ReadRawModifiedDetails(
        details_eo
            .inner_as::<ReadRawModifiedDetails>()
            .expect("decode")
            .clone(),
    );
    let nodes = vec![HistoryReadValueId {
        node_id: node_id.clone(),
        index_range: opcua::types::NumericRange::None,
        data_encoding: opcua::types::QualifiedName::null(),
        continuation_point: opcua::types::ByteString::null(),
    }];
    let history_results = session
        .history_read(details, TimestampsToReturn::Both, false, &nodes)
        .await
        .expect("HistoryRead must succeed concurrently with active subscription");
    assert_eq!(history_results.len(), 1);
    assert!(
        history_results[0].status_code.is_good(),
        "HistoryRead status must be Good — subscription must not interfere"
    );
    let dvs = decode_history_data(&history_results[0].history_data);
    assert_eq!(
        dvs.len(),
        5,
        "HistoryRead must return all 5 seeded rows even with active subscription"
    );

    // Step 4: confirm the subscription's publish path is still alive
    // after HistoryRead returned. The first notification should arrive
    // within ~1 publishing-interval of subscription creation; a
    // generous 10s timeout absorbs CI variance.
    let first_notification = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect(
            "subscription DataChangeNotification must arrive within 10s — \
             HistoryRead must not block the publish path (review patch P18 regression)",
        )
        .expect("notification channel closed unexpectedly");
    assert!(
        first_notification.value.is_some()
            || first_notification.status.is_some()
            || first_notification.source_timestamp.is_some(),
        "DataChangeNotification must carry at least one populated field"
    );

    // Tear down the subscription cleanly.
    let _ = session.delete_subscription(subscription_id).await;
    let _ = session.disconnect().await;
    event_handle.abort();
}

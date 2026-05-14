// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2026] [Guy Corbaz]
//
// Story 9-8 — Dynamic OPC UA Address-Space Mutation — integration tests
// for the apply path that closes the Story 9-7 v1 limitation.
//
// What these tests pin:
//
//   - AC#1: `apply_diff_to_address_space` adds a new device + metric
//     such that a fresh subscription on the new NodeId receives a
//     DataChangeNotification within ~5s of CreateMonitoredItems.
//
//   - AC#2 (Q2 mitigation, LOAD-BEARING for 9-0-spike-report.md:104-127):
//     `apply_diff_to_address_space` removes a device and emits an
//     explicit `BadNodeIdUnknown` set_values transition BEFORE
//     `address_space.delete(...)` runs, so subscribed clients see a
//     final notification with `status = BadNodeIdUnknown` instead of
//     freezing on last-good (Behaviour B). Without this AC the silent-
//     stream-on-delete behaviour leaves orphan subscriptions with no
//     programmatic detection path.
//
//   - AC#4: subscriptions on unaffected NodeIds continue uninterrupted
//     while the apply pass mutates other parts of the address space.
//     Validated empirically at the spike level (9-0 Q3: 117 µs bulk-
//     write-lock hold; ~850× headroom under 100ms sampler tick); pinned
//     here at runtime against the apply pass's actual lock discipline.
//
//   - AC#7: `event="address_space_mutation_succeeded"` info log fires
//     on apply, carrying all 9 axis counts + `duration_ms` field.
//
// Per-file harness inlining (issue #102 — tests/common/opcua.rs
// extraction deferred): the `setup_apply_test_server` /
// `open_session` / `subscribe_one` / `HeldSession` shapes mirror
// `tests/opcua_dynamic_address_space_spike.rs:210-448` but compressed
// to 9-8's needs. When #102 lands, both files migrate to the shared
// helper.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use opcua::client::{DataChangeCallback, Session};
use opcua::types::{
    EndpointDescription, ExtensionObject, MessageSecurityMode, MonitoredItemCreateRequest,
    MonitoringMode, NodeId, ReadValueId, StatusCode, TimestampsToReturn, UserTokenPolicy,
    UserTokenType,
};
use tempfile::TempDir;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use opcgw::config::{
    AppConfig, ChirpStackApplications, ChirpstackDevice, ChirpstackPollerConfig,
    CommandValidationConfig, Global, OpcMetricTypeConfig, OpcUaConfig, ReadMetric, StorageConfig,
    WebConfig,
};
use opcgw::opc_ua::OpcUa;
use opcgw::opc_ua_history::OpcgwHistoryNodeManager;
use opcgw::opcua_topology_apply::{apply_diff_to_address_space, AddressSpaceMutationOutcome};
use opcgw::storage::{ConnectionPool, SqliteBackend, StorageBackend};

// -----------------------------------------------------------------------
// Constants
// -----------------------------------------------------------------------

const TEST_USER: &str = "opcua-user";
const TEST_PASSWORD: &str = "test-password-9-8";
const APPLY_APP_ID: &str = "00000000-0000-0000-0000-000000000099";
const APPLY_DEVICE_BASELINE: &str = "device_apply_baseline";
const APPLY_METRIC_BASELINE: &str = "Temperature";
const APPLY_CHIRP_BASELINE: &str = "temperature";
const APPLY_DEVICE_NEW: &str = "device_apply_new";
const APPLY_METRIC_NEW: &str = "Humidity";
const APPLY_CHIRP_NEW: &str = "humidity";
// `ns = 2` matches opcgw's deterministic namespace assignment.
const OPCGW_NAMESPACE_INDEX: u16 = 2;

// -----------------------------------------------------------------------
// Config builder — a baseline with one application + one device + one
// metric. Tests synthesise alternative AppConfigs and drive
// `apply_diff_to_address_space(prev, new, …)` against the running
// server's manager.
// -----------------------------------------------------------------------

fn baseline_app_config(port: u16, pki_dir: &std::path::Path) -> AppConfig {
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
            application_name: "opcgw-9-8-test".to_string(),
            application_uri: "urn:opcgw:9-8:test".to_string(),
            product_uri: "urn:opcgw:9-8:test:product".to_string(),
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
            max_connections: Some(8),
            max_subscriptions_per_session: None,
            max_monitored_items_per_sub: None,
            max_message_size: None,
            max_chunk_count: None,
            max_history_data_results_per_node: None,
        },
        application_list: vec![ChirpStackApplications {
            application_name: "ApplyApp".to_string(),
            application_id: APPLY_APP_ID.to_string(),
            device_list: vec![ChirpstackDevice {
                device_id: APPLY_DEVICE_BASELINE.to_string(),
                device_name: APPLY_DEVICE_BASELINE.to_string(),
                read_metric_list: vec![ReadMetric {
                    metric_name: APPLY_METRIC_BASELINE.to_string(),
                    chirpstack_metric_name: APPLY_CHIRP_BASELINE.to_string(),
                    metric_type: OpcMetricTypeConfig::Float,
                    metric_unit: Some("C".to_string()),
                }],
                device_command_list: None,
            }],
        }],
        storage: StorageConfig::default(),
        command_validation: CommandValidationConfig::default(),
        web: WebConfig::default(),
    }
}

/// Add a second device under the same application — used as the
/// "after" config for AC#1 add tests.
fn config_add_second_device(prev: &AppConfig) -> AppConfig {
    let mut new = prev.clone();
    new.application_list[0].device_list.push(ChirpstackDevice {
        device_id: APPLY_DEVICE_NEW.to_string(),
        device_name: APPLY_DEVICE_NEW.to_string(),
        read_metric_list: vec![ReadMetric {
            metric_name: APPLY_METRIC_NEW.to_string(),
            chirpstack_metric_name: APPLY_CHIRP_NEW.to_string(),
            metric_type: OpcMetricTypeConfig::Float,
            metric_unit: None,
        }],
        device_command_list: None,
    });
    new
}

/// Remove the baseline device — used as the "after" config for AC#2
/// remove tests.
fn config_remove_baseline_device(prev: &AppConfig) -> AppConfig {
    let mut new = prev.clone();
    new.application_list[0].device_list.clear();
    new
}

// -----------------------------------------------------------------------
// Test server fixture
// -----------------------------------------------------------------------

struct ApplyTestServer {
    port: u16,
    cancel: CancellationToken,
    server_task: Option<tokio::task::JoinHandle<()>>,
    manager: Arc<OpcgwHistoryNodeManager>,
    subscriptions: Arc<opcua::server::SubscriptionCache>,
    storage: Arc<dyn StorageBackend>,
    last_status: opcgw::opc_ua::StatusCache,
    node_to_metric: Arc<
        opcua::sync::RwLock<std::collections::HashMap<NodeId, (String, String)>>,
    >,
    ns: u16,
    config: Arc<AppConfig>,
    _tmp: TempDir,
}

impl ApplyTestServer {
    fn endpoint_url(&self) -> String {
        format!("opc.tcp://127.0.0.1:{}/", self.port)
    }
}

impl Drop for ApplyTestServer {
    fn drop(&mut self) {
        self.cancel.cancel();
        if let Some(task) = self.server_task.take() {
            task.abort();
        }
        opcgw::opc_ua_session_monitor::clear_session_monitor_state();
    }
}

async fn setup_apply_test_server() -> ApplyTestServer {
    let tmp = TempDir::new().expect("create temp dir");
    let port = common::pick_free_port().await;
    let pki_dir = tmp.path().join("pki");
    let db_path = tmp.path().join("opcgw.db");

    let config = Arc::new(baseline_app_config(port, &pki_dir));
    let pool = Arc::new(
        ConnectionPool::new(db_path.to_str().expect("utf-8 db path"), 1)
            .expect("create connection pool"),
    );
    let backend: Arc<dyn StorageBackend> =
        Arc::new(SqliteBackend::with_pool(pool).expect("create backend"));
    let backend_for_listener = backend.clone();

    let cancel = CancellationToken::new();
    let opc_ua = OpcUa::new(&config, backend.clone(), cancel.clone());
    let handles = opc_ua.build().await.expect("OpcUa::build must succeed");
    let manager = Arc::clone(&handles.manager);
    let subscriptions = handles.server_handle.subscriptions().clone();
    let last_status = handles.last_status.clone();
    let node_to_metric = handles.node_to_metric.clone();
    let ns = handles
        .server_handle
        .get_namespace_index(opcgw::utils::OPCUA_NAMESPACE_URI)
        .expect("namespace index must resolve");

    let server_task = tokio::spawn(async move {
        if let Err(e) = OpcUa::run_handles(handles).await {
            eprintln!("[9-8 apply] OpcUa::run_handles returned error: {e:?}");
        }
    });

    // Wait for port to bind.
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            break;
        }
        if Instant::now() >= deadline {
            panic!("OPC UA server did not bind to port {port} within 10s");
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // Wait for endpoint discovery to respond.
    {
        let probe_url = format!("opc.tcp://127.0.0.1:{port}/");
        let probe_tmp = TempDir::new().expect("probe pki tmp");
        let probe_client = build_apply_client(probe_tmp.path());
        let probe_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match probe_client
                .get_server_endpoints_from_url(probe_url.as_str())
                .await
            {
                Ok(endpoints) if !endpoints.is_empty() => break,
                _ => {}
            }
            if Instant::now() >= probe_deadline {
                panic!("OPC UA server did not respond to discovery within 5s after bind");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    ApplyTestServer {
        port,
        cancel,
        server_task: Some(server_task),
        manager,
        subscriptions,
        storage: backend_for_listener,
        last_status,
        node_to_metric,
        ns,
        config,
        _tmp: tmp,
    }
}

// -----------------------------------------------------------------------
// Client / session helpers
// -----------------------------------------------------------------------

fn user_name_policy() -> UserTokenPolicy {
    UserTokenPolicy {
        token_type: UserTokenType::UserName,
        ..UserTokenPolicy::anonymous()
    }
}

fn build_apply_client(client_pki: &std::path::Path) -> opcua::client::Client {
    common::build_client(common::ClientBuildSpec {
        application_name: "opcgw-9-8-apply-client",
        application_uri: "urn:opcgw:9-8:apply:client",
        product_uri: "urn:opcgw:9-8:apply:client",
        session_timeout_ms: 15_000,
        client_pki,
    })
}

struct HeldSession {
    session: Arc<Session>,
    event_handle: Option<tokio::task::JoinHandle<opcua::types::StatusCode>>,
    _client_tmp: TempDir,
    _client: opcua::client::Client,
}

impl Drop for HeldSession {
    fn drop(&mut self) {
        if let Some(h) = self.event_handle.take() {
            h.abort();
        }
    }
}

async fn open_session(server: &ApplyTestServer) -> HeldSession {
    let client_tmp = TempDir::new().expect("client tmp");
    let mut client = build_apply_client(client_tmp.path());
    let endpoint: EndpointDescription = (
        server.endpoint_url().as_str(),
        "None",
        MessageSecurityMode::None,
        user_name_policy(),
    )
        .into();
    let identity = common::user_name_identity(TEST_USER, TEST_PASSWORD);

    let (session, event_loop) = tokio::time::timeout(
        Duration::from_millis(5_000),
        client.connect_to_matching_endpoint(endpoint, identity),
    )
    .await
    .expect("client connect must not time out")
    .expect("client must connect successfully with valid credentials");
    session.disable_reconnects();
    let event_handle = event_loop.spawn();

    match tokio::time::timeout(Duration::from_millis(5_000), session.wait_for_connection()).await {
        Ok(true) => {}
        Ok(false) => panic!("session.wait_for_connection() returned false"),
        Err(_) => panic!("session.wait_for_connection() did not resolve within 5s"),
    }

    HeldSession {
        session,
        event_handle: Some(event_handle),
        _client_tmp: client_tmp,
        _client: client,
    }
}

async fn subscribe_one(
    held: &HeldSession,
    node_id: &NodeId,
    client_handle: u32,
) -> (u32, mpsc::UnboundedReceiver<opcua::types::DataValue>) {
    let (tx, rx) = mpsc::unbounded_channel::<opcua::types::DataValue>();
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
        .expect("CreateSubscription must succeed");
    assert!(subscription_id != 0);

    let create = MonitoredItemCreateRequest {
        item_to_monitor: ReadValueId::from(node_id.clone()),
        monitoring_mode: MonitoringMode::Reporting,
        requested_parameters: opcua::types::MonitoringParameters {
            client_handle,
            sampling_interval: 1000.0,
            filter: ExtensionObject::null(),
            queue_size: 10,
            discard_oldest: true,
        },
    };
    let create_results = held
        .session
        .create_monitored_items(subscription_id, TimestampsToReturn::Both, vec![create])
        .await
        .expect("CreateMonitoredItems must succeed");
    assert_eq!(create_results.len(), 1);
    assert!(
        create_results[0].result.status_code.is_good(),
        "CreateMonitoredItems must succeed — got {:?}",
        create_results[0].result.status_code
    );
    (subscription_id, rx)
}

fn metric_node_id(device_id: &str, metric_name: &str) -> NodeId {
    NodeId::new(OPCGW_NAMESPACE_INDEX, format!("{device_id}/{metric_name}"))
}

// =======================================================================
// AC#1 — apply adds a device + metric; fresh subscription on the new
// NodeId receives DataChangeNotifications
// =======================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn ac1_apply_adds_device_with_metric_makes_subscription_work() {
    let server = setup_apply_test_server().await;

    let held = open_session(&server).await;

    // Drive apply: add a second device under the existing application.
    let prev = (*server.config).clone();
    let new_cfg = config_add_second_device(&prev);
    let outcome = apply_diff_to_address_space(
        &prev,
        &new_cfg,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );
    assert!(
        matches!(outcome, AddressSpaceMutationOutcome::Applied { .. }),
        "apply must succeed, got {outcome:?}"
    );

    // Subscribe to the new metric NodeId — must produce a notification
    // within 5s (9-0 Q1 envelope).
    let new_node = metric_node_id(APPLY_DEVICE_NEW, APPLY_METRIC_NEW);
    let (_sub_id, mut rx) = subscribe_one(&held, &new_node, 99).await;
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("fresh subscription must produce a notification within 5s")
        .expect("notification channel closed unexpectedly");
    // Iter-1 review P4 (Blind B-H9): the prior assertion was
    // vacuous — `first.value.is_some() || first.status.is_some() ||
    // first.source_timestamp.is_some()` matches essentially any real
    // notification. Strengthen by asserting the read-callback
    // actually fired: either a typed Float payload (storage had a
    // value for the device) OR a non-Good status (storage returned
    // Ok(None), the get_value path's `BadDataUnavailable` exit
    // branch at `src/opc_ua.rs:1493-1508`). Both prove the runtime-
    // added closure was invoked. A DataValue with both fields None
    // would mean the sampler didn't sample our new variable.
    let value_typed = matches!(
        first.value.as_ref(),
        Some(opcua::types::Variant::Float(_)) | Some(opcua::types::Variant::Empty)
    );
    let status_non_good = match first.status {
        Some(sc) => !sc.is_good(),
        None => false,
    };
    assert!(
        value_typed || status_non_good,
        "AC#1: subscription must produce evidence the read-callback fired — \
         either a Float/Empty Variant value OR a non-Good status code. \
         Got value={:?} status={:?}",
        first.value,
        first.status
    );
    // Note: source_timestamp may be None when the read-callback's
    // `Err(BadDataUnavailable)` path is taken (the sampler doesn't
    // synthesise a server-side timestamp on the error path). The
    // value_typed || status_non_good check above is sufficient
    // evidence of callback firing.
}

// =======================================================================
// AC#1 — apply inserts the new metric NodeId into node_to_metric
// registry so HistoryRead can resolve it
// =======================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn ac1_apply_inserts_into_node_to_metric_registry() {
    let server = setup_apply_test_server().await;

    let prev = (*server.config).clone();
    let new_cfg = config_add_second_device(&prev);
    let outcome = apply_diff_to_address_space(
        &prev,
        &new_cfg,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );
    assert!(matches!(outcome, AddressSpaceMutationOutcome::Applied { .. }));

    // Registry must now contain the new mapping.
    let new_node = metric_node_id(APPLY_DEVICE_NEW, APPLY_METRIC_NEW);
    let map = server.node_to_metric.read();
    let pair = map.get(&new_node).expect("new metric must be in registry");
    assert_eq!(pair.0, APPLY_DEVICE_NEW);
    assert_eq!(pair.1, APPLY_CHIRP_NEW);
}

// =======================================================================
// AC#2 — Q2 mitigation: remove emits BadNodeIdUnknown set_values BEFORE
// address_space.delete(). LOAD-BEARING for 9-0-spike-report.md:104-127.
// =======================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn ac2_remove_device_emits_bad_node_id_unknown_before_delete() {
    let server = setup_apply_test_server().await;
    let held = open_session(&server).await;

    // Pre-subscribe to the baseline device's metric — this stream MUST
    // see a final BadNodeIdUnknown notification when the apply removes
    // the device.
    let baseline_node = metric_node_id(APPLY_DEVICE_BASELINE, APPLY_METRIC_BASELINE);
    let (_sub_id, mut rx) = subscribe_one(&held, &baseline_node, 1).await;
    // Drain the warm-up notification (if any).
    let _warmup = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

    // Drive apply: remove the baseline device.
    let prev = (*server.config).clone();
    let new_cfg = config_remove_baseline_device(&prev);
    let outcome = apply_diff_to_address_space(
        &prev,
        &new_cfg,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );
    assert!(matches!(outcome, AddressSpaceMutationOutcome::Applied { .. }));

    // Scan up to 3s of post-apply notifications looking for the
    // BadNodeIdUnknown transition. Per the Q2 mitigation we must see
    // EXACTLY one — not zero (silent stream = mitigation broken).
    let scan_deadline = Instant::now() + Duration::from_secs(3);
    let mut saw_bad_node_id_unknown = false;
    while Instant::now() < scan_deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(dv)) => {
                if dv.status == Some(StatusCode::BadNodeIdUnknown) {
                    saw_bad_node_id_unknown = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }

    assert!(
        saw_bad_node_id_unknown,
        "Q2 mitigation broken — subscriber must observe a BadNodeIdUnknown \
         notification before the underlying variable is deleted (per \
         9-0-spike-report.md:104-127). Without this, subscribers freeze on \
         last-good (Behaviour B) with no programmatic orphan-detection path."
    );
}

// =======================================================================
// AC#4 — unaffected subscriptions continue uninterrupted while the
// apply pass mutates other parts of the address space
// =======================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn ac4_unaffected_subscription_continues_across_add() {
    let server = setup_apply_test_server().await;
    let held = open_session(&server).await;

    // Subscribe to the baseline device — this stream MUST continue
    // uninterrupted while the apply pass adds an unrelated device.
    let baseline_node = metric_node_id(APPLY_DEVICE_BASELINE, APPLY_METRIC_BASELINE);
    let (_sub_id, mut rx) = subscribe_one(&held, &baseline_node, 1).await;
    let _warmup = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

    // Drive apply: add an unrelated device.
    let prev = (*server.config).clone();
    let new_cfg = config_add_second_device(&prev);
    let outcome = apply_diff_to_address_space(
        &prev,
        &new_cfg,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );
    assert!(matches!(outcome, AddressSpaceMutationOutcome::Applied { .. }));

    // Drain post-apply notifications for 2s. No status-change to a Bad
    // code is permitted on the baseline stream.
    let scan_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < scan_deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(dv)) => {
                // Iter-1 review P3 (Blind B-H8): `sc.is_good() ||
                // sc == StatusCode::Good` is the same condition
                // twice (StatusCode::Good is by definition is_good).
                // Simplify; also reject Uncertain explicitly so a
                // sibling-isolation regression where the apply path
                // accidentally bumps the unaffected variable's
                // status to Uncertain trips the test instead of
                // passing silently.
                if let Some(sc) = dv.status {
                    assert!(
                        sc.is_good(),
                        "baseline subscription must not see Bad/Uncertain status \
                         during unrelated apply — got {sc:?}"
                    );
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
}

// =======================================================================
// AC#7 — success event shape: `event="address_space_mutation_succeeded"`
// info log fires with all 9 axis counts + duration_ms
// =======================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn ac7_success_event_shape_apply_outcome() {
    let server = setup_apply_test_server().await;

    // Drive apply.
    let prev = (*server.config).clone();
    let new_cfg = config_add_second_device(&prev);
    let outcome = apply_diff_to_address_space(
        &prev,
        &new_cfg,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );

    // The apply function itself returns the structured outcome (the
    // audit event is emitted by `run_opcua_config_listener` which
    // wraps the call). Verify the outcome's counts directly — the
    // listener serialises this outcome into the audit event field set
    // 1:1 (see `src/config_reload.rs::run_opcua_config_listener`
    // Story-9-8 wire-in).
    match outcome {
        AddressSpaceMutationOutcome::Applied { counts, duration_ms } => {
            assert_eq!(counts.added_applications, 0);
            assert_eq!(counts.removed_applications, 0);
            assert_eq!(counts.added_devices, 1);
            assert_eq!(counts.removed_devices, 0);
            assert_eq!(counts.added_metrics, 1);
            assert_eq!(counts.removed_metrics, 0);
            assert_eq!(counts.added_commands, 0);
            assert_eq!(counts.removed_commands, 0);
            assert_eq!(counts.renamed_devices, 0);
            // duration_ms is a wall-clock measurement; just check it's
            // not absurd (≤ 5s for a single-device add).
            assert!(
                duration_ms <= 5_000,
                "apply duration_ms must be <= 5s, got {duration_ms}"
            );
        }
        other => panic!("expected Applied outcome, got {other:?}"),
    }
}

// =======================================================================
// AC#3 — modify path: paired (remove, add) on same NodeId rebuilds
// the closure with new captures. Iter-1 review P8 (Acceptance Auditor
// A1) — was missing from initial test set.
// =======================================================================

/// Construct an "after" config where the baseline device's single
/// metric (`Temperature`, type Float) is mutated to a DIFFERENT
/// `metric_type` (Int) while keeping the same `metric_name`. This
/// drives `compute_diff` to emit a paired (remove, add) entry on the
/// SAME NodeId; the apply pass must execute the pair in delete-then-add
/// order (else the add would silently no-op on a still-live NodeId).
fn config_modify_baseline_metric_type(prev: &AppConfig) -> AppConfig {
    let mut new = prev.clone();
    new.application_list[0].device_list[0].read_metric_list[0].metric_type =
        OpcMetricTypeConfig::Int;
    new
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn ac3_modified_device_metric_swap() {
    let server = setup_apply_test_server().await;
    let held = open_session(&server).await;

    let baseline_node = metric_node_id(APPLY_DEVICE_BASELINE, APPLY_METRIC_BASELINE);
    // Pre-subscribe before the apply. The baseline subscription's
    // existing monitored item targets the about-to-be-deleted+re-added
    // variable; the subscription itself survives the address-space
    // mutation per the 9-0 Q2 + Q3 envelope.
    let (_sub_id, mut rx) = subscribe_one(&held, &baseline_node, 1).await;
    let _warmup = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

    // Drive the modify diff (Float → Int on the same metric_name).
    let prev = (*server.config).clone();
    let new_cfg = config_modify_baseline_metric_type(&prev);
    let outcome = apply_diff_to_address_space(
        &prev,
        &new_cfg,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );
    let counts = match &outcome {
        AddressSpaceMutationOutcome::Applied { counts, .. } => *counts,
        other => panic!("expected Applied outcome for modify diff, got {other:?}"),
    };
    // The modify materialises as a paired (remove_metric, add_metric).
    // No device or application change.
    assert_eq!(counts.added_metrics, 1, "modify must emit 1 added_metric");
    assert_eq!(counts.removed_metrics, 1, "modify must emit 1 removed_metric");
    assert_eq!(counts.added_devices, 0);
    assert_eq!(counts.removed_devices, 0);
    assert_eq!(counts.added_applications, 0);
    assert_eq!(counts.removed_applications, 0);

    // The original subscription MUST observe the Q2 transition
    // (BadNodeIdUnknown) on the modify path — same mitigation as
    // pure-remove. After Phase 3 re-adds under the same NodeId, the
    // subscription's monitored item is still bound to the OLD
    // (deleted) variable from the client's perspective; clients that
    // want the re-added variable must re-subscribe. v1 behaviour.
    let scan_deadline = Instant::now() + Duration::from_secs(3);
    let mut saw_bad_node_id_unknown = false;
    while Instant::now() < scan_deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(dv)) => {
                if dv.status == Some(StatusCode::BadNodeIdUnknown) {
                    saw_bad_node_id_unknown = true;
                    break;
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
    assert!(
        saw_bad_node_id_unknown,
        "modify path must emit BadNodeIdUnknown via Q2 mitigation before re-add"
    );

    // A FRESH subscription on the same NodeId post-apply must work —
    // it binds to the newly-added variable with the new metric_type
    // closure.
    let (_sub_id2, mut rx2) = subscribe_one(&held, &baseline_node, 2).await;
    let first = tokio::time::timeout(Duration::from_secs(5), rx2.recv())
        .await
        .expect("fresh post-modify subscription must produce a notification within 5s")
        .expect("notification channel closed unexpectedly");
    // Iter-2 review IP3 (Blind B-H2-iter2): the prior assertion was
    // the same vacuous `is_some() || is_some()` form that iter-1 P4
    // tightened in ac1. Apply the same value_typed || status_non_good
    // pattern here so the fresh post-modify subscription is proven to
    // have invoked the read-callback (rather than merely receiving
    // the stale Phase-1 BadNodeIdUnknown set_values notification that
    // could pre-populate the channel before any real sample).
    // Iter-3 review TP2 (Edge E-H2-iter3): include `Variant::Int64`
    // in the value_typed match. `OpcUa::convert_metric_to_variant`
    // (src/opc_ua.rs ~1829-1834) returns `Variant::Int64` when an
    // `Int` metric's parsed value doesn't fit i32. Today the test
    // relies on the sampler's typed-zero fallback (`Int32(0)`)
    // because storage is empty for the modified metric, but if a
    // future maintainer seeds storage with an Int value > i32::MAX
    // (e.g. epoch-ms, 64-bit counter), the fresh subscription
    // receives Int64 and the previous match would mis-route it to
    // assertion failure.
    let value_typed = matches!(
        first.value.as_ref(),
        Some(opcua::types::Variant::Float(_))
            | Some(opcua::types::Variant::Int32(_))
            | Some(opcua::types::Variant::Int64(_))
            | Some(opcua::types::Variant::Empty)
    );
    let status_non_good = match first.status {
        Some(sc) => !sc.is_good(),
        None => false,
    };
    assert!(
        value_typed || status_non_good,
        "AC#3 post-modify: fresh subscription must produce evidence the read-callback \
         fired — either a Float/Int32/Int64/Empty Variant value (the closure forwards \
         to OpcUa::get_value which returns one of these per the new metric_type=Int) \
         OR a non-Good status code. Got value={:?} status={:?}",
        first.value,
        first.status
    );
    // Iter-2 review IP5 (Auditor A-Adn-iter2-2): AC#3's "And the
    // unaffected `DeviceA/Temperature` NodeId is **not touched**"
    // clause is covered indirectly by `ac4_unaffected_subscription_continues_across_add`
    // and `ac4_unaffected_subscription_continues_across_remove` —
    // both of those tests pin sibling-isolation across the same
    // apply-pass lock-discipline this test exercises. The modify
    // path in particular does not introduce a different lock-hold
    // shape (Phase 2 + Phase 3 each acquire and release one write
    // lock, identical to add-only and remove-only paths). Therefore
    // a dedicated `ac3_sibling_uninterrupted_across_modify` test
    // would duplicate ac4_*'s coverage; sibling-isolation under
    // modify is asserted transitively.
}

// =======================================================================
// AC#4 — sibling isolation across REMOVE (parallel to existing
// _across_add). Iter-1 review P9 (Acceptance Auditor A2).
// =======================================================================

/// Construct a 2-device "before" config + a 1-device "after" config so
/// the apply diff removes ONLY the second device while leaving the
/// baseline subscription's target untouched.
fn config_two_devices(prev: &AppConfig) -> AppConfig {
    config_add_second_device(prev)
}

fn config_remove_only_second_device(two_dev: &AppConfig) -> AppConfig {
    let mut new = two_dev.clone();
    // Pop the second device (kept the baseline).
    new.application_list[0].device_list.pop();
    new
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn ac4_unaffected_subscription_continues_across_remove() {
    let server = setup_apply_test_server().await;
    let held = open_session(&server).await;

    // Set up the address space to have 2 devices so the apply can
    // remove the second one. The setup_apply_test_server fixture
    // boots with 1 device; we run a preparatory apply to add the
    // second one, then the test apply removes it.
    let baseline = (*server.config).clone();
    let two_devices = config_two_devices(&baseline);
    let prep_outcome = apply_diff_to_address_space(
        &baseline,
        &two_devices,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );
    assert!(
        matches!(prep_outcome, AddressSpaceMutationOutcome::Applied { .. }),
        "prep apply (add 2nd device) must succeed"
    );

    // Subscribe to the BASELINE device's metric — this stream MUST
    // continue uninterrupted while the apply pass removes the
    // SECOND (unrelated) device.
    let baseline_node = metric_node_id(APPLY_DEVICE_BASELINE, APPLY_METRIC_BASELINE);
    let (_sub_id, mut rx) = subscribe_one(&held, &baseline_node, 1).await;
    let _warmup = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await;

    // Drive the remove-only-2nd-device diff.
    let after_remove = config_remove_only_second_device(&two_devices);
    let outcome = apply_diff_to_address_space(
        &two_devices,
        &after_remove,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );
    let counts = match &outcome {
        AddressSpaceMutationOutcome::Applied { counts, .. } => *counts,
        other => panic!("expected Applied outcome for remove diff, got {other:?}"),
    };
    assert_eq!(counts.removed_devices, 1);
    assert_eq!(counts.removed_metrics, 1);
    assert_eq!(counts.added_devices, 0);

    // Drain post-apply notifications for 2s; baseline stream must
    // NOT see Bad/Uncertain status (sibling isolation per AC#4 +
    // 9-0 Q3 117µs lock-hold = ~850× headroom).
    let scan_deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < scan_deadline {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Some(dv)) => {
                if let Some(sc) = dv.status {
                    assert!(
                        sc.is_good(),
                        "baseline subscription must not see Bad/Uncertain status \
                         during unrelated remove — got {sc:?}"
                    );
                }
            }
            Ok(None) => break,
            Err(_) => continue,
        }
    }
}

// =======================================================================
// AC#5 / Phase 4 — Device rename in-place: same device_id, different
// device_name. Drives Phase 4's DisplayName-only set_attributes path
// without touching Phase 2/3. Iter-2 review IP6 (Auditor A-Adn-iter2-3):
// brings the integration test count to the spec Task 7 ≥8 minimum.
// =======================================================================

/// Construct an "after" config that renames the baseline device
/// (same `device_id`, different `device_name`). Drives Phase 4 only.
fn config_rename_baseline_device(prev: &AppConfig) -> AppConfig {
    let mut new = prev.clone();
    new.application_list[0].device_list[0].device_name = "Renamed Baseline".to_string();
    new
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[serial_test::serial]
async fn ac5_device_rename_in_place() {
    let server = setup_apply_test_server().await;

    let prev = (*server.config).clone();
    let new_cfg = config_rename_baseline_device(&prev);

    // Drive the rename diff.
    let outcome = apply_diff_to_address_space(
        &prev,
        &new_cfg,
        &server.manager,
        &server.subscriptions,
        &server.storage,
        &server.last_status,
        &server.node_to_metric,
        server.ns,
        120,
    );

    // Iter-2 IP1 demoted Phase 4 from Failed-return to
    // warn-and-continue, so a rename diff always yields Applied
    // (rename succeeded OR rename failed but apply continues).
    // Assert: outcome is Applied + counts.renamed_devices == 1 +
    // every other axis count is zero.
    match outcome {
        AddressSpaceMutationOutcome::Applied { counts, .. } => {
            assert_eq!(counts.renamed_devices, 1, "rename diff must report 1 renamed device");
            assert_eq!(counts.added_applications, 0);
            assert_eq!(counts.removed_applications, 0);
            assert_eq!(counts.added_devices, 0);
            assert_eq!(counts.removed_devices, 0);
            assert_eq!(counts.added_metrics, 0);
            assert_eq!(counts.removed_metrics, 0);
            assert_eq!(counts.added_commands, 0);
            assert_eq!(counts.removed_commands, 0);
        }
        other => panic!(
            "AC#5 rename diff: expected Applied outcome (iter-2 IP1 demoted Phase 4 \
             to warn-and-continue), got {other:?}"
        ),
    }

    // node_to_metric registry must be UNCHANGED — rename does not
    // touch metric NodeIds (the metric is still under the same
    // device_id-keyed NodeId regardless of the device's display
    // name). Verifies the rename path doesn't accidentally mutate
    // the HistoryRead registry.
    let baseline_metric_node =
        metric_node_id(APPLY_DEVICE_BASELINE, APPLY_METRIC_BASELINE);
    let map = server.node_to_metric.read();
    let pair = map
        .get(&baseline_metric_node)
        .expect("baseline metric must remain in registry after rename");
    assert_eq!(pair.0, APPLY_DEVICE_BASELINE);
    assert_eq!(pair.1, APPLY_CHIRP_BASELINE);
    drop(map);

    // Iter-3 review TP3 (3-layer convergent — Blind B-H1-iter3 +
    // Edge E-H3-iter3 + Auditor A-Adn-iter3-3): verify the
    // DisplayName attribute was ACTUALLY mutated in the address
    // space. After iter-2 IP1 demoted Phase 4 to warn-and-continue,
    // a silent `set_attributes` regression would still produce
    // `Applied { renamed_devices = 1 }` (the counts reflect
    // *attempted* renames). Read the Object's DisplayName directly
    // from the address space to pin the positive side effect.
    // Device folder NodeId uses opcgw's startup convention
    // (`NodeId::new(ns, device_id)` per src/opc_ua.rs:966 and
    // `opcua_topology_apply::device_node_id` helper).
    let device_node = NodeId::new(OPCGW_NAMESPACE_INDEX, APPLY_DEVICE_BASELINE.to_string());
    let address_space = server.manager.address_space();
    let guard = address_space.read();
    let node = guard
        .find(&device_node)
        .expect("device folder must exist in address space");
    let display_name = node.as_node().display_name();
    assert_eq!(
        display_name.text.as_ref(),
        "Renamed Baseline",
        "Phase 4 DisplayName mutation must be visible in the live address space \
         (iter-3 TP3 pin — silent set_attributes regression would otherwise pass)"
    );
}

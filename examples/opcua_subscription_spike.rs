// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 8-1 reference spike — NOT production code. Do not import any
// item from this binary. Story 8-2 will introduce production
// subscription support; this binary is a developer test tool kept
// in `examples/` per CLAUDE.md scope-discipline rule.
//
// Two modes:
//
//   `cargo run --example opcua_subscription_spike -- --plan-a`
//     Connects to a locally-running gateway, creates ONE subscription
//     with ONE monitored item on `Temperature`, waits up to 5 s for
//     a `DataChangeNotification`, prints a JSON-on-stderr summary.
//     Default mode. ~5 s wall clock.
//
//   `cargo run --example opcua_subscription_spike -- --load-probe`
//     The AC#8 throughput probe: 100 monitored items × 1 Hz × 5 min
//     against a synthetic node-id list (clamped to whatever the
//     gateway's address space exposes). Captures notification count,
//     latency p50/p95/p99, drop count. Opt-in only — long-running.
//
// Both modes assume:
//   - The gateway is running locally (default `opc.tcp://127.0.0.1:4855/`).
//   - The user/password match the gateway's `[opcua].user_name` /
//     `[opcua].user_password` (typically `opcua-user` / supplied via
//     `OPCGW_OPCUA__USER_PASSWORD` env var).
//
// JSON output goes to stderr so test scripts can grep `^{` from a
// captured log file. stdout reserved for human-readable progress.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::{Parser, ValueEnum};
use opcua::client::{
    ClientBuilder, DataChangeCallback, IdentityToken, Password as ClientPassword,
};
use opcua::types::{
    EndpointDescription, MessageSecurityMode, MonitoredItemCreateRequest, MonitoringMode, NodeId,
    ReadValueId, TimestampsToReturn, UserTokenPolicy, UserTokenType,
};

#[derive(Parser, Debug)]
#[command(version, about = "OPC UA subscription spike (Story 8-1)")]
struct Args {
    /// Spike mode.
    #[arg(long, value_enum, default_value_t = Mode::PlanA)]
    mode: Mode,

    /// Plan A — minimal subscription confirmation. Default.
    #[arg(long, conflicts_with_all = ["load_probe", "mode"])]
    plan_a: bool,

    /// Load probe — 100 items × 1 Hz × 5 min. Opt-in.
    #[arg(long, conflicts_with_all = ["plan_a", "mode"])]
    load_probe: bool,

    /// Server URL.
    #[arg(long, default_value = "opc.tcp://127.0.0.1:4855/")]
    url: String,

    /// Username.
    #[arg(long, default_value = "opcua-user")]
    user: String,

    /// Password. Defaults to `OPCGW_OPCUA__USER_PASSWORD` env var if
    /// not given on the CLI. (clap's `env` attribute is gated behind
    /// the `env` feature, which we don't enable to keep production
    /// dep flags untouched — manual fallback below.)
    #[arg(long)]
    password: Option<String>,

    /// Client PKI directory (auto-created).
    #[arg(long, default_value = "./pki-spike-8-1-client")]
    pki_dir: PathBuf,

    /// Namespace index for the gateway's metric nodes. Default 2 —
    /// async-opcua's first user-supplied namespace under
    /// SimpleNodeManager.
    #[arg(long, default_value_t = 2)]
    namespace: u16,

    /// Single-monitored-item NodeId for `--plan-a`. Must exist in the
    /// gateway's address space.
    #[arg(long, default_value = "Temperature")]
    plan_a_node: String,

    /// Number of monitored items for `--load-probe`. AC#8 target: 100.
    #[arg(long, default_value_t = 100)]
    load_items: usize,

    /// Probe duration (seconds) for `--load-probe`. AC#8 target: 300 (5 min).
    #[arg(long, default_value_t = 300)]
    load_secs: u64,

    /// Publishing interval (ms) for `--load-probe`. AC#8 target: 1000 (1 Hz).
    #[arg(long, default_value_t = 1000)]
    load_publish_ms: u64,

    /// Comma-separated NodeId names for `--load-probe`. If fewer than
    /// `--load-items`, names cycle. Default reuses the
    /// `tests/config/config.toml` fixture's `Temperature` plus
    /// `Application01` / `Application02` / etc. — caller should
    /// supply `--load-nodes "Metric01,Metric02,..."` matching their
    /// running gateway's actual address space.
    #[arg(long, default_value = "Temperature")]
    load_nodes: String,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum Mode {
    PlanA,
    LoadProbe,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> std::process::ExitCode {
    let mut args = Args::parse();

    if args.password.is_none() {
        args.password = std::env::var("OPCGW_OPCUA__USER_PASSWORD").ok();
    }
    if args.password.is_none() {
        eprintln!("spike: --password required (or set OPCGW_OPCUA__USER_PASSWORD env var)");
        return std::process::ExitCode::from(2);
    }

    let mode = if args.load_probe {
        Mode::LoadProbe
    } else if args.plan_a {
        Mode::PlanA
    } else {
        args.mode
    };

    println!(
        "spike: mode={:?} url={} user={} namespace={}",
        mode, args.url, args.user, args.namespace
    );

    match mode {
        Mode::PlanA => run_plan_a(&args).await,
        Mode::LoadProbe => run_load_probe(&args).await,
    }
}

/// Build a temporary OPC UA client. Mirrors the test harness shape;
/// `verify_server_certs(false)` + `trust_server_certs(true)` keeps the
/// spike usable against a sample-keypair gateway without manual cert
/// import.
fn build_client(args: &Args) -> Result<opcua::client::Client, String> {
    ClientBuilder::new()
        .application_name("opcgw-spike-8-1")
        .application_uri("urn:opcgw:spike:8-1:client")
        .product_uri("urn:opcgw:spike:8-1:client")
        .create_sample_keypair(true)
        .trust_server_certs(true)
        .verify_server_certs(false)
        .session_retry_limit(0)
        .session_timeout(60_000)
        .pki_dir(&args.pki_dir)
        .client()
        .map_err(|e| format!("client build failed: {e:?}"))
}

async fn run_plan_a(args: &Args) -> std::process::ExitCode {
    println!("plan-a: building client + connecting");
    let mut client = match build_client(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("plan-a: {e}");
            emit_summary(&PlanASummary {
                ok: false,
                error: Some(e),
                first_notification_latency_ms: None,
                publish_count: 0,
            });
            return std::process::ExitCode::from(2);
        }
    };

    let endpoint: EndpointDescription = (
        args.url.as_str(),
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy {
            token_type: UserTokenType::UserName,
            ..UserTokenPolicy::anonymous()
        },
    )
        .into();
    let identity = IdentityToken::UserName(args.user.clone(), ClientPassword(args.password.clone().unwrap_or_default()));

    let connect_result = match tokio::time::timeout(
        Duration::from_secs(15),
        client.connect_to_matching_endpoint(endpoint, identity),
    )
    .await
    {
        Ok(Ok(pair)) => pair,
        Ok(Err(e)) => {
            let msg = format!("connect failed: {e:?}");
            eprintln!("plan-a: {msg}");
            emit_summary(&PlanASummary {
                ok: false,
                error: Some(msg),
                first_notification_latency_ms: None,
                publish_count: 0,
            });
            return std::process::ExitCode::from(1);
        }
        Err(_) => {
            let msg = "connect timed out after 15 s".to_string();
            eprintln!("plan-a: {msg}");
            emit_summary(&PlanASummary {
                ok: false,
                error: Some(msg),
                first_notification_latency_ms: None,
                publish_count: 0,
            });
            return std::process::ExitCode::from(1);
        }
    };

    let (session, event_loop) = connect_result;
    session.disable_reconnects();
    let event_handle = event_loop.spawn();

    let connected = tokio::time::timeout(Duration::from_secs(10), session.wait_for_connection())
        .await
        .unwrap_or(false);
    if !connected {
        let msg = "session did NOT activate within 10 s — check log/opc_ua.log".to_string();
        eprintln!("plan-a: {msg}");
        let _ = session.disconnect().await;
        event_handle.abort();
        emit_summary(&PlanASummary {
            ok: false,
            error: Some(msg),
            first_notification_latency_ms: None,
            publish_count: 0,
        });
        return std::process::ExitCode::from(1);
    }
    println!("plan-a: session activated");

    let publish_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let first_notification = Arc::new(Mutex::new(None::<Instant>));

    let publish_count_cb = publish_count.clone();
    let first_notification_cb = first_notification.clone();
    let subscribe_started = Instant::now();

    let subscription_id = match session
        .create_subscription(
            Duration::from_millis(1000),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |_dv, _item| {
                let prev = publish_count_cb.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if prev == 0 {
                    if let Ok(mut guard) = first_notification_cb.lock() {
                        *guard = Some(Instant::now());
                    }
                }
            }),
        )
        .await
    {
        Ok(id) => id,
        Err(sc) => {
            let msg = format!("CreateSubscription failed: {sc:?}");
            eprintln!("plan-a: {msg}");
            let _ = session.disconnect().await;
            event_handle.abort();
            emit_summary(&PlanASummary {
                ok: false,
                error: Some(msg),
                first_notification_latency_ms: None,
                publish_count: 0,
            });
            return std::process::ExitCode::from(1);
        }
    };
    println!("plan-a: subscription_id={subscription_id}");

    let node_id = NodeId::new(args.namespace, args.plan_a_node.clone());
    let create = MonitoredItemCreateRequest {
        item_to_monitor: ReadValueId::from(node_id),
        monitoring_mode: MonitoringMode::Reporting,
        requested_parameters: opcua::types::MonitoringParameters {
            client_handle: 1,
            sampling_interval: 1000.0,
            filter: opcua::types::ExtensionObject::null(),
            queue_size: 10,
            discard_oldest: true,
        },
    };
    match session
        .create_monitored_items(subscription_id, TimestampsToReturn::Both, vec![create])
        .await
    {
        Ok(results) if !results.is_empty() && results[0].result.status_code.is_good() => {
            println!(
                "plan-a: monitored_item_id={} (status Good)",
                results[0].result.monitored_item_id
            );
        }
        Ok(results) => {
            let msg = format!(
                "CreateMonitoredItems returned non-Good: {:?}",
                results.first().map(|r| r.result.status_code)
            );
            eprintln!("plan-a: {msg}");
            let _ = session.disconnect().await;
            event_handle.abort();
            emit_summary(&PlanASummary {
                ok: false,
                error: Some(msg),
                first_notification_latency_ms: None,
                publish_count: 0,
            });
            return std::process::ExitCode::from(1);
        }
        Err(sc) => {
            let msg = format!("CreateMonitoredItems failed: {sc:?}");
            eprintln!("plan-a: {msg}");
            let _ = session.disconnect().await;
            event_handle.abort();
            emit_summary(&PlanASummary {
                ok: false,
                error: Some(msg),
                first_notification_latency_ms: None,
                publish_count: 0,
            });
            return std::process::ExitCode::from(1);
        }
    }

    println!("plan-a: waiting up to 5 s for first DataChangeNotification");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if first_notification.lock().unwrap().is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let latency_ms = first_notification
        .lock()
        .unwrap()
        .map(|t| t.duration_since(subscribe_started).as_millis() as u64);
    let count = publish_count.load(std::sync::atomic::Ordering::Relaxed);

    let _ = session.delete_subscription(subscription_id).await;
    let _ = session.disconnect().await;
    event_handle.abort();
    let _ = event_handle.await;

    let ok = latency_ms.is_some();
    if ok {
        println!(
            "plan-a: PASS — first notification at {} ms, total received {}",
            latency_ms.unwrap_or(0),
            count
        );
    } else {
        println!("plan-a: FAIL — no notification within 5 s");
    }
    emit_summary(&PlanASummary {
        ok,
        error: if ok {
            None
        } else {
            Some("no DataChangeNotification within 5 s — Plan A failed; pivot to Plan B".to_string())
        },
        first_notification_latency_ms: latency_ms,
        publish_count: count,
    });

    if ok {
        std::process::ExitCode::from(0)
    } else {
        std::process::ExitCode::from(1)
    }
}

async fn run_load_probe(args: &Args) -> std::process::ExitCode {
    println!(
        "load-probe: items={} duration={} s publish_interval={} ms",
        args.load_items, args.load_secs, args.load_publish_ms
    );

    let node_names: Vec<String> = args
        .load_nodes
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if node_names.is_empty() {
        eprintln!("load-probe: --load-nodes must contain at least one node name");
        return std::process::ExitCode::from(2);
    }

    let mut client = match build_client(args) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("load-probe: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    let endpoint: EndpointDescription = (
        args.url.as_str(),
        "None",
        MessageSecurityMode::None,
        UserTokenPolicy {
            token_type: UserTokenType::UserName,
            ..UserTokenPolicy::anonymous()
        },
    )
        .into();
    let identity = IdentityToken::UserName(args.user.clone(), ClientPassword(args.password.clone().unwrap_or_default()));

    let connect_result =
        match tokio::time::timeout(Duration::from_secs(15), client.connect_to_matching_endpoint(endpoint, identity))
            .await
        {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                eprintln!("load-probe: connect failed: {e:?}");
                return std::process::ExitCode::from(1);
            }
            Err(_) => {
                eprintln!("load-probe: connect timed out");
                return std::process::ExitCode::from(1);
            }
        };
    let (session, event_loop) = connect_result;
    session.disable_reconnects();
    let event_handle = event_loop.spawn();

    let connected = tokio::time::timeout(Duration::from_secs(10), session.wait_for_connection())
        .await
        .unwrap_or(false);
    if !connected {
        eprintln!("load-probe: session did NOT activate");
        let _ = session.disconnect().await;
        event_handle.abort();
        return std::process::ExitCode::from(1);
    }
    println!("load-probe: session activated");

    // Per-monitored-item arrival timestamps (Mutex-guarded for the
    // callback's `&mut self` access pattern). Recording the full
    // timeline lets us compute p50/p95/p99 inter-notification interval
    // post-run without buffering DataValues themselves.
    let arrivals: Arc<Mutex<Vec<(u32, Instant)>>> = Arc::new(Mutex::new(Vec::with_capacity(
        args.load_items * (args.load_secs as usize / (args.load_publish_ms as usize / 1000).max(1)) + 1,
    )));
    let arrivals_cb = arrivals.clone();

    let subscription_id = match session
        .create_subscription(
            Duration::from_millis(args.load_publish_ms),
            300, // lifetime — long enough that the subscription doesn't time out at minute 5
            30,  // keep-alive
            0,
            0,
            true,
            DataChangeCallback::new(move |_dv, item| {
                if let Ok(mut guard) = arrivals_cb.lock() {
                    guard.push((item.client_handle(), Instant::now()));
                }
            }),
        )
        .await
    {
        Ok(id) => id,
        Err(sc) => {
            eprintln!("load-probe: CreateSubscription failed: {sc:?}");
            let _ = session.disconnect().await;
            event_handle.abort();
            return std::process::ExitCode::from(1);
        }
    };
    println!("load-probe: subscription_id={subscription_id}");

    // Build N MonitoredItemCreateRequests. NodeId names cycle through
    // the supplied `--load-nodes` list. `client_handle` is the per-
    // item identifier we use for arrival tracking.
    let create_requests: Vec<MonitoredItemCreateRequest> = (0..args.load_items)
        .map(|i| {
            let name = node_names[i % node_names.len()].clone();
            MonitoredItemCreateRequest {
                item_to_monitor: ReadValueId::from(NodeId::new(args.namespace, name)),
                monitoring_mode: MonitoringMode::Reporting,
                requested_parameters: opcua::types::MonitoringParameters {
                    client_handle: i as u32 + 1,
                    sampling_interval: args.load_publish_ms as f64,
                    filter: opcua::types::ExtensionObject::null(),
                    queue_size: 10,
                    discard_oldest: true,
                },
            }
        })
        .collect();

    let creation_started = Instant::now();
    let create_results = match session
        .create_monitored_items(subscription_id, TimestampsToReturn::Both, create_requests)
        .await
    {
        Ok(r) => r,
        Err(sc) => {
            eprintln!("load-probe: CreateMonitoredItems failed: {sc:?}");
            let _ = session.disconnect().await;
            event_handle.abort();
            return std::process::ExitCode::from(1);
        }
    };
    let good_count = create_results
        .iter()
        .filter(|r| r.result.status_code.is_good())
        .count();
    println!(
        "load-probe: created {} monitored items ({} Good) in {:?}",
        create_results.len(),
        good_count,
        creation_started.elapsed()
    );

    if good_count == 0 {
        eprintln!("load-probe: NO monitored items succeeded — aborting");
        let _ = session.delete_subscription(subscription_id).await;
        let _ = session.disconnect().await;
        event_handle.abort();
        return std::process::ExitCode::from(1);
    }

    let probe_started = Instant::now();
    let mut last_progress = Instant::now();
    while probe_started.elapsed() < Duration::from_secs(args.load_secs) {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if last_progress.elapsed() >= Duration::from_secs(30) {
            let n = arrivals.lock().unwrap().len();
            println!(
                "load-probe: t={:>4} s notifications_so_far={}",
                probe_started.elapsed().as_secs(),
                n
            );
            last_progress = Instant::now();
        }
    }
    let probe_ended = Instant::now();
    println!("load-probe: probe window closed at {:?}", probe_ended - probe_started);

    let _ = session.delete_subscription(subscription_id).await;
    let _ = session.disconnect().await;
    event_handle.abort();
    let _ = event_handle.await;

    // Compute summary metrics. Group arrivals by client_handle, then
    // for each item compute the inter-notification intervals across
    // the whole window.
    let arrivals = arrivals.lock().unwrap().clone();
    let mut per_item: BTreeMap<u32, Vec<Instant>> = BTreeMap::new();
    for (handle, t) in &arrivals {
        per_item.entry(*handle).or_default().push(*t);
    }
    let mut intervals_ms: Vec<u128> = Vec::with_capacity(arrivals.len());
    for ts in per_item.values() {
        for w in ts.windows(2) {
            intervals_ms.push(w[1].duration_since(w[0]).as_millis());
        }
    }
    intervals_ms.sort_unstable();

    let p = |q: f64| -> Option<u128> {
        if intervals_ms.is_empty() {
            return None;
        }
        let idx = ((intervals_ms.len() as f64 - 1.0) * q).round() as usize;
        Some(intervals_ms[idx])
    };

    let summary = LoadProbeSummary {
        items_requested: args.load_items,
        items_created_good: good_count,
        publish_interval_ms: args.load_publish_ms,
        duration_secs: args.load_secs,
        total_notifications: arrivals.len(),
        unique_items_with_arrivals: per_item.len(),
        median_interval_ms: p(0.50),
        p95_interval_ms: p(0.95),
        p99_interval_ms: p(0.99),
        max_interval_ms: intervals_ms.last().copied(),
        min_interval_ms: intervals_ms.first().copied(),
        per_item_arrival_counts: per_item.iter().map(|(h, v)| (*h, v.len())).collect(),
    };

    println!("load-probe: PASS — see JSON summary on stderr");
    emit_load_summary(&summary);

    std::process::ExitCode::from(0)
}

// -----------------------------------------------------------------------
// JSON-on-stderr summaries. Hand-written to avoid pulling serde_json into
// the spike binary's dep tree (the spike target is "no new deps" per
// CLAUDE.md scope discipline).
// -----------------------------------------------------------------------

struct PlanASummary {
    ok: bool,
    error: Option<String>,
    first_notification_latency_ms: Option<u64>,
    publish_count: u64,
}

fn emit_summary(s: &PlanASummary) {
    let err = match &s.error {
        Some(e) => format!("\"{}\"", e.replace('"', "\\\"")),
        None => "null".to_string(),
    };
    let latency = match s.first_notification_latency_ms {
        Some(v) => v.to_string(),
        None => "null".to_string(),
    };
    eprintln!(
        "{{\"plan\":\"A\",\"ok\":{},\"error\":{},\"first_notification_latency_ms\":{},\"publish_count\":{}}}",
        s.ok, err, latency, s.publish_count
    );
}

struct LoadProbeSummary {
    items_requested: usize,
    items_created_good: usize,
    publish_interval_ms: u64,
    duration_secs: u64,
    total_notifications: usize,
    unique_items_with_arrivals: usize,
    median_interval_ms: Option<u128>,
    p95_interval_ms: Option<u128>,
    p99_interval_ms: Option<u128>,
    max_interval_ms: Option<u128>,
    min_interval_ms: Option<u128>,
    per_item_arrival_counts: Vec<(u32, usize)>,
}

fn emit_load_summary(s: &LoadProbeSummary) {
    let opt = |v: Option<u128>| match v {
        Some(x) => x.to_string(),
        None => "null".to_string(),
    };
    let per_item: Vec<String> = s
        .per_item_arrival_counts
        .iter()
        .map(|(h, n)| format!("\"{h}\":{n}"))
        .collect();
    eprintln!(
        "{{\"plan\":\"load-probe\",\
         \"items_requested\":{},\
         \"items_created_good\":{},\
         \"publish_interval_ms\":{},\
         \"duration_secs\":{},\
         \"total_notifications\":{},\
         \"unique_items_with_arrivals\":{},\
         \"min_interval_ms\":{},\
         \"median_interval_ms\":{},\
         \"p95_interval_ms\":{},\
         \"p99_interval_ms\":{},\
         \"max_interval_ms\":{},\
         \"per_item_arrival_counts\":{{{}}}}}",
        s.items_requested,
        s.items_created_good,
        s.publish_interval_ms,
        s.duration_secs,
        s.total_notifications,
        s.unique_items_with_arrivals,
        opt(s.min_interval_ms),
        opt(s.median_interval_ms),
        opt(s.p95_interval_ms),
        opt(s.p99_interval_ms),
        opt(s.max_interval_ms),
        per_item.join(","),
    );
}

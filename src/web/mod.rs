// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Embedded Axum web server (Story 9-1).
//!
//! Hosts the gateway's web UI on a configurable HTTP port sharing the
//! same Tokio runtime as the OPC UA server and ChirpStack poller. The
//! server is **opt-in** via `[web].enabled = true` (default `false`)
//! so existing operators upgrading from Phase A don't get an
//! unexpected new listening port.
//!
//! # Surface
//!
//! - Single global Basic-auth middleware ([`auth::basic_auth_middleware`])
//!   wrapping every route. Credentials are shared with the OPC UA
//!   surface (`[opcua].user_name` / `[opcua].user_password`).
//! - Static files served from the `static/` directory via
//!   `tower-http::services::ServeDir`. Stories 9-2 / 9-3 / 9-4 / 9-5 / 9-6
//!   replace the placeholder HTML with real content.
//! - `GET /api/health` — minimal smoke endpoint returning
//!   `{"status":"ok"}`. Used by integration tests to verify the auth
//!   middleware fires without depending on any static-file fixture.
//!
//! # Lifecycle
//!
//! Bind happens synchronously in `main` (Story 9-1 review iter-1 D1
//! resolution: fail-fast at startup if the configured port is taken
//! or the bind address is invalid — consistent with Story 7-2's
//! fail-closed pattern). The pre-bound `TcpListener` is then handed
//! to [`run`] which `tokio::spawn`s the serve loop and joins the
//! existing `CancellationToken`-driven shutdown sequence in
//! `src/main.rs`.

pub mod api;
pub mod auth;
pub mod config_writer;
pub mod csrf;

// Iter-1 review P30: `test_support` ships in release binaries
// because integration tests in `tests/` cannot see `#[cfg(test)]`
// items (they link against the production lib build). Gating on a
// feature flag would require every integration test to enable it
// per-target, which Cargo cannot express cleanly without a
// circular self-dev-dep. The module is marked `#![allow(dead_code)]`
// and is never reachable from non-test consumers; the production
// binary's size cost is negligible (~150 LOC of TOML fixture
// strings).
pub mod test_support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
// Story 9-4: `post`, `put`, `delete` are used as `MethodRouter`
// builders chained after `get(...)` via `.post()/.put()/.delete()`
// in `build_router`; no separate import is needed.
use axum::Router;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::storage::StorageBackend;
use crate::utils::OpcGwError;
use auth::{basic_auth_middleware, WebAuthState};

/// Per-application summary for the dashboard — application identity
/// plus how many devices are configured under it. Story 9-3 extends
/// this with the actual per-device list so the live-metrics page can
/// walk the topology without re-reading `AppConfig.application_list`.
#[derive(Clone, Debug, PartialEq)]
pub struct ApplicationSummary {
    pub application_id: String,
    pub application_name: String,
    /// Number of configured devices. Equals `devices.len()` by
    /// construction; kept as its own field so `/api/status` (Story
    /// 9-2) doesn't have to walk `devices` to compute the count.
    pub device_count: usize,
    /// Per-device summary in TOML-declaration order so the dashboard
    /// renders devices in a stable, operator-controlled sequence
    /// rather than HashMap iteration order. Story 9-3 addition.
    pub devices: Vec<DeviceSummary>,
}

/// Per-device summary for the live-metrics page (Story 9-3) — device
/// identity plus the canonical list of configured metric specs. The
/// metric values themselves come from the `metric_values` SQLite
/// table at request time; this snapshot tells the handler which
/// metrics to look up + the order to render them in.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceSummary {
    pub device_id: String,
    pub device_name: String,
    /// Configured metric specs in TOML-declaration order. Adding a new
    /// metric to the bottom of the TOML list shows up at the bottom of
    /// the row in the dashboard — no random hashmap reordering.
    ///
    /// **Story 9-3 review iter-1 H1 fix:** the previous shape carried
    /// two parallel `Vec<String>` + `Vec<OpcMetricTypeConfig>` whose
    /// length invariant was only enforced by-construction in
    /// `from_config`. A future refactor (filter, partial walk, hot-
    /// reload Story 9-7) could let the lengths drift; the
    /// `/api/devices` handler's `.zip()` would silently truncate to
    /// the shorter Vec, dropping metrics from the dashboard with no
    /// error. Bundling them into a struct lets the type system
    /// enforce the invariant.
    pub metrics: Vec<MetricSpec>,
}

/// Configured metric: name + data type, in 1-to-1 correspondence by
/// construction. Ships as a sibling to `DeviceSummary` so external
/// callers can walk the list without juggling parallel Vecs.
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSpec {
    pub metric_name: String,
    pub metric_type: crate::config::OpcMetricTypeConfig,
}

/// Frozen-at-startup snapshot of the gateway's configured topology.
///
/// Built once in `main.rs::main` from `AppConfig.application_list` and
/// shared via `Arc<AppState>` to every `/api/*` handler. Story 9-7
/// (configuration hot-reload) will replace the field type with a
/// `tokio::sync::watch::Receiver<DashboardConfigSnapshot>` so a
/// hot-reload re-publishes a fresh snapshot without restarting the
/// web server.
///
/// Counts are **configured**, not **live ChirpStack-discovered** — the
/// dashboard answers "how many devices is the gateway *trying to*
/// poll", which is the operator-relevant question for "is my config
/// loaded correctly". A live-discovered count would require querying
/// ChirpStack's gRPC API on every dashboard refresh.
#[derive(Clone, Debug, PartialEq)]
pub struct DashboardConfigSnapshot {
    pub application_count: usize,
    pub device_count: usize,
    pub applications: Vec<ApplicationSummary>,
}

impl DashboardConfigSnapshot {
    /// Walk `config.application_list` once and build the summary.
    /// Pure function; no I/O.
    ///
    /// Story 9-3 deepens the walk by one level — the per-device
    /// `metrics: Vec<MetricSpec>` list comes from each device's
    /// `read_metric_list`, in TOML-declaration order.
    pub fn from_config(config: &AppConfig) -> Self {
        let applications: Vec<ApplicationSummary> = config
            .application_list
            .iter()
            .map(|app| {
                let devices: Vec<DeviceSummary> = app
                    .device_list
                    .iter()
                    .map(|dev| {
                        let metrics: Vec<MetricSpec> = dev
                            .read_metric_list
                            .iter()
                            .map(|m| MetricSpec {
                                metric_name: m.metric_name.clone(),
                                metric_type: m.metric_type.clone(),
                            })
                            .collect();
                        DeviceSummary {
                            device_id: dev.device_id.clone(),
                            device_name: dev.device_name.clone(),
                            metrics,
                        }
                    })
                    .collect();
                let device_count = devices.len();
                ApplicationSummary {
                    application_id: app.application_id.clone(),
                    application_name: app.application_name.clone(),
                    device_count,
                    devices,
                }
            })
            .collect();
        let application_count = applications.len();
        let device_count = applications.iter().map(|a| a.device_count).sum();
        Self {
            application_count,
            device_count,
            applications,
        }
    }
}

/// Shared state for every `/api/*` handler + the auth middleware.
///
/// One `Arc<AppState>` per process, constructed in `main.rs::main`
/// after the OPC UA / poller / command-status / command-timeout
/// backends. The web server's own `SqliteBackend` lives in the
/// `backend` field — same per-task ownership pattern as the other
/// four tokio tasks (Story 4-1 / 5-1 / 8-3 precedent).
///
/// `start_time` is captured at `AppState` construction (not at
/// process start) so `uptime_secs` reflects "web-server uptime" —
/// close enough to "gateway uptime" because the web server spawns
/// alongside the other tasks at startup.
///
/// # Story 9-7 hot-reload swap discipline
///
/// `dashboard_snapshot` and `stale_threshold_secs` are wrapped in
/// interior-mutability primitives (`std::sync::RwLock` and
/// `AtomicU64`) so the web-config-listener task in `main.rs` can
/// atomically replace them when the configuration is hot-reloaded
/// via SIGHUP. Handlers read through `.read().unwrap().clone()` /
/// `.load(Ordering::Relaxed)` — both are O(1) and lock-free in the
/// uncontended path. The `auth` field stays `Arc<WebAuthState>`
/// because rotating credentials at runtime would require modifying
/// the auth-middleware's captured Arc — `src/web/auth.rs` is a
/// Story 9-7 file invariant, so credential rotation is
/// restart-required in v1 (caught by `config_reload::classify_diff`).
pub struct AppState {
    pub auth: Arc<WebAuthState>,
    pub backend: Arc<dyn StorageBackend>,
    /// Story 9-7: wrapped in `RwLock<Arc<...>>` so the
    /// web-config-listener task can atomically swap the snapshot
    /// after a hot-reload. Read-side is a brief `.read().unwrap()`
    /// followed by a clone of the Arc — sub-microsecond in the
    /// uncontended path.
    pub dashboard_snapshot: std::sync::RwLock<Arc<DashboardConfigSnapshot>>,
    pub start_time: Instant,
    /// Story 9-3 (FR37): resolved `[opcua].stale_threshold_seconds`
    /// (default 120) — used as the "Good → Uncertain" boundary in
    /// `/api/devices` JSON. Story 9-7 makes this an `AtomicU64` so
    /// hot-reload can swap the value without restarting the web
    /// server. **Note (v1 limitation):** the OPC UA path
    /// (`src/opc_ua.rs`) captures the threshold into per-variable
    /// read-callback closures at startup; hot-reload of this knob
    /// therefore affects **only** the web dashboard's "Good →
    /// Uncertain" boundary in v1. Documented in
    /// `docs/security.md § Configuration hot-reload`.
    pub stale_threshold_secs: std::sync::atomic::AtomicU64,
    /// Story 9-4: handle to the configuration reload routine. CRUD
    /// handlers call `config_reload.reload().await` after writing
    /// the TOML to trigger validate-then-swap of the live
    /// `Arc<AppConfig>` and the dashboard snapshot. Threaded into
    /// `AppState` from `main.rs::main` (the same `Arc<...>` already
    /// retained by the SIGHUP listener task is reused here — no
    /// new construction).
    pub config_reload: Arc<crate::config_reload::ConfigReloadHandle>,
    /// Story 9-4: TOML round-trip helper for CRUD-driven config
    /// mutations. `figment` (`src/config.rs`) is the read side;
    /// this is the write side. Held lock serialises concurrent
    /// CRUD requests across the entire write+reload critical
    /// section.
    pub config_writer: Arc<crate::web::config_writer::ConfigWriter>,
}

/// Story 9-7 Task 4 — extracted from the inline clamp at
/// `src/main.rs:823-846` so startup AND the web-config-listener task
/// share a single source of truth for the
/// `[opcua].stale_threshold_seconds` clamp logic.
///
/// Returns the clamped threshold along with an
/// [`StaleThresholdClampOutcome`] that the caller uses to decide
/// whether to emit the `warn!` log line (we don't log from inside
/// this helper because the clamp is called from both startup and
/// hot-reload, and the appropriate operator-action text differs in
/// the two cases).
///
/// Two invariants enforced:
///   - `0` is replaced with `DEFAULT_STALE_THRESHOLD_SECS` (would
///     otherwise mark every metric immediately stale on the dashboard).
///   - Anything strictly greater than `BAD_THRESHOLD_SECS` is replaced
///     with `DEFAULT_STALE_THRESHOLD_SECS` (would otherwise compress
///     the "uncertain" band to nothing).
///
/// Boundary is exclusive on the upper side (`>` not `>=`) — `86400`
/// exactly is allowed because `AppConfig::validate` accepts the range
/// `(0, 86400]`.
pub fn clamp_stale_threshold(raw: u64) -> (u64, StaleThresholdClampOutcome) {
    if raw == 0 {
        (
            api::DEFAULT_STALE_THRESHOLD_SECS,
            StaleThresholdClampOutcome::ClampedFromZero,
        )
    } else if raw > api::BAD_THRESHOLD_SECS {
        (
            api::DEFAULT_STALE_THRESHOLD_SECS,
            StaleThresholdClampOutcome::ClampedFromAboveBad,
        )
    } else {
        (raw, StaleThresholdClampOutcome::Accepted)
    }
}

/// Outcome of [`clamp_stale_threshold`] — the caller uses this to
/// decide whether to emit a warn log line and which template to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StaleThresholdClampOutcome {
    Accepted,
    ClampedFromZero,
    ClampedFromAboveBad,
}

/// Inner timeout for the graceful-shutdown drain. After [`run`] sees
/// `cancel.cancelled()` it gives `axum::serve` up to this many seconds
/// to drain in-flight requests; if a slow-loris client (or any other
/// stuck connection) holds the drain open past the budget the serve
/// future is aborted and `run` returns. The 10-second outer timeout in
/// `src/main.rs` is the second-line defence; this inner timeout caps
/// the per-surface cost so a single hung web request can't eat the
/// whole shutdown budget that the other 4 tasks share.
const GRACEFUL_SHUTDOWN_BUDGET_SECS: u64 = 5;

/// Build the Axum `Router` for the embedded web server.
///
/// All routes inherit the `basic_auth_middleware` layer — the `auth`
/// state is shared via `from_fn_with_state` so the per-request
/// extractor finds the configured digests + key without thread-local
/// state. The `/api/health` route is the smoke endpoint used by
/// integration tests; static files are served from the `static_dir`
/// path under the namespace root.
///
/// # Layer ordering invariant (load-bearing for AC#5 security)
///
/// `.layer(...)` is applied AFTER both `.route(...)` and
/// `.fallback_service(...)`. In axum 0.8 this order means the auth
/// layer wraps both the routed `/api/health` handler AND the
/// `ServeDir` fallback, so unauth requests for static files (or for
/// non-existent paths) are rejected with 401 BEFORE the file-system
/// dispatch runs. AC#5's security property: an unauthenticated
/// attacker probing for file existence via `GET /nonexistent.html`
/// must see 401, not 404 — otherwise the 404-vs-401 differential
/// leaks the directory layout. Pinned by
/// `tests/web_auth.rs::test_unauth_unknown_path_returns_401`.
///
/// # Symlink-following limitation
///
/// `tower-http = "0.6"`'s `ServeDir` does not expose a
/// symlink-disable knob (`follow_symlinks(false)` is **not** part of
/// its public API as of 0.6.8 — verified against the upstream source
/// during Story 9-1 review iter-1). On Linux, `tokio::fs::File::open`
/// — which `ServeDir` uses underneath — follows symlinks by default.
/// **Operators must ensure the `static/` directory contains no
/// symlinks**, especially symlinks pointing outside the directory
/// (e.g. to `/etc`, `/proc`, or to operator home directories).
/// Tracked as a follow-up: a custom `tower::Service` wrapper that
/// canonicalises every request path against the canonical `static/`
/// root before dispatch would close this gap, but that's a Story
/// 9-1 scope expansion beyond what the review iter-1 budget allows.
/// See `docs/security.md § Web UI authentication § Anti-patterns`.
///
/// # Arguments
///
/// * `auth_state` — the shared auth state (one per process).
/// * `static_dir` — directory holding the placeholder HTML files
///   (Stories 9-2+ fill them in). The path resolves relative to the
///   gateway's current working directory at the time of the request,
///   so operators must arrange for `static/` to be reachable from
///   `WorkingDirectory` (systemd) / `WORKDIR` (Docker) / the cwd of
///   the spawning shell. See `docs/security.md § Web UI authentication
///   § Deployment requirements`.
pub fn build_router(app_state: Arc<AppState>, static_dir: PathBuf) -> Router {
    let auth_state = app_state.auth.clone();
    // Story 9-4: build the CSRF state from the live config's
    // `[web].allowed_origins` (or the default
    // `http://<bind_address>:<port>`). The state is captured at
    // router-build time; hot-reload of `[web].allowed_origins` is
    // restart-required (caught by `config_reload::classify_diff`).
    let csrf_state = {
        let live = app_state.config_reload.subscribe();
        let cfg = (*live.borrow()).clone();
        csrf::CsrfState::new(cfg.web.resolved_allowed_origins())
    };

    Router::new()
        .route("/api/health", get(api_health))
        .route("/api/status", get(api::api_status))
        .route("/api/devices", get(api::api_devices))
        // Story 9-4 CRUD routes (FR34).
        .route(
            "/api/applications",
            get(api::list_applications).post(api::create_application),
        )
        .route(
            "/api/applications/{application_id}",
            get(api::get_application)
                .put(api::update_application)
                .delete(api::delete_application),
        )
        // Story 9-5 CRUD routes (FR35) — device + metric mapping CRUD
        // nested under the application surface. The path-aware CSRF
        // middleware dispatches `event="device_crud_rejected"` on
        // these routes (Story 9-5 Task 2).
        .route(
            "/api/applications/{application_id}/devices",
            get(api::list_devices).post(api::create_device),
        )
        .route(
            "/api/applications/{application_id}/devices/{device_id}",
            get(api::get_device)
                .put(api::update_device)
                .delete(api::delete_device),
        )
        .fallback_service(ServeDir::new(static_dir))
        // Layer ordering invariant (load-bearing): axum 0.8 stacks
        // .layer(...) calls in REVERSE declaration order. For runtime
        // ordering "auth runs first → CSRF runs second → handler runs
        // third → security headers added on response", the layers
        // must be declared in the inverse order. The CSRF layer is
        // declared FIRST (innermost in the stack); the auth layer
        // SECOND; the security-header layers are LAST (outermost) so
        // they run on every response after handler completion.
        .layer(axum::middleware::from_fn_with_state(
            csrf_state,
            csrf::csrf_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            basic_auth_middleware,
        ))
        // Iter-1 review P9: clickjacking defence. `X-Frame-Options:
        // DENY` blocks framing entirely (legacy header for IE/old
        // Safari). `Content-Security-Policy: frame-ancestors 'none'`
        // is the modern equivalent (also accepted by current Chrome,
        // Firefox, Edge). Both ship — defence-in-depth.
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::header::X_FRAME_OPTIONS,
            axum::http::HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            axum::http::HeaderName::from_static("content-security-policy"),
            axum::http::HeaderValue::from_static("frame-ancestors 'none'"),
        ))
        .with_state(app_state)
}

/// Trivial health-check endpoint. Returns `{"status":"ok"}` with a
/// 200 status code on every authenticated request. NOT an
/// operator-facing endpoint — its job is to give integration tests
/// a known route they can hit to verify the auth middleware fires
/// without depending on a static-file fixture.
async fn api_health() -> impl IntoResponse {
    (
        StatusCode::OK,
        [(axum::http::header::CONTENT_TYPE, "application/json")],
        "{\"status\":\"ok\"}",
    )
}

/// Bind a `TcpListener` for the embedded web server (Story 9-1
/// review iter-1, D1 resolution).
///
/// Bind happens synchronously in `main` BEFORE `tokio::spawn` so a
/// bind failure surfaces as a startup error and aborts the gateway
/// — consistent with Story 7-2's fail-closed pattern. If we deferred
/// the bind into the spawned task, an operator who set
/// `[web].enabled = true` and started the gateway with a port already
/// in use would see only a single `error!` log line and the gateway
/// would otherwise run normally — easy to miss in busy log streams.
///
/// # Errors
///
/// Returns `Err(OpcGwError::Web(...))` on bind failure: port in use,
/// permission denied (privileged port without `CAP_NET_BIND_SERVICE`),
/// or platform refusal of the bind address.
pub async fn bind(addr: SocketAddr) -> Result<TcpListener, OpcGwError> {
    TcpListener::bind(addr).await.map_err(|e| {
        OpcGwError::Web(format!(
            "failed to bind web server to {addr}: {e} (port in use? \
             insufficient permission for the bind address?)"
        ))
    })
}

/// Run the embedded web server on a pre-bound `TcpListener` until
/// `cancel` is cancelled.
///
/// Emits the `event="web_server_started"` info-level diagnostic event
/// with the resolved bind address + realm, then serves requests.
/// Cancellation drains in-flight requests via `with_graceful_shutdown`
/// bounded by a [`GRACEFUL_SHUTDOWN_BUDGET_SECS`]-second post-cancel
/// timeout (review iter-2 fix — caps slow-loris connections from
/// stalling the drain indefinitely without affecting normal-operation
/// uptime), and emits a plain info-level shutdown log line (without
/// `event=` field — AC#8 limits Story 9-1 to exactly two structured
/// event names) before returning `Ok(())`.
///
/// # Drain-timeout shape
///
/// `tokio::select!` between two futures:
///   1. The `axum::serve(...).with_graceful_shutdown(cancel.cancelled())`
///      future, which resolves on graceful-drain completion or on a
///      serve error.
///   2. A "post-cancel deadline" future: wait for `cancel.cancelled()`,
///      then sleep for [`GRACEFUL_SHUTDOWN_BUDGET_SECS`]. If this
///      branch wins, the drain has stalled past the budget — emit a
///      warn line and return (forces the listener to drop, breaking
///      any hung connection).
///
/// Crucially, the timeout starts ticking **only after `cancel`
/// fires**, not at server-startup time. The previous iter-1 shape
/// `tokio::time::timeout(5s, serve_future)` measured from the moment
/// the future was first polled — which made the server self-terminate
/// after 5 seconds of normal operation. Iter-2 caught that
/// regression.
///
/// # Errors
///
/// Returns `Err(OpcGwError::Web(...))` if the underlying `axum::serve`
/// surfaces an unexpected I/O error before cancel fires.
pub async fn run(
    listener: TcpListener,
    router: Router,
    realm: &str,
    cancel: CancellationToken,
) -> Result<(), OpcGwError> {
    let bound = listener.local_addr().map_err(|e| {
        OpcGwError::Web(format!("failed to read local_addr from listener: {e}"))
    })?;

    info!(
        event = "web_server_started",
        bind_address = %bound.ip(),
        port = bound.port(),
        realm = %realm,
        "Embedded web server started"
    );

    let cancel_for_serve = cancel.clone();
    let serve_future = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { cancel_for_serve.cancelled().await });

    // Two-future select: the serve future wins on clean drain or
    // I/O error; the post-cancel deadline future wins only if the
    // graceful-shutdown path stalls past the budget.
    let cancel_for_deadline = cancel.clone();
    let post_cancel_deadline = async move {
        cancel_for_deadline.cancelled().await;
        tokio::time::sleep(Duration::from_secs(GRACEFUL_SHUTDOWN_BUDGET_SECS)).await;
    };

    tokio::select! {
        // Bias toward the serve future so a clean drain that
        // completes within the budget is preferred over a racing
        // deadline. The select macro polls in source order under
        // `biased`.
        biased;
        result = serve_future => {
            match result {
                Ok(()) => {
                    info!(
                        bind_address = %bound.ip(),
                        port = bound.port(),
                        "Embedded web server stopped (graceful shutdown)"
                    );
                    Ok(())
                }
                Err(e) => {
                    warn!(error = %e, "Embedded web server exited with error");
                    Err(OpcGwError::Web(format!("web server I/O error: {e}")))
                }
            }
        }
        _ = post_cancel_deadline => {
            warn!(
                bind_address = %bound.ip(),
                port = bound.port(),
                budget_secs = GRACEFUL_SHUTDOWN_BUDGET_SECS,
                "Embedded web server graceful-shutdown budget elapsed; \
                 forcing close (likely a hung in-flight request)"
            );
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AppConfig, ChirpStackApplications, ChirpstackDevice, ChirpstackPollerConfig,
        CommandValidationConfig, Global, OpcMetricTypeConfig, OpcUaConfig, ReadMetric,
        StorageConfig, WebConfig,
    };
    use crate::storage::memory::InMemoryBackend;
    use crate::web::auth::WebAuthState;

    /// Minimal valid `AppConfig` for the dashboard-snapshot unit tests.
    /// Mirrors the explicit-field pattern in
    /// `src/web/auth.rs::tests::web_auth_test_config` because `Global`,
    /// `ChirpstackPollerConfig`, and `OpcUaConfig` do not derive Default.
    pub(crate) fn snapshot_test_config(applications: Vec<ChirpStackApplications>) -> AppConfig {
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
                api_token: "t".to_string(),
                tenant_id: "00000000-0000-0000-0000-000000000000".to_string(),
                polling_frequency: 10,
                retry: 1,
                delay: 1,
                list_page_size: 100,
            },
            opcua: OpcUaConfig {
                application_name: "test".to_string(),
                application_uri: "urn:test".to_string(),
                product_uri: "urn:test:product".to_string(),
                diagnostics_enabled: false,
                hello_timeout: Some(5),
                host_ip_address: Some("127.0.0.1".to_string()),
                host_port: Some(0),
                create_sample_keypair: true,
                certificate_path: "own/cert.der".to_string(),
                private_key_path: "private/private.pem".to_string(),
                trust_client_cert: false,
                check_cert_time: false,
                pki_dir: "./pki".to_string(),
                user_name: "opcua-user".to_string(),
                user_password: "secret".to_string(),
                stale_threshold_seconds: Some(120),
                max_connections: None,
                max_subscriptions_per_session: None,
                max_monitored_items_per_sub: None,
                max_message_size: None,
                max_chunk_count: None,
                max_history_data_results_per_node: None,
            },
            storage: StorageConfig::default(),
            command_validation: CommandValidationConfig::default(),
            web: WebConfig::default(),
            application_list: applications,
        }
    }

    fn make_app(name: &str, id: &str, devices: usize) -> ChirpStackApplications {
        ChirpStackApplications {
            application_name: name.to_string(),
            application_id: id.to_string(),
            device_list: (0..devices)
                .map(|i| ChirpstackDevice {
                    device_id: format!("dev-{name}-{i}"),
                    device_name: format!("Device {i}"),
                    read_metric_list: vec![ReadMetric {
                        metric_name: "temperature".to_string(),
                        chirpstack_metric_name: "temp".to_string(),
                        metric_type: OpcMetricTypeConfig::Float,
                        metric_unit: None,
                    }],
                    device_command_list: None,
                })
                .collect(),
        }
    }

    /// Story 9-2 AC#1 — frozen-at-startup snapshot walks the
    /// application list once and produces correct totals.
    /// Story 9-3 extension — the snapshot also carries the per-device
    /// list with metric names + types in TOML-declaration order.
    #[test]
    fn dashboard_snapshot_from_config_walks_application_list_once() {
        let config = snapshot_test_config(vec![
            make_app("Sensors", "550e8400-e29b-41d4-a716-446655440001", 3),
            make_app("Valves", "550e8400-e29b-41d4-a716-446655440002", 3),
        ]);
        let snapshot = DashboardConfigSnapshot::from_config(&config);
        assert_eq!(snapshot.application_count, 2);
        assert_eq!(snapshot.device_count, 6);
        assert_eq!(snapshot.applications.len(), 2);
        assert!(snapshot.applications.iter().all(|a| a.device_count == 3));
        // Story 9-3 — devices list mirrors the configured topology.
        for app in &snapshot.applications {
            assert_eq!(app.devices.len(), 3);
            for dev in &app.devices {
                assert_eq!(dev.metrics.len(), 1);
                assert_eq!(dev.metrics[0].metric_name, "temperature");
                assert_eq!(dev.metrics[0].metric_type, OpcMetricTypeConfig::Float);
            }
        }
    }

    /// Story 9-3 AC#1 — a device with zero `read_metric_list` entries
    /// produces a `DeviceSummary` with an empty `metrics` Vec rather
    /// than getting dropped from the snapshot. Operator deletes all
    /// metrics from a device temporarily; the device should still
    /// appear in the dashboard (as an empty row), not vanish.
    #[test]
    fn dashboard_snapshot_from_config_handles_device_with_zero_metrics() {
        let mut config = snapshot_test_config(vec![]);
        config.application_list = vec![ChirpStackApplications {
            application_name: "App".to_string(),
            application_id: "id-1".to_string(),
            device_list: vec![ChirpstackDevice {
                device_id: "dev-empty".to_string(),
                device_name: "Empty Device".to_string(),
                read_metric_list: vec![],
                device_command_list: None,
            }],
        }];
        let snapshot = DashboardConfigSnapshot::from_config(&config);
        assert_eq!(snapshot.application_count, 1);
        assert_eq!(snapshot.device_count, 1);
        assert_eq!(snapshot.applications[0].devices.len(), 1);
        assert!(snapshot.applications[0].devices[0].metrics.is_empty());
    }

    /// Story 9-2 AC#1 — empty application list produces all-zeros.
    #[test]
    fn dashboard_snapshot_from_config_handles_empty_application_list() {
        let snapshot = DashboardConfigSnapshot::from_config(&snapshot_test_config(vec![]));
        assert_eq!(snapshot.application_count, 0);
        assert_eq!(snapshot.device_count, 0);
        assert!(snapshot.applications.is_empty());
    }

    /// Story 9-2 AC#1 — application with zero devices contributes 0
    /// to `device_count` and is preserved as a 0-count summary entry.
    #[test]
    fn dashboard_snapshot_from_config_handles_application_with_zero_devices() {
        let config = snapshot_test_config(vec![
            make_app("EmptyApp", "id-1", 0),
            make_app("OneDev", "id-2", 1),
        ]);
        let snapshot = DashboardConfigSnapshot::from_config(&config);
        assert_eq!(snapshot.application_count, 2);
        assert_eq!(snapshot.device_count, 1);
        assert_eq!(snapshot.applications[0].device_count, 0);
        assert_eq!(snapshot.applications[1].device_count, 1);
    }

    /// `build_router` returns a `Router` shape that compiles cleanly
    /// under the new `Arc<AppState>` shape. The actual request-routing
    /// behaviour is exercised by the `tests/web_auth.rs` and
    /// `tests/web_dashboard.rs` integration tests; this is a smoke test
    /// that the builder type-checks under the current axum version.
    #[test]
    fn build_router_smoke() {
        let auth = Arc::new(WebAuthState::new_with_fresh_key(
            "opcua-user",
            "secret",
            "opcgw-test".to_string(),
        ));
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cfg = snapshot_test_config(vec![]);
        let snapshot = Arc::new(DashboardConfigSnapshot::from_config(&cfg));
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let toml_path = tmp.path().join("config.toml");
        std::fs::write(&toml_path, "[global]\ndebug = true\n").expect("write toml");
        let (handle, _rx) = crate::config_reload::ConfigReloadHandle::new(
            Arc::new(cfg),
            toml_path.clone(),
        );
        let app_state = Arc::new(AppState {
            auth,
            backend,
            dashboard_snapshot: std::sync::RwLock::new(snapshot),
            start_time: Instant::now(),
            stale_threshold_secs: std::sync::atomic::AtomicU64::new(
                api::DEFAULT_STALE_THRESHOLD_SECS,
            ),
            config_reload: Arc::new(handle),
            config_writer: crate::web::config_writer::ConfigWriter::new(toml_path),
        });
        let dir = PathBuf::from("static");
        let _router: Router = build_router(app_state, dir);
    }

    /// Story 9-7 Task 4 unit test — clamp helper rejects `0` (would
    /// flag every metric stale on the dashboard).
    #[test]
    fn clamp_stale_threshold_rejects_zero() {
        let (clamped, outcome) = clamp_stale_threshold(0);
        assert_eq!(clamped, api::DEFAULT_STALE_THRESHOLD_SECS);
        assert_eq!(outcome, StaleThresholdClampOutcome::ClampedFromZero);
    }

    /// Story 9-7 Task 4 unit test — clamp helper rejects values strictly
    /// above `BAD_THRESHOLD_SECS` (would compress the uncertain band).
    #[test]
    fn clamp_stale_threshold_rejects_above_bad() {
        let (clamped, outcome) = clamp_stale_threshold(api::BAD_THRESHOLD_SECS + 1);
        assert_eq!(clamped, api::DEFAULT_STALE_THRESHOLD_SECS);
        assert_eq!(outcome, StaleThresholdClampOutcome::ClampedFromAboveBad);
    }

    /// Story 9-7 Task 4 unit test — clamp helper accepts the boundary
    /// value (BAD_THRESHOLD_SECS exactly) and ordinary values per the
    /// `AppConfig::validate` contract `(0, 86400]`.
    #[test]
    fn clamp_stale_threshold_accepts_boundary_and_ordinary() {
        let (clamped, outcome) = clamp_stale_threshold(api::BAD_THRESHOLD_SECS);
        assert_eq!(clamped, api::BAD_THRESHOLD_SECS);
        assert_eq!(outcome, StaleThresholdClampOutcome::Accepted);

        let (clamped, outcome) = clamp_stale_threshold(120);
        assert_eq!(clamped, 120);
        assert_eq!(outcome, StaleThresholdClampOutcome::Accepted);
    }
}

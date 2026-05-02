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

pub mod auth;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use axum::Router;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tower_http::services::ServeDir;
use tracing::{info, warn};

use crate::utils::OpcGwError;
use auth::{basic_auth_middleware, WebAuthState};

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
pub fn build_router(auth_state: Arc<WebAuthState>, static_dir: PathBuf) -> Router {
    Router::new()
        .route("/api/health", get(api_health))
        .fallback_service(ServeDir::new(static_dir))
        .layer(axum::middleware::from_fn_with_state(
            auth_state,
            basic_auth_middleware,
        ))
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
    use crate::web::auth::WebAuthState;

    /// `build_router` returns a `Router` shape that compiles cleanly.
    /// The actual request-routing behaviour is exercised by the
    /// `tests/web_auth.rs` integration tests; this is a smoke test
    /// that the builder type-checks under the current axum version.
    #[test]
    fn build_router_smoke() {
        let state = Arc::new(WebAuthState::new_with_fresh_key(
            "opcua-user",
            "secret",
            "opcgw-test".to_string(),
        ));
        let dir = PathBuf::from("static");
        let _router: Router = build_router(state, dir);
    }
}

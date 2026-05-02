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
//! Mirrors the OPC UA server / ChirpStack poller / command poller /
//! command-timeout-handler tasks: spawned via `tokio::spawn`, joined
//! into the `tokio::select!` shutdown sequence in `src/main.rs`. The
//! [`run`] function wraps `axum::serve` with
//! `with_graceful_shutdown(cancel.cancelled())` so cancellation drains
//! in-flight requests cleanly.

pub mod auth;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

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

/// Build the Axum `Router` for the embedded web server.
///
/// All routes inherit the `basic_auth_middleware` layer — the `auth`
/// state is shared via `from_fn_with_state` so the per-request
/// extractor finds the configured digests + key without thread-local
/// state. The `/api/health` route is the smoke endpoint used by
/// integration tests; static files are served from the `static_dir`
/// path under the namespace root.
///
/// # Arguments
///
/// * `auth_state` — the shared auth state (one per process, shared
///   key with the OPC UA auth surface).
/// * `static_dir` — directory holding the placeholder HTML files
///   (Stories 9-2+ fill them in). Typically `static/` resolved
///   relative to the gateway's current working directory.
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

/// Run the embedded web server until `cancel` is cancelled.
///
/// Binds to `addr`, emits the `event="web_server_started"` info-level
/// diagnostic event with the resolved bind address + realm, then
/// serves requests. Cancellation drains in-flight requests via
/// `with_graceful_shutdown` and emits a plain info-level shutdown
/// log line (without `event=` field — AC#8 limits Story 9-1 to
/// exactly two structured event names) before returning `Ok(())`.
///
/// # Errors
///
/// Returns `Err(OpcGwError::Web(...))` on bind failure (port in use,
/// permission denied, unparseable address, etc.) or unexpected I/O
/// error during request handling.
pub async fn run(
    addr: SocketAddr,
    router: Router,
    realm: &str,
    cancel: CancellationToken,
) -> Result<(), OpcGwError> {
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        OpcGwError::Web(format!(
            "failed to bind web server to {addr}: {e} (port in use? \
             insufficient permission for the bind address?)"
        ))
    })?;

    let bound = listener.local_addr().map_err(|e| {
        OpcGwError::Web(format!(
            "failed to read local_addr after bind to {addr}: {e}"
        ))
    })?;

    info!(
        event = "web_server_started",
        bind_address = %bound.ip(),
        port = bound.port(),
        realm = realm,
        "Embedded web server started"
    );

    let serve_result = axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async move { cancel.cancelled().await })
    .await;

    match serve_result {
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

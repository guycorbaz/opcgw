// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2024 Guy Corbaz

//! Story F-0: the "Apply changes" endpoint.
//!
//! Configuration edits made through the web UI (the singleton-config editor
//! and the application/device/metric/command CRUD handlers) are **staged** —
//! they write to SQLite but do not take effect on the running gateway and do
//! not restart anything. The operator applies a whole batch at once by
//! POSTing to `/api/config/apply`.
//!
//! This handler fires [`AppState::apply_signal`]; the in-process restart
//! supervisor in `src/main.rs` wakes on that signal, re-reads the full
//! configuration from SQLite, and performs one graceful **in-process** soft
//! restart of the data-plane (poller, OPC UA server, gRPC event stream,
//! command-timeout handler). The Docker container is **never** restarted.
//!
//! The supervisor emits the lifecycle events `apply_requested` /
//! `apply_completed` / `apply_failed`; this handler emits `apply_invoked`
//! and returns `202 Accepted` immediately (the restart runs asynchronously;
//! clients observe completion via `pending_changes` flipping back to `false`
//! on `GET /api/status`, or by the OPC UA endpoint briefly cycling).

use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use tracing::info;

use super::AppState;

/// `POST /api/config/apply` — request an in-process soft restart so all
/// staged configuration changes take effect. Basic-auth + CSRF protected
/// (the `config_apply` CSRF resource bucket emits `config_apply_rejected`
/// on a CSRF failure). Returns `202 Accepted`.
pub async fn api_config_apply(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let had_pending = state.has_pending_changes();
    info!(
        event = "apply_invoked",
        had_pending_changes = had_pending,
        "POST /api/config/apply: signalling in-process data-plane soft restart"
    );
    // Wake the supervisor loop in main.rs. If it is mid-cycle (not currently
    // awaiting), `notify_one` stores a permit so the next `notified()`
    // returns immediately — no apply request is lost.
    state.apply_signal.notify_one();
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "status": "apply_requested",
            "had_pending_changes": had_pending,
        })),
    )
}

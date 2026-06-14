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

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use axum::{extract::State, http::StatusCode, response::IntoResponse, Json};
use serde_json::json;
use tracing::info;

use super::AppState;

/// Story F-0 review (D2): process-global "the most recent Apply failed" flag.
///
/// There is exactly one restart supervisor and one `AppState` per process, so
/// a module-level atomic is the simplest channel for the supervisor
/// (`src/main.rs`) to publish an apply outcome that `GET /api/status` then
/// surfaces to the web UI (consistent with the existing once-per-boot
/// `AtomicBool` guards). Without it, `apply_failed` is invisible to the
/// operator: `apply-bar.js` would show a failed Apply as a silent 30 s hang
/// because `pending_changes` never flips back to `false` on failure.
static LAST_APPLY_FAILED: AtomicBool = AtomicBool::new(false);

/// Record that the most recent Apply failed (called by the supervisor on
/// either a config re-read failure or a build-time respawn failure/revert).
pub fn mark_apply_failed() {
    LAST_APPLY_FAILED.store(true, Ordering::Relaxed);
}

/// Clear the apply-failure flag (called when a fresh Apply is invoked).
pub fn clear_apply_failed() {
    LAST_APPLY_FAILED.store(false, Ordering::Relaxed);
}

/// Whether the most recent Apply failed. Exposed on `GET /api/status` as
/// `apply_failed` so the web UI can distinguish a failed Apply from a pending
/// or in-progress one.
pub fn apply_failed_flag() -> bool {
    LAST_APPLY_FAILED.load(Ordering::Relaxed)
}

/// `POST /api/config/apply` — request an in-process soft restart so all
/// staged configuration changes take effect. Basic-auth + CSRF protected
/// (the `config_apply` CSRF resource bucket emits `config_apply_rejected`
/// on a CSRF failure).
///
/// Returns `202 Accepted` when there are staged changes to apply, or
/// `200 OK` with `status: "no_pending_changes"` when there is nothing to do
/// — the latter avoids a gratuitous soft restart (which drops every OPC UA
/// client) on a duplicate/stale POST (review P4).
pub async fn api_config_apply(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    if !state.has_pending_changes() {
        info!(
            event = "apply_invoked",
            had_pending_changes = false,
            "POST /api/config/apply: no pending changes; nothing to apply (no restart)"
        );
        return (
            StatusCode::OK,
            Json(json!({
                "status": "no_pending_changes",
                "had_pending_changes": false,
            })),
        );
    }
    // A fresh Apply attempt clears any prior failure flag (review D2).
    clear_apply_failed();
    info!(
        event = "apply_invoked",
        had_pending_changes = true,
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
            "had_pending_changes": true,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    /// Story F-0 review (D2): the process-global apply-failure flag round-trips
    /// through `mark`/`clear` and is what `GET /api/status` reports as
    /// `apply_failed`. Serialised so the shared static can't race other tests.
    #[test]
    #[serial(apply_failed_flag)]
    fn apply_failed_flag_roundtrips() {
        clear_apply_failed();
        assert!(!apply_failed_flag(), "flag must start cleared");
        mark_apply_failed();
        assert!(apply_failed_flag(), "mark_apply_failed must set the flag");
        clear_apply_failed();
        assert!(!apply_failed_flag(), "clear_apply_failed must reset the flag");
    }
}

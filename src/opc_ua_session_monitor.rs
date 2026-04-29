// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! OPC UA session-count monitoring (Story 7-3, AC#3, FR44).
//!
//! Two-event correlation pattern (same shape as Story 7-2 NFR12) for
//! observing concurrent OPC UA sessions and at-limit rejections:
//!
//! 1. **Periodic gauge** — `SessionMonitor::run_gauge_loop` emits an
//!    `info!(event="opcua_session_count", current=N, limit=L)` line every
//!    `OPCUA_SESSION_GAUGE_INTERVAL_SECS` seconds. Operators graph this
//!    for capacity planning.
//! 2. **At-limit accept warn** — `AtLimitAcceptLayer` (a
//!    `tracing_subscriber::Layer`) latches onto async-opcua's
//!    `info!("Accept new connection from {addr} ({n})")` event from
//!    `target = "opcua_server::server"`. On every TCP accept, it reads
//!    the live session count from async-opcua's diagnostics summary; if
//!    it is `>=` the configured limit, it emits a
//!    `warn!(event="opcua_session_count_at_limit", source_ip=..., limit=..., current=...)`.
//!
//! Why this pattern: async-opcua 0.17.1 rejects (N+1)th sessions inside
//! `SessionManager::create_session` with no log emission and no
//! source-IP wiring — see Story 7-3 spec for the design rationale.

use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use opcua::server::ServerHandle;
use opcua::types::{Variant, VariableId};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::utils::OPCUA_SESSION_GAUGE_INTERVAL_SECS;

/// Tracing target async-opcua uses for the `Accept new connection from
/// {addr}` event we correlate against. Mirrors the library's lib name
/// (`opcua_server`, see `async-opcua-server-0.17.1/Cargo.toml`).
const ACCEPT_EVENT_TARGET: &str = "opcua_server::server";

/// Prefix of the `Accept new connection from {addr}` event message.
/// Shared between `AtLimitAcceptLayer::on_event` (filter) and
/// `parse_source_ip` (extractor) so a future async-opcua wording change
/// only requires one constant to update.
const ACCEPT_MESSAGE_PREFIX: &str = "Accept new connection from ";

/// Shared state read by `AtLimitAcceptLayer::on_event`. Production wires
/// this via `set_session_monitor_state` once the OPC UA server is up;
/// the layer no-ops while the slot is `None`. Tests set/clear it per
/// test (under `#[serial_test::serial]`) so the layer reflects the
/// current test's server.
struct MonitorState {
    handle: ServerHandle,
    limit: usize,
}

fn shared_state() -> &'static Arc<RwLock<Option<MonitorState>>> {
    static SLOT: OnceLock<Arc<RwLock<Option<MonitorState>>>> = OnceLock::new();
    SLOT.get_or_init(|| Arc::new(RwLock::new(None)))
}

/// Populate the shared state read by `AtLimitAcceptLayer`. Called by
/// `OpcUa::run` after `create_server` returns the `ServerHandle`.
///
/// Recovers from `RwLock` poisoning so a previously-panicked task does
/// not silently disable the layer for the rest of the process. Logs a
/// debug! line on (re-)installation so a stale-overwrite race (test
/// teardown clearing while a freshly-spawned `OpcUa::run` is mid-set)
/// is observable in the logs.
pub fn set_session_monitor_state(handle: ServerHandle, limit: usize) {
    let slot = shared_state();
    let mut guard = match slot.write() {
        Ok(g) => g,
        Err(poison) => {
            error!(
                event = "session_monitor_state_poisoned",
                "RwLock poisoned in set_session_monitor_state — recovering"
            );
            poison.into_inner()
        }
    };
    if guard.is_some() {
        debug!(
            event = "session_monitor_state_overwritten",
            limit = %limit,
            "Replacing existing MonitorState (test re-entry or supervisor restart)"
        );
    }
    *guard = Some(MonitorState { handle, limit });
}

/// Clear the shared state. Production calls this on graceful shutdown
/// (so a residual handle does not hold a reference past server stop);
/// tests call it on teardown. Idempotent and poison-tolerant.
pub fn clear_session_monitor_state() {
    let slot = shared_state();
    let mut guard = match slot.write() {
        Ok(g) => g,
        Err(poison) => {
            error!(
                event = "session_monitor_state_poisoned",
                "RwLock poisoned in clear_session_monitor_state — recovering"
            );
            poison.into_inner()
        }
    };
    *guard = None;
}

/// Test-only probe — true when a `MonitorState` is currently installed.
/// Used by the shutdown-cleanliness integration test to verify
/// `OpcUa::run`'s graceful-exit path actually clears the slot. Only
/// referenced by integration tests, which compile separately from the
/// bin target — narrow `allow(dead_code)` keeps the bin lint clean
/// without disabling the lint module-wide.
#[allow(dead_code)]
pub fn session_monitor_state_active() -> bool {
    let slot = shared_state();
    match slot.read() {
        Ok(g) => g.is_some(),
        Err(_) => false,
    }
}

/// RAII guard that clears the shared `MonitorState` on drop. Used by
/// `OpcUa::run` so a panic in `server.run()` does not leave a stale
/// `ServerHandle` in the process-wide static `OnceLock`. Code-review
/// feedback 2026-04-29 (panic-safety hardening).
pub struct MonitorStateGuard;

impl Drop for MonitorStateGuard {
    fn drop(&mut self) {
        clear_session_monitor_state();
    }
}

/// Read `current_session_count` from async-opcua's diagnostics summary
/// (Story 7-3, AC#3). Returns 0 when the variant is unexpected (audit
/// path — should be unreachable for async-opcua 0.17.1 because the
/// counter is `LocalValue<u32>` whose `IntoVariant` always produces
/// `Variant::UInt32`).
///
/// **Sentinel-zero hazard:** also returns 0 when the diagnostics
/// variable is missing — typically because `OpcUaConfig::diagnostics_enabled`
/// is `false`. The latter case logs an `error!` so operators can
/// distinguish "no sessions" from "diagnostics misconfigured." Startup
/// validation (`AppConfig::validate`) refuses
/// `max_connections.is_some() && !diagnostics_enabled` to make this
/// path unreachable in practice.
pub fn read_current_session_count(handle: &ServerHandle) -> u32 {
    let summary = &handle.info().diagnostics.summary;
    let dv = match summary.get(
        VariableId::Server_ServerDiagnostics_ServerDiagnosticsSummary_CurrentSessionCount,
    ) {
        Some(dv) => dv,
        None => {
            error!(
                event = "session_count_variable_missing",
                "ServerDiagnosticsSummary_CurrentSessionCount unreadable — \
                 likely diagnostics_enabled=false. Returning sentinel 0."
            );
            return 0;
        }
    };
    match dv.value.as_ref() {
        Some(Variant::UInt32(n)) => *n,
        other => {
            error!(
                event = "session_count_variant_unexpected",
                variant = ?other,
                "CurrentSessionCount returned a non-UInt32 variant — investigate"
            );
            0
        }
    }
}

/// Hand-written, no-regex parse of async-opcua's accept-event message.
/// Format pinned by `server.rs:367`:
/// `"Accept new connection from {addr} ({counter})"`. Returns the `addr`
/// substring (e.g. `"127.0.0.1:54311"` — note: `host:port`, not just IP)
/// or `None` on a malformed input or any addr containing control
/// characters (defence against log injection if upstream ever emits
/// untrusted bytes).
///
/// The returned slice is stored as the `source_ip` field on the
/// `opcua_session_count_at_limit` warn — kept named `source_ip` for
/// consistency with Story 7-2 NFR12's audit-event field convention,
/// even though the value is technically `host:port`.
pub fn parse_source_ip(message: &str) -> Option<&str> {
    let after = message.strip_prefix(ACCEPT_MESSAGE_PREFIX)?;
    let (addr, _) = after.split_once(' ')?;
    if addr.is_empty() {
        return None;
    }
    if addr.chars().any(|c| c.is_control()) {
        return None;
    }
    Some(addr)
}

/// Tracing-Layer that observes async-opcua's `Accept new connection
/// from {addr}` events and emits an at-limit warn when the live session
/// count is `>=` the configured limit (Story 7-3, AC#3).
pub struct AtLimitAcceptLayer;

impl AtLimitAcceptLayer {
    pub fn new() -> Self {
        Self
    }
}

impl Default for AtLimitAcceptLayer {
    fn default() -> Self {
        Self::new()
    }
}

/// Capture the `message` field from a tracing event so we can match
/// the `Accept new connection from {addr} ...` prefix.
struct MessageVisitor(Option<String>);

impl tracing::field::Visit for MessageVisitor {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.0 = Some(value.to_string());
        }
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        if field.name() == "message" {
            self.0 = Some(format!("{value:?}"));
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for AtLimitAcceptLayer {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        // Cheap target gate first — the layer fires on every event, so
        // bailing on non-matching targets is essential.
        if event.metadata().target() != ACCEPT_EVENT_TARGET {
            return;
        }

        let mut visitor = MessageVisitor(None);
        event.record(&mut visitor);
        let Some(message) = visitor.0 else { return };
        if !message.starts_with(ACCEPT_MESSAGE_PREFIX) {
            return;
        }

        let slot = shared_state();
        let guard = match slot.read() {
            Ok(g) => g,
            Err(poison) => {
                // Recover from poisoning so a single panic in a writer
                // does not silently disable the layer for the rest of
                // the process. We log once via tracing::error! at the
                // dispatcher level — note this re-enters the
                // dispatcher, but `error!` on a different target is
                // safe (no recursion through `ACCEPT_EVENT_TARGET`).
                tracing::error!(
                    event = "session_monitor_state_poisoned",
                    "RwLock poisoned in AtLimitAcceptLayer::on_event — recovering"
                );
                poison.into_inner()
            }
        };
        let Some(state) = guard.as_ref() else { return };

        let current = read_current_session_count(&state.handle);
        if (current as usize) >= state.limit {
            // Source-IP parse — fall back to "unknown" if async-opcua's
            // wording changes. We still emit the warn (the rejection is
            // what operators care about) but `source_ip="unknown"`
            // signals the parser is out of sync with the library.
            let source_ip = parse_source_ip(&message).unwrap_or("unknown");
            warn!(
                event = "opcua_session_count_at_limit",
                source_ip = %source_ip,
                limit = %state.limit,
                current = %current,
                "OPC UA session at configured cap; new connection will be rejected"
            );
        }
    }
}

/// Periodic gauge task — runs until the gateway's cancellation token
/// fires. Emits `info!(event="opcua_session_count", current, limit)`
/// every `OPCUA_SESSION_GAUGE_INTERVAL_SECS` seconds.
pub struct SessionMonitor {
    handle: ServerHandle,
    limit: usize,
    cancel: CancellationToken,
}

impl SessionMonitor {
    pub fn new(handle: ServerHandle, limit: usize, cancel: CancellationToken) -> Self {
        Self {
            handle,
            limit,
            cancel,
        }
    }

    pub async fn run_gauge_loop(self) {
        debug!(
            interval_secs = OPCUA_SESSION_GAUGE_INTERVAL_SECS,
            limit = self.limit,
            "Starting OPC UA session-count gauge loop"
        );
        let mut ticker =
            tokio::time::interval(Duration::from_secs(OPCUA_SESSION_GAUGE_INTERVAL_SECS));
        // First tick fires immediately — skip it (in a select! so a
        // shutdown signal during the swallow is honoured) so the first
        // gauge line is observed after one full interval.
        tokio::select! {
            _ = self.cancel.cancelled() => {
                debug!("Session-count gauge loop stopping during startup (cancellation)");
                return;
            }
            _ = ticker.tick() => {}
        }
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => {
                    debug!("Session-count gauge loop stopping (cancellation)");
                    return;
                }
                _ = ticker.tick() => {
                    let current = read_current_session_count(&self.handle);
                    info!(
                        event = "opcua_session_count",
                        current = %current,
                        limit = %self.limit,
                        "OPC UA session-count gauge"
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_source_ip_happy_path() {
        let msg = "Accept new connection from 192.168.1.5:54311 (3)";
        assert_eq!(parse_source_ip(msg), Some("192.168.1.5:54311"));
    }

    #[test]
    fn test_parse_source_ip_returns_none_on_unrecognised_message() {
        assert_eq!(parse_source_ip("Some other message"), None);
    }

    #[test]
    fn test_parse_source_ip_returns_none_on_truncated_message() {
        assert_eq!(parse_source_ip("Accept new connection from "), None);
        // No trailing space at all — `split_once(' ')` returns None.
        assert_eq!(parse_source_ip("Accept new connection from"), None);
    }

    #[test]
    fn test_parse_source_ip_rejects_control_characters() {
        // Defence against log injection: any control char in the
        // extracted addr causes None. Code-review feedback 2026-04-29.
        assert_eq!(
            parse_source_ip("Accept new connection from \x00:0 (1)"),
            None
        );
        assert_eq!(
            parse_source_ip("Accept new connection from 127.0.0.1\x1b[31m:5000 (1)"),
            None
        );
    }
}

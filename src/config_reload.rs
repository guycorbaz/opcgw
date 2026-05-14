// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Configuration hot-reload (Story 9-7).
//!
//! Owns the SIGHUP-triggered reload routine, the
//! `tokio::sync::watch::Sender<Arc<AppConfig>>` propagation channel, and
//! the knob-taxonomy classifier that distinguishes hot-reload-safe
//! changes from restart-required changes.
//!
//! # Validate-then-swap discipline
//!
//! Reload is **validate-first / atomic-swap**:
//!   1. Re-invoke the figment chain (TOML + `OPCGW_*` env overlay).
//!   2. Run [`AppConfig::validate`] on the candidate.
//!   3. Run the knob-taxonomy classifier; reject if any restart-required
//!      knob changed.
//!   4. On success, publish `Arc::new(candidate)` to the watch channel.
//!
//! If any step fails the watch channel is left untouched and an error
//! is returned to the caller, which logs an
//! `event="config_reload_failed"` warn with the appropriate
//! `reason ∈ {validation, io, restart_required}`. Story 9-0's spike
//! finding "transactional rollback is not required"
//! (`9-0-spike-report.md:196`) is what lets this be a simple
//! validate-then-swap rather than a multi-step transaction.
//!
//! # Knob taxonomy
//!
//! See `_bmad-output/implementation-artifacts/9-7-configuration-hot-reload.md`
//! § "Knob Taxonomy" for the canonical lists. Restart-required knobs are
//! whitelisted in [`classify_diff`]; everything else is treated as
//! hot-reload-safe (or address-space-mutating, which Story 9-8 picks up).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use figment::providers::{Env, Format, Toml};
use figment::Figment;
use tokio::sync::{watch, Mutex};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::config::{AppConfig, Global, CommandValidationConfig, LoggingConfig};
use crate::utils::OpcGwError;

/// Public outcome of a successful reload — the SIGHUP listener turns
/// this into the `event="config_reload_succeeded"` info-level log line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReloadOutcome {
    /// Candidate config equals the live config; no swap was performed.
    /// Still emitted as a `succeeded` event so the operator sees
    /// confirmation that the SIGHUP reached the gateway.
    NoChange,
    /// At least one section changed; the watch channel was updated.
    Changed {
        /// How many top-level config sections (`global`, `chirpstack`,
        /// `opcua`, `storage`, `command_validation`, `logging`, `web`,
        /// `application_list`) differ between old and new. Coarse but
        /// useful for "is this a one-knob tweak or a wholesale rewrite"
        /// triage.
        changed_section_count: usize,
        /// `true` iff `application_list` differs at the application/device/
        /// metric level. Story 9-7 logs the diff but does NOT mutate the
        /// OPC UA address space — Story 9-8 owns the apply.
        includes_topology_change: bool,
        /// Wall-clock cost of validate + classify + swap, in
        /// milliseconds. Excludes propagation latency to the
        /// downstream subscribers (which is bounded by their own
        /// loop cadence — see AC#5).
        duration_ms: u64,
    },
}

/// Public failure of a reload — the SIGHUP listener turns this into
/// the `event="config_reload_failed"` warn-level log line.
///
/// Spec forbids new `OpcGwError` enum variants; this is a sibling type
/// that wraps strings so the failed-event log can carry a structured
/// `reason` field without polluting the global error taxonomy.
#[derive(Debug, Clone, thiserror::Error)]
pub enum ReloadError {
    /// Figment failed to load + parse the TOML / env overlay (file
    /// missing, syntax error, deserialisation failure).
    #[error("config IO/parse error: {0}")]
    Io(String),
    /// `AppConfig::validate()` rejected the candidate (port out of
    /// range, empty credentials, invalid PKI permissions, etc.).
    #[error("config validation error: {0}")]
    Validation(String),
    /// A restart-required knob was changed; spec mandates the change
    /// is **rejected** rather than silently dropped.
    #[error("restart-required knob changed: {knob}")]
    RestartRequired { knob: String },
}

impl ReloadError {
    /// Stable string for the `reason=` field on the failed-event log
    /// line. Pinned by the docs/logging.md operations table — do not
    /// rename without updating the table at the same time.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Io(_) => "io",
            Self::Validation(_) => "validation",
            Self::RestartRequired { .. } => "restart_required",
        }
    }

    /// `Some(knob)` for `RestartRequired`; `None` otherwise. Used to
    /// populate the `changed_knob=` field on the failed-event log line
    /// (spec AC#3).
    pub fn changed_knob(&self) -> Option<&str> {
        match self {
            Self::RestartRequired { knob } => Some(knob.as_str()),
            _ => None,
        }
    }
}

impl From<ReloadError> for OpcGwError {
    fn from(e: ReloadError) -> Self {
        // Spec: "Reuse OpcGwError::Configuration(String) for
        // validation/restart-required failures, OpcGwError::Storage(...)
        // for IO failures." Keep the reason tag in the message so the
        // OpcGwError-side consumer (if any) can still see it.
        match &e {
            ReloadError::Io(_) => OpcGwError::Storage(e.to_string()),
            _ => OpcGwError::Configuration(e.to_string()),
        }
    }
}

/// Owns the watch channel and the path of the canonical config file.
/// Cloned once into the SIGHUP listener task; subsequent receivers are
/// produced via [`subscribe`](Self::subscribe).
pub struct ConfigReloadHandle {
    tx: watch::Sender<Arc<AppConfig>>,
    config_path: PathBuf,
    /// Serialises concurrent `reload()` calls. Without this, two
    /// near-simultaneous SIGHUPs could both call `tx.borrow().clone()`,
    /// classify against the same observed live config, and race on
    /// `tx.send(...)` — last writer wins regardless of which read
    /// raced first. Iter-1 review P7.
    reload_lock: Mutex<()>,
}

impl ConfigReloadHandle {
    /// Construct the handle with an initial config and the canonical
    /// TOML path. Returns `(handle, initial_receiver)` — the handle is
    /// retained by the SIGHUP listener; the receiver is cloned (via
    /// [`subscribe`](Self::subscribe) or `Receiver::clone`) into each
    /// subsystem (poller, web, OPC UA listener).
    pub fn new(
        initial: Arc<AppConfig>,
        config_path: PathBuf,
    ) -> (Self, watch::Receiver<Arc<AppConfig>>) {
        let (tx, rx) = watch::channel(initial);
        (
            Self {
                tx,
                config_path,
                reload_lock: Mutex::new(()),
            },
            rx,
        )
    }

    /// Mint a fresh receiver that observes future swaps. Equivalent to
    /// `initial_rx.clone()` in semantics; provided as an explicit
    /// method so subscribers don't have to thread the original
    /// receiver around.
    pub fn subscribe(&self) -> watch::Receiver<Arc<AppConfig>> {
        self.tx.subscribe()
    }

    /// Run a full reload cycle. Returns `Ok(ReloadOutcome)` on success
    /// (whether or not anything changed) or a structured
    /// [`ReloadError`] on failure. The watch channel is updated
    /// **only** on `Ok(Changed { .. })`.
    pub async fn reload(&self) -> Result<ReloadOutcome, ReloadError> {
        // Iter-1 review P7: serialise concurrent reloads. Two
        // near-simultaneous SIGHUPs would otherwise race on
        // borrow → classify → send with arbitrary "last writer wins"
        // ordering. Holding the lock across the await is safe — the
        // figment IO is bounded and reload is rare.
        let _guard = self.reload_lock.lock().await;

        let start = Instant::now();

        // Step 1+2: load + validate the candidate.
        let candidate = load_and_validate(&self.config_path)?;

        // Step 3: classify diff against the live config. `borrow()`
        // takes a read-lock on the watch state; we drop it before the
        // optional `send()` so there's no risk of self-deadlock if
        // the channel has subscribers polling concurrently.
        let live = self.tx.borrow().clone();
        let summary = classify_diff(&live, &candidate)?;

        if summary.changed_section_count == 0 {
            return Ok(ReloadOutcome::NoChange);
        }

        // Step 4: atomic swap. `send` returns Err only when there are
        // no receivers; in that case the watch channel is unmodified,
        // but the next subscriber created via `tx.subscribe()` will
        // observe the prior live value (correct: there's no consumer
        // who could have observed the new value, so dropping it is
        // semantically equivalent to never publishing).
        let _ = self.tx.send(Arc::new(candidate));

        Ok(ReloadOutcome::Changed {
            changed_section_count: summary.changed_section_count,
            includes_topology_change: summary.topology_changed,
            duration_ms: start.elapsed().as_millis() as u64,
        })
    }
}

/// Internal reload step 1+2 — re-invoke the figment chain (TOML +
/// `OPCGW_*` env overlay) and run `AppConfig::validate()`. Distinguishes
/// `io` vs `validation` failures so the failed-event log line can carry
/// the right `reason=` field.
fn load_and_validate(path: &std::path::Path) -> Result<AppConfig, ReloadError> {
    // Pre-flight existence check — gives a more specific error
    // than figment's "no provider had a value" wrapping. The
    // race against concurrent unlink is harmless: the figment
    // load below will surface its own IO error in that window.
    if !path.exists() {
        return Err(ReloadError::Io(format!(
            "config file not found: {}",
            path.display()
        )));
    }

    let path_str = path.to_string_lossy().into_owned();

    // Mirror `AppConfig::from_path` exactly so the env-var overlay
    // remains in effect (FR32 carry-forward).
    let candidate: AppConfig = Figment::new()
        .merge(Toml::file(&path_str))
        .merge(Env::prefixed("OPCGW_").split("__").global())
        .extract()
        .map_err(|e| ReloadError::Io(format!("figment load/parse failed: {e}")))?;

    candidate
        .validate()
        .map_err(|e| ReloadError::Validation(e.to_string()))?;

    Ok(candidate)
}

/// Coarse summary of what changed between two configs. Used internally
/// by [`ConfigReloadHandle::reload`] to decide whether to swap and to
/// populate the success-event log fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DiffSummary {
    pub(crate) changed_section_count: usize,
    pub(crate) topology_changed: bool,
}

/// Knob-taxonomy classifier. Walks the restart-required whitelist
/// first and returns
/// [`ReloadError::RestartRequired`](ReloadError::RestartRequired) on
/// the **first** offending knob (so the operator gets a single,
/// actionable line rather than a wall of "this also changed" noise).
/// On no restart-required violation, returns a [`DiffSummary`]
/// describing how many sections changed and whether the topology
/// (`application_list`) was modified.
///
/// Visible to integration tests via `pub(crate)`; not in the public
/// API surface.
pub(crate) fn classify_diff(
    old: &AppConfig,
    new: &AppConfig,
) -> Result<DiffSummary, ReloadError> {
    // ---------- restart-required: chirpstack ----------
    if old.chirpstack.server_address != new.chirpstack.server_address {
        return Err(ReloadError::RestartRequired {
            knob: "chirpstack.server_address".to_string(),
        });
    }
    if old.chirpstack.api_token != new.chirpstack.api_token {
        return Err(ReloadError::RestartRequired {
            knob: "chirpstack.api_token".to_string(),
        });
    }
    if old.chirpstack.tenant_id != new.chirpstack.tenant_id {
        return Err(ReloadError::RestartRequired {
            knob: "chirpstack.tenant_id".to_string(),
        });
    }

    // ---------- restart-required: opcua ----------
    if old.opcua.host_ip_address != new.opcua.host_ip_address {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.host_ip_address".to_string(),
        });
    }
    if old.opcua.host_port != new.opcua.host_port {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.host_port".to_string(),
        });
    }
    if old.opcua.application_name != new.opcua.application_name {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.application_name".to_string(),
        });
    }
    if old.opcua.application_uri != new.opcua.application_uri {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.application_uri".to_string(),
        });
    }
    if old.opcua.product_uri != new.opcua.product_uri {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.product_uri".to_string(),
        });
    }
    if old.opcua.pki_dir != new.opcua.pki_dir {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.pki_dir".to_string(),
        });
    }
    if old.opcua.certificate_path != new.opcua.certificate_path {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.certificate_path".to_string(),
        });
    }
    if old.opcua.private_key_path != new.opcua.private_key_path {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.private_key_path".to_string(),
        });
    }
    if old.opcua.max_connections != new.opcua.max_connections {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.max_connections".to_string(),
        });
    }
    if old.opcua.max_subscriptions_per_session != new.opcua.max_subscriptions_per_session {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.max_subscriptions_per_session".to_string(),
        });
    }
    if old.opcua.max_monitored_items_per_sub != new.opcua.max_monitored_items_per_sub {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.max_monitored_items_per_sub".to_string(),
        });
    }
    if old.opcua.max_message_size != new.opcua.max_message_size {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.max_message_size".to_string(),
        });
    }
    if old.opcua.max_chunk_count != new.opcua.max_chunk_count {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.max_chunk_count".to_string(),
        });
    }

    // ---------- restart-required: opcua credentials (v1 limitation) ----------
    //
    // Spec § "Auth-rotating" recommends rebuilding `WebAuthState` +
    // `OpcgwAuthManager` digests on credential change. Both are
    // captured at startup via constructors that this story is
    // forbidden from modifying (AC#8 invariants:
    // `src/web/auth.rs`, `src/opc_ua_auth.rs`, and
    // `src/opc_ua.rs` are file-invariant). Without those
    // modifications, swapping `AppState.auth` cannot influence the
    // already-installed middleware closure (it captured the original
    // Arc at router-build time). v1 therefore classifies credential
    // changes as **restart-required** so a hot-reload that bumps the
    // password is rejected loudly rather than silently ignored.
    // Future story: a middleware refactor to look up auth via
    // `State<Arc<AppState>>` would lift this restriction.
    if old.opcua.user_name != new.opcua.user_name {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.user_name".to_string(),
        });
    }
    if old.opcua.user_password != new.opcua.user_password {
        return Err(ReloadError::RestartRequired {
            knob: "opcua.user_password".to_string(),
        });
    }

    // ---------- restart-required: web ----------
    if old.web.port != new.web.port {
        return Err(ReloadError::RestartRequired {
            knob: "web.port".to_string(),
        });
    }
    if old.web.bind_address != new.web.bind_address {
        return Err(ReloadError::RestartRequired {
            knob: "web.bind_address".to_string(),
        });
    }
    if old.web.enabled != new.web.enabled {
        return Err(ReloadError::RestartRequired {
            knob: "web.enabled".to_string(),
        });
    }
    // `web.auth_realm` is captured into `WebAuthState.realm` (and into
    // `web::run`'s `realm` parameter for the WWW-Authenticate header)
    // at router-build time. Same v1 limitation as the opcua
    // credentials above — restart-required until the auth-state
    // refactor lands.
    if old.web.auth_realm != new.web.auth_realm {
        return Err(ReloadError::RestartRequired {
            knob: "web.auth_realm".to_string(),
        });
    }
    // Story 9-4: `web.allowed_origins` is captured into `CsrfState`
    // at router-build time. Same v1 limitation as `auth_realm` —
    // restart-required until the live-borrow refactor (#113) lands.
    if old.web.allowed_origins != new.web.allowed_origins {
        return Err(ReloadError::RestartRequired {
            knob: "web.allowed_origins".to_string(),
        });
    }

    // ---------- restart-required: storage ----------
    if old.storage.database_path != new.storage.database_path {
        return Err(ReloadError::RestartRequired {
            knob: "storage.database_path".to_string(),
        });
    }
    if old.storage.retention_days != new.storage.retention_days {
        return Err(ReloadError::RestartRequired {
            knob: "storage.retention_days".to_string(),
        });
    }

    // ---------- restart-required: global ----------
    //
    // Iter-1 review D4 / P24: every `[global]` knob is consumed at
    // startup by a long-running task (storage pruner, command-delivery
    // poller, command-timeout reaper, history retention pruner). Hot
    // reload would need per-task watch wiring matching what 9-7 did
    // for the chirpstack poller. Tracked in #114; v1 rejects loudly
    // rather than silently dropping the change.
    if old.global.debug != new.global.debug {
        return Err(ReloadError::RestartRequired {
            knob: "global.debug".to_string(),
        });
    }
    if old.global.prune_interval_minutes != new.global.prune_interval_minutes {
        return Err(ReloadError::RestartRequired {
            knob: "global.prune_interval_minutes".to_string(),
        });
    }
    if old.global.command_delivery_poll_interval_secs
        != new.global.command_delivery_poll_interval_secs
    {
        return Err(ReloadError::RestartRequired {
            knob: "global.command_delivery_poll_interval_secs".to_string(),
        });
    }
    if old.global.command_delivery_timeout_secs != new.global.command_delivery_timeout_secs {
        return Err(ReloadError::RestartRequired {
            knob: "global.command_delivery_timeout_secs".to_string(),
        });
    }
    if old.global.command_timeout_check_interval_secs
        != new.global.command_timeout_check_interval_secs
    {
        return Err(ReloadError::RestartRequired {
            knob: "global.command_timeout_check_interval_secs".to_string(),
        });
    }
    if old.global.history_retention_days != new.global.history_retention_days {
        return Err(ReloadError::RestartRequired {
            knob: "global.history_retention_days".to_string(),
        });
    }

    // ---------- restart-required: command_validation ----------
    //
    // Iter-1 review D4 / P24: spec § "Hot-reload-safe" originally listed
    // `commands.*` as hot-reload-safe but the validator captures
    // `Arc<CommandValidationConfig>` at startup; making it live-borrow
    // needs an audit of every read site. Tracked in #115.
    if old.command_validation.cache_ttl_secs != new.command_validation.cache_ttl_secs {
        return Err(ReloadError::RestartRequired {
            knob: "command_validation.cache_ttl_secs".to_string(),
        });
    }
    if old.command_validation.strict_precision_mode != new.command_validation.strict_precision_mode
    {
        return Err(ReloadError::RestartRequired {
            knob: "command_validation.strict_precision_mode".to_string(),
        });
    }
    if old.command_validation.default_string_max_length
        != new.command_validation.default_string_max_length
    {
        return Err(ReloadError::RestartRequired {
            knob: "command_validation.default_string_max_length".to_string(),
        });
    }
    // `device_schemas` is HashMap<String, Vec<CommandSchema>>.
    //
    // Iter-2 review P29: cannot rely on derived PartialEq here —
    // `ParameterType::Float { min: f64, max: f64 }` propagates
    // `f64::PartialEq` which says `NaN != NaN`. A config with
    // `min = nan` would make every reload appear to "change"
    // device_schemas, locking SIGHUP into permanent
    // `RestartRequired { knob: "command_validation.device_schemas" }`.
    // Use a NaN-safe bit-pattern comparison instead.
    if !device_schemas_equal(
        &old.command_validation.device_schemas,
        &new.command_validation.device_schemas,
    ) {
        return Err(ReloadError::RestartRequired {
            knob: "command_validation.device_schemas".to_string(),
        });
    }

    // ---------- restart-required: logging ----------
    //
    // Iter-1 review D4 / P24: `tracing-subscriber` captures the level
    // filter at construction time. Live mutation needs a
    // `tracing_subscriber::reload::Layer` handle that 9-7 did not wire
    // up. Tracked in #116. v1 rejects rather than silently dropping.
    if !logging_equal(&old.logging, &new.logging) {
        return Err(ReloadError::RestartRequired {
            knob: "logging".to_string(),
        });
    }

    // ---------- count changed sections (hot-reload-safe + topology) ----------
    //
    // Most config substructs do not derive `PartialEq` (they were never
    // intended to be compared field-by-field), so we serialise to TOML
    // and compare the resulting strings. This is O(config size) once
    // per reload — well below the cost of the figment load itself.
    // `toml::to_string` is infallible for `Serialize` types from
    // `serde::Deserialize`-derived structs; the gateway's config
    // structs all round-trip correctly.

    let mut changed_section_count: usize = 0;

    // Topology change is the load-bearing flag for Story 9-8 stub
    // logging — track it separately even though it also bumps the
    // section count.
    let topology_changed = !apps_equal(&old.application_list, &new.application_list);
    if topology_changed {
        changed_section_count += 1;
    }

    if !chirpstack_equal(&old.chirpstack, &new.chirpstack) {
        changed_section_count += 1;
    }
    if !opcua_equal(&old.opcua, &new.opcua) {
        changed_section_count += 1;
    }
    if !web_equal(&old.web, &new.web) {
        changed_section_count += 1;
    }
    if !storage_equal(&old.storage, &new.storage) {
        changed_section_count += 1;
    }
    // Iter-1 review D4 / P24: the per-field guards above already
    // reject any change in these sections as restart-required, so
    // reaching here means equality. The calls below exist purely to
    // keep the destructure-landmine helpers in the call graph (clippy
    // would flag them dead otherwise) and to leave a sentinel for the
    // future hot-reload upgrades tracked in #114 / #115 / #116.
    if !global_equal(&old.global, &new.global) {
        changed_section_count += 1;
    }
    if !command_validation_equal(&old.command_validation, &new.command_validation) {
        changed_section_count += 1;
    }
    if !logging_equal(&old.logging, &new.logging) {
        changed_section_count += 1;
    }

    Ok(DiffSummary {
        changed_section_count,
        topology_changed,
    })
}

// ----------------------------------------------------------------------
// Section-equality helpers.
//
// The config substructs do not derive `PartialEq`, so we compare only
// the fields the diff classifier actually cares about. Restart-required
// knobs are already handled above; these helpers cover the
// hot-reload-safe + auth-rotating fields per the knob taxonomy.
// ----------------------------------------------------------------------

fn chirpstack_equal(a: &crate::config::ChirpstackPollerConfig, b: &crate::config::ChirpstackPollerConfig) -> bool {
    // Restart-required fields (server_address, api_token, tenant_id)
    // are caught above; reaching here means they're equal. Compare the
    // hot-reload-safe knobs.
    //
    // Iter-2 review P28: destructure pattern landmine — if a future
    // field is added to `ChirpstackPollerConfig` without a matching
    // restart-required guard or hot-reload comparison, the destructure
    // here forces a compile error. Symmetric with web_equal /
    // storage_equal / global_equal / command_validation_equal.
    let crate::config::ChirpstackPollerConfig {
        server_address: _,
        api_token: _,
        tenant_id: _,
        polling_frequency,
        retry,
        delay,
        list_page_size,
    } = a;
    polling_frequency == &b.polling_frequency
        && retry == &b.retry
        && delay == &b.delay
        && list_page_size == &b.list_page_size
}

fn opcua_equal(a: &crate::config::OpcUaConfig, b: &crate::config::OpcUaConfig) -> bool {
    // Hot-reload-safe knobs only — restart-required ones (including
    // user_name/user_password per the v1 limitation) are caught
    // above and never reach this comparison.
    //
    // Iter-2 review P28: destructure pattern landmine. `OpcUaConfig`
    // is the largest config struct and the most likely to grow new
    // fields — without this destructure, additions would silently
    // bypass the classifier.
    let crate::config::OpcUaConfig {
        application_name: _,
        application_uri: _,
        product_uri: _,
        host_ip_address: _,
        host_port: _,
        diagnostics_enabled,
        create_sample_keypair,
        certificate_path: _,
        private_key_path: _,
        trust_client_cert,
        check_cert_time,
        pki_dir: _,
        user_name: _,
        user_password: _,
        stale_threshold_seconds,
        max_connections: _,
        max_subscriptions_per_session: _,
        max_monitored_items_per_sub: _,
        max_message_size: _,
        max_chunk_count: _,
        hello_timeout,
        max_history_data_results_per_node,
    } = a;
    stale_threshold_seconds == &b.stale_threshold_seconds
        && diagnostics_enabled == &b.diagnostics_enabled
        && hello_timeout == &b.hello_timeout
        && create_sample_keypair == &b.create_sample_keypair
        && trust_client_cert == &b.trust_client_cert
        && check_cert_time == &b.check_cert_time
        && max_history_data_results_per_node == &b.max_history_data_results_per_node
}

fn web_equal(a: &crate::config::WebConfig, b: &crate::config::WebConfig) -> bool {
    // Iter-1 review P2: destructure pattern forces a compile error if
    // a future field is added to `WebConfig` without a matching
    // restart-required guard or hot-reload comparison being added
    // here. All current fields are restart-required (caught above).
    let crate::config::WebConfig {
        port: _,
        bind_address: _,
        enabled: _,
        auth_realm: _,
        allowed_origins: _,
    } = a;
    let _ = b;
    true
}

fn storage_equal(a: &crate::config::StorageConfig, b: &crate::config::StorageConfig) -> bool {
    // Iter-1 review P3: same destructure-landmine pattern as web_equal.
    // All current fields are restart-required (caught above); future
    // hot-reload-safe knobs add comparisons here.
    let crate::config::StorageConfig {
        database_path: _,
        retention_days: _,
    } = a;
    let _ = b;
    true
}

/// Iter-1 review D4 / P24: destructure-landmine helper. Every `[global]`
/// knob is restart-required in v1 (caught by per-field guards in
/// `classify_diff`); reaching this helper means the section is equal.
/// The destructure pattern forces a compile error if a future field is
/// added to `Global` without a matching guard above. Tracked in #114
/// for the future hot-reload upgrade.
fn global_equal(a: &Global, b: &Global) -> bool {
    let Global {
        debug: _,
        prune_interval_minutes: _,
        command_delivery_poll_interval_secs: _,
        command_delivery_timeout_secs: _,
        command_timeout_check_interval_secs: _,
        history_retention_days: _,
    } = a;
    let _ = b;
    true
}

/// Iter-1 review D4 / P24: same destructure-landmine pattern as
/// `global_equal`. All `[command_validation]` fields are restart-required
/// in v1; tracked in #115 for the future hot-reload upgrade.
fn command_validation_equal(
    a: &CommandValidationConfig,
    b: &CommandValidationConfig,
) -> bool {
    let CommandValidationConfig {
        cache_ttl_secs: _,
        strict_precision_mode: _,
        default_string_max_length: _,
        device_schemas: _,
    } = a;
    let _ = b;
    true
}

/// Iter-2 review P29: NaN-safe equality for the
/// `command_validation.device_schemas` HashMap. Walks the structure
/// and compares `f64` fields via `to_bits()` so `NaN` compares equal
/// to itself with the same bit pattern. Without this, derived
/// `PartialEq` on `ParameterType::Float` propagates `f64::PartialEq`'s
/// `NaN != NaN` semantics and a config with `min = nan` would make
/// SIGHUP reload fail on every cycle.
fn device_schemas_equal(
    a: &std::collections::HashMap<String, Vec<crate::command_validation::CommandSchema>>,
    b: &std::collections::HashMap<String, Vec<crate::command_validation::CommandSchema>>,
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().all(|(k, va)| match b.get(k) {
        None => false,
        Some(vb) => schema_list_equal(va, vb),
    })
}

fn schema_list_equal(
    a: &[crate::command_validation::CommandSchema],
    b: &[crate::command_validation::CommandSchema],
) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| schema_equal(x, y))
}

fn schema_equal(
    a: &crate::command_validation::CommandSchema,
    b: &crate::command_validation::CommandSchema,
) -> bool {
    // Destructure landmine pattern — future field additions force
    // compile errors here.
    let crate::command_validation::CommandSchema {
        command_name,
        parameters,
        description,
    } = a;
    command_name == &b.command_name
        && description == &b.description
        && param_list_equal(parameters, &b.parameters)
}

fn param_list_equal(
    a: &[crate::command_validation::ParameterDef],
    b: &[crate::command_validation::ParameterDef],
) -> bool {
    a.len() == b.len()
        && a.iter()
            .zip(b.iter())
            .all(|(x, y)| param_equal(x, y))
}

fn param_equal(
    a: &crate::command_validation::ParameterDef,
    b: &crate::command_validation::ParameterDef,
) -> bool {
    let crate::command_validation::ParameterDef {
        name,
        param_type,
        required,
        description,
    } = a;
    name == &b.name
        && required == &b.required
        && description == &b.description
        && param_type_equal(param_type, &b.param_type)
}

/// NaN-safe `ParameterType` equality. The Float variant compares
/// `f64` bounds via `to_bits()` so two NaNs with the same bit
/// pattern compare equal (whereas `f64::PartialEq` returns false).
fn param_type_equal(
    a: &crate::command_validation::ParameterType,
    b: &crate::command_validation::ParameterType,
) -> bool {
    use crate::command_validation::ParameterType::*;
    match (a, b) {
        (String { max_length: a_ml }, String { max_length: b_ml }) => a_ml == b_ml,
        (Int { min: a_min, max: a_max }, Int { min: b_min, max: b_max }) => {
            a_min == b_min && a_max == b_max
        }
        (
            Float {
                min: a_min,
                max: a_max,
                decimal_places: a_dp,
            },
            Float {
                min: b_min,
                max: b_max,
                decimal_places: b_dp,
            },
        ) => {
            a_min.to_bits() == b_min.to_bits()
                && a_max.to_bits() == b_max.to_bits()
                && a_dp == b_dp
        }
        (Bool, Bool) => true,
        (
            Enum {
                values: a_v,
                case_sensitive: a_cs,
            },
            Enum {
                values: b_v,
                case_sensitive: b_cs,
            },
        ) => a_v == b_v && a_cs == b_cs,
        // Iter-3 review P42: cross-variant pairs are not equal.
        // Enumerated explicitly (rather than `_ => false`) so adding
        // a future `ParameterType` variant produces a
        // non-exhaustive-match compile error instead of silently
        // returning false on any same-variant pair containing the
        // new variant — which would have made `device_schemas_equal`
        // always reject same-content schemas as inequal, locking
        // SIGHUP into permanent `RestartRequired`.
        (String { .. }, _)
        | (Int { .. }, _)
        | (Float { .. }, _)
        | (Bool, _)
        | (Enum { .. }, _) => false,
    }
}

/// Iter-1 review D4 / P24: equality for `[logging]`. Compares the
/// outer `Option` and (when both `Some`) every field of `LoggingConfig`
/// via destructure pattern. Tracked in #116 for the future log-level
/// hot-reload upgrade.
fn logging_equal(a: &Option<LoggingConfig>, b: &Option<LoggingConfig>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => {
            let LoggingConfig { dir, level } = a;
            dir == &b.dir && level == &b.level
        }
        (None, None) => true,
        _ => false,
    }
}

fn apps_equal(
    a: &[crate::config::ChirpStackApplications],
    b: &[crate::config::ChirpStackApplications],
) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(x, y)| {
        // Iter-2 review P30: destructure pattern landmine — any
        // future field added to `ChirpStackApplications` must be
        // explicitly compared here. (Order-insensitivity at this
        // level is left as a follow-up; reordering apps in the TOML
        // is rare in practice and not currently flagged in spec.)
        let crate::config::ChirpStackApplications {
            application_id,
            application_name,
            device_list,
        } = x;
        application_id == &y.application_id
            && application_name == &y.application_name
            && devices_equal(device_list, &y.device_list)
    })
}

fn devices_equal(a: &[crate::config::ChirpstackDevice], b: &[crate::config::ChirpstackDevice]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b.iter()).all(|(x, y)| {
        // Iter-1 review P1: destructure pattern forces a compile
        // error if a future field is added to `ChirpstackDevice`.
        // All current fields are compared explicitly.
        let crate::config::ChirpstackDevice {
            device_id,
            device_name,
            read_metric_list,
            device_command_list,
        } = x;
        device_id == &y.device_id
            && device_name == &y.device_name
            && metrics_equal(read_metric_list, &y.read_metric_list)
            && command_list_equal(device_command_list, &y.device_command_list)
    })
}

/// Iter-1 review P1: device command-list equality. Order-insensitive
/// comparison keyed by `command_id` so reordering the TOML entries
/// doesn't trigger a spurious topology-change log.
///
/// Iter-2 review P27: treat `None` as semantically equivalent to
/// `Some([])` — both represent "no commands". A user who edits the
/// TOML to remove an empty `[[application.device.command]]` block
/// (collapsing `Some([])` → `None`) should not trigger a spurious
/// topology-change log.
fn command_list_equal(
    a: &Option<Vec<crate::config::DeviceCommandCfg>>,
    b: &Option<Vec<crate::config::DeviceCommandCfg>>,
) -> bool {
    fn as_slice(opt: &Option<Vec<crate::config::DeviceCommandCfg>>) -> &[crate::config::DeviceCommandCfg] {
        opt.as_deref().unwrap_or(&[])
    }
    let a = as_slice(a);
    let b = as_slice(b);
    if a.len() != b.len() {
        return false;
    }
    let mut a: Vec<&crate::config::DeviceCommandCfg> = a.iter().collect();
    let mut b: Vec<&crate::config::DeviceCommandCfg> = b.iter().collect();
    a.sort_by_key(|c| c.command_id);
    b.sort_by_key(|c| c.command_id);
    a.iter().zip(b.iter()).all(|(x, y)| {
        let crate::config::DeviceCommandCfg {
            command_id,
            command_name,
            command_confirmed,
            command_port,
        } = *x;
        command_id == &y.command_id
            && command_name == &y.command_name
            && command_confirmed == &y.command_confirmed
            && command_port == &y.command_port
    })
}

fn metrics_equal(a: &[crate::config::ReadMetric], b: &[crate::config::ReadMetric]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    // Iter-1 review P14: order-insensitive comparison. Sorting by
    // `metric_name` (which is the OPC UA NodeId discriminator and is
    // unique within a device) so a TOML reorder of semantically
    // equivalent entries doesn't trigger a spurious topology-change
    // log line.
    let mut a: Vec<&crate::config::ReadMetric> = a.iter().collect();
    let mut b: Vec<&crate::config::ReadMetric> = b.iter().collect();
    a.sort_by(|x, y| x.metric_name.cmp(&y.metric_name));
    b.sort_by(|x, y| x.metric_name.cmp(&y.metric_name));
    a.iter().zip(b.iter()).all(|(x, y)| {
        // Destructure pattern: future fields force compile error.
        let crate::config::ReadMetric {
            metric_name,
            chirpstack_metric_name,
            metric_type,
            metric_unit,
        } = *x;
        metric_name == &y.metric_name
            && chirpstack_metric_name == &y.chirpstack_metric_name
            && metric_type == &y.metric_type
            && metric_unit == &y.metric_unit
    })
}

// ---------------------------------------------------------------------------
// Subsystem listeners
// ---------------------------------------------------------------------------
//
// `main.rs` spawns one listener task per subsystem that needs to react to
// hot-reloads. The poller has its own integrated `config_rx` arm in its
// outer-loop `tokio::select!` (Task 3); the web + OPC UA subsystems use the
// listener helpers below.
//
// Issue #110 constraint: every listener cooperates with `cancel_token`
// explicitly via `tokio::select!` — no RAII drop reliance. `RunHandles`
// has no `Drop` impl (rustc E0509 blocks adding one while `run_handles`
// destructures the struct), so cancellation is the only correct shutdown
// signal.

/// Story 9-7 Task 4 — web subsystem hot-reload listener.
///
/// On every `config_rx.changed()` notification, atomically swaps the
/// fields of `AppState` that depend on `AppConfig`:
///   - `dashboard_snapshot` ← fresh `DashboardConfigSnapshot::from_config(&new)`
///   - `stale_threshold_secs` ← `clamp_stale_threshold(...)` of the new value
///
/// `auth` is **not** swapped (v1 limitation — see
/// [`classify_diff`] for the rationale). Credential rotation is
/// caught by the classifier as a restart-required knob change and
/// rejected at the SIGHUP entry point, so by the time this listener
/// receives a new config the credentials are guaranteed unchanged.
///
/// Loops until `cancel_token` is fired or the watch sender is dropped.
pub async fn run_web_config_listener(
    app_state: Arc<crate::web::AppState>,
    mut config_rx: watch::Receiver<Arc<AppConfig>>,
    cancel_token: CancellationToken,
) {
    // Iter-2 review P31: consume the initial publish so the first
    // `changed()` waits for the NEXT swap rather than firing
    // immediately. `tokio::sync::watch::Receiver::subscribe()` returns
    // a receiver that has NOT marked the current value as seen, so
    // without this `borrow_and_update()` the listener would emit a
    // spurious `config_reload_applied` log line at startup.
    let _ = config_rx.borrow_and_update();
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(
                    operation = "config_reload_listener_stopped",
                    subsystem = "web",
                    "Web config-listener stopping (cancel)"
                );
                return;
            }
            changed = config_rx.changed() => {
                if changed.is_err() {
                    // Iter-1 review P15: sender dropped without
                    // cancel-token fire = anomalous (hot-reload now
                    // permanently broken for this subsystem). warn!
                    // not info! so the operator sees it.
                    warn!(
                        operation = "config_reload_listener_stopped",
                        subsystem = "web",
                        "Web config-listener stopping (sender dropped without cancel)"
                    );
                    return;
                }
                let new_config = config_rx.borrow_and_update().clone();

                // Build the fresh dashboard snapshot off the new
                // config and atomically swap. The brief write-lock
                // window blocks any concurrent reader for the
                // duration of one Arc-replace; sub-microsecond cost.
                let new_snapshot = Arc::new(crate::web::DashboardConfigSnapshot::from_config(
                    &new_config,
                ));
                {
                    // Iter-1 review P5: recover from poison rather
                    // than panic the listener task. A single
                    // poisoned lock would otherwise kill the
                    // listener and break hot-reload for the rest of
                    // the process lifetime.
                    //
                    // Iter-2 review P34: surface poison-recovery to
                    // operator audit log so the panic that caused
                    // the poison can be correlated with subsequent
                    // hot-reload behaviour.
                    let mut guard = app_state
                        .dashboard_snapshot
                        .write()
                        .unwrap_or_else(|e| {
                            warn!(
                                operation = "rwlock_poison_recovered",
                                site = "config_reload_listener",
                                "dashboard_snapshot RwLock was poisoned; recovering inner \
                                 value (a prior holder panicked — investigate)"
                            );
                            e.into_inner()
                        });
                    *guard = new_snapshot;
                }

                // Resolve + clamp the stale threshold. Any clamp event
                // is logged at warn level here (rather than inside the
                // helper) so the operator-action text can name the
                // hot-reload trigger explicitly.
                let raw_threshold = new_config
                    .opcua
                    .stale_threshold_seconds
                    .unwrap_or(crate::web::api::DEFAULT_STALE_THRESHOLD_SECS);
                let (clamped, outcome) = crate::web::clamp_stale_threshold(raw_threshold);
                if outcome != crate::web::StaleThresholdClampOutcome::Accepted {
                    tracing::warn!(
                        operation = "stale_threshold_clamped",
                        trigger = "hot_reload",
                        configured = raw_threshold,
                        clamped_to = clamped,
                        "Hot-reloaded [opcua].stale_threshold_seconds outside the \
                         valid (0, 86400] band; clamping to default for the web \
                         dashboard's 'uncertain' band"
                    );
                }
                app_state
                    .stale_threshold_secs
                    .store(clamped, std::sync::atomic::Ordering::Relaxed);

                info!(
                    operation = "config_reload_applied",
                    subsystem = "web",
                    stale_threshold_secs = clamped,
                    "Web subsystem picked up reloaded config"
                );
            }
        }
    }
}

/// Story 9-7 Task 5 + **Story 9-8 Tasks 2-5** — OPC UA subsystem
/// hot-reload listener with end-to-end address-space mutation apply.
///
/// On every `config_rx.changed()` notification:
///
///  1. **Topology diff log** (Story 9-7 backward compat): emits
///     `event="topology_change_detected"` info-level event with the
///     four 9-7-pinned axis counts (`added_devices`, `removed_devices`,
///     `modified_devices`, `story_9_8_seam`) so the existing
///     `tests/config_hot_reload.rs::topology_change_logs_seam_for_9_8`
///     integration test continues to pass byte-for-byte.
///  2. **Apply pass** (Story 9-8): calls
///     [`crate::opcua_topology_apply::apply_diff_to_address_space`]
///     which walks the 7-axis `AddressSpaceDiff` (added/removed
///     applications/devices/metrics/commands + renamed_devices) and
///     applies the four-phase mutation envelope (Q2 set_values
///     mitigation → delete → add → DisplayName rename).
///  3. **Apply outcome log** (Story 9-8): emits
///     `event="address_space_mutation_succeeded"` info on success
///     (with all 9 axis counts + `duration_ms`) or
///     `event="address_space_mutation_failed"` warn on failure (with
///     `reason` + sanitised `error: %e` field).
///
/// Story 9-7's documented v1 limitation ("dashboard updates but OPC UA
/// stays frozen") is closed by this implementation — topology
/// hot-reload is now end-to-end functional and FR24 is satisfied.
///
/// Loops until `cancel_token` is fired or the watch sender is dropped.
///
/// **Issue #110 carry-forward**: the listener cooperates with
/// `cancel_token.cancel()` explicitly — no RAII drop reliance.
#[allow(clippy::too_many_arguments)]
pub async fn run_opcua_config_listener(
    manager: Arc<crate::opc_ua_history::OpcgwHistoryNodeManager>,
    subscriptions: Arc<opcua::server::SubscriptionCache>,
    storage: Arc<dyn crate::storage::StorageBackend>,
    last_status: crate::opc_ua::StatusCache,
    node_to_metric: Arc<
        opcua::sync::RwLock<
            std::collections::HashMap<opcua::types::NodeId, (String, String)>,
        >,
    >,
    ns: u16,
    initial: Arc<AppConfig>,
    mut config_rx: watch::Receiver<Arc<AppConfig>>,
    cancel_token: CancellationToken,
) {
    // Iter-2 review P31: consume the initial publish so the first
    // `changed()` waits for the next SIGHUP rather than firing
    // immediately on a freshly-subscribed receiver.
    let _ = config_rx.borrow_and_update();
    let mut prev = initial;
    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(
                    operation = "config_reload_listener_stopped",
                    subsystem = "opcua",
                    "OPC UA config-listener stopping (cancel)"
                );
                return;
            }
            changed = config_rx.changed() => {
                if changed.is_err() {
                    // Iter-1 review P15: sender dropped without
                    // cancel-token fire = anomalous. warn! not info!.
                    warn!(
                        operation = "config_reload_listener_stopped",
                        subsystem = "opcua",
                        "OPC UA config-listener stopping (sender dropped without cancel)"
                    );
                    return;
                }
                let new_config = config_rx.borrow_and_update().clone();

                // Step 1 — emit the Story-9-7-pinned diff log for
                // backward compat with the integration test
                // `topology_change_logs_seam_for_9_8`.
                log_topology_diff(&prev, &new_config);

                // Step 2 — Story 9-8 apply pass. Reads the new config's
                // staleness threshold so runtime-added closures
                // capture the live value at add time (note: existing
                // closures from earlier adds keep their captured
                // threshold — issue #113 carry-forward).
                let stale_threshold = new_config
                    .opcua
                    .stale_threshold_seconds
                    .unwrap_or(crate::opc_ua::DEFAULT_STALE_THRESHOLD_SECS);
                let outcome = crate::opcua_topology_apply::apply_diff_to_address_space(
                    &prev,
                    &new_config,
                    &manager,
                    &subscriptions,
                    &storage,
                    &last_status,
                    &node_to_metric,
                    ns,
                    stale_threshold,
                );

                // Step 3 — apply outcome audit event + prev advancement.
                //
                // Iter-2 review IP1 (Edge E-H1-iter2 + Blind B-H1-iter2
                // converged HIGH-REG): the iter-1 P2 "keep prev on any
                // Failed" guard combined with iter-1 P1's
                // None-as-failure capture created an unrecoverable
                // replay loop: Phase 2/3 partial-fail → P2 keeps
                // prev → next reload re-computes same diff → Phase 2
                // hits already-deleted NodeIds → P1 routes to
                // Failed{REMOVE_FAILED} → loop forever.
                //
                // Refined iter-2 semantics — advance `prev` UNLESS
                // the failure reason is SET_ATTRIBUTES_FAILED (the
                // ONLY reason now emitted from Phase 1, since
                // iter-2 IP1 demoted Phase 4's rename-failure to
                // warn-and-continue). Phase 1 failure means nothing
                // was committed to the address space; retry with
                // the same prev is correct. All other Failed paths
                // (REMOVE_FAILED from Phase 2, ADD_FAILED from
                // Phase 3) imply partial commit — advance prev to
                // avoid the retry loop; the operator-visible
                // Failed event tells them what to investigate.
                let mutation_succeeded = match &outcome {
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::NoChange
                    | crate::opcua_topology_apply::AddressSpaceMutationOutcome::Applied {
                        ..
                    } => true,
                    // Phase 1 failure: nothing committed → keep
                    // prev so retry has a chance.
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::Failed {
                        reason,
                        ..
                    } if *reason
                        == crate::opcua_topology_apply::failure_reason::SET_ATTRIBUTES_FAILED =>
                    {
                        false
                    }
                    // Phase 2/3 partial-failure: some mutations
                    // committed → advance prev to avoid replay loop
                    // (per iter-2 IP1).
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::Failed {
                        ..
                    } => true,
                };
                match outcome {
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::NoChange => {
                        // Diff was empty — already logged via
                        // log_topology_diff (which returns false and
                        // emits nothing on no-changes); nothing more
                        // to emit here.
                    }
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::Applied {
                        counts,
                        duration_ms,
                    } => {
                        info!(
                            event = "address_space_mutation_succeeded",
                            added_applications = counts.added_applications,
                            removed_applications = counts.removed_applications,
                            added_devices = counts.added_devices,
                            removed_devices = counts.removed_devices,
                            added_metrics = counts.added_metrics,
                            removed_metrics = counts.removed_metrics,
                            added_commands = counts.added_commands,
                            removed_commands = counts.removed_commands,
                            renamed_devices = counts.renamed_devices,
                            duration_ms,
                            "OPC UA address space mutated per topology diff"
                        );
                    }
                    crate::opcua_topology_apply::AddressSpaceMutationOutcome::Failed {
                        counts,
                        duration_ms,
                        reason,
                        error,
                    } => {
                        // Iter-3 review TP1 (Edge E-H1-iter3): make the
                        // warn message reason-aware. After iter-2 IP1
                        // refined the prev-advancement guard, only
                        // Phase 1 (SET_ATTRIBUTES_FAILED) failures
                        // keep prev unchanged; Phase 2/3 partial
                        // failures advance prev to avoid the replay
                        // loop. The prior message text said "prev not
                        // advanced; retry to converge" for ALL Failed
                        // outcomes, which mis-directed operators
                        // following a Phase 2/3 partial failure (where
                        // prev IS advanced and retry would see an
                        // empty diff).
                        let retry_hint = if reason
                            == crate::opcua_topology_apply::failure_reason::SET_ATTRIBUTES_FAILED
                        {
                            "prev not advanced — retry SIGHUP / CRUD reload to converge \
                             (Phase 1 failure means no mutations were committed)"
                        } else {
                            "prev advanced — address space may be in partial-apply state; \
                             inspect counts + error and reconcile manually if needed \
                             (subsequent reloads will not re-attempt this diff)"
                        };
                        warn!(
                            event = "address_space_mutation_failed",
                            reason,
                            error,
                            duration_ms,
                            added_applications = counts.added_applications,
                            removed_applications = counts.removed_applications,
                            added_devices = counts.added_devices,
                            removed_devices = counts.removed_devices,
                            added_metrics = counts.added_metrics,
                            removed_metrics = counts.removed_metrics,
                            added_commands = counts.added_commands,
                            removed_commands = counts.removed_commands,
                            renamed_devices = counts.renamed_devices,
                            "OPC UA address-space mutation failed: {retry_hint}"
                        );
                    }
                }

                if mutation_succeeded {
                    prev = new_config;
                }
            }
        }
    }
}

/// Iter-1 review P23 — emits the `event="topology_change_detected"`
/// info log carrying `added_devices` / `removed_devices` /
/// `modified_devices` field counts when the topology has changed.
///
/// Returns `true` if the log was emitted (i.e., the topology actually
/// differs); `false` for an unchanged topology. Public so integration
/// tests for AC#4 can drive the emission without standing up a full
/// `OpcgwHistoryNodeManager`.
pub fn log_topology_diff(prev: &AppConfig, new: &AppConfig) -> bool {
    let diff = topology_device_diff(prev, new);
    if diff.has_changes() {
        // Iter-1 review P6: spec AC#4 mandates `event=` on this audit
        // line (the SIGHUP listener also uses `event=`). Was
        // `operation=` before.
        info!(
            event = "topology_change_detected",
            added_devices = diff.added,
            removed_devices = diff.removed,
            modified_devices = diff.modified,
            story_9_8_seam = true,
            "Topology change detected; Story 9-8 owns the address-space apply"
        );
        true
    } else {
        false
    }
}

/// Coarse device-level topology diff between two configs. Identifies
/// devices by `(application_id, device_id)`; "modified" means the
/// device exists in both with the same composite ID but its
/// `read_metric_list` or `device_name` differs.
struct TopologyDeviceDiff {
    added: usize,
    removed: usize,
    modified: usize,
}

impl TopologyDeviceDiff {
    fn has_changes(&self) -> bool {
        self.added != 0 || self.removed != 0 || self.modified != 0
    }
}

fn topology_device_diff(old: &AppConfig, new: &AppConfig) -> TopologyDeviceDiff {
    use std::collections::HashMap;

    type DeviceKey = (String, String);

    fn collect_devices(cfg: &AppConfig) -> HashMap<DeviceKey, &crate::config::ChirpstackDevice> {
        let mut map = HashMap::new();
        for app in &cfg.application_list {
            for dev in &app.device_list {
                map.insert((app.application_id.clone(), dev.device_id.clone()), dev);
            }
        }
        map
    }

    let old_devices = collect_devices(old);
    let new_devices = collect_devices(new);

    let mut added = 0usize;
    let mut removed = 0usize;
    let mut modified = 0usize;

    for (key, new_dev) in &new_devices {
        match old_devices.get(key) {
            None => added += 1,
            Some(old_dev) => {
                // Iter-2 review P26: include `device_command_list` in
                // the modified-device count so this helper agrees
                // with `classify_diff` (which also flags command-list
                // edits as topology changes via `command_list_equal`).
                // Without this, a SIGHUP that mutates only commands
                // would set `includes_topology_change=true` in the
                // ReloadOutcome but emit NO `topology_change_detected`
                // log — Story 9-8 would silently drop the change.
                if old_dev.device_name != new_dev.device_name
                    || !metrics_equal(&old_dev.read_metric_list, &new_dev.read_metric_list)
                    || !command_list_equal(
                        &old_dev.device_command_list,
                        &new_dev.device_command_list,
                    )
                {
                    modified += 1;
                }
            }
        }
    }
    for key in old_devices.keys() {
        if !new_devices.contains_key(key) {
            removed += 1;
        }
    }

    TopologyDeviceDiff {
        added,
        removed,
        modified,
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

    /// Build a minimal valid `AppConfig` for the classifier tests.
    /// Mirrors the explicit-field pattern used by `src/web/mod.rs::tests`
    /// because the substructs do not derive `Default`.
    fn baseline() -> AppConfig {
        AppConfig {
            global: Global {
                debug: false,
                prune_interval_minutes: 60,
                command_delivery_poll_interval_secs: 5,
                command_delivery_timeout_secs: 60,
                command_timeout_check_interval_secs: 10,
                history_retention_days: 7,
            },
            logging: None,
            chirpstack: ChirpstackPollerConfig {
                server_address: "http://127.0.0.1:18080".to_string(),
                api_token: "tok".to_string(),
                tenant_id: "00000000-0000-0000-0000-000000000000".to_string(),
                polling_frequency: 10,
                retry: 30,
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
                host_port: Some(4855),
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
            application_list: vec![ChirpStackApplications {
                application_name: "App1".to_string(),
                application_id: "550e8400-e29b-41d4-a716-446655440001".to_string(),
                device_list: vec![ChirpstackDevice {
                    device_id: "dev-1".to_string(),
                    device_name: "Dev 1".to_string(),
                    read_metric_list: vec![ReadMetric {
                        metric_name: "temp".to_string(),
                        chirpstack_metric_name: "t".to_string(),
                        metric_type: OpcMetricTypeConfig::Float,
                        metric_unit: None,
                    }],
                    device_command_list: None,
                }],
            }],
        }
    }

    /// AC#1 / Task 1 unit test — equal configs produce zero
    /// changed-section count, so the reload routine returns NoChange
    /// and never updates the watch channel.
    #[test]
    fn classify_diff_ignores_equal_configs() {
        let a = baseline();
        let b = baseline();
        let summary = classify_diff(&a, &b).expect("equal configs must classify cleanly");
        assert_eq!(summary.changed_section_count, 0);
        assert!(!summary.topology_changed);
    }

    /// AC#3 / Task 1 unit test — host_port is in the restart-required
    /// whitelist; classify_diff returns RestartRequired with the
    /// offending knob name.
    #[test]
    fn classify_diff_rejects_host_port_change() {
        let a = baseline();
        let mut b = baseline();
        b.opcua.host_port = Some(4856);
        let err = classify_diff(&a, &b)
            .expect_err("host_port change must be rejected as restart-required");
        match err {
            ReloadError::RestartRequired { knob } => {
                assert_eq!(knob, "opcua.host_port");
            }
            other => panic!("expected RestartRequired, got {other:?}"),
        }
    }

    /// AC#2 / Task 1 unit test — chirpstack.retry is hot-reload-safe;
    /// classify_diff accepts the change and reports
    /// `changed_section_count = 1`, `topology_changed = false`.
    #[test]
    fn classify_diff_accepts_retry_change() {
        let a = baseline();
        let mut b = baseline();
        b.chirpstack.retry = 5;
        let summary = classify_diff(&a, &b)
            .expect("retry change must classify as hot-reload-safe");
        assert_eq!(summary.changed_section_count, 1);
        assert!(!summary.topology_changed);
    }

    /// Task 1 sanity — application_list mutation flips
    /// `topology_changed` and bumps `changed_section_count`. Story
    /// 9-7 logs but does not apply this; Story 9-8 picks up the apply.
    #[test]
    fn classify_diff_topology_change_sets_flag() {
        let a = baseline();
        let mut b = baseline();
        b.application_list[0].device_list.push(ChirpstackDevice {
            device_id: "dev-2".to_string(),
            device_name: "Dev 2".to_string(),
            read_metric_list: vec![],
            device_command_list: None,
        });
        let summary = classify_diff(&a, &b).expect("topology change must classify cleanly");
        assert!(summary.topology_changed);
        assert!(summary.changed_section_count >= 1);
    }

    /// Sanity — `ReloadError::reason()` returns the stable strings
    /// pinned by the `docs/logging.md` operations table.
    #[test]
    fn reload_error_reason_strings_are_stable() {
        assert_eq!(ReloadError::Io("x".into()).reason(), "io");
        assert_eq!(ReloadError::Validation("x".into()).reason(), "validation");
        assert_eq!(
            ReloadError::RestartRequired {
                knob: "k".into()
            }
            .reason(),
            "restart_required"
        );
    }

    /// Sanity — `changed_knob()` is `Some` only for the
    /// `RestartRequired` variant (drives the `changed_knob=` field
    /// on the failed-event log line — AC#3).
    #[test]
    fn reload_error_changed_knob_only_set_for_restart_required() {
        assert_eq!(ReloadError::Io("x".into()).changed_knob(), None);
        assert_eq!(ReloadError::Validation("x".into()).changed_knob(), None);
        assert_eq!(
            ReloadError::RestartRequired {
                knob: "opcua.host_port".into()
            }
            .changed_knob(),
            Some("opcua.host_port"),
        );
    }
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Global Utilities Module
//!
//! This module provides shared constants, error types, and utility functions
//! used throughout the OPC UA ChirpStack Gateway application. It serves as
//! a centralized location for:
//!
//! - Application-wide constants and configuration defaults
//! - Common error types and error handling
//! - Utility functions for debugging and development
//!
//! # Constants Organization
//!
//! Constants are organized into logical groups:
//! - **OPC UA Configuration**: Default network settings and identifiers
//! - **File System Paths**: Configuration file locations
//! - **ChirpStack Integration**: Metric names and identifiers for ChirpStack server monitoring
//!
//! # Error Handling
//!
//! The module defines a comprehensive error enum that covers all major
//! error categories in the application, enabling consistent error handling
//! across different components.

#![allow(unused)]

use std::string::ToString;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

// =============================================================================
// OPC UA Configuration Constants
// =============================================================================

/// OPC UA address space identifier for the ChirpStack gateway.
///
/// This URI uniquely identifies the OPC UA address space used by the gateway
/// server. It is used in OPC UA client discovery and server identification.
///
/// # Usage
///
/// This constant is typically used when:
/// - Configuring the OPC UA server application identity
/// - Setting up the server's address space namespace
/// - Client discovery and connection establishment
pub const OPCUA_ADDRESS_SPACE: &str = "urn:chirpstack_opcua";

/// OPC UA namespace URI for the gateway application.
///
/// This URI defines the namespace for all OPC UA nodes created by the gateway.
/// It ensures that node identifiers are unique and properly scoped within
/// the OPC UA information model.
///
/// # Note
///
/// The current value "urn:UpcUaG" appears to be a shortened form and may
/// need to be updated to a more descriptive URI in future versions.
pub const OPCUA_NAMESPACE_URI: &str = "urn:UpcUaG";

/// Default network timeout for OPC UA operations in seconds.
///
/// This timeout applies to various OPC UA network operations including:
/// - TCP connection establishment
/// - Hello message exchange
/// - Request/response cycles
///
/// # Default Value
///
/// The default of 5 seconds provides a balance between responsiveness
/// and reliability for typical network conditions.
pub const OPCUA_DEFAULT_NETWORK_TIMEOUT: u32 = 5;

/// Default IP address for the OPC UA server to bind to.
///
/// When no specific IP address is configured, the server will bind to
/// localhost (127.0.0.1), making it accessible only from the local machine.
///
/// # Security Consideration
///
/// For production deployments, consider using "0.0.0.0" to bind to all
/// interfaces or a specific IP address for controlled access.
pub const OPCUA_DEFAULT_IP_ADDRESS: &str = "127.0.0.1";

/// Default port number for the OPC UA server.
///
/// Port 4840 is the standard registered port for OPC UA communication
/// as defined by the OPC Foundation. This ensures compatibility with
/// standard OPC UA clients and tools.
///
/// # Note
///
/// If port 4840 is already in use, the application should be configured
/// to use an alternative port through the configuration file.
pub const OPCUA_DEFAULT_PORT: u16 = 4840;

/// Default maximum number of concurrent OPC UA client sessions (Story 7-3, FR44).
///
/// When `[opcua].max_connections` is unset (or `None`), the gateway caps
/// concurrent sessions at this value. Single source of truth — `opc_ua.rs`
/// reads this constant via `Option::unwrap_or` and `AppConfig::validate`
/// references it implicitly through [`OPCUA_MAX_CONNECTIONS_HARD_CAP`] /
/// the `Some(0)` lower-bound check.
pub const OPCUA_DEFAULT_MAX_CONNECTIONS: usize = 10;

/// Hard upper bound for `[opcua].max_connections` (Story 7-3, FR44).
///
/// Values above this cap are rejected at startup by `AppConfig::validate`.
/// 4096 is a "you almost certainly want a deployment review before going
/// here" guard rather than a physical limit — see
/// `_bmad-output/implementation-artifacts/7-3-connection-limiting.md`
/// "Why 4096" for the back-of-envelope. Operators hitting it should file
/// the per-IP rate-limiting follow-up rather than raising the cap.
pub const OPCUA_MAX_CONNECTIONS_HARD_CAP: usize = 4096;

/// Period (seconds) between `info!(event="opcua_session_count")` gauge
/// emissions from `SessionMonitor` (Story 7-3, AC#3). 5s strikes a
/// balance between operator-facing utilisation visibility and log volume.
/// Tests in `tests/opc_ua_connection_limit.rs` sleep
/// `OPCUA_SESSION_GAUGE_INTERVAL_SECS + 1` seconds to guarantee at least
/// one tick — keep that relationship intact when tuning.
pub const OPCUA_SESSION_GAUGE_INTERVAL_SECS: u64 = 5;

/// Default maximum subscriptions per OPC UA session (Story 8-2, AC#1, FR21).
///
/// Mirrors `async-opcua-server-0.17.1::lib.rs:131`'s
/// `MAX_SUBSCRIPTIONS_PER_SESSION = 10`. When `[opcua].max_subscriptions_per_session`
/// is unset (or `None`), the gateway falls back to this value so the
/// configured behaviour matches the async-opcua library default. Override
/// via env var `OPCGW_OPCUA__MAX_SUBSCRIPTIONS_PER_SESSION`.
pub const OPCUA_DEFAULT_MAX_SUBSCRIPTIONS_PER_SESSION: usize = 10;

/// Hard upper bound for `[opcua].max_subscriptions_per_session` (Story 8-2, AC#1).
///
/// Values above this cap are rejected at startup by `AppConfig::validate`.
/// 1000 is a "deployment review needed" guard against subscription-flood
/// DoS — a single misbehaving SCADA client creating 1000+ subscriptions
/// would saturate the publish pipeline. Operators hitting it should
/// inventory their client topology before raising.
pub const OPCUA_MAX_SUBSCRIPTIONS_PER_SESSION_HARD_CAP: usize = 1000;

/// Default maximum monitored items per subscription (Story 8-2, AC#1, FR21).
///
/// Mirrors `async-opcua-server-0.17.1::lib.rs:64`'s
/// `DEFAULT_MAX_MONITORED_ITEMS_PER_SUB = 1000`. Note the library field
/// name is `max_monitored_items_per_sub` (not `_per_subscription`); the
/// gateway field, TOML key, and env var all use the library name. Override
/// via env var `OPCGW_OPCUA__MAX_MONITORED_ITEMS_PER_SUB`.
pub const OPCUA_DEFAULT_MAX_MONITORED_ITEMS_PER_SUB: usize = 1000;

/// Hard upper bound for `[opcua].max_monitored_items_per_sub` (Story 8-2, AC#1).
///
/// Values above this cap are rejected at startup by `AppConfig::validate`.
/// 100_000 is structurally absurd for the address spaces typical opcgw
/// deployments expose (10–1000 nodes total) — the cap exists to surface
/// configuration mistakes (typo, unit confusion) rather than restrict
/// legitimate sizing.
pub const OPCUA_MAX_MONITORED_ITEMS_PER_SUB_HARD_CAP: usize = 100_000;

/// Default maximum OPC UA message size in bytes (Story 8-2, AC#1, FR21).
///
/// Mirrors `async-opcua-types-0.17.1::lib.rs:43`'s `MAX_MESSAGE_SIZE =
/// 65535 * MAX_CHUNK_COUNT = 65535 * 5 = 327_675`. When
/// `[opcua].max_message_size` is unset (or `None`), the gateway falls
/// back to this value so behaviour matches the library default. Override
/// via env var `OPCGW_OPCUA__MAX_MESSAGE_SIZE`.
pub const OPCUA_DEFAULT_MAX_MESSAGE_SIZE: usize = 65_535 * 5;

/// Hard upper bound for `[opcua].max_message_size` in bytes (Story 8-2, AC#1).
///
/// Values above this cap are rejected at startup by `AppConfig::validate`.
/// The value is exactly `OPCUA_MAX_CHUNK_COUNT_HARD_CAP × 65535 =
/// 4096 × 65535 = 268_431_360 ≈ 255.996 MiB` so that the two hard caps
/// are mathematically coherent — any message at this ceiling can fit
/// inside the maximum allowable chunk count, avoiding a configuration
/// where both knobs are at their per-knob caps but the cross-knob
/// coherence check would still reject the combination. Protects against
/// memory-exhaustion DoS via forged "large array" reads — default opcgw
/// deployments expose scalar metrics and never approach this ceiling.
pub const OPCUA_MAX_MESSAGE_SIZE_HARD_CAP: usize = 4096 * 65_535;

/// Default maximum number of message chunks per OPC UA message (Story 8-2, AC#1, FR21).
///
/// Mirrors `async-opcua-types-0.17.1::lib.rs:48`'s `MAX_CHUNK_COUNT = 5`.
/// When `[opcua].max_chunk_count` is unset (or `None`), the gateway falls
/// back to this value so behaviour matches the library default. Override
/// via env var `OPCGW_OPCUA__MAX_CHUNK_COUNT`.
pub const OPCUA_DEFAULT_MAX_CHUNK_COUNT: usize = 5;

/// Hard upper bound for `[opcua].max_chunk_count` (Story 8-2, AC#1).
///
/// Values above this cap are rejected at startup by `AppConfig::validate`.
/// 4096 chunks × 65535 bytes/chunk = 268_431_360 bytes (≈ 256 MiB),
/// exactly equal to `OPCUA_MAX_MESSAGE_SIZE_HARD_CAP` so the two knobs
/// can both reach their per-knob maxima simultaneously without
/// tripping the cross-knob coherence check. Values above signal a
/// misconfiguration rather than a deliberate sizing.
pub const OPCUA_MAX_CHUNK_COUNT_HARD_CAP: usize = 4096;

/// FR22 floor for `[storage].retention_days` (Story 8-3, AC#3).
///
/// FR22 mandates a minimum of 7 days for historical metric data
/// retention. Lower values would defeat the historical-trend
/// use case (a SCADA operator analysing soil-moisture patterns
/// over the past week needs at least one week of data on hand).
/// Values below this floor are rejected at startup by `validate()`.
pub const STORAGE_RETENTION_DAYS_FLOOR: u32 = 7;

/// Hard upper bound for `[storage].retention_days` (Story 8-3, AC#3).
///
/// Values above this cap are rejected at startup by `validate()`.
/// At 10s polling × ~400 metric pairs × 365 days the metric_history
/// table approaches 1.3 billion rows which strains both pruning
/// performance and per-call HistoryRead query latency. Operators
/// that need longer retention must open a follow-up issue so the
/// tuning trade-offs (sharding, archival to cold storage, lower
/// polling rate) can be reviewed.
pub const STORAGE_RETENTION_DAYS_HARD_CAP: u32 = 365;

/// Default for `[opcua].max_history_data_results_per_node` (Story 8-3, AC#3).
///
/// Per-call cap on the number of `HistoryData` rows returned by a
/// single OPC UA `HistoryRead` request for one NodeId. 10000 rows
/// at 10s polling is ~28 hours of poll data — sufficient for typical
/// FUXA dashboard time-windows. SCADA clients that want longer
/// windows page via repeated calls (Story 8-3 does NOT implement
/// OPC UA `ByteString` continuation points; manual paging via
/// `last_returned_row.timestamp + 1µs` is the contract — see
/// `docs/security.md#historical-data-access`). Override via env
/// var `OPCGW_OPCUA__MAX_HISTORY_DATA_RESULTS_PER_NODE`.
pub const OPCUA_DEFAULT_MAX_HISTORY_DATA_RESULTS_PER_NODE: usize = 10_000;

/// Hard upper bound for `[opcua].max_history_data_results_per_node` (Story 8-3, AC#3).
///
/// Values above this cap are rejected at startup by `validate()`.
/// 1_000_000 rows is the per-call response-size DoS protection
/// ceiling; values above signal a misconfiguration. A per-call
/// response of 1M `HistoryData` rows would saturate the publish
/// pipeline and likely exceed `max_message_size` even at the cap.
pub const OPCUA_MAX_HISTORY_DATA_RESULTS_PER_NODE_HARD_CAP: usize = 1_000_000;

// =============================================================================
// Web UI Constants (Story 9-1)
// =============================================================================
//
// The four `WEB_DEFAULT_*` / `WEB_MIN_PORT` / `WEB_MAX_PORT` /
// `WEB_AUTH_REALM_MAX_LEN` constants are the single source of truth for the
// `[web]` configuration block. `src/config.rs` references them in
// `AppConfig::validate`; `src/web/mod.rs` reads them via `unwrap_or` when
// the operator leaves a knob unset; tests assert against the constants
// rather than duplicating the literals.

/// Default port for the embedded Axum web server (Story 9-1, AC#1).
///
/// 8080 is the conventional non-root HTTP listening port. The web server
/// is opt-in (default `WEB_DEFAULT_ENABLED = false`), so this port is
/// only bound when the operator explicitly enables `[web].enabled = true`.
/// Override via env var: `OPCGW_WEB__PORT`.
pub const WEB_DEFAULT_PORT: u16 = 8080;

/// Lower bound for `[web].port` (Story 9-1, AC#1).
///
/// Below 1024 lives the privileged-port range, which on Linux requires
/// `CAP_NET_BIND_SERVICE` or root. The gateway should not need elevated
/// privileges; values below this floor are rejected at startup by
/// `AppConfig::validate`.
pub const WEB_MIN_PORT: u16 = 1024;

/// Upper bound for `[web].port` (Story 9-1, AC#1).
///
/// 65535 is the largest representable TCP port. Values above are rejected
/// at startup. Note: `u16` already caps at 65535 — the constant exists so
/// the validator's error message is symmetric with [`WEB_MIN_PORT`] and
/// callers don't need to repeat the literal.
pub const WEB_MAX_PORT: u16 = 65_535;

/// Default bind address for the embedded Axum web server (Story 9-1, AC#1).
///
/// `"0.0.0.0"` listens on every interface — appropriate for the typical
/// LAN-internal deployment. Operators can restrict to `"127.0.0.1"` if a
/// reverse proxy on the same host fronts the gateway. Override via env
/// var: `OPCGW_WEB__BIND_ADDRESS`.
pub const WEB_DEFAULT_BIND_ADDRESS: &str = "0.0.0.0";

/// Default Basic-auth realm string sent in `WWW-Authenticate` headers
/// (Story 9-1, AC#1).
///
/// The realm is the human-readable label browsers display in their
/// credential prompt. `"opcgw"` is short and unambiguous; operators
/// running multiple gateways may override per-deployment to make the
/// prompt distinguishable. Override via env var:
/// `OPCGW_WEB__AUTH_REALM`.
pub const WEB_DEFAULT_AUTH_REALM: &str = "opcgw";

/// Maximum length (chars) of `[web].auth_realm` (Story 9-1, AC#1).
///
/// Strings longer than this are rejected at startup by
/// `AppConfig::validate`. 64 is comfortably above the conventional
/// "Application Realm" string lengths and below the point where the
/// `WWW-Authenticate` header line becomes operationally awkward.
pub const WEB_AUTH_REALM_MAX_LEN: usize = 64;

/// Default for `[web].enabled` (Story 9-1, AC#1).
///
/// **`false` by design** — existing operators upgrading from Phase A
/// must not get a surprise new listening port without opt-in. The
/// shipped `config/config.toml` ships the `[web]` block commented-out
/// with the opt-in step documented inline.
pub const WEB_DEFAULT_ENABLED: bool = false;

/// Maximum bytes per chunk in async-opcua 0.17.1 (Story 8-2 code review).
///
/// **Note on naming:** this constant is named "min" because it is the
/// smaller-than-or-equal bound used by the cross-knob coherence check —
/// the actual per-chunk byte limit in async-opcua is **at most** 65535
/// bytes per `async-opcua-types-0.17.1::lib.rs:43` (`MAX_MESSAGE_SIZE =
/// 65535 * MAX_CHUNK_COUNT`). This is **not** the OPC UA Part 6
/// TransportProfile minimum (which is 8192 bytes for
/// `ReceiveBufferSize` / `SendBufferSize` per Part 6 §6.7.2); the
/// gateway uses the library's per-chunk ceiling because that is the
/// effective constraint at runtime.
///
/// Used by the cross-knob coherence check in `AppConfig::validate`:
/// `max_message_size > max_chunk_count × OPCUA_MIN_CHUNK_SIZE_BYTES` is
/// geometrically impossible — any message larger than the chunk-derived
/// ceiling will fail at runtime with `BadResponseTooLarge`. Reject this
/// combination at startup instead of letting it surface as an opaque
/// per-message rejection in production.
pub const OPCUA_MIN_CHUNK_SIZE_BYTES: usize = 65_535;

// =============================================================================
// Configuration File Path Constants
// =============================================================================

/// Default directory path for configuration files.
///
/// All configuration files are expected to be located in this directory
/// relative to the application's working directory. This includes:
/// - Main application configuration (`config.toml`)
/// - Certificate and key files
/// - PKI directory structure
///
/// # Directory Structure
///
/// ```text
/// config/
/// ├── config.toml
/// ├── certs/
/// │   ├── server.crt
/// │   └── server.key
/// └── pki/
///     ├── trusted/
///     ├── rejected/
///     └── issued/
/// ```
pub const OPCGW_CONFIG_PATH: &str = "config";

/// Default configuration file name.
///
/// This is the primary configuration file that contains all application
/// settings including ChirpStack connection parameters, OPC UA server
/// configuration, and device definitions.
///
/// # File Format
///
/// The configuration file uses TOML format for human-readable configuration
/// with support for hierarchical settings and environment variable overrides.
pub const OPCGW_CONFIG_FILE: &str = "config.toml";

// =============================================================================
// ChirpStack Integration Constants
// =============================================================================

/// Display name for the ChirpStack server in OPC UA.
///
/// This name appears in the OPC UA address space as the folder or object
/// name containing ChirpStack server status and diagnostic information.
/// It provides a human-readable identifier for the ChirpStack integration.
pub const OPCGW_CP_NAME: &str = "Chirpstack";

/// OPC UA variable name for ChirpStack server availability status.
///
/// This variable exposes the current availability state of the ChirpStack
/// server as a boolean value in the OPC UA address space:
/// - `true`: ChirpStack server is reachable and responding
/// - `false`: ChirpStack server is unreachable or not responding
///
/// # Usage in OPC UA
///
/// Clients can subscribe to this variable to receive real-time notifications
/// about ChirpStack server connectivity changes.
pub const OPCGW_CP_AVAILABILITY_NAME: &str = "Chirpstack_available";

/// OPC UA variable name for ChirpStack server response time.
///
/// This variable exposes the current average response time for ChirpStack
/// API calls as a floating-point value in milliseconds. It provides
/// performance monitoring capabilities for the ChirpStack connection.
///
/// # Metric Interpretation
///
/// - Low values (< 100ms): Excellent performance
/// - Medium values (100-1000ms): Acceptable performance
/// - High values (> 1000ms): Performance issues, may indicate network problems
/// - Zero value: No recent measurements or server unavailable
pub const OPCGW_CP_RESPONSE_TIME_NAME: &str = "ResponseTime";

// =============================================================================
// Secret-Handling Constants (Story 7-1)
// =============================================================================

/// Prefix used in shipped configuration templates to mark fields whose value
/// must be supplied by the operator before first run (Story 7-1, AC#1/AC#2).
///
/// `AppConfig::validate` rejects `chirpstack.api_token` and `opcua.user_password`
/// when their value starts with this literal prefix, with an error message
/// pointing at the corresponding `OPCGW_*` env var. The prefix is checked
/// verbatim — future placeholders following the same convention generalise.
pub const PLACEHOLDER_PREFIX: &str = "REPLACE_ME_WITH_";

/// Replacement string emitted by the redacting `Debug` impls of
/// `ChirpstackPollerConfig` and `OpcUaConfig` (Story 7-1, AC#3).
///
/// 14 characters total: 3 asterisks + the 8-letter word `REDACTED` + 3
/// asterisks. Centralised here so tests can assert against the constant
/// rather than duplicating the literal.
pub const REDACTED_PLACEHOLDER: &str = "***REDACTED***";

// =============================================================================
// OPC UA Security Constants (Story 7-2)
// =============================================================================

/// Internal token id registered against every OPC UA endpoint and resolved by
/// `OpcgwAuthManager` to the configured `[opcua].user_name` /
/// `[opcua].user_password` (Story 7-2, AC#1).
///
/// Decoupled from the operator's actual username so a future multi-user
/// expansion can introduce additional ids (e.g. `"power-user"`,
/// `"readonly-user"`) without renaming this baseline. The token id is
/// internal to async-opcua's user-policy table — not surfaced to clients in
/// any user-facing string — so renaming is a non-breaking change.
pub const OPCUA_USER_TOKEN_ID: &str = "default-user";

/// Internal device identifier for ChirpStack server monitoring.
///
/// This identifier is used internally by the gateway to track and store
/// ChirpStack server status metrics. It follows the same pattern as
/// regular device IDs but is reserved for the ChirpStack server itself.
///
/// # Usage
///
/// This ID is used when:
/// - Storing ChirpStack server metrics in the internal storage
/// - Retrieving server status for OPC UA exposure
/// - Distinguishing server metrics from device metrics
pub const OPCGW_CP_ID: &str = "cp0";

// =============================================================================
// Performance Budgets (Story 6-3, AC#3)
// =============================================================================
//
// Each budget is the wall-clock latency above which a `debug!` operation log
// is upgraded to `warn!` with `exceeded_budget=true`. The numbers come from
// Epic 5's per-subsystem budgets — they are diagnostics, not hard limits, so
// crossing them is a "look at me" signal rather than an error. Centralised
// here so the thresholds match across `opc_ua.rs`, `chirpstack.rs`, and
// `storage/sqlite.rs` and any future tuning happens in one place.

/// OPC UA read budget per Epic 5 Story 5-1: a single read should complete in
/// well under 100 ms. Crossing this threshold from a hot path indicates
/// contention on the shared SQLite backend or an unexpectedly slow
/// staleness/health computation.
pub const OPC_UA_READ_BUDGET_MS: u64 = 100;

// ---------------------------------------------------------------------------
// Storage-latency budgets (GH-144)
//
// These two budgets only gate the WARN-vs-DEBUG decision on storage-query and
// batch-write timing logs — they change no functional behavior. They are
// runtime-tunable via environment variables (resolved once at startup, see
// `init_storage_budgets_from_env`) because the right ceiling depends on the
// backing disk: a local SSD finishes most queries in well under 1 ms, while a
// NAS / network-backed SQLite routinely runs ~100 ms+ per single-row write and
// up to ~2 s per end-of-cycle batch. The shipped defaults are sized for
// NAS-class storage so the WARNs stay meaningful out of the box; operators on
// fast local disks can tighten them to restore early regression detection.

/// Default storage-query budget in ms (used when `OPCGW_STORAGE_QUERY_BUDGET_MS`
/// is unset or invalid). Sized for NAS-class single-row writes.
pub const DEFAULT_STORAGE_QUERY_BUDGET_MS: u64 = 250;

/// Default batch-write budget in ms (used when `OPCGW_BATCH_WRITE_BUDGET_MS` is
/// unset or invalid). Sized for a ~100-metric end-of-cycle batch on NAS storage.
pub const DEFAULT_BATCH_WRITE_BUDGET_MS: u64 = 2000;

/// Env var overriding the storage-query budget (positive integer milliseconds).
pub const STORAGE_QUERY_BUDGET_ENV: &str = "OPCGW_STORAGE_QUERY_BUDGET_MS";

/// Env var overriding the batch-write budget (positive integer milliseconds).
pub const BATCH_WRITE_BUDGET_ENV: &str = "OPCGW_BATCH_WRITE_BUDGET_MS";

static STORAGE_QUERY_BUDGET_MS: AtomicU64 = AtomicU64::new(DEFAULT_STORAGE_QUERY_BUDGET_MS);
static BATCH_WRITE_BUDGET_MS: AtomicU64 = AtomicU64::new(DEFAULT_BATCH_WRITE_BUDGET_MS);

/// Current storage-query budget in ms (the threshold above which a
/// `storage_query` timing log is upgraded from `debug!` to
/// `warn!(exceeded_budget=true)`). Reads a process-global atomic — cheap enough
/// to call on every query; never re-reads the environment.
pub fn storage_query_budget_ms() -> u64 {
    STORAGE_QUERY_BUDGET_MS.load(Ordering::Relaxed)
}

/// Current batch-write budget in ms (the threshold above which a `batch_write`
/// timing log is emitted at `warn!` instead of `debug!`). Reads a
/// process-global atomic; never re-reads the environment.
pub fn batch_write_budget_ms() -> u64 {
    BATCH_WRITE_BUDGET_MS.load(Ordering::Relaxed)
}

/// Resolve both storage-latency budgets from the environment exactly once at
/// startup. Call this *after* the tracing subscriber is initialised so the
/// resolution logs are captured. Invalid or zero values fall back to the
/// default with a single `warn!`; a successfully applied override logs at
/// `info!` with `source="env"`, otherwise `source="default"`.
pub fn init_storage_budgets_from_env() {
    resolve_budget_env(
        STORAGE_QUERY_BUDGET_ENV,
        &STORAGE_QUERY_BUDGET_MS,
        DEFAULT_STORAGE_QUERY_BUDGET_MS,
    );
    resolve_budget_env(
        BATCH_WRITE_BUDGET_ENV,
        &BATCH_WRITE_BUDGET_MS,
        DEFAULT_BATCH_WRITE_BUDGET_MS,
    );
}

/// Parse a budget env var value: `Ok(ms)` for a positive integer, `Err(reason)`
/// for anything else (non-numeric, or zero — zero would warn on every query).
/// Pure function so it can be unit-tested without touching the global atomics.
fn parse_budget_value(raw: &str) -> Result<u64, &'static str> {
    match raw.trim().parse::<u64>() {
        Ok(0) => Err("must be greater than zero"),
        Ok(ms) => Ok(ms),
        Err(_) => Err("not a positive integer"),
    }
}

/// Apply one env var to its atomic, logging the resolved value and source.
fn resolve_budget_env(env_key: &str, slot: &AtomicU64, default_ms: u64) {
    match std::env::var(env_key) {
        Ok(raw) => match parse_budget_value(&raw) {
            Ok(ms) => {
                slot.store(ms, Ordering::Relaxed);
                tracing::info!(
                    operation = "storage_budget_init",
                    env_key = env_key,
                    budget_ms = ms,
                    source = "env",
                    "Storage-latency budget resolved from environment"
                );
            }
            Err(reason) => {
                tracing::warn!(
                    operation = "storage_budget_init",
                    env_key = env_key,
                    rejected_value = %raw,
                    reason = reason,
                    budget_ms = default_ms,
                    source = "default",
                    "Invalid storage-latency budget override; using default"
                );
            }
        },
        Err(_) => {
            tracing::info!(
                operation = "storage_budget_init",
                env_key = env_key,
                budget_ms = default_ms,
                source = "default",
                "Storage-latency budget using built-in default"
            );
        }
    }
}

/// Maximum number of daily-rolling log files retained on disk (CR #143).
///
/// opcgw writes a single application log (`opcgw.log.<date>`) that rotates
/// daily; the rolling appender prunes files beyond this many days so the log
/// directory is self-limiting instead of growing without bound (a 16-day,
/// 11 GB pile-up was observed in production before this cap). ~2 weeks of
/// history is enough to investigate any incident.
pub const LOG_MAX_RETAINED_FILES: usize = 14;

// =============================================================================
// Error Types
// =============================================================================

/// Comprehensive error type for the OPC UA ChirpStack Gateway.
///
/// This enum covers all major error categories that can occur in the gateway
/// application. Each variant includes a descriptive message that provides
/// context about the specific error condition.
///
/// # Error Categories
///
/// - **Configuration**: Issues with loading or parsing configuration files
/// - **ChirpStack**: Problems communicating with the ChirpStack server
/// - **OPC UA**: Errors in OPC UA server operations or client communication
/// - **Storage**: Issues with internal data storage and retrieval
///
/// # Usage
///
/// This error type implements the `Error` trait from `thiserror`, providing:
/// - Automatic `Display` implementation with descriptive messages
/// - Error source chaining capabilities
/// - Integration with Rust's standard error handling patterns
///
/// # Examples
///
/// ```rust,ignore
/// use crate::utils::OpcGwError;
///
/// fn load_config() -> Result<Config, OpcGwError> {
///     // Configuration loading logic
///     Err(OpcGwError::ConfigurationError(
///         "Failed to parse TOML file".to_string()
///     ))
/// }
/// ```
#[derive(Error, Debug)]
pub enum OpcGwError {
    /// Configuration-related errors.
    ///
    /// This variant is used for errors that occur during:
    /// - Configuration file loading and parsing
    /// - Environment variable processing
    /// - Configuration validation
    /// - Missing or invalid configuration parameters
    ///
    /// # Common Causes
    ///
    /// - Malformed TOML syntax
    /// - Missing required configuration fields
    /// - Invalid configuration values (e.g., malformed URLs)
    /// - File system permissions issues
    #[error("Configuration error: {0}")]
    Configuration(String),

    /// ChirpStack server communication errors.
    ///
    /// This variant covers errors related to ChirpStack server interaction:
    /// - Network connectivity issues
    /// - Authentication failures
    /// - API response parsing errors
    /// - ChirpStack server errors (HTTP 4xx/5xx responses)
    ///
    /// # Common Causes
    ///
    /// - ChirpStack server is down or unreachable
    /// - Invalid API token or expired credentials
    /// - Network connectivity issues
    /// - ChirpStack API changes or compatibility issues
    #[error("ChirpStack poller error: {0}")]
    ChirpStack(String),

    /// OPC UA server and client communication errors.
    ///
    /// This variant encompasses errors in the OPC UA layer:
    /// - Server startup and configuration issues
    /// - Client connection problems
    /// - Certificate and security errors
    /// - Address space management errors
    ///
    /// # Common Causes
    ///
    /// - Port binding failures (port already in use)
    /// - Certificate validation issues
    /// - Invalid OPC UA configuration
    /// - Client authentication failures
    /// - Address space construction errors
    #[error("OPC UA error: {0}")]
    OpcUa(String),

    /// Internal storage system errors.
    ///
    /// This variant is used for errors in the internal data storage layer:
    /// - Device lookup failures
    /// - Metric storage and retrieval issues
    /// - Data consistency problems
    /// - Storage system initialization errors
    ///
    /// # Common Causes
    ///
    /// - Attempting to access non-existent devices or metrics
    /// - Storage corruption or inconsistency
    /// - Memory allocation failures
    /// - Thread synchronization issues
    #[error("Storage error: {0}")]
    Storage(String),

    /// Database-specific errors (SQLite).
    ///
    /// This variant covers errors from the persistent database layer:
    /// - SQLite connection failures
    /// - Schema migration errors
    /// - Query execution failures
    /// - Database corruption
    #[error("Database error: {0}")]
    Database(String),

    /// Command parameter validation errors.
    ///
    /// This variant is used for errors during command parameter validation:
    /// - Parameter type mismatches
    /// - Out-of-range values
    /// - Missing required parameters
    /// - Invalid enum values
    /// - Missing device schema
    #[error("Command validation error for device '{device_id}', command '{command_name}': {reason}")]
    CommandValidation {
        device_id: String,
        command_name: String,
        reason: String,
    },

    /// Embedded Axum web server runtime errors (Story 9-1).
    ///
    /// Used for runtime failures in the web server: bind failure, listener
    /// I/O errors, request-handling panics propagated as errors. Startup
    /// configuration mistakes (port out of range, unparseable bind address,
    /// invalid auth realm) flow through [`OpcGwError::Configuration`]
    /// instead — those are caught by `AppConfig::validate`.
    #[error("Web server error: {0}")]
    Web(String),
}

// =============================================================================
// Utility Functions
// =============================================================================

/// Prints the type name of a referenced value to standard output.
///
/// This utility function is primarily used for debugging and development
/// purposes. It uses Rust's reflection capabilities to determine and display
/// the concrete type of a value at runtime.
///
/// # Arguments
///
/// * `_` - A reference to any type implementing any traits. The parameter
///   is unnamed since only the type information is used, not the value itself.
///
/// # Type Parameter
///
/// * `T` - The type of the referenced value. This is automatically inferred
///   by the Rust compiler based on the argument.
///
/// # Output
///
/// The function prints the fully qualified type name to stdout. For primitive
/// types, this will be simple names like "i32" or "f64". For complex types,
/// it will include the full module path.
///
/// # Examples
///
/// ```rust,ignore
/// use crate::utils::print_type_of;
///
/// let integer = 42;
/// let float = 3.14;
/// let text = "hello";
/// let vector = vec![1, 2, 3];
///
/// print_type_of(&integer);  // Prints: i32
/// print_type_of(&float);    // Prints: f64
/// print_type_of(&text);     // Prints: &str
/// print_type_of(&vector);   // Prints: alloc::vec::Vec<i32>
/// ```
///
/// # Usage Scenarios
///
/// This function is particularly useful when:
/// - Debugging generic code where types may not be obvious
/// - Verifying type inference in complex expressions
/// - Learning about Rust's type system and type naming conventions
/// - Troubleshooting compilation issues related to type mismatches
///
/// # Performance
///
/// The function has minimal runtime overhead as it only accesses type
/// metadata that is available at compile time. The actual printing is
/// the only runtime operation.
///
/// # Development vs Production
///
/// This function is primarily intended for development and debugging.
/// Consider removing calls to this function in production code or
/// guarding them with debug assertions:
///
/// ```rust,ignore
/// #[cfg(debug_assertions)]
/// print_type_of(&my_variable);
/// ```
pub fn print_type_of<T>(_: &T) {
    println!("{}", std::any::type_name::<T>())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GH-144: a positive integer is accepted as the budget value.
    #[test]
    fn parse_budget_value_accepts_positive_integer() {
        assert_eq!(parse_budget_value("50"), Ok(50));
        assert_eq!(parse_budget_value("2000"), Ok(2000));
        // surrounding whitespace is tolerated
        assert_eq!(parse_budget_value("  120 "), Ok(120));
    }

    /// GH-144: non-numeric input is rejected so the caller falls back to default.
    #[test]
    fn parse_budget_value_rejects_non_numeric() {
        assert!(parse_budget_value("abc").is_err());
        assert!(parse_budget_value("12ms").is_err());
        assert!(parse_budget_value("").is_err());
        assert!(parse_budget_value("-5").is_err());
    }

    /// GH-144: zero is rejected — a 0 ms budget would warn on every query.
    #[test]
    fn parse_budget_value_rejects_zero() {
        assert!(parse_budget_value("0").is_err());
    }

    /// GH-144: the shipped defaults are the NAS-realistic values and the
    /// accessors return them when no override has been applied. No test calls
    /// `init_storage_budgets_from_env`, so the process-global atomics remain at
    /// their compile-time defaults throughout the test binary — hence the exact
    /// equality is safe here.
    #[test]
    fn budget_defaults_are_nas_realistic() {
        assert_eq!(DEFAULT_STORAGE_QUERY_BUDGET_MS, 250);
        assert_eq!(DEFAULT_BATCH_WRITE_BUDGET_MS, 2000);
        assert_eq!(storage_query_budget_ms(), DEFAULT_STORAGE_QUERY_BUDGET_MS);
        assert_eq!(batch_write_budget_ms(), DEFAULT_BATCH_WRITE_BUDGET_MS);
    }

    /// GH-144: a valid env override is parsed and applied to its slot.
    /// Uses a private local atomic + a unique env key so it exercises the real
    /// resolution path WITHOUT touching the process-global budget atomics (which
    /// would risk cross-test bleed). Edition 2021 `set_var`/`remove_var` are safe.
    #[test]
    fn resolve_budget_env_applies_valid_override() {
        let key = "OPCGW_TEST_BUDGET_VALID";
        std::env::set_var(key, "77");
        let slot = AtomicU64::new(DEFAULT_STORAGE_QUERY_BUDGET_MS);
        resolve_budget_env(key, &slot, DEFAULT_STORAGE_QUERY_BUDGET_MS);
        std::env::remove_var(key);
        assert_eq!(slot.load(Ordering::Relaxed), 77);
    }

    /// GH-144: a non-numeric override leaves the slot at the default.
    #[test]
    fn resolve_budget_env_falls_back_on_invalid() {
        let key = "OPCGW_TEST_BUDGET_INVALID";
        std::env::set_var(key, "abc");
        let slot = AtomicU64::new(DEFAULT_BATCH_WRITE_BUDGET_MS);
        resolve_budget_env(key, &slot, DEFAULT_BATCH_WRITE_BUDGET_MS);
        std::env::remove_var(key);
        assert_eq!(slot.load(Ordering::Relaxed), DEFAULT_BATCH_WRITE_BUDGET_MS);
    }

    /// GH-144: a zero override leaves the slot at the default (0 would warn on
    /// every query).
    #[test]
    fn resolve_budget_env_falls_back_on_zero() {
        let key = "OPCGW_TEST_BUDGET_ZERO";
        std::env::set_var(key, "0");
        let slot = AtomicU64::new(DEFAULT_STORAGE_QUERY_BUDGET_MS);
        resolve_budget_env(key, &slot, DEFAULT_STORAGE_QUERY_BUDGET_MS);
        std::env::remove_var(key);
        assert_eq!(slot.load(Ordering::Relaxed), DEFAULT_STORAGE_QUERY_BUDGET_MS);
    }

    /// GH-144: when the env var is unset, the slot keeps its default.
    #[test]
    fn resolve_budget_env_uses_default_when_unset() {
        let key = "OPCGW_TEST_BUDGET_UNSET";
        std::env::remove_var(key);
        let slot = AtomicU64::new(DEFAULT_STORAGE_QUERY_BUDGET_MS);
        resolve_budget_env(key, &slot, DEFAULT_STORAGE_QUERY_BUDGET_MS);
        assert_eq!(slot.load(Ordering::Relaxed), DEFAULT_STORAGE_QUERY_BUDGET_MS);
    }
}

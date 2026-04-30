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

/// SQLite query budget. Most reads/writes against the WAL-mode backend
/// finish in under 1 ms; 10 ms is a generous ceiling that surfaces lock
/// contention, large result sets, or an under-tuned `busy_handler` without
/// firing on every cycle.
pub const STORAGE_QUERY_BUDGET_MS: u64 = 10;

/// Batch-write budget: an end-of-cycle batch covering ~100 metrics should
/// complete in well under 500 ms even on slower disks. Above this we want a
/// `warn!` so it shows up in production logs without `OPCGW_LOG_LEVEL=debug`.
pub const BATCH_WRITE_BUDGET_MS: u64 = 500;

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
/// ```rust,no_run
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
/// ```rust
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
/// ```rust
/// #[cfg(debug_assertions)]
/// print_type_of(&my_variable);
/// ```
pub fn print_type_of<T>(_: &T) {
    println!("{}", std::any::type_name::<T>())
}

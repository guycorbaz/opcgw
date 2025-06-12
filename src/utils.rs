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
    ConfigurationError(String),

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
    ChirpStackError(String),

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
    OpcUaError(String),

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
    StorageError(String),
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

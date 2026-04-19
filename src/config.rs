// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Configuration Management Module
//!
//! This module provides comprehensive configuration file management for the OPC UA ChirpStack Gateway.
//! It supports loading configuration from TOML files and environment variables, with structured
//! organization for different service components.
//!
//! # Configuration Sources
//!
//! The configuration is loaded from:
//! - TOML configuration file (default: `config/config.toml`)
//! - Environment variables with `OPCGW_` prefix
//! - Default values for optional parameters
//!
//! # Usage
//!
//! ```rust,no_run
//! use crate::config::AppConfig;
//!
//! let config = AppConfig::new()?;
//! println!("ChirpStack server: {}", config.chirpstack.server_address);
//! ```
//!

#[allow(unused)]
use crate::utils::{OpcGwError, OPCGW_CONFIG_PATH};
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use tracing::{debug, trace};
use serde::Deserialize;

/// Global application configuration parameters.
///
/// Contains application-wide settings that affect the overall behavior
/// of the gateway service. These settings may be expanded in future versions.
#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
pub struct Global {
    /// Enable detailed debug logging throughout the application.
    ///
    /// When set to `true`, enables verbose logging for troubleshooting.
    /// Currently not actively used but reserved for future implementation.
    pub debug: bool,
}

/// ChirpStack connection and polling configuration.
///
/// Contains all parameters required to establish connection with the ChirpStack
/// LoRaWAN Network Server and configure the polling behavior for device metrics.
#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
pub struct ChirpstackPollerConfig {
    /// ChirpStack server address including protocol and port.
    ///
    /// Format: `http://hostname:port` or `https://hostname:port`
    /// Example: `"http://localhost:8080"` or `"https://chirpstack.example.com:8080"`
    pub server_address: String,

    /// API token for authentication with ChirpStack server.
    ///
    /// This token must have sufficient permissions to:
    /// - List applications and devices
    /// - Retrieve device metrics
    /// - Access the configured tenant
    pub api_token: String,

    /// The tenant ID for multi-tenant ChirpStack deployments.
    ///
    /// Specifies which tenant's data to access. For single-tenant
    /// deployments, this is typically the default tenant ID.
    pub tenant_id: String,

    /// Device polling frequency in seconds.
    ///
    /// Determines how often the gateway polls ChirpStack for updated
    /// device metrics. Lower values provide more frequent updates but
    /// increase server load.
    pub polling_frequency: u64,

    /// Maximum number of connection retry attempts.
    ///
    /// When the ChirpStack server is unavailable, the gateway will
    /// retry connection up to this many times before giving up.
    pub retry: u32,

    /// Delay between retry attempts in seconds.
    ///
    /// Time to wait between consecutive connection retry attempts
    /// when the ChirpStack server is unavailable.
    pub delay: u64,
}

/// OPC UA server configuration parameters.
///
/// Contains all settings required to configure and run the OPC UA server
/// that exposes ChirpStack device data to OPC UA clients. This includes
/// security settings, network configuration, and certificate management.
#[derive(Debug, Deserialize, Clone)]
pub struct OpcUaConfig {
    /// Human-readable name for the OPC UA application.
    ///
    /// This name appears in OPC UA client discovery and connection dialogs.
    /// Example: `"ChirpStack Gateway"`
    pub application_name: String,

    /// Unique URI identifying this OPC UA application.
    ///
    /// Must be a valid URI that uniquely identifies this application instance.
    /// Example: `"urn:ChirpStackGateway:Server"`
    pub application_uri: String,

    /// URI identifying the product or software vendor.
    ///
    /// Used for OPC UA application identification and discovery.
    /// Example: `"urn:ChirpStackGateway:Product"`
    pub product_uri: String,

    /// Enable or disable OPC UA server diagnostics.
    ///
    /// When enabled, the server exposes diagnostic information such as
    /// connection counts, data change notifications, and server statistics.
    pub diagnostics_enabled: bool,

    /// TCP hello timeout in milliseconds.
    ///
    /// Maximum time to wait for initial TCP connection handshake.
    /// `None` uses the OPC UA library default.
    pub hello_timeout: Option<u32>,

    /// IP address for the OPC UA server to bind to.
    ///
    /// Specifies which network interface to listen on. Use:
    /// - `"0.0.0.0"` to listen on all interfaces
    /// - `"127.0.0.1"` for localhost only
    /// - Specific IP for single interface
    /// - `None` uses the library default.
    pub host_ip_address: Option<String>,

    /// Port number for the OPC UA server.
    ///
    /// Standard OPC UA port is 4840, but any available port can be used.
    /// `None` uses the library default port.
    pub host_port: Option<u16>,

    /// Automatically create sample certificate and private key.
    ///
    /// When `true`, generates a self-signed certificate for testing.
    /// For production, set to `false` and provide proper certificates.
    pub create_sample_keypair: bool,

    /// File system path to the server certificate.
    ///
    /// Path to the X.509 certificate file in PEM or DER format.
    /// Example: `"/etc/opcgw/certs/server.crt"`
    pub certificate_path: String,

    /// File system path to the server private key.
    ///
    /// Path to the private key file corresponding to the certificate.
    /// Example: `"/etc/opcgw/certs/server.key"`
    pub private_key_path: String,

    /// Automatically trust client certificates.
    ///
    /// When `true`, accepts any client certificate without validation.
    /// For production, set to `false` and properly manage client certificates.
    pub trust_client_cert: bool,

    /// Enable certificate time validity checking.
    ///
    /// When `true`, rejects expired or not-yet-valid certificates.
    /// Should typically be `true` for production deployments.
    pub check_cert_time: bool,

    /// Directory path for PKI certificate storage.
    ///
    /// Directory containing trusted, rejected, and issued certificates.
    /// Example: `"/etc/opcgw/pki"`
    pub pki_dir: String,

    /// Username for OPC UA server authentication.
    ///
    /// Used when the server requires username/password authentication.
    /// Can be empty if anonymous access is allowed.
    pub user_name: String,

    /// Password for OPC UA server authentication.
    ///
    /// Corresponding password for the username. Should be stored securely
    /// and can be overridden via environment variables.
    pub user_password: String,
}

/// ChirpStack application configuration.
///
/// Defines a ChirpStack application and its associated devices that should
/// be monitored by the gateway. Each application corresponds to a logical
/// grouping of LoRaWAN devices in ChirpStack.
#[derive(Debug, Deserialize, Clone)]
pub struct ChirpStackApplications {
    /// Human-readable name of the ChirpStack application.
    ///
    /// This is the display name used in the ChirpStack web interface.
    /// Example: `"Building Sensors"`
    pub application_name: String,

    /// Unique identifier of the ChirpStack application.
    ///
    /// This is the UUID or ID assigned by ChirpStack to identify the application.
    /// Example: `"550e8400-e29b-41d4-a716-446655440000"`
    pub application_id: String,

    /// List of devices within this application to monitor.
    ///
    /// Contains configuration for each device including which metrics
    /// to collect and how to expose them via OPC UA.
    #[serde(rename = "device")]
    pub device_list: Vec<ChirpstackDevice>,
}

/// Configuration for a specific ChirpStack device.
///
/// Defines a LoRaWAN device and specifies which metrics should be collected
/// from ChirpStack and how they should be presented in the OPC UA server.
#[derive(Debug, Deserialize, Clone)]
pub struct ChirpstackDevice {
    /// Unique device identifier in ChirpStack.
    ///
    /// This is typically the DevEUI (Device Extended Unique Identifier)
    /// or the device ID assigned by ChirpStack.
    /// Example: `"0018b20000000001"`
    pub device_id: String,

    /// Display name for the device in OPC UA.
    ///
    /// This name will appear in the OPC UA address space and should be
    /// descriptive and unique within the application.
    /// Example: `"Temperature Sensor 01"`
    pub device_name: String,

    /// List of metrics to collect from this device.
    ///
    /// Specifies which ChirpStack metrics to monitor and how to
    /// expose them in the OPC UA server.
    #[serde(rename = "read_metric")]
    pub read_metric_list: Vec<ReadMetric>,
    /// List of commands that can be send to this device.
    #[serde(rename = "command")]
    pub device_command_list: Option<Vec<DeviceCommandCfg>>,
}

/// Data types supported for OPC UA metric values.
///
/// Defines the possible data types that can be used when exposing
/// ChirpStack metrics through the OPC UA interface. The type determines
/// how the raw metric data is converted and presented.
#[derive(Debug, Deserialize, Clone, PartialEq)]
pub enum OpcMetricTypeConfig {
    /// Boolean value (true/false).
    ///
    /// Typically used for status indicators, alarms, or binary sensors.
    /// ChirpStack values of 0.0 map to `false`, 1.0 maps to `true`.
    Bool,

    /// Signed 64-bit integer value.
    ///
    /// Used for counters, discrete measurements, or enumerated values.
    /// ChirpStack float values are truncated to integer.
    Int,

    /// Double-precision floating-point value.
    ///
    /// Used for analog measurements like temperature, humidity, pressure.
    /// Preserves the full precision of ChirpStack metric values.
    Float,

    /// String value.
    ///
    /// Used for textual data, device status messages, or formatted values.
    /// Currently not implemented in the conversion logic.
    String,
}

// Structure that holds the data of the device read metrics we would like to monitor
///
/// This structure defines a mapping between ChirpStack device metrics and their
/// corresponding OPC UA representation. It allows the gateway to expose LoRaWAN
/// device data through the OPC UA protocol with proper type conversion and metadata.
#[derive(Debug, Deserialize, Clone)]
pub struct ReadMetric {
    /// The display name that will appear as a node identifier in the OPC UA address space
    /// This is the human-readable name that OPC UA clients will see when browsing metrics
    pub metric_name: String,

    /// The original metric name as defined in the ChirpStack network server
    /// This corresponds to the exact field name in ChirpStack's device data payload
    /// and is used for mapping incoming telemetry data to the correct metric
    pub chirpstack_metric_name: String,

    /// The data type configuration for this metric in the OPC UA context
    /// Defines how the raw ChirpStack data should be interpreted and presented
    /// to OPC UA clients (e.g., as integer, float, boolean, string)
    pub metric_type: OpcMetricTypeConfig,

    /// Optional unit of measurement for the metric (e.g., "°C", "V", "A", "%")
    /// When specified, this unit information can be exposed to OPC UA clients
    /// to provide context about the metric's physical meaning and scale
    #[allow(dead_code)]
    pub metric_unit: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct DeviceCommandCfg {
    /// Unique command identifier
    pub command_id: i32,
    /// Command name
    pub command_name: String,
    /// If the device has to send a confirmation after received the command
    pub command_confirmed: bool,
    /// The port of the chirpstack device
    pub command_port: i32,
}

/// SQLite storage configuration parameters.
///
/// Contains settings for data persistence via SQLite database.
#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
pub struct StorageConfig {
    /// Path to the SQLite database file.
    ///
    /// Example: `"data/opcgw.db"`
    /// Default: `"data/opcgw.db"`
    #[serde(default = "default_database_path")]
    pub database_path: String,

    /// Historical data retention period in days.
    ///
    /// Older data is automatically pruned. Default: 7 days
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,

    /// Pruning task interval in minutes.
    ///
    /// How often to check and remove expired data. Default: 60 minutes
    #[serde(default = "default_prune_interval")]
    pub prune_interval_minutes: u32,
}

/// Default database path
fn default_database_path() -> String {
    "data/opcgw.db".to_string()
}

/// Default retention days
fn default_retention_days() -> u32 {
    7
}

/// Default prune interval
fn default_prune_interval() -> u32 {
    60
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            database_path: default_database_path(),
            retention_days: default_retention_days(),
            prune_interval_minutes: default_prune_interval(),
        }
    }
}

/// Main application configuration structure.
///
/// Contains all configuration sections required to run the OPC UA ChirpStack Gateway.
/// This structure is loaded from TOML configuration files and environment variables
/// using the Figment library.
#[allow(dead_code)]
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    /// Global application settings.
    pub global: Global,

    /// ChirpStack connection and polling configuration.
    pub chirpstack: ChirpstackPollerConfig,

    /// OPC UA server configuration.
    pub opcua: OpcUaConfig,

    /// Storage and persistence configuration.
    #[serde(default)]
    pub storage: StorageConfig,

    /// List of ChirpStack applications and devices to monitor.
    #[serde(rename = "application")]
    pub application_list: Vec<ChirpStackApplications>,
}

impl AppConfig {
    /// Creates a new `AppConfig` instance by loading configuration from files and environment.
    ///
    /// This function performs hierarchical configuration loading:
    /// 1. Loads base configuration from TOML file
    /// 2. Overlays environment variables with `OPCGW_` prefix
    /// 3. Validates and parses the complete configuration
    ///
    /// # Configuration File Location
    ///
    /// The configuration file path is determined by:
    /// - `CONFIG_PATH` environment variable if set
    /// - Default: `${OPCGW_CONFIG_PATH}/config.toml`
    ///
    /// # Environment Variables
    ///
    /// Configuration values can be overridden using environment variables with
    /// the `OPCGW_` prefix. Nested values use double underscores (`__`).
    ///
    /// Examples:
    /// - `OPCGW_CHIRPSTACK__SERVER_ADDRESS=https://chirpstack.example.com:8080`
    /// - `OPCGW_OPCUA__HOST_PORT=4841`
    ///
    /// # Returns
    ///
    /// * `Ok(AppConfig)` - Successfully loaded and parsed configuration
    /// * `Err(OpcGwError)` - Configuration loading or parsing failed
    ///
    /// # Errors
    ///
    /// Returns `OpcGwError::ConfigurationError` if:
    /// - Configuration file cannot be read
    /// - TOML parsing fails
    /// - Required configuration fields are missing
    /// - Environment variable parsing fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use crate::config::AppConfig;
    ///
    /// let config = AppConfig::new()?;
    /// println!("ChirpStack server: {}", config.chirpstack.server_address);
    /// println!("OPC UA port: {:?}", config.opcua.host_port);
    /// ```
    pub fn new() -> Result<Self, OpcGwError> {
        debug!("Creating new AppConfig instance");

        // Determine configuration file path
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| format!("{}/config.toml", OPCGW_CONFIG_PATH));

        trace!(config_path = %config_path, "Loading configuration");

        // Load and merge configuration from multiple sources
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .merge(Env::prefixed("OPCGW_").global())
            .extract()
            .map_err(|e| {
                // Provide more context about what failed
                let error_msg = format!("Configuration deserialization failed: {}", e);
                OpcGwError::Configuration(error_msg)
            })?;

        // Validate configuration business rules
        config.validate()?;

        //debug!("Configuration is {:?} ", config);
        Ok(config)
    }

    /// Validates the configuration for correctness and consistency.
    ///
    /// Performs comprehensive validation of all configuration sections including:
    /// - ChirpStack connection parameters
    /// - OPC UA server settings
    /// - Application and device definitions
    /// - Metric configurations
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Configuration is valid
    /// * `Err(OpcGwError::Configuration)` - Validation failed with collected error messages
    ///
    /// # Errors
    ///
    /// Returns validation errors for:
    /// - Missing or empty required fields
    /// - Invalid value ranges
    /// - Duplicate device IDs
    /// - Empty application or device lists
    /// - Invalid URLs or formats
    pub fn validate(&self) -> Result<(), OpcGwError> {
        let mut errors = Vec::new();

        // Validate ChirpstackPollerConfig
        if self.chirpstack.server_address.is_empty() {
            errors.push("chirpstack.server_address: must not be empty".to_string());
        } else if !self.chirpstack.server_address.starts_with("http://")
            && !self.chirpstack.server_address.starts_with("https://")
        {
            errors.push(
                "chirpstack.server_address: must start with 'http://' or 'https://'".to_string(),
            );
        }

        if self.chirpstack.api_token.is_empty() {
            errors.push("chirpstack.api_token: must not be empty".to_string());
        }

        if self.chirpstack.tenant_id.is_empty() {
            errors.push("chirpstack.tenant_id: must not be empty".to_string());
        }

        if self.chirpstack.polling_frequency == 0 {
            errors.push("chirpstack.polling_frequency: must be greater than 0".to_string());
        }

        if self.chirpstack.retry == 0 {
            errors.push("chirpstack.retry: must be greater than 0".to_string());
        }

        if self.chirpstack.delay == 0 {
            errors.push("chirpstack.delay: must be greater than 0".to_string());
        }

        // Validate OpcUaConfig
        if self.opcua.application_name.is_empty() {
            errors.push("opcua.application_name: must not be empty".to_string());
        }

        if self.opcua.application_uri.is_empty() {
            errors.push("opcua.application_uri: must not be empty".to_string());
        }

        if self.opcua.host_port == Some(0) {
            errors.push("opcua.host_port: must not be 0 if specified".to_string());
        }

        if self.opcua.user_name.is_empty() {
            errors.push("opcua.user_name: must not be empty".to_string());
        }

        if self.opcua.user_password.is_empty() {
            errors.push("opcua.user_password: must not be empty".to_string());
        }

        // Validate application_list
        if self.application_list.is_empty() {
            errors.push("application_list: at least one application must be configured".to_string());
        } else {
            let mut seen_device_ids = std::collections::HashSet::new();

            for (app_idx, app) in self.application_list.iter().enumerate() {
                let app_context = format!("application[{}]", app_idx);

                if app.application_name.is_empty() {
                    errors.push(format!("{}.application_name: must not be empty", app_context));
                }

                if app.application_id.is_empty() {
                    errors.push(format!("{}.application_id: must not be empty", app_context));
                }

                if app.device_list.is_empty() {
                    errors.push(format!(
                        "{}.device_list: at least one device must be configured",
                        app_context
                    ));
                } else {
                    for (dev_idx, device) in app.device_list.iter().enumerate() {
                        let dev_context = format!("{}.device[{}]", app_context, dev_idx);

                        if device.device_id.is_empty() {
                            errors.push(format!("{}.device_id: must not be empty", dev_context));
                        } else {
                            // Check for duplicate device IDs
                            if seen_device_ids.contains(&device.device_id) {
                                errors.push(format!(
                                    "{}.device_id: '{}' is duplicated across applications",
                                    dev_context, device.device_id
                                ));
                            } else {
                                seen_device_ids.insert(device.device_id.clone());
                            }
                        }

                        if device.device_name.is_empty() {
                            errors.push(format!("{}.device_name: must not be empty", dev_context));
                        }

                        if device.read_metric_list.is_empty() {
                            errors.push(format!(
                                "{}.read_metric_list: at least one metric must be configured",
                                dev_context
                            ));
                        }
                    }
                }
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(OpcGwError::Configuration(format!(
                "Configuration validation failed:\n  - {}",
                errors.join("\n  - ")
            )))
        }
    }

    /// Retrieves the application name for a given application ID.
    ///
    /// Searches through the configured applications to find the one with the
    /// matching ID and returns its display name.
    ///
    /// # Arguments
    ///
    /// * `application_id` - The unique identifier of the ChirpStack application
    ///
    /// # Returns
    ///
    /// * `Some(String)` - The application name if found
    /// * `None` - If no application with the given ID exists
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let app_name = config.get_application_name(&"app-123".to_string());
    /// match app_name {
    ///     Some(name) => println!("Application name: {}", name),
    ///     None => println!("Application not found"),
    /// }
    /// ```
    #[allow(dead_code)]
    pub fn get_application_name(&self, application_id: &String) -> Option<String> {
        for app in self.application_list.iter() {
            if app.application_id == *application_id {
                return Some(app.application_name.clone());
            }
        }
        None
    }

    /// Retrieves the application ID for a given application name.
    ///
    /// Searches through the configured applications to find the one with the
    /// matching name and returns its unique identifier.
    ///
    /// # Arguments
    ///
    /// * `application_name` - The display name of the ChirpStack application
    ///
    /// # Returns
    ///
    /// * `Some(String)` - The application ID if found
    /// * `None` - If no application with the given name exists
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let app_id = config.get_application_id(&"Building Sensors".to_string());
    /// match app_id {
    ///     Some(id) => println!("Application ID: {}", id),
    ///     None => println!("Application not found"),
    /// }
    /// ```
    #[allow(dead_code)]
    pub fn get_application_id(&self, application_name: &String) -> Option<String> {
        for app in self.application_list.iter() {
            if app.application_name == *application_name {
                return Some(app.application_id.clone());
            }
        }
        None
    }

    /// Retrieves the device name for a given device ID.
    ///
    /// Searches through all configured applications and their devices to find
    /// the device with the matching ID and returns its display name.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the ChirpStack device
    ///
    /// # Returns
    ///
    /// * `Some(String)` - The device display name if found
    /// * `None` - If no device with the given ID exists
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let device_name = config.get_device_name(&"0018b20000000001".to_string());
    /// match device_name {
    ///     Some(name) => println!("Device name: {}", name),
    ///     None => println!("Device not found"),
    /// }
    /// ```
    pub fn get_device_name(&self, device_id: &String) -> Option<String> {
        debug!(device_id = %device_id, "Looking up device name");

        // Search through all applications and devices
        for app in self.application_list.iter() {
            for device in app.device_list.iter() {
                if device.device_id == *device_id {
                    return Some(device.device_name.clone());
                }
            }
        }
        None
    }

    /// Retrieves the device ID for a given device name within a specific application.
    ///
    /// Searches for a device with the specified name within the given application.
    /// If multiple devices have the same name, returns the first match found.
    ///
    /// # Arguments
    ///
    /// * `device_name` - The display name of the device
    /// * `application_id` - The unique identifier of the ChirpStack application
    ///
    /// # Returns
    ///
    /// * `Some(String)` - The device ID if found
    /// * `None` - If no matching device exists in the specified application
    ///
    /// # Note
    ///
    /// This function does not check for duplicate device names within an application.
    /// If duplicates exist, the first match is returned.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let device_id = config.get_device_id(
    ///     &"Temperature Sensor 01".to_string(),
    ///     &"app-123".to_string()
    /// );
    /// match device_id {
    ///     Some(id) => println!("Device ID: {}", id),
    ///     None => println!("Device not found in application"),
    /// }
    /// ```
    #[allow(dead_code)]
    pub fn get_device_id(&self, device_name: &String, application_id: &String) -> Option<String> {
        // Search for the specified application
        for app in self.application_list.iter() {
            if app.application_id == *application_id {
                // Search for device within the application
                for device in app.device_list.iter() {
                    if device.device_name == *device_name {
                        return Some(device.device_id.clone());
                    }
                }
            }
        }
        None
    }

    /// Retrieves the list of metrics configured for a specific device.
    ///
    /// Searches through all applications to find the device with the specified ID
    /// and returns a clone of its metric configuration list.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the ChirpStack device
    ///
    /// # Returns
    ///
    /// * `Some(Vec<Metric>)` - The list of configured metrics if the device is found
    /// * `None` - If no device with the given ID exists
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let metrics = config.get_metric_list(&"0018b20000000001".to_string());
    /// match metrics {
    ///     Some(metric_list) => {
    ///         println!("Device has {} metrics configured", metric_list.len());
    ///         for metric in metric_list {
    ///             println!("Metric: {}", metric.metric_name);
    ///         }
    ///     },
    ///     None => println!("Device not found or has no metrics"),
    /// }
    /// ```
    pub fn get_metric_list(&self, device_id: &String) -> Option<Vec<ReadMetric>> {
        debug!(device_id = %device_id, "Retrieving metric list");

        // Search through all applications and devices
        for app in self.application_list.iter() {
            for device in app.device_list.iter() {
                if device.device_id == *device_id {
                    return Some(device.read_metric_list.clone());
                }
            }
        }
        None
    }

    /// Retrieves the OPC UA metric type for a ChirpStack metric name and device.
    ///
    /// Looks up the configured metric type that should be used when exposing
    /// a specific ChirpStack metric through the OPC UA interface. The type
    /// determines how the raw metric data is converted and presented.
    ///
    /// # Arguments
    ///
    /// * `chirpstack_metric_name` - The metric name as defined in ChirpStack
    /// * `device_id` - The unique identifier of the ChirpStack device
    ///
    /// # Returns
    ///
    /// * `Some(OpcMetricTypeConfig)` - The configured metric type if found
    /// * `None` - If the device or metric is not found in the configuration
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let metric_type = config.get_metric_type(
    ///     &"temperature".to_string(),
    ///     &"0018b20000000001".to_string()
    /// );
    /// match metric_type {
    ///     Some(OpcMetricTypeConfig::Float) => println!("Temperature is a float value"),
    ///     Some(OpcMetricTypeConfig::Bool) => println!("Temperature is a boolean value"),
    ///     Some(OpcMetricTypeConfig::Int) => println!("Temperature is an integer value"),
    ///     Some(OpcMetricTypeConfig::String) => println!("Temperature is a string value"),
    ///     None => println!("Metric type not configured"),
    /// }
    /// ```
    pub fn get_metric_type(
        &self,
        chirpstack_metric_name: &String,
        device_id: &String,
    ) -> Option<OpcMetricTypeConfig> {
        debug!(
            metric_name = %chirpstack_metric_name,
            device_id = %device_id,
            "Looking up metric type"
        );

        // Get the metric list for the device
        let metric_list = self.get_metric_list(device_id)?;

        trace!(metric_list = ?metric_list, "Metric list for device");

        // Search for the specific metric
        for metric in metric_list.iter() {
            if metric.chirpstack_metric_name == *chirpstack_metric_name {
                return Some(metric.metric_type.clone());
            }
        }
        None
    }
}

/// Configuration module test suite.
///
/// Tests various aspects of the configuration loading and lookup functionality
/// using a test configuration file. These tests verify that the configuration
/// system correctly parses TOML files and provides accurate data retrieval.
#[cfg(test)]
mod tests {
    use super::*;

    /// Loads test configuration from a TOML file.
    ///
    /// Uses a test-specific configuration file to avoid dependencies on
    /// production configuration. The file path can be overridden using
    /// the `CONFIG_PATH` environment variable.
    ///
    /// # Returns
    ///
    /// * `AppConfig` - The loaded test configuration
    ///
    /// # Panics
    ///
    /// Panics if the test configuration file cannot be loaded or parsed.
    /// This is appropriate for test scenarios where configuration errors
    /// should cause immediate test failure.
    fn get_config() -> AppConfig {
        let current_dir = std::env::current_dir().unwrap();
        println!("Current working directory: {:?}", current_dir);
        let config_path =
            std::env::var("CONFIG_PATH").unwrap_or_else(|_| "tests/config/config.toml".to_string());
        debug!(config_path = %config_path, "Loading test config");
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load test configuration");
        config
    }

    /// Tests application name lookup by ID.
    ///
    /// Verifies that the configuration system can correctly resolve
    /// application names from their IDs, and returns `None` for
    /// non-existent applications.
    #[test]
    fn test_get_application_name() {
        let config = get_config();
        let application_id = String::from("application_1");
        let no_application_id = String::from("no_application");
        let expected_name = String::from("Application01");

        assert_eq!(
            config.get_application_name(&application_id),
            Some(expected_name)
        );
        assert_eq!(config.get_application_name(&no_application_id), None);
    }

    /// Tests application ID lookup by name.
    ///
    /// Verifies that the configuration system can correctly resolve
    /// application IDs from their display names, and returns `None`
    /// for non-existent applications.
    #[test]
    fn test_get_application_id() {
        let config = get_config();
        let application_name = String::from("Application01");
        let no_application_name = String::from("no_Application");
        let expected_application_id = String::from("application_1");

        assert_eq!(
            config.get_application_id(&application_name),
            Some(expected_application_id)
        );
        assert_eq!(config.get_application_id(&no_application_name), None);
    }

    /// Tests device name lookup by ID.
    ///
    /// Verifies that the configuration system can correctly resolve
    /// device names from their IDs across all applications, and
    /// returns `None` for non-existent devices.
    #[test]
    fn test_get_device_name() {
        let config = get_config();
        let device_id = String::from("device_1");
        let no_device_name = String::from("no_device");
        let expected_device_name = String::from("Device01");

        assert_eq!(
            config.get_device_name(&device_id),
            Some(expected_device_name)
        );
        assert_eq!(config.get_device_name(&no_device_name), None);
    }

    /// Tests device ID lookup by name within an application.
    ///
    /// Verifies that the configuration system can correctly resolve
    /// device IDs from their names within a specific application context.
    #[test]
    fn test_get_device_id() {
        let config = get_config();
        let application_id = String::from("application_1");
        let device_name = String::from("Device01");
        let no_device_name = String::from("no_Device");
        let expected_device_id = String::from("device_1");

        assert_eq!(
            config.get_device_id(&device_name, &application_id),
            Some(expected_device_id)
        );
        assert_eq!(config.get_device_id(&no_device_name, &application_id), None);
    }

    /// Tests metric list retrieval for devices.
    ///
    /// Verifies that the configuration system correctly returns metric
    /// lists for existing devices and `None` for non-existent devices.
    /// Also validates the expected number of metrics in the test configuration.
    #[test]
    fn test_get_metric_list() {
        let config = get_config();
        let device_id = String::from("device_1");
        let no_device_id = String::from("no_device");

        let metric_list = config.get_metric_list(&device_id);
        let no_metric_list = config.get_metric_list(&no_device_id);

        println!("Metric list: {:?}", metric_list);
        assert!(metric_list.is_some());
        assert_eq!(metric_list.unwrap().len(), 2);
        assert!(no_metric_list.is_none());
    }

    /// Tests metric type lookup functionality.
    ///
    /// Verifies that the configuration system correctly resolves metric
    /// types for valid device and metric combinations, and returns `None`
    /// for invalid combinations.
    #[test]
    fn test_get_metric_type() {
        let config = get_config();
        let device_id = String::from("device_1");
        let no_device_id = String::from("no_device");
        let chirpstack_metric_name = String::from("metric_1");
        let no_chirpstack_metric_name = String::from("no_metric");
        let expected_metric_type = OpcMetricTypeConfig::Float;

        assert_eq!(
            config.get_metric_type(&chirpstack_metric_name, &device_id),
            Some(expected_metric_type)
        );
        assert_eq!(
            config.get_metric_type(&no_chirpstack_metric_name, &device_id),
            None
        );
        assert_eq!(
            config.get_metric_type(&chirpstack_metric_name, &no_device_id),
            None
        );
    }

    /// Tests global application configuration.
    ///
    /// Verifies that global configuration parameters are correctly
    /// loaded from the test configuration file.
    #[test]
    fn test_application_global_config() {
        let config = get_config();
        assert!(config.global.debug);
    }

    /// Tests ChirpStack configuration parameters.
    ///
    /// Verifies that ChirpStack-specific configuration values are
    /// correctly loaded and match expected test values.
    #[test]
    fn test_chirpstack_config() {
        let config = get_config();
        assert_eq!(config.chirpstack.server_address, "http://localhost:8080");
        assert_eq!(config.chirpstack.api_token, "test_token");
        assert_eq!(config.chirpstack.tenant_id, "tenant_id");
        assert_eq!(config.chirpstack.polling_frequency, 10);
    }

    /// Tests application configuration loading.
    ///
    /// Verifies that the application list is correctly loaded and that
    /// application lookup functions work with the test data.
    #[test]
    fn test_application_config() {
        let config = get_config();

        // Verify applications were loaded
        assert!(!config.application_list.is_empty());

        // Test specific application lookups
        assert_eq!(
            config
                .get_application_name(&"application_1".to_string())
                .unwrap(),
            "Application01"
        );
        assert_eq!(
            config
                .get_application_name(&"application_2".to_string())
                .unwrap(),
            "Application02"
        );
        assert_eq!(
            config
                .get_application_id(&"Application02".to_string())
                .unwrap(),
            "application_2"
        );

        // Test non-existent application
        assert_eq!(
            config.get_application_id(&"noapplication".to_string()),
            None
        );
    }

    /// Tests device configuration loading.
    ///
    /// Verifies that device lists are correctly loaded and that device
    /// lookup functions work with the test data.
    #[test]
    fn test_devices_config() {
        let config = get_config();

        // Verify basic structure
        assert!(!config.application_list.is_empty());
        assert!(!config.application_list[0].device_list.is_empty());

        // Test device name lookup
        assert_eq!(
            config.get_device_name(&"device_1".to_string()).unwrap(),
            "Device01"
        );

        // Test device ID lookup
        assert_eq!(
            config
                .get_device_id(&"Device01".to_string(), &"application_1".to_string())
                .unwrap(),
            "device_1"
        );
    }

    /// Tests that valid configuration passes validation.
    ///
    /// Verifies that a properly configured instance successfully
    /// passes all validation checks.
    #[test]
    fn test_validation_valid_config() {
        let config = get_config();
        assert!(config.validate().is_ok());
    }

    /// Tests that missing required field produces clear error.
    ///
    /// Verifies that validation detects missing ChirpStack server address
    /// and returns an appropriate error message.
    #[test]
    fn test_validation_missing_chirpstack_address() {
        let mut config = get_config();
        config.chirpstack.server_address = String::new();

        let result = config.validate();
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("server_address"));
        assert!(error_msg.contains("must not be empty"));
    }

    /// Tests that invalid polling_frequency (0) produces clear error.
    ///
    /// Verifies that validation detects polling frequency of 0
    /// and returns an appropriate error message.
    #[test]
    fn test_validation_invalid_polling_frequency() {
        let mut config = get_config();
        config.chirpstack.polling_frequency = 0;

        let result = config.validate();
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("polling_frequency"));
        assert!(error_msg.contains("greater than 0"));
    }

    /// Tests that empty application_list produces clear error.
    ///
    /// Verifies that validation detects an empty application list
    /// and returns an appropriate error message.
    #[test]
    fn test_validation_empty_application_list() {
        let mut config = get_config();
        config.application_list.clear();

        let result = config.validate();
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("application_list"));
        assert!(error_msg.contains("at least one"));
    }

    /// Tests that duplicate device_ids produces clear error.
    ///
    /// Verifies that validation detects duplicate device IDs
    /// across applications and returns an appropriate error message.
    #[test]
    fn test_validation_duplicate_device_ids() {
        let mut config = get_config();

        // Create a duplicate device ID in a second application
        if config.application_list.len() > 1 {
            config.application_list[1].device_list[0].device_id =
                config.application_list[0].device_list[0].device_id.clone();

            let result = config.validate();
            assert!(result.is_err());
            let error_msg = result.unwrap_err().to_string();
            assert!(error_msg.contains("duplicated"));
        }
    }

    /// Tests that invalid OPC UA port (0) produces clear error.
    ///
    /// Verifies that validation detects invalid port 0
    /// and returns an appropriate error message.
    #[test]
    fn test_validation_invalid_opc_ua_port() {
        let mut config = get_config();
        config.opcua.host_port = Some(0);

        let result = config.validate();
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("host_port"));
        assert!(error_msg.contains("must not be 0"));
    }

    /// Tests that empty device list in application produces clear error.
    ///
    /// Verifies that validation detects applications with no devices
    /// and returns an appropriate error message.
    #[test]
    fn test_validation_empty_device_list() {
        let mut config = get_config();
        config.application_list[0].device_list.clear();

        let result = config.validate();
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("device_list"));
        assert!(error_msg.contains("at least one device"));
    }

    /// Tests that invalid URL format in server address produces error.
    ///
    /// Verifies that validation detects invalid ChirpStack server address
    /// format and returns an appropriate error message.
    #[test]
    fn test_validation_invalid_server_address_format() {
        let mut config = get_config();
        config.chirpstack.server_address = "invalid-url".to_string();

        let result = config.validate();
        assert!(result.is_err());
        let error_msg = result.unwrap_err().to_string();
        assert!(error_msg.contains("server_address"));
        assert!(error_msg.contains("http://") || error_msg.contains("https://"));
    }
}

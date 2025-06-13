// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Storage Management Module
//!
//! This module provides a centralized storage service for the OPC UA ChirpStack Gateway.
//! It serves as the data layer that:
//! - Stores device metrics collected from ChirpStack
//! - Provides data access for the OPC UA server
//! - Maintains ChirpStack server status information
//! - Manages device and metric lifecycle
//!
//! # Architecture
//!
//! The storage system uses an in-memory approach with HashMap-based indexing for
//! fast device and metric lookups. Data is organized hierarchically:
//! ```text
//! Storage
//! ├── ChirpStack Status (availability, response time)
//! └── Devices (by device_id)
//!     ├── Device Name
//!     └── Metrics (by metric_name)
//!         └── Metric Values (typed)
//! ```
//!
//! # Thread Safety
//!
//! This module is designed to be used with Tokio's async runtime and requires
//! external synchronization (e.g., Arc<Mutex<Storage>>) for concurrent access.
//!
//! # Usage
//!
//! ```rust,no_run
//! use crate::storage::{Storage, MetricType};
//! use crate::config::AppConfig;
//!
//! let config = AppConfig::new()?;
//! let mut storage = Storage::new(&config);
//!
//! // Set a metric value
//! storage.set_metric_value(
//!     &"device_123".to_string(),
//!     "temperature",
//!     MetricType::Float(23.5)
//! );
//!
//! // Retrieve a metric value
//! if let Some(value) = storage.get_metric_value("device_123", "temperature") {
//!     println!("Temperature: {:?}", value);
//! }
//! ```

#![allow(unused)]

use crate::chirpstack::{ApplicationDetail, ChirpstackPoller, DeviceListDetail};
use crate::config::OpcMetricTypeConfig;
use crate::utils::*;
use crate::{storage, AppConfig};
use log::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Supported metric data types for ChirpStack device measurements.
///
/// This enum represents the different types of metric values that can be
/// collected from ChirpStack devices and stored in the gateway. Each variant
/// corresponds to a specific data type that can be exposed via OPC UA.
///
/// # Type Mapping
///
/// The metric types map to OPC UA data types as follows:
/// - `Bool` → OPC UA Boolean
/// - `Int` → OPC UA Int64
/// - `Float` → OPC UA Double
/// - `String` → OPC UA String
///
/// # Examples
///
/// ```rust
/// use crate::storage::MetricType;
///
/// let temperature = MetricType::Float(23.5);
/// let alarm_status = MetricType::Bool(false);
/// let packet_count = MetricType::Int(1024);
/// let device_info = MetricType::String("Online".to_string());
/// ```
#[derive(Clone, Debug, PartialEq)]
pub enum MetricType {
    /// Boolean value for binary states, alarms, or flags.
    ///
    /// Typically used for:
    /// - Device online/offline status
    /// - Alarm conditions
    /// - Binary sensor states
    Bool(bool),

    /// 64-bit signed integer for discrete measurements and counters.
    ///
    /// Typically used for:
    /// - Packet counters
    /// - Sequence numbers
    /// - Discrete sensor readings
    Int(i64),

    /// Double-precision floating-point for analog measurements.
    ///
    /// Typically used for:
    /// - Temperature readings
    /// - Humidity percentages
    /// - Voltage measurements
    /// - Any continuous analog value
    Float(f64),

    /// String value for textual data and formatted information.
    ///
    /// Typically used for:
    /// - Device status messages
    /// - Firmware versions
    /// - Location descriptions
    /// - Formatted sensor readings
    String(String),
}

/// Represents a ChirpStack LoRaWAN device and its associated metrics.
///
/// This structure stores all information related to a single device that is
/// monitored by the gateway. It maintains the device's display name and a
/// collection of its current metric values.
///
/// # Storage Strategy
///
/// Metrics are stored in a HashMap with the ChirpStack metric name as the key
/// and the current value as the payload. This allows for O(1) metric lookups
/// and updates.
///
/// # Note
///
/// Device IDs must be unique across all applications in ChirpStack, while
/// metric names only need to be unique within a single device.
pub struct Device {
    /// Human-readable name of the device as configured in the gateway.
    ///
    /// This name is used for display purposes in the OPC UA address space
    /// and may differ from the device name in ChirpStack.
    device_name: String,

    /// Collection of current metric values for this device.
    ///
    /// The key is the ChirpStack metric name (case-sensitive) and the value
    /// is the current metric reading. Metrics are updated as new data arrives
    /// from ChirpStack polling.
    device_metrics: HashMap<String, MetricType>,
}

/// Structure for enquing commands to chirpstack devices
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceCommand {
    pub device_eui: String,
    pub confirmed: bool,
    pub f_port: u32,
    pub data: Vec<u8>,
}
/// Status information for the ChirpStack server connection.
///
/// This structure tracks the operational status of the ChirpStack server
/// connection, including availability and performance metrics. It is used
/// by the OPC UA server to expose system health information to clients.
///
/// # Usage in OPC UA
///
/// This information is typically exposed as diagnostic nodes in the OPC UA
/// address space, allowing clients to monitor the health of the ChirpStack
/// connection.
#[derive(Clone, Debug, PartialEq)]
pub struct ChirpstackStatus {
    /// Indicates whether the ChirpStack server is reachable and responding.
    ///
    /// This flag is updated by the ChirpStack poller based on the success
    /// or failure of API calls. When `false`, it indicates that the server
    /// is unreachable, authentication has failed, or the server returned
    /// an error status.
    pub server_available: bool,

    /// Average response time of ChirpStack API calls in milliseconds.
    ///
    /// This metric provides insight into the performance of the ChirpStack
    /// server and network connection. It is calculated as a rolling average
    /// of recent API call response times.
    pub response_time: f64,
}

/// Central storage manager for device metrics and system status.
///
/// This structure serves as the main data repository for the gateway,
/// providing a unified interface for storing and retrieving device metrics
/// collected from ChirpStack. It maintains both the current metric values
/// and the operational status of the ChirpStack connection.
///
/// # Design Principles
///
/// - **Fast Lookups**: Uses HashMap-based indexing for O(1) device and metric access
/// - **Type Safety**: Strongly typed metric values with compile-time validation
/// - **Configuration-Driven**: Device structure is initialized from application configuration
/// - **Status Monitoring**: Tracks ChirpStack server health for diagnostics
///
/// # Lifecycle
///
/// 1. **Initialization**: Created with device structure from configuration
/// 2. **Operation**: Continuously updated by ChirpStack poller
/// 3. **Access**: Queried by OPC UA server for client requests
///
/// # Thread Safety
///
/// This structure is not thread-safe by itself. When used in a multi-threaded
/// environment (typical with Tokio), it should be wrapped in appropriate
/// synchronization primitives like `Arc<Mutex<Storage>>`.
pub struct Storage {
    /// Application configuration used to initialize device structure.
    ///
    /// This configuration is cloned during storage initialization and used
    /// for device lookups and validation operations.
    config: AppConfig,

    /// Current status of the ChirpStack server connection.
    ///
    /// Updated periodically by the ChirpStack poller to reflect the current
    /// health and performance of the ChirpStack API connection.
    chirpstack_status: ChirpstackStatus,

    /// Collection of all monitored devices indexed by their ChirpStack device ID.
    ///
    /// The device ID serves as the primary key for device lookups. Each device
    /// contains its display name and current metric values. The structure is
    /// initialized based on the application configuration.
    devices: HashMap<String, Device>,

    device_command_queue: Vec<DeviceCommand>,
}

impl Storage {
    /// Creates a new Storage instance from the provided application configuration.
    ///
    /// This constructor initializes the storage system by parsing the application
    /// configuration and creating the internal device and metric structure. Each
    /// configured device and its associated metrics are pre-allocated with default
    /// values to ensure consistent data access patterns.
    ///
    /// # Arguments
    ///
    /// * `app_config` - Reference to the application configuration containing
    ///   device and metric definitions
    ///
    /// # Returns
    ///
    /// A new `Storage` instance with:
    /// - All configured devices pre-allocated
    /// - All metrics initialized with type-appropriate default values
    /// - ChirpStack status set to default (available, 0ms response time)
    ///
    /// # Device Initialization
    ///
    /// For each device in the configuration:
    /// 1. Creates a `Device` struct with the configured display name
    /// 2. Initializes all configured metrics with default values based on type
    /// 3. Stores the device in the internal HashMap using ChirpStack device ID as key
    ///
    /// # Metric Default Values
    ///
    /// - `Bool` metrics: initialized to `false`
    /// - `Int` metrics: initialized to `0`
    /// - `Float` metrics: initialized to `0.0`
    /// - `String` metrics: initialized to empty string
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use crate::config::AppConfig;
    /// use crate::storage::Storage;
    ///
    /// let config = AppConfig::new()?;
    /// let storage = Storage::new(&config);
    /// println!("Storage initialized with {} devices", storage.devices.len());
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(n) complexity where n is the total number of
    /// configured devices and metrics. It should typically be called once
    /// during application startup.
    pub fn new(app_config: &AppConfig) -> Storage {
        debug!("Creating new Storage instance");
        let mut devices: HashMap<String, Device> = HashMap::new();

        // Process each application in the configuration
        for application in app_config.application_list.iter() {
            debug!("Processing application: {}", application.application_name);

            // Process each device within the application
            for device in application.device_list.iter() {
                debug!(
                    "Initializing device: {} (ID: {})",
                    device.device_name, device.device_id
                );

                // Initialize metrics HashMap for this device
                let mut device_metrics = HashMap::new();
                for metric in device.metric_list.iter() {
                    // Initialize metric with type-appropriate default value
                    let default_value = match metric.metric_type {
                        OpcMetricTypeConfig::Bool => MetricType::Bool(false),
                        OpcMetricTypeConfig::Int => MetricType::Int(0),
                        OpcMetricTypeConfig::Float => MetricType::Float(0.0),
                        OpcMetricTypeConfig::String => MetricType::String(String::new()),
                    };
                    device_metrics.insert(metric.chirpstack_metric_name.clone(), default_value);
                    trace!(
                        "Initialized metric '{}' for device '{}'",
                        metric.chirpstack_metric_name,
                        device.device_id
                    );
                }

                // Create device instance
                let new_device = Device {
                    device_name: device.device_name.clone(),
                    device_metrics,
                };

                // Store device in the main collection
                devices.insert(device.device_id.clone(), new_device);
            }
        }

        debug!(
            "Storage initialization complete with {} devices",
            devices.len()
        );

        debug!("Creating device command queue");
        let mut device_command_queue = Self::create_commands(); //TODO: remove after testing

        Storage {
            config: app_config.clone(),
            chirpstack_status: ChirpstackStatus {
                server_available: true,
                response_time: 0.0,
            },
            devices,
            device_command_queue,
            //device_command_queue: Vec::new(),
        }
    }

    /// Retrieves a mutable reference to a device by its ChirpStack device ID.
    ///
    /// This method provides direct access to a device's internal structure,
    /// allowing for modification of device properties and metrics. It is
    /// primarily used internally by other storage methods.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique ChirpStack identifier for the device
    ///
    /// # Returns
    ///
    /// * `Some(&mut Device)` - Mutable reference to the device if found
    /// * `None` - If no device with the specified ID exists
    ///
    /// # Usage
    ///
    /// This method is typically used by higher-level storage operations
    /// rather than being called directly by external code.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let mut storage = Storage::new(&config);
    /// if let Some(device) = storage.get_device(&"device_123".to_string()) {
    ///     println!("Found device: {}", device.device_name);
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(1) average time complexity due to HashMap indexing.
    pub fn get_device(&mut self, device_id: &String) -> Option<&mut Device> {
        debug!("Retrieving device with ID: {}", device_id);
        self.devices.get_mut(device_id)
    }

    /// Retrieves the display name of a device by its ChirpStack device ID.
    ///
    /// This method looks up a device in the storage and returns its configured
    /// display name. The display name is typically used in the OPC UA address
    /// space and user interfaces.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique ChirpStack identifier for the device
    ///
    /// # Returns
    ///
    /// * `Some(String)` - The device's display name if the device exists
    /// * `None` - If no device with the specified ID is found
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let storage = Storage::new(&config);
    /// match storage.get_device_name(&"device_123".to_string()) {
    ///     Some(name) => println!("Device name: {}", name),
    ///     None => println!("Device not found"),
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(1) average time complexity for the device lookup.
    /// The string clone operation adds minimal overhead.
    pub fn get_device_name(&self, device_id: &String) -> Option<String> {
        debug!("Looking up device name for ID: {}", device_id);
        match self.devices.get(device_id) {
            Some(device) => Some(device.device_name.clone()),
            None => {
                debug!("Device ID '{}' not found", device_id);
                None
            }
        }
    }

    /// Retrieves the current value of a specific metric for a device.
    ///
    /// This method performs a two-level lookup: first finding the device by ID,
    /// then locating the specific metric by its ChirpStack name. It returns a
    /// clone of the metric value to avoid borrowing issues.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique ChirpStack identifier for the device
    /// * `chirpstack_metric_name` - The exact metric name as used in ChirpStack
    ///
    /// # Returns
    ///
    /// * `Some(MetricType)` - A clone of the metric value if found
    /// * `None` - If the device or metric is not found
    ///
    /// # Error Conditions
    ///
    /// This method returns `None` in the following cases:
    /// - Device with the specified ID does not exist
    /// - Device exists but the metric name is not found
    /// - Metric name case mismatch (metric names are case-sensitive)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let mut storage = Storage::new(&config);
    /// match storage.get_metric_value("device_123", "temperature") {
    ///     Some(MetricType::Float(temp)) => println!("Temperature: {}°C", temp),
    ///     Some(other) => println!("Unexpected metric type: {:?}", other),
    ///     None => println!("Metric not found"),
    /// }
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(1) average time complexity for both the device
    /// and metric lookups due to HashMap indexing.
    pub fn get_metric_value(
        &mut self,
        device_id: &str,
        chirpstack_metric_name: &str,
    ) -> Option<MetricType> {
        debug!(
            "Retrieving metric '{}' for device '{}'",
            chirpstack_metric_name, device_id
        );

        // First, find the device
        match self.get_device(&device_id.to_string()) {
            None => {
                debug!("Device '{}' not found", device_id);
                None
            }
            Some(device) => {
                // Then, find the metric within the device
                match device.device_metrics.get(chirpstack_metric_name) {
                    None => {
                        debug!(
                            "Metric '{}' not found for device '{}'",
                            chirpstack_metric_name, device_id
                        );
                        None
                    }
                    Some(metric_value) => {
                        trace!(
                            "Found metric '{}' with value: {:?}",
                            chirpstack_metric_name,
                            metric_value
                        );
                        Some(metric_value.clone())
                    }
                }
            }
        }
    }

    /// Updates the value of a specific metric for a device.
    ///
    /// This method locates the specified device and updates the value of the
    /// named metric. If the metric doesn't exist, it will be created. This is
    /// the primary method used by the ChirpStack poller to update metric values.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique ChirpStack identifier for the device
    /// * `chirpstack_metric_name` - The exact metric name as used in ChirpStack
    /// * `value` - The new metric value to store
    ///
    /// # Panics
    ///
    /// This method panics if the specified device ID is not found in storage.
    /// This is intentional behavior because attempting to set metrics for
    /// non-existent devices indicates a configuration or logic error that
    /// should be caught during development.
    ///
    /// # Error Handling
    ///
    /// Rather than panicking, consider checking device existence first:
    /// ```rust,no_run
    /// if storage.get_device(&device_id).is_some() {
    ///     storage.set_metric_value(&device_id, "temperature", MetricType::Float(23.5));
    /// } else {
    ///     eprintln!("Device {} not found", device_id);
    /// }
    /// ```
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let mut storage = Storage::new(&config);
    ///
    /// // Update temperature reading
    /// storage.set_metric_value(
    ///     &"device_123".to_string(),
    ///     "temperature",
    ///     MetricType::Float(23.5)
    /// );
    ///
    /// // Update alarm status
    /// storage.set_metric_value(
    ///     &"device_123".to_string(),
    ///     "alarm_active",
    ///     MetricType::Bool(true)
    /// );
    /// ```
    ///
    /// # Performance
    ///
    /// This operation has O(1) average time complexity for device lookup
    /// and metric insertion/update due to HashMap indexing.
    ///
    /// # Thread Safety
    ///
    /// This method requires mutable access to the storage and is not thread-safe.
    /// Use appropriate synchronization when calling from multiple threads.
    pub fn set_metric_value(
        &mut self,
        device_id: &String,
        chirpstack_metric_name: &str,
        value: MetricType,
    ) {
        debug!(
            "Setting metric '{}' = {:?} for device '{}'",
            chirpstack_metric_name, value, device_id
        );

        match self.get_device(device_id) {
            Some(device) => {
                device
                    .device_metrics
                    .insert(chirpstack_metric_name.to_string(), value);
                trace!(
                    "Successfully updated metric '{}' for device '{}'",
                    chirpstack_metric_name,
                    device_id
                );
            }
            None => {
                panic!(
                    "Cannot set metric '{}' for device '{}': device not found in storage",
                    chirpstack_metric_name, device_id
                );
            }
        }
    }

    /// Updates the ChirpStack server status information.
    ///
    /// This method updates the stored status information about the ChirpStack
    /// server connection, including availability and response time metrics.
    /// This information is typically updated by the ChirpStack poller and
    /// exposed to OPC UA clients for monitoring purposes.
    ///
    /// # Arguments
    ///
    /// * `status` - New status information containing server availability and response time
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use crate::storage::{Storage, ChirpstackStatus};
    ///
    /// let mut storage = Storage::new(&config);
    /// let status = ChirpstackStatus {
    ///     server_available: false,
    ///     response_time: 5000.0, // 5 seconds timeout
    /// };
    /// storage.update_chirpstack_status(status);
    /// ```
    ///
    /// # Usage in Monitoring
    ///
    /// The updated status information can be exposed via OPC UA diagnostic nodes
    /// to allow clients to monitor the health of the ChirpStack connection.
    pub fn update_chirpstack_status(&mut self, status: ChirpstackStatus) {
        debug!(
            "Updating ChirpStack status: available={}, response_time={}ms",
            status.server_available, status.response_time
        );
        self.chirpstack_status.server_available = status.server_available;
        self.chirpstack_status.response_time = status.response_time;
    }

    /// Retrieves the current ChirpStack server status.
    ///
    /// Returns a clone of the current status information, including server
    /// availability and response time metrics. This method is typically used
    /// by the OPC UA server to expose diagnostic information to clients.
    ///
    /// # Returns
    ///
    /// A clone of the current `ChirpstackStatus` containing:
    /// - `server_available`: Whether the ChirpStack server is reachable
    /// - `response_time`: Average response time in milliseconds
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let storage = Storage::new(&config);
    /// let status = storage.get_chirpstack_status();
    /// println!("ChirpStack available: {}", status.server_available);
    /// println!("Response time: {}ms", status.response_time);
    /// ```
    pub fn get_chirpstack_status(&self) -> ChirpstackStatus {
        self.chirpstack_status.clone()
    }

    /// Checks if the ChirpStack server is currently available.
    ///
    /// This is a convenience method that returns only the availability flag
    /// from the ChirpStack status. It's useful for quick availability checks
    /// without needing the full status structure.
    ///
    /// # Returns
    ///
    /// * `true` - ChirpStack server is available and responding
    /// * `false` - ChirpStack server is unreachable or not responding
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let storage = Storage::new(&config);
    /// if storage.get_chirpstack_available() {
    ///     println!("ChirpStack server is online");
    /// } else {
    ///     println!("ChirpStack server is offline");
    /// }
    /// ```
    ///
    /// # Usage
    ///
    /// This method is particularly useful for:
    /// - Conditional logic based on server availability
    /// - Simple status display in user interfaces
    /// - Health check endpoints
    pub fn get_chirpstack_available(&self) -> bool {
        self.chirpstack_status.server_available
    }

    /// Retrieves the current ChirpStack server response time.
    ///
    /// Returns the average response time for ChirpStack API calls in milliseconds.
    /// This metric provides insight into the performance of the ChirpStack
    /// connection and can be used for monitoring and alerting.
    ///
    /// # Returns
    ///
    /// Response time in milliseconds as a floating-point number.
    /// A value of 0.0 typically indicates either:
    /// - No API calls have been made yet
    /// - The server is not responding (check availability first)
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let storage = Storage::new(&config);
    /// let response_time = storage.get_chirpstack_response_time();
    /// if response_time > 1000.0 {
    ///     println!("Warning: ChirpStack response time is high: {}ms", response_time);
    /// }
    /// ```
    ///
    /// # Performance Monitoring
    ///
    /// This value can be used to:
    /// - Detect network performance issues
    /// - Monitor ChirpStack server performance
    /// - Trigger alerts for degraded performance
    /// - Adjust polling frequency based on response times
    pub fn get_chirpstack_response_time(&self) -> f64 {
        self.chirpstack_status.response_time
    }

    /// Logs all stored metrics to the debug output.
    ///
    /// This diagnostic method iterates through all devices and their metrics,
    /// logging detailed information about the current state of the storage.
    /// It's primarily used for debugging and troubleshooting purposes.
    ///
    /// # Output Format
    ///
    /// The method logs information at different levels:
    /// - **Debug**: "Dumping metrics from storage" (method start)
    /// - **Trace**: Device information and metric values
    ///
    /// Currently, only `Float` type metrics are logged in detail. Other metric
    /// types are processed but not logged (empty match arms).
    ///
    /// # Log Output Example
    ///
    /// ```text
    /// DEBUG: Dumping metrics from storage
    /// TRACE: Device name 'Temperature Sensor 01', id: 'device_123'
    /// TRACE:     Metric "temperature": 23.5
    /// TRACE:     Metric "humidity": 65.2
    /// ```
    ///
    /// # Usage
    ///
    /// This method is typically called:
    /// - During debugging sessions
    /// - After metric updates to verify storage state
    /// - In diagnostic routines
    /// - When troubleshooting metric collection issues
    ///
    /// # Performance Considerations
    ///
    /// This method iterates through all devices and metrics, so it has O(n*m)
    /// complexity where n is the number of devices and m is the average number
    /// of metrics per device. Use sparingly in production due to logging overhead.
    ///
    /// # Future Enhancement
    ///
    /// The empty match arms for `Bool`, `Int`, and `String` types suggest that
    /// logging for these types may be added in future versions.
    pub fn dump_storage(&mut self) {
        debug!("Dumping metrics from storage");
        for (device_id, device) in &self.devices {
            trace!("Device name '{}', id: '{}'", device.device_name, device_id);
            for (metric_name, metric) in device.device_metrics.iter() {
                match metric {
                    MetricType::Bool(value) => {
                        trace!("    Metric '{}': {}", metric_name, value);
                    }
                    MetricType::Int(value) => {
                        trace!("    Metric '{}': {}", metric_name, value);
                    }
                    MetricType::Float(value) => {
                        trace!("    Metric '{}': {}", metric_name, value);
                    }
                    MetricType::String(value) => {
                        trace!("    Metric '{}': {}", metric_name, value);
                    }
                }
            }
        }
    }

    /// Adds a new command to the end of the device command queue.
    ///
    /// This function appends the given command to the queue, which will be processed
    /// in LIFO (Last In, First Out) order when dequeued.
    ///
    /// # Parameters
    /// * `command` - The `DeviceCommand` to add to the queue
    ///
    /// # Examples
    /// ```
    /// let mut storage = Storage::new(&config);
    /// let command = DeviceCommand {
    ///     device_eui: "1234567890ABCDEF".to_string(),
    ///     confirmed: true,
    ///     f_port: 1,
    ///     data: vec![0x01, 0x02, 0x03],
    /// };
    /// storage.enqueue_command(command);
    /// ```
    pub fn push_command(&mut self, command: DeviceCommand) {
        self.device_command_queue.push(command);
    }

    /// Removes and returns the last command from the device command queue.
    ///
    /// This function operates in LIFO (Last In, First Out) order, removing the most
    /// recently added command from the queue.
    ///
    /// # Returns
    /// * `Some(DeviceCommand)` - The last command in the queue if one exists
    /// * `None` - If the command queue is empty
    ///
    /// # Examples
    /// ```
    /// let mut storage = Storage::new(&config);
    /// match storage.dequeue_command() {
    ///     Some(command) => println!("Dequeued command for device: {}", command.device_eui),
    ///     None => println!("No commands to dequeue"),
    /// }
    /// ```
    pub fn pop_command(&mut self) -> Option<DeviceCommand> {
        self.device_command_queue.pop()
    }

    /// Returns a copy of the device command queue if it contains commands, or None if empty.
    ///
    /// # Returns
    /// * `Some(Vec<DeviceCommand>)` - A clone of the command queue if it has at least one command
    /// * `None` - If the command queue is empty
    ///
    /// # Examples
    /// ```
    /// let storage = Storage::new(&config);
    /// match storage.get_device_command_queue() {
    ///     Some(commands) => println!("Found {} commands", commands.len()),
    ///     None => println!("No commands in queue"),
    /// }
    /// ```
    pub fn get_device_command_queue(&self) -> Vec<DeviceCommand> {
        self.device_command_queue.clone()
    }

    //TODO: remove after testing
    fn create_commands() -> Vec<DeviceCommand> {
        trace!("Creating commands");
        // Create a list of command for testing
        let vanne1_command = DeviceCommand {
            device_eui: "524d1e0a02243201".to_string(),
            confirmed: false,
            f_port: 10,
            data: vec![0x02],
        };

        let vanne2_command = DeviceCommand {
            device_eui: "3f8e3904c1523201".to_string(),
            confirmed: false,
            f_port: 10,
            data: vec![0x02],
        };

        let vanne3_command = DeviceCommand {
            device_eui: "999b3d04c1523201".to_string(),
            confirmed: false,
            f_port: 10,
            data: vec![0x02],
        };

        let mut device_command_queue = vec![
            vanne1_command,
            vanne2_command,
            vanne3_command
        ];
        debug!("Command queue is {:?}", device_command_queue);
        device_command_queue
    }
}

/// Storage module test suite.
///
/// This module contains comprehensive tests for the storage functionality,
/// including device management, metric operations, and status tracking.
/// Tests use a dedicated test configuration to ensure isolation from
/// production settings.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;
    use figment::{
        providers::{Format, Toml},
        Figment,
    };

    /// Loads test configuration from a TOML file.
    ///
    /// This helper function provides a consistent way to load test configuration
    /// across all test cases. It uses a test-specific configuration file to
    /// avoid dependencies on production configuration.
    ///
    /// # Configuration Path
    ///
    /// The configuration file path is determined by:
    /// - `CONFIG_PATH` environment variable if set
    /// - Default: `"tests/config/config.toml"`
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
        let config_path =
            std::env::var("CONFIG_PATH").unwrap_or_else(|_| "tests/config/config.toml".to_string());
        debug!("Loading test configuration from: {}", config_path);
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load test configuration");
        config
    }

    /// Tests ChirpStack status management functionality.
    ///
    /// This test verifies the complete lifecycle of ChirpStack status handling:
    /// 1. **Initial State**: Verifies default status after storage creation
    /// 2. **Status Update**: Tests updating status with new values
    /// 3. **Status Retrieval**: Verifies all status accessor methods
    ///
    /// # Test Scenarios
    ///
    /// - Initial status: server available = true, response time = 0.0
    /// - Updated status: server available = false, response time = 1.0ms
    /// - Accessor method consistency across different retrieval methods
    ///
    /// # Assertions
    ///
    /// - Initial state matches expected defaults
    /// - Status update correctly modifies stored values
    /// - All accessor methods return consistent values
    /// - Status structure equality works correctly
    #[test]
    fn test_chirpstack_status() {
        let response_time = 1.0;
        let status = false;
        let app_config = get_config();
        let mut storage = Storage::new(&app_config);

        // Test initial status
        assert_eq!(storage.chirpstack_status.server_available, true);
        assert_eq!(storage.chirpstack_status.response_time, 0.0);

        // Test status update
        let chirpstack_status = ChirpstackStatus {
            server_available: status,
            response_time,
        };
        storage.update_chirpstack_status(chirpstack_status.clone());

        // Test status retrieval methods
        assert_eq!(storage.get_chirpstack_status(), chirpstack_status);
        assert_eq!(storage.get_chirpstack_available(), status);
        assert_eq!(storage.get_chirpstack_response_time(), response_time);
    }

    /// Tests configuration loading and storage initialization.
    ///
    /// This test verifies that:
    /// 1. Test configuration loads successfully
    /// 2. Storage initializes with the loaded configuration
    /// 3. At least one application is present in the configuration
    ///
    /// This is a basic smoke test to ensure the test infrastructure
    /// is working correctly.
    #[test]
    fn test_load_metrics() {
        let app_config = get_config();
        let storage = Storage::new(&app_config);

        // Verify that we loaded a meaningful configuration
        assert!(storage.config.application_list.len() > 0);
    }

    /// Tests device retrieval functionality.
    ///
    /// Verifies that the `get_device` method correctly retrieves devices
    /// that were initialized from the configuration. Uses a known device ID
    /// from the test configuration.
    ///
    /// # Test Data
    ///
    /// Assumes the test configuration contains a device with ID "device_1".
    #[test]
    fn test_get_device() {
        let mut storage = Storage::new(&get_config());
        let device = storage.get_device(&String::from("device_1"));

        // Verify device exists
        assert!(device.is_some());
    }

    /// Tests device name retrieval functionality.
    ///
    /// Verifies that the `get_device_name` method correctly:
    /// 1. Returns the expected name for existing devices
    /// 2. Returns `None` for non-existent devices
    ///
    /// # Test Data
    ///
    /// - Existing device: "device_1" should map to "Device01"
    /// - Non-existent device: "no_device" should return `None`
    #[test]
    fn test_get_device_name() {
        let storage = Storage::new(&get_config());
        let device_id = String::from("device_1");
        let no_device_id = String::from("no_device");

        // Test existing device
        assert_eq!(
            storage.get_device_name(&device_id),
            Some("Device01".to_string())
        );

        // Test non-existent device
        assert_eq!(storage.get_device_name(&no_device_id), None);
    }

    /// Tests metric value setting for non-existent devices.
    ///
    /// This test verifies that attempting to set a metric value for a device
    /// that doesn't exist in storage results in a panic. This behavior is
    /// intentional as it indicates a programming error that should be caught
    /// during development.
    ///
    /// # Expected Behavior
    ///
    /// The test should panic when trying to set a metric for "no_device"
    /// which is not present in the test configuration.
    ///
    /// # Safety Note
    ///
    /// The `#[should_panic]` attribute ensures this test passes only if
    /// a panic occurs, validating the error handling behavior.
    #[test]
    #[should_panic(expected = "Cannot set metric")]
    fn test_set_metric_value_panic() {
        let mut storage = Storage::new(&get_config());
        let no_device_id = String::from("no_device");
        let no_metric = String::from("no_metric");
        let value = 10.0;

        // This should panic
        storage.set_metric_value(&no_device_id, &no_metric, storage::MetricType::Float(value));
    }

    /// Tests metric value setting and retrieval functionality.
    ///
    /// This comprehensive test verifies the complete metric lifecycle:
    /// 1. **Setting Values**: Updates a metric for an existing device
    /// 2. **Retrieving Values**: Confirms the stored value matches the set value
    /// 3. **Error Cases**: Tests retrieval for non-existent devices and metrics
    ///
    /// # Test Scenarios
    ///
    /// - Valid device + valid metric: should succeed
    /// - Invalid device + valid metric: should return `None`
    /// - Valid device + invalid metric: should return `None`
    ///
    /// # Test Data
    ///
    /// - Device: "device_1" (existing)
    /// - Metric: "metric_1" (should exist in test config)
    /// - Value: 10.0 (Float type)
    #[test]
    fn test_metric_operations() {
        let app_config = get_config();
        let mut storage = Storage::new(&app_config);
        let device_id = String::from("device_1");
        let no_device_id = String::from("no_device");
        let metric = String::from("metric_1");
        let no_metric = String::from("no_metric");
        let value = 10.0;

        // Test setting and getting metric value
        storage.set_metric_value(&device_id, &metric, storage::MetricType::Float(value));
        assert_eq!(
            storage.get_metric_value(&device_id, &metric),
            Some(MetricType::Float(value))
        );

        // Test error cases
        assert_eq!(storage.get_metric_value(&no_device_id, &metric), None);
        assert_eq!(storage.get_metric_value(&device_id, &no_metric), None);
    }

    #[test]
    fn test_command_queue() {
        let mut storage = Storage::new(&get_config());
        let command = DeviceCommand {
            device_eui: "device01".to_string(),
            confirmed: true,
            f_port: 100,
            data: vec![10, 20],
        };
        storage.push_command(command);
        let result = storage.pop_command();

        assert_eq!(result.clone().unwrap().device_eui, "device01");
        assert_eq!(result.clone().unwrap().confirmed, true);
        assert_eq!(result.clone().unwrap().f_port, 100);
        assert_eq!(result.clone().unwrap().data, [10, 20]);
    }
}
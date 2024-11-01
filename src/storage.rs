// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage storage
//!
//! Provide storage service for both
//! Chirpstack poller, that updated data
//! and opc ua server, that retrieves data
//!

#![allow(unused)]

use crate::chirpstack::{ApplicationDetail, ChirpstackPoller, DeviceListDetail};
use crate::config::MetricTypeConfig;
use crate::AppConfig;
use log::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Type of metric returned by Chirpstack server
#[derive(Clone, Debug, PartialEq)]
pub enum MetricType {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

/// Structure for storing metrics
/// It is necessary to store device id as well to identify the different metrics as
/// metric name are not unique in chirpstack. However, device_id is unique.
pub struct Device {
    /// The chirpstack name of the device
    device_name: String,
    /// The list of metrics. First field is chirpstack metric name, second field is the value
    device_metrics: HashMap<String, MetricType>,
}

/// Main structure for storing application data, metrics, and managing devices and applications.
pub struct Storage {
    config: AppConfig,
    /// Device. First field is device id, second field is device
    devices: HashMap<String, Device>,
}

impl Storage {
    /// Creates a new instance of `Storage` from the provided `AppConfig`.
    ///
    /// This function initializes a `Storage` instance by parsing the application's configuration,
    /// including its devices and their respective metrics. Each device and metric is added to
    /// respective hashmaps for quick look-up.
    ///
    /// # Arguments
    ///
    /// * `app_config` - A reference to the application's configuration.
    ///
    /// # Returns
    ///
    /// * A new instance of `Storage`.
    pub fn new(app_config: &AppConfig) -> Storage {
        debug!("Creating a new Storage instance");
        let mut devices: HashMap<String, Device> = HashMap::new();
        // Parse applications
        for application in app_config.application_list.iter() {
            // Parse device
            for device in application.device_list.iter() {
                let new_device = Device {
                    device_name: device.device_name.clone(),
                    device_metrics: HashMap::new(),
                };
                let device_id = device.device_id.clone();
                let mut device_metrics = HashMap::new();
                for metric in device.metric_list.iter() {
                    let metric_type = match metric.metric_type {
                        MetricTypeConfig::Bool => MetricType::Bool(false),
                        MetricTypeConfig::Int => MetricType::Int(0),
                        MetricTypeConfig::Float => MetricType::Float(0.0),
                        MetricTypeConfig::String => MetricType::String("".to_string()),
                    };
                    device_metrics.insert(metric.metric_name.clone(), MetricType::Float(0.0));
                }
                devices.insert(device_id, new_device);
            }
        }
        Storage {
            config: app_config.clone(),
            devices,
        }
    }

    /// Retrieves a device by its ID from the collection of devices.
    ///
    /// # Arguments
    ///
    /// * `device_id` - A reference to a `String` containing the ID of the device to retrieve.
    ///
    /// # Returns
    ///
    /// * `Option<&Device>` - An `Option` which will be `Some(&Device)` if the device is found,
    ///   or `None` if the device is not found.
    ///
    /// # Example
    ///
    /// ```
    /// let device_opt = instance.get_device(&"device123".to_string());
    /// if let Some(device) = device_opt {
    ///     println!("Device found: {:?}", device);
    /// } else {
    ///     println!("Device not found.");
    /// }
    /// ```
    pub fn get_device(&self, device_id: &String) -> Option<&Device> {
        trace!("Getting device {}", device_id);
        self.devices.get(device_id)
    }

    /// Retrieves the name of a device given its device ID.
    ///
    /// # Arguments
    ///
    /// * `device_id` - A reference to a String that holds the ID of the device.
    ///
    /// # Returns
    ///
    /// * A String representing the device's name.
    ///
    /// # Panics
    ///
    /// This function will panic if the device with the specified ID is not found
    /// within `self.devices`.
    pub fn get_device_name(&self, device_id: &String) -> String {
        let device = self
            .devices
            .get(device_id)
            .expect(format!("Device '{}' not found", device_id).as_str());
        device.device_name.clone()
    }

    /// Retrieves the metric value for a specified device and metric name.
    ///
    /// # Arguments
    ///
    /// * `device_id` - A string slice that holds the unique identifier of the device.
    /// * `chirpstack_metric_name` - A string slice that holds the name of the metric to retrieve.
    ///
    /// # Returns
    ///
    /// * `MetricType` - The value of the specified metric for the given device.
    ///
    /// # Panics
    ///
    /// This function will panic if:
    /// * The device with the given `device_id` is not found in the devices list.
    /// * The metric with the given `chirpstack_metric_name` is not found in the device's metrics.
    pub fn get_metric_value(&self, device_id: &str, chirpstack_metric_name: &str) -> MetricType {
        trace!(
            "Getting metric value for device '{}': '{}'",
            device_id,
            chirpstack_metric_name
        );
        // Get device according to its device id
        let device = self
            .devices
            .get(device_id)
            .expect(format!("Device '{}' not found", device_id).as_str());
        // Get metric value according to metric name
        let value = device
            .device_metrics
            .get(chirpstack_metric_name)
            .expect(format!("Metric '{}' not found", chirpstack_metric_name).as_str());
        trace!(
            "Getting metric value for device '{}': '{:?}'",
            device_id,
            value
        );
        value.clone()
    }

    /// Sets the metric value for a specific device.
    ///
    /// This function updates the metric value for the provided device. It retrieves the device
    /// from the internal device storage, updates the specified metric, and then persists the
    /// changes to the storage.
    ///
    /// # Arguments
    ///
    /// * `device_id` - A reference to a `String` that represents the unique identifier of the device.
    /// * `metric_name` - A string slice that holds the name of the metric to be updated.
    /// * `value` - The new value of the metric, of type `MetricType`.
    ///
    /// # Panics
    ///
    /// This function will panic if the device with the specified `device_id` cannot be found.
    ///
    /// # Examples
    ///
    /// ```
    /// let mut storage = Storage::new();
    /// storage.set_metric_value(&"device123".to_string(), "temperature", MetricType::Float(23.5));
    /// ```
    pub fn set_metric_value(
        &mut self,
        device_id: &String,
        chirpstack_metric_name: &str,
        value: MetricType,
    ) {
        //trace!("Setting metric for device'{}', value: '{}'", device_id, metric_name);
        let mut device: &mut Device = self.devices.get_mut(device_id).expect(&format!(
            "Can't get device with id '{}'",
            device_id.as_str()
        ));
        device
            .device_metrics
            .insert(chirpstack_metric_name.to_string(), value);
        //self.dump_storage();
    }

    /// Dumps the storage metrics to the log.
    ///
    /// This function iterates over all devices and their associated metrics,
    /// logging detailed information for each metric. Specifically, only the
    /// `Float` metrics are logged with their names and values.
    ///
    /// # Logs
    /// - At the start, logs "Dumping metrics from storage".
    /// - For each device, logs the device name and ID.
    /// - For each `Float` metric, logs the metric name and its value.
    ///
    /// # Example
    /// ```
    /// self.dump_storage();
    /// ```
    pub fn dump_storage(&mut self) {
        trace!("Dumping metrics from storage");
        for (device_id, device) in &self.devices {
            trace!("Device name '{}', id: '{}'", device.device_name, device_id);
            for (metric_name, metric) in device.device_metrics.iter() {
                match metric {
                    MetricType::Bool(value) => {}
                    MetricType::Int(value) => {}
                    MetricType::Float(value) => {
                        trace!("    Metric {:?}: {:#?}", metric_name, value);
                    }
                    MetricType::String(value) => {}
                }
            }
        }
    }
}

/// Storage tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage;
    use figment::{
        providers::{Format, Toml},
        Figment,
    };

    /// Retrieves the application configuration.
    ///
    /// This function attempts to load the application configuration from a file
    /// specified by the environment variable `CONFIG_PATH`. If the environment
    /// variable is not set, it defaults to `"tests/config/default.toml"`.
    ///
    /// The configuration is loaded using the `Figment` library, which merges the
    /// TOML file contents into a configuration structure of type `AppConfig`.
    ///
    /// # Panics
    ///
    /// This function will panic if it fails to load or parse the configuration
    /// file.
    ///
    /// # Returns
    ///
    /// * `AppConfig` - The application configuration.
    ///
    fn get_config() -> AppConfig {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");
        config
    }

    /// This test function, `test_load_metrics`, verifies that the application configuration
    /// is properly loaded and that the list of applications in the storage configuration
    /// is not empty. It ensures that the function `get_config` loads a valid configuration
    /// and that the `Storage` struct is correctly initialized with this configuration.
    #[test]
    fn test_load_metrics() {
        let app_config = get_config();
        let storage = Storage::new(&app_config);
        assert!(storage.config.application_list.len() > 0); // We loaded something
    }

    /// This test verifies that the `get_device` method of the `Storage` struct
    /// successfully retrieves a device by its identifier. The test ensures that
    /// a device with the identifier "device_1" exists in the storage.
    #[test]
    fn test_get_device() {
        let storage = Storage::new(&get_config());
        let device = storage.get_device(&String::from("device_1"));
        assert!(device.is_some()); // device has bee found
    }

    /// Tests the `get_device_name` method of the `Storage` struct.
    ///
    /// This test initializes a new `Storage` instance using a configuration obtained from `get_config()`,
    /// and then retrieves the name of a device with the ID "device_1". The test verifies that the retrieved
    /// device name matches the expected value "Device01".
    #[test]
    fn get_device_name() {
        let storage = Storage::new(&get_config());
        let device_id = String::from("device_1");
        let device_name = storage.get_device_name(&device_id);
        assert_eq!(device_name, "Device01");
    }

    /// This test function verifies the functionality of setting and retrieving a metric value
    /// in the `Storage` struct.
    ///
    /// The following steps are taken:
    /// 1. Retrieve the application configuration using `get_config()`.
    /// 2. Initialize a `Storage` instance with the configuration.
    /// 3. Set a metric value for a device (`device_1`) and a specific metric (`metric_1`) to a
    ///    floating-point value of 10.0 using `set_metric_value()`.
    /// 4. Retrieve the metric value for `device_1` and `metric_1` using `get_metric_value()`.
    /// 5. Assert that the retrieved value matches the value that was set (10.0).
    ///
    /// This ensures that the `set_metric_value` and `get_metric_value` functions in the `Storage`
    /// struct work correctly for the given inputs.
    #[test]
    fn test_metric() {
        let app_config = get_config();
        let mut storage = Storage::new(&app_config);
        storage.set_metric_value(
            &"device_1".to_string(),
            &"metric_1".to_string(),
            storage::MetricType::Float(10.0),
        );
        let metric = storage.get_metric_value(&"device_1".to_string(), &"metric_1".to_string());
        assert_eq!(metric, MetricType::Float(10.0));
    }
}

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
use crate::AppConfig;
use crate::config::{MetricTypeConfig};
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
/// It is necessary to store device as well to identify the different metrics as
/// metric name are not unique in chirpstack. However, device_id is unique.
pub struct Device {
    /// The chirpstack name of the device
    pub device_name: String,
    /// The list of metrics. First field is chirpstack metric name, second field is the value
    pub device_metrics: HashMap<String, MetricType>,
}

/// Main structure for storing application data, metrics, and managing devices and applications.
pub struct Storage {
    pub config: AppConfig,
    /// Device. First field is device id, second field is device
    pub devices: HashMap<String, Device>,
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
        let mut devices:HashMap<String, Device> = HashMap::new();
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
                    device_metrics.insert(
                        metric.metric_name.clone(), MetricType::Float(0.0)
                    );
                }
              devices.insert(device_id, new_device);
            }

        }
        Storage {
            config: app_config.clone(),
            devices,
        }
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
        trace!("Getting metric value for device '{}': '{}'", device_id, chirpstack_metric_name);
        // Get device according to its device id
        let device = self.devices.get(device_id)
            .expect(format!("Device '{}' not found", device_id).as_str());
        // Get metric value according to metric name
        let value = device.device_metrics.get(chirpstack_metric_name)
            .expect(format!("Metric '{}' not found", chirpstack_metric_name).as_str());
        trace!("Getting metric value for device '{}': '{:?}'", device_id, value);
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
    pub fn set_metric_value(&mut self, device_id: &String, metric_name: &str, value: MetricType) {
        trace!("Setting metric for device'{}', value: '{}'", device_id, metric_name);
        let mut device: &mut Device = self.devices.get_mut(device_id)
            .expect(&format!("Can't get device with id '{}'", device_id.as_str()));
        device.device_metrics.insert(metric_name.to_string(), value);
        self.dump_storage();
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
        let device = self.devices.get(device_id)
            .expect(format!("Device '{}' not found", device_id).as_str());
        device.device_name.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::{
        providers::{Format, Toml},
        Figment,
    };

    /// Create a config object for test functions
    /// If changes are done on "tests/default.toml"
    /// the tests below might fail.
    fn get_config() -> AppConfig {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");
        config
    }

    /// Test if metrics list is loaded
    #[test]
    fn test_load_metrics() {
        let app_config = get_config();
        let storage = Storage::new(&app_config);
        assert!(storage.config.application_list.len() > 0); // We loaded something
    }

    /// Test if one loaded metric is present
    #[test]
    fn test_get_metric() {
        let app_config = get_config();
        let storage = Storage::new(&app_config);
        let device = storage.devices.get("device_1").unwrap();
        assert_eq!(device.device_name, "Device01".to_string()); // The correct device is loaded
        //FIXME: add correct test
        //let metric = device.device_metrics.get("metric_1").unwrap();
        //assert!(metric.is_some()); // Metric is loaded
    }

    /// Test if one metric value is present
    /// We test with the default value configured
    /// when metric is initialized
    #[test]
    #[ignore]
    pub fn test_get_metric_value() {
        let storage = Storage::new(&get_config());
        let device = storage.devices.get("device_01").unwrap();
        //let metric_value = storage.get_metric_value();
        //assert_eq!(metric_value, Some(MetricType::Float(0.0)));
    }
}

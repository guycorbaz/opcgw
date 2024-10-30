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
    /// Creates and returns a new instance of `Storage`
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

    ///Return a metric value for the device and metric name passed in parameters
    pub fn get_metric_value(&self, device_id: &str, metric_name: &str) -> MetricType {
        trace!("Getting metric value for device '{}': '{}'", device_id, metric_name);
        // Get device according to its device id
        let device = self.devices.get(device_id)
            .expect(format!("Device '{}' not found", device_id).as_str());
        // Get metric value according to metric name
        let value = device.device_metrics.get(metric_name)
            .expect(format!("Metric '{}' not found", metric_name).as_str());
        trace!("Getting metric value for device '{}': '{:?}'", device_id, value);
        value.clone()
    }

    /// Set value for metric name passed in  parameters
    pub fn set_metric_value(&mut self, device_id: &String, metric_name: &str, value: MetricType) {
        trace!("Setting metric for device'{}', value: '{}'", device_id, metric_name);
        let mut device: &mut Device = self.devices.get_mut(device_id)
            .expect(&format!("Can't get device with id '{}'", device_id.as_str()));
        device.device_metrics.insert(metric_name.to_string(), value);
        self.dump_storage();
    }

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

    /// Return the name of device with device_id
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
        let metric = storage.device_metrics.get(&String::from("Metric01"));
        assert!(metric.is_some()); // Metric is loaded
    }

    /// Test if one metric value is present
    /// We test with the default value configured
    /// when metric is initialized
    #[test]
    pub fn test_get_metric_value() {
        let storage = Storage::new(&get_config());
        let metric_value = storage.get_metric_value("Metric01");
        assert_eq!(metric_value, Some(MetricType::Float(0.0)));
    }
}

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
use log::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// structure for storing one metric
pub struct DeviceMetric {
    /// The name of the metric as configured in Chirpstack
    pub metric_name: String,
    /// The timestamp of the metric
    pub metric_timestamp: String,
    /// The value of the metric
    pub metric_value: String,
    /// The kind of metric as defined in Chirpstack
    pub metric_type: String,
}

/// Main structure for storing application data, metrics, and managing devices and applications.
pub struct Storage {
    config: AppConfig,
    /// Mapping of device EUIs to their respective metrics.
    /// String is device_id, DeviceMetric is the metric
    device_metrics: HashMap<String, DeviceMetric>,
}

impl Storage {
    /// Creates and returns a new instance of `Storage`
    pub fn new(app_config: &AppConfig) -> Storage {
        debug!("Creating a new Storage instance");

        Storage {
            config: app_config.clone(),
            device_metrics: HashMap::new(),
        }
    }

    /// Loads the list of applications from the configuration into the storage.
    pub fn load_applications(&mut self) {
        debug!("Loading applications list");
        todo!();
        //for application in &self.config.applications {
        //    println!("Application {}", application.0.clone());
        //    let app = Application {
        //        name: application.0.clone(),
        //        application_id: application.1.clone(),
        //    };
        //    self.application_list.push(app);
        //}
    }

    // Stores device metrics for a given device EUI.
    //pub fn store_device_metrics(&mut self, dev_eui: String, metrics: DeviceMetrics) {
    //    debug!("Storing metrics for device: {}", dev_eui);
    //    self.device_metrics.insert(dev_eui, metrics);
    //}

    // Retrieves device metrics for a given device EUI.
    //pub fn get_device_metrics(&self, dev_eui: &str) -> Option<&DeviceMetrics> {
    //    debug!("Getting metrics for device: {}", dev_eui);
    //    self.device_metrics.get(dev_eui)
    //}
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::{
        providers::{Format, Toml},
        Figment,
    };

    /// Create a config object for test functions
    /// If changes are don on "tests/default.toml"
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
    #[ignore]
    #[test]
    fn test_load_applications() {}

    #[ignore]
    #[test]
    fn test_list_applications() {}

    #[ignore]
    #[test]
    fn test_find_application_name() {}

    #[ignore]
    #[test]
    fn test_load_devices() {}

    #[ignore]
    #[test]
    fn test_find_device_name() {}
}

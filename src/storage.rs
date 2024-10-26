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
use crate::config::MetricTypeConfig;
use log::{debug, error, info, trace, warn};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::mpsc;

/// Type of metric returned by Chirpstack server
pub enum MetricType {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

/// structure for storing one metric
pub struct DeviceMetric {
    // The timestamp of the metric
    //pub metric_timestamp: String,
    /// The value of the metric
    pub metric_value: MetricType,

}

/// Main structure for storing application data, metrics, and managing devices and applications.
pub struct Storage {
    config: AppConfig,
    /// String is metric name, DeviceMetric is the metric
    device_metrics: HashMap<String, MetricType>,
}

impl Storage {
    /// Creates and returns a new instance of `Storage`
    pub fn new(app_config: &AppConfig) -> Storage {
        debug!("Creating a new Storage instance");

        // Build and initialize the device metric storage
        let mut device_metrics = HashMap::new();

        // Parse applications
        for application in app_config.application_list.iter() {
            // Parse device
            for device in application.device_list.iter() {
                // Parse metrics
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
            }
        }
        Storage {
            config: app_config.clone(),
            device_metrics, // HashMap is empty:
        }
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
}

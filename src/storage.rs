// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage storage
//!
//! Provide storage management for opc_ua_chirpstack_gateway
//!
//! # Example:
//! Add example code...

use crate::chirpstack::{ApplicationDetail, ChirpstackPoller, DeviceDetails, DeviceListDetail};
use crate::Config;
use log::{debug, error, info, trace, warn};
use std::collections::HashMap;
use tokio::sync::mpsc;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
pub struct DeviceMetrics {
    pub dev_eui: String,
    pub timestamp: i64,
    pub rx_packets: i32,
    pub gw_rssi: f32,
    pub gw_snr: f32,
    // Ajoutez d'autres champs selon les besoins
}

/// Represents a device in the system.
pub struct Device {
    /// Unique identifier of the device, provided by Chirpstack.
    pub device_id: String,
    /// ID of the application this device belongs to.
    pub application_id: String,
    /// Name of the device.
    pub name: String,
}

/// Represents an application in the system, provided by Chirpstack.
pub struct Application {
    /// Unique identifier of the application.
    pub application_id: String,
    /// Name of the application.
    pub name: String,
}

/// Main structure for storing application data, metrics, and managing devices and applications.
pub struct Storage {
    config: Config,
    /// Mapping of device EUIs to their respective metrics.
    device_metrics: HashMap<String, DeviceMetrics>,
    /// List of applications with their unique identifiers as keys.
    application_list: Vec<Application>,
    /// List of devices with their unique identifiers as keys.
    device_list: Vec<Device>,
}

impl Storage {
    /// Creates and returns a new instance of `Storage`
    pub fn new(app_config: &Config) -> Storage {
        debug!("Creating a new Storage instance");

        Storage {
            config: app_config.clone(),
            device_metrics: HashMap::new(),
            application_list: Vec::new(),
            device_list: Vec::new(),
        }
    }

    /// Loads the list of applications from the configuration into the storage.
    pub fn load_applications(&mut self) {
        debug!("Loading applications list");
        for application in &self.config.applications {
            println!("Application {}", application.0.clone());
            let app = Application {
                name: application.0.clone(),
                application_id: application.1.clone(),
            };
            self.application_list.push(app);
        }
    }

    /// Retrieves the application name for a given application id.
    fn find_application_name(&self, id: &String) -> String {
        for app in self.application_list.iter() {
            if app.application_id == *id {
                return app.name.clone();
            }
        }
        "".to_string()
    }

    /// Prints the list of applications to the console.
    pub fn list_applications(&self) {
        debug!("Listing applications");
        for application in &self.application_list {
            trace!("Application: {:?}", application.name);
        }
    }

    /// Load devices list from configuration
    pub fn load_devices(&mut self) {
        debug!("Loading devices list");
        for device in &self.config.devices {
            let device_id = device.1.device_id.clone();
            let application_id = device.1.application_id.clone();
            let device_name = device.0;
            debug!("Device ID: {}, name {}", device_id, device_name);
            self.device_list.push(Device {
                device_id: device_id.clone(),
                application_id: application_id.to_string(),
                name: device_name.clone(),
            });
        }
    }

    /// Find the device name from the device id
    pub fn find_device_name(&mut self, device_id: &String) -> String {
        for device in &self.device_list {
            if device.device_id == *device_id {
                return device.name.clone();
            }
        }
        "".to_string()
    }

    /// Prints the list of devices to the console.
    pub fn list_devices(&self) {
        debug!("Listing devices");
        for device in &self.device_list {
            println!(
                "Device {:#?}, linked application: {}",
                device.name,
                self.find_application_name(&device.application_id)
            );
        }
    }

    /// Stores a metric with the given key and value.
    pub fn store_metric(&mut self, key: String, value: String) {
        debug!("Storing metric: {} = {}", key, value);
        todo!();
        self.metrics.insert(key, value);
    }

    /// Retrieves a metric value for the given key.
    pub fn get_metric(&self, key: &str) -> Option<&String> {
        debug!("Getting metric: {}", key);
        todo!();
        self.metrics.get(key)
    }

    /// Stores device metrics for a given device EUI.
    pub fn store_device_metrics(&mut self, dev_eui: String, metrics: DeviceMetrics) {
        debug!("Storing metrics for device: {}", dev_eui);
        self.device_metrics.insert(dev_eui, metrics);
    }

    /// Retrieves device metrics for a given device EUI.
    pub fn get_device_metrics(&self, dev_eui: &str) -> Option<&DeviceMetrics> {
        debug!("Getting metrics for device: {}", dev_eui);
        self.device_metrics.get(dev_eui)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use figment::{
        providers::{Format, Toml},
        Figment,
    };
    #[test]
    fn test_load_applications() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");
        let mut storage = Storage::new(&config);

        storage.load_applications(); // What we are testing

        assert_eq!(
            storage.find_application_name(&"Application01".to_string()),
            "application_1"
        );
    }

    #[test]
    fn test_list_applications() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");
        let mut storage = Storage::new(&config);

        storage.load_applications();

        storage.list_applications(); // What we are testing
    }

    #[test]
    fn test_load_devices() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");
        let mut storage = Storage::new(&config);

        storage.load_devices();

        assert!(storage.device_list.len() > 0);
        assert_eq!(
            storage.device_list[0].application_id,
            "Application01".to_string()
        );
        assert_eq!(storage.device_list[0].device_id, "Device01".to_string());
    }

    #[test]
    fn test_find_device_name() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");
        let mut storage = Storage::new(&config);

        storage.load_devices();
        assert_eq!(
            storage.find_device_name(&"Device01".to_string()),
            "device_1"
        );
    }
}

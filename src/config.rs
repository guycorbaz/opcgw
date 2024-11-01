// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage configuration files
//!
//! Provides configuration file management for opc_ua_chirpstack_gateway
//!

#![allow(unused)] //FIXME: Remove for release

use crate::utils::OpcGwError;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use log::{debug, trace};
use serde::Deserialize;
use std::collections::HashMap;

/// Structure for storing global application configuration  parameters.
/// This might change in future
#[derive(Debug, Deserialize, Clone)]
pub struct Global {
    /// Set to true for detailed debug log
    /// Not used now
    pub debug: bool,
}

/// Structure for storing Chirpstack connection parameters
#[derive(Debug, Deserialize, Clone)]
pub struct ChirpstackPollerConfig {
    /// ChirpStack server address.
    pub server_address: String,
    /// API token for authentication with ChirpStack.
    pub api_token: String,
    /// The tenant ID we are working with.
    pub tenant_id: String,
    /// Server polling frequency
    pub polling_frequency: u64,
}

/// Structure for storing opc ua server configuration parameters
/// For the time being, the configuration is
/// coming from a dedicated file. This will be improved
/// in future
#[derive(Debug, Deserialize, Clone)]
pub struct OpcUaConfig {
    /// Config file path for opc ua server
    pub config_file: String,
}

/// Chirpstack application description
/// This defines how to connect to server
#[derive(Debug, Deserialize, Clone)]
pub struct ChirpStackApplications {
    /// Chirpstack application name
    pub application_name: String,
    /// Chirpstack application ID
    pub application_id: String,
    /// The list of devices for the application
    #[serde(rename = "device")]
    pub device_list: Vec<ChirpstackDevice>,
}

/// Structure that holds the data of the device
/// we would like to monitor
#[derive(Debug, Deserialize, Clone)]
pub struct ChirpstackDevice {
    /// The device id defined in chirpstack
    pub device_id: String,
    /// The name that will appear in opc ua
    pub device_name: String,
    /// The list of metrics for the device
    #[serde(rename = "metric")]
    pub metric_list: Vec<Metric>,
}

/// Type of metrics
#[derive(Debug, Deserialize, Clone)]
pub enum MetricTypeConfig {
    Bool,
    Int,
    Float,
    String,
}

/// Structure that holds the data of the device
/// metrics we would like to monitor
#[derive(Debug, Deserialize, Clone)]
pub struct Metric {
    /// The name that will appear in opc ua
    pub metric_name: String,
    /// The name defined in chirpstack
    pub chirpstack_metric_name: String,
    /// The type of metric
    pub metric_type: MetricTypeConfig,
    /// Unit of the metric
    pub metric_unit: Option<String>,
}

/// Structure for storing configuration loaded by figment
#[derive(Debug, Deserialize, Clone)]
pub struct AppConfig {
    /// Global application configuration
    pub global: Global,
    /// ChirpStack-specific configuration.
    pub chirpstack: ChirpstackPollerConfig,
    /// OPC UA server-specific configuration.
    pub opcua: OpcUaConfig,
    /// List of applications we are we would like to monitor
    #[serde(rename = "application")]
    pub application_list: Vec<ChirpStackApplications>,
}

impl AppConfig {
    /// Creates and initialize a new instance of the application configuration.
    ///
    /// This method loads the configuration from TOML files and environment variables.
    /// It first looks for a default configuration file, then an optional local file,
    /// and finally environment variables prefixed with "APP_".
    ///

    pub fn new() -> Result<Self, OpcGwError> {
        debug!("Creating new AppConfig");

        // Define config file path
        let config_path =
            std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config/default.toml".to_string());

        // Reading the configuration
        trace!("with config path: {}", config_path);
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .merge(Env::prefixed("OPCGW_").global())
            .extract()
            .map_err(|e| OpcGwError::ConfigurationError(format!("Connexion error: {}", e)))?;
        //trace!("config: {:#?}", config);
        Ok({ config })
    }

    /// This function retrieves the application name corresponding
    /// to the given application ID.
    ///
    /// # Arguments
    ///
    /// * `application_id` - A reference to the application ID as a `String`.
    ///
    /// # Returns
    ///
    /// This function returns an `Option<String>`. It returns `Some(String)`
    /// containing the application name if a match is found,
    /// and `None` if no match is found.
    ///
    pub fn get_application_name(&self, application_id: &String) -> Option<String> {
        for app in self.application_list.iter() {
            if app.application_id == *application_id {
                return Some(app.application_name.clone());
            }
        }
        None
    }

    /// This function retrieves the application id corresponding
    /// to the given application name.
    ///
    /// # Arguments
    ///
    /// * `application_name` - A reference to the application name as a `String`.
    ///
    /// # Returns
    ///
    /// This function returns an `Option<String>`. It returns `Some(String)`
    /// containing the application id if a match is found,
    /// and `None` if no match is found.
    ///
    pub fn get_application_id(&self, application_name: &String) -> Option<String> {
        for app in self.application_list.iter() {
            if app.application_name == *application_name {
                return Some(app.application_id.clone());
            }
        }
        None
    }

    /// Returns the name of the device given its `device_id` and `application_id`.
    ///
    /// # Parameters
    /// - `device_id`: A reference to the device's unique identifier as a `String`.
    /// - `application_id`: A reference to the application's unique identifier as a `String`.
    ///
    /// # Returns
    /// - `Some(String)`: The name of the device if found.
    /// - `None`: If the device with the given `device_id`
    ///    under the specified `application_id` is not found.
    ///
    pub fn get_device_name(&self, device_id: &String) -> Option<String> {
        // Search for the application
        for app in self.application_list.iter() {

                // Search for device id
                for device in app.device_list.iter() {
                    if device.device_id == *device_id {
                        return Some(device.device_name.clone());
                    }
                }

        }
        None
    }

    /// Returns the id of the device given its `device_name` and `application_id`.
    ///
    /// If several devices in an application have the same name, the first
    /// is returned. There are no check for duplication.
    ///
    /// # Parameters
    /// - `device_name`: A reference to the device's name as a `String`.
    /// - `application_id`: A reference to the application's unique identifier as a `String`.
    ///
    /// # Returns
    /// - `Some(String)`: The id of the device if found.
    /// - `None`: If the device with the given `device_id`
    ///    under the specified `application_id` is not found.
    ///
    pub fn get_device_id(&self, device_name: &String, application_id: &String) -> Option<String> {
        // Search for the application
        for app in self.application_list.iter() {
            if app.application_id == *application_id {
                // Search for device id
                for device in app.device_list.iter() {
                    if device.device_name == *device_name {
                        return Some(device.device_id.clone());
                    }
                }
            }
        }
        None
    }


    /// Retrieves the metric type configuration based on the ChirpStack metric name.
    ///
    /// This method searches through the application list, then each application's device list,
    /// and within each device, it searches the metric list to find the metric corresponding to the
    /// provided ChirpStack metric name. If found, it returns the metric type configuration.
    ///
    /// # Arguments
    ///
    /// * `chirpstack_metric_name` - A reference to a String containing the name of the ChirpStack metric.
    ///
    /// # Returns
    ///
    /// * `Option<MetricTypeConfig>` - An Option containing the metric
    pub fn get_metric_type(&self, chirpstack_metric_name: &String) -> Option<MetricTypeConfig> {
        self.application_list.iter()
            .flat_map(|app| app.device_list.iter())
            .flat_map(|device| device.metric_list.iter())
            .find(|metric| metric.chirpstack_metric_name == *chirpstack_metric_name)
            .map(|metric| metric.metric_type.clone())
    }
}

/// Test config module
#[cfg(test)]
mod tests {
    use super::*;

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

    /// Test if global application parameters
    /// are loaded
    #[test]
    fn test_application_global_config() {
        let config = get_config();
        assert_eq!(config.global.debug, true);
    }

    /// Test if chirpstack configuration parameters
    /// are loaded
    #[test]
    fn test_chirpstack_config() {
        let config = get_config();
        assert_eq!(config.chirpstack.server_address, "localhost:8080");
        assert_eq!(config.chirpstack.api_token, "test_token");
        assert_eq!(config.chirpstack.tenant_id, "tenant_id");
        assert_eq!(config.chirpstack.polling_frequency, 10);
    }

    /// Test if opc ua configuration parameters
    /// are loaded
    #[test]
    fn test_opcua_config() {
        let config = get_config();
        assert_eq!(config.opcua.config_file, "server.conf");
    }

    /// Test if application list
    /// is loaded
    #[test]
    fn test_application_config() {
        let config = get_config();
        assert!(config.application_list.len() > 0); // We have loaded something
        assert_eq!(
            config
                .get_application_name(&"application_1".to_string())
                .unwrap()
                .to_string(),
            "Application01".to_string()
        );
        assert_eq!(
            config
                .get_application_name(&"application_2".to_string())
                .unwrap()
                .to_string(),
            "Application02".to_string()
        );
        assert_eq!(
            config
                .get_application_id(&"Application02".to_string())
                .unwrap()
                .to_string(),
            "application_2".to_string()
        );
    }

    /// Test devices list
    /// is loaded

    #[test]
    fn test_devices_config() {
        let config = get_config();
        assert!(!config.application_list.is_empty());
        assert!(!config.application_list[0].device_list.is_empty()); // There are devices
        assert_eq!(
            config
                .get_device_name(&"device_1".to_string(), &"application_1".to_string())
                .unwrap()
                .to_string(),
            "Device01".to_string()
        );
        assert_eq!(
            config
                .get_device_id(&"Device01".to_string(), &"application_1".to_string())
                .unwrap()
                .to_string(),
            "device_1".to_string()
        );
    }

}

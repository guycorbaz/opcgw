// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage configuration files
//!
//! Provides configuration file management for opc_ua_chirpstack_gateway
//!

#![allow(unused)] //FIXME: Remove for release

use crate::utils::{OpcGwError, OPCGW_CONFIG_PATH};
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
#[derive(Debug, Deserialize, Clone, PartialEq)]
pub enum OpcMetricTypeConfig {
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
    pub metric_type: OpcMetricTypeConfig,
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
    /// Creates a new instance of `AppConfig` by reading the configuration from a TOML file and environment variables.
    ///
    /// This function performs the following steps:
    /// 1. Retrieves the configuration file path from the `CONFIG_PATH` environment variable, or defaults to "config/default.toml" if not set.
    /// 2. Uses the `Figment` library to read the configuration from the TOML file and merge it with environment variables prefixed with `OPCGW_`.
    /// 3. Extracts the configuration and handles any errors that may occur during this process.
    ///
    /// # Returns
    /// * `Ok(Self)` - if the configuration is successfully read and parsed.
    /// * `Err(OpcGwError)` - if there is an error reading or parsing the configuration.
    ///
    /// # Errors
    /// This function will return an `OpcGwError::ConfigurationError` if there is an error reading the configuration file or merging environment variables.
    pub fn new() -> Result<Self, OpcGwError> {
        debug!("Creating new AppConfig");

        // Define config file path
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| format!("{}/default.toml", OPCGW_CONFIG_PATH).to_string());

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
        debug!("Getting device name");
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

    /// Retrieves the list of metrics for a given device ID.
    ///
    /// This function iterates through the list of applications and their corresponding devices.
    /// If a matching device ID is found, the list of metrics for that device is returned.
    /// If no matching device ID is found, it returns `None`.
    ///
    /// # Arguments
    ///
    /// * `self` - A reference to the current struct instance.
    /// * `device_id` - A reference to the device ID for which the metrics list is required.
    ///
    /// # Returns
    ///
    /// * `Option<Vec<Metric>>` - Returns `Some(Vec<Metric>)` if a matching device ID is found,
    /// otherwise returns `None`.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let metrics = instance.get_metric_list(&device_id);
    /// match metrics {
    ///     Some(metrics_list) => println!("Found metrics: {:?}", metrics_list),
    ///     None => println!("No metrics found for the given device ID"),
    /// }
    /// ```
    pub fn get_metric_list(&self, device_id: &String) -> Option<Vec<Metric>> {
        debug!("Getting metric list");
        // Search in applications
        for app in self.application_list.iter() {
            // Search for device
            for device in app.device_list.iter() {
                if device.device_id == *device_id {
                    return Some(device.metric_list.clone());
                }
            }
        }
        None
    }

    /// Retrieves the `OpcMetricTypeConfig` associated with a given ChirpStack metric name for a specified device.
    ///
    /// # Arguments
    ///
    /// * `chirpstack_metric_name` - A reference to a `String` representing the name of the ChirpStack metric.
    /// * `device_id` - A reference to a `String` representing the unique identifier of the device.
    ///
    /// # Returns
    ///
    /// * `Option<OpcMetricTypeConfig>` - Returns `Some(OpcMetricTypeConfig)` if the metric type is found for the given
    ///   ChirpStack metric name and device, otherwise returns `None`.
    ///
    /// # Example
    ///
    /// ```rust
    /// let chirpstack_metric_name = String::from("some_metric_name");
    /// let device_id = String::from("device123");
    /// if let Some(metric_type) = get_metric_type(&chirpstack_metric_name, &device_id) {
    ///     println!("Metric type found: {:?}", metric_type);
    /// } else {
    ///     println!("Metric type not found.");
    /// }
    /// ```
    pub fn get_metric_type(
        &self,
        chirpstack_metric_name: &String,
        device_id: &String,
    ) -> Option<OpcMetricTypeConfig> {
        debug!("Getting metric type");
        let metric_list = match self.get_metric_list(device_id) {
            Some(metrics) => metrics,
            None => return None,
        };
        trace!("metric list: {:?}", metric_list);
        for metric in metric_list.iter() {
            if metric.chirpstack_metric_name == *chirpstack_metric_name {
                return Some(metric.metric_type.clone());
            }
        }
        None
    }
}

/// Test config module
#[cfg(test)]
mod tests {
    use super::*;
    use opcua::types::process_decode_io_result;

    /// Loads the application configuration from a TOML file.
    ///
    /// The configuration file path is determined by the `CONFIG_PATH` environment variable.
    /// If this environment variable is not set, it defaults to "tests/config/default.toml".
    ///
    /// # Returns
    ///
    /// * `AppConfig` - The application configuration extracted from the TOML file.
    ///
    /// # Panics
    ///
    /// This function will panic if the configuration file cannot be loaded or parsed.
    fn get_config() -> AppConfig {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");
        config
    }

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

    #[test]
    fn test_get_metric_list() {
        let config = get_config();
        let device_id = String::from("device_1");
        let no_device_id = String::from("no_device");
        let metric_list = config.get_metric_list(&device_id);
        let no_metric_list = config.get_metric_list(&no_device_id);
        println!("metric list: {:?}", metric_list);
        assert!(metric_list.is_some());
        assert!(metric_list.unwrap().len() == 2);
    }
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
    /// This test verifies that the global configuration for the application
    /// is correctly set to enable debug mode.
    #[test]
    fn test_application_global_config() {
        let config = get_config();
        assert_eq!(config.global.debug, true);
    }

    /// Tests ChirpStack configuration to ensure default values are correctly set.
    ///
    /// This test retrieves the configuration by calling `get_config()` and verifies that:
    /// - The server address is "localhost:8080"
    /// - The API token is "test_token"
    /// - The tenant ID is "tenant_id"
    /// - The polling frequency is 10 seconds
    #[test]
    fn test_chirpstack_config() {
        let config = get_config();
        assert_eq!(config.chirpstack.server_address, "localhost:8080");
        assert_eq!(config.chirpstack.api_token, "test_token");
        assert_eq!(config.chirpstack.tenant_id, "tenant_id");
        assert_eq!(config.chirpstack.polling_frequency, 10);
    }

    /// This test verifies that the OPC UA configuration file is correctly set.
    ///
    /// The test retrieves the current configuration using the `get_config()` function.
    /// It then asserts that the `opcua.config_file` field in the configuration
    /// is equal to the expected value "server.conf".
    ///
    /// # Example
    /// ```
    /// let config = get_config();
    /// assert_eq!(config.opcua.config_file, "server.conf");
    /// ```
    #[test]
    fn test_opcua_config() {
        let config = get_config();
        assert_eq!(config.opcua.config_file, "server.conf");
    }

    /// This test ensures the integrity of the application configuration.
    /// The test performs the following checks:
    /// 1. Verifies that the configuration loads at least one application.
    /// 2. Checks if the application name retrieved for 'application_1' matches "Application01".
    /// 3. Checks if the application name retrieved for 'application_2' matches "Application02".
    /// 4. Verifies that the application ID for "Application02" is correctly retrieved as 'application_2'.
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

        assert_eq!(
            config.get_application_id(&"noapplication".to_string()),
            None
        );
    }

    /// This test ensures that the device configuration is loaded correctly.
    /// It checks that:
    /// 1. The application list is not empty.
    /// 2. The first application in the list has a non-empty device list.
    /// 3. The device name for a device with ID "device_1" matches "Device01".
    /// 4. The device ID for a device named "Device01" in the application "application_1" matches "device_1".
    #[test]
    fn test_devices_config() {
        let config = get_config();
        assert!(!config.application_list.is_empty());
        assert!(!config.application_list[0].device_list.is_empty()); // There are devices
        assert_eq!(
            config
                .get_device_name(&"device_1".to_string())
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

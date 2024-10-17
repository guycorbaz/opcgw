// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage configuration files
//!
//! Provides configuration file management for opc_ua_chirpstack_gateway
//!
//! # Example:
//! Add example code...

use crate::utils::OpcGwError;
//use crate::utils::OpcGwError::ConfigurationError;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use log::{debug, trace};
use serde::Deserialize;
use std::collections::HashMap;

/// Global configuration for the application
#[derive(Debug, Deserialize, Clone)]
pub struct Global {
    /// set to true for detailed debug log
    pub debug: bool,
}
/// Configuration for the ChirpStack connection.
#[derive(Debug, Deserialize, Clone)]
pub struct ChirpstackConfig {
    /// ChirpStack server address.
    pub server_address: String,
    /// API token for authentication with ChirpStack.
    pub api_token: String,
    /// Tenant ID we are working with.
    pub tenant_id: String,
    /// Server polling frequency
    pub polling_frequency: u64,
}

/// Configuration for the OPC UA server.
/// For the time being, the configuration is
/// coming from a specific file
#[derive(Debug, Deserialize, Clone)]
pub struct OpcUaConfig {
    /// Config file path for opc ua server
    pub config_file: String,
}

/// CHirpstack application description
/// These informations are definbes in
/// the Chirpstack server
#[derive(Debug, Deserialize, Clone)]
pub struct ChirpStackApplications {
    /// Chirpstack application name
    pub name: String,
    /// Chirpstack application ID
    id: String,
}

/// Chirpstack to opc ua application configuration
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    /// Global application configuration
    pub global: Global,
    /// ChirpStack-specific configuration.
    pub chirpstack: ChirpstackConfig,
    /// OPC UA server-specific configuration.
    pub opcua: OpcUaConfig,
    /// List of applications we are interested in
    pub applications: HashMap<String, String>, // First field is name, second, id
    /// List of devices we are interested in
    pub devices: HashMap<String, Device>,      // First field is name, second, id

}


#[derive(Debug, Deserialize, Clone)]
pub struct Device {
    pub device_id: String,
    pub application_id: String,
}

impl Config {
    /// Creates a new instance of the application configuration.
    ///
    /// This method loads the configuration from TOML files and environment variables.
    /// It first looks for a default configuration file, then an optional local file,
    /// and finally environment variables prefixed with "APP_".
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing either the loaded configuration or a configuration error.
    pub fn new() -> Result<Self, OpcGwError> {
        debug!("Creating new AppConfig");

        // Define config file path TODO: Add the possibility to pass it via command line parameter
        let config_path =
            std::env::var("CONFIG_PATH")
                .unwrap_or_else(|_| "config/default.toml".to_string());

        // Reading the configuration from 'config_path'
        trace!("with config path: {}", config_path);
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .merge(Env::prefixed("OPCGW_").global())
            .extract()
            .map_err(|e| OpcGwError::ConfigurationError(format!("Connexion error: {}", e)))?;

        Ok({
            config
        })
    }
}

///Test load_config
/// This function does NOT test applications neither devices
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_application_global_config() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");

        assert_eq!(config.global.debug, true);
    }

    #[test]
    fn test_chirpstack_config() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");

        assert_eq!(config.chirpstack.server_address, "localhost:8080");
        assert_eq!(config.chirpstack.api_token, "test_token");
        assert_eq!(config.chirpstack.tenant_id, "tenant_id");
        assert_eq!(config.chirpstack.polling_frequency, 10);
    }

    #[test]
    fn test_opcua_config() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");

        assert_eq!(config.opcua.config_file, "server.conf");
    }

    #[test]
    fn test_application_config() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");

        assert!(config.applications.len() > 0);
        assert_eq!(config.applications.get("application_1").unwrap(), "Application01");
    }

    #[test]
    fn test_devices_config() {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");

        assert!(config.devices.len() > 0);
        assert_eq!(config.devices.get("device_1").unwrap().device_id, "Device01");
        assert_eq!(config.devices.get("device_1").unwrap().application_id, "Application01");
    }

}

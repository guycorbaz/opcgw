// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage configuration files
//!
//! Provides configuration file management for opc_ua_chirpstack_gateway
//!
//! # Example:
//! Add example code...

use serde::Deserialize;
use figment::{Figment, providers::{Format, Toml, Env}};
use log::{debug, error, info, warn, trace};
use std::collections::HashMap;
use crate::utils::OpcGwError;
use crate::utils::OpcGwError::ConfigurationError;
use crate::opc_ua::OpcUa;

/// General configuration for the application
#[derive(Debug, Deserialize, Clone)]
pub struct Application {
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
}

/// Configuration for the OPC UA server.
#[derive(Debug, Deserialize, Clone)]
pub struct OpcUaConfig {
    pub config_file: String,
    /// URL of the OPC UA server.
    pub server_url: String,
    pub policy: String,
    pub mode: String,
    pub uri: String,
    /// Name of the OPC UA server.
    pub server_name: String,
    pub system_type: String,
    pub discovery_urls: String, //TODO: change it to a vector to pass several URLs
    pub cert_file: String,
    pub private_key_file: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct ChirpStackApplications {
    pub name: String,
    id: String,
}

/// Global application configuration.
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub application: Application,
    /// ChirpStack-specific configuration.
    pub chirpstack: ChirpstackConfig,
    /// OPC UA server-specific configuration.
    pub opcua: OpcUaConfig,
    pub applications: HashMap<String, String>,  // Firs field is name, second, id
    pub devices: HashMap<String, String>,       // firs field is name, second, id
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
        let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config/default.toml".to_string());
        trace!("with config path: {}", config_path);
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .merge(Env::prefixed("OPCGW_").global())
            .extract()
            .map_err(|e| OpcGwError::ConfigurationError(format!("Connexion error: {}", e)))?;

        Ok({
            //trace!{"Configuration: {:#?}", config,}
            config})
    }
}

///Test load_config
/// This function does NOT test applications neither devices
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_config() {

        let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "tests/config/default.toml".to_string());
        let config: Config = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load configuration");

        assert_eq!(config.application.debug, true);
        assert_eq!(config.chirpstack.server_address, "localhost:8080");
        assert_eq!(config.chirpstack.api_token, "test_token");
        assert_eq!(config.chirpstack.tenant_id, "tenant_id");
        assert_eq!(config.opcua.server_url, "opc.tcp://localhost:4840");
        assert_eq!(config.opcua.server_name, "ChirpStack OPC UA Server");
    }
}

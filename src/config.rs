//! Configuration module for the ChirpStack to OPC UA Gateway application.
//!
//! This module handles loading and structuring the application configuration
//! from TOML files and environment variables.

use config::{Config, ConfigError, Environment, File};
use log::{debug, error, info, warn};
use serde::Deserialize;

/// Configuration for the ChirpStack connection.
#[derive(Debug, Deserialize)]
pub struct ChirpstackConfig {
    /// ChirpStack server address.
    pub server_address: String,
    /// API token for authentication with ChirpStack.
    pub api_token: String,
    /// Tenant ID we are working with.
    pub tenant_id: String,
}

/// Configuration for the OPC UA server.
#[derive(Debug, Deserialize)]
pub struct OpcUaConfig {
    /// URL of the OPC UA server.
    pub server_url: String,
    /// Name of the OPC UA server.
    pub server_name: String,
}

/// Global application configuration.
#[derive(Debug, Deserialize)]
pub struct AppConfig {
    /// ChirpStack-specific configuration.
    pub chirpstack: ChirpstackConfig,
    /// OPC UA server-specific configuration.
    pub opcua: OpcUaConfig,
}

impl AppConfig {
    /// Creates a new instance of the application configuration.
    ///
    /// This method loads the configuration from TOML files and environment variables.
    /// It first looks for a default configuration file, then an optional local file,
    /// and finally environment variables prefixed with "APP_".
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing either the loaded configuration or a configuration error.
    pub fn new() -> Result<Self, ConfigError> {
        debug!("Creating new AppConfig");
        let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config".to_string());

        let s = Config::builder()
            .add_source(File::with_name(&format!("{}/default", config_path)))
            .add_source(File::with_name(&format!("{}/local", config_path)).required(false))
            .add_source(Environment::with_prefix("APP"))
            .build()?;

        s.try_deserialize()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    #[test]
    fn test_load_config() {
        env::set_var("CONFIG_PATH", "tests/config");
        let config = AppConfig::new().expect("Failed to load configuration");

        assert_eq!(config.chirpstack.server_address, "localhost:8080");
        assert_eq!(config.chirpstack.api_token, "test_token");
        assert_eq!(config.chirpstack.tenant_id, "tenant_id");
        assert_eq!(config.opcua.server_url, "opc.tcp://localhost:4840");
        assert_eq!(config.opcua.server_name, "ChirpStack OPC UA Server");
    }
}

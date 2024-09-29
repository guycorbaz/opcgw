use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use std::path::Path;

#[derive(Debug, Deserialize)]
pub struct ChirpstackConfig {
    pub server_address: String,
    pub api_token: String,
}

#[derive(Debug, Deserialize)]
pub struct OpcUaConfig {
    pub server_url: String,
    pub server_name: String,
}

#[derive(Debug, Deserialize)]
pub struct AppConfig {
    pub chirpstack: ChirpstackConfig,
    pub opcua: OpcUaConfig,
}

impl AppConfig {
    pub fn new() -> Result<Self, ConfigError> {
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
        assert_eq!(config.opcua.server_url, "opc.tcp://localhost:4840");
        assert_eq!(config.opcua.server_name, "ChirpStack OPC UA Server");
    }
}

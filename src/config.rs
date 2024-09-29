//! Module de configuration pour l'application ChirpStack to OPC UA Gateway.
//! 
//! Ce module gère le chargement et la structure de la configuration de l'application
//! à partir de fichiers TOML et de variables d'environnement.

use config::{Config, ConfigError, Environment, File};
use serde::Deserialize;
use log::{info, warn, error, debug};

/// Configuration pour la connexion à ChirpStack.
#[derive(Debug, Deserialize)]
pub struct ChirpstackConfig {
    /// Adresse du serveur ChirpStack.
    pub server_address: String,
    /// Token API pour l'authentification auprès de ChirpStack.
    pub api_token: String,
}

/// Configuration pour le serveur OPC UA.
#[derive(Debug, Deserialize)]
pub struct OpcUaConfig {
    /// URL du serveur OPC UA.
    pub server_url: String,
    /// Nom du serveur OPC UA.
    pub server_name: String,
}

/// Configuration globale de l'application.
#[derive(Debug, Deserialize)]
pub struct AppConfig {
    /// Configuration spécifique à ChirpStack.
    pub chirpstack: ChirpstackConfig,
    /// Configuration spécifique au serveur OPC UA.
    pub opcua: OpcUaConfig,
}

impl AppConfig {
    /// Crée une nouvelle instance de la configuration de l'application.
    ///
    /// Cette méthode charge la configuration à partir de fichiers TOML et de variables d'environnement.
    /// Elle recherche d'abord un fichier de configuration par défaut, puis un fichier local optionnel,
    /// et enfin des variables d'environnement préfixées par "APP_".
    ///
    /// # Retours
    ///
    /// Retourne un `Result` contenant soit la configuration chargée, soit une erreur de configuration.
    pub fn new() -> Result<Self, ConfigError> {
        debug!("new");
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

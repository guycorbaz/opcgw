//! Module pour les fonctions et structures utilitaires.
//! 
//! Ce module contient des fonctions et des structures utilitaires communes à l'application.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Erreur de configuration: {0}")]
    ConfigError(String),
    #[error("Erreur ChirpStack: {0}")]
    ChirpStackError(String),
    #[error("Erreur OPC UA: {0}")]
    OpcUaError(String),
    #[error("Erreur de stockage: {0}")]
    StorageError(String),
}

pub fn setup_logger() -> Result<(), log::SetLoggerError> {
    // Implémentez ici la configuration du logger
    Ok(())
}

// Ajoutez ici d'autres fonctions utilitaires selon les besoins

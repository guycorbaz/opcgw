//! Module pour les fonctions et structures utilitaires.
//! 
//! Ce module contient des fonctions et des structures utilitaires communes à l'application.

use thiserror::Error;
use log::LevelFilter;
use log4rs::{
    append::file::FileAppender,
    config::{Appender, Config, Root},
    encode::pattern::PatternEncoder,
};

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



// Ajoutez ici d'autres fonctions utilitaires selon les besoins

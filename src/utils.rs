//! Module pour les fonctions et structures utilitaires.
//!
//! Ce module contient des fonctions et des structures utilitaires communes à l'application.

use log::LevelFilter;
use log4rs::{
    append::file::FileAppender,
    config::{Appender, Config, Root},
    encode::pattern::PatternEncoder,
};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpcGwError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
    #[error("ChirpStack error: {0}")]
    ChirpStackError(String),
    #[error("OPC UA error: {0}")]
    OpcUaError(String),
    #[error("Stockage error: {0}")]
    StorageError(String),
}

// Ajoutez ici d'autres fonctions utilitaires selon les besoins

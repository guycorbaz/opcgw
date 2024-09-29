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

pub fn setup_logger() -> Result<(), log4rs::Error> {
    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new("{d} - {l} - {m}\n")))
        .build("log/output.log")?;

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(Root::builder().appender("logfile").build(LevelFilter::Info))?;

    log4rs::init_config(config)?;
    Ok(())
}

// Ajoutez ici d'autres fonctions utilitaires selon les besoins

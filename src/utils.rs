// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Global utilities for the program
//!
//!
//!
//! # Example:
//! Add example code...

use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpcGwError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
    #[error("ChirpStack error: {0}")]
    ChirpStackError(String),
    #[error("OPC UA error: {0}")]
    OpcUaError(String),
    //#[error("Stockage error: {0}")]
    //StorageError(String),
}

// Ajoutez ici d'autres fonctions utilitaires selon les besoins

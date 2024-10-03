//! Module pour le serveur OPC UA.
//!
//! Ce module gère la création et le fonctionnement du serveur OPC UA.

use crate::config::OpcUaConfig;
use log::{debug, error, info, warn};
use opcua::server::prelude::*;
use std::sync::Arc;
use std::path::Path;
use thiserror::Error;

// Définir les erreurs spécifiques pour OpcUaServer
#[derive(Debug, Error)]
pub enum OpcUaServerError {
    #[error("Failed to load server configuration: {0}")]
    ConfigLoadError(String),
    #[error("General server error: {0}")]
    General(String),
}

pub struct OpcUaServer {
    config: OpcUaConfig,
    server: Option<Server>,
}

impl OpcUaServer {
    pub fn new(config: OpcUaConfig) -> Self {
        debug!("Creating new OPC UA server");
        OpcUaServer {
            config,
            server: None,
        }
    }

    // TODO: Redesign this method
    //pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
    //    debug!("Starting OPC UA server");
        

    //    Ok(())
    //}

    //pub fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
    //    debug!("Stopping OPC UA server");
    //    if let Some(server) = self.server.take() {
    //        server.stop();
    //        info!("OPC UA server stopped successfully");
    //    } else {
    //        warn!("OPC UA server was not running");
    //    }
    //    Ok(())
    //}

    // Ajoutez ici d'autres méthodes pour gérer le serveur OPC UA
}

//! Module pour le serveur OPC UA.
//!
//! Ce module gère la création et le fonctionnement du serveur OPC UA.

use crate::config::OpcUaConfig;
use log::{debug, error, info, warn};
use opcua::server::prelude::*;
use std::sync::Arc;

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

    pub fn start(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Starting OPC UA server");
        
        let server_builder = ServerBuilder::new();
        let server_config = ServerConfig::load(&self.config.server_url)?;
        
        let mut server = server_builder
            .application_name(self.config.server_name.clone())
            .application_uri(self.config.server_url.clone())
            .product_uri("urn:chirpstack_opcua_gateway")
            .create_sample_keypair(true)
            .config(server_config)
            .build()?;

        // Créer l'espace d'adressage
        let ns = {
            let address_space = server.address_space();
            let mut address_space = address_space.write();
            address_space.register_namespace("urn:chirpstack_opcua_gateway")?
        };

        // Ajouter un dossier racine pour les données ChirpStack
        let chirpstack_folder = address_space.add_folder("ChirpStack", "ChirpStack Data", &NodeId::objects_folder_id())?;

        // Ici, vous pouvez ajouter d'autres nœuds pour représenter les données ChirpStack

        self.server = Some(server);

        // Démarrer le serveur
        let server = Arc::clone(self.server.as_ref().unwrap());
        std::thread::spawn(move || {
            server.run();
        });

        info!("OPC UA server started successfully");
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        debug!("Stopping OPC UA server");
        if let Some(server) = self.server.take() {
            server.stop();
            info!("OPC UA server stopped successfully");
        } else {
            warn!("OPC UA server was not running");
        }
        Ok(())
    }

    // Ajoutez ici d'autres méthodes pour gérer le serveur OPC UA
}

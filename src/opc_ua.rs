//! Module pour le serveur OPC UA.
//!
//! Ce module gère la création et le fonctionnement du serveur OPC UA.

use std::{path::PathBuf, sync::Arc};
use opcua::server::prelude::*;
use tokio::signal;
use log::{debug, error, info, warn, trace};
use tokio::io::AsyncWriteExt;
use crate::config::OpcUaConfig;
use opcua::sync::RwLock;

#[derive(Debug)]
pub struct OpcUa {
    config: OpcUaConfig,
    server_config: ServerConfig,
}

impl OpcUa {

    pub fn new(opc_ua_config: OpcUaConfig) -> Self {
        trace!("New OPC UA server");

        OpcUa {
            server_config: OpcUa::create_server_config( &opc_ua_config),
            config: opc_ua_config
        }
    }

    /// Start opc ua server
    pub async fn start_server(&self, runtime: &tokio::runtime::Runtime) {
        trace!("Starting OPC UA server");
        let mut server = Server::new(self.server_config.clone());
        trace!("Created OPC UA server");

        Server::run_server_on_runtime(
            runtime.clone(),
            Server::new_server_task(Arc::new(RwLock::new(server))),
            true,
        );
    }

    fn create_server_config(opc_ua_cfg: &OpcUaConfig) -> ServerConfig {
        trace!("Creating OpcUaConfig with config in: {:#?}",opc_ua_cfg.config_file);
        let mut config_path = PathBuf::from(&opc_ua_cfg.config_file);
        let server_config = ServerConfig::load(&mut config_path)
            .expect("Failed to load server config");  //TODO: Improve error handling
        server_config
    }


}



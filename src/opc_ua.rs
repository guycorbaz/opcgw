
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage opc ua server
//!
//!
//!
//! # Example:
//! Add example code...

use crate::config::OpcUaConfig;
use crate::utils::OpcGwError;
use log::{debug, error, info, trace, warn};
use opcua::server::prelude::*;
use opcua::sync::Mutex;
use opcua::sync::RwLock;
use opcua::types::VariableId::OperationLimitsType_MaxNodesPerTranslateBrowsePathsToNodeIds;
use std::option::Option;
use std::path::PathBuf;
use std::sync::Arc;

pub struct OpcUa {
    pub opc_ua_config: OpcUaConfig,
    pub server_config: ServerConfig,
    pub server: Arc<RwLock<Server>>,
    pub ns: u16,
}

impl OpcUa {
    pub fn new(opc_ua_config: &OpcUaConfig) -> Self {
        trace!("New OPC UA structure");
        let server_config = Self::create_server_config(&opc_ua_config.config_file.clone());
        let server = Arc::new(RwLock::new(Self::create_server(server_config.clone())));

        OpcUa {
            opc_ua_config: opc_ua_config.clone(),
            server_config,
            server,
            ns: 0,
        }
    }

    fn create_server_config(config_file_name: &String) -> ServerConfig {
        debug!("Creating server config");
        match ServerConfig::load(&PathBuf::from(config_file_name)) {
            Ok(config) => config,
            Err(e) => panic!(
                "{}",
                OpcGwError::OpcUaError(format!("Can not create server config {:?}", e))
            ),
        }
    }

    fn create_server(server_config: ServerConfig) -> Server {
        debug!("Creating server");
        Server::new(server_config.clone())
    }

    pub async fn run(&self) -> Result<(), OpcGwError> {
        debug!("Running OPC UA server");

        let server_task = Server::new_server_task(self.server.clone());

        // Run the server indefinitely
        server_task.await;

        Ok(())
    }
}

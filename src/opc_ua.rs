//! Module pour le serveur OPC UA.
//!
//! Ce module gère la création et le fonctionnement du serveur OPC UA.

use std::{path::PathBuf, sync::Arc};
use opcua::sync::Mutex;
use opcua::server::prelude::*;
use log::{debug, error, info, warn, trace};
use crate::config::OpcUaConfig;
use opcua::sync::RwLock;
use crate::utils::OpcGwError;


pub struct OpcUa {
    pub server_config: ServerConfig,
    pub server: Server,
    pub ns: u16,
}




impl OpcUa {

    pub fn config(&self) -> &ServerConfig {
        &self.server_config
    }

    pub fn add_folder(&self) {
       let address_space = self.server.address_space();
        let mut address_space = address_space.write();
        //let folder_id = address_space
        //        .add_folder("GW_folder", "GW_folder", &NodeId::objects_folder_id())
        //        .unwrap();
        if let Ok(folder_id) = address_space.add_folder("Gw_folder", "Gw_folder",
            &NodeId::objects_folder_id()) {
            let v1_node = NodeId::new(self.ns, "v1");
            let v2_node = NodeId::new(self.ns, "v2");
            let v3_node = NodeId::new(self.ns, "v3");
            let v4_node = NodeId::new(self.ns, "v4");

            {
                let _ = address_space.add_variables(
                    vec![
                        Variable::new(&v1_node, "v1", "v1", 0_i32),
                        Variable::new(&v2_node, "v2", "v2", false),
                        Variable::new(&v3_node, "v3", "v3", UAString::from("")),
                        Variable::new(&v4_node, "v4", "v4", 0f64),
                    ],
                    &folder_id,
                );
            }
        } else {
            error!("Failed to add folder to address_space");
        }
    }

    pub fn new(opc_ua_config: OpcUaConfig) -> Self {
        trace!("New OPC UA structure");

        let server_config =OpcUa::create_server_config( &opc_ua_config);
        let server = Server::new(server_config.clone());
        let address_space = server.address_space();
        let mut address_space = address_space.write();
        let ns = address_space
            .register_namespace("urn:opc-ua-gateway")
            .unwrap(); // TODO: improve error management

            OpcUa {
                server_config: server_config,
                server: server,
                ns: ns,
            }

    }


    fn create_server_config(opc_ua_cfg: &OpcUaConfig) -> ServerConfig {
        trace!("Creating OpcUaConfig with config in: {:#?}",opc_ua_cfg.config_file);
        let mut config_path = PathBuf::from(&opc_ua_cfg.config_file);
        let server_config = ServerConfig::load(&mut config_path)
            .expect("Failed to load server config");  //TODO: Improve error handling
        server_config
    }


}



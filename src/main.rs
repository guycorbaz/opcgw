mod chirpstack;
mod config;

mod chirpstack_test;
mod opc_ua;
mod storage;
mod utils;

// Inclure le module généré
pub mod chirpstack_api {
    //tonic::include_proto!("chirpstack");
}

use crate::chirpstack_test::test_chirpstack;
use chirpstack::ChirpstackClient;
use config::Config;
use log::{debug, error, info, trace, warn};
use opcua::server::server::Server;
use opc_ua::OpcUa;
use storage::Storage;
use tokio::runtime::{Builder, Runtime};
use std::{path::PathBuf, sync::Arc};
use opcua::sync::RwLock;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure logger
    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("starting up");

    trace!("Create configuration:");
    let config = Config::new()?;

    trace!("Create chirpstack server");
    let chirpstack_server = ChirpstackClient::new(config.chirpstack);

    trace!("Creating opc ua server");
    let opc_ua_server_config = OpcUa::new(config.opcua);
    let mut opc_ua_server = Server::new(opc_ua_server_config.server_config);

    // Add opc ua structure

    // Create the runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");

    trace!("Run OPC UA server");
    Server::run_server_on_runtime(
        runtime,
        Server::new_server_task(Arc::new(RwLock::new(opc_ua_server))),
        true
    );

        Ok(())

}

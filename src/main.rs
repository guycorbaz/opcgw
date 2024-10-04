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
use crate::chirpstack::{ApplicationDetail, ChirpstackClient, DeviceDetails, DeviceListDetail};
use crate::chirpstack_test::test_chirpstack;
use opcua::sync::RwLock;
use config::Config;
use log::{debug, error, info, trace, warn};
use opcua::server::server::Server;
use opc_ua::OpcUa;
use storage::Storage;
use tokio::runtime::{Builder, Runtime};
//use tokio::sync::RwLock;
use std::{path::PathBuf, sync::Arc};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure logger
    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("starting up opcgw");

    trace!("Create application configuration:");
    let application_config = Config::new()?;

    trace!("Create chirpstack client");
    //let chirpstack_client = ChirpstackClient::new(&application_config.chirpstack).await.expect("Failed to create chirpstack client");
    //let applications_list = chirpstack_client.list_applications().await?;
    //let devices_list = chirpstack_client.list_devices("ae2012c2-75a1-407d-98ab-1520fb511edf".to_string()).await?;

    trace!("Create storage");
    let mut storage:Storage = Storage::new(&application_config).await;
    storage.load_applications();
    storage.load_devices().await;
    storage.list_devices();



    //trace!("Creating opc ua server");
    //let opc_ua = OpcUa::new(config.opcua);
    //opc_ua.add_folder();


    // Add opc ua structure



    //trace!("Run OPC UA server");
    //Server::run_server_on_runtime(
    //    runtime,
    //    Server::new_server_task(Arc::new(RwLock::new(opc_ua.server))),
    //    true
    //);
    info!("Stopping application");
        Ok(())

}

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
use log::{debug, error, info, warn};
use opc_ua::OpcUaServer;
use storage::Storage;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure logger
    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("Starting opc ua chirpstack gateway");

    // Charger la configuration
    //debug!("Load configuration");
    let config = Config::new().expect("Failed to load configuration");

    //info!("OPC UA server: {}", config.opcua.server_url); TODO: uncoment
    //info!("OPC UA server name: {}", config.opcua.server_name); TODO: uncoment

    // Initialize components
    let mut chirpstack_client = ChirpstackClient::new(config.chirpstack).await?;
    //test_chirpstack(&mut chirpstack_client).await; //TODO: Remove: for testing only

    //chirpstack::print_list(&applications); //TODO: remove: for debugging purpose
    //let opc_ua_server = OpcUaServer::new(config.opcua); TODO: uncoment
    //let (storage, command_receiver) = Storage::new(); TODO: uncoment

    // Start OPC UA server
    //debug!("Start OPC UA server");
    //opc_ua_server.start()?; TODO:uncoment

    // Ici, nous ajouterons la logique principale de l'application
    // Par exemple, une boucle pour traiter les commandes et les données

    Ok(())
}

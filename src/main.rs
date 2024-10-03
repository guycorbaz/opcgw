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
use opc_ua::OpcUa;
use storage::Storage;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure logger
    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("Starting opc ua chirpstack gateway");

    // Create the runtime
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("Failed to create Tokio runtime");

    // Run the async main function
    runtime.block_on(async {
        // Charger la configuration
        let config = Config::new().expect("Failed to load configuration");

        // Initialize components
        let mut chirpstack_client = ChirpstackClient::new(config.chirpstack).await?;
        let opc_ua_server = OpcUa::new(config.opcua);

        // Start OPC UA server
        opc_ua_server.start_server(&runtime).await;

        // Ici, nous ajouterons la logique principale de l'application
        // Par exemple, une boucle pour traiter les commandes et les données

        Ok(())
    })
}

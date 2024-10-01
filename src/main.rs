mod config;
mod chirpstack;
mod opc_ua;
mod storage;
mod utils;

// Inclure le module généré
pub mod chirpstack_api {
    //tonic::include_proto!("chirpstack");
}

use config::AppConfig;
use chirpstack::ChirpstackClient;
use opc_ua::OpcUaServer;
use storage::Storage;
use log::{info, warn, error, debug};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configurer le logger
    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("Starting opc ua chirpstack gateway");

    // Charger la configuration
    //debug!("Load configuration");
    let config = AppConfig::new().expect("Failed to load configuration");
    
    info!("ChirpStack to OPC UA Gateway");
    info!("ChirpStack server: {}", config.chirpstack.server_address);
    //info!("OPC UA server: {}", config.opcua.server_url); TODO: uncoment
    //info!("OPC UA server name: {}", config.opcua.server_name); TODO: uncoment

    // Initialiser les composants
    let chirpstack_client = ChirpstackClient::new(config.chirpstack).await?;

    // Get the list of applications TODO: remove after testing
    match chirpstack_client.list_applications("52f14cd4-c6f1-4fbd-8f87-4025e1d49242".to_string()).await {
        Ok(applications) => {
            debug!("Print list of applications");
            chirpstack::print_app_list(&applications)
        },
        Err(e) => error!("Error when collecting applications: {}",e),
    }

    // Get the list of devices TODO: remove after testing
    match chirpstack_client.list_devices("ae2012c2-75a1-407d-98ab-1520fb511edf".to_string()).await {
        Ok(devices) => {
            debug!("Print list of devices");
            chirpstack::print_dev_list(&devices);
        },
        Err(e) => error!("Error when collecting devices: {}",e),
    }
    
    
    //chirpstack::print_list(&applications); //TODO: remove: for debugging purpose
    //let opc_ua_server = OpcUaServer::new(config.opcua); TODO: uncoment
    //let (storage, command_receiver) = Storage::new(); TODO: uncoment

    // Démarrer le serveur OPC UA
    //debug!("Start OPC UA server");
    //opc_ua_server.start()?; TODO:uncoment

    // Ici, nous ajouterons la logique principale de l'application
    // Par exemple, une boucle pour traiter les commandes et les données

    Ok(())
}

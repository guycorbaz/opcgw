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
use utils::setup_logger;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configurer le logger
    setup_logger()?;

    // Charger la configuration
    let config = AppConfig::new().expect("Failed to load configuration");
    
    println!("ChirpStack to OPC UA Gateway");
    println!("ChirpStack server: {}", config.chirpstack.server_address);
    println!("OPC UA server: {}", config.opcua.server_url);

    // Initialiser les composants
    let chirpstack_client = ChirpstackClient::new(config.chirpstack).await?;
    let opc_ua_server = OpcUaServer::new(config.opcua);
    let (storage, command_receiver) = Storage::new();

    // Démarrer le serveur OPC UA
    opc_ua_server.start()?;

    // Ici, nous ajouterons la logique principale de l'application
    // Par exemple, une boucle pour traiter les commandes et les données

    Ok(())
}

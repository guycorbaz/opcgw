mod config;
mod chirpstack;
mod opc_ua;
mod storage;
mod utils;

use config::AppConfig;

fn main() {
    // Charger la configuration
    let config = AppConfig::new().expect("Failed to load configuration");
    
    println!("ChirpStack to OPC UA Gateway");
    println!("ChirpStack server: {}", config.chirpstack.server_address);
    println!("OPC UA server: {}", config.opcua.server_url);
}

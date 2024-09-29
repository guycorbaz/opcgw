//! Module pour la communication avec ChirpStack.
//! 
//! Ce module gère la connexion et l'interaction avec le serveur ChirpStack.

use tonic::{transport::Channel, Request};
use crate::config::ChirpstackConfig;
use crate::utils::AppError;

// Importez ici les types générés par tonic
// Par exemple :
// use chirpstack::api::device::DeviceServiceClient;

pub struct ChirpstackClient {
    config: ChirpstackConfig,
    // client: DeviceServiceClient<Channel>,
}

impl ChirpstackClient {
    pub async fn new(config: ChirpstackConfig) -> Result<Self, AppError> {
        // Créez une connexion au serveur ChirpStack
        let channel = Channel::from_shared(config.server_address.clone())
            .unwrap()
            .connect()
            .await
            .map_err(|e| AppError::ChirpStackError(format!("Erreur de connexion: {}", e)))?;

        // Créez le client gRPC
        // let client = DeviceServiceClient::new(channel);

        Ok(ChirpstackClient { 
            config,
            // client,
        })
    }

    // Exemple de méthode pour interagir avec ChirpStack
    // pub async fn get_device(&self, dev_eui: &str) -> Result<Device, AppError> {
    //     let request = Request::new(GetDeviceRequest {
    //         dev_eui: dev_eui.to_string(),
    //     });
    //
    //     let response = self.client
    //         .get_device(request)
    //         .await
    //         .map_err(|e| AppError::ChirpStackError(format!("Erreur lors de la récupération du device: {}", e)))?;
    //
    //     Ok(response.into_inner().device.unwrap())
    // }

    // Ajoutez ici d'autres méthodes pour interagir avec ChirpStack
}

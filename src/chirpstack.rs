//! Module pour la communication avec ChirpStack.
//! 
//! Ce module gère la connexion et l'interaction avec le serveur ChirpStack.

use tonic::{transport::Channel, Request};
use crate::config::ChirpstackConfig;
use crate::utils::AppError;

// Importation des types générés
use chirpstack_api::api::device_service_client::DeviceServiceClient;
use chirpstack_api::api::application_service_client::ApplicationServiceClient;
use chirpstack_api::api::{GetDeviceRequest, Device, ListApplicationsRequest, ListApplicationsResponse};

pub struct ChirpstackClient {
    config: ChirpstackConfig,
    device_client: DeviceServiceClient<Channel>,
    application_client: ApplicationServiceClient<Channel>,
}

impl ChirpstackClient {
    pub async fn new(config: ChirpstackConfig) -> Result<Self, AppError> {
        // Créez une connexion au serveur ChirpStack
        let channel = Channel::from_shared(config.server_address.clone())
            .unwrap()
            .connect()
            .await
            .map_err(|e| AppError::ChirpStackError(format!("Erreur de connexion: {}", e)))?;

        // Créez les clients gRPC
        let device_client = DeviceServiceClient::new(channel.clone());
        let application_client = ApplicationServiceClient::new(channel);

        Ok(ChirpstackClient { 
            config,
            device_client,
            application_client,
        })
    }

    pub async fn list_applications(&self) -> Result<ListApplicationsResponse, AppError> {
        let request = Request::new(ListApplicationsRequest {
            limit: 100,  // Vous pouvez ajuster cette valeur selon vos besoins
            offset: 0,
            search: String::new(),
            tenant_id: String::new(),
        });

        let response = self.application_client
            .clone()
            .list(request)
            .await
            .map_err(|e| AppError::ChirpStackError(format!("Erreur lors de la récupération des applications: {}", e)))?;

        Ok(response.into_inner())
    }

    // Ajoutez ici d'autres méthodes pour interagir avec ChirpStack
}

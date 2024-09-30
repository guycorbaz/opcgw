//! Module pour la communication avec ChirpStack.
//! 
//! Ce module gère la connexion et l'interaction avec le serveur ChirpStack.
//! Il fournit une interface pour effectuer des opérations sur les applications
//! et les appareils via l'API gRPC de ChirpStack.

use tonic::{transport::Channel, Request};
use crate::config::ChirpstackConfig;
use crate::utils::AppError;
use log::{info,warn,error,debug};

// Importation des types générés
use chirpstack_api::api::device_service_client::DeviceServiceClient;
use chirpstack_api::api::application_service_client::ApplicationServiceClient;
use chirpstack_api::api::{GetDeviceRequest, Device, ListApplicationsRequest, ListApplicationsResponse, ApplicationListItem};

/// Structure représentant un client ChirpStack.
/// 
/// Cette structure encapsule la configuration et les clients gRPC nécessaires
/// pour interagir avec l'API ChirpStack.
pub struct ChirpstackClient {
    config: ChirpstackConfig,
    device_client: DeviceServiceClient<Channel>,
    application_client: ApplicationServiceClient<Channel>,
}

impl ChirpstackClient {
    /// Crée une nouvelle instance de `ChirpstackClient`.
    ///
    /// # Arguments
    ///
    /// * `config` - La configuration ChirpStack à utiliser pour la connexion.
    ///
    /// # Retourne
    ///
    /// Un `Result` contenant soit le `ChirpstackClient` créé, soit une `AppError`.
    pub async fn new(config: ChirpstackConfig) -> Result<Self, AppError> {
        // Créez une connexion au serveur ChirpStack
        debug!("new");
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

    /// Liste les applications disponibles sur le serveur ChirpStack.
    ///
    /// # Retourne
    ///
    /// Un `Result` contenant soit un vecteur d'`Application`, soit une `AppError`.
    pub async fn list_applications(&self) -> Result<Vec<Application>, AppError> {
        debug!("Get list of applications");
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

        let applications = self.convert_to_applications(response.into_inner());
        Ok(applications)
    }
    

    /// Convertit la réponse de l'API en un vecteur d'`Application`.
    ///
    /// # Arguments
    ///
    /// * `response` - La réponse de l'API contenant la liste des applications.
    ///
    /// # Retourne
    ///
    /// Un vecteur d'`Application`.
    fn convert_to_applications(&self, response: ListApplicationsResponse) -> Vec<Application> {
        debug!("convert_to_application");
        
        response.result.into_iter().map(|app: ApplicationListItem| Application {
            id: app.id,
            name: app.name,
            description: app.description,
            // Map other fields here if needed
        }).collect()
    }

    // Ajoutez ici d'autres méthodes pour interagir avec ChirpStack
}

/// Structure représentant une application ChirpStack.
#[derive(Debug)]
pub struct Application {
    /// Identifiant unique de l'application.
    pub id: String,
    /// Nom de l'application.
    pub name: String,
    /// Description de l'application.
    pub description: String,
}


/// Affiche la liste des applications sur la console
///
/// # Arguments
///
/// `list` - La liste des éléments à imprimer
///
/// # Retourne
///
/// .
pub fn print_list<T: std::fmt::Display>(list: &Vec<T>) {
    for element in list {
        println!("Element: ");
    }
}


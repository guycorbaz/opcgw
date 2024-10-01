//! Module for communication with ChirpStack.
//!
//! This module manages the connection and interaction with the ChirpStack server.
//! It provides an interface to perform operations on applications
//! and devices via the ChirpStack gRPC API.

use crate::config::ChirpstackConfig;
use crate::utils::AppError;
use log::{debug, error, info, warn};
use tonic::service::{interceptor, Interceptor};
use tonic::{transport::Channel, Request, Status};

// Import generated types
use chirpstack_api::api::application_service_client::ApplicationServiceClient;
use chirpstack_api::api::device_service_client::DeviceServiceClient;
use chirpstack_api::api::{ApplicationListItem, Device, DeviceListItem, GetDeviceRequest, ListApplicationsRequest, ListApplicationsResponse, ListDevicesRequest, ListDevicesResponse};
use tonic::codegen::InterceptedService;

/// Structure representing a ChirpStack client.
///
/// This structure encapsulates the configuration and the gRPC clients needed
/// to interact with the ChirpStack API.
pub struct ChirpstackClient {
    config: ChirpstackConfig,
    //device_client: DeviceServiceClient<Channel>,
    device_client: DeviceServiceClient<InterceptedService<Channel, AuthInterceptor>>,
    //application_client: ApplicationServiceClient<Channel>,
    application_client: ApplicationServiceClient<InterceptedService<Channel, AuthInterceptor>>,
}

// Definition of the interceptor for authentication
#[derive(Clone)]
struct AuthInterceptor {
    api_token: String,
}

impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", self.api_token).parse().unwrap(),
        );
        Ok(request)
    }
}

impl ChirpstackClient {
    pub fn config(&self) -> &ChirpstackConfig {
        &self.config
    }
    /// Creates a new instance of `ChirpstackClient`.
    ///
    /// # Arguments
    ///
    /// * `config` - The ChirpStack configuration to use for the connection.
    ///
    /// # Returns
    ///
    /// A `Result` containing either the created `ChirpstackClient` or an `AppError`.
    pub async fn new(config: ChirpstackConfig) -> Result<Self, AppError> {
        // Create a connexion to server
        debug!("new {:?}", config);
        let channel = Channel::from_shared(config.server_address.clone())
            .unwrap()
            .connect()
            .await
            .map_err(|e| AppError::ChirpStackError(format!("Connexion error: {}", e)))?;


        let interceptor = AuthInterceptor {
            api_token: config.api_token.clone(),
        };

        //let device_client = DeviceServiceClient::new(channel.clone());
        //let application_client = ApplicationServiceClient::new(channel.clone());
        let device_client = DeviceServiceClient::with_interceptor(channel.clone(), interceptor.clone());
        let application_client = ApplicationServiceClient::with_interceptor(channel, interceptor.clone());

        Ok(ChirpstackClient {
            config,
            device_client,
            application_client,
        })
    }

    /// Lists the applications available on the ChirpStack server.
    ///
    /// # Arguments
    ///
    /// * `tenand_id` - The ID of the tenant containing the applications
    ///
    /// # Returns
    ///
    /// A `Result` containing either a vector of `Application`, or an `AppError`.
    pub async fn list_applications(&self) -> Result<Vec<ApplicationDetail>, AppError> {
        debug!("Get list of applications");
        debug!("Create request");
        let request = Request::new(ListApplicationsRequest {
            limit: 100, // Vous pouvez ajuster cette valeur selon vos besoins
            offset: 0,
            search: String::new(),
            tenant_id: self.config.tenant_id.clone(), // We work on only one tenant defined in parameter file
        });
        debug!("Request created with: {:?}", request);

        debug!("Send request");
        let response = self
            .application_client
            .clone()
            .list(request)
            .await
            .map_err(|e| {
                AppError::ChirpStackError(format!("Error when collecting application list: {}", e))
            })?;
        debug!("Convert result");
        let applications = self.convert_to_applications(response.into_inner());
        Ok(applications)
    }

    pub async fn list_devices(&self, application_id: String) -> Result<Vec<DeviceDetail>, AppError> {
        debug!("Get list of devices");
        debug!("Create request");
        let request = Request::new(ListDevicesRequest {
            limit: 100,
            offset: 0,
            search: String::new(),
            application_id: application_id,
            multicast_group_id: String::new(),
        });
        debug!("Request created with: {:?}", request);

        debug!("Send request");
        let response= self
            .device_client
            .clone()
            .list(request)
            .await
            .map_err(|e: Status| {
                AppError::ChirpStackError(format!("Error when collecting devices list: {e}"))
            })?;
            debug!("Convert result");
            let devices: Vec<DeviceDetail> = self.convert_to_devices(response.into_inner());
            Ok(devices)


    }



    /// Converts the API response into a vector of `Application`.
    ///
    /// # Arguments
    ///
    /// * `response` - The API response containing the list of applications.
    ///
    /// # Returns
    ///
    /// A vector of `Application`.
    fn convert_to_applications(&self, response: ListApplicationsResponse) -> Vec<ApplicationDetail> {
        debug!("convert_to_applications");

        response
            .result
            .into_iter()
            .map(|app: ApplicationListItem| ApplicationDetail {
                id: app.id,
                name: app.name,
                description: app.description,
                // Map other fields here if needed
            })
            .collect()
    }

    fn convert_to_devices(&self, response: ListDevicesResponse) -> Vec<DeviceDetail> {
        debug!("convert_to_devices");

        response
            .result
            .into_iter()
            .map(|dev:DeviceListItem| DeviceDetail {
                dev_eui: dev.dev_eui,
                name: dev.name,
                description: dev.description,
                // Map other fields here if needed
            })
            .collect()
    }

    // Ajoutez ici d'autres méthodes pour interagir avec ChirpStack
}

/// Structure représentant une application ChirpStack.
#[derive(Debug)]
pub struct ApplicationDetail {
    /// Unique application identifier
    pub id: String,
    /// Application name
    pub name: String,
    /// Application description
    pub description: String,
}

#[derive(Debug)]
pub struct DeviceDetail {
    pub dev_eui: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug)]
pub struct DeviceStatusDetail {
    pub margin: i32,
    pub external_power_source: bool,
    pub battery_level: f32,
}

#[derive(Debug)]
pub struct DeviceStateDetail {
    pub name: String,
    pub value: String,
}

/// Prints the list of applications to the console
///
/// # Arguments
///
/// `list` - The list of applications to print
///
/// # Returns
///
/// .
pub fn print_app_list(list: &Vec<ApplicationDetail>) {
    for app in list {
        println!(
            "ID: {}, Nom: {}, Description: {}",
            app.id, app.name, app.description
        );
    }
}

/// Prints the list of deices to the console
///
/// # Arguments
///
/// `list` - The list of devices to print
///
/// # Returns
///
/// .
pub fn print_dev_list(list: &Vec<DeviceDetail>) {
    for dev in list {
        println!(
            "euid: {}, Nom: {}, Description: {}",
            dev.dev_eui, dev.name, dev.description
        );
    }
}

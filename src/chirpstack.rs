// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage communications with Chirpstack 4 server
//!
//!
//!
//! # Example:
//! Add example code...

use crate::config::ChirpstackConfig;
use crate::utils::OpcGwError;
use log::{debug, error, info, trace, warn};
use prost_types::Timestamp;
use std::collections::HashMap;
use std::time::{Duration, SystemTime};
use tonic::service::Interceptor;
use tonic::{transport::Channel, Request, Status};

// Import generated types
use chirpstack_api::api::application_service_client::ApplicationServiceClient;
use chirpstack_api::api::device_service_client::DeviceServiceClient;
use chirpstack_api::api::{
    ApplicationListItem, DeviceListItem, GetDeviceRequest,
    ListApplicationsRequest, ListApplicationsResponse, ListDevicesRequest, ListDevicesResponse,
};
use chirpstack_api::api::{DeviceState, GetDeviceMetricsRequest};
use chirpstack_api::common::{Aggregation, Metric, MetricDataset};
use serde::Deserialize;
use tokio::time::Instant;
use tonic::codegen::InterceptedService;

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

/// Structure representing a ChirpStack client.
///
/// This structure encapsulates the configuration and the gRPC clients needed
/// to interact with the ChirpStack API.
/// Represents a client for interacting with the ChirpStack API.
///
#[derive(Debug, Clone)]
pub struct ChirpstackClient {
    /// Configuration for the ChirpStack connection.
    config: ChirpstackConfig,
    /// Client for interacting with device-related endpoints.
    device_client: DeviceServiceClient<InterceptedService<Channel, AuthInterceptor>>,
    /// Client for interacting with application-related endpoints.
    application_client: ApplicationServiceClient<InterceptedService<Channel, AuthInterceptor>>,
}

impl ChirpstackClient {
    pub fn config(&self) -> &ChirpstackConfig {
        debug!("Return chirpstack config");
        //trace!("Return chirpstack config: {:?}", self.config);
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
    pub async fn new(config: &ChirpstackConfig) -> Result<Self, OpcGwError> {
        debug!("Create a new chirpstack connection");
        //trace!("With: {:#?}", config);
        let channel = Channel::from_shared(config.server_address.clone())
            .unwrap()
            .connect()
            .await
            .map_err(|e| OpcGwError::ChirpStackError(format!("Connexion error: {}", e)))?;

        let interceptor = AuthInterceptor {
            api_token: config.api_token.clone(),
        };

        trace!("Create DeviceServiceClient");
        let device_client =
            DeviceServiceClient::with_interceptor(channel.clone(), interceptor.clone());
        trace!("Create ApplicationServiceClient");
        let application_client =
            ApplicationServiceClient::with_interceptor(channel, interceptor.clone());

        Ok(ChirpstackClient {
            config: config.clone(),
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
    pub async fn list_applications(&self) -> Result<Vec<ApplicationDetail>, OpcGwError> {
        debug!("Get list of applications");
        trace!("Create request");
        let request = Request::new(ListApplicationsRequest {
            limit: 100, // Vous pouvez ajuster cette valeur selon vos besoins
            offset: 0,
            search: String::new(),
            tenant_id: self.config.tenant_id.clone(), // We work on only one tenant defined in parameter file
        });
        trace!("Request created with: {:?}", request);

        trace!("Send request");
        let response = self
            .application_client
            .clone()
            .list(request)
            .await
            .map_err(|e| {
                OpcGwError::ChirpStackError(format!("Error when collecting application list: {}", e))
            })?;
        trace!("Convert result");
        let applications = self.convert_to_applications(response.into_inner());
        Ok(applications)
    }

    pub async fn list_devices(
        &self,
        application_id: String,
    ) -> Result<Vec<DeviceListDetail>, OpcGwError> {
        debug!("Get list of devices");
        trace!("for application: {:?}", application_id);
        trace!("Create request");

        let request = Request::new(ListDevicesRequest {
            limit: 100,
            offset: 0,
            search: String::new(),
            application_id: application_id,
            multicast_group_id: String::new(),
        });
        trace!("Request created with: {:?}", request);

        trace!("Send request");
        let response = self
            .device_client
            .clone()
            .list(request)
            .await
            .map_err(|e: Status| {
                OpcGwError::ChirpStackError(format!("Error when collecting devices list: {e}"))
            })?;
        trace!("Convert result");
        let devices: Vec<DeviceListDetail> = self.convert_to_devices(response.into_inner());
        Ok(devices)
    }

    pub async fn get_device_details(&mut self, dev_eui: String) -> Result<DeviceDetails, OpcGwError> {
        debug!("Get device details");
        //trace!("for device: {:?}", dev_eui);
        let request = Request::new(GetDeviceRequest { dev_eui });

        match self.device_client.get(request).await {
            Ok(response) => {
                let device = response.into_inner();
                Ok(DeviceDetails {
                    dev_eui: device.device.clone().unwrap().dev_eui,
                    name: device.device.clone().unwrap().name,
                    application_id: device.device.clone().unwrap().application_id,
                    is_disabled: device.device.clone().unwrap().is_disabled,
                    description: device.device.clone().unwrap().description,
                    battery_level: device.device_status.unwrap().battery_level,
                    margin: device.device_status.unwrap().margin,
                    variables: device.device.clone().unwrap().variables,
                    tags: device.device.clone().unwrap().tags,
                })
            }
            Err(e) => Err(OpcGwError::ChirpStackError(format!(
                "Error getting device metrics: {}",
                e
            ))),
        }
    }

    pub async fn get_device_metrics(
        &mut self,
        dev_eui: String,
        duration: u64,
        aggregation: i32,
    ) -> Result<DeviceMetric, OpcGwError> {
        debug!("Get device metrics for");
        //trace!("for device: {:?}", dev_eui);
        trace!("Create request");
        let request = Request::new(GetDeviceMetricsRequest {
            dev_eui: dev_eui.clone(),
            start: Some(Timestamp::from(SystemTime::now())),
            end: Some(Timestamp::from(
                SystemTime::now() + Duration::from_secs(duration),
            )),
            aggregation: aggregation,
        });

        trace!("Format result");
        match self.device_client.get_metrics(request).await {
            Ok(response) => {
                let inner_response = response.into_inner();

                let metrics: HashMap<String, Metric> = inner_response
                    .metrics
                    .into_iter()
                    .map(|(key, value)| (key, value))
                    .collect();

                let states: HashMap<String, DeviceState> = inner_response
                    .states
                    .into_iter()
                    .map(|(key, value)| (key, value))
                    .collect();

                Ok(DeviceMetric { metrics, states })
            }
            Err(e) => Err(OpcGwError::ChirpStackError(format!(
                "Error getting device metrics: {}",
                e
            ))),
        }
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
    fn convert_to_applications(
        &self,
        response: ListApplicationsResponse,
    ) -> Vec<ApplicationDetail> {
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

    /// Converts the API response into a vector of `DeviceListDetail`.
    ///
    /// # Arguments
    ///
    /// * `response` - The API response containing the list of devices.
    ///
    /// # Returns
    ///
    /// A vector of `DeviceListDetail`.
    fn convert_to_devices(&self, response: ListDevicesResponse) -> Vec<DeviceListDetail> {
        debug!("convert_to_devices");

        response
            .result
            .into_iter()
            .map(|dev: DeviceListItem| DeviceListDetail {
                dev_eui: dev.dev_eui,
                name: dev.name,
                description: dev.description,
                // Map other fields here if needed
            })
            .collect()
    }

    // Ajoutez ici d'autres méthodes pour interagir avec ChirpStack
}

/// Structure representing a chirpstack application.
#[derive(Debug, Deserialize, Clone)]
pub struct ApplicationDetail {
    /// Unique application identifier
    pub id: String,
    /// Application name
    pub name: String,
    /// Application description
    pub description: String,
}

#[derive(Debug, Deserialize, Clone)]
/// Represents details of a device in a list format.
pub struct DeviceListDetail {
    /// The unique identifier for the device (DevEUI).
    pub dev_eui: String,
    /// The name of the device.
    pub name: String,
    /// A description of the device.
    pub description: String,
}

#[derive(Debug, Deserialize, Clone)]
/// Represents detailed information about a device.
pub struct DeviceDetails {
    /// The unique identifier for the device (DevEUI).
    pub dev_eui: String,
    /// The name of the device.
    pub name: String,
    /// A description of the device.
    pub description: String,
    /// The ID of the application this device belongs to.
    pub application_id: String,
    /// Indicates whether the device is disabled.
    pub is_disabled: bool,
    /// The current battery level of the device.
    pub battery_level: f32,
    /// The signal margin of the device.
    pub margin: i32,
    /// Custom variables associated with the device.
    pub variables: HashMap<String, String>,
    /// Tags associated with the device.
    pub tags: HashMap<String, String>,
}

/// Represents metrics and states for a device.
/// #[derive(Debug, Deserialize, Clone)]
pub struct DeviceMetric {
    /// A map of metric names to their corresponding Metric objects.
    pub metrics: HashMap<String, Metric>,
    /// A map of state names to their corresponding DeviceState objects.
    pub states: HashMap<String, DeviceState>,
}

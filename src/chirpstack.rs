// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage communications with Chirpstack 4 server

use crate::config::ChirpstackConfig;
use crate::utils::OpcGwError;
use chirpstack_api::api::{DeviceState, GetDeviceMetricsRequest};
use chirpstack_api::common::Metric;
use log::{debug, error, trace};
use prost_types::Timestamp;
use serde::Deserialize;
use std::collections::HashMap;
use std::time::SystemTime;
use tokio::runtime::{Builder, Runtime};
use tokio::time::{sleep, Duration};
use tonic::codegen::InterceptedService;
use tonic::service::Interceptor;
use tonic::{transport::Channel, Request, Status};

// Import generated types
use chirpstack_api::api::application_service_client::ApplicationServiceClient;
use chirpstack_api::api::device_service_client::DeviceServiceClient;
use chirpstack_api::api::{
    ApplicationListItem, DeviceListItem, GetDeviceRequest, ListApplicationsRequest,
    ListApplicationsResponse, ListDevicesRequest, ListDevicesResponse,
};

/// Structure representing a chirpstack application.
#[derive(Debug, Deserialize, Clone)]
pub struct ApplicationDetail {
    /// Unique application identifier
    pub application_id: String,
    /// Application name
    pub application_name: String,
    /// Application description
    pub application_description: String,
}

/// Represents details of a device in a list format.
#[derive(Debug, Deserialize, Clone)]
pub struct DeviceListDetail {
    /// The unique identifier for the device (DevEUI).
    pub dev_eui: String,
    /// The name of the device.
    pub name: String,
    /// A description of the device.
    pub description: String,
}

/// Represents detailed information about a device.
#[derive(Debug, Deserialize, Clone)]
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
#[derive(Debug, Deserialize, Clone)]
pub struct DeviceMetric {
    //FIXME
    /// A map of metric names to their corresponding Metric objects.
    pub metrics: HashMap<String, Metric>,
    // A map of state names to their corresponding DeviceState objects.
    //pub states: HashMap<String, DeviceState>,
}

/// Definition of the interceptor for passing
/// authentication token to Chirpstack server
#[derive(Clone)]
struct AuthInterceptor {
    /// Chirpstack API token
    api_token: String,
}

/// Interceptor that allow to pass api token to chirpstack server
impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", self.api_token).parse().unwrap(),
        );
        Ok(request)
    }
}

/// Chirpstack poller
#[derive(Debug, Clone)]
pub struct ChirpstackPoller {
    /// Configuration for the ChirpStack connection.
    config: ChirpstackConfig,
    /// Client for interacting with device-related endpoints.
    device_client: Option<DeviceServiceClient<InterceptedService<Channel, AuthInterceptor>>>,
    /// Client for interacting with application-related endpoints.
    application_client:
        Option<ApplicationServiceClient<InterceptedService<Channel, AuthInterceptor>>>,
}

impl ChirpstackPoller {
    /// Initialize a new Chirpstack poller instance.
    /// The chirpstacl poller has to be instatiated in
    /// a tokio runtime
    ///
    /// Example
    ///     let chirpstack_poller = match ChirpstackPoller::new(&application_config.chirpstack).await{
    ///         Ok(poller) => poller,
    ///         Err(e) => panic!("Failed to create chirpstack poller: {}", e),
    ///     };
    ///
    pub async fn new(config: &ChirpstackConfig) -> Result<Self, OpcGwError> {
        debug!("Create a new chirpstack connection");
        let channel = Channel::from_shared(config.server_address.clone())
            .unwrap()
            .connect()
            .await
            .map_err(|e| OpcGwError::ChirpStackError(format!("Connection error: {}", e)))?;

        let interceptor = AuthInterceptor {
            api_token: config.api_token.clone(),
        };

        // Create Chirpstack devices interface
        trace!("Create DeviceServiceClient");
        let device_client =
            DeviceServiceClient::with_interceptor(channel.clone(), interceptor.clone());

        // Create Chirpstack applications interface
        trace!("Create ApplicationServiceClient");
        let application_client =
            ApplicationServiceClient::with_interceptor(channel, interceptor.clone());

        Ok(ChirpstackPoller {
            config: config.clone(),
            device_client: Some(device_client),
            application_client: Some(application_client),
        })
    }

    /// Run the ChirpStack client process
    /// This has to be launched by tokio
    /// Example
    ///     let chirpstack_handle = tokio::spawn(async move {
    ///         if let Err(e) = chirpstack_poller.run().await {
    ///             error!("ChirpStack poller error: {:?}", e);
    ///         }
    ///
    pub async fn run(&mut self) -> Result<(), OpcGwError> {
        //TODO: Implement
        trace!(
            "Running chirpstack client poller every {} s",
            self.config.polling_frequency
        );
        let duration = Duration::from_secs(self.config.polling_frequency);
        loop {
            debug!("Polling metrics");
            if let Err(e) = self.poll_metrics().await {
                error!("Error polling devices: {:?}", e);
            }
            tokio::time::sleep(duration).await;
        }
    }

    async fn poll_devices(&self) -> Result<(), OpcGwError> {
        // Implement device polling logic
        let app_list = self.get_applications_list_from_server().await?;
        for app in app_list {
            let dev_list = self
                .get_devices_list_from_server(app.application_id.clone())
                .await
                .unwrap();
            debug!("Devices list: {:#?}", dev_list);
        }

        Ok(())
    }

    async fn poll_applications(&self) -> Result<(), OpcGwError> {
        // Implement application polling logic
        let app_list = self.get_applications_list_from_server().await?;
        println!("Applications: {:?}", app_list);
        Ok(())
    }

    /// Poll metrics for each device
    async fn poll_metrics(&mut self) -> Result<(), OpcGwError> {
        debug!("Polling metrics");
        let app_list = self.get_applications_list_from_server().await?;
        for app in app_list {
            let dev_list = self
                .get_devices_list_from_server(app.application_id.clone())
                .await
                .unwrap();
            for dev in dev_list {
                let dev_metrics = &self
                    .get_device_metrics_from_server(
                        dev.dev_eui.clone(),
                        self.config.polling_frequency,
                        1,
                    )
                    .await?;
                for metric in dev_metrics.metrics.clone() {
                    println!("{:#?}", metric);
                }
            }
        }
        Ok(())
    }

    /// Lists the applications available on the ChirpStack server.
    pub async fn get_applications_list_from_server(
        &self,
    ) -> Result<Vec<ApplicationDetail>, OpcGwError> {
        debug!("Get list of applications");
        trace!("Create request");
        let request = Request::new(ListApplicationsRequest {
            limit: 100, // Vous pouvez ajuster cette valeur selon vos besoins
            offset: 0,
            search: String::new(),
            tenant_id: self.config.tenant_id.clone(), // We work on only one tenant defined in parameter file
        });
        trace!("Request created with: {:#?}", request);

        trace!("Send request");
        let response = self
            .application_client
            .clone()
            .expect("Application client is not initialized")
            .list(request)
            .await
            .map_err(|e| {
                OpcGwError::ChirpStackError(format!(
                    "Error when collecting application list: {}",
                    e
                ))
            })?;
        trace!("Convert result");

        let applications = self.convert_to_applications(response.into_inner());
        Ok(applications)
    }

    /// Get device list from Chirpstack server
    pub async fn get_devices_list_from_server(
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
            application_id,
            multicast_group_id: String::new(), // We don't need the multicast group for now
        });
        trace!("Request created with: {:?}", request);

        trace!("Send request");
        let response = self
            .device_client
            .clone()
            .expect("Device client is not initialized")
            .list(request)
            .await
            .map_err(|e: Status| {
                OpcGwError::ChirpStackError(format!("Error when collecting devices list: {e}"))
            })?;
        trace!("Convert result");
        let devices: Vec<DeviceListDetail> = self.convert_to_devices(response.into_inner());
        Ok(devices)
    }

    /// Get device details from Chirpstack server
    pub async fn get_device_details_from_server(
        &mut self,
        dev_eui: String,
    ) -> Result<DeviceDetails, OpcGwError> {
        debug!("Get device details");
        todo!();
        //    trace!("for device: {:?}", dev_eui);
        //    let request = Request::new(GetDeviceRequest { dev_eui });

        //    match self.device_client.get(request).await {
        //        Ok(response) => {
        //            let device = response.into_inner();
        //            Ok(DeviceDetails {
        //                dev_eui: device.device.clone().unwrap().dev_eui,
        //                name: device.device.clone().unwrap().name,
        //                application_id: device.device.clone().unwrap().application_id,
        //                is_disabled: device.device.clone().unwrap().is_disabled,
        //                description: device.device.clone().unwrap().description,
        //                battery_level: device.device_status.unwrap().battery_level,
        //                margin: device.device_status.unwrap().margin,
        //                variables: device.device.clone().unwrap().variables,
        //                tags: device.device.clone().unwrap().tags,
        //            })
        //        }
        //        Err(e) => Err(OpcGwError::ChirpStackError(format!(
        //            "Error getting device metrics: {}",
        //            e
        //        ))),
        //    }
    }

    /// Get device metrics from Chirpèstack server
    pub async fn get_device_metrics_from_server(
        &mut self,
        dev_eui: String,
        duration: u64,
        aggregation: i32,
    ) -> Result<DeviceMetric, OpcGwError> {
        debug!("Get device metrics for");

        trace!("for device: {:?}", dev_eui);
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

        if let Some(device_client) = &mut self.device_client {
            match device_client.get_metrics(request).await {
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

                    Ok(DeviceMetric { metrics })
                }
                Err(e) => Err(OpcGwError::ChirpStackError(format!(
                    "Error getting device metrics: {}",
                    e
                ))),
            }
        } else {
            Err(OpcGwError::ChirpStackError(String::from(
                "Device client is not initialized",
            )))
        }
    }

    /// Converts the API response into a vector of `Application`.
    fn convert_to_applications(
        &self,
        response: ListApplicationsResponse,
    ) -> Vec<ApplicationDetail> {
        debug!("convert_to_applications");

        response
            .result
            .into_iter()
            .map(|app: ApplicationListItem| ApplicationDetail {
                application_id: app.id,
                application_name: app.name,
                application_description: app.description,
                // Map other fields here if needed
            })
            .collect()
    }

    /// Converts the API response into a vector of `DeviceListDetail`.
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
}

/// Print the list of applications on screen
/// At the time being, this is just for debugging
pub fn print_application_list(list: &Vec<ApplicationDetail>) {
    for app in list {
        println!("{:#?}", app);
    }
}

pub fn print_device_list(list: &Vec<DeviceListDetail>) {
    for device in list {
        println!(
            "Device EUI: {}, Name: {}, Description: {}",
            device.dev_eui, device.name, device.description
        );
    }
}

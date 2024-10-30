// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//! Manage communications with Chirpstack 4 server

#![allow(unused)]

use crate::config::{ChirpstackPollerConfig, AppConfig, MetricTypeConfig};
use crate::utils::OpcGwError;
use chirpstack_api::api::{DeviceState, GetDeviceMetricsRequest};
use chirpstack_api::common::Metric;
use log::{debug, error, trace};
use prost_types::Timestamp;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
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
use crate::storage::{MetricType, Storage};

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

/// Represents metrics and states for a device.
#[derive(Debug, Deserialize, Clone)]
pub struct DeviceMetric {
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
#[derive(Clone)]
pub struct ChirpstackPoller {
    /// Configuration for the ChirpStack connection.
    config: AppConfig,
    /// Client for interacting with device-related endpoints.
    device_client: Option<DeviceServiceClient<InterceptedService<Channel, AuthInterceptor>>>,
    /// Client for interacting with application-related endpoints.
    application_client:
        Option<ApplicationServiceClient<InterceptedService<Channel, AuthInterceptor>>>,
    /// Metrics list
    pub storage: Arc<std::sync::Mutex<Storage>>,
}

impl ChirpstackPoller {
    /// Create and initialize a new Chirpstack poller instance.
    /// for one tenant which id is loaded in configuration
    /// The chirpstack poller has to be instantiated in
    /// a tokio runtime
    ///
    /// Example
    ///     let chirpstack_poller = match ChirpstackPoller::new(&application_config.chirpstack).await{
    ///         Ok(poller) => poller,
    ///         Err(e) => panic!("Failed to create chirpstack poller: {}", e),
    ///     };
    ///
    pub async fn new(config: &AppConfig, storage: Arc<Mutex<Storage>>) -> Result<Self, OpcGwError> {
        debug!("Create a new chirpstack connection");
        let channel = Channel::from_shared(config.chirpstack.server_address.clone())
            .unwrap()
            .connect()
            .await
            .map_err(|e| OpcGwError::ChirpStackError(format!("Connection error: {}", e)))?;

        let interceptor = AuthInterceptor {
            api_token: config.chirpstack.api_token.clone(),
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
            storage,
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
        trace!(
            "Running chirpstack client poller every {} s",
            self.config.chirpstack.polling_frequency
        );
        let duration = Duration::from_secs(self.config.chirpstack.polling_frequency);
        // Start the poller
        loop {
            if let Err(e) = self.poll_metrics().await {
                error!("Error polling devices: {:?}", e);
            }
            tokio::time::sleep(duration).await;
        }
    }


    /// Polls metrics from the configured applications and devices.
    ///
    /// This function polls the metrics for all devices listed in the configuration.
    /// Initially, it collects all device IDs from the application list.
    /// For each device in each application, it pushes the device ID into a vector.
    ///
    /// # Errors
    ///
    /// Returns an `OpcGwError` if there is an issue during the polling process.
    async fn poll_metrics(&mut self) -> Result<(), OpcGwError> {
        debug!("Polling metrics");
        let app_list = self.config.application_list.clone();
        //trace!("app_list: {:#?}", app_list);
        // Collect device IDs first
        let mut device_ids = Vec::new();
        for app in &self.config.application_list {
            for dev in &app.device_list {
                device_ids.push(dev.device_id.clone());
            }
            //trace!("device_ids: {:#?}", device_ids);
        }

        // Now, fetch metrics using mutable borrow
        for dev_id in device_ids {
            let dev_metrics = self
                .get_device_metrics_from_server(
                    dev_id.clone(),
                    self.config.chirpstack.polling_frequency,
                    1,
                )
                .await?;
            for metric in &dev_metrics.metrics.clone() {
                //trace!("------Got metrics:");
                //trace!("{:#?}", metric);
                for (key, metric) in &dev_metrics.metrics {
                    self.store_metric(&dev_id.clone(), &metric.clone());
                }
            }
        }
        Ok(())
    }

    /// Stores a metric for a given device based on its metric type configuration.
    ///
    /// This function first logs the intention to store the metric and captures
    /// the metric's name and its first data value. It then tries to retrieve the
    /// metric type from the configuration. Based on the metric type, it processes
    /// the metric and stores the value accordingly.
    ///
    /// # Arguments
    ///
    /// * `device_id` - A reference to the ID of the device.
    /// * `metric` - A reference to the metric to be stored.
    pub fn store_metric(&self, device_id: &String, metric: &Metric) {
        trace!("Store device metric in storage");
        let metric_name = metric.name.clone();
        let value = metric.datasets[0].data[0].clone();
        trace!("Value for {:?} is: {:#?}", metric_name, value);

        match self.config.get_metric_type(&metric_name) {
            Some(metric_type) => {
                trace!("Metric type: {:?} for metric {:?}", metric_type, metric_name);
                match metric_type {
                    MetricTypeConfig::Bool => {},
                    MetricTypeConfig::Int => {},
                    MetricTypeConfig::Float => {
                        let storage = self.storage.clone();
                        let mut storage = storage.lock().expect("Can't lock storage"); // Should we wait if already locked ?
                        storage.set_metric_value(device_id,&metric_name,  MetricType::Float(value.into()));
                    },
                    MetricTypeConfig::String => {},
                }
            },
            None => {
                // Log or handle the None case according to your needs.
                trace!("No metric type found for metric: {:?}", metric_name);
            },
        };

    }


    /// Retrieves the list of applications from the server.
    ///
    /// This asynchronous function sends a request to the application server to obtain a list of applications
    /// associated with a specific tenant. The request includes parameters for limiting the number of applications
    /// retrieved (`limit`), and an offset value to specify the starting point in the list of applications. The `tenant_id`
    /// is used to specify the tenant for which the applications are being requested.
    ///
    /// # Returns
    /// A result containing a vector of `ApplicationDetail` on success, or an `OpcGwError` on failure.
    ///
    /// # Errors
    /// Returns `OpcGwError::ChirpStackError` if there is an error while collecting the application list.
    ///
    /// # Example
    /// ```rust
    /// let applications = my_instance.get_applications_list_from_server().await?;
    /// ```
    ///
    /// Note: Ensure that the application client is initialized before calling this function.
    pub async fn get_applications_list_from_server(
        &self,
    ) -> Result<Vec<ApplicationDetail>, OpcGwError> {
        debug!("Get list of applications");
        trace!("Create request");
        let request = Request::new(ListApplicationsRequest {
            limit: 100, // Can be adjusted according to needs, but what does it means ?
            offset: 0,
            search: String::new(),
            tenant_id: self.config.chirpstack.tenant_id.clone(), // We work on only one tenant defined in parameter file
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

    /// Retrieves a list of devices from the server for a specified application.
    ///
    /// This asynchronous function communicates with the server to obtain a list of devices
    /// associated with the given application ID. It constructs a request with specific parameters,
    /// sends the request using the device client, and processes the server's response. The resulting
    /// list of devices is then converted into a vector of `DeviceListDetail` objects and returned.
    ///
    /// # Arguments
    ///
    /// * `application_id` - A `String` representing the application ID for which the device list
    ///   is being requested.
    ///
    /// # Returns
    ///
    /// * `Result<Vec<DeviceListDetail>, OpcGwError>` - On success, returns a vector of `DeviceListDetail`
    ///   objects. On failure, returns an `OpcGwError` indicating the type of error encountered.
    ///
    /// # Errors
    ///
    /// This function will return an `OpcGwError` if:
    /// * The device client is not initialized.
    /// * There is an error when communicating with the server.
    ///
    /// # Example
    ///
    /// ```rust
    /// let application_id = "some_application_id".to_string();
    /// let devices = some_instance.get_devices_list_from_server(application_id).await;
    /// match devices {
    ///     Ok(device_list) => println!("Devices: {:?}", device_list),
    ///     Err(error) => eprintln!("Error: {:?}", error),
    /// }
    /// ```
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
            aggregation,
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

    /// Converts a ListApplicationsResponse into a vector of ApplicationDetail.
    ///
    /// This function takes a ListApplicationsResponse, which contains a list of ApplicationListItem,
    /// and converts it into a vector of ApplicationDetail. Each ApplicationListItem is mapped to
    /// an ApplicationDetail with corresponding fields.
    ///
    /// # Arguments
    ///
    /// * `response` - The ListApplicationsResponse containing application details to convert.
    ///
    /// # Returns
    ///
    /// * `Vec<ApplicationDetail>` - A vector of ApplicationDetail containing the converted application details.
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

/// Prints the details of applications in a formatted manner.
///
/// This function takes a reference to a vector of `ApplicationDetail`
/// instances and prints each application's details in a pretty-printed
/// format using the `println!` macro.
///
/// # Arguments
///
/// * `list` - A reference to a vector containing application details.
///
/// # Examples
///
/// ```
/// let applications = vec![
///     ApplicationDetail { /* fields */ },
///     ApplicationDetail { /* fields */ },
/// ];
/// print_application_list(&applications);
/// ```
pub fn print_application_list(list: &Vec<ApplicationDetail>) {
    for app in list {
        println!("{:#?}", app);
    }
}

/// Prints the details of each device in the provided device list.
///
/// # Arguments
///
/// * `list` - A reference to a vector of `DeviceListDetail` containing device information.
///
/// # Example
///
/// ```
/// let device_list = vec![
///     DeviceListDetail {
///         dev_eui: "0018B20000001122".to_string(),
///         name: "Device1".to_string(),
///         description: "Temperature Sensor".to_string(),
///     },
///     DeviceListDetail {
///         dev_eui: "0018B20000003344".to_string(),
///         name: "Device2".to_string(),
///         description: "Humidity Sensor".to_string(),
///     },
/// ];
/// print_device_list(&device_list);
/// ```
///
/// This will print:
/// ```shell
/// Device EUI: 0018B20000001122, Name: Device1, Description: Temperature Sensor
/// Device EUI: 0018B20000003344, Name: Device2, Description: Humidity Sensor
/// ```
pub fn print_device_list(list: &Vec<DeviceListDetail>) {
    for device in list {
        println!(
            "Device EUI: {}, Name: {}, Description: {}",
            device.dev_eui, device.name, device.description
        );
    }
}

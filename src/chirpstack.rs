// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//! Manage communications with Chirpstack 4 server

use crate::config::{AppConfig, ChirpstackPollerConfig, OpcMetricTypeConfig};
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
use crate::storage::{MetricType, Storage};
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

/// This method is called to intercept a gRPC request and injects an authorization token into the request's metadata.
///
/// # Arguments
///
/// * `request` - The incoming gRPC request that will be intercepted.
///
/// # Returns
///
/// * `Result<Request<()>, Status>` - Returns the modified request with the authorization token added to its metadata,
/// or an error status if the token insertion fails.
///
/// # Errors
///
/// This method will panic if the authorization token cannot be parsed.
impl Interceptor for AuthInterceptor {
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        debug!("Interceptor::call");
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", self.api_token).parse().expect(
                &OpcGwError::ChirpStackError(format!("Failed to parse authorization token"))
                    .to_string(),
            ),
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
    /// Asynchronously creates a new Chirpstack connection using the provided configuration and storage.
    ///
    /// This function establishes a connection to the Chirpstack server and prepares clients for
    /// interacting with Chirpstack devices and applications. It utilizes an authentication interceptor
    /// for securing the API communications.
    ///
    /// # Arguments
    ///
    /// * `config` - A reference to the application configuration.
    /// * `storage` - A shared reference-counted, thread-safe storage.
    ///
    /// # Returns
    ///
    /// `Result<Self, OpcGwError>` - Returns an instance of `ChirpstackPoller` on success, or an `OpcGwError` on failure.
    ///
    /// # Errors
    ///
    /// This function will return an `OpcGwError` if it fails to connect to the Chirpstack server.
    ///
    /// # Example
    ///
    /// ```
    /// let config = AppConfig::new();
    /// let storage = Arc::new(Mutex::new(Storage::new()));
    /// let poller = ChirpstackPoller::new(&config, storage.clone()).await?;
    /// ```
    pub async fn new(config: &AppConfig, storage: Arc<Mutex<Storage>>) -> Result<Self, OpcGwError> {
        debug!("Create a new chirpstack connection");
        let channel = Channel::from_shared(config.chirpstack.server_address.clone())
            .expect(&OpcGwError::ChirpStackError(format!("Failed to create channel")).to_string())
            .connect()
            .await
            .map_err(|e| OpcGwError::ChirpStackError(format!("Connection error: {}", e)))?;

        // Create interceptor for authentification key
        trace!("Create authenticator");
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

    /// Runs the ChirpStack client poller at a specified interval defined in the configuration.
    ///
    /// The poller continuously invokes the `poll_metrics` function to fetch device metrics.
    /// If an error occurs during polling, it logs an error message but continues retrying
    /// after waiting for the specified duration.
    ///
    /// # Errors
    /// Returns an `OpcGwError` if an error occurs during polling.
    ///
    /// # Examples
    /// ```rust
    /// use your_crate::YourStruct;
    /// // Assumes you have appropriate initializations as per your implementation.
    /// let mut your_instance = YourStruct::new();
    /// your_instance.run().await?;
    /// ```
    pub async fn run(&mut self) -> Result<(), OpcGwError> {
        debug!(
            "Running chirpstack client poller every {} s",
            self.config.chirpstack.polling_frequency
        );
        let duration = Duration::from_secs(self.config.chirpstack.polling_frequency);
        // Start the poller
        loop {
            if let Err(e) = self.poll_metrics().await {
                error!(
                    "{}",
                    &OpcGwError::ChirpStackError(format!("Error polling devices: {:?}", e))
                );
            }
            tokio::time::sleep(duration).await;
        }
    }

    /// Asynchronously polls metrics for all devices in the configured application list.
    ///
    /// This function first collects all device IDs from the applications specified
    /// in the configuration. It then fetches metrics for each device by calling
    /// `get_device_metrics_from_server` and stores the received metrics using
    /// `store_metric`.
    ///
    /// # Errors
    /// Returns `OpcGwError` if there is an error in fetching the device metrics.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// let mut metrics_poller = MyMetricsPoller::new(config);
    /// metrics_poller.poll_metrics().await?;
    /// ```
    ///
    /// # Async
    /// This function is asynchronous and should be awaited.
    ///
    /// # Logging
    /// - Logs a debug message at the start of the function.
    /// - Logs the fetched metrics at trace level.
    async fn poll_metrics(&mut self) -> Result<(), OpcGwError> {
        debug!("Polling metrics");
        let app_list = self.config.application_list.clone();
        // Collect device IDs first
        let mut device_ids = Vec::new();
        for app in &self.config.application_list {
            for dev in &app.device_list {
                device_ids.push(dev.device_id.clone());
            }
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
                trace!("------Got metrics:");
                trace!("{:#?}", metric);
                for (key, metric) in &dev_metrics.metrics {
                    self.store_metric(&dev_id.clone(), &metric.clone());
                }
            }
        }
        Ok(())
    }

    /// Runs the ChirpStack client poller at a specified interval defined in the configuration.
    ///
    /// The poller continuously invokes the `poll_metrics` function to fetch device metrics.
    /// If an error occurs during polling, it logs an error message but continues retrying
    /// after waiting for the specified duration.
    ///
    /// # Errors
    /// Returns an `OpcGwError` if an error occurs during polling.
    ///
    /// # Examples
    /// ```rust
    /// use your_crate::YourStruct;
    /// // Assumes you have appropriate initializations as per your implementation.
    /// let mut your_instance = YourStruct::new();
    /// your_instance.run().await?;
    /// ```
    pub fn store_metric(&self, device_id: &String, metric: &Metric) {
        debug!("Store device metric in storage");
        let device_name = self
            .config
            .get_device_name(device_id)
            .expect(&OpcGwError::ChirpStackError(format!("Failed to get device name")).to_string());
        let metric_name = metric.name.clone();
        let value = metric.datasets[0].data[0].clone();

        match self.config.get_metric_type(&metric_name, device_id) {
            Some(metric_type) => match metric_type {
                OpcMetricTypeConfig::Bool => {}
                OpcMetricTypeConfig::Int => {}
                OpcMetricTypeConfig::Float => {
                    let storage = self.storage.clone();
                    let mut storage = storage.lock().expect(
                        &OpcGwError::ChirpStackError(format!("Can't lock storage")).to_string(),
                    );
                    storage.set_metric_value(
                        device_id,
                        &metric_name,
                        MetricType::Float(value.into()),
                    );
                    trace!("------------Dumping storage-----------------");
                    storage.dump_storage();
                }
                OpcMetricTypeConfig::String => {}
            },
            None => {
                error!(
                    "{}",
                    &OpcGwError::ChirpStackError(format!(
                        "No metric type found for metric: {:?} of device {:?}",
                        metric_name, device_name
                    ))
                );
            }
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
        debug!("Get device metrics");
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

    /// Converts a `ListApplicationsResponse` into a vector of `ApplicationDetail`.
    ///
    /// This method takes a `ListApplicationsResponse` and iterates over its
    /// `result` field, mapping each `ApplicationListItem` to an `ApplicationDetail`.
    ///
    /// # Parameters
    /// - `response`: The `ListApplicationsResponse` object containing the list
    ///   of application items to be converted.
    ///
    /// # Returns
    /// A vector of `ApplicationDetail` objects.
    ///
    /// # Example
    /// ```
    /// let response = ListApplicationsResponse {
    ///     result: vec![
    ///         ApplicationListItem { id: 1, name: String::from("App1"), description: String::from("Description1") },
    ///         ApplicationListItem { id: 2, name: String::from("App2"), description: String::from("Description2") }
    ///     ]
    /// };
    /// let app_details = convert_to_applications(response);
    /// assert_eq!(app_details.len(), 2);
    /// ```
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
        trace!("{:#?}", app);
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
        trace!(
            "Device EUI: {}, Name: {}, Description: {}",
            device.dev_eui, device.name, device.description
        );
    }
}

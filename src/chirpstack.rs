// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//! Manage communications with Chirpstack 4 server

use crate::config::{AppConfig, ChirpstackPollerConfig, OpcMetricTypeConfig};
use crate::utils::OpcGwError;
use chirpstack_api::api::{DeviceState, GetDeviceMetricsRequest};
use chirpstack_api::common::Metric;
use log::{debug, error, trace, warn};
use ping;
use prost_types::Timestamp;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{SystemTime, Instant};
use tokio::runtime::{Builder, Runtime};
use tokio::time::{sleep, Duration};
use tonic::codegen::InterceptedService;
use tonic::service::Interceptor;
use tonic::{transport::Channel, Request, Status};
use url::Url;

// Import generated types
use crate::storage::{ChirpstackStatus, MetricType, Storage};
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
        debug!("Create a new Chirpstack connection");

        Ok(ChirpstackPoller {
            config: config.clone(),
            storage,
        })
    }

    /// Asynchronously creates a channel for communication with the ChirpStack server.
    ///
    /// # Errors
    ///
    /// Returns an `OpcGwError` if the creation or connection of the channel fails.
    ///
    /// # Returns
    ///
    /// An `Ok` variant containing the `tonic::transport::Channel` if successful, else an error variant.
    ///
    /// # Example
    ///
    /// ```rust
    /// let channel = self.create_channel().await?;
    /// ```
    async fn create_channel(&self) -> Result<tonic::transport::Channel, OpcGwError> {
        debug!("Create channel");
        let channel = Channel::from_shared(self.config.chirpstack.server_address.clone())
            .map_err(|e| {
                OpcGwError::ConfigurationError(format!("Failed to create channel: {}", e))
            })?
            .connect()
            .await
            .map_err(|e| {
                OpcGwError::ConfigurationError(format!("Failed to intercept channel: {}", e))
            })?;
        Ok(channel)
    }

    /// Creates an authentication interceptor.
    ///
    /// This method initializes and returns an `AuthInterceptor`
    /// instance configured with the API token from the chirpstack configuration.
    ///
    /// # Returns
    /// An `AuthInterceptor` instance with the configured API token.
    fn create_interceptor(&self) -> AuthInterceptor {
        debug!("Create interceptor");
        let interceptor = AuthInterceptor {
            api_token: self.config.chirpstack.api_token.clone(),
        };
        interceptor
    }

    /// Asynchronously creates a new ApplicationServiceClient with an interceptor.
    ///
    /// This function initializes a communication channel and attaches an authentication interceptor to it,
    /// then creates and returns an ApplicationServiceClient. In case of any error during the creation of the
    /// channel, it returns an `OpcGwError`.
    ///
    /// # Returns
    ///
    /// * `Ok(ApplicationServiceClient<InterceptedService<Channel, AuthInterceptor>>)` - On successful creation of the client.
    /// * `Err(OpcGwError)` - If there is an error while creating the channel.
    ///
    /// # Errors
    ///
    /// This function will return an error if the `self.create_channel().await` step fails.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let client = instance.create_application_client().await?;
    /// ```
    async fn create_application_client(
        &self,
    ) -> Result<ApplicationServiceClient<InterceptedService<Channel, AuthInterceptor>>, OpcGwError>
    {
        let channel = match self.create_channel().await {
            Ok(channel) => channel,
            Err(e) => {
                trace!("Error when creating channel : {:?}", e);
                return Err(e);
            }
        };
        let interceptor = self.create_interceptor();
        let application_client = ApplicationServiceClient::with_interceptor(channel, interceptor);
        Ok(application_client)
    }

    /// Asynchronously creates a client for interacting with the device service.
    ///
    /// This function initiates the creation of a DeviceServiceClient by
    /// establishing a gRPC channel and attaching an authentication interceptor.
    ///
    /// # Returns
    ///
    /// A `Result` which is:
    /// - `Ok` containing `DeviceServiceClient<InterceptedService<Channel, AuthInterceptor>>`
    ///   if the client was successfully created.
    /// - `Err(OpcGwError)` if there was an error in creating the channel or any other failure.
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The underlying `create_channel` function fails.
    /// - Any other error occurs during client creation.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let client = self.create_device_client().await?;
    /// ```
    async fn create_device_client(
        &self,
    ) -> Result<DeviceServiceClient<InterceptedService<Channel, AuthInterceptor>>, OpcGwError> {
        debug!("Create device client");
        let channel = match self.create_channel().await {
            Ok(channel) => channel,
            Err(e) => {
                trace!("Error when creating channel : {:?}", e);
                return Err(e);
            }
        };
        let interceptor = self.create_interceptor();
        let application_client = DeviceServiceClient::with_interceptor(channel, interceptor);
        Ok(application_client)
    }

    /// Checks the availability of the server by attempting to ping its IP address.
    ///
    /// This function extracts the IP address of the server and then pings it to check its availability.
    /// It handles any errors that may occur during the extraction of the IP address or the ping operation.
    ///
    /// # Returns
    ///
    /// - `Ok(())` if the server is available and the ping operation is successful.
    /// - `Err(OpcGwError)` if the IP address cannot be extracted or the ping operation fails.
    ///
    /// # Errors
    ///
    /// - Returns `OpcGwError::ChirpStackError` if the ping operation fails.
    ///
    /// # Panics
    ///
    /// - Panics if the IP address cannot be extracted from the server.
    ///
    /// # Example
    ///
    /// ```rust
    /// let opc_gw = OpcGateway::new();
    /// match opc_gw.check_server_availability() {
    ///     Ok(_) => println!("Server is available"),
    ///     Err(e) => println!("Server is not available: {:?}", e),
    /// }
    /// ```
    fn check_server_availability(&self) -> Result<Duration , OpcGwError> {
        debug!("Check server availability");
        let addr = self
            .extract_ip_address()
            .expect("Cannoit extract ip address");
        trace!("Server ip address is {:?}", addr);
        let timeout = Duration::from_secs(1);
        trace!("Ping {}", addr);
        let start = Instant::now();
        let result = ping::rawsock::ping(addr, None, None, None, None, None);
        let elapsed = start.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();
        trace!("Ping {} took {:?}", addr, elapsed);
        trace!("Ping has been sent");
        trace!("result is: {:?}", result);
        match result {
            Ok(_) => {
                let chirpstack_status = ChirpstackStatus{
                    server_available: true,
                    response_time: elapsed_secs,
                };
                return Ok(elapsed);
            }
            Err(error) => {
                let chirpstack_status = ChirpstackStatus{
                    server_available: false,
                    response_time: 0.0,
                };
                return Err(OpcGwError::ChirpStackError("Ping failed".to_string()));
            }
        }
    }

    /// Extracts the IP address from the Chirpstack server address provided in the configuration.
    ///
    /// This method attempts to parse the server address from the configuration as a URL,
    /// and extracts the host part as an IP address. If successful, it returns the extracted
    /// IP address. If the parsing fails or if the host is not an IP address, it returns an error.
    ///
    /// # Returns
    ///
    /// * `Ok(IpAddr)` - If the IP address is successfully extracted from the server address.
    /// * `Err(OpcGwError)` - If there is an error in parsing the URL or the IP address.
    fn extract_ip_address(&self) -> Result<IpAddr, OpcGwError> {
        debug!(
            "Extract ip address from {}",
            self.config.chirpstack.server_address.clone()
        );
        let server_address = self.config.chirpstack.server_address.clone();

        trace!("Parse URL for ip address");
        let url = Url::parse(&server_address).map_err(|e| {
            OpcGwError::ConfigurationError(format!("Failed to parse server address: {}", e))
        })?;

        if let Some(host_str) = url.host_str() {
            if let Ok(ip_addr) = host_str.parse::<IpAddr>() {
                trace!("Extracted ip address is: {}", ip_addr.clone());
                return Ok(ip_addr.clone());
            } else {
                return Err(OpcGwError::ConfigurationError(format!(
                    "Failed to parse IP address from host: {}",
                    host_str
                )));
            }
        } else {
            return Err(OpcGwError::ConfigurationError(
                "No host found in server address".to_string(),
            ));
        }
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
        // Define wait time
        let wait_time = Duration::from_secs(self.config.chirpstack.polling_frequency);
        // Start the poller
        loop {
            if let Err(e) = self.poll_metrics().await {
                error!(
                    "{}",
                    &OpcGwError::ChirpStackError(format!("Error polling devices: {:?}", e))
                );
            }
            // Wait for "wait_time"
            tokio::time::sleep(wait_time).await;
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

        // Get list of applications from configuration
        let app_list = self.config.application_list.clone();

        // Collect device IDs first
        let mut device_ids = Vec::new();

        // Now, parse all devices fro device id
        for app in &self.config.application_list {
            for dev in &app.device_list {
                device_ids.push(dev.device_id.clone());
            }
        }

        // Get metrics from server for each device
        for dev_id in device_ids {
            let dev_metrics = self
                .get_device_metrics_from_server(
                    dev_id.clone(),
                    self.config.chirpstack.polling_frequency,
                    1,
                )
                .await?;
            // Parse metrics received from server
            for metric in &dev_metrics.metrics.clone() {
                trace!("Got metrics:");
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
        // We are collecting only the first returned metric
        let storage = self.storage.clone();
        match self.config.get_metric_type(&metric_name, device_id) {
            Some(metric_type) => match metric_type {
                OpcMetricTypeConfig::Bool => {
                    // Convert to right boolean value
                    let mut storage = storage.lock().expect(
                        &OpcGwError::ChirpStackError(format!("Can't lock storage")).to_string(),
                    );
                    let value = metric.datasets[0].data[0].clone();
                    let mut bool_value = false;
                    match value {
                        0.0 => bool_value = false,
                        1.0 => bool_value = true,
                        _ => error!(
                            "{}",
                            OpcGwError::ChirpStackError(format!("Not a bolean value").to_string())
                        ),
                    }
                    storage.set_metric_value(device_id, &metric_name, MetricType::Bool(bool_value));
                }
                OpcMetricTypeConfig::Int => {
                    let int_value = metric.datasets[0].data[0].clone() as i64;
                    let mut storage = storage.lock().expect(
                        &OpcGwError::ChirpStackError(format!("Can't lock storage")).to_string(),
                    );
                    storage.set_metric_value(device_id, &metric_name, MetricType::Int(int_value));
                }
                OpcMetricTypeConfig::Float => {
                    let value = metric.datasets[0].data[0].clone();
                    let mut storage = storage.lock().expect(
                        &OpcGwError::ChirpStackError(format!("Can't lock storage")).to_string(),
                    );
                    storage.set_metric_value(
                        device_id,
                        &metric_name,
                        MetricType::Float(value.into()),
                    );
                }
                OpcMetricTypeConfig::String => {
                    warn!(
                        "{}",
                        OpcGwError::ChirpStackError(format!("String conversion not implemented"))
                            .to_string()
                    );
                }
                _ => {
                    warn!(
                        "{}",
                        OpcGwError::ChirpStackError(format!("Wrong metric name")).to_string()
                    );
                }
            },
            None => {
                warn!(
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
        let application_client = self.create_application_client().await?;
        trace!("Send request");
        let response = application_client
            .clone()
            //.expect("Application client is not initialized")
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
        let device_client = self.create_device_client().await?;
        trace!("Send request");
        let response = device_client
            .clone()
            //.expect("Device client is not initialized")
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

        // Check if chirpstack server is available with a ping
        trace!("Check for Chirpstack server availability");
        let retry = self.config.chirpstack.retry;
        let mut count = 0;
        let delay = Duration::from_secs(self.config.chirpstack.delay);
        loop {
            if count == retry {
                panic!("Timeout: cannot reach Chirpstack server");
            }
            match self.check_server_availability() {
                Ok(t) => break,
                _ => {
                    warn!(
                        "{}",
                        OpcGwError::ChirpStackError(format!("Waiting for Chirpstack server"))
                    );
                    trace!("Count = {}", count);
                    count += 1;
                    tokio::time::sleep(delay).await;
                }
            }
        }

        trace!("Create device service client for Chirpstack");
        let mut device_client = self.create_device_client().await.unwrap();

        trace!("Request created with: {:#?}", request);
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
            device.dev_eui,
            device.name,
            device.description
        );
    }
}

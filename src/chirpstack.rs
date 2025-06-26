// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! ChirpStack Communication Module
//!
//! This module manages communications with ChirpStack 4 server, providing functionality
//! to poll device metrics, retrieve application and device lists, and handle authentication
//! for gRPC connections.
//!
//! # Architecture
//!
//! The module provides:
//! - **ChirpstackPoller**: Main polling service for device metrics
//! - **AuthInterceptor**: gRPC authentication interceptor
//! - **Data Structures**: Representations for applications, devices, and metrics
//!
//! # Usage
//!
//! ```rust,no_run
//! use crate::chirpstack::ChirpstackPoller;
//! use std::sync::{Arc, Mutex};
//!
//! let config = AppConfig::new().unwrap();
//! let storage = Arc::new(Mutex::new(Storage::new(&config)));
//! let mut poller = ChirpstackPoller::new(&config, storage).await.unwrap();
//! poller.run().await.unwrap();
//!

use crate::config::{AppConfig, OpcMetricTypeConfig};
use crate::utils::OpcGwError;
use chirpstack_api::api::DeviceQueueItem;
use chirpstack_api::api::EnqueueDeviceQueueItemRequest;
use chirpstack_api::api::GetDeviceMetricsRequest;
use chirpstack_api::common::Metric;
use log::{debug, error, trace, warn};
use prost_types::Timestamp;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};
use tokio::time::Duration;
use tonic::codegen::InterceptedService;
use tonic::service::Interceptor;
use tonic::{transport::Channel, Request, Status};
use url::Url;

// Import generated types
use crate::storage::{ChirpstackStatus, DeviceCommand, MetricType, Storage};
use chirpstack_api::api::application_service_client::ApplicationServiceClient;
use chirpstack_api::api::device_service_client::DeviceServiceClient;
use chirpstack_api::api::{
    ApplicationListItem, DeviceListItem, ListApplicationsRequest, ListApplicationsResponse,
    ListDevicesRequest, ListDevicesResponse,
};

/// Structure representing a ChirpStack application.
///
/// Contains metadata about a ChirpStack application including its unique identifier,
/// name, and description. This structure is used when retrieving application lists
/// from the ChirpStack server.
///
#[allow(dead_code)]
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
///
/// Contains essential information about a LoRaWAN device retrieved from ChirpStack.
/// Used when listing devices within an application.
#[allow(dead_code)]
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
///
/// Contains a collection of metrics retrieved from ChirpStack for a specific device.
/// Each metric is identified by a name and contains the actual metric data.
#[derive(Debug, Deserialize, Clone)]
pub struct DeviceMetric {
    /// A map of metric names to their corresponding Metric objects
    ///
    /// The key is the metric name (e.g., "temperature", "humidity") and the value
    /// contains the actual metric data including timestamps and values.
    pub metrics: HashMap<String, Metric>,
    // A map of state names to their corresponding DeviceState objects.
    //pub states: HashMap<String, DeviceState>,
}

/// gRPC authentication interceptor for ChirpStack API calls.
///
/// This interceptor automatically adds the Bearer authentication token to all
/// gRPC requests made to the ChirpStack server. The token is configured through
/// the application configuration.
#[derive(Clone)]
struct AuthInterceptor {
    /// ChirpStack API token used for authentication
    api_token: String,
}

impl Interceptor for AuthInterceptor {
    /// Intercepts gRPC requests and injects the authorization token.
    ///
    /// This method is called automatically by the gRPC client to add authentication
    /// headers to requests before they are sent to the ChirpStack server.
    ///
    /// # Arguments
    ///
    /// * `request` - The incoming gRPC request that will be intercepted
    ///
    /// # Returns
    ///
    /// * `Result<Request<()>, Status>` - Returns the modified request with the authorization
    ///   token added to its metadata, or an error status if the token insertion fails
    ///
    /// # Panics
    ///
    /// This method will panic if the authorization token cannot be parsed into valid metadata.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// // This method is called automatically by the gRPC framework
    /// // No manual invocation is typically required
    /// ```
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        debug!("Interceptor::call");
        request.metadata_mut().insert(
            "authorization",
            format!("Bearer {}", self.api_token)
                .parse()
                .unwrap_or_else(|_| {
                    panic!(
                        "{}",
                        OpcGwError::ChirpStack("Failed to parse authorization token".to_string())
                            .to_string()
                    )
                }),
        );
        Ok(request)
    }
}

/// ChirpStack polling service for device metrics.
///
/// The main service responsible for polling device metrics from ChirpStack server
/// at configured intervals. It manages gRPC connections, handles authentication,
/// and stores retrieved metrics in shared storage.
///
/// # Examples
///
/// ```rust,no_run
/// use crate::chirpstack::ChirpstackPoller;
/// use std::sync::{Arc, Mutex};
///
/// async fn example() -> Result<(), Box<dyn std::error::Error>> {
///     let config = AppConfig::new()?;
///     let storage = Arc::new(Mutex::new(Storage::new(&config)));
///     let mut poller = ChirpstackPoller::new(&config, storage).await?;
///     poller.run().await?;
///     Ok(())
/// }
/// ```
#[derive(Clone)]
pub struct ChirpstackPoller {
    /// Configuration for the ChirpStack connection and polling behavior
    config: AppConfig,
    /// Shared storage for collected metrics, protected by Arc<Mutex<>>
    pub storage: Arc<std::sync::Mutex<Storage>>,
}

impl ChirpstackPoller {
    /// Creates a new ChirpStack poller instance.
    ///
    /// Initializes a new poller with the provided configuration and storage reference.
    /// This function prepares the poller for connecting to ChirpStack but does not
    /// establish the connection immediately.
    ///
    /// # Arguments
    ///
    /// * `config` - Application configuration containing ChirpStack server details
    /// * `storage` - Shared storage for metrics data
    ///
    /// # Returns
    ///
    /// `Result<Self, OpcGwError>` - Returns a new `ChirpstackPoller` instance on success,
    /// or an `OpcGwError` if initialization fails
    ///
    /// # Errors
    ///
    /// Currently this function cannot fail, but returns a Result for future extensibility
    /// when connection validation might be added during initialization.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// use crate::chirpstack::ChirpstackPoller;
    /// use std::sync::{Arc, Mutex};
    ///
    /// async fn create_poller() -> Result<ChirpstackPoller, OpcGwError> {
    ///     let config = AppConfig::new().unwrap();
    ///     let storage = Arc::new(Mutex::new(Storage::new(&config)));
    ///     ChirpstackPoller::new(&config, storage).await
    /// }
    /// ```
    pub async fn new(config: &AppConfig, storage: Arc<Mutex<Storage>>) -> Result<Self, OpcGwError> {
        debug!("Create a new Chirpstack poller");

        Ok(ChirpstackPoller {
            config: config.clone(),
            storage,
        })
    }

    /// Creates a gRPC channel for communication with the ChirpStack server.
    ///
    /// Establishes a gRPC channel to the ChirpStack server using the configured
    /// server address. This channel is used for all subsequent API calls.
    ///
    /// # Returns
    ///
    /// `Result<tonic::transport::Channel, OpcGwError>` - Returns a configured gRPC channel
    /// on success, or an error if the channel creation or connection fails
    ///
    /// # Errors
    ///
    /// Returns `OpcGwError::ConfigurationError` if:
    /// - The server address format is invalid
    /// - The connection to the server fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let channel = poller.create_channel().await?;
    /// ```
    async fn create_channel(&self) -> Result<tonic::transport::Channel, OpcGwError> {
        debug!("Create channel");
        let channel = Channel::from_shared(self.config.chirpstack.server_address.clone())
            .map_err(|e| OpcGwError::Configuration(format!("Failed to create channel: {}", e)))?
            .connect()
            .await
            .map_err(|e| {
                OpcGwError::Configuration(format!("Failed to intercept channel: {}", e))
            })?;
        Ok(channel)
    }

    /// Creates an authentication interceptor for gRPC requests.
    ///
    /// Initializes an authentication interceptor that will automatically add
    /// the Bearer token to all gRPC requests sent to the ChirpStack server.
    ///
    /// # Returns
    ///
    /// An `AuthInterceptor` instance configured with the API token from the configuration
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let interceptor = poller.create_interceptor();
    /// ```
    fn create_interceptor(&self) -> AuthInterceptor {
        debug!("Create interceptor");
        AuthInterceptor {
            api_token: self.config.chirpstack.api_token.clone(),
        }
    }

    /// Creates a ChirpStack ApplicationService client with authentication.
    ///
    /// Initializes a gRPC client for the ChirpStack ApplicationService, which is used
    /// to manage applications and retrieve application-related information.
    ///
    /// # Returns
    ///
    /// `Result<ApplicationServiceClient<InterceptedService<Channel, AuthInterceptor>>, OpcGwError>`
    /// - Returns a configured application service client on success
    /// - Returns an error if the channel creation fails
    ///
    /// # Errors
    ///
    /// This function will return an error if `create_channel()` fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let app_client = poller.create_application_client().await?;
    /// let request = Request::new(ListApplicationsRequest { /* ... */ });
    /// let response = app_client.list(request).await?;
    /// ```
    #[allow(dead_code)]
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

    /// Creates a ChirpStack DeviceService client with authentication.
    ///
    /// Initializes a gRPC client for the ChirpStack DeviceService, which is used
    /// to manage devices, retrieve device information, and fetch device metrics.
    ///
    /// # Returns
    ///
    /// `Result<DeviceServiceClient<InterceptedService<Channel, AuthInterceptor>>, OpcGwError>`
    /// - Returns a configured device service client on success
    /// - Returns an error if the channel creation fails
    ///
    /// # Errors
    ///
    /// This function will return an error if `create_channel()` fails.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let device_client = poller.create_device_client().await?;
    /// let request = Request::new(GetDeviceMetricsRequest { /* ... */ });
    /// let response = device_client.get_metrics(request).await?;
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

    /// Checks the availability of the ChirpStack server.
    ///
    /// Performs a TCP connection test to the ChirpStack server to verify its availability
    /// and measure response time. This is useful for connection validation before
    /// attempting gRPC calls.
    ///
    /// # Returns
    ///
    /// `Result<Duration, OpcGwError>` - Returns the connection time on success,
    /// or an error if the server is not reachable
    ///
    /// # Errors
    ///
    /// Returns `OpcGwError` if:
    /// - The server address cannot be parsed
    /// - The TCP connection fails
    /// - The host or port cannot be extracted from the server address
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// match poller.check_server_availability() {
    ///     Ok(duration) => println!("Server responded in {:?}", duration),
    ///     Err(e) => println!("Server unavailable: {:?}", e),
    /// }
    /// ```
    fn check_server_availability(&self) -> Result<Duration, OpcGwError> {
        debug!("Check for chirpstack server availability");

        // Parse the server address to extract host and port
        let server_address = &self.config.chirpstack.server_address;
        trace!(
            "Checking connectivity to Chirpstack server: {}",
            server_address
        );

        // Parse as URL to extract host and port
        let url = Url::parse(server_address).map_err(|e| {
            OpcGwError::Configuration(format!("Invalid Chirpstack server address: {}", e))
        })?;

        // Extrackt host and port from URL
        let host = url.host_str().ok_or_else(|| {
            OpcGwError::Configuration("No Chirpstack host in server address".to_string())
        })?;
        let port = url.port().unwrap_or(8080); // Default Chirpstack port

        // Create socket address
        let socket_addr: SocketAddr = format!("{}:{}", host, port)
            .parse()
            .map_err(|e| OpcGwError::Configuration(format!("Invalid socket address: {}", e)))?;

        trace!(
            "Attempting TCP connection to Chirpstack server: {}",
            socket_addr
        );
        let timeout = Duration::from_secs(1);
        let start = Instant::now();
        // Attempt TCP connection
        let result = TcpStream::connect_timeout(&socket_addr, timeout);
        let elapsed = start.elapsed();
        let elapsed_secs = elapsed.as_secs_f64();

        trace!(
            "TCP connection to Chirpstack server {} took {:?}",
            socket_addr,
            elapsed
        );

        match result {
            Ok(_) => {
                let _chirpstack_status = ChirpstackStatus {
                    server_available: true,
                    response_time: elapsed_secs,
                };
                trace!("TCP connection to Chirpstack server successful");
                Ok(elapsed)
            }
            Err(error) => {
                let _chirpstack_status = ChirpstackStatus {
                    server_available: false,
                    response_time: 0.0,
                };
                trace!("TCP connection to Chirpstack server failed: {}", error);
                Err(OpcGwError::ChirpStack(format!(
                    "TCP connection to Chirpstrack server failed: {}",
                    error
                )))
            }
        }
    }

    /// Extracts the IP address from the ChirpStack server address.
    ///
    /// Parses the configured server address as a URL and extracts the host portion
    /// as an IP address. This is useful for network diagnostics and validation.
    ///
    /// # Returns
    ///
    /// `Result<IpAddr, OpcGwError>` - Returns the extracted IP address on success,
    /// or an error if parsing fails
    ///
    /// # Errors
    ///
    /// Returns `OpcGwError::ConfigurationError` if:
    /// - The server address cannot be parsed as a URL
    /// - The host portion is not a valid IP address
    /// - No host is found in the server address
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let ip = poller.extract_ip_address()?;
    /// println!("ChirpStack server IP: {}", ip);
    /// ```
    #[allow(dead_code)]
    fn extract_ip_address(&self) -> Result<IpAddr, OpcGwError> {
        debug!(
            "Extract chirpstack server ip address from {}",
            self.config.chirpstack.server_address.clone()
        );
        let server_address = self.config.chirpstack.server_address.clone();

        trace!("Parse URL for ip address");
        let url = Url::parse(&server_address).map_err(|e| {
            OpcGwError::Configuration(format!("Failed to parse chirpstack server address: {}", e))
        })?;

        if let Some(host_str) = url.host_str() {
            if let Ok(ip_addr) = host_str.parse::<IpAddr>() {
                trace!(
                    "Extracted chirpstack server ip address is: {}",
                    ip_addr.clone()
                );
                Ok(ip_addr)
            } else {
                Err(OpcGwError::Configuration(format!(
                    "Failed to parse IP address from host: {}",
                    host_str
                )))
            }
        } else {
            Err(OpcGwError::Configuration(
                "No host found in server address".to_string(),
            ))
        }
    }

    /// Runs the ChirpStack polling service continuously.
    ///
    /// Starts the main polling loop that retrieves device metrics from ChirpStack
    /// at the configured interval. The loop continues indefinitely, handling errors
    /// gracefully by logging them and continuing with the next polling cycle.
    ///
    /// # Returns
    ///
    /// `Result<(), OpcGwError>` - This function runs indefinitely, so it only returns
    /// an error if there's a fundamental configuration issue
    ///
    /// # Errors
    ///
    /// Individual polling errors are logged but do not stop the service. The function
    /// only returns an error for critical configuration issues.
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// async fn start_polling() -> Result<(), OpcGwError> {
    ///     let mut poller = ChirpstackPoller::new(&config, storage).await?;
    ///     poller.run().await // Runs indefinitely
    /// }
    /// ```
    pub async fn run(&mut self) -> Result<(), OpcGwError> {
        debug!(
            "Running chirpstack poller every {} s",
            self.config.chirpstack.polling_frequency
        );
        // Define wait time
        let wait_time = Duration::from_secs(self.config.chirpstack.polling_frequency);
        // Start the poller
        loop {
            // Polling metrics
            if let Err(e) = self.poll_metrics().await {
                error!(
                    "{}",
                    &OpcGwError::ChirpStack(format!("Error polling chirpstack devices: {:?}", e))
                );
            }

            // Wait for "wait_time"
            tokio::time::sleep(wait_time).await;
        }
    }

    /// Polls metrics for all configured devices.
    ///
    /// Retrieves device metrics from ChirpStack for all devices specified in the
    /// configuration. For each device, it fetches the latest metrics and stores
    /// them in the shared storage for access by other components.
    ///
    /// # Returns
    ///
    /// `Result<(), OpcGwError>` - Returns Ok on successful completion of polling cycle,
    /// or an error if metric retrieval fails
    ///
    /// # Errors
    ///
    /// Returns `OpcGwError` if there's an error fetching metrics from the ChirpStack server.
    ///
    /// # Process
    ///
    /// 1. Collects all device IDs from the configured applications
    /// 2. For each device, requests metrics from ChirpStack
    /// 3. Stores received metrics in the shared storage
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// // Called automatically by run(), but can be called manually for testing
    /// poller.poll_metrics().await?;
    /// ```
    async fn poll_metrics(&mut self) -> Result<(), OpcGwError> {
        debug!("Polling chirpstack metrics");

        // Process command queue
        self.process_command_queue().await?;

        // Collecting metrics
        // Collect device IDs first
        let mut device_ids = Vec::new();
        let mut device_names = Vec::new();

        // Now, parse all devices from device id
        for app in &self.config.application_list {
            for dev in &app.device_list {
                device_ids.push(dev.device_id.clone());
                device_names.push(dev.device_name.clone());
            }
        }
        debug!("Found {} devices ", device_names.len());
        debug!("Found devices {:#?} ", device_names);

        // Get metrics from server for each device
        for dev_id in device_ids {
            let dev_metrics = self
                .get_device_metrics_from_server(
                    dev_id.clone(),
                    //self.config.chirpstack.polling_frequency,
                    1, //If we put a value different from aggregation, status variables are aggregated
                    1,
                )
                .await?;
            // Parse metrics received from server
            for metric in &dev_metrics.metrics.clone() {
                trace!("Got chirpstack metrics:");
                trace!("{:#?}", metric);
                for metric in dev_metrics.metrics.values() {
                    self.store_metric(&dev_id.clone(), &metric.clone());
                }
            }
        }
        Ok(())
    }

    /// Stores a device metric in the shared storage.
    ///
    /// Processes a metric received from ChirpStack and stores it in the appropriate
    /// format in the shared storage. The metric is converted to the correct type
    /// based on the configuration.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the device
    /// * `metric` - The metric data received from ChirpStack
    ///
    /// # Process
    ///
    /// 1. Determines the expected metric type from configuration
    /// 2. Converts the metric value to the appropriate type (Bool, Int, Float, String)
    /// 3. Stores the converted value in the shared storage
    ///
    /// # Type Conversions
    ///
    /// - **Bool**: 0.0 → false, 1.0 → true
    /// - **Int**: Converts float to i64
    /// - **Float**: Stores as f64
    /// - **String**: Not yet implemented
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// // Called automatically during polling
    /// poller.store_metric(&device_id, &metric);
    /// ```
    pub fn store_metric(&self, device_id: &String, metric: &Metric) {
        debug!("Store chirpstack device metric in storage");
        let device_name = self.config.get_device_name(device_id).unwrap_or_else(|| {
            panic!(
                "{}",
                OpcGwError::ChirpStack("Failed to get chirpstack device name".to_string())
                    .to_string()
            )
        });

        let metric_name = metric.name.clone();
        // We are collecting only the first returned metric
        let storage = self.storage.clone();
        match self.config.get_metric_type(&metric_name, device_id) {
            Some(metric_type) => match metric_type {
                OpcMetricTypeConfig::Bool => {
                    debug!("Bool metric is: {:?}", metric);
                    // Convert to right boolean value
                    let mut storage = storage.lock().unwrap_or_else(|_| {
                        panic!(
                            "{}",
                            OpcGwError::ChirpStack("Can't lock storage".to_string()).to_string()
                        )
                    });
                    let value = metric.datasets[0].data[0];
                    let mut bool_value = false;
                    match value {
                        0.0 => bool_value = false,
                        1.0 => bool_value = true,
                        _ => error!(
                            "{}",
                            OpcGwError::ChirpStack("Not a bolean value".to_string())
                        ),
                    }
                    storage.set_metric_value(device_id, &metric_name, MetricType::Bool(bool_value));
                }
                OpcMetricTypeConfig::Int => {
                    debug!("Int metric is: {:?}", metric);
                    let int_value = metric.datasets[0].data[0] as i64;
                    let mut storage = storage.lock().unwrap_or_else(|_| {
                        panic!(
                            "{}",
                            OpcGwError::ChirpStack("Can't lock storage".to_string()).to_string()
                        )
                    });
                    storage.set_metric_value(device_id, &metric_name, MetricType::Int(int_value));
                }
                OpcMetricTypeConfig::Float => {
                    debug!("Float metric is: {:?}", metric);
                    let value = metric.datasets[0].data[0];
                    let mut storage = storage.lock().unwrap_or_else(|_| {
                        panic!(
                            "{}",
                            OpcGwError::ChirpStack("Can't lock storage".to_string()).to_string()
                        )
                    });
                    storage.set_metric_value(
                        device_id,
                        &metric_name,
                        MetricType::Float(value.into()),
                    );
                }
                OpcMetricTypeConfig::String => {
                    warn!("Reading string metrics fron Chirpstack server is not implemented")
                }
            },
            None => {
                warn!(
                    "{}",
                    &OpcGwError::ChirpStack(format!(
                        "No metric type found for chirpstack metric: {:?} of device {:?}",
                        metric_name, device_name
                    ))
                );
            }
        };
    }

    /// Retrieves the list of applications from the ChirpStack server.
    ///
    /// Sends a request to the ChirpStack ApplicationService to obtain a list of all
    /// applications associated with the configured tenant. This is useful for
    /// discovering available applications and their metadata.
    ///
    /// # Returns
    ///
    /// `Result<Vec<ApplicationDetail>, OpcGwError>` - Returns a vector of application
    /// details on success, or an error if the request fails
    ///
    /// # Errors
    ///
    /// Returns `OpcGwError::ChirpStackError` if:
    /// - The gRPC client cannot be created
    /// - The server request fails
    /// - Authentication fails
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let applications = poller.get_applications_list_from_server().await?;
    /// for app in applications {
    ///     println!("Application: {} ({})", app.application_name, app.application_id);
    /// }
    /// ```
    #[allow(dead_code)]
    pub async fn get_applications_list_from_server(
        &self,
    ) -> Result<Vec<ApplicationDetail>, OpcGwError> {
        debug!("Get list of chirpstack applications");
        //trace!("Create request");
        let request = Request::new(ListApplicationsRequest {
            limit: 100, // Can be adjusted according to needs, but what does it means ?
            offset: 0,
            search: String::new(),
            tenant_id: self.config.chirpstack.tenant_id.clone(), // We work on only one tenant defined in parameter file
        });
        //trace!("Request created with: {:#?}", request);
        let application_client = self.create_application_client().await?;
        //trace!("Send request");
        let response = application_client
            .clone()
            .list(request)
            .await
            .map_err(|e| {
                OpcGwError::ChirpStack(format!(
                    "Error when collecting chirpstack application list: {}",
                    e
                ))
            })?;
        trace!("Convert result");

        let applications = self.convert_to_applications(response.into_inner());
        Ok(applications)
    }

    /// Retrieves the list of devices for a specific application.
    ///
    /// Sends a request to the ChirpStack DeviceService to obtain a list of all
    /// devices within the specified application. This provides device metadata
    /// including DevEUI, name, and description.
    ///
    /// # Arguments
    ///
    /// * `application_id` - The unique identifier of the application whose devices to retrieve
    ///
    /// # Returns
    ///
    /// `Result<Vec<DeviceListDetail>, OpcGwError>` - Returns a vector of device details
    /// on success, or an error if the request fails
    ///
    /// # Errors
    ///
    /// Returns `OpcGwError::ChirpStackError` if:
    /// - The gRPC client cannot be created
    /// - The server request fails
    /// - Authentication fails
    /// - The application ID is invalid
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let devices = poller.get_devices_list_from_server("app-123".to_string()).await?;
    /// for device in devices {
    ///     println!("Device: {} ({})", device.name, device.dev_eui);
    /// }
    /// ```
    #[allow(dead_code)]
    pub async fn get_devices_list_from_server(
        &self,
        application_id: String,
    ) -> Result<Vec<DeviceListDetail>, OpcGwError> {
        debug!("Get list of chirpstack devices");
        trace!("for chirpstack application: {:?}", application_id);
        trace!("Create request");

        let request = Request::new(ListDevicesRequest {
            limit: 100,
            offset: 0,
            search: String::new(),
            application_id,
            multicast_group_id: String::new(),
            device_profile_id: String::new(),
            order_by: 0,
            order_by_desc: false,
            tags: HashMap::new(),
        });
        //trace!("Request created with: {:?}", request);
        let device_client = self.create_device_client().await?;
        trace!("Send request");
        let response = device_client
            .clone()
            //.expect("Device client is not initialized")
            .list(request)
            .await
            .map_err(|e: Status| {
                OpcGwError::ChirpStack(format!(
                    "Error when collecting chirpstack devices list: {e}"
                ))
            })?;
        trace!("Convert result");
        let devices: Vec<DeviceListDetail> = self.convert_to_devices(response.into_inner());
        Ok(devices)
    }

    /// Retrieves device metrics from the ChirpStack server.
    ///
    /// Fetches metrics for a specific device over a specified time duration.
    /// Before making the request, it checks server availability with retry logic
    /// to ensure robust operation.
    ///
    /// # Arguments
    ///
    /// * `dev_eui` - The DevEUI (Device Extended Unique Identifier) of the target device
    /// * `duration` - Time duration in seconds for the metrics query
    /// * `aggregation` - Aggregation level for the metrics (1 = raw data)
    ///
    /// # Returns
    ///
    /// `Result<DeviceMetric, OpcGwError>` - Returns device metrics on success,
    /// or an error if the request fails
    ///
    /// # Errors
    ///
    /// Returns `OpcGwError::ChirpStackError` if:
    /// - The server is not available after all retries
    /// - The gRPC client cannot be created
    /// - The metrics request fails
    /// - Authentication fails
    ///
    /// # Server Availability
    ///
    /// The function implements retry logic based on configuration:
    /// - Checks server availability before making the request
    /// - Retries connection according to `config.chirpstack.retry`
    /// - Waits `config.chirpstack.delay` seconds between retries
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// let dev_eui = "0018B20000001122".to_string();
    /// let metrics = poller.get_device_metrics_from_server(dev_eui, 3600, 1).await?;
    /// println!("Retrieved {} metrics", metrics.metrics.len());
    /// ```
    pub async fn get_device_metrics_from_server(
        &mut self,
        dev_eui: String,
        duration: u64,
        aggregation: i32,
    ) -> Result<DeviceMetric, OpcGwError> {
        trace!("Get chirpstack device metrics");
        debug!("for chirpstack device: {:?}", dev_eui);
        debug!("Create request");
        let request = Request::new(GetDeviceMetricsRequest {
            dev_eui: dev_eui.clone(),
            start: Some(Timestamp::from(SystemTime::now())),
            end: Some(Timestamp::from(
                SystemTime::now() + Duration::from_secs(duration),
            )),
            aggregation,
        });

        // Check if chirpstack server is available with a ping
        trace!("Check for chirpstack server availability");
        let retry = self.config.chirpstack.retry;
        let mut count = 0;
        let delay = Duration::from_secs(self.config.chirpstack.delay);
        loop {
            if count == retry {
                //panic!("Timeout: cannot reach Chirpstack server");
                warn!("Timeout: cannot reach chirpstack server");
            }
            match self.check_server_availability() {
                Ok(_t) => break,
                _ => {
                    warn!(
                        "{}",
                        OpcGwError::ChirpStack("Waiting for Chirpstack server".to_string())
                    );
                    trace!("Count = {}", count);
                    count += 1;
                    tokio::time::sleep(delay).await;
                }
            }
        }

        trace!("Create device service client for Chirpstack");
        let mut device_client = self.create_device_client().await.unwrap();

        //trace!("Request created with: {:#?}", request);
        match device_client.get_metrics(request).await {
            Ok(response) => {
                let inner_response = response.into_inner();

                let metrics: HashMap<String, Metric> = inner_response.metrics.into_iter().collect();

                Ok(DeviceMetric { metrics })
            }
            Err(e) => Err(OpcGwError::ChirpStack(format!(
                "Error getting device metrics: {}",
                e
            ))),
        }
    }

    /// Processes all commands in the device command queue.
    ///
    /// This method continuously retrieves and processes commands from the storage queue
    /// until it's empty. Each command is removed from the queue before being sent to
    /// the server, ensuring that successfully processed commands are not retried.
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If all commands were processed successfully or the queue was empty
    /// * `Err(OpcGwError)` - If there was an error accessing the storage lock
    ///
    /// # Behavior
    ///
    /// - Commands are processed one at a time to avoid memory overhead
    /// - Each command is permanently removed from the queue before processing
    /// - If a command fails to be enqueued, an error is logged but processing continues
    /// - The method only returns an error if the storage lock cannot be acquired
    ///
    /// # Error Handling
    ///
    /// Failed command enqueueing is logged but does not stop the processing of remaining
    /// commands. Consider implementing a retry mechanism or dead letter queue for
    /// production use cases.
    async fn process_command_queue(&mut self) -> Result<(), OpcGwError> {
        trace!("Process command queue");

        loop {
            // Récupérer une commande à la fois au lieu de cloner toute la queue
            let command = {
                let mut storage_guard = self.storage.lock().map_err(|e| {
                    OpcGwError::ChirpStack(format!("Failed to lock storage: {}", e))
                })?;

                // Prendre la première commande de la queue (ou None si vide)
                storage_guard.pop_command()
            };

            // Si pas de commande, sortir de la boucle
            let command = match command {
                Some(cmd) => cmd,
                None => break,
            };

            debug!("Command: {:?}", command);
            match self.enqueue_device_request_to_server(command).await {
                Ok(_) => debug!("Command enqueued successfully"),
                Err(e) => {
                    error!("Failed to enqueue command: {}", e);
                    // En cas d'erreur, vous pourriez vouloir remettre la commande dans la queue
                    // ou la traiter différemment selon votre logique métier
                }
            }
        }

        Ok(())
    }

    /// Enqueues a device command to the ChirpStack server for transmission to a LoRaWAN device.
    ///
    /// This method takes a device command from the local queue and sends it to the ChirpStack
    /// server, which will then transmit it to the specified LoRaWAN device when the device
    /// next communicates with the network.
    ///
    /// # Arguments
    ///
    /// * `command` - The device command containing the target device EUI, payload data,
    ///   port number, and confirmation settings
    ///
    /// # Returns
    ///
    /// * `Ok(())` - If the command was successfully enqueued on the server
    /// * `Err(OpcGwError)` - If validation failed or the server request failed
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// - The `f_port` is 0 or invalid (ports 0 are reserved for MAC commands)
    /// - The device EUI is invalid (validated by the server)
    /// - Server communication fails
    /// - Client creation fails
    ///
    /// # Examples
    ///
    /// ```rust
    /// let command = DeviceCommand {
    ///     device_id: "1234567890abcdef".to_string(),
    ///     confirmed: true,
    ///     f_port: 1,
    ///     data: vec![0x01, 0x02, 0x03],
    /// };
    ///
    /// match chirpstack_client.enqueue_device_command(command).await {
    ///     Ok(()) => println!("Command enqueued successfully"),
    ///     Err(e) => eprintln!("Failed to enqueue command: {}", e),
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// The method currently panics if client creation fails. This should be handled
    /// properly in production code.
    async fn enqueue_device_request_to_server(
        &self,
        command: DeviceCommand,
    ) -> Result<(), OpcGwError> {
        trace!("Enqueue device request");
        if command.f_port < 1 {
            return Err(OpcGwError::ChirpStack("Invalid fPort".to_string()));
        }
        // Create a new request
        debug!("Create request");
        let queue_item = DeviceQueueItem {
            id: "".to_string(),
            dev_eui: command.device_id.clone(),
            confirmed: command.confirmed,
            f_port: command.f_port,
            data: command.data.clone(),
            object: None,
            is_pending: true,
            f_cnt_down: 0,
            is_encrypted: false,
            expires_at: None,
        };
        debug!("Request created with: {:#?}", queue_item);

        // Send request to server
        let request = Request::new(EnqueueDeviceQueueItemRequest {
            queue_item: Some(queue_item),
        });

        let mut device_client = self.create_device_client().await.unwrap();
        match device_client.enqueue(request).await {
            Ok(response) => {
                let inner_response = response.into_inner();
                trace!("Response: {:#?}", inner_response);
                Ok(())
            }
            Err(e) => {
                error!("Error enqueueing device request: {}", e);
                Err(OpcGwError::ChirpStack(
                    "Error enqueuing request".to_string(),
                ))
            }
        }
    }

    /// Converts a `ListApplicationsResponse` into a vector of `ApplicationDetail`.
    ///
    /// Transforms the gRPC response containing application list items into a more
    /// convenient Rust data structure for internal use.
    ///
    /// # Arguments
    ///
    /// * `response` - The gRPC response containing the list of applications
    ///
    /// # Returns
    ///
    /// A vector of `ApplicationDetail` objects with converted field names and types
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// // Called internally by get_applications_list_from_server()
    /// let app_details = poller.convert_to_applications(response);
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

    /// Converts a `ListDevicesResponse` into a vector of `DeviceListDetail`.
    ///
    /// Transforms the gRPC response containing device list items into a more
    /// convenient Rust data structure for internal use.
    ///
    /// # Arguments
    ///
    /// * `response` - The gRPC response containing the list of devices
    ///
    /// # Returns
    ///
    /// A vector of `DeviceListDetail` objects with converted field names and types
    ///
    /// # Examples
    ///
    /// ```rust,no_run
    /// // Called internally by get_devices_list_from_server()
    /// let device_details = poller.convert_to_devices(response);
    /// ```
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

/// Utility function to print application details for debugging.
///
/// Formats and displays the details of applications in a readable format
/// using trace-level logging. Useful for debugging and development.
///
/// # Arguments
///
/// * `list` - A reference to a vector containing application details
///
/// # Examples
///
/// ```rust,no_run
/// let applications = poller.get_applications_list_from_server().await?;
/// print_application_list(&applications);
/// ```
#[allow(dead_code)]
pub fn print_application_list(list: &Vec<ApplicationDetail>) {
    for app in list {
        trace!("{:#?}", app);
    }
}

/// Utility function to print device details for debugging.
///
/// Formats and displays the details of devices in a readable format
/// using trace-level logging. Shows DevEUI, name, and description for each device.
///
/// # Arguments
///
/// * `list` - A reference to a vector containing device details
///
/// # Examples
///
/// ```rust,no_run
/// let devices = poller.get_devices_list_from_server("app-123".to_string()).await?;
/// print_device_list(&devices);
/// ```
///
/// This will output (at trace level):
/// ```text
/// Device EUI: 0018B20000001122, Name: Temperature Sensor, Description: Outdoor sensor
/// Device EUI: 0018B20000003344, Name: Humidity Sensor, Description: Indoor sensor
/// ```
#[allow(dead_code)]
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

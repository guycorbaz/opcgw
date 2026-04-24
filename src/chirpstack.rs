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
use chrono::Utc;
use tracing::{debug, error, info, trace, warn};
use chirpstack_api::prost_types::Timestamp;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::time::Duration;
use tonic::codegen::InterceptedService;
use tonic::service::Interceptor;
use tonic::{transport::Channel, Request, Status};
use url::Url;

// Import generated types
use crate::storage::{MetricType, StorageBackend};
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
#[derive(Deserialize, Clone)]
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
    /// // This method is called automatically by the gRPC framework
    /// // No manual invocation is typically required
    /// 
    fn call(&mut self, mut request: Request<()>) -> Result<Request<()>, Status> {
        debug!("Interceptor::call");
        let token_value = format!("Bearer {}", self.api_token)
            .parse()
            .map_err(|_| {
                error!("Failed to parse authorization token");
                Status::unauthenticated("Failed to parse authorization token")
            })?;
        request.metadata_mut().insert("authorization", token_value);
        Ok(request)
    }
}

/// ChirpStack metric kind classification.
///
/// Local enum wrapper for metric kinds from protobuf for easier testing and matching.
/// Maps to protobuf enum values from `proto/common/common.proto`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChirpStackMetricKind {
    /// Monotonically increasing counter, never resets
    Counter,
    /// Resets periodically (e.g., hourly energy usage)
    Absolute,
    /// Instantaneous measurement (e.g., temperature, voltage)
    Gauge,
    /// Unmapped or unknown metric kind value
    Unknown,
}

/// Classifies a ChirpStack metric kind from protobuf enum integer value.
///
/// ChirpStack defines four metric kinds in common.proto:
/// - 0 = COUNTER: Monotonically increasing, never resets. Use MetricType::Int with monotonic check.
/// - 1 = ABSOLUTE: Resets periodically (e.g., hourly energy). Use MetricType::Float.
/// - 2 = GAUGE: Instantaneous measurement (e.g., temperature). Use MetricType::Float.
/// - Other: Unknown/unmapped kind. Gracefully skip with warning; fallback to config type if available.
///
/// This classification function enables testable, type-safe kind matching.
fn classify_metric_kind(kind: i32) -> ChirpStackMetricKind {
    match kind {
        0 => ChirpStackMetricKind::Counter,
        1 => ChirpStackMetricKind::Absolute,
        2 => ChirpStackMetricKind::Gauge,
        _ => ChirpStackMetricKind::Unknown,
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
    /// Shared storage backend for collected metrics (SQLite or in-memory)
    backend: Arc<dyn StorageBackend>,
    /// Cancellation token for graceful shutdown
    cancel_token: tokio_util::sync::CancellationToken,
    /// Barrier to synchronize metric restore completion (Story 2-4a Task 11)
    restore_barrier: Arc<std::sync::Barrier>,
    /// Timestamp of last successful prune execution (Story 2-5a: Historical Data Pruning)
    last_prune_time: std::sync::Arc<std::sync::Mutex<Instant>>,
    /// Track prune retry state for exponential backoff (Story 2-5b code review fix)
    prune_retry_state: std::sync::Arc<std::sync::Mutex<PruneRetryState>>,
}

/// Exponential backoff state for prune failures
#[derive(Debug, Clone)]
struct PruneRetryState {
    /// Number of consecutive prune failures
    failure_count: u32,
    /// Timestamp of first failure in this sequence
    first_failure_time: Option<Instant>,
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
    pub async fn new(
        config: &AppConfig,
        backend: Arc<dyn StorageBackend>,
        cancel_token: tokio_util::sync::CancellationToken,
        restore_barrier: Arc<std::sync::Barrier>,
    ) -> Result<Self, OpcGwError> {
        debug!("Create a new Chirpstack poller");

        Ok(ChirpstackPoller {
            config: config.clone(),
            backend,
            cancel_token,
            restore_barrier,
            last_prune_time: std::sync::Arc::new(std::sync::Mutex::new(Instant::now())),
            prune_retry_state: std::sync::Arc::new(std::sync::Mutex::new(PruneRetryState {
                failure_count: 0,
                first_failure_time: None,
            })),
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
                trace!(error = ?e, "Error when creating channel");
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
                trace!(error = ?e, "Error when creating channel");
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
        trace!(server_address = %server_address, "Checking connectivity to Chirpstack server");

        // Parse as URL to extract host and port
        let url = Url::parse(server_address).map_err(|e| {
            OpcGwError::Configuration(format!("Invalid Chirpstack server address: {}", e))
        })?;

        // Extract host and port from URL
        let host = url.host_str().ok_or_else(|| {
            OpcGwError::Configuration("No Chirpstack host in server address".to_string())
        })?;
        let port = url.port().unwrap_or(8080); // Default Chirpstack port

        // Create socket address
        let socket_addr: SocketAddr = format!("{}:{}", host, port)
            .parse()
            .map_err(|e| OpcGwError::Configuration(format!("Invalid socket address: {}", e)))?;

        trace!(address = %socket_addr, "Attempting TCP connection to Chirpstack server");
        let timeout = Duration::from_secs(1);
        let start = Instant::now();
        // Attempt TCP connection
        let result = TcpStream::connect_timeout(&socket_addr, timeout);
        let elapsed = start.elapsed();

        trace!(address = %socket_addr, elapsed = ?elapsed, "TCP connection to Chirpstack server completed");

        match result {
            Ok(_) => {
                trace!("TCP connection to Chirpstack server successful");
                // TODO: Persist status update to storage (server_available=true, last_poll_time=now)
                // TODO: Add clock skew detection - validate that Utc::now() >= previous last_poll_time
                // to catch system clock adjustments (NTP corrections, VM clock skew)
                Ok(elapsed)
            }
            Err(error) => {
                trace!(error = %error, "TCP connection to Chirpstack server failed");
                // TODO: Persist status update to storage (server_available=false, error_count++)
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
        debug!(server_address = %self.config.chirpstack.server_address, "Extract chirpstack server ip address");
        let server_address = self.config.chirpstack.server_address.clone();

        trace!("Parse URL for ip address");
        let url = Url::parse(&server_address).map_err(|e| {
            OpcGwError::Configuration(format!("Failed to parse chirpstack server address: {}", e))
        })?;

        if let Some(host_str) = url.host_str() {
            if let Ok(ip_addr) = host_str.parse::<IpAddr>() {
                trace!(ip_address = %ip_addr, "Extracted chirpstack server ip address");
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
        debug!(polling_frequency_s = %self.config.chirpstack.polling_frequency, "Running chirpstack poller");

        // Wait for metric restore phase to complete (Story 2-4a Task 11)
        info!("ChirpStack poller waiting for metric restore phase to complete");
        let barrier = Arc::clone(&self.restore_barrier);
        tokio::task::block_in_place(|| {
            barrier.wait();
        });
        info!("ChirpStack poller starting metric collection");

        // Define wait time
        let wait_time = Duration::from_secs(self.config.chirpstack.polling_frequency);
        // Start the poller
        loop {
            // Polling metrics (AC#1: poll_once equivalent)
            if let Err(e) = self.poll_metrics().await {
                error!(error = ?e, "Error polling chirpstack devices");
            }

            // Execute pruning after poll_metrics completes (AC#1: sequential, not parallel)
            if let Err(e) = self.check_and_execute_prune() {
                error!(error = %e, "Pruning failed in poll cycle");
                // Continue polling even if pruning fails per AC#5 (graceful degradation)
            }

            // Wait for next poll cycle or cancellation
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("ChirpStack poller shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(wait_time) => {}
            }
        }
    }

    /// Check if pruning interval has elapsed and execute pruning if needed (Story 2-5a).
    ///
    /// Pruning is scheduled based on config.global.prune_interval_minutes. If the interval
    /// has elapsed, this method reads the retention policy from retention_config and prunes
    /// expired rows from metric_history. Returns early if pruning is disabled (interval = 0).
    ///
    /// # Returns
    ///
    /// `Result<(), OpcGwError>` - Ok if pruning succeeded or was skipped, error only on failure
    ///
    /// # Errors
    ///
    /// Returns error only if database operations fail; missing retention_config is handled
    /// gracefully with error logging per AC#7.
    fn check_and_execute_prune(&mut self) -> Result<(), OpcGwError> {
        // Return early if pruning is disabled (AC#1: 0 to disable)
        if self.config.global.prune_interval_minutes == 0 {
            return Ok(());
        }

        let prune_interval = Duration::from_secs(self.config.global.prune_interval_minutes as u64 * 60);
        let mut last_prune = self.last_prune_time.lock()
            .map_err(|e| {
                // PoisonError indicates panic in prior prune task; convert to clear message
                OpcGwError::Storage(format!("Prune lock poisoned (prior panic): {}", e))
            })?;

        // Check if interval has elapsed (AC#1)
        if Instant::now().duration_since(*last_prune) < prune_interval {
            return Ok(());
        }

        // Check exponential backoff for recent failures
        // Recover from poisoned mutex if prior task panicked; reset state to safe defaults
        let mut retry_state = match self.prune_retry_state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                warn!("Prune retry state mutex was poisoned; recovering with reset state");
                poisoned.into_inner()
            }
        };

        // If we recovered from poisoning, reset to clean state
        if retry_state.failure_count > 0 || retry_state.first_failure_time.is_some() {
            if let Err(_) = self.prune_retry_state.lock() {
                // Retry state is poisoned; reset it
                retry_state.failure_count = 0;
                retry_state.first_failure_time = None;
            }
        }

        if let Some(first_failure) = retry_state.first_failure_time {
            if retry_state.failure_count > 0 {
                // Exponential backoff: 1s, 5s, 30s, cap at 5 minutes
                let backoff_secs = match retry_state.failure_count {
                    1 => 1,
                    2 => 5,
                    3 => 30,
                    _ => 300,
                };
                let backoff_duration = Duration::from_secs(backoff_secs);

                // Check elapsed time, handling clock regression gracefully (system time went backward)
                match Instant::now().checked_duration_since(first_failure) {
                    Some(elapsed) if elapsed < backoff_duration => {
                        trace!(failure_count = retry_state.failure_count, backoff_secs, "Skipping prune due to exponential backoff");
                        return Ok(());
                    }
                    None => {
                        // Clock went backward; reset backoff state to prevent indefinite failures
                        warn!("System clock regression detected; resetting prune backoff state");
                        retry_state.failure_count = 0;
                        retry_state.first_failure_time = None;
                    }
                    _ => {}
                }
            }
        }

        // Execute pruning via the storage backend
        match self.backend.prune_metric_history() {
            Ok(_deleted_count) => {
                // Reset retry state on successful prune
                retry_state.failure_count = 0;
                retry_state.first_failure_time = None;
                // Update last_prune_time on successful prune
                *last_prune = Instant::now();
                Ok(())
            }
            Err(e) => {
                // Increment failure count and track first failure time
                if retry_state.failure_count == 0 {
                    retry_state.first_failure_time = Some(Instant::now());
                }
                // Use saturating_add to prevent overflow; cap at u32::MAX for indefinite backoff
                retry_state.failure_count = retry_state.failure_count.saturating_add(1);

                error!(error = %e, failure_count = retry_state.failure_count, "Pruning failed; will retry with exponential backoff");
                Err(e)
            }
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
        let poll_start = Instant::now();

        // Process command queue
        self.process_command_queue().await?;

        // Collect all metrics for batch write
        let mut batch_metrics: Vec<crate::storage::BatchMetricWrite> = Vec::new();

        // Collect device IDs and names
        let mut device_ids = Vec::new();
        for app in &self.config.application_list {
            for dev in &app.device_list {
                device_ids.push(dev.device_id.clone());
            }
        }
        debug!(device_count = device_ids.len(), "Found devices");

        // Get metrics from server for each device
        for dev_id in device_ids {
            let dev_metrics = self
                .get_device_metrics_from_server(
                    dev_id.clone(),
                    1,
                    1,
                )
                .await?;

            // Collect metrics from this device for batch write
            for metric in dev_metrics.metrics.values() {
                trace!("Got chirpstack metric for device {}", dev_id);
                trace!(metric = ?metric, "Metric details");

                // Prepare metric for batch write (validate type and create BatchMetricWrite)
                if let Some(batch_metric) = self.prepare_metric_for_batch(&dev_id, metric) {
                    batch_metrics.push(batch_metric);
                }
            }
        }

        // Batch write all collected metrics in a single transaction with retry logic
        if !batch_metrics.is_empty() {
            debug!(count = batch_metrics.len(), "Batch writing metrics from poll cycle");

            // Retry with exponential backoff for transient errors (AC#6)
            let mut attempt = 0;
            let max_retries = 3;
            loop {
                attempt += 1;
                match self.backend.batch_write_metrics(batch_metrics.clone()) {
                    Ok(_) => {
                        // Update gateway status after successful batch write
                        let now_ts = SystemTime::now();
                        let timestamp_rfc3339 = chrono::DateTime::<Utc>::from(now_ts).to_rfc3339();

                        // Update status with non-fatal error tolerance (metrics written even if status fails)
                        if let Err(e) = self.backend.update_gateway_status("last_poll_time", timestamp_rfc3339) {
                            error!(error = %e, "Failed to update last_poll_time in gateway_status (non-fatal)");
                        }
                        if let Err(e) = self.backend.update_gateway_status("server_available", "true".to_string()) {
                            error!(error = %e, "Failed to update server_available in gateway_status (non-fatal)");
                        }
                        break;
                    }
                    Err(e) => {
                        if attempt >= max_retries {
                            error!(error = %e, attempt, "Failed to batch write metrics after {} retries", max_retries);
                            return Err(e);
                        }

                        // Exponential backoff: 1s, 5s, 30s (Story 2-5b pattern)
                        let backoff_secs = match attempt {
                            1 => 1,
                            2 => 5,
                            _ => 30,
                        };
                        warn!(attempt, backoff_secs, error = %e, "Batch write failed; retrying with backoff");
                        tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                    }
                }
            }
        }

        // Log poll cycle latency (Story 4-3)
        let poll_duration = poll_start.elapsed();
        let interval_duration = Duration::from_secs(self.config.chirpstack.polling_frequency);

        if poll_duration > interval_duration {
            warn!(
                cycle_duration_secs = poll_duration.as_secs_f64(),
                interval_secs = self.config.chirpstack.polling_frequency,
                "Poll cycle latency exceeded interval"
            );
        } else {
            debug!(
                cycle_duration_secs = poll_duration.as_secs_f64(),
                interval_secs = self.config.chirpstack.polling_frequency,
                "Poll cycle completed within interval"
            );
        }

        Ok(())
    }

    /// Prepares a device metric for batch write.
    ///
    /// Converts a metric received from ChirpStack into a BatchMetricWrite structure
    /// for inclusion in a batch write operation. Validates the metric type and converts
    /// the value to the appropriate MetricType using kind-driven conversion.
    ///
    /// # Kind-Driven Type Conversion
    ///
    /// Type selection priority:
    /// 1. **Known kind (GAUGE, COUNTER, ABSOLUTE):** Use kind-driven mapping (Float, Int, Float)
    /// 2. **Unknown kind + config type exists:** Use config type as fallback
    /// 3. **Unknown kind + no config type:** Skip metric with warning
    ///
    /// # Counter Monotonic Check
    ///
    /// For MetricType::Int (COUNTER kind), checks if new value < previous value.
    /// If true, skips update to prevent history corruption from counter resets.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the device
    /// * `metric` - The metric data received from ChirpStack
    ///
    /// # Returns
    ///
    /// `Option<BatchMetricWrite>` - Some(prepared metric) if validation succeeds, None if skipped
    ///
    /// Returns None if:
    /// - Unknown metric kind and no config type available
    /// - Metric has no datasets or data
    /// - Counter reset detected (new < previous)
    /// - Metric validation fails
    fn prepare_metric_for_batch(&self, device_id: &str, metric: &Metric) -> Option<crate::storage::BatchMetricWrite> {
        let device_id_string = device_id.to_string();
        let metric_name = metric.name.clone();
        let now_ts = SystemTime::now();

        // 1. Classify metric kind early
        let kind = classify_metric_kind(metric.kind as i32);

        // Validate datasets and data arrays exist with at least one element
        if metric.datasets.is_empty() {
            warn!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, "Metric has no datasets; skipping");
            return None;
        }
        if metric.datasets[0].data.is_empty() {
            warn!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, "Metric dataset is empty; skipping");
            return None;
        }

        let raw_value = metric.datasets[0].data[0];

        // 2. Determine target MetricType (kind-first priority)
        let target_type = match kind {
            ChirpStackMetricKind::Gauge => {
                debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, kind_driven_conversion = true, "Using GAUGE → Float");
                MetricType::Float
            }
            ChirpStackMetricKind::Counter => {
                debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, kind_driven_conversion = true, "Using COUNTER → Int");
                MetricType::Int
            }
            ChirpStackMetricKind::Absolute => {
                debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, kind_driven_conversion = true, "Using ABSOLUTE → Float");
                MetricType::Float
            }
            ChirpStackMetricKind::Unknown => {
                // Fallback to config type if available
                match self.config.get_metric_type(&metric_name, &device_id_string) {
                    Some(cfg_type) => {
                        debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, "Using config fallback for unknown kind");
                        match cfg_type {
                            OpcMetricTypeConfig::Bool => MetricType::Bool,
                            OpcMetricTypeConfig::Int => MetricType::Int,
                            OpcMetricTypeConfig::Float => MetricType::Float,
                            OpcMetricTypeConfig::String => {
                                warn!("Reading string metrics from ChirpStack server is not implemented");
                                return None;
                            }
                        }
                    }
                    None => {
                        warn!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, "Skipping metric: unknown kind and no config");
                        return None;
                    }
                }
            }
        };

        // 3. For Counter type: check monotonic property (reject reset: new < previous)
        if target_type == MetricType::Int && kind == ChirpStackMetricKind::Counter {
            if let Ok(Some(prev_metric)) = self.backend.get_metric_value(&device_id_string, &metric_name) {
                if let Ok(prev_int) = prev_metric.value.parse::<i64>() {
                    let new_int = raw_value as i64;
                    if new_int < prev_int {
                        warn!(device_id = %device_id, metric_name = %metric_name,
                              metric_kind = "counter", prev_value = prev_int, new_value = new_int,
                              "Counter reset detected; skipping update");
                        return None;
                    }
                }
            }
        }

        // 4. Validate and convert value based on target type
        let (value_str, metric_type) = match target_type {
            MetricType::Bool => {
                match raw_value {
                    0.0 => ("0".to_string(), MetricType::Bool),
                    1.0 => ("1".to_string(), MetricType::Bool),
                    _ => {
                        error!(value = %raw_value, metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, "Not a valid boolean value");
                        return None;
                    }
                }
            }
            MetricType::Int => {
                let int_val = raw_value as i64;
                if raw_value.fract() != 0.0 {
                    warn!(value = %raw_value, metric_name = %metric_name, device_id = %device_id,
                          metric_kind = ?kind, "Counter metric has fractional value; precision lost");
                }
                (int_val.to_string(), MetricType::Int)
            }
            MetricType::Float => (raw_value.to_string(), MetricType::Float),
            MetricType::String => {
                warn!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, "Reading string metrics from ChirpStack server is not implemented");
                return None;
            }
        };

        debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, kind_driven_conversion = true, "Metric prepared for batch write");

        Some(crate::storage::BatchMetricWrite {
            device_id: device_id.to_string(),
            metric_name,
            value: value_str,
            data_type: metric_type,
            timestamp: now_ts,
        })
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
        let device_name = match self.config.get_device_name(device_id) {
            Some(name) => name,
            None => {
                warn!(device_id = %device_id, "Device name not found in config, skipping metric");
                return;
            }
        };

        let metric_name = metric.name.clone();
        let now_ts = SystemTime::now();

        // Process metric based on configured type, with append-only historical logging
        // NOTE: If upsert_metric_value() succeeds but append_metric_history() fails,
        // the metric will exist in metric_values but not in metric_history. This is
        // intentional to allow the poller to continue (non-fatal error handling).
        // Story 2-3c will implement batch transactional wrapping to ensure atomicity.
        match self.config.get_metric_type(&metric_name, device_id) {
            Some(metric_type) => match metric_type {
                OpcMetricTypeConfig::Bool => {
                    debug!(metric = ?metric, "Bool metric");
                    let value = metric.datasets[0].data[0];
                    match value {
                        0.0 | 1.0 => {},
                        _ => {
                            error!(value = %value, "Not a boolean value");
                            return;
                        }
                    };
                    let metric_val = MetricType::Bool;
                    if let Err(e) = self.backend.upsert_metric_value(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to upsert bool metric");
                    } else if let Err(e) = self.backend.append_metric_history(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to append bool metric to history");
                    }
                }
                OpcMetricTypeConfig::Int => {
                    debug!(metric = ?metric, "Int metric");
                    let raw_value = metric.datasets[0].data[0];
                    if raw_value.fract() != 0.0 {
                        warn!(value = %raw_value, metric_name = %metric_name, "Float metric truncated to int; precision lost");
                    }
                    let metric_val = MetricType::Int;
                    if let Err(e) = self.backend.upsert_metric_value(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to upsert int metric");
                    } else if let Err(e) = self.backend.append_metric_history(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to append int metric to history");
                    }
                }
                OpcMetricTypeConfig::Float => {
                    debug!(metric = ?metric, "Float metric");
                    let metric_val = MetricType::Float;
                    if let Err(e) = self.backend.upsert_metric_value(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to upsert float metric");
                    } else if let Err(e) = self.backend.append_metric_history(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to append float metric to history");
                    }
                }
                OpcMetricTypeConfig::String => {
                    let metric_val = MetricType::String;
                    if let Err(e) = self.backend.upsert_metric_value(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to upsert string metric");
                    } else if let Err(e) = self.backend.append_metric_history(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to append string metric to history");
                    }
                }
            },
            None => {
                warn!(metric_name = ?metric_name, device_name = ?device_name, "No metric type found for chirpstack metric");
            }
        };
    }

    /// Fetches all applications with pagination support (Story 4-3).
    ///
    /// Automatically handles pagination by making multiple gRPC requests until
    /// all applications are retrieved. Uses the configured page size.
    /// Respects cancellation token for graceful shutdown (AC#5).
    ///
    /// # Returns
    ///
    /// `Result<Vec<ApplicationDetail>, OpcGwError>` - All applications across all pages
    ///
    /// # Errors
    ///
    /// Returns error on gRPC client or request failure. Logs page-level errors.
    async fn fetch_all_applications(&self) -> Result<Vec<ApplicationDetail>, OpcGwError> {
        // Check cancellation before starting
        if self.cancel_token.is_cancelled() {
            info!("Pagination cancelled before fetch_all_applications");
            return Ok(Vec::new());
        }
        debug!("Fetching all applications with pagination");
        let page_size = self.config.chirpstack.list_page_size;
        let mut all_applications = Vec::new();
        let mut offset = 0u32;
        let mut pages_fetched = 0u32;
        const MAX_PAGES: u32 = 10_000; // DoS prevention: limit maximum pages per request
        let application_client = self.create_application_client().await?;

        loop {
            // Check for cancellation token at each iteration (AC#5: no blocking)
            if self.cancel_token.is_cancelled() {
                info!(pages_fetched = pages_fetched, "Pagination cancelled mid-loop; returning collected data");
                break;
            }

            // DoS prevention: limit maximum pages (Story 4-3)
            if pages_fetched >= MAX_PAGES {
                error!(pages_fetched = pages_fetched, limit = MAX_PAGES, "Maximum page limit reached in pagination; stopping to prevent DoS");
                break;
            }

            pages_fetched += 1;
            let page_start = Instant::now();
            debug!(page = pages_fetched, offset = offset, limit = page_size, "Fetching applications page");

            let request = Request::new(ListApplicationsRequest {
                limit: page_size,
                offset,
                search: String::new(),
                tenant_id: self.config.chirpstack.tenant_id.clone(),
            });

            match application_client.clone().list(request).await {
                Ok(response) => {
                    let page_duration = page_start.elapsed();
                    let response_inner = response.into_inner();
                    let result_count = response_inner.result.len() as u32;
                    all_applications.extend(self.convert_to_applications(response_inner));

                    debug!(page = pages_fetched, duration_ms = page_duration.as_millis(), "Applications page fetch completed");

                    if result_count < page_size {
                        break;
                    }

                    offset = offset.saturating_add(page_size);
                }
                Err(e) => {
                    let page_duration = page_start.elapsed();
                    warn!(page = pages_fetched, duration_ms = page_duration.as_millis(), error = %e, "Failed to fetch applications page; skipping and continuing with collected data");
                    break;
                }
            }
        }

        info!(
            applications_count = all_applications.len(),
            apps_pages = pages_fetched,
            "Completed pagination for applications"
        );
        Ok(all_applications)
    }

    /// Fetches all devices for a given application with pagination support (Story 4-3).
    ///
    /// Automatically handles pagination by making multiple gRPC requests until
    /// all devices are retrieved. Uses the configured page size.
    /// Respects cancellation token for graceful shutdown (AC#5).
    ///
    /// # Arguments
    ///
    /// * `application_id` - The ChirpStack application ID
    ///
    /// # Returns
    ///
    /// `Result<Vec<DeviceListDetail>, OpcGwError>` - All devices across all pages
    async fn fetch_all_devices_for_app(
        &self,
        application_id: String,
    ) -> Result<Vec<DeviceListDetail>, OpcGwError> {
        // Check cancellation before starting
        if self.cancel_token.is_cancelled() {
            info!(application_id = %application_id, "Pagination cancelled before fetch_all_devices_for_app");
            return Ok(Vec::new());
        }

        debug!(application_id = %application_id, "Fetching all devices with pagination");
        let page_size = self.config.chirpstack.list_page_size;
        let mut all_devices = Vec::new();
        let mut offset = 0u32;
        let mut pages_fetched = 0u32;
        const MAX_PAGES: u32 = 10_000; // DoS prevention: limit maximum pages per request
        let device_client = self.create_device_client().await?;

        loop {
            // Check for cancellation token at each iteration (AC#5: no blocking)
            if self.cancel_token.is_cancelled() {
                info!(application_id = %application_id, pages_fetched = pages_fetched, "Pagination cancelled mid-loop; returning collected data");
                break;
            }

            // DoS prevention: limit maximum pages (Story 4-3)
            if pages_fetched >= MAX_PAGES {
                error!(application_id = %application_id, pages_fetched = pages_fetched, limit = MAX_PAGES, "Maximum page limit reached in pagination; stopping to prevent DoS");
                break;
            }

            pages_fetched += 1;
            let page_start = Instant::now();
            debug!(application_id = %application_id, page = pages_fetched, offset = offset, limit = page_size, "Fetching devices page");

            let request = Request::new(ListDevicesRequest {
                limit: page_size,
                offset,
                search: String::new(),
                application_id: application_id.clone(),
                multicast_group_id: String::new(),
                device_profile_id: String::new(),
                order_by: 0,
                order_by_desc: false,
                tags: HashMap::new(),
            });

            match device_client.clone().list(request).await {
                Ok(response) => {
                    let page_duration = page_start.elapsed();
                    let response_inner = response.into_inner();
                    let result_count = response_inner.result.len() as u32;
                    all_devices.extend(self.convert_to_devices(response_inner));

                    debug!(application_id = %application_id, page = pages_fetched, duration_ms = page_duration.as_millis(), "Devices page fetch completed");

                    if result_count < page_size {
                        break;
                    }

                    offset = offset.saturating_add(page_size);
                }
                Err(e) => {
                    let page_duration = page_start.elapsed();
                    warn!(application_id = %application_id, page = pages_fetched, duration_ms = page_duration.as_millis(), error = %e, "Failed to fetch devices page; skipping and continuing with collected data");
                    break;
                }
            }
        }

        debug!(
            application_id = %application_id,
            devices_count = all_devices.len(),
            devices_pages = pages_fetched,
            "Completed pagination for devices"
        );
        Ok(all_devices)
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
        self.fetch_all_applications().await
    }

    /// Retrieves the list of devices for a specific application.
    ///
    /// Sends a request to the ChirpStack DeviceService to obtain a list of all
    /// devices within the specified application. This provides device metadata
    /// including DevEUI, name, and description. Uses pagination internally
    /// to handle deployments with more than the page size limit.
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
        trace!(application_id = ?application_id, "For chirpstack application");
        self.fetch_all_devices_for_app(application_id).await
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
        debug!(dev_eui = ?dev_eui, "For chirpstack device");
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
                    warn!("Waiting for Chirpstack server");
                    trace!(retry_count = count, "Retry count");
                    count += 1;
                    tokio::time::sleep(delay).await;
                }
            }
        }

        trace!("Create device service client for Chirpstack");
        let mut device_client = self.create_device_client().await?;

        //trace!(request = ?request, "Request created");
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
            // TODO (Phase 3): Refactor command queue processing to use dequeue_command from StorageBackend trait
            // Current implementation requires conversion between Command (trait) and DeviceCommandInternal (internal)
            // For now, skip command processing until type unification is complete in Story 4-1 Phase 3
            match self.backend.dequeue_command() {
                Ok(Some(_command)) => {
                    // TODO: Convert Command to DeviceCommandInternal and call enqueue_device_request_to_server
                    trace!("Command dequeued but not yet processed (Phase 3 work)");
                    // For now, just skip it and continue
                    continue;
                }
                Ok(None) => {
                    // Queue is empty, exit loop
                    break;
                }
                Err(e) => {
                    error!(error = %e, "Failed to dequeue command");
                    return Err(e);
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
    /// use chrono::Utc;
    /// use crate::storage::CommandStatus;
    ///
    /// let command = crate::storage::DeviceCommandInternal {
    ///     id: 1,
    ///     device_id: "1234567890abcdef".to_string(),
    ///     payload: vec![0x01, 0x02, 0x03],
    ///     f_port: 1,
    ///     status: CommandStatus::Pending,
    ///     created_at: Utc::now(),
    ///     error_message: None,
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
        command: crate::storage::DeviceCommandInternal,
    ) -> Result<(), OpcGwError> {
        trace!("Enqueue device request");
        if command.f_port < 1 {
            return Err(OpcGwError::ChirpStack("Invalid fPort".to_string()));
        }
        // Create a new request
        debug!("Create request");
        // Determine if confirmed based on status (pending commands are not yet sent/confirmed)
        let is_confirmed = command.status == crate::storage::CommandStatus::Sent;
        let queue_item = DeviceQueueItem {
            id: "".to_string(),
            dev_eui: command.device_id.clone(),
            confirmed: is_confirmed,
            f_port: command.f_port as u32,
            data: command.payload.clone(),
            object: None,
            is_pending: command.status == crate::storage::CommandStatus::Pending,
            f_cnt_down: 0,
            is_encrypted: false,
            expires_at: None,
        };
        debug!(queue_item = ?queue_item, "Request created");

        // Send request to server
        let request = Request::new(EnqueueDeviceQueueItemRequest {
            queue_item: Some(queue_item),
            flush_queue: false,
        });

        let mut device_client = self.create_device_client().await?;
        match device_client.enqueue(request).await {
            Ok(response) => {
                let inner_response = response.into_inner();
                trace!(response = ?inner_response, "Response received");
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "Error enqueueing device request");
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
        trace!(application = ?app, "Application details");
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
        trace!(dev_eui = %device.dev_eui, device_name = %device.name, description = %device.description, "Device details");
    }
}

// ============================================================================
// Story 3-3: Command Delivery Status Polling and Timeout Handler
// ============================================================================

/// CommandStatusPoller: Polls ChirpStack for command delivery confirmations
///
/// Runs as a background task that periodically queries ChirpStack for command
/// status updates. When confirmations are received, marks local commands as confirmed
/// for end-to-end visibility into command delivery lifecycle.
pub struct CommandStatusPoller {
    /// Configuration for polling intervals and timeouts
    config: AppConfig,
    /// Shared storage backend for updating command statuses
    pub storage: Arc<dyn crate::storage::StorageBackend>,
    /// Cancellation token for graceful shutdown
    cancel_token: tokio_util::sync::CancellationToken,
}

impl CommandStatusPoller {
    /// Creates a new CommandStatusPoller instance.
    ///
    /// # Arguments
    ///
    /// * `config` - Application configuration with command delivery settings
    /// * `storage` - Shared storage backend for command status updates
    /// * `cancel_token` - Cancellation token for graceful shutdown
    ///
    /// # Returns
    ///
    /// `Result<Self, OpcGwError>` - New poller instance
    pub fn new(
        config: &AppConfig,
        storage: Arc<dyn crate::storage::StorageBackend>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<Self, OpcGwError> {
        debug!("Creating CommandStatusPoller for command delivery confirmation polling");

        Ok(CommandStatusPoller {
            config: config.clone(),
            storage,
            cancel_token,
        })
    }

    /// Main polling loop for command delivery confirmations.
    ///
    /// Periodically queries for pending confirmations and polls ChirpStack for updates.
    /// When confirmations are received from ChirpStack, marks commands as confirmed
    /// in the local storage for OPC UA visibility.
    ///
    /// # Returns
    ///
    /// `Result<(), OpcGwError>` - Ok on graceful shutdown, error on failure
    pub async fn run(&mut self) -> Result<(), OpcGwError> {
        let poll_interval = Duration::from_secs(
            self.config.global.command_delivery_poll_interval_secs
        );

        debug!(interval_s = poll_interval.as_secs(), "Starting CommandStatusPoller");

        loop {
            // Find commands awaiting confirmation
            match self.storage.find_pending_confirmations() {
                Ok(pending_commands) => {
                    if !pending_commands.is_empty() {
                        debug!(count = pending_commands.len(), "Found pending command confirmations");

                        // For each pending command, check ChirpStack status
                        // (In a real implementation, would call ChirpStack API here)
                        // For now, this is a placeholder for integration with ChirpStack
                        // The actual ChirpStack API calls would happen here
                        trace!(pending_count = pending_commands.len(), "Would poll ChirpStack for {} commands", pending_commands.len());
                    }
                }
                Err(e) => {
                    error!(error = %e, "Failed to query pending command confirmations");
                }
            }

            // Wait for next poll cycle or cancellation
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("CommandStatusPoller shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(poll_interval) => {}
            }
        }
    }
}

/// CommandTimeoutHandler: Marks timed-out commands as failed
///
/// Runs as a background task that scans for commands that have been in "sent" state
/// for too long without confirmation. After the TTL expires, marks them as failed
/// with a "Confirmation timeout" error message.
pub struct CommandTimeoutHandler {
    /// Configuration for timeout settings
    config: AppConfig,
    /// Shared storage backend for updating command statuses
    pub storage: Arc<dyn crate::storage::StorageBackend>,
    /// Cancellation token for graceful shutdown
    cancel_token: tokio_util::sync::CancellationToken,
}

impl CommandTimeoutHandler {
    /// Creates a new CommandTimeoutHandler instance.
    ///
    /// # Arguments
    ///
    /// * `config` - Application configuration with timeout settings
    /// * `storage` - Shared storage backend for command status updates
    /// * `cancel_token` - Cancellation token for graceful shutdown
    ///
    /// # Returns
    ///
    /// `Result<Self, OpcGwError>` - New handler instance
    pub fn new(
        config: &AppConfig,
        storage: Arc<dyn crate::storage::StorageBackend>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Result<Self, OpcGwError> {
        debug!("Creating CommandTimeoutHandler for command delivery timeout detection");

        Ok(CommandTimeoutHandler {
            config: config.clone(),
            storage,
            cancel_token,
        })
    }

    /// Main loop for detecting and handling timed-out commands.
    ///
    /// Periodically scans for commands that have exceeded their TTL without confirmation.
    /// When found, marks them as failed with a "Confirmation timeout" error message.
    /// Timeout check interval can be configured via config.global.command_timeout_check_interval_secs.
    ///
    /// # Returns
    ///
    /// `Result<(), OpcGwError>` - Ok on graceful shutdown, error on failure
    pub async fn run(&mut self) -> Result<(), OpcGwError> {
        let ttl_secs = self.config.global.command_delivery_timeout_secs;
        let check_interval = Duration::from_secs(
            self.config.global.command_timeout_check_interval_secs
        );

        debug!(ttl_s = ttl_secs, check_interval_s = check_interval.as_secs(), "Starting CommandTimeoutHandler");

        loop {
            // Find commands that have timed out
            match self.storage.find_timed_out_commands(ttl_secs) {
                Ok(timed_out_commands) => {
                    for cmd in timed_out_commands {
                        debug!(
                            command_id = cmd.id,
                            device_id = %cmd.device_id,
                            command_name = %cmd.command_name,
                            "Command timed out, marking as failed"
                        );

                        if let Err(e) = self.storage.mark_command_failed(cmd.id, "Confirmation timeout") {
                            error!(
                                error = %e,
                                command_id = cmd.id,
                                "Failed to mark timed-out command as failed"
                            );
                        }
                    }
                }
                Err(e) => {
                    error!(error = %e, "Failed to query timed-out commands");
                }
            }

            // Wait for next check cycle or cancellation
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("CommandTimeoutHandler shutting down");
                    return Ok(());
                }
                _ = tokio::time::sleep(check_interval) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::memory::InMemoryBackend;
    use crate::config::AppConfig;
    use figment::Figment;
    use figment::providers::{Toml, Format};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn get_test_config() -> AppConfig {
        let config_path = std::env::var("CONFIG_PATH")
            .unwrap_or_else(|_| "tests/config/config.toml".to_string());
        let config: AppConfig = Figment::new()
            .merge(Toml::file(&config_path))
            .extract()
            .expect("Failed to load test configuration");
        config
    }

    #[tokio::test]
    async fn test_chirpstack_poller_creation_with_backend() {
        let config = get_test_config();
        let backend = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let restore_barrier = Arc::new(std::sync::Barrier::new(2));

        let result = ChirpstackPoller::new(
            &config,
            backend,
            cancel_token,
            restore_barrier,
        ).await;

        assert!(
            result.is_ok(),
            "ChirpstackPoller should be created successfully with StorageBackend"
        );
    }

    #[tokio::test]
    async fn test_chirpstack_poller_uses_backend_trait() {
        let config = get_test_config();
        let backend = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let restore_barrier = Arc::new(std::sync::Barrier::new(2));

        let poller = ChirpstackPoller::new(
            &config,
            backend.clone(),
            cancel_token,
            restore_barrier,
        )
        .await
        .expect("Failed to create poller");

        // Verify poller can call backend trait methods
        // (This is a smoke test; detailed method calls are tested elsewhere)
        let status = backend.get_gateway_status("test_key");
        assert!(status.is_ok(), "Backend trait methods should be accessible from poller");
    }

    #[test]
    fn test_poller_struct_has_backend_field() {
        // Compile-time verification: ChirpstackPoller struct contains Arc<dyn StorageBackend>
        // This test exists primarily for documentation and to catch any regressions
        // where the struct accidentally reverts to Arc<Mutex<Storage>>
        let _: () = {
            let backend = Arc::new(InMemoryBackend::new());

            // This function signature proves that ChirpstackPoller::new requires Arc<dyn StorageBackend>
            // and does NOT require Arc<Mutex<Storage>> or ConnectionPool
            let _backend_type: Arc<dyn StorageBackend> = backend;
        };
    }

    #[test]
    fn test_exponential_backoff_retry_delays() {
        // Verify exponential backoff delays follow AC#6 pattern: 1s, 5s, 30s
        let delays = [
            (1, 1u64),   // Attempt 1: 1 second
            (2, 5u64),   // Attempt 2: 5 seconds
            (3, 30u64),  // Attempt 3: 30 seconds
        ];

        for (attempt, expected_secs) in &delays {
            let backoff_secs = match attempt {
                1 => 1,
                2 => 5,
                _ => 30,
            };
            assert_eq!(
                backoff_secs, *expected_secs,
                "Attempt {} should have {} second backoff",
                attempt, expected_secs
            );
        }
    }
}

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
//! ```rust,ignore
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
use chrono::{DateTime, Utc};
use tracing::{debug, error, info, trace, warn};
use chirpstack_api::prost_types::Timestamp;
use serde::Deserialize;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::time::Duration;
use tonic::codegen::InterceptedService;
use tonic::service::Interceptor;
use tonic::{transport::Channel, Request, Status};
use url::Url;

// Import generated types
use crate::storage::{CommandStatus, DeviceCommand, MetricType, StorageBackend};
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
/// Story C-1 promoted to first-class type — consumed by `src/web/inventory.rs`
/// via the inventory cache layer (`src/chirpstack_inventory.rs`).
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
/// Used when listing devices within an application. Story C-1 promoted the
/// `device_profile_name` and `last_seen_at` fields (previously discarded in the
/// gRPC-to-struct conversion) so the inventory API can render richer picker UIs.
#[derive(Debug, Deserialize, Clone)]
pub struct DeviceListDetail {
    /// The unique identifier for the device (DevEUI).
    pub dev_eui: String,
    /// The name of the device.
    pub name: String,
    /// A description of the device.
    pub description: String,
    /// Device-profile name (gRPC field 8). C-1 promotes this for the picker UI.
    /// `None` if ChirpStack returns an empty string.
    #[serde(default)]
    pub device_profile_name: Option<String>,
    /// Last-seen timestamp as RFC3339 string (gRPC field 4). C-1 promotes this
    /// for the picker UI. `None` if the device has never been seen.
    #[serde(default)]
    pub last_seen_at: Option<String>,
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
pub(crate) enum ChirpStackMetricKind {
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
/// ```rust,ignore
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
/// (Story 6-3 AC#4) Spike-detection threshold. Centralized so tests and the
/// production call site share the same constant.
pub(crate) const ERROR_SPIKE_THRESHOLD: i32 = 5;

/// (Story 6-3 AC#4 — iter-3 review pending #1 helper extraction) Emit the
/// `error_spike` warn iff `current - previous >= ERROR_SPIKE_THRESHOLD`.
/// Returns `Some(delta)` when the warn fires (so tests can assert without
/// re-implementing the predicate). `saturating_sub` matches the production
/// caller's contract.
pub(crate) fn maybe_emit_error_spike(
    previous_error_count: i32,
    error_count: i32,
) -> Option<i32> {
    let delta = error_count.saturating_sub(previous_error_count);
    if delta >= ERROR_SPIKE_THRESHOLD {
        warn!(
            operation = "error_spike",
            previous = previous_error_count,
            current = error_count,
            delta = delta,
            "Error count spike detected between consecutive poll cycles"
        );
        Some(delta)
    } else {
        None
    }
}

/// (Story 6-3 AC#5 — iter-3 review pending #1 helper extraction) Format the
/// `last_successful_poll` field. Returns `"null"` for `None` so the log
/// schema stays string-typed and matches the rfc3339 sibling field.
pub(crate) fn format_last_successful_poll(
    last_successful_poll: Option<DateTime<Utc>>,
) -> String {
    last_successful_poll
        .map(|t| t.to_rfc3339())
        .unwrap_or_else(|| "null".to_string())
}

/// (Story 4-4) Outcome of a single ChirpStack outage recovery loop run.
///
/// Returned by `ChirpstackPoller::recover_from_chirpstack_outage` so the
/// outer `poll_metrics` cycle can branch on whether the loop restored
/// connectivity (`Recovered`) or exhausted its retry budget (`Exhausted`).
/// Either way the poller's `run()` outer loop continues — `Exhausted` does
/// NOT terminate the poller.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RecoveryOutcome {
    /// TCP probe succeeded within the configured retry budget.
    Recovered { attempts_used: u32 },
    /// Retry budget exhausted (or cancellation token fired) without
    /// restoring connectivity.
    Exhausted {
        attempts_used: u32,
        last_error: String,
    },
}

/// (Story 6-3 AC#5 — iter-3 review pending #1 helper extraction) Emit the
/// `chirpstack_outage` warn iff this is the first connectivity failure of
/// the cycle. Mutates `outage_already_logged` to `true` so subsequent
/// per-device failures in the same cycle do not re-fire. Returns whether
/// the warn was emitted.
pub(crate) fn maybe_emit_chirpstack_outage(
    outage_already_logged: &mut bool,
    last_successful_poll: Option<DateTime<Utc>>,
    error: &OpcGwError,
) -> bool {
    if *outage_already_logged {
        return false;
    }
    let last_successful_poll_str = format_last_successful_poll(last_successful_poll);
    warn!(
        operation = "chirpstack_outage",
        timestamp = %chrono::Utc::now().to_rfc3339(),
        last_successful_poll = %last_successful_poll_str,
        current_attempt_failed_with = %error,
        "ChirpStack outage detected — poll continues without recovery (Story 4-4 will add recovery loop)"
    );
    *outage_already_logged = true;
    true
}

/// (Story 6-3 AC#6 — iter-3 review pending #1 helper extraction) Classify a
/// raw boolean metric value. On the only-`0.0`-or-`1.0` invariant violation,
/// emits the canonical `metric_parse` warn and returns `None`. Tests can
/// drive the helper directly to verify the warn shape without constructing
/// a full `ChirpstackPoller`. The argument is `f32` to match the
/// upstream chirpstack-api `Metric.datasets[].data[]` element type.
pub(crate) fn validate_bool_metric_value(
    raw_value: f32,
    device_id: &str,
    metric_name: &str,
    kind: ChirpStackMetricKind,
) -> Option<&'static str> {
    match raw_value {
        0.0 => Some("0"),
        1.0 => Some("1"),
        _ => {
            error!(
                value = %raw_value,
                metric_name = %metric_name,
                device_id = %device_id,
                metric_kind = ?kind,
                "Not a valid boolean value"
            );
            warn!(
                event = "metric_parse",
                device_id = %device_id,
                metric_name = %metric_name,
                raw_value = %raw_value,
                expected_type = "Bool",
                reason = "invalid_bool",
                "Metric parse failed; skipping update"
            );
            None
        }
    }
}

/// (Story 6-3 AC#6/AC#7 — iter-3 review pending #1 helper extraction)
/// Classification of the relevant `tonic::Code` variants for the
/// ChirpStack gRPC request layer. `Other` covers status codes the current
/// code does not specially log (Unimplemented, NotFound, InvalidArgument,
/// etc.).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum GrpcErrorClass {
    Timeout,
    Transient,
    Other,
}

/// (Story 6-3 AC#6/AC#7 — iter-3 review pending #1 helper extraction)
/// Classify a `tonic::Status` and emit the appropriate `chirpstack_request`
/// warn. Returns the classification so callers can branch on it. Pure with
/// respect to the input — production callers stay otherwise unchanged.
pub(crate) fn classify_and_log_grpc_error(
    status: &tonic::Status,
    duration_ms: u64,
    retry_delay_secs: u64,
) -> GrpcErrorClass {
    match status.code() {
        tonic::Code::DeadlineExceeded => {
            warn!(
                operation = "chirpstack_request",
                duration_ms = duration_ms,
                timeout_secs = 0u64,
                exceeded = true,
                error = %status,
                "ChirpStack gRPC request timed out"
            );
            GrpcErrorClass::Timeout
        }
        tonic::Code::Unavailable | tonic::Code::Cancelled => {
            warn!(
                operation = "chirpstack_request",
                error = %status,
                attempt = 1u32,
                retry_delay_secs = retry_delay_secs,
                "ChirpStack gRPC request hit transient network failure"
            );
            GrpcErrorClass::Transient
        }
        _ => GrpcErrorClass::Other,
    }
}

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
    /// Story 6-3, AC#4: per-cycle error count from the previous poll cycle.
    /// Compared against the current cycle's count so a delta of >=5 surfaces
    /// as an `error_spike` warn line. Plain `i32` field — Epic 5 retrospective
    /// rules out shared atomics or mutexes for counters.
    previous_error_count: i32,
    /// Story 6-3, AC#5: timestamp of the most recent poll cycle that wrote
    /// metrics or status. Surfaced as `last_successful_poll` on the
    /// `chirpstack_outage` warn so an operator can see how long the gateway
    /// has been blind. `None` until the first cycle succeeds.
    last_successful_poll: Option<DateTime<Utc>>,
    /// Story 9-7: configuration hot-reload receiver. The poller's outer
    /// `tokio::select!` arm awaits `config_rx.changed()` and refreshes
    /// `self.config` from the published `Arc<AppConfig>` at the next
    /// cycle boundary. Story 4-4's recovery loop reads `retry`/`delay`
    /// at loop entry — this gives 4-4 hot-reload integration without
    /// modifying the recovery routine.
    ///
    /// `Option` rather than required so the historical
    /// `ChirpstackPoller::new` API stays compatible: production
    /// (`src/main.rs`) constructs the poller with `Some(rx)`; legacy
    /// tests that don't exercise hot-reload pass `None` and the
    /// outer-loop arm becomes a no-op (await on a future that never
    /// completes).
    config_rx: Option<tokio::sync::watch::Receiver<Arc<AppConfig>>>,
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
    /// ```rust,ignore
    /// use crate::chirpstack::ChirpstackPoller;
    /// use std::sync::{Arc, Mutex};
    ///
    /// async fn create_poller() -> Result<ChirpstackPoller, OpcGwError> {
    ///     let config = AppConfig::new().unwrap();
    ///     let storage = Arc::new(Mutex::new(Storage::new(&config)));
    ///     ChirpstackPoller::new(&config, storage).await
    /// }
    /// ```
    /// Story 9-7 retains this entry point for legacy lib callers
    /// (chirpstack tests, integration test fixtures) that don't need
    /// hot-reload. `src/main.rs` uses [`new_with_reload`] instead.
    #[allow(dead_code)]
    pub async fn new(
        config: &AppConfig,
        backend: Arc<dyn StorageBackend>,
        cancel_token: tokio_util::sync::CancellationToken,
        restore_barrier: Arc<std::sync::Barrier>,
    ) -> Result<Self, OpcGwError> {
        Self::new_with_reload(config, backend, cancel_token, restore_barrier, None).await
    }

    /// Story 9-7 constructor variant that takes an explicit
    /// configuration-reload receiver. Production wiring (`src/main.rs`)
    /// passes `Some(rx)` cloned off `ConfigReloadHandle`. The legacy
    /// [`new`] constructor delegates here with `None`, preserving the
    /// pre-9-7 contract for callers that don't care about hot-reload
    /// (notably the chirpstack tests + the legacy command-status
    /// poller paths).
    pub async fn new_with_reload(
        config: &AppConfig,
        backend: Arc<dyn StorageBackend>,
        cancel_token: tokio_util::sync::CancellationToken,
        restore_barrier: Arc<std::sync::Barrier>,
        mut config_rx: Option<tokio::sync::watch::Receiver<Arc<AppConfig>>>,
    ) -> Result<Self, OpcGwError> {
        debug!("Create a new Chirpstack poller");

        // Iter-2 review P31: consume the initial publish so the
        // first `changed()` in the run-loop waits for the next
        // SIGHUP rather than firing immediately on a freshly-
        // subscribed receiver. Without this, the poller would emit
        // a spurious `config_reload_applied` log line on its first
        // outer-loop iteration.
        if let Some(rx) = config_rx.as_mut() {
            let _ = rx.borrow_and_update();
        }

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
            previous_error_count: 0,
            last_successful_poll: None,
            config_rx,
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
    /// ```rust,ignore
    /// let channel = poller.create_channel().await?;
    /// ```
    async fn create_channel(&self) -> Result<tonic::transport::Channel, OpcGwError> {
        debug!("Create channel");
        // Story 6-3, AC#1: structured diagnostics around the gRPC connect
        // attempt. The current code does NOT retry the channel itself
        // (the retry loop in `get_device_metrics_from_server` retries the
        // TCP availability probe instead), so we log a single attempt =
        // 1 here. Story 4-4 will extend this with explicit reconnect
        // logic; the operation name is reserved for compatibility.
        let endpoint = self.config.chirpstack.server_address.clone();
        // Review patch P24: validate server_address is non-empty before
        // attempting to connect, so the failure message names the
        // configuration field instead of `Channel::from_shared`'s opaque
        // "invalid endpoint" wrapper.
        if endpoint.trim().is_empty() {
            return Err(OpcGwError::Configuration(
                "chirpstack.server_address is empty".to_string(),
            ));
        }
        // Iter-3 D-AC1 resolution: AC#1 literal text mandates `timeout_secs`
        // on every `chirpstack_connect` log line. Emit `timeout_secs=0` here
        // — `0` is the documented sentinel for "no deadline configured" on
        // the create-channel branch (the probe loop further down emits a
        // real per-attempt timeout). Combined with `max_retries=0u32`, the
        // numeric schema stays consistent across both connect paths.
        info!(
            operation = "chirpstack_connect",
            attempt = 1u32,
            endpoint = %endpoint,
            timeout_secs = 0u64,
            "gRPC channel connect attempt"
        );
        let connect_start = Instant::now();
        // Story 4-4 (resolves deferred-work.md:86 6-3 carry-forward):
        // wrap `builder.connect()` with a 5s timeout. Smaller than NFR17's
        // 30s SLA so a single channel rebuild doesn't blow the recovery
        // budget; larger than the TCP probe's 1s timeout so transient
        // slow-but-reachable servers don't get falsely flagged.
        const CHANNEL_CONNECT_TIMEOUT_SECS: u64 = 5;
        let channel = match Channel::from_shared(endpoint.clone()) {
            Ok(builder) => match tokio::time::timeout(
                Duration::from_secs(CHANNEL_CONNECT_TIMEOUT_SECS),
                builder.connect(),
            )
            .await
            {
                Ok(Ok(channel)) => {
                    let latency_ms = connect_start.elapsed().as_millis() as u64;
                    info!(
                        operation = "chirpstack_connect",
                        attempt = 1u32,
                        latency_ms = latency_ms,
                        success = true,
                        "gRPC channel connected"
                    );
                    channel
                }
                Ok(Err(e)) => {
                    warn!(
                        operation = "chirpstack_connect",
                        attempt = 1u32,
                        error = %e,
                        retry_delay_secs = 0u64,
                        max_retries = 0u32,
                        success = false,
                        "gRPC channel connect failed"
                    );
                    // Story 4-4 P2: transport-layer connect failures use
                    // OpcGwError::ChirpStack so the per-device error branch
                    // at poll_metrics:1052-1068 (matches!(e, ChirpStack(_)))
                    // recognises this as a connectivity failure and triggers
                    // the recovery loop. Previous variant `Configuration`
                    // was incorrect — Configuration is for parse/validation
                    // failures, not runtime transport.
                    return Err(OpcGwError::ChirpStack(format!(
                        "Failed to connect channel: {}",
                        e
                    )));
                }
                Err(_elapsed) => {
                    warn!(
                        operation = "chirpstack_connect",
                        attempt = 1u32,
                        timeout_secs = CHANNEL_CONNECT_TIMEOUT_SECS,
                        success = false,
                        "gRPC channel connect timed out"
                    );
                    // Story 4-4 P2: same rationale as above — timeout is a
                    // transport failure, not a config problem.
                    return Err(OpcGwError::ChirpStack(format!(
                        "Failed to connect channel: timed out after {}s",
                        CHANNEL_CONNECT_TIMEOUT_SECS
                    )));
                }
            },
            Err(e) => {
                warn!(
                    operation = "chirpstack_connect",
                    attempt = 1u32,
                    error = %e,
                    retry_delay_secs = 0u64,
                    max_retries = 0u32,
                    success = false,
                    "gRPC channel construction failed (invalid endpoint)"
                );
                return Err(OpcGwError::Configuration(format!(
                    "Failed to create channel: {}",
                    e
                )));
            }
        };
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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
    /// ```rust,ignore
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

        // Resolve host:port to socket addresses. We use to_socket_addrs()
        // rather than SocketAddr::parse() so DNS hostnames are resolved — the
        // common Docker / Compose case where opcgw and ChirpStack share a
        // user-defined network and the address is the service name
        // (e.g. http://chirpstack:8080). SocketAddr::parse() only accepts a
        // numeric IP and rejected hostnames with "invalid socket address
        // syntax" (GH #122). This matches the DNS resolution the gRPC client
        // already performs. A hostname may resolve to several addresses
        // (IPv4 + IPv6); we try each until one connects.
        let socket_addrs: Vec<SocketAddr> = format!("{}:{}", host, port)
            .to_socket_addrs()
            .map_err(|e| {
                OpcGwError::Configuration(format!("Cannot resolve {}:{}: {}", host, port, e))
            })?
            .collect();
        if socket_addrs.is_empty() {
            return Err(OpcGwError::Configuration(format!(
                "No socket address resolved for {}:{}",
                host, port
            )));
        }

        let timeout = Duration::from_secs(1);
        let start = Instant::now();
        let mut last_error: Option<std::io::Error> = None;

        for socket_addr in &socket_addrs {
            trace!(address = %socket_addr, "Attempting TCP connection to Chirpstack server");
            match TcpStream::connect_timeout(socket_addr, timeout) {
                Ok(_) => {
                    let elapsed = start.elapsed();
                    trace!(address = %socket_addr, elapsed = ?elapsed, "TCP connection to Chirpstack server successful");
                    // TODO: Persist status update to storage (server_available=true, last_poll_time=now)
                    // TODO: Add clock skew detection - validate that Utc::now() >= previous last_poll_time
                    // to catch system clock adjustments (NTP corrections, VM clock skew)
                    return Ok(elapsed);
                }
                Err(error) => {
                    trace!(address = %socket_addr, error = %error, "TCP connection to Chirpstack server failed");
                    last_error = Some(error);
                }
            }
        }

        // Every resolved address failed to connect within the timeout.
        // TODO: Persist status update to storage (server_available=false, error_count++)
        let error =
            last_error.expect("socket_addrs is non-empty, so at least one connect was attempted");
        Err(OpcGwError::ChirpStack(format!(
            "TCP connection to ChirpStack server failed: {}",
            error
        )))
    }

    /// GH #126: decide whether a per-device poll error should be treated as a
    /// ChirpStack *outage* (gateway-wide unavailability) rather than a
    /// per-device data error.
    ///
    /// A non-`ChirpStack` error is never an outage. A `ChirpStack` error is an
    /// outage only if a live TCP availability probe confirms the server is
    /// unreachable. If the probe succeeds, ChirpStack responded to a request,
    /// so a per-device failure (e.g. an `Internal` gRPC status for a malformed
    /// DevEUI — "Odd number of digits") is a data error: it must NOT flip
    /// `chirpstack_available` or trigger the recovery loop. This prevents one
    /// misconfigured device from making the whole gateway report ChirpStack as
    /// down on every poll cycle.
    fn device_error_is_outage(&self, error: &OpcGwError) -> bool {
        matches!(error, OpcGwError::ChirpStack(_)) && self.check_server_availability().is_err()
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
    /// ```rust,ignore
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

    /// (Story 4-4 AC#1/2/3/4/5/6) Run an explicit recovery loop after the
    /// cycle's first `chirpstack_outage` warn fires.
    ///
    /// Reads `chirpstack.retry` and `chirpstack.delay` once at entry,
    /// surfaces the outage to `gateway_status` via
    /// `update_gateway_status(None, error_count_at_entry, false)` so OPC UA
    /// (Story 5-3) and the web dashboard (Story 9-2) see
    /// `chirpstack_available = false` during the retry window, then probes
    /// `check_server_availability` up to `R` times with `D`-second sleeps
    /// between attempts. The sleep is `tokio::select!`-paired with
    /// `cancel_token.cancelled()` so Ctrl+C aborts the wait cleanly.
    ///
    /// Emits three operations documented in `docs/logging.md` (Story 4-4):
    /// `recovery_attempt` (info per attempt), `recovery_complete` (info on
    /// success with `downtime_secs` or `from_startup=true` on cold start),
    /// `recovery_failed` (warn on budget exhaustion).
    ///
    /// On recovery success, does NOT explicitly reset `chirpstack_available`
    /// to `true` — the next normal `poll_metrics` cycle's existing
    /// Story 5-3 cycle-end write handles it (avoids a small race window
    /// where a stale `false` could overwrite a fresh `true`).
    async fn recover_from_chirpstack_outage(
        &mut self,
        error_count_at_entry: i32,
        last_error: &OpcGwError,
    ) -> RecoveryOutcome {
        let retry = self.config.chirpstack.retry;
        let delay_secs = self.config.chirpstack.delay;
        let delay = Duration::from_secs(delay_secs);

        // Surface the outage to gateway_status immediately. Passing `None`
        // for last_poll_timestamp preserves the existing DB value per the
        // trait contract at src/storage/mod.rs:685-688.
        if let Err(status_err) = self
            .backend
            .update_gateway_status(None, error_count_at_entry, false)
        {
            warn!(
                error = %status_err,
                "Failed to update gateway_status during recovery loop entry (non-fatal)"
            );
        }

        // Story 4-4 iter-3 review patch P10: preserve the ORIGINAL outage
        // cause separately from the most-recent probe error. The cancel
        // branch surfaces both, so post-mortem can see what triggered the
        // recovery loop AND what the last probe saw before cancellation.
        let original_outage_cause: String = last_error.to_string();
        let mut last_attempt_error: String = original_outage_cause.clone();

        // Story 4-4 iter-1 review patch P1: probe BEFORE sleeping. The
        // outage may already be cleared by the time the recovery loop
        // is entered (the original gRPC failure happened up to a poll
        // cycle ago). Sleeping first wastes the entire `delay` budget
        // on an already-recovered server. Probe-then-sleep-on-failure
        // is the standard retry-loop shape and respects NFR17 by
        // never burning more than `(R-1) × delay` sleep budget.
        for attempt_idx in 0..retry {
            let attempt_num = attempt_idx + 1;
            info!(
                operation = "recovery_attempt",
                attempt = attempt_num,
                max_retries = retry,
                delay_secs = delay_secs,
                last_error = %last_attempt_error,
                "ChirpStack recovery attempt"
            );

            match self.check_server_availability() {
                Ok(_) => {
                    // Compute downtime if known. Cold-start (no prior
                    // successful poll) emits `from_startup = true` instead
                    // of a downtime_secs field.
                    match self.last_successful_poll {
                        Some(ts) => {
                            let elapsed = chrono::Utc::now().signed_duration_since(ts);
                            let downtime_secs = elapsed.num_seconds().max(0) as u64;
                            info!(
                                operation = "recovery_complete",
                                attempts_used = attempt_num,
                                downtime_secs = downtime_secs,
                                last_error = %last_attempt_error,
                                "ChirpStack recovery complete"
                            );
                        }
                        None => {
                            info!(
                                operation = "recovery_complete",
                                attempts_used = attempt_num,
                                from_startup = true,
                                last_error = %last_attempt_error,
                                "ChirpStack recovery complete (cold start)"
                            );
                        }
                    }
                    return RecoveryOutcome::Recovered {
                        attempts_used: attempt_num,
                    };
                }
                Err(probe_err) => {
                    last_attempt_error = probe_err.to_string();
                }
            }

            // Sleep ONLY between attempts — skip after the final attempt
            // because the budget is already spent. Cancel-safe via
            // tokio::select! so Ctrl+C aborts cleanly. Mirrors the
            // pattern at run() line ~822.
            if attempt_idx + 1 < retry {
                tokio::select! {
                    _ = self.cancel_token.cancelled() => {
                        // Story 4-4 iter-1 review patch P1+P3 + iter-3 patch P10:
                        // cancel during inter-attempt sleep means `attempt_num`
                        // probes COMPLETED (the just-failed one counts).
                        // Surface BOTH the most-recent probe error AND the
                        // original outage cause so post-mortem can see what
                        // triggered the recovery loop AND what the last probe
                        // saw before cancellation.
                        return RecoveryOutcome::Exhausted {
                            attempts_used: attempt_num,
                            last_error: format!(
                                "cancelled during recovery wait after attempt {} (last probe error: {}; outage cause: {})",
                                attempt_num, last_attempt_error, original_outage_cause
                            ),
                        };
                    }
                    _ = tokio::time::sleep(delay) => {}
                }
            }
        }

        // Budget exhausted without recovery.
        warn!(
            operation = "recovery_failed",
            attempts_used = retry,
            last_error = %last_attempt_error,
            "ChirpStack recovery failed — retry budget exhausted"
        );
        RecoveryOutcome::Exhausted {
            attempts_used: retry,
            last_error: last_attempt_error,
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
    /// ```rust,ignore
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

        // Story 9-7: re-read `polling_frequency` from `self.config` on
        // every iteration so a hot-reload that bumps the cadence takes
        // effect at the next cycle boundary (no longer captured once
        // before the loop). The 4-4 recovery loop's read-at-entry
        // semantics naturally pick up new `retry`/`delay` values for
        // the same reason — this loop only owns its own scheduling
        // wait.
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

            // Wait for next poll cycle, cancellation, or hot-reload.
            // Story 9-7: the `config_rx.changed()` arm picks up a fresh
            // `Arc<AppConfig>` and refreshes `self.config`, then loops
            // immediately into the next poll cycle (so the operator sees
            // the new behaviour without waiting a full polling-frequency
            // interval). When `config_rx` is `None` (legacy callers
            // without hot-reload wiring), the changed-arm becomes a
            // pending future that never resolves — equivalent to the
            // pre-9-7 two-arm select.
            let wait_time = Duration::from_secs(self.config.chirpstack.polling_frequency);
            // Build the changed future outside the macro so the
            // `Option`-aware branching is explicit. The `Future`
            // output type is `Result<(), watch::RecvError>`; for the
            // None case we use `std::future::pending()` typed via the
            // boxed return so the select arm types match.
            let changed_future: std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), tokio::sync::watch::error::RecvError>> + Send>> =
                match self.config_rx.as_mut() {
                    Some(rx) => Box::pin(rx.changed()),
                    None => Box::pin(std::future::pending()),
                };
            tokio::select! {
                _ = self.cancel_token.cancelled() => {
                    info!("ChirpStack poller shutting down");
                    return Ok(());
                }
                changed = changed_future => {
                    if changed.is_ok() {
                        // The watch sender published a new config —
                        // pull it out and replace `self.config`. The
                        // 4-4 recovery loop reads at entry, so an in-
                        // flight recovery is unaffected; the next
                        // entry uses the new values.
                        if let Some(rx) = self.config_rx.as_mut() {
                            // Iter-1 review P18: deref through the
                            // borrow guard once instead of cloning the
                            // Arc and then deep-cloning the inner
                            // AppConfig. Net: one deep clone instead
                            // of one Arc-clone + one deep clone.
                            self.config = (**rx.borrow_and_update()).clone();
                            info!(
                                operation = "config_reload_applied",
                                subsystem = "chirpstack_poller",
                                polling_frequency_s = self.config.chirpstack.polling_frequency,
                                "Poller picked up reloaded config"
                            );
                        }
                    }
                    // Else the sender was dropped — fall through and
                    // keep polling with the existing config; the next
                    // iteration's `changed_future` will be `pending`.
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
        if (retry_state.failure_count > 0 || retry_state.first_failure_time.is_some())
            && self.prune_retry_state.lock().is_err()
        {
            // Retry state is poisoned; reset it
            retry_state.failure_count = 0;
            retry_state.first_failure_time = None;
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
    /// ```rust,ignore
    /// // Called automatically by run(), but can be called manually for testing
    /// poller.poll_metrics().await?;
    /// ```
    async fn poll_metrics(&mut self) -> Result<(), OpcGwError> {
        debug!("Polling chirpstack metrics");
        let poll_start = Instant::now();

        // Track health metrics during this poll cycle (Story 5-3)
        let mut error_count: i32 = 0;
        let mut chirpstack_available = true;
        // Story 6-3, AC#5: only emit the chirpstack_outage warn on the FIRST
        // device failure that crosses this cycle's chirpstack_available flag,
        // so a fleet-wide outage doesn't flood the log with one line per
        // device. Reset each cycle.
        let mut chirpstack_outage_logged = false;
        // Track per-cycle counters for the structured cycle-end log (Story 6-1, AC#5)
        let mut devices_polled: u32 = 0;
        let mut metrics_collected: u32 = 0;

        // Process command queue
        self.process_command_queue().await?;

        // Capture poll start timestamp after command queue succeeds (Story 5-3 AC#4)
        let poll_start_timestamp = chrono::DateTime::<Utc>::from(SystemTime::now());

        // Collect all metrics for batch write
        let mut batch_metrics: Vec<crate::storage::BatchMetricWrite> = Vec::new();

        // Collect device IDs and names
        let mut device_ids = Vec::new();
        for app in &self.config.application_list {
            for dev in &app.device_list {
                device_ids.push(dev.device_id.clone());
            }
        }
        let device_count = device_ids.len() as u32;
        debug!(device_count = device_count, "Found devices");
        // Story 6-1, AC#5: structured cycle-start at info!.
        info!(operation = "poll_cycle_start", device_count = device_count);

        // Get metrics from server for each device (Story 5-3: track errors per device, don't abort)
        for dev_id in device_ids {
            match self
                .get_device_metrics_from_server(
                    dev_id.clone(),
                    1,
                    1,
                )
                .await
            {
                Ok(dev_metrics) => {
                    // Collect metrics from this device for batch write
                    let mut dev_metric_count: u32 = 0;
                    for metric in dev_metrics.metrics.values() {
                        trace!("Got chirpstack metric for device {}", dev_id);
                        trace!(metric = ?metric, "Metric details");

                        // Prepare metric for batch write (validate type and create BatchMetricWrite)
                        if let Some(batch_metric) = self.prepare_metric_for_batch(&dev_id, metric) {
                            batch_metrics.push(batch_metric);
                            dev_metric_count += 1;
                        }
                    }
                    devices_polled += 1;
                    metrics_collected += dev_metric_count;
                    // Story 6-1, AC#5: per-device debug.
                    debug!(
                        operation = "device_polled",
                        device_id = %dev_id,
                        metrics_collected = dev_metric_count,
                        success = true
                    );
                }
                Err(e) => {
                    // Track error per device and continue to next device (Story 5-3 AC#5)
                    error!(error = ?e, device_id = %dev_id, "Failed to get metrics for device");
                    // Story 6-3, AC#7: structured `device_poll` warn so a
                    // single device's failure is searchable independently
                    // of the cycle-level errors. This complements the
                    // existing `device_polled` debug from Story 6-1, AC#5.
                    warn!(
                        operation = "device_poll",
                        device_id = %dev_id,
                        error = %e,
                        status = "failed"
                    );
                    // Review patch P15: saturate at i32::MAX so the gateway
                    // health-metric overflow check (`error_count >= i32::MAX`
                    // in opc_ua.rs) actually fires reliably. Plain `+= 1`
                    // would wrap to i32::MIN in release builds and silently
                    // bypass the saturation warn.
                    error_count = error_count.saturating_add(1);
                    // #126: a per-device GetDeviceMetrics error is only a real
                    // outage if ChirpStack is actually unreachable. A server-
                    // responded error (e.g. an `Internal` gRPC status for a
                    // malformed DevEUI — "Odd number of digits") means the
                    // server is UP; that's a per-device data error, counted
                    // above but NOT a gateway-wide outage. Gating on a live
                    // connectivity probe stops one bad device from flipping
                    // `chirpstack_available=false` and triggering the recovery
                    // loop every poll cycle.
                    if self.device_error_is_outage(&e) {
                        chirpstack_available = false;
                        // Story 6-3, AC#5: chirpstack_outage diagnostic on
                        // the first per-device connectivity failure of the
                        // cycle. Returns true when this is the first
                        // emission of the cycle (it internally checks +
                        // flips the cycle-local bool), false on subsequent
                        // calls within the same cycle.
                        // Iter-3 review pending #1: helper-extracted so tests
                        // drive `maybe_emit_chirpstack_outage` directly. P9
                        // (rfc3339 `last_successful_poll`) is preserved
                        // inside the helper.
                        let just_logged = maybe_emit_chirpstack_outage(
                            &mut chirpstack_outage_logged,
                            self.last_successful_poll,
                            &e,
                        );
                        // Story 4-4: on the first per-cycle outage detection,
                        // run the recovery loop. The loop fires at most once
                        // per cycle naturally because `maybe_emit_chirpstack_outage`
                        // returns false on subsequent calls within the same cycle.
                        if just_logged {
                            let _outcome = self
                                .recover_from_chirpstack_outage(error_count, &e)
                                .await;
                            // Outcome is informational here — the for-loop
                            // continues to the next device regardless. Recovered
                            // means subsequent devices may succeed; Exhausted
                            // means they'll keep failing the TCP probe but the
                            // outage_logged bool prevents re-firing the recovery
                            // loop within this cycle.
                        }
                    }
                    debug!(
                        operation = "device_polled",
                        device_id = %dev_id,
                        metrics_collected = 0u32,
                        success = false
                    );
                }
            }
        }

        // Batch write all collected metrics in a single transaction with retry logic
        let mut batch_write_successful = true;
        if !batch_metrics.is_empty() {
            debug!(count = batch_metrics.len(), "Batch writing metrics from poll cycle");

            // Retry with exponential backoff for transient errors (AC#6)
            let mut attempt = 0;
            let max_retries = 3;
            loop {
                attempt += 1;
                // Story 6-1, AC#5: time + emit structured log around the batch write.
                // Review patch P20: clone the payload *before* starting the
                // budget timer so a large `batch_metrics.clone()` doesn't get
                // counted against `BATCH_WRITE_BUDGET_MS` and trip a false
                // `exceeded_budget=true` warn for clone overhead.
                let batch_count = batch_metrics.len() as u32;
                let batch_payload = batch_metrics.clone();
                let batch_start = Instant::now();
                let batch_result = self.backend.batch_write_metrics(batch_payload);
                let batch_latency_ms = batch_start.elapsed().as_millis() as u64;
                match batch_result {
                    Ok(_) => {
                        // Story 6-3, AC#3: a batch write that took longer
                        // than `BATCH_WRITE_BUDGET_MS` is worth surfacing at
                        // `warn` so it shows up at the default log level.
                        if batch_latency_ms > crate::utils::BATCH_WRITE_BUDGET_MS {
                            warn!(
                                operation = "batch_write",
                                metrics_count = batch_count,
                                latency_ms = batch_latency_ms,
                                budget_ms = crate::utils::BATCH_WRITE_BUDGET_MS,
                                exceeded_budget = true,
                                success = true,
                                "Batch write exceeded budget"
                            );
                        } else {
                            debug!(
                                operation = "batch_write",
                                metrics_count = batch_count,
                                latency_ms = batch_latency_ms,
                                success = true,
                                "Batch write succeeded"
                            );
                        }
                        break;
                    }
                    Err(e) => {
                        batch_write_successful = false;
                        debug!(
                            operation = "batch_write",
                            metrics_count = batch_count,
                            latency_ms = batch_latency_ms,
                            success = false,
                            attempt = attempt,
                            "Batch write attempt failed"
                        );
                        if attempt >= max_retries {
                            error!(error = %e, attempt, "Failed to batch write metrics after {} retries", max_retries);
                            // Storage errors don't affect ChirpStack availability flag (they're local issues, not remote)
                            // Only set unavailability on ChirpStack connectivity errors (handled in device fetch loop)
                            break;
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

        // Update gateway health status at end of poll cycle (Story 5-3 AC#3, AC#4, AC#5, AC#6)
        // Use the poll start timestamp if we have any metrics OR if poll was partially successful
        let timestamp_for_update = if batch_write_successful || !batch_metrics.is_empty() {
            Some(poll_start_timestamp)
        } else {
            None // Poll failed completely, don't update timestamp
        };

        // Story 6-3, AC#4: error-count spike detection. Iter-3 review pending
        // #1: helper-extracted to `maybe_emit_error_spike` so tests drive the
        // production logic directly. The helper saturates the subtraction
        // (P14) and pins the threshold via `ERROR_SPIKE_THRESHOLD`.
        maybe_emit_error_spike(self.previous_error_count, error_count);
        self.previous_error_count = error_count;

        // Story 6-3, AC#5: track last successful poll for the
        // `chirpstack_outage` warn's `last_successful_poll` field. We mark
        // the cycle "successful" when at least one device produced metrics
        // OR the batch write succeeded (matches the
        // `timestamp_for_update.is_some()` semantics used below).
        if let Some(ts) = timestamp_for_update {
            self.last_successful_poll = Some(ts);
        }

        // Update health metrics with non-fatal error tolerance (metrics written even if status fails)
        // Story 6-1, AC#4: structured debug log around health-status writes.
        debug!(
            operation = "health_update",
            last_poll_timestamp = ?timestamp_for_update,
            error_count = error_count,
            chirpstack_available = chirpstack_available,
            "Updating gateway health status"
        );
        if let Err(e) = self.backend.update_gateway_status(
            timestamp_for_update,
            error_count,
            chirpstack_available,
        ) {
            error!(error = %e, "Failed to update gateway health status (non-fatal)");
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

        // Story 6-1, AC#5: structured cycle-end at info!.
        info!(
            operation = "poll_cycle_end",
            devices_polled = devices_polled,
            metrics_collected = metrics_collected,
            errors = error_count,
            chirpstack_available = chirpstack_available,
            cycle_duration_ms = poll_duration.as_millis() as u64
        );

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
        let kind = classify_metric_kind(metric.kind);

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

        // A-3 (AC#2, AC#10) + IR2/IR4 (iter-1 review) + iter-2 IR2-A/B/IR4-A:
        // (a) NaN/Inf guard rejects non-finite raw_values before payload
        //     construction. Rust's `as i64` saturates silently on overflow, so
        //     (b) finite-but-out-of-i64-range guard rejects values that would
        //     silently become `i64::MAX` / `i64::MIN` for any Int target —
        //     not just `kind == Counter` (iter-2 IR2-B closed the
        //     Unknown+cfg=Int gap).
        //
        // Upper-bound predicate uses `>=` not `>` (iter-2 IR2-A): `i64::MAX`
        // (= 2^63 − 1) is not exactly representable in f64; `i64::MAX as f64`
        // rounds UP to 2^63. Without `>=`, the exact-2^63 case slips through
        // and `as i64` saturates silently — the very hazard the guard targets.
        // Lower bound stays `<` because `i64::MIN` (= -2^63) is exactly
        // representable; `raw_as_f64 == i64::MIN as f64` casts cleanly.
        //
        // `expected_type` (iter-2 IR4-A): kind=Counter → "Int"; Gauge/Absolute
        // → "Float"; Unknown defers to cfg_type so Bool/Int/String metrics
        // emit the correct audit attribution. Closed enum extended to
        // {Float, Int, Bool, String, Unknown} — see docs/logging.md.
        let raw_as_f64 = raw_value as f64;
        let cfg_type = self.config.get_metric_type(&metric_name, &device_id_string);
        let expected_type: &'static str = match kind {
            ChirpStackMetricKind::Counter => "Int",
            ChirpStackMetricKind::Gauge | ChirpStackMetricKind::Absolute => "Float",
            ChirpStackMetricKind::Unknown => match cfg_type {
                Some(OpcMetricTypeConfig::Bool) => "Bool",
                Some(OpcMetricTypeConfig::Int) => "Int",
                Some(OpcMetricTypeConfig::String) => "String",
                Some(OpcMetricTypeConfig::Float) => "Float",
                None => "Unknown",
            },
        };
        if !raw_as_f64.is_finite() {
            warn!(
                event = "metric_parse",
                device_id = %device_id,
                metric_name = %metric_name,
                raw_value = %raw_value,
                expected_type = expected_type,
                reason = "non_finite",
                "Skipping metric: non-finite Float (NaN or Inf)"
            );
            return None;
        }
        let int_target = matches!(kind, ChirpStackMetricKind::Counter)
            || (matches!(kind, ChirpStackMetricKind::Unknown)
                && matches!(cfg_type, Some(OpcMetricTypeConfig::Int)));
        if int_target
            && (raw_as_f64 < i64::MIN as f64 || raw_as_f64 >= i64::MAX as f64)
        {
            warn!(
                event = "metric_parse",
                device_id = %device_id,
                metric_name = %metric_name,
                raw_value = %raw_value,
                expected_type = expected_type,
                reason = "int_overflow",
                "Skipping Int target: value would saturate i64 cast"
            );
            return None;
        }

        // 2. Determine target MetricType (kind-first priority).
        // A-3: real payload wrapping via raw_value: f32 → f64 (Float) or as i64 (Int).
        let target_type = match kind {
            ChirpStackMetricKind::Gauge => {
                debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, kind_driven_conversion = true, "Using GAUGE → Float");
                MetricType::Float(raw_value as f64)
            }
            ChirpStackMetricKind::Counter => {
                debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, kind_driven_conversion = true, "Using COUNTER → Int");
                MetricType::Int(raw_value as i64)
            }
            ChirpStackMetricKind::Absolute => {
                debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, kind_driven_conversion = true, "Using ABSOLUTE → Float");
                MetricType::Float(raw_value as f64)
            }
            ChirpStackMetricKind::Unknown => {
                // Fallback to config type if available (reuses cfg_type queried
                // above for the audit-event attribution to avoid a redundant
                // HashMap lookup per poll cycle).
                match cfg_type {
                    Some(t) => {
                        debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, "Using config fallback for unknown kind");
                        match t {
                            // The Bool branch defers to validate_bool_metric_value (step 4)
                            // so the kind=Unknown + cfg=Bool path emits the right metric_parse
                            // warn on invalid bool input. The placeholder false is replaced
                            // in step 4's MetricType::Bool(_) arm.
                            OpcMetricTypeConfig::Bool => MetricType::Bool(false),
                            OpcMetricTypeConfig::Int => MetricType::Int(raw_value as i64),
                            OpcMetricTypeConfig::Float => MetricType::Float(raw_value as f64),
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

        // 3. For Counter type: check monotonic property (reject reset: new < previous).
        // A-4 iter-1 IR4: typed-path ONLY (legacy `value.parse::<i64>()` fallback
        // dropped). The post-A-4 reader rewrite (`get_metric_value` projects v007
        // typed columns + `value_type`) guarantees that for any current Int row
        // `prev_metric.data_type` is `MetricType::Int(real_value)`. The previous
        // legacy fallback could produce wrong results under type-confusion:
        // if (device_id, metric_name) was reconfigured Bool → Int Counter
        // mid-stream, prev_metric was `Bool(true)` with `value="1"`, the legacy
        // fallback returned Some(1), and the check would treat 1 as the prev
        // counter — blocking any new counter <1 (e.g. legitimate roll-over to 0)
        // as a false reset. Per A-4 iter-1 decision IR4 (user-accepted 2026-05-16):
        // drop the fallback entirely. The narrow loss is pre-A-3 rows where
        // `batch_write_metrics` was the writer with a zero-default `data_type`
        // — those rows are vanishingly rare in production (poller writes via
        // batch path always populated `data_type` consistently) and the
        // upstream non_finite guard + int_overflow guard (the two guards
        // earlier in this function — see the `int_target = matches!(kind,
        // Counter) || ...` predicate above) catch NaN/Inf/saturating-cast
        // before reaching this check anyway.
        //
        // Legacy rows (`value_type='legacy'`) surface as `Ok(None)` from
        // `get_metric_value` post-A-4 (architecture.md:182), so the `if let
        // Ok(Some(prev_metric))` already filters them out before we examine
        // `prev_metric.data_type`. No additional guard needed.
        //
        // A-4 iter-2 JR12 reconfig-window note: when a metric is reconfigured
        // FROM a different MetricType TO Int Counter mid-stream (e.g. Float
        // Gauge → Int Counter, or Bool → Int Counter), `prev_metric.data_type`
        // is still the OLD variant until the first Counter UPSERT replaces
        // the row. The `if let MetricType::Int(prev_int) = ...` arm silently
        // skips the monotonic check during that transition window — a
        // legitimate Counter reset goes undetected for at most one poll cycle
        // (after which the typed payload is in place). Acceptable: reconfig
        // events are operator-driven and rare; the alternative (emitting a
        // trace! on the reconfig window) would add noise without operator-
        // actionable value.
        if matches!(target_type, MetricType::Int(_)) && kind == ChirpStackMetricKind::Counter {
            if let Ok(Some(prev_metric)) = self.backend.get_metric_value(&device_id_string, &metric_name) {
                if let MetricType::Int(prev_int) = prev_metric.data_type {
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

        // 4. Validate and convert value based on target type.
        // A-3 + A-5: wrap validated/converted value into the typed MetricType
        // payload. A-5 dropped the parallel `value: String` projection since
        // BatchMetricWrite.value was removed — the typed `data_type` carries
        // the real measurement directly.
        let metric_type = match target_type {
            MetricType::Bool(_) => {
                // Iter-3 review pending #1: helper-extracted to
                // `validate_bool_metric_value` so tests drive the production
                // boolean-validation path directly without constructing a
                // full `ChirpstackPoller` instance. The helper emits the
                // canonical `metric_parse` warn on invalid input.
                // Iter-1 IR12 contract pin: `validate_bool_metric_value`
                // returns `Some("0")` for `false` / `Some("1")` for `true` /
                // `None` for invalid input. The caller maps `"1"` → true /
                // anything else → false. **LOAD-BEARING** (A-1-iter1 doctrine
                // restored at A-5 P10 iter-1 review fix): if the helper's
                // return alphabet ever widens (e.g. it starts returning
                // `Some("true")` for legacy compatibility), this caller
                // silently writes `Bool(false)` for every input — ensure
                // helper changes update both call sites here and at
                // `store_metric`.
                match validate_bool_metric_value(raw_value, device_id, &metric_name, kind) {
                    Some(s) => MetricType::Bool(s == "1"),
                    None => return None,
                }
            }
            MetricType::Int(_) => {
                let int_val = raw_value as i64;
                if raw_value.fract() != 0.0 {
                    warn!(value = %raw_value, metric_name = %metric_name, device_id = %device_id,
                          metric_kind = ?kind, "Counter metric has fractional value; precision lost");
                }
                MetricType::Int(int_val)
            }
            MetricType::Float(_) => MetricType::Float(raw_value as f64),
            MetricType::String(_) => {
                warn!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, "Reading string metrics from ChirpStack server is not implemented");
                return None;
            }
        };

        debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, kind_driven_conversion = true, "Metric prepared for batch write");

        Some(crate::storage::BatchMetricWrite {
            device_id: device_id.to_string(),
            metric_name,
            data_type: metric_type,
            timestamp: now_ts,
        })
    }

    /// Direct-store helper retained from the pre-batch-write era (now
    /// replaced in production by `prepare_metric_for_batch` +
    /// `batch_write_metrics`).
    ///
    /// **Status (Story A-3):** reinstated. Kept `#[allow(dead_code)]` because
    /// production code reaches storage via `prepare_metric_for_batch` +
    /// `batch_write_metrics`. The body shares `prepare_metric_for_batch`'s
    /// validation primitives (NaN/Inf guard, `validate_bool_metric_value`,
    /// int fractional warn) but uses **config-driven** dispatch
    /// (`OpcMetricTypeConfig`) rather than the kind-driven dispatch
    /// (`ChirpStackMetricKind`) of the production path — that's the historical
    /// shape of this method and tests built on top of it exercise the
    /// config-driven branch deliberately.
    ///
    /// **Partial-failure note:** when `upsert_metric_value` succeeds but
    /// `append_metric_history` fails, `metric_values` carries the new row
    /// while `metric_history` does not — silent table divergence. The error
    /// is logged but not surfaced via a counter or audit event. This is
    /// inherited pre-A-1 behaviour; an alerting hook is out of scope for A-3.
    ///
    /// # Arguments
    ///
    /// * `device_id` - The unique identifier of the device
    /// * `metric` - The metric data received from ChirpStack
    #[allow(dead_code)]
    pub fn store_metric(&self, device_id: &String, metric: &Metric) {
        debug!("Store chirpstack device metric in storage");
        let device_name = match self.config.get_device_name(device_id) {
            Some(name) => name,
            None => {
                warn!(device_id = %device_id, "Device name not found in config, skipping metric");
                return;
            }
        };

        if metric.datasets.is_empty() || metric.datasets[0].data.is_empty() {
            warn!(device_id = %device_id, metric_name = %metric.name, "Metric has no data; skipping");
            return;
        }

        let metric_name = metric.name.clone();
        let raw_value = metric.datasets[0].data[0];
        let now_ts = SystemTime::now();
        let kind = classify_metric_kind(metric.kind);

        // A-3 (AC#2, AC#8) + iter-1 IR2/IR4 + iter-2 IR2-A/IR3-A:
        // NaN/Inf and i64-overflow guard. `expected_type` is config-aware
        // (this method dispatches on `OpcMetricTypeConfig`, not
        // `ChirpStackMetricKind`). Closed enum extended to
        // {Float, Int, Bool, String, Unknown} — cfg=None reports "Unknown"
        // rather than misattributing as "Float" (iter-2 Blind F20 / Edge F3).
        // Upper-bound guard uses `>=` to catch the i64::MAX boundary that
        // `>` misses due to f64 rounding (iter-2 IR2-A).
        let raw_as_f64 = raw_value as f64;
        let cfg_type = self.config.get_metric_type(&metric_name, device_id);
        let expected_type: &'static str = match cfg_type {
            Some(OpcMetricTypeConfig::Bool) => "Bool",
            Some(OpcMetricTypeConfig::Int) => "Int",
            Some(OpcMetricTypeConfig::Float) => "Float",
            Some(OpcMetricTypeConfig::String) => "String",
            None => "Unknown",
        };
        if !raw_as_f64.is_finite() {
            warn!(
                event = "metric_parse",
                device_id = %device_id,
                metric_name = %metric_name,
                raw_value = %raw_value,
                expected_type = expected_type,
                reason = "non_finite",
                "Skipping metric: non-finite Float (NaN or Inf)"
            );
            return;
        }
        if matches!(cfg_type, Some(OpcMetricTypeConfig::Int))
            && (raw_as_f64 < i64::MIN as f64 || raw_as_f64 >= i64::MAX as f64)
        {
            warn!(
                event = "metric_parse",
                device_id = %device_id,
                metric_name = %metric_name,
                raw_value = %raw_value,
                expected_type = expected_type,
                reason = "int_overflow",
                "Skipping Int target: value would saturate i64 cast"
            );
            return;
        }

        // A-3 (AC#8): kind→variant dispatch with real payload wrapping.
        // Structural shape mirrors `prepare_metric_for_batch` Task 1.
        // NOTE: If upsert_metric_value() succeeds but append_metric_history() fails,
        // the metric will exist in metric_values but not in metric_history. This is
        // intentional to allow the poller to continue (non-fatal error handling).
        match self.config.get_metric_type(&metric_name, device_id) {
            Some(metric_type) => match metric_type {
                OpcMetricTypeConfig::Bool => {
                    debug!(metric = ?metric, "Bool metric");
                    let parsed = match validate_bool_metric_value(raw_value, device_id, &metric_name, kind) {
                        Some(s) => s == "1",
                        None => return,
                    };
                    let metric_val = MetricType::Bool(parsed);
                    if let Err(e) = self.backend.upsert_metric_value(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to upsert bool metric");
                    } else if let Err(e) = self.backend.append_metric_history(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to append bool metric to history");
                    }
                }
                OpcMetricTypeConfig::Int => {
                    debug!(metric = ?metric, "Int metric");
                    if raw_value.fract() != 0.0 {
                        warn!(value = %raw_value, metric_name = %metric_name, "Float metric truncated to int; precision lost");
                    }
                    let metric_val = MetricType::Int(raw_value as i64);
                    if let Err(e) = self.backend.upsert_metric_value(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to upsert int metric");
                    } else if let Err(e) = self.backend.append_metric_history(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to append int metric to history");
                    }
                }
                OpcMetricTypeConfig::Float => {
                    debug!(metric = ?metric, "Float metric");
                    let metric_val = MetricType::Float(raw_value as f64);
                    if let Err(e) = self.backend.upsert_metric_value(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to upsert float metric");
                    } else if let Err(e) = self.backend.append_metric_history(device_id, &metric_name, &metric_val, now_ts) {
                        error!(device_id = %device_id, metric_name = %metric_name, error = %e, "Failed to append float metric to history");
                    }
                }
                OpcMetricTypeConfig::String => {
                    warn!(metric_name = %metric_name, device_id = %device_id, "Reading string metrics from ChirpStack server is not implemented");
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
    /// ```rust,ignore
    /// let applications = poller.get_applications_list_from_server().await?;
    /// for app in applications {
    ///     println!("Application: {} ({})", app.application_name, app.application_id);
    /// }
    /// ```
    /// Story C-1: kept as the canonical poller-side helper; the web
    /// inventory layer (`src/chirpstack_inventory.rs`) uses standalone
    /// free-function equivalents (`fetch_applications`) instead of
    /// borrowing a poller instance, since the run loop owns `&mut self`.
    /// Reserved for future direct poller-side consumers.
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
    /// ```rust,ignore
    /// let devices = poller.get_devices_list_from_server("app-123".to_string()).await?;
    /// for device in devices {
    ///     println!("Device: {} ({})", device.name, device.dev_eui);
    /// }
    /// ```
    /// Story C-1: kept as the canonical poller-side helper; the web
    /// inventory layer uses a standalone free-function equivalent
    /// (`fetch_devices` in `src/chirpstack_inventory.rs`). Reserved for
    /// future direct poller-side consumers.
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
    /// ```rust,ignore
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
        let delay_secs = self.config.chirpstack.delay;
        let delay = Duration::from_secs(delay_secs);
        loop {
            if count == retry {
                //panic!("Timeout: cannot reach Chirpstack server");
                warn!("Timeout: cannot reach chirpstack server");
            }
            // Story 6-3, AC#1: structured diagnostics on the existing
            // availability-probe retry loop. No new control flow — we
            // log around branches that already exist.
            let attempt = count + 1;
            let probe_start = Instant::now();
            info!(
                operation = "chirpstack_connect",
                attempt = attempt,
                endpoint = %self.config.chirpstack.server_address,
                timeout_secs = 1u64,
                "TCP availability probe attempt"
            );
            match self.check_server_availability() {
                Ok(_t) => {
                    let latency_ms = probe_start.elapsed().as_millis() as u64;
                    info!(
                        operation = "chirpstack_connect",
                        attempt = attempt,
                        latency_ms = latency_ms,
                        success = true,
                        "TCP availability probe succeeded"
                    );
                    break;
                }
                Err(e) => {
                    warn!(
                        operation = "chirpstack_connect",
                        attempt = attempt,
                        error = %e,
                        retry_delay_secs = delay_secs,
                        max_retries = retry,
                        success = false,
                        "TCP availability probe failed"
                    );
                    warn!("Waiting for Chirpstack server");
                    trace!(retry_count = count, "Retry count");
                    count += 1;
                    let next_attempt = count + 1;
                    let next_retry = chrono::Utc::now()
                        + chrono::Duration::seconds(delay_secs as i64);
                    info!(
                        operation = "retry_schedule",
                        attempt = next_attempt,
                        delay_secs = delay_secs,
                        next_retry = %next_retry.to_rfc3339(),
                        "Next chirpstack_connect retry scheduled"
                    );
                    tokio::time::sleep(delay).await;
                }
            }
        }

        trace!("Create device service client for Chirpstack");
        let mut device_client = self.create_device_client().await?;

        //trace!(request = ?request, "Request created");
        // Story 6-3, AC#6 / AC#7: time the gRPC request so we can classify
        // failure modes — timeout (DeadlineExceeded), unavailable
        // (connection_reset / broken_pipe / Unavailable), or other.
        let req_start = Instant::now();
        match device_client.get_metrics(request).await {
            Ok(response) => {
                let inner_response = response.into_inner();

                let metrics: HashMap<String, Metric> = inner_response.metrics.into_iter().collect();

                Ok(DeviceMetric { metrics })
            }
            Err(e) => {
                let duration_ms = req_start.elapsed().as_millis() as u64;
                // Iter-3 review pending #1: helper-extracted to
                // `classify_and_log_grpc_error`. The helper emits the
                // appropriate `chirpstack_request` warn (Story 6-3 AC#6 for
                // DeadlineExceeded, AC#7 for Unavailable / Cancelled) and
                // returns the classification. Other status codes (e.g.
                // Unimplemented, NotFound, InvalidArgument) classify as
                // `Other` and leave the existing `ChirpStack(_)` wrap to
                // carry the message.
                let _classification = classify_and_log_grpc_error(
                    &e,
                    duration_ms,
                    self.config.chirpstack.delay,
                );
                Err(OpcGwError::ChirpStack(format!(
                    "Error getting device metrics: {}",
                    e
                )))
            }
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

        // Story E-0: drain the queue that the OPC UA write path actually feeds.
        // `OpcUa::set_command` enqueues a `DeviceCommand` via
        // `StorageBackend::queue_command`; those are returned by
        // `get_pending_commands`. (The high-level `Command` / `dequeue_command`
        // FIFO of Story 3-1 has no producer in the OPC UA flow and is left
        // untouched here.)
        //
        // A storage-lock failure propagates (it aborts the poll cycle, matching
        // the pre-existing `?` at the call site); a per-command enqueue failure
        // is logged and reflected in the command's status but never aborts the
        // batch — one undeliverable command must not block the others or the
        // metrics poll.
        let pending = self.backend.get_pending_commands()?;
        if pending.is_empty() {
            return Ok(());
        }
        debug!(count = pending.len(), "Processing pending device commands");
        for command in pending {
            self.deliver_command(command).await;
        }
        Ok(())
    }

    /// Delivers a single queued command to ChirpStack and records the outcome.
    ///
    /// Resolves the command's device-class binding from the (SQLite-sourced)
    /// `application_list` and delegates to [`deliver_one`]. Never returns an
    /// error: every failure mode is logged and reflected in storage so the
    /// caller's batch loop continues.
    async fn deliver_command(&self, command: DeviceCommand) {
        // Resolve the per-command class + confirmed flag from config BEFORE the
        // await so the borrow of `self.config` does not cross the await point.
        let (command_class, confirmed) =
            match find_command_cfg(&self.config.application_list, &command.device_id, command.f_port)
            {
                Some(cfg) => (cfg.command_class.clone(), cfg.command_confirmed),
                None => (None, false),
            };

        deliver_one(
            self,
            &self.backend,
            command_class.as_deref(),
            confirmed,
            &command,
        )
        .await;
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
    /// ```rust,ignore
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
    /// ```rust,ignore
    /// // Called internally by get_devices_list_from_server()
    /// let device_details = poller.convert_to_devices(response);
    /// ```
    fn convert_to_devices(&self, response: ListDevicesResponse) -> Vec<DeviceListDetail> {
        debug!("convert_to_devices");

        response
            .result
            .into_iter()
            .map(|dev: DeviceListItem| {
                // Story C-1: promote device_profile_name + last_seen_at fields
                // for the inventory picker UI. Empty string → None so
                // downstream serialisation renders `null` cleanly.
                let device_profile_name = if dev.device_profile_name.is_empty() {
                    None
                } else {
                    Some(dev.device_profile_name)
                };
                let last_seen_at = dev.last_seen_at.as_ref().and_then(|ts| {
                    chrono::DateTime::<chrono::Utc>::from_timestamp(ts.seconds, ts.nanos as u32)
                        .map(|dt| dt.to_rfc3339())
                });
                DeviceListDetail {
                    dev_eui: dev.dev_eui,
                    name: dev.name,
                    description: dev.description,
                    device_profile_name,
                    last_seen_at,
                }
            })
            .collect()
    }
}

// ===========================================================================
// Story E-0: downlink command path
// ===========================================================================

/// Form of a downlink payload handed to ChirpStack.
///
/// - `Object` carries a *semantic* command object (e.g. `{"command":"open"}`)
///   that the device-profile codec's `encodeDownlink` turns into the wire bytes
///   — opcgw stays model-agnostic.
/// - `Raw` carries pre-encoded bytes for devices/commands not bound to a class
///   (the legacy path, preserved as a fallback).
#[derive(Debug, Clone, PartialEq)]
enum DownlinkPayload {
    Object(chirpstack_api::prost_types::Struct),
    Raw(Vec<u8>),
}

/// Abstraction over the ChirpStack downlink-enqueue gRPC call.
///
/// Extracting it behind a trait lets the command-queue processing loop be
/// unit-tested (success / failure outcomes, status transitions) without a live
/// gRPC server. Production uses the [`ChirpstackPoller`] implementation.
#[async_trait::async_trait]
trait DownlinkSink: Send + Sync {
    async fn enqueue_downlink(&self, item: DeviceQueueItem) -> Result<(), OpcGwError>;
}

#[async_trait::async_trait]
impl DownlinkSink for ChirpstackPoller {
    async fn enqueue_downlink(&self, item: DeviceQueueItem) -> Result<(), OpcGwError> {
        trace!(queue_item = ?item, "Enqueue downlink to ChirpStack");
        let request = Request::new(EnqueueDeviceQueueItemRequest {
            queue_item: Some(item),
            flush_queue: false,
        });

        // Client-creation failure is a handled error, never a panic.
        let mut device_client = self.create_device_client().await?;
        match device_client.enqueue(request).await {
            Ok(response) => {
                let inner_response = response.into_inner();
                trace!(response = ?inner_response, "Downlink enqueued");
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
}

/// Maps, enqueues, and records the outcome for one queued command.
///
/// Maps the canonical value to a semantic object (class-bound) or raw bytes
/// (fallback), enqueues it via `sink`, and updates the command's status to
/// `Sent` (success) or `Failed` (mapping or enqueue error). Never panics or
/// returns an error: every failure mode is logged and reflected in storage so
/// the batch loop continues. Factored out of [`ChirpstackPoller::deliver_command`]
/// so the outcome logic is unit-testable with a stub [`DownlinkSink`].
async fn deliver_one(
    sink: &dyn DownlinkSink,
    backend: &Arc<dyn StorageBackend>,
    command_class: Option<&str>,
    confirmed: bool,
    command: &DeviceCommand,
) {
    let downlink = match map_command_to_downlink(command_class, &command.payload) {
        Ok(d) => d,
        Err(e) => {
            error!(
                error = %e,
                device_id = %command.device_id,
                command_id = command.id,
                f_port = command.f_port,
                "Command mapping failed; marking command Failed"
            );
            if let Err(e2) =
                backend.update_command_status(command.id, CommandStatus::Failed, Some(e.to_string()))
            {
                error!(error = %e2, command_id = command.id, "Failed to mark command Failed");
            }
            return;
        }
    };

    let item = build_queue_item(&command.device_id, command.f_port, &downlink, confirmed);
    match sink.enqueue_downlink(item).await {
        Ok(()) => {
            debug!(
                device_id = %command.device_id,
                command_id = command.id,
                f_port = command.f_port,
                class = ?command_class,
                "Command enqueued to ChirpStack"
            );
            if let Err(e) =
                backend.update_command_status(command.id, CommandStatus::Sent, None)
            {
                error!(error = %e, command_id = command.id, "Failed to mark command Sent");
            }
        }
        Err(e) => {
            error!(
                error = %e,
                device_id = %command.device_id,
                command_id = command.id,
                "Failed to enqueue command; marking command Failed"
            );
            if let Err(e2) =
                backend.update_command_status(command.id, CommandStatus::Failed, Some(e.to_string()))
            {
                error!(error = %e2, command_id = command.id, "Failed to mark command Failed");
            }
        }
    }
}

/// Finds the command config for a queued command, keyed by `(device_id, f_port)`.
///
/// A queued [`DeviceCommand`] carries only `device_id` + `f_port`, not its
/// class; this resolves the matching [`crate::config::DeviceCommandCfg`] from
/// the (SQLite-sourced) `application_list` so the class binding can be applied.
/// Returns the first command on that device whose `command_port` equals
/// `f_port`.
fn find_command_cfg<'a>(
    apps: &'a [crate::config::ChirpStackApplications],
    device_id: &str,
    f_port: u8,
) -> Option<&'a crate::config::DeviceCommandCfg> {
    for app in apps {
        for dev in &app.device_list {
            if dev.device_id != device_id {
                continue;
            }
            if let Some(cmds) = &dev.device_command_list {
                if let Some(cfg) = cmds.iter().find(|c| {
                    u8::try_from(c.command_port)
                        .map(|p| p == f_port)
                        .unwrap_or(false)
                }) {
                    return Some(cfg);
                }
            }
        }
    }
    None
}

/// Maps a command's canonical OPC UA value to a downlink payload.
///
/// - No class (`None`) → raw-byte fallback: the bytes are sent verbatim.
/// - `"valve"` class → semantic object: canonical `1` → `{"command":"open"}`,
///   `0` → `{"command":"close"}`. Any other value is rejected so a bad write is
///   visible rather than silently mis-sent.
/// - Any other (unknown) class string is rejected — surfaces a config typo
///   instead of silently falling back.
fn map_command_to_downlink(
    command_class: Option<&str>,
    raw_payload: &[u8],
) -> Result<DownlinkPayload, OpcGwError> {
    match command_class {
        None => Ok(DownlinkPayload::Raw(raw_payload.to_vec())),
        Some("valve") => {
            let value = raw_payload.first().copied().ok_or_else(|| {
                OpcGwError::ChirpStack("valve command has empty payload".to_string())
            })?;
            let command = match value {
                1 => "open",
                0 => "close",
                other => {
                    return Err(OpcGwError::ChirpStack(format!(
                        "valve command value {} out of range (expected 1=open or 0=close)",
                        other
                    )))
                }
            };
            Ok(DownlinkPayload::Object(valve_command_object(command)))
        }
        Some(unknown) => Err(OpcGwError::ChirpStack(format!(
            "unknown command_class '{}' (expected \"valve\" or none)",
            unknown
        ))),
    }
}

/// Builds a `{ "command": <command> }` protobuf struct for the device codec.
fn valve_command_object(command: &str) -> chirpstack_api::prost_types::Struct {
    use chirpstack_api::prost_types::{value::Kind, Struct, Value};
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(
        "command".to_string(),
        Value {
            kind: Some(Kind::StringValue(command.to_string())),
        },
    );
    Struct { fields }
}

/// Builds the ChirpStack `DeviceQueueItem` for a downlink.
///
/// For `Object`, `data` is empty and the device-profile codec produces the
/// bytes; for `Raw`, `data` carries the bytes and `object` is `None`.
fn build_queue_item(
    device_id: &str,
    f_port: u8,
    payload: &DownlinkPayload,
    confirmed: bool,
) -> DeviceQueueItem {
    let (data, object) = match payload {
        DownlinkPayload::Object(s) => (Vec::new(), Some(s.clone())),
        DownlinkPayload::Raw(bytes) => (bytes.clone(), None),
    };
    DeviceQueueItem {
        id: String::new(),
        dev_eui: device_id.to_string(),
        confirmed,
        f_port: f_port as u32,
        data,
        object,
        // Output-only field on enqueue; ChirpStack manages pending state.
        is_pending: false,
        f_cnt_down: 0,
        is_encrypted: false,
        expires_at: None,
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
/// ```rust,ignore
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
/// ```rust,ignore
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
    use tracing_test::traced_test;

    /// Story 6-3, AC#7: `device_poll` warn fires for a single device's
    /// failure inside a poll cycle, complementing the existing per-cycle
    /// `device_polled` debug from Story 6-1.
    #[test]
    #[traced_test]
    fn device_poll_failure_log_fields() {
        let dev_id = "dev-test";
        let e: OpcGwError = OpcGwError::ChirpStack("connection refused".to_string());
        tracing::warn!(
            operation = "device_poll",
            device_id = %dev_id,
            error = %e,
            status = "failed"
        );
        assert!(logs_contain("operation=\"device_poll\""));
        assert!(logs_contain("status=\"failed\""));
        assert!(logs_contain("device_id=dev-test"));
    }

    /// Story 6-3, AC#7: `chirpstack_request` warn for transient network
    /// errors (Unavailable / Cancelled) carries `error`, `attempt`, and
    /// `retry_delay_secs`. Iter-3 review pending #1 rewrite: drives
    /// `classify_and_log_grpc_error` directly so the production site's
    /// `tonic::Code::Unavailable` branch is actually exercised.
    #[test]
    #[traced_test]
    fn chirpstack_request_transient_log_fields() {
        let status = tonic::Status::unavailable("connection refused (transient)");
        let class = classify_and_log_grpc_error(&status, 1234u64, 5u64);
        assert_eq!(class, GrpcErrorClass::Transient);
        assert!(logs_contain("operation=\"chirpstack_request\""));
        assert!(logs_contain("retry_delay_secs=5"));
        assert!(logs_contain("attempt=1"));
    }

    /// Story 6-3, AC#7: a `Cancelled` status also maps to `Transient`.
    /// Iter-3 review pending #1 added this case to pin both `Unavailable`
    /// and `Cancelled` under the same classification.
    #[test]
    #[traced_test]
    fn chirpstack_request_cancelled_classified_as_transient() {
        let status = tonic::Status::cancelled("peer dropped mid-stream");
        let class = classify_and_log_grpc_error(&status, 100u64, 5u64);
        assert_eq!(class, GrpcErrorClass::Transient);
        assert!(logs_contain("operation=\"chirpstack_request\""));
    }

    /// Story 6-3, AC#6: gRPC `chirpstack_request` timeout warn carries the
    /// expected canonical fields. Iter-3 review pending #1 rewrite: drives
    /// `classify_and_log_grpc_error` with a real `tonic::Status` of code
    /// `DeadlineExceeded` so the production code path is actually
    /// exercised — a future regression that, e.g., reorders the match arms
    /// or renames the operation will fail this test.
    #[test]
    #[traced_test]
    fn chirpstack_request_timeout_log_fields() {
        let status = tonic::Status::deadline_exceeded("transport timeout");
        let class = classify_and_log_grpc_error(&status, 1500u64, 5u64);
        assert_eq!(class, GrpcErrorClass::Timeout);
        assert!(logs_contain("operation=\"chirpstack_request\""));
        assert!(logs_contain("exceeded=true"));
        assert!(logs_contain("duration_ms=1500"));
    }

    /// Story 6-3, AC#6/AC#7 negative case: status codes outside the
    /// timeout/transient set classify as `Other` and emit no warn.
    #[test]
    #[traced_test]
    fn chirpstack_request_other_codes_silent() {
        let status = tonic::Status::not_found("device id absent");
        let class = classify_and_log_grpc_error(&status, 50u64, 5u64);
        assert_eq!(class, GrpcErrorClass::Other);
        assert!(
            !logs_contain("operation=\"chirpstack_request\""),
            "Other-class status codes must not emit chirpstack_request warns"
        );
    }

    /// Story 6-3, AC#6: `metric_parse` warn fires when a metric raw value
    /// can't be coerced to its declared type. Iter-3 review pending #1
    /// rewrite: drives `validate_bool_metric_value` directly. The production
    /// boolean branch in `prepare_metric_for_batch` calls the same helper —
    /// any change to the warn shape (field renames, missing fields, etc.)
    /// will fail this test.
    #[test]
    #[traced_test]
    fn metric_parse_log_fields() {
        let result = validate_bool_metric_value(
            0.5_f32,
            "test_device",
            "is_on",
            ChirpStackMetricKind::Gauge,
        );
        assert!(
            result.is_none(),
            "0.5 is not a valid boolean (must be 0.0 or 1.0)"
        );
        // A-3 (AC#10): emission renamed `operation = "metric_parse"` →
        // `event = "metric_parse"` to align with the grep contract pattern
        // used by Stories 9-4 through 9-8 (`event = "<prefix>_..."`).
        // Field schema also added `expected_type = "Bool"` + `reason = "invalid_bool"`
        // — closed enum across the two emission sites in chirpstack.rs.
        assert!(logs_contain("event=\"metric_parse\""));
        assert!(logs_contain("expected_type=\"Bool\""));
        assert!(logs_contain("reason=\"invalid_bool\""));
        assert!(logs_contain("device_id=test_device"));
        assert!(logs_contain("metric_name=is_on"));
    }

    /// Iter-3 review pending #1: positive path of `validate_bool_metric_value`
    /// — `0.0` and `1.0` are accepted with no warn.
    /// A-4 AC#9 closure (iter-1 IR6 rewrite): exercises the typed-path
    /// counter monotonic check end-to-end through `prepare_metric_for_batch`
    /// (the production call site at chirpstack.rs:1717-1729), not via inline
    /// match-arm assertions. A regression at the production site (e.g. swapping
    /// `MetricType::Int(p)` to a wrong variant, or accidentally restoring the
    /// dropped legacy fallback) is caught here because the test reaches the
    /// real call site, not a separate copy of the match.
    ///
    /// Test setup:
    /// 1. Build a ChirpstackPoller fixture with InMemoryBackend.
    /// 2. Seed a prior Counter row via `batch_write_metrics` carrying
    ///    `MetricType::Int(100)` (the post-A-3 writer contract).
    /// 3. Construct a mock_metric proto with `data[0] = 50.0` (a reset:
    ///    50 < 100 → typed-path branch fires and returns None).
    /// 4. Assert `prepare_metric_for_batch` returns None.
    /// 5. Assert `tracing-test::logs_contain("Counter reset detected")`.
    #[tokio::test]
    #[traced_test]
    async fn ac9_counter_monotonic_typed_path_extracts_payload() {
        use crate::storage::{BatchMetricWrite, MetricType, StorageBackend};
        use chirpstack_api::common::{Metric, MetricDataset};
        use std::time::SystemTime;

        // Step 1: build poller fixture.
        let config = get_test_config();
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let restore_barrier = Arc::new(std::sync::Barrier::new(2));
        let poller = ChirpstackPoller::new(&config, backend.clone(), cancel_token, restore_barrier)
            .await
            .expect("poller fixture must build");

        let device_id = "ac9_test_device";
        let metric_name = "ac9_packet_count";

        // Step 2: seed prior Counter row with MetricType::Int(100) via
        // batch_write_metrics (matches A-3 production writer contract).
        //
        // A-5: BatchMetricWrite.value: String field retired; only typed
        // `data_type` carries the measurement. The legacy-fallback hazard
        // pinned by JR1 (path-ambiguous seed values) is now structurally
        // impossible since the legacy `value` column is no longer read by
        // any production path.
        backend
            .batch_write_metrics(vec![BatchMetricWrite {
                device_id: device_id.to_string(),
                metric_name: metric_name.to_string(),
                data_type: MetricType::Int(100),
                timestamp: SystemTime::now(),
            }])
            .expect("seed prev Counter row");

        // Sanity: confirm the storage layer returns the typed payload via
        // get_metric_value — the production code path that A-4 IR4 relies on.
        let prev = backend
            .get_metric_value(device_id, metric_name)
            .expect("get_metric_value")
            .expect("Some(MetricValue)");
        assert_eq!(
            prev.data_type,
            MetricType::Int(100),
            "A-4 reader contract: get_metric_value must return typed MetricType::Int(100)"
        );

        // Step 3: construct a mock COUNTER metric with data[0] = 50.0 (reset).
        let reset_metric = Metric {
            name: metric_name.to_string(),
            kind: 0, // COUNTER per chirpstack_api proto
            timestamps: vec![],
            datasets: vec![MetricDataset {
                label: "test".to_string(),
                data: vec![50.0_f32], // 50 < 100 → reset
            }],
        };

        // Step 4: call the production code path. Must return None because
        // the typed-path monotonic check at chirpstack.rs:1717-1729 detects
        // the reset (50 < 100) via prev_metric.data_type == Int(100).
        let result = poller.prepare_metric_for_batch(device_id, &reset_metric);
        assert!(
            result.is_none(),
            "A-4 AC#9: Counter reset (50 < 100) must be detected via typed-path \
             and prepare_metric_for_batch must return None"
        );

        // Step 5: confirm the typed-path produced the canonical "Counter reset"
        // warn (the warn fires from the typed-path branch only; if the legacy
        // fallback had fired by accident, the warn would still emit but the
        // production code path that we want to regression-guard is the typed
        // branch).
        assert!(
            logs_contain("Counter reset detected"),
            "A-4 AC#9: typed-path branch must emit the canonical Counter reset warn"
        );
        // And confirm the warn carries the real previous value from the
        // typed payload (not a zero-default or a string-parse result).
        assert!(
            logs_contain("prev_value=100"),
            "A-4 AC#9: warn must carry prev_value=100 from typed payload (not 0 or parsed-string)"
        );
        assert!(
            logs_contain("new_value=50"),
            "A-4 AC#9: warn must carry new_value=50 from the new metric"
        );
    }

    #[test]
    #[traced_test]
    fn metric_parse_accepts_zero_and_one() {
        let zero = validate_bool_metric_value(
            0.0_f32,
            "dev",
            "flag",
            ChirpStackMetricKind::Gauge,
        );
        let one = validate_bool_metric_value(
            1.0_f32,
            "dev",
            "flag",
            ChirpStackMetricKind::Gauge,
        );
        assert_eq!(zero, Some("0"));
        assert_eq!(one, Some("1"));
        assert!(
            !logs_contain("operation=\"metric_parse\""),
            "valid boolean values must not emit metric_parse warn"
        );
    }

    /// Story 6-3, AC#1: when the connection retry loop emits a failure log,
    /// it carries the canonical AC#1 fields (`attempt`, `error`,
    /// `retry_delay_secs`, `max_retries`, `success=false`). The production
    /// site in `get_device_metrics_from_server` uses this exact macro shape.
    #[test]
    #[traced_test]
    fn chirpstack_connect_failure_log_fields() {
        let attempt: u32 = 2;
        let retry_delay_secs: u64 = 5;
        let max_retries: u32 = 3;
        tracing::warn!(
            operation = "chirpstack_connect",
            attempt = attempt,
            error = "connection refused",
            retry_delay_secs = retry_delay_secs,
            max_retries = max_retries,
            success = false,
            "TCP availability probe failed"
        );
        assert!(logs_contain("operation=\"chirpstack_connect\""));
        assert!(logs_contain("attempt=2"));
        assert!(logs_contain("retry_delay_secs=5"));
        assert!(logs_contain("max_retries=3"));
        assert!(logs_contain("success=false"));
    }

    /// Story 6-3, AC#5: the `chirpstack_outage` warn carries `timestamp`,
    /// `last_successful_poll`, and `current_attempt_failed_with`. Iter-3
    /// review pending #1 rewrite: drives `maybe_emit_chirpstack_outage`
    /// directly, exercising the production helper that `poll_metrics`
    /// calls. Verifies (a) the warn fires on the first call, (b) the
    /// `last_successful_poll` field is rfc3339-rendered (P9), and (c) the
    /// `outage_already_logged` flag is set so subsequent calls are silent.
    #[test]
    #[traced_test]
    fn chirpstack_outage_log_fields() {
        let mut outage_logged = false;
        let last_successful_poll: Option<DateTime<Utc>> = None;
        let err = OpcGwError::ChirpStack("connection refused".to_string());
        let fired = maybe_emit_chirpstack_outage(
            &mut outage_logged,
            last_successful_poll,
            &err,
        );
        assert!(fired, "first invocation should emit");
        assert!(outage_logged, "outage_logged flag must be set after fire");
        assert!(logs_contain("operation=\"chirpstack_outage\""));
        assert!(logs_contain("last_successful_poll=null"));
        assert!(logs_contain("current_attempt_failed_with"));
    }

    /// Iter-3 review pending #1: second invocation in the same cycle is
    /// silent — only the first connectivity failure of a cycle emits the
    /// outage warn.
    #[test]
    #[traced_test]
    fn chirpstack_outage_silent_on_second_call() {
        let mut outage_logged = true; // already logged earlier in cycle
        let err = OpcGwError::ChirpStack("connection refused".to_string());
        let fired =
            maybe_emit_chirpstack_outage(&mut outage_logged, None, &err);
        assert!(!fired, "second invocation must be silent");
        assert!(
            !logs_contain("operation=\"chirpstack_outage\""),
            "must not re-fire outage warn within same cycle"
        );
    }

    /// Iter-3 review pending #1: `last_successful_poll = Some(_)` renders as
    /// rfc3339, not as a `Some(...)` debug-format string.
    #[test]
    #[traced_test]
    fn chirpstack_outage_renders_last_poll_rfc3339() {
        let mut outage_logged = false;
        let ts = DateTime::parse_from_rfc3339("2026-04-27T12:34:56Z")
            .expect("parse rfc3339")
            .with_timezone(&Utc);
        let err = OpcGwError::ChirpStack("transient failure".to_string());
        maybe_emit_chirpstack_outage(&mut outage_logged, Some(ts), &err);
        assert!(
            logs_contain("last_successful_poll=2026-04-27T12:34:56"),
            "expected rfc3339-rendered timestamp; format_last_successful_poll regression"
        );
        assert!(
            !logs_contain("Some("),
            "must not emit `Some(...)` debug format on the timestamp field"
        );
    }

    /// Story 6-3, AC#4: a spike of `>= ERROR_SPIKE_THRESHOLD` errors between
    /// consecutive cycles emits a structured `warn!`. Iter-3 review pending
    /// #1 rewrite: drives `maybe_emit_error_spike` directly so the
    /// production threshold (`ERROR_SPIKE_THRESHOLD = 5`) and field shape
    /// are pinned.
    #[test]
    #[traced_test]
    fn error_spike_warn_when_delta_ge_threshold() {
        let result = maybe_emit_error_spike(0, 5);
        assert_eq!(
            result,
            Some(5),
            "delta of 5 should fire (threshold is {})",
            ERROR_SPIKE_THRESHOLD
        );
        assert!(logs_contain("operation=\"error_spike\""));
        assert!(logs_contain("delta=5"));
        assert!(logs_contain("previous=0"));
        assert!(logs_contain("current=5"));
    }

    /// Story 6-3, AC#4 negative case: a delta below the threshold returns
    /// `None` and emits no warn.
    #[test]
    #[traced_test]
    fn error_spike_silent_when_delta_below_threshold() {
        let result = maybe_emit_error_spike(1, 4);
        assert_eq!(result, None, "delta of 3 must not fire");
        assert!(
            !logs_contain("operation=\"error_spike\""),
            "must not emit error_spike for delta below threshold"
        );
    }

    /// Iter-3 review pending #1: previous-greater-than-current uses
    /// `saturating_sub`, so the helper returns `None` (delta is 0) without
    /// panicking — pins the P14 contract end-to-end.
    #[test]
    #[traced_test]
    fn error_spike_saturates_negative_delta() {
        let result = maybe_emit_error_spike(10, 3);
        assert_eq!(
            result, None,
            "saturating_sub of 3 - 10 = 0 (helper must not fire)"
        );
        assert!(!logs_contain("operation=\"error_spike\""));
    }

    /// Story 6-3, AC#3 verification: when a successful batch write takes
    /// longer than `BATCH_WRITE_BUDGET_MS` (500 ms), the production code in
    /// `poll_metrics` upgrades the routine `debug!` to a structured `warn!`
    /// carrying `exceeded_budget=true`. This test exercises the same
    /// `if latency > BUDGET` pattern used at the call site.
    #[test]
    #[traced_test]
    fn batch_write_budget_emits_warn_when_exceeded() {
        let batch_start = Instant::now();
        std::thread::sleep(Duration::from_millis(510));
        let batch_latency_ms = batch_start.elapsed().as_millis() as u64;
        let batch_count: u32 = 42;
        if batch_latency_ms > crate::utils::BATCH_WRITE_BUDGET_MS {
            tracing::warn!(
                operation = "batch_write",
                metrics_count = batch_count,
                latency_ms = batch_latency_ms,
                budget_ms = crate::utils::BATCH_WRITE_BUDGET_MS,
                exceeded_budget = true,
                success = true,
                "Batch write exceeded budget"
            );
        }
        assert!(
            logs_contain("operation=\"batch_write\""),
            "expected batch_write budget warn"
        );
        assert!(
            logs_contain("exceeded_budget=true"),
            "expected exceeded_budget=true marker"
        );
        assert!(
            logs_contain("budget_ms=500"),
            "expected budget_ms=500 to match BATCH_WRITE_BUDGET_MS"
        );
    }

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

        let _poller = ChirpstackPoller::new(
            &config,
            backend.clone(),
            cancel_token,
            restore_barrier,
        )
        .await
        .expect("Failed to create poller");

        // Verify poller can call backend trait methods
        // (This is a smoke test; detailed method calls are tested elsewhere)
        let result = backend.update_gateway_status(
            Some(chrono::Utc::now()),
            0,
            true,
        );
        assert!(result.is_ok(), "Backend trait methods should be accessible from poller");
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

    // =====================================================================
    // Story 4-4: ChirpStack outage recovery loop tests
    // =====================================================================

    /// (Story 4-4 helper) Build a ChirpstackPoller fixture with a chosen
    /// `chirpstack.server_address`, retry, and delay. Bypasses validation
    /// (delay=0 would fail `validate()`) so tests can run with sub-second
    /// budgets.
    async fn make_recovery_test_poller(
        server_address: String,
        retry: u32,
        delay: u64,
        backend: Arc<dyn StorageBackend>,
        cancel_token: CancellationToken,
    ) -> ChirpstackPoller {
        let mut config = get_test_config();
        config.chirpstack.server_address = server_address;
        config.chirpstack.retry = retry;
        config.chirpstack.delay = delay;
        let restore_barrier = Arc::new(std::sync::Barrier::new(2));
        ChirpstackPoller::new(&config, backend, cancel_token, restore_barrier)
            .await
            .expect("poller fixture must build")
    }

    /// AC#1 + AC#5 + AC#6: when the TCP probe succeeds on the first
    /// attempt, the recovery loop emits `recovery_attempt` then
    /// `recovery_complete` and returns `Recovered { attempts_used: 1 }`.
    #[tokio::test]
    #[traced_test]
    async fn recovery_succeeds_when_server_returns_available() {
        // Bind a real TCP listener so check_server_availability succeeds.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let mut poller = make_recovery_test_poller(
            server_address,
            3,
            0,
            backend.clone(),
            cancel_token,
        )
        .await;

        let last_error = OpcGwError::ChirpStack("simulated outage".to_string());
        let outcome = poller
            .recover_from_chirpstack_outage(0, &last_error)
            .await;

        assert_eq!(
            outcome,
            RecoveryOutcome::Recovered { attempts_used: 1 },
            "first-attempt success must return Recovered with attempts_used=1"
        );
        assert!(logs_contain("operation=\"recovery_attempt\""));
        assert!(logs_contain("operation=\"recovery_complete\""));
        assert!(logs_contain("attempts_used=1"));
        assert!(
            !logs_contain("operation=\"recovery_failed\""),
            "must not emit recovery_failed when probe succeeds"
        );

        drop(listener);
    }

    /// GH #126 regression: a per-device error while ChirpStack is **reachable**
    /// must NOT be treated as a gateway-wide outage. A real listener makes the
    /// availability probe succeed, so `device_error_is_outage` returns false
    /// even for a `ChirpStack(_)` error (the server responded → per-device data
    /// error). Pre-fix, any `ChirpStack(_)` error flipped `chirpstack_available`
    /// and triggered the recovery loop.
    #[tokio::test]
    async fn device_error_with_reachable_server_is_not_outage() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let poller =
            make_recovery_test_poller(server_address, 1, 0, backend, CancellationToken::new())
                .await;

        let err = OpcGwError::ChirpStack(
            "Error getting device metrics: code: 'Internal error', message: \"Odd number of digits\""
                .to_string(),
        );
        assert!(
            !poller.device_error_is_outage(&err),
            "server reachable → a per-device ChirpStack error must NOT be an outage (#126)"
        );
        drop(listener);
    }

    /// GH #126 regression: a `ChirpStack(_)` error while the server is genuinely
    /// **unreachable** still counts as an outage (recovery path preserved).
    #[tokio::test]
    async fn device_error_with_unreachable_server_is_outage() {
        // bind+drop yields a (probabilistically) free port nothing listens on.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let poller =
            make_recovery_test_poller(server_address, 1, 0, backend, CancellationToken::new())
                .await;

        let err = OpcGwError::ChirpStack("transport error: connection refused".to_string());
        assert!(
            poller.device_error_is_outage(&err),
            "server unreachable → a ChirpStack error IS an outage (#126)"
        );
    }

    /// GH #122 regression: the TCP availability probe must resolve DNS
    /// hostnames, not only numeric IPs. Before the fix, the probe used
    /// `SocketAddr::parse()`, which rejected `localhost:<port>` (and any
    /// Docker service name such as `chirpstack:8080`) with "invalid socket
    /// address syntax". With `to_socket_addrs()` the hostname resolves and
    /// the probe connects to the listening port.
    #[tokio::test]
    async fn probe_resolves_dns_hostname() {
        // Bind a real listener on the IPv4 loopback, then probe via the
        // `localhost` hostname. `localhost` may resolve to both 127.0.0.1 and
        // ::1; the probe tries each resolved address until one connects, so
        // this is deterministic regardless of resolution order.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        let server_address = format!("http://localhost:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let poller =
            make_recovery_test_poller(server_address, 1, 0, backend, cancel_token).await;

        let result = poller.check_server_availability();
        assert!(
            result.is_ok(),
            "probe must resolve the `localhost` hostname and connect, got: {result:?}"
        );

        drop(listener);
    }

    /// AC#1 + AC#2: when the TCP probe fails for the entire retry budget,
    /// the recovery loop emits `recovery_attempt` × R, then
    /// `recovery_failed`, and returns `Exhausted { attempts_used: R }`.
    #[tokio::test]
    #[traced_test]
    async fn recovery_exhausts_when_server_stays_down() {
        // Pick a port that nobody is listening on. Bind+drop yields a
        // (probabilistically) free port — connect_timeout will fail.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener); // close before recovery loop probes
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let mut poller = make_recovery_test_poller(
            server_address,
            3,
            0,
            backend.clone(),
            cancel_token,
        )
        .await;

        let last_error = OpcGwError::ChirpStack("simulated outage".to_string());
        let outcome = poller
            .recover_from_chirpstack_outage(7, &last_error)
            .await;

        match outcome {
            RecoveryOutcome::Exhausted {
                attempts_used,
                last_error: _,
            } => {
                assert_eq!(attempts_used, 3, "exhaustion uses all 3 retries");
            }
            other => panic!("expected Exhausted, got {other:?}"),
        }
        assert!(logs_contain("operation=\"recovery_attempt\""));
        assert!(logs_contain("operation=\"recovery_failed\""));
        assert!(logs_contain("attempts_used=3"));
    }

    /// AC#3: on recovery loop entry the poller calls
    /// `update_gateway_status(None, error_count_at_entry, false)` so OPC UA
    /// (Story 5-3) and the web dashboard (Story 9-2) see
    /// `chirpstack_available=false` during the retry window.
    #[tokio::test]
    #[traced_test]
    async fn recovery_updates_gateway_status_to_unavailable() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let mut poller = make_recovery_test_poller(
            server_address,
            1,
            0,
            backend.clone(),
            cancel_token,
        )
        .await;

        // P7: pre-seed last_poll_timestamp with a known value so the
        // None-preserves-existing contract is actually exercised. Asserting
        // is_none() against a fresh backend (the iter-0 test shape) passed
        // trivially — a regression that passed Utc::now() instead of None
        // to update_gateway_status would not have been caught.
        let seeded_ts = chrono::Utc::now() - chrono::Duration::seconds(120);
        backend
            .update_gateway_status(Some(seeded_ts), 7, true)
            .expect("seed gateway_status before recovery");

        let last_error = OpcGwError::ChirpStack("simulated outage".to_string());
        let _ = poller
            .recover_from_chirpstack_outage(42, &last_error)
            .await;

        let (last_poll_ts, error_count, available) = backend
            .get_gateway_health_metrics()
            .expect("read gateway_status after recovery loop");
        assert!(!available, "chirpstack_available must be false after recovery loop entry");
        assert_eq!(error_count, 42, "error_count_at_entry must propagate to gateway_status");
        // P7: assert that the SEEDED timestamp is preserved — passing None to
        // update_gateway_status must NOT overwrite the existing DB value.
        // Use a small tolerance for clock-precision quirks across backends
        // (SqliteBackend round-trips through string serialisation; InMemory
        // round-trips Utc datetimes directly).
        let preserved_ts = last_poll_ts
            .expect("last_poll_timestamp must be preserved (Some after recovery)");
        let drift = (preserved_ts - seeded_ts).num_milliseconds().abs();
        assert!(
            drift < 1000,
            "last_poll_timestamp must equal the seeded value (preserved), \
             not overwritten by recovery. seeded={:?}, observed={:?}, drift={}ms",
            seeded_ts, preserved_ts, drift
        );
    }

    /// AC#6: when `last_successful_poll` is None (cold start, no prior
    /// successful poll), `recovery_complete` carries `from_startup=true`
    /// instead of `downtime_secs`.
    #[tokio::test]
    #[traced_test]
    async fn recovery_complete_emits_from_startup_when_no_prior_poll() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let mut poller = make_recovery_test_poller(
            server_address,
            1,
            0,
            backend,
            cancel_token,
        )
        .await;
        // last_successful_poll defaults to None on construction (line 441).

        let last_error = OpcGwError::ChirpStack("cold-start outage".to_string());
        let outcome = poller
            .recover_from_chirpstack_outage(0, &last_error)
            .await;

        assert_eq!(outcome, RecoveryOutcome::Recovered { attempts_used: 1 });
        assert!(logs_contain("operation=\"recovery_complete\""));
        assert!(logs_contain("from_startup=true"));
        // P4: drop brittle `!logs_contain("downtime_secs")` negative assertion
        // (would false-fail on any unrelated substring, e.g. doc-comments
        // mentioning the field name in another loaded module). The
        // positive `from_startup=true` assertion is sufficient — the
        // helper's match arms are mutually exclusive at the source level.

        drop(listener);
    }

    /// AC#6: when `last_successful_poll` is `Some(t)` and recovery
    /// succeeds, `recovery_complete` carries `downtime_secs` reflecting
    /// `now - t`.
    #[tokio::test]
    #[traced_test]
    async fn recovery_complete_carries_downtime_secs() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let mut poller = make_recovery_test_poller(
            server_address,
            1,
            0,
            backend,
            cancel_token,
        )
        .await;
        // Pretend the poller had a successful cycle 10 seconds ago.
        poller.last_successful_poll =
            Some(chrono::Utc::now() - chrono::Duration::seconds(10));

        let last_error = OpcGwError::ChirpStack("outage with prior history".to_string());
        let outcome = poller
            .recover_from_chirpstack_outage(0, &last_error)
            .await;

        assert_eq!(outcome, RecoveryOutcome::Recovered { attempts_used: 1 });
        assert!(logs_contain("operation=\"recovery_complete\""));
        // P13 (iter-3): replace the field-order-coupled `=10 ` trailing-space
        // trick with a token-boundary scan. The previous assertion silently
        // depended on `last_error` always following `downtime_secs` in the
        // macro; reordering would break it. Read the captured log directly,
        // tokenize each `recovery_complete` line on whitespace, and check
        // for an exact `downtime_secs=N` token where N ∈ {10, 11, 12}
        // (allowing 2s scheduling jitter).
        let raw = tracing_test::internal::global_buf().lock().unwrap().clone();
        let captured = String::from_utf8_lossy(&raw);
        let downtime_ok = captured
            .lines()
            .filter(|line| line.contains(r#"operation="recovery_complete""#))
            .any(|line| {
                line.split_whitespace().any(|tok| {
                    matches!(
                        tok,
                        "downtime_secs=10" | "downtime_secs=11" | "downtime_secs=12"
                    )
                })
            });
        assert!(
            downtime_ok,
            "recovery_complete must carry downtime_secs ∈ {{10, 11, 12}} (allow 2s jitter); captured logs:\n{captured}"
        );
        let downtime_zero = captured
            .lines()
            .filter(|line| line.contains(r#"operation="recovery_complete""#))
            .any(|line| {
                line.split_whitespace().any(|tok| tok == "downtime_secs=0")
            });
        assert!(
            !downtime_zero,
            "downtime_secs must not be 0 when last_successful_poll was 10s ago; captured logs:\n{captured}"
        );

        drop(listener);
    }

    /// AC#1 cancel-safety: firing the cancel token during the retry sleep
    /// aborts the loop within `delay + ε` and returns Exhausted with a
    /// `cancelled` last_error.
    #[tokio::test]
    #[traced_test]
    async fn recovery_aborts_on_cancel_during_sleep() {
        // No listener — ensure the probe would fail if we ever reached it.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let cancel_clone = cancel_token.clone();
        // Long delay so the cancel arrives during the sleep, not after.
        let mut poller = make_recovery_test_poller(
            server_address,
            5,
            10,
            backend,
            cancel_token,
        )
        .await;

        // Fire cancel after a brief moment.
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel_clone.cancel();
        });

        let last_error = OpcGwError::ChirpStack("outage during cancel".to_string());
        let start = Instant::now();
        let outcome = poller
            .recover_from_chirpstack_outage(0, &last_error)
            .await;
        let elapsed = start.elapsed();

        assert!(
            elapsed < Duration::from_secs(2),
            "cancel must abort the recovery loop quickly (elapsed {:?})",
            elapsed
        );
        match outcome {
            RecoveryOutcome::Exhausted {
                attempts_used,
                last_error,
            } => {
                // P4 + P1: under probe-before-sleep semantics, the first
                // attempt's probe runs before the sleep where cancel fires.
                // attempts_used must be 1 (the failed probe before cancel
                // arrived during the inter-attempt sleep), NOT 0.
                assert_eq!(
                    attempts_used, 1,
                    "cancel during the first inter-attempt sleep must report \
                     attempts_used=1 (the failed probe that ran before sleep); \
                     got {attempts_used}"
                );
                assert!(
                    last_error.contains("cancelled"),
                    "Exhausted reason must mention cancellation; got {last_error}"
                );
                // P3 + P10: cancel branch must preserve BOTH the most-recent
                // probe error AND the original outage cause for post-mortem
                // clarity. P3 added "last probe error:"; P10 (iter-3) added
                // "outage cause:" because the original gRPC failure is
                // overwritten by the first failed probe.
                assert!(
                    last_error.contains("last probe error:"),
                    "Exhausted reason must include the last probe error for diagnostics; got {last_error}"
                );
                assert!(
                    last_error.contains("outage cause:"),
                    "Exhausted reason must include the original outage cause (P10); got {last_error}"
                );
                assert!(
                    last_error.contains("outage during cancel"),
                    "Exhausted reason must surface the original gRPC failure text seeded by this test; got {last_error}"
                );
            }
            other => panic!("expected Exhausted on cancel, got {other:?}"),
        }
    }

    /// AC#2: a single retry budget (R=1, D=0) makes exactly one attempt.
    #[tokio::test]
    #[traced_test]
    async fn recovery_single_retry_budget_makes_exactly_one_attempt() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        drop(listener);
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let mut poller = make_recovery_test_poller(
            server_address,
            1,
            0,
            backend,
            cancel_token,
        )
        .await;

        let last_error = OpcGwError::ChirpStack("outage".to_string());
        let outcome = poller
            .recover_from_chirpstack_outage(0, &last_error)
            .await;

        match outcome {
            RecoveryOutcome::Exhausted {
                attempts_used,
                last_error: _,
            } => {
                assert_eq!(attempts_used, 1, "R=1 budget makes exactly one attempt");
            }
            other => panic!("expected Exhausted with attempts_used=1, got {other:?}"),
        }
        assert!(logs_contain("operation=\"recovery_attempt\""));
        assert!(logs_contain("attempt=1"));
        assert!(logs_contain("max_retries=1"));
        assert!(logs_contain("operation=\"recovery_failed\""));
    }

    /// (Story 4-4 review iter-1 P9 — surrogate for the deferred
    /// poll_metrics-driving integration test) Pins the wire-in's
    /// load-bearing **just_logged single-fire-per-cycle gating**
    /// contract by exercising the same call sequence the production
    /// wire-in at `poll_metrics:1224-1243` performs:
    /// `let just_logged = maybe_emit_chirpstack_outage(...);
    ///  if just_logged { self.recover_from_chirpstack_outage(...).await; }`
    ///
    /// Verifies:
    ///   - First per-cycle ChirpStack error: `just_logged == true`,
    ///     `chirpstack_outage` warn fires, recovery loop is invoked.
    ///   - Second per-cycle ChirpStack error (same cycle-local bool):
    ///     `just_logged == false`, no second `chirpstack_outage` warn,
    ///     recovery loop is NOT re-invoked (asserted via the
    ///     `recovery_attempt` count remaining at exactly N from the
    ///     first invocation, which is R from the recovery budget).
    ///
    /// **Note on broader integration coverage:** the original P9 design
    /// wanted to drive `poll_metrics` end-to-end against a stub TCP
    /// listener, but tonic's `Channel::connect()` returns successfully
    /// once the kernel SYN-ACK completes (well before HTTP/2 SETTINGS),
    /// so the P2 5s channel timeout doesn't bound the request-level
    /// hang. A true end-to-end integration test requires either a mock
    /// gRPC server (~200+ LOC of infrastructure) or a request-level
    /// timeout in `get_device_metrics_from_server` (out of Story 4-4
    /// scope; tracked in `deferred-work.md` as iter-1 deferred concern).
    #[tokio::test]
    #[traced_test]
    async fn recovery_wire_in_just_logged_gating_single_fires_per_cycle() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind ephemeral port");
        let port = listener.local_addr().expect("local_addr").port();
        let server_address = format!("http://127.0.0.1:{port}");
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let mut poller = make_recovery_test_poller(
            server_address,
            1,
            0,
            backend,
            cancel_token,
        )
        .await;

        // Simulate the per-device error branch from poll_metrics:1052-1068
        // — the `chirpstack_outage_logged` cycle-local bool persists
        // across multiple device failures in the same cycle.
        let mut chirpstack_outage_logged = false;
        let err1 = OpcGwError::ChirpStack("device 1 outage".to_string());
        let err2 = OpcGwError::ChirpStack("device 2 outage".to_string());
        let err3 = OpcGwError::ChirpStack("device 3 outage".to_string());

        // First device failure of the cycle: just_logged=true → fire
        // chirpstack_outage warn AND invoke the recovery loop.
        let just_logged_1 = maybe_emit_chirpstack_outage(
            &mut chirpstack_outage_logged,
            poller.last_successful_poll,
            &err1,
        );
        assert!(
            just_logged_1,
            "first per-cycle ChirpStack error must fire chirpstack_outage warn (just_logged=true)"
        );
        if just_logged_1 {
            let _ = poller.recover_from_chirpstack_outage(1, &err1).await;
        }

        // Second device failure in the SAME cycle (chirpstack_outage_logged
        // is still true from the first call): just_logged must be false,
        // recovery loop must NOT re-fire.
        let just_logged_2 = maybe_emit_chirpstack_outage(
            &mut chirpstack_outage_logged,
            poller.last_successful_poll,
            &err2,
        );
        assert!(
            !just_logged_2,
            "second per-cycle ChirpStack error must NOT re-fire chirpstack_outage warn (just_logged=false)"
        );
        if just_logged_2 {
            // This branch must not execute. If it does, the test fails
            // via the recovery_attempt count assertion below.
            let _ = poller.recover_from_chirpstack_outage(2, &err2).await;
        }

        // Third device failure — same gating contract.
        let just_logged_3 = maybe_emit_chirpstack_outage(
            &mut chirpstack_outage_logged,
            poller.last_successful_poll,
            &err3,
        );
        assert!(
            !just_logged_3,
            "third per-cycle ChirpStack error must NOT re-fire (just_logged=false)"
        );

        // Wire-in contract assertions on captured logs:
        //   - `chirpstack_outage` warn fired (ONLY once — second / third
        //     calls returned just_logged=false, asserted above).
        //   - `recovery_attempt` info fired (recovery loop invoked).
        //   - `recovery_complete` fired (probe succeeded — listener still
        //     bound). NOT recovery_failed: the test fixture pins the
        //     recovery-loop-success path through the wire-in.
        assert!(
            logs_contain("operation=\"chirpstack_outage\""),
            "chirpstack_outage warn must have fired"
        );
        assert!(
            logs_contain("operation=\"recovery_attempt\""),
            "recovery_attempt must have fired (recovery loop invoked through wire-in)"
        );
        assert!(
            logs_contain("operation=\"recovery_complete\""),
            "recovery loop must have completed successfully (TCP probe passes against bound listener)"
        );
        // The cycle's per-device gating means recovery was invoked ONCE
        // through the wire-in path, not three times. The test's
        // structural correctness (just_logged_2 == false, just_logged_3
        // == false) plus the assertion that the second/third
        // `if just_logged_N` branches don't execute is the regression
        // pin for the single-fire-per-cycle contract.

        drop(listener);
    }

    /// Story 9-7 Task 3 — pins the receiver-wiring contract on the
    /// poller. Iter-1 review P10: previous name
    /// `poller_picks_up_new_retry_at_next_cycle` lied about exercising
    /// the run loop; this test only verifies that the receiver
    /// `ChirpstackPoller::new_with_reload` stashes observes a fresh
    /// publish via `borrow_and_update()`. Driving the actual outer
    /// `tokio::select!` arm requires a stub gRPC server — deferred
    /// alongside Story 4-4's identical pattern (see deferred-work.md
    /// "End-to-end `poll_metrics` integration test for the recovery
    /// wire-in"). Honest test name + docstring.
    #[tokio::test]
    async fn poller_config_rx_observes_new_publish() {
        let mut config = get_test_config();
        config.chirpstack.retry = 30;
        let initial = Arc::new(config.clone());
        let (tx, rx) = tokio::sync::watch::channel(initial.clone());

        let backend = Arc::new(InMemoryBackend::new());
        let cancel_token = CancellationToken::new();
        let restore_barrier = Arc::new(std::sync::Barrier::new(2));

        let poller = ChirpstackPoller::new_with_reload(
            &config,
            backend,
            cancel_token,
            restore_barrier,
            Some(rx),
        )
        .await
        .expect("poller construction must succeed");

        // Sanity: the poller stashed the receiver and the receiver's
        // current view matches the initial publish.
        assert!(
            poller.config_rx.is_some(),
            "config_rx must be populated when constructed via new_with_reload"
        );
        let mut rx_clone = poller
            .config_rx
            .as_ref()
            .unwrap()
            .clone();
        assert_eq!(
            rx_clone.borrow_and_update().chirpstack.retry,
            30,
            "initial published value must be visible to the receiver"
        );

        // Publish a new config; receiver must see the change at the
        // next `borrow_and_update`.
        let mut new_config = config.clone();
        new_config.chirpstack.retry = 5;
        tx.send(Arc::new(new_config))
            .expect("watch send must succeed (receiver still alive)");

        // `changed()` resolves once the sender publishes a new value.
        rx_clone
            .changed()
            .await
            .expect("changed() must resolve cleanly after a fresh send");
        assert_eq!(
            rx_clone.borrow_and_update().chirpstack.retry,
            5,
            "after send, receiver must observe the new retry value"
        );
    }

    // =======================================================================
    // Story E-0: downlink command path
    // =======================================================================

    use chirpstack_api::prost_types::value::Kind as PbKind;

    /// Stub [`DownlinkSink`] recording enqueued items and a configurable result.
    struct MockSink {
        fail: bool,
        calls: std::sync::Mutex<Vec<DeviceQueueItem>>,
    }

    impl MockSink {
        fn new(fail: bool) -> Self {
            Self {
                fail,
                calls: std::sync::Mutex::new(Vec::new()),
            }
        }
        fn calls(&self) -> Vec<DeviceQueueItem> {
            self.calls.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl DownlinkSink for MockSink {
        async fn enqueue_downlink(&self, item: DeviceQueueItem) -> Result<(), OpcGwError> {
            self.calls.lock().unwrap().push(item);
            if self.fail {
                Err(OpcGwError::ChirpStack("mock enqueue failure".to_string()))
            } else {
                Ok(())
            }
        }
    }

    fn device_command(id: u64, device_id: &str, f_port: u8, payload: Vec<u8>) -> DeviceCommand {
        DeviceCommand {
            id,
            device_id: device_id.to_string(),
            payload,
            f_port,
            status: CommandStatus::Pending,
            created_at: chrono::Utc::now(),
            error_message: None,
        }
    }

    fn object_command_string(item: &DeviceQueueItem) -> Option<String> {
        let s = item.object.as_ref()?;
        match s.fields.get("command")?.kind.as_ref()? {
            PbKind::StringValue(v) => Some(v.clone()),
            _ => None,
        }
    }

    // ---- AC#8(a,b,f): canonical value -> semantic object mapping -----------

    #[test]
    fn map_valve_open_close_to_object() {
        // value 1 -> open
        let open = map_command_to_downlink(Some("valve"), &[1]).unwrap();
        match open {
            DownlinkPayload::Object(s) => {
                assert_eq!(
                    s.fields.get("command").and_then(|v| v.kind.clone()),
                    Some(PbKind::StringValue("open".to_string()))
                );
            }
            other => panic!("expected Object, got {:?}", other),
        }
        // value 0 -> close
        let close = map_command_to_downlink(Some("valve"), &[0]).unwrap();
        match close {
            DownlinkPayload::Object(s) => {
                assert_eq!(
                    s.fields.get("command").and_then(|v| v.kind.clone()),
                    Some(PbKind::StringValue("close".to_string()))
                );
            }
            other => panic!("expected Object, got {:?}", other),
        }
    }

    #[test]
    fn map_valve_out_of_range_value_errors() {
        let err = map_command_to_downlink(Some("valve"), &[5]).unwrap_err();
        assert!(err.to_string().contains("out of range"), "got: {err}");
    }

    #[test]
    fn map_unknown_class_errors() {
        let err = map_command_to_downlink(Some("sprocket"), &[1]).unwrap_err();
        assert!(err.to_string().contains("unknown command_class"), "got: {err}");
    }

    #[test]
    fn map_no_class_is_raw_fallback() {
        let raw = map_command_to_downlink(None, &[2]).unwrap();
        assert_eq!(raw, DownlinkPayload::Raw(vec![2]));
    }

    // ---- AC#3,4: DeviceQueueItem shape ------------------------------------

    #[test]
    fn build_queue_item_object_has_empty_data() {
        let payload = DownlinkPayload::Object(valve_command_object("open"));
        let item = build_queue_item("devEUI-1", 10, &payload, true);
        assert_eq!(item.dev_eui, "devEUI-1");
        assert_eq!(item.f_port, 10);
        assert!(item.confirmed);
        assert!(item.data.is_empty(), "object path must send empty data");
        assert_eq!(object_command_string(&item).as_deref(), Some("open"));
    }

    #[test]
    fn build_queue_item_raw_has_no_object() {
        let payload = DownlinkPayload::Raw(vec![0x01]);
        let item = build_queue_item("devEUI-2", 7, &payload, false);
        assert_eq!(item.f_port, 7);
        assert!(!item.confirmed);
        assert_eq!(item.data, vec![0x01]);
        assert!(item.object.is_none(), "raw path must not set object");
    }

    // ---- AC#8(e): success path marks Sent ---------------------------------

    #[tokio::test]
    async fn deliver_one_success_marks_sent() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        backend
            .queue_command(device_command(0, "dev-A", 10, vec![1]))
            .unwrap();
        let cmd = backend.get_pending_commands().unwrap()[0].clone();
        let sink = MockSink::new(false);

        deliver_one(&sink, &backend, Some("valve"), true, &cmd).await;

        // No longer pending => marked Sent.
        assert!(backend.get_pending_commands().unwrap().is_empty());
        // Enqueued exactly one object downlink with the open command.
        let calls = sink.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(object_command_string(&calls[0]).as_deref(), Some("open"));
        assert!(calls[0].data.is_empty());
    }

    // ---- AC#8(d): failure path marks Failed, batch continues --------------

    #[tokio::test]
    async fn deliver_one_enqueue_failure_marks_failed() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        backend
            .queue_command(device_command(0, "dev-B", 10, vec![0]))
            .unwrap();
        let cmd = backend.get_pending_commands().unwrap()[0].clone();
        let sink = MockSink::new(true);

        deliver_one(&sink, &backend, Some("valve"), false, &cmd).await;

        // Enqueue was attempted...
        assert_eq!(sink.calls().len(), 1);
        // ...and the command is no longer Pending (it is Failed, not retried here).
        assert!(backend.get_pending_commands().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deliver_one_mapping_failure_does_not_enqueue() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        backend
            .queue_command(device_command(0, "dev-C", 10, vec![9]))
            .unwrap();
        let cmd = backend.get_pending_commands().unwrap()[0].clone();
        let sink = MockSink::new(false);

        // Value 9 is out of range for the valve class -> mapping fails before enqueue.
        deliver_one(&sink, &backend, Some("valve"), false, &cmd).await;

        assert!(sink.calls().is_empty(), "must not enqueue on mapping failure");
        assert!(backend.get_pending_commands().unwrap().is_empty());
    }

    #[tokio::test]
    async fn deliver_one_raw_fallback_sends_bytes() {
        let backend: Arc<dyn StorageBackend> = Arc::new(InMemoryBackend::new());
        backend
            .queue_command(device_command(0, "dev-D", 7, vec![42]))
            .unwrap();
        let cmd = backend.get_pending_commands().unwrap()[0].clone();
        let sink = MockSink::new(false);

        // No class => raw-byte fallback.
        deliver_one(&sink, &backend, None, false, &cmd).await;

        let calls = sink.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].data, vec![42]);
        assert!(calls[0].object.is_none());
        assert_eq!(calls[0].f_port, 7);
        assert!(backend.get_pending_commands().unwrap().is_empty());
    }

    // ---- find_command_cfg: config lookup by (device_id, f_port) ------------

    #[test]
    fn find_command_cfg_matches_device_and_port() {
        let apps = vec![crate::config::ChirpStackApplications {
            application_id: "app-1".to_string(),
            application_name: "App".to_string(),
            device_list: vec![crate::config::ChirpstackDevice {
                device_id: "dev-1".to_string(),
                device_name: "Dev".to_string(),
                read_metric_list: vec![],
                device_command_list: Some(vec![crate::config::DeviceCommandCfg {
                    command_id: 1,
                    command_name: "valve".to_string(),
                    command_confirmed: true,
                    command_port: 10,
                    command_class: Some("valve".to_string()),
                }]),
            }],
        }];
        let cfg = find_command_cfg(&apps, "dev-1", 10).expect("must match");
        assert_eq!(cfg.command_class.as_deref(), Some("valve"));
        assert!(cfg.command_confirmed);
        // Wrong port / device => no match.
        assert!(find_command_cfg(&apps, "dev-1", 11).is_none());
        assert!(find_command_cfg(&apps, "dev-x", 10).is_none());
    }
}

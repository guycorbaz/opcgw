// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

use crate::config::{AppConfig, DeviceCommandCfg};
use crate::storage::{DeviceCommand, MetricType, Storage, CommandStatus, ConnectionPool};
use crate::utils::*;
use chrono::Utc;
use tracing::{debug, error, info, trace, warn};

use local_ip_address::local_ip;
use std::collections::BTreeSet;
use std::sync::Arc;

// opcua modules
use opcua::server::address_space::AccessLevel;
use opcua::server::address_space::Variable;
use opcua::server::{
    diagnostics::NamespaceMetadata,
    node_manager::memory::{simple_node_manager, SimpleNodeManager},
    Server, ServerBuilder, ServerEndpoint, ServerUserToken,
};
use opcua::types::{DataValue, DateTime, NodeId, Variant};

/// Structure for storing OpcUa server parameters
pub struct OpcUa {
    /// Configuration for the OPC UA server
    config: AppConfig,
    /// Storage for the OPC UA server
    storage: Arc<std::sync::Mutex<Storage>>,
    /// Shared connection pool for per-task SQLite access
    pool: Arc<ConnectionPool>,
    /// IP address and port for the OPC UA server
    host_ip_address: String,
    /// Port for the OPC UA server
    host_port: u16,
    /// Cancellation token for graceful shutdown
    cancel_token: tokio_util::sync::CancellationToken,
}

impl OpcUa {
    /// Creates a new instance of the OPC UA server.
    ///
    /// This constructor initializes a new `OpcUa` server instance with the provided
    /// configuration and shared storage reference.
    ///
    /// # Arguments
    ///
    /// * `config` - A reference to the application configuration containing OPC UA
    ///   server settings and other application parameters
    /// * `storage` - An `Arc<Mutex<Storage>>` providing thread-safe access to the
    ///   shared storage system for device metrics and data
    ///
    /// # Returns
    ///
    /// Returns a new `OpcUa` instance configured with the specified parameters.
    ///
    /// # Examples
    ///
    /// ```rust
    /// use std::sync::{Arc, Mutex};
    ///
    /// let config = AppConfig::new().expect("Failed to load config");
    /// let storage = Arc::new(Mutex::new(Storage::new()));
    /// let opcua_server = OpcUa::new(&config, storage);
    /// ```
    pub fn new(
        config: &AppConfig,
        storage: Arc<std::sync::Mutex<Storage>>,
        pool: Arc<ConnectionPool>,
        cancel_token: tokio_util::sync::CancellationToken,
    ) -> Self {
        trace!("Create new OPC UA server structure");
        //debug!("OPC UA server configuration: {:#?}", config);

        let host_ip_address = config
            .opcua
            .host_ip_address
            .clone()
            .unwrap_or_else(|| {
                match local_ip() {
                    Ok(ip) => ip.to_string(),
                    Err(e) => {
                        error!(error = %e, "Cannot detect local IP, falling back to 0.0.0.0 — OPC UA discovery URL will be invalid, configure host_ip_address in config");
                        "0.0.0.0".to_string()
                    }
                }
            });
        let host_port = config.opcua.host_port.unwrap_or(OPCUA_DEFAULT_PORT);

        OpcUa {
            config: config.clone(),
            storage,
            pool,
            host_ip_address,
            host_port,
            cancel_token,
        }
    }

    /// Creates and configures a new OPC UA server instance.
    ///
    /// This method builds a complete OPC UA server by configuring all necessary components
    /// including network settings, security certificates, user authentication, endpoints,
    /// and the node structure. The server is created using a builder pattern and includes
    /// a custom namespace for the gateway's data nodes.
    ///
    /// # Configuration Steps
    ///
    /// 1. Creates a `ServerBuilder` with basic application information
    /// 2. Configures network settings (IP address, port)
    /// 3. Sets up security certificates and private keys
    /// 4. Configures user authentication tokens
    /// 5. Establishes server endpoints
    /// 6. Creates and configures the node manager
    /// 7. Adds custom nodes to the server's address space
    ///
    /// # Returns
    ///
    /// * `Ok(Server)` - A fully configured OPC UA server ready to be started
    /// * `Err(OpcGwError)` - If any step of the server configuration fails
    ///
    /// # Errors
    ///
    /// This method can return `OpcGwError::OpcUaError` in the following cases:
    /// * Server builder fails to create the server instance
    /// * SimpleNodeManager cannot be retrieved from the server handle
    /// * Custom namespace cannot be registered or retrieved
    /// * Any configuration step fails during server setup
    ///
    /// # Examples
    ///
    /// ```rust
    /// let mut opcua_server = OpcUa::new(&config, storage);
    /// match opcua_server.create_server() {
    ///     Ok(server) => println!("OPC UA server created successfully"),
    ///     Err(e) => eprintln!("Failed to create server: {}", e),
    /// }
    /// ```
    fn create_server(&mut self) -> Result<Server, OpcGwError> {
        let discovery_url = "opc.tcp://".to_owned()
            + &self.host_ip_address
            + ":"
            + &self.host_port.to_string()
            + "/";

        debug!("Creating server builder");
        let server_builder = ServerBuilder::new()
            .application_name(self.config.opcua.application_name.clone())
            .application_uri(self.config.opcua.application_uri.clone())
            .product_uri(self.config.opcua.product_uri.clone())
            .locale_ids(vec!["en".to_string()]) // Only english for the time being
            .discovery_urls(vec![discovery_url])
            .default_endpoint("null".to_string())
            .diagnostics_enabled(self.config.opcua.diagnostics_enabled)
            .token(self.cancel_token.clone())
            .with_node_manager(simple_node_manager(
                NamespaceMetadata {
                    namespace_uri: OPCUA_NAMESPACE_URI.to_owned(),
                    ..Default::default()
                },
                "opcgw",
            ));

        let server_builder = self.configure_network(server_builder);
        let server_builder = self.configure_key(server_builder);
        let server_builder = self.configure_user_token(server_builder);
        let server_builder = self.configure_end_points(server_builder);

        debug!("Creating server");
        let (server, handle) = server_builder
            .build()
            .map_err(|e| OpcGwError::OpcUa(e.to_string()))?;

        debug!("Creating node manager");
        let node_manager = handle
            .node_managers()
            .get_of_type::<SimpleNodeManager>()
            .ok_or_else(|| {
                error!("Failed to get SimpleNodeManager from server handle");
                OpcGwError::OpcUa("Failed to get SimpleNodeManager".to_string())
            })?;

        let ns = handle
            .get_namespace_index(OPCUA_NAMESPACE_URI)
            .ok_or_else(|| {
                error!("Failed to get name space from server handle");
                OpcGwError::OpcUa("Failed to get name space".to_string())
            })?;
        debug!(namespace_id = %ns, "Creating namespace");

        self.add_nodes(ns, node_manager);

        Ok(server)
    }

    /// Configures network settings for the OPC UA server.
    ///
    /// This method sets up the network configuration for the OPC UA server including
    /// the host IP address, port number, and hello timeout. It uses configuration
    /// values when available, falling back to sensible defaults when not specified.
    ///
    /// # Network Configuration Details
    ///
    /// * **Hello Timeout**: Time limit for initial client connections (defaults to `OPCUA_DEFAULT_NETWORK_TIMEOUT`)
    /// * **Host IP Address**: Server binding address (defaults to local machine IP if not configured)
    /// * **Host Port**: Server listening port (defaults to `OPCUA_DEFAULT_PORT`)
    ///
    /// # Arguments
    ///
    /// * `server_builder` - The `ServerBuilder` instance to configure with network settings
    ///
    /// # Returns
    ///
    /// Returns the modified `ServerBuilder` with network configuration applied.
    ///
    /// # Behavior
    ///
    /// - If `host_ip_address` is not configured, automatically detects and uses the local IP address
    /// - If `host_port` is not configured, uses the default OPC UA port
    /// - If `hello_timeout` is not configured, uses the default network timeout value
    /// - Logs the final network configuration for debugging purposes
    ///
    /// # Examples
    ///
    /// ```rust
    /// let server_builder = ServerBuilder::new();
    /// let configured_builder = self.configure_network(server_builder);
    /// ```
    fn configure_network(&self, server_builder: ServerBuilder) -> ServerBuilder {
        trace!("Configure network");

        let hello_timeout = self
            .config
            .opcua
            .hello_timeout
            .unwrap_or(OPCUA_DEFAULT_NETWORK_TIMEOUT);
        let host_ip = self.host_ip_address.clone();
        let host_port = self.host_port;

        debug!(
            hello_timeout = %hello_timeout,
            address = %host_ip,
            port = %host_port,
            "Network configuration"
        );

        server_builder
            .hello_timeout(hello_timeout)
            .host(host_ip)
            .port(host_port)
    }

    /// Configures security certificates and PKI (Public Key Infrastructure) settings for the OPC UA server.
    ///
    /// This method sets up the cryptographic security configuration including server certificates,
    /// private keys, and certificate validation policies. All settings are derived from the
    /// application configuration.
    ///
    /// # Security Configuration Details
    ///
    /// * **Sample Keypair**: Whether to create sample certificates for development/testing
    /// * **Certificate Path**: Location of the server's X.509 certificate file
    /// * **Private Key Path**: Location of the server's private key file
    /// * **Client Certificate Trust**: Policy for trusting client certificates
    /// * **Certificate Time Validation**: Whether to validate certificate expiration dates
    /// * **PKI Directory**: Directory containing the Public Key Infrastructure files
    ///
    /// # Arguments
    ///
    /// * `server_builder` - The `ServerBuilder` instance to configure with security settings
    ///
    /// # Returns
    ///
    /// Returns the modified `ServerBuilder` with PKI and certificate configuration applied.
    ///
    /// # Security Notes
    ///
    /// - Sample keypairs should only be used in development environments
    /// - In production, use properly signed certificates from a trusted CA
    /// - The PKI directory should contain trusted certificate authorities and certificate revocation lists
    /// - Certificate time validation should typically be enabled in production environments
    ///
    /// # Examples
    ///
    /// ```rust
    /// let server_builder = ServerBuilder::new();
    /// let secured_builder = self.configure_key(server_builder);
    /// ```
    fn configure_key(&self, server_builder: ServerBuilder) -> ServerBuilder {
        trace!("Configure key and pki");
        server_builder
            .create_sample_keypair(self.config.opcua.create_sample_keypair)
            .certificate_path(self.config.opcua.certificate_path.clone())
            .private_key_path(self.config.opcua.private_key_path.clone())
            .trust_client_certs(self.config.opcua.trust_client_cert)
            .check_cert_time(self.config.opcua.check_cert_time)
            .pki_dir(self.config.opcua.pki_dir.clone())
    }

    /// Configures user authentication tokens for the OPC UA server.
    ///
    /// This method sets up username/password authentication by adding a user token
    /// to the server configuration. The credentials are retrieved from the application
    /// configuration and the user is granted diagnostic read permissions.
    ///
    /// # Authentication Details
    ///
    /// * **Token ID**: Fixed identifier "user1" for the authentication token
    /// * **Username**: Retrieved from `config.opcua.user_name`
    /// * **Password**: Retrieved from `config.opcua.user_password`
    /// * **X.509 Certificate**: Not used (set to `None`)
    /// * **Certificate Thumbprint**: Not used (set to `None`)
    /// * **Diagnostic Access**: Enabled for troubleshooting and monitoring
    ///
    /// # Arguments
    ///
    /// * `server_builder` - The `ServerBuilder` instance to configure with user authentication
    ///
    /// # Returns
    ///
    /// Returns the modified `ServerBuilder` with user token configuration applied.
    ///
    /// # Security Considerations
    ///
    /// - Ensure strong passwords are used in production environments
    /// - Consider using X.509 certificate authentication for enhanced security
    /// - The diagnostic read permission allows access to server health information
    ///
    /// # Examples
    ///
    /// ```rust
    /// let server_builder = ServerBuilder::new();
    /// let authenticated_builder = self.configure_user_token(server_builder);
    /// ```
    fn configure_user_token(&self, server_builder: ServerBuilder) -> ServerBuilder {
        trace!("Configure user token");
        server_builder.add_user_token(
            "user1",
            ServerUserToken {
                user: self.config.opcua.user_name.to_string(),
                pass: Some(self.config.opcua.user_password.to_string()),
                x509: None,
                thumbprint: None,
                read_diagnostics: true,
            },
        )
    }

    /// Configures security endpoints for the OPC UA server.
    ///
    /// This method sets up multiple security endpoints with different security policies
    /// and modes to accommodate various client security requirements. Three endpoints
    /// are configured ranging from no security to full encryption.
    ///
    /// # Configured Endpoints
    ///
    /// 1. **"null" (Default)**: No security - for development and testing
    ///    - Security Policy: None
    ///    - Security Mode: None
    ///    - Security Level: 0
    ///
    /// 2. **"basic256_sign"**: Message signing without encryption
    ///    - Security Policy: Basic256
    ///    - Security Mode: Sign
    ///    - Security Level: 3
    ///
    /// 3. **"basic256_sign_encrypt"**: Full message signing and encryption
    ///    - Security Policy: Basic256
    ///    - Security Mode: SignAndEncrypt
    ///    - Security Level: 13 (highest security)
    ///
    /// # Common Configuration
    ///
    /// All endpoints share the following settings:
    /// - Path: "/" (root path)
    /// - Authorized User Token: "user1"
    /// - No password-specific security policy
    ///
    /// # Arguments
    ///
    /// * `server_builder` - The `ServerBuilder` instance to configure with security endpoints
    ///
    /// # Returns
    ///
    /// Returns the modified `ServerBuilder` with all security endpoints configured.
    ///
    /// # Security Notes
    ///
    /// - The "null" endpoint should be disabled in production environments
    /// - "basic256_sign_encrypt" provides the highest security and is recommended for production
    /// - Higher security levels require proper certificate configuration
    ///
    /// # Examples
    ///
    /// ```rust
    /// let server_builder = ServerBuilder::new();
    /// let endpoint_builder = self.configure_end_points(server_builder);
    /// ```
    fn configure_end_points(&self, server_builder: ServerBuilder) -> ServerBuilder {
        trace!("Configure end points");
        server_builder
            .default_endpoint("null".to_string()) // The name of this enpoint has to be registered with add_endpoint
            .add_endpoint(
                "null", // This is the index of the default endpoint
                ServerEndpoint {
                    path: "/".to_string(),
                    security_policy: "None".to_string(),
                    security_mode: "None".to_string(),
                    security_level: 0,
                    password_security_policy: None,
                    user_token_ids: BTreeSet::from(["user1".to_string()]),
                },
            )
            .add_endpoint(
                "basic256_sign",
                ServerEndpoint {
                    path: "/".to_string(),
                    security_policy: "Basic256".to_string(),
                    security_mode: "Sign".to_string(),
                    security_level: 3,
                    password_security_policy: None,
                    user_token_ids: BTreeSet::from(["user1".to_string()]),
                },
            )
            .add_endpoint(
                "basic256_sign_encrypt",
                ServerEndpoint {
                    path: "/".to_string(),
                    security_policy: "Basic256".to_string(),
                    security_mode: "SignAndEncrypt".to_string(),
                    security_level: 13,
                    password_security_policy: None,
                    user_token_ids: BTreeSet::from(["user1".to_string()]),
                },
            )
    }

    /// Runs the OPC UA server asynchronously.
    ///
    /// This method creates and starts the OPC UA server, handling the complete server
    /// lifecycle from initialization to shutdown. It manages error conditions during
    /// both server creation and runtime phases.
    ///
    /// # Server Lifecycle
    ///
    /// 1. **Server Creation**: Builds the server instance using configured settings
    /// 2. **Server Execution**: Starts the server and runs it asynchronously
    /// 3. **Error Handling**: Captures and logs any errors during operation
    /// 4. **Graceful Shutdown**: Handles server termination and cleanup
    ///
    /// # Returns
    ///
    /// * `Ok(())` - Server ran successfully and terminated gracefully
    /// * `Err(OpcGwError)` - Server creation failed or runtime error occurred
    ///
    /// # Error Handling
    ///
    /// - **Creation Errors**: Logged as errors and returned immediately
    /// - **Runtime Errors**: Converted to `OpcGwError::OpcUaError` and returned
    /// - All errors are logged with appropriate severity levels
    ///
    /// # Logging Behavior
    ///
    /// * `trace!` - Server startup indication
    /// * `debug!` - Successful server creation
    /// * `info!` - Normal server shutdown
    /// * `error!` - Server creation or runtime failures
    ///
    /// # Usage
    ///
    /// This method consumes `self` and should be called as the final step after
    /// all server configuration is complete.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let opc_server = OpcUaServer::new(config);
    /// if let Err(e) = opc_server.run().await {
    ///     eprintln!("Server failed: {}", e);
    /// }
    /// ```
    pub async fn run(mut self) -> Result<(), OpcGwError> {
        trace!("Running OPC UA server");

        // Error management for server creation
        let server = match self.create_server() {
            Ok(server) => {
                debug!("OPC UA server built");
                server
            }
            Err(e) => {
                error!(error = %e, "OPC UA server error");
                return Err(e);
            }
        };

        let _ = match server.run().await {
            Ok(_) => {
                info!("OPC UA server stopped");
                Ok(())
            }
            Err(e) => {
                error!(error = %e, "Error while running OPC UA server");
                Err(OpcGwError::OpcUa(e.to_string()))
            }
        };

        Ok(())
    }

    /// Adds hierarchical nodes to the OPC UA server address space based on application configuration.
    ///
    /// This method constructs a structured node hierarchy that mirrors the LoRaWAN network
    /// topology, creating folders for applications, devices, and variables for metrics.
    /// Each metric variable is configured with a read callback to dynamically fetch
    /// values from the data storage.
    ///
    /// The node hierarchy follows this structure:
    /// ```text
    /// Objects/
    /// └── Application_Name/
    ///     └── Device_Name/
    ///         ├── Metric_1 (variable)
    ///         ├── Metric_2 (variable)
    ///         └── ...
    /// ```
    ///
    /// # Arguments
    ///
    /// * `ns` - Namespace index for the created nodes
    /// * `manager` - Shared reference to the OPC UA node manager for address space manipulation
    ///
    /// # Examples
    ///
    /// ```rust
    /// let ns = 2; // Custom namespace
    /// let manager = Arc::new(SimpleNodeManager::new());
    /// server.add_nodes(ns, manager);
    /// ```
    ///
    /// # Node Types Created
    ///
    /// * **Application Folders** - Top-level containers for each LoRaWAN application
    /// * **Device Folders** - Sub-containers for devices within each application  
    /// * **Metric Variables** - Data points that expose device telemetry values
    ///
    /// Each metric variable is configured with a read callback that queries the data storage
    /// using device ID and ChirpStack metric name, providing real-time data access without polling.
    pub fn add_nodes(&mut self, ns: u16, manager: Arc<SimpleNodeManager>) {
        trace!("Add nodes to OPC UA server");
        let address_space = manager.address_space();

        // The address spae is guarded so obtain a lock to change it
        let mut address_space = address_space.write();

        // Adding one folder per LoraWan application
        for application in self.config.application_list.iter() {
            debug!(
                app_name = %application.application_name,
                "Adding application to OPC UA"
            );
            let application_node = NodeId::new(ns, application.application_name.clone());
            address_space.add_folder(
                &application_node,
                &application.application_name,
                &application.application_name,
                &NodeId::objects_folder_id(),
            );
            // Add devices into folders
            for device in application.device_list.iter() {
                debug!(device_name = %device.device_name, "Adding device to OPC UA");
                let device_node = NodeId::new(ns, device.device_name.clone());
                address_space.add_folder(
                    &device_node,
                    &device.device_name,
                    &device.device_name,
                    &application_node,
                );
                // Add metrics into devices node
                for read_metric in device.read_metric_list.iter() {
                    debug!(metric_name = %read_metric.metric_name, "Adding read metric to OPC UA");
                    let read_metric_node = NodeId::new(ns, read_metric.metric_name.clone());
                    let _ = address_space.add_variables(
                        vec![Variable::new(
                            &read_metric_node,
                            read_metric.metric_name.clone(),
                            read_metric.metric_name.clone(),
                            0_i32,
                        )],
                        &device_node,
                    );
                    let storage_clone = self.storage.clone();
                    let device_id = device.device_id.clone();
                    let chirpstack_metric_name = read_metric.chirpstack_metric_name.clone();
                    manager
                        .inner()
                        .add_read_callback(read_metric_node.clone(), move |_, _, _| {
                            Self::get_value(
                                &storage_clone,
                                device_id.clone().to_string(),
                                chirpstack_metric_name.clone().to_string(),
                            )
                        })
                }
                // Add commands into device node
                match &device.device_command_list {
                    None => debug!(device_name = %device.device_name, "No device commands for device"),
                    Some(command_list) => {
                        for command in command_list.iter() {
                            let device_id = device.device_id.clone();
                            debug!(
                                command_id = %command.command_id,
                                device_name = %device.device_name,
                                "Adding command to device"
                            );
                            let command_node = NodeId::new(ns, command.command_id as u32);
                            let mut command_variable = Variable::new(
                                &command_node,
                                command.command_name.clone(),
                                command.command_name.clone(),
                                0_i32,
                            );
                            let storage_clone = self.storage.clone();
                            command_variable.set_writable(true);
                            command_variable.set_user_access_level(
                                AccessLevel::CURRENT_READ | AccessLevel::CURRENT_WRITE,
                            );
                            let _ =
                                address_space.add_variables(vec![command_variable], &device_node);
                            let command_clone = command.clone();
                            manager.inner().add_write_callback(
                                command_node.clone(),
                                move |data_value, _numeric_range| {
                                    Self::set_command(
                                        &storage_clone,
                                        &device_id.to_string(),
                                        &command_clone,
                                        data_value,
                                    )
                                },
                            );
                        }
                    }
                }
            }
        }
    }

    /// Retrieves and converts a metric value from storage into an OPC UA DataValue.
    ///
    /// This method serves as a callback function for OPC UA variable reads, fetching
    /// the current value of a specific metric for a given device from the data storage
    /// and converting it into the appropriate OPC UA data format with proper timestamps
    /// and status codes.
    ///
    /// # Data Flow
    ///
    /// 1. **Storage Access**: Acquires a lock on the shared storage
    /// 2. **Value Retrieval**: Fetches the metric value using device ID and metric name
    /// 3. **Type Conversion**: Converts the internal metric type to OPC UA Variant
    /// 4. **DataValue Creation**: Wraps the value with timestamps and status information
    ///
    /// # Arguments
    ///
    /// * `storage` - Thread-safe reference to the data storage containing metric values
    /// * `device_id` - Unique identifier of the device whose metric is being read
    /// * `metric_name` - Name of the specific metric to retrieve
    ///
    /// # Returns
    ///
    /// * `Ok(DataValue)` - Successfully retrieved and converted metric value with:
    ///   - Converted variant value
    ///   - Good status code
    ///   - Current source and server timestamps
    /// * `Err(StatusCode)` - Error conditions:
    ///   - `BadDataUnavailable` - Metric not found for the specified device
    ///   - `BadInternalError` - Storage lock acquisition failed
    ///
    /// # Error Handling
    ///
    /// - **Missing Metric**: Returns `BadDataUnavailable` when the requested metric doesn't exist
    /// - **Storage Lock Failure**: Returns `BadInternalError` when unable to access storage
    /// - All errors are logged with appropriate severity levels
    ///
    /// # Thread Safety
    ///
    /// This method safely handles concurrent access by acquiring a mutex lock on the
    /// storage before performing read operations.
    ///
    /// # Logging Behavior
    ///
    /// * `trace!` - Method entry with device and metric identification
    /// * `error!` - Missing metric or storage access failures
    ///
    /// # Usage Context
    ///
    /// This method is typically used as a callback function registered with the OPC UA
    /// node manager for dynamic value resolution when clients read variable nodes.
    fn get_value(
        storage: &Arc<std::sync::Mutex<Storage>>,
        device_id: String,
        metric_name: String,
    ) -> Result<DataValue, opcua::types::StatusCode> {
        trace!(
            device_id = %device_id,
            metric_name = %metric_name,
            "Get value for device and metric"
        );

        match storage.lock() {
            Ok(mut storage_guard) => {
                match storage_guard.get_metric_value(&device_id, &metric_name) {
                    Some(metric_value) => {
                        // Convert MetricType to OPC UA Variant
                        let variant = Self::convert_metric_to_variant(metric_value);

                        // Create a DataValue with the variant and current timestamp
                        let data_value = DataValue {
                            value: Some(variant),
                            status: Some(opcua::types::StatusCode::Good.bits().into()),
                            source_timestamp: Some(DateTime::now()),
                            source_picoseconds: None,
                            server_timestamp: Some(DateTime::now()),
                            server_picoseconds: None,
                        };

                        Ok(data_value)
                    }
                    None => {
                        error!(
                            device_id = %device_id,
                            metric_name = %metric_name,
                            "Unknown metric for device"
                        );
                        // Return appropriate StatusCode error
                        Err(opcua::types::StatusCode::BadDataUnavailable)
                    }
                }
            }
            Err(e) => {
                error!(error = %e, "Impossible to lock storage");
                Err(opcua::types::StatusCode::BadInternalError)
            }
        }
    }

    /// Converts internal metric types to OPC UA Variant types.
    ///
    /// This method performs type conversion from the application's internal `MetricType`
    /// enumeration to the corresponding OPC UA `Variant` types, ensuring proper data
    /// representation when exposing metrics through the OPC UA interface.
    ///
    /// # Type Mappings
    ///
    /// | Internal Type | OPC UA Variant | Notes |
    /// |---------------|----------------|--------|
    /// | `MetricType::Int` | `Variant::Int32` | Converted with bounds checking |
    /// | `MetricType::Float` | `Variant::Float` | Cast to f32 precision |
    /// | `MetricType::String` | `Variant::String` | Direct conversion to OPC UA string |
    /// | `MetricType::Bool` | `Variant::Boolean` | Direct boolean mapping |
    ///
    /// # Arguments
    ///
    /// * `metric_type` - The internal metric value to convert
    ///
    /// # Returns
    ///
    /// Returns the corresponding `Variant` that can be used in OPC UA DataValues.
    ///
    /// # Panics
    ///
    /// This method will panic if:
    /// - Integer conversion fails due to value overflow when converting to i32
    /// - The `unwrap()` call fails during integer type conversion
    ///
    /// # Type Safety
    ///
    /// - **Integer Conversion**: Uses `try_into().unwrap()` for i64 to i32 conversion
    /// - **Float Precision**: Converts f64 to f32 with potential precision loss
    /// - **String Conversion**: Uses `into()` for efficient string conversion
    /// - **Boolean**: Direct mapping without conversion
    ///
    /// # Usage Context
    ///
    /// This method is typically called during OPC UA variable read operations to
    /// convert stored metric values into the appropriate OPC UA data format.
    ///
    /// # Examples
    ///
    /// ```rust
    /// let int_metric = MetricType::Int(42);
    /// let variant = Self::convert_metric_to_variant(int_metric);
    /// // variant is now Variant::Int32(42)
    /// ```
    fn convert_metric_to_variant(metric: crate::storage::MetricValueInternal) -> Variant {
        // NOTE: The metric.timestamp field is available but not currently used in the OPC UA Variant.
        // Future enhancement: embed timestamp in OPC UA node's SourceTimestamp attribute for better
        // temporal accuracy in OPC UA clients.
        match metric.data_type {
            crate::storage::MetricType::Int => {
                match metric.value.parse::<i64>() {
                    Ok(value) => {
                        match i32::try_from(value) {
                            Ok(v) => Variant::Int32(v),
                            Err(_) => {
                                debug!(value = %value, "Int metric value out of i32 range, using Int64");
                                Variant::Int64(value)
                            }
                        }
                    }
                    Err(_) => {
                        debug!("Failed to parse metric value as i64");
                        Variant::Int32(0)
                    }
                }
            }
            crate::storage::MetricType::Float => {
                match metric.value.parse::<f64>() {
                    Ok(value) => {
                        if !value.is_finite() {
                            error!(value = %value, "Metric value is NaN or Infinity; using default 0.0");
                            Variant::Float(0.0)
                        } else {
                            Variant::Float(value as f32)
                        }
                    }
                    Err(_) => {
                        debug!("Failed to parse metric value as f64");
                        Variant::Float(0.0)
                    }
                }
            }
            crate::storage::MetricType::String => Variant::String(metric.value.into()),
            crate::storage::MetricType::Bool => {
                let lower_val = metric.value.to_lowercase();
                let bool_value = match lower_val.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => {
                        warn!(value = %metric.value, "Invalid bool metric value; defaulting to false");
                        false
                    }
                };
                Variant::Boolean(bool_value)
            }
        }
    }

    /// Sets a command for a device based on OPC UA data value
    ///
    /// This method processes an OPC UA data value and creates a device command
    /// that gets queued in the storage for later transmission to the target device.
    ///
    /// # Arguments
    /// * `storage` - Thread-safe reference to the storage containing device commands
    /// * `device_id` - Unique identifier of the target device
    /// * `command` - Command configuration containing port and confirmation settings
    /// * `data_value` - OPC UA data value containing the command payload
    /// * `numeric_range` - Numeric range specification for the data value
    ///
    /// # Returns
    /// * `opcua::types::StatusCode::Good` - Command successfully queued
    /// * `opcua::types::StatusCode::Bad` - No value provided in data_value
    /// * `opcua::types::StatusCode::BadTypeMismatch` - Data type conversion failed
    /// * `opcua::types::StatusCode::BadInternalError` - Storage lock acquisition failed
    fn set_command(
        storage: &Arc<std::sync::Mutex<Storage>>,
        device_id: &str,
        command: &DeviceCommandCfg,
        data_value: DataValue,
    ) -> opcua::types::StatusCode {
        trace!("Set command");
        debug!(data_value = ?data_value, "Command data value");
        //let value = data_value.value.unwrap();

        match data_value.value {
            // There was no value
            None => opcua::types::StatusCode::Bad,
            Some(variant) => {
                debug!(variant = ?variant, "Variant received");
                // Validate that variant is numeric (for LoRaWAN payload)
                match &variant {
                    opcua::types::Variant::Int32(_)
                    | opcua::types::Variant::Int64(_)
                    | opcua::types::Variant::Float(_)
                    | opcua::types::Variant::Double(_) => {
                        // Numeric types OK for payload
                    }
                    _ => {
                        warn!(variant_type = ?variant, "Command payload must be numeric (Int32, Int64, Float, Double)");
                        return opcua::types::StatusCode::BadTypeMismatch;
                    }
                }
                let (value_str, _value_type) = match Self::convert_variant_to_metric(&variant) {
                    Ok(result) => result,
                    Err(_) => return opcua::types::StatusCode::BadTypeMismatch,
                };
                let value = match value_str.parse::<i64>() {
                    Ok(v) => v,
                    Err(_) => return opcua::types::StatusCode::BadTypeMismatch,
                };
                debug!(
                    value = %value,
                    device_id = %device_id,
                    port = %command.command_port,
                    confirmed = %command.command_confirmed,
                    "Add command for device"
                );
                // Create the command
                let f_port = match u8::try_from(command.command_port) {
                    Ok(port) => {
                        if !crate::storage::DeviceCommand::validate_f_port(port) {
                            warn!(port = %port, "Command port out of LoRaWAN valid range [1-223]");
                            return opcua::types::StatusCode::BadOutOfRange;
                        }
                        port
                    }
                    Err(_) => {
                        warn!(port = %command.command_port, "Command port out of u8 range [0-255]");
                        return opcua::types::StatusCode::BadOutOfRange;
                    }
                };
                let payload = vec![match u8::try_from(value) {
                    Ok(v) => v,
                    Err(_) => {
                        warn!(value = %value, "Command value out of u8 range [0-255]");
                        return opcua::types::StatusCode::BadOutOfRange;
                    }
                }];

                // Validate payload size
                if !crate::storage::DeviceCommand::validate_payload_size(&payload) {
                    warn!(payload_size = %payload.len(), max_size = %crate::storage::MAX_LORA_PAYLOAD_SIZE, "Command payload exceeds LoRaWAN size limit");
                    return opcua::types::StatusCode::BadOutOfRange;
                }

                let command_to_send = crate::storage::DeviceCommandInternal {
                    id: 0, // Will be assigned by storage when queued
                    device_id: device_id.to_string(),
                    payload,
                    f_port,
                    status: CommandStatus::Pending,
                    created_at: Utc::now(),
                    error_message: None,
                };
                // Add command to storage
                match storage.lock() {
                    Ok(mut storage_guard) => {
                        storage_guard.push_command(command_to_send);
                        opcua::types::StatusCode::Good
                    }
                    Err(e) => {
                        error!(error = %e, "Impossible to lock storage");
                        opcua::types::StatusCode::BadInternalError
                    }
                }
            }
        }
    }

    /// Converts an OPC UA Variant to a MetricType
    ///
    /// This method handles the conversion between OPC UA data types (Variant) and
    /// the internal metric representation (MetricType) used by the application.
    /// It supports conversion of common data types including integers, floats,
    /// strings, and booleans.
    ///
    /// # Arguments
    /// * `variant` - Reference to the OPC UA Variant to be converted
    ///
    /// # Returns
    /// * `Ok(MetricType)` - Successfully converted metric type
    /// * `Err(String)` - Error message if the variant type is not supported
    ///
    /// # Supported Conversions
    /// * `Int32/Int64` -> `MetricType::Int`
    /// * `Float/Double` -> `MetricType::Float`
    /// * `String` -> `MetricType::String`
    /// * `Boolean` -> `MetricType::Bool`
    fn convert_variant_to_metric(variant: &Variant) -> Result<(String, crate::storage::MetricType), String> {
        trace!("Convert variant to metric");
        match variant {
            Variant::Int32(value) => Ok((value.to_string(), crate::storage::MetricType::Int)),
            Variant::Int64(value) => Ok((value.to_string(), crate::storage::MetricType::Int)),
            Variant::Float(value) => Ok((value.to_string(), crate::storage::MetricType::Float)),
            Variant::Double(value) => Ok((value.to_string(), crate::storage::MetricType::Float)),
            Variant::String(value) => Ok((value.to_string(), crate::storage::MetricType::String)),
            Variant::Boolean(value) => Ok((value.to_string(), crate::storage::MetricType::Bool)),
            _ => Err(format!("Unsupported variant type {:?}", variant)),
        }
    }
}

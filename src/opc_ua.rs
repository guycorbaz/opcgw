// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage opc ua server

#![allow(unused)]

use crate::config::{AppConfig, ChirpstackDevice, OpcUaConfig};
use crate::storage::{MetricType, Storage};
use crate::utils::{OpcGwError, OPCUA_ADDRESS_SPACE};
use log::{debug, error, info, trace, warn};
use opcua::server::prelude::*;
use opcua::sync::Mutex;
//use std::sync::Mutex;
use local_ip_address::local_ip;
use opcua::sync::RwLock;
use opcua::types::variant::Variant::Float;
use opcua::types::DataTypeId::Integer;
use opcua::types::VariableId::OperationLimitsType_MaxNodesPerTranslateBrowsePathsToNodeIds;
use std::option::Option;
use std::path::PathBuf;
use std::sync::Arc;

/// Structure for storing OpcUa server parameters
pub struct OpcUa {
    /// Application configuration parameters
    pub config: AppConfig,
    /// OPC UA server config
    pub server_config: ServerConfig,
    /// opc ua server instance
    pub server: Arc<RwLock<Server>>,
    /// Index of the opc ua address space
    pub ns: u16,
    /// Metrics list
    pub storage: Arc<std::sync::Mutex<Storage>>,
}

impl OpcUa {
    /// Creates a new OPC UA structure with the given configuration and storage.
    ///
    /// This function initializes the OPC UA server configuration using the provided
    /// configuration file path. It retrieves the local IP address to configure the TCP
    /// settings of the OPC UA server. The function then creates an OPC UA server instance,
    /// registers a namespace, and returns an `OpcUa` structure encapsulating the server and other
    /// necessary components.
    ///
    /// # Arguments
    ///
    /// * `config` - A reference to the `AppConfig` structure containing the application configuration.
    /// * `storage` - An `Arc` wrapped `Mutex` for thread-safe access to the storage.
    ///
    /// # Returns
    ///
    /// Returns an instance of the `OpcUa` structure initialized with the provided configuration and storage.
    ///
    pub fn new(config: &AppConfig, storage: Arc<std::sync::Mutex<Storage>>) -> Self {
        trace!("New OPC UA structure");
        // Create de server configuration using the provided config file path
        //trace!("opcua config file is {:?}", config.opcua.config_file);
        let mut server_config = Self::create_server_config(&config.opcua.config_file.clone());

        let my_ip_address = local_ip().unwrap();
        //trace!("Server IP address: {}", my_ip_address);
        server_config.tcp_config.host = my_ip_address.to_string();
        //trace!("OPC UA server configuration: {:#?}", server_config);

        // Create a server instance and wrap it in an Arc and RwLock for safe shared access
        let server = Arc::new(RwLock::new(Self::create_server(server_config.clone())));

        // Register the namespace in the OPC UA server
        let ns = {
            // Access the server's address space in read mode
            let address_space = {
                // Access the RwLock in read mode and then call the address_space method
                let server = server.read();
                server.address_space()
            };
            // Lock the address space for writing and register the namespace
            let mut address_space = address_space.write();
            address_space
                .register_namespace(OPCUA_ADDRESS_SPACE)
                .unwrap()
        };

        // Return the new OpcUa structure
        OpcUa {
            config: config.clone(),
            server_config,
            server,
            ns,
            storage,
        }
    }

    /// Creates a server configuration from the given file name.
    ///
    /// This function attempts to load a server configuration from the specified file name.
    /// If the configuration is loaded successfully, it returns the server configuration.
    /// In the event of an error, it will panic and provide a detailed error message.
    ///
    /// # Arguments
    ///
    /// * `config_file_name` - A reference to the name of the configuration file.
    ///
    /// # Returns
    ///
    /// * `ServerConfig` - The loaded server configuration.
    ///
    /// # Panics
    ///
    /// This function will panic if it cannot load the server configuration from the given file name.
    /// The error message will be wrapped in `OpcGwError::OpcUaError`.
    fn create_server_config(config_file_name: &String) -> ServerConfig {
        debug!("Creating server config");
        trace!("opcua config file is {:?}", config_file_name);
        // Attempt to load the server configuration from the given file name
        match ServerConfig::load(&PathBuf::from(config_file_name)) {
            // If successful, return the loaded configuration
            Ok(config) => config,

            // If an error occurs, panic and provide a detailed error message
            Err(e) => panic!(
                "{}",
                OpcGwError::OpcUaError(format!("Can not create server config {:?}", e))
            ),
        }
    }

    /// Creates a new server instance with the given configuration.
    ///
    /// # Arguments
    ///
    /// * `server_config` - Configuration settings for the server.
    ///
    /// # Returns
    ///
    /// * A newly created `Server` instance.
    ///
    /// # Example
    ///
    /// ```rust
    /// let config = ServerConfig::new();
    /// let server = create_server(config);
    /// ```
    fn create_server(server_config: ServerConfig) -> Server {
        debug!("Creating server");
        Server::new(server_config.clone())
    }

    /// Runs the OPC UA server asynchronously.
    ///
    /// This function performs the following actions:
    /// 1. Logs a debug message indicating that the OPC UA server is running.
    /// 2. Populates the address space for the server.
    /// 3. Creates and awaits the server task to run indefinitely.
    ///
    /// # Errors
    ///
    /// Returns an `OpcGwError` if any operation within the function fails.
    ///
    /// # Examples
    ///
    /// ```
    /// // Assuming `opc_gw` is an instance of a struct that has the `run` method
    /// opc_gw.run().await?;
    /// ```
    ///
    /// # Panics
    ///
    /// The function does not explicitly handle any panics. It is expected that any
    /// panics that occur within the function should be handled by the caller.
    ///
    /// # Notes
    ///
    /// Ensure that the server is properly configured and the address space is
    /// correctly populated before calling this function.
    ///
    /// # async
    ///
    /// This function is asynchronous and should be awaited.
    pub async fn run(&self) -> Result<(), OpcGwError> {
        debug!("Running OPC UA server");
        self.populate_address_space();
        let server_task = Server::new_server_task(self.server.clone());
        // Run the server indefinitely
        server_task.await;
        Ok(())
    }

    /// Populates the server's address space with applications and their devices.
    ///
    /// This method first reads the current server state and accesses the server's address space.
    /// It then iterates over the list of applications specified in the configuration, adding each
    /// application and its associated devices to the address space as folders and variables respectively.
    ///
    /// # Steps:
    /// 1. Read the server state.
    /// 2. Access the server's address space.
    /// 3. Obtain a writable reference to the address space.
    /// 4. Iterate through the application's list:
    ///     a. Add a folder for each application.
    ///     b. For each device in the application, add a folder under the application's folder.
    ///     c. Add variables for each device in the address space.
    ///
    /// # Panics:
    /// The function will panic if any `unwrap` calls fail, indicating an error in adding folders or variables.
    ///
    /// # Example:
    /// ```rust
    /// // Assuming `server` is an instance of your server type and `config` is properly set up
    /// server.populate_address_space();
    /// ```
    ///
    /// # Note:
    /// This function assumes that the server, configuration, and address space are logically and syntactically correct.
    pub fn populate_address_space(&self) {
        // Read the server state
        let server = self.server.read();
        // Access the server's address space
        let address_space = server.address_space();
        // Obtain writable reference to the address space
        let mut address_space = address_space.write();
        let app = self.config.application_list.clone();
        for application in app {
            // Adding application level folder
            let folder_id = address_space
                .add_folder(
                    application.application_name.clone(),
                    application.application_name.clone(),
                    &NodeId::objects_folder_id(),
                )
                .unwrap();
            for device in application.device_list {
                // Adding device under the application folder
                let device_id = address_space
                    .add_folder(
                        device.device_name.clone(),
                        device.device_name.clone(),
                        &folder_id,
                    )
                    .unwrap();
                address_space.add_variables(
                    // Add variables to the device in address space
                    self.create_variables(&device),
                    &device_id,
                );
            }
        }
    }

    /// Creates OPC UA variables for each metric in the given ChirpstackDevice.
    ///
    /// This method iterates over the list of metrics from the provided `ChirpstackDevice`
    /// and creates corresponding OPC UA variables for each metric. Each variable is assigned
    /// a unique `NodeId` and an initial value of `Float(0.0)`. A getter function is also created
    /// for each variable to fetch its value from the storage.
    ///
    /// # Parameters
    ///
    /// * `&self`: A reference to the current instance of the struct.
    /// * `device`: A reference to a `ChirpstackDevice` that contains the metrics to be converted into OPC UA variables.
    ///
    /// # Returns
    ///
    /// * `Vec<Variable>`: A vector containing the generated OPC UA variables.
    ///
    /// # Example
    ///
    /// ```
    /// let device = ChirpstackDevice::new(...);
    /// let variables = self.create_variables(&device);
    /// for variable in variables {
    ///     println!("Created variable: {:?}", variable);
    /// }
    /// ```
    ///
    /// # Panics
    ///
    /// This function does not explicitly panic, but the caller is responsible for ensuring
    /// that the provided `ChirpstackDevice` is properly constructed and contains valid metrics.
    ///
    /// # Notes
    ///
    /// * The `self` reference is cloned and moved into a closure to handle asynchronous value fetching.
    /// * Each metric and its corresponding node ID are wrapped in `Arc` and `Mutex` for thread-safe access within the getter closure.
    fn create_variables(&self, device: &ChirpstackDevice) -> Vec<Variable> {
        trace!("Creating opc ua variables");

        // Initialize an empty vector to store the generated variables
        let mut variables = Vec::<Variable>::new();

        // Iterate over each metric in the device's metric list
        for metric in device.metric_list.clone() {
            let metric_name = metric.metric_name.clone();
            let chirpstack_metric_name = metric.chirpstack_metric_name.clone();
            trace!("Creating variable for metric {:?}", &metric_name);

            // Create the variable node id for the metric
            let metric_node_id = NodeId::new(self.ns, metric_name.clone());

            // Move self and metric_node_id into the closure
            let device_id = device.device_id.clone();
            let self_arc = Arc::new(self);
            let metric_node_id_arc = Arc::new(metric_node_id.clone());
            let chirpstack_metric_name_arc = Arc::new(chirpstack_metric_name.clone());
            let storage = self.storage.clone();

            // Create a new Variable with the node, name, and an initial value
            let mut metric_variable = Variable::new(
                &metric_node_id,
                metric_name.clone(),
                metric_name,
                Float(0.0),
            );

            // Crete getter
            let getter = AttrFnGetter::new(
                move |_, _, _, _, _, _| -> Result<Option<DataValue>, StatusCode> {
                    //trace!("Get variable value");
                    let dev_id = device_id.clone();
                    let id = metric_node_id_arc.clone();
                    let name = chirpstack_metric_name_arc.clone();
                    let value =
                        get_metric_value(&device_id.clone(), &name.clone(), storage.clone());
                    Ok(Some((DataValue::new_now(value))))
                },
            );

            metric_variable.set_value_getter(Arc::new(Mutex::new(getter)));
            // Add variable to variables list

            variables.push(metric_variable);
        }
        variables
    }
}

/// Retrieves the value of a specified metric for a given device from storage.
///
/// # Arguments
///
/// * `device_id` - A reference to a `String` that holds the identifier of the device.
/// * `chirpstack_metric_name` - A reference to a `String` that contains the name of the metric to retrieve.
/// * `storage` - An `Arc` wrapped around a `Mutex` that allows shared access to the `Storage` structure.
///
/// # Returns
///
/// The value of the specified metric as an `f32`. If the metric value is not found or not of type `Float`, it returns `0.0`.
///
/// # Panics
///
/// This function will panic if it fails to lock the `Mutex` for storage.
///
/// # Examples
///
/// ```rust
/// let value = get_metric_value(&device_id, &metric_name, storage);
/// println!("Metric value: {}", value);
/// ```
fn get_metric_value(
    device_id: &String,
    chirpstack_metric_name: &String,
    storage: Arc<std::sync::Mutex<Storage>>,
) -> f32 {
    trace!("Get metric value for {:?}", &chirpstack_metric_name);
    let storage = storage.clone();
    let mut storage = storage
        .lock()
        .expect(format!("Mutex for storage is poisoned").as_str());
    let device = storage.get_device(device_id).unwrap();
    let value = storage.get_metric_value(device_id, chirpstack_metric_name);

    trace!("Value of metric is: {:?}", value);
    let metric_value = match value {
        Some(MetricType::Float(v)) => v,
        _ => 0.0,
    };
    metric_value as f32
}

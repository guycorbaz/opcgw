// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage opc ua server

#![allow(unused)]

use crate::config::{OpcUaConfig, AppConfig, ChirpstackDevice};
use crate::utils::{OpcGwError,OPCUA_ADDRESS_SPACE};
use crate::storage::{MetricType, Storage};
use log::{debug, error, info, trace, warn};
use opcua::server::prelude::*;
use opcua::sync::Mutex;
//use std::sync::Mutex;
use opcua::sync::RwLock;
use opcua::types::VariableId::OperationLimitsType_MaxNodesPerTranslateBrowsePathsToNodeIds;
use opcua::types::variant::Variant::{Float};
use std::option::Option;
use std::path::PathBuf;
use std::sync::Arc;
use opcua::types::DataTypeId::Integer;

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

    /// Creates a new instance of the OPC UA structure using the provided configuration.
    ///
    /// This function performs the following steps:
    /// 1. Creates the server configuration using the provided config file path.
    /// 2. Creates a server instance and wraps it in an `Arc` and `RwLock` for safe shared access.
    /// 3. Registers the namespace in the OPC UA server.
    ///
    /// # Arguments
    ///
    /// * `config` - A reference to the `AppConfig` struct which holds the configuration data.
    ///
    /// # Returns
    ///
    /// A new instance of `Self`.
    pub fn new(config: &AppConfig, storage: Arc<std::sync::Mutex<Storage>>) -> Self {
        trace!("New OPC UA structure");
        // Create de server configuration using the provided config file path
        //trace!("opcua config file is {:?}", config.opcua.config_file);
        let server_config = Self::create_server_config(&config
            .opcua.config_file
            .clone());

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


    /// Creates the server configuration from the specified configuration file.
    ///
    /// # Arguments
    ///
    /// * `config_file_name` - A string slice that holds the name of the configuration file.
    ///
    /// # Returns
    ///
    /// * `ServerConfig` - The created server configuration.
    ///
    /// # Panics
    ///
    /// The function will panic if the server configuration cannot be created due to an error.
    ///
    /// # Example
    ///
    /// ```
    /// let config = create_server_config(&"config.yaml".to_string());
    /// ```
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
    /// This function initializes and runs an OPC UA server task. The server runs
    /// indefinitely until it receives a termination signal.
    ///
    /// # Returns
    ///
    /// * `Result<(), OpcGwError>` - Returns `Ok(())` if the server
    pub async fn run(&self) -> Result<(), OpcGwError> {
        debug!("Running OPC UA server");
        self.populate_address_space();
        let server_task = Server::new_server_task(self.server.clone());
        // Run the server indefinitely
        server_task.await;
        Ok(())
    }

    /// Populates the address space with applications and their devices.
    ///
    /// This method reads the server state and accesses the server's address space.
    /// It then iterates over the list of applications from the configuration, adding each application
    /// as a folder in the address space. For each application, it adds its devices as subfolders
    /// and attaches the respective variables to each device.
    ///
    /// # Panics
    /// This method will panic if there is a failure in adding folders or variables to the address space.
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
                .add_folder(application.application_name.clone(),
                            application.application_name.clone(),
                            &NodeId::objects_folder_id())
                .unwrap();
            for device in application.device_list {
                // Adding device under the application folder
                let device_id = address_space
                    .add_folder(device.device_name.clone(),
                                device.device_name.clone(),
                                &folder_id)
                    .unwrap();
                address_space.add_variables(
                    // Add variables to the device in address space
                    self.create_variables(&device), &device_id
                );
            }
        }
    }


    /// Creates OPC UA variables from a given ChirpstackDevice.
    ///
    /// This function initializes an empty vector to store `Variable` instances,
    /// iterates over each metric in the device's metric list, clones the metric name,
    /// creates a new `NodeId` for the variable, and pushes the new `Variable` into the vector.
    ///
    /// # Parameters
    /// - `device`: A reference to a `ChirpstackDevice` containing the metrics from which
    ///             the OPC UA variables will be created.
    ///
    /// # Returns
    /// A vector of `Variable` instances created from the device's metrics.
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
                Float(0.0));

            // Crete getter
            let getter = AttrFnGetter::new(
                move | _, _, _, _, _, _, | -> Result<Option<DataValue>, StatusCode> {
                    //trace!("Get variable value");
                    let dev_id = device_id.clone();
                    let id = metric_node_id_arc.clone();
                    let name = chirpstack_metric_name_arc.clone();
                    let value = get_metric_value(&device_id.clone(), &name.clone(), storage.clone());
                    Ok(Some((DataValue::new_now(value))))
                }
            );

            metric_variable.set_value_getter(Arc::new(Mutex::new(getter)));
            // Add variable to variables list

            variables.push(metric_variable);
        }
        variables
    }


}


/// Retrieves the value of a specified metric for a given device from the storage.
///
/// # Arguments
///
/// * `device_id` - A reference to a string that holds the ID of the device.
/// * `chirpstack_metric_name` - A reference to a string that holds the name of the metric to retrieve.
/// * `storage` - An `Arc` wrapped around a `Mutex` protected `Storage` object.
///
/// # Returns
///
/// * `f32` - The value of the metric as a floating point number. If the metric type is not `Float`, returns 0.0.
///
fn get_metric_value(device_id: &String, chirpstack_metric_name: &String, storage: Arc<std::sync::Mutex<Storage>>) -> f32 {
    trace!("Get metric value for {:?}", &chirpstack_metric_name);
    let storage = storage.clone();
    let storage = storage.lock()
        .expect(format!("Mutex for storage is poisoned").as_str());
    let device = storage.devices.get(device_id).unwrap();
    let value = storage.get_metric_value(device_id, chirpstack_metric_name);

    trace!("Value of metric is: {:?}", value);
    let metric_value = match value {
        MetricType::Float(v) => v,
        _ => 0.0,
    };
    metric_value as f32
}

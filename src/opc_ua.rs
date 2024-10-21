// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Manage opc ua server

#![allow(unused)]

use crate::config::{OpcUaConfig, AppConfig};
use crate::utils::{OpcGwError,OPCUA_ADDRESS_SPACE};
use log::{debug, error, info, trace, warn};
use opcua::server::prelude::*;
use opcua::sync::Mutex;
use opcua::sync::RwLock;
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
    pub fn new(config: &AppConfig) -> Self {
        trace!("New OPC UA structure");
        // Create de server configuration using the provided config file path
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

    /// Populate the address space with parameters from config
    pub fn populate_address_space(&self) {
        let server = self.server.read();
        let address_space = server.address_space();
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
                // Adding devices in applications
                let device_id = address_space
                    .add_folder(device.device_name.clone(),
                                device.device_name.clone(),
                                &folder_id)
                    .unwrap();
                for metric in device.metric_list {

                    println!("{}, {}. {}", application.application_name, device.device_name, metric.metric_name);
                }
            }
        }
    }
}

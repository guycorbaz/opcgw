// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//TODO: Remove for production
#![allow(unused)]

use std::collections::BTreeMap;
use crate::config::{AppConfig, ChirpstackDevice, OpcUaConfig};
use crate::storage::{MetricType, Storage};
use crate::utils::{OpcGwError, OPCUA_ADDRESS_SPACE};
use log::{debug, error, info, trace, warn};
use tonic::transport::Endpoint;

use local_ip_address::local_ip;
use std::option::Option;
use std::path::PathBuf;
use std::sync::Arc;
use std::collections::BTreeSet;


// opcua modules
use opcua::crypto::SecurityPolicy;
use opcua::server::{
    diagnostics::NamespaceMetadata,
    node_manager::memory::{simple_node_manager, SimpleNodeManager},
    ServerBuilder, ServerConfig, ServerEndpoint, ServerUserToken,
    Limits, SubscriptionLimits, OperationalLimits,
};
use opcua::types::{MessageSecurityMode};



/// Structure for storing OpcUa server parameters
pub struct OpcUa {
    // OPC UA server config
    //pub server_builder: ServerBuilder,
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
        trace!("Create new OPC UA structure");
        //let server_builder = Self::create_server_builder();
  
        OpcUa {
        //    server_builder,
        }
    }

       fn create_server_builder() -> ServerBuilder {
        debug!("Creating ServerBuilder");

        //TODO: configure server from opcua configuration file
        let server_builder =ServerBuilder::new()
            .application_name("Chirpstack OPC UA Gateway")
            .application_uri("urn:chirpstack:opcua:gateway")
            .product_uri("urn:chirpstack:opcua:gateway")
            .create_sample_keypair(true)
            .certificate_path("own/cert.der")
            .private_key_path("private/private.pem")
            .trust_client_certs(true)
            .check_cert_time(true)
            .pki_dir("./pki")
            .hello_timeout(5)
            .host("localhost")//TODO: Use local ip address
            .port(4840)
            .locale_ids(vec!["en".to_string()])
            .discovery_urls(vec!["opc.tcp://localhost:4840/".to_string()])
            .default_endpoint("null".to_string())
            .add_user_token(
                "user1",
                ServerUserToken {
                    user: "user1".to_string(),
                    pass: Some("user1".to_string()),
                    x509: None,
                    thumbprint: None,
                    read_diagnostics: true
                }
            )
            .default_endpoint("null".to_string())// The name of this enpoint has to be registered with add_endpoint
            .add_endpoint(
                "null", // This is the index of the default endpoint
                ServerEndpoint{
                    path: "/".to_string(),
                    security_policy: "None".to_string(),
                    security_mode: "None".to_string(),
                    security_level: 0,
                    password_security_policy: None,
                    user_token_ids: BTreeSet::from([
                        "user1".to_string()
                    ])
                }
            )
            .add_endpoint(
                "basic256_sign",
                ServerEndpoint{
                    path: "/".to_string(),
                    security_policy: "Basic256".to_string(),
                    security_mode: "Sign".to_string(),
                    security_level: 3,
                    password_security_policy: None,
                    user_token_ids: BTreeSet::from([
                        "user1".to_string()
                    ])
                }
            )
            .add_endpoint(
                "basic256_sign_encrypt",
                ServerEndpoint{
                    path: "/".to_string(),
                    security_policy: "Basic256".to_string(),
                    security_mode: "SignAndEncrypt".to_string(),
                    security_level: 13,
                    password_security_policy: None,
                    user_token_ids: BTreeSet::from([
                        "user1".to_string()
                    ])
                }
            )

            .diagnostics_enabled(true)
            .with_node_manager(
                simple_node_manager(
                    NamespaceMetadata {
                        namespace_uri: "urn:DemoServer".to_owned(),
                        ..Default::default()
                    },
                    "demo",
                )
            );
        server_builder
}

fn create_limits() -> Limits {
    todo!()
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
    pub async fn run(mut self) -> Result<(), OpcGwError> {
        debug!("Running OPC UA server");
        let (server, handle) = Self::create_server_builder()
            .build()
            .map_err(|e| OpcGwError::OpcUaError(e.to_string()))?;
        debug!("Opc ua server is built");
        info!("OPC UA server started on opc.tcp:://localhost:4840/");
        server.run().await.map_err(|e| OpcGwError::OpcUaError(e.to_string()));
        info!("OPC UA server started on opc.tcp:://localhost:4840/");
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
        trace!("Populating address space");
        todo!();
    }
}



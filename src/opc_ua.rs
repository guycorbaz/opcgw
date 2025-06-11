// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//TODO: Remove for production
#![allow(unused)]

use crate::config::{AppConfig, ChirpstackDevice, OpcUaConfig};
use crate::storage::{MetricType, Storage};
use crate::utils::{OpcGwError, OPCUA_ADDRESS_SPACE};
use log::{debug, error, info, trace, warn};
use rand::prelude::*;
use std::collections::BTreeMap;
use tokio_util::sync::CancellationToken;
use tonic::transport::Endpoint;

use local_ip_address::local_ip;
use std::collections::BTreeSet;
use std::option::Option;
use std::path::PathBuf;
use std::sync::Arc;

// opcua modules
use opcua::crypto::SecurityPolicy;
use opcua::server::address_space::Variable;
use opcua::server::{
    diagnostics::NamespaceMetadata,
    node_manager::memory::{simple_node_manager, SimpleNodeManager},
    Limits, OperationalLimits, Server, ServerBuilder, ServerConfig, ServerEndpoint,
    ServerUserToken, SubscriptionCache, SubscriptionLimits,
};
use opcua::types::{MessageSecurityMode, NodeId};

/// Structure for storing OpcUa server parameters
pub struct OpcUa {
    /// Configuration for the OPC UA server
    config: AppConfig,
    /// Storage for the OPC UA server
    storage: Arc<std::sync::Mutex<Storage>>,
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
        trace!("Create new OPC UA server structure");

        OpcUa {
            config: config.clone(),
            storage,
        }
    }

    fn create_server(&mut self) -> Result<Server, OpcGwError> {
        debug!("Configure Server");

        //TODO: configure server from opcua configuration file
        debug!("Creating server builder");
        let server_builder = ServerBuilder::new()
            .application_name("Chirpstack OPC UA Gateway")
            .application_uri("urn:chirpstack:opcua:gateway")
            .product_uri("urn:chirpstack:opcua:gateway")
            .locale_ids(vec!["en".to_string()])
            .discovery_urls(vec!["opc.tcp://localhost:4840/".to_string()])
            .default_endpoint("null".to_string())
            .diagnostics_enabled(true)
            .with_node_manager(simple_node_manager(
                NamespaceMetadata {
                    namespace_uri: "urn:UpcUaGw".to_owned(),
                    ..Default::default()
                },
                "demo",
            ));

        let server_builder = self.configure_network(server_builder);
        let server_builder = self.configure_key(server_builder);
        let server_builder = self.configure_user_token(server_builder);
        let server_builder = self.configure_end_points(server_builder);

        debug!("Creating server");
        let (server, handle) = server_builder
            .build()
            .map_err(|e| OpcGwError::OpcUaError(e.to_string()))?;

        debug!("Creating node manager");
        let node_manager = handle
            .node_managers()
            .get_of_type::<SimpleNodeManager>()
            .unwrap();

        debug!("Creating namespace");
        let ns = handle.get_namespace_index("urn:UpcUaGw").unwrap();

        self.add_nodes(ns, node_manager);

        Ok(server)
    }

    fn configure_network(&self, mut server_builder: ServerBuilder) -> ServerBuilder {
        debug!("Configure network");
        server_builder
            .hello_timeout(5)
            .host("localhost") //TODO: Use local ip address
            .port(4840)
    }
    fn configure_key(&self, mut server_builder: ServerBuilder) -> ServerBuilder {
        debug!("Configure key and pki");
        server_builder
            .create_sample_keypair(true)
            .certificate_path("own/cert.der")
            .private_key_path("private/private.pem")
            .trust_client_certs(true)
            .check_cert_time(true)
            .pki_dir("./pki")
    }

    fn configure_user_token(&self, mut server_builder: ServerBuilder) -> ServerBuilder {
        debug!("Configure user token");
        server_builder.add_user_token(
            "user1",
            ServerUserToken {
                user: "user1".to_string(),
                pass: Some("user1".to_string()),
                x509: None,
                thumbprint: None,
                read_diagnostics: true,
            },
        )
    }

    fn configure_end_points(&self, mut server_builder: ServerBuilder) -> ServerBuilder {
        debug!("Configure end points");
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

    fn create_limits(&self) -> Limits {
        todo!()
    }

    pub async fn run(mut self) -> Result<(), OpcGwError> {
        debug!("Running OPC UA server");

        // Error management for server creation
        let server = match self.create_server() {
            Ok(server) => {
                debug!("OPC UA server built");
                server
            }
            Err(e) => {
                error!("OPC UA server error: {}", e);
                return Err(e);
            }
        };

        info!("OPC UA server started on opc.tcp:://localhost:4840/"); //TODO: make sure message display the url from parameters
        match server.run().await {
            Ok(_) => {
                info!("OPC UA server stopped");
                Ok(())
            }
            Err(e) => {
                error!("Error w hile running OPC UA server {}", e);
                Err(OpcGwError::OpcUaError(e.to_string()))
            }
        };
        Ok(())
    }

    pub fn add_nodes(&mut self, ns: u16, manager: Arc<SimpleNodeManager>) {
        trace!("Add nodes to OPC UA server");
        let address_space = manager.address_space();

        // For testing
        //TODO: load folders and variable from configuration
        let v1_node = NodeId::new(ns, "v1");
        let v2_node = NodeId::new(ns, "v2");

        // The address spae is guarded so obtain a lock to change it
        let mut address_space = address_space.write();
        
        // Adding one folder per LoraWan application
        for application in self.config.application_list.iter() {
            debug!("Application {}", application.application_name);
            let application_node = NodeId::new(ns, application.application_name.clone());
            address_space.add_folder(
                &application_node,
                &application.application_name,
                &application.application_name,
                &NodeId::objects_folder_id(),
            );
        }
        
        // Create a folder
        let sample_folder_id = NodeId::new(ns, "SampleFolder");
        address_space.add_folder(
            &sample_folder_id,
            "SampleFolder",
            "SampleFolder",
            &NodeId::objects_folder_id(),
        );

        // Variables
        let _ = address_space.add_variables(
            vec![
                Variable::new(&v1_node, "v1", "v1", 0_i32),
                Variable::new(&v2_node, "v2", "v2", false),
            ],
            &sample_folder_id,
        );
    }
}

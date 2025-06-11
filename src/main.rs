// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! ChirpStack to OPC UA Gateway
//!
//! This application provides a gateway service that bridges ChirpStack 4 LoRaWAN
//! Network Server with OPC UA clients. It polls device data from ChirpStack and
//! exposes it through an OPC UA server interface for industrial automation systems.
//!



mod chirpstack;
mod config;
mod opc_ua;
mod storage;
mod utils;


pub mod chirpstack_api {
    //tonic::include_proto!("chirpstack");
}

use crate::chirpstack::{ChirpstackPoller};
use crate::storage::Storage;
use clap::Parser;
use config::AppConfig;
use log::{error, info};
use opc_ua::OpcUa;
use std::sync::Mutex;
use std::{path::PathBuf, sync::Arc};
use utils::OPCGW_CONFIG_PATH;

/// Command-line arguments for the ChirpStack to OPC UA Gateway.
///
/// This structure defines the available command-line options for configuring
/// the gateway's behavior at startup.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Set custom configuration file path
    ///
    /// Specifies an alternative location for the configuration file.
    /// If not provided, the default configuration path will be used.
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Enable debug logging with increasing verbosity
    ///
    /// Use multiple times to increase debug level:
    /// - `-d`: Basic debug information
    /// - `-dd`: Verbose debug information
    /// - `-ddd`: Very verbose debug information
    #[arg(short, long, action = clap::ArgAction::Count)]
    debug: u8,
}

/// Main entry point for the ChirpStack to OPC UA Gateway application.
///
/// This function initializes and orchestrates the complete gateway system by:
/// 1. Parsing command-line arguments
/// 2. Configuring the logging system
/// 3. Loading application configuration
/// 4. Creating shared storage for inter-task communication
/// 5. Initializing ChirpStack poller and OPC UA server
/// 6. Running both services concurrently
///
/// # Architecture
///
/// The application uses a concurrent architecture with two main tasks:
/// - **ChirpStack Poller Task**: Periodically polls device data from ChirpStack
/// - **OPC UA Server Task**: Serves real-time device metrics to OPC UA clients
///
/// Both tasks share access to a thread-safe storage system that acts as a
/// data bridge between the ChirpStack API and OPC UA address space.
///
/// # Error Handling
///
/// The function handles initialization errors by panicking with descriptive
/// messages. Runtime errors from individual tasks are logged but do not
/// terminate the entire application unless both tasks fail.
///
/// # Returns
///
/// * `Ok(())` - Application completed successfully
/// * `Err(Box<dyn std::error::Error>)` - Critical initialization error occurred
///
/// # Panics
///
/// This function will panic if:
/// - Logger initialization fails
/// - Application configuration cannot be loaded
/// - ChirpStack poller creation fails
/// - Task spawning fails
///
/// # Examples
///
/// ```bash
/// # Run with default configuration
/// opcgw
///
/// # Run with custom config and debug logging
/// opcgw --config /etc/opcgw/config.toml --debug
/// ```
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse arguments
    let args = Args::parse();

    // Configure logger
    log4rs::init_file(
        format!("{}/log4rs.yaml", OPCGW_CONFIG_PATH),
        Default::default(),
    )
    .expect("Failed to initialize logger");
    info!("starting opcgw");

    // Create a new configuration and load its parameters
    let application_config = match AppConfig::new() {
        Ok(config) => Arc::new(config),
        Err(e) => panic!("Failed to load config: {}", e),
    };

    // Create shared storage for ChirpStack poller and OPC UA server threads
    let storage = Arc::new(Mutex::new(Storage::new(&application_config)));

    // Create chirpstack poller
    let mut chirpstack_poller =
        match ChirpstackPoller::new(&application_config, storage.clone()).await {
            Ok(poller) => poller,
            Err(e) => panic!("Failed to create chirpstack poller: {}", e),
        };

    // Create OPC UA server
    let opc_ua = OpcUa::new(&application_config, storage.clone());

    // Run chirpstack poller and OPC UA server in separate tasks
    let chirpstack_handle = tokio::spawn(async move {
        if let Err(e) = chirpstack_poller.run().await {
            error!("ChirpStack poller error: {:?}", e);
        }
    });

    // Run ChirpStack poller and OPC UA server in separate tasks
    let opcua_handle = tokio::spawn(async move {
        if let Err(e) = opc_ua.run().await {
            error!("OPC UA server error: {:?}", e);
        }
    });

    // Wait for all tasks to complete
    tokio::try_join!(chirpstack_handle, opcua_handle).expect("Failed to run tasks");

    info!("Stopping opcgw");
    Ok(())
}

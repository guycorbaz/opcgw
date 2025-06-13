// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! ChirpStack to OPC UA Gateway
//!
//! This application provides a gateway service that bridges ChirpStack 4 LoRaWAN
//! Network Server with OPC UA clients. It polls device data from ChirpStack and
//! exposes it through an OPC UA server interface for industrial automation systems.
//!
//! # Architecture
//!
//! The gateway consists of two main components running concurrently:
//! - **ChirpStack Poller**: Polls device data from ChirpStack LoRaWAN Network Server
//! - **OPC UA Server**: Exposes the collected data through an OPC UA interface
//!
//! # Configuration
//!
//! The application uses a configuration file and supports command-line arguments
//! for customization. Logging is configured via log4rs.

mod chirpstack;
mod config;
mod opc_ua;
mod storage;
mod utils;

/// ChirpStack API protobuf definitions
///
/// This module would contain the generated protobuf code for ChirpStack API.
/// Currently commented out - uncomment when protobuf generation is set up.
pub mod chirpstack_api {
    //tonic::include_proto!("chirpstack");
}

use crate::chirpstack::ChirpstackPoller;
use crate::storage::Storage;
use clap::Parser;
use config::AppConfig;
use log::{error, info};
use opc_ua::OpcUa;
use std::sync::Mutex;
use std::{path::PathBuf, sync::Arc};
use utils::OPCGW_CONFIG_PATH;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Path to the configuration file
    ///
    /// If not specified, the application will use the default configuration
    /// file location defined by the application.
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Debug verbosity level
    ///
    /// Use multiple times to increase verbosity (e.g., -d, -dd, -ddd).
    /// This controls the logging level for debugging purposes.
    #[arg(short, long, action = clap::ArgAction::Count)]
    debug: u8,
}

/// Main entry point for the ChirpStack to OPC UA Gateway
///
/// This function:
/// 1. Parses command line arguments
/// 2. Initializes logging configuration
/// 3. Loads application configuration
/// 4. Creates shared storage for data exchange
/// 5. Starts ChirpStack poller and OPC UA server in separate tasks
/// 6. Waits for both tasks to complete
///
/// # Returns
///
/// Returns `Ok(())` on successful completion, or an error if any component fails to initialize.
///
/// # Panics
///
/// This function will panic if:
/// - The configuration cannot be loaded
/// - The ChirpStack poller cannot be created
/// - The logger cannot be initialized
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse arguments
    let _args = Args::parse();

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

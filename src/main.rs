// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Chirpstack to opc ua gateway
//!
//! Provide a Chirpstack 4 to opc ua server gateway.
//!

mod chirpstack;
mod config;
mod opc_ua;
mod storage;
mod utils;

// Inclure le module généré
pub mod chirpstack_api {
    //tonic::include_proto!("chirpstack");
}
use crate::chirpstack::{ApplicationDetail, ChirpstackPoller, DeviceDetails, DeviceListDetail};
use clap::Parser;
use config::Config;
use log::{debug, error, info, trace, warn};
use opc_ua::OpcUa;
use opcua::server::server::Server;
use opcua::sync::RwLock;
use std::time::Duration;
use std::{path::PathBuf, sync::Arc, thread};
use storage::Storage;
use tokio::runtime::{Builder, Runtime};
use tokio::time;

// Manage arguments
// Version (-V) is automatically derives from Cargo.toml
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Set custom config path
    #[arg(short, long, value_name = "FILE")]
    config: Option<PathBuf>,

    /// Turn debugging information on
    #[arg(short, long, action = clap::ArgAction::Count)]
    debug: u8,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse arguments
    let args = Args::parse(); //TODO: add possibility to pass config file as argument

    // Configure logger
    log4rs::init_file("log4rs.yaml", Default::default()).expect("Failed to initialize logger");
    info!("starting");

    // Create a new configuration and load parameters
    let application_config = match Config::new() {
        Ok(config) => Arc::new(config),
        Err(e) => panic!("Failed to load config: {}", e),
    };

    // Create chirpstack poller
    trace!("Create chirpstack poller");
    let mut chirpstack_poller = match ChirpstackPoller::new(&application_config.chirpstack).await {
        Ok(poller) => poller,
        Err(e) => panic!("Failed to create chirpstack poller: {}", e),
    };

    // Create OPC UA server
    trace!("Create OPC UA server");
    //let opc_ua = OpcUa::new(&application_config.opcua);

    // Run chirpstack poller and OPC UA server in separate tasks
    let chirpstack_handle = tokio::spawn(async move {
        if let Err(e) = chirpstack_poller.run().await {
            error!("ChirpStack poller error: {:?}", e);
        }
    });

    // Create OPC UA server
    let opc_ua = OpcUa::new(&application_config.opcua);

    // Run OPC UA server and periodic metrics reading in separate tasks
    let opcua_handle = tokio::spawn(async move {
        if let Err(e) = opc_ua.run().await {
            error!("OPC UA server error: {:?}", e);
        }
    });

    let read_metrics_task = tokio::spawn(async move {
        loop {
            if let Err(e) = opc_ua.read_device_metrics().await {
                error!("Error reading device metrics: {:?}", e);
            }
            time::sleep(Duration::from_secs(10)).await;
        }
    });

    // Wait for all tasks to complete
    tokio::try_join!(chirpstack_handle, opcua_handle, read_metrics_task).expect("Failed to run tasks");

    info!("Stopping");
    Ok(())
}

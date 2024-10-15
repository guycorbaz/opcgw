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
use std::{thread, path::PathBuf, sync::Arc};
use std::time::Duration;
use storage::Storage;
use tokio::runtime::{Builder, Runtime};

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


//#[tokio::main]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse arguments
    let args = Args::parse();
    // Configure logger
    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("starting");

    // Create a new configuration
    let application_config = Config::new()?;

    // Create chirpstack poller
    trace!("Create chirpstack poller");
    let chirpstack_poller = ChirpstackPoller::new(&application_config.chirpstack)
        .expect("Failed to create chirpstack client");

    // Create OPC UA server
    trace!("Create OPC UA server");
    let opc_ua = OpcUa::new(&application_config.opcua);

    // Run chirpstack poller and OPC UA server in separate tasks
    let chirpstack_handle = tokio::spawn(async move {
        chirpstack_poller.run().await;
    });

    let opcua_handle = tokio::spawn(async move {
        opc_ua.run().await;
    });

    // Wait for both tasks to complete
    tokio::try_join!(chirpstack_handle, opcua_handle)?;

    info!("Stopping");
    Ok(())
}

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
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Parse arguments
    let args = Args::parse();
    // Configure logger
    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("starting");

    // Create a new configuration
    let application_config = Config::new()?;

    trace!("Create tokio runtime");
    let opc_runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build() {
        Ok(runtime) => runtime,
        Err(e) =>panic!("Cannot create Tokio Runtime: {:?}", e),
    };
    let chirpstack_runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build() {
        Ok(runtime) => runtime,
        Err(e) =>panic!("Cannot create Tokio Runtime: {:?}", e),
    };

    trace!("Create chirpstack poller"); //TODO: Comment marche ce bousin avec tokio ?
    let chirpstack_poller =
        ChirpstackPoller::new(&application_config.chirpstack)
            .expect("Failed to create chirpstack client");

    trace!("Run chirpstach poller");
    chirpstack_poller.run_on_runtime(chirpstack_runtime);




    trace!("Create opc ua server");
    let opc_ua = OpcUa::new(&application_config.opcua);

    trace!("Create opcua server handler");
    // Create a non blocking server running on tokio runtime define above
    // It needs to be joined //TODO: add join for this server
    let opcua_server_handler = Server::run_server_on_runtime(
        opc_runtime,
        Server::new_server_task(Arc::new(RwLock::new(opc_ua.server))),
        false,
    ).unwrap();



    info!("Stopping");
    Ok(())
}

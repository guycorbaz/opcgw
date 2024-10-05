// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Chirpstack to opc ua gateway
//!
//! Provide a Chirpstack 4 to opc ua server gateway.
//!

mod chirpstack;
mod config;

mod chirpstack_test;
mod opc_ua;
mod storage;
mod utils;

// Inclure le module généré
pub mod chirpstack_api {
    //tonic::include_proto!("chirpstack");
}
use crate::chirpstack::{ApplicationDetail, ChirpstackClient, DeviceDetails, DeviceListDetail};
use crate::chirpstack_test::test_chirpstack;
use opcua::sync::RwLock;
use config::Config;
use log::{debug, error, info, trace, warn};
use opcua::server::server::Server;
use opc_ua::OpcUa;
use storage::Storage;
use tokio::runtime::{Builder, Runtime};
//use tokio::sync::RwLock;
use std::{path::PathBuf, sync::Arc};


/// Start  opc_ua_chirpstack_gateway
//#[tokio::main]
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure logger
    log4rs::init_file("log4rs.yaml", Default::default()).unwrap();
    info!("starting");

    trace!("Create application configuration:");
    let application_config = Config::new()?;

    trace!("Create opc ua server");
    let opc_ua = OpcUa::new(&application_config.opcua);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();

    trace!("Create storage");
    //let mut storage:Storage = Storage::new(&application_config).await;
    //storage.load_applications();
    //storage.load_devices().await;
    //storage.list_devices();

    trace!("Start opc ua server");
    Server::run_server_on_runtime(
        runtime,
        Server::new_server_task(Arc::new(RwLock::new(opc_ua.server))),
        true,
    );




    //trace!("Creating opc ua server");
    //let opc_ua = OpcUa::new(config.opcua);
    //opc_ua.add_folder();


    // Add opc ua structure



    //trace!("Run OPC UA server");
    //Server::run_server_on_runtime(
    //    runtime,
    //    Server::new_server_task(Arc::new(RwLock::new(opc_ua.server))),
    //    true
    //);
    info!("Stopping");
        Ok(())

}

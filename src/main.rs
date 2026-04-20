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
use crate::storage::{Storage, ConnectionPool};
use clap::Parser;
use config::AppConfig;
use tracing::{error, info};
use tracing_appender::non_blocking;
use tracing_subscriber::{filter, fmt, layer::SubscriberExt, util::SubscriberInitExt, Layer};
use tokio_util::sync::CancellationToken;
use opc_ua::OpcUa;
use std::sync::Mutex;
use std::{path::PathBuf, sync::Arc};
use std::time::Duration;

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

    // Configure tracing subscriber with per-module file appenders (daily rotation)
    let (chirpstack_writer, _guard1) =
        non_blocking(tracing_appender::rolling::daily("log", "chirpstack.log"));
    let (opcua_writer, _guard2) =
        non_blocking(tracing_appender::rolling::daily("log", "opc_ua.log"));
    let (root_writer, _guard3) =
        non_blocking(tracing_appender::rolling::daily("log", "opc_ua_gw.log"));
    let (storage_writer, _guard4) =
        non_blocking(tracing_appender::rolling::daily("log", "storage.log"));
    let (config_writer, _guard5) =
        non_blocking(tracing_appender::rolling::daily("log", "config.log"));

    tracing_subscriber::registry()
        // Console layer: all modules at debug
        .with(
            fmt::layer()
                .with_writer(std::io::stdout)
                .with_filter(filter::LevelFilter::DEBUG),
        )
        // Root file layer: all modules at debug
        .with(
            fmt::layer()
                .with_writer(root_writer)
                .with_filter(filter::LevelFilter::DEBUG),
        )
        // Per-module file layers with per-layer target filters
        .with(
            fmt::layer()
                .with_writer(chirpstack_writer)
                .with_filter(
                    filter::Targets::new()
                        .with_target("opcgw::chirpstack", tracing::Level::TRACE),
                ),
        )
        .with(
            fmt::layer()
                .with_writer(opcua_writer)
                .with_filter(
                    filter::Targets::new()
                        .with_target("opcgw::opc_ua", tracing::Level::TRACE)
                        .with_target("async_opcua", tracing::Level::DEBUG),
                ),
        )
        .with(
            fmt::layer()
                .with_writer(storage_writer)
                .with_filter(
                    filter::Targets::new()
                        .with_target("opcgw::storage", tracing::Level::TRACE),
                ),
        )
        .with(
            fmt::layer()
                .with_writer(config_writer)
                .with_filter(
                    filter::Targets::new()
                        .with_target("opcgw::config", tracing::Level::TRACE),
                ),
        )
        .init();

    info!("starting opcgw");

    // Create a new configuration and load its parameters
    let application_config = match AppConfig::new() {
        Ok(config) => Arc::new(config),
        Err(e) => {
            error!(error = %e, "Failed to load configuration");
            return Err(e.into());
        }
    };

    // Log startup confirmation with key parameters
    let total_devices: usize = application_config
        .application_list
        .iter()
        .map(|app| app.device_list.len())
        .sum();
    let opc_ua_endpoint = format!(
        "{}:{}",
        application_config.opcua.host_ip_address.as_deref().unwrap_or("0.0.0.0"),
        application_config.opcua.host_port.unwrap_or(4840)
    );

    info!(
        poll_interval_seconds = application_config.chirpstack.polling_frequency,
        application_count = application_config.application_list.len(),
        device_count = total_devices,
        opc_ua_endpoint = %opc_ua_endpoint,
        chirpstack_server = %application_config.chirpstack.server_address,
        "Gateway started successfully"
    );

    // Create cancellation token for graceful shutdown
    let cancel_token = CancellationToken::new();

    // Create connection pool for per-task SQLite access (Story 2-2x: per-task connections)
    // Pool shared via Arc; each task (poller, OPC UA) gets own connection from pool via Arc::clone()
    // SQLite WAL mode: true concurrent readers + single writer (no Rust Mutex bottleneck)
    let pool = match ConnectionPool::new("data/opcgw.db", 3) {
        Ok(pool_inner) => Arc::new(pool_inner),
        Err(e) => {
            error!(error = %e, "Failed to create connection pool");
            return Err(e.into());
        }
    };

    // Create shared storage for ChirpStack poller and OPC UA server threads
    let storage = Arc::new(Mutex::new(Storage::new(&application_config)));

    // Create chirpstack poller
    let mut chirpstack_poller =
        match ChirpstackPoller::new(&application_config, storage.clone(), pool.clone(), cancel_token.clone())
            .await
        {
            Ok(poller) => poller,
            Err(e) => {
                error!(error = %e, "Failed to create chirpstack poller");
                return Err(e.into());
            }
        };

    // Create OPC UA server
    let opc_ua = OpcUa::new(&application_config, storage.clone(), pool.clone(), cancel_token.clone());

    // Run chirpstack poller and OPC UA server in separate tasks
    let chirpstack_handle = tokio::spawn(async move {
        if let Err(e) = chirpstack_poller.run().await {
            error!(error = ?e, "ChirpStack poller error");
        }
    });

    let opcua_handle = tokio::spawn(async move {
        if let Err(e) = opc_ua.run().await {
            error!(error = ?e, "OPC UA server error");
        }
    });

    // Wait for shutdown signal (SIGINT or SIGTERM)
    let mut sigterm =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;

    tokio::select! {
        _ = tokio::signal::ctrl_c() => {
            info!("Received SIGINT, shutting down");
        }
        _ = sigterm.recv() => {
            info!("Received SIGTERM, shutting down");
        }
    }

    // Cancel the token to signal all tasks to stop
    cancel_token.cancel();

    // Wait for tasks to finish gracefully (with timeout)
    match tokio::time::timeout(
        std::time::Duration::from_secs(10),
        async { tokio::try_join!(chirpstack_handle, opcua_handle) },
    )
    .await
    {
        Ok(Ok(_)) => info!("All tasks shut down cleanly"),
        Ok(Err(e)) => error!(error = %e, "Task error during shutdown"),
        Err(_) => error!("Shutdown timed out after 10 seconds, forcing exit"),
    }

    // Close connection pool (ensure all connections flushed/closed)
    if let Err(e) = pool.close() {
        error!(error = %e, "Error closing connection pool");
    }

    info!("Stopping opcgw");
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio_util::sync::CancellationToken;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn test_cancellation_token_propagation() {
        let token = CancellationToken::new();
        let task_completed = Arc::new(AtomicBool::new(false));
        let task_completed_clone = task_completed.clone();
        let token_clone = token.clone();

        // Spawn a task that loops until cancelled
        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token_clone.cancelled() => {
                        task_completed_clone.store(true, Ordering::SeqCst);
                        return;
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {}
                }
            }
        });

        // Give the task time to start
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // Cancel the token
        token.cancel();

        // Wait for the task to complete
        let _ = handle.await;

        // Verify the task saw the cancellation and completed
        assert!(task_completed.load(Ordering::SeqCst), "Task should have completed after cancellation");
    }
}

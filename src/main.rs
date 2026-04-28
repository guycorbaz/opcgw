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
//! for customization. Logging is built on top of `tracing` + `tracing-subscriber`
//! with per-module daily-rolling file appenders and a stderr console layer.
//! The output directory is resolved from (in order): the `OPCGW_LOG_DIR` env var,
//! `[logging].dir` in `config.toml`, then the default `./log`.

mod chirpstack;
mod command_validation;
mod config;
mod opc_ua;
mod opc_ua_auth;
mod security;
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
use crate::storage::{Storage, ConnectionPool, StorageBackend, MetricValueInternal};
use clap::Parser;
use config::{AppConfig, LoggingConfig};
use figment::{providers::{Env, Format, Toml}, Figment};
use tracing::{debug, error, info, trace, warn};
use tracing_appender::non_blocking;
use tracing_subscriber::fmt::time::ChronoUtc;
use tracing_subscriber::{filter, fmt, layer::SubscriberExt, util::SubscriberInitExt, Layer};
use tokio_util::sync::CancellationToken;
use opc_ua::OpcUa;
use std::sync::{Mutex, Barrier};
use std::{path::PathBuf, sync::Arc};

/// Logging-only peek of the TOML config used during the bootstrap phase
/// (before `AppConfig::new()` is called). Lets us resolve `[logging].dir`
/// without paying for full config validation, so config-load errors can
/// still reach the file appender via `error!`.
#[derive(serde::Deserialize)]
struct LoggingPeek {
    logging: Option<LoggingConfig>,
}

/// Best-effort one-shot peek of `[logging]` from the TOML file plus the
/// figment-style env overlay (`OPCGW_LOGGING__DIR`, `OPCGW_LOGGING__LEVEL`).
/// A parse error returns `None`; the full `AppConfig::new()` call later
/// will surface the underlying error via tracing once the subscriber
/// is up. Shared between `resolve_log_dir` and `resolve_log_level`
/// (Story 6-2) so the file is only read once during bootstrap.
///
/// Merging the `OPCGW_` env layer here mirrors `AppConfig::new()` and
/// ensures the long-form figment env vars also influence the bootstrap
/// resolvers (the short forms `OPCGW_LOG_DIR` / `OPCGW_LOG_LEVEL` are
/// still consulted by the resolvers themselves and take precedence).
fn peek_logging_config(config_path: &str) -> Option<LoggingConfig> {
    Figment::new()
        .merge(Toml::file(config_path))
        .merge(Env::prefixed("OPCGW_").split("__").global())
        .extract::<LoggingPeek>()
        .ok()
        .and_then(|p| p.logging)
}

/// Resolve the log directory in precedence order: `OPCGW_LOG_DIR` env >
/// `[logging].dir` (TOML or `OPCGW_LOGGING__DIR` env, merged by the peek) >
/// `./log`. Returns the resolved directory and a short tag identifying the
/// source (`"env"`, `"config"`, `"default"`) so callers can suppress the
/// post-init divergence warning when an override was the intended winner.
///
/// Empty / whitespace-only env values are treated as unset (Story 6-1
/// review patch).
fn resolve_log_dir(peeked: Option<&LoggingConfig>) -> (String, &'static str) {
    let from_env = std::env::var("OPCGW_LOG_DIR")
        .ok()
        .filter(|s| !s.trim().is_empty());
    if let Some(dir) = from_env {
        return (dir, "env");
    }
    let from_toml = peeked
        .and_then(|l| l.dir.clone())
        .filter(|s| !s.trim().is_empty());
    if let Some(dir) = from_toml {
        return (dir, "config");
    }
    ("./log".to_string(), "default")
}

/// Story 6-2, AC#1: parse a log-level string into a `LevelFilter`.
///
/// Accepts the five canonical values case-insensitively: `trace`, `debug`,
/// `info`, `warn`, `error`. Any other input returns `Err(original_input)`
/// — callers should fall back to `LevelFilter::INFO` and surface a
/// stderr warning (tracing isn't initialised yet at that point).
fn parse_log_level(input: &str) -> Result<filter::LevelFilter, String> {
    match input.trim().to_lowercase().as_str() {
        "trace" => Ok(filter::LevelFilter::TRACE),
        "debug" => Ok(filter::LevelFilter::DEBUG),
        "info" => Ok(filter::LevelFilter::INFO),
        "warn" => Ok(filter::LevelFilter::WARN),
        "error" => Ok(filter::LevelFilter::ERROR),
        _ => Err(input.to_string()),
    }
}

/// Story 6-2, AC#1 / AC#4: resolve the global log level using the
/// precedence chain `-d` CLI count > `OPCGW_LOG_LEVEL` env >
/// `[logging].level` (peeked from TOML or the figment env overlay) >
/// `LevelFilter::INFO`. Returns the resolved level and a short tag
/// identifying the source (`"cli"`, `"env"`, `"config"`, `"default"`)
/// so the post-init log line can surface where it came from.
///
/// CLI mapping: `cli_debug == 0` means no override (fall through to env);
/// `1` → DEBUG; `2+` → TRACE. The CLI flag only escalates verbosity, never
/// suppresses it.
///
/// Invalid values at the env or config layer fall through with a single
/// stderr warning — startup never aborts. Empty / whitespace-only values
/// at either layer are treated as unset (matches the `OPCGW_LOG_DIR`
/// empty-string handling). The level filter, once installed in the
/// subscriber, is checked at runtime by the `tracing` macros — a single
/// branch per call site, well below profiler resolution.
fn resolve_log_level(
    cli_debug: u8,
    peeked: Option<&LoggingConfig>,
) -> (filter::LevelFilter, &'static str) {
    match cli_debug {
        0 => {}
        1 => return (filter::LevelFilter::DEBUG, "cli"),
        _ => return (filter::LevelFilter::TRACE, "cli"),
    }
    if let Ok(s) = std::env::var("OPCGW_LOG_LEVEL") {
        if !s.trim().is_empty() {
            match parse_log_level(&s) {
                Ok(lf) => return (lf, "env"),
                Err(_) => {
                    eprintln!(
                        "Warning: Invalid OPCGW_LOG_LEVEL='{}' (valid: trace, debug, info, warn, error). Using config or default.",
                        s
                    );
                }
            }
        }
    }
    if let Some(level_str) = peeked.and_then(|l| l.level.as_deref()) {
        if !level_str.trim().is_empty() {
            match parse_log_level(level_str) {
                Ok(lf) => return (lf, "config"),
                Err(_) => {
                    eprintln!(
                        "Warning: Invalid [logging].level='{}' in config.toml (valid: trace, debug, info, warn, error). Falling back to default.",
                        level_str
                    );
                }
            }
        }
    }
    (filter::LevelFilter::INFO, "default")
}

/// Ensure the log directory exists and is writable. On failure, falls back
/// to `./log` and writes a diagnostic to stderr (tracing isn't up yet).
/// Returns the final directory that callers should use for appenders.
fn prepare_log_dir(requested: String) -> String {
    if let Err(e) = std::fs::create_dir_all(&requested) {
        eprintln!(
            "opcgw: cannot create log directory '{}' ({}); falling back to './log'",
            requested, e
        );
        let _ = std::fs::create_dir_all("./log");
        return "./log".to_string();
    }
    // Probe writability so we fail fast instead of silently dropping logs
    // through tracing-appender's non-blocking writer thread.
    let probe = std::path::Path::new(&requested).join(".opcgw-write-probe");
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            requested
        }
        Err(e) => {
            eprintln!(
                "opcgw: log directory '{}' is not writable ({}); falling back to './log'",
                requested, e
            );
            let _ = std::fs::create_dir_all("./log");
            "./log".to_string()
        }
    }
}

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
    let args = Args::parse();

    // Story 6-1, AC#1 (review patch D1): two-phase init.
    //   Phase 1 — peek `[logging].dir` from TOML *without* full validation,
    //             resolve `log_dir` (OPCGW_LOG_DIR env > peek > default), and
    //             bring up tracing so any subsequent error has a file appender
    //             to land in.
    //   Phase 2 — call `AppConfig::new()`; failures are logged via `error!`,
    //             which reaches both stderr and the per-module appenders.
    //
    // Config path precedence: CLI `-c FILE` > `CONFIG_PATH` env > default.
    // The chosen path drives both the bootstrap peek and `AppConfig::new()`.
    let config_path = args
        .config
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .or_else(|| std::env::var("CONFIG_PATH").ok())
        .unwrap_or_else(|| format!("{}/config.toml", crate::utils::OPCGW_CONFIG_PATH));
    // Single TOML+env peek shared by the dir + level resolvers (Story 6-2).
    let peeked = peek_logging_config(&config_path);
    let (log_dir_requested, log_dir_source) = resolve_log_dir(peeked.as_ref());
    let log_dir = prepare_log_dir(log_dir_requested);
    // Story 6-2, AC#1 / AC#4: resolve global log level
    // (CLI `-d` > OPCGW_LOG_LEVEL env > [logging].level > default INFO).
    let (log_level, log_level_source) = resolve_log_level(args.debug, peeked.as_ref());

    let init_start = std::time::Instant::now();

    // Configure tracing subscriber with per-module file appenders (daily rotation)
    let (chirpstack_writer, _guard1) =
        non_blocking(tracing_appender::rolling::daily(&log_dir, "chirpstack.log"));
    let (opcua_writer, _guard2) =
        non_blocking(tracing_appender::rolling::daily(&log_dir, "opc_ua.log"));
    let (root_writer, _guard3) =
        non_blocking(tracing_appender::rolling::daily(&log_dir, "opc_ua_gw.log"));
    let (storage_writer, _guard4) =
        non_blocking(tracing_appender::rolling::daily(&log_dir, "storage.log"));
    let (config_writer, _guard5) =
        non_blocking(tracing_appender::rolling::daily(&log_dir, "config.log"));

    // Story 6-3, AC#2: microsecond-precision UTC timestamps so concurrent
    // events (e.g. opc_ua_read vs. batch_write) get distinct timestamps and
    // chronological ordering is reconstructable from `grep`. Same format on
    // every layer (console + root + per-module files) so cross-file
    // correlation works.
    let micro_ts = || ChronoUtc::new("%Y-%m-%dT%H:%M:%S%.6fZ".to_string());

    tracing_subscriber::registry()
        // Console layer: stderr so container log drivers capture it.
        // Story 6-2: filter level resolved via OPCGW_LOG_LEVEL > [logging].level > INFO.
        // Per-module Targets filters below remain independent (AC#2).
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_timer(micro_ts())
                .with_filter(log_level),
        )
        // Root file layer: same global level as the console.
        .with(
            fmt::layer()
                .with_writer(root_writer)
                .with_timer(micro_ts())
                .with_filter(log_level),
        )
        // Per-module file layers with per-layer target filters
        .with(
            fmt::layer()
                .with_writer(chirpstack_writer)
                .with_timer(micro_ts())
                .with_filter(
                    filter::Targets::new()
                        .with_target("opcgw::chirpstack", tracing::Level::TRACE),
                ),
        )
        .with(
            fmt::layer()
                .with_writer(opcua_writer)
                .with_timer(micro_ts())
                .with_filter(
                    filter::Targets::new()
                        .with_target("opcgw::opc_ua", tracing::Level::TRACE)
                        .with_target("async_opcua", tracing::Level::DEBUG),
                ),
        )
        .with(
            fmt::layer()
                .with_writer(storage_writer)
                .with_timer(micro_ts())
                .with_filter(
                    filter::Targets::new()
                        .with_target("opcgw::storage", tracing::Level::TRACE),
                ),
        )
        .with(
            fmt::layer()
                .with_writer(config_writer)
                .with_timer(micro_ts())
                .with_filter(
                    filter::Targets::new()
                        .with_target("opcgw::config", tracing::Level::TRACE),
                ),
        )
        .init();

    let init_ms = init_start.elapsed().as_millis();
    info!(log_dir = %log_dir, tracing_init_ms = init_ms, "tracing subscriber initialised");
    // Story 6-2, AC#1: surface which level the subscriber actually applied,
    // and where it came from. If OPCGW_LOG_LEVEL=error, this line itself is
    // suppressed — that's the contract.
    info!(
        operation = "logging_init",
        level = %log_level,
        source = log_level_source,
        "Resolved global log level"
    );
    info!("starting opcgw");

    // Phase 2: now that tracing is up, load the full config from the same
    // path the bootstrap peek used. Any failure here reaches the file
    // appenders via `error!` (review patch D1).
    let application_config = match AppConfig::from_path(&config_path) {
        Ok(config) => Arc::new(config),
        Err(e) => {
            error!(error = %e, "Failed to load configuration");
            return Err(e.into());
        }
    };

    // Story 7-2 (AC#6): warn — but do not block — when a release build
    // ships with `create_sample_keypair = true`. Operators legitimately
    // running release-mode dev builds with auto-generated keypairs should
    // be allowed; the warning is the operational pressure that nudges
    // production deployments toward manually-provisioned certs.
    if let Some(message) = security::warn_if_create_sample_keypair_in_release(
        application_config.opcua.create_sample_keypair,
        !cfg!(debug_assertions),
    ) {
        warn!(
            event = "create_sample_keypair_in_release",
            mitigation = "Set create_sample_keypair = false and provision keypair manually for production deployments. See docs/security.md.",
            "{}",
            message
        );
    }

    // If bootstrap fell back to the default *and* the fully-loaded config
    // names a different `[logging].dir`, the operator needs to restart with
    // the proper directory accessible. We only warn in that genuine
    // mismatch case — overrides via env or `[logging].dir` are the
    // intended winners and shouldn't trigger a "restart to apply" warning.
    if log_dir_source == "default" {
        if let Some(cfg_dir) = application_config
            .logging
            .as_ref()
            .and_then(|l| l.dir.as_deref())
        {
            if cfg_dir != log_dir {
                warn!(
                    bootstrap_log_dir = %log_dir,
                    config_log_dir = %cfg_dir,
                    "config [logging].dir differs from bootstrap log_dir (bootstrap used default); restart to apply"
                );
            }
        }
    }

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

    // Create barrier for synchronizing restore completion (Task 11)
    let restore_barrier = Arc::new(Barrier::new(2));

    // Restore metrics from database on startup (Story 2-4a)
    let sqlite_backend = crate::storage::SqliteBackend::with_pool(pool.clone())
        .map_err(|e| {
            error!(error = %e, "Failed to create SQLite backend for metric restore");
            e
        })?;

    match sqlite_backend.load_all_metrics() {
        Ok(metrics) => {
            let metric_count = metrics.len();
            let mut storage_guard = storage.lock()
                .map_err(|e| {
                    error!(error = %e, "Failed to acquire storage lock for metric restore");
                    crate::utils::OpcGwError::Storage(format!("Storage lock failed: {}", e))
                })?;

            let mut restored_count = 0;
            let mut orphan_count = 0;
            let mut orphan_metrics = Vec::new();

            for metric in metrics {
                let metric_value_internal = MetricValueInternal {
                    device_id: metric.device_id.clone(),
                    metric_name: metric.metric_name.clone(),
                    value: metric.value,
                    timestamp: metric.timestamp,
                    data_type: metric.data_type,
                };

                match storage_guard.set_metric_value(&metric.device_id, &metric.metric_name, metric_value_internal) {
                    Ok(()) => {
                        restored_count += 1;
                        trace!(
                            device_id = %metric.device_id,
                            metric_name = %metric.metric_name,
                            "Restored metric from database"
                        );
                    }
                    Err(e) => {
                        orphan_count += 1;
                        // Collecting orphan device_ids for logging. Full orphan cleanup/pruning deferred to Epic 2-5.
                        orphan_metrics.push(metric.device_id.clone());
                        debug!(
                            error = %e,
                            device_id = %metric.device_id,
                            metric_name = %metric.metric_name,
                            reason = "device not in configuration",
                            "Skipped orphan metric during restore"
                        );
                    }
                }
            }

            info!(
                restored_count = restored_count,
                orphan_count = orphan_count,
                total_attempted = metric_count,
                "Metric restore completed"
            );

            if orphan_count > 0 {
                if orphan_count <= 10 {
                    // Log all device IDs when count is manageable
                    for device_id in &orphan_metrics {
                        debug!(device_id = %device_id, "Orphan metric detected (device not in config)");
                    }
                } else {
                    // Log sample of first 10 devices + aggregate count for large orphan sets
                    let sample_size = std::cmp::min(10, orphan_metrics.len());
                    for device_id in &orphan_metrics[..sample_size] {
                        debug!(device_id = %device_id, "Orphan metric detected (device not in config)");
                    }
                    let remaining = orphan_count - sample_size as i32;
                    debug!(
                        sample_count = sample_size,
                        remaining_count = remaining,
                        total_orphans = orphan_count,
                        "Orphan metrics (showing sample of {} + {} more)", sample_size, remaining
                    );
                }
            }
        }
        Err(e) => {
            error!(error = %e, "Failed to restore metrics from database, continuing with empty metrics (graceful degradation)");
        }
    }

    // Create SQLite backend for ChirpStack poller (Story 4-1: independent backend per task)
    let poller_backend: Arc<dyn crate::storage::StorageBackend> = match crate::storage::SqliteBackend::with_pool(pool.clone()) {
        Ok(backend) => Arc::new(backend),
        Err(e) => {
            error!(error = %e, "Failed to create SQLite backend for ChirpStack poller");
            return Err(e.into());
        }
    };

    // Create chirpstack poller with restore barrier
    let mut chirpstack_poller =
        match ChirpstackPoller::new(
            &application_config,
            poller_backend,
            cancel_token.clone(),
            Arc::clone(&restore_barrier),
        )
            .await
        {
            Ok(poller) => poller,
            Err(e) => {
                error!(error = %e, "Failed to create chirpstack poller");
                return Err(e.into());
            }
        };

    // Create SQLite backend for OPC UA server (Story 5-1: independent backend per task)
    let opcua_backend: Arc<dyn crate::storage::StorageBackend> = match crate::storage::SqliteBackend::with_pool(pool.clone()) {
        Ok(backend) => Arc::new(backend),
        Err(e) => {
            error!(error = %e, "Failed to create SQLite backend for OPC UA server");
            return Err(e.into());
        }
    };

    // Create OPC UA server
    let opc_ua = OpcUa::new(&application_config, opcua_backend, cancel_token.clone());

    // Signal poller that restore is complete (Task 11)
    info!("Metric restore phase complete; signaling poller to start");
    restore_barrier.wait();

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

    // Spawn command status poller task (Task 3-3 Task 5)
    let pool_poller = pool.clone();
    let cancel_poller = cancel_token.clone();
    let config_poller = application_config.clone();
    let poller_handle = tokio::spawn(async move {
        let backend = Arc::new(storage::SqliteBackend::with_pool(pool_poller)
            .expect("Failed to create SqliteBackend for poller"));
        match chirpstack::CommandStatusPoller::new(&config_poller, backend, cancel_poller) {
            Ok(mut cmd_poller) => {
                if let Err(e) = cmd_poller.run().await {
                    error!(error = ?e, "CommandStatusPoller error");
                }
            }
            Err(e) => error!(error = ?e, "Failed to create CommandStatusPoller"),
        }
    });

    // Spawn command timeout handler task (Task 3-3 Task 5)
    let pool_timeout = pool.clone();
    let cancel_timeout = cancel_token.clone();
    let config_timeout = application_config.clone();
    let timeout_handle = tokio::spawn(async move {
        let backend = Arc::new(storage::SqliteBackend::with_pool(pool_timeout)
            .expect("Failed to create SqliteBackend for timeout handler"));
        match chirpstack::CommandTimeoutHandler::new(&config_timeout, backend, cancel_timeout) {
            Ok(mut cmd_timeout) => {
                if let Err(e) = cmd_timeout.run().await {
                    error!(error = ?e, "CommandTimeoutHandler error");
                }
            }
            Err(e) => error!(error = ?e, "Failed to create CommandTimeoutHandler"),
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
        async { tokio::try_join!(chirpstack_handle, opcua_handle, poller_handle, timeout_handle) },
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

    #[test]
    fn test_metric_restore_from_database() {
        use crate::storage::{StorageBackend, MetricType};
        use std::fs;

        // Create a temporary database
        let db_path = format!("/tmp/opcgw_test_restore_{}.db", uuid::Uuid::new_v4());

        // Create a backend and populate with metrics
        {
            let backend = crate::storage::SqliteBackend::new(&db_path).expect("Should create backend");
            let now = std::time::SystemTime::now();

            // Insert metrics of different types
            backend.upsert_metric_value("device_1", "temperature", &MetricType::Float, now)
                .expect("Should upsert float");
            backend.upsert_metric_value("device_1", "humidity", &MetricType::Int, now)
                .expect("Should upsert int");
            backend.upsert_metric_value("device_2", "active", &MetricType::Bool, now)
                .expect("Should upsert bool");
            backend.upsert_metric_value("device_2", "status", &MetricType::String, now)
                .expect("Should upsert string");
        }

        // Load metrics and populate storage
        {
            let backend = crate::storage::SqliteBackend::new(&db_path).expect("Should create backend");
            let metrics = backend.load_all_metrics().expect("Should load metrics");

            assert_eq!(metrics.len(), 4, "Should have 4 metrics");

            // Verify type conversion
            for metric in metrics {
                assert!(!metric.device_id.is_empty(), "Device ID should not be empty");
                assert!(!metric.metric_name.is_empty(), "Metric name should not be empty");

                // Verify data types are correct
                match metric.metric_name.as_str() {
                    "temperature" => assert_eq!(metric.data_type, MetricType::Float),
                    "humidity" => assert_eq!(metric.data_type, MetricType::Int),
                    "active" => assert_eq!(metric.data_type, MetricType::Bool),
                    "status" => assert_eq!(metric.data_type, MetricType::String),
                    _ => panic!("Unexpected metric name: {}", metric.metric_name),
                }
            }
        }

        let _ = fs::remove_file(&db_path);
    }

    // ===== Story 6-2 tests =====

    use super::{parse_log_level, resolve_log_level};
    use crate::config::LoggingConfig;
    use tracing_subscriber::filter::LevelFilter;

    /// Story 6-2, AC#1: every canonical lowercase value parses to its
    /// matching `LevelFilter`.
    #[test]
    fn parse_log_level_lowercase() {
        assert_eq!(parse_log_level("trace").unwrap(), LevelFilter::TRACE);
        assert_eq!(parse_log_level("debug").unwrap(), LevelFilter::DEBUG);
        assert_eq!(parse_log_level("info").unwrap(), LevelFilter::INFO);
        assert_eq!(parse_log_level("warn").unwrap(), LevelFilter::WARN);
        assert_eq!(parse_log_level("error").unwrap(), LevelFilter::ERROR);
    }

    /// Story 6-2, AC#1: parsing is case-insensitive.
    #[test]
    fn parse_log_level_uppercase_and_mixed() {
        assert_eq!(parse_log_level("TRACE").unwrap(), LevelFilter::TRACE);
        assert_eq!(parse_log_level("Debug").unwrap(), LevelFilter::DEBUG);
        assert_eq!(parse_log_level("iNfO").unwrap(), LevelFilter::INFO);
        assert_eq!(parse_log_level("WARN").unwrap(), LevelFilter::WARN);
        assert_eq!(parse_log_level("ERROR").unwrap(), LevelFilter::ERROR);
        // Whitespace is trimmed.
        assert_eq!(parse_log_level("  info  ").unwrap(), LevelFilter::INFO);
    }

    /// Story 6-2, AC#1: invalid values return `Err(original)` and never
    /// match a real level.
    #[test]
    fn parse_log_level_invalid() {
        assert!(parse_log_level("verbose").is_err());
        assert!(parse_log_level("").is_err());
        assert!(parse_log_level("DEBUG_MORE").is_err());
        assert!(parse_log_level("1").is_err());
        // The error preserves the original input for downstream messages.
        assert_eq!(
            parse_log_level("verbose").err().as_deref(),
            Some("verbose")
        );
    }

    /// Story 6-2, AC#4: precedence — env > config > default. Uses
    /// `temp_env::with_var` to scope env mutations safely under
    /// parallel `cargo test` (the dev-dep was added in Story 6-1).
    #[test]
    fn resolve_log_level_precedence_env_wins() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("warn".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", Some("debug"), || {
            let (lf, source) = resolve_log_level(0, Some(&cfg));
            assert_eq!(lf, LevelFilter::DEBUG);
            assert_eq!(source, "env");
        });
    }

    #[test]
    fn resolve_log_level_precedence_config_used_when_env_unset() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("warn".to_string()),
        };
        // Use temp_env to *unset* the env var even if the host has it set.
        temp_env::with_var("OPCGW_LOG_LEVEL", None::<&str>, || {
            let (lf, source) = resolve_log_level(0, Some(&cfg));
            assert_eq!(lf, LevelFilter::WARN);
            assert_eq!(source, "config");
        });
    }

    #[test]
    fn resolve_log_level_default_when_both_absent() {
        temp_env::with_var("OPCGW_LOG_LEVEL", None::<&str>, || {
            let (lf, source) = resolve_log_level(0, None);
            assert_eq!(lf, LevelFilter::INFO);
            assert_eq!(source, "default");
        });
    }

    /// Story 6-2, AC#4: invalid env value falls through to config (with
    /// stderr warning, which we don't assert on directly here).
    #[test]
    fn resolve_log_level_invalid_env_falls_through_to_config() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("warn".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", Some("verbose"), || {
            let (lf, source) = resolve_log_level(0, Some(&cfg));
            assert_eq!(lf, LevelFilter::WARN);
            assert_eq!(source, "config");
        });
    }

    /// Story 6-2, AC#4: invalid env AND invalid config → default INFO.
    #[test]
    fn resolve_log_level_both_invalid_falls_to_default() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("loud".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", Some("verbose"), || {
            let (lf, source) = resolve_log_level(0, Some(&cfg));
            assert_eq!(lf, LevelFilter::INFO);
            assert_eq!(source, "default");
        });
    }

    /// Story 6-2, AC#6: a `trace!` call site costs effectively nothing when
    /// the subscriber's max level is above TRACE. `tracing` macros do a
    /// runtime level check against the installed subscriber's filter — a
    /// single branch per call site, well below profiler resolution — so no
    /// manual caching is needed. (Compile-time short-circuit only kicks in
    /// when `tracing/release_max_level_*` features are set, which this
    /// project does not enable.) Marked `#[ignore]` — invoke explicitly:
    ///
    /// ```text
    /// cargo test --release --bin opcgw bench_trace_at_error_level -- --ignored --nocapture
    /// ```
    ///
    /// The bench runs 100 000 iterations of two tight loops:
    ///   1. `trace!("…")` (filtered out by the ERROR subscriber set up below).
    ///   2. an empty loop body.
    ///
    /// In release mode the no-op loop is constant-folded to ~0 ns, so the
    /// ratio is meaningless; what matters is the absolute `trace!` cost,
    /// which has measured at ~0.46 ns/iter.
    #[test]
    #[ignore]
    fn bench_trace_at_error_level() {
        use tracing::trace;
        use tracing_subscriber::filter::LevelFilter;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::{fmt, Layer};

        // Iter-3 review pending #5 resolution: scope the ERROR-level
        // subscriber to this bench via `tracing::subscriber::with_default`
        // instead of `try_init`. The previous best-effort `try_init`
        // returned the *globally-installed* subscriber when another test
        // had already set one — so the bench could end up measuring full
        // emission cost (under a TRACE subscriber) instead of filter-skip
        // cost. The scoped subscriber guarantees the bench measures
        // exactly the configured ERROR-level filter.
        //
        // NOTE (iter-3 regression check): `with_default` does NOT propagate
        // the subscriber to spawned threads. The bench loop below is
        // single-threaded — keep it that way. If a future refactor adds
        // `std::thread::spawn` or `tokio::spawn` inside this scope, those
        // threads will fall back to the global default subscriber and the
        // bench number will silently revert to measuring full emission
        // cost instead of the filter-skip path.
        let bench_subscriber = tracing_subscriber::registry().with(
            fmt::layer()
                .with_writer(std::io::sink)
                .with_filter(LevelFilter::ERROR),
        );

        tracing::subscriber::with_default(bench_subscriber, || {
            const ITERS: usize = 100_000;

            // Warm up
            for _ in 0..1_000 {
                trace!(target: "opcgw::bench", x = 1, "noop");
            }

            let t_trace = std::time::Instant::now();
            for i in 0..ITERS {
                trace!(target: "opcgw::bench", iter = i, "should not be emitted");
            }
            let trace_ns = t_trace.elapsed().as_nanos();

            let t_noop = std::time::Instant::now();
            let mut sink: u64 = 0;
            for i in 0..ITERS {
                sink = sink.wrapping_add(i as u64);
            }
            let noop_ns = t_noop.elapsed().as_nanos();
            // Use sink so the optimiser can't elide the loop entirely.
            std::hint::black_box(sink);

            let trace_per = trace_ns as f64 / ITERS as f64;
            let noop_per = noop_ns as f64 / ITERS as f64;
            eprintln!("=== Story 6-2 AC#6 microbench ===");
            eprintln!("iterations: {}", ITERS);
            eprintln!("trace! @ ERROR level: {:.2} ns/iter (total {} ns)", trace_per, trace_ns);
            eprintln!("no-op loop:           {:.2} ns/iter (total {} ns)", noop_per, noop_ns);
            eprintln!(
                "ratio: {:.2}× (trace / no-op) — should be small in release mode",
                if noop_per > 0.0 { trace_per / noop_per } else { f64::INFINITY }
            );
        });
    }

    /// Story 6-2, AC#4: empty env var is treated as unset (matches the
    /// `OPCGW_LOG_DIR` empty-string handling).
    #[test]
    fn resolve_log_level_empty_env_treated_as_unset() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("error".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", Some(""), || {
            let (lf, source) = resolve_log_level(0, Some(&cfg));
            assert_eq!(lf, LevelFilter::ERROR);
            assert_eq!(source, "config");
        });
    }

    /// Empty `[logging].level = ""` (post-figment-merge result of an unset
    /// or empty `OPCGW_LOGGING__LEVEL`) is treated as unset and falls
    /// through cleanly to the default — no warning with empty quotes.
    #[test]
    fn resolve_log_level_empty_config_treated_as_unset() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", None::<&str>, || {
            let (lf, source) = resolve_log_level(0, Some(&cfg));
            assert_eq!(lf, LevelFilter::INFO);
            assert_eq!(source, "default");
        });
    }

    /// Same for whitespace-only config level — symmetric with env handling.
    #[test]
    fn resolve_log_level_whitespace_config_treated_as_unset() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("   ".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", None::<&str>, || {
            let (lf, source) = resolve_log_level(0, Some(&cfg));
            assert_eq!(lf, LevelFilter::INFO);
            assert_eq!(source, "default");
        });
    }

    /// CLI `-d` (count=1) overrides env and config, mapping to DEBUG.
    #[test]
    fn resolve_log_level_cli_single_d_maps_to_debug() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("error".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", Some("warn"), || {
            let (lf, source) = resolve_log_level(1, Some(&cfg));
            assert_eq!(lf, LevelFilter::DEBUG);
            assert_eq!(source, "cli");
        });
    }

    /// CLI `-dd` (count=2+) maps to TRACE and overrides everything.
    #[test]
    fn resolve_log_level_cli_double_d_maps_to_trace() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("error".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", Some("warn"), || {
            let (lf, source) = resolve_log_level(2, Some(&cfg));
            assert_eq!(lf, LevelFilter::TRACE);
            assert_eq!(source, "cli");
        });
        temp_env::with_var("OPCGW_LOG_LEVEL", Some("warn"), || {
            let (lf, source) = resolve_log_level(5, Some(&cfg));
            assert_eq!(lf, LevelFilter::TRACE);
            assert_eq!(source, "cli");
        });
    }

    /// CLI count of 0 is "no override" — fall through to the env/config chain.
    #[test]
    fn resolve_log_level_cli_zero_does_not_override() {
        let cfg = LoggingConfig {
            dir: None,
            level: Some("error".to_string()),
        };
        temp_env::with_var("OPCGW_LOG_LEVEL", None::<&str>, || {
            let (lf, source) = resolve_log_level(0, Some(&cfg));
            assert_eq!(lf, LevelFilter::ERROR);
            assert_eq!(source, "config");
        });
    }

    // ===== resolve_log_dir source-tag tests =====

    use super::resolve_log_dir;

    #[test]
    fn resolve_log_dir_env_wins() {
        let cfg = LoggingConfig {
            dir: Some("/from-config".to_string()),
            level: None,
        };
        temp_env::with_var("OPCGW_LOG_DIR", Some("/from-env"), || {
            let (dir, source) = resolve_log_dir(Some(&cfg));
            assert_eq!(dir, "/from-env");
            assert_eq!(source, "env");
        });
    }

    #[test]
    fn resolve_log_dir_config_used_when_env_unset() {
        let cfg = LoggingConfig {
            dir: Some("/from-config".to_string()),
            level: None,
        };
        temp_env::with_var("OPCGW_LOG_DIR", None::<&str>, || {
            let (dir, source) = resolve_log_dir(Some(&cfg));
            assert_eq!(dir, "/from-config");
            assert_eq!(source, "config");
        });
    }

    #[test]
    fn resolve_log_dir_default_when_both_absent() {
        temp_env::with_var("OPCGW_LOG_DIR", None::<&str>, || {
            let (dir, source) = resolve_log_dir(None);
            assert_eq!(dir, "./log");
            assert_eq!(source, "default");
        });
    }

    // ===== Story 6-3 tests =====

    /// Story 6-3, AC#2: the format string we hand to `ChronoUtc::new` produces
    /// six-digit microsecond precision in UTC. Verifies the formatter through
    /// the actual `FormatTime` trait used by the subscriber, not by re-running
    /// `chrono::Utc::now().format(...)` directly — this is the integration
    /// surface the AC requires.
    #[test]
    fn microsecond_timestamp_format_matches_pattern() {
        
        use tracing_subscriber::fmt::format::Writer;
        use tracing_subscriber::fmt::time::{ChronoUtc, FormatTime};

        let timer = ChronoUtc::new("%Y-%m-%dT%H:%M:%S%.6fZ".to_string());
        let mut buf = String::new();
        let mut w = Writer::new(&mut buf);
        timer.format_time(&mut w).expect("format_time");

        // Expected shape: `YYYY-MM-DDTHH:MM:SS.ffffffZ` — find the dot, then
        // assert exactly six digits, then `Z`.
        let dot = buf.find('.').unwrap_or_else(|| {
            panic!("microsecond timestamp must contain a '.', got {buf:?}")
        });
        let after = &buf[dot + 1..];
        assert!(
            after.ends_with('Z'),
            "microsecond timestamp must end with 'Z', got {buf:?}"
        );
        let micros = &after[..after.len() - 1];
        assert_eq!(
            micros.len(),
            6,
            "expected exactly 6 fractional digits (\\d{{6}}), got {micros:?} in {buf:?}"
        );
        assert!(
            micros.chars().all(|c| c.is_ascii_digit()),
            "fractional component must be all ASCII digits, got {micros:?}"
        );

        // Sanity: the date portion before 'T' is exactly `YYYY-MM-DD`.
        let t_pos = buf.find('T').expect("must have 'T' separator");
        assert_eq!(t_pos, 10, "date prefix must be 10 chars, got {buf:?}");
    }

    #[test]
    fn resolve_log_dir_empty_env_falls_through() {
        let cfg = LoggingConfig {
            dir: Some("/from-config".to_string()),
            level: None,
        };
        temp_env::with_var("OPCGW_LOG_DIR", Some(""), || {
            let (dir, source) = resolve_log_dir(Some(&cfg));
            assert_eq!(dir, "/from-config");
            assert_eq!(source, "config");
        });
    }
}

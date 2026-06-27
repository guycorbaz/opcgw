// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Shared test helpers for the web module's inline `#[cfg(test)]`
//! sub-modules and integration tests under `tests/`. Story 9-4
//! introduced this module so api.rs / web/mod.rs / external
//! integration tests can construct an `AppState` without
//! duplicating the `ConfigReloadHandle` + `SqliteBackend` plumbing
//! at every call site.

#![allow(dead_code)]

use std::sync::Arc;

use tempfile::TempDir;

use crate::config::AppConfig;
use crate::config_reload::ConfigReloadHandle;
use crate::storage::SqliteBackend;

/// Build a `ConfigReloadHandle` + `SqliteBackend` pair pointing at a
/// fresh per-test tempdir. The SQLite backend is created with
/// migrations applied and the `MINIMAL_TOML`'s application list seeded.
/// The `TempDir` keeps the on-disk DB alive; callers must not drop it
/// before the test completes (or call `std::mem::forget`).
pub fn make_test_reload_handle_and_writer(
) -> (Arc<ConfigReloadHandle>, Arc<SqliteBackend>, TempDir) {
    make_test_reload_handle_and_writer_with_apps(
        &stub_app_config().application_list,
    )
}

/// Variant accepting a custom application list. Useful for tests that
/// need specific application_list shapes for CRUD pre-conditions.
pub fn make_test_reload_handle_and_writer_with_apps(
    apps: &[crate::config::ChirpStackApplications],
) -> (Arc<ConfigReloadHandle>, Arc<SqliteBackend>, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("test.db");
    let backend = SqliteBackend::new(db_path.to_str().expect("db path"))
        .expect("sqlite backend");
    // Seed the SQLite backend with the supplied application list so
    // CRUD handlers that read from SQLite have consistent initial state.
    for app in apps {
        backend
            .insert_application(&crate::config::ChirpStackApplications {
                application_id: app.application_id.clone(),
                application_name: app.application_name.clone(),
                device_list: vec![],
            })
            .unwrap_or(());
        for dev in &app.device_list {
            backend
                .insert_device_with_metrics(
                    &app.application_id,
                    &dev.device_id,
                    &dev.device_name,
                    &dev.read_metric_list,
                    dev.stale_threshold_seconds,
                )
                .unwrap_or(());
            if let Some(cmds) = &dev.device_command_list {
                for cmd in cmds {
                    backend
                        .insert_command(&app.application_id, &dev.device_id, cmd)
                        .unwrap_or(());
                }
            }
        }
    }
    let initial = Arc::new(stub_app_config_with_apps(apps));
    let (handle, _rx) = ConfigReloadHandle::new(initial);
    (Arc::new(handle), Arc::new(backend), dir)
}


/// Stub `AppConfig` mirroring `MINIMAL_TOML`. For tests that don't
/// call `reload()` the actual values don't matter — the consumer
/// only needs the type to satisfy `Arc<AppConfig>`.
pub fn stub_app_config() -> AppConfig {
    use crate::config::*;
    let apps = vec![ChirpStackApplications {
        application_name: "Building Sensors".to_string(),
        application_id: "app-1".to_string(),
        device_list: vec![ChirpstackDevice {
            device_id: "dev-1".to_string(),
            device_name: "Dev One".to_string(),
            stale_threshold_seconds: None,
            read_metric_list: vec![ReadMetric {
                metric_name: "temperature".to_string(),
                chirpstack_metric_name: "temperature".to_string(),
                metric_type: OpcMetricTypeConfig::Float,
                metric_unit: Some("C".to_string()),
            }],
            device_command_list: None,
        }],
    }];
    stub_app_config_with_apps(&apps)
}

/// Build an `AppConfig` with a custom application list. Reuses the
/// fixed global/chirpstack/opcua/storage/web stubs so test helpers
/// only need to vary the application shape.
pub fn stub_app_config_with_apps(apps: &[crate::config::ChirpStackApplications]) -> AppConfig {
    use crate::config::*;
    AppConfig {
        global: Global {
            debug: true,
            prune_interval_minutes: 60,
            command_delivery_poll_interval_secs: 5,
            command_delivery_timeout_secs: 60,
            command_timeout_check_interval_secs: 10,
            history_retention_days: 7,
        },
        logging: None,
        chirpstack: ChirpstackPollerConfig {
            server_address: "http://127.0.0.1:18080".to_string(),
            api_token: "t".to_string(),
            tenant_id: "00000000-0000-0000-0000-000000000000".to_string(),
            polling_frequency: 10,
            retry: 1,
            delay: 1,
            list_page_size: 100,
            inventory_cache_ttl_seconds: 60,
            inventory_uplink_max_wait_seconds: 5,
            stream_all_devices: false,
        },
        opcua: OpcUaConfig {
            application_name: "test".to_string(),
            application_uri: "urn:test".to_string(),
            product_uri: "urn:test:product".to_string(),
            diagnostics_enabled: false,
            hello_timeout: Some(5),
            host_ip_address: Some("127.0.0.1".to_string()),
            host_port: Some(4855),
            create_sample_keypair: true,
            certificate_path: "own/cert.der".to_string(),
            private_key_path: "private/private.pem".to_string(),
            trust_client_cert: false,
            check_cert_time: false,
            pki_dir: "./pki".to_string(),
            user_name: "opcua-user".to_string(),
            user_password: "secret".to_string(),
            stale_threshold_seconds: Some(120),
            max_connections: None,
            max_subscriptions_per_session: None,
            max_monitored_items_per_sub: None,
            max_message_size: None,
            max_chunk_count: None,
            max_history_data_results_per_node: None,
        },
        storage: StorageConfig {
            database_path: "data/opcgw.db".to_string(),
            retention_days: 7,
        },
        web: WebConfig {
            port: Some(8080),
            bind_address: Some("127.0.0.1".to_string()),
            auth_realm: Some("opcgw-test".to_string()),
            enabled: Some(false),
            allowed_origins: None,
        },
        command_validation: CommandValidationConfig::default(),
        application_list: apps.to_vec(),
    }
}

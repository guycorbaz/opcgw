// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Shared test helpers for the web module's inline `#[cfg(test)]`
//! sub-modules and integration tests under `tests/`. Story 9-4
//! introduced this module so api.rs / web/mod.rs / external
//! integration tests can construct an `AppState` without
//! duplicating the `ConfigReloadHandle` + `ConfigWriter` plumbing
//! at every call site.

#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::Arc;

use tempfile::TempDir;

use crate::config::AppConfig;
use crate::config_reload::ConfigReloadHandle;
use crate::web::config_writer::ConfigWriter;

/// Build a `ConfigReloadHandle` + `ConfigWriter` pair pointing at a
/// fresh per-test tempdir holding a minimal `config.toml`. Returns
/// the handle, the writer, and the `TempDir` (caller is responsible
/// for keeping the tempdir alive for the duration of the test —
/// dropping it would unlink the TOML file).
pub fn make_test_reload_handle_and_writer(
) -> (Arc<ConfigReloadHandle>, Arc<ConfigWriter>, TempDir) {
    make_test_reload_handle_and_writer_with_toml(MINIMAL_TOML)
}

/// Variant accepting custom TOML content. Useful for tests that
/// need specific application_list shapes for CRUD pre-conditions.
pub fn make_test_reload_handle_and_writer_with_toml(
    toml_str: &str,
) -> (Arc<ConfigReloadHandle>, Arc<ConfigWriter>, TempDir) {
    let dir = TempDir::new().expect("tempdir");
    let toml_path: PathBuf = dir.path().join("config.toml");
    std::fs::write(&toml_path, toml_str).expect("write fixture toml");
    // Construct a stub initial AppConfig — for tests that don't
    // exercise reload(), the initial doesn't have to match the
    // file. For tests that DO exercise reload(), the call to
    // `handle.reload()` re-reads the file and that determines
    // behaviour, not the initial.
    let initial = Arc::new(stub_app_config());
    let (handle, _rx) = ConfigReloadHandle::new(initial, toml_path.clone());
    (Arc::new(handle), ConfigWriter::new(toml_path), dir)
}

const MINIMAL_TOML: &str = r#"
[global]
debug = true
prune_interval_minutes = 60
command_delivery_poll_interval_secs = 5
command_delivery_timeout_secs = 60
command_timeout_check_interval_secs = 10
history_retention_days = 7

[chirpstack]
server_address = "http://127.0.0.1:18080"
api_token = "t"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 10
retry = 1
delay = 1
list_page_size = 100

[opcua]
application_name = "test"
application_uri = "urn:test"
product_uri = "urn:test:product"
diagnostics_enabled = false
hello_timeout = 5
host_ip_address = "127.0.0.1"
host_port = 4855
create_sample_keypair = true
certificate_path = "own/cert.der"
private_key_path = "private/private.pem"
trust_client_cert = false
check_cert_time = false
pki_dir = "./pki"
user_name = "opcua-user"
user_password = "secret"
stale_threshold_seconds = 120

[storage]
database_path = "data/opcgw.db"
retention_days = 7

[web]
port = 8080
bind_address = "127.0.0.1"
enabled = false
auth_realm = "opcgw-test"

[[application]]
application_name = "Building Sensors"
application_id = "app-1"

  [[application.device]]
  device_id = "dev-1"
  device_name = "Dev One"

    [[application.device.read_metric]]
    metric_name = "temperature"
    chirpstack_metric_name = "temperature"
    metric_type = "Float"
    metric_unit = "C"
"#;

/// Stub `AppConfig` mirroring `MINIMAL_TOML`. For tests that don't
/// call `reload()` the actual values don't matter — the consumer
/// only needs the type to satisfy `Arc<AppConfig>`.
fn stub_app_config() -> AppConfig {
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
        application_list: vec![ChirpStackApplications {
            application_name: "Building Sensors".to_string(),
            application_id: "app-1".to_string(),
            device_list: vec![ChirpstackDevice {
                device_id: "dev-1".to_string(),
                device_name: "Dev One".to_string(),
                read_metric_list: vec![ReadMetric {
                    metric_name: "temperature".to_string(),
                    chirpstack_metric_name: "temperature".to_string(),
                    metric_type: OpcMetricTypeConfig::Float,
                    metric_unit: Some("C".to_string()),
                }],
                device_command_list: None,
            }],
        }],
    }
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2024 Guy Corbaz

//! Regression test for the v2.0-GA-blocker deadlock at `src/main.rs:740`
//! discovered 2026-05-20 by the end-to-end test against a real ChirpStack.
//!
//! # Pre-fix behaviour (the bug)
//!
//! `restore_barrier.wait()` was called BEFORE the `chirpstack_poller` task
//! was spawned. The barrier expects 2 participants (main + poller); main
//! blocked forever waiting for the poller to call its own `barrier.wait()`
//! at `src/chirpstack.rs::ChirpstackPoller::run`, but the poller was never
//! spawned because the `tokio::spawn(chirpstack_poller.run())` call came
//! AFTER the already-blocked wait. The OPC UA server task spawn (also after
//! the wait) was equivalently unreachable. The session-count gauge — spawned
//! earlier inside `OpcUa::build()` — continued to fire and misled every CI
//! smoke test into reporting the gateway as "healthy" while it accepted
//! zero OPC UA client connections.
//!
//! # Why 14+ doctrine validations and 9 stories did not catch this
//!
//! - `cargo test --all-targets` exercises sub-modules via test harnesses
//!   that supply their own mock barriers; it never drives `main()`'s full
//!   task-spawn flow.
//! - The B-1 Docker smoke test treated "session-count gauge fires" as
//!   proof of OPC UA server health. The gauge is spawned by `OpcUa::build()`
//!   BEFORE the deadlock point, so it fires happily even when the actual
//!   `TcpListener::bind()` never executes.
//! - Iter-N+1 code reviews catch phrase-harmonization-drift, fake
//!   regression-guards, and closed-enum-doc-sync issues — they cannot
//!   detect runtime deadlocks pre-existing the diff.
//! - Static analysis (`clippy`, `cargo check`) is fundamentally blind to
//!   ordering bugs that produce no compiler warnings.
//!
//! Only end-to-end real-world testing — booting the binary and watching
//! whether the TCP listener actually accepts connections — surfaces this
//! class of bug. This integration test closes the methodology gap by
//! asserting the OPC UA listener is bound within 15 seconds of startup.
//!
//! # Post-fix behaviour (this test's invariant)
//!
//! 1. `chirpstack_poller` task is spawned BEFORE `restore_barrier.wait()`.
//!    The poller's `run()` reaches its own `barrier.wait()` and both
//!    participants release.
//! 2. The OPC UA server task is spawned after the barrier passes.
//! 3. The OPC UA server's `run().await` reaches its `TcpListener::bind()`
//!    call and the listener becomes reachable from clients.
//!
//! If the deadlock regresses, this test fails because the OPC UA port is
//! never bound within the timeout window.

use std::io::Write;
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Write a self-contained config that lets the gateway start up without
/// touching the network beyond its own loopback OPC UA listener.
///
/// - `[chirpstack].server_address` points at a closed loopback port so the
///   gRPC connect fails fast; the deadlock under test is independent of
///   ChirpStack reachability, so this isolates "did `main()` reach the
///   spawns?" from "is ChirpStack up?".
/// - `[opcua].create_sample_keypair = true` lets the gateway auto-generate
///   a self-signed cert into the tempdir's `./pki/` instead of requiring
///   the test runner to provision real PKI.
/// - `[opcua].host_port` uses an unprivileged high port so multiple test
///   invocations and a real running gateway do not conflict.
fn write_minimal_config(temp_dir: &std::path::Path, opcua_port: u16) -> std::path::PathBuf {
    let config_path = temp_dir.join("config.toml");
    let mut f = std::fs::File::create(&config_path).expect("create config.toml");
    write!(
        f,
        r#"[global]
debug = false

[chirpstack]
# Closed loopback port — gRPC connect fails fast; test is independent of
# real ChirpStack reachability.
server_address = "http://127.0.0.1:1"
api_token = "test_token_placeholder"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 60
retry = 1
delay = 1

[opcua]
application_name = "opcgw startup test"
application_uri = "urn:opcgw:test"
product_uri = "urn:opcgw:test"
host_ip_address = "127.0.0.1"
host_port = {opcua_port}
diagnostics_enabled = true
create_sample_keypair = true
certificate_path = "own/cert.der"
private_key_path = "private/private.pem"
trust_client_cert = true
check_cert_time = true
pki_dir = "./pki"
user_name = "opcua-user"
user_password = "test_password_placeholder"

[[application]]
application_name = "TestApp"
application_id = "00000000-0000-0000-0000-000000000001"

[[application.device]]
device_name = "TestDevice"
device_id = "00000000-0000-0000-0000-000000000002"

[[application.device.read_metric]]
metric_name = "TestMetric"
chirpstack_metric_name = "test_metric"
metric_type = "Float"
metric_unit = "m"
"#,
        opcua_port = opcua_port,
    )
    .expect("write config");
    config_path
}

/// Pre-create the directories the gateway expects to find under its CWD.
/// The release binary requires `./log`, `./pki/{own,private,trusted,rejected}`,
/// and `./data` to exist; `create_sample_keypair = true` then populates the
/// PKI files on first run.
fn pre_create_runtime_dirs(temp_dir: &std::path::Path) {
    for sub in [
        "log",
        "data",
        "pki",
        "pki/own",
        "pki/private",
        "pki/trusted",
        "pki/rejected",
    ] {
        std::fs::create_dir_all(temp_dir.join(sub)).expect("create runtime dir");
    }
}

#[test]
fn main_startup_binds_opc_ua_port_within_timeout() {
    // Unprivileged high port to avoid clashes with a real running gateway
    // on the canonical 4840/4855 ports.
    let opcua_port: u16 = 24840;

    let temp_dir = tempfile::tempdir().expect("create tempdir");
    pre_create_runtime_dirs(temp_dir.path());
    write_minimal_config(temp_dir.path(), opcua_port);

    // `CARGO_BIN_EXE_opcgw` is set by cargo for integration tests and
    // points at the freshly-built binary (debug or release per the
    // invocation's profile).
    let bin_path = env!("CARGO_BIN_EXE_opcgw");

    let mut child: Child = Command::new(bin_path)
        .args(["-c", "config.toml"])
        .current_dir(temp_dir.path())
        // Suppress the gateway's stdout/stderr from polluting test output;
        // the test asserts behaviour via the TCP listener, not logs.
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to spawn opcgw binary — is the release/debug binary built?");

    let deadline = Instant::now() + Duration::from_secs(15);
    let target: std::net::SocketAddr = format!("127.0.0.1:{}", opcua_port)
        .parse()
        .expect("parse loopback addr");

    let mut bound = false;
    let mut early_exit = false;

    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&target, Duration::from_millis(300)).is_ok() {
            bound = true;
            break;
        }
        // If the child has exited (e.g., config-parse error, panic), stop
        // waiting; the test will fail with the right diagnostic below.
        match child.try_wait() {
            Ok(Some(_status)) => {
                early_exit = true;
                break;
            }
            Ok(None) => {}
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    // Cleanup: SIGKILL the child (cargo test does not deliver SIGTERM
    // gracefully on all platforms, and the gateway's normal shutdown path
    // is not under test here).
    let _ = child.kill();
    let _ = child.wait();

    if early_exit {
        panic!(
            "opcgw exited before binding OPC UA port {} — config-parse \
             error or panic during startup. Check the integration-test's \
             generated `config.toml` and the binary's stderr.",
            opcua_port
        );
    }

    assert!(
        bound,
        "OPC UA listener was not bound on 127.0.0.1:{} within 15 s. \
         This indicates a regression of the deadlock fixed at \
         `src/main.rs::main` (poller task must be spawned BEFORE \
         `restore_barrier.wait()`; see this test's module doc for the \
         full pre-fix narrative). The gateway process started but the \
         chirpstack_poller and OPC UA server tasks did not get spawned \
         because main blocked on the barrier waiting for a 2nd participant \
         that the bug-ordering ensures never arrives.",
        opcua_port,
    );
}

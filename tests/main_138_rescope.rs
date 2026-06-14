// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2024 Guy Corbaz

//! Story F-0 / CR #138 integration test: the gRPC uplink event-stream **scope**
//! is recomputed from the freshly-read configuration on every Apply.
//!
//! #138 root cause: the set of devices opcgw subscribes to on the ChirpStack
//! `StreamDeviceEvents` gRPC stream was frozen at boot — changing
//! `chirpstack.stream_all_devices` (or adding a valve-class device) required a
//! full container restart to take effect. Under the F-0 staged-apply model the
//! supervisor re-reads the effective config from SQLite on Apply and re-spawns
//! `run_event_ingestion`, which recomputes `streamed_devices(&config)`. This
//! test proves that end-to-end through the real binary:
//!
//!   1. Boot with `stream_all_devices = false` and a single non-valve device →
//!      `streamed_devices` is empty → the event task logs `uplink_ingestion_idle`
//!      and never logs `uplink_ingestion_start`.
//!   2. Stage a singleton edit flipping `chirpstack.stream_all_devices = true`
//!      via the real `PUT /api/config/singleton/chirpstack` staging endpoint
//!      (writes SQLite, does NOT restart).
//!   3. `POST /api/config/apply` → the supervisor re-reads config and re-spawns
//!      the data-plane → `streamed_devices` now returns the device → the event
//!      task logs `uplink_ingestion_start`.
//!
//! Only a subprocess test exercises the boot → SQLite-overlay → apply →
//! re-scope path against the real config-load machinery. The test config points
//! gRPC at a dead address on purpose: the `uplink_ingestion_start` scope log is
//! emitted BEFORE any stream connection is attempted, so we assert on the
//! scope-log signal, not on a live ChirpStack stream.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Minimal std-only base64 (standard alphabet) for the Basic-auth header —
/// avoids pulling a crate into the integration-test target.
fn base64(input: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if chunk.len() > 1 { T[((n >> 6) & 63) as usize] as char } else { '=' });
        out.push(if chunk.len() > 2 { T[(n & 63) as usize] as char } else { '=' });
    }
    out
}

fn write_config(temp_dir: &std::path::Path, opcua_port: u16, web_port: u16) -> std::path::PathBuf {
    let config_path = temp_dir.join("config.toml");
    let mut f = std::fs::File::create(&config_path).expect("create config.toml");
    // NOTE: `stream_all_devices` is intentionally left at its `false` default
    // and the single device is non-valve, so the boot-time stream scope is
    // empty (`uplink_ingestion_idle`).
    write!(
        f,
        r#"[global]
debug = false

[chirpstack]
server_address = "http://127.0.0.1:1"
api_token = "test_token_placeholder"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 60
retry = 1
delay = 1

[opcua]
application_name = "opcgw 138 rescope test"
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

[web]
enabled = true
port = {web_port}
bind_address = "127.0.0.1"

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
        web_port = web_port,
    )
    .expect("write config");
    config_path
}

fn pre_create_runtime_dirs(temp_dir: &std::path::Path) {
    for sub in ["log", "data", "pki", "pki/own", "pki/private", "pki/trusted", "pki/rejected"] {
        std::fs::create_dir_all(temp_dir.join(sub)).expect("create runtime dir");
    }
}

fn wait_for_bind(port: u16, timeout: Duration) -> bool {
    let target: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&target, Duration::from_millis(300)).is_ok() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    false
}

/// Send an authenticated, same-origin HTTP request with an optional JSON body
/// and return the status line (e.g. `"HTTP/1.1 202 Accepted"`).
fn http_request(web_port: u16, method: &str, path: &str, json_body: Option<&str>) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", web_port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let auth = base64(b"opcua-user:test_password_placeholder");
    let body = json_body.unwrap_or("");
    let req = format!(
        "{method} {path} HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Authorization: Basic {auth}\r\n\
         Origin: http://127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n\
         {body}",
        method = method,
        path = path,
        port = web_port,
        auth = auth,
        len = body.len(),
        body = body,
    );
    stream.write_all(req.as_bytes())?;
    let mut resp = String::new();
    stream.read_to_string(&mut resp)?;
    Ok(resp.lines().next().unwrap_or("").to_string())
}

/// Kill-on-drop wrapper so a panicking assertion never leaks a gateway process
/// (a leaked process holds the OPC UA + web ports and breaks subsequent runs).
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn read_log(stderr_path: &std::path::Path) -> String {
    std::fs::read_to_string(stderr_path).unwrap_or_default()
}

#[test]
fn apply_recomputes_grpc_stream_scope_from_fresh_config() {
    let opcua_port: u16 = 24862;
    let web_port: u16 = 24875;

    let temp_dir = tempfile::tempdir().expect("create tempdir");
    pre_create_runtime_dirs(temp_dir.path());
    write_config(temp_dir.path(), opcua_port, web_port);

    let stderr_path = temp_dir.path().join("stderr.log");
    let stderr_file = std::fs::File::create(&stderr_path).expect("create stderr log");

    let bin_path = env!("CARGO_BIN_EXE_opcgw");
    let mut child = ChildGuard(
        Command::new(bin_path)
            .args(["-c", "config.toml"])
            .current_dir(temp_dir.path())
            .stdout(Stdio::null())
            .stderr(stderr_file)
            .spawn()
            .expect("spawn opcgw binary"),
    );

    // Boot: OPC UA + web must both bind.
    assert!(
        wait_for_bind(opcua_port, Duration::from_secs(20)),
        "OPC UA port {opcua_port} never bound at startup"
    );
    assert!(
        wait_for_bind(web_port, Duration::from_secs(20)),
        "web port {web_port} never bound at startup"
    );

    // create_sample_keypair generates the private key at 0o664; the apply
    // re-read runs full NFR9 validation which requires 0o600. Mirror the
    // operator-provisioned permissions so the apply validates cleanly.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let key = temp_dir.path().join("pki/private/private.pem");
        let _ = std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600));
        let dir = temp_dir.path().join("pki/private");
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    // --- Boot-scope assertion: the stream scope is empty (#138 pre-state). ---
    // Give the event task a moment to log its (idle) scope decision.
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !read_log(&stderr_path).contains("uplink_ingestion_idle") {
        std::thread::sleep(Duration::from_millis(200));
    }
    let boot_log = read_log(&stderr_path);
    assert!(
        boot_log.contains("uplink_ingestion_idle"),
        "boot event task should be idle (stream_all_devices=false, non-valve device); stderr:\n{}",
        boot_log.lines().rev().take(20).collect::<Vec<_>>().join("\n")
    );
    assert!(
        !boot_log.contains("uplink_ingestion_start"),
        "the gRPC stream scope must be EMPTY at boot — saw uplink_ingestion_start before any config change"
    );

    // --- Stage the re-scope: flip stream_all_devices via the real staging endpoint. ---
    let put_status = http_request(
        web_port,
        "PUT",
        "/api/config/singleton/chirpstack",
        Some(r#"{"stream_all_devices": true}"#),
    )
    .expect("PUT /api/config/singleton/chirpstack");
    assert!(
        put_status.contains("202"),
        "staging stream_all_devices=true should return 202 Accepted, got: {put_status:?}"
    );

    // --- Apply: the supervisor re-reads config and re-spawns the data-plane. ---
    let apply_status = http_request(web_port, "POST", "/api/config/apply", None)
        .expect("POST /api/config/apply");
    assert!(
        apply_status.contains("202"),
        "apply did not return 202 Accepted: {apply_status:?}"
    );

    // --- Post-apply assertion: the stream scope is recomputed (#138 fixed). ---
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !read_log(&stderr_path).contains("uplink_ingestion_start") {
        std::thread::sleep(Duration::from_millis(200));
    }
    let final_log = read_log(&stderr_path);
    assert!(
        final_log.contains("apply_completed"),
        "supervisor did not log apply_completed after the apply; stderr tail:\n{}",
        final_log.lines().rev().take(25).collect::<Vec<_>>().join("\n")
    );
    assert!(
        final_log.contains("uplink_ingestion_start"),
        "after Apply the gRPC stream scope was NOT recomputed — #138 would still require a container restart.\nstderr tail:\n{}",
        final_log.lines().rev().take(25).collect::<Vec<_>>().join("\n")
    );

    // The process must still be the same one (in-process soft restart, #138 no
    // longer needs a container restart to widen the stream scope).
    assert!(
        matches!(child.0.try_wait(), Ok(None)),
        "gateway process exited on apply — #138 re-scope must happen in-process, not via container restart"
    );

    // `child` (ChildGuard) is killed on drop.
}

// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2024 Guy Corbaz

//! Story F-0 integration test: `POST /api/config/apply` performs an
//! **in-process** soft restart of the data-plane — the gateway process does
//! NOT exit (the Docker container is never restarted), the OPC UA listener
//! rebinds, and a SECOND apply also works (the supervisor loop is
//! re-entrant, not one-shot).
//!
//! This is the F-0 analogue of `tests/main_startup_no_deadlock.rs`: only an
//! end-to-end subprocess test — booting the real binary and driving it over
//! TCP/HTTP — can prove the restart supervisor cycles the data-plane in
//! process without the deadlock-prone `main.rs` spawn ordering regressing.
//!
//! Liveness proof: stderr is captured to a file and we assert the
//! supervisor's `apply_completed` event appears once per apply, AND that the
//! child PID never changes (the process never exited).

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
application_name = "opcgw apply test"
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

/// POST /api/config/apply with Basic auth + same-origin header; returns the
/// HTTP status line (e.g. "HTTP/1.1 202 Accepted").
fn post_apply(web_port: u16) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", web_port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let auth = base64(b"opcua-user:test_password_placeholder");
    let req = format!(
        "POST /api/config/apply HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Authorization: Basic {auth}\r\n\
         Origin: http://127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\r\n",
        port = web_port,
        auth = auth,
    );
    stream.write_all(req.as_bytes())?;
    let mut resp = String::new();
    stream.read_to_string(&mut resp)?;
    Ok(resp.lines().next().unwrap_or("").to_string())
}

/// Stage a (benign) singleton config change so a subsequent Apply has
/// something pending to apply (Story F-0 review P4: Apply with no pending
/// changes is a no-op). PUTs `{"debug": false}` to the `[global]` section —
/// re-writing the same value still stages a pending change. Returns the
/// status line (expects "... 202 Accepted").
fn stage_singleton_change(web_port: u16) -> std::io::Result<String> {
    let mut stream = TcpStream::connect(("127.0.0.1", web_port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    let auth = base64(b"opcua-user:test_password_placeholder");
    let body = r#"{"debug": false}"#;
    let req = format!(
        "PUT /api/config/singleton/global HTTP/1.1\r\n\
         Host: 127.0.0.1:{port}\r\n\
         Authorization: Basic {auth}\r\n\
         Origin: http://127.0.0.1:{port}\r\n\
         Content-Type: application/json\r\n\
         Content-Length: {len}\r\n\
         Connection: close\r\n\r\n\
         {body}",
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

/// Kill-on-drop wrapper so a panicking assertion never leaks a gateway
/// process (a leaked process holds the OPC UA + web ports and breaks every
/// subsequent run with `Address already in use`).
struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl ChildGuard {
    fn id(&self) -> u32 {
        self.0.id()
    }
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.0.try_wait()
    }
}

fn count_apply_completed(stderr_path: &std::path::Path) -> usize {
    std::fs::read_to_string(stderr_path)
        .map(|s| s.matches("apply_completed").count())
        .unwrap_or(0)
}

#[test]
fn apply_soft_restarts_data_plane_in_process_and_is_reentrant() {
    let opcua_port: u16 = 24842;
    let web_port: u16 = 24855;

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

    // `create_sample_keypair = true` generates the private key at 0o664 during
    // OPC UA build; the apply re-read runs the full NFR9 validation (the key
    // exists by now), which requires 0o600. Production keys are
    // operator-provisioned at 0o600 — mirror that here so the apply path
    // validates a clean environment rather than the dev-keygen permission gap.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let key = temp_dir.path().join("pki/private/private.pem");
        let _ = std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600));
        let dir = temp_dir.path().join("pki/private");
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }

    let pid_before = child.id();

    // ---- First Apply ----
    // Stage a change first (review P4: Apply with no pending changes is a no-op).
    let stage1 = stage_singleton_change(web_port).expect("stage singleton change (1)");
    assert!(
        stage1.contains("202"),
        "staging a singleton change did not return 202: {stage1:?}"
    );
    let status = post_apply(web_port).expect("POST /api/config/apply (1)");
    assert!(
        status.contains("202"),
        "first apply did not return 202 Accepted: {status:?}"
    );

    // The data-plane respawns: wait for the supervisor's apply_completed and
    // for the OPC UA port to be bound again.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && count_apply_completed(&stderr_path) < 1 {
        std::thread::sleep(Duration::from_millis(200));
    }
    if count_apply_completed(&stderr_path) < 1 {
        let log = std::fs::read_to_string(&stderr_path).unwrap_or_default();
        let tail: String = log.lines().rev().take(20).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
        panic!("supervisor did not log apply_completed after the first apply.\n--- stderr tail ---\n{tail}");
    }
    assert!(
        matches!(child.try_wait(), Ok(None)),
        "gateway process EXITED on apply — F-0 requires an in-process soft restart, not a process/container restart"
    );
    assert_eq!(child.id(), pid_before, "process PID changed — the process restarted instead of soft-restarting in place");
    assert!(
        wait_for_bind(opcua_port, Duration::from_secs(15)),
        "OPC UA port did not rebind after the first apply"
    );

    // ---- Second Apply (re-entrancy) ----
    let stage2 = stage_singleton_change(web_port).expect("stage singleton change (2)");
    assert!(
        stage2.contains("202"),
        "staging a singleton change (2) did not return 202: {stage2:?}"
    );
    let status2 = post_apply(web_port).expect("POST /api/config/apply (2)");
    assert!(status2.contains("202"), "second apply did not return 202: {status2:?}");
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline && count_apply_completed(&stderr_path) < 2 {
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        count_apply_completed(&stderr_path) >= 2,
        "supervisor did not cycle a SECOND time — the restart loop is not re-entrant"
    );
    assert!(
        matches!(child.try_wait(), Ok(None)),
        "gateway process exited on the second apply"
    );
    assert_eq!(child.id(), pid_before, "PID changed across the second apply");
    assert!(
        wait_for_bind(opcua_port, Duration::from_secs(15)),
        "OPC UA port did not rebind after the second apply"
    );

    // `child` (ChildGuard) is killed on drop.
}

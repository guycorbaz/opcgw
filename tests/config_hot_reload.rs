// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story 9-7 integration tests — configuration hot-reload via the
//! `ConfigReloadHandle` reload routine.
//!
//! Per the spec (`9-7-configuration-hot-reload.md` § Task 6, last
//! bullet), these tests **do NOT actually fire SIGHUP at the test
//! process**. SIGHUP wiring is exercised by the manual smoke test
//! described in Task 8. Instead each test:
//!   1. Writes a per-test TOML to a `tempfile::NamedTempFile`.
//!   2. Constructs a `ConfigReloadHandle::new(initial, path)`.
//!   3. Mutates the TOML on disk (rewrite via `std::fs::write`).
//!   4. Calls `handle.reload()` directly and asserts on the outcome
//!      and on captured `tracing` events (where applicable).
//!
//! The `tracing-test` capture is shared across tests, so anything
//! that asserts on log content uses `#[traced_test]` to scope its
//! own buffer.

use std::sync::Arc;

use opcgw::config::AppConfig;
use opcgw::config_reload::{ConfigReloadHandle, ReloadOutcome};
use tempfile::NamedTempFile;
use tracing_test::traced_test;

/// Minimal TOML body — used as the starting state for most tests.
/// Mirrors `tests/config/config.toml` shape but trimmed to one
/// application + one device + one metric so the diffs are easy to
/// reason about.
const BASE_TOML: &str = r#"
[global]
debug = true

[chirpstack]
server_address = "http://localhost:18080"
api_token = "test_token"
tenant_id = "00000000-0000-0000-0000-000000000000"
polling_frequency = 10
retry = 30
delay = 1

[opcua]
application_name = "test"
application_uri = "urn:test"
product_uri = "urn:test:product"
diagnostics_enabled = true
create_sample_keypair = true
certificate_path = "own/cert.der"
private_key_path = "private/private.pem"
trust_client_cert = true
check_cert_time = true
pki_dir = "./pki"
user_name = "test-user"
user_password = "test-pass"
host_port = 4855
host_ip_address = "127.0.0.1"
stale_threshold_seconds = 60

[[application]]
application_name = "App1"
application_id = "550e8400-e29b-41d4-a716-446655440001"

[[application.device]]
device_name = "Dev1"
device_id = "550e8400-e29b-41d4-a716-446655440011"

[[application.device.read_metric]]
metric_name = "temperature"
chirpstack_metric_name = "temp"
metric_type = "Float"
metric_unit = "C"
"#;

/// Helper — write a TOML body to a fresh `NamedTempFile` and load it
/// via `AppConfig::from_path`. Returns `(temp, config)`. The temp
/// file must be retained by the caller so its destructor doesn't
/// race the reload.
fn write_and_load(toml_body: &str) -> (NamedTempFile, AppConfig) {
    let temp = NamedTempFile::new().expect("create tempfile");
    std::fs::write(temp.path(), toml_body).expect("write toml body");
    let config = AppConfig::from_path(&temp.path().to_string_lossy())
        .expect("initial config must load + validate");
    (temp, config)
}

/// Helper — overwrite the temp file with new TOML content. The path
/// is preserved so the reload routine re-reads from the same
/// location.
fn rewrite(temp: &NamedTempFile, toml_body: &str) {
    std::fs::write(temp.path(), toml_body).expect("rewrite toml");
}

// =============================================================================
// AC#1 — validation-first reload, atomic swap
// =============================================================================

/// AC#1 — validation failure leaves the watch channel untouched and
/// returns a Validation reason.
#[tokio::test]
async fn reload_rejects_invalid_candidate() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, mut rx) = ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Mutate the TOML to an invalid value (`retry = 0` violates the
    // chirpstack `retry > 0` invariant in `AppConfig::validate`).
    let invalid = BASE_TOML.replace("retry = 30", "retry = 0");
    rewrite(&temp, &invalid);

    let result = handle.reload().await;
    let err = result.expect_err("validation must fail for retry = 0");
    assert_eq!(err.reason(), "validation");
    assert_eq!(err.changed_knob(), None);

    // Watch channel must be unchanged — receiver still sees the
    // initial config (`borrow_and_update` returns the live view).
    let live = rx.borrow_and_update().clone();
    assert_eq!(live.chirpstack.retry, 30);
}

/// AC#1 — successful reload of a hot-reload-safe knob publishes a
/// new `Arc<AppConfig>` to the watch channel and returns a
/// `Changed { changed_section_count >= 1, .. }` outcome.
#[tokio::test]
async fn reload_succeeds_for_valid_candidate() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, mut rx) = ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // `retry = 30 → 5` is a hot-reload-safe scalar change in the
    // chirpstack section (per knob taxonomy).
    let mutated = BASE_TOML.replace("retry = 30", "retry = 5");
    rewrite(&temp, &mutated);

    let outcome = handle
        .reload()
        .await
        .expect("hot-reload-safe change must succeed");
    match outcome {
        ReloadOutcome::Changed {
            changed_section_count,
            includes_topology_change,
            ..
        } => {
            assert!(changed_section_count >= 1, "at least 1 section changed");
            assert!(!includes_topology_change, "scalar tweak is not topology");
        }
        ReloadOutcome::NoChange => panic!("expected Changed, got NoChange"),
    }

    // Watch channel must now publish the new value. `changed()`
    // resolves once the sender publishes; then `borrow_and_update`
    // returns the new view.
    rx.changed().await.expect("changed() must resolve");
    let live = rx.borrow_and_update().clone();
    assert_eq!(live.chirpstack.retry, 5);
}

/// Equal candidate (no-op reload) returns `NoChange` and does NOT
/// publish to the watch channel.
#[tokio::test]
async fn reload_with_equal_candidate_returns_no_change() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, mut rx) = ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Don't mutate the file at all — re-read the same content.
    let outcome = handle.reload().await.expect("equal-config reload must succeed");
    assert_eq!(outcome, ReloadOutcome::NoChange);

    // Watch channel must NOT have been touched. We can't directly
    // assert "no publish happened" via the watch API, but we can
    // assert the receiver still observes the same Arc-equal value.
    let live = rx.borrow_and_update().clone();
    assert!(Arc::ptr_eq(&live, &initial_arc));
}

// =============================================================================
// AC#2 — hot-reload-safe knob propagation
// =============================================================================

/// AC#2 surrogate — a fresh subscriber observes the new
/// `chirpstack.retry` value after a successful reload. Iter-1 review
/// P10: renamed for honesty — this test does NOT drive
/// `ChirpstackPoller::run`'s outer `tokio::select!` arm; that requires
/// a stub gRPC server (deferred — same pattern as Story 4-4's
/// `poll_metrics` integration test deferral). The test verifies the
/// watch-channel-to-subscriber contract, which is the load-bearing
/// piece the poller's run loop consumes via `borrow_and_update()`.
#[tokio::test]
async fn reload_publishes_new_retry_to_subscribers() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, _initial_rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Subscribe AFTER construction — mimics the production wiring
    // where each subsystem subscribes via `handle.subscribe()`.
    let mut poller_rx = handle.subscribe();
    assert_eq!(poller_rx.borrow_and_update().chirpstack.retry, 30);

    let mutated = BASE_TOML.replace("retry = 30", "retry = 10");
    rewrite(&temp, &mutated);
    let _ = handle.reload().await.expect("reload must succeed");

    poller_rx.changed().await.expect("changed() must resolve");
    assert_eq!(poller_rx.borrow_and_update().chirpstack.retry, 10);
}

// =============================================================================
// AC#3 — restart-required knob rejection
// =============================================================================

/// AC#3 — `host_port = 4855 → 4856` is rejected with
/// `reason="restart_required"` and a `changed_knob` field naming
/// the offending knob.
#[tokio::test]
async fn reload_rejects_restart_required_port_change() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, mut rx) = ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    let mutated = BASE_TOML.replace("host_port = 4855", "host_port = 4856");
    rewrite(&temp, &mutated);

    let err = handle
        .reload()
        .await
        .expect_err("port change must be rejected");
    assert_eq!(err.reason(), "restart_required");
    assert_eq!(err.changed_knob(), Some("opcua.host_port"));

    // Watch channel must still hold the old config.
    let live = rx.borrow_and_update().clone();
    assert_eq!(live.opcua.host_port, Some(4855));
}

/// AC#3 surrogate — same reject discipline for `chirpstack.server_address`,
/// the most common "I just want to change the URL" mistake.
#[tokio::test]
async fn reload_rejects_restart_required_server_address_change() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, _rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    let mutated = BASE_TOML.replace(
        r#"server_address = "http://localhost:18080""#,
        r#"server_address = "http://chirpstack.example.com:8080""#,
    );
    rewrite(&temp, &mutated);

    let err = handle
        .reload()
        .await
        .expect_err("server_address change must be rejected");
    assert_eq!(err.reason(), "restart_required");
    assert_eq!(err.changed_knob(), Some("chirpstack.server_address"));
}

/// AC#3 surrogate — credential rotation is restart-required in v1
/// (documented limitation; see `docs/security.md`). Operators
/// rotating `[opcua].user_password` must restart the gateway.
#[tokio::test]
async fn reload_rejects_credential_change_v1_limitation() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, _rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    let mutated = BASE_TOML.replace(
        r#"user_password = "test-pass""#,
        r#"user_password = "rotated-pass""#,
    );
    rewrite(&temp, &mutated);

    let err = handle
        .reload()
        .await
        .expect_err("credential change must be rejected in v1");
    assert_eq!(err.reason(), "restart_required");
    assert_eq!(err.changed_knob(), Some("opcua.user_password"));
}

// =============================================================================
// AC#4 — dashboard reflection (topology change at the watch level)
// =============================================================================

/// AC#4 surrogate — adding a third device to the same application
/// produces a topology change visible via the watch channel. A full
/// `/api/status` integration would require spinning up the web
/// server; this test pins the watch-channel side that the web
/// listener consumes.
#[tokio::test]
async fn dashboard_reflects_added_device_after_reload() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, mut rx) = ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Initial: 1 application × 1 device.
    {
        let live = rx.borrow_and_update().clone();
        assert_eq!(live.application_list.len(), 1);
        assert_eq!(live.application_list[0].device_list.len(), 1);
    }

    // Append a second device to the same application.
    let extended = format!(
        r#"{BASE_TOML}

[[application.device]]
device_name = "Dev2"
device_id = "550e8400-e29b-41d4-a716-446655440012"

[[application.device.read_metric]]
metric_name = "humidity"
chirpstack_metric_name = "humid"
metric_type = "Float"
metric_unit = "%"
"#
    );
    rewrite(&temp, &extended);

    let outcome = handle
        .reload()
        .await
        .expect("topology-only change must succeed");
    match outcome {
        ReloadOutcome::Changed {
            includes_topology_change,
            ..
        } => assert!(includes_topology_change),
        _ => panic!("expected Changed with topology flag, got {outcome:?}"),
    }

    rx.changed().await.expect("changed() must resolve");
    let live = rx.borrow_and_update().clone();
    assert_eq!(live.application_list[0].device_list.len(), 2);
}

// =============================================================================
// AC#9 — secrets + permissions
// =============================================================================

/// AC#9 — reload routine surfaces a `validation` reason (sanitised
/// error message) when the candidate config carries a private-key
/// path with loose perms. This test creates a real file with mode
/// 0644 and points the candidate at it; the validate step refuses.
///
/// Unix-only because the perms model is POSIX-specific.
#[cfg(unix)]
#[tokio::test]
async fn reload_rejects_loose_private_key_perms() {
    use std::os::unix::fs::PermissionsExt;

    // Set up: build a parent directory with mode 0700 and a
    // private-key file with mode 0644 (would be flagged as loose by
    // `validate_private_key_permissions`).
    let dir = tempfile::tempdir().expect("create tempdir for pki");
    let parent = dir.path().join("private");
    std::fs::create_dir(&parent).expect("create parent dir");
    let mut parent_perms = std::fs::metadata(&parent).expect("read parent perms").permissions();
    parent_perms.set_mode(0o700);
    std::fs::set_permissions(&parent, parent_perms).expect("set parent perms");
    let key_path = parent.join("loose.pem");
    std::fs::write(&key_path, b"-----BEGIN PRIVATE KEY-----\n").expect("write key");
    let mut key_perms = std::fs::metadata(&key_path).expect("read key perms").permissions();
    key_perms.set_mode(0o644);
    std::fs::set_permissions(&key_path, key_perms).expect("set loose key perms");

    // Initial config points to the existing test PKI (defaults that
    // the build script ships).
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, _rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Mutate: point private_key_path at the loose-perms file.
    let key_str = key_path.to_string_lossy();
    let mutated = BASE_TOML.replace(
        r#"private_key_path = "private/private.pem""#,
        &format!(r#"private_key_path = "{key_str}""#),
    );
    rewrite(&temp, &mutated);

    let err = handle
        .reload()
        .await
        .expect_err("loose key perms must be rejected");
    // Iter-1 review P11: tightened from `validation | restart_required`
    // to just `validation`. AC#9 explicitly requires the perm validator
    // to run; `load_and_validate` calls `AppConfig::validate()` BEFORE
    // `classify_diff`, so a loose-perms key is caught with
    // `reason="validation"` even though the path field also changed
    // (which would otherwise be a restart-required reject). If this
    // assertion ever flips back to `restart_required`, it means the
    // perm validator stopped running — surface that as a regression.
    assert_eq!(
        err.reason(),
        "validation",
        "loose private key perms must trigger validation rejection (not restart_required); got reason={}",
        err.reason()
    );
}

/// AC#9 — reload routine never logs the secret values via its own
/// log emissions. (We don't emit logs from the reload routine
/// directly — all logging is in the SIGHUP listener in
/// `src/main.rs`. This test pins the contract that the
/// `ReloadError`'s `Display` impl never includes the api_token,
/// password, or PKI material.)
#[tokio::test]
async fn reload_does_not_log_secrets() {
    let secret_token = "SECRET_SENTINEL_TOKEN_DO_NOT_LEAK";
    let secret_pass = "SECRET_SENTINEL_PASS_DO_NOT_LEAK";

    let with_secrets = BASE_TOML
        .replace(
            r#"api_token = "test_token""#,
            &format!(r#"api_token = "{secret_token}""#),
        )
        .replace(
            r#"user_password = "test-pass""#,
            &format!(r#"user_password = "{secret_pass}""#),
        );
    let (temp, initial) = write_and_load(&with_secrets);
    let initial_arc = Arc::new(initial);
    let (handle, _rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Mutate: trigger a validation failure so we capture the error
    // string. `retry = 0` is a known-invalid value that produces a
    // `Configuration("...")` error.
    let invalid = with_secrets.replace("retry = 30", "retry = 0");
    rewrite(&temp, &invalid);

    let err = handle.reload().await.expect_err("retry=0 must fail");
    let rendered = err.to_string();
    assert!(
        !rendered.contains(secret_token),
        "ReloadError must not leak api_token in its Display impl: {rendered}"
    );
    assert!(
        !rendered.contains(secret_pass),
        "ReloadError must not leak user_password in its Display impl: {rendered}"
    );
}

/// Iter-1 review P12 — companion to `reload_does_not_log_secrets`
/// covering the `Io` error path (figment IO/parse failures). A
/// malformed TOML can produce error messages that include surrounding
/// context lines; pin that no secrets leak even when the parser
/// quotes nearby content.
#[tokio::test]
async fn reload_does_not_log_secrets_on_io_failure() {
    let secret_token = "SECRET_SENTINEL_TOKEN_DO_NOT_LEAK";
    let secret_pass = "SECRET_SENTINEL_PASS_DO_NOT_LEAK";

    let with_secrets = BASE_TOML
        .replace(
            r#"api_token = "test_token""#,
            &format!(r#"api_token = "{secret_token}""#),
        )
        .replace(
            r#"user_password = "test-pass""#,
            &format!(r#"user_password = "{secret_pass}""#),
        );
    let (temp, initial) = write_and_load(&with_secrets);
    let initial_arc = Arc::new(initial);
    let (handle, _rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Mutate: corrupt the TOML to trigger an `Io` (figment parse)
    // error. The corruption is positioned right next to the secret
    // tokens so figment's "near here" error context — if it ever
    // included surrounding lines — would catch the leak.
    let corrupted = with_secrets.replace("retry = 30", "retry = INVALID_NOT_A_NUMBER &&&");
    rewrite(&temp, &corrupted);

    let err = handle.reload().await.expect_err("malformed TOML must fail");
    assert_eq!(
        err.reason(),
        "io",
        "expected io reason for parse failure, got {}",
        err.reason()
    );
    let rendered = err.to_string();
    assert!(
        !rendered.contains(secret_token),
        "ReloadError(Io) must not leak api_token: {rendered}"
    );
    assert!(
        !rendered.contains(secret_pass),
        "ReloadError(Io) must not leak user_password: {rendered}"
    );
}

// =============================================================================
// AC#10 — stale-threshold semantics
// =============================================================================

/// AC#10 — stale_threshold_seconds is hot-reload-safe; the new value
/// is published to the watch channel and any subscriber (web
/// AppState listener) sees it on the next `borrow_and_update`.
///
/// **v1 limitation:** the OPC UA path captures the threshold into
/// per-variable read-callback closures at startup, so the
/// hot-reloaded value affects ONLY the web dashboard's "Good →
/// Uncertain" boundary. Documented in `docs/security.md §
/// Configuration hot-reload`.
#[tokio::test]
async fn stale_threshold_change_propagates_to_subscribers() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, mut rx) = ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());
    assert_eq!(
        rx.borrow_and_update().opcua.stale_threshold_seconds,
        Some(60)
    );

    let mutated = BASE_TOML.replace(
        "stale_threshold_seconds = 60",
        "stale_threshold_seconds = 120",
    );
    rewrite(&temp, &mutated);
    let _ = handle.reload().await.expect("threshold change must succeed");

    rx.changed().await.expect("changed() must resolve");
    assert_eq!(
        rx.borrow_and_update().opcua.stale_threshold_seconds,
        Some(120)
    );
}

// =============================================================================
// IO failure path
// =============================================================================

/// IO reason — config file deleted between startup and reload.
#[tokio::test]
async fn reload_returns_io_reason_when_file_missing() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let path = temp.path().to_path_buf();
    let (handle, _rx) = ConfigReloadHandle::new(initial_arc.clone(), path.clone());

    // Drop the temp file so the path is gone.
    drop(temp);

    let err = handle
        .reload()
        .await
        .expect_err("missing file must fail with io");
    // The pre-flight existence check returns Io; if a race against
    // recreation happens to skip that branch, the figment load
    // would also return Io. Either way the reason is io.
    //
    // Iter-1 review P16: dropped a fragile substring assertion over
    // the error string (`display.contains(path) || display.contains("figment")`)
    // because both branches matched unstable wording (figment crate
    // updates flipped them historically). The structured `reason()`
    // assertion below is the load-bearing pin; the human-readable
    // message is operator-facing and not a contract.
    assert_eq!(err.reason(), "io");
    let _ = path;
}

/// Smoke test — `_initial_reload_rx` returned by `new()` is a working
/// `watch::Receiver` (the contract the spec promises).
#[tokio::test]
async fn new_returns_initial_receiver_observing_initial_config() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (_handle, mut rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    let live = rx.borrow_and_update().clone();
    assert!(Arc::ptr_eq(&live, &initial_arc));
}

// =============================================================================
// AC#6 — subscriptions uninterrupted across hot-reload-safe knob changes
// =============================================================================

/// Iter-1 review P22 — AC#6 surrogate (`subscriptions_uninterrupted_across_safe_reload`).
///
/// Spec AC#6 demands: "an OPC UA client subscribed to a variable
/// continues delivering DataChange notifications without
/// interruption" on a hot-reload-safe knob change. Full end-to-end
/// verification needs the OPC UA spike's `subscribe_one` harness,
/// which currently lives in `tests/opcua_dynamic_address_space_spike.rs`
/// as a private function. Sharing it requires extracting a common
/// test-helpers module — out of scope for the iter-1 patch round
/// (tracked alongside Story 4-4's poll_metrics deferral).
///
/// What this test pins **structurally**: a hot-reload-safe knob
/// change (chirpstack.retry) yields `Changed { includes_topology_change: false }`,
/// so the OPC UA-side topology-change seam (`log_topology_diff`)
/// emits NO `topology_change_detected` event, so Story 9-8 (when it
/// lands) would NOT mutate the address space, so existing
/// subscriptions are mechanically preserved. Logical proof rather
/// than mechanical proof — surfaced to the reader via this docstring.
#[tokio::test]
#[traced_test]
async fn subscriptions_uninterrupted_across_safe_reload() {
    let (temp, initial) = write_and_load(BASE_TOML);
    // Iter-2 review P25: snapshot the PRE-reload config BEFORE
    // calling `handle.reload()`. The previous iter-1 attempt grabbed
    // `_rx.borrow()` after the reload, which trivially equalled the
    // post-reload candidate (passing vacuously). This snapshot is the
    // load-bearing reference for the diff comparison below.
    let pre_reload_snapshot = initial.clone();

    let initial_arc = Arc::new(initial);
    let (handle, _rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Mutate ONLY a hot-reload-safe knob (chirpstack.retry).
    let mutated = BASE_TOML.replace("retry = 30", "retry = 10");
    rewrite(&temp, &mutated);
    let outcome = handle.reload().await.expect("safe-knob reload must succeed");

    match outcome {
        ReloadOutcome::Changed {
            includes_topology_change,
            ..
        } => {
            assert!(
                !includes_topology_change,
                "hot-reload-safe knob change must NOT flag topology — \
                 includes_topology_change=true would mean Story 9-8 \
                 mutates the address space and disturbs subscriptions"
            );
        }
        ReloadOutcome::NoChange => {
            panic!("retry change must register as Changed, not NoChange");
        }
    }

    // Iter-2 P25: pass the PRE-reload snapshot as the "live" arg so
    // `log_topology_diff` actually exercises the before/after diff
    // path. The candidate is loaded from the post-mutation TOML.
    let candidate = AppConfig::from_path(&temp.path().to_string_lossy())
        .expect("post-reload candidate must load");
    let logged = opcgw::config_reload::log_topology_diff(&pre_reload_snapshot, &candidate);
    assert!(
        !logged,
        "hot-reload-safe knob change must not trigger topology_change_detected"
    );
    assert!(
        !logs_contain("event=\"topology_change_detected\""),
        "no topology log line expected on a safe-knob-only reload"
    );
}

// =============================================================================
// AC#4 — topology change detection seam for Story 9-8
// =============================================================================

/// Iter-1 review P23 — AC#4 verification (`topology_change_logs_seam_for_9_8`).
///
/// Adding a device under an existing application is a topology
/// change. The seam emits an info-level
/// `event="topology_change_detected"` log carrying `added_devices=1,
/// removed_devices=0, modified_devices=0` and `story_9_8_seam=true`.
#[tokio::test]
#[traced_test]
async fn topology_change_logs_seam_for_9_8() {
    let (_temp, before) = write_and_load(BASE_TOML);

    // Construct the after-config by appending a second device under
    // the same application.
    let with_extra_device = format!(
        "{}\n\n[[application.device]]\ndevice_name = \"Dev2\"\ndevice_id = \"550e8400-e29b-41d4-a716-446655440012\"\n\n[[application.device.read_metric]]\nmetric_name = \"humidity\"\nchirpstack_metric_name = \"hum\"\nmetric_type = \"Float\"\nmetric_unit = \"%\"\n",
        BASE_TOML
    );
    let (_temp_after, after) = write_and_load(&with_extra_device);

    let logged = opcgw::config_reload::log_topology_diff(&before, &after);
    assert!(logged, "device addition must trigger topology log");

    assert!(
        logs_contain("event=\"topology_change_detected\""),
        "log emission must use event= field per AC#4"
    );
    assert!(
        logs_contain("added_devices=1"),
        "expected added_devices=1 in log line"
    );
    assert!(
        logs_contain("removed_devices=0"),
        "expected removed_devices=0 in log line"
    );
    assert!(
        logs_contain("modified_devices=0"),
        "expected modified_devices=0 in log line"
    );
    assert!(
        logs_contain("story_9_8_seam=true"),
        "expected story_9_8_seam=true marker for Story 9-8 handoff"
    );
}

// =============================================================================
// Iter-1 review P24 — classifier coverage of [global], [command_validation],
// [logging] sections (D4 outcome). All three reclassified as restart-required
// in v1; tests pin the loud-rejection contract.
// =============================================================================

/// P24 — `[global]` knob change is rejected as restart-required.
/// Tracked in #114 for the future hot-reload upgrade.
#[tokio::test]
async fn reload_rejects_global_section_change() {
    let (temp, initial) = write_and_load(BASE_TOML);
    let initial_arc = Arc::new(initial);
    let (handle, mut rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Flip global.debug from true → false.
    let mutated = BASE_TOML.replace("debug = true", "debug = false");
    rewrite(&temp, &mutated);

    let err = handle
        .reload()
        .await
        .expect_err("global section change must be rejected");
    assert_eq!(
        err.reason(),
        "restart_required",
        "global section is restart-required in v1 (#114)"
    );
    assert_eq!(err.changed_knob(), Some("global.debug"));

    // Watch channel unchanged.
    let live = rx.borrow_and_update().clone();
    assert!(live.global.debug);
}

/// P24 — `[command_validation]` knob change is rejected as
/// restart-required. Tracked in #115.
#[tokio::test]
async fn reload_rejects_command_validation_section_change() {
    let with_cv = format!(
        "{}\n[command_validation]\ncache_ttl_secs = 3600\n",
        BASE_TOML
    );
    let (temp, initial) = write_and_load(&with_cv);
    let initial_arc = Arc::new(initial);
    let (handle, _rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Bump cache_ttl_secs.
    let mutated = with_cv.replace("cache_ttl_secs = 3600", "cache_ttl_secs = 7200");
    rewrite(&temp, &mutated);

    let err = handle
        .reload()
        .await
        .expect_err("command_validation change must be rejected");
    assert_eq!(
        err.reason(),
        "restart_required",
        "command_validation section is restart-required in v1 (#115)"
    );
    assert_eq!(
        err.changed_knob(),
        Some("command_validation.cache_ttl_secs")
    );
}

/// P24 — `[logging]` knob change is rejected as restart-required.
/// Tracked in #116 (log4rs / tracing-subscriber rolling reload).
#[tokio::test]
async fn reload_rejects_logging_section_change() {
    let with_logging = format!(
        "{}\n[logging]\nlevel = \"info\"\n",
        BASE_TOML
    );
    let (temp, initial) = write_and_load(&with_logging);
    let initial_arc = Arc::new(initial);
    let (handle, _rx) =
        ConfigReloadHandle::new(initial_arc.clone(), temp.path().to_path_buf());

    // Bump log level info → debug.
    let mutated = with_logging.replace(r#"level = "info""#, r#"level = "debug""#);
    rewrite(&temp, &mutated);

    let err = handle
        .reload()
        .await
        .expect_err("logging change must be rejected");
    assert_eq!(
        err.reason(),
        "restart_required",
        "logging section is restart-required in v1 (#116)"
    );
    assert_eq!(err.changed_knob(), Some("logging"));
}

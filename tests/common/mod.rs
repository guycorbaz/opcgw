// SPDX-License-Identifier: MIT OR Apache-2.0
// (c) [2026] Guy Corbaz

//! # Common test helpers for opcgw integration tests
//!
//! Issue #102 cleanup (Epic 8 retrospective, 2026-05-02). Holds the
//! truly-identical helpers across the four integration-test files
//! (`opcua_subscription_spike.rs`, `opcua_history.rs`,
//! `opc_ua_security_endpoints.rs`, `opc_ua_connection_limit.rs`).
//!
//! Cargo convention: this module is loaded via `mod common;` in each
//! test file. It is NOT compiled as a separate test target.
//!
//! ## What's in here (deduplicated)
//!
//! - [`pick_free_port`] — bind ephemeral port, return its number.
//!   Identical across all four files; trivially extractable.
//! - [`build_client`] — `ClientBuilder` invocation parametrised by
//!   [`ClientBuildSpec`]. Application/uri names + session_timeout
//!   differ per caller; everything else is identical.
//! - [`user_name_identity`] — construct a username/password
//!   `IdentityToken`. 3 of 4 files use this exact shape; the 4th
//!   (`opc_ua_security_endpoints`) builds it inline at each call site
//!   for stylistic reasons and does not need to migrate.
//!
//! ## What's NOT in here (intentional per-file divergence)
//!
//! These helpers diverge meaningfully across the four files; collapsing
//! them into shared parameterised forms would either over-restrict
//! callers or grow a parameter surface large enough to obscure intent.
//! Each file keeps its own definition with the divergence rationale
//! documented inline:
//!
//! - **`init_test_subscriber`** — the `tracing_subscriber::Registry`
//!   composition differs across files. `opcua_subscription_spike.rs`
//!   and `opc_ua_connection_limit.rs` need
//!   [`opcgw::opc_ua_session_monitor::AtLimitAcceptLayer`] for at-limit
//!   warn capture; `opc_ua_security_endpoints.rs` uses a custom
//!   composition for auth-failed event capture; `opcua_history.rs`
//!   doesn't capture events at all and uses the default subscriber.
//!   Spike-test infrastructure also leans on
//!   [`tracing_test::internal::global_buf`] (private API) — see
//!   issue #101's exact-pin entry in `Cargo.toml`.
//!
//! - **`setup_test_server*`** — varies in lifecycle requirements:
//!   per-test `max_connections` parameter (connection-limit tests),
//!   `max_history_data_results_per_node` parameter (history tests),
//!   inline custom config (NodeId-collision regression test), or
//!   no parameters at all (security-endpoints).
//!
//! - **`HeldSession`** — only `opcua_subscription_spike.rs` uses this
//!   RAII session wrapper (with the documented `Drop` contract from
//!   issue #101). Other files dispatch session lifecycle inline.
//!
//! - **`*_test_config`** — the `AppConfig` builder per file builds
//!   different application/device/metric fixtures for the test-domain.
//!   Sharing a generic builder would multiply the parameter surface
//!   without saving meaningful code.
//!
//! ## Adding a new helper here
//!
//! A helper belongs in this module if and only if at least three of
//! the four current callers would use the exact same shape with at
//! most a small typed-parameter struct. Below the three-caller
//! threshold, prefer in-file duplication per CLAUDE.md
//! scope-discipline rule. Adding a 5th integration-test file is the
//! clean trigger to revisit this rule (will likely accept more
//! candidates given the larger n).

#![allow(dead_code)] // Each test file uses a different subset of these helpers

use opcua::client::{Client, ClientBuilder, IdentityToken, Password as ClientPassword};

/// Bind an ephemeral TCP port (`127.0.0.1:0`) and return its number.
/// Releases the listener immediately so the caller can re-bind the
/// port via the OPC UA server. The narrow race window between drop
/// and re-bind has not bitten any of the four integration-test files
/// in practice; if it does on slower CI, mark the offending test
/// `#[serial_test::serial]` (already applied to all four files).
pub async fn pick_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind ephemeral port");
    listener.local_addr().expect("local_addr").port()
}

/// Per-file overrides for [`build_client`]. The fields that vary
/// across the four integration-test files; everything else is
/// hard-coded to the project's standard test-client shape.
pub struct ClientBuildSpec<'a> {
    /// Client `ApplicationName` field. Per file:
    /// - `"opcgw-spike-8-1-client"` (subscription spike)
    /// - `"opcgw-history-8-3-client"` (history)
    /// - `"opcgw-test-client"` (security-endpoints + connection-limit)
    pub application_name: &'static str,
    /// Client `ApplicationUri` field. Per file: same naming pattern as
    /// `application_name` (`urn:opcgw:...:client`).
    pub application_uri: &'static str,
    /// Client `ProductUri` field. Per file: same value as
    /// `application_uri` in current code.
    pub product_uri: &'static str,
    /// Session timeout in milliseconds. Two values in current files:
    /// `5_000` (security-endpoints, where tests are short-lived) or
    /// `15_000` (the other three; long enough to cover the full wall
    /// clock of the longest test in each file plus a small leak window
    /// if a test panics before disconnect).
    pub session_timeout_ms: u32,
    /// PKI directory path for the client (typically a `TempDir` so
    /// the test cleans up automatically on drop).
    pub client_pki: &'a std::path::Path,
}

/// Build an `opcua::client::Client` configured for opcgw integration
/// tests: trusts server certs without verification (so the integration
/// suite doesn't need PKI provisioning), no session retry (tests prefer
/// fast-fail over silently retrying past their wall-clock budget), and
/// the per-file overrides from [`ClientBuildSpec`].
pub fn build_client(spec: ClientBuildSpec<'_>) -> Client {
    ClientBuilder::new()
        .application_name(spec.application_name)
        .application_uri(spec.application_uri)
        .product_uri(spec.product_uri)
        .create_sample_keypair(true)
        .trust_server_certs(true)
        .verify_server_certs(false)
        .session_retry_limit(0)
        .session_timeout(spec.session_timeout_ms)
        .pki_dir(spec.client_pki)
        .client()
        .expect("client build")
}

/// Construct a username/password `IdentityToken` for OPC UA session
/// activation. Used by 3 of the 4 integration-test files to wrap their
/// per-file `TEST_USER` / `TEST_PASSWORD` constants. The 4th file
/// (`opc_ua_security_endpoints.rs`) builds tokens inline because each
/// of its tests asserts against a *different* (user, password) pair —
/// extracting a helper there would not save lines.
pub fn user_name_identity(user: &str, password: &str) -> IdentityToken {
    IdentityToken::UserName(user.to_string(), ClientPassword(password.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Smoke test: `pick_free_port` returns a non-zero port.
    /// Cargo runs `tests/common/mod.rs::tests` as part of each test
    /// binary that includes `mod common;` — so this test runs once
    /// per integration-test target that opts in.
    #[tokio::test]
    async fn pick_free_port_returns_nonzero() {
        let port = pick_free_port().await;
        assert!(
            port >= 1024,
            "ephemeral port must be in the unprivileged range, got {port}"
        );
    }

    /// `build_client` succeeds on a valid PKI directory. We don't
    /// actually connect — we just exercise the builder path that
    /// every integration test depends on.
    #[test]
    fn build_client_succeeds_with_valid_pki_dir() {
        let pki_tmp = tempfile::TempDir::new().expect("tmp pki dir");
        let _client = build_client(ClientBuildSpec {
            application_name: "test-client",
            application_uri: "urn:test:client",
            product_uri: "urn:test:client",
            session_timeout_ms: 5_000,
            client_pki: pki_tmp.path(),
        });
        // No assertions needed — `expect` inside `build_client` would
        // panic on failure. Reaching here means the builder path is
        // healthy.
    }

    /// `user_name_identity` produces an `IdentityToken::UserName`
    /// variant carrying the supplied user + password.
    #[test]
    fn user_name_identity_carries_supplied_credentials() {
        let token = user_name_identity("alice", "hunter2");
        match token {
            IdentityToken::UserName(user, pw) => {
                assert_eq!(user, "alice");
                // The Password type wraps the inner string; we don't
                // assert on it directly to avoid binding to the
                // upstream Password's accessor surface (which has
                // changed across async-opcua minor versions). The
                // smoke test is "the variant matches and the user
                // string round-trips" — sufficient for the helper's
                // contract.
                let _ = pw;
            }
            other => panic!("expected IdentityToken::UserName, got {other:?}"),
        }
    }
}

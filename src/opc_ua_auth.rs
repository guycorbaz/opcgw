// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Custom OPC UA authenticator (Story 7-2).
//!
//! Implements [`async_opcua::server::AuthManager`] so the gateway controls
//! its own credential check and emits an audit-trail `warn!` event on every
//! rejected authentication attempt. This satisfies NFR12 — "failed
//! authentication attempts logged with source IP" — via two-event correlation
//! with async-opcua's own connection-accept event (the `AuthManager` trait
//! does not receive the peer's `SocketAddr`; it is logged separately by
//! async-opcua's accept loop, and operators correlate by timestamp).
//!
//! Design notes:
//!
//! - Single-user model: the configured `[opcua].user_name` /
//!   `[opcua].user_password` pair is the only valid credential. Multi-user is
//!   explicitly Out of Scope for Story 7-2.
//! - The username submitted by the client is sanitised before logging so a
//!   malicious user cannot inject control characters or fake log events.
//! - The attempted password is **never** logged at any level.
//! - On success a `debug!` event is emitted (not `info!`) so steady-state
//!   reads do not log on every session establishment; the trail is still
//!   available with `OPCGW_LOG_LEVEL=debug`.

use std::sync::Arc;

use async_trait::async_trait;
use constant_time_eq::constant_time_eq;
use hmac::{Hmac, Mac};
use opcua::server::authenticator::{
    user_pass_security_policy_id, user_pass_security_policy_uri, AuthManager, Password, UserToken,
};
use opcua::server::ServerEndpoint;
use opcua::types::{Error, StatusCode, UAString, UserTokenPolicy, UserTokenType};
use sha2::Sha256;
use tracing::{debug, warn};

use crate::config::AppConfig;
use crate::utils::OPCUA_USER_TOKEN_ID;

type HmacSha256 = Hmac<Sha256>;

/// Compute `HMAC-SHA-256(key, data)` and return the 32-byte digest.
///
/// Used by [`OpcgwAuthManager`] to hash both configured and submitted
/// credentials before constant-time comparison. The HMAC keying makes
/// the digest non-deterministic across processes (so a digest cannot be
/// replayed against a different gateway instance) and the SHA-256 output
/// is fixed-length, eliminating the length oracle that a direct
/// content compare would leave open.
fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    // E6: `Hmac::<Sha256>::new_from_slice` accepts arbitrary key lengths
    // (it is the variable-key constructor — the fixed-key path is
    // `new`/`new_from_slice` on a `Mac`-trait impl with a `KeySize`
    // type-level constant, which we do not use). The `expect` is therefore
    // unreachable for SHA-256-keyed HMAC; calling it out here so future
    // readers don't grep for "InvalidLength" handling.
    let mut mac = HmacSha256::new_from_slice(key)
        .expect("Hmac::new_from_slice never fails for variable-key HMAC");
    mac.update(data);
    let result = mac.finalize().into_bytes();
    let mut out = [0u8; 32];
    out.copy_from_slice(&result);
    out
}

/// Custom authenticator for the gateway's single-user OPC UA model.
///
/// Holds **HMAC-SHA-256 digests** of the configured user/password keyed
/// by a per-process random secret rather than the plaintext credentials.
/// This closes the credential-length timing oracle that a direct
/// `constant_time_eq` of the plaintext would leave open — every digest
/// is exactly 32 bytes, so the comparison takes a content-independent
/// number of bit-operations regardless of the original credential
/// length. As a side benefit, the plaintext is no longer held in the
/// auth manager's heap (the configured plaintext still lives in the
/// `AppConfig` for one bootstrap call, but the auth manager itself
/// keeps only digests).
///
/// The HMAC key is randomly generated at process start via the OS RNG
/// and never persisted, so digests cannot be precomputed offline by an
/// attacker who learned the configured plaintext from a backup, a
/// memory dump from another process, or a cold boot. Each gateway
/// process has its own keying — replay attacks across instances do
/// not work either.
pub struct OpcgwAuthManager {
    /// HMAC-SHA-256(`hmac_key`, configured `user_name`).
    user_digest: [u8; 32],
    /// HMAC-SHA-256(`hmac_key`, configured `user_password`).
    pass_digest: [u8; 32],
    /// Per-process random secret, never persisted. Re-generated on every
    /// process start so digests are not stable across restarts.
    hmac_key: [u8; 32],
    /// Defence-in-depth: `false` if the configured user OR password was
    /// empty at construction time. `AppConfig::validate` rejects empty
    /// configured credentials, but if any future path bypasses
    /// validation this flag prevents the auth manager from accepting
    /// an empty submitted pair as matching empty configured digests.
    is_configured: bool,
}

impl OpcgwAuthManager {
    /// Build an authenticator from the application config.
    ///
    /// Reads the configured `user_name` / `user_password`, computes their
    /// HMAC-SHA-256 digests under a freshly-generated random key, and
    /// drops the plaintext on the way out. The resulting struct is
    /// `'static` and can be wrapped in an `Arc` for
    /// `ServerBuilder::with_authenticator`.
    ///
    /// # Panics
    ///
    /// Panics at process start if the OS RNG (`getrandom(2)` on Linux)
    /// is unavailable. This is intentional and the only safe behaviour
    /// for a security-critical credential check — running with a
    /// zero-byte HMAC key would silently produce identical digests
    /// across instances and across credentials of the same length, which
    /// is worse than a hard fail. Pathological causes (chroot without
    /// `/dev/urandom`, seccomp blocking the syscall, very early boot
    /// before the entropy pool is seeded) should produce a clear startup
    /// crash rather than a silent security weakening.
    pub fn new(config: &AppConfig) -> Self {
        let mut hmac_key = [0u8; 32];
        getrandom::getrandom(&mut hmac_key)
            .expect("system RNG must produce 32 bytes for HMAC key");
        let user_digest = hmac_sha256(&hmac_key, config.opcua.user_name.as_bytes());
        let pass_digest = hmac_sha256(&hmac_key, config.opcua.user_password.as_bytes());
        let is_configured =
            !config.opcua.user_name.is_empty() && !config.opcua.user_password.is_empty();
        Self {
            user_digest,
            pass_digest,
            hmac_key,
            is_configured,
        }
    }

    /// Wrap `self` in an `Arc<dyn AuthManager>` for
    /// `ServerBuilder::with_authenticator`.
    pub fn into_arc(self) -> Arc<dyn AuthManager> {
        Arc::new(self)
    }

    /// Sanitise an unauthenticated username for logging.
    ///
    /// The submitted username comes from a client that has not yet
    /// authenticated — it can contain control characters, ANSI escapes, or
    /// embedded newlines that would corrupt log readers or forge fake log
    /// lines. We truncate the **raw** input to 64 chars first (on character
    /// boundaries — `chars().take()` never splits a code point), then escape
    /// control chars + non-ASCII via `escape_default`. This avoids the
    /// edge case where `take(64)` lands inside a multi-char escape sequence
    /// like `\u{1f600}` (would otherwise yield a malformed `\u{1f6` tail).
    fn sanitise_user(raw: &str) -> String {
        let truncated: String = raw.chars().take(64).collect();
        truncated.escape_default().to_string()
    }
}

#[async_trait]
impl AuthManager for OpcgwAuthManager {
    /// Validate username + password against the single configured credential.
    ///
    /// On success: emits a `debug!` audit event and returns
    /// `UserToken(OPCUA_USER_TOKEN_ID)` so async-opcua maps the session to
    /// the same user-token id every endpoint advertises.
    ///
    /// On failure: emits a `warn!` audit event with the **sanitised**
    /// submitted username (never the password) and returns
    /// `BadUserAccessDenied`. NFR12 source IP is correlated with the
    /// preceding `info!("Accept new connection from {addr} (...)")` event
    /// emitted by async-opcua's accept loop — see `docs/security.md`.
    async fn authenticate_username_identity_token(
        &self,
        endpoint: &ServerEndpoint,
        username: &str,
        password: &Password,
    ) -> Result<UserToken, Error> {
        let sanitised_user = Self::sanitise_user(username);

        // P11 (defense-in-depth): hard-reject empty submitted credentials
        // or an empty configured baseline. `AppConfig::validate` already
        // rejects empty configured user/password at startup; the
        // `is_configured` flag pinned at construction time prevents a
        // future refactor or test path that bypasses validation from
        // accepting empty-on-empty matches.
        if username.is_empty() || password.get().is_empty() || !self.is_configured {
            warn!(
                event = "opcua_auth_failed",
                user = %sanitised_user,
                endpoint = %endpoint.path,
                reason = "empty_credential",
                "OPC UA authentication failed: empty submitted or configured credential"
            );
            return Err(Error::new(
                StatusCode::BadUserAccessDenied,
                "Authentication failed",
            ));
        }

        // P22 + N1: HMAC-SHA-256 each side under a per-process random key,
        // then constant-time compare the fixed-length digests. This is
        // fully constant-time across the whole comparison:
        //   - HMAC-SHA-256 itself is constant-time relative to message
        //     length (the SHA-256 compression runs the same number of
        //     rounds for any input < 2^64 bits).
        //   - The digests are always exactly 32 bytes, so
        //     `constant_time_eq` cannot short-circuit on length mismatch.
        //   - Both HMAC computations and both comparisons run
        //     unconditionally before the bitwise `&` combine, so neither
        //     side leaks via evaluation order.
        // The HMAC key is regenerated on every process start, so an
        // attacker cannot precompute candidate digests offline.
        let user_input_digest = hmac_sha256(&self.hmac_key, username.as_bytes());
        let pass_input_digest = hmac_sha256(&self.hmac_key, password.get().as_bytes());
        let user_match = constant_time_eq(&user_input_digest, &self.user_digest);
        let pass_match = constant_time_eq(&pass_input_digest, &self.pass_digest);
        if user_match & pass_match {
            debug!(
                event = "opcua_auth_succeeded",
                user = %sanitised_user,
                endpoint = %endpoint.path,
                "OPC UA authentication succeeded"
            );
            Ok(UserToken(OPCUA_USER_TOKEN_ID.to_string()))
        } else {
            warn!(
                event = "opcua_auth_failed",
                user = %sanitised_user,
                endpoint = %endpoint.path,
                "OPC UA authentication failed — see preceding 'Accept new connection from' info event for source IP"
            );
            Err(Error::new(
                StatusCode::BadUserAccessDenied,
                "Authentication failed",
            ))
        }
    }

    /// Advertise a single username/password policy on every endpoint that
    /// includes `OPCUA_USER_TOKEN_ID` in its `user_token_ids`.
    ///
    /// We delegate the policy-id and policy-uri choice to async-opcua's own
    /// helpers (`user_pass_security_policy_id` / `_uri`) so the policy
    /// matches whatever password-security policy async-opcua expects for
    /// the endpoint's security policy (None / Basic256 / Basic256Sha256 /
    /// …). Without this hook `supports_user_pass()` returns false and
    /// async-opcua refuses to route username/password tokens to our impl.
    fn user_token_policies(&self, endpoint: &ServerEndpoint) -> Vec<UserTokenPolicy> {
        if endpoint
            .user_token_ids
            .contains(OPCUA_USER_TOKEN_ID)
        {
            vec![UserTokenPolicy {
                policy_id: user_pass_security_policy_id(endpoint),
                token_type: UserTokenType::UserName,
                issued_token_type: UAString::null(),
                issuer_endpoint_url: UAString::null(),
                security_policy_uri: user_pass_security_policy_uri(endpoint),
            }]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_user_escapes_control_chars() {
        // Newlines and other control chars must be escaped — preventing log
        // injection by a malicious client passing
        // `evil\n[INJECTED]\nfake-event` as the username.
        let sanitised = OpcgwAuthManager::sanitise_user("evil\n[INJECTED]\nfake-event");
        assert!(
            !sanitised.contains('\n'),
            "sanitised username must not contain literal newlines, got {sanitised:?}"
        );
        assert!(
            sanitised.contains("\\n"),
            "sanitised username should preserve a printable escape, got {sanitised:?}"
        );
    }

    #[test]
    fn sanitise_user_truncates_at_64_chars_of_raw_input() {
        let long = "a".repeat(200);
        let sanitised = OpcgwAuthManager::sanitise_user(&long);
        // P5: cap is on raw chars (each ASCII `a` escapes to one char), so
        // the final length is exactly 64.
        assert_eq!(
            sanitised.chars().count(),
            64,
            "sanitised username of 200×'a' must be exactly 64 chars after truncation"
        );
    }

    #[test]
    fn sanitise_user_passes_through_simple_ascii() {
        let sanitised = OpcgwAuthManager::sanitise_user("opcua-user");
        assert_eq!(sanitised, "opcua-user");
    }

    #[test]
    fn sanitise_user_does_not_split_unicode_escape() {
        // P5: pre-truncating raw chars first means `take(64)` cannot land
        // mid-`\u{1f600}` escape sequence. Build a raw input where the
        // 64th char is a non-ASCII code point that escapes into a 10-char
        // sequence (`\u{1f600}`), and assert no truncated `\u{1f6` tail
        // appears in the output.
        let mut raw = "a".repeat(63);
        raw.push('\u{1f600}'); // 64th char — emoji
        raw.push_str("trailing"); // stripped because we truncate at 64
        let sanitised = OpcgwAuthManager::sanitise_user(&raw);
        // The emoji must be fully escaped (10 chars) — no malformed tail.
        assert!(
            sanitised.contains("\\u{1f600}"),
            "fully-escaped emoji must appear, got: {sanitised:?}"
        );
        // Trailing input was past the 64-char raw cap and must not appear.
        assert!(
            !sanitised.contains("trailing"),
            "input past raw 64-char boundary must be dropped, got: {sanitised:?}"
        );
    }

    // P13: prove the sanitiser → warn-event pipeline blocks log injection
    // without depending on the OPC UA wire format (which may strip control
    // characters before they reach `OpcgwAuthManager`). The integration
    // test in `tests/opc_ua_security_endpoints.rs` covers the full path;
    // this unit test pins the sanitiser-into-tracing bridge directly.
    #[test]
    #[tracing_test::traced_test]
    fn sanitise_user_in_warn_event_blocks_log_injection() {
        let evil = "evil\n[INJECTED]\nfake-event";
        let sanitised = OpcgwAuthManager::sanitise_user(evil);
        warn!(
            event = "opcua_auth_failed",
            user = %sanitised,
            endpoint = "/",
            "OPC UA authentication failed (test)"
        );
        assert!(
            !logs_contain("\n[INJECTED]\n"),
            "literal newline-bracketed [INJECTED] must not appear in captured logs"
        );
        assert!(
            logs_contain("opcua_auth_failed"),
            "warn event with structured field must appear in logs"
        );
    }

    // ---------------------------------------------------------------------
    // HMAC-keyed credential digest (N1 — closes the constant_time_eq
    // length oracle by hashing both sides to a fixed 32-byte digest).
    // ---------------------------------------------------------------------

    #[test]
    fn hmac_sha256_is_deterministic_under_same_key() {
        let key = [0x42u8; 32];
        let a = hmac_sha256(&key, b"opcua-user");
        let b = hmac_sha256(&key, b"opcua-user");
        assert_eq!(a, b, "same key + same input must produce same digest");
    }

    #[test]
    fn hmac_sha256_differs_for_different_inputs() {
        let key = [0x42u8; 32];
        let a = hmac_sha256(&key, b"opcua-user");
        let b = hmac_sha256(&key, b"opcua-other");
        assert_ne!(a, b, "different inputs must produce different digests");
    }

    #[test]
    fn hmac_sha256_differs_for_different_keys() {
        let a = hmac_sha256(&[0x42u8; 32], b"opcua-user");
        let b = hmac_sha256(&[0x77u8; 32], b"opcua-user");
        assert_ne!(
            a, b,
            "different per-process keys must produce different digests \
             (so digests cannot be replayed across gateway instances)"
        );
    }

    #[test]
    fn hmac_sha256_output_is_fixed_length_for_any_input() {
        // Closes the length oracle: regardless of input length the digest
        // is always 32 bytes, so `constant_time_eq` over digests can no
        // longer short-circuit.
        let key = [0u8; 32];
        let short = hmac_sha256(&key, b"a");
        let medium = hmac_sha256(&key, b"opcua-user");
        let long = hmac_sha256(&key, &[0x55u8; 4096]);
        assert_eq!(short.len(), 32);
        assert_eq!(medium.len(), 32);
        assert_eq!(long.len(), 32);
    }

    // ---------------------------------------------------------------------
    // OpcgwAuthManager construction (E2 / E3).
    //
    // These tests rely on construction via the public `new` entry point,
    // which is the only supported way to obtain an `OpcgwAuthManager`.
    // The struct's fields are private; tests reach them via dedicated
    // `#[cfg(test)]` accessors below to avoid widening visibility.
    // ---------------------------------------------------------------------

    impl OpcgwAuthManager {
        #[cfg(test)]
        pub(crate) fn user_digest_for_test(&self) -> [u8; 32] {
            self.user_digest
        }
        #[cfg(test)]
        pub(crate) fn pass_digest_for_test(&self) -> [u8; 32] {
            self.pass_digest
        }
        #[cfg(test)]
        pub(crate) fn hmac_key_for_test(&self) -> [u8; 32] {
            self.hmac_key
        }
        #[cfg(test)]
        pub(crate) fn is_configured_for_test(&self) -> bool {
            self.is_configured
        }
    }

    /// Build a minimal `AppConfig` for unit tests. Only the OPC UA
    /// `user_name` / `user_password` fields matter for these tests — the
    /// rest is filled with shape-correct defaults.
    #[cfg(test)]
    fn auth_test_config(user: &str, password: &str) -> AppConfig {
        use crate::config::{
            ChirpStackApplications, ChirpstackPollerConfig, CommandValidationConfig, Global,
            OpcUaConfig, StorageConfig,
        };
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
                host_port: Some(0),
                create_sample_keypair: true,
                certificate_path: "own/cert.der".to_string(),
                private_key_path: "private/private.pem".to_string(),
                trust_client_cert: false,
                check_cert_time: false,
                pki_dir: "./pki".to_string(),
                user_name: user.to_string(),
                user_password: password.to_string(),
                stale_threshold_seconds: Some(120),
            },
            application_list: vec![ChirpStackApplications {
                application_name: "App".to_string(),
                application_id: "00000000-0000-0000-0000-000000000001".to_string(),
                device_list: vec![],
            }],
            storage: StorageConfig::default(),
            command_validation: CommandValidationConfig::default(),
        }
    }

    #[test]
    fn new_generates_distinct_random_key_per_instance() {
        // E2: two `OpcgwAuthManager` built from the *same* config must
        // produce different HMAC keys (and therefore different user/pass
        // digests). If a future refactor accidentally hardcodes the key,
        // this test catches it.
        let cfg = auth_test_config("opcua-user", "secret");
        let a = OpcgwAuthManager::new(&cfg);
        let b = OpcgwAuthManager::new(&cfg);
        assert_ne!(
            a.hmac_key_for_test(),
            b.hmac_key_for_test(),
            "two instances must have distinct random keys"
        );
        assert_ne!(
            a.user_digest_for_test(),
            b.user_digest_for_test(),
            "two instances must have distinct user digests (different keys)"
        );
        assert_ne!(
            a.pass_digest_for_test(),
            b.pass_digest_for_test(),
            "two instances must have distinct pass digests (different keys)"
        );
    }

    #[test]
    fn new_sets_is_configured_true_when_both_credentials_non_empty() {
        let cfg = auth_test_config("opcua-user", "secret");
        let mgr = OpcgwAuthManager::new(&cfg);
        assert!(mgr.is_configured_for_test());
    }

    #[test]
    fn new_sets_is_configured_false_when_user_is_empty() {
        let cfg = auth_test_config("", "secret");
        let mgr = OpcgwAuthManager::new(&cfg);
        assert!(!mgr.is_configured_for_test());
    }

    #[test]
    fn new_sets_is_configured_false_when_password_is_empty() {
        let cfg = auth_test_config("opcua-user", "");
        let mgr = OpcgwAuthManager::new(&cfg);
        assert!(!mgr.is_configured_for_test());
    }

    #[tokio::test]
    async fn authenticate_rejects_when_is_configured_false() {
        // E3: defense-in-depth — even if an `OpcgwAuthManager` is somehow
        // constructed from an empty-credential config (bypassing
        // `AppConfig::validate`), the auth path must reject every
        // submitted credential pair, including a matching empty/empty
        // pair.
        use opcua::server::ServerEndpoint;
        let cfg = auth_test_config("", "");
        let mgr = OpcgwAuthManager::new(&cfg);
        assert!(!mgr.is_configured_for_test());

        let endpoint = ServerEndpoint::new_none("/", &[OPCUA_USER_TOKEN_ID.to_string()]);
        let pwd = Password::new(String::new());
        let result = mgr
            .authenticate_username_identity_token(&endpoint, "", &pwd)
            .await;
        assert!(
            result.is_err(),
            "empty/empty submitted pair must be rejected when not configured"
        );
    }
}

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
use opcua::server::authenticator::{
    user_pass_security_policy_id, user_pass_security_policy_uri, AuthManager, Password, UserToken,
};
use opcua::server::ServerEndpoint;
use opcua::types::{Error, StatusCode, UAString, UserTokenPolicy, UserTokenType};
use tracing::{debug, warn};

use crate::config::AppConfig;
use crate::utils::OPCUA_USER_TOKEN_ID;

/// Custom authenticator for the gateway's single-user OPC UA model.
///
/// Holds an owned copy of the configured user/password so the struct can be
/// `Arc`-shared across the async-opcua server tasks without lifetimes
/// leaking back into the configuration.
pub struct OpcgwAuthManager {
    user: String,
    pass: String,
}

impl OpcgwAuthManager {
    /// Build an authenticator from the application config.
    ///
    /// Clones the configured `user_name` / `user_password` so the resulting
    /// struct is `'static` and can be wrapped in an `Arc` for
    /// `ServerBuilder::with_authenticator`.
    pub fn new(config: &AppConfig) -> Self {
        Self {
            user: config.opcua.user_name.clone(),
            pass: config.opcua.user_password.clone(),
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
    /// lines. `escape_default()` turns every non-ASCII or control character
    /// into a printable escape sequence; the result is then truncated to
    /// 64 chars to bound log-line growth.
    fn sanitise_user(raw: &str) -> String {
        raw.escape_default().to_string().chars().take(64).collect()
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
        let endpoint_path = endpoint.path.clone();

        if username == self.user && password.get() == self.pass.as_str() {
            debug!(
                event = "opcua_auth_succeeded",
                user = %sanitised_user,
                endpoint = %endpoint_path,
                "OPC UA authentication succeeded"
            );
            Ok(UserToken(OPCUA_USER_TOKEN_ID.to_string()))
        } else {
            warn!(
                event = "opcua_auth_failed",
                user = %sanitised_user,
                endpoint = %endpoint_path,
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
    fn sanitise_user_truncates_at_64_chars() {
        let long = "a".repeat(200);
        let sanitised = OpcgwAuthManager::sanitise_user(&long);
        assert!(
            sanitised.chars().count() <= 64,
            "sanitised username must be truncated to ≤64 chars, got {} chars",
            sanitised.chars().count()
        );
    }

    #[test]
    fn sanitise_user_passes_through_simple_ascii() {
        let sanitised = OpcgwAuthManager::sanitise_user("opcua-user");
        assert_eq!(sanitised, "opcua-user");
    }
}

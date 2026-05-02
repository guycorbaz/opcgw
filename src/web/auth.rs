// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! HTTP Basic-auth middleware for the embedded Axum web server (Story 9-1).
//!
//! Implements an Axum tower-style middleware layer that:
//!
//! 1. Extracts the `Authorization` header from every request.
//! 2. Validates the `Basic <base64>` shape, base64-decodes the blob, and
//!    splits on the first `:` into `(user, password)`.
//! 3. Computes HMAC-SHA-256 digests of the submitted user + password
//!    under the same per-process random key the OPC UA `OpcgwAuthManager`
//!    uses (Story 7-2), and constant-time compares them against the
//!    digests of the configured credentials.
//! 4. On success, forwards the request to the inner handler. On any
//!    failure, returns `401 Unauthorized` + `WWW-Authenticate: Basic
//!    realm="..."` + emits a `event="web_auth_failed"` warn event.
//!
//! The 401 response constructor returns identical headers + body on
//! every failure mode (no early return on unknown user vs. wrong
//! password) so the wire response is constant-time. The audit event
//! differentiates the failure mode via the `reason` field for forensic
//! triage; the response to the client is identical across modes.
//!
//! # Reuse from Story 7-2
//!
//! - HMAC-SHA-256 keyed digest:
//!   [`crate::security_hmac::hmac_sha256`] — extracted from
//!   `OpcgwAuthManager` so both auth surfaces share one implementation
//!   (Phase-B carry-forward rule, `epics.md:782`).
//! - Per-process random HMAC key: borrowed from the live
//!   [`crate::opc_ua_auth::OpcgwAuthManager`] via its `hmac_key()`
//!   accessor — one key per process, two consumers.
//! - Username sanitisation for audit logging: same `escape_default` +
//!   64-char raw-truncation pattern as
//!   `OpcgwAuthManager::sanitise_user`. Inlined here as
//!   [`sanitise_user`] (3 lines, simpler than re-exporting through
//!   `OpcgwAuthManager`).
//!
//! # NFR12 source-IP — direct vs. correlated
//!
//! Story 7-2 / 7-3 needed two-event correlation because async-opcua's
//! `AuthManager` doesn't receive peer `SocketAddr`. **Axum has direct
//! access** via `ConnectInfo<SocketAddr>`. The audit-event payload
//! carries `source_ip` directly; no correlation step is needed for
//! the web surface.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use constant_time_eq::constant_time_eq;
use tracing::warn;

use crate::config::AppConfig;
use crate::opc_ua_auth::OpcgwAuthManager;
use crate::security_hmac::hmac_sha256;

/// Reasons for a `web_auth_failed` audit event. Mirrors the discrete
/// failure modes the middleware can hit; surfaced as the `reason` field
/// of the audit event for operator triage. The wire response to the
/// client is **identical** across all reasons (constant-time path) — the
/// reason exists only in the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthFailureReason {
    /// No `Authorization` header at all.
    Missing,
    /// Header present but doesn't start with `Basic ` (case-insensitive
    /// per RFC 7617 §2 — but in practice browsers send `Basic`).
    MalformedScheme,
    /// `Authorization: Basic <blob>` where `<blob>` is not valid base64.
    MalformedBase64,
    /// Decoded blob has no `:` separating user and password.
    MissingColon,
    /// Submitted username doesn't match the configured one.
    UserMismatch,
    /// Submitted password doesn't match the configured one (username
    /// did match — operator-visible signal that the user account is
    /// known but the password was wrong, useful for lockout heuristics
    /// in higher layers).
    PasswordMismatch,
}

impl AuthFailureReason {
    /// Stable string used as the `reason="..."` audit-event field.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::MalformedScheme => "malformed_scheme",
            Self::MalformedBase64 => "malformed_base64",
            Self::MissingColon => "missing_colon",
            Self::UserMismatch => "user_mismatch",
            Self::PasswordMismatch => "password_mismatch",
        }
    }
}

/// Per-process auth state for the web middleware.
///
/// Holds digests of the configured credentials + a borrowed reference
/// to the same per-process HMAC key the OPC UA auth surface uses.
/// Constructed once at startup via [`WebAuthState::from_opcua_auth`]
/// and shared across every request as `Arc<WebAuthState>`.
pub struct WebAuthState {
    /// HMAC-SHA-256(`hmac_key`, configured `user_name`).
    user_digest: [u8; 32],
    /// HMAC-SHA-256(`hmac_key`, configured `user_password`).
    pass_digest: [u8; 32],
    /// Per-process random secret. Owned (not borrowed) so the
    /// `WebAuthState` is `'static` and can be wrapped in `Arc` without
    /// lifetime annotations bleeding into Axum's handler signatures.
    /// Cloned from `OpcgwAuthManager::hmac_key()` at construction time.
    hmac_key: [u8; 32],
    /// Realm string for the `WWW-Authenticate` header. Sanitised at
    /// validation time (no `"` allowed, ≤ 64 chars).
    realm: String,
    /// Defence-in-depth: `false` if the configured user OR password was
    /// empty at construction time. Mirrors `OpcgwAuthManager::is_configured`.
    is_configured: bool,
}

impl WebAuthState {
    /// Build a `WebAuthState` from the application config (Story 9-1 AC#2 Shape 1).
    ///
    /// Reads `[opcua].user_name` / `[opcua].user_password` (the web
    /// surface shares credentials with the OPC UA surface — see AC#1
    /// in the spec), generates a fresh per-process HMAC key via
    /// `getrandom`, hashes both credentials, and drops the
    /// references. The resulting struct is `'static` and can be
    /// wrapped in `Arc` for the middleware.
    ///
    /// `realm` is the validated `[web].auth_realm` (or its default).
    ///
    /// # Panics
    ///
    /// Panics at startup if the OS RNG (`getrandom(2)` on Linux) is
    /// unavailable. Same rationale as
    /// [`OpcgwAuthManager::new`](crate::opc_ua_auth::OpcgwAuthManager::new):
    /// running with a zero-byte HMAC key would silently produce
    /// identical digests across instances and credentials of the same
    /// length, which is worse than a hard fail.
    pub fn new(config: &AppConfig, realm: String) -> Self {
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
            realm,
            is_configured,
        }
    }

    /// Build a `WebAuthState` borrowing the per-process HMAC key from
    /// an existing [`OpcgwAuthManager`] (Story 9-1 AC#2 Shape 2 —
    /// "OR (cleaner)" path; not currently used in production because
    /// the OPC UA auth manager is constructed inside `OpcUa::run`
    /// rather than `main.rs` and AC#6 forbids modifying `src/opc_ua.rs`
    /// to surface it. Kept for symmetry with the spec and for future
    /// stories that may refactor the construction order).
    ///
    /// `user` and `password` are the configured plaintext credentials.
    #[allow(dead_code)]
    pub fn from_opcua_auth(opcua: &OpcgwAuthManager, user: &str, password: &str, realm: String) -> Self {
        let hmac_key = *opcua.hmac_key();
        let user_digest = hmac_sha256(&hmac_key, user.as_bytes());
        let pass_digest = hmac_sha256(&hmac_key, password.as_bytes());
        let is_configured = !user.is_empty() && !password.is_empty();
        Self {
            user_digest,
            pass_digest,
            hmac_key,
            realm,
            is_configured,
        }
    }

    /// Build a fresh `WebAuthState` with explicit credentials (test
    /// helper, used by unit tests + integration tests in
    /// `tests/web_auth.rs`). Production code uses [`new`].
    ///
    /// `pub` (not `#[cfg(test)]`) because integration tests live in
    /// a separate crate and can only use the gateway's public API;
    /// the `#[cfg(test)]` guard would hide this helper from them.
    /// `#[allow(dead_code)]` because the production `bin` build does
    /// not call this function — only the lib's unit tests + the
    /// integration tests do. Using it from production code paths is
    /// a smell; the static analyzer cannot enforce that, but the
    /// doc-comment can.
    #[allow(dead_code)]
    pub fn new_with_fresh_key(user: &str, password: &str, realm: String) -> Self {
        let mut hmac_key = [0u8; 32];
        getrandom::getrandom(&mut hmac_key)
            .expect("system RNG must produce 32 bytes for HMAC key");
        let user_digest = hmac_sha256(&hmac_key, user.as_bytes());
        let pass_digest = hmac_sha256(&hmac_key, password.as_bytes());
        let is_configured = !user.is_empty() && !password.is_empty();
        Self {
            user_digest,
            pass_digest,
            hmac_key,
            realm,
            is_configured,
        }
    }

    /// Realm string for the `WWW-Authenticate` header. Currently
    /// unused outside `auth.rs` itself; exposed so future stories
    /// (e.g. 9-2 status dashboard) can read the active realm without
    /// re-routing through the config struct.
    #[allow(dead_code)]
    pub fn realm(&self) -> &str {
        &self.realm
    }
}

/// Sanitise an unauthenticated username for logging.
///
/// Mirrors [`crate::opc_ua_auth::OpcgwAuthManager::sanitise_user`] —
/// truncates the **raw** input to 64 chars first (on character
/// boundaries — `chars().take()` never splits a code point), then
/// escapes control chars + non-ASCII via `escape_default`. This
/// blocks log injection by a malicious client passing
/// `evil\n[INJECTED]\nfake-event` as the username.
///
/// The function is duplicated rather than re-exported because it is
/// 3 lines and per CLAUDE.md scope-discipline a tiny duplicate is
/// preferable to widening `OpcgwAuthManager`'s public surface beyond
/// what AC#7 explicitly allows.
fn sanitise_user(raw: &str) -> String {
    let truncated: String = raw.chars().take(64).collect();
    truncated.escape_default().to_string()
}

/// Build the canonical 401 Unauthorized response.
///
/// The same headers + body are emitted regardless of the failure
/// reason — the constant-time path. The realm string flows in via
/// the closure so the response is consistent with the configured
/// `[web].auth_realm`.
fn build_unauthorized_response(realm: &str) -> Response {
    let www_authenticate = format!("Basic realm=\"{}\"", realm);
    // realm has been validated at config load time (no `"` allowed,
    // ≤ 64 chars), so the `HeaderValue::from_str` cannot fail; the
    // `unwrap_or_else` covers the truly impossible case of an
    // already-broken header value (e.g. a future code path that
    // forgets to validate the realm) without panicking on the
    // request-handling hot path.
    let header_value = HeaderValue::from_str(&www_authenticate)
        .unwrap_or_else(|_| HeaderValue::from_static("Basic realm=\"opcgw\""));
    let mut resp = (StatusCode::UNAUTHORIZED, "Unauthorized\n").into_response();
    resp.headers_mut().insert(header::WWW_AUTHENTICATE, header_value);
    resp
}

/// Emit the `event="web_auth_failed"` audit event.
///
/// Uses `warn!`-level so it survives the NFR12 startup-warn check —
/// operators must run with log level ≥ `info` for the source-IP
/// correlation to be visible (same constraint as Story 7-2).
fn emit_auth_failure_event(
    source_ip: std::net::IpAddr,
    submitted_user_raw: &str,
    path: &str,
    reason: AuthFailureReason,
) {
    let sanitised_user = sanitise_user(submitted_user_raw);
    warn!(
        event = "web_auth_failed",
        source_ip = %source_ip,
        user = %sanitised_user,
        path = path,
        reason = reason.as_str(),
        "Web UI authentication failed"
    );
}

/// Decode the `Authorization: Basic <base64>` header into the (user,
/// password) pair. Returns the failure reason on any malformed shape.
///
/// Returns `Ok((user, pass))` on success; on any failure, returns
/// `Err((reason, raw_user))` where `raw_user` is whatever could be
/// extracted before the failure point (or empty). The caller emits
/// the audit event with the raw username so a malicious client
/// passing a malformed header still has its submitted username
/// surface in the audit log (sanitised — see [`sanitise_user`]).
fn decode_basic_auth_header(value: &HeaderValue) -> Result<(String, String), (AuthFailureReason, String)> {
    let raw = value
        .to_str()
        .map_err(|_| (AuthFailureReason::MalformedScheme, String::new()))?;

    // RFC 7617 §2: scheme is case-insensitive; strip a `Basic ` prefix
    // case-insensitively. In practice every browser and HTTP client
    // sends the canonical capitalisation; the case-fold is defense-
    // in-depth.
    let blob = if let Some(rest) = raw.strip_prefix("Basic ") {
        rest
    } else if raw.len() >= 6 && raw[..6].eq_ignore_ascii_case("Basic ") {
        &raw[6..]
    } else {
        return Err((AuthFailureReason::MalformedScheme, String::new()));
    };

    // Trim leading/trailing whitespace within the blob — some clients
    // pad with spaces. RFC 7617 doesn't strictly allow this but
    // tolerating it costs nothing here.
    let blob = blob.trim();

    let decoded = BASE64_STANDARD
        .decode(blob.as_bytes())
        .map_err(|_| (AuthFailureReason::MalformedBase64, String::new()))?;

    // RFC 7617 says the credentials are encoded with the charset of
    // the realm (typically UTF-8). Any non-UTF8 bytes count as
    // malformed.
    let decoded_str = String::from_utf8(decoded)
        .map_err(|_| (AuthFailureReason::MalformedBase64, String::new()))?;

    // Split on the first `:` only — subsequent colons go to the
    // password (RFC 7617 §2: passwords may contain colons; usernames
    // may not).
    if let Some((user, pass)) = decoded_str.split_once(':') {
        Ok((user.to_string(), pass.to_string()))
    } else {
        // Capture the would-be-user (the whole blob) for the audit
        // event — sanitised at emission time.
        Err((AuthFailureReason::MissingColon, decoded_str))
    }
}

/// Axum middleware enforcing Basic auth on every request.
///
/// Wires into the router via `axum::middleware::from_fn_with_state`.
/// On success, forwards to `next`. On any failure (missing header,
/// malformed shape, wrong credentials), emits the `web_auth_failed`
/// audit event and returns a constant-time 401 response.
pub async fn basic_auth_middleware(
    State(state): State<Arc<WebAuthState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let source_ip = addr.ip();

    // Defence-in-depth: refuse to authenticate anyone if the
    // `WebAuthState` was somehow built from empty credentials. The
    // `AppConfig::validate` startup check rejects empty configured
    // user/password, so this branch should be unreachable in
    // production — but if a future refactor or test path bypasses
    // validation, this prevents the "empty submitted matches empty
    // configured" trap.
    if !state.is_configured {
        emit_auth_failure_event(source_ip, "", &path, AuthFailureReason::Missing);
        return build_unauthorized_response(&state.realm);
    }

    let auth_header = match req.headers().get(header::AUTHORIZATION) {
        Some(h) => h,
        None => {
            emit_auth_failure_event(source_ip, "", &path, AuthFailureReason::Missing);
            return build_unauthorized_response(&state.realm);
        }
    };

    let (submitted_user, submitted_pass) = match decode_basic_auth_header(auth_header) {
        Ok(pair) => pair,
        Err((reason, raw_user)) => {
            emit_auth_failure_event(source_ip, &raw_user, &path, reason);
            return build_unauthorized_response(&state.realm);
        }
    };

    // Compute both digests unconditionally before any comparison.
    // The constant-time-comparison pattern from Story 7-2's
    // `OpcgwAuthManager::authenticate_username_identity_token` —
    // both HMAC computations and both `constant_time_eq` calls run
    // before the bitwise `&` combine, so the timing of the rejection
    // path doesn't depend on which field mismatched.
    let user_digest = hmac_sha256(&state.hmac_key, submitted_user.as_bytes());
    let pass_digest = hmac_sha256(&state.hmac_key, submitted_pass.as_bytes());
    let user_match = constant_time_eq(&user_digest, &state.user_digest);
    let pass_match = constant_time_eq(&pass_digest, &state.pass_digest);

    if user_match & pass_match {
        // Authenticated — forward to the inner handler. No audit
        // event on success (Story 7-2 emits a `debug!` event on
        // success, intentionally below the operator default; we
        // skip it on the web path to keep the request hot path
        // tracing-free).
        return next.run(req).await;
    }

    // Discriminate which field mismatched for the audit event. The
    // distinction is **deliberate** — the constant-time comparison
    // still happened for both digests, so the timing is identical
    // regardless of which mismatched. The audit event records which
    // one for forensic purposes; the response to the client is
    // identical (401 + same headers).
    let reason = if !user_match {
        AuthFailureReason::UserMismatch
    } else {
        AuthFailureReason::PasswordMismatch
    };
    emit_auth_failure_event(source_ip, &submitted_user, &path, reason);
    build_unauthorized_response(&state.realm)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Method, Request as HttpRequest};

    fn header_value(s: &str) -> HeaderValue {
        HeaderValue::from_str(s).expect("test header value")
    }

    /// Helper: build a `WebAuthState` for tests with a known
    /// (user, password). Using `new_with_fresh_key` so each test
    /// has an isolated key.
    fn test_state(user: &str, pass: &str) -> Arc<WebAuthState> {
        Arc::new(WebAuthState::new_with_fresh_key(
            user,
            pass,
            "opcgw-test".to_string(),
        ))
    }

    /// Helper: invoke the middleware and capture the resulting
    /// response (or `None` if the request was forwarded). The inner
    /// "next" handler returns 200 OK so a forwarded request is
    /// distinguishable from a 401 rejection.
    async fn invoke(
        state: Arc<WebAuthState>,
        auth_header: Option<&str>,
        path: &str,
    ) -> Response {
        let mut req = HttpRequest::builder()
            .method(Method::GET)
            .uri(path)
            .body(Body::empty())
            .expect("build test request");
        if let Some(value) = auth_header {
            req.headers_mut()
                .insert(header::AUTHORIZATION, header_value(value));
        }

        // Build a `Next` whose inner handler returns 200 OK with a
        // marker body so a forwarded request is observable in the
        // response. We can't easily build a `Next` directly — Axum
        // doesn't expose a public constructor — so we exercise the
        // middleware via a real router.
        let app = axum::Router::new()
            .route(
                "/",
                axum::routing::get(|| async { (StatusCode::OK, "ok") }),
            )
            .route(
                "/api/health",
                axum::routing::get(|| async { (StatusCode::OK, "ok") }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                basic_auth_middleware,
            ))
            .with_state(())
            .into_make_service_with_connect_info::<SocketAddr>();

        // Drive the request through tower::ServiceExt::oneshot.
        // `axum::Router` implements `tower::Service<Request<Body>>` so
        // the oneshot shortcut takes ownership of the router for a
        // single call without needing a `make_service` wrapper.
        use tower::ServiceExt;
        let service = axum::Router::new()
            .route(
                "/",
                axum::routing::get(|| async { (StatusCode::OK, "ok") }),
            )
            .route(
                "/api/health",
                axum::routing::get(|| async { (StatusCode::OK, "ok") }),
            )
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                basic_auth_middleware,
            ));
        // Inject the ConnectInfo extension so the middleware's
        // `ConnectInfo<SocketAddr>` extractor finds it without an
        // actual TCP listener.
        let test_addr: SocketAddr = "127.0.0.1:9999".parse().unwrap();
        req.extensions_mut().insert(ConnectInfo::<SocketAddr>(test_addr));
        let _ = app; // suppress warning on the unused make-service variant
        service
            .oneshot(req)
            .await
            .expect("call middleware")
    }

    /// Missing `Authorization` header → 401 + WWW-Authenticate.
    #[tokio::test]
    async fn middleware_rejects_missing_authorization_header() {
        let state = test_state("opcua-user", "secret");
        let resp = invoke(state, None, "/").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let www = resp
            .headers()
            .get(header::WWW_AUTHENTICATE)
            .expect("WWW-Authenticate header");
        assert!(
            www.to_str().unwrap().starts_with("Basic realm=\""),
            "got: {:?}",
            www
        );
    }

    /// `Authorization: Bearer xxx` (wrong scheme) → 401.
    #[tokio::test]
    async fn middleware_rejects_malformed_scheme() {
        let state = test_state("opcua-user", "secret");
        let resp = invoke(state, Some("Bearer xxx"), "/").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// `Authorization: Basic !!not-base64!!` → 401.
    #[tokio::test]
    async fn middleware_rejects_malformed_base64() {
        let state = test_state("opcua-user", "secret");
        let resp = invoke(state, Some("Basic !!not-base64!!"), "/").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// `Authorization: Basic <base64-without-colon>` → 401.
    #[tokio::test]
    async fn middleware_rejects_missing_colon() {
        let state = test_state("opcua-user", "secret");
        // base64("nocolon") = "bm9jb2xvbg=="
        let blob = BASE64_STANDARD.encode(b"nocolon");
        let header = format!("Basic {}", blob);
        let resp = invoke(state, Some(&header), "/").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Wrong username → 401. The `reason=user_mismatch` audit event
    /// fires (asserted indirectly via the constant-time path).
    #[tokio::test]
    async fn middleware_rejects_wrong_user() {
        let state = test_state("opcua-user", "secret");
        let blob = BASE64_STANDARD.encode(b"wrong-user:secret");
        let header = format!("Basic {}", blob);
        let resp = invoke(state, Some(&header), "/").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Wrong password → 401. The `reason=password_mismatch` audit
    /// event fires.
    #[tokio::test]
    async fn middleware_rejects_wrong_password() {
        let state = test_state("opcua-user", "secret");
        let blob = BASE64_STANDARD.encode(b"opcua-user:wrong-password");
        let header = format!("Basic {}", blob);
        let resp = invoke(state, Some(&header), "/").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// Correct credentials → 200 forwarded to the inner handler.
    #[tokio::test]
    async fn middleware_forwards_on_correct_credentials() {
        let state = test_state("opcua-user", "secret");
        let blob = BASE64_STANDARD.encode(b"opcua-user:secret");
        let header = format!("Basic {}", blob);
        let resp = invoke(state, Some(&header), "/").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Empty configured credentials (defence-in-depth) reject every
    /// submitted pair, including matching empty/empty. Mirrors the
    /// `OpcgwAuthManager::authenticate_rejects_when_is_configured_false`
    /// test from Story 7-2.
    #[tokio::test]
    async fn middleware_rejects_when_is_configured_false() {
        let state = test_state("", "");
        let blob = BASE64_STANDARD.encode(b":");
        let header = format!("Basic {}", blob);
        let resp = invoke(state, Some(&header), "/").await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }

    /// `sanitise_user` blocks log injection by escaping newlines and
    /// other control characters. Same contract as
    /// `OpcgwAuthManager::sanitise_user` — the duplicate
    /// implementation must produce equivalent output.
    #[test]
    fn sanitise_user_escapes_control_chars() {
        let sanitised = sanitise_user("evil\n[INJECTED]\nfake-event");
        assert!(
            !sanitised.contains('\n'),
            "sanitised username must not contain literal newlines, got {sanitised:?}"
        );
        assert!(
            sanitised.contains("\\n"),
            "sanitised username should preserve a printable escape, got {sanitised:?}"
        );
    }

    /// `AuthFailureReason::as_str` produces the stable wire/audit
    /// values. Pinned because the audit-event `reason` field is a
    /// public-facing contract.
    #[test]
    fn auth_failure_reason_as_str_stable() {
        assert_eq!(AuthFailureReason::Missing.as_str(), "missing");
        assert_eq!(AuthFailureReason::MalformedScheme.as_str(), "malformed_scheme");
        assert_eq!(AuthFailureReason::MalformedBase64.as_str(), "malformed_base64");
        assert_eq!(AuthFailureReason::MissingColon.as_str(), "missing_colon");
        assert_eq!(AuthFailureReason::UserMismatch.as_str(), "user_mismatch");
        assert_eq!(AuthFailureReason::PasswordMismatch.as_str(), "password_mismatch");
    }
}

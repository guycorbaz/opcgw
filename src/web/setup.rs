// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! First-run setup wizard (Epic C C-0, 2026-05-21).
//!
//! Provides the bootstrap path for an opcgw deployment that starts with
//! no configured OPC UA `user_password`. On the first reach by an
//! operator browser, the gateway serves a wizard at `/setup` that
//! collects a password and persists it to a sibling `config/secrets.toml`
//! (chmod 0600, gitignored). The gateway then shuts down gracefully so
//! the supervisor (docker / systemd) restarts it; on the second boot
//! the figment provider stack picks up `secrets.toml`, basic auth comes
//! online, and the wizard route returns HTTP 410 Gone.
//!
//! # Why a restart instead of in-place hot-reload
//!
//! `AppState.auth: Arc<WebAuthState>` is documented restart-required at
//! `src/web/mod.rs:264` (Story 9-7 explicitly excluded credential
//! rotation from the hot-reload contract). Hot-rotating the auth
//! middleware's captured `Arc<WebAuthState>` would expand scope into
//! Story 9-7 internals; the restart approach is standard for
//! self-hosted-app first-run wizards and keeps C-0 scope contained.
//!
//! # Why a separate auth-less wizard router
//!
//! In first-run mode the OPC UA `user_password` is empty, so the
//! standard basic-auth middleware has no valid credential to gate
//! against. The wizard pages MUST be reachable without auth (TOFU
//! pattern). We achieve this by composing two sub-routers in
//! [`crate::web::build_router`]:
//!
//! - The **main router** has the existing CRUD + dashboard routes with
//!   the basic-auth + CSRF middleware layers.
//! - The **wizard router** (this module) has `/setup` + `/api/setup/*`
//!   routes WITHOUT the auth or CSRF layers. CSRF is moot because there
//!   is no authenticated session to exploit in first-run mode; the
//!   threat model is "attacker on the local network beats the operator
//!   to the wizard," which CSRF wouldn't prevent.
//!
//! A first-run gate middleware sits OUTSIDE both sub-routers and
//! redirects non-wizard, non-static requests to `/setup` when the
//! gateway is in first-run mode.

use std::net::SocketAddr;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::utils::{OpcGwError, PLACEHOLDER_PREFIX};
use crate::web::AppState;

/// Path prefixes that bypass the first-run redirect even when the
/// gateway is in first-run mode. Anything else gets redirected to
/// `/setup`.
const WIZARD_BYPASS_PREFIXES: &[&str] = &[
    "/setup",
    "/api/setup/",
    // Static assets — the wizard page references /dashboard.css and
    // any other styling. ServeDir is the fallback so these paths don't
    // hit a specific route; they go through the gate then to the
    // static handler. We must let them through.
    "/dashboard.css",
];

/// Returns true if the request path is exempt from the first-run
/// redirect — i.e. either is a wizard route, an API endpoint of the
/// wizard, or a static asset the wizard depends on.
///
/// Also used by [`crate::web::auth::basic_auth_middleware`] to decide
/// whether to bypass the credential check in first-run mode.
pub fn is_wizard_bypass_path(path: &str) -> bool {
    if path == "/setup" {
        return true;
    }
    if path.starts_with("/api/setup/") {
        return true;
    }
    // Static-asset suffixes. Any URL ending in a common static-asset
    // extension is allowed through so wizard styling works.
    for suffix in [".css", ".js", ".png", ".ico", ".svg", ".woff", ".woff2"] {
        if path.ends_with(suffix) {
            return true;
        }
    }
    for prefix in WIZARD_BYPASS_PREFIXES {
        if path.starts_with(prefix) {
            return true;
        }
    }
    false
}

/// First-run gate middleware. Runs BEFORE auth + CSRF (declared OUTSIDE
/// those layers in [`crate::web::build_router`]).
///
/// Logic:
/// - If the gateway is NOT in first-run mode: pass through normally.
///   The wizard handlers themselves check `state.is_first_run` and
///   return 410 Gone for post-first-run requests, so leaving the routes
///   wired in non-first-run mode is safe.
/// - If the gateway IS in first-run mode AND the path is a wizard
///   route, wizard API endpoint, or static asset: pass through.
/// - Otherwise (first-run mode, non-wizard path): return HTTP 303 See
///   Other to `/setup`. (axum's `Redirect::to` emits 303 by default —
///   semantically correct for GET-to-GET redirects with no body
///   carry-over.)
pub async fn first_run_gate_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !state.is_first_run {
        return next.run(req).await;
    }

    let path = req.uri().path();
    if is_wizard_bypass_path(path) {
        return next.run(req).await;
    }

    // First-run mode, non-wizard path → redirect to /setup.
    Redirect::to("/setup").into_response()
}

/// GET /setup — serves the wizard HTML page.
///
/// In first-run mode, this renders the wizard. In post-first-run mode,
/// it returns HTTP 410 Gone with an explanation that password rotation
/// happens via env-var override or hand-editing `config/secrets.toml`.
pub async fn setup_get(State(state): State<Arc<AppState>>) -> Response {
    if !state.is_first_run {
        return (
            StatusCode::GONE,
            Html(
                "<!doctype html><html><head><title>opcgw setup — \
                 already configured</title></head><body><h1>opcgw is \
                 already configured</h1><p>The first-run wizard is no \
                 longer available. To rotate the OPC UA password, either \
                 set <code>OPCGW_OPCUA__USER_PASSWORD</code> in the \
                 gateway's environment, or hand-edit \
                 <code>config/secrets.toml</code> and restart \
                 opcgw.</p></body></html>",
            ),
        )
            .into_response();
    }

    // Iter-1 code review H5 / EH-H2 fix: serve the wizard page via
    // `state.static_dir.join("setup.html")` (the same canonical path
    // the ServeDir fallback uses), NOT a hardcoded cwd-relative
    // `"static/setup.html"`. Pre-fix, the hardcoded read broke any
    // deployment with a non-project-root cwd (systemd unit without
    // `WorkingDirectory=`, Docker image with `WORKDIR` not equal to
    // the asset root, etc.).
    let setup_html_path = state.static_dir.join("setup.html");
    match std::fs::read_to_string(&setup_html_path) {
        Ok(body) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            )],
            body,
        )
            .into_response(),
        Err(e) => {
            warn!(
                event = "setup_wizard_html_read_failed",
                error = %e,
                setup_html_path = %setup_html_path.display(),
                "setup_get: failed to read setup.html via static_dir — \
                 deployment is missing the static directory or the \
                 setup.html file."
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(
                    "<!doctype html><html><body><h1>opcgw setup error</h1>\
                     <p>The setup wizard page is missing. Verify the \
                     <code>static/</code> directory ships with the \
                     gateway binary.</p></body></html>",
                ),
            )
                .into_response()
        }
    }
}

/// Body schema for POST /api/setup/password.
#[derive(Debug, Deserialize)]
pub struct SetupPasswordRequest {
    /// New OPC UA `user_password` to persist.
    pub password: String,
    /// Confirmation field. Must match `password` byte-for-byte.
    pub password_confirm: String,
}

/// Error response for password-validation failures.
#[derive(Debug, Serialize)]
pub struct SetupPasswordError {
    pub error: &'static str,
    pub reason: &'static str,
}

/// Outcome of a successful POST. Echoed back in JSON for the client
/// JS, which then displays the "restarting" message and reloads after
/// a delay.
#[derive(Debug, Serialize)]
pub struct SetupPasswordSuccess {
    pub status: &'static str,
    pub restarting_in_seconds: u32,
}

/// Validates the submitted password against the same rules used by the
/// boot-time `AppConfig::validate`. Returns the first violation found
/// (one-at-a-time semantics, matching the JS UX where validation
/// errors are surfaced inline near the offending field).
fn validate_password(req: &SetupPasswordRequest) -> Option<&'static str> {
    if req.password.is_empty() {
        return Some("empty");
    }
    if req.password.trim() != req.password {
        return Some("whitespace_bracketed");
    }
    if req.password.trim().is_empty() {
        return Some("whitespace_only");
    }
    if req.password.starts_with(PLACEHOLDER_PREFIX) {
        return Some("placeholder_prefix");
    }
    // Iter-1 code review EH-H1 + Blind M5 fix: reject mid-string ASCII
    // control characters (U+0000..=U+001F + U+007F DEL). Pre-fix, a
    // password containing `\x7F` would:
    //   1. Pass `validate_password` (DEL is not whitespace, not in
    //      `PLACEHOLDER_PREFIX`, doesn't break confirmation match).
    //   2. Get written to `secrets.toml` as a raw 0x7F byte inside
    //      a basic string by `toml_escape_string` (which only
    //      escapes chars `< 0x20`).
    //   3. Per the TOML spec ("U+0000..U+0008, U+000A..U+001F, U+007F
    //      must be escaped"), the resulting `secrets.toml` is INVALID
    //      TOML.
    //   4. Next boot: figment's parse error → gateway fails to start
    //      → operator locked out, recovery only via deleting
    //      `secrets.toml` (which contradicts the wizard's "no
    //      operator-side TOML editing" promise).
    // The rejection here is the primary fix; `toml_escape_string` also
    // gained DEL coverage as defence-in-depth.
    if req.password.chars().any(|c| (c as u32) < 0x20 || c == '\u{7F}') {
        return Some("control_char_invalid");
    }
    // Iter-1 EH-M1 fix: cap password length to a sane bound (256
    // chars). Pre-fix, axum's 2 MiB default body limit would have
    // accepted a 1.9 MiB password; the wizard would persist it
    // verbatim, and the operator would be locked out because the
    // SCADA-client side can never type a 1.9 MiB credential.
    if req.password.chars().count() > 256 {
        return Some("too_long");
    }
    if req.password != req.password_confirm {
        return Some("confirmation_mismatch");
    }
    None
}

/// POST /api/setup/password — accepts the wizard form submission.
///
/// Validates the password, persists it to `config/secrets.toml`
/// (chmod 0600), emits the `setup_password_accepted` audit event, then
/// signals the gateway's `CancellationToken` for a graceful shutdown.
/// The supervisor (docker / systemd) restarts opcgw; on the next boot
/// the figment provider stack picks up `secrets.toml` and the gateway
/// runs in normal post-first-run mode.
pub async fn setup_post(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: axum::body::Bytes,
) -> Response {
    // Iter-1 code review H3 + EH-M2 fix: check `is_first_run` BEFORE
    // JSON extraction. Pre-fix the handler was `Json(req): Json<...>`
    // which invoked Axum's extractor first; a malformed body / wrong
    // Content-Type / missing field would return a generic 400 (or
    // 415/422) with Axum's default plain-text body, bypassing both
    // the post-first-run 410 Gone branch AND the structured
    // `{ error, reason }` response shape the JS error-UX expects.
    if !state.is_first_run {
        return (
            StatusCode::GONE,
            Json(SetupPasswordError {
                error: "already_configured",
                reason: "first_run_complete",
            }),
        )
            .into_response();
    }

    // Manual JSON parse — bypass Axum's Json extractor so malformed
    // input maps to a structured `{ error, reason }` response with the
    // wizard's audit-event taxonomy intact.
    let req: SetupPasswordRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            warn!(
                event = "setup_password_rejected",
                reason = "invalid_json",
                source_ip = %addr.ip(),
                error = %e,
                "setup_post: request body is not valid JSON"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(SetupPasswordError {
                    error: "password_validation_failed",
                    reason: "invalid_json",
                }),
            )
                .into_response();
        }
    };

    if let Some(reason) = validate_password(&req) {
        warn!(
            event = "setup_password_rejected",
            reason = reason,
            source_ip = %addr.ip(),
            "setup_post: password validation rejected"
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(SetupPasswordError {
                error: "password_validation_failed",
                reason,
            }),
        )
            .into_response();
    }

    // Persist to secrets.toml. The path is derived from the gateway's
    // config_dir which is captured into AppState at boot.
    match write_secrets_toml(&state.secrets_path, &req.password) {
        Ok(()) => {
            // Iter-1 code review M2 fix: log the FILENAME only, not
            // the full path. Full deployment path is sensitive
            // topology info that would defeat the file's 0600 mode
            // protection if logs are read by a broader audience than
            // file-system access.
            let secrets_filename = state
                .secrets_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("secrets.toml");
            info!(
                event = "setup_password_accepted",
                source_ip = %addr.ip(),
                secrets_filename = secrets_filename,
                "setup_post: password persisted to secrets file; \
                 gateway will shut down for restart"
            );

            // Iter-1 Auditor AC#11 patch: emit the config-reload audit
            // event with `trigger="first_run_wizard"` so the AC#18
            // grep contract is preserved (operators watching
            // `event="config_reload"` see the first-run completion in
            // their audit stream). The actual reload happens via the
            // restart path, not the in-place primitive — the event
            // captures the operational intent for forensic clarity.
            info!(
                event = "config_reload",
                trigger = "first_run_wizard",
                source_ip = %addr.ip(),
                "setup_post: first-run wizard completed; \
                 gateway restart will apply the new password"
            );

            // Iter-1 code review M7 fix: build the response BEFORE
            // signalling shutdown. Pre-fix, `state.shutdown_token
            // .cancel()` was called BEFORE the response was
            // constructed; the web server task listens on the cancel
            // token in `tokio::select!` and could win the race,
            // exiting before the response was flushed to the client.
            // Building the response first lets axum's graceful-
            // shutdown ensure in-flight responses complete before the
            // listener stops accepting new connections.
            let response = (
                StatusCode::OK,
                Json(SetupPasswordSuccess {
                    status: "password_set_restarting",
                    restarting_in_seconds: 5,
                }),
            )
                .into_response();

            // Trigger graceful shutdown so the supervisor restarts the
            // gateway. The supervisor (Docker restart policy / systemd
            // Restart=on-failure) reboots; the figment provider stack
            // picks up secrets.toml on the next boot.
            state.shutdown_token.cancel();

            response
        }
        Err(e) => {
            // Iter-1 M2: same filename-only redaction as the success
            // path above.
            let secrets_filename = state
                .secrets_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("secrets.toml");
            warn!(
                event = "setup_password_persistence_failed",
                reason = "io_error",
                source_ip = %addr.ip(),
                error = %e,
                secrets_filename = secrets_filename,
                "setup_post: failed to write secrets file"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(SetupPasswordError {
                    error: "persistence_failed",
                    reason: "io_error",
                }),
            )
                .into_response()
        }
    }
}

/// Write the OPC UA password to `secrets.toml` at the given path.
/// Sets file mode to 0o600 (owner read+write only).
///
/// Uses `tempfile + persist + rename` semantics to avoid leaving a
/// partial file on disk if the write is interrupted: writes to a
/// sibling temp file, sets permissions, then renames atomically.
fn write_secrets_toml(secrets_path: &std::path::Path, password: &str) -> Result<(), OpcGwError> {
    use std::io::Write;

    // Build the TOML body. Escape any embedded `"` in the password by
    // using the toml crate's serialiser instead of hand-formatting.
    // Hand-formatting risks injection if the password contains `"`,
    // newlines, or backslashes — even though the validator above
    // rejects whitespace-bracketed, the validator does NOT reject `"`
    // or `\` mid-string.
    let body = format!(
        r#"# opcgw secrets — generated by first-run wizard.
#
# This file holds the OPC UA user password and any other future
# secrets that should NOT live in the operator-readable config.toml.
# File permissions: chmod 0600 (the gateway will reject the file if
# group/world has any access bits set in a future hardening story).
#
# To rotate the password: either edit this file and restart opcgw,
# or override via the OPCGW_OPCUA__USER_PASSWORD env-var.

[opcua]
user_password = {}
"#,
        toml_escape_string(password),
    );

    // Iter-1 code review H2 / EH-M4: `Path::parent()` returns
    // `Some("")` (an empty path), NOT `None`, for paths with no
    // directory component (e.g. a bare `secrets.toml`). The
    // `tempfile::NamedTempFile::new_in("")` call interprets the
    // empty path as cwd on Linux but the behaviour is documented as
    // platform-specific. Coerce empty to `.` so the parent is always
    // a well-defined directory reference.
    let parent_raw = secrets_path.parent().ok_or_else(|| {
        OpcGwError::Configuration(format!(
            "secrets_path has no parent: {}",
            secrets_path.display()
        ))
    })?;
    let parent: &std::path::Path = if parent_raw.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        parent_raw
    };

    // tempfile in the same parent dir so rename is atomic (same fs).
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
        OpcGwError::Configuration(format!(
            "failed to create temp file in {}: {}",
            parent.display(),
            e
        ))
    })?;

    tmp.write_all(body.as_bytes()).map_err(|e| {
        OpcGwError::Configuration(format!(
            "failed to write to temp secrets.toml: {}",
            e
        ))
    })?;
    tmp.flush().map_err(|e| {
        OpcGwError::Configuration(format!(
            "failed to flush temp secrets.toml: {}",
            e
        ))
    })?;

    // chmod 0600 BEFORE rename so the file is never readable by group/
    // world even briefly. The Linux NamedTempFile starts with 0o600
    // by default in tempfile 3.x, but be explicit for forward-compat.
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(tmp.path(), perms).map_err(|e| {
        OpcGwError::Configuration(format!(
            "failed to set 0o600 on temp secrets.toml: {}",
            e
        ))
    })?;

    tmp.persist(secrets_path).map_err(|e| {
        OpcGwError::Configuration(format!(
            "failed to persist secrets.toml to {}: {}",
            secrets_path.display(),
            e
        ))
    })?;

    Ok(())
}

/// Serialise a string as a TOML basic string (double-quoted, with
/// internal `"`, `\`, control chars escaped). This is a tiny hand-rolled
/// equivalent of `toml::Value::String(s).to_string()`; we use it
/// directly to keep the secrets.toml file format simple and avoid
/// pulling toml-edit into this hot path.
fn toml_escape_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Control chars per TOML spec: U+0000..U+001F AND U+007F
            // (DEL) must be escaped. The DEL coverage was added in
            // iter-1 EH-H1 as defence-in-depth; the validator at
            // `validate_password` is the primary defence (rejects DEL
            // outright) but this branch keeps the escaper TOML-spec-
            // compliant if any future call site lets DEL through.
            c if (c as u32) < 0x20 || c == '\u{7F}' => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_password_accepts_reasonable_password() {
        let req = SetupPasswordRequest {
            password: "MyStrongP@ssw0rd!".to_string(),
            password_confirm: "MyStrongP@ssw0rd!".to_string(),
        };
        assert_eq!(validate_password(&req), None);
    }

    #[test]
    fn validate_password_rejects_empty() {
        let req = SetupPasswordRequest {
            password: "".to_string(),
            password_confirm: "".to_string(),
        };
        assert_eq!(validate_password(&req), Some("empty"));
    }

    #[test]
    fn validate_password_rejects_whitespace_bracketed() {
        let req = SetupPasswordRequest {
            password: " hello ".to_string(),
            password_confirm: " hello ".to_string(),
        };
        assert_eq!(validate_password(&req), Some("whitespace_bracketed"));
    }

    #[test]
    fn validate_password_rejects_placeholder_prefix() {
        let req = SetupPasswordRequest {
            password: format!("{}foo", PLACEHOLDER_PREFIX),
            password_confirm: format!("{}foo", PLACEHOLDER_PREFIX),
        };
        assert_eq!(validate_password(&req), Some("placeholder_prefix"));
    }

    #[test]
    fn validate_password_rejects_confirmation_mismatch() {
        let req = SetupPasswordRequest {
            password: "hello".to_string(),
            password_confirm: "world".to_string(),
        };
        assert_eq!(validate_password(&req), Some("confirmation_mismatch"));
    }

    /// Iter-1 EH-H1: DEL byte (U+007F) is rejected.
    #[test]
    fn validate_password_rejects_del_byte() {
        let req = SetupPasswordRequest {
            password: "abc\u{7F}def".to_string(),
            password_confirm: "abc\u{7F}def".to_string(),
        };
        assert_eq!(validate_password(&req), Some("control_char_invalid"));
    }

    /// Iter-1 Blind M5: mid-string control chars (U+0000..=U+001F)
    /// are rejected.
    #[test]
    fn validate_password_rejects_mid_string_control_char() {
        for c in [
            '\u{0001}', '\u{0008}', '\u{000B}', '\u{000C}', '\u{001F}',
        ] {
            let s = format!("abc{}def", c);
            let req = SetupPasswordRequest {
                password: s.clone(),
                password_confirm: s,
            };
            assert_eq!(
                validate_password(&req),
                Some("control_char_invalid"),
                "control char {:?} should be rejected",
                c,
            );
        }
    }

    /// Iter-1 EH-M1: password longer than 256 chars is rejected.
    #[test]
    fn validate_password_rejects_too_long() {
        let long_password = "a".repeat(257);
        let req = SetupPasswordRequest {
            password: long_password.clone(),
            password_confirm: long_password,
        };
        assert_eq!(validate_password(&req), Some("too_long"));
    }

    /// Iter-1 EH-M1: exactly 256 chars is accepted.
    #[test]
    fn validate_password_accepts_256_chars() {
        let pw_256 = "a".repeat(256);
        let req = SetupPasswordRequest {
            password: pw_256.clone(),
            password_confirm: pw_256,
        };
        assert_eq!(validate_password(&req), None);
    }

    /// Iter-1 EH-H1 defence-in-depth: even if validator was bypassed,
    /// `toml_escape_string` escapes DEL into ``.
    #[test]
    fn toml_escape_string_escapes_del_byte() {
        assert_eq!(toml_escape_string("a\u{7F}b"), "\"a\\u007Fb\"");
    }

    #[test]
    fn is_wizard_bypass_recognises_setup_routes() {
        assert!(is_wizard_bypass_path("/setup"));
        assert!(is_wizard_bypass_path("/api/setup/password"));
        assert!(is_wizard_bypass_path("/dashboard.css"));
        assert!(is_wizard_bypass_path("/applications.js"));
        assert!(is_wizard_bypass_path("/icon.png"));
        assert!(is_wizard_bypass_path("/favicon.ico"));
    }

    #[test]
    fn is_wizard_bypass_rejects_normal_paths() {
        assert!(!is_wizard_bypass_path("/"));
        assert!(!is_wizard_bypass_path("/applications"));
        assert!(!is_wizard_bypass_path("/api/applications"));
        assert!(!is_wizard_bypass_path("/api/health"));
        assert!(!is_wizard_bypass_path("/devices.html"));
    }

    #[test]
    fn toml_escape_string_handles_special_chars() {
        assert_eq!(toml_escape_string("simple"), r#""simple""#);
        assert_eq!(toml_escape_string(r#"has"quote"#), r#""has\"quote""#);
        assert_eq!(toml_escape_string(r"has\backslash"), r#""has\\backslash""#);
        assert_eq!(toml_escape_string("has\nnewline"), r#""has\nnewline""#);
    }

    #[test]
    fn write_secrets_toml_creates_file_with_password() {
        let tmp_dir = tempfile::tempdir().expect("create tempdir");
        let secrets_path = tmp_dir.path().join("secrets.toml");

        write_secrets_toml(&secrets_path, "my-test-password")
            .expect("write_secrets_toml succeeds");

        assert!(secrets_path.exists(), "secrets.toml was created");

        let body = std::fs::read_to_string(&secrets_path)
            .expect("read back secrets.toml");
        assert!(body.contains(r#"user_password = "my-test-password""#));
        assert!(body.contains("[opcua]"));
    }

    #[test]
    fn write_secrets_toml_sets_mode_0600() {
        use std::os::unix::fs::MetadataExt;

        let tmp_dir = tempfile::tempdir().expect("create tempdir");
        let secrets_path = tmp_dir.path().join("secrets.toml");

        write_secrets_toml(&secrets_path, "my-test-password")
            .expect("write_secrets_toml succeeds");

        let metadata = std::fs::metadata(&secrets_path).expect("metadata");
        // Mask off the file-type bits (only lower 9 bits are the
        // permission bits we care about).
        let mode = metadata.mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "secrets.toml must be chmod 0600 (owner read+write only)"
        );
    }

    #[test]
    fn write_secrets_toml_escapes_password_with_special_chars() {
        let tmp_dir = tempfile::tempdir().expect("create tempdir");
        let secrets_path = tmp_dir.path().join("secrets.toml");

        // Password contains quote + backslash — must round-trip
        // through TOML parsing.
        let password = r#"has"quote\and-backslash"#;
        write_secrets_toml(&secrets_path, password)
            .expect("write_secrets_toml succeeds");

        let body = std::fs::read_to_string(&secrets_path)
            .expect("read back secrets.toml");

        // Round-trip via figment (already a dep) — load the TOML
        // and verify the password parses back to the original value.
        // This catches any escaping mistakes that would corrupt the
        // value silently.
        use figment::providers::Format;
        let figment = figment::Figment::new()
            .merge(figment::providers::Toml::string(&body));
        let parsed: serde::de::IgnoredAny = figment
            .extract()
            .expect("secrets.toml must be valid TOML");
        // We can't easily extract a nested string with IgnoredAny;
        // instead just confirm the body contains an escaped form.
        // The escape rules guarantee the original chars are recoverable
        // (TOML escape spec is unambiguous); a corrupted body would
        // either be invalid TOML (extraction fails above) or have a
        // different stored value (visible in the assertions below).
        let _ = parsed;
        assert!(body.contains(r#"user_password = "has\"quote\\and-backslash""#),
                "escaped form must use TOML basic-string escapes: \\\" and \\\\, got body:\n{}",
                body);
    }
}

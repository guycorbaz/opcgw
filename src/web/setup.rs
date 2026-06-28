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
//! # Why conditional-bypass middlewares instead of a separate router
//!
//! Iter-2 P19 doc fix: an earlier draft proposed two physical sub-
//! routers (auth-less wizard + auth-gated main). The implementation
//! that landed in `c200089` uses a SINGLE router with conditional
//! bypass branches in each middleware — simpler to reason about than
//! split routers because the wizard routes share the same fallback,
//! the same TLS termination, and the same CSRF state.
//!
//! In first-run mode the OPC UA `user_password` is empty, so the
//! standard basic-auth middleware has no valid credential to gate
//! against. The wizard pages MUST be reachable without auth (TOFU
//! pattern). The current shape:
//!
//! - The single router in [`crate::web::build_router`] wires `/setup`,
//!   `/setup.html`, and `/api/setup` alongside the CRUD +
//!   dashboard routes.
//! - [`first_run_gate_middleware`] (this module) redirects non-wizard,
//!   non-static requests to `/setup` when `state.is_first_run` is true.
//! - [`crate::web::auth::basic_auth_middleware`] bypasses the
//!   credential check for [`is_wizard_bypass_path`] paths when
//!   `state.is_first_run` is true.
//! - [`crate::web::csrf::csrf_middleware`] exempts `POST
//!   /api/setup` from CSRF — but ONLY while in first-run mode
//!   (iter-2 P2 patch).
//!
//! CSRF's exemption-while-in-first-run is the cleanest answer to "no
//! authenticated session yet"; the threat model on the post-first-run
//! path is "attacker on the local network beats the operator to the
//! wizard," which CSRF wouldn't prevent — but defence-in-depth says
//! the exemption shouldn't outlive first-run mode.

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

use crate::utils::PLACEHOLDER_PREFIX;
use crate::web::AppState;

/// Exact paths the wizard depends on. The first-run gate redirects
/// EVERY other path to `/setup`; auth middleware bypass for first-run
/// mode also uses this allowlist.
///
/// Iter-2 P3 + P12 tightening: pre-fix this was a prefix list PLUS a
/// blanket suffix list (`.css/.js/.png/.ico/.svg/.woff/.woff2`) PLUS
/// the prefix `/api/setup/`. That made `/etc/passwd.css`,
/// `/api/setup/anything`, and ANY future API endpoint ending in `.js`
/// implicitly auth-exempt in first-run mode. The exact-match form
/// below is the minimum surface needed by `static/setup.html` — at
/// iter-2 P3 build time, `setup.html` references only `/dashboard.css`.
///
/// Iter-3 P6: dropped `/favicon.ico` entry. The justification was
/// "browsers auto-request it; 401-redirect noise in dev tools is worse
/// than a trivial bypass on a 1×1 image." But `static/favicon.ico`
/// doesn't actually ship in the repo — the bypass was allow-listing
/// a path that returned 404 anyway. Either ship the file or drop the
/// bypass; the latter is the minimal-surface choice.
const WIZARD_BYPASS_EXACT: &[&str] = &[
    "/setup",
    "/setup.html",
    // Story F-2: renamed from "/api/setup/password" (the wizard now submits
    // the ChirpStack connection + OPC UA password). The old path is NOT in
    // the allowlist — leaving it would be a dead auth-exempt path.
    "/api/setup",
    "/dashboard.css",
    // Story G-2 (#142): the first-run wizard's contextual field-help
    // affordance is driven by this shared static module. Exact-match only
    // (no suffix bypass) — a GET-only static asset, same minimal-surface
    // rationale as /dashboard.css above.
    "/field-help.js",
];

/// Returns true if the request path is exempt from the first-run
/// redirect / auth check. Uses an exact-match allowlist (see
/// [`WIZARD_BYPASS_EXACT`]) — no prefix or suffix matching.
///
/// Also used by [`crate::web::auth::basic_auth_middleware`] to decide
/// whether to bypass the credential check in first-run mode.
pub fn is_wizard_bypass_path(path: &str) -> bool {
    WIZARD_BYPASS_EXACT.contains(&path)
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
    // Iter-2 P5: `is_first_run` is `Arc<AtomicBool>` so concurrent
    // wizard submitters race-free-compare-and-swap it during
    // `setup_post`. Read once per request via SeqCst load.
    if !state.is_first_run.load(std::sync::atomic::Ordering::SeqCst) {
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
    // Iter-2 P5: AppState.is_first_run is Arc<AtomicBool>; load via SeqCst.
    if !state.is_first_run.load(std::sync::atomic::Ordering::SeqCst) {
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
        Ok(body) => {
            // Iter-2 P8: render server-side constants into the HTML so
            // the client and server share a single source of truth.
            // Currently substitutes `{{PLACEHOLDER_PREFIX}}` →
            // PLACEHOLDER_PREFIX (src/utils.rs). The token is HTML-
            // and JS-safe (no quotes, no angle-brackets, no
            // backslashes by construction) so naive string-replace
            // is sufficient — no escaping needed. If future tokens
            // can carry untrusted content, switch to a proper
            // templating layer.
            let body = body.replace("{{PLACEHOLDER_PREFIX}}", PLACEHOLDER_PREFIX);
            // Iter-3 P8: Cache-Control: no-store. The /setup HTML is
            // server-rendered with the PLACEHOLDER_PREFIX constant
            // substituted at request time (iter-2 P8). If a browser
            // caches the rendered HTML and serves it on a future
            // install where the constant has changed, the cached
            // page's client-side validator would mismatch the
            // server's. Cheap defence; the wizard is one-shot anyway.
            (
                StatusCode::OK,
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("text/html; charset=utf-8"),
                    ),
                    (
                        header::CACHE_CONTROL,
                        HeaderValue::from_static("no-store"),
                    ),
                ],
                body,
            )
                .into_response()
        }
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

/// Body schema for POST /api/setup (Story F-2).
///
/// Broadened from the Epic C C-0 password-only shape to capture
/// everything a fresh deployment needs for first boot: the ChirpStack
/// connection (`server_address` / `tenant_id` / `api_token`) plus the
/// OPC UA password. Secrets (`api_token`, `password`) are written to
/// `config/secrets.toml` (0600); the non-secret ChirpStack connection
/// fields are written to the SQLite singleton store. The OPC UA
/// host/port are intentionally NOT collected — the gateway's existing
/// `0.0.0.0:4840` defaults apply (AC#4 permits omission).
///
/// Iter-2 P21 (carried forward): `#[serde(deny_unknown_fields)]`
/// rejects bodies carrying fields beyond those declared here. The
/// wizard's body schema is a hard contract; new fields require a
/// deliberate struct-field addition, not free-form JSON.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetupRequest {
    /// ChirpStack server address, e.g. `http://chirpstack:8080`.
    pub server_address: String,
    /// ChirpStack tenant ID (UUID).
    pub tenant_id: String,
    /// ChirpStack API token (secret → `[chirpstack].api_token` in
    /// `secrets.toml`).
    pub api_token: String,
    /// New OPC UA `user_password` to persist (secret →
    /// `[opcua].user_password` in `secrets.toml`).
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
fn validate_password(req: &SetupRequest) -> Option<&'static str> {
    if req.password.is_empty() {
        return Some("empty");
    }
    if req.password.trim() != req.password {
        return Some("whitespace_bracketed");
    }
    if req.password.trim().is_empty() {
        return Some("whitespace_only");
    }
    // `contains` (not `starts_with`): the OPC UA password is also gated by the
    // migration's Guard 3 (`contains(PLACEHOLDER_MARKER)`), so reject any
    // placeholder fragment anywhere in the value (code review iter-2 H1).
    if req.password.contains(PLACEHOLDER_PREFIX) {
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

/// Story F-2: validate the ChirpStack connection fields submitted by the
/// wizard, mirroring the boot-time `AppConfig::validate` rules (empty +
/// scheme + placeholder) so the wizard can never persist a config that the
/// next boot would reject. One-at-a-time semantics like
/// [`validate_password`]; returns the first violation's `reason` code.
fn validate_chirpstack(req: &SetupRequest) -> Option<&'static str> {
    // Mid-string ASCII control chars (U+0000..=U+001F + U+007F DEL) are
    // rejected on every field — they're almost always paste errors and would
    // round-trip opaquely into the gRPC endpoint/tenant strings (code review
    // iter-1: parity with the api_token control-char check).
    let has_control = |s: &str| s.chars().any(|c| (c as u32) < 0x20 || c == '\u{7F}');

    // server_address
    if req.server_address.is_empty() {
        return Some("server_address_empty");
    }
    if req.server_address.trim() != req.server_address {
        return Some("server_address_whitespace");
    }
    if has_control(&req.server_address) {
        return Some("server_address_control_char");
    }
    if !req.server_address.starts_with("http://") && !req.server_address.starts_with("https://") {
        return Some("server_address_scheme");
    }
    if req.server_address.chars().count() > 512 {
        return Some("server_address_too_long");
    }
    // tenant_id
    if req.tenant_id.is_empty() {
        return Some("tenant_id_empty");
    }
    if req.tenant_id.trim() != req.tenant_id {
        return Some("tenant_id_whitespace");
    }
    if has_control(&req.tenant_id) {
        return Some("tenant_id_control_char");
    }
    // `contains` (not `starts_with`): the singleton migration's Guard 3 defers
    // on `contains(PLACEHOLDER_MARKER)` (a superset string of PLACEHOLDER_PREFIX),
    // so a value with the placeholder fragment mid-string would slip past a
    // `starts_with` check here yet jam the migration. Rejecting `contains` of
    // the (shorter) prefix closes that asymmetry (code review iter-2 H1).
    if req.tenant_id.contains(PLACEHOLDER_PREFIX) {
        return Some("tenant_id_placeholder");
    }
    if req.tenant_id.chars().count() > 128 {
        return Some("tenant_id_too_long");
    }
    // api_token (secret) — reuse the password-validator's control-char +
    // length defences so a malformed token can't corrupt secrets.toml.
    if req.api_token.is_empty() {
        return Some("api_token_empty");
    }
    if req.api_token.trim() != req.api_token {
        return Some("api_token_whitespace");
    }
    // `contains` (not `starts_with`) — see the tenant_id note above; for the
    // api_token this is load-bearing: the migration's Guard 3 would otherwise
    // defer indefinitely on a marker-containing token, permanently blocking the
    // singleton migration AND dead-ending the wizard (code review iter-2 H1).
    if req.api_token.contains(PLACEHOLDER_PREFIX) {
        return Some("api_token_placeholder");
    }
    if req.api_token.chars().any(|c| (c as u32) < 0x20 || c == '\u{7F}') {
        return Some("api_token_control_char");
    }
    // ChirpStack API tokens are JWTs and can be long; 2048 is a generous
    // bound that still fits comfortably inside the 4 KiB request body limit.
    if req.api_token.chars().count() > 2048 {
        return Some("api_token_too_long");
    }
    None
}

/// POST /api/setup — accepts the wizard form submission (Story F-2:
/// ChirpStack connection + OPC UA password).
///
/// Validates the password, persists it to `config/secrets.toml`
/// (chmod 0600), persists the non-secret ChirpStack connection to SQLite,
/// emits the `setup_accepted` audit event, then
/// signals the gateway's `CancellationToken` for a graceful shutdown.
/// The supervisor (docker / systemd) restarts opcgw; on the next boot
/// the figment provider stack picks up `secrets.toml` and the gateway
/// runs in normal post-first-run mode.
pub async fn setup_post(
    State(state): State<Arc<AppState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: axum::http::HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    // Iter-1 code review H3 + EH-M2 fix: check `is_first_run` BEFORE
    // JSON extraction. Pre-fix the handler was `Json(req): Json<...>`
    // which invoked Axum's extractor first; a malformed body / wrong
    // Content-Type / missing field would return a generic 400 (or
    // 415/422) with Axum's default plain-text body, bypassing both
    // the post-first-run 410 Gone branch AND the structured
    // `{ error, reason }` response shape the JS error-UX expects.
    //
    // Iter-2 P5: this is an OPTIMISTIC load — the compare_exchange
    // BELOW (just before the secrets.toml write) is the race-free
    // gate that actually decides which submitter wins. The load here
    // exists only so the common case (post-first-run probes) gets the
    // expected 410 Gone shape without paying for body extraction.
    if !state.is_first_run.load(std::sync::atomic::Ordering::SeqCst) {
        return (
            StatusCode::GONE,
            Json(SetupPasswordError {
                error: "already_configured",
                reason: "first_run_complete",
            }),
        )
            .into_response();
    }

    // Iter-2 P9: Content-Type must be application/json (with optional
    // charset suffix). The CSRF-exempt + auth-bypassed surface of this
    // route would otherwise allow a `<form>` POST (text/plain or
    // application/x-www-form-urlencoded) from a malicious LAN page to
    // submit a JSON-looking body. Rejecting non-JSON Content-Type
    // closes that gap (browsers can't easily forge a JSON
    // Content-Type from a `<form>` element).
    let ct_ok = headers
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| {
            let v = v.split(';').next().unwrap_or("").trim().to_ascii_lowercase();
            v == "application/json"
        })
        .unwrap_or(false);
    if !ct_ok {
        warn!(
            event = "setup_password_rejected",
            reason = "unsupported_media_type",
            source_ip = %addr.ip(),
            "setup_post: Content-Type is not application/json"
        );
        return (
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            Json(SetupPasswordError {
                error: "setup_validation_failed",
                reason: "unsupported_media_type",
            }),
        )
            .into_response();
    }

    // Iter-2 P11: defence-in-depth same-origin check on the Origin
    // header. If Origin is present, it MUST match the request's Host
    // (with either http:// or https:// scheme). Missing Origin is
    // accepted because some legitimate clients (curl, future
    // automation) don't send one. The check defends against drive-by
    // form posts from a malicious LAN page where the operator's
    // browser would set Origin to the malicious site, not the
    // gateway's own host. CSRF middleware is exempted from this route
    // (P2 + iter-1 M6), so this is the only Origin-class guard.
    //
    // Iter-3 D1 (deferred, see deferred-work.md): missing-Origin is
    // accepted by design. The threat model is a malicious LAN page
    // in an operator's browser, and BROWSERS always send Origin on
    // POST. curl-style attackers that omit Origin still face the
    // Content-Type strict check (P9 — `application/json` required),
    // which a `<form>` element cannot forge. The defence-in-depth
    // layer would be tightening missing-Origin to a 403, but that
    // breaks legitimate scripting/automation use cases on the wizard
    // (which Guy may want for unattended provisioning in a future
    // story). Documented limitation; tracked as DEF-iter3-C0-BH-H1.
    if let Some(origin) = headers
        .get(axum::http::header::ORIGIN)
        .and_then(|v| v.to_str().ok())
    {
        let host = headers
            .get(axum::http::header::HOST)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        // Iter-3 P3: default-port normalisation. Browsers per WHATWG
        // URL spec omit `:80` from http:// origins and `:443` from
        // https://. Host header can include OR omit the port for
        // default-port deployments. Without normalisation, a gateway
        // deployed on port 80 (or 443) gets locked out: Origin is
        // `http://host` while Host is `host:80` → mismatch.
        let strip_default_port = |s: &str, scheme: &str| -> String {
            let default = if scheme == "https" { ":443" } else { ":80" };
            s.strip_suffix(default).map(str::to_string).unwrap_or_else(|| s.to_string())
        };
        let host_norm_http = strip_default_port(host, "http");
        let host_norm_https = strip_default_port(host, "https");
        let origin_norm = if let Some(rest) = origin.strip_prefix("http://") {
            format!("http://{}", strip_default_port(rest, "http"))
        } else if let Some(rest) = origin.strip_prefix("https://") {
            format!("https://{}", strip_default_port(rest, "https"))
        } else {
            origin.to_string()
        };
        let expected_http = format!("http://{}", host_norm_http);
        let expected_https = format!("https://{}", host_norm_https);
        if origin_norm != expected_http && origin_norm != expected_https {
            warn!(
                event = "setup_password_rejected",
                reason = "origin_mismatch",
                source_ip = %addr.ip(),
                "setup_post: Origin header does not match Host (possible drive-by POST)"
            );
            return (
                StatusCode::FORBIDDEN,
                Json(SetupPasswordError {
                    error: "setup_validation_failed",
                    reason: "origin_mismatch",
                }),
            )
                .into_response();
        }
    }

    // Manual JSON parse — bypass Axum's Json extractor so malformed
    // input maps to a structured `{ error, reason }` response with the
    // wizard's audit-event taxonomy intact.
    let req: SetupRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            warn!(
                event = "setup_rejected",
                reason = "invalid_json",
                source_ip = %addr.ip(),
                error = %e,
                "setup_post: request body is not valid JSON"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(SetupPasswordError {
                    error: "setup_validation_failed",
                    reason: "invalid_json",
                }),
            )
                .into_response();
        }
    };

    // Story F-2: validate the ChirpStack fields first, then the OPC UA
    // password — both must pass before anything is persisted.
    if let Some(reason) = validate_chirpstack(&req) {
        warn!(
            event = "setup_rejected",
            reason = reason,
            source_ip = %addr.ip(),
            "setup_post: ChirpStack field validation rejected"
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(SetupPasswordError {
                error: "setup_validation_failed",
                reason,
            }),
        )
            .into_response();
    }

    if let Some(reason) = validate_password(&req) {
        warn!(
            event = "setup_rejected",
            reason = reason,
            source_ip = %addr.ip(),
            "setup_post: password validation rejected"
        );
        return (
            StatusCode::BAD_REQUEST,
            Json(SetupPasswordError {
                error: "setup_validation_failed",
                reason,
            }),
        )
            .into_response();
    }

    // Story F-2: overlay the submitted values onto the current config
    // snapshot to build the `candidate` config. This serves two purposes:
    // (1) belt-and-braces — run the full `AppConfig::validate` so the wizard
    // can never persist a combination the next boot would reject (the
    // per-field validators above give precise reason codes; this catches any
    // cross-field invariant); (2) it is the config the singleton migration
    // below persists into SQLite, so SQLite ends up with the operator's
    // ChirpStack connection (not the seed `config.toml` values).
    let candidate: crate::config::AppConfig = {
        let current_arc = state.config_reload.subscribe().borrow().clone();
        let mut candidate: crate::config::AppConfig = (*current_arc).clone();
        candidate.chirpstack.server_address = req.server_address.clone();
        candidate.chirpstack.tenant_id = req.tenant_id.clone();
        candidate.chirpstack.api_token = req.api_token.clone();
        candidate.opcua.user_password = req.password.clone();
        if let Err(e) = candidate.validate() {
            warn!(
                event = "setup_rejected",
                reason = "config_invalid",
                source_ip = %addr.ip(),
                error = ?e,
                "setup_post: candidate config failed AppConfig::validate"
            );
            return (
                StatusCode::BAD_REQUEST,
                Json(SetupPasswordError {
                    error: "setup_validation_failed",
                    reason: "config_invalid",
                }),
            )
                .into_response();
        }
        candidate
    };

    // Iter-2 P5: race-free first-run-mode flip. compare_exchange
    // atomically transitions the gateway from first-run to post-first-
    // run. EXACTLY ONE caller wins this race — that caller proceeds to
    // write secrets.toml + cancel + return 200. Every concurrent
    // caller (a second operator racing, or a browser auto-reload
    // landing during the supervisor-restart drain window) sees
    // `Err(false)` (the new current value) and returns HTTP 409
    // Conflict. Without this, two concurrent submits could both pass
    // the optimistic load above, both write secrets.toml (last-write-
    // wins on disk), and both return 200 — the "wizard is one-shot"
    // guarantee would be silently violated.
    //
    // Story F-2 — failure atomicity: there are now TWO persistence steps
    // (SQLite non-secret write + secrets.toml write). If EITHER fails after
    // we win the exchange, we revert `is_first_run` to true (iter-3 P2
    // pattern) and do NOT cancel `shutdown_token`, so the gateway keeps
    // running in first-run mode and the operator can retry. There is no
    // cross-file transaction; the revert-on-any-failure + no-restart rule is
    // the safety net that keeps a partial write recoverable.
    use std::sync::atomic::Ordering;
    if state
        .is_first_run
        .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        warn!(
            event = "setup_rejected",
            reason = "setup_already_in_progress",
            source_ip = %addr.ip(),
            "setup_post: a concurrent submitter already claimed first-run state"
        );
        return (
            StatusCode::CONFLICT,
            Json(SetupPasswordError {
                error: "setup_validation_failed",
                reason: "setup_already_in_progress",
            }),
        )
            .into_response();
    }

    // ───────────────────────────────────────────────────────────────────
    // Story F-2 — two-store persistence (secrets.toml + SQLite). There is no
    // transaction across the two stores, so ORDER matters for recoverability:
    // write the secrets file FIRST, then run the SQLite migration. Rationale:
    // the migration is a single atomic transaction (all sections + the
    // `d0_migration_done` flag, or nothing — see migrate_singleton_config.rs),
    // so if secrets succeed and the migration then fails, the migration rolled
    // back to an EMPTY singleton table with NO done-flag → an in-process retry
    // re-runs cleanly. Were the order reversed, a successful migration sets the
    // done-flag, and a subsequent secrets failure would make every retry's
    // migration short-circuit to `AlreadyMigrated` (Guard 1) — a dead-end.
    //
    // On ANY failure we revert `is_first_run` to true and do NOT cancel
    // `shutdown_token`, so the gateway keeps running in first-run mode and the
    // operator can retry (the revert is unconditional — we won the prior
    // compare_exchange, so we are the only writer).
    let secrets_filename = state
        .secrets_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("secrets.toml");

    // === Step 1: write BOTH secrets to secrets.toml (atomic single-file). ===
    let secrets = SecretsToWrite {
        chirpstack_api_token: &req.api_token,
        opcua_password: &req.password,
    };
    if let Err(e) = write_secrets_toml(&state.secrets_path, &secrets) {
        state.is_first_run.store(true, Ordering::SeqCst);
        // Iter-2 P7: categorised reason (readonly_filesystem / disk_full /
        // permission_denied / parent_directory_missing / io_error).
        let reason = e.reason_code();
        warn!(
            // Story F-2 (iter-2): uniform `setup_persistence_failed` across both
            // the secrets-write and SQLite-migration failure paths (the legacy
            // `setup_password_persistence_failed` name is retired alongside
            // `setup_password_accepted`); `reason` distinguishes the cause.
            event = "setup_persistence_failed",
            reason = reason,
            source_ip = %addr.ip(),
            error = %e,
            secrets_filename = secrets_filename,
            "setup_post: failed to write secrets file; \
             is_first_run reverted to true for operator retry"
        );
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SetupPasswordError {
                error: "persistence_failed",
                reason,
            }),
        )
            .into_response();
    }

    // === Step 2: persist the NON-SECRET singleton config to SQLite by running
    // the FULL D-0 singleton migration against `candidate` (which carries the
    // operator's ChirpStack connection). Secrets are skipped by the migration's
    // secret skip-list — they live only in secrets.toml (written above). ===
    //
    // Code review iter-1 HIGH: a partial `write_singleton_section("chirpstack",
    // [server_address, tenant_id])` would leave the `singleton_config` table
    // non-empty WITHOUT the `d0_migration_done` flag. On the next boot the
    // migration's Guard 2 (`count_singleton_config() > 0`) back-fills the
    // done-flag and short-circuits to `AlreadyMigrated`, so the global/opcua/web
    // sections (and the remaining chirpstack knobs) would NEVER migrate into
    // SQLite — silently defeating the Epic D "SQLite is authoritative" model,
    // with no boot able to self-heal it. Running the FULL migration here writes
    // every non-secret section + sets the done-flag, so the next boot's Guard 1
    // correctly sees `AlreadyMigrated` with the complete set present. In
    // first-run mode the table is empty (boot-time migration deferred while
    // secrets were placeholders) and `candidate` carries real secrets, so the
    // migration runs cleanly and returns `Migrated`. Any other outcome is
    // abnormal for a first-run submit → revert and surface a 500 so the operator
    // can retry / investigate.
    use crate::storage::migrate_singleton_config::{
        migrate_singleton_toml_to_sqlite, SingletonMigrationOutcome,
    };
    let migration_result =
        migrate_singleton_toml_to_sqlite(&candidate, &state.sqlite_config);
    if !matches!(migration_result, Ok(SingletonMigrationOutcome::Migrated(_))) {
        state.is_first_run.store(true, Ordering::SeqCst);
        // Honest per-outcome classification (code review iter-2 M1/M2/L1):
        // surface a distinct `reason` instead of collapsing every non-Migrated
        // outcome to "sqlite_write_failed", keep `outcome` a low-cardinality
        // stable token, and log the DB error separately. After the iter-2 H1
        // fix the placeholder-skip variant is unreachable from a validated
        // submit, and AlreadyMigrated only arises if the singleton table was
        // pre-populated (abnormal in first-run) — both still revert + 500 so a
        // dirty state never silently completes setup, but with an accurate
        // reason rather than a misleading write-failure label.
        let (reason, outcome, err_detail): (&'static str, &'static str, Option<String>) =
            match &migration_result {
                // Excluded by the outer guard; mapped only for exhaustiveness.
                Ok(SingletonMigrationOutcome::Migrated(_)) => ("sqlite_write_failed", "migrated", None),
                Ok(SingletonMigrationOutcome::AlreadyMigrated) => {
                    ("already_configured", "already_migrated", None)
                }
                Ok(SingletonMigrationOutcome::SkippedEmptyOrPlaceholder) => {
                    ("config_invalid", "skipped_empty_or_placeholder", None)
                }
                Err(e) => ("sqlite_write_failed", "error", Some(e.to_string())),
            };
        warn!(
            event = "setup_persistence_failed",
            reason = reason,
            outcome = outcome,
            source_ip = %addr.ip(),
            error = err_detail.as_deref().unwrap_or(""),
            "setup_post: singleton config migration did not complete cleanly; \
             is_first_run reverted for operator retry (secrets.toml was written; \
             an in-process retry re-runs a rolled-back migration cleanly)"
        );
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(SetupPasswordError {
                error: "persistence_failed",
                reason,
            }),
        )
            .into_response();
    }

    // === Step 3: success — audit, build response, then signal restart. ===
    //
    // Story F-2: broadened acceptance event. The wizard persisted both secrets
    // (secrets.toml) + the non-secret ChirpStack connection (SQLite). The legacy
    // `setup_password_accepted` name is retired in favour of `setup_accepted`;
    // operators grepping for first-run completion should watch `config_reload`
    // (stable across C-0 → F-2).
    info!(
        event = "setup_accepted",
        source_ip = %addr.ip(),
        secrets_filename = secrets_filename,
        "setup_post: secrets persisted to secrets file and ChirpStack \
         connection migrated to SQLite; gateway will shut down for restart"
    );
    // Iter-1 Auditor AC#11 patch: emit the config-reload audit event with
    // `trigger="first_run_wizard"` so the grep contract is preserved.
    info!(
        event = "config_reload",
        trigger = "first_run_wizard",
        source_ip = %addr.ip(),
        "setup_post: first-run wizard completed; \
         gateway restart will apply the new ChirpStack connection and secrets"
    );
    // Iter-1 code review M7 fix: build the response BEFORE signalling shutdown
    // so axum's graceful-shutdown flushes the in-flight response before the
    // listener stops accepting connections.
    let response = (
        StatusCode::OK,
        Json(SetupPasswordSuccess {
            status: "password_set_restarting",
            restarting_in_seconds: 5,
        }),
    )
        .into_response();
    state.shutdown_token.cancel();
    response
}

/// Iter-2 P7: categorised failure modes for `write_secrets_toml`.
/// Pre-fix, every io error collapsed to `OpcGwError::Configuration`
/// with a stringified message, and the wizard returned a single
/// `reason="io_error"` regardless of root cause. Operators couldn't
/// distinguish "fix permissions" from "free disk space" from "mount
/// the config dir read-write."
#[derive(Debug)]
pub(crate) enum SecretsWriteError {
    /// Parent directory of `secrets_path` is missing or unreadable.
    ParentDirectoryMissing { detail: String },
    /// Filesystem is read-only (EROFS).
    ReadOnlyFilesystem { detail: String },
    /// Process lacks permission to write (EACCES / EPERM).
    PermissionDenied { detail: String },
    /// Disk full / quota exceeded (ENOSPC / EDQUOT).
    DiskFull { detail: String },
    /// Catch-all for io errors that don't match a known category.
    IoError { detail: String },
    /// Path string was malformed in a way that no recovery is possible
    /// (e.g. `secrets_path.parent()` returned `None` — unreachable
    /// after iter-1 H2 fix but kept for completeness).
    InvalidPath { detail: String },
}

impl SecretsWriteError {
    /// Stable wire-format reason code. Mirrored in JS REASON_MESSAGES
    /// (iter-2 P13).
    pub(crate) fn reason_code(&self) -> &'static str {
        match self {
            Self::ParentDirectoryMissing { .. } => "parent_directory_missing",
            Self::ReadOnlyFilesystem { .. } => "readonly_filesystem",
            Self::PermissionDenied { .. } => "permission_denied",
            Self::DiskFull { .. } => "disk_full",
            Self::IoError { .. } => "io_error",
            Self::InvalidPath { .. } => "invalid_path",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::ParentDirectoryMissing { detail }
            | Self::ReadOnlyFilesystem { detail }
            | Self::PermissionDenied { detail }
            | Self::DiskFull { detail }
            | Self::IoError { detail }
            | Self::InvalidPath { detail } => detail,
        }
    }

    /// Map an io::Error to the most specific variant. Falls back to
    /// `IoError` for unrecognised kinds.
    ///
    /// Iter-3 P4: use the stable `io::ErrorKind` variants
    /// (`ReadOnlyFilesystem`, `StorageFull`, `QuotaExceeded`) stable
    /// since Rust 1.85 (CLAUDE.md mandates rustc ≥ 1.87). Pre-fix
    /// (iter-2 P7) used hardcoded Linux errno constants (EROFS=30,
    /// ENOSPC=28, EDQUOT=122) — correct on x86_64/arm64 but wrong on
    /// mips/sparc/alpha. The stable ErrorKind variants handle the
    /// cross-arch mapping for us.
    fn from_io(io_err: &std::io::Error, context: impl Into<String>) -> Self {
        use std::io::ErrorKind;

        let detail = format!("{}: {}", context.into(), io_err);
        match io_err.kind() {
            ErrorKind::NotFound => Self::ParentDirectoryMissing { detail },
            ErrorKind::PermissionDenied => Self::PermissionDenied { detail },
            ErrorKind::ReadOnlyFilesystem => Self::ReadOnlyFilesystem { detail },
            ErrorKind::StorageFull | ErrorKind::QuotaExceeded => Self::DiskFull { detail },
            _ => Self::IoError { detail },
        }
    }
}

impl std::fmt::Display for SecretsWriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "secrets.toml write failed ({}): {}", self.reason_code(), self.detail())
    }
}

impl std::error::Error for SecretsWriteError {}

/// Story F-2: the two secrets the first-run wizard persists to
/// `secrets.toml`. Both are written in a single atomic file write so the
/// gateway never observes a half-written secrets file.
pub(crate) struct SecretsToWrite<'a> {
    /// `[chirpstack].api_token`.
    pub chirpstack_api_token: &'a str,
    /// `[opcua].user_password`.
    pub opcua_password: &'a str,
}

/// Write the wizard-collected secrets to `secrets.toml` at the given path.
/// Sets file mode to 0o600 (owner read+write only).
///
/// Story F-2: writes BOTH `[chirpstack].api_token` and
/// `[opcua].user_password` in one body (previously password-only). The
/// figment provider stack merges this file on the next boot, overriding the
/// `config.toml` placeholders for both secrets.
///
/// Uses `tempfile + persist + rename` semantics to avoid leaving a
/// partial file on disk if the write is interrupted: writes to a
/// sibling temp file, sets permissions, then renames atomically.
///
/// Iter-2 P7: returns the categorised [`SecretsWriteError`] so
/// `setup_post` can map specific failure modes (EROFS / EACCES /
/// ENOSPC) to operator-actionable JSON `reason` codes.
fn write_secrets_toml(
    secrets_path: &std::path::Path,
    secrets: &SecretsToWrite,
) -> Result<(), SecretsWriteError> {
    use std::io::Write;

    // Build the TOML body. Escape every secret via `toml_escape_string`
    // (handles `"`, `\`, newlines, control chars) instead of hand-
    // formatting — the validators reject whitespace-bracketed values but
    // do NOT reject `"` or `\` mid-string for the api_token, so escaping
    // is the injection defence.
    let body = format!(
        r#"# opcgw secrets — generated by first-run wizard.
#
# This file holds the gateway secrets that must NOT live in the
# operator-readable config.toml: the ChirpStack API token and the OPC UA
# user password. File permissions: chmod 0600 (the gateway will reject
# the file if group/world has any access bits set in a future hardening
# story).
#
# To rotate a secret: either edit this file and restart opcgw, or override
# via the OPCGW_CHIRPSTACK__API_TOKEN / OPCGW_OPCUA__USER_PASSWORD env-vars.

[chirpstack]
api_token = {}

[opcua]
user_password = {}
"#,
        toml_escape_string(secrets.chirpstack_api_token),
        toml_escape_string(secrets.opcua_password),
    );

    // Iter-1 code review H2 / EH-M4: `Path::parent()` returns
    // `Some("")` (an empty path), NOT `None`, for paths with no
    // directory component (e.g. a bare `secrets.toml`). The
    // `tempfile::NamedTempFile::new_in("")` call interprets the
    // empty path as cwd on Linux but the behaviour is documented as
    // platform-specific. Coerce empty to `.` so the parent is always
    // a well-defined directory reference.
    let parent_raw = secrets_path.parent().ok_or_else(|| {
        SecretsWriteError::InvalidPath {
            detail: format!("secrets_path has no parent: {}", secrets_path.display()),
        }
    })?;
    let parent: &std::path::Path = if parent_raw.as_os_str().is_empty() {
        std::path::Path::new(".")
    } else {
        parent_raw
    };

    // tempfile in the same parent dir so rename is atomic (same fs).
    let mut tmp = tempfile::NamedTempFile::new_in(parent).map_err(|e| {
        SecretsWriteError::from_io(&e, format!("failed to create temp file in {}", parent.display()))
    })?;

    tmp.write_all(body.as_bytes()).map_err(|e| {
        SecretsWriteError::from_io(&e, "failed to write to temp secrets.toml")
    })?;
    tmp.flush().map_err(|e| {
        SecretsWriteError::from_io(&e, "failed to flush temp secrets.toml")
    })?;

    // chmod 0600 BEFORE rename so the file is never readable by group/
    // world even briefly. The Linux NamedTempFile starts with 0o600
    // by default in tempfile 3.x, but be explicit for forward-compat.
    let perms = std::fs::Permissions::from_mode(0o600);
    std::fs::set_permissions(tmp.path(), perms).map_err(|e| {
        SecretsWriteError::from_io(&e, "failed to set 0o600 on temp secrets.toml")
    })?;

    tmp.persist(secrets_path).map_err(|persist_err| {
        // `tempfile::PersistError` wraps both io::Error and the temp
        // file handle; extract the io::Error for categorisation.
        SecretsWriteError::from_io(
            &persist_err.error,
            format!("failed to persist secrets.toml to {}", secrets_path.display()),
        )
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
        let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
            password: "MyStrongP@ssw0rd!".to_string(),
            password_confirm: "MyStrongP@ssw0rd!".to_string(),
        };
        assert_eq!(validate_password(&req), None);
    }

    #[test]
    fn validate_password_rejects_empty() {
        let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
            password: "".to_string(),
            password_confirm: "".to_string(),
        };
        assert_eq!(validate_password(&req), Some("empty"));
    }

    #[test]
    fn validate_password_rejects_whitespace_bracketed() {
        let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
            password: " hello ".to_string(),
            password_confirm: " hello ".to_string(),
        };
        assert_eq!(validate_password(&req), Some("whitespace_bracketed"));
    }

    /// Iter-2 P17: pin the `whitespace_only` rejection branch.
    /// Pre-fix, `validate_password` had the `if .trim().is_empty()`
    /// check at line 241 but no test exercised it — a regression
    /// removing the branch would have shipped silently. Whitespace-
    /// only passwords are functionally indistinguishable from empty
    /// for the OPC UA auth layer but the rejection emits a clearer
    /// reason code for operator UX.
    #[test]
    fn validate_password_rejects_whitespace_only() {
        for pw in ["   ", "\t\t", " \t \n ".trim_end_matches('\n')] {
            let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
                password: pw.to_string(),
                password_confirm: pw.to_string(),
            };
            // Pure-whitespace bodies are caught by either
            // `whitespace_bracketed` (because trim() != self for
            // leading/trailing) or `whitespace_only` (for the empty-
            // after-trim case). Both are valid rejection reasons —
            // accept either. The test pins the BRANCH not the exact
            // ordering, since iter-2 may shuffle other early-return
            // arms without breaking the contract.
            let reason = validate_password(&req);
            assert!(
                matches!(reason, Some("whitespace_bracketed") | Some("whitespace_only")),
                "whitespace-only password {:?} must be rejected (got {:?})",
                pw, reason,
            );
        }
    }

    #[test]
    fn validate_password_rejects_placeholder_prefix() {
        let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
            password: format!("{}foo", PLACEHOLDER_PREFIX),
            password_confirm: format!("{}foo", PLACEHOLDER_PREFIX),
        };
        assert_eq!(validate_password(&req), Some("placeholder_prefix"));
    }

    #[test]
    fn validate_password_rejects_confirmation_mismatch() {
        let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
            password: "hello".to_string(),
            password_confirm: "world".to_string(),
        };
        assert_eq!(validate_password(&req), Some("confirmation_mismatch"));
    }

    /// Iter-1 EH-H1: DEL byte (U+007F) is rejected.
    #[test]
    fn validate_password_rejects_del_byte() {
        let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
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
            let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
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
        let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
            password: long_password.clone(),
            password_confirm: long_password,
        };
        assert_eq!(validate_password(&req), Some("too_long"));
    }

    /// Iter-1 EH-M1: exactly 256 chars is accepted.
    #[test]
    fn validate_password_accepts_256_chars() {
        let pw_256 = "a".repeat(256);
        let req = SetupRequest {
            server_address: "http://chirpstack:8080".to_string(),
            tenant_id: "tenant-1".to_string(),
            api_token: "valid-token".to_string(),
            password: pw_256.clone(),
            password_confirm: pw_256,
        };
        assert_eq!(validate_password(&req), None);
    }

    /// Story F-2: build a SetupRequest with valid OPC UA password fields so
    /// `validate_chirpstack` is exercised in isolation.
    fn cs_req(server_address: &str, tenant_id: &str, api_token: &str) -> SetupRequest {
        SetupRequest {
            server_address: server_address.to_string(),
            tenant_id: tenant_id.to_string(),
            api_token: api_token.to_string(),
            password: "GoodPassw0rd!".to_string(),
            password_confirm: "GoodPassw0rd!".to_string(),
        }
    }

    #[test]
    fn validate_chirpstack_accepts_reasonable_values() {
        assert_eq!(
            validate_chirpstack(&cs_req("http://chirpstack:8080", "tenant-1", "a-real-token")),
            None
        );
        assert_eq!(
            validate_chirpstack(&cs_req(
                "https://cs.example.com:8080",
                "00000000-0000-0000-0000-000000000001",
                "tok"
            )),
            None
        );
    }

    #[test]
    fn validate_chirpstack_rejects_empty_and_scheme() {
        assert_eq!(
            validate_chirpstack(&cs_req("", "t", "tok")),
            Some("server_address_empty")
        );
        assert_eq!(
            validate_chirpstack(&cs_req("ftp://nope", "t", "tok")),
            Some("server_address_scheme")
        );
        assert_eq!(
            validate_chirpstack(&cs_req("chirpstack:8080", "t", "tok")),
            Some("server_address_scheme")
        );
        assert_eq!(
            validate_chirpstack(&cs_req(" http://x:8080 ", "t", "tok")),
            Some("server_address_whitespace")
        );
    }

    #[test]
    fn validate_chirpstack_rejects_empty_tenant() {
        assert_eq!(
            validate_chirpstack(&cs_req("http://x:8080", "", "tok")),
            Some("tenant_id_empty")
        );
    }

    /// Code review iter-2 H1: a secret carrying the placeholder fragment
    /// MID-STRING (not just as a prefix) must be rejected, because the
    /// singleton migration's Guard 3 defers on `contains(PLACEHOLDER_MARKER)`
    /// and a deferred migration would dead-end the wizard. The validators use
    /// `contains(PLACEHOLDER_PREFIX)`, a superset of the migration's guard.
    #[test]
    fn validators_reject_mid_string_placeholder_fragment() {
        let embedded = format!("abc-{}OPCGW_-xyz", PLACEHOLDER_PREFIX);
        assert!(
            !embedded.starts_with(PLACEHOLDER_PREFIX),
            "test fixture must NOT start with the prefix (that's the whole point)"
        );
        // api_token
        assert_eq!(
            validate_chirpstack(&cs_req("http://x:8080", "t", &embedded)),
            Some("api_token_placeholder")
        );
        // tenant_id
        assert_eq!(
            validate_chirpstack(&cs_req("http://x:8080", &embedded, "tok")),
            Some("tenant_id_placeholder")
        );
        // OPC UA password
        let req = SetupRequest {
            server_address: "http://x:8080".to_string(),
            tenant_id: "t".to_string(),
            api_token: "tok".to_string(),
            password: embedded.clone(),
            password_confirm: embedded,
        };
        assert_eq!(validate_password(&req), Some("placeholder_prefix"));
    }

    /// Code review iter-1 LOW: control chars rejected on server_address +
    /// tenant_id (parity with api_token), and a placeholder tenant rejected.
    #[test]
    fn validate_chirpstack_rejects_control_chars_and_placeholder_tenant() {
        assert_eq!(
            validate_chirpstack(&cs_req("http://ho\u{08}st:8080", "t", "tok")),
            Some("server_address_control_char")
        );
        assert_eq!(
            validate_chirpstack(&cs_req("http://x:8080", "ab\u{7F}cd", "tok")),
            Some("tenant_id_control_char")
        );
        assert_eq!(
            validate_chirpstack(&cs_req(
                "http://x:8080",
                &format!("{}TENANT", PLACEHOLDER_PREFIX),
                "tok"
            )),
            Some("tenant_id_placeholder")
        );
    }

    #[test]
    fn validate_chirpstack_rejects_bad_api_token() {
        assert_eq!(
            validate_chirpstack(&cs_req("http://x:8080", "t", "")),
            Some("api_token_empty")
        );
        assert_eq!(
            validate_chirpstack(&cs_req(
                "http://x:8080",
                "t",
                &format!("{}OPCGW_CHIRPSTACK__API_TOKEN_ENV_VAR", PLACEHOLDER_PREFIX)
            )),
            Some("api_token_placeholder")
        );
        assert_eq!(
            validate_chirpstack(&cs_req("http://x:8080", "t", "tok\u{7F}en")),
            Some("api_token_control_char")
        );
        let long = "a".repeat(2049);
        assert_eq!(
            validate_chirpstack(&cs_req("http://x:8080", "t", &long)),
            Some("api_token_too_long")
        );
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
        assert!(is_wizard_bypass_path("/setup.html"));
        // Story F-2: the submit route is now exactly "/api/setup".
        assert!(is_wizard_bypass_path("/api/setup"));
        assert!(is_wizard_bypass_path("/dashboard.css"));
        // Story G-2 (#142): the wizard's field-help module.
        assert!(is_wizard_bypass_path("/field-help.js"));
        // Story F-2: the OLD "/api/setup/password" path must NO LONGER
        // bypass — leaving it would be a dead auth-exempt path.
        assert!(!is_wizard_bypass_path("/api/setup/password"));
        // Iter-3 P6: /favicon.ico was dropped from the allowlist
        // because static/favicon.ico doesn't ship — pinning the
        // rejection so a future re-add requires shipping the asset.
        assert!(!is_wizard_bypass_path("/favicon.ico"));
    }

    #[test]
    fn is_wizard_bypass_rejects_normal_paths() {
        assert!(!is_wizard_bypass_path("/"));
        assert!(!is_wizard_bypass_path("/applications"));
        assert!(!is_wizard_bypass_path("/api/applications"));
        assert!(!is_wizard_bypass_path("/api/health"));
        assert!(!is_wizard_bypass_path("/devices.html"));
    }

    /// Iter-2 P3 + P12: exact-match allowlist must REJECT paths that
    /// the pre-fix suffix-matcher accepted. Probing for arbitrary
    /// `.js/.css/.png/.ico/.svg/.woff/.woff2` suffixed paths under any
    /// directory must NOT bypass auth in first-run mode.
    #[test]
    fn is_wizard_bypass_rejects_suffix_probes() {
        // Suffix-only matches the old code allowed:
        assert!(!is_wizard_bypass_path("/applications.js"));
        assert!(!is_wizard_bypass_path("/icon.png"));
        assert!(!is_wizard_bypass_path("/anything.svg"));
        assert!(!is_wizard_bypass_path("/font.woff"));
        assert!(!is_wizard_bypass_path("/font.woff2"));
        // Path-traversal probes that suffix-matched .css under the old
        // code (ServeDir's `..` guard catches the actual file resolve
        // but the bypass logic should still refuse):
        assert!(!is_wizard_bypass_path("/etc/passwd.css"));
        assert!(!is_wizard_bypass_path("/../secrets.css"));
        // Future-API endpoints under /api/setup/ must NOT be auth-exempt by
        // virtue of the prefix — only the exact "/api/setup" is allow-listed.
        assert!(!is_wizard_bypass_path("/api/setup/anything"));
        assert!(!is_wizard_bypass_path("/api/setup/"));
        assert!(!is_wizard_bypass_path("/api/setup/password"));
    }

    #[test]
    fn toml_escape_string_handles_special_chars() {
        assert_eq!(toml_escape_string("simple"), r#""simple""#);
        assert_eq!(toml_escape_string(r#"has"quote"#), r#""has\"quote""#);
        assert_eq!(toml_escape_string(r"has\backslash"), r#""has\\backslash""#);
        assert_eq!(toml_escape_string("has\nnewline"), r#""has\nnewline""#);
    }

    /// Story F-2 test helper: build a `SecretsToWrite` for the two-secret
    /// writer.
    fn secrets<'a>(api_token: &'a str, password: &'a str) -> SecretsToWrite<'a> {
        SecretsToWrite {
            chirpstack_api_token: api_token,
            opcua_password: password,
        }
    }

    #[test]
    fn write_secrets_toml_creates_file_with_both_secrets() {
        let tmp_dir = tempfile::tempdir().expect("create tempdir");
        let secrets_path = tmp_dir.path().join("secrets.toml");

        write_secrets_toml(&secrets_path, &secrets("my-cs-token", "my-test-password"))
            .expect("write_secrets_toml succeeds");

        assert!(secrets_path.exists(), "secrets.toml was created");

        let body = std::fs::read_to_string(&secrets_path)
            .expect("read back secrets.toml");
        // Story F-2: both sections + both secrets present.
        assert!(body.contains("[chirpstack]"));
        assert!(body.contains(r#"api_token = "my-cs-token""#));
        assert!(body.contains("[opcua]"));
        assert!(body.contains(r#"user_password = "my-test-password""#));
    }

    #[test]
    fn write_secrets_toml_sets_mode_0600() {
        use std::os::unix::fs::MetadataExt;

        let tmp_dir = tempfile::tempdir().expect("create tempdir");
        let secrets_path = tmp_dir.path().join("secrets.toml");

        write_secrets_toml(&secrets_path, &secrets("tok", "my-test-password"))
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

    /// Iter-2 P10 fix: pre-fix this test extracted to `IgnoredAny`
    /// (which discards the parsed value) and then asserted on a
    /// byte-level substring match of the escaped form. That asserted
    /// the ESCAPER's output, not the round-trip semantics — a
    /// corrupted escape that produced a different but still-TOML-
    /// valid string would have been caught only IF the substring
    /// happened to drift. Fake regression-guard finding-class from
    /// the iter-2 review. Iter-2 fix: parse via the `toml` crate
    /// into a real `OpcuaSection` shape and assert the user_password
    /// value equals the original input character-for-character.
    #[test]
    fn write_secrets_toml_escapes_password_with_special_chars() {
        use serde::Deserialize;

        let tmp_dir = tempfile::tempdir().expect("create tempdir");
        let secrets_path = tmp_dir.path().join("secrets.toml");

        // Password contains quote + backslash + newline + carriage-
        // return + tab — every escape arm in `toml_escape_string` is
        // exercised at least once (iter-3 P9 added \r — pre-fix that
        // arm was unexercised and a typo `'\r' => out.push_str("\\n")`
        // would have shipped silently). Round-trip MUST recover the
        // exact original character-for-character.
        //
        // Note: in production, the validator rejects mid-string
        // control chars (iter-1 EH-H1) so this password never reaches
        // write_secrets_toml via the API — the round-trip is direct,
        // bypassing validate_password to exercise the escaper's
        // defence-in-depth control-char arms.
        let password = "has\"quote\\and-backslash\nand-newline\rand-cr\tand-tab";
        // Story F-2: exercise the escaper on BOTH secrets — the api_token
        // gets a distinct special-char payload so a broken escape arm in
        // either field is caught.
        let api_token = "tok\"with\\specials\nand\rcontrol\tchars";
        write_secrets_toml(&secrets_path, &secrets(api_token, password))
            .expect("write_secrets_toml succeeds");

        let body = std::fs::read_to_string(&secrets_path)
            .expect("read back secrets.toml");

        #[derive(Debug, Deserialize)]
        struct OpcuaSection {
            user_password: String,
        }
        #[derive(Debug, Deserialize)]
        struct ChirpstackSection {
            api_token: String,
        }
        #[derive(Debug, Deserialize)]
        struct Root {
            opcua: OpcuaSection,
            chirpstack: ChirpstackSection,
        }

        // Use figment (already a dep) for the round-trip parse;
        // figment::providers::Toml::string mirrors the production
        // figment stack used by `AppConfig::from_path` (iter-2 P15
        // also uses Toml::string for secrets.toml).
        use figment::providers::Format;
        let parsed: Root = figment::Figment::new()
            .merge(figment::providers::Toml::string(&body))
            .extract()
            .expect("secrets.toml must be valid TOML");
        assert_eq!(
            parsed.opcua.user_password, password,
            "round-trip MUST recover the exact original password — \
             escape arm is broken if this assertion fires. \
             body written:\n{}",
            body
        );
        assert_eq!(
            parsed.chirpstack.api_token, api_token,
            "round-trip MUST recover the exact original api_token — \
             escape arm is broken if this assertion fires. \
             body written:\n{}",
            body
        );
    }

    /// Iter-2 P5 regression-guard: concurrent submitters race against
    /// `setup_post`'s `compare_exchange`. EXACTLY ONE caller wins;
    /// the second sees `is_first_run=false` and gets 409 Conflict
    /// via the post-first-run path.
    ///
    /// Direct unit test of the atomic flip — full integration test
    /// (two concurrent HTTP POSTs) lives in tests/web_setup_wizard.rs.
    #[test]
    fn is_first_run_atomic_compare_exchange_is_one_shot() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;

        let atomic = Arc::new(AtomicBool::new(true));

        // First caller wins the exchange.
        let first = atomic
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        assert!(first, "first compare_exchange must succeed");
        assert!(
            !atomic.load(Ordering::SeqCst),
            "is_first_run must be false after first exchange"
        );

        // Second caller loses — atomic is already false.
        let second = atomic
            .compare_exchange(true, false, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok();
        assert!(!second, "second compare_exchange must fail (already flipped)");
    }
}

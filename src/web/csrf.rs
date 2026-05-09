// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! CSRF middleware for state-changing routes (Story 9-4).
//!
//! Story 9-1 deferred CSRF to Stories 9-4 / 9-5 / 9-6 per
//! `deferred-work.md:221`. Story 9-4 ships the canonical defence
//! that all three CRUD stories share — a hybrid of:
//!
//!  - **Origin same-origin enforcement** — every state-changing
//!    method (anything other than GET/HEAD/OPTIONS) MUST carry an
//!    `Origin` header whose scheme + host + port match the gateway's
//!    configured `[web].allowed_origins` list (default:
//!    `http://<bind_address>:<port>`). The `Referer` header is NOT
//!    consulted (Story 9-4 review iter-1 D2-P): per OWASP, Referer
//!    is forgeable from non-browser callers and unreliable on
//!    HTTPS→HTTP downgrade. Strict-Referrer-Policy clients without
//!    `Origin` are rejected.
//!  - **JSON-only `Content-Type` requirement** — the body content
//!    type must be `application/json` (with optional `; charset=...`
//!    suffix per RFC 7231). This rejects browser form-submit CSRF
//!    (the `application/x-www-form-urlencoded` and
//!    `multipart/form-data` content types a `<form>` POST emits).
//!
//! Safe methods (GET, HEAD, OPTIONS) bypass both checks; every other
//! method (including CONNECT, TRACE, custom methods) is treated as
//! state-changing and CSRF-checked (Story 9-4 review iter-1 P13).
//!
//! # Layer ordering invariant (load-bearing)
//!
//! Axum 0.8 stacks `.layer(...)` calls in **reverse declaration
//! order**, so for the runtime ordering "auth runs first → CSRF runs
//! second → handler runs third" the CSRF layer must be declared
//! BEFORE the auth layer in the router builder. See
//! [`crate::web::build_router`] for the canonical site.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use tracing::warn;

use crate::web::api::ErrorResponse;

/// Per-process CSRF state. Built at router construction time from
/// `WebConfig.resolved_allowed_origins()`. Hot-reload of
/// `[web].allowed_origins` is **restart-required** in v1 (same
/// blocker as GH #113 live-borrow refactor) so this state is
/// immutable for the lifetime of the router.
pub struct CsrfState {
    /// Pre-normalised lowercase scheme://host[:port] strings. The
    /// `Origin` / `Referer` header value is normalised the same way
    /// before comparison. Any entry containing a path / query /
    /// fragment is rejected by `WebConfig::validate` so we never see
    /// such an entry here.
    pub allowed_origins: Vec<String>,
}

impl CsrfState {
    pub fn new(allowed_origins: Vec<String>) -> Arc<Self> {
        let normalised = allowed_origins
            .into_iter()
            .map(|o| normalise_origin(&o))
            .collect();
        Arc::new(Self {
            allowed_origins: normalised,
        })
    }
}

/// Normalise an origin string for comparison: strip trailing slash,
/// lowercase the scheme + host, strip default ports (`:80` for `http`,
/// `:443` for `https`). Port is otherwise left as-is.
///
/// Iter-1 review P10: default-port equivalence — browsers omit the
/// port on default scheme/port pairs, so `http://gateway.local:80`
/// and `http://gateway.local` must compare equal.
fn normalise_origin(raw: &str) -> String {
    let trimmed = raw.trim().trim_end_matches('/');
    // Lowercase ONLY the scheme + host portion. URL parsing here is
    // intentionally minimal — we trust `WebConfig::validate` to have
    // rejected malformed entries upstream.
    if let Some(scheme_end) = trimmed.find("://") {
        let (scheme, rest) = trimmed.split_at(scheme_end);
        // rest starts with "://"
        let after_scheme = &rest[3..];
        // Find the boundary between host[:port] and any later
        // segment (which shouldn't exist after validation, but
        // defence-in-depth).
        let host_end = after_scheme
            .find(['/', '?', '#'])
            .unwrap_or(after_scheme.len());
        let host = &after_scheme[..host_end];
        let scheme_lower = scheme.to_ascii_lowercase();
        let host_lower = host.to_ascii_lowercase();
        // Iter-1 review P10: drop default ports.
        let host_normalised = match scheme_lower.as_str() {
            "http" => host_lower
                .strip_suffix(":80")
                .map(str::to_string)
                .unwrap_or(host_lower),
            "https" => host_lower
                .strip_suffix(":443")
                .map(str::to_string)
                .unwrap_or(host_lower),
            _ => host_lower,
        };
        format!("{scheme_lower}://{host_normalised}")
    } else {
        trimmed.to_ascii_lowercase()
    }
}

/// Origin from the `Origin` header. Returns `None` if `Origin` is
/// absent, `null`, malformed, or appears more than once.
///
/// **Iter-1 review D2-P (load-bearing):** Referer fallback was
/// dropped per OWASP guidance ("Origin first, Referer additive — not
/// fallback"). Referer is forgeable from non-browser callers and
/// unreliable on HTTPS→HTTP downgrade. Strict-Referrer-Policy
/// browsers + very old browsers without `Origin` are explicitly out
/// of scope; document operator-action in `docs/security.md §
/// Configuration mutations § CSRF defence`.
fn extract_origin(headers: &HeaderMap) -> Option<String> {
    // Iter-1 review P11: reject if multiple `Origin` headers are
    // present — RFC 6454 says at most one, but a buggy proxy or
    // attacker-controlled request can attach more. `headers.get(...)`
    // returns only the first; using `get_all` lets us count.
    let mut origin_iter = headers.get_all(header::ORIGIN).iter();
    let first = origin_iter.next()?;
    if origin_iter.next().is_some() {
        // Multi-Origin: bypass attempt; refuse.
        return None;
    }
    let v = first.to_str().ok()?;
    let trimmed = v.trim();
    if trimmed.is_empty() || trimmed == "null" {
        // Iter-1 review P14 (subsumed by D2-P): explicit fail-closed
        // on `Origin: null` (sandboxed iframes, `data:` URLs). Do
        // NOT fall back to Referer.
        return None;
    }
    Some(normalise_origin(trimmed))
}

fn content_type_is_json(headers: &HeaderMap) -> bool {
    let Some(value) = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let lower = value.trim().to_ascii_lowercase();
    // Accept "application/json" or "application/json; charset=...".
    // Iter-1 review P12: `;` is the RFC 7231 parameter separator. The
    // earlier `application/json ` (trailing space) branch was dropped
    // — that path accepted non-standard `application/json badness=true`.
    lower == "application/json" || lower.starts_with("application/json;")
}

fn is_state_changing(method: &Method) -> bool {
    // Iter-1 review P13: positive allow-list — only the safe,
    // idempotent methods bypass CSRF. CONNECT, TRACE, and any
    // future custom method are state-changing by default. The
    // earlier negative match (`POST | PUT | DELETE | PATCH`) silently
    // bypassed everything else.
    !matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS)
}

/// Story 9-5 AC#5/AC#8: dispatch the CSRF rejection audit-event
/// name by URL path so each resource gets its own grep contract.
///
/// - `/api/applications/:application_id/devices/.../commands*` → `"command"` (Story 9-6 future)
/// - `/api/applications/:application_id/devices*`              → `"device"`
/// - `/api/applications/...`                                   → `"application"` (Story 9-4)
/// - anything else                                             → `"unknown"` (catch-all; should not fire on
///   any wired mutating route today)
///
/// The longer-prefix branches are matched first so a future Story 9-6
/// commands surface lifts cleanly into the dispatch.
pub(crate) fn csrf_event_resource_for_path(path: &str) -> &'static str {
    // Match the bare LIST/CREATE surface FIRST so POST /api/applications
    // (no application_id) emits `event="application_crud_rejected"` on
    // CSRF rejection — preserving Story 9-4's grep contract at the
    // runtime level (the helper's unit test originally returned
    // "unknown" here; iter-1 of Story 9-5 widened it to "application"
    // after the integration-test layer surfaced the gap).
    if path == "/api/applications" || path == "/api/applications/" {
        return "application";
    }
    const APPS_PREFIX: &str = "/api/applications/";
    if let Some(after_apps) = path.strip_prefix(APPS_PREFIX) {
        // `after_apps` is `<application_id>` or `<application_id>/...`
        // Strip the application_id segment to inspect what follows.
        if let Some((_app_id, rest)) = after_apps.split_once('/') {
            // `rest` now starts after the application_id segment.
            // Patterns to recognise:
            //   "devices"                                  → device
            //   "devices/<device_id>"                      → device
            //   "devices/<device_id>/commands"             → command
            //   "devices/<device_id>/commands/<command_id>" → command
            if let Some(after_devices) = rest.strip_prefix("devices") {
                // Boundary check: must be exactly "devices" or
                // "devices/..." (NOT a prefix-collision like
                // "devicesXYZ" — though no such route would be wired).
                if after_devices.is_empty() || after_devices.starts_with('/') {
                    // Story 9-6 future: detect commands sub-resource.
                    let after_devices_slash = after_devices.strip_prefix('/').unwrap_or("");
                    if let Some((_dev_id, rest2)) = after_devices_slash.split_once('/') {
                        if rest2 == "commands" || rest2.starts_with("commands/") {
                            return "command";
                        }
                    }
                    return "device";
                }
            }
            // Anything else under /api/applications/<id>/... is
            // application-level (no other sub-resources today).
        }
        return "application";
    }
    "unknown"
}

fn reject(status: StatusCode, message: &str, hint: Option<&str>) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: message.to_string(),
            hint: hint.map(|s| s.to_string()),
        }),
    )
        .into_response()
}

/// CSRF middleware. See module-level doc-comment for the threat model.
pub async fn csrf_middleware(
    State(state): State<Arc<CsrfState>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: Request<Body>,
    next: Next,
) -> Response {
    if !is_state_changing(req.method()) {
        return next.run(req).await;
    }

    let method = req.method().clone();
    // Iter-1 review P27: log only the path portion, NOT the full URI
    // — query strings can carry secrets (`?token=...`) and should
    // never reach the audit log. `req.uri().path()` returns just the
    // path; do NOT switch to `req.uri().to_string()` here.
    let path = req.uri().path().to_string();
    // Story 9-5 AC#5/AC#8: per-resource event-name dispatch.
    let resource = csrf_event_resource_for_path(&path);
    let headers = req.headers();

    // Origin / Referer check.
    let origin = extract_origin(headers);
    let origin_ok = match origin.as_ref() {
        None => false,
        Some(o) => state.allowed_origins.iter().any(|allowed| allowed == o),
    };
    if !origin_ok {
        // Story 9-5 AC#8: literal event-name strings per match arm
        // so `git grep -hoE 'event = "<resource>_[a-z_]+"' src/`
        // returns each name exactly. Dynamic-string dispatch would
        // break the grep contract.
        //
        // The "command" arm intentionally falls through to the
        // generic catch-all in Story 9-5 — Story 9-6 will replace
        // the catch-all with a literal `command_crud_rejected` warn
        // when commands CRUD lands. Adding the literal here today
        // would constitute Story 9-5 scope creep.
        let origin_str = origin.as_deref().unwrap_or("<absent>");
        match resource {
            "device" => warn!(
                event = "device_crud_rejected",
                reason = "csrf",
                path = %path,
                method = %method,
                source_ip = %addr.ip(),
                origin = origin_str,
                "CSRF rejected: missing or cross-origin Origin"
            ),
            "application" => warn!(
                event = "application_crud_rejected",
                reason = "csrf",
                path = %path,
                method = %method,
                source_ip = %addr.ip(),
                origin = origin_str,
                "CSRF rejected: missing or cross-origin Origin"
            ),
            _ => warn!(
                event = "crud_rejected",
                reason = "csrf",
                resource = resource,
                path = %path,
                method = %method,
                source_ip = %addr.ip(),
                origin = origin_str,
                "CSRF rejected: missing or cross-origin Origin"
            ),
        }
        return reject(
            StatusCode::FORBIDDEN,
            "CSRF check failed: Origin header missing, null, or not in allow-list",
            Some(
                "set the Origin header on POST/PUT/DELETE/PATCH and ensure [web].allowed_origins includes the URL the operator's browser is using; Referer is no longer consulted (D2-P)",
            ),
        );
    }

    // JSON-only Content-Type check.
    if !content_type_is_json(headers) {
        match resource {
            "device" => warn!(
                event = "device_crud_rejected",
                reason = "csrf",
                path = %path,
                method = %method,
                source_ip = %addr.ip(),
                "CSRF rejected: Content-Type is not application/json"
            ),
            "application" => warn!(
                event = "application_crud_rejected",
                reason = "csrf",
                path = %path,
                method = %method,
                source_ip = %addr.ip(),
                "CSRF rejected: Content-Type is not application/json"
            ),
            _ => warn!(
                event = "crud_rejected",
                reason = "csrf",
                resource = resource,
                path = %path,
                method = %method,
                source_ip = %addr.ip(),
                "CSRF rejected: Content-Type is not application/json"
            ),
        }
        return reject(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "CSRF check failed: Content-Type must be application/json",
            Some("set Content-Type: application/json on POST/PUT/DELETE/PATCH bodies"),
        );
    }

    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use axum::http::HeaderValue;
    use axum::middleware;
    use axum::routing::{get, post};
    use axum::Router;
    use std::net::{IpAddr, Ipv4Addr};
    use tower::util::ServiceExt;

    fn req_with(
        method: Method,
        path: &str,
        headers: &[(&str, &str)],
        body: &str,
    ) -> Request<Body> {
        let mut builder = Request::builder().method(method).uri(path);
        for (k, v) in headers {
            builder = builder.header(*k, *v);
        }
        builder.body(Body::from(body.to_string())).unwrap()
    }

    async fn run_through_layer(
        allowed: Vec<&str>,
        mut req: Request<Body>,
    ) -> (StatusCode, String) {
        let state = CsrfState::new(allowed.into_iter().map(String::from).collect());
        // Inject `ConnectInfo` directly into the request extensions
        // so the middleware's extractor finds it without needing
        // `into_make_service_with_connect_info`. Same shape as
        // axum's own internal ConnectInfo plumbing — the connect
        // info lives in request extensions whether the service was
        // built with into_make_service_with_connect_info or by
        // hand.
        let conn_info: axum::extract::ConnectInfo<SocketAddr> = axum::extract::ConnectInfo(
            SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 1234),
        );
        req.extensions_mut().insert(conn_info);

        let app = Router::new()
            .route("/api/foo", get(|| async { "ok" }))
            .route("/api/foo", post(|| async { "ok" }))
            .layer(middleware::from_fn_with_state(state, csrf_middleware));

        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let body = String::from_utf8_lossy(&bytes).to_string();
        (status, body)
    }

    #[tokio::test]
    async fn csrf_passes_safe_methods() {
        let req = req_with(Method::GET, "/api/foo", &[], "");
        let (status, body) = run_through_layer(vec!["http://127.0.0.1:8080"], req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn csrf_rejects_post_with_no_origin() {
        let req = req_with(
            Method::POST,
            "/api/foo",
            &[("content-type", "application/json")],
            "{}",
        );
        let (status, body) = run_through_layer(vec!["http://127.0.0.1:8080"], req).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert!(body.contains("CSRF"));
    }

    #[tokio::test]
    async fn csrf_rejects_post_with_form_urlencoded() {
        let req = req_with(
            Method::POST,
            "/api/foo",
            &[
                ("origin", "http://127.0.0.1:8080"),
                ("content-type", "application/x-www-form-urlencoded"),
            ],
            "x=1",
        );
        let (status, body) = run_through_layer(vec!["http://127.0.0.1:8080"], req).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(body.contains("application/json"));
    }

    #[tokio::test]
    async fn csrf_accepts_post_with_same_origin_and_json() {
        let req = req_with(
            Method::POST,
            "/api/foo",
            &[
                ("origin", "http://127.0.0.1:8080"),
                ("content-type", "application/json"),
            ],
            "{}",
        );
        let (status, body) = run_through_layer(vec!["http://127.0.0.1:8080"], req).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, "ok");
    }

    #[tokio::test]
    async fn csrf_rejects_cross_origin() {
        let req = req_with(
            Method::POST,
            "/api/foo",
            &[
                ("origin", "http://evil.example.com"),
                ("content-type", "application/json"),
            ],
            "{}",
        );
        let (status, _body) = run_through_layer(vec!["http://127.0.0.1:8080"], req).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }

    #[test]
    fn normalise_origin_strips_trailing_slash_and_lowercases_scheme_host() {
        assert_eq!(
            normalise_origin("HTTP://Gateway.Local:8080/"),
            "http://gateway.local:8080"
        );
        assert_eq!(
            normalise_origin("http://127.0.0.1:8080"),
            "http://127.0.0.1:8080"
        );
    }

    /// Iter-1 review P10: default-port equivalence — `http://x:80`
    /// must equal `http://x`; `https://x:443` must equal `https://x`.
    #[test]
    fn normalise_origin_drops_default_ports() {
        assert_eq!(
            normalise_origin("http://gateway.local:80"),
            "http://gateway.local"
        );
        assert_eq!(
            normalise_origin("HTTPS://Gateway.Local:443/"),
            "https://gateway.local"
        );
        // Non-default ports stay.
        assert_eq!(
            normalise_origin("http://gateway.local:8080"),
            "http://gateway.local:8080"
        );
        // Wrong scheme/port pair stays.
        assert_eq!(
            normalise_origin("https://gateway.local:80"),
            "https://gateway.local:80"
        );
    }

    /// Iter-1 review P11: multi-`Origin` header bypass attempt is
    /// rejected (returns `None`).
    #[test]
    fn extract_origin_rejects_multiple_origin_headers() {
        let mut h = HeaderMap::new();
        h.append(header::ORIGIN, HeaderValue::from_static("http://allowed:8080"));
        h.append(header::ORIGIN, HeaderValue::from_static("http://evil.example"));
        assert_eq!(extract_origin(&h), None);
    }

    /// Iter-1 review D2-P / P14: `Origin: null` is fail-closed; no
    /// Referer fallback.
    #[test]
    fn extract_origin_fails_closed_on_origin_null_no_referer_fallback() {
        let mut h = HeaderMap::new();
        h.insert(header::ORIGIN, HeaderValue::from_static("null"));
        h.insert(
            header::REFERER,
            HeaderValue::from_static("http://allowed:8080/"),
        );
        // Returns None — Referer is NOT consulted.
        assert_eq!(extract_origin(&h), None);
    }

    /// Iter-1 review D2-P: when Origin is absent, Referer is NOT
    /// consulted — request rejected even if Referer matches the
    /// allow-list.
    #[test]
    fn extract_origin_does_not_fall_back_to_referer() {
        let mut h = HeaderMap::new();
        h.insert(
            header::REFERER,
            HeaderValue::from_static("http://allowed:8080/foo"),
        );
        assert_eq!(extract_origin(&h), None);
    }

    #[test]
    fn content_type_json_accepts_charset_suffix() {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json; charset=utf-8"),
        );
        assert!(content_type_is_json(&h));

        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("application/json"));
        assert!(content_type_is_json(&h));

        let mut h = HeaderMap::new();
        h.insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain"));
        assert!(!content_type_is_json(&h));
    }

    /// Iter-1 review P12: drop the `application/json `-followed-by-
    /// space branch — RFC 7231 says `;` is the parameter separator.
    #[test]
    fn content_type_json_rejects_space_followed_by_garbage() {
        let mut h = HeaderMap::new();
        h.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json badness=true"),
        );
        assert!(!content_type_is_json(&h));
    }

    /// Iter-1 review P13: positive method allow-list — only safe
    /// methods bypass CSRF. CONNECT/TRACE/custom methods are
    /// state-changing.
    #[test]
    fn is_state_changing_uses_positive_allow_list() {
        assert!(!is_state_changing(&Method::GET));
        assert!(!is_state_changing(&Method::HEAD));
        assert!(!is_state_changing(&Method::OPTIONS));
        assert!(is_state_changing(&Method::POST));
        assert!(is_state_changing(&Method::PUT));
        assert!(is_state_changing(&Method::DELETE));
        assert!(is_state_changing(&Method::PATCH));
        assert!(is_state_changing(&Method::CONNECT));
        assert!(is_state_changing(&Method::TRACE));
    }

    /// Story 9-5 AC#5/AC#8: path-aware CSRF event-name dispatch maps
    /// the URL path → resource string used by `csrf_middleware` to
    /// pick the per-resource literal `event = "<resource>_crud_rejected"`.
    /// Order-of-precedence matters: longer prefixes (commands inside
    /// devices) match first so Story 9-6 lifts cleanly.
    #[test]
    fn csrf_event_resource_for_path_maps_correctly() {
        // Application-level surface (Story 9-4 + Story 9-5 widening).
        // Bare /api/applications and /api/applications/ are the
        // LIST/CREATE surface: POST /api/applications without an
        // application_id IS application-level. Mapping to "application"
        // keeps the runtime-emitted CSRF audit-event name aligned with
        // the source-grep contract `event="application_crud_rejected"`.
        assert_eq!(
            csrf_event_resource_for_path("/api/applications"),
            "application"
        );
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/"),
            "application"
        );
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo"),
            "application"
        );
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/"),
            "application"
        );

        // Device surface (Story 9-5).
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/devices"),
            "device"
        );
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/devices/bar"),
            "device"
        );
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/devices/bar/"),
            "device"
        );

        // Command surface (Story 9-6 future).
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/devices/bar/commands"),
            "command"
        );
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/devices/bar/commands/1"),
            "command"
        );

        // Other / non-applications routes.
        assert_eq!(csrf_event_resource_for_path("/api/health"), "unknown");
        assert_eq!(csrf_event_resource_for_path("/api/devices"), "unknown");
        assert_eq!(csrf_event_resource_for_path("/dashboard.html"), "unknown");
        assert_eq!(csrf_event_resource_for_path("/"), "unknown");

        // Prefix-collision defence: no false-positive on
        // `/api/applications/foo/devicesXYZ` (no boundary slash).
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/devicesXYZ"),
            "application"
        );

        // Iter-1 review L9 (Blind B25 + Edge E14): empty-segment
        // edges. Empty application_id segment (`/api/applications//devices`)
        // routes through the helper as device because split_once('/')
        // yields ("", "devices") — the helper does not validate the
        // application_id segment, that's the path-validator's job at
        // the handler layer. Pin this behaviour so a future tightening
        // (e.g., reject empty segment with "unknown") is intentional.
        assert_eq!(
            csrf_event_resource_for_path("/api/applications//devices"),
            "device"
        );
        // Empty device_id segment under /commands: split_once('/')
        // of "/commands" yields ("", "commands") so the helper still
        // recognises the command sub-resource. Real routing would
        // 404 such a path, but the audit-event tier categorises it
        // as command (consistent with non-empty segments).
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/devices//commands"),
            "command"
        );
        // Trailing slash on commands route still maps to command.
        assert_eq!(
            csrf_event_resource_for_path("/api/applications/foo/devices/bar/commands/"),
            "command"
        );
    }
}

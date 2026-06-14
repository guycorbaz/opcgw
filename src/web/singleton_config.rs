// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story D-1: web handlers for the singleton-configuration editor.
//!
//! GET `/api/config/singleton` → snapshot of the four singleton sections
//! (`[global]` / `[chirpstack]` / `[opcua]` / `[web]`) from SQLite, with
//! secret fields replaced by placeholder strings.
//!
//! PUT `/api/config/singleton/<section>` → atomically replace one
//! section's editable fields. Validates the candidate AppConfig,
//! commits to SQLite via `SqliteBackend::write_singleton_section`,
//! emits audit events, then triggers a supervisor restart via
//! `state.shutdown_token.cancel()`.
//!
//! Mirrors the C-0 wizard's submit-and-restart pattern. Secrets stay
//! in `config/secrets.toml` and are not editable from this UI.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Map, Value};
use tracing::{info, warn};

use crate::storage::migrate_singleton_config::{
    secret_fields_for_section, KNOWN_SECTIONS,
};
use crate::web::AppState;

const SECRET_PLACEHOLDER: &str = "<set via config/secrets.toml>";

/// GET `/api/config/singleton` — returns the four-section snapshot from
/// SQLite with secret placeholders injected. Basic-auth-gated (via the
/// middleware stack); CSRF-exempt (read-only).
pub async fn get_singleton_config(
    State(state): State<Arc<AppState>>,
) -> Response {
    // opcgw uses a single web-auth identity (the configured
    // `[opcua].user_name`); the middleware authenticates against that
    // exclusively. Capture it for the audit log so reviewers can trace
    // "who edited what" across boots.
    let auth_user = state
        .config_reload
        .subscribe()
        .borrow()
        .opcua
        .user_name
        .clone();

    let rows = match state.sqlite_config.load_singleton_config() {
        Ok(rows) => rows,
        Err(e) => {
            warn!(
                event = "config_get_singleton_failed",
                auth_user = ?auth_user,
                error = ?e,
                "GET /api/config/singleton: failed to load singleton_config"
            );
            return internal_error("Failed to load singleton config");
        }
    };

    // Group rows by section into nested JSON objects.
    let mut sections: Map<String, Value> = Map::new();
    for s in KNOWN_SECTIONS {
        sections.insert((*s).to_string(), Value::Object(Map::new()));
    }
    for (section, key, value_json) in &rows {
        let parsed: Value = match serde_json::from_str(value_json) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    event = "config_get_singleton_failed",
                    auth_user = ?auth_user,
                    section = ?section,
                    key = ?key,
                    error = ?e,
                    "GET /api/config/singleton: malformed JSON in SQLite row"
                );
                return internal_error("Malformed JSON in stored config");
            }
        };
        if let Some(Value::Object(obj)) = sections.get_mut(section) {
            obj.insert(key.clone(), parsed);
        }
    }

    // Inject secret placeholders so the UI can render a read-only field.
    for (section_name, fields) in
        crate::storage::migrate_singleton_config::SECRET_FIELDS_BY_SECTION
    {
        if let Some(Value::Object(obj)) = sections.get_mut(*section_name) {
            for field in *fields {
                obj.insert(
                    (*field).to_string(),
                    Value::String(SECRET_PLACEHOLDER.to_string()),
                );
            }
        }
    }

    info!(
        event = "config_get_singleton",
        auth_user = ?auth_user,
        section_count = KNOWN_SECTIONS.len(),
        "GET /api/config/singleton: snapshot served"
    );

    (StatusCode::OK, Json(Value::Object(sections))).into_response()
}

/// PUT `/api/config/singleton/<section>` — replace the editable fields
/// for one section atomically. Basic-auth + CSRF. Rejects payloads
/// containing secret field names; validates the candidate AppConfig;
/// writes to SQLite; emits audit events; triggers supervisor restart.
pub async fn put_singleton_section(
    State(state): State<Arc<AppState>>,
    Path(section): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    // opcgw uses a single web-auth identity (the configured
    // `[opcua].user_name`); the middleware authenticates against that
    // exclusively. Capture it for the audit log so reviewers can trace
    // "who edited what" across boots.
    let auth_user = state
        .config_reload
        .subscribe()
        .borrow()
        .opcua
        .user_name
        .clone();

    // Validate section is in the allowlist.
    if !KNOWN_SECTIONS.contains(&section.as_str()) {
        warn!(
            event = "singleton_config_rejected",
            reason = "invalid_section",
            section = ?section,
            auth_user = ?auth_user,
            "PUT /api/config/singleton/{}: section not in allowlist",
            section
        );
        return validation_error(
            StatusCode::BAD_REQUEST,
            "invalid_section",
            None,
            Some("section must be one of: global, chirpstack, opcua, web"),
        );
    }

    // Body must be a JSON object.
    let body_obj = match body {
        Value::Object(m) => m,
        _ => {
            warn!(
                event = "singleton_config_rejected",
                reason = "validation",
                section = ?section,
                auth_user = ?auth_user,
                "PUT /api/config/singleton/{}: request body is not a JSON object",
                section
            );
            return validation_error(
                StatusCode::BAD_REQUEST,
                "validation",
                None,
                Some("request body must be a JSON object mapping field names to values"),
            );
        }
    };

    // Secret-field rejection: walk the payload keys against the skip-list.
    let secrets = secret_fields_for_section(&section);
    for k in body_obj.keys() {
        if secrets.contains(&k.as_str()) {
            warn!(
                event = "singleton_config_rejected",
                reason = "secret_field_not_editable",
                section = ?section,
                field = ?k,
                auth_user = ?auth_user,
                "PUT /api/config/singleton/{}: payload contains secret field {:?}",
                section,
                k
            );
            return validation_error(
                StatusCode::BAD_REQUEST,
                "secret_field_not_editable",
                Some(k),
                Some("secrets must be set via config/secrets.toml or environment variables"),
            );
        }
    }

    // Construct a candidate AppConfig by cloning the current snapshot
    // from the watch channel and overlaying the new section values.
    let current_arc = state.config_reload.subscribe().borrow().clone();
    let mut candidate: crate::config::AppConfig = (*current_arc).clone();

    // Build the (section, key, value-as-json-string) row representation
    // and reuse the overlay helper to apply it.
    let mut rows = Vec::with_capacity(body_obj.len());
    for (k, v) in body_obj.iter() {
        let v_str = match serde_json::to_string(v) {
            Ok(s) => s,
            Err(e) => {
                warn!(
                    event = "singleton_config_rejected",
                    reason = "validation",
                    section = ?section,
                    field = ?k,
                    auth_user = ?auth_user,
                    error = ?e,
                    "PUT /api/config/singleton/{}: failed to re-serialise field",
                    section
                );
                return validation_error(
                    StatusCode::BAD_REQUEST,
                    "validation",
                    Some(k),
                    Some("failed to encode field value"),
                );
            }
        };
        rows.push((section.clone(), k.clone(), v_str));
    }

    if let Err(e) = candidate.overlay_singletons_from_sqlite_rows(&rows) {
        warn!(
            event = "singleton_config_rejected",
            reason = "validation",
            section = ?section,
            auth_user = ?auth_user,
            error = ?e,
            "PUT /api/config/singleton/{}: candidate overlay failed",
            section
        );
        return validation_error(
            StatusCode::BAD_REQUEST,
            "validation",
            None,
            Some("candidate config construction failed; check field types match the section schema"),
        );
    }

    // Run the existing AppConfig validator on the candidate.
    if let Err(e) = candidate.validate() {
        warn!(
            event = "singleton_config_rejected",
            reason = "validation",
            section = ?section,
            auth_user = ?auth_user,
            error = ?e,
            "PUT /api/config/singleton/{}: AppConfig::validate rejected candidate",
            section
        );
        // I1-F6 (iter-1): don't expose internal OpcGwError Display in
        // the HTTP body — its variants can carry file paths, struct
        // field names, or implementation detail. Log the full error
        // structurally (already done above via `error = ?e`); return
        // a static, operator-facing hint.
        return validation_error(
            StatusCode::BAD_REQUEST,
            "validation",
            None,
            Some(
                "config validation failed; check field values are within \
                 allowed ranges. The full error is in the audit log.",
            ),
        );
    }

    // Convert the body into (key, value-as-json-string) pairs for the
    // backend write helper.
    let fields: Vec<(String, String)> = rows
        .iter()
        .map(|(_section, k, v_str)| (k.clone(), v_str.clone()))
        .collect();

    if let Err(e) = state
        .sqlite_config
        .write_singleton_section(&section, &fields)
    {
        // I1-F3 (iter-1): DO NOT emit `singleton_config_rejected` here
        // — that audit event is reserved for client-error cases
        // (validation / secret_field_not_editable / invalid_section /
        // csrf). A SQLite write failure is a server fault; HTTP 500
        // is the canonical signal. Emit a distinct event so audit
        // pipelines tracking client errors don't conflate them with
        // storage faults.
        warn!(
            event = "singleton_config_storage_error",
            section = ?section,
            auth_user = ?auth_user,
            error = ?e,
            "PUT /api/config/singleton/{}: SQLite write failed",
            section
        );
        return internal_error("Failed to persist singleton section");
    }

    // Success path: audit + stage (Story F-0).
    info!(
        event = "singleton_config_updated",
        section = ?section,
        field_count = fields.len(),
        auth_user = ?auth_user,
        "PUT /api/config/singleton/{}: section persisted to SQLite",
        section
    );

    // Story F-0: the change is STAGED, not applied. Previously this handler
    // called `state.shutdown_token.cancel()` to trigger a full container
    // restart. Now it bumps the pending-changes marker; the operator applies
    // all staged edits at once via `POST /api/config/apply`, which performs
    // one in-process soft restart (no container restart).
    state.stage_config_write("singleton_config");

    (
        StatusCode::ACCEPTED,
        Json(json!({"status": "staged", "pending_changes": true})),
    )
        .into_response()
}

fn validation_error(
    status: StatusCode,
    reason: &str,
    field: Option<&str>,
    hint: Option<&str>,
) -> Response {
    let mut body = json!({
        "error": "validation",
        "reason": reason,
    });
    if let Some(f) = field {
        body["field"] = json!(f);
    }
    if let Some(h) = hint {
        body["hint"] = json!(h);
    }
    (status, Json(body)).into_response()
}

fn internal_error(msg: &str) -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"error": "internal_server_error", "hint": msg})),
    )
        .into_response()
}

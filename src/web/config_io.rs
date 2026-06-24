// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story F-4: configuration export / import.
//!
//! - `GET /api/config/export` → the current effective configuration as a
//!   portable TOML document, with the two secrets (`chirpstack.api_token`,
//!   `opcua.user_password`) and the deployment-specific `[storage]` /
//!   `[logging]` / `[command_validation]` sections REMOVED. Auth-gated,
//!   read-only (CSRF-exempt). Served as a download.
//! - `POST /api/config/import` → a JSON envelope `{ "toml": "<text>" }`
//!   (CSRF requires `application/json`, so this is NOT a multipart upload — the
//!   browser reads the file client-side). The handler merges the imported TOML
//!   over the current effective config (so absent sections — including the
//!   secrets the export omits — keep the target instance's own values),
//!   validates the candidate, then STAGES it to SQLite (atomic app-tree
//!   replace + singleton-section writes) and bumps the pending-changes marker.
//!   It does NOT apply inline — the operator applies via `POST /api/config/apply`
//!   (Story F-0); the supervisor re-validates before tearing down the data
//!   plane, so a bad import is a non-disruptive `apply_failed`.
//!
//! # Why the figment merge for import
//!
//! The export omits secrets + host sections, so a standalone parse of the
//! imported TOML into `AppConfig` would fail (missing required fields) and
//! would also clobber the target's secrets. Instead we use the SAME figment
//! mechanism the boot path uses: serialize the current config as the base
//! layer, merge the imported TOML on top. Figment DEEP-merges tables, so an
//! imported `[chirpstack]` without `api_token` keeps the base's token (secrets
//! preserved per-instance); arrays (`[[application]]`) are replaced wholesale
//! when present (the app tree is imported), or the current tree is kept when
//! the file contains no applications.

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use serde_json::json;
use tracing::{info, warn};

use crate::config::AppConfig;
use crate::storage::migrate_singleton_config::{secret_fields_for_section, SECRET_FIELDS_BY_SECTION};
use crate::web::AppState;

/// Top-level sections dropped from the export: deployment/host-specific
/// (`storage` = database path; `logging` = file paths) and the scaffold
/// `command_validation` (config.toml-only, not part of the portable singleton
/// surface). The four singleton sections + the `application` tree remain.
const EXPORT_EXCLUDED_SECTIONS: &[&str] = &["storage", "logging", "command_validation"];

/// Build the export TOML from the effective config: serialize, then strip the
/// excluded sections and the two secret fields. Returns the TOML string.
pub(crate) fn build_export_toml(config: &AppConfig) -> Result<String, String> {
    let mut value =
        toml::Value::try_from(config).map_err(|e| format!("serialize config to toml: {e}"))?;

    if let toml::Value::Table(table) = &mut value {
        for section in EXPORT_EXCLUDED_SECTIONS {
            table.remove(*section);
        }
        // Strip secrets via the single-source-of-truth skip-list so the export
        // can never carry `api_token` / `user_password`.
        for (section, fields) in SECRET_FIELDS_BY_SECTION {
            if let Some(toml::Value::Table(sec)) = table.get_mut(*section) {
                for f in *fields {
                    sec.remove(*f);
                }
            }
        }
    }

    toml::to_string_pretty(&value).map_err(|e| format!("render toml: {e}"))
}

/// `GET /api/config/export` — download the portable config as TOML.
pub async fn export_config(State(state): State<Arc<AppState>>) -> Response {
    let cfg = state.config_reload.subscribe().borrow().clone();
    match build_export_toml(&cfg) {
        Ok(body) => {
            info!(
                event = "config_exported",
                bytes = body.len(),
                "GET /api/config/export: config exported (secrets excluded)"
            );
            (
                StatusCode::OK,
                [
                    (
                        header::CONTENT_TYPE,
                        HeaderValue::from_static("text/plain; charset=utf-8"),
                    ),
                    (
                        header::CONTENT_DISPOSITION,
                        HeaderValue::from_static("attachment; filename=\"opcgw-config.toml\""),
                    ),
                    (header::CACHE_CONTROL, HeaderValue::from_static("no-store")),
                ],
                body,
            )
                .into_response()
        }
        Err(e) => {
            // NFR7: log the detail, return a generic body.
            warn!(
                event = "config_export_failed",
                error = %e,
                "GET /api/config/export: failed to build export"
            );
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({"error": "internal_server_error"})),
            )
                .into_response()
        }
    }
}

/// Body schema for `POST /api/config/import`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImportRequest {
    /// The TOML config text (as produced by `GET /api/config/export`).
    pub toml: String,
}

/// Build the candidate config by merging the imported TOML over the current
/// effective config. Figment deep-merges tables (so omitted secrets keep the
/// target's values) and replaces arrays (the `application` tree is imported
/// when present).
fn merge_imported_config(current: &AppConfig, imported_toml: &str) -> Result<AppConfig, String> {
    use figment::providers::{Format, Serialized, Toml};
    use figment::Figment;

    Figment::from(Serialized::defaults(current))
        .merge(Toml::string(imported_toml))
        .extract()
        .map_err(|e| e.to_string())
}

fn import_error(status: StatusCode, reason: &str, hint: &str) -> Response {
    (
        status,
        Json(json!({ "error": "import_failed", "reason": reason, "hint": hint })),
    )
        .into_response()
}

/// `POST /api/config/import` — stage an imported config (Story F-4 + F-0).
pub async fn import_config(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    // Parse the JSON envelope manually so a bad body maps to the structured
    // error shape rather than Axum's default plain-text 400/415.
    let req: ImportRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(e) => {
            warn!(event = "config_import_rejected", reason = "invalid_json", error = %e,
                  "POST /api/config/import: body is not valid JSON {{toml}}");
            return import_error(
                StatusCode::BAD_REQUEST,
                "invalid_json",
                "POST a JSON body of the shape {\"toml\": \"<config text>\"}",
            );
        }
    };

    // Build + validate the candidate BEFORE any write.
    let current = state.config_reload.subscribe().borrow().clone();
    let candidate = match merge_imported_config(&current, &req.toml) {
        Ok(c) => c,
        Err(e) => {
            warn!(event = "config_import_rejected", reason = "invalid_toml", error = %e,
                  "POST /api/config/import: imported TOML failed to parse/merge");
            return import_error(
                StatusCode::BAD_REQUEST,
                "invalid_toml",
                "the uploaded file is not a valid opcgw config TOML",
            );
        }
    };
    if let Err(e) = candidate.validate() {
        // NFR7: log the full validation error, return a static hint.
        warn!(event = "config_import_rejected", reason = "config_invalid", error = ?e,
              "POST /api/config/import: candidate config failed AppConfig::validate");
        return import_error(
            StatusCode::BAD_REQUEST,
            "config_invalid",
            "the imported configuration failed validation (e.g. duplicate IDs or out-of-range \
             values). The full error is in the gateway log.",
        );
    }

    // Persist (staged) — ALL of it in ONE transaction (code review iter-1):
    // the four singleton sections + the whole application tree are replaced
    // atomically by `import_replace_all`, so a storage failure anywhere rolls
    // the entire import back, leaving the prior config intact (no half-staged
    // state). Secrets are NEVER written — the skip-list excludes them, so the
    // target keeps its own secrets.toml. Build the per-section (key, value)
    // rows first (secret fields skipped); a serialize failure aborts before
    // any write.
    use crate::storage::migrate_singleton_config::serialize_section;
    let mut singletons: Vec<(&str, Vec<(String, String)>)> = Vec::with_capacity(4);
    macro_rules! push_section {
        ($name:literal, $field:expr) => {
            match serialize_section($field, secret_fields_for_section($name)) {
                Ok(f) => singletons.push(($name, f)),
                Err(e) => return import_storage_error(&state, $name, &e.to_string()),
            }
        };
    }
    push_section!("global", &candidate.global);
    push_section!("chirpstack", &candidate.chirpstack);
    push_section!("opcua", &candidate.opcua);
    push_section!("web", &candidate.web);

    if let Err(e) = state
        .sqlite_config
        .import_replace_all(&singletons, &candidate.application_list)
    {
        return import_storage_error(&state, "import", &e.to_string());
    }

    // Stage — the operator applies via POST /api/config/apply (F-0). Do NOT
    // apply inline.
    state.stage_config_write("import");

    info!(
        event = "config_imported",
        applications = candidate.application_list.len(),
        "POST /api/config/import: config staged; operator must Apply to activate"
    );
    (
        StatusCode::ACCEPTED,
        Json(json!({ "status": "staged", "pending_changes": true })),
    )
        .into_response()
}

/// Shared 500 path for a storage failure during import. The singleton writes
/// are section-replaces and the app-tree replace is atomic; a failure here
/// leaves the gateway running on its current config (nothing applied — import
/// only stages), and the operator can retry the import.
fn import_storage_error(_state: &Arc<AppState>, section: &str, detail: &str) -> Response {
    warn!(
        event = "config_import_storage_error",
        section = section,
        error = detail,
        "POST /api/config/import: SQLite write failed"
    );
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({ "error": "internal_server_error" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web::test_support::stub_app_config;

    #[test]
    fn export_excludes_secrets_and_host_sections() {
        let mut cfg = stub_app_config();
        cfg.chirpstack.api_token = "SECRET-TOKEN-XYZ".to_string();
        cfg.opcua.user_password = "SECRET-PASSWORD-XYZ".to_string();

        let toml = build_export_toml(&cfg).expect("build export");

        // Secrets must NEVER appear.
        assert!(
            !toml.contains("SECRET-TOKEN-XYZ"),
            "export leaked the api_token:\n{toml}"
        );
        assert!(
            !toml.contains("SECRET-PASSWORD-XYZ"),
            "export leaked the user_password:\n{toml}"
        );
        assert!(!toml.contains("api_token"), "export must omit the api_token key");
        assert!(
            !toml.contains("user_password"),
            "export must omit the user_password key"
        );
        // Host/deployment sections excluded.
        assert!(!toml.contains("[storage]"), "export must omit [storage]");
        assert!(!toml.contains("[logging]"), "export must omit [logging]");
        // Portable sections present.
        assert!(toml.contains("[chirpstack]"), "export must include [chirpstack]");
        assert!(toml.contains("[opcua]"), "export must include [opcua]");
    }

    #[test]
    fn export_round_trips_through_merge_preserving_secrets() {
        // Source config with real secrets + a distinctive non-secret value.
        let mut source = stub_app_config();
        source.chirpstack.api_token = "SOURCE-TOKEN".to_string();
        source.chirpstack.server_address = "http://exported-host:8080".to_string();
        source.opcua.user_password = "SOURCE-PASSWORD".to_string();

        let exported = build_export_toml(&source).expect("export");

        // Import target with DIFFERENT secrets — they must be preserved.
        let mut target = stub_app_config();
        target.chirpstack.api_token = "TARGET-TOKEN".to_string();
        target.chirpstack.server_address = "http://old-host:1111".to_string();
        target.opcua.user_password = "TARGET-PASSWORD".to_string();

        let merged = merge_imported_config(&target, &exported).expect("merge");

        // Non-secret value imported from the source.
        assert_eq!(merged.chirpstack.server_address, "http://exported-host:8080");
        // Secrets preserved from the TARGET (import never carries/overwrites them).
        assert_eq!(merged.chirpstack.api_token, "TARGET-TOKEN");
        assert_eq!(merged.opcua.user_password, "TARGET-PASSWORD");
    }
}

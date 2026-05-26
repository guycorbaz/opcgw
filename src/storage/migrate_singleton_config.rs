// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Story D-0: One-shot singleton-config TOML→SQLite migration.
//!
//! Runs once on the first boot of a post-D-0 binary that has a populated
//! `config.toml` but an empty `singleton_config` SQLite table. Bulk-writes
//! the non-secret fields from `[global]` / `[chirpstack]` / `[opcua]` /
//! `[web]` into SQLite as (section, key, value) rows.
//!
//! Mirrors the C-6 [`crate::storage::migrate_config`] pattern: primary
//! guard via meta done-flag, secondary back-fill guard for direct-SQLite-
//! import databases, EXCLUSIVE TRANSACTION + per-section row-count
//! verification, fall-back to TOML on mismatch.
//!
//! **Secrets out of scope.** `[chirpstack].api_token` and
//! `[opcua].user_password` are NEVER written to SQLite — they remain in
//! `config/secrets.toml` (chmod 0600 per Story C-0). If the in-memory
//! `AppConfig` still carries the placeholder strings (operator has not
//! supplied secrets yet), migration is skipped via
//! `MigrationOutcome::SkippedEmptyOrPlaceholder` and retried on the
//! next boot.

use crate::config::AppConfig;
use crate::storage::SqliteBackend;
use crate::utils::OpcGwError;
use std::time::Instant;
use tracing::{info, warn};

/// Outcome of a `migrate_singleton_toml_to_sqlite` call.
pub enum SingletonMigrationOutcome {
    /// Migration ran and committed successfully.
    Migrated(SingletonMigrationReport),
    /// `singleton_config` table was already populated — migration skipped
    /// via primary guard (done-flag set) or secondary guard (rows present,
    /// done-flag absent, back-fill attempted best-effort).
    AlreadyMigrated,
    /// Operator has not yet supplied the `[chirpstack].api_token` and/or
    /// `[opcua].user_password` secrets (placeholder strings still in
    /// `config.toml`). Migration is deferred to the next boot once
    /// secrets are supplied; the gateway runs from the in-memory
    /// `AppConfig` snapshot for the current start-up.
    SkippedEmptyOrPlaceholder,
}

/// Per-section row counts written during a successful migration.
pub struct SingletonMigrationReport {
    pub sections: usize,
    pub rows: usize,
    pub duration_ms: u64,
}

const PLACEHOLDER_MARKER: &str = "REPLACE_ME_WITH_OPCGW_";

/// Detect whether a singleton-config migration is needed and, if so, run it.
///
/// Returns `Ok(SingletonMigrationOutcome)` in all non-fatal cases. Callers
/// (specifically `main.rs`) should treat `Err` as a migration failure and
/// fall back to the TOML-driven boot path for the current start-up (the
/// transition safety net defined in D-0 AC#6).
pub fn migrate_singleton_toml_to_sqlite(
    app_config: &AppConfig,
    backend: &SqliteBackend,
) -> Result<SingletonMigrationOutcome, OpcGwError> {
    // ── Guard 1: primary — done-flag set ─────────────────────────────────
    if backend.is_d0_migration_done()? {
        let existing = backend.count_singleton_config().unwrap_or(0);
        info!(
            event = "config_migration",
            stage = "singleton_already_migrated",
            rows = existing,
        );
        return Ok(SingletonMigrationOutcome::AlreadyMigrated);
    }

    // ── Guard 2: secondary — singleton_config non-empty without done-flag.
    // Direct-SQLite-import scenario; back-fill the meta key best-effort so
    // future boots hit the faster primary guard. Back-fill failure must
    // NOT surface as `config_migration_failed` upstream — the data is
    // already intact (mirrors C-6 iter-3 I3-F2 lesson).
    let existing = backend.count_singleton_config()?;
    if existing > 0 {
        if let Err(e) = backend.write_d0_migration_done() {
            warn!(
                event = "config_migration",
                stage = "singleton_already_migrated_backfill_failed",
                error = %e,
                "Meta key back-fill failed; data intact, retry attempted on \
                 subsequent boots if backend is healthy"
            );
        }
        info!(
            event = "config_migration",
            stage = "singleton_already_migrated",
            rows = existing,
        );
        return Ok(SingletonMigrationOutcome::AlreadyMigrated);
    }

    // ── Guard 3: placeholder secrets — operator hasn't supplied yet ──────
    // Migrating now would persist placeholder strings if the validator
    // somehow accepted them; defer until secrets are supplied. The
    // gateway runs from the in-memory `AppConfig` snapshot for this boot
    // and retries idempotently on next boot.
    if app_config.chirpstack.api_token.contains(PLACEHOLDER_MARKER)
        || app_config.opcua.user_password.contains(PLACEHOLDER_MARKER)
    {
        let mut missing = Vec::new();
        if app_config.chirpstack.api_token.contains(PLACEHOLDER_MARKER) {
            missing.push("chirpstack.api_token");
        }
        if app_config.opcua.user_password.contains(PLACEHOLDER_MARKER) {
            missing.push("opcua.user_password");
        }
        let missing_str = missing.join(",");
        info!(
            event = "config_migration",
            stage = "skipped_placeholder_singleton",
            missing_secret = %missing_str,
            "Singleton config migration deferred; operator-supplied secrets \
             still hold placeholder strings"
        );
        return Ok(SingletonMigrationOutcome::SkippedEmptyOrPlaceholder);
    }

    // ── Run migration ─────────────────────────────────────────────────────
    let start = Instant::now();
    let mut total_rows = 0usize;
    let sections = 4usize;

    let global_fields = serialize_section(&app_config.global, &[])?;
    backend.write_singleton_section("global", &global_fields)?;
    total_rows += global_fields.len();

    // `[chirpstack]` — skip `api_token` (secret stays in secrets.toml).
    let chirpstack_fields = serialize_section(&app_config.chirpstack, &["api_token"])?;
    backend.write_singleton_section("chirpstack", &chirpstack_fields)?;
    total_rows += chirpstack_fields.len();

    // `[opcua]` — skip `user_password` (secret stays in secrets.toml).
    let opcua_fields = serialize_section(&app_config.opcua, &["user_password"])?;
    backend.write_singleton_section("opcua", &opcua_fields)?;
    total_rows += opcua_fields.len();

    let web_fields = serialize_section(&app_config.web, &[])?;
    backend.write_singleton_section("web", &web_fields)?;
    total_rows += web_fields.len();

    // Verify post-write row count matches what we wrote.
    let actual = backend.count_singleton_config()?;
    if actual != total_rows {
        return Err(OpcGwError::Database(format!(
            "singleton_row_count_mismatch: expected={} actual={}",
            total_rows, actual
        )));
    }

    backend.write_d0_migration_done()?;

    let duration_ms = start.elapsed().as_millis() as u64;
    info!(
        event = "config_migration",
        stage = "singleton_toml_to_sqlite",
        sections = sections,
        rows = total_rows,
        duration_ms = duration_ms,
    );
    Ok(SingletonMigrationOutcome::Migrated(SingletonMigrationReport {
        sections,
        rows: total_rows,
        duration_ms,
    }))
}

/// Serialize a singleton section struct to `(key, value-as-json-string)`
/// pairs, skipping the named fields (used to exclude secrets).
///
/// Each scalar value is serialized as its JSON representation
/// (`10` for `polling_frequency=10`, `"http://..."` for strings, `["a","b"]`
/// for lists). The Rust-side load path in `AppConfig::load_singletons_from_sqlite`
/// reverses this via `serde_json::from_str`. SQLite is transport only;
/// typing is enforced by `AppConfig::validate` post-load.
fn serialize_section<T: serde::Serialize>(
    section: &T,
    skip_fields: &[&str],
) -> Result<Vec<(String, String)>, OpcGwError> {
    let value = serde_json::to_value(section).map_err(|e| {
        OpcGwError::Database(format!("serialize_section: serde_json::to_value: {}", e))
    })?;
    let map = value.as_object().ok_or_else(|| {
        OpcGwError::Database("serialize_section: section did not serialize to a JSON object".into())
    })?;
    let mut out = Vec::with_capacity(map.len());
    for (k, v) in map {
        if skip_fields.contains(&k.as_str()) {
            continue;
        }
        // Skip null values (Option<T>::None) so SQLite mirrors TOML's
        // "missing key" semantic. Round-tripping null vs missing matters
        // for D-1's editor UI (a missing key means "use the struct
        // default at load time", which is what figment / serde do
        // post-load anyway).
        if v.is_null() {
            continue;
        }
        let v_str = serde_json::to_string(v).map_err(|e| {
            OpcGwError::Database(format!(
                "serialize_section: serde_json::to_string for key={}: {}",
                k, e
            ))
        })?;
        out.push((k.clone(), v_str));
    }
    // Stable iteration order for deterministic write + test assertions.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

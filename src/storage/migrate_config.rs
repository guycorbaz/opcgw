// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! One-shot TOML→SQLite configuration migration (Story C-6).
//!
//! Runs once on the first post-C-6 boot of an existing gateway:
//! detects that the `applications` SQLite table is empty while
//! `AppConfig.application_list` is non-empty, then bulk-inserts all
//! application/device/metric/command rows in one transaction.
//!
//! On every subsequent boot the `applications` table is non-empty so
//! the migration guard short-circuits immediately (idempotent).

use crate::config::AppConfig;
use crate::storage::SqliteBackend;
use crate::utils::OpcGwError;
use std::time::Instant;
use tracing::{info, warn};

/// Outcome of a `migrate_toml_to_sqlite` call.
pub enum MigrationOutcome {
    /// Migration ran and committed successfully.
    Migrated(MigrationReport),
    /// `applications` table was already non-empty — migration skipped.
    AlreadyMigrated,
    /// `AppConfig.application_list` was empty (C-0 empty-bootstrap path).
    SkippedEmptySource,
}

/// Per-entity counts written during the migration.
pub struct MigrationReport {
    pub applications: usize,
    pub devices: usize,
    pub metrics: usize,
    pub commands: usize,
    pub duration_ms: u64,
}

/// Detect whether a TOML→SQLite migration is needed and, if so, run it.
///
/// Returns `Ok(MigrationOutcome)` in all non-fatal cases.  Callers should
/// treat `Err` as a migration failure and fall back to the TOML-driven boot
/// path (the transition safety net defined in AC#5 / AC#12).
pub fn migrate_toml_to_sqlite(
    app_config: &AppConfig,
    backend: &SqliteBackend,
) -> Result<MigrationOutcome, OpcGwError> {
    // ── Guard 1: already migrated ─────────────────────────────────────────
    // Primary check: meta done-flag (survives operator deletion of all
    // applications via the web UI — row-count alone would false-trigger
    // re-migration in that scenario).
    if backend.is_c6_migration_done()? {
        let existing = backend.count_applications()?;
        info!(
            event = "config_migration",
            stage = "already_migrated",
            applications = existing,
        );
        return Ok(MigrationOutcome::AlreadyMigrated);
    }
    // Secondary check: non-empty applications table without the done-flag
    // (e.g. a direct SQLite import that bypassed migrate_applications_config).
    // Back-fill the meta key so future boots hit the faster primary guard.
    // Back-fill is best-effort: data is already intact, so a transient pool
    // failure here must not surface as `config_migration_failed` upstream.
    let existing = backend.count_applications()?;
    if existing > 0 {
        if let Err(e) = backend.write_c6_migration_done() {
            warn!(
                event = "config_migration",
                stage = "already_migrated_backfill_failed",
                error = %e,
                "Meta key back-fill failed; data intact, retry attempted on \
                 subsequent boots if backend is healthy"
            );
        }
        info!(
            event = "config_migration",
            stage = "already_migrated",
            applications = existing,
        );
        return Ok(MigrationOutcome::AlreadyMigrated);
    }

    // ── Guard 2: nothing to migrate (C-0 empty-bootstrap) ─────────────────
    if app_config.application_list.is_empty() {
        info!(event = "config_migration", stage = "skipped_empty_source");
        return Ok(MigrationOutcome::SkippedEmptySource);
    }

    // ── Run migration ─────────────────────────────────────────────────────
    let start = Instant::now();
    match backend.migrate_applications_config(&app_config.application_list) {
        Ok((apps, devices, metrics, commands)) => {
            let duration_ms = start.elapsed().as_millis() as u64;
            info!(
                event = "config_migration",
                stage = "toml_to_sqlite",
                applications = apps,
                devices = devices,
                metrics = metrics,
                commands = commands,
                duration_ms = duration_ms,
            );
            Ok(MigrationOutcome::Migrated(MigrationReport {
                applications: apps,
                devices,
                metrics,
                commands,
                duration_ms,
            }))
        }
        Err(e) => Err(e),
    }
}

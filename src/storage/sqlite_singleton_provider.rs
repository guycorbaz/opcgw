// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! SQLite singleton-config → figment Provider (Story D-2).
//!
//! Reads the four singleton sections (`[global]`, `[chirpstack]`,
//! `[opcua]`, `[web]` — non-secret fields) from the SQLite
//! `singleton_config` table populated by Story D-0's boot-time
//! migration, and exposes them through the [`figment::Provider`]
//! trait so figment's existing layered loader can merge them at the
//! right precedence level.
//!
//! # Final precedence ordering (Story D-2)
//!
//! Pre-D-2, Story D-1 used a post-figment `Arc::make_mut` overlay
//! that ran AFTER figment had assembled the config from TOML + env-var,
//! which left SQLite winning over env-var — the opposite of D-0 spec
//! AC#8's intended `env > SQLite > TOML > default` ordering. D-2
//! replaces that overlay with this Provider, slotted into the
//! figment stack between TOML and env-var:
//!
//! ```text
//! 1. Toml::file(config_path)       (lowest priority — bootstrap seed)
//! 2. Toml::string(secrets.toml)    (secret fields only)
//! 3. SqliteSingletonProvider       (non-secret runtime config — THIS)
//! 4. Env::prefixed("OPCGW_")       (highest priority — operator override)
//! ```
//!
//! Each higher layer overrides keys present in lower layers. The
//! shape delivers the AC#8 ordering as a structural figment guarantee
//! rather than a post-hoc patch.
//!
//! # Error handling
//!
//! All internal failures (pool checkout, SQL execution, JSON parse)
//! are non-fatal — the Provider returns an empty [`figment::value::Map`]
//! and emits a `config_provider_failed` `warn` event so figment falls
//! through to the next provider (TOML). The gateway boots cleanly
//! against the figment-loaded TOML defaults even if SQLite is
//! unavailable.

use std::sync::Arc;

use figment::{
    providers::Serialized,
    value::{Dict, Map},
    Metadata, Profile, Provider,
};
use tracing::warn;

use crate::storage::sqlite::SqliteBackend;

/// figment Provider that reads non-secret runtime configuration from
/// the SQLite `singleton_config` table written by D-0 + D-1.
///
/// See module-level doc for the precedence ordering and error
/// semantics. The Provider does NOT touch secret fields
/// (`api_token`, `user_password`) — those continue to flow through
/// `config/secrets.toml` per the post-C-0 secret store contract.
pub struct SqliteSingletonProvider {
    backend: Arc<SqliteBackend>,
}

impl SqliteSingletonProvider {
    /// Construct a Provider bound to the given SQLite backend handle.
    ///
    /// The `Arc<SqliteBackend>` allows the Provider to be re-evaluated
    /// on subsequent figment `.extract()` calls (e.g. for the D-1
    /// PUT-handler's candidate-AppConfig validation path) without
    /// re-opening the connection pool.
    pub fn new(backend: Arc<SqliteBackend>) -> Self {
        Self { backend }
    }
}

impl Provider for SqliteSingletonProvider {
    fn metadata(&self) -> Metadata {
        Metadata::named("opcgw SQLite singleton_config (Story D-2)")
    }

    fn data(&self) -> Result<Map<Profile, Dict>, figment::Error> {
        // Fast path: figment Provider invoked before D-0 migration has
        // populated the table. Returning an empty map causes figment
        // to fall through to the TOML layer below this one. Same shape
        // covers the `config.toml`-only fresh-deployment case.
        let rows = match self.backend.load_singleton_config() {
            Ok(rows) => rows,
            Err(e) => {
                warn!(
                    event = "config_provider_failed",
                    error = ?e,
                    "Failed to read singleton_config from SQLite — \
                     falling through to next figment provider"
                );
                return Ok(Map::new());
            }
        };

        if rows.is_empty() {
            return Ok(Map::new());
        }

        // Re-assemble the rows into the nested
        // `{ section: { key: value_json, ... }, ... }` shape that
        // matches the AppConfig figment layout. D-0 stores each row's
        // `value` column as a JSON-encoded string — parse it back into
        // a `serde_json::Value` so figment's serializer can route the
        // typed value to the right AppConfig field.
        let mut root: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for (section, key, value_json) in rows {
            let parsed: serde_json::Value = match serde_json::from_str(&value_json) {
                Ok(v) => v,
                Err(e) => {
                    warn!(
                        event = "config_provider_failed",
                        section = ?section,
                        key = ?key,
                        error = ?e,
                        "Failed to parse SQLite singleton_config value — skipping row"
                    );
                    continue;
                }
            };

            let entry = root
                .entry(section)
                .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()));
            if let serde_json::Value::Object(section_map) = entry {
                section_map.insert(key, parsed);
            }
        }

        // Delegate the serde_json::Value → figment::value::Value
        // conversion to figment's own `Serialized` provider, which
        // owns the bridge for arbitrary Serialize types and is
        // version-pinned in lockstep with figment's value model.
        // Avoids hand-rolling a JSON → figment Value walker that
        // would silently rot the next time figment bumps its
        // value enum shape.
        Serialized::defaults(serde_json::Value::Object(root)).data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn fresh_backend() -> (Arc<SqliteBackend>, TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let db_path: PathBuf = tmp.path().join("opcgw.db");
        let backend = Arc::new(
            SqliteBackend::new(db_path.to_str().expect("utf-8 path"))
                .expect("backend"),
        );
        (backend, tmp)
    }

    #[test]
    fn provider_returns_empty_map_when_singleton_table_empty() {
        let (backend, _tmp) = fresh_backend();
        let provider = SqliteSingletonProvider::new(backend);
        let data = provider.data().expect("data ok");
        assert!(
            data.is_empty(),
            "expected empty Map when singleton_config has no rows, got {:?}",
            data
        );
    }

    #[test]
    fn provider_metadata_names_the_source() {
        let (backend, _tmp) = fresh_backend();
        let provider = SqliteSingletonProvider::new(backend);
        let meta = provider.metadata();
        assert!(
            meta.name.contains("singleton_config"),
            "metadata name should mention the source table; got {:?}",
            meta.name
        );
    }

    #[test]
    fn provider_returns_populated_map_after_section_write() {
        let (backend, _tmp) = fresh_backend();
        backend
            .write_singleton_section(
                "chirpstack",
                &[
                    (
                        "polling_frequency".to_string(),
                        "42".to_string(),
                    ),
                    (
                        "server_address".to_string(),
                        "\"http://example.test:8080\"".to_string(),
                    ),
                ],
            )
            .expect("write");
        let provider = SqliteSingletonProvider::new(backend);
        let data = provider.data().expect("data ok");
        // Figment uses Profile::Default for non-profile providers.
        let default_dict = data
            .get(&Profile::Default)
            .expect("Profile::Default present");
        let chirpstack = default_dict
            .get("chirpstack")
            .expect("chirpstack section present");
        // chirpstack is a Value::Dict at this layer; can't pattern-match
        // without the figment::value::Value enum import, so render via
        // Debug and check the shape contains the keys we wrote.
        let rendered = format!("{:?}", chirpstack);
        assert!(
            rendered.contains("polling_frequency"),
            "expected polling_frequency in {:?}",
            rendered
        );
        assert!(
            rendered.contains("server_address"),
            "expected server_address in {:?}",
            rendered
        );
    }

    // Note: the malformed-value-json skip path is exercised by the
    // integration test in `tests/d2_figment_provider.rs` (Test 8),
    // which can inject directly into SQLite via a public helper.
    // Keeping the unit test surface narrow to public APIs only.
}

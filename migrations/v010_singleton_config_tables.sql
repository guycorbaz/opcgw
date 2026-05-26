-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Copyright (c) [2024] Guy Corbaz
--
-- Story D-0: Singleton Configuration → SQLite Migration
--
-- Schema v010 adds the `singleton_config` table that holds the
-- `[global]` / `[chirpstack]` / `[opcua]` / `[web]` non-secret config
-- sections in a single canonical store, mirroring what C-6 (v009) did for
-- the `[[application]]` collection tree.
--
-- Design call (per D-0 spec § Dev Notes "Schema design call"): generic
-- key-value shape (Option A) — `(section, key, value)` PRIMARY KEY. The
-- value column is TEXT and holds either a scalar TOML lexeme
-- ("polling_frequency"=>"10", "debug"=>"true") or a JSON-encoded list
-- ("allowed_origins"=>'["http://127.0.0.1:8088"]'). Rust-side typing is
-- enforced by `AppConfig::validate` post-load; SQLite is transport only.
--
-- The four-section CHECK pins the section namespace at the schema level so
-- D-1's editor UI cannot create rogue sections without a schema migration.
-- Secrets (`[chirpstack].api_token`, `[opcua].user_password`) are NEVER
-- written to this table — they stay in `config/secrets.toml` per the
-- Story C-0 chmod-0600 pattern.

CREATE TABLE singleton_config (
    section TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    PRIMARY KEY (section, key),
    CHECK (section IN ('global', 'chirpstack', 'opcua', 'web'))
);

-- Section lookups (`SELECT key, value FROM singleton_config WHERE section
-- = ?1`) drive the boot-time AppConfig load + D-1's per-section editor.
-- The PRIMARY KEY already provides an index on (section, key), but a
-- dedicated section index keeps the section-scan path cheap on databases
-- where the primary-key BTree fragments over time.
CREATE INDEX idx_singleton_config_section ON singleton_config(section);

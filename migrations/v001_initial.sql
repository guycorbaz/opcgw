-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Copyright (c) [2024] Guy Corbaz
--
-- opcgw SQLite Schema - Version 001
-- Initial schema for opcgw persistence layer

-- Configure SQLite for reliability and performance
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;
PRAGMA temp_store = MEMORY;

-- Schema versioning: increment when adding new migrations
PRAGMA user_version = 1;

-- ============================================================================
-- Table: metric_values
-- Purpose: Current metric values (hot table)
-- Strategy: UPSERT (INSERT OR REPLACE) keyed on (device_id, metric_name)
-- Performance: <1ms lookup time via primary key
--
-- UPSERT Semantics:
-- - PRIMARY KEY (device_id, metric_name) enforces one row per device/metric pair
-- - INSERT OR REPLACE updates existing rows atomically
-- - created_at is preserved via COALESCE subquery to track first-insert time
-- - updated_at is always set to current timestamp on any insert/update
--
-- created_at preservation pattern:
--   INSERT OR REPLACE ... VALUES (..., COALESCE((SELECT created_at FROM metric_values
--   WHERE device_id=?1 AND metric_name=?2), ?timestamp))
--   This ensures: first insert -> created_at=now; subsequent updates -> created_at unchanged
-- ============================================================================
CREATE TABLE IF NOT EXISTS metric_values (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,               -- Serialized value (TEXT format for durability)
  data_type TEXT NOT NULL,           -- MetricType variant: Float, Int, Bool, String
  timestamp TEXT NOT NULL,           -- ISO8601 UTC format
  updated_at TEXT NOT NULL,          -- Last update time (set on every insert/update)
  created_at TEXT NOT NULL,          -- Creation time (preserved across UPSERT updates)
  UNIQUE(device_id, metric_name)
);

CREATE INDEX IF NOT EXISTS idx_metric_values_device_metric
  ON metric_values(device_id, metric_name);

-- ============================================================================
-- Table: metric_history
-- Purpose: Historical metric values (append-only audit log)
-- Strategy: INSERT only, DELETE only during pruning
-- Performance: Composite index (device_id, timestamp) for time-range queries
-- ============================================================================
CREATE TABLE IF NOT EXISTS metric_history (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,               -- JSON-serialized MetricValueInternal
  data_type TEXT NOT NULL,           -- Float, Int, Bool, String
  timestamp TEXT NOT NULL,           -- ISO8601 UTC format
  created_at TEXT NOT NULL           -- Record creation time
);

CREATE INDEX IF NOT EXISTS idx_metric_history_device_timestamp
  ON metric_history(device_id, timestamp);

-- ============================================================================
-- Table: command_queue
-- Purpose: Persistent FIFO command queue for device operations
-- Strategy: INSERT on queue, UPDATE on status change, DELETE on pruning
-- Performance: AUTOINCREMENT ID for FIFO ordering, (status, created_at) index
-- ============================================================================
CREATE TABLE IF NOT EXISTS command_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id TEXT NOT NULL,
  payload BLOB NOT NULL,             -- LoRaWAN frame data (max 250 bytes)
  f_port INTEGER NOT NULL,           -- 1-223 per LoRaWAN spec
  status TEXT NOT NULL,              -- Pending, Sent, Failed
  created_at TEXT NOT NULL,          -- Command creation timestamp (ISO8601)
  updated_at TEXT NOT NULL,          -- Last status change timestamp
  error_message TEXT,                -- Optional error description
  CHECK(f_port >= 1 AND f_port <= 223)
);

CREATE INDEX IF NOT EXISTS idx_command_queue_status_created
  ON command_queue(status, created_at);

-- ============================================================================
-- Table: gateway_status
-- Purpose: Key-value store for gateway health and state
-- Strategy: Flexible key-value pattern for operational metrics
-- Keys: server_available (bool), last_poll_time (ISO8601 or null), error_count (int)
-- ============================================================================
CREATE TABLE IF NOT EXISTS gateway_status (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- ============================================================================
-- Table: retention_config
-- Purpose: Data retention and pruning policies
-- Strategy: Per-table retention configuration for historical data
-- ============================================================================
CREATE TABLE IF NOT EXISTS retention_config (
  id INTEGER PRIMARY KEY,
  data_type TEXT NOT NULL UNIQUE,    -- metric_values or metric_history
  retention_days INTEGER NOT NULL,
  auto_delete BOOLEAN NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL
);

-- Initialize default retention policies
INSERT OR IGNORE INTO retention_config (data_type, retention_days, auto_delete, updated_at)
VALUES
  ('metric_values', 30, 1, datetime('now')),
  ('metric_history', 90, 1, datetime('now'));

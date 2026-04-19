-- schema/v001_initial.sql
-- Initial SQLite schema for opcgw (opcgw - OPC UA Gateway for ChirpStack)
-- Version 1: Core tables for metrics, commands, and gateway state
-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Copyright (c) 2024 Guy Corbaz

-- ============================================================================
-- PRAGMA Configuration
-- ============================================================================
-- Write-Ahead Logging mode enables concurrent readers while poller (single writer) owns write access
PRAGMA journal_mode = WAL;
-- Enable foreign key constraints (future-proofing for referential integrity)
PRAGMA foreign_keys = ON;
-- Balanced durability: crash-safe but faster than FULL synchronous mode
PRAGMA synchronous = NORMAL;

-- Schema version tracking for migration management
PRAGMA user_version = 1;

-- ============================================================================
-- Table 1: metric_values (Current metric state - UPSERT pattern)
-- ============================================================================
-- Purpose: Stores latest metric value for each (device_id, metric_name) pair
-- Pattern: INSERT OR REPLACE for UPSERT; unique constraint prevents stale values
-- Concurrency: Single writer (poller) via Mutex; readers use WAL concurrent access
CREATE TABLE IF NOT EXISTS metric_values (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,                 -- JSON-serialized MetricValueInternal
  data_type TEXT NOT NULL,             -- Float, Int, Bool, String (from MetricType enum)
  timestamp TEXT NOT NULL,             -- ISO8601 UTC format: "2026-04-19T12:34:56.789Z"
  updated_at TEXT NOT NULL,            -- Latest update time for change detection
  UNIQUE(device_id, metric_name)
);

CREATE INDEX IF NOT EXISTS idx_metric_values_device_metric
  ON metric_values(device_id, metric_name);

-- ============================================================================
-- Table 2: metric_history (Historical audit trail - append-only)
-- ============================================================================
-- Purpose: Immutable append-only audit trail of all metric values over time
-- Pattern: INSERT only; DELETE only for pruning (via retention policy)
-- Index: Composite (device_id, timestamp) enables efficient time-range queries
-- Concurrency: Append-safe under WAL; pruning task deletes old rows
CREATE TABLE IF NOT EXISTS metric_history (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,                 -- JSON-serialized MetricValueInternal
  data_type TEXT NOT NULL,             -- Float, Int, Bool, String
  timestamp TEXT NOT NULL,             -- ISO8601 UTC format
  created_at TEXT NOT NULL             -- When inserted (append timestamp)
);

CREATE INDEX IF NOT EXISTS idx_metric_history_device_timestamp
  ON metric_history(device_id, timestamp);

-- ============================================================================
-- Table 3: command_queue (FIFO command queueing for LoRaWAN downlinks)
-- ============================================================================
-- Purpose: Stores pending commands to send to LoRaWAN devices via ChirpStack
-- Pattern: FIFO via auto-increment ID; status filtering for "get pending"
-- Constraints: f_port ∈ [1, 223] per LoRaWAN spec; payload ≤ 250 bytes max
-- State machine: Pending → Sent → Failed (status field)
CREATE TABLE IF NOT EXISTS command_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id TEXT NOT NULL,
  payload BLOB NOT NULL,               -- LoRaWAN frame data (max 250 bytes per spec)
  f_port INTEGER NOT NULL,             -- Application port: valid range 1-223
  status TEXT NOT NULL,                -- Pending, Sent, Failed (from CommandStatus)
  created_at TEXT NOT NULL,            -- ISO8601 UTC: when command was queued
  updated_at TEXT NOT NULL,            -- Last status change time
  error_message TEXT,                  -- Nullable: error details if status='Failed'
  CHECK(f_port >= 1 AND f_port <= 223)
);

CREATE INDEX IF NOT EXISTS idx_command_queue_status_created
  ON command_queue(status, created_at);

-- ============================================================================
-- Table 4: gateway_status (Key-value store for server state)
-- ============================================================================
-- Purpose: Flexible key-value store for gateway-wide state and metrics
-- Pattern: Key-value; single row per key; atomic replace on update
-- Keys: "server_available" (bool), "last_poll_time" (ISO8601 or null), "error_count" (int)
-- Rationale: Trades query simplicity for schema flexibility (no migration needed for new keys)
CREATE TABLE IF NOT EXISTS gateway_status (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,                 -- Value stored as TEXT (parsed by caller)
  updated_at TEXT NOT NULL             -- ISO8601 UTC: last update time
);

-- ============================================================================
-- Table 5: retention_config (Data retention policy configuration)
-- ============================================================================
-- Purpose: Configurable retention periods per data type (metric_values vs metric_history)
-- Pattern: Key-value with unique constraint on data_type
-- Columns: data_type, retention_days (how long to keep), auto_delete (enable pruning)
-- Usage: Pruning task queries this table to determine what rows to delete
CREATE TABLE IF NOT EXISTS retention_config (
  id INTEGER PRIMARY KEY,
  data_type TEXT NOT NULL UNIQUE,     -- metric_values or metric_history
  retention_days INTEGER NOT NULL,     -- How many days to retain (default: 30 for values, 90 for history)
  auto_delete BOOLEAN NOT NULL DEFAULT 1,  -- Enable/disable automatic pruning
  updated_at TEXT NOT NULL             -- ISO8601 UTC: last config update
);

-- ============================================================================
-- Initial Data
-- ============================================================================
-- Initialize retention_config with sensible defaults
-- INSERT OR IGNORE ensures idempotence (safe to re-run migration)
INSERT OR IGNORE INTO retention_config (data_type, retention_days, auto_delete, updated_at)
VALUES
  ('metric_values', 30, 1, datetime('now')),
  ('metric_history', 90, 1, datetime('now'));

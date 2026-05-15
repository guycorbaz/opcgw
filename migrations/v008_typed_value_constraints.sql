-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Copyright (c) [2024] Guy Corbaz
--
-- Migration v008: Cross-Column CHECK Constraints on Typed Value Columns (Epic A, Story A-3)
-- Purpose: Add table-level CHECK constraints to metric_values and metric_history
--          enforcing the exactly-one-non-NULL invariant per `value_type` discriminant:
--            value_type='legacy'  → all 4 typed columns NULL
--            value_type='Float'   → value_real NOT NULL, others NULL
--            value_type='Int'     → value_int NOT NULL, others NULL
--            value_type='Bool'    → value_bool NOT NULL, others NULL
--            value_type='String'  → value_text NOT NULL, others NULL
-- Used by:  Stories A-4, A-5, A-6 (readers can trust the invariant).
-- Scope:    Becomes provable only AFTER A-3's writer rewiring (`SqliteBackend::set_metric`,
--           `upsert_metric_value`, `append_metric_history`, `batch_write_metrics` populate
--           typed columns + `value_type` from `MetricType` pattern-match). Adding it before
--           A-3 writers shipped (i.e. in A-2) would have rejected every legacy-shaped INSERT.
-- Pattern:  SQLite's ALTER TABLE does NOT support adding table-level CHECK constraints.
--           Uses the standard CREATE TABLE … AS SELECT recreate pattern: O(table-size).
--           Wrapped in explicit BEGIN/COMMIT to give v008 atomic guarantees (partial close
--           of A-1-iter3-DEF6 / A-2-iter1-DEF-IH1 for v008 specifically).

BEGIN TRANSACTION;

-- ============================================================================
-- metric_values: recreate with cross-column CHECK constraint
-- ============================================================================
-- Preserves: id, all 8 base columns from v001, all 5 typed columns from v007,
-- the value_bool IN (0,1) CHECK from v007, the value_type whitelist CHECK
-- from v007, plus the cross-column CHECK.
CREATE TABLE metric_values_new (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,
  data_type TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  value_real REAL NULL,
  value_int INTEGER NULL,
  value_bool INTEGER NULL CHECK(value_bool IS NULL OR value_bool IN (0, 1)),
  value_text TEXT NULL,
  value_type TEXT NOT NULL DEFAULT 'legacy'
    CHECK(value_type IN ('legacy', 'Float', 'Int', 'Bool', 'String')),
  CHECK (
    (value_type = 'legacy' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Float' AND value_real IS NOT NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Int' AND value_real IS NULL AND value_int IS NOT NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Bool' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NOT NULL AND value_text IS NULL)
    OR (value_type = 'String' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NOT NULL)
  ),
  UNIQUE(device_id, metric_name)
);

INSERT INTO metric_values_new
  SELECT id, device_id, metric_name, value, data_type, timestamp, updated_at, created_at,
         value_real, value_int, value_bool, value_text, value_type
  FROM metric_values;

DROP TABLE metric_values;
ALTER TABLE metric_values_new RENAME TO metric_values;

CREATE INDEX IF NOT EXISTS idx_metric_values_device_metric
  ON metric_values(device_id, metric_name);

-- ============================================================================
-- metric_history: same payload-bearing CHECK pattern, different base columns
-- ============================================================================
-- KEY DIFFERENCES from metric_values:
--   - no updated_at column (history rows are immutable on insert);
--   - no UNIQUE constraint (multiple rows per metric over time);
--   - index is idx_metric_history_device_timestamp for time-range queries.
CREATE TABLE metric_history_new (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,
  data_type TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  created_at TEXT NOT NULL,
  value_real REAL NULL,
  value_int INTEGER NULL,
  value_bool INTEGER NULL CHECK(value_bool IS NULL OR value_bool IN (0, 1)),
  value_text TEXT NULL,
  value_type TEXT NOT NULL DEFAULT 'legacy'
    CHECK(value_type IN ('legacy', 'Float', 'Int', 'Bool', 'String')),
  CHECK (
    (value_type = 'legacy' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Float' AND value_real IS NOT NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Int' AND value_real IS NULL AND value_int IS NOT NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Bool' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NOT NULL AND value_text IS NULL)
    OR (value_type = 'String' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NOT NULL)
  )
);

INSERT INTO metric_history_new
  SELECT id, device_id, metric_name, value, data_type, timestamp, created_at,
         value_real, value_int, value_bool, value_text, value_type
  FROM metric_history;

DROP TABLE metric_history;
ALTER TABLE metric_history_new RENAME TO metric_history;

CREATE INDEX IF NOT EXISTS idx_metric_history_device_timestamp
  ON metric_history(device_id, timestamp);

COMMIT;

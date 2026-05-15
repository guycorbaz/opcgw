-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Copyright (c) [2024] Guy Corbaz
--
-- Migration v007: Typed Value Columns for Storage Payload Migration (Epic A, Story A-2)
-- Purpose: Add typed value columns to metric_values and metric_history so the
--          payload-bearing MetricType enum from Story A-1 (Float(f64) / Int(i64) /
--          Bool(bool) / String(String)) can survive the persistence layer with
--          type fidelity. Closes the schema gap that blocks Story A-3 (poller
--          value-payload write pipeline) and the downstream A-4 / A-5 / A-6 reads.
-- Used by: Story A-3 (writers populate typed columns from MetricType payload);
--          Stories A-4, A-5, A-6 (readers consume typed columns).
-- Scope: Schema-only DDL. Writers and readers in src/storage/sqlite.rs are NOT
--        modified in A-2; they continue to populate / consume the legacy
--        `value TEXT NOT NULL` + `data_type TEXT NOT NULL` columns. Pre-A-2 rows
--        are tagged value_type='legacy' via the column default — no UPDATE
--        statement is required.

-- ============================================================================
-- metric_values: add typed value columns + discriminant
-- ============================================================================
ALTER TABLE metric_values ADD COLUMN value_real REAL NULL;
ALTER TABLE metric_values ADD COLUMN value_int  INTEGER NULL;
ALTER TABLE metric_values ADD COLUMN value_bool INTEGER NULL
    CHECK(value_bool IS NULL OR value_bool IN (0, 1));
ALTER TABLE metric_values ADD COLUMN value_text TEXT NULL;
ALTER TABLE metric_values ADD COLUMN value_type TEXT NOT NULL DEFAULT 'legacy'
    CHECK(value_type IN ('legacy', 'Float', 'Int', 'Bool', 'String'));

-- ============================================================================
-- metric_history: same column additions for the HistoryRead path
-- ============================================================================
ALTER TABLE metric_history ADD COLUMN value_real REAL NULL;
ALTER TABLE metric_history ADD COLUMN value_int  INTEGER NULL;
ALTER TABLE metric_history ADD COLUMN value_bool INTEGER NULL
    CHECK(value_bool IS NULL OR value_bool IN (0, 1));
ALTER TABLE metric_history ADD COLUMN value_text TEXT NULL;
ALTER TABLE metric_history ADD COLUMN value_type TEXT NOT NULL DEFAULT 'legacy'
    CHECK(value_type IN ('legacy', 'Float', 'Int', 'Bool', 'String'));

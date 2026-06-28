-- SPDX-License-Identifier: MIT OR Apache-2.0
-- (c) [2026] Guy Corbaz
--
-- Migration v013 — Story G-4 (#127): dashboard error drill-down.
--
-- Adds a bounded error-event feed so the dashboard's cumulative error count
-- can drill down to the actual recent errors (timestamp, category, the
-- offending device/application, and a sanitized message). Retention is bounded
-- by DEFAULT_ERROR_EVENT_CAP (overridable via OPCGW_ERROR_EVENT_CAP): every
-- insert prunes rows beyond the cap (ring-buffer discipline). This records
-- discrete events only — no aggregation (#130).
CREATE TABLE IF NOT EXISTS error_events (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    ts             TEXT    NOT NULL,
    category       TEXT    NOT NULL,
    device_id      TEXT,
    application_id TEXT,
    message        TEXT    NOT NULL
);

-- Newest-first reads (`ORDER BY id DESC LIMIT ?`) and the prune-to-cap delete
-- both walk the id index; the PK already provides it, but an explicit index on
-- ts keeps time-range reads cheap if added later.
CREATE INDEX IF NOT EXISTS idx_error_events_id ON error_events (id DESC);

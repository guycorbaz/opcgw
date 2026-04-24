-- Migration v005: Add gateway_status table for operational health tracking
-- Purpose: Track poller health metrics (last poll time, error counts, connectivity state)
-- Used by: ChirpstackPoller (Story 4-1) to persist gateway health metrics to SQLite

CREATE TABLE IF NOT EXISTS gateway_status (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

-- Index on updated_at for efficient time-based queries (e.g., "last 24 hours of status changes")
CREATE INDEX IF NOT EXISTS idx_gateway_status_updated_at ON gateway_status(updated_at);

-- Initial health status rows (will be updated by poller on each poll cycle)
INSERT OR IGNORE INTO gateway_status (key, value, updated_at)
VALUES
    ('last_successful_poll', '1970-01-01T00:00:00.000000Z', '1970-01-01T00:00:00.000000Z'),
    ('error_count', '0', '1970-01-01T00:00:00.000000Z'),
    ('chirpstack_available', 'false', '1970-01-01T00:00:00.000000Z');

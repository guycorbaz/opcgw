-- Migration v006: Restructure gateway_status table for health metrics (Story 5-3)
-- Purpose: Replace key-value schema with structured health metrics schema
-- Used by: OPC UA server (Story 5-3) to expose gateway health to clients
-- Changes: Recreate table with new structure, extract meaningful values from old key-value format

-- Drop old index first (can be done before table changes)
DROP INDEX IF EXISTS idx_gateway_status_updated_at;

-- Create new gateway_status table with structured health metrics schema
CREATE TABLE IF NOT EXISTS gateway_status_new (
    id INTEGER PRIMARY KEY DEFAULT 1,
    last_poll_timestamp TEXT,
    error_count INTEGER DEFAULT 0,
    chirpstack_available BOOLEAN DEFAULT false
);

-- Migrate data from old key-value format to new structured format
-- Extract only meaningful values (ignore the default "1970-01-01" epoch timestamp)
INSERT INTO gateway_status_new (id, last_poll_timestamp, error_count, chirpstack_available)
SELECT
    1 as id,
    CASE
        WHEN key = 'last_successful_poll' AND value != '1970-01-01T00:00:00.000000Z' THEN value
        ELSE NULL
    END as last_poll_timestamp,
    CASE
        WHEN key = 'error_count' THEN CAST(value AS INTEGER)
        ELSE 0
    END as error_count,
    CASE
        WHEN key = 'chirpstack_available' THEN (value = 'true')
        ELSE false
    END as chirpstack_available
FROM gateway_status
WHERE key IN ('last_successful_poll', 'error_count', 'chirpstack_available')
GROUP BY 1;

-- If migration didn't insert anything, insert defaults
INSERT OR IGNORE INTO gateway_status_new (id, last_poll_timestamp, error_count, chirpstack_available)
VALUES (1, NULL, 0, false);

-- Drop old table
DROP TABLE gateway_status;

-- Rename new table to gateway_status
ALTER TABLE gateway_status_new RENAME TO gateway_status;

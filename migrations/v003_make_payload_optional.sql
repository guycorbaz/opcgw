-- Migration v003: Make payload and f_port optional for new Command struct
-- Recreate command_queue table to allow NULL payload and f_port
-- This allows the new high-level Command struct (Story 3-1) to be used alongside legacy DeviceCommand

-- Step 1: Create temporary table with new schema
CREATE TABLE command_queue_new (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id TEXT NOT NULL,
  payload BLOB,                       -- NULL for high-level commands (Story 3-1)
  f_port INTEGER,                     -- NULL for high-level commands (Story 3-1)
  command_name TEXT,                  -- New: high-level command name
  parameters TEXT,                    -- New: JSON parameters for validation
  status TEXT NOT NULL,               -- Pending, Sent, Failed
  created_at TEXT NOT NULL,           -- Command creation timestamp
  updated_at TEXT NOT NULL,           -- Last status change timestamp
  enqueued_at TEXT,                   -- New: enqueue time for high-level commands
  sent_at TEXT,                       -- New: when command was sent
  confirmed_at TEXT,                  -- New: when command was confirmed
  error_message TEXT,                 -- Optional error description
  command_hash TEXT,                  -- New: SHA256 hash for deduplication
  chirpstack_result_id TEXT,          -- New: result ID from ChirpStack
  CHECK(f_port IS NULL OR (f_port >= 1 AND f_port <= 223))
);

-- Step 2: Copy existing data
INSERT INTO command_queue_new SELECT
  id, device_id, payload, f_port, NULL, NULL, status, created_at, updated_at,
  NULL, NULL, NULL, error_message, NULL, NULL
FROM command_queue;

-- Step 3: Drop old table and rename new one
DROP TABLE command_queue;
ALTER TABLE command_queue_new RENAME TO command_queue;

-- Step 4: Recreate index
CREATE INDEX IF NOT EXISTS idx_command_queue_status_created
  ON command_queue(status, created_at);

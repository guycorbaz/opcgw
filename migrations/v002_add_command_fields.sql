ALTER TABLE command_queue ADD COLUMN command_name TEXT;
ALTER TABLE command_queue ADD COLUMN parameters TEXT;
ALTER TABLE command_queue ADD COLUMN enqueued_at TEXT;
ALTER TABLE command_queue ADD COLUMN sent_at TEXT;
ALTER TABLE command_queue ADD COLUMN confirmed_at TEXT;
ALTER TABLE command_queue ADD COLUMN command_hash TEXT;
ALTER TABLE command_queue ADD COLUMN chirpstack_result_id TEXT;

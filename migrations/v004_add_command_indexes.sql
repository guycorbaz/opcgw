-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Copyright (c) [2024] Guy Corbaz
--
-- opcgw SQLite Schema - Version 004
-- Add indexes for efficient command delivery status polling

-- Index on (status, sent_at) for timeout handler queries
-- Used by: find_timed_out_commands() to efficiently find sent commands past TTL
CREATE INDEX IF NOT EXISTS idx_command_queue_status_sent_at
  ON command_queue(status, sent_at);

-- Index on (status, confirmed_at) for confirmation status queries
-- Used by: find_pending_confirmations() to efficiently find commands awaiting confirmation
CREATE INDEX IF NOT EXISTS idx_command_queue_status_confirmed_at
  ON command_queue(status, confirmed_at);

# Story 3-1: SQLite-Backed FIFO Command Queue

**Epic:** 3 (Reliable Command Execution)  
**Phase:** Phase 3 (Phase A)  
**Status:** ready-for-dev  
**Created:** 2026-04-22  
**Author:** Guy Corbaz (Project Lead)  

---

## Objective

Implement a durable, FIFO command queue backed by SQLite to enable reliable command delivery from gateway to ChirpStack devices. Commands are enqueued by the OPC UA server (write requests), processed in FIFO order, and tracked through delivery lifecycle.

---

## Acceptance Criteria

### AC#1: FIFO Queue Ordering
- Commands are delivered to ChirpStack devices in enqueue order
- Ordering verified across application restarts (persisted to SQLite)
- Concurrent enqueue/dequeue operations maintain FIFO semantics
- **Verification:** Unit test with 100 commands enqueued, verify ChirpStack order matches enqueue sequence

### AC#2: Queue Persistence & Recovery
- Unenqueued commands survive application crash/restart
- On startup, queue is restored from SQLite in original order
- Partially processed commands (enqueued but not yet sent) resume from queue, not retried
- **Verification:** Integration test: crash simulator, restart, verify queue state

### AC#3: Command Enqueue API
- Commands accepted from OPC UA server with device ID, metric ID, command name, parameters
- Enqueue operation idempotent: duplicate command IDs skip requeue (command deduplication)
- Enqueue timeout: 5s max (fail fast if queue unavailable)
- **Verification:** Unit test: enqueue 10 commands in rapid succession, verify no duplicates

### AC#4: Queue Capacity & Overflow
- Queue max capacity: 10,000 commands (configurable)
- When full, new enqueue attempts rejected with clear error (backpressure)
- Operator observability: gauge metric tracking queue depth (0-100%)
- **Verification:** Stress test: enqueue until capacity, verify rejection, confirm gauge accuracy

### AC#5: Command Lifecycle Tracking
- Each queued command has state: `pending` → `sent` → `confirmed` (or `failed`)
- State transitions logged with timestamp for audit trail
- Query API allows inspection of queue state by device/command
- **Verification:** Unit test: verify state machine transitions, timestamp presence

### AC#6: Concurrent Access Safety
- Multiple OPC UA clients can enqueue commands simultaneously
- Queue operations (enqueue/dequeue/status) are thread-safe via Mutex
- No data races on SQLite writes (per-connection isolation via chirpstack.rs)
- **Verification:** Concurrent unit test: 10 tasks enqueue in parallel, verify final count matches

### AC#7: SQLite Schema Design
- Table: `command_queue` (id, device_id, metric_id, command_name, parameters_json, enqueued_at, sent_at, status)
- Primary key: auto-incrementing id (ROWID) for FIFO ordering
- Index on status (for efficient pending command queries)
- Index on device_id (for device-specific queries)
- **Verification:** Schema review, query plan analysis

### AC#8: Command Dequeue API
- Dequeue returns next pending command by ROWID order
- Atomic dequeue + status update (no race conditions)
- Dequeue timeout: 1s (configurable, for polling)
- **Verification:** Unit test: concurrent enqueue/dequeue, verify no command loss

---

## Technical Approach

### Data Model

```
CREATE TABLE command_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id TEXT NOT NULL,
  metric_id TEXT NOT NULL,
  command_name TEXT NOT NULL,
  parameters_json TEXT,
  enqueued_at TEXT NOT NULL,     -- RFC3339 microsecond
  sent_at TEXT,                  -- RFC3339 microsecond or NULL
  status TEXT NOT NULL,          -- 'pending', 'sent', 'confirmed', 'failed'
  command_hash TEXT NOT NULL,    -- SHA256(device_id + command_name + parameters) for dedup
  created_by TEXT,               -- OPC UA client identifier (optional)
  error_message TEXT             -- Human-readable error if status='failed'
);

CREATE INDEX idx_status ON command_queue(status);
CREATE INDEX idx_device_id ON command_queue(device_id);
CREATE UNIQUE INDEX idx_command_hash ON command_queue(command_hash) WHERE status='pending';
```

### StorageBackend Extension

Add methods to `StorageBackend` trait:
- `enqueue_command(&self, cmd: Command) -> Result<u32>` — returns command ID
- `dequeue_command(&self) -> Result<Option<Command>>` — returns oldest pending or None
- `update_command_status(&self, id: u32, status: CommandStatus) -> Result<()>`
- `list_commands(&self, filter: CommandFilter) -> Result<Vec<Command>>`
- `get_queue_depth(&self) -> Result<usize>`

### Command Struct

```rust
pub struct Command {
    pub id: u32,
    pub device_id: String,
    pub metric_id: String,
    pub command_name: String,
    pub parameters: serde_json::Value,
    pub enqueued_at: SystemTime,
    pub sent_at: Option<SystemTime>,
    pub status: CommandStatus,
}

pub enum CommandStatus {
    Pending,
    Sent,
    Confirmed,
    Failed(String),
}
```

### Implementation Steps

1. **Define command queue schema** in `src/storage/sqlite.rs`
   - Implement migration logic (one-time creation if table doesn't exist)
   - Add UNIQUE index on command_hash for deduplication

2. **Implement StorageBackend command methods**
   - `enqueue_command()`: compute command_hash, check dedup, insert row, return id
   - `dequeue_command()`: SELECT ... WHERE status='pending' ORDER BY id LIMIT 1, update status to 'sent'
   - `update_command_status()`: UPDATE ... WHERE id = ? SET status = ?, sent_at = ?
   - All wrapped in transaction for atomicity

3. **Add command queue gauge metric**
   - Hook `get_queue_depth()` into metrics (tracing instrumentation)
   - Track gauge: "command_queue_depth" with label "status" (pending/sent/failed)

4. **Test Command Struct serialization/deserialization**
   - JSON parameters round-trip without data loss
   - Timestamp precision preserved (use microsecond formatting from Story 2-5b)

5. **Integration with InMemoryBackend** (for testing)
   - Implement same command queue API using Vec + Mutex
   - No deduplication (for unit test simplicity)

---

## Assumptions & Constraints

- **No distributed queue:** Single-instance SQLite (not sharded across gateways)
- **Command parameters:** JSON-serializable types only (enforced by serde)
- **Enqueue source:** Currently OPC UA server only (future: REST API, MQTT)
- **Command ACK:** Caller responsible for confirming completion (Story 3-3 handles status updates)
- **Ordering guarantee:** FIFO by ROWID, not by device (all devices share single queue)

---

## File List

**New Files:**
- None (schema embedded in migration logic)

**Modified Files:**
- `src/storage/mod.rs` — Add Command, CommandStatus, CommandFilter types
- `src/storage/sqlite.rs` — Add command queue table creation, enqueue/dequeue methods
- `src/storage/inmemory.rs` — Add in-memory command queue for testing
- `Cargo.toml` — No new dependencies (use existing serde_json)

**Test Files:**
- `tests/command_queue_tests.rs` — New file: 15-20 test cases covering all AC

---

## Dev Notes

### Decision: FIFO via ROWID vs Timestamp
Using SQLite's AUTOINCREMENT ROWID ensures strict FIFO even if system clock regresses (Story 2-5b vulnerability). Alternative: ORDER BY enqueued_at would be susceptible to clock skew. Chose ROWID.

### Decision: Deduplication via Hash
Command dedup prevents OPC UA write retries from creating duplicate queue entries. Hash key: SHA256(device_id + command_name + parameters_json). Collision risk negligible for this domain. Index is UNIQUE + partial (WHERE status='pending') to allow dedup only for pending—sent/confirmed commands can be requeued by operator.

### Decision: Single Queue vs Per-Device Queues
Single FIFO queue simplifies implementation and ensures fairness (no starving devices). Per-device queues would be needed if some devices are slow, but Story 3-1 scope is basic FIFO. Future optimization if needed.

### Clock Skew Handling
Timestamp stored in RFC3339 microsecond format (from Story 2-5b). Clock regression handled via checked_duration_since() if audit trail needs timestamp diffs. Not critical for queue ordering (ROWID is source of truth).

---

## Acceptance Checklist

- [ ] StorageBackend trait has all 5 command queue methods with doc comments
- [ ] SQLite schema created with AUTOINCREMENT id, status/device_id indexes
- [ ] Deduplication working (UNIQUE index on command_hash, WHERE status='pending')
- [ ] InMemoryBackend implements command queue (Vec-backed for tests)
- [ ] All 15+ unit tests passing
- [ ] Integration test: restart scenario, queue persists
- [ ] Gauge metric "command_queue_depth" emitting (count by status)
- [ ] Timestamps in RFC3339 microsecond format (consistent with Story 2-5b)
- [ ] Code review signoff: no clippy warnings, no unsafe code
- [ ] SPDX license headers on all new code

---

## References

- **Epic 3 Planning:** `_bmad-output/implementation-artifacts/epic-3-planning.md`
- **Story 2-5b (Queue Inspiration):** Timestamp precision, error handling patterns
- **CLAUDE.md:** Configuration, build commands, architecture

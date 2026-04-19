# Epic 2: Data Persistence — Refined Story Breakdown

**Date:** 2026-04-19  
**Original Stories:** 5  
**Refined Stories:** 12  
**Rationale:** Tighter granularity for better control, visibility, and risk mitigation

---

## Story Dependency Graph

```
2-1a: Trait Definition
    ↓
2-1b: Supporting Types (MetricType, DeviceCommand, ChirpstackStatus)
    ↓
2-1c: InMemoryBackend Implementation
    ↓
2-2a: SQLite Schema Design
    ↓
2-2b: Schema Creation & Versioning
    ↓
2-2c: SqliteBackend Basic CRUD
    ↓
2-2d: Prepared Statements & SQL Safety
    ↓
2-3a: Metric Values Persistence (UPSERT)
    ↓
2-3b: Historical Metrics Append-Only
    ↓
2-3c: Batch Write Optimization
    ↓
2-4a: Metric Restore on Startup
    ↓
2-4b: Graceful Degradation (Missing/Corrupted DB)
    ↓
2-5a: Historical Data Pruning Task
```

---

## Refined Stories

### **2-1a: StorageBackend Trait Definition**

**Original:** Part of Story 2-1  
**Duration:** ~1-2 days

As a **developer**,
I want a clean `StorageBackend` trait with a well-defined interface,
So that all storage implementations follow the same contract.

**Acceptance Criteria:**
- `StorageBackend` trait defined in `src/storage/mod.rs`
- Trait methods for:
  - Metric get/set operations
  - Gateway status operations
  - Command queue operations
- Trait supports concurrent access (Arc-safe)
- Documentation on trait design philosophy
- No implementation code (interface only)

**Dev Notes:**
- Trait design should be minimal: get_metric(), set_metric(), get_status(), update_status(), queue_command(), get_commands()
- Error type: OpcGwError::Storage
- All methods are sync (blocking) — async wrapper layer can be added in Epic 4 if needed

---

### **2-1b: Core Storage Data Types**

**Original:** Part of Story 2-1  
**Duration:** ~1-2 days

As a **developer**,
I want well-defined data types for metrics, commands, and gateway state,
So that the storage layer has type safety and clear semantics.

**Acceptance Criteria:**
- `MetricType` enum: Float, Int, Bool, String
- `MetricValue` struct: device_id, metric_name, value, timestamp, data_type
- `DeviceCommand` struct: device_id, payload, f_port, status, created_at, error_message
- `ChirpstackStatus` struct: server_available, last_poll_time, error_count
- `CommandStatus` enum: Pending, Sent, Failed
- All types in `src/storage/mod.rs`
- Unit tests for serialization/deserialization if applicable

**Dev Notes:**
- Use `chrono::DateTime<Utc>` for timestamps
- MetricValue.value is stored as String for flexibility (SQLite TEXT column)
- DeviceCommand.f_port is u8

---

### **2-1c: InMemoryBackend Implementation**

**Original:** Part of Story 2-1  
**Duration:** ~1-2 days

As a **developer**,
I want an in-memory storage backend for unit tests,
So that tests can run without SQLite dependency.

**Acceptance Criteria:**
- `InMemoryBackend` struct in `src/storage/memory.rs`
- Implements `StorageBackend` trait
- Uses HashMap for metric storage
- Thread-safe via Arc<Mutex<>> or RwLock
- All existing unit tests pass using `InMemoryBackend`
- No file I/O, pure in-memory HashMap

**Dev Notes:**
- Storage structure: HashMap<device_id, HashMap<metric_name, MetricValue>>
- Command queue: Vec<DeviceCommand> with FIFO ordering
- Test helper: `fn new_memory_backend() -> InMemoryBackend`

---

### **2-2a: SQLite Schema Design & Documentation**

**Original:** Part of Story 2-2  
**Duration:** ~1-2 days

As a **developer**,
I want a well-designed SQLite schema that supports metrics, commands, and state,
So that the database structure is optimal and documented.

**Acceptance Criteria:**
- Schema design document with 5 tables:
  - `metric_values` (device_id, metric_name, value, timestamp, data_type) — PRIMARY KEY (device_id, metric_name)
  - `metric_history` (id, device_id, metric_name, value, timestamp) — append-only, indexed on (device_id, timestamp)
  - `command_queue` (id, device_id, payload, f_port, status, created_at, error_message) — indexed on (status, created_at)
  - `gateway_status` (key, value) — key-value store for server_available, last_poll_time, error_count
  - `retention_config` (metric_id, retention_days) — retention rules per metric (future use)
- Rationale for each table and column choice
- Index strategy documented
- Schema versioning approach documented (pragma version or migration table)
- SQL file created but not yet embedded

**Dev Notes:**
- Use WAL journal mode for crash safety
- Schema SQL: `schema/v001_initial.sql`
- Version stored as pragma `user_version = 1`

---

### **2-2b: Schema Creation & Migration Infrastructure**

**Original:** Part of Story 2-2  
**Duration:** ~1-2 days

As a **developer**,
I want automatic schema creation on first run with support for future migrations,
So that the gateway initializes cleanly without manual SQL.

**Acceptance Criteria:**
- `migrations/` directory with embedded SQL files
- `v001_initial.sql` embedded via `include_str!()`
- Schema versioning using `pragma user_version`
- `fn init_schema(conn: &Connection) -> Result<()>` function
- On first run: detect version = 0, run v001_initial.sql, set version = 1
- On future runs: detect version, run pending migrations
- Unit test verifies schema creation from scratch
- Integration test verifies idempotence (running twice is safe)

**Dev Notes:**
- Embedded SQL avoids runtime file dependency
- `include_str!()` loads SQL at compile time
- Migration runner pattern ready for v002, v003, etc. in future

---

### **2-2c: SqliteBackend Basic CRUD Implementation**

**Original:** Part of Story 2-2  
**Duration:** ~2-3 days

As a **developer**,
I want a working SQLite backend that implements the StorageBackend trait,
So that metrics can be persisted to disk.

**Acceptance Criteria:**
- `SqliteBackend` struct in `src/storage/sqlite.rs`
- Implements `StorageBackend` trait
- Opens SQLite in WAL mode (crash safety)
- `new(path: &str) -> Result<Self>` constructor
- Schema auto-created on first run
- Basic CRUD for metrics:
  - `get_metric(device_id, metric_name) -> Option<MetricValue>`
  - `set_metric(device_id, metric_name, MetricValue) -> Result<()>`
- Basic CRUD for commands:
  - `queue_command(command) -> Result<()>`
  - `get_pending_commands() -> Result<Vec<DeviceCommand>>`
  - `update_command_status(command_id, status) -> Result<()>`
- Gateway status CRUD:
  - `get_status() -> Result<ChirpstackStatus>`
  - `update_status(status) -> Result<()>`
- Integration tests for basic CRUD
- Error handling: corrupted database, permission denied, etc.

**Dev Notes:**
- Each method performs its own database query (no transaction yet)
- Prepared statements NOT used yet (see Story 2-2d)
- Simple error propagation: `rusqlite::Error -> OpcGwError::Storage`

---

### **2-2d: Prepared Statements & SQL Injection Prevention**

**Original:** Part of Story 2-2  
**Duration:** ~1-2 days

As a **developer**,
I want safe, efficient SQL queries using prepared statements,
So that SQL injection is prevented and performance is optimized.

**Acceptance Criteria:**
- All queries in `SqliteBackend` use prepared statements
- No format!() or string concatenation in SQL
- `fn create_prepared_statements() -> Result<PreparedStatements>` struct
- Common queries cached:
  - SELECT metric_values WHERE device_id=? AND metric_name=?
  - INSERT OR REPLACE INTO metric_values
  - INSERT INTO metric_history
  - SELECT * FROM command_queue WHERE status = ? ORDER BY created_at
  - UPDATE command_queue SET status = ? WHERE id = ?
- Unit tests verify parameterized queries
- Performance benchmark: 400 metric writes <500ms

**Dev Notes:**
- Create a `PreparedStatements` struct holding all statement handles
- Bind values using `params![]` macro from rusqlite
- Rebuild metrics write query to use batch INSERT with multiple rows (story 2-3c focuses on batching optimization)

---

### **2-3a: Metric Values Persistence (UPSERT)**

**Original:** Part of Story 2-3  
**Duration:** ~1-2 days

As an **operator**,
I want last-known metric values saved to SQLite after each poll,
So that values survive a gateway restart.

**Acceptance Criteria:**
- Add to `SqliteBackend`: `fn persist_metric_values(metrics: Vec<MetricValue>) -> Result<()>`
- Uses UPSERT (INSERT OR REPLACE) keyed on (device_id, metric_name)
- Stores: device_id, metric_name, value (as text), data_type, timestamp
- Integration test: poll 10 devices with 5 metrics each, verify all in metric_values table
- Verify that re-inserting same metric overwrites previous value

**Dev Notes:**
- Single row UPSERT per metric (not yet batched)
- Timestamp is server time, not device time (for now)
- MetricValue.value serialized to text (handle Float precision)

---

### **2-3b: Historical Metrics Append-Only Storage**

**Original:** Part of Story 2-3  
**Duration:** ~1-2 days

As an **operator**,
I want historical metric data stored for trend analysis and auditing,
So that I can see how metrics changed over time.

**Acceptance Criteria:**
- Add to `SqliteBackend`: `fn append_metric_history(metrics: Vec<MetricValue>) -> Result<()>`
- Each metric write appends to metric_history (never updates, pure append)
- Columns: id (auto), device_id, metric_name, value, timestamp, data_type
- Index on (device_id, metric_name, timestamp) for efficient range queries
- Integration test: verify 100 metric_history rows after 10 poll cycles
- Verify data integrity: no lost rows, correct ordering by timestamp

**Dev Notes:**
- metric_history is write-only from this story's perspective
- Reads happen in Story 2-4a (restore) and Epic 7 (historical queries)
- Each row gets a unique auto-increment id

---

### **2-3c: Batch Write Optimization & Transaction Handling**

**Original:** Part of Story 2-3  
**Duration:** ~2-3 days

As an **operator**,
I want metric writes optimized for batch performance,
So that 400 metrics per poll cycle complete in <500ms.

**Acceptance Criteria:**
- Poller calls `fn persist_batch_metrics(metrics: Vec<MetricValue>) -> Result<()>`
- All metric_values UPSERTs in a single transaction
- All metric_history INSERTs in a single transaction
- Batch write for 400 metrics completes in <500ms (NFR3)
- Prepared statements reused across batch
- Integration test: measure and assert <500ms for 400 metrics
- Benchmark test: 1000, 5000 metrics (for future reference)

**Dev Notes:**
- Use `BEGIN TRANSACTION; ... COMMIT;` for atomic writes
- Prepare all statements upfront, reuse in loop
- Handle transaction rollback on error
- This is where batching really matters for performance

---

### **2-4a: Metric Restore on Startup**

**Original:** Part of Story 2-4  
**Duration:** ~1-2 days

As an **operator**,
I want last-known metrics loaded into OPC UA on startup,
So that SCADA clients see valid data immediately.

**Acceptance Criteria:**
- Main.rs: after loading config, before starting poller:
  - Open SQLite connection
  - Query metric_values table for all metrics
  - Add each metric to OPC UA address space with its last value
- Startup with 100 devices completes in <10 seconds (NFR4)
- Test: insert 100 metrics, restart gateway, verify all in OPC UA

**Dev Notes:**
- New initialization phase in main.rs after config load
- Query: SELECT device_id, metric_name, value, data_type FROM metric_values
- Add variables to OPC UA server before it starts accepting connections

---

### **2-4b: Graceful Degradation (Missing or Corrupted Database)**

**Original:** Part of Story 2-4  
**Duration:** ~1-2 days

As an **operator**,
I want the gateway to start cleanly even if the database is missing or corrupted,
So that brief data loss is better than a complete failure.

**Acceptance Criteria:**
- If database file doesn't exist: create it fresh, gateway starts with empty state (no error)
- If database is corrupted:
  - Attempt repair (SQLite PRAGMA integrity_check)
  - If repair fails: log error, delete corrupted file, create fresh database
  - Gateway starts with empty state
- If database is missing metrics but SQLite is valid: start with whatever is there
- Integration tests for each scenario
- Error handling: permission denied, disk full, etc.

**Dev Notes:**
- Use `match` on rusqlite errors to distinguish corruption from missing file
- Log which scenario occurred at info level
- No user action required for graceful degradation

---

### **2-5a: Historical Data Pruning Task Setup**

**Original:** Part of Story 2-5  
**Duration:** ~1-2 days

As an **operator**,
I want old historical data automatically deleted to prevent unbounded disk growth,
So that the gateway doesn't fill up the NAS disk.

**Acceptance Criteria:**
- Add `[storage]` config section:
  - `database_path` (default: `./data/opcgw.db`)
  - `retention_days` (default: 7)
  - `prune_interval_minutes` (default: 60)
- Poller spawns pruning task: every `prune_interval_minutes`, delete rows older than `retention_days`
- Pruning query: DELETE FROM metric_history WHERE timestamp < (now - retention_days)
- Log at debug: "Pruned X rows from metric_history"
- No blocker if pruning fails: log error and continue

**Dev Notes:**
- Pruning runs as a separate tokio task within poller
- Uses DELETE, not TRUNCATE (preserves data integrity)
- Timestamp comparison: SQLite datetime functions

---

### **2-5b: Pruning Integration & Testing**

**Original:** Part of Story 2-5  
**Duration:** ~1-2 days

As a **QA engineer**,
I want to verify pruning works correctly over time,
So that historical data doesn't grow unbounded.

**Acceptance Criteria:**
- Integration test: insert 1000 metric_history rows spanning 14 days
- Set retention_days = 7
- Run pruning task
- Verify: rows older than 7 days deleted, newer rows retained
- Verify: memory usage bounded over weeks of operation (NFR5)
- Verify: pruning doesn't interfere with concurrent writes
- Stress test: 100,000 rows, prune, verify performance

**Dev Notes:**
- Use synthetic timestamps to simulate day-old data
- Concurrent write test: insert while pruning runs
- Performance target: pruning completes before next poll cycle (doesn't block polling)

---

## Summary

**Total Stories:** 12 (vs. 5 original)  
**Total Effort:** ~15-18 developer-days (vs. estimated 10-12 for original 5)  
**Benefit:** Better visibility, more frequent validation, lower risk of cascading failures

**Story Distribution:**
- Trait & Types: 3 stories (2-1a, 2-1b, 2-1c)
- Schema & Infrastructure: 2 stories (2-2a, 2-2b)
- SQLite Implementation: 2 stories (2-2c, 2-2d)
- Metric Persistence: 3 stories (2-3a, 2-3b, 2-3c)
- Startup & Restore: 2 stories (2-4a, 2-4b)
- Pruning: 2 stories (2-5a, 2-5b)

---

## Next Steps

1. ✅ Story refinement complete
2. Update `epics.md` with refined stories
3. Create story files in `_bmad-output/implementation-artifacts/`
4. Update `sprint-status.yaml` with new story list
5. Schedule Epic 2 planning session with full team
6. Kick off Story 2-1a development


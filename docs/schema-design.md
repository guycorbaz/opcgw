# SQLite Schema Design for opcgw

**Status:** Design Complete (Story 2-2a)  
**Date:** 2026-04-19  
**Version:** 1.0

## Overview

This document describes the SQLite schema for opcgw (OPC UA Gateway for ChirpStack). The schema is optimized for:
- **Concurrent access**: WAL mode enables readers during writes
- **Metric persistence**: Current values (UPSERT) + historical audit trail (append-only)
- **Command queueing**: FIFO ordering for LoRaWAN downlinks
- **Flexible state**: Key-value store for gateway-wide status
- **Data retention**: Configurable pruning policies

## Table of Contents

1. [Design Principles](#design-principles)
2. [Table Specifications](#table-specifications)
3. [Indexes and Query Patterns](#indexes-and-query-patterns)
4. [Type Serialization](#type-serialization)
5. [Concurrency Model](#concurrency-model)
6. [Query Examples](#query-examples)
7. [Migration Strategy](#migration-strategy)
8. [Performance and Capacity](#performance-and-capacity)

## Design Principles

### 1. Single Writer, Multiple Readers (WAL Mode)

- **Poller** (ChirpstackPoller): Single writer task, owns exclusive write connection via Mutex
- **Readers**: OPC UA server uses separate read-only connection (WAL allows concurrent reads)
- **Concurrency**: No Rust-level mutex needed for database—WAL handles it at storage layer

### 2. UPSERT Pattern for Metrics

- Current metric values stored in `metric_values` table with UNIQUE constraint on (device_id, metric_name)
- Pattern: `INSERT OR REPLACE` updates stale values without explicit UPDATE
- Efficiency: <1ms lookups via index on unique key

### 3. Append-Only Historical Audit Trail

- All metric updates also logged to `metric_history` (immutable append-only table)
- Supports time-range queries ("metrics for device X over last 7 days")
- Pruning task deletes old rows based on `retention_config` policies

### 4. JSON Serialization for Type Flexibility

- Metrics stored as JSON in TEXT columns
- Enables schema-free storage of various metric types (Float, Int, Bool, String)
- Parseable by all languages; no binary format coupling

### 5. Nullable Key-Value Store

- `gateway_status` table trades strict schema for operational flexibility
- New state keys can be added without schema migrations
- Example keys: "server_available", "last_poll_time", "error_count"

## Table Specifications

### Table 1: metric_values

**Purpose**: Store current metric state for each (device_id, metric_name) pair.

**Pattern**: UPSERT (INSERT OR REPLACE)—replaces stale value with fresh metric.

| Column | Type | Constraints | Purpose |
|--------|------|-------------|---------|
| id | INTEGER | PRIMARY KEY | Auto-increment row ID |
| device_id | TEXT | NOT NULL | Device identifier (e.g., "0c3f0001") |
| metric_name | TEXT | NOT NULL | Metric identifier (e.g., "temperature") |
| value | TEXT | NOT NULL | JSON-serialized MetricValueInternal (see [Type Serialization](#type-serialization)) |
| data_type | TEXT | NOT NULL | Metric type: Float, Int, Bool, String (from MetricType enum tag) |
| timestamp | TEXT | NOT NULL | ISO8601 UTC when metric was sampled (e.g., "2026-04-19T12:34:56.789Z") |
| updated_at | TEXT | NOT NULL | ISO8601 UTC when inserted/replaced (for change detection) |

**Constraints**:
- UNIQUE(device_id, metric_name) — ensures one row per device-metric pair
- PRIMARY KEY(id) — implicit index for AUTOINCREMENT

**Index**:
- `idx_metric_values_device_metric(device_id, metric_name)` — covers UPSERT lookups

**Query Performance**:
- Get current metric: `SELECT value FROM metric_values WHERE device_id=? AND metric_name=?`
  Expected: <1ms via index

**Concurrency**: Safe under WAL; reads during poller writes don't block.

---

### Table 2: metric_history

**Purpose**: Immutable audit trail of all metric values over time for historical analysis.

**Pattern**: Append-only; INSERT only (no UPDATE). DELETE only for pruning.

| Column | Type | Constraints | Purpose |
|--------|------|-------------|---------|
| id | INTEGER | PRIMARY KEY | Auto-increment row ID |
| device_id | TEXT | NOT NULL | Device identifier |
| metric_name | TEXT | NOT NULL | Metric identifier |
| value | TEXT | NOT NULL | JSON-serialized MetricValueInternal |
| data_type | TEXT | NOT NULL | Metric type: Float, Int, Bool, String |
| timestamp | TEXT | NOT NULL | ISO8601 UTC when metric was sampled |
| created_at | TEXT | NOT NULL | ISO8601 UTC when inserted (append timestamp) |

**Constraints**:
- PRIMARY KEY(id) — implicit AUTOINCREMENT
- **No unique constraints** — allows duplicate values for historical tracking

**Index**:
- `idx_metric_history_device_timestamp(device_id, timestamp)` — covers range queries

**Query Performance**:
- Range query: `SELECT * FROM metric_history WHERE device_id=? AND timestamp BETWEEN ? AND ? ORDER BY timestamp DESC LIMIT 1000`
  Expected: <100ms for 1000-row range via composite index

**Concurrency**: Append-safe under WAL; pruning deletes old rows via background task.

---

### Table 3: command_queue

**Purpose**: FIFO queue for LoRaWAN downlink commands to be sent to ChirpStack.

**Pattern**: FIFO via auto-increment ID; status filtering for "get pending".

| Column | Type | Constraints | Purpose |
|--------|------|-------------|---------|
| id | INTEGER | PRIMARY KEY AUTOINCREMENT | Command ID (FIFO ordering) |
| device_id | TEXT | NOT NULL | Target device identifier |
| payload | BLOB | NOT NULL | LoRaWAN frame data (max 250 bytes per spec) |
| f_port | INTEGER | NOT NULL, CHECK(1–223) | LoRaWAN application port |
| status | TEXT | NOT NULL | State: Pending, Sent, Failed (from CommandStatus enum) |
| created_at | TEXT | NOT NULL | ISO8601 UTC when command was enqueued |
| updated_at | TEXT | NOT NULL | ISO8601 UTC last status change |
| error_message | TEXT | (nullable) | Failure reason if status='Failed' |

**Constraints**:
- PRIMARY KEY(id) AUTOINCREMENT — ensures unique, monotonic IDs for FIFO ordering
- CHECK(f_port >= 1 AND f_port <= 223) — LoRaWAN spec compliance

**Index**:
- `idx_command_queue_status_created(status, created_at)` — covers FIFO fetch pattern

**Query Performance**:
- FIFO fetch: `SELECT * FROM command_queue WHERE status='Pending' ORDER BY id ASC`
  Expected: <10ms for 1000-pending-command queue via composite index
- Update status: `UPDATE command_queue SET status=?, updated_at=datetime('now') WHERE id=?`
  Expected: <1ms via PRIMARY KEY

**State Machine**:
- Pending → Sent (after successful transmission to ChirpStack)
- Pending → Failed (after error or retry exhaustion)
- (Sent never transitions to Failed; audit trail preserved)

**Concurrency**: Single writer (poller) via Mutex; readers (OPC UA) use WAL.

---

### Table 4: gateway_status

**Purpose**: Key-value store for gateway-wide state and operational metrics.

**Pattern**: Key-value; atomic replace on update.

| Column | Type | Constraints | Purpose |
|--------|------|-------------|---------|
| key | TEXT | PRIMARY KEY | Status key (e.g., "server_available", "last_poll_time") |
| value | TEXT | NOT NULL | Value as string (parsed by caller) |
| updated_at | TEXT | NOT NULL | ISO8601 UTC when key was last updated |

**Constraints**:
- PRIMARY KEY(key) — one row per key, atomic replace on UPDATE

**Query Performance**:
- Get status: `SELECT value FROM gateway_status WHERE key=?`
  Expected: <1ms via PRIMARY KEY
- Update status: `UPDATE gateway_status SET value=?, updated_at=datetime('now') WHERE key=?`
  Expected: <1ms via PRIMARY KEY

**Predefined Keys**:
| Key | Type | Example | Purpose |
|-----|------|---------|---------|
| "server_available" | bool | "true" or "false" | ChirpStack server online status |
| "last_poll_time" | ISO8601 or null | "2026-04-19T12:34:56.789Z" | Timestamp of last successful poll |
| "error_count" | int | "5" | Cumulative error count since startup |

**Rationale**: Key-value pattern trades query simplicity for schema flexibility. New keys can be added without schema migrations.

**Concurrency**: Single writer (poller) via Mutex; readers use WAL.

---

### Table 5: retention_config

**Purpose**: Configurable retention periods and pruning policies for metric tables.

**Pattern**: One row per data type; pruning task queries this to determine what to delete.

| Column | Type | Constraints | Purpose |
|--------|------|-------------|---------|
| id | INTEGER | PRIMARY KEY | Row ID |
| data_type | TEXT | NOT NULL UNIQUE | Table type: "metric_values" or "metric_history" |
| retention_days | INTEGER | NOT NULL | How many days to retain (default: 30 for values, 90 for history) |
| auto_delete | BOOLEAN | NOT NULL DEFAULT 1 | Enable/disable automatic pruning (1=on, 0=off) |
| updated_at | TEXT | NOT NULL | ISO8601 UTC when config was last updated |

**Constraints**:
- PRIMARY KEY(id)
- UNIQUE(data_type) — one row per data type

**Query Performance**:
- Get retention days: `SELECT retention_days FROM retention_config WHERE data_type=?`
  Expected: <1ms via UNIQUE index

**Initial Data**:
```sql
INSERT INTO retention_config (data_type, retention_days, auto_delete, updated_at)
VALUES
  ('metric_values', 30, 1, datetime('now')),
  ('metric_history', 90, 1, datetime('now'));
```

**Concurrency**: Pruning task (background) reads and updates this table.

---

## Indexes and Query Patterns

### Summary of All Indexes

| Index Name | Table | Columns | Reason |
|------------|-------|---------|--------|
| PRIMARY KEY | metric_values | (id) | Implicit; AUTOINCREMENT |
| UNIQUE | metric_values | (device_id, metric_name) | UPSERT lookups; prevents duplicates |
| idx_metric_values_device_metric | metric_values | (device_id, metric_name) | Covers SELECT queries |
| PRIMARY KEY | metric_history | (id) | Implicit; AUTOINCREMENT |
| idx_metric_history_device_timestamp | metric_history | (device_id, timestamp) | Time-range queries; efficient sorting |
| PRIMARY KEY | command_queue | (id) | Implicit; AUTOINCREMENT for FIFO |
| idx_command_queue_status_created | command_queue | (status, created_at) | FIFO fetch filtered by status |
| PRIMARY KEY | gateway_status | (key) | Atomic key lookups |
| PRIMARY KEY | retention_config | (id) | Implicit |
| UNIQUE | retention_config | (data_type) | Prevent duplicate config rows |

### Composite Index Design

**Composite Index: (device_id, timestamp)**

Used by: `metric_history` table for range queries.

Query: `SELECT * FROM metric_history WHERE device_id=? AND timestamp BETWEEN ? AND ?`

- **Why composite?** Both device_id and timestamp are in WHERE clause; index must cover both columns to avoid table scan.
- **Order matters**: device_id first (filters to single device), then timestamp (ranges over times within that device).
- **B-tree property**: Rows are sorted by device_id, then by timestamp within each device_id group.

**Composite Index: (status, created_at)**

Used by: `command_queue` table for FIFO fetch with status filtering.

Query: `SELECT * FROM command_queue WHERE status='Pending' ORDER BY id ASC`

- **Why composite?** WHERE filters by status, ORDER BY ID (which correlates with created_at for FIFO).
- **Order matters**: status first (filters to subset), then created_at (enables efficient FIFO traversal).

---

## Type Serialization

### JSON Serialization Format

Metrics are stored as JSON-serialized `MetricValueInternal` structs in the `value` column of both `metric_values` and `metric_history` tables.

**Rust Definition** (from Story 2-2):
```rust
pub struct MetricValueInternal {
    pub device_id: String,
    pub metric_name: String,
    pub value: String,               // Stores "23.5" or "true" or "42" as STRING
    pub timestamp: DateTime<Utc>,
    pub data_type: MetricType,       // Float, Int, Bool, or String (tag enum)
}

pub enum MetricType {
    Float,
    Int,
    Bool,
    String,
}
```

**JSON Example** (stored in SQLite TEXT column):
```json
{
  "device_id": "0c3f0001",
  "metric_name": "temperature",
  "value": "23.5",
  "timestamp": "2026-04-19T12:34:56.789Z",
  "data_type": "Float"
}
```

**Storage Strategy**:
1. Rust code creates `MetricValueInternal` struct with all metadata
2. Serialize to JSON using `serde_json::to_string()`
3. Store JSON string in SQLite TEXT column (`value` field)
4. Retrieve: parse JSON string back to struct using `serde_json::from_str()`
5. data_type column stores just the enum tag ("Float", "Int", "Bool", "String") for indexing

**Why JSON?**
- Schema-free: any metric type stored in same column
- Debuggable: human-readable in SQLite browser
- Portable: parseable by any language (SQL queries, external tools)
- Flexible: can extend with new fields without schema migration

**Timestamp Format**:
- **Format**: ISO8601 in UTC (RFC3339)
- **Example**: "2026-04-19T12:34:56.789Z"
- **Storage**: TEXT (lexicographically comparable—string sort = time sort)
- **Parsing**: `chrono::DateTime<Utc>::parse_from_rfc3339()` in Rust

---

## Concurrency Model

### Write-Ahead Logging (WAL) Mode

SQLite in WAL mode allows concurrent readers while a single writer commits to the log.

**Configuration**:
```sql
PRAGMA journal_mode = WAL;         -- Enable Write-Ahead Logging
PRAGMA foreign_keys = ON;         -- Referential integrity (future-proofing)
PRAGMA synchronous = NORMAL;      -- Crash-safe; faster than FULL
```

**Concurrency Behavior**:

```
Poller Task (Writer)                 OPC UA Server (Reader)
└─ Arc<Mutex<>> sqlite3 connection   └─ Separate read-only connection
   └─ Exclusive write to WAL            └─ Reads via WAL shared page cache
```

- **Poller**: Single task owns the write connection via `Arc<Mutex<>>` (non-shareable)
  - Prevents multiple threads writing simultaneously
  - All metrics, commands, status updates go through this connection
  - PRAGMA synchronous=NORMAL is safe (crash-safe via WAL)

- **OPC UA Server**: Separate read-only connection
  - Reads via WAL shared page cache (no blocking on poller writes)
  - Can be called concurrently by multiple OPC UA client connections
  - Never writes (read-only prevents accidental mutations)

**Why This Works**:
- WAL keeps two files: main database file + `-wal` (write-ahead log) + `-shm` (shared memory)
- Readers see committed state from WAL log without blocking writers
- Writer appends to WAL, flushes to main DB asynchronously
- No Rust-level mutex needed for database—SQLite handles it

**Crash Safety**:
- PRAGMA synchronous=NORMAL: OS filesystem buffer sync on each transaction (good enough for WAL)
- PRAGMA synchronous=FULL would fsync after each write (slower, rarely needed with WAL)

---

## Query Examples

### 1. Get Current Metric Value

```sql
SELECT value FROM metric_values
WHERE device_id = ? AND metric_name = ?
LIMIT 1;
```

**Expected**: <1ms via index on (device_id, metric_name)  
**Rust Code**:
```rust
let value = db.query_row(
    "SELECT value FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
    [device_id, metric_name],
    |row| row.get::<_, String>(0),
)?;
let metric: MetricValueInternal = serde_json::from_str(&value)?;
```

---

### 2. UPSERT Current Metric (Insert or Replace)

```sql
INSERT OR REPLACE INTO metric_values
  (device_id, metric_name, value, data_type, timestamp, updated_at)
VALUES (?, ?, ?, ?, ?, datetime('now'));
```

**Behavior**:
- If (device_id, metric_name) exists: replace entire row
- If not exists: insert new row
- Replaces stale value with fresh metric in one operation

**Rust Code**:
```rust
let value_json = serde_json::to_string(&metric_value)?;
db.execute(
    "INSERT OR REPLACE INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))",
    [&metric_value.device_id, &metric_value.metric_name, &value_json, &format!("{:?}", metric_value.data_type), &metric_value.timestamp.to_rfc3339()],
)?;
```

---

### 3. Query Historical Metrics (Time Range)

```sql
SELECT * FROM metric_history
WHERE device_id = ? AND timestamp BETWEEN ? AND ?
ORDER BY timestamp DESC
LIMIT 1000;
```

**Expected**: <100ms for 1000-row range via composite index on (device_id, timestamp)  
**Use Case**: OPC UA client requests "give me all temperature readings from this device over the last 7 days"

**Rust Code**:
```rust
let start_time = Utc::now() - Duration::days(7);
let end_time = Utc::now();

let mut stmt = db.prepare(
    "SELECT * FROM metric_history WHERE device_id = ?1 AND timestamp BETWEEN ?2 AND ?3 ORDER BY timestamp DESC LIMIT 1000"
)?;

let metrics = stmt.query_map(
    [&device_id, &start_time.to_rfc3339(), &end_time.to_rfc3339()],
    |row| {
        let value_json: String = row.get(3)?;
        let metric: MetricValueInternal = serde_json::from_str(&value_json)?;
        Ok(metric)
    },
)?;
```

---

### 4. Get Pending Commands (FIFO)

```sql
SELECT * FROM command_queue
WHERE status = 'Pending'
ORDER BY id ASC;
```

**Expected**: <10ms for 1000-pending-command queue via index on (status, created_at)  
**Use Case**: Poller fetches next command(s) to send to ChirpStack

**Rust Code**:
```rust
let mut stmt = db.prepare(
    "SELECT id, device_id, payload, f_port, status, created_at, updated_at, error_message FROM command_queue WHERE status = 'Pending' ORDER BY id ASC"
)?;

let commands = stmt.query_map([], |row| {
    Ok(DeviceCommandInternal {
        id: row.get(0)?,
        device_id: row.get(1)?,
        payload: row.get(2)?,
        f_port: row.get(3)?,
        status: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        error_message: row.get(7)?,
    })
})?;
```

---

### 5. Update Command Status

```sql
UPDATE command_queue
SET status = ?, updated_at = datetime('now')
WHERE id = ?;
```

**Expected**: <1ms via PRIMARY KEY on id  
**Use Case**: After sending command to ChirpStack, update status to 'Sent' or 'Failed'

**Rust Code**:
```rust
db.execute(
    "UPDATE command_queue SET status = ?1, updated_at = datetime('now') WHERE id = ?2",
    rusqlite::params![new_status, command_id],
)?;
```

---

### 6. Get/Update Gateway Status

```sql
-- Get
SELECT value FROM gateway_status WHERE key = ?;

-- Update (atomic replace)
UPDATE gateway_status SET value = ?, updated_at = datetime('now') WHERE key = ?;
INSERT OR IGNORE INTO gateway_status (key, value, updated_at) VALUES (?, ?, datetime('now'));
```

**Expected**: <1ms via PRIMARY KEY on key

**Rust Code**:
```rust
// Get server availability
let available: String = db.query_row(
    "SELECT value FROM gateway_status WHERE key = 'server_available'",
    [],
    |row| row.get(0),
)?;
let is_available: bool = available == "true";

// Update last poll time
db.execute(
    "UPDATE gateway_status SET value = ?1, updated_at = datetime('now') WHERE key = 'last_poll_time'",
    rusqlite::params![Utc::now().to_rfc3339()],
)?;
```

---

### 7. Prune Old Metrics (Background Pruning Task)

```sql
-- Get retention policy
SELECT retention_days, auto_delete FROM retention_config WHERE data_type = 'metric_history';

-- Delete old rows (keep only latest per metric)
DELETE FROM metric_history
WHERE device_id = ? AND timestamp < ?
  AND timestamp NOT IN (
    SELECT MAX(timestamp) FROM metric_history
    WHERE device_id = ?
    GROUP BY metric_name
  );
```

**Rationale**: Deletes old rows while keeping the most recent value per metric (for trending/analysis).

---

## Migration Strategy

### Version Tracking

Schema versions are tracked via `PRAGMA user_version`:

```sql
PRAGMA user_version = 1;  -- Current schema version
```

On application startup:
1. Read current `PRAGMA user_version` from database
2. Compare to application's expected version
3. Apply pending migrations if version < expected

### Migration Naming Convention

Migrations are stored in `schema/` directory with version prefix:

```
schema/v001_initial.sql          -- First schema (v1)
schema/v002_add_indexes.sql      -- Second change (v2)
schema/v003_add_retry_count.sql  -- Third change (v3)
```

### Idempotence Requirement

**All migrations must be idempotent** (safe to re-run):

```sql
-- ✓ GOOD: IF NOT EXISTS prevents errors on re-run
CREATE TABLE IF NOT EXISTS metric_values (...);
CREATE INDEX IF NOT EXISTS idx_foo ON bar(...);

-- ✗ BAD: Will fail if run twice
CREATE TABLE metric_values (...);
DROP TABLE metric_values;
```

### Adding Columns (Forward Compatibility)

To add a new column to an existing table:

```sql
-- Migration: v002_add_retry_count.sql
ALTER TABLE command_queue ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;

-- Update version
PRAGMA user_version = 2;
```

**Benefits**:
- Existing data gets default value (0 for retry_count)
- No data loss; old code can still read table
- New code can immediately use retry_count field

### Adding Indexes

To add an index without downtime:

```sql
-- Migration: v003_add_device_index.sql
CREATE INDEX IF NOT EXISTS idx_metric_history_device ON metric_history(device_id);

-- Update version
PRAGMA user_version = 3;
```

---

## Performance and Capacity

### Query Performance Targets

| Query | Expected Latency | Index Used | Notes |
|-------|------------------|------------|-------|
| Get current metric | <1ms | UNIQUE(device_id, metric_name) | Covers most common operation |
| UPSERT metric | <1ms | UNIQUE(device_id, metric_name) | Replace-only; no DELETE |
| Time-range query (1000 rows) | <100ms | (device_id, timestamp) | Composite index; range scan |
| Get pending commands | <10ms | (status, created_at) | FIFO fetch filtered by status |
| Update command status | <1ms | PRIMARY KEY(id) | Single-row point update |
| Get/update gateway status | <1ms | PRIMARY KEY(key) | Simple key lookup |
| Prune old metrics | ~10-100ms | (device_id, timestamp) | Background task; bulk DELETE |

### Capacity Estimates

**Typical deployment** (1000 devices × 10 metrics each):

| Metric | Value | Notes |
|--------|-------|-------|
| Unique (device, metric) pairs | ~10,000 | metric_values table rows |
| Metric samples/day (polling every 60s) | ~1,440,000 | 10K pairs × 144 samples/day |
| metric_history rows/year (30-day retention) | ~40M | 1.44M/day × 30 days |
| Approximate disk space | ~1-2 GB | JSON values are compressible |
| WAL overhead | ~10% of DB size | Write-ahead log + shared memory |

**Scaling notes**:
- 10,000 rows in metric_values table: <1ms lookups remain fast
- 40M rows in metric_history: composite index keeps range queries <100ms
- WAL size grows with write rate; checkpoint task flushes WAL periodically
- Retention pruning keeps disk usage bounded (30-90 day retention policy)

### Tuning Recommendations

For high-volume deployments (>10K devices):
1. **Increase cache size**: `PRAGMA cache_size = 50000;` (default: 2000 pages)
2. **Adjust checkpoint interval**: `PRAGMA wal_autocheckpoint = 100000;` (default: 1000 pages)
3. **Monitor WAL size**: If `-wal` file grows >100MB, force checkpoint during low-traffic period
4. **Consider sharding**: Split into separate SQLite files per region/application

---

## Files

- `schema/v001_initial.sql` — Complete SQL DDL for initial schema
- `docs/schema-design.md` — This document

---

## Next Steps

See Story 2-2b (Schema Creation and Migration) for implementation of:
- Rust code to load and execute migrations on startup
- Integration with main.rs and configuration
- Tests for migration idempotence and schema correctness

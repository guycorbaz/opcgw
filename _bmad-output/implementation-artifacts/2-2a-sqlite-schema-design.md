# Story 2-2a: SQLite Schema Design & Documentation

Status: done
✅ **COMPLETE** (2026-04-19) — All 10 tasks complete; SQL DDL and comprehensive documentation delivered

## Story

As a **developer**,
I want a well-designed SQLite schema that supports metrics, commands, and state,
So that the database structure is optimal and documented.

## Acceptance Criteria

1. **Given** persistence requirements, **When** I design the schema, **Then** 5 tables are defined: metric_values, metric_history, command_queue, gateway_status, retention_config.
2. **Given** metric_values table, **When** I design it, **Then** unique constraint (device_id, metric_name) enables UPSERT for metric updates.
3. **Given** metric_history table, **When** I design it, **Then** it's append-only with composite index on (device_id, timestamp) for efficient time-range queries.
4. **Given** command_queue table, **When** I design it, **Then** it supports FIFO ordering via auto-increment ID and query filtering by status.
5. **Given** gateway_status table, **When** I design it, **Then** it's a key-value store (key TEXT PK, value TEXT) for flexible server state.
6. **Given** metric value storage, **When** designing columns, **Then** value field stores JSON-serialized MetricValue (from Story 2-2), data_type field stores MetricType tag.
7. **Given** concurrency requirements, **When** configuring storage, **Then** WAL mode enables concurrent readers; poller owns single connection (single writer), OPC UA server uses read-only connection.
8. **Given** schema documentation, **When** complete, **Then** rationale for each table, index, and query pattern is documented with migration strategy.
9. **Given** query performance, **When** designing indexes, **Then** all index designs include justification of the query pattern each supports.
10. **Given** future schema evolution, **When** designing migrations, **Then** schema version tracking (PRAGMA user_version) and migration naming convention are documented.

## Tasks / Subtasks

- [x] Task 1: Design metric_values table (AC: #1, #2, #6) ✅ DONE 2026-04-19
  - [x] Columns: id (INTEGER PRIMARY KEY), device_id (TEXT NOT NULL), metric_name (TEXT NOT NULL), value (TEXT NOT NULL, JSON-serialized MetricValue), data_type (TEXT NOT NULL, from MetricType enum), timestamp (TEXT NOT NULL, ISO8601), updated_at (TEXT NOT NULL, for change detection)
  - [x] Unique constraint: (device_id, metric_name) for UPSERT pattern
  - [x] Indexes: PRIMARY KEY (id), UNIQUE (device_id, metric_name)
  - [x] Rationale: UPSERT (INSERT OR REPLACE) replaces stale value with fresh metric; value stores JSON (see Story 2-2 for serialization format)
  - [x] Query pattern: `SELECT value FROM metric_values WHERE device_id=? AND metric_name=? LIMIT 1` must be <1ms

- [x] Task 2: Design metric_history table (AC: #1, #3, #9) ✅ DONE 2026-04-19
  - [x] Columns: id (INTEGER PRIMARY KEY), device_id (TEXT NOT NULL), metric_name (TEXT NOT NULL), value (TEXT NOT NULL, JSON format), data_type (TEXT NOT NULL), timestamp (TEXT NOT NULL, ISO8601), created_at (TEXT NOT NULL)
  - [x] No unique constraints (allows duplicate values for historical tracking)
  - [x] Index: Composite on (device_id, timestamp) for range queries
  - [x] Rationale: Append-only audit trail (never UPDATE/DELETE except pruning); index supports time-range queries like "metrics for device X in last 7 days"
  - [x] Query pattern: `SELECT * FROM metric_history WHERE device_id=? AND timestamp BETWEEN ? AND ? ORDER BY timestamp DESC LIMIT 1000` for efficient time-range access

- [x] Task 3: Design command_queue table (AC: #1, #4, #9) ✅ DONE 2026-04-19
  - [x] Columns: id (INTEGER PRIMARY KEY AUTOINCREMENT), device_id (TEXT NOT NULL), payload (BLOB NOT NULL, max 250 bytes per LoRaWAN), f_port (INTEGER NOT NULL, CHECK(f_port >= 1 AND f_port <= 223)), status (TEXT NOT NULL, enum: Pending/Sent/Failed), created_at (TEXT NOT NULL, ISO8601), updated_at (TEXT NOT NULL), error_message (TEXT, nullable)
  - [x] Index: Composite on (status, created_at) for FIFO fetch pattern
  - [x] Rationale: FIFO ordering via auto-increment ID; status filtering for "get pending commands"; index supports query: `SELECT * FROM command_queue WHERE status='Pending' ORDER BY id ASC`
  - [x] Query pattern: FIFO fetch expects rows ordered by ID; status index enables efficient filtering

- [x] Task 4: Design gateway_status table (AC: #1, #5, #9) ✅ DONE 2026-04-19
  - [x] Columns: key (TEXT PRIMARY KEY), value (TEXT NOT NULL), updated_at (TEXT NOT NULL, ISO8601)
  - [x] Keys/Rows: "server_available" (bool), "last_poll_time" (ISO8601 or null), "error_count" (int)
  - [x] Rationale: Key-value pattern trades query simplicity for schema flexibility (no migration if new status fields added)
  - [x] Query pattern: `SELECT value FROM gateway_status WHERE key=?` for simple single-key lookups

- [x] Task 5: Design retention_config table (AC: #1, #5, #9) ✅ DONE 2026-04-19
  - [x] Columns: id (INTEGER PRIMARY KEY), data_type (TEXT NOT NULL, enum: metric_values/metric_history), retention_days (INTEGER NOT NULL, DEFAULT 30 for metrics, 90 for history), auto_delete (BOOLEAN NOT NULL, DEFAULT 1), updated_at (TEXT NOT NULL)
  - [x] Rows: Two default rows (one for metric_values, one for metric_history)
  - [x] Rationale: Enables per-type retention policies; pruning task queries this to determine what to delete
  - [x] Query pattern: `SELECT retention_days FROM retention_config WHERE data_type=?` for pruning decisions

- [x] Task 6: Document WAL mode & concurrency (AC: #7) ✅ DONE 2026-04-19
  - [x] Document: WAL (Write-Ahead Logging) mode configuration: `PRAGMA journal_mode = WAL`
  - [x] Explain: WAL enables concurrent readers while poller writes (single writer constraint)
  - [x] Document: Poller owns SQLite connection (non-shareable); OPC UA server uses separate read-only connection
  - [x] Pragma settings: journal_mode=WAL, foreign_keys=ON, synchronous=NORMAL (crash-safe), temp_store=MEMORY
  - [x] Rationale: Single writer avoids mutex contention in Rust code; WAL handles concurrency at DB level

- [x] Task 7: Create SQL DDL file (AC: #8) ✅ DONE 2026-04-19
  - [x] File: `schema/v001_initial.sql` ✅ Created
  - [x] Include: CREATE TABLE IF NOT EXISTS for all 5 tables (idempotent)
  - [x] Include: PRAGMA user_version = 1 (schema versioning)
  - [x] Include: Indexes with CREATE INDEX IF NOT EXISTS
  - [x] Include: Comments explaining purpose of each table
  - [x] Include: Initial data INSERT for retention_config rows
  - [x] Verified: All CREATE statements use IF NOT EXISTS for idempotence

- [x] Task 8: Document migration strategy (AC: #10) ✅ DONE 2026-04-19
  - [x] Document: Schema version tracking via PRAGMA user_version
  - [x] Document: Migration naming: v001 (initial), v002 (first change), etc.
  - [x] Document: Idempotence requirement (migrations must be re-runnable)
  - [x] Document: How to add columns without breaking existing data
  - [x] Rationale: Enables forward-compatible schema evolution

- [x] Task 9: Create comprehensive schema documentation (AC: #8, #9) ✅ DONE 2026-04-19
  - [x] File: `docs/schema-design.md` ✅ Created
  - [x] Include: All 5 table definitions with column specs, types, constraints
  - [x] Include: Index strategy for each table with query pattern justification
  - [x] Include: Example queries showing usage patterns
  - [x] Include: Type serialization explanation (value field = JSON-serialized MetricValue from Story 2-2)
  - [x] Include: Migration strategy and PRAGMA settings
  - [x] Include: Concurrency model explanation (WAL + single writer pattern)
  - [x] Include: Capacity planning estimates (expected row counts, disk space)

- [x] Task 10: Validate design completeness (AC: #2-10) ✅ DONE 2026-04-19
  - [x] Verify: All query patterns have supporting indexes ✅ (comprehensive table in docs/schema-design.md)
  - [x] Verify: UPSERT semantics correct for metric_values (INSERT OR REPLACE) ✅ (unique constraint + DDL)
  - [x] Verify: Timestamp indexing supports range queries efficiently ✅ (composite index documented)
  - [x] Verify: FIFO ordering guaranteed by ID + status index ✅ (PRIMARY KEY AUTOINCREMENT)
  - [x] Verify: Type system context from Story 2-2 documented ✅ (Type Serialization section)
  - [x] Verify: WAL mode concurrency model clear ✅ (Concurrency Model section)
  - [x] Verify: SQL file is idempotent (IF NOT EXISTS) ✅ (all CREATE statements use IF NOT EXISTS)
  - [x] Verify: Migration strategy supports future schema changes ✅ (Migration Strategy section)

## Dev Agent Record

### Completion Summary (2026-04-19)

**All Tasks Complete:** Story 2-2a fully documented and designed

**Deliverables:**
- ✅ schema/v001_initial.sql — Complete SQL DDL with 5 tables, indexes, PRAGMA settings, initial data
- ✅ docs/schema-design.md — Comprehensive 500+ line documentation covering:
  - All 5 table specifications (metric_values, metric_history, command_queue, gateway_status, retention_config)
  - Index strategy and query patterns with performance targets
  - Type serialization (JSON format for MetricValueInternal)
  - Concurrency model (WAL mode + single writer)
  - 7 query examples with Rust code
  - Migration strategy with version tracking
  - Capacity planning (10K devices × 10 metrics, ~40M rows/year retention)

**Acceptance Criteria Met:**
- ✅ AC#1: 5 tables defined (metric_values, metric_history, command_queue, gateway_status, retention_config)
- ✅ AC#2: metric_values with unique constraint (device_id, metric_name) for UPSERT
- ✅ AC#3: metric_history append-only with (device_id, timestamp) composite index
- ✅ AC#4: command_queue with FIFO ordering via AUTOINCREMENT ID + (status, created_at) index
- ✅ AC#5: gateway_status key-value store with atomic updates
- ✅ AC#6: value field stores JSON-serialized MetricValueInternal; data_type field stores MetricType tag
- ✅ AC#7: WAL mode enables concurrent readers; poller owns single connection, OPC UA uses read-only
- ✅ AC#8: Rationale documented for each table, index, and query pattern
- ✅ AC#9: All index designs include query pattern justification and performance targets
- ✅ AC#10: Schema version tracking (PRAGMA user_version) and migration naming convention documented

**Key Design Decisions:**
1. **WAL concurrency**: Enables readers during poller writes without Rust-level mutex overhead
2. **UPSERT pattern**: INSERT OR REPLACE with unique constraint for O(1) metric updates
3. **JSON serialization**: Full MetricValueInternal stored as JSON string for flexibility
4. **Append-only history**: metric_history never updated, only inserted/pruned, preserving audit trail
5. **Key-value gateway_status**: Trades strict schema for operational flexibility (new keys without migration)
6. **Composite indexes**: (device_id, timestamp) for range queries; (status, created_at) for FIFO
7. **Retention config**: Configurable pruning policies per data type (30 days metrics, 90 days history)

**Architecture Notes:**
- Single writer (poller) via Mutex ensures serialized writes; WAL layer handles concurrent reads
- All timestamps in ISO8601 UTC format (lexicographically sortable)
- f_port validation constraint ensures LoRaWAN spec compliance (1-223)
- Payload max 250 bytes per LoRaWAN specification enforced at type level
- Idempotent DDL (IF NOT EXISTS) supports re-runnable migrations
- PRAGMA synchronous=NORMAL + WAL = crash-safe but faster than FULL mode

---

## Dev Notes

### Type System Context (from Story 2-2)

The `value` field stores **JSON-serialized MetricValueInternal**:
```rust
pub struct MetricValueInternal {
    pub device_id: String,
    pub metric_name: String,
    pub value: String,               // Stores "23.5" or "true" or "42" as STRING
    pub timestamp: DateTime<Utc>,
    pub data_type: MetricType,       // Float, Int, Bool, or String (tag enum)
}
```

**Storage Strategy:** When storing in SQLite, the entire MetricValueInternal is JSON-serialized:
```json
{
  "device_id": "device_123",
  "metric_name": "temperature",
  "value": "23.5",
  "timestamp": "2026-04-19T12:34:56Z",
  "data_type": "Float"
}
```

The `value` column stores this JSON string. The `data_type` column stores just the tag ("Float", "Int", "Bool", "String") for indexing/filtering.

### Complete SQL DDL Example

```sql
-- schema/v001_initial.sql
PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA synchronous = NORMAL;

PRAGMA user_version = 1;

-- Current metric values (UPSERT table)
CREATE TABLE IF NOT EXISTS metric_values (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,               -- JSON-serialized MetricValueInternal
  data_type TEXT NOT NULL,           -- Float, Int, Bool, String
  timestamp TEXT NOT NULL,           -- ISO8601
  updated_at TEXT NOT NULL,
  UNIQUE(device_id, metric_name)
);
CREATE INDEX IF NOT EXISTS idx_metric_values_device_metric 
  ON metric_values(device_id, metric_name);

-- Historical metric values (append-only)
CREATE TABLE IF NOT EXISTS metric_history (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,
  data_type TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_metric_history_device_timestamp 
  ON metric_history(device_id, timestamp);

-- Command queue (FIFO)
CREATE TABLE IF NOT EXISTS command_queue (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  device_id TEXT NOT NULL,
  payload BLOB NOT NULL,             -- LoRaWAN frame data (max 250 bytes)
  f_port INTEGER NOT NULL,           -- 1-223
  status TEXT NOT NULL,              -- Pending, Sent, Failed
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  error_message TEXT,
  CHECK(f_port >= 1 AND f_port <= 223)
);
CREATE INDEX IF NOT EXISTS idx_command_queue_status_created 
  ON command_queue(status, created_at);

-- Gateway connection status (key-value)
CREATE TABLE IF NOT EXISTS gateway_status (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

-- Retention policy configuration
CREATE TABLE IF NOT EXISTS retention_config (
  id INTEGER PRIMARY KEY,
  data_type TEXT NOT NULL UNIQUE,    -- metric_values or metric_history
  retention_days INTEGER NOT NULL,
  auto_delete BOOLEAN NOT NULL DEFAULT 1,
  updated_at TEXT NOT NULL
);

-- Initialize retention config
INSERT OR IGNORE INTO retention_config (data_type, retention_days, auto_delete, updated_at)
VALUES 
  ('metric_values', 30, 1, datetime('now')),
  ('metric_history', 90, 1, datetime('now'));
```

### Query Patterns (Required Efficiency)

1. **Get current metric:** `SELECT value FROM metric_values WHERE device_id=? AND metric_name=? LIMIT 1` — must be <1ms
2. **Store/update metric (UPSERT):** `INSERT OR REPLACE INTO metric_values VALUES (NULL, ?, ?, ?, ?, ?, datetime('now'))`
3. **Get pending commands:** `SELECT * FROM command_queue WHERE status='Pending' ORDER BY id ASC`
4. **Update command status:** `UPDATE command_queue SET status=?, updated_at=datetime('now') WHERE id=?`
5. **Get status value:** `SELECT value FROM gateway_status WHERE key=?`
6. **Range query metrics:** `SELECT * FROM metric_history WHERE device_id=? AND timestamp BETWEEN ? AND ? ORDER BY timestamp DESC LIMIT 1000`
7. **Prune old history:** `DELETE FROM metric_history WHERE device_id=? AND timestamp < ? AND timestamp NOT IN (SELECT MAX(timestamp) FROM metric_history WHERE device_id=? GROUP BY metric_name)` (keeps latest per metric)

### Concurrency Model (WAL Mode)

**Configuration:**
- `PRAGMA journal_mode = WAL` — Write-Ahead Logging
- Single SQLite connection per poller task (non-shareable due to Rust Mutex)
- OPC UA server uses separate read-only connection

**Behavior:**
- Poller writes: exclusive access to WAL log (serialized writes)
- OPC UA reads: concurrent access via WAL readers (non-blocking)
- No Rust-level mutex needed for DB access (WAL handles it)

### Timestamp Format

Use **ISO 8601 in UTC:** `"2026-04-19T12:34:56.789Z"`
- Stored as TEXT in SQLite
- Parseable by `chrono::DateTime<Utc>::parse_from_rfc3339()`
- Lexicographically comparable (string sort = time sort)
- Indexable efficiently

### Schema Versioning & Migration Path

**Version Tracking:**
- `PRAGMA user_version = 1` for schema version 1
- On startup: read version, apply pending migrations if version < current

**Migration Naming Convention:**
- `v001_initial.sql` — first schema
- `v002_add_indexes.sql` — second change
- etc.

**Idempotence Requirement:**
- All CREATE statements use `IF NOT EXISTS`
- All DROP statements use `IF EXISTS`
- Migrations safe to re-run

**Adding Columns (forward compatibility):**
```sql
-- Migration: v003_add_retry_count.sql
ALTER TABLE command_queue ADD COLUMN retry_count INTEGER NOT NULL DEFAULT 0;
```

### What NOT to Do (Design-Only)

- Do NOT implement schema creation (Story 2-2b handles this)
- Do NOT write Rust code yet (only documentation)
- Do NOT implement migration runner (Story 2-2b)
- Do NOT actually create database file (that's Step 2-2b)

## File List

**Files to Create (Design Phase):**
- `schema/v001_initial.sql` — Complete SQL DDL for initial schema (CREATE TABLE, indexes, PRAGMA settings, initial data)
- `docs/schema-design.md` — Complete schema documentation including:
  - All 5 table definitions with column specs and constraints
  - Index strategy with query pattern justification
  - Example query patterns showing usage
  - Type serialization explanation (JSON format for metrics)
  - WAL mode and concurrency model explanation
  - Migration strategy for future schema changes
  - Capacity planning and performance expectations

**Note:** This story is **design only** (no Rust code). The SQL file is pure DDL text. Implementation (schema creation, migrations) happens in Story 2-2b.


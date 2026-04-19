# Story 2.2b: SQLite Schema Creation and Migration

Status: in-progress

## Story

As a **developer**,
I want to implement SQLite schema creation and run migrations in Rust code,
so that the gateway can initialize its database on startup with proper versioning and error recovery.

## Acceptance Criteria

1. **Given** the schema from Story 2-2a (5 tables, indexes, PRAGMA settings), **When** the gateway starts, **Then** the SQLite database file is created or opened, and all tables are created via embedded migration SQL.

2. **Given** an existing database file, **When** the gateway starts, **Then** no error is raised — existing schema is reused if version matches, or migrations are applied if version is outdated.

3. **Given** migration files in `migrations/` directory, **When** the gateway initializes, **Then** migrations are executed in version order (v001 before v002, etc.) using embedded SQL via `include_str!()`.

4. **Given** a successful migration, **When** the next startup occurs, **Then** the PRAGMA user_version is checked to confirm version matches and no re-migration occurs.

5. **Given** a corrupted database or missing permissions, **When** the gateway starts, **Then** a clear error message is logged (not a panic), and the gateway exits with a non-zero code.

6. **Given** the [storage] config section with database_path, **When** the gateway loads configuration, **Then** the path is validated and the directory is created if needed (mkdir -p).

7. **Given** successful schema creation, **When** the gateway starts, **Then** an info-level log confirms "Database initialized at [path], version X".

8. **Given** the StorageBackend trait from Story 2-1, **When** I create SqliteBackend, **Then** it implements StorageBackend trait with all required methods.

9. **Given** WAL mode configured in schema, **When** the gateway opens the database, **Then** PRAGMA journal_mode is set to WAL and verified via PRAGMA query.

10. **Given** concurrent access patterns (poller writes, OPC UA reads), **When** multiple connections are created to the same database, **Then** each task opens its own SQLite Connection (non-shared) and WAL mode enables concurrent readers.

## Tasks / Subtasks

- [ ] **Task 1: Create migrations/ directory structure and v001_initial.sql**
  - [ ] Create `migrations/` directory at project root
  - [ ] Copy schema from Story 2-2a into `migrations/v001_initial.sql` with SQL comments
  - [ ] Verify: All CREATE TABLE use `IF NOT EXISTS` (idempotent)
  - [ ] Verify: All indexes created with `IF NOT EXISTS`
  - [ ] Verify: PRAGMA user_version = 1 is set
  - [ ] Verify: Initial INSERT OR IGNORE for retention_config (non-destructive)
  - [ ] Verify: Comments explain purpose of each table and index

- [ ] **Task 2: Create storage/schema.rs with migration runner**
  - [ ] Create module: `src/storage/schema.rs`
  - [ ] Implement function: `run_migrations(conn: &Connection) -> Result<(), OpcGwError>`
  - [ ] Read PRAGMA user_version to determine current version
  - [ ] Execute v001_initial.sql via include_str!() if version < 1
  - [ ] Set PRAGMA user_version = 1 after migration
  - [ ] Log at info: "Applied migration v001_initial"
  - [ ] Error handling: Wrap rusqlite errors in OpcGwError::Database

- [ ] **Task 3: Create storage/sqlite.rs — SqliteBackend implementation**
  - [ ] Create module: `src/storage/sqlite.rs`
  - [ ] Struct: `pub struct SqliteBackend { conn: Connection }`
  - [ ] Constructor: `pub fn new(path: &str) -> Result<Self, OpcGwError>`
    - [ ] Create parent directory if not exists: `std::fs::create_dir_all()`
    - [ ] Open connection: `Connection::open(path)?`
    - [ ] Set WAL mode and verify via PRAGMA query
    - [ ] Set foreign_keys ON, synchronous NORMAL, temp_store MEMORY
    - [ ] Call `schema::run_migrations(&conn)?` to initialize schema
    - [ ] Log at info: "Opened SQLite database at [path], schema version X"
    - [ ] Return `Ok(SqliteBackend { conn })`

  - [ ] Implement StorageBackend trait methods:
    - [ ] `get_metric(device_id, metric_name) -> Result<Option<MetricValueInternal>, OpcGwError>`
      - [ ] SELECT from metric_values, parse JSON value field
    - [ ] `store_metric(metric) -> Result<(), OpcGwError>`
      - [ ] INSERT OR REPLACE into metric_values with JSON serialization
    - [ ] `store_metric_history(metric) -> Result<(), OpcGwError>`
      - [ ] INSERT into metric_history (append-only)
    - [ ] `get_gateway_status(key) -> Result<Option<String>, OpcGwError>`
    - [ ] `set_gateway_status(key, value) -> Result<(), OpcGwError>`
    - [ ] `queue_command(device_id, payload, f_port) -> Result<(), OpcGwError>`
    - [ ] `get_pending_commands() -> Result<Vec<StoredCommand>, OpcGwError>`
      - [ ] ORDER BY id ASC for FIFO
    - [ ] `update_command_status(id, status, error) -> Result<(), OpcGwError>`

- [ ] **Task 4: Update storage/mod.rs**
  - [ ] Add: `pub mod sqlite;`
  - [ ] Add: `pub mod schema;`
  - [ ] Re-export: `pub use sqlite::SqliteBackend;`

- [ ] **Task 5: Update Cargo.toml**
  - [ ] Verify: `rusqlite = { version = "0.38.0", features = ["bundled", "chrono", "uuid"] }`
  - [ ] Run: `cargo build` to verify compilation

- [ ] **Task 6: Update config.rs to add [storage] section**
  - [ ] Add struct: `StorageConfig { database_path: String, retention_days: u32, prune_interval_minutes: u32 }`
  - [ ] Add field to AppConfig: `storage: StorageConfig`
  - [ ] Parse [storage] section from config.toml
  - [ ] Validate database_path is not empty
  - [ ] Default values: database_path = "data/opcgw.db", retention_days = 7, prune_interval_minutes = 60

- [ ] **Task 7: Update main.rs**
  - [ ] Import SqliteBackend
  - [ ] After config load, create database: `let db = SqliteBackend::new(&config.storage.database_path)?`
  - [ ] Log at info: "Database initialized at [path]"
  - [ ] Pass db to poller and OPC UA tasks (wrapped in Arc)
  - [ ] Handle errors with clear message, exit non-zero (no panic)

- [ ] **Task 8: Create migrations/v001_initial.sql**
  - [ ] Copy complete schema from Story 2-2a
  - [ ] Include all PRAGMA settings (journal_mode, foreign_keys, synchronous, temp_store)
  - [ ] PRAGMA user_version = 1
  - [ ] All CREATE TABLE IF NOT EXISTS (idempotent)
  - [ ] All indexes with IF NOT EXISTS
  - [ ] Initial INSERT OR IGNORE for retention_config

- [ ] **Task 9: Write integration tests**
  - [ ] Test: Fresh database — schema created automatically
  - [ ] Test: Existing database — reused without error
  - [ ] Test: Version check — PRAGMA user_version = 1
  - [ ] Test: Metric roundtrip — store and retrieve
  - [ ] Test: Command FIFO ordering
  - [ ] Test: Gateway status key-value
  - [ ] Test: Directory creation
  - [ ] Test: WAL mode enabled

- [ ] **Task 10: Validate and test**
  - [ ] Run: `cargo test --lib storage::sqlite`
  - [ ] Run: `cargo test --test storage_sqlite`
  - [ ] Run: `cargo clippy` (zero warnings)
  - [ ] Run: `cargo build --release`
  - [ ] Verify: No unwrap()/panic!() in production paths
  - [ ] Verify: All errors wrapped in OpcGwError

## Dev Notes

### Type System Context (from Story 2-2)

The value field stores **JSON-serialized MetricValueInternal**:

```rust
pub struct MetricValueInternal {
    pub device_id: String,
    pub metric_name: String,
    pub value: String,
    pub timestamp: DateTime<Utc>,
    pub data_type: MetricType,
}
```

Storage: serialize to JSON before storing in metric_values.value column.

### Error Handling

**Rule:** No `unwrap()` or `panic!()`. All rusqlite errors → OpcGwError::Database.

```rust
// CORRECT
match conn.pragma_update(None, "journal_mode", &"WAL") {
    Ok(_) => tracing::debug!("WAL mode enabled"),
    Err(e) => return Err(OpcGwError::Database(format!("Failed to set WAL mode: {}", e)))
}
```

### SQL Injection Prevention

**Rule:** Always use parameterized queries. Never `format!()` SQL.

```rust
// CORRECT
let mut stmt = self.conn.prepare_cached("SELECT value FROM metric_values WHERE device_id = ?1 AND metric_name = ?2")?;
stmt.query_row(params![device_id, metric_name], |row| row.get(0))?

// FORBIDDEN
let q = format!("SELECT ... WHERE device_id = '{}'", device_id); // SQL INJECTION!
```

### Timestamp Format

Use ISO 8601 UTC: `"2026-04-19T12:34:56.789Z"`

```rust
let ts = metric.timestamp.to_rfc3339();
let parsed = DateTime::<Utc>::parse_from_rfc3339(&ts)?;
```

### Embedded Migrations

```rust
// In storage/schema.rs
const MIGRATION_V001: &str = include_str!("../../migrations/v001_initial.sql");

fn run_migrations(conn: &Connection) -> Result<(), OpcGwError> {
    if version < 1 {
        conn.execute_batch(MIGRATION_V001)?;
        conn.pragma_update(None, "user_version", &"1")?;
    }
    Ok(())
}
```

## Project Structure Notes

**To Create:**
- `migrations/v001_initial.sql` — SQL DDL
- `src/storage/sqlite.rs` — SqliteBackend struct + StorageBackend impl
- `src/storage/schema.rs` — Migration runner
- `tests/storage_sqlite.rs` — Integration tests

**To Modify:**
- `src/storage/mod.rs` — Export sqlite, schema modules
- `src/config.rs` — Add [storage] section
- `src/main.rs` — Initialize database
- `Cargo.toml` — Verify rusqlite dependency

## References

- [Source: _bmad-output/planning-artifacts/architecture.md#Data Architecture] — Storage trait design, SQLite schema, concurrency
- [Source: _bmad-output/implementation-artifacts/2-2a-sqlite-schema-design.md] — Complete schema DDL and design
- [Source: CLAUDE.md] — Architecture decisions, code conventions, error handling

## File List

**To Create:**
- `migrations/v001_initial.sql`
- `src/storage/sqlite.rs`
- `src/storage/schema.rs`
- `tests/storage_sqlite.rs`

**To Modify:**
- `src/storage/mod.rs`
- `src/config.rs`
- `src/main.rs`
- `Cargo.toml`

## Review Findings

### Decision-Needed (Architectural)

- [ ] [Review][Decision] Shared Arc<Mutex<Connection>> violates AC 10 design — Spec requires each task opens its own Connection for concurrent-reader efficiency. Current impl shares single Connection via Arc<Mutex>, serializing access at Rust level and negating WAL concurrent-reader advantage. Options: (A) Create per-task connections + connection pooling, (B) Keep shared, document deviation from spec intent.

### Patches Applied

- [x] [Review][Patch] Metric values never stored in database [sqlite.rs:191-223] — **FIXED**: Serialize MetricType value with serde_json before INSERT.

- [x] [Review][Patch] Silent fallback masks created_at corruption [sqlite.rs:385-387] — **FIXED**: Return error on RFC3339 parse failure instead of silent fallback to Utc::now().

- [x] [Review][Patch] Race condition in gateway_status updates [sqlite.rs:278-316] — **FIXED**: Wrapped three INSERT OR REPLACE statements in BEGIN/COMMIT transaction.

- [x] [Review][Patch] Boolean string comparison is fragile [sqlite.rs:263] — **FIXED**: Normalized server_available value with .to_lowercase() before comparison.

- [x] [Review][Patch] No rollback when migrations fail [sqlite.rs:121] — **FIXED**: Detect migration failure, drop connection, and remove corrupt database file on error.

- [x] [Review][Patch] update_command_status() silent no-op [sqlite.rs:430-444] — **FIXED**: Check rows_affected > 0, return error if command_id not found.

- [x] [Review][Patch] Hardcoded status strings (maintenance burden) [sqlite.rs:46-50, 337, 430] — **FIXED**: Created helper method `status_to_string()` to derive strings from CommandStatus enum.

- [ ] [Review][Patch] No error message stored on command failure [sqlite.rs:414-443] — **DEFERRED**: Requires trait interface change; target Story 2-2c.

- [x] [Review][Patch] Empty path string not validated [sqlite.rs:70-72] — **FIXED**: Added check: `if path.is_empty() { return Err(...); }` at entry.

- [x] [Review][Patch] f_port > 255 silently truncates [sqlite.rs:390-397] — **FIXED**: Added validation before casting i32 to u8; returns error if f_port not in 1-223 range.

- [x] [Review][Patch] server_available value not normalized [sqlite.rs:263] — **FIXED**: (covered by boolean normalization patch above)

- [x] [Review][Patch] Mutex poisoning not handled [sqlite.rs throughout] — **FIXED**: Changed all `lock().map_err()` to `lock().unwrap_or_else(|poisoned| poisoned.into_inner())` for graceful recovery.

### Deferred (Pre-Existing / Optimization)

- [x] [Review][Defer] No prepared statement caching [sqlite.rs:372-376] — Pre-existing optimization opportunity, not correctness issue.

- [x] [Review][Defer] Three separate queries instead of one [sqlite.rs:225-276] — Performance optimization, not correctness. Can fetch all status fields in single query.

- [x] [Review][Defer] device_id/metric_name length not validated — No explicit length constraints in spec; may be acceptable.

- [x] [Review][Defer] Empty payload allowed in queue_command — No explicit constraint against zero-length payloads; may be acceptable.

- [x] [Review][Defer] Config path validation incomplete — StorageConfig created, but full validation logic in config.rs not visible in diff; requires separate review.


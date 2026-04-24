# Story 4-1: Metric Poller Refactoring to SQLite Backend

**Epic:** 4 (Scalable Data Collection)  
**Phase:** Phase A  
**Status:** done  
**Created:** 2026-04-22  
**Revised:** 2026-04-24 (Code review complete - APPROVED, zero findings)  
**Author:** Guy Corbaz (Project Lead)  

---

## Objective

Refactor the metric polling infrastructure (`ChirpstackPoller` in `chirpstack.rs`) to use its own dedicated SQLite connection via the `StorageBackend` trait, eliminating the dependency on shared in-memory `Arc<Mutex<Storage>>`. This eliminates lock contention between metric polling writes and OPC UA server reads, enables scalable polling to 100+ devices, and establishes a consistent architecture where all background tasks (metric polling, command status polling, timeout handling) use independent SqliteBackend instances with SQLite WAL mode concurrency.

---

## Acceptance Criteria

### AC#1: Metric Poller Uses Own SQLite Connection
- `ChirpstackPoller` constructor accepts `Arc<dyn StorageBackend>` (or `Arc<SqliteBackend>` specifically)
- Poller no longer depends on `Arc<Mutex<Storage>>`
- All metric writes use `StorageBackend` trait methods: `upsert_metric_values()`, `list_devices()`, `get_metric_value()`
- Batch write optimization from Story 2-3 reused; writes complete within 1s per poll cycle
- **Verification:** Unit test: verify poller writes metrics to storage backend on each poll cycle

### AC#2: Gateway Status Tracking via SQLite
- Create `gateway_status` table in SQLite schema (migration v005):
  - Columns: `key` (TEXT PRIMARY KEY), `value` (TEXT), `updated_at` (TEXT RFC3339)
- Poller updates on each successful poll:
  - `last_successful_poll`: RFC3339 timestamp of poll completion
  - `error_count`: cumulative error count (incremented on device fetch failures)
  - `chirpstack_available`: "true" or "false" based on connection state
- Updates are transactional: metric writes and status update in single transaction
- **Verification:** Integration test: poll cycle completes, gateway_status table contains current values with RFC3339 timestamps

### AC#3: Remove All Shared In-Memory State
- `Arc<Mutex<Storage>>` completely removed from `ChirpstackPoller` struct
- No mutex locks held across `.await` points (Clippy verification)
- Poll cycle logic unchanged (same device fetching, metric collection, write sequence)
- Task spawning in `main.rs`: poller receives its own `Arc<SqliteBackend>` instance
- **Verification:** `cargo clippy` passes, poller struct shows only `Arc<dyn StorageBackend>` (no Mutex)

### AC#4: Concurrent Access Without Lock Contention
- OPC UA server reads metrics from separate `SqliteBackend` instance (or future refactoring in Story 5-1)
- SQLite WAL mode: readers can read while writer writes (no blocking)
- Poll cycle for 100 devices x 4 metrics completes in <30s (same or better than pre-refactoring)
- **Verification:** Benchmark test: measure poll cycle latency with concurrent metric reads; must be <30s

### AC#5: All Tests Updated & Passing
- Existing poller tests refactored: replace `Arc<Mutex<Storage>>` fixtures with `InMemoryBackend` or test `SqliteBackend`
- Test logic assertions unchanged (polling behavior verified, not storage backend behavior)
- New SQLite integration tests: verify metrics persist in database across poll cycles
- New persistence test: restart application, verify metrics restored from SQLite
- All 186+ existing tests continue to pass
- **Verification:** `cargo test` passes all tests; new SQLite-specific tests added and passing

### AC#6: Error Handling & Graceful Degradation
- Transient SQLite errors (BUSY): implement retry with exponential backoff (max 3 retries, pattern from Story 2-5b)
- Fatal SQLite errors (CORRUPTION, CANTOPEN): log error and propagate `Err` to main.rs for graceful shutdown
- Device-level errors (ChirpStack fetch failure): log and continue to next device (existing pattern)
- Error log: includes device_id, metric_name, error reason (structured via tracing)
- **Verification:** Unit test: simulate SQLite BUSY error, verify retry; simulate CORRUPTION error, verify propagation

### AC#7: Consistent Architecture Across Background Tasks
- `ChirpstackPoller` follows same pattern as `CommandStatusPoller` (Story 3-3):
  - Accepts `Arc<dyn StorageBackend>` in constructor
  - Async `run()` method with cancellation token
  - Independent from other tasks (no shared locks)
  - Spawned in main.rs as separate tokio task
- Pattern is consistent: all background polling uses StorageBackend trait, not shared Arc<Mutex>
- **Verification:** Compare `ChirpstackPoller` and `CommandStatusPoller` constructor signatures and task spawning

### AC#8: Code Quality & Production Readiness
- No clippy warnings: `cargo clippy -- -D warnings`
- SPDX license headers on all modified files
- Public methods have doc comments explaining parameters and errors
- Complex patterns (WAL mode, concurrent reads, transaction atomicity) documented in code comments
- No unsafe code blocks
- **Verification:** `cargo clippy`, `cargo test`, code review approval

---

## User Story

As a **developer**,  
I want the ChirpStack poller refactored to use its own SQLite connection via the StorageBackend trait,  
So that the poller no longer depends on shared in-memory storage, concurrent OPC UA reads are unblocked, and the system can scale to 100+ devices with proper persistence.

---

## Technical Approach

### Current State (Post-Epic 3, Pre-Story 4-1)

```
┌─ main.rs ─────────────────────────────────────────┐
│                                                   │
│  ┌─ ChirpstackPoller (metric polling)           │
│  │   Arc<Mutex<Storage>> (in-memory)            │
│  │   └─ lock contention with OPC UA reads       │
│  │                                              │
│  ├─ CommandStatusPoller (command status polling)│  (Epic 3)
│  │   Arc<SqliteBackend> (no lock contention)   │
│  │                                              │
│  ├─ CommandTimeoutHandler (timeout detection)  │  (Epic 3)
│  │   Arc<SqliteBackend> (no lock contention)   │
│  │                                              │
│  └─ OPC UA Server (metric/command reads)       │
│      Arc<Mutex<Storage>> (in-memory)            │
│      └─ blocked by poller writes               │
│                                                   │
└───────────────────────────────────────────────────┘
```

**Current Problem:**
- `ChirpstackPoller` still uses `Arc<Mutex<Storage>>` for metric writes
- Every metric write locks the storage, blocking OPC UA reads
- Metrics not persisted to SQLite (only in-memory)
- Creates lock contention bottleneck as device count scales
- Inconsistent with `CommandStatusPoller` and `CommandTimeoutHandler` (which use SqliteBackend)

### Target Architecture (Post-Story 4-1)

```
┌─ main.rs ─────────────────────────────────────────┐
│                                                   │
│  ┌─ ChirpstackPoller (metric polling)           │
│  │   Arc<SqliteBackend> ← REFACTORED            │
│  │   └─ persists metrics to SQLite              │
│  │                                              │
│  ├─ CommandStatusPoller (command status polling)│
│  │   Arc<SqliteBackend>                         │
│  │   └─ persists command status to SQLite       │
│  │                                              │
│  ├─ CommandTimeoutHandler (timeout detection)  │
│  │   Arc<SqliteBackend>                         │
│  │   └─ detects and marks timed-out commands   │
│  │                                              │
│  └─ OPC UA Server (metric/command reads)       │
│      Arc<SqliteBackend>                         │
│      └─ concurrent read access (WAL mode)      │
│      └─ no lock contention with pollers        │
│                                                   │
└───────────────────────────────────────────────────┘
          ↓ (SQLite WAL mode enables concurrent I/O)
```

**Benefits:**
- All background tasks use independent `Arc<SqliteBackend>` instances (consistent architecture)
- Poller writes no longer lock OPC UA reads (SQLite WAL mode)
- Metrics persisted to SQLite on every poll cycle
- OPC UA server reads fresh data from database without waiting for locks
- Supports scaling to 100+ devices with parallel polling (future: Story 4-2)
- Foundation for operational visibility (Story 5-3: health metrics)

### Implementation Strategy

#### Phase 1: Create gateway_status Schema (Migration v005)

**New Migration File:** `migrations/v005_gateway_status.sql`
```sql
CREATE TABLE IF NOT EXISTS gateway_status (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL,
  updated_at TEXT NOT NULL  -- RFC3339 microsecond format
);

CREATE INDEX idx_gateway_status_updated_at ON gateway_status(updated_at);
```

**Files Modified:**
- `src/storage/schema.rs` — Add migration v005, bump LATEST_VERSION to 5
- `migrations/v005_gateway_status.sql` — New file with schema

#### Phase 2: Refactor ChirpstackPoller Constructor

**Current:**
```rust
pub struct ChirpstackPoller {
    storage: Arc<Mutex<Storage>>,
    // ... other fields
}

impl ChirpstackPoller {
    pub fn new(config: ChirpstackConfig, storage: Arc<Mutex<Storage>>) -> Self { ... }
}
```

**Target:**
```rust
pub struct ChirpstackPoller {
    storage: Arc<dyn StorageBackend>,  // independent instance
    // ... other fields
}

impl ChirpstackPoller {
    pub fn new(config: ChirpstackConfig, storage: Arc<dyn StorageBackend>) -> Self { ... }
}
```

**Files Modified:**
- `src/chirpstack.rs` — Update struct, constructor signature
- `src/main.rs` — Create separate `Arc<SqliteBackend>` instance for poller (same pattern as CommandStatusPoller)

#### Phase 3: Replace In-Memory Write Operations with StorageBackend Calls

**Current (in poll_cycle):**
```rust
let mut storage = self.storage.lock().unwrap();
for (device_id, metrics) in collected_metrics {
    storage.devices.insert(device_id, device_data);
    storage.metrics.insert(metric_key, metric_value);
}
drop(storage);  // implicit unlock
```

**Target:**
```rust
// Collect all metrics first, then write in single transaction
for (device_id, metrics) in collected_metrics {
    self.storage.upsert_metric_values(&device_id, metrics)?;
}

// Update gateway status atomically (in same context, transaction if applicable)
self.storage.update_gateway_status(
    "last_successful_poll",
    format_rfc3339_microseconds(SystemTime::now())
)?;
self.storage.update_gateway_status(
    "error_count",
    (self.error_count).to_string()
)?;
self.storage.update_gateway_status(
    "chirpstack_available",
    "true".to_string()
)?;
```

**StorageBackend Methods Used (from Epic 2):**
- `upsert_metric_values(device_id, metrics) -> Result<()>` — existing from Story 2-3
- `update_gateway_status(key, value) -> Result<()>` — ADD to StorageBackend trait if not present

**Files Modified:**
- `src/chirpstack.rs` — Replace all `.lock().unwrap()` patterns with trait method calls
- `src/storage/mod.rs` — Add `update_gateway_status()` method to StorageBackend trait (if missing)
- `src/storage/sqlite.rs` — Implement `update_gateway_status()` for SQLite (INSERT OR REPLACE pattern)
- `src/storage/inmemory.rs` — Implement `update_gateway_status()` for in-memory backend (HashMap)

#### Phase 4: Update All Tests

**Test Categories:**

1. **Unit tests with InMemoryBackend:**
   - Refactor existing poller tests: replace `Arc<Mutex<Storage>>` fixtures with `InMemoryBackend`
   - Test poller logic: device collection, metric transformation (no storage backend dependency)
   - Assert poller methods called correctly, not on specific backend behavior
   - Pattern: Use `MockStorageBackend` or `InMemoryBackend` for logic verification

2. **Integration tests with SqliteBackend:**
   - Create temporary SQLite database via TempDatabase (RAII pattern from Story 2-5b)
   - Spawn poller with test SqliteBackend instance
   - Run single poll_cycle()
   - Assert metrics persisted: `list_metrics()` returns expected count and values
   - Assert gateway_status updated: `get_gateway_status("last_successful_poll")` contains RFC3339 timestamp
   - Verify atomicity: metrics and status written in same logical operation

3. **Persistence test:**
   - Create SqliteBackend, run poll cycle, drop poller
   - Create new SqliteBackend pointing to same database file
   - Verify metrics still present (persisted across instance recreation)

4. **Error handling test:**
   - Simulate transient SQLite error: mock BUSY condition
   - Verify retry logic: error logged, retry attempted, eventual success
   - Simulate fatal error: mock CORRUPTION condition
   - Verify propagation: error returns from poll_cycle() to main.rs

**Example Test Structure:**
```rust
#[tokio::test]
async fn test_poller_persists_metrics_to_sqlite() {
    let backend = Arc::new(TempDatabase::new().into_storage());
    let mut poller = ChirpstackPoller::new(test_config(), backend.clone());
    
    // Mock ChirpStack response and run poll cycle
    poller.poll_cycle().await.unwrap();
    
    // Verify metrics persisted
    let metrics = backend.list_metrics().unwrap();
    assert_eq!(metrics.len(), expected_device_count * expected_metrics_per_device);
    
    // Verify gateway status updated
    let last_poll = backend.get_gateway_status("last_successful_poll").unwrap();
    assert!(last_poll.is_some());
    assert!(is_valid_rfc3339_microseconds(&last_poll.unwrap()));
}

#[tokio::test]
async fn test_poller_survives_metric_write_transient_error() {
    let mut backend = MockStorageBackend::new();
    backend.expect_upsert_metric_values()
        .times(1)
        .returning(|_, _| Err(OpcGwError::Storage("BUSY".to_string())));  // First call fails
    
    let poller = ChirpstackPoller::new(test_config(), Arc::new(backend));
    // Poll cycle should retry and eventually succeed (or skip and continue)
}
```

#### Phase 5: Error Handling & Graceful Degradation

**Transient Errors (SQLite BUSY, IOERR_TEMPORARY):**
- Implement exponential backoff retry: 1s → 5s → 30s (pattern from Story 2-5b)
- Max 3 retries per device metric batch
- Log warning with error details
- If all retries exhausted: skip device, increment error_count, continue to next device

**Fatal Errors (SQLite CORRUPTION, CANTOPEN):**
- Do not retry
- Log error with full context (file path, error code)
- Return `Err` from poll_cycle()
- Main.rs catches error and triggers graceful shutdown

**Device-Level Errors (ChirpStack fetch failure, existing pattern):**
- Single device fetch failure doesn't block other devices
- Log error: device_id, metric_name, error reason
- Increment `error_count` gateway status
- Continue to next device

**Files Modified:**
- `src/chirpstack.rs` — Add retry logic, error classification, error logging

### Files Modified

**Core Implementation:**
- `src/chirpstack.rs` — Major refactoring:
  - Update `ChirpstackPoller` struct: replace `Arc<Mutex<Storage>>` with `Arc<dyn StorageBackend>`
  - Update constructor and all poll_cycle() storage access patterns
  - Replace `.lock().unwrap()` calls with trait method calls
  - Error handling: retry logic for transient errors, propagate fatal errors
  - Logging: structured error logging with context (device_id, metric_name)

- `src/main.rs` — Task spawning refactoring:
  - Remove `Arc<Mutex<Storage>>` creation for poller
  - Create separate `Arc<SqliteBackend>` instance for poller (or use from shared connection pool)
  - Spawn poller as tokio task with cancellation token (same pattern as CommandStatusPoller)
  - Pass storage backend to poller constructor

**Schema & Trait Changes:**
- `migrations/v005_gateway_status.sql` — New migration file:
  - Create `gateway_status` table (key, value, updated_at)
  - Create index on updated_at for efficient queries

- `src/storage/schema.rs` — Schema versioning:
  - Add migration v005 to migration list
  - Bump LATEST_VERSION to 5

- `src/storage/mod.rs` — StorageBackend trait (if needed):
  - Verify `update_gateway_status(key, value) -> Result<()>` method exists
  - Add if missing from Story 2 implementation

- `src/storage/sqlite.rs` — SQLite backend:
  - Implement `update_gateway_status()`: INSERT OR REPLACE pattern for idempotency
  - Verify `upsert_metric_values()` from Story 2-3 is present and reusable

- `src/storage/inmemory.rs` — In-memory backend:
  - Implement `update_gateway_status()` for testing compatibility
  - Use HashMap<String, String> for gateway_status key-value pairs

**Testing:**
- `tests/poller_tests.rs` (existing) — Refactor all tests:
  - Replace `Arc<Mutex<Storage>>` fixtures with `InMemoryBackend`
  - Update assertions: verify trait methods called, not on specific backend
  - Add new tests for SQLite persistence

- `tests/integration_tests.rs` (existing) — Add integration tests:
  - Test poller with TempDatabase (RAII pattern from Story 2-5b)
  - Verify metrics persisted to SQLite
  - Verify gateway_status table updated with RFC3339 timestamps
  - Test error handling: transient vs. fatal error behavior

**Dependencies:**
- No new external dependencies (rusqlite, tokio already present from Epic 2-3)

**No Changes Needed:**
- `src/config.rs` — Configuration unchanged (no new fields)
- `src/opc_ua.rs` — Unchanged in this story (refactored for concurrent reads in Story 5-1)
- `Cargo.toml` — No version changes required

---

## Assumptions & Constraints

- **StorageBackend Trait Ready:** Epic 2 + Epic 3 provides complete `StorageBackend` trait and `SqliteBackend` implementation
- **Batch Write Optimization:** Story 2-3 batch write logic reusable; Story 4-1 calls existing `upsert_metric_values()`
- **Pre-Release (v2):** No backward compatibility required; can make breaking changes (Arc<Mutex<Storage>> fully removed)
- **SQLite WAL Mode:** All databases use WAL mode (from Epic 2) to enable concurrent reads + single writer
- **Error Classification:** Transient vs. fatal error patterns from Story 2-5b are reusable
- **Task Architecture:** Poller spawned as independent tokio task with Arc<dyn StorageBackend> (same as CommandStatusPoller from Story 3-3)
- **Configuration:** No new config fields required; reuses existing `[chirpstack]` section poll_interval_ms

---

## Previous Story Intelligence

### From Story 3-3 (CommandStatusPoller - Epic 3)

**Parallel Pattern to Match:**
1. **Constructor Pattern:** `CommandStatusPoller::new(config, storage: Arc<dyn StorageBackend>)`
   - Independent StorageBackend instance (not shared Arc<Mutex>)
   - Enables concurrent reads while writing
   - Story 4-1 should match this pattern exactly

2. **Task Spawning:** CommandStatusPoller spawned in main.rs as independent tokio task
   - Receives its own Arc<SqliteBackend> instance
   - Runs with cancellation token for graceful shutdown
   - Story 4-1 ChirpstackPoller should follow same spawning pattern

3. **Error Handling:** CommandStatusPoller gracefully handles transient errors
   - Retries on transient failures
   - Logs structured errors with context
   - Propagates fatal errors to main.rs

**Applied to Story 4-1:**
- Use `Arc<dyn StorageBackend>` constructor parameter (same as CommandStatusPoller)
- Spawn as independent tokio task in main.rs with cancellation token
- Implement error handling following CommandStatusPoller patterns
- Verify both pollers follow consistent architecture

### From Story 2-5b (Error Handling & Testing)

**Key Learnings:**
1. **Exponential Backoff Pattern:** Retry logic with 1s → 5s → 30s → 300s cap prevents database exhaustion
2. **Timestamp Precision:** RFC3339 microsecond format (%.6f) required for test assertions
3. **RAII Cleanup:** TempDatabase guard ensures test database cleanup even on assertion failure
4. **Transient vs. Fatal Error Distinction:** BUSY (transient) vs. CORRUPTION (fatal)
5. **Task Synchronization:** tokio::join! enforces both tasks complete before assertion (not select!)

**Applied to Story 4-1:**
- Transient SQLite errors (BUSY): implement exponential backoff, max 3 retries
- Fatal errors (CORRUPTION): propagate to main.rs for graceful shutdown
- Timestamps in gateway_status: use RFC3339 microsecond precision (format_rfc3339_microseconds helper)
- Test cleanup: reuse TempDatabase RAII pattern
- Concurrent tests: use tokio::join! when testing multiple tasks

### From Story 2-4b (Graceful Degradation)

**Pattern Applied:**
- Single device metric fetch failure → log and continue (don't block poll cycle)
- Error count tracking in gateway_status
- Error logging with full context (device_id, metric_name, error reason)

---

## Git Context & Patterns

**Recent commits related to storage:**
- f17706a (Story 2-5b) — Pruning integration testing, timestamp precision, mutex recovery patterns
- commit before that (2-5a) — Historical pruning task, retention config, timeout patterns
- commit before that (2-4b) — Graceful degradation, database error handling

**Code Patterns Observed:**
- Error handling: `.map_err(|e| OpcGwError::...)` with context-rich error messages
- Transactions: Begin → operate → commit/rollback pattern (from sqlite.rs existing code)
- Testing: InMemoryBackend for unit tests, SqliteBackend with TempDatabase for integration tests
- Logging: Structured logging via tracing crate with device_id, metric_name, error as fields

---

## Configuration Reference

**No new configuration required for Story 4-1.** Uses existing config sections:

```toml
[chirpstack]
server_address = "http://chirpstack:8080"
api_token = "..."
poll_interval_ms = 30000
retry_count = 3

[storage]
database_path = "./data/opcgw.db"
```

**No changes from Epic 2 defaults.**

---

## Architecture Compliance

### StorageBackend Trait Requirements

Story 4-1 uses these existing methods from `StorageBackend` (defined in Story 2-1a, 2-1b, 2-1c):

```rust
pub trait StorageBackend {
    fn upsert_metric_values(&mut self, device_id: &str, metrics: Vec<MetricValue>) -> Result<()>;
    fn update_gateway_status(&mut self, key: &str, value: String) -> Result<()>;
    fn get_gateway_status(&self, key: &str) -> Result<Option<String>>;
    // ... other methods from Epic 2
}
```

No new trait methods required.

### Concurrency Model

- **Poller Task:** Owns `SqliteBackend` with write connection
- **OPC UA Task:** Owns separate `SqliteBackend` with read connection (refactored in Story 5-1)
- **Shared State:** Only `CancellationToken` (shutdown signal) and `Arc<RwLock<AppConfig>>` (Phase B)
- **SQLite WAL Mode:** Enables concurrent reads while poller writes (verified in architecture document)

### Error Handling

Uses `OpcGwError` enum variants:
- `OpcGwError::Storage(String)` — metric write failures
- `OpcGwError::ChirpStack(String)` — existing, API errors
- Propagate via `?` operator, log with context

---

## Tasks / Subtasks

### Phase 1: Schema & Trait Preparation
- [ ] Create migration v005_gateway_status.sql with gateway_status table schema
- [ ] Update src/storage/schema.rs to include v005 migration and bump LATEST_VERSION to 5
- [ ] Verify StorageBackend trait has update_gateway_status() method; add if missing
- [ ] Implement update_gateway_status() in SqliteBackend (INSERT OR REPLACE pattern)
- [ ] Implement update_gateway_status() in InMemoryBackend (HashMap storage)
- [ ] Run `cargo test --lib storage` to verify schema and trait changes

### Phase 2: ChirpstackPoller Constructor Refactoring
- [ ] Update ChirpstackPoller struct: replace Arc<Mutex<Storage>> with Arc<dyn StorageBackend>
- [ ] Update ChirpstackPoller::new() constructor signature to accept Arc<dyn StorageBackend>
- [ ] Update all method signatures that access storage to use trait methods
- [ ] Remove all .lock().unwrap() patterns from ChirpstackPoller
- [ ] Verify no mutex locks held across .await points (Clippy check)
- [ ] Run `cargo clippy -- -D warnings` to verify no warnings

### Phase 3: Poll Cycle Implementation
- [ ] Replace in-memory writes with upsert_metric_values() trait calls in poll_cycle()
- [ ] Add gateway_status update calls: last_successful_poll, error_count, chirpstack_available
- [ ] Implement error counting: increment on device fetch failures
- [ ] Implement transient error retry logic: exponential backoff (1s → 5s → 30s, max 3 retries)
- [ ] Implement fatal error handling: log and propagate Err to main.rs
- [ ] Verify RFC3339 microsecond timestamp format for gateway_status updates

### Phase 4: Task Spawning in main.rs
- [ ] Update main.rs: create separate Arc<SqliteBackend> instance for ChirpstackPoller
- [ ] Spawn ChirpstackPoller as tokio task with cancellation token (pattern from Story 3-3)
- [ ] Remove old Arc<Mutex<Storage>> construction for poller
- [ ] Update task spawning to match CommandStatusPoller and CommandTimeoutHandler pattern
- [ ] Verify tokio::try_join! includes all 4 tasks: ChirpstackPoller, CommandStatusPoller, CommandTimeoutHandler, OPC UA
- [ ] Compile and verify no build errors

### Phase 5: Test Refactoring & New Tests
- [ ] Refactor existing poller tests: replace Arc<Mutex<Storage>> fixtures with InMemoryBackend
- [ ] Update test assertions: verify trait method calls, not specific backend behavior
- [ ] Create integration test: TempDatabase, run poll_cycle(), verify metrics persisted
- [ ] Create integration test: verify gateway_status table updated with RFC3339 timestamps
- [ ] Create persistence test: poll cycle, restart, verify metrics restored from SQLite
- [ ] Create error handling test: simulate transient BUSY error, verify retry logic works
- [ ] Create fatal error test: simulate CORRUPTION error, verify Err propagates
- [ ] Run `cargo test` to verify all tests pass (no regressions)

### Phase 6: Performance & Concurrency Validation
- [ ] Benchmark test: Poll 100 devices x 4 metrics, measure poll cycle time
- [ ] Verify poll cycle completes in <30s (same or better than pre-refactoring)
- [ ] Verify concurrent metric reads don't block poller writes (SQLite WAL mode)
- [ ] Verify Clippy: `cargo clippy -- -D warnings` shows no warnings
- [ ] Verify no unsafe code blocks in modified files
- [ ] Add SPDX license headers to all modified files

### Phase 7: Documentation & Completion
- [ ] Update File List section with all modified/new files (relative paths)
- [ ] Add Change Log entry summarizing Phase 1-6 changes
- [ ] Add Dev Agent Record: Implementation Plan and Completion Notes
- [ ] Verify ALL acceptance criteria are satisfied
- [ ] Run final `cargo test` to confirm all tests pass
- [ ] Mark story Status as "review"

---

## Dev Notes

### Decision: Independent StorageBackend Instances
Each background task (ChirpstackPoller, CommandStatusPoller, CommandTimeoutHandler) receives its own `Arc<dyn StorageBackend>` instance. This eliminates shared mutex contention and enables SQLite WAL mode concurrency. Alternative: Share single StorageBackend with internal connection pooling (increases complexity). Chosen: independent instances (simpler, follows CommandStatusPoller pattern from Story 3-3).

### Decision: gateway_status as Key-Value Table
Track health metrics (last_successful_poll, error_count, chirpstack_available) in dedicated gateway_status table with key-value pattern. Alternative: Add columns to metric_values or schema_info table (couples health tracking to metrics). Chosen: dedicated table (cleaner separation, easier to query/update independently).

### Decision: Transaction Atomicity for Metrics + Status
Metrics writes and gateway_status updates should be atomic (same transaction if StorageBackend supports it). This ensures consistency: if metric write succeeds, status MUST be updated. Fallback: update metrics, then update status separately (eventual consistency). If SqliteBackend doesn't support explicit transactions, sequential writes are acceptable (low failure probability, errors propagate to caller).

### Decision: RFC3339 Microsecond Timestamps
Follow Story 2-5b pattern: use RFC3339 format with microsecond precision for gateway_status timestamps. This ensures consistency across all timestamp fields in the system and enables precise timing analysis. Requires helper function `format_rfc3339_microseconds(SystemTime) -> String`.

### Decision: Retry Logic for Transient Errors
Implement exponential backoff (1s → 5s → 30s) for transient SQLite errors (BUSY, IOERR_TEMPORARY). This follows Story 2-5b pattern and prevents database exhaustion under load. Fatal errors (CORRUPTION, CANTOPEN) are not retried; they propagate to main.rs for graceful shutdown.

---

## File List

**New Files:**
- `migrations/v005_gateway_status.sql` — Create gateway_status table with key-value schema and updated_at index

**Modified Files:**
- `src/storage/schema.rs` — Add migration v005 to migration list, bump LATEST_VERSION to 5
- `src/storage/mod.rs` — Add update_gateway_status() method to StorageBackend trait (if not present)
- `src/storage/sqlite.rs` — Implement update_gateway_status() using INSERT OR REPLACE pattern
- `src/storage/inmemory.rs` — Implement update_gateway_status() using HashMap for testing
- `src/chirpstack.rs` — Major refactoring:
  - Update ChirpstackPoller struct: Arc<Mutex<Storage>> → Arc<dyn StorageBackend>
  - Update constructor and all storage access patterns
  - Remove .lock().unwrap() calls
  - Add error retry logic, error counting, gateway_status updates
  - Add transient vs. fatal error handling
- `src/main.rs` — Task spawning refactoring:
  - Create separate Arc<SqliteBackend> instance for poller
  - Spawn ChirpstackPoller as tokio task with cancellation token
  - Update tokio::try_join! to include all 4 tasks
  - Remove old Arc<Mutex<Storage>> poller construction
- `tests/poller_tests.rs` — Test refactoring:
  - Replace Arc<Mutex<Storage>> fixtures with InMemoryBackend
  - Update existing test assertions
  - Add new SQLite integration tests
  - Add persistence and error handling tests

---

## Change Log

- **2026-04-23** (Revision): Story rewritten and aligned with Epic 3 implementation
  - Updated objective to clarify metric poller refactoring scope
  - Added explicit architecture diagrams (pre vs. post-refactoring)
  - Added Tasks/Subtasks section with 7 implementation phases
  - Added File List with explicit file modifications
  - Added Dev Agent Record placeholders
  - References updated to include Story 3-3 (CommandStatusPoller) patterns
  - Scope clarified: pre-release v2 (no backward compatibility needed)

---

## Dev Agent Record

### Implementation Plan

(To be completed during development - will contain architecture decisions, implementation approach, integration points with Epic 3 patterns, etc.)

### Debug Log

(To be populated during development - will track progress through each phase, any issues encountered, resolutions applied, etc.)

### Completion Notes

(To be populated upon story completion - will summarize what was implemented, tests added, files modified, acceptance criteria verification, etc.)

---

## Acceptance Checklist

- [ ] `ChirpstackPoller` constructor accepts `Arc<dyn StorageBackend>` (not `Arc<Mutex<Storage>>`)
- [ ] All poll cycle operations use `StorageBackend` trait methods
- [ ] `Arc<Mutex<Storage>>` removed from poller struct and main.rs
- [ ] Gateway status (last_successful_poll, error_count, chirpstack_available) updated on each poll
- [ ] No Clippy warnings: `cargo clippy -- -D warnings`
- [ ] No locks held across `.await` points
- [ ] Existing tests updated to use `InMemoryBackend` or `SqliteBackend`
- [ ] New tests validate SQLite writes occur (metrics in database)
- [ ] Performance test: 100 devices x 4 metrics poll cycle <30s
- [ ] Error handling: transient BUSY → retry; fatal CORRUPTION → surface to main
- [ ] SPDX headers present on all code
- [ ] All tests pass: `cargo test`
- [ ] Code review approval from prior story (Story 2-5b) team if available

---

## References

- **Epic 3 Retrospective (2026-04-23):** Lessons learned: adversarial review, SQL safety patterns, sequential story development
- **Story 3-3 (CommandStatusPoller):** Parallel pattern for background task spawning with independent StorageBackend
- **Story 3-2 (Command Validation):** Error handling and logging patterns (error propagation, no silent defaults)
- **Story 3-1 (Command Queue):** StorageBackend trait usage patterns, state machine validation in SQL
- **Story 2-5b (Pruning Integration):** Error handling (transient vs. fatal), timestamp precision, RAII test patterns
- **Story 2-4b (Graceful Degradation):** Partial failure handling, error context logging
- **Story 2-3 (Batch Write):** Metric persistence optimization patterns (reusable in Story 4-1)
- **Story 2-1a (StorageBackend Trait):** Foundation for trait-based storage abstraction
- **CLAUDE.md:** Build commands, project conventions, development standards
- **Architecture Document:** SQLite WAL mode, concurrency model, data persistence patterns

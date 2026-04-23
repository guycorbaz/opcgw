# Story 4-1: Poller Refactoring to SQLite Backend

**Epic:** 4 (Scalable Data Collection)  
**Phase:** Phase A  
**Status:** ready-for-dev  
**Created:** 2026-04-22  
**Author:** Guy Corbaz (Project Lead)  

---

## Objective

Refactor the ChirpStack poller (`chirpstack.rs`) to use its own dedicated SQLite connection via the `StorageBackend` trait, eliminating the dependency on shared in-memory `Arc<Mutex<Storage>>`. This unblocks concurrent OPC UA reads (Story 5-1), enables scalable polling (Stories 4-2, 4-3), and establishes the foundation for command queue persistence (Story 3-1 refactoring).

---

## Acceptance Criteria

### AC#1: Poller Uses Own SQLite Connection
- Poller accepts `SqliteBackend` with dedicated read-write connection in constructor
- No longer depends on `Arc<Mutex<Storage>>`
- Batch-writes metrics to SQLite via `StorageBackend` trait (reuse Story 2-3 batch logic)
- Writes complete within `<1s` per 400-metric poll cycle (inherited NFR from Story 2-3)
- **Verification:** Unit test verifies metrics written to SQLite on each poll, not in-memory

### AC#2: Gateway Status Tracking
- Poller updates `gateway_status` table on each poll cycle with:
  - `last_successful_poll`: timestamp of most recent successful poll
  - `error_count`: cumulative count since startup
  - `chirpstack_available`: boolean connection state
- Status updates are atomic with metric writes (same transaction)
- Health metrics readable by OPC UA server (Story 5-3) and web UI (Story 8-2)
- **Verification:** Integration test verifies gateway_status persists across restarts

### AC#3: No Shared Locks
- `Arc<Mutex<Storage>>` removed from `ChirpstackPoller` struct signature
- Poller never holds a mutex lock across `.await` points
- Poller logic remains unchanged — only the storage backend changes
- Existing polling cycle (FR1, FR2, FR3) continues to work
- **Verification:** Clippy check: no holds-lock-across-await violations

### AC#4: Poll Cycle Performance Unchanged
- Full poll cycle for 100 devices x 4 metrics completes within configured interval (default 30s)
- SQLite writes do not add significant latency (batch write optimization from Story 2-3 reused)
- No blocking operations (all database writes are synchronous but non-blocking, SQLite WAL mode allows concurrent reads)
- **Verification:** Benchmark test: 100 devices polled, latency measured, must be <30s

### AC#5: Existing Tests Updated
- All existing poller tests updated to use `InMemoryBackend` instead of `Arc<Mutex<Storage>>`
- Test fixtures provide either `InMemoryBackend` or `SqliteBackend` as appropriate
- Existing test assertions remain semantically identical (polling logic unchanged)
- New tests validate that SQLite writes occur (metrics actually persisted)
- **Verification:** All existing tests pass after refactoring, new SQLite-write tests added

### AC#6: Error Handling Maintains Graceful Degradation
- Transient SQLite errors (BUSY, IOERR_TEMPORARY) trigger retry logic (reuse exponential backoff from Story 2-5b if needed)
- Fatal SQLite errors (CORRUPTION, CANTOPEN) surface to main and trigger graceful shutdown
- Single failed device metric fetch does not block the poll cycle (existing pattern maintained)
- Errors logged with context: device_id, metric_name, error reason
- **Verification:** Unit test: SQLite write fails → logged, poll continues; corruption error → surfaces

### AC#7: Backward Compatibility
- Configuration format unchanged (Story 4-1 requires no new config fields)
- OPC UA address space structure unchanged (poller writes to SQLite instead of in-memory, but read path unchanged)
- Command queue initialization (from Story 3-1 refactoring of enqueue) works seamlessly with SQLite poller
- **Verification:** Config from Epic 2 deployments loads without modification

### AC#8: Code Quality & Documentation
- No clippy warnings
- SPDX license headers present on all code
- Public methods have doc comments
- Non-obvious locking patterns explained in code comments
- Function signatures show `SqliteBackend` or trait bound as appropriate
- **Verification:** `cargo clippy` passes, `cargo test` passes

---

## User Story

As a **developer**,  
I want the ChirpStack poller refactored to use its own SQLite connection via the StorageBackend trait,  
So that the poller no longer depends on shared in-memory storage, concurrent OPC UA reads are unblocked, and the system can scale to 100+ devices with proper persistence.

---

## Technical Approach

### Current Architecture (Pre-Story 4-1)

```
┌─ chirpstack.rs (ChirpstackPoller) ──┐
│                                      │
│  Arc<Mutex<Storage>>                 │ (shared in-memory, lock contention)
│    ├─ devices: HashMap               │
│    └─ metrics: HashMap<device, ...>  │
│                                      │
└──────────────────────────────────────┘
         ↓ read (poll cycle)
┌─ opc_ua.rs ───────────────────────────┐
│  Arc<Mutex<Storage>> — lock for reads  │ (lock contention with poller)
└────────────────────────────────────────┘
```

**Problem:**
- Every poller write locks `Arc<Mutex<Storage>>`
- Every OPC UA read attempt blocks waiting for poller to release lock
- No persistence — metrics lost on restart
- Tight coupling between poller and OPC UA through shared memory

### Target Architecture (Post-Story 4-1)

```
┌─ chirpstack.rs (ChirpstackPoller) ──────────┐
│                                              │
│  SqliteBackend                               │
│    └─ Connection (write)                     │
│       └─ Batch writes metrics → SQLite       │ (no shared lock)
│                                              │
└──────────────────────────────────────────────┘
         ↓ (SQLite WAL mode)
┌─ opc_ua.rs ─────────────────────────────────┐
│  SqliteBackend                               │
│    └─ Connection (read)                      │ (concurrent with poller write)
│       └─ Reads metrics from SQLite           │
└──────────────────────────────────────────────┘
```

**Benefits:**
- Poller and OPC UA have independent connections (no lock contention)
- SQLite WAL mode enables concurrent readers + single writer
- Metrics persisted automatically on every poll cycle
- OPC UA server can read fresh data without waiting for poller
- Foundation for scaling to 100+ devices and parallel polling

### Implementation Strategy

#### Phase 1: Refactor ChirpstackPoller Signature

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
    storage: SqliteBackend,  // or Arc<dyn StorageBackend>
    // ... other fields
}

impl ChirpstackPoller {
    pub fn new(config: ChirpstackConfig, storage: SqliteBackend) -> Self { ... }
}
```

**Files Modified:**
- `src/chirpstack.rs` — Update struct, constructor, and all storage access paths
- `src/main.rs` — Create `SqliteBackend` instance instead of `Arc<Mutex<Storage>>` for poller

#### Phase 2: Replace In-Memory Write Operations with SQLite Calls

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
let mut transaction = self.storage.begin_transaction()?;
for (device_id, metrics) in collected_metrics {
    transaction.upsert_metric_values(&device_id, metrics)?;
}
transaction.update_gateway_status(
    "last_successful_poll", 
    SystemTime::now()
)?;
transaction.commit()?;
```

**Storage Methods Used (from Story 2-3):**
- `upsert_metric_values(device_id, metrics) -> Result<()>` — reuse batch write logic
- `update_gateway_status(key, value) -> Result<()>` — new method for health tracking

**Files Modified:**
- `src/chirpstack.rs` — Replace all `storage.lock()` + mutate patterns with trait method calls
- `src/storage/sqlite.rs` — Add `update_gateway_status()` method if not already present

#### Phase 3: Update All Tests

**Test Categories:**

1. **Unit tests with InMemoryBackend:**
   - Replace `Arc<Mutex<Storage>>` fixtures with `InMemoryBackend`
   - Assert poller collects and prepares metrics correctly
   - Keep logic assertions unchanged

2. **Integration tests with SqliteBackend:**
   - Create temporary SQLite database for test
   - Run full poll cycle
   - Assert metrics appear in SQLite `metric_values` table
   - Assert `gateway_status` table updated with timestamps

3. **Performance tests:**
   - Poll 100 devices x 4 metrics
   - Measure time for one complete poll cycle
   - Assert <30s (or configured interval)

**Example Test Structure:**
```rust
#[test]
fn test_poller_writes_metrics_to_sqlite() {
    let db = TempDatabase::new();  // RAII guard from Story 2-5b
    let poller = ChirpstackPoller::new(config, db.storage());
    poller.poll_cycle().unwrap();
    
    let metrics = db.storage().list_metrics().unwrap();
    assert_eq!(metrics.len(), expected_count);
    assert_eq!(db.storage().get_gateway_status("last_successful_poll"), Some(...));
}
```

#### Phase 4: Verify Graceful Degradation

**Transient Errors:**
- SQLite BUSY → implement retry with exponential backoff (reuse pattern from Story 2-5b)
- Log at warn level, retry up to 3 times, then skip device if all retries fail

**Fatal Errors:**
- SQLite corruption → log error, propagate `Err` to `main.rs`
- Main.rs catches error and triggers graceful shutdown

**Device-Level Errors (existing pattern):**
- Single device fetch fails → log and continue to next device
- Poll cycle completes even if some devices fail
- `error_count` incremented for tracking

### Files Modified

**Core Implementation:**
- `src/chirpstack.rs` — Major refactoring: constructor, poll_cycle(), all storage operations
- `src/main.rs` — Change poller construction: create `SqliteBackend` instance instead of `Arc<Mutex<Storage>>`

**Testing:**
- `tests/poller_tests.rs` (existing) — Update all fixtures and assertions to use new storage pattern
- `tests/integration_tests.rs` (existing) — Add test for SQLite persistence across poll cycles

**Dependencies:**
- No new dependencies (rusqlite and StorageBackend trait already in place from Epic 2)

**No Changes Needed:**
- `src/storage/mod.rs`, `src/storage/sqlite.rs`, `src/storage/inmemory.rs` — StorageBackend trait already exists
- `src/opc_ua.rs` — Unchanged in this story (refactored later in Story 5-1)
- `config/` — No configuration changes required

---

## Assumptions & Constraints

- **SQLite Already Available:** Epic 2 provides complete `StorageBackend` trait and `SqliteBackend` implementation (dependency satisfied)
- **Batch Write Optimization:** Story 2-3 batch write logic is reusable (no re-implementation of optimization)
- **Backward Compatibility:** Configuration format unchanged from pre-Epic 2 (no breaking changes)
- **Error Handling Pattern:** Exponential backoff for transient SQLite errors can reuse logic from Story 2-5b
- **No Shared Locking:** Assumption that separate SQLite connections don't create bottlenecks (SQLite WAL mode assumption valid for this scale)

---

## Previous Story Intelligence

### From Story 2-5b (Most Recent Completed Story)

**Key Learnings:**
1. **Exponential Backoff Pattern:** Retry logic with 1s → 5s → 30s → 300s cap prevents database exhaustion
2. **Timestamp Precision:** RFC3339 microsecond format (%.6f) required for test assertions (applies to gateway_status timestamps)
3. **Mutex Poisoning Recovery:** Use `into_inner()` on PoisonError if transient panics occur (defensive pattern)
4. **RAII Cleanup:** TempDatabase guard ensures test database cleanup even on assertion failure
5. **Concurrent Task Synchronization:** tokio::join! enforces both tasks complete before assertion (not select!)

**Applied to Story 4-1:**
- Transient SQLite errors: implement similar backoff if BUSY error occurs
- Timestamps in gateway_status: use microsecond precision for consistency
- Test cleanup: reuse TempDatabase RAII pattern from Story 2-5b
- Concurrent tests: if multiple poller instances tested, use join! not select!

### From Story 2-4b (Graceful Degradation)

**Pattern Applied:**
- Transient vs. fatal error distinction (BUSY vs. CORRUPTION)
- Graceful degradation: single device failure doesn't stop poll cycle
- Error logging with full context for debugging

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

## Acceptance Checklist

- [ ] `ChirpstackPoller` constructor accepts `SqliteBackend` (not `Arc<Mutex<Storage>>`)
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

- **Epic 4 Planning:** `_bmad-output/planning-artifacts/epics.md` (lines 475-550)
- **Architecture Decision:** `_bmad-output/planning-artifacts/architecture.md` (Concurrency Model, Data Architecture)
- **Story 2-1a (StorageBackend):** Trait definition, initial implementation
- **Story 2-3 (Batch Write):** Reusable batch optimization logic
- **Story 2-5b (Code Review):** Error handling patterns, timestamp precision, test patterns
- **Story 5-1 (Next Story):** OPC UA refactoring depends on poller completion
- **CLAUDE.md:** Build commands, project conventions

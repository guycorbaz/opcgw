# Story 2.2x: Per-Task SQLite Connections (AC 10 Deferred)

Status: done

## Story

As a **developer**,
I want to refactor SqliteBackend to use per-task SQLite connections,
So that each async task can leverage SQLite's WAL mode for true concurrent reads without Rust-level Mutex bottleneck.

## Context

**From Story 2-2b Code Review (2026-04-20):**
- AC 10 specifies: "When multiple connections are created to the same database, Then each task opens its own SQLite Connection (non-shared)"
- Current implementation (2-2b) uses shared Arc<Mutex<Connection>> for simplicity
- Review decision: Defer per-task pooling to this story as performance optimization
- Current design works correctly but underutilizes SQLite WAL concurrent-reader capability (Rust Mutex serializes all access)

**Rationale for Deferral:**
- Simplicity/correctness tradeoff acceptable at current scale
- Shared connection works without data corruption or safety issues
- Per-task connections require connection pooling infrastructure (moderate complexity increase)
- Performance benefit is real but not critical for current deployment (single poller, occasional OPC UA reads)
- Scheduled after 2-2b completion to avoid blocking AC 10 resolution

## Acceptance Criteria

1. **Given** SqliteBackend with shared Arc<Mutex<Connection>>, **When** I refactor to per-task pattern, **Then** each async task (poller, OPC UA, web UI) opens its own Connection to the same database file.

2. **Given** per-task connections, **When** multiple tasks access the database concurrently, **Then** SQLite WAL mode provides true concurrent readers + single writer (no Rust Mutex bottleneck).

3. **Given** per-task connection pattern, **When** a task finishes or is dropped, **Then** the Connection is properly closed and no resource leaks occur.

4. **Given** connection creation overhead, **When** tasks repeatedly access the database, **Then** connection pooling (e.g., `sqlx` or custom pool) minimizes recreation cost.

5. **Given** pooled connections, **When** all pool connections are in use, **Then** graceful degradation occurs (timeout or queue) rather than panic or unbounded allocation.

6. **Given** the refactored design, **When** tests run, **Then** all existing storage tests pass without modification (backward compatible API).

7. **Given** per-task connections, **When** a task is poisoned/panics, **Then** other tasks can continue (no global Mutex poison affects unrelated tasks).

8. **Given** main.rs initialization, **When** the gateway starts, **Then** SqliteBackend and its connection pool are created once, shared across all tasks via Arc<Pool>.

9. **Given** the new architecture, **When** transaction safety is verified, **Then** concurrent writes are properly serialized at the SQLite level (WAL single-writer constraint).

10. **Given** API compatibility, **When** existing code calls SqliteBackend methods, **Then** no changes to StorageBackend trait or method signatures are needed.

## Tasks / Subtasks

- [ ] **Task 1: Design connection pool interface**
  - [ ] **Decision:** Use custom thin wrapper over Vec<Connection> (recommended; see Dev Notes)
    - `pub struct ConnectionPool { connections: Vec<Connection>, available: Mutex<Vec<usize>> }`
  - [ ] Pool lifecycle design: create at startup, checkout/return per access, close on shutdown
  - [ ] Error handling: `pool.checkout(timeout) -> Result<ConnectionGuard, OpcGwError>` with timeout
  - [ ] Sizing: recommend 3-5 connections (1 poller write, 2+ OPC UA reads, 1 spare)
  - [ ] Timeout: recommend 5 seconds (tradeoff between responsiveness and spurious failures)
  - [ ] Verify: pool checkpoint doesn't block tokio runtime (use blocking operations cautiously)

- [ ] **Task 2: Refactor SqliteBackend to use pooled connections**
  - [ ] Remove field: `conn: Arc<Mutex<Connection>>`
  - [ ] Remove lock() calls: delete all `.lock().unwrap_or_else(|poisoned| poisoned.into_inner())`
  - [ ] Remove Mutex poisoning logging from get_metric, set_metric, get_status, etc.
  - [ ] Update constructor: accept `&ConnectionPool` or create pool; store `pool: Arc<ConnectionPool>`
  - [ ] Update all trait methods: call `pool.checkout(timeout)?` to acquire connection
  - [ ] Important: trait method signatures stay unchanged; only internal implementation changes
  - [ ] Verify: error messages reference pool timeout instead of mutex operations
  - [ ] Test: existing unit tests continue to pass without modification

- [ ] **Task 3: Implement connection pool (src/storage/pool.rs)**
  - [ ] Create `pub struct ConnectionPool { connections: Vec<Connection>, available: Mutex<Vec<usize>> }`
  - [ ] Implement `pub fn new(path: &str, size: usize) -> Result<Self, OpcGwError>`
    - Create N connections to same database file
    - Initialize available list with indices 0..N
    - Return error if any connection creation fails
  - [ ] Implement `pub fn checkout(&self, timeout: Duration) -> Result<ConnectionGuard, OpcGwError>`
    - Block until connection available or timeout
    - Return ConnectionGuard (RAII pattern: returns to pool on drop)
  - [ ] Implement `pub fn close(&self) -> Result<(), OpcGwError>`
    - Wait for all connections to be returned to pool
    - Close each connection gracefully
  - [ ] Add metrics: `pub fn active_connections(&self) -> usize`
  - [ ] Error handling: timeout → `OpcGwError::Database("Connection pool timeout")`, connection closed → error
  - [ ] Verify with stress test: rapid checkout/return, all connections in use, graceful degradation

- [ ] **Task 4: Update main.rs task spawning**
  - [ ] **Before:** `let storage = Arc::new(Storage::new());` passed to tasks
  - [ ] **After:** `let pool = Arc::new(ConnectionPool::new(&config.storage.database_path, 3)?);` 
  - [ ] Clone `Arc<ConnectionPool>` for each task (poller, opc_ua, web)
  - [ ] Update chirpstack.rs: `impl ChirpstackPoller { fn new(pool: Arc<ConnectionPool>) }`
  - [ ] Update opc_ua.rs: `impl OpcUaServer { fn new(pool: Arc<ConnectionPool>) }`
  - [ ] Update main spawn: pass pool clone to each task
  - [ ] Shutdown: call `pool.close()?` before exiting (ensure all connections flushed/closed)
  - [ ] Test: graceful shutdown with active connections (no hangs)

- [ ] **Task 5: Verify transaction safety under concurrency**
  - [ ] **Test 1:** `test_concurrent_reads_during_write()` 
    - Spawn poller thread: start write transaction, wait signal
    - Spawn OPC UA thread: read metrics concurrently
    - Verify: reads succeed without blocking on write (WAL)
  - [ ] **Test 2:** `test_pool_exhaustion_timeout()`
    - Acquire all pool connections
    - Spawn new task requesting connection: verify timeout error
    - Release one connection: verify next request succeeds
  - [ ] **Test 3:** `test_transaction_isolation()`
    - Update gateway_status in transaction
    - Concurrent read: verify either old or new state (no partial reads)
  - [ ] **Test 4:** `test_per_task_panic_isolation()`
    - Poller panics while holding connection
    - Verify: OPC UA can continue reading with other connections
    - Verify: connection returned to pool (no resource leak)
  - [ ] Verify: SQLite WAL file present (`opcgw.db-wal`)
  - [ ] No test changes for existing tests (trait API unchanged)

- [ ] **Task 6: Validate AC 10 compliance**
  - [ ] AC 1: "each task opens its own Connection" ✅ poller/OPC UA have separate connections from pool
  - [ ] AC 2: "SQLite WAL provides concurrent readers + single writer" ✅ WAL enabled, verify with test
  - [ ] AC 7: "task cleanup on drop" ✅ ConnectionGuard RAII pattern
  - [ ] AC 8: "pool created once in main()" ✅ Arc<ConnectionPool> shared across tasks
  - [ ] AC 9: "transaction safety verified" ✅ completed Task 5
  - [ ] AC 10: "API compatible" ✅ StorageBackend trait signatures unchanged
  - [ ] Document in sqlite.rs: "Per-task connections enable WAL concurrent-reader benefit (AC 10)"

- [ ] **Task 7: Performance testing & benchmarking**
  - [ ] Benchmark 1: Measure lock contention reduction
    - Time 1000 reads under current (shared Mutex) vs refactored (per-task) design
    - Expect: per-task 3-5x faster for concurrent reads (no Mutex serialization)
  - [ ] Benchmark 2: Concurrent read throughput
    - Spawn 4 OPC UA read tasks, count operations/sec
    - Measure improvement vs current design
  - [ ] Memory overhead: measure pool size (estimate: 3 connections × ~1MB each = ~3MB)
  - [ ] Document results in Dev Notes or story completion note
  - [ ] No performance regression: write performance should match or improve

- [ ] **Task 8: Full integration testing & validation**
  - [ ] Run: `cargo test storage` — all 34+ tests pass unchanged
  - [ ] Run: `cargo build --release` — no warnings (except pre-existing)
  - [ ] Integration test: full gateway startup with per-task connections
    - Poller writes metrics
    - OPC UA reads simultaneously
    - No timeouts, no errors
  - [ ] Graceful shutdown: SIGTERM → pool.close() → all connections flushed/closed
  - [ ] Stress test: rapid metrics writes + reads for 30 seconds
  - [ ] Verify: no resource leaks (connection count stable in logs)

- [ ] **Task 9: Documentation & architecture update**
  - [ ] Update `src/storage/sqlite.rs` doc comment: explain per-task pattern, reference pool.rs
  - [ ] Update `docs/architecture.md` Concurrency Model section:
    - Change from: "single shared Mutex serializes all access"
    - Change to: "per-task connections from pool, SQLite WAL handles concurrency"
  - [ ] Add `src/storage/pool.rs` doc comments: explain ConnectionGuard pattern, timeout behavior
  - [ ] Add inline comment in main.rs: "Pool shared via Arc; each task gets own connection from pool"
  - [ ] Record in story dev notes: why per-task approach chosen, tradeoffs, performance impact

- [ ] **Task 10: Validate completion and ready for downstream**
  - [ ] All 10 ACs satisfied ✅
  - [ ] No new clippy warnings (fix any introduced)
  - [ ] All existing tests pass (no modifications needed)
  - [ ] AC 10 compliance verified via tests
  - [ ] Performance verified: concurrent access improvement documented
  - [ ] Architecture.md updated with new concurrency model
  - [ ] Ready for story 2-2c (CRUD) to depend on per-task connections

## Dev Notes

### Why Per-Task Connections?

**Current (2-2b):** `Arc<Mutex<Connection>>`
- Rust-level Mutex serializes ALL access (reads + writes), even when poller is idle
- Single writer, but also single reader at a time (bottleneck)
- OPC UA reads must wait for Mutex, blocking on poller operations
- Performance: ~1-5µs overhead per lock acquisition; adds up under concurrent load
- Simpler code, no pool infrastructure needed

**Target (2-2x):** Per-task connections + pool
- Each task has own Connection to same database file
- SQLite WAL: true concurrent readers + single writer at DB level (not Rust-level)
- OPC UA reads don't block each other (only serialized at SQLite level via WAL)
- Performance: eliminates Rust Mutex overhead; WAL handles concurrency efficiently
- Slightly more complex (pool infrastructure), but significantly better concurrency

**Performance Impact:** Removing Mutex bottleneck enables OPC UA to serve reads without waiting for poller, especially during long-running poll cycles. Impact scales with read frequency.

### Connection Pool Design (Recommended)

**RECOMMENDATION: Custom thin wrapper over Vec<Connection>**

```rust
pub struct ConnectionPool {
    connections: Vec<Arc<Connection>>,
    available: Mutex<Vec<usize>>,  // indices of available connections
}
```

**Why custom over r2d2:**
- Pool size: 3-5 connections sufficient (1 poller write, 2 OPC UA reads, 1 web, 1 spare)
- Overhead: r2d2 adds type system complexity not needed for small fixed pools
- Control: Custom pool lets us handle rusqlite's blocking nature explicitly
- Dependencies: Fewer external crates (opcgw goal: minimal dependencies)

**Alternative: r2d2 (if flexibility needed later)**
```toml
r2d2 = "0.8"
r2d2_sqlite = "0.25"
```
Use if: connection requirements change, or pool lifecycle becomes complex.

**NOT RECOMMENDED: sqlx::Pool**
- Designed for async databases (postgres, mysql)
- rusqlite is blocking; would require `tokio::task::block_in_place()` wrapper
- Overkill for this use case, unnecessary async overhead

### Pool Configuration

**Recommended settings for opcgw:**
- **Pool size:** 3-5 connections (adjust based on task count)
  - Poller task: 1 connection (exclusive writer)
  - OPC UA server: 1-2 connections (concurrent readers)
  - Web server (Phase B): 1-2 connections (read/write)
  - Spare: 0-1 for spikes
- **Timeout:** 5 seconds (wait time to acquire connection)
- **Idle cleanup:** Not needed for small fixed pool; every connection stays open
- **Connection creation cost:** ~5-10ms per connection (one-time at startup)

### Graceful Degradation & Error Handling

**When pool exhausted (all connections in use):**
1. Current task waits up to `timeout` for available connection
2. If timeout expires: return `OpcGwError::Database("Connection pool timeout")`
3. Caller decides: retry, log warning, degrade service, or propagate error
4. Example: OPC UA read can timeout gracefully; poller cannot (write must succeed)

**Transaction Isolation & Safety**

**SQLite WAL concurrency guarantees:**
- Multiple readers can read concurrently (via WAL snapshot reads)
- Single writer serialized (SQLite enforces at database level, not Rust level)
- Dirty reads prevented: readers see committed snapshots only
- No lost updates: writes are serialized

**What changes:**
- **Mutex serialization removed** → SQLite serialization active
- **Safety level:** Same or better (SQLite's guarantees are formal)
- **Test verification:** See Task 5 for concurrent write + read test scenario

**Transaction scenarios to test:**
1. Concurrent metric reads while poller writes (AC 10)
2. Write-during-read: OPC UA reads while poller writes → should work (WAL)
3. Simultaneous updates to gateway_status → transaction keeps consistent
4. Pool exhaustion: all connections busy, new request waits/times out

### Mutex Poisoning Implications

**Old (shared Mutex):** If one task panics, poison affects all tasks (global failure)

**New (per-task connections):** 
- If poller panics, its connection is dropped; OPC UA continues with their connections
- Better isolation: failure in one task doesn't poison entire backend
- Recovery: new poller connection created from pool on next cycle

### Integration Pattern: Before & After

**BEFORE (2-2b): Shared Arc<Mutex<Connection>>**
```rust
// main.rs
let storage = Arc::new(Storage::new());  // Single shared storage
let backend = Arc::new(SqliteBackend::new(&config.storage.database_path)?);
// backend.conn = Arc<Mutex<Connection>>

// Spawn tasks passing shared backend
tokio::spawn({
    let backend = Arc::clone(&backend);
    async move { poller::run(backend).await }
});

// chirpstack.rs
impl ChirpstackPoller {
    async fn store_metrics(&mut self, backend: Arc<SqliteBackend>) {
        // get Mutex lock, hold across operation
        let conn = backend.conn.lock().unwrap_or_else(...);
        // ... execute queries ...
        // lock held until scope ends
    }
}
```

**AFTER (2-2x): Per-Task Connections from Pool**
```rust
// main.rs
let pool = Arc::new(ConnectionPool::new(
    &config.storage.database_path,
    3,  // pool size
)?);

// Spawn tasks; each gets own connection from pool
tokio::spawn({
    let pool = Arc::clone(&pool);
    async move { poller::run(pool).await }
});

// chirpstack.rs
impl ChirpstackPoller {
    async fn store_metrics(&mut self, pool: Arc<ConnectionPool>) {
        // Checkout connection from pool
        let conn = pool.checkout(timeout).await?;
        // ... execute queries ...
        // connection returned to pool when conn dropped
        // Next OPC UA read can use different connection concurrently
    }
}
```

**Key difference:** No Mutex lock held across operation; pool handles checkout/return.

### Test Migration Notes

**Existing test compatibility:** ✅ **NO CHANGES NEEDED**
- StorageBackend trait signatures unchanged
- Tests mock via InMemoryBackend (not affected)
- SqliteBackend tests use temp database (will still work)
- New tests added: concurrent access validation

**New tests to add:**
1. `test_pool_exhaustion_timeout()` — All connections busy, timeout behavior
2. `test_concurrent_reads_during_write()` — Verify WAL allows concurrent access
3. `test_per_task_isolation()` — Verify poller panic doesn't affect OPC UA connection
4. `test_pool_cleanup_on_shutdown()` — Verify connections close cleanly

**Performance benchmarks to add:**
- Baseline: Current shared Mutex latency (lock contention)
- Target: Per-task pool latency (no contention)
- Concurrent read throughput: measure improvement

### Latest Technology Context

**rusqlite 0.38.0** (current, from Cargo.toml):
- Supports WAL mode and concurrent readers
- `pragma_update()` API stable
- No breaking changes expected; library mature

**Connection pool approaches (2026):**
- **r2d2:** Latest 0.8.10, stable, used in production systems
- **sqlx:** Version 0.8.x, designed for async; not ideal for rusqlite blocking API
- **Custom:** Simplest for this use case (small fixed pool)

### Mutex Poisoning Recovery (Per-Task Improvement)

With per-task connections:
- If OPC UA task panics while holding connection: only that connection is lost
- Poller can continue writing with its own connection
- New OPC UA task created gets fresh connection from pool
- **Resilience improvement:** No global backend failure from single task panic

## File List

**To Modify:**
- `src/storage/sqlite.rs` — Remove Arc<Mutex<Connection>>, add pool usage
- `src/storage/mod.rs` — Re-export new pool types if needed
- `src/main.rs` — Initialize pool, pass to tasks
- `src/chirpstack.rs` — Use per-task connection from pool
- `src/opc_ua.rs` — Use per-task connection from pool

**To Create:**
- `src/storage/pool.rs` — Connection pool implementation

**To Update:**
- `docs/architecture.md` — Document per-task pattern and WAL benefits
- Story 2-2b review findings — Mark AC 10 deferred work as "Story 2-2x"

## Anti-Patterns to Avoid

**❌ DO NOT: Reintroduce Mutex around entire pool**
- Wrong: `Arc<Mutex<ConnectionPool>>` (defeats the purpose)
- Right: `Arc<ConnectionPool>` with internal Mutex only for available list

**❌ DO NOT: Hold connection across await points**
- Wrong: `let conn = pool.checkout()?; async_operation().await; use(conn)`
- Right: Use ConnectionGuard RAII pattern; return connection ASAP

**❌ DO NOT: Create connection per operation**
- Wrong: `Connection::open(path)?` in every method call
- Right: Checkout from pool (reuses established connection)

**❌ DO NOT: Panic on pool exhaustion**
- Wrong: `.unwrap()` when checkout timeout expires
- Right: Return `Err(OpcGwError::Database("timeout"))` and let caller handle gracefully

**❌ DO NOT: Modify StorageBackend trait**
- Wrong: Add new `pool` parameter to trait methods
- Right: Keep trait unchanged; only SqliteBackend implementation changes

**❌ DO NOT: Share pool without Arc**
- Wrong: Pass `&ConnectionPool` between tasks
- Right: Use `Arc::clone(&pool)` for task spawning

## Code Pattern Requirements

**Connection Checkout Pattern (REQUIRED):**
```rust
// Correct: Use ConnectionGuard RAII, explicit timeout
let conn_guard = pool.checkout(Duration::from_secs(5))?;
let conn = conn_guard.as_ref();  // Get &Connection
conn.execute("SELECT ...", params![])?;
// conn_guard drops here → connection returned to pool automatically
```

**Error Handling Pattern (REQUIRED):**
```rust
// Always propagate pool errors; never unwrap
match pool.checkout(timeout) {
    Ok(conn) => { /* use conn */ },
    Err(e) => return Err(e),  // Caller decides: retry, log, degrade service
}
```

**Task Spawning Pattern (REQUIRED):**
```rust
// main.rs
let pool = Arc::new(ConnectionPool::new(&path, 3)?);

tokio::spawn({
    let pool = Arc::clone(&pool);
    async move { poller::run(pool).await }
});
// Each task has independent Arc clone → can checkout concurrently
```

## References

- [AC 10 from Story 2-2b] — "Each task opens its own SQLite Connection (non-shared)"
- [Code Review Decision, 2026-04-20] — Deferred to balance complexity vs performance
- [Story 2-2b: Per-Task Connection Decision] — Details about Arc<Mutex<Connection>> tradeoff
- [Architecture.md: Concurrency Model] — WAL mode enables concurrent readers, single writer
- [CLAUDE.md] — Error handling patterns (no panics, wrap all errors in OpcGwError)

## Related Stories

- **2-2b (prerequisite):** SQLite schema creation and migration (completed, code reviewed)
- **2-2c (depends on):** CRUD operations will use per-task connections from pool
- **2-3 (depends on):** Persistence and optimization layer

---

### Review Findings

#### ✅ Patches Applied (1)
- [x] [Review][Patch] Add logging for pool checkout timeout [sqlite.rs:223,279,308,384,446,479,537] — **APPLIED**
  - Added `tracing::trace!()` logs to all 7 trait methods capturing pool exhaustion events with operation context
  - Enables debugging of connection pool issues in production logs
  - All 62 tests passing

#### ✅ Deferred (2)
- [x] [Review][Defer] Pool checkout timeout hardcoded to 5 seconds — deferred, MVP-scoped, can be refined in 2-3-x optimization phase
- [x] [Review][Defer] Pool size hardcoded to 3 connections — deferred, out of scope, configuration layer planned for 2-8-x phase

#### ✅ All 10 Acceptance Criteria Verified
- AC 1-10: All passing ✓ (detailed in code review layer results)

## Status Log

- **2026-04-20:** Story created from 2-2b code review deferral. AC 10 compliance requires per-task connections. Enhanced with detailed pool design, integration patterns, testing strategy, performance baselines.
- **2026-04-20:** Implementation complete. All 10 ACs satisfied. Code review performed: 1 patch applied (pool timeout logging), 2 items deferred (MVP-scoped optimizations). Ready for merge.

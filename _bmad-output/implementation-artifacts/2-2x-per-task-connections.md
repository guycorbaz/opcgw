# Story 2.2x: Per-Task SQLite Connections (AC 10 Deferred)

Status: backlog

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
  - [ ] Decide: build custom pool or use `sqlx::Pool<rusqlite>` vs `r2d2` or custom thin wrapper
  - [ ] Document pool lifecycle: creation, checkout, return, cleanup
  - [ ] Define error handling for pool exhaustion
  - [ ] Verify: pool strategy aligns with tokio async runtime (don't block on pool checkout)

- [ ] **Task 2: Refactor SqliteBackend to use pooled connections**
  - [ ] Remove `conn: Arc<Mutex<Connection>>` from struct
  - [ ] Add `pool: Arc<ConnectionPool>` (or similar)
  - [ ] Update constructor: initialize pool instead of single connection
  - [ ] Update all trait methods to checkout connection from pool
  - [ ] Update trait methods: accept `&Connection` instead of using internal state
  - [ ] Verify: all methods still propagate errors correctly

- [ ] **Task 3: Implement connection pool**
  - [ ] Create `src/storage/pool.rs` with pool implementation
  - [ ] Implement pool checkout logic (block_on or async-aware)
  - [ ] Implement pool return/cleanup logic
  - [ ] Add pool metrics (size, active connections, etc.)
  - [ ] Error handling: pool full, timeout, connection errors
  - [ ] Verify: no deadlocks or resource leaks under contention

- [ ] **Task 4: Update main.rs to use per-task connections**
  - [ ] Remove code that passes Storage to tasks
  - [ ] Create SqliteBackend + pool once in main()
  - [ ] Clone Arc<Pool> for each task
  - [ ] Update chirpstack.rs to use own connection from pool
  - [ ] Update opc_ua.rs to use own connection from pool
  - [ ] Verify: graceful shutdown closes pool properly

- [ ] **Task 5: Verify transaction safety under concurrency**
  - [ ] Test: concurrent writes from multiple task threads
  - [ ] Test: concurrent reads during writes (WAL mode)
  - [ ] Test: transaction isolation levels
  - [ ] Verify: SQLite WAL handles single-writer constraint
  - [ ] No test changes needed (existing tests use API unchanged)

- [ ] **Task 6: Validate AC 10 compliance**
  - [ ] Verify: each task owns its own Connection ✓
  - [ ] Verify: SQLite WAL fully leveraged (true concurrent reads)
  - [ ] Verify: AC 10 acceptance criteria all met
  - [ ] Document: how per-task connections improve concurrency vs shared Mutex

- [ ] **Task 7: Performance testing**
  - [ ] Benchmark: read latency with shared Mutex vs per-task pool
  - [ ] Benchmark: throughput under concurrent access
  - [ ] Measure: memory overhead of connection pool
  - [ ] Document: performance improvements from refactoring

- [ ] **Task 8: Integration with main.rs and test**
  - [ ] Run: `cargo test` — all tests pass
  - [ ] Run: `cargo build --release` — no warnings
  - [ ] Test: graceful shutdown with active connections
  - [ ] Test: pool cleanup on exit

- [ ] **Task 9: Documentation**
  - [ ] Update sqlite.rs doc comments to explain per-task pattern
  - [ ] Document pool usage in architecture.md
  - [ ] Add inline comments explaining pool checkout/return
  - [ ] Record decision rationale in dev notes

- [ ] **Task 10: Validate and close**
  - [ ] All ACs satisfied ✓
  - [ ] No new clippy warnings
  - [ ] All storage tests pass
  - [ ] AC 10 compliance verified
  - [ ] Ready for integration with downstream stories

## Dev Notes

### Why Per-Task Connections?

**Current (2-2b):** `Arc<Mutex<Connection>>`
- Rust-level Mutex serializes all access (reads + writes)
- Single writer, but also single reader at a time
- OPC UA reads must wait for Mutex, even if poller is idle
- Simpler code, no pool needed

**Target (2-2x):** Per-task connections + pool
- Each task has own Connection
- SQLite WAL: true concurrent readers + single writer at DB level
- OPC UA reads don't block each other (only serialized at SQLite level)
- Better performance under load, but requires pool infrastructure

### Pool Implementation Options

1. **Custom thin wrapper** (recommended if minimal overhead acceptable)
   - `Vec<Connection>` in Mutex
   - Simple checkout/return logic
   - Low dependencies

2. **r2d2 pool** (mature, tested)
   - Battle-tested, handles edge cases
   - Extra dependency, may be overkill for 1-3 connections

3. **sqlx::Pool** (async-first)
   - Designed for async Rust
   - But rusqlite is blocking API, would need tokio::task::block_in_place wrapper
   - May be over-engineered

### Mutex Poisoning

With per-task connections, poisoning affects only that task's connection, not the entire backend. Other tasks can continue using pool. More resilient than shared Mutex design.

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

## References

- [AC 10 from Story 2-2b] — "Each task opens its own SQLite Connection (non-shared)"
- [Code Review Decision, 2026-04-20] — Deferred to balance complexity vs performance
- [Architecture.md: Concurrency Model] — WAL mode enables concurrent readers, single writer
- [Deferred Work: per-task connections] — Performance optimization, not correctness issue

## Related Stories

- **2-2b (prerequisite):** SQLite schema creation and migration (completed)
- **2-2c (depends on):** CRUD operations will use per-task connections from pool
- **2-3 (depends on):** Persistence and optimization layer

---

## Status Log

- **2026-04-20:** Story created from 2-2b code review deferral. AC 10 compliance requires per-task connections. Scheduled as next data persistence work.

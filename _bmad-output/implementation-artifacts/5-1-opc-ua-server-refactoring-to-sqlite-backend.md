# Story 5-1: OPC UA Server Refactoring to SQLite Backend

**Epic:** 5 (Operational Visibility)  
**Phase:** Phase A  
**Status:** ready-for-dev  
**Created:** 2026-04-24  
**Author:** Claude Code (Automated Story Generation)

---

## Objective

Refactor the OPC UA server (`opc_ua.rs`) to read metrics directly from SQLite via its own `SqliteBackend` connection, eliminating the dependency on the shared in-memory `Arc<Mutex<Storage>>`. This decouples OPC UA reads from poller writes, ensures OPC UA operations complete in <100ms per read, and establishes the final piece of lock-free architecture where all subsystems (poller, OPC UA, command handlers) operate independently via SQLite WAL mode concurrency.

---

## Acceptance Criteria

### AC#1: OPC UA Server Uses Own SQLite Connection
- `OpcUa` struct constructor accepts `Arc<dyn StorageBackend>` (or `Arc<SqliteBackend>` specifically)
- OPC UA server no longer depends on `Arc<Mutex<Storage>>`
- All metric reads use `StorageBackend` trait methods: `get_metric_value()`, `list_devices()`, `list_applications()`
- Variable value reads query SQLite directly on each OPC UA Read operation
- **Verification:** Unit test: verify OPC UA variable read queries storage backend

### AC#2: OPC UA Read Performance Meets NFR1 (<100ms)
- Single OPC UA Read operation (fetch one metric value) completes in <100ms
- Metric reads include: device lookup, metric value fetch, type conversion
- Benchmark test with 300 device configurations shows consistent <100ms reads
- Cold-start read (SQLite page cache miss) completes in <100ms
- **Verification:** Benchmark test: 1000 random metric reads, all <100ms; p95 latency <50ms

### AC#3: Remove All Arc<Mutex<Storage>> from OPC UA Server
- `Arc<Mutex<Storage>>` completely removed from `OpcUa` struct
- No mutex locks held across `.await` points (Clippy verification)
- Task spawning in `main.rs`: OPC UA server receives its own `Arc<SqliteBackend>` instance
- Address space construction still reads config (static, no locks needed)
- **Verification:** `cargo clippy` passes; OpcUa struct shows only `Arc<dyn StorageBackend>` (no Mutex)

### AC#4: OPC UA Address Space Organization Unchanged
- Devices still organized by Application > Device > Metric hierarchy (FR14)
- Browse path remains: `/Objects/Applications/{app_id}/Devices/{dev_id}/Metrics/{metric_name}`
- SCADA clients can browse and discover all configured devices and metrics (FR15)
- **Verification:** Integration test: browse OPC UA address space, verify hierarchy structure

### AC#5: Metric Values with Correct Data Types (FR16)
- Boolean metrics: OPC UA Boolean (type_id = 1)
- Int metrics: OPC UA Int32 (type_id = 6)
- Float metrics: OPC UA Double (type_id = 11)
- String metrics: OPC UA String (type_id = 12)
- Type conversion happens at read time (query SQLite, convert, return)
- **Verification:** Unit test: verify each metric type converts correctly; integration test: FUXA reads correct types

### AC#6: No Lock Contention with Poller or Other Tasks
- Poller writes to its own `Arc<SqliteBackend>` instance (no lock conflicts)
- OPC UA reads from separate `Arc<SqliteBackend>` instance (no lock conflicts)
- SQLite WAL mode: readers and writers don't block each other
- Concurrent metric read (OPC UA) and concurrent metric write (poller) both succeed
- **Verification:** Concurrency test: simultaneous poller writes and OPC UA reads, both complete without timeout

### AC#7: All Tests Updated & Passing
- Existing OPC UA tests refactored: replace `Arc<Mutex<Storage>>` with `Arc<dyn StorageBackend>`
- Test logic assertions unchanged (OPC UA behavior verified, not storage backend behavior)
- New SQLite integration tests: verify OPC UA reads return current metrics from SQLite
- All 352+ existing tests continue to pass (no regressions)
- **Verification:** `cargo test` passes all tests; new OPC UA SQLite tests added and passing

### AC#8: Error Handling & Graceful Degradation
- SQLite read errors (BUSY, IO): log error, return OPC UA status `Bad` (status_code != Good)
- OPC UA client receives valid error response (not panic)
- Missing metric (not in database): return `OPC_STATUS_BAD_NOT_FOUND` or similar
- Device/application lookup failures: return appropriate status code
- **Verification:** Unit test: simulate SQLite read error, verify OPC UA returns Bad status

### AC#9: Code Quality & Production Readiness
- No clippy warnings: `cargo clippy -- -D warnings`
- SPDX license headers on all modified files
- Public methods have doc comments explaining parameters and errors
- No unsafe code blocks (use Arc, not raw pointers)
- Complex patterns (concurrent reads, WAL mode interaction) documented in code comments
- **Verification:** `cargo clippy`, `cargo test`, code review approval

---

## User Story

As a **developer**,  
I want the OPC UA server refactored to read metrics from its own SQLite connection,  
So that OPC UA reads are decoupled from the poller, no shared locks exist between tasks, and OPC UA operations complete in <100ms without contention.

---

## Technical Approach

### Current State (Post-Epic 4, Pre-Story 5-1)

```
┌─ main.rs ────────────────────────────────────────┐
│                                                 │
│  ┌─ ChirpstackPoller                           │
│  │   Arc<SqliteBackend> → metrics to SQLite   │
│  │                                             │
│  ├─ CommandStatusPoller                        │
│  │   Arc<SqliteBackend>                        │
│  │                                             │
│  ├─ CommandTimeoutHandler                      │
│  │   Arc<SqliteBackend>                        │
│  │                                             │
│  └─ OPC UA Server ← STILL HAS PROBLEM         │
│     Arc<Mutex<Storage>> ← lock contention   │
│     └─ blocked by poller writes              │
│                                                 │
└─────────────────────────────────────────────────┘
```

**Current Problem:**
- OPC UA server still uses `Arc<Mutex<Storage>>` for metric reads
- Poller holds mutex lock during metric writes
- OPC UA reads blocked while poller writes (lock contention)
- Inconsistent with other subsystems (poller, commands use SQLite)
- OPC UA Read latency unpredictable due to lock wait times

### Target Architecture (Post-Story 5-1)

```
┌─ main.rs ────────────────────────────────────────┐
│                                                 │
│  ┌─ ChirpstackPoller                           │
│  │   Arc<SqliteBackend> → metrics to SQLite   │
│  │                                             │
│  ├─ CommandStatusPoller                        │
│  │   Arc<SqliteBackend>                        │
│  │                                             │
│  ├─ CommandTimeoutHandler                      │
│  │   Arc<SqliteBackend>                        │
│  │                                             │
│  └─ OPC UA Server ← REFACTORED                │
│     Arc<SqliteBackend> → reads from SQLite   │
│     └─ no lock contention (WAL mode)         │
│                                                 │
└─────────────────────────────────────────────────┘
          ↓ (SQLite WAL mode)
   Readers and Writers Concurrent
```

**Benefits:**
- All subsystems use independent `Arc<SqliteBackend>` instances (fully lock-free)
- OPC UA reads don't block poller writes (SQLite WAL mode)
- OPC UA reads from fresh data in database (consistent, current)
- Read latency predictable and fast (<100ms guarantee)
- Foundation for stale-data detection (Story 5-2)
- Foundation for health metrics (Story 5-3)

### Implementation Strategy

#### Phase 1: Refactor OpcUa Constructor & Field Storage

**Current:**
```rust
pub struct OpcUa {
    storage: Arc<Mutex<Storage>>,
    config: AppConfig,
    // ... other fields
}

impl OpcUa {
    pub async fn new(config: AppConfig, storage: Arc<Mutex<Storage>>) -> Result<Self> { ... }
}
```

**Target:**
```rust
pub struct OpcUa {
    storage: Arc<dyn StorageBackend>,  // independent instance
    config: AppConfig,
    // ... other fields
}

impl OpcUa {
    pub async fn new(config: AppConfig, storage: Arc<dyn StorageBackend>) -> Result<Self> { ... }
}
```

**Files Modified:**
- `src/opc_ua.rs` — Update struct, constructor, all metric read operations

#### Phase 2: Replace Metric Read Operations

**Current pattern (with Mutex):**
```rust
let storage = self.storage.lock().unwrap();
let metric = storage.get_metric_value(device_id, metric_name)?;
drop(storage);  // release lock before conversion
```

**Target pattern (with StorageBackend):**
```rust
let metric = self.storage.get_metric_value(device_id, metric_name)?;
// No lock, no drop needed
```

**Methods to Update:**
- `read_metric_from_storage()` — Core read operation
- `build_address_space()` — Device/metric enumeration (reads config, not changed)
- Variable value callbacks (`on_read`, `on_write` handlers) — Query storage for current value

#### Phase 3: Update Variable Value Resolution

**Pattern:** OPC UA variable `on_read` callback
```rust
// When SCADA client reads an OPC UA variable:
// 1. Extract device_id, metric_name from variable browse path
// 2. Call storage.get_metric_value(device_id, metric_name)
// 3. Convert returned value to OPC UA DataValue with timestamp
// 4. Return to client
// All without locks
```

#### Phase 4: Update Tests & Fixtures

- Refactor OPC UA tests: replace `Arc<Mutex<Storage>>` with test `Arc<dyn StorageBackend>`
- Create test backend that simulates SQLite behavior (return predefined metric values)
- Add integration tests: OPC UA reads return current SQLite metric values
- Benchmark test: measure OPC UA read latency with 300+ device configurations

#### Phase 5: Update main.rs Task Spawning

**Current:**
```rust
let storage = Arc::new(Mutex::new(Storage::new(&config)?));
let opc_ua_storage = Arc::clone(&storage);
let opc_ua_server = OpcUa::new(config.clone(), opc_ua_storage).await?;
```

**Target:**
```rust
let storage = Arc::new(SqliteBackend::new(&config)?);
let opc_ua_storage = Arc::clone(&storage);  // or separate instance
let opc_ua_server = OpcUa::new(config.clone(), opc_ua_storage).await?;
```

---

## Tasks / Subtasks

### Task 1: OpcUa Struct Refactoring
- [ ] Read `src/opc_ua.rs` and understand current struct, constructor, metric read methods
- [ ] Identify all places where `self.storage` is locked (Mutex::lock())
- [ ] Create new `OpcUa` struct with `Arc<dyn StorageBackend>` instead of `Arc<Mutex<Storage>>`
- [ ] Update constructor signature: accept `Arc<dyn StorageBackend>` parameter
- [ ] Replace Mutex::lock() calls with direct StorageBackend method calls
- [ ] Verify no locks held across `.await` points (run `cargo clippy`)

### Task 2: Metric Value Read Operations
- [ ] Identify all methods that read metric values: `read_metric_from_storage()`, variable callbacks
- [ ] Update each method to use `self.storage.get_metric_value()` instead of Mutex lock + Storage method
- [ ] Ensure type conversion (Float/Int/Bool/String) still happens correctly
- [ ] Preserve metric timestamp and quality information in conversion

### Task 3: Address Space Construction
- [ ] Review `build_address_space()` method
- [ ] Identify read-only config accesses (these don't change)
- [ ] Check for any shared storage accesses (should be minimal - only in variable callbacks)
- [ ] Verify address space still organizes by Application > Device > Metric hierarchy

### Task 4: Test Refactoring
- [ ] Identify all existing OPC UA tests using `Arc<Mutex<Storage>>`
- [ ] Create mock `StorageBackend` for testing (or use `InMemoryBackend`)
- [ ] Update test fixtures to pass `Arc<dyn StorageBackend>` to `OpcUa::new()`
- [ ] Verify existing test logic still passes (no behavior change, only storage backend change)
- [ ] Add new SQLite integration test: OPC UA reads return current SQLite values

### Task 5: Benchmark & Performance Validation
- [ ] Create benchmark test: 1000 random metric reads from OPC UA
- [ ] Measure latency distribution (p50, p95, p99)
- [ ] Verify all reads complete in <100ms (AC#2)
- [ ] Test with 300 device configurations for realistic scale
- [ ] Document results in Dev Agent Record

### Task 6: main.rs Integration
- [ ] Update `main.rs` to create `Arc<SqliteBackend>` for OPC UA (if not already done)
- [ ] Pass storage instance to `OpcUa::new()` call
- [ ] Verify all tasks receive independent storage instances (poller, OPC UA, commands)
- [ ] Run full application integration test: poller writes metrics, OPC UA reads them

### Task 7: Error Handling & Edge Cases
- [ ] Handle SQLite read errors: return OPC UA Bad status (not panic)
- [ ] Handle missing metrics: return `BAD_NOT_FOUND` or similar
- [ ] Handle device not found: return appropriate status code
- [ ] Write unit tests for each error scenario

### Task 8: Code Quality & Documentation
- [ ] Run `cargo clippy -- -D warnings` and fix all issues
- [ ] Add doc comments to public methods explaining SQLite backend behavior
- [ ] Add inline comments for complex patterns (WAL mode, concurrent access)
- [ ] Verify SPDX headers on modified files
- [ ] Update File List with all changed files

### Task 9: Full Test Suite Execution
- [ ] Run `cargo test` and verify all 352+ tests pass
- [ ] Check for regressions in existing tests (compare to Epic 4 baseline)
- [ ] Verify new OPC UA SQLite tests pass
- [ ] Run `cargo clippy` to ensure no warnings

### Task 10: Story Completion & Review Preparation
- [ ] Mark all tasks complete with [x]
- [ ] Update File List section with exact file paths
- [ ] Add comprehensive completion notes to Dev Agent Record
- [ ] Change Log: document SQLite migration for OPC UA, removal of Mutex<Storage>
- [ ] Set Status to "review" and save story file

---

## Dev Notes

### Architecture Context
- **Storage Trait:** `src/storage/mod.rs` defines `StorageBackend` trait with methods:
  - `get_metric_value(device_id: &str, metric_name: &str) -> Result<MetricValue>`
  - `list_devices(app_id: &str) -> Result<Vec<Device>>`
  - `list_applications() -> Result<Vec<Application>>`
- **Mutex Pattern (Legacy):** `Arc<Mutex<Storage>>` blocks reads while writes occur
- **SQLite WAL Mode:** Separate readers and writers (in use since Epic 2)
- **Goal:** Replace legacy Mutex pattern with SQLite concurrency

### Key Files
- `src/opc_ua.rs` (~870 lines) — Main file to refactor
- `src/storage/mod.rs` — StorageBackend trait definition
- `src/storage/sqlite.rs` — SqliteBackend implementation
- `src/main.rs` — Task spawning (update storage instance passing)
- `tests/` — OPC UA integration tests to refactor

### Previous Learnings (From Epics 1-4)
- **Lock-free Architecture:** Epic 4 (Story 4-1) proved arc<SqliteBackend> works well for poller
- **Code Review Value:** 3-layer adversarial review (Epic 4, Story 4-3) caught 6 critical issues
- **SQLite Reliability:** Epic 2 established SQLite schema, migrations, transaction patterns
- **Graceful Error Handling:** Epic 4 pattern: log and continue on transient errors

### Testing Strategy (Red-Green-Refactor)
1. **Red:** Write test expecting OPC UA read via StorageBackend (test will fail with current Mutex code)
2. **Green:** Refactor OPC UA to accept `Arc<dyn StorageBackend>`, implement read via trait method
3. **Refactor:** Clean up, add error handling, verify performance

### Performance Requirements
- OPC UA Read <100ms (AC#2) — Must validate with benchmark test
- Address space still organized by App > Device > Metric (AC#4)
- Concurrent reads/writes (AC#6) — SQLite WAL mode handles this

### No-Go Scenarios (HALT Conditions)
- If StorageBackend trait methods insufficient for OPC UA reads → HALT and refine trait
- If Mutex removal breaks existing tests and cannot be fixed → HALT for design review
- If OPC UA read latency >100ms after refactoring → HALT to diagnose (indexing? scanning?)

---

## File List

### New Files
- None expected (refactoring, not adding new files)

### Modified Files
- `src/opc_ua.rs` — Struct refactoring, method updates, error handling
- `src/main.rs` — Storage instance passing to OpcUa (minor change)
- `tests/` — OPC UA tests refactored (multiple test files if applicable)

### Deleted Files
- None

---

## Change Log

- **2026-04-24** — Story created with comprehensive spec; ready for implementation
  - AC#1-9 defined covering architecture, performance, error handling
  - Tasks 1-10 define exact implementation sequence (red-green-refactor)
  - Technical approach documented with before/after architecture diagrams
  - Dev notes include architecture context, previous learnings, testing strategy

---

## Status

**Current:** ready-for-dev  
**Transitions:** ready-for-dev → in-progress → review → done

---

## Dev Agent Record

### Implementation Plan
(To be filled during development)

### Completion Notes
(To be filled on story completion)

### Debug Log
(To be filled as issues arise during development)

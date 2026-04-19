# Story 2.1: StorageBackend Trait and InMemoryBackend

Status: ready-for-dev

## Story

As a **developer**,
I want a thin StorageBackend trait with an in-memory implementation,
So that all modules can be refactored against a clean storage interface and unit tests run without SQLite.

## Acceptance Criteria

1. **Given** the current `Arc<Mutex<Storage>>` with HashMap-based storage, **When** I create a `storage/` module directory structure, **Then** `StorageBackend` trait defines simple get/set methods for metric values, gateway status, and command queue operations.

2. **Given** the StorageBackend trait, **When** I implement `InMemoryBackend` using HashMap, **Then** all existing tests pass using this in-memory implementation (no SQLite dependency for tests).

3. **Given** data types for metrics and commands, **When** implementing the trait, **Then** the following types are defined in `storage/mod.rs`:
   - `MetricType` enum (Float, Int, Bool, String)
   - `MetricValue` struct with timestamp support
   - `DeviceCommand` struct with FIFO fields
   - `ChirpstackStatus` struct for gateway health

4. **Given** the refactored storage layer, **When** running tests, **Then** `cargo test` passes all 26+ tests with zero warnings.

5. **Given** concurrent access patterns, **When** the trait is designed, **Then** FR29 is addressed through trait design (concurrent-ready interface, not blocking get/set methods).

6. **Given** the implementation is complete, **When** I run clippy, **Then** `cargo clippy` produces zero warnings.

## Tasks / Subtasks

- [ ] Task 1: Design StorageBackend trait interface (AC: #1)
  - [ ] Define trait methods for metric CRUD operations
  - [ ] Define trait methods for gateway status access
  - [ ] Define trait methods for command queue operations
  - [ ] Ensure methods are non-blocking (return owned data, not references)
  - [ ] Document trait with examples

- [ ] Task 2: Define data types in storage/mod.rs (AC: #3)
  - [ ] Create MetricType enum: Float, Int, Bool, String
  - [ ] Create MetricValue struct with value, timestamp, device_id, metric_name fields
  - [ ] Create DeviceCommand struct with device_id, payload, command_id, status, created_at fields
  - [ ] Create CommandStatus enum: Pending, Sent, Failed(String)
  - [ ] Create ChirpstackStatus struct with connection_state, last_poll_timestamp, error_count fields
  - [ ] Implement Clone + Debug on all types

- [ ] Task 3: Implement InMemoryBackend (AC: #2)
  - [ ] Create storage/memory.rs with InMemoryBackend struct
  - [ ] Use HashMap<String, MetricValue> for metrics (keyed on device_id:metric_name)
  - [ ] Use Vec<DeviceCommand> for command queue (FIFO order)
  - [ ] Implement StorageBackend trait methods with simple HashMap operations
  - [ ] Add Arc<Mutex<...>> for thread-safe access
  - [ ] Ensure all operations complete without blocking

- [ ] Task 4: Refactor existing code to use StorageBackend trait (AC: #2)
  - [ ] Update chirpstack.rs to accept &dyn StorageBackend instead of Arc<Mutex<Storage>>
  - [ ] Update storage.rs get_device(), set_metric(), etc. to use trait methods
  - [ ] Update all existing tests to create InMemoryBackend and pass it to modules
  - [ ] Verify no existing tests reference Arc<Mutex<Storage>> directly
  - [ ] Ensure backward compatibility with existing test patterns

- [ ] Task 5: Module organization and visibility (AC: #1)
  - [ ] Create `src/storage/` directory with mod.rs
  - [ ] Create `src/storage/memory.rs` for InMemoryBackend
  - [ ] Export StorageBackend trait and InMemoryBackend from storage/mod.rs
  - [ ] Update main.rs to import from storage module
  - [ ] Verify all modules can access types through storage::

- [ ] Task 6: Testing - data types and InMemoryBackend (AC: #4)
  - [ ] Write unit tests for MetricValue serialization/deserialization
  - [ ] Write unit tests for DeviceCommand creation and status transitions
  - [ ] Write integration tests for InMemoryBackend CRUD operations
  - [ ] Verify all 26 existing tests pass with new storage layer
  - [ ] Test concurrent access to InMemoryBackend (simulated concurrent reads/writes)

- [ ] Task 7: Build, test, lint (AC: #4, #5, #6)
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass (26+)
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### Current Storage Architecture

The project currently uses:
```rust
Arc<Mutex<Storage>>
  └── Storage struct with HashMap<String, HashMap<String, f64>>
      └── Device → Metric → Value mapping
```

Limitations:
- No timestamp support for staleness detection
- No persistent storage (all data lost on restart)
- Mutex lock contention between poller and OPC UA server
- No command queue
- No gateway health tracking

### New Architecture: StorageBackend Trait

The new design separates interface from implementation:

```rust
pub trait StorageBackend: Send + Sync {
    // Metric operations
    fn get_metric(&self, device_id: &str, metric_name: &str) -> Option<MetricValue>;
    fn set_metric(&self, device_id: &str, metric_name: &str, value: MetricValue) -> Result<()>;
    fn get_metrics(&self, device_id: &str) -> Option<Vec<MetricValue>>;
    
    // Command queue operations
    fn enqueue_command(&self, cmd: DeviceCommand) -> Result<()>;
    fn get_pending_commands(&self) -> Result<Vec<DeviceCommand>>;
    fn update_command_status(&self, cmd_id: &str, status: CommandStatus) -> Result<()>;
    
    // Gateway status
    fn get_status(&self, key: &str) -> Option<String>;
    fn set_status(&self, key: &str, value: String) -> Result<()>;
}
```

Benefits:
- Decoupled from SQLite (tests use InMemoryBackend)
- Each task owns its connection (no shared locks in Phase A)
- Foundation for Phase B's SQLiteBackend and web UI

### Data Types Design

**MetricValue:**
- Carries timestamp for staleness detection (FR17)
- Supports Float, Int, Bool, String types
- Includes device_id and metric_name for context

```rust
pub struct MetricValue {
    pub device_id: String,
    pub metric_name: String,
    pub value: MetricType,
    pub recorded_at: std::time::SystemTime,
}

pub enum MetricType {
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
}
```

**DeviceCommand:**
- Supports FIFO ordering (created_at timestamp)
- Includes status for delivery tracking (FR13)
- Persistent across restarts (prepared for Story 2.2)

```rust
pub struct DeviceCommand {
    pub command_id: String,
    pub device_id: String,
    pub payload: Vec<u8>,
    pub status: CommandStatus,
    pub created_at: std::time::SystemTime,
}

pub enum CommandStatus {
    Pending,
    Sent,
    Failed(String),
}
```

**ChirpstackStatus:**
- Tracks gateway health for FR18 (OPC UA health metrics)
- Updated by poller each cycle

```rust
pub struct ChirpstackStatus {
    pub connection_state: String, // "connected" | "unavailable"
    pub last_poll_timestamp: Option<std::time::SystemTime>,
    pub error_count: u32,
    pub last_error: Option<String>,
}
```

### InMemoryBackend Implementation

The InMemoryBackend should be a simple, thread-safe wrapper around HashMaps:

```rust
pub struct InMemoryBackend {
    metrics: Arc<Mutex<HashMap<String, MetricValue>>>,
    commands: Arc<Mutex<Vec<DeviceCommand>>>,
    status: Arc<Mutex<HashMap<String, String>>>,
}
```

**Key patterns:**
- Each field wrapped in Arc<Mutex<>> for shared ownership
- Methods take &self (no mut reference required — Mutex provides interior mutability)
- All get operations clone and return owned data (no lifetime issues)
- All set operations take owned data

### Refactoring Existing Code

**chirpstack.rs changes:**
- Change constructor from `fn new(config: &AppConfig, storage: Arc<Mutex<Storage>>, ...)`
  to `fn new(config: &AppConfig, backend: Arc<dyn StorageBackend>, ...)`
- Update polling loop: `backend.set_metric()` instead of `storage.lock().set_metric()`
- Update error handling: propagate from `backend` results

**storage.rs deprecation:**
- Keep existing module for backward compatibility during transition
- All public methods delegate to StorageBackend trait
- Mark old Storage struct as deprecated

**main.rs changes:**
- Create InMemoryBackend instead of Storage
- Pass `Arc::new(backend)` to ChirpstackPoller and OpcUa
- Remove old `Arc::new(Mutex::new(Storage::new()))`

### Testing Strategy

**Unit tests for data types:**
- MetricValue: serialization, timestamp handling, clone
- DeviceCommand: status transitions, FIFO ordering property
- ChirpstackStatus: updates and reads

**Integration tests for InMemoryBackend:**
- Basic get/set for metrics
- Command queue FIFO ordering
- Status key-value operations
- Concurrent reads/writes (spawn tasks, verify no data corruption)

**Regression tests:**
- Run all 26 existing tests against InMemoryBackend
- Verify chirpstack.rs tests still pass
- Verify storage.rs tests still pass (if any)
- Verify config validation tests still pass

### Previous Story Intelligence (Story 1.5)

**Configuration Validation & Clean Startup** established:
- Error handling pattern: `OpcGwError::Configuration` with detailed messages
- Validation approach: collect all errors, return as structured message
- Startup confirmation logging with key parameters

**Learnings to apply in this story:**
1. Error propagation: Use `Result<T, OpcGwError>` throughout (add OpcGwError::Storage variant if needed)
2. Testing: Use test config at `tests/config/config.toml` (valid URL format required)
3. Logging: Use tracing macros with structured fields (already migrated in Story 1.2)
4. Startup: Log storage initialization at info level with backend type

### Architecture Compliance (from Planning)

**From Architecture Document:**
- Storage trait design: "Thin `StorageBackend` trait with simple get/set methods" ✓
- Concurrency model: "Each async task owns its own SQLite Connection — no shared lock" (prepared for Phase A) ✓
- Data freshness: "Every metric value must carry a timestamp" (MetricValue.recorded_at) ✓
- Error handling: "Single `OpcGwError` enum, add variants as needed" (add Storage if needed) ✓

**From Requirements Inventory (Epic 2):**
- FR25: Persist last-known metric values (prepared for Story 2.2)
- FR26: Restore values from persistent storage (prepared for Story 2.4)
- FR27: Store historical metric data with timestamps (MetricValue carries timestamp)
- FR28: Prune historical data (prepared for Story 2.5)
- FR29: Concurrent read/write without blocking (trait-level concurrency support)
- FR30: Batch metric writes (StorageBackend supports batch operations)

### Git Intelligence

**From Story 1.5 commits:**
- Error handling: All errors return Result<T, OpcGwError>
- Tests: Use get_config() helper from tests/config/config.toml
- Logging: Use tracing structured logging throughout
- File organization: New modules go in src/ directory

**Best practices established:**
- Doc comments on all public items
- SPDX license headers on all files
- Zero unwrap()/panic() in production paths
- Comprehensive test coverage (all 26 tests passing)

### What NOT to Do

- DO NOT create `[storage]` config section yet — that's Story 2.2
- DO NOT implement SQLiteBackend — that's Story 2.2
- DO NOT add migration logic — that's Story 2.2
- DO NOT implement batch operations beyond set_metric — keep interface simple
- DO NOT cache metric values in application code — trait interface is canonical source
- DO NOT add async/await to trait methods — keep synchronous for simplicity
- DO NOT break existing chirpstack.rs or opc_ua.rs logic — refactoring is interface-only

### Development Approach

**Phase 1: Define types and trait**
1. Create `src/storage/mod.rs` with trait definition
2. Define MetricValue, DeviceCommand, ChirpstackStatus, etc.
3. Document trait with examples

**Phase 2: Implement InMemoryBackend**
1. Create `src/storage/memory.rs`
2. Implement StorageBackend for InMemoryBackend
3. Add unit tests for basic operations

**Phase 3: Refactor existing code**
1. Update chirpstack.rs to use trait
2. Update opc_ua.rs to use trait
3. Update test fixtures
4. Verify all 26 tests pass

**Phase 4: Validate and polish**
1. Run full test suite
2. Clippy checks
3. Code review of trait design

### Testing Standards

- Unit tests for all data types
- Integration tests for InMemoryBackend
- Regression tests (all 26 existing tests must pass)
- Concurrent access tests (spawn 4+ tasks, verify no data corruption)
- `cargo test` passes with zero warnings
- `cargo clippy` produces zero warnings

### References

- [Epic 2: Data Persistence](../planning-artifacts/epics.md#epic-2-data-persistence)
- [Architecture: Storage Backend](../planning-artifacts/architecture.md#data-architecture)
- [Architecture: Concurrency Model](../planning-artifacts/architecture.md#concurrency-model)
- [Story 1.5: Configuration Validation](./1-5-configuration-validation-and-clean-startup.md)
- [Rust Error Handling](https://doc.rust-lang.org/std/result/)

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List

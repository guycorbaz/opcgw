# Story 4-2: Support All ChirpStack Metric Types

**Epic:** 4 (Scalable Data Collection)  
**Phase:** Phase A  
**Status:** ready-for-dev  
**Created:** 2026-04-23  
**Author:** BMad Story Context Engine  

---

## Objective

Extend the ChirpStack poller to correctly handle all metric types returned by the ChirpStack gRPC API: Gauge, Counter, Absolute, and Unknown. Each type will be mapped to the appropriate OPC UA data type (Float, Int, String) and stored in the correct format for downstream consumers (OPC UA server, web UI, historical queries).

---

## Story

As an **operator**,  
I want all ChirpStack metric types handled correctly,  
So that every sensor reports accurate data regardless of its metric format, and the gateway works with diverse device types without silently skipping or misinterpreting measurements.

---

## Acceptance Criteria

### AC#1: Gauge Metric Support
- When the poller receives a metric with `kind == GAUGE` from ChirpStack, store as `MetricType::Float`
- The value is the raw floating-point measurement from the dataset
- OPC UA reads expose the value as `OPC_FLOAT` data type

### AC#2: Counter Metric Support
- When the poller receives a metric with `kind == COUNTER` from ChirpStack
- It stores the metric value as `MetricType::Int`
- **Monotonic behavior:** Accept updates where `new >= previous` (includes equal values, which are idempotent); reject with warning if `new < previous` (counter reset detected)
- OPC UA reads expose the value as `OPC_INT32` data type
- When counter reset detected: log warning with `device_id`, `metric_name`, `previous_value`, `new_value` fields, but preserve the previous stored value (don't corrupt history)
- Verification: Unit tests validate counter increment acceptance, reset rejection, and idempotent equal values

### AC#3: Absolute Metric Support
- When the poller receives a metric with `kind == ABSOLUTE` from ChirpStack, store as `MetricType::Float`
- The value is the raw measurement from the dataset (semantically: resetting counter, e.g., hourly energy usage)
- OPC UA reads expose the value as `OPC_FLOAT` data type

### AC#4: Unknown Metric Type Handling
- When the poller receives a metric with an unrecognized or unmapped metric kind, skip it gracefully (don't store)
- Log warning with structured fields: `device_id`, `metric_name`, `metric_kind`
- The poll cycle continues (no crash) — single metric failure doesn't stop the poller or affect other device metrics

### AC#5: Extract and Match Metric Kind from gRPC
- Poller reads the `kind: MetricKind` field from each `chirpstack_api::common::Metric` struct (from `chirpstack_api::common` module)
- **Protobuf enum integer values** (from `proto/common/common.proto`):
  - `0` = COUNTER (monotonically increasing, never resets)
  - `1` = ABSOLUTE (resets periodically, e.g., energy used this hour)
  - `2` = GAUGE (instantaneous measurement, e.g., temperature)
  - Other = Unknown (unmapped or new values)
- Extraction: `metric.kind as i32` converts protobuf enum to integer for matching
- Centralize in single function `classify_metric_kind(kind: i32) -> ChirpStackMetricKind` for testability
- Verification: Unit test validates 0, 1, 2, and unmapped values classify correctly

### AC#6: Type Conversion Priority (Kind-First Override)
- **Type selection priority:** ChirpStack kind mapping always takes precedence; config type is fallback only
  - **Known kind (GAUGE, COUNTER, ABSOLUTE):** Use kind-driven mapping (Float, Int, Float) — ignore config type
  - **Unknown kind + config type exists:** Use config type as fallback (graceful degradation)
  - **Unknown kind + no config type:** Skip metric with warning (AC#4)
- **Logging:** When kind-driven mapping is applied, log at debug level: `kind_driven_conversion=true, metric_kind=?kind`
- This resolves conflicts: if config says "bool" but ChirpStack kind is "gauge", kind always wins (→ Float storage)

### AC#7: OPC UA Data Type Mapping (No Changes to opc_ua.rs)
- Metric type → OPC UA data type mapping already implemented correctly in `opc_ua.rs`:
  - `MetricType::Float` → `DataType::Float` ✓
  - `MetricType::Int` → `DataType::Int32` ✓
  - `MetricType::Bool` → `DataType::Boolean` ✓
  - `MetricType::String` → `DataType::String` ✓
- No changes needed to `opc_ua.rs`; AC#7 verified by existing tests

### AC#8: Test Isolation (No Real ChirpStack Dependencies)
- All tests use mock `Metric` structs with `kind` field set directly (no gRPC calls)
- Each test creates its own `InMemoryBackend` or temporary SQLite storage (RAII cleanup)
- Unit + integration tests run without network, real ChirpStack, or external dependencies

### AC#9: Structured Logging with Context
- All warn/error logs include: `device_id`, `metric_name`, `metric_kind`, and error reason
- No sensitive data in logs (API tokens, payloads never logged)
- Log levels: `warn!` for recoverable (unknown kind, counter reset), `error!` for fatal (storage failures)

### AC#10: FR4 Requirement Closure
- Directly satisfies FR4: "System can handle all ChirpStack metric types (Gauge, Counter, Absolute, Unknown)"
- All four types handled without crash; Unknown types logged and skipped gracefully

---

## Test Requirements Summary

**9 Tests Required** (verify all acceptance criteria):

| Category | Test | Purpose | AC# |
|----------|------|---------|-----|
| **Happy Path (3)** | `test_metric_kind_gauge_to_float` | GAUGE → Float | #1 |
| | `test_metric_kind_counter_to_int` | COUNTER → Int | #2 |
| | `test_metric_kind_absolute_to_float` | ABSOLUTE → Float | #3 |
| **Edge Cases (3)** | `test_metric_kind_unknown_graceful_skip` | Unknown → skip with warning | #4 |
| | `test_counter_monotonic_increase_accepted` | new > previous → accept | #2 |
| | `test_counter_reset_rejected` | new < previous → reject with warning | #2 |
| **Classification (2)** | `test_classify_metric_kind_enum_values` | Protobuf enum 0,1,2 → types | #5 |
| | `test_kind_overrides_config_type` | Kind precedence over config | #6 |
| **Integration (1)** | `test_counter_monotonic_across_poll_cycles` | Monotonic check persists in SQLite | #2 |

**All tests:** Use mocks, no real ChirpStack connection

---

## Technical Approach

### ChirpStack Metric Kind Enum

From `chirpstack_api::common::MetricKind` protobuf enum:
```
COUNTER = 0     // Monotonically increasing counter
ABSOLUTE = 1    // Resetting counter (e.g., energy used this hour)
GAUGE = 2       // Gauge measurement (e.g., temperature, voltage)
(Unknown/Other) // Any unmapped value
```

The `Metric` struct from `chirpstack_api::common::Metric` contains:
```rust
pub struct Metric {
    pub name: String,               // Metric name (e.g., "temperature")
    pub kind: MetricKind,           // Kind discriminator
    pub unit: String,               // Unit string (e.g., "°C")
    pub datasets: Vec<Dataset>,     // Time-series data
    // ... other fields
}

pub struct Dataset {
    pub label: String,
    pub data: Vec<f64>,             // Floating-point values
}
```

### Implementation Strategy

#### Phase 1: Add Metric Kind Classification Function

**Location:** `src/chirpstack.rs`

**Import:** `MetricKind` is from `chirpstack_api::common` (already imported in chirpstack.rs via `use chirpstack_api::common::Metric`)

Create a new function that safely classifies `MetricKind`:

```rust
/// Classifies a ChirpStack metric kind (protobuf enum as i32) into a local enum.
/// Protobuf enum values from proto/common/common.proto:
///   0 = COUNTER (monotonically increasing, never resets)
///   1 = ABSOLUTE (resets periodically, e.g., hourly usage)
///   2 = GAUGE (instantaneous measurement, e.g., temperature)
///   Other = Unknown
fn classify_metric_kind(kind: i32) -> ChirpStackMetricKind {
    match kind {
        0 => ChirpStackMetricKind::Counter,
        1 => ChirpStackMetricKind::Absolute,
        2 => ChirpStackMetricKind::Gauge,
        _ => ChirpStackMetricKind::Unknown,
    }
}

/// Local enum wrapper for metric kinds (easier to test/match than protobuf).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChirpStackMetricKind {
    Counter,    // Monotonically increasing
    Absolute,   // Resets periodically
    Gauge,      // Instantaneous
    Unknown,    // Unmapped value
}
```

**Usage in metric processing:**
```rust
let kind = classify_metric_kind(metric.kind as i32);  // Extract i32 from protobuf enum
```

**Files Modified:**
- `src/chirpstack.rs` — Add `ChirpStackMetricKind` enum and `classify_metric_kind()` function

#### Phase 2: Extend `prepare_metric_for_batch()` to Consider Metric Kind

**Current location:** `src/chirpstack.rs` line ~805

**Enhanced Implementation Pattern:**

```rust
fn prepare_metric_for_batch(&self, device_id: &str, metric: &Metric) -> Option<BatchMetricWrite> {
    // 1. Classify metric kind early
    let kind = classify_metric_kind(metric.kind as i32);
    
    // 2. Determine target MetricType (kind-first priority)
    let target_type = match kind {
        ChirpStackMetricKind::Gauge => MetricType::Float,
        ChirpStackMetricKind::Counter => MetricType::Int,
        ChirpStackMetricKind::Absolute => MetricType::Float,
        ChirpStackMetricKind::Unknown => {
            match self.config.get_metric_type(&metric_name, device_id) {
                Some(cfg_type) => cfg_type,  // Fallback to config
                None => {
                    warn!(metric_name = %metric_name, device_id = %device_id, 
                          metric_kind = ?kind, "Skipping metric: unknown kind and no config");
                    return None;
                }
            }
        }
    };
    
    // 3. For Counter type: atomic monotonic check + write within transaction
    if target_type == MetricType::Int {
        // ATOMIC TRANSACTION: Check previous value and write within same transaction
        let mut tx = self.storage.begin_transaction()?;  // Begin transaction
        
        // Check: Load previous value
        if let Ok(Some(prev_metric)) = tx.get_metric_value(device_id, &metric_name) {
            let prev_int = prev_metric.value.parse::<i64>().unwrap_or(0);
            let new_int = new_value.parse::<i64>().unwrap_or(0);
            if new_int < prev_int {
                warn!(device_id = %device_id, metric_name = %metric_name, 
                      prev_value = prev_int, new_value = new_int, 
                      "Counter reset detected; skipping update");
                return None;  // Transaction auto-rolled back on drop
            }
        }
        // If check passes, write happens later within same transaction
        // (Do NOT commit here; return BatchMetricWrite for inclusion in poll-cycle transaction)
    }
    
    // 4. Validate datasets and create BatchMetricWrite
    if metric.datasets.is_empty() || metric.datasets[0].data.is_empty() {
        warn!(metric_name = %metric_name, device_id = %device_id, "Metric has no data; skipping");
        return None;
    }
    
    let value = metric.datasets[0].data[0];
    debug!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, 
           kind_driven_conversion = true, "Metric prepared for batch write");
    
    Some(BatchMetricWrite {
        device_id: device_id.to_string(),
        metric_name,
        value: value.to_string(),
        data_type: target_type,
        timestamp: SystemTime::now(),
    })
}
```

**Critical Implementation Notes:**
- **Monotonic check atomicity:** Perform `get_metric_value()` and write within same transaction to prevent race conditions between check and write
- **Transaction scope:** Transaction begins before check, commits after all poll-cycle metrics written (in `batch_write_metrics()`)
- **Idempotency:** Equal values (`new == previous`) are accepted; only `new < previous` is rejected

**Files Modified:**
- `src/chirpstack.rs` — Update `prepare_metric_for_batch()` function to use kind-driven type conversion and monotonic counter checking

#### Phase 3: Optimize Counter Monotonic Check Performance

**Location:** `src/chirpstack.rs` in `prepare_metric_for_batch()`

With 100 devices × 4 metrics = 400 counter lookups per poll cycle, use prepared statement caching:

```rust
// Instead of: conn.prepare("SELECT ... WHERE device_id = ?1 ...")?
// Use: conn.prepare_cached("SELECT ... WHERE device_id = ?1 ...")?
```

The `prepare_cached()` method from rusqlite avoids recompiling SQL statements. Critical for 100+ device scale.

**Files Modified:**
- `src/chirpstack.rs` — Use `prepare_cached()` for counter monotonic value lookups

#### Phase 4: Update Logging to Include Metric Kind

Add `metric_kind` structured field to all metric-related warn/error logs:

```rust
// Unknown kind: log classification result
warn!(metric_name = %metric_name, device_id = %device_id, metric_kind = ?kind, 
      "Skipping metric: unknown kind and no config");

// Counter reset: log values for debugging
warn!(device_id = %device_id, metric_name = %metric_name, metric_kind = "counter",
      prev_value = prev_int, new_value = new_int, 
      "Counter reset detected; skipping update");

// Kind-driven conversion: log override
debug!(metric_kind = ?kind, kind_driven_conversion = true, 
       "Metric prepared for batch write");
```

**Files Modified:**
- `src/chirpstack.rs` — Add `metric_kind` field to metric processing logs

#### Phase 5: Add Unit Tests

**Location:** `tests/` directory or inline in `src/chirpstack.rs` (`#[cfg(test)]` module)

**9 Tests Required** (mapped to Test Requirements Summary section above):

1. **Happy Path (3 tests):**
   - `test_metric_kind_gauge_to_float` — GAUGE (kind=2) → MetricType::Float
   - `test_metric_kind_counter_to_int` — COUNTER (kind=0) → MetricType::Int
   - `test_metric_kind_absolute_to_float` — ABSOLUTE (kind=1) → MetricType::Float

2. **Counter Monotonic Logic (3 tests):**
   - `test_counter_monotonic_increase_accepted` — new > previous → accept (normal increment)
   - `test_counter_reset_rejected` — new < previous → reject with warning (counter reset)
   - `test_counter_equal_value_accepted` — new == previous → accept (idempotent)

3. **Unknown Kind Handling (1 test):**
   - `test_metric_kind_unknown_graceful_skip` — kind=99 (unknown) → skip with warn log, poll continues

4. **Classification Function (1 test):**
   - `test_classify_metric_kind_enum_values` — Function correctly maps 0→Counter, 1→Absolute, 2→Gauge, other→Unknown

5. **Integration Test (1 test):**
   - `test_counter_monotonic_across_poll_cycles` — Monotonic check works when metrics persisted in SQLite and reloaded

**Mock metric helper (required for all tests):**
```rust
fn mock_metric(name: &str, kind: i32, value: f64) -> chirpstack_api::common::Metric {
    use chirpstack_api::common::{Metric, Dataset};
    
    Metric {
        name: name.to_string(),
        kind,  // Raw i32: 0=COUNTER, 1=ABSOLUTE, 2=GAUGE, other=Unknown
        unit: "".to_string(),
        datasets: vec![Dataset {
            label: "test_data".to_string(),
            data: vec![value],
        }],
        // Fill in any other required fields with defaults
        ..Default::default()
    }
}
```

**Usage in tests:**
```rust
let gauge_metric = mock_metric("temperature", 2, 25.5);  // kind=2 (GAUGE)
let counter_metric = mock_metric("packet_count", 0, 100.0);  // kind=0 (COUNTER)
let unknown_metric = mock_metric("unknown_type", 99, 50.0);  // kind=99 (Unknown)
```

**Test structure:**
- Use `InMemoryBackend` for unit tests (fast, no I/O)
- Use `SqliteBackend` with temporary database for integration tests (counter monotonic check across poll cycles)
- All tests use mocks, not real ChirpStack connection

**Files Created/Modified:**
- `tests/metric_types_test.rs` (new) — Dedicated test module for metric type conversions
- `src/chirpstack.rs` — May include inline unit tests if small

#### Phase 6: Documentation & Code Quality

Add comprehensive doc comment to `classify_metric_kind()` function:

```rust
/// Classifies a ChirpStack metric kind from protobuf enum integer value.
/// 
/// ChirpStack defines four metric kinds in common.proto:
/// - 0 = COUNTER: Monotonically increasing, never resets. Use MetricType::Int with monotonic check.
/// - 1 = ABSOLUTE: Resets periodically (e.g., hourly energy). Use MetricType::Float.
/// - 2 = GAUGE: Instantaneous measurement (e.g., temperature). Use MetricType::Float.
/// - Other: Unknown/unmapped kind. Gracefully skip with warning; fallback to config type if available.
///
/// This classification function enables testable, type-safe kind matching.
```

**Finalize with quality checks:**
- `cargo clippy -- -D warnings` — no warnings
- `cargo test` — all 9 tests passing
- SPDX headers on all new code
- No unwrap() in production paths

**Files Modified:**
- `src/chirpstack.rs` — Add comprehensive doc comments, ensure code quality

### Counter Monotonic Tracking Details

**Why:** Some applications (e.g., energy meters) send counters that reset periodically. If a new value is less than the previous value, it indicates a reset and the update should be skipped to avoid corrupting historical trends.

**Implementation:**
1. On each metric update for `MetricType::Int`:
   - Query SQLite `metric_values` table for the previous stored value
   - Compare: `new_value < previous_value` → log warning and skip update
   - Compare: `new_value >= previous_value` → normal update (even if equal, allowed for redundancy)
   - If no previous value exists, accept the new value (first write)

2. Logging on counter reset:
   - Log level: `warn!`
   - Fields: `device_id`, `metric_name`, `previous_value`, `new_value`
   - Message: "Counter reset detected; skipping update"

3. Error handling:
   - If SQLite query fails, log at error level and skip the update (fail safe)
   - Don't crash the poll cycle

**Testing the monotonic check:**
- Unit test: Set up two metric values, verify second smaller value is rejected
- Integration test: Run two poll cycles with values [100, 50], assert only 100 is in history

---

## Assumptions & Constraints

- **Metric Kind Always Present:** All `Metric` structs from ChirpStack have `kind` field populated
- **StorageBackend trait has `get_metric_value()`:** If not present, add it (simple SELECT, returns `Option<MetricValue>`)
- **Atomic Transactions Required:** `SqliteBackend.begin_transaction()` must support atomic read-check-write within same transaction (critical for counter monotonic safety)
- **No New Kinds Mid-Phase:** ChirpStack API frozen at 4.15.0; fallback handles unmapped values gracefully
- **Counter Reset Preservation:** Skip resets (don't update history), but preserve previous value in database (no data corruption)
- **`prepare_cached()` Available:** Rust rusqlite 0.38.0 has `prepare_cached()` for statement reuse (already in Cargo.toml)

---

## Previous Story Intelligence

### From Story 4-1 (Poller Refactoring)

**Reuse these patterns:**
1. `prepare_metric_for_batch()` hook — extend this function for kind-driven type conversion
2. Mock metric structures — similar pattern for testing metric kinds
3. Structured logging with `device_id` + `metric_name` — add `metric_kind` field
4. SqliteBackend trait methods — call `get_metric_value()` for counter check (may need to add to trait)

### From Story 3-2 (Command Parameter Validation)

**Similar validation pattern to reuse:**
- Story 3-2 validates command parameters (type, range, f_port) before forwarding to ChirpStack
- Story 4-2 validates metric kinds before storing
- Both use structured logging with context fields
- Both gracefully skip invalid inputs with warnings
- **Recommendation:** Review Story 3-2's validation error handling pattern; apply same approach to metric kind classification

### From Story 2-3 (Batch Write Optimization)

**Key Learnings Applied:**
1. **Transactional writes:** Story 4-2 monotonic check should fit within same transaction as metric write (atomic all-or-nothing)
2. **No holding locks across await:** All monotonic checks happen before async sleep (inside `prepare_metric_for_batch()` which is sync)

### From Stories 2-4b / 2-5b (Error Handling & Degradation)

**Key Learnings Applied:**
1. **Single device failure doesn't stop poll:** Unknown metric kind follows same pattern (warn + skip, continue)
2. **Fatal vs. recoverable errors:** Unknown kind is recoverable (graceful skip), not fatal
3. **Error context in all paths:** Counter reset logging follows structured fields pattern

---

## Git Context & Patterns

**Recent commits related to metric handling:**
- (From Story 4-1) Established `prepare_metric_for_batch()` pattern
- (From Story 2-3) Batch write transactions and atomicity
- (From Stories 1-2/1-3) Tracing structured logging with key-value fields

**Code patterns to follow:**
- Metric validation: check datasets not empty, check data not empty, match on config type
- Error propagation: use `?` operator, provide context via `.map_err(|e| OpcGwError::...)`
- Logging: use `tracing::{debug, warn, error}` macros with structured fields
- Testing: mock fixtures, InMemoryBackend for unit tests, SqliteBackend for integration

---

## Configuration Reference

**No new configuration required for Story 4-2.** Uses existing sections:

```toml
[chirpstack]
server_address = "http://chirpstack:8080"
api_token = "..."

[[application]]
name = "Example App"
id = 1
[[application.device]]
name = "Sensor 1"
deveui = "..."
[[application.device.metrics]]
name = "temperature"
# type is optional; ChirpStack kind overrides if present
type = "float"  # optional, kind-driven conversion takes precedence
```

**Behavior:**
- If metric kind is GAUGE/COUNTER/ABSOLUTE → kind-driven type is used (ignores config type)
- If metric kind is UNKNOWN and config type exists → config type is used
- If metric kind is UNKNOWN and no config type → metric skipped with warning

---

## Architecture Alignment

### Storage Backend & Concurrency

**StorageBackend trait requirements:**
- `upsert_metric_values()` — Already exists from Epic 2
- `get_metric_value(device_id, metric_name) -> Result<Option<MetricValue>>` — Must be callable for monotonic check
  - If not present in trait, add it
  - Implementation: Single SQL SELECT on metric_values table
  - Used by: Counter monotonic check in `prepare_metric_for_batch()`

**Concurrency model (unchanged from Story 4-1):**
- Poller task owns `SqliteBackend` with write connection
- Calls `get_metric_value()` and `upsert_metric_values()` **within same transaction** (atomic: no race between check and write)
- No new shared state (CancellationToken + AppConfig unchanged)

### Error Handling

- `OpcGwError::Storage(String)` — metric read/write, monotonic check failures
- `OpcGwError::ChirpStack(String)` — ChirpStack gRPC errors
- **Graceful degradation:** Non-fatal errors (unknown kind, counter reset) logged as warnings, poll continues
- **Fatal errors** (SQLite corruption, binding failure) propagate to main for shutdown

---

## Files to Modify

| File | Change | Lines |
|------|--------|-------|
| `src/chirpstack.rs` | (1) Add `ChirpStackMetricKind` enum; (2) Add `classify_metric_kind()` fn; (3) Extend `prepare_metric_for_batch()` with kind-driven type + monotonic check; (4) Add `metric_kind` to logs | ~150 |
| `src/storage/mod.rs` | Add `get_metric_value()` to `StorageBackend` trait (if missing) | ~5 |
| `src/storage/sqlite.rs` | Implement `get_metric_value()` (if missing) | ~10 |
| `tests/metric_types_test.rs` | Create new: 9 unit/integration tests with mocks | ~300 |

**No changes needed:** `src/opc_ua.rs`, `src/config.rs`, `src/main.rs`, `Cargo.toml` — OPC UA mapping already correct, no new dependencies, no config changes

---

## Acceptance Checklist

- [ ] `ChirpStackMetricKind` enum created with Counter, Absolute, Gauge, Unknown variants
- [ ] `classify_metric_kind()` function implemented and tested
- [ ] `prepare_metric_for_batch()` extended to use kind-driven type conversion
- [ ] Counter monotonic check implemented: new < previous → skip with warning
- [ ] Counter monotonic check retrieves previous value from SQLite within transaction
- [ ] Kind-driven type override logs at debug level
- [ ] Unknown kind with no config logs warning and skips (graceful degradation)
- [ ] All metric processing logs include `metric_kind` structured field
- [ ] 9 unit + integration tests created and passing
- [ ] Mock metric helpers created for test isolation (no real ChirpStack needed)
- [ ] `cargo test` passes all new tests
- [ ] `cargo clippy` produces no warnings
- [ ] No unsafe code
- [ ] SPDX headers present on all new files
- [ ] FR4 is satisfied: All four metric types handled without crash
- [ ] Code review approval from previous team

---

## References

- **Epic 4 Requirements:** `_bmad-output/planning-artifacts/epics.md` (Story 4.2, lines 497-514)
- **Architecture — Concurrency Model:** `_bmad-output/planning-artifacts/architecture.md` (SQLite connections per task, WAL mode)
- **Architecture — Error Handling Patterns:** `_bmad-output/planning-artifacts/architecture.md` (OpcGwError variants, non-fatal error handling)
- **Story 4-1 (Previous):** Poller refactoring to SQLite backend; established `prepare_metric_for_batch()` pattern
- **Story 2-3 (Batch Write):** Batch transaction pattern; reused for atomic monotonic check + write
- **Story 1-2 (Logging):** Tracing structured logging pattern with key-value fields
- **ChirpStack API:** `proto/common/common.proto` (MetricKind enum definition)
- **CLAUDE.md:** Build commands, project conventions, no unwrap() rule

---

## Critical Implementation Details

### 🔥 Key Do's and Don'ts

**DO:**
- Use `prepare_cached()` for counter monotonic value lookups (400 lookups/cycle at 100 devices)
- Perform monotonic check + write **within same SQLite transaction** (atomic, no race)
- Log `metric_kind` field in all metric processing logs for debuggability
- Accept equal counter values (`new == previous`) as idempotent updates
- Gracefully skip unknown kinds; poll continues (non-fatal error)

**DON'T:**
- Perform monotonic check and write in separate operations (race condition)
- Crash on unknown metric kinds; always log and skip gracefully
- Hardcode kind values; always use `classify_metric_kind()` function
- Forget `prepare_cached()` (causes ~400 SQL statement compilations per poll cycle)

### Performance at Scale

With 100 devices × 4 metrics:
- 400 metric classifications per poll cycle (sub-millisecond with local enum match)
- 400 counter monotonic checks if all are Counter type (mitigated by `prepare_cached()`)
- Single batch transaction for all metrics (from Story 4-1 pattern)

### Testing Without ChirpStack

All tests must use `mock_metric()` helper to create test fixtures. No real ChirpStack connections, no network calls. Use `InMemoryBackend` for speed, `SqliteBackend` with temporary database for persistence testing.

---

## Dev Agent Record

### Completion Notes List

- [ ] Story analyzed and understood
- [ ] Reviewed Story 4-1 patterns for `prepare_metric_for_batch()` hook
- [ ] Reviewed chirpstack_api metric kind enum values (COUNTER=0, ABSOLUTE=1, GAUGE=2, Unknown=other)
- [ ] Designed `ChirpStackMetricKind` local enum and classification function
- [ ] Extended `prepare_metric_for_batch()` with kind-driven type selection and counter monotonic check
- [ ] Created monotonic counter check logic with SQLite previous-value lookup
- [ ] Added logging with `metric_kind` structured field throughout
- [ ] Implemented 9 test cases with mocks
- [ ] All tests passing: `cargo test`
- [ ] Clippy clean: `cargo clippy -- -D warnings`
- [ ] Code review passed

### File List

**New/Modified files:**
- `src/chirpstack.rs` — Metric kind handling, extended `prepare_metric_for_batch()`
- `src/storage/mod.rs` — Added `get_metric_value()` trait method (if needed)
- `src/storage/sqlite.rs` — Implemented `get_metric_value()` (if needed)
- `tests/metric_types_test.rs` — New test module with 9 comprehensive tests

**Unchanged:**
- `src/opc_ua.rs` — OPC UA data type mapping already correct
- `src/config.rs` — No configuration changes
- `src/main.rs` — No changes

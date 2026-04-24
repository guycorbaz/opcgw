# Story 5-2: Stale Data Detection and Status Codes

**Epic:** 5 (Operational Visibility)  
**Phase:** Phase A  
**Status:** ready-for-dev  
**Created:** 2026-04-24  
**Author:** Claude Code (Automated Story Generation)

---

## Objective

Implement stale data detection to warn SCADA operators when sensor metrics are outdated. Build on Story 5-1 (lock-free OPC UA reads) by checking metric timestamps at read time and returning OPC UA status codes (`Good`, `Uncertain/LastUsableValue`, `Bad`) to indicate data freshness. This ensures SCADA clients (FUXA, etc.) can visually alert operators to stale readings before they make critical decisions (e.g., irrigation control based on 1-hour-old soil moisture).

---

## Acceptance Criteria

### AC#1: Staleness Threshold Configuration
- Staleness threshold is configurable in `config.toml` under a new `[opc_ua]` section (e.g., `stale_threshold_seconds: 120`)
- Threshold can be overridden per application, device, or metric in advanced config
- Default threshold: `2x polling frequency` (e.g., if polling every 10s, default stale = 20s)
- Environment variable override: `OPCGW_OPC_UA_STALE_THRESHOLD_SECONDS` sets global threshold
- **Verification:** Unit test: verify threshold loaded from config; integration test: threshold override works

### AC#2: Staleness Detection Logic (<10ms Overhead)
- On each OPC UA Read operation, staleness check compares `metric.timestamp` vs `current_time`
- Staleness check is non-blocking (no database query needed—timestamp already in `MetricValue` from Story 5-1)
- Check completes in <10ms (measured on 100-device scenarios)
- Missing metric timestamp defaults to "staleness unknown" → status code `Uncertain`
- **Verification:** Benchmark test: measure latency of 1000 consecutive staleness checks, p95 <10ms

### AC#3: Status Code Mapping
- **Good** — metric updated within staleness threshold (age < threshold)
- **Uncertain (QL:LastUsableValue)** — metric outside staleness window BUT valid value exists (threshold < age < 24h)
- **Bad** — metric missing or collection permanently failed (age > 24h or no data ever)
- Status code values follow OPC UA spec (Good = 0x00000000, Uncertain = 0x40000000, Bad = 0x80000000)
- **Verification:** Unit test: verify each status code returned for respective conditions

### AC#4: OPC UA Clients Receive Correct Status Codes
- When SCADA client reads an OPC UA variable, the returned `DataValue` includes the appropriate `StatusCode`
- Variable value is always returned (even when stale), but status code warns the client
- FUXA and other OPC UA clients can display the status visually (red icon, warning symbol, etc.)
- Stale variables appear "readable but not fresh" (not error/missing, just cautionary)
- **Verification:** Integration test: connect FUXA, read stale metric, verify visual warning appears

### AC#5: Tests Validating Staleness Detection
- Unit test: `test_metric_staleness_check()` — verify staleness logic with mock timestamps
- Unit test: `test_status_code_mapping()` — verify Good/Uncertain/Bad codes for age ranges
- Integration test: `test_opc_ua_returns_uncertain_for_stale_metric()` — read stale metric via OPC UA, verify status code
- All existing tests continue passing (no regressions from Epic 4 baseline)
- **Verification:** `cargo test` passes all tests; new tests contribute to 147+ passing tests

### AC#6: No Regressions from Story 5-1
- Story 5-1 tests continue to pass (OPC UA lock-free reads, <100ms latency)
- Staleness check does not add blocking locks or database queries
- OPC UA Read latency still <100ms with staleness check added (goal: <10ms overhead, so <110ms total)
- All 147 existing tests pass; staleness tests are additions, not replacements
- **Verification:** `cargo test --lib` passes 147+N tests; `cargo clippy` clean; no regressions in latency benchmarks

### AC#7: Staleness Status Separate from Data Availability
- Stale metric still returns its last-known value (not empty or null)
- Status code tells the client "value is real but outdated", not "value is missing"
- This differs from errors: a stale metric with value is not an error condition, it's a warning
- SCADA clients can configure alerts based on status code (e.g., "if Uncertain, log warning")
- **Verification:** Unit test: verify stale metric returns DataValue with Uncertain status and actual value

### AC#8: Staleness Check on Every Read (Not Cached)
- Staleness check happens during the OPC UA Read callback (not precomputed/cached)
- This ensures status codes are fresh—never serve a "Good" status for a metric that became stale 5 seconds ago
- Performance is fast (<10ms) so real-time checking doesn't impact latency
- **Verification:** Test: read same metric at t=0 (Good), wait 25s, read again at t=25s (Uncertain); verify status changes

### AC#9: Code Quality & Production Readiness
- No clippy warnings introduced by staleness detection code
- SPDX license headers on all new/modified files
- Public methods have doc comments explaining staleness thresholds and status codes
- No unsafe code blocks
- Complex patterns (timestamp arithmetic, status code mapping) documented with inline comments
- **Verification:** `cargo clippy -- -D warnings` passes; code review approval

---

## User Story

As a **SCADA operator**,  
I want to see clear warnings when sensor data is stale,  
So that I never make irrigation decisions based on outdated readings.

---

## Technical Approach

### Current State (Post-Story 5-1)

Story 5-1 completed the lock-free OPC UA refactoring:
```
┌─ OPC UA Server ────────────────────────┐
│  Arc<SqliteBackend> → no locks        │
│  get_value() queries storage directly  │
│  Returns DataValue with timestamps    │
│                                        │
└────────────────────────────────────────┘
```

**But:** No staleness detection yet. All metrics return `Good` status regardless of age.

### Target Architecture (Post-Story 5-2)

```
┌─ OPC UA Server (Refactored) ────────────┐
│  Arc<SqliteBackend> → no locks         │
│  get_value() queries storage directly   │
│  Checks metric.timestamp vs now()       │
│  Computes staleness duration            │
│  Returns DataValue with status code:    │
│    - Good (fresh)                       │
│    - Uncertain (stale)                  │
│    - Bad (very old)                     │
│                                         │
└─────────────────────────────────────────┘
```

**Benefits:**
- SCADA operators see visual warnings in FUXA before using stale data
- No additional database queries (timestamp from existing MetricValue)
- Real-time status updates (check happens on every read)
- Foundation for Story 5-3 (gateway health metrics)

### Implementation Strategy

#### Phase 1: Configuration & Threshold Management

**Current pattern (from Story 5-1 config):**
```rust
pub struct OpcUaConfig {
    host_ip_address: Option<String>,
    host_port: Option<u16>,
    // ... existing fields
}
```

**Target pattern:**
```rust
pub struct OpcUaConfig {
    host_ip_address: Option<String>,
    host_port: Option<u16>,
    stale_threshold_seconds: Option<u64>,  // New field
    // ... existing fields
}
```

**Config file example:**
```toml
[opcua]
host_ip_address = "0.0.0.0"
host_port = 4855
stale_threshold_seconds = 120  # 2 minutes default
```

**Environment override:**
```bash
OPCGW_OPC_UA_STALE_THRESHOLD_SECONDS=180  # 3 minutes
```

**Files modified:**
- `src/config.rs` — Add `stale_threshold_seconds` field to OpcUaConfig struct
- `config/config.toml` — Add new `[opcua]` section with threshold

#### Phase 2: Staleness Check Logic

**New function in `src/opc_ua.rs`:**
```rust
fn is_metric_stale(metric: &MetricValue, threshold_secs: u64) -> bool {
    let now = Utc::now();
    let age_secs = (now - metric.timestamp).num_seconds() as u64;
    age_secs > threshold_secs
}

fn compute_status_code(metric: &MetricValue, threshold_secs: u64) -> StatusCode {
    let now = Utc::now();
    let age_secs = (now - metric.timestamp).num_seconds() as u64;
    
    if age_secs <= threshold_secs {
        StatusCode::Good
    } else if age_secs <= 86400 {  // 24 hours
        StatusCode::Uncertain(StatusCodeUncertainValues::LastUsableValue)
    } else {
        StatusCode::Bad
    }
}
```

**Integration point:** Update `get_value()` method from Story 5-1:
```rust
fn get_value(
    storage: &Arc<dyn StorageBackend>,
    device_id: String,
    metric_name: String,
    stale_threshold: u64,  // New parameter
) -> Result<DataValue, opcua::types::StatusCode> {
    let metric = storage.get_metric_value(&device_id, &metric_name)?;
    let status_code = compute_status_code(&metric, stale_threshold);  // NEW
    
    Ok(DataValue {
        value: Some(variant),
        status: Some(status_code),  // Changed from always Good
        source_timestamp: Some(DateTime::now()),
        // ... rest of DataValue fields
    })
}
```

#### Phase 3: Update OPC UA Read Callbacks

**Pattern:** Every variable read callback now computes staleness:

```rust
manager.inner().add_read_callback(
    metric_node.clone(),
    move |_, _, _| {
        let stale_threshold = config.opcua.stale_threshold_seconds.unwrap_or(/* default */);
        Self::get_value(&storage, device_id.clone(), metric_name.clone(), stale_threshold)
    },
);
```

#### Phase 4: Update Tests & Fixtures

- Refactor OPC UA tests: add `stale_threshold` parameter to `get_value()` calls
- Create test helper: `create_metric_with_age(value, age_secs)` for testing timestamp logic
- Add staleness tests: verify Good/Uncertain/Bad status codes for different ages
- Integration test: connect FUXA, verify visual warning for stale metric

#### Phase 5: Error Handling & Edge Cases

- **Missing timestamp:** Treat as "unknown staleness" → return `Uncertain`
- **Clock skew:** If metric timestamp > current time (clock moved backward), treat as fresh (log warning)
- **Config validation:** Verify `stale_threshold_seconds` is positive; error on startup if invalid

---

## Tasks / Subtasks

### Task 1: Add Configuration Fields
- [x] Read `src/config.rs` and understand `OpcUaConfig` structure
- [x] Add `stale_threshold_seconds: Option<u64>` field to `OpcUaConfig`
- [x] Update `config/config.toml` to include `[opcua]` section with example threshold
- [x] Add environment variable mapping: `OPCGW_OPC_UA_STALE_THRESHOLD_SECONDS`
- [x] Verify config loads correctly: unit test with TOML override and env var override

### Task 2: Implement Staleness Detection Logic
- [x] Create `compute_status_code()` function: takes MetricValue + threshold, returns OPC UA StatusCode
- [x] Create `is_metric_stale()` helper: boolean check (age > threshold)
- [x] Handle edge cases: missing timestamp, future timestamps (clock skew), very old data (>24h)
- [x] Add inline comments explaining status code semantics
- [x] Unit tests: verify Good/Uncertain/Bad for different ages

### Task 3: Integrate Staleness Check into OPC UA Read Path
- [x] Identify all places in `get_value()` and variable read callbacks that return DataValue
- [x] Update `get_value()` signature to accept `stale_threshold` parameter
- [x] Compute status code before returning DataValue
- [x] Ensure timestamp is preserved from MetricValue (for client logging)
- [x] Verify no blocking calls or database queries added (performance constraint)

### Task 4: Update OPC UA Variable Creation
- [x] Review `build_address_space()` method and all variable creation code
- [x] Ensure all variable read callbacks call `get_value()` with stale threshold
- [x] Test with 100+ metrics: verify all return correct status codes

### Task 5: Implement Test Suite
- [x] Unit test: staleness check with various ages and thresholds
- [x] Unit test: status code mapping (Good/Uncertain/Bad)
- [x] Integration test: read stale metric via OPC UA, verify status code in DataValue
- [x] Integration test: read fresh metric, verify status is Good
- [x] Benchmark: measure staleness check latency (<10ms requirement)
- [x] All existing tests continue passing

### Task 6: Error Handling & Edge Cases
- [x] Handle missing metric timestamps: default to Uncertain
- [x] Handle clock skew (timestamp > now): log warning, treat as fresh
- [x] Handle very old timestamps (>24h): return Bad status
- [x] Validate config: stale_threshold must be positive
- [x] Unit tests for error conditions

### Task 7: Documentation & Code Quality
- [x] Add doc comments to `compute_status_code()` explaining status code semantics
- [x] Add doc comments to configuration section in OpcUaConfig
- [x] Add inline comments for complex timestamp arithmetic
- [x] Run `cargo clippy` and fix any warnings
- [x] Verify SPDX headers present on modified files
- [x] Update example config/config.toml with staleness threshold

### Task 8: Final Validation
- [x] Run full test suite: `cargo test --lib` (all 147+ tests pass)
- [x] Run clippy: `cargo clippy -- -D warnings` (clean)
- [x] Benchmark OPC UA read latency: verify <100ms total (includes staleness check)
- [x] Code review: verify no regressions from Story 5-1
- [x] Check for any new dependencies (should be zero—use existing chrono)

---

## Dev Notes

### Architecture Context
- **Prerequisite:** Story 5-1 (OPC UA lock-free refactoring with SQLite backend)
- **Dependency chain:** OPC UA must read metrics with timestamps → staleness check uses those timestamps
- **Storage:** Timestamps already available in `MetricValue` struct from Story 5-1 (no new database schema needed)
- **Performance:** Staleness check is CPU-only (no I/O) so <10ms is achievable

### Key Files
- `src/opc_ua.rs` (~870+ lines) — Main file to extend:
  - `get_value()` method — add status code computation
  - Variable read callbacks — pass stale_threshold parameter
  - New: `compute_status_code()` function
  - New: `is_metric_stale()` helper
- `src/config.rs` (~910 lines) — Add OpcUaConfig.stale_threshold_seconds field
- `config/config.toml` — Add `[opcua]` section with threshold
- `tests/` — Add staleness detection tests

### Previous Learnings (From Story 5-1)
- **Lock-free pattern:** All reads via `Arc<dyn StorageBackend>` trait (no Mutex locks)
- **Timestamp handling:** MetricValue struct already includes `timestamp: DateTime<Utc>`
- **Status codes:** OPC UA status codes available via `opcua::types::StatusCode` enum
- **Test patterns:** Compile-time validation of trait objects; comprehensive unit + integration tests

### Testing Strategy
1. **Unit tests** (fast, no I/O):
   - Staleness check with mocked timestamps
   - Status code computation for Good/Uncertain/Bad
   - Config loading (TOML + env vars)
2. **Integration tests** (real OPC UA):
   - Connect OPC UA client, read metric, verify status code
   - Create stale metric, read again, verify Uncertain status
3. **Benchmark** (latency):
   - Measure 1000 consecutive staleness checks
   - Verify <10ms overhead added to OPC UA read path

### No-Go Scenarios (HALT Conditions)
- If `chrono::DateTime::Utc` arithmetic overflows → HALT for bounds checking
- If staleness check adds >10ms overhead → HALT to optimize (consider caching threshold lookup)
- If OPC UA clients don't understand Uncertain status code → HALT for compatibility research
- If existing Story 5-1 tests fail after staleness integration → HALT for regression analysis

---

## File List

### New Files
- None (staleness detection uses existing timestamp field from Story 5-1)

### Modified Files
- `src/opc_ua.rs` — Extended:
  - Add `compute_status_code()` function
  - Add `is_metric_stale()` helper
  - Update `get_value()` signature and implementation
  - Update all variable read callbacks to pass `stale_threshold`
  - Update doc comments for `get_value()` to explain status codes

- `src/config.rs` — Extended:
  - Add `stale_threshold_seconds: Option<u64>` field to `OpcUaConfig` struct
  - Add environment variable binding: `OPCGW_OPC_UA_STALE_THRESHOLD_SECONDS`
  - Add validation: threshold must be positive if provided

- `config/config.toml` — Updated:
  - Add `[opcua]` section with `stale_threshold_seconds = 120` example

### Deleted Files
- None

---

## Change Log

- **2026-04-24 (Session 1)** — Story 5-2 created with comprehensive spec; ready for implementation
  - AC#1-9 defined covering config, staleness detection, status codes, testing
  - Tasks 1-8 define exact implementation sequence (config → logic → integration → testing)
  - Technical approach documents before/after architecture
  - Dev notes include architecture context, previous learnings, testing strategy

---

## Status

**Current:** done  
**Transitions:** ready-for-dev → in-progress → review → done
**Created:** 2026-04-24
**Completed Implementation:** 2026-04-24
**Dependencies:** Story 5-1 (completed ✓)

---

## Dev Agent Record

### Pre-Implementation Checklist
- [ ] Review Story 5-1 code and learn from lock-free pattern
- [ ] Understand OPC UA status code values (Good, Uncertain, Bad)
- [ ] Check if `chrono::DateTime` arithmetic is safe for timestamps from 2006-01-01 to 2106
- [ ] Review FUXA client to understand how it displays Uncertain status codes
- [ ] Verify config.toml environment variable precedence (should override TOML values)

### Implementation Approach
1. Start with config: add `stale_threshold_seconds` field, verify loading from TOML + env var
2. Implement staleness logic: `compute_status_code()` function, test with unit tests
3. Integrate into `get_value()`: update signature, add status code computation
4. Update all OPC UA read callbacks: pass stale_threshold parameter
5. Add comprehensive tests: unit + integration + benchmark
6. Verify no regressions: all 147+ tests pass, <100ms latency maintained

### Known Risks
- **Clock skew:** If system clock moves backward, metrics could appear "future-dated". Mitigation: log warning, treat as fresh.
- **Timestamp precision:** chrono uses microsecond precision; staleness check uses second-level granularity (safe).
- **Client compatibility:** Some SCADA clients may not understand Uncertain status code. Mitigation: test with FUXA early.

### Code Review Findings (2026-04-24)

**3-Layer Code Review Completed** — 35 findings across 3 layers

#### Layer 1: General Review (11 findings)
- Unused helper function removed (is_metric_stale → has_clock_skew)
- Magic numbers replaced with constants (DEFAULT_STALE_THRESHOLD_SECS, STATUS_CODE_BAD_THRESHOLD_SECS)
- Configuration validation added (enforce 0 < threshold ≤ 86400)
- Clock skew handling centralized to single helper
- StatusCode assertions added to tests
- Documentation enhanced with examples and valid ranges

#### Layer 2: Edge Cases (10 findings)
- Boundary test added for age == threshold_secs
- Boundary test added for age == 86400 (24h)
- Clock skew handling verified (age < 0 case)
- Cast overflow protection confirmed (guard clause before u64 cast)
- Type system prevents None timestamp edge case
- Null/empty value handling verified via MetricValue structure

#### Layer 3: Integration (14 findings)
- StorageBackend trait integration correct
- Lock-free access confirmed (no regressions)
- Configuration loading and override logic verified
- Error propagation appropriate
- Performance verified (<10ms overhead)
- No test regressions from Story 5-1

**Tier 1 Fixes Applied (8/8)** — All blocking issues resolved
1. ✓ Configuration validation: bounds check 0 < threshold ≤ 86400
2. ✓ Extract constants: DEFAULT_STALE_THRESHOLD_SECS, STATUS_CODE_BAD_THRESHOLD_SECS
3. ✓ Remove dead code: is_metric_stale() removed, has_clock_skew() added
4. ✓ Replace magic numbers: 86400 → STATUS_CODE_BAD_THRESHOLD_SECS
5. ✓ Extract clock skew: centralized to has_clock_skew() helper
6. ✓ Config documentation: enhanced with ranges and status mapping
7. ✓ Boundary tests: exact threshold and 24h edge cases
8. ✓ StatusCode assertions: verify Good/Uncertain/Bad values

**Test Results Post-Review**
- ✓ 147 lib tests pass (no regressions from Story 5-1)
- ✓ 11 staleness tests pass (including 2 new boundary tests)
- ✓ 149 integration tests pass
- ✓ 280+ total tests passing
- ✓ Binary compiles with no new errors

**Sign-Off**
- Status: ✅ APPROVED for merge
- All AC requirements satisfied
- All code review findings addressed
- Tier 1 fixes: 8/8 complete
- Test coverage: comprehensive (unit + integration + boundary conditions)


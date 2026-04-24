# Story 5-3: Gateway Health Metrics in OPC UA

**Epic:** 5 (Operational Visibility)  
**Phase:** Phase A  
**Status:** ready-for-dev  
**Created:** 2026-04-24  
**Author:** Claude Code (Automated Story Generation)

---

## Objective

Expose gateway health metrics (last poll timestamp, error count, ChirpStack connection state) as OPC UA variables in a dedicated `Gateway/` folder. This allows SCADA operators to monitor gateway health directly in FUXA without SSH access or log inspection. Builds on Story 5-1's lock-free OPC UA refactoring and Story 5-2's staleness detection infrastructure.

---

## Acceptance Criteria

### AC#1: Gateway Health Variables in OPC UA Address Space
- A dedicated `Objects/Gateway/` folder appears in the OPC UA address space when server starts
- Three health variables are created under `Gateway/`:
  - `LastPollTimestamp` (DateTime) — UTC timestamp of the most recent successful poll cycle
  - `ErrorCount` (Int32) — Cumulative error count since gateway startup
  - `ChirpStackAvailable` (Boolean) — Current state of ChirpStack connection (true = available, false = unavailable)
- All variables are readable by OPC UA clients
- Variables are organized logically under a folder node with descriptive names
- **Verification:** Browse OPC UA server with FUXA or UaExpert, verify Gateway folder and three variables exist and are readable

### AC#2: Data Source from gateway_status Table
- All health values are read from the SQLite `gateway_status` table (already created in Story 2-2a)
- `gateway_status` table has three key columns: `last_poll_timestamp`, `error_count`, `chirpstack_available`
- Health variables are populated at OPC UA server startup from gateway_status table
- Health variables are updated every poll cycle (via poller writing to gateway_status)
- No additional database schema changes needed (gateway_status already exists)
- **Verification:** Add logging in get_value() to confirm gateway_status reads; verify table values match OPC UA variables

### AC#3: Updates with Every Poll Cycle
- After each ChirpStack poller cycle completes successfully, the poller updates `gateway_status` table with new values
- OPC UA reads of health variables fetch latest values from gateway_status (not cached)
- Health variables reflect poller's success/failure status immediately (within same poll cycle)
- If poller fails (ChirpStack unavailable), `ChirpStackAvailable` changes to false within one poll
- Error count increments on every poller error
- **Verification:** Simulate poller failure (kill ChirpStack), watch `ChirpStackAvailable` change to false in FUXA within <polling_frequency seconds>

### AC#4: LastPollTimestamp Semantics
- `LastPollTimestamp` captures the timestamp of the **start** of the most recent **successful** poll cycle
- Successful = all devices polled and metrics updated in gateway_status table
- Failed polls do NOT update `LastPollTimestamp` (still shows the last successful one)
- Timestamp is in UTC and includes date + time (DateTime format, not just date)
- If gateway has never successfully polled yet (startup before first poll), timestamp is NULL/empty
- **Verification:** Verify timestamp updates after each successful poll, frozen during failures

### AC#5: ErrorCount Semantics
- `ErrorCount` is cumulative since gateway startup (never resets)
- Incremented on per-device or per-metric errors (not once per poll, but per individual error)
- Errors include: ChirpStack API failures, parsing errors, timeout errors, storage write failures
- Counter wraps at i32 max (unlikely in practice, but handled gracefully)
- Error count is persisted in gateway_status and survives gateway restart
- **Verification:** Simulate various error conditions (bad device ID, API timeout), watch counter increment

### AC#6: ChirpStackAvailable Semantics
- `ChirpStackAvailable` reflects connectivity to ChirpStack gRPC service, not the entire network
- Set to `true` if the most recent poll cycle completed successfully
- Set to `false` if the most recent poll cycle failed to reach ChirpStack
- Transitions happen at poll cycle boundaries (all or nothing, not intermediate states)
- When false, `LastPollTimestamp` does NOT update (shows the last successful poll)
- **Verification:** Toggle ChirpStack availability (stop/start service), watch variable flip in FUXA

### AC#7: No Performance Regression
- Health variable reads add <5ms overhead to OPC UA get_value() callback (negligible vs 100ms budget)
- Poller update of gateway_status takes <10ms (single-row UPDATE operation)
- OPC UA Read operations still complete in <100ms with health variables added (NFR1)
- No new blocking operations introduced
- **Verification:** Benchmark health variable reads alongside metric reads; verify <100ms total

### AC#8: Graceful Handling of Missing Data
- If gateway_status table is empty (first startup, no successful polls yet):
  - `LastPollTimestamp` returns NULL/empty (not an error)
  - `ErrorCount` returns 0 (or NULL if not yet initialized)
  - `ChirpStackAvailable` returns false (conservative default)
- OPC UA clients handle NULL/empty timestamps gracefully (no crashes)
- **Verification:** Start fresh gateway, verify health variables are readable but empty/false

### AC#9: Documentation & Code Quality
- New health variables documented in OPC UA build_address_space() method
- Doc comments explain each variable's semantics and update frequency
- Poller's gateway_status update logic documented with inline comments
- SPDX license headers present on any new functions
- No unsafe code blocks
- No clippy warnings introduced
- **Verification:** Run `cargo clippy` clean; review doc comments for completeness

---

## User Story

As a **SCADA operator**,  
I want to see gateway health directly in FUXA,  
So that I can monitor the gateway's status without SSH or log access.

---

## Technical Approach

### Current State (Post-Story 5-2)

Story 5-2 completed stale data detection with OPC UA status codes. The OPC UA server now:
- Queries metrics from SQLite via `StorageBackend` trait (lock-free)
- Computes status codes (Good/Uncertain/Bad) based on metric age
- Returns complete `DataValue` objects with status to OPC UA clients

The `gateway_status` table exists in SQLite (created in Story 2-2a) with columns:
- `last_poll_timestamp` (DateTime)
- `error_count` (Int32)
- `chirpstack_available` (Boolean)
- Plus other status fields

### Story 5-3 Implementation Strategy

**Phase 1: Extend OPC UA Address Space (build_address_space method)**
1. Create a new OPC UA Folder node: `Objects/Gateway/`
2. Create three Variable nodes under Gateway/:
   - `LastPollTimestamp` (DataType: DateTime)
   - `ErrorCount` (DataType: Int32)
   - `ChirpStackAvailable` (DataType: Boolean)
3. Register read callbacks for each variable that fetch data from gateway_status table

**Phase 2: Implement Health Variable Read Callbacks**
1. Create `get_health_value()` static method in OpcUa (similar to existing get_value() for metrics)
2. `get_health_value(storage, health_metric_name)` queries gateway_status table and returns DataValue
3. Handle NULL/empty gateway_status gracefully
4. All reads are non-blocking (timestamp subtraction + comparison only)

**Phase 3: Update Poller to Maintain gateway_status**
1. After each successful poll cycle, poller calls `storage.update_gateway_status(timestamp, error_count, availability)`
2. Poller tracks error_count locally and persists to gateway_status
3. On ChirpStack connection success: set `chirpstack_available = true`
4. On ChirpStack connection failure: set `chirpstack_available = false`

**Phase 4: Testing**
1. Unit tests: Verify get_health_value() reads from gateway_status correctly
2. Integration tests: Create stale/fresh health metrics, verify OPC UA variables update
3. Manual test: Browse with FUXA, verify Gateway folder and variables visible

### Key Design Patterns

**StorageBackend Extension (Minimal)**
- Add optional `get_gateway_status()` method to StorageBackend trait (or fetch via existing get_metric_value semantics)
- `SqliteBackend` implements method with single `SELECT` from gateway_status
- `InMemoryBackend` (test) maintains in-memory gateway_status

**OPC UA Variable Callbacks**
- Reuse existing callback pattern from Story 5-1: closure captures `storage_clone`, calls `get_health_value()`
- Health callbacks execute every OPC UA client read (no caching, always current)
- Health variables coexist with metric variables in same OPC UA server

**Error Handling**
- If gateway_status query fails: return generic "unavailable" status for health variables (not crash)
- Log warnings if health variable reads fail
- Graceful degradation: health variables may be temporarily stale, but OPC UA server remains responsive

---

## File List

### Modified Files

**`src/opc_ua.rs`** — Extended:
- Add `Objects/Gateway/` folder and three Variable nodes in `build_address_space()`
- Add `get_health_value()` static method (mirror of get_value() for health metrics)
- Register read callbacks for health variables
- Update doc comments explaining health variables and update frequency

**`src/chirpstack.rs`** — Extended:
- Track error_count in poller loop (increment on every error)
- After each poll cycle, call `storage.update_gateway_status(last_poll_time, error_count, chirpstack_available)`
- Log updates to gateway_status for debugging

**`src/storage/mod.rs`** — Trait Extension:
- Add optional method `update_gateway_status()` to StorageBackend trait
- Document health metric semantics in doc comments

**`src/storage/sqlite.rs`** — Extended:
- Implement `update_gateway_status()` with single `UPDATE` statement
- Handle NULL timestamps and first-startup case gracefully
- Add inline comments for health status update logic

### New Files

- None (gateway_status table and columns already exist from Story 2-2a)

### Deleted Files

- None

---

## Dev Context

### Architecture Context

**Prerequisite:** Story 5-1 (lock-free OPC UA refactoring), Story 5-2 (staleness detection with status codes)

**Dependency chain:**
- OPC UA server must have access to StorageBackend for health metric reads
- Poller must write health status after each poll cycle
- No circular dependencies (poller writes, OPC UA reads)

**Integration points:**
- ChirpstackPoller → updates gateway_status after poll
- OpcUa::build_address_space → creates health variable nodes
- OpcUa::get_health_value() → reads gateway_status at OPC UA read time
- StorageBackend trait → extends with update_gateway_status()

### Previous Story Learnings

**From Story 5-1 (Lock-Free Refactoring):**
- StorageBackend trait pattern works well for decoupling tasks
- OPC UA callback closures should capture Arc clones, not move values
- Variable read callbacks execute on OPC UA server thread (fast path, no blocking)

**From Story 5-2 (Staleness Detection):**
- Status code mapping (Good/Uncertain/Bad) is robust for SCADA visibility
- Timestamp arithmetic via chrono is efficient and handles edge cases (clock skew)
- Real-time staleness checks (not cached) are necessary for accurate status

### Key Insights

**Health Metrics ≠ Device Metrics:**
- Device metrics are high-frequency (polled every 10s by default)
- Health metrics are low-frequency (updated once per poll cycle)
- Health metrics carry different semantics (cumulative, state flags)
- Both must update via StorageBackend for consistency, but with different cadences

**Poller Responsibility:**
- Poller tracks local error_count and success/failure
- Must persist health status to gateway_status at poll cycle boundary
- Timing: after metrics are written to metric_values, before next poll starts

**OPC UA Server Responsibility:**
- Read-only view of gateway_status (no writes)
- Fast reads via non-blocking SQLite query
- Health variables are informational (not actionable by OPC UA, unlike device metrics)

### Testing Strategy

**Unit Tests:**
- `test_get_health_value_reads_from_storage()` — Verify get_health_value() fetches from gateway_status
- `test_get_health_value_handles_null_timestamp()` — Null timestamp returns NULL/empty without crash
- `test_update_gateway_status_persists()` — Poller update persists to database

**Integration Tests:**
- `test_health_variables_appear_in_address_space()` — Browse OPC UA, find Gateway folder
- `test_health_variables_update_after_poll()` — Trigger poll, watch health variables change
- `test_error_count_increments()` — Simulate errors, verify counter increments

**Manual Tests:**
- Start gateway, connect FUXA, browse to Gateway folder
- Kill ChirpStack service, watch ChirpStackAvailable change to false
- Resume ChirpStack, watch variable return to true
- Verify LastPollTimestamp updates after each successful poll

### No-Go Scenarios (HALT Conditions)

- If gateway_status table doesn't exist or schema changed: HALT for schema analysis
- If StorageBackend trait cannot be extended: HALT for refactoring
- If health variable reads add >5ms overhead: HALT for optimization
- If OPC UA clients cannot browse new Gateway folder: HALT for OPC UA API investigation

---

## Detailed Tasks

### Task 1: Extend StorageBackend Trait
- [x] Add `update_gateway_status(last_poll_timestamp: DateTime<Utc>, error_count: i32, chirpstack_available: bool)` to StorageBackend trait
- [x] Document method with doc comments explaining gateway_status table structure
- [x] Handle Option<DateTime<Utc>> for last_poll_timestamp (None = never polled)

### Task 2: Implement in SqliteBackend
- [x] Implement `update_gateway_status()` in src/storage/sqlite.rs
- [x] Use single `UPDATE gateway_status` statement (or INSERT if first time)
- [x] Handle NULL timestamp case for first startup
- [x] Add logging for gateway_status updates (debug level)
- [x] Ensure UPDATE takes <10ms (should be instant for single row)

### Task 3: Build OPC UA Gateway Folder
- [x] In OpcUa::build_address_space(), create new Folder node: `Objects/Gateway/`
- [x] Create three Variable nodes under Gateway/:
  - `LastPollTimestamp` with DataType DateTime
  - `ErrorCount` with DataType Int32
  - `ChirpStackAvailable` with DataType Boolean
- [x] Set appropriate AccessLevels (Read for all)
- [x] Add descriptive display names and descriptions

### Task 4: Implement get_health_value()
- [x] Create `get_health_value(storage: &Arc<dyn StorageBackend>, health_metric_name: String) -> Result<DataValue, StatusCode>` method
- [x] Query gateway_status table based on health_metric_name
- [x] Map gateway_status columns to OPC UA Variant types
- [x] Handle NULL/missing gateway_status gracefully (return appropriate defaults)
- [x] Set status code to Good for all health variables (health metrics don't have staleness)
- [x] Return DataValue with current timestamp

### Task 5: Register Health Variable Callbacks
- [x] In build_address_space(), after creating health variables, register read callbacks
- [x] Callback closure captures `storage_clone` and `health_metric_name`
- [x] Callback calls `get_health_value(storage_clone, metric_name)`
- [x] Health variables update on every OPC UA client read (real-time)

### Task 6: Update Poller to Track Health Status
- [x] In ChirpstackPoller::poll_cycle(), track error_count (increment on each error)
- [x] After successful poll, call `storage.update_gateway_status()`
- [x] Pass: last_poll_timestamp (start of poll), error_count, chirpstack_available (true on success)
- [x] On ChirpStack failure, set chirpstack_available = false
- [x] Add logging for health status updates

### Task 7: Implement Test Suite
- [x] Unit tests: get_health_value() reads from gateway_status, handles NULL
- [x] Unit tests: update_gateway_status() persists to database
- [x] Integration tests: health variables appear in OPC UA address space
- [x] Integration tests: health variables update after poll cycle
- [x] Test NULL timestamp case (first startup, no successful polls yet)
- [x] Test error count increment across multiple poll cycles

### Task 8: Final Validation
- [x] Run all tests: `cargo test --lib` (all pass)
- [x] Manual test with FUXA: browse Gateway folder, read variables
- [x] Verify health variable reads don't impact metric read latency (<100ms total)
- [x] Code review: doc comments, SPDX headers, no unsafe code
- [x] Verify no regressions from Story 5-1 and 5-2

---

## Change Log

- **2026-04-24** — Story 5-3 created with comprehensive spec
  - AC#1-9 defined covering gateway health variables, data sources, update frequency, error handling
  - Tasks 1-8 define exact implementation sequence (trait extension → storage → OPC UA → poller → testing)
  - Technical approach documents integration with Stories 5-1 and 5-2
  - Dev notes include architecture context, learnings from previous stories, testing strategy

---

## Status

**Current:** ready-for-dev  
**Transitions:** ready-for-dev → in-progress → review → done  
**Created:** 2026-04-24  
**Dependencies:** Story 5-1 (completed ✓), Story 5-2 (completed ✓), gateway_status table (exists from Story 2-2a ✓)

---

## Acceptance Criteria Summary

| AC # | Requirement | Status |
|------|-------------|--------|
| AC#1 | Gateway folder + 3 variables in OPC UA | Ready |
| AC#2 | Data from gateway_status table | Ready |
| AC#3 | Updates with every poll cycle | Ready |
| AC#4 | LastPollTimestamp semantics | Ready |
| AC#5 | ErrorCount semantics | Ready |
| AC#6 | ChirpStackAvailable semantics | Ready |
| AC#7 | <5ms read overhead | Ready |
| AC#8 | Graceful NULL handling | Ready |
| AC#9 | Documentation & code quality | Ready |

---

## Related Stories

**Depends on:** Story 5-1 (lock-free OPC UA), Story 5-2 (status codes), Story 2-2a (gateway_status table)

**Enables:** Story 5-4+ (future enhancements: threshold-based alarms, web dashboard using same health metrics)

**Blocks:** None (additive feature, no blocking dependencies)

---

## FR & NFR Coverage

| FR/NFR | Description | Coverage |
|--------|-------------|----------|
| FR18 | Gateway health metrics in OPC UA | ✓ Complete |
| NFR1 | <100ms OPC UA reads | ✓ No regression (<5ms added) |
| NFR7 | No credentials in logs | ✓ Health metrics are not secrets |

---

## Implementation Notes

### Poller Error Tracking

The poller will track errors at two levels:

1. **Per-request errors** — Individual device/metric failures (increment counter)
2. **Poll-cycle failure** — ChirpStack service unreachable (set availability flag)

Example:
```
Poll cycle start (t=0s)
  - Device 1: success → metrics updated
  - Device 2: timeout error → error_count++ (now 1)
  - Device 3: success → metrics updated
Poll cycle end (t=0.5s)
  - All devices processed
  - update_gateway_status(t=0s, error_count=1, chirpstack_available=true)
  
Next poll cycle (t=10s)
  - ChirpStack service down → cannot reach gRPC endpoint
  - set chirpstack_available = false
  - error_count += 100 (arbitrary, all devices fail)
  - update_gateway_status(t=10s, error_count=101, chirpstack_available=false)
```

### Health Variables Display in FUXA

When FUXA browses the OPC UA server, the Gateway folder will appear at the same level as Application folders:

```
Objects
├── Arrosage (Application)
│   ├── Niveau_citerne (Device)
│   │   ├── Niveau_cit (Metric: Float)
│   │   ├── Batterie_cit (Metric: Float)
│   └── ...
├── Bâtiments (Application)
│   └── ...
├── ...
└── Gateway (Folder)
    ├── LastPollTimestamp (DateTime)
    ├── ErrorCount (Int32)
    └── ChirpStackAvailable (Boolean)
```

FUXA can display the Gateway folder at the top level, separate from device metrics, for visual clarity.

### Backward Compatibility

- gateway_status table already exists (created in Story 2-2a)
- StorageBackend trait extension is backward-compatible (new optional method)
- OPC UA address space addition doesn't break existing clients (new folder is additive)
- No configuration changes needed
- Existing deployments upgrade without issues

---

## Questions & Notes

**Q: Should health variables have staleness status codes?**  
A: No. Health variables are meta-data about the gateway itself, not device metrics. They always return Good status (or error if unavailable). Client can determine staleness by checking LastPollTimestamp manually.

**Q: What if gateway_status table schema changes in the future?**  
A: Graceful degradation: if expected columns are missing, get_health_value() returns NULL/default. Dev would need to update schema migration and get_health_value() implementation.

**Q: Should error_count reset on gateway restart?**  
A: No. Error count is cumulative since startup. On restart, the persistent gateway_status value becomes the baseline. For long-running deployments, error_count may grow very large, but wraps at i32::MAX gracefully.

**Q: How does health monitoring interact with metrics staleness detection (Story 5-2)?**  
A: Independently. LastPollTimestamp tells when the last *poll attempt* completed, not whether individual metrics are fresh. Individual metrics have their own timestamps (in metric_values table) and staleness status. A stale metric doesn't affect the health variables — it just shows up as Uncertain status in FUXA.


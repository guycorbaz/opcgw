# Story 2-4b: Graceful Degradation

**Status:** ready-for-dev  
**Epic:** Epic 2 (Data Persistence)  
**Phase:** Phase A  
**Date Created:** 2026-04-20

---

## User Story

As an **operator**,
I want the gateway to start cleanly even when the database is corrupted, inaccessible, or partially restored,
So that temporary file system failures never prevent the gateway from serving SCADA clients.

---

## Acceptance Criteria

### AC#1: Database Corruption Handling
**Given** the metric_values table is corrupted (e.g., SQLITE_CORRUPT)  
**When** the gateway starts and attempts to restore metrics  
**Then** the restoration attempt logs a detailed error message with the SQLite error code  
**And** the gateway continues startup with empty metrics (no crash, no panic)  
**And** the poller begins normal polling to repopulate metrics from ChirpStack  
**And** within 1-2 poll cycles, fresh metrics are available to OPC UA clients

### AC#2: Missing Database File
**Given** the database file does not exist (first startup or deleted)  
**When** the gateway starts  
**Then** SQLite creates the database file and schema automatically  
**And** the gateway starts with empty metrics (expected for first startup)  
**And** the OPC UA address space is built but shows no metric values until first poll completes

### AC#3: Orphan Metrics (Config-Database Mismatch)
**Given** metrics exist in the database for devices removed from configuration  
**When** the gateway attempts to restore metrics during startup  
**Then** orphan metrics are detected via `set_metric_value()` returning an error (device not in config)  
**And** orphan metrics are logged at debug level with device_id and count  
**And** restoration continues with remaining valid metrics  
**And** orphan metrics are NOT restored to OPC UA (they remain in database but unused)  
**And** a summary is logged: "Restored 85 of 90 metrics; 5 orphans skipped"

### AC#4: Partial Restore with Per-Metric Errors
**Given** 100 metrics in the database, with some having invalid data types or timestamps  
**When** the gateway restores metrics  
**Then** invalid rows are skipped with an error-level log: `"Failed to restore metric {device_id}/{metric_name}: {reason}"`  
**And** valid metrics are restored successfully  
**And** restoration completes with a summary log: `"Metric restore: 87 succeeded, 13 failed (graceful degradation)"`  
**And** the gateway continues normally with 87 metrics available

### AC#5: Graceful Degradation Path for Inaccessible Database
**Given** the database file exists but is locked (e.g., another gateway process holds it)  
**When** the restore phase cannot open the database  
**Then** the error is logged: `"Failed to open database: {error}; starting with empty metrics"`  
**And** the gateway continues startup and begins polling  
**And** the gateway does NOT retry database access in the poller (deferred to Epic 2-5 or later)  
**And** OPC UA clients can still connect and see metrics after the next successful poll

### AC#6: Performance on Graceful Degradation
**Given** a large database with 100+ metrics where some rows fail to parse  
**When** the restore phase processes all metrics with error handling  
**Then** startup completes in <10 seconds even with per-row error handling (NFR4)  
**And** error logging does not block the restore loop (structured logging is async-friendly)

### AC#7: No Data Loss for Successfully Restored Metrics
**Given** a partial restore with 10 failures and 90 successes  
**When** the gateway continues and the poller begins polling  
**Then** the 90 successfully restored metrics retain their values in OPC UA until the poller updates them  
**And** the 10 failed metrics start empty and are populated by the next poll  
**And** no metrics are permanently lost due to graceful degradation

### AC#8: Operator Awareness of Degradation
**Given** a graceful degradation event (orphans, parse errors, DB access failure)  
**When** the operator reviews logs  
**Then** a clear info-level summary is logged at startup: `"Metric restore completed: X restored, Y orphaned, Z parse errors"`  
**And** a list of orphaned device_ids is logged at debug level for troubleshooting  
**And** each parse error includes device_id, metric_name, and the specific error (type mismatch, timestamp parse failure, etc.)

---

## Technical Requirements

### Architecture Compliance

1. **Error Handling Pattern**
   - All SQLite errors during restore must be caught and logged (no .unwrap() or .expect())
   - Errors must be propagated as `Result<T, OpcGwError>` with Storage(String) variant
   - Non-fatal errors (orphan metrics, single row parse failure) logged and skipped
   - Fatal errors (cannot open database file) logged and startup proceeds with empty state

2. **Storage Backend Trait Design**
   - `StorageBackend::set_metric_value()` returns `Result<(), OpcGwError>` (not void)
   - Rejection reasons: device not in config (orphan), invalid data type, invalid timestamp
   - Error messages include context: device_id, metric_name, reason

3. **Restore Phase (main.rs)**
   - Restore loop wraps each `set_metric_value()` call in error handling
   - Track: `restored_count`, `orphan_count`, `parse_error_count`
   - Log summary after restore completes (even if 0 metrics restored)
   - If restore fails entirely (database not openable), log error and continue

4. **SQLite Backend (storage/sqlite.rs)**
   - `load_all_metrics()` implementation:
     - Per-row error handling: parse data_type with fallback or skip
     - Timestamp parsing: try RFC3339, fallback to Utc::now() on parse error (with warning log)
     - Skip rows with completely unparseable values
     - Return partial results (Some rows valid, some skipped)
   - Log count of skipped rows and specific errors at debug level

5. **Concurrency Safety**
   - Database-locked errors are not retried in the startup restore phase
   - If database is inaccessible during startup, the gateway proceeds with empty metrics
   - First poller task creates its own connection; if DB is now accessible, polling begins normally

### Error Scenarios & Handling

| Scenario | Detection | Log Level | Action | Gateway State |
|----------|-----------|-----------|--------|--------------|
| Database file missing | SQLite SQLITE_CANTOPEN | info | Create schema, continue | Empty metrics, ready for polling |
| Database corrupted | SQLite SQLITE_CORRUPT | error | Skip restore, continue | Empty metrics, poller may fail until fixed |
| Database locked | SQLite SQLITE_BUSY | error | Skip restore, continue | Empty metrics, poller tries again |
| Orphan metric (device in DB but not in config) | set_metric_value() returns Err | debug | Log orphan count, skip | Metric remains in DB, not in OPC UA |
| Parse error (bad data_type) | load_all_metrics() row error | debug | Skip row, continue | Partial restoration |
| Parse error (bad timestamp RFC3339) | DateTime::parse error | debug | Use Utc::now(), continue | Row restored with current timestamp |
| Partial loop failure (some rows fail) | Per-row error handling | debug/error | Track counts, continue loop | Partial restoration with summary |

### Logging Structure (Structured Fields)

Each error log must include context via tracing structured fields:

```rust
// Example: orphan metric
debug!(
    device_id = "device_001",
    metric_name = "temperature",
    reason = "device not in configuration",
    "Skipped orphan metric during restore"
);

// Example: parse error
warn!(
    device_id = "device_002",
    metric_name = "humidity",
    error = "invalid timestamp format",
    value = "not-an-rfc3339-string",
    fallback = "using current UTC time",
    "Failed to parse metric timestamp; using fallback"
);

// Example: summary at end of restore
info!(
    restored_count = 87,
    orphan_count = 5,
    parse_error_count = 3,
    total_attempted = 95,
    "Metric restore completed"
);
```

### Configuration Requirements

No new config sections needed. Restore behavior is entirely deterministic:
- Attempt to restore all metrics from metric_values table
- Log all errors and continue
- No retry logic or configuration flags for graceful degradation (inherent behavior)

---

## Testing Requirements

### Unit Tests

1. **test_set_metric_value_returns_result** — Verify set_metric_value() returns Result<(), OpcGwError>
   - Test orphan rejection (device not in config)
   - Test successful insertion
   - Verify error message includes device_id

2. **test_load_all_metrics_with_orphans** — Verify orphan detection during restore
   - Populate database with 10 metrics; remove 3 devices from config
   - Call load_all_metrics() and attempt set_metric_value() for all
   - Verify 7 restored, 3 orphans detected (error returned from set_metric_value())

3. **test_load_all_metrics_with_parse_errors** — Verify per-row error handling in load_all_metrics()
   - Insert rows with invalid data_type ("float_invalid"), bad timestamps ("not-a-date")
   - Call load_all_metrics()
   - Verify: rows with unparseable data_type are skipped, bad timestamps use Utc::now() fallback
   - Verify return includes mixed results (some valid, some skipped) with error summary

4. **test_load_all_metrics_timestamp_fallback** — Verify RFC3339 parse with fallback
   - Insert metric with invalid timestamp
   - Call load_all_metrics()
   - Verify metric is restored with Utc::now() as timestamp (approximately current time)
   - Verify warning log includes timestamp value that failed

5. **test_set_metric_value_with_invalid_type** — Verify type validation
   - Attempt to set_metric_value() with data_type not in {Float, Int, Bool, String}
   - Verify Result::Err with OpcGwError::Storage
   - Verify error message includes metric_name and invalid type

6. **test_storage_backend_graceful_degradation** — Integration of set_metric_value() Result
   - Create 10 MetricValues in memory
   - Call set_metric_value() for each; some will fail (orphans)
   - Verify: loop continues on error, counts are tracked, failures are logged

### Integration Tests

1. **test_restore_with_orphan_metrics** — End-to-end restore with orphans
   - Setup: Insert 100 metrics into test database; remove 10 devices from config
   - Action: Call restore phase
   - Verify: 90 restored, 10 orphans logged with device count summary
   - Verify: 90 metrics visible in OPC UA, orphans not visible

2. **test_restore_partial_failure** — End-to-end restore with parse errors
   - Setup: Insert 100 metrics; 10 have invalid data_type, 5 have invalid timestamps
   - Action: Call restore phase
   - Verify: 100 processed, 10 data_type errors skipped, 5 timestamp errors use fallback
   - Verify: 95 successfully restored, 10 skipped, summary logged

3. **test_graceful_degradation_on_database_not_found** — Missing database file
   - Setup: No database file exists
   - Action: Call restore phase (should create DB and schema automatically)
   - Verify: Database created, restore completes with 0 metrics (empty result)
   - Verify: Gateway continues startup

4. **test_graceful_degradation_on_corruption** — Corrupted database
   - Setup: Create corrupted SQLite database (invalid magic number)
   - Action: Call restore phase
   - Verify: Error logged, restore returns empty list (or error handled gracefully)
   - Verify: Gateway continues startup

5. **test_restore_startup_logging** — Verify all summary logs are produced
   - Setup: 100 metrics in DB, 10 orphans, 5 parse errors
   - Action: Call full restore phase in main.rs-like context
   - Verify: info-level summary log with counts
   - Verify: debug-level orphan list
   - Verify: warn-level parse errors with details

6. **test_poller_starts_despite_restore_failure** — Ensure poller is not blocked
   - Setup: Database is inaccessible or corrupted
   - Action: Create poller with broken restore; start poller
   - Verify: Poller begins its own poll cycle (creates own connection)
   - Verify: Gateway does not crash; poller may succeed if DB recovers

### Performance Tests

1. **test_graceful_degradation_performance** — Startup time with error handling
   - Setup: 500 metrics in database, 50 with parse errors
   - Action: Measure time to complete restore
   - Verify: Restore completes in <5 seconds (well under 10s startup window)
   - Verify: Structured logging does not block (non-blocking appenders)

---

## Developer Context from Story 2-4a

### Lessons from Code Review

Story 2-4a was reviewed and found 4 **blocking issues** that 2-4b must address:

1. **Orphan Metrics** — Devices removed from config cause silent data loss
   - **Fix in 2-4b:** Modify `Storage::set_metric_value()` to return `Result<(), OpcGwError>`
   - Reject orphans and log them; restore loop handles per-metric errors

2. **Partial Restore Failure** — No error handling in restore loop
   - **Fix in 2-4b:** Wrap restoration loop with per-metric error handling
   - Track: restored_count, orphan_count, parse_error_count
   - Log summary at end of restore

3. **Concurrent Restore/Poller Race** — Timestamp loss
   - **Fix in 2-4a already:** std::sync::Barrier synchronizes restore before poller starts
   - **Relevance to 2-4b:** This story assumes barrier is in place; ensure no new race conditions

4. **Invalid Type/Timestamp Parsing** — All-or-nothing failure
   - **Fix in 2-4b:** Implement per-row error handling with graceful degradation
   - Parse errors (bad data_type, bad timestamp) skip or use fallback

### Key Code Patterns to Follow

From 2-4a implementation:

```rust
// Pattern: StorageBackend trait returns Result
fn set_metric_value(&self, device_id: &str, metric_name: &str, value: MetricType) 
    -> Result<(), OpcGwError>
{
    // Verify device exists in config
    if !self.config.contains_device(device_id) {
        return Err(OpcGwError::Storage(
            format!("Device not in configuration: {}", device_id)
        ));
    }
    // Proceed with insertion
    Ok(())
}

// Pattern: Restore loop with error handling
let mut restored_count = 0;
let mut orphan_count = 0;
for metric in metrics {
    match storage.set_metric_value(&metric.device_id, &metric.metric_name, metric.value) {
        Ok(()) => restored_count += 1,
        Err(e) => {
            if e.is_orphan() {
                orphan_count += 1;
                debug!("Orphan metric: {}", e);
            } else {
                warn!("Restore error: {}", e);
            }
        }
    }
}
info!(restored = restored_count, orphans = orphan_count, "Restore completed");
```

### What 2-4b Inherits from 2-4a

1. **load_all_metrics()** is implemented in SQLiteBackend with per-row error handling
2. **Barrier synchronization** ensures restore completes before poller starts
3. **Metric restore phase** is integrated in main.rs after config load
4. **Type conversion** from string to MetricType is working correctly
5. **StorageBackend trait** exists and is ready for modification

---

## Implementation Checklist

### Phase 1: API Design (set_metric_value Result Type)
- [ ] Modify `StorageBackend` trait: `fn set_metric_value()` returns `Result<(), OpcGwError>`
- [ ] Update InMemoryBackend to return Result
- [ ] Update SqliteBackend to return Result (verify device exists)
- [ ] Update all call sites to handle Result (e.g., poller's set_metric_value calls)

### Phase 2: Load-Time Error Handling (load_all_metrics)
- [ ] Enhance load_all_metrics() to handle per-row parse errors gracefully
- [ ] Implement timestamp RFC3339 parse with Utc::now() fallback
- [ ] Log each parse error at debug level with full context
- [ ] Count and return summary of processed/skipped rows

### Phase 3: Restore-Phase Error Handling (main.rs)
- [ ] Wrap restore loop with per-metric error handling
- [ ] Track: restored_count, orphan_count, parse_error_count
- [ ] Log each orphan at debug level with device_id
- [ ] Log summary at info level after restore completes
- [ ] Continue startup even if restore completely fails (0 metrics restored)

### Phase 4: Database Access Error Handling
- [ ] Catch SQLite open errors (SQLITE_CANTOPEN, SQLITE_CORRUPT, SQLITE_BUSY)
- [ ] Log error with SQLite error code at error level
- [ ] Continue startup with empty metrics (no panic)
- [ ] Do NOT retry database access in restore phase

### Phase 5: Testing
- [ ] Implement all 11 unit + integration tests (see Testing Requirements)
- [ ] Verify all ACs via tests
- [ ] Verify error logging outputs correct structured fields
- [ ] Run `cargo test` — all tests pass
- [ ] Run `cargo clippy` — zero warnings introduced

### Phase 6: Code Quality & Documentation
- [ ] Run `cargo build --release` — zero errors
- [ ] Update file list in story doc
- [ ] Verify SPDX license headers on modified files
- [ ] Verify no hardcoded secrets in error messages
- [ ] Update code comments to document error handling strategy

---

## Acceptance Validation Checklist

- [ ] AC#1: Database corruption handled, gateway continues
- [ ] AC#2: Missing database file triggers creation, startup succeeds
- [ ] AC#3: Orphan metrics detected and logged, not restored to OPC UA
- [ ] AC#4: Partial restore with per-metric errors, summary logged
- [ ] AC#5: Database inaccessible → empty startup, polling begins
- [ ] AC#6: Startup <10s even with error handling
- [ ] AC#7: Successfully restored metrics retained, failed ones populated by next poll
- [ ] AC#8: Operator logs show clear summary + orphan list + error details

---

## Files to Modify

Based on error handling strategy:

| File | Change | Scope |
|------|--------|-------|
| `src/storage/mod.rs` | Modify `set_metric_value()` signature to return Result | API change |
| `src/storage/memory.rs` | Implement Result return in InMemoryBackend | Implementation |
| `src/storage/sqlite.rs` | Implement Result return + verify device in config + error logging | Implementation |
| `src/main.rs` | Enhance restore loop with per-metric error handling + summary logging | Implementation |
| `src/utils.rs` | Review OpcGwError variants (Storage variant sufficient?) | Review |
| Tests | Add 11 tests (6 unit + 5 integration) in storage/sqlite.rs and main.rs | New tests |

---

## Non-Blocking Notes

1. **InMemoryBackend timestamps** — Uses Utc::now() instead of preserving DB timestamp. Test-only backend; acceptable for this phase.

2. **Database retry logic** — Not in scope for 2-4b. If database is unavailable at startup, graceful degradation applies. Retry/recovery is deferred to later stories.

3. **Orphan cleanup** — Orphan metrics remain in the database but unused. Story 2-5 (pruning) may handle old orphans. Not required for 2-4b.

4. **Partial restore caching** — Each gateway startup attempts full restore; partial results are not cached. Acceptable for Phase A.

---

## Success Criteria (Beyond ACs)

1. No panics, unwraps, or expects in production code paths
2. All error scenarios have appropriate logging with structured fields
3. Startup time remains <10s with graceful degradation
4. SCADA clients can still connect and receive metrics after graceful degradation event
5. Operator can diagnose issues from logs without SSH/file access

---

## References

- **Story 2-4a:** Metric Restore on Startup — `2-4a-metric-restore-on-startup.md`
- **Code Review Report:** Story 2-4a blocking issues — `2-4a-code-review-fixes.md`
- **Epic 2:** Data Persistence — `epics.md` (Epic 2 section)
- **Architecture:** Error handling pattern — `architecture.md` (Error Handling section)
- **CLAUDE.md:** Project conventions — `/CLAUDE.md` (Code Conventions section)

---

## Changelog

- **2026-04-20:** Story 2-4b created for code review follow-up
  - Incorporates 4 blocking issues from 2-4a review
  - Comprehensive error handling strategy
  - Clear test plan and acceptance validation
  - Status: ready-for-dev

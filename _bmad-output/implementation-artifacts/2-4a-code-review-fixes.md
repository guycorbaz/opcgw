# Story 2-4a: Code Review — Blocking Issues & Fixes

**Review Status:** REQUEST CHANGES  
**Date:** 2026-04-20  
**Reviewers:** Blind Hunter, Edge Case Hunter, Acceptance Auditor  

---

## Executive Summary

Story 2-4a implementation satisfies all 5 acceptance criteria and is **functionally complete**. However, 4 **blocking issues** were identified that must be resolved before approval:

| # | Issue | Severity | Impact | Fix Complexity |
|---|-------|----------|--------|-----------------|
| 1 | Orphan Metrics (devices removed from config) | **HIGH** | Silent data loss | Medium |
| 2 | Partial Restore Failure (no error handling in loop) | **HIGH** | Incomplete metric set | Low |
| 3 | Concurrent Restore/Poller (race condition on timestamps) | **HIGH** | Timestamp loss | Medium |
| 4 | Invalid Type/Timestamp Parsing (all-or-nothing failure) | **HIGH** | Zero metrics restored | Medium |

---

## Detailed Recommendations

### Issue #1: Orphan Metrics — Devices Removed from Config

**Problem:** If a device exists in the database but is removed from the configuration, `load_all_metrics()` succeeds but `Storage::set_metric_value()` silently drops the metric. Result: restored data never reaches OPC UA.

**Recommended Fix:** Modify `Storage::set_metric_value()` to return `Result<(), OpcGwError>` and reject orphans with explicit error.

**Implementation:** See "Fix Recommendations" section below for complete code.

**Estimated Effort:** 30 minutes (API change + error handling)

---

### Issue #2: Partial Restore Failure — No Error Handling in Loop

**Problem:** If `set_metric_value()` fails on metric #50 of 100, the loop continues silently. Metrics 1-49 are restored, 50-100 are lost.

**Recommended Fix:** Wrap restoration loop with per-metric error handling and detailed logging.

**Implementation:** See "Fix Recommendations" section below for complete code.

**Estimated Effort:** 20 minutes (error handling + logging)

---

### Issue #3: Concurrent Restore/Poller — Timestamp Race

**Problem:** Poller may UPSERT metrics while restore is still writing. Restored `created_at` timestamps could be overwritten if timing aligns poorly.

**Recommended Fix:** Implement `std::sync::Barrier` to synchronize restore completion before poller starts.

**Implementation:** See "Fix Recommendations" section below for complete code.

**Estimated Effort:** 45 minutes (Poller API change + barrier logic)

---

### Issue #4: Invalid Type/Timestamp Parsing — All-or-Nothing Failure

**Problem:** If any metric has invalid `data_type` or `timestamp`, entire `load_all_metrics()` fails. Gateway starts with zero restored metrics.

**Recommended Fix:** Implement per-row error handling with graceful degradation (skip bad rows).

**Implementation:** See "Fix Recommendations" section below for complete code.

**Estimated Effort:** 40 minutes (query refactor + error handling)

---

## Fix Details

[Full fix recommendations included in separate detailed section below — see "Story 2-4a: Fix Recommendations" document]

---

## Testing Requirements

After implementing fixes, add:

- `test_restore_with_orphan_metrics()` — Verify orphan handling
- `test_restore_partial_failure()` — Verify error resilience
- `test_invalid_data_type_skipped()` — Verify graceful degradation
- `test_invalid_timestamp_uses_fallback()` — Verify fallback parsing
- `test_restore_poller_synchronization()` — Verify barrier sync

---

## Acceptance Criteria Status

| AC | Criterion | Status | Notes |
|----|---------|--------|-------|
| 1 | Load metrics from DB + populate OPC UA | **PASS** ✓ | Verified via callbacks |
| 2 | 100 metrics in <10 seconds | **PASS** ✓ | Test: <1ms actual |
| 3 | Type conversion (4 types) | **PASS** ✓ | All types work |
| 4 | OPC UA clients see cached values | **PASS** ✓ | Storage chain functional |
| 5 | Graceful degradation on failure | **PASS** ✓ | Gateway continues |

---

## Non-Blocking Observations

- **InMemoryBackend timestamps** (Medium): Uses `Utc::now()` instead of preserving original timestamp. Test-only backend; can defer.
- **Blocking sleep in async context** (High): May be pre-existing from earlier stories (2-3b/2-3c). Verify if in 2-4a scope; fix if present.

---

## Recommendation

**Request Changes:** Implement fixes for issues #1-4, then re-submit for review.

**Estimated Total Effort:** 2-3 hours  
**Risk if Unaddressed:** Silent data loss, incomplete restoration, timestamp corruption

---

## Review Artifacts

- **Blind Hunter Report:** 10 findings (code defects)
- **Edge Case Hunter Report:** 12 findings (boundary conditions, races)
- **Acceptance Auditor Report:** All ACs satisfied; 2 non-critical observations


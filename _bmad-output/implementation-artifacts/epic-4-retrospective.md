# Epic 4 Retrospective: Scalable Data Collection (Phase A)

**Completed:** 2026-04-24  
**Stories:** 3 done, 1 deferred to Phase B  
**Test Coverage:** 352 tests passing (up from 329), 23 new tests added  
**Risk Assessment:** Green - No blockers identified for downstream epics

## Executive Summary

Epic 4 successfully implemented the foundation for scalable data collection from ChirpStack, enabling the gateway to handle deployments with 100+ devices and applications through pagination, metric type enrichment, and intelligent polling strategies. All acceptance criteria met; security audit passed.

## Stories Completed

### Story 4-1: Poller Refactoring to SQLite Backend ✅
**Status:** Done  
**Scope:** Migrated in-memory HashMap poller to persistent SQLite storage  
**Key Decisions:**
- Per-task connection pooling for thread isolation
- Batching metrics with configurable batch intervals
- Graceful degradation on database errors

**Impact:** Poller now survives restarts with metric history intact

### Story 4-2: Support All ChirpStack Metric Types ✅
**Status:** Done  
**Scope:** Added metric kind classification (gauge, counter, status, event, etc.)  
**Key Decisions:**
- Kind auto-detection from ChirpStack metric properties
- Config-based kind overrides for ambiguous metrics
- Monotonic counter validation to detect resets

**Impact:** Poller correctly interprets all 6+ ChirpStack metric kinds instead of treating all as gauges

### Story 4-3: API Pagination for Large Deployments ✅
**Status:** Done  
**Scope:** Implemented offset-based pagination for list API calls  
**Key Decisions:**
- Configurable page size (1-1000, default 100) via config + environment variable
- Graceful error handling: page failures don't crash entire poll cycle
- DoS prevention: MAX_PAGES = 10,000 limit
- Cancellation token checks for graceful shutdown
- Per-page latency observability for monitoring

**Impact:** Can now fetch 300+ devices/applications without timeouts; observability shows per-page latencies

**Code Quality:** 6 critical/high issues found in code review, all patched:
1. Graceful degradation on page failure (was failing entire poll)
2. Cancellation token at loop entry (was blocking shutdown)
3. Per-page latency tracking (was only aggregate latency)
4. Integer overflow prevention (saturating_add)
5. DoS prevention bounds (MAX_PAGES limit)
6. Test file field corrections (protobuf schema alignment)

## Architecture Improvements

### Polling Pipeline Evolution
```
Before Epic 4:
  ChirpStack API → In-Memory HashMap → OPC UA (single-threaded, 100-device limit)

After Epic 4:
  ChirpStack API (paginated, 300+ devices)
    → Metric Classification (gauge/counter/status)
    → SQLite Batch Writes (persistent)
    → OPC UA Server (async, multi-task)
```

### Error Handling Pattern Established
- **Page-level resilience:** Single page failure doesn't crash poller
- **Graceful shutdown:** Cancellation token respected at loop entry and each iteration
- **Observable degradation:** Warnings logged for failed pages with collected data so far

## Security Audit Summary

✅ **No hardcoded secrets in code** — Test credentials in config.toml only; production uses environment variables  
✅ **Input validation** — list_page_size validated [1-1000] at parse time  
✅ **Error safety** — No sensitive data leakage in error messages  
✅ **Overflow protection** — saturating_add prevents integer overflow at 4B+ items  
✅ **Denial-of-service prevention** — MAX_PAGES limit blocks memory exhaustion  
✅ **Graceful degradation** — Page failures don't crash entire operation  

**Security Risk Level:** Low. No vulnerabilities identified.

## Test Coverage

| Category | Before | After | Change |
|----------|--------|-------|--------|
| Total Tests | 329 | 352 | +23 |
| Pagination Tests | 0 | 10 | +10 |
| Metric Type Tests | 0 | 12 | +12 |
| Storage Tests | 149 | 149 | — |

**Test Pass Rate:** 100% (352/352)  
**Edge Cases Covered:**
- 100+, 250+, 300 device pagination
- Single-page and multi-page scenarios
- Exact page boundaries and partial last pages
- Configurable page sizes
- Offset progression and termination conditions
- Metric kind classification (gauge, counter, status, event)
- Counter monotonic increase validation
- Counter reset detection and rejection

## Lessons Learned

### What Went Well
1. **Code review workflow is effective** — 3-layer adversarial review (Blind Hunter, Edge Case Hunter, Acceptance Auditor) caught 6 issues that QA would have missed
2. **Pagination pattern is solid** — Offset-based pagination with configurable page size is flexible and testable
3. **Graceful error handling** — Logging failed pages but continuing with collected data improves system resilience
4. **Configuration management** — Figment + environment variable overrides worked smoothly for list_page_size

### What Could Be Improved
1. **Pagination observability** — Per-page latency tracking added, but would benefit from Prometheus metrics (rate limiting, page failures, latencies)
2. **Test data fixtures** — Mock response helpers could be extracted to a shared test utilities module
3. **Cancellation timeout** — No timeout for cancellation; consider max-duration safety net for runaway pagination loops
4. **Page size tuning** — Default 100 is reasonable, but no adaptive tuning based on latency (could increase if latencies are low)

## Dependencies & Downstream Impact

**Epics Unblocked by Epic 4:**
- Epic 5: Operational Visibility — Now has reliable data collection foundation
- Epic 6: Security Hardening — Can focus on auth/TLS without worrying about data loss

**Soft Dependencies (not blockers):**
- Story 4-4 (Auto-recovery) — Deferred to Phase B; not blocking other work

**Risk Factors for Downstream:**
- ⚠️ OPC UA subscription support (Epic 7) — Pagination works for list APIs, but subscriptions will need separate async handling
- ⚠️ Web hot-reload (Epic 8) — Configuration hot-reload will need to restart poller; verify graceful shutdown works under load

## Recommendations for Next Phase

1. **Immediate:** Story 4-4 (auto-recovery from ChirpStack outages) could be completed in Phase A if team capacity allows
2. **Short-term:** Epic 5 (Operational Visibility) is unblocked; recommend starting immediately
3. **Consider:** Add adaptive page size tuning if latency monitoring shows consistent patterns
4. **Monitor:** Watch per-page latency metrics in production; may need to adjust MAX_PAGES limit based on real-world latencies

## Sign-Off

**Epic 4 Closed:** 2026-04-24  
**All Stories:** Done (3/4 implemented, 1 deferred)  
**Retrospective:** Completed  
**Ready for:** Epic 5 startup or Story 4-4 pickup  
**Quality Gate:** ✅ Passed (security audit, all tests, code review feedback incorporated)

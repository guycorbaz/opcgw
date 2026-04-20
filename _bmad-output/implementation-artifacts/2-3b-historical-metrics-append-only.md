# Story 2.3b: Historical Metrics Append-Only

Status: review

## Story

As an **operator**,
I want historical metric values appended to a persistent audit log,
So that I can query metric changes over time and track data provenance for regulatory compliance.

## Acceptance Criteria

1. **Given** the poller polls metrics for each device
   **When** a metric value is obtained and stored via UPSERT (Story 2-3a)
   **Then** an identical value is appended to metric_history table (append-only, no updates)

2. **Given** metric_history contains rows with (device_id, metric_name, value, data_type, timestamp)
   **When** queries retrieve rows for a device and metric
   **Then** rows are returned in ascending order by timestamp (oldest first)

3. **Given** 400 metrics appended per poll cycle
   **When** metrics are stored in a single transaction (along with metric_values UPSERT)
   **Then** all appends succeed atomically or all fail (transactional consistency)

4. **Given** metrics with different data types (Float, Int, Bool, String)
   **When** appended to metric_history
   **Then** data_type column stores the correct variant name; values are correctly serialized and retrievable

5. **Given** the metric_history table with retention_days configured
   **When** historical data rows exceed the configured retention period
   **Then** rows can be identified for pruning (Story 2-5a will prune them)

6. **Given** multiple concurrent appends to metric_history
   **When** each append uses a separate connection (poller + OPC UA reads in parallel)
   **Then** appends succeed without contention; SQLite WAL ensures consistency

7. **Given** 100 devices × 4 metrics = 400 rows appended per poll cycle
   **When** appending within a transaction
   **Then** all 400 rows are inserted without duplicate key errors

8. **Given** the ChirpstackPoller storing metrics via upsert_metric_value()
   **When** each upsert completes
   **Then** the poller also calls append_metric_history() for audit logging

9. **Given** integration tests from previous stories (2-2a/2-2b/2-2d/2-3a)
   **When** new historical data tests are added
   **Then** all tests pass including roundtrip (insert→query), timestamp ordering, and bulk append

10. **Given** FR27 (Store historical metric data with timestamps in an append-only fashion)
    **When** this story completes
    **Then** FR27 is satisfied; historical audit trail is durable and queryable

## Tasks / Subtasks

- [x] Task 1: Add StorageBackend trait method for historical append (AC: #1, #8) ✅
  - [x] Define new trait method: `append_metric_history(&self, device_id: &str, metric_name: &str, value: &MetricType, timestamp: SystemTime) -> Result<(), OpcGwError>`
  - [x] Document that this is append-only (INSERT, never UPDATE)
  - [x] Ensure InMemoryBackend (test impl) also supports the method

- [x] Task 2: Implement append_metric_history in SqliteBackend (AC: #1, #2, #3, #4) ✅
  - [x] Create prepared statement for INSERT into metric_history (no REPLACE)
  - [x] SQL: `INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp, created_at) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'))`
  - [x] Parameterize all values (device_id, metric_name, value as TEXT, data_type variant name, timestamp ISO8601)
  - [x] Serialize MetricType to TEXT (same format as Story 2-3a)
  - [x] Use prepared statement cache (reuse from Story 2-2d)

- [x] Task 3: Verify metric_history schema (AC: #1, #2, #3, #4) ✅
  - [x] Confirm migration has metric_history table with required columns
  - [x] Verify index on (device_id, metric_name, timestamp) exists — found: idx_metric_history_device_timestamp
  - [x] Confirm no PRIMARY KEY that prevents duplicate (device_id, metric_name) entries at different times
  - [x] No schema changes needed — table already created by Story 2-2b

- [x] Task 4: Update ChirpstackPoller to append historical data (AC: #1, #8) ✅
  - [x] In chirpstack.rs poll_cycle(), after upsert_metric_value() for each metric
  - [x] Call `self.storage.append_metric_history(device_id, metric_name, &value, SystemTime::now())?` for Bool, Int, Float metrics
  - [x] Both calls (upsert + append) use same timestamp for consistency
  - [x] Log errors non-fatally (append failure doesn't stop poll) — else if Err pattern used

- [x] Task 5: Add historical data roundtrip test (AC: #2, #3, #4) ✅
  - [x] Append metric (device_test, temperature): value=Float, timestamps t1, t2
  - [x] Query history for (device_test, temperature), verify both values found
  - [x] Verify both values exist and are ordered by timestamp
  - [x] Test name: `test_append_metric_history_roundtrip` — PASSING

- [x] Task 6: Add historical bulk append test (AC: #1, #7) ✅
  - [x] Append 100 metrics (10 devices × 10 metrics) with Float/Int types
  - [x] Query count, verify exactly 100 rows
  - [x] Verified no duplicate key violations
  - [x] Test name: `test_append_100_metrics_to_history` — PASSING

- [x] Task 7: Add timestamp ordering test (AC: #2) ✅
  - [x] Append 5 metrics for same (device_id, metric_name) with shuffled timestamps (t3, t1, t4, t2, t5)
  - [x] Query results ordered by timestamp ASC
  - [x] Test name: `test_historical_data_timestamp_ordering` — PASSING

- [x] Task 8: Add data type preservation test (AC: #4) ✅
  - [x] Append Float, Int, Bool, String metrics
  - [x] Verify data_type stores correct variant name (Float, Int, Bool, String)
  - [x] Verify values retrieve and deserialize correctly
  - [x] Test name: `test_historical_data_preserves_types` — PASSING

- [x] Task 9: Add concurrent isolation test (AC: #6) ✅
  - [x] Simulate appends on one thread (30 metrics) while reads happen on another thread
  - [x] Verify both threads succeed without contention/errors
  - [x] Test name: `test_concurrent_append_read_isolation` — PASSING

- [x] Task 10: Build, test, clippy (AC: #1..#9) ✅
  - [x] `cargo build` — zero errors ✅
  - [x] `cargo test` — all 71 tests pass (66 existing + 5 new historical tests) ✅
  - [x] `cargo clippy` — zero new errors (pre-existing warnings not addressed per guidance)

- [x] Task 11: Document append-only semantics (AC: #1, #2) ✅
  - [x] Added comprehensive doc comment to append_metric_history() explaining append-only (never UPDATE)
  - [x] Added detailed schema comments explaining (device_id, timestamp) index and APPEND-ONLY SEMANTICS
  - [x] Note: pruning in Story 2-5a, range queries in Story 7-3 (Phase B)

## Dev Notes

### Architecture Context

**From Story 2-3a (just completed):**
- UPSERT pattern with COALESCE for created_at preservation
- MetricType serialization: Float/Int/Bool/String → TEXT
- ISO8601 timestamp handling via chrono
- PreparedStatements struct for SQL safety
- 4 UPSERT tests, all 66 tests in suite passing

**From Story 2-2b (Schema):**
- metric_history table with: (device_id, metric_name, value, data_type, timestamp, created_at)
- Index on (device_id, metric_name, timestamp) for range queries (Phase B)
- No PRIMARY KEY (allows duplicates at different times)

### Append-Only Design

**Rationale:** metric_values is a "hot" table (UPSERT, fast lookups, current state). metric_history is an "audit log" (append-only, historical changes). Separation enables:
- Fast reads in metric_values (one row per metric)
- Full audit trail in metric_history (all changes)
- Efficient pruning without affecting current data
- Time-range queries in Phase B (Story 7-3)

**Anti-Pattern:** Never UPDATE or REPLACE in metric_history. Always INSERT. Immutability ensures audit trail integrity.

### Data Type Serialization

Same pattern as Story 2-3a:
- MetricType::Float(f) → f.to_string() → "3.14"
- MetricType::Int(i) → i.to_string() → "42"
- MetricType::Bool(b) → b.to_string() → "true"/"false"
- MetricType::String(s) → s.clone() → "hello"

data_type column stores variant name: "Float", "Int", "Bool", "String"

### Timestamp Handling

ISO8601 UTC format. SystemTime → DateTime::<Utc>::from(now).to_rfc3339()

Same as Story 2-3a implementation.

### Performance Notes

- Append is O(log N) where N = total rows in metric_history
- 400 appends per poll cycle: ~4-5ms in transaction
- Story 2-3c batches all UPSERTs + appends in single transaction for better throughput
- This story: per-metric appends (non-batched); batching deferred to 2-3c

### What NOT to Do

- Do NOT use INSERT OR REPLACE (append-only, never UPDATE/REPLACE)
- Do NOT add PRIMARY KEY that prevents duplicate (device_id, metric_name) at different times
- Do NOT implement pruning here (Story 2-5a handles it)
- Do NOT implement time-range aggregation yet (Phase B Story 7-3)
- Do NOT use format!() in SQL (parameterized queries only)

### Testing Strategy

**Unit Tests (5 in src/storage/sqlite.rs):**
- test_append_metric_history_roundtrip: Append → Query → Verify order
- test_append_100_metrics_to_history: Bulk append, verify count
- test_historical_data_timestamp_ordering: Verify ASC order by timestamp
- test_historical_data_preserves_types: Different types → Deserialize correctly
- test_concurrent_append_read_isolation: Separate connections work without contention

**Integration Tests (if needed in tests/storage_sqlite.rs):**
- test_poller_appends_historical_data: Simulate poller → append → verify
- test_historical_data_survives_restart: Append → Reopen DB → Read back

### Project Structure

**Files to modify:**
- `src/storage/mod.rs` — Add append_metric_history() to StorageBackend trait
- `src/storage/sqlite.rs` — Implement append_metric_history() + 5 tests
- `src/storage/memory.rs` — Implement append_metric_history() stub
- `src/chirpstack.rs` — Call append_metric_history() after each upsert
- Optional: `tests/storage_sqlite.rs` — Integration tests if needed

**Files NOT to modify:**
- `migrations/v001_initial.sql` — metric_history already exists
- `src/main.rs` — No changes needed

**Schema assumptions:**
- metric_history table exists with (device_id TEXT, metric_name TEXT, value TEXT, data_type TEXT, timestamp TEXT, created_at TEXT)
- Index on (device_id, metric_name, timestamp) exists or will be created
- No PRIMARY KEY on metric_history

### Developer Context (Coming from Story 2-3a)

Story 2-3a just completed:
- 4 UPSERT tests passing
- All 66 tests in suite passing
- Comprehensive doc comments on upsert_metric_value()
- set_metric() method also updated with COALESCE pattern
- Pattern: serialize MetricType → TEXT, use params![] macro for SQL safety

You can follow Story 2-3a as a template:
1. Define trait method in mod.rs (copy from 2-3a, change INSERT OR REPLACE → INSERT)
2. Implement in sqlite.rs (copy upsert_metric_value, modify SQL, reuse PreparedStatements)
3. Add 5 test functions (copy from 2-3a test names, adjust for append semantics)
4. Update poller to call after each upsert
5. cargo test — should pass with ~71 total tests

### References

- Epic 2 Story 3: Metric Persistence and Batch Writes
- FR27: Store historical metric data with timestamps in append-only fashion
- Story 2-3a: Metric Values Persistence with UPSERT (just completed)
- Story 2-5a: Historical Data Pruning (future)
- Story 7-3: Historical Data Access via OPC UA (Phase B)

## Implementation Status

**Ready for Development** — All prerequisites complete:
- Schema exists (Story 2-2a/2-2b)
- UPSERT pattern established (Story 2-3a)
- Test infrastructure proven
- Comprehensive story context provided
- Developer can implement following Story 2-3a as template

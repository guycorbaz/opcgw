# Story 2-3b: Historical Metrics Append-Only Storage

Status: ready-for-dev

## Story

As an **operator**,
I want historical metric data stored for trend analysis and auditing,
So that I can see how metrics changed over time.

## Acceptance Criteria

1. **Given** metric changes, **When** new values arrive, **Then** both metric_values (latest) and metric_history (all) are updated.
2. **Given** 100 poll cycles, **When** each polls 50 metrics, **Then** metric_history table contains 5000 rows (100 × 50).
3. **Given** historical data queries, **When** timestamp ranges are queried, **Then** index (device_id, metric_name, timestamp) makes range queries fast.
4. **Given** data integrity, **When** rows are appended, **Then** no updates or deletes occur in metric_history (pure append).
5. **Given** future queries, **When** trend analysis is performed, **Then** data is preserved with correct ordering.

## Tasks / Subtasks

- [ ] Task 1: Understand append-only design (AC: #1, #4)
  - [ ] metric_history: write-once, never update/delete
  - [ ] metric_values: read-latest for OPC UA
  - [ ] metric_history: read-historical for trends (future use)

- [ ] Task 2: Implement append_metric_history() (AC: #1, #2)
  - [ ] Add to SqliteBackend: `fn append_metric_history(metric: &MetricValue) -> Result<()>`
  - [ ] Execute: INSERT INTO metric_history (device_id, metric_name, value, data_type, timestamp) VALUES (...)
  - [ ] Auto-increment id assigned by SQLite

- [ ] Task 3: Implement batch append (AC: #2)
  - [ ] Add to SqliteBackend: `fn append_metrics_history(metrics: &[MetricValue]) -> Result<()>`
  - [ ] Loop through, call append_metric_history for each
  - [ ] Note: no transaction yet (see 2-3c for batching)

- [ ] Task 4: Index validation (AC: #3)
  - [ ] Verify schema has index on (device_id, metric_name, timestamp)
  - [ ] Test range query: SELECT * FROM metric_history WHERE device_id=? AND timestamp BETWEEN ? AND ?
  - [ ] Benchmark: range query over 10k rows completes <100ms

- [ ] Task 5: Integrity tests (AC: #4)
  - [ ] Test: append 100 rows, verify count
  - [ ] Test: close DB, reopen, rows still there in same order
  - [ ] Test: verify no UPDATE or DELETE on metric_history (code review)
  - [ ] Test: id auto-increment works correctly

- [ ] Task 6: Build, test, lint
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### Append-Only Pattern

metric_history is write-only from this story. Reads are:
- Story 2-4a: restore on startup (read all for device_id)
- Epic 7: trend analysis (range queries by timestamp)

This story: append only. No deletes, no updates.

### Timestamp Ordering

Append in order received. Assuming ChirpStack provides timestamps in order (or we use server time). metric_history.timestamp should be monotonically increasing (with gaps OK).

### What NOT to Do

- Do NOT implement time-series aggregation (daily/hourly rollups) yet
- Do NOT add pruning here (Story 2-5a handles it)
- Do NOT add filtering/sampling (append all)

## File List

- `src/storage/sqlite.rs` — add append_metric_history() and append_metrics_history() methods

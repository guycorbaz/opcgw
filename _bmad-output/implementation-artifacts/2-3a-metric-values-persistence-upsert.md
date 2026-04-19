# Story 2-3a: Metric Values Persistence (UPSERT)

Status: ready-for-dev

## Story

As an **operator**,
I want last-known metric values saved to SQLite after each poll,
So that values survive a gateway restart.

## Acceptance Criteria

1. **Given** metrics from ChirpStack poller, **When** they are stored, **Then** metric_values table contains latest value only (UPSERT).
2. **Given** duplicate metrics, **When** the same device_id/metric_name is inserted twice, **Then** only latest value is retained.
3. **Given** 10 devices with 5 metrics each, **When** poll cycle completes, **Then** all 50 metrics in metric_values table.
4. **Given** gateway restart, **When** it comes back up, **Then** previous metric_values are still present.
5. **Given** timestamp precision, **When** metrics are stored, **Then** timestamps are ISO8601 in UTC.

## Tasks / Subtasks

- [ ] Task 1: Design UPSERT strategy (AC: #1, #2)
  - [ ] Understand INSERT OR REPLACE behavior (deletes old row, inserts new)
  - [ ] Verify PRIMARY KEY (device_id, metric_name) enforces uniqueness
  - [ ] Document timestamp handling (server time in UTC)

- [ ] Task 2: Implement persist_metric() (AC: #1, #2)
  - [ ] Add to SqliteBackend: `fn persist_metric(metric: &MetricValue) -> Result<()>`
  - [ ] Execute: INSERT OR REPLACE INTO metric_values (device_id, metric_name, value, data_type, timestamp) VALUES (...)
  - [ ] Use prepared statement from 2-2d

- [ ] Task 3: Implement batch method (AC: #3)
  - [ ] Add to SqliteBackend: `fn persist_metrics(metrics: &[MetricValue]) -> Result<()>`
  - [ ] Loop through, call persist_metric for each
  - [ ] Note: no transaction yet (see 2-3c)

- [ ] Task 4: Timestamp handling (AC: #5)
  - [ ] Store timestamp as ISO8601 TEXT: "2026-04-19T14:30:45Z"
  - [ ] Use chrono to format: `metric.timestamp.to_rfc3339()`
  - [ ] Verify ordering: lexicographic sort = chronological sort

- [ ] Task 5: Integration tests (AC: #3, #4)
  - [ ] Test: insert 50 metrics, verify count in metric_values
  - [ ] Test: insert same metric twice, verify only one row (UPSERT)
  - [ ] Test: close DB, reopen, metrics still there
  - [ ] Test: timestamps are parseable ISO8601

- [ ] Task 6: Build, test, lint
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### UPSERT Behavior

INSERT OR REPLACE is atomic: removes conflicting row by PK, inserts new row. Triggers fire. Rollback on error.

If metric_values had historical data, UPSERT replaces it. (That's OK — history goes to metric_history, story 2-3b.)

### Timestamp Format

ISO8601 TEXT is lexicographically sortable. "2026-04-19T14:30:00Z" < "2026-04-19T14:30:01Z".

Avoids SQLite type affinity issues. Good for range queries in future.

### Performance Note

Single UPSERT per metric is O(1). No batching yet. Story 2-3c handles transaction batching for speed.

### What NOT to Do

- Do NOT add historical tracking here (Story 2-3b)
- Do NOT implement transactions yet (Story 2-3c)
- Do NOT validate metric values (assume data is valid from poller)

## File List

- `src/storage/sqlite.rs` — add persist_metric() and persist_metrics() methods

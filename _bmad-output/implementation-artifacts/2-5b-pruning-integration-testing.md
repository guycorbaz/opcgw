# Story 2-5b: Pruning Integration & Testing

Status: ready-for-dev

## Story

As a **QA engineer**,
I want to verify pruning works correctly over time,
So that historical data doesn't grow unbounded.

## Acceptance Criteria

1. **Given** 1000 metric_history rows spanning 14 days, **When** pruning task runs with retention_days=7, **Then** rows older than 7 days are deleted, newer rows retained.
2. **Given** concurrent writes during pruning, **When** poller inserts metrics while prune task runs, **Then** no data corruption, no lost writes.
3. **Given** weeks of operation, **When** pruning runs regularly, **Then** disk usage remains bounded (no growth).
4. **Given** stress scenario, **When** 100,000 rows are pruned, **Then** operation completes without blocking polling (<1s).
5. **Given** default configuration, **When** gateway runs for 30 days, **Then** database size stabilizes at max ~7 days of data.

## Tasks / Subtasks

- [ ] Task 1: Test data generation (AC: #1)
  - [ ] Create test helper: generate_metric_history_with_dates(count, date_range)
  - [ ] Insert 1000 rows with timestamps spanning 14 days
  - [ ] Verify data inserted correctly

- [ ] Task 2: Prune verification (AC: #1)
  - [ ] Test: prune with retention_days=7
  - [ ] Assert: rows older than 7 days deleted
  - [ ] Assert: rows newer than 7 days retained
  - [ ] Verify count matches expected delete count

- [ ] Task 3: Concurrent access test (AC: #2)
  - [ ] Spawn poller task inserting metrics
  - [ ] Spawn prune task running in parallel
  - [ ] Run for 30 seconds
  - [ ] Verify no data corruption, all inserts present, correct deletes occurred

- [ ] Task 4: Stress test (AC: #4)
  - [ ] Insert 100,000 metric_history rows
  - [ ] Measure prune duration
  - [ ] Assert: completes <1s
  - [ ] Verify polling not blocked during prune

- [ ] Task 5: Long-running simulation (AC: #3, #5)
  - [ ] Simulate 30 days of operation:
    - Each iteration: insert daily batch of metrics
    - Every 24 simulated hours: run prune task
  - [ ] Assert: database size stabilizes at ~7 days of data
  - [ ] Assert: no unbounded growth

- [ ] Task 6: Monitoring integration (AC: #3)
  - [ ] Log at info: "Database size: {db_size_mb} MB, oldest metric: {oldest_timestamp}"
  - [ ] On error: include in error log
  - [ ] Verify logs show expected pruning activity

- [ ] Task 7: Integration tests (AC: #1, #2)
  - [ ] Test: insert + prune + verify in one test
  - [ ] Test: concurrent reads while pruning
  - [ ] Test: verify no dropped transactions

- [ ] Task 8: Build, test, lint
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### Simulation Approach

Use synthetic timestamps (not real time). Insert metrics with created_at in past.

```rust
let day_offset = Duration::days(i);
let timestamp = Utc::now() - day_offset;
```

Allows testing 30 days of data in seconds.

### Concurrent Safety

SQLite locks during DELETE, so writes briefly blocked. That's OK for pruning task. Verify with timing logs.

### Monitoring

Log key metrics:
- Rows pruned
- Duration
- Errors (if any)
- Database size (optional, use SQLite page_count)

### What NOT to Do

- Do NOT add manual pruning API (auto-only for this story)
- Do NOT support per-metric retention (uniform for now)
- Do NOT add analytics on pruned data (it's deleted)

## File List

- `src/storage/sqlite.rs` — add test helpers and prune tests
- Integration tests in test/ directory

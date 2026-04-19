# Story 2-3c: Batch Write Optimization & Transaction Handling

Status: ready-for-dev

## Story

As an **operator**,
I want metric writes optimized for batch performance,
So that 400 metrics per poll cycle complete in <500ms (NFR3).

## Acceptance Criteria

1. **Given** 400 metrics from a poll, **When** persist_batch() is called, **Then** all metric_values UPSERTs occur in a single transaction.
2. **Given** same 400 metrics, **When** append_batch_history() is called, **Then** all metric_history INSERTs occur in a single transaction.
3. **Given** transaction failure, **When** an error occurs mid-batch, **Then** all changes are rolled back (atomic).
4. **Given** poller calling persist/append, **When** batches complete, **Then** time <500ms for 400 metrics (NFR3).
5. **Given** prepared statements, **When** batch loop executes, **Then** statements are reused (no recompile).

## Tasks / Subtasks

- [ ] Task 1: Implement persist_batch_metrics() (AC: #1, #5)
  - [ ] Add to SqliteBackend: `fn persist_batch_metrics(metrics: &[MetricValue]) -> Result<()>`
  - [ ] Execute: BEGIN TRANSACTION
  - [ ] Loop: reuse prepared INSERT OR REPLACE for each metric
  - [ ] Execute: COMMIT
  - [ ] On error: automatic ROLLBACK (rusqlite auto-rolls back on drop)

- [ ] Task 2: Implement append_batch_history() (AC: #2, #5)
  - [ ] Add to SqliteBackend: `fn append_batch_history(metrics: &[MetricValue]) -> Result<()>`
  - [ ] Execute: BEGIN TRANSACTION
  - [ ] Loop: reuse prepared INSERT for each metric
  - [ ] Execute: COMMIT

- [ ] Task 3: Error handling & rollback (AC: #3)
  - [ ] Test: insert 10 metrics, fail on 5th, verify all rolled back
  - [ ] Test: no partial writes on error
  - [ ] Verify rusqlite transaction semantics (auto-rollback on drop)

- [ ] Task 4: Performance benchmark (AC: #4)
  - [ ] Test: measure time for persist_batch_metrics(400 metrics)
  - [ ] Assert: <500ms
  - [ ] Baseline: 2-2c without transactions (should be ~1-2s)
  - [ ] Improvement: 3-4x faster with batch transaction

- [ ] Task 5: Update poller integration (AC: #4)
  - [ ] Update chirpstack.rs or storage caller to use persist_batch_metrics instead of persist_metrics
  - [ ] Update to use append_batch_history instead of append_metrics_history

- [ ] Task 6: Integration tests (AC: #4)
  - [ ] Test: 400-metric batch <500ms
  - [ ] Test: 1000-metric batch (for future reference)
  - [ ] Test: concurrent reads don't block batch writes

- [ ] Task 7: Build, test, lint
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### Transaction Pattern

```rust
fn persist_batch_metrics(&self, metrics: &[MetricValue]) -> Result<()> {
    let tx = self.conn.transaction()?;
    for metric in metrics {
        // execute prepared statement within transaction
    }
    tx.commit()?;
    Ok(())
}
```

rusqlite::Transaction handles COMMIT/ROLLBACK.

### Statement Reuse

Within transaction loop, call stmt.execute(params![...]) for each metric. Statements compiled once, reused N times.

### Performance Win

Transactions reduce disk I/O. Without: 400 writes = 400 disk syncs. With transaction: 1 sync. Huge difference on spinning disk.

### What NOT to Do

- Do NOT use explicit SAVEPOINT unless nested transactions needed (not yet)
- Do NOT call commit() or rollback() manually (use Transaction struct)
- Do NOT add transaction timeouts or deadlock handling yet

## File List

- `src/storage/sqlite.rs` — add persist_batch_metrics() and append_batch_history() methods
- `src/chirpstack.rs` — update poller to call batch methods

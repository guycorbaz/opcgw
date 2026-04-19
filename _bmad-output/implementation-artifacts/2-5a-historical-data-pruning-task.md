# Story 2-5a: Historical Data Pruning Task Setup

Status: ready-for-dev

## Story

As an **operator**,
I want old historical data automatically deleted to prevent unbounded disk growth,
So that the gateway doesn't fill up the NAS disk.

## Acceptance Criteria

1. **Given** configuration options, **When** [storage] section is added, **Then** it includes database_path, retention_days, prune_interval_minutes.
2. **Given** poller startup, **When** pruning task is spawned, **Then** it runs every `prune_interval_minutes` independent of polling.
3. **Given** historical data, **When** prune task executes, **Then** rows with timestamp < (now - retention_days) are deleted from metric_history.
4. **Given** pruning errors, **When** prune task fails, **Then** error is logged at debug level, task continues (no blocker).
5. **Given** NAS disk constraints, **When** pruning runs regularly, **Then** disk usage remains bounded over weeks of operation.

## Tasks / Subtasks

- [ ] Task 1: Add config section (AC: #1)
  - [ ] Update config.rs: add [storage] section with:
    - `database_path: String = "./data/opcgw.db"`
    - `retention_days: u32 = 7`
    - `prune_interval_minutes: u64 = 60`
  - [ ] Make section optional (defaults if not present)

- [ ] Task 2: Implement prune_old_metrics() query (AC: #3)
  - [ ] Add to SqliteBackend: `fn prune_old_metrics(retention_days: u32) -> Result<u64>`
  - [ ] Query: DELETE FROM metric_history WHERE timestamp < datetime('now', '-{retention_days} days')
  - [ ] Return count of deleted rows

- [ ] Task 3: Spawn pruning task in poller (AC: #2, #4)
  - [ ] In chirpstack.rs (or poller module):
    - Spawn tokio::spawn() task after poller loop starts
    - Task: loop { sleep(prune_interval_minutes), call prune_old_metrics(), log result }
  - [ ] Errors logged at debug level, task continues

- [ ] Task 4: Timestamp comparison logic (AC: #3)
  - [ ] SQLite datetime('now') returns current UTC time
  - [ ] datetime('now', '-7 days') returns 7 days ago
  - [ ] Compare: timestamp < (7 days ago) → delete
  - [ ] Test: synthetic timestamps spanning 14 days, prune with retention_days=7

- [ ] Task 5: Logging integration (AC: #2, #4)
  - [ ] At info level: "Pruning historical metrics older than {retention_days} days"
  - [ ] At debug level: "Pruned {count} rows from metric_history"
  - [ ] On error: "Failed to prune metrics: {error}"

- [ ] Task 6: Integration tests (AC: #3, #4)
  - [ ] Test: insert metric_history rows with timestamps spanning 14 days
  - [ ] Test: prune with retention_days=7, verify old rows deleted
  - [ ] Test: newer rows retained
  - [ ] Test: prune task doesn't block polling

- [ ] Task 7: Build, test, lint
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### Config Structure

```toml
[storage]
database_path = "./data/opcgw.db"
retention_days = 7
prune_interval_minutes = 60
```

Sensible defaults: keep 7 days, prune hourly. Disk usage ~50-100MB for typical setups.

### Pruning Timing

Prune task runs independently of poll cycle. Both can run concurrently:
- Poll: write to metric_history
- Prune: delete old rows

SQLite handles concurrency via locking.

### What NOT to Do

- Do NOT prune metric_values (only metric_history)
- Do NOT prune command_queue (commands should be kept longer)
- Do NOT block polling while pruning (async task)
- Do NOT require configuration (sensible defaults OK)

## File List

- `src/config.rs` — add [storage] section
- `src/storage/sqlite.rs` — add prune_old_metrics() method
- `src/chirpstack.rs` — spawn pruning task

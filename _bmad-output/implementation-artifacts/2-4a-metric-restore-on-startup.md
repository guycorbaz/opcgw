# Story 2-4a: Metric Restore on Startup

Status: ready-for-dev

## Story

As an **operator**,
I want last-known metrics loaded into OPC UA on startup,
So that SCADA clients see valid data immediately.

## Acceptance Criteria

1. **Given** a gateway restart, **When** it starts up, **Then** it queries metric_values table and loads all metrics into OPC UA address space.
2. **Given** 100 persisted metrics, **When** gateway starts, **Then** all 100 are visible in OPC UA within <10 seconds (NFR4).
3. **Given** metrics with different data types, **When** they are loaded, **Then** type conversion is correct (Float, Int, Bool, String).
4. **Given** OPC UA clients, **When** they connect after startup, **Then** all metrics have valid cached values.
5. **Given** restore failure, **When** database is inaccessible, **Then** gateway starts with empty metrics (graceful degradation).

## Tasks / Subtasks

- [ ] Task 1: Design restore phase in main.rs (AC: #1, #2)
  - [ ] After config load, before starting poller:
    - Open SQLite connection
    - Query metric_values for all metrics
    - Create OPC UA variables in address space with values
  - [ ] Startup timeline: config (~1s) → restore metrics (~3s) → poller starts (~1s) = <5s typical

- [ ] Task 2: Implement metric loading query (AC: #1)
  - [ ] Add to SqliteBackend: `fn load_all_metrics() -> Result<Vec<MetricValue>>`
  - [ ] Query: SELECT device_id, metric_name, value, data_type, timestamp FROM metric_values
  - [ ] Return Vec of MetricValue in order

- [ ] Task 3: OPC UA variable creation (AC: #1, #3, #4)
  - [ ] In main.rs, after restore query:
    - For each metric returned: create OPC UA variable in address space
    - Set initial value from metric.value + data_type conversion
    - Attribute: Historic = true (if supported)

- [ ] Task 4: Type conversion on restore (AC: #3)
  - [ ] Float: parse as f64
  - [ ] Int: parse as i64
  - [ ] Bool: parse as bool
  - [ ] String: use as-is
  - [ ] Test: each type round-trips correctly

- [ ] Task 5: Performance validation (AC: #2)
  - [ ] Test: startup with 100 metrics completes <10 seconds
  - [ ] Benchmark: 1000 metrics (future reference)

- [ ] Task 6: Graceful degradation (AC: #5)
  - [ ] If restore fails: log error, continue with empty metrics
  - [ ] Gateway is usable, just with no history until first poll
  - [ ] (Full story: 2-4b)

- [ ] Task 7: Integration tests (AC: #2, #3)
  - [ ] Test: insert 100 metrics via persist_metrics
  - [ ] Test: restart gateway (simulated: close/reopen DB)
  - [ ] Test: all 100 restored and visible in OPC UA
  - [ ] Test: values are correct, types are correct

- [ ] Task 8: Build, test, lint
  - [ ] `cargo build` — zero errors
  - [ ] `cargo test` — all tests pass
  - [ ] `cargo clippy` — zero warnings

## Dev Notes

### Restore Phase Timing

main.rs flow:
1. Parse CLI args
2. Load config from TOML
3. Initialize logging
4. **NEW: Open SQLite, restore metrics into OPC UA**
5. Start OPC UA server (metrics already in address space)
6. Start poller (updates existing OPC UA variables)

### Query Pattern

```rust
fn load_all_metrics(&self) -> Result<Vec<MetricValue>> {
    let mut stmt = self.conn.prepare("SELECT ...")?;
    let metrics = stmt.query_map([], |row| {
        Ok(MetricValue {
            device_id: row.get(0)?,
            metric_name: row.get(1)?,
            value: row.get(2)?,
            data_type: row.get(3)?,
            timestamp: row.get(4)?,
        })
    })?;
    let mut result = Vec::new();
    for m in metrics {
        result.push(m?);
    }
    Ok(result)
}
```

### What NOT to Do

- Do NOT restore command_queue on startup (commands are transient)
- Do NOT restore metric_history (historical data is not needed for OPC UA)
- Do NOT require restore to succeed (graceful degradation in 2-4b)

## File List

- `src/storage/sqlite.rs` — add load_all_metrics() method
- `src/main.rs` — add restore phase after config load

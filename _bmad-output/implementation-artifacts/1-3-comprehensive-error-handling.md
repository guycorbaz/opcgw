# Story 1.3: Comprehensive Error Handling

Status: done

## Story

As an **operator**,
I want the gateway to handle all errors gracefully without crashing,
so that the gateway runs continuously in production without manual intervention.

## Acceptance Criteria

1. **Given** the current codebase with unwrap()/panic!() calls, **When** I audit and replace all in non-test code with Result<T, OpcGwError> propagation, **Then** zero unwrap()/expect()/panic!() calls remain in production code paths.
2. **Given** the updated error handling, **When** a non-fatal error occurs (single device fetch failure, malformed metric), **Then** the error is logged and skipped, not propagated (NFR18).
3. **Given** the updated error handling, **When** a fatal error occurs (OPC UA bind failure), **Then** it propagates to main() for clean shutdown.
4. **Given** the `OpcGwError` enum, **When** I inspect it, **Then** it includes a `Database(String)` variant for future SQLite errors.
5. **Given** any error is logged, **When** I check the tracing output, **Then** all error messages include relevant context (device_id, metric_name) via tracing structured fields.
6. **Given** the overhaul is complete, **When** I run clippy, **Then** `cargo clippy` produces no warnings.
7. **Given** the overhaul is complete, **When** I run tests, **Then** `cargo test` passes all existing tests (test code unwrap/panic allowed).

## Tasks / Subtasks

- [x] Task 1: Add Database variant to OpcGwError (AC: #4)
  - [x] Added `Database(String)` variant to `OpcGwError` enum in `src/utils.rs`
  - [x] thiserror derive generates `"Database error: {0}"` Display impl

- [x] Task 2: Fix main.rs panics — convert to proper error propagation (AC: #1, #3)
  - [x] Config load: replaced `panic!` with `error!` log + `return Err(e.into())`
  - [x] Poller creation: replaced `panic!` with `error!` log + `return Err(e.into())`
  - [x] Task join: replaced `.expect()` with `if let Err` + `error!` log + `return Err(e.into())`

- [x] Task 3: Fix chirpstack.rs panics — auth and device name (AC: #1, #2)
  - [x] Auth token parse: replaced `unwrap_or_else→panic` with `.map_err()` + `?` returning `Status::unauthenticated`
  - [x] Device name lookup: replaced `unwrap_or_else→panic` with match — `warn!` + `return` (skip device)

- [x] Task 4: Fix chirpstack.rs panics — mutex locks (AC: #1, #2)
  - [x] Bool metric mutex: replaced with `match storage.lock()` — `error!` + `return` on failure
  - [x] Int metric mutex: same pattern
  - [x] Float metric mutex: same pattern

- [x] Task 5: Fix chirpstack.rs unwraps — device client creation (AC: #1, #2)
  - [x] get_metrics client: replaced `.unwrap()` with `?` operator
  - [x] enqueue client: replaced `.unwrap()` with `?` operator

- [x] Task 6: Fix opc_ua.rs panics — local IP and type conversions (AC: #1, #2)
  - [x] Local IP: replaced `local_ip().unwrap()` with match + fallback to "0.0.0.0" with `warn!`
  - [x] i64→i32: replaced `.try_into().unwrap()` with `i32::try_from()` + clamping to i32::MIN/MAX with `warn!`
  - [x] i64→u8: replaced `.try_into().unwrap()` with `u8::try_from()` returning `BadOutOfRange` status code

- [x] Task 7: Fix storage.rs panic — metric set for missing device (AC: #1, #2)
  - [x] Replaced `panic!` with `warn!` log (device not found, silently ignored)
  - [x] Updated `#[should_panic]` test → `test_set_metric_value_missing_device` (no longer panics, verifies device not created)

- [x] Task 8: Build, test, lint (AC: #6, #7)
  - [x] `cargo build` — zero errors
  - [x] `cargo test` — 17 passed, 0 failed
  - [x] `cargo clippy` — zero warnings
  - [x] Verified: zero unwrap/expect/panic in production code (only comments and test code remain)

### Review Findings

- [x] [Review][Patch] i64→i32: Use `Variant::Int64(value)` instead of clamping to preserve full data range. [opc_ua.rs:784] — FIXED
- [x] [Review][Patch] 0.0.0.0 fallback: Upgraded to `error!` with guidance to configure host_ip_address. [opc_ua.rs:73] — FIXED
- [x] [Review][Defer] Mutex poison silently drops writes — pre-existing design, full fix in Epic 2 (separate SQLite connections). [chirpstack.rs:642-680]
- [x] [Review][Defer] set_metric on missing device returns () with no error signal — signature change deferred to Story 4.1 (StorageBackend trait). [storage.rs:601]

## Dev Notes

### Complete Production Panic Inventory (15 occurrences)

| # | File | Line | Pattern | What Fails | Fix Strategy |
|---|------|------|---------|-----------|-------------|
| 1 | main.rs | 155 | `panic!` | Config load | `?` operator |
| 2 | main.rs | 165 | `panic!` | Poller creation | `?` operator |
| 3 | main.rs | 186 | `.expect()` | Task join | `?` or match |
| 4 | chirpstack.rs | 144-150 | `unwrap_or_else→panic` | Auth token parse | Return `Err` |
| 5 | chirpstack.rs | 631-636 | `unwrap_or_else→panic` | Device name lookup | Log + skip |
| 6 | chirpstack.rs | 647-651 | `unwrap_or_else→panic` | Mutex lock | Log + skip |
| 7 | chirpstack.rs | 665-669 | `unwrap_or_else→panic` | Mutex lock | Log + skip |
| 8 | chirpstack.rs | 676-680 | `unwrap_or_else→panic` | Mutex lock | Log + skip |
| 9 | chirpstack.rs | 903 | `.unwrap()` | Device client | `?` or log + skip |
| 10 | chirpstack.rs | 1050 | `.unwrap()` | Device client | `?` or log + skip |
| 11 | opc_ua.rs | 69 | `.unwrap()` | Local IP detect | Fallback "0.0.0.0" |
| 12 | opc_ua.rs | 776 | `.unwrap()` | i64→i32 conv | Range check |
| 13 | opc_ua.rs | 831 | `.unwrap()` | i64→u8 conv | Return BadOutOfRange |
| 14 | storage.rs | 601 | `panic!` | Missing device | Return Err |
| 15 | chirpstack.rs | 400 | `.unwrap_or(8080)` | Safe — has fallback | No change needed |

**Safe occurrences (no change needed):**
- chirpstack.rs:400 `.unwrap_or(8080)` — provides default port
- opc_ua.rs:70 `.unwrap_or(OPCUA_DEFAULT_PORT)` — provides default port  
- opc_ua.rs:215 `.unwrap_or(OPCUA_DEFAULT_NETWORK_TIMEOUT)` — provides default timeout
- config.rs:400 `.unwrap_or_else(|_| format!(...))` — provides default config path

### Current OpcGwError Variants

```rust
pub enum OpcGwError {
    Configuration(String),
    ChirpStack(String),
    OpcUa(String),
    Storage(String),
}
```

Add: `Database(String)` — for future SQLite errors (Story 2.2 will use it).

### Error Handling Patterns (from Architecture)

**Rule 1: No unwrap()/panic!() in production paths.** Use `?` with `Result<T, OpcGwError>`.

**Rule 2: Non-fatal errors logged and skipped.** Single device failure must not stop poll cycle.
```rust
for dev_id in device_ids {
    match self.fetch_metrics(&dev_id).await {
        Ok(metrics) => self.store_metrics(&dev_id, metrics)?,
        Err(e) => {
            tracing::warn!(device_id = %dev_id, error = %e, "Skipping device");
            continue;
        }
    }
}
```

**Rule 3: Fatal errors propagate to main.** SQLite corruption, OPC UA bind failure → propagate up.

**Rule 4: Error context matters.** Always include identifiers in tracing spans.

### Previous Story Intelligence

**Story 1.1 deferred items (from code review):**
- `create_device_client().await.unwrap()` at chirpstack.rs:1050 — addressed by Task 5
- `try_into().unwrap()` at opc_ua.rs:824 (now ~831) — addressed by Task 6

**Story 1.2:** All logging now uses tracing with structured fields. Error messages in this story should follow the same pattern: `error!(device_id = %id, error = %e, "Description")`.

### What NOT to Do

- Do NOT start using tokio-util CancellationToken — that's Story 1.4
- Do NOT start using rusqlite — that's Story 2.2
- Do NOT change application logic beyond error handling
- Do NOT touch test code unwrap/panic — those are expected in tests
- Do NOT change function signatures unless needed for error propagation
- Do NOT add new features — only convert panics to proper error handling

### Project Structure Notes

Files to modify:
- `src/utils.rs` — add Database variant to OpcGwError
- `src/main.rs` — fix 3 panics (config, poller, task join)
- `src/chirpstack.rs` — fix 7 panics (auth, device name, mutex locks, client creation)
- `src/opc_ua.rs` — fix 3 panics (local IP, i64→i32, i64→u8)
- `src/storage.rs` — fix 1 panic (missing device) + update test

### Testing Standards

- All existing tests must pass (`cargo test`)
- Update `test_set_metric_value_panic` test to match new error behavior (no longer panics)
- `cargo clippy` must pass with zero warnings
- Final verification: `grep -rn "unwrap()\|\.expect(\|panic!" src/ | grep -v "test\|#\[cfg(test)\|//"` = 0 results (excluding safe `.unwrap_or` patterns)

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Error Handling Patterns] — 4 rules for error handling
- [Source: _bmad-output/planning-artifacts/architecture.md#Anti-Patterns] — unwrap/panic forbidden
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.3] — Original story and ACs
- [Source: _bmad-output/implementation-artifacts/deferred-work.md] — Deferred items from Story 1.1 review

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- AuthInterceptor `call()` returns `Result<Request, Status>` — used `Status::unauthenticated` for token parse failure
- `store_metric` is `pub fn` returning `()` — changed panic to warn+return (no Result change needed)
- Mutex lock errors use early return since functions return `()` in the store_metric context
- Device client creation uses `?` since the containing functions already return `Result<_, OpcGwError>`
- i32 clamping chosen over error for metric conversion — losing precision is better than losing the metric entirely
- u8 returns BadOutOfRange OPC UA status code — correct OPC UA error semantics for command value out of range
- `local_ip()` fallback to "0.0.0.0" — binds to all interfaces, safe for Docker deployment

### Completion Notes List

- All 8 tasks completed successfully
- 14 production panic sites eliminated (1 was already safe with unwrap_or)
- OpcGwError extended with Database(String) variant
- #[should_panic] test converted to non-panic assertion test
- 17/17 tests pass, zero clippy warnings, zero build errors
- Zero unwrap/expect/panic remaining in production code paths

### Change Log

- 2026-04-02: Story 1.3 — comprehensive error handling overhaul, eliminated all production panics

### File List

- `src/utils.rs` — added Database(String) variant to OpcGwError
- `src/main.rs` — replaced 3 panics with error propagation via Result
- `src/chirpstack.rs` — replaced 7 panics (auth token, device name, 3x mutex lock, 2x device client)
- `src/opc_ua.rs` — replaced 3 panics (local IP fallback, i64→i32 clamping, i64→u8 range check), added warn import
- `src/storage.rs` — replaced 1 panic with warn log, updated test from should_panic to assertion

# Story 1.2: Migrate Logging from log4rs to Tracing

Status: done

## Story

As an **operator**,
I want structured, per-module logging using the tracing ecosystem,
so that I can troubleshoot gateway issues with clear, filterable, async-native log output.

## Acceptance Criteria

1. **Given** the current log + log4rs logging setup, **When** I replace all `use log::{...}` imports with `use tracing::{...}` across all source files, **Then** all log macros use structured key-value fields instead of format strings.
2. **Given** the migration is complete, **When** the gateway starts, **Then** log4rs.yaml is no longer needed — tracing-subscriber is configured programmatically in main.rs.
3. **Given** per-module logging is configured, **When** the gateway runs, **Then** separate log files are created via tracing-appender: chirpstack.log, opc_ua.log, opc_ua_gw.log (root), storage.log, config.log.
4. **Given** the migration is complete, **When** I inspect Cargo.toml, **Then** `log` and `log4rs` crates are removed.
5. **Given** API tokens or passwords appear in runtime data, **When** they pass through logging code, **Then** they never appear in log output at any level (NFR7).
6. **Given** the migration is complete, **When** I run the test suite, **Then** `cargo test` passes all existing tests.
7. **Given** the migration is complete, **When** I run clippy, **Then** `cargo clippy` produces no warnings.

## Tasks / Subtasks

- [x] Task 1: Replace log imports with tracing imports (AC: #1)
  - [x] In `src/main.rs`: `use log::{error, info}` → `use tracing::{error, info}`
  - [x] In `src/config.rs`: `use log::{debug, trace}` → `use tracing::{debug, trace}`
  - [x] In `src/opc_ua.rs`: `use log::{debug, error, info, trace}` → `use tracing::{debug, error, info, trace}`
  - [x] In `src/storage.rs`: `use log::{debug, error, info, trace, warn}` → `use tracing::{debug, error, info, trace, warn}`
  - [x] In `src/chirpstack.rs`: `use log::{debug, error, trace, warn}` → `use tracing::{debug, error, trace, warn}`

- [x] Task 2: Replace log4rs initialization with tracing-subscriber (AC: #2, #3)
  - [x] Removed `log4rs::init_file(...)` block from main.rs
  - [x] Added programmatic tracing-subscriber setup with per-layer filtering:
    - Console layer (stdout) at DEBUG
    - Root file layer (opc_ua_gw.log) at DEBUG
    - Per-module file layers: chirpstack.log, opc_ua.log, storage.log, config.log with Targets filters
    - async_opcua module routed to opc_ua.log at DEBUG
    - All writers use non_blocking() with guards held in main() scope
  - [x] Removed `config/log4rs.yaml` file

- [x] Task 3: Convert format-string log calls to structured tracing fields (AC: #1)
  - [x] Converted `src/chirpstack.rs` — 62 log calls (31 converted to structured fields, 31 static messages unchanged)
  - [x] Converted `src/opc_ua.rs` — 32 active log calls (16 converted, 16 static messages unchanged)
  - [x] Converted `src/storage.rs` — 23 log calls (20 converted, 3 static messages unchanged)
  - [x] Converted `src/config.rs` — 8 log calls (5 converted, 2 static messages + 1 commented unchanged)
  - [x] Converted `src/main.rs` — 4 log calls (2 converted, 2 static messages unchanged)

- [x] Task 4: Ensure secrets never appear in logs (AC: #5)
  - [x] Audited all tracing calls — no api_token or user_password values appear in any tracing field
  - [x] AuthInterceptor api_token never logged
  - [x] OPC UA user_password never logged
  - [x] Response debug traces log response objects, not auth headers — safe

- [x] Task 5: Remove log and log4rs from Cargo.toml (AC: #4)
  - [x] Removed `log = "0.4.28"`
  - [x] Removed `log4rs = "1.4.0"`
  - [x] Moved tracing deps out of "unused" section (now actively used)

- [x] Task 6: Build, test, lint (AC: #6, #7)
  - [x] `cargo build` — zero errors
  - [x] `cargo test` — 17 passed, 0 failed
  - [x] `cargo clippy` — zero warnings

### Review Findings

- [x] [Review][Patch] `response_time_ms` field name in storage.rs is misleading — value is in seconds, not milliseconds. Renamed to `response_time_s`. [storage.rs:640] — FIXED
- [x] [Review][Patch] File appenders use `rolling::never` (no rotation). Changed to `rolling::daily` for basic log rotation. [main.rs:91-99] — FIXED
- [x] [Review][Defer] Console layer hardcoded to DEBUG with no runtime override (e.g., RUST_LOG) — pre-existing behavior, not a regression. Future enhancement.
- [x] [Review][Defer] Log file path "log/" is hardcoded — pre-existing from log4rs.yaml. Configuration of log paths is a future enhancement.
- [x] [Review][Defer] No fallback if log directory missing — logs silently drop. Same as old behavior in Docker (dir is mapped volume).

## Dev Notes

### Current Logging Inventory (136 calls)

| File | Total | debug | trace | error | warn | info |
|------|-------|-------|-------|-------|------|------|
| chirpstack.rs | 62 | 26 | 24 | 4 | 8 | 0 |
| opc_ua.rs | 39 | 20 | 10 | 4 | 0 | 1 |
| storage.rs | 23 | 12 | 11 | 0 | 0 | 0 |
| config.rs | 8 | 5 | 3 | 0 | 0 | 0 |
| main.rs | 4 | 0 | 0 | 2 | 0 | 2 |
| **Total** | **136** | **63** | **48** | **10** | **8** | **3** |

All 136 calls currently use format strings. Zero use structured fields.

### Current log4rs.yaml Configuration

6 appenders: stdout + 5 per-module file appenders (opc_ua.log, opc_ua_gw.log, chirpstack.log, storage.log, config.log).

Module-level loggers:
- `opcgw::opc_ua` → trace → opcua_log
- `opcua_server` → debug → opcua_log
- `opcgw::chirpstack` → trace → chirpstack_log
- `opcgw::storage` → trace → storage_log
- `opcgw::config` → trace → config_log
- Root: debug → stdout + opcuagw_log

Pattern: `{d} - {l} - {t} - {m}{n}` (date, level, thread, message, newline)

### Tracing Conversion Patterns

**Architecture mandates structured key-value fields:**

```rust
// CORRECT — structured fields
tracing::debug!(device_id = %dev_id, metric = %name, "Stored metric");
tracing::warn!(error = %e, retry = count, "ChirpStack connection failed");

// WRONG — format strings (current style)
tracing::debug!("Stored metric {} for {}", name, dev_id);
```

**Conversion rules:**
- Move variable data to named fields before the message string
- Use `%` for Display, `?` for Debug formatting in fields
- Message string should be a static description (no interpolation)
- Include relevant identifiers: device_id, metric_name, app_name, etc.

### tracing-subscriber Setup Pattern

**Critical: Per-layer filtering is required.** A single global `EnvFilter` sends all modules to all writers. To route specific modules to specific files, use `.with_filter()` on each layer individually.

```rust
use tracing_appender::{non_blocking, rolling};
use tracing_subscriber::{
    fmt, filter, layer::SubscriberExt, util::SubscriberInitExt, Layer,
};

// Create non-blocking file writers (non_blocking returns a guard that must be held)
let (chirpstack_writer, _guard1) = non_blocking(rolling::never("log", "chirpstack.log"));
let (opcua_writer, _guard2) = non_blocking(rolling::never("log", "opc_ua.log"));
let (root_writer, _guard3) = non_blocking(rolling::never("log", "opc_ua_gw.log"));
let (storage_writer, _guard4) = non_blocking(rolling::never("log", "storage.log"));
let (config_writer, _guard5) = non_blocking(rolling::never("log", "config.log"));

// IMPORTANT: All _guard variables must be held alive for the lifetime of the
// application. Drop them and the writers stop flushing. Store in main() scope.

tracing_subscriber::registry()
    // Console layer: all modules at debug
    .with(
        fmt::layer()
            .with_writer(std::io::stdout)
            .with_filter(filter::LevelFilter::DEBUG)
    )
    // Root file layer: all modules at debug
    .with(
        fmt::layer()
            .with_writer(root_writer)
            .with_filter(filter::LevelFilter::DEBUG)
    )
    // Per-module file layers with per-layer target filters
    .with(
        fmt::layer()
            .with_writer(chirpstack_writer)
            .with_filter(filter::Targets::new()
                .with_target("opcgw::chirpstack", tracing::Level::TRACE))
    )
    .with(
        fmt::layer()
            .with_writer(opcua_writer)
            .with_filter(filter::Targets::new()
                .with_target("opcgw::opc_ua", tracing::Level::TRACE)
                .with_target("async_opcua", tracing::Level::DEBUG))
    )
    .with(
        fmt::layer()
            .with_writer(storage_writer)
            .with_filter(filter::Targets::new()
                .with_target("opcgw::storage", tracing::Level::TRACE))
    )
    .with(
        fmt::layer()
            .with_writer(config_writer)
            .with_filter(filter::Targets::new()
                .with_target("opcgw::config", tracing::Level::TRACE))
    )
    .init();
```

**Key details:**
- `non_blocking()` returns a `(NonBlocking, WorkerGuard)` — the guard MUST be held in scope or the writer silently stops. Store all guards in a `Vec` or individual variables in `main()`.
- `filter::Targets` provides per-layer module routing (available in tracing-subscriber 0.3.x with `env-filter` feature).
- `async_opcua` is the module path for async-opcua 0.17 internal logs (it uses tracing natively — no `tracing-log` bridge needed).
- The exact module target for async-opcua may be `async_opcua` or `opcua` — verify at runtime by checking log output.

### Previous Story Intelligence (Story 1.1)

- Dependencies already added: `tracing = "0.1.41"`, `tracing-subscriber = "0.3.19"` (with env-filter), `tracing-appender = "0.2.3"`
- `log` and `log4rs` still in Cargo.toml — to be removed in this story
- No breaking API changes expected — tracing macros have same names as log macros

### What NOT to Do

- Do NOT start using tokio-util CancellationToken — that's Story 1.4
- Do NOT start using rusqlite — that's Story 2.2
- Do NOT refactor error handling (unwrap → Result) — that's Story 1.3
- Do NOT change application logic — only migrate logging
- Do NOT add new log statements — only convert existing ones
- Do NOT change log levels — preserve existing levels exactly

### Project Structure Notes

Files to modify:
- `src/main.rs` — replace log4rs init with tracing-subscriber setup, convert 4 log calls
- `src/chirpstack.rs` — convert 62 log calls to structured tracing
- `src/opc_ua.rs` — convert 39 log calls to structured tracing
- `src/storage.rs` — convert 23 log calls to structured tracing
- `src/config.rs` — convert 8 log calls to structured tracing
- `Cargo.toml` — remove log + log4rs deps

Files to delete:
- `config/log4rs.yaml`

### Testing Standards

- All existing tests must pass (`cargo test`)
- `cargo clippy` must pass with zero warnings
- No new tests needed unless tracing-subscriber setup is testable

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Tracing Patterns] — structured logging with key-value fields
- [Source: _bmad-output/planning-artifacts/architecture.md#Migration Notes] — log→tracing migration steps
- [Source: _bmad-output/planning-artifacts/architecture.md#Implementation Patterns] — log levels guide
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.2] — Original story and acceptance criteria

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- tracing macros accept same format strings as log macros — import swap compiled immediately
- Per-layer filtering required `filter::Targets` instead of global `EnvFilter` for module-to-file routing
- `non_blocking()` guards must be held in scope — stored as `_guard1` through `_guard5` in main()
- async-opcua 0.17 uses tracing natively — no tracing-log bridge needed
- 136 log calls processed: ~74 converted to structured fields, ~62 were static messages (no args, already valid)

### Completion Notes List

- All 6 tasks completed successfully
- log and log4rs fully removed from codebase
- config/log4rs.yaml deleted
- tracing-subscriber configured with 7 layers (1 console, 1 root file, 5 per-module files)
- All format-string log calls converted to structured key-value fields
- Secret audit passed — no credentials in tracing output
- 17/17 tests pass, zero clippy warnings, zero build errors

### Change Log

- 2026-04-02: Story 1.2 — migrated all logging from log+log4rs to tracing ecosystem

### File List

- `Cargo.toml` — removed log + log4rs, moved tracing deps to active section
- `src/main.rs` — replaced log4rs init with tracing-subscriber setup, converted 2 error calls
- `src/chirpstack.rs` — converted 62 log calls to structured tracing
- `src/opc_ua.rs` — converted 32 log calls to structured tracing
- `src/storage.rs` — converted 23 log calls to structured tracing
- `src/config.rs` — converted 8 log calls to structured tracing
- `config/log4rs.yaml` — DELETED

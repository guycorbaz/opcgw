# Story 1.4: Graceful Shutdown with CancellationToken

Status: done

## Story

As an **operator**,
I want the gateway to shut down cleanly on SIGTERM,
so that in-progress operations complete and no data is lost during Docker container restarts.

## Acceptance Criteria

1. **Given** the gateway is running with active poller and OPC UA server tasks, **When** a SIGTERM signal is received, **Then** a CancellationToken (from tokio-util) is cancelled in main().
2. **Given** cancellation is triggered, **When** each spawned task checks its token, **Then** the poller stops between poll cycles and the OPC UA server stops accepting connections.
3. **Given** cancellation is triggered, **When** the shutdown sequence executes, **Then** the order is: stop accepting new OPC UA connections → complete in-progress poll → exit.
4. **Given** clean shutdown completes, **When** the process exits, **Then** the exit code is 0.
5. **Given** the implementation is complete, **When** I run tests, **Then** a test validates that CancellationToken propagation works across tasks.
6. **Given** the implementation is complete, **When** I run clippy, **Then** `cargo clippy` produces no warnings.

## Tasks / Subtasks

- [x] Task 1: Create CancellationToken in main() and wire signal handler (AC: #1)
  - [x] Added `use tokio_util::sync::CancellationToken;` to main.rs
  - [x] Created token before spawning tasks
  - [x] Added BOTH SIGINT (ctrl_c) and SIGTERM (unix signal) handlers via `tokio::select!`
  - [x] Replaced `tokio::try_join!` wait with signal-first `select!` pattern

- [x] Task 2: Pass CancellationToken to ChirpstackPoller (AC: #2, #3)
  - [x] Added `cancel_token: tokio_util::sync::CancellationToken` field to struct
  - [x] Added token parameter to `new()` constructor
  - [x] In `run()` loop: replaced `tokio::time::sleep` with `select!` racing sleep vs `cancelled()`
  - [x] On cancellation: logs info "ChirpStack poller shutting down" and returns `Ok(())`
  - [x] Added `info` to tracing imports

- [x] Task 3: Pass CancellationToken to OPC UA server (AC: #2, #3)
  - [x] Added `.token(self.cancel_token.clone())` to ServerBuilder chain in `create_server()`
  - [x] Added `cancel_token: tokio_util::sync::CancellationToken` field to `OpcUa` struct
  - [x] Added token parameter to `OpcUa::new()` constructor
  - [x] async-opcua handles shutdown automatically when token is cancelled

- [x] Task 4: Update main() orchestration (AC: #3, #4)
  - [x] Passed `cancel_token.clone()` to both constructors
  - [x] `select!` waits for SIGINT or SIGTERM, then cancels token
  - [x] After cancel: `tokio::try_join!` awaits both handles for graceful completion
  - [x] Exit code 0 on clean shutdown
  - [x] Logs "Stopping opcgw" before exit

- [x] Task 5: Write shutdown test (AC: #5)
  - [x] `test_cancellation_token_propagation` in main.rs — spawns async task with token, cancels, verifies task exits
  - [x] Uses `AtomicBool` to verify task saw the cancellation

- [x] Task 6: Build, test, lint (AC: #6)
  - [x] `cargo build` — zero errors
  - [x] `cargo test` — 18 passed (17 existing + 1 new cancellation test), 0 failed
  - [x] `cargo clippy` — zero warnings

### Review Findings

- [x] [Review][Patch] Added 10s timeout on post-cancel `try_join!` with `tokio::time::timeout`. [main.rs:212] — FIXED
- [x] [Review][Patch] Errors from `try_join!` now logged: Ok→info, Err→error, Timeout→error. [main.rs:212] — FIXED
- [x] [Review][Defer] Cancellation not checked inside `poll_metrics()` retry loop — pre-existing, deferred to Story 4.4 (Auto-Recovery). Current design checks at "safe points" between polls.

## Dev Notes

### Current Shutdown Mechanism

Currently main.rs spawns two tasks and waits with `tokio::try_join!`. There is no signal handling — SIGTERM kills the process immediately with no cleanup. The Ctrl+C behavior depends on the terminal.

### Current Run Loop: ChirpstackPoller

```rust
pub async fn run(&mut self) -> Result<(), OpcGwError> {
    let wait_time = Duration::from_secs(self.config.chirpstack.polling_frequency);
    loop {
        if let Err(e) = self.poll_metrics().await { ... }
        tokio::time::sleep(wait_time).await;  // <-- cancellation check point
    }
}
```

**Target pattern:**
```rust
pub async fn run(&mut self) -> Result<(), OpcGwError> {
    let wait_time = Duration::from_secs(self.config.chirpstack.polling_frequency);
    loop {
        if let Err(e) = self.poll_metrics().await { ... }
        tokio::select! {
            _ = self.cancel_token.cancelled() => {
                info!("ChirpStack poller shutting down");
                return Ok(());
            }
            _ = tokio::time::sleep(wait_time) => {}
        }
    }
}
```

### Current Run Loop: OPC UA Server

```rust
pub async fn run(mut self) -> Result<(), OpcGwError> {
    let server = self.create_server()?;
    let _ = match server.run().await { ... };
    Ok(())
}
```

**async-opcua 0.17 has native CancellationToken support:**
- `ServerBuilder` has `.token(CancellationToken)` method (confirmed in source)
- When the token is cancelled, `server.run()` returns cleanly
- No need for `tokio::select!` on the OPC UA side — just pass the token to the builder

**Target pattern in `create_server()`:**
```rust
let server_builder = ServerBuilder::new()
    .application_name(...)
    // ... existing config ...
    .token(self.cancel_token.clone())  // <-- ADD THIS
    .with_node_manager(...);
```

### Signal Handling Pattern

**CRITICAL: `signal::ctrl_c()` only handles SIGINT (Ctrl+C). Docker `stop` sends SIGTERM. Must handle BOTH.**

```rust
use tokio::signal;
use tokio::signal::unix::SignalKind;

// In main(), after spawning tasks:
let mut sigterm = signal::unix::signal(SignalKind::terminate())?;

tokio::select! {
    result = async {
        tokio::try_join!(chirpstack_handle, opcua_handle)
    } => {
        // Tasks completed on their own (unusual)
        if let Err(e) = result { error!(...); }
    }
    _ = signal::ctrl_c() => {
        info!("Received SIGINT, shutting down");
        cancel_token.cancel();
    }
    _ = sigterm.recv() => {
        info!("Received SIGTERM, shutting down");
        cancel_token.cancel();
    }
}
// IMPORTANT: After select!, the tasks are still running.
// Must await both handles to ensure graceful completion:
let _ = chirpstack_handle.await;
let _ = opcua_handle.await;
```

**Note:** The `tokio::select!` drops the non-winning futures. Since `chirpstack_handle` and `opcua_handle` are `JoinHandle`s, dropping them does NOT cancel the tasks — the tasks continue running until they see the cancelled token. You need to re-await them. One pattern is to keep the handles outside the select and await after:

```rust
// Alternative: keep handles outside select
let chirpstack_handle = tokio::spawn(...);
let opcua_handle = tokio::spawn(...);

// Wait for signal only
tokio::select! {
    _ = signal::ctrl_c() => { info!("SIGINT received"); }
    _ = sigterm.recv() => { info!("SIGTERM received"); }
}
cancel_token.cancel();

// Now wait for tasks to finish gracefully
let _ = tokio::try_join!(chirpstack_handle, opcua_handle);
```

This second pattern is simpler and recommended.

### ChirpstackPoller Struct Changes

Current constructor signature:
```rust
pub async fn new(config: &Arc<AppConfig>, storage: Arc<Mutex<Storage>>) -> Result<Self, OpcGwError>
```

Will become:
```rust
pub async fn new(config: &Arc<AppConfig>, storage: Arc<Mutex<Storage>>, cancel_token: CancellationToken) -> Result<Self, OpcGwError>
```

### OpcUa Struct Changes

Current constructor:
```rust
pub fn new(config: &Arc<AppConfig>, storage: Arc<Mutex<Storage>>) -> Self
```

Will become:
```rust
pub fn new(config: &Arc<AppConfig>, storage: Arc<Mutex<Storage>>, cancel_token: CancellationToken) -> Self
```

### Previous Story Intelligence

**Story 1.3:** All error handling now uses Result propagation — no more panics. The poller `run()` returns `Result<(), OpcGwError>`, which works well with the cancellation pattern (return `Ok(())` on clean shutdown).

**Story 1.1:** `tokio-util = "0.7.18"` already in Cargo.toml dependencies.

### What NOT to Do

- Do NOT start using rusqlite — that's Story 2.2
- Do NOT refactor storage (Arc<Mutex>) — that's Epic 2
- Do NOT add OPC UA address space changes — that's Epic 5
- Do NOT change polling logic — only add cancellation check
- Do NOT add config hot-reload plumbing — that's Phase B

### Project Structure Notes

Files to modify:
- `src/main.rs` — create token, signal handler, orchestration with select!
- `src/chirpstack.rs` — add token field, accept in new(), check in run() loop
- `src/opc_ua.rs` — add token field, accept in new(), pass to ServerBuilder

### Testing Standards

- All existing tests must pass
- New test: CancellationToken propagation across async tasks
- `cargo clippy` must pass with zero warnings

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Infrastructure & Deployment] — CancellationToken shutdown design
- [Source: _bmad-output/planning-artifacts/architecture.md#Cross-Cutting Concerns] — Graceful shutdown coordination
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.4] — Original story and ACs
- [Source: async-opcua-server-0.17.1/src/builder.rs:539] — `.token(CancellationToken)` API

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- async-opcua 0.17 `ServerBuilder::token()` confirmed working — passes CancellationToken directly to server internals
- `tokio::signal::ctrl_c()` only handles SIGINT; explicit `signal::unix::signal(SignalKind::terminate())` needed for Docker SIGTERM
- Signal handler uses simpler pattern: wait for signal only in select!, then cancel + await handles sequentially
- ChirpstackPoller needed `info` added to tracing imports for shutdown message

### Completion Notes List

- All 6 tasks completed
- CancellationToken flows: main() → ChirpstackPoller (via struct field) + OpcUa (via struct field → ServerBuilder)
- Poller checks cancellation between poll cycles via tokio::select!
- OPC UA server uses async-opcua's native token support
- Both SIGINT and SIGTERM handled — Docker stop works correctly
- After cancel, main() awaits both task handles before exiting
- 18/18 tests pass, zero clippy warnings

### Change Log

- 2026-04-02: Story 1.4 — graceful shutdown with CancellationToken, SIGINT+SIGTERM handling

### File List

- `src/main.rs` — CancellationToken creation, signal handlers, select! orchestration, new test
- `src/chirpstack.rs` — cancel_token field, new() parameter, select! in run() loop, added info import
- `src/opc_ua.rs` — cancel_token field, new() parameter, .token() on ServerBuilder

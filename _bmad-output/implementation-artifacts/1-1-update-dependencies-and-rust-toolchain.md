# Story 1.1: Update Dependencies and Rust Toolchain

Status: done

## Story

As a **developer**,
I want all project dependencies updated to their target stable versions,
so that the codebase has the foundation needed for Phase A work (tonic 0.14, async-opcua 0.17, tracing, tokio-util).

## Acceptance Criteria

1. **Given** the current Cargo.toml with outdated dependencies, **When** I update Rust to 1.94.0 and all crate versions per the Architecture document, **Then** `cargo build` compiles successfully with zero errors.
2. **Given** updated dependencies, **When** I run the test suite, **Then** `cargo test` passes all existing tests.
3. **Given** updated dependencies, **When** I run the linter, **Then** `cargo clippy` produces no warnings.
4. **Given** tonic updated to 0.14.5, **When** the ChirpStack poller authenticates, **Then** the AuthInterceptor API works correctly in chirpstack.rs.
5. **Given** async-opcua updated to 0.17.1, **When** the OPC UA server starts, **Then** SimpleNodeManager API changes are adapted in opc_ua.rs.
6. **Given** the updated Cargo.toml, **When** I inspect dependencies, **Then** new dependencies are present: tracing, tracing-subscriber, tracing-appender, tokio-util, rusqlite (bundled). These are added but NOT yet used — usage comes in later stories.

## Tasks / Subtasks

- [x] Task 1: Update Rust toolchain (AC: #1)
  - [x] Update `rust-version` field in `Cargo.toml` from "1.87.0" to "1.94.0"
  - [x] Update `Dockerfile` ARG `RUST_VERSION` from 1.87.0 to 1.94.0 (line 5)
  - [x] Update `Cargo.toml` edition if needed (stay on 2021) — no change needed
  - [x] Verify `rustup update` brings toolchain to 1.94.0+ — confirmed rustc 1.94.0

- [x] Task 2: Update existing dependency versions in Cargo.toml (AC: #1, #2, #3)
  - [x] tokio: 1.47.1 → 1.50.0
  - [x] tonic: 0.13.1 → 0.14.5 (MAJOR)
  - [x] tonic-build (build-dep): 0.13.1 → 0.14.5
  - [x] chirpstack_api: 4.13.0 → 4.17.0 (resolved by cargo to latest compatible)
  - [x] prost-types: removed from Cargo.toml — tonic 0.14 pulls prost 0.14.3 transitively
  - [x] async-opcua: ^0.16.0 → 0.17.1
  - [x] serde: 1.0.219 → 1.0.228
  - [x] clap: 4.5.47 → 4.6.0
  - [x] thiserror: 2.0.16 → 2.0.18
  - [x] figment: 0.10.19 → keep (no change, stable)
  - [x] url: 2.5.7 → keep (no change)
  - [x] local-ip-address: 0.6.5 → keep (no change)

- [x] Task 3: Add new dependencies (not yet used) (AC: #6)
  - [x] Add `tracing = "0.1.41"`
  - [x] Add `tracing-subscriber = { version = "0.3.19", features = ["env-filter"] }`
  - [x] Add `tracing-appender = "0.2.3"`
  - [x] Add `tokio-util = "0.7.18"`
  - [x] Add `rusqlite = { version = "0.38.0", features = ["bundled"] }`
  - [x] Keep `log` and `log4rs` for now — they're removed in Story 1.2

- [x] Task 4: Fix tonic 0.13 → 0.14 breaking changes (AC: #4)
  - [x] AuthInterceptor — no changes needed, Interceptor trait API unchanged
  - [x] Channel creation — no changes needed, API unchanged
  - [x] `InterceptedService` import path — unchanged
  - [x] `prost-types` — removed explicit dep, using chirpstack_api re-export instead
  - [x] chirpstack_api 4.17.0 compatible with tonic 0.14 — confirmed
  - [x] build.rs — `tonic_build::configure()` moved to `tonic_prost_build::configure()`, added `tonic-prost-build` to build-deps

- [x] Task 5: Fix async-opcua 0.16 → 0.17 breaking changes (AC: #5)
  - [x] `SimpleNodeManager` API — unchanged
  - [x] `ServerBuilder` API — unchanged
  - [x] Import paths — unchanged
  - [x] `NamespaceMetadata` — unchanged
  - [x] `NodeId::new(ns, i32)` — i32 no longer implements Into<Identifier>, fixed with `as u32` cast
  - [x] Fixed unused `numeric_range` variable in write callback (prefixed with `_`)

- [x] Task 6: Build, test, lint (AC: #1, #2, #3)
  - [x] `cargo build` — zero errors
  - [x] `cargo test` — 17 passed, 0 failed
  - [x] `cargo clippy` — zero warnings (fixed pre-existing `metric_unit` dead_code warning)

### Review Findings

- [x] [Review][Patch] chirpstack_api version mismatch: Cargo.toml declares "4.15.0" but resolves to 4.17.0. Pin to "4.17.0" to match actual compiled version. [Cargo.toml:22] — FIXED
- [x] [Review][Defer] `command_id as u32` lossy cast — pre-existing, deferred to Story 3.2 (Command Parameter Validation) [opc_ua.rs:592]
- [x] [Review][Defer] `create_device_client().await.unwrap()` — pre-existing panic risk, deferred to Story 1.3 (Error Handling) [chirpstack.rs:1084]
- [x] [Review][Defer] `flush_queue: false` hardcoded — preserves existing behavior, deferred to Story 3.1 (FIFO Command Queue) [chirpstack.rs:1081]
- [x] [Review][Defer] `try_into().unwrap()` in set_command — pre-existing panic risk, deferred to Story 1.3 [opc_ua.rs:824]
- [x] [Review][Defer] `command_port as u32` lossy cast — pre-existing, deferred to Story 3.2 [opc_ua.rs:823]

## Dev Notes

### Critical: tonic 0.13 → 0.14 Migration

This is a **major version bump**. Known areas of concern:

**Current tonic API usage in chirpstack.rs:**
- `impl Interceptor for AuthInterceptor` with `fn call(&mut self, request: Request<()>)` — the Interceptor trait may have changed signature
- `tonic::codegen::InterceptedService` — import path may have moved
- `tonic::service::Interceptor` — trait location may have changed
- `Channel::from_shared(url).connect().await` — channel creation API
- `ClientType::with_interceptor(channel, interceptor)` — client instantiation
- `Request::new(...)` + `client.method(request).await` — should be stable

**Current tonic-build usage in build.rs:**
- `tonic_build::configure().build_server(true).compile_protos(&[...], &["proto"])` — check if `compile_protos` signature changed

**Strategy:** Start with `cargo build` after version bump. The compiler will show exactly what broke. Fix each error methodically.

### Critical: async-opcua 0.16 → 0.17 Migration

**Current async-opcua API usage in opc_ua.rs:**
- `ServerBuilder::new()` with chained `.application_name()`, `.port()`, `.host()`, etc.
- `simple_node_manager(NamespaceMetadata { ... }, "opcgw")` — free function
- `server_builder.build()` returning `(Server, handle)` tuple
- `handle.node_managers().get_of_type::<SimpleNodeManager>()`
- `manager.inner().add_read_callback(node_id, closure)` — read callback registration
- `manager.inner().add_write_callback(node_id, closure)` — write callback registration
- `address_space.write()` — RwLock write guard on address space
- `address_space.add_folder(...)`, `address_space.add_variables(...)`
- `server.run().await` — server main loop
- Import: `opcua::server::address_space::{AccessLevel, Variable}`
- Import: `opcua::server::node_manager::memory::{simple_node_manager, SimpleNodeManager}`
- Import: `opcua::types::{DataValue, DateTime, NodeId, Variant}`

**Strategy:** Same as tonic — bump version, let compiler guide fixes. Check async-opcua changelog/migration guide for 0.17.

### prost-types Version Alignment

tonic 0.14 likely depends on prost 0.14.x. The current `prost-types = "0.13.5"` MUST be updated to match whatever prost version tonic 0.14 pulls in. Check with `cargo tree -i prost` after updating tonic.

### What NOT to Do

- Do NOT start using tracing macros yet — that's Story 1.2
- Do NOT start using tokio-util CancellationToken — that's Story 1.4
- Do NOT start using rusqlite — that's Story 2.2
- Do NOT remove `log` or `log4rs` — that's Story 1.2
- Do NOT refactor error handling — that's Story 1.3
- Do NOT change any application logic — only fix API compatibility

### Project Structure Notes

All changes are in:
- `Cargo.toml` — version bumps, new deps, rust-version field update
- `Dockerfile` — RUST_VERSION ARG update from 1.87.0 to 1.94.0
- `build.rs` — if tonic-build API changed
- `src/chirpstack.rs` — tonic API fixes only
- `src/opc_ua.rs` — async-opcua API fixes only
- No new files created in this story

**Docker note:** The Dockerfile installs `protobuf-compiler` for the build stage. If tonic-build 0.14 changes protobuf requirements, verify the Docker build still works with `docker build .` after all changes.

### Testing Standards

- All existing tests must pass (`cargo test`)
- No new tests needed — this is a dependency update story
- `cargo clippy` must pass with zero warnings

### References

- [Source: _bmad-output/planning-artifacts/architecture.md#Technology Stack & New Dependencies] — Target versions for all crates
- [Source: _bmad-output/planning-artifacts/architecture.md#Migration Notes] — tonic and async-opcua migration guidance
- [Source: _bmad-output/planning-artifacts/architecture.md#Dependency Decisions] — Rationale for each dependency choice
- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.1] — Original story and acceptance criteria

## Dev Agent Record

### Agent Model Used

Claude Opus 4.6 (1M context)

### Debug Log References

- tonic 0.14 moved `configure()` from `tonic_build` to `tonic_prost_build` crate — required adding `tonic-prost-build = "0.14.5"` to build-deps
- `prost_types::Timestamp` no longer directly available — chirpstack_api re-exports it as `chirpstack_api::prost_types::Timestamp`
- chirpstack_api resolved to 4.17.0 (latest compatible with tonic 0.14), added `flush_queue` field to `EnqueueDeviceQueueItemRequest`
- async-opcua 0.17 changed `NodeId::new` identifier type — `i32` no longer implements `Into<Identifier>`, requires `as u32` cast

### Completion Notes List

- All 6 tasks completed successfully
- 3 breaking changes found and fixed (tonic_build API, prost_types import, NodeId identifier type)
- 1 new API field added (flush_queue on EnqueueDeviceQueueItemRequest)
- 2 pre-existing clippy warnings fixed (unused variable, dead_code)
- 17/17 existing tests pass, zero clippy warnings, zero build errors
- New deps added but not yet used: tracing, tracing-subscriber, tracing-appender, tokio-util, rusqlite

### Change Log

- 2026-04-02: Story 1.1 implementation complete — all dependencies updated, breaking changes fixed

### File List

- `Cargo.toml` — updated dependency versions, added new deps, updated rust-version
- `Cargo.lock` — regenerated by cargo
- `Dockerfile` — updated RUST_VERSION from 1.87.0 to 1.94.0
- `build.rs` — changed `tonic_build::configure()` to `tonic_prost_build::configure()`
- `src/chirpstack.rs` — fixed `prost_types::Timestamp` import, added `flush_queue` field
- `src/opc_ua.rs` — fixed `NodeId::new()` i32→u32 cast, prefixed unused `numeric_range` with `_`
- `src/config.rs` — added `#[allow(dead_code)]` on `metric_unit` field

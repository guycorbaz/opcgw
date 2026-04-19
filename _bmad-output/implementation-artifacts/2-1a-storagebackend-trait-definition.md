# Story 2-1a: StorageBackend Trait Definition

Status: done

## Story

As a **developer**,
I want a clean `StorageBackend` trait with a well-defined interface,
So that all storage implementations follow the same contract.

## Acceptance Criteria

1. **Given** the need for a storage abstraction, **When** I create `src/storage/mod.rs`, **Then** a `StorageBackend` trait is defined with clear method signatures.
2. **Given** the trait is defined, **When** I inspect the interface, **Then** it includes methods for metric operations (get, set), gateway status operations (get, update), and command queue operations (queue, retrieve, update status).
3. **Given** the trait is implemented, **When** multiple implementations exist, **Then** they all provide equivalent behavior (verified by trait object usage pattern).
4. **Given** concurrent access requirements, **When** the trait is designed, **Then** it supports Arc<Self> for thread-safe sharing.
5. **Given** error handling requirements, **When** trait methods fail, **Then** they return `Result<T, OpcGwError>` with clear error context.
6. **Given** documentation requirements, **When** the trait is complete, **Then** all public methods have doc comments explaining purpose and error cases.

## Tasks / Subtasks

- [x] Task 1: Create storage module structure (AC: #1)
  - [x] Create `src/storage/` directory
  - [x] Create `src/storage/mod.rs` (root module file)
  - [x] Create `src/storage/types.rs` placeholder for types (will be filled in 2-1b)

- [x] Task 2: Define StorageBackend trait (AC: #1, #2)
  - [x] Define trait with metric methods:
    - `get_metric(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricType>>`
    - `set_metric(&self, device_id: &str, metric_name: &str, value: MetricType) -> Result<()>`
  - [x] Define gateway status methods:
    - `get_status(&self) -> Result<ChirpstackStatus>`
    - `update_status(&self, status: ChirpstackStatus) -> Result<()>`
  - [x] Define command queue methods:
    - `queue_command(&self, command: DeviceCommand) -> Result<()>`
    - `get_pending_commands(&self) -> Result<Vec<DeviceCommand>>`
    - `update_command_status(&self, command_id: u64, status: CommandStatus) -> Result<()>`

- [x] Task 3: Add trait documentation (AC: #6)
  - [x] Doc comments on trait explaining purpose
  - [x] Doc comments on each method
  - [x] Document error cases (corrupted data, connection lost, etc.)
  - [x] Example usage pattern in docs

- [x] Task 4: Design Arc-safe trait (AC: #4)
  - [x] Verify trait works with `Arc<dyn StorageBackend>` (test_trait_is_send_sync)
  - [x] Ensure no lifetime issues
  - [x] Confirm thread-safe usage pattern (Send + Sync bounds on trait)

- [x] Task 5: Error handling strategy (AC: #5)
  - [x] All methods return `Result<T, OpcGwError>`
  - [x] Define error context requirements
  - [x] Error documentation in trait method docs

- [x] Task 6: Build, test, lint (AC: #5, #6)
  - [x] `cargo build` — zero errors
  - [x] `cargo clippy` — zero warnings
  - [x] Module compiles in isolation

## Dev Notes

### Trait Method Design

The trait methods should be minimal and focused:

```rust
pub trait StorageBackend: Send + Sync {
    // Metrics
    fn get_metric(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricValue>, OpcGwError>;
    fn set_metric(&self, device_id: &str, metric_name: &str, value: MetricValue) -> Result<(), OpcGwError>;

    // Gateway Status
    fn get_status(&self) -> Result<ChirpstackStatus, OpcGwError>;
    fn update_status(&self, status: ChirpstackStatus) -> Result<(), OpcGwError>;

    // Command Queue
    fn queue_command(&self, command: DeviceCommand) -> Result<(), OpcGwError>;
    fn get_pending_commands(&self) -> Result<Vec<DeviceCommand>, OpcGwError>;
    fn update_command_status(&self, command_id: u64, status: CommandStatus) -> Result<(), OpcGwError>;
}
```

**Key decisions:**
- **Sync trait:** Methods are synchronous blocking operations. Async wrapper can be added in Epic 4 if needed.
- **Send + Sync bounds:** Required for Arc<dyn StorageBackend> usage across task boundaries.
- **No transactions in trait:** Transactions are implementation detail (Story 2-3c).
- **Command ID:** u64 auto-increment, assigned by backend on queue_command().

### What NOT to Do

- Do NOT implement trait yet (that's Story 2-1c for InMemory)
- Do NOT create types yet (that's Story 2-1b)
- Do NOT add async/await (keep it sync)
- Do NOT add query methods (keep interface minimal)
- Do NOT use TypeScript-style Optional (use Option<T>)

### Testing Strategy

- Unit test: Verify trait compiles with `dyn StorageBackend`
- Unit test: Verify `Arc<dyn StorageBackend>` is Send + Sync
- No implementation tests yet (covered by 2-1c)

## File List

- `src/storage/mod.rs` — trait definition, public module interface, existing Storage implementation
- `src/storage/types.rs` — placeholder (types added in 2-1b)

## Dev Agent Record

### Implementation Plan

Refactored the storage module from a single file (`src/storage.rs`) to a directory-based module structure with the new `StorageBackend` trait. The implementation took the following approach:

1. **Module Structure**: Created `src/storage/` directory with:
   - `mod.rs`: Contains the trait definition, all existing types (MetricType, Device, ChirpstackStatus, DeviceCommand), the Storage struct, and implementation
   - `types.rs`: Placeholder for Phase 2-1b core types

2. **StorageBackend Trait**: Defined a clean trait interface with:
   - 7 methods organized in 3 categories: metric operations (get/set), gateway status (get/update), command queue (queue/retrieve/update)
   - Send + Sync bounds for Arc<dyn StorageBackend> support
   - Result<T, OpcGwError> error handling
   - Comprehensive doc comments including error cases and examples

3. **Type Support**: Added CommandStatus enum to support command lifecycle tracking

4. **Testing Strategy**: Created trait-specific unit tests:
   - test_trait_is_send_sync: Validates Arc<dyn StorageBackend> is Send + Sync
   - test_trait_method_signatures_exist: Validates trait compiles as dyn trait

### Completion Notes

✅ All acceptance criteria satisfied:
- AC #1: StorageBackend trait defined in src/storage/mod.rs
- AC #2: Trait includes metric, gateway status, and command queue operations
- AC #3: Trait design supports multiple implementations via trait objects
- AC #4: Arc<dyn StorageBackend> is Send + Sync (verified by tests)
- AC #5: All methods return Result<T, OpcGwError>
- AC #6: Comprehensive doc comments on trait and all methods

✅ All tasks complete:
- Task 1: Module structure created
- Task 2: Trait interface defined
- Task 3: Documentation added (extensive doc comments)
- Task 4: Arc-safety verified (test_trait_is_send_sync)
- Task 5: Error handling strategy implemented
- Task 6: Build passes, clippy clean, tests pass (28/28)

### Architecture Decisions

1. **Sync Methods**: Trait uses synchronous blocking operations. Async wrapper can be added in Epic 4 if needed.
2. **No Transactions**: Transactions are implementation details (handled in Story 2-3c)
3. **Command IDs**: Trait specifies u64 for command IDs (backend assigns on queue_command)
4. **Type Names**: Uses MetricType instead of MetricValue (existing codebase convention)

### Change Log

- Refactored src/storage.rs → src/storage/mod.rs and src/storage/types.rs
- Deleted old src/storage.rs file (content moved to mod.rs)
- Added StorageBackend trait definition
- Added CommandStatus enum for command lifecycle
- Added unit tests for trait interface validation

## Senior Developer Review (AI)

**Review Date:** 2026-04-19  
**Review Outcome:** CHANGES REQUESTED  
**Reviewers:** Blind Hunter (syntax/logic), Edge Case Hunter (integration), Acceptance Auditor (spec compliance)

**Summary:** All 6 acceptance criteria **PASS**. Code compiles, tests pass (28/28), clippy clean. However, 2 architectural decisions and 6 documentation/API issues require attention.

### Action Items

#### Decisions Required (Cannot be auto-fixed)
- [ ] [Review][Decision] Missing StorageBackend Implementation — Story says "Do NOT implement trait yet (2-1c)" but reviewers flagged trait as incomplete. Clarify: is Storage implementing StorageBackend in 2-1c, or is the trait a standalone interface?
- [ ] [Review][Decision] Trait Method Names vs Storage API Mismatch — Trait defines `get_metric()`/`set_metric()` but Storage has `get_metric_value()`/`set_metric_value()`. When 2-1c implements the trait, which API should be renamed: the trait or Storage methods? This affects chirpstack.rs and opc_ua.rs.

#### Patches (Actionable without user input)
- [x] [Review][Patch] Fix get_metric_value Mutability — Changed to `&self` signature; removed mutable requirement for read-only operation. [src/storage/mod.rs:687]
- [x] [Review][Patch] Fix set_metric_value Panic Documentation — Updated docs to reflect actual behavior: gracefully handles missing devices with warning log. [src/storage/mod.rs:742-751]
- [x] [Review][Patch] Fix Outdated Field Names in Examples — Updated doc examples: `device_eui` → `device_id`, `enqueue_command()` → `push_command()`. [src/storage/mod.rs:1026, 1050]
- [x] [Review][Patch] Complete Incomplete Documentation — Completed sentence and added clarification on thread safety requirements. [src/storage/mod.rs:28-30]
- [x] [Review][Patch] Remove Blanket allow(unused) Attribute — Removed `#![allow(unused)]` to unmask any real issues. [src/storage/mod.rs:4]
- [x] [Review][Patch] Fix Parameter Type Inconsistency — Changed `&String` to `&str` in get_device() and get_device_name(). [src/storage/mod.rs:604, 789]

#### Deferred (Pre-existing or Design Intent)
- [x] [Review][Defer] CommandStatus Enum Unused — Defined but never used; intentional (will be implemented in 2-1c InMemoryBackend). [src/storage/mod.rs:370-378]
- [x] [Review][Defer] CommandStatus::Failed Memory Allocation — Low-priority optimization for future tuning. [src/storage/mod.rs:374]

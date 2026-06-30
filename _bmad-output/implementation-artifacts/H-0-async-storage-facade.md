# Story H.0: Async Storage Facade (spawn_blocking boundary)

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator running opcgw on a CPU-constrained host (e.g. a small Docker container)**,
I want the gateway's database access to never block the async runtime's worker threads,
so that the poller, OPC UA server, and web UI stay responsive under load instead of stalling on SQL.

## Context & Problem (GitHub #73)

`StorageBackend` (`src/storage/mod.rs:182`) is a **fully synchronous** trait — ~30 methods doing blocking `rusqlite` I/O — shared as `Arc<dyn StorageBackend>`. It is invoked from **~30–50 async call sites** across the codebase. Every such call blocks a tokio worker thread for the duration of the SQL operation. Two retry-backoff loops additionally call `std::thread::sleep` directly on the runtime (`src/storage/sqlite.rs:1265`, `:1346`) when the connection pool is exhausted.

This has only been survivable because `#[tokio::main]` (`src/main.rs:521`) defaults to the **multi-threaded** runtime (~32 workers on the dev host). On CPU-limited deployments (small Docker containers with 1–2 vCPUs) the worker pool is tiny, and blocking SQL + 100–300 ms pool-retry sleeps stall the poller, the OPC UA server, and web handlers. Story G-4's code review explicitly deferred "blocking `pool.checkout` #73" as a LOW pointing at this story.

**Owner decision (2026-06-29): Strategy A** — introduce an **async facade** that wraps `Arc<dyn StorageBackend>` and runs each call via `tokio::task::spawn_blocking`, converting async call sites to `.await`. The synchronous trait and both backend implementations stay unchanged. (Rejected alternatives: making `StorageBackend` natively async, or swapping in an async SQLite driver — both far larger blast radius; scattering `spawn_blocking` at every call site — loses the single-boundary property.)

## Acceptance Criteria

1. **AC#1 — Facade exists.** A new async facade type (e.g. `AsyncStorage`) wraps a single `Arc<dyn StorageBackend>` (cloneable, `Send + Sync + 'static`) and exposes an `async` method for **every** `StorageBackend` method that is invoked from an async context. Each async method clones the `Arc`, moves owned arguments into a `tokio::task::spawn_blocking` closure, calls the underlying sync method, and returns its `Result<_, OpcGwError>` unchanged (the `JoinError` from a panicking blocking task is mapped to an `OpcGwError`, not unwrapped).

2. **AC#2 — No behavioural change.** Return types, values, `OpcGwError` error mapping, and result ordering are identical to calling the backend directly. The facade is a pure execution-context shim — no added validation, caching, ret– or logging semantics beyond what already exists in the backend.

3. **AC#3 — Async call sites converted.** All production **async** call sites that today call the backend directly now go through the facade with `.await`:
   - `src/chirpstack.rs` (ChirpstackPoller): `record_error_event` (:910), `prune_metric_history` (:1302), `batch_write_metrics` (:1531), `update_gateway_status` (:1627), `get_metric_value` (:1861), `upsert_metric_value` + `append_metric_history` (:2029–:2052), `get_pending_commands` (:2522).
   - `src/chirpstack_events.rs` (uplink/ack ingestion, async fns): `get_metric_value` (:645), `batch_write_metrics` (:720, :807), `find_command_by_result_id` (:834), `mark_command_confirmed` (:873), `mark_command_failed` (:899).
   - `src/web/api.rs` (axum async handlers): `get_gateway_health_metrics` (:184), `recent_error_events` (:545), `load_all_metrics` (:594).
   - `src/opc_ua_history.rs`: `query_metric_history` (:320) — **only if** its enclosing call path is `async`; see AC#5.

4. **AC#4 — The two `thread::sleep` backoffs run off the runtime.** Because the pool-retry loop now executes inside `spawn_blocking`, the `std::thread::sleep` calls at `src/storage/sqlite.rs:1265` and `:1346` no longer block an async worker. They may remain `std::thread::sleep` (a blocking sleep inside `spawn_blocking` is correct — do **not** convert them to `tokio::time::sleep`).

5. **AC#5 — Genuinely-synchronous call sites are handled correctly, not blindly converted.** The OPC UA node-manager / method-call callbacks in `src/opc_ua.rs` (e.g. `:1347`, `:1558`, `:1655`, `:2075`) and any other site where the enclosing function is **not** `async` (or is a sync trait-callback that cannot `.await`) must be assessed individually:
   - If the context is `async`, convert to the facade + `.await`.
   - If the context is a **sync callback executed on the OPC UA server task** and cannot be made async, either route it through `tokio::task::block_in_place` + the facade (multi-thread runtime guarantees this is available) **or** keep the direct sync call and document why in a code comment. Do **not** silently leave a blocking call on an async worker, and do **not** introduce a nested-runtime `block_on`.
   - The story's File List + Completion Notes must state, per such site, which option was chosen and why.

6. **AC#6 — Startup path.** One-shot startup calls in `src/main.rs` (concrete `SqliteBackend` during boot, e.g. `:310`, `:966`, `:1017`, `:1067`) run sequentially before the long-lived tasks spawn. These may be left as direct sync calls (documented) **or** wrapped — but if left, add a one-line comment noting they are intentional one-shot boot calls outside the steady-state async hot path.

7. **AC#7 — Tests prove the boundary.** Add at least one test demonstrating that storage access from an async context does not block the runtime — e.g. a test on a single-worker runtime (`#[tokio::test(flavor = "multi_thread", worker_threads = 1)]` or a current-thread runtime with `spawn_blocking`) where concurrent async tasks make progress while a storage call is in flight. Existing storage/poller/web tests continue to pass unchanged.

8. **AC#8 — Gates green.** `cargo test --all-targets` is 0-failed (the full ~1700-test suite), `cargo clippy --all-targets -- -D warnings` is clean, and `cargo build --release` succeeds. Closes #73.

## Tasks / Subtasks

- [x] **Task 1 — Build the `AsyncStorage` facade (AC#1, AC#2, AC#4)**
  - [x] Added `src/storage/async_facade.rs` defining `AsyncStorage { inner: Arc<dyn StorageBackend> }`, `#[derive(Clone)]`.
  - [x] 19 `async fn` wrappers (the methods reached from async contexts), each `let inner = self.inner.clone(); spawn_blocking(move || inner.method(args…)).await.map_err(join_err)?`.
  - [x] `join_err` helper maps `tokio::task::JoinError` → `OpcGwError::Storage("storage task failed: …")`; the JoinResult is never `.unwrap()`'d.
  - [x] All `&str`/`&MetricType`/`&ErrorEvent` args are converted to owned at the boundary so the closure is `'static`.
  - [x] Instead of a `blocking()` accessor, sync sites use the standalone `run_blocking_storage()` helper (AC#5) — cleaner than exposing `inner`.
- [x] **Task 2 — Thread the facade through the owning structs (AC#3)**
  - [x] Chose the **`.async_store()` extension** (`AsyncStorageExt for Arc<dyn StorageBackend>`) over changing struct field types — zero churn to `ChirpstackPoller`/`CommandStatusPoller`/`CommandTimeoutHandler`/history-manager/`AppState` fields, constructors, or the ~30 `Arc::new(InMemoryBackend::new())` test fixtures. Single source of truth (`AsyncStorage` methods) reached via `.async_store()`.
- [x] **Task 3 — Convert async call sites (AC#3)**
  - [x] `src/chirpstack.rs` poller + command-poller sites → `.await` (made `capture_error_event`, `check_and_execute_prune`, `prepare_metric_for_batch` `async`; scoped the prune std-`MutexGuard`s so they don't cross the await — `!Send` fix).
  - [x] `src/chirpstack_events.rs`: made `filter_fresher_writes`, `ingest_event`, `handle_ack` `async`; converted their storage calls; updated the 4 sync `#[test]` ack tests to `#[tokio::test] async` + `.await`.
  - [x] `src/web/api.rs` async handlers → `.await`.
  - [x] `src/opc_ua_history.rs:320` `query_metric_history` (inside `async fn history_read_raw_modified`) → `.await`.
- [x] **Task 4 — Assess and handle sync call sites (AC#5, AC#6)**
  - [x] `src/opc_ua.rs` storage sites (`get_value`'s `get_metric_value`, `recent_commands`, `get_gateway_health_metrics`, `queue_command`) are **sync async-opcua callbacks** — wrapped in `run_blocking_storage()` (uses `block_in_place` on a multi-thread worker, runs inline in non-runtime/sync-test contexts so the direct `get_value` unit tests don't panic). Documented per site.
  - [x] `src/main.rs` boot calls use the concrete `SqliteBackend` sequentially before tasks spawn — left as direct sync calls per AC#6.
  - [x] `ChirpstackPoller::store_metric` (legacy, **no production callers**) left untouched.
- [x] **Task 5 — Tests (AC#7, AC#8)**
  - [x] Added `facade_runs_storage_off_the_async_worker` (AC#7, single-worker runtime proves storage runs off the worker) + `facade_preserves_backend_semantics`.
  - [x] `cargo test`: **1803 passed / 0 failed / 74 ignored** across 39 binaries. `cargo clippy --all-targets -- -D warnings`: clean.
- [x] **Task 6 — Docs & issue close (AC#8)**
  - [x] README "Current Version" note + Planning row updated for Epic H / H-0.
  - [x] Implementation commit references `Closes #73`.

## Dev Notes

### Architecture & constraints

- **Runtime:** `#[tokio::main]` with no flavor → multi-threaded runtime. This is what makes both `spawn_blocking` (Task 1) and `block_in_place` (AC#5 fallback) viable. Do **not** assume a current-thread runtime.
- **Two distinct "storage" concepts — do not conflate them.** There is the `StorageBackend` trait (`Arc<dyn StorageBackend>`, SQLite/in-memory) that this story targets, **and** a legacy in-memory `Storage` behind `Arc<Mutex<Storage>>` used in `src/main.rs`/`src/opc_ua.rs` for the address-space/name map. H-0 is **only** about `StorageBackend`. The `Arc<Mutex<Storage>>` mutex is out of scope.
- **`spawn_blocking` ownership:** the closure must be `'static`, so every borrowed argument (`&str`, `&MetricType`, `&CommandFilter`, `&ErrorEvent`, …) is converted to an owned value at the facade boundary. `MetricType`, `Command`, `DeviceCommand`, `ErrorEvent` are already `Clone`/owned-friendly — verify each.
- **Error mapping:** `spawn_blocking(...).await` yields `Result<T, JoinError>`. A `JoinError` only occurs on panic or cancellation of the blocking task. Map it to `OpcGwError::Storage("storage task failed: …")`. Never `.expect()`/`.unwrap()` it in production paths.
- **Do not change** `OpcGwError` semantics, the SQL, the pool, or the retry-backoff logic. The retry sleeps stay `std::thread::sleep` (AC#4) — they are now correctly off-runtime.

### Authoritative call-site enumeration (do NOT trust the line numbers above)

The line numbers in the ACs are a 2026-06-29 snapshot and **will drift** as you edit. Before and during the work, regenerate the authoritative set so no site is missed (a partial conversion that leaves some calls blocking is the main failure mode here):

```bash
# Every place a StorageBackend/handle method is invoked (production, excl. tests):
grep -rnE '\b(backend|storage|self\.backend|self\.storage|state\.backend|inner)\.[a-z_]+\(' src/ \
  | grep -vE '_tests\.rs|#\[cfg\(test|mod tests|InMemoryBackend::new|\.clone\(\)|\.lock\(\)'
```

Then, for each hit, classify the enclosing fn as **async** (→ facade + `.await`) or **sync** (→ AC#5 rule). Methods to expect include the metric path (`get_metric_value`, `upsert_metric_value`, `append_metric_history`, `batch_write_metrics`, `load_all_metrics`, `prune_metric_history`, `query_metric_history`), the command path (`get_pending_commands`, `enqueue_command`, `dequeue_command`, `mark_command_sent/confirmed/failed`, `find_command_by_result_id`, `find_pending_confirmations`, `find_timed_out_commands`, `recent_commands`, `queue_command`, `update_command_status`), status/health (`get_status`, `update_status`, `update_gateway_status`, `get_gateway_health_metrics`), and error events (`record_error_event`, `recent_error_events`). **Pay special attention** to the command-timeout / confirmation handler paths (`find_timed_out_commands` / `find_pending_confirmations`) which run on their own async interval task and are easy to miss.

### The sync-callback wrinkle (most important guardrail)

`src/opc_ua.rs` already does `let storage_clone = self.storage.clone();` before several calls and then invokes `storage.get_metric_value(...)`, `storage.recent_commands(...)`, `storage.get_gateway_health_metrics(...)`, `storage.queue_command(...)`. These run inside **async-opcua node-manager / method callbacks**. Before converting any of them, the dev MUST determine whether the enclosing callback is `async fn` (then `.await` the facade) or a **sync** trait method (then `.await` is impossible — use `block_in_place` + facade, or keep the direct sync call with a documented rationale). Blindly adding `.await` will not compile in sync callbacks; blindly leaving them defeats the purpose in async ones. This per-site judgement is the core engineering content of the story — record the decision per site in Completion Notes.

### Source tree — files to touch

- **New:** `src/storage/async_facade.rs` (or facade in `src/storage/mod.rs`).
- **Modify:** `src/storage/mod.rs` (export facade), `src/chirpstack.rs`, `src/chirpstack_events.rs`, `src/web/api.rs`, `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/main.rs`, and the test fixtures that construct the owning structs.
- **Unchanged (strict):** `src/storage/sqlite.rs` (except the two retry sleeps stay as-is — no edit needed), `src/storage/memory.rs`, `src/storage/types.rs`, `src/storage/schema.rs`, `src/storage/pool.rs`, the `StorageBackend` trait method signatures.

### Testing standards

- Project uses `cargo test` (unit tests in-module under `#[cfg(test)]` + integration tests in `tests/`). Match the existing pattern.
- tmpfs disk-quota gotcha for SQLite tests: if `protoc`/build hits `Disk quota exceeded`, export `TMPDIR=/home/gcorbaz/.cache/cargo-tmp` (see project memory `reference_cargo_tmpfs_workaround`).
- AC#7 test idea: spawn a long storage call via the facade and, concurrently, a cheap async task incrementing a counter, on `worker_threads = 1`; assert the counter advances while the storage call is outstanding (proves the SQL ran on the blocking pool, not the single worker).

### Why this class of bug matters (review-discipline note)

Runtime-blocking defects are **not caught by the adversarial code-review layers or the unit suite** — they surface only under real concurrency/load (cf. project memory `incident_main_deadlock_2026_05_20` and the #146 onboarding bug, both caught only by running the real binary). Expect ≥2 code-review iterations; the iter-N+1 doctrine applies because Task 1 introduces brand-new flow-control (the facade + JoinError mapping) and Task 4 is judgement-heavy.

### Project Structure Notes

- Follows the lettered-epic convention (Epic H, story key `H-0-async-storage-facade`), registered in `_bmad-output/planning-artifacts/epics.md` (§ "Epic H: Runtime Correctness & Tech-Debt") and `_bmad-output/implementation-artifacts/sprint-status.yaml`.
- SPDX headers (`MIT OR Apache-2.0`) + copyright `(c) [2024] Guy Corbaz` required on the new source file.
- No new crate dependencies expected (`tokio` is already a dependency with the needed features).

### References

- [Source: GitHub issue #73 — rescoped 2026-06-29 to the sync-rusqlite-on-async bug]
- [Source: src/storage/mod.rs:182 — `StorageBackend` trait definition (sync, ~30 methods)]
- [Source: src/storage/sqlite.rs:1265, :1346 — `std::thread::sleep` pool-retry backoffs]
- [Source: src/chirpstack.rs:412 — `ChirpstackPoller.backend: Arc<dyn StorageBackend>`]
- [Source: src/opc_ua.rs:1347/:1558/:1655/:2075 — OPC UA callback storage call sites (sync-context wrinkle)]
- [Source: _bmad-output/planning-artifacts/epics.md#Epic H: Runtime Correctness & Tech-Debt]
- [Source: project memory incident_main_deadlock_2026_05_20 — runtime defects evade review layers]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context)

### Debug Log References

- Initial Send-error: making `check_and_execute_prune` async surfaced `future cannot be sent between threads safely` (`tokio::spawn` in `main.rs:389`) because two `std::sync::MutexGuard`s (`last_prune_time`, `prune_retry_state`) were held across the new `spawn_blocking` await. Fixed by scoping the guards (interval gate + backoff gate) so they drop before the await, then re-locking after to update state — safe because `&mut self` guarantees no concurrent prune. Behaviour preserved.
- `cargo clippy --all-targets -- -D warnings`: clean (one transient `unused_import: AsyncStorage` resolved by not re-exporting the type name at `storage` root — callers reach it via `.async_store()`).
- `cargo test`: 1803 passed / 0 failed / 74 ignored across 39 test binaries.

### Completion Notes List

- **Design**: `AsyncStorage` facade (`src/storage/async_facade.rs`) wraps one `Arc<dyn StorageBackend>`; 19 async methods each run the sync backend call on `tokio::task::spawn_blocking`. Reached at call sites via the `AsyncStorageExt::async_store()` extension on `Arc<dyn StorageBackend>` — this avoided changing any struct field type, constructor signature, or test fixture (the lowest-blast-radius way to satisfy AC#1–3).
- **No behavioural change (AC#2)**: identical return types, `OpcGwError` mapping, and ordering; the facade only changes *where* the blocking work runs. `JoinError` (panic/cancel of the blocking task) maps to `OpcGwError::Storage`, never `.unwrap()`'d.
- **Sync OPC UA callbacks (AC#5)**: `src/opc_ua.rs` read/method callbacks are sync `Fn` closures that cannot `.await`; their blocking storage calls are wrapped in `run_blocking_storage()`, which uses `block_in_place` only on a multi-thread worker and runs inline otherwise (so the direct-call `get_value` unit tests and any current-thread context never panic).
- **AC#4**: the two `std::thread::sleep` pool-retry backoffs (`sqlite.rs:1265`, `:1346`) now execute inside `spawn_blocking`, off the async workers; left as blocking sleeps (correct there) per AC#4 — `sqlite.rs` unchanged.
- **AC#6 / legacy**: `main.rs` boot calls (concrete `SqliteBackend`, sequential pre-task) left direct; `ChirpstackPoller::store_metric` has no production callers (legacy) and was left untouched.
- **AC#7**: `facade_runs_storage_off_the_async_worker` runs on a `worker_threads = 1` runtime and asserts a cooperative ticker task completes concurrently with 20 facade storage calls — only possible because the storage ran on the blocking pool, not the single worker.

### File List

- `src/storage/async_facade.rs` — **new**: `AsyncStorage` facade (19 async methods) + `AsyncStorageExt::async_store()` + `run_blocking_storage()` helper + `join_err` + 2 unit tests (AC#7).
- `src/storage/mod.rs` — register `pub mod async_facade`; `pub use async_facade::{run_blocking_storage, AsyncStorageExt}`.
- `src/chirpstack.rs` — `use ...AsyncStorageExt`; `capture_error_event`/`check_and_execute_prune`/`prepare_metric_for_batch` → `async`; prune guard-scoping (`!Send` fix); poller + `deliver_command` + `CommandStatusPoller`/`CommandTimeoutHandler` storage calls → facade `.await`; updated the 2 internal callers + 1 async test caller of the now-async fns.
- `src/chirpstack_events.rs` — `use ...AsyncStorageExt`; `filter_fresher_writes`/`ingest_event`/`handle_ack` → `async` + facade `.await`; live-pump callers `.await`; 4 ack `#[test]` → `#[tokio::test] async` + `.await`.
- `src/web/api.rs` — `use crate::storage::AsyncStorageExt`; `api_status`/`api_errors`/`api_devices` storage reads → facade `.await`.
- `src/opc_ua_history.rs` — `use ...AsyncStorageExt`; `query_metric_history` (in `async fn history_read_raw_modified`) → facade `.await`.
- `src/opc_ua.rs` — `use ...run_blocking_storage`; 4 sync-callback storage calls wrapped in `run_blocking_storage()` (AC#5).
- `README.md` — Current Version note + Planning row for Epic H / H-0.
- `_bmad-output/planning-artifacts/epics.md` — Epic H section (added at story creation).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — Epic H + H-0 registration / status transitions.

## Change Log

- 2026-06-30 — H-0 implemented: async storage facade (`spawn_blocking` boundary) over the synchronous `StorageBackend`; all production async call sites converted to `.await`; sync OPC UA callbacks wrapped in `block_in_place`-safe helper. 1803 tests pass, clippy `-D warnings` clean. Closes #73.

## Completion Note (story creation)

Ultimate context-engine analysis completed — comprehensive developer guide created. Epic H (Runtime Correctness & Tech-Debt) opened to host this and future v2.x tech-debt stories (#110, #79, substring-matcher codification are candidate follow-ons). Owner pre-decided Strategy A (async `spawn_blocking` facade) and the full BMad story flow. The central engineering risk is the async-vs-sync calling-context audit in `src/opc_ua.rs` (AC#5) — flagged prominently so the dev does not blindly `.await` sync OPC UA callbacks.

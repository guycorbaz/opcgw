---
title: 'GH-74 Startup validation for the metric_history index'
type: 'bugfix'
created: '2026-06-26'
status: 'done'
baseline_commit: 'e953fcce4369c5b097159a05759fc37b8fb02a5b'
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The `idx_metric_history_device_timestamp` index is created by migration (`migrations/v001_initial.sql`, re-asserted in `v008`) but the gateway never verifies at startup that it actually exists. If the index is dropped or a migration partially fails, time-range history queries silently fall back to full-table scans — a performance cliff with no operator-visible signal.

**Approach:** After migrations run, query `sqlite_master` for the index by name. If it is absent, emit a single loud, structured `warn!` (with a remediation hint) so the degradation is observable. Do not fail startup — a missing performance index must not take the gateway down.

## Boundaries & Constraints

**Always:** Run the check on every storage-init path that runs migrations (centralize it inside `run_migrations`, before its final `Ok(())`, so `SqliteBackend::new` / `with_pool` / `new_with_initialization` all inherit it). Use the existing `tracing` `warn!` with a stable `event=` marker and a `recommended_action` field, matching the project's structured-log style. SPDX headers and doc comments on any new public item.

**Ask First:** Promoting the missing-index condition from WARN to a hard startup error (the issue scopes this as WARN-only; escalation is a separate decision).

**Never:** Recreating/repairing the index automatically (out of scope — migrations own schema creation). Adding a new migration. Changing the integrity-check flow in `pool.rs`. Querying `sqlite_master` on a hot per-query path (this is startup-only).

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Index present (normal) | Fresh/migrated DB after `run_migrations` | Validation passes; no WARN emitted; startup proceeds | N/A |
| Index missing | Index dropped or migration partial | One `warn!` with `event="metric_history_index_missing"` + remediation; startup still succeeds | Logged, non-fatal |
| `sqlite_master` query fails | Query itself errors | Propagate as `OpcGwError::Database` (consistent with surrounding migration error handling) | Return Err |

</frozen-after-approval>

## Code Map

- `src/storage/schema.rs` -- `run_migrations` (line ~53, returns `Ok(())` at ~368); add the index-existence check before the return. Index name string lives here.
- `migrations/v001_initial.sql` (line 75) / `migrations/v008_typed_value_constraints.sql` (line 116) -- canonical `CREATE INDEX IF NOT EXISTS idx_metric_history_device_timestamp` definitions (read-only reference for the exact name).
- `src/storage/sqlite.rs` -- `with_pool` (line ~501) calls `schema::run_migrations`; no change needed, inherits the check.

## Tasks & Acceptance

**Execution:**
- [x] `src/storage/schema.rs` -- Added `const METRIC_HISTORY_INDEX_NAME` and private `fn validate_required_indexes(conn) -> Result<(), OpcGwError>` (sqlite_master count query; count 0 → structured `warn!` with `event="metric_history_index_missing"` + `recommended_action`; query error → `OpcGwError::Database`). Called from `run_migrations` before the final `Ok(())`. Doc comments + SPDX preserved.
- [x] `src/storage/schema.rs` (tests module) -- Added `test_validate_required_indexes_present_after_migration` (index present → Ok, no warn) and `test_validate_required_indexes_warns_when_missing` (index dropped → Ok + `metric_history_index_missing` WARN via `#[traced_test]` + `logs_contain`). Both pass.
- [x] `README.md` -- Noted the startup index-integrity check in the Logging section (doc-sync rule).
- [x] `_bmad-output/implementation-artifacts/sprint-status.yaml` -- N/A: GH-74 is a tracker bug, not an epic story; no `development_status` entry exists → documented skip per sync-sprint-status precondition.

**Acceptance Criteria:**
- Given a normally migrated database, when the gateway starts, then no index-related WARN is logged and startup completes.
- Given the `idx_metric_history_device_timestamp` index is absent, when the gateway starts, then exactly one `warn!` carrying `event="metric_history_index_missing"` and a remediation hint is logged and startup still succeeds.
- Given a missing index, when validation runs, then the gateway does NOT attempt to recreate it and does NOT abort.

## Spec Change Log

- **iter-1 review (2026-06-26):** No intent_gap/bad_spec — code is faithful to the frozen I/O matrix. Two patches applied (no loopback): (1) **MEDIUM** (Edge Case Hunter) — the `validate_required_indexes` doc comment over-promised "never take the service down" while the spec-approved query-error path returns `Err`; reworded the doc to distinguish a *missing* index (non-fatal warn) from a *failed `sqlite_master` lookup* (fatal `OpcGwError::Database`, consistent with all migration steps). Behavior unchanged — matches the frozen matrix. (2) **LOW** (Blind Hunter) — added `AND tbl_name='metric_history'` to the existence query for catalog precision. Remaining findings (plural-naming, test-assertion brittleness, exactly-one cardinality not pinned, redundant existence re-query) accepted as LOW; no new logic branches introduced, loop terminates at LOW.

## Verification

**Commands:**
- `cargo test --lib storage::schema` -- expected: new index-validation tests pass. ✅ 27/27.
- `cargo test` -- expected: full suite green, no regressions. ✅ lib 630/0 + all integration suites pass.
- `cargo clippy --all-targets -- -D warnings` -- expected: clean. ✅

## Suggested Review Order

**Validation logic (entry point)**

- Where the check is wired into startup — runs once per `run_migrations`, just before success.
  [`schema.rs:378`](../../src/storage/schema.rs#L378)

- The validator itself: name+table existence query, non-fatal warn on absence, fatal only on catalog read failure.
  [`schema.rs:402`](../../src/storage/schema.rs#L402)

- The operator-facing structured warning (stable `event=` marker + remediation).
  [`schema.rs:422`](../../src/storage/schema.rs#L422)

- The single source of truth for the index name.
  [`schema.rs:21`](../../src/storage/schema.rs#L21)

**Tests & docs (supporting)**

- Present-path: index exists → validation silent.
  [`schema.rs:505`](../../src/storage/schema.rs#L505)

- Missing-path: index dropped → non-fatal + `metric_history_index_missing` warning asserted.
  [`schema.rs:533`](../../src/storage/schema.rs#L533)

- Operator doc note (event name/field matches the code).
  [`README.md:408`](../../README.md#L408)

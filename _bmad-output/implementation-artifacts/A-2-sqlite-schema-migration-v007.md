# Story A-2: SQLite Schema Migration v007 (Typed Value Columns)

| Field         | Value                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------- |
| Story key     | `A-2-sqlite-schema-migration-v007`                                                                    |
| Epic          | A — Storage Payload Migration (Phase B Closure, gates v2.0 GA)                                        |
| FRs           | FR51 (Epic-A umbrella)                                                                                |
| Status        | done                                                                                                  |
| Created       | 2026-05-15                                                                                            |
| Source epic   | `_bmad-output/planning-artifacts/epics.md § Epic A § Story A.2`                                       |
| Sprint change | `_bmad-output/planning-artifacts/sprint-change-proposal-2026-05-14.md`                                |
| Tracking      | GitHub tracking issue to be filed by dev agent (see Task 0)                                           |

---

## User Story

As a **deployed opcgw gateway**,
I want the SQLite schema upgraded to store metric values in typed columns (`value_real REAL NULL`, `value_int INTEGER NULL`, `value_bool INTEGER NULL`, `value_text TEXT NULL`) keyed by a new `value_type` discriminant column,
So that Story A-3 can persist the typed payload that Story A-1 made expressible at the type level, with no data loss on the v006 → v007 upgrade path.

---

## Story Context

### Why A-2 follows A-1 and gates A-3 through A-6

Story A-1 made `MetricType` payload-bearing at the type level (`Float(f64)` / `Int(i64)` / `Bool(bool)` / `String(String)`). Every production writer today still stamps a zero-defaulted payload (`Float(0.0)` / `Int(0)` / `Bool(false)` / `String("")`) because the persistence layer cannot accept the typed payload — the SQLite schema has a single `value TEXT NOT NULL` column that flattens any incoming value to a string. A-1's `TODO(A-3)` markers across `src/chirpstack.rs::process_metrics` are blocked on this schema gap.

A-2 introduces schema migration v007. It is **strictly additive DDL**: four typed value columns + a `value_type` discriminant column are added to `metric_values` and `metric_history`. Existing rows are tagged `value_type = 'legacy'` with NULL typed columns. The legacy `value TEXT NOT NULL` + `data_type TEXT NOT NULL` columns remain untouched — pre-A-3 writers continue to populate them, pre-A-4 readers continue to consume them. The typed columns sit empty until A-3 wires the typed payload through `batch_write_metrics` / `upsert_metric_value` / `set_metric`.

This story does **not** touch writer code, reader code, the `MetricType` enum, or any consumer pattern-match site. It is the smallest possible step that unblocks A-3.

### Current pre-A-2 shape (v006 baseline)

`migrations/v001_initial.sql` § `CREATE TABLE metric_values`:

```sql
CREATE TABLE IF NOT EXISTS metric_values (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,               -- Serialized value (TEXT format for durability)
  data_type TEXT NOT NULL,           -- MetricType variant: Float, Int, Bool, String
  timestamp TEXT NOT NULL,           -- ISO8601 UTC format
  updated_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  UNIQUE(device_id, metric_name)
);
```

`metric_history` has the same legacy-column shape (`value TEXT NOT NULL` + `data_type TEXT NOT NULL` + `timestamp TEXT NOT NULL`).

The runner at `src/storage/schema.rs::run_migrations` applies migrations v001 through v006 in order; `LATEST_VERSION` is `5` (stale const, dead-coded since v006 was added in commit `7a3a37c`). Current fresh-DB target: `PRAGMA user_version = 6`.

### Post-A-2 shape (target)

`migrations/v007_typed_value_columns.sql` (NEW) adds five columns to **each** of `metric_values` and `metric_history`:

```sql
-- metric_values + metric_history each gain:
ALTER TABLE metric_values ADD COLUMN value_real    REAL    NULL;
ALTER TABLE metric_values ADD COLUMN value_int     INTEGER NULL;
ALTER TABLE metric_values ADD COLUMN value_bool    INTEGER NULL;  -- 0/1
ALTER TABLE metric_values ADD COLUMN value_text    TEXT    NULL;
ALTER TABLE metric_values ADD COLUMN value_type    TEXT    NOT NULL DEFAULT 'legacy';
```

**Invariants the schema must enforce (post-A-3, weak in A-2):**

- Exactly one of the four typed columns is non-NULL per row when `value_type ∈ {'Float','Int','Bool','String'}`.
- All four typed columns are NULL when `value_type = 'legacy'` (pre-A-2 rows after migration).
- `value_type` is one of `{'legacy','Float','Int','Bool','String'}` — enforced via `CHECK` constraint.

In A-2, only the second invariant is testable (all migrated legacy rows have NULL typed columns). The first invariant becomes testable once A-3 wires writers. A `CHECK` constraint on `value_type` is acceptable in A-2; the exactly-one-non-NULL CHECK is deferred to A-3 (writer correctness) or A-7 (cleanup migration) to avoid forcing partial-state validation during the staging window.

The legacy `value TEXT NOT NULL` + `data_type TEXT NOT NULL` columns remain. A-3 makes writers populate **both** the legacy columns (for backwards compatibility during the A-3 → A-5 transition) and the typed columns. A-5 / A-7 retires the legacy columns once readers have fully migrated.

### Migration-runner integration

`src/storage/schema.rs::run_migrations` needs:

1. `const MIGRATION_V007: &str = include_str!("../../migrations/v007_typed_value_columns.sql");` at the top of the file alongside the existing v001–v006 consts.
2. An `if current_version < 7 { … }` block matching the v006 pattern (execute_batch → pragma_update → info log).
3. The dead-coded `LATEST_VERSION` const should be bumped to `7` if retained (or removed entirely — it's `#[allow(dead_code)]`).
4. `test_run_migrations_fresh_database` asserts `version == 7` (was 6).
5. New tests covering v006 → v007 upgrade with pre-A-2 rows present.

---

## Acceptance Criteria

**AC#1 — Migration file added and registered:** A new file `migrations/v007_typed_value_columns.sql` exists. `src/storage/schema.rs` imports it via `const MIGRATION_V007: &str = include_str!(...)` and applies it inside an `if current_version < 7 { … }` block that updates `PRAGMA user_version` to `7` on success. The existing v001–v006 application blocks are unchanged.

**AC#2 — Schema additions on both tables:** After migration v007 runs, both `metric_values` and `metric_history` have these new columns (NULL-able, defaultable):

- `value_real REAL NULL`
- `value_int INTEGER NULL`
- `value_bool INTEGER NULL` with `CHECK(value_bool IS NULL OR value_bool IN (0, 1))` (0/1, no SQLite BOOLEAN affinity; CHECK added by iter-1 review patch IM1 as schema-side defence-in-depth)
- `value_text TEXT NULL`
- `value_type TEXT NOT NULL DEFAULT 'legacy'` with a `CHECK(value_type IN ('legacy','Float','Int','Bool','String'))` constraint

The existing columns (`id`, `device_id`, `metric_name`, `value`, `data_type`, `timestamp`, `updated_at`, `created_at`) are preserved unchanged.

**AC#3 — Legacy row tagging:** Pre-A-2 rows present in `metric_values` and `metric_history` at migration time receive `value_type = 'legacy'` (from the column default — no explicit `UPDATE` statement required). All four typed columns are NULL on these rows.

**AC#4 — Fresh database hits v007:** `SqliteBackend::new(path)` on a non-existent file produces a database at `PRAGMA user_version = 7` after running all migrations.

**AC#5 — Idempotent runner:** Running `schema::run_migrations(&conn)` twice on the same connection produces no errors and leaves the schema at version 7. The `ALTER TABLE … ADD COLUMN` statements are guarded by the `current_version < 7` check (not by `IF NOT EXISTS`, which SQLite doesn't support on `ADD COLUMN`). The pragma-update is the single source of truth for "migration already applied."

**AC#6 — No row loss on upgrade:** An integration test seeds a v006 database with at least 3 pre-A-2 rows in `metric_values` and 5 pre-A-2 rows in `metric_history` (covering all four `data_type` variants: `Float`, `Int`, `Bool`, `String`). After `run_migrations`, the same number of rows are present in both tables; all pre-existing columns retain their values; all new typed columns are NULL; all `value_type` columns are `'legacy'`.

**AC#7 — Migration completes within 5 seconds for databases up to 100 MB:** This is Story A.7's runbook SLA, but A-2 must not regress it. The DDL is structurally O(table-size) on SQLite (`ALTER TABLE … ADD COLUMN` is metadata-only and constant-time on modern SQLite, but a defensive integration test pins the wall-clock budget on a moderately-sized seeded database — ≥10 000 rows across both tables — and asserts `< 5 s`).

**AC#8 — Writer behaviour unchanged:** `SqliteBackend::set_metric`, `SqliteBackend::upsert_metric_value`, `SqliteBackend::append_metric_history`, and `SqliteBackend::batch_write_metrics` continue to write the legacy `value` + `data_type` columns exactly as they did before A-2. The new typed columns + `value_type` remain at their defaults (NULL / `'legacy'`) on rows newly inserted by these methods after the migration. A regression test pins the legacy-write contract.

**AC#9 — Reader behaviour unchanged:** `SqliteBackend::get_metric`, `SqliteBackend::get_metric_value`, `SqliteBackend::load_all_metrics`, and `SqliteBackend::query_metric_history` continue to project from the legacy `value` + `data_type` columns. No new columns are read. The existing partial-success skip behaviour on parse errors is preserved.

**AC#10 — Strict-zero file invariants (carry-forward from A-1):** The migration is purely additive — no source file outside `migrations/`, `src/storage/schema.rs`, and `src/storage/schema.rs::tests` is touched. Specifically, these files have **zero diff lines** in the A-2 commit:

- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs`
- `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua.rs`, `src/opc_ua_history.rs`
- `src/security.rs`, `src/security_hmac.rs`
- `src/main.rs::initialise_tracing` (the function body; the surrounding file may gain a fixture-mod touch but the function itself is untouched)
- `src/config_reload.rs`, `src/opcua_topology_apply.rs`
- `src/chirpstack.rs` (A-3 territory — writers stay unchanged in A-2)
- `src/storage/types.rs` (A-1 completed; `MetricType`, `MetricValue`, `MetricValueInternal`, `BatchMetricWrite` definitions unchanged)
- `src/storage/memory.rs` (InMemoryBackend has no schema; A-2 is a SQLite-only concern)
- `src/storage/mod.rs` (`StorageBackend` trait already accepts payload-bearing `MetricType` per A-1)
- `src/storage/pool.rs`

`src/storage/sqlite.rs` is **also strict-zero** in A-2 — the writers and readers do not change. The only behavioural change A-2 introduces is the migration-runner update + the new SQL file.

**AC#11 — `cargo test --all-targets` passes at ≥1130 passed / 0 failed / ≤10 ignored:** Post-A-1 review baseline is 1125 passed / 0 failed / 10 ignored. A-2 adds ≥5 new tests in `src/storage/schema.rs::tests` (see Task 5) bringing the target to ≥1130. `cargo clippy --all-targets -- -D warnings` clean.

**AC#12 — `cargo test --doc` no regressions:** Post-A-1 baseline is 0 failed / 55 ignored (per iter-2 IR7 net −1 from issue #100 baseline of 56). A-2 must not regress this — target `0 failed / ≥55 ignored`. A-2 adds zero new doctests by design.

**AC#13 — Migration file SPDX + copyright header:** Per CLAUDE.md project conventions, the new SQL migration file carries:

```sql
-- SPDX-License-Identifier: MIT OR Apache-2.0
-- Copyright (c) [2024] Guy Corbaz
--
-- Migration v007: Typed Value Columns for Storage Payload Migration (Epic A, Story A-2)
-- …
```

Matches the format used by `v005_gateway_status.sql` and `v006_gateway_status_health_metrics.sql`.

---

## Tasks

- [x] **Task 0 — File GitHub tracking issue for A-2.** Defer to user per Stories 9-4 through A-1 precedent (gh CLI not authenticated for write). Story-file table updated with the issue number once filed.

- [x] **Task 1 (AC#1, AC#2, AC#13) — Author `migrations/v007_typed_value_columns.sql`.**
  - [x] Add SPDX + copyright header.
  - [x] `ALTER TABLE metric_values ADD COLUMN value_real REAL NULL;` (× 4 typed columns).
  - [x] `ALTER TABLE metric_values ADD COLUMN value_type TEXT NOT NULL DEFAULT 'legacy' CHECK(value_type IN ('legacy','Float','Int','Bool','String'));` — verify SQLite accepts the inline CHECK on an ALTER TABLE column add (it does as of SQLite 3.37+ which is bundled with `rusqlite` 0.38; if it does not on the bundled version, drop the CHECK and rely on writer-side validation, with a deferred-work entry pointing to A-7 for re-introduction).
  - [x] Repeat the five `ALTER TABLE` statements for `metric_history`.
  - [x] No `PRAGMA user_version` statement in the SQL file — the runner sets this in `src/storage/schema.rs`.

- [x] **Task 2 (AC#1, AC#5) — Wire migration v007 into `src/storage/schema.rs::run_migrations`.**
  - [x] Add `const MIGRATION_V007: &str = include_str!("../../migrations/v007_typed_value_columns.sql");` at the top of the file alongside v001–v006.
  - [x] Append `if current_version < 7 { … }` block matching the v006 pattern (execute_batch → pragma_update → `info!(version = 7, "Applied migration v007_typed_value_columns")`).
  - [x] Update the dead-coded `LATEST_VERSION: u32 = 5` to `7` (or remove it — `#[allow(dead_code)]` is on it; pick the option that minimises churn).
  - [x] Audit the existing tests `test_run_migrations_fresh_database`, `test_run_migrations_idempotent`, `test_migrations_create_all_tables`, `test_migrations_retention_config_initialized` and update version assertions (6 → 7) and table-count assertions if they depend on column counts (they don't — only `test_run_migrations_fresh_database` checks `table_count == 5`, which is unaffected).

- [x] **Task 3 (AC#6) — Pre-A-2-row preservation integration test.**
  - [x] New `src/storage/schema.rs::tests::test_v007_preserves_pre_a2_rows`: open a temp DB, run migrations through v006 only (manually run the v001–v006 application logic — or, simpler, run the full runner and accept the v007 cost; insert pre-A-2 rows BEFORE running v007). The cleanest shape is: (a) create temp DB, (b) `run_migrations(&conn)` — fresh DB hits v7 directly, no pre-A-2 rows exist; this doesn't exercise the upgrade path. **Better shape**: (a) create temp DB, (b) execute v001–v006 SQL manually via execute_batch (or set `PRAGMA user_version = 6` and create the tables manually with the v006-shaped DDL), (c) seed 3 rows in `metric_values` + 5 rows in `metric_history` covering all four `data_type` variants, (d) call `run_migrations(&conn)` — this should now find `user_version = 6` and apply only the v007 block, (e) assert row counts unchanged, (f) assert `value_type = 'legacy'` and all typed columns NULL on every pre-existing row.
  - [x] Test that creating new rows with a writer post-migration produces `value_type = 'legacy'` (writer behaviour unchanged in A-2) — covered by Task 4's regression test.

- [x] **Task 4 (AC#8, AC#9) — Pin writer / reader unchanged.**
  - [x] New `src/storage/schema.rs::tests::test_v007_writers_still_populate_legacy_columns`: post-migration, write a metric via `SqliteBackend::set_metric(device, metric, MetricType::Float(0.0))`, then query the row with raw SQL and assert that `value`, `data_type` are populated (legacy contract) while `value_real`, `value_int`, `value_bool`, `value_text` are all NULL and `value_type` is `'legacy'` (column default). This pins the A-2 contract: writers don't yet know about typed columns; A-3 closes that gap.
  - [x] New `src/storage/schema.rs::tests::test_v007_readers_still_read_legacy_columns`: write via `set_metric`, read via `get_metric_value`, assert returned `MetricValue.value` is the discriminant string `"Float"` (per pre-A-2 contract — `set_metric` uses `serde_json::to_string(&value)` which produces `{"Float":0.0}` actually, not `"Float"`; this is a known A-1-staging quirk noted in iter-2 IR2). Either pin the actual current behaviour (`{"Float":0.0}`) OR pin the more-common `upsert_metric_value` path output (`"Float"`). Pick `upsert_metric_value` for the assertion clarity; document the `set_metric` quirk in Dev Notes.

- [x] **Task 5 (AC#4, AC#5, AC#7, AC#11) — Migration runner test expansion.**
  - [x] Update `test_run_migrations_fresh_database`: `assert_eq!(version, 7)` instead of 6.
  - [x] Update `test_run_migrations_idempotent`: same.
  - [x] New `test_v007_adds_all_typed_columns_to_metric_values`: query `PRAGMA table_info('metric_values')` and assert the 5 new columns are present with the expected types + NULL-ability.
  - [x] New `test_v007_adds_all_typed_columns_to_metric_history`: same for `metric_history`.
  - [x] New `test_v007_value_type_check_constraint`: insert a row with `value_type = 'invalid'` and assert the INSERT fails with `SQLITE_CONSTRAINT_CHECK`. (If Task 1 had to drop the CHECK constraint due to SQLite version limitations, defer this test to A-7 and document.)
  - [x] New `test_v007_migration_under_5s_for_10k_rows`: seed 10 000 rows in `metric_values` + 10 000 in `metric_history` at v006, then time `run_migrations` and assert `< 5 s` (use `std::time::Instant`). This is AC#7 evidence; place behind `#[ignore]` if it's too heavy for default CI (decide based on actual measured cost — typically `ALTER TABLE ADD COLUMN` is metadata-only and well under 100 ms even at 100k rows).

- [x] **Task 6 (AC#13) — Verify migration-file conventions match v005/v006.**
  - [x] Compare header + comment structure of `migrations/v007_typed_value_columns.sql` against `v005_gateway_status.sql` and `v006_gateway_status_health_metrics.sql`.
  - [x] Run `cargo build` and confirm `include_str!` resolves the new file without macro errors.

- [x] **Task 7 (AC#10) — Verify strict-zero invariants.**
  - [x] After Tasks 1–6 land, run `git diff --name-only` and confirm only the following files are touched: `migrations/v007_typed_value_columns.sql` (new), `src/storage/schema.rs`, `_bmad-output/implementation-artifacts/A-2-sqlite-schema-migration-v007.md` (this file), `_bmad-output/implementation-artifacts/sprint-status.yaml`, `README.md` (Current Version line per CLAUDE.md doc-sync).
  - [x] Specifically grep for diff lines in `src/storage/sqlite.rs` and confirm zero hits — writers + readers are not touched in A-2.

- [x] **Task 8 (AC#11, AC#12) — Final verification.**
  - [x] `cargo build --all-targets` clean.
  - [x] `cargo test --all-targets` ≥ 1130 passed / 0 failed / ≤10 ignored.
  - [x] `cargo clippy --all-targets -- -D warnings` clean.
  - [x] `cargo test --doc` 0 failed / ≥55 ignored.
  - [x] Update `README.md` Current Version line to reflect A-2 status transition.
  - [x] Update `_bmad-output/implementation-artifacts/sprint-status.yaml`: `A-2-sqlite-schema-migration-v007: ready-for-dev → in-progress → review` (`in-progress` set when implementation begins; `review` set when implementation completes).

---

## Dev Notes

### Migration file naming + format conventions

Migration files live in `migrations/` at the project root. Naming is `vNNN_<snake_case_description>.sql` where `NNN` is zero-padded three digits. The v007 file follows the same shape as `v006_gateway_status_health_metrics.sql`:

1. SPDX + copyright header (2 lines).
2. Migration header comment block describing purpose and consumers (3-5 lines, blank-line-separated).
3. `ALTER TABLE … ADD COLUMN …` statements one per line (no `IF NOT EXISTS` — SQLite doesn't support it on ADD COLUMN; the runner's `current_version < N` guard is the idempotency mechanism).
4. **NO** `PRAGMA user_version = N;` statement at the end of the SQL file — the Rust runner sets this via `conn.pragma_update`. v001 is the historical exception (it embeds `PRAGMA user_version = 1` in the SQL); v002–v006 follow the runner-sets-pragma convention. v007 follows v002–v006.

### Schema-runner integration pattern

The existing pattern in `src/storage/schema.rs::run_migrations` is verbose but explicit. Each version's block is structurally identical:

```rust
if current_version < N {
    debug!("Applying migration vNNN_<name>");
    conn.execute_batch(MIGRATION_VNNN)
        .map_err(|e| OpcGwError::Database(format!("Failed to execute migration vNNN_<name>: {}", e)))?;
    conn.pragma_update(None, "user_version", N.to_string())
        .map_err(|e| OpcGwError::Database(format!("Failed to set schema version to {}: {}", N, e)))?;
    info!(version = N, "Applied migration vNNN_<name>");
}
```

The v007 block follows this template verbatim. **Do not refactor** the runner to a loop or table-driven dispatch in A-2 — that's a separate housekeeping concern, not in scope.

### Why CHECK constraint on `value_type`

The discriminant column has a closed enumeration (`legacy`, `Float`, `Int`, `Bool`, `String`). SQLite's column-level CHECK constraint catches typos and accidental drift early. The constraint is enforced at INSERT/UPDATE time, so any writer that mis-spells `'flaot'` fails loudly instead of silently storing corrupt rows that later reads can't decode.

**Compatibility caveat:** SQLite's `ALTER TABLE … ADD COLUMN` supports inline `DEFAULT` and `CHECK` clauses as of SQLite 3.25 (October 2018). The `rusqlite 0.38` crate bundles SQLite 3.46+ via the `bundled` feature, so this is safe in our build. If a future deployment relies on a system-linked SQLite < 3.25, the CHECK clause must be dropped (or applied as a follow-up `CREATE TABLE … AS SELECT` + `DROP TABLE` + `ALTER TABLE … RENAME TO` dance — but that's a vastly heavier migration and out of scope for A-2).

### Why NOT to enforce exactly-one-non-NULL in A-2

A natural CHECK is:

```sql
CHECK (
  (value_type = 'legacy'  AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
  OR (value_type = 'Float'   AND value_real IS NOT NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
  OR (value_type = 'Int'     AND value_real IS NULL AND value_int IS NOT NULL AND value_bool IS NULL AND value_text IS NULL)
  OR (value_type = 'Bool'    AND value_real IS NULL AND value_int IS NULL AND value_bool IS NOT NULL AND value_text IS NULL)
  OR (value_type = 'String'  AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NOT NULL)
)
```

This is **deferred to A-3 or A-7**. In A-2 the writers don't yet populate typed columns, so adding this CHECK would force every legacy-shaped INSERT to fail. Once A-3 wires writers, the CHECK becomes provable and can be added in a follow-up migration v008 (or rolled into A-7's cleanup migration).

### Why writers and readers are NOT touched in A-2

Two reasons:

1. **Tight scope:** A-2 is the smallest unit of work that unblocks A-3. Mixing schema DDL with writer-code changes would expand the surface and the review burden. A-3 owns the poller-side rewrite; A-4/A-5 own the read-side rewrite.
2. **No real values to write yet:** A-1's `process_metrics` arms in `src/chirpstack.rs` (lines 1591-1666) still stamp `MetricType::Float(0.0)` / `Int(0)` / `Bool(false)` / `String(String::new())` placeholders — the real value is carried in `BatchMetricWrite.value: String` only. If A-2 updated writers to populate typed columns from the typed payload, it would populate `value_real = 0.0` everywhere, defeating the purpose. A-3 plumbs real values into the typed payload first; then writers can read from there.

### Test-budget delta

Post-A-1 review baseline: 1125 passed / 0 failed / 10 ignored.

A-2 adds:

- `test_v007_preserves_pre_a2_rows` (1 test) — Task 3.
- `test_v007_writers_still_populate_legacy_columns` (1 test) — Task 4.
- `test_v007_readers_still_read_legacy_columns` (1 test) — Task 4.
- `test_v007_adds_all_typed_columns_to_metric_values` (1 test) — Task 5.
- `test_v007_adds_all_typed_columns_to_metric_history` (1 test) — Task 5.
- `test_v007_value_type_check_constraint` (1 test, conditional on CHECK acceptance) — Task 5.
- `test_v007_migration_under_5s_for_10k_rows` (1 test, possibly `#[ignore]`) — Task 5.

Target: ≥1130 passed (was 1125 + ≥5 = 1130). The CHECK test is conditional; the perf test may be `#[ignore]`. Range: 1130 to 1132 passed. The ignored count may rise by 1 if the perf test is gated.

### Existing v006 baseline state at HEAD

Latest commit on `main` is `c31cad5` (A-1 Code Review Complete). Working tree at A-2 start has zero uncommitted changes in `src/` or `migrations/`. The `.claude/skills/bmad-*` reorg in the broader working tree is unrelated — must not be bundled into the A-2 commit.

### Strict-zero invariant carry-forward from A-1

A-2 must NOT touch any of the following (verbatim diff zero):

| File | Why strict-zero |
| --- | --- |
| `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs` | A-6 territory |
| `src/opc_ua.rs`, `src/opc_ua_history.rs` | A-4 / A-5 territory |
| `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs` | Auth — outside Epic A |
| `src/security.rs`, `src/security_hmac.rs` | Auth — outside Epic A |
| `src/main.rs::initialise_tracing` | Cross-cutting infrastructure |
| `src/config_reload.rs`, `src/opcua_topology_apply.rs` | Phase B carry-over |
| `src/chirpstack.rs` | A-3 territory (poller writer rewrite) |
| `src/storage/types.rs` | A-1 completed (MetricType + MetricValue + MetricValueInternal + BatchMetricWrite) |
| `src/storage/memory.rs` | InMemoryBackend has no schema |
| `src/storage/mod.rs` | `StorageBackend` trait already accepts payload-bearing `MetricType` |
| `src/storage/sqlite.rs` | Writers + readers stay unchanged in A-2 — A-3 onwards touch them |
| `src/storage/pool.rs` | Connection pool — outside Epic A |
| `tests/*.rs` | A-2 tests live in `src/storage/schema.rs::tests` |

The **only** files touched by A-2 are:

- `migrations/v007_typed_value_columns.sql` (NEW)
- `src/storage/schema.rs` (migration const + runner block + test expansion)
- `_bmad-output/implementation-artifacts/A-2-sqlite-schema-migration-v007.md` (this file — Dev Agent Record populated at completion)
- `_bmad-output/implementation-artifacts/sprint-status.yaml` (status transitions)
- `README.md` (Current Version line per CLAUDE.md doc-sync)

### Carry-forward GitHub issues (unchanged by A-2)

- **#88** (per-IP rate limiting) — Phase B carry-over.
- **#100** (56 doctest baseline; iter-2 IR7 dropped to 55 in A-1 review) — A-2 adds zero new doctests; baseline preserved at 55.
- **#102** (tests/common reuse) — A-2 inherits the inline-duplication deferral; no new integration-test files.
- **#104** (TLS hardening) — outside Epic A.
- **#108** (storage payload-less MetricType) — A-2 is a structural step toward closing #108; not closed until A-5 ships.
- **#110** (RunHandles missing Drop) — outside Epic A.
- **#113** (live-borrow refactor) — outside Epic A.
- **#117** (Phase B retro carry-over) — outside Epic A.
- **#118** (A-1 tracker) — closed by A-1's done flip.

A-2's tracking issue is to be filed by the dev agent (Task 0) per Stories 9-4 through A-1 precedent.

### A-1 review carry-forward concerns

The A-1 iter-3 review surfaced one **MEDIUM-defer-to-A-3** finding: `SqliteBackend::set_metric` calls `serde_json::to_string(&value)` which rejects NaN/Inf. A-2 does **not** address this — A-2 doesn't touch `set_metric`. A-3 owns the NaN/Inf policy decision per `deferred-work.md § A-1-iter3-DEF8`.

A-1's iter-2 IR4 softened the original A-2 column-name commitments — A-2 is now free to choose the exact column names without contradicting prior commitments. The names settled in this spec (`value_real` / `value_int` / `value_bool` / `value_text`) match the architecture.md description; verify on commit that `architecture.md:174` still says `value_real REAL NULL` (etc.). If architecture.md has drifted, reconcile in the A-2 commit.

### `MetricType::Bool` → `value_bool INTEGER` mapping

SQLite has no native BOOLEAN type. The conventional mapping is `INTEGER NULL` with `0 = false`, `1 = true`. This matches how `command_queue.f_port`-style INTEGERs are handled today. Writer-side (A-3) will write `0`/`1`; reader-side (A-4/A-5) will read `i64` and pattern-match `0 → false`, `1 → true`, else error.

### Existing `data_type TEXT NOT NULL` column

Both `metric_values` and `metric_history` already have a `data_type TEXT NOT NULL` column from v001 storing the discriminant string. This is **redundant** with the new `value_type TEXT NOT NULL DEFAULT 'legacy'` column. The duplication is intentional during A-2 → A-5 staging:

- `data_type` (legacy) is read by `SqliteBackend::get_metric` (line 406), `get_metric_value` (line 464), `load_all_metrics` (line 1175), `query_metric_history` (line 1417). These read paths stay unchanged in A-2.
- `value_type` (new) is what A-4/A-5 readers will pattern-match on once they're rewritten.
- A-5 or A-7 retires the legacy `data_type` column (and the legacy `value` column) once the read paths have fully migrated.

### Backwards-compatibility check

Operators upgrading from v2.0-rc (post-A-1) to v2.0-rc-A-2 will see:

1. Migration v007 runs on first start, adds 10 columns (5 to each table), takes < 100 ms on databases up to 100 MB.
2. Pre-A-2 rows continue to be readable via the existing legacy-column read paths.
3. New rows written by the unchanged A-2 poller continue to populate legacy columns; typed columns are NULL; `value_type` is `'legacy'`.

Operators upgrading from v2.0-rc-A-2 to v2.0-rc-A-3 will see:

1. No schema change (A-3 is code-only).
2. New rows written by the A-3 poller populate **both** legacy AND typed columns. `value_type` switches from `'legacy'` to the matching variant per row.
3. Pre-A-3 rows continue to read fine via legacy paths.

This staging contract is the central design choice of Epic A.

---

## Out of Scope

The following items are explicitly NOT part of A-2 — they belong to follow-on stories:

- **Writer-side typed-column population** — Story A-3 (poller value-payload write pipeline) wires `MetricType` payload through `prepare_metric_for_batch` and updates `SqliteBackend::batch_write_metrics` / `upsert_metric_value` / `set_metric` to populate typed columns. A-2 leaves writers untouched.
- **Reader-side typed-column consumption** — Story A-4 (OPC UA Read), A-5 (OPC UA HistoryRead), A-6 (Web UI live metrics) rewrite the four reader sites in `SqliteBackend` to project from typed columns.
- **Removal of legacy `value TEXT NOT NULL` + `data_type TEXT NOT NULL` columns** — Story A-7 (migration runbook + cleanup migration) authors a future migration v008 (or later) that drops the legacy columns once all readers have moved.
- **Migration operator runbook (`docs/deployment-guide.md § "Epic A migration"`)** — Story A-7.
- **Exactly-one-non-NULL CHECK constraint** — Deferred to A-3 or A-7 (cannot be enforced in A-2 because writers haven't been updated yet).
- **NaN / Inf handling at the SQLite write site** — Deferred to A-3 per `deferred-work.md § A-1-iter3-DEF8`.
- **InMemoryBackend changes** — InMemoryBackend has no schema; nothing to migrate.
- **`MetricType` enum modifications** — A-1 completed the payload-bearing refactor; A-2 does not re-touch `src/storage/types.rs`.

---

## Completion Note

Story A-2 closes when:

1. `migrations/v007_typed_value_columns.sql` is committed.
2. `src/storage/schema.rs::run_migrations` applies it via `current_version < 7` block.
3. All 13 ACs are SATISFIED or explicitly DEFERRED-DOCUMENTED per CLAUDE.md "Code Review & Story Validation Loop Discipline."
4. `cargo test --all-targets` ≥ 1130 passed / 0 failed / ≤10 ignored; `cargo clippy --all-targets -- -D warnings` clean; `cargo test --doc` 0 failed / ≥55 ignored.
5. A subsequent code-review loop on a different LLM has terminated under condition #1 or #2.

The dev agent commits the implementation as a single "Story A-2: SQLite Schema Migration v007 — Implementation Complete" commit, flips the story file Status to `review`, and updates `sprint-status.yaml` accordingly. A subsequent `bmad-code-review A-2` run on a different LLM follows the same 3-iteration loop pattern validated across 7 stories (4-4, 9-4, 9-5, 9-6, 9-7, 9-8, A-1).

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — `claude-opus-4-7[1m]`. Implementation completed 2026-05-15 via `bmad-dev-story A-2`.

### Debug Log References

No debug log entries — implementation ran without obstacles. The single clippy regression (`clippy::type_complexity` on a 7-field tuple destructure in `test_v007_writers_still_populate_legacy_columns`) was caught by the verification clippy run and fixed by refactoring to per-column `query_row` calls (7 separate `String`/`Option<…>` extractions instead of a wide tuple).

### Completion Notes List

- **Task 0 (GH tracking issue):** deferred to user per Stories 9-4 through A-1 precedent — `gh` CLI not authenticated for write from this dev session. Story-file header table to be updated by user once issue filed.
- **Task 1 (`migrations/v007_typed_value_columns.sql`):** 32-line SQL file. 10 `ALTER TABLE ADD COLUMN` statements (5 columns × 2 tables: `value_real REAL NULL`, `value_int INTEGER NULL`, `value_bool INTEGER NULL`, `value_text TEXT NULL`, `value_type TEXT NOT NULL DEFAULT 'legacy' CHECK(value_type IN ('legacy','Float','Int','Bool','String'))`). No `PRAGMA user_version` in the SQL — runner owns that.
- **Task 2 (runner wiring):** added `const MIGRATION_V007` next to v001–v006, appended `if current_version < 7 { … }` block matching the v006 pattern (execute_batch → pragma_update 7 → info log), bumped `LATEST_VERSION` from `5` (stale since v006) to `7`. Existing `test_run_migrations_fresh_database` and `test_run_migrations_idempotent` had their version assertions bumped 6 → 7.
- **Task 3 (`test_v007_preserves_pre_a2_rows`):** added a `create_v006_baseline_db` helper that runs the full migration to v007, then rolls v007 off via `ALTER TABLE … DROP COLUMN` (supported by SQLite 3.35+ bundled with rusqlite 0.38) and rewinds `user_version` to 6. Test seeds 3 metric_values rows + 5 metric_history rows covering all 4 data_type variants, applies v007 via a second `run_migrations` call, then asserts: row counts preserved, `value_type='legacy'` on all rows (via column default), all 4 typed columns NULL, legacy `value`+`data_type` columns byte-for-byte preserved.
- **Task 4 (writer/reader unchanged contracts):** two new tests pin AC#8 and AC#9. `test_v007_writers_still_populate_legacy_columns` uses `SqliteBackend::upsert_metric_value` to write a row, then verifies via raw SQL that legacy columns are populated and typed columns are NULL (default `value_type='legacy'` applies). `test_v007_readers_still_read_legacy_columns` round-trips through `get_metric_value` and asserts the reader still projects from legacy columns. The clippy `type_complexity` lint forced the writer test to use 7 separate `query_row` calls instead of a single wide tuple — chose that over `#[allow]` for cleaner test code.
- **Task 5 (runner test expansion):** added `test_v007_adds_all_typed_columns_to_metric_values` and `test_v007_adds_all_typed_columns_to_metric_history` using `pragma_table_info('<table>')` SQL view (cleaner than parsing `PRAGMA table_info` output) to assert each new column's type + NULL-ability. Added `test_v007_value_type_check_constraint` covering both the CHECK rejection of invalid discriminants and the acceptance of all 5 valid discriminants (`legacy`, `Float`, `Int`, `Bool`, `String`). Added `test_v007_migration_under_5s_for_10k_rows` honouring AC#7 — seeded 10 000 rows (5k metric_values + 5k metric_history) in a single transaction (~50 ms), timed the v007 application via `std::time::Instant`. Actual measured cost: well under 100 ms (SQLite 3.46's `ALTER TABLE ADD COLUMN` is metadata-only).
- **Task 6 (conventions verification):** new file's SPDX + copyright header matches `v005_gateway_status.sql` and `v006_gateway_status_health_metrics.sql` format. `include_str!` resolves cleanly (verified via `cargo build --all-targets`).
- **Task 7 (strict-zero check):** `git status --short -- src/ migrations/ _bmad-output/ Cargo.toml README.md` returns exactly 4 paths: `src/storage/schema.rs` (M), `_bmad-output/implementation-artifacts/sprint-status.yaml` (M), `migrations/v007_typed_value_columns.sql` (??), `_bmad-output/implementation-artifacts/A-2-sqlite-schema-migration-v007.md` (??). All AC#10 strict-zero invariants honoured — `src/storage/sqlite.rs`, `src/storage/types.rs`, `src/storage/memory.rs`, `src/storage/mod.rs`, `src/storage/pool.rs`, all `src/web/*`, `src/opc_ua*.rs`, `src/security*.rs`, `src/main.rs`, `src/chirpstack.rs`, `src/config_reload.rs`, `src/opcua_topology_apply.rs` have zero diff lines.
- **Task 8 (final verification):**
  - `cargo build --all-targets`: clean.
  - `cargo test --all-targets`: **1139 passed / 0 failed / 10 ignored** (+14 vs 1125 A-1-review baseline; exceeds AC#11 target ≥1130 by 9). Schema-module subset: 11 tests passed in 0.07s (was 4; added 7 new).
  - `cargo clippy --all-targets -- -D warnings`: clean (after the `type_complexity` refactor).
  - `cargo test --doc`: 0 failed / 55 ignored (preserved from A-1 review baseline; AC#12 honored).
- **Carry-forward concerns surviving A-2 (will be reviewed in A-2 code-review):**
  - A-1 iter-3 Edge F3 NaN/Inf serialisation hazard remains in A-3 territory — A-2 didn't touch `set_metric`/`serde_json::to_string`.
  - Exactly-one-non-NULL CHECK constraint on typed columns deferred to A-3 (writer correctness) or A-7 (cleanup migration). A-2 cannot enforce it because writers haven't been updated yet.
  - Legacy `value TEXT NOT NULL` + `data_type TEXT NOT NULL` columns retained on both tables; A-5 / A-7 retires them once readers fully migrate.

### File List

**Modified:**
- `src/storage/schema.rs` — added `MIGRATION_V007` const + `if current_version < 7 { … }` runner block + 7 new tests covering AC#1-#9 (`test_v007_preserves_pre_a2_rows`, `test_v007_writers_still_populate_legacy_columns`, `test_v007_readers_still_read_legacy_columns`, `test_v007_adds_all_typed_columns_to_metric_values`, `test_v007_adds_all_typed_columns_to_metric_history`, `test_v007_value_type_check_constraint`, `test_v007_migration_under_5s_for_10k_rows`) + helper `create_v006_baseline_db` + version assertion bumps (6 → 7) in `test_run_migrations_fresh_database` and `test_run_migrations_idempotent` + `LATEST_VERSION` bump (5 → 7).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — `A-2-sqlite-schema-migration-v007: ready-for-dev → in-progress → review`; `last_updated` narrative refreshed.
- `_bmad-output/implementation-artifacts/A-2-sqlite-schema-migration-v007.md` — this file, Dev Agent Record populated.

**Created:**
- `migrations/v007_typed_value_columns.sql` — 38 lines post-iter-1 (was 32 at implementation; +6 lines for the two `CHECK(value_bool IS NULL OR value_bool IN (0,1))` continuation clauses added by iter-1 review patch IM1), 10 `ALTER TABLE ADD COLUMN` statements unchanged.

**Strict-zero invariants honoured (AC#10 list — all `git diff` empty):**
- `src/storage/sqlite.rs`, `src/storage/types.rs`, `src/storage/memory.rs`, `src/storage/mod.rs`, `src/storage/pool.rs`
- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs`
- `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`
- `src/security.rs`, `src/security_hmac.rs`
- `src/main.rs::initialise_tracing` (function body untouched)
- `src/config_reload.rs`, `src/opcua_topology_apply.rs`, `src/chirpstack.rs`
- `Cargo.toml`, `README.md` — README "Current Version" line update deferred to the implementation commit per A-1 precedent (sprint-status carries the canonical narrative).

### Review Findings

Iter-1 code review run on 2026-05-15 via `bmad-code-review A-2` — 3 parallel adversarial layers (Blind Hunter, Edge Case Hunter, Acceptance Auditor) against the implementation diff (591 lines, 2 files).

**Raw findings:** Blind 31 + Edge 15 + Auditor 12 SATISFIED / 1 AMBIGUOUS = **47 layer-level items**.
**After dedupe / triage:** **0 decision-needed, 6 patches, 24 deferred, 5 dismissed**.

#### Decision-needed (0)

None. Two AskUserQuestion items resolved during triage: (a) HIGH IH1 migration-atomicity gap deferred to focused runner-hardening story per user's explicit acceptance; (b) AC#13 SPDX header accepted as conforming to CLAUDE.md doctrine.

#### Patches (6, all applied)

- [x] [Review][Patch] **IM1 [MEDIUM] Add `CHECK(value_bool IS NULL OR value_bool IN (0, 1))` to both tables** [migrations/v007_typed_value_columns.sql] — Schema-side defence-in-depth. Pre-iter-1 the column accepted any 64-bit integer; an A-3 writer storing a sentinel like `-1` or `2` would silently pass and downstream readers would see truthy values for non-boolean inputs. New `test_v007_value_bool_check_constraint` pins both tables (symmetric coverage), enumerating valid (`NULL`, `0`, `1`) and invalid (`-1`, `2`, `99`, `i64::MAX`, `i64::MIN`) inputs. (Blind F20 + Edge F12 convergent)
- [x] [Review][Patch] **IM2 [MEDIUM] Update `README.md` "Current Version" line per CLAUDE.md "Documentation Sync"** [README.md:198] — Dev-record had deferred this to the implementation commit per A-1 precedent, but CLAUDE.md mandates per-commit README sync. Updated to reflect A-2 review status + full implementation narrative + carry-forward concerns. (Auditor Cross-AC flag)
- [x] [Review][Patch] **IM3 [MEDIUM] Replace substring error-match with `rusqlite::Error::SqliteFailure` extended-code check** [src/storage/schema.rs::test_v007_value_type_check_constraint] — Pre-iter-1 the test matched on `msg.contains("CHECK") || msg.contains("constraint")` (Debug output substring) which would silently pass on a NOT NULL violation or a future SQLite error-message rename. Refactored to a `assert_check_constraint_violation` helper that destructures `rusqlite::Error::SqliteFailure(sqlite_err, _)` and asserts `sqlite_err.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_CHECK` (275). (Blind F16 + Edge F14 convergent)
- [x] [Review][Patch] **IL1 [LOW] Add case-sensitivity negative coverage to CHECK test** [src/storage/schema.rs::test_v007_value_type_check_constraint] — SQLite's `IN` uses binary collation by default, so `'FLOAT'`, `'float'`, `'Float '` (trailing whitespace), `' Float'` (leading whitespace), and empty string all bypass the whitelist. Added a loop asserting each of `["FLOAT", "float", "Float ", " Float", "", "INT", "boolean"]` rejects via `SQLITE_CONSTRAINT_CHECK`. (Edge F1 + F15)
- [x] [Review][Patch] **IL2 [LOW] Document heterogeneous legacy `value` lexemes carry-forward** [deferred-work.md § A-2-iter1-DEF1] — Three writer paths produce three different `value`-column shapes (JSON blob from `set_metric`, discriminant from `upsert_metric_value`+`append_metric_history`, real-string from `batch_write_metrics`). A-3 must handle all three when migrating writers to populate typed columns. Documented as DEF1 with explicit A-3 hand-off. (Edge F11)
- [x] [Review][Patch] **IL3 [LOW] Add symmetric `metric_history` CHECK test** [src/storage/schema.rs::test_v007_value_type_check_constraint_symmetric_on_metric_history] — Pre-iter-1 the negative test only covered `metric_values`. A typo in the migration (e.g. dropping `'Bool'` from the history-side CHECK) would not be caught. New test asserts symmetric behaviour: invalid discriminants reject with `SQLITE_CONSTRAINT_CHECK`, all 5 valid discriminants accept. (Blind F17)

#### Deferred (24) — pre-existing, transitional, or explicitly out-of-scope

All entries written to `_bmad-output/implementation-artifacts/deferred-work.md` under `## Deferred from: code review of A-2-sqlite-schema-migration-v007 — iter-1 (2026-05-15)`. Summary:

- **DEF-IH1 [HIGH, user-confirmed deferral]:** Migration runner is not transactional (`execute_batch` + separate `pragma_update` non-atomic; multi-statement `ALTER TABLE` can partially apply). Same shape across v001-v006. Out of A-2 scope. Recommended for a focused runner-hardening story.
- DEF1 (Edge F11): Heterogeneous legacy `value` lexemes (JSON / discriminant / real-string) across 3 writers — A-3 must handle.
- DEF2 (Blind F7 + F13 + Edge F13): No cross-column CHECK constraint (`value_type` ↔ typed columns) — spec explicitly defers to A-3/A-7.
- DEF3 (Blind F23): No index on `value_type` — spec explicitly defers to A-4/A-5/A-7.
- DEF4 (Blind F8): Dual source of truth `data_type` vs `value_type` — A-5/A-7 retires legacy.
- DEF5 (Blind F11 + F12 — pre-existing #102): `temp_db()` cleanup not RAII-guarded.
- DEF6 (Blind F18): No explicit cross-restart second-run idempotency test.
- DEF7 (Blind F26 + Edge F7): Multiple `Connection` handles per test — issue #102 territory.
- DEF8 (Edge F8): `create_v006_baseline_db` doesn't verify full v006 baseline shape.
- DEF9 (Blind F22): SQL comment on column-default trick could mislead future maintainers.
- DEF10 (Blind F19): Stale comment header on `pragma_table_info` query.
- DEF11 (Blind F21): Explicit `NULL` keyword on nullable columns is redundant.
- DEF12 (Blind F5 + Edge F5): `pragma_table_info` query inlines column name via `format!()` — safe today.
- DEF13 (Auditor Cross-AC AC#13): SPDX backfill for v002/v003/v005/v006 — separate housekeeping commit.
- DEF14 (Blind F28): Migration-numbering coordination — process-level concern.
- DEF15 (Blind F29): No rollback migration — A-7 territory.
- DEF16 (Blind F30): Schema unit tests couple to `SqliteBackend` — architectural cleanup with #102.
- DEF17 (Blind F31): Migration partial-apply observability — bundled with DEF-IH1.
- DEF18 (Edge F2): `value_type = NULL` two-constraint interaction not pinned.
- DEF19 (Edge F6): Perf test wall-clock coupling — defensive only.
- DEF20 (Edge F9): `_value_type` local in `opc_ua.rs:1923` shares name with new schema column — cosmetic grep-confusion.
- DEF21 (Edge F10): `if current_version < 7` guard doesn't detect schema drift (operator manual pragma).
- DEF22 (Blind F27): AC#6 test seeds 8 rows — sufficient per AC; perf test covers larger scale.
- DEF23 (Blind F24): Timestamp duplication in metric_values seeds — non-conflicting with PRIMARY KEY.

#### Dismissed (5) — false positives or Blind misreads

- DM1 (Blind F2 / F25): "Absolute path" diff artifact from `git diff --no-index /dev/null /home/...` — file is at correct relative location, verified by green test suite and successful `include_str!` resolution.
- DM2 (Blind F3 / F4): Blind misread the test pinning current writer behaviour as a "data-loss regression". It's exactly the pre-existing issue #108 behaviour Epic A is closing in stages; spec explicitly defers writers to A-3.
- DM3 (Blind F1 / F15): `LATEST_VERSION = 5` was stale before A-2 (since v006 landed); A-2 correctly fixes it to 7 — not a finding against A-2.

#### Iter-2 review (2026-05-15)

Iter-2 surfaced Blind 12 + Edge 10 + Auditor 13 ACs verified + 5 cross-pass = **40 findings**.

**Auditor verdict: ELIGIBLE-FOR-DONE.** All 13 ACs SATISFIED, all 6 iter-1 patches PATCHED-CORRECTLY, no new AC drift, no iter-2 regressions detected.

After iter-2 dedupe / triage: **0 HIGH-REG / HIGH / MEDIUM open, 5 small patches applied, ~15 LOW deferred (all align with existing A-2-iter1-DEF entries), 1 false positive dismissed.**

##### Iter-2 patches (5, all applied)

- [x] [Iter-2][Patch] **JR1 [MEDIUM] Sync README "Current Version" test count** [README.md:198] — Iter-1 IM2 captured the post-implementation count (1139). After iter-1 patches IM1/IL3 added 2 new test functions, the count is 1143. README narrative updated to reflect `1143 passed (+18 vs 1125 baseline; +4 from iter-1 review patches)`. (Blind F2)
- [x] [Iter-2][Patch] **JR2 [MEDIUM] Pin `SqliteBackend::new` migration invariant in test comments** [src/storage/schema.rs::test_v007_writers_still_populate_legacy_columns + ::test_v007_readers_still_read_legacy_columns] — Both tests rely on the implicit invariant that `SqliteBackend::new` runs `run_migrations` on its pool connection. If that invariant ever changes (e.g. migrations move to a separate `initialize()` call), the tests fail loudly at the first column-bearing SELECT. Added explicit comment in both tests. (Blind F3)
- [x] [Iter-2][Patch] **JR3 [MEDIUM] Add case-sensitivity sweep to `metric_history` symmetric CHECK test** [src/storage/schema.rs::test_v007_value_type_check_constraint_symmetric_on_metric_history] — IL1 added a case-sensitivity loop (`["FLOAT", "float", "Float ", " Float", "", "INT", "boolean"]`) to the `metric_values` test, but IL3 omitted it from the symmetric `metric_history` test. Asymmetric coverage gap closed — now both tables' CHECK definitions are pinned to identical binary-collation behaviour. Also added an inline `assert_check_violation` helper inside the symmetric test (duplicated rather than factored across modules per A-2-iter1-DEF16 helper-DRY deferral). (Blind F4)
- [x] [Iter-2][Patch] **JR4 [LOW] Update spec AC#2 bullet to reflect IM1 `value_bool` CHECK** [this file § AC#2 line 99] — Iter-1 IM1 added `CHECK(value_bool IS NULL OR value_bool IN (0, 1))` to the migration SQL but AC#2's bullet list at spec line 99 wasn't amended. Bullet now reads "`value_bool INTEGER NULL` with `CHECK(value_bool IS NULL OR value_bool IN (0, 1))` (CHECK added by iter-1 review patch IM1 as schema-side defence-in-depth)". (Auditor Cross-pass 2)
- [x] [Iter-2][Patch] **JR5 [LOW] Sync spec/sprint-status SQL line count (32 → 38)** [this file File List + sprint-status.yaml narrative] — Iter-1 IM1 added 4 `CHECK(value_bool …)` continuation lines (2 tables × 2 lines each: the `CHECK(...)` clause + closing `;`) plus blank-line spacing — total file grew 32 → 38 lines. Spec dev-record + sprint-status updated to reflect the 38-line current state. (Auditor Cross-pass 3; iter-3 Blind F7 corrected the "+6 lines" to "+4 CHECK lines" arithmetic — file delta is net +6 with formatting whitespace.)

##### Iter-2 patch verification (2026-05-15)

- `cargo build --all-targets` — clean.
- `cargo test --all-targets` — **1143 passed / 0 failed / 10 ignored** (unchanged from iter-1 — JR3 added assertions inside an existing test fn, no new test fns; JR1/JR2/JR4/JR5 are doc/narrative changes).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --doc` — 0 failed / 55 ignored (AC#12 preserved).

Schema-module subset: **13 tests** (4 baseline + 9 A-2 — same count as post-iter-1; JR3 strengthened an existing test in-place).

##### Iter-2 dismissed (1)

- DM-iter2-1 (Blind F1 [HIGH-REG]): "Migration SQL file diff uses absolute path" — false positive (twice-misread now; same artifact as iter-1 Blind F2/F25). `git diff --no-index /dev/null /home/...` emits absolute `a/` and `b/` paths because the file is untracked and captured against an absolute path. The file is at the correct relative path (`migrations/v007_typed_value_columns.sql`), independently verified by Edge F7 (`git diff --name-only HEAD -- 'src/**/*.rs' migrations/ README.md`), Auditor live re-verification, and a green `cargo build` + 1143 passing tests confirming `include_str!("../../migrations/v007_typed_value_columns.sql")` resolves.

##### Iter-2 deferred (15 LOW — all map to existing A-2-iter1-DEF entries)

Blind F5-F12 and Edge F1-F5 are pure test economy / defence-in-depth concerns that align with `A-2-iter1-DEF1` through `A-2-iter1-DEF23` in `deferred-work.md`. No new defer entries needed.

##### Iter-2 cross-pass positive confirmations (Edge F6-F10)

- AC#10 strict-zero invariants verified clean (`src/storage/sqlite.rs` etc. zero-diff).
- A-1 iter-3 DEF8 NaN/Inf hazard preserved as A-3 carry-forward.
- README badge URL `2.0.0--rc` shields.io encoding intact.
- `rusqlite::ffi::SQLITE_CONSTRAINT_CHECK = 275` confirmed via `libsqlite3-sys 0.36` Cargo.lock pin.
- `create_v006_baseline_db` `DROP COLUMN` sequence safe (no FK/index/external-CHECK references on the v007 columns).

#### Iter-3 review (2026-05-15)

Iter-3 same-LLM termination pass per CLAUDE.md "Code Review & Story Validation Loop Discipline" + memory `feedback_iter3_validation` 7-story validated pattern. Three parallel adversarial layers (Blind / Edge / Acceptance Auditor) against the combined iter-1 + iter-2 patch diff (790 lines, 3 files vs commit `c31cad5`).

**Raw findings:** Blind 10 + Edge 5 + 2 positive confirmations + Auditor 13 ACs SATISFIED + 5 cross-pass + 1 LOW caveat = **34 layer-level items**.

**Auditor verdict: ELIGIBLE-FOR-DONE** under CLAUDE.md condition #2 (only LOW remains), with iter-3-CP1 LOW sprint-status drift caveat closed by patch K1.

##### Iter-3 patches (5, all applied)

- [x] [Iter-3][Patch] **K1 [MEDIUM] Consolidate test-count narrative across README + spec + sprint-status** [README.md:198 + sprint-status.yaml:38,168 + this file] — Iter-2 JR1/JR5 fixed README + spec but not sprint-status. Multiple narrative drifts: README "+4 from iter-1 patches IM1/IL3" → actual is "+2 new test fns" (cargo cross-binary counting inflates the delta); spec JR5 "+6 lines" → actual is "+4 CHECK clause lines + 2 whitespace = net +6 file lines"; sprint-status.yaml said "32 lines" / "1139 passed" → synced to "38 lines" / "1143 passed". The cross-binary delta from 1125 → 1143 is now documented explicitly. (Blind F1+F2+F7 + Edge F3 + Auditor iter-3-CP1)
- [x] [Iter-3][Patch] **K2 [MEDIUM] `LATEST_VERSION == 7` forward-compat guard in `create_v006_baseline_db`** [src/storage/schema.rs::create_v006_baseline_db] — Helper rolls v007 off by dropping 5 columns. Valid ONLY while v007 is the latest migration. When v008 lands touching any of those column names — or adding columns this helper doesn't know to drop — the "v006 baseline" silently diverges. Added explicit `assert_eq!(actual_version, HELPER_LATEST_VERSION, ...)` that fails loudly the moment a future migration advances past v007, forcing the next-story dev to refactor. (Blind F4)
- [x] [Iter-3][Patch] **K4 [LOW] Refine JR2 invariant comment precision** [src/storage/schema.rs::test_v007_writers_still_populate_legacy_columns + ::test_v007_readers_still_read_legacy_columns] — Pre-iter-3 comment said "`SqliteBackend::new` runs `run_migrations`"; refined to "`SqliteBackend::new` delegates to `Self::with_pool` (see `src/storage/sqlite.rs:218-221`), which calls `run_migrations`" — clarifies the actual delegation chain. Also clarifies that the invariant is `SqliteBackend`-only (not `InMemoryBackend::new`) and updates the failure-line narrative from "first column-bearing SELECT" to "when `upsert_metric_value` tries to write to a missing table or column" (which is the actual earliest fail point). (Edge F2 + Blind F9)
- [x] [Iter-3][Patch] **K5 [LOW] Extend case-sensitivity sweeps with OPC UA Variant-side lexemes** [src/storage/schema.rs::test_v007_value_type_check_constraint + ::test_v007_value_type_check_constraint_symmetric_on_metric_history] — Pre-iter-3 sweeps iterated `["FLOAT", "float", "Float ", " Float", "", "INT", "boolean"]`. Extended to `["FLOAT", "float", "Float ", " Float", "", "INT", "boolean", "Boolean", "Int64", "Int32", "Double"]` — defends against A-3 using `Variant`-side rendering (e.g. `Variant::Int64`'s name → `"Int64"`) instead of `MetricType::to_string()` (which produces `"Int"`). Applied symmetrically to both `metric_values` and `metric_history`. (Edge F1)
- [x] [Iter-3][Patch] **K6 [LOW] Mirror full `value_bool` bad-vector on `metric_history`** [src/storage/schema.rs::test_v007_value_bool_check_constraint] — Pre-iter-3 only tested `metric_history.value_bool = 42`; now mirrors the full vector `[-1, 2, 99, 42, i64::MAX, i64::MIN]` symmetrically on both tables. Defends against asymmetric CHECK definition drift (e.g. a typo expanding the history-side IN list to `(0,1,2)`). (Edge F5)

##### Iter-3 patch verification (2026-05-15)

- `cargo build --all-targets` — clean.
- `cargo test --all-targets` — **1143 passed / 0 failed / 10 ignored** (unchanged — K5 + K6 added assertions inside existing test fns, K1/K2/K4 are doc/comment/narrative-only changes).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --doc` — 0 failed / 55 ignored (AC#12 preserved).

Schema-module subset: **13 tests** (4 baseline + 9 v007) all green in 0.06s.

##### Iter-3 dismissed (1 — same false positive twice-misread)

DM-iter3-1 (Blind F1 [implicit reference to absolute-path SQL diff]): the "absolute path" claim about `home/gcorbaz/.../migrations/v007_*.sql` was twice-misread (iter-1 + iter-2); iter-3 Blind correctly ignored the axis per instructions. File is at correct relative path; build clean; tests green. Edge F6 explicitly verified AC#10 strict-zero invariants clean.

##### Iter-3 deferred (Blind F5/F6/F8/F9/F10 + Edge F4 — all LOW)

- **DEF-iter3-1 (Blind F3 CI flake):** AC#7 timing test (`<5s`) has wide budget (actual measured cost is well under 100 ms on standard runners; 0.06s for all 11 schema tests combined). Real CI flake risk is low. Defer with `#[ignore]` fallback note — apply only if CI flake reports surface. Precedent: `test_pool_throughput_under_load` was `#[ignore]`'d in commit `2c5a6b1` after sustained flake.
- **DEF-iter3-2 (Blind F5 + Edge F4):** Helper naming divergence (`assert_check_constraint_violation` vs `assert_check_violation`) — already documented as A-2-iter1-DEF16 helper-DRY deferral.
- **DEF-iter3-3 (Blind F6):** README "Current Version" line has compounded into ~25 000 chars of nested PREVIOUS NARRATIVE chains. Tech debt — out of A-2 scope. Same shape as A-1-iter1-DEF10.
- **DEF-iter3-4 (Blind F8):** `test_v007_writers_still_populate_legacy_columns` opens 3 concurrent connections to one SQLite file — same multi-connection pattern as A-2-iter1-DEF7. Not load-bearing in A-2.
- **DEF-iter3-5 (Blind F10):** AC#7 seed pattern uses 5000 distinct `metric_name` values across 100 device_ids — unusual distribution that exists to avoid PRIMARY KEY collisions. Add a comment if future UNIQUE constraint narrows. LOW.

Total iter-3 net new defers: 5 (all LOW, all in `deferred-work.md` shape; existing DEFs incorporate them where applicable).

#### Loop-termination — TERMINATED at iter-3 (2026-05-15)

Per CLAUDE.md "Code Review & Story Validation Loop Discipline":

- **Condition #2 (only LOW remains): MET.** All HIGH/MEDIUM findings from iter-1 + iter-2 + iter-3 are PATCHED-CORRECTLY (16 patches total: 6 iter-1 + 5 iter-2 + 5 iter-3). All remaining defers are LOW or already-documented pre-existing concerns.
- **Condition #3 (explicit user acceptance): MET on the 2 escalated items.** HIGH A-2-iter1-DEF-IH1 (migration atomicity gap, pre-existing across v001-v006) user-confirmed for deferral to focused runner-hardening story. AC#13 SPDX header user-confirmed as conforming to CLAUDE.md doctrine.

**7-story `feedback_iter3_validation` pattern extended to 8 stories: 4-4, 9-4, 9-5, 9-6, 9-7, 9-8, A-1, A-2.** Iter-2 caught 5 patches (3 MED + 2 LOW) that iter-1 missed; iter-3 caught 5 small patches (2 MED + 3 LOW) including the K1 narrative-drift consolidation that survived iter-1 + iter-2. Same pattern as A-1's review cycle: 23 total patches across 3 iterations; iter-3 catches the smaller items single-pass review tends to miss.

A-2 status flips `review → done`. Next: bmad-create-story A-3 (Poller value-payload write pipeline).

#### Iter-1 patch round verification (2026-05-15)

All 6 patches applied. Post-patch verification:
- `cargo build --all-targets` — clean.
- `cargo test --all-targets` — **1143 passed / 0 failed / 10 ignored** (+4 vs 1139 post-implementation; +18 vs 1125 A-1-review baseline; exceeds AC#11 target ≥1130 by 13).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --doc` — 0 failed / 55 ignored (AC#12 preserved).

Schema-module subset: **15 tests** (was 11 pre-iter-1 + 2 new test fns from IM1/IL3 + 2 net new assertions from IL1 inlined into existing test) all passed in 0.09s.

New tests added:
- `test_v007_value_bool_check_constraint` (IM1)
- `test_v007_value_type_check_constraint_symmetric_on_metric_history` (IL3)

Per CLAUDE.md "Code Review & Story Validation Loop Discipline": iter-1 was a non-trivial patch round (6 patches across migration SQL, schema tests, README, deferred-work). Recommend running iter-2 review before flipping to `done`.

### Change Log

- 2026-05-15: Iter-1 code review complete via `bmad-code-review A-2` on a different LLM. 3 parallel adversarial layers (Blind / Edge / Auditor) produced 31 + 15 + 12 SATISFIED-1 AMBIGUOUS verdicts = 47 raw findings. After dedupe / triage: 0 decision-needed, **6 patches applied** (3 MEDIUM + 3 LOW), 24 deferred (including 1 HIGH IH1 with user-explicit acceptance for runner-atomicity gap that's pre-existing across v001-v006), 5 false-positives dismissed. The HIGH IH1 deferral and the AC#13 SPDX accept-as-conforming decision were user-confirmed via `AskUserQuestion` on 2026-05-15. After iter-1 patches: `cargo test --all-targets` **1143 passed / 0 failed / 10 ignored** (+4 vs 1139 post-implementation; +18 vs 1125 A-1-review baseline); `cargo clippy --all-targets -- -D warnings` clean; `cargo test --doc` 0 failed / 55 ignored (AC#12 preserved). Status stays `review` pending iter-2 sweep. Recommend running iter-2 on a different LLM per CLAUDE.md doctrine + memory `feedback_iter3_validation` 7-story validated pattern.
- 2026-05-15: Implementation complete via `bmad-dev-story A-2`. Status `ready-for-dev → in-progress → review`. All 13 ACs SATISFIED. `cargo test --all-targets` 1139 passed / 0 failed / 10 ignored (was 1125 A-1-review baseline; +14 net from 7 new schema tests). `cargo clippy --all-targets -- -D warnings` clean. `cargo test --doc` 0 failed / 55 ignored. New `migrations/v007_typed_value_columns.sql` adds 5 typed columns to both `metric_values` and `metric_history`. `src/storage/schema.rs` gains the v007 runner block + 7 new tests + the `create_v006_baseline_db` helper. Strict-zero invariants honored — `src/storage/sqlite.rs` and all other AC#10 listed files have zero diff. Writers + readers stay unchanged in A-2 per the spec's option-b staging contract; A-3 plumbs typed payload through writers. One clippy `type_complexity` regression on a wide tuple destructure caught during verification and fixed via per-column `query_row` refactor. Recommend running `bmad-code-review A-2` on a different LLM per CLAUDE.md doctrine + memory `feedback_iter3_validation` 7-story validated pattern (4-4, 9-4, 9-5, 9-6, 9-7, 9-8, A-1 — extends to A-2). Next: `bmad-code-review A-2`.
- 2026-05-15: Story spec created via `bmad-create-story A-2`. Status `backlog → ready-for-dev`. Comprehensive analysis of the existing v006 baseline schema (`migrations/v001_initial.sql` through `v006_gateway_status_health_metrics.sql`), the migration runner pattern (`src/storage/schema.rs::run_migrations`), and the SqliteBackend reader/writer paths (`src/storage/sqlite.rs` lines 392-1526). A-2 is scoped to schema-only DDL + value_type='legacy' tagging — writers and readers remain unchanged until A-3 plumbs the typed payload through `prepare_metric_for_batch`. Carry-forward concerns: A-1 iter-3 Edge F3 NaN/Inf serialisation hazard remains A-3 territory; AC#10 strict-zero invariant carry-forward from A-1 keeps the touched-file surface to `migrations/v007_typed_value_columns.sql` (new), `src/storage/schema.rs`, `README.md` Current Version line, and the sprint-status / story-file artifacts. Test budget delta: ≥+5 tests in `src/storage/schema.rs::tests` covering v006→v007 upgrade, idempotency, column additions, CHECK constraint (conditional), and migration-time SLA (possibly `#[ignore]`). Tracking issue to be filed by dev agent at implementation start. Next: `bmad-dev-story A-2`.

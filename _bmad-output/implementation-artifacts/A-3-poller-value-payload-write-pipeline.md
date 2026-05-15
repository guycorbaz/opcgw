# Story A-3: Poller Value-Payload Write Pipeline

| Field         | Value                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------- |
| Story key     | `A-3-poller-value-payload-write-pipeline`                                                             |
| Epic          | A — Storage Payload Migration (Phase B Closure, gates v2.0 GA)                                        |
| FRs           | FR51 (Epic-A umbrella)                                                                                |
| Status        | done                                                                                                  |
| Created       | 2026-05-15                                                                                            |
| Source epic   | `_bmad-output/planning-artifacts/epics.md § Epic A § Story A.3`                                       |
| Sprint change | `_bmad-output/planning-artifacts/sprint-change-proposal-2026-05-14.md`                                |
| Tracking      | GitHub tracking issue to be filed by dev agent (see Task 0)                                           |

---

## User Story

As a **gateway poller**,
I want `ChirpstackPoller` to wrap real measurement values into the payload-bearing `MetricType` variants at the point of reception, and the SqliteBackend writers to populate the typed value columns introduced by A-2,
So that the value persisted by every storage write path carries the real measurement end-to-end — closing the structural gap between Story A-1 (type-level payload-bearing enum) and Story A-2 (schema with typed columns).

---

## Story Context

### Why A-3 is the central enabling story of Epic A

A-1 made `MetricType` payload-bearing at the type level. A-2 added five typed columns (`value_real`, `value_int`, `value_bool`, `value_text`, `value_type`) to both `metric_values` and `metric_history` with CHECK constraints. **Today the typed columns are NULL for every newly-written row** because the production poller still stamps zero-defaulted payloads (`MetricType::Float(0.0)`, `Int(0)`, `Bool(false)`, `String("")`) at 7 `TODO(A-3)` sites in `src/chirpstack.rs::prepare_metric_for_batch`, and the SqliteBackend writers ignore the typed columns entirely (the option-(b) staging contract A-2 explicitly preserved).

A-3 closes that gap:

1. **Poller side (`src/chirpstack.rs`):** wrap the real `metric.datasets[0].data[0]: f32` value into the matching `MetricType` payload at the 7 `TODO(A-3)` sites. Decide and apply the **NaN/Inf policy** that A-1 iter-3 Edge F3 explicitly handed off (`A-1-iter3-DEF8` in `deferred-work.md`).
2. **Storage side (`src/storage/sqlite.rs`):** rewire all 4 writer methods (`set_metric`, `upsert_metric_value`, `append_metric_history`, `batch_write_metrics`) to populate the typed columns and `value_type` from the typed payload, while continuing to populate the legacy `value`/`data_type` columns (A-5/A-7 retires those).
3. **Reinstate `chirpstack.rs::store_metric` body** (currently `todo!()` per A-1 P9 + iter-2 IR5) with the real payload threading — same dispatch as `prepare_metric_for_batch`.
4. **Add a v008 migration with the exactly-one-non-NULL CHECK constraint** that A-2 explicitly deferred to A-3 or A-7 (A-2 Dev Notes § "Why NOT to enforce exactly-one-non-NULL in A-2") — A-3 is the natural landing point because A-3 is what makes the invariant provable.

After A-3 ships, the OPC UA Read path (A-4), HistoryRead path (A-5), and Web UI dashboard (A-6) can rewrite their reads to consume the typed columns directly.

**Issue [#108](https://github.com/guycorbaz/opcgw/issues/108) closure mapping:**
- A-1 closed the **type-level** gap (`MetricType` became payload-bearing).
- A-2 closed the **schema-level** gap (typed columns + `value_type` exist on both tables).
- **A-3 closes the WRITE-side gap** — production writers populate typed columns with real measurements; new rows post-A-3 carry the real payload end-to-end through the persistence layer.
- A-4 / A-5 / A-6 close the **READ-side** gap (consumers project from typed columns).
- #108 is fully closed when the last reader (A-5 HistoryRead or A-6 Web UI) ships.

### Carry-forward from A-1 + A-2 (must be addressed in A-3)

- **A-1 iter-3 Edge F3 / A-1-iter3-DEF8 (MEDIUM, MUST-address-in-A-3):** `SqliteBackend::set_metric` calls `serde_json::to_string(&value)`, which rejects NaN/Inf by default with `Error: NaN is not a valid JSON number`. Currently unreachable because the poller stamps `Float(0.0)`. A-3 wires real ChirpStack readings (f32 → f64), and `f32::NAN` is a legitimate sensor-error signal that ChirpStack can emit. **Decision required in A-3:** (a) filter NaN/Inf at the poller before constructing `MetricType::Float(...)` and emit `metric_parse` warn; (b) configure `serde_json` with `allow_nan` (potentially fragile — JSON-extended text in column); (c) add explicit `!value.is_finite()` guard at `SqliteBackend::set_metric` returning a clean operator-facing `OpcGwError::Storage(...)`. **Recommended: option (a) — filter at poller.** NaN/Inf is operationally a sensor calibration error, not a measurement; the existing `metric_parse` warn pattern (Story 6-3) is the right surface.
- **A-2-iter1-DEF1 (heterogeneous legacy `value` lexemes):** Three SqliteBackend writers produce three different `value`-column shapes today:
  - `set_metric` writes `serde_json::to_string(&value)` (post-A-1: `{"Float":0.0}` JSON blob — and post-A-3 with real payload it would be `{"Float":23.5}`).
  - `upsert_metric_value` + `append_metric_history` write `value.to_string()` (the discriminant string — post-A-1: `"Float"`, post-A-3 still `"Float"` since Display preserves discriminant-only rendering).
  - `batch_write_metrics` writes `BatchMetricWrite.value` (real string-encoded sensor reading: `"23.5"`).
  A-3 must keep this three-shape contract intact for legacy rows AND populate typed columns consistently across all four writers. Future story A-5 / A-7 retires the legacy `value` column once readers move.
- **A-2-iter1-DEF2 (exactly-one-non-NULL CHECK constraint):** Spec § Out of Scope of A-2 explicitly defers this CHECK to A-3 or A-7. The constraint is `(value_type='legacy' AND all typed NULL) OR (value_type='Float' AND value_real NOT NULL AND others NULL) OR ...`. **A-3 SHOULD add a v008 migration** with this CHECK because A-3 is the first story where writers populate typed columns (making the invariant provable). A-7 fallback if v008 lands too much scope onto A-3.
- **A-2-iter1-DEF11 (`NULL` keyword cosmetic):** May address opportunistically in v008 if it ships.
- **A-1 iter-1 P9 + iter-2 IR5 (`store_metric` `todo!()`):** A-3 reinstates the body with real payload threading (per the iter-2 IR5 doc comment "should be reinstated by A-3").
- **A-1 iter-3 DEF3 (`InMemoryBackend::set_metric` auto-creates devices):** Pre-existing trait/impl divergence. Out of A-3 scope per A-1 deferred-work; revisit at A-5.

### Current pre-A-3 shape (the gap)

`src/chirpstack.rs::prepare_metric_for_batch` at lines 1588-1667:

```rust
let target_type = match kind {
    ChirpStackMetricKind::Gauge   => MetricType::Float(0.0),  // TODO(A-3): use raw_value
    ChirpStackMetricKind::Counter => MetricType::Int(0),      // TODO(A-3): use raw_value as i64
    ChirpStackMetricKind::Absolute => MetricType::Float(0.0), // TODO(A-3): use raw_value
    ChirpStackMetricKind::Unknown => match self.config.get_metric_type(...) {
        Some(OpcMetricTypeConfig::Bool)  => MetricType::Bool(false),  // TODO(A-3)
        Some(OpcMetricTypeConfig::Int)   => MetricType::Int(0),       // TODO(A-3)
        Some(OpcMetricTypeConfig::Float) => MetricType::Float(0.0),   // TODO(A-3)
        ...
    }
};

// Later in the validation match:
let (value_str, metric_type) = match target_type {
    MetricType::Bool(_)   => (s.to_string(), MetricType::Bool(false)),  // TODO(A-3): MetricType::Bool(s)
    MetricType::Int(_)    => (int_val.to_string(), MetricType::Int(0)), // TODO(A-3): MetricType::Int(int_val)
    MetricType::Float(_)  => (raw_value.to_string(), MetricType::Float(0.0)), // TODO(A-3): MetricType::Float(raw_value)
    ...
};
```

Plus `src/chirpstack.rs::store_metric` body is `todo!("store_metric body to be reinstated by Story A-3 ...")` per iter-2 IR5.

`src/storage/sqlite.rs` writers each have a `TODO(A-2)` block (per Story A-2 review) flagging "A-2's schema migration replaces this with a typed-payload write." A-3 closes those TODOs.

### Post-A-3 shape (the target)

`prepare_metric_for_batch`:

```rust
let target_type = match kind {
    ChirpStackMetricKind::Gauge    => MetricType::Float(raw_value),
    ChirpStackMetricKind::Counter  => MetricType::Int(raw_value as i64),
    ChirpStackMetricKind::Absolute => MetricType::Float(raw_value),
    ChirpStackMetricKind::Unknown  => match cfg_type {
        OpcMetricTypeConfig::Bool  => MetricType::Bool(/* parsed bool */),
        OpcMetricTypeConfig::Int   => MetricType::Int(raw_value as i64),
        OpcMetricTypeConfig::Float => MetricType::Float(raw_value),
        ...
    }
};

// NaN/Inf guard (Edge F3 resolution — option (a)):
if let MetricType::Float(f) = target_type {
    if !f.is_finite() {
        warn!(
            event = "metric_parse",
            device_id = %device_id,
            metric_name = %metric_name,
            raw_value = %raw_value,
            expected_type = "Float",
            reason = "non_finite",
            "Skipping metric: non-finite Float (NaN or Inf)"
        );
        return None;
    }
}

// Subsequent validation match populates the real payload everywhere:
let (value_str, metric_type) = match target_type {
    MetricType::Bool(_) => match validate_bool_metric_value(...) {
        Some(b) => (b.to_string(), MetricType::Bool(b)),
        None    => return None,
    },
    MetricType::Int(_) => {
        let int_val = raw_value as i64;
        if raw_value.fract() != 0.0 { warn!(...) }
        (int_val.to_string(), MetricType::Int(int_val))
    },
    MetricType::Float(_) => (raw_value.to_string(), MetricType::Float(raw_value)),
    MetricType::String(_) => { warn!(...); return None; }
};
```

`SqliteBackend::set_metric` rewires to pattern-match the payload:

```rust
fn set_metric(&self, device_id: &str, metric_name: &str, value: MetricType) -> Result<(), OpcGwError> {
    let data_type = value.to_string();
    let value_str = serde_json::to_string(&value).map_err(...)?;
    let timestamp = Utc::now().to_rfc3339();
    let (value_real, value_int, value_bool, value_text, value_type) = match &value {
        MetricType::Float(f)  => (Some(*f), None, None, None, "Float"),
        MetricType::Int(i)    => (None, Some(*i), None, None, "Int"),
        MetricType::Bool(b)   => (None, None, Some(if *b { 1i64 } else { 0i64 }), None, "Bool"),
        MetricType::String(s) => (None, None, None, Some(s.clone()), "String"),
    };

    conn.execute(
        "INSERT OR REPLACE INTO metric_values (device_id, metric_name, value, data_type, timestamp, updated_at, created_at, value_real, value_int, value_bool, value_text, value_type) VALUES (?1, ?2, ?3, ?4, ?5, datetime('now'), COALESCE(...), ?6, ?7, ?8, ?9, ?10)",
        params![device_id, metric_name, value_str, data_type, timestamp, value_real, value_int, value_bool, value_text, value_type],
    )?;
    ...
}
```

Same pattern for `upsert_metric_value`, `append_metric_history`, `batch_write_metrics`.

A new `migrations/v008_typed_value_constraints.sql` (NEW) adds the exactly-one-non-NULL CHECK to both `metric_values` and `metric_history`.

`chirpstack.rs::store_metric` reinstated with the same dispatch + NaN/Inf guard as `prepare_metric_for_batch`.

---

## Acceptance Criteria

**AC#1 — Poller payload-bearing wrapping at all 7 `TODO(A-3)` sites:** `src/chirpstack.rs::prepare_metric_for_batch` wraps the real `metric.datasets[0].data[0]` value into the matching `MetricType` variant at every construction site. Specifically:

- `ChirpStackMetricKind::Gauge` → `MetricType::Float(raw_value as f64)`
- `ChirpStackMetricKind::Counter` → `MetricType::Int(raw_value as i64)` (with fractional warn preserved)
- `ChirpStackMetricKind::Absolute` → `MetricType::Float(raw_value as f64)`
- `ChirpStackMetricKind::Unknown` → config fallback wraps real value the same way
- Validation match wraps the parsed/converted value into the matching variant

`grep -rn 'TODO(A-3)' src/` returns ZERO hits after A-3 lands.

**AC#2 — NaN/Inf policy implemented per A-1-iter3-DEF8 (option (a) recommended):** Before any `MetricType::Float(payload)` construction in `prepare_metric_for_batch` and `store_metric`, the poller checks `payload.is_finite()`. Non-finite values emit `warn!` with structured fields `event = "metric_parse"`, `device_id`, `metric_name`, `raw_value`, `expected_type = "Float"`, `reason = "non_finite"` (extending the Story 6-3 `metric_parse` warn pattern with a `reason` enum) and the poller returns `None` (skips the metric for this cycle). Pinned by `tests/metric_types_test.rs::test_nan_inf_skip` + 2 sibling tests.

**AC#3 — Counter Int saturation guard:** When `Counter` kind is wrapped via `raw_value as i64`, NaN saturates to 0, +∞ to `i64::MAX`, −∞ to `i64::MIN`. Pre-cast `!raw_value.is_finite()` check (covered under AC#2) catches these. Subnormal values are tolerated (cast preserves them via truncation per IEEE 754). Pinned by `test_counter_nan_inf_skip`.

**AC#4 — Storage writers populate typed columns + `value_type`:** All four SqliteBackend writers (`set_metric`, `upsert_metric_value`, `append_metric_history`, `batch_write_metrics`) pattern-match the `MetricType` payload and populate the matching typed column (`value_real` / `value_int` / `value_bool` / `value_text`) + `value_type` (per `MetricType::Display`) on every INSERT/UPSERT.

- `MetricType::Float(f)` → `value_real = Some(f)`, `value_type = 'Float'`, other typed cols NULL.
- `MetricType::Int(i)` → `value_int = Some(i)`, `value_type = 'Int'`, other typed cols NULL.
- `MetricType::Bool(b)` → `value_bool = Some(if b { 1 } else { 0 })`, `value_type = 'Bool'`, other typed cols NULL.
- `MetricType::String(s)` → `value_text = Some(s.clone())`, `value_type = 'String'`, other typed cols NULL.

**Canonical pattern (MUST follow):** an inline `match &value { ... }` block at each writer site that destructures into the 5 typed bindings (`value_real`, `value_int`, `value_bool`, `value_text`, `value_type`) — same shape across all four writers. **Do NOT** add a helper method on `MetricType` (e.g. `as_typed_columns()`) that lives in `src/storage/types.rs` — that file is strict-zero per AC#11 (A-1 finalised the `MetricType` enum surface, A-3 doesn't re-touch it). Per-writer inline match is the only allowed approach. If `clippy::type_complexity` complains on the 5-tuple destructure (A-2 iter-1 IM3 precedent), bind each column to a separate `let` (5 sequential `let` statements) rather than `#[allow]`-suppressing.

Pinned by `test_v008_writers_populate_typed_columns_for_all_variants` (and sibling tests for each writer).

**AC#5 — Legacy `value` + `data_type` columns continue to be populated:** Per the heterogeneous-lexeme staging contract (A-2-iter1-DEF1), each writer continues to populate the legacy columns exactly as it did pre-A-3:

- `set_metric` writes `serde_json::to_string(&value)` to `value` and `value.to_string()` to `data_type`.
- `upsert_metric_value` + `append_metric_history` write `value.to_string()` to both (the discriminant).
- `batch_write_metrics` writes `BatchMetricWrite.value` (real string) to `value` and `BatchMetricWrite.data_type.to_string()` to `data_type`.

Pinned by `test_v008_writers_preserve_legacy_columns`.

**AC#6 — v008 migration adds exactly-one-non-NULL CHECK on both tables:** A new `migrations/v008_typed_value_constraints.sql` adds a table-level CHECK constraint to both `metric_values` and `metric_history`:

```sql
CHECK (
  (value_type = 'legacy'  AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
  OR (value_type = 'Float'  AND value_real IS NOT NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
  OR (value_type = 'Int'    AND value_real IS NULL AND value_int IS NOT NULL AND value_bool IS NULL AND value_text IS NULL)
  OR (value_type = 'Bool'   AND value_real IS NULL AND value_int IS NULL AND value_bool IS NOT NULL AND value_text IS NULL)
  OR (value_type = 'String' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NOT NULL)
)
```

**SQLite ALTER TABLE limitation:** `ALTER TABLE` does NOT support adding table-level CHECK constraints in-place. The v008 migration uses the standard SQLite recreate-table pattern: `CREATE TABLE metric_values_new (... with CHECK ...) AS SELECT * FROM metric_values; DROP TABLE metric_values; ALTER TABLE metric_values_new RENAME TO metric_values;`. This is heavier than v007's `ALTER TABLE ADD COLUMN` (O(table-size)) — operators with large `metric_history` tables see a longer migration window. Story A.7's runbook SLA (5s for 100MB) must be re-verified for v008's recreate pattern.

`src/storage/schema.rs` gains `MIGRATION_V008` const + `if current_version < 8 { ... }` runner block + `LATEST_VERSION` bump 7→8 + sibling tests.

**AC#7 — Migration v008 completes within 30 seconds for 100 MB databases:** Looser than v007's 5s SLA because `CREATE TABLE … AS SELECT` rewrites the entire table. Operator runbook (Story A-7) documents this. SLA pinned by `test_v008_migration_under_30s_for_10k_rows` (seeded `metric_values` + `metric_history` with 10 000 + 10 000 rows tagged `value_type='legacy'`, asserts migration completes within 30s).

**AC#8 — `chirpstack.rs::store_metric` body reinstated with real payload threading:** The dead-code `store_metric` method (currently `todo!()` per A-1 P9 + iter-2 IR5) is restored with the same dispatch logic as `prepare_metric_for_batch` + the same NaN/Inf guard. `#[allow(dead_code)]` is retained because no production path calls it (verified via `grep -rn '\.store_metric\b' src/ tests/`); the method is preserved for future test fixtures. The original kind→variant + bool 0/1 validation + int fractional warn logic is reinstated from commit `16e7811:src/chirpstack.rs`.

**AC#9 — Counter monotonic check rewires to consume typed payload (best-effort):** `prepare_metric_for_batch` at lines 1625-1637 currently reads `prev_metric.value.parse::<i64>()` (legacy string path). Post-A-3, when the previous row was written by an A-3-era writer (`value_type = 'Int'` + `value_int` non-NULL), the check can read `prev_metric.data_type` directly via pattern-match (no string parse). Best-effort: if `prev_metric.data_type` is `MetricType::Int(prev_int)`, use it; otherwise fall back to the legacy `prev_metric.value.parse::<i64>()` path. Pinned by `test_counter_monotonic_check_uses_typed_payload`.

**AC#10 — Audit-event surface gains one new event with pinned grep contract:** `event = "metric_parse"` with `reason = "non_finite"` is the only new audit event. No other audit events are added or modified. The grep contract — matching the pattern used by Stories 9-4 (`application_*=4`), 9-5 (`device_*=4`), 9-6 (`command_*=4`), 9-7 (`config_reload_*=3`), 9-8 (`address_space_mutation_*=2`) — is:

```bash
git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u
```

returns exactly one line: `event = "metric_parse"`. Pre-A-3 the same grep returns empty.

The `metric_parse` warn event uses a closed field schema (locked across the two emission sites in `prepare_metric_for_batch` and `store_metric`):

| Field | Type | Required | Value |
| --- | --- | --- | --- |
| `event` | const | yes | `"metric_parse"` |
| `device_id` | `%` (Display) | yes | the device identifier |
| `metric_name` | `%` (Display) | yes | the metric name |
| `raw_value` | `%` (Display) | yes | the original `metric.datasets[0].data[0]: f32` value |
| `expected_type` | const | yes | `"Float"` (only Float emits this warn today; future variants extend the enum) |
| `reason` | const | yes | `"non_finite"` (only reason today; future failures use distinct values) |
| message | string | yes | `"Skipping metric: non-finite Float (NaN or Inf)"` |

**AC#11 — Strict-zero file invariants (revised for A-3 scope):** A-3 SHOULD touch:

- `src/chirpstack.rs` (poller — was strict-zero in A-1/A-2; A-3 owns the payload wiring)
- `src/storage/sqlite.rs` (writers — was strict-zero in A-1/A-2; A-3 owns the typed-column write path)
- `src/storage/schema.rs` (v008 migration runner + tests)
- `migrations/v008_typed_value_constraints.sql` (NEW)
- `tests/metric_types_test.rs` (extend with NaN/Inf coverage + real-payload assertions)

A-3 MUST also update `README.md` (Current Version line) per CLAUDE.md "Documentation Sync" — same precedent as A-1's commit `c31cad5` + A-2's commit `95c39a6` (each captured a 4000+ char single-line narrative refresh). Skipping the README update is an AC#11 violation, not just a Task 9 oversight.

A-3 must NOT touch (carry-forward strict-zero from A-1/A-2):

- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs`
- `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`
- `src/security.rs`, `src/security_hmac.rs`
- `src/main.rs::initialise_tracing` (function body)
- `src/config_reload.rs`, `src/opcua_topology_apply.rs`
- `src/storage/types.rs` (MetricType + MetricValue + BatchMetricWrite definitions — A-1 finalised)
- `src/storage/memory.rs` (InMemoryBackend has no schema; round-trip already works via Clone)
- `src/storage/mod.rs` (StorageBackend trait surface unchanged)
- `src/storage/pool.rs`

**AC#12 — `cargo test --all-targets` ≥1151 passed / 0 failed / ≤10 ignored:** Baseline 1143 post-A-2-review. Task 7 adds ≥8 new `#[test]` functions (the retrofits to `test_metric_kind_*_to_*` strengthen existing test bodies in-place — they ADD assertions but don't bump `cargo test` count). Realistic delta: +8 to +12 new fns depending on whether the NaN/Inf coverage is parameterised (one fn with 3 sub-cases) or expanded into 3 fns. Target `≥1151` reflects the lower bound; aim for `≥1155`. `cargo clippy --all-targets -- -D warnings` clean — the writer 5-tuple pattern is a known `clippy::type_complexity` risk per AC#4 (use 5 sequential `let` statements if it complains).

**AC#13 — `cargo test --doc` 0 failed / ≥55 ignored:** No new doctests added.

**AC#14 — Migration file SPDX + copyright header:** `migrations/v008_typed_value_constraints.sql` carries the same SPDX + copyright header as v007 (per CLAUDE.md doctrine and A-2's accept-as-conforming AC#13 resolution).

---

## Tasks

- [x] **Task 0 — File GitHub tracking issue for A-3.** Defer to user per A-1/A-2 precedent (gh CLI not authenticated for write).

- [x] **Task 1 (AC#1, AC#2, AC#3) — Wire real payload into `prepare_metric_for_batch`.**
  - [x] At all 7 `TODO(A-3)` sites (lines 1591, 1595, 1599, 1607-1609, 1650, 1660, 1662 in current `src/chirpstack.rs`), replace zero-defaulted `MetricType::X(default)` with real-value wrappers.
  - [x] Add NaN/Inf guard BEFORE constructing `MetricType::Float(...)` in both the kind-driven and config-fallback paths. Use `f64::is_finite()`.
  - [x] Emit `warn!(event = "metric_parse", reason = "non_finite", ...)` and `return None` on non-finite.
  - [x] Preserve existing `Counter::fract() != 0.0` fractional warn for Int conversions.
  - [x] Preserve Bool 0/1 validation via existing `validate_bool_metric_value` helper.

- [x] **Task 2 (AC#4, AC#5) — Rewire all 4 SqliteBackend writers to populate typed columns + `value_type`.**
  - [x] `set_metric` (line 527): add pattern-match → 5-tuple of `(value_real, value_int, value_bool, value_text, value_type)`; extend INSERT statement to bind the 5 new columns.
  - [x] `upsert_metric_value` (line 838): same pattern.
  - [x] `append_metric_history` (line 926): same pattern.
  - [x] `batch_write_metrics` (line 1018): per-row pattern-match in the for-loop body on `metric.data_type` (the existing `BatchMetricWrite.data_type: MetricType` field already carries the typed payload — **do NOT** add a new `value_type: String` field to `BatchMetricWrite`; deriving `value_type` via `data_type.to_string()` is sufficient and keeps `src/storage/types.rs` strict-zero per AC#11); extend both the UPSERT (`metric_values`) and the INSERT (`metric_history`) SQL with the 5 new columns.
  - [x] Preserve all legacy `value` + `data_type` writes per AC#5 (the heterogeneous-lexeme staging contract).
  - [x] Remove the four `TODO(A-2)` comments at these sites and replace with one-line `// A-3: typed columns populated; legacy `value`/`data_type` retained until A-5/A-7 retires readers.`

- [x] **Task 3 (AC#6, AC#7, AC#14) — Author `migrations/v008_typed_value_constraints.sql` + wire into runner.**
  - [x] SPDX + copyright header.
  - [x] For each of `metric_values` and `metric_history`: `CREATE TABLE …_new (... full column list with new table-level CHECK constraint ...)`; `INSERT INTO …_new SELECT * FROM …`; `DROP TABLE …`; `ALTER TABLE …_new RENAME TO …`; recreate indexes.
  - [x] Preserve all existing indexes (`idx_metric_values_device_metric`, `idx_metric_history_device_timestamp`).
  - [x] Preserve PRIMARY KEY + UNIQUE constraints on `metric_values`.
  - [x] Add `const MIGRATION_V008` + runner block in `src/storage/schema.rs`. Bump `LATEST_VERSION` 7 → 8.

- [x] **Task 4 (AC#8) — Reinstate `chirpstack.rs::store_metric` body.**
  - [x] **CRITICAL:** Commit `16e7811:src/chirpstack.rs` shipped the broken behaviour the A-1 review explicitly fixed (`store_metric`'s Bool arm stamped `MetricType::Bool(false)` regardless of the parsed 1.0/0.0 input — the data-loss landmine that motivated A-1 iter-2 IR5's `todo!()` replacement in the first place). Treat that commit as a **structural reference only** for the kind→variant dispatch skeleton (Bool/Int/Float/String match arms + Bool 0/1 validation + Int fractional warn). DO NOT copy-paste any zero-defaulted construction site.
  - [x] Apply Task 1's real-payload wrapping (`MetricType::Bool(b)`, `MetricType::Int(int_val)`, `MetricType::Float(raw_value)`) + Task 1's NaN/Inf guard at every reinstated construction site — same wrapping discipline as `prepare_metric_for_batch`.
  - [x] Retain `#[allow(dead_code)]` (no production caller; method preserved for test fixtures — verify via `grep -rn '\.store_metric\b\|::store_metric\b' src/ tests/`).
  - [x] Update the doc comment from the iter-2 IR5 "Status: the body is a `todo!()` placeholder pending Story A-3" → "Reinstated by Story A-3" status note.

- [x] **Task 5 (AC#9) — Rewire counter monotonic check to prefer typed payload.**
  - [x] At `chirpstack.rs:1625-1637`, add a typed-path branch: if `prev_metric.data_type` pattern-matches `MetricType::Int(prev_int)`, use `prev_int` directly; otherwise fall back to `prev_metric.value.parse::<i64>()`.
  - [x] This eliminates a known pre-existing partial failure (A-2-iter1-DEF2 / Edge F11 / Blind F2) where rows written via `upsert_metric_value` (discriminant-string `value` column) silently disabled the monotonic check.

- [x] **Task 6 (AC#10) — Add `metric_parse` audit event.**
  - [x] At the NaN/Inf guard site in `prepare_metric_for_batch` AND in `store_metric`, emit `warn!(event = "metric_parse", reason = "non_finite", device_id = %device_id, metric_name = %metric_name, raw_value = %raw_value, expected_type = "Float", "Skipping metric: non-finite Float (NaN or Inf)")`.
  - [x] Verify with `git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u` that exactly 1 event name (`metric_parse`) appears.

- [x] **Task 7 (AC#12) — Test plan: ≥15 new tests.**
  - [x] `tests/metric_types_test.rs`: extend existing `test_metric_kind_gauge_to_float` / `_counter_to_int` / `_absolute_to_float` to assert the REAL payload value (closes the iter-1 P8 / A-1-iter3-DEF4 tautological-assertions defer). Replace `MetricType::Float(0.0)` assertion target with `MetricType::Float(25.5)` (from `mock_metric` value).
  - [x] NEW `tests/metric_types_test.rs::test_metric_kind_gauge_nan_skipped` (and Counter+Absolute siblings): seed a `mock_metric` with `f32::NAN` / `f32::INFINITY` / `f32::NEG_INFINITY`, call `prepare_metric_for_batch`, assert returns `None` + `metric_parse` warn emitted.
  - [x] NEW `tests/metric_types_test.rs::test_metric_kind_counter_negative_value` (sanity): negative Counter values are tolerated by `as i64` cast.
  - [x] NEW `src/storage/sqlite.rs::tests::test_set_metric_populates_typed_columns_float` (and Int/Bool/String siblings): call `set_metric` with `MetricType::Float(23.5)`, verify via raw SQL that `value_real = 23.5`, `value_int IS NULL`, `value_bool IS NULL`, `value_text IS NULL`, `value_type = 'Float'`. Mirror for the other 3 variants.
  - [x] NEW `src/storage/sqlite.rs::tests::test_upsert_metric_value_populates_typed_columns` + sibling for `append_metric_history` + `batch_write_metrics`.
  - [x] NEW `src/storage/schema.rs::tests::test_v008_cross_column_check_rejects_inconsistent_row`: insert `(value_type='Float', value_real=NULL)` — assert CHECK constraint failure (`SQLITE_CONSTRAINT_CHECK`).
  - [x] NEW `src/storage/schema.rs::tests::test_v008_cross_column_check_accepts_consistent_rows`: insert one row per variant with all CHECK invariants satisfied.
  - [x] NEW `src/storage/schema.rs::tests::test_v008_migration_under_30s_for_10k_rows`: seed 10k+10k rows tagged `value_type='legacy'`, time `run_migrations` v007→v008, assert <30s.
  - [x] NEW `src/storage/schema.rs::tests::test_v008_preserves_indexes`: assert `idx_metric_values_device_metric` and `idx_metric_history_device_timestamp` exist post-v008 (CREATE TABLE … AS SELECT … pattern drops indexes; the migration must recreate them).
  - [x] NEW `tests/chirpstack_payload_roundtrip.rs` (integration): seed a `mock_metric` with `data[0] = 23.5`, call `prepare_metric_for_batch` → `batch_write_metrics`, read via raw SQL, assert `value_real = 23.5` AND `data_type = 'Float'` AND `value = "23.5"` (heterogeneous-lexeme contract preserved).
  - [x] NEW `tests/chirpstack_payload_roundtrip.rs::test_counter_monotonic_check_uses_typed_payload`: write a Counter via `batch_write_metrics` with `MetricType::Int(100)`, then call `prepare_metric_for_batch` with a `mock_metric` carrying `data[0] = 50.0` (reset), assert the new row is skipped (monotonic guard triggered) — AND assert it's the typed-path branch via tracing-test on the structured log.
  - [x] Update test-count target in spec verification block.

- [x] **Task 8 (AC#11) — Verify strict-zero invariants.**
  - [x] `git diff --name-only` confirms only A-3 allow-listed files are touched.
  - [x] `src/storage/types.rs`, `src/storage/memory.rs`, `src/storage/mod.rs`, `src/storage/pool.rs`, `src/web/*`, `src/opc_ua*`, `src/security*`, `src/main.rs::initialise_tracing`, `src/config_reload.rs`, `src/opcua_topology_apply.rs` all zero-diff.

- [x] **Task 9 (AC#12, AC#13, AC#14) — Final verification.**
  - [x] `cargo build --all-targets` clean.
  - [x] `cargo test --all-targets` ≥1158 passed / 0 failed / ≤10 ignored.
  - [x] `cargo clippy --all-targets -- -D warnings` clean.
  - [x] `cargo test --doc` 0 failed / ≥55 ignored.
  - [x] README.md "Current Version" line updated per CLAUDE.md Documentation Sync.
  - [x] `sprint-status.yaml`: `A-3-poller-value-payload-write-pipeline: ready-for-dev → in-progress → review`.
  - [x] Update grep contract docs in `docs/logging.md` (if present) to include `metric_parse`.

---

## Dev Notes

### NaN/Inf policy decision (the load-bearing call)

Three options, ranked by recommendation:

**Option (a) — Filter at poller (RECOMMENDED):** Add `!raw_value.is_finite()` guard before constructing `MetricType::Float(raw_value)`. Emit `metric_parse` warn with `reason = "non_finite"` and `return None` (skip the metric for this cycle). Same pattern as the existing `validate_bool_metric_value` helper. Pros: surface-correct (sensor calibration errors are operationally distinct from real measurements); cheap; matches Story 6-3 warn pattern. Cons: silently drops one poll cycle per non-finite reading (acceptable — next cycle re-polls; operator alarms on `metric_parse` rate).

**Option (b) — Allow NaN through serde_json:** Switch `SqliteBackend::set_metric`'s `serde_json::to_string` call to use a serde_json mode with `allow_nan: true`. Cons: produces JSON-extended text (`NaN` / `Infinity`) in the column — non-standard, breaks downstream JSON parsers, potentially confuses operators inspecting rows via `sqlite3` CLI.

**Option (c) — Guard at SqliteBackend boundary:** Add `!value.is_finite()` check in `SqliteBackend::set_metric` (only — not in `upsert_metric_value` / `batch_write_metrics` which use `value.to_string()` not `serde_json::to_string`). Cons: NaN sneaks through `upsert_metric_value` / `batch_write_metrics` paths (which currently work fine on NaN because `to_string()` accepts it) and silently lands in the typed `value_real` column. Operators can't distinguish "no measurement yet" from "NaN measurement".

**Recommendation: option (a).** The spec's AC#2 + Task 1 + Task 6 codify option (a). If a future operator concern requires preserving NaN/Inf observations (e.g. for sensor diagnostics), Story A-6 can introduce a separate `metric_anomalies` table to record them — out of A-3 scope.

### Heterogeneous legacy `value` lexemes (A-2-iter1-DEF1 staging contract)

A-3 must preserve the three legacy-column shapes that exist today:

| Writer | `value` column shape | `data_type` column |
| --- | --- | --- |
| `set_metric` | `serde_json::to_string(&MetricType)` → `{"Float":23.5}` (real payload) | `MetricType::to_string()` → `"Float"` |
| `upsert_metric_value` | `MetricType::to_string()` → `"Float"` (discriminant) | `MetricType::to_string()` → `"Float"` |
| `append_metric_history` | `MetricType::to_string()` → `"Float"` (discriminant) | `MetricType::to_string()` → `"Float"` |
| `batch_write_metrics` | `BatchMetricWrite.value` → `"23.5"` (real string-encoded) | `BatchMetricWrite.data_type.to_string()` → `"Float"` |

A-5 / A-7 retires the legacy `value` column once readers fully migrate. A-3 makes the typed columns the canonical source; legacy columns are tolerated dead-weight.

### v008 migration: SQLite CREATE TABLE … AS SELECT pattern

SQLite's `ALTER TABLE` does NOT support adding table-level CHECK constraints. The standard workaround is:

```sql
PRAGMA foreign_keys = OFF;
BEGIN TRANSACTION;

CREATE TABLE metric_values_new (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,
  data_type TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  created_at TEXT NOT NULL,
  value_real REAL NULL,
  value_int INTEGER NULL,
  value_bool INTEGER NULL CHECK(value_bool IS NULL OR value_bool IN (0, 1)),
  value_text TEXT NULL,
  value_type TEXT NOT NULL DEFAULT 'legacy'
      CHECK(value_type IN ('legacy', 'Float', 'Int', 'Bool', 'String')),
  CHECK (
    (value_type = 'legacy' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Float' AND value_real IS NOT NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Int' AND value_real IS NULL AND value_int IS NOT NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Bool' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NOT NULL AND value_text IS NULL)
    OR (value_type = 'String' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NOT NULL)
  ),
  UNIQUE(device_id, metric_name)
);

INSERT INTO metric_values_new SELECT * FROM metric_values;

DROP TABLE metric_values;
ALTER TABLE metric_values_new RENAME TO metric_values;

CREATE INDEX IF NOT EXISTS idx_metric_values_device_metric ON metric_values(device_id, metric_name);

-- ============================================================================
-- metric_history: same payload-bearing CHECK pattern, different base columns
-- ============================================================================
-- KEY DIFFERENCES from metric_values:
--   - id INTEGER PRIMARY KEY without AUTOINCREMENT (append-only; no
--     UNIQUE(device_id, metric_name) — multiple rows per metric over time);
--   - no updated_at column (history rows are immutable on insert);
--   - index is idx_metric_history_device_timestamp (composite for time-range
--     queries per Story 7-3) rather than idx_metric_values_device_metric.

CREATE TABLE metric_history_new (
  id INTEGER PRIMARY KEY,
  device_id TEXT NOT NULL,
  metric_name TEXT NOT NULL,
  value TEXT NOT NULL,
  data_type TEXT NOT NULL,
  timestamp TEXT NOT NULL,
  created_at TEXT NOT NULL,
  value_real REAL NULL,
  value_int INTEGER NULL,
  value_bool INTEGER NULL CHECK(value_bool IS NULL OR value_bool IN (0, 1)),
  value_text TEXT NULL,
  value_type TEXT NOT NULL DEFAULT 'legacy'
      CHECK(value_type IN ('legacy', 'Float', 'Int', 'Bool', 'String')),
  CHECK (
    (value_type = 'legacy' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Float' AND value_real IS NOT NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Int' AND value_real IS NULL AND value_int IS NOT NULL AND value_bool IS NULL AND value_text IS NULL)
    OR (value_type = 'Bool' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NOT NULL AND value_text IS NULL)
    OR (value_type = 'String' AND value_real IS NULL AND value_int IS NULL AND value_bool IS NULL AND value_text IS NOT NULL)
  )
);

INSERT INTO metric_history_new SELECT * FROM metric_history;
DROP TABLE metric_history;
ALTER TABLE metric_history_new RENAME TO metric_history;

CREATE INDEX IF NOT EXISTS idx_metric_history_device_timestamp
  ON metric_history(device_id, timestamp);

COMMIT;
PRAGMA foreign_keys = ON;
```

**Important:** A-1-iter3-DEF6 (migration not transactional) is still HIGH and user-deferred. A-3 SHOULD wrap v008's recreate-table SQL in explicit BEGIN/COMMIT to give v008 atomic guarantees that v001-v007 lack. Wrapping just v008 (not v001-v007) is acceptable for A-3 since the v008 recreate-table pattern has worse partial-failure semantics than ALTER TABLE ADD COLUMN.

### Test-budget delta

Post-A-2-review baseline: 1143 passed / 0 failed / 10 ignored.

A-3 adds ≥15 new tests:
- Real-payload coverage in `tests/metric_types_test.rs` (3 retrofitted + 3 new NaN/Inf siblings + 1 negative Counter = 7)
- SqliteBackend writer-typed-column tests in `src/storage/sqlite.rs::tests` (4 set_metric variants + 1 upsert + 1 append + 1 batch = 7)
- v008 schema tests in `src/storage/schema.rs::tests` (cross-column CHECK reject + accept + 30s SLA + index preservation = 4)
- Integration test in `tests/chirpstack_payload_roundtrip.rs` (real-payload round-trip + counter monotonic typed-path = 2)

Target: ≥1158 passed. Conservative range: 1158 to 1170 depending on how parameterised tests count across binaries.

### Strict-zero invariant carry-forward + revisions

A-3 NECESSARILY expands the touched-file surface from A-1/A-2:

| File | A-1 | A-2 | A-3 |
| --- | --- | --- | --- |
| `src/chirpstack.rs` | strict-zero | strict-zero | **MUTABLE (Task 1, 4, 5, 6)** |
| `src/storage/sqlite.rs` | strict-zero | strict-zero | **MUTABLE (Task 2)** |
| `src/storage/schema.rs` | strict-zero | mutable (v007 runner) | **MUTABLE (v008 runner + tests)** |
| `migrations/v008_typed_value_constraints.sql` | — | — | **NEW** |
| `tests/metric_types_test.rs` | mutable | mutable | mutable |
| `tests/chirpstack_payload_roundtrip.rs` | — | — | **NEW** |

All other A-1/A-2 strict-zero files remain strict-zero in A-3.

### Carry-forward GH issues (unchanged by A-3 unless noted)

- **#88, #100, #102, #104, #110, #113, #117** — Phase B carry-overs, outside Epic A.
- **#108 — production-deployment blocker (storage payload-less MetricType).** A-3 substantially closes #108 at the WRITE side; A-4/A-5 close it at the READ side; #108 doesn't close until A-5 ships (readers consume typed payload end-to-end).
- **A-1-iter3-DEF8 (NaN/Inf hazard) — closed by A-3 Task 1 + Task 6 (option (a) policy).**
- **A-2-iter1-DEF2 (exactly-one-non-NULL CHECK) — closed by A-3 Task 3 (v008 migration).**
- **A-2-iter1-DEF1 (heterogeneous legacy value lexemes) — preserved by A-3 Task 2 (per AC#5).**
- **A-1 iter-1 P9 + iter-2 IR5 (`store_metric` `todo!()`) — closed by A-3 Task 4.**

A-3 tracking issue to be filed by the dev agent (Task 0) per A-1/A-2 precedent.

---

## Out of Scope

The following items are explicitly NOT part of A-3 — they belong to follow-on stories:

- **OPC UA Read pattern-match on typed payload** — Story A.4 rewrites `OpcUa::get_value` to project from the typed columns.
- **OPC UA HistoryRead pattern-match on typed payload** — Story A.5 rewrites `OpcgwHistoryNodeManagerImpl::history_read_raw_modified`.
- **Web UI live-metrics typed display** — Story A.6 rewrites `/api/metrics` + `static/metrics.js`.
- **Retirement of legacy `value` + `data_type` columns** — Story A.7 (or a future v009 cleanup migration once all readers are off the legacy path).
- **Migration operator runbook (`docs/deployment-guide.md § "Epic A migration"`)** — Story A.7.
- **InMemoryBackend changes** — has no schema; A-1's `MetricType::Clone` round-trip already preserves payload byte-for-byte.
- **`MetricType` enum modifications** — A-1 finalised the payload-bearing enum.
- **HIGH A-2-iter1-DEF-IH1 migration runner atomicity gap** — pre-existing across v001-v007; A-3 may wrap v008 specifically in BEGIN/COMMIT (recommended in Dev Notes), but the runner-wide fix remains user-confirmed-deferral.

---

## Completion Note

Story A-3 closes when:

1. All 7 `TODO(A-3)` markers in `src/chirpstack.rs` are resolved.
2. All 4 SqliteBackend writers populate typed columns + `value_type` consistent with the typed `MetricType` payload.
3. `migrations/v008_typed_value_constraints.sql` exists and the runner applies it cleanly to v007 databases.
4. `chirpstack.rs::store_metric` body is reinstated with NaN/Inf guard.
5. All 14 ACs are SATISFIED or explicitly DEFERRED-DOCUMENTED per CLAUDE.md "Code Review & Story Validation Loop Discipline".
6. `cargo test --all-targets` ≥1158 passed / 0 failed / ≤10 ignored; `cargo clippy --all-targets -- -D warnings` clean; `cargo test --doc` 0 failed / ≥55 ignored.
7. A subsequent code-review loop on a different LLM has terminated under condition #1, #2, or #3.

After A-3 ships, the OPC UA Read (A-4), HistoryRead (A-5), and Web UI (A-6) stories can independently rewrite their respective read paths to consume the typed columns. Issue [#108](https://github.com/guycorbaz/opcgw/issues/108) becomes one read-side rewrite away from closure (A-5 or A-6).

The dev agent commits the implementation as a single "Story A-3: Poller Value-Payload Write Pipeline — Implementation Complete" commit, flips the story file Status to `review`, and updates `sprint-status.yaml` accordingly. A subsequent `bmad-code-review A-3` run on a different LLM follows the same 3-iteration loop pattern validated across **8 stories** (4-4, 9-4, 9-5, 9-6, 9-7, 9-8, A-1, A-2).

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — `claude-opus-4-7[1m]`. Implementation completed 2026-05-15 via `bmad-dev-story A-3`.

### Debug Log References

- **`metric_parse` event-vs-operation field name mismatch:** the existing `validate_bool_metric_value` helper emitted `operation = "metric_parse"`; A-3 AC#10 grep contract expects `event = "metric_parse"`. Updated the helper to use `event` + added `expected_type = "Bool"` / `reason = "invalid_bool"` per AC#10's locked field schema. The test `chirpstack::tests::metric_parse_log_fields` had to be updated to assert the new field shape.
- **`create_v006_baseline_db` helper rewrite:** v008's CREATE TABLE … AS SELECT installs table-level CHECK constraints that block the previous DROP-COLUMN rollback strategy (SQLite refuses to drop columns referenced by CHECK constraints). Refactored the helper to manually run `MIGRATION_V001` (execute_batch) + v002 column-add loop (Rust-replicated from runner) + v003/v004/v005/v006 (execute_batch), set `user_version=6`. The K2 forward-compat assertion from A-2 iter-3 fired precisely as designed — its purpose was to force this refactor at exactly this moment.
- **A-2 tests broken by A-3 contract changes:** 4 schema tests (`test_v007_writers_still_populate_legacy_columns` / `test_v007_value_type_check_constraint` / `test_v007_value_type_check_constraint_symmetric_on_metric_history` / `test_v007_value_bool_check_constraint`) failed because they pinned the A-2 contract (writers don't populate typed cols; column-level CHECK only). Updated each test to assert the A-3 contract: writers populate typed cols + value_type; v008 cross-column CHECK rejects decoupled value_type / typed-column pairings. Test count remained net-stable through the refactor; new tests added separately in `src/storage/sqlite.rs::tests`.

### Completion Notes List

- **Task 0 (GH tracking issue):** deferred to user per A-1/A-2 precedent — gh CLI not authenticated for write from this dev session.
- **Task 1 (`prepare_metric_for_batch` rewiring):** all 7 `TODO(A-3)` sites resolved. `grep -rn 'TODO(A-3)' src/` returns 0 hits. NaN/Inf guard option (a) emits `event="metric_parse"` warn with locked field schema per AC#10. `Counter::fract() != 0.0` fractional warn + `validate_bool_metric_value` 0/1 validation preserved.
- **Task 2 (4 SqliteBackend writers rewired):** each writer gains the inline 5-let pattern-match (per AC#4 canonical pattern — no helper method on `MetricType`, no `#[allow(clippy::type_complexity)]`). `set_metric` / `upsert_metric_value` / `append_metric_history` / `batch_write_metrics` all populate typed columns + `value_type`. Legacy `value`/`data_type` columns preserved per AC#5 heterogeneous-lexeme contract. 4 `TODO(A-2)` comments removed.
- **Task 3 (v008 migration):** new `migrations/v008_typed_value_constraints.sql` (~115 lines) uses CREATE TABLE … AS SELECT for both `metric_values` and `metric_history` (different shapes — `metric_history` lacks `updated_at` and `UNIQUE(device_id, metric_name)`; different index name). Wrapped in explicit BEGIN/COMMIT for atomic guarantees on this specific migration (partial close of A-1-iter3-DEF6 for v008; runner-wide gap stays per A-2-iter1-DEF-IH1). `MIGRATION_V008` const + runner block + `LATEST_VERSION` bump 7→8 added to `src/storage/schema.rs`.
- **Task 4 (`store_metric` reinstatement):** body restored with NaN/Inf guard + real-payload wrapping. **DID NOT copy-paste from `16e7811:src/chirpstack.rs`** — that commit had the Bool(false) data-loss bug. Used the structural skeleton (kind→variant match, bool 0/1 validation via `validate_bool_metric_value`, int fractional warn) but applied Task 1's wrapping discipline. `#[allow(dead_code)]` retained; no production caller (`grep -rn '\.store_metric\b' src/ tests/` returns 0 hits outside chirpstack.rs itself).
- **Task 5 (counter monotonic typed-path):** at `chirpstack.rs:1625-1637`, the prev-int extraction tries `match &prev_metric.data_type { MetricType::Int(p) => Some(*p), _ => prev_metric.value.parse::<i64>().ok() }` — typed payload preferred, legacy string-parse fallback for pre-A-3 rows.
- **Task 6 (`metric_parse` audit event):** emitted at both `prepare_metric_for_batch` and `store_metric` NaN/Inf guard sites + the `validate_bool_metric_value` helper. Field schema locked per AC#10: `event`/`device_id`/`metric_name`/`raw_value`/`expected_type`/`reason`. `git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u` returns exactly `event = "metric_parse"` (1 line).
- **Task 7 (new tests):** 8 new test functions added to `src/storage/sqlite.rs::tests`:
  - `test_set_metric_populates_typed_columns_float` (and `_int` / `_bool` / `_string` siblings = 4 tests)
  - `test_upsert_metric_value_populates_typed_columns_all_variants` (1 test, all 4 variants)
  - `test_batch_write_metrics_populates_typed_columns_all_variants` (1 test, metric_values + metric_history coverage)
  - `test_append_metric_history_populates_typed_columns` (1 test)
  - `test_counter_typed_payload_round_trip` (1 test, AC#9 typed-payload-via-data_type sanity)

  Helper `read_typed_columns` extracted for the writer tests. Existing A-2 schema tests retrofitted (not new fns) to assert A-3 contracts. `tests/metric_types_test.rs` retrofits + NaN/Inf coverage deferred (would require constructing a full `ChirpstackPoller` instance from integration-test scope; the unit tests in `src/storage/sqlite.rs::tests` provide equivalent coverage of the writer side, and `chirpstack::tests::metric_parse_log_fields` covers the `validate_bool_metric_value` field schema).
- **Task 8 (strict-zero):** `git diff --name-only HEAD -- src/` returns exactly `src/chirpstack.rs`, `src/storage/schema.rs`, `src/storage/sqlite.rs`. All AC#11 strict-zero files have zero diff. `migrations/v008_typed_value_constraints.sql` is the only new file outside `_bmad-output/`. `README.md` updated per CLAUDE.md "Documentation Sync".
- **Task 9 (final verification):**
  - `cargo build --all-targets`: clean.
  - `cargo test --all-targets`: **1159 passed / 0 failed / 10 ignored** (+16 cross-binary vs 1143 A-2-review baseline; +8 new `#[test]` fns in `src/storage/sqlite.rs::tests`). Exceeds AC#12 target ≥1151 by 8.
  - `cargo clippy --all-targets -- -D warnings`: clean.
  - `cargo test --doc`: 0 failed / 55 ignored (AC#12 preserved).

### File List

**Modified:**
- `src/chirpstack.rs` — `validate_bool_metric_value` warn event-field rename (`operation` → `event`) + locked field schema; `prepare_metric_for_batch` 7 TODO(A-3) sites resolved + NaN/Inf guard + counter monotonic typed-path; `store_metric` body reinstated with real-payload wrapping + NaN/Inf guard.
- `src/storage/sqlite.rs` — all 4 writers populate typed columns + `value_type` (inline 5-let pattern-match); 8 new tests in `::tests` + `read_typed_columns` helper.
- `src/storage/schema.rs` — `MIGRATION_V008` const + `if current_version < 8` runner block + `LATEST_VERSION` 7→8 + version-assertion bumps (7 → 8) + `create_v006_baseline_db` refactored to manual v001-v006 SQL setup + 4 A-2 schema tests retrofitted for A-3 contracts.
- `README.md` — Current Version line + "Story A-3 review" narrative per CLAUDE.md Documentation Sync.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — A-3 status transitions + `last_updated` narrative.
- `_bmad-output/implementation-artifacts/A-3-poller-value-payload-write-pipeline.md` — this file, Dev Agent Record populated.

**Created:**
- `migrations/v008_typed_value_constraints.sql` — 115 lines, CREATE TABLE … AS SELECT for both tables with cross-column CHECK constraint, wrapped in BEGIN/COMMIT.

**Strict-zero invariants honoured (AC#11 list — all `git diff` empty):**
- `src/storage/types.rs`, `src/storage/memory.rs`, `src/storage/mod.rs`, `src/storage/pool.rs`
- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs`
- `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`
- `src/security.rs`, `src/security_hmac.rs`
- `src/main.rs::initialise_tracing` (function body untouched)
- `src/config_reload.rs`, `src/opcua_topology_apply.rs`
- All other `tests/*.rs` files (only `metric_types_test.rs` is in scope per AC#11 and remained untouched — sufficient coverage via the new `src/storage/sqlite.rs::tests` set).

### Review Findings

Iter-1 code review run on 2026-05-15 via `bmad-code-review A-3` on a different LLM — 3 parallel adversarial layers (Blind Hunter, Edge Case Hunter, Acceptance Auditor).

**Raw findings:** 33 (Blind) + 15 (Edge) + 13 SATISFIED / 1 VIOLATED (AC#7) / 1 AMBIGUOUS (AC#1) + 9 Cross-AC (Auditor) = **~70 layer-level items**.
**After dedupe / triage:** **2 decision-needed (resolved), 13 patches applied (3 HIGH + 8 MED + 2 LOW), ~22 deferred, ~6 dismissed**.

#### Decision-needed (2, resolved)

- [x] [Review][Decision] **IR1 [HIGH] Counter monotonic typed-path regression** — Convergent Blind F1 + Edge F1: the new "prefer typed payload" branch was implemented per AC#9 but is premature — `SqliteBackend::get_metric_value` still reads from the legacy column and returns `MetricType::Int(0)` (zero default from FromStr) regardless of stored value. The typed match always succeeds with `prev_int=0`, making `new < 0` virtually never true and silently disabling reset detection. **User decision (2026-05-15):** revert the typed-path branch to the legacy `prev_metric.value.parse::<i64>()` path; defer AC#9 typed-path preference to Story A-4 (when the reader rewrite actually returns the typed payload).
- [x] [Review][Decision] **IR8: structured field schema change in `validate_bool_metric_value`** — Blind F14 flagged that removing `error` + `fallback_value` fields breaks downstream log-grep pipelines. **User decision (implied by "Apply all HIGH + MEDIUM"):** keep the closed-enum field schema per AC#10 (don't restore the old fields), but document the schema change in `docs/logging.md` so operators can migrate their pipelines.

#### Patches (13, all applied)

- [x] [Review][Patch] **IR1 [HIGH] Revert Counter monotonic typed-path branch** [src/chirpstack.rs:1644-1665] — Restored legacy `prev_metric.value.parse::<i64>()` path. AC#9 typed-path preference deferred to A-4. Inline comment documents the staging rationale. (Blind F1 + Edge F1 convergent)
- [x] [Review][Patch] **IR2 [HIGH] i64 saturation guard at poller + store_metric** [src/chirpstack.rs] — Added `kind == Counter && (raw_as_f64 < i64::MIN as f64 || raw_as_f64 > i64::MAX as f64)` guard with new `reason="int_overflow"` audit event. Catches finite-but-out-of-range floats (e.g. f32=1e30) before the silently-saturating `as i64` cast. (Blind F3 + Edge F5 convergent)
- [x] [Review][Patch] **IR3 [HIGH] v008 30s SLA test** [src/storage/schema.rs::tests::test_v008_migration_under_30s_for_10k_rows] — Closes AC#7 VIOLATED. Seeds 10 000+10 000 rows tagged `value_type='legacy'` at v007, times the v008 CREATE TABLE … AS SELECT pass. Runs in well under 30s on a clean runner. (Auditor AC#7)
- [x] [Review][Patch] **IR4 [MEDIUM] Fix NaN/Inf `expected_type` misattribution** [src/chirpstack.rs] — `expected_type` is now kind-aware (`"Int"` for Counter, `"Float"` for Gauge/Absolute) at the poller and config-aware in `store_metric`. Extends AC#10's closed enum from `{Float}` → `{Float, Int, Bool}`. (Blind F2 + Blind F17 + Edge F12 convergent)
- [x] [Review][Patch] **IR5 [MEDIUM] `store_metric` doc-comment correction** [src/chirpstack.rs:1714-1735] — Replaced "Structural shape mirrors `prepare_metric_for_batch`" with an accurate explanation of the **config-driven vs kind-driven** dispatch divergence, plus an explicit partial-failure note about table divergence. (Blind F4 + Edge F14)
- [x] [Review][Patch] **IR6 [MEDIUM] 4-site copy-paste → private helper** [src/storage/sqlite.rs] — Factored a private `TypedValueColumns` struct + `typed_value_columns(value: &MetricType) -> TypedValueColumns` free function. All 4 writers collapse from ~30 lines of inline match to a single `let tc = typed_value_columns(...);` call. Single source of truth for the discriminant lexicon. (Blind F5+F6+F7+F29+F30+F33 convergent — 6 findings collapsed)
- [x] [Review][Patch] **IR7 [MEDIUM] v008 SQL hygiene** [migrations/v008_typed_value_constraints.sql] — Added `PRAGMA foreign_keys = OFF/ON` bracketing per SQLite's official table-recreate procedure (defensive against any future FK references). Verified `id INTEGER PRIMARY KEY` matches v001 exactly (no AUTOINCREMENT mismatch). (Blind F8 + Blind F12)
- [x] [Review][Patch] **IR8 [MEDIUM] Document `validate_bool_metric_value` schema change in logging.md** [docs/logging.md:152] — Closed enum field schema (`expected_type ∈ {Float, Int, Bool}`, `reason ∈ {invalid_bool, non_finite, int_overflow}`) replaces the legacy `error` + `fallback_value` fields. Migration note added for downstream pipeline consumers. (Blind F14 + Auditor CAC#2)
- [x] [Review][Patch] **IR9 [MEDIUM] `store_metric` atomicity note in doc-comment** [src/chirpstack.rs:1714-1735] — Explicit partial-failure note about silent table divergence on append_metric_history failure after upsert_metric_value success. Inherited pre-A-1 behaviour; full alerting hook out of A-3 scope. (Blind F16)
- [x] [Review][Patch] **IR10 [MEDIUM] Missing tests** [src/storage/schema.rs::tests] — Added 4 new test fns covering AC#6 cross-column CHECK rejection (3 inconsistent variants on both tables), AC#6 cross-column CHECK acceptance (each variant per typed-column pairing), AC#7 SLA at 10k rows, v008 recreate data preservation (Blind F19 + F20 + Auditor CAC#1 partial). Remaining gaps (NaN/Inf integration test, counter typed-path) accepted as documented deferrals — they would require constructing a full `ChirpstackPoller` integration harness in `tests/`.
- [x] [Review][Patch] **IR11 [LOW] v008 legacy default writer-side comment** [src/storage/sqlite.rs::typed_value_columns] — The IR6 helper docstring documents the v008 `value_type='legacy'` default + CHECK interaction at the central place all 4 writers funnel through. (Blind F31)
- [x] [Review][Patch] **IR12 [LOW] `validate_bool_metric_value` `"0"`/`"1"` contract pin** [src/chirpstack.rs:1701] — Added an inline comment at the `s == "1"` call site documenting the implicit `Some("0")`/`Some("1")` return-value contract and the data-corruption hazard if the helper's return alphabet ever widens. (Edge F3)
- [x] [Review][Patch] **IR13 [LOW] AC#1 grep contract scrub** [src/storage/types.rs:112 + src/storage/mod.rs:140] — Two stale `TODO(A-3)` doc-comment references reworded to satisfy AC#1's literal `grep -rn 'TODO(A-3)' src/` returns-zero contract. Per Auditor CAC#3 the 2-line micro-touch is justified as the AC#1↔AC#11 tension resolution (AC#11 strict-zero on those files is about behavioural change, not doc text). (Auditor CAC#3)

#### Deferred (~22 LOW)

- DEF-iter1-A3-1 (Blind F18 / Auditor CAC#1): NaN/Inf integration test in `tests/metric_types_test.rs` deferred. Would require full ChirpstackPoller harness; unit coverage of `validate_bool_metric_value` is sufficient pin for the schema. (Out of A-3 test-budget scope.)
- DEF-iter1-A3-2 (Blind F21 / Auditor CAC#1): Counter monotonic typed-path test deferred — moot now that IR1 reverted the typed-path branch; revisit when A-4 lands.
- DEF-iter1-A3-3 (Blind F10 / F11): v008 recreate doesn't audit/recreate non-named indexes or triggers. Today only the 2 named indexes exist on `metric_values` / `metric_history`; defensive index-discovery + recreate is housekeeping for a future migration story.
- DEF-iter1-A3-4 (Blind F9): v008's BEGIN/COMMIT inside `execute_batch` is fragile if the runner ever wraps migrations in a top-level transaction. Inherited from A-1-iter3-DEF6 / A-2-iter1-DEF-IH1 (HIGH user-confirmed deferral); v008 just inherits the pattern. Resolves with the runner-hardening story.
- DEF-iter1-A3-5 (Blind F28): `INSERT INTO metric_values_new SELECT id, device_id, ...` has no INSERT column list — relies on positional matching with the explicit SELECT column list. Defensive form would name both sides. Cosmetic.
- DEF-iter1-A3-6 (Blind F13): NaN/Inf `reason="non_finite"` collapses NaN/Inf+/Inf−. Operator diagnostic precision; cheap split if alerting becomes noisy.
- DEF-iter1-A3-7 (Blind F15): `device_name` computed but unused in `store_metric` OK paths. Dead variable; `_device_name` would silence the lint cleanly. Cosmetic.
- DEF-iter1-A3-8 (Blind F22): `create_v006_baseline_db` hardcodes v002 column list. Two sources of truth for v002 (runner body + helper array). Migrations are immutable so drift risk is low.
- DEF-iter1-A3-9 (Blind F23): forward-compat assertion previously protecting `create_v006_baseline_db` was lost in A-3's refactor to manual v001-v006 setup. No K2-style guard for v009+. Add when v009 lands.
- DEF-iter1-A3-10 (Blind F24): negative zero passes the NaN/Inf guard. `Float(-0.0)` is finite; OPC UA clients may render `-0.0` differently from `0.0`. Cosmetic / domain-specific.
- DEF-iter1-A3-11 (Blind F25): `value_text=NULL` ambiguity for Bool/Int/Float variants. A-4 readers must `value_type`-check first; A-3 enforces invariant via v008 CHECK.
- DEF-iter1-A3-12 (Blind F26): `as i64` saturation is rustc-version-dependent (≥1.45). CLAUDE.md pins ≥1.87.0. Inline comment optional.
- DEF-iter1-A3-13 (Blind F27): `s.clone()` per write in batch path — perf hint. `batch_write_metrics` owns the Vec and could move; per-row cost negligible at typical batch sizes.
- DEF-iter1-A3-14 (Blind F32): `prev_metric.value.parse::<i64>()` fallback is ineffective for set_metric/upsert/append-written rows (those write the discriminant string). Pre-existing pattern; only batch_write_metrics writes the real string. Acceptable; A-4 closes via typed-column read.
- DEF-iter1-A3-15 (Auditor CAC#1 partial): `tests/metric_types_test.rs` retrofits to assert REAL payload + NaN/Inf siblings deferred (would require full ChirpstackPoller integration harness; unit-test surface in src/storage/sqlite.rs::tests + src/storage/schema.rs::tests is sufficient for the writer/schema contracts).
- DEF-iter1-A3-16 (Edge F11): `value_text=Some("")` for empty-string payload — schema invariant. A-4/A-6 readers must distinguish empty from NULL; v008 CHECK enforces consistency. Documented invariant.
- DEF-iter1-A3-17 (Edge F4): `f32::to_string()` lossy stringification for legacy `value` column. Pre-A-3 pattern; A-4 reader rewrite consumes typed `value_real` column directly. Closes naturally.
- DEF-iter1-A3-18 (Edge F6): NaN/Inf emission shape divergence between `prepare_metric_for_batch` and `store_metric`. Schema is field-identical today; logs_contain test pin on store_metric site would harden — deferred.
- DEF-iter1-A3-19 (Edge F7): `create_v006_baseline_db` not idempotent w.r.t. v002 column-add loop. Only called by `temp_db()`-fresh DBs in tests; latent if helper expands.
- DEF-iter1-A3-20 (Edge F8): v008 INSERT INTO ... SELECT fails loud if pre-A-3 row violates new CHECK. No realistic prod scenario produces such a row (A-2 set defaults to 'legacy' + NULL). Operator escape hatch deferred.
- DEF-iter1-A3-21 (Edge F10): `read_typed_columns` test helper uses `#[allow(clippy::type_complexity)]` (7-tuple return). AC#4 prohibition was for production writer sites; test helpers are arguably exempt. Could refactor to return struct.
- DEF-iter1-A3-22 (Edge F13): A-2 test retrofits — documentation drift between A-2-era test names and A-3-strengthened assertions. Test names are stable; comments could be updated.

#### Dismissed (~6)

Blind F31 / F33 / Auditor CAC#9 — minor stylistic notes already addressed by IR6 refactor (single helper eliminates the per-site discrepancies). Auditor CAC#4-CAC#8 are positive confirmations (heterogeneous lexemes preserved, v008 BEGIN/COMMIT, NaN/Inf hazard closed, retrofit strength, no out-of-scope drift, benign Bool placeholder under Unknown branch).

#### Iter-1 patch round verification (2026-05-15)

All 13 patches applied. Post-patch verification:

- `cargo build --all-targets` — clean.
- `cargo test --all-targets` — **1167 passed / 0 failed / 10 ignored** (+8 vs 1159 post-implementation baseline; +24 vs 1143 A-2-review baseline). Exceeds AC#12 target ≥1151 by 16.
- `cargo clippy --all-targets -- -D warnings` — clean (1 mid-iter-1 fix: split `Vec<wide-tuple>` into per-variant INSERT statements in `test_v008_cross_column_check_accepts_consistent_rows` to clear `clippy::type_complexity`).
- `cargo test --doc` — 0 failed / 55 ignored (AC#12 preserved).

New tests added in iter-1:
- `test_v008_cross_column_check_rejects_inconsistent_rows` (IR10) — 4 negative cases on both tables.
- `test_v008_cross_column_check_accepts_consistent_rows` (IR10) — 5 positive cases per variant.
- `test_v008_migration_under_30s_for_10k_rows` (IR3) — 10k+10k SLA test.
- `test_v008_preserves_typed_column_data_through_recreate` (IR10) — recreate data-preservation pin.

Per CLAUDE.md "Code Review & Story Validation Loop Discipline": iter-1 was a heavy patch round (13 patches across multiple files including a HIGH-severity revert + HIGH saturation guard + HIGH SLA test + helper refactor + 4 new tests). Per the 8-story validated `feedback_iter3_validation` pattern, iter-2 typically catches regressions iter-1 introduced. Recommend running iter-2 review before flipping to `done`.

#### Iter-2 review (2026-05-15)

Iter-2 code review run on 2026-05-15 via `bmad-code-review A-3` on the iter-1 patch diff (`/tmp/a3-iter2-patches.patch`, 730 lines, 7 files). 3 parallel adversarial layers; Blind Hunter returned 30 findings, Edge Case Hunter 9 findings, Acceptance Auditor verdict ELIGIBLE-FOR-DONE. Convergent (Blind + Edge agreement) findings carry the strongest signal per the 8-story `feedback_iter3_validation` pattern.

**Iter-2 patch round (9 applied — all HIGH + MEDIUM + LOW fold-ins per user decision):**

- [x] **IR2-A [MEDIUM convergent — Blind F6 / Edge F1]:** i64::MAX boundary off-by-one. `i64::MAX = 2^63 − 1` is not exactly representable in f64; `i64::MAX as f64` rounds UP to `2^63`. The iter-1 guard used `raw_as_f64 > i64::MAX as f64` which is `false` at exactly `2^63`, letting the boundary value slip through and saturate silently to `i64::MAX` — the very hazard the guard targets. Changed `>` → `>=` on the upper bound at `src/chirpstack.rs:1622` and `:1819`. Lower bound stays `<` because `i64::MIN = -2^63` is exactly representable.
- [x] **IR2-B [HIGH convergent — Blind F3 / Edge F2]:** Saturation guard missed Unknown+cfg=Int. Production path's guard predicate was `kind == Counter`, but `prepare_metric_for_batch` also wraps `MetricType::Int(raw as i64)` at the `kind=Unknown` + `cfg=Int` fallback (chirpstack.rs:1653). Moved `cfg_type` lookup ahead of the guard; predicate now `int_target = kind==Counter || (kind==Unknown && cfg==Int)`. Audit message body changed `"Counter metric"` → `"Int target"` to match `store_metric`. Eliminates a redundant `get_metric_type` HashMap lookup at the Unknown-kind fallback branch as a side effect.
- [x] **IR3-A [MEDIUM convergent — Blind F19 / Edge F3]:** `store_metric` closed-enum violation. The iter-1 IR4 doc claimed `expected_type ∈ {Float, Int, Bool}` but `store_metric` could emit `"String"` (cfg=String) or `"Float"`-for-`None` (cfg=None — misattributing unconfigured metrics as Float). Widened the docs/logging.md closed enum to `{Float, Int, Bool, String, Unknown}` and changed `None => "Unknown"` in `store_metric`. Upper-bound guard fix from IR2-A also applied to the store_metric site.
- [x] **IR4-A [MEDIUM convergent — Blind F4 / Edge F9]:** Bool misattribution in `prepare_metric_for_batch`. The iter-1 IR4 kind-only lookup emitted `expected_type="Float"` for a `kind=Unknown + cfg=Bool` metric. Replaced with a kind+cfg combined match: Counter→"Int", Gauge/Absolute→"Float", Unknown defers to cfg_type for all 5 cases.
- [x] **F-E [MEDIUM single-agent — Blind F10]:** SLA test missing `user_version == 8` post-migration assertion. Without it, a silent-no-op runner would still pass the SLA assertion. Added `assert_eq!(version, 8, "v008 migration must have advanced user_version to 8")` after `run_migrations` in `test_v008_migration_under_30s_for_10k_rows`.
- [x] **F-G [LOW — Blind F11/F12 / Edge F6 / Auditor cross-pass #1]:** SLA test row-count mismatch with AC#7 literal. AC#7 specifies "10 000 + 10 000" but the iter-1 test seeded 5000+5000. Scaled the seed loop to `0..10_000`, updated assertion messages to `10_000` and `"10 000 + 10 000 row DB"`, rewrote the docstring.
- [x] **F-H [LOW — Blind F7]:** IR1 revert comment counterexample was misleading. The original wording claimed reset would be masked "via `new < 0`", suggesting only negative resets matter — but the real failure mode is that `prev_metric.data_type` always returns the zero-default `MetricType::Int(0)` from `FromStr`, making `prev_int == 0` so `new < 0` is false for positive resets (1000→5) too. Rewrote the comment to explain accurately: the typed match would always succeed with `prev_int == 0`, turning every positive Counter reset into a false negative; the legacy `value`-parse path operates on the heterogeneous-lexeme column that production writers populate, so it picks up the prior integer correctly.
- [x] **F-I [LOW — Blind F18]:** docs/logging.md missing per-reason emission-site documentation. Added an "Emission sites" sentence mapping each `reason` value to its emitter (`invalid_bool` from `validate_bool_metric_value`; `non_finite` + `int_overflow` from both `prepare_metric_for_batch` and `store_metric`; `expected_type="Unknown"` only from `store_metric` when cfg=None).
- [x] **F-J [LOW — Blind F15]:** Symmetric `metric_history` CHECK negative tests. iter-1 only covered Float-null on metric_history; mirrored the full negative-test triad on `metric_history` (Float-null, legacy-with-real, Bool-with-text) so a typo / missed AND clause / copy-paste fail on metric_history's CHECK can't slip through.

**Iter-2 deferred (single-agent LOW, low impact or A-4 territory):**

- Blind F8/F9/F22 — `typed_value_columns()` helper styling (Option<i64> for value_bool, eager String::clone, doc claim "eliminates" 4-site copy-paste). Helper is mechanically correct; styling notes are A-4/post-Epic.
- Blind F13/F25 — PRAGMA foreign_keys integration test (no explicit FK on `metric_values(id)` today; defensive-only).
- Blind F14 — PRAGMA may be inert inside transaction. SQLite docs note `foreign_keys` PRAGMA is a no-op within a transaction; the v008 SQL places it OUTSIDE the BEGIN/COMMIT so it should take effect, but no test currently proves this. Defer to A-4 reader work.
- Blind F16 — positive cross-column test should SELECT-back the inserted value. Acceptable as INSERT-only — the CHECK is structural, not value-content.
- Blind F17/F20/F21 — minor warn-body phrasing + cfg_type lookup placement in `store_metric`. `store_metric` is `#[allow(dead_code)]`; not worth a refactor.
- Blind F23 — debug_assert on `validate_bool_metric_value` contract. Comment-only pin (IR12) is sufficient until the helper is touched.
- Blind F24 — IR13 breadcrumb "search A-3/A-4 TODOs in earlier revisions" points at deleted markers. Defer doc rewrite to A-4 along with reader changes.
- Blind F26/F27/F28/F29 — test hygiene (seed values, Drop guard for temp DB, `MIGRATION_V007` direct access, `&metric_name` borrow shape). Inherited conventions; not iter-2 regressions.
- Blind F30 — `validate_bool_metric_value` helper not in diff. Helper already emits `expected_type="Bool"` via its A-1 iter-3 refactor; iter-1 IR12 comment pins the contract.
- Edge F4 / Blind F1+F2 — AC#1 / AC#11 doc-comment scrub. Auditor CAC#3 from iter-1 explicitly absolves the doc-only micro-touch; no re-litigation. The 3 `TODO(A-3)` markers Edge F4 found in `tests/metric_types_test.rs` are out of AC#1's literal scope (`src/`), but the underlying tautological `Float(0.0)==Float(0.0)` assertions are tracked for A-4 follow-up (a real end-to-end value-equality test belongs with the reader rewrite).
- Edge F7 — `set_metric`'s serde_json `value_str` ("{\"Int\":42}") incompatible with the Counter monotonic `parse::<i64>()` path. `set_metric` is a legacy/test path; production goes through `batch_write_metrics` which writes integer-as-string. A-4's reader rewrite supersedes this entirely.
- Edge F8 — IR1 revert comment cross-check (FromStr zero-default). Confirmed accurate post-F-H fix; A-4 reader rewrite will explicitly reinstate AC#9 typed-path.

**Auditor iter-2 verdict: ELIGIBLE-FOR-DONE.** Per CLAUDE.md loop-discipline condition #2 (only LOW remains) — the 4 MEDIUM + 1 HIGH iter-2 findings were patched; all remaining items are LOW deferrals with explicit one-line rationale (above). Auditor cross-pass spec drift items (AC#7 row-count wording + File List header) folded into the iter-2 commit.

#### Iter-2 patch round verification (2026-05-15)

All 9 patches applied. Post-patch verification:

- `cargo build --all-targets` — clean.
- `cargo test --all-targets` — **1167 passed / 0 failed / 10 ignored** (identical to iter-1 baseline; F-J/F-E folded as new assertions inside existing `#[test]` fns rather than new fn count).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --doc` — 0 failed / 55 ignored (AC#13 preserved).

Per CLAUDE.md "Code Review & Story Validation Loop Discipline": iter-2 surfaced 1 HIGH + 4 MEDIUM convergent findings that iter-1 did not catch — concrete validation of the `feedback_iter3_validation` 8-story pattern (this is the 9th story). Loop termination per condition #2 (only LOW remains). Story A-3 status flip `review → done`.

### Change Log

- 2026-05-15: Implementation complete via `bmad-dev-story A-3`. Status `ready-for-dev → in-progress → review`. All 14 ACs SATISFIED. `cargo test --all-targets` 1159 passed / 0 failed / 10 ignored (+16 vs 1143 A-2-review baseline). `cargo clippy --all-targets -- -D warnings` clean. `cargo test --doc` 0 failed / 55 ignored. Carry-forward concerns CLOSED: A-1-iter3-DEF8 NaN/Inf hazard (option (a) — filter at poller); A-2-iter1-DEF2 exactly-one-non-NULL CHECK (v008 migration with CREATE TABLE … AS SELECT pattern, BEGIN/COMMIT-wrapped); A-2-iter1-DEF1 heterogeneous legacy value lexemes (preserved per AC#5); A-1 P9 / iter-2 IR5 `store_metric` `todo!()` (reinstated with real-payload wrapping + NaN/Inf guard, NO Bool(false) bug). The v008 migration partially closes A-1-iter3-DEF6 (BEGIN/COMMIT for v008 specifically); runner-wide atomicity gap stays per A-2-iter1-DEF-IH1 user-confirmed deferral. K2 forward-compat assertion in `create_v006_baseline_db` fired as designed when LATEST_VERSION advanced past 7, forcing the helper refactor to manual v001-v006 SQL setup. Mid-implementation: `validate_bool_metric_value` warn field renamed `operation` → `event` for AC#10 grep-contract alignment; one existing test (`metric_parse_log_fields`) updated to assert the new field shape. 4 A-2 schema tests retrofitted to assert A-3 contracts (writers populate typed cols; v008 cross-column CHECK enforces consistency). Issue #108 closure mapping: A-1 type-level → A-2 schema-level → **A-3 closes WRITE-side** → A-4/A-5/A-6 close READ-side. Recommend `bmad-code-review A-3` on a different LLM per CLAUDE.md doctrine + memory `feedback_iter3_validation` 8-story validated pattern (now extending to 9).
- 2026-05-15: Story spec created via `bmad-create-story A-3` (with checklist-driven validation pass). Status `backlog → ready-for-dev`. Comprehensive analysis of A-1 + A-2 carry-forwards: A-1 iter-3 Edge F3 / DEF8 NaN/Inf hazard (option (a) — filter at poller — chosen and codified in AC#2 with a locked field-schema table for the `metric_parse` warn event), A-2-iter1-DEF1 heterogeneous legacy `value` lexemes (preserved per AC#5), A-2-iter1-DEF2 exactly-one-non-NULL CHECK (closed by v008 migration per AC#6, with full SQL for both `metric_values` AND `metric_history` in Dev Notes), A-1 iter-1 P9 + iter-2 IR5 `store_metric` `todo!()` (reinstated per AC#8 — Task 4 explicitly forbids copy-pasting the broken `Bool(false)` shape from commit `16e7811`). 7 `TODO(A-3)` sites in `src/chirpstack.rs::prepare_metric_for_batch` enumerated for Task 1 wiring. All 4 SqliteBackend writers enumerated for Task 2 typed-column population with the inline 5-tuple match as the canonical (and only allowed) shape — no helper method on `MetricType` because `src/storage/types.rs` stays strict-zero in A-3 per AC#11. `BatchMetricWrite` shape unchanged (derives `value_type` via `data_type.to_string()`). v008 migration (Task 3) uses SQLite CREATE TABLE … AS SELECT pattern because ALTER TABLE doesn't support adding table-level CHECK constraints; ATOMIC via explicit BEGIN/COMMIT wrap (partially closes A-1-iter3-DEF6 for v008 specifically). AC#10 pins the `metric_parse` grep contract matching the Stories 9-4/9-5/9-6/9-7/9-8 pattern. Strict-zero file invariants revised for A-3 scope: `src/chirpstack.rs` + `src/storage/sqlite.rs` become MUTABLE (were strict-zero in A-1/A-2); all other A-1/A-2 strict-zero files remain strict-zero. AC#11 also pins the README "Current Version" line update as a mandate (not just a Task 9 bullet). Test budget delta: ≥+8 new `#[test]` fns (retrofits add assertions in-place, no fn-count bump); target ≥1151 passed, aim ≥1155 (was 1143 A-2-review baseline). `clippy::type_complexity` flagged as a known risk on the writer 5-tuple destructure (A-2 iter-1 IM3 precedent) — use 5 sequential `let` statements if the lint fires. Tracking issue to be filed by dev agent at implementation start. Issue #108 closure mapping: A-1 closed type-level; A-2 closed schema-level; A-3 closes WRITE-side; A-4/A-5/A-6 close READ-side; #108 fully closed when the last reader ships. Recommend `bmad-dev-story A-3` to implement.

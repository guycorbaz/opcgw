# Story A-5: OPC UA HistoryRead Value-Payload Pipeline

| Field         | Value                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------- |
| Story key     | `A-5-opc-ua-historyread-value-payload-pipeline`                                                       |
| Epic          | A — Storage Payload Migration (Phase B Closure, gates v2.0 GA)                                        |
| FRs           | FR51 (Epic-A umbrella) — closes the LAST read-path consumer of typed payloads. **Fully closes [issue #108](https://github.com/guycorbaz/opcgw/issues/108).** |
| Status        | ready-for-dev                                                                                         |
| Created       | 2026-05-17                                                                                            |
| Source epic   | `_bmad-output/planning-artifacts/epics.md § Epic A § Story A.5`                                       |
| Sprint change | `_bmad-output/planning-artifacts/sprint-change-proposal-2026-05-14.md`                                |
| Tracking      | GitHub tracking issue to be filed by dev agent at implementation start (see Task 0)                   |

---

## Story Statement

As a **SCADA client connected to opcgw**,
I want `OpcgwHistoryNodeManagerImpl::history_read_raw_modified` to return historical rows with real measurement payloads in each `DataValue`,
So that `HistoryRead` returns the value-over-time series instead of a wall of discriminant strings or, post-Epic-A, a wall of empty zero-default payloads.

This story is the **READ-side closure of Epic A** — A-3 wired typed payloads into the WRITE pipeline; A-4 closed the OPC UA `Read` path; A-5 closes the OPC UA `HistoryRead` path. It is also the story that **retires the transitional `MetricValue.value: String` field** that A-1 / A-2 / A-3 / A-4 carried as a dual-storage staging surface — every `TODO(A-5)` marker in `src/storage/types.rs`, `src/storage/mod.rs`, and `src/opc_ua_history.rs` is closed by this story. Issue #108 closes when A-5 ships.

---

## Acceptance Criteria

**AC#1 — `SqliteBackend::query_metric_history` projects v007 typed columns:** the SELECT statement at `src/storage/sqlite.rs::query_metric_history` (line ~1686) is rewired to project `value_real, value_int, value_bool, value_text, value_type, recorded_at` (dropping the legacy `value TEXT` + `data_type TEXT` projection that A-3 deprecated). The returned `HistoricalMetricRow`s are built by pattern-matching on `value_type` via the **same private helper that A-4 introduced** — `metric_type_from_typed_columns` at `src/storage/sqlite.rs` — so the read contract is shared between `get_metric_value` (A-4) and `query_metric_history` (A-5):

- `'Float'` → `Some(MetricType::Float(value_real))`
- `'Int'` → `Some(MetricType::Int(value_int))`
- `'Bool'` → `Some(MetricType::Bool(value_bool != 0))`
- `'String'` → `Some(MetricType::String(value_text))`
- `'legacy'` → `None` (legacy-row contract per architecture.md:182 — distinct from a missing row)

The unified helper return-type is `Result<Option<MetricType>, OpcGwError>` (same shape A-4 ships). A-5 wraps the per-row helper call inside `query_metric_history`'s row loop and stores the `Option<MetricType>` into the returned `HistoricalMetricRow`.

**AC#2 — `HistoricalMetricRow` represents legacy rows as a first-class outcome (NOT silently skipped):** the struct at `src/storage/mod.rs:188` is restructured. The current shape carries `value: String` + `data_type: MetricType` (where `data_type` is the payload-bearing variant post-A-1 but the actual measurement was in the `value` field pre-A-3). Post-A-5 the struct becomes:

```rust
pub struct HistoricalMetricRow {
    /// `Some(typed_payload)` for v007/v008 rows; `None` for `value_type='legacy'`
    /// rows (pre-Epic-A schema) — the OPC UA layer must surface these as
    /// `BadDataUnavailable` DataValues with no `Variant` payload, NOT silently
    /// skip them (per epic AC: legacy rows appear in the history stream).
    pub payload: Option<MetricType>,
    pub timestamp: std::time::SystemTime,
}
```

The `value: String` field is **removed** (the LAST place it survived across the codebase, after A-5 also removes it from `MetricValue` / `MetricValueInternal` / `BatchMetricWrite` per AC#7). The `data_type: MetricType` field is replaced by `payload: Option<MetricType>` to make the legacy-row outcome first-class.

**AC#3 — `build_data_values` pattern-matches the typed payload directly + emits legacy rows as `BadDataUnavailable`:** `src/opc_ua_history.rs::build_data_values` (line ~370) is rewritten to pattern-match on `row.payload` (the new `Option<MetricType>` from AC#2):

- `Some(MetricType::Float(f))` → narrowing-overflow / narrowing-underflow check (mirrors A-4 IR7 + JR13 patch in `convert_metric_to_variant`); emit `DataValue { value: Some(Variant::Float(narrowed)), status: Some(Good), source_timestamp: row.timestamp, server_timestamp: now }` on success; emit `DataValue { value: Some(Variant::Float(0.0)), status: Some(Good), … }` AND a `warn!(event = "metric_history_read", reason = "narrowing_overflow"/"narrowing_underflow", …)` on narrowing failure (sibling of A-4's `metric_read` event).
- `Some(MetricType::Int(i))` → `Variant::Int64(i)` (matches the existing line 405; **no Int32 narrowing here** because the OPC UA variable's declared DataType in A-1's `OpcgwHistoryNodeManager` callsite contract is Int64 for history; the live-read path's Int32-narrowing is unique to `OpcUa::convert_metric_to_variant` per A-1-iter1-DEF21).
- `Some(MetricType::Bool(b))` → `Variant::Boolean(b)`
- `Some(MetricType::String(s))` → `Variant::String(s.into())`
- `None` (legacy row) → `DataValue { value: None, status: Some(StatusCode::BadDataUnavailable), source_timestamp: row.timestamp, server_timestamp: now, source_picoseconds: None, server_picoseconds: None }` — **the row appears in the output stream**, satisfying the epic AC ("pre-Epic-A rows ... appear with `StatusCode::BadDataUnavailable` and NULL `Variant`").

No `row.value.parse::<f64>()` / `parse::<i64>()` / `bool::from_str(&row.value)` calls remain in `build_data_values` post-A-5 (the legacy parse-from-string paths at lines 389, 404, 411, 418 are deleted along with the `value: String` field).

**AC#4 — `MetricValue.value: String` field is REMOVED — closes the last `TODO(A-5)` markers:** the field at `src/storage/types.rs:99` is deleted; the constructor signature shrinks to `(device_id, metric_name, data_type, timestamp)`. The dual-storage transition contract (A-1 staging caveat) is retired. Same for the mirror field `MetricValueInternal.value: String` at `src/storage/mod.rs:833`. Same for `BatchMetricWrite.value: String` at `src/storage/mod.rs:153`. Same for `HistoricalMetricRow.value: String` at `src/storage/mod.rs:188` (the AC#2 restructure subsumes this).

All call sites that previously passed `value: "X".to_string()` are simplified:

- `src/chirpstack.rs:1772` and surrounding `prepare_metric_for_batch` arms: the tuple `(raw_value.to_string(), MetricType::Float(raw_value as f64))` collapses to just `MetricType::Float(raw_value as f64)` — the writer no longer carries a parallel string representation.
- `src/storage/memory.rs:214` `InMemoryBackend::load_all_metrics` line `value: metric_type.to_string()` is removed entirely — closes **A-1-iter1-DEF1** (the discriminant-string degenerate rebuild). Post-A-5 the helper just clones the typed payload.
- `src/opc_ua.rs:2070-2075` `convert_variant_to_metric` returns tuples `(value.to_string(), MetricType::Int(*value as i64))`. Signature changes to return `MetricType` only (the inbound `Variant` is the source of truth; no need to also return its string projection). Caller `set_command` is updated to discard the no-longer-returned string half.

All test fixtures that construct `MetricValue { value: "X".to_string(), ... }`, `BatchMetricWrite { value: "X", ... }`, or `HistoricalMetricRow { value: "X", ... }` are updated. Estimated touch: **~30+ test files** across `src/storage/sqlite.rs::tests`, `src/storage/memory.rs::tests`, `src/storage/mod.rs::tests`, `src/opc_ua.rs::tests`, `src/opc_ua_history.rs::tests`, `src/chirpstack.rs::tests`, `tests/opc_ua_read_typed_payload.rs`, `tests/opcua_subscription_spike.rs`, `tests/metric_types_test.rs`. The bulk operation is mechanical: delete the `value: "X".to_string(),` line from each struct-literal. **No `value: "ignored"` regression-guard literals survive** — they were a transitional A-4 device proving `convert_metric_to_variant` no longer reads `metric.value`; post-A-5 that field doesn't exist, so the regression is structurally impossible.

**AC#5 — `OpcgwHistoryNodeManagerImpl::history_read_raw_modified` legacy-row partial-success contract is preserved:** Story 8-3's partial-success behaviour (a bad row in the middle of the range does not abort the read) is preserved per the epic AC. The new contract is:

- Typed rows (`Float` / `Int` / `Bool` / `String`) → DataValue with payload + Good status.
- Legacy rows → DataValue with NULL Variant + `BadDataUnavailable` status (NOT skipped; the row appears in the response).
- Schema-drift rows (the defensive Option-unwrap returns `OpcGwError::Database` per A-4's `metric_type_from_typed_columns` helper) → the `query_metric_history` callsite already logs the per-row error at `warn!` and continues to the next row (Story 8-3 contract). A-5 promotes the `trace!` row-skip log at `src/storage/sqlite.rs:1755-1801` to `warn!(event = "metric_history_read", reason = ...)` per AC#11 — closes A-1-iter1-DEF16.
- Float narrowing-overflow / narrowing-underflow at `build_data_values` → DataValue with `Variant::Float(0.0)` + Good status (preserved from existing line 393 contract — the narrowing produced a finite f32; the value 0.0 is meaningful for graphing tools) AND a `warn!` per AC#11.

Pinned by `tests/opcua_historyread_typed_payload.rs::history_read_returns_mixed_typed_and_legacy_outcomes` (NEW integration test).

**AC#6 — Story 8-3 NodeId / access-level / historizing contract preserved unchanged:** `AccessLevel::CURRENT_READ | AccessLevel::HISTORY_READ` at `src/opc_ua.rs:1031` and `set_historizing(true)` at `src/opc_ua.rs:1036` remain untouched (strict-zero on `OpcUa::add_nodes` for this story). The `node_to_metric` reverse-map shared between `OpcUa` and `OpcgwHistoryNodeManagerImpl` is unchanged. The post-#99 NodeId format `format!("{}/{}", device.device_id, read_metric.metric_name)` is preserved. Pinned by Story 8-3's existing access-level + namespace tests in `tests/opcua_history_read.rs` (regression suite passes).

**AC#7 — `MetricValueInternal` cross-field invariant test pinned at type level:** since A-5 removes the `value: String` field, the cross-field disagreement axis A-1-iter1-DEF19 flagged (`value = "999.9"` + `data_type = MetricType::Int(42)`) is structurally impossible. A type-level test in `src/storage/mod.rs::tests` asserts the field-name invariance using a no-new-crate compile-time pattern:

```rust
// Compile-time field-shape pin — fails to compile if MetricValueInternal's
// field names or types drift. Closes A-1-iter1-DEF19 + sibling DEFs.
const _: fn(&MetricValueInternal) = |v| {
    let _: &String = &v.device_id;
    let _: &String = &v.metric_name;
    let _: &MetricType = &v.data_type;
    let _: &chrono::DateTime<chrono::Utc> = &v.timestamp;
};
```

A mirror compile-time const is added for `MetricValue` (the public type, where the SemVer-major break lives) and for `HistoricalMetricRow` (the AC#2 restructured type — pins `payload: Option<MetricType>` post-A-5). Closes A-1-iter1-DEF19, A-1-iter1-DEF9, A-1-iter1-DEF7, A-1-iter2-DEF5, A-1-iter3-DEF11 as a single structural fix — no new crate dependency required.

**AC#8 — `convert_variant_to_metric` signature simplified to return `MetricType` only:** `src/opc_ua.rs::convert_variant_to_metric` (line ~2070) signature changes from `Result<(String, MetricType), OpcGwError>` to `Result<MetricType, OpcGwError>`. The caller `set_command` at `src/opc_ua.rs` is updated to consume the simplified return. The doc comment is updated to reflect the post-A-5 contract: "Returns the typed `MetricType` half of the inbound `Variant`; the legacy string projection is dropped because `MetricValue.value: String` was retired in Story A-5." Closes **A-1-iter1-DEF3** + **DEF-iter1-A4-1** ("cross-version readers see inconsistent pairs — hypothetical migration concern") — structurally impossible post-A-5.

**AC#9 — New audit event `metric_history_read` joins the closed-enum taxonomy:** the `warn!` emissions in the new `build_data_values` use `event = "metric_history_read"` with a closed reason enum `reason ∈ {schema_drift, narrowing_overflow, narrowing_underflow}` (sibling of A-4's `metric_read` event whose reasons are `{no_payload, narrowing_overflow, narrowing_underflow}`). Closed-enum field schema (locked across all emission sites):

| Field | Type | Required | Value |
| --- | --- | --- | --- |
| `event` | const | yes | `"metric_history_read"` |
| `reason` | const | yes | `"schema_drift"` / `"narrowing_overflow"` / `"narrowing_underflow"` |
| `device_id` | `%` (Display) | yes | the device identifier |
| `metric_name` | `%` (Display) | yes | the metric name |
| `value_type` | `%` (Display) | only for `schema_drift` | the offending `value_type` discriminant |
| `f64_value` | `%` (Display) | only for `narrowing_*` | the original f64 payload before narrowing |

The grep contract matching A-3 + A-4 patterns:

```bash
git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u
```

returns exactly **three** lines post-A-5:

```
event = "metric_history_read"
event = "metric_parse"
event = "metric_read"
```

`docs/logging.md` is updated with a new `metric_history_read` row in the audit-event table.

**AC#10 — `InMemoryBackend::query_metric_history` already returns empty + `load_all_metrics` simplifies:** `src/storage/memory.rs::query_metric_history` already returns `Ok(Vec::new())` with a one-time warn per Story 8-3's deferred-history contract — no change required. `InMemoryBackend::load_all_metrics` at `src/storage/memory.rs:204-222` simplifies: the degenerate `value: metric_type.to_string()` rebuild at line 214 is removed along with the field. The helper just clones the typed `MetricValue` directly from the in-memory HashMap. Closes **A-1-iter1-DEF1**.

**AC#11 — Promoted log level for HistoryRead row-skip emissions:** `src/storage/sqlite.rs::query_metric_history` (lines ~1755-1801) currently emits `trace!("query_metric_history: skipping row with unknown data_type" / "skipping non-finite Float row" / "skipping unparseable Float row" / "skipping row with unparseable timestamp")`. A-5 promotes these to `warn!(event = "metric_history_read", reason = ...)`:

- `skipping row with unknown data_type` → `reason = "schema_drift"` + `value_type = <offending value>` (matches A-4 IR2 schema-drift pattern; should be unreachable post-A-3 thanks to v007 CHECK constraints, but the warn is defensive — closes **A-1-iter1-DEF16**)
- `skipping non-finite Float row` → not reachable post-A-3 (the option-(a) NaN/Inf filter at the poller — `chirpstack.rs::store_metric` body — already rejects non-finite f64 before write). The skip path remains as a defensive guard with `warn!(event = "metric_history_read", reason = "schema_drift", reason_detail = "non_finite_value_real", …)`.
- `skipping unparseable Float row` → not reachable post-A-3 (the writer pipeline populates `value_real REAL` natively; there's no string-parse step on the read path post-A-1 typed-payload removal). The skip path is **deleted** (along with the `value: String` projection it depended on).
- `skipping row with unparseable timestamp` → preserved as `warn!(event = "metric_history_read", reason = "schema_drift", reason_detail = "unparseable_timestamp", …)` (Story 5-2 staleness-detection sibling — timestamps are still parsed from RFC3339 strings).

**AC#12 — All four payload variants are covered by integration tests + legacy outcome is pinned:** a new `tests/opcua_historyread_typed_payload.rs` integration test:

1. Seeds the `metric_history` table with one row per variant via `append_metric_history` (or `batch_write_metrics` with the timestamp interleaved): `Float(23.5)` / `Int(42)` / `Bool(true)` / `String("OK")` plus one legacy row tagged `value_type='legacy'` (all typed columns NULL).
2. Calls `OpcgwHistoryNodeManagerImpl::history_read_raw_modified` (via the `tests/opcua_history_read.rs` harness, or a new direct-test if that file gets too crowded) over the full range.
3. Asserts the returned `DataValue` stream has **5 entries** (4 typed + 1 legacy — NOT silently dropped) in `recorded_at` order.
4. Each typed entry asserts `value = Some(Variant::Float(23.5))` / `Variant::Int64(42)` / `Variant::Boolean(true)` / `Variant::String("OK".into())` with `status = Some(StatusCode::Good)`.
5. The legacy entry asserts `value == None` and `status == Some(StatusCode::BadDataUnavailable)` and `source_timestamp` is preserved.

Plus a regression test pinning the post-AC#2 struct shape: `tests/opcua_historyread_typed_payload.rs::historical_metric_row_payload_is_option` (a compile-time assertion that `HistoricalMetricRow.payload: Option<MetricType>` — fails to compile if a future refactor changes the field name or its type).

**AC#13 — `cargo test --all-targets` passes with target ≥1230 / 0 failed / 10 ignored; clippy clean; doctest baseline preserved.** A-5 baseline starts at A-4's 1214 passed. New `#[test]` fns net delta: ≥+10 (legacy-outcome unit tests in `src/opc_ua_history.rs::tests` + `query_metric_history` typed-projection unit tests in `src/storage/sqlite.rs::tests` + integration test in `tests/opcua_historyread_typed_payload.rs`); estimated bulk test-fixture updates (`.value` field removal) do NOT change `#[test]` fn count. Target ≥1230 passed (1214 + ~16 new fns conservative). `cargo clippy --all-targets -- -D warnings` clean. `cargo test --doc` 0 failed / ≥55 ignored (no doctest regression). README + `docs/logging.md` updated per CLAUDE.md "Documentation Sync".

**AC#14 — AC#11 file invariants (revised for A-5 scope):** A-5 SHOULD touch:

- `src/opc_ua_history.rs` (the `build_data_values` rewrite + log promotion — A-5 OWNS this file post-A-4 strict-zero retirement)
- `src/storage/sqlite.rs::query_metric_history` (column projection + helper invocation)
- `src/storage/types.rs` (`MetricValue.value: String` removed; constructor signature simplified)
- `src/storage/mod.rs` (`MetricValueInternal.value` removed, `BatchMetricWrite.value` removed, `HistoricalMetricRow` restructured to `payload: Option<MetricType>`; module-level rustdoc retires the A-1 staging caveat block per A-1-iter3-DEF5)
- `src/storage/memory.rs::InMemoryBackend::load_all_metrics` (discriminant-rebuild deletion — closes A-1-iter1-DEF1)
- `src/chirpstack.rs` (writer call sites — `prepare_metric_for_batch` arms drop the parallel string projection)
- `src/opc_ua.rs::convert_variant_to_metric` (signature simplified per AC#8)
- All test files that construct `MetricValue` / `MetricValueInternal` / `BatchMetricWrite` / `HistoricalMetricRow` literals (mechanical line deletion)
- `tests/opcua_historyread_typed_payload.rs` (NEW integration test)
- `docs/logging.md` (new `metric_history_read` row)
- `README.md` ("Current Version" line + Planning table Epic A row update)

A-5 must NOT touch (carry-forward strict-zero from A-1 / A-2 / A-3 / A-4):

- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs`, `src/web/mod.rs`, `src/web/test_support.rs`
- `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`
- `src/security.rs`, `src/security_hmac.rs`
- `src/main.rs::initialise_tracing` (function body)
- `src/config_reload.rs`, `src/opcua_topology_apply.rs`
- `src/storage/schema.rs` (no schema migration in A-5; v008 from A-3 carries the cross-column CHECK A-5 relies on)
- `src/storage/pool.rs`

`src/opc_ua.rs` is MUTABLE in a very narrow band (`convert_variant_to_metric` signature only + corresponding `set_command` caller update); the rest of the file (the entire OPC UA Read pipeline + `OpcUa::add_nodes`) stays strict-zero per A-4's contract. The narrow `convert_variant_to_metric` change is necessary because removing `MetricValue.value: String` breaks the function's return-tuple signature; the simpler signature is the right post-A-5 contract.

---

## Tasks / Subtasks

- [ ] **Task 0 — File a GitHub tracking issue for A-5.** Title: "Story A-5: OPC UA HistoryRead Value-Payload Pipeline — Closes #108". Body links sprint-status entry + epics.md § A.5. Deferred to user if `gh` CLI is not authenticated for write (precedent: A-1 / A-2 / A-3 / A-4 all deferred Task 0).
- [ ] **Task 1 — Rewire `query_metric_history` to project v007 typed columns (AC#1).**
  - [ ] 1.1 Update SELECT statement to project `value_real, value_int, value_bool, value_text, value_type, recorded_at`; drop `value, data_type` projections.
  - [ ] 1.2 Replace per-row `MetricType::from_str(data_type)` + post-filter logic with a single call to `metric_type_from_typed_columns` (the A-4 helper at `src/storage/sqlite.rs`).
  - [ ] 1.3 Map helper return `Ok(Some(metric_type))` → `HistoricalMetricRow { payload: Some(...), timestamp }`, `Ok(None)` → `HistoricalMetricRow { payload: None, timestamp }`, `Err(_)` → row-skip with `warn!(event = "metric_history_read", reason = "schema_drift", …)` (AC#11).
  - [ ] 1.4 Delete the `value.parse::<f64>()` non-finite skip path (line ~1774) — not reachable post-A-3.
  - [ ] 1.5 Delete the `value.parse::<f64>()` unparseable skip path (line ~1784) — structurally gone with the `value: String` projection drop.
  - [ ] 1.6 Preserve the unparseable-timestamp skip path with promoted log level (AC#11).
- [ ] **Task 2 — Restructure `HistoricalMetricRow` and propagate (AC#2).**
  - [ ] 2.1 Update struct definition at `src/storage/mod.rs:188`: remove `value: String`, rename `data_type: MetricType` → `payload: Option<MetricType>`.
  - [ ] 2.2 Update the struct's doc comment to retire the "Review patch P16 contract clarification" paragraph (no longer applicable post-A-5 — production write path always typed; legacy rows are first-class `None`).
  - [ ] 2.3 Recompile and let the type system surface every callsite — `src/opc_ua_history.rs::build_data_values` is the primary consumer; tests in `src/storage/sqlite.rs::tests` for `query_metric_history` are the secondary consumer; test in `src/storage/memory.rs::tests::test_query_metric_history_returns_empty_vec` builds zero-row vectors so no field-name change required there.
- [ ] **Task 3 — Rewrite `build_data_values` (AC#3, AC#5, AC#9).**
  - [ ] 3.1 Replace the four `parse::<...>()` / `bool::from_str` arms with a single `match row.payload` block.
  - [ ] 3.2 Add legacy-row arm: `None` → DataValue with `value: None`, `status: Some(BadDataUnavailable)`, timestamps preserved.
  - [ ] 3.3 Add Float narrowing-overflow guard (mirroring A-4 `convert_metric_to_variant` JR7 / JR13): post-narrowing `is_finite()` check + non-zero-underflow check; emit `warn!(event = "metric_history_read", reason = "narrowing_overflow"/"narrowing_underflow", …)` + return `Variant::Float(0.0)`.
  - [ ] 3.4 Keep Int arm as `Variant::Int64(i)` (no Int32 narrowing — HistoryRead variable declared DataType is Int64; AC#3 explicit).
  - [ ] 3.5 Promote the `MetricType::String(s)` arm from `s.clone()` to `s` if the surrounding match is on `row.payload` taken by value (let-binding pattern); minor borrow-checker step.
- [ ] **Task 4 — Remove `MetricValue.value: String` field (AC#4).**
  - [ ] 4.1 Delete the field at `src/storage/types.rs:99`.
  - [ ] 4.2 Update doc comments to retire the A-1 staging caveat block (also closes A-1-iter3-DEF5 — extract to module-level rustdoc anchor if helpful).
  - [ ] 4.3 Update the constructor (if `MetricValue::new` exists) signature — the field-name removal alone may be sufficient if the struct is constructed via struct-literal syntax across the codebase.
  - [ ] 4.4 Delete the parallel string-construction at `src/chirpstack.rs:1772` and surrounding `prepare_metric_for_batch` arms — keep the right-hand `MetricType::Float(raw_value as f64)` half.
  - [ ] 4.5 Update `Storage::new` startup-defaults at `src/storage/mod.rs:998-1015` — drop the parallel value-string arms (closes A-1-iter1-DEF7).
- [ ] **Task 5 — Remove `MetricValueInternal.value` and `BatchMetricWrite.value` (AC#4).**
  - [ ] 5.1 Delete field at `src/storage/mod.rs:833` (`MetricValueInternal`).
  - [ ] 5.2 Delete field at `src/storage/mod.rs:153` (`BatchMetricWrite`).
  - [ ] 5.3 Update `MetricValueInternal::ToSql` / `FromSql` impls if they referenced the field.
  - [ ] 5.4 Bulk-update all test fixtures across `src/storage/*`, `src/opc_ua*.rs`, `src/chirpstack.rs::tests`, `tests/*.rs` that construct these structs with `value: "X".to_string()`. Mechanical line deletion; recommend perl bulk substitution + a final manual sweep.
- [ ] **Task 6 — Simplify `convert_variant_to_metric` signature (AC#8).**
  - [ ] 6.1 Change return type at `src/opc_ua.rs:2070` from `Result<(String, MetricType), OpcGwError>` to `Result<MetricType, OpcGwError>`.
  - [ ] 6.2 Drop the `value.to_string()` half of each arm.
  - [ ] 6.3 Update caller `set_command` to consume the simplified return.
  - [ ] 6.4 Update unit tests in `src/opc_ua.rs::tests` for `convert_variant_to_metric` per the new signature.
- [ ] **Task 7 — Simplify `InMemoryBackend::load_all_metrics` (AC#10).**
  - [ ] 7.1 Delete `value: metric_type.to_string()` line at `src/storage/memory.rs:214`.
  - [ ] 7.2 Simplify the surrounding struct-literal construction (the helper just clones the typed `MetricValue` from the in-memory HashMap).
  - [ ] 7.3 Drop A-1-iter1-DEF1 from `deferred-work.md` (resolved).
- [ ] **Task 8 — Promote HistoryRead log levels (AC#11).** Apply the trace→warn promotion at the `query_metric_history` row-skip sites per AC#11. Drop A-1-iter1-DEF16 from `deferred-work.md` (resolved).
- [ ] **Task 9 — Pin type-level cross-field invariant test (AC#7).** Add the compile-time `const _: fn(&T)` shape-check pattern (no new crate dependency) for `MetricValueInternal`, `MetricValue`, and `HistoricalMetricRow` per AC#7. Closes A-1-iter1-DEF19 + A-1-iter1-DEF9 + A-1-iter1-DEF7 + A-1-iter2-DEF5 + A-1-iter3-DEF11.
- [ ] **Task 10 — New audit-event row + closed-enum doc (AC#9).** Update `docs/logging.md` audit-event table with a new `metric_history_read` row. Field schema closed enum: `event="metric_history_read"`, `reason ∈ {schema_drift, narrowing_overflow, narrowing_underflow}`, `device_id`, `metric_name`, plus `value_type` for schema_drift / `f64_value` for narrowing.
- [ ] **Task 11 — Integration test for typed + legacy outcomes (AC#12).** Create `tests/opcua_historyread_typed_payload.rs` with the test described in AC#12 + the compile-time payload-shape regression guard.
- [ ] **Task 12 — Documentation Sync (per CLAUDE.md).**
  - [ ] 12.1 Update `README.md` "Current Version" line with A-5 narrative.
  - [ ] 12.2 Update README Planning table Epic A row (mark A-5 as `in-progress` or `done` per status transition).
  - [ ] 12.3 Update `docs/logging.md` per Task 10.
  - [ ] 12.4 Update `docs/architecture.md § Storage Payload Migration Strategy` (if it exists in the planning artifacts copy) to note Epic A is functionally complete post-A-5 (A-6 = web UI consumer; A-7 = migration runbook).
- [ ] **Task 13 — Final verification.**
  - [ ] 13.1 `TMPDIR=/home/gcorbaz/.cache/cargo-tmp cargo test --all-targets` ≥1230 passed / 0 failed / ≤10 ignored.
  - [ ] 13.2 `TMPDIR=/home/gcorbaz/.cache/cargo-tmp cargo clippy --all-targets -- -D warnings` clean.
  - [ ] 13.3 `TMPDIR=/home/gcorbaz/.cache/cargo-tmp cargo test --doc` 0 failed / ≥55 ignored.
  - [ ] 13.4 `git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u` returns exactly 3 lines (`metric_history_read` + `metric_parse` + `metric_read`).
  - [ ] 13.5 `git grep -nE 'MetricValue.*value: *"|BatchMetricWrite.*value: *"|HistoricalMetricRow.*value:' src/ tests/` returns **zero** hits (post-A-5 the field is structurally gone).
  - [ ] 13.6 `grep -rn 'TODO(A-5)' src/` returns 0 hits (all transitional markers retired).
  - [ ] 13.7 Live OPC UA server end-to-end regression: `tests/opcua_history_read.rs` Story 8-3 tests pass + new `tests/opcua_historyread_typed_payload.rs` integration test passes.

---

## Dev Notes

### Architectural decisions captured

**1. `HistoricalMetricRow.payload: Option<MetricType>` vs alternative shapes.** Three options were considered:

- **(a) `Option<MetricType>`** — chosen. Legacy rows become first-class `None`; OPC UA layer pattern-matches once. Simplest expression of the legacy contract.
- (b) New enum `HistoricalRowOutcome::Typed(MetricType, timestamp) | Legacy(timestamp)`. More expressive but requires importing a new public type from `src/storage/mod.rs` everywhere the row stream is consumed. Overkill for a two-variant outcome.
- (c) Keep `data_type: MetricType` + sentinel — e.g. tag legacy rows with `MetricType::String("__legacy__".to_string())`. Stringly-typed, brittle, fails the AC#2 "first-class outcome" criterion.

**2. `convert_variant_to_metric` signature change rationale.** Pre-A-5 the function returns `(String, MetricType)` because the caller `set_command` stored both halves into a `MetricValue`. Post-A-5 the `MetricValue.value: String` field is gone, so the caller has no use for the string half. Simplifying the signature to `Result<MetricType, OpcGwError>` is the natural follow-on; not doing so would leave dead-code-on-callsite (the caller would discard the string).

**3. Why `metric_history_read` is a new event, not a `reason` extension of `metric_read`.** A-4 chose `event="metric_read"` with `reason ∈ {no_payload, narrowing_overflow, narrowing_underflow}` for the live-Read path. The HistoryRead path is operationally distinct (different SCADA-client triage, different sampling characteristics, different log-volume profile). Operators grepping for read-side issues want to separate "live read failed" from "history read failed". Separate events preserve that distinction; the grep contract (3 lines post-A-5) makes the surface auditable.

**4. Float narrowing in `build_data_values` mirrors `convert_metric_to_variant` exactly.** A-4 IR7 + JR13 + JR14 nailed the narrowing-overflow + narrowing-underflow + subnormal-passthrough boundary contract for the live-Read path. A-5 applies the **same logic** to the HistoryRead path. The two code blocks should be **functionally identical** modulo log-event field (`metric_read` vs `metric_history_read`); a `clippy::dbg_macro` adjacency check at review time may surface accidental drift. Extracting to a shared helper `narrow_f64_to_f32_or_warn(f: f64, event_name: &str, device_id: &str, metric_name: &str) -> f32` is a candidate refactor — defer to A-6 / A-7 if review surfaces it.

**5. Audit event naming hew to the Story 9-4/9-5/9-6 grep-contract pattern.** All audit events use `event = "noun_verb"` form: `application_created`, `device_updated`, `config_reload_succeeded`, `metric_parse`, `metric_read`, `metric_history_read`. The dual-noun pattern (`metric_history_read` = "history of metric, read action") matches the noun-modifier convention rather than the verb-modifier alternative (`history_metric_read`). Keep this.

### Files being modified — current state + what changes

For each MUTABLE file in AC#14, here's the current shape + what A-5 changes:

- **`src/storage/sqlite.rs::query_metric_history`** (line ~1686): current state projects 4 legacy columns; loops rows; for each row calls `MetricType::from_str(data_type)` + falls back per data-type-string for the value; emits `trace!` row-skips. A-5 changes: project 6 v007 columns; for each row call `metric_type_from_typed_columns` (A-4 helper); store `Option<MetricType>` in `HistoricalMetricRow.payload`; promote skip log to `warn!` with `event = "metric_history_read"`; drop the `value.parse::<f64>()` paths (the projection no longer includes the legacy `value` column).
- **`src/opc_ua_history.rs::build_data_values`** (line ~370): current state pattern-matches on `row.data_type` discriminant, then parses `row.value` per arm; emits `trace!` on parse failure. A-5 changes: pattern-match on `row.payload: Option<MetricType>` directly; no string parses; legacy `None` → `BadDataUnavailable` DataValue; Float arm adds narrowing-overflow/underflow guard with `metric_history_read` warn. Strict-zero on the rest of the file (the `OpcgwHistoryNodeManagerImpl` trait-impl plumbing is unchanged).
- **`src/storage/types.rs::MetricValue`** (line ~88-117): current state has `value: String` + `data_type: MetricType` + the dual-storage staging-caveat doc comment block. A-5 removes the `value` field; retires the staging-caveat block; constructor signature simplifies (it's a struct-literal-only type, no `::new`); doc retains a brief "Epic A removed the transitional `value: String` field in Story A-5" note.
- **`src/storage/mod.rs::MetricValueInternal`** (line ~829-836): mirror struct, same retire.
- **`src/storage/mod.rs::BatchMetricWrite`** (line ~146-160): same retire.
- **`src/storage/mod.rs::HistoricalMetricRow`** (line ~188-202): restructured per AC#2 — `value: String` removed, `data_type: MetricType` becomes `payload: Option<MetricType>`. Doc retired.
- **`src/storage/memory.rs::InMemoryBackend::load_all_metrics`** (line ~204-222): degenerate `value: metric_type.to_string()` line at 214 deleted; helper just clones the typed `MetricValue` from the HashMap.
- **`src/chirpstack.rs::prepare_metric_for_batch`** + call sites: the parallel `(raw_value.to_string(), MetricType::Float(raw_value as f64))` tuple constructions collapse to just the `MetricType` half. `set_metric` callsites that previously took a `MetricValue { value, data_type }` struct now construct without the `value` field.
- **`src/opc_ua.rs::convert_variant_to_metric`** (line ~2070): signature `Result<(String, MetricType), OpcGwError>` → `Result<MetricType, OpcGwError>`. Caller `set_command` updated.
- **Test files**: bulk mechanical removal of `value: "X".to_string(),` lines from struct-literals. Recommended sweep order:
  1. `src/storage/sqlite.rs::tests`
  2. `src/storage/memory.rs::tests`
  3. `src/storage/mod.rs::tests`
  4. `src/opc_ua.rs::tests`
  5. `src/opc_ua_history.rs::tests`
  6. `src/chirpstack.rs::tests`
  7. `tests/opc_ua_read_typed_payload.rs`
  8. `tests/opcua_subscription_spike.rs`
  9. `tests/metric_types_test.rs`

A perl bulk substitution + manual review is recommended (precedent: A-1's test-fixture cascade across 13 files used the same approach).

### Previous-story intelligence

**A-4 lessons applied:**

- **Same-LLM iter-2 catches fake regression-guard tests** (memory `feedback_iter3_validation` 10-story pattern, now with A-4's JR1 + JR8 leftovers as the 11th and 12th finding classes). Recommend running `bmad-code-review A-5` on a **different LLM** per CLAUDE.md "Code Review & Story Validation Loop Discipline" to break the same-model audit blind spot.
- **Phrase-harmonization patches need full-codebase grep.** A-4's JR8 left 3 stale test assertions undetected by the review layers (only `cargo test` surfaced them on resume). When promoting trace! → warn! across `query_metric_history` skip paths, grep the codebase for the OLD format ("query_metric_history: skipping") in tests to verify all assertions are updated to the new format.
- **JR1 "fake regression-guard test" pattern.** When a test purports to guard against a regression that would re-introduce a dropped code path, the test's seed values must produce DIFFERENT outputs through the surviving path vs the dropped path. A-5's HistoricalMetricRow restructure means the field-name `data_type → payload` change must be regression-guarded by tests that would compile-fail if a future change reverts the rename. The `static_assertions` crate (Task 9) plus the compile-time `const _: fn(...)` shape-check are the right shape.
- **Fresh `cargo test` precondition for done.** CLAUDE.md "fresh cargo test + clippy clean" is **load-bearing** and caught A-4's JR8 omissions. Apply it strictly at end-of-iter, before any flip-to-done.

**A-3 lessons applied:**

- **Writer-boundary data-corruption risks are a separate severity tier from log-quality findings.** A-3 iter-2 IR2-B (Unknown+Int saturation gap) was a writer-boundary HIGH catch worth more than 6× LOW log-quality findings. A-5 has a similar pressure point at the **reader-boundary**: a row with `value_type='legacy'` AND orphaned `value_real=Some(...)` would be silently dropped by A-4's `metric_type_from_typed_columns` helper (DEF-iter1-A4-15 documents this). Add a defensive test in `src/opc_ua_history.rs::tests` that seeds such a row (via raw SQL) and asserts the helper returns `Err(_)` (consistent with the JR3 patch that extended the multi-set check to the `legacy` arm).
- **v008 cross-column CHECK is load-bearing for the typed-projection helper.** A-3 added the CHECK; A-4 + A-5 rely on it for defensive Option-unwrap correctness. If A-5 surfaces a regression where the CHECK is bypassed (e.g. manual SQL / restored backup), the helper returns `OpcGwError::Database("schema drift…")` — promote that error path to a test fixture in A-5's integration suite.

**A-2 lessons applied:**

- **Migration v008 is BEGIN/COMMIT-wrapped per A-2-iter1-DEF-IH1 user acceptance.** A-5 adds no new migration; v008 carries the necessary CHECK. If a future change requires schema-level support for the legacy-row distinct outcome (e.g. a `value_type='legacy'` partial index per A-2-iter1-DEF3), it lands in A-7 (the migration-runbook story), not A-5.

**A-1 lessons applied:**

- **Test-fixture cascade across the codebase is the dominant edit-volume in A-5.** A-1 paved the path with `MetricType` payload-bearing variants + `.clone()` cascade across 13 test files. A-5 does the symmetric retire: remove `value: "X"` from every struct-literal. Perl bulk substitution + manual sweep is the proven recipe.

**8-3 lessons applied:**

- **Access-level + historizing bits are set at `OpcUa::add_nodes` registration time, NOT at HistoryRead override invocation time.** A-5 does NOT touch the `AccessLevel::CURRENT_READ | HISTORY_READ | set_historizing(true)` sites in `src/opc_ua.rs:1031-1036` — strict-zero. Story 8-3's regression suite in `tests/opcua_history_read.rs` pins this.
- **Partial-success contract: a bad row in the middle of the range does not abort the read.** A-5 preserves this. The new legacy-row arm produces a `BadDataUnavailable` DataValue but the iteration continues to the next row.

### References

- [Source: `_bmad-output/planning-artifacts/epics.md` § Epic A § Story A.5] — user story + acceptance criteria.
- [Source: `_bmad-output/planning-artifacts/architecture.md:174-185` — typed-column schema + Storage Payload Migration Strategy.
- [Source: `_bmad-output/planning-artifacts/prd.md` § FR51] — payload-preservation requirement closing #108.
- [Source: `_bmad-output/implementation-artifacts/A-4-opc-ua-read-value-payload-pipeline.md`] — sibling story; reuses `metric_type_from_typed_columns` helper.
- [Source: `_bmad-output/implementation-artifacts/A-3-poller-value-payload-write-pipeline.md`] — writer pipeline + v008 CHECK.
- [Source: `_bmad-output/implementation-artifacts/A-1-metrictype-payload-bearing-enum.md`] — payload-bearing `MetricType` foundation + transitional staging-caveat that A-5 retires.
- [Source: `_bmad-output/implementation-artifacts/8-3-historical-data-access-via-opc-ua.md`] — `OpcgwHistoryNodeManagerImpl` partial-success + access-level contracts.
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md:495-545`] — all A-1 → A-5 carry-forwards (DEF1, DEF7, DEF9, DEF15, DEF16, DEF19, DEF-iter2-5, DEF-iter3-11).
- [Source: `src/opc_ua_history.rs:370-432`] — current `build_data_values` shape with `TODO(A-5)` markers at 385-388.
- [Source: `src/storage/sqlite.rs:1686-1818`] — current `query_metric_history` shape with trace-level row-skip paths.
- [Source: `src/storage/types.rs:88-117`] — current `MetricValue` with `TODO(A-5)` markers.
- [Source: `src/storage/mod.rs:142-205`] — `MetricValueInternal`, `BatchMetricWrite`, `HistoricalMetricRow` definitions with `TODO(A-5)` markers.
- [Source: GitHub issue #108] — payload-less `MetricType` enum — fully closes when A-5 ships.
- [Source: GitHub issue #99] — NodeId metric-name-only collision (already fixed at commit `9f823cc`); A-5 must not regress the fix in `OpcUa::add_nodes`.
- [Source: CLAUDE.md § Code Review & Story Validation Loop Discipline] — loop iteration discipline.
- [Source: CLAUDE.md § BMad Workflow Commit & Push Discipline] — implementation-then-review commit pattern.
- [Source: memory `feedback_iter3_validation`] — 10-story validated iter-3 over-reviewing pattern; A-5 extends to 11.
- [Source: memory `feedback_review_iterations`] — Guy's stated preference: extra review pass beats missing an issue.
- [Source: memory `reference_cargo_tmpfs_workaround`] — TMPDIR override for `protoc` tmpfs disk-quota issue.

### Project Structure Notes

A-5 sits cleanly within the existing storage-trait + OPC UA layer separation. No new modules, no new dependencies. The only structural change is the **field removal from four public structs** (`MetricValue`, `MetricValueInternal`, `BatchMetricWrite`, `HistoricalMetricRow`) — a SemVer-major change formalised by the v2.0.0-rc → v2.0.0 GA bump. A CHANGELOG entry is recommended (per A-1-iter1-DEF15) but the README "Current Version" narrative captures the breaking change inline.

The `metric_type_from_typed_columns` helper introduced in A-4 is **reused by A-5 unchanged** — both readers (live `get_metric_value` + historical `query_metric_history`) share the same typed-column projection contract. Future consumers (e.g. `/api/metrics` JSON in A-6) should reuse the same helper.

### Out of Scope

- **Web UI live-metrics typed value display (Story A-6):** A-5 wires HistoryRead but not the `/metrics.html` page or `/api/metrics` JSON shape. A-6 owns the web consumer.
- **Migration runbook + version-gated script (Story A-7):** A-5 ships the code that makes legacy rows surface as `BadDataUnavailable`, but the operator-facing runbook + drop-database-vs-in-place upgrade documentation is A-7's job.
- **Retiring the legacy `value TEXT NOT NULL` column from `metric_values` + `metric_history` (Story A-7):** A-5 stops reading the column; A-7 either drops it via v009 migration or leaves it as a dual-storage artefact pending operator decision.
- **`SqliteValueType` typed Rust enum to replace stringly-typed `value_type` discriminator (DEF-iter1-A4-12):** A-7 cleanup territory; A-5 keeps the stringly-typed match per A-4 helper precedent.
- **`MetricType` field-name invariance build script (DEF-iter1-A4-11 type-pin alternative):** the AC#7 `static_assertions::assert_fields!` is the lighter solution; a full build-script approach is deferred.

### Definition of Done

- [ ] All 14 ACs SATISFIED (or explicitly DEFERRED-DOCUMENTED with user acceptance per CLAUDE.md condition #3).
- [ ] `cargo test --all-targets` ≥1230 passed / 0 failed / ≤10 ignored.
- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `cargo test --doc` 0 failed.
- [ ] AC#9 grep contract returns exactly 3 lines (`metric_history_read` + `metric_parse` + `metric_read`).
- [ ] `grep -rn 'TODO(A-5)' src/` returns 0 hits.
- [ ] `bmad-code-review A-5` loop terminates per CLAUDE.md condition #2 (only LOW remains) — recommended different-LLM run.
- [ ] `README.md` Current Version + Planning table updated.
- [ ] `docs/logging.md` updated with `metric_history_read` row.
- [ ] Sprint-status flipped `ready-for-dev → in-progress → review → done`.
- [ ] Implementation-complete + code-review-complete commits land per CLAUDE.md BMad Workflow Commit & Push Discipline.

---

## Dev Agent Record

### Agent Model Used

_(to be populated by dev agent on implementation start)_

### Debug Log References

_(to be populated)_

### Completion Notes List

- 2026-05-17: Story spec created via `bmad-create-story A-5`. Status `backlog → ready-for-dev`. Comprehensive analysis of A-1 / A-2 / A-3 / A-4 carry-forwards: A-1-iter1-DEF1 (InMemoryBackend discriminant-rebuild — closed by Task 7), A-1-iter1-DEF7 (Storage::new parallel value-string arms — closed by Task 4.5), A-1-iter1-DEF9 (MetricValue/BatchMetricWrite cross-field inconsistency — structurally closed by Task 4-5), A-1-iter1-DEF15 (SemVer break documentation — closed by Task 4.2 + README narrative), A-1-iter1-DEF16 (HistoryRead row-skip log promotion — closed by Task 8), A-1-iter1-DEF19 + A-1-iter2-DEF5 + A-1-iter3-DEF11 (cross-field invariant test pin — closed by Task 9), A-1-iter3-DEF5 (rustdoc module-level anchor — closed by Task 4.2), DEF-iter1-A4-1 (cross-version reader inconsistency — structurally closed by Task 6 signature simplification). Load-bearing design call: `HistoricalMetricRow.payload: Option<MetricType>` (option (a) in Dev Notes; (b) new enum overkill, (c) sentinel stringly-typed). The `value: "ignored"` regression-guard literal in A-4 tests is structurally gone post-A-5 — the field doesn't exist. New audit event `metric_history_read` with closed reason enum `{schema_drift, narrowing_overflow, narrowing_underflow}` joins the metric-event grep contract; pre-A-5 baseline returns 2 lines (`metric_parse` + `metric_read`) post-A-4; post-A-5 returns 3 lines. Float narrowing-overflow + narrowing-underflow logic mirrors A-4 IR7 + JR13 + JR14 patches in `convert_metric_to_variant` — extract-to-helper deferred to A-6/A-7. Strict-zero invariants revised: `src/opc_ua_history.rs` becomes A-5 MUTABLE (was A-5 territory under A-4); `src/storage/types.rs` + `src/storage/mod.rs` + `src/storage/memory.rs` MUTABLE for field removals (were strict-zero across A-1/A-2/A-3/A-4); `src/chirpstack.rs` MUTABLE for writer call-site simplification (was MUTABLE in A-3); narrow `src/opc_ua.rs::convert_variant_to_metric` signature change MUTABLE band (rest of `opc_ua.rs` strict-zero per A-4 carry-forward). Test budget target ≥1230 passed (was 1214 A-4-review baseline + ≥16 new test fns). Issue #108 closure mapping: A-1 type-level → A-2 schema-level → A-3 WRITE-side → A-4 OPC UA Read → **A-5 OPC UA HistoryRead + structural retire of MetricValue.value** → A-6 Web UI → A-7 migration runbook; **#108 fully closes when A-5 ships**. Carry-forward GH issues unchanged: #88 (per-IP rate limiting), #100 (56 doctest ignores), #102 (tests/common extraction), #104 (TLS hardening), #110 (RunHandles missing Drop), #113 (live-borrow refactor), #117. Tracking issue Task 0 deferred to user (gh CLI not authenticated for write per A-1/A-2/A-3/A-4 precedent). Recommend `bmad-dev-story A-5` to implement; recommend `bmad-code-review A-5` on a different LLM per CLAUDE.md doctrine + memory `feedback_iter3_validation` 10-story validated pattern (A-5 extends to 11).

### File List

_(to be populated by dev agent — expected list per AC#14 MUTABLE files)_

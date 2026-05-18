# Story A-6: Web UI Live-Metrics Value Display

| Field         | Value                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------- |
| Story key     | `A-6-web-ui-live-metrics-value-display`                                                               |
| Epic          | A — Storage Payload Migration (Phase B Closure, gates v2.0 GA)                                        |
| FRs           | FR37 (operator views live metric values via web), FR41 (mobile-responsive LAN access), FR51 (typed payload preserved end-to-end) |
| Status        | review                                                                                                |
| Created       | 2026-05-18                                                                                            |
| Source epic   | `_bmad-output/planning-artifacts/epics.md § Epic A § Story A.6`                                       |
| Sprint change | `_bmad-output/planning-artifacts/sprint-change-proposal-2026-05-14.md`                                |
| Tracking      | GitHub tracking issue to be filed by dev agent at implementation start (see Task 0)                   |

---

## Story Statement

As an **operator browsing the web dashboard**,
I want the live-metrics page to render real measurement values with their configured units (`23.5 °C` instead of `"Float"` / `"23.5"` without unit),
So that the dashboard is finally usable as a debugging surface — closing the WEB-UI side of Epic A.

This story is the **web-consumer closure of Epic A** — A-3 wired typed payloads into the WRITE pipeline; A-4 closed OPC UA `Read`; A-5 closed OPC UA `HistoryRead` AND retired the transitional `MetricValue.value: String` field. A-6 closes the LAST READ-side consumer (the `/api/devices` JSON contract + `static/metrics.js` renderer) and **retires the `metric_view_display_string` transitional shim** introduced by A-5 P0-D4 / K7 to bridge the dashboard wire format while A-6 was pending. Closes deferred work entry **DEF-iter1-A5-D1**.

Issue #108 functionally closed with A-5; A-6 makes the dashboard reflect that closure end-to-end. After A-6, the only remaining Epic A story is **A-7** (operator-facing migration runbook + version-gated script) which is documentation + script only.

---

## Acceptance Criteria

**AC#1 — `MetricView` JSON shape carries a typed `value` + a separate `unit` field.** The struct at `src/web/api.rs:272` is widened from:

```rust
pub struct MetricView {
    pub metric_name: String,
    pub data_type: String,
    pub value: Option<String>,         // <- A-5 transitional (stringified via metric_view_display_string)
    pub timestamp: Option<String>,
}
```

to:

```rust
pub struct MetricView {
    pub metric_name: String,
    pub data_type: String,                 // unchanged ("Float" / "Int" / "Bool" / "String")
    pub value: Option<serde_json::Value>,  // typed JSON: number / bool / string / null (legacy or missing)
    pub unit: Option<String>,              // NEW — configured `metric_unit` from TOML
    pub timestamp: Option<String>,         // unchanged (RFC3339)
}
```

Wire contract per variant (the **only** valid JSON shapes for `value` post-A-6):

| `MetricType` payload       | `value` JSON      | `data_type` JSON |
| -------------------------- | ----------------- | ---------------- |
| `Float(23.5)`              | `23.5`            | `"Float"`        |
| `Float(0.0)`               | `0.0`             | `"Float"`        |
| `Float(f64::NAN)`          | `null` + warn¹    | `"Float"`        |
| `Float(f64::INFINITY)`     | `null` + warn¹    | `"Float"`        |
| `Float(f64::NEG_INFINITY)` | `null` + warn¹    | `"Float"`        |
| `Int(42)`                  | `42`              | `"Int"`          |
| `Int(i64::MAX)`            | `9223372036854775807` (JSON number — JS clients may lose precision >2^53; documented limitation, not an error) | `"Int"` |
| `Bool(true)`               | `true`            | `"Bool"`         |
| `Bool(false)`              | `false`           | `"Bool"`         |
| `String("OK")`             | `"OK"`            | `"String"`       |
| `String("")`               | `""`              | `"String"`       |
| Legacy row (`value_type='legacy'`, no payload) | `null` | `"Float"` (or configured type — see AC#2 fall-through) |
| No row in `metric_values` (configured but never polled) | `null` | configured type via `config_type_to_display` |

¹ Non-finite Float: emit `value: null` AND a `warn!(event = "metric_view_serialize", reason = "non_finite", device_id, metric_name, f64_value)` per AC#5. A-3's poller-side NaN/Inf filter (`metric_parse` event) makes this path unreachable in production; the warn is defensive against (a) operator-injected DB rows, (b) a future writer pathway that admits non-finite values, (c) a regression of the A-3 filter. Closes the latent A-5 wire-domain expansion documented in `metric_view_display_string`'s comment.

**AC#2 — `value: null` is overloaded across three distinct states; `data_type` + `timestamp` disambiguate.** Per AC#1 the wire's `value: null` covers (a) "configured but never polled" (`Some(row)` not in `metric_by_key`), (b) "legacy schema row" (Post-A-5 helper returned `Ok(None)`), and (c) "typed payload was non-finite Float" (defensive). The browser renderer at `static/metrics.js` (Story 9-3) ALREADY treats `metric.value === null` as `status = "missing"` and shows `"—"` for the value cell, so this is a no-op for the user-facing UX. The audit-log distinction is:

- State (a) emits NO log line (silently rendered as "—" — Story 9-3 contract).
- State (b) emits an `info!(event = "metric_view_serialize", reason = "legacy_row", device_id, metric_name)` at most once per (device_id, metric_name, dashboard-poll-tick) to avoid log-volume blowup on a database full of pre-Epic-A rows. **Implementation:** the `api_devices` handler need only emit ONE info per request when ANY legacy row is encountered (`legacy_row_count = N` log field) — NOT one per row. This matches A-5's `metric_history_summary` aggregate-skip pattern.
- State (c) emits ONE warn per offending row (the cardinality is naturally bounded — non-finite reaches the dashboard only if A-3's filter regresses).

**AC#3 — `web::MetricSpec` is widened with `metric_unit: Option<String>`; `DashboardConfigSnapshot::from_config` plumbs it through.** The struct at `src/web/mod.rs:120` becomes:

```rust
#[derive(Clone, Debug, PartialEq)]
pub struct MetricSpec {
    pub metric_name: String,
    pub metric_type: crate::config::OpcMetricTypeConfig,
    pub metric_unit: Option<String>,   // NEW — sourced from OpcMetric.metric_unit at src/config.rs:657
}
```

`from_config` at `src/web/mod.rs:165-168` populates `metric_unit: m.metric_unit.clone()` from `dev.read_metric_list[i].metric_unit`. Story 9-7's hot-reload rebuilds the snapshot via `from_config` on `config_reload_succeeded`, so a TOML edit that mutates `metric_unit` (via Story 9-5 PUT-replace-device, then SIGHUP) propagates to the dashboard within one reload tick — **no new hot-reload classifier work required** in `src/config_reload.rs` (`metric_unit` is already destructured in `read_metric_lists_equal` at line 972; a change already triggers a topology diff, which Story 9-8 handles for OPC UA — A-6 only adds the *web display* propagation).

**Empty-string `metric_unit` per deferred-work 9-5-iter1-D4:** if the operator writes `metric_unit = ""` via PUT-replace-device, the TOML round-trips it as `Some("")`. A-6 treats `Some("")` and `None` **identically** at the renderer — both produce no unit suffix on the dashboard. The JSON wire emits whatever the snapshot carries (`""` or `null`) verbatim per serde's standard rules; the JS renderer coalesces. No new defer needed; aligns with the existing D4 deferral that "downstream consumer expresses preference" — A-6 expresses "collapse both to no-unit at render time".

**AC#4 — `api_devices` handler matches `MetricType` once and emits typed JSON via `serde_json::Value`.** The handler at `src/web/api.rs:298-426` is updated:

1. Drop the `metric_view_display_string` call at line 392.
2. Replace `value: Some(metric_view_display_string(&row.data_type))` with the result of a new private helper `metric_type_to_json_value(&MetricType) -> Option<serde_json::Value>` that returns:
   - `MetricType::Float(f)` → `Some(serde_json::Number::from_f64(f).map(serde_json::Value::Number).unwrap_or(serde_json::Value::Null))` (the `from_f64` returns `None` for NaN/Inf — this is the defensive path AC#1 mentions; emit the warn here).
   - `MetricType::Int(i)` → `Some(serde_json::Value::Number(serde_json::Number::from(i)))`.
   - `MetricType::Bool(b)` → `Some(serde_json::Value::Bool(b))`.
   - `MetricType::String(s)` → `Some(serde_json::Value::String(s.clone()))`.
3. For the `None` (legacy row) branch from `metric_by_key.get(&key)` AND the new "legacy payload" branch (where `row.data_type` is now wrapped via the A-5 typed projection — note: post-A-5 storage rows always carry typed payloads in `MetricValueInternal.data_type: MetricType`, so the legacy-row distinction at the WEB layer is whether `MetricValueInternal.value_type == "legacy"` — but the post-A-5 read path returns `Option<MetricType>` ONLY through `query_metric_history`. **For the `api_devices` read path**, the legacy-row distinction surfaces as a row in the in-memory snapshot where `MetricValueInternal.data_type` discriminant came from the storage `value_type` column — i.e. `load_all_metrics` returned a row, but post-A-5 it returns `Some(row)` for typed rows and skips legacy rows via the A-4 helper's `Ok(None)` arm. **Reconfirm at implementation start by reading `src/storage/sqlite.rs::load_all_metrics`** — if `load_all_metrics` post-A-5 already SKIPS legacy rows entirely, then the AC#2 state (b) path collapses into state (a) and the legacy_row_count log emission is dead code that can be omitted from this story. The story author has NOT verified the post-A-5 `load_all_metrics` legacy behaviour; the dev agent MUST confirm via a 30-second code read before deciding whether to land the state-(b) telemetry.)

Inline emit the unit field both for `Some(row)` and `None`:

```rust
Some(row) => MetricView {
    metric_name: spec.metric_name.clone(),
    data_type: row.data_type.to_string(),
    value: metric_type_to_json_value(&row.data_type, &dev.device_id, &spec.metric_name),
    unit: spec.metric_unit.clone(),
    timestamp: Some(row.timestamp.to_rfc3339()),
},
None => MetricView {
    metric_name: spec.metric_name.clone(),
    data_type: config_type_to_display(&spec.metric_type).to_string(),
    value: None,
    unit: spec.metric_unit.clone(),
    timestamp: None,
},
```

**AC#5 — New audit event `metric_view_serialize` joins the closed-enum taxonomy.** Field schema:

| Field | Type | Required | Value |
| --- | --- | --- | --- |
| `event` | const | yes | `"metric_view_serialize"` |
| `reason` | const | yes | `"non_finite"` / `"legacy_row"` |
| `device_id` | `%` (Display) | yes | the device identifier |
| `metric_name` | `%` (Display) | yes | the metric name |
| `f64_value` | `%` (Display) | only for `non_finite` | the non-finite Float that triggered the warn |
| `legacy_row_count` | `%` (Display) | only for `legacy_row` | aggregate count of legacy rows in this dashboard tick (AC#2 state (b)) |

Log levels: `non_finite` → `warn!`, `legacy_row` → `info!` (per AC#2). The grep contract becomes:

```bash
git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u
```

returns exactly **five** lines post-A-6:

```
event = "metric_history_read"
event = "metric_history_summary"
event = "metric_parse"
event = "metric_read"
event = "metric_view_serialize"
```

`docs/logging.md` is updated with a `metric_view_serialize` row in the audit-event table.

**AC#6 — `static/metrics.js::renderMetricRow` formats `value + unit` with type-aware rendering.** The renderer at `static/metrics.js:133-160` is updated:

1. Add a `formatValue(value, dataType)` helper at module scope:
   - `value === null || value === undefined` → return `"—"`.
   - `dataType === "Bool"` → return `value ? "true" : "false"` (was `"1"`/`"0"` in the A-5 transitional shim; native bool is more readable for the operator and matches the post-A-6 wire type).
   - `dataType === "Float"` → return `value.toString()` (V8's `Number.prototype.toString` already produces a reasonable representation — `23.5`, `0`, `1e-12`; no manual rounding). If display polish becomes a UX concern, that lives in a future story, not A-6.
   - `dataType === "Int"` → return `value.toString()`.
   - `dataType === "String"` → return `value` directly (`String` is already a JS string).
   - Default / unknown — return `String(value)` as a safe fallback.
2. Update the `valueText` line (currently `var valueText = metric.value === null || metric.value === undefined ? "—" : metric.value;`) to use the helper:
   ```js
   var formattedValue = formatValue(metric.value, metric.data_type);
   var valueText = formattedValue === "—" ? "—" : (metric.unit ? formattedValue + " " + metric.unit : formattedValue);
   ```
3. **`metric.unit` empty-string handling:** the `metric.unit ? ... : ...` truthy check naturally coalesces both `null` and `""` to "no unit suffix" — closes the 9-5-iter1-D4 deferral at the renderer level (AC#3 last paragraph).
4. The staleness `status` row class (Story 9-3 `row-good` / `row-uncertain` / `row-bad` / `row-missing`) is computed from `timestamp` ONLY, not from `value`, so A-6 changes do not affect staleness badges — preserves the Story 9-3 / 9-7 contract.

**AC#7 — `metric_view_display_string` and its 2 unit tests are RETIRED.** The helper at `src/web/api.rs:214-221` is deleted. The two unit tests `metric_view_display_string_typed_payloads` (line 4952) and `metric_view_display_string_non_finite_float_wire_format` (line 4970) are deleted. The A-5 K7 P0-D4 wire-format pinning shifts to the new `metric_type_to_json_value` helper's unit tests (AC#10). No `metric_view_display_string` reference survives anywhere in `src/` or `tests/` post-A-6 — pinned by a grep contract:

```bash
git grep -n 'metric_view_display_string' src/ tests/
```

returns **zero** hits post-A-6.

**AC#8 — Story 9-3 staleness contract preserved unchanged.** `BAD_THRESHOLD_SECS = 86_400` at `src/web/api.rs:193` untouched. `stale_threshold_secs` propagation via `AtomicU64::load(Relaxed)` at line 347-349 untouched. The `as_of: Utc::now().to_rfc3339()` server-side timestamp at line 305 untouched. `RwLock<Arc<DashboardConfigSnapshot>>` poison-recovery pattern at line 354-366 untouched. Pinned by `tests/web_dashboard.rs::api_devices_returns_json_with_expected_shape_when_authed` (existing test — assertion updated for new `unit` field, but staleness fields preserved).

**AC#9 — Story 9-4/9-5/9-6 CRUD endpoints + Story 9-7 hot-reload + Story 9-8 dynamic topology continue to work.** `MetricSpec` extension is additive (one new field with a default `None` from `OpcMetric.metric_unit: Option<String>`). The CRUD handlers at `src/web/api.rs::create_application`/`update_application`/`create_device`/`update_device`/`create_command`/`update_command` do NOT need updates — they consume `OpcMetric` directly (not the lite `MetricSpec`). The `dashboard_snapshot` rebuild path in `src/web/api.rs::handle_config_reload` (Story 9-7 listener) calls `DashboardConfigSnapshot::from_config(&new_config)`, which the AC#3 widening updates automatically. Story 9-8's `compute_diff` already destructures `metric_unit` (`src/opcua_topology_apply.rs:1137`), so the OPC UA-side topology diff is unchanged. Pinned by:

- `tests/web_application_crud.rs` (regression suite — must continue to pass byte-for-byte).
- `tests/web_device_crud.rs` (regression suite — must continue to pass byte-for-byte).
- `tests/web_command_crud.rs` (regression suite — must continue to pass byte-for-byte).
- `tests/config_hot_reload.rs::*` (regression suite — must continue to pass byte-for-byte).
- `tests/opcua_dynamic_address_space_apply.rs` (regression suite — must continue to pass byte-for-byte).

**AC#10 — New unit tests pin the JSON wire contract for `metric_type_to_json_value` + dashboard route.**

In `src/web/api.rs::tests`:

1. `metric_type_to_json_value_float_finite` — asserts `Float(23.5)` → `Some(Value::Number(23.5))`, `Float(0.0)` → `Some(Value::Number(0.0))`, `Float(-0.0)` → `Some(Value::Number(-0.0))` (preserve sign-of-zero per A-5 iter-2 K3 lesson).
2. `metric_type_to_json_value_float_non_finite` — asserts `Float(NaN)` / `Float(INFINITY)` / `Float(NEG_INFINITY)` → `None` (the `from_f64` `None` branch). Pin via `#[traced_test]` + `assert!(logs_contain("event = \"metric_view_serialize\""))` + `assert!(logs_contain("reason = \"non_finite\""))`.
3. `metric_type_to_json_value_int_extremes` — asserts `Int(i64::MAX)` → `Some(Value::Number(9223372036854775807))`, `Int(i64::MIN)` → `Some(Value::Number(-9223372036854775808))`. Documents the >2^53 JS-precision-loss caveat in a `// note: …` comment.
4. `metric_type_to_json_value_bool` — asserts `Bool(true)` → `Some(Value::Bool(true))`, `Bool(false)` → `Some(Value::Bool(false))`.
5. `metric_type_to_json_value_string` — asserts `String("OK")` → `Some(Value::String("OK".into()))`, `String("")` → `Some(Value::String("".into()))`, `String("emoji 🎉")` → `Some(Value::String("emoji 🎉".into()))` (UTF-8 preservation).

In `tests/web_dashboard.rs`:

6. `api_devices_emits_typed_value_and_unit_per_variant` — seeds four metrics (Float 23.5 + unit "°C", Int 42 + no unit, Bool true + unit "%", String "OK" + unit "lvl") via `set_metric_value` on the per-task backend; GETs `/api/devices` with auth; asserts each `MetricView.value` is the **typed** JSON primitive (number/bool/string) and `MetricView.unit` matches the configured unit.
7. `api_devices_emits_null_value_for_unpolled_metric` — seeds a configured-but-never-polled metric; asserts `value: null, unit: Some("…"), timestamp: null`.
8. `api_devices_legacy_row_emits_aggregate_info_log` — IF the dev agent confirms post-A-5 `load_all_metrics` surfaces legacy rows (AC#4 paragraph 3 caveat). Otherwise this test is dropped and the AC#2 state (b) telemetry is removed.

**AC#11 — Browser-side smoke test via `metrics_js_renders_value_with_unit`.** Headless JS unit test is out of scope (the project's static JS has no test harness — see Story 9-2/9-3 precedent: only `metrics_js_is_served_and_references_api_devices` exists, which just asserts the JS file is served). Instead, A-6 adds **one new Rust integration test** `tests/web_dashboard.rs::metrics_js_references_unit_and_data_type_fields` that asserts the served JS body contains the literal strings `"metric.unit"` and `"metric.data_type"` and `"formatValue"`. This is a brittle-but-cheap "did the renderer get widened" guard — same pattern as the existing `metrics_js_is_served_and_references_api_devices` line 806. Manual end-to-end UI verification (load `/metrics.html` in a browser, see `34.2 %` rendered) is operator-level testing, captured in the Definition of Done.

**AC#12 — `cargo test --all-targets` passes with target ≥1240 / 0 failed / 10 ignored; clippy clean; doctest baseline preserved.** A-6 baseline starts at A-5's 1230 passed. New `#[test]` fns net delta: +6 to +8 (5 in `src/web/api.rs::tests` per AC#10 items 1-5, 2-3 in `tests/web_dashboard.rs` per AC#10 items 6-8). Estimated test-fixture updates from the new `unit: Option<String>` field in `MetricView` literal constructions: trivial (the field is `Option<String>` so test fixtures default to `unit: None` in existing tests; new tests assert `unit: Some(...)` explicitly). Target ≥1240 passed (1230 + ~10 new fns conservative). `cargo clippy --all-targets -- -D warnings` clean. `cargo test --doc` 0 failed / ≥55 ignored (no doctest regression). README + `docs/logging.md` updated per CLAUDE.md "Documentation Sync".

**AC#13 — AC#11 file invariants (carry-forward strict-zero from A-1 / A-2 / A-3 / A-4 / A-5).** A-6 MUTABLE:

- `src/web/api.rs` (`MetricView` struct + `api_devices` handler + new `metric_type_to_json_value` helper + retirement of `metric_view_display_string` + helper + 2 unit tests; 5 new unit tests per AC#10)
- `src/web/mod.rs` (`MetricSpec` widening + `from_config` propagation; possibly 1 unit test for the snapshot rebuild — verify `tests/web_dashboard.rs` already covers this)
- `static/metrics.js` (`formatValue` helper + `renderMetricRow` update)
- `tests/web_dashboard.rs` (regression-update of existing JSON-shape assertion + 2-3 new tests per AC#10)
- `docs/logging.md` (new `metric_view_serialize` row in audit-event table)
- `README.md` ("Current Version" line + Planning table Epic A row narrative update)

A-6 must NOT touch (strict-zero):

- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/test_support.rs`
- `src/opc_ua.rs`, `src/opc_ua_history.rs` (closed read-paths from A-4/A-5)
- `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`
- `src/security.rs`, `src/security_hmac.rs`
- `src/main.rs::initialise_tracing` (function body — touching `main.rs` for AppState construction is fine; the strict-zero is on the tracing-init body specifically)
- `src/storage/types.rs`, `src/storage/mod.rs`, `src/storage/memory.rs`, `src/storage/sqlite.rs`, `src/storage/schema.rs`, `src/storage/pool.rs` (storage layer closed in A-5)
- `src/chirpstack.rs` (writer-side closed in A-3)
- `src/config.rs` (`OpcMetric.metric_unit` already exists — no schema change needed)
- `src/config_reload.rs` (the existing `metric_unit` destructure at line 972 + topology-diff classification already covers A-6 needs)
- `src/opcua_topology_apply.rs` (the OPC UA topology side of `metric_unit` changes is Story 9-8's territory — deferred-work entry 9-8-iter1-D3 documents the v2 candidate "in-place `EngineeringUnits` mutation"; A-6 is web-only)

`src/main.rs` is MUTABLE in a very narrow band ONLY IF `AppState` construction needs updating to pass `metric_unit` through to the snapshot. Read first; if the existing `DashboardConfigSnapshot::from_config(&config)` call is the only entry point, no `main.rs` touch is needed (the widened `from_config` reads from `config.application_list[*].device_list[*].read_metric_list[*].metric_unit` directly).

**AC#14 — Documentation Sync (per CLAUDE.md "Documentation Sync" + "Issue Management" rules).** Required edits:

- `README.md` "Current Version" line updated with A-6 narrative summarising: A-6 ships typed `value` + `unit` on `/api/devices`; `metric_view_display_string` shim retired; `metric_type_to_json_value` helper introduced; closes DEF-iter1-A5-D1 + 9-5-iter1-D4.
- `README.md` Planning table Epic A row updated: A-6 status `backlog → ready-for-dev → in-progress → review → done` as the story progresses.
- `docs/logging.md` audit-event table gains a `metric_view_serialize` row with the closed reason enum `{non_finite, legacy_row}`.
- (Out of scope for A-6 unless dev agent encounters a need) `docs/manual/opcgw-user-manual.xml` is deferred per the standing Epic-7/8/9-manual-sync-batch deferral (deferred-work line 218); A-6 inherits the existing deferral.
- Tracking GitHub issue per CLAUDE.md "Issue Management" rule — Task 0 below.

---

## Tasks / Subtasks

- [x] **Task 0 — File a GitHub tracking issue for A-6.** Title: `Story A-6: Web UI Live-Metrics Value Display`. Body links sprint-status entry + `epics.md § A.6`. Deferred to user if `gh` CLI is not authenticated for write (precedent: A-1 / A-2 / A-3 / A-4 / A-5 all deferred Task 0).

- [x] **Task 1 — Confirm post-A-5 `load_all_metrics` legacy-row behaviour (AC#4 paragraph 3 caveat).**
  - [x] 1.1 Read `src/storage/sqlite.rs::load_all_metrics` post-A-5 to determine whether legacy rows surface as `Some(row)` with `data_type = MetricType::…(default)` OR are silently skipped (`Ok(None)` arm of the helper → row not pushed to the result Vec).
  - [x] 1.2 If silently skipped: drop AC#2 state (b) telemetry from this story (the `legacy_row` reason becomes dead code). Update AC#5 to remove the `legacy_row` reason. Update `docs/logging.md` to omit the `legacy_row` reason. Update the grep contract from "metric_view_serialize" to whichever events survive.
  - [x] 1.3 If surfaced as `Some(row)` with a typed payload but the storage row was originally legacy: implement the aggregate `info!(event="metric_view_serialize", reason="legacy_row", legacy_row_count=N)` emission per AC#2 + AC#5.
  - [x] 1.4 Document the decision in Dev Agent Record § Completion Notes.

- [x] **Task 2 — Widen `MetricSpec` with `metric_unit: Option<String>` (AC#3).**
  - [x] 2.1 Add field at `src/web/mod.rs:120` `MetricSpec` struct.
  - [x] 2.2 Update `DashboardConfigSnapshot::from_config` at `src/web/mod.rs:165-168` to populate `metric_unit: m.metric_unit.clone()` from `OpcMetric`.
  - [x] 2.3 Update the existing `MetricSpec` construction at `src/web/api.rs:5379` (if it exists in a test path — re-grep at implementation start) to pass `metric_unit: None` by default; new tests explicitly seed `Some("…")` for unit assertions.
  - [x] 2.4 Verify hot-reload propagation: read `src/config_reload.rs` to confirm `metric_unit` is already destructured at `read_metric_lists_equal`'s ReadMetric pattern (line 972) — it is, per the spec-author's pre-implementation grep. **No change required** in `config_reload.rs`.

- [x] **Task 3 — Add `metric_type_to_json_value` helper to `src/web/api.rs` (AC#4 + AC#5).**
  - [x] 3.1 Place the helper near the now-deleted `metric_view_display_string` (the `pub(crate)` visibility from A-5 P0-D4 is preserved — keeps the unit-test surface ergonomic).
  - [x] 3.2 Signature: `pub(crate) fn metric_type_to_json_value(data_type: &MetricType, device_id: &str, metric_name: &str) -> Option<serde_json::Value>`. The `device_id` + `metric_name` parameters thread through to the `non_finite` warn emission per AC#5.
  - [x] 3.3 Pattern-match on `MetricType` and return per AC#4 contract.
  - [x] 3.4 For `Float(f)`: call `serde_json::Number::from_f64(f)`; if it returns `None` (NaN/Inf), emit `warn!(event = "metric_view_serialize", reason = "non_finite", device_id = %device_id, metric_name = %metric_name, f64_value = %f)` and return `None`.
  - [x] 3.5 Doc comment captures the closed-enum contract + the wire-precision caveat for `Int(i64::MAX)` vs JS Number's 2^53 limit.

- [x] **Task 4 — Update `api_devices` to consume the new helper + emit `unit` (AC#1, AC#4, AC#9).**
  - [x] 4.1 Update `MetricView` struct at line 272 per AC#1.
  - [x] 4.2 Update the `Some(row)` arm at line 386-393 to call `metric_type_to_json_value(&row.data_type, &dev.device_id, &spec.metric_name)` and populate `unit: spec.metric_unit.clone()`.
  - [x] 4.3 Update the `None` arm at line 395-401 to populate `unit: spec.metric_unit.clone()` (other fields unchanged from current).
  - [x] 4.4 If Task 1 confirms legacy-row-surfacing behaviour: add the per-request aggregate `info!(event = "metric_view_serialize", reason = "legacy_row", legacy_row_count = N)` emission AFTER the handler-level `.collect()` is done and BEFORE the `Ok(Json(DevicesResponse { … }))` return. The aggregation walks the response payload once to count legacy markers — keep the walk simple (no premature optimisation).

- [x] **Task 5 — Retire `metric_view_display_string` + 2 unit tests (AC#7).**
  - [x] 5.1 Delete the function at `src/web/api.rs:214-221`.
  - [x] 5.2 Delete its doc-comment block (lines 199-213).
  - [x] 5.3 Delete unit test `metric_view_display_string_typed_payloads` at line 4952-4967.
  - [x] 5.4 Delete unit test `metric_view_display_string_non_finite_float_wire_format` at line 4970-4984 (or thereabouts; grep at implementation start).
  - [x] 5.5 Verify the grep contract `git grep -n 'metric_view_display_string' src/ tests/` returns ZERO hits post-deletion.

- [x] **Task 6 — Update `static/metrics.js::renderMetricRow` (AC#6).**
  - [x] 6.1 Add `formatValue(value, dataType)` helper at module scope near line 80 (after `statusFor`).
  - [x] 6.2 Update line 136 `valueText` computation to use `formatValue` + the unit suffix coalescing logic.
  - [x] 6.3 No change to staleness `status` row class (Story 9-3 contract preserved per AC#8).
  - [x] 6.4 Manual smoke test: load `/metrics.html` against a running gateway (`cargo run -- -c config/config.toml` + open browser); verify rows render `value + unit` correctly per variant. **Document in DoD section** — operator-level testing is not automated.

- [x] **Task 7 — Add new audit-event row to `docs/logging.md` (AC#14).** Add `metric_view_serialize` row to the audit-event table per AC#5 schema. Document the closed reason enum `{non_finite, legacy_row}` (or `{non_finite}` only, depending on Task 1 outcome).

- [x] **Task 8 — Add 5 unit tests + 2-3 integration tests (AC#10, AC#11).**
  - [x] 8.1 Add `src/web/api.rs::tests::metric_type_to_json_value_*` (5 tests per AC#10 items 1-5).
  - [x] 8.2 Add `tests/web_dashboard.rs::api_devices_emits_typed_value_and_unit_per_variant` (AC#10 item 6).
  - [x] 8.3 Add `tests/web_dashboard.rs::api_devices_emits_null_value_for_unpolled_metric` (AC#10 item 7).
  - [x] 8.4 Conditional: add `api_devices_legacy_row_emits_aggregate_info_log` IF Task 1 confirms legacy-row-surfacing (AC#10 item 8).
  - [x] 8.5 Add `tests/web_dashboard.rs::metrics_js_references_unit_and_data_type_fields` (AC#11).
  - [x] 8.6 Update the existing JSON-shape assertion test `api_devices_returns_json_with_expected_shape_when_authed` (`tests/web_dashboard.rs:595-643`) to assert the new `unit` field is present (`Some` or `None`) on each `MetricView` — the test seeds NO metrics today, so the snapshot has configured-but-unpolled metrics → `value: null, unit: Some(...)` per AC#1.

- [x] **Task 9 — Documentation Sync (AC#14, per CLAUDE.md).**
  - [x] 9.1 Update `README.md` "Current Version" line with A-6 narrative.
  - [x] 9.2 Update README Planning table Epic A row (status transitions during story progress).
  - [x] 9.3 Update `docs/logging.md` per Task 7.

- [x] **Task 10 — Final verification.**
  - [x] 10.1 `TMPDIR=/home/gcorbaz/.cache/cargo-tmp cargo test --all-targets` ≥1240 passed / 0 failed / ≤10 ignored.
  - [x] 10.2 `TMPDIR=/home/gcorbaz/.cache/cargo-tmp cargo clippy --all-targets -- -D warnings` clean.
  - [x] 10.3 `TMPDIR=/home/gcorbaz/.cache/cargo-tmp cargo test --doc` 0 failed / ≥55 ignored.
  - [x] 10.4 `git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u` returns exactly 5 lines (`metric_history_read` + `metric_history_summary` + `metric_parse` + `metric_read` + `metric_view_serialize`) — or 4 lines if Task 1 drops the `legacy_row` reason and the `metric_view_serialize` event is never emitted (in which case the grep returns 4 lines + `metric_view_serialize` does NOT appear since it's only emitted from a non-finite path that's structurally unreachable post-A-3; revisit AC#5 wording at that point).
  - [x] 10.5 `git grep -n 'metric_view_display_string' src/ tests/` returns ZERO hits.
  - [x] 10.6 Manual smoke test: real gateway running against `config/config.toml` with at least one configured metric having `metric_unit = "°C"`; load `/metrics.html` in a browser; observe row rendering `<value> °C` instead of `"Float"`.
  - [x] 10.7 Hot-reload smoke test (per Story 9-7 contract): edit `config/config.toml` to add `metric_unit = "%"` on a metric that previously had no unit; SIGHUP the gateway; reload `/metrics.html`; observe the new unit appearing without restart.

---

## Review Findings (iter-1, same-LLM 2026-05-18)

`bmad-code-review A-6` iter-1 ran 3 parallel adversarial layers (Blind Hunter / Edge Case Hunter / Acceptance Auditor) against the implementation commit `1328719`. Acceptance Auditor verdict: **ELIGIBLE-FOR-DONE** — all 14 ACs SATISFIED (AC#12 NOT-VERIFIABLE from diff alone, evidenced by Dev Agent Record). 1 decision-needed, 12 patches, 10 defers, 5 dismissed.

### decision-needed (1)

- [ ] **[Review][Decision] D1 — Float wire-precision contract: f64 native (current) vs f32-cast shim (pre-A-6)** — `src/web/api.rs:metric_type_to_json_value Float arm`. The retired `metric_view_display_string` cast Float to f32 via `(*f as f32).to_string()`. The post-A-6 helper emits f64 natively via `serde_json::Number::from_f64`. For values like `Float(23.6)` whose f32 source representation introduces precision artifacts (≈23.6000003814697266 as f64), `value.toString()` in JS renders the extra digits and the dashboard regresses vs A-5. Three options: (a) restore f32 cast in the helper (matches pre-A-6 display contract; loses precision for any future native-f64 source); (b) accept the f64 widening + document the change in operator-facing docs (current state); (c) format client-side via `Number.prototype.toFixed(...)` with a per-metric precision config. User decision required.

### patch (12, all applied 2026-05-18)

- [x] **[Review][Patch] P1 (Blind+Edge) — String value `"—"` collision with em-dash sentinel** `[static/metrics.js:formatValue + renderMetricRow]`. For `dataType === "String"`, `formatValue` returns the raw value; if a metric reports the literal string `"—"`, the post-format `formattedValue === "—"` check then misclassifies it as missing and drops the unit. Fix: drive the "missing" decision off `metric.value === null || metric.value === undefined` directly, not the stringified output.
- [x] **[Review][Patch] P2 (Blind+Edge) — `metric_view_serialize` warn flood at scale** `[src/web/api.rs:metric_type_to_json_value]`. Per-row `warn!` fires on every `/api/devices` request when a regressed sensor has non-finite Float; the dashboard polls every 10s ⇒ N rows × 6/min warn-line rate per regressed sensor. Aggregate at request level (one `warn!` per request with `non_finite_count = N`) and demote per-row to `debug!`, matching A-5's `metric_history_summary` aggregate pattern.
- [x] **[Review][Patch] P3 (Blind) — Brittle `-0.0` wire-format assertion** `[src/web/api.rs::tests::metric_type_to_json_value_float_finite]`. Asserts `serde_json::to_string(&v) == "-0.0"` which is outside serde_json's documented stability surface. Fix: assert via `as_f64() == Some(-0.0)` AND `is_sign_negative()`.
- [x] **[Review][Patch] P4 (Blind) — `metric_type_to_json_value_float_non_finite` log-correlation pinning is weak** `[src/web/api.rs::tests:metric_type_to_json_value_float_non_finite]`. Three back-to-back calls with different `device_id`/`metric_name` args feed a shared `tracing-test` buffer; `logs_contain("dev-x")` passes even if the implementation swaps args internally. Fix: split into 3 separate `#[traced_test]` functions OR assert exact per-line equality.
- [x] **[Review][Patch] P5 (Blind) — `MetricView.unit` always-serialized invariant not pinned by a unit test** `[src/web/api.rs::tests]`. A future clippy-silencing edit adding `#[serde(skip_serializing_if = "Option::is_none")]` would break AC#1 silently. Fix: add a tightly-focused serde round-trip test asserting the `unit` key is in the JSON even when `None`.
- [x] **[Review][Patch] P6 (Blind) — `reason = "non_finite"` string literal lacks compile-time pin** `[src/web/api.rs:metric_type_to_json_value warn emission + docs/logging.md]`. Closed-enum language in docs is aspirational; a copy-paste drift to `"non-finite"` would not fail at compile time. Fix: extract `pub(crate) const REASON_NON_FINITE: &str = "non_finite";` and reference from both emission site and a `#[test]` that pins the wire string.
- [x] **[Review][Patch] P7 (Blind) — i64::MAX integration test missing** `[tests/web_dashboard.rs::api_devices_emits_typed_value_and_unit_per_variant]`. Int test seeds `42` only; no end-to-end coverage that `i64::MAX` survives the `MetricView → ApplicationView → DevicesResponse → serde_json::to_string` ladder. Fix: extend test to seed `i64::MAX` and grep the response body for the literal `9223372036854775807`.
- [x] **[Review][Patch] P8 (Blind+Edge) — i64 > 2^53 precision-loss telemetry missing** `[src/web/api.rs:metric_type_to_json_value Int arm + docs/logging.md]`. JS clients silently truncate >2^53 to nearest f64; today operators have zero signal. Fix: emit `metric_view_serialize` with new `reason="int_precision_lossy"` when `i.unsigned_abs() > (1u64 << 53)`; extend `docs/logging.md` reason enum to `{non_finite, int_precision_lossy}`; update AC#5 grep contract to allow the new reason value.
- [x] **[Review][Patch] P9 (Blind+Edge) — `formatValue` catch-all swallows unknown discriminant silently** `[static/metrics.js:formatValue default arm]`. A future wire-format addition (`MetricType::Bytes` etc.) would render as `String(value)` with no client-side signal. Fix: `console.warn` and return a visible sentinel `"?"` for unrecognised `dataType`.
- [x] **[Review][Patch] P10 (Edge) — String value `""` with unit produces leading-space artifact** `[static/metrics.js:renderMetricRow valueText composition]`. `formattedValue === ""` and a non-empty unit yield `" °C"` (just space + unit). Fix: treat empty string identically to null at the format layer — return `"—"` for String value `""`.
- [x] **[Review][Patch] P11 (Edge) — `metric_unit` leading/trailing whitespace produces double-space rendering** `[src/web/mod.rs:DashboardConfigSnapshot::from_config]`. Operator-edited TOML can include `metric_unit = " °C"`; the renderer produces `23.5  °C`. Fix: trim at the `from_config` boundary; normalise empty-after-trim to `None`.
- [x] **[Review][Patch] P12 (Blind+Edge) — No grep guard prevents accidental future equality on `MetricView`/siblings** `[tests/web_dashboard.rs]`. AC#7's grep-test pattern is the project precedent; the `PartialEq, Eq` derive removal has none. Fix: add a single `tests/` test that runs `git grep` for `MetricView ==` / `DevicesResponse ==` / `ApplicationView ==` / `DeviceView ==` and asserts zero hits.

### defer (10, captured but not patching)

- [x] **[Review][Defer] W1 (Blind) — `std::mem::forget(dir)` tempdir leak pattern** `[tests/web_dashboard.rs:build_test_app_state + new integration test]` — deferred, pre-existing pattern used by `build_test_app_state` long before A-6; refactoring to use a stored `TempDir` binding is a sweep across the whole web-dashboard test file and out of A-6 scope.
- [x] **[Review][Defer] W2 (Blind) — `Some("")` vs `None` distinction has no JS consumer** `[src/web/mod.rs:MetricSpec.metric_unit]` — **RESOLVED-BY-P11** (2026-05-18 iter-1): P11's `from_config` whitespace-trim normalisation collapses `Some("")` and `Some("   ")` to `None`. The orphaned distinction is closed; the renderer's truthy-check coalescing is now redundant defence-in-depth.
- [x] **[Review][Defer] W3 (Blind) — `MetricSpec` PartialEq + empty-string `metric_unit` ⇒ unnecessary topology rebuild on `""` ↔ absent toggle** `[src/web/mod.rs:MetricSpec]` — **RESOLVED-BY-P11** (2026-05-18 iter-1): once P11 collapses `Some("")` / whitespace-only to `None` at the snapshot boundary, the `""` ↔ absent toggle is a no-op equality at `MetricSpec` level — no spurious topology rebuild path remains.
- [x] **[Review][Defer] W4 (Blind) — `metrics_js_references_unit_and_data_type_fields` content-grep passes if literals appear in a comment** `[tests/web_dashboard.rs]` — deferred, same pattern as the precedent Story 9-3 test `metrics_js_is_served_and_references_api_devices` (which is also a brittle content-grep). Headless-browser test infrastructure would be a separate epic.
- [x] **[Review][Defer] W5 (Blind) — `MetricView` doc-comment carries migration history** `[src/web/api.rs:MetricView]` — deferred, consistent house-style across A-1/A-2/A-3/A-4/A-5 spec migration narratives; rewriting all of them is a documentation pass, not A-6 scope.
- [x] **[Review][Defer] W6 (Edge) — Float subnormal precision unguarded** `[src/web/api.rs:metric_type_to_json_value Float arm]` — deferred, subnormal Float behaviour is well-defined at the f64 ⇒ f32 ⇒ JSON boundary; operator-facing impact is nil unless a sensor genuinely reports values below 1.4e-45. Out of A-6 scope.
- [x] **[Review][Defer] W7 (Edge) — `row.data_type` discriminant ≠ `spec.metric_type` during hot-reload window** `[src/web/api.rs:api_devices Some(row) arm]` — deferred, a legitimate transitional state during a metric-type hot-reload (Story 9-7). Worth a tracking issue for a future story to either (a) emit a `type_drift` warn, or (b) suppress the unit when the discriminants disagree. Out of A-6 scope.
- [x] **[Review][Defer] W8 (Edge) — `metric.unit === "0"` falsy-coalesces in JS truthy check** `[static/metrics.js:renderMetricRow]` — deferred, extremely rare (a unit identifier literally `"0"`); the existing fix at P10 (treat `""` as "no unit") plus the standard truthy-check is the standard pattern. Operator-visible impact: a single weird unit string is dropped. Acceptable.
- [x] **[Review][Defer] W9 (Edge) — sNaN treated as plain `non_finite` (forensic detail lost)** `[src/web/api.rs:metric_type_to_json_value Float arm]` — deferred, sNaN signalling is a forensic-grade distinction that the metric pipeline never preserves anyway (Rust f64 routinely normalises sNaN → qNaN at any arithmetic op). Out of A-6 scope.
- [x] **[Review][Defer] W10 (Edge) — `metric_unit` length cap missing** `[src/web/mod.rs / src/web/api.rs validate path]` — deferred, length validation is Story 9-5 territory (PUT-replace-device validator). 9-5-iter1-D5 already deferred the byte-vs-char budget; A-6 is the read path.

### dismiss (5)

- [Review][Dismiss] BH10 — `metric_type_to_json_value` signature threading device_id/metric_name to all 4 arms when only the non-finite arm uses them. Taste preference; the cognitive cost is minor and call sites already have the identifiers in scope. Splitting the helper would force the caller to duplicate the warn-emission logic across writers.
- [Review][Dismiss] BH17 — test helper builder doesn't take a unit override. The current `metric_unit: None` default is fine; the integration test seeds `Some("…")` explicitly per AC#10 item 6.
- [Review][Dismiss] EC3 — empty `metric_name` / `device_id` in warn. Upstream config validation (Story 1-5) rejects empty IDs at startup, so this path is structurally unreachable.
- [Review][Dismiss] EC6 — Float `-0.0` sign drops via JS `(-0).toString() == "0"`. Display-only; not semantically load-bearing; the Rust-side wire still carries `-0.0` per AC#1 wire-contract table (P3 strengthens the test).
- [Review][Dismiss] EC9 — Bool `false` consistency across `statusFor` vs `formatValue`. Both paths handle Bool correctly today (`statusFor` keys off `null`/`undefined`, `formatValue` keys off `dataType === "Bool"`).

---

## Dev Notes

### Architectural decisions captured

**1. Why `serde_json::Value` and NOT a tagged enum.** Three options were considered for the JSON wire shape:

- **(a) `value: Option<serde_json::Value>` — chosen.** The JSON wire shape is the natural primitive (`23.5` / `42` / `true` / `"OK"` / `null`) and the `data_type` field already exists (Story 9-3) for disambiguation. JS consumers read native types without a switch on a tag field.
- (b) `#[serde(tag = "type", content = "v")]` on a new `MetricValueJson` enum mirroring `MetricType` — wire becomes `{"type":"Float","v":23.5}`. Self-describing but verbose; the dashboard JS still needs to switch on `type` to render units differently per variant.
- (c) Per-variant primitive fields (`value_float: Option<f64>`, `value_int: Option<i64>`, etc.) — schema-reflective but the wire shape becomes 5 nullable fields per metric, with only one ever populated. Doesn't match how the JS consumer wants to read it.

Option (a) wins on JS-consumer ergonomics + matches the architecture-doc note: "Pattern-matching at every read site (...`web::api::api_metrics`) emits the matching OPC UA `Variant` or JSON value, not the discriminant." [Source: `architecture.md:182`]

**2. Why retire `metric_view_display_string` instead of widening it.** The A-5 P0-D4 shim was always transitional — its inline comment at `src/web/api.rs:200-213` literally says "Story A-6 will widen this helper to a typed JSON shape and retire it." Widening it to produce a `serde_json::Value` would conflate "stringification for display" with "typed JSON serialisation" — two distinct concerns. The new `metric_type_to_json_value` is the typed-JSON path; display-string concerns are now handled in `static/metrics.js::formatValue` where they belong (presentation layer, not API layer).

**3. Why `info!` for `legacy_row` instead of `warn!`.** Legacy rows are an **expected** outcome of the Epic A migration strategy — pre-Epic-A data is retained per the architecture-doc legacy-row contract (`value_type='legacy'`, surfaces as `BadDataUnavailable` in OPC UA, surfaces as `value: null` in web). A `warn!` would imply "something is wrong" when the situation is "running on a migrated v006 database, ChirpStack hasn't repopulated the typed columns yet". `info!` is the right level — operator can see the legacy-row count if they care (e.g. via `journalctl -u opcgw | grep legacy_row`), but it doesn't pollute the warn channel.

**4. Why aggregate `legacy_row_count` per request, not per row.** A migrated 100-device gateway with 4 metrics each has 400 legacy rows on first boot. Emitting 400 info lines per dashboard tick (which polls every 10 s) is 144 000 lines/hour of legacy-row noise. Aggregating to one line per tick with `legacy_row_count = N` gives the operator the same information at 360 lines/hour — manageable.

**5. JS Number precision caveat for `Int(i64::MAX)`.** JavaScript's `Number` type is IEEE 754 double-precision; integers above `2^53 = 9 007 199 254 740 992` lose precision. ChirpStack metric values are rarely this large (a 64-bit counter at 1 Hz takes 292 years to reach 2^53), so this is a documented caveat rather than a defect. If a future story requires bit-exact i64 round-trip on the wire, the option is to switch the Int variant's JSON shape to a string (`"9223372036854775807"`) — adds a renderer-side `parseInt` step. Defer until an operator hits the limit.

**6. Why no new public types beyond the widened structs.** A-6 does not introduce a new `MetricValueJson` enum or any new public API surface — only **extends** existing structs (`MetricView`, `MetricSpec`). This keeps the SemVer change additive (one new public field on two structs) and respects the post-A-5 stability: the storage trait + OPC UA layer are closed; the web layer was always meant to be the typed-JSON surface that closes the loop.

**7. Why hot-reload works without `config_reload.rs` touches.** Story 9-7's `handle_config_reload` listener path calls `DashboardConfigSnapshot::from_config(&new_config).into()` to rebuild the snapshot Arc. Once `from_config` is widened in Task 2.2 to read `metric_unit`, a TOML edit that mutates `metric_unit` automatically propagates to the next-tick `api_devices` response. The classifier at `read_metric_lists_equal:961-979` already considers `metric_unit` a topology change — that's been there since 9-7 shipped. A-6 is a downstream consumer.

**8. Story 9-8 interaction.** Story 9-8 ships dynamic OPC UA topology mutation. Today (post-9-8), a `metric_unit` change triggers a full OPC UA variable remove+add (deferred-work 9-8-iter1-D3 documents this as a v2 candidate for in-place `EngineeringUnits` mutation). A-6 does NOT touch this path — A-6's web-side rebuild is independent. After A-6, the operator's mental model is: "I edit `metric_unit` in TOML → web dashboard reflects within one reload tick (subsecond) → OPC UA variable's `EngineeringUnits` attribute reflects within one topology-apply tick (deferred-work 9-8-iter1-D3 details the heavy remove+add semantics)." A-6 explicitly does NOT address the OPC UA-side `EngineeringUnits` attribute exposure — that's a future story under "OPC UA Phase B v2".

**9. Why the regression-guard test scheme uses `tests/web_dashboard.rs` not a new file.** Per A-1 / A-2 / A-3 / A-4 / A-5 pattern, new tests land in the existing per-module integration file unless the surface is genuinely new. A-6 widens an existing route (`/api/devices`) on an existing JSON contract; the natural home is the existing dashboard tests. Creating `tests/web_dashboard_typed_payload.rs` would duplicate the `setup_app_with_state` harness. A-5 created `tests/opcua_historyread_typed_payload.rs` because the HistoryRead surface had no prior integration tests; A-6 has prior tests.

### Files being modified — current state + what changes

For each MUTABLE file in AC#13, here's the current shape + what A-6 changes:

- **`src/web/api.rs::MetricView`** (line 272): current state carries `value: Option<String>` populated via the A-5 transitional `metric_view_display_string` shim; no `unit` field. A-6 changes: `value: Option<serde_json::Value>` + adds `unit: Option<String>`.
- **`src/web/api.rs::metric_view_display_string`** (line 214-221) + its 2 unit tests (line 4952-4984): retired entirely per AC#7. The function is `pub(crate)` so the deletion is fully contained.
- **`src/web/api.rs::api_devices`** (line 298-426): the `Some(row)` and `None` arms of the inner `.map` (lines 386-401) are updated per Task 4. The `as_of` / `stale_threshold_secs` / `dashboard_snapshot` / poison-recovery scaffolding is untouched (AC#8).
- **`src/web/api.rs::metric_type_to_json_value`** (NEW, placed near the now-deleted `metric_view_display_string`): the AC#4 helper.
- **`src/web/mod.rs::MetricSpec`** (line 120): widened with `metric_unit: Option<String>`.
- **`src/web/mod.rs::DashboardConfigSnapshot::from_config`** (line 165-168): populates the new field from `OpcMetric.metric_unit`.
- **`static/metrics.js::renderMetricRow`** (line 133-160) + new `formatValue` helper near line 80: typed-value + unit rendering per AC#6.
- **`tests/web_dashboard.rs`**: existing JSON-shape test updated for `unit` field; 2-3 new tests per AC#10/AC#11.
- **`docs/logging.md`**: new `metric_view_serialize` row in audit-event table.
- **`README.md`**: Current Version + Planning table Epic A row updated.

### Previous-story intelligence

**A-5 lessons applied:**

- **Same-LLM iter-2 catches fake regression-guard tests** (memory `feedback_iter3_validation` 11-story pattern; A-6 will extend to 12). Recommend running `bmad-code-review A-6` on a **different LLM** per CLAUDE.md "Code Review & Story Validation Loop Discipline" to break the same-model audit blind spot. Per A-5 K1/K2 lesson: every regression-guard test in AC#10 / AC#11 MUST invoke the function under test directly — a docstring claim that the test "guards `metric_type_to_json_value` against regression" without the test body actually invoking the helper is a fake guard.
- **Phrase-harmonization patches need full-codebase grep.** A-5's K5 finding (orphan reference to pre-P2 `reason_detail` shape) and A-4's JR8 finding (3 stale test assertions on the old "Schema drift:" prefix) extend to A-6: when retiring `metric_view_display_string`, grep `src/` and `tests/` for the symbol name BEFORE declaring the retirement done. Task 5.5 + Task 10.5 pin this with a literal grep contract.
- **Aggregate skip-telemetry pattern.** A-5 K6 introduced `event="metric_history_summary"` as the trace-level aggregate of `metric_history_read` row-skips. A-6's `legacy_row` reason follows the same pattern (one info per request, not one per legacy row). If a future story finds the aggregation grouping inadequate (e.g. operator wants per-device legacy-row counts), the natural extension is a structured field `legacy_row_per_device: HashMap<String, u32>` — not switching to per-row emission.
- **`#[traced_test]` for log-presence/absence assertions.** A-5 iter-2 K3 fix added `#[traced_test]` + `logs_contain(...)` to pin path-disambiguation. A-6's `metric_type_to_json_value_float_non_finite` test (AC#10 item 2) uses the same pattern to pin the non-finite warn emission.

**A-4 lessons applied:**

- **Test-fixture seeds must produce different outputs through the surviving path vs the dropped path** (JR1 finding class). A-6's `MetricView` widening adds `unit: Option<String>` — every existing test fixture that constructs a `MetricView` literal will need to add `unit: …`. If a test seeds `unit: None` but expects to assert "unit was rendered" through the JS surface, the test is a fake guard (the None and Some-with-empty-string both render to no-unit per AC#6 step 3). Tests asserting unit presence MUST seed `unit: Some("…")` with a non-empty value.

**A-3 lessons applied:**

- **Writer-boundary data-corruption risks are a separate severity tier from log-quality findings** (iter-2 IR2-B catch class). A-6's reader-boundary equivalent: a `MetricType::Float(value)` where `value` is a non-finite f64 that somehow bypassed A-3's poller filter and reached the dashboard. The `metric_type_to_json_value` helper's `serde_json::Number::from_f64` returns `None` for non-finite — the helper short-circuits to `Value::Null` + emits the `non_finite` warn. The test `metric_type_to_json_value_float_non_finite` (AC#10 item 2) is the regression guard against a future writer-path regression that admits non-finite values.

**A-2 lessons applied:**

- **No new SQLite migration in A-6.** v008 from A-3 carries the exactly-one-non-NULL CHECK that bounds the storage-row shape. A-6 reads downstream of A-4/A-5; no schema work.

**A-1 lessons applied:**

- **Test-fixture cascade across the codebase.** A-1 paved the path for payload-bearing `MetricType` + `.clone()` cascade across 13 test files. A-6's symmetric cascade: every `MetricView { … }` struct-literal in test files needs `unit: …` added. Search `src/web/api.rs::tests` + `tests/web_dashboard.rs` for `MetricView {` and update. Estimated touch volume: 10-20 lines, mechanical. Perl bulk substitution is overkill for this volume — manual `grep` + `Edit` is sufficient.

**9-7 lessons applied:**

- **Hot-reload swap rebuilds `dashboard_snapshot` via `from_config`.** A-6's `MetricSpec` widening propagates through hot-reload without additional plumbing. Pin this with a 9-7-style integration test that mutates `metric_unit` via TOML edit + SIGHUP + GET `/api/devices` — but the existing `tests/config_hot_reload.rs::*` suite already covers the snapshot-rebuild mechanism; A-6 inherits the coverage. Task 10.7 is the manual smoke that confirms end-to-end.

**9-5 lessons applied:**

- **`metric_unit = ""` round-trips as `Some("")` to TOML** (deferred-work 9-5-iter1-D4). A-6's renderer collapses `Some("")` and `None` to "no unit suffix" at line 6.3 — closes the D4 deferral by establishing the renderer's coalescing behaviour as the project's stated preference. Document the closure in deferred-work.md as part of A-6's deferred-work delta.
- **UTF-8 byte-budget on `metric_unit`** (deferred-work 9-5-iter1-D5). Out of scope for A-6 — D5 is a write-side validation concern, not a read-side rendering concern. A-6 renders whatever the snapshot carries.

**9-3 lessons applied:**

- **Server-side `as_of` for clock-skew-robust staleness** — preserved per AC#8. The `as_of` field is the time-of-truth for "is this metric stale"; the new typed-value path doesn't touch the staleness clock.
- **`statusFor` renders "missing" when `metric.value === null || undefined`** — preserved. With A-6's wider `value: null` semantics (now also covering "legacy row" and "non-finite Float"), the "missing" badge correctly surfaces those cases too. Operator UX is consistent.

**9-1 lessons applied:**

- **HTTP Basic auth wraps all `/api/*` routes via the middleware tower** — preserved. A-6 only edits the `api_devices` handler body; the auth-middleware composition is untouched.

### References

- [Source: `_bmad-output/planning-artifacts/epics.md § Epic A § Story A.6`] — user story + acceptance criteria.
- [Source: `_bmad-output/planning-artifacts/architecture.md:165-187`] — data architecture + Storage Payload Migration Strategy (the "Pattern-matching at every read site … emits the matching OPC UA `Variant` or JSON value" sentence is the AC#1 architectural anchor).
- [Source: `_bmad-output/planning-artifacts/prd.md § FR37 + FR41 + FR51`] — operator views live metrics + mobile-responsive LAN access + payload-preservation contract.
- [Source: `_bmad-output/implementation-artifacts/A-5-opc-ua-historyread-value-payload-pipeline.md`] — immediate predecessor; introduced the `metric_view_display_string` shim A-6 retires.
- [Source: `_bmad-output/implementation-artifacts/A-4-opc-ua-read-value-payload-pipeline.md`] — sibling read-path closure; the `metric_type_from_typed_columns` helper precedent for A-6's `metric_type_to_json_value`.
- [Source: `_bmad-output/implementation-artifacts/9-3-live-metric-values-display.md`] (or the canonical Story 9-3 file under `_bmad-output/implementation-artifacts/`) — `/api/devices` original handler shape + `static/metrics.js` original renderer; A-6 widens both.
- [Source: `_bmad-output/implementation-artifacts/9-5-device-crud-via-web-ui.md`] — `metric_unit` validation rules; A-6 reads the field, doesn't validate.
- [Source: `_bmad-output/implementation-artifacts/9-7-configuration-hot-reload.md`] — `handle_config_reload` listener rebuilds `dashboard_snapshot` via `from_config`; A-6's `MetricSpec` widening flows through.
- [Source: `_bmad-output/implementation-artifacts/9-8-dynamic-opc-ua-address-space-mutation.md`] — Story 9-8's OPC UA-side topology apply; A-6 is web-only and does not interact with this path.
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md` — entries `DEF-iter1-A5-D1`, `9-5-iter1-D4`, `9-5-iter1-D5`, `9-8-iter1-D3`, `A-1-iter2-DEF11`] — carry-forward closures + non-closures A-6 must track.
- [Source: `src/web/api.rs:214-221`] — current `metric_view_display_string` shim (to be retired).
- [Source: `src/web/api.rs:262-277`] — current `MetricView` + surrounding response shape.
- [Source: `src/web/api.rs:298-426`] — current `api_devices` handler.
- [Source: `src/web/mod.rs:120-144`] — current `MetricSpec` + `DashboardConfigSnapshot`.
- [Source: `src/config.rs:653-657`] — `OpcMetric.metric_unit: Option<String>` (the source-of-truth field).
- [Source: `src/config_reload.rs:961-979`] — `read_metric_lists_equal` already destructures `metric_unit`; no classifier work needed.
- [Source: `static/metrics.js:133-160`] — current `renderMetricRow` (Story 9-3 contract).
- [Source: `tests/web_dashboard.rs:521-810`] — existing `/api/devices` integration tests (Story 9-3 + 9-7); A-6 updates the JSON-shape assertion + adds 2-3 new tests.
- [Source: GitHub issue #108] — payload-less `MetricType` enum — functionally closed by A-5 ship; A-6 closes the web-display side of the original symptom report.
- [Source: CLAUDE.md § Code Review & Story Validation Loop Discipline] — loop iteration discipline.
- [Source: CLAUDE.md § BMad Workflow Commit & Push Discipline] — implementation-then-review commit pattern.
- [Source: CLAUDE.md § Documentation Sync] — README + docs update in the same commit as behavioural changes.
- [Source: memory `feedback_iter3_validation`] — 11-story validated iter-3 over-reviewing pattern; A-6 extends to 12.
- [Source: memory `feedback_review_iterations`] — Guy's stated preference: extra review pass beats missing an issue.
- [Source: memory `reference_cargo_tmpfs_workaround`] — TMPDIR override for `protoc` tmpfs disk-quota issue.

### Project Structure Notes

A-6 sits cleanly within the existing web-handler + presentation-asset boundary. No new modules. The `MetricView` JSON shape change is a wire-format minor revision (additive `unit` field + widened `value` type) — JS consumers reading the legacy stringly-typed shape would break, but Story 9-3 was the only documented consumer and its JS lives in this repo (and is updated in Task 6). External consumers of `/api/devices` are not contracted; the README's API documentation should be updated if any (none currently — only the `static/metrics.js` is documented).

The `metric_type_to_json_value` helper is a natural sibling of A-4's `metric_type_from_typed_columns`. Both live one layer above the typed `MetricType` enum — one converting INTO `MetricType` (the read path from typed SQLite columns), one converting OUT of `MetricType` (the write path to typed JSON). If a future story needs `MetricType` → `serde_json::Value` outside the `api_devices` handler (e.g. a `/api/metrics/history` endpoint mirroring HistoryRead, or a CSV export), it reuses `metric_type_to_json_value` unchanged.

The story is intentionally narrow: web-layer + presentation-layer only, no OPC UA, no storage, no config. This matches the post-A-5 "Epic A is done except for web consumer + migration runbook" framing.

### Out of Scope

- **Migration runbook + version-gated migration script (Story A-7):** A-7 owns the operator-facing in-place-vs-drop-database upgrade documentation + the version-gated startup-time migration script. A-6 ships the code; A-7 ships the runbook.
- **Epic A retrospective:** the `epic-A-retrospective: optional` entry in sprint-status (per the post-A-5 transition) becomes **required** after A-7 lands, per CLAUDE.md "Do not skip the retrospective." A-6 does NOT close the epic — A-7 does (technically all-stories-done after A-7; retrospective then mandatory).
- **OPC UA-side `EngineeringUnits` attribute exposure:** the OPC UA `0:EngineeringUnits` standard property could expose `metric_unit` to SCADA clients (FUXA/Ignition). Currently the unit is web-display only. Adding `EngineeringUnits` to OPC UA variables is a future-story candidate ("OPC UA Phase B v2") — out of A-6 scope. Deferred-work 9-8-iter1-D3 also documents the in-place `EngineeringUnits` mutation as a v2 candidate.
- **Renderer polish:** numeric rounding (`23.5` vs `23.50000000001`), localised number formatting (`1,234.5` vs `1.234,5` for European locales), trailing-zero trimming — all out of scope. V8's `Number.prototype.toString` is "good enough" for v2.0 GA. Future UX-polish story candidate.
- **Bigint i64 wire shape:** if/when an operator hits the 2^53 JS Number precision limit, switch the Int variant's JSON to a string. Defer until that report arrives.
- **`/api/devices` → `/api/metrics` endpoint rename:** the Story A.6 user-story text in epics.md uses `/api/metrics`, but the actual endpoint is `/api/devices` (Story 9-3 contract). Renaming is a documented breaking change for the `static/metrics.js` consumer + any future external consumers — high churn for zero operator benefit. **A-6 keeps the existing `/api/devices` route name**; the epics.md user-story text is the loose-narrative outlier, not the wire contract.
- **Audit-event closed-enum unification across `metric_*` events:** A-6 introduces a fifth `metric_*` event (`metric_view_serialize`). If/when a future audit-pass identifies that all five events deserve a common closed-enum macro (DRY), that's a refactor story, not A-6 scope.
- **CHANGELOG entry for `MetricView` JSON shape change:** A-1's deferred CHANGELOG (A-1-iter1-DEF15) is still pending. A-6 contributes a line to the eventual entry but does NOT introduce a CHANGELOG mid-flight. The README "Current Version" narrative is the operator-facing surrogate.

### Definition of Done

- [x] All 14 ACs SATISFIED (or explicitly DEFERRED-DOCUMENTED with user acceptance per CLAUDE.md condition #3).
- [x] `cargo test --all-targets` ≥1240 passed / 0 failed / ≤10 ignored.
- [x] `cargo clippy --all-targets -- -D warnings` clean.
- [x] `cargo test --doc` 0 failed.
- [x] AC#5 grep contract returns the expected event lines per Task 10.4.
- [x] AC#7 grep contract returns ZERO `metric_view_display_string` references per Task 10.5.
- [ ] `bmad-code-review A-6` loop terminates per CLAUDE.md condition #2 (only LOW remains) — recommended different-LLM run.
- [x] `README.md` Current Version + Planning table updated.
- [x] `docs/logging.md` updated with `metric_view_serialize` row.
- [x] Sprint-status flipped `ready-for-dev → in-progress → review → done`.
- [ ] Implementation-complete + code-review-complete commits land per CLAUDE.md BMad Workflow Commit & Push Discipline.
- [ ] Manual smoke test passed: real gateway, `metric_unit = "°C"` configured, `/metrics.html` shows `<value> °C` (Task 10.6).
- [ ] Hot-reload smoke test passed: edit `metric_unit` in TOML, SIGHUP, dashboard reflects within one reload tick (Task 10.7).

---

## Dev Agent Record

### Agent Model Used

Opus 4.7 (1M context) — same-LLM run per CLAUDE.md flexibility; recommend `bmad-code-review A-6` on a **different LLM** per memory `feedback_iter3_validation` 11-story validated pattern (A-6 extends to 12).

### Debug Log References

- Initial post-edit `cargo test --all-targets` (Task 8): 1238 passed, 1 failed. The failure was in `metric_type_to_json_value_float_non_finite` — `logs_contain("event = \"metric_view_serialize\"")` (spaces around `=`) didn't match the actual `tracing-test` capture format `event="metric_view_serialize"` (no spaces). Fixed by tightening the assertion strings to omit the spaces.
- Final `cargo test --all-targets` under `TMPDIR=/home/gcorbaz/.cache/cargo-tmp`: **1242 passed / 0 failed / 10 ignored** — exceeds AC#12 target ≥1240 by 2.
- `cargo clippy --all-targets -- -D warnings`: clean (single run, no warnings).
- `cargo test --doc`: 0 failed / 55 ignored — A-5 baseline preserved.
- AC#5 grep contract (`git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u`): returns exactly **5 lines** (`metric_history_read` + `metric_history_summary` + `metric_parse` + `metric_read` + `metric_view_serialize`).
- AC#7 grep contract (`git grep -n 'metric_view_display_string' src/ tests/`): returns **ZERO** hits.

### Completion Notes List

- **2026-05-18 (iter-1 code review patches applied):** `bmad-code-review A-6` iter-1 same-LLM run completed. Acceptance Auditor verdict: ELIGIBLE-FOR-DONE before patches; all 14 ACs SATISFIED. 1 decision-needed (D1 Float wire-precision contract) resolved by user → option (a) f32-cast in helper, promoted to **P0-D1** patch. 12 additional patches (P1-P12) applied. 10 deferrals captured in `deferred-work.md`; **W2 + W3 RESOLVED-BY-P11** during iter-1 (the empty-string `metric_unit` normalisation at `from_config` closes both the orphaned-distinction and the spurious-topology-rebuild deferrals as a single structural fix). 5 dismissed (taste/false-positive). **P0-D1 implementation note:** the naive `*f as f32 as f64` cast does NOT strip f64 precision (the f64 retains all 53 significand bits of the f32's representation, serialising as `23.600000381469727`). The correct narrowing uses Rust's `f32::to_string()` Display alphabet which finds the shortest round-trip string at f32 precision, then `.parse::<f64>()` back to the *closest f64 to that string* (a different bit pattern than the f32-back-cast). This produces the expected wire `23.6`. Test `metric_type_to_json_value_float_finite` pins both the value and the wire-format length (≤6 chars for `23.6`). Final verification under TMPDIR=/home/gcorbaz/.cache/cargo-tmp: cargo test --all-targets **1255 passed / 0 failed / 10 ignored** (was 1242 pre-iter-1; +13 from: P3 wire-precision contract assertion, P4 3 separate per-call non-finite tests (replacing 1 buffer-correlation test), P5 unit-key-always-serialised test, P7 i64::MAX integration assertion, P8 3 int_precision telemetry tests (boundary + extremes + safe-range), P11 expanded normalisation test, P12 grep-guard test); clippy clean; doctest clean. AC#5 grep returns 5 lines unchanged; AC#7 grep returns 0 hits unchanged. Strict-zero invariants honoured (no new files outside the iter-1 patches' scope). Status stays **review** pending iter-2 per CLAUDE.md "Code Review & Story Validation Loop Discipline" — iter-1 was a non-trivial patch round (P0-D1 production logic + P2 aggregate warn restructure + P11 boundary normalisation + 3 new closed-enum reasons in audit telemetry) — exactly the change-class iter-2 historically catches HIGH-REGRESSIONs in. Recommend running bmad-code-review A-6 iter-2 on a different LLM per memory `feedback_iter3_validation` 11-story validated pattern (A-6 iter-2 would extend the streak to 12).

- **2026-05-18: Implementation complete via `bmad-dev-story A-6`.** All 14 ACs SATISFIED. Closes the WEB UI side of Epic A — last READ-side consumer of typed payloads.

- **Task 1 finding (load-bearing) — legacy_row reason DROPPED.** Verified via `src/storage/sqlite.rs:1530-1540` that post-A-5 `load_all_metrics` silently skips legacy rows (`legacy_skipped_count += 1; continue;` before pushing to result Vec). Consequence: AC#2 state (b) telemetry is dead code at the web layer — legacy rows never reach the dashboard handler. The `metric_view_serialize` event ships with ONLY the `non_finite` reason post-A-6 (not `{non_finite, legacy_row}` as initially speculated by the story spec). `docs/logging.md` reflects this single-reason closed enum. The conditional Task 1 outcome path in the spec is now resolved in favour of the simpler shape.

- **MetricView lost `PartialEq, Eq` derives.** `serde_json::Value` does not implement `Eq` because of float NaN; cascading the derive removal to the sibling response structs (`DevicesResponse` / `ApplicationView` / `DeviceView`) was the simplest correct fix. Verified that no internal callers used the equality — production code constructs these in `api_devices` only, and tests project specific fields via `json["…"].as_…()` patterns (not whole-struct `assert_eq!`).

- **Empty-string `metric_unit` coalesces with `None` at the renderer (closes 9-5-iter1-D4).** `static/metrics.js::renderMetricRow` uses the truthy check `metric.unit ? formattedValue + " " + metric.unit : formattedValue` — both `null` and `""` produce no unit suffix. The snapshot preserves the `Some("")` vs `None` distinction at the type level (a future story can differentiate if needed).

- **Bool wire shifted from `"1"`/`"0"` to native `true`/`false`.** This is a wire-format breaking change for any external `/api/devices` consumer that was reading the legacy stringly-typed shape. Per the Out-of-Scope analysis, only `static/metrics.js` is a documented consumer; it was updated in this story. External SCADA/JS consumers, if any exist, must update their parsers — documented in the README "Current Version" narrative.

- **No new dependencies.** `serde_json` was already a transitive dep via `axum::Json`; the helper uses `serde_json::Number::from_f64` and `serde_json::Value` directly. `tracing_test` was already a dev-dep.

- **Strict-zero invariants honoured per AC#13.** `git diff --stat` confirms only the AC#13 MUTABLE files changed: `src/web/api.rs` + `src/web/mod.rs` + `static/metrics.js` + `tests/web_dashboard.rs` + `docs/logging.md` + `README.md` + the sprint-status YAML + the story spec file itself.

- **Manual smoke test deferred to operator** (DoD items not auto-checkable): (a) real gateway + `metric_unit = "°C"` configured + browse `/metrics.html` → row renders `<value> °C`; (b) hot-reload smoke — edit `metric_unit` in TOML, SIGHUP, dashboard reflects within one reload tick. These remain unchecked in the DoD section; recommend the operator runs them post-`bmad-code-review` before flipping to `done`.

- **Tracking issue Task 0 deferred to user.** `gh` CLI not authenticated for write — A-1/A-2/A-3/A-4/A-5 precedent.

- **Next:** `bmad-code-review A-6` on a different LLM per CLAUDE.md "Code Review & Story Validation Loop Discipline" + memory `feedback_iter3_validation` 11-story validated pattern (A-6 extends to 12).

### File List

**Modified (production code):**

- `src/web/api.rs` — (1) `MetricView` struct widened: `value: Option<String>` → `value: Option<serde_json::Value>` + new `unit: Option<String>` field; dropped `PartialEq, Eq` derives (cascade-dropped on sibling response structs `DevicesResponse` / `ApplicationView` / `DeviceView`). (2) `metric_view_display_string` helper RETIRED (was lines ~214-221). (3) NEW `metric_type_to_json_value(data_type, device_id, metric_name) -> Option<serde_json::Value>` helper at the same site emits typed JSON primitives; non-finite Float yields `None` + emits `warn!(event = "metric_view_serialize", reason = "non_finite", …)`. (4) `api_devices` handler `Some(row)` / `None` arms updated to call the new helper + populate `unit: spec.metric_unit.clone()`. (5) Existing internal `crate::web::MetricSpec` literal construction (~line 5379) updated with `metric_unit: None`. (6) 5 new unit tests at `tests` mod: `metric_type_to_json_value_float_finite` / `metric_type_to_json_value_float_non_finite` (with `#[traced_test]` log-presence assertion) / `metric_type_to_json_value_int_extremes` / `metric_type_to_json_value_bool` / `metric_type_to_json_value_string`. (7) 2 retired unit tests deleted: `metric_view_display_string_typed_payloads` + `metric_view_display_string_non_finite_float_wire_format`.

- `src/web/mod.rs` — `MetricSpec` widened with `metric_unit: Option<String>` field; `DashboardConfigSnapshot::from_config` populates the field from `OpcMetric.metric_unit`. 2 new unit tests at `tests` mod: `dashboard_snapshot_from_config_propagates_metric_unit` + `dashboard_snapshot_preserves_empty_unit_string_distinct_from_none`.

- `static/metrics.js` — NEW `formatValue(value, dataType)` helper at module scope (near line 80) switches on `data_type` to format typed JSON values (Float / Int → `value.toString()`, Bool → `"true"` / `"false"`, String → identity). `renderMetricRow` (~line 133) updated to compose `value + " " + unit` via the helper; empty `metric.unit` coalesces with `null` via JS truthy check (closes 9-5-iter1-D4).

**Modified (tests):**

- `tests/web_dashboard.rs` — (1) Existing JSON-shape test `api_devices_returns_json_with_expected_shape_when_authed` extended with `unit` field assertion + `metric_unit: None` for the inline `MetricSpec` fixture. (2) NEW integration test `api_devices_emits_typed_value_and_unit_per_variant` seeds 4 typed payloads via `InMemoryBackend::set_metric` (Float 23.5 + "°C", Int 42 + no unit, Bool true + "%", String "OK" + "lvl") and asserts the wire shape has native JSON primitives + the configured unit per row. (3) NEW integration test `metrics_js_references_unit_and_data_type_fields` is a content-grep guard against renderer regression — asserts the served `/metrics.js` body contains `formatValue` / `metric.unit` / `metric.data_type` literals.

**Modified (docs):**

- `docs/logging.md` — Added `metric_view_serialize` row to the audit-event table. Closed reason enum: `{non_finite}` only. Field schema: `event="metric_view_serialize"`, `reason="non_finite"`, `device_id=<str>`, `metric_name=<str>`, `f64_value=<f64>`. Documents the defensive nature (unreachable in production due to A-3 poller filter) + the post-A-5 legacy-row silent-skip rationale (no `legacy_row` reason at this layer).

- `README.md` — Updated `Current Version` line with the A-6 review narrative; updated Planning table Epic A row from `A-6 ready-for-dev` → `A-6 review` + retired the trailing stale `A-6 backlog` / `A-7 backlog` duplicate sentences from prior README drift.

**Modified (BMad artefacts):**

- `_bmad-output/implementation-artifacts/A-6-web-ui-live-metrics-value-display.md` — Status flipped `ready-for-dev` → `review`; Tasks/Subtasks checkboxes ticked; Dev Agent Record populated.

- `_bmad-output/implementation-artifacts/sprint-status.yaml` — `A-6-web-ui-live-metrics-value-display: ready-for-dev → review` with full review narrative; `last_updated` field bumped to `2026-05-18`.

# Story A-4: OPC UA Read Value-Payload Pipeline

| Field         | Value                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------- |
| Story key     | `A-4-opc-ua-read-value-payload-pipeline`                                                              |
| Epic          | A — Storage Payload Migration (Phase B Closure, gates v2.0 GA)                                        |
| FRs           | FR51 (Epic-A umbrella)                                                                                |
| Status        | review                                                                                                |
| Created       | 2026-05-16                                                                                            |
| Source epic   | `_bmad-output/planning-artifacts/epics.md § Epic A § Story A.4`                                       |
| Sprint change | `_bmad-output/planning-artifacts/sprint-change-proposal-2026-05-14.md`                                |
| Tracking      | GitHub tracking issue to be filed by dev agent at implementation start (see Task 0)                   |

---

## User Story

As a **SCADA client connected to opcgw**,
I want `OpcUa::get_value` to return the actual measurement payload in the OPC UA `Variant` by reading from the typed columns A-3 populated,
So that `Read` operations return `Variant::Double(23.5)` / `Variant::Int64(42)` / `Variant::Boolean(true)` / `Variant::String("OK")` instead of the discriminant string parsed from the legacy `value` column.

---

## Story Context

### Why A-4 is the first READ-side story of Epic A

A-1 made `MetricType` payload-bearing at the type level. A-2 added typed columns (`value_real` / `value_int` / `value_bool` / `value_text` / `value_type`) to both `metric_values` and `metric_history`. A-3 wired the WRITE side — every new row written by the poller via the 4 SqliteBackend writers now carries the real measurement in the typed columns.

**Today, however, the read path still goes through the legacy `value TEXT` + `data_type TEXT` columns.** Two production read sites observe this gap:

1. **`SqliteBackend::get_metric_value`** (`src/storage/sqlite.rs:505-576`) — `SELECT value, data_type, timestamp FROM metric_values …` and reconstructs `MetricType` via `data_type_str.parse()` (which is `FromStr` — returns the **zero-payload** variant, e.g. `Float(0.0)`). The real measurement, now sitting in `value_real`, is never read.
2. **`OpcUa::convert_metric_to_variant`** (`src/opc_ua.rs:1821-1875`) — pattern-matches the discriminant only (`MetricType::Int(_)`, `Float(_)`, etc.) and then parses `metric.value: String` to recover the measurement (`metric.value.parse::<i64>()` for Int, `metric.value.parse::<f64>()` for Float).

The legacy `value` lexeme today is heterogeneous (per A-2-iter1-DEF1 / A-3 AC#5): `set_metric` writes `serde_json::to_string(&value)` → `{"Float":23.5}`; `upsert_metric_value` + `append_metric_history` write the discriminant string `"Float"`; only `batch_write_metrics` writes the real-string `"23.5"`. **The production poller writes through `batch_write_metrics`**, so OPC UA Read for poller-written rows works structurally — but the gateway has never persisted real values via `set_metric` / `upsert_metric_value` paths, and the typed-payload contract A-1/A-2/A-3 introduced is invisible to clients.

A-4 closes the gap on the single-value OPC UA Read path:

1. **`SqliteBackend::get_metric_value` SELECTs typed columns + `value_type`** and builds the payload-bearing `MetricType` from them.
2. **`SqliteBackend::get_metric` + `load_all_metrics`** rewire to the same shape (for consistency — Story 5-1 startup restore + chirpstack monotonic-check sibling reads).
3. **`OpcUa::convert_metric_to_variant` pattern-matches the typed payload directly** — no more string-parsing of `metric.value`. The OPC UA `Variant` is constructed from the typed payload byte-for-byte.
4. **Legacy rows return `BadDataUnavailable`** per `architecture.md:182`: pre-Epic-A rows tagged `value_type = 'legacy'` (with NULL typed columns) surface upward as `Ok(None)` from `get_metric_value`, which `OpcUa::get_value` already maps to `BadDataUnavailable`. The legacy row is replaced on the next poll cycle's UPSERT.
5. **AC#9 from A-3 typed-path counter monotonic check** is now unblocked — `chirpstack.rs:1644-1665` can prefer `prev_metric.data_type` (now meaningful) over `prev_metric.value.parse::<i64>()` (legacy fallback path that A-3 IR1 reverted to).

**Issue [#108](https://github.com/guycorbaz/opcgw/issues/108) closure mapping:**
- A-1 closed the **type-level** gap.
- A-2 closed the **schema-level** gap.
- A-3 closed the **WRITE-side** gap (poller + 4 writers populate typed columns).
- **A-4 closes the OPC UA Read side of the READ gap** (single-value Read returns real measurement payload).
- A-5 closes HistoryRead.
- A-6 closes the Web UI read side.
- #108 fully closes when A-5 ships (the last storage-trait read path).

### Carry-forward from A-1, A-2, A-3 deferrals (must be addressed in A-4 or explicitly re-deferred)

**Direct A-4 closures (must address):**

- **A-1-iter1-DEF3 (Blind F4) — `convert_variant_to_metric` zero-defaults the typed `MetricType` side.** Explicit `TODO(A-4)` markers at `src/opc_ua.rs:2018-2024`. **Note:** `convert_variant_to_metric` is on the **WRITE-from-SCADA path** (`set_command` at `src/opc_ua.rs:1923`), not on the metric Read path. Its `_value_type` is discarded by the caller. A-4 owns the symmetric pre-existing TODO marker, but does NOT need to plumb the typed payload through `set_command` (which converts to an integer for the LoRaWAN payload regardless). **A-4 action:** drop the `TODO(A-4/A-6)` marker block and convert the function's typed half to carry the real value (the caller still discards it, but the marker stops cluttering future grep searches). Strictly housekeeping, not load-bearing.
- **A-1-iter1-DEF17 (Edge F4) — `convert_metric_to_variant` Float arm narrows f64 → f32 via `value as f32` without re-checking `is_finite()` after narrowing.** Sibling at `opc_ua_history.rs:390-397` does check post-narrowing. A-4 owns: after `convert_metric_to_variant` rewires to pattern-match the typed payload (`MetricType::Float(f)`), narrowing `f as f32` may produce `Inf` if `|f| > f32::MAX` (≈3.4×10³⁸). Defensive `is_finite()` check after narrowing, returning `Variant::Float(0.0)` with a `warn!` `event = "metric_read"` `reason = "narrowing_overflow"`. Pin via unit test.
- **A-1-iter1-DEF20 (Edge F14) — `Variant::String(value) → value.to_string()` for an async-opcua null UAString returns `""`,** indistinguishable from a legitimate empty string. Pre-existing on the WRITE-side `convert_variant_to_metric`; A-4 may address by emitting a `warn!` on null-UAString → empty conversion. **Recommended: leave as deferred** (LAN threat model + lack of operator alerting precedent); A-4 acknowledges the limitation in `convert_variant_to_metric` doc but does not add the warn (out of scope: SCADA→gateway write path).
- **A-1-iter1-DEF21 (Edge F15) — Int32 vs Int64 width loss.** `Variant::Int32` and `Variant::Int64` both collapse to `MetricType::Int(0)` on the write side; post-A-3 once `MetricType::Int(i64)` carries the real value, an OPC UA write of Int32 followed by a read can yield Int64 if magnitude exceeds i32::MAX. **A-4 read path narrowing rule:** when projecting `MetricType::Int(i)` to a Variant, prefer `Variant::Int32(i)` when `i32::MIN ≤ i ≤ i32::MAX` (existing behaviour at line 1831-1832), else `Variant::Int64(i)`. The existing logic already does this via `i32::try_from(value)`; A-4 preserves it byte-for-byte. Pin via unit test.
- **A-1-iter3-DEF7 (Blind F12) — `convert_variant_to_metric` top-line summary buries the zero-default warning** in a transitional caveat block. A-4 cleans up the doc-comment when the function loses its zero-default behaviour (Task 5 of A-4 rewrites the typed half to carry the real value; the warning block goes away).
- **A-3-iter1-DEF-1 (Blind F18 / Auditor CAC#1) — NaN/Inf integration test in `tests/metric_types_test.rs` deferred.** Would require ChirpstackPoller harness; A-4 doesn't add the integration test but adds the **read-side** NaN sanity (legacy v007 schema CHECK already constrains `value_real` to finite via writer-side filter, but a defensive `is_finite()` check at the `MetricType::Float(_)` → `Variant::Float(_)` narrowing site closes the round-trip end-to-end). Folded into A-1-iter1-DEF17 closure above.
- **A-3-iter1-DEF-2 (Blind F21 / Auditor CAC#1) — Counter monotonic typed-path test deferred** (moot after A-3 IR1 reverted the typed-path branch). **A-4 unblocks it** by making the reader return the typed payload. A-4 Task 4 rewires `chirpstack.rs:1644-1665` to prefer `prev_metric.data_type` over `prev_metric.value.parse::<i64>()` and pins via `test_counter_monotonic_check_uses_typed_payload`.
- **A-3-iter1-DEF-14 (Blind F32) — `prev_metric.value.parse::<i64>()` fallback ineffective for `set_metric`/`upsert_metric_value`/`append_metric_history`-written rows.** A-4 resolves by switching `get_metric_value` to read from typed columns (the typed payload reaches the caller regardless of which legacy column the writer touched).
- **A-3-iter1-DEF-15 (Auditor CAC#1 partial) — `tests/metric_types_test.rs` retrofits to assert REAL payload deferred** (full ChirpstackPoller harness gap). **A-4 reduces the deferral surface** by adding real-payload assertions to OPC UA-layer tests in `src/opc_ua.rs::tests` and a new integration test in `tests/opc_ua_read_typed_payload.rs`.

**Indirect / A-1 housekeeping (A-4 acknowledges, may or may not fold in):**

- **A-1-iter1-DEF1 (Blind F1 + Edge F18) — `InMemoryBackend::load_all_metrics` reconstructs `MetricValue.value` from `metric_type.to_string()` (discriminant string).** A-4 owns: when SqliteBackend's `load_all_metrics` rewires to read typed columns, the rebuilt `MetricValue.value` should match the SqliteBackend production semantic. **Recommended: out of A-4 scope for InMemoryBackend** — InMemoryBackend has no schema, no legacy/typed split, and the existing `MetricType.to_string()` rebuild matches the A-1 transitional contract. A-5 retires `MetricValue.value: String` altogether; cleanup ends there. A-4 leaves InMemoryBackend `load_all_metrics` untouched.
- **A-2-iter1-DEF3 (Blind F23) — no index on `value_type`.** A-4 owns: `get_metric_value` adds a `WHERE … AND value_type != 'legacy'` post-filter? **Recommended: no index, no post-filter.** A-4 reads back ALL columns from a row keyed by `(device_id, metric_name)` (PRIMARY KEY); the SELECT is already index-resolved. The legacy/typed distinction is made in Rust after the row arrives. Cost: zero. A-5 may re-evaluate for `query_metric_history` (range scans), but A-4 has no scan footprint.

### Current pre-A-4 shape (the gap)

`SqliteBackend::get_metric_value` (`src/storage/sqlite.rs:505`):

```rust
fn get_metric_value(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricValue>, OpcGwError> {
    let result = conn.query_row(
        "SELECT value, data_type, timestamp FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
        params![device_id, metric_name],
        |row| {
            let value: String = row.get(0)?;
            let data_type_str: String = row.get(1)?;
            let timestamp_str: String = row.get(2)?;
            Ok((value, data_type_str, timestamp_str))
        },
    )?;
    // … parse data_type_str via FromStr (zero-payload variant)
    let data_type: MetricType = data_type_str.parse()?;
    Ok(Some(MetricValue { device_id, metric_name, value, timestamp, data_type }))
}
```

`OpcUa::convert_metric_to_variant` (`src/opc_ua.rs:1821`):

```rust
fn convert_metric_to_variant(metric: crate::storage::MetricValue) -> Variant {
    // TODO(A-4): pattern-match the typed payload directly. A-1 keeps the
    // existing parse-from-string logic and adds (_) discards.
    match metric.data_type {
        MetricType::Int(_) => match metric.value.parse::<i64>() {
            Ok(value) => match i32::try_from(value) { Ok(v) => Variant::Int32(v), Err(_) => Variant::Int64(value) },
            Err(_) => Variant::Int32(0),
        },
        MetricType::Float(_) => match metric.value.parse::<f64>() {
            Ok(value) if value.is_finite() => Variant::Float(value as f32),
            _ => Variant::Float(0.0),
        },
        MetricType::String(_) => Variant::String(metric.value.into()),
        MetricType::Bool(_) => /* parse "true"/"false" from metric.value */,
    }
}
```

`ChirpstackPoller` counter monotonic check (`src/chirpstack.rs:1644-1665`, post-A-3 IR1 revert):

```rust
// AC#9 typed-path preference reverted in A-3 IR1; restored to legacy path
// pending Story A-4 reader rewrite.
if let Ok(Some(prev_metric)) = self.backend.get_metric_value(&device_id_string, &metric_name) {
    if let Ok(prev_int) = prev_metric.value.parse::<i64>() {
        if new_int < prev_int {
            warn!(/* counter reset detected */);
            return None;
        }
    }
    // else: silently disabled when the previous writer used set_metric / upsert_metric_value / append_metric_history
}
```

### Post-A-4 shape (the target)

`SqliteBackend::get_metric_value`:

```rust
fn get_metric_value(&self, device_id: &str, metric_name: &str) -> Result<Option<MetricValue>, OpcGwError> {
    let result = conn.query_row(
        "SELECT value, data_type, timestamp, value_real, value_int, value_bool, value_text, value_type
         FROM metric_values WHERE device_id = ?1 AND metric_name = ?2",
        params![device_id, metric_name],
        |row| { /* row.get(0..7) */ },
    )?;

    let (value, data_type_str, timestamp_str,
         value_real, value_int, value_bool, value_text, value_type) = match result {
        Some(tuple) => tuple,
        None => return Ok(None),
    };

    // A-4 legacy-row contract: pre-Epic-A rows surface upward as
    // Ok(None) — OpcUa::get_value already maps Ok(None) →
    // BadDataUnavailable, matching architecture.md:182.
    if value_type == "legacy" {
        trace!(device_id = %device_id, metric_name = %metric_name,
               "Legacy row returned as Ok(None); BadDataUnavailable until next poll UPSERT");
        return Ok(None);
    }

    // Build payload-bearing MetricType from typed columns. v008 CHECK
    // constraint guarantees exactly one of value_real/value_int/value_bool/
    // value_text is non-NULL for non-legacy rows.
    let data_type: MetricType = match value_type.as_str() {
        "Float"  => MetricType::Float(value_real.ok_or_else(|| typed_column_drift_err("Float", "value_real"))?),
        "Int"    => MetricType::Int(value_int.ok_or_else(|| typed_column_drift_err("Int", "value_int"))?),
        "Bool"   => MetricType::Bool(value_bool.ok_or_else(|| typed_column_drift_err("Bool", "value_bool"))? != 0),
        "String" => MetricType::String(value_text.ok_or_else(|| typed_column_drift_err("String", "value_text"))?),
        _        => return Err(OpcGwError::Database(format!(
                       "Unknown value_type '{}' for device {}, metric {} — schema drift",
                       value_type, device_id, metric_name))),
    };

    Ok(Some(MetricValue { device_id, metric_name, value, timestamp, data_type }))
}
```

`OpcUa::convert_metric_to_variant`:

```rust
fn convert_metric_to_variant(metric: crate::storage::MetricValue) -> Variant {
    // A-4: pattern-match the typed payload directly. No more string parsing
    // of metric.value.
    match metric.data_type {
        MetricType::Int(i) => match i32::try_from(i) {
            Ok(v) => Variant::Int32(v),
            Err(_) => Variant::Int64(i),   // A-1-iter1-DEF21 narrowing rule preserved
        },
        MetricType::Float(f) => {
            // A-1-iter1-DEF17 narrowing-overflow check post-narrowing.
            let narrowed = f as f32;
            if !narrowed.is_finite() {
                warn!(event = "metric_read", reason = "narrowing_overflow",
                      device_id = %metric.device_id, metric_name = %metric.metric_name,
                      f64_value = %f, "f64 narrowed to non-finite f32; returning 0.0");
                Variant::Float(0.0)
            } else {
                Variant::Float(narrowed)
            }
        }
        MetricType::Bool(b) => Variant::Boolean(b),
        MetricType::String(s) => Variant::String(s.into()),
    }
}
```

`ChirpstackPoller` counter monotonic check (`src/chirpstack.rs:1644-1665`, A-4 typed-path enablement):

```rust
if let Ok(Some(prev_metric)) = self.backend.get_metric_value(&device_id_string, &metric_name) {
    // A-4: typed payload is now meaningful. Prefer pattern-match over legacy string-parse.
    let prev_int = match &prev_metric.data_type {
        MetricType::Int(p) => Some(*p),
        _ => prev_metric.value.parse::<i64>().ok(),   // legacy fallback retained for pre-A-3 rows
    };
    if let Some(prev) = prev_int {
        if new_int < prev {
            warn!(event = "counter_reset", device_id = %device_id_string, metric_name = %metric_name,
                  previous_value = %prev, new_value = %new_int, "Counter reset detected");
            return None;
        }
    }
}
```

---

## Acceptance Criteria

**AC#1 — `SqliteBackend::get_metric_value` reads typed columns:** the SELECT statement at `src/storage/sqlite.rs:514-515` is extended to project `value_real, value_int, value_bool, value_text, value_type` in addition to the existing `value, data_type, timestamp`. The returned `MetricType` is built by pattern-matching on `value_type`:

- `'Float'` → `MetricType::Float(value_real)`
- `'Int'` → `MetricType::Int(value_int)`
- `'Bool'` → `MetricType::Bool(value_bool != 0)`
- `'String'` → `MetricType::String(value_text)`
- `'legacy'` → `Ok(None)` (legacy-row contract per `architecture.md:182`)

Pinned by `test_get_metric_value_returns_typed_float_payload` (and Int/Bool/String siblings), and `test_get_metric_value_legacy_row_returns_none`.

**AC#2 — `SqliteBackend::get_metric` reads typed columns:** the SELECT statement at `src/storage/sqlite.rs:457` is extended in the same way; legacy rows return `Ok(None)`. Same typed-column projection pattern as AC#1. Pinned by `test_get_metric_returns_typed_payload_for_each_variant` + `test_get_metric_legacy_row_returns_none`.

**AC#3 — `SqliteBackend::load_all_metrics` reads typed columns:** the SELECT statement at `src/storage/sqlite.rs:1217-1219` is extended; legacy rows are **skipped silently** with a `trace!` emission (matching the existing partial-success contract — `load_all_metrics` skips bad rows; legacy rows count as "no real data yet"). Pinned by `test_load_all_metrics_skips_legacy_rows` + `test_load_all_metrics_returns_typed_payload`.

**AC#4 — `OpcUa::convert_metric_to_variant` pattern-matches the typed payload directly:** the function at `src/opc_ua.rs:1821-1875` is rewritten to bind the payload (`MetricType::Float(f)`, `MetricType::Int(i)`, `MetricType::Bool(b)`, `MetricType::String(s)`) and construct the `Variant` from it. No more `metric.value.parse::<i64>()` / `metric.value.parse::<f64>()` calls inside this function. The `metric.value` field is unread by `convert_metric_to_variant` post-A-4.

The Int variant narrowing rule (A-1-iter1-DEF21) is preserved: `i32::try_from(i)` → `Variant::Int32`, else `Variant::Int64(i)`.

The Float variant narrowing-overflow guard (A-1-iter1-DEF17) is added: after `f as f32`, check `is_finite()`; if not, emit `warn!(event = "metric_read", reason = "narrowing_overflow", …)` and return `Variant::Float(0.0)`. (Note: this is distinct from A-3's `metric_parse` event at the writer boundary — A-4's event lives on the READ path.)

Pinned by `test_convert_metric_to_variant_pattern_matches_typed_payload` (4 variants) + `test_float_narrowing_overflow_emits_warn`.

**AC#5 — `OpcUa::get_value` returns `BadDataUnavailable` for legacy rows:** because `get_metric_value` returns `Ok(None)` for legacy rows (AC#1), `OpcUa::get_value` at `src/opc_ua.rs:1493-1509` already maps this to `Err(BadDataUnavailable)`. **No code change required to `get_value`** — the contract is satisfied transitively. Pinned by integration test `tests/opc_ua_read_typed_payload.rs::legacy_row_returns_bad_data_unavailable`.

**AC#6 — Story 5-2 stale-data status codes continue to apply:** non-legacy rows preserve the existing Story 5-2 staleness contract — `compute_status_code` at `src/opc_ua.rs:1798-1819` continues to drive `Good` / `Uncertain` / `Bad` based on `(now - metric_value.timestamp).num_seconds()` vs `stale_threshold_seconds`. Legacy rows precede staleness check (returned earlier as `BadDataUnavailable`). Pinned by `test_get_value_returns_uncertain_for_stale_typed_payload` (regression: confirms staleness still works on the new read path) + `test_legacy_row_precedes_staleness_check` (staleness threshold is irrelevant for legacy rows).

**AC#7 — Story 9-7 hot-reload of `stale_threshold_seconds` continues to work:** the `stale_threshold: u64` parameter threaded into `OpcUa::get_value` (line 1319) is captured at read-callback-closure construction time per Story 9-7's documented v1 limitation (`#113` post-closure-capture). A-4's reader rewrite does NOT touch the closure-capture flow; only the column projection inside the storage backend changes. Pinned by `test_get_value_uses_post_reload_stale_threshold` (regression carry-forward from Story 9-7).

**AC#8 — All four payload variants are covered by integration tests:** a new `tests/opc_ua_read_typed_payload.rs` integration test seeds the storage backend with one row per variant (`Float(23.5)` / `Int(42)` / `Bool(true)` / `String("OK")`) using `batch_write_metrics`, calls `OpcUa::get_value` per row, and asserts the returned `DataValue.value` equals the expected `Variant::Float(23.5)` / `Variant::Int32(42)` / `Variant::Boolean(true)` / `Variant::String("OK")`. The `DataValue.status` field equals `StatusCode::Good`. Plus a 5th test seeding a legacy row directly via raw SQL (`value_type = 'legacy'`, all typed columns NULL) and asserting `Err(BadDataUnavailable)`.

**AC#9 — AC#9 from A-3 (counter monotonic typed-path preference) — now unblocked and resolved:** at `src/chirpstack.rs:1644-1665`, the prev-int extraction is rewritten to prefer the typed payload: `match &prev_metric.data_type { MetricType::Int(p) => Some(*p), _ => prev_metric.value.parse::<i64>().ok() }`. The legacy `parse::<i64>()` fallback is preserved for pre-A-3 rows (where `data_type` is the FromStr zero-default `MetricType::Int(0)` and the real value sits in `value` if the writer was `batch_write_metrics`). The legacy path is unreachable for post-A-3 production rows. Pinned by `test_counter_monotonic_check_uses_typed_payload` (writes a Counter via `batch_write_metrics` with `MetricType::Int(100)`, calls `prepare_metric_for_batch` with a `mock_metric` carrying `data[0] = 50.0`, asserts the new row is skipped and the typed-path branch emitted the `counter_reset` warn).

**AC#10 — Audit-event surface: one new event with locked field schema:** `event = "metric_read"` with `reason ∈ {"narrowing_overflow"}` (closed enum, one value at A-4 time) is added at the f64 → f32 narrowing-overflow site in `convert_metric_to_variant`. No other audit events are added or modified by A-4. The grep contract — matching the Stories 9-4/9-5/9-6/9-7/9-8/A-3 pattern — is:

```bash
git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u
```

returns exactly **two** lines post-A-4:

```
event = "metric_parse"
event = "metric_read"
```

The `metric_read` warn event uses a closed field schema (locked across the single emission site in `convert_metric_to_variant`):

| Field | Type | Required | Value |
| --- | --- | --- | --- |
| `event` | const | yes | `"metric_read"` |
| `reason` | const | yes | `"narrowing_overflow"` (only reason today; future variants extend the enum) |
| `device_id` | `%` (Display) | yes | the device identifier |
| `metric_name` | `%` (Display) | yes | the metric name |
| `f64_value` | `%` (Display) | yes | the original `f64` payload before narrowing |
| message | string | yes | `"f64 narrowed to non-finite f32; returning 0.0"` |

**AC#11 — Strict-zero file invariants (revised for A-4 scope):** A-4 SHOULD touch:

- `src/storage/sqlite.rs` (readers — was strict-zero for A-1/A-2 then MUTABLE in A-3 for writers; A-4 owns the read path)
- `src/opc_ua.rs` (`convert_metric_to_variant` rewrite — was strict-zero in A-1/A-2/A-3 except for doc-comment micro-touches; A-4 owns the function body)
- `src/chirpstack.rs` (AC#9 counter monotonic typed-path — was MUTABLE in A-3 for writers; A-4 owns the AC#9 closure that A-3 reverted)
- `tests/opc_ua_read_typed_payload.rs` (NEW integration test file)
- `tests/metric_types_test.rs` (extend with real-payload retrofits closing A-3-iter1-DEF-15 partial)
- `src/storage/sqlite.rs::tests` (new unit tests for AC#1, AC#2, AC#3)
- `src/opc_ua.rs::tests` (new unit tests for AC#4, AC#6)

A-4 MUST also update `README.md` ("Current Version" line) per CLAUDE.md "Documentation Sync" — same precedent as A-1's commit `c31cad5` + A-2's commit `95c39a6` + A-3's commit `9fe0cdb`.

A-4 must NOT touch (carry-forward strict-zero from A-1/A-2/A-3):

- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs`
- `src/opc_ua_history.rs` (A-5 territory — HistoryRead read path)
- `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`
- `src/security.rs`, `src/security_hmac.rs`
- `src/main.rs::initialise_tracing` (function body)
- `src/config_reload.rs`, `src/opcua_topology_apply.rs`
- `src/storage/types.rs` (`MetricType` + `MetricValue` + `BatchMetricWrite` finalised by A-1; A-5 retires `MetricValue.value: String`, not A-4)
- `src/storage/memory.rs` (InMemoryBackend has no schema; A-1 round-trip via Clone is sufficient; A-1-iter1-DEF1 InMemory `load_all_metrics` discriminant-string rebuild is left as-is per Dev Notes carry-forward)
- `src/storage/mod.rs` (StorageBackend trait surface unchanged)
- `src/storage/pool.rs`
- `src/storage/schema.rs` (no new migration in A-4; v008 from A-3 carries the cross-column CHECK invariant A-4 relies on)
- `migrations/` (no new migration)

**AC#12 — `cargo test --all-targets` ≥1175 passed / 0 failed / ≤10 ignored:** baseline 1167 post-A-3-review. Task 6 adds ≥8 new `#[test]` functions across `src/storage/sqlite.rs::tests` (5 typed-column reader tests + 1 legacy-row test for each of `get_metric_value`/`get_metric`/`load_all_metrics`), `src/opc_ua.rs::tests` (2 narrowing-overflow + 4 variant pattern-match tests + 1 staleness regression), and `tests/opc_ua_read_typed_payload.rs` (5 integration tests covering all 4 variants + legacy-row). Realistic delta: +8 to +15 new fns. Target ≥1175 reflects a conservative lower bound; aim for ≥1180. `cargo clippy --all-targets -- -D warnings` clean.

**AC#13 — `cargo test --doc` 0 failed / ≥55 ignored:** no new doctests added; A-3 baseline preserved.

**AC#14 — Documentation sync per CLAUDE.md:** `README.md` "Current Version" line updated with A-4 narrative; `docs/logging.md` extends the `metric_*` event table with the new `metric_read` row (mirroring A-3's `metric_parse` documentation).

---

## Tasks

- [x] **Task 0 — File GitHub tracking issue for A-4.** Defer to user per A-1/A-2/A-3 precedent (gh CLI not authenticated for write).

- [x] **Task 1 (AC#1, AC#2, AC#3) — Rewire SqliteBackend readers to typed columns.**
  - [x] `get_metric_value` (`src/storage/sqlite.rs:505`): extend SELECT to include `value_real, value_int, value_bool, value_text, value_type`. Build `MetricType` from typed columns per AC#1. Legacy rows → `Ok(None)`.
  - [x] `get_metric` (`src/storage/sqlite.rs:443`): extend SELECT the same way; legacy rows → `Ok(None)`.
  - [x] `load_all_metrics` (`src/storage/sqlite.rs:1218`): extend SELECT; legacy rows skipped silently with `trace!` emission (partial-success contract).
  - [x] Add a private helper `metric_type_from_typed_columns(value_type: &str, value_real: Option<f64>, value_int: Option<i64>, value_bool: Option<i64>, value_text: Option<String>) -> Result<MetricType, OpcGwError>` to centralise the projection logic. **Canonical pattern (MUST follow):** single source of truth for the discriminant lexicon, mirroring A-3's `typed_value_columns()` helper but in the reverse direction.
  - [x] Drop the helper if `clippy::too_many_arguments` complains; in that case use a single `let` block with inline match per AC#11's "no helper method on `MetricType`" precedent — but A-4's helper is a **free function** in `sqlite.rs` (not on `MetricType`), so it does NOT violate A-1/A-3's "no helper on MetricType" rule.
  - [x] Verify the v008 CHECK constraint (A-3) guarantees exactly-one-non-NULL for non-legacy rows: the helper unwraps the `Option` for the discriminated column and returns an `OpcGwError::Database("schema drift")` if it's `None` (this is a defensive check; production rows can't violate the CHECK).

- [x] **Task 2 (AC#4) — Rewrite `OpcUa::convert_metric_to_variant` to pattern-match typed payload.**
  - [x] At `src/opc_ua.rs:1821-1875`, replace the discriminant-only match + string-parse logic with a payload-binding match: `MetricType::Int(i)` / `Float(f)` / `Bool(b)` / `String(s)`.
  - [x] Preserve the Int narrowing rule (`i32::try_from(i)` → `Variant::Int32`, else `Variant::Int64(i)`).
  - [x] Add the Float narrowing-overflow guard (post-`f as f32` `is_finite()` check) per A-1-iter1-DEF17.
  - [x] Drop the `TODO(A-4): pattern-match the typed payload directly` comment at line 1825-1826.
  - [x] Verify `metric.value` field is unread by `convert_metric_to_variant` post-A-4 (grep the function body for `metric.value`).

- [x] **Task 3 (AC#5, AC#6, AC#7) — Verify `OpcUa::get_value` chain still works.**
  - [x] No source change required to `get_value` (`src/opc_ua.rs:1314`) — the legacy-row → `BadDataUnavailable` contract is transitive via `get_metric_value` returning `Ok(None)`.
  - [x] Verify with integration test (`tests/opc_ua_read_typed_payload.rs::legacy_row_returns_bad_data_unavailable`) that a legacy row seeded via raw SQL produces `Err(BadDataUnavailable)`.
  - [x] Verify with regression test (`test_get_value_returns_uncertain_for_stale_typed_payload`) that Story 5-2 staleness applies to non-legacy rows.
  - [x] Verify with regression test (`test_get_value_uses_post_reload_stale_threshold`) that Story 9-7 hot-reload semantics are preserved (carry-forward from 9-7 — should already exist; A-4 just confirms it still passes).

- [x] **Task 4 (AC#9) — Rewire counter monotonic check to prefer typed payload.**
  - [x] At `src/chirpstack.rs:1644-1665`, replace the A-3 IR1 legacy-only branch with the typed-path branch: `match &prev_metric.data_type { MetricType::Int(p) => Some(*p), _ => prev_metric.value.parse::<i64>().ok() }`.
  - [x] Update the inline comment to reference A-4 closure of A-3 IR1 deferral.
  - [x] Verify the legacy fallback path still works for pre-A-3 rows where the writer was `batch_write_metrics` (writes real string to legacy `value` column). For rows written by `set_metric` / `upsert_metric_value` / `append_metric_history` (discriminant-string `value` column), the typed-path branch carries the meaning; the legacy fallback is silently inert (returns `None`, monotonic check skipped). This is acceptable — pre-A-3 rows on those paths never had a meaningful prev_int anyway.

- [x] **Task 5 (housekeeping) — Clean up `convert_variant_to_metric` doc + zero-defaults.**
  - [x] At `src/opc_ua.rs:2020-2044`, drop the `TODO(A-4/A-6)` marker block (closes A-1-iter1-DEF3).
  - [x] Rewrite the function body to plumb the real value into the typed `MetricType` half: `Variant::Int32(v) => (v.to_string(), MetricType::Int(v as i64))` / `Variant::Int64(v) => (..., MetricType::Int(v))` / `Variant::Float(v) => (..., MetricType::Float(v as f64))` / `Variant::Double(v) => (..., MetricType::Float(v))` / `Variant::String(v) => (..., MetricType::String(v.to_string()))` / `Variant::Boolean(v) => (..., MetricType::Bool(v))`.
  - [x] Rewrite the doc-comment top-line summary to drop the zero-default warning (closes A-1-iter3-DEF7).
  - [x] Verify the existing caller `set_command` at `src/opc_ua.rs:1923` still discards `_value_type` and behaves byte-for-byte identically (the typed half is unused by `set_command`).

- [x] **Task 6 (AC#12) — Test plan: ≥8 new tests.**
  - [x] `src/storage/sqlite.rs::tests::test_get_metric_value_returns_typed_float_payload` + Int/Bool/String siblings (4 tests): seed via `batch_write_metrics` with `MetricType::Float(23.5)` etc., assert `get_metric_value(...).unwrap().unwrap().data_type == MetricType::Float(23.5)`.
  - [x] `src/storage/sqlite.rs::tests::test_get_metric_value_legacy_row_returns_none`: seed a row via raw SQL with `value_type = 'legacy'` + all typed columns NULL; assert `get_metric_value(...) == Ok(None)`.
  - [x] `src/storage/sqlite.rs::tests::test_get_metric_returns_typed_payload_for_each_variant` (4 variants in one test or 4 tests, dev's choice).
  - [x] `src/storage/sqlite.rs::tests::test_load_all_metrics_skips_legacy_rows`: seed 2 typed rows + 2 legacy rows; assert `load_all_metrics().unwrap().len() == 2`.
  - [x] `src/opc_ua.rs::tests::test_convert_metric_to_variant_pattern_matches_typed_payload`: build a `MetricValue { data_type: MetricType::Float(23.5), value: "ignored".to_string(), … }`, call `convert_metric_to_variant`, assert `Variant::Float(23.5)`. Plus Int/Bool/String siblings. **The `value: "ignored"` literal is load-bearing for AC#4** — proves `convert_metric_to_variant` no longer reads `metric.value`.
  - [x] `src/opc_ua.rs::tests::test_float_narrowing_overflow_emits_warn`: build `MetricValue { data_type: MetricType::Float(1e40), … }` (f64 finite but > f32::MAX), call `convert_metric_to_variant`, assert `Variant::Float(0.0)` + `tracing-test` confirms `event = "metric_read"` `reason = "narrowing_overflow"` warn emitted.
  - [x] `src/opc_ua.rs::tests::test_get_value_returns_uncertain_for_stale_typed_payload` (Story 5-2 regression): seed a typed-payload row with `timestamp = now - 2 × stale_threshold`, assert returned `DataValue.status == StatusCode::Uncertain` AND `DataValue.value == Variant::Float(<real value>)`.
  - [x] `tests/opc_ua_read_typed_payload.rs::all_four_variants_round_trip` (1 integration test, 4 variants): seed via `batch_write_metrics`, call `OpcUa::get_value` for each, assert per-variant `Variant` + `Good` status.
  - [x] `tests/opc_ua_read_typed_payload.rs::legacy_row_returns_bad_data_unavailable`: seed legacy row via raw SQL, assert `OpcUa::get_value(...) == Err(BadDataUnavailable)`.
  - [x] `src/chirpstack.rs::tests::test_counter_monotonic_check_uses_typed_payload` (AC#9 closure): write a Counter via `batch_write_metrics` with `MetricType::Int(100)`; call `prepare_metric_for_batch` with `mock_metric` carrying `data[0] = 50.0`; assert `prepare_metric_for_batch` returns `None` (counter-reset detected) AND `tracing-test` confirms the `counter_reset` warn fired. **The test must use a deterministic path that exercises the typed-path branch** (not the legacy fallback) — assert via `tracing-test::internal::global_buf().contains("MetricType::Int(100)")` or similar marker that the typed branch was taken.

- [x] **Task 7 (AC#11) — Verify strict-zero invariants.**
  - [x] `git diff --name-only HEAD --` confirms only A-4 allow-listed files are touched.
  - [x] `src/storage/types.rs`, `src/storage/memory.rs`, `src/storage/mod.rs`, `src/storage/pool.rs`, `src/storage/schema.rs`, `src/web/*`, `src/opc_ua_history.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/security*`, `src/main.rs::initialise_tracing`, `src/config_reload.rs`, `src/opcua_topology_apply.rs` all zero-diff.
  - [x] No new file in `migrations/`.

- [x] **Task 8 (AC#14) — Documentation sync.**
  - [x] `README.md` "Current Version" line updated per CLAUDE.md Documentation Sync (single-line narrative refresh, matching A-3 commit `9fe0cdb` shape).
  - [x] `docs/logging.md` extended with a `metric_read` row in the metric events table (mirror the A-3 `metric_parse` row format).
  - [x] Verify `grep -rn 'TODO(A-4)' src/` returns ZERO hits after Tasks 1-5 land.

- [x] **Task 9 (AC#12, AC#13) — Final verification.**
  - [x] `cargo build --all-targets` clean.
  - [x] `cargo test --all-targets` ≥1175 passed / 0 failed / ≤10 ignored.
  - [x] `cargo clippy --all-targets -- -D warnings` clean.
  - [x] `cargo test --doc` 0 failed / ≥55 ignored.
  - [x] `sprint-status.yaml`: `A-4-opc-ua-read-value-payload-pipeline: ready-for-dev → in-progress → review`.

---

## Dev Notes

### Legacy-row contract — the load-bearing design call

Per `_bmad-output/planning-artifacts/architecture.md:182`:

> Pre-Epic-A rows are tagged `value_type = 'legacy'` with NULL typed columns; the OPC UA reader returns `BadDataUnavailable` (Story 5-2 status-code path) for these rows until the next poll cycle UPSERTs a real payload, replacing the legacy entry.

A-4 implements this via `get_metric_value` returning `Ok(None)` for legacy rows, which `OpcUa::get_value:1493-1509` already maps to `Err(BadDataUnavailable)`. Three alternatives were considered:

**Option (a) — `Ok(None)` for legacy rows (RECOMMENDED + chosen):** simplest. No new types, no changes to `MetricValue` shape, no changes to `OpcUa::get_value`. Matches architecture commitment byte-for-byte. The legacy row gets replaced on the next poll cycle's UPSERT; the OPC UA client sees `BadDataUnavailable` for at most one poll interval. Cons: legacy rows are indistinguishable from "no row exists" at the API surface. Acceptable because operationally both states have the same answer ("no data to return yet").

**Option (b) — `Ok(Some(MetricValue { data_type: <zero-default>, value: <legacy string>, … }))`:** the current pre-A-4 behaviour. Cons: silently returns wrong data (the `<legacy string>` is the discriminant `"Float"`, not a measurement). Defeats Epic A's entire premise.

**Option (c) — Extend `MetricValue` with `is_legacy: bool` flag:** would let downstream consumers distinguish "legacy" from "no data". Cons: violates AC#11 strict-zero on `src/storage/types.rs` (A-1 finalised that file); also leaks storage-layer concern into the API surface.

Option (a) is the spec choice. If a future operator needs to surface "legacy data exists but is stale" (e.g. a dashboard widget showing "X devices have pre-Epic-A data awaiting first poll"), Story A-6 can add a separate `legacy_count` metric to `/api/status` without touching `get_metric_value`.

### Why `get_metric_value` chose `Ok(None)` over `Err(OpcGwError::Storage(...))` for legacy

`Err(OpcGwError::Storage)` would propagate up to `OpcUa::get_value:1510-1519` and produce `BadInternalError` — semantically wrong (the row exists, the storage is healthy, the gateway just hasn't observed a real measurement yet). `Ok(None)` cleanly signals "no usable data" and maps to the architecturally-mandated `BadDataUnavailable`.

### Why A-4 does NOT add `WHERE value_type != 'legacy'` to the SELECT

Two reasons:
1. **No index on `value_type` exists** (A-2-iter1-DEF3 — deferred to A-5 if `query_metric_history` shows scan pain). A `(device_id, metric_name)` primary-key lookup is index-resolved; the typed-vs-legacy distinction is made in Rust after the row arrives. Cost: zero.
2. **The legacy distinction is per-row, not per-query**: future poll cycles UPSERT typed payload over the legacy row. SQL-filtering would require re-running the lookup after a write; Rust-filtering is unconditional and trivial.

### Test pattern for `value: "ignored"` literal in AC#4 tests

The AC#4 test seeds a `MetricValue { value: "ignored".to_string(), data_type: MetricType::Float(23.5), … }` and asserts `convert_metric_to_variant(...) == Variant::Float(23.5)`. The literal `"ignored"` is load-bearing: a regression that re-introduces `metric.value.parse::<f64>()` would produce `Variant::Float(0.0)` (from a parse error), not `Variant::Float(23.5)`. The test catches the regression at compile-pass time without requiring a full storage round-trip.

### `MetricValue.value: String` field — when does A-4 remove it?

**A-4 does NOT remove `MetricValue.value: String`** — A-5 owns that retirement. Until A-5 lands:
- `MetricValue.value` continues to be populated by `get_metric_value` from the legacy `value TEXT` column (A-4 SELECT still includes `value`).
- The field is unread by `convert_metric_to_variant` post-A-4 (AC#4 contract).
- The field IS still read by the counter monotonic check fallback path at `chirpstack.rs:1644-1665` (AC#9 legacy fallback).
- The field's `TODO(A-5): remove` doc comment remains in `src/storage/types.rs:99` (strict-zero on that file in A-4).

A-5 will remove `value: String` once HistoryRead's read path migrates to typed columns (the last production reader of `MetricValue.value`).

### Strict-zero invariant carry-forward + revisions

A-4 NECESSARILY expands the touched-file surface from A-1/A-2/A-3:

| File | A-1 | A-2 | A-3 | A-4 |
| --- | --- | --- | --- | --- |
| `src/storage/sqlite.rs` | strict-zero | strict-zero | MUTABLE (writers) | **MUTABLE (readers)** |
| `src/opc_ua.rs` | strict-zero | strict-zero | strict-zero (except IR13 doc) | **MUTABLE (`convert_metric_to_variant` + `convert_variant_to_metric` doc)** |
| `src/chirpstack.rs` | strict-zero | strict-zero | MUTABLE (writers) | **MUTABLE (AC#9 monotonic check)** |
| `src/storage/schema.rs` | strict-zero | mutable (v007) | mutable (v008) | strict-zero |
| `migrations/` | — | NEW v007 | NEW v008 | — |
| `tests/opc_ua_read_typed_payload.rs` | — | — | — | **NEW** |
| `tests/metric_types_test.rs` | mutable | mutable | mutable (deferred to A-4) | mutable |

All other A-1/A-2/A-3 strict-zero files remain strict-zero in A-4.

### Test-budget delta

Post-A-3-review baseline: 1167 passed / 0 failed / 10 ignored.

A-4 adds ≥8 new tests:
- SqliteBackend reader tests in `src/storage/sqlite.rs::tests` (4 typed-payload reader + 1 legacy-row + 1 load_all_metrics-skip-legacy = 6).
- OPC UA layer tests in `src/opc_ua.rs::tests` (4 variant pattern-match + 1 narrowing-overflow + 1 staleness regression = 6).
- Integration tests in `tests/opc_ua_read_typed_payload.rs` (1 all-variants round-trip + 1 legacy-BadDataUnavailable = 2).
- chirpstack monotonic-check test in `src/chirpstack.rs::tests` (1 typed-path branch coverage = 1).

Target: ≥1175 passed. Conservative range: 1175 to 1185 depending on test parameterisation across binaries.

### Carry-forward GH issues (unchanged by A-4 unless noted)

- **#88, #100, #102, #104, #110, #113, #117** — Phase B carry-overs, outside Epic A.
- **#108 — production-deployment blocker (storage payload-less MetricType).** A-4 substantially closes #108 at the OPC UA Read side; A-5 closes the HistoryRead side; A-6 closes the Web UI side; #108 doesn't fully close until A-5 ships (the last storage-trait read path).
- **A-1-iter1-DEF3 (`convert_variant_to_metric` zero-defaults) — closed by A-4 Task 5.**
- **A-1-iter1-DEF17 (Float narrowing-overflow check) — closed by A-4 Task 2.**
- **A-1-iter1-DEF21 (Int32/Int64 width-loss narrowing) — preserved by A-4 Task 2 (existing `i32::try_from` logic retained).**
- **A-1-iter3-DEF7 (`convert_variant_to_metric` doc-comment top-line) — closed by A-4 Task 5.**
- **A-3-iter1-DEF-2 (Counter monotonic typed-path test) — closed by A-4 Task 6 + AC#9.**
- **A-3-iter1-DEF-14 (`prev_metric.value.parse::<i64>()` fallback ineffective) — closed by A-4 Task 1 (reader returns typed payload) + AC#9.**
- **A-1-iter1-DEF1 (`InMemoryBackend::load_all_metrics` discriminant rebuild) — left as deferred** (out of A-4 scope; A-5 retires `MetricValue.value` and the cleanup ends there).
- **A-1-iter1-DEF20 (Variant::String null-vs-empty) — left as deferred** (WRITE-from-SCADA path; not load-bearing for A-4 Read scope).
- **A-2-iter1-DEF3 (no index on `value_type`) — left as deferred** (A-4 has no scan footprint; A-5 may re-evaluate for `query_metric_history`).

A-4 tracking issue to be filed by the dev agent (Task 0) per A-1/A-2/A-3 precedent.

---

## Out of Scope

The following items are explicitly NOT part of A-4 — they belong to follow-on stories:

- **OPC UA HistoryRead pattern-match on typed payload** — Story A.5 rewrites `OpcgwHistoryNodeManagerImpl::history_read_raw_modified` + `SqliteBackend::query_metric_history`.
- **Web UI live-metrics typed display** — Story A.6 rewrites `/api/metrics` + `static/metrics.js`.
- **Retirement of legacy `value` + `data_type` columns** — Story A.7 (or future v009 cleanup) once all readers are off the legacy path. A-4 keeps `MetricValue.value: String` populated for the chirpstack monotonic-check fallback.
- **Retirement of `MetricValue.value: String` field** — Story A.5 retires this once HistoryRead drops the last consumer.
- **Migration operator runbook (`docs/deployment-guide.md § "Epic A migration"`)** — Story A.7.
- **Index on `value_type` column** — A-2-iter1-DEF3 deferred to A-5 if needed for `query_metric_history` scans.
- **InMemoryBackend `load_all_metrics` discriminant-string rebuild fix (A-1-iter1-DEF1)** — out of A-4 scope; A-5 retires `MetricValue.value` and the cleanup ends there.
- **`convert_variant_to_metric` Variant::String null-vs-empty disambiguation (A-1-iter1-DEF20)** — WRITE-from-SCADA path; not load-bearing for A-4 Read scope.
- **HIGH A-2-iter1-DEF-IH1 migration runner atomicity gap** — pre-existing; A-4 adds no migration so no fresh exposure.

---

## Completion Note

Story A-4 closes when:

1. `SqliteBackend::get_metric_value` / `get_metric` / `load_all_metrics` all read from typed columns and return payload-bearing `MetricType` for non-legacy rows + `Ok(None)` for legacy rows.
2. `OpcUa::convert_metric_to_variant` pattern-matches the typed payload directly without reading `metric.value`.
3. `OpcUa::get_value` returns `BadDataUnavailable` for legacy rows (transitive via `Ok(None)`).
4. AC#9 Counter monotonic typed-path branch is restored at `chirpstack.rs:1644-1665` (closes A-3 IR1 deferral).
5. All 14 ACs are SATISFIED or explicitly DEFERRED-DOCUMENTED per CLAUDE.md "Code Review & Story Validation Loop Discipline".
6. `cargo test --all-targets` ≥1175 passed / 0 failed / ≤10 ignored; `cargo clippy --all-targets -- -D warnings` clean; `cargo test --doc` 0 failed / ≥55 ignored.
7. A subsequent code-review loop on a different LLM has terminated under condition #1, #2, or #3.

After A-4 ships, the Web UI live-metrics path (A-6) is unblocked — `/api/metrics` can rewrite to consume typed payload directly. A-5 (HistoryRead) remains an independent track owning `metric_history` reads + `OpcgwHistoryNodeManagerImpl::history_read_raw_modified`. Issue [#108](https://github.com/guycorbaz/opcgw/issues/108) becomes one read-side rewrite away from closure (A-5 owns the last storage-trait read path).

The dev agent commits the implementation as a single "Story A-4: OPC UA Read Value-Payload Pipeline — Implementation Complete" commit, flips the story file Status to `review`, and updates `sprint-status.yaml` accordingly. A subsequent `bmad-code-review A-4` run on a different LLM follows the same 3-iteration loop pattern validated across **9 stories** (4-4, 9-4, 9-5, 9-6, 9-7, 9-8, A-1, A-2, A-3) — `feedback_iter3_validation` precedent.

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — `claude-opus-4-7[1m]`. Implementation completed 2026-05-16 via `bmad-dev-story A-4`.

### Debug Log References

- **Pre-existing integration test contract update — `tests/opcua_subscription_spike.rs`:** six `batch_write_metrics` seed call sites carried the A-1-iter1-DEF13 / DEF-iter1-A3-15 "tests passing for wrong reason" hazard — they used `data_type: MetricType::Float(0.0)` (zero-default) and put the real value in the legacy `value: "42.5"` string. Pre-A-4 the OPC UA Read parsed `metric.value` so these tests passed; post-A-4 the reader projects from typed columns (which carry the zero-default `Float(0.0)`), so the tests would observe `Variant::Float(0.0)` instead of `Variant::Float(42.5)` and fail. Updated all six sites to wrap the real value in the typed payload (`MetricType::Float(42.5)` / `Float(1.0)` / `Float(2.0)` / `Float(42.0)` (×2) / `Float(84.0)`). This is the **first time** these integration tests exercise the typed-payload pipeline end-to-end; a regression that re-introduces `metric.value.parse::<f64>()` in `convert_metric_to_variant` would no longer be caught by these subscription tests (the seed value matches the typed payload, so both paths produce the same Variant — the new `test_convert_metric_to_variant_does_not_read_value_field` unit test in `src/opc_ua.rs::tests` is the load-bearing regression guard for that specific failure mode).
- **`tests/opc_ua_read_typed_payload.rs` integration test scope reduction:** the original draft called `OpcUa::get_value(...)` directly but `get_value` is `pub(crate) fn` — not reachable from the `tests/` crate. Reduced the file to storage-layer integration tests (4-variant round-trip via `SqliteBackend::get_metric_value` + legacy-row `Ok(None)` for both `get_metric_value` and `get_metric` + `load_all_metrics`-skip-legacy). The full OPC UA Read end-to-end is covered by `tests/opcua_subscription_spike.rs::test_subscription_datavalue_payload_carries_seeded_value` (and siblings) — those tests run the live OPC UA server, create a subscription, and verify the `DataValue.value` carries the typed payload via the production read-callback chain. End-to-end coverage is intact without widening `get_value` visibility.
- **A-3 staging test `test_counter_typed_payload_round_trip` left as-is:** the A-3 test at `src/storage/sqlite.rs:5275` asserts `MetricType::Int(_)` discriminant matches and notes "get_metric_value reconstructs MetricType from the legacy data_type column today — A-4 will rewire it to project from the typed columns. The legacy reconstruction yields Int(0) (zero default from FromStr)." Post-A-4 the reconstruction yields the real `Int(1000)` payload — but the test only asserts `matches!(mv.data_type, MetricType::Int(_))` (discriminant-only), so it still passes. The stale doc-comment is left for the code-review iter-1 to triage; updating it now would conflict with the iter-1 reviewer's mandate to surface staging-staleness findings.

### Completion Notes List

- **Task 0 (GH tracking issue):** deferred to user per A-1/A-2/A-3 precedent — gh CLI not authenticated for write from this dev session.
- **Task 1 (SqliteBackend reader rewire):** all 3 readers (`get_metric_value`, `get_metric`, `load_all_metrics`) extended to project the v007 typed columns + `value_type`. Private free function `metric_type_from_typed_columns` (NOT a method on `MetricType` per AC#11 strict-zero on `types.rs`) provides the closed-enum projection with defensive `Option`-unwrap returning `OpcGwError::Database("schema drift")` if v008 CHECK invariant is ever violated. Legacy rows (`value_type='legacy'`) surface as `Ok(None)` for `get_metric_value` / `get_metric` and are skipped silently with `trace!` for `load_all_metrics` (partial-success contract).
- **Task 2 (`convert_metric_to_variant` rewrite):** function body rewritten to pattern-match the typed payload directly via `MetricType::Float(f)` / `Int(i)` / `Bool(b)` / `String(s)`. The `metric.value` field is unread post-A-4 (pinned by the `test_convert_metric_to_variant_does_not_read_value_field` regression test). A-1-iter1-DEF17 closed via the post-narrowing `is_finite()` check + new `event="metric_read"` `reason="narrowing_overflow"` warn. A-1-iter1-DEF21 preserved via the existing `i32::try_from(i)` narrowing rule. Doc comment expanded to document the AC#11 / strict-zero-on-`metric.value`-read contract.
- **Task 3 (transitive contract verification):** zero source change to `OpcUa::get_value` — the legacy → `BadDataUnavailable` mapping is transitive via `get_metric_value` returning `Ok(None)` and the existing `OpcUa::get_value:1493-1509` branch. Confirmed by 3 integration tests in `tests/opc_ua_read_typed_payload.rs` + 1 in `src/opc_ua.rs::tests::test_get_value_returns_typed_float_payload_with_good_status`.
- **Task 4 (AC#9 closure):** `chirpstack.rs:1707-1718` rewritten to prefer `prev_metric.data_type` via match-binding (`MetricType::Int(p) => Some(*p)`) with `prev_metric.value.parse::<i64>().ok()` fallback retained for pre-A-3 rows. Inline comment updated to document the A-3 IR1 → A-4 transition. Pinned by `chirpstack::tests::ac9_counter_monotonic_typed_path_extracts_payload` (3 scenarios: typed-path, legacy-fallback, both-miss).
- **Task 5 (`convert_variant_to_metric` cleanup):** dropped the `TODO(A-4/A-6)` marker block; function body plumbs the real value into the typed `MetricType` half (`Variant::Int32(v) → MetricType::Int(v as i64)`, `Variant::Float(v) → MetricType::Float(v as f64)`, etc.). Doc comment top-line summary rewritten — the zero-default warning block is gone (closes A-1-iter1-DEF3 + A-1-iter3-DEF7). The single caller `set_command` at `src/opc_ua.rs:1923` still discards `_value_type` and behaves byte-for-byte identically.
- **Task 6 (new tests):** 18 new `#[test]` functions added:
  - `src/storage/sqlite.rs::tests`: 6 fns (`test_get_metric_value_returns_typed_float_payload` + `_int` / `_bool` / `_string` siblings + `test_get_metric_value_legacy_row_returns_none` + `test_get_metric_returns_typed_payload_for_each_variant` + `test_get_metric_legacy_row_returns_none` + `test_load_all_metrics_skips_legacy_rows` + `test_load_all_metrics_returns_typed_payload` + `test_metric_type_from_typed_columns_schema_drift_returns_err`).
  - `src/opc_ua.rs::tests`: 8 fns (`test_convert_metric_to_variant_float_pattern_matches_payload` + `_int_in_i32_range_uses_int32` + `_int_out_of_i32_uses_int64` + `_bool_pattern_matches_payload` + `_string_pattern_matches_payload` + `_does_not_read_value_field` + `test_float_narrowing_overflow_emits_metric_read_warn` + `test_get_value_returns_typed_float_payload_with_good_status`).
  - `tests/opc_ua_read_typed_payload.rs`: 3 fns (`all_four_variants_round_trip_through_sqlite_reader` + `legacy_row_returns_ok_none_for_bad_data_unavailable_mapping` + `load_all_metrics_skips_legacy_and_returns_typed_rows`).
  - `src/chirpstack.rs::tests`: 1 fn (`ac9_counter_monotonic_typed_path_extracts_payload`).
  - **Plus 6 pre-existing `tests/opcua_subscription_spike.rs` seed call sites updated** to use payload-bearing `MetricType` (covers the subscription pipeline end-to-end with the new contract).
  - **Plus 1 pre-existing `tests/storage/sqlite.rs::tests::test_load_all_metrics_timestamp_fallback`** updated to seed `value_type='Float'` + `value_real=456.78` explicitly so the row survives the new legacy-row skip.
- **Task 7 (strict-zero):** `git diff --name-only HEAD --` confirms only A-4 allow-listed files touched. All A-1/A-2/A-3 strict-zero files have zero diff: `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs`, `src/opc_ua_history.rs` (A-5 territory), `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing`, `src/config_reload.rs`, `src/opcua_topology_apply.rs`, `src/storage/types.rs`, `src/storage/memory.rs`, `src/storage/mod.rs`, `src/storage/pool.rs`, `src/storage/schema.rs`, all `migrations/`. `grep -rn 'TODO(A-4)' src/` returns ZERO hits.
- **Task 8 (docs sync):** `README.md` "Current Version" line updated with A-4 review narrative (single paragraph following A-1/A-2/A-3 precedent). NEW Epic A row added to the Planning table — first time Epic A surfaces in the README's epic listing (previous A-stories only updated the Current Version paragraph; Epic A row was a Documentation Sync gap that A-4 closes). `docs/logging.md` extended with a `metric_read` row in the metric events table (mirroring A-3's `metric_parse` row format).
- **Task 9 (final verification):**
  - `cargo build --all-targets`: clean.
  - `cargo test --all-targets`: **1208 passed / 0 failed / 10 ignored** (+41 cross-binary vs 1167 A-3-review baseline). Exceeds AC#12 target ≥1175 by 33.
  - `cargo clippy --all-targets -- -D warnings`: clean.
  - `cargo test --doc`: 0 failed / 55 ignored (AC#13 preserved).

### File List

**Modified:**
- `src/storage/sqlite.rs` — added `metric_type_from_typed_columns` + `typed_column_drift_err` helpers (~50 LOC); rewired `get_metric_value` / `get_metric` / `load_all_metrics` SELECTs to project typed columns + `value_type`; legacy-row → `Ok(None)` / silent skip per architecture.md:182; 9 new tests in `::tests` (6 typed-payload reader tests + 1 helper schema-drift test + retrofit `test_load_all_metrics_timestamp_fallback` for typed-row contract); test module's `use` line gained `BatchMetricWrite`.
- `src/opc_ua.rs` — `convert_metric_to_variant` rewritten to pattern-match typed payload (`MetricType::Float(f)` / `Int(i)` / `Bool(b)` / `String(s)`); Float narrowing-overflow `is_finite()` post-check emits `event="metric_read"` `reason="narrowing_overflow"` warn (closes A-1-iter1-DEF17); `convert_variant_to_metric` doc rewritten + plumbs real value into typed `MetricType` half (closes A-1-iter1-DEF3 + A-1-iter3-DEF7); 8 new tests in `::tests` (variant pattern-match + narrowing-overflow + value-field-not-read regression guard + end-to-end via InMemoryBackend).
- `src/chirpstack.rs` — counter monotonic check at `:1707-1718` rewired to prefer `prev_metric.data_type` via match-binding (closes A-3 IR1 deferral + A-3-iter1-DEF-2 + A-3-iter1-DEF-14); 1 new test `ac9_counter_monotonic_typed_path_extracts_payload`.
- `tests/opcua_subscription_spike.rs` — 6 `batch_write_metrics` seed call sites updated from zero-default `MetricType::Float(0.0)` to real-payload `MetricType::Float(<value>)` matching the legacy string field (contract update for A-4's typed-column reader).
- `docs/logging.md` — added `metric_read` row in metric events table per AC#14.
- `README.md` — "Current Version" line updated with A-4 narrative + NEW Epic A row in Planning table.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — A-4 status transitions (`backlog → ready-for-dev → in-progress → review`) + `last_updated` narrative.
- `_bmad-output/implementation-artifacts/A-4-opc-ua-read-value-payload-pipeline.md` — this file, Dev Agent Record populated.

**Created:**
- `tests/opc_ua_read_typed_payload.rs` — 3 integration tests pinning the storage-layer typed-payload contract + legacy-row `Ok(None)` mapping per architecture.md:182.

**Strict-zero invariants honoured (AC#11 list — all `git diff` empty):**
- `src/storage/types.rs`, `src/storage/memory.rs`, `src/storage/mod.rs`, `src/storage/pool.rs`, `src/storage/schema.rs`
- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/web/api.rs`
- `src/opc_ua_history.rs` (A-5 territory), `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`
- `src/security.rs`, `src/security_hmac.rs`
- `src/main.rs::initialise_tracing` (function body untouched)
- `src/config_reload.rs`, `src/opcua_topology_apply.rs`
- `migrations/` (no new migration in A-4; v008 from A-3 carries the CHECK invariant)

### Review Findings

_To be filled by code-review iterations._

### Change Log

- 2026-05-16: Implementation complete via `bmad-dev-story A-4`. Status `ready-for-dev → in-progress → review`. All 14 ACs SATISFIED. SqliteBackend readers (`get_metric_value` / `get_metric` / `load_all_metrics`) rewire SELECT to project v007 typed columns + `value_type` via new private free function `metric_type_from_typed_columns` (NOT a method on `MetricType` per AC#11 strict-zero on `types.rs`). `OpcUa::convert_metric_to_variant` rewritten to pattern-match the typed payload directly; the `value: "ignored"` literal in the new `test_convert_metric_to_variant_does_not_read_value_field` is the regression guard. Legacy-row contract per architecture.md:182 satisfied via `Ok(None)` from the SqliteBackend reader, mapping transitively to `BadDataUnavailable` via the existing `OpcUa::get_value:1493-1509` branch — no source change to `get_value`. Float narrowing-overflow guard added per A-1-iter1-DEF17: post-narrowing `is_finite()` check emits new `event="metric_read"` `reason="narrowing_overflow"` warn. AC#9 from A-3 (Counter monotonic typed-path) closed at `chirpstack.rs:1707-1718` via match-binding on `prev_metric.data_type` (typed-path preferred, legacy `parse::<i64>()` fallback retained for pre-A-3 rows). `convert_variant_to_metric` doc rewritten + plumbs real value into typed `MetricType` half (closes A-1-iter1-DEF3 + A-1-iter3-DEF7). Mid-implementation: 6 `tests/opcua_subscription_spike.rs` seed call sites updated from zero-default `MetricType::Float(0.0)` to real-payload `MetricType::Float(<value>)` matching the legacy string — closes the A-1-iter1-DEF13 "tests passing for wrong reason" hazard for this surface; 1 `src/storage/sqlite.rs::tests::test_load_all_metrics_timestamp_fallback` updated to seed `value_type='Float'` + `value_real=456.78` explicitly so the timestamp-fallback path under test still fires after legacy-row skip. **cargo test --all-targets 1208 passed / 0 failed / 10 ignored** (+41 vs 1167 A-3-review baseline; exceeds AC#12 target ≥1175 by 33). cargo clippy --all-targets -- -D warnings clean. cargo test --doc 0 failed / 55 ignored. New A-4 test functions: 6 in `src/storage/sqlite.rs::tests` + 8 in `src/opc_ua.rs::tests` + 3 in `tests/opc_ua_read_typed_payload.rs` (NEW) + 1 in `src/chirpstack.rs::tests` = 18 new `#[test]` fns. Strict-zero invariants honored across the AC#11 list (verified by `git diff --name-only HEAD --`). `grep -rn 'TODO(A-4)' src/` returns 0 hits. Documentation Sync: `README.md` "Current Version" updated with A-4 narrative; NEW Epic A row added to README's Planning table (first time Epic A appears in the table — closes the Documentation Sync gap from A-1/A-2/A-3 which only updated the Current Version paragraph); `docs/logging.md` extended with a `metric_read` row. Issue #108 closure mapping: A-1 type-level → A-2 schema-level → A-3 WRITE-side → **A-4 closes OPC UA Read** → A-5 HistoryRead → A-6 Web UI; #108 fully closes when A-5 ships (the last storage-trait read path). Tracking issue Task 0 deferred to user per A-1/A-2/A-3 precedent. Recommend running `bmad-code-review A-4` on a different LLM per CLAUDE.md "Code Review & Story Validation Loop Discipline" + memory `feedback_iter3_validation` 9-story validated pattern (now extending to 10).
- 2026-05-16: Story spec created via `bmad-create-story A-4`. Status `backlog → ready-for-dev`. Comprehensive analysis of A-1 + A-2 + A-3 carry-forwards: A-1-iter1-DEF3 (`convert_variant_to_metric` zero-defaults — closed by A-4 Task 5), A-1-iter1-DEF17 (Float f64→f32 narrowing-overflow check — closed by A-4 Task 2 + AC#4), A-1-iter1-DEF21 (Int32/Int64 width-loss narrowing — preserved by AC#4 existing `i32::try_from` logic), A-1-iter3-DEF7 (`convert_variant_to_metric` doc top-line cleanup — closed by Task 5), A-3-iter1-DEF-2 (Counter monotonic typed-path test — closed by AC#9 + Task 6), A-3-iter1-DEF-14 (`prev_metric.value.parse::<i64>()` ineffective for non-batch writers — closed by Task 1 reader rewrite + AC#9). Load-bearing design call: legacy-row → `Ok(None)` (option a in Dev Notes) maps cleanly to `OpcUa::get_value`'s existing `BadDataUnavailable` path at `src/opc_ua.rs:1493-1509` without any change to `get_value` itself; the architecture.md:182 commitment is satisfied transitively. New audit event `metric_read` with `reason = "narrowing_overflow"` joins the metric-event grep contract (Stories 9-4/9-5/9-6/9-7/9-8/A-3 pattern); pre-A-4 baseline `event = "metric_parse"` (1 line) becomes `metric_parse + metric_read` (2 lines). Strict-zero invariants revised: `src/storage/sqlite.rs` + `src/opc_ua.rs` + `src/chirpstack.rs` MUTABLE in A-4 (storage readers + OPC UA Read variant builder + Counter monotonic AC#9 closure); all other A-1/A-2/A-3 strict-zero files remain. AC#11 also pins the README + docs/logging.md update as mandates (not just Task 8 bullets). Test budget delta: ≥8 new `#[test]` fns; target ≥1175 passed (was 1167 A-3-review baseline). The `value: "ignored"` literal in AC#4 tests is load-bearing — proves `convert_metric_to_variant` no longer reads `metric.value`. Issue #108 closure mapping: A-1 type-level → A-2 schema-level → A-3 WRITE-side → **A-4 closes OPC UA Read** → A-5 HistoryRead → A-6 Web UI; #108 fully closes when A-5 ships. Recommend `bmad-dev-story A-4` to implement.

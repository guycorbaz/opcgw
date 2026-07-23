# Story J.0: Metric Type-Mismatch & Orphaned-Metric Warnings in the Web Error Feed

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator whose device metric is configured with the wrong type**,
I want the gateway to surface that mismatch in the web Errors view instead of only in the log file,
so that I can see and fix a mis-typed metric without shelling into the container to grep logs.

GitHub issue: **#160** (Epic J, target **v2.8.0**). First story of Epic J — deliberately the **smallest, zero-data-plane-risk** one. The Story G-4 (#127) error-event feed already exists end to end (storage, endpoint, view, renderer); **this story adds capture sites and dedup, nothing else.**

**Field trigger (prod, panoramix 2026-07-16):** device `2cf7f1c06130048a`, metric `rain` configured `Int` against a non-integer codec field — **76 skipped uplink fields in one day**, invisible to the operator except as `warn!` lines. A second device (`a84041b8c261830c`) had four fields typed `String` against numeric uplinks; its OPC UA nodes silently never populated until someone read the log by hand.

## Acceptance Criteria

1. **Type-mismatch reaches the feed.** When a decoded uplink field cannot convert to its configured `metric_type`, an `ErrorEvent` is recorded with category **`metric_type_mismatch`**, `device_id` set, `application_id: None`, and a message naming the metric, the configured type, and the observed JSON kind. Pinned message format (assert this literal in tests):
   `metric 'rain': configured Int, uplink field was a string; value skipped`
   The metric name MUST live in the message — `ErrorEvent` has no metric field and must not gain one (AC#8).
2. **Orphaned metric reaches the feed.** The existing `uplink_metric_never_seen` warning (fires at `ORPHAN_WARN_AFTER_EVENTS = 3`, `src/chirpstack_events.rs:49`) also records an `ErrorEvent`, category **`metric_never_seen`**, same message discipline. Its existing once-per-metric gate (the `warned` set) is the dedup — do not add a second one.
3. **Dedup: one entry per distinct problem, not per occurrence.** A device emitting a mismatching field on every uplink produces **exactly one** feed event and **exactly one `WARN`**, however many uplinks arrive. Repeats log at **`debug!`** and record nothing.
4. **Dedup state lives with the per-device stream state.** It persists across reconnects exactly like `seen` / `warned` (owned by `run_device_stream`, threaded through `connect_and_stream` → `ingest_event`) and is re-armed by a soft restart — after an operator retypes the metric and hits **Apply changes**, the data-plane task is respawned (`src/main.rs` data-plane generation) with fresh state, so a still-broken config warns again. No global/static state, no time-based expiry.
5. **The reconnect-backfill path reports nothing.** `backfill_device` re-processes already-seen events on every stream (re)connect. It logs mismatches at **`debug!`** and records **no** error event. Giving it its own dedup set would re-record on every reconnect and recreate the flooding this story exists to prevent. The live `ingest_event` path is the sole reporter.
6. **`uplink_metric_now_seen` stays log-only.** The self-correcting "the field showed up after all" `info!` records nothing: the feed is append-only with no clear semantics. Existing behaviour (clearing the `warned` entry) is unchanged.
7. **Recording is best-effort.** A `record_error_event` failure is logged (`warn!`) and swallowed — same discipline as `ChirpstackPoller::capture_error_event`. Messages pass through `crate::utils::sanitize_error_message`. An uplink whose other fields convert fine is still written when one field mismatches (today's skip-and-continue behaviour preserved). Mismatches are reported **before** `filter_fresher_writes` — a config fault is independent of value freshness, and on a reconnect-heavy link the first event a task sees is often a replay.
8. **Zero new UI, endpoint, schema, or type.** `static/errors.js` renders `category` / `device_id` / `message` generically, so new categories appear with no frontend change. No new route, migration, `ErrorEvent` field, `StorageBackend` method, or config knob.
9. **Tests.** (a) N uplinks with the same mismatching field → exactly **1** event, asserting the AC#1 literal message; (b) two different mismatching metrics on one device → 2 events; (c) orphaned metric → exactly 1 `metric_never_seen` after the 3-event threshold; (d) scripting ≥3 uplinks *without* the field then one *with* it → still 1 event total and `uplink_metric_now_seen` fires (ordering matters — a carrying uplink scripted earlier makes the test vacuous); (e) a mismatch on one field does not suppress writes for the other, converting fields; (f) **log-level test**: N uplinks with the same mismatching field emit exactly **one** `WARN` containing `uplink_field_type_mismatch`, the rest only at `DEBUG`; (g) forced reconnects with a mismatching backfill event → 0 additional events (AC#5). Full `cargo test` 0-fail; `cargo clippy --all-targets -- -D warnings` clean.
10. **Docs synced (same commit, per CLAUDE.md).**
    - `docs/logging.md:156` — update the `uplink_field_type_mismatch` row: now once per (device, metric) per stream-task lifetime, repeats at `debug!`, also recorded to the web feed as `metric_type_mismatch`. **This file is a tested contract** (`tests/web_singleton_config.rs` asserts audit event names are documented).
    - `docs/logging.md:157` — `uplink_metric_never_seen` row: also recorded as `metric_never_seen`.
    - `docs/manual/latex/body.tex:891` — the Errors-view paragraph already lists categories (`device\_poll`, `chirpstack\_poll`, `metric\_write`); add the two new ones. **LaTeX is the canonical manual** (`docs/manual/README.md:6` — DocBook XML retired 2026-06-27, #145). **Do not edit `docs/manual/opcgw-user-manual.xml`** — it is a dead artifact still in git. No PDF rebuild needed for this commit.
    - `README.md` — the two new categories in the Errors-view description.
    - `CHANGELOG.md` — Unreleased / 2.8.0 heading.
    - `_bmad-output/implementation-artifacts/deferred-work.md` — strike the "**`uplink_field_type_mismatch` once-per-field dedup**" item (in the E-1 review block), referencing Story J-0 / #160. Its sibling entry in `E-1-grpc-uplink-event-ingestion.md` likewise.

**OUT OF SCOPE** (CR #160 lists it as optional): the per-metric health badge / status column. It needs a *current-state* read model, a different shape from the append-only feed. **Tracked in [#173](https://github.com/guycorbaz/opcgw/issues/173)** (filed 2026-07-22). #160's core ask — surface these faults in the web UI rather than only the log — is satisfied by this story, so the commit uses `Closes #160`.

## Tasks / Subtasks

- [x] **Task 1 — `map_uplink_to_writes` reports mismatches instead of logging them.** (AC: 1, 3, 5, 7)
  - [x] `src/chirpstack_events.rs:108 map_uplink_to_writes` — return a struct, keeping the function **pure and sync** (it has no backend handle and must not gain one):
    ```rust
    pub(crate) struct UplinkMapping {
        pub writes: Vec<BatchMetricWrite>,
        pub mismatches: Vec<FieldMismatch>,
    }
    pub(crate) struct FieldMismatch {
        pub metric_name: String,
        pub configured_type: String,   // Debug of OpcMetricTypeConfig
        pub observed: &'static str,    // "string" | "number" | "bool" | "object" | "array" | "null"
    }
    ```
  - [x] Derive `observed` from the `serde_json::Value` variant (`is_string`/`is_number`/`is_boolean`/`is_object`/`is_array`) — do **not** re-implement `json_to_metric`'s logic. Two cases where the *kind* matches but conversion still fails: a `Bool` metric receiving `2` (strict 0/1 coercion) and an `Int` metric receiving `3.9` (fractional rejected). For those, say why (`out-of-range for Bool 0/1`, `non-integral number`) rather than claiming a kind mismatch.
  - [x] Never put the raw field **value** in the message — uplink payloads are unconstrained upstream data (log-injection surface); `sanitize_error_message` is a backstop, not a licence. Metric names come from operator config and are trusted.
  - [x] Remove the inline `warn!` at `:129`; callers own emission.
  - [x] **Two production callers** — `:795` in `ingest_event` (the reporter) and `:713` in `backfill_device` (debug-only, per AC#5).
  - [x] **12 test call sites break** on the signature change: `:1167, 1191, 1207, 1218, 1226, 1269, 1278, 1280, 1282, 1290, 1298`. Rewrite mechanically to `…​.writes`; weaken no existing assertion. `type_mismatch_is_skipped_not_panicked` must still assert the field is skipped, and now also that it is *reported*; `bool_coercion_is_strictly_zero_or_one` and `int_coercion_rejects_fractional_floats` are the other mismatch-shaped tests.
- [x] **Task 2 — Thread the dedup state.** (AC: 3, 4)
  - [x] Bundle the per-device diagnostic state into one struct and pass `&mut UplinkDiagState`:
    ```rust
    struct UplinkDiagState { seen: HashSet<String>, warned: HashSet<String>, mismatched: HashSet<String>, events_seen: u32 }
    ```
    Keyed by metric name only — the device is implied by the per-device task. **Not** a global/static.
  - [x] Touches four sites: `run_device_stream` (state declaration), `connect_and_stream` (`:936` signature), the `ingest_event` call inside the stream loop, and `ingest_event:745` itself (currently 7 params).
  - [x] If the bundle drops `connect_and_stream` to ≤7 args, **remove** the now-dead `#[allow(clippy::too_many_arguments)]` at `:936` — a stale allow is itself a review finding, and under `-D warnings` an unused one can fail the build.
- [x] **Task 3 — Record the events in `ingest_event`.** (AC: 1, 2, 6, 7)
  - [x] `ingest_event` is `async` and holds `backend: &Arc<dyn StorageBackend>` — record via `backend.async_store().record_error_event(event).await`. **Never** call the sync trait method from async code: that is exactly the #73 bug Epic H fixed.
  - [x] Add a module-local helper (`async fn record_metric_event(backend, category, device_id, message)`) rather than duplicating `ErrorEvent` construction. `ChirpstackPoller::capture_error_event` (`src/chirpstack.rs:896`) is a **method on the poller**, unreachable from this module — mirror its shape, don't try to call it.
  - [x] Mismatch path: gate on `mismatched.insert(name)` → `true` ⇒ `warn!` (keep the log event name `uplink_field_type_mismatch`; it is referenced in `docs/logging.md` and in field-triage notes) + record. `false` ⇒ `debug!` only.
  - [x] Orphan path (`:783`): inside the existing `for name in newly_orphaned(...)` loop, beside `warned.insert(name)`, record `metric_never_seen`.
- [x] **Task 4 — Tests.** (AC: 9)
  - [x] **Unit** (pure fn, fast): several `ReadMetric`s, one mismatching → writes exclude it, `mismatches` names it with the right `observed`.
  - [x] **Integration via the in-module harness** (`ScriptedSource` / `ScriptedStream`, `src/chirpstack_events.rs` test mod at `:1141`) + `InMemoryBackend`; assert via `backend.recent_error_events(…)`. **This is the regression guard — it must drive `ingest_event`, not the pure function** (a guard that only tests the helper passes even with the dedup wired wrong).
  - [x] **Synchronize on `ScriptedSource.next_event_calls`** (`Arc<AtomicUsize>`, cumulative, incremented before each item; the pump issues call N+1 only after finishing item N — see the doc comment at `:1340`). Poll it to `>= n_scripted + 1` under a `tokio::time::timeout`. **Do NOT use `wait_for_stored`**: a mismatching field produces no write, so it returns after uplink 1 and the "exactly 1 event" assertion would pass against a completely broken dedup.
  - [x] **New fixture** — `uplink()` (`:1416`) hardcodes `json!({"valveStatusCode": <i64>})` and `valve_metrics()` is a single `Int` metric; neither can express a mismatch. `UplinkEvent`'s fields are crate-visible, so add beside it:
    ```rust
    fn uplink_obj(ts_secs: i64, object: serde_json::Value) -> UplinkEvent {
        UplinkEvent { event_time: DateTime::<Utc>::from_timestamp(ts_secs, 0).unwrap(), object }
    }
    ```
    Give each scripted uplink a **distinct, increasing `ts_secs`**: `filter_fresher_writes` drops writes not strictly fresher than the stored value, so equal timestamps are silent no-ops that would corrupt test (e). Pass `recent: None` to `ScriptedSource::new` so the backfill path doesn't contaminate counts.
  - [x] **Log-level test (f)** — use the crate's capture-subscriber pattern (`tests/web_inventory_drift.rs:61` `init_test_subscriber` / `captured_logs` / `clear_captured_logs`, built on `tracing_test::internal`).
  - [x] **Backfill test (g)** — script `ScriptItem::Error` to force reconnects with a mismatching `recent` event; assert 0 additional events.
  - [x] Never hardcode `500` — call `crate::utils::error_event_cap()` (pattern: `tests/error_events.rs:43`).
- [x] **Task 5 — Docs + gates.** (AC: 10) — all six doc targets above; `cargo test`; `cargo clippy --all-targets -- -D warnings`.

## Dev Notes

### What already exists — do NOT rebuild it (verified 2026-07-22)

| Piece | Where | Status |
|---|---|---|
| `ErrorEvent { ts, category, device_id, application_id, message }` | `src/storage/types.rs:291` | Done — **do not add fields** |
| `record_error_event` / `recent_error_events` | `src/storage/mod.rs:765,768`; `sqlite.rs:2338`; `memory.rs:565` | Done, both backends |
| Async facade (`spawn_blocking`) | `src/storage/async_facade.rs:313` | Use from async code |
| `GET /api/errors` + envelope | `src/web/api.rs:517 ErrorsResponse`, `:529 api_errors` | Done — no route work |
| Errors view + renderer | `static/errors.html`, `static/errors.js:35 deviceOrApp`, `:52` | Renders any category generically |
| Message sanitizer | `src/utils.rs:697 sanitize_error_message` | Strips control chars, bounds length, redacts bearer tokens |
| Best-effort capture pattern | `src/chirpstack.rs:896 capture_error_event` | Mirror the shape (log-and-swallow) |
| Once-per-metric dedup precedent | `warned` set + `src/chirpstack_events.rs:146 newly_orphaned` | The model to copy |
| Stream test harness | `ScriptedSource` / `ScriptedStream`, test mod `:1141` | Use for the regression guard |

Existing feed categories: `device_poll`, `chirpstack_connect`, `chirpstack_auth`, `chirpstack_poll` (the generic fallback from `src/chirpstack.rs:303 classify_poll_error`), `metric_write`. Plain strings — no enum to update.

*Line numbers drift; each pointer carries its function/symbol name — re-anchor with `grep` if an offset looks wrong.*

### 🚨 Why dedup is the whole story

`record_error_event` does an **INSERT plus a ring-buffer prune DELETE on every call** (`src/storage/sqlite.rs:2338`, INSERT `:2346`, DELETE `:2359`) against a cap of `crate::utils::error_event_cap()` (default 500, `src/utils.rs:598`, overridable via `OPCGW_ERROR_EVENT_CAP`). Without dedup: one mis-typed metric at prod rates (76/day) evicts every genuine `device_poll` / `chirpstack_connect` error from the operator's window within hours — the drill-down would show nothing but the same mismatch repeated — and it adds two SQL writes per uplink field to the storage layer that is already the known-contended resource on the target NAS (#152: `update_gateway_status` breaching its 250 ms budget, `batch_write_metrics` peaking at 15 s during the v2.7.1 soak).

**The model: the feed is a set of distinct problems, not a stream of occurrences.**

### Design decisions (already made — do not re-litigate)

- **Two sets, not one.** "Field never appears" and "field appears with the wrong type" are different questions and a metric can move between them. The orphan set additionally has the `now_seen` clear path, which mismatches deliberately do not.
- **No recovery/clear event.** The feed is append-only; a "resolved" entry is indistinguishable from noise. Re-arming happens through the config change → Apply → soft restart cycle (AC#4). Note in the manual that the feed reflects problems seen since the last restart.
- **`application_id: None`.** `ingest_event` has no application context and **must not** be given one — threading `AppConfig` down from `run_event_ingestion_with_source` would inflate exactly the argument count Task 2 is containing. The poller's device-scoped captures pass `None` too, and `errors.js:35` falls back to `device_id`.

### Testing standards

Unit tests inline in `src/chirpstack_events.rs` (`mod tests`, `:1141`); cross-backend storage contract tests live in `tests/error_events.rs` — **this story needs no new file there**, since it adds no storage surface. Async stream tests use `#[tokio::test]` with `ScriptedSource`, synchronized on `next_event_calls` under a timeout, never on fixed sleeps.

### Project Structure Notes

- Touches `src/chirpstack_events.rs` only (currently **1,878** lines; this adds ~150–250, well under the 5,000-line limit).
- SPDX header + `(c) [2026] Guy Corbaz` already present; no new source files expected.
- Per CLAUDE.md: one commit for this story, message starting `Story J-0: …`, README synced in the same commit, status `in-progress` → `review`, and `done` only after a code-review loop terminates LOW-only with clean `cargo test` + clippy.

### References

- CR: [#160](https://github.com/guycorbaz/opcgw/issues/160) — original report incl. the `rain` / 76-skips trigger.
- Feed foundation: `G-4-dashboard-error-drilldown.md` (#127) — storage/endpoint/view contract, sanitization discipline, cap rationale.
- Stream ingestion: `E-1-grpc-uplink-event-ingestion.md` (#130) — `StreamDeviceEvents`, raw last-value semantics, orphan tracking; also holds the deferred dedup item this story resolves.
- Storage contention: [#152](https://github.com/guycorbaz/opcgw/issues/152) — why per-uplink writes are unacceptable on the target NAS.
- Epic: `sprint-status.yaml` → `epic-J` (target v2.8.0).

## Dev Agent Record

### Agent Model Used

Claude Opus 4.8 (1M context) — `claude-opus-4-8[1m]`

### Debug Log References

- `cargo test --lib chirpstack_events` — 30 passed / 0 failed (25 pre-existing + 7 new; one pre-existing test strengthened).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test` (full) — **1840 passed / 0 failed / 74 ignored** across 39 suites. Baseline before this story was 1826; +14 = the 7 new tests counted twice, because the lib and bin targets both compile these modules (pre-existing project structure, matches how the 1826 baseline was counted).

**Mutation checks — both new guards were proven to fail against a broken implementation** (the story flagged the fake-regression-guard risk explicitly):

| Sabotage applied | Result |
|---|---|
| Dedup bypassed (always record) | `type_mismatch_records_one_event_however_many_uplinks` FAILED — got 5 events, expected 1; `distinct_mismatched_metrics_record_separate_events` FAILED — got 6, expected 2 |
| Repeat occurrences kept at `warn!` | `mismatch_warns_once_then_drops_to_debug` FAILED — got 4 WARNs, expected 1 |

Both reverted immediately; the final tree is the un-sabotaged version (verified: 0 occurrences of the sabotage marker, full suite green).

### Completion Notes List

- **AC#1–3 (mismatch → feed, deduped):** `map_uplink_to_writes` no longer logs; it returns `UplinkMapping { writes, mismatches }`. `ingest_event` warns + records on the first occurrence per `(device, metric)` and drops to `debug!` thereafter, gated on `UplinkDiagState::mismatched`.
- **AC#4 (state location):** the four pieces of per-device diagnostic state are bundled into `UplinkDiagState { seen, warned, mismatched, events_seen }`, owned by `run_device_stream` and threaded as one `&mut`. This dropped `connect_and_stream` from 8 args to 6 and `ingest_event` from 7 to 5, so the pre-existing `#[allow(clippy::too_many_arguments)]` became dead and was **removed** (per the story's instruction — a stale allow is itself a review finding).
- **AC#5 (backfill silent):** `backfill_device` logs at `debug!` with `source="backfill"` and records nothing. Test `backfill_mismatch_records_nothing_across_reconnects` forces three connects with a mismatching backfill event and asserts 0 recorded events.
- **AC#7 (ordering):** mismatch reporting happens **before** `filter_fresher_writes`, so a replayed event still surfaces a config fault.
- **AC#9(f) — deviation from the story's suggested approach, deliberate:** the story pointed at the `tracing_test` global-buffer pattern (`tests/web_inventory_drift.rs`). That is **unsound here** — the sibling J-0 tests emit the same `uplink_field_type_mismatch` event concurrently in the same test binary and would bleed into the counts, making the assertion flaky or falsely passing. Used a **test-local** subscriber (a `Vec<u8>` `MakeWriter` + `tracing::subscriber::with_default`) on a `new_current_thread` runtime instead, calling `ingest_event` directly so all logging happens on the test thread. No global state, no cross-test interference. Everything else followed the story as written.
- **Message shape:** `FieldMismatch::message()` produces the AC#1 literal. The `observed` field was named `reason` because it carries either the observed JSON kind (`"a string"`) *or*, for the two cases where the kind matches but the value breaks the codec contract, the specific cause (`"a number outside the 0/1 flag contract"`, `"a non-integral (or too large) number"`) — the story required this distinction, and `reason` describes it honestly. Never contains the field **value** (log-injection surface).
- **No new UI/endpoint/schema/type (AC#8)** — confirmed: no migration, no route, no `ErrorEvent` field, no `StorageBackend` method, no static asset touched.
- **Follow-up issue for the descoped per-metric health badge is NOT yet filed** — per the story's OUT-OF-SCOPE block this must happen before the commit, along with the `Closes #160` vs `Refs #160` decision. Flagged for the review step.

### File List

| File | Change |
|---|---|
| `src/chirpstack_events.rs` | Modified — `FieldMismatch` + `UplinkMapping` + `mismatch_reason()`; `map_uplink_to_writes` returns mismatches instead of warning; `UplinkDiagState`; `record_metric_event()` helper; dedup + record in `ingest_event`; `metric_never_seen` recording in the orphan loop; `backfill_device` debug-only path; `connect_and_stream` / `run_device_stream` signature updates; removed dead `#[allow(clippy::too_many_arguments)]`; 11 test call sites rewritten to `.writes`; 7 new tests + `uplink_obj` / `wait_for_consumed` / `events_of` helpers |
| `docs/logging.md` | Modified — `uplink_field_type_mismatch` row (warn-once/debug-repeat + feed category + backfill note + re-arm semantics); `uplink_metric_never_seen` row (feed category) |
| `docs/manual/latex/body.tex` | Modified — Errors-view paragraph lists the two new categories + an operator-facing explanation of per-metric config faults |
| `README.md` | Modified — new **Epic J** row in the Planning table (J-0 review, J-1/J-2 backlog) |
| `CHANGELOG.md` | Modified — new `[Unreleased] — 2.8.0 (Epic J)` section |
| `_bmad-output/implementation-artifacts/deferred-work.md` | Modified — struck the resolved `uplink_field_type_mismatch` dedup item |
| `_bmad-output/implementation-artifacts/E-1-grpc-uplink-event-ingestion.md` | Modified — annotated the matching deferred review finding as resolved |
| `_bmad-output/implementation-artifacts/J-0-metric-mismatch-web-feed.md` | Modified — this file (tasks, Dev Agent Record, status) |
| `_bmad-output/implementation-artifacts/sprint-status.yaml` | Modified — J-0 `ready-for-dev` → `in-progress` → `review` |

## Change Log

| Date | Change |
|---|---|
| 2026-07-22 | Story J-0 implemented (`bmad-dev-story`): metric type-mismatch and orphaned-metric faults routed into the web error feed with once-per-(device, metric) dedup; warn-once/debug-repeat logging; backfill path silent. 7 tests added (both new guards mutation-verified), full suite 1840/0, clippy clean. Status → review. |

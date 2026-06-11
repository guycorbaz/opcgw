# Story E.1: Uplink-Event Ingestion — last-known value for all measurements (no aggregation)

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->
<!-- This story is a v2.2.0 RELEASE BLOCKER (#130). It MUST be done before tagging v2.2.0 stable. -->
<!-- AC#11 real-world validation gate (Task 9) is a manual hardware test pending before this story may flip to `done`:
     rc2 de-aggregated-read strong PASS (2026-06-09) + rc4 valve-cycle PASS (2026-06-11) recorded; the 2026-06-11
     backfill code is new and needs the next rc's fleet sign-off (cold-start values present right after boot). -->

## Story

As an **opcgw operator**,
I want the gateway to ingest ChirpStack's decoded uplink events and expose each device value as its **last-known value with the device's source timestamp**,
so that OPC UA reflects the true device state (discrete *and* analog) **without any gateway-side aggregation** — the SCADA does any averaging/trending.

## Context & Why This Story Exists

The 2026-06-08 Tonhe valve test (against v2.2.0-rc1 in pre-prod) proved a structural flaw, tracked as **GitHub #130**: opcgw's only device-value path is the metrics poll (`GetMetrics`, `src/chirpstack.rs:2376`), which **time-aggregates** every uplink in a bucket by measurement kind (Gauge→average, Absolute→sum, Counter→delta). A discrete state has no meaningful average/sum, so the valve produced impossible values:

- `valveStatusCode = 391` (Absolute → `196 closing + 195 closed` summed)
- `valvePosition = 1.5`, `moving = 1.5` (Gauge → averaged)

No measurement kind fixes this — every kind aggregates. The same flaw applies to **all** points; analog sensors merely hide it (a short-window average ≈ the last reading, and the reported timestamp is the **poll time**, not the device's report time).

**Locked principle (#130, Guy's directive):** a SCADA/OPC UA gateway exposes the **raw last-known value** of every measurement + the device's **source timestamp** + **quality**, and performs **no aggregation**. Aggregation/trending is the SCADA's job.

This story makes the **gRPC uplink event stream the canonical last-value path for all measurements**. ChirpStack's `InternalService.StreamDeviceEvents` delivers each decoded uplink object verbatim (no bucketing). opcgw already consumes this stream for the web inventory (`src/chirpstack_inventory.rs:373`); E-1 wires a long-lived version into the runtime, stores the last decoded value per field with the device's event timestamp, and stops exposing aggregated `GetMetrics` values for any stream-covered field.

> ⚠️ **CRITICAL ARCHITECTURE FINDINGS (verified in code 2026-06-08) — read before coding.**
>
> 1. **Two different ChirpStack APIs, two different services.** Values today come from `DeviceServiceClient::get_metrics` (aggregated). The event stream is `InternalServiceClient::stream_device_events(StreamDeviceEventsRequest{dev_eui})` returning a stream of `LogItem`; the decoded object is JSON inside `LogItem.body` at key `object`, and the event time is `LogItem.time` (a `prost_types::Timestamp`). Filter `LogItem.description == "up"`. See `src/chirpstack_inventory.rs:404-414` (call) + `:481-574` (`log_item_to_uplink`).
>
> 2. **`StreamDeviceEvents` is PER-DEVICE.** There is no application-level gRPC event stream in `InternalService`. E-1 therefore runs **one long-lived stream per configured device**, each with its own reconnect/backoff. With many devices this is many concurrent streams → config must scope which devices stream (AC#8). The existing inventory consumer opens a stream, collects *N* recent items, then **returns** — E-1 needs the **long-lived** variant that never stops until cancelled.
>
> 3. **The write timestamp is the trap.** The current poll write stamps every value with `SystemTime::now()` (poll time) at `src/chirpstack.rs:1624`, and `MetricValue.timestamp` (`src/storage/types.rs:79-99`, `DateTime<Utc>`) carries it. The single-metric `set_metric` trait method stamps at call time with **no timestamp parameter**. E-1 **must** write with the device event time (`LogItem.time`) — use the batch path (`BatchMetricWrite.timestamp`, `src/storage/mod.rs:144-153`) or extend the write API. If E-1 stamps `now()`, the whole point is lost.
>
> 4. **The OPC UA `source_timestamp` is also `now()` today.** `get_value` sets `source_timestamp: Some(DateTime::now())` (`src/opc_ua.rs` ~`:1464`) instead of the metric's timestamp. #130 requires the *device's* source timestamp be exposed — fix this to `metric_value.timestamp`. (Quality via `compute_status_code` already uses `metric.timestamp`, so quality becomes correct automatically once E-1 writes event times.)
>
> 5. **The poll must not clobber the stream.** If both the `GetMetrics` poll and the stream write the same metric name, the poll's newer-but-aggregated `now()` value would overwrite the stream's correct value. E-1 must make the stream **authoritative** for stream-covered fields and stop the poll from writing them (see [Design Decisions](#design-decisions-confirm-before-or-during-dev)). The poll's **non-value** duties (server-availability `cp0`, error counts, pruning) stay.

## Acceptance Criteria

1. **Live uplink-event ingestion task.** A new runtime task consumes `InternalService.StreamDeviceEvents` for each configured device, spawned from `main.rs` alongside the metrics poller, sharing the `Arc<dyn StorageBackend>` and the `CancellationToken`; it reuses the `src/chirpstack_inventory.rs` stream-open/parse patterns (`InternalServiceClient`, Bearer interceptor, `LogItem` → JSON `body.object`). It shuts down cleanly on Ctrl+C / cancel (no orphaned tasks; mirrors the poller's shutdown).

2. **Last-known value, no aggregation.** For each `up` event, every configured `read_metric` whose `chirpstack_metric_name` matches a field of the decoded object (`body.object`) is stored as its **raw last value** — never averaged/summed/delta'd. **opcgw exposes no `GetMetrics`-aggregated value on OPC UA for any stream-covered field** (see AC#7).

3. **Device source timestamp + quality.** The stored `MetricValue.timestamp` is the device event time (`LogItem.time`), not ingest/poll time. The OPC UA read path exposes that timestamp as the `DataValue.source_timestamp` — fix `src/opc_ua.rs` `get_value` which currently sets `DateTime::now()` to use `metric_value.timestamp`. Quality (Good/Uncertain/Bad via `compute_status_code`, `src/opc_ua.rs:1811`) is therefore computed from real device-report age.

4. **All metric types, including String.** Decoded JSON fields are converted to the configured `metric_type` (`OpcMetricTypeConfig` Int/Float/Bool/String, `src/config.rs`). The **String path is implemented end-to-end** (storage `MetricType::String` + OPC UA `Variant::String` already exist; this is the first poller-side path to populate it). The stale GetMetrics rejection at `src/chirpstack.rs:1733` ("Reading string metrics… not implemented") is updated/removed since String now flows via the stream. Type mismatches (e.g. a configured Int field arriving as a JSON string) are logged and skipped, not panicked.

5. **Valve-class normalized status (concrete driver).** For a device bound to the valve class (the E-0 `command_class = "valve"` flag, resolved per [Design Decisions](#design-decisions-confirm-before-or-during-dev)), expose normalized status from the decoded object: a `ValveState` string (open / opening / closed / closing / blocked / fault / unknown) plus `Moving` / `Fault` / `LowBattery`. Map directly from the codec's already-normalized fields (`state`, `moving`, `fault`, `lowBattery`) — opcgw does not re-derive them. Generic (non-class) devices pass their configured fields through unchanged (additive, no regression).

6. **Resilience: reconnect + backoff.** A stream drop/error triggers reconnect with backoff mirroring the Epic 4 auto-recovery resilience; one device's stream failure must not kill the ingestion task or other devices' streams. The connection state is observable in logs (structured, field-style).

7. **Backfill rule — no aggregated value ever wins.** Define and enforce value-path precedence so the metrics poll cannot overwrite a stream-sourced value with an aggregated one. **Recommended:** the stream is authoritative for any field present in a device's decoded object; the `GetMetrics` poll **stops writing** those read_metrics (it retains only its non-value duties — `cp0` server-availability, error counts, pruning — and, optionally, fields explicitly marked poll-only/no-uplink-object, which remain clearly aggregated/legacy). On (re)connect, backfill the last value via the **bounded recent-events fetch** (the inventory-style `stream_recent_device_uplinks`, which returns the real last decoded object), **not** `GetMetrics`, so a correct value is present before the next live event. See [Design Decisions](#design-decisions-confirm-before-or-during-dev).

8. **Config to scope the stream.** Configuration controls which applications/devices are streamed (to bound concurrent stream count / event volume); the default behaviour is documented and backward-compatible with existing configs (no new required field).

9. **Automated tests.** Against a stub/in-process seam (introduce an injection point over the stream source + the decoded-object→storage mapping as a pure function, mirroring E-0's `DownlinkSink` approach):
   - (a) a decoded-object event → last-value Storage write **with the event timestamp** (assert the stored `MetricValue.timestamp == LogItem.time`, not `now()`);
   - (b) a String field → `MetricType::String` end-to-end (write + OPC UA `Variant::String`);
   - (c) a valve event → normalized `ValveState` + `Moving`/`Fault`/`LowBattery`;
   - (d) generic-device passthrough (configured numeric fields ingested unchanged);
   - (e) reconnect after a simulated stream drop continues ingestion;
   - (f) backfill serves the last value before the first live event;
   - (g) **no-aggregation precedence**: a `GetMetrics` poll cycle does **not** clobber a fresher stream value for a stream-covered field;
   - (h) OPC UA `source_timestamp == metric event timestamp` and quality reflects age (Good within threshold, Uncertain/Bad when aged).

10. **Quality gates + docs.** SPDX headers on all touched/new files; `cargo test` and `cargo clippy --all-targets -- -D warnings` clean. Update: `README.md`; `config/config.toml` + `config/config.example.toml` (stream-scope config + corrected valve metric mapping — `valveStatusCode` etc. now come from the event stream, drop the stale `batteryLevel` GetMetrics mapping); DocBook manual (`docs/manual/opcgw-user-manual.xml`); `docs/architecture.md` (data flow: event stream is the canonical last-value path, poll demoted to backfill/health); `docs/LoRa/TONHE Valve/README.md` §5 (the poll-aggregation warning is now resolved by E-1). Update `docs/logging.md` if any new structured/audit events are introduced.

11. **Real-world validation gate (blocks `done`).** Per the main-deadlock incident doctrine ([[incident_main_deadlock_2026_05_20]]): against the **live ChirpStack + a physical Tonhe E20 valve**, confirm OPC UA shows the **true discrete valve state** (open/closed) updating on real uplinks, carrying the device's source timestamp, and that **no aggregated value (e.g. `391` / `1.5`) ever appears**. Also confirm at least one analog sensor (e.g. water level or temperature) shows its real last reading via the stream. Record the outcome in Completion Notes. Automated tests + clippy passing is **not** sufficient.

> **Release gate (#130):** this story must reach `done` before tagging **v2.2.0** stable. v2.2.0-rc1 must not be promoted to production while opcgw exposes aggregated values in the value path.

## Tasks / Subtasks

> **Phasing (dev-story split, 2026-06-09, user-confirmed):** this run implements **E-1a** — stream ingestion + last-value store with the device event timestamp + OPC UA source-timestamp fix + valve mapping + no-aggregation precedence for valve-class devices. **E-1b** (migrate all non-valve analog sensors off the poll + explicit stream-scope config + backfill-via-recent-events + integration/real-world gates) is left unchecked; the story **stays `in-progress`** until E-1b lands. Both are required before v2.2.0.

- [x] **Task 1 — Long-lived per-device event-stream consumer (AC: 1, 6)** — `src/chirpstack_events.rs`: `run_event_ingestion` supervisor spawns one `run_device_stream` per valve-class device; reconnect with capped-exponential backoff; `tokio::select!` on `CancellationToken`.
  - [x] Long-lived stream loop (`connect_and_stream`) reusing the `chirpstack_inventory` pattern (`InternalServiceClient::stream_device_events`, filter `"up"`, `body.object` + `LogItem.time` via `parse_up_event`).
  - [x] Spawned from `main.rs` (per-task `SqliteBackend`, Story 5-1 pattern) + joined at shutdown.
  - [x] Reconnect/backoff; per-device failure isolated (supervisor + other devices unaffected).
  - [x] `CancellationToken` honoured; no orphaned tasks.
- [x] **Task 2 — Decoded-object → last-value Storage write with event timestamp (AC: 2, 3, 4)** — `map_uplink_to_writes` (pure) + `json_to_metric`.
  - [x] Pure mapping fn (takes `&[ReadMetric]` for testability); keyed by `chirpstack_metric_name` (the storage key the poll writes + OPC UA reads).
  - [x] Writes via `BatchMetricWrite.timestamp = event_time.into()` (device time, NOT `now()`).
  - [x] JSON conversion number→Int/Float, bool/0-1→Bool, str→String; mismatch → `uplink_field_type_mismatch` warn + skip (no panic).
- [x] **Task 3 — String metric path end-to-end (AC: 4)** — stream populates `MetricType::String` (test `string_field_maps_end_to_end`); OPC UA `Variant::String` read already supported. (The poll-path String rejection at `chirpstack.rs:1733` stays — it's only reached by poll-served non-valve devices, accurate until E-1b.)
- [x] **Task 4 — Valve status mapping (AC: 5)** — decoded `state`/`valveStatusCode`/`valvePosition`/`moving`/`fault`/`lowBattery` flow to their configured `read_metric`s with correct types (tests `valve_flags_map_to_bool_and_int`, `maps_each_field_with_event_timestamp`). NOTE: separate **canonically-named** `ValveState` nodes are deferred to E-2's class registry — in E-1a the operator's configured read_metrics carry the normalized values.
- [x] **Task 5 — No-aggregation precedence: demote the poll (AC: 2, 7)**
  - [x] `poll_metrics` **skips streamed devices** (`device_is_streamed`) so the stream is the sole writer — no aggregated value reaches OPC UA for them.
  - [x] Stop poll writes for *all* devices — via the fleet-wide `chirpstack.stream_all_devices` toggle (E-1b mechanism, 2026-06-09): when set, every device with read_metrics streams and the poll skips it.
  - [x] Startup/reconnect backfill via the bounded recent-events fetch (2026-06-11): `backfill_device` runs after every successful stream connect, fetches the newest recent event via `chirpstack_inventory::stream_recent_device_uplinks` (limit 5 / 3 s, never `GetMetrics`), and writes under the `is_fresher` timestamp guard so an older backfill can never clobber a newer live value (connect-first ordering means no live event is missed; the guard resolves the overlap in the stream's favour).
  - [x] **Orphan-warn** (2026-06-09): after `ORPHAN_WARN_AFTER_EVENTS` (3) uplinks, `uplink_metric_never_seen` warns once per configured field never seen in the device's object — flags DevStatus-battery / field-name-mismatch during the fleet migration. Pure `newly_orphaned` + test.
- [x] **Task 6 — OPC UA source timestamp fix (AC: 3)** — `get_value` now sets `source_timestamp = DateTime::from(metric_value.timestamp)`; server_timestamp stays `now()`. Test `source_timestamp_is_device_event_time_not_now`.
- [x] **Task 7 — Stream-scope config (AC: 8)** — fleet-wide `chirpstack.stream_all_devices` toggle (`#[serde(default)]` false; env `OPCGW_CHIRPSTACK__STREAM_ALL_DEVICES`; restart-required, no schema migration — K/V singleton). Per-device granularity + orphan-warn for never-seen fields deferred (refinement).
- [x] **Task 8 — Tests (AC: 9)** — ✅ pure mapping ×6 (a,b,c,d + float-coercion + absent-field + type-mismatch) + ✅ `source_timestamp` (h) [E-1a]. ✅ (e)/(f)/(g) added 2026-06-11 against the new injectable `UplinkSource`/`UplinkStream` trait seam (AC#9's prescribed stub approach, mirroring E-0's `DownlinkSink`): `reconnect_after_stream_drop_continues_ingestion` (e), `backfill_serves_last_value_before_first_live_event` (f — asserts the stored timestamp is the DEVICE event time), `older_backfill_never_clobbers_fresher_stream_value` (g — precedence: the poll never writes streamed devices per `should_stream_routing`; backfill is the only remaining non-live write path and the guard pins it), plus `is_fresher_guard_boundaries` (cold-start / newer / equal / older). `cargo test` lib 585/0 (+4 new this slice).
- [ ] **Task 9 — Real-world validation (AC: 11)** — *substantially passed; final rc sign-off pending.* ✅ 2026-06-09 on v2.2.0-rc2 pre-prod (`stream_all_devices=true`): valve `valveStatusCode=193`/`valvePosition=1` (true state, no `391`/`1.5`), analog sensors (meteo02/flow001) stream real last-values, stale sensors correctly read Uncertain from device-report age — AC#11 de-aggregated read path = strong PASS. ✅ 2026-06-11 on rc4: full Fuxa-driven valve OPEN+CLOSE cycle with true state + fresh timestamps (E-0 AC#10 test, same read path). ⏳ The **backfill code added 2026-06-11 is new** — per the main-deadlock doctrine, a final fleet sign-off on the next rc (cold-start: values present right after boot without waiting a report interval) is the remaining **gate for `done`**.
- [x] **Task 10 — Docs + quality (AC: 10)** — ✅ `docs/logging.md` (6 `uplink_*` events [E-1a] + 4 `uplink_backfill*` events [2026-06-11]), `docs/LoRa/TONHE Valve/README.md` §5 (E-1b implemented), `docs/architecture.md` (module + data flow incl. seam + backfill), `README.md` (Current Version + Epic E planning row), `config/config.toml` (`stream_all_devices` knob), `config/config.example.toml` (valve blocks corrected: `command_class="valve"` on all three valve commands, stale DevStatus `batteryLevel` mappings dropped with explanatory note, Vanne03 status Float→Int, richer `state`/`valvePosition`/`lowBattery` mapping example), DocBook manual (value-path explanation in `[chirpstack]`, source-timestamp staleness + per-device threshold guidance, `command_class` stream-routing note; `xmllint` clean). SPDX present; `cargo clippy --all-targets -- -D warnings` clean.

### Review Findings (iter-1, 2026-06-11 — 3 parallel layers on Opus; implementing model Fable 5)

- [x] [Review][Patch] **MEDIUM (edge): live stream writes are unguarded — ChirpStack replays recent event history on every (re)connect, transiently regressing last-values** [src/chirpstack_events.rs `ingest_event`] — FIXED: `filter_fresher_writes` shared helper (factored from the backfill) now guards the live pump too; new `uplink_replay_skipped` debug event; regression test `replayed_older_live_event_never_regresses_stored_value`.
- [x] [Review][Patch] **MEDIUM (edge): same DevEUI configured under two applications spawns two duplicate streams** [src/chirpstack_events.rs `streamed_devices`] — FIXED: HashSet dedup + debug log; test `streamed_devices_dedups_cross_application_deveui`.
- [x] [Review][Patch] **LOW (edge+blind): per-device stale_threshold_seconds lacks range validation and the SQLite load path wraps negative i64 → huge u64** [src/config.rs validate; src/storage/sqlite.rs load] — FIXED: (0, 86400] validation per device mirroring the global knob; `.filter(|v| *v >= 0)` on the load cast.
- [x] [Review][Patch] **LOW (blind): Bool coercion accepts any non-zero integer, contradicting the documented 0/1 contract** [src/chirpstack_events.rs `json_to_metric`] — FIXED: strict 0/1; other integers → type-mismatch warn; test `bool_coercion_is_strictly_zero_or_one`.
- [x] [Review][Patch] **LOW (blind): Int-from-float coercion silently truncates fractional values** [src/chirpstack_events.rs `json_to_metric`] — FIXED: f64 accepted only when integral and within 2^53 exact range; test `int_coercion_rejects_fractional_floats`.
- [x] [Review][Patch] **LOW (blind): side-effecting `seen.insert() && warned.remove()` short-circuit chain is fragile** [src/chirpstack_events.rs `ingest_event`] — FIXED: explicit `first_sighting` statements.
- [x] [Review][Patch] **LOW (blind): an `up` event with missing/malformed proto timestamp is dropped with zero diagnostic** [src/chirpstack_events.rs `parse_up_event`] — FIXED: new `uplink_event_dropped` warn (reasons `unparseable_body` / `malformed_proto_timestamp`) + logging.md row.
- [x] [Review][Patch] **LOW (blind): schema test assertion messages still say v011 after the v012 bump** [src/storage/schema.rs multiple] — FIXED: 6 message strings corrected to v012.
- [x] [Review][Patch] **LOW (blind): initial `source.connect()` is not cancellation-aware — shutdown can block on a hanging connect** [src/chirpstack_events.rs `connect_and_stream`] — FIXED: connect wrapped in `tokio::select!` with the cancel token.
- [x] [Review][Defer] **LOW (edge): a panicking per-device stream task is not restarted** [src/chirpstack_events.rs supervisor] — deferred: no panicky path in the loop body; panic-supervision absent project-wide (poller identical); robustness follow-up.
- [x] [Review][Defer] **LOW (blind): API token cloned into every per-device task, no zeroization** [src/chirpstack_events.rs] — deferred: same pattern as poller/inventory interceptors; zeroization is a project-wide hardening item.
- [x] [Review][Defer] **LOW (blind): `uplink_field_type_mismatch` warns on every uplink for a persistently mistyped field (no once-per-field dedup)** [src/chirpstack_events.rs] — deferred: LoRaWAN cadence keeps volume modest and persistent visibility of a misconfiguration is arguably desired; revisit with the orphan-warn pattern if it proves noisy.

Dismissed (8): BH "update_device drops stale_threshold" (verified false — `UPDATE devices SET device_name, updated_at` preserves the column; web surface for the field is a documented deferral); BH "topology-diff inclusion contradicts restart-required" (deliberate, documented in `bdb032e`); BH "no backfill on reconnect" (backfill is in this very diff, c18d289); BH "orphan-warn noise for intermittent fields" (designed self-correction, documented); edge "future source_timestamp on device clock skew" (`LogItem.time` is ChirpStack-server-stamped; #130 mandates the report time verbatim); AA AC#9(b) test-shape, AA AC#4 literal-text deviation, AA AC#5 canonical-node deferral (all documented deviations/cosmetic, behaviour verified present).

<details><summary>Original (pre-split) task detail — retained for E-1b</summary>

- [ ] **Task 1 — Long-lived per-device event-stream consumer (AC: 1, 6)**
  - [ ] Factor a long-lived stream loop out of / alongside the bounded `stream_recent_device_uplinks` (`src/chirpstack_inventory.rs:373-470`): open `InternalServiceClient::stream_device_events`, iterate `stream.message()` until cancelled, parse via `log_item_to_uplink`-style logic (filter `description == "up"`, extract `body.object` + `LogItem.time`).
  - [ ] Spawn one task per configured (scoped) device from `main.rs` near the poller spawn (`src/main.rs:1064`), passing `Arc<dyn StorageBackend>` + `cancel_token.clone()`. Decide single-supervisor-task-fans-out vs one-tokio-task-per-device; document the choice.
  - [ ] Reconnect with backoff on `Err`/stream-close (mirror Epic 4 resilience); a per-device failure is logged and retried without affecting other devices or aborting the supervisor.
  - [ ] Honour `CancellationToken` in a `tokio::select!` so all streams stop on shutdown (no orphaned tasks).
- [ ] **Task 2 — Decoded-object → last-value Storage write with event timestamp (AC: 2, 3, 4)**
  - [ ] Pure mapping fn: `(device_id, decoded_object: &serde_json::Value, event_time: DateTime<Utc>, &AppConfig) -> Vec<BatchMetricWrite>` — for each configured `read_metric` of the device, look up its field in `object` via `config.get_metric_type(chirpstack_metric_name, device_id)`, convert the JSON value to the configured `MetricType`, and stamp `timestamp = event_time`.
  - [ ] Write via the batch path (`BatchMetricWrite.timestamp`, `src/storage/mod.rs:144-153`) so the **device event time** is persisted (NOT `SystemTime::now()` as the poll does at `src/chirpstack.rs:1624`). If a single-metric write is used, extend the trait to accept a timestamp.
  - [ ] JSON value conversion: number→Int/Float (per config), bool→Bool, string→String; mismatch → log + skip (no panic).
- [ ] **Task 3 — Implement the String metric path end-to-end (AC: 4)**
  - [ ] Confirm `MetricType::String` writes through both backends and reads out as `Variant::String` (`src/opc_ua.rs` `convert_metric_to_variant`). Add coverage if any gap.
  - [ ] Update the stale rejection at `src/chirpstack.rs:1733` (now String flows via the stream).
- [ ] **Task 4 — Valve-class normalized status (AC: 5)**
  - [ ] Resolve whether a device is valve-class (reuse E-0's `command_class = "valve"` on `DeviceCommandCfg`, or a device-level tag — see Design Decision 3).
  - [ ] Map decoded `state`/`moving`/`fault`/`lowBattery` → canonical `ValveState` (String) + `Moving`/`Fault`/`LowBattery` metrics. Keep the mapping concrete here (E-2 lifts it into the registry); generic devices unaffected.
- [ ] **Task 5 — No-aggregation precedence: demote the poll (AC: 2, 7)**
  - [ ] Stop the `GetMetrics` poll value-write (`src/chirpstack.rs` ~1620-1745) for read_metrics that are stream-covered; retain the poll's `cp0`/error-count/prune duties.
  - [ ] Implement startup/reconnect **backfill via the bounded recent-events fetch** (not `GetMetrics`).
  - [ ] Ensure no code path exposes an averaged/summed value on OPC UA for a stream-covered field.
- [ ] **Task 6 — OPC UA source timestamp fix (AC: 3)**
  - [ ] In `get_value` (`src/opc_ua.rs` ~`:1464`) set `source_timestamp = metric_value.timestamp` instead of `DateTime::now()`. Verify `compute_status_code` quality still behaves (it already reads `metric.timestamp`).
- [ ] **Task 7 — Stream-scope config (AC: 8)**
  - [ ] Add backward-compatible config to enable/scope streaming (which applications/devices). Document default. Round-trips through both config sources (TOML bootstrap + SQLite singleton per Epic D) with `#[serde(default)]`.
- [ ] **Task 8 — Tests (AC: 9)** — implement (a)–(h) using the parse-level pure-fn seam + an injectable stream source; keep `cargo test` green, add net-new tests.
- [ ] **Task 9 — Real-world validation (AC: 11)** — against live ChirpStack + physical valve: verify true discrete state + real analog last-value over OPC UA, correct source timestamps, no aggregated values. Record in Completion Notes. **Gate for `done`.**
- [ ] **Task 10 — Docs + quality (AC: 10)** — README, config.toml/example, DocBook manual, `docs/architecture.md`, `docs/LoRa/TONHE Valve/README.md` §5, `docs/logging.md` (if new events); SPDX; `cargo test`; `cargo clippy --all-targets -- -D warnings`; `xmllint` the manual.

</details>

## Design Decisions (confirm before or during dev)

1. **Stop the poll value-write vs. dedupe at write time.** *Recommended:* make the stream the sole writer for any field present in the decoded object and **stop the poll from writing those read_metrics** — simplest guarantee that no aggregated value reaches OPC UA. Alternative (let both write, drop the older/aggregated at write time) is more code and more failure modes. Either way, AC#7 must hold: no averaged/summed value on OPC UA for a covered field.

2. **Backfill source.** *Recommended:* backfill the last value on startup/reconnect via the **bounded recent-events fetch** (`stream_recent_device_uplinks`, which returns the real last decoded object — no aggregation). Do **not** backfill via `GetMetrics` (that re-introduces aggregation). Fields with genuinely no uplink-object source (if any remain) stay on the poll and are documented as aggregated/legacy.

3. **How is "valve-class" resolved for uplink mapping?** *Recommended:* reuse E-0's `command_class = "valve"` on `DeviceCommandCfg` (`src/config.rs`) — a device with a valve command is valve-class. Alternative (device-level kind tag) overlaps E-2; avoid introducing it here. Confirm: a pure-sensor valve with no command is unlikely, so command-derived class is sufficient for E-1; E-2 generalizes.

4. **Per-device task vs. single supervisor.** *Recommended:* one supervisor task that spawns/owns a child stream per scoped device (bounded, observable, clean cancel). Document the cap and what happens when a device is added/removed via hot-reload (config_rx) — or explicitly defer hot-reload of the stream set to a follow-up if it bloats scope.

5. **Split E-1a/E-1b?** This story spans stream infra + valve + migrating *all* analog sensors off the poll. If it proves too large for one dev cycle, split into **E-1a** (stream mechanism + last-value store + event timestamp + source-timestamp fix + valve) and **E-1b** (migrate all read_metrics off the poll + backfill + disable poll value-write). **Both are required before v2.2.0** (the release gate is "no aggregation anywhere"), so a partial E-1a alone does not unblock the tag.

## Dev Notes

### Exact source anchors (verified 2026-06-08)
- **Event stream consumer (reuse):** `src/chirpstack_inventory.rs:404-414` (`InternalServiceClient::stream_device_events(StreamDeviceEventsRequest{dev_eui})`, Bearer interceptor `:347-361`); iteration `:421-445` (`stream.message()`); `log_item_to_uplink` `:481-574` (filter `description=="up"`, JSON `body.object`, `LogItem.time` → `DateTime<Utc>`). Proto types from `crate::chirpstack_internal_proto::api` (`LogItem`, `StreamDeviceEventsRequest`).
- **Poller task / runtime:** `ChirpstackPoller` struct `src/chirpstack.rs:383-420` (fields incl. `backend: Arc<dyn StorageBackend>`, `cancel_token`, `config_rx`); ctor `new_with_reload` `:485-518`; spawn in `src/main.rs:951-968` + `:1064-1068` (`tokio::spawn(poller.run())`); `CancellationToken` `src/main.rs:546`, import `:78`; channel creation `src/chirpstack.rs:541-575`.
- **Current metrics→storage mapping (to demote):** `src/chirpstack.rs:1620-1745`; `raw_value = metric.datasets[0].data[0]` `:1639`; `config.get_metric_type(&metric_name, &device_id_string)` `:1661`; type match + **String rejection** `:1733-1736`; poll timestamp `let now_ts = SystemTime::now();` `:1624`; `get_metrics` call `:2376`.
- **Config:** `ReadMetric { metric_name, chirpstack_metric_name, metric_type, metric_unit }` `src/config.rs:670-691`; `OpcMetricTypeConfig {Bool,Int,Float,String}` `:639-663`; `get_metric_type(chirpstack_metric_name, device_id)` `~:2362`. `DeviceCommandCfg.command_class: Option<String>` from E-0 (`src/config.rs:693`).
- **Storage:** `MetricValue { device_id, metric_name, timestamp: DateTime<Utc>, data_type: MetricType }` `src/storage/types.rs:79-99`; `BatchMetricWrite { device_id, metric_name, data_type, timestamp: SystemTime }` `src/storage/mod.rs:144-153`; `get_metric_value` returns `MetricValue` `src/storage/mod.rs:211-243`. `StorageBackend` trait + in-memory (`src/storage/memory.rs`) + SQLite (`src/storage/sqlite.rs`) must stay in lockstep.
- **OPC UA:** `compute_status_code` `src/opc_ua.rs:1811-1832` (uses `metric.timestamp`; `DEFAULT_STALE_THRESHOLD_SECS=120` `:37`, `STATUS_CODE_BAD_THRESHOLD_SECS=86400` `:39`); `get_value` `:1314-1464`, **source_timestamp = `DateTime::now()` to fix** `~:1464`; `convert_metric_to_variant` (incl. `Variant::String`) `~:1862-1935`.

### Tonhe valve decoded object (the concrete driver)
The codec (`docs/LoRa/TONHE Valve/tonhe-e20-valve-codec.js`, updated commit `fc84bc3`) emits, per `up` event on fPort 10: integers `valveStatusCode`, `valvePosition`, `moving`, `fault`, `lowBattery`, plus strings `state`, `statusText`. E-1 reads these straight from `body.object` — **no aggregation** — so `valveStatusCode` reads the true byte (e.g. `195` closed) and `state` reads `"closed"`. This is exactly the value the 2026-06-08 poll path corrupted to `391`/`1.5`.

### Latest tech notes (verified 2026-06-08, no new deps)
- `chirpstack_api = "4.17.0"`, `tonic = "0.14.5"`, `prost`/`prost-types = "0.14"`, `tokio = "1.50.0"`, `tokio-util = "0.7.18"` (`CancellationToken`), `chrono = "0.4.26"`, `async-trait = "0.1.81"` (used for E-0's `DownlinkSink`; reuse the trait-seam pattern for the stream source).
- `LogItem.time` is `Option<prost_types::Timestamp>`; convert with `DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32)` (guard ranges as `log_item_to_uplink` does, `src/chirpstack_inventory.rs:~520`).
- The decoded object lives in `LogItem.body` as a JSON string (`serde_json::from_str`), key `object` — it is **not** a prost Struct on this path (unlike the downlink enqueue object in E-0).

### Migration surface / risk (from config.example.toml; verify against the LIVE config)
Devices currently on the aggregated poll that move to the stream: water/tank level + battery/current/voltage (`Niveau_citerne`), valves (`valveStatusCode` etc.), SHT temp/humidity + `BatV` (Magasin/Grange/Tunnel1), soil sensor (Verger2), and the meteo station's ~10 fields (rain/temp/humidity/pressure/wind/UV/light/battery). **Risk:** a `chirpstack_metric_name` that is *not* a codec-decoded object field (e.g. ChirpStack-native `rssi`/`snr`, or a device-info `batteryLevel` that comes from DevStatus rather than the uplink object) will not appear in `body.object` and would orphan. AC#11 must validate at least one analog sensor live; flag any orphaned mapping (keep on poll-as-legacy or drop) during dev.

### Architecture compliance
- `OpcGwError` (`src/utils.rs`) via `thiserror`; reuse `ChirpStack`/`Storage`/`Configuration` variants; no new panics.
- Structured tracing, field-style, consistent with existing call sites; add `config/log4rs.yaml` levels only if a new module is introduced (a new `src/chirpstack_events.rs`-style submodule is reasonable — keep it under the `chirpstack` logging target or document a new target in `docs/logging.md`).
- Storage stays behind `StorageBackend`; both backends in lockstep.
- Config backward-compatible (`#[serde(default)]`); Epic D made SQLite authoritative for singletons but `[[application.device.read_metric]]` arrays are config/TOML-seeded — verify deserialization.

### Out of scope (do NOT do here)
- **E-2 device-class registry** — E-1 keeps the valve mapping concrete (reusing E-0's `command_class` flag); do not build the registry.
- **E-3 command delivery confirmation** (`CommandStatusPoller`) — uplink ingestion only; do not correlate command acks here.
- **MQTT** (`CR-EPIC-C-MQTT`) — Route B uses gRPC `StreamDeviceEvents`; not reopened.
- **Proportional/position actuators** — valve is binary open/close.
- Deleting the `GetMetrics` poll entirely — it retains `cp0` server-availability + error-count + pruning duties; only its **device-value writes for stream-covered fields** are removed.

### Project Structure Notes
- New work likely in: a new/extended stream consumer (factor the long-lived loop near `src/chirpstack_inventory.rs` or a new `src/chirpstack_events.rs`), `src/chirpstack.rs` (poll demotion + task spawn wiring), `src/main.rs` (spawn), `src/opc_ua.rs` (source_timestamp fix), `src/config.rs` (stream-scope), docs. Keep any test seam (stream-source trait) co-located, not scattered (mirror E-0's `DownlinkSink`).
- SPDX header (`MIT OR Apache-2.0`) + `(c) [2026] Guy Corbaz` on every new/edited source file.

### References
- [Source: _bmad-output/planning-artifacts/epics.md#Epic E] — Story E.1 (elevated scope), "no gateway-side aggregation" locked decision, Route note, DoD release-gate (lines ~1360-1454).
- [Source: _bmad-output/planning-artifacts/sprint-change-proposal-2026-06-08.md] — full #130 impact analysis + approach.
- [Source: _bmad-output/implementation-artifacts/E-0-downlink-command-path.md] — `command_class` flag, `DownlinkSink` test-seam pattern, prost/tonic notes.
- [Source: src/chirpstack_inventory.rs:373-574] — existing `StreamDeviceEvents` consumer to reuse.
- [Source: src/chirpstack.rs:1620-1745,2376,1624] — poll mapping + timestamp to demote/fix.
- [Source: src/storage/types.rs:79-99; src/storage/mod.rs:144-153] — `MetricValue`/`BatchMetricWrite` timestamp.
- [Source: src/opc_ua.rs:1811-1832,~1464] — quality + source_timestamp.
- GitHub issues #130 (release blocker), #129 (Epic E); memory `project_device_abstraction_valves` (2026-06-08 update); main-deadlock doctrine `incident_main_deadlock_2026_05_20`.

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (1M context) — bmad-dev-story, 2026-06-09 (E-1a slice).
claude-fable-5 — bmad-dev-story, 2026-06-11 (E-1b remainder: backfill + seam tests + doc-sync).

### Debug Log References

- `cargo check --all-targets` → clean.
- `cargo clippy --all-targets -- -D warnings` → clean.
- `cargo test` → all binaries pass, 0 failed (lib 597 incl. 7 new: 6 `chirpstack_events` mapping tests + 1 `opc_ua` source-timestamp test).

### Completion Notes List

**E-1a implemented (story split — E-1b deferred, story stays `in-progress`).** Design decisions DD1–DD4 taken as recommended; DD5 split confirmed with the user.

- **Task 1/2 (AC#1,2,3,4)** — New `src/chirpstack_events.rs`. `run_event_ingestion` supervisor spawns one `run_device_stream` per **valve-class** device (E-1a scope), each a long-lived `InternalService.StreamDeviceEvents` consumer with capped-exponential reconnect backoff and `CancellationToken` shutdown (DD4: supervisor + per-device child tasks; stream-set fixed from the startup snapshot, hot-reload deferred). The pure `map_uplink_to_writes(device_id, &[ReadMetric], &object, event_time)` maps each configured `read_metric` present in the decoded object to a `BatchMetricWrite` stamped with the **device event time** (`LogItem.time`) — no aggregation — keyed by `chirpstack_metric_name` (the same storage key the poll writes and `OpcUa::get_value` reads). `json_to_metric` coerces number↔Int/Float, integer-0/1→Bool, str→String; mismatches emit `uplink_field_type_mismatch` and skip.
- **Task 3 (AC#4)** — String metrics now populate poller-side via the stream (`MetricType::String`), read back as `Variant::String`. The poll-path String-rejection (`chirpstack.rs:1733`) is left intact: it is only reachable by poll-served (non-valve) devices and stays accurate until E-1b.
- **Task 4 (AC#5)** — Valve decoded fields (`state`, `valveStatusCode`, `valvePosition`, `moving`, `fault`, `lowBattery`) flow to their configured read_metrics with correct types. Canonically-named `ValveState` nodes are an E-2 registry concern; in E-1a the configured read_metric names carry the normalized values.
- **Task 5 (AC#2,7 — partial, valve scope)** — `poll_metrics` skips valve-class devices (`device_is_valve_class`) so the stream is the **sole, authoritative writer** for valves and no `GetMetrics`-aggregated value (the `391`/`1.5` bug) ever reaches OPC UA for them. Full demotion for all devices + explicit backfill = **E-1b**.
- **Task 6 (AC#3, DD per #130)** — `OpcUa::get_value` now sets the OPC UA `source_timestamp` from `metric_value.timestamp` (device report time) instead of `DateTime::now()`; `server_timestamp` stays `now()`. Quality (`compute_status_code`) already keyed off `metric.timestamp`, so it now reflects real device-report age.
- **Wiring** — `chirpstack_events` declared in `main.rs` + `lib.rs`; ingestion task spawned alongside the poller (independent `SqliteBackend` per task) and joined at shutdown.

**No new dependencies.** No schema migration. New structured events documented in `docs/logging.md`.

**E-1b remainder implemented 2026-06-11 (claude-fable-5):**

- **Backfill (Task 5 final subtask, AC#7)** — new `UplinkSource`/`UplinkStream` trait seam over the gRPC stream (the AC#9-prescribed injection point, mirroring E-0's `DownlinkSink`); `GrpcUplinkSource` is the production impl (its `recent()` reuses `chirpstack_inventory::stream_recent_device_uplinks`, limit 5 / 3 s — real decoded events, never `GetMetrics`). After every successful stream connect, `backfill_device` stores the newest recent event under the **`is_fresher` timestamp guard**: connect-first ordering means no live event can be missed, and the guard means an older backfill can never clobber a newer live value (equal timestamps are skipped — no pointless churn). Backfill failures are logged (`uplink_backfill_failed` warn) and swallowed — best-effort; the live stream is canonical. 4 new structured events (`uplink_backfill` info / `_empty` debug / `_skipped` debug / `_failed` warn) documented in `docs/logging.md`.
- **Seam tests (Task 8, AC#9 e/f/g)** — scripted `ScriptedSource`/`ScriptedStream` stubs; tests pin reconnect-continues-ingestion (e), backfill-serves-last-value-with-device-timestamp (f), and older-backfill-never-clobbers-fresher-stream-value (g — the precedence test; the GetMetrics poll never writes streamed devices per `should_stream_routing`, so backfill is the only non-live write path and the guard pins it). Plus `is_fresher_guard_boundaries`. The live-event pump body was factored into `ingest_event` (pure refactor, behaviour unchanged).
- **Doc-sync (Task 10 remainder)** — README (Current Version + Epic E row), `config/config.toml` (`stream_all_devices` knob + caveat), `config/config.example.toml` (valve blocks corrected: `command_class="valve"` on all 3 valve commands, stale DevStatus `batteryLevel` mappings dropped, Vanne03 `Float`→`Int`, `state`/`valvePosition`/`lowBattery` example), DocBook manual (value-path explanation, source-timestamp staleness + per-device threshold guidance, `command_class` stream-routing note; `xmllint` clean), `docs/architecture.md`, Tonhe README §5.

⚠️ **Sole remaining gate before `done` (Task 9 / AC#11):** real-world outcomes so far — 2026-06-09 rc2 de-aggregated-read **strong PASS** (valve true state 193/1, analog last-values, Uncertain-from-report-age) and 2026-06-11 rc4 full Fuxa-driven valve OPEN+CLOSE with true state. The **backfill code is new in this slice** and needs the next rc's fleet sign-off (cold-start: values present right after boot) per the main-deadlock doctrine. E-1 remains the **v2.2.0 release blocker** (#130).

> **Adapter note (2026-06-09 E-2 correct-course):** E-1b's uplink mapping must grow a **value-transform hook** (enum map / linear scale+offset / bitmask-shift) so **Tier-2 object-remap** devices (codec installed but uneditable) translate the vendor decoded-object into canonical values. E-1a (Tier-1 Tonhe valve) needs none. See `_bmad-output/planning-artifacts/sprint-change-proposal-2026-06-09.md` + Epic E Story E.2.

### File List

Added:
- `src/chirpstack_events.rs` — uplink event ingestion: pure mapping (`map_uplink_to_writes`, `json_to_metric`) + `device_is_valve_class`/`streamed_devices` + long-lived stream consumer (`run_device_stream`, `connect_and_stream`, `parse_up_event`) + supervisor (`run_event_ingestion`) + 6 unit tests.

Modified:
- `src/lib.rs` / `src/main.rs` — declare `chirpstack_events`; spawn `run_event_ingestion` (per-task SqliteBackend) alongside the poller + join at shutdown.
- `src/chirpstack.rs` — `poll_metrics` skips valve-class devices (no-aggregation precedence, E-1a).
- `src/opc_ua.rs` — `get_value` source_timestamp = device event time; new `source_timestamp_is_device_event_time_not_now` test.
- `docs/logging.md` — 6 new `uplink_*` operations-reference events.
- `docs/LoRa/TONHE Valve/README.md` — §5 E-1a "implemented for valves" note.
- `docs/architecture.md` — new `chirpstack_events.rs` module + data-flow section.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — E-1 ready-for-dev → in-progress (split note).
- *(E-1b mechanism, 2026-06-09)* `src/config.rs` — `stream_all_devices: bool` field on `ChirpstackPollerConfig`. `src/chirpstack_events.rs` — `should_stream` + `device_is_streamed` + generalized `streamed_devices` + `should_stream_routing` test + supervisor log copy. `src/chirpstack.rs` — poll skip now `device_is_streamed`. `config/config.example.toml` — `stream_all_devices` knob + battery-orphan caveat. Construction-site fixtures updated: `src/opc_ua_auth.rs`, `src/opcua_topology_apply.rs`, `src/web/{test_support,auth,mod}.rs`, `tests/opcua_history.rs`, `tests/opcua_dynamic_address_space_{apply,spike}.rs`, `tests/opc_ua_{connection_limit,security_endpoints}.rs`, `tests/opcua_subscription_spike.rs`.
- *(E-1b remainder, 2026-06-11)* `src/chirpstack_events.rs` — `UplinkSource`/`UplinkStream` trait seam + `UplinkEvent` + `GrpcUplinkSource`/`GrpcUplinkStream` (gRPC code moved behind the seam), `backfill_device` + `is_fresher` guard, `ingest_event` (factored pump body), `run_event_ingestion_with_source`, 4 new tests + scripted stubs. `docs/logging.md` — 4 `uplink_backfill*` event rows. `config/config.toml` — `stream_all_devices` knob. `config/config.example.toml` — valve blocks corrected (command_class, batteryLevel dropped, Float→Int, richer mapping example). `docs/manual/opcgw-user-manual.xml` — value-path para, staleness source-timestamp + per-device threshold, command_class stream note. `docs/architecture.md` — chirpstack_events section updated (seam + backfill, E-1b). `docs/LoRa/TONHE Valve/README.md` — §5 E-1b implemented note. `README.md` — Current Version + Epic E row. `_bmad-output/implementation-artifacts/sprint-status.yaml` — E-1 in-progress → review.

## Change Log

| Date | Change |
|------|--------|
| 2026-06-09 | Story E-1 dev-story **E-1a slice** implemented (user-confirmed split). New `chirpstack_events` module: long-lived `StreamDeviceEvents` ingestion for valve-class devices → last-known value with device source timestamp, no aggregation (#130); poll skips valve-class devices; OPC UA `source_timestamp` = device event time. 7 new tests; full suite 597/0 lib + all integration bins green; clippy `-D warnings` clean. Status stays **in-progress** (E-1b + AC#11 real-world gate pending; E-1 is a v2.2.0 release blocker). |
| 2026-06-09 | **E-1b mechanism slice** (release-scope = strict, Guy): new `chirpstack.stream_all_devices` config toggle (`#[serde(default)]` false; env override; restart-required; no schema migration). Generalized streaming + poll-skip from valve-class-only → `should_stream`/`device_is_streamed` (valve-class OR stream_all_devices, with metrics). Default off = zero behavior change; when on, every device de-aggregates to the event-stream last-value path. +1 test (`should_stream_routing`); lib 598/0 + all bins green; clippy clean. `config.example.toml` documents the knob incl. the DevStatus-battery orphan caveat. **Remaining E-1b:** orphan-warn for configured fields never seen on the stream, (re)connect backfill via recent-events fetch, per-device granularity, integration stream/reconnect tests, **AC#11 fleet validation against live ChirpStack** (the `done` gate). Story stays **in-progress**. |
| 2026-06-09 | Orphan-warn shipped (`uplink_metric_never_seen`, commit `350f064`). **rc2 cut** (`3a9f2b5`, Docker build green) and **validated on hardware** (pre-prod, `stream_all_devices=true`): valve `valveStatusCode=193` (open)/`valvePosition=1`, no `391`/`1.5`; analog sensors stream real last-values; stale sensors correctly read *Uncertain* (quality ages from device report time → source-timestamp fix confirmed). **AC#11 de-aggregated read path = strong PASS.** Surfaced: global 120 s stale threshold too tight for ~15–20 min LoRaWAN sensors → **per-device stale threshold filed as [#132](https://github.com/guycorbaz/opcgw/issues/132), folded into E-1b** (config field + v012 migration + getter wiring). Still open: orphan-grep capture, AC#10 (opcgw-driven valve OPEN/CLOSE). |
| 2026-06-11 | **Code review iter-1** (3 parallel layers on Opus; impl model Fable 5): 9 patches applied — headline = freshness guard extended to the LIVE pump (`filter_fresher_writes` shared with backfill; ChirpStack replays event history on every (re)connect, which previously regressed last-values transiently), cross-application DevEUI stream dedup, per-device threshold validation + negative-load guard, strict 0/1 Bool + integral-only Int coercion, explicit orphan-tracking statements, `uplink_event_dropped` diagnostic, v012 assertion-message fixes, cancellation-aware connect. +4 tests (lib module 16/16). 3 LOW defers in deferred-work.md, 8 dismissed (incl. Blind Hunter's false HIGH on `update_device` — verified the UPDATE preserves the threshold column). Dismissed Blind Hunter "no backfill" finding — backfill is in this diff. Iter-2 re-review MANDATORY per the iter-N+1 doctrine (iter-1 introduced new flow-control: live-path guard + dedup + cancellable connect). |
| 2026-06-11 | **E-1b remainder — story implementation COMPLETE → review.** New `UplinkSource`/`UplinkStream` injectable seam (AC#9's stub approach); `GrpcUplinkSource` production impl; **(re)connect backfill** via the bounded recent-events fetch under the `is_fresher` timestamp guard (AC#7 closed — connect-first ordering + guard = no missed events, no clobber); AC#9 (e)/(f)/(g) seam tests + guard boundary test (lib 585/0, +4). Full doc-sync: README, config.toml knob, config.example.toml valve corrections, DocBook manual (xmllint clean), architecture.md, Tonhe README, logging.md (4 backfill events). Remaining before `done`: AC#11 final fleet sign-off on the next rc (backfill is new code; rc2 read-path PASS + rc4 valve-cycle PASS already recorded). |
| 2026-06-09 | **Orphan-warn self-correction** + **#132 per-device stale threshold** implemented. (a) `uplink_metric_never_seen` softened to "not seen in the first uplinks…"; new `uplink_metric_now_seen` (info) clears the flag when an intermittent field later arrives (the `WaterFlowValue` case from the pre-prod logs). (b) #132: optional per-device `stale_threshold_seconds` on `ChirpstackDevice` (`#[serde(default)]`) — schema **migration v012** adds the `devices` column; threaded through `insert_device` + TOML→SQLite seed insert + `load_all_applications_config`; OPC UA resolves device-override → global → 120 s default and captures it in the read callback; `config_reload` topology-diff includes it (restart-required); round-trip test `test_e1_per_device_stale_threshold_roundtrip`; all schema version assertions 11→12. Web CRUD surface for the threshold deferred (consistent with command_class). lib green + all bins green; clippy `-D warnings` clean. Existing deploys: use the global `[opcua].stale_threshold_seconds` now; per-device override settable on fresh-seed / via future web. **Remaining E-1b:** (re)connect backfill, web surface for per-device threshold, integration stream/reconnect tests, **AC#10** + AC#11 sign-off. Story stays **in-progress**. |

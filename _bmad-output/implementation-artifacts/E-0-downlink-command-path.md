# Story E.0: Downlink Command Path (first testable slice)

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->
<!-- AC#10 real-world valve OPEN/CLOSE gate (Task 8) PASSED 2026-06-11 on v2.2.0-rc4 (pre-prod, panoramix): FULL open+close cycle driven from Fuxa via OPC UA: each write → opcgw enqueued the semantic object on fPort 10 in ChirpStack → valve physically opened, then closed, each on its next periodic wake (~5 min battery cadence). command_class="valve" set via the web editor (E-2a surface, #135). Story flipped review → done. -->

## Story

As an **opcgw operator with a device command defined**,
I want an OPC UA write to a command node to actually be delivered to the device via ChirpStack,
so that I can pilot an actuator (e.g. open/close a Tonhe E20 valve) from the OPC UA / SCADA side.

## Context & Why This Story Exists

Today the command path is **built but unwired**. An OPC UA write reaches storage and then dies silently:

1. OPC UA client writes a numeric value to a command node → `OpcUa::set_command` (`src/opc_ua.rs:1935`) builds a `DeviceCommand { payload: vec![value as u8], f_port, status: Pending }` and calls `storage.queue_command(...)` (`src/opc_ua.rs:2017`).
2. The poller's `process_command_queue` (`src/chirpstack.rs:2430`, called every cycle at `src/chirpstack.rs:1317`) **dequeues and discards** behind a "Story 4-1 Phase 3" TODO (`src/chirpstack.rs:2434-2442`).
3. `enqueue_device_request_to_server` (`src/chirpstack.rs:2511`) — the function that would actually call ChirpStack's `DeviceService.Enqueue` — is `#[allow(dead_code)]` with **zero call sites**.

This story wires step 2→3 so a write becomes a real LoRaWAN downlink, and switches the valve-class downlink from raw bytes to a **semantic object** so the device-profile codec produces the wire bytes (keeping opcgw model-agnostic). First concrete driver: 3 Tonhe E20 motorized valves.

> ⚠️ **CRITICAL ARCHITECTURE FINDING (verified in code 2026-06-06) — read before coding.**
> There are **two independent command collections** in storage, and they are not connected:
> - **`commands` (type `DeviceCommand`)** — fed by the OPC UA write path (`queue_command` `src/storage/memory.rs:127`), read by `get_pending_commands()` (`src/storage/memory.rs:138`), status updated by `update_command_status()` (`src/storage/memory.rs:147`). **This is the queue OPC UA writes actually land in.**
> - **`command_queue` (type `Command`)** — the high-level Story 3-1 queue with `parameters: serde_json::Value`, fed by `enqueue_command()` / drained by `dequeue_command()`. **Nothing in the OPC UA write path feeds this queue.**
>
> `process_command_queue` currently drains `dequeue_command()` — the **wrong** queue. An OPC UA-written command therefore never reaches it even after you delete the TODO. The epic's "Command ↔ DeviceCommandInternal type unification" framing is misleading: the unification that matters for E-0 is **routing `process_command_queue` to the queue OPC UA actually feeds (`get_pending_commands` / `DeviceCommand`)**, not wrestling the `Command`/`serde_json` type. See [Design Decisions](#design-decisions-confirm-before-or-during-dev) — this is the load-bearing call for this story.

## Acceptance Criteria

1. **The command actually leaves opcgw.** `process_command_queue` drains the queue that OPC UA writes feed and, for each pending command, calls ChirpStack `DeviceService.Enqueue` via `enqueue_device_request_to_server` (or its renamed successor). The drop-and-skip TODO at `src/chirpstack.rs:2434-2442` is removed and `#[allow(dead_code)]` on the enqueue fn is removed (it now has a live call site).

2. **Single coherent type along the send path.** The dequeued item type matches the enqueue function's parameter type. Reconcile `DeviceCommand` (`src/storage/types.rs:162`), `DeviceCommandInternal` (`src/storage/mod.rs:809`, identical fields), and the queue actually used so there is **one** command type from "drained from storage" → "enqueued to ChirpStack". No `serde_json::Value` parameter encoding is introduced in this story.

3. **Semantic-object downlink for valve-class commands.** A command marked as valve-class maps the canonical OPC UA value `1` → `{"command":"open"}` and `0` → `{"command":"close"}`, and enqueues a `DeviceQueueItem` with `object: Some(Struct{...})`, **empty `data`**, and the configured `f_port` (valve = `10`). opcgw builds the `prost_types::Struct`; the device-profile codec (`encodeDownlink`) produces the wire bytes — opcgw never encodes valve bytes itself.

4. **Raw-byte fallback preserved (additive, no regression).** A command **not** marked class-bound enqueues exactly as the legacy `DeviceCommand` path intends: `data: <payload bytes>`, `object: None`, configured `f_port`. Generic devices behave exactly as before this story.

5. **Minimal, forward-compatible class opt-in.** A device/command can be flagged as valve-class **without** building the full E-2 class registry. Default (flag absent) = raw-byte fallback (AC#4). The mechanism must be a clean superset that E-2 can later subsume into the registry (do not paint E-2 into a corner). Recommended: an optional field on `DeviceCommandCfg` (`src/config.rs:693`); see [Design Decisions](#design-decisions-confirm-before-or-during-dev).

6. **Status transitions + resilient batch.** On successful enqueue, command status `Pending → Sent` (timestamped). On enqueue failure, status is set to `Failed` with an `error_message` and an ERROR-level structured log; **processing of remaining commands continues** (one failure must not abort the cycle, nor must it bubble an `Err` that aborts the whole poll — `process_command_queue` is called with `?` at `src/chirpstack.rs:1317`, so a returned `Err` aborts the metrics poll). Full delivery confirmation (Sent → Confirmed) is **out of scope** (Story E-3).

7. **fPort validation + no panics.** fPort is validated (LoRaWAN range `1..=223`; valve uses `10`); an invalid fPort fails that command (logged + status Failed) rather than panicking. Remove the "method currently panics if client creation fails" behaviour noted in the enqueue fn doc (`src/chirpstack.rs:2504-2507`) — client-creation failure must be a handled error, not a panic.

8. **Automated tests.** Cover, against a stub/mock ChirpStack `DeviceService` (introduce an injection seam if none exists — see [Testing](#testing-requirements)):
   - (a) valve-class value `1` → `Enqueue` called with `object == {"command":"open"}`, `data` empty, `f_port == 10`;
   - (b) valve-class value `0` → `{"command":"close"}`;
   - (c) raw-byte fallback → `data == [bytes]`, `object == None`, correct `f_port`;
   - (d) enqueue-failure path → command status `Failed` + remaining commands still processed + no poll abort;
   - (e) success path → status `Pending → Sent`;
   - (f) unit test for the canonical-value → command-object mapping function in isolation.

9. **Quality gates + docs.** SPDX headers on all touched/new files; `cargo test` and `cargo clippy --all-targets -- -D warnings` clean. `README.md` updated (command downlink is now wired end-to-end; document the new config flag). `config/config.toml` + DocBook manual (`docs/manual/opcgw-user-manual.xml`) updated for the new command-class config surface. `docs/LoRa/TONHE Valve/README.md` §4 corrected (see [Known Doc Discrepancy](#known-doc-discrepancy-must-fix)).

10. **Real-world validation gate (blocks `done`).** Per the main-deadlock incident doctrine ([[incident_main_deadlock_2026_05_20]]): **OPEN and CLOSE a physical Tonhe E20 valve from opcgw end-to-end** (OPC UA write → valve actuates) before E-0 flips to `done`. Record the result in the Completion Notes. Automated tests + clippy passing is **not** sufficient for this story.

## Tasks / Subtasks

- [x] **Task 1 — Route `process_command_queue` to the correct queue (AC: 1, 2, 6)**
  - [x] Delete the drop-and-skip TODO block (`src/chirpstack.rs:2434-2442`).
  - [x] Drain the queue OPC UA feeds: iterate `self.backend.get_pending_commands()` (returns `Vec<DeviceCommand>`, `src/storage/memory.rs:138`). Decide ordering (FIFO by `id`) and whether to drain all pending each cycle.
  - [x] For each command: call the enqueue fn; on `Ok` → `update_command_status(id, Sent, None)`; on `Err` → `update_command_status(id, Failed, Some(msg))` + ERROR log; **never early-return `Err` from the loop on a single enqueue failure** (only a storage-lock failure should propagate).
  - [x] Confirm both backends (in-memory `src/storage/memory.rs` + SQLite `src/storage/sqlite.rs`) implement `get_pending_commands` + `update_command_status` consistently; align if not.
- [x] **Task 2 — Unify the command type on the send path (AC: 2)**
  - [x] Make `enqueue_device_request_to_server` accept the same type `get_pending_commands` yields (`DeviceCommand`), or provide a single `From`/`Into`. Eliminate the redundant `DeviceCommandInternal` if it adds no value, OR document why both remain.
  - [x] Remove `#[allow(dead_code)]` (`src/chirpstack.rs:2510`).
- [x] **Task 3 — Add the valve-class opt-in config field (AC: 5)**
  - [x] Add an optional field to `DeviceCommandCfg` (`src/config.rs:693`), e.g. `command_class: Option<String>` (serde `#[serde(default)]`). Absent = generic/raw. Document the accepted value(s) (`"valve"` for now).
  - [x] Ensure it round-trips through both config sources (TOML bootstrap + SQLite singleton path, per Epic D) without breaking existing configs (must be backward-compatible — existing configs have no such field).
- [x] **Task 4 — Canonical-value → command-object mapping (AC: 3, 4)**
  - [x] Implement a pure mapping fn: `(command_class, canonical_value) -> DownlinkPayload` where `DownlinkPayload` is either `Object(prost_types::Struct)` or `Raw(Vec<u8>)`. For valve: `1 -> object {"command":"open"}`, `0 -> object {"command":"close"}`; any other value for valve class → error (logged + Failed).
  - [x] Recover the canonical value from the queued `DeviceCommand.payload` (currently `payload[0]`, the byte `set_command` stored from the OPC UA write). Confirm `set_command` still stores the raw OPC UA value as `payload[0]` (it does — `src/opc_ua.rs:1993`), so `payload[0]` is the canonical value for class-bound commands.
  - [x] Look up the command's `command_class` + `command_port` from config by `(device_id, f_port)` (the queued `DeviceCommand` carries `device_id` + `f_port` but **not** the class — you must resolve it from `AppConfig`).
- [x] **Task 5 — Build the `DeviceQueueItem` correctly (AC: 3, 4, 7)**
  - [x] Build `prost_types::Struct` for the object case using `chirpstack_api::prost_types::{Struct, Value, value::Kind}` (the crate re-exports prost-types — see [Tech Notes](#latest-tech-notes-verified-2026-06-06)).
  - [x] Object case: `object: Some(struct)`, `data: vec![]`. Raw case: `object: None`, `data: payload`.
  - [x] Set `confirmed` from `DeviceCommandCfg.command_confirmed` (the valve sends a conform packet; confirmation handling itself is E-3). Set `f_port` from config.
  - [x] Replace any panic on client-creation failure with a handled `OpcGwError::ChirpStack(...)`.
- [x] **Task 6 — Tests (AC: 8)**
  - [x] Introduce a `DeviceService` injection seam if needed (trait or in-process tonic test server) so enqueue can be asserted without a live ChirpStack.
  - [x] Implement test cases (a)–(f) from AC#8.
- [x] **Task 7 — Docs + quality (AC: 9)**
  - [x] Update `README.md`, `config/config.toml` sample, DocBook manual, and `docs/LoRa/TONHE Valve/README.md` §4 (fix the `1`/`2` vs `1`/`0` discrepancy).
  - [x] SPDX headers; `cargo test`; `cargo clippy --all-targets -- -D warnings`.
- [ ] **Task 8 — Real-world valve test (AC: 10)** — open AND close a physical Tonhe E20 from opcgw; record outcome in Completion Notes. **Gate for `done`.**

### Review Findings (iter-1, 2026-06-06 — 3 layers: Blind Hunter + Edge Case Hunter + Acceptance Auditor)

Decision-needed (resolved 2026-06-06):
- [x] [Review][Decision] Queued command loses its identity — class resolved by `(device_id, f_port)` only [`src/chirpstack.rs` `find_command_cfg`] — **DEFERRED to E-2** (Guy, 2026-06-06): the E-2 class registry will carry command identity; harmless for the E-0 valve (single command on port 10). `DeviceCommand` carries no `command_id`/class, so a device with two same-port commands of different classes resolves to whichever appears first.
- [x] [Review][Decision] At-least-once delivery — duplicate downlink / no retry [`src/chirpstack.rs` `deliver_one`] — **DEFERRED to E-3 + documented** (Guy, 2026-06-06): E-3 (command delivery confirmation) adds Sent→Confirmed tracking + retry; at-least-once is the accepted E-0 semantic (see Dev Notes "Delivery semantics"). If `update_command_status(Sent)` fails after a successful enqueue the row re-sends next cycle; transient enqueue failures go to `Failed` with no retry.

Patch (all applied 2026-06-06; iter-2 re-review clean — mutation test confirmed the strengthened tests discriminate Sent vs Failed):
- [x] [Review][Patch] Status-transition tests assert "not pending", not actual `Sent`/`Failed` [`src/chirpstack.rs` tests] — FIXED: added `InMemoryBackend::command_status_for_test` and rewrote `deliver_one_success_marks_sent` / `deliver_one_enqueue_failure_marks_failed` / `deliver_one_mapping_failure_does_not_enqueue` / `deliver_one_raw_fallback_sends_bytes` to assert the actual `Sent`/`Failed` status + error_message.
- [x] [Review][Patch] AC#8(d) "batch continues / no poll abort" untested [`src/chirpstack.rs` tests] — FIXED: new `deliver_batch_continues_past_a_failure` (3 commands, middle one fails mapping → other two still enqueued + Sent, bad one Failed).
- [x] [Review][Patch] AC#5 SQLite round-trip of a non-None `command_class` untested [`src/storage/sqlite_tests.rs`] — FIXED: new `test_e0_command_class_roundtrip` (insert `Some("valve")` → load asserts round-trip → update to `None` → reload asserts cleared).
- [x] [Review][Patch] `valve` mapping silently truncates multi-byte payloads [`src/chirpstack.rs` `map_command_to_downlink`] — FIXED: now rejects `payload.len() != 1` (covers empty + multi-byte); new `map_valve_empty_payload_errors` + `map_valve_multibyte_payload_errors` tests.

Deferred (pre-existing / out of E-0 scope):
- [x] [Review][Defer] Command queue drains on the metrics-poll cadence, not `[command_delivery]` interval [`src/chirpstack.rs:1317`] — deferred, pre-existing (the `process_command_queue` call site predates E-0).
- [x] [Review][Defer] In-memory backend `queue_command` skips the f_port/payload validation SQLite enforces (backend parity) [`src/storage/memory.rs:127`] — deferred, pre-existing test-fidelity gap; production (SQLite + `set_command`) validates.

Dismissed (4): `is_pending: false` (correct — ChirpStack-managed output field); `command_port` 0/>255 silent no-match (rejected upstream by config validation + `set_command`); f_port validated at producer not send path (defense-in-depth, AC#7 met); AC#10 informational (correctly pending).

## Design Decisions (confirm before or during dev)

These are genuine forks where the epic text and the code diverge. Recommendations below; confirm with Guy if unsure — they change the implementation shape.

1. **Which queue does `process_command_queue` drain?** — **Recommended: `get_pending_commands()` / `DeviceCommand`** (the queue OPC UA actually feeds). The `Command`/`command_queue`/`serde_json` path (`dequeue_command`) has no producer in the OPC UA flow and dragging it in pulls Story 3-1's parameter-encoding rabbit hole into E-0 for no benefit. Re-point the poller at the `DeviceCommand` queue and leave `command_queue`/`dequeue_command` untouched (or note them as Story 3-1 surface to revisit).

2. **How is "valve-class" signalled in E-0 without the E-2 registry?** — **Recommended: optional `command_class: Option<String>` on `DeviceCommandCfg`** (`"valve"` = the only recognized value in E-0; absent = raw fallback). It's the smallest forward-compatible surface and E-2 will lift the value→object map behind this same flag into the registry. Alternative (device-level kind tag) is heavier and overlaps E-2 — avoid for E-0.

3. **Canonical value `0` for a valve is the whole point of the object path.** The raw-byte fallback with the OPC UA value as the byte **cannot close the valve**: value `0` → byte `0x00`, but the valve's CLOSE byte is `0x02` on fPort 10 (and OPEN is `0x01`). So `0` raw = invalid valve command. The semantic object (`0 → {"command":"close"}` → codec → `0x02`) is what makes `0=Close` work. Keep this concrete justification in mind — it's why AC#3 is mandatory, not cosmetic.

## Dev Notes

### Exact source anchors
- Poll loop calls command processing: `src/chirpstack.rs:1317` (`self.process_command_queue().await?;` — note the `?`).
- `process_command_queue`: `src/chirpstack.rs:2430` (TODO to delete: `:2434-2442`).
- `enqueue_device_request_to_server`: `src/chirpstack.rs:2511` (`#[allow(dead_code)]` at `:2510`; builds `DeviceQueueItem` at `:2523-2534` with `object: None`, `data: command.payload`; panic-doc at `:2504-2507`).
- OPC UA write handler: `src/opc_ua.rs:1935` (`set_command`); builds single-byte payload at `:1993`; calls `queue_command` at `:2017`; write-callback registration at `src/opc_ua.rs:1114-1124`.
- Storage `DeviceCommand`: `src/storage/types.rs:162`. `DeviceCommandInternal`: `src/storage/mod.rs:809` (identical fields). `CommandStatus` enum (`Pending`/`Sent`/`Confirmed`/`Failed`): `src/storage/types.rs:106`.
- In-memory queue: `queue_command` `src/storage/memory.rs:127`, `get_pending_commands` `:138`, `update_command_status` `:147`. SQLite equivalents in `src/storage/sqlite.rs`.
- Config command struct `DeviceCommandCfg`: `src/config.rs:693` (`command_id`, `command_name`, `command_confirmed`, `command_port: i32`). Devices carry `device_command_list: Option<Vec<DeviceCommandCfg>>` (`src/config.rs:630`).
- `DeviceQueueItem` / `EnqueueDeviceQueueItemRequest` imports: `src/chirpstack.rs:31-32`. prost-types re-export already used at `src/chirpstack.rs:37` (`use chirpstack_api::prost_types::Timestamp;`).
- E-3 stub (`CommandStatusPoller`) lives ~`src/chirpstack.rs:2705`; **do not implement it here** — only the Pending→Sent transition belongs to E-0.

### Tonhe E20 valve protocol (the concrete driver)
Source: `docs/LoRa/TONHE Valve/tonhe-e20-valve-codec.js` + `README.md`. LoRaWAN 868, **Class A**, single-byte.
- **Downlink fPort 10:** `0x01` = OPEN, `0x02` = CLOSE. The codec's `encodeDownlink` accepts a friendly object: `{"command":"open"}` → `{fPort:10, bytes:[0x01]}`; `{"command":"close"}` → `{fPort:10, bytes:[0x02]}`. (Also `set_period`/`query_period`/`poll`, and a raw `{fPort,bytes}` passthrough — out of E-0 scope.)
- **Class A timing:** a queued downlink is delivered only on the valve's next wake-up (default ~20 min) or after a SET-button press. For the AC#10 test, **press SET** on the valve to force immediate delivery; do not interpret a delayed actuation as a failure.
- Config for a valve command: `command_port = 10`, `command_class = "valve"`; operator writes `1` (open) / `0` (close) to the OPC UA command node.

### Known doc discrepancy (MUST fix)
`docs/LoRa/TONHE Valve/README.md` §4 currently says *"write `1` to open, `2` to close"* — that describes the **legacy raw-byte path** (OPC UA value = wire byte). The Epic E locked design is **`1`=open, `0`=close** via the semantic object. Update §4 to the `1`/`0` semantic-object convention as part of AC#9, and keep `2` only as a raw-passthrough advanced note if you wish.

### Architecture compliance
- Custom error type `OpcGwError` (`src/utils.rs`) via `thiserror`; new error cases use existing variants (`ChirpStack`, `Configuration`, `Storage`). No new panics (AC#7).
- Structured logging (tracing) consistent with existing call sites — use field-style (`error = %e, device_id = %id, ...`) as seen at `src/chirpstack.rs:2449-2551`. Add/adjust `config/log4rs.yaml` levels only if you add a new module (you won't — work is in existing modules).
- Storage stays behind the `StorageBackend` trait; both backends must stay in lockstep (in-memory is test-facing, SQLite is production — Epic D).
- Config is backward-compatible: the new optional field must not break existing `config.toml` or SQLite-singleton rows (Epic D made SQLite authoritative for singletons, but `[[application.device.command]]` arrays are still config/TOML-seeded — verify the new field deserializes with `#[serde(default)]`).

### Latest tech notes (verified 2026-06-06)
- `chirpstack_api = "4.17.0"`, `prost = "0.14"`, `prost-types = "0.14"` (`Cargo.toml:20,28-29`). prost-types is re-exported as `chirpstack_api::prost_types` (already imported in `chirpstack.rs`). **Use that re-export**, not a separate `prost_types` crate path, to avoid version-skew.
- `DeviceQueueItem.object` is `Option<chirpstack_api::prost_types::Struct>`; `.data` is `Vec<u8>`.
- Building the object in Rust:
  ```rust
  use chirpstack_api::prost_types::{Struct, Value, value::Kind};
  use std::collections::BTreeMap; // prost-types Struct.fields is a BTreeMap<String, Value>
  let mut fields = BTreeMap::new();
  fields.insert(
      "command".to_string(),
      Value { kind: Some(Kind::StringValue("open".to_string())) },
  );
  let object = Some(Struct { fields });
  ```
  (Confirm `fields` map type against the pulled prost-types 0.14 — it is `BTreeMap` in 0.14; the compiler will tell you immediately if not.)

### Testing requirements
- Existing command tests are **storage-level only** (`tests/command_delivery_tests.rs`, `src/storage/memory.rs` unit tests for enqueue/dequeue/status) — none exercise the gRPC `Enqueue`. You must add a seam to assert the enqueue call. Options, cheapest first:
  1. Extract the `DeviceQueueItem`-building + value→object mapping into a **pure function** and unit-test it directly (covers AC#8a–c, f without any gRPC) — do this regardless.
  2. For the call-path tests (AC#8d, e), inject a trait object over `DeviceServiceClient::enqueue` (a small `trait DownlinkSink { async fn enqueue(&self, item: DeviceQueueItem, confirmed: bool) -> Result<...> }`) so a mock can record calls and simulate failure.
- Keep the in-memory backend for status-transition assertions (`get_pending_commands` → process → assert `Sent`/`Failed`).
- `cargo test` must stay green (current baseline ~1544 tests per Epic D close); add net-new tests, don't weaken existing ones.

### Delivery semantics (E-0 — at-least-once; accepted, review D2)
E-0 delivers commands **at-least-once**. A command leaves `Pending` only when its
status is updated to `Sent`/`Failed` after the enqueue attempt. Two consequences,
both accepted for E-0 and owned by **E-3** (delivery confirmation):
- If `update_command_status(Sent)` fails *after* a successful enqueue (storage
  error in the narrow window between enqueue and status-write), the row stays
  `Pending` and is re-enqueued next poll cycle → a possible **duplicate
  downlink** (duplicate actuation for a valve).
- A transient enqueue failure goes straight to `Failed` with **no retry**.
E-3 will add `Sent → Confirmed` tracking plus retry/idempotency. Do not add
retry/idempotency in E-0.

### Out of scope (do NOT do here)
- E-1 uplink ingestion (`StreamDeviceEvents`, normalized `ValveState`) — separate story.
- E-2 device-class registry — E-0 uses the minimal config flag; do not build the registry.
- E-3 confirmation poller (`CommandStatusPoller`) — only Pending→Sent here.
- MQTT (`CR-EPIC-C-MQTT`) — not reopened.
- Proportional/position actuators — valve is binary open/close.

### Project Structure Notes
- All work is in existing modules: `src/chirpstack.rs`, `src/config.rs`, possibly `src/storage/{mod,memory,sqlite,types}.rs` (only if Task 1/2 require aligning backends), and docs. No new top-level module expected. If you add a `DownlinkSink` trait for testing, keep it in `src/chirpstack.rs` (or a small `src/chirpstack/...` submodule) — do not scatter.
- SPDX header (`MIT OR Apache-2.0`) + `(c) [2026] Guy Corbaz` on every new/edited source file (note: 2026, matching the codec file).

### References
- [Source: _bmad-output/planning-artifacts/epics.md#Epic E: Model-Agnostic, Class-Aware Device-Abstraction Layer] (lines 1360-1395, esp. Story E.0 scope + locked design decisions + feasibility baseline)
- [Source: _bmad-output/planning-artifacts/epics.md#Epic E — Story Acceptance Criteria] (lines 1439-1447)
- [Source: docs/LoRa/TONHE Valve/tonhe-e20-valve-codec.js] (protocol summary + `encodeDownlink`)
- [Source: docs/LoRa/TONHE Valve/README.md] (test-from-ChirpStack-UI procedure; §4 maps to opcgw)
- [Source: src/chirpstack.rs:2430,2511,1317] (the unwired path)
- [Source: src/opc_ua.rs:1935] (`set_command` producer)
- [Source: src/storage/memory.rs:127,138,147] (`DeviceCommand` queue)
- GitHub issue #129; memory `project_device_abstraction_valves.md`; main-deadlock doctrine `incident_main_deadlock_2026_05_20`.

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (1M context) — bmad-dev-story, 2026-06-06.

### Debug Log References

- `cargo test` → 1577 passed / 0 failed across all targets.
- `cargo clippy --all-targets -- -D warnings` → clean (one `type_complexity` lint on the 7-tuple command-row read was resolved with a `CommandRow` type alias).
- `xmllint --noout docs/manual/opcgw-user-manual.xml` → valid.

### Completion Notes List

Implemented the downlink command path end-to-end, resolving the load-bearing
architecture finding from story creation (the OPC UA write path feeds the
`DeviceCommand` queue, **not** the high-level `Command`/`dequeue_command`
queue the epic text implied).

- **Task 1 (AC#1,2,6)** — `process_command_queue` now drains
  `get_pending_commands()` (the queue `OpcUa::set_command` feeds) and delivers
  each via the new `deliver_command`/`deliver_one` path. The drop-and-skip TODO
  is gone. A storage-lock failure still propagates (aborts the poll cycle, as
  before); a per-command mapping/enqueue failure is logged + recorded as
  `Failed` and the batch continues.
- **Task 2 (AC#2)** — The send path uses a single type, `DeviceCommand`, from
  storage drain → enqueue. The old `enqueue_device_request_to_server`
  (`#[allow(dead_code)]`, took `DeviceCommandInternal`) was removed and replaced
  by the `DownlinkSink` trait (impl'd for `ChirpstackPoller`); `DeviceCommandInternal`
  is no longer referenced by `chirpstack.rs` (it remains only on the legacy
  `Storage` struct).
- **Task 3 (AC#5)** — New optional `command_class: Option<String>` on
  `DeviceCommandCfg` (`#[serde(default)]`, backward-compatible). Rounds-trips
  through the SQLite application store: **new migration v011** adds the
  `command_class` column to the `commands` table; `insert_command` /
  `update_command` / `load_all_applications_config` thread it through. The
  config-reload topology-diff (`commands_equal`) compares it so a class change is
  detected. The web command-CRUD path defaults it to `None` (web surface for
  class binding is E-2).
- **Task 4 (AC#3,4)** — `map_command_to_downlink`: `None` → `Raw` fallback;
  `"valve"` → `Object` (`1`→`{"command":"open"}`, `0`→`{"command":"close"}`,
  other value → error); unknown class string → error (surfaces config typos).
- **Task 5 (AC#3,4,7)** — `build_queue_item` builds the `DeviceQueueItem`
  (`object`+empty `data` vs `data`+no `object`) using
  `chirpstack_api::prost_types::{Struct,Value,value::Kind}`. `confirmed` comes
  from `command_confirmed`. Client-creation failure is a handled `OpcGwError`
  (no panic).
- **Task 6 (AC#8)** — 12 new tests: object mapping (open/close/out-of-range/
  unknown-class/raw), queue-item shape (object empty-data / raw no-object),
  success→Sent, enqueue-failure→Failed (via stub `MockSink` + `InMemoryBackend`),
  mapping-failure-no-enqueue, raw-fallback bytes, and `find_command_cfg` lookup.
- **Task 7 (AC#9)** — Docs updated: README Planning (new Epic E row),
  `config/config.toml` (commented valve command example), DocBook manual
  (`command_class` in example + prose), `docs/LoRa/TONHE Valve/README.md` §4
  (corrected `1`/`2` → `1`/`0` semantic-object convention). SPDX header on the
  new migration. No new `event=` audit events were introduced (the path uses
  `debug!`/`error!` operational logs), so `docs/logging.md` needs no change.

**No new audit events / no new dependencies.** `async-trait` (already a dep) is
used for the `DownlinkSink` seam.

**Design decisions** from the story were taken as recommended (all three):
drain the `DeviceCommand` queue; minimal `command_class` config opt-in; leave
the Story 3-1 `Command`/`dequeue_command` path untouched.

⚠️ **AC#10 — Real-world valve OPEN/CLOSE gate is PENDING (Task 8 unchecked).**
This is a manual, hardware-in-the-loop test that must be performed against a
physical Tonhe E20 valve before E-0 flips to `done` (main-deadlock incident
doctrine). Automated tests + clippy passing is **not** sufficient. Story is set
to `review`; the gate must be satisfied at/by code-review-completion time.

### File List

Modified:
- `src/chirpstack.rs` — rewired `process_command_queue`; new `deliver_command` method + free `deliver_one`; `DownlinkPayload` enum; `DownlinkSink` trait + impl; `find_command_cfg` / `map_command_to_downlink` / `valve_command_object` / `build_queue_item` helpers; removed old `enqueue_device_request_to_server`; new test suite.
- `src/config.rs` — `command_class: Option<String>` on `DeviceCommandCfg`; updated test fixtures.
- `src/config_reload.rs` — `commands_equal` compares `command_class` (destructure landmine).
- `src/storage/schema.rs` — register v011 migration; `LATEST_VERSION = 11`; updated version assertions.
- `src/storage/sqlite.rs` — `commands` table read/insert/update thread `command_class`; `CommandRow` type alias.
- `src/storage/sqlite_tests.rs` — updated `DeviceCommandCfg` fixtures.
- `src/opcua_topology_apply.rs` — `command_class: None` in `AddedCommand`→`DeviceCommandCfg` apply + test helper.
- `src/web/api.rs` — `command_class: None` on web create_command (E-2 will expose it).
- `README.md` — Epic E Planning row.
- `config/config.toml` — valve command example with `command_class`.
- `docs/manual/opcgw-user-manual.xml` — `command_class` in command-config example + prose.
- `docs/LoRa/TONHE Valve/README.md` — §4 corrected to the `1`/`0` semantic-object convention.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — E-0 status.

Added:
- `migrations/v011_command_class.sql` — `ALTER TABLE commands ADD COLUMN command_class TEXT`.

## Change Log

| Date | Change |
|------|--------|
| 2026-06-06 | Story E-0 implementation complete (Tasks 1–7). Downlink command path wired end-to-end: OPC UA command writes now reach the device via ChirpStack; valve-class commands send a semantic command object, generic commands keep the raw-byte path. Schema migration v011 adds `command_class`. 12 new tests; full suite 1577/0, clippy + xmllint clean. Status → review. **AC#10 real-world valve gate (Task 8) pending before `done`.** |
| 2026-06-06 | Code review iter-1 (Blind Hunter + Edge Case Hunter + Acceptance Auditor). 2 decision-needed (D1 queued-command identity → deferred to E-2; D2 at-least-once delivery → deferred to E-3 + documented), 4 patches applied (3 test-strength gaps + valve multi-byte guard), 2 pre-existing defers logged, 4 dismissed. iter-2 focused re-review of the patches: clean (mutation test confirmed status-transition tests now discriminate). Full suite 1585/0; clippy clean. Review loop terminated (0 HIGH/MEDIUM open). Story remains `review` — blocked only on the AC#10 real-world valve gate before `done`. |

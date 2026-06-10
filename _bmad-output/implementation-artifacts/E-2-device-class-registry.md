# Story E.2: Device-Class + Per-Model Adapter Registry

Status: review

<!-- Delivered as E-2a (Increments 1–2): the command_class web/config surface
(#135) + the device-class registry / DeviceDriver trait with the valve as the
first Tier-1 driver. E-2b (Tier-2 object-remap engine + command_kind/SetLevel +
2nd class) is deferred to backlog — see deferred-work.md "E-2 scope split". This
review covers the E-2a scope only. -->


<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **opcgw maintainer**,
I want a registry of device **classes** (canonical OPC UA surface) and per-(class,model) **adapters** (canonical↔ChirpStack translation),
so that every model of a class — even one whose ChirpStack codec I cannot edit — presents one identical On/Off / SetLevel + status surface to SCADA, and adding a model is a declarative addition (not a core code change).

## Context & Why Now

Epic E set out to make opcgw a model-agnostic, class-aware device-abstraction layer. E-0 wired the downlink command path (raw-byte + semantic-object enqueue); E-1a/E-1b wired uplink-event ingestion (last-known value, no aggregation). Both currently hard-code the **valve** specifics inline:

- `map_command_to_downlink` (`src/chirpstack.rs`) special-cases `command_class == "valve"` → `{"command":"open"|"close"}`.
- `chirpstack_events.rs` detects valve-class devices via `command_class == Some("valve")` and normalizes valve uplink fields inline.

This story generalizes both into a **registry**, and — critically — adds the **config/web surface to actually assign a device's class/model**, which today is impossible: the web command CRUD hard-codes `command_class: None` (`src/web/api.rs:3187`). That gap is tracked as **[#135](https://github.com/guycorbaz/opcgw/issues/135)** and is why a valve **close** sent through opcgw goes out as raw `0x00` (the Tonhe ignores it) instead of the codec's `0x02`. Real-world validation on 2026-06-10 confirmed the valve hardware + delivery + correct bytes all work; the only gap to Fuxa→opcgw close is exposing `command_class="valve"`.

**Design source of truth:** `_bmad-output/planning-artifacts/sprint-change-proposal-2026-06-09.md` (the correct-course that recast E-2) and `epics.md` §"Story E.2" + §"Epic E — Story Acceptance Criteria". This story supersedes the original 2026-06-06 lock that put *all* model translation in the ChirpStack codec — the new constraint is that **a codec may be installed but not editable** to opcgw's canonical shape, so opcgw must own the translation when needed.

## Acceptance Criteria

1. **Class registry foundation.** A device-class registry defines, per class, the canonical OPC UA **command kinds** (`onoff` binary, `setlevel` analog, `raw` legacy) and a normalized **status vocabulary** (uniform core `Active` / `Transitioning` / `Fault`, plus class-specific extras). The **valve** class is registered with its canonical states (open/opening/closed/closing/blocked/fault/unknown) and flags (`Moving`/`Fault`/`LowBattery`), reproducing today's E-1a valve normalization exactly.

2. **Adapter abstraction (hybrid).** A per-(class,model) adapter abstraction exists: a Rust `trait DeviceDriver { encode(canonical) -> Downlink; decode(uplink) -> CanonicalFields }` escape hatch for complex models, **plus** a declarative-profile interpreter (config/SQLite-driven) for simple models. Each direction (uplink decode / downlink encode) independently selects a **tier**: **T1 codec-canonical**, **T2 vendor-object remap** (field rename + value transforms), **T3 native bytes** (raw `data` / `bytes`+`fPort`).

3. **Valve refactored into the registry as the first T1 driver — zero behaviour change.** The inline valve logic in `map_command_to_downlink` (`chirpstack.rs`) and the valve uplink normalization in `chirpstack_events.rs` are moved behind the registry. All existing valve tests (`map_valve_*`, `deliver_one_*`, chirpstack_events valve tests, `test_e0_command_class_roundtrip`) pass unchanged; the live valve still maps `1`→`{"command":"open"}` / `0`→`{"command":"close"}` and normalizes the same status fields.

4. **`command_kind` command bindings.** E-0's command binding is generalized so a command carries a `command_kind` (`onoff` | `setlevel` | `raw`). `onoff`: writable-Variable value → lookup → payload, with **on-polarity configurable per model** (valve default `1`=open). `setlevel`: value → scale/offset/encode → payload. `raw`: existing single-byte passthrough (unchanged default for unclassed commands).

5. **T2 object-remap driver (proves the uneditable-codec path).** One Tier-2 object-remap profile is shipped for a second model whose codec emits a *different* decoded object — translated to canonical via **field rename + value transforms (enum map, linear scale+offset, bitmask/shift)**, owned entirely by opcgw. It presents the **identical** On/Off + status OPC UA surface as the T1 valve.

6. **SetLevel encode path.** At least one `setlevel` binding encodes an analog canonical value via the adapter (scale + offset → payload bytes), with a test proving the encoding.

7. **Second class stub (proves class extensibility).** A second class (e.g. `switch`, On/Off) is registered to prove the canonical surface is defined once per class; a minimal driver suffices.

8. **Config + web surface to assign `(class, model)` / `command_kind`.** The web command CRUD accepts and persists `command_class` (and the new class/model/command_kind binding): add the field(s) to the `ALLOWED_FIELDS` allow-lists and request extraction in `create_command` **and** `update_command` (`src/web/api.rs`), **remove the hard-coded `command_class: None`** (`api.rs:3187`), and surface the field in the command create/edit forms + table (`static/commands.html`, `static/commands.js`) following the C-2 picker pattern. Unknown class/model/command_kind values are rejected with the existing `command_crud_rejected` audit event shape (HTTP 400). **This closes [#135](https://github.com/guycorbaz/opcgw/issues/135).**

9. **Additive — generic + T1 devices unchanged.** A device bound to no class keeps exposing arbitrary numeric metrics + raw writable command nodes exactly as before Epic E; the `raw` / `command_class == None` path is byte-for-byte unchanged. Existing generic-device and raw-fallback tests pass.

10. **Validation + config-reload integrity.** Class/model/command_kind values are validated in `AppConfig::validate()` (unknown → config error, consistent with existing command validation); a CRUD write triggers `notify_crud_write` and the runtime poller picks up the new binding (mirroring the existing `commands_equal` reload path, which already compares `command_class`).

11. **Tests (cargo test + clippy clean).** New tests cover: valve **T1** round-trip (open/close encode + status decode); **T2** remap round-trip exercising **enum map + linear scale + bitmask-shift**; **SetLevel** encode; a **generic** device unaffected (regression guard); the **second class** validates extensibility; web CRUD **accepts/persists/round-trips** a valid `(class, model, command_kind)` and **rejects** unknown values. Regression-guard tests must invoke the function-under-test directly and assert on real outputs (no fake guards — see Dev Notes). `cargo test` and `cargo clippy --all-targets -- -D warnings` are clean.

12. **Docs sync (same commit).** Update `docs/architecture.md` (adapter/registry model + tier diagram), the config reference + DocBook manual `docs/manual/opcgw-user-manual.xml` (class/model + command_kind surface, how to set a valve command's class), `docs/logging.md` if any new events are emitted, and the **README Planning** section + `sprint-status.yaml` to mirror E-2 status. SPDX `MIT OR Apache-2.0` + copyright header on every new source file.

## Tasks / Subtasks

- [ ] **Task 1 — Registry + adapter abstraction (AC: 1, 2)**
  - [ ] Create `src/device_registry.rs` (or `src/devices/` module): define `CommandKind { OnOff, SetLevel, Raw }`, a canonical status model (`uniform core Active/Transitioning/Fault` + class extras), and `trait DeviceDriver { fn encode(&self, kind, value) -> Result<DownlinkPayload, OpcGwError>; fn decode(&self, fport, bytes/object) -> Result<CanonicalFields, OpcGwError>; }`.
  - [ ] Define the declarative profile struct (serde-deserializable: field-rename map, enum map, linear scale+offset, bitmask/shift) and a profile-interpreter driver implementing `DeviceDriver`.
  - [ ] Build a `ClassRegistry` that resolves `(class, model)` → driver, with the per-direction tier choice. Register it once at startup; thread it where `map_command_to_downlink` / `chirpstack_events` need it (config-derived, like `find_command_cfg`).
  - [ ] Reuse `DownlinkPayload` (`chirpstack.rs`) and `valve_command_object`'s `prost_types::Struct` builder; do not invent a parallel downlink type.
- [ ] **Task 2 — Refactor valve into first T1 driver, no behaviour change (AC: 3, 9)**
  - [ ] Move the `Some("valve")` arm of `map_command_to_downlink` (`chirpstack.rs:~2741`) into a `ValveDriver` (T1). Keep `None` → `DownlinkPayload::Raw` exactly (the additive contract).
  - [ ] Move valve uplink normalization from `chirpstack_events.rs` (the `command_class == Some("valve")` detection at `:258` and field mapping) into `ValveDriver::decode`.
  - [ ] Run the full existing valve test set unchanged → must stay green (this is the regression gate for the refactor).
- [ ] **Task 3 — `command_kind` bindings + SetLevel (AC: 4, 6)**
  - [ ] Extend `DeviceCommandCfg` (`src/config.rs:718`) with `command_kind` (default `raw` for back-compat) + optional model + on-polarity; keep `command_class` as the class binding. `#[serde(default)]` everything for back-compat.
  - [ ] Implement `onoff` (value→lookup, polarity-aware) and `setlevel` (value→scale/offset/encode) in the driver/registry; `raw` stays the existing single-byte passthrough.
  - [ ] Thread the new fields through SQLite (Task 5) and `commands_equal` (`config_reload.rs:620`).
- [ ] **Task 4 — T2 object-remap driver + second class stub (AC: 5, 7)**
  - [ ] Ship one T2 declarative profile for a second model (uneditable-codec case): exercise field-rename + enum map + linear scale + bitmask-shift in `decode`, presenting the canonical valve/switch surface.
  - [ ] Register a second class (`switch`, On/Off) with a minimal driver to prove the registry generalizes beyond valve.
- [ ] **Task 5 — Persistence + config validation (AC: 8, 10)**
  - [ ] If new columns are needed (command_kind / model / polarity), add migration `v013_*.sql` (mirror `v011_command_class.sql`: `ALTER TABLE commands ADD COLUMN ... ` nullable, no default = back-compat); bump schema version + assertions in `schema.rs`. (`command_class` column already exists via v011.)
  - [ ] Update `insert_command` / `update_command` / `load_all_applications_config` (`storage/sqlite.rs:2782/2825/3035`) to carry the new fields. Note `update_command_by_id` (`:2878`, used by the HTTP PUT path) currently does **not** touch `command_class` — reconcile so the web PUT persists the binding.
  - [ ] Add `(class, model, command_kind)` validation in `AppConfig::validate()` (unknown class/model/kind → `OpcGwError::Configuration`).
- [ ] **Task 6 — Web API + UI surface (AC: 8, closes #135)**
  - [ ] `src/web/api.rs`: add `command_class` (+ model/command_kind) to `ALLOWED_FIELDS` in `create_command` (`:2936`) and `update_command` (`:3270`); extract from the request `serde_json::Value`; **delete the hard-coded `command_class: None`** (`:3187`); validate values; keep the `command_crud_rejected` / `unknown_field` audit shape for rejects.
  - [ ] `static/commands.html` + `static/commands.js`: add a class/model/command_kind selector to the create form (`commands.js:~176`) and edit modal (`commands.html:~61`), a display column in the table (`commands.js:~137`), and include the field in the POST/PUT JSON payloads. Follow the C-2 inventory-picker pattern (`static/inventory-picker.js`).
- [ ] **Task 7 — Tests (AC: 11)**
  - [ ] Unit: registry resolution; `ValveDriver` T1 encode/decode; T2 profile decode (enum + scale + bitmask); SetLevel encode; `raw`/generic unchanged (regression guard that invokes the real path with non-overlapping seeds); second-class resolution.
  - [ ] Integration: extend `tests/web_command_crud.rs` — POST/PUT with valid `command_class="valve"` persists + round-trips; unknown class/kind → 400 with audit; generic command (no class) still works.
  - [ ] `cargo test` + `cargo clippy --all-targets -- -D warnings` clean.
- [ ] **Task 8 — Docs sync (AC: 12)**
  - [ ] `docs/architecture.md`, `docs/manual/opcgw-user-manual.xml`, `docs/logging.md` (if new events), README Planning section, `sprint-status.yaml`. SPDX headers on new files. `xmllint` the DocBook.

### Review Findings (iter-1, 2026-06-10)

Code review of **E-2a** (commits `336cd94` + `ca34449`) via 3 adversarial layers (Blind Hunter, Edge Case Hunter, Acceptance Auditor). **0 HIGH.** In-scope ACs #8, #3/#9, #1–2 core, #11 verified satisfied; the valve refactor preserves error strings byte-for-byte (genuine regression gate, not a fake guard).

- [ ] [Review][Patch] `command_class` validation not single-sourced + not enforced at config-load (MEDIUM, converged blind+edge+auditor / AC#10) — web validator uses a hand-maintained `KNOWN_COMMAND_CLASSES` const duplicating the registry (drift), and `AppConfig::validate()` doesn't check `command_class` at all (a bad class via TOML/SQLite boots clean, fails silently at delivery). Fix: registry = single source of truth (`validate_command_class` → `registry().driver_for()`) + add a `command_class` arm to `AppConfig::validate()`. [src/web/api.rs, src/config.rs]
- [ ] [Review][Patch] No test for the non-string `command_class` rejection branch (LOW) — add `command_class: 5` → 400. [tests/web_command_crud.rs]
- [ ] [Review][Patch] Weak valve-encode test assertion (Debug substring) (LOW) — assert the codec object structurally. [src/device_registry.rs]
- [ ] [Review][Patch] README Planning understates Increment 2 registry refactor (LOW / AC#12). [README.md]
- [x] [Review][Defer] JS edit-form silently coerces an unknown stored class to "(none)" and clears it on save (LOW) [static/commands.js] — deferred to E-2b: cannot trigger with a single registered class; fix with a registry-driven dropdown.
- [x] [Review][Defer] No end-to-end persisted-class → downlink-object test (LOW) — deferred to E-2b: unit (encode) + CRUD (persist) layers cover the halves.
- Dismissed (2): case/whitespace strictness (consistent exact-match, not a defect); valve-decode "boundary" (accepted/documented E-2a/E-2b split).

## Dev Notes

### Architecture patterns & constraints

- **Additive, not a rewrite.** The `command_class == None` / `raw` path must remain byte-for-byte identical. The registry sits *in front of* the existing `DownlinkPayload::{Raw,Object}` enum and the E-1a last-value store; it does not replace them. Generic devices are the majority case and must be untouched (AC#9).
- **No gateway-side aggregation (LOCKED, #130).** The value path stays last-known-value + device source timestamp + quality; the registry's `decode` produces canonical fields that feed the *existing* E-1a no-aggregation store. Do **not** route any value through `GetMetrics` aggregation.
- **The valve's per-model bytes live in the ChirpStack codec (T1), not opcgw.** `docs/LoRa/TONHE Valve/tonhe-e20-valve-codec.js` is a ChirpStack artifact. opcgw's `ValveDriver` emits the *semantic object* `{"command":"open"|"close"}`; the codec's `encodeDownlink` turns it into `0x01`/`0x02`. T2 is the opposite case: opcgw owns the translation because the codec can't be edited.
- **Tiers are per-direction.** A model can be T1 on downlink and T2 on uplink, etc. Model the tier choice independently for `encode` vs `decode`.
- **Keep the status ontology light** until a real second class forces shape (sprint-change-proposal §4.2). The `switch` stub is to prove extensibility, not to over-build a taxonomy.

### Source tree — exact touchpoints (verified)

| Area | File:line | Current state | Action |
|---|---|---|---|
| Command config struct | `src/config.rs:718–741` | `command_class: Option<String>` `#[serde(default)]` | add `command_kind` (default raw), optional model/polarity |
| Downlink mapping | `src/chirpstack.rs:~2741` `map_command_to_downlink`; `valve_command_object` `:~2778`; `deliver_one` `:~2644`; `deliver_command` `:~2475` | `None`→Raw, `Some("valve")`→Object, unknown→Err | move valve arm into `ValveDriver`; dispatch via registry |
| Uplink normalization | `src/chirpstack_events.rs:258` | `command_class == Some("valve")` inline | move into `ValveDriver::decode` |
| Web create | `src/web/api.rs:2936` (ALLOWED_FIELDS), `:3179–3188` (hard-coded `None`) | `command_class` not accepted | accept + validate; delete hard-coded None |
| Web update | `src/web/api.rs:3270` (ALLOWED_FIELDS) | `command_class` not accepted | accept + validate |
| SQLite commands | `migrations/v009_*.sql:43`; `migrations/v011_command_class.sql`; `src/storage/sqlite.rs` insert `:2782`, update `:2825`, `update_command_by_id` `:2878` (does NOT set command_class — reconcile), load `:3035` | column exists; load/insert/update carry `command_class` | add migration only if new columns; fix `update_command_by_id` gap |
| Schema version | `src/storage/schema.rs` (asserts v12) | latest v012 | bump + assert if new migration |
| Config reload | `src/config_reload.rs:620` `commands_equal` | already compares `command_class` | extend for new fields |
| Web UI | `static/commands.html:61` (edit modal), `static/commands.js:137` (table), `:176` (create form); `static/inventory-picker.js` (C-2 pattern) | no class field | add selector + column + payload |
| Tests | `src/chirpstack.rs:4316–4493` (8 map/deliver tests); `src/storage/sqlite_tests.rs:3684` (`test_e0_command_class_roundtrip`); `tests/web_command_crud.rs` | valve-only / no class in CRUD | extend per AC#11 |

### Previous-story intelligence (E-0, E-1a/E-1b)

- **E-0** established: `DownlinkPayload::{Raw,Object}`, the `object`-based enqueue via `build_queue_item`, the raw-byte fallback, and the unit-testable `deliver_one(sink, backend, class, confirmed, cmd)` with a mock `DownlinkSink`. **Reuse these — do not reinvent.** E-0's real-world valve gate doctrine applies: the refactor must keep the live valve working (validated 2026-06-10; periodic 5-min reporting + open/close confirmed end-to-end via the API path).
- **E-1a** established `src/chirpstack_events.rs` consuming `InternalService.StreamDeviceEvents`, valve-class detection via `command_class == Some("valve")`, last-value store with device source timestamp, first poller-side `MetricType::String`. The valve `decode` refactor must preserve these field outputs (`valveStatusCode`, `valvePosition`, `moving`, `fault`, `lowBattery`, `state`).
- **E-1b** added `chirpstack.stream_all_devices` toggle, per-device `stale_threshold_secs` (schema v012), and orphan-warn (`uplink_metric_never_seen` / `uplink_metric_now_seen`). Don't disturb these.

### Conventions & anti-patterns to avoid

- SPDX `// SPDX-License-Identifier: MIT OR Apache-2.0` + `// (c) [2026] Guy Corbaz` header on every new `.rs` file. Rust 2021, rustc ≥ 1.87. Errors via `OpcGwError` (`utils.rs`, `thiserror`). Doc-comment all public items.
- **No `error.to_string().contains(...)` string-matching** for control flow (the substring-matcher anti-pattern repeatedly flagged across Epics C/D). Match typed `OpcGwError` variants.
- **No fake regression guards** (Epic A finding class): a regression-guard test must invoke the function-under-test directly and use seeds whose outputs differ between the surviving and dropped code paths. The generic-device-unaffected test (AC#9/#11) is exactly this kind of guard — make it real.
- **Code-review loop discipline (CLAUDE.md):** after `bmad-dev-story`, run `bmad-code-review` and loop until only LOW findings remain; iter-N+1 is mandatory when iter-N introduces new code (this story introduces a whole registry → expect ≥2 iters). Story flips to `done` only on clean `cargo test` + `clippy -D warnings`.

### Scope / slicing note (for the dev agent or correct-course)

This is a large story (registry + T1 refactor + T2 driver + 2nd class + SetLevel + web surface + docs). A natural split, if it proves oversized, mirrors E-1's a/b split:
- **E-2a** = registry abstraction + valve T1 refactor (no behaviour change) + **web/config surface for `command_class`** (closes #135, delivers the immediate Fuxa→opcgw valve-close fix).
- **E-2b** = T2 object-remap driver + SetLevel + second-class stub + full docs of the tier model.
Do **not** silently descope; if splitting, run `bmad-correct-course` and update `sprint-status.yaml` + `epics.md`. The #135 close-fix is the highest-value slice and should land first either way.

### Testing standards

- Unit tests inline under `#[cfg(test)]`; integration tests in `tests/*.rs`. Mock `DownlinkSink` for delivery tests (see `chirpstack.rs` deliver_one tests). Use `#[traced_test]` for any log-emit assertions. SQLite tests use `temp_backend_path()` (see `sqlite_tests.rs`). DocBook validated with `xmllint --noout`.

### Project Structure Notes

- New registry code: prefer a dedicated module (`src/device_registry.rs` or `src/devices/mod.rs`) rather than swelling `chirpstack.rs` (already ~4500 lines). Register it in `main.rs` alongside config load and thread it into the poller (`ChirpstackPoller`) and the events task, derived from `AppConfig` like `find_command_cfg`.
- Declarative profiles: decide config home — inline in the `[[application.device.command]]` SQLite/config surface vs a separate profiles table/file. Given Epic D made SQLite the singleton-config source of truth and config.toml bootstrap-only, keep new persistent binding fields in the `commands` table (mirroring `command_class`); a model-profile catalog (for T2 transforms) may be a small static/registered-in-code table for the shipped drivers rather than user-editable in this story.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story-E.2] — recast scope, success criteria, additive contract.
- [Source: _bmad-output/planning-artifacts/epics.md#Epic-E-Story-Acceptance-Criteria] — end-to-end Given/When/Then.
- [Source: _bmad-output/planning-artifacts/sprint-change-proposal-2026-06-09.md] — §4.1 two-axes, §4.2 revised command/status surface, §4.3 E.2 rewrite, §5 success criteria.
- [Source: src/chirpstack.rs#map_command_to_downlink] — valve T1 mapping to refactor.
- [Source: src/chirpstack_events.rs] — E-1a valve uplink normalization to refactor.
- [Source: src/web/api.rs:3187] — hard-coded `command_class: None` (#135).
- [Source: migrations/v011_command_class.sql] — column precedent for any new migration.
- [Source: memory project_device_abstraction_valves.md] — full locked design + 2026-06-10 hardware validation (valve close proven with `0x02`; #134/#135 filed).
- GitHub: [#129 Epic E](https://github.com/guycorbaz/opcgw/issues/129), [#135 command_class surface](https://github.com/guycorbaz/opcgw/issues/135).

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (1M context)

### Debug Log References

- `cargo test` (full suite): 0 failures (316+ tests across all binaries/integration suites green).
- `cargo clippy --all-targets -- -D warnings`: clean.
- New tests: `cargo test --test web_command_crud command_class` → 4 passed.

### Completion Notes List

**Increment 1 — `command_class` web/config surface (AC#8, closes #135) — COMPLETE & VERIFIED (2026-06-10).**
Delivers the immediate goal: a valve command can now be bound to `command_class = "valve"` through the web command editor, so opcgw delivers the semantic `{"command":"open"/"close"}` object (codec → `0x01`/`0x02`) instead of an invalid raw byte. `map_command_to_downlink` already mapped `"valve"`; this increment opens the config surface that fed it `None`.

What landed:
- `src/web/api.rs`: `command_class` added to `create_command` + `update_command` allow-lists; extracted (string|null) with a 400 on wrong type; validated via new `validate_command_class` against `KNOWN_COMMAND_CLASSES` (`["valve"]`, kept in sync with the runtime dispatch); the hard-coded `command_class: None` at the old `:3187` removed; `CommandResponse` gained `command_class` and all four build sites (list/get/create/update) echo it.
- `src/storage/sqlite.rs`: `update_command_by_id` now persists `command_class` (the web PUT path previously dropped it).
- `static/commands.{html,js}`: class `<select>` in the create form + edit modal, a `command_class` table column, payload wiring (create omits when blank; PUT sends `null` to clear).
- `tests/web_command_crud.rs`: +4 integration tests (valve persists+round-trips; unknown class → 400; absent → null; PUT set-then-clear).
- `README.md`: Epic E status line updated.

**Validation:** unknown class rejected with `event="command_crud_rejected" reason="validation" field="command_class"` (value Debug-formatted to prevent log injection — matches the B-H5 house pattern). No new audit-event *type* (reuses `command_crud_rejected`), so `docs/logging.md` needs no change.

**Increment 2 — registry + valve T1 refactor (Tasks 1–2 core) — COMPLETE & VERIFIED (2026-06-10).**
Zero behaviour change; pure architectural groundwork for the additive driver model.

What landed:
- New `src/device_registry.rs`: `trait DeviceDriver { class_name; encode_command }`, the Tier-1 `ValveDriver` (encode `1`→open / `0`→close object; same payload-length + value validation + error messages as before), a `ClassRegistry` resolving `command_class` → driver, and a process-wide `LazyLock` `registry()`. +3 unit tests. (Tier model documented in the module header; a `decode_uplink` hook + declarative-profile interpreter land with the Tier-2 work — valve decode is a passthrough since the codec emits canonical fields, stored as-is by `chirpstack_events::map_uplink_to_writes`.)
- `src/chirpstack.rs`: `map_command_to_downlink` is now thin dispatch — `None`→raw, bound class→`registry().driver_for(class).encode_command(...)`, unknown→error. `DownlinkPayload` + `valve_command_object` made `pub(crate)` so the registry can build the codec object.
- `src/lib.rs` + `src/main.rs`: declare `device_registry`.

**Verification:** existing `map_valve_*` (4) + `deliver_one_*` (4) tests pass unchanged (the regression gate for the refactor); +3 new registry unit tests; full suite 0 failures; `clippy -D warnings` clean.

**NOT done (remaining E-2 scope — Increment 3 = E-2b):** Tasks 3–5, 7 (the `command_kind`/SetLevel bindings, the Tier-2 object-remap transform engine + a second driver, a stub 2nd class) plus the registry-dependent parts of Task 6 (model/command_kind selectors) and Task 8 (architecture/DocBook docs of the tier model). Task checkboxes left unchecked because no *whole* task is end-to-end complete. Recommend `bmad-correct-course` to formally split E-2a (Increments 1–2, shippable now) / E-2b (Increment 3) before continuing.

### File List

- `src/device_registry.rs` (new) — DeviceDriver trait + ValveDriver (T1) + ClassRegistry + registry()
- `src/chirpstack.rs` (modified) — map_command_to_downlink delegates to the registry; DownlinkPayload/valve_command_object pub(crate)
- `src/lib.rs` (modified) — declare device_registry module
- `src/main.rs` (modified) — declare device_registry module
- `src/web/api.rs` (modified) — command_class CRUD surface + validator + CommandResponse field
- `src/storage/sqlite.rs` (modified) — `update_command_by_id` persists command_class
- `static/commands.html` (modified) — edit-modal class selector
- `static/commands.js` (modified) — create-form selector, table column, payload wiring
- `tests/web_command_crud.rs` (modified) — +4 command_class integration tests
- `README.md` (modified) — Epic E status line
- `_bmad-output/implementation-artifacts/E-2-device-class-registry.md` (this story file)
- `_bmad-output/implementation-artifacts/sprint-status.yaml` (status → in-progress)

# Story 9.6: Command CRUD via Web UI

**Epic:** 9 (Web Configuration & Hot-Reload — Phase B)
**Phase:** Phase B
**Status:** ready-for-dev
**Created:** 2026-05-12
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md:883-897` is the BDD source of truth. The epics file numbers this story `8.6` (legacy carry-over from before the Phase A/B split); sprint-status, file naming, and this spec use `9-6`. `epics.md:771` documents the offset. Story 9-6 lifts the 4 BDD clauses from epics.md as ACs #1–#4 and adds carry-forward invariants from Stories 9-1 / 9-2 / 9-3 / 9-4 / 9-5 / 9-7 / 7-2 / 7-3 / 8-3 as ACs #5–#12.

---

## User Story

As an **operator**,
I want to manage device commands through the web interface,
So that I can configure new valve commands without editing config files (FR36, FR41).

---

## Objective

Stories 9-4 and 9-5 shipped the CRUD scaffold (CSRF middleware + path-aware audit dispatch + `ConfigWriter` lock-and-rollback + `AppState` reuse + `validate_path_application_id` resource-aware dispatch + `find_application_index` resource-threaded dispatch + audit-event reason taxonomy). Story 9-6 is the **third and final consumer** of that scaffold and closes **FR36** by landing device-command CRUD on the `[[application.device.command]]` sub-table — the same sub-table Story 9-5's PUT-replace-device explicitly preserved byte-for-byte (Story 9-5 Task 6).

1. **CRUD endpoints for `[[application.device.command]]`** — 5 routes nested under the existing device surface:

   - `GET    /api/applications/:application_id/devices/:device_id/commands`
   - `GET    /api/applications/:application_id/devices/:device_id/commands/:command_id`
   - `POST   /api/applications/:application_id/devices/:device_id/commands`
   - `PUT    /api/applications/:application_id/devices/:device_id/commands/:command_id`
   - `DELETE /api/applications/:application_id/devices/:device_id/commands/:command_id`

   `command_id` in the URL path is a **decimal integer** (matches `DeviceCommandCfg::command_id: i32` at `src/config.rs:663`), not a string — unlike `:application_id` / `:device_id` which are strings. Path-id validation parses the segment as `i32` and rejects non-numeric / out-of-range / negative values with 400 + `event="command_crud_rejected" reason="validation"`.

2. **CSRF middleware literal-arm completion** — Stories 9-4 and 9-5 widened the CSRF middleware (`src/web/csrf.rs`) to dispatch the rejection audit-event name by URL path. The `"command"` arm is **already routed** by `csrf_event_resource_for_path` (lines 209-214 plus the `commands` sub-resource recognition), but the two rejection-emission `match` blocks at lines 277-306 (Origin/Referer reject) and 318-344 (Content-Type reject) currently fall through to the catch-all `event="crud_rejected"` (no resource prefix) for `"command"`. Per the source comment at `src/web/csrf.rs:271-275`:

   > *"The 'command' arm intentionally falls through to the generic catch-all in Story 9-5 — Story 9-6 will replace the catch-all with a literal `command_crud_rejected` warn when commands CRUD lands. Adding the literal here today would constitute Story 9-5 scope creep."*

   Story 9-6 adds the literal `"command" => warn!(event = "command_crud_rejected", reason = "csrf", ...)` arm to both match blocks. This is the **load-bearing source-grep precondition** for AC#5/AC#8 (`git grep -hoE 'event = "command_[a-z_]+"' src/ | sort -u` returns exactly 4 lines).

3. **`validate_path_device_id` widening** (`src/web/api.rs:600`). Story 9-5 created the helper with a hard-coded `event="device_crud_rejected"` warn — fine when only device handlers called it. Story 9-6's command handlers also call it (the `:device_id` segment appears in every command URL), so a command-handler-invoked `validate_path_device_id` would currently misroute the audit event to `device_crud_rejected` instead of `command_crud_rejected`. This is the same regression class the Story 9-5 iter-3 Blind#3 patch defused for `find_application_index`. Fix: widen `validate_path_device_id` to take `resource: &'static str` and dispatch the event-name literal per arm (parallel to `validate_path_application_id` at `src/web/api.rs:500-589`). All existing call sites in 9-5's device handlers pass `"device"`; new command handlers pass `"command"`.

4. **Per-device `command_id` + `command_name` uniqueness in `AppConfig::validate`.** `AppConfig::validate` (`src/config.rs:977-1700`) does NOT enforce these today (verified: the device-walk loop has `seen_metric_names` / `seen_chirpstack_metric_names` HashSets at `:634-664` — added by Story 9-5 — but no `seen_command_ids` / `seen_command_names`). Two commands sharing a `command_id` within ONE device's `device_command_list` collide at OPC UA registration time (`src/opc_ua.rs:1059` constructs `NodeId::new(ns, command.command_id as u32)` — last-wins on `HashMap::insert` semantics, same root cause as issue #99 for metrics). Two commands sharing a `command_name` collide on operator-driven addressing in the web UI (and historically on any `command_name`-keyed lookup). Story 9-6 extends `AppConfig::validate` additively with the parallel HashSet pattern at `:634-664`. Without it, a POST with a duplicate `command_id` silently passes validation, the OPC UA address space registration silently overwrites, and the AC#3 duplicate-rejection test would falsely pass at the HTTP layer.

5. **Programmatic reload trigger** — every mutating handler calls `app_state.config_reload.reload().await` at the end of its successful path. Story 9-7's reload routine already serialises concurrent calls via its internal `tokio::sync::Mutex`; Story 9-4's `ConfigWriter::lock()` serialises CRUD-vs-CRUD; the two locks are acquired in the same order (`config_writer.lock` → `config_reload`'s internal mutex) — no double-mutex deadlock. Command-level mutations DO produce a non-zero command-diff via `commands_equal` (`src/config_reload.rs:917-948`); Story 9-7's `topology_device_diff` helper classifies these as `modified_devices += 1` per the iter-2 P26 device_command_list classifier fix.

6. **Audit logging** — **four** new `event=` names following the 9-4 / 9-5 grep-contract pattern: `event="command_created"` (info), `event="command_updated"` (info), `event="command_deleted"` (info), `event="command_crud_rejected"` (warn). Each carries `application_id`, `device_id`, `command_id`, `source_ip`; on rejection also `reason ∈ {validation, csrf, conflict, reload_failed, io, immutable_field, unknown_field, ambient_drift, poisoned, rollback_failed, application_not_found, device_not_found, command_not_found}`. One new reason value: `command_not_found` (PUT/DELETE/GET on a non-existent `:command_id`). Re-update `docs/logging.md` operations table.

7. **Static HTML + JS** — `static/commands.html` is currently a placeholder (`<p>Story 9-6 will fill this in (command CRUD).</p>`). Story 9-6 replaces it with a real CRUD page: per-application/per-device nested selector + commands table per selected device + create form + edit modal + delete-confirm. Vanilla JS — **no SPA framework, no build step, no `npm install`** — same minimal-footprint stance as 9-2 / 9-3 / 9-4 / 9-5. Mobile-responsive via the established media-query baseline. Header nav links added in `static/applications.html`, `static/devices-config.html`, `static/devices.html` (round-trip navigation).

This story is the **third and final CRUD landing** (9-4 = applications, 9-5 = devices + metrics, 9-6 = commands); after 9-6 ships, the FR34/35/36 cluster closes and only Story 9-8 (dynamic OPC UA address-space mutation) remains in the Epic 9 backlog. Story 9-6 explicitly resists scope creep into payload-template editing and `[command_validation.device_schemas]` CRUD (see Out of Scope).

The new code surface is **moderate**:

- **~250–350 LOC of CRUD handlers** in `src/web/api.rs` (extends the existing file; same shape as 9-5's `list_devices` / `get_device` / `create_device` / `update_device` / `delete_device`).
- **~30–50 LOC of CSRF literal-arm completion** in `src/web/csrf.rs` (adds 2 `"command" =>` warn arms; no helper or signature change).
- **~30 LOC of router wiring** in `src/web/mod.rs` — 2 new `.route(...)` calls.
- **~20–40 LOC of `validate_path_device_id` widening** in `src/web/api.rs` (parallel to iter-3 Blind#3 pattern; all device-handler call sites updated to pass `"device"`).
- **~80–120 LOC of validation extension** in `src/config.rs` (per-device `command_id` + `command_name` uniqueness + 2 new unit tests).
- **~250–350 LOC of HTML/CSS/JS** in `static/commands.html` + `static/commands.js` (nested resource — application selector → device selector → commands table).
- **~450–650 LOC of integration tests** in a new `tests/web_command_crud.rs` (≥ 25 tests after carry-forward + AC#11 cross-resource path-aware-CSRF regression + AC#5 carry-forward invariants).
- **Documentation sync**: `docs/security.md` § "Configuration mutations" gets a "Command CRUD" subsection; `docs/logging.md` operations table gains 4 rows; `README.md` Planning row updated.

---

## Out of Scope

- **Payload template editing.** `epics.md:894` mentions *"payload template, and validation rules"* in the BDD wording, but `DeviceCommandCfg` (`src/config.rs:660-670`) has only 4 fields: `command_id: i32`, `command_name: String`, `command_confirmed: bool`, `command_port: i32`. There is no `payload_template` field on the struct today. Adding one would be a schema change (also touching `src/opc_ua.rs:1856-1928` command-emission path + the `[command_validation.device_schemas]` integration). Story 9-6 ships CRUD on the **current 4 fields only**; payload-template editing is a future-story enhancement (would close the *"payload template"* clause of `epics.md:894`).
- **`[command_validation.device_schemas]` CRUD.** The schema-driven validation surface (Story 3-2 / `src/command_validation.rs`) is a **separate config section** keyed by `device_id` under `[command_validation]` (not under `[[application]]`). Editing schemas would require a parallel CRUD surface; out of scope for v1. The current `DeviceCommandCfg` fields are CRUD-able without touching schemas. The "validation rules" clause of `epics.md:894` maps onto this future surface; tracked as a future enhancement (no GitHub issue filed today — no operator demand surfaced).
- **`command_id` rename.** Like `application_id` in 9-4 and `device_id` in 9-5, `command_id` is **immutable** in PUT paths. Renaming would orphan storage rows in `command_queue` keyed by `command_id` (`src/storage/sqlite.rs:676`). Operator workaround: DELETE the command then POST a new one. Returns 400 + `reason="immutable_field"` if the PUT body contains `command_id`.
- **Cascade-delete of pending `command_queue` rows on command DELETE.** v1 leaves orphaned `command_queue` rows for a deleted `command_id`. Same precedent as Story 9-5's "no cascade-delete of `metric_values` / `metric_history` on device DELETE". Documented in `docs/security.md` § "Configuration mutations" → "v1 limitations".
- **ChirpStack-side validation of `command_name` or `command_port`.** Same deferral as 9-4's `application_id` and 9-5's `device_id` — v1 trusts the operator-supplied values; runtime command emission surfaces `f_port` violations via `src/storage/sqlite.rs:652` ("Invalid f_port {N}: must be 1-223") AFTER the OPC UA Write enqueue path runs.
- **CSRF synchronizer-token / double-submit cookie pattern.** v1 inherits 9-4's Origin + JSON-only Content-Type defence. Documented as v2 upgrade path.
- **Per-IP rate limiting on mutating routes.** Inherited deferral from 9-1 (issue #88) and 9-4 / 9-5. Same single-operator LAN threat model.
- **TLS / HTTPS.** Inherited deferral from 9-1 (issue #104).
- **Filesystem watch (`notify` crate).** Out of scope per 9-7's same deferral. CRUD handler + SIGHUP are the two reload triggers in v1.
- **Hot-reload of OPC UA command-node address-space mutation.** Story 9-7 deferred OPC UA address-space mutation to Story 9-8; Story 9-6's CRUD writes config + triggers reload, which fires `event="topology_change_detected"` (9-7 invariant carrying `modified_devices=N` per Story 9-7 iter-2 P26 device_command_list classifier fix), but the OPC UA address space stays at startup state until 9-8 lands. **Operator-visible:** newly created commands appear in the dashboard immediately; SCADA clients connected via OPC UA must reconnect to see the new command nodes. Documented in `docs/security.md`.
- **Atomic-rollback if `ConfigReloadHandle::reload()` fails after TOML write.** v1 inherits 9-4's / 9-5's best-effort rollback discipline (with the iter-1 D3-P poison flag). Same operator-action expectations.
- **SQLite-side persistence of command config.** Same architectural decision as 9-4 / 9-5: `[storage]` SQLite tables are runtime state (metric_values, command_queue, gateway_status), not configuration topology. Adding a `device_commands` table would be Epic-A scale.
- **Issue #108 (storage payload-less MetricType).** Orthogonal — 9-6 does not touch metric_values payload semantics.
- **Doctest cleanup** (issue #100). Not blocking; 9-6 adds zero new doctests.
- **POST/PUT body cap.** Inherited from 9-5 iter-1 D10 — auth-gated surface, default axum 2 MB acceptable for v1; cross-resource cap is a dedicated hardening story.
- **Body-validation-before-existence-check info disclosure.** Inherited from 9-5 iter-2 M2 / iter-1 D8 — authenticated operators can fingerprint command_ids by varying body validity; defer for consistency with 9-4 / 9-5 resolution.

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. Story 9-6 wires existing primitives together — the CRUD scaffold from 9-4 + the device-CRUD shape from 9-5 are the load-bearing reuse targets.

| What | Where | Status |
|------|-------|--------|
| `pub struct AppState { auth, backend, dashboard_snapshot, start_time, stale_threshold_secs, config_reload, config_writer }` | `src/web/mod.rs:222` (post-9-5) | **Wired today.** Story 9-6 reuses **unchanged** — no new field needed. |
| `pub struct ConfigWriter { config_path, write_lock, poisoned }` + full API (`lock`, `read_raw`, `parse_document_from_bytes`, `write_atomically`, `rollback`, `is_poisoned`) | `src/web/config_writer.rs` | **Wired today.** Generic over the TOML document; Story 9-6 calls the same methods to mutate `[[application.device.command]]` sub-tables. **Acquire `lock()` and hold it across the entire write+reload+(rollback) sequence** — same lost-update-race fix from 9-4 Task 2. |
| `pub fn csrf_middleware(...)` + `pub struct CsrfState` + `pub(crate) fn csrf_event_resource_for_path(path: &str) -> &'static str` | `src/web/csrf.rs:183-225` (helper), `:239-353` (middleware) | **Wired today (9-5).** The helper **already routes** `/api/applications/<app>/devices/<dev>/commands*` → `"command"` (verified: `src/web/csrf.rs:209-214`). What's missing: the rejection-emission `match` blocks at `:277-306` and `:318-344` have a catch-all `_ =>` arm for `"command"` that emits `event="crud_rejected"` (no resource prefix). Story 9-6 adds the literal `"command" => warn!(event = "command_crud_rejected", ...)` arm. **Per the source comment at `:271-275`, this is the explicit Story 9-6 hand-off.** |
| `pub struct ConfigReloadHandle::reload()` | `src/config_reload.rs:181-218` | **Wired today (Story 9-7).** Reused unchanged. |
| `commands_equal` + `topology_device_diff` device-level classifier | `src/config_reload.rs:917-948` + Story 9-7 iter-2 P26 | **Wired today.** `commands_equal` already does an ID-keyed comparison of `Option<Vec<DeviceCommandCfg>>` so reordering doesn't trigger a false diff. **Story 9-6 mutations DO trigger a non-zero `modified_devices` count** when commands are added/edited/removed on an existing device — verify at impl time. |
| `pub struct ChirpstackDevice { device_id, device_name, read_metric_list, device_command_list: Option<Vec<DeviceCommandCfg>> }` | `src/config.rs:570-598` | **Wired today.** `#[serde(rename = "command")]` on `device_command_list` means the TOML key is `[[application.device.command]]`. `Option<Vec<...>>` allows a device with zero commands to either omit the sub-table entirely (deserialises to `None`) or have an empty `[[application.device.command]]` array (rare but valid — deserialises to `Some(vec![])`). **Story 9-6 POST/PUT/DELETE handlers must preserve the `None`-vs-`Some(empty)` distinction** in TOML mutations: POST first command on a `None`-state device creates the sub-table; DELETE last command leaves `Some(empty)` unless we explicitly remove the sub-table (decision below). |
| `pub struct DeviceCommandCfg { command_id: i32, command_name: String, command_confirmed: bool, command_port: i32 }` | `src/config.rs:660-670` | **Wired today.** The 4 fields Story 9-6 ships CRUD for. **Derive list is `#[derive(Debug, Deserialize, Clone)]` — no `Serialize`.** Story 9-6 MUST use a parallel `CommandResponse` struct in `src/web/api.rs` rather than adding `Serialize` to this type (matches Story 9-5's `MetricMappingResponse` pattern). |
| `DeviceCommand::validate_f_port(f_port: u8) -> bool` | `src/storage/types.rs:153-157` | **Wired today.** Returns `(1..=223).contains(&f_port)`. **Story 9-6 reuses this** in `validate_command_field("command_port", ...)` — do NOT roll a parallel range check. The handler converts `i32` → `u8` via `u8::try_from` and rejects on the `Err(_)` path (negative or > 255) before invoking the helper. |
| `AppConfig::validate(&self) -> Result<(), OpcGwError>` | `src/config.rs:977-1700` | **Wired today.** Already enforces: per-device `metric_name` + `chirpstack_metric_name` uniqueness via `seen_metric_names` / `seen_chirpstack_metric_names` HashSets (Story 9-5 amendment at `:634-664`); `device_id` cross-application uniqueness via `seen_device_ids: HashSet`; `application_id` uniqueness via `seen_application_ids: HashSet`. **Story 9-6 extends additively** with per-device `command_id` + `command_name` uniqueness HashSets (modelled on the existing pattern). **No new validation rules at the handler level** — single source of truth. |
| `pub struct ApplicationSummary` / `pub struct DeviceSummary` / `pub struct MetricSpec` | `src/web/mod.rs:78-123` | **Wired today.** Story 9-6 does NOT extend these — commands are not on the dashboard snapshot path. Story 9-6's per-device GET handler reads the **live** `Arc<AppConfig>` via `state.config_reload.subscribe().borrow().clone()` (Story 9-5 access pattern for `read_metric_list`). Same `.clone()` + drop-the-borrow-before-`.await` discipline. |
| Story 9-4 / 9-5 helpers: `validate_application_field` / `validate_device_field` / `validate_path_application_id` / `application_not_found_response` / `device_not_found_response` / `internal_error_response` / `io_error_response` / `reload_error_response` / `handle_rollback` / `handle_restart_required` / `find_application_index` | `src/web/api.rs` (locate via `grep -n "fn <name>"`) | **All wired today and already `resource`-threaded.** Helpers that accept `resource: &'static str`: `find_application_index` (iter-3 Blind#3), `handle_rollback` (iter-1 HIGH H1), `io_error_response` (iter-1 HIGH H1), `reload_error_response` (iter-1 HIGH H1), `handle_restart_required` (iter-1 HIGH H1), `validate_path_application_id` (iter-1 HIGH H1). **Story 9-6 just adds `"command" =>` literal arms to each** (4 arms per match: one for origin/csrf rejection wording, one for content-type rejection wording, etc.). **Story 9-6 reuses each helper directly via existing visibility** (same module, no visibility change). |
| `validate_path_device_id` | `src/web/api.rs:600` | **Wired today (Story 9-5) — REQUIRES WIDENING in Story 9-6.** Currently hard-coded to emit `event="device_crud_rejected"`. Story 9-6 command handlers also invoke this helper for the `:device_id` segment; a command-handler-invoked path-validation failure would currently mis-emit `device_crud_rejected` instead of `command_crud_rejected`. Fix: add `resource: &'static str` parameter, dispatch event-name literal per arm (parallel to `validate_path_application_id`). All current call sites in Story 9-5's device handlers pass `"device"`; new command handlers pass `"command"`. |
| `config_type_to_display(t: &OpcMetricTypeConfig) -> &'static str` | `src/web/api.rs:199` (private `fn`) | **Wired today (Story 9-3 + 9-4 + 9-5).** **Not relevant for Story 9-6** — commands don't carry an OpcMetricTypeConfig. |
| Story 9-4 / 9-5 pre-flight malformed-block-rejection (iter-2 P35 + iter-3 P41) | `src/web/api.rs::create_application` / `update_application` / `delete_application` + `create_device` / `update_device` / `delete_device` pre-flight | **Wired today.** Story 9-6's mutating handlers apply the **same** pre-flight: walk the `[[application.device.command]]` array under the matching device; reject with 409 if any block has missing/non-integer `command_id`, missing/non-string `command_name`, missing/non-bool `command_confirmed`, or missing/non-integer `command_port` BEFORE the duplicate-detection / mutation step. |
| Story 9-4 / 9-5 lock+read_raw → parse_document_from_bytes → mutate → write_atomically → reload → on-error rollback discipline | `src/web/api.rs::create_application` and `src/web/api.rs::create_device` | **Wired today.** Story 9-6's mutating handlers replicate the **byte-for-byte equivalent** flow on the command sub-resource. Extract a shared helper if duplication exceeds ~40 lines per handler; otherwise inline. |
| Story 9-4 / 9-5 poison-flag check on `is_poisoned()` | `src/web/config_writer.rs` | **Wired today.** Story 9-6 inherits unchanged — no second `poisoned` field. |
| Story 9-4 / 9-5 audit-event reason taxonomy | `src/web/api.rs` rejection paths | **Wired today.** Story 9-6 reuses the same set + adds 1 new reason: `command_not_found` (404 — PUT/DELETE/GET on a non-existent `:command_id` under a known device). |
| `tracing-test::internal::global_buf()` log assertions + unique-per-test sentinels (Story 9-4 iter-2 P26) | `tests/web_application_crud.rs` + `tests/web_device_crud.rs` precedent | **Wired today.** Story 9-6 reuses the same pattern — `uuid::Uuid::new_v4().simple()` for the positive-path assertion to defeat parallel-test buffer-bleed. |
| `tempfile::NamedTempFile` for per-test isolation + `tests/common/mod.rs::make_test_reload_handle` + `crate::web::test_support::make_test_reload_handle_and_writer` | `tests/common/mod.rs` + `src/web/test_support.rs` | **Wired today (Story 9-4 Task 5).** Story 9-6's `tests/web_command_crud.rs` reuses the existing helpers via the established import path; do NOT roll new fixture construction. |
| `OpcGwError::Web(String)` variant | `src/utils.rs:618-626` | **Wired today.** Reused for Story 9-6 runtime errors. **No new variants.** |
| `toml_edit = "0.25.11"` direct dep | `Cargo.toml` | **Wired today (Story 9-4 Task 1).** Story 9-6 reuses; no new dep. |
| `build_device_table` + `build_read_metric_array` | `src/web/api.rs:2574, 2596` | **Wired today (Story 9-5 Task 6).** Story 9-6 adds two parallel helpers `build_command_table` + `build_command_array_of_tables` that construct `[[application.device.command]]` tables from a `CreateCommandRequest`. PUT-replace-command MUST mutate the matching `command` table in place (parallel to Story 9-5 PUT-replace-device for `read_metric`) — anti-pattern guard: do NOT serialise `DeviceCommandCfg` back via `toml::Value`. |
| Issue #99 NodeId fix (commit `9f823cc`) | `src/opc_ua.rs:966, 978, 1024-1032, 1059` | **Wired today (Epic 8 carry-forward, 2026-05-02).** The command NodeId is `NodeId::new(ns, command.command_id as u32)` at `:1059` — keyed by **`command_id` alone within a device's namespace** (no `device_id` prefix; the device folder NodeId already isolates per-device namespaces). Story 9-6's per-device `command_id` uniqueness enforcement (AC#3) is the prerequisite for the OPC UA layer to register without overwriting. Per-device uniqueness is sufficient; cross-device same-`command_id` is **safe** (the device folder NodeId namespaces the command). **No new OPC UA changes from Story 9-6** — issue #99 territory was metric NodeIds, not command NodeIds. |

---

## Acceptance Criteria

### AC#1 (FR36, epics.md:891-897): Command CRUD via web interface

- **Given** the authenticated web server (Stories 9-1 + 9-2 + 9-3 + 9-4 + 9-5 + 9-7) running with at least one configured application + device.
- **When** the operator navigates to `/commands.html` in a browser.
- **Then** the page shows a per-application/per-device nested selector (or accordion-of-accordions) listing each device's configured commands with `command_id`, `command_name`, `command_port` (f_port), `command_confirmed`, and a row of action buttons (`Edit`, `Delete`).
- **And** a "Create command" form (anchored under each device section) accepts `command_id` (integer ≥ 1) + `command_name` (non-empty trimmed string) + `command_port` (integer 1..=223) + `command_confirmed` (checkbox bool) and POSTs to `/api/applications/:application_id/devices/:device_id/commands`.
- **And** clicking `Edit` opens an inline edit form for `command_name` + `command_port` + `command_confirmed` (`command_id` is **read-only** because changing it would orphan `command_queue` rows; rename is rejected with 400 `"command_id is immutable; delete and recreate to change"`).
- **And** clicking `Delete` opens a confirmation dialog; on confirm, sends `DELETE /api/applications/:application_id/devices/:device_id/commands/:command_id`.
- **And** changes are validated before saving (per AC#3 below).
- **And** changes are persisted to `config/config.toml` (per AC#4 below). **(Spec amendment from epics.md:897: SQLite-side persistence deferred — see Out of Scope, same as 9-4 / 9-5.)**
- **Verification:**
  - Test: `tests/web_command_crud.rs::commands_html_renders_per_device_table` — `GET /commands.html` returns 200 with auth header + body contains nested per-application + per-device section markers (e.g., `<section data-application-id="..."` and `<section data-device-id="..."`) + a `<form` with `action` matching `/api/applications/{app}/devices/{dev}/commands` shape (or a `data-action-template` attribute that the JS expands at runtime) + `method="POST"` (or `data-method="POST"`).
  - Test: `tests/web_command_crud.rs::commands_js_fetches_api_commands_per_device` — `GET /commands.js` returns 200 with `Content-Type: text/javascript` (or `application/javascript`) + body contains `fetch("/api/applications/`.

### AC#2 (FR36): JSON CRUD endpoints with full lifecycle

Endpoints (all behind Basic auth via the existing layer + CSRF middleware via the existing layer):

| Method | Path | Request body | Success status | Response body |
|--------|------|--------------|---------------|---------------|
| `GET` | `/api/applications/:application_id/devices/:device_id/commands` | — | 200 | `{"application_id": "...", "device_id": "...", "commands": [{"command_id": 1, "command_name": "...", "command_port": 10, "command_confirmed": true}, ...]}` (404 if application or device not found) |
| `GET` | `/api/applications/:application_id/devices/:device_id/commands/:command_id` | — | 200 | `{"command_id": 1, "command_name": "...", "command_port": 10, "command_confirmed": true}` (404 if not found) |
| `POST` | `/api/applications/:application_id/devices/:device_id/commands` | `{"command_id": 1, "command_name": "...", "command_port": 10, "command_confirmed": true}` | 201 (with `Location: /api/applications/:app/devices/:dev/commands/:command_id` header) | `{"command_id": 1, "command_name": "...", "command_port": 10, "command_confirmed": true}` |
| `PUT` | `/api/applications/:application_id/devices/:device_id/commands/:command_id` | `{"command_name": "...", "command_port": 10, "command_confirmed": true}` (NO `command_id` in body — path is authoritative) | 200 | `{"command_id": 1, "command_name": "...", "command_port": 10, "command_confirmed": true}` |
| `DELETE` | `/api/applications/:application_id/devices/:device_id/commands/:command_id` | — | 204 | (empty body) |

- **And** the JSON response uses snake_case field names matching the existing `/api/status` + `/api/devices` + `/api/applications` + `/api/applications/:id/devices` convention.
- **And** error responses follow the existing `ErrorResponse { error: String, hint: Option<String> }` shape from Stories 9-2 / 9-4 / 9-5.
- **And** all routes inherit the Basic auth middleware via the layer-after-route invariant (Story 9-1 AC#5) AND the CSRF middleware (Story 9-4 AC#5, with the path-aware audit dispatch from Story 9-5's AC#5 extended to `"command"` per this story's Task 2).
- **And** `command_id` is serialised as a JSON **number** (not a string) — matches the `i32` type. The Location header uses the integer's `Display` form (no leading zeros).
- **And** the `:command_id` URL path segment is parsed as `i32` by the handler (or via a `Path<(String, String, i32)>` extractor); a non-numeric / negative / out-of-i32-range path segment returns 400 + `event="command_crud_rejected" reason="validation" field="command_id"` (NOT 404 — the path is structurally malformed, not "resource not found").
- **Verification:**
  - Test: `tests/web_command_crud.rs::get_commands_returns_seeded_list_under_device` — start the server with a 1-app/1-device/2-command config; `GET /api/applications/:app/devices/:dev/commands` returns 200 + JSON body with both commands.
  - Test: `tests/web_command_crud.rs::get_commands_returns_404_for_unknown_device` — `GET /api/applications/:app/devices/nonexistent/commands` returns 404 + body `{"error": "device not found", "hint": null}`.
  - Test: `tests/web_command_crud.rs::get_commands_returns_404_for_unknown_application` — `GET /api/applications/nonexistent/devices/:dev/commands` returns 404 + body `{"error": "application not found", "hint": null}`.
  - Test: `tests/web_command_crud.rs::get_command_by_id_returns_404_for_unknown_command` — `GET /api/applications/:app/devices/:dev/commands/9999` returns 404 + body `{"error": "command not found", "hint": null}`.
  - Test: `tests/web_command_crud.rs::get_command_with_non_numeric_path_returns_400` — `GET /api/applications/:app/devices/:dev/commands/not-a-number` returns 400 + audit log emits `event="command_crud_rejected" reason="validation" field="command_id"`.
  - Test: `tests/web_command_crud.rs::post_command_creates_then_get_returns_201` — POST a fresh command; assert 201 + `Location` header points at `/api/applications/:app/devices/:dev/commands/<command_id>`; subsequent `GET .../commands/<command_id>` returns 200 + body matches.
  - Test: `tests/web_command_crud.rs::post_command_on_device_with_none_command_list_creates_subtable` — start with a device whose `[[application.device.command]]` sub-table is absent (`device_command_list = None` post-deserialisation); POST a command; assert 201; verify the config TOML now contains a `[[application.device.command]]` block.
  - Test: `tests/web_command_crud.rs::put_command_updates_fields_then_get_reflects` — PUT a new `command_name` + `command_port` + `command_confirmed`; assert 200 + body has new values; subsequent `GET .../commands/:id` reflects.
  - Test: `tests/web_command_crud.rs::delete_command_returns_204_then_404` — DELETE a command; assert 204 (no body); subsequent `GET .../commands/:id` returns 404.

### AC#3 (FR40, epics.md:896): Validation BEFORE write; rollback ON reload failure

> **Validate-side contract amendment (load-bearing):** `AppConfig::validate` does NOT enforce per-device `command_id` or `command_name` uniqueness today (verified: the device-walk loop has `seen_metric_names` / `seen_chirpstack_metric_names` HashSets at `src/config.rs:634-664` from Story 9-5, but no parallel HashSets for the `device_command_list` field). Two commands sharing `command_id` within ONE device collide on the OPC UA NodeId `NodeId::new(ns, command.command_id as u32)` at `src/opc_ua.rs:1059`, silently overwriting via `HashMap::insert` last-wins (same class as issue #99 for metric NodeIds, but per-device-scoped instead of cross-device). Two commands sharing `command_name` collide on operator-driven addressing in the web UI and on any future `command_name`-keyed lookup. Story 9-6 extends `AppConfig::validate` to ALSO reject duplicate `command_id` AND duplicate `command_name` within a single `device.device_command_list`, modelled on the existing pattern at `:634-664`. This is an **additive** edit to `src/config.rs` (allowed under file-modification scope) and is **load-bearing for AC#3's duplicate-rejection tests below**. Without it, a POST with a duplicate `command_id` silently passes validation, the OPC UA address space registration silently overwrites, and the AC#3 test would falsely pass at the HTTP layer. **Tracked in Task 1 sub-bullets.**

- **Given** any mutating CRUD request.
- **When** the request body fails handler-level shape validation:
  - missing `command_id` / `command_name` / `command_port` / `command_confirmed` (POST), or missing `command_name` / `command_port` / `command_confirmed` (PUT)
  - `command_id` not a positive integer (≤ 0 or non-integer JSON value)
  - `command_name` empty after `.trim()` (Story 9-4 iter-1 P16 precedent — whitespace-only strings rejected)
  - `command_name` length out of `[1, 256]` (Story 9-4 / 9-5 precedent)
  - `command_name` violates char-class — accept `is_valid_app_name_char` (Story 9-5's name-class: ASCII alphanumerics, `'-'`, `'_'`, `'.'`, spaces, parentheses) since command names are operator-facing labels, not identifiers used in URL paths
  - `command_port` not in `[1, 223]` (per `DeviceCommand::validate_f_port` at `src/storage/types.rs:155` — LoRaWAN application f_port range)
  - `command_confirmed` not a bool (serde-side rejection)
- **Then** the handler returns 400 + `{"error": "...", "hint": "..."}` BEFORE touching the TOML file.
- **And** when handler-level validation passes BUT the post-write `ConfigReloadHandle::reload()` returns `Err(ReloadError::Validation(_))` because the TOML mutation produced an `AppConfig` that fails `AppConfig::validate()` (e.g., duplicate `command_id` within the device, duplicate `command_name` within the device).
- **Then** the handler restores the pre-write TOML bytes from the in-memory backup, returns 422 Unprocessable Entity + `{"error": "...", "hint": "..."}` carrying the validation error message.
- **And** the post-write reload's `Err(ReloadError::Io(_))` and `Err(ReloadError::RestartRequired { knob })` paths inherit Stories 9-4 / 9-5 discipline: rollback bytes, return 500 (Io) or the iter-1 D1-P 409 (ambient drift refusal).
- **Verification:**
  - Test: `tests/web_command_crud.rs::post_command_with_empty_name_returns_400` — POST `{"command_id": 1, "command_name": "", "command_port": 10, "command_confirmed": true}` returns 400 + body mentions `command_name` + the TOML file is unchanged on disk.
  - Test: `tests/web_command_crud.rs::post_command_with_port_below_range_returns_400` — POST with `command_port: 0` returns 400 + body mentions `command_port` + range `[1, 223]`.
  - Test: `tests/web_command_crud.rs::post_command_with_port_above_range_returns_400` — POST with `command_port: 224` returns 400 + body mentions `command_port` + range.
  - Test: `tests/web_command_crud.rs::post_command_with_negative_id_returns_400` — POST with `command_id: -1` returns 400 + body mentions `command_id`.
  - Test: `tests/web_command_crud.rs::post_command_with_zero_id_returns_400` — POST with `command_id: 0` returns 400 (positive-i32 contract).
  - Test: `tests/web_command_crud.rs::post_command_with_duplicate_command_id_within_device_returns_422` — POST a `command_id` already present in the device's `device_command_list`; assert 422 + body mentions duplicate `command_id`. Pre/post TOML byte-equality is asserted to confirm rollback (Story 9-5 iter-1 patch — see Validation Patches Inherited section below).
  - Test: `tests/web_command_crud.rs::post_command_with_duplicate_command_name_within_device_returns_422` — POST a `command_name` already present in the device's `device_command_list`; assert 422.
  - Test: `tests/web_command_crud.rs::put_command_id_in_body_is_rejected` — PUT body containing `{"command_id": 999}` returns 400 OR 422 (per Story 9-4 iter-1 P5 / iter-2 P29 deferred-work item — axum maps `serde(deny_unknown_fields)` to 422 by default; Story 9-6 inherits the cosmetic spec/impl divergence) + body mentions `command_id is immutable` (when 400) or includes the `unknown_field` rejection (when 422). Test relaxed to accept either.
  - Test: `tests/web_command_crud.rs::post_command_with_same_command_id_on_different_device_succeeds` — POST `command_id: 1` on dev-A; POST `command_id: 1` on dev-B (same application); assert both succeed (201) + the per-device-NodeId-namespace argument from Existing Infrastructure table holds.

### AC#4 (FR40): TOML round-trip via `toml_edit`; atomic write; preserve sibling sub-tables

- **Given** any successful mutating CRUD request on a command.
- **When** the handler reaches the write step.
- **Then** the file write is **atomic** via `ConfigWriter::write_atomically` (per Story 9-4 contract: tempfile + rename + dir-fsync).
- **And** the resulting TOML file preserves all operator-edited comments + key order + whitespace from the original.
- **And** any existing `[[application.device.read_metric]]` sub-table under the modified device is **preserved** byte-for-byte (Story 9-5 territory; Story 9-6 must not inadvertently strip metric blocks via a serialise-via-`toml::Value` path). Symmetric to Story 9-5 Task 6.
- **And** any **other** application's `[[application]]` block, any **other** device's `[[application.device]]` block under the same application, and any **other** command's `[[application.device.command]]` block under the same device is preserved byte-for-byte.
- **And** the resulting TOML round-trips cleanly through `figment::Toml::file(...)` + `AppConfig::deserialize` in the post-write reload.
- **And** command ordering is preserved: a sequence of POSTs producing `command_id = [1, 2, 3]` results in TOML where `[[application.device.command]]` blocks appear in that order. DELETE of `command_id = 2` produces TOML with blocks `[1, 3]` (in-place removal, no reordering).
- **Verification:**
  - Test: `tests/web_command_crud.rs::post_command_preserves_comments` — seed the config TOML with a `# OPERATOR_COMMAND_COMMENT_MARKER` line in a `[[application.device.command]]` block; POST a NEW command on the same device; read the file back; assert the marker line is still present + the new `[[application.device.command]]` block was appended.
  - Test: `tests/web_command_crud.rs::put_command_preserves_read_metric_subtable` — seed a device with a `[[application.device.read_metric]]` sub-table (Story 9-5 territory) AND a `[[application.device.command]]` sub-table; PUT a `command_name` rename on one command; read the file back; assert the `read_metric` sub-table is byte-equal to the original. **Symmetric to Story 9-5's `put_device_preserves_command_subtable` — load-bearing regression guard for Story 9-5.**
  - Test: `tests/web_command_crud.rs::post_command_preserves_other_devices_commands` — seed 2 devices under the same application, each with 1 command; POST a new command on dev-A; assert dev-B's command block is byte-equal to the original.
  - Test: `tests/web_command_crud.rs::delete_command_preserves_other_commands_under_device` — seed a device with 3 commands `[1, 2, 3]`; DELETE `command_id = 2`; assert the resulting TOML has `[[application.device.command]]` blocks for ONLY `command_id = 1` and `command_id = 3`, in that order, byte-equal to their pre-delete content (modulo whitespace between blocks if `toml_edit` re-formats — verify behaviour at impl time and choose the assertion-tolerance accordingly).

### AC#5 (CSRF carry-forward + literal-arm completion): Stories 9-4 / 9-5 defence + per-resource event dispatch

- **Given** any POST / PUT / DELETE request to `/api/applications/:application_id/devices/:device_id/commands*`.
- **When** the request fails the Story 9-4 CSRF defence (missing/mismatched Origin, missing/wrong Content-Type).
- **Then** the request is rejected with the same status codes (403 / 415) + ErrorResponse body shape from Story 9-4 / 9-5.
- **And** the audit-event name is `event="command_crud_rejected" reason="csrf"` (NOT `event="crud_rejected"` — this is the Story 9-6 literal-arm completion per the source comment at `src/web/csrf.rs:271-275`).
- **And** Story 9-4's `event="application_crud_rejected" reason="csrf"` continues to fire for `/api/applications/*` routes (Story 9-4 invariant).
- **And** Story 9-5's `event="device_crud_rejected" reason="csrf"` continues to fire for `/api/applications/:id/devices*` routes that do NOT match the commands sub-resource (Story 9-5 invariant).
- **And** GET requests are NOT subject to CSRF checks (idempotent + safe — Story 9-4 invariant).
- **Verification:**
  - Test: `tests/web_command_crud.rs::post_command_without_origin_returns_403_with_command_event` — POST `/api/applications/:app/devices/:dev/commands` with valid auth + valid JSON body but no `Origin`; assert 403 + warn log emitted with `event="command_crud_rejected" reason="csrf"`.
  - Test: `tests/web_command_crud.rs::post_command_with_cross_origin_returns_403_with_command_event` — POST with `Origin: http://evil.example.com`; assert 403 + warn log with `event="command_crud_rejected"`.
  - Test: `tests/web_command_crud.rs::post_application_csrf_event_unchanged_under_9_6_changes` — Story 9-4 regression: POST `/api/applications` with no Origin; assert the warn log still emits `event="application_crud_rejected"` (NOT `command_crud_rejected`, NOT `crud_rejected`).
  - Test: `tests/web_command_crud.rs::post_device_csrf_event_unchanged_under_9_6_changes` — Story 9-5 regression: POST `/api/applications/:app/devices` with no Origin; assert the warn log still emits `event="device_crud_rejected"` (NOT `command_crud_rejected`).
  - Test: `tests/web_command_crud.rs::post_command_with_form_urlencoded_returns_415` — POST with `Content-Type: application/x-www-form-urlencoded`; assert 415 + warn log with `event="command_crud_rejected"`.

### AC#6 (delete safety): Application + device + command existence preconditions

- **Given** any **mutating** request (POST / PUT / DELETE) to `/api/applications/:application_id/devices/:device_id/commands*`.
- **When** `:application_id` does not match any configured application.
- **Then** the request is rejected with 404 Not Found + `{"error": "application not found", "hint": "verify the application_id; navigate to /applications.html to list configured applications"}`.
- **And** the audit log emits `event="command_crud_rejected" reason="application_not_found"` warn.
- **Given** an existing application.
- **When** `:device_id` does not match any device under that application (POST / PUT / DELETE).
- **Then** the request is rejected with 404 Not Found + `{"error": "device not found", "hint": "verify the device_id; navigate to /devices-config.html to list configured devices"}`.
- **And** the audit log emits `event="command_crud_rejected" reason="device_not_found"` warn.
- **Given** an existing application + device.
- **When** `:command_id` does not match any command under that device (PUT / DELETE).
- **Then** the request is rejected with 404 Not Found + `{"error": "command not found", "hint": "verify the command_id; navigate to /commands.html to list configured commands"}`.
- **And** the audit log emits `event="command_crud_rejected" reason="command_not_found"` warn.
- **And** the TOML file is unchanged.
- **GET 404s are NOT audit events.** GET-side not-found responses return the same 404 + ErrorResponse body but do NOT emit a `_crud_rejected` warn log — `_crud_rejected` is reserved for state-changing rejections (Story 9-4 / 9-5 audit-event semantic preserved). **Exception:** path-validation failures (non-numeric `:command_id`) DO emit `_crud_rejected` regardless of HTTP method, mirroring Story 9-5's Decision-2 ("path-shape rejection IS a CRUD rejection regardless of method").
- **Note:** Story 9-6 does NOT reject deleting the last command under a device. The post-9-4 warn-demotion of empty lists (`src/config.rs:1586-1595`) means a device can have zero commands; the post-delete state results in `device_command_list = Some(empty)` or the sub-table being removed entirely (decision per Task 6 below).
- **Verification:**
  - Test: `tests/web_command_crud.rs::delete_command_under_unknown_application_returns_404` — DELETE under a non-existent application; assert 404 + body + warn log `reason="application_not_found"` + TOML unchanged.
  - Test: `tests/web_command_crud.rs::delete_command_under_unknown_device_returns_404` — DELETE under a known application but unknown device; assert 404 + warn log `reason="device_not_found"` + TOML unchanged.
  - Test: `tests/web_command_crud.rs::delete_unknown_command_under_known_device_returns_404` — DELETE a non-existent `command_id` under a known device; assert 404 + warn log `reason="command_not_found"` + TOML unchanged.
  - Test: `tests/web_command_crud.rs::delete_last_command_under_device_succeeds` — start with a 1-app/1-device/1-command config; DELETE the only command; assert 204 + the device now has zero commands in subsequent GETs.

### AC#7 (FR40 reload integration): Programmatic reload after write

- **Given** any successful CRUD write on a command.
- **When** the handler completes the TOML write.
- **Then** the handler calls `app_state.config_reload.reload().await` BEFORE returning the HTTP response.
- **And** on `Ok(ReloadOutcome::Changed { .. })` — the expected outcome for any command-level mutation — the handler proceeds to write the success response (201/200/204).
- **And** the existing `run_web_config_listener` task picks up the new `Arc<AppConfig>` from the watch channel (Story 9-7); the next `GET /api/applications/:app/devices/:dev/commands` call sees the new state.
- **And** the existing `run_opcua_config_listener` task emits the `event="topology_change_detected"` info log carrying `modified_devices=K` (Story 9-7 iter-2 P26 device_command_list classifier fix). Command-level mutations classify as `modified_devices += 1`, NOT `added_devices` or `removed_devices`.
- **And** the OPC UA address space stays at startup state until Story 9-8 lands (carry-forward from 9-7 / 9-4 / 9-5). **Operator-visible:** the dashboard reflects the new command immediately; SCADA clients connected via OPC UA must reconnect to see the new command nodes. **Documented in `docs/security.md` § Configuration mutations § v1 limitations.**
- **Verification:**
  - Test: `tests/web_command_crud.rs::post_command_triggers_reload_and_subsequent_get_reflects` — POST a new command; immediately afterwards `GET /api/applications/:app/devices/:dev/commands`; assert the new command is present within 1 second (poll-with-budget pattern from 9-4 / 9-5).
  - Test: `tests/web_command_crud.rs::post_command_emits_command_created_event` — POST a new command; assert `tracing_test::internal::global_buf()` contains an `event="command_created"` line carrying `application_id` + `device_id` + `command_id` + `source_ip="127.0.0.1"` + a unique-per-test sentinel for the positive-path assertion (Story 9-4 iter-2 P26 pattern).
  - Test: `tests/web_command_crud.rs::post_command_emits_topology_change_log` — POST a new command; assert the captured logs contain `event="topology_change_detected"` with `modified_devices=1` (or similar — the exact field name depends on 9-7's implementation; verify at impl time).

### AC#8 (NFR12 carry-forward + grep contract): Audit logging shape

- **Given** the existing `event="..."` audit-event convention (Stories 6-1 → 9-7).
- **When** any CRUD outcome is emitted on the command surface.
- **Then** the new events match: `command_created` (info), `command_updated` (info), `command_deleted` (info), `command_crud_rejected` (warn). All four carry `source_ip` + `application_id` + `device_id` **(always)** plus `command_id` **(when applicable — populated for success events `command_created/updated/deleted` AND for rejection events that fail AFTER `command_id` is known; ABSENT on early-validation rejections such as malformed-body, missing-body-field, or path-validation failures BEFORE the command_id is parsed)**. Rejected events also carry `reason ∈ {validation, csrf, conflict, reload_failed, io, immutable_field, unknown_field, ambient_drift, poisoned, rollback_failed, application_not_found, device_not_found, command_not_found}`. On rejection, the sanitised `error: %e` field is included (NFR7 — no secrets, but `application_id` / `device_id` / `command_id` / `command_name` are NOT secrets and are included for operator-action triage).
- **And** zero changes to `src/main.rs::initialise_tracing` (NFR12 startup-warn invariant from `9-1:259`).
- **And** Story 9-4's `event="application_*"` grep contract continues to return exactly 4 lines (no regression).
- **And** Story 9-5's `event="device_*"` grep contract continues to return exactly 4 lines (no regression).
- **And** Story 9-7's `event="config_reload_*"` grep contract continues to return exactly 3 lines (no regression).
- **Verification:**
  - `git grep -hoE 'event = "command_[a-z_]+"' src/ | sort -u` returns exactly 4 lines (`command_created`, `command_updated`, `command_deleted`, `command_crud_rejected`).
  - `git grep -hoE 'event = "device_[a-z_]+"' src/ | sort -u` continues to return exactly 4 lines (Story 9-5 invariant).
  - `git grep -hoE 'event = "application_[a-z_]+"' src/ | sort -u` continues to return exactly 4 lines (Story 9-4 invariant).
  - `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` continues to return exactly 3 lines (Story 9-7 invariant).
  - `git diff HEAD --stat src/main.rs::initialise_tracing` shows zero changes to the function body.

### AC#9 (FR41 carry-forward): Mobile-responsive `commands.html`

- **Given** the existing `static/dashboard.css` baseline + Stories 9-4 / 9-5 mobile-responsive precedent.
- **When** `static/commands.html` is rendered in a browser at viewport widths < 600px.
- **Then** the per-application/per-device accordion collapses to single-column rows + the action buttons stack vertically + the create-form scales to 100% width.
- **And** the `<meta viewport>` tag is present.
- **And** the create form + edit modal scale to 100% width on mobile.
- **And** `commands.html` reuses `static/dashboard.css` (inline `<style>` block in `commands.html` for any command-specific overrides, per the 9-5 pattern).
- **Verification:**
  - Test: `tests/web_command_crud.rs::commands_html_carries_viewport_meta` — `GET /commands.html` body contains `<meta name="viewport"`.
  - Test: `tests/web_command_crud.rs::commands_uses_dashboard_css_baseline` — body of `commands.html` contains `<link rel="stylesheet" href="/dashboard.css"`.

### AC#10 (file invariants): Stories 9-1 / 9-2 / 9-3 / 9-4 / 9-5 / 9-7 / 7-2 / 7-3 / 8-3 zero-LOC carry-forward

- **And** `git diff HEAD --stat src/web/auth.rs src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/opc_ua_history.rs src/security.rs src/security_hmac.rs src/main.rs::initialise_tracing src/opc_ua.rs` shows ZERO production-code changes.
  - **Note: `src/opc_ua.rs` is untouched by Story 9-6.** The command NodeId construction at `src/opc_ua.rs:1059` is `NodeId::new(ns, command.command_id as u32)` — keyed by `command_id` within the per-device namespace. Per-device `command_id` uniqueness (AC#3) is the prerequisite that prevents collision; Story 9-6 enforces it at the `AppConfig::validate` layer. **No new OPC UA changes from Story 9-6.**
- **And** `src/config_reload.rs` may be modified **only additively** if a new field on `WebConfig` is required (none anticipated for 9-6 — the CSRF middleware already exists from 9-4 / 9-5). If the dev agent identifies a need to extend `web_equal` for a new field, the same Story 9-4 / 9-5 amendment applies (additive destructure-pattern extension only; no other edits permitted). `commands_equal` (`src/config_reload.rs:917-948`) is already implemented per Story 9-7 and Story 9-6 does NOT modify it.
- **And** the existing `event="config_reload_attempted/succeeded/failed"` and `event="topology_change_detected"` events still fire on the CRUD-triggered reload path (9-7 invariant — no regression).
- **Verification:**
  - `git diff HEAD --stat src/config_reload.rs` shows zero changes (anticipated) OR only additive `web_equal` extensions if a new field is needed.
  - `git diff HEAD --stat` for the strict zero-LOC files above shows zero changes; cargo test still passes; `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` continues to return exactly 3 lines.

### AC#11 (Stories 9-4 / 9-5 path-aware-CSRF cross-resource regression — load-bearing)

- **Given** Story 9-6 adds 2 literal `"command" =>` arms to the CSRF middleware's rejection emission (`src/web/csrf.rs:277-306` and `:318-344`) + threads `resource: &'static str` through `validate_path_device_id`.
- **When** Story 9-4's and 9-5's existing tests run.
- **Then** Story 9-4's `event="application_*"` grep contract = 4 (unchanged) AND Story 9-5's `event="device_*"` grep contract = 4 (unchanged).
- **And** the existing Story 9-4 and Story 9-5 integration tests (in `tests/web_application_crud.rs` and `tests/web_device_crud.rs`) MUST continue to pass byte-for-byte. Story 9-6 cannot regress the existing test suite.
- **And** the `validate_path_device_id` widening's all-call-site invariant: every existing call site in `src/web/api.rs` (device handlers) passes `"device"` and emits `event="device_crud_rejected"` (Story 9-5 invariant); new command-handler call sites pass `"command"` and emit `event="command_crud_rejected"`.
- **Verification:**
  - Test: `tests/web_command_crud.rs::post_application_csrf_event_unchanged_under_9_6_changes` (also part of AC#5 — regression-pin for Story 9-4).
  - Test: `tests/web_command_crud.rs::post_device_csrf_event_unchanged_under_9_6_changes` (also part of AC#5 — regression-pin for Story 9-5).
  - Test: `src/web/api.rs::tests::validate_path_device_id_under_command_resource_emits_command_event` — unit test that calls `validate_path_device_id(BAD_DEVICE_ID_WITH_CRLF, addr, "command")` and asserts the captured log emits `event="command_crud_rejected"` (NOT `device_crud_rejected`).
  - Test: `src/web/api.rs::tests::validate_path_device_id_under_device_resource_still_emits_device_event` — same helper invoked with `resource="device"` (the Story 9-5 call sites' behaviour); assert `event="device_crud_rejected"`. **Pins the Story 9-5 invariant under the Story 9-6 widening.**
  - `cargo test --test web_application_crud` passes ALL existing tests with zero failures.
  - `cargo test --test web_device_crud` passes ALL existing tests with zero failures.

### AC#12 (NFR9 + NFR7 carry-forward): Permission + secret hygiene preserved on CRUD

- **Given** the post-write reload routine re-invokes `AppConfig::validate()` (which includes the existing `validate_private_key_permissions` re-check from Story 9-7 AC#9).
- **When** the operator-supplied input would somehow surface a private key path with loose permissions (only theoretically possible if 9-6 adds path fields — `command_id`, `command_name`, `command_port`, `command_confirmed` are all non-path scalars).
- **Then** the existing `validate_private_key_permissions` re-check catches it and the reload is rejected — 9-6 inherits this for free (same shape as 9-4 / 9-5).
- **And** no secret values (`api_token`, `user_password`, `web` password) are emitted in any of the four new audit events. `application_id` / `device_id` / `command_id` / `command_name` are NOT secrets — they are operator-supplied identifiers.
- **Verification:**
  - Test: `tests/web_command_crud.rs::command_crud_does_not_log_secrets_success_path` — set `chirpstack.api_token = "SECRET_SENTINEL_TOKEN_DO_NOT_LEAK"` in the test config; POST a new command (success path); grep captured logs for the sentinel; assert zero matches.
  - Test: `tests/web_command_crud.rs::command_crud_io_failure_does_not_log_secrets` — same sentinel token; POST a command with valid handler-level shape; corrupt the TOML on disk between the write and the reload (chmod-000 the file via `std::os::unix::fs::PermissionsExt`) so reload fails with `ReloadError::Io(_)`; assert `status == 500` (Story 9-5 iter-1 E13 precedent — pin to `INTERNAL_SERVER_ERROR`, not `assert_ne!(CREATED)`); grep the captured logs for the sentinel; assert zero matches. Wrap the chmod-000 in a hand-rolled RAII guard (small Drop-impl struct that restores perms in `drop()`) so tempdir cleanup runs even if the assertion panics (Story 9-5 iter-1 L12 / B18 precedent at `tests/web_device_crud.rs:1578` — "scopeguard-style RAII", no `scopeguard` crate import).

### AC#13 (test count + clippy + grep contracts)

- `cargo test --lib --bins --tests` reports **at least 1056 passed** (1004 baseline from Story 9-5 + 42 integration tests in `tests/web_command_crud.rs` per Task 9 list + the new `delete_last_command_leaves_clean_toml_round_trip` test from Task 6 = ≥ 43 integration tests + 3 unit tests in `src/config.rs::tests` (per-device `command_id` + `command_name` uniqueness + `test_validation_same_command_id_across_devices_is_allowed` cross-device-allowed pin) + 5 unit tests in `src/web/api.rs::tests` (the AC#11 `validate_path_device_id` widening tests + the 3 `validate_path_command_id` parsing tests from Task 3) + 2 unit tests in `src/web/csrf.rs::tests` (the new `csrf_rejects_post_command_emits_command_event` + `csrf_rejects_post_command_form_urlencoded_emits_command_event` from Task 2)). The floor is set as a safety margin; the dev agent should land closer to ≥ 1056 with reasonable test discipline.
- `cargo clippy --all-targets -- -D warnings` is clean.
- `cargo test --doc` reports 0 failed (56 ignored — pre-existing #100 baseline, unchanged).
- New integration test file count grows by 1.
- No new direct dependencies (Story 9-6 reuses the Story 9-4 `toml_edit` dep + the existing `tempfile` / `reqwest` / `tracing-test` dev-deps). **No `scopeguard` crate**: the chmod-cleanup pattern is hand-rolled RAII inline in tests (per Story 9-5 precedent at `tests/web_device_crud.rs:1578` — comment reads *"use scopeguard-style RAII"*, no crate import).
- `git grep -hoE 'event = "command_[a-z_]+"' src/ | sort -u` returns exactly 4 lines.
- `git grep -hoE 'event = "application_[a-z_]+"' src/ | sort -u` returns exactly 4 lines (Story 9-4 invariant).
- `git grep -hoE 'event = "device_[a-z_]+"' src/ | sort -u` returns exactly 4 lines (Story 9-5 invariant).
- `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` returns exactly 3 lines (Story 9-7 invariant).

---

## Knob Taxonomy Update (re Story 9-7)

Story 9-7's `classify_diff` (`src/config_reload.rs:274`) classifies `application_list` as **address-space-mutating**. Story 9-6's CRUD handlers all trigger this code path through `[[application.device.command]]` mutations:

- **POST `/api/applications/:app/devices/:dev/commands`** → `application_list[i].device_list[j].device_command_list` content changes (or transitions from `None` to `Some([...])`) → `classify_diff` flags topology change via `devices_equal` deep-compare (Story 9-7 iter-2 P26 device_command_list classifier fix) → reload swap proceeds → web listener swaps dashboard snapshot → OPC UA listener logs `topology_change_detected` (modified_devices=1).
- **PUT `/api/applications/:app/devices/:dev/commands/:command_id`** → `device_command_list[k].command_name`/`command_port`/`command_confirmed` changes → `classify_diff` flags topology change (because `commands_equal` compares deeply per Story 9-7 iter-2 P26).
- **DELETE `/api/applications/:app/devices/:dev/commands/:command_id`** → `device_command_list.len()` decreases → topology change.

**No new entries needed in the Knob Taxonomy table.** Story 9-6's CRUD surface operates entirely within the existing "address-space-mutating" bucket; Story 9-8 will eventually pick up the actual OPC UA address-space mutations driven by these CRUD calls (including command-node add/remove/modify).

---

## CSRF Literal-Arm Completion (Story 9-5 follow-up — load-bearing for AC#5/AC#8)

Story 9-5's `csrf_middleware` (`src/web/csrf.rs:277-306` and `:318-344`) dispatches the rejection audit-event name by URL path resource. Both rejection match blocks have arms for `"application"` and `"device"` plus a catch-all `_ =>` that emits `event="crud_rejected"` (no resource prefix). The `"command"` variant returned by `csrf_event_resource_for_path` (lines 209-214) currently lands in that catch-all. Per the source comment at `:271-275`:

> *"The 'command' arm intentionally falls through to the generic catch-all in Story 9-5 — Story 9-6 will replace the catch-all with a literal `command_crud_rejected` warn when commands CRUD lands. Adding the literal here today would constitute Story 9-5 scope creep."*

**Refinement adopted by Story 9-6:** add the literal `"command" => warn!(event = "command_crud_rejected", ...)` arm at both rejection-emission sites. The catch-all `_ =>` remains in place for any future un-routed resource (currently unreachable but defensive). **No change to `csrf_event_resource_for_path` itself** — it already routes correctly.

```rust
// In src/web/csrf.rs, BOTH rejection-emission match blocks gain:
//   :277-306 (Origin/Referer reject)
//   :318-344 (Content-Type reject)

match resource {
    "device" => warn!(event = "device_crud_rejected", ...),
    "application" => warn!(event = "application_crud_rejected", ...),
    "command" => warn!(event = "command_crud_rejected", ...),  // NEW in 9-6
    _ => warn!(event = "crud_rejected", resource = resource, ...),
}
```

**Why per-resource event names rather than a single `crud_rejected` with a `resource` field** (same reasoning as Story 9-5 CSRF Path-Aware Dispatch section):

1. **Preserves Stories 9-4 / 9-5 grep contracts.** `application_*` count = 4 + `device_*` count = 4 are pinned by AC#8 in each predecessor.
2. **Matches the existing per-resource success-event pattern.** `application_created` / `device_created` / now `command_created` follow the same convention for grep symmetry.
3. **Story 9-6 is the natural completion point.** The catch-all-becomes-literal hand-off was anticipated in the Story 9-5 source comment.
4. **No information loss.** The `path` field on the audit event already carries the full URL path; operators can grep on `path` for fine-grained analysis.

**Implementation notes:**

- The `csrf_event_resource_for_path` helper itself does NOT change (it already returns `"command"` correctly per Story 9-5).
- Story 9-5's existing CSRF unit tests (`csrf_passes_safe_methods`, `csrf_rejects_post_with_no_origin`, `csrf_rejects_post_with_form_urlencoded_content_type`, `csrf_event_resource_for_path_maps_correctly`) MUST continue to pass byte-for-byte. The new literal-arm addition does not change their assertions (they assert on status codes + body shape, not on event-name regex).
- A new unit test `csrf_rejects_post_command_emits_command_event` pins the new arm at the unit-test layer: send a POST to `/api/applications/foo/devices/bar/commands` through the CSRF middleware with no Origin header; capture the warn log; assert it carries `event="command_crud_rejected"` (NOT `crud_rejected`).

---

## Validation Patches Inherited from Story 9-5 Iter-Loop

These patches landed in Story 9-5's iter-1/iter-2/iter-3 reviews and Story 9-6 MUST preserve them. Each is a one-line carry-forward — Story 9-6's test list and helper authorship MUST not regress them.

1. **Iter-1 review patch (Story 9-5 + transitively 9-6): duplicate-rejection tests assert pre/post-byte-equality of `config.toml`** [Blind B12 + Edge E9/E10 in Story 9-5]. Every Story 9-6 duplicate-id / duplicate-name 422 test MUST `let pre = std::fs::read(&config_path); … let post = std::fs::read(&config_path); assert_eq!(pre, post);` — silent rollback failure would otherwise pass the assertion.
2. **Iter-1 review patch (Story 9-5 + transitively 9-6): unique-per-test sentinel for positive-path log assertions** [Auditor A4 + Story 9-4 iter-2 P26 precedent]. Every Story 9-6 positive-path test that asserts `event="command_*"` log emission MUST include a `uuid::Uuid::new_v4().simple()` sentinel in a field (e.g., `command_name = format!("test-cmd-{}", sentinel)`) AND assert that the sentinel appears in the captured logs — defeating parallel-test buffer-bleed.
3. **Iter-1 review patch (Story 9-5 + transitively 9-6): per-test fixture `_listener_handle` stored on fixture struct + `.await` (or `abort + .await`) on `shutdown()`** [Blind B24 + Story 9-5 iter-2 L4]. Story 9-6's `CommandCrudFixture` MUST follow the same pattern; `JoinError::Panic` MUST re-propagate.
4. **Iter-1 review patch (Story 9-5 + transitively 9-6): tempdir guard via hand-rolled scopeguard-style RAII for chmod-based fault-injection tests** [Blind B18]. The `command_crud_io_failure_does_not_log_secrets` test (AC#12) MUST wrap the `chmod 0o000` step in a hand-rolled RAII guard (Drop impl on a small struct that restores perms in `drop()`) — see Story 9-5's `tests/web_device_crud.rs:1578` precedent (the comment reads "use scopeguard-style RAII" but the `scopeguard` crate is NOT a dependency; the pattern is implemented inline).
5. **Iter-2 review patch (Story 9-5 + transitively 9-6): editor modal `loading` flag wrapped in `try/finally`** [M4]. Story 9-6's `static/commands.js` edit-modal MUST follow the same shape: a synchronous DOM-null deref above the inner try block must not leave the modal permanently inert.
6. **Iter-2 review patch (Story 9-5 + transitively 9-6): `fetchJson` treats `Content-Length: 0` as no-body** [L2]. Story 9-6's `static/commands.js` MUST replicate the helper.
7. **Iter-2 review patch (Story 9-5 + transitively 9-6): DELETE-without-Content-Type assertion includes audit emission check** [L3]. Story 9-6's `delete_command_without_content_type_returns_415` test MUST assert `event="command_crud_rejected" reason="csrf"` (the existing 9-5 audit-event behaviour now also includes the Story 9-6 `"command"` arm).
8. **Iter-3 review patch (Story 9-5 + transitively 9-6): unit tests with deterministic fixtures (no conditional gating)** [Auditor A7]. Story 9-6's new `AppConfig::validate` unit tests (`test_validation_duplicate_command_id_within_device`, `test_validation_duplicate_command_name_within_device`) MUST use deterministic fixtures — no `if !device_list.is_empty()` gating.
9. **Iter-3 review patch (Story 9-5 + transitively 9-6): grep-anchor instructions instead of numeric line refs in dev notes** [Auditor F4]. Story 9-6's Dev Agent Record / Completion Notes MUST reference helpers via `grep -n "fn <name>" src/web/api.rs` rather than line numbers (which drift across iter-loop patches).

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issue (CLAUDE.md compliance)

- [ ] Open issue `Story 9-6: Command CRUD via Web UI` referencing FR36, FR40, FR41, AC#1-13 of this spec. Include a one-line FR-traceability table. Cross-link to Stories 9-4 / 9-5 issues for CRUD-scaffold inheritance + Story 9-7 issue for hot-reload integration. Capture the issue number in the Dev Agent Record before any code change. **`gh CLI` may not be authenticated for write in this session per Stories 9-4 / 9-5 precedent — if not, defer issue creation to the user and proceed with implementation while documenting the pending issue # placeholder in the Dev Agent Record.**

### Task 1: Validate-side amendments to `AppConfig::validate` (`src/config.rs`) (AC#3)

- [ ] **Add per-device `command_id` uniqueness check.** Modelled on the existing `seen_metric_names` HashSet pattern at `src/config.rs:634-664` (Story 9-5 amendment). Inside the existing device-walk loop, after the existing per-device validations, add (locate the device-walk loop via `grep -n "for (d_idx, device) in" src/config.rs`):
  ```rust
  if let Some(command_list) = &device.device_command_list {
      let mut seen_command_ids: std::collections::HashSet<i32> = std::collections::HashSet::new();
      let mut seen_command_names: std::collections::HashSet<String> = std::collections::HashSet::new();
      for (c_idx, command) in command_list.iter().enumerate() {
          let command_context = format!("{}.command[{}]", dev_context, c_idx);
          if seen_command_ids.contains(&command.command_id) {
              errors.push(format!(
                  "{}.command_id: {} is duplicated within device.device_command_list",
                  command_context, command.command_id
              ));
          } else {
              seen_command_ids.insert(command.command_id);
          }
          if seen_command_names.contains(&command.command_name) {
              errors.push(format!(
                  "{}.command_name: '{}' is duplicated within device.device_command_list",
                  command_context, command.command_name
              ));
          } else {
              seen_command_names.insert(command.command_name.clone());
          }
      }
  }
  ```
- [ ] Add 2 new unit tests in the existing `#[cfg(test)] mod tests` block:
  - `test_validation_duplicate_command_id_within_device` — fixture with one device whose `device_command_list` has two entries with the same `command_id`; assert `validate()` returns `Err(_)` carrying the duplicate-detection message. **Use deterministic fixture per Story 9-5 iter-3 A7** (no `if !device_list.is_empty()` gating).
  - `test_validation_duplicate_command_name_within_device` — same shape for `command_name`.
  - `test_validation_same_command_id_across_devices_is_allowed` — symmetric to Story 9-5's `test_validation_same_metric_name_across_devices_is_allowed`; pin that cross-device same-`command_id` is **not** rejected (the per-device-NodeId-namespace argument from Existing Infrastructure table).
- [ ] **Do NOT** add cross-device `command_id` uniqueness — two devices CAN share a `command_id` (the device folder NodeId namespaces the command). Cross-device uniqueness would re-introduce a false-positive rejection class.

### Task 2: CSRF literal-arm completion (`src/web/csrf.rs`) (AC#5, AC#8)

- [ ] Add the `"command" => warn!(event = "command_crud_rejected", ...)` arm to BOTH rejection-emission `match` blocks in `csrf_middleware` (locate via `grep -n "match resource" src/web/csrf.rs`):
  - Origin/Referer rejection (currently `src/web/csrf.rs:277-306`).
  - Content-Type rejection (currently `src/web/csrf.rs:318-344`).
- [ ] **Preserve the existing field set** (path, method, source_ip, reason, origin/—) byte-for-byte to avoid Stories 9-4 / 9-5 grep-contract regressions.
- [ ] **Do NOT** change `csrf_event_resource_for_path` — it already routes `/api/applications/<app>/devices/<dev>/commands*` → `"command"` correctly (verified: `src/web/csrf.rs:209-214`).
- [ ] **Do NOT** delete the catch-all `_ =>` arm — it remains as a defensive future-proofing guard for any un-routed resource (currently unreachable but Story 9-5 iter-1 D2 carry-forward).
- [ ] Add a unit test `csrf_rejects_post_command_emits_command_event` covering: send a POST to `/api/applications/foo/devices/bar/commands` through the CSRF middleware with no Origin header; capture the warn log; assert `event="command_crud_rejected"` AND `reason="csrf"`.
- [ ] Add a unit test `csrf_rejects_post_command_form_urlencoded_emits_command_event` covering: send a POST with valid Origin but `Content-Type: application/x-www-form-urlencoded`; capture the warn log; assert `event="command_crud_rejected"` AND `reason="csrf"`.
- [ ] Verify Stories 9-4 / 9-5's existing CSRF unit tests still pass byte-for-byte (the rejection-emission paths now route through the new arm only for `"command"`-resourced requests; tests assert on status codes + body shape).
- [ ] **Update `csrf_event_resource_for_path_maps_correctly` if needed** — verify it still passes; no change anticipated (the helper itself is unchanged).

### Task 3: `validate_path_device_id` widening (`src/web/api.rs`) (AC#3, AC#5, AC#8, AC#11)

- [ ] Widen `validate_path_device_id` to accept `resource: &'static str` (parallel to `validate_path_application_id` at `src/web/api.rs:500-589`; locate the existing helper via `grep -n "fn validate_path_device_id" src/web/api.rs`). Dispatch event-name literal per arm:
  ```rust
  fn validate_path_device_id(device_id: &str, addr: &SocketAddr, resource: &'static str) -> Result<(), Response> {
      // ... existing length + char-class checks ...
      // Replace the hard-coded `warn!(event = "device_crud_rejected", ...)`
      // with a match-arm dispatch:
      match resource {
          "device" => warn!(event = "device_crud_rejected", reason = "validation", ...),
          "command" => warn!(event = "command_crud_rejected", reason = "validation", ...),
          _ => warn!(event = "crud_rejected", reason = "validation", resource = resource, ...),
      }
      // ... return Err(...) ...
  }
  ```
- [ ] **Update ALL call sites of `validate_path_device_id` in `src/web/api.rs`** to pass the appropriate `resource` literal:
  - Device handlers (`get_device`, `create_device`, `update_device`, `delete_device`, `list_devices`): pass `"device"`.
  - **New command handlers** (Task 4): pass `"command"`.
- [ ] Add private `fn validate_path_command_id(command_id_str: &str, addr: &SocketAddr) -> Result<i32, Response>` (NEW helper). The helper:
  1. Parses `command_id_str` as `i32` via `str::parse::<i32>()`. On `Err(_)`: emit `event="command_crud_rejected" reason="validation" field="command_id"` with `bad_str = %command_id_str`; return 400.
  2. Rejects `command_id <= 0` (positive-integer contract — `command_id = 0` is reserved-as-unset). Emit same audit + return 400.
  3. Returns `Ok(parsed_i32)` on success.
- [ ] Add private `fn validate_command_field(field: &str, value: &CommandFieldValue, addr: &SocketAddr) -> Result<(), Response>` (NEW helper) covering the 4 command-body fields:
  - `command_name`: char-class via `is_valid_app_name_char` (the Story 9-5 device-name class — ASCII alphanumerics + `'-'_.'` + spaces + parentheses), length `[1, 256]`, trim-rejects-empty.
  - `command_port`: `i32` parsed by serde; reject `<= 0` OR `> 255` at handler-level pre-check; convert to `u8` and call `DeviceCommand::validate_f_port(port_u8)` (`src/storage/types.rs:155`); reject 400 if false. Hint: `"must be 1..=223 (LoRaWAN application f_port range)"`.
  - `command_confirmed`: bool — handler-level validation is satisfied by serde's deserialise success (no further checks beyond the type-system constraint).
  - `command_id`: i32 — only used in POST body validation (PUT path-id is authoritative; PUT body MUST NOT carry `command_id` per AC#3 immutable-field rule). Same range check as `command_port` (positive).
- [ ] Add private `fn command_not_found_response() -> Response` (NEW helper) parallel to `device_not_found_response` (locate via `grep -n "fn device_not_found_response" src/web/api.rs`); returns 404 + `{"error": "command not found", "hint": null}` (the audit-event emission is the caller's responsibility — Stories 9-4 / 9-5 audit-event semantic).
- [ ] Add a unit test `validate_path_command_id_rejects_non_numeric_path` (in `src/web/api.rs::tests`); call the helper with `"not-a-number"`; assert `Err(_)` returned + captured log carries `event="command_crud_rejected" reason="validation" field="command_id"`.
- [ ] Add a unit test `validate_path_command_id_rejects_negative` and `validate_path_command_id_rejects_zero`.
- [ ] Add unit tests for the AC#11 `validate_path_device_id` widening invariant:
  - `validate_path_device_id_under_command_resource_emits_command_event` (NEW): call the widened helper with a CRLF-injected `device_id` AND `resource = "command"`; assert the captured log carries `event="command_crud_rejected"`.
  - `validate_path_device_id_under_device_resource_still_emits_device_event` (NEW): same helper with `resource = "device"`; assert `event="device_crud_rejected"`. **Pins the Story 9-5 invariant under the Story 9-6 widening.**

### Task 4: CRUD handlers in `src/web/api.rs` (AC#1, AC#2, AC#3, AC#4, AC#6, AC#7)

- [ ] **Extend `axum` imports** in `src/web/api.rs` if needed — `axum::extract::Path` already imported; the multi-segment `Path<(String, String, String)>` extractor for `(:application_id, :device_id, :command_id)` (treat `command_id` as String at routing layer, parse to i32 in the handler via `validate_path_command_id` Task 3) is the same import.
- [ ] Add the following handlers to `src/web/api.rs`:
  - `pub async fn list_commands(State(state): State<Arc<AppState>>, ConnectInfo(addr): ConnectInfo<SocketAddr>, Path((application_id, device_id)): Path<(String, String)>) -> Result<Json<CommandListResponse>, Response>` — read path: validate `application_id` (`validate_path_application_id(..., "command")`) + `device_id` (`validate_path_device_id(..., "command")`); load live `Arc<AppConfig>` via `state.config_reload.subscribe().borrow().clone()` (Story 9-5 access pattern); find application then device (404 with `application_not_found_response` or `device_not_found_response`); project the device's `device_command_list.unwrap_or_default()` into the response shape.
  - `pub async fn get_command(State, ConnectInfo, Path((application_id, device_id, command_id_str)): Path<(String, String, String)>) -> Result<Json<CommandResponse>, Response>` — same validation chain + `validate_path_command_id(command_id_str, addr) -> i32`; find command by exact `command_id == parsed` match (404 with `command_not_found_response`).
  - `pub async fn create_command(State, ConnectInfo, Path((application_id, device_id)), Json(body): Json<CreateCommandRequest>) -> Result<(StatusCode, [(HeaderName, String); 1], Json<CommandResponse>), Response>` — write path: `validate_path_application_id(..., "command")` + `validate_path_device_id(..., "command")`; acquire `state.config_writer.lock().await` FIRST, then `read_raw → parse_document_from_bytes` (Stories 9-4 / 9-5 lock-and-rollback discipline), then handler-level validation (`validate_command_field` for each body field), then walk the `[[application]]` array via `find_application_index(..., "command")` to find the matching `application_id` (return `application_not_found` if absent), then walk the matching application's `device` array of tables for the device pre-flight (Story 9-5 iter-3 P41 pattern); locate the device's `command` array (creating it if `None`); pre-flight reject 409 if any existing block has malformed `command_id`/`command_name`/`command_port`/`command_confirmed`; reject 409 if `command_id` already present under this device (duplicate within device); append a new `[[application.device.command]]` table via `build_command_table` (Task 6); `write_atomically`; `config_reload.reload().await`; on reload error → `rollback`. On post-`write_atomically` error (Story 9-5 iter-3 EH3-H1 pattern) → ALSO call `rollback` BEFORE returning 500. On success: emit `event="command_created"` info + return 201 + Location header (`/api/applications/:app/devices/:dev/commands/<command_id>`) + body.
  - `pub async fn update_command(State, ConnectInfo, Path((application_id, device_id, command_id_str)), body)` — write path: same lock-acquire-first shape; path-id validation + body field validation; manual deserialise to `serde_json::Value` then walk-and-reject on `command_id` (`immutable_field`) per Stories 9-4 iter-2 P29 + 9-5 patterns; pre-flight (Story 9-5 iter-3 P41) reject 409 on malformed sibling command blocks; locate the matching command (404 if absent); replace `command_name` + `command_port` + `command_confirmed` IN-PLACE via `toml_edit` table mutation (Task 6); write + reload + (rollback on error). Emit `event="command_updated"` info on success.
  - `pub async fn delete_command(State, ConnectInfo, Path((application_id, device_id, command_id_str)))` — write path: lock-acquire-first; path validation; pre-flight per iter-3 P41; locate the matching command (404 if absent); remove the `[[application.device.command]]` table from the parent device's command array (Task 6); decide: if removing the last command leaves an empty `Vec<DeviceCommandCfg>` that serialises as a no-op (silently drops the empty `command` key), or whether to actively remove the now-empty `command` array key from the device table — pick the former for minimal-diff TOML (verify `toml_edit::ArrayOfTables::remove` behaviour at impl time); write + reload + (rollback on error). Emit `event="command_deleted"` info on success.
- [ ] Add the request/response types alongside the existing 9-4 / 9-5 types:
  - `#[derive(Deserialize)] #[serde(deny_unknown_fields)] pub struct CreateCommandRequest { command_id: i32, command_name: String, command_port: i32, command_confirmed: bool }` — `serde(deny_unknown_fields)` so unknown body fields are rejected by serde with 422 (matching Story 9-4 / 9-5 cosmetic divergence).
  - `pub struct UpdateCommandRequest { command_name: String, command_port: i32, command_confirmed: bool }` — **NO `serde(deny_unknown_fields)`** because Story 9-6 handles `command_id` immutable-field rejection manually (Stories 9-4 iter-2 P29 + 9-5 patterns): deserialise to `serde_json::Value`, walk-and-reject on `command_id` field.
  - `#[derive(Serialize)] pub struct CommandListResponse { application_id: String, device_id: String, commands: Vec<CommandResponse> }`
  - `#[derive(Serialize)] pub struct CommandResponse { command_id: i32, command_name: String, command_port: i32, command_confirmed: bool }` — symmetric to Story 9-5's `MetricMappingResponse`; do NOT add `Serialize` to `DeviceCommandCfg`.
- [ ] **DO NOT** introduce a new `OpcGwError` variant. Map: handler-level shape errors → 400 + ErrorResponse; validation errors from reload → 422 + ErrorResponse; conflict errors (malformed sibling blocks, duplicate command_id within device) → 409 + ErrorResponse; CSRF errors → 403/415 (handled by middleware Task 2); reload IO/restart-required errors → 500 / 409 ambient-drift; not-found → 404 + ErrorResponse with `application_not_found_response` / `device_not_found_response` / `command_not_found_response`.
- [ ] **Pass `resource = "command"` to ALL `resource`-threaded helpers** invoked from the new handlers:
  - `validate_path_application_id(application_id, addr, "command")`
  - `validate_path_device_id(device_id, addr, "command")` (post-Task 3 widening)
  - `find_application_index(doc, application_id, addr, "command")`
  - `handle_rollback(state, original_bytes, site, addr, cause, "command")`
  - `io_error_response(e, site, addr, "command")`
  - `reload_error_response(e, site, addr, "command")` — `grep -n "fn reload_error_response" src/web/api.rs` to confirm signature.
  - `handle_restart_required(e, site, addr, original_bytes, state, "command")` — same `grep` to confirm.

### Task 5: Audit-event emission for not-found paths (`src/web/api.rs`) (AC#6, AC#8)

- [ ] In each Story 9-6 handler that returns `application_not_found_response` / `device_not_found_response` / `command_not_found_response`: emit the warn log at the call site BEFORE returning the helper's response (parallel to Story 9-5 Task 5 pattern):
  ```rust
  warn!(
      target: "audit",
      event = "command_crud_rejected",
      reason = "application_not_found",
      application_id = %application_id,
      device_id = %device_id,
      source_ip = %addr.ip(),
      "command CRUD rejected: parent application not found"
  );
  return Err(application_not_found_response());
  ```
  - Same pattern for `device_not_found` (PUT/POST/DELETE under known application but unknown device).
  - Same pattern for `command_not_found` (PUT/DELETE under known device but unknown command_id).
- [ ] **GET 404s do NOT emit `_crud_rejected` warn logs** — preserve the Story 9-4 / 9-5 audit-event semantic that `_crud_rejected` is reserved for state-changing rejections.
- [ ] **Exception:** path-validation failures (non-numeric `:command_id` triggering `validate_path_command_id`) DO emit `_crud_rejected` regardless of HTTP method (Story 9-5 Decision-2 — path-shape rejection IS a CRUD rejection regardless of method).

### Task 6: TOML mutation that preserves `[[application.device.read_metric]]` sub-tables (Task 4 sub-bullet, AC#4)

- [ ] **Load-bearing (symmetric to Story 9-5 Task 6):** when the dev agent writes the PUT/DELETE handlers, the TOML mutation MUST be done at the table level via `toml_edit::DocumentMut::get_mut` + `as_array_of_tables_mut` rather than serialising the command back via `toml::Value`. The latter would serialise `DeviceCommandCfg` as a stand-alone block but the round-trip would silently strip the device's `read_metric` sub-tables if any nested-table semantics are mishandled.
- [ ] **POST mutation shape:**
  1. Locate the `[[application]]` table by `application_id`.
  2. Locate the device by `device_id` within the application's `device` array of tables.
  3. **In-place** mutate the device table: get-or-create the `command` array of tables (`tbl.entry("command").or_insert(toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new())).as_array_of_tables_mut()`).
  4. Append a new command table built via `build_command_table(command_id, command_name, command_port, command_confirmed)`.
  5. **DO NOT touch** the device's `read_metric` array or any other sub-tables / unknown fields under the device — preserve byte-for-byte (regression guard for Story 9-5).
- [ ] **PUT mutation shape:**
  1. Locate the `[[application]]` + device tables as above.
  2. Iterate to find the command table with the matching `command_id`.
  3. **In-place** mutate the command table: `cmd_tbl.insert("command_name", new_name)`, `cmd_tbl.insert("command_port", new_port)`, `cmd_tbl.insert("command_confirmed", new_confirmed)`. **DO NOT touch** `command_id` (immutable).
  4. **DO NOT touch** the sibling `read_metric` array — preserve byte-for-byte.
- [ ] **DELETE mutation shape:**
  1. Locate the `[[application]]` + device + command tables as above.
  2. Iterate to find the command table's index in the parent device's `command` array.
  3. Call `array_of_tables.remove(idx)` — `toml_edit` correctly removes the table.
  4. **DO NOT touch** the sibling `read_metric` array — preserve byte-for-byte.
  5. **Decision (pinned):** if removing the last command leaves an empty `command` `ArrayOfTables`, **leave it in place** — do NOT actively remove the `command` key from the device table. Rationale: `toml_edit` round-trips an empty `ArrayOfTables` cleanly (serialises as nothing on the wire — verified at impl time) and a subsequent POST re-populates without needing to re-create the array key. This keeps the DELETE path's TOML mutation **minimal-diff** and **symmetric to the POST path** (which uses `or_insert(ArrayOfTables::new())` to handle the `None → Some(empty)` transition).
  6. **Pinning test (NEW, AC#4):** `tests/web_command_crud.rs::delete_last_command_leaves_clean_toml_round_trip` — seed a device with exactly 1 command; DELETE that command; assert (a) status 204; (b) the post-delete TOML file parses cleanly via `figment::Toml::file(...)` + `AppConfig::deserialize`; (c) the resulting `device.device_command_list` deserialises to `Some(vec![])` OR `None` (accept either — `toml_edit`'s exact serialisation behaviour for empty ArrayOfTables determines which); (d) a subsequent POST of a fresh command on the same device succeeds (201). This test pins the contract regardless of the exact serialisation choice `toml_edit` makes.
- [ ] Add helper `fn build_command_table(command_id: i32, command_name: &str, command_port: i32, command_confirmed: bool) -> toml_edit::Table` (NEW, parallel to `build_device_table` at `src/web/api.rs:2574`). Inserts the 4 fields in declaration order: `command_id`, `command_name`, `command_confirmed`, `command_port` (matches the source-comment order at `src/config.rs:660-670`).
- [ ] Add a unit test `mutate_command_preserves_read_metric_subtable` in `src/web/api.rs::tests` (or in a new helper module if PUT-mutation is extracted to a function): seed a `DocumentMut` with a device carrying both a `read_metric` array AND a `command` array; PUT-mutate one command; serialise the doc back to a string; assert the `read_metric` sub-array is byte-equal to the original.

### Task 7: Router wiring (`src/web/mod.rs`) (AC#1, AC#2)

- [ ] In `src/web/mod.rs::build_router`:
  - Add 2 new `.route(...)` calls for the command CRUD endpoints. Use axum 0.8's nested-path syntax: `"/api/applications/{application_id}/devices/{device_id}/commands"` (GET + POST) and `"/api/applications/{application_id}/devices/{device_id}/commands/{command_id}"` (GET + PUT + DELETE). axum 0.8's `Path` extractor handles the multi-segment extraction via `Path<(String, String, String)>` per Task 4 handler signatures.
  - The CSRF middleware from Story 9-4 + literal-arm completion (Task 2) will fire for the new POST/PUT/DELETE routes automatically (its match is on HTTP method, not URL path — the audit-event name dispatches by path per Task 2).
  - The Basic auth middleware is already wired and inherits via the layer-after-route invariant.
- [ ] No `build_router` signature change.

### Task 8: Static assets (`static/commands.html` + `static/commands.js`) (AC#1, AC#9)

- [ ] **Replace** `static/commands.html` (currently a placeholder: `<p>Story 9-6 will fill this in (command CRUD).</p>`). Vanilla HTML, mobile-responsive, reuses `static/dashboard.css` + inline `<style>` block for command-specific overrides (per the 9-5 pattern). Layout: per-application accordion → per-device sub-section → commands table per device + create-command form anchored under each device + edit modal driven by `<dialog>`. ≤ 250 lines.
- [ ] Create `static/commands.js` (NEW, replacing the implicit empty script reference). Vanilla JS (no framework). On `DOMContentLoaded`: fetch `/api/applications` for the application list, then per-application fetch `/api/applications/:id/devices` for the device list, then per-device fetch `/api/applications/:app/devices/:dev/commands` for the commands list. Bind create/edit/delete handlers. Re-fetch the commands list on every successful mutation. ≤ 350 lines.
- [ ] **Edit modal MUST follow Story 9-5 iter-2 M4 pattern**: wrap loading-flag set/reset in `try/finally` so a synchronous DOM-null deref above the inner try block doesn't leave the modal permanently inert.
- [ ] **fetchJson helper MUST treat `Content-Length: 0` as no-body** (Story 9-5 iter-2 L2 pattern).
- [ ] **HTML escape MUST cover backtick** (Story 9-5 iter-1 P25 carry-forward).
- [ ] **Do NOT** introduce any new framework, build step, or `npm install`.
- [ ] Update header nav links in **all 5 static pages that currently render a `<nav>` element** (the current nav state is inconsistent across pages — 3 distinct variants verified: `Dashboard | Applications | Live Metrics`, `Dashboard | Applications | Devices configuration | Live Metrics`, `Dashboard | Devices | Live Metrics`). Story 9-6 adds a `Commands` link to each, harmonising at the same time:
  - `static/index.html` — add a `Commands` link if a `<nav>` exists.
  - `static/applications.html` — add a `Commands` link (one-line edit; AC#10 does not forbid `static/*.html` modifications).
  - `static/devices-config.html` — add a `Commands` link.
  - `static/devices.html` — add a `Commands` link.
  - `static/metrics.html` — add a `Commands` link.
- [ ] **`static/commands.html` itself** carries the full nav (`Dashboard | Applications | Devices configuration | Live Metrics | Commands` — current-page item bolded or styled distinct per the convention you find in `devices-config.html`).
- [ ] Full nav-harmonisation across the entire static surface (making every page's `<nav>` identical) is **NOT** in Story 9-6's scope; only the Commands-link addition is. Spec-level note: Story 9-6 surfaces but does not fully resolve the pre-existing nav drift.

### Task 9: Integration tests (`tests/web_command_crud.rs`) (AC#1-AC#12)

- [ ] Create `tests/web_command_crud.rs` with the test list below. Use the `tests/common/mod.rs` helpers from Story 9-4 + `tests/web_device_crud.rs` fixture patterns from Story 9-5. Each test owns a `tempfile::TempDir` containing a fresh `config.toml` (with at least one application + one device + (depending on test) some seeded commands).

Required test cases (≥ 25):

1. `commands_html_renders_per_device_table` (AC#1)
2. `commands_js_fetches_api_commands_per_device` (AC#1)
3. `commands_html_carries_viewport_meta` (AC#9)
4. `commands_uses_dashboard_css_baseline` (AC#9)
5. `get_commands_returns_seeded_list_under_device` (AC#2)
6. `get_commands_returns_404_for_unknown_application` (AC#2 + AC#6)
7. `get_commands_returns_404_for_unknown_device` (AC#2 + AC#6)
8. `get_command_by_id_returns_404_for_unknown_command` (AC#2 + AC#6)
9. `get_command_with_non_numeric_path_returns_400` (AC#2 path validation)
10. `post_command_creates_then_get_returns_201` (AC#2)
11. `post_command_on_device_with_none_command_list_creates_subtable` (AC#2)
12. `put_command_updates_fields_then_get_reflects` (AC#2)
13. `delete_command_returns_204_then_404` (AC#2)
14. `post_command_with_empty_name_returns_400` (AC#3)
15. `post_command_with_port_below_range_returns_400` (AC#3 — port = 0)
16. `post_command_with_port_above_range_returns_400` (AC#3 — port = 224)
17. `post_command_with_negative_id_returns_400` (AC#3)
18. `post_command_with_zero_id_returns_400` (AC#3)
19. `post_command_with_duplicate_command_id_within_device_returns_422` (AC#3 — load-bearing: includes pre/post TOML byte-equality assertion per Story 9-5 iter-1 patch)
20. `post_command_with_duplicate_command_name_within_device_returns_422` (AC#3 — same)
21. `post_command_with_same_command_id_on_different_device_succeeds` (AC#3 — cross-device-allowed contract)
22. `put_command_id_in_body_is_rejected` (AC#3 — accepts 400 OR 422 per 9-4/9-5 cosmetic divergence)
23. `post_command_preserves_comments` (AC#4)
24. `put_command_preserves_read_metric_subtable` (AC#4 — load-bearing, prevents Story 9-5 regression)
25. `post_command_preserves_other_devices_commands` (AC#4)
26. `delete_command_preserves_other_commands_under_device` (AC#4)
27. `post_command_without_origin_returns_403_with_command_event` (AC#5)
28. `post_command_with_cross_origin_returns_403_with_command_event` (AC#5)
29. `post_application_csrf_event_unchanged_under_9_6_changes` (AC#5 + AC#11 — Story 9-4 regression)
30. `post_device_csrf_event_unchanged_under_9_6_changes` (AC#5 + AC#11 — Story 9-5 regression)
31. `post_command_with_form_urlencoded_returns_415` (AC#5)
32. `delete_command_without_content_type_returns_415` (AC#5 — pin audit emission per Story 9-5 iter-2 L3)
33. `delete_command_under_unknown_application_returns_404` (AC#6)
34. `delete_command_under_unknown_device_returns_404` (AC#6)
35. `delete_unknown_command_under_known_device_returns_404` (AC#6)
36. `delete_last_command_under_device_succeeds` (AC#6)
37. `post_command_triggers_reload_and_subsequent_get_reflects` (AC#7)
38. `post_command_emits_command_created_event` (AC#7 + AC#8 — uses unique-per-test sentinel per 9-4 iter-2 P26)
39. `post_command_emits_topology_change_log` (AC#7)
40. `command_crud_does_not_log_secrets_success_path` (AC#12)
41. `command_crud_io_failure_does_not_log_secrets` (AC#12 — pin status=500 per 9-5 iter-1 E13; wrap chmod in hand-rolled RAII Drop-impl guard per 9-5 iter-1 L12 — `scopeguard` crate is NOT a dep)
42. `auth_required_for_post_commands` (AC#10 — POST without `Authorization` header returns 401 + `event="web_auth_failed"` log; also covers GET/PUT/DELETE)

- [ ] Use `tracing-test::traced_test` + `tracing_test::internal::global_buf()` for log assertions (Stories 9-4 / 9-5 pattern).
- [ ] Use `reqwest` for HTTP requests.
- [ ] Per Story 9-5 iter-2 L4 / iter-1 B24: fixture struct stores `JoinHandle` + `shutdown()` re-propagates `JoinError::Panic`.
- [ ] Per Story 9-5 iter-1 L12 / B18: chmod-based fault-injection tests wrap perm changes in a hand-rolled RAII guard (Drop-impl struct that restores perms) — NOT the `scopeguard` crate, which is not a dependency; precedent at `tests/web_device_crud.rs:1578`.

### Task 10: Documentation sync (AC#12 backfill, AC#13)

- [ ] `docs/logging.md`: add 4 rows to the operations table (after the 9-5 `device_*` block):
  - `command_created` — info — fields: application_id, device_id, command_id, source_ip — operator-action: none.
  - `command_updated` — info — same fields — operator-action: none.
  - `command_deleted` — info — same fields — operator-action: none.
  - `command_crud_rejected` — warn — fields: application_id, device_id, command_id (when applicable), source_ip, reason, error — operator-action: per `reason`. Add a one-line note that the path-aware CSRF dispatch (Stories 9-5 + 9-6) now produces three resource-specific rejection event names plus a defensive catch-all.
- [ ] `docs/security.md` § Configuration mutations: add a new "Command CRUD (Story 9-6)" subsection covering (a) the 5 endpoint surface, (b) the path-aware CSRF audit dispatch (now complete with the `"command"` arm), (c) the per-device `command_id` + `command_name` uniqueness contract (AC#3), (d) the v1 limitations specific to 9-6: no payload-template editing, no `[command_validation.device_schemas]` CRUD, no `command_id` rename, no cascade-delete of `command_queue` rows, OPC UA address-space mutation deferred to 9-8.
- [ ] `docs/security.md` § Anti-patterns: extend with a paragraph on `command_id` uniqueness within a device (collision class same as #99; Story 9-6 validate enforcement).
- [ ] `README.md`: bump Current Version date; flip Epic 9 row 9-6 status to `done` after final implementation. **Update the Web UI subsection** to mention the command-CRUD page.
- [ ] `_bmad-output/implementation-artifacts/sprint-status.yaml`: update `last_updated` narrative + flip 9-6 status (this happens at the end of the dev-story workflow).
- [ ] `_bmad-output/implementation-artifacts/deferred-work.md`: gains entries for any patches the dev agent identifies but defers (e.g., payload-template editing as future enhancement; `[command_validation.device_schemas]` CRUD as future enhancement; cascade-delete of `command_queue` rows on command DELETE).

### Task 11: Final verification (AC#13)

- [ ] `cargo test --lib --bins --tests` reports ≥ 1056 passed / 0 failed (per the AC#13 breakdown).
- [ ] `cargo clippy --all-targets -- -D warnings` clean.
- [ ] `cargo test --doc` 0 failed (56 ignored baseline unchanged).
- [ ] `git grep -hoE 'event = "command_[a-z_]+"' src/ | sort -u` returns exactly 4 lines.
- [ ] `git grep -hoE 'event = "device_[a-z_]+"' src/ | sort -u` continues to return exactly 4 lines (Story 9-5 invariant — AC#11).
- [ ] `git grep -hoE 'event = "application_[a-z_]+"' src/ | sort -u` continues to return exactly 4 lines (Story 9-4 invariant — AC#11).
- [ ] `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` continues to return exactly 3 lines (Story 9-7 invariant — AC#10).
- [ ] `git diff HEAD --stat src/web/auth.rs src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/opc_ua_history.rs src/security.rs src/security_hmac.rs src/opc_ua.rs` shows ZERO production-code changes (AC#10 strict-zero).
- [ ] **Existing Stories 9-4 + 9-5 integration tests pass byte-for-byte** (AC#11 regression). `cargo test --test web_application_crud` and `cargo test --test web_device_crud` both pass with zero failures.
- [ ] Manual smoke test: build + run gateway with `[web].enabled = true`; navigate to `http://127.0.0.1:8080/commands.html`; pick an application → pick a device → CREATE a command with `command_id = 1`, `command_name = "OpenValve"`, `command_port = 10`, `command_confirmed = true` → EDIT the `command_name` → DELETE the command via the UI; observe the four new audit-event log lines + verify `config/config.toml` contains the change after each step + the `[[application.device.read_metric]]` sub-table (if any) is preserved.

---

## Dev Notes

### Anti-patterns to avoid (per CLAUDE.md scope-discipline rule)

- **Do NOT** add a `payload_template` field to `DeviceCommandCfg`. Out of scope per epics.md gap (the BDD mentions it but the struct doesn't have it — schema change is a separate story).
- **Do NOT** add CRUD for `[command_validation.device_schemas]`. That's a separate config section keyed by `device_id` under `[command_validation]`, not `[[application.device.command]]`.
- **Do NOT** modify `src/opc_ua.rs`. Issue #99 is **already fixed**; Story 9-6 only enforces per-device `command_id` uniqueness via `AppConfig::validate` (Task 1). The command NodeId construction at `src/opc_ua.rs:1059` is per-device-scoped — per-device uniqueness is sufficient.
- **Do NOT** modify `src/web/auth.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing`. AC#10 file invariants from Stories 9-1 / 9-2 / 9-3 / 9-4 / 9-5 / 9-7 / 7-2 / 7-3 / 8-3.
- **Do NOT** introduce new dependencies. Story 9-4 / 9-5's `toml_edit` + the existing `tempfile` / `reqwest` / `tracing-test` dev-deps cover Story 9-6's needs. **Do NOT add `scopeguard`** — the chmod-cleanup pattern is hand-rolled inline RAII (Drop-impl struct) per Story 9-5's precedent at `tests/web_device_crud.rs:1578`.
- **Do NOT** serialise `DeviceCommandCfg` back to TOML via `toml::Value` (Task 6 anti-pattern — would silently strip the device's `read_metric` sub-table or any other sibling fields).
- **Do NOT** add `Serialize` to `DeviceCommandCfg`. Use a parallel `CommandResponse` struct in `src/web/api.rs` (Story 9-5's `MetricMappingResponse` pattern).
- **Do NOT** add cascade-delete of `command_queue` rows on command DELETE. v1 leaves orphaned rows.
- **Do NOT** introduce a new `OpcGwError` variant.
- **Do NOT** roll a new HTTP client in tests — `reqwest` is the established dev-dep.
- **Do NOT** delete the `_ =>` catch-all arm in the CSRF middleware. It remains as a defensive future-proofing guard.
- **Do NOT** roll a parallel f_port range check. Reuse `DeviceCommand::validate_f_port` at `src/storage/types.rs:155`.

### Why this Story 9-6 lands now

Story 9-5 done — the device-CRUD scaffold is complete, including the `validate_path_device_id` helper that 9-6 needs to widen, the `find_application_index` helper that 9-6 reuses, the Story 9-5 PUT-replace-device that already preserves `[[application.device.command]]` sub-tables. The recommended order at `epics.md:793` is `9-1 → 9-2 → 9-3 → 9-0 → 9-7 → 9-8 → 9-4 / 9-5 / 9-6`. With 9-4 / 9-5 / 9-7 done and #99 fixed, the dependency cluster for 9-6 is:

- **9-1 done** — Axum + Basic auth + `WebConfig`.
- **9-2 done** — `AppState` shape + `DashboardConfigSnapshot`.
- **9-3 done** — REST endpoint + JSON contract conventions + integration-test harness + `DeviceSummary`.
- **9-4 done** — CSRF middleware + ConfigWriter + audit-event taxonomy + `application_*` events + path-id validation pattern + lock-and-rollback discipline.
- **9-5 done** — Device + metric mapping CRUD + path-aware CSRF dispatch + resource-threading through helpers (`validate_path_application_id`, `find_application_index`, `handle_rollback`, etc.) + `device_*` events + the `[[application.device.command]]` sub-table preservation contract.
- **9-7 done** — `ConfigReloadHandle::reload()` + watch-channel + dashboard-snapshot atomic swap + `commands_equal` + `topology_device_diff` device-level classifier.
- **#99 fixed (commit `9f823cc`)** — Metric NodeId per-device-distinct. Command NodeIds were already per-device-scoped via the device folder NodeId namespacing.
- **9-8 backlog** — Story 9-6 does NOT depend on 9-8. The dashboard reflects new commands immediately; OPC UA address space stays at startup state until 9-8 lands. Same v1 limitation as 9-7 / 9-4 / 9-5.

Landing 9-6 now closes FR36 + closes the FR34/35/36 cluster (applications + devices + commands all CRUD-able via web UI).

### Interaction with Story 9-5 (Device + Metric Mapping CRUD — done)

- **`validate_path_device_id`** — Story 9-5 created the helper with hard-coded `event="device_crud_rejected"`. Story 9-6 widens it with `resource: &'static str` (Task 3). All Story 9-5 device-handler call sites updated to pass `"device"` — byte-for-byte audit behaviour preserved.
- **`[[application.device.command]]` sub-table preservation** — Story 9-5's PUT-replace-device test `put_device_preserves_command_subtable` ALREADY verifies that 9-5 doesn't touch the command sub-table. Story 9-6's symmetric test `put_command_preserves_read_metric_subtable` (AC#4 Task 9 #24) MUST pass — pins the 9-5 invariant under 9-6 changes.
- **`csrf_event_resource_for_path`** — Story 9-5 already routes `"command"` correctly (verified `src/web/csrf.rs:209-214`). Story 9-6 only needs to add the literal `"command" =>` arm at the rejection-emission sites — no helper change.
- **`find_application_index`** — Story 9-5 iter-3 Blind#3 patched the helper to take `resource: &'static str`. Story 9-6 just passes `"command"`.
- **`AppConfig::validate` device-walk loop** — Story 9-5 added `seen_metric_names` + `seen_chirpstack_metric_names` HashSets. Story 9-6 adds `seen_command_ids` + `seen_command_names` parallel HashSets (additive, no Story 9-5 edits).

### Interaction with Story 9-4 (Application CRUD — done)

- **CSRF middleware** — reused; Story 9-6 adds the literal `"command" =>` arm to the rejection match blocks (Task 2). The defence layer itself stays byte-for-byte unchanged.
- **ConfigWriter** — reused unchanged. Lock-and-hold-across-reload pattern inherited.
- **AppState** — reused unchanged.
- **Audit-event taxonomy** — Story 9-6's events parallel 9-4 / 9-5; the reason-set extends with 1 new value (`command_not_found`).
- **Helpers** — Story 9-4 / 9-5's `validate_application_field`, `validate_device_field`, `application_not_found_response`, `device_not_found_response`, `internal_error_response`, `io_error_response`, `reload_error_response`, `handle_rollback`, `handle_restart_required`, `validate_path_application_id` are all reused; Story 9-6 adds `validate_path_command_id`, `validate_command_field`, `command_not_found_response`.

### Interaction with Story 9-7 (Hot-Reload — done)

- Same as 9-4 / 9-5: `ConfigReloadHandle::reload()` is the load-bearing primitive.
- The reload's internal `tokio::sync::Mutex` serialises CRUD-vs-SIGHUP.
- The reload's `topology_device_diff` helper (iter-2 P26 device_command_list classifier fix) correctly classifies command-level mutations as `modified_devices += 1`.

### Interaction with Story 9-3 (Live Metric Values Display — done)

- `/devices.html` (Story 9-3 live-metrics) is separate from `/commands.html` (Story 9-6 CRUD). The two pages cross-link via header nav (Task 8).
- The dashboard snapshot auto-refreshes after every CRUD-triggered reload (Story 9-7 invariant) — but the dashboard doesn't surface commands, so Story 9-6 doesn't extend `DashboardConfigSnapshot`.

### Interaction with Story 9-8 (Dynamic Address-Space Mutation — backlog)

After a 9-6 CRUD edit + reload, the existing 9-7 `run_opcua_config_listener` emits `event="topology_change_detected"` with `modified_devices=N`. Story 9-8 will eventually consume this signal to mutate the OPC UA address space (adding/removing command NodeIds at runtime). **v1 limitation (carried from 9-7 / 9-4 / 9-5):** the dashboard updates immediately; the OPC UA address space stays at startup state until 9-8 lands. SCADA clients connected via OPC UA must reconnect to see new commands. Documented in `docs/security.md` § Configuration mutations § v1 limitations.

### Carry-forward GitHub Issues

Story 9-6 inherits the following carry-forward issues unchanged (none of them block 9-6):

- **#88** — per-IP rate limiting (Phase A carry-forward; Phase B structural relevance).
- **#100** — doctest cleanup (56 ignored baseline; Story 9-6 adds zero new doctests).
- **#102** — `tests/common/web.rs` extraction (Story 9-5 inherited the deferral; Story 9-6 also inherits — inline helpers in `tests/web_command_crud.rs`).
- **#104** — TLS / HTTPS hardening.
- **#108** — storage payload-less MetricType (orthogonal to commands).
- **#110** — RunHandles missing Drop.
- **#113** — live-borrow refactor (Story 9-6 does NOT extend — no new restart-required knob).

### Project Structure Notes

- **No new modules** — Story 9-6 extends `src/web/api.rs` + `src/web/csrf.rs` + `src/config.rs` + `src/web/mod.rs`.
- **Modified files (production code)**:
  - `src/web/api.rs` — 5 new CRUD handlers + 4 new request/response types + `validate_path_command_id` helper + `validate_command_field` helper + `command_not_found_response` helper + `build_command_table` helper + `validate_path_device_id` widening (parameter addition + match-arm dispatch).
  - `src/web/csrf.rs` — 2 new `"command" =>` literal arms in the rejection-emission match blocks + 2 new unit tests.
  - `src/config.rs` — `validate()` extended additively for per-device `command_id` + `command_name` uniqueness + 3 new unit tests (2 duplicate-rejection + 1 cross-device-allowed pin).
  - `src/web/mod.rs` — 2 new `.route(...)` calls in `build_router`.
- **Modified files (tests)**:
  - `tests/web_command_crud.rs` — NEW, ≥ 42 integration tests including the AC#11 cross-resource regression suite.
- **Modified files (static)**:
  - `static/commands.html` — replaces placeholder with full CRUD page.
  - `static/commands.js` — NEW.
  - `static/applications.html`, `static/devices-config.html`, `static/devices.html` — header nav link addition (one-line edit each).
- **Modified files (docs)**:
  - `docs/logging.md`, `docs/security.md`, `README.md`, `_bmad-output/implementation-artifacts/sprint-status.yaml`, `_bmad-output/implementation-artifacts/deferred-work.md`.
- **Untouched files (AC#10 invariant)**:
  - `src/web/auth.rs`, `src/opc_ua.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing` (function body).

### Testing Standards

- Per `_bmad-output/planning-artifacts/architecture.md`, integration tests live in `tests/`; unit tests inline with `#[cfg(test)] mod tests`.
- `tracing-test` + `tracing_test::internal::global_buf()` for log assertions (Story 9-4 iter-2 P26 unique-per-test sentinel pattern; Story 9-5 iter-2 L4 listener-handle re-propagation pattern).
- `serial_test::serial` discipline NOT required by default unless a flake surfaces (9-4 / 9-7 precedent); Story 9-5 iter-2 L3 added `#[serial(captured_logs)]` to specific tests where parallel log emission would bleed through — Story 9-6 inherits the pattern for similar tests.
- `tempfile::TempDir` + `NamedTempFile` for per-test config TOML files.
- `reqwest` for HTTP client.
- Hand-rolled RAII guard (Drop-impl struct that restores perms in `drop()`) for chmod-based fault-injection cleanup — see Story 9-5's `tests/web_device_crud.rs:1578` precedent comment "scopeguard-style RAII". **The `scopeguard` crate itself is NOT a dependency** (verified: `grep -n scopeguard Cargo.toml` returns nothing); do not `cargo add` it.
- **For AC#11 path-aware-CSRF cross-resource regression tests:** run the existing `tests/web_application_crud.rs` and `tests/web_device_crud.rs` test binaries as part of `cargo test`; they MUST pass byte-for-byte.

### Doctest cleanup

- 9-6 adds **zero new doctests** — the 56 ignored doctests baseline (issue #100) stays unchanged.

### File List (expected post-implementation)

**Modified files (production):**
- `src/web/api.rs` (modified) — 5 new handlers + 4 new types + 4 new helpers + `validate_path_device_id` widening.
- `src/web/csrf.rs` (modified) — 2 new `"command" =>` literal arms + 2 new unit tests.
- `src/config.rs` (modified) — `validate()` additive `command_id` + `command_name` uniqueness rules + 3 new unit tests.
- `src/web/mod.rs` (modified) — 2 new routes in `build_router`.

**New files (tests):**
- `tests/web_command_crud.rs` (NEW) — ≥ 42 integration tests including 2 AC#11 cross-resource regression tests.

**Replaced files (static):**
- `static/commands.html` (REPLACED placeholder) — CRUD page with nested application+device selector + edit modal + create form.

**New files (static):**
- `static/commands.js` (NEW) — vanilla JS controller.

**Modified files (static):**
- `static/applications.html` — header nav link to `/commands.html`.
- `static/devices-config.html` — header nav link to `/commands.html`.
- `static/devices.html` — header nav link to `/commands.html`.

**Modified files (docs):**
- `docs/logging.md` — added 4 rows for `command_*` events + path-aware CSRF dispatch note.
- `docs/security.md` — extended `## Configuration mutations` with "Command CRUD (Story 9-6)" subsection + Anti-patterns extension.
- `README.md` — Current Version narrative updated; Epic 9 row updated to reflect 9-6 in review.

**Modified files (story tracking):**
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — 9-6 status flipped from `in-progress` to `review`.
- `_bmad-output/implementation-artifacts/9-6-command-crud-via-web-ui.md` — Status flipped to `review`; all Tasks 0–11 checked complete; Dev Agent Record updated; File List + Change Log filled.

### References

- [Source: `_bmad-output/planning-artifacts/epics.md#Story-8.6` (= sprint-status 9-6), lines 883-897 — BDD acceptance criteria]
- [Source: `_bmad-output/planning-artifacts/epics.md` line 793 — recommended sequencing 9-1 → 9-2 → 9-3 → 9-0 → 9-7 → 9-8 → 9-4 / 9-5 / 9-6]
- [Source: `_bmad-output/planning-artifacts/epics.md` line 771 — numbering offset note (epics file "Story 8.6" = sprint-status 9-6)]
- [Source: `_bmad-output/planning-artifacts/prd.md#FR36, FR40, FR41` lines 402, 406, 407 — command CRUD + validate-and-rollback + mobile-responsive]
- [Source: `_bmad-output/planning-artifacts/prd.md#NFR7-NFR12` lines 437-442 — secrets + permissions + audit logging]
- [Source: `_bmad-output/planning-artifacts/prd.md#FR12` line 363 — command parameter validation (type, range, f_port)]
- [Source: `_bmad-output/planning-artifacts/architecture.md` lines 200-209, 416-421, 444-450, 491, 517-523, 530-534 — config lifecycle + web/ module reservation + static/ layout + web boundary + main.rs orchestration + data-boundary table]
- [Source: `_bmad-output/implementation-artifacts/9-5-device-and-metric-mapping-crud-via-web-ui.md` lines 1-938 — full Story 9-5 spec + iter-1/2/3 review patches (load-bearing precedent)]
- [Source: `_bmad-output/implementation-artifacts/9-4-application-crud-via-web-ui.md` lines 1-919 — full Story 9-4 spec (CSRF + ConfigWriter + AppState + audit taxonomy + iter-1/2/3 review patches)]
- [Source: `_bmad-output/implementation-artifacts/9-7-configuration-hot-reload.md` lines 91, 137-145, 181-218, 274-330, 593, 600-642 — `ConfigReloadHandle` API + `commands_equal` + `topology_device_diff` helper]
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md` lines 218-384 — Story 9-1 / 9-3 / 9-4 / 9-5 / 9-7 deferred items 9-6 inherits + Story 9-5 iter-1/2/3 review-deferred entries]
- [Source: `src/web/mod.rs:78, 97, 222, 364, 396-405` — current `ApplicationSummary` / `DeviceSummary` / `AppState` / `build_router` shape post-9-5 (with the 9-5 device CRUD routes)]
- [Source: `src/web/api.rs` (current shape, post-9-5): `validate_path_application_id` at `:500`, `validate_path_device_id` at `:600`, `find_application_index` at `:2510`, `build_device_table` at `:2574`, `build_read_metric_array` at `:2596`, `validate_application_field` at `:2625`, `validate_device_field` at `:2720`, `handle_rollback` at `:2837`, `application_not_found_response` at `:2883`, `device_not_found_response` at `:2896`, `io_error_response` at `:2912`, `reload_error_response` at `:3033`, `handle_restart_required` at `:3092`. **Verify these locations via `grep -n "fn <name>" src/web/api.rs` at impl time — line numbers drift across iter-loop patches per Story 9-5 iter-3 F4.**]
- [Source: `src/web/csrf.rs:183-225, 239-353, 271-275` — `csrf_event_resource_for_path` + rejection-emission match blocks + the explicit Story 9-6 hand-off comment]
- [Source: `src/web/config_writer.rs` — current `ConfigWriter` API]
- [Source: `src/config.rs:570-670, 977-1700` — `ChirpstackDevice` + `ReadMetric` + `OpcMetricTypeConfig` + `DeviceCommandCfg` + `AppConfig::validate` (with the existing `seen_metric_names` / `seen_chirpstack_metric_names` HashSets from Story 9-5)]
- [Source: `src/config_reload.rs:181-218, 274, 917-948, 1001-1104` — `ConfigReloadHandle::reload()` + `classify_diff` + `commands_equal` + `run_web_config_listener`]
- [Source: `src/storage/types.rs:153-157` — `DeviceCommand::validate_f_port(u8) -> bool` (LoRaWAN 1..=223 range)]
- [Source: `src/storage/sqlite.rs:652-655` — `f_port` runtime validation (`"Invalid f_port {N}: must be 1-223"`)]
- [Source: `src/opc_ua.rs:1059` — command NodeId construction (`NodeId::new(ns, command.command_id as u32)` — per-device-scoped; AC#3 prerequisite)]
- [Source: GitHub issues #88, #100, #102, #104, #108, #110, #113 — carry-forward concerns documented but out-of-scope for Story 9-6]

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — `claude-opus-4-7[1m]` — via the bmad-dev-story skill.

### Debug Log References

(To be filled by the dev agent during implementation.)

### Completion Notes List

(To be filled by the dev agent as Tasks 0–11 complete.)

### File List

(To be filled by the dev agent during implementation.)

### Change Log

| Date | Change | Author |
|------|--------|--------|
| 2026-05-12 | Story created | Claude Code (bmad-create-story) |

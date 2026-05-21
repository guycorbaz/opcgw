# Story C-2: Inventory Pickers in the Web UI

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `C-2-inventory-pickers-web-ui`                                                                              |
| Epic            | C — Auto-Discovery and Web-First Configuration (post-v2.0 GA)                                               |
| FRs             | none (Epic C is post-PRD)                                                                                   |
| Status          | ready-for-dev                                                                                               |
| Created         | 2026-05-21                                                                                                  |
| Source epic     | `_bmad-output/planning-artifacts/epics.md § Epic C § Story C.2`                                             |
| Depends on      | C-1 (`/api/inventory/{applications,devices,uplinks}` endpoints must be live)                                |
| Tracking        | GitHub issue `#__` — user opens out-of-band                                                                 |

---

## User Story

As an **opcgw operator adding a new application, device, or metric through the web UI**,
I want to pick the ChirpStack resource from a named list fetched at form-fill time instead of typing a UUID or DevEUI by hand,
So that I never have to switch to the ChirpStack web UI to look up an ID, never have to remember exact DevEUI formatting, and never have to copy a codec metric key by sight from a side-by-side window.

---

## Story Context

### Why C-2 is the operator-visible payoff of Epic C

C-0 made the gateway bootable empty. C-1 gave the gateway an inventory API. C-2 is the story where the operator finally feels the difference: instead of `Application ID [____________________________]` (paste a UUID), the form is `Application [▼ Arrosage ]` (choose from a list opcgw fetched from ChirpStack 0.5 seconds ago).

The downstream effect is dramatic: a typical operator session goes from "open ChirpStack web UI in tab 2 → copy UUID → switch to opcgw → paste → repeat for device → repeat for each metric" to "click → click → click → submit." That's the UX gap the v2.0 GA walkthrough surfaced on 2026-05-20.

### Existing pages C-2 modifies

- `static/applications.html` (72 lines today) — replace the free-form `application_id` `<input type="text">` at line 44 with a name-driven `<select>` populated from `GET /api/inventory/applications`. The `application_name` field stays editable (operator may override the OPC UA-side display name).
- `static/applications.js` (148 lines today) — adjust the create-form submit handler to read the `application_id` from the picker's `<option value>` rather than from a text input.
- `static/devices-config.html` (93 lines today) — the device-add flow lives in a modal (line 71-89). Extend it with a device picker (populated when an application is selected) and a metric-pick sub-form (populated when a device is selected) that reads `observed_keys` from `/api/inventory/uplinks`.
- `static/devices-config.js` (423 lines today) — substantial extension to drive the cascading dropdowns.

### The cascading picker flow

The full add-flow is **application → device → metric**:

1. Operator clicks "Add application" on `/applications`.
2. UI fetches `/api/inventory/applications` (without `?refresh`).
3. Operator picks application "Arrosage" → form pre-fills `application_id = "ae2012c2-..."` (hidden) + `application_name = "Arrosage"` (editable).
4. Operator submits → existing `POST /api/applications` flow lands the new app in opcgw config.

Then on the devices-config page:

5. Operator picks application "Arrosage" (from the existing app list — already configured in opcgw).
6. Operator clicks "Add device under Arrosage" → UI fetches `/api/inventory/devices?application_id=ae2012c2-...`.
7. Operator picks device "WaterFlowSensor" → form pre-fills `device_id = "a84041b8a1867e20"` (hidden, but rendered as a non-editable footnote so the operator can see/copy the DevEUI for ChirpStack-side troubleshooting) + `device_name = "WaterFlowSensor"` (editable).

Then for each metric the operator wants to expose:

8. Operator clicks "Add metric" within the device sub-form → UI fetches `/api/inventory/uplinks?dev_eui=a84041b8...&limit=10`.
9. UI renders a multi-select list of `observed_keys` with per-key wire-type inference + override dropdown.
10. Operator picks `[x] water_flow` + leaves wire_type = "Float" (auto-inferred) → metric row appears with `chirpstack_metric_name = "water_flow"`, `metric_name = "water_flow"` (editable for OPC UA display rename), `metric_type = Float`.
11. Operator may add multiple metrics per device in one form submission.

The submit at step 11 follows the existing `PUT /api/devices/<dev_eui>` or `POST /api/applications/<app_id>/devices` flow (whichever the current shape is — verify in `src/web/api.rs` during implementation).

### Manual fallback (the load-bearing escape hatch)

ChirpStack can be unreachable. The picker UI must NOT trap the operator: if any inventory endpoint returns 502, the picker switches to a "Type manually" mode rendering the original free-form text input. The operator can then paste a UUID or DevEUI as today, and the form submission flows through the unchanged backend CRUD path. The toggle between picker and manual modes is operator-visible:

- Top of the picker section: a small `[Switch to manual entry]` link that flips the picker to a `<input type="text">` (and vice versa).
- On automatic 502 fallback: a yellow banner "Could not reach ChirpStack — switched to manual entry. [Retry picker]". Clicking Retry re-attempts the inventory fetch.

This protects against three failure modes:
- ChirpStack down for maintenance — operator can still configure opcgw using a value they know.
- Device not yet enrolled in ChirpStack — operator can pre-create opcgw config so the device works the moment it joins.
- Codec hasn't emitted a metric yet — operator can hand-type a metric_name they know the codec will produce.

### Audit-event surface

C-2 adds operator-attributable picker-event audit:

- `event="picker_opened"` — when the picker UI fires its inventory fetch. Carries `picker_resource={application|device|uplink}` and the relevant scoping fields.
- `event="picker_manual_fallback"` — when the picker switches to manual mode (either operator-clicked or auto-on-502). Carries `reason={operator_choice|chirpstack_unreachable|chirpstack_error}`.
- `event="metric_wire_type_inferred"` — when a metric is added via the picker. Carries `dev_eui`, `chirpstack_metric_name`, `inferred_type`, `operator_chosen_type` (when different from inferred), `sample_values_count`.

The first two are emitted server-side via a new `POST /api/audit/picker-event` thin endpoint the picker JS calls (CSRF-protected, basic-auth-gated). Alternative: emit client-side via `console.log` only and let the browser DevTools be the audit surface — but server-side audit is the project convention, so use the endpoint.

The third (`metric_wire_type_inferred`) fires from the existing `POST /api/devices/...` or whatever the metric-create endpoint is, when the request includes the new `picker_metadata` field (see AC#10).

---

## Acceptance Criteria

### Application picker on `/applications`

1. **The "Create application" form replaces its free-form `application_id` input with a name-driven `<select>` populated from `GET /api/inventory/applications`.**
   - On page load, the picker fires the inventory fetch (no `?refresh`). While loading: `<select disabled><option>Loading…</option></select>`.
   - On success: the picker renders `<option value="<app_id>">{name}</option>` for each item, sorted alphabetically (the API already sorts, but the picker doesn't assume — it sorts client-side to defend against any future API change).
   - On 502/error: the picker switches to manual fallback per AC#2.
   - On `count: 0` (ChirpStack has no applications for this tenant): the picker renders an "no applications in ChirpStack — type one manually or create one in ChirpStack first" empty state and pre-flips to manual mode.

2. **Manual fallback toggle on application picker.**
   - A `[Switch to manual entry]` / `[Switch to picker]` toggle link above the input.
   - In manual mode: the original `<input type="text" id="new-application-id" name="application_id" required>` is rendered (same as today's UI per `static/applications.html:44`).
   - In picker mode: the `<select>` is rendered.
   - Mode persists per browser session (localStorage), so an operator who prefers manual entry doesn't have to re-toggle every page load.

3. **`application_name` field defaults to the picker's selected name but is operator-editable.**
   - When the operator picks "Arrosage", the `application_name` input pre-fills with "Arrosage".
   - The operator can then edit it to "Arrosage (Watering — Garden Sector 1)" or whatever OPC UA-side label they want.
   - When the operator switches selections in the picker, `application_name` is re-populated UNLESS the operator has already edited it — heuristic: track an `edited` flag on the `application_name` input that flips true on first user keystroke; once true, don't re-populate.

### Device picker on `/devices-config`

4. **Device-add modal renders a name-driven picker fed by `GET /api/inventory/devices?application_id=<chosen-app>`.**
   - The picker is per-application — opening "Add device under Arrosage" fetches devices for application_id `ae2012c2-...`; opening "Add device under Bâtiments" fetches devices for `194f12ab-...`.
   - Picker rendering follows the same shape as the application picker (sorted dropdown, loading / empty / error states).
   - Pre-fills `device_id` (hidden) + `device_name` (editable, defaults to ChirpStack's device name) when an option is selected.
   - DevEUI is shown as small-text under the picker (e.g., `<small>DevEUI: a84041b8a1867e20</small>`) so the operator can copy it for ChirpStack-side troubleshooting if needed. The DevEUI is NOT operator-editable in the picker mode — it's the cache key for the metric pick step.

5. **Manual fallback toggle on device picker — same shape as AC#2.**
   - Mode persists per browser session per application (or globally — Dev Agent picks; document in Dev Notes).

6. **Empty-list and error states match AC#1.**

### Metric picker (multi-select with wire-type inference)

7. **Within the device-add modal, the "Add metric" sub-form fetches `GET /api/inventory/uplinks?dev_eui=<chosen-dev>&limit=10` when the operator chooses a device.**
   - While loading: `<p>Loading recent uplinks…</p>` (this can take up to `inventory_uplink_max_wait_seconds` seconds, default 5 — see C-1 AC#6).
   - On 502: the operator can still add metrics manually using the existing metric-row form (the metric-row at `static/devices-config.html:26-34` renders a single row of free-form inputs).

8. **`observed_keys` from the API are rendered as a multi-select checkbox list.**
   - Each row: `[ ] <chirpstack_metric_name>  [▼ Float|Int|Bool|String]  sample: <sample_value>`.
   - The wire-type dropdown defaults to the inferred type (`observed_keys[i].wire_type` from the API).
   - The operator can override the wire type per row.
   - The operator can check multiple keys to add multiple metrics in one submission.

9. **Empty `observed_keys` triggers the manual-entry mode for metrics.**
   - When `observed_keys: []` (no recent uplinks within `max_wait`), render: "No recent uplinks for this device. Either wait for the device to send and refresh, or add metrics manually below." + the existing manual metric-row form.
   - A `[Refresh picker]` button re-fires the uplinks fetch.

10. **On metric submit, the picker emits the per-metric wire-type-inference audit event** via the metric-creation request including a `picker_metadata` field:

    ```json
    {
      "chirpstack_metric_name": "water_flow",
      "metric_name": "water_flow",
      "metric_type": "Float",
      "picker_metadata": {
        "inferred_type": "Float",
        "operator_chosen_type": "Float",
        "sample_values_count": 8
      }
    }
    ```

    The server-side handler emits `event="metric_wire_type_inferred"` with these fields. If `picker_metadata` is absent (operator used manual entry, or the request comes from a non-picker source), no event fires — manual-entry remains unaudited beyond the existing `device_crud` events.

### Picker-event audit (server-side surface)

11. **New `POST /api/audit/picker-event` thin endpoint** for client-side audit events.
    - Body: `{ "event": "picker_opened" | "picker_manual_fallback", "fields": { ... } }`.
    - Server validates the event name is one of the documented values (reject unknown events with 400).
    - Server emits the structured `tracing` audit event with `source="web_picker"` and a sanitised version of the operator-provided fields (reject any fields not in an allowlist per event).
    - Basic-auth gated. CSRF-protected (token from the same source as the existing CRUD handlers).
    - No mutation of opcgw state — the endpoint is logging-only.

12. **`picker_opened` audit fields:** `picker_resource={application|device|uplink}`, `application_id=<… for device/uplink scope>`, `dev_eui=<… for uplink scope>`, `cache_status=<received from C-1's response>`.

13. **`picker_manual_fallback` audit fields:** `picker_resource={application|device|uplink}`, `reason={operator_choice|chirpstack_unreachable|chirpstack_error}`, `error_detail=<optional shortstring>`.

### Picker UX details (defensive against common UI footguns)

14. **CSRF discipline is preserved.**
    - The submit flow (POST /api/applications, etc.) honours the existing CSRF token discipline. The picker's `POST /api/audit/picker-event` carries the CSRF token the same way.
    - GET inventory fetches do NOT require CSRF (matches C-1 AC contract).
    - There is NO race where the picker's `<option value>` becomes stale between selection and submit — the form submit always reads the current `<select>` value at submit time, not a captured-at-fetch value.

15. **Picker-selection ID round-trip integrity.**
    - When the operator picks "Arrosage" with `<option value="ae2012c2-c7f1-4fbd-8f87-4025e1d49242">Arrosage</option>` and submits, the POST body's `application_id` is exactly `"ae2012c2-c7f1-4fbd-8f87-4025e1d49242"` — no truncation, no case change, no whitespace.
    - Integration test asserts this round-trip.

16. **Re-fetch on demand: each picker has a small refresh icon** that fires the inventory fetch with `?refresh=true` (bypasses cache). Useful when the operator added a device to ChirpStack in tab 2 and wants to see it in opcgw immediately.

17. **Form reset behaviour.**
    - After successful submit, the form clears (matches today's behaviour).
    - The picker re-fetches inventory on next form open (cache may serve a hit if within TTL — that's the intended behaviour from C-1).

18. **Localstorage is scoped per-origin.**
    - The manual-vs-picker mode preference is stored under a single localStorage key per page (`opcgw.picker.applications.mode`, `opcgw.picker.devices.mode`, `opcgw.picker.metrics.mode`).
    - No cross-page mode leakage. An operator who picked "manual" for applications can still use picker mode for devices.

### Inventory cache + sibling-story interaction

19. **Picker-driven CRUD writes invalidate the cache** as per C-1 AC#9 — no new work in C-2; this is just a reminder for testing. An integration test verifies: open picker → see app A → use picker to create app B in opcgw → re-open picker → cache invalidation has happened, the next fetch is a cache miss (cache_status="miss" in audit log).

20. **Picker DOES NOT use `?refresh=true` by default** (matches the scope decision). The `?refresh=true` parameter is only fired when the operator explicitly clicks the refresh icon (AC#16) or when the drift view (Story C-4) calls the endpoints.

### Integration tests + regression invariants

21. **Integration tests** in a new `tests/web_picker.rs` or extending `tests/web_application_crud.rs` / `tests/web_device_crud.rs`. At minimum 10 tests covering:
    - Application picker: GET /applications renders the page; submitting via picker mode pre-fills application_id correctly.
    - Application picker: 502 from `/api/inventory/applications` flips the form to manual mode.
    - Application picker: empty inventory triggers manual mode + empty-state copy.
    - Device picker: device pre-fill works; the DevEUI displayed under the picker matches the hidden field.
    - Metric picker: `observed_keys` rendered as multi-select; wire-type override changes submit payload.
    - Metric picker: `picker_metadata` field round-trips to the server; the server emits `metric_wire_type_inferred` audit event.
    - Metric picker: empty `observed_keys` triggers manual-entry mode.
    - Cache invalidation: creating an app via picker → next inventory fetch is a cache miss.
    - `POST /api/audit/picker-event` rejects unknown event names with 400.
    - `POST /api/audit/picker-event` emits the audit event with sanitised fields.

22. **JavaScript-side smoke tests are NOT in scope for `cargo test`.** Frontend testing remains manual + integration-test driven (a Selenium / Playwright setup is a future story). Document the manual smoke test in Dev Notes: walk through application → device → metric add-flow against Guy's real ChirpStack at 192.168.1.12:8080 with verification of each picker pre-fill.

23. **`cargo test --all-targets` passes.** Pre-C-2 baseline is post-C-1's count (assume ≥ 1281/0/≥10 if C-0 and C-1 hit their targets). C-2 target: ≥ 1291 / 0 / ≥ 10 — the +10 from AC#21's integration tests. Document the actual delta in Dev Notes.

24. **`cargo clippy --all-targets -- -D warnings` clean.**

25. **`cargo test --doc` no regressions.** ≥ 56 ignored, 0 failed.

26. **Strict-zero file invariants.** NO changes to: `migrations/*.sql`, `Cargo.toml`, `Cargo.lock`, `src/chirpstack.rs` (C-1 owns ChirpStack-side changes), `src/storage/*`, `src/opc_ua*.rs`, `src/main.rs`. Mutable scope:
    - `static/applications.html` + `static/applications.js`
    - `static/devices-config.html` + `static/devices-config.js`
    - Possibly a new `static/inventory-picker.js` shared module (Dev Agent decides if the JS surface area justifies extraction — current devices-config.js is 423 lines; adding pickers may push it past 700, in which case extract)
    - `src/web/api.rs` (new `POST /api/audit/picker-event` handler + `picker_metadata` field handling on existing metric-create path)
    - `src/web/mod.rs` (route wiring for the new audit endpoint)
    - `tests/web_picker.rs` (NEW) or extensions to existing `tests/web_*.rs`
    - `docs/web-api.md` / `docs/inventory-api.md` (document the picker-event audit endpoint + the `picker_metadata` field)
    - `docs/logging.md` (3 new audit events)
    - `_bmad-output/implementation-artifacts/sprint-status.yaml`
    - This story spec file

### Documentation sync

27. **`docs/web-api.md` (or wherever C-1's API docs landed)** gets a new section "Picker-event audit endpoint" documenting `POST /api/audit/picker-event` + a "metric-create with picker_metadata" sub-section under the existing device-CRUD docs.

28. **`docs/logging.md`** gains entries for `picker_opened`, `picker_manual_fallback`, `metric_wire_type_inferred`.

29. **`README.md` Planning table** Epic C row updated post-C-2 landing ("Epic C 3/6 done").

30. **DocBook user manual `docs/manual/opcgw-user-manual.xml`** gains a new `<sect1>` titled "Adding applications, devices, and metrics via the web UI" under the Configuration chapter (added in B-1). Walks through the cascading picker flow with screenshots-or-equivalent-descriptions. DocBook 4.5 syntax preserved per memory `[[project_user_manual_format]]`.

### GitHub tracking issue

31. GitHub tracking issue (suggested title: "C-2: Inventory pickers in the web UI") opened by user out-of-band. Issue number captured in Dev Notes.

---

## Tasks / Subtasks

- [ ] **Task 0 — Tracking issue acknowledgment (AC: #31)**
  - [ ] 0.1 User opens GitHub issue.
  - [ ] 0.2 Capture issue number in Dev Notes.
  - [ ] 0.3 `Refs #N` in every commit.

- [ ] **Task 1 — Application picker on `/applications` (AC: #1, #2, #3)**
  - [ ] 1.1 Update `static/applications.html`: replace `<input id="new-application-id">` with a `<select>` + toggle link, keeping the manual `<input>` as a fallback element with `hidden`.
  - [ ] 1.2 Update `static/applications.js`: add `fetchInventoryApplications()`, render the dropdown, handle picker → manual fallback, handle 502, handle empty-list, handle the application_name pre-fill + edited-flag logic.
  - [ ] 1.3 Wire up the toggle link + localStorage mode persistence per AC#2 and AC#18.
  - [ ] 1.4 Manual smoke test against Guy's real ChirpStack: open `/applications`, see "Arrosage" + "Bâtiments" in the picker.

- [ ] **Task 2 — Device picker on `/devices-config` (AC: #4, #5, #6)**
  - [ ] 2.1 Update `static/devices-config.html`: extend the "Add device" sub-form (or modal) with a picker for devices.
  - [ ] 2.2 Update `static/devices-config.js`: add `fetchInventoryDevices(application_id)`, render the dropdown, handle the cascading state (picker is empty until an application is chosen).
  - [ ] 2.3 Show the DevEUI under the picker as small-text per AC#4.
  - [ ] 2.4 Manual smoke test: under application "Arrosage", see device "WaterFlowSensor" in the picker.

- [ ] **Task 3 — Metric picker with wire-type inference (AC: #7, #8, #9, #10)**
  - [ ] 3.1 Extend the device-add form with the metric-pick sub-form rendering `observed_keys` as multi-select with per-row wire-type dropdown.
  - [ ] 3.2 Wire up the submission to include `picker_metadata` per metric.
  - [ ] 3.3 Handle the empty-observed-keys case + the [Refresh picker] button.
  - [ ] 3.4 Manual smoke test: pick a device that recently sent uplinks; verify the metric picker renders observed keys; submit and verify the metric appears.

- [ ] **Task 4 — Server-side audit endpoint + picker_metadata handling (AC: #11, #12, #13, #14)**
  - [ ] 4.1 New `POST /api/audit/picker-event` handler in `src/web/api.rs` (or `src/web/audit.rs` if extracted).
  - [ ] 4.2 Handler validates event name + sanitises fields per allowlist.
  - [ ] 4.3 Emit `tracing::info!(event = …)` with `source="web_picker"`.
  - [ ] 4.4 In the existing metric-create path, parse optional `picker_metadata` field and emit `event="metric_wire_type_inferred"` when present.
  - [ ] 4.5 Router wiring in `src/web/mod.rs`.

- [ ] **Task 5 — Shared client-side module (AC: #1, #4, #7 — refactor concern)**
  - [ ] 5.1 If the picker JS surface area grows past ~150 lines duplicated across `applications.js` and `devices-config.js`, extract into `static/inventory-picker.js` with `renderApplicationPicker(targetEl)`, `renderDevicePicker(targetEl, app_id)`, `renderMetricPicker(targetEl, dev_eui)`, plus a shared `auditPickerEvent(eventName, fields)` helper.
  - [ ] 5.2 Document the extraction choice in Dev Notes.

- [ ] **Task 6 — Integration tests (AC: #21)**
  - [ ] 6.1 Create `tests/web_picker.rs` (or extend existing CRUD test files).
  - [ ] 6.2 Implement the 10 named tests from AC#21.
  - [ ] 6.3 Cache-invalidation test: end-to-end pick-and-create flow that asserts the next inventory fetch is a cache miss.

- [ ] **Task 7 — Documentation sync (AC: #27, #28, #29, #30)**
  - [ ] 7.1 `docs/web-api.md` — picker-event audit endpoint + `picker_metadata` field.
  - [ ] 7.2 `docs/logging.md` — 3 new audit events.
  - [ ] 7.3 `README.md` Planning table.
  - [ ] 7.4 DocBook user manual `<sect1>` for the picker UX (DocBook 4.5 syntax — verify with `xmllint --noout --valid`).

- [ ] **Task 8 — Regression gate + commit (AC: #23, #24, #25, #26)**
  - [ ] 8.1 `cargo test --all-targets` → record count; target ≥ 1291/0/≥10.
  - [ ] 8.2 `cargo clippy --all-targets -- -D warnings` → clean.
  - [ ] 8.3 `cargo test --doc` → no regressions.
  - [ ] 8.4 Manual end-to-end smoke against Guy's real ChirpStack: add an application via picker, add a device under it via picker, add 2 metrics via picker; verify all four flow into opcgw config and the OPC UA browse tree updates.
  - [ ] 8.5 Commit message: `Story C-2: Inventory pickers in the web UI - Implementation Complete` + `Refs #<issue>`.

---

## Dev Notes

### Why the manual fallback toggle is load-bearing, not optional

A picker that traps the operator (no way to type a value by hand) is a step backward from today's UX. Three real-world cases require manual entry:

1. **ChirpStack maintenance window** — opcgw operator is configuring a SCADA tag at 11 PM, ChirpStack is being upgraded by the LoRaWAN admin team. The operator should not have to wait.
2. **Device not yet enrolled** — operator wants to pre-create opcgw config so the device works the moment its first uplink lands.
3. **Metric not yet emitted** — codec has a conditional metric (e.g., `battery_alarm`) that fires only on a specific event. The operator knows the field name from the codec source; they want to add it before it fires.

The fallback toggle + auto-fallback-on-502 covers all three.

### Why localStorage for picker-vs-manual mode preference

Some operators will prefer manual entry permanently (e.g., scripted deployment via copy-paste from a runbook). Forcing them through the picker on every page load adds friction. Persisting the preference per-page (not per-session) means the choice survives browser refresh / re-login.

The localStorage scoping in AC#18 keeps the keys explicit per page; no cross-contamination, no schema-versioned localStorage migration headache.

### Why a separate `POST /api/audit/picker-event` endpoint instead of client-side logging

opcgw's audit-event convention is server-side `tracing` events with structured fields. Client-side `console.log` is operator-tooling, not audit. To keep the audit log canonical:

- The picker JS calls a thin server endpoint.
- The server validates + sanitises the event.
- The server emits the `tracing` event into the canonical audit stream.

This adds two round-trips per picker session (open + fallback), which is acceptable overhead. The alternative — pure client-side audit — would leak operator events out of the canonical stream and is rejected.

### Why metric_wire_type_inferred fires from the metric-create path, not the picker

Wire-type inference happens at metric-pick time, but the **decision** is sealed at metric-create time (operator may have overridden the inferred type between picking and submitting). The server emits the audit event at the moment the decision becomes permanent (POST /api/devices/.../metrics or wherever) so the audit reflects the as-shipped choice, not the pre-edit state.

### Picker re-fetch on form re-open vs cached state

When the operator opens the "Add application" form, the picker fires `/api/inventory/applications`. If they cancel, then immediately re-open the form, the picker re-fires the fetch — but C-1's cache TTL (60 s default) will serve a hit the second time. This is intended: the picker JS is dumb (always fetches on render); the cache layer (C-1) is smart (decides whether to hit ChirpStack).

The picker does NOT cache results in JS; this avoids browser-tab desync (operator has two tabs open; one creates an app; the other tab's JS cache wouldn't see it; opcgw's server-side cache invalidation per C-1 AC#9 keeps the canonical view consistent across tabs).

### Frontend testing pragmatics

opcgw doesn't have a JS test framework today. C-2 adds significant JS surface (~500-700 lines across the picker module + page integrations). Three options for testing JS-side correctness:

- **(a) Integration tests via server-side endpoints + manual smoke** (chosen for C-2): the `tests/web_picker.rs` tests cover the SERVER side of every picker interaction (inventory API responses, audit endpoint, metric-create with picker_metadata). The CLIENT side is covered by manual smoke against real ChirpStack documented in Task 8.4.
- **(b) Headless browser tests via wasm-bindgen + headless_chrome** — out of scope; introduces a heavy CI dependency.
- **(c) Selenium / Playwright suite** — out of scope; a future Epic E candidate.

The Dev Agent should NOT introduce a JS test framework in C-2. Option (a) is acceptable for the picker's complexity level; if future stories need it, address then.

### The `application_name` "edited" flag heuristic

AC#3's "track whether the operator has edited the field" requires keystroke-level state. Two implementations:

- **(a)** Set `edited = true` on the input's `keydown` event (any keystroke counts, even backspace-then-retype-same-value).
- **(b)** Set `edited = true` only when the current value diverges from the picker's auto-populated value.

Option (a) is simpler and more predictable from the operator's POV ("once I touched it, you stop overwriting it"). Use option (a).

### Carry-forward GitHub issues

#88 (rate limiting), #100 (doctest baseline), #102 (tests/common reuse), #104 (TLS hardening), #117 (perf-CI lane). C-2 may surface a new issue if cache invalidation race window (C-1 Dev Notes) becomes operator-visible — log to `deferred-work.md` if so.

---

## Out of Scope

- **Drift view (`/inventory-drift`)** — C-4's deliverable. C-2's pickers handle "add new" only; reconciling pre-existing config with ChirpStack drift is C-4's job.
- **Duplicate-prevention server-side enforcement** — C-3's deliverable. C-2 pickers may temporarily allow the operator to attempt to add a duplicate (the existing CRUD path rejects it today; C-3 makes the rejection consistent + cross-path).
- **Bulk-add (add many devices in one shot)** — out of scope. The picker adds one resource per submit; multi-metric per device-add is the only multi-row mode.
- **Device profile / decoder selection** — opcgw doesn't care about ChirpStack codec specifics beyond reading `decoded_object`. Out of scope.
- **JS test framework** — see Dev Notes.
- **Picker for ChirpStack commands queue** — Story 9-6's commands.html UI is unchanged; C-2 does not pickerise it.
- **Mobile UX polish** — the existing CSS has 600px breakpoints; C-2 keeps them but doesn't add tablet-specific touches.

---

## Completion Note

To be filled in by the dev agent at story completion. Should include: actual test count delta, final extraction decision for `static/inventory-picker.js`, manual smoke-test results against Guy's real ChirpStack, any deferred follow-ups added to `deferred-work.md` (especially around JS testing infrastructure).

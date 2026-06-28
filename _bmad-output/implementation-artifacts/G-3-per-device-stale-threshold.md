# Story G.3: Per-Device OPC UA Stale Threshold

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator with slow-reporting LoRaWAN sensors**,
I want to set the OPC UA stale threshold **per device** from the web UI (not just globally),
so that infrequent sensors (a weather station every few minutes, a valve that reports rarely) don't read `Uncertain`/`Bad` between their normal uplinks, while fast devices still flag genuine staleness quickly.

**GitHub issue:** [#132](https://github.com/guycorbaz/opcgw/issues/132). **Epic:** G — Web UX & Usability (v2.5.0 line). Builds on the **G-0 drill-down** device-detail view.

## Context & Scope Boundary (read first)

**Most of #132 already exists** — this story closes the *web write path*, not the data model:

- ✅ The config model has a per-device override: `ChirpstackDevice.stale_threshold_seconds: Option<u64>` (`src/config.rs:668`, "overrides the global `[opcua].stale_threshold_seconds`, default 120"), with validation (`config.rs:2010-2014`, Story E-1).
- ✅ SQLite **already persists** it: migration **v012** (`migrations/v012_device_stale_threshold.sql`) added the `devices.stale_threshold_seconds` column; `load_all_applications_config` reads it (`src/storage/sqlite.rs:3022-3048, 3146`) and the bulk-migration insert writes it (`sqlite.rs:2579-2586`). **No new migration — schema is at v012.**
- ✅ The OPC UA staleness derivation **already honors** per-device overrides (Story E-1, 2026-06-12), sourced from the loaded config.

**The gaps this story closes:**

1. **Web CRUD never writes it.** `create_device` / `update_device` (`src/web/api.rs`) and the storage method `insert_device_with_metrics` (`src/storage/sqlite.rs:3548`) take only name + metrics — the device create/update **request structs lack `stale_threshold_seconds`**, so a device created/edited via the web UI always has `NULL` (→ global). There is no update path for the column at all.
2. **`/api/devices` exposes only the global threshold.** The response has a single top-level `stale_threshold_secs: u64` (`api.rs:446`, the resolved global); `DeviceView` (`api.rs:462-465`) carries no per-device value, so the **F-3 dashboard freshness panel uses global for every device** (F-3 explicitly descoped per-device to global-only).
3. **No UI field.** The G-0 device-detail view (`static/config.js` `mountDeviceDetail`, Metrics panel) has device-name + metric rows but no stale-threshold input.

**In scope:** request structs + handlers + storage write/read for the existing column; `/api/devices` per-device field; client freshness honoring it; the device-detail form field; tests + docs.
**Out of scope / do NOT:** add a new schema migration (the column exists); change the OPC UA staleness derivation (E-1 already honors it); touch the global `[opcua].stale_threshold_seconds`; add new endpoints; reinvent clamping (reuse `crate::web::clamp_stale_threshold` + `DEFAULT_STALE_THRESHOLD_SECS = 120`).

## Acceptance Criteria

1. **Editable per device in the web UI.** The G-0 device-detail view (`/config.html` → an application → a device) shows an optional **Stale threshold (seconds)** field in the Metrics panel, labelled as overriding the global default (120 s) and empty = "use global". Saving the device (the existing `PUT …/devices/:id`) persists it; clearing it (empty) stores `NULL` (revert to global). Lives in the G-0 drill-down, reusing its patterns; no new page.
2. **Web CRUD round-trips it.** The device **create** (`POST …/devices`) and **update** (`PUT …/devices/:id`) request structs accept an optional `stale_threshold_seconds`; the handlers validate it and persist to the existing `devices.stale_threshold_seconds` column; `GET …/devices/:id` returns the stored value (or null). Round-trips through SQLite via the v012 column. No new migration.
3. **Validation reuses existing rules.** An out-of-band value is handled the same way the rest of the codebase does — per-device validation (`config.rs:2010-2014`) / `clamp_stale_threshold` band `(0, 86400]`; reject or clamp consistently with the global path, with a clear error/audit. `0`/negative rejected; absent = global.
4. **`/api/devices` carries the per-device value.** `DeviceView` gains `stale_threshold_seconds: Option<u64>` (per device); the existing top-level global `stale_threshold_secs` / `bad_threshold_secs` stay. The client freshness band (dashboard `dashboard.js` device-freshness panel, and any device freshness shown in the config view) uses the **per-device** threshold when set, falling back to the global default when null — the same band model, no third model.
5. **OPC UA path takes effect on Apply.** Because the OPC UA staleness derivation already reads per-device overrides from the loaded config (E-1) and the value now persists to SQLite, a web-set per-device threshold takes effect after the operator clicks **Apply changes** (F-0 soft restart). Verify end-to-end; no new OPC UA code expected.
6. **No-regression.** Served-HTML marker pins unchanged; the `/api/devices` shape-pin test (`tests/web_dashboard.rs::api_devices_returns_json_with_expected_shape_when_authed`) is updated to assert the new per-device field while keeping the existing global fields; existing device-CRUD tests pass (extended, not weakened); no `<nav` in `static/*.html`; no build step.
7. **Quality gates.** `cargo test` 0-fail; `cargo clippy --all-targets -- -D warnings` clean; `node --check` on changed JS; manual PDF rebuilds; no new dependency.

## Tasks / Subtasks

- [x] **Task 1 — Device CRUD request/response + handlers.** Added `#[serde(default)] stale_threshold_seconds: Option<u64>` to `CreateDeviceRequest` + `DeviceResponse` (api.rs); `update_device`'s JSON-walk gained a `"stale_threshold_seconds"` arm (null→clear, number→override); new `validate_opt_stale_threshold` helper rejects out-of-band `(0, 86400]` with a 400 (`#[allow(clippy::result_large_err)]` per the existing validator convention); `get_device` returns the stored value.
- [x] **Task 2 — Storage write path.** `insert_device_with_metrics` + `update_device_name_and_metrics` (sqlite.rs) take `Option<u64>` and write the v012 `devices.stale_threshold_seconds` column (INSERT col + `UPDATE … SET stale_threshold_seconds = ?`). No new migration. `load_all_applications_config` already reads it → create/update round-trip verified by test.
- [x] **Task 3 — `/api/devices` per-device field.** `DeviceView` + `DeviceSummary` (web/mod.rs) gained `stale_threshold_seconds: Option<u64>`, populated in `DashboardConfigSnapshot::from_config` and surfaced in `api_devices`. Top-level global `stale_threshold_secs`/`bad_threshold_secs` unchanged.
- [x] **Task 4 — Client freshness honors per-device.** `static/dashboard.js summariseFreshness` now uses `device.stale_threshold_seconds` (when a valid positive number) else the global `staleSecs` — same band model, per-device `staleSecs`.
- [x] **Task 5 — Device-detail form field.** `static/config.js mountDeviceDetail` Metrics panel adds an optional numeric "Stale threshold (seconds)" input (prefilled from `dev.stale_threshold_seconds`); the device `PUT` payload sends the parsed int, or `null` when empty (PUT-replace → clears to global).
- [x] **Task 6 — Tests.** `api_devices_returns_json_with_expected_shape_when_authed` extended (d1=Some(600), d2=null asserted); new `post_put_device_round_trips_per_device_stale_threshold` (create→GET→clear→invalid-rejected); all DeviceSummary fixtures updated; `node --check` clean.
- [x] **Task 7 — Docs + gates.** Manual `body.tex` staleness/config-knob note added (per-device override web-editable); manual PDF rebuilt (67 pp). `web-api.md` carries no device-CRUD field table, so no change needed there. Gates: full `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean, `node --check config.js dashboard.js` OK.

## Dev Notes

### The column exists — this is the write path + surfacing, not the data model
Migration **v012** (`migrations/v012_device_stale_threshold.sql`, schema version 12, Story E-1) already added `devices.stale_threshold_seconds INTEGER NULL`. `load_all_applications_config` reads it (`sqlite.rs:3022-3048` → mapped at `:3146` into `ChirpstackDevice.stale_threshold_seconds`). **Do NOT add a migration.** The bulk-migration insert (`sqlite.rs:2579-2586`) shows the exact column write; mirror that in the CRUD `insert_device_with_metrics` + the update path.

### Key files / line anchors
- `src/web/api.rs`: device create/update request structs (~`1896`/`1917`/`2041`/`2050` area — verify exact names), `create_device` handler (calls `insert_device_with_metrics` at `~2304` with name+metrics only — add threshold), `DeviceView` (`462-465`), `/api/devices` handler (`~553-688`, resolves global `stale_threshold_secs` at `564`), `DEFAULT_STALE_THRESHOLD_SECS = 120` (`260`), `clamp_stale_threshold` (`src/web/mod.rs:437`).
- `src/storage/sqlite.rs`: `insert_device_with_metrics` (`3548`), bulk-insert column reference (`2579`), `load_all_applications_config` (`2995`, reads column at `3022-3048`).
- `src/config.rs`: `ChirpstackDevice.stale_threshold_seconds` (`668`), per-device validation (`2010-2014`).
- `static/config.js`: `mountDeviceDetail` Metrics panel (device-name input + metric rows + Save → `PUT …/devices/:id`).
- `static/dashboard.js`: device-freshness band (the F-3 panel using the global threshold today).
- `tests/web_dashboard.rs`: `api_devices_returns_json_with_expected_shape_when_authed` (shape pin to extend), `DEFAULT_STALE_THRESHOLD_SECS`/120 default assertion.

### Patterns to follow / anti-patterns to avoid
- ✅ Reuse `clamp_stale_threshold` + `DEFAULT_STALE_THRESHOLD_SECS`; mirror the global-path validation/audit for consistency.
- ✅ `#[serde(default)]` Option on the request struct so existing clients (and config import) without the field keep working (backward-compatible payload — note: this is an *additive optional* field, allowed under AC2's "no new endpoint"; the import/export path (F-4) should also tolerate it).
- ✅ Frontend changes ride the G-0 drill-down patterns (the device `PUT` already exists; just add a field to the payload).
- ❌ New schema migration (column exists at v012).
- ❌ Changing the OPC UA staleness derivation (E-1 already honors per-device).
- ❌ A second freshness band model on the client — extend the existing one (per-device ?? global).
- ❌ Touching the global `[opcua].stale_threshold_seconds` semantics.

### No-regression contract
`/api/devices` gains an *additive* per-device field — update the shape-pin test to assert it while keeping the global-field assertions. Served-HTML marker pins (dashboard/metrics/config) are unaffected. Device-CRUD tests extend (set/clear/invalid round-trip). Config export/import (F-4) round-trips the new optional field (verify it isn't stripped).

### Previous-story intelligence
- E-1 added the column + OPC UA honoring + validation; F-3 exposed the global on `/api/devices` and descoped per-device — this story finishes that thread.
- G-0 built the device-detail view this UI field slots into; reuse its `el()`/fetch/`PUT` patterns and the `jsonHeaders()` + `credentials:'include'` convention.

### Project Structure Notes
Touches `src/web/api.rs`, `src/storage/sqlite.rs`, `static/config.js`, `static/dashboard.js`, tests, docs. No new files, no new dependency, no migration. (Unlike G-0, this story DOES touch Rust — the web write path + storage — which is expected and in scope.)

### References
- [Source: _bmad-output/planning-artifacts/epics.md#Epic G — Story G.3]
- [Source: GitHub #132]
- [Source: src/config.rs:668,2010 · src/storage/sqlite.rs:2579,3022,3548 · src/web/api.rs:260,446,462,553 · src/web/mod.rs:437 · migrations/v012_device_stale_threshold.sql]
- [Source: static/config.js (mountDeviceDetail) · static/dashboard.js (freshness panel) · tests/web_dashboard.rs (api_devices shape pin) · tests/web_device_crud.rs]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context)

### Debug Log References

- Gates: full `cargo test` exit 0 (no failures); `cargo clippy --all-targets -- -D warnings` clean; `web_dashboard` 16/0 (extended /api/devices shape pin), `web_device_crud` incl. new `post_put_device_round_trips_per_device_stale_threshold` pass; `node --check static/config.js static/dashboard.js` OK; manual PDF 67 pp.

### Completion Notes List

- **Scope held:** closed the web write path only — no schema migration (v012 column reused), no change to the OPC UA staleness derivation (E-1 already honors per-device from the loaded config), no new endpoint. After the operator clicks **Apply**, the F-0 soft restart reloads config from SQLite and the OPC UA path picks up web-set per-device thresholds.
- **PUT-replace semantics** for the field: a `null` (or absent) `stale_threshold_seconds` clears the override to the global default, consistent with how `device_name`/`read_metric_list` PUT-replace. The G-0 device form always sends the field.
- **Validation** rejects out-of-band `(0, 86400]` at the CRUD layer (400) so a value that would later fail `AppConfig::validate` can't poison the next reload. Reused the band constant `BAD_THRESHOLD_SECS`.
- **Signature change churn:** `insert_device_with_metrics` / `update_device_name_and_metrics` gained an `Option<u64>` param → updated all callers (test_support.rs + 7 integration-test helper blocks, all passing the device's own value or `None`).
- **Reviewer focus:** the `update_device` null-vs-number JSON-walk arm + PUT-replace clearing; that the v012 column write is correct in both insert and update; the client per-device-vs-global freshness fallback; no secret/no-regression on `/api/devices` shape (additive field).

### File List

- `src/web/api.rs` — `CreateDeviceRequest`/`DeviceResponse`/`DeviceView` + `stale_threshold_seconds`; `validate_opt_stale_threshold`; create/update/get_device + `api_devices` wiring
- `src/web/mod.rs` — `DeviceSummary.stale_threshold_seconds` + `from_config` populate
- `src/storage/sqlite.rs` — `insert_device_with_metrics` + `update_device_name_and_metrics` write the v012 column
- `src/web/test_support.rs` — caller updated for the new arg
- `static/config.js` — device-detail "Stale threshold (seconds)" input + PUT payload
- `static/dashboard.js` — per-device freshness band (per-device threshold ?? global)
- `tests/web_dashboard.rs` — `/api/devices` shape pin extended (per-device field); DeviceSummary fixtures updated
- `tests/web_device_crud.rs` — new round-trip test + helper caller updated
- `tests/{web_picker,web_duplicate_prevention,web_application_crud,web_command_crud,web_inventory_drift,sqlite_application_list_on_restart}.rs` — helper callers updated for the new arg
- `docs/manual/latex/body.tex` — per-device stale-threshold note (staleness / config-knob)

### Change Log

- 2026-06-27: Implemented G-3 — per-device OPC UA stale threshold web write path (#132): device CRUD + storage column write (v012, no migration) + `/api/devices` per-device field + dashboard freshness + device-detail form input. Status → review.
- 2026-06-27: Code review (3 adversarial layers + mandatory iter-2). 0 HIGH. iter-1 fixed 1 MEDIUM + LOWs (see review section); iter-2 verified all fixes correct, no new defects, LOW-only remains. Loop terminated. Status → done.

## Senior Developer Review (AI)

**Date:** 2026-06-27 · **Reviewer model:** claude-opus-4-8[1m] (3 parallel layers + mandatory iter-2) · **Outcome:** Approved (loop terminated — LOW-only).

**Auditor:** 6/7 ACs fully MET; all "do NOT" constraints upheld (no migration, no OPC UA-derivation change, global semantics untouched, no new endpoint, band/clamp reused). AC4 partially met → the M1 fix below.

**iter-1 fixes:**
- **[MED] M1 — Live Metrics view (`metrics.js`) ignored the per-device threshold.** It reads the same `/api/devices` but computed per-metric status with the *global* threshold only, so slow sensors still showed `Uncertain`/`Bad` there (the exact #132 pain) and the metrics.js/dashboard.js band models — which must stay identical — had diverged. Fixed: `renderDevice` now derives `devStale` (per-device when a valid positive number, else global) and passes it down, lock-step with dashboard.js.
- **[MED] uncertain-band collapse** when a per-device stale nears/exceeds the global bad boundary → both clients now use `devBad = max(globalBad, devStale)` so a large override never mislabels a fresh device as `bad`.
- **[MED] stale PUT hint** — `update_device`'s unknown-field hint still said "PUT accepts only device_name and read_metric_list"; updated to list `stale_threshold_seconds` + document PUT-replace clear semantics.
- **[LOW] config.js parse** — replaced `parseInt` (truncated `100.9`→100; `NaN`→null silent clear) with `Number()` + `Number.isInteger` + range guard that rejects bad input with an error.
- **[LOW] test** — added a re-GET after the clear to confirm the SQLite column was actually nulled (not just the echoed response); **[LOW]** parse-failure log text "non-negative"→"positive".

**iter-2 re-review (mandatory — iter-1 added new band logic):** all five fixes verified correct; metrics.js/dashboard.js per-device paths confirmed byte-identical; config.js band `[1,86400]` == server `(0,86400]` for integers; the test re-GET genuinely reads reloaded config. No new defects.

**Accepted LOW (no patch):** create-vs-update error-shape asymmetry for a malformed threshold (typed extractor gives a generic 400 on create vs a structured 400 on PUT); `BAD_THRESHOLD_SECS` constant reused as the upper bound (named for a different concept but value-correct); pre-existing metrics.js-vs-dashboard.js divergence in the *global* fallback when the server sends a non-positive global (latent, predates G-3); F-4 export/import round-trip of the field verified by code-read (import_replace_all writes the column; `#[serde(default)]` import-safe), no test added.

# Story G.3: Per-Device OPC UA Stale Threshold

Status: ready-for-dev

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

- [ ] **Task 1 — Device CRUD request/response + handlers (AC: 2, 3).** Add optional `stale_threshold_seconds` to the device create + update request structs in `src/web/api.rs` (the `#[serde(default)]` Option<u64> pattern); validate via the existing per-device rule / `clamp_stale_threshold`; include it in the device GET response struct. Surface a clear rejection (structured error body) on an invalid value.
- [ ] **Task 2 — Storage write path (AC: 2).** Extend `insert_device_with_metrics` (`src/storage/sqlite.rs:3548`) and the device-update storage path to write the `devices.stale_threshold_seconds` column (the v012 column already exists; the bulk-migration insert at `sqlite.rs:2579` is the reference for the column write). Ensure `load_all_applications_config` (already reads it) round-trips create→load and update→load.
- [ ] **Task 3 — `/api/devices` per-device field (AC: 4).** Add `stale_threshold_seconds: Option<u64>` to `DeviceView` (`api.rs:462`), populated from the loaded device config; keep the top-level global `stale_threshold_secs`/`bad_threshold_secs`.
- [ ] **Task 4 — Client freshness honors per-device (AC: 4).** Update the dashboard device-freshness band logic (`static/dashboard.js`) to use `device.stale_threshold_seconds ?? global` when computing fresh/stale/bad; keep the existing clamp/fallback guard (120/86400). Mirror in any device freshness shown in the config view if applicable (keep one band model).
- [ ] **Task 5 — Device-detail form field (AC: 1).** In `static/config.js` `mountDeviceDetail` Metrics panel, add an optional numeric "Stale threshold (seconds)" input (prefilled from `dev.stale_threshold_seconds`), included in the existing device `PUT` payload; empty → omit/`null` (global).
- [ ] **Task 6 — Tests (AC: 6).** Update `api_devices_returns_json_with_expected_shape_when_authed` to assert the per-device field; add device-CRUD coverage that a created/updated device round-trips `stale_threshold_seconds` (set, cleared→null, invalid→rejected) through SQLite; client `node --check`. Confirm existing `web_device_crud.rs` / `web_dashboard.rs` pass.
- [ ] **Task 7 — Docs + gates (AC: 6, 7).** Update the LaTeX manual (`docs/manual/latex/body.tex` — note per-device override is now web-editable in the device detail view; the staleness section) + `docs/web-api.md` (device CRUD field + `/api/devices` shape) if present; rebuild the manual PDF. Run full `cargo test`, `cargo clippy --all-targets -- -D warnings`, `node --check`.

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

### Debug Log References

### Completion Notes List

### File List

### Change Log

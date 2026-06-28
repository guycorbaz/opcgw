# Story G.1: Device-Profile Metric Picker

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator adding metrics to a device in the web UI**,
I want to choose from the measurements declared in the device's ChirpStack **device profile** (not only the keys observed in a recent uplink),
so that I can configure metrics for a device that hasn't transmitted a decoded uplink yet — without typing `chirpstack_metric_name` by hand and risking a typo.

GitHub issue: **#124** (milestone #4 — v2.4.0 Web UX & Usability). Builds on Epic C (C-1 inventory query layer, C-2 pickers) and Epic G G-0 (drill-down config UI). Read-only inventory + form pre-fill — **the write path is unchanged** (existing device CRUD staged-apply).

## Acceptance Criteria

1. **Profile-sourced candidates without recent traffic.** For a device whose ChirpStack device profile declares `measurements`, the metric picker in the G-0 device → metrics view lists those measurements as selectable candidates **even when the device has sent no recent decoded uplink** (the case that today shows an empty picker and forces manual entry, per #124).
2. **New inventory endpoint.** `GET /api/inventory/measurements?dev_eui=…` returns the device profile's declared measurements as `{ items:[{ key, name, kind, metric_type }], count, cache_status, dev_eui, device_profile_id, fetched_at }`, mirroring the `/api/inventory/uplinks` envelope, auth-gating, and audit-event pattern. `dev_eui` is validated/normalised with the existing `normalise_dev_eui` (16 hex, colon/dash separators accepted); missing/invalid `dev_eui` → `400` with the same error JSON shape as `inventory_uplinks`.
3. **Profile-id resolution + measurement fetch.** The handler resolves the device's `device_profile_id` from its `dev_eui` (ChirpStack `DeviceService.Get`), then fetches the profile (`DeviceProfileService.Get`) and returns its `measurements` map. The **map key** is the candidate `chirpstack_metric_name`; `Measurement.name` is carried as a human label only (never substituted for the key).
4. **Kind → metric_type inference.** Each measurement's `MeasurementKind` maps to a suggested `metric_type`: `GAUGE → Float`, `COUNTER → Int`, `ABSOLUTE → Int`, `STRING → String`, `UNKNOWN → String` (operator-overridable). Mapping lives in one place with unit tests covering all five variants.
5. **Picker merges both sources, clearly distinguished.** The device → metrics picker offers candidates from **device-profile measurements** (primary) **and** recently-observed uplink keys (secondary), merged and de-duplicated by key, each row visibly tagged with its source (e.g. `profile` vs `observed`). When the same key appears in both, it appears once, tagged as present in both (no duplicate row).
6. **Pre-fill on select.** Ticking a candidate pre-fills `chirpstack_metric_name` with the key and the row's `metric_type` with the inferred type (from `kind` for profile candidates, from `wire_type` for uplink candidates); the operator can override the type per row before saving. Saving still routes through the existing staged-apply device-update path unchanged.
7. **Manual entry preserved.** The manual metric-entry fallback (the existing "Switch to manual metric entry" path) still works unchanged; when both sources are empty the picker degrades to the manual form as it does today for empty uplinks.
8. **Degraded mode, not hard error.** When ChirpStack is unreachable (or the device/profile lookup fails), the endpoint returns a degraded result and the UI shows the existing picker fallback banner (consistent with the C-4 drift view) — never a hard 500 / blank UI. The operator can still configure metrics manually.
9. **Tests + no secret/PII leakage.** Server-side tests cover the new endpoint (happy path with a stubbed profile, `dev_eui` validation, degraded mode, audit-event emission) and the kind→metric_type mapping. The audit event logs `dev_eui` / `device_profile_id` / counts only — **no api_token or decoded payload values** (per `docs/security.md`, matching the `inventory_uplinks` audit). Existing served-asset / DOM-ID assertions remain green. `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean, `node --check` on changed JS.

## Tasks / Subtasks

- [x] **Task 1 — Device-profile gRPC client.** (AC: 3) — Implemented as standalone clients inside `fetch_device_profile_measurements` (the web path uses the standalone `BearerInterceptor` fetchers, NOT the `&mut self` poller methods — same precedent as `fetch_devices`). Added `device_profile_service_client::DeviceProfileServiceClient` + `GetDeviceProfileRequest` + `GetDeviceRequest` imports; all resolve from `chirpstack_api` 4.17.0 (lib compiles clean).
- [x] **Task 2 — Inventory query function + kind mapping (chirpstack_inventory.rs).** (AC: 3, 4) — `ProfileMeasurement` + `DeviceProfileMeasurements` types; `fetch_device_profile_measurements` (DeviceService.Get → device_profile_id → DeviceProfileService.Get → measurements map, sorted by key); `measurement_kind_mapping(i32) -> (kind_label, metric_type)` returning storage-valid type strings. 3 unit tests (all 5 variants + unknown fallback + valid-storage-type invariant).
- [x] **Task 3 — `/api/inventory/measurements` handler (src/web/inventory.rs).** (AC: 2, 8) — `inventory_measurements` mirrors `inventory_devices`: `normalise_dev_eui`, TTL cache via `get_or_fetch_measurements`, `{items,count,cache_status,fetched_at,dev_eui,device_profile_id}` envelope, `inventory_query resource=measurements` audit (ids/counts only), and the established `502 + {error:chirpstack_error,reason}` degraded path (the picker UI turns it into a banner/manual fallback — same pattern devices/uplinks use; chosen over an in-band degraded envelope for consistency, documented here).
- [x] **Task 4 — Route registration (src/web/mod.rs).** (AC: 2) — `.route("/api/inventory/measurements", get(inventory::inventory_measurements))` alongside the siblings; auth-gated (the `requires_auth` test returns 401, proving registration). No external route-enumeration list needed updating.
- [x] **Task 5 — Picker JS API (static/inventory-picker.js).** (AC: 5, 8) — `fetchMeasurements(devEui,{refresh})` added + exported, mirroring `fetchUplinks` (throws Error with `.status` on non-2xx).
- [x] **Task 6 — Device → metrics picker UI (static/config.js).** (AC: 1, 5, 6, 7) — `loadMetricPicker(devEui, opts)` now fetches both sources, merges/dedupes by key into `appendMetricCandidate` rows tagged `profile`/`observed`/`profile + observed`; heading updated; refresh button passes `{refresh:true}`; empty→manual fallback + partial-degraded banner preserved; `readPickerMetrics` reads the unchanged `data-key`/`data-inferred` contract. CSS grid widened to 5 columns + `.metric-source-tag` style in `config.html`.
- [x] **Task 7 — Tests, gates, docs.** (AC: 9) — 3 integration tests in `tests/web_inventory.rs` (auth→401, missing dev_eui→400, invalid dev_eui→400, matching the existing deterministic uplinks-test scope; the live 502 path reuses the already-tested `chirpstack_failure_reason` + envelope). Full `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean, `node --check` clean, LaTeX manual rebuilds clean (exit 0). Doc-sync: README Epic-G row + `docs/manual/latex/body.tex` (§ web pickers).

## Dev Notes

### What exists today (verified 2026-06-28)

- **The gap (#124).** `loadMetricPicker` (`static/config.js:700`) sources candidates *only* from `picker.fetchUplinks(devEui,{limit:10})` → `data.observed_keys` (the `/api/inventory/uplinks` response, which derives keys from `compute_observed_keys` over decoded uplinks). With no recent decoded uplink the list is empty (`config.js:713`) and the UI flips to manual entry. G-1 adds the profile-measurement source so candidates exist regardless of traffic.
- **Inventory substrate (C-1/C-2).** `src/chirpstack_inventory.rs` holds the query layer: `fetch_applications` (821), `fetch_devices` (902), `stream_recent_device_uplinks` (373), `compute_observed_keys` (709), the `WireType` enum + `infer_wire_type` (582/622), and `InventoryCache` (189) with `get_or_fetch_applications/devices` + `invalidate_*`. The web handlers are in `src/web/inventory.rs` (applications/devices/uplinks) and `src/web/drift.rs` (drift, the degraded-mode reference). Routes register in `src/web/mod.rs:551-566`.
- **Mirror target.** `inventory_uplinks` (`src/web/inventory.rs:296`) is the closest existing handler — copy its `dev_eui` validation, config-snapshot read, envelope, and audit `info!` shape. Uplinks are *uncached* (audit always fires); applications/devices use the TTL cache + `?refresh=true`. For measurements (rarely change) the epic prescribes the cached path.
- **Picker JS.** `window.opcgwPicker` (`static/inventory-picker.js:235`) already exports `fetchApplications/fetchDevices/fetchUplinks/auditEvent/mode/editedFlag/warnUnlessAbort`. Add `fetchMeasurements` alongside. The metric picker UI lives in `config.js` `mountDeviceDetail` → `loadMetricPicker`; `METRICS_PAGE_KEY='metrics'` drives the picker/manual mode toggle.

### ChirpStack device-profile API (the new dependency)

- Proto: `proto/chirpstack/api/device_profile.proto` — `service DeviceProfileService { rpc Get(GetDeviceProfileRequest) returns (GetDeviceProfileResponse) }` (line ~113); `message DeviceProfile { map<string, Measurement> measurements = 27; }` (245); `message Measurement { string name = 2; MeasurementKind kind = 3; }` (426); `enum MeasurementKind { UNKNOWN=0, COUNTER=1, ABSOLUTE=2, GAUGE=3, STRING=4 }` (29).
- Crate: `chirpstack_api = 4.17.0` generates `chirpstack_api::api::device_profile_service_client::DeviceProfileServiceClient` + the `GetDeviceProfile{Request,Response}`, `DeviceProfile`, `Measurement`, `MeasurementKind` types from the same bundled proto (opcgw also compiles it in `build.rs`). Use the same `chirpstack_api::api::*` namespace as the existing `device_service_client::DeviceServiceClient` (`src/chirpstack.rs:52`).
- **Profile-id resolution.** `InventoryDevice` / `DeviceListDetail` deliberately drop `device_profile_id` (`chirpstack_inventory.rs:947` sets it empty; the struct only carries `device_profile_name`). So resolve it freshly via `DeviceService.Get(dev_eui)` → `device.device_profile_id`, then `DeviceProfileService.Get(id)`. (A device-list-detail extension to carry the id is an alternative but touches more code; `DeviceService.Get` is the minimal path #124 describes as "the device record".)

### Critical guardrails / gotchas

- **Map key ≠ name.** The `measurements` map *key* is the metric key that becomes `chirpstack_metric_name`. `Measurement.name` is a user-facing label only — never write it into the key field (AC#3).
- **metric_type values must match the device-CRUD validator.** The pre-filled `metric_type` must be one of the strings the existing device-update validation accepts (Float/Int/Bool/String per the storage metric types). Reuse `WireType::as_str()` semantics so picker output round-trips through staged-apply unchanged.
- **No write-path change.** G-1 is read-only inventory + form pre-fill, exactly like C-2. The actual persistence still flows through the existing device create/update staged-apply endpoints (the G-3 / F-0 path). Do not add a new write route.
- **No-aggregation (#130).** Measurements are declarative config, not metric values — fine to expose. Do not start aggregating uplink values.
- **No build step.** Vanilla JS only — no framework, no `node_modules`, no `package.json` (Epic F/G invariant). `node --check` is the JS gate.
- **Degraded mode parity.** Match `src/web/drift.rs` semantics: ChirpStack-unreachable surfaces a banner + manual fallback, not a 500. The picker's existing fallback-banner machinery (`config.js` `setMetricBanner`) is the UI hook.
- **Audit hygiene.** The `inventory_query` audit event must carry identifiers + counts only (mirror `inventory_uplinks` at `src/web/inventory.rs:367`). No `api_token`, no decoded payload values (`docs/security.md`).

### Project Structure Notes

- Backend touch: `src/chirpstack.rs` (new client), `src/chirpstack_inventory.rs` (fetch fn + kind mapping + optional cache method), `src/web/inventory.rs` (handler), `src/web/mod.rs` (route). Frontend: `static/inventory-picker.js`, `static/config.js`. Tests: `src/web/inventory.rs` `#[cfg(test)]`, `src/chirpstack_inventory.rs` `#[cfg(test)]`, plus the relevant `tests/web_*` served-asset suite if a route enumeration assertion exists.
- Naming/conventions: SPDX headers already present on all touched `src/*.rs`; keep the `event=…` structured-logging style; vanilla-JS IIFE module style in `static/*.js`.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Epic G — Story G.1: Device-Profile Metric Picker]
- [Source: GitHub issue #124 — CR: choose metrics from ChirpStack device-profile measurements]
- [Source: src/web/inventory.rs:296 — `inventory_uplinks` (handler to mirror)]
- [Source: src/chirpstack_inventory.rs:582,622,709,902 — WireType / infer_wire_type / compute_observed_keys / fetch_devices]
- [Source: src/chirpstack.rs:736 — `create_device_client` (client pattern to mirror)]
- [Source: proto/chirpstack/api/device_profile.proto:29,113,245,426 — MeasurementKind / Get RPC / measurements map / Measurement]
- [Source: static/config.js:700 — `loadMetricPicker` (UI to extend)]
- [Source: static/inventory-picker.js:235 — `window.opcgwPicker` export (add `fetchMeasurements`)]
- [Source: src/web/drift.rs — degraded-mode reference]
- Previous story intelligence: G-0 (`G-0-drilldown-config-navigation.md`) delivered the hash-routed `config.html`/`config.js` drill-down that hosts this picker; G-3 (`G-3-per-device-stale-threshold.md`) is the most recent device-CRUD touch (per-device field via the same device-update path) — its staged-apply round-trip is the write contract G-1 pre-fills into.

## Dev Agent Record

### Agent Model Used

Opus 4.8 (1M context) — claude-opus-4-8[1m]

### Debug Log References

- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test` — all suites 0-fail (web_inventory 11/0 incl. 3 new measurements tests; web_picker 13/0 no regression; lib kind-mapping 3/0).
- `node --check static/config.js static/inventory-picker.js` — clean.
- `docs/manual/latex/build.sh` — exit 0, no LaTeX errors.

### Completion Notes List

- **Read-only + form pre-fill only** — confirmed NO write-path change. The picker pre-fills `chirpstack_metric_name` + `metric_type`; persistence still flows through the existing device-CRUD staged-apply path untouched.
- **Two chained gRPC calls, cached as one unit.** `fetch_device_profile_measurements` does `DeviceService.Get(dev_eui)` → `device_profile_id` → `DeviceProfileService.Get(id)` → measurements. Cached by `(tenant_id, dev_eui)` so a hit skips BOTH round-trips; the resolved `device_profile_id` rides inside the cached value. `?refresh=true` bypasses.
- **Map key is the metric name** — the `measurements` map key becomes `chirpstack_metric_name`; `Measurement.name` is carried as a display label only (never substituted for the key), per AC#3.
- **Kind→type mapping** returns storage-valid type strings (`Int`/`Float`/`String`) so picker output round-trips through the device-CRUD validator unchanged; a `valid-storage-type` invariant test guards this.
- **Degraded mode** uses the established `502 + {error, reason}` contract (same as devices/uplinks) rather than an in-band degraded envelope — the JS `loadMetricPicker` tolerates either source failing: shows the surviving source + a partial-data banner, or falls back to manual entry with a banner when both fail.
- **Audit hygiene** — `inventory_query resource=measurements` logs `tenant_id`/`dev_eui`/`device_profile_id`/counts/duration only; no api_token, no payload values.
- **Front-end source tags** — merged candidates are de-duplicated by key and each row is tagged `profile` / `observed` / `profile + observed`; the picker grid grew from 4 to 5 columns for the tag.

### File List

- `src/chirpstack_inventory.rs` — imports; `ProfileMeasurement` + `DeviceProfileMeasurements` types; `measurement_kind_mapping`; `fetch_device_profile_measurements`; `MeasurementsCacheMap` + `measurements` cache field + `get_or_fetch_measurements`; 3 unit tests.
- `src/web/inventory.rs` — imports; `InventoryMeasurementsResponse` + `MeasurementsQuery`; `inventory_measurements` handler.
- `src/web/mod.rs` — `/api/inventory/measurements` route.
- `static/inventory-picker.js` — `fetchMeasurements` + export entry.
- `static/config.js` — `appendMetricCandidate` + reworked `loadMetricPicker` (two-source merge); heading text; refresh-button `{refresh:true}`.
- `static/config.html` — `.metric-pick-row` grid (5 cols) + `.metric-source-tag` style.
- `tests/web_inventory.rs` — 3 integration tests for `/api/inventory/measurements`.
- `docs/manual/latex/body.tex` — § web-pickers metric-picker description (two-source) + endpoint enumeration.
- `README.md` — Epic G status row (G-1 in review, G-3 marked done, count 2/5).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — G-1 status.
- `_bmad-output/implementation-artifacts/G-1-device-profile-metric-picker.md` — this story file.

## Change Log

- 2026-06-28 — Implementation complete (all 7 tasks). Device-profile metric picker: new `GET /api/inventory/measurements` endpoint sourcing candidates from ChirpStack device-profile measurements, merged with observed uplink keys in the picker UI. Status ready-for-dev → review.
- 2026-06-28 — Code review (3 adversarial layers Blind/Edge/Auditor on Sonnet + mandatory iter-2). AC#1–8 MET. Loop terminated LOW-only. Status review → done. Addressed review findings (4 patches resolved + 1 MED deferred-as-LOW with Guy's explicit accept).

### Review Findings (2026-06-28)

- [x] [Review][Patch] `GetDeviceProfileResponse.device_profile == None` cached as empty → now returns Err (symmetric with `device == None`) [src/chirpstack_inventory.rs] — edge MED, fixed.
- [x] [Review][Patch] No cancel check between the two chained gRPC calls → added [src/chirpstack_inventory.rs] — blind LOW, fixed.
- [x] [Review][Patch] Missing degraded-mode 502 integration test → added `inventory_measurements_chirpstack_unreachable_returns_502` [tests/web_inventory.rs] — auditor MED, fixed.
- [x] [Review][Patch] `MeasurementsQuery.force_refresh()` had no unit test → added 2 tests [src/web/inventory.rs] — auditor LOW, fixed.
- [x] [Review][Patch] Module doc comment said "Story C-1 … three handlers" → updated to four [src/web/inventory.rs] — auditor LOW, fixed.
- [x] [Review][Defer] Happy-path 200 + audit-event integration tests need a mock ChirpStack gRPC server (no inventory handler has this infra) — **MED accepted-as-LOW by Guy** (success-path logic unit-tested, degraded path now integration-tested, no token in audit scope); see deferred-work.md.
- [x] [Review][Defer] `profile/device not found` → `chirpstack_grpc_error` bucket (classifier vocabulary; full message in audit) — LOW, deferred.
- [x] [Review][Defer] InventoryCache mutex-across-fetch / channel-per-miss / TTL=0 insert — pre-existing systemic pattern across all three cache methods, LOW, deferred.
- [x] [Review][Defer] Picker `fetch*` helpers don't thread `AbortSignal` into `fetch()` — pre-existing pattern, LOW, deferred.
- [x] [Review][Dismiss] "Stale controller not aborted" (blind) — false positive; `loadMetricPicker` aborts the prior controller at its top.

iter-2 re-review (fresh Sonnet agent, on the patch delta): patches clean, 0 HIGH / 0 MED, distinguishes `device_profile=None` (Err) from `Some(empty measurements)` (Ok 200) correctly. Gates: full `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean.

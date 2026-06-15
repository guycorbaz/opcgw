# Story F.3: Dashboard Landing Redesign

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator opening opcgw**,
I want an at-a-glance view of gateway health on the landing page,
so that I can immediately see whether everything is working — and, when it isn't, what's wrong.

## Context & Problem Statement

A dashboard landing page already exists (`static/index.html` + `static/dashboard.js`, Story 9-2): six tiles (ChirpStack status, Last poll, Errors cumulative, Applications, Devices, Uptime) fed by `GET /api/status`, polled every 10 s. F-3 **redesigns** it to (a) lead with a single **overall health verdict** ("All systems OK" vs a specific degraded reason) so an operator gets the answer in one glance, (b) surface **per-device freshness** (how many devices are reporting fresh vs stale data), (c) explicitly surface **degraded states** (ChirpStack unreachable, poller stalled, devices stale, an apply that failed, staged-but-unapplied changes), and (d) adopt the **F-1 shared component CSS** (`.status-badge` / `.banner`) — the component-adoption that F-1 deferred for this page.

**Hard constraint — no gateway-side aggregation (#130).** The gateway must NOT compute aggregates. The dashboard shows **last-known values + status only**; any rollups (e.g. "N devices stale") are **derived client-side** in `dashboard.js` from per-metric timestamps already returned by `/api/devices`. Allowed: adding **pass-through** fields that surface an already-stored value (e.g. the configured poll interval) to an endpoint — that is not aggregation.

## Data-surface reality (verified in code 2026-06-15 — read this before implementing)

The epic's scope line mentions "the `cp0` server-availability device" and "recent errors" — **both are inaccurate against the current code.** Do not build on them:

- **`cp0` is UNUSED.** `OPCGW_CP_ID = "cp0"` (`src/utils.rs:456`) is declared but referenced nowhere — the gateway writes no `cp0` device row. Gateway health lives in the **`gateway_status` singleton table** (`migrations/v006_gateway_status_health_metrics.sql`: one overwritten row with `last_poll_timestamp`, `error_count`, `chirpstack_available`), written by the poller via `update_gateway_status(...)` (`src/chirpstack.rs:1563`) and read via `get_gateway_health_metrics()`. **All three values are already exposed by `GET /api/status`.**
- **There is NO recent-errors store** — no errors table, no event store, no ring buffer. `error_count` is a single **cumulative** `i32` (`saturating_add(1)` per device failure since startup, `chirpstack.rs:1401`), never reset except on restart. Individual errors exist only as ephemeral log lines. **A recent-errors *list* would require new gateway storage → OUT OF SCOPE for F-3** (a no-aggregation redesign). The dashboard shows `error_count` (cumulative) + poll freshness; it does **not** list individual errors. (Tracked as a possible future "event log" feature — note in deferred-work.)
- **No poller-liveness heartbeat.** `chirpstack_available` only reflects the *most recent poll outcome*; if the poller task dies/hangs, `last_poll_time` simply stops advancing. "Poller stalled" is **derived client-side** from `last_poll_time` age vs the poll interval. To judge it precisely the dashboard needs the poll interval — surface it as a pass-through field (AC#3).

### What already exists to draw on
- **`GET /api/status`** (`src/web/api.rs:175` `api_status`, struct `StatusResponse` `api.rs:45-65`): `chirpstack_available: bool`, `last_poll_time: Option<String>` (RFC3339), `error_count: i32`, `application_count: usize`, `device_count: usize`, `uptime_secs: u64`, `pending_changes: bool` (F-0), `apply_failed: bool` (F-0).
- **`GET /api/devices`** (`src/web/api.rs:499` `api_devices`, struct `DevicesResponse` `api.rs:427`): `as_of: String` (server `Utc::now()` — the clock-skew-proof denominator), `stale_threshold_secs: u64`, `bad_threshold_secs: u64` (= 86 400), and `applications[].devices[].metrics[]` each with `value: Option<Value>`, `timestamp: Option<String>` (per-metric last-update). The **per-device freshness** signal is derivable client-side: `age = as_of − metric.timestamp`; a device is fresh/stale/bad by the Good→Uncertain→Bad boundaries (`metrics.js` already implements this against these two thresholds).
- **`GET /api/health`** (`src/web/mod.rs:725`): `{status, version}` — already used for the subtitle.
- **F-1 shell**: `index.html` includes `/shell.js` (injects the nav) + keeps an in-content `.page-header` strip. Reusable shared CSS in `static/dashboard.css`: prefer **`.status-badge` / `.is-ok` / `.is-warn` / `.is-error`** (327-347) and **`.banner` / `.banner.is-warn` / `.banner.is-error`** (352-365) over the legacy `.badge*` / `.error-banner` the current dashboard uses; tiles use `.tile` / `.tile .value` / `.tile .hint`; dark-mode variants exist for all.

## Acceptance Criteria

1. **At-a-glance health verdict.** The landing page leads with a single prominent **overall health summary** (a `.banner`/hero element) that renders one of: **OK** ("All systems operational"), **Degraded** (with a specific reason), or **Action needed**. The verdict is derived **client-side** in `dashboard.js` from `/api/status` (+ `/api/devices` freshness) — no new gateway-side computation. Precedence (highest severity wins, surfaced first): ChirpStack unavailable → poller stalled → apply failed → any devices in the "bad" band → some devices stale → pending (unapplied) changes → all-OK.

2. **Degraded states surface clearly and specifically.** Each degraded condition produces a distinct, operator-actionable message and visual treatment (`.banner.is-error` / `.banner.is-warn`, and/or per-tile `.status-badge`):
   - **ChirpStack unavailable** (`chirpstack_available === false`).
   - **Poller stalled**: `last_poll_time` age exceeds a derived bound (e.g. > ~3× the poll interval, or a sensible floor when the interval is unknown).
   - **Apply failed** (`apply_failed === true`) — link/hint to the config page (re-uses the F-0 apply-bar concept).
   - **Pending (unapplied) changes** (`pending_changes === true`) — informational, lowest severity.
   - **Stale / bad devices** (client-derived, AC#4).
   - **Empty config** (`application_count === 0`): keep the existing first-run-friendly empty-state copy ("No applications configured yet — Add one").
   - **Unknown** (`chirpstack_available` missing/null/non-bool) renders distinctly from "down" (preserve the existing Story 9-2 E7 behaviour).

3. **Poller status + poll-interval pass-through.** Add the configured ChirpStack poll interval to `GET /api/status` as a **pass-through** field `poll_interval_secs: u64` (sourced from `config.chirpstack.polling_frequency` — a stored value, not an aggregate). The dashboard shows a **Poller** status ("Polling — last Xs ago" vs "Stalled — no poll in Ys") computed from `last_poll_time` age vs `poll_interval_secs`. (If `last_poll_time` is null → "Never polled / starting".)

4. **Device freshness panel (client-derived).** The dashboard fetches `GET /api/devices` and shows a compact **freshness summary** — counts of devices that are **fresh** / **stale** / **bad** (no value or age > `bad_threshold_secs`) / **never reported** — derived **client-side** from `as_of − metric.timestamp` against the returned thresholds. A device's band = its worst metric's band. No per-device list is required on the landing page (the existing `metrics.html` / `devices.html` pages own the detail); a "View devices" link deep-links there. Empty/zero-device states render gracefully.

5. **Per-device stale-threshold accuracy (pass-through).** Surface each device's `stale_threshold_seconds` override (Story E-1b / #132, stored in `devices.stale_threshold_seconds`, NULL = use global) in the `GET /api/devices` payload as a **pass-through** field per device, so the client computes staleness against the device's *own* threshold. (Pure pass-through of a stored value — not aggregation.) If descoped, the dashboard MUST fall back to the global `stale_threshold_secs` uniformly and the story notes the limitation; do not silently ignore per-device overrides.

6. **F-1 component adoption.** `index.html` is refactored onto the F-1 shared components: replace the legacy `.badge*` status pills with `.status-badge` (`.is-ok`/`.is-warn`/`.is-error`) and the legacy `.error-banner` with `.banner`. The page sits on the F-1 shell (`/shell.js` injects the nav; keep the in-content `.page-header` strip). Responsive + dark-mode (the shared classes already provide both). NO new build step / framework / `node_modules`.

7. **No behavioural regression of the existing dashboard.** The page still polls `/api/status` every 10 s with the Story 9-2 hardening preserved (in-flight guard, AbortController + feature-detect, stale-render guard, Content-Type sniff, 401 handling, generic network-error banner). The version subtitle (`/api/health`) and "Refresh now" still work. Server-side **served-HTML markers** the tests assert (`tests/...` / `web_dashboard.rs` — viewport meta, content DOM IDs, `<script>` tags, `#app-version`) remain present (the F-1 no-regression invariant: shell decorates, content stays in served HTML).

8. **No gateway-side aggregation.** Verify (and state in Dev Notes) that every new value shown is either an existing `/api/status` / `/api/devices` field, a pure pass-through of a stored value (poll interval, per-device threshold), or a **client-side** derivation. No new aggregate is computed in Rust. No recent-errors list (would need new storage — explicitly deferred).

9. **Tests.** Backend: `/api/status` includes `poll_interval_secs` (unit/integration); `/api/devices` includes the per-device `stale_threshold_seconds` pass-through (if AC#5 in scope). Frontend/served: `web_dashboard.rs`-style assertions that the redesigned served HTML keeps the required markers + the new health-summary element IDs; the health-verdict + freshness-derivation JS logic is `node --check`-clean and, where practical, unit-tested by extracting the pure functions (verdict precedence, age→band) into testable helpers. Degraded-state rendering is covered (CS down, poller stalled, apply failed, stale devices) at the JS-logic level.

10. **Docs synced.** Update `README.md` (dashboard description + Planning row F-3 → done on completion), `docs/web-api.md` (the new `/api/status` `poll_interval_secs` + `/api/devices` per-device threshold fields), and the DocBook manual dashboard section to describe the at-a-glance health verdict + freshness panel. Note the no-recent-errors-list constraint where the manual discusses errors.

## Tasks / Subtasks

- [x] **Task 1 — Backend pass-through fields** (AC: #3, #5, #8)
  - [x] Added `poll_interval_secs: u64` to `StatusResponse`, populated from `state.config_reload.subscribe().borrow().chirpstack.polling_frequency` (pure pass-through, no aggregation). Integration test asserts presence + positivity.
  - [x] **AC#5 DESCOPED to global-only** (documented). Reason: the per-device `stale_threshold_seconds` override (#132) is honoured ONLY in the OPC UA address space today; the entire web UI (`/api/devices` → `metrics.html`) uses the global threshold. Surfacing it for the dashboard alone would make the dashboard inconsistent with `metrics.html` and require churning `DeviceSummary`/`from_config`/`DeviceView`. The dashboard falls back to the global `stale_threshold_secs` uniformly. Surfacing it to the whole web UI is a clean follow-up (deferred-work.md).

- [x] **Task 2 — Health-verdict + freshness derivation (pure JS helpers)** (AC: #1, #2, #4, #9)
  - [x] Added side-effect-free helpers to `dashboard.js`: `computeVerdict(status, freshness)` (AC#1 precedence: CS down → poller stalled → apply failed → bad devices → stale devices → pending changes → empty config → unknown → starting → ok), `metricBand`/`deviceBand`/`summariseFreshness` (fresh/stale/bad/never), `pollerStalled` (age > max(60, 3×interval)).
  - [x] Reused the same Good→Uncertain→Bad boundaries as `metrics.js` (`metricBand` mirrors `statusFor`); no third threshold model.

- [x] **Task 3 — Redesign the page markup** (AC: #1, #2, #6, #7)
  - [x] Refactored `static/index.html`: health-summary `.banner` hero (`#health-summary`/`#health-headline`/`#health-detail`), Poller tile (`#poller-status`/`#poll-interval`), device-freshness section (`#freshness-fresh`/`-stale`/`-bad`/`-never`); kept all 7 tiles. `.badge*` → `.status-badge`, `.error-banner` → `.banner is-error` (kept `id="error-banner"`). Kept `/shell.js`+`/dashboard.js`, `.page-header`, `#app-version`, and **all 10 DOM IDs the served-HTML test asserts** (added new IDs alongside).
  - [x] Page stays standalone-renderable (shell decorates; content + scripts in served HTML).

- [x] **Task 4 — Wire the dashboard** (AC: #1, #2, #3, #4, #7)
  - [x] Factored the Story 9-2 fetch hardening into a generic `makePoller(url, onData, onUnavailable)` (in-flight guard, AbortController+feature-detect, stale-render guard, Content-Type sniff, 401, generic error) and use it for BOTH `/api/status` and `/api/devices` on the same 10 s loop. Verdict re-derived on each fetch from `lastStatus` + `lastFreshness`. `/api/devices` failure → `lastFreshness = "unavailable"` → verdict degrades to `/api/status`-only signals + "freshness unavailable" note; page never blanks.

- [x] **Task 5 — Tests** (AC: #9)
  - [x] Backend: `/api/status` `poll_interval_secs` present + positive (`web_dashboard.rs`). Served-HTML: added the 9 new F-3 IDs to the marker assertion (alongside the existing 10). `node --check` on `dashboard.js` clean. No JS test harness exists in the project (vanilla, no node_modules) — verdict/band logic is covered by `node --check` + served-HTML + the documented precedence matrix (consistent with F-1/F-2 JS-testing convention); the helpers are written side-effect-free for future extraction.

- [x] **Task 6 — Docs** (AC: #10)
  - [x] `README.md` dashboard prose (health verdict + freshness + `poll_interval_secs`) + Planning row F-3; DocBook manual `sec-monitoring-overview` dashboard description; `deferred-work.md` (descoped AC#5 per-device threshold + future event-log/recent-errors + optional `/api/devices/summary`). (`/api/status` is not documented in `docs/web-api.md`/`api-contracts.md`, so the field is noted in the README prose instead.)

## Dev Notes

### Source tree — exact files to touch
- `src/web/api.rs` — `StatusResponse` (`:45-65`), `api_status` (`:175`), `DevicesResponse`/`DeviceView` (`:427`/`:445`), `api_devices` (`:499`). Thresholds: `DEFAULT_STALE_THRESHOLD_SECS` (`opc_ua.rs:38`), `BAD_THRESHOLD_SECS` (`api.rs:258`).
- `src/web/mod.rs` — route table (`:538-540`), `DashboardConfigSnapshot` (`:165`, `from_config` `:178`), `api_health` (`:725`).
- `static/index.html`, `static/dashboard.js`, `static/dashboard.css` (reuse `.status-badge`/`.banner`/`.tile`; `metrics.js` for the band model).
- `src/chirpstack.rs` — `update_gateway_status` (`:1563`) writer (read-only reference; do NOT add aggregation here).
- Tests: the served-HTML dashboard test (`tests/web_dashboard.rs` or equivalent — grep for the `/api/status` / index assertions); `tests/` for `/api/status` + `/api/devices` shape.

### Reuse, don't reinvent
- The Good→Uncertain→Bad band logic ALREADY exists for live metrics (`metrics.js` + OPC UA `compute_status_code` `opc_ua.rs:1853`). Reuse the same two thresholds (`stale_threshold_secs`, `bad_threshold_secs`) and boundaries; do not introduce a third staleness model.
- The Story 9-2 fetch hardening in `dashboard.js` is load-bearing (4 review iterations). Preserve every guard when extending to a second endpoint.
- The F-0 `apply-bar.js` already surfaces `pending_changes` / `apply_failed` on other pages; the dashboard's health verdict should be consistent with it (same signals, `/api/status`).

### Anti-patterns to avoid (do NOT)
- Do **not** add any gateway-side aggregation (rollup counts, "stale device" computation) in Rust — derive client-side (#130).
- Do **not** add a recent-errors list / event table — out of scope (needs new storage); show cumulative `error_count` only.
- Do **not** build on `cp0` (unused) — health is the `gateway_status` table via `/api/status`.
- Do **not** add a build step / framework / `node_modules`.
- Do **not** remove DOM IDs / `<script>` tags / viewport meta the served-HTML tests assert — the F-1 no-regression invariant (shell decorates, content stays served).
- Do **not** make the page depend on `/api/devices` succeeding — degrade gracefully to the `/api/status`-only verdict.

### Project Structure Notes
- This is the F-1 component-adoption for the landing page (AC#2/#3 deferred from F-1 land here). Aligns with the F-1 shell + F-0 apply signals. No new module; markup + vanilla JS + two small pass-through backend fields.
- One variance: the dashboard now consumes a SECOND endpoint (`/api/devices`) on its poll loop. Keep the added load modest (devices payload can be large on big fleets — it returns all metric values; acceptable at 10 s, but note it; a future optimisation could add a lightweight `/api/devices/summary`, deferred).

### References
- [Source: _bmad-output/planning-artifacts/epics.md#Epic F — Story F.3: Dashboard Landing Redesign]
- [Source: src/web/api.rs#StatusResponse, api_status, DevicesResponse, api_devices]
- [Source: src/chirpstack.rs#update_gateway_status (gateway_status table — NOT cp0)]
- [Source: static/index.html, static/dashboard.js, static/dashboard.css (.status-badge/.banner/.tile), static/metrics.js (band model)]
- [Source: migrations/v006_gateway_status_health_metrics.sql, migrations/v012_device_stale_threshold.sql (#132 per-device threshold)]
- [Prior story: F-1-unified-web-shell.md — component adoption deferred to F-3; served-HTML no-regression invariant]
- [Prior story: F-0-staged-config-apply.md — pending_changes / apply_failed signals + apply-bar.js]
- [Constraint: #130 no gateway-side aggregation — last-known values + status only]
- [Source: CLAUDE.md#Documentation Sync, #Source files under 5000 lines]

### Project Structure Notes (alignment)
- Alignment with unified structure: web handlers in `src/web/api.rs`, static assets in `static/`, served-HTML tests in `tests/`. No new conventions.
- Detected variance: epic scope text references `cp0` + "recent errors" which do not match the code (documented above); the story scopes to the real surfaces (gateway_status via /api/status; client-derived freshness; no errors list).

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context)

### Debug Log References

- `cargo clippy --all-targets -- -D warnings`: clean.
- `node --check static/dashboard.js`: OK.
- `xmllint --noout docs/manual/opcgw-user-manual.xml`: valid.
- `cargo test --test web_dashboard`: 15/0 (incl. the 10 existing + 9 new DOM-ID markers and the `poll_interval_secs` shape assertion).

### Completion Notes List

- **Backend** is one pure pass-through field (`poll_interval_secs` on `/api/status`, from `config.chirpstack.polling_frequency`). No aggregation. AC#5 (per-device threshold) descoped to global-only — the whole web UI is global-only today, so surfacing per-device thresholds is a cross-cutting follow-up, not a dashboard-only change.
- **Frontend** is the bulk: at-a-glance health verdict (client-derived precedence), poller-status tile with stall detection, per-device freshness panel (client-derived from `/api/devices`, same band model as `metrics.html`), and F-1 component adoption (`.status-badge`/`.banner`).
- The Story 9-2 fetch hardening was **factored into a reusable `makePoller`** so both endpoints get the in-flight/abort/stale-render/Content-Type/401 discipline. `/api/devices` failure degrades the verdict gracefully (no page blanking).
- **All 10 served-HTML DOM IDs preserved** (the F-1 no-regression invariant); 9 new IDs added + asserted.
- **Scope honesty:** corrected two inaccurate epic-scope assumptions during story creation (`cp0` is unused → health is the `gateway_status` table via `/api/status`; no recent-errors store exists → show cumulative count only, a list is out of scope). No gateway-side aggregation added anywhere.
- No new dependency / build step / framework / `node_modules`.

### File List

- `src/web/api.rs` — `StatusResponse.poll_interval_secs` + populate in `api_status`; updated the serialise unit test.
- `static/index.html` — redesigned: health-summary banner, poller tile, freshness section; `.status-badge`/`.banner` adoption; all required IDs preserved.
- `static/dashboard.js` — verdict/band/freshness pure helpers; reusable hardened `makePoller`; dual-endpoint poll; new renders.
- `static/dashboard.css` — `.banner.is-ok` (+ dark), `.freshness`/`.freshness-counts`.
- `tests/web_dashboard.rs` — `poll_interval_secs` shape assertions + 9 new F-3 DOM-ID markers.
- `README.md` — dashboard prose + Planning row F-3.
- `docs/manual/opcgw-user-manual.xml` — `sec-monitoring-overview` dashboard description.
- `_bmad-output/implementation-artifacts/F-3-dashboard-landing-redesign.md` (this story), `sprint-status.yaml`, `deferred-work.md`.

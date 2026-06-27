# Story G.0: Drill-Down Configuration Navigation

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator configuring opcgw from the web UI**,
I want to navigate my configuration as a hierarchy (Application → Device → Metrics/Commands) instead of three flat, separate pages,
so that I can see and edit the structure of my deployment in context, the way it actually maps to ChirpStack.

**GitHub issue:** [#139](https://github.com/guycorbaz/opcgw/issues/139). **Epic:** G — Web UX & Usability (v2.4.0). **Foundational** — G-1 (device-profile metric picker) and G-2 (contextual field help) build on the device view this story creates.

## Context & Scope Boundary (read first)

Today, configuration is spread across three flat pages that the operator must mentally stitch together:

- `applications.html` — application CRUD (`applications.js`)
- `devices-config.html` — device CRUD **and** per-device metric-mapping (`read_metric_list`) CRUD, via an edit modal (`devices-config.js`)
- `commands.html` — per-device command CRUD (`commands.js`)

G-0 replaces these three with **one hierarchical drill-down**: pick an Application → see/edit its Devices → drill into a Device → see/edit its **Metrics** (the `read_metric_list` mappings) **and** **Commands** in one place.

**CRITICAL scope clarifications (prevent the obvious mistakes):**

- **"Metrics" here = metric *configuration* (`read_metric_list` mappings), NOT the live-values view.** `metrics.html` ("Live Metrics") is a read-only runtime display of current values — it is **out of scope**, stays its own page and its own nav item. Do **not** fold it into the drill-down.
- **In scope to consolidate/retire:** `applications.html`, `devices-config.html`, `commands.html` (+ their `.js`).
- **Out of scope, untouched:** `index.html` (dashboard), `metrics.html` (live values), `singleton-config.html` (gateway settings), `inventory-drift.html` (reconciliation).
- **This is a FRONTEND-ONLY story.** Every CRUD endpoint already exists and **already stages to SQLite via the F-0 staged-apply path** (no inline apply, no restart). **Do NOT add, rename, or change any `/api/*` route, handler, request/response shape, validation, or audit event.** If you find yourself editing `src/web/*.rs` handlers, you have left scope — stop. (Allowed Rust touch: **test-only** updates in `tests/web_dashboard.rs` for the nav contract, see AC-7.)
- **No build step.** Vanilla HTML/CSS/JS only. No `package.json`, no `node_modules`, no framework, no bundler. Assets served as-is by `tower-http ServeDir`.

## Acceptance Criteria

1. **Single drill-down config surface.** A consolidated configuration page (recommended: new `static/config.html` + `static/config.js`) presents the hierarchy **Application → Device → Metrics + Commands**:
   - Level 1 lists applications (with device counts) and supports create / rename / delete.
   - Drilling into an application lists its devices and supports device create / edit / delete.
   - Drilling into a device shows, on one screen, its **Metrics** (read_metric mappings) and **Commands**, each with full create / edit / delete.
2. **Reuses existing endpoints verbatim.** All reads/writes use the current routes exactly as they are today (see Dev Notes §API map). No new endpoint, no changed payload shape. Every write continues to flow through the existing handlers (which stage to SQLite); the operator applies via the existing F-0 **Apply changes** bar.
3. **In-context navigation.** A breadcrumb (e.g. `Applications / <app-name> / <device-name>`) shows the current location and lets the operator navigate back up. The current view is **deep-linkable and reload-safe** via `location.hash` (e.g. `#/app/<application_id>`, `#/app/<application_id>/device/<device_id>`); reloading or sharing a URL returns to the same level. Browser Back/Forward traverses levels.
4. **Nav unification.** `static/shell.js`'s `NAV` array collapses the three separate links (**Applications**, **Devices configuration**, **Commands**) into **one** entry — `{ href: '/config.html', label: 'Configuration' }` — placed where Applications was. Live Metrics, Inventory drift, Singleton config remain. The active-link highlight resolves correctly for the new page.
5. **Old pages don't strand bookmarks.** `applications.html`, `devices-config.html`, `commands.html` are retired. Replace each with a minimal redirect stub to the equivalent `config.html` location (meta-refresh + `location.replace`, deep-linking to the matching level where the URL carried an id) so existing bookmarks/docs still land somewhere sensible. (Stubs may be removed in a later release; redirecting now avoids 404s.)
6. **No regression in the preserved surfaces.** `index.html` and `metrics.html` served-HTML markers, and the `/api/status` + `/api/devices` JSON shapes, are unchanged — the existing pins in `tests/web_dashboard.rs` (dashboard tiles, metrics grid, API shapes) still pass untouched. All `tests/web_application_crud.rs`, `tests/web_device_crud.rs`, `tests/web_command_crud.rs`, `tests/web_duplicate_prevention.rs`, `tests/web_picker.rs` pass unchanged (they exercise the API + picker contracts the new UI still uses).
7. **Nav-contract test updated (the one allowed Rust change).** `tests/web_dashboard.rs::shell_js_is_served_and_owns_the_nav` currently pins the 7 nav labels incl. "Applications" / "Devices configuration" / "Commands". Update **only** that test to assert the new nav set (one "Configuration" label replacing the three; the others intact) and that `config.html` is served with `src="/shell.js"` and no hard-coded `<nav>`. Add a served-HTML marker test for `config.html` (viewport meta + its key container DOM IDs + `src="/config.js"`), mirroring the existing index/metrics marker tests.
8. **Shared patterns reused, not reinvented.** The new page includes and uses the established modules: `shell.js` (nav), `apply-bar.js` (staged-apply affordance — auto-injected, no per-page wiring), `inventory-picker.js` (`window.opcgwPicker`) for create flows + audit, and the `makePoller`/AbortController + `credentials:'include'` + Origin-based-CSRF (`Content-Type: application/json` on mutations) fetch conventions. localStorage picker-mode keys keep their per-context names. Do not duplicate escapeHtml/edited-flag/audit helpers — call `opcgwPicker`.
9. **Behavioural parity.** Everything the three old pages could do is still possible: app create/rename/delete; device create/edit/delete with metric-mapping rows (add/remove); command create/edit/delete (name, fPort 1-223, confirmed, command_class). Duplicate-rejection error bodies (`{error:"duplicate",field,value,scope,hint}`) surface to the operator as they do today. No secret ever rendered.
10. **Quality gates green.** `cargo test` 0-fail (incl. the updated + new `web_dashboard.rs` tests); `cargo clippy --all-targets -- -D warnings` clean; `node --check` on every new/changed `.js`; `xmllint`/well-formedness on new HTML (match the prior stories' gate). No `package.json`/`node_modules` introduced; `git grep -c "<nav"` over `static/*.html` stays 0 (shell owns the nav).

## Tasks / Subtasks

- [x] **Task 1 — Scaffold the drill-down page (AC: 1, 3, 8).** `static/config.html` (shell.js + config.js + apply-bar.js + inventory-picker.js, `.page-header`, `#breadcrumb`, `#config-root`, ported CSS) + `static/config.js` (hash router `#/`, `#/app/:id`, `#/app/:id/device/:id`, breadcrumb, per-render token to cancel stale loads, `hashchange` Back/Forward).
- [x] **Task 2 — Applications level (AC: 1, 2, 9).** `mountApplications`: list + create (opcgwPicker app picker + manual fallback + edited-flag + abort + audit + empty→manual + drift prefill), rename (PUT), delete (DELETE); duplicate/validation error bodies surfaced.
- [x] **Task 3 — Devices level (AC: 1, 2, 9).** `mountDevices` + `buildAddDeviceSection`: device list + add-device (cascading device picker + uplink metric picker, manual fallbacks, edited-flag, abort, audit, C-4 deep-link prefill), delete, Open → device detail.
- [x] **Task 4 — Device detail: Metrics + Commands (AC: 1, 2, 9).** `mountDeviceDetail`: metric-mapping rows (add/remove, save via PUT device) + commands table with create form + dynamic edit dialog (fPort 1-223, confirmed, command_class) + delete.
- [x] **Task 5 — Nav + redirects (AC: 4, 5).** `shell.js` NAV collapsed 3 → "Configuration" → `/config.html`. `applications.html`/`devices-config.html`/`commands.html` replaced with redirect stubs (meta-refresh + `location.replace`, devices-config deep-links to `#/app/:id` when `prefill_app_id` present). Old `*.js` controllers removed.
- [x] **Task 6 — Tests (AC: 6, 7, 10).** `shell_js_is_served_and_owns_the_nav` updated to the new nav set + checks `config.html`; new `config_html_is_served_with_drilldown_markup`; `config.html`/`config.js` added to the test static dir. Retargeted the served-asset tests in `web_application_crud.rs` / `web_device_crud.rs` / `web_command_crud.rs` to `config.html`/`config.js` (necessary because the old pages became redirect stubs — see Completion Notes deviation). `node --check` clean.
- [x] **Task 7 — Docs sync (AC: 10).** LaTeX manual (`docs/manual/latex/body.tex`): drill-down add-flow + nav-bar list + 3 drift/storage references updated; manual PDF rebuilt clean (67 pp). `README.md` config-guidance reference updated.
- [x] **Task 8 — Gates (AC: 10).** `cargo test` full suite exit 0; `cargo clippy --all-targets -- -D warnings` clean; `node --check` on `config.js` + `shell.js` OK; `git grep -c "<nav" static/*.html` == 0; no `package.json`/`node_modules`.

## Dev Notes

### This is a frontend restructure on top of finished backends — reuse, don't rebuild
All Application/Device/Metric/Command CRUD endpoints exist and already **stage to SQLite** (F-0). G-0 changes only how the operator *navigates and assembles* those existing calls. The single biggest failure mode for this story is drifting into `src/web/*.rs` — don't. The only Rust edit is the nav-contract **test** (AC-7).

### API map (reuse exactly — [Source: src/web/mod.rs, src/web/api.rs])
- Applications: `GET /api/applications` (mod.rs:564), `POST /api/applications` (mod.rs:1437), `GET /api/applications/{id}` (mod.rs:1399), `PUT …` (mod.rs:1545), `DELETE …` (mod.rs:1754).
- Devices (nested): `GET /api/applications/{app}/devices` (mod.rs:578), `POST` (mod.rs:2174), `GET …/{dev}` (mod.rs:2122), `PUT` (mod.rs:2396), `DELETE` (mod.rs:2753).
- Commands (nested): `GET …/{dev}/commands` (mod.rs:2908), `POST` (mod.rs:3010), `GET …/{id}` (mod.rs:2963), `PUT` (mod.rs:3385), `DELETE` (mod.rs:3712).
- Inventory pickers (for create flows): `GET /api/inventory/applications` (mod.rs:545), `GET /api/inventory/devices?application_id=` (mod.rs:549). Audit: `POST /api/audit/picker-event` (mod.rs:614).
- Device request shape: `{device_id, device_name, read_metric_list:[{metric_name, chirpstack_metric_name, metric_type, metric_unit?}], device_command_list?:[…]}`. Command shape: `{command_id, command_name, command_port(1..=223), command_confirmed:bool, command_class?}`.
- All mutations: `POST`/`PUT`/`DELETE` require auth + CSRF. CSRF is **Origin-based** (no token header): send `Content-Type: application/json` and `credentials:'include'`; the server validates `Origin` against `[web].allowed_origins`. GET reads are CSRF-exempt.

### Frontend conventions to reuse (do NOT reinvent) — [Source: static/]
- **`shell.js`**: single `NAV` source of truth (lines 26-34); injects `<header class="app-shell">` once (idempotent guard line 45); active link via normalized `location.pathname`. Pages opt in with just `<script src="/shell.js"></script>` and must NOT hard-code `<nav>`.
- **`apply-bar.js`**: auto-injects the staged-apply bar, polls `/api/status` `pending_changes` every 4s, drives `POST /api/config/apply`. Include the script; no per-page wiring. Every config write you make will surface "Unapplied changes" automatically.
- **`inventory-picker.js` (`window.opcgwPicker`)**: `fetchApplications/ fetchDevices/ fetchUplinks`, `mode.get/set(pageKey)` (localStorage `opcgw.picker.{pageKey}.mode`), the **edited-flag heuristic** (`editedFlag.attach/has/reset/recordPickerPopulation` — prevents a picker repopulation from clobbering operator typing; the C-2 iter-3 fix), `auditEvent`, `escapeHtml`, `warnUnlessAbort`. Reuse these for create flows + audit; do not duplicate.
- **`makePoller`/fetch hardening (dashboard.js)**: AbortController per call, in-flight guard, stale-render generation guard, Content-Type sniff, 401 handling. Use the same shape for any polling; use AbortController to cancel in-flight fetches on rapid drill navigation (avoid stale-wins races — same class of bug C-2 fixed for mode toggles).
- All `fetch()` calls include `credentials:'include'`.

### The no-regression contract — [Source: tests/web_dashboard.rs]
- **Untouched pins (must keep passing):** dashboard markers `dashboard_html_contains_viewport_meta_and_status_tiles_markup` (:376, 18 DOM IDs); `dashboard_css_contains_responsive_media_query` (:465); metrics markers `metrics_html_contains_viewport_meta_and_grid_markup` (:1022); API shapes `api_status_returns_json_with_expected_shape_when_authed` (:278) and `api_devices_returns_json_with_expected_shape_when_authed` (:635). G-0 must not touch index/metrics HTML or the `/api/status`,`/api/devices` handlers, so these pass unchanged.
- **The one test you update (AC-7):** `shell_js_is_served_and_owns_the_nav` (:1081) pins the 7 nav labels. Change the assertion set to the new nav (Dashboard, **Configuration**, Live Metrics, Inventory drift, Singleton config) and assert `config.html` includes `src="/shell.js"` with no `<nav>`. Add a `config_html_*` marker test mirroring lines 376/1022.
- **Layer ordering invariant (don't disturb):** security-headers → first-run-gate → auth → csrf → routes+ServeDir (mod.rs:694). New static files are served by the ServeDir fallback automatically.

### Recommended structure (latitude allowed, constraints firm)
A single `config.html` + `config.js` with hash-routed in-page views (`#/`, `#/app/:id`, `#/app/:id/device/:id`), breadcrumb, and three toggled containers is the cleanest no-build drill-down and is the recommended shape. You may instead evolve `applications.html` into the root if that proves simpler — but if so you still must (a) host device + command editing in a device view, (b) update the nav to one "Configuration" entry, (c) redirect the retired pages, (d) keep all pins in AC-6/7. Whatever the shape: shell owns the nav, apply-bar is present, picker helpers are reused, no build step.

### Anti-patterns to avoid (these are the disasters)
- ❌ Touching any `src/web/*.rs` handler / route / payload / validation / audit event. (Test-only edit excepted.)
- ❌ Re-implementing escapeHtml, the edited-flag heuristic, audit posting, or fetch hardening — call `opcgwPicker`/the shared helpers.
- ❌ Hard-coding a `<nav>` in `config.html` (breaks the shell-owns-nav invariant + its grep contract).
- ❌ Folding "Live Metrics" (runtime values) into the config drill-down — different concern, stays separate.
- ❌ Applying config inline / adding a restart — all writes stage; the operator clicks Apply (F-0). Never call `/api/config/apply` from the config page (that's apply-bar's job).
- ❌ Rendering any secret; introducing a bundler/`node_modules`; leaving the old pages as 404s.

### Previous-story intelligence (Epic F — what worked)
- F-1 established the shell-owns-nav pattern + the served-HTML no-regression invariant (shell decorates, pages keep their content/viewport/script markers). G-0 is the first big consumer of that pattern — honour it.
- F-3 factored fetch-hardening into a reusable poller and aligned client band-models; reuse, don't fork.
- F-0 made every write stage; apply-bar is the single apply affordance. The C-2 review caught a "stale-fetch-on-mode-toggle" race — apply the same AbortController discipline to drill navigation.
- Doc-sync (CLAUDE.md): the manual is now LaTeX (`docs/manual/latex/body.tex`, `make pdf`) — update it in the same change set, not the retired DocBook.

### Project Structure Notes
- New files: `static/config.html`, `static/config.js`. Redirect stubs replace `static/applications.html`, `static/devices-config.html`, `static/commands.html`. Old `applications.js`/`devices-config.js`/`commands.js` are superseded by `config.js` — retire them (or keep temporarily only if a redirect stub still references them; prefer clean removal once `config.js` covers their behaviour). Changed: `static/shell.js` (NAV), `tests/web_dashboard.rs` (nav + config markers), `docs/manual/latex/body.tex`, possibly `README.md`/`docs/web-api.md`.
- No Rust production-code changes. No new dependencies.

### References
- [Source: _bmad-output/planning-artifacts/epics.md#Epic G — Story G.0]
- [Source: GitHub #139]
- [Source: src/web/mod.rs (routes/layer-ordering), src/web/api.rs (CRUD handlers)]
- [Source: static/shell.js, static/apply-bar.js, static/inventory-picker.js, static/dashboard.js, static/applications.js, static/devices-config.js, static/commands.js]
- [Source: tests/web_dashboard.rs (served-HTML + API-shape pins), tests/web_application_crud.rs, tests/web_device_crud.rs, tests/web_command_crud.rs, tests/web_duplicate_prevention.rs, tests/web_picker.rs]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context)

### Debug Log References

- Gates: full `cargo test` exit 0 (no failures); `cargo clippy --all-targets -- -D warnings` clean; `web_dashboard` 16/0 (incl. new `config_html_is_served_with_drilldown_markup` + updated nav contract); `web_application_crud` 25/0, `web_command_crud` 55/0, `web_device_crud` 48/0 (retargeted asset tests); `node --check static/config.js static/shell.js` OK; `git grep -c "<nav" static/*.html` all 0; manual PDF rebuilt 67 pp.

### Completion Notes List

- **Frontend-only, as scoped.** Zero `src/web/*.rs` production changes — all CRUD/inventory/apply endpoints reused verbatim (they already stage to SQLite via F-0). The new UI is one hash-routed drill-down (`static/config.html` + `static/config.js`) with three mountable views; the C-2 `opcgwPicker` helpers (modes, edited-flag, abort-on-rapid-action, audit, escapeHtml) are reused, not reinvented, and the hardened behaviours (empty→manual fallback, `recordPickerPopulation`, AbortController stale-fetch guards, C-4 deep-link prefill) are carried across.
- **Per-render token** cancels stale view loads on rapid drill navigation (the drill-level analogue of C-2's mode-toggle abort fix).
- **DEVIATION from AC-7's "one allowed Rust change".** AC-7 anticipated only the nav-contract test would change. In fact the served-asset smoke tests in `web_application_crud.rs` / `web_device_crud.rs` / `web_command_crud.rs` load the now-retired `applications.html`/`.js` etc.; retiring those pages (→ redirect stubs) necessarily breaks them, so they were retargeted to `config.html`/`config.js` (test-only changes tracking the asset move; all assertions preserved in spirit — page served, references its controller, viewport meta). No production code touched. Flagging for the reviewer.
- **Command edit** uses a dynamically-built `<dialog>` (showModal with attribute fallback), preserving the existing modal UX without static markup in `config.html`.
- **Drift deep-links** still work: the `devices-config.html` redirect stub forwards query params and sets `#/app/<prefill_app_id>`; `config.js` consumes `prefill_app_id` / `prefill_dev_eui` / `prefill_name` / `prefill_metric_key`.
- Old controllers `static/{applications,devices-config,commands}.js` removed (superseded by `config.js`; nothing references them).
- **Suggested reviewer focus:** the retargeted CRUD asset tests (deviation above); the drill-down abort/stale-render token; faithful carry-over of the picker edited-flag + prefill paths; that no secret is rendered and CSRF/auth contracts are unchanged (endpoints untouched).

### File List

- `static/config.html` (new) — drill-down page shell
- `static/config.js` (new) — hash router + 3 mountable views (apps / devices / device-detail) + command edit dialog
- `static/shell.js` (modified) — NAV: 3 config links → one "Configuration" → `/config.html`
- `static/applications.html` (modified) — redirect stub → `/config.html`
- `static/devices-config.html` (modified) — redirect stub → `/config.html` (+ `#/app/:id` on prefill)
- `static/commands.html` (modified) — redirect stub → `/config.html`
- `static/applications.js` (deleted), `static/devices-config.js` (deleted), `static/commands.js` (deleted) — superseded by `config.js`
- `tests/web_dashboard.rs` (modified) — static-dir list += config.{html,js}; nav contract updated; new `config_html_is_served_with_drilldown_markup`
- `tests/web_application_crud.rs`, `tests/web_device_crud.rs`, `tests/web_command_crud.rs` (modified) — asset smoke retargeted to config.{html,js}
- `docs/manual/latex/body.tex` (modified) — drill-down add-flow + nav-bar list + drift/storage references
- `README.md` (modified) — config-guidance reference → `/config.html` drill-down

### Change Log

- 2026-06-27: Implemented G-0 — consolidated applications/devices-config/commands flat pages into the `/config.html` drill-down (Application → Device → Metrics/Commands), frontend-only, reusing existing staged-apply endpoints + opcgwPicker. Status → review.

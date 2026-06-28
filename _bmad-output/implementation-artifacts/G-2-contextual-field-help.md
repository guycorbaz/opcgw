# Story G.2: Contextual Field Help

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a **newcomer configuring opcgw from the web UI**,
I want inline contextual help on each configuration field — what it means, the valid format/units, and the consequence of changing it,
so that I can configure the gateway correctly without leaving the browser to read the TOML schema docs.

GitHub issue: **#142** (milestone #4 — v2.4.0 Web UX & Usability). Frontend-only, vanilla, no build step (Epic F/G invariant). Builds on the F-1 shell, the G-0 drill-down config forms, the singleton-config editor, and the first-run setup wizard.

## Acceptance Criteria

1. **Every documented config field carries a help affordance.** Each field in the three config surfaces gets an accessible contextual-help affordance describing (a) what the field means, (b) its expected format / units / valid range, and (c) the consequence of changing it. The surfaces + fields (from #142):
   - **First-run setup wizard** (`static/setup.html`): `server_address`, `tenant_id`, `api_token`, OPC UA `password`.
   - **Singleton-config editor** (`static/singleton-config.js`, rendered into `.field` rows keyed `f-{section}-{key}`): `[global]`, `[chirpstack]` (incl. `polling_frequency`, `retry`, `delay`, `stream_all_devices`, `list_page_size`), `[opcua]` (incl. `host_ip_address`, `host_port`, `user_name`, `pki_dir`, `create_sample_keypair`, `stale_threshold_seconds`, `max_connections`), `[web]` (`port`, `bind_address`, `auth_realm`, `allowed_origins`).
   - **Device / metric / command forms** (`static/config.js`): `metric_type`, `metric_unit`, `chirpstack_metric_name`, `metric_name`, `command_id`, `command_name`, `command_port`, `command_confirmed`, `command_class`, and the per-device `stale_threshold_seconds` override.
2. **Single source of truth for help text.** All help strings live in ONE shared module (`static/field-help.js`, `window.opcgwHelp`) keyed by a stable field key (e.g. `chirpstack.polling_frequency`, `device.stale_threshold_seconds`, `setup.server_address`). No help strings inlined ad-hoc in the three page scripts. The catalog is derived from `docs/configuration.md` (the canonical field descriptions) so the UI and docs don't drift; the module header notes this provenance.
3. **Accessible affordance.** The help is reachable by keyboard and screen reader — not a bare `title=` tooltip. The affordance associates the help text with its input via `aria-describedby` (and, if an info-icon toggle is used, the toggle is a real `<button>` with `aria-label` + `aria-expanded`, operable by Enter/Space, and the revealed text has `role="note"`). Help is also dismissible / non-trapping (Escape or toggle closes a popover; inline hints need no dismissal).
4. **Consistent vanilla component, no build step.** One reusable affordance + its CSS (in `dashboard.css`, the shared component sheet), used identically across all three surfaces. No framework, no `node_modules`, no `package.json`, no new runtime dependency. `static/field-help.js` is plain JS included via a `<script>` tag on each of `config.html`, `singleton-config.html`, and `setup.html` (setup is F-1-shell-excluded, so it needs its own tag — do not rely on `shell.js` injection there).
5. **Optional doc deep-links.** Where useful, a help entry may include a link to the relevant section of the rendered docs / LaTeX user manual (e.g. an `<a target="_blank" rel="noopener">` "Learn more"). Links are optional per field and degrade gracefully if absent.
6. **No regression to the served-HTML / DOM-ID invariant.** The existing server-side served-HTML assertions (`tests/web_dashboard.rs` and the CRUD served-asset tests) must stay green — the help affordance decorates existing fields; it must not relocate content, change asserted DOM IDs, or break the wizard/CRUD round-trips. Adding `<script src="field-help.js">` to the three pages is permitted; update any served-asset test that pins the exact script set for those pages.
7. **Coverage is verifiable.** A check (JS-level and/or a served-asset test) confirms that `field-help.js` is served and linked from each of the three pages, and that every field key the page scripts request help for actually resolves to a non-empty entry in the catalog (no silent missing-help gaps).
8. **Gates green.** `node --check` on all changed/added JS; full `cargo test` 0-fail; `cargo clippy --all-targets -- -D warnings` clean (if any Rust touched — expected none beyond a possible served-asset test); LaTeX manual rebuilds clean if its web-UI section is updated.

## Tasks / Subtasks

- [x] **Task 1 — Help-text catalog + affordance module (`static/field-help.js`).** (AC: 2, 3, 4, 5) — `window.opcgwHelp` IIFE: `HELP` catalog keyed `{section/form}.{field}` (~55 entries from `docs/configuration.md`, provenance noted in header); `affordance(key, inputEl)` returns an accessible info-icon `<button>` toggle (aria-expanded/-controls + Escape) + a `role="note"` region wired via `aria-describedby` (preserving any existing); `attachByData(root)` for static `[data-help]` pages (auto-runs on DOMContentLoaded); `has`/`text`/`_keys`. Unknown key → null + console.warn-once. `docHref` optional "Learn more" link (AC#5).
- [x] **Task 2 — Shared CSS (`static/dashboard.css`).** (AC: 4) — `.field-help` / `.field-help-toggle` / `.field-help-text` component + dark-scheme variants; no layout regression.
- [x] **Task 3 — Setup wizard (`static/setup.html`).** (AC: 1, 4, 6) — added `<script src="/field-help.js">`; converted the 5 fields (`server_address`/`tenant_id`/`api_token`/`password`/`password_confirm`) to `data-help` and **retired the bespoke `.hint` spans** (their detail folded into the catalog) so there is one help mechanism. **`/field-help.js` added to `WIZARD_BYPASS_EXACT`** (src/web/setup.rs) so the first-run wizard can load it (exact-match, same minimal-surface rationale as `/dashboard.css`); `is_wizard_bypass` test updated lock-step.
- [x] **Task 4 — Singleton-config editor (`static/singleton-config.{html,js}`).** (AC: 1, 4, 6) — added `<script>`; `renderField` appends `opcgwHelp.affordance(section + '.' + key, control)` for every rendered field (incl. secrets, anchored to the badge). The injected `<button>`/`<span>` are not matched by `collectSection`'s `input, textarea, select` query → read-back unaffected. Catalog covers all `[global]/[chirpstack]/[opcua]/[web]` keys.
- [x] **Task 5 — Device/metric/command forms (`static/config.{html,js}`).** (AC: 1, 4, 6, 7) — added `<script>`; `fieldHelp`/`appendHelp` helpers; attached to device `stale_threshold_seconds`, the 4 metric fields in `buildMetricRow`, and the 5 command fields. `appendHelp` no-ops on null (no `appendChild(null)`).
- [x] **Task 6 — Tests, gates, docs.** (AC: 6, 7, 8) — added `field-help.js` to the test static-copy list + `field_help_js_is_served_and_pages_reference_it` (served 200 + `window.opcgwHelp` + a 13-key coverage canary spanning every surface + all three pages reference the script). `node --check` clean (field-help/config/singleton-config), full `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean, LaTeX manual rebuilds clean (exit 0). Doc-sync: README Epic-G row + `docs/manual/latex/body.tex` (§ web pickers — "Contextual field help").

## Dev Notes

### What exists today (verified 2026-06-28)

- **Three config surfaces, three rendering styles:**
  1. **`static/setup.html`** — static first-run wizard, `<label for>` + `<input>` pairs; already has a one-off `<span class="hint">Re-type the same password.</span>` (line ~148) — the precedent for an inline hint, to be generalised. The wizard is **excluded from the F-1 shell** (first-run page), so it must include `field-help.js` via its own `<script>` tag, not rely on `shell.js`.
  2. **`static/singleton-config.js`** — `renderField(section, key, value)` (line ~19) builds a `.field` wrapper: `<label htmlFor="f-{section}-{key}">{key}</label>` + an input/select/textarea `id="f-{section}-{key}"` chosen by JSON value type; secrets render a `.field-secret` badge instead. `SECTIONS = ['global','chirpstack','opcua','web']`. The `f-{section}-{key}` id is the stable anchor for `aria-describedby`. `.field label { font-family: monospace }` (in `singleton-config.html`).
  3. **`static/config.js`** — the G-0 drill-down builds device/metric/command forms dynamically via the `el(tag, attrs, children)` helper (line ~44; supports `text`, `class`, `html`, `on*`, and raw attrs). Fields use `el('label', { text: '…' })`; no stable ids, so attach help by passing the affordance node into the form next to each label. Some buttons already use `title:` tooltips (picker refresh) — not a substitute for the accessible affordance.
- **`el()` helper** already supports arbitrary attributes and event handlers, so building/attaching the affordance node is straightforward.
- **Help-text source:** `docs/configuration.md` (599 lines) is the canonical per-field description doc the issue says to keep in sync with. Derive the catalog text from it.

### Design guidance

- **One affordance, one catalog.** The whole point (issue #142) is help *at the point of use* without drift. Put every string in `field-help.js` keyed by `{section/form}.{field}`; never inline help in the page scripts. A single reusable affordance keeps the three surfaces consistent.
- **Accessibility is an AC, not a nice-to-have.** A bare `title=` tooltip is invisible to keyboard and screen-reader users. Use `aria-describedby` to bind the help text to the input; if you use an info-icon toggle, it must be a `<button>` with `aria-label` + `aria-expanded` and Escape-to-close. Inline always-visible hints are the simplest fully-accessible option if toggle complexity isn't worth it — document whichever you pick.
- **No build step.** Vanilla JS/CSS only; `node --check` is the JS gate. CSS for the component goes in `dashboard.css` (shared sheet) so all three pages pick it up.
- **Served-HTML invariant.** `tests/web_dashboard.rs` (and the CRUD served-asset tests) assert specific served-HTML markers (viewport meta, content DOM IDs, `<script>` presence). Adding `<script src="field-help.js">` is fine; just extend any test that enumerates the exact script set rather than letting it fail. Do NOT change existing asserted IDs or relocate field content.
- **Coverage, not silent gaps.** A page that requests help for a field key with no catalog entry must surface it (console.warn + a test), so newly-added fields can't silently ship help-less.

### Project Structure Notes

- New: `static/field-help.js` (catalog + affordance). Touched: `static/setup.html`, `static/singleton-config.{html,js}`, `static/config.{html,js}`, `static/dashboard.css`. Tests: extend `tests/web_dashboard.rs` (served-asset + script-marker) and/or a JS coverage assertion. Likely NO `src/*.rs` change beyond a served-asset test addition.
- Conventions: vanilla IIFE module style (`window.opcgwHelp`, like `window.opcgwPicker` in `inventory-picker.js`); SPDX headers not required on static assets (match existing `static/*.js` which have none). Keep help strings short and operator-oriented.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Epic G — Story G.2: Contextual Field Help]
- [Source: GitHub issue #142 — Add online (contextual) help for each configuration field]
- [Source: docs/configuration.md — canonical per-field descriptions (help-text source of truth)]
- [Source: static/singleton-config.js:19 — `renderField` (singleton field anchor `f-{section}-{key}`)]
- [Source: static/setup.html:122-148 — wizard fields + the existing `.hint` precedent]
- [Source: static/config.js:44 — `el()` helper; device/metric/command form construction ~556-1100]
- [Source: static/inventory-picker.js:235 — `window.opcgwPicker` IIFE-module precedent for `window.opcgwHelp`]
- Previous story intelligence: G-0 (`G-0-drilldown-config-navigation.md`) built `config.html`/`config.js` that host the device/metric/command forms; G-1 (`G-1-device-profile-metric-picker.md`) is the most recent `config.js` touch (metric picker) — both kept the served-HTML DOM-ID invariant, which G-2 must also honour. The setup wizard's shell-exclusion is a documented F-2 decision.

## Dev Agent Record

### Agent Model Used

Opus 4.8 (1M context) — claude-opus-4-8[1m]

### Debug Log References

- `node --check` field-help.js / config.js / singleton-config.js — clean.
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test` — all suites 0-fail (new `field_help_js_is_served_and_pages_reference_it` + the 3 `is_wizard_bypass_*` tests green).
- `docs/manual/latex/build.sh` — exit 0.

### Completion Notes List

- **One catalog, one affordance** — all help text lives in `static/field-help.js`; the three surfaces share it (no inlined strings). Catalog derived from `docs/configuration.md`.
- **Wizard-bypass change (the one non-obvious Rust touch).** The first-run wizard gate is an **exact-match** allowlist (hardened in C-0 iter-2/3 — the old blanket `.js` suffix bypass was deliberately removed). setup.html is otherwise self-contained, so loading the shared `/field-help.js` on the wizard required adding it to `WIZARD_BYPASS_EXACT` (GET-only static asset, same rationale as the existing `/dashboard.css` entry, which also carries the field-help CSS). `is_wizard_bypass_recognises_setup_routes` updated; the suffix-probe rejection tests still hold (exact path added, not a suffix rule).
- **Accessibility** — info-icon `<button>` toggle (Enter/Space/Escape), `aria-expanded`/`aria-controls`, and `aria-describedby` from the input to a `role="note"` region. Not a bare `title=` tooltip.
- **No read-back interference** — the injected affordance nodes (`<button>`/`<span>`) are never matched by `singleton-config.js`'s `collectSection` query (`input, textarea, select`) nor by config.js metric/command read-back, so form submission is unchanged.
- **Setup hints retired** — the bespoke `.hint` spans in setup.html (which duplicated help text outside any catalog — the drift risk #142 calls out) were folded into the catalog and removed.
- **Coverage canary** — a served-asset test asserts a representative key from every surface resolves in the catalog, and that all three pages load the script; `affordance` console.warns once on an unknown key.

### File List

- `static/field-help.js` — NEW. Catalog (~55 entries) + accessible affordance + `attachByData` auto-init.
- `static/dashboard.css` — `.field-help*` component + dark-scheme styles.
- `static/setup.html` — `<script>` + `data-help` on 5 wizard fields; bespoke `.hint` spans removed.
- `static/singleton-config.html` — `<script src="/field-help.js">`.
- `static/singleton-config.js` — `renderField` appends the affordance per field.
- `static/config.html` — `<script src="/field-help.js">`.
- `static/config.js` — `fieldHelp`/`appendHelp` helpers; help on device stale-threshold, metric fields (`buildMetricRow`), command fields.
- `src/web/setup.rs` — `/field-help.js` added to `WIZARD_BYPASS_EXACT` + lock-step test.
- `tests/web_dashboard.rs` — `field-help.js` added to static-copy list + `field_help_js_is_served_and_pages_reference_it`.
- `docs/manual/latex/body.tex` — § web pickers "Contextual field help" paragraph.
- `README.md` — Epic G status row (G-2 in review).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — G-2 status.
- `_bmad-output/implementation-artifacts/G-2-contextual-field-help.md` — this story file.

## Change Log

- 2026-06-28 — Implementation complete (all 6 tasks). Contextual field help across the setup wizard, singleton-config editor, and device/metric/command forms via a shared `static/field-help.js` catalog + accessible affordance. Status ready-for-dev → review.

# Story F.1: Unified Web Shell (vanilla, no build step)

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator using the opcgw web UI**,
I want a consistent navigation, header, and layout across all pages,
so that the gateway feels like one cohesive application rather than a set of separately hand-rolled pages.

Epic F ([#140](https://github.com/guycorbaz/opcgw/issues/140)), story 2 of 5. Recommended order F-0 → **F-1** → F-2 → F-3 → F-4. F-0 (staged config + Apply) is **done**; F-1 builds the shell that hosts F-0's pending-changes/Apply affordance and underpins the visuals of F-2 (wizard) and F-3 (dashboard redesign).

## Context & Critical Findings (read before implementing)

Verified in code 2026-06-14 (F-1 story-creation analysis):

1. **There are 9 hand-rolled static pages, not 8.** `static/`: `index.html`, `applications.html`, `devices-config.html`, `metrics.html`, `commands.html`, `singleton-config.html`, `inventory-drift.html`, `setup.html`, **and `devices.html`** (an 18-line legacy/minimal page the epic's "8 pages" list omits). All 9 must end up consistent. Confirm `devices.html`'s role during implementation (it predates `metrics.html` as the live view) — refactor it onto the shell or, if it is dead, propose removal as a separate decision (do not silently delete — cf. CLAUDE.md).

2. **The nav is duplicated 9×.** Every page hand-writes the identical 7-link `<nav>` (`Dashboard | Applications | Devices configuration | Live Metrics | Commands | Inventory drift | Singleton config`), differing only in which link is wrapped in `<strong>` (the active page). This is the primary duplication F-1 removes. `devices.html` and `setup.html` are not in the nav today.

3. **No build step — vanilla only (Guy's locked decision).** No framework, no `node_modules`, no bundler, no transpile. There is **no `package.json`** in the repo and there must not be one after this story. Static assets are served verbatim by `tower-http::services::ServeDir` (`src/web/mod.rs:71`, `ServeDir::new(static_dir)`). The established DRY mechanism for shared client behaviour with no build step is a **vanilla JS component that injects DOM at load** — exactly the pattern F-0 introduced with `static/apply-bar.js` (self-contained: injects its own `<style>` + DOM on `DOMContentLoaded`). F-1's shell should follow that precedent.

4. **Styling today:** all 9 pages link `/dashboard.css` (8 KB, mobile-first grid + `prefers-color-scheme: dark`). Six pages also carry page-specific inline `<style>` blocks (`applications`, `commands`, `devices-config`, `inventory-drift`, `setup`, `singleton-config`). F-1 consolidates the *common* component styles (buttons, forms, tables, status badges, banners, the nav/header/layout shell) into shared CSS; genuinely page-specific rules may stay inline.

5. **Regression risk — server-side tests assert SERVED HTML.** `tests/web_dashboard.rs` (and setup/CRUD test files) GET pages and assert markers in the **served** HTML: `<meta name="viewport"` (FR41), the per-page status-tile / grid DOM IDs the JS hooks into, and `<script>` presence (e.g. `dashboard_html_contains_viewport_meta_and_status_tiles_markup` at `:358`, `metrics_html_contains_viewport_meta_and_grid_markup` at `:991`). **No test asserts the `<nav>` links in served HTML** (verified by grep). Therefore a shell that injects the **nav** client-side does NOT break these tests — *provided* each page keeps its `<meta viewport>`, its content DOM IDs, and its `<script>` tags in the served HTML. This is the central no-regression invariant: the shell wraps/decorates; it must not move page content out of the served HTML.

6. **F-0 affordance to host.** `static/apply-bar.js` (the pending-changes banner + "Apply changes" button) is currently included on `applications`, `devices-config`, `metrics`, `commands`, `singleton-config`. F-1 should give it a consistent home in the shell (so every config surface shows it uniformly) and ensure the shell layout and the fixed bottom bar do not collide.

**Locked design (Guy, 2026-06-14):** vanilla + shared shell, **NO build step / framework / node_modules**; treat the no-build-step web UI as an asset for an auditable industrial gateway. F-1 and F-2 are fairly independent (could swap order). This story is **visuals/structure only** — no API or behaviour changes.

## Acceptance Criteria

1. **Shared shell component (vanilla, no build step).** A new `static/shell.js` (self-contained vanilla JS, no framework/imports) injects, on every page that includes it, a **unified top navigation + header** (app title/branding + the nav links) and a consistent page layout wrapper. The active nav item is derived at runtime from `location.pathname` (no per-page hard-coding of the active link). No `node_modules`, no bundler, no `package.json` is introduced.

2. **Component CSS.** Shared, reusable CSS classes for the common UI primitives — buttons, form fields, tables, status badges, and banners — plus the nav/header/layout shell, consolidated into shared CSS (extend `static/dashboard.css` and/or add a `static/shell.css`). The styling is **responsive** (works at mobile widths; the nav remains usable on a narrow viewport) and preserves the existing `prefers-color-scheme: dark` behaviour.

3. **All pages refactored onto the shell.** Every operator page — `index`, `applications`, `devices-config`, `metrics`, `commands`, `singleton-config`, `inventory-drift`, `setup`, and `devices` (or a documented decision to retire `devices.html`) — includes `shell.js`, drops its hand-written duplicated `<nav>`, and uses the shared component classes. Page-specific inline `<style>` blocks are reduced to only genuinely page-specific rules (common primitives moved to shared CSS).

4. **No behavioural regression.** Each refactored page keeps, in its **served** HTML, its `<meta name="viewport">` tag, the content DOM IDs its JavaScript hooks into, and its `<script>` tags; all existing API interactions are byte-for-byte unchanged. The full existing `tests/web_*.rs` suite passes unchanged (in particular the served-HTML markup assertions in `web_dashboard.rs` and `web_setup_wizard.rs`).

5. **Hosts the F-0 pending-changes / Apply affordance.** The F-0 `apply-bar.js` pending/Apply affordance is presented consistently across the configuration surfaces via the shell, and the shell layout does not visually collide with the fixed Apply bar. Its behaviour (poll `/api/status`, POST `/api/config/apply`) is unchanged.

6. **No new runtime dependency.** No framework, bundler, transpiler, `package.json`, or `node_modules` is added; no new Rust dependency in `Cargo.toml`. The web UI remains a set of static assets served by `ServeDir`.

7. **Consistency check.** After the refactor, the nav markup exists in exactly **one** place (the shell), not duplicated per page; adding/renaming a nav entry is a one-file change. A reviewer can confirm by grepping `static/` for `<nav` and finding zero (or one shared) literal nav block.

8. **Tests / checks.** Add lightweight coverage that each page still serves its required markers (viewport + script + shell inclusion), that the shell asset is served by `ServeDir`, and that no page-level API contract changed. `cargo test` passes 0-fail; `cargo clippy --all-targets -- -D warnings` is clean. If the DocBook manual screenshots/▸ navigation description change, `xmllint --noout` stays clean.

## Tasks / Subtasks

- [x] **Task 1 — Build the shell component** (AC: 1, 2, 6) — DONE
  - [x] `static/shell.js`: self-contained vanilla IIFE (`'use strict'`, no imports) injects a unified `<header class="app-shell">` (brand + nav) at `document.body` first-child; single `NAV` array; active link from `location.pathname` (`.is-active` + `aria-current="page"`); idempotent guard.
  - [x] Component CSS appended to `static/dashboard.css`: `.app-shell` / `.app-shell__brand` / `.app-shell__nav` (+ active state, responsive `flex-wrap`, dark mode) plus shared `.btn` / `.btn-primary` / `.status-badge` (is-ok/warn/error) / `.banner` primitives. Keeps `prefers-color-scheme: dark`.
  - [x] **Decision (recorded in Completion Notes):** JS injection (DRY, matches `apply-bar.js`, no build step). The shell injects ONLY chrome — page content / DOM IDs / viewport / scripts stay in served HTML (finding 5 honoured).

- [x] **Task 2 — Refactor the simple pages first** (AC: 3, 4) — DONE
  - [x] `index.html`, `metrics.html`, `devices.html`: include `shell.js`, removed the hand-written `<nav>` (kept each page's own `<header class="page-header">` title content, incl. index's `#app-version` so `dashboard.js` is untouched). Viewport + content DOM IDs + scripts intact.
  - [x] `tests/web_dashboard.rs` green after the change.

- [x] **Task 3 — Refactor the config/CRUD pages** (AC: 3, 4, 5) — DONE
  - [x] `applications.html`, `devices-config.html`, `commands.html`, `singleton-config.html`, `inventory-drift.html`: include `shell.js`, removed `<nav>`, kept every DOM ID / dialog / form. (Page-specific inline `<style>` left intact — no behavioural regression; shared primitives are opt-in.)
  - [x] `apply-bar.js` confirmed included on all config surfaces (added to `inventory-drift.html` too, since drift actions stage changes); the fixed bottom bar does not collide with the top shell.
  - [x] Matching `web_*_crud.rs` / `web_singleton_config.rs` / `web_inventory_drift.rs` tests green.

- [x] **Task 4 — Refactor the wizard** (AC: 3, 4) — DONE (by exclusion decision)
  - [x] **Decision:** `setup.html` is intentionally EXCLUDED from the nav shell — it is the first-run, pre-config page and every other page is gated during first-run, so a nav linking to them would be wrong. It keeps its own standalone header + `dashboard.css` styling; no `shell.js`. First-run flow unchanged; `tests/web_setup_wizard.rs` green (it references `/devices.html` as a gated-path example — still served).

- [x] **Task 5 — Consistency + dead-nav sweep** (AC: 7) — DONE
  - [x] `grep '<nav' static/*.html` → **zero** literal navs; the nav lives only in `shell.js`. Adding/renaming an entry is a one-file change.
  - [x] `devices.html`: NOT dead — `tests/web_setup_wizard.rs:178` + `src/web/setup.rs:1013` reference `/devices.html` as a protected-path example, so it must stay. Refactored onto the shell (kept). Retirement remains a possible future cleanup but is out of scope.

- [x] **Task 6 — Tests + gates** (AC: 4, 8) — DONE
  - [x] New `tests/web_dashboard.rs::shell_js_is_served_and_owns_the_nav`: GET `/shell.js` (auth'd) → 200 + contains `app-shell` + all 7 nav labels; `/index.html` + `/metrics.html` include `src="/shell.js"` and contain NO literal `<nav`. Added `shell.js` to the test's production-static-dir copy list.
  - [x] Full `tests/web_*.rs` suite unchanged-green (no test asserted a per-page nav, per finding 5).
  - [x] `cargo test` 0-fail; `cargo clippy --all-targets -- -D warnings` clean; `node --check static/shell.js` OK; no `package.json` / `node_modules` / new Cargo dep.

- [x] **Task 7 — Docs sync** (AC: 8) — DONE
  - [x] DocBook manual § `sec-web-config`: added a paragraph on the unified nav bar + the no-build-step note; `xmllint --noout` clean.
  - [x] `docs/architecture.md`: added a "Static web UI (Story F-1)" note under the `web/` module (shell.js owns the nav; component CSS; wizard excluded; no build step).
  - [x] `README.md` Epic F row: F-1 marked implementation-complete (in review) with the shell summary.

## Dev Notes

- **Hard constraint (Guy, locked):** vanilla JS + CSS only. No framework, no bundler, no transpiler, **no `package.json`, no `node_modules`, no build step.** This is a deliberate property of an auditable industrial gateway. Any proposal that adds a build toolchain is out of scope and must be rejected.
- **Precedent to follow:** `static/apply-bar.js` (Story F-0) is the canonical "self-contained vanilla component injected at `DOMContentLoaded`" pattern — IIFE, `'use strict'`, injects its own `<style>`, polls `/api/status` with `credentials: 'include'`. `shell.js` should match this shape and house (or sit alongside) the apply bar.
- **Central no-regression invariant (finding 5):** the shell DECORATES; it must not relocate page content out of the **served** HTML. Server-side tests assert `<meta name="viewport">`, content DOM IDs, and `<script>` tags in the GET response. Injecting nav/header via JS is fine; moving the dashboard tiles / form fields / table containers into JS injection is NOT (it would break the existing assertions and any future server-side scrape). Keep page content in the static HTML; inject only the shared chrome.
- **Active-link derivation:** drive the active nav item from `location.pathname` in `shell.js` (single nav-definition array), eliminating the per-page `<strong>` hard-coding. This is what makes "add a nav entry = one-file change" (AC#7) true.
- **Static serving:** `src/web/mod.rs:71` `ServeDir::new(static_dir)` serves `static/` verbatim; `static_dir` is also threaded into the first-run wizard path (Epic C C-0 fix, `src/web/mod.rs:308-317`) so `/setup` and the static fallback resolve to the same files. New assets (`shell.js`, optional `shell.css`) are picked up with no routing changes.
- **Cache-busting caveat (deployment_panoramix_nas memory):** `ServeDir` has no cache-busting; operators hard-refresh after an image pull. Not F-1's job to fix, but be aware when manually verifying.
- **`devices.html`:** 18 lines, predates `metrics.html` as the live view. Likely vestigial. Refactor onto the shell for consistency, or raise a one-line retire decision — do not silently delete (CLAUDE.md: look at the target; if it contradicts how it was described, surface it).
- **Scope discipline:** F-1 is structure/visuals only. The dashboard *content* redesign is **F-3**; the wizard *field* expansion is **F-2**. Do not pull that work forward — only restyle `setup.html` and `index.html` onto the shell here.

### Project Structure Notes

- New files: `static/shell.js` (+ optional `static/shell.css`). Modified: all 9 `static/*.html` pages, `static/dashboard.css` (component classes), possibly `static/*.js` only where a page's JS builds DOM that should use shared classes. New/updated tests under `tests/` (a shell-presence test; existing `web_*.rs` adjusted only if needed).
- No Rust source changes expected beyond possibly a test; **no** `Cargo.toml` dependency change; **no** `package.json`.
- Naming: keep the existing kebab-case static asset convention (`shell.js`, `apply-bar.js`, `dashboard.css`).

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story F.1] — scope summary, vanilla/no-build-step constraint, 8-page list (note the 9th, `devices.html`).
- [Source: _bmad-output/implementation-artifacts/sprint-status.yaml#F-1-unified-web-shell] — locked design detail; F-1↔F-2 independence.
- [Source: static/apply-bar.js] — the self-contained vanilla-component precedent (F-0) the shell follows + the affordance to host.
- [Source: src/web/mod.rs:71,308-317] — `ServeDir` static serving + `static_dir` threading into the wizard path.
- [Source: tests/web_dashboard.rs:358,991] — served-HTML markup assertions (viewport + tiles/grid + script) that constrain the shell approach.
- [Source: _bmad-output/implementation-artifacts/F-0-staged-config-apply.md] — F-0 staged-apply + apply-bar.js the shell hosts.
- [Source: CLAUDE.md] — no-build-step ethos; doc-sync + issue-tracking on commit; don't silently delete files.

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (bmad-dev-story)

### Debug Log References

- `cargo test --test web_dashboard --test web_setup_wizard` → 14 + 12 passed after the page refactor (no served-HTML regression).
- `cargo test --test web_dashboard` → 15 passed incl. the new `shell_js_is_served_and_owns_the_nav`.
- Full `cargo test` → 0 failed (37 binaries ok); `cargo clippy --all-targets -- -D warnings` clean; `node --check static/shell.js` OK; `xmllint --noout` clean.

### Completion Notes List

- **Approach = JS-injected shell (no build step).** `static/shell.js` is a self-contained vanilla component (same pattern as F-0's `apply-bar.js`) that injects one `<header class="app-shell">` (brand + nav) at the top of `<body>` and marks the active link from `location.pathname`. This makes the nav a single source of truth: `grep '<nav' static/*.html` now returns **zero** — the 7-link nav that was hand-duplicated across 9 pages is gone.
- **No-regression invariant honoured (story finding 5).** The shell injects only chrome; every page keeps its `<meta viewport>`, content DOM IDs, and `<script>` tags in the **served** HTML, so the server-side markup assertions in `web_dashboard.rs` (and the setup/CRUD tests) still pass unchanged. `index.html` kept its own `#app-version`/title header (renamed to `.page-header`) so `dashboard.js` is untouched.
- **Scope discipline.** Per-page inline `<style>` blocks were left intact rather than aggressively re-skinned — the shared `.btn`/`.status-badge`/`.banner` primitives are provided as opt-in classes. Re-skinning each page's bespoke styles onto the primitives is deferred (would add regression risk for no functional gain); the core value (unified nav/header + component CSS available) is delivered. Deeper visual consolidation can ride F-3 (dashboard redesign).
- **Wizard excluded by decision.** `setup.html` is first-run/pre-config; the other pages are gated then, so it keeps a standalone header with no nav shell.
- **`devices.html` kept** (not retired): two tests reference `/devices.html` as a protected-path example; refactored onto the shell instead.
- **`apply-bar.js`** is now also included on `inventory-drift.html` (drift actions stage config changes), so every config surface shows the F-0 Apply affordance consistently.
- Gates: `cargo test` 0-fail, clippy `-D warnings` clean, `node --check` OK, `xmllint` clean, no `package.json`/`node_modules`/new Cargo dep. Status → review.

### File List

- `static/shell.js` — NEW: unified nav/header shell component (vanilla, injected at load).
- `static/dashboard.css` — `.app-shell*` shell styles + shared `.btn` / `.status-badge` / `.banner` component primitives (+ dark mode).
- `static/index.html`, `static/metrics.html`, `static/devices.html` — drop `<nav>`, keep `.page-header` content, include `shell.js`.
- `static/applications.html`, `static/devices-config.html`, `static/commands.html`, `static/inventory-drift.html` — drop `<nav>`, `.page-header`, include `shell.js` (inventory-drift also gains `apply-bar.js`).
- `static/singleton-config.html` — drop `<nav>`, include `shell.js`.
- `tests/web_dashboard.rs` — add `shell.js` to the production-static-dir copy list; new `shell_js_is_served_and_owns_the_nav` test.
- `docs/architecture.md`, `docs/manual/opcgw-user-manual.xml`, `README.md` — unified-shell docs.
- `_bmad-output/implementation-artifacts/F-1-unified-web-shell.md`, `sprint-status.yaml` — story + status.

### Change Log

- 2026-06-14 — F-1 implemented: unified web shell (`shell.js` + component CSS), 8 operator pages refactored off the duplicated per-page nav, wizard excluded by decision, shell-presence test added, docs synced. Status ready-for-dev → review.

## Review Findings (iter-1, 2026-06-14 — Blind Hunter + Edge Case Hunter + Acceptance Auditor)

Counts: 1 decision-needed (batched), 1 patch, 3 defer (LOW). Both hunters independently raised P1. 6/8 ACs fully met (AC#1/#4/#5/#6/#7/#8); AC#2/#3 partial (the component-system half deferred).

**patch (applied):**

- [x] [Review][Patch] (MED, P1) Doubled dark header bars — FIXED: added `.page-header` rules to `dashboard.css` (transparent bg, inherited text, light `.subtitle`/`.header-controls`/`back-link` contrast) so a page's own header is now an in-content title strip, not a second dark bar. Each page shows exactly one dark bar (the injected `.app-shell`) + a light title. [static/dashboard.css]

**decision-needed (resolved by Guy):**

- [x] [Review][Decision→Defer] (MED, D1) AC#2/#3 component-system scope — RESOLVED: Guy accepted **deferring** the forms/tables primitives + page-wide component-class adoption to **F-3** (dashboard redesign re-skins pages anyway); recorded in `deferred-work.md`. The nav-unification headline goal (the duplicated `<nav>` killed, one vanilla no-build-step shell) is delivered. `setup.html` exclusion **ratified** (first-run page; the other pages are gated then, so a nav to them would be wrong; it had no nav to begin with → no duplication).

**defer (LOW, pre-existing / accepted-limitation):**

- [x] [Review][Defer] (LOW) Subpath/reverse-proxy mount breaks active-link + absolute `/x.html` hrefs — **pre-existing** (the removed hand-written navs used the same absolute hrefs); active-link degrades to "nothing highlighted", not an error. [static/shell.js] → `deferred-work.md`
- [x] [Review][Defer] (LOW) The new Rust test cannot verify the nav actually *renders* (no headless-browser harness under the no-build-step constraint); `apply-bar.js` absent from the test's static-copy list (pre-existing, no assertion depends on it). Accepted limitation. → `deferred-work.md`
- [x] [Review][Defer] (LOW) `singleton-config.html` keeps its `<h1>` in `<main>` (no `.page-header`) vs siblings — minor; largely resolved once P1 makes `.page-header` a light title.

**Loop termination:** the P1 patch is applied; the two MEDIUM AC-gaps (D1) are explicitly user-accepted as deferred-to-F-3 + setup-exclusion-ratified; remaining findings are LOW (deferred). A full iter-2 adversarial re-review is **not** warranted — the only change since iter-1 is a 6-rule declarative CSS title-strip fix (not parser/classifier/flow-control code per the iter-N+1 doctrine), whose sole residual risk is visual and is best verified in a browser, not by an automated layer. `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean, `node --check` OK, `xmllint` clean. Story → `done`.

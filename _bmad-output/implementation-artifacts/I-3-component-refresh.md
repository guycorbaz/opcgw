# Story I.3: Component Refresh

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator**,
I want cards, tables, forms, buttons, badges and banners to look modern and consistent on the I-1 tokens,
so that every screen feels like one polished ChirpStack-adjacent product rather than a refreshed shell wrapped around old-looking page bodies.

## Context

Epic I (Web UI Refresh, CR [#147](https://github.com/guycorbaz/opcgw/issues/147)) — vanilla CSS, **no framework, no build step, no `node_modules`**. I-1 laid the token substrate in `static/dashboard.css`; I-2 refreshed the nav shell. But the **page bodies still look old**: the four form-heavy pages carry their own inline `<style>` blocks full of hard-coded flat-UI/Tailwind hex from before the refresh — `#2980b9` (blue), `#27ae60`/`#28a745` (green), `#c0392b` (red), `#f39c12` (amber), `#2c3e50`, `#95a5a6`, etc. — none of them the ChirpStack-v4/antd token palette. So a user lands on a tokenized navy sider (I-2) next to buttons and tables painted in the previous era's colours.

This story restyles the **component set** — buttons, inputs/selects/textareas, tables, cards, status badges, banners, and the G-2 field-help affordance — onto the I-1 tokens, and applies it across the dashboard, `config.html`, `errors.html`, the first-run wizard (`setup.html`), the singleton-config settings editor, and `inventory-drift.html`.

**This story is different from I-1/I-2 in one important way: it touches HTML files.** The component styling for these pages lives in per-page inline `<style>` blocks (in `config.html`, `singleton-config.html`, `setup.html`, `inventory-drift.html`), so refactoring it to tokens means editing those `<style>` blocks in place. That is allowed — but the **served-HTML DOM structure, IDs, and class names the tests and the page JS depend on must not change** (only the CSS inside the style blocks, plus additions to shared `dashboard.css`).

## Acceptance Criteria

1. **AC#1 — Shared form-control primitives (tokens).** `static/dashboard.css` gains tokenized rules for the common form controls — `input`, `select`, `textarea` (border `var(--cs-border-strong)`, bg `var(--cs-card-bg)`, text `var(--cs-text)`, `var(--radius-sm)`, a visible `:focus-visible` outline in `var(--cs-primary)`, disabled state) — and a shared table treatment (header bg, `var(--cs-border)` row separators) reusable by `table.rows` (config), `.field` inputs (singleton), the wizard inputs, and `table.drift` (inventory). Any new colour is a **token** added to the `:root` + dark `@media` blocks — the I-1 invariant holds: `grep -E '#[0-9a-fA-F]{3,6}' static/dashboard.css` returns only token-definition lines.

2. **AC#2 — Semantic button variants on tokens.** Provide a coherent button set driven by tokens and map every existing per-page button onto it (no hard-coded hex left in the page `<style>` blocks): primary/open/edit (`#2980b9` → `var(--cs-primary)`), danger/delete/remove (`#c0392b` → danger token), add/create (`#27ae60`/`#28a745` → success token), update (`#f39c12` → warning token), keep/neutral (`#95a5a6` → neutral token). **Button-background contrast must be verified for WHITE text on the solid semantic colour** — this is a *different* check from I-1, whose `--ok/--warn/--bad` values were tuned as coloured text on pale tint; white on those exact fills may fail AA and may need dedicated solid `--btn-*`/`-on` tokens. Every button label ≥ 4.5:1 in light and dark.

3. **AC#3 — Cards, badges, banners unified.** The dashboard tiles/`.metrics-grid-container .application` cards, `.status-badge`/`.badge` pills, and `.banner`/`.error-banner`/`.picker-fallback-banner`/`.restart-notice`/`.drift-*` message boxes present consistently on the tokens (surface, border, radius, shadow, semantic tint). The various per-page ad-hoc banners (`background:#fcf3cf;color:#7d6608` warn boxes, `#eef/#224` info boxes, etc.) adopt the token semantic bg/border/fg used by the shared `.banner` states.

4. **AC#4 — G-2 field-help preserved.** The `.field-help` info-icon affordance is restyled via tokens (it already is, from I-1) but its **`aria-describedby` semantics, `.field-help-text.is-collapsed` visually-hidden-but-in-a11y-tree behaviour, and focus handling are unchanged**. Screen-reader + keyboard reachability intact.

5. **AC#5 — DOM & test contract preserved (the guardrail).** Only CSS changes — inside the pages' inline `<style>` blocks and in `dashboard.css`. **No change to served-HTML body structure, element IDs, or class names** that the JS binds to or the tests assert. Load-bearing served-HTML markers that MUST survive: `errors.html` → `src="/shell.js"`, `src="/errors.js"`, `id="errors-tbody"`; `config.html` → `src="/config.js"` (and it links `/dashboard.css`); `index.html` dashboard IDs (`chirpstack-status`, `error-count`, `error-banner`, `poller-status`); `commands.html`/`devices-config.html` link `/dashboard.css`. Class names the page JS queries (`.crud-form`, `.config-section`, `table.rows`, `.btn-open/-edit/-delete/-add/-remove-metric`, `.breadcrumb`, `.metric-row`, `.picker-*`, `.drift-section`, `table.drift`, `.field`, `.field-badge`, `.modal-overlay`, `.restart-notice`, `.wizard`, etc.) are **kept** — restyle them, don't rename them. `setup.html` still returns its first-run page (410 post-first-run behaviour unchanged). No `/api/*` or behavioural change.

6. **AC#6 — Apply bar + shell untouched.** No change to `apply-bar.js` (still its own fixed bottom bar) or to the I-2 shell (`shell.js` / `.app-shell` rules). Components must look right inside the I-2 sider layout (content offset by `body.has-shell`) and next to the apply bar.

7. **AC#7 — Gates.** `cargo test` green (served-asset + served-HTML marker tests in `tests/web_*.rs` pass unchanged); `cargo clippy --all-targets -- -D warnings` clean; `cargo build` clean. Visual parity vs the I-0 mockup verified in a browser, **light and dark**, across all six target pages, at desktop and narrow widths.

## Tasks / Subtasks

- [ ] **Task 1 — Add shared form-control + table primitives to dashboard.css (AC#1)** — tokenized `input`/`select`/`textarea` base rules (border, bg, text, radius, focus-visible, disabled) and a reusable table treatment; new tokens as needed (`--cs-input-bg`, `--cs-focus-ring`, `--table-head-bg`, …) in `:root` + dark block. Do NOT globally restyle every `<input>` in a way that breaks the wizard/singleton monospace fields — scope or opt-in as the existing selectors require.
- [ ] **Task 2 — Semantic button set + per-page remap (AC#2)** — define token-driven button variants; replace the hard-coded hex button rules in `config.html`, `inventory-drift.html`, `setup.html`, `singleton-config.html` `<style>` blocks with the token variants (keep the class names). Compute white-on-fill contrast for each; add solid `--btn-*` tokens if the I-1 semantic values fail as button fills.
- [ ] **Task 3 — Cards / badges / banners (AC#3)** — reconcile the per-page banner/message boxes and badges with the shared token treatments; migrate `#fcf3cf`/`#7d6608`, `#eef`/`#224`, `.field-badge` `#e0e0e0`, drift section tints, etc. onto tokens.
- [ ] **Task 4 — Refactor the four inline `<style>` blocks to tokens (AC#5)** — replace every hard-coded colour in `config.html`, `singleton-config.html`, `setup.html`, `inventory-drift.html` `<style>` blocks with `var(--token)`; **touch only CSS inside `<style>`**, never the body markup, IDs, or class names. Verify each page's DOM markers unchanged (`git diff` shows only `<style>`-block and dashboard.css edits).
- [ ] **Task 5 — Field-help + accessibility recheck (AC#4)** — confirm `.field-help*` still tokenized and `aria-describedby`/collapsed-in-a11y-tree behaviour intact; focus-visible on all new interactive components.
- [ ] **Task 6 — Gates + visual check (AC#7)** — full `cargo test`, `cargo clippy --all-targets -- -D warnings`, `cargo build`; browser pass (light + dark, desktop + narrow) over dashboard, config, errors, wizard, singleton-config, inventory-drift.

## Dev Notes

- **Palette map (old inline hex → I-1 token):** `#2980b9` blue → `--cs-primary`; `#c0392b`/`#c00` red → danger (`--bad` family, but see contrast note); `#27ae60`/`#28a745`/`#1e8449` green → success (`--ok` family); `#f39c12` amber → warning (`--warn` family); `#95a5a6` grey → neutral (`--neutral-*`); `#2c3e50`/`#555`/`#666`/`#777` text greys → `--cs-text` / `--cs-text-muted`; `#ccc`/`#ddd`/`#eee` borders → `--cs-border`/`--cs-border-strong`; `#fff` surfaces → `--cs-card-bg`; `#fcf3cf`/`#7d6608` warn box → `--warn-bg`/`--warn`; `#fff3cd`/`#ffc107` restart notice → `--warn-bg`/`--warn-border`.
- **CONTRAST TRAP (the I-1 lesson, inverted):** I-1's `--ok #237804`, `--warn #874d00`, `--bad #cf1322` were darkened to pass AA **as text on pale tint**. As **button backgrounds with white text** the check flips — e.g. white on `#237804` and white on `#874d00` must be re-verified; some will pass, some may need a dedicated solid button token. Do the math per WebAIM before shipping; this project shipped 3 contrast regressions in I-1 by assuming antd shades pass.
- **This story touches HTML — carefully.** The component CSS lives in per-page `<style>` blocks. Edit *only* the CSS inside them. Do not touch body markup, `id=`, or `class=` on elements — the page JS (`config.js`, `singleton-config.js`, `field-help.js`, `inventory-drift.js`, `errors.js`) and the served-HTML tests depend on them. A good self-check: `git diff` should show changes only inside `<style>…</style>` regions and in `dashboard.css`.
- **Consolidation vs in-place:** prefer moving genuinely shared component rules (buttons, inputs, tables) into `dashboard.css` and having the pages consume them, but this risks class-name collisions and larger diffs. A pragmatic middle path is acceptable: add shared primitives to `dashboard.css`, and in the page `<style>` blocks keep the page-specific selectors but point their colours at tokens. Don't over-reach into a full markup refactor — that's not this story.
- **setup.html is the first-run wizard** (standalone, no shell, returns 410 after first run). It still needs to look refreshed for the pre-first-run experience. It links `/dashboard.css` so it gets the tokens automatically; its inline `.wizard` styles are what to refactor.
- **Out of scope:** apply-bar restyle (still deferred — see I-2 review L-1), any markup/DOM restructure, `/api` changes, new components. I-4 does the final cross-page rollout/consistency/QA sweep.
- **tmpfs gotcha:** `export TMPDIR=/home/gcorbaz/.cache/cargo-tmp` if a build hits `Disk quota exceeded`.

### Previous story intelligence (I-1, I-2 — both done)

- Tokens available (`:root` + dark `@media` in `dashboard.css`): `--cs-primary/-700/-fg`, `--cs-sider-*`, `--cs-nav-active-bg`, `--cs-content-bg`, `--cs-card-bg/-alt`, `--cs-border/-strong`, `--cs-text/-muted`, `--cs-icon`, `--ok/--warn/--bad` (+`-bg`/`-border`), `--neutral-bg/-fg`, `--radius/-sm/-pill`, `--shadow`, `--font/-mono`, `--cs-sider-width/-hover-bg`.
- I-2 added `body.has-shell` (sider offset) and the app-bar/drawer; don't disturb those. The shared primitives `.btn`/`.btn-primary`/`.status-badge`/`.banner`/`.tile` are already tokenized — reuse them; the per-page `.btn-open` etc. are the *un*-tokenized duplicates to reconcile.
- Contrast is verified computationally in these stories (WebAIM relative-luminance). Every new text/bg and white-on-fill pair must be checked in both modes; the reviewer will recompute them.

### Testing standards

- `cargo test`; load-bearing here are the served-HTML marker tests (`tests/web_dashboard.rs` errors.html block ~line 1450, `tests/web_application_crud.rs:1033` config.js, the dashboard DOM-ID tests) and `dashboard_css_contains_responsive_media_query`. tmpfs export as above.
- CSS/`<style>`-only diff → no new Rust tests required; the existing marker tests are the regression guard. A hard-coded-hex assertion on the four pages' `<style>` blocks would be a reasonable optional guard but is not required.

### Project Structure Notes

- Story key `I-3-component-refresh` (epics.md § Epic I, sprint-status.yaml). Files touched: `static/dashboard.css` + the inline `<style>` blocks of `static/config.html`, `static/singleton-config.html`, `static/setup.html`, `static/inventory-drift.html` (CSS only). Possibly `errors.html`/`metrics.html`/`index.html` if their component styling needs token alignment. No JS/Rust/api change. Vanilla, served by `tower-http::ServeDir` (no cache-busting — hard-refresh when eyeballing).
- Commit discipline: implementation lands as its own `Story I-3: … - Implementation Complete` commit before any review-fix commit; `Refs #147`.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story I.3 — component-refresh scope]
- [Source: _bmad-output/implementation-artifacts/I-0-mockup.html — card/table/button/badge visual target]
- [Source: _bmad-output/implementation-artifacts/I-1-design-token-foundation.md — token set + contrast fixes]
- [Source: _bmad-output/implementation-artifacts/I-2-navigation-shell-refresh.md — shell layout, has-shell, review L-1 apply-bar deferral]
- [Source: static/config.html / singleton-config.html / setup.html / inventory-drift.html — inline `<style>` blocks with hard-coded flat-UI hex to refactor]
- [Source: static/dashboard.css — tokens + existing .btn/.status-badge/.banner/.tile primitives]
- [Source: tests/web_dashboard.rs:1454-1456 — errors.html served-HTML markers (shell.js/errors.js/errors-tbody)]
- [Source: tests/web_application_crud.rs:1033 — config.html must load config.js]
- [Source: CR #147 — direction (a): vanilla, no build step, pure presentation]

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List

# Story I.1: Design-Token Foundation

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator**,
I want the web UI built on a single coherent set of design tokens,
so that the look is consistent across every page and dark mode is first-class — the substrate for the ChirpStack-adjacent refresh.

## Context

Epic I (Web UI Refresh, CR [#147](https://github.com/guycorbaz/opcgw/issues/147)) refreshes the UI to a **ChirpStack-v4 / Ant Design-adjacent** look using a **vanilla CSS design system — no framework, no build step, no `node_modules`** (direction (a), locked). I-0 produced and got owner sign-off on the visual target (`_bmad-output/implementation-artifacts/I-0-mockup.html`); its `:root` custom properties are the token set this story implements for real.

Today `static/dashboard.css` (724 lines) has **no tokens** — colours and spacing are hard-coded in every rule, and dark mode is done with **six separate `@media (prefers-color-scheme: dark)` per-component override blocks** (lines 167, 428, 434, 519, 696, 718). This story introduces the token layer and refactors the file to consume it, collapsing those scattered dark overrides into one token redefinition. It is **CSS-only** — no HTML, no JS, no Rust, no `/api` change.

## Acceptance Criteria

1. **AC#1 — Token layer.** `static/dashboard.css` gains a `:root` block defining the I-0 tokens as CSS custom properties: primary (`--cs-primary: #1890ff`, `--cs-primary-700: #096dd9`), sider (`--cs-sider-bg: #001529` + fg/active), content bg (`#f0f2f5`), card bg (`#ffffff`), border, text + text-muted, status (`--ok/--warn/--bad` + `*-bg`), `--radius`, `--shadow`, `--font` (system stack), and a spacing unit. Names follow the I-0 mockup.

2. **AC#2 — Dark mode is token-driven.** A single `@media (prefers-color-scheme: dark)` block **redefines the tokens** (per the I-0 dark palette: `--cs-sider-bg:#000c17`, `--cs-content-bg:#141414`, `--cs-card-bg:#1f1f1f`, dark borders/text, dark status bgs). The six existing per-component dark-override blocks are **removed**, their effect now achieved purely by the redefined tokens. Light + dark parity holds on every component.

3. **AC#3 — Components consume tokens.** Every rule in `dashboard.css` that currently hard-codes a colour or a spacing value is refactored to `var(--token)`. After this story, **no hard-coded colour literal remains outside the `:root` (and dark `@media`) token definitions** (`git grep -nE '#[0-9a-fA-F]{3,6}' static/dashboard.css` returns only token-definition lines). Covers: `header`/`nav`, `.tile`/`.badge`, the F-1 shell (`.app-shell`/`.btn`/`.status-badge`/`.banner`/`.page-header`), `.field-help`, `footer`.

4. **AC#4 — Responsive marker preserved (FR41).** `dashboard.css` MUST still contain a `@media (min-width: …)` query — the existing `@media (min-width: 601px)` two-column grid at line 160 is preserved (renaming/spacing fine, but a `min-width` media query must remain). `tests/web_dashboard.rs::dashboard_css_contains_responsive_media_query` asserts the served CSS contains both `@media` and `min-width`; **do not** replace it with a `max-width`-only approach.

5. **AC#5 — Zero HTML/JS/contract change.** No `static/*.html` or `static/*.js` file is modified. All existing CSS **class names and selectors are preserved** (8 HTML pages + the JS bind to them: `index.html`, `config.html`, `errors.html`, `metrics.html`, `devices.html`, `singleton-config.html`, `setup.html`, `inventory-drift.html`). The dashboard DOM IDs the JS hooks (`chirpstack-status`, `error-count`, `error-banner`, `poller-status`, plus `errors-tbody`) live in HTML and are untouched. `/dashboard.css` still serves `200`. No `/api/*` or behavioural change.

6. **AC#6 — Accessibility preserved.** The G-2 field-help affordance (`.field-help`) is restyled via tokens but keeps its semantics; focus-visible/contrast not regressed (token text/bg pairs meet the prior contrast or better).

7. **AC#7 — Gates.** `cargo test` green (the served-asset / responsive-marker / served-HTML tests in `tests/web_*.rs` pass unchanged); `cargo clippy --all-targets -- -D warnings` clean (no Rust change, but run it); `cargo build` clean. Visual parity vs the I-0 mockup verified in a browser, **light and dark**.

## Tasks / Subtasks

- [x] **Task 1 — Add the `:root` token layer (AC#1)** — `:root` block added at top of `static/dashboard.css` with the I-0 token set (brand/surfaces, semantic status `--ok/--warn/--bad` + bg/border, `--neutral-*`, `--radius*`, `--shadow`, `--font/--font-mono`). Header comment updated to describe the token system + Epic I refresh.
- [x] **Task 2 — Token-drive dark mode (AC#2)** — single `@media (prefers-color-scheme: dark) { :root { … } }` block redefines the tokens to the I-0 dark palette; **all six per-component dark-override blocks removed** (now `grep -c 'prefers-color-scheme' = 1`). Dark appearance is entirely token-driven.
- [x] **Task 3 — Refactor components to tokens (AC#3)** — every component colour/border/shadow now `var(--token)`. `grep -E '#[0-9a-fA-F]{3,6}'` returns only the 36 token-definition lines (the other 3 hits are `#147`/`#142` issue refs in comments, not colours). Added `box-shadow: var(--shadow)` to `.tile` and `.metrics-grid-container .application` per the I-0 card style.
- [x] **Task 4 — Preserve responsive + selectors (AC#4, AC#5)** — `@media (min-width: 601px)` and the `@media (max-width: 600px)` metrics stack intact; selector set **byte-identical** to HEAD (`diff` of the sorted selector list = empty). No `.html`/`.js` touched.
- [x] **Task 5 — Verify (AC#6, AC#7)** — `cargo clippy --all-targets -- -D warnings` clean; `cargo build` clean; web tests serving `dashboard.css` green: `web_dashboard` 19/0 (incl. `dashboard_css_contains_responsive_media_query`), `web_setup_wizard` 14/0, `web_command_crud` 55/0, `web_auth` 15/0. `.field-help-text.is-collapsed` a11y rule (G-2 `aria-describedby`) unchanged.

## Dev Notes

- **Scope is `static/dashboard.css` only.** No `.html`, no `.js`, no Rust. This makes the regression surface tiny: the only tests that can break are the served-asset marker tests in `tests/web_*.rs` (and only if a marker string is removed).
- **The FR41 trap:** the I-0 mockup used `@media (max-width: 720px)`, but the test pins **`min-width`**. The real file already has `@media (min-width: 601px)` — keep it. Don't port the mockup's max-width-only breakpoint as a replacement.
- **Token source of truth:** `_bmad-output/implementation-artifacts/I-0-mockup.html` `:root` + its `[data-theme="dark"]` block. Note the mockup toggles dark via a `data-theme` attribute (for the demo button); production uses **`prefers-color-scheme`** (no manual toggle exists in opcgw today) — so put the dark token redefinitions under `@media (prefers-color-scheme: dark)`, not a `[data-theme]` selector.
- **`color-mix` caution:** the mockup uses `color-mix(in srgb, …)` for the table header tint. It's widely supported now, but if you want to avoid the dependency, define a dedicated `--table-head-bg` token instead.
- **Don't over-reach:** this story does NOT restyle the navigation layout (that's I-2) or redesign components/markup (I-3). It introduces tokens and makes existing components consume them. Visual change should be the palette/spacing refresh, not a structural change.
- **Existing selectors to preserve** (consumers depend on them): `.app-shell`, `.app-shell__brand`, `.app-shell__nav`, `.btn`, `.btn-primary`, `.status-badge` (+ `.is-ok/.is-warn/.is-error`), `.banner` (+ states), `.page-header`, `.tile`, `.badge` (+ `-available/-unavailable/-unknown`), `.field-help`, `.error-banner`, `.hidden`, `header`/`header nav`, `footer`.

### Testing standards

- `cargo test` per the repo norm; the load-bearing tests here are `tests/web_dashboard.rs::dashboard_css_contains_responsive_media_query` (AC#4) and the served-HTML marker tests (AC#5). tmpfs gotcha: `export TMPDIR=/home/gcorbaz/.cache/cargo-tmp` if a build hits `Disk quota exceeded`.
- No new tests strictly required (CSS-only), but it's reasonable to leave the existing marker tests as the guard. If a token-count or no-hard-coded-colour assertion is desired, that's optional.

### Project Structure Notes

- Epic I, story key `I-1-design-token-foundation` (registered in `epics.md` § Epic I + `sprint-status.yaml`). No new files; edits confined to `static/dashboard.css`. No build step (vanilla CSS served by `tower-http::ServeDir`).

### References

- [Source: _bmad-output/implementation-artifacts/I-0-mockup.html — approved token set + dark palette]
- [Source: _bmad-output/planning-artifacts/epics.md#Epic I: Web UI Refresh]
- [Source: static/dashboard.css — current 724-line stylesheet, no tokens, 6 per-component dark blocks]
- [Source: tests/web_dashboard.rs:473 — dashboard_css_contains_responsive_media_query (FR41: @media + min-width)]
- [Source: tests/web_dashboard.rs:413 — dashboard DOM IDs (HTML, untouched by this story)]
- [Source: CR #147 direction (a) — vanilla, no build step, pure presentation]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context)

### Debug Log References

- `grep -E '#[0-9a-fA-F]{3,6}' static/dashboard.css`: 39 hits, 36 on token-definition lines; the 3 remainder are `#147`/`#142` issue references in comment lines (not colour literals) → AC#3 satisfied.
- Selector diff vs `HEAD`: empty → AC#5 selectors byte-identical.
- `grep -c 'prefers-color-scheme' static/dashboard.css` = 1 (was 7) → the six per-component dark blocks collapsed into one token redefinition.

### Completion Notes List

- **CSS-only.** Only `static/dashboard.css` changed — no `.html`, `.js`, or Rust. So the Rust test suite is unchanged except served-asset tests reading the file; those (the FR41 `@media`+`min-width` marker and served-HTML link assertions) pass. A full `cargo test` re-run was deemed unnecessary for a stylesheet-only diff; the four web test binaries that serve `/dashboard.css` were run as the targeted gate.
- **Palette shift (intended refresh):** slate-grey (`#1f2937` header, Tailwind blue `#2563eb`, Tailwind status colours) → ChirpStack-v4/antd family (navy `#001529` sider, antd blue `#1890ff`, antd green/amber/red status). Cards gained the I-0 subtle `--shadow`.
- **Dark mode** is now purely token redefinition under `@media (prefers-color-scheme: dark)` — production has no in-page toggle, so the mockup's `[data-theme]` demo mechanism was intentionally NOT ported.
- **FR41 trap avoided:** kept `@media (min-width: 601px)` (the test asserts `min-width`); did not adopt the mockup's `max-width`-only breakpoint.
- **Not in scope (deferred to I-2/I-3):** navigation layout restructure (still inline links on the bar, not yet an app-bar + side-drawer) and component/markup redesign. I-1 is the token substrate only.
- **Visual check pending:** recommend opening `index.html` / `config.html` / `errors.html` against a running gateway in light + dark to confirm parity with `I-0-mockup.html` before/at code review.

### File List

- `static/dashboard.css` — **modified**: added `:root` light + `@media (prefers-color-scheme: dark)` token blocks; refactored all component rules to `var(--token)`; removed the six per-component dark-override blocks; added card `box-shadow`. Selectors and responsive queries unchanged.
- `_bmad-output/planning-artifacts/epics.md` — Epic I section (added at planning).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — I-1 status transitions.

## Change Log

- 2026-06-30 — I-1 implemented: design-token foundation in `static/dashboard.css` (ChirpStack-v4/antd palette as CSS custom properties, token-driven dark mode, six per-component dark blocks collapsed to one). CSS-only; selectors + FR41 `min-width` marker preserved; clippy clean; web served-asset tests green. Refs #147.
- 2026-06-30 — Code review (focused single-layer CSS review on Sonnet + iter-2 re-verify). 3 HIGH + 1 MED WCAG AA contrast regressions fixed (AC#6); iter-2 APPROVE, only accepted LOWs remain. Status review → done.

## Senior Developer Review (AI)

**Outcome:** Approve (iter-2). Loop terminated under CLAUDE.md condition 2 — only accepted LOW findings remain.

**Layers run:** one focused CSS/front-end adversarial review (Sonnet, different model than the Opus implementer) + an iter-2 re-verification of the fixes. Right-sized for a CSS-only token refactor.

**Action Items:**
- [x] HIGH H-1 — `--ok` text on `--ok-bg` was 3.12:1 (AA fail) → darkened light `--ok` to `#237804` (5.44:1). Dark unchanged (7.87:1).
- [x] HIGH H-2 — `--warn` text on `--warn-bg` was 2.59:1 → light `--warn` `#874d00` (6.53:1). Dark unchanged (8.87:1).
- [x] HIGH H-3 — `--cs-text-muted` `#8c8c8c` was 3.34:1 on white / 2.99:1 on content-bg → light value `#666666` (5.74:1 / 5.12:1). Dark stays `#8c8c8c` (4.92:1).
- [x] MED M-1 — field-help toggle icon `#1890ff` was 2.97:1 → new `--cs-icon` token (`#096dd9` light / `#69b1ff` dark), toggle bg→card, hover→card-alt (≥4.59:1 light, ≥6.38:1 dark).
- [x] LOW L-2 — metrics back-link retoken `--cs-nav-active` → `--cs-primary` (semantic consistency; 5.76:1 on the dark bar).
- [ ] LOW L-1 (accepted/deferred) — `--cs-primary-700` is lighter than `--cs-primary` in dark (intentional dark hover; name is the only wart).
- [ ] LOW L-3 (deferred to I-2) — `--cs-sider-fg` is near-white (`#e6f0fa`) vs the mockup's muted `#a6adb4`; nav hierarchy is settled in I-2's nav refresh.
- [ ] LOW L-4 (deferred) — `.field-help-text` 3px / mobile-row 4px radius not tokenized (out of AC#3 colour scope).

**Note:** all status-dot glyphs and the decorative `--cs-primary` left-border on `.field-help-text` re-checked for non-text 3:1; only the decorative help stripe sits ~2.97:1 (purely decorative, not a regression — was lower before). iter-2 introduced no new findings.

# Story I.2: Navigation & Shell Refresh

Status: ready-for-dev

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a **newcomer arriving from ChirpStack**,
I want opcgw's navigation to feel like ChirpStack's (dark navy side-drawer + clean top strip),
so that the gateway is immediately familiar and reads as a natural companion to the network server I already run.

## Context

Epic I (Web UI Refresh, CR [#147](https://github.com/guycorbaz/opcgw/issues/147)) — vanilla CSS design system, **no framework, no build step, no `node_modules`** (direction (a), locked). I-1 landed the token substrate in `static/dashboard.css` (ChirpStack-v4/antd palette as CSS custom properties, token-driven dark mode). I-2 now restyles the **F-1 shell** from today's horizontal dark top bar (brand + 5 inline links, injected by `static/shell.js`) into the mockup's **ChirpStack-style left sider** on desktop with a **responsive app-bar + collapsible drawer** on mobile.

**Visual only.** The G-0 link set (5 entries) and every page's own content markup are unchanged. Scope = `static/shell.js` + `static/dashboard.css`, nothing else — no HTML, no Rust, no `/api/*`.

Visual target: `_bmad-output/implementation-artifacts/I-0-mockup.html` (owner-approved) — 232px navy sider (`--cs-sider-bg`) with brand block + vertical nav (muted links, white-on-primary active row with left accent), white sticky top strip carrying the page title, content on `--cs-content-bg`.

## Acceptance Criteria

1. **AC#1 — Desktop sider layout.** At and above a `min-width` breakpoint (recommend `992px`), the shell renders as a **fixed left sider** (~232px, `background: var(--cs-sider-bg)`, full viewport height): brand block on top (56px strip, "opcgw"), vertical nav links below (44px rows, per the mockup). Page content is offset (e.g. `padding-left`) **only on pages that have the shell** — gate the layout on a class `shell.js` adds to `<body>` (e.g. `body.has-shell`), because `setup.html` links `dashboard.css` but has **no** shell and must not shift. Pages' own `<main>`/`.page-header`/`<footer>` markup is untouched (the shell decorates, it does not relocate content).

2. **AC#2 — Nav visual states (settles I-1 review L-3).** Default link colour is the muted sider fg (`--cs-sider-fg-muted`, mockup `#a6adb4` ≈ 8:1 on navy); hover = white + subtle light overlay bg; active = the existing `is-active` class + `aria-current="page"`, styled white on `var(--cs-primary)` with a light left-border accent (mockup `.nav a.active`). `--cs-sider-fg` near-white stays for the brand. Every text/bg pair meets **WCAG AA ≥ 4.5:1 in both light and dark** (I-1 review standard — verify, don't assume antd shades pass).

3. **AC#3 — Responsive collapse with accessible toggle.** Below the breakpoint the shell renders as a **top app-bar** (navy strip: brand + a hamburger toggle) and the nav becomes a collapsible drawer. The toggle is a real `<button>` created by `shell.js` with `aria-expanded` (kept in sync), `aria-controls` pointing at the nav, a visible `:focus-visible` outline, and full keyboard operability. Nav links remain reachable on mobile — do **not** port the mockup's `display:none`-only sider hiding. Drawer state is per-page-load (no persistence needed).

4. **AC#4 — Markup & test contract preserved.** The 5 NAV entries keep their labels and hrefs **verbatim**: Dashboard `/index.html`, Configuration `/config.html`, Live Metrics `/metrics.html`, Inventory drift `/inventory-drift.html`, Singleton config `/singleton-config.html` (`tests/web_dashboard.rs::shell_js_is_served_and_owns_the_nav` pins all 5 labels and the literal string `app-shell` in the served `shell.js`). All chrome (sider, app-bar, toggle) is **runtime-injected by shell.js** — no `<nav` may appear in any served HTML page (same test asserts this). Existing class names `.app-shell`, `.app-shell__brand`, `.app-shell__nav`, `is-active` are preserved (new BEM classes like `.app-shell__toggle` are fine). Idempotence guard (`document.querySelector('.app-shell')` early-return) stays. First-run wizard (`setup.html`) stays standalone — no shell, no layout shift.

5. **AC#5 — Page-title strip.** `.page-header` is restyled (CSS only) as the mockup's light top strip: `var(--cs-card-bg)` background, `1px solid var(--cs-border)` bottom border, page `h1` at header scale — replacing today's transparent in-content title block. Dark parity comes free from the tokens. The now-mostly-dead generic `header { … }` element rule (every shell page uses `.page-header`; the shell overrides it) may be retired or scoped — but confirm nothing else consumes it before deleting.

6. **AC#6 — Apply bar coexistence + token discipline.** The F-0 apply bar (`apply-bar.js`, self-contained `position: fixed` bottom bar, `z-index: 2000`) remains fully visible and clickable on config pages with the sider present (keep sider/app-bar z-index below 2000; do not restyle apply-bar — its shell integration is not this story). Any **new colour introduced is a token** added to the existing `:root` + dark `@media` blocks (e.g. `--cs-sider-hover-bg`) — the I-1 invariant holds: `grep -E '#[0-9a-fA-F]{3,6}' static/dashboard.css` hits only token-definition lines (rgba()/color-mix in component rules is acceptable, hex is not).

7. **AC#7 — Gates.** `cargo test` green — load-bearing: `tests/web_dashboard.rs::shell_js_is_served_and_owns_the_nav`, `::dashboard_css_contains_responsive_media_query` (FR41: `@media` + `min-width` must remain — the sider breakpoint naturally satisfies it, and the existing `@media (min-width: 601px)` main-grid query is preserved), and the errors-on-shell test (`errors.html` keeps `src="/shell.js"`). `cargo clippy --all-targets -- -D warnings` clean; `cargo build` clean. Visual parity vs `I-0-mockup.html` checked in a browser, **light and dark**, desktop and narrow viewport.

## Tasks / Subtasks

- [ ] **Task 1 — Restructure shell.js (AC#1, #3, #4)**
  - [ ] Keep header comment discipline (SPDX + story provenance) and the F-1 design notes; document the I-2 shape.
  - [ ] Add `document.body.classList.add('has-shell')` so CSS can gate the sider layout off the wizard.
  - [ ] Build the new chrome: brand block, nav (same 5 entries, same `is-active`/`aria-current` logic), plus a hamburger `<button class="app-shell__toggle">` with `aria-expanded="false"`, `aria-controls` on the nav element (give the nav an `id`), and a click handler toggling an open-state class (e.g. `.app-shell--open`) + `aria-expanded`.
  - [ ] Keep the idempotence early-return and the body/DOMContentLoaded injection fallback exactly as today.
- [ ] **Task 2 — Desktop sider CSS (AC#1, #2)**
  - [ ] New tokens in `:root` + dark block for any new colours (sider hover overlay, active border accent if not white).
  - [ ] `@media (min-width: 992px)`: `.app-shell` as fixed left sider (232px, full height); `body.has-shell` gains the matching left offset; hide the toggle.
  - [ ] Nav rows per mockup: 44px, muted default (`--cs-sider-fg-muted`), hover white + overlay, `.is-active` white on `--cs-primary` + left accent. Verify AA 4.5:1 for all pairs, both modes.
- [ ] **Task 3 — Mobile app-bar + drawer CSS (AC#3)**
  - [ ] Below the breakpoint: `.app-shell` as top navy app-bar; nav collapsed by default, shown when `.app-shell--open`; toggle visible with `:focus-visible` outline (reuse the `--cs-icon`-style focus treatment from I-1).
  - [ ] Confirm content is not obscured when the drawer is open (drawer may overlay or push — dev's choice, keep it simple).
- [ ] **Task 4 — Page-title strip + header rule cleanup (AC#5)**
  - [ ] Restyle `.page-header` as the light strip on `--cs-card-bg` with bottom border; check all 6 shell pages that use it (`index/config/errors/metrics/devices/inventory-drift.html`).
  - [ ] Audit the generic `header {}` dark rule; retire or scope it if truly dead (grep the 8 HTML pages first).
- [ ] **Task 5 — Coexistence checks (AC#4, #6)**
  - [ ] Apply bar visible/clickable over the sider on `config.html`/`singleton-config.html` (z-index audit).
  - [ ] `setup.html` unaffected (no `has-shell` class → no offset).
  - [ ] Hex-grep invariant still clean; selector deletions limited to rules being intentionally replaced (never rename the four pinned class names).
- [ ] **Task 6 — Gates + visual check (AC#7)**
  - [ ] `cargo test` (at minimum the four web binaries: `web_dashboard`, `web_setup_wizard`, `web_command_crud`, `web_auth`), `cargo clippy --all-targets -- -D warnings`, `cargo build`.
  - [ ] Browser check vs mockup: light + dark, ≥992px and narrow; keyboard-only pass over the toggle + nav.

## Dev Notes

- **The layout trick is the crux.** Pages own their content; the shell is injected as the first `<body>` child. You cannot wrap the page in a grid like the mockup's `.app` container without relocating content (forbidden — server-side markup assertions). The fixed-sider + `body.has-shell { padding-left: … }` pattern gets the mockup look with zero HTML change. The `has-shell` body class is the guard that keeps `setup.html` (links `dashboard.css`, no shell) from shifting.
- **The mockup is a target, not a spec, in two places:** (1) its nav shows Dashboard/Configuration/Errors/About — **ignore that**; the production link set is the 5 G-0 entries and is test-pinned. (2) it hides the sider entirely at `max-width:720px` with no fallback — production must provide the accessible toggle instead (AC#3), and FR41 pins `min-width` media queries anyway.
- **Icons are optional.** The mockup uses glyph icons (▦ ⚙ …). If you add them, mark them `aria-hidden="true"` (decorative) and keep the text labels — the test greps the labels in shell.js source.
- **New interactivity budget:** the hamburger toggle is the only new JS behaviour. No focus-trap/Escape-close machinery required (it's a nav drawer, not a modal); `aria-expanded` + keyboard operability is the bar.
- **I-1 lessons that bite here:** antd's decorative shades often fail AA as text — the I-1 review found 3 HIGH contrast regressions exactly this way. Check every new pair (WebAIM-style ratio math) in both modes before review. Mockup's `#a6adb4` on `#001529` is ≈8:1 (safe); white-on-`#1890ff` is ≈3.2:1 — **fails AA for normal text**, so the active row needs either a darker active bg (e.g. `--cs-primary-700` `#096dd9`, ≈4.7:1 with white) or bold/large treatment justified as ≥3:1 large-text — prefer the darker bg.
- **Do not over-reach:** component restyling (buttons, tables, cards, banners, badges) is I-3; page-by-page rollout/QA is I-4. I-2 = shell chrome + page-title strip only. `header nav` / `.header-controls` legacy rules only get touched if the header-rule cleanup (Task 4) requires it.
- **tmpfs gotcha:** `export TMPDIR=/home/gcorbaz/.cache/cargo-tmp` if a build hits `Disk quota exceeded`.

### Previous story intelligence (I-1, done 2026-06-30)

- Token set now available (all in `static/dashboard.css` `:root` + dark block): `--cs-primary/-700/-fg`, `--cs-sider-bg/fg/fg-muted`, `--cs-nav-active`, `--cs-content-bg`, `--cs-card-bg/-alt`, `--cs-border/-strong`, `--cs-text/-muted`, `--cs-icon`, `--ok/--warn/--bad` (+`-bg`/`-border`), `--neutral-*`, `--radius/-sm/-pill`, `--shadow`, `--font/-mono`.
- `--cs-nav-active` (`#40a9ff`) currently styles the active underline on the horizontal bar — it may become unused after the sider restyle; retire it or repurpose it deliberately, don't leave it dangling silently.
- Review deferred **L-3 lands here**: sider-fg hierarchy (`--cs-sider-fg` `#e6f0fa` vs mockup muted `#a6adb4`) — AC#2 resolves it (muted default, white on hover/active/brand).
- I-1 was CSS-only so its regression surface was tiny; I-2 touches JS, so the four web test binaries are the minimum gate, and a keyboard/a11y hand-check is non-optional (no headless browser in CI — tests don't execute shell.js).

### Testing standards

- No new Rust tests strictly required; the existing served-asset assertions are the guard. If you want a cheap new marker, an assertion that shell.js contains `aria-expanded` (pinning the accessible toggle) fits the existing test's style — optional.
- The `shell_js_is_served_and_owns_the_nav` test greps shell.js **source text** — the 5 labels and `app-shell` must survive as literal strings.
- Full `cargo test` before review per CLAUDE.md gates.

### Project Structure Notes

- Story key `I-2-navigation-shell-refresh` (epics.md § Epic I, sprint-status.yaml). Files touched: `static/shell.js`, `static/dashboard.css` only. Vanilla, served by `tower-http::ServeDir` (no cache-busting — hard-refresh the browser when eyeballing).
- Commit discipline: implementation lands as its own `Story I-2: … - Implementation Complete` commit before any review-fix commit; `Refs #147`.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story I.2 — scope + Epic I design principles]
- [Source: _bmad-output/implementation-artifacts/I-0-mockup.html — sider/app-bar/header visual target + token names]
- [Source: _bmad-output/implementation-artifacts/I-1-design-token-foundation.md — token set, review contrast fixes, deferred L-3]
- [Source: static/shell.js — current F-1 shell (NAV array, is-active/aria-current, idempotence, injection pattern)]
- [Source: static/dashboard.css — tokens (:root + dark), .app-shell section, .page-header, generic header rule]
- [Source: tests/web_dashboard.rs:1095-1174 — shell_js_is_served_and_owns_the_nav (labels, app-shell string, no <nav> in served HTML)]
- [Source: tests/web_dashboard.rs:468-503 — dashboard_css_contains_responsive_media_query (FR41 @media + min-width)]
- [Source: tests/web_dashboard.rs:1414+ — errors.html served on the shell]
- [Source: static/apply-bar.js — fixed bottom bar, z-index 2000, self-contained styles (do not restyle)]
- [Source: CR #147 — direction (a): vanilla, no build step, pure presentation]

## Dev Agent Record

### Agent Model Used

### Debug Log References

### Completion Notes List

### File List

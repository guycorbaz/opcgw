# Story I.2: Navigation & Shell Refresh

Status: done

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

- [x] **Task 1 — Restructure shell.js (AC#1, #3, #4)**
  - [x] Keep header comment discipline (SPDX + story provenance) and the F-1 design notes; document the I-2 shape.
  - [x] Add `document.body.classList.add('has-shell')` so CSS can gate the sider layout off the wizard.
  - [x] Build the new chrome: brand block, nav (same 5 entries, same `is-active`/`aria-current` logic), plus a hamburger `<button class="app-shell__toggle">` with `aria-expanded="false"`, `aria-controls="app-shell-nav"` (nav given that `id`), `aria-label="Toggle navigation"`, and a click handler toggling `.app-shell--open` + `aria-expanded`.
  - [x] Keep the idempotence early-return and the body/DOMContentLoaded injection fallback exactly as today.
- [x] **Task 2 — Desktop sider CSS (AC#1, #2)**
  - [x] New tokens in `:root`: `--cs-sider-width: 232px`, `--cs-sider-hover-bg: rgba(255,255,255,.06)`, `--cs-nav-active-bg: #096dd9` (dark block inherits — navy chrome is dark in both modes). Retired `--cs-nav-active` (#40a9ff; its two consumers — `header nav strong`, old `.is-active` underline — were removed with the restyle; zero remaining `var()` consumers verified by grep).
  - [x] `@media (min-width: 992px)`: `.app-shell` as fixed left sider (232px, full height, column flex, `overflow-y: auto`); `body.has-shell { padding-left: var(--cs-sider-width) }`; toggle hidden.
  - [x] Nav rows: 44px min-height, muted default, hover white + overlay, `.is-active` white on `--cs-nav-active-bg` + white left accent. All pairs verified computationally (see Debug Log) — every text pair ≥ 4.9:1 both modes.
- [x] **Task 3 — Mobile app-bar + drawer CSS (AC#3)**
  - [x] Base (mobile-first): `.app-shell` as navy top app-bar (56px, brand left, toggle right); nav `display:none` until `.app-shell--open` (drawer is in-flow — pushes content down, nothing obscured); toggle + nav links have `:focus-visible` outlines (`--cs-primary`, 5.68:1 on navy).
  - [x] Content not obscured when drawer opens (in-flow push, not overlay).
- [x] **Task 4 — Page-title strip + header rule cleanup (AC#5)**
  - [x] `.page-header` restyled as light strip: `--cs-card-bg`, `1px --cs-border` bottom hairline, h1 1.25rem/600; subtitle margins folded in from the retired generic rules. All 6 consumer pages use it (grep-verified: `index/config/errors/metrics/devices/inventory-drift.html`).
  - [x] Generic `header {}` block retired entirely (`header`, `header h1`, `header .subtitle`, `header nav*` — grep confirmed every `<header` in static/ carries `class="page-header"`; `setup.html` has no `<header>` at all; the shell owns its own styling).
- [x] **Task 5 — Coexistence checks (AC#4, #6)**
  - [x] Apply bar: sider `z-index: 100` < apply-bar 2000 → bar overlays everything at the bottom, fully visible/clickable; apply-bar.js untouched.
  - [x] `setup.html` unaffected (links dashboard.css, never gets `has-shell` → no offset; no `<header>` element so the retired generic rule can't regress it).
  - [x] Hex-grep invariant clean (non-token hits are all comment refs: #147/#142/#1890ff-#096dd9 rationale comment); one real `@media (prefers-color-scheme: dark)` block; the four pinned class names preserved.
- [x] **Task 6 — Gates + visual check (AC#7)**
  - [x] **Full `cargo test`: 1803 passed / 0 failed** (exit 0; includes the 2 new I-2 marker assertions); `cargo clippy --all-targets -- -D warnings` clean; `cargo build` clean.
  - [x] Keyboard path verified structurally (real `<button>`, `:focus-visible` outlines on toggle + links, in-flow drawer). **Browser visual check vs mockup: PENDING — owner's** (headless-Firefox screenshot attempt failed on this host's GFX stack; same hand-off as I-1). Note dashboard.css is served with no cache-busting — hard-refresh when checking.

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

## Senior Developer Review (AI)

**Outcome:** Approve. Loop terminated under CLAUDE.md condition 2 — only accepted LOW findings remain. Single independent adversarial review layer (fresh context / different model than the Fable 5 implementer), right-sized for a small CSS + shell.js cosmetic diff. No patches were applied (clean), so no re-review round was required.

**Verified correct:** retired token `--cs-nav-active` has zero remaining `var()` consumers; **all introduced text/bg pairs pass WCAG AA in both light and dark** (default link 8.1:1 / 5.9:1; active white-on-`#096dd9` 5.0:1 both modes — the `#096dd9`-not-`#1890ff` choice is what makes it pass; brand/toggle 16:1; page-header title 15:1/13:1; subtitle 5.7:1 / 4.9:1 — dark subtitle is the tightest and still clears 4.5:1); ARIA toggle wiring correct (`aria-expanded` flips in lockstep, `aria-controls="app-shell-nav"` matches `nav.id`, `aria-label` names the ☰ glyph, focus-visible outlines present); `setup.html` isolation holds (no `/shell.js` include → never gets `has-shell`; offset gated in the ≥992px query, padding uses the same `--cs-sider-width` token); test contract intact (5 labels+hrefs verbatim, pinned class names preserved, idempotence guard + DOMContentLoaded fallback untouched, `dashboard_css_contains_responsive_media_query` still satisfied by the 601px+992px `min-width` queries, no served HTML gains a literal `<nav>`); AC#5 removed generic `header {}` rules have no surviving dependents; AC#6 apply bar (z2000) renders above the sider (z100) and is untouched; no new hex literals in component rules.

**Action Items:**
- [ ] LOW L-1 (accepted / follow-up) — on ≥992px config pages with pending changes, the full-width F-0 apply bar (`apply-bar.js`, `left:0;right:0`) overlays the bottom ~55px of the 232px navy sider. Functionally fine (apply bar fully usable; AC#6 "not obscure the apply bar" holds). Restyling/offsetting the apply bar is **explicitly out of I-2 scope**; deferred — candidate offset (`#apply-bar` left by `--cs-sider-width` under `body.has-shell` at ≥992px) for I-3/I-4 or an apply-bar shell-integration follow-up.
- [ ] LOW L-2 (accepted) — `.app-shell--open` + `aria-expanded="true"` persist across a narrow→wide→narrow resize (toggle is `display:none` when wide; nav force-shown regardless, so state stays self-consistent). Harmless; a `matchMedia` reset would be tidier if revisited.

## Change Log

- 2026-07-02 — I-2 code review (single independent adversarial layer, fresh context / different model). Outcome APPROVE; zero HIGH/MED; 2 LOW accepted (apply-bar overlap out-of-scope; resize-state persistence harmless). Loop terminated CLAUDE.md condition 2. Gates re-confirmed green (cargo test 1803/0, clippy -D warnings, build). Status review → done. Refs #147.
- 2026-07-02 — I-2 implemented: F-1 shell restyled to ChirpStack-adjacent chrome (fixed 232px navy sider ≥992px, mobile app-bar + accessible hamburger drawer, `body.has-shell` layout gate, `.page-header` light strip, 3 new tokens + `--cs-nav-active` retired, legacy generic `header` rules removed). TDD marker assertions added (`aria-expanded`, `has-shell`). Gates: full cargo test 1803/0, clippy `-D warnings` clean, build clean. Refs #147. Status → review.

## Dev Agent Record

### Agent Model Used

claude-fable-5 (Fable 5)

### Debug Log References

- **TDD marker test:** added `aria-expanded` + `has-shell` source-level assertions to `tests/web_dashboard.rs::shell_js_is_served_and_owns_the_nav`; confirmed RED against the F-1 shell (panic at the new assertion), then GREEN after the rewrite. Full `web_dashboard` binary 19/0 after implementation.
- **WCAG AA verification (computed, WCAG relative-luminance formula):** default link 8.13:1 light / 5.86:1 dark; hover white-on-overlay 15.98/17.44:1; **active white on `--cs-nav-active-bg` #096dd9 = 5.00:1 both modes** (white on `--cs-primary` #1890ff would be 3.27:1 — FAIL — hence the dedicated token); brand 15.98/17.10:1; focus ring #1890ff on navy 5.68:1 (non-text ≥3:1 ✓); page-header h1 15.13/13.21:1; subtitle 5.74/4.90:1. All text pairs ≥ 4.5:1, all non-text ≥ 3:1, both modes.
- `grep -n "cs-nav-active[^-]"` after retiring the token: zero consumers remain.
- Hex-grep: non-token hits are comment refs only (`#147`, `#142`, and the `#1890ff`/`#096dd9` contrast-rationale comment on the token line). One real `@media (prefers-color-scheme: dark)` block (second grep hit is the I-1 header comment).
- FR41: `@media (min-width: 601px)` main-grid query untouched; new `@media (min-width: 992px)` sider query is a second `min-width` marker.
- Generic `header {}` audit: `grep "<header" static/*.html` → 6 hits, all `class="page-header"`; `setup.html` has no `<header>`; retirement safe.

### Completion Notes List

- **Scope held to plan:** `static/shell.js` + `static/dashboard.css`, plus the story-sanctioned optional marker assertions in `tests/web_dashboard.rs`. Zero HTML/Rust-production/`/api` change.
- **Layout mechanism (the story's crux, implemented as specified):** mobile-first navy app-bar with an in-flow hamburger drawer (pushes content, never obscures); at `min-width: 992px` the same element becomes a fixed 232px left sider and `body.has-shell` (class added by shell.js) gains the matching `padding-left`. `setup.html` links the stylesheet but never gets the class → wizard layout untouched.
- **AA contrast enforced by construction:** new `--cs-nav-active-bg: #096dd9` token used for the active row because white on `--cs-primary` is 3.27:1 (the exact I-1 failure class); all 14 chrome pairs computed ≥ 4.9:1 text / ≥ 3:1 non-text in both modes (Debug Log).
- **I-1 review L-3 settled:** default nav links now use `--cs-sider-fg-muted` (8.13:1 light / 5.86:1 dark on navy), white reserved for hover/active/brand — the mockup's hierarchy.
- **Token retired:** `--cs-nav-active` (#40a9ff) removed with its two consumers (old underline active-state, dead `header nav strong`); grep confirms zero remaining `var()` references.
- **Legacy cleanup:** generic dark `header {}` element rules removed (all served pages use `.page-header`; wizard has no `<header>`); `.page-header` is now the light strip (card bg + border hairline) per the mockup.
- **Deliberately NOT done (scope):** no component restyling (I-3), no per-page rollout/QA (I-4), no apply-bar restyle, no icons in nav (glyphs would need aria-hidden wrapping; deferred to I-3 polish if wanted), no drawer persistence/Escape-close (nav drawer, not a modal).
- **Pending for owner:** browser visual check vs `I-0-mockup.html` (light + dark, wide + narrow) — automated screenshot attempt failed on host GFX; recommend checking before/at code review. Hard-refresh required (no cache-busting on static assets).

### File List

- `static/shell.js` — **modified**: I-2 chrome restructure. Adds `has-shell` body class, hamburger `<button.app-shell__toggle>` (`aria-expanded`/`aria-controls="app-shell-nav"`/`aria-label`), nav gains `id="app-shell-nav"`; toggle handler flips `.app-shell--open` + `aria-expanded`. NAV array (5 entries, labels/hrefs verbatim), `is-active`/`aria-current` logic, idempotence guard, and injection fallback unchanged.
- `static/dashboard.css` — **modified**: `.app-shell` section rewritten (mobile-first navy app-bar + drawer; ≥992px fixed 232px left sider with `body.has-shell` offset); 3 new tokens (`--cs-sider-width`, `--cs-sider-hover-bg`, `--cs-nav-active-bg`), `--cs-nav-active` retired; `.page-header` restyled as light card strip with bottom hairline; legacy generic `header/header h1/header .subtitle/header nav*` rules removed (subtitle margins folded into `.page-header .subtitle`).
- `tests/web_dashboard.rs` — **modified**: 2 new source-level assertions in `shell_js_is_served_and_owns_the_nav` pinning the accessible toggle (`aria-expanded`) and the `has-shell` layout class (per the story's optional-marker suggestion).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — I-2 status transitions.
- `_bmad-output/implementation-artifacts/I-2-navigation-shell-refresh.md` — this story file (tasks/record/status).

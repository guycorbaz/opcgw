# Epic G Retrospective — Web UX & Usability (v2.4.0)

**Date:** 2026-06-28
**Facilitator:** Bob (Scrum Master) · **Project Lead:** Guy
**Status:** Epic G COMPLETE — 5/5 stories done

---

## 1. Epic summary

Epic G was the first **post-public-announcement** release line (opcgw was announced on the ChirpStack forum 2026-06-27). Its goal: polish the web UI for newcomers arriving from the community. All five stories shipped, each built on the F-1 shell + F-0 staged-apply with **no build step**.

| Story | Issue | Outcome |
|-------|-------|---------|
| G-0 Drill-Down Config Navigation | #139 | done — `config.html`/`config.js` hash-routed Application→Device→Metrics/Commands; retired 3 flat pages |
| G-1 Device-Profile Metric Picker | #124 | done — `GET /api/inventory/measurements` (DeviceService.Get→DeviceProfileService.Get), two-source merged picker |
| G-2 Contextual Field Help | #142 | done — shared `field-help.js` catalog + accessible affordance across 3 form surfaces |
| G-3 Per-Device Stale Threshold | #132 | done — web write-path for the existing v012 column; #132 CLOSED |
| G-4 Dashboard Error Drill-Down | #127 | done — migration v013 `error_events` ring buffer + `GET /api/errors` + `errors.html` |

**Delivery quality:** every story ran the full 3-layer adversarial code review (Blind / Edge Case / Acceptance Auditor) on a **different model (Sonnet)** than the implementer (Opus), plus a mandatory iter-2. All loops terminated **LOW-only**. Gates green throughout: `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean, `node --check` clean, LaTeX manual rebuilds clean.

---

## 2. Mandatory epic security review

**VERDICT: CLEAN — 0 HIGH / 0 MEDIUM / 2 LOW** (full audit of the `d2b2230..a101c1d` Epic G diff; independent Sonnet reviewer).

- **No hardcoded secrets** — the `api_token` flows from `AppConfig` into a `BearerInterceptor`, never a literal; no secrets in JS.
- **Input validation** — `normalise_dev_eui` (16-hex), `/api/errors ?limit` cap→400, `validate_opt_stale_threshold` band-check, `?refresh` strict match.
- **No info leakage** — `sanitize_error_message` (Bearer redaction + control-char strip + length bound); endpoint error bodies are generic, raw errors go to operator log only; the measurements 502 returns only the 4 stable `chirpstack_failure_reason` strings.
- **No SQLi** — every new statement parameterized (`error_events` insert/prune/select, device CRUD); `usize→i64` limit clamp guards the `-1`/no-limit coercion.
- **Access control** — `/api/errors` + `/api/inventory/measurements` are inside the auth + CSRF + first-run-gate layer stack; the new `/field-help.js` wizard-bypass entry is exact-match, GET-only, static, secret-free.
- **XSS** — all API/ChirpStack data rendered via `textContent`; no `innerHTML` sink reached with dynamic data.
- **Resource bounds** — error-event ring buffer capped (`OPCGW_ERROR_EVENT_CAP`); `/api/errors` limit capped.

**LOW-1** (dead `html:` sink in `config.js` `el()`): **fixed inline** in this retro — added a DANGER comment so a future dev can't accidentally route user data through it. **LOW-2** (inventory caches have no max-entries eviction — pre-existing pattern across apps/devices/measurements): captured as **AI-G-2**.

---

## 3. What went well

- **The Opus-implements / Sonnet-reviews split kept paying off.** Cross-model review surfaced real defects every story: G-1's `device_profile=None` cached-as-empty, G-2's `aria-describedby`-on-`hidden` (3 layers converged independently), G-4's runtime-cap-vs-const divergence (2 layers converged).
- **iter-2 repeatedly caught defects the iter-1 patches *themselves* introduced** — the single strongest validation of the mandatory iter-N+1 rule this epic: G-2 iter-2 caught 2 doc-type errors (`u64` vs `u32`/`usize`) added by the iter-1 doc patches; G-4 iter-2 caught a hardcoded cap test + a classifier auth/connect mis-ordering both introduced by iter-1. Without iter-2 these ship.
- **No-build-step vanilla discipline held** across a frontend-heavy epic — one shared `field-help.js` and `inventory-picker.js`, reused, no framework, no `node_modules`.
- **Scope discipline** — G-4's cap was deliberately kept off the singleton-config/UI surface (const + env), avoiding a cascade into the G-2 help catalog.

## 4. What was hard / lessons

- **Loading shared JS on the first-run wizard needs an explicit auth-bypass allowlist entry.** The wizard gate is a *hardened exact-match* allowlist (the blanket `.js` suffix bypass was removed back in C-0 iter-2/3). `field-help.js` had to be added to `WIZARD_BYPASS_EXACT` — a non-obvious Rust touch on an otherwise frontend story. **Lesson:** any new shared static asset the wizard page needs must be allowlisted lock-step with its test.
- **A runtime-configurable cap must be read at the point of use, not frozen in a sibling constant.** G-4 shipped a `ERRORS_LIMIT_CAP=500` const that diverged from the `OPCGW_ERROR_EVENT_CAP` runtime value — caught in review. **Lesson:** when a value is env-overridable, every consumer reads the accessor.
- **Accessibility is an acceptance criterion, not a nice-to-have.** G-2's `aria-describedby` pointed at a `hidden` node (dropped from the a11y tree → announced nothing). **Lesson:** for a disclosure-style affordance, keep the described element in the a11y tree (CSS-collapse), don't use `hidden`.
- **Doc-type accuracy drifts silently.** G-2's catalog was authored ahead of `docs/configuration.md` (12 undocumented fields) and the backfill introduced wrong type annotations. **Lesson (carried):** field-help text and the config reference are one source of truth — keep them lock-step.

## 5. Previous-retro follow-through (Epic F)

- **AI-F-1 (real-world onboarding smoke as the release gate)** — STILL OPEN. Epic G shipped to `main` + Docker `:2.4` but the end-to-end onboarding smoke against a real ChirpStack has not been re-run for the v2.4.0 surface. Carried as **AI-G-5** — this remains the gate before promoting a v2.4.0 *stable* tag (cf. the 2026-05-20 main-deadlock incident: review layers don't catch runtime/deadlock issues).

---

## 6. Action items

| ID | Action | Owner | Priority |
|----|--------|-------|----------|
| AI-G-1 | Harden the `el()` `html:` sink with a DANGER comment (security LOW-1) | — | **DONE inline** in the retro commit |
| AI-G-2 | Add a `max_entries` eviction bound to `InventoryCache` (apps/devices/measurements maps) — pre-existing LOW-2 | Guy's call | LOW (follow-up) |
| AI-G-3 | Wrap error-event INSERT + prune in one transaction; or accept the benign converges-to-cap race | Guy's call | LOW (deferred-work.md) |
| AI-G-4 | `#73` — blocking `pool.checkout` from async tasks (now also on the error-capture path) — the cross-cutting `spawn_blocking`/async-pool fix | Guy's call | MEDIUM (carried, #73) |
| AI-G-5 | **Real-world onboarding + web-UX smoke against a live ChirpStack before any v2.4.0 *stable* tag** (carries AI-F-1; the release gate) | Guy | **HIGH (release gate)** |

## 7. Milestone / release status

- All Epic G commits on `origin/main` through `a101c1d` (after this retro push).
- GitHub milestone **#4 (v2.4.0)** issues all resolved: #139 (G-0), #124 (G-1), #142 (G-2), #132 (G-3, closed), #127 (G-4). Close the milestone after push.
- v2.4.0 has an `-rc1` cut (`a71a823`); promoting to **stable** is gated on **AI-G-5**.

## 8. Next direction

Epic G is the end of the currently-planned roadmap. Candidate next directions (Guy's call): (a) run AI-G-5 and cut v2.4.0 stable; (b) a v2.x technical-debt epic (the #73 async-pool fix, the cache eviction bound, the substring-matcher codification carried since Epic C/D); (c) CR-driven work from the forum backlog. No new epic should start before AI-G-5 if a v2.4.0 stable tag is intended.

---

*Retrospective complete. Security check CLEAN. Epic G closed 5/5.*

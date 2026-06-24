# Epic F Retrospective — Onboarding & Web UX for Public Release

**Date:** 2026-06-24
**Facilitator:** Bob (Scrum Master) · **Project Lead:** Guy
**Epic tracking:** GitHub [#140](https://github.com/guycorbaz/opcgw/issues/140)
**Status at retro:** all 5 stories done (F-0…F-4); this retrospective closes the epic.

---

## 1. Epic Summary

Epic F was a **post-PRD readiness epic** — its single purpose was to make opcgw's *first-touch* experience (configuration + web UI) smooth enough to **announce the project to the ChirpStack team**. It covered no original PRD FRs; the functional contract was captured in `epics.md` and the 2026-06-14 design dialogue.

**Delivery metrics**
- Stories completed: **5/5** (F-0 staged-config-apply, F-1 unified web shell, F-2 zero-touch first-run wizard, F-3 dashboard landing redesign, F-4 config export/import).
- Commit range: `1c4fa96..f20c639` (impl + per-story review-fix commits, one story per commit pair).
- Gates at close: `cargo test` 38 suites / 0 failed; `cargo clippy --all-targets -D warnings` clean.
- Epic security review: **CLEAN** (0 HIGH / 0 MEDIUM / 3 LOW — see §6).
- New runtime dependencies: `toml 0.8` only (F-4). **No build step, framework, or `node_modules` added** — the core constraint held.

**Outcome:** opcgw can now be stood up browser-only from an empty `config.toml`/`.env`, edits accumulate as staged "pending changes" applied via one explicit operator-initiated soft restart (container never restarts), every page shares one nav/shell, the landing page shows health at a glance, and configurations export/import as portable secret-free TOML.

---

## 2. Per-Story Outcomes & Review Cost

| Story | What shipped | Review loop |
|-------|--------------|-------------|
| **F-0** staged-config-apply | Built the **in-process restart supervisor** (new `restart_token` + supervisor loop wrapping the data-plane spawn — the historically deadlock-prone zone). Unified ALL config under staged + explicit Apply; removed the restart-required allowlist; **closed #138** (gRPC stream re-scopes on apply). | iter-1 + mandatory iter-2 — caught **2 HIGH** the suite missed: an `applied_gen` lost-update race and a build-failure that exited the process. |
| **F-1** unified web shell | Shared `static/shell.js` (vanilla) injecting one nav/header on every page; component CSS (`.app-shell`/`.btn`/`.status-badge`/`.banner`). | iter-1 — caught a doubled-header-bar regression; full component-primitive adoption deferred to F-3. |
| **F-2** zero-touch wizard | `/setup` now captures CS `server_address`/`tenant_id`/`api_token` + OPC UA password; secrets→`secrets.toml` 0600, rest→SQLite; `validate()`/`is_first_run()` extended to carve out missing CS creds so a pristine config boots into the wizard. | iter-1 + iter-2 + iter-3 — caught **2 HIGH**: a partial singleton write that poisoned the D-0 migration (Guard 2 short-circuit, no self-heal), and a placeholder prefix-vs-marker asymmetry that jammed the migration and dead-ended the wizard. |
| **F-3** dashboard redesign | Client-derived health verdict + poller-stall tile + per-device freshness panel, all from existing `/api/status` + `/api/devices` — **no gateway aggregation** (#130). One backend pass-through field (`poll_interval_secs`). | iter-1 + iter-2 — no HIGH; 3 MEDs fixed (per-poller error slots vs shared-banner clobber; freshness-threshold validation; band-model alignment). |
| **F-4** config export/import | TOML export (secrets stripped via `SECRET_FIELDS_BY_SECTION`) + figment-merge import (target keeps its own secrets) staged through F-0's Apply flow; single-EXCLUSIVE-txn `import_replace_all`. | iter-1 **1 HIGH** (`command_class` dropped on import → Epic E valve binding lost on round-trip) + 3 MED; **iter-2b independent re-review 1 MEDIUM** (pool connections lacked `busy_timeout` → spurious 500 under concurrent writers; fixed, GH #141). |

---

## 3. What Went Well

1. **The staged-apply design (F-0) was a force-multiplier, not just a feature.** Choosing "stage + explicit Apply soft-restart" over live hot-mutation *deleted* whole classes of code: the restart-required allowlist, bespoke gRPC re-subscribe logic, and it auto-closed **#138**. The 9-7/9-8 live-reload plumbing went dormant. A rare case where the simpler model removed complexity rather than adding it.
2. **The no-build-step constraint held under real pressure.** Five UI-touching stories, a shared shell, a dashboard redesign, and an export/import UI — all delivered in vanilla JS with one new crate (`toml`) and zero `node_modules`. The "auditable industrial gateway" story is intact.
3. **The adversarial code-review loop earned its keep again.** HIGH/MEDIUM defects invisible to the full test suite were caught in **4 of 5 stories** (F-0: 2 HIGH, F-2: 2 HIGH, F-4: 1 HIGH + 1 MEDIUM). Consistent with the project-long iter-N+1 doctrine.
4. **Clean security posture at the gate.** The epic security review found zero exploitable defects — the per-story loops had already established complete secret skip-lists, body limits, CSRF scoping, tight wizard-bypass allowlists, and revert-on-failure atomicity.

---

## 4. What Was Hard — Key Lessons

1. **"Self-verification ≠ independent re-review" (the standout lesson).** F-4 arrived this session with a prior-session iter-2 that declared the loop clean — but it was a *same-author self-check*. Because iter-1 had introduced **brand-new transaction flow-control** (`import_replace_all`), the iter-N+1 doctrine mandated a *fresh-agent* adversarial pass. That iter-2b caught the `busy_timeout` MEDIUM the self-check missed. **Lesson: when an iteration introduces brand-new flow-control, the next iteration must be an independent pass, not the author re-reading their own change — and the loop is not "done" until that pass runs.** (Reinforces [[feedback_iter3_validation]].)
2. **Migration-state coupling is a repeat trap.** Both F-2 HIGHs and F-4's core design centered on the D-0 singleton-migration guards — writing singleton rows *outside* the migration trips Guard 2 and permanently blocks the full migration with no self-heal. F-4 had to be *designed* around it (singletons written inside the import EXCLUSIVE txn). This trap has now bitten across D-0, F-2, and shaped F-4. It deserves a codified note / a guard-rail helper.
3. **Pre-existing latent debt surfaces when a new feature widens the window.** The `busy_timeout` gap was pre-existing across all 8 EXCLUSIVE writers, but F-4's whole-tree-replace-under-EXCLUSIVE made the race materially more likely — which is what made the review flag it. Fixing it hardened the entire pool. Review of new code is also a probe for old assumptions.

---

## 5. Previous-Retro Continuity (Epic E → Epic F)

| Epic E action item | Status in Epic F |
|--------------------|------------------|
| **AI-E-3** — triage open CR backlog, sequence next direction | ✅ **Directly drove Epic F.** #138 (uplink stream-set hot-reload) was absorbed into F-0 and is now **CLOSED**. |
| **AI-E-2** — decide E-2b's fate | ⏳ Untouched (still backlog; out of Epic F scope). |
| **AI-E-1** — unify in-memory command store (test fidelity) | ⏳ Not addressed (low urgency, prod is SQLite). |
| **AI-E-4** — standing skill-codification / cleanup epic | ❌ Still unpaid — now carried since Epics C/D/E. Epic F added one more candidate (the migration-guard trap, lesson §4.2). |

---

## 6. Epic Security Review (CLAUDE.md epic-completion gate)

**Scope:** full `1c4fa96..f20c639` diff across all F-0…F-4 `src/`, `static/`, `config/` changes.
**Verdict: CLEAN — 0 HIGH / 0 MEDIUM / 3 LOW.** Satisfies the CLAUDE.md epic-completion security requirement.

Checklist (all PASS): no hardcoded credentials; secrets never reach SQLite/logs/export (export skip-list complete — the only two secrets are `chirpstack.api_token` + `opcua.user_password`; web-auth creds live in `.env`, outside the export surface); input validation + body limits (4 KiB setup / 16 KiB singleton PUT / 1 MiB import) + `validate()`-before-persist; error messages leak nothing; SQL fully parameterized (only `format!` SQL is `DELETE FROM {table}` over a fixed allowlist); access control (all routes auth+CSRF gated; `WIZARD_BYPASS_EXACT` exact-match only; apply re-reads+validates before teardown, revert-on-failure); no `unsafe`, no panics on external input.

**LOW / informational (deferred, non-blocking):**
- **L1 (hardening) — import merge validates but never persists attacker-supplied `[storage]`/`[logging]`/`[command_validation]`.** Not exploitable (those sections are never written by `import_replace_all` and re-read from `config.toml`/env on apply), but for symmetry the imported TOML should strip `EXPORT_EXCLUDED_SECTIONS` before merge too. → added to deferred-work.
- **L2 — `apply_failed` is a process-global `AtomicBool`.** Correct under the single-AppState-per-process model; noted for completeness.
- **L3 — ServeDir symlink-follow caveat** is pre-existing, unchanged, already documented in `docs/security.md`.

---

## 7. Technical Debt Carried Forward

- **F-0-FOLLOWUP:** fully remove the now-dormant `config_reload.rs` / `notify_crud_write` live-reload plumbing (9-7/9-8 era) — dead under the unified apply model.
- **#141 fix follow-on (LOW):** pool `PRAGMA foreign_keys` still not set on pooled connections (only the standalone migration conn) — CASCADE-reliant `delete_application`/`delete_device` could orphan rows; F-4's explicit child-first deletes are safe regardless. Separate dedicated fix.
- **Security L1:** strip `EXPORT_EXCLUDED_SECTIONS` from imported TOML before the figment merge (symmetry / future-proofing).
- **F-4 LOWs:** zero-application import can't clear the tree (figment array-absent kept); export reflects the *applied* config not staged edits; no served-HTML marker test for the new `config-io` section of `singleton-config.html`.
- **AI-E-4** standing skill-codification / cleanup epic — substring-matcher, typed-error refactor, in-memory test fidelity, plus the new migration-guard-trap guard-rail (§4.2).

---

## 8. Next Direction & Action Items

**Project Lead decision (Guy, 2026-06-24): cut a release and announce.** Epic F's entire reason for existing was readiness to announce opcgw to the ChirpStack team; with all 5 stories done, the announcement is the immediate next step — gated on a real-world smoke of the new onboarding flow first (the AI-D-3 real-world-test gate still stands; cf. [[incident_main_deadlock_2026_05_20]] — `cargo test` does not exercise the full boot/apply path).

| # | Action | Category | Owner | Notes |
|---|--------|----------|-------|-------|
| **AI-F-1** | Real-world smoke of the zero-touch onboarding flow on panoramix: empty-config boot → wizard → CS connect → Apply soft-restart → export/import round-trip | release-gate | Guy | **Critical path before any tag.** Exercises F-0/F-2/F-4 against the real ChirpStack + a real OPC UA client. |
| **AI-F-2** | Cut **v2.3.0** stable (tag + GitHub release + Docker Hub/GHCR multi-arch `2.3.0`/`2.3`/`latest`), then **announce to the ChirpStack team** | release | Guy | After AI-F-1 passes. Release notes lead with the browser-only onboarding + staged-apply story. |
| **AI-F-3** | Close out the F-0-FOLLOWUP dead-code removal (`config_reload.rs`/`notify_crud_write`) | tech-debt | dev | Reduces confusion; the plumbing is dormant but still compiled. |
| **AI-F-4** | Codify the **migration-guard trap** (write singleton rows only inside the migration; never via partial section writes) as a guard-rail helper or a CLAUDE.md/skill note | tech-debt / process | dev | Has now bitten across D-0, F-2; F-4 had to design around it. Feed into AI-E-4. |
| **AI-F-5** | Sequence the remaining CR backlog post-announcement: #136 (decouple command dispatch from metrics poll), #137 (generalize device-class registry beyond Tonhe), #139 (web-UI drill-down config) | planning | Guy | Post-release; informed by ChirpStack-team feedback. |
| **AI-E-4** (carried) | Stand up the skill-codification / cleanup epic | tech-debt | Guy | Standing since Epics C/D/E; now with the migration-guard item added. |

---

## 9. Readiness Assessment

- **Testing & quality:** `cargo test` 38 suites / 0 failed; clippy `-D warnings` clean. **Real-world smoke NOT yet run** (AI-F-1) — the release gate.
- **Security:** CLEAN (§6) — no blockers.
- **Deployment:** not yet tagged/released; v2.3.0 is the planned cut after AI-F-1.
- **Stakeholder acceptance:** the "stakeholder" is the ChirpStack team — acceptance is the post-announcement feedback loop (AI-F-5).
- **Codebase health:** stable; the F-0 supervisor (historically deadlock-prone) is covered by subprocess integration tests; one dormant-code cleanup (AI-F-3) outstanding.
- **Unresolved blockers:** none technical. The only gate before release is the real-world smoke (AI-F-1).

**Bob (Scrum Master):** "Epic F is complete and clean. The one thing standing between us and the announcement is the real-world smoke — not a code problem, a verification step. Nice work, team. Let's not announce until that smoke passes."

# Epic D — Singleton Configuration → SQLite — Retrospective

**Date:** 2026-05-28
**Facilitator:** Bob (Scrum Master)
**Project Lead:** Guy Corbaz
**Epic status:** 3/3 stories `done`; retrospective `done`.

---

## Epic summary

Epic D is the **natural C-6 follow-up** recommended by the 2026-05-26 Epic C retrospective. Story C-6 moved the `[[application]]` tree from `config.toml` into SQLite; the remaining singleton sections — `[global]`, `[chirpstack]`, `[opcua]`, `[web]` — stayed in TOML. Epic D migrated those too, added a web editor for them, and made `config.toml` inert at runtime. A deliberately narrow 3-story epic (vs Epic C's 6) to stay finishable.

**End state achieved:** opcgw now has exactly **three persistence surfaces** —
1. **SQLite** (`data/opcgw.db`) — authoritative for configuration + metric values + commands + history,
2. **`config/secrets.toml`** — operator-supplied secrets, chmod 0o600 via atomic-rename,
3. **`config.toml`** — bootstrap-seed only, read once at first boot, never mutated at runtime.

Guy's 2026-05-20 articulated end-state — *"in the final version, all configuration should be in database"* — is now fully realised by Epic D combined with C-6.

**Stories:** D-0 (singleton → SQLite migration) · D-1 (singleton config editor UI) · D-2 (decommission TOML mutation surface, must-land-last).

**Span:** 2026-05-26 → 2026-05-28 (opened same session as the Epic C retrospective close).

**Test gate at epic close:** `cargo test` **1544 passed / 0 failed / 73 ignored** (+62 net new vs the 1482 post-Epic-C baseline); `cargo clippy --all-targets -- -D warnings` clean; `xmllint --noout --valid` clean.

**Doctrine streak:** 24× → **32× cumulative** iter-N+1 validations. Epic D added 8 review iterations (D-0: 3, D-1: 2, D-2: 3); 7 of the 8 surfaced real findings — only D-2's iter-3 returned zero findings (the strongest possible loop termination, CLAUDE.md condition #1).

---

## Per-story summary

### D-0 — Singleton Config → SQLite Migration

- **Spec + impl:** `2c4a955` spec + `cdba5e6` impl (2026-05-26). Schema **v010** adds a generic K/V `singleton_config(section, key, value)` table (**Option A** chosen over per-section typed tables, for type-uniformity + zero-schema-change-for-future-fields + JSON-encoded handling of `[web].allowed_origins`). Boot-time one-shot migration mirrors C-6: primary done-flag guard + secondary back-fill guard + placeholder-secrets skip + row-count verify, falling back to TOML on mismatch. **AI-C-SEC-2 incorporated** — chmod 0o600 on fresh DB creation.
- **Review cycle (3 iters on Sonnet):** `9adf84c` iter-1 (11 patches) + `6a753cf` iter-2 (9 patches) + `00b61e6` iter-3 (4 patches) + `272dae3` flip. **25th–27th cumulative doctrine validations.** iter-1 caught **3 HIGH converged across BH/ECH/AA** (no outer EXCLUSIVE transaction + TOCTOU + AC#4 violation); iter-2 caught **2 NEW HIGH in iter-1's own brand-new EXCLUSIVE-transaction logic** (unbounded count query + Test 11 WAL fake-guard); iter-3 caught 2 converged doc-sync gaps from iter-2's patches. Final 1506/0/73.
- **Key decisions:** Schema Option A (K/V); `[opcua].user_name` migrated to SQLite (not skipped like `user_password`) per the OPC UA threat model (usernames are non-secret); AC#7 read-path swap **deferred to D-2** (figment Provider rework needs subsystem-construction reordering) — documented as D-0-FOLLOWUP-2.

### D-1 — Singleton Config Editor in the Web UI

- **Spec + impl:** `0129d34` spec + `662ad4e` impl (2026-05-27). New `src/web/singleton_config.rs` (GET + PUT handlers), `static/singleton-config.{html,js}`, boot-time `overlay_singletons_from_sqlite_rows` (closes the boot-time half of D-0-FOLLOWUP-2). Secret skip-list extracted to `SECRET_FIELDS_BY_SECTION`. CSRF `singleton_config` resource bucket added. Nav strip extended on 8 sites.
- **Review cycle (2 iters on Sonnet):** `858f917` iter-1 (12 patches + orphan-file deletion) + `5d9e4ca` iter-2 (7 patches) + `561ff62` flip. **28th–29th cumulative doctrine validations.** iter-1 caught **1 HIGH** (overlay partial-mutation — non-deterministic HashMap + early `?`-return left `AppConfig` in a hybrid state) + 7 MED (notify_crud_write silently reverting singleton edits → new `ConfigReloadHandle::seed_post_overlay`; fake-regression-guard Test 4; log-injection sinks; `OpcGwError` in HTTP body; JS NaN guard; setup.html exclusion). iter-2 was doc/test/refactor with no new flow-control → loop correctly terminated at iter-2 (no iter-3 mandate). Final 1521/0/73.
- **Key decision:** AC#8 precedence inversion (post-D-1, SQLite wins over env-var at boot) accepted as a transitional state; proper `env > SQLite > TOML > default` ordering deferred to D-2 as D-1-FOLLOWUP-1.

### D-2 — Decommission TOML Mutation Surface (must-land-last)

- **Spec + impl:** `6f0ad21` spec + `c84072c` impl (2026-05-27). New `src/storage/sqlite_singleton_provider.rs` — a `figment::Provider` that reads singleton config from SQLite and sits between the secrets.toml and env-var layers, delivering `env > SQLite > TOML > default` as a **structural guarantee** (replaces D-1's `Arc::make_mut` overlay). New `AppConfig::from_path_with_sqlite` API. Orphan `src/web/config_writer.rs` deleted (551 LOC, Story 9-4 era). `toml_edit` removed from `Cargo.toml` (the secrets.toml pre-validator swapped to figment's own TOML parser). New `config_toml_unused_warning` warn-event, once-per-boot, when `config.toml` is present alongside a populated `singleton_config`.
- **CI fix:** `86b74d4` added the missing SPDX header to `src/storage/sqlite_tests.rs` — a **pre-existing C-6 regression** (committed by `1c09911` without a header) that had silently failed the CI gate on every D-0/D-1 commit. First green CI since 2026-05-26.
- **Review cycle (3 iters on Sonnet):** `3d3642e` iter-1 (12 patches) + `4d8066e` iter-2 (3 patch groups) + clean iter-3 folded into `95437d4` flip. **30th–32nd cumulative doctrine validations.** iter-1 caught **1 HIGH** — AC#12 DocBook user manual rewrite was *claimed in the impl commit but the file was never touched* (caught **only by the Acceptance Auditor**) — plus a 3-way-converged fake-regression-guard (t07/08/09 asserted only the boolean predicate, never the warn emit → extracted testable `AppConfig::maybe_emit_config_toml_unused_warning` helper) + a Provider read-time secret-filter gap. iter-2 caught the count/reload/warn ordering interaction in iter-1's *own* new code (relocated into the reload `Ok` arm) + the secret-filter's missing test. **iter-3 CLEAN — zero findings from both Blind Hunter and Edge Case Hunter.** Final 1544/0/73.
- D-2 closed D-1-FOLLOWUP-1 (figment Provider) + D-1-FOLLOWUP-2 (security.md) + D-1-FOLLOWUP-3 (architecture.md) + D-1-FOLLOWUP-4 (DocBook).

---

## Cross-story synthesis

### What went well

1. **The three-surface end-state landed exactly as designed,** and via a *structural* mechanism (the `SqliteSingletonProvider` figment Provider) rather than a runtime overlay patch. The precedence ordering is now a compile-time-shaped guarantee, not a boot-time mutation.
2. **D-2 terminated on a genuinely clean iter-3** — zero findings from both reviewer layers, *after* iter-2 had relocated control flow. The iter-N+1 mandate proved well-bounded again: it fired where new code was introduced (D-0 all 3 iters, D-2 iter-3) and correctly stood down where it wasn't (D-1 iter-2).
3. **AI-C-SEC-2 — the one Epic C security follow-up assigned to this epic — was actually resolved.** D-0's `pool.rs` now uses `OpenOptions::create_new(true).mode(0o600)` (TOCTOU-free atomic create) with a once-per-boot warn for pre-existing wider-mode DBs. An epic that was *handed* a prior security item and *closed* it.
4. **The dependency-driven cleanup chain worked.** D-2 closed four D-1 follow-ups in one sweep, deleted a 551-LOC orphan module, and removed a production dependency — all subtractive, no backwards-compat shims.
5. **The Acceptance Auditor earned its place again.** D-2 iter-1's lone HIGH (a claimed-but-untouched DocBook deliverable) was invisible to Blind Hunter and Edge Case Hunter; only the AA, which reads the spec ACs, caught it. **Confirmed finding-class: documentation-AC violations are AA-only.**

### What didn't go well

1. **The substring-matcher anti-pattern regressed — D-0 actively re-extended it.** Epic C's AI-C-1 said codify the `error.to_string().contains(...)` classifier ban into the skill because *"the pattern lives in developer reflex faster than the doctrine reaches."* D-0 then added a `"singleton_row_count_mismatch"` substring arm in `main.rs` (D-0-FOLLOWUP-1). The retro predicted this exact relapse; the skill source remains untouched.
2. **Skill-codification debt grew 8 → 9 items** (D-0-FOLLOWUP-1 joins AI-A-1/2/3 + AI-B-1/2/3 + AI-C-1/2). It is now a stable **four-epic carry-forward** (A→B→C→D). No epic has paid it down; every epic adds to it.
3. **CI was silently RED from C-6 through every D-0/D-1 commit** — a missing SPDX header on `src/storage/sqlite_tests.rs` (C-6's `1c09911`), not fixed until `86b74d4` on 2026-05-28. The gate worked but the red state persisted across an epic boundary unnoticed.
4. **GitHub tracking issues for D-0/D-1/D-2 were not opened** — the `Refs #__` placeholder pattern from Epics A/B/C continued (AI-C-6 carry-forward; gh CLI not write-authenticated in agent sessions).

### Epic C retro follow-through

| Epic C action item | Status in Epic D | Evidence |
|---|---|---|
| **AI-C-SEC-2** — SQLite file perms 0o600 | ✅ **Done** | `pool.rs` atomic `create_new + mode(0o600)` (D-0); verified by security review |
| **AI-C-2** — patch-size-is-not-skip-signal | ✅ Honored in practice | D-2 ran iter-3 on a control-flow relocation; D-1 correctly stopped at iter-2 |
| **AI-C-1** — codify substring-matcher ban in skill | ❌ **Not done — regressed** | D-0-FOLLOWUP-1 re-extended the anti-pattern |
| **AI-C-5** — skill-codification epic | ❌ Grew 8 → 9 | Pure carry-forward, now 4 epics deep |
| **AI-C-6** — GitHub issues per story | ❌ Not done | `Refs #__` placeholders continued |
| **AI-C-3** — real-world batched validation gate | ⏳ **Deferred** | Manual smoke against real ChirpStack deferred per the main-deadlock doctrine (Guy confirmed in this retro: still deferred) |
| AI-C-SEC-1 / AI-C-SEC-3 | ➖ Out of Epic D scope | Storage / C-0 territory; still v2.x |

**Score: 2 done (AI-C-SEC-2 + AI-C-2 in practice); 1 regressed (AI-C-1); 2 grown/continued carry-forward (AI-C-5, AI-C-6); 1 deferred (AI-C-3).** The clearest signal: the skill-codification debt cannot be paid down *by a feature epic* — it needs its own epic, which is now the chosen next direction.

---

## Security review

Conducted by an automated subagent against the Epic D diff (`a955bc4..HEAD`, HEAD `95437d4`) per CLAUDE.md "Epic Completion Requirements." **Verdict: CLEAN — 0 HIGH, 0 MEDIUM, 2 LOW.**

| # | Check | Result |
|---|---|---|
| 1 | Hardcoded credentials/secrets | CLEAN — only placeholder constants |
| 2 | Input validation (PUT body / Provider read / migration) | CLEAN — section allowlist, JSON-object guard, per-field re-serialize, candidate `AppConfig::validate()`; Provider/GET skip malformed rows |
| 3 | Secret/path leak in HTTP responses | CLEAN — `OpcGwError` logged server-side only; clients get static hints |
| 4 | SQL injection | CLEAN — all new queries use `params![]` / `?` |
| 5 | Command injection (`check-d0-migration.sh`) | CLEAN (1 LOW note) — `set -euo pipefail`, `$DB` quoted everywhere |
| 6 | Access control / secret masking / chmod | CLEAN — both routes behind Basic-Auth + CSRF; secrets rejected on write AND masked on read; Provider read-time secret filter; atomic 0o600 create |
| 7 | New `unsafe` blocks | CLEAN — none |
| 8 | SPDX headers on new files | CLEAN — all four new source/SQL files carry `MIT OR Apache-2.0` |

**Previously-flagged classes confirmed correctly handled in final code:** secret-in-HTTP-body (static hint only), substring error-matching (`singleton_row_count_mismatch` checked before `row_count_mismatch`, ordering documented as load-bearing), file-permission AI-C-SEC-2 (atomic, no symlink-following `set_permissions`), read-path secret shadowing (Provider read-time filter is load-bearing defense-in-depth).

**Findings (LOW only, non-blocking):**
- **S-D1 (LOW)** — `scripts/check-d0-migration.sh:120` interpolates the loop-controlled `$SECTION` into a sample-row SQL string. Values are a hardcoded literal loop (`global chirpstack opcua web`), not externally injectable. Cosmetic; the only un-parameterized SQL in the script.
- **S-D2 (LOW)** — `[opcua].user_name` is stored in `singleton_config` in plaintext (not in the secret skip-list). This is a **documented, user-accepted defer** (D-0 iter-1 I1-F12), consistent with OPC UA treating usernames as non-secret; mitigated by chmod 0o600. No action required.

Neither LOW blocks Epic D close per CLAUDE.md loop discipline.

---

## Action items

### Process / next direction

**AI-D-1 — Open the v2.x skill-codification + cleanup epic (Guy's chosen next direction).** This is the strongest case yet: D-0 *re-extended* the substring anti-pattern the Epic C retro asked to codify, and the debt is now a stable four-epic carry-forward. The epic should bundle:
- The **9 skill-codification items** (AI-A-1/2/3 + AI-B-1/2/3 + AI-C-1/2 + D-0-FOLLOWUP-1) — codify the substring-matcher ban, inline-comment misdirection check, deferral-impact misclassification, etc. into the `bmad-code-review` skill source itself, not just memory + CLAUDE.md.
- **AI-C-4** — substring-matcher cleanup pass + the **typed `OpcGwError::RowCountMismatch` refactor** (D-0-FOLLOWUP-1) replacing the substring classifier at all emission sites (C-6 + D-0).
- **Issue #102** — `tests/common` extraction (carryover from Epic 9-5/A/B/C).
- **AI-C-6** — open the missing GitHub tracking issues for C-0/C-1/C-2/C-4/C-6 + D-0/D-1/D-2 in one batch.
- Owner: Project Lead (Guy). Success criteria: future error-handling stories surface the substring pattern at iter-1, not iter-3.

**AI-D-2 — Add an SPDX-header CI guard to the pre-commit / story-close checklist.** The C-6 missing-header regression went red across an epic boundary. A `grep`-based header check at story-close (or a pre-commit hook) would have caught it at C-6, not D-2. Owner: Guy. Success criteria: no story-close commit lands with a CI-red header gate.

### Highest-leverage carry-forward

**AI-D-3 — Real-world batched validation (AI-C-3) is now a critical-path gate before any production cutover.** Guy confirmed in this retro that Epic D has **not** been smoke-tested against his real ChirpStack + a real OPC UA client. The 2026-05-20 `main.rs` deadlock proved review layers do not catch runtime issues; Epic D added a boot-sequence reorder (`from_path_with_sqlite` + Provider stack + overlay removal) — exactly the kind of change that incident pattern warns about. **Run the post-D-2 binary against real ChirpStack + OPC UA for at least one full poll cycle, confirming config now flows from SQLite, before tagging a release or cutting over.** Owner: Guy, at his discretion on timing.

### Technical debt — v2.x carry-forward (rolls into AI-D-1's epic)

- D-0-FOLLOWUP-1 (MED) — typed `OpcGwError::RowCountMismatch` refactor.
- I2-F10 (LOW) — `BEGIN EXCLUSIVE TRANSACTION` SQLITE_BUSY at boot if another connection holds a lock (idempotent retry on next boot; documented).
- D-1 LOW defers — setup.html nav exclusion, GET orphan-row silence, JS Origin assumption, Test 11 error-path coverage, secret-field whitespace-bypass normalization.
- AI-C-SEC-1 (prune_old_metrics format-string SQL) + AI-C-SEC-3 (setup_get filename log) — pre-Epic-D, still open.

### Memory updates

- **AI-D-4** — Update `feedback_iter3_validation` memory: 24× → 32× cumulative; new confirmations (D-2 iter-3 clean termination after control-flow relocation; AA-only documentation-AC finding class; substring-matcher *regression* despite the codification ask).
- **AI-D-5** — Save this retrospective doc + flip `epic-D` and `epic-D-retrospective` to `done`.

---

## Next epic preparation

**No Epic E is defined.** Epic D is the last epic in `_bmad-output/planning-artifacts/epics.md` (1–9 numeric + A/B/C/D lettered). The fork the Epic C retro parked is now resolved:

**Decision (Guy, 2026-05-28): open a v2.x skill-codification + cleanup epic** (see AI-D-1). Rationale: the skill-codification debt is structurally stuck — four epics of carry-forward, and D-0 demonstrated that a feature epic will *add* to it rather than pay it down. The dedicated epic is the only mechanism that closes it.

Sequencing note: **AI-D-3 (real-world smoke) should run before any release tag**, independent of the cleanup epic. The cleanup epic is pure-quality (skill source + error-typing + tests/common + GitHub issues) and carries low runtime risk, so it can proceed in parallel with Guy scheduling the smoke test.

---

## Readiness assessment

| Dimension | Status |
|---|---|
| Testing & quality | ✅ `cargo test` 1544/0/73; `cargo clippy --all-targets -- -D warnings` clean; `xmllint --valid` clean; CI green since `86b74d4` |
| Deployment | ✅ All Epic D commits on `origin/main` (D-2 chain through `95437d4`); this retro commit + push pending |
| Stakeholder acceptance | ✅ Project Lead (Guy) participated in all 3 stories' review loops |
| Technical health | ✅ Three-surface model clean; no new `unsafe`; orphan module + dead dep removed; SPDX gate now green |
| **Real-world end-to-end** | ⏳ **DEFERRED — highest-leverage open item.** Not yet run for Epic D; critical-path gate before production cutover (AI-D-3) |
| Security | ✅ CLEAN — 0 HIGH, 0 MED, 2 LOW (S-D1 cosmetic, S-D2 user-accepted); AI-C-SEC-2 resolved this epic |
| Unresolved blockers | None GA-blocking; the real-world smoke gate is the one item standing between "stories done" and "production-ready" |

---

## Closure

Epic D: Singleton Configuration → SQLite — **REVIEWED AND CLOSED**.

The doctrine validation streak now stands at **32× cumulative**. Epic D's standout lessons: (1) D-2's clean iter-3 *after* a control-flow relocation reaffirms that the iter-N+1 mandate is well-bounded — it fires on new code and stands down without it; (2) the Acceptance Auditor remains the only layer that catches silently-skipped documentation deliverables; (3) most importantly, the substring-matcher anti-pattern *regressed inside this epic* despite the Epic C retro explicitly asking to codify the ban — proving the skill-codification debt cannot be paid down by a feature epic and must get its own.

opcgw's configuration story is now complete: three persistence surfaces, `config.toml` inert at runtime, SQLite authoritative, secrets isolated and hardened. The remaining gap to production-ready is operational, not architectural — the deferred real-world validation gate.

**Next mandatory actions (this session):**
1. Commit this retrospective + sprint-status flips (`epic-D` and `epic-D-retrospective` → `done`).
2. `git push origin main` — the checkpoint that makes the closed epic visible.

**Next out-of-band actions (Guy):**
- **AI-D-3** real-world batched validation against the real ChirpStack — before any release tag.
- **AI-D-1** scope + open the v2.x skill-codification + cleanup epic (via `bmad-*` planning skills).
- **AI-C-6 / AI-D-1** GitHub tracking issues for the untracked Epic C + D stories — one-shot batch when gh-CLI write auth is available.

---

*Retrospective facilitated by Bob (Scrum Master) on 2026-05-28. Project Lead: Guy Corbaz. Saved to `_bmad-output/implementation-artifacts/epic-D-retro-2026-05-28.md`.*

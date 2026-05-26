# Epic C — Auto-Discovery and Web-First Configuration — Retrospective

**Date:** 2026-05-26
**Facilitator:** Amelia (Senior Software Engineer)
**Project Lead:** Guy Corbaz
**Epic status:** 6/6 stories `done`; retrospective `done`.

---

## Epic summary

Epic C is the **first post-v2.0-GA epic**, opened 2026-05-21 specifically to deliver the user-experience capability Guy described after his v2.0 GA walkthrough on 2026-05-20: *"opcgw should be a self-driving configuration surface — operators pick by name from ChirpStack inventory, not by UUID from memory."* Final 6-story scope (C-5 MQTT path explicitly removed mid-planning):

- **C-0 — Empty-config bootstrap + first-run setup wizard.** Validator now accepts `application_list.len() == 0`; new web wizard for OPC UA password persists to a new `config/secrets.toml`; OPC UA server rejects all auth until configured. Wizard submit triggers a graceful CancellationToken restart instead of in-place auth hot-reload (design deviation #1, agreed pre-impl).
- **C-1 — ChirpStack inventory query layer + 60s TTL cache.** Server-side helpers + `/api/inventory/*` endpoints fetch application / device / metric inventory from ChirpStack on-demand. Cache MISS-only audit events per the "avoid too much ChirpStack hit" directive.
- **C-2 — Inventory pickers in the web UI.** New shared `static/inventory-picker.js` module (~200 LOC) drives cascading application → device → metric pickers in the existing application + device CRUD pages. Manual-entry fallback toggle preserved. New `POST /api/audit/picker-event` endpoint captures operator picker activity for diagnostic auditing.
- **C-3 — Server-side duplicate-prevention validator.** Reject `application_id` / `device_id` / `metric_name` duplicates at the same scope level (NOT across applications — same DevEUI under different applications is explicitly allowed per the C-3 spec). Audit shape `reason="conflict" conflict_kind="duplicate"` aligns with existing `reason="conflict"` grep contract.
- **C-4 — Inventory drift view.** New `src/web/drift.rs` module owns `compute_drift` + the `inventory_drift` axum handler. 4-class diff (ok / stale / available / drifted) with operator-triggered refresh and deep-links to C-2 pickers via `prefill_*` query params.
- **C-6 — TOML → SQLite configuration migration + SQLite-driven hot-reload (lands LAST).** Schema v009 adds `applications`/`devices`/`metrics`/`commands` tables + `meta` for the migration done-flag. One-shot data migration with row-count verification inside `BEGIN EXCLUSIVE TRANSACTION`. **Story 9-7's TOML file-watcher REMOVED**; hot-reload now fires on CRUD-write completion via `notify_crud_write`. **Story 9-5's `src/web/config_writer.rs` (551 lines) DELETED**; `toml_edit` removed from `Cargo.toml` (the only allowed `Cargo.toml` mutation in Epic C). Migration runbook + verification script (`scripts/check-c6-migration.sh`) shipped. Falls back to TOML on row-count mismatch (transition safety net).

**Velocity:** 6/6 stories across 6 calendar days (2026-05-21 → 2026-05-26). Marathon day on 2026-05-22 (C-0 + C-1 both flipped to done same session). Marathon week culminating on 2026-05-26 (C-6 four-iter review cycle + flip in one session post-PC-restart).

**Test gate at epic close:** `cargo test` 1482 passed / 0 failed / 73 ignored (+178 net new tests across Epic C vs the 1304-baseline post-Epic-B); `cargo clippy --all-targets -- -D warnings` clean.

**Doctrine streak:** 14× → 24× cumulative iter-N+1 validations across the project (Epic C added 10 review iterations — 3 each for C-0/C-1/C-2/C-3 + 1 for C-4 + 4 for C-6 — most of which surfaced real findings that would have shipped silently without the iter-N+1 round).

---

## Per-story summary

### C-0 — Empty-config bootstrap + first-run setup wizard

- **Spec:** 2026-05-21 (commit chain `4f59592` scoping + `afe6869` spec).
- **Implementation:** 2026-05-22 commit `c200089`. Two design deviations agreed pre-impl: (1) wizard submit triggers graceful `CancellationToken` restart instead of in-place auth hot-reload; (2) OPC UA reject-all auth implicit via existing `OpcgwAuthManager::is_configured=false`.
- **Review cycle:** iter-1 `7ec2fc1` + iter-2 `d1d1332` + iter-3 `d84fa7d` + flip `e24ad2a`. **15th cumulative doctrine validation.** Cumulative 49 patches across 3 iters + 22 LOW/MED + 1 user-accepted HIGH deferred. **Final architectural shape:** SHARED `Arc<AtomicBool>` first-run state across `AppState`/`WebAuthState`/`CsrfState`; 4-entry exact-match `WIZARD_BYPASS_EXACT` allowlist; 4 KiB body limit + strict `application/json` Content-Type + same-origin Origin check with default-port normalisation; race-free `compare_exchange` with revert-on-failure; new `SecretsWriteError` enum mapping `io::ErrorKind` variants.
- **New finding classes:** (a) "patch-chain half-implemented across server/client boundary" — iter-2 caught iter-1's centralised `reload_error_response` helper changing the response BODY shape but not the response audit-log shape; (b) "flip-state-before-write-without-revert dead-end" — caught at iter-2 in iter-1's `AtomicBool::store(true)` placement before the disk write.

### C-1 — ChirpStack inventory query layer + cache

- **Spec + impl:** 2026-05-22 commit `b5df23f`. New module-level cache with 60s TTL + `?refresh=true` bypass; cache MISS-only audit emission.
- **Review cycle:** iter-1 `372280f` + iter-2 `3f35ddb` + iter-3 `5840c04` + flip `8ec6656`. **17th cumulative doctrine validation.** Final 1404 / 0 / 10 / clippy clean.
- **New finding classes:** (a) "Substring-matcher attribution leak" — iter-3 caught iter-2's substring-match on a tonic `Cancelled` error message that could be triggered by unrelated cancellation paths; (b) "New audit-emit field is a new injection sink" — iter-3 caught iter-2's new `cache_key=` audit field forwarding unsanitised operator input. **Self-validating doctrine moment in iter-3:** the iter-2 reviewer (also the author of the iter-2 patches) had anticipated the substring-matcher leak in their own Blind Hunter prompt yet iter-2 shipped it anyway. Iter-3 reviewers ARE the safety net even when the author thinks they've considered the edge case.

### C-2 — Inventory pickers in the web UI

- **Spec + impl:** 2026-05-23 commit `5ddfdae`. New shared `static/inventory-picker.js` module (~200 LOC); extended `applications.html`/`.js` + `devices-config.html`/`.js` with cascading pickers + manual-fallback toggle; new `POST /api/audit/picker-event` endpoint + `PickerMetadata` field on `MetricMappingRequest` with `metric_wire_type_inferred` audit emission.
- **Review cycle:** iter-1 `104d46f` + iter-2 `3c9a225` + iter-3 `d5be23f` + flip `98c2d28`. **19th doctrine validation.** 37 patches across 3 iters + 12 LOW/MED + 1 dismiss. Finding-count convergence per iter: 19 → 11 → 7 (signature of a well-bounded review loop).
- **New finding classes:** (a) "Stale-fetch-on-mode-toggle" — mode-toggle handlers must abort in-flight async before flipping UI state; (b) "Partial-fix-collision" — sanitisation-based namespace prefix trades one collision class for another; monotonic counter is the correct fix.

### C-3 — Server-side duplicate-prevention validator

- **Spec + impl:** 2026-05-23 commit `5f5b9a7`. Same-level dup-prevention only (cross-application same DevEUI explicitly allowed). Implementation completed via PC-restart-and-resume after a mid-implementation shell wedge.
- **Review cycle:** iter-1 `4ad945c` (12 patches) + iter-2 `de1c716` (7 patches addressing iter-1 patch-chain regressions) + iter-3 `aed57b4` (5 patches addressing residual parser apostrophe-leak) + flip `65e525d`. **21st doctrine validation.** Closes #107. Final 1437/0/65. 24 patches + 6 user-approved MED defers + 9 LOW auto-defers + 10 dismisses.
- **New finding-class refinements:** (a) "Structural-anchor parsers still need allowlists, not just LAST-match anchors" — iter-2 thought `rfind()` alone closed the substring-attribution leak; iter-3 proved it doesn't (`application_id="foo' is duplicated within bar"` defeated the iter-2 quote-framing because `find()` matched FIRST occurrence inside value); require `rfind` + structural ALLOWLIST on trailing-scope. (b) "Test fixtures lag code refactors" — every patch obviating a scenario must also retire matching test cases.

### C-4 — Inventory drift view

- **Spec + impl:** 2026-05-24 commit `77a73a0`. New `src/web/drift.rs` module owns pure `compute_drift` function + `inventory_drift` axum handler. New `audit_drift_action` handler mirrors C-2 picker-event allowlist pattern. New `static/inventory-drift.{html,js}` page with 4-class collapsible sections + confirmation modal + unreachable banner. Nav strip extended on 6 sites. 13 unit tests + 10 integration tests. **The only Epic C story that did NOT require iter-2 review** — iter-1 patches did not introduce new code that triggered the doctrine mandate.
- **Review cycle:** iter-1 `23117af` (only) + flip `66d45b3`. **22nd doctrine validation opportunity** — the doctrine mandates iter-N+1 only when iter-N introduces new code. C-4's iter-1 patches were doc-only / config / minor; iter-2 was correctly NOT mandated. Loop terminated cleanly after iter-1.

### C-6 — TOML→SQLite configuration migration + SQLite-driven hot-reload

- **Spec + impl:** 2026-05-25 commit `1c09911`. Schema v009 + `migrate_config.rs` + migration runbook + `scripts/check-c6-migration.sh` + 16 integration tests. SIGHUP listener + `config_writer.rs` + `config_hot_reload.rs` deleted. Final 1497/0/55.
- **Review cycle (4 iters — the project's longest):** iter-1 `1a5bb95` (14 patches) + iter-2 `1cf2304` (9 patches) + iter-3 `e59fed2` (2 patches) + iter-4 `6d077d1` (2 doc-only patches) + flip `b4df39b`. **23rd + 24th cumulative doctrine validations** (iter-3 + iter-4 both surfaced real findings that would have shipped silently). Final 1482/0/73 / clippy clean.
- **New finding-class refinement from iter-4:** "Small iter-N patch rounds (8 lines code + docstring) STILL warrant iter-N+1 review." The iter-3 patch round was tiny — 8 lines of flow-control + a docstring update. The pragmatic counter-argument ("reviewer-designed code, low risk, skip iter-4") was rejected; iter-4 caught (a) an overstatement in the warn message ("next boot will retry" — `count_applications()?` still propagates, so the retry guarantee is partial) and (b) an AC#24 doc-sync gap (`stage="already_migrated_backfill_failed"` missing from `docs/logging.md` and `c-6-migration-runbook.md`). **Patch size is NOT a reliable signal for skipping iter-N+1.**

---

## Cross-story synthesis

### What went well

1. **The "name-translation gateway" framing held throughout the epic.** Every UI surface introduced in Epic C (C-2 pickers, C-4 drift view, C-0 wizard) shows operator-facing names — never UUIDs as primary keys. UUIDs appear in detail views and audit logs but never as the picker key. The architectural decision (memory `project_epic_c_auto_discovery_vision.md`) was load-bearing and remained unchallenged across 6 stories.

2. **C-5 MQTT removal mid-planning was the right call.** Originally scoped as a real-time path; removed 2026-05-21 (`CR-EPIC-C-MQTT` in `deferred-work.md`). Reason: MQTT events carry only UUIDs, useless to SCADA operators. Re-promote only on explicit operator request. The epic stayed finishable; MQTT-direct integration would have been a parallel architecture to the picker-driven flow, doubling surface area.

3. **PC-restart-and-resume worked twice without state loss.** Both C-3 (mid-implementation shell wedge) and C-6 (mid-iter-2 patches applied + uncommitted) survived a forced PC restart and resumed cleanly. The session-pause memory entries proved their value as resume manifests — pre-restart state was reconstructed in minutes.

4. **Doctrine validated 10 more times across Epic C (15× → 24×).** Real findings were surfaced in 9 of 17 review iterations — including iter-4 on an 8-line patch round (C-6). The pragmatic "this is too small to need another round" instinct was wrong every time it was tested. Two new finding-class refinements added (C-3 structural-anchor parsers + test-fixture lag; C-6 patch-size-is-not-skip-signal).

5. **Schema migration v009 landed cleanly with the row-count verification + transaction-bounded done-flag.** C-6's idempotency guard is now primary on the `meta` done-flag (resilient to operator deletion of all applications via the web UI), with a secondary guard on row-count > 0 (for direct SQLite imports that bypassed `migrate_applications_config`). The secondary guard back-fills the done-flag on first encounter so future boots use the faster primary path.

6. **Strict-zero invariants preserved across every story.** Each Epic C story declared specific files that must NOT be touched (e.g. `src/opc_ua.rs` was strictly-zero for C-1/C-2/C-3/C-4/C-6). Honored across all stories — verified by per-story `git diff --stat` at flip time.

### What didn't go well

1. **Substring-matcher attribution leak surfaced in 3 separate stories (C-1, C-3, C-6).** Same anti-pattern (string `contains("…")` on `error.to_string()` to classify errors or extract attribution): caught in C-1 iter-3, again in C-3 iter-3 (with structural-anchor refinement needed), and again in C-6 iter-3 (deferred as MED-equivalent with user acceptance after ECH verified current code safe). **The repeated occurrence implies the pattern lives in developer reflex faster than the doctrine reaches; codifying a typed-error-variant policy into the bmad-code-review skill (carry-forward from AI-B-8) is overdue.**

2. **The `main.rs` deadlock incident (2026-05-20) was the doctrine's first NEGATIVE validation** — and it occurred mid-Epic-C-planning, not mid-Epic-C-implementation. Per memory `incident_main_deadlock_2026_05_20`: a structural deadlock at `src/main.rs:740` had been latent for ~30 days, survived 14× iter-N+1 validations, B-1 Docker smoke test, full `cargo test`, and clippy. Caught only by Guy's batched real-world test (AI-B-7) against his real ChirpStack. Epic C's batched-validation strategy for real-world testing was deliberately conservative as a result — manual real-world smoke against Guy's actual ChirpStack was deferred per the main-deadlock-incident doctrine for C-2, C-3, C-4, C-6 (only C-1 received explicit smoke testing).

3. **The marathon C-0 + C-1 same-session day (2026-05-22) was high-throughput but cognitively heavy.** Both stories went through 3 iter rounds in the same conversation; memory entries from that session (`session_pause_2026_05_22_C0_done_C1_ready` superseded by `session_pause_2026_05_22_C1_done_pushed`) show three layered supersedings — a sign that the working-state model drifted faster than usual. C-1 iter-3's "self-validating doctrine moment" (the author anticipated the substring-matcher leak in their own prompt yet iter-2 shipped it) is the strongest evidence that even author-aware reviews need fresh-eyes iter-N+1.

4. **AI-A-5 (Issue #102 — `tests/common` extraction) still not addressed.** Carryover from Epic 9-5/9-4/A/B. Each post-Epic-9 story has added tests that import from `tests/common/mod.rs`; the file is now 400+ lines with multiple distinct helpers (`make_test_reload_handle`, etc.). Extraction was deferred at Epic B as v2.x; remains deferred at Epic C. No GA-blocking risk (the file works), but every new test story re-encounters the cognitive load.

5. **GitHub tracking issues not opened for C-2/C-4/C-6.** Per CLAUDE.md "Issue Management": every story should be tracked. C-3 closed #107 (the only Epic C story with a tracked issue). C-0/C-1/C-2/C-4/C-6 all opted out of tracking-issue creation, citing "user opens out-of-band" — but none were actually opened. **Pattern carries forward from Epic A precedent (gh CLI not authenticated for write in agent sessions).**

### Epic B retro follow-through

| AI item | Description | Status post-Epic C | Evidence |
|---|---|---|---|
| AI-B-1 | Codify "inline-comment misdirection" check in skill | ❌ Not addressed | Skill source untouched; doctrine still memory + CLAUDE.md |
| AI-B-2 | Document "deferral-impact misclassification" pattern in skill | ❌ Not addressed | Same |
| AI-B-3 | Verify workflow secret references against actual repo secrets | ❌ N/A this epic | No workflow changes in Epic C |
| **AI-B-4** | **Cargo bump `2.0.0-rc` → `2.0.0` (pre-tag gate)** | ✅ Done | v2.0.0 + v2.0.1 + v2.0.2 published 2026-05-20/21 (per memory `session_pause_2026_05_20_v201_published`) |
| **AI-B-5** | **Rename `DOCKER_USERNAME` → `DOCKERHUB_USERNAME`** | ✅ Done | Verified 2026-05-20 per Epic B retro session memory |
| **AI-B-6** | **Verify `DOCKERHUB_TOKEN` not expired** | ✅ Done | Verified 2026-05-20 Never/R+W+Delete scope |
| **AI-B-7** | **End-to-end real-world test (ChirpStack + OPC UA)** | ✅ Done with incident | Caught the `main.rs` deadlock 2026-05-20 — fix in commit `917d634` |
| AI-B-8 | Skill codification (AI-A-1/2/3 + AI-B-1/2/3) | ❌ Not addressed | v2.x carry-forward; Epic C does not move it |
| AI-B-9 | SHA-pin GH Actions (S5 follow-up) | ❌ Not addressed | v2.x |
| AI-B-10 | Hardcoded `gcorbaz/opcgw` repo-coupling | ❌ Not addressed | v2.x |
| AI-B-11 | `feedback_iter3_validation` memory updates | ✅ Done | Memory now reflects 24× streak + C-3 / C-6 finding-class refinements |
| AI-B-12 | Save retrospective doc | ✅ Done | Epic B retro committed `0113ff6` |

**Score: 6/12 done (incl. all v2.0 GA gates from Epic B); 1 N/A this epic; 5 v2.x carry-forward (skill codification = stable 3-epic carry-forward — clearly needs a dedicated story).**

---

## Security review

Conducted inline by an automated subagent (Sonnet 4.6) on the iter-N+1 review-clean state of the Epic C commits per CLAUDE.md "Epic Completion Requirements." **Verdict: CLEAN — 0 HIGH, 0 MEDIUM, 3 LOW.** All Epic-C-introduced surfaces are secure; the 3 LOW findings are either pre-existing patterns (S01, S02) or residual hardening on an iter-1-patched code path (S03).

### Check summary

| # | Check | Result |
|---|---|---|
| 1 | Hardcoded credentials/secrets | CLEAN |
| 2a | ChirpStack API input validation (`fetch_applications`/`fetch_devices` pagination cap, typed protobuf de/serialize, `UPLINKS_LIMIT_CAP=50`) | CLEAN |
| 2b | OPC UA writes (no new attack surface in Epic C) | CLEAN |
| 2c | Config / migration boundary validation (`params![]`, EXCLUSIVE transaction row-count verify, parameterised done-flag) | CLEAN |
| 2d | Web inputs (`#[serde(deny_unknown_fields)]`, event-name allowlists, 4 KiB body limits, 256-byte UTF-8-safe field caps) | CLEAN |
| 2e | Path traversal (`secrets_path` operator-controlled; `static_dir.join` server-controlled) | CLEAN |
| 3 | Error message leakage (HTTP responses use generic 500; raw errors logged at warn only) | 1 LOW (S03) |
| 4 | SQL injection (all Epic C SQL uses bound params) | 1 LOW (S01 — dead code) |
| 5 | Command injection (no `std::process::Command` in Epic C; `check-c6-migration.sh` uses quoted `"$DB"`) | CLEAN |
| 6a | C-0 wizard access control (`WIZARD_BYPASS_EXACT` 4-entry exact-match; race-free `compare_exchange`; post-first-run `410 Gone`) | CLEAN |
| 6b | C-1/C-2/C-3/C-4 endpoint auth (inherit Basic-Auth + CSRF stack from `build_router`) | CLEAN |
| 6c | SQLite file permissions (inherit process umask — pre-existing, now load-bearing for C-6 config data) | 1 LOW (S02) |
| 7 | CSRF / Origin enforcement (all POST endpoints behind CSRF; only `/api/setup/password` first-run exemption, gated by `is_first_run` + strict Content-Type + same-origin Origin) | CLEAN |
| 8 | Unsafe code in Epic C-introduced files | CLEAN (pre-existing `unsafe impl Send/Sync` in `pool.rs` is not new) |
| 9 | SPDX license headers on new files | CLEAN |
| 10 | Migration data integrity (row-count verify inside EXCLUSIVE transaction; primary+secondary guard; ROLLBACK on mismatch) | CLEAN |

### Findings

- **S01 (LOW) — SQL string interpolation in `prune_old_metrics` (pre-Epic C, dead code)** [`src/storage/sqlite.rs:2366-2371`] — `format!("DELETE FROM metric_history WHERE timestamp < datetime('now', '-{} days')", retention_days)` interpolates `retention_days` (a `u32` validated 7–365 in `AppConfig::validate`). No actual injection possible at runtime, but unsafe-by-style. Introduced in Story 8-3 (pre-Epic C); function annotated `#![allow(dead_code)]`; no live call site. Tracked as **AI-C-SEC-1** (v2.x).
- **S02 (LOW) — SQLite database file permissions inherit process umask** [`src/storage/pool.rs:72`] — `Connection::open(path)` uses default umask (typically 0o022 → world-readable 0o644). Pre-Epic C, now load-bearing because C-6 stores application/device/metric configuration topology in `data/opcgw.db`. `secrets.toml` is correctly chmod 0o600 via atomic-rename. Tracked as **AI-C-SEC-2** (v2.x — hardening pass + documented deployment note: Docker volume permissions, systemd `UMask=0077`).
- **S03 (LOW) — `setup_get` error path logs full filesystem path at warn level** [`src/web/setup.rs:213-217`] — Analogous `setup_post` paths were patched in iter-1 M2 to log only the filename; `setup_get` failure branch was not updated. Leaks deployment filesystem layout (Docker WORKDIR, host path mappings). Tracked as **AI-C-SEC-3** (v2.x — apply iter-1 M2 pattern: `.file_name()` only).

None of S01/S02/S03 block Epic C close per CLAUDE.md loop discipline (only LOW remains). All three captured as v2.x carry-forward action items below.

---

## Action items

### Process improvements

**AI-C-1 — Codify "substring-matcher attribution leak" into the `bmad-code-review` Blind Hunter prompt.**

- Owner: Project Lead (Guy)
- Description: The pattern has now surfaced in 3 separate Epic C stories (C-1 iter-3, C-3 iter-3, C-6 iter-3). Codify in the skill source itself, not just memory: add a Blind Hunter checklist item — "for any `error.to_string().contains(...)` or `.starts_with(...)` / `.ends_with(...)` in error-classification or attribution code, flag as substring-matcher anti-pattern unless a typed-variant alternative is documented."
- Success criteria: future stories with error-handling code surface the pattern at iter-1, not iter-3.
- Joins AI-A-1/2/3 + AI-B-1/2/3 + AI-B-8 as long-standing skill-codification carry-forward.

**AI-C-2 — Add "patch-size-is-not-skip-signal" guard to iter-N+1 mandate.**

- Owner: Project Lead (Guy)
- Description: C-6 iter-3 patch round was 8 lines and felt skippable; iter-4 caught 2 real findings. Update the CLAUDE.md doctrine wording to explicitly state: "iter-N+1 mandate is based on new-code-introduction, not patch size. An 8-line flow-control change is new code." (Currently the wording says "when iter-N introduces brand-new code (parsers, classifiers, flow-control), iter-N+1 is MANDATORY not optional" — the "brand-new" qualifier is ambiguous and was the basis for the pragmatic skip argument.)
- Success criteria: future small-patch iter rounds default to iter-N+1 without re-litigating.

**AI-C-3 — Track real-world batched validation as an explicit pre-close-epic gate.**

- Owner: Project Lead (Guy)
- Description: Per the `main.rs` deadlock incident, real-world end-to-end testing catches latent issues that no review layer surfaces. Add to the CLAUDE.md "Epic Completion Requirements" alongside the security check + tests-pass check: "Before retrospective, run gateway against real ChirpStack + real OPC UA client for at least one full poll cycle and confirm metric flow." For Epic C: per the conservative deferral post-2026-05-20 deadlock, this is **deferred to Guy's discretion** but flagged as the highest-leverage carry-forward.
- Success criteria: every future epic ends with a documented real-world smoke result before retrospective.

### Technical debt — v2.x post-GA

- **AI-C-4 — Substring-matcher cleanup pass.** Issue #102's tests/common extraction is one half of a broader v2.x cleanup; the substring-matcher pass is the other half. Both should become explicit v2.x stories so they don't disappear into rolling carry-forward.
- **AI-C-5 — Skill codification (now 8 cumulative items: AI-A-1/2/3 + AI-B-1/2/3 + AI-C-1/2).** Three epics of carry-forward; promote to a dedicated v2.x story.
- **AI-C-6 — GitHub tracking issues for Epic C stories C-0/C-1/C-2/C-4/C-6** (per CLAUDE.md "Issue Management"). One-shot batch via `gh issue create` when Guy's session has gh-CLI write auth.

### Security follow-ups — v2.x carry-forward

- **AI-C-SEC-1** — Replace `format!`-built SQL in `prune_old_metrics` (`src/storage/sqlite.rs:2366`) with `params![]` bound parameter. Safety-by-design fix; current code path is dead and `u32`-bounded, so no exploit risk today.
- **AI-C-SEC-2** — Tighten SQLite database file permissions to 0o600 after creation in `src/storage/pool.rs:72`. Now load-bearing for C-6 config data. Either set permissions on first open OR document the deployment requirement (Docker volume mode, systemd `UMask=0077`) in `docs/deployment-guide.md`.
- **AI-C-SEC-3** — Apply iter-1 M2 pattern to `setup_get` error log at `src/web/setup.rs:213-217`: extract `.file_name().and_then(|n| n.to_str())` instead of `setup_html_path.display()` so the warn line carries only the filename, not the full deployment path.

### Memory updates

- **AI-C-7 — Update `feedback_iter3_validation` memory entry** to reflect 24× cumulative streak + the two new C-3 / C-6 finding-class refinements (structural-anchor parsers still need allowlists + small patch rounds still warrant iter-N+1).
- **AI-C-8 — Save retrospective doc** (this commit).

---

## Next epic preparation

**No Epic D defined.** Epic C is the last epic in `_bmad-output/planning-artifacts/epics.md` (11 epics total: 1–9 numeric + A/B/C lettered). The "Next Epic Preview" workflow step is skipped.

This is the natural moment to choose between:

1. **Open Epic D** — define a new scope (e.g. consolidated `[chirpstack]`/`[opcua]`/`[web]`/`[global]` singleton-config-to-SQLite migration; web UI for the singletons; or the deferred `CR-EPIC-C-MQTT` real-time path if operator demand has shifted).
2. **Land a v2.x cleanup epic** that bundles AI-C-4/AI-C-5/AI-C-6 + issue #102 + the skill-codification carry-forward + the substring-matcher cleanup pass.
3. **Pause epic planning** until operator feedback on the v2.0.x deployments arrives. Epic C's deliverables (auto-discovery wizard + pickers + drift view + SQLite migration) are substantive enough that real-world adoption should drive the next epic's scope.

**Recommendation:** option 3 with an option-2 contingency. The Epic C delivery is large; real-world feedback should shape what comes next before more scope is added.

---

## Readiness assessment

| Dimension | Status |
|---|---|
| Testing & quality | ✅ `cargo test` 1482/0/73; `cargo clippy --all-targets -- -D warnings` clean; doctests 0/55 ignored (baseline unchanged) |
| Deployment | ⏳ 6 unpushed Epic C commits + 1 unpushed C-6 retro commit (this) — push pending |
| Stakeholder acceptance | ✅ Project Lead (Guy) participated in all 6 stories' iter-N rounds |
| Technical health | ✅ Strict-zero invariants honored per-story; no `unsafe` blocks introduced; all new SPDX headers present (verified by security review subagent) |
| Real-world end-to-end | ⏳ Deferred per the `main.rs` deadlock incident's batched-validation doctrine — Guy's call when to run |
| Security | ✅ CLEAN — 0 HIGH, 0 MEDIUM, 3 LOW (S01/S02/S03 captured as AI-C-SEC-1/2/3 v2.x); zero Epic-C-introduced regressions |
| Unresolved blockers | None GA-blocking; 5 v2.x carry-forward action items captured above |

---

## Closure

Epic C: Auto-Discovery and Web-First Configuration — **REVIEWED AND CLOSED**.

The doctrine validation streak now stands at **24× cumulative** across 7 surface types (storage refactor / integration rewrite / docs+script / YAML+Markdown+DocBook / web wizard / inventory API + cache / SQLite migration). Two new finding classes added during Epic C (C-3 structural-anchor parsers need allowlists; C-6 small patch rounds still warrant iter-N+1) push the doctrine into deeper code-quality territory beyond simple regression catches.

The "name-translation gateway" framing — operators see names not UUIDs — survived every story unchallenged. C-6's TOML→SQLite migration consolidates configuration writes onto a single canonical store, retiring two prior modules (Story 9-7's TOML file-watcher + Story 9-5's `toml_edit` config writer) without backwards-compatibility shims.

The post-v2.0-GA epic pattern (smaller scope, faster iteration, more reviewer-layer cycles per story) proved efficient relative to Epic 9's 8-story marathon. C-4 demonstrated that single-iteration termination is possible when iter-1 patches don't introduce new code — the doctrine's mandate is well-bounded.

**Next mandatory actions (this session):**

1. Commit this retrospective + sprint-status flips (`epic-C` and `epic-C-retrospective` both → `done`).
2. `git push origin/main` — checkpoint that makes Epic C visible to the team.

**Next out-of-band actions (Guy):**

- AI-C-3 real-world batched validation against Guy's actual ChirpStack — Guy's discretion when to run.
- AI-C-6 GitHub tracking issues for C-0/C-1/C-2/C-4/C-6 — one-shot batch via `gh issue create`.
- Decide between option-1 (open Epic D), option-2 (v2.x cleanup epic), or option-3 (pause for operator feedback) — see "Next epic preparation" above.

---

*Retrospective facilitated by Amelia (Developer) on 2026-05-26. Project Lead: Guy Corbaz. Saved to `_bmad-output/implementation-artifacts/epic-C-retro-2026-05-26.md`.*

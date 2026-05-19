# Epic A Retrospective — Storage Payload Migration (Phase B Closure)

**Date:** 2026-05-19
**Facilitator:** Amelia (Developer, BMad bmad-retrospective skill)
**Project Lead:** Guy
**Epic duration:** 2026-05-14 → 2026-05-18 (~5 calendar days)
**Stories:** A-1 through A-7 — all `done`
**Outcome:** Issue #108 fully closed end-to-end; v2.0 GA gate cleared per Epic 9 retro AI6.

---

## Epic summary

| Dimension | Result |
|---|---|
| Stories completed | 7 / 7 (100%) |
| Iteration cadence | 3-iter on A-1/A-2 → settled at 2-iter for A-3 through A-7 |
| Total review patches applied | ~150 (avg ~21/story; range 16 → 27) |
| `cargo test` final | 1256 passed / 0 failed / 10 ignored (+131 from pre-Epic-A 1125 baseline) |
| `cargo clippy --all-targets -- -D warnings` | Clean |
| Doctest baseline | 55 ignored (drift from 56 via deliberate A-1 iter-2 IR7 removal) |
| HIGH-severity production issues caught by iter-2 | At least 11 (A-3 IR2-B, A-4 JR1, A-5 K1+K2, A-6 K1-K6, A-7 L1+L2) — all would have shipped without iter-2 |
| Security review (this retro) | CLEAN with one LOW finding, patched inline |

**Business outcome:** opcgw now persists and round-trips real measurement values for the first time in the project's history (`Float(23.5)` instead of the string `"Float"`). v2.0 GA blocker per Epic 9 AI6 cleared.

---

## Per-story summary

### A-1 — MetricType Payload-Bearing Enum
Type-level surgery: `MetricType` became `Float(f64) / Int(i64) / Bool(bool) / String(String)`, `Copy` dropped. Cascaded `.clone()` / borrows / move-semantics across ~28 files; perl bulk substitution + manual sweep. **Option B staging discipline** (preserve discriminant-string write path with `TODO(A-2)` markers) was the load-bearing decision of the whole epic. Review: 3 iterations / 23 patches. Iter-2 caught HIGH-REG IR1 (trait-doc split-brain false invariants). Iter-3 dropped 4 false positives + 4 small LOW patches.

### A-2 — SQLite Schema Migration v007
Strictly additive DDL: `value_real/value_int/value_bool/value_text/value_type` columns added to both `metric_values` and `metric_history` with column-level CHECK constraints. Pre-existing rows default to `value_type='legacy'`. Writers/readers untouched. Review: 3 iterations / 16 patches. Iter-1 surfaced HIGH IH1 (migration-runner non-atomicity, pre-existing v001-v006, user-confirmed deferral). Iter-3 K1 consolidated test-count narrative drift across README + spec + sprint-status.

### A-3 — Poller Value-Payload Write Pipeline
Central enabling story: wired real payload through all 7 `TODO(A-3)` sites in `chirpstack.rs::prepare_metric_for_batch`; rewired all 4 SqliteBackend writers; added v008 migration (`CREATE TABLE ... AS SELECT` with cross-column CHECK, BEGIN/COMMIT-wrapped); chose option (a) NaN/Inf filter at poller. Review: 2 iterations / 22 patches. Iter-1 caught HIGH IR1 reversion (Counter monotonic typed-path branch was premature — get_metric_value still read legacy column returning zero-default). Iter-2 surfaced **convergent HIGH IR2-B** (Unknown+cfg=Int saturation gap, kind-only predicate) + **MED IR2-A** (i64::MAX boundary off-by-one).

### A-4 — OPC UA Read Value-Payload Pipeline
Rewired `SqliteBackend::get_metric_value` / `get_metric` / `load_all_metrics` to project typed columns via new `metric_type_from_typed_columns` helper; legacy rows surface as `Ok(None)` → transitively map to `BadDataUnavailable`. `convert_metric_to_variant` rewritten to pattern-match typed payload. Closed 6 `tests/opcua_subscription_spike.rs` seed sites that had A-1-iter1-DEF13 "tests passing for wrong reason" hazard. Review: 2 iterations / 27 patches (widest reader rewrite). Iter-2 caught HIGH-REG **JR1 fake regression-guard test** — new finding class first identified here: test seeded `value: "100"` paired with `Int(100)`; if dropped legacy fallback `prev_metric.value.parse::<i64>()` were restored, both old and new paths would satisfy the assertion.

### A-5 — OPC UA HistoryRead Value-Payload Pipeline
**The story that fully closed #108.** Rewired `query_metric_history` to project typed columns; restructured `HistoricalMetricRow` to `payload: Option<MetricType>` (legacy rows become first-class `None`); rewrote `build_data_values` to pattern-match payload directly; **removed `MetricValue.value: String`** (SemVer-major). Added compile-time field-shape pins (`const _: fn(&T) = |v| { let MetricType { ... } = v; }`). New audit events: `metric_history_read` + `metric_history_summary` (aggregate-per-request log discipline). Review: 2 iterations / 22 patches. Iter-2 caught **2 HIGH-REGs (K1+K2) — same JR1 class**: tests claimed to guard `build_data_values` regression but never invoked it.

### A-6 — Web UI Live-Metrics Value Display
Widened `MetricView` to `value: Option<serde_json::Value>` + new `unit: Option<String>`; retired the `metric_view_display_string` shim; new `metric_type_to_json_value` helper (Float→number, Int→number, Bool→boolean, String→string); widened `MetricSpec` with `metric_unit`; updated `static/metrics.js` with new `formatValue` helper. Bool wire shifts to native `true`/`false`. Dropped `PartialEq, Eq` from response structs (serde_json::Value doesn't impl Eq over NaN). Review: 2 iterations / 22 patches. **Iter-2 surfaced 6 HIGH-REGs (K1-K6)** — most critical: K1 `docs/logging.md` missing `int_precision_lossy` row (AC#14 doc-sync violation), K2 f32 subnormal-underflow produced 40-character decimal strings, K4 NBSP whitespace trim silently destroyed locale typography, K6 grep guard structurally broken.

### A-7 — Migration Runbook and Version-Gated Script
Documentation-dominant final story. Added `## Epic A migration` section (+186 lines) to `docs/deployment-guide.md`: pre-upgrade checklist, Path A (in-place) / Path B (drop-and-recreate), post-migration verification, rollback contract, SLA expectation, 6 common gotchas. New POSIX `scripts/check-schema-version.sh` (first `scripts/` entry). New end-to-end test `test_v006_to_v008_full_upgrade_path_under_5s`. Zero Rust production-code changes. Review: 2 iterations / 18 patches. **Iter-2 surfaced 2 HIGH-REG phrase-harmonization-drift findings** (L1: spec AC#1 step 4 still referenced stale `event="poll_cycle_complete"` after K2 patched the runbook; L2: spec AC#4 referenced wrong path after K3 patched it). **First doc-dominant story to validate the iter-2/iter-3 over-reviewing doctrine** — the doctrine is shape-identical across code and prose.

---

## Cross-story synthesis

### What went well

1. **Option B staging discipline (A-1)** was the load-bearing decision of the whole epic. Without it, the storage-trait refactor would have rippled into every reader simultaneously and made each story unreviewable.
2. **Helper-extraction at the 3-site threshold held.** `metric_type_from_typed_columns` (A-4) was reused by A-5 unchanged; `metric_type_to_json_value` (A-6) is the symmetric write-direction sibling. Single source of truth for the discriminant lexicon at every boundary.
3. **`HistoricalMetricRow.payload: Option<MetricType>` (A-5)** eliminated the stringly-typed sentinel hack. Legacy rows became first-class `None`. Pattern-match once at the OPC UA layer.
4. **Compile-time field-shape pins via destructure (A-5 P1).** `const _: fn(&T) = |v| { let MetricType::Float(a) = v else { return; }; ... }` forces compile errors on field additions. Closed A-1's DEF9/DEF15/DEF19 invariant-test gaps in one shot.
5. **Iter-3 over-reviewing doctrine validated 13×.** Memory entry `feedback_iter3_validation` extended from 12 → 13 stories with A-7 — the first doc-dominant confirming case. The doctrine is shape-identical across code and prose.
6. **Aggregate-per-request audit events** (`metric_history_summary`, `metric_view_serialize`) prevent N-rows × 6/min warn-line floods on dashboard polling. Cleaner operator logs at no contract cost.
7. **Source-of-AC cross-reference rule (A-7 iter-2 L1+L2)** — the iter-N+1 reviewer rule for phrase-harmonization-drift is now codified across both code and prose work.

### What didn't go well

1. **"Fake regression-guard test" — new finding class** first identified in A-4 iter-2 JR1, with three subsequent confirmations: A-5 K1, A-5 K2, A-6 P12. Without iter-2, all four would have shipped as silent regression debt. **Rule now codified:** every regression-guard test must invoke the function under test directly AND use seeds that produce DIFFERENT outputs through the surviving vs dropped path.
2. **Phrase-harmonization-drift between code, runbook, and spec source-of-AC text.** Appeared in A-2 (test-count narrative across README/spec/sprint-status), A-3 (line-number-stale comments), A-4 (3 stale "Schema drift:" assertions), A-5 K5 (orphan `reason_detail` reference), culminating in A-7 L1+L2. **Iter-N+1 reviewer rule:** cross-reference every patched string against its source-of-AC location.
3. **Closed-enum field schemas need kind+cfg combined matches, not kind-only.** Bit Epic A three times (A-3 IR2-B, A-3 IR4, A-6 K1). Each new closed-enum value needs codified doc-sync in the same commit.
4. **Test-fixture cascade pain compounded across A-1, A-4, A-5.** Issue #102 (`tests/common` extraction) was flagged in Epic 8 retro, kept open in Epic 9 retro, and bit us repeatedly in Epic A. **Three retros now** — needs to be a hard commitment in v2.x.

### Epic 9 retrospective follow-through

| AI | Description | Status | Note |
|---|---|---|---|
| AI1 | Pre-commit gate when story is review with uncommitted spec | ❌ Not implemented | Discipline held without tooling |
| AI2 | Update `bmad-dev-story` Step 9 to suggest commit before exit | ❓ Unclear | No evidence in Epic A trail |
| AI3 | Document per-transition commit pattern in CLAUDE.md | ✅ Present | "BMad Workflow Commit & Push Discipline" section is authoritative |
| AI4 | Spike reports validate prose with integration tests | ⏳ N/A | No spike stories in Epic A |
| AI5 | Extract `tests/common/opcua.rs` (issue #102) | ❌ Not done | **Compounded cost during Epic A** |
| **AI6** | **Open Epic A — Storage Payload Migration** | ✅ **DONE** | **The single most important AI6 commitment landed** |
| AI7 | Triage `deferred-work.md` before Epic A | ❓ Partial | Some Epic 9 carry-forwards closed by Epic A |

**Verdict:** AI6 — the most important commitment — landed cleanly. **AI5 is the most consequential miss** because it directly compounded test-fixture cascade pain in A-1/A-4/A-5.

### Doctrine validation (iter-2 / iter-3 over-reviewing)

Memory entry `feedback_iter3_validation` extended from 12 → 13 stories. Critical iter-2 catches in Epic A alone:
- **A-3 iter-2 IR2-B (HIGH conv):** Unknown+cfg=Int saturation gap that iter-1 missed — silently produced `i64::MAX` from f32=1e30 inputs.
- **A-4 iter-2 JR1 (HIGH-REG):** Fake regression-guard test class identified for the first time.
- **A-5 iter-2 K1+K2 (HIGH-REG ×2):** Same JR1 class — tests claimed to guard `build_data_values` but never invoked it.
- **A-6 iter-2 K1-K6 (6 HIGH-REGs):** AC#14 doc-sync; f32 subnormal-underflow 40-char strings; f32 overflow at wrong log level; NBSP whitespace silently stripped; empty-string holes; structurally broken grep guard.
- **A-7 iter-2 L1+L2 (HIGH-REG ×2):** Phrase-harmonization-drift in the first doc-dominant story.

**Without iter-2, every one of these would have shipped.** Memory now permanently records A-7 as the 13th confirming case + first doc-dominant.

### Security review

**Conducted inline during this retrospective per CLAUDE.md "Epic Completion Requirements".**

Verdict: **CLEAN with one LOW finding, patched inline.**

- ✅ Strict-zero invariant compliance (no auth/security/main::initialise_tracing touched)
- ✅ No hardcoded credentials / secrets
- ✅ SQL parameterization (`?N` placeholders throughout; no injection surface introduced)
- ✅ `scripts/check-schema-version.sh` clean (POSIX `/bin/sh`, `set -eu`, quoted args, no `eval`, no destructive ops; pre-flight table-existence check prevents misidentifying non-opcgw SQLite files)
- ✅ Migration DDL clean (v007 + v008 pure DDL; v008 BEGIN/COMMIT-wrapped)
- ✅ New structured-log closed enums are static literals; no PII/secret flow
- ✅ Web display path safe (`document.createTextNode` — no XSS sink)
- ✅ NaN/Inf + i64-saturation guards in `chirpstack.rs::prepare_metric_for_batch` sound at boundaries
- ✅ `metric_type_from_typed_columns` defense-in-depth on schema drift

LOW finding patched: `docs/deployment-guide.md:285` Gotcha #6 had `rm ./opcgw.db*` glob (could remove `opcgw.db.bak.YYYY-MM-DD` backups); replaced with explicit `rm ./opcgw.db ./opcgw.db-wal ./opcgw.db-shm` form.

---

## Action items

### Process improvements

**AI-A-1 — Codify "iter-N+1 reviewer rule" in `bmad-code-review` skill.**
Cross-reference every patched string against its source-of-AC location (spec, runbook, code, `docs/logging.md`, sprint-status). Phrase-harmonization-drift bit Epic A 5 times.
*Owner:* Guy (skill author). *Deadline:* before next BMad workflow run.

**AI-A-2 — Codify "fake regression-guard test" check in `bmad-code-review` Edge Case Hunter prompt.**
Every regression-guard test must invoke the function under test directly AND use seeds that produce different outputs through the surviving vs dropped path.
*Owner:* Guy. *Deadline:* before next BMad workflow run.

**AI-A-3 — Codify "closed-enum doc-sync" check.**
When code adds a new value to a closed enum (`event=`, `reason=`, etc.), `docs/logging.md` MUST be updated in the same commit. A-6 K1 was the canonical miss.
*Owner:* Guy. *Deadline:* before next BMad workflow run.

### Technical debt — GA-blocking

**AI-A-4 — CHANGELOG entry for `MetricValue.value: String` removal (A-1-iter1-DEF15).**
SemVer-major retire affecting any external Rust consumer of `opcgw::storage`.
*Owner:* Guy. *Deadline:* **before v2.0 GA tag.** *Effort:* small.

### Technical debt — v2.x post-GA

**AI-A-5 — Issue #102 (`tests/common` extraction) — HARD COMMITMENT.**
Three retros in a row (Epic 8, Epic 9, Epic A). 6+ callers now.
*Owner:* Guy. *Deadline:* **first v2.x story after GA tag** (no further deferral). *Priority:* HIGH.

**AI-A-6 — Issue #100 (doctest baseline cleanup).**
55 ignored doctests; baseline drifts every epic.
*Owner:* Guy. *Deadline:* first v2.x doc-pass story. *Priority:* MEDIUM.

**AI-A-7 — A-2 IH1 (migration-runner non-atomicity).**
HIGH severity, pre-existing across v001–v008.
*Owner:* Guy. *Deadline:* before next migration story. *Priority:* HIGH (deferred, not dropped).

**AI-A-8 — Manual XML user manual sync (4 epics behind).**
Standing deferred-work line 218. A-7 explicitly punted.
*Owner:* Guy. *Deadline:* first v2.x doc-pass story. *Priority:* MEDIUM.

**AI-A-9 — v008 SLA optimization for large databases (A-7 D1).**
Linear scaling: ~25 min for 500k rows. Runbook warns about it.
*Owner:* Guy. *Deadline:* when an operator reports large-DB pain. *Priority:* LOW.

### Memory updates

**AI-A-10 — Update `feedback_iter3_validation` memory.**
Already records 13-story validation; confirm A-7 doc-dominant case is captured.
*Owner:* Auto-memory. *Status:* complete (existing entry covers it).

**AI-A-11 — Save new memory: "fake regression-guard test" pattern.**
New finding class identified during Epic A. Worth preserving for future code-review prompts.
*Owner:* Auto-memory. *Deadline:* end of this retrospective.

---

## v2.0 GA release readiness (replaces "Next Epic Preparation")

There is no Epic B defined. Epic A is the v2.0 GA gating epic. The "next epic" landscape is the **v2.0 GA release itself**.

### Critical path before `v2.0` tag

1. ✅ Epic A complete (7/7 stories done)
2. ✅ `cargo test` clean (1256/0/10), `cargo clippy` clean
3. ✅ Operator migration runbook in place (`docs/deployment-guide.md § "Epic A migration"`)
4. ✅ Security check per CLAUDE.md "Epic Completion Requirements" — completed in this retrospective; clean with one LOW finding patched
5. ⏳ CHANGELOG entry for `MetricValue.value` retire (AI-A-4) — single commit, small
6. ⏳ `git push` to origin/main — 5 unpushed commits per CLAUDE.md "After an epic retrospective" rule
7. ⏳ Tag `v2.0` once items 5-6 land

### Deliberately out of v2.0 GA scope (post-GA v2.x increments)

- Issue #102 (tests/common), #100 (doctest baseline), #88 (per-IP rate limiting), #104 (TLS hardening), #110 (RunHandles Drop), #113-116 (per-section hot-reload follow-ups)
- Migration-runner atomicity (AI-A-7)
- v008 SLA tuning (AI-A-9)
- Manual XML sync (AI-A-8)
- Story 8-4 revival (threshold alarms — now functionally unblocked)
- OPC UA EngineeringUnits attribute exposure (deferred from A-6)

### Significant discoveries (do they require scope change?)

| Discovery | Affects GA? | Recommendation |
|---|---|---|
| MetricValue.value retire is SemVer-major | Yes — narrative | CHANGELOG entry (AI-A-4) |
| f32-cast wire-format shim is load-bearing | No | Documented; future "high-precision metric" story re-evaluates |
| i64 > 2^53 JS precision loss is observable | No | `metric_view_serialize reason=int_precision_lossy` telemetry in place |
| OPC UA EngineeringUnits not exposed | No — known limitation | v2.x follow-up issue |
| Migration runner non-atomic (IH1) | No — operator-known | AI-A-7 before v009 |
| v008 SLA gap on large DBs | No — runbook documents it | AI-A-9 if reported |
| Manual XML 4 epics behind | No — punted | AI-A-8 v2.x doc-pass |
| `scripts/check-schema-version.sh` is first scripts/ entry | No | Minor GA narrative addition |

**Verdict: NO scope change required for v2.0 GA.** Epic A delivered exactly what AI6 specified.

---

## Readiness assessment

| Dimension | Status |
|---|---|
| Testing & quality | ✅ 1256/0/10 + clippy clean + doctest 0/55 |
| Deployment | ⏳ Not yet tagged; 5 unpushed commits on main |
| Stakeholder acceptance | ✅ Project Lead (Guy) confirmed synthesis accurate during this retro |
| Technical health | ✅ Codebase health strong: helper boundaries clean, closed-enum discipline tightening, schema drift defense-in-depth, payload-bearing types compile-time pinned |
| Unresolved blockers | ⏳ AI-A-4 (CHANGELOG) is the only GA-blocking item remaining; small |
| Security | ✅ Clean with one LOW finding patched inline |

**Epic A is functionally complete and v2.0-GA-ready** pending the trivial CHANGELOG entry + push.

---

## Closure

Epic A: Storage Payload Migration (Phase B Closure) — **REVIEWED AND CLOSED**.

**Key takeaways:**
1. Option B staging is the right discipline for type-system surgery that cascades widely
2. Iter-2/iter-3 over-reviewing doctrine is now 13-story validated and shape-identical across code and prose
3. "Fake regression-guard test" is a new finding class to watch for in every future code review
4. Closed-enum doc-sync needs to be a same-commit requirement, not a separate doc-pass story
5. Issue #102 must land in v2.x — three retros of deferral is the limit

**Commitments made today:** 11 action items, 0 preparation tasks (no next epic), 2 critical-path items before v2.0 GA tag (CHANGELOG + push).

**Next steps:**
1. Save and commit this retrospective
2. Add CHANGELOG entry (AI-A-4)
3. `git push` to origin/main (per CLAUDE.md "After an epic retrospective" rule)
4. Tag `v2.0`

# Epic B — Docker Hub Publishing + DocBook User Manual Update — Retrospective

**Date:** 2026-05-20
**Facilitator:** Amelia (Senior Software Engineer)
**Project Lead:** Guy Corbaz
**Epic status:** 1/1 stories `done`; retrospective `done`.

---

## Epic summary

Epic B was a **single-story scope-narrow epic** opened on 2026-05-19 specifically to close the final pre-v2.0-GA-tag gaps surfaced in the Epic A retrospective (commit `b2e435f`, 2026-05-19). The story (B-1) delivered:

- **Dual-registry Docker image publishing** — Docker Hub (`gcorbaz/opcgw`) + GHCR (`guycorbaz/opcgw`), multi-arch `linux/amd64` + `linux/arm64`, triggered on `v*` tag push from a single GitHub Actions workflow.
- **Dockerfile hardening** — non-root user `opcgw` (UID 10001) enabled, base image pinned from `ubuntu:latest` → `ubuntu:24.04` (LTS Noble).
- **Repo README + CHANGELOG sync** — Docker section, Epic B row in Planning table, current-version block.
- **NEW canonical Markdown source** for the hub.docker.com Overview page (`docs/dockerhub-description.md`, ~190 lines).
- **DocBook user manual update** (+1921 / −106 lines, now 3975 lines) — closes Epic A retro action item AI-A-8. New `ch-upgrade` chapter; rewritten `ch-installation`, `ch-configuration`, `ch-troubleshooting`; logo embedded on title page; v2.0.0-GA revhistory entry.
- **NEW Makefile** at `docs/manual/Makefile` wrapping xsltproc + dblatex for headless HTML/PDF builds.
- **NEW logo pack** at `docs/logo/` (provided by Guy mid-story).
- **Strict-zero invariant honoured throughout** — zero changes to `src/`, `tests/`, `Cargo.toml`, `Cargo.lock`, `migrations/`, `config/`.

**Velocity:** 1/1 stories in 1 calendar day (2026-05-19 spec + impl + iter-1 review; 2026-05-20 iter-2 review + flip-to-done). Cleanest single-story velocity in the project.

**Test gate:** `cargo test --all-targets` 1256/0/10, `cargo clippy --all-targets -- -D warnings` clean, `cargo test --doc` 0/55, DocBook 4.5 DTD valid, workflow + compose YAML parse, Makefile `make -n` dry-run clean.

---

## Per-story summary

### B-1 — Docker Hub Publishing + DocBook User Manual Update

- **Spec creation:** 2026-05-19 (commit `25cb062`). 23 ACs across 6 groupings; 10 tasks; two D-list decisions (D1 Docker Hub long-description sync approach, D2 manual build pipeline).
- **Implementation:** 2026-05-19 (commit `e7931fe`). D1 = (a) auto-sync via `peter-evans/dockerhub-description@v4`; D2 = (a) Makefile under `docs/manual/`. **Local Docker smoke test PASSED on the fifth boot iteration** — each prior boot exposed a real Dockerfile/compose/docs gap (permissions → config bind-mount → PKI fail-closed → SQLite `data/` directory → success).
- **Code review iter-1 same-LLM (Opus 4.7):** 2026-05-19 (commit `1cf3ba7`). 45 raw findings (3 parallel reviewer subagents: Blind Hunter, Edge Case Hunter, Acceptance Auditor) → 26 PATCH applied + 6 DEFER + 7 DISMISS. Dominant failure class was phrase-harmonization-drift (7 HIGH + 11 MED) where operator-doc grep recipes targeted structured-log strings the source never emits — same shape as A-7 L1+L2 / A-5 K5 / A-6 K1.
- **Code review iter-2 same-LLM-fresh-context (Opus 4.7):** 2026-05-20 (commit `fb14d69`). 28 findings → 2 decisions resolved (U1 build-push split, U2 escalate T8 to HARD pre-tag gate) + 15 patches (U3–U17) + 4 defers (U18–U21) + 2 dismissals (X1 max_message_size 327675 verified intentional, X2 poll_cycle recipe correct on re-read). **Doctrine validated 14×; B-1 is the 2nd doc-dominant confirming case after A-7.**

---

## Cross-story synthesis

### What went well

1. **Local Docker smoke test was load-bearing.** Caught 5 progressive startup issues during implementation that would have been first-deployment blockers (UID 10001 permissions, missing config bind-mount, PKI fail-closed without keypair, missing `data/` directory). Without smoke testing, B-1 would have shipped a Docker image that crash-looped on first run.

2. **Both D-list decisions locked in version-controlled outputs.** D1 auto-sync keeps the hub.docker.com Overview page in git; D2 Makefile enables headless manual builds. Reversibility is high; future Overview edits don't drift; manual is no longer oXygen-coupled.

3. **Doctrine validation streak extends to 14 cases, 2 doc-dominant (A-7, B-1).** The iter-N+1 pattern is now empirically surface-agnostic: it holds equally for code-heavy storage refactors (A-1/2/3), integration-layer rewrites (A-4/5/6 web/JS), operator-facing docs + scripts (A-7), and YAML/Markdown/DocBook infrastructure (B-1).

4. **Phrase-harmonization-drift detection is now mechanical.** Iter-2 surfaced 3 examples in iter-1's own patches (U4 sister sentence `[opcua].endpoint` at +7 lines from T3; U5 chown `./data` 23 lines from T9; U7 chmod 2/4 occurrences). All shape-identical to prior cases. The detection method (`grep` for the OLD phrase after every patch) is now boilerplate.

5. **AI-A-8 (Manual XML sync) pulled forward from v2.x to v2.0 GA.** Epic A retro projected this as v2.x post-GA; Epic B materially extended GA scope to include it. Justified: operators deploying v2.0 from Docker Hub need a current install/configure/troubleshoot manual; out-of-date docs are a worse first-impression blocker than skill codification (AI-A-1/2/3).

### What didn't go well

1. **Iter-1 was a heavy patch round with 6 HIGH regressions surviving into iter-2.** Root cause: iter-1's same-LLM subagents that authored manual content also reviewed it, anchoring on what they thought they wrote rather than what they actually wrote. The doctrine predicted this; the doctrine caught this. But it's worth noting that **self-review bias** (author = reviewer) is a distinct mechanism from **same-LLM context contamination** (different chat windows but same model weights).

2. **`actions/checkout@v6` was in the workflow since the original implementation commit and survived 3 iter-1 reviewer layers.** The inline-comment-misdirection finding-class is uncomfortable: documenting an *intentional* `@v5` pin on `docker/build-push-action` (good engineering practice) drew attention away from the unrelated `@v6` typo on `actions/checkout` 45 lines above. Reviewers need an explicit per-`uses:` audit step that ignores nearby inline comments.

3. **iter-1 deferred T8 (Cargo bump) as a "minor pre-tag cleanup."** Iter-2 escalated to a HARD pre-tag GATE because `docker/metadata-action@v5` suppresses `:2.0` for prerelease versions, making every `:2.0` doc reference dead with `manifest unknown`. The iter-1 classification was understandable given iter-1's knowledge — but the *consequence* was much larger than the deferral implied. **General lesson:** when an iter defers an item, it should fact-check the impact on the *release pipeline*, not just the immediate story scope.

4. **Secret-name mismatch surfaced only via Guy's direct question.** The repo has `DOCKER_USERNAME` (added 2024-11-05 for prior workflows) but the new workflow expects `DOCKERHUB_USERNAME`. Both iter-1 and iter-2 reviewers verified the workflow against the spec/AC — neither checked the workflow's secret references against the *actual existing secrets in the repo*. The U1 GHCR-only-fallback patch means nothing breaks (workflow gracefully degrades to GHCR-only publish), but Docker Hub publishing would silently never fire until the secret is renamed.

### Epic A retro follow-through

| AI item | Description | Status post-Epic B | Evidence |
|---|---|---|---|
| AI-A-1 | Codify iter-N+1 rule in `bmad-code-review` skill | ❌ Not addressed | Skill source untouched; doctrine still enforced via memory + CLAUDE.md |
| AI-A-2 | Codify "fake regression-guard test" check | ❌ Not addressed | Same |
| AI-A-3 | Codify "closed-enum doc-sync" check | ❌ Not addressed | Same |
| **AI-A-4** | **CHANGELOG entry for MetricValue.value retire** | ✅ Done | Commit `ed2396e` (2026-05-19) |
| AI-A-5 | Issue #102 (tests/common extraction) — v2.x | ⏳ Carry-forward | Post-GA |
| AI-A-6 | Issue #100 (doctest baseline) — v2.x | ⏳ Carry-forward | Post-GA |
| AI-A-7 | A-2 IH1 (migration runner atomicity) | ⏳ Carry-forward | No migration story in Epic B |
| **AI-A-8** | **Manual XML user manual sync — v2.x MED** | ✅ Done (pulled forward) | B-1 delivered +1921/−106 lines on `opcgw-user-manual.xml` |
| AI-A-9 | v008 SLA optimization — LOW | ⏳ Carry-forward | LOW deferred |
| AI-A-10 | Update `feedback_iter3_validation` memory | ✅ Done | Memory now 14× with new "inline-comment misdirection" finding-class |
| AI-A-11 | "Fake regression-guard test" memory | ✅ Done | `feedback_fake_regression_guard_tests.md` exists |

**Score: 4/11 done, 4/11 carry-forward to v2.x (intentional), 3/11 not addressed (skill codification — AI-A-1/2/3 escalate to v2.0 GA-blocker status?).**

### Doctrine validation (iter-2 over-reviewing)

B-1 = **14th consecutive story** validating the iter-N+1 doctrine. B-1 also became the **2nd doc-dominant confirming case** (after A-7), proving the doctrine extends across:

- Code-heavy storage refactors (A-1/A-2/A-3)
- Integration-layer rewrites (A-4/A-5/A-6 web/JS)
- Operator-facing docs + small script + small test (A-7)
- **YAML/Markdown/DocBook infrastructure (B-1)** — the broadest doc-dominant surface yet, including GitHub Actions adversarial review

Iter-2 of B-1 caught **6 HIGH + 6 MED + 5 LOW** that iter-1 missed:

| # | iter-1 anchor | iter-2 catch | Severity | Class |
|---|---|---|---|---|
| U1 | T10 gated only the Docker Hub LOGIN step | Build-push step unconditional → GHCR-only-fallback was a lie | HIGH | Phrase-harmonization-drift extended to YAML conditionals |
| U2 | T8 deferred as "minor pre-tag cleanup" | Cargo `2.0.0-rc` × docker/metadata-action prerelease-suppression → `:2.0` dead | HIGH | Deferral-impact misclassification |
| U3 | (never patched) | `actions/checkout@v6` does not exist; pre-existing iter-1 miss | HIGH | **NEW finding-class: inline-comment misdirection** |
| U4 | T3 patched `OPCGW_OPCUA__ENDPOINT` env-var | Sister sentence `[opcua].endpoint` at +7 lines | HIGH | Phrase-harmonization-drift (classic) |
| U5 | T9 patched `docker run -v ./data:...` example | Chown 23 lines before missed `./data` | HIGH | Phrase-harmonization-drift (same-procedure asymmetry) |
| U6 | (never patched) | Manual conflates Rust constant `OPCGW_CONFIG_PATH` with env var | HIGH | Source-of-truth conflation |
| U7 | T17 patched 2 of 4 `chmod 600 pki/private/*.pem` occurrences | NFR9 warning + Troubleshooting remediation still had broken glob | MED | Phrase-harmonization-drift (cross-chapter) |
| U8 | T37 added `EXPOSE 8080` | Compose ports gap | MED | Cross-file consistency |
| U9 | (never patched) | `opcgw.log` outlier in upgrade recipe vs actual `opc_ua_gw.log` | MED | Single-source-of-truth drift |
| U10 | (never patched) | "Choosing the Image" note describes pre-B-1 compose state | MED | Stale-narrative-vs-current-code |
| U11 | (never patched) | CHANGELOG `Variant::Double(23.5)` vs code emits `Variant::Float` | MED | Code-vs-doc claim |
| U12 | (never patched) | `opcua_session_count` gauge requires `diagnostics_enabled=true` | MED | Behavioural prerequisite |

**New finding-class surfaced and added to memory:** "inline-comment misdirection" — comments documenting one intentional decision can mask unrelated drift nearby. Detection rule: for each `uses: actions/X@vN` in a workflow file, verify @vN is the current stable major **regardless of nearby inline comments justifying other version pins**.

### Security review (per CLAUDE.md "Epic Completion Requirements")

**Verdict: CLEAN.** Zero HIGH/MED findings in B-1-introduced changes.

| # | Concern | Severity | Status |
|---|---|---|---|
| S1 | README `api_token = "your-api-token"` + `user_password = "secure-password"` | LOW | Placeholder text, explicitly meant to be replaced — clear |
| S2 | Manual references default OPC UA username `"opcua-user"` | LOW | Matches `src/config.rs` actual default — not a leak |
| S3 | Manual "world-readable private key" wording | — | Part of NFR9 fail-closed warning — good security posture |
| S4 | All URLs in published docs | — | Only public (hub.docker.com, github.com, ghcr.io) — no internal infra leak |
| S5 | All GH Actions pinned to major versions | LOW | Industry-standard; SHA pinning is a defense-in-depth v2.x follow-up |
| S6 | `docs/quickstart.md:75` shows `create_sample_keypair = true` without per-line prod warning | LOW | Pre-existing, NOT in B-1 scope; `docs/security.md:508` documents the warning |
| S7 | Workflow `permissions:` minimal (`contents: read, packages: write`) | — | Correct least-privilege |
| S8 | `gcorbaz` username hardcoded in 8+ files | — | Project-coupling note, not security; v2.x DX item |

---

## Action items

### Process improvements

**AI-B-1 — Codify "inline-comment misdirection" check in `bmad-code-review` Blind Hunter prompt.**

- Owner: Project Lead (Guy)
- Description: Add explicit instruction to Blind Hunter: "for each `uses: actions/X@vN` line in a workflow file, verify `@vN` is the current stable major regardless of nearby inline comments justifying other version pins."
- Success criteria: next workflow review catches typos that survived B-1 iter-1.
- Joins AI-A-1/2/3 as a skill-codification carry-forward.

**AI-B-2 — Document "deferral-impact misclassification" pattern in `bmad-code-review` triage step.**

- Owner: Project Lead (Guy)
- Description: When an iter classifies a finding as DEFER, the triage must include a *release-pipeline impact check* (does this break docs/registry/release-tooling claims? Does this gate a downstream operation?). U2/T8 escalation from "minor pre-tag cleanup" to HARD pre-tag GATE is the canonical example.
- Success criteria: future deferrals carry an explicit "release-pipeline impact" line.

**AI-B-3 — Verify workflow secret references against actual repo secrets during review.**

- Owner: Project Lead (Guy)
- Description: `gh secret list --repo X/Y` is a one-liner that surfaces name mismatches at review time. Add to iter-N reviewer instructions for any workflow change.
- Success criteria: secret-name mismatches surface during review, not via operator question post-merge.

### Technical debt — GA-blocking

**AI-B-4 — T8 / U2 HARD pre-tag gate: bump `Cargo.toml` `2.0.0-rc` → `2.0.0`.**

- Owner: Amelia / Project Lead
- Description: Single small commit touching `Cargo.toml`, `Cargo.lock` (regenerated), and `README.md` badge (`version-2.0.0--rc-blue` → `version-2.0.0-blue`). Must land BEFORE `v2.0.0` tag is pushed, or every `:2.0` doc reference returns `manifest unknown`.
- Success criteria: `Cargo.toml::version == "2.0.0"`; `cargo build --release` produces a `2.0.0` artifact; first `v2.0.0` tag push emits `:2.0` floating tag from `docker/metadata-action@v5`.
- Status this session: **In progress** — see Step 11 below.

**AI-B-5 — Rename repo secret `DOCKER_USERNAME` → `DOCKERHUB_USERNAME`.**

- Owner: Project Lead (Guy)
- Description: The workflow references `secrets.DOCKERHUB_USERNAME` but the repo has `DOCKER_USERNAME` from a prior workflow (2024-11-05). The U1 GHCR-only-fallback means nothing breaks, but Docker Hub publishing will silently never fire. Run `gh secret delete DOCKER_USERNAME --repo guycorbaz/opcgw && gh secret set DOCKERHUB_USERNAME --body "gcorbaz" --repo guycorbaz/opcgw`.
- Success criteria: `gh secret list` shows `DOCKERHUB_USERNAME` instead of `DOCKER_USERNAME`.

**AI-B-6 — Verify `DOCKERHUB_TOKEN` is not expired before `v2.0.0` tag push.**

- Owner: Project Lead (Guy)
- Description: Token was added 2026-04-13 (~5 weeks ago). If it has a finite expiration, regenerate at <https://hub.docker.com/settings/security> with Read/Write/Delete scope (Delete needed for `peter-evans/dockerhub-description@v4` Overview-sync step).
- Success criteria: a `workflow_dispatch:` dry-run or throwaway `v0.0.0-test` tag successfully publishes to both registries.

**AI-B-7 — End-to-end real-world test (ChirpStack + OPC UA client) per Guy's batched-validation pattern.**

- Owner: Project Lead (Guy)
- Description: Per memory `session_epic_A_retro_2026_05_19`, Guy batches real-world validation until the version is "finished." After AI-B-4/5/6 land and after `git push`, run the gateway against a real ChirpStack instance with a real OPC UA client (e.g., UAExpert) and validate end-to-end metric flow.
- Success criteria: at least one full poll cycle's worth of metrics visible in the OPC UA client browser tree.

### Technical debt — v2.x post-GA

- **AI-B-8 — Skill codification (AI-A-1/2/3 + AI-B-1/2/3).** Six iter-N+1 process patterns are now empirically validated (14× total). They live in CLAUDE.md + memory but not yet in the `bmad-code-review` skill source itself. v2.x story to codify.
- **AI-B-9 — SHA-pin GH Actions** (S5 follow-up). Defense-in-depth against compromised major-version tags. v2.x.
- **AI-B-10 — Hardcoded `gcorbaz/opcgw` repo-coupling** (S8 follow-up). 8+ files reference the username; if the project moves to an org or different account, this is a multi-file sweep. Consider introducing a `{{ vars.IMAGE_NAMESPACE }}` indirection in the workflow + a single source-of-truth doc. v2.x DX.

### Memory updates

- **AI-B-11 — `feedback_iter3_validation` memory updated** (this session). 14× with new "inline-comment misdirection" finding-class.
- **AI-B-12 — Save retrospective doc** (this session, see Step 11 below).

---

## v2.0 GA release readiness (final readiness gate)

Epic A retro projected: "There is no Epic B defined. Epic A is the v2.0 GA gating epic."

Reality after Epic B: **Epic B materially extended GA scope** to include dual-registry Docker publishing, Dockerfile hardening, dockerhub-description Overview page, manual XML rewrite (AI-A-8 pulled forward), and Makefile build pipeline.

### Critical path before `v2.0.0` tag (final)

| # | Gate | Status |
|---|---|---|
| 1 | Epic A complete (7/7 + retro `b2e435f`) | ✅ Done & pushed 2026-05-19 |
| 2 | AI-A-4 CHANGELOG entry (`ed2396e`) | ✅ Done & pushed 2026-05-19 |
| 3 | Epic B B-1 spec + impl + iter-1 + iter-2 | ✅ Done; commits `25cb062`, `e7931fe`, `1cf3ba7`, `fb14d69` |
| 4 | Epic B retrospective (this doc) | ✅ Done — this commit |
| 5 | Epic B inline security review | ✅ CLEAN, embedded in this doc |
| 6 | AI-B-4 — Cargo bump `2.0.0-rc` → `2.0.0` (HARD pre-tag gate) | ⏳ Separate commit in this session |
| 7 | AI-B-5 — Rename `DOCKER_USERNAME` → `DOCKERHUB_USERNAME` | ⏳ Guy out-of-band (gh CLI one-liner) |
| 8 | AI-B-6 — Verify `DOCKERHUB_TOKEN` not expired | ⏳ Guy out-of-band |
| 9 | AI-B-7 — End-to-end real-world test | ⏳ Guy batched validation |
| 10 | `git push origin/main` | ⏳ This session after #4 + #6 |
| 11 | Tag `v2.0.0` | ⏳ Final step — auto-triggers workflow → both registries publish |

### Deliberately out of v2.0 GA scope (carry-forward to v2.x)

- AI-A-5 (#102 tests/common extraction)
- AI-A-6 (#100 doctest baseline)
- AI-A-7 (migration runner atomicity, before next migration story)
- AI-A-9 (v008 SLA tuning)
- AI-B-8 (skill codification — AI-A-1/2/3 + AI-B-1/2/3)
- AI-B-9 (SHA pinning of GH Actions)
- AI-B-10 (hardcoded `gcorbaz/opcgw` repo-coupling)
- Pre-existing project items: Issue #88 per-IP rate limiting, #104 TLS hardening, #110 RunHandles Drop, #113–116 hot-reload follow-ups, Story 8-4 threshold alarms

### Significant discoveries (do they require scope change?)

| Discovery | Affects GA? | Recommendation |
|---|---|---|
| `actions/checkout@v6` typo survived iter-1 reviewers | No — patched in iter-2 (U3) | New finding-class memo'd (AI-B-1) |
| Workflow build-push step unconditional → GHCR-only-fallback lie | No — patched in iter-2 (U1, split into 2 steps) | — |
| `DOCKER_USERNAME` vs `DOCKERHUB_USERNAME` secret-name mismatch | No — workflow gracefully degrades to GHCR-only; rename trivially | AI-B-5 |
| Cargo prerelease × docker/metadata-action `:2.0` suppression | Yes — narrative | AI-B-4 HARD pre-tag gate |
| AI-A-8 (Manual XML sync) pulled into pre-GA scope | Yes — increased GA scope by ~2000 doc lines + new Makefile + dockerhub-description.md + logo pack | Already absorbed in Epic B; no further action |
| `DOCKERHUB_TOKEN` age (~5 weeks) | Possibly — if PAT expiration is finite | AI-B-6 verification before tag |

**Verdict: NO further scope change required for v2.0 GA.** The remaining items (#6–#11 above) are mechanical or operator-actions.

---

## Readiness assessment

| Dimension | Status |
|---|---|
| Testing & quality | ✅ `cargo test --all-targets` 1256/0/10; `clippy --all-targets -- -D warnings` clean; `cargo test --doc` 0/55; DocBook DTD valid; YAML + Makefile parse clean |
| Deployment | ⏳ 1 commit unpushed (`fb14d69`); retro commit + Cargo bump commit pending in this session; tag pending |
| Stakeholder acceptance | ✅ Project Lead (Guy) participated in iter-1 + iter-2 review + this retro |
| Technical health | ✅ Strict-zero on src/; Dockerfile hardened (UID 10001, ubuntu:24.04 LTS); workflow split into registry-conditional legs; manual current to v2.0 reality |
| Unresolved blockers | ⏳ AI-B-4 (Cargo bump) is the only GA-blocking *commit*; AI-B-5/6/7 are out-of-band operator actions |
| Security | ✅ CLEAN per inline review above (zero HIGH/MED in B-1-introduced changes) |

**Epic B is functionally complete and v2.0 GA is two commits + three operator-actions + one tag away.**

---

## Closure

Epic B: Docker Hub Publishing + DocBook User Manual Update — **REVIEWED AND CLOSED**.

The doctrine validation streak now stands at 14× across 4 surface types (storage refactor / integration rewrite / docs+script / YAML+Markdown+DocBook). The iter-N+1 pattern is empirically surface-agnostic and should default for all future stories.

The single-story epic pattern (open a narrow epic to close gaps surfaced by a prior retro) proved efficient — 1 day from epic open to retro close, 4 commits total. Recommended for future "GA gap-closure" or "tactical fix" scopes.

**Next mandatory actions (this session):**
1. Commit this retrospective + sprint-status flip.
2. AI-B-4 Cargo bump commit (HARD pre-tag gate).
3. `git push origin/main`.

**Next out-of-band actions (Guy):**
- AI-B-5 secret rename.
- AI-B-6 PAT verification.
- AI-B-7 end-to-end real-world test.
- Tag `v2.0.0` when AI-B-5/6/7 green.

---

*Retrospective facilitated by Amelia (Developer) on 2026-05-20. Project Lead: Guy Corbaz. Saved to `_bmad-output/implementation-artifacts/epic-B-retro-2026-05-20.md`.*

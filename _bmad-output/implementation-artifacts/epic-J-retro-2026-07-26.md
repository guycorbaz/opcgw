# Epic J Retrospective — Config Authority & Command Responsiveness (v2.8.0)

**Date:** 2026-07-26
**Facilitator:** Bob (Scrum Master) · **Project Lead:** Guy
**Status:** Epic J COMPLETE — 3/3 stories done
**Mode note:** J-1's review and all of J-2 ran in an **autonomous session** under the owner's standing delegation of 2026-07-25 ("continue autonomously with bmad until the end of the epic; when in doubt, party-session and decide; report at the end so we finalize or correct together"). Every autonomous decision is listed for ratification in `epic-J-autonomous-session-report-2026-07-25.md`.

---

## 1. Epic summary

Epic J took the three change-requests the v2.7.1 stable release left on the table — deliberately chosen over the other open CRs (#163 blocked upstream on async-opcua, #137 waiting for a second real device, #147/Epic I a separate UI track).

| Story | Issue | Outcome |
|-------|-------|---------|
| J-0 Metric type-mismatch & orphan warnings in the web feed | #160 | done 2026-07-23 — `map_uplink_to_writes` returns `UplinkMapping`; warn-once-per-(device,metric) dedup; new `metric_type_mismatch` / `metric_never_seen` categories render in the existing feed with zero frontend change |
| J-1 Decouple command dispatch from the metrics poll | #136 | done 2026-07-25 — event-driven `CommandDispatcher` (`src/chirpstack_dispatch.rs`) woken by a `Notify` the OPC UA write fires; poll-head drain removed; full delivery-lifecycle contract added in review |
| J-2 Enforce the env-var allowlist | #169 / #168 | done 2026-07-26 — allowlist-filtered figment provider; web/SQLite genuinely authoritative; **BREAKING**, with a migration note |

**Delivery quality:** every story ran the 3-layer adversarial review (Blind Hunter / Edge Case Hunter / Acceptance Auditor) on a **different model than the implementer**, plus mandatory re-reviews of each fix round. J-1 needed **four** iterations after the initial pass (iters 3–6), J-2 **two**. Every loop terminated LOW-only. Final gates: `cargo test` **1917 / 0 fail** (from an 1826 baseline at v2.7.1 — **+91 tests**), `cargo clippy --all-targets -- -D warnings` clean.

---

## 2. Mandatory epic security review

**VERDICT: PASS — after remediation.** Independent reviewer over the full `e95f188..HEAD` diff (35 files, +4318/−530). The first pass returned **FAIL on checklist item 5 (access control)**; the retrospective was held open, the findings were fixed, and PR #181 merged before this retro closed.

| # | Item | Verdict |
|---|------|---------|
| 1 | No hardcoded credentials/secrets | PASS — only doc placeholders and test fixtures |
| 2 | Input validation on external data | PASS — gRPC `f_port` range + mapping checks retained, malformed rows quarantined, OPC UA writes type/range-checked, dispatch gated on `is_good()` |
| 3 | Error messages don't leak sensitive info | PASS — all new events log **names only, never values**, verified line by line; `tonic::Status` cannot carry the API token (Display omits metadata/details) |
| 4 | No injection vulnerabilities | PASS — every new/changed rusqlite call fully parameterised; no `format!`-built SQL anywhere in the diff |
| 5 | Permission / access control | **FAIL → FIXED** (see below) |
| 6 | No unsafe code; SPDX headers | PASS — zero `unsafe`; the one new source file carries the header |

**HIGH-1 (fixed, PR #181): a fourth allowlist bypass, empirically demonstrated.** figment parses env *values* into structured dicts, so a **section-level** variable — `OPCGW_CHIRPSTACK='{polling_frequency=99,server_address="…"}'` — deep-merged over the SQLite provider and set **any number** of blocked fields at once, including `[opcua].user_name` (the web Basic-auth login) and `[web].allowed_origins`. The separator-less key hit the filter's fail-open `None` arm and the reporter skipped it, so it was also **silent**. This is the third distinct bypass shape the epic's own reviews found (after `__`-only parsing and whitespace) and the most powerful. Now blocked and reported; mutation-verified with the reviewer's exploit values.

**MEDIUM-1 (fixed):** `maybe_warn_env_shadows_singleton` — the *third* env scanner — had never adopted the shared normalizer, keeping `std::env::vars()` (**panics the boot** on any non-UTF-8 environment variable) and a case-sensitive, `__`-only, untrimmed parse. All three scanners plus the provider filter now classify identically.

**MEDIUM-2 (fixed, docs):** `docs/security.md` gained a v2.8.0 upgrade checklist — a deployment hardened through `.env` (`trust_client_cert`, `check_cert_time`, `allowed_origins`) silently reverts to the stored, laxer value.

**LOW-1 (fixed):** the remote-supplied gRPC text J-1 persists into `command_queue.error_message` (served to operators over OPC UA) now goes through `sanitize_error_message`, matching the J-0 feed path. **LOW-2 (fixed):** `NotConnected`/`TimedOut` dropped from the retry-safe io-error kinds — their post-send impossibility rested on hyper-util internals. **LOW-3 (accepted, informational):** `application_list` remains env-injectable by design (an env-setter is already an admin), now recorded in the threat model — noteworthy because J-1 makes any injected command dispatch within seconds.

**DoS surface:** no new unbounded surface — retry ladder capped and deadline-terminated, `Notify` coalesces write bursts, the outage short-circuit bounds work to one connect + one WARN per drive, and every new WARN is once-per-boot or once-per-row guarded.

---

## 3. What went well

- **The mandatory iter-N+1 rule was vindicated harder than in any previous epic.** Every single re-review round found real defects *in the previous round's fixes* — six rounds in a row across the two data-plane stories:
  - J-1 iter-4 caught that iter-3's new "retry transient failures" made *ambiguous* enqueue outcomes retryable — a **systematic duplicate-downlink** (double valve actuation) that was strictly worse than the terminal-`Failed` it replaced.
  - J-1 iter-5 caught that the iter-4 classifier was dead for the *most common* outage shape (warm cached client) and that a failed bookkeeping write re-opened the double-send window.
  - J-1 iter-6 caught that the iter-5 classifier still missed the **DNS-failure** shape — which is exactly what the flagship compose deployment produces when the ChirpStack container restarts.
  - J-2 iter-1 caught a **complete allowlist bypass** (a dot-separated variable defeated the whole story), iter-2 caught that my *fix* had two more bypass/panic classes in it, and then the **mandatory epic security review found a fourth, more powerful shape** (a section-level dict variable) that all three previous rounds had missed.
  Without the loop — and without the security gate on top of it — each of these ships.
- **Cross-model review kept working, and the model changed mid-epic.** J-1 was implemented on Opus 4.8 and reviewed on Fable 5; J-2 was implemented on Fable 5 and reviewed on Opus 5. Independence of the reviewing model mattered more than which model it was.
- **Reviewers verifying claims against dependency sources.** The J-1 iter-5/6 reviewer read the actual tonic/hyper-util code to confirm whether the connect is eager (making the retry path live) and where the io-error kind survives in the `Status` chain; the J-2 reviewers read figment's `env.rs` and built a standalone harness to *prove* the bypasses. Assertions about third-party behaviour are now expected to come with a source citation or an experiment.
- **Story validation earned its keep before a line was written.** J-2's fresh-context validation caught that the drafted filter order (`.filter` after `.split`) would have been a **silent no-op**, plus a breaking test and two unclassified variables — all before dev started.
- **The real-binary smoke found what tests could not, twice.** J-1's smoke surfaced a pre-existing poller shutdown defect (#178). J-2's smoke caught both a boot ordering flaw (the ignored-var report ran *after* the load, so a boot that failed *because* of an ignored var never explained itself) and a duplicate WARN. Neither is visible to `cargo test`.

## 4. What was hard / lessons

- **A "fail-open" default in a security-ish filter is a bug generator.** All *four* J-2 bypasses came from the same shape: an input that didn't match the expected pattern fell through to `return true`. The fix that finally held was not another special case but **one shared normalizer that mirrors the downstream library exactly**, used by both the filter and the reporter — making classification drift impossible by construction. **Lesson:** when filtering input for a downstream parser, normalize with *that parser's* rules, in one place, and prove filter/reporter agreement with a table-driven test over adversarial shapes. Corollary learned the hard way: enumerate what the downstream parser accepts *beyond* the obvious form — figment accepted four (`__`, `.`, whitespace-padded, and a section-level **dict value**), and each unenumerated one was a silent total bypass.
- **"Transient vs terminal" is not a binary — the third state is *ambiguous*.** J-1's delivery contract only became correct once it distinguished *provably-undelivered* (retry safely) from *may-have-been-delivered* (never blind-retry; tell the operator "delivery uncertain"). For anything that actuates hardware, ambiguity must fail toward "ask a human", not "try again". **Lesson:** classify I/O failures by *what the peer may have observed*, not by whether they look temporary.
- **A safety mechanism must be evaluated against the deployment it protects.** The J-1 classifier was correct in the lab and useless on the NAS until iter-6, because compose addresses ChirpStack by DNS name and a stopped container fails at *resolution*, not connection-refusal. **Lesson:** when writing an error classifier, enumerate the failure shapes of the *actual* production topology.
- **A helpful diagnostic can create the incident it warns about.** J-2's anti-lockout hint printed the login name from the *bootstrap* snapshot, i.e. the stale value — precisely wrong in the deployments it was written for. **Lesson:** a log line that reports "the effective value" must be emitted after the value is actually effective.
- **"Shared helper" is only true once every call site is converted.** J-2's module doc claimed the single normalizer "makes drift impossible" while a *third* scanner still had its own ad-hoc parsing — including the boot-panic the review had just removed from its sibling. **Lesson:** when consolidating duplicated logic, grep for every instance of the pattern and convert them in the same commit, or the doc comment becomes a false guarantee.
- **`git add -A` is not safe in this repo.** It swept a 22 GB-adjacent set of local artifacts (`data/opcgw.db`, rotated logs, BMad-local config, unrelated untracked assets) into a review commit — twice. Fixed by hardening `.gitignore` and staging explicitly. **Lesson:** stage by path, always.
- **Autonomous mode needs an explicit decision ledger.** Party-mode resolved four interlocking delivery-semantics decisions (J-1) and two allowlist-membership decisions (J-2) without the owner. That is only safe because every one is written down for ratification. **Lesson (new doctrine):** in autonomous runs, any decision the owner would plausibly make differently gets a numbered ratification item, not just a code comment.

## 5. Previous-retro follow-through

- **AI-G-5 (real-binary smoke as the release gate)** — **honoured, and it paid off twice** (see §3). Local smoke ran for both data-plane stories; the NAS half is now bound to the v2.8.0-rc1 tag per the owner's Docker-Hub-only rule.
- **#73 async-pool / `spawn_blocking`** — untouched this epic; the new dispatcher uses the established `async_store()` facade, so no new blocking-from-async sites were added.
- **#172 config.rs split** — **regressed**: `config.rs` grew 5741 → ~6100 lines. J-2's AC#11 permitted a test-module extraction; it was deliberately not started mid-review. Now overdue (see AI-J-4).

---

## 6. Action items

| ID | Action | Owner | Priority |
|----|--------|-------|----------|
| AI-J-1 | **Cut `v2.8.0-rc1` and soak on panoramix** — the release gate for BOTH data-plane stories (J-1 command dispatch, J-2 allowlist). Owner repoints the compose tag; NAS runs Docker Hub images only. | Guy (soak) | **HIGH (release gate)** |
| AI-J-2 | **Ratify the autonomous decisions** R1–R6 in `epic-J-autonomous-session-report-2026-07-25.md` (delivery-deadline value, ambiguous-failure policy, the two blocked env vars, deferrals). | Guy | **HIGH** |
| AI-J-3 | **#177** — at-least-once duplicate-send windows (crash/abort in the enqueue→mark-Sent gap; mark-Sent storage failure). Needs `chirpstack_result_id` idempotency; also the prerequisite for any future per-task supervision. | Guy's call | MEDIUM |
| AI-J-4 | **#172 + config.rs split** — extract `mod tests` to `src/config_tests.rs` (the `storage/sqlite_tests.rs` precedent). Two files now exceed the project's 5000-line rule (`config.rs` ~6100, `web/api.rs` ~5900). | Guy's call | MEDIUM |
| AI-J-5 | **#178** — ChirpStack availability wait-loop ignores shutdown cancellation, so teardown during an outage always burns the 10 s force-abort and logs a spurious ERROR. Found by the J-1 smoke. | Guy's call | LOW |
| AI-J-6 | Deferred UX: an "env override active" badge in the Admin page for the four allowlisted bootstrap keys (an Admin save + Apply currently reports success while the env value keeps winning until the next boot). | Guy's call | LOW |
| AI-J-7 | **LOW-3 (accepted):** record in the threat model that `application_list` is env-injectable and that J-1 now dispatches any injected command within seconds. Revisit if the env boundary ever stops implying admin. | Guy's call | LOW |
| AI-J-8 | Adopt the **shared-normalizer / no-fail-open** pattern as project doctrine for any future input filter, and the **ambiguous-failure** taxonomy for any future I/O retry (both codified in this retro §4). | — | Doctrine |

---

## 7. Milestone / release status

- All Epic J commits on `origin/main` through this retro push. PRs #175/#176 (J-0 + a fix), #179 (J-1), #180 (J-2) merged.
- Issues closed by the epic: **#160** (J-0), **#136** (J-1), **#169** + **#168** (J-2). Filed along the way: **#177**, **#178**.
- **v2.8.0 is NOT yet cut.** Next action is AI-J-1 (`v2.8.0-rc1` → Docker Hub → owner soak). v2.8.0 stable is gated on that soak, exactly as v2.7.1 was.
- ⚠️ **The rc upgrade notes must carry the J-2 migration steps** — this is the first **breaking** release of the config surface.

## 8. Next direction

Epic J closes the config-authority/responsiveness line. Candidates for what follows (Guy's call): (a) a **technical-debt epic** — AI-J-3 (#177 idempotency), AI-J-4 (#172 file split), #73's async-pool remediation, and the substring-matcher codification carried since Epic C/D; (b) **#137** device-class registry, if a second real device model has appeared; (c) **#147**/Epic I, the web-UI modernization track, which still has I-3 ready-for-dev and I-4 in backlog. No new epic should start before AI-J-1 lands, given two data-plane stories are awaiting their production soak.

---

*Retrospective complete. Security review PASS (after remediation, PR #181). Epic J closed 3/3.*

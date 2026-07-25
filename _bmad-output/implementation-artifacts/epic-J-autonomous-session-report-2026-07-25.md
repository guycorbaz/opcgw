# Epic J — Autonomous Session Report (2026-07-25)

Owner-facing record of every decision taken in the autonomous BMad run (owner pre-delegation 2026-07-25: "continue autonomously with bmad until the end of the epic; when in doubt, party-session and decide; report at the end so we finalize or correct together"). **FINAL — Epic J closed 3/3, v2.8.0-rc1 tagged and publishing.**

## Session-start decisions (owner-answered, not autonomous)

1. **J-1 soak gate:** local AI-G-5 real-binary smoke suffices to flip J-1 done; the panoramix NAS soak is deferred to the **v2.8.0-rc1 release gate**.
2. **J-2 prerequisite:** the panoramix `.env` reconciliation has NOT been done; the allowlist is designed from the **current** `.env` (read live over SMB) with the documented mismatch preferences (SQLite wins).
3. **Merge policy:** PR per story, self-merge after a clean review loop.
4. **Mid-session owner rule (new, saved to memory):** the NAS only runs Docker Hub images ⇒ anything needing a NAS soak ships as an rc tag first (→ v2.8.0-rc1 task).

## Story J-1 — decoupled command dispatch (DONE, merged via PR #179, closes #136)

### Crash recovery (pre-epic work)
The previous session crashed after J-1 implementation but before its commit. Verified: git store clean (fsck), no conflict markers; the only real damage was a corrupted incremental-build cache (undefined-symbol link failures) — cleared (`target/debug/incremental`, freed 22.9 GiB). Implementation committed as `f27b19f`.

### Review loop (iters 3–6 this session; iters 1–2 were the prior session's)
Fresh 3-layer adversarial review (Blind/Edge/Auditor subagents on Fable vs the Opus-4.8 implementer) then three mandatory re-review rounds per the iter-3 doctrine. **Every round found real defects in the previous round's fixes** — the doctrine is strongly re-validated.

#### 🎉 Party session #1 (delivery-semantics cluster, 4 MEDIUM decisions)
Panel: Winston (Architect), Amelia (Dev), Quinn (QA), Murat (TEA), John (PM). Consensus, no dissent:
- **D1 — bounded retry** for transient sink failures (was: any ChirpStack hiccup marked commands terminally `Failed`).
- **D2 — delivery deadline**: a `Pending` command older than `command_delivery_timeout_secs` (reused knob, no new config) is expired `Failed`, never delivered late — hardware-safety gate for the startup drain. AC#5 amended accordingly.
- **D3 — orphans terminal**: rows whose device/command was de-configured after queueing are `Failed`, never raw-byte-sent to a removed device.
- **D4 — poison-row quarantine**: a malformed queue row is marked `Failed` in storage instead of livelocking all dispatch.

#### Iteration highlights (all fixes mutation-verified where guard-shaped)
- **iter-4** caught that iter-3's D1 retried *ambiguous* RPC failures → systematic duplicate-downlink window; narrowed to **provably-undelivered-only retry** (ambiguous → terminal `Failed("delivery uncertain — verify the device queue")`); symmetric deadline (clock step-back can't immortalize rows); drain short-circuit under outage; `is_good()` gate; cached gRPC client + 10 s RPC deadline; `.expect()` → handled error on the sole delivery path.
- **iter-5** verified the tonic eager-connect crux LIVE in dependency sources, then caught the **warm-cache misclassification** (first command of a warm-cache outage was terminally Failed — fixed via typed `is_provably_unsent` source-chain classifier, no substring matching) and the **unmarked-terminal re-delivery** hole (Failed-write failure → row re-delivered → double actuation; fixed via a task-local carry map, delivery suppressed until the status write lands).
- **iter-6** caught the **DNS-class outage gap** — the flagship compose deployment addresses ChirpStack by container DNS name, and a stopped container fails at *resolution*, not refusal; fixed via exact-label match + `TimedOut` kind, guarded by **two real-stack e2e classifier tests** (live tonic client vs closed port and `.invalid` host) that fail loudly on dependency drift. Loop terminated (LOW-only residuals).

#### Autonomous decisions needing your ratification
- **R1 — "Retry only provably-undelivered" policy** (iter-4/5/6 refinement of party-D1): ambiguous RPC outcomes are terminal with an operator-facing "delivery uncertain" reason. Rationale: for hardware actuation, a re-issued command by a human beats an automatic possible-duplicate. Alternative you could choose: ChirpStack device-queue inspection before retry (deferred to #177).
- **R2 — Delivery deadline = `command_delivery_timeout_secs` (60 s default)** reused for the dispatch side. If you want a longer dispatch window (e.g. ride out multi-minute ChirpStack restarts), say so — it's one constant/knob decision.
- **R3 — `Refs #136` on the implementation commit, `Closes #136` on the PR** (deviation from AC#11's literal "commit message: Closes"): the issue closed at merge, which is when the code actually landed on main.
- **R4 — Deferred with issues filed:** #177 (pre-existing at-least-once duplicate windows — enqueue→mark-Sent crash gap + mark-after-enqueue storage failure; needs `chirpstack_result_id` idempotency), #178 (pre-existing poller shutdown wait-loop burns the 10 s teardown force-abort when ChirpStack is down — found by the smoke).

### Verification
- Gates at loop close: `cargo test` **1898/0** (up from 1872 baseline; +26 net new tests), clippy `-D warnings` clean.
- Mutation checks performed: D2 age gate, D1 retry scheduling, iter-4 ambiguous-terminal, iter-4 symmetric gate, iter-5 carry map — each sabotage made its guard test fail, then reverted. (One process slip: an over-broad `git checkout` during the iter-5 mutation restore wiped uncommitted edits in one file; re-applied from session record, gates re-run green. Recorded in the story file.)
- **AI-G-5 local real-binary smoke: PASS** — scratch-dir boot (no deadlock), OPC UA bind + real TCP connect, `Starting CommandDispatcher`, graceful `CommandDispatcher shutting down` ~1 ms after SIGINT, exit 0; NFR9 0600-key gate verified enforcing. **NAS soak pending v2.8.0-rc1** (see Release).

## Story J-2 — env allowlist enforcement (in progress at draft time)

### Design input captured live from panoramix (SMB)
- Compose still on `gcorbaz/opcgw:2.7.1-rc4` (reconciliation not done — matches your session-start answer).
- 10 active `OPCGW_*` vars; the 2 mismatches identified precisely: `OPCUA__STALE_THRESHOLD_SECONDS` (env 1500 vs SQLite **10800** — SQLite wins per your standing preference) and `GLOBAL__DEBUG` (env false vs SQLite **true** — SQLite wins; flip it on the Admin page if undesired).
- ⚠️ **`stream_all_devices` has NO SQLite row** (D-0 migration pre-dated the field) — env is its only source of `true` on the NAS. The J-2 migration note + rc upgrade notes must have you save it via the Admin page BEFORE upgrading, or streaming silently flips off.

### Autonomous design decision (needs ratification)
- **R5 — Allowlist boundary = "web-editable fields are blocked from env":** within the four Admin-page sections (`global`/`chirpstack`/`opcua`/`web`), only secrets (`api_token`, `user_password`) and the bootstrap set (`web.enabled/bind_address/port`, `opcua.host_port`) stay env-overridable; everything else is ignored + WARN (`env_var_ignored`). Sections the web cannot edit (`[storage]`, `[command_validation]`, `[logging]`) and the short-form/out-of-figment knobs (`OPCGW_LOG_DIR/LEVEL`, budgets, `OPCGW_ERROR_EVENT_CAP`, `CONFIG_PATH`) are untouched. Effect on your live `.env`: `GLOBAL__DEBUG`, `STALE_THRESHOLD_SECONDS`, `STREAM_ALL_DEVICES` become ignored (by design — they're the vars that were shadowing the Admin page).

### Delivered (PR #180, closes #169 + #168)

Allowlist-filtered figment provider; `env_var_ignored` WARN naming each ignored key; the shadow WARN narrowed to the keys that genuinely still win. Full doc sweep (manual chapter + appendix, configuration.md, .env.example, security.md, README, CHANGELOG migration note).

**The review found — and fixed — four separate ways to defeat the allowlist entirely.** This is the story's most important outcome, and worth your attention because each was *silent*:
1. `OPCGW_CHIRPSTACK__…` with a **dot** instead of `__` (figment nests on `.` regardless).
2. **Whitespace** after the prefix (figment trims twice; the untrimmed section missed the known-sections list and the check failed open).
3. A **section-level variable with a dict value** — `OPCGW_CHIRPSTACK='{polling_frequency=99,server_address="…"}'` — overriding *any number* of blocked fields at once, including the web login name. Found by the mandatory security review after three review rounds had missed it.
4. (Pre-empted before dev by the story validation: the drafted filter order would have been a no-op.)

All four are now closed by **one shared normalizer** that mirrors figment's own key handling and is used by the filter *and* all three reporters, so classification drift is impossible by construction. Every shape has a mutation-verified regression guard.

### Autonomous decisions needing ratification (J-2)

- **R6 — `OPCGW_CHIRPSTACK__SERVER_ADDRESS` and `OPCGW_OPCUA__USER_NAME` are BLOCKED** (party session #2). Both are advertised in `.env.example`, so this is a visible behaviour change; the rationale is that your own 2026-07-20 `.env` cleanup already removed them as "web/SQLite-managed", and both are Admin-page-editable. Mitigation for `user_name` (it is also your web login): the gateway now logs `env_var_ignored_login_name` naming the **effective** login name at boot, so a changed login is self-answering rather than a lockout.
- **R7 — the four bootstrap keys stay env-capable** (`web.enabled/bind_address/port`, `opcua.host_port`) because the gateway must serve `/setup` before SQLite exists and compose consumes them for port mapping/healthcheck. Consequence you should know: for these four, env still outranks the Admin page, and an Admin save + Apply will report success while the env value keeps winning until the next boot. Deferred as an Admin-page badge (AI-J-6).

## Epic J retrospective + security review

Both complete and on `main` (`epic-J-retro-2026-07-26.md`). **Security verdict: PASS 6/6 — after remediation.** The first pass failed the access-control item (bypass #3 above, plus a third env scanner that still had the boot-panic-on-non-UTF-8 and case-drift its siblings had been fixed for). I held the retrospective open, fixed everything, and merged PR #181 before closing the epic — per the CLAUDE.md rule that an epic cannot close without a clean security check.

Action items **AI-J-1..AI-J-8** are in the retro; AI-J-1 (this rc + your soak) is the release gate, AI-J-2 is your ratification of R1–R7.

## Release: v2.8.0-rc1 — TAGGED AND PUBLISHING

`v2.8.0-rc1` is tagged and pushed; the Docker Build workflow is publishing multi-arch images to Docker Hub (`gcorbaz/opcgw:2.8.0-rc1`) and GHCR. **Not** tagged `:latest` — it is a release candidate. The rc soaks BOTH data-plane stories on the NAS: J-1 (command reaches the ChirpStack queue within seconds of a write; no duplicate downlinks) and J-2 (ignored-var WARNs correct; Admin page authoritative).

⚠️ **Do the pre-upgrade steps BEFORE you pull the image** — this is the first breaking change to the config surface:
1. **Before** pulling the rc: open Admin → ChirpStack section → verify `stream_all_devices = true` (backfilled from the effective config) → Save section (persists the row).
2. Optionally clean the three now-ignored vars from `.env` (they'd only produce WARNs).
3. Decide `global.debug` (SQLite currently `true`).
4. **Check the security-flag checklist in `docs/security.md`** — if you ever hardened `trust_client_cert`, `check_cert_time` or `allowed_origins` through `.env`, that hardening is dropped unless the Admin page holds it.
5. Pull the rc, restart, then read the first boot's `env_var_ignored` lines — they are the definitive list of what your `.env` was silently overriding.
6. During the soak, watch for: `command_dispatch_*` (a valve command should reach the ChirpStack queue in seconds — the E-0 symptom that started J-1), any `command_dispatch_expired` (a command that missed its 60 s delivery deadline), and the absence of duplicate downlinks.

## Open items for the owner
- **Ratify R1–R7** (or ask for changes — everything is one commit away). The two I would most want your eyes on: **R2** (the 60 s delivery deadline — if your ChirpStack restarts take longer than that, commands issued during a restart will expire rather than deliver, and you may want a larger value) and **R6** (`SERVER_ADDRESS`/`USER_NAME` blocked).
- #151 (NAS `opcgw.db` mode 777 → 600) still open, owner-side manual fix.
- The retrospective's action items (appended after it runs).

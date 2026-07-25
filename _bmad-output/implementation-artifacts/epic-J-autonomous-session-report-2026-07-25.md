# Epic J — Autonomous Session Report (2026-07-25)

Owner-facing record of every decision taken in the autonomous BMad run (owner pre-delegation 2026-07-25: "continue autonomously with bmad until the end of the epic; when in doubt, party-session and decide; report at the end so we finalize or correct together"). **DRAFT — finalized at end of session.**

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

_(J-2 implementation/review/landing to be appended.)_

## Release: v2.8.0-rc1 (planned)

Cut after J-2 lands: bump + tag → Docker Hub/GHCR publish → you repoint panoramix compose. The rc soaks BOTH stories on the NAS: J-1 (command reaches the ChirpStack queue within seconds of a write; no duplicate downlinks) and J-2 (ignored-var WARNs correct; Admin page authoritative). Upgrade steps for you (will be in the rc notes):
1. **Before** pulling the rc: open Admin → ChirpStack section → verify `stream_all_devices = true` (backfilled from the effective config) → Save section (persists the row).
2. Optionally clean the three now-ignored vars from `.env` (they'd only produce WARNs).
3. Decide `global.debug` (SQLite currently `true`).
4. Pull rc, restart, check the error feed + `env_var_ignored`/`command_dispatch_*` events.

## Open items for the owner
- Ratify R1–R5 (or ask for changes — everything is one commit away).
- #151 (NAS `opcgw.db` mode 777 → 600) still open, owner-side manual fix.
- The retrospective's action items (appended after it runs).

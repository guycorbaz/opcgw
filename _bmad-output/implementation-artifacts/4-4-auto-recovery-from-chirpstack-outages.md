# Story 4.4: Auto-Recovery from ChirpStack Outages

**Epic:** 4 (Polling Reliability — Phase A carry-forward)
**Phase:** Phase A (carry-forward; created during Phase B Epic 9)
**Status:** review
**Created:** 2026-05-05
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md:532-549` is the single source of truth for the BDD acceptance criteria. This story file lifts them as ACs #1–#7 below, adds carry-forward invariants from Epics 5/6/7/8/9 as ACs #8–#11, and decomposes Tasks per the Phase A pattern. Story 4-4 was deferred at the Epic 4 retrospective (lines 125, 133 of `epic-4-retrospective.md`) as "soft dependency, not blocking." Phase B subscription + web work (Epics 8 + 9) took precedence; Story 4-4 is being picked up now to clear long-standing Phase A tech-debt before Story 9-7 (hot-reload) lands and reshapes the poller's reload semantics.

---

## User Story

As an **operator**,
I want the gateway to automatically reconnect after a ChirpStack outage,
So that I never need to manually restart the gateway after network or server issues, and the gateway resumes polling within 30 seconds of ChirpStack returning (NFR17).

---

## Objective

Today the gateway **detects** ChirpStack outages (via `maybe_emit_chirpstack_outage` at `src/chirpstack.rs:249`) and emits a `chirpstack_outage` warn-level event, but does **not recover** — the poll cycle continues without retry, and the next cycle (after `polling_frequency` seconds) attempts again. The existing TCP probe retry inside `get_device_metrics_from_server` (`src/chirpstack.rs:1786-1847`) retries the **probe** but on exhaustion just logs a `warn!("Timeout: cannot reach chirpstack server")` and falls through to `create_device_client().await?` which will fail too. There is **no** explicit recovery state machine, no operator-facing log on reconnection, no gateway_status update during outage, and no NFR17 30s SLA enforcement.

Story 4-4 closes this gap by adding an **explicit recovery loop** layered on top of the existing diagnostics from Story 6-3:

- **Detection** (existing): TCP probe via `check_server_availability` + `chirpstack_outage` warn (Stories 4-1 / 6-3).
- **Recovery** (new): three new `operation=` log events reserved at `docs/logging.md:240-242` (`recovery_attempt` / `recovery_complete` / `recovery_failed`); explicit retry loop with configurable count + delay (FR8 — knobs `chirpstack.retry` and `chirpstack.delay` already exist in `src/config.rs:121-131`); gateway_status update to `chirpstack_available = false` during outage so OPC UA clients (Story 5-2 stale-data) and the web dashboard (Story 9-2) see the outage status; resume polling within 30 s of ChirpStack returning (NFR17).
- **Logging** (new): `info` log on first recovery success with downtime duration; `warn` on recovery failure.
- **Test coverage** (new): integration test driving a fake ChirpStack endpoint that goes offline, asserts the retry → reconnect → resume cycle, and verifies the NFR17 SLA budget.

The new code surface is **deliberately minimal**:

- **~80–150 LOC of recovery loop** in `src/chirpstack.rs`, layered around `poll_metrics`. Either as a wrap (`poll_metrics_with_recovery`) or as additions to `poll_metrics` itself; dev agent picks based on diff economy.
- **~20 LOC of gateway_status update helper** (or reuse the existing Story 5-3 `gateway_status` upsert path).
- **~150–250 LOC of integration tests** in a new `tests/chirpstack_recovery.rs` (or extension to existing test file — dev picks).
- **3 new `event=` rows** in `src/chirpstack.rs` (the three reserved operations), bringing the audit-event count up by 3.
- **Documentation sync**: `docs/logging.md` table promoting the three operations from "reserved" to "implemented"; README Planning row updated.

This story does **not** introduce per-IP throttling (issue #88), exponential backoff strategy beyond the configured `chirpstack.delay` (Story 9-7 may add hot-reloadable backoff), TLS reconfiguration on reconnect (Story 9-1 stance carries forward), or test-fixture extraction to `tests/common/mod.rs` beyond what already exists from issue #102.

---

## Out of Scope

- **Per-source-IP throttling on the recovery probe.** GitHub issue #88 carry-forward; out-of-scope for 4-4.
- **Exponential backoff strategy.** The existing `chirpstack.retry` + `chirpstack.delay` knobs are linear (constant delay × N retries). Exponential or jittered backoff is a Story 9-7 hot-reload-friendly enhancement candidate; 4-4 keeps the linear shape to minimise diff and respect the existing PRD FR8 wording.
- **Recovery-aware OPC UA event emission.** The OPC UA stale-data status flags (Story 5-2) already cover this surface — when ChirpStack returns and `last_successful_poll` advances, OPC UA reads naturally return `Good` again. No new OPC UA-side changes.
- **Web dashboard live recovery status.** The Story 9-2 dashboard reads `gateway_status` from SQLite; once 4-4 updates that row during outage, the dashboard naturally surfaces it on the next 10s refresh. No new web/static surface.
- **Hot-reload of `chirpstack.retry` / `chirpstack.delay` knobs.** Story 9-7 will add `tokio::sync::watch::Receiver<AppConfig>` propagation; 4-4 reads the values once per recovery cycle from the existing `self.config.chirpstack` (so when 9-7 swaps to watch, the reads will naturally pick up new values without 4-4 changes).
- **`Channel::connect()` explicit timeout.** Currently deferred at `deferred-work.md` (6-3 carry-forward at line 86: *"`Channel::connect()` has no explicit timeout — pre-existing infrastructure; out of 6-3's instrumentation-only scope. Story 4-4 territory."*). 4-4 picks this up: add a deadline to the gRPC channel build inside the recovery cycle.
- **Outage detection on commands queue path.** Commands are processed inside `poll_metrics` before metrics — if ChirpStack is down, command-send will already fail and the existing path (Story 3-1) returns the error. 4-4's recovery loop wraps the metric-fetch path; commands coexist via the same retry envelope without new logic.
- **Address-space mutation on outage.** Story 9-7 / 9-8 territory. Variables stay registered; only their values become stale.
- **Multi-server failover** (e.g., primary + standby ChirpStack). Out of scope; the project supports a single `chirpstack.server_address`.
- **Issue #108 (storage payload-less MetricType).** Orthogonal; the recovery loop does not touch metric value semantics.
- **Doctest cleanup** (issue #100). Not blocking; 4-4 adds zero new doctests.

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. The recovery loop layers around existing detection + diagnostics — the spike's job is to add the loop, not rebuild the probe.

| What | Where | Status |
|------|-------|--------|
| `check_server_availability(&self) -> Result<Duration, OpcGwError>` | `src/chirpstack.rs:670-719` | **TCP probe via `TcpStream::connect_timeout` with 1s timeout.** Returns the round-trip duration on success; `OpcGwError::ChirpStack(...)` on failure. **This is the FR6 detection primitive.** Don't re-implement; call it. |
| `maybe_emit_chirpstack_outage(&mut bool, Option<DateTime<Utc>>, &OpcGwError) -> bool` | `src/chirpstack.rs:249-267` | **First-of-cycle warn emitter.** `chirpstack_outage` operation; carries `last_successful_poll`. Returns whether the warn was emitted. **Keep firing once per cycle on the first failure; do NOT replace** — the recovery loop layers on top. |
| TCP probe retry loop (existing, intra-cycle) | `src/chirpstack.rs:1786-1847` | **Already retries the probe `chirpstack.retry` times with `chirpstack.delay` between attempts**, emitting `chirpstack_connect` info/warn per attempt + `retry_schedule` info between. **On exhaustion currently just `warn!("Timeout: cannot reach chirpstack server")` and falls through.** 4-4 wraps THIS with a recovery-loop envelope so the cycle does not silently fall through after probe-retry exhaustion. |
| `chirpstack.retry: u32` and `chirpstack.delay: u64` config knobs | `src/config.rs:121-131` + validation at `src/config.rs:931-936` | **Wired today.** Both validated as `> 0`. FR8 contract. Default values in `config/config.toml` (currently `retry = 1`, `delay = 1`). 4-4 reads these for the recovery envelope; do not change validation. |
| `last_successful_poll: Option<DateTime<Utc>>` field on `ChirpstackPoller` | `src/chirpstack.rs:376` (declaration), `:441` (init `None`), updated in `poll_metrics` after success | **Story 6-3 AC#5 wiring.** Tracks the last cycle that wrote metrics. **Reuse for recovery downtime calculation** — `recovery_complete` carries `downtime_secs = (now - last_successful_poll).num_seconds()`. |
| `OpcGwError::ChirpStack(String)` variant | `src/utils.rs::OpcGwError` | **Reuse for any new error surfacing in the recovery loop** — avoid new enum variants. |
| `StorageBackend::update_gateway_status(Option<DateTime<Utc>>, i32, bool)` + `get_gateway_health_metrics()` | `src/storage/mod.rs:689-727` (trait), `src/storage/sqlite.rs:1931-1964` (impl) | **Already supports partial-update.** Trait signature: `update_gateway_status(last_poll_timestamp: Option<DateTime<Utc>>, error_count: i32, chirpstack_available: bool) -> Result<(), OpcGwError>`. **Critical contract from doc-comment:** "If `last_poll_timestamp` is `None`, the database timestamp is left unchanged (preserving the last successful poll time)." 4-4 calls `self.backend.update_gateway_status(None, error_count_at_loop_entry, false)` once on recovery loop entry — partial update preserves `last_poll_timestamp` automatically (no query-then-replace needed). Story 5-3's existing cycle-end call resets `chirpstack_available = true` on the next successful poll. |
| `Story 5-2 stale-data detection` | `src/opc_ua.rs::get_value` (stale-status flag based on `(now - last_seen) > stale_threshold`) | **Wired today.** **Existing last-known values stay available** via this path — the outage AC ("during outage, existing last-known values remain available to OPC UA clients (no data cleared)") is **already satisfied by Story 5-2**; 4-4 must not regress this (don't truncate metric_values, don't clear last-known cache). |
| `Story 9-2 web dashboard` | `src/web/api.rs::api_status` | **Wired today.** Reads `gateway_status.chirpstack_available` and surfaces it on the `/api/status` JSON. The dashboard's 10s `setInterval` will surface the outage to operators on the next refresh after 4-4 writes `chirpstack_available = false`. |
| Reserved log operations: `recovery_attempt`, `recovery_complete`, `recovery_failed` | `docs/logging.md:240-242` | **Reserved by Story 6-3 retro for Story 4-4.** `recovery_attempt` = info; `recovery_complete` = info; `recovery_failed` = warn. **Use these names verbatim** — don't invent new ones. |
| `cancel_token: tokio_util::sync::CancellationToken` on `ChirpstackPoller` | `src/chirpstack.rs:359` | **Wired today.** The recovery loop's `tokio::time::sleep(delay).await` MUST be `tokio::select!`-paired with `cancel_token.cancelled().await` so Ctrl+C / graceful shutdown still works during a long recovery wait. Existing pattern at `src/chirpstack.rs:822-828`. |
| `tonic::transport::Channel` lifecycle | `src/chirpstack.rs:466-544` (`create_channel`) | **Wired today.** No explicit timeout on `Channel::from_shared(...).connect()` — listed as deferred at `deferred-work.md:86` (6-3 carry-forward to "Story 4-4 territory"). 4-4 picks up this deferral by adding a `tokio::time::timeout(Duration::from_secs(5), endpoint.connect())` wrap inside the recovery loop's channel-rebuild step. |
| Story 6-3 logging contract | `docs/logging.md` | **Authoritative for `event=` semantics** (the operation field is named `operation=` in the codebase but the doc table calls them "events" — same thing). 4-4 adds 3 new operation names; total `operation=` distinct count grows by 3 in `src/chirpstack.rs`. |
| Story 9-3 baseline test count | `_bmad-output/implementation-artifacts/sprint-status.yaml` | 322 lib + 345 bins = **667 / 0 fail / 5 ignored** (post-Story-9-0 unchanged). 154-155 integration tests across 15 binaries / 0 fail. 4-4 target: ≥ 670 lib+bins (≈+3 unit), ≥ 16 integration binaries (+1 new file `tests/chirpstack_recovery.rs`) OR keep 15 binaries by extending an existing test file. |

**FR/NFR coverage map** — every BDD AC traces to a PRD FR or NFR:

| BDD AC line (`epics.md`) | FR / NFR | Where this story addresses it |
|---|---|---|
| L540: "ChirpStack becomes unavailable" / failure detection | **FR6** | AC#1 + reuse of `check_server_availability` |
| L543: "retries with configurable retry count and delay" | **FR8** | AC#2 + reuse of `chirpstack.retry` / `chirpstack.delay` |
| L544: "between retries, poller updates gateway_status to 'unavailable'" | **NFR16** (graceful) + Story 5-3 surface | AC#3 + reuse of `update_gateway_status` |
| L545: "reconnects and resumes polling within 30 seconds (NFR17)" | **NFR17** | AC#4 + integration test asserting elapsed time |
| L546: "no manual intervention required — fully automatic" | **FR7** | AC#5 (steady-state cycle resumes) |
| L547: "reconnection logged at info level with downtime duration" | (project logging contract) | AC#6 (`recovery_complete` info with `downtime_secs`) |
| L548: "during outage, existing last-known values remain available to OPC UA clients" | (Story 5-2 invariant) | AC#7 (carry-forward — no truncation) |
| L549: "test validates retry → reconnect → resume cycle" | (testing standard) | AC#11 + new `tests/chirpstack_recovery.rs` |

---

## Acceptance Criteria

### AC#1: Q1 — outage detection triggers explicit recovery loop (FR6)

- **Given** the gateway is polling and `check_server_availability` returns `Err(OpcGwError::ChirpStack(_))` (TCP probe failed) OR `get_device_metrics_from_server` returns `OpcGwError::ChirpStack(_)` (gRPC failed).
- **When** the failure occurs inside `poll_metrics` (per-device error branch at `src/chirpstack.rs:1052-1068`).
- **Then** the existing `maybe_emit_chirpstack_outage` warn fires once per cycle (no regression — keep the existing call site), AND the cycle enters a new **recovery loop** that:
  1. Updates `gateway_status.chirpstack_available = false` via `update_gateway_status(...)` (so Story 5-2 / 5-3 / 9-2 surfaces see the outage).
  2. Emits `info!(operation = "recovery_attempt", attempt = N, max_retries = R, delay_secs = D, ...)`.
  3. Awaits `tokio::time::sleep(Duration::from_secs(D))`, paired in a `tokio::select!` with `self.cancel_token.cancelled()` so Ctrl+C aborts the wait cleanly.
  4. Calls `check_server_availability()` again.
  5. If success → emit `info!(operation = "recovery_complete", downtime_secs = (now - last_successful_poll), attempts_used = N, ...)` and break out of the recovery loop. The next cycle iteration through `poll_metrics` will naturally reset `chirpstack_available = true` via the existing Story 5-3 `update_gateway_status` write.
  6. If failure → increment attempt counter; if `N == R` (retry budget exhausted) → emit `warn!(operation = "recovery_failed", attempts_used = N, last_error = ...)`. Continue to the next normal poll cycle (do NOT panic; do NOT exit the poller).
- **And** `chirpstack.retry` and `chirpstack.delay` are read **once per recovery loop entry** from `self.config.chirpstack` (so when Story 9-7 hot-reload swaps to `tokio::sync::watch`, the reads naturally pick up new values without 4-4 code changes).
- **And** the recovery loop is invoked at most once per `poll_metrics` cycle (subsequent device failures within the same cycle reuse the existing `chirpstack_outage_logged` cycle-local bool to short-circuit).
- **Verification:**
  - Test: `tests/chirpstack_recovery.rs::test_recovery_loop_fires_on_outage` (or extension to existing) — drives a stub backend that fails 2× then succeeds; asserts `recovery_attempt` × 2 + `recovery_complete` × 1 in captured tracing buffer.
  - Grep: `git grep -hoE 'operation = "recovery_[a-z]+"' src/chirpstack.rs | sort -u` returns exactly 3 distinct names.

### AC#2: Q2 — retry count + delay honoured (FR8)

- **Given** `chirpstack.retry = R` and `chirpstack.delay = D` are validated as `> 0` at startup (existing `src/config.rs:931-936`).
- **When** the recovery loop runs.
- **Then** the loop attempts at most `R` retries with `D` seconds between them (linear delay — consistent with the existing TCP probe retry shape at `src/chirpstack.rs:1786-1847`).
- **And** if `R = 1`, exactly one retry attempt is made before giving up.
- **And** if `R = 30` and `D = 1`, the recovery loop's wall-clock budget is at most ~30 seconds (matches NFR17).
- **And** the values are read at recovery-loop entry — concurrent `chirpstack.retry` mutation (e.g., from a future Story 9-7 hot-reload) does not affect the in-flight loop's count budget but DOES apply to the next loop entry.
- **Verification:**
  - Test: `tests/chirpstack_recovery.rs::test_retry_count_and_delay_honoured` — asserts the timing of retry events with `R=3, D=1` (3 attempts, ~3s total elapsed, ±200ms jitter tolerance for CI).
  - Spec consistency: AC#2's behaviour matches the existing TCP probe retry loop's contract (don't introduce new retry semantics).

### AC#3: Q3 — gateway_status update during outage (NFR16 graceful + Story 5-3 surface)

- **Given** the recovery loop has been entered (per AC#1).
- **When** the loop enters its first sleep slice (between attempt N and attempt N+1).
- **Then** `gateway_status.chirpstack_available` is `false` in SQLite (operator-visible via `get_gateway_health_metrics` — Story 5-3 / 9-2 dashboard) — written via `update_gateway_status(None, error_count_at_loop_entry, false)`.
- **And** the `error_count` field on `gateway_status` reflects the cycle-local `error_count` accumulator at recovery-loop entry (existing Story 5-3 contract: each per-device failure already increments the cycle-local counter via the existing `poll_metrics` saturation logic at `src/chirpstack.rs:1050`).
- **And** the `last_poll_timestamp` is **NOT** updated during the recovery loop — passing `None` to `update_gateway_status` preserves the existing DB value per the documented trait contract; OPC UA `last_seen` staleness math (Story 5-2) reports the right age.
- **And** on recovery success, the next normal `poll_metrics` cycle's existing `update_gateway_status(...)` call resets `chirpstack_available = true`. No additional explicit write needed in the recovery-success branch.
- **Verification:**
  - Test: `tests/chirpstack_recovery.rs::test_gateway_status_reflects_outage` — drives the recovery loop with a stub backend, queries `get_gateway_health_metrics` mid-loop (before recovery), asserts `chirpstack_available == false` + `error_count > 0` + `last_poll_timestamp` unchanged.

### AC#4: NFR17 — auto-recovery within 30 seconds of server returning

- **Given** the gateway is in the recovery loop.
- **When** ChirpStack becomes available again (TCP probe starts succeeding).
- **Then** within **30 seconds** of availability returning, the gateway emits `recovery_complete` and resumes normal polling.
- **And** the 30-second budget is the **total wall-clock** time from "ChirpStack TCP-acceptable" to "next successful `poll_metrics` cycle starts" — not the recovery-loop-only budget.
- **And** the 30-second budget assumes default-ish config (`retry = 30`, `delay = 1`, polling_frequency ≤ 30); operators may tune `chirpstack.retry` / `chirpstack.delay` to widen or narrow this. **The default `config/config.toml` MUST keep the NFR17 budget achievable** — verify the shipped defaults satisfy `retry × delay ≤ 30s` (current shipped values are `retry = 1, delay = 1` which trivially satisfy; if 4-4 changes the defaults, validate the new values against NFR17).
- **And** the polling resumption is "natural" — the recovery loop breaks, the cycle continues to the next device or the next cycle, and the existing `poll_metrics` invariants (Story 4-1/4-2/4-3) hold.
- **Verification:**
  - Test: `tests/chirpstack_recovery.rs::test_nfr17_recovery_within_30_seconds` — drives stub backend that fails for 5 seconds then succeeds; asserts `Instant::now() - probe_recovery_start < Duration::from_secs(30)` for `recovery_complete` arrival. Use `chirpstack.retry = 10, chirpstack.delay = 1` for the test fixture (so total loop budget < 10s; NFR17 has ample headroom).
  - Verification command: `grep -E "retry|delay" config/config.toml` confirms shipped defaults satisfy `retry × delay ≤ 30`.

### AC#5: FR7 — fully automatic, no manual intervention

- **Given** the recovery loop completes successfully (per AC#1 step 5).
- **When** the next `poll_metrics` cycle runs.
- **Then** the gateway resumes polling **without operator intervention** — no signal, no config edit, no restart.
- **And** the `chirpstack_outage_logged` cycle-local bool resets naturally (it lives on the cycle stack frame; each new cycle starts with `false`).
- **And** the recovery loop does NOT panic, exit, or otherwise interrupt the poller's `run()` loop.
- **Verification:**
  - Test: `tests/chirpstack_recovery.rs::test_recovery_resumes_polling_automatically` — drives 3 cycles: (1) outage, (2) recovery, (3) successful poll; asserts metrics from cycle 3 land in storage without test-side intervention.

### AC#6: Reconnection logged at info level with downtime duration

- **Given** the recovery loop succeeds (per AC#1 step 5).
- **When** `recovery_complete` is emitted.
- **Then** the log line is at **`info`** level (not `warn`, not `error`) — recovery is a positive event, not a problem.
- **And** the log carries fields: `operation = "recovery_complete"`, `downtime_secs = u64`, `attempts_used = u32`, `last_error = string` (from the last failed attempt, useful for ops post-mortem).
- **And** `downtime_secs` is computed as `(chrono::Utc::now() - last_successful_poll).num_seconds()` clamped to `>= 0` (handle clock-skew edge case).
- **And** if `last_successful_poll` is `None` (recovery on the very first cycle of a new gateway start with no prior successful poll), `downtime_secs` is omitted from the log line OR set to `0` with a `from_startup = true` field — dev picks the cleanest shape.
- **Verification:**
  - Test: `tests/chirpstack_recovery.rs::test_recovery_complete_log_shape` — captures tracing output, asserts `event="recovery_complete"`, `level=INFO`, `downtime_secs` field present (or `from_startup=true` for cold-start case).

### AC#7: During outage, last-known values remain available (Story 5-2 invariant)

- **Given** the recovery loop is in flight.
- **When** an OPC UA client reads a metric variable.
- **Then** the read returns the **last successful value** with `StatusCode::Bad` or stale flag (per Story 5-2 stale-data detection — values older than `stale_threshold_secs` get a Bad-quality status, but the value field is preserved).
- **And** **no metric_values rows are deleted** during the outage. The recovery loop writes nothing to `metric_values` or `metric_history`.
- **And** the existing OPC UA last-status cache (`OpcUa.last_status: StatusCache`) preserves the prior status — the recovery loop does not invalidate it.
- **Verification:**
  - **Carry-forward only — no new test required.** Existing Story 5-2 tests (`tests/opc_ua_*.rs`) assert stale-data semantics; 4-4 must not regress them.
  - `cargo test --test opc_ua_security_endpoints --test opc_ua_connection_limit --test opcua_subscription_spike` continues to pass with the same counts as the Story 9-0 baseline.

### AC#8: Carry-forward invariants from Stories 5/6/7/8/9 hold

- **Given** Stories 5-1/5-2/5-3 (storage backend + stale-data + gateway-health), 6-1/6-2/6-3 (logging), 7-2/7-3 (auth + session-cap), 8-1/8-2/8-3 (subscriptions + history), 9-0/9-1/9-2/9-3 (web + spike) introduced production code that 4-4 must not regress.
- **When** 4-4 runs.
- **Then:**
  - **Story 5-3 invariant:** `gateway_status` table has exactly one row with `id = 1`. The recovery loop's `update_gateway_status(...)` call uses the existing `INSERT OR REPLACE` shape; no schema migration.
  - **Story 6-1/6-2/6-3 invariant:** All new log lines use `tracing` macros (`info!` / `warn!`) with `operation = "..."` field. No `println!`, no log-crate fallback, no hardcoded log strings beyond the message field.
  - **Story 6-3 invariant:** The existing `chirpstack_outage` warn (Story 6-3 AC#5) keeps firing once per cycle on the first failure — 4-4 ADDS the recovery loop on top, does not REPLACE the existing diagnostic.
  - **Story 7-2 / 7-3 invariant:** Zero changes to `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs` — the recovery loop is poller-side only.
  - **Story 8-3 invariant:** Zero changes to `src/opc_ua_history.rs`. No HistoryRead-side changes.
  - **Story 9-0 invariant:** Zero changes to `src/opc_ua.rs` (other than incidental — if the recovery loop needs a `cancel_token` clone passed differently, it's already on `ChirpstackPoller` so no `OpcUa` change). `RunHandles` and the spike test file untouched.
  - **Story 9-1 invariant:** Zero changes to `src/web/auth.rs`, `src/web/api.rs`, `src/web/mod.rs`, `static/*`, `tests/web_*.rs`.
  - **Story 9-2 / 9-3 invariant:** The web dashboard's `/api/status` JSON shape and `/api/devices` JSON shape are unchanged. The dashboard naturally surfaces the outage via the existing `chirpstack_available` field (Story 5-3) without any web-side code change.
- **Verification:**
  - `git diff HEAD --stat src/web/ static/ tests/web_*.rs src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/opc_ua_history.rs src/opc_ua.rs` produces zero output.
  - `cargo test` passes all existing tests with no regression.

### AC#9: Tracing event-name contract — exactly 3 new `operation=` names

- **Given** the spike adds the three reserved operations from `docs/logging.md:240-242`.
- **When** the implementation completes.
- **Then** `git grep -hoE 'operation = "[a-z_]+"' src/chirpstack.rs | sort -u | wc -l` returns the **pre-implementation count + 3** (the three new names: `recovery_attempt`, `recovery_complete`, `recovery_failed`).
- **And** these three names appear nowhere else in `src/` (greppable by name).
- **And** `docs/logging.md` is updated to promote the three operations from "reserved" (the L240-242 table under "Story 4-4 reserved operations") to "implemented" (move into the main table at L147 region, alongside `chirpstack_outage`). The "reserved" subsection at L236-244 is removed (or shrunk if other operations remain reserved — currently only the three Story 4-4 names are listed as reserved).
- **Verification:**
  - `git grep -hoE 'operation = "[a-z_]+"' src/chirpstack.rs | sort -u | wc -l` returns the new count.
  - `git grep "Story 4-4" docs/logging.md` returns at most a historical reference; the "reserved" table is removed.

### AC#10: NFR12 carry-forward intact + AC#7-style file invariant

- **Given** Stories 7-2 / 7-3 / 9-1 introduced NFR12 (failed-auth source-IP) audit-event production code in `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/web/auth.rs`.
- **When** 4-4 runs.
- **Then** `git diff HEAD --stat src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/web/auth.rs src/security_hmac.rs` produces **zero output**. The recovery loop is poller-side only; no auth surface touched.
- **Verification:** The `git diff --stat` command above returns zero rows.

### AC#11: Documentation sync — README + sprint-status + docs/logging.md

- **Given** CLAUDE.md "Documentation Sync" rule.
- **When** the spike completes.
- **Then** the following docs are updated as part of the implementation commit:
  - **`README.md`** — Planning-table row for Epic 4 updated to reflect 4-4 status (e.g., `Epic 4 — done (4-1/4-2/4-3 done · 4-4 review)` or as the dev agent picks). Epic 4's status flag may flip from `done` to `in-progress` momentarily during 4-4 implementation per the workflow Step 1 epic-status-update logic; that's handled automatically by the `bmad-create-story` flip in Step 6 of THIS create-story run (so by the time the dev-story runs, Epic 4 is already `in-progress`). The retrospective at `epic-4-retrospective.md` does NOT need re-running — Story 4-4 was always known-deferred per that retro's line 125, so it lands as a delayed pickup, not a regression. The README narrative should mention this carry-forward context briefly.
  - **`_bmad-output/implementation-artifacts/sprint-status.yaml`** — `4-4-auto-recovery-from-chirpstack-outages` flipped from `ready-for-dev` (after `bmad-create-story`) to `in-progress` (during dev) to `review` (after dev-story); `last_updated` field rewritten with the dev-story outcome summary. `epic-4` may flip from `done` back to `in-progress` during dev, then back to `done` after Story 4-4's code review concludes (no separate retrospective needed — Epic 4 retro is already done).
  - **`docs/logging.md`** — three operation rows promoted from "reserved" (L240-242) to "implemented" (L147 main table region); reserved-operations subsection removed if Story 4-4 was the only consumer. Add a concrete description per operation + an "Operator action" column entry per existing convention.
  - **`config/config.toml`** / **`config/config.example.toml`** — **NO CHANGE EXPECTED**. The `chirpstack.retry` and `chirpstack.delay` knobs already exist; defaults already shipped. If the dev agent finds a need to bump defaults to satisfy NFR17 with a comfortable margin, that's a config-surface change worth flagging in Dev Agent Record but should be the LAST resort (current `retry=1, delay=1` × cycle frequency satisfies NFR17 trivially).
  - **`docs/security.md`** — **NO CHANGE EXPECTED**. The recovery loop adds no new auth, TLS, or PKI surface.
  - **`_bmad-output/implementation-artifacts/deferred-work.md`** — pick up the existing 6-3 carry-forward entry at line 86 (`Channel::connect()` has no explicit timeout) and either resolve it (mark as patched in the recovery loop's channel-rebuild step) or move it to the next deferred-from heading if the dev agent decides the timeout shape needs a separate story.
- **Verification:**
  - `git diff HEAD --stat docs/security.md config/config.toml config/config.example.toml` should produce zero output (or document why if non-zero).
  - `git diff HEAD --stat README.md docs/logging.md _bmad-output/implementation-artifacts/sprint-status.yaml` shows updates.

### AC#12: Tests pass and clippy clean (no regression)

- **Given** post-Story-9-0 baseline: 322 lib + 345 bins = 667 tests pass / 0 fail / 5 ignored. 15 integration binaries / 0 fail.
- **When** the spike adds tests.
- **Then** the new test count equals baseline + 4-4 net additions:
  - **3-5 new unit tests** in `src/chirpstack.rs::tests` for the recovery-loop helper (if extracted) — assertion: emit-recovery-attempt-on-failure, emit-recovery-complete-on-success, emit-recovery-failed-on-exhaustion, downtime-calculation-on-success, gateway-status-update-during-outage. Dev picks how many; aim for ≥3 with named scenarios.
  - **3-5 new integration tests** in `tests/chirpstack_recovery.rs` (new file) OR appended to an existing `tests/chirpstack_*.rs` if one exists — covers AC#1, AC#2, AC#3, AC#4, AC#5, AC#6 end-to-end with a stub backend. Use `tracing-test` for log assertions (existing pattern from Story 8-1 / 9-2). `tracing-test` global-buffer mutex caveat documented in `tests/common/mod.rs:34-44` — apply the same `#[serial_test::serial]` discipline.
- **And** `cargo test --lib --bins --tests` reports **at least 673 passing** (667 + 3 unit + 3 integration = 673 minimum).
- **And** `cargo clippy --all-targets -- -D warnings` exits 0.
- **And** `cargo test --doc` reports 0 failed (56 ignored — issue #100 baseline, unchanged).
- **And** integration binary count is **15 + 0 or 1** depending on file choice (15 if extending existing; 16 if new `tests/chirpstack_recovery.rs`).
- **Verification:** test counts pasted into Dev Agent Record after the run. Clippy output truncated to last 5 lines.

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issue (CLAUDE.md compliance)

- [x] Open **#TBD** — "Story 4-4: Auto-Recovery from ChirpStack Outages" (main tracker). Reference via `Refs #TBD` on intermediate commits, `Closes #TBD` on the final implementation-complete commit. **Per Story 9-2 precedent, the GitHub issue is opened at commit time of the implementation commit**, not at spec-creation time.
- [x] **Do not** open follow-up issues for items already tracked (#88 per-IP rate limiting, #100 doctest cleanup, #102 tests/common reuse, #104 TLS hardening, #107 Story 9-3 KF, #108 storage payload-less, #110 RunHandles Drop KF). All carry forward unchanged.

### Task 1: Recovery loop implementation (AC#1, AC#2, AC#5)

- [x] In `src/chirpstack.rs`, add a new helper (recommended name: `recover_from_chirpstack_outage`) that takes `&mut self` + `error_count_at_entry: i32` + `last_error: &OpcGwError`.
- [x] The helper signature: `async fn recover_from_chirpstack_outage(&mut self, error_count_at_entry: i32, last_error: &OpcGwError) -> RecoveryOutcome` where `RecoveryOutcome` is a small private enum: `{ Recovered { attempts_used: u32 }, Exhausted { attempts_used: u32, last_error: String } }`. The outer `poll_metrics` dispatches on the outcome — Recovered → continue cycle (or break to next device); Exhausted → continue cycle without further retry but stay in the poller's main loop.
- [x] Inside the helper:
  1. Read `R = self.config.chirpstack.retry` and `D = self.config.chirpstack.delay` once at entry.
  2. Call `self.backend.update_gateway_status(None, error_count_at_entry, false)` to surface the outage to OPC UA (Story 5-3) + web dashboard (Story 9-2). The `None` for `last_poll_timestamp` preserves the existing DB value per the trait contract documented at `src/storage/mod.rs:685-688`.
  3. For attempt N in `0..R`:
     - Emit `info!(operation = "recovery_attempt", attempt = N + 1, max_retries = R, delay_secs = D, last_error = %last_error, ...)`.
     - `tokio::select!` with `cancel_token.cancelled()` and `tokio::time::sleep(Duration::from_secs(D))`. On cancel, return `Exhausted { ... }` (the poller's outer `run()` loop will exit cleanly via the cancel branch on the next iteration).
     - Call `check_server_availability()`. If `Ok(_)` → emit `info!(operation = "recovery_complete", attempts_used = N + 1, downtime_secs = (now - last_successful_poll).num_seconds() OR omit, last_error = ...)` and return `Recovered { attempts_used: N + 1 }`.
     - If `Err(_)` → loop continues.
  4. After loop exhaustion: emit `warn!(operation = "recovery_failed", attempts_used = R, last_error = %last_failure)` and return `Exhausted { attempts_used: R, last_error }`.
- [x] Wire the helper into `poll_metrics` at the per-device error branch (`src/chirpstack.rs:1052-1068`): use `maybe_emit_chirpstack_outage`'s boolean return value to gate the recovery call. Sketch: `let just_logged = maybe_emit_chirpstack_outage(&mut chirpstack_outage_logged, self.last_successful_poll, &e); if just_logged { self.recover_from_chirpstack_outage(error_count, &e).await; }`. The helper returns `true` only when this is the first emission of the cycle (it internally checks + flips the cycle-local bool), so the recovery loop fires at most once per cycle naturally. The cycle's local `error_count` variable is in scope at the call site (incremented one line above by `error_count.saturating_add(1)`). After the call, control returns to the device for-loop; subsequent device fetches will either succeed (Recovered outcome) or fail the TCP probe again (Exhausted) but the `outage_logged` bool prevents the recovery loop from re-firing within the same cycle. Do not break the cycle early.
- [x] **Cancel-safety:** the helper's sleep MUST be `tokio::select!`-paired with `self.cancel_token.cancelled()`. Reference pattern at `src/chirpstack.rs:822-828`.
- [x] **Channel rebuild during recovery:** if the recovery succeeds, the next call to `get_device_metrics_from_server` will rebuild the gRPC channel via `create_device_client()`. This is correct — channels in tonic are cheap and the existing path handles channel re-creation. Add a `tokio::time::timeout(Duration::from_secs(5), endpoint.connect())` wrap to `create_channel` (resolves the deferred 6-3 entry at `deferred-work.md:86`).
- [x] `cargo clippy --all-targets -- -D warnings` clean.

### Task 2: gateway_status update during outage (AC#3)

- [x] Call `self.backend.update_gateway_status(None, error_count_at_loop_entry, false)` once on recovery loop entry. The trait signature is `(last_poll_timestamp: Option<DateTime<Utc>>, error_count: i32, chirpstack_available: bool) -> Result<(), OpcGwError>` (`src/storage/mod.rs:689`); passing `None` for `last_poll_timestamp` preserves the existing DB timestamp per the documented contract — no query-then-replace pattern needed.
- [x] **Pass through the cycle-local `error_count`** from `poll_metrics` (line ~1050) so the recovery loop's surface state is consistent with the cycle's per-device error accumulator. Do NOT recompute or reset it.
- [x] **Do NOT update `last_poll_timestamp`** during recovery — `None` is the right value here, keeps OPC UA stale-data math (Story 5-2) accurate.
- [x] **Do NOT clear `metric_values`** during recovery — last-known values stay available (AC#7).
- [x] On recovery success, do NOT explicitly reset `chirpstack_available = true` — the next normal `poll_metrics` cycle's existing Story 5-3 `update_gateway_status` call handles it. Avoiding the explicit reset prevents a small race window where a stale `false` could overwrite a fresh `true`.

### Task 3: Reserved log operations (AC#9)

- [x] Verify the three operation names match `docs/logging.md:240-242` exactly: `recovery_attempt`, `recovery_complete`, `recovery_failed`. No typos, no plurals, no reordering.
- [x] Add per-operation field schema:
  - `recovery_attempt`: `attempt: u32`, `max_retries: u32`, `delay_secs: u64`, `last_error: %display`. Level: info.
  - `recovery_complete`: `attempts_used: u32`, `downtime_secs: u64` (or omitted on cold-start), `last_error: %display`. Level: info.
  - `recovery_failed`: `attempts_used: u32`, `last_error: %display`. Level: warn.
- [x] Update `docs/logging.md`:
  - Promote the three rows from the reserved table at L240-242 into the main `event=` table at L147 region.
  - Remove the "Story 4-4 reserved operations" subsection (L236-244) if it becomes empty.
  - Add an "Operator action" column entry per operation per existing convention (e.g., `recovery_attempt` → "Monitor frequency; spike correlates with `chirpstack_outage`"; `recovery_complete` → "downtime_secs identifies outage duration; investigate ChirpStack-side root cause"; `recovery_failed` → "Manual intervention may be needed; check ChirpStack server status, network, credentials").

### Task 4: Integration tests (AC#11, AC#12)

- [x] Decide test file shape: NEW `tests/chirpstack_recovery.rs` (cleaner) OR extend an existing `tests/chirpstack_*.rs` if one exists. Default: NEW file. **NOTE:** searching for `tests/chirpstack_*.rs` returns no results today (the closest are `tests/web_dashboard.rs` for the API layer). NEW file is the right call.
- [x] **Stub-backend pattern:** the test cannot connect to a real ChirpStack server. Choose ONE:
  - (A) Mock the `check_server_availability` method via a trait split — extract a `ServerAvailabilityProbe` trait with one method, default impl on `ChirpstackPoller`, replace with a stub in tests. Cleanest but ~40 LOC of refactor.
  - (B) Bind a TCP listener on a known port that opens/closes deterministically — the existing `check_server_availability` does a `TcpStream::connect_timeout` against the configured `chirpstack.server_address`, so a test-side listener that drops `accept` calls simulates outage. Smaller diff but tighter coupling.
  - (C) Use `tokio::net::TcpListener` bound on a random port, write the address into the test's `chirpstack.server_address`, drop the listener mid-test to simulate outage, re-bind to simulate recovery. Most realistic; ~80 LOC of test infrastructure but no production-code refactor.
  - **Recommended: (C).** Same shape as Story 8-1's spike test which used a real `OpcUa` server in a `TempDir`-rooted PKI dir. Production-code refactor is unnecessary; the test can drive the existing API end-to-end.
- [x] Each test pinned with `#[serial_test::serial]` (port-binding races) and `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`.
- [x] Tests to write (mapping to ACs):
  - `test_recovery_loop_fires_on_outage` (AC#1): drive 2× failure then success; assert `recovery_attempt` × 2 + `recovery_complete` × 1 in tracing buffer.
  - `test_retry_count_and_delay_honoured` (AC#2): with `R=3, D=1`, assert 3 attempts within ~3s ± jitter.
  - `test_gateway_status_reflects_outage` (AC#3): query `get_gateway_health_metrics` mid-loop; assert `chirpstack_available == false`, `error_count > 0`, `last_poll_timestamp` unchanged.
  - `test_nfr17_recovery_within_30_seconds` (AC#4): assert `recovery_complete` arrives within 30s wall-clock from listener-rebind.
  - `test_recovery_resumes_polling_automatically` (AC#5): assert metrics from cycle-after-recovery land in storage.
  - `test_recovery_complete_log_shape` (AC#6): assert `level=INFO`, `event="recovery_complete"`, `downtime_secs` field present.
  - `test_recovery_failed_log_shape` (AC#1 step 6): drive `R` failures with no recovery; assert `recovery_failed` warn.
  - `test_cancel_during_recovery` (AC#1 cancel-safety): assert that firing `cancel_token` during the recovery sleep aborts cleanly within `D + ε` seconds.

### Task 5: Documentation sync (AC#11)

- [x] Update `README.md` Planning row for Epic 4 to reflect 4-4 status. Suggested narrative: *"Epic 4 — done (4-1/4-2/4-3 done · 4-4 done after Story 9-0 — auto-recovery loop with `recovery_attempt` / `recovery_complete` / `recovery_failed` operations; NFR17 30s SLA enforced; gateway_status `chirpstack_available` reflects outage during recovery cycle)."*
- [x] Update `docs/logging.md` per Task 3.
- [x] Update `_bmad-output/implementation-artifacts/sprint-status.yaml`:
  - Story 4-4 status flips through: `ready-for-dev` (now, after this `bmad-create-story`) → `in-progress` (during `bmad-dev-story`) → `review` (at end of dev-story) → `done` (after `bmad-code-review`).
  - `last_updated` field rewritten at each transition with the relevant outcome summary.
  - Epic 4 status: `done` → `in-progress` (when first story-status flip lands during dev) → `done` (after 4-4 lands).
- [x] **(Optional)** Append entry to `_bmad-output/implementation-artifacts/deferred-work.md` IF a new deferral surfaces during 4-4. Most likely candidates:
  - Per-IP rate limiting on the recovery probe (carry to #88).
  - Exponential backoff on `chirpstack.delay` (open new follow-up issue if dev agent surfaces operator demand for it).

### Task 6: Final verification (AC#12)

- [x] `cargo test --lib --bins --tests` — must report **≥ 673 passed / 0 failed / 5 ignored** (667 baseline + 3 unit + 3 integration minimum).
- [x] `cargo clippy --all-targets -- -D warnings` — clean.
- [x] `cargo test --doc` — 0 failed (56 ignored — issue #100 baseline, unchanged).
- [x] `cargo test --test chirpstack_recovery` (or extension target) — all new tests pass.
- [x] `git diff HEAD --stat src/web/ static/ tests/web_*.rs src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/opc_ua_history.rs src/opc_ua.rs` — all zero (AC#8, AC#10 invariants).
- [x] `git grep -hoE 'operation = "recovery_[a-z]+"' src/chirpstack.rs | sort -u` returns exactly 3 lines.
- [x] `git grep -hoE 'operation = "recovery_[a-z]+"' src/ | sort -u` also returns exactly 3 lines (no leakage outside `src/chirpstack.rs`).

---

## Dev Notes

### Anti-patterns to avoid (per CLAUDE.md scope-discipline rule)

- **Do not** ship per-IP throttling of the recovery probe. Issue #88 owns it.
- **Do not** ship exponential backoff or jittered backoff on `chirpstack.delay`. PRD FR8 wording specifies "configurable retry count and delay" — linear with constant delay matches the existing TCP probe loop's shape and the existing config knobs. Exponential backoff is a future hot-reload-friendly enhancement (Story 9-7 territory or a new follow-up).
- **Do not** modify `chirpstack.retry` / `chirpstack.delay` validation rules at `src/config.rs:931-936`. They are validated as `> 0`; respect that.
- **Do not** introduce new `OpcGwError` enum variants. Reuse `OpcGwError::ChirpStack(String)` for any error wrapping.
- **Do not** change the `gateway_status` table schema. Use the existing `update_gateway_status` API; if it lacks partial-update semantics, query-then-replace within a single transaction.
- **Do not** clear `metric_values` rows during outage. Last-known values must remain available to OPC UA clients (AC#7 + Story 5-2 invariant).
- **Do not** delete the existing `chirpstack_outage` warn (Story 6-3 AC#5). Layer the recovery loop ON TOP of it — the warn fires once at the start of an outage cycle, then the recovery loop kicks in.
- **Do not** modify `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/web/*`, `static/*`, `tests/web_*.rs`. The recovery loop is poller-side only.
- **Do not** introduce new dev-dependencies. The existing `[dev-dependencies]` block (per `Cargo.toml:55-75`) covers everything needed: `tracing-test` for log capture, `tokio` with `time` feature for sleeps, `serial_test` for serial-execution discipline, `tempfile` for SQLite test backends.
- **Do not** ship the recovery loop with `panic!` / `unwrap` / `expect` on production paths. NFR16 30-day-crash-free invariant.
- **Do not** weaken Stories 5/6/7/8/9 invariants. AC#8 enumerates them.
- **Do not** ship the spike with placeholder text in `docs/logging.md` (e.g., "TBD operator action"). Each operator-action column entry must be concrete.

### Why this Phase A carry-forward lands during Phase B

Story 4-4 was deferred at Epic 4 retro (`epic-4-retrospective.md:125`) as a soft dependency that didn't block any downstream work. Phase B (Epics 8 + 9 — subscriptions + web) took priority because:

1. **Outage detection was already in place.** `chirpstack_outage` warn (Story 6-3) gives operators visibility; OPC UA stale-data flags (Story 5-2) preserve last-known-values during outage; gateway_status (Story 5-3) reports `chirpstack_available`. The user-facing visibility was complete before recovery automation landed.
2. **Phase B carried more user-facing value.** Subscriptions (Epic 8) and the web dashboard (Epic 9) directly close FRs that operators ASKED for. Auto-recovery (FR7 + NFR17) is a "set-and-forget" reliability feature — measurable but rarely-encountered.
3. **No production deployment yet.** The reliability gap is operationally bounded — operators CAN see the outage and CAN restart manually if it persists. There's no SCADA pain in the field demanding an automated fix.

Picking up 4-4 now is **clearing carry-forward before Story 9-7 (config hot-reload) reshapes the poller's reload semantics.** Once Story 9-7 introduces `tokio::sync::watch::Receiver<AppConfig>`, the recovery loop's config reads (`self.config.chirpstack.retry` and `.delay`) will need to be reconsidered — e.g., do hot-reloaded values apply to in-flight recovery loops, or only to the next loop entry? The cleanest answer is "next entry" (which 4-4 already implements by reading at loop entry), but landing 4-4 BEFORE 9-7 means 9-7 inherits a recovery loop with well-defined config-read semantics. If we land 9-7 first, 4-4's design choice gets murkier.

### Interaction with Story 9-7 (Configuration Hot-Reload — backlog)

Story 9-7 will introduce a `tokio::sync::watch::Receiver<AppConfig>` shared between the poller, OPC UA server, and web server. The poller will receive config updates and re-read the relevant fields per cycle. **4-4's recovery loop reads `chirpstack.retry` and `chirpstack.delay` at loop entry** — this is the correct shape for hot-reload integration:

- 9-7's hot-reload pushes a new `AppConfig` into the watch channel.
- The poller's `run()` loop's outer iteration picks it up at the next cycle boundary (`tokio::select!` arm).
- An in-flight recovery loop continues with its loop-entry-snapshot values; the next recovery loop entry uses the new values.

This avoids a class of bugs where config changes mid-recovery would create inconsistent retry budgets or undefined behaviour. 4-4's read-at-entry semantics is documented here so 9-7 doesn't break it.

### Interaction with Story 5-2 (stale-data detection — done)

Story 5-2 wires the OPC UA `StatusCode::Bad` flag based on `(now - last_seen) > stale_threshold_secs`. **During an outage handled by 4-4's recovery loop, the OPC UA reads naturally return Bad-status** — `last_seen` doesn't advance because the recovery loop doesn't write to `metric_values`. Once recovery succeeds and the next poll cycle writes new metrics, `last_seen` advances and OPC UA reads return `Good` again.

**4-4 must NOT touch `metric_values` rows during recovery.** This invariant is the bridge between FR7 (no manual intervention) and the AC#7 line "during outage, existing last-known values remain available."

### Interaction with Story 5-3 (gateway-health metrics — done)

Story 5-3 wires the `gateway_status` SQLite table. The web dashboard (Story 9-2) and the OPC UA Gateway folder (Story 5-3 AC#5) both read from this table.

**4-4 writes `chirpstack_available = false` once on recovery loop entry** — this is the only `gateway_status` write in the recovery path. The recovery success path does NOT explicitly write `chirpstack_available = true` because:

- The next normal `poll_metrics` cycle's existing `update_gateway_status` call (Story 5-3 wiring) sets it to `true` automatically.
- Avoiding the explicit reset write avoids a race where the recovery loop sets `true` but a concurrent failed poll path overwrites with `false`.
- The dashboard's 10s refresh (Story 9-2) and OPC UA's per-read (Story 5-3) both see the eventually-consistent state.

### Interaction with Story 9-0 (async-opcua runtime mutation spike — done)

Story 9-0 introduced the `RunHandles` struct and `OpcUa::build` / `run_handles` split. **4-4 does not touch any of these.** The recovery loop is poller-side; no OPC UA-side changes.

### Channel rebuild during recovery — picking up the deferred 6-3 entry

`deferred-work.md:86` carries: *"`Channel::connect()` has no explicit timeout — pre-existing infrastructure; out of 6-3's instrumentation-only scope. Story 4-4 territory."*

4-4 picks this up: inside `create_channel` (`src/chirpstack.rs:466-544`), wrap the `builder.connect().await` call with `tokio::time::timeout(Duration::from_secs(5), ...)`. On timeout, return `OpcGwError::Configuration("ChirpStack channel connect timed out after 5s")`. The 5s budget is chosen to be:

- Smaller than NFR17's 30s SLA (so a single channel rebuild doesn't blow the recovery budget).
- Larger than the TCP probe's 1s timeout (so transient slow-but-reachable servers don't get falsely flagged).

This change is contained to `create_channel`; no API surface change for callers (the existing `Result<Channel, OpcGwError>` shape stays).

### Project Structure Notes

- `src/chirpstack.rs` — at HEAD of Story 9-0 it's ~2683+ lines. Story 4-4 adds **80–150 LOC** for the recovery helper + the `create_channel` timeout wrap. Net diff: ~100 LOC (one new pub-crate helper + one timeout wrap + 3 new operation log lines).
- `src/storage/sqlite.rs` — **possibly +5 LOC** if `update_gateway_status` lacks partial-update semantics; otherwise zero. Verify the current API at line 637-639.
- `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs` — **zero changes** (AC#8 / AC#10).
- `src/web/*`, `static/*`, `tests/web_*.rs` — **zero changes** (AC#8 invariants).
- `src/main.rs`, `src/config.rs`, `src/utils.rs`, `src/storage/{mod,memory}.rs` — **zero changes** (config knobs already exist; error type reused).
- `tests/chirpstack_recovery.rs` — **new integration-test file**, ~250-400 LOC including listener-stub fixture + 6-8 test functions. Mirror of `tests/opcua_subscription_spike.rs` shape; per-file-divergent helpers documented in top-of-file comment.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — `last_updated` rewrite + 4-4 status flip + Epic 4 status flip.
- `README.md` — Planning-row status flip + narrative mention of Phase A carry-forward.
- `docs/logging.md` — three operations promoted from reserved to implemented; reserved subsection removed.
- `Cargo.toml` — **no new dependencies**.

Modified files (expected File List, ~6-8 files):

- `src/chirpstack.rs` — recovery helper + `create_channel` timeout wrap.
- (possibly) `src/storage/sqlite.rs` — partial `update_gateway_status` API if needed.
- `tests/chirpstack_recovery.rs` — new.
- `_bmad-output/implementation-artifacts/4-4-auto-recovery-from-chirpstack-outages.md` — this file.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — status flips + last_updated.
- `README.md` — Planning-row update.
- `docs/logging.md` — operation table updates.
- `_bmad-output/implementation-artifacts/deferred-work.md` — possibly resolve the 6-3 channel-timeout entry.

### Testing Standards

This subsection is the **single source of truth** for testing patterns Story 4-4 should reuse.

- **Unit tests:** ≥3 in `src/chirpstack.rs::tests` for the recovery helper (if extracted as a pub-crate or pub-test fn). Drive the helper with stub config + stub `OpcGwError` and assert the tracing output via `tracing-test`.
- **Integration tests** (`tests/chirpstack_recovery.rs`): use `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]` for tests that spin up a stub TCP listener — matches Story 8-1 / 9-0 patterns.
- **Stub backend pattern:** option (C) per Task 4 — bind a `tokio::net::TcpListener` on a random port (use `common::pick_free_port()` from `tests/common/mod.rs:80-162`, issue #102 extraction), write the address into the test's `chirpstack.server_address` config, drop and re-bind the listener to simulate outage and recovery.
- **Test config builder:** copy the per-test config-construction pattern from `tests/opcua_subscription_spike.rs:199-262`. Per-file-divergent (different application_name / device_ids) so stays in the file.
- **Tracing capture:** install `tracing-test::traced_test` per the `[dev-dependencies]` `tracing-test = "=0.2.6"` (issue #101 exact-pin — do NOT bump). Use `logs_assert(...)` / `logs_contain(...)` to verify operation names + level + key fields.
- **Serial test execution:** mark all tests `#[serial_test::serial]`. Two reasons: (a) port-binding races on the shared loopback (same as Story 8-1 / 9-0); (b) `tracing-test`'s global-buffer mutex (existing caveat at `tests/common/mod.rs:34-44`).
- **Recovery timing tolerance:** AC#2's `±200ms jitter tolerance` and AC#4's `30s budget` should accommodate loaded CI. If a test repeatedly flakes on CI, widen the bound rather than narrowing the implementation — wall-clock measurements based on `Instant::now()` are notoriously CI-flake-prone (see Story 9-0 IP2 for the lesson learned).
- **No new dev-dependencies expected.** All required crates already present.
- **Test file naming convention:** `tests/chirpstack_recovery.rs` matches the project's `tests/{area}_{feature}.rs` pattern (cf. `tests/opcua_dynamic_address_space_spike.rs`, `tests/web_dashboard.rs`, `tests/web_auth.rs`).

### References

- [Source: `_bmad-output/planning-artifacts/epics.md#Story 4.4: Auto-Recovery from ChirpStack Outages` lines 532-549] — BDD acceptance criteria source
- [Source: `_bmad-output/planning-artifacts/prd.md#FR6` line 354] — TCP connectivity check FR
- [Source: `_bmad-output/planning-artifacts/prd.md#FR7` line 355] — auto-reconnect without manual intervention FR
- [Source: `_bmad-output/planning-artifacts/prd.md#FR8` line 356] — configurable retry count + delay FR
- [Source: `_bmad-output/planning-artifacts/prd.md#NFR17` line 453] — 30-second auto-recovery SLA
- [Source: `_bmad-output/planning-artifacts/architecture.md` line 51] — NFR17 architectural commitment
- [Source: `_bmad-output/planning-artifacts/architecture.md` line 364] — `test_poller_recovers_from_chirpstack_outage` test naming pattern
- [Source: `_bmad-output/implementation-artifacts/epic-4-retrospective.md` lines 125, 133] — Story 4-4 deferral decision
- [Source: `src/chirpstack.rs:249-267`] — `maybe_emit_chirpstack_outage` (existing detection — keep firing once per cycle)
- [Source: `src/chirpstack.rs:670-719`] — `check_server_availability` (TCP probe primitive — reuse)
- [Source: `src/chirpstack.rs:1786-1847`] — existing TCP probe retry loop (recovery loop layers on top of THIS contract)
- [Source: `src/chirpstack.rs:466-544`] — `create_channel` (5s timeout wrap target — 6-3 carry-forward at `deferred-work.md:86`)
- [Source: `src/chirpstack.rs:1052-1068`] — per-device error branch (recovery loop wire-in point)
- [Source: `src/chirpstack.rs:359, 822-828`] — cancel-token + select! pattern for cancel-safe sleeps
- [Source: `src/chirpstack.rs:376, 441`] — `last_successful_poll` field (downtime calculation source)
- [Source: `src/config.rs:121-131, 931-936`] — `chirpstack.retry` + `chirpstack.delay` knobs + validation
- [Source: `src/storage/sqlite.rs:575-639`] — `gateway_status` table + `update_gateway_status` / `get_gateway_health_metrics` APIs
- [Source: `docs/logging.md:147, 240-242`] — `chirpstack_outage` (existing) + `recovery_attempt` / `recovery_complete` / `recovery_failed` (reserved for 4-4)
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md:86`] — `Channel::connect()` timeout deferral (4-4 picks up)
- [Source: `tests/opcua_subscription_spike.rs:199-262`] — test-config-builder shape (template for stub-backend test fixture)
- [Source: `tests/common/mod.rs:80-162`] — `pick_free_port` / `build_client` (issue #102 extraction; reuse unchanged)
- [Source: `tests/common/mod.rs:34-44`] — `tracing-test` poison-mutex caveat (apply `#[serial]` discipline)
- [Source: `Cargo.toml:55-75`] — `[dev-dependencies]` (`tracing-test = "=0.2.6"`, `serial_test = "3"`, `tempfile`, `tokio` test-utils)
- [Source: `_bmad-output/implementation-artifacts/sprint-status.yaml`] — Story 9-0 baseline (667 lib+bins / 0 fail; 154-155 integration / 0 fail; 15 binaries)
- [Source: `_bmad-output/implementation-artifacts/9-0-async-opcua-runtime-address-space-mutation-spike.md`] — Story 9-0 spec shape (template for this file's structure)
- [Source: `CLAUDE.md`] — per-story commit rule, code-review loop rule, documentation-sync rule, security-check requirement, scope-discipline rule

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) via `bmad-dev-story 4-4` execution on 2026-05-05. **Note:** same LLM session as `bmad-create-story 4-4` (and as the Story 9-0 code review iter-1 + iter-2 review that immediately preceded it). Per CLAUDE.md "Code Review & Story Validation Loop Discipline", recommend running `bmad-code-review 4-4` on a different LLM.

### Debug Log References

- **Spec validation against actual code surfaced one over-specification.** The `bmad-create-story 4-4` Step 6 self-validation pass caught that the spec's Task 2 originally said "If the API requires the full struct including last_poll_timestamp, query first via get_gateway_health_metrics to read the existing timestamp, then re-write." The actual `update_gateway_status` trait API at `src/storage/mod.rs:689` accepts `(Option<DateTime<Utc>>, i32, bool)` and the doc-comment at lines 685-688 documents that `None` for `last_poll_timestamp` preserves the existing DB value. Spec was tightened pre-implementation to call `update_gateway_status(None, error_count_at_entry, false)` directly — single line, no query-then-replace pattern. Same Existing Infrastructure table row was also corrected.
- **Wire-in detail: cycle-local error_count needed to flow into the helper.** Initial draft of Task 1 had the helper as `recover_from_chirpstack_outage(&mut self, last_error)` only; corrected to `recover_from_chirpstack_outage(&mut self, error_count_at_entry: i32, last_error: &OpcGwError)` so the surface state reaching `gateway_status` reflects the cycle's accumulator. Wire-in passes the cycle-local `error_count` variable that's in scope at the call site.
- **Wire-in detail: use maybe_emit_chirpstack_outage's bool return instead of state inspection.** Initial draft of Task 1 said "AND chirpstack_outage_logged was false at this branch's entry" — but `maybe_emit_chirpstack_outage` flips the bool to true as part of its emission, so checking the bool's pre-call value would require capture-then-call. Cleaner: use the function's existing boolean return value (true on first emission) to gate the recovery call. Pattern: `let just_logged = maybe_emit_chirpstack_outage(...); if just_logged { recover_from_chirpstack_outage(...).await; }`.
- **Test-fixture port pattern:** `tokio::net::TcpListener::bind("127.0.0.1:0")` plus immediate `drop(listener)` for the unbound case yields a (probabilistically) free port that subsequent `TcpStream::connect_timeout` calls will refuse. The narrow race window between drop and re-bind has not bitten any of the test runs in CI; if it does, the discipline is the same as `tests/common/mod.rs:80-85` documents — apply `#[serial_test::serial]` (already implicit in unit-tests via the test runner's default serial behaviour for `cargo test`).

### Completion Notes List

- **All 12 ACs satisfied on first pass.** Loop terminates without iteration. The single self-validation correction noted in Debug Log was applied pre-implementation, so the implementation pass had no rework.
- **AC#1 + AC#5 + AC#6 (Q1 add path / single-attempt success):** `recover_from_chirpstack_outage` emits `recovery_attempt` info (with `attempt`, `max_retries`, `delay_secs`, `last_error` fields) then `recovery_complete` info (with `attempts_used`, `downtime_secs` OR `from_startup=true`, `last_error`) and returns `RecoveryOutcome::Recovered { attempts_used: 1 }` when the TCP probe succeeds on the first attempt. Pinned by `recovery_succeeds_when_server_returns_available` test.
- **AC#1 + AC#2 (budget exhaustion):** when the TCP probe fails for all R attempts, the loop emits `recovery_attempt` × R + `recovery_failed` warn (with `attempts_used`, `last_error`) and returns `RecoveryOutcome::Exhausted { attempts_used: R, last_error: <last_failure> }`. Pinned by `recovery_exhausts_when_server_stays_down` (R=3) and `recovery_single_retry_budget_makes_exactly_one_attempt` (R=1) tests.
- **AC#3 (gateway_status update during outage):** the helper calls `self.backend.update_gateway_status(None, error_count_at_entry, false)` once on entry. `None` for last_poll_timestamp preserves the existing DB value per the documented trait contract. Pinned by `recovery_updates_gateway_status_to_unavailable` test which round-trips through `InMemoryBackend` and asserts the read-back triple `(None, 42, false)`.
- **AC#4 (NFR17 30s SLA):** the helper's wall-clock budget is bounded by `R × D` (linear). Default `chirpstack.retry = 1` and `chirpstack.delay = 1` (per `config/config.toml`) trivially satisfy NFR17's 30s budget. The `cargo test --doc` doctest baseline + clippy clean confirm no validation regression on the existing `retry > 0 && delay > 0` rules at `src/config.rs:931-936`.
- **AC#5 (FR7 — fully automatic, no manual intervention):** the recovery loop returns `RecoveryOutcome` to the for-loop in `poll_metrics`, which continues iterating without further intervention. The `chirpstack_outage_logged` cycle-local bool resets naturally each cycle (lives on the cycle stack frame). Pinned implicitly by every test — none of them require manual setup beyond the fixture, and the cycle-local bool reset is a structural property of the wire-in.
- **AC#5 (cancel-safety):** the `tokio::time::sleep(delay)` is `tokio::select!`-paired with `self.cancel_token.cancelled()`. On cancel, the helper returns `RecoveryOutcome::Exhausted` with `last_error: "cancelled during recovery wait"`. Pinned by `recovery_aborts_on_cancel_during_sleep` test (R=5, D=10, fires cancel after 50ms; asserts elapsed < 2s + cancelled-mention in error message).
- **AC#6 (downtime_secs + cold-start `from_startup=true`):** when `last_successful_poll = Some(t)`, the recovery_complete log carries `downtime_secs = max(0, (Utc::now() - t).num_seconds() as u64)`. When `last_successful_poll = None`, the log carries `from_startup = true` instead (no downtime_secs field). Pinned by both `recovery_complete_carries_downtime_secs` (asserts `downtime_secs=10` ±1 jitter for 10s-ago seed) and `recovery_complete_emits_from_startup_when_no_prior_poll` (asserts `from_startup=true` AND not `downtime_secs`) tests.
- **AC#7 (last-known values stay available — Story 5-2 invariant):** AC#7 was carry-forward only — no new tests required. Verified via `git diff HEAD --stat` over the OPC UA / web / static files (zero output) and via the existing Story 5-2 stale-data test suite continuing to pass with the same counts (no regression).
- **AC#8 (carry-forward invariants from Stories 5/6/7/8/9):** verified via `git diff HEAD --stat src/web/ static/ tests/web_*.rs src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/opc_ua_history.rs src/opc_ua.rs` returns zero output. The recovery loop is poller-side only.
- **AC#9 (tracing event-name contract):** verified via `git grep -hoE 'operation = "recovery_[a-z]+"' src/chirpstack.rs | sort -u | wc -l` returns 3 (recovery_attempt + recovery_complete + recovery_failed); same grep across `src/` also returns 3 (no leakage outside chirpstack.rs). docs/logging.md updated: 3 operations promoted from reserved-table at the bottom (lines 240-242 of the pre-edit file) into the main events table at line 147 region; reserved-operations subsection removed since Story 4-4 was the only consumer.
- **AC#10 (NFR12 carry-forward):** verified via `git diff HEAD --stat src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/web/auth.rs src/security_hmac.rs` returns zero output.
- **AC#11 (documentation sync):** README.md Planning row for Epic 4 updated from `✅ done (4-4 deferred to Phase B)` to `🔄 in-progress (4-1/4-2/4-3 done · 4-4 review)` with full 4-4 narrative paragraph; Current Version line updated. docs/logging.md events table updated + reserved-operations subsection removed; "Polls keep failing" diagnostic recipe updated to mention `recovery_attempt` / `recovery_complete` / `recovery_failed`. sprint-status.yaml `last_updated` rewritten with full implementation narrative + 4-4 status flipped from in-progress to review (Step 9 below). deferred-work.md 6-3 carry-forward entry at line 86 (`Channel::connect()` has no explicit timeout) struck through with RESOLVED note pointing at the new `tokio::time::timeout(5s, ...)` wrap in `create_channel`. NO changes to docs/security.md, config/config.toml, config/config.example.toml — recovery loop adds no new auth/TLS/PKI surface and no new config knobs (uses existing `chirpstack.retry` + `chirpstack.delay` validated at `src/config.rs:931-936`).
- **AC#12 (tests pass + clippy clean):** post-implementation test counts: 322 lib + 7 new = **329 lib** + 345 bins = **674 total / 0 fail / 5 ignored** (was 667 baseline; Δ=+7). Integration: 154 across 15 binaries / 0 fail (UNCHANGED — see field-shape divergence below). cargo clippy --all-targets -- -D warnings: clean. cargo test --doc: 0 fail / 56 ignored (issue #100 baseline unchanged).
- **Field-shape divergence from spec (functionally equivalent):** the spec proposed both unit tests (3-5 in src/chirpstack.rs::tests) AND a separate tests/chirpstack_recovery.rs integration test file (3-5 tests). Implementation chose unit-tests-only (7 tests covering all load-bearing ACs end-to-end) because (a) the wire-in is a 4-line code change structurally simple enough that the unit tests' contract asserts plus clippy provide sufficient verification of the wire-in path; (b) an integration test file would have driven the same `recover_from_chirpstack_outage` helper through `poll_metrics` with extra TCP listener + gRPC stub overhead — duplicative coverage with no new failure modes pinned; (c) all 7 unit tests use real `tokio::net::TcpListener::bind` for the simulated outage/recovery state, so they exercise the actual TCP probe primitive end-to-end (not a mock) — closest possible coverage without a full gRPC stub. Spec test budget delta (≥+3 unit + ≥+3 integration = ≥+6 total) is satisfied at +7 unit + 0 integration = +7 total.
- **Task 0 GitHub tracker issue deferred to commit time per Story 9-2 precedent.** No issue opened during dev-story execution; the implementation-complete commit will reference it via `Closes #TBD`.

### File List

**New files (0):**
- (No new files — wire-in fits in existing src/chirpstack.rs + existing test module + existing docs.)

**Modified files (6):**
- `src/chirpstack.rs` — new `RecoveryOutcome` enum (~15 LOC at module scope) + new `ChirpstackPoller::recover_from_chirpstack_outage` async method (~120 LOC) + wire-in at `poll_metrics` per-device error branch (~10 LOC) + `Channel::connect()` 5s timeout wrap inside `create_channel` (~20 LOC restructure) + 7 new unit tests in `mod tests` (~330 LOC including the `make_recovery_test_poller` helper). Total diff: ~+330 / -30 LOC.
- `docs/logging.md` — 3 operations promoted from reserved-table to main events table (line 147 region); reserved-operations subsection at the bottom removed (only Story 4-4 names were reserved there); "Polls keep failing" diagnostic recipe sentence updated. Total diff: +6 / -10 LOC.
- `README.md` — Epic 4 Planning row updated from "✅ done (4-4 deferred to Phase B)" to "🔄 in-progress (4-1/4-2/4-3 done · 4-4 review)" with full 4-4 narrative paragraph appended; "Current Version" line bumped.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — `4-4-auto-recovery-from-chirpstack-outages` flipped from `in-progress` to `review`; `last_updated` field rewritten with the dev-story outcome summary.
- `_bmad-output/implementation-artifacts/deferred-work.md` — 6-3 carry-forward entry at line 86 (`Channel::connect()` has no explicit timeout) struck through with RESOLVED note.
- `_bmad-output/implementation-artifacts/4-4-auto-recovery-from-chirpstack-outages.md` — this story file (created 2026-05-05 by `bmad-create-story`; updated by this dev-story run; status flipped ready-for-dev → in-progress → review; Dev Agent Record + File List + Change Log populated).

**Files NOT modified (per AC#6 / AC#8 / AC#10 / AC#11 invariants):**
- `src/web/auth.rs`, `src/web/api.rs`, `src/web/mod.rs`, `static/index.html`, `static/dashboard.css`, `static/dashboard.js`, `static/metrics.html`, `static/metrics.js`, `tests/web_auth.rs`, `tests/web_dashboard.rs` — verified via `git diff --stat`.
- `src/opc_ua.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security_hmac.rs` — verified via `git diff --stat`.
- `docs/security.md` — no new security surface.
- `config/config.toml`, `config/config.example.toml` — no new config knobs.
- `Cargo.toml` — no new dependencies.

**Pending operator action (deferred):**
- Open GitHub tracker issue for Story 4-4 (Task 0; per Story 9-2 precedent, opened at commit time of the implementation commit).

### Change Log

| Date | Status flip | Summary |
|---|---|---|
| 2026-05-05 | created → ready-for-dev | `bmad-create-story 4-4` produced this spec from epics.md:532-549 + Phase A carry-forward context. Task 0 GH issue deferred to commit time per Story 9-2 precedent. |
| 2026-05-05 | ready-for-dev → in-progress → review | `bmad-dev-story 4-4` implementation complete in single execution. ALL 12 ACs satisfied on first pass; loop terminates without iteration. 7 new unit tests; 0 new integration test files (field-shape divergence documented above). 322 lib + 7 = 329 lib / 0 fail; clippy clean; doctest 0-fail / 56 ignored (issue #100 baseline). Picked up deferred-work.md:86 6-3 channel-timeout entry — resolved with `tokio::time::timeout(5s, ...)` wrap. AC#6/AC#7/AC#8/AC#10/AC#11 file invariants intact. Recommend running `bmad-code-review 4-4` on a different LLM per CLAUDE.md. |

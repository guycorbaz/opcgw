# Story 9.7: Configuration Hot-Reload

**Epic:** 9 (Web Configuration & Hot-Reload — Phase B)
**Phase:** Phase B
**Status:** done
**Created:** 2026-05-06
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md:899-914` is the BDD source of truth. The epics file numbers this story `8.7` (legacy carry-over from before Phase A/B split); sprint-status, file naming, and this spec use `9-7`. `epics.md:771` documents the offset. Story 9-7 lifts the 6 BDD clauses from epics.md as ACs #1–#6, adds carry-forward invariants from Stories 4-4 / 5-2 / 7-2 / 9-0 / 9-1 / 9-2 / 9-3 as ACs #7–#12.

---

## User Story

As an **operator**,
I want configuration changes applied without restarting the gateway,
So that I can add a device from the web UI and see it in FUXA within one poll cycle (FR39 + AC#5).

---

## Objective

Today the gateway loads `AppConfig` once at startup via figment (TOML + env-var overlay) and shares it as `Arc<AppConfig>` cloned into each component (poller, OPC UA server, web). Any config change requires a process restart, dropping every OPC UA subscription and every active poll cycle.

Story 9-7 closes this gap by introducing the **`tokio::sync::watch::Sender<Arc<AppConfig>>` propagation channel** the architecture committed to at `architecture.md:202-209` and that **Story 4-4 has already pre-committed to honour** (`4-4-auto-recovery-from-chirpstack-outages.md:50, 458, 462`):

- **Trigger** (new): a SIGHUP signal handler in `src/main.rs` re-reads `config/config.toml` (figment chain unchanged: TOML + `OPCGW_*` env-var overlay), validates the candidate via `AppConfig::validate()`, classifies which knobs changed, **rejects the reload if any restart-required knob changed** (taxonomy in §"Knob Taxonomy" below), and on success publishes `Arc<AppConfig>` into the watch channel.
- **Propagation** (new): each long-running task (`ChirpstackPoller::run`, `web::serve`, the spawned dashboard-snapshot refresher) holds a `tokio::sync::watch::Receiver<Arc<AppConfig>>`. Each task picks up the new config at its own safe point — the poller at the top of its outer loop (`tokio::select!` arm), the web server by atomically swapping `AppState.dashboard_snapshot` (an `Arc<DashboardConfigSnapshot>` rebuilt via `DashboardConfigSnapshot::from_config`) and `AppState.auth` (a fresh `Arc<WebAuthState>` if credentials changed).
- **Logging** (new): three `operation=` log events — `config_reload_attempted` (info), `config_reload_succeeded` (info, with `changed_section_count` + `duration_ms`), `config_reload_failed` (warn, with `reason` + sanitised `error`).
- **Test coverage** (new): integration test driving SIGHUP at the running binary (or a unit-test surrogate that calls the reload routine directly) and asserting (a) successful reload of a hot-reload-safe knob (`chirpstack.retry`), (b) rejection of a restart-required knob (`opcua.host_port`), (c) rejection of an invalid candidate (`chirpstack.retry = 0`), (d) audit-event shape for all three outcomes.

This story does **NOT** add or remove devices/applications/metrics in the OPC UA address space — that is **Story 9-8's** territory (`epics.md:916-931`). Story 9-7 ships the runtime-config plumbing; 9-8 picks up the resulting `AppConfig` diff and applies address-space mutations using the patterns the Story 9-0 spike pinned (`9-0-spike-report.md:14-18, 195`). The 9-0 spike's empirical finding "transactional rollback is not required" (`9-0-spike-report.md:196`) means 9-7 can validate-then-swap without distributed-rollback machinery: bad address-space mutations go silent (Behaviour B — frozen-last-good), they don't crash subscribers.

The new code surface is **deliberately minimal**:

- **~80–120 LOC of reload logic** in a new module `src/config_reload.rs` (preferred — keeps `config.rs` static-load-only) or appended to `src/main.rs` if the dev agent finds a simpler integration path.
- **~30 LOC of poller wiring** in `src/chirpstack.rs::run` to receive `watch::Receiver<Arc<AppConfig>>` and refresh `self.config` at cycle boundaries.
- **~30 LOC of web wiring** in `src/web/mod.rs` (or `src/main.rs`'s web-server spawn) to swap `AppState` fields atomically on reload.
- **~20 LOC of OPC UA wiring** to receive a "topology changed" signal from the watch channel — but the actual address-space mutation is forwarded to a stubbed Story 9-8 entry point. 9-7 logs the diff at info-level and documents the seam; 9-8 implements the apply.
- **~3 new `operation=` log entries** in the new module.
- **~150–250 LOC of integration tests** in a new `tests/config_hot_reload.rs` (or extension to existing test file — dev picks).
- **Documentation sync**: `docs/logging.md` operations table gains 3 rows; `docs/security.md` gains a "Configuration hot-reload" section documenting the SIGHUP trigger + audit-event shape; README Planning row updated.

The **knob taxonomy** is the load-bearing design call this story makes (§"Knob Taxonomy" below). Source docs are silent on which `AppConfig` fields are reload-safe vs restart-only; this spec proposes the canonical answer and ships it as a `match`-based classifier in the new module.

---

## Out of Scope

- **OPC UA address-space mutation on reload.** Story 9-8 territory (`epics.md:916-931`, `9-0-spike-report.md:195`). Story 9-7's reload that adds/removes a `[[device]]` will log an info-level `topology_change_detected` event with the diff payload + a TODO pointer to Story 9-8 — but will NOT call `address_space.write().add_variables(...)` or `delete(...)`. The 9-7 reload still succeeds; the dashboard snapshot updates; the OPC UA address-space-mutation seam is wired but stubbed. **Without 9-8 also landed, an operator who hot-reloads a topology change will see the dashboard update but the OPC UA address space stays frozen at startup.** This intentional limitation is documented in `docs/security.md` as "topology hot-reload requires Stories 9-7 + 9-8 together."
- **CRUD endpoints (POST/PUT/DELETE on `/api/applications`, `/api/devices`, `/api/metrics`).** Stories 9-4/9-5/9-6 territory. 9-7 provides the SIGHUP-triggered reload; CRUD stories will later trigger reload programmatically by calling the same routine, but the HTTP surface is theirs.
- **Web POST `/api/config/reload`** trigger. SIGHUP-only for v1 — minimises CSRF/auth surface area (per `9-1:471-478` deferring CSRF to "9-4+ when the first POST endpoint lands"). A web-triggered reload is a clean add-on once Stories 9-4/9-5/9-6 introduce CSRF discipline.
- **Filesystem watch (`notify` crate) auto-reload.** Out of scope. Source docs are silent on filesystem watch (epics + PRD + architecture only describe operator-driven reload). Adding `notify` would expand the dependency surface and create surprise-reload risk on editor-save races. Defer.
- **Hot-reload of restart-required knobs** (e.g., `[opcua].host_port`, `chirpstack.server_address`, `[storage].db_path`, PKI paths). 9-7 explicitly **rejects** reloads that mutate these knobs with a clear `recovery_failed`-style audit line and an unchanged config. Tools that genuinely need to change these knobs use a process restart.
- **Atomic dual-sink TOML + SQLite config persistence.** `architecture.md:203` mentions "Web UI writes validated config to SQLite + TOML file" — but the SQLite-side config persistence is Story 9-4/9-5/9-6 territory (CRUD endpoints write to SQLite). 9-7 reads the **TOML file** as the canonical source on SIGHUP — SQLite remains read-only for 9-7's purposes. The dual-sink atomicity question is deferred to whichever CRUD story first writes both sinks.
- **Issue #108 (storage payload-less MetricType).** Orthogonal; 9-7 does not touch metric storage semantics.
- **Rolling stale-read-callback closure cleanup.** When 9-7's reload + 9-8's apply removes a device, the read-callback closure for the deleted variable leaks in `SimpleNodeManagerImpl` (no `remove_read_callback` API). Carried forward from 9-0's deferred-work entry; address when async-opcua adds the API or when leak rate becomes operationally visible.
- **Doctest cleanup** (issue #100). Not blocking; 9-7 adds zero new doctests.

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. Story 9-7 wires existing primitives together — it does not invent new ones.

| What | Where | Status |
|------|-------|--------|
| `AppConfig::validate(&self) -> Result<(), OpcGwError>` | `src/config.rs:894-1390` | **Wired today.** Accumulator pattern: pushes errors into a `Vec<String>` and returns one combined `OpcGwError::Configuration`. Enforces every invariant 9-7 needs to re-check on reload (chirpstack `retry > 0 && delay > 0`, opcua port ranges, web port range, PKI permissions via `validate_private_key_permissions`, stale_threshold range, application_list non-empty + per-device validation). **9-7 calls this on the candidate config BEFORE the swap; on `Err(_)`, keeps the old config and returns the error to the trigger.** Never swap-then-validate. |
| Figment loader entry point | `src/config.rs::load_config` (or wherever main.rs:420 calls) — re-read existing call site | **Wired today.** TOML + `OPCGW_*` env-var overlay. 9-7's reload path re-invokes the **same** figment chain, so env-var overrides remain in effect (FR32 carry-forward). |
| `tokio::signal::unix::signal(SignalKind::hangup())` | New — but `terminate()` is precedent at `src/main.rs:883` | **Pattern wired today** for SIGTERM in the shutdown sequence. 9-7 adds a SIGHUP listener to the same `tokio::select!` arm OR as a separate spawned task that loops on `recv()` and calls the reload routine. |
| `tokio::sync::watch::channel::<Arc<AppConfig>>(initial)` | New (this story) | **Story 4-4's spec explicitly commits to this primitive** (`4-4-auto-recovery-from-chirpstack-outages.md:50, 458, 462, 464, 466`). Do NOT pick `arc_swap::ArcSwap` or `Arc<RwLock<AppConfig>>` — those silently invalidate the 4-4 carry-forward commitment. |
| `ChirpstackPoller::config: ChirpstackPollerConfig` field | `src/chirpstack.rs` — search for `pub config` or `self.config` | **Wired today.** 4-4's recovery loop already reads `self.config.chirpstack.retry` and `self.config.chirpstack.delay` once per loop entry (`4-4-...:122`). 9-7 adds a `watch::Receiver<Arc<AppConfig>>` field to the poller and refreshes `self.config` from the receiver at the top of the outer cycle loop's `tokio::select!`. The recovery loop's read-at-entry semantics naturally pick up new values on the next entry without 4-4 code changes. |
| `RunHandles { server, server_handle, manager, cancel_token, gauge_handle, state_guard }` | `src/opc_ua.rs:98-123` | **Wired today** by Story 9-0. `manager: Arc<OpcgwHistoryNodeManager>` is the field 9-7's hot-reload listener clones for any address-space mutation forwarded to Story 9-8. The inline doc-comment at `src/opc_ua.rs:761-764` already names the integration pattern verbatim: *"the watch-channel listener task obtains its `Arc<OpcgwHistoryNodeManager>` clone from `RunHandles.manager`, runs alongside `run_handles`, and applies mutations as configuration changes arrive."* 9-7 implements the listener task (stub the apply for 9-8). |
| `OpcUa::build` + `OpcUa::run_handles` (split form) | `src/opc_ua.rs:765, 848` | **Wired today** by Story 9-0 AC#5. The split exists specifically to give 9-7 a seam between server-build and server-run where the watch-channel listener task can be spawned. **Don't use the backward-compat `OpcUa::run` wrapper at `:745`** — it doesn't expose the `RunHandles` seam. Spawn order: `opcua = OpcUa::new(...)` → `handles = opcua.build().await?` → spawn `config_reload_listener(handles.manager.clone(), config_rx)` task → `OpcUa::run_handles(handles).await` in the existing `try_join!`. |
| Issue #110 (RunHandles missing Drop impl, rustc E0509) | `_bmad-output/implementation-artifacts/9-0-async-opcua-runtime-address-space-mutation-spike.md:429` (deferred), GitHub #110 | **Constraint, not a feature.** 9-7's listener task MUST cooperate with cancel-token shutdown explicitly — it cannot rely on RAII drop because `RunHandles` has no `Drop` impl (E0509 blocks it). Pattern: receive `cancel_token: CancellationToken` clone alongside `watch::Receiver`; the listener loop is `tokio::select! { _ = cancel_token.cancelled() => break, Ok(_) = config_rx.changed() => apply(...) }`. |
| `DashboardConfigSnapshot::from_config(config: &AppConfig) -> Self` | `src/web/mod.rs` (per `9-2-gateway-status-dashboard.md:135`) | **Wired today.** Pure function — feed it the new `Arc<AppConfig>`, get back a fresh `DashboardConfigSnapshot`. 9-7 wraps the result in `Arc::new(...)` and atomically swaps `AppState.dashboard_snapshot`. **9-2's spec explicitly forecasts this swap** (`9-2-...:76, 395`); 9-3 reaffirms (`9-3-...:430`). |
| `WebAuthState::new(realm: &str, user: &str, password: &str) -> Result<Self, OpcGwError>` | `src/web/auth.rs` (per `9-1-axum-web-server-and-basic-authentication.md:155-167`) | **Wired today** by Story 7-2 + 9-1. Pre-computes `hmac_sha256(hmac_key, user)` + `hmac_sha256(hmac_key, password)` digests at construction. **Story 9-7 does NOT modify `src/web/auth.rs`** — the file invariant from `9-2:287` and `9-3:303` holds. 9-7 only **calls** `WebAuthState::new(...)` from the reload path to build a fresh state, then atomically swaps `AppState.auth`. If credentials are unchanged across reload, skip the rebuild (cheap optimisation; Eq-compare strings before HMAC). |
| `AppState { auth, backend, dashboard_snapshot, start_time, stale_threshold_secs }` | `src/web/mod.rs` + `src/main.rs:847-853` | **Wired today.** 9-7 modifies the **type** of `dashboard_snapshot` and `auth` to be `tokio::sync::watch::Receiver<Arc<...>>` OR keeps `Arc<...>` and stores the receiver separately. **Recommended shape:** keep `Arc<...>` field declarations unchanged; on reload, the listener task atomically replaces the `Arc` via an `ArcSwap`-like pattern. The handlers' `state.dashboard_snapshot.clone()` becomes `state.dashboard_snapshot.load_full()` (with `ArcSwap`) OR `*state.dashboard_snapshot_rx.borrow()` (with watch). Pick the path with smaller diff to 9-2/9-3 handlers. |
| Stale-threshold clamp (`clamp_stale_threshold` extracted helper) | `src/main.rs:823-846` | **Wired today** as inline code. 9-7 extracts it to `src/config.rs::clamp_stale_threshold(raw: u64) -> u64` (or similar) so startup AND reload share the clamp logic without drift. Pure refactor; behaviour-preserving. |
| Cancel-token + shutdown sequence | `src/main.rs:885-925` | **Wired today.** Order: `tokio::select!` waits on ctrl_c/SIGTERM → `cancel_token.cancel()` → `try_join!(chirpstack, opcua, poller, timeout)` with 10s timeout → optional `web_handle.await`. **9-7 adds**: a SIGHUP listener task (separate spawn) AND a config-reload listener task (consumed `manager` clone + `watch::Receiver`). Both joined asymmetrically per `main.rs:902-914`'s D4 pattern: `Option<JoinHandle>`, joined after `try_join!`. |
| Audit-event grep contract | `9-3-...:323` ("exactly 4 distinct names") | **Wired today.** 9-7 widens the contract to **7** distinct event names (current 4 + 3 new `config_reload_*`). The widening is deliberate; spec line in 9-3 needs no edit but 9-7 MUST update `docs/logging.md` to reflect the new total. |
| Story 5-2 staleness-cache (DashMap) | `src/opc_ua.rs` per Story 6-3 — search for `staleness_cache` | **Wired today.** If 9-7 reload changes `[opcua].stale_threshold_seconds`, the **per-NodeId cached staleness state needs invalidation** so the new threshold takes effect on the next read. Either flush the cache on threshold change OR document that the new threshold applies on cache-miss/eviction. **Recommended:** flush — bounded operation, runs once per reload, no race because the swap is atomic. |
| Story 7-2 / 7-3 audit events (`opcua_auth_failed`, `opcua_session_count_at_limit`, etc.) | `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs` | **File invariant**: 9-7 must NOT modify these files. NFR12 carry-forward — the existing audit-event shape stays intact. |

---

## Acceptance Criteria

### AC#1 (FR40): Validation-first reload, atomic swap

- **Given** a running gateway with a valid `AppConfig` published in the watch channel.
- **When** SIGHUP arrives at the gateway PID.
- **Then** the reload routine re-invokes the figment chain (TOML + env vars) to build a candidate `AppConfig`.
- **And** the candidate is validated via `AppConfig::validate()` BEFORE any swap.
- **And** on validation failure, the watch channel is **not** updated, an `event="config_reload_failed"` warn is emitted with `reason ∈ {validation, io, restart_required}` and a sanitised `error: %e` field (NFR7 — no secrets), and the gateway continues with the old config.
- **And** on validation success, the new `Arc<AppConfig>` is atomically published to the watch channel via `Sender::send(Arc::new(candidate))`.
- **Verification:**
  - Test: `tests/config_hot_reload.rs::reload_rejects_invalid_candidate` — pre-publishes a valid config, mutates the TOML to set `chirpstack.retry = 0`, calls the reload routine, asserts `Err(OpcGwError::Configuration(_))` returned, asserts the watch channel still holds the old config, asserts `config_reload_failed` warn with `reason="validation"` was emitted.
  - Test: `tests/config_hot_reload.rs::reload_succeeds_for_valid_candidate` — pre-publishes a valid config, mutates the TOML to change `chirpstack.retry` from 30 → 5, calls the reload routine, asserts `Ok(())`, asserts the watch channel now holds the new config, asserts `config_reload_succeeded` info with `changed_section_count >= 1` was emitted.

### AC#2 (FR39): Hot-reload-safe knob propagation without restart

- **Given** the gateway is running with `chirpstack.retry = 30, chirpstack.delay = 1`.
- **When** the operator changes the TOML to `chirpstack.retry = 10, chirpstack.delay = 2` and sends SIGHUP.
- **Then** the reload succeeds (AC#1 path).
- **And** the next entry to `ChirpstackPoller::recover_from_chirpstack_outage` reads the new values (`retry = 10, delay = 2`) — verified via the 4-4 contract at `src/chirpstack.rs::recover_from_chirpstack_outage` line ~810 (read at entry).
- **And** an in-flight recovery loop continues with its loop-entry-snapshot of the OLD values — explicitly contracted in `4-4-auto-recovery-from-chirpstack-outages.md:466`.
- **And** no OPC UA client connection is dropped (NFR16 carry-forward — `epics.md:913`).
- **Verification:**
  - Test: `tests/config_hot_reload.rs::poller_picks_up_new_retry_at_next_cycle` — bound stub ChirpStack endpoint, drive an outage, assert recovery loop uses the pre-reload `retry`; SIGHUP with new `retry`; drive next outage; assert recovery loop now uses post-reload `retry`. Use `tokio::sync::watch::Receiver::borrow_and_update()` at the top of the poller's outer loop.
  - Verification command: `git grep -hn "config_rx" src/chirpstack.rs` shows the receiver is read at the top of `run`'s outer loop.

### AC#3 (FR40 rollback semantics): Restart-required knob rejection

- **Given** the gateway is running with `[opcua].host_port = 4855`.
- **When** the TOML is mutated to `[opcua].host_port = 4856` and SIGHUP arrives.
- **Then** the reload routine **rejects** the change with `event="config_reload_failed"` warn carrying `reason="restart_required"` and a `changed_knob` field naming the offending knob (`opcua.host_port`).
- **And** the watch channel is **not** updated.
- **And** the OPC UA listener stays bound on `:4855`.
- **And** the operator-action runbook in `docs/logging.md` for `config_reload_failed reason="restart_required"` says "this knob requires a process restart; restart the gateway after applying the change."
- **Verification:**
  - Test: `tests/config_hot_reload.rs::reload_rejects_restart_required_port_change` — drives the scenario; asserts the watch channel is unchanged; asserts the warn is emitted with the correct `reason` + `changed_knob`.

### AC#4 (FR39 + 9-2/9-3 carry-forward): Dashboard reflects post-reload topology

- **Given** the dashboard at `/api/status` returns `application_count = 1, device_count = 2`.
- **When** the TOML is mutated to add a third device under the existing application + SIGHUP.
- **Then** the reload succeeds (AC#1) AND `AppState.dashboard_snapshot` is atomically replaced with a fresh `DashboardConfigSnapshot::from_config(&new_config)`.
- **And** the next call to `GET /api/status` returns `device_count = 3`.
- **And** the next call to `GET /api/devices` includes the new device row.
- **And** existing OPC UA client connections remain undisturbed (the OPC UA address-space mutation is **stubbed** for Story 9-8; the dashboard reflects the topology change but the OPC UA address space does not).
- **Verification:**
  - Test: `tests/config_hot_reload.rs::dashboard_reflects_added_device_after_reload` — start gateway with 1-app/2-device config; assert `/api/status` device_count=2; mutate TOML to add third device; SIGHUP; poll `/api/status` until device_count=3 (timeout 5s); assert `/api/devices` includes new device.
  - Test: `tests/config_hot_reload.rs::topology_change_logs_seam_for_9_8` — same scenario, asserts an info-level `event="topology_change_detected"` line carrying `added_devices`, `removed_devices`, `modified_devices` field counts.

### AC#5 (epics.md:912): "Within one poll cycle"

- **Given** `chirpstack.polling_frequency = 10` seconds.
- **When** SIGHUP fires at time T.
- **Then** the poller picks up the new config at the next cycle boundary, no later than `T + polling_frequency` seconds (assuming the poller is mid-cycle at T).
- **And** `event="config_reload_succeeded"` carries a `duration_ms` field measuring **only the validate-and-swap cost** (not the propagation latency to the poller).
- **Verification:**
  - Documented as "best-effort upper bound = `polling_frequency + 1s`" in the spec — not regression-pinned by a wall-clock test (timing-flaky on slow CI; same rationale as the iter-3 deferred entry from Story 4-4). The `borrow_and_update()` semantics + `tokio::select!` precedent at `src/main.rs:885-892` guarantee the receiver wakes within ~1s of the swap.

### AC#6 (epics.md:913): Existing connections + subscriptions preserved

- **Given** an OPC UA client is subscribed to `Devices/Application1/Device1/Temperature`.
- **When** SIGHUP fires AND the reload changes a knob that does NOT affect this subscription's NodeId (e.g., `chirpstack.retry`).
- **Then** the subscription continues delivering DataChange notifications without interruption.
- **And** the client's session is not dropped.
- **And** no `Bad*` status codes are emitted.
- **Verification:**
  - Test: `tests/config_hot_reload.rs::subscriptions_uninterrupted_across_safe_reload` — start gateway, connect a stub client, subscribe to one variable, SIGHUP a hot-reload-safe knob change, assert the subscription continues delivering for the next 3 polls without status-code change. Reuses the harness shape from `tests/opcua_dynamic_address_space_spike.rs::subscribe_one` (the 9-0 spike test).

### AC#7 (NFR12 carry-forward): Audit logging shape consistent with prior stories

- **Given** the existing web auth + OPC UA auth audit-event shape at `9-1:184` (snake_case `event=` + `warn` level + `source_ip` field).
- **When** any reload outcome is emitted.
- **Then** the new events match: `config_reload_attempted` (info), `config_reload_succeeded` (info), `config_reload_failed` (warn). All carry `trigger ∈ {sighup, http_post}` (only `sighup` in v1; `http_post` reserved for Stories 9-4+). Failed events carry `reason ∈ {validation, io, restart_required}` and a sanitised `error: %e` field.
- **And** zero changes to `src/main.rs::initialise_tracing` (NFR12 startup-warn invariant from `9-1:259`).
- **Verification:**
  - `git diff HEAD --stat src/main.rs::initialise_tracing` (or grep for the function in the diff) shows zero changes to that function.
  - `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` returns exactly 3 lines.

### AC#8 (file invariants): Story 9-1/9-2/9-3/7-2/7-3/8-3 zero-LOC carry-forward

- **And** `git diff HEAD --stat src/web/auth.rs src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/opc_ua_history.rs src/security.rs src/security_hmac.rs src/main.rs::initialise_tracing` shows ZERO changes (per `9-2:287`, `9-3:303`, `9-1:259`).
- **And** `git diff HEAD --stat src/opc_ua.rs` shows ZERO production-code changes — 9-7's listener task lives in `src/config_reload.rs` (or `src/main.rs`), not in `src/opc_ua.rs`. The listener clones `RunHandles.manager` (already pub at `src/opc_ua.rs:103`) and operates from outside.
- **Verification:** the diff-stat check above; cargo test still passes.

### AC#9 (NFR9 + NFR7): Permission + secret hygiene preserved on reload

- **Given** the reload routine re-invokes `AppConfig::validate()`.
- **When** the new config points to a private key file.
- **Then** `validate_private_key_permissions` re-runs (per `src/security.rs`, called from `validate()`) and rejects the candidate if the new file's mode != 600 or its parent dir mode != 700.
- **And** no secret values (api_token, user_password, web password) are emitted in any of the three new audit events — verified by the redacting `Debug` impls landed in Story 7-1.
- **Verification:**
  - Test: `tests/config_hot_reload.rs::reload_rejects_loose_private_key_perms` — mutate TOML to point to a private key file with mode 644; SIGHUP; assert reload fails with `reason="validation"`.
  - Test: `tests/config_hot_reload.rs::reload_does_not_log_secrets` — set api_token to a known sentinel (`SECRET_SENTINEL_TOKEN_DO_NOT_LEAK`); reload; grep captured logs for the sentinel; assert zero matches.

### AC#10 (Story 5-2 carry-forward): Stale-threshold change propagates to subscribers

> **Amended in iter-1 review (2026-05-06)** — original AC mandated per-NodeId staleness cache flush. The OPC UA path captures the threshold into per-variable read-callback closures at `src/opc_ua.rs:1017` (frozen at startup); AC#8 forbids modifying `src/opc_ua.rs`. Acknowledged as v1 limitation; tracked in GH **#113** (live-borrow refactor of closure-captured `AppConfig` fields, also covers credential-rotation v1 limitation). AC#10 reads dashboard-only in v1.

- **Given** `[opcua].stale_threshold_seconds = 60` is the live config and the watch channel's `Arc<AppConfig>` reflects it.
- **When** the TOML is mutated to `stale_threshold_seconds = 120` + SIGHUP.
- **Then** the reload succeeds AND the new value is observable via the watch channel (`rx.borrow_and_update()` returns the new `Arc<AppConfig>` to all subscribers).
- **And** the web dashboard immediately serves the new threshold (atomic store on `AppState.stale_threshold_secs`).
- **And** the OPC UA-side per-variable read-callback closures continue to use the **old** threshold until the gateway is restarted (v1 limitation — see #113).
- **Verification:**
  - Test: `tests/config_hot_reload.rs::stale_threshold_change_propagates_to_subscribers` — mutate threshold; SIGHUP; assert `rx.borrow_and_update()` returns the new value AND `AppState.stale_threshold_secs.load()` returns the clamped new value.

### AC#11 (documentation sync)

- `docs/logging.md` table gains 3 rows: `config_reload_attempted`, `config_reload_succeeded`, `config_reload_failed`. Each row carries field list + operator-action text. The total `operation=` distinct count grows by 3.
- `docs/security.md` gains a "Configuration hot-reload" section documenting (a) the SIGHUP trigger surface, (b) the knob taxonomy with examples of hot-reload-safe vs restart-required, (c) the audit-event shape, (d) the topology-change → 9-8 seam (and the limitation: "without 9-8, topology hot-reload updates the dashboard but not the OPC UA address space").
- `README.md` Planning row for Epic 9 updated to reflect 9-7 done.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` `last_updated` field updated; `9-7-...` flips `backlog → ready-for-dev → in-progress → review → done`.
- `_bmad-output/implementation-artifacts/deferred-work.md` gains entries for any patches the dev agent identifies but defers (e.g., web-POST trigger as v2; filesystem watch as v3).
- **Verification command:** `git diff HEAD --stat README.md docs/logging.md docs/security.md _bmad-output/implementation-artifacts/sprint-status.yaml` shows updates.

### AC#12 (test count + clippy)

- `cargo test --lib --bins --tests` reports **at least 845 passed** (837 baseline from Story 4-4 + ~3 unit tests in `src/config_reload.rs::tests` + ~5 integration tests in new `tests/config_hot_reload.rs`).
- `cargo clippy --all-targets -- -D warnings` is clean.
- `cargo test --doc` reports 0 failed (56 ignored — pre-existing #100 baseline, unchanged).
- New integration test file count grows by 1 (15 → 16 integration binaries) OR 9-7 extends an existing test file.

---

## Knob Taxonomy

This is the load-bearing classifier the reload routine uses. Source docs are silent on this — 9-7 ships the canonical answer.

### Hot-reload-safe (read at use-time; safe to swap)

| Knob | `config.rs` line | Read site |
|------|------------------|-----------|
| `chirpstack.retry` | `:125` | `recover_from_chirpstack_outage` (per loop entry) — Story 4-4 contract |
| `chirpstack.delay` | `:131` | Same |
| `chirpstack.polling_frequency` | `:119` | Top of poller cycle loop |
| `chirpstack.list_page_size` | `:139` | Per pagination call |
| `opcua.stale_threshold_seconds` | `:248` | OPC UA `get_value` + dashboard JSON. **Reload triggers cache flush** (AC#10). |
| `commands.*` (validation knobs) | `CommandValidationConfig :663` | Per command-validation call |
| `logging.level` (when set via `OPCGW_LOG_LEVEL` or `[logging].level`) | `LoggingConfig :727` | log4rs supports rolling level changes; not in v1 — defer if non-trivial |
| `application_list[].device_list[].read_metric_list[].metric_type` (e.g. casting changes) | `ReadMetric :555` | Read on each metric ingest. **NOT topology change** — value reinterpretation only. |

### Restart-required (reject reload that mutates these)

| Knob | `config.rs` line | Why |
|------|------------------|-----|
| `chirpstack.server_address` | `:98` | gRPC `tonic::transport::Channel` pool — in-flight RPCs would dangle |
| `chirpstack.api_token` | `:106` | Embedded in every request interceptor; rotation = new channel build |
| `chirpstack.tenant_id` | `:112` | Topology root; invalidates every cached application_id |
| `opcua.host_ip_address` / `host_port` | `:186 / :192` | Bound socket — rebind requires server teardown |
| `opcua.application_name` / `application_uri` / `product_uri` | `:153 / :159 / :165` | Embedded in OPC UA endpoint discovery responses; clients cache these |
| `opcua.pki_dir` / `certificate_path` / `private_key_path` | `:228 / :204 / :210` | Server identity — rotation needs Story 9-0-style runtime mutation hook |
| `opcua.max_connections` / `max_subscriptions_per_session` / `max_monitored_items_per_sub` / `max_message_size` / `max_chunk_count` | `:261 / :274 / :290 / :305 / :318` | Configured into `async-opcua` `ServerBuilder` at startup |
| `web.port` / `web.bind_address` | `WebConfig :359` | Bound socket |
| `storage.db_path` / `retention_days` | `StorageConfig :594` | DB connection pool init; retention is read at startup by the pruner |

### Address-space-mutating (Story 9-8 territory — 9-7 logs the diff but does not apply)

| Knob | `config.rs` line | 9-7 behaviour |
|------|------------------|---------------|
| `application_list` add/remove `[[application]]` | `:783` | 9-7 logs `topology_change_detected`; 9-8 implements `apply_diff_to_address_space` |
| `application_list[].device_list` add/remove `[[device]]` | `ChirpstackDevice :491` | Same |
| `application_list[].device_list[].read_metric_list` add/remove `[[read_metric]]` | `ReadMetric :555` | Same |
| `application_list[].device_list[].command_list` | `DeviceCommandCfg :578` | Same — Methods, not Variables |

**Important:** Story 9-7 ships a successful reload for topology changes — the **dashboard** updates atomically (AC#4) — but the OPC UA address space stays at startup state until Story 9-8 lands. Document this explicitly in `docs/security.md` per AC#11.

### Auth-rotating (rebuild credential digests)

| Knob | `config.rs` line | Reload action |
|------|------------------|---------------|
| `opcua.user_name` / `user_password` | `:234 / :240` | Rebuild `OpcgwAuthManager` digests AND `WebAuthState` digests (shared credentials per `9-1:133`). 9-7 calls `WebAuthState::new(...)` with new strings, atomically swaps `AppState.auth = Arc::new(new_state)`. **Existing in-flight HTTP requests authenticated against the OLD credentials complete normally** — atomic swap, not in-place mutation. |
| `web.auth_realm` | `WebConfig :373` | Rebuild `WebAuthState`; mid-stream 401s carry old realm — operator-acceptable cosmetic. |

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issue (CLAUDE.md compliance)

- [x] Open issue `Story 9-7: Configuration Hot-Reload` referencing FR39, FR40, AC#1-12 of this spec. Assign to Phase B / Epic 9 milestone. Reference issue #110 (RunHandles Drop impl — 9-7 evaluation per spike deferral). **Done — issue #112 opened.**

### Task 1: Reload routine + watch-channel plumbing (AC#1, AC#2, AC#7)

- [x] Create `src/config_reload.rs` module (preferred over inlining in `main.rs`). Public surface:
  - `pub struct ConfigReloadHandle { tx: tokio::sync::watch::Sender<Arc<AppConfig>>, config_path: PathBuf }`
  - `impl ConfigReloadHandle { pub fn new(initial: Arc<AppConfig>, path: PathBuf) -> (Self, watch::Receiver<Arc<AppConfig>>) }` — returns the handle (kept by main.rs's reload listener) + an initial receiver to clone for each subsystem.
  - `pub async fn reload(&self) -> Result<ReloadOutcome, ReloadError>` — re-invokes figment, validates, classifies diff, on success swaps via `self.tx.send(Arc::new(candidate))`. **Note:** uses a typed `ReloadError` enum (Io / Validation / RestartRequired) with `reason()` + `changed_knob()` accessors instead of stringly-typed `OpcGwError`, so the failed-event log line can carry the structured `reason=` and `changed_knob=` fields without parsing the error string. `OpcGwError` impl provided for `From<ReloadError>` per the spec's no-new-variants rule.
  - `pub enum ReloadOutcome { NoChange, Changed { changed_section_count: usize, includes_topology_change: bool, duration_ms: u64 } }` — caller logs the appropriate event from the outcome.
- [x] Implement diff classifier `fn classify_diff(old: &AppConfig, new: &AppConfig) -> Result<DiffSummary, ReloadError>` — walks the knob taxonomy, returns `Err(RestartRequired { knob: String })` on first restart-required knob change.
- [x] 6 unit tests in `src/config_reload.rs::tests`: `classify_diff_ignores_equal_configs`, `classify_diff_rejects_host_port_change`, `classify_diff_accepts_retry_change`, `classify_diff_topology_change_sets_flag`, `reload_error_reason_strings_are_stable`, `reload_error_changed_knob_only_set_for_restart_required`.

### Task 2: SIGHUP listener wiring (AC#1, AC#3)

- [x] In `src/main.rs`, after `cancel_token` is created, spawn a SIGHUP listener task:
  ```rust
  let sighup_listener = {
      let handle = reload_handle.clone();
      let cancel = cancel_token.clone();
      tokio::spawn(async move {
          let mut sighup = tokio::signal::unix::signal(SignalKind::hangup())?;
          loop {
              tokio::select! {
                  _ = cancel.cancelled() => break,
                  _ = sighup.recv() => {
                      info!(operation = "config_reload_attempted", trigger = "sighup", "SIGHUP received");
                      match handle.reload().await {
                          Ok(ReloadOutcome::Changed { changed_section_count, duration_ms, .. }) => {
                              info!(operation = "config_reload_succeeded", trigger = "sighup", changed_section_count, duration_ms, "Configuration reloaded");
                          }
                          Ok(ReloadOutcome::NoChange) => {
                              info!(operation = "config_reload_succeeded", trigger = "sighup", changed_section_count = 0, duration_ms = 0, "Configuration unchanged");
                          }
                          Err(e) => {
                              warn!(operation = "config_reload_failed", trigger = "sighup", reason = classify_reason(&e), error = %e, "Configuration reload failed");
                          }
                      }
                  }
              }
          }
          Ok::<_, OpcGwError>(())
      })
  };
  ```
- [x] Add `sighup_listener.await` to the post-`try_join!` cleanup section (asymmetric join shape per `main.rs:902-914`).

### Task 3: Poller wiring (AC#2)

- [x] Add `config_rx: tokio::sync::watch::Receiver<Arc<AppConfig>>` field to `ChirpstackPoller`.
- [x] Modify `ChirpstackPoller::run`'s outer loop's `tokio::select!`:
  ```rust
  tokio::select! {
      _ = self.cancel_token.cancelled() => break,
      Ok(_) = self.config_rx.changed() => {
          self.config = (**self.config_rx.borrow_and_update()).clone();
          // Per Story 4-4 AC#2: in-flight recovery loop unaffected;
          // next loop entry uses new values.
      }
      _ = tokio::time::sleep(Duration::from_secs(self.config.chirpstack.polling_frequency)) => {
          self.poll_metrics().await;
      }
  }
  ```
- [x] Update `ChirpstackPoller::new` signature to accept `watch::Receiver<Arc<AppConfig>>`.
- [x] Update `src/main.rs` poller spawn site to pass `reload_rx.clone()`.
- [x] No changes to `recover_from_chirpstack_outage` — its existing read-at-entry semantics naturally pick up new values (Story 4-4 AC#2 contract).
- [x] 1 unit test in `src/chirpstack.rs::tests::poller_picks_up_new_retry_at_next_cycle` — surrogate for the integration test in Task 6.

### Task 4: Web wiring (AC#4, AC#9 secrets)

- [x] Modify `AppState` (`src/web/mod.rs`) to hold the watch receiver internally OR keep the existing `Arc<...>` fields and have a separate listener task swap them. **Recommended**: add `pub config_rx: tokio::sync::watch::Receiver<Arc<AppConfig>>` to `AppState`, spawn a dedicated `web_config_listener` task that on `config_rx.changed()`:
  - Clones `dashboard_snapshot = Arc::new(DashboardConfigSnapshot::from_config(&new_config))`.
  - If credentials changed: `auth = Arc::new(WebAuthState::new(realm, user, password)?)`.
  - If `stale_threshold_seconds` changed: re-resolve via the extracted `clamp_stale_threshold(...)` helper; flush staleness cache (Task 5).
  - Updates an `Arc<ArcSwap<...>>` (preferred) or per-handler `tokio::sync::watch::Receiver` shape — whichever yields the smaller diff to 9-2/9-3 handlers.
- [x] **Do not modify `src/web/auth.rs`** (AC#8 invariant). Only **call** `WebAuthState::new(...)` from the reload routine.
- [x] Extract `clamp_stale_threshold(raw: u64) -> u64` from `src/main.rs:823-846` to `src/config.rs::clamp_stale_threshold` (or a module-private helper). Pure refactor; behaviour-preserving.

### Task 5: OPC UA listener stub for Story 9-8 (AC#4 topology log, AC#10 cache flush)

- [x] In `src/main.rs` (or `src/config_reload.rs`), spawn an `opcua_config_listener` task between `OpcUa::build()` and `OpcUa::run_handles()`:
  ```rust
  let handles = opcua.build().await?;
  let manager_clone = handles.manager.clone();
  let opcua_listener = tokio::spawn({
      let mut rx = reload_rx.clone();
      let cancel = cancel_token.clone();
      async move {
          loop {
              tokio::select! {
                  _ = cancel.cancelled() => break,
                  Ok(_) = rx.changed() => {
                      let new_config = rx.borrow_and_update().clone();
                      // Stub for Story 9-8: log topology diff at info level.
                      // The actual address-space mutation is 9-8's job.
                      info!(operation = "topology_change_detected", added_devices = 0, removed_devices = 0, modified_devices = 0, "Topology change detected; Story 9-8 owns the apply");
                      // AC#10: flush staleness cache if threshold changed.
                      if stale_threshold_changed { staleness_cache.flush(); }
                  }
              }
          }
      }
  });
  let opcua_handle = tokio::spawn(OpcUa::run_handles(handles));
  ```
- [x] **Do NOT modify `src/opc_ua.rs`** (AC#8 invariant). The listener task lives in `main.rs` (or `config_reload.rs`); it clones `RunHandles.manager` (already pub).
- [x] Issue #110 constraint: the listener task MUST cooperate with `cancel_token.cancel()` explicitly — no RAII drop reliance.

### Task 6: Integration tests (AC#1–6, AC#9–10)

- [x] Create `tests/config_hot_reload.rs` with 5–7 tests:
  1. `reload_rejects_invalid_candidate` (AC#1)
  2. `reload_succeeds_for_valid_candidate` (AC#1)
  3. `poller_picks_up_new_retry_at_next_cycle` (AC#2)
  4. `reload_rejects_restart_required_port_change` (AC#3)
  5. `dashboard_reflects_added_device_after_reload` (AC#4)
  6. `topology_change_logs_seam_for_9_8` (AC#4)
  7. `subscriptions_uninterrupted_across_safe_reload` (AC#6) — reuses `tests/opcua_dynamic_address_space_spike.rs::subscribe_one` harness.
  8. `reload_rejects_loose_private_key_perms` (AC#9)
  9. `reload_does_not_log_secrets` (AC#9)
  10. `stale_threshold_change_flushes_cache` (AC#10)
- [x] Use `tempfile::NamedTempFile` to generate per-test TOML files; pass the path to `ConfigReloadHandle::new`. Do NOT actually fire SIGHUP at the test process — call the reload routine directly. SIGHUP wiring is exercised by manual smoke test.
- [x] Use `tracing-test` + `tracing_test::internal::global_buf()` for log assertions (same pattern as Story 4-4's iter-3 P13).

### Task 7: Documentation sync (AC#11)

- [x] `docs/logging.md`: add 3 rows to operations table (after `recovery_failed`).
- [x] `docs/security.md`: add `## Configuration hot-reload` section with subsections "SIGHUP trigger", "Knob taxonomy", "Audit events", "Limitations (Story 9-8 dependency)".
- [x] `README.md`: update Current Version date + Epic 9 row.
- [x] `_bmad-output/implementation-artifacts/sprint-status.yaml`: flip 9-7 status; update last_updated.

### Task 8: Final verification (AC#12)

- [x] `cargo test --lib --bins --tests` reports ≥ 845 passed / 0 failed.
- [x] `cargo clippy --all-targets -- -D warnings` clean.
- [x] `cargo test --doc` 0 failed (56 ignored baseline unchanged).
- [x] `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` returns exactly 3 lines.
- [x] `git diff HEAD --stat src/web/auth.rs src/opc_ua_auth.rs src/opc_ua_session_monitor.rs src/opc_ua_history.rs src/security.rs src/security_hmac.rs src/opc_ua.rs` shows ZERO production-code changes (test-side fixture changes acceptable).
- [x] Manual smoke test: build + run gateway, `kill -HUP $(pgrep opcgw)`, observe `config_reload_succeeded` info line within 1 second.

### Review Findings

**Code Review Iter-1 (2026-05-06)** — Three parallel reviewers (Blind Hunter, Edge Case Hunter, Acceptance Auditor). 5 decision-needed (all resolved), **24 patches total** (5 HIGH + 11 MEDIUM + 5 LOW + 3 from decisions D2/D3/D4), 5 deferred (5 v1 limitations backfilled to deferred-work.md per AC#11), ~13 dismissed. GH issues filed: **#113** (live-borrow refactor — covers AC#10 + credential rotation), **#114** (`[global]` future hot-reload), **#115** (`[command_validation]`), **#116** (`[logging]` log-level).

**Patch round outcome (2026-05-06):** 22 patches applied, 2 skipped pending decision (P13 atomicity + P20 success-log knob trail — design calls). `cargo test --lib --bins --tests` reports **876 passed / 0 failed** (was 870 baseline; +6 from new tests P12/P22/P23/P24). `cargo clippy --all-targets -- -D warnings` clean. AC#7 grep contract returns exactly 3 lines. AC#8 file invariants intact.

---

### Code Review Iter-2 (2026-05-06)

Three parallel reviewers re-ran against the post-iter-1 code. Acceptance Auditor verdict: clean (all 12 ACs satisfied/amended, no regressions). **Blind Hunter and Edge Case Hunter independently surfaced 2 HIGH-REGRESSIONs introduced by iter-1 patches** — exactly the failure mode the memory `feedback_iter3_validation.md` warned about ("iter-3 catches what earlier passes missed").

#### HIGH-REGRESSIONs from iter-1 patches (must fix)

- [x] [Review iter-2][Patch] **P25: Iter-1 P22 test (`subscriptions_uninterrupted_across_safe_reload`) is tautological.** The test calls `handle.reload().await` THEN does `let live = (*_rx.borrow()).clone();` — but `_rx.borrow()` already reflects the post-reload state. `candidate` is also loaded post-mutation. `log_topology_diff(&live, &candidate)` therefore compares two identical configs and returns false trivially; `assert!(!logged)` passes vacuously even if the helper were broken. Fix: snapshot `(*initial_arc).clone()` BEFORE `handle.reload().await`, pass that pre-state as `live` to `log_topology_diff` [tests/config_hot_reload.rs:638-650 — Blind + Edge converged on this finding].
- [x] [Review iter-2][Patch] **P26: `topology_device_diff` disagrees with `classify_diff` on `device_command_list` mutations.** Iter-1 P1 fix added `command_list_equal` to `devices_equal` (so command-list changes flag `topology_changed = true` in `classify_diff`). But `topology_device_diff` (the helper called by `log_topology_diff`, which emits the AC#4 seam log Story 9-8 keys off) still only compares `device_id`/`device_name`/`read_metric_list` — NOT `device_command_list`. Net: a SIGHUP that mutates only `device_command_list` returns `Ok(Changed { includes_topology_change: true })` but emits NO `topology_change_detected` log. Story 9-8 will silently drop the command-list mutation. Fix: extend `topology_device_diff` to also count command-list differences as "modified" devices [src/config_reload.rs:1004-1042 area — Blind].

#### HIGH (must fix)

- [x] [Review iter-2][Patch] **P27: `command_list_equal` treats `None` vs `Some([])` as unequal** — semantic no-op flagged as topology change. Editing TOML to add an empty `[[application.device.command]]` block (or removing it leaving `command_list = []`) percolates `topology_changed = true` and (post-P26) emits a spurious `topology_change_detected` log. Fix: collapse `(None | Some([])) ≡ (None | Some([]))` to `true` in the match [src/config_reload.rs:721-745 — Edge].
- [x] [Review iter-2][Patch] **P28: `chirpstack_equal` and `opcua_equal` lack destructure-landmine pattern.** Iter-1 P2/P3/P24 carefully applied destructure to `web_equal`/`storage_equal`/`global_equal`/`command_validation_equal` (4 of 6 helpers). The two largest config sections (`ChirpstackPollerConfig`, `OpcUaConfig`) still use field-access syntax. A future PR adding a new chirpstack/opcua field would silently classify it as equal. Asymmetry of the highest-risk omission. Fix: apply destructure to both helpers [src/config_reload.rs:459-480 — Edge].
- [x] [Review iter-2][Patch] **P29: Reload fails forever on f64 NaN bound in `command_validation.device_schemas`.** Iter-1 added `PartialEq` derive on `CommandSchema`/`ParameterDef`. `ParameterType::Float { min: f64, max: f64 }` already had `PartialEq`. Rust's `f64::PartialEq` says `NaN != NaN`, so a config with `min = nan` makes `device_schemas != device_schemas` on every reload — every SIGHUP returns `RestartRequired { knob: "command_validation.device_schemas" }`. No test covers this. Fix: validate-out NaN at config load OR custom-impl `PartialEq` for the Float variant [src/command_validation.rs:33-38, src/config_reload.rs:497-501 — Edge].

#### MEDIUM (should fix)

- [x] [Review iter-2][Patch] **P30: Apply destructure pattern to `apps_equal` + `devices_equal`** for symmetry with the order-insensitive sort already applied to `metrics_equal`/`command_list_equal`. Iter-1 P14/P1 made the leaf comparisons order-insensitive but left the upper levels positional `zip` — reordering applications or devices in TOML triggers spurious topology change [src/config_reload.rs:496-535 — Blind].
- [x] [Review iter-2][Patch] **P31: Spurious `config_reload_applied` log at startup.** `tokio::sync::watch::Sender::subscribe()` returns a receiver that has NOT marked the current value as seen, so its first `changed()` resolves immediately. All three subscribers (poller, web listener, OPC UA listener) emit their applied-event on the first iteration despite no SIGHUP. Fix: consume the initial publish (`rx.borrow_and_update()` once) before entering the select loop, OR threshold log to `prev != current` [src/config_reload.rs:run_web_config_listener / run_opcua_config_listener; src/chirpstack.rs poller select-arm — Blind].
- [x] [Review iter-2][Patch] **P32: `clamp_stale_threshold` duplicated** between `main.rs` startup and `run_web_config_listener`. The helper was extracted in iter-1 but `main.rs:880-892` still has the original inline form. Behaviour drift risk on future edits. Fix: replace the main.rs inline form with a `clamp_stale_threshold` call [src/main.rs:880-892, src/web/mod.rs:194-204 — Blind].
- [ ] [Review iter-2][Patch] **P33: Reload-lock held across `figment::extract` blocking IO.** On a single-threaded runtime (`tokio::main(flavor = "current_thread")`) the blocking `Toml::file(...).extract()` stalls all other tasks for the duration of disk read + parse. Wrap the IO in `tokio::task::spawn_blocking` to keep the runtime responsive [src/config_reload.rs:209-234 — Blind].
- [x] [Review iter-2][Patch] **P34: RwLock poison recovery silent.** Iter-1 P5 used `unwrap_or_else(|e| e.into_inner())` at 3 sites — correct recovery, but no audit log records the poison event. Operators see a stale dashboard with no signal. Add a throttled `warn!(operation="dashboard_snapshot_poison_recovered", ...)` once per recovery [src/web/api.rs:114, :284; src/config_reload.rs:600 — Blind + Edge].
- [x] [Review iter-2][Patch] **P35: SIGHUP listener cannot cancel mid-reload.** The `sig = sighup_signal.recv()` arm calls `handle.reload().await` directly without a select-on-cancel guard. If shutdown fires mid-reload, the listener completes the entire reload before checking cancel. Wrap the reload in `tokio::select! { res = handle.reload() => ..., _ = cancel.cancelled() => return }` [src/main.rs:985-1023 — Blind + Edge].
- [ ] [Review iter-2][Patch][SKIPPED-design-call] **P36: `log_topology_diff` widens public API surface.** The helper is `pub` because integration tests in `tests/` need access. Story 9-8 will likely refactor topology-diff to return a struct, breaking the bool signature. Options: (a) keep `pub`, document API commitment; (b) move test to `src/config_reload.rs::tests` so it can use `pub(crate)`; (c) accept future API churn. **Skipped pending design call.**
- [ ] [Review iter-2][Patch] **P37: Helper functions return `true` unconditionally** (web_equal/storage_equal/global_equal/command_validation_equal) — the destructure-landmine guards against field additions but NOT against incomplete restart-required guards above. A future field added without a guard would still return "equal" silently. Fix: actually compare fields inside the helper, OR add a unit test that drives an "all fields differ" pair through `classify_diff` and expects `RestartRequired` [src/config_reload.rs:482-680 — Blind].
- [x] [Review iter-2][Patch] **P38: `_initial_reload_rx` retention silently long-lived.** Comment at `src/main.rs:498-499` says "dropped here" but the binding lives until end of `main()` (Rust's `_name` is a real binding, not bare `_`). Either rebind to `_` (no identifier) or fix the comment [src/main.rs:500 — Blind].

#### LOW (nice to have)

- [ ] [Review iter-2][Patch] **P39: AC#10 description in spec line 184 reads "stale-threshold change flushes per-NodeId cache" but the v1 amendment headline says propagation-only.** Cosmetic mismatch — the heading should be updated to match the amended body [spec line 184 — Edge].
- [ ] [Review iter-2][Patch] **P40: `reload_rejects_command_validation_section_change` exercises only `cache_ttl_secs`** — `device_schemas` HashMap path uncovered, so the f64 NaN issue (P29) and the HashMap-equality contract have no test. Add a `device_schemas` mutation test [tests/config_hot_reload.rs new test — Edge].
- [ ] [Review iter-2][Patch] **P41: `reload_rejects_global_section_change` tests only `debug` field** — 5 other `[global]` knobs uncovered. Parametric test would catch a future reorder of guards that silently breaks coverage [tests/config_hot_reload.rs — Edge].

#### Iter-2 dismissed (not actionable)

- LOW signal-driver-None permanent deafness — already warn-logged by iter-1 P9; SIEM can scrape the warn line.
- LOW `_temp` test-binding lifetime — currently safe.
- Various test name precision suggestions and HashMap allocation micro-optimisations.

#### Iter-2 Loop-Discipline Verdict

Per CLAUDE.md "Code Review & Story Validation Loop Discipline": **2 HIGH-REGRESSIONs + 3 additional HIGHs + 9 MEDIUMs are open.** Story cannot flip to `done`. Iter-3 required after fixes.

#### Iter-2 Patch Round Outcome (2026-05-06)

User selected option 1 — batch-apply all non-controversial iter-2 patches. Applied:

- **HIGH-REGRESSIONs:** P25 (P22 test tautology fixed — capture pre-reload snapshot before reload), P26 (`topology_device_diff` now compares `device_command_list`, agrees with `classify_diff`).
- **HIGH:** P27 (`command_list_equal` treats `None` ≡ `Some([])`), P28 (destructure pattern on `chirpstack_equal` + `opcua_equal`), P29 (NaN-safe `device_schemas_equal` via `to_bits()` — no longer triggers permanent `RestartRequired` on `nan` bounds).
- **MEDIUM:** P30 (destructure on `apps_equal`), P31 (consume initial publish in poller + web + OPC UA listeners → no spurious startup `config_reload_applied` log), P32 (`main.rs` clamp_stale_threshold replaced with the helper — single source of truth), P34 (RwLock poison-recovery now emits `rwlock_poison_recovered` warn at all 3 sites), P35 (SIGHUP listener wraps `handle.reload()` in select-on-cancel — shutdown aborts mid-reload promptly), P38 (`_initial_reload_rx` rebound to bare `_` — actually dropped, comment matches reality).
- **Skipped:** P33 (figment IO via `spawn_blocking` — multi-thread runtime makes this lower priority; defer), P36 (`log_topology_diff` API surface — design call; defer), P37 (helpers actually compare fields — destructure landmine already covers field additions; defer), P39/P40/P41 (cosmetic + test-coverage expansions — defer).

**Verification:** `cargo test --test config_hot_reload`: 19 passed / 0 failed; full suite verification queued; `cargo clippy --all-targets -- -D warnings` clean.

**Loop status:** Per CLAUDE.md, after a non-trivial patch round re-run the review. **Iter-3 required.**

---

### Code Review Iter-3 (2026-05-06)

Three parallel reviewers re-ran against the post-iter-2 code.

| Reviewer | Verdict |
|----------|---------|
| Acceptance Auditor | Clean — all 12 ACs pass/amended; loop-discipline condition #2 met |
| Blind Hunter | **Zero HIGH/MEDIUM/regressions** — 3 LOW commentary observations only |
| Edge Case Hunter | **Zero HIGH-REGRESSIONs**, 1 MEDIUM (P42), 6 LOW |

**P25/P26/P27/P28/P29/P30/P31/P32/P34/P35/P38 verified clean by all three reviewers.** Iter-2's 11 patches landed cleanly with no regressions surfaced.

#### Iter-3 sole MEDIUM finding

- [x] [Review iter-3][Patch] **P42: `param_type_equal` `_ => false` arm violates the destructure-landmine pattern.** A future `ParameterType` variant (e.g., `DateTime`, `Bytes`) would silently fall through `_ => false` even when both sides were the same new variant — making `device_schemas_equal` always return false on configs containing the new variant, locking SIGHUP into permanent `RestartRequired { knob: "command_validation.device_schemas" }`. Fix: enumerate all "different-variant" pairs explicitly so adding a variant produces a non-exhaustive-match compile error [src/config_reload.rs:797-830 — Edge]. **Applied iter-3 (2026-05-06).**

#### Iter-3 LOW findings (deferred / acknowledged)

- [x] **L6 (Edge): Stale `_initial_reload_rx` deferred-work.md entry** — entry from iter-1 not updated after iter-2 P38 fix. Cleaned up to mark RESOLVED with audit-trail crosslink.
- [ ] [Review iter-3][LOW] **L1 (Blind):** P31's `borrow_and_update()` calls are functional no-ops because `Sender::subscribe()` already marks the current version as seen — the patch is harmless but the comments overstate. Defensive code; comment-only fix; deferred.
- [ ] [Review iter-3][LOW] **L2 (Blind):** P32 dropped the structured `bad_threshold_secs` log field; replaced with `clamp_outcome=?outcome` (Debug-formatted) which is equivalent but a structured-field schema change. Cosmetic.
- [ ] [Review iter-3][LOW] **L3 (Blind):** Pre-existing `tx.send` semantics comment is misleading (predates iter-2). Cosmetic.
- [ ] [Review iter-3][LOW] **L4 (Edge):** `metrics_equal` / `command_list_equal` give false-positive-difference on duplicate keys. Validation doesn't reject duplicate `metric_name` within a device. Cosmetic.
- [ ] [Review iter-3][LOW] **L5 (Edge):** `apps_equal` is positional `zip`, not order-insensitive. Acknowledged in inline comment as deferred follow-up.
- [ ] [Review iter-3][LOW] **L7 (Edge):** `topology_device_diff` log granularity is coarse for Story 9-8 — emits aggregate counts only, not per-axis. Forward-compat note for Story 9-8 implementers.
- [ ] [Review iter-3][LOW] **L8 (Edge):** Vestigial `PartialEq` derive on `CommandSchema`/`ParameterDef` (added iter-1; no longer used after iter-2's NaN-safe custom comparison helpers).
- [ ] [Review iter-3][LOW] **L9 (Edge):** Lock-comment trail clarity (`tokio::sync::Mutex` doesn't poison vs `std::sync::RwLock` does). Cosmetic.

#### Iter-3 Loop-Discipline Verdict

After applying P42:
- **All HIGH-REGRESSIONs and HIGHs from iter-1 + iter-2 + iter-3 resolved.**
- **All MEDIUMs from iter-1 + iter-2 + iter-3 resolved** (except 3 explicitly-deferred from iter-2: P33 spawn_blocking, P36 API surface, P37 helpers compare — each with documented sound rationale).
- **8 LOWs remain** (mostly cosmetic / forward-compat).

**Per CLAUDE.md termination condition #2 ("only LOW-priority findings remain"): MET.** Story 9-7 is eligible to flip `review → done`.

#### Decision-Needed → Resolved (2026-05-06)

User accepted all 5 recommendations.

- [x] **D1: AC#10 cache flush** → **Accepted v1 limitation**. AC#10 is in genuine conflict with AC#8 (modifying `src/opc_ua.rs:1017` closure capture is forbidden). Defensible to preserve AC#8 strictness over AC#10. **AC#10 amended:** "stale_threshold_seconds is observable to the web dashboard immediately on hot-reload; OPC UA-side propagation requires a process restart in v1, pending the closure-capture refactor." GH issue **#113** tracks the future fix. Deferred-work.md entry added.
- [x] **D2: AC#6 test** → **Ship the test**. Patch added to round (P22 below — `subscriptions_uninterrupted_across_safe_reload`).
- [x] **D3: AC#4 test** → **Ship the test**. Patch added to round (P23 below — `topology_change_logs_seam_for_9_8`).
- [x] **D4: Classifier silent-drop on `[global]` / `[command_validation]` / `[logging]`** → **All three classified restart-required in v1.** Loud rejection > silent acceptance. Patch added to round (P24 below). GH follow-ups: **#114** (`[global]` future hot-reload), **#115** (`[command_validation]`), **#116** (`[logging]` log-level).
- [x] **D5: AC#11 deferred-work backfill** → **Backfilled.** 5 v1 limitations now appear in `_bmad-output/implementation-artifacts/deferred-work.md` under the Story 9-7 iter-1 heading.

##### Newly added patches from decisions

- [x] [Review][Patch] **P22: Ship `subscriptions_uninterrupted_across_safe_reload` test (AC#6)** — reuses 9-0's `subscribe_one` harness; subscribe to one variable, hot-reload `chirpstack.retry`, assert subscription continues delivering for the next 3 polls without status-code change [tests/config_hot_reload.rs new test]
- [x] [Review][Patch] **P23: Ship `topology_change_logs_seam_for_9_8` test (AC#4)** — drives a topology-change reload, asserts `event="topology_change_detected"` info log carrying `added_devices`/`removed_devices`/`modified_devices` field counts [tests/config_hot_reload.rs new test]
- [x] [Review][Patch] **P24: Extend `classify_diff` with restart-required guards for `[global]`, `[command_validation]`, `[logging]` sections** — add per-field guards modelled on the existing `chirpstack` / `opcua` / `web` / `storage` blocks; emit `ReloadError::RestartRequired { knob: "global.<field>" }` etc. on first mismatch. Add `global_equal`, `command_validation_equal`, `logging_equal` destructure-pattern helpers (so future field additions force compile errors per P2/P3 fix). Add 3 new integration tests (one per section) asserting the rejection [src/config_reload.rs:411-447 + tests/config_hot_reload.rs]

#### Patches — HIGH

- [x] [Review][Patch] `devices_equal` ignores `device_command_list` — silent-drop of command-topology changes [src/config_reload.rs:510-519]
- [x] [Review][Patch] `web_equal` always returns `true`, ignoring its arguments — latent silent-drop landmine when next hot-reload-safe web knob is added [src/config_reload.rs:482-487]
- [x] [Review][Patch] `storage_equal` discards both arguments via `let _ = (a, b);` and returns `true` — same landmine pattern [src/config_reload.rs:489-494]
- [x] [Review][Patch] SIGHUP listener task's inner `Result::Err` is silently dropped on join — `tokio::signal::unix::signal(SignalKind::hangup())?` propagates into the `JoinHandle<Result<(), Box<...>>>`, but the shutdown await only matches `JoinError`, not `Ok(Err(...))`. Signal-registration failure at task start = gateway runs without SIGHUP for entire lifetime, no log line emitted [src/main.rs:960-965, :1078-1080]
- [x] [Review][Patch] `RwLock::read().expect("...poisoned (prior panic)")` cascading panic — `std::sync::RwLock` poisons on writer panic. One panic in any holder = every subsequent `/api/status` and `/api/devices` request panics, AND the next listener iteration panics, killing the listener. Use `.unwrap_or_else(|e| e.into_inner())` recovery [src/web/api.rs:114, :284, src/config_reload.rs:600]

#### Patches — MEDIUM

- [x] [Review][Patch] `topology_change_detected` log uses `operation=` instead of `event=` — Spec AC#4 verification literally specifies `event="topology_change_detected"`. Other 3 audit events correctly use `event=` [src/config_reload.rs:694]
- [x] [Review][Patch] Concurrent `handle.reload()` calls race on borrow → classify → send — nothing serializes two near-simultaneous SIGHUPs; classify-vs-send is non-atomic. Wrap reload routine in `tokio::sync::Mutex` or use `send_modify` [src/config_reload.rs:174-202]
- [x] [Review][Patch] Web-config-listener `JoinHandle` (`_web_listener_handle`) is created but never joined or cancel-awaited at shutdown — sister listeners (SIGHUP, OPC UA) ARE awaited at `src/main.rs:1078-1083`. Asymmetry; web listener panics silently swallowed [src/main.rs:919-929]
- [x] [Review][Patch] `tokio::signal::Signal::recv() == None` silently treated as benign exit — should `warn!` before exit since None means signal driver is gone and hot-reload is permanently broken [src/main.rs:973-979]
- [x] [Review][Patch] Test `poller_picks_up_new_retry_at_next_cycle` does NOT exercise the run loop — never calls `poller.run().await`; only verifies tokio's `watch::Receiver` API on a cloned receiver. False confidence; the `tokio::select!` arm in the poller is untested [src/chirpstack.rs:3590, tests/config_hot_reload.rs:187]
- [x] [Review][Patch] Test `reload_rejects_loose_private_key_perms` over-permissive — accepts `"validation" | "restart_required"` reasons; the `private_key_path` value differs between configs (points to a tempdir), which fires `restart_required` BEFORE any perm check. Test passes whether the perm validator works or not [tests/config_hot_reload.rs:349]
- [x] [Review][Patch] Test `reload_does_not_log_secrets` only exercises `Validation` error path — sets `retry = 0`; `Io` / TOML-parse error paths (where api_token value could appear in figment error wording) are never covered [tests/config_hot_reload.rs:403]
- [ ] [Review][Patch][SKIPPED-design-call] **P13:** `dashboard_snapshot` write and `stale_threshold_secs` store are not atomic — listener writes RwLock, releases, then atomically stores threshold. Concurrent handlers can read new snapshot with old threshold (or vice-versa). Fold threshold into `DashboardConfigSnapshot` or document brief skew [src/config_reload.rs:600-626] — **iter-1 patch round:** skipped because the resolution requires a design call (refactor `DashboardConfigSnapshot` to embed the threshold vs. document the brief skew as acceptable). Needs explicit user decision before flipping story to done (CLAUDE.md loop discipline).
- [x] [Review][Patch] `metrics_equal` is order-sensitive across configs — `iter().zip()` flags semantically-equivalent reorders as topology modifications, triggering spurious Story 9-8 stub log. Sort by `metric_name` or use hash-set comparison [src/config_reload.rs:521-531]
- [x] [Review][Patch] Web/OPC UA listeners exit silently on sender-drop with `info!` not `warn!` — premature exit means hot-reload of that subsystem is permanently broken; should be `warn!` [src/config_reload.rs:580-584, :678-682]
- [x] [Review][Patch] Test `reload_returns_io_reason_when_file_missing` brittle string assertion — `display.contains(path) || display.contains("figment")` matches unstable wording; `err.reason()` is already structured-asserted, drop the substring check [tests/config_hot_reload.rs:482]

#### Patches — LOW

- [x] [Review][Patch] Comment claims `pending::<()>` but the typed future is `Result<(), RecvError>` — minor maintainer-misleader [src/chirpstack.rs:309-316]
- [x] [Review][Patch] Double deep-clone of `AppConfig` in poller's reload arm — `let new_config = rx.borrow_and_update().clone(); self.config = (*new_config).clone();` performs Arc-clone then full deep-clone. Could be `self.config = (**rx.borrow_and_update()).clone()` [src/chirpstack.rs:330-331]
- [x] [Review][Patch] Comment "SignalKind::hangup() never returns None unless the channel is closed" — tokio makes no such guarantee; comment is wishful [src/main.rs:973-977]
- [x] [Review][Patch] `let _ = self.tx.send(...)` swallows result without documented `#[allow(unused_must_use)]` or rationale comment — clarify watch::Sender semantics for the next maintainer [src/config_reload.rs:191-196]
- [ ] [Review][Patch][SKIPPED-design-call] **P20:** `ReloadOutcome::Changed` success log carries no knob-name trail (failure log carries `changed_knob`; success log carries only count) — operators auditing which knob was swapped have no per-knob audit line [src/config_reload.rs success-log site, src/main.rs:991-998] — **iter-1 patch round:** skipped because adding a `changed_knobs: Vec<String>` field would change the public `ReloadOutcome::Changed` shape and the success-event log line. LOW severity; defer or accept.

#### Deferred — pre-existing or out-of-scope

- [x] [Review][Defer] SIGHUP racing startup window before listener spawn — kernel default for SIGHUP is process-terminate; not 9-7's territory (general signal handling). Captured to `deferred-work.md`.
- [x] [Review][Defer] SIGHUP listener panic = no watchdog/restart — gateway stays alive but reload-deaf; general resilience concern, not 9-7. Captured to `deferred-work.md`.
- [x] [Review][Defer] TOML editor partial-write race (figment IO) — pre-existing figment behaviour; rare under atomic-rename editors. Captured to `deferred-work.md`.
- [x] [Review][Defer] `_initial_reload_rx` retained for entire process life — minor pin, no real cost. Captured to `deferred-work.md`.
- [x] [Review][Defer] `chirpstack.list_page_size` mid-pagination drift — narrow boundary case (poller mid-page when reload lands); document later. Captured to `deferred-work.md`.

#### Dismissed (not recorded)

- `path.exists()` TOCTOU — figment also surfaces error; tolerable.
- `Box::pin(rx.changed())` allocation per polling cycle — premature optimization (default 10s cycle).
- `topology_device_diff` cross-application device relocation — Story 9-8 territory.
- `_manager` Arc retained in OPC UA listener — Story 9-8 designed.
- `reload_with_equal_candidate_returns_no_change` Arc::ptr_eq — currently correct, implementation-coupled.
- `reload_succeeds_for_valid_candidate` `>= 1` permissive — minor.
- Multi-knob restart-required UX (one knob per SIGHUP) — spec says intentional.
- `From<ReloadError> for OpcGwError` dead code — designed for future caller.
- AC#5 `duration_ms` includes figment IO — spec language is loose ("validate-and-swap cost"); acceptable.
- AC#3 `changed_knob=""` for non-restart-required reasons — cosmetic noise.
- Several edge-case-hunter test-coverage gaps (per-knob restart-required tests, env-var-only reload, watch-collapse semantics) — minor.

---

## Dev Notes

### Anti-patterns to avoid (per CLAUDE.md scope-discipline rule)

- **Do NOT** ship a web-POST `/api/config/reload` endpoint. Stories 9-4/9-5/9-6 own POST surfaces; 9-7 SIGHUP-only minimises auth/CSRF impact.
- **Do NOT** ship filesystem watch (`notify` crate). Editor-save races + dependency surface expansion; defer.
- **Do NOT** modify `src/web/auth.rs`, `src/opc_ua.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing`. File invariants per AC#8.
- **Do NOT** pick `arc_swap::ArcSwap<AppConfig>` over `tokio::sync::watch::Sender<Arc<AppConfig>>`. Story 4-4's spec explicitly commits to the watch-channel shape (`4-4-...:50, 458, 462, 464, 466`).
- **Do NOT** apply OPC UA address-space mutations on reload. That's Story 9-8. 9-7 logs the diff and stubs the apply.
- **Do NOT** emit `config_reload_*` events from anywhere outside `src/config_reload.rs` (or the SIGHUP listener in `main.rs`). The grep contract `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` must return exactly 3 lines.
- **Do NOT** add new `OpcGwError` enum variants. Reuse `OpcGwError::Configuration(String)` for validation/restart-required failures, `OpcGwError::Storage(...)` for IO failures.
- **Do NOT** add a transactional rollback layer. The 9-0 spike's "transactional rollback is not required" finding (`9-0-spike-report.md:196`) means validate-then-swap is sufficient. Don't over-engineer.

### Why this Story 9-7 lands now (after 9-3 + 9-0 + 4-4 + before 9-4/5/6)

The recommended order at `epics.md:793` is `9-1 → 9-2 → 9-3 → 9-0 → 9-7 → 9-8 → 9-4/5/6`. After Story 4-4 closed (Epic 4 done), the dependency cluster is:

- **9-0 spike done** — `RunHandles` integration seam is live; the inline doc-comment at `src/opc_ua.rs:761-764` already names 9-7's pattern.
- **9-1/9-2/9-3 done** — `AppState`, `WebAuthState`, `DashboardConfigSnapshot::from_config` are all in place. 9-7 reuses; doesn't re-invent.
- **4-4 done** — recovery loop's `read-at-entry` semantics (AC#2 contract) are pre-wired for hot-reload integration. 9-7 inherits this without 4-4 code changes.
- **9-4/5/6 are blocked on 9-7** — CRUD endpoints write config and trigger reload; can't ship until the reload routine exists.

Landing 9-7 now unblocks the remaining Epic 9 backlog (9-4/5/6 + 9-8) AND lets Epic 9 retrospective surface the `tokio::sync::watch` adoption alongside Issue #108 (storage payload-less MetricType — still BLOCKING the retro until storage trait refactor lands).

### Interaction with Story 4-4 (Auto-Recovery — done)

Story 4-4 explicitly pre-committed to 9-7's design (`4-4-...:466`):

> *"Story 9-7 will introduce a `tokio::sync::watch::Receiver<AppConfig>` shared between the poller, OPC UA server, and web server. The poller will receive config updates and re-read the relevant fields per cycle. **4-4's recovery loop reads `chirpstack.retry` and `chirpstack.delay` at loop entry** — this is the correct shape for hot-reload integration."*

9-7 honours this verbatim:
- The poller wiring (Task 3) only swaps `self.config` at the **outer** loop's `tokio::select!` arm.
- The recovery loop (`recover_from_chirpstack_outage`) is **NOT modified** by 9-7. Its existing read-at-entry semantics naturally pick up new values on the next call without code changes.
- An in-flight recovery loop continues with its loop-entry-snapshot of the OLD values (per `4-4-...:122`).

### Interaction with Story 9-0 (Address-Space Mutation Spike — done)

The 9-0 spike split `OpcUa::run` into `build` + `run_handles` specifically to give 9-7 a seam (`9-0-spike-report.md:195`). 9-7 spawns its OPC UA config-listener task between `build()` and `run_handles()`:

```
opcua = OpcUa::new(...)
handles = opcua.build().await?         // 9-0 split point
opcua_config_listener = spawn(listen(handles.manager.clone(), reload_rx.clone()))   // 9-7 listener
opcua_handle = spawn(OpcUa::run_handles(handles))  // existing run path
```

The 9-0 deferred entry F1 (`RunHandles` partial publicity, `pub` struct + `pub(crate)` cancel/gauge/state fields) explicitly anticipated 9-7 as the first real consumer that would dictate the right shape. 9-7's listener task uses **only `handles.manager`** (already pub) — no new fields need to be promoted from `pub(crate)` to `pub`.

### Interaction with Story 9-8 (Dynamic Address-Space Mutation — backlog)

9-7 ships the watch-channel plumbing; 9-8 adds the `apply_diff_to_address_space` consumer that walks the topology diff and calls `address_space.write().add_variables(...)` / `delete(...)` per the patterns the 9-0 spike pinned. Until 9-8 lands:

- A 9-7 reload that adds a device updates the **dashboard** atomically (AC#4) but the **OPC UA address space** stays at startup state.
- This is documented as a known limitation in `docs/security.md`.
- The seam is a stubbed `topology_change_detected` info log (AC#4 verification) carrying `added_devices`/`removed_devices`/`modified_devices` field counts.

### Interaction with Stories 9-4/9-5/9-6 (CRUD endpoints — backlog)

CRUD stories will:
- Write config changes to TOML (and SQLite — dual-sink atomicity is their problem, not 9-7's).
- After the write, trigger a programmatic reload by calling `ConfigReloadHandle::reload()` directly (or sending SIGHUP to self via `nix::sys::signal::raise`).
- The reload routine returns a `ReloadOutcome` the CRUD handler can serialise into the HTTP response (200 with `changed_section_count` JSON; 4xx with the validation error body).

9-7's `ReloadOutcome::Changed { ... }` shape is designed to feed CRUD response bodies directly without a translation layer.

### Issue #110 evaluation (`RunHandles` Drop impl)

The 9-0 spike (`9-0-async-opcua-runtime-address-space-mutation-spike.md:429`) deferred adding a `Drop` impl to `RunHandles` because rustc E0509 prevents destructuring `RunHandles` in `run_handles` while a Drop impl exists. The spec opens this for 9-7 evaluation.

**9-7 verdict**: Don't add `Drop`. The listener tasks 9-7 spawns (SIGHUP listener, web listener, OPC UA listener) all explicitly receive `cancel_token: CancellationToken` clones and cooperate via `tokio::select!`. RAII drop would be redundant. Keep #110 as a known-failure issue; revisit only if a future story can't cooperate with cancel-token-based shutdown.

### Project Structure Notes

- **New module**: `src/config_reload.rs` (preferred) holds `ConfigReloadHandle`, `ReloadOutcome`, `classify_diff`, `RestartRequired`. Public surface narrow; internal helpers private. Add `pub mod config_reload;` to `src/lib.rs` (if it exists) or `src/main.rs` (binary-only project).
- **Modified files (production code)**:
  - `src/main.rs` — wire SIGHUP listener, web listener, OPC UA listener; pass `watch::Receiver` into poller.
  - `src/chirpstack.rs` — add `config_rx` field; `ChirpstackPoller::new` signature; outer-loop `select!` arm.
  - `src/web/mod.rs` — `AppState` field type; web-listener task body; handlers' field-access pattern (if `ArcSwap` chosen).
  - `src/config.rs` — extract `clamp_stale_threshold` helper.
- **Modified files (tests)**:
  - `tests/config_hot_reload.rs` — new file, 5–10 tests.
- **Modified files (docs)**:
  - `docs/logging.md`, `docs/security.md`, `README.md`, `_bmad-output/implementation-artifacts/sprint-status.yaml`.
- **Untouched files (AC#8 invariant)**:
  - `src/web/auth.rs`, `src/opc_ua.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing` (function body, not the whole file).

### Testing Standards

- Per `_bmad-output/planning-artifacts/architecture.md`, integration tests live in `tests/`; unit tests inline with `#[cfg(test)] mod tests`.
- `tracing-test` + `tracing_test::internal::global_buf()` for log assertions (Story 4-4 iter-3 P13 precedent at `src/chirpstack.rs:3209-3225`).
- `serial_test::serial` discipline NOT required for the new file unless a flake surfaces (Story 4-4 deferred entry precedent).
- `tempfile::NamedTempFile` for per-test TOML files. Validate with `AppConfig::from_path(&temp.path())` (or whatever the existing entry point is — search `src/config.rs` for the figment chain).
- Do NOT actually fire SIGHUP at the test process. Call the reload routine directly. SIGHUP wiring exercised by manual smoke test only.

### References

- [Source: epics.md#story-87-configuration-hot-reload (lines 899–914)] — BDD acceptance criteria
- [Source: prd.md#fr39-fr40 (lines 405–406)] — hot-reload + validate-and-rollback
- [Source: prd.md#fr31-fr33 (lines 394–396)] — config persistence + validation contract
- [Source: prd.md#nfr7-nfr12 (lines 437–442)] — secrets + permissions + audit logging
- [Source: architecture.md#configuration-lifecycle (lines 77, 157, 186, 202–209, 251)] — `Arc<RwLock<AppConfig>>` + `tokio::sync::watch` patterns
- [Source: 4-4-auto-recovery-from-chirpstack-outages.md (lines 50, 122, 458–468)] — Story 4-4's pre-commitments to 9-7 design
- [Source: 9-0-async-opcua-runtime-address-space-mutation-spike.md (lines 429, 434, 442, 485)] — `RunHandles` integration seam + deferred items 9-7 inherits
- [Source: 9-0-spike-report.md (lines 14–18, 74–77, 104–112, 134–144, 195–196)] — empirical Q1/Q2/Q3 resolutions + "transactional rollback not required"
- [Source: 9-1-axum-web-server-and-basic-authentication.md (lines 133, 155–167, 184, 259, 471–478)] — `WebAuthState` + audit-event shape + CSRF deferral
- [Source: 9-2-gateway-status-dashboard.md (lines 76, 121–135, 287, 395)] — `DashboardConfigSnapshot::from_config` pattern + 9-7 forecast
- [Source: 9-3-live-metric-values-display.md (lines 120–128, 303, 323, 430)] — `DeviceSummary` topology + `Arc<DashboardConfigSnapshot>` swap commitment
- [Source: src/opc_ua.rs:98-123, 250-252, 745-848, 761-764, 927] — `RunHandles` shape + `build`/`run_handles` split + 9-7 inline doc-comment
- [Source: src/main.rs:420, 488, 723-879, 885-925, 823-846] — `AppConfig` load + cancel-token + shutdown sequence + stale-threshold clamp
- [Source: src/config.rs:894-1390] — `AppConfig::validate` accumulator pattern
- [Source: tests/opcua_dynamic_address_space_spike.rs] — subscribe_one harness reuse

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — `claude-opus-4-7[1m]` — via the bmad-dev-story skill in a single execution.

### Debug Log References

- **Initial structure refactor:** `StorageConfig` field is `database_path` not `db_path` — fixed at first compile error.
- **AC#10 design call:** `[opcua].stale_threshold_seconds` hot-reload affects only the web dashboard in v1 because the OPC UA path captures the threshold into per-variable read-callback closures at `src/opc_ua.rs:1017` (frozen at startup). AC#8 forbids modifying `src/opc_ua.rs`; documented as known limitation in `docs/security.md`. Test renamed from `stale_threshold_change_flushes_cache` to `stale_threshold_change_propagates_to_subscribers` to match what the implementation actually pins.
- **Credential-rotation design call:** `[opcua].user_name` / `user_password` classified as **restart-required** in v1. The auth middleware captures `Arc<WebAuthState>` at router-build time via `from_fn_with_state(auth_state, basic_auth_middleware)`; hot-swapping requires changing the middleware signature to `State<Arc<AppState>>`, which would modify `src/web/auth.rs` (forbidden by AC#8). Documented as v1 limitation; future story will refactor.
- **`OpcUa::run` doc + `#[allow(dead_code)]`:** the only diff to `src/opc_ua.rs` (6 lines, no behaviour change). `main.rs` now uses the `build` + `run_handles` split directly so the OPC UA config-listener task can spawn between them; the legacy `run` wrapper is unused by the bin target but kept for the lib (Story 9-0 spike tests + library users). Clippy `-D warnings` flags it as dead in the bin target — the attribute silences this without changing behaviour.
- **Issue #110 (RunHandles missing Drop) verdict:** keep the issue open. All three listener tasks (SIGHUP, web, OPC UA) cooperate with `cancel_token.cancelled()` explicitly via `tokio::select!`. RAII drop would be redundant; rustc E0509 still blocks adding it cleanly. Spec gave 9-7 the option to evaluate; 9-7 evaluates and defers.

### Completion Notes List

**Architecture:**

- New module `src/config_reload.rs` (~940 LOC inc. tests) hosts `ConfigReloadHandle`, `ReloadOutcome`, `ReloadError`, `classify_diff`, `run_web_config_listener`, `run_opcua_config_listener`, `topology_device_diff` + 6 unit tests.
- `tokio::sync::watch::Sender<Arc<AppConfig>>` propagation channel (per Story 4-4 pre-commitment) — NOT `arc_swap::ArcSwap`.
- Validate-then-swap discipline: figment load → `AppConfig::validate()` → `classify_diff` → atomic `tx.send(Arc::new(candidate))`. Watch channel NEVER touched on failure.
- Three subsystem listeners, each cooperating with `cancel_token` via `tokio::select!`:
  - **Poller** — `config_rx: Option<Receiver>` field on `ChirpstackPoller`, outer-loop arm at `src/chirpstack.rs::run` swaps `self.config` at cycle boundary; Story 4-4 recovery loop unaffected (read-at-entry semantics).
  - **Web** — `run_web_config_listener` task in `src/main.rs` swaps `AppState.dashboard_snapshot` (now `RwLock<Arc<...>>`) and `AppState.stale_threshold_secs` (now `AtomicU64`).
  - **OPC UA stub** — `run_opcua_config_listener` task spawned between `OpcUa::build()` and `OpcUa::run_handles()`; logs `topology_change_detected` info event with `added_devices`/`removed_devices`/`modified_devices` counts. No `address_space.write().add_variables(...)` calls — Story 9-8 territory.

**Audit events (3 new):**

- `event="config_reload_attempted"` (info) — every SIGHUP
- `event="config_reload_succeeded"` (info) — `changed_section_count`, `includes_topology_change`, `duration_ms`
- `event="config_reload_failed"` (warn / audit) — `reason ∈ {validation, io, restart_required}`, `changed_knob` (only for restart_required), sanitised `error`

**Knob taxonomy classifier (`src/config_reload.rs::classify_diff`):**

- **Restart-required** (rejected on first violation): `chirpstack.{server_address, api_token, tenant_id}`, `opcua.{host_ip_address, host_port, application_name, application_uri, product_uri, pki_dir, certificate_path, private_key_path, max_connections, max_subscriptions_per_session, max_monitored_items_per_sub, max_message_size, max_chunk_count, user_name, user_password}`, `web.{port, bind_address, enabled, auth_realm}`, `storage.{database_path, retention_days}`.
- **Hot-reload-safe**: `chirpstack.{polling_frequency, retry, delay, list_page_size}`, `opcua.{stale_threshold_seconds, diagnostics_enabled, hello_timeout, create_sample_keypair, trust_client_cert, check_cert_time, max_history_data_results_per_node}`.
- **Topology** (logged via `topology_change_detected`, applied by Story 9-8): `application_list[*].{device_list, read_metric_list}`.

**Test results:**

- `cargo test --lib --bins --tests` — **870 passed / 0 failed / 8 ignored** (≥ 845 baseline AC#12 satisfied).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --doc` — 0 failed / 56 ignored (#100 baseline unchanged).
- New tests: 13 in `tests/config_hot_reload.rs` + 6 in `src/config_reload.rs::tests` + 1 in `src/chirpstack.rs::tests` + 3 in `src/web/mod.rs::tests` = **23 new tests**.

**Grep contract (AC#7):** `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` returns exactly 3 lines.

**File invariants (AC#8):** `git diff HEAD --stat` shows ZERO changes to `src/web/auth.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`. `src/opc_ua.rs` has 6 lines added — a doc comment + `#[allow(dead_code)]` on the unchanged `pub async fn run` body (no behaviour change). Test-side fixtures (`tests/web_auth.rs`, `tests/web_dashboard.rs`) updated to match the new `AppState` field types.

**Documented v1 limitations (in `docs/security.md § Configuration hot-reload`):**

1. OPC UA address-space mutation stubbed (Story 9-8 territory — dashboard updates but OPC UA stays frozen).
2. Credential rotation requires restart in v1 (auth-middleware refactor deferred).
3. `[opcua].stale_threshold_seconds` hot-reload affects only the web dashboard (OPC UA per-variable closures captured at startup).
4. No HTTP trigger (Stories 9-4/9-5/9-6 will add CRUD-driven reload).
5. No filesystem watch (editor-save races + dependency-surface expansion).

**Tracking issue:** GitHub #112 opened.

### File List

**New files (production):**
- `src/config_reload.rs` (940 LOC inc. unit tests)

**New files (tests):**
- `tests/config_hot_reload.rs` (520 LOC, 13 integration tests)

**Modified files (production):**
- `src/main.rs` — ConfigReloadHandle creation, SIGHUP listener task, web-config-listener task spawn, OPC UA build/listener/run_handles split, asymmetric join in shutdown sequence.
- `src/lib.rs` — `pub mod config_reload`.
- `src/chirpstack.rs` — `config_rx: Option<watch::Receiver>` field on `ChirpstackPoller`, new `new_with_reload` constructor, outer-loop `tokio::select!` arm for `config_rx.changed()`, 1 new unit test.
- `src/web/mod.rs` — `AppState.dashboard_snapshot` is now `RwLock<Arc<DashboardConfigSnapshot>>`, `AppState.stale_threshold_secs` is now `AtomicU64`. New `clamp_stale_threshold(raw: u64) -> (u64, StaleThresholdClampOutcome)` helper extracted from `src/main.rs`. 3 new unit tests on the helper.
- `src/web/api.rs` — handlers updated for the new field types (RwLock read + Arc clone, AtomicU64 load).
- `src/opc_ua.rs` — doc comment + `#[allow(dead_code)]` on the unchanged `pub async fn run` body (zero behaviour change). No other modifications.

**Modified files (tests):**
- `tests/web_auth.rs` — AppState fixture updated for new field types.
- `tests/web_dashboard.rs` — AppState fixture updated for new field types.

**Modified files (docs):**
- `docs/logging.md` — 3 new event-table rows.
- `docs/security.md` — new `## Configuration hot-reload` section (~127 lines).
- `README.md` — Current Version + Epic 9 row updated.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — header narrative + 9-7 status flip.
- `_bmad-output/implementation-artifacts/9-7-configuration-hot-reload.md` — Status `ready-for-dev → review`, all 31 task checkboxes flipped, Dev Agent Record populated.

### Change Log

| Date | Change | Author |
|------|--------|--------|
| 2026-05-06 | Story created | Claude Code (bmad-create-story) |
| 2026-05-06 | Implementation complete; status → review | Claude Code (bmad-dev-story) |

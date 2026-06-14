# Story F.0: Staged Config with Explicit "Apply Changes"

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **opcgw operator editing configuration from the web UI**,
I want my edits to accumulate as "pending changes" and apply them all at once with an explicit **"Apply changes"** button that performs an **in-process soft restart**,
so that connected OPC UA clients and the poller aren't dropped on every individual save, and applying configuration never restarts the Docker container.

## Context & Critical Findings (read before implementing)

This is the **foundational** story of Epic F ([#140](https://github.com/guycorbaz/opcgw/issues/140)) and the **highest-risk** one. Two facts about the *current* codebase reshape it — both verified in code 2026-06-14:

1. **Today's "restart" is a CONTAINER restart, not in-process.** There is **no** in-process restart loop in `src/main.rs`. The singleton-config editor (`src/web/singleton_config.rs:324`) and the first-run wizard (`src/web/setup.rs:598`) call `state.shutdown_token.cancel()`, which fans out through the shared `CancellationToken` (`src/main.rs:549`), `main()` tears down all tasks and **returns `Ok(())` at `src/main.rs:1498`** → the **process exits** → `docker-compose.yml:21` (`restart: always`) relaunches the whole container. The code says so explicitly (`src/web/setup.rs:14-22`, `src/web/mod.rs:342-348`). **F-0 must BUILD the in-process soft restart** — this is the core of the story, not a detail.

2. **Config changes are NOT uniformly "restart on every save" today.** The current behaviour is mixed:
   - **Device / metric / application / command CRUD already applies LIVE** via `reload_handle.notify_crud_write(...)` (`src/web/api.rs` — e.g. `create_device` at ~line 2270, `create_application` at ~line 1465) → a `tokio::sync::watch<Arc<AppConfig>>` channel (`src/config_reload.rs`). The poller picks it up at the next cycle (`src/chirpstack.rs:~1129`); the OPC UA address space is mutated live (Story 9-8 `apply_diff_to_address_space`, driven by `run_opcua_config_listener` in `src/config_reload.rs`); the web dashboard snapshot is rebuilt (`run_web_config_listener`).
   - **The gRPC uplink event-stream scope is FROZEN at boot** (`src/chirpstack_events.rs:~1089` `streamed_devices(&config)` computed once; deferral documented at `~line 1067`). A newly-added streamed device (valve-class, or with `stream_all_devices=true`) silently never streams until a restart — **this is exactly [#138](https://github.com/guycorbaz/opcgw/issues/138)**.
   - **Only the singleton `[global]/[chirpstack]/[opcua]/[web]` editor + the wizard** do the process-exit/container restart.

**Locked design decision (2026-06-14, Guy):** **unify ALL config changes under staged + Apply.** Every config edit (CRUD via inventory pickers + the singleton editor) becomes a staged "pending change" persisted to SQLite with **no live apply**. The explicit **Apply** performs **one in-process soft restart** of the data-plane (poller, OPC UA server, gRPC event stream, command-timeout handler) that re-reads the full config from SQLite. This:
- gives **one** consistent mental model + a single "pending changes / Apply" affordance;
- **fixes #138** for free (the event-stream task is respawned → re-scopes the device set);
- **removes the restart-required allowlist** (OPC UA endpoint/port/security/PKI now take effect via Apply like everything else);
- makes the existing live-hot-reload path (`notify_crud_write` → watch-channel consumers + 9-7/9-8 live mutation) **dormant** (superseded by the soft restart). F-0 disconnects the live-apply trigger; **full removal of the 9-7/9-8 watch plumbing is an explicit FOLLOW-UP, not F-0** (keep F-0 bounded; see Dev Notes).

**Honest trade-off (accepted):** a soft restart briefly drops OPC UA client sessions + the poller, and device-adds are no longer instant — but it happens **once per batch, operator-initiated** via Apply, and the **container never restarts**.

## Acceptance Criteria

1. **In-process soft restart supervisor.** The data-plane subsystems — ChirpStack poller, OPC UA server, gRPC uplink event-ingestion task, command-timeout handler — run inside a restart loop. On an "apply" signal they are cancelled, **awaited to completion**, and **respawned in-process** after re-reading the full configuration from SQLite, **without the process exiting and without the Docker container restarting**. The embedded web server and the SQLite connection pool **persist** across the cycle (the web server initiates the restart, so it must survive it; the pool's lifecycle is process-level, not per-cycle).

2. **Real shutdown is preserved.** SIGINT (Ctrl+C) and SIGTERM still perform a clean full-process shutdown: the supervisor loop breaks, the connection pool closes (`pool.close()`), and the process exits `Ok(())`. The existing `tests/main_startup_no_deadlock.rs` (subprocess-spawns the binary, asserts the OPC UA port binds within 15 s) still passes unchanged.

3. **Staged config writes (no live apply, no restart).** Every config-write endpoint persists to SQLite and does **NOT** apply the change live and does **NOT** trigger any restart:
   - `PUT /api/config/singleton/<section>` (`src/web/singleton_config.rs`) — the `state.shutdown_token.cancel()` call (`:324`) is **removed** from this path; the response no longer means "restart_pending".
   - all application/device/metric/command CRUD handlers (`src/web/api.rs`) — the `reload_handle.notify_crud_write(...)` live-apply call is **removed** from the per-write path (the SQLite write + duplicate-prevention pre-flight stay).

4. **Pending-changes marker.** A monotonic write-generation counter in shared web state is incremented on **every** successful config write (singleton PUT + every CRUD handler). The gateway exposes whether unapplied changes exist (current generation > last-applied generation) via the status API. After a successful Apply, the marker reports **no** pending changes.

5. **Apply endpoint.** A new `POST /api/config/apply` endpoint (HTTP Basic auth + CSRF, consistent with the singleton PUT surface) triggers the in-process soft restart and returns a response indicating the restart is underway. After it completes, the running subsystems reflect the freshly-read SQLite configuration.

6. **#138 closed + allowlist removed.** Adding a streamed device (valve-class, or any device with `stream_all_devices=true`) via CRUD and then clicking Apply results in the gRPC uplink event stream being **re-scoped to include the new device** (a stream task is spawned for it), verified **without a container restart**. All singleton settings — including the previously restart-required OPC UA endpoint / port / security policy / PKI paths / `[web]` port / `allowed_origins` — take effect via Apply (no special-case "restart-required" labelling remains in the apply path).

7. **Deadlock-safe, leak-free cycling.** The restore-barrier ordering between the poller spawn and `OpcUa::run_handles` — the 2026-05-20 deadlock fix at `src/main.rs:~1067-1077` (poller spawned **before** `restore_barrier.wait()`, OPC UA spawned after) — is preserved in **every** restart iteration (a fresh `Barrier` per cycle). No task or connection leak across cycles: the event-ingestion child tasks (one per streamed device) and the config-listener tasks are all cancelled and awaited on each teardown.

8. **New integration test proves the cycle.** A new integration test (extending the `tests/main_startup_no_deadlock.rs` subprocess pattern) drives an Apply via the web API against a running binary and asserts: (a) the OPC UA port **rebinds** within 15 s after Apply, (b) the process did **not** exit (same PID), and (c) a **second** Apply also succeeds (the loop is re-entrant, not one-shot).

9. **Clean gates + docs.** `cargo test` (incl. the new test) passes 0-fail; `cargo clippy --all-targets -- -D warnings` is clean; `xmllint` clean if the DocBook manual is touched. New audit events (`apply_requested` / `apply_completed` / `apply_failed`, plus a per-write `config_staged` info event) are documented in `docs/logging.md`. `docs/architecture.md` (in-process soft-restart supervisor replacing the container-restart model; the apply contract), `docs/security.md` (singleton editor no longer container-restarts), the DocBook user manual (operator flow: edit → pending → Apply), and `README.md` are updated. The now-dormant live-hot-reload path (`notify_crud_write` + 9-7/9-8 watch consumers) is documented as **superseded** under the unified apply model, with full removal flagged as a follow-up.

## Tasks / Subtasks

- [x] **Task 1 — In-process data-plane restart supervisor in `src/main.rs`** (AC: 1, 2, 7) — DONE
  - [x] Per-cycle `restart_token = cancel_token.child_token()`; data-plane tasks observe it (real SIGINT/SIGTERM cancels the parent → child cancels too; Apply cancels the child directly).
  - [x] Data-plane construction + spawn extracted into `spawn_data_plane()`; the `'supervisor` loop builds a **fresh** `Barrier::new(2)` per cycle and spawns in the deadlock-safe order (poller before `restore_barrier.wait()`, OPC UA after) — preserved every cycle.
  - [x] Web server + `AppState` + connection pool kept OUTSIDE the loop (web server moved above the loop; `web_app_state` clone held for snapshot refresh). Web `tokio::select!` still observes only `cancel_token`.
  - [x] On Apply: re-read config → `restart_token.cancel()` → `join_data_plane()` (bounded 10 s) → `continue 'supervisor`. On SIGINT/SIGTERM: `cancel_token.cancel()` → join → break → `pool.close()` → return. Pool NOT closed between cycles; migrations stay boot-once.
  - **Verified:** `cargo build` clean; `tests/main_startup_no_deadlock.rs` (both tests) pass in 0.63 s — boot path + deadlock-safe ordering intact through the refactor.

- [x] **Task 2 — Re-read configuration from SQLite each cycle** (AC: 1, 6) — DONE
  - [x] New `reload_effective_config()` helper mirrors the boot effective-config load (`from_path_with_sqlite` singleton overlay + SQLite `application_list` fold-in). Called on Apply **before** teardown (bad read → non-disruptive `apply_failed`, current data-plane keeps running).
  - [x] The gRPC event-stream scope is recomputed because `spawn_data_plane` passes the freshly-read `config` to `run_event_ingestion` each cycle (`streamed_devices(&config)`) — the #138 fix mechanism (asserted by the Task 8 test).
  - **Note:** confirmed the in-memory `storage` Mutex is **vestigial** (never passed to poller/OPC UA — they read SQLite directly), so the loop does NOT rebuild storage; SQLite persistence carries values across the soft restart.

- [x] **Task 3 — Pending-changes marker** (AC: 4) — DONE
  - [x] `AppState` gains `pending_gen` / `applied_gen` (`Arc<AtomicU64>`) + `apply_signal` (`Arc<Notify>`), constructed boot-once in `main.rs` and shared with the supervisor; `stage_config_write()` + `has_pending_changes()` helpers added. Supervisor stores `pending_gen → applied_gen` after each (re)spawn.
  - [x] Counter bumped in every config-write endpoint after a successful SQLite write (Tasks 4 + 5).

- [x] **Task 4 — Stage the singleton editor** (AC: 3, 4) — DONE
  - [x] `src/web/singleton_config.rs:~322` now calls `state.stage_config_write("singleton_config")` and returns `{"status":"staged","pending_changes":true}` (202); the `state.shutdown_token.cancel()` call + `"restart_pending"` semantics + `singleton_config_restart_required` event are removed. Emits `config_staged`.

- [x] **Task 5 — Stage the CRUD handlers** (AC: 3, 4) — DONE (with a documented deviation)
  - [x] In `src/web/api.rs`, every CRUD handler now calls `state.stage_config_write("crud")` after the SQLite write (9 handlers: ~1476/1683/1810/2296/2667/2788/3320/3665/3771). C-3 dup-prevention pre-flight + inventory-cache invalidation kept.
  - **Deviation (intentional, documented):** the `notify_crud_write(...)` call was **kept** rather than deleted. It is now a no-op in practice — ALL watch-channel CONSUMERS are dormant (poller built with `config_rx = None` at `spawn_data_plane`; OPC UA 9-8 listener not spawned; `web_config_listener_handle = None`), so it never applies live (AC#3 intent satisfied). The producer is retained only to keep the within-session dup-prevention snapshot fresh; full removal is the F-0-FOLLOWUP. Rationale in `src/config_reload.rs` header doc.

- [x] **Task 6 — Apply endpoint + web→supervisor wiring** (AC: 1, 5) — DONE
  - [x] `POST /api/config/apply` in new module `src/web/apply.rs` (Basic auth + CSRF; `config_apply` CSRF resource bucket added in `src/web/csrf.rs`, emits `config_apply_rejected`). Handler fires `state.apply_signal.notify_one()`, emits `apply_invoked`, returns `202 {"status":"apply_requested"}`. The supervisor (`main.rs`) awaits `apply_signal.notified()`, re-reads config, cancels `restart_token`, joins, respawns; emits `apply_requested`/`apply_completed`/`apply_failed`.
  - [x] `GET /api/status` extended with `pending_changes: bool` (`src/web/api.rs:~58,228`).

- [x] **Task 7 — Dormant live-reload path** (AC: 9) — DONE
  - [x] `src/config_reload.rs` header documents the consumer path as dormant under F-0 (`#![allow(dead_code)]` added). F-0-FOLLOWUP **recorded in `deferred-work.md`** (`## Deferred from: Story F-0 … (2026-06-14)` — full watch-plumbing removal + vestigial `storage` Mutex retirement).

- [x] **Task 8 — Tests** (AC: 2, 6, 8) — DONE; full-suite + clippy green
  - [x] `tests/main_apply_restart.rs` — subprocess: boot → POST apply → OPC UA rebinds, same PID, 2nd apply succeeds. **PASSES (20.8s).** `main_startup_no_deadlock.rs` still green (2 tests, 0.31s).
  - [x] **#138 re-scope integration test — `tests/main_138_rescope.rs` WRITTEN + PASSES (10.4s).** Subprocess boot with `stream_all_devices=false` + a non-valve device → asserts `uplink_ingestion_idle` and NO `uplink_ingestion_start` at boot; stages `PUT /api/config/singleton/chirpstack {"stream_all_devices":true}` via the real staging endpoint; `POST /api/config/apply`; asserts `apply_completed` + `uplink_ingestion_start` appear post-apply and the PID is unchanged. gRPC points at a dead addr — the assertion is on the scope-log line (emitted before any stream connect), not a live stream.
  - [x] Unit/integration (in-process): added to `tests/web_singleton_config.rs` — Test 4 rewritten to stage; new `f0_apply_requires_basic_auth` (401), `f0_apply_requires_csrf` (403 cross-origin), `f0_apply_returns_202_and_fires_signal` (202 + `apply_signal` permit). **Fix applied this session: the 202 test now sends `Content-Type: application/json` (CSRF requires it — the subprocess test and the JS client both send it); without it the request was a 415.** Test 12 doc-sync list updated. `tests/web_device_crud.rs` — new staging test. **All ~16 web_*.rs fixtures got the 3 new AppState fields.**
  - [x] **RAN GREEN:** full `cargo test` (0 failed) + `cargo clippy --all-targets -- -D warnings` (clean), `TMPDIR=/home/gcorbaz/.cache/cargo-tmp`.

- [x] **Task 9 — Docs sync** (AC: 9) — DONE
  - [x] `docs/logging.md` — retired `singleton_config_restart_required`; added the 6-event staged-apply taxonomy (`config_staged`/`apply_invoked`/`apply_requested`/`apply_completed`/`apply_failed`/`config_apply_rejected`) to the event table + a Story F-0 entry in Related stories. (Unblocks Test 12.)
  - [x] `docs/architecture.md` — apply-model note on the snapshot section + new "In-process data-plane restart supervisor (Story F-0)" subsection (spawn ordering preserved every cycle; #138 re-scope mechanism).
  - [x] `docs/security.md` — replaced "restart-required vs hot-reloadable" with "Apply model and restart behaviour (Story F-0)": no container restart, OPC-UA clients reconnect once/batch, Apply is auth+CSRF, bad config non-disruptive, web-login rotation still restart-required.
  - [x] DocBook user manual — "When changes take effect" rewritten to the staged → Apply soft-restart operator flow. `xmllint --noout` clean.
  - [x] `README.md` — Epic F planning row flipped to in-progress with the F-0 implementation-complete summary (closes #138; refs #140).
  - [x] Minimal web UI affordance (Dev Notes "Web UI minimal in F-0"): NEW `static/apply-bar.js` (self-contained pending banner polling `/api/status` + Apply button POSTing `/api/config/apply`), included on `applications`/`devices-config`/`metrics`/`commands`/`singleton-config`. `singleton-config.{html,js}` copy + 202 handling updated from "restart" to "staged".

## Dev Notes

### The architectural shape (most important)

Today `main()` is **linear**: build everything once (`:300-1415`) → `tokio::select!` on SIGINT/SIGTERM (`:1420-1427`) → `cancel_token.cancel()` (`:1430`) → bounded join (`:1450-1490`) → `pool.close()` (`:1493`) → `return Ok(())` (`:1498`). F-0 turns the **data-plane** half into a loop while leaving the **process-level** half (web server, pool, signal handling, real shutdown) as the outer scope.

Recommended structure:
- **Outer (process lifetime):** parse config, init logging, open pool, run migrations, build web `AppState`, spawn the web server (observes `cancel_token` only). Then enter the supervisor loop. After the loop, `pool.close()` + return.
- **Supervisor loop (per apply cycle):** re-read `AppConfig` from SQLite; fresh `Barrier::new(2)`; spawn poller (before barrier wait), `restore_barrier.wait()`, spawn `OpcUa::run_handles` (after), spawn events + timeout; `tokio::select!` on `{ restart_token.cancelled() → teardown+continue, cancel_token.cancelled() → teardown+break }`; on teardown, cancel the per-cycle token, await all data-plane handles (bounded by the existing 10 s timeout), mint a fresh `restart_token`.

`OpcUa::build()` + `OpcUa::run_handles()` were deliberately split (Story 9-0) to be re-invokable — reuse them per cycle. The poller is `chirpstack_poller.run()`; the events task is `chirpstack_events::run_event_ingestion(config, backend, token)`.

### ⚠️ Deadlock zone — read `incident_main_deadlock_2026_05_20`

`src/main.rs:~1067-1077` is the exact code that caused a **30-day-latent structural deadlock** (commit `c510814`, fixed `917d634`): the poller MUST be spawned **before** `restore_barrier.wait()`, and `OpcUa::run_handles` spawned **after**. That ordering must hold **on every loop iteration**. The session-count gauge spawns inside `OpcUa::build()` and fires regardless of the deadlock, so it is NOT a valid liveness signal — **only** the subprocess-spawn TCP-bind test (`tests/main_startup_no_deadlock.rs`) catches this class of bug. The new Apply-restart test (Task 8) is the equivalent guard for the cycle path and is **mandatory** before this story is `done` (cf. CLAUDE.md real-world-validation doctrine).

### Tokens — do not confuse the two

- `cancel_token` (`:549`) = **process-wide shutdown**, shared with `AppState.shutdown_token` (`src/web/mod.rs:342`) and every task. Cancelling it must still mean "exit the process." Do not repurpose it for apply.
- `restart_token` (new) = **per-cycle data-plane teardown**. Apply fires this; the supervisor mints a fresh one each iteration. The web server never observes it.

### Staging vs. the existing live-reload plumbing

`src/config_reload.rs` (`ConfigReloadHandle`, the `watch<Arc<AppConfig>>` channel, `notify_crud_write`, `seed_post_overlay`) and the 9-7/9-8 listeners are the **live** apply path. Under the unified F-0 model the **soft restart is the only apply path**, so these go dormant once Task 5 removes the `notify_crud_write` senders. Keep them compiled-but-idle in F-0 (removing them touches the poller's `config_rx` subscription at `:959`, both listener spawns at `:1036-1050` and `:1386-1388`, and the 9-8 `apply_diff_to_address_space` machinery — too much surface for F-0). Log an F-0-FOLLOWUP in `deferred-work.md`.

Note: `seed_post_overlay` (`:751`) and the boot-time CRUD reseed (`:843` `notify_crud_write`) are part of the **boot** overlay sequence — leave the boot path intact; only the **per-write** live-apply calls in the request handlers are removed.

### CSRF / auth surface

The singleton editor already wires a `singleton_config` CSRF resource bucket and Basic-auth (Story D-1). Mirror it for `POST /api/config/apply` (`config_apply` bucket) — see `src/web/csrf.rs` reject-arm pattern. Per-write endpoints keep their existing auth/CSRF; only their post-write side effect changes.

### Web UI (minimal in F-0)

F-0 is primarily backend. A **minimal** "pending changes / Apply" affordance is enough (a banner + button hitting `/api/config/apply`, reading the pending flag from `/api/status`). The polished, shell-integrated version is **F-1**'s job — don't build the shell here. Vanilla JS, no build step (project rule — see existing `static/*.js`).

### Testing standards

- Rust 2021, `cargo test` (lib + bins + integration under `tests/`), `cargo clippy --all-targets -- -D warnings`. Integration tests that exercise `main()` end-to-end use the **subprocess-spawn** pattern (`tests/main_startup_no_deadlock.rs`) — assert real TCP binds, never trust in-process gauges. DocBook changes validated with `xmllint --noout --valid`.
- Watch for test flakes from parallel env-var/port use (precedent: `temp_env::with_var` isolation in `src/opc_ua.rs` tests). The Apply-restart test must use a unique OPC UA port + its own temp SQLite/`secrets.toml`.

### Project Structure Notes

- Touch: `src/main.rs` (supervisor loop), `src/web/mod.rs` (pending-gen state + apply signal handle on `AppState`), `src/web/singleton_config.rs` (Task 4), `src/web/api.rs` (Tasks 5 + 6), `src/web/csrf.rs` (apply bucket), `static/` (minimal banner/button), `tests/` (new integration tests), `docs/*`, `README.md`.
- Leave dormant but present: `src/config_reload.rs`, `src/opcua_topology_apply.rs` (9-8). Record their retirement as F-0-FOLLOWUP in `deferred-work.md`.
- No new crate dependencies expected (tokio `CancellationToken` / `Notify` / `mpsc` already in use).
- SPDX headers `(c) [2024] Guy Corbaz`, MIT OR Apache-2.0 on any new file (project convention). Keep source files < 5000 lines (`feedback_source_file_size`); `src/web/api.rs` is already large (~233 KB) — if the apply handler + pending state would bloat it, place them in a small new `src/web/apply.rs` module.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Epic F → Story F.0]
- [Source: src/main.rs:549 (cancel_token), :591 (restore_barrier), :738 (from_path_with_sqlite), :953-959 (poller build + config_rx), :1012-1078 (OpcUa build/run_handles + barrier ordering), :1104 (timeout), :1126-1131 (events), :1420-1498 (select + teardown + return)]
- [Source: src/web/singleton_config.rs:120-328 (PUT handler, :324 shutdown_token.cancel)]
- [Source: src/web/api.rs (CRUD handlers; ~1465 create_application notify_crud_write, ~2270 create_device notify_crud_write); GET /api/status]
- [Source: src/web/mod.rs:342-348 (shutdown_token), :264 (auth restart-required note)]
- [Source: src/web/setup.rs:14-22, :594-598 (wizard supervisor-restart comments — F-2 territory, left as-is in F-0)]
- [Source: src/config_reload.rs (ConfigReloadHandle API, watch channel, notify_crud_write, listeners)]
- [Source: src/chirpstack_events.rs:~1067-1124 (streamed_devices computed once at boot — #138 root cause)]
- [Source: docker-compose.yml:21 (restart: always — proves container-restart model today)]
- [Source: GitHub #140 (Epic F), #138 (closed by F-0)]
- [Source: memory incident_main_deadlock_2026_05_20 (main.rs spawn-ordering deadlock; subprocess-spawn test is the only guard)]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (bmad-dev-story)

### Debug Log References

- `cargo build` clean after the supervisor refactor (only dead-code warnings for the not-yet-wired `stage_config_write`/`has_pending_changes`/`pending_gen`).
- `cargo test --test main_startup_no_deadlock` → 2 passed (0.63 s): boot path + 2026-05-20 deadlock-safe spawn ordering verified through the refactor.

### Completion Notes List

- **Phase 1–2 done (the high-risk core):** built the in-process restart supervisor. `main.rs` now: boot-once setup → spawn persistent web server (moved above the loop) → `'supervisor` loop { refresh web snapshot; fresh `child_token` + `Barrier`; `spawn_data_plane()`; `applied_gen = pending_gen`; select(SIGINT/SIGTERM → exit | apply_signal → re-read config, cancel child, `join_data_plane`, respawn) } → await web → `pool.close()`. New helpers: `DataPlaneHandles`, `spawn_data_plane()`, `join_data_plane()`, `reload_effective_config()`. The container is never restarted; `cancel_token` remains the only real-exit path.
- Discovered + recorded: the in-memory `storage` Mutex is vestigial (data-plane reads SQLite directly), simplifying the loop. Web-login credential rotation stays restart-required (web server persists across the soft restart) — to be documented in Task 9.
- **Implementation complete (2026-06-14, all 9 tasks):** Tasks 8 + 9 finished this session. Added the #138 re-scope subprocess test (`tests/main_138_rescope.rs`, passes); fixed the in-process `f0_apply_returns_202_and_fires_signal` test (missing `Content-Type: application/json` → CSRF 415); documented the full staged-apply audit taxonomy across `docs/logging.md` / `architecture.md` / `security.md` / DocBook manual / `README.md`; recorded the F-0-FOLLOWUP in `deferred-work.md`; built the minimal `apply-bar.js` pending/Apply affordance and re-pointed the singleton editor copy from "restart" to "staged". **Gates green: full `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean, `xmllint --noout` clean.** Status flipped `in-progress` → `review`. NEXT: `bmad-code-review F-0` (foundational/highest-risk — `main.rs` deadlock zone; iter-N+1 mandatory).

### File List

**Implementation (pre-existing this story):**

- `src/main.rs` — supervisor loop + `spawn_data_plane`/`join_data_plane`/`reload_effective_config` helpers; removed boot-once `restore_barrier`; data-plane block extracted; web server moved above the loop; `pending_gen`/`applied_gen`/`apply_signal` created boot-once; poller built with `config_rx = None`; 9-8 OPC UA listener + web-config listener no longer spawned (handle = `None`).
- `src/web/mod.rs` — `AppState`: `pending_gen` / `applied_gen` / `apply_signal` fields + `stage_config_write()` / `has_pending_changes()`; `pub mod apply`; `/api/config/apply` route.
- `src/web/apply.rs` — NEW: `api_config_apply` handler (fires `apply_signal`, 202, `apply_invoked`).
- `src/web/singleton_config.rs` — PUT stages instead of restarting (Task 4).
- `src/web/api.rs` — CRUD handlers call `stage_config_write("crud")`; `/api/status` gains `pending_changes`.
- `src/web/csrf.rs` — `config_apply` CSRF resource bucket + reject arm.
- `src/config_reload.rs` — header doc: consumer path dormant under F-0; `#![allow(dead_code)]`.
- `tests/main_apply_restart.rs` — NEW: subprocess apply-cycle test (PASSES).
- `tests/web_singleton_config.rs` — Test 4 rewritten + 3 new apply tests + Test 12 doc-sync list updated + fixture exposes `app_state`.
- `tests/web_device_crud.rs` — new staging test + fixture exposes `app_state`.
- `tests/web_*.rs` (≈14 files) — fixtures updated with the 3 new `AppState` fields.

**Added to complete the story (2026-06-14):**

- `tests/main_138_rescope.rs` — NEW: #138 stream re-scope subprocess test (idle-at-boot → stage `stream_all_devices=true` → Apply → `uplink_ingestion_start`; PASSES).
- `tests/web_singleton_config.rs` — `f0_apply_returns_202_and_fires_signal` fixed to send `Content-Type: application/json`.
- `static/apply-bar.js` — NEW: minimal pending-changes banner + "Apply changes" button (polls `/api/status`, POSTs `/api/config/apply`).
- `static/applications.html`, `static/devices-config.html`, `static/metrics.html`, `static/commands.html` — include `apply-bar.js`.
- `static/singleton-config.html`, `static/singleton-config.js` — copy + 202 handling re-pointed from "restart" to "staged"; include `apply-bar.js`.
- `docs/logging.md`, `docs/architecture.md`, `docs/security.md`, `docs/manual/opcgw-user-manual.xml`, `README.md` — staged-apply model documented (Task 9).
- `_bmad-output/implementation-artifacts/deferred-work.md` — F-0-FOLLOWUP entries.

### NEXT (story now `review`)

1. `bmad-code-review F-0` on a different LLM (foundational/highest-risk story — `main.rs:~1067-1077` deadlock zone; iter-N+1 mandatory per CLAUDE.md doctrine).
2. Implementation-Complete commit lands BEFORE any review-fix commit (BMad discipline). Closes #138; refs #140.

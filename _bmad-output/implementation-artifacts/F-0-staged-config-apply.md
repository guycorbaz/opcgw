# Story F.0: Staged Config with Explicit "Apply Changes"

Status: ready-for-dev

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

- [ ] **Task 1 — In-process data-plane restart supervisor in `src/main.rs`** (AC: 1, 2, 7)
  - [ ] Introduce a per-cycle `restart_token: CancellationToken` distinct from the process-wide `cancel_token` (`:549`). The data-plane tasks observe a `tokio::select!` over **both** (`restart_token.cancelled()` → cycle; `cancel_token.cancelled()` → real shutdown).
  - [ ] Wrap the data-plane construction + spawn block (config re-read at `~:738`; poller build `~:953`; `OpcUa::build`/`run_handles` `~:1012-1078`; events `~:1126`; timeout `~:1104`; the restore-barrier ordering at `~:1067-1077`) in a **restart loop**. Each iteration: re-read config from SQLite, build a **fresh** `Barrier::new(2)`, spawn in the deadlock-safe order, then await the select.
  - [ ] Keep the **web server**, its `AppState`, and the **connection pool** OUTSIDE the loop (constructed once; survive every cycle). The web server's `tokio::select!` continues to observe only `cancel_token` (real shutdown).
  - [ ] On `restart_token` fire: cancel data-plane → await all data-plane handles (reuse the existing asymmetric join shape at `:1450-1490`, but bounded + then `continue` the loop) → mint a fresh `restart_token` → loop. On `cancel_token` fire: break loop → `pool.close()` → return.
  - [ ] **Do NOT** call `pool.close()` between cycles. Do NOT re-run schema migration between cycles (migration is boot-once).

- [ ] **Task 2 — Re-read configuration from SQLite each cycle** (AC: 1, 6)
  - [ ] Reuse `AppConfig::from_path_with_sqlite(...)` (the boot path at `src/main.rs:738`) inside the loop so new singleton values + the new device set are picked up. (Secrets still come from `secrets.toml`/env via the figment stack — unchanged.)
  - [ ] Confirm the gRPC event-stream scope is recomputed from the freshly-read config each cycle (`streamed_devices(&config)` in `src/chirpstack_events.rs`) — this is what closes #138.

- [ ] **Task 3 — Pending-changes marker** (AC: 4)
  - [ ] Add a monotonic generation counter to shared web state (e.g. `AtomicU64` on `AppState` / a small shared struct in `src/web/mod.rs`) plus a "last-applied" snapshot. `pending = current > applied`.
  - [ ] Bump the counter in every config-write endpoint after a successful SQLite write (Tasks 4 + 5). Reset `applied = current` after a successful Apply (Task 6).

- [ ] **Task 4 — Stage the singleton editor** (AC: 3, 4)
  - [ ] In `src/web/singleton_config.rs`, remove `state.shutdown_token.cancel()` (`:324`) and the `"restart_pending"` semantics; after the SQLite `write_singleton_section`, bump the pending-gen and return a "staged" response. Emit `config_staged`.

- [ ] **Task 5 — Stage the CRUD handlers** (AC: 3, 4)
  - [ ] In `src/web/api.rs`, in every application/device/metric/command create/update/delete handler, remove the `reload_handle.notify_crud_write(...)` live-apply call; keep the SQLite write + the C-3 duplicate-prevention pre-flight + inventory-cache invalidation. Bump the pending-gen; emit `config_staged`.

- [ ] **Task 6 — Apply endpoint + web→supervisor wiring** (AC: 1, 5)
  - [ ] Add `POST /api/config/apply` (Basic auth + CSRF; add a `config_apply` CSRF resource bucket mirroring `singleton_config`). Handler signals the supervisor to cycle (e.g. fire the shared `restart_token`, or send on an `mpsc`/`Notify` the supervisor awaits), emits `apply_requested`, resets the applied-gen on success, returns the underway/awaited result. Emit `apply_completed` / `apply_failed`.
  - [ ] Expose pending status — extend `GET /api/status` (`src/web/api.rs`) with a `pending_changes: bool` (+ optional count) field, or add `GET /api/config/pending`. Prefer extending `/api/status` (the dashboard already polls it).

- [ ] **Task 7 — Dormant live-reload path** (AC: 9)
  - [ ] With Task 5 removing the only `notify_crud_write` senders, the watch-channel consumers (`run_web_config_listener`, `run_opcua_config_listener` / `apply_diff_to_address_space`, the poller `config_rx` pickup) receive no sends and idle. Leave the modules in place; add a short doc-comment noting they are superseded by the F-0 apply model. **Do not rip out 9-7/9-8 in F-0** — record "retire watch-channel live-reload + 9-8 live mutation" in `deferred-work.md` as an F-0-FOLLOWUP.

- [ ] **Task 8 — Tests** (AC: 2, 6, 8)
  - [ ] New integration test (subprocess pattern from `tests/main_startup_no_deadlock.rs`): boot binary → POST `/api/config/apply` → assert OPC UA port rebinds within 15 s, same PID, second Apply succeeds.
  - [ ] Integration test: stage a streamed device via CRUD (no live effect) → Apply → assert the event stream now covers it (closes #138). 
  - [ ] Unit/integration: singleton PUT + a CRUD write each bump pending-gen and do NOT apply live / do NOT cancel a token; Apply resets the marker; `/api/status` (or `/api/config/pending`) reports the marker; CSRF/auth on the apply endpoint.
  - [ ] Confirm `tests/main_startup_no_deadlock.rs` and the full suite stay green; clippy `-D warnings` clean.

- [ ] **Task 9 — Docs sync** (AC: 9)
  - [ ] `docs/architecture.md` (apply model + in-process restart supervisor; the three persistence surfaces unchanged), `docs/security.md` (singleton editor no longer container-restarts; Apply soft-restart drops/reaccepts OPC UA clients once per batch), `docs/logging.md` (`config_staged`, `apply_requested`, `apply_completed`, `apply_failed`), DocBook user manual (operator: edit → pending banner → Apply), `README.md` (Planning row / behaviour note). Reference [#140](https://github.com/guycorbaz/opcgw/issues/140) + closes [#138](https://github.com/guycorbaz/opcgw/issues/138).

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

(to be filled by the dev agent)

### Debug Log References

### Completion Notes List

### File List

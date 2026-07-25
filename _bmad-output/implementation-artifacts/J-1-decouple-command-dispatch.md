# Story J.1: Decouple Command Dispatch from the Metrics Poll Loop

Status: review

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator issuing an OPC UA command to a LoRaWAN device**,
I want the gateway to enqueue that downlink to ChirpStack **immediately** instead of on the next metrics-poll cycle,
so that a command isn't stranded for up to a full `chirpstack.polling_frequency` interval and doesn't read as a failure.

GitHub issue: **CR #136** (Epic J "Config Authority & Command Responsiveness", target **v2.8.0**). Second story of Epic J. **DATA-PLANE story** — unlike J-0 (zero data-plane risk), this touches the command send path, so it requires the real-binary / NAS soak before promotion to `done` (AI-G-5 doctrine — see Dev Notes § Promotion Gate).

**The defect (premise re-verified 2026-07-22):** `poll_metrics` calls `process_command_queue()` at its very head (`src/chirpstack.rs:1443`). Command delivery therefore only runs once per poll cycle. An OPC UA `set_command` write persists a `Pending` row (`src/opc_ua.rs:2173 queue_command`) and returns `Good` **immediately**, but the downlink does not reach ChirpStack's queue until the poller's next tick — up to `chirpstack.polling_frequency` seconds later (default poll cadence, config-driven). **Field observation (E-0 AC#10 valve test):** the operator wrote a valve command, watched an empty ChirpStack device queue for a whole cycle, and read the empty queue as a delivery failure.

**Architectural driver:** since Story E-1 (#130) the live data path is `StreamDeviceEvents` and the metrics poll is demoted (streamed devices are skipped in `poll_metrics`, `src/chirpstack.rs:1474`) and may be retired to backfill-only. Commands must therefore **not** hang off the poll loop at all.

**Design (CR #136 preferred shape (a) — LOCKED):** event-driven. `OpcUa::set_command` signals a shared `tokio::sync::Notify` on a successful enqueue; a **dedicated command-dispatch task** awaits that `Notify` and drains the pending-command queue. The metrics poll stops delivering commands entirely.

## Acceptance Criteria

1. **Immediate dispatch.** After an OPC UA `set_command` write returns `StatusCode::Good`, the resulting `Pending` command is delivered to ChirpStack without waiting for the metrics-poll cycle — driven by a `tokio::sync::Notify` the write path fires and a dedicated dispatch task awaits. **No `sleep`/interval gates the command→enqueue latency.**

2. **Signal only on successful enqueue.** The dispatch signal (`Notify::notify_one`) fires **only** when `set_command` successfully queued the command (returns `Good`). Every early-return path (`Bad`, `BadTypeMismatch`, `BadOutOfRange`, `BadInternalError` at `src/opc_ua.rs:2098–2181`) fires **nothing** — a rejected write must not wake the dispatcher. `Notify::notify_one` is sync and callable from the sync OPC UA write callback; **do not** make the callback async.

3. **Poll loop no longer delivers commands.** `self.process_command_queue().await?` is **removed** from the head of `poll_metrics` (`src/chirpstack.rs:1442–1443`). After this story the metrics poll performs no command delivery. A grep for `process_command_queue` in the poll path returns nothing; the method (or its body) lives only on the dispatcher.

4. **Single-owner delivery — no double-send.** Exactly one task delivers commands (the dispatcher). `get_pending_commands()` returns rows by `status = 'Pending'` and `deliver_one` only marks `Sent` **after** the enqueue succeeds (`src/chirpstack.rs:2818–2844`), so two concurrent drainers would each read the same `Pending` row and enqueue the **same downlink twice**. The poller must not also drain (AC#3 guarantees this) and the dispatcher is a single task, so its own drains are serialized. **No duplicate downlink is possible.**

5. **Startup / respawn drain.** On task start the dispatcher performs one drain **before** first awaiting the signal, so commands persisted `Pending` before boot (or carried across a soft-restart / crash) are delivered without needing a fresh OPC UA write to wake it. (Today the poll-head call covered this; the dispatcher must preserve it.)

6. **No lost wakeup (coalescing).** The dispatcher drains **all** currently-pending rows each cycle, then awaits `notified()`. `Notify` stores a single permit, so a command enqueued while a drain is in flight makes the subsequent `notified()` return immediately → re-drain. Because each drain empties the queue, one wakeup covering N signals delivers all N. A command enqueued between "queue read empty" and "await notified" is covered by the stored permit. **Assert this** with the AC#10(c) burst test.

7. **Graceful shutdown & fresh state on Apply.** The dispatcher's loop is `tokio::select!` over `cancel_token.cancelled()` (returns `Ok(())`, logs `"…dispatcher shutting down"`) and `dispatch_signal.notified()`, mirroring `CommandStatusPoller::run` (`src/chirpstack.rs:3108–3114`). It is spawned inside `spawn_data_plane` under the per-cycle `restart_token`, so a real SIGINT/SIGTERM and an Apply-respawn both stop it; the respawn creates a **fresh** `Notify` shared between the new `OpcUa` and the new dispatcher (no stale cross-generation signalling).

8. **Shared gRPC client factory — no duplicated auth logic, no token leak.** The dispatcher needs a `DeviceServiceClient` to enqueue downlinks. Extract `create_channel` + `create_interceptor` + `create_device_client` (currently inherent `ChirpstackPoller` methods at `src/chirpstack.rs:594 / 727 / 795`, config-only: they read exactly `config.chirpstack.server_address` and `config.chirpstack.api_token`) into config-parameterized free functions (or a small `ChirpStackClientFactory`) in `src/chirpstack.rs`, used by **both** the poller and the dispatcher. Reuse `normalize_chirpstack_endpoint`, the `AuthInterceptor` (`src/chirpstack.rs:121`), the `#148` port-defaulting, and the 5 s connect timeout — do **not** hand-roll a second connect path. `api_token` must never appear in a log line.

9. **Reuse existing delivery machinery — no reinvention.** Delivery goes through the existing `deliver_one` free fn (`src/chirpstack.rs:2791`), `find_command_cfg` (`:2871`), `build_queue_item`, `map_command_to_downlink`, and the `DownlinkSink` trait (`:2743`). The dispatcher implements `DownlinkSink` via the extracted client factory (mirroring the poller's impl at `:2753`). The Story 3-1 high-level `Command` / `dequeue_command` FIFO (`src/storage/sqlite.rs:1990`) has no OPC UA producer and is **left untouched**. No new `StorageBackend` method, no schema change, **no new config knob** (the design is purely event-driven — there is nothing to configure).

10. **Tests.** All new tests use the existing `MockSink` stub (`src/chirpstack.rs:4549`) — no live gRPC. Full `cargo test` 0-fail; `cargo clippy --all-targets -- -D warnings` clean.
    - (a) **Dispatch delivers a queued command:** enqueue one `Pending` `DeviceCommand`, signal the dispatcher once, assert the `MockSink` received exactly one downlink and the row transitioned `Pending → Sent`.
    - (b) **Startup drain (AC#5):** with a `Pending` row already present, the dispatcher's first pass (before any signal) delivers it.
    - (c) **Burst coalescing (AC#6):** enqueue 3 `Pending` rows, fire the signal (1 or many times), assert **all 3** are delivered `Sent` and none is delivered twice (assert `MockSink.calls().len() == 3`).
    - (d) **No delivery on the poll path (AC#3 regression guard):** a `poll_metrics` run (or a direct assertion that the poll head no longer calls the queue) does **not** deliver a `Pending` command. Guard must fail on the pre-J-1 code where the poll head drained the queue — i.e. drive the actual `poll_metrics` / poll body, not a hand-rolled loop (fake-regression-guard class, see [[feedback_fake_regression_guard_tests]]).
    - (e) **Signal-on-Good only (AC#2):** a `set_command` call that returns a `Bad*` status (e.g. non-numeric variant → `BadTypeMismatch`) does **not** fire the `Notify` (assert via a test-visible signal counter or that a waiting `notified()` stays pending). A `Good` write fires exactly once.
    - (f) **Graceful shutdown (AC#7):** cancelling the token makes `run()` return `Ok(())` promptly while blocked on the signal.
    - (g) **Single-owner / no double-send (AC#4):** two drains over the same backend snapshot (or the drain plus a simulated concurrent poll-head call on pre-J-1 code) never enqueue the same command id twice.

11. **Docs synced (same commit, per CLAUDE.md).**
    - `docs/logging.md` — add the command-dispatch event rows: dispatcher start, per-drain `debug!(count=…)`, and shutdown. (Note: `tests/web_singleton_config.rs:715 d1_audit_event_names_documented_in_logging_md` only checks a **fixed list of 12 config/apply audit events** — it will NOT trip on a missing dispatcher row, so no test enforces this. Doc-sync is still mandatory per CLAUDE.md; just don't rely on a test to catch a miss.)
    - `docs/manual/latex/body.tex` — update the command/downlink section to state commands are dispatched immediately on write (event-driven), not on the poll cycle. **LaTeX is the canonical manual** (`docs/manual/README.md` — DocBook XML retired 2026-06-27, #145). **Do not edit `docs/manual/opcgw-user-manual.xml`** (dead artifact still in git). No PDF rebuild required for this commit.
    - `README.md` — reflect the responsiveness improvement (commands no longer wait a poll cycle); update the Planning section so the Epic J / Story J-1 row mirrors `sprint-status.yaml`.
    - `CHANGELOG.md` — under `[Unreleased]` / `2.8.0`.
    - Commit message references the issue: **`Closes #136`** (this story fully delivers CR #136's ask).

**OUT OF SCOPE:** retiring the metrics poll to backfill-only (a separate future decision noted in the epic); any change to `Command`/`dequeue_command` FIFO (Story 3-1); any new configuration knob; command *confirmation* / timeout behaviour (owned by `CommandStatusPoller` + `CommandTimeoutHandler`, unchanged); the `#137` multi-manufacturer class registry.

## Tasks / Subtasks

- [x] **Task 1 — Extract a shared ChirpStack device-client factory.** (AC: 8, 9)
  - [x] In `src/chirpstack.rs`, extract `create_channel` (`:594`), `create_interceptor` (`:727`), and `create_device_client` (`:795`) into config-parameterized free functions (or a small `ChirpStackClientFactory { config: AppConfig }`) — they depend only on `config.chirpstack.{server_address, api_token}`. Preserve `normalize_chirpstack_endpoint`, `#148` port defaulting, the empty-address guard, the `chirpstack_connect` structured logs, and the `CHANNEL_CONNECT_TIMEOUT_SECS = 5` timeout **verbatim**.
  - [x] Re-point `ChirpstackPoller`'s existing callers (`create_device_client` used at `:2251, 2520, 2762`; `create_application_client`) at the extracted factory so there is exactly ONE connect path. Weaken no existing behaviour; `api_token` stays out of every log line.
  - [x] Confirm `cargo test` + `clippy` green after the pure refactor **before** adding the new task (keeps the diff bisectable).

- [x] **Task 2 — Add the `CommandDispatcher` task.** (AC: 1, 4, 5, 6, 7, 9)
  - [x] New `pub struct CommandDispatcher` in `src/chirpstack.rs`, alongside `CommandStatusPoller` (`:3035`) and `CommandTimeoutHandler` (`:3124`). Fields: `config: AppConfig`, `backend: Arc<dyn StorageBackend>`, `cancel_token: CancellationToken`, `dispatch_signal: Arc<tokio::sync::Notify>`. `new(config, backend, cancel_token, dispatch_signal)` mirrors the sibling constructors.
  - [x] Implement `DownlinkSink` for `CommandDispatcher` (`enqueue_downlink`) using the Task 1 factory — same body shape as the poller's impl (`:2753–2781`): build `EnqueueDeviceQueueItemRequest`, call `device_client.enqueue`, return the queue-item id (or the empty-id warn fallback).
  - [x] Factor the drain out of `process_command_queue` (`:2581`) into a form the dispatcher can call: `get_pending_commands().await?` → for each, resolve `(command_class, confirmed)` via `find_command_cfg` (`:2871`) and call `deliver_one(&self, &self.backend, class, confirmed, &cmd)`. The poller currently does this via `deliver_command` (`:2613`); reuse the same helpers so both paths share one delivery implementation (do **not** duplicate the mapping/enqueue/status logic).
  - [x] `run(&mut self)`: (1) one **startup drain** (AC#5); (2) `loop { tokio::select! { _ = cancel_token.cancelled() => { info!("CommandDispatcher shutting down"); return Ok(()); } _ = dispatch_signal.notified() => { drain_all().await; } } }`. Each `drain_all` empties the queue (AC#6). Per-command failures are logged + reflected in status and never abort the drain (existing `deliver_one` contract); a storage-lock failure on `get_pending_commands` is logged and the drain retries on the next signal (do NOT panic/return — a poisoned drain would strand all future commands).

- [x] **Task 3 — Thread the dispatch signal from the write path.** (AC: 1, 2, 7)
  - [x] Add `dispatch_signal: Arc<tokio::sync::Notify>` to the `OpcUa` struct (`src/opc_ua.rs:159`) and a param to `OpcUa::new` (`:208`). Store it so the address-space builder can clone it into each command write callback.
  - [x] **Fix ALL `OpcUa::new` callers** — the new param breaks the build until every call site is updated, and `cargo test` (AC#10) cannot even compile otherwise. Production caller: `src/main.rs:383`. **8 test call sites across 6 files** also break — pass a throwaway `Arc::new(tokio::sync::Notify::new())`: `tests/opc_ua_security_endpoints.rs:229`, `tests/opcua_history.rs:196` & `:919`, `tests/opcua_dynamic_address_space_apply.rs:239`, `tests/opcua_subscription_spike.rs:287` & `:1410`, `tests/opcua_dynamic_address_space_spike.rs:236`, `tests/opc_ua_connection_limit.rs:250`. Also fix the stale 2-arg doc-comment example at `src/opc_ua.rs:300` (`OpcUa::new(&config, storage)` → 3-arg form). All 6 test files go in the File List.
  - [x] At the command write-callback site (`src/opc_ua.rs:1192–1202`): clone the signal into the closure; after `Self::set_command(...)` returns, fire `signal.notify_one()` **iff** the returned `StatusCode` is `Good` (AC#2 — capturing the returned status covers every early-return path `2097–2181` uniformly). Keep `set_command`'s signature stable (do not thread the Notify into `set_command` itself — signalling on the returned `Good` keeps the fn pure/testable and covers both callers uniformly).
  - [x] Second write-callback site `src/opcua_topology_apply.rs:679–683` (Story 9-8 live-mutation apply — currently **dormant** under F-0's staged-apply model, config-listener not spawned): thread the same signal-on-`Good` there too, so the path is correct if ever reactivated. If plumbing the signal into `apply_diff` is disproportionate for a dormant path, leave a `// J-1:` comment marking it and note the decision in Completion Notes — but the live path (Task 3 bullet 2) is mandatory.

- [x] **Task 4 — Spawn the dispatcher; remove command delivery from the poll loop.** (AC: 1, 3, 4, 7)
  - [x] In `spawn_data_plane` (`src/main.rs:339`): create `let dispatch_signal = Arc::new(tokio::sync::Notify::new());` **per cycle** (fresh each respawn, AC#7). Pass a clone into `OpcUa::new` (Task 3) and into a new `CommandDispatcher` task spawned under `restart_token` with its own `SqliteBackend::with_pool(pool.clone())` — copy the shape of the `cmd_status` / `cmd_timeout` spawns (`:402–442`).
  - [x] Add `cmd_dispatch: JoinHandle<()>` to the `DataPlaneHandles` **definition** (`src/main.rs:285`) and its construction (`:460`); include it in `join_data_plane` (`:474`) — bump the task array (`:482`, currently `[;5]`) to 6 and add its abort handle.
  - [x] **Remove** `self.process_command_queue().await?;` from `poll_metrics` (`src/chirpstack.rs:1442–1443`). Leave the delivery helpers (`deliver_command`, `deliver_one`, `find_command_cfg`) in place — the dispatcher uses them. If `process_command_queue` itself becomes unused after Task 2 relocates the drain, remove it (and its now-orphaned test scaffolding references) rather than leaving dead code.
  - [x] **Connection-pool sizing (keep the documented invariant honest, not a starvation fix):** the pool is created at `src/main.rs:792` (`ConnectionPool::new("data/opcgw.db", 5)`). Connections are per-op RAII checkouts (`ConnectionPool::checkout` → `ConnectionGuard`, `src/storage/pool.rs:363`), not lifetime-pinned, and the dispatcher checks out only briefly per drain — so a bump is precautionary, not correctness-required. **But the comment at `:787–791` is already stale**: it counts `5 = poller + opc_ua + command-status + command-timeout + web` and omits the E-1 `events` claimer (`:447`). Bump the size and rewrite the comment to include BOTH the `events` claimer and the new dispatcher, so the documented count matches reality.

- [x] **Task 5 — Tests.** (AC: 10) Implement (a)–(g) from AC#10 in `src/chirpstack.rs` `#[cfg(test)]` using `MockSink` (`:4550`) and `InMemoryBackend`. For (d)/(g) drive the real drain/poll code paths, not hand-rolled loops. **Anti-example:** the existing `deliver_batch_continues_past_a_failure` test (`src/chirpstack.rs:4820`) opens with `// Mirrors process_command_queue's loop: drain all pending…` — it *reimplements* the loop instead of driving the real code. That shape is exactly the AC#10(d)/(g) trap: do not copy it, and check whether that test needs updating once `process_command_queue` is relocated/removed. Mutation-verify the two guards that matter: (d) must fail if the poll head still drains; (g)/(c) must fail if a command can be enqueued twice.

- [x] **Task 6 — Docs sync.** (AC: 11) Update `docs/logging.md`, `docs/manual/latex/body.tex`, `README.md` (incl. Planning row), `CHANGELOG.md`. Do NOT touch the retired DocBook XML.

- [x] **Task 7 — Gates + soak.** (AC: 10, Promotion Gate) `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean. Then the real-binary / NAS soak (Dev Notes § Promotion Gate) before flipping to `done`.

## Dev Notes

### Architecture & data flow (verified against the tree 2026-07-23)

- **Command write path (unchanged by this story except the signal):** OPC UA client writes a numeric value to a command node → sync write callback `src/opc_ua.rs:1195` → `OpcUa::set_command` (`:2086`) validates (numeric, f_port 1–223, payload ≤ `MAX_LORA_PAYLOAD_SIZE`, u8 range) → `run_blocking_storage(|| storage.queue_command(cmd))` (`:2173`) persists a `Pending` row → returns `Good`. **This story adds:** `notify_one()` after a `Good` return.
- **Delivery path (relocated, not rewritten):** `get_pending_commands()` (`src/storage/sqlite.rs:1026`, `status='Pending'`) → `deliver_command` (`:2613`) resolves class/confirmed via `find_command_cfg` → `deliver_one` (`:2791`) maps to a downlink, enqueues via `DownlinkSink`, marks `Sent` (with `chirpstack_result_id`) or `Failed`. Confirmation/timeout are separate tasks (`CommandStatusPoller`, `CommandTimeoutHandler`) and are **out of scope**.
- **Why a separate task, not a new arm in the poller's `run()` select (`:1223`):** the poll loop runs `poll_metrics()` at the top of each iteration, so waking it to dispatch would either trigger a full metric poll or require restructuring the loop; and dispatch latency would couple to the poller's availability (a long poll or a 4-4 recovery loop would block commands). A dedicated task fully decouples dispatch from the poll cadence — this is CR #136's stated shape (a) and the architecturally-correct fix given the poll is being demoted.
- **Poll already skips streamed devices** (`:1474`, `device_is_streamed`) — reinforces that the poll must not be the command carrier.

### Notify pattern precedent

The codebase already uses `tokio::sync::Notify` for exactly this "signal a task to act now" shape: `apply_signal` (`src/main.rs:762`) wired from the web Apply handler into the restart supervisor. Follow that idiom. `notify_one()` is sync (safe from the OPC UA sync callback); a signal fired with no waiter parks one permit, so the dispatcher never misses a wake that races its `notified()` await (AC#6).

### Reuse map (do NOT reinvent)

| Need | Existing symbol | Location |
|------|-----------------|----------|
| gRPC channel + auth | `create_channel`, `create_interceptor`, `create_device_client`, `AuthInterceptor` | `src/chirpstack.rs:594 / 727 / 795 / 121` |
| Endpoint normalization (#148) | `normalize_chirpstack_endpoint` | `src/chirpstack.rs` |
| Downlink enqueue seam | `DownlinkSink` trait | `src/chirpstack.rs:2743` |
| Map + enqueue + status | `deliver_one`, `build_queue_item`, `map_command_to_downlink` | `src/chirpstack.rs:2791 / …` |
| Class/confirmed lookup | `find_command_cfg` | `src/chirpstack.rs:2871` |
| Pending rows | `get_pending_commands` | `src/storage/{sqlite,memory}.rs`, trait `mod.rs:353` |
| Task skeleton (struct+new+run+select) | `CommandStatusPoller` | `src/chirpstack.rs:3035` |
| Test stub | `MockSink` | `src/chirpstack.rs:4550` |
| Spawn/join wiring | `cmd_status`/`cmd_timeout` spawns; `DataPlaneHandles` (def `:285`, ctor `:460`); `join_data_plane` | `src/main.rs:402 / 285 / 474` |

### Anti-patterns / disasters to prevent

- **Double-send (AC#4):** the #1 correctness risk. `get_pending_commands` selects by `Pending`, and `deliver_one` marks `Sent` only after enqueue — so any second concurrent drainer double-enqueues. Removing the poll-head call (AC#3) and keeping delivery single-owner is the guard; do not add a "belt-and-suspenders" periodic drain on the poller.
- **`#73` / Epic-H async-storage bug:** never call a sync `StorageBackend` trait method from async context. Use `backend.async_store().…` in the dispatcher's async paths (as `deliver_one` already does at `:2809/2844`) and `run_blocking_storage` only in the sync OPC UA callback (as `set_command` already does). See [[project_issue73_async_storage]].
- **Fake regression guard (AC#10d/g):** drive the real drain/poll code, with seeds that make the pre-J-1 (poll-head drain) path and the J-1 (dispatcher-only) path produce **different** observable outputs, else the guard passes on both. See [[feedback_fake_regression_guard_tests]].
- **File size:** `src/chirpstack.rs` is already the largest module (~4900+ lines). This story adds a struct + factory. If it pushes past the 5000-line limit, extract the command-dispatch + client-factory into a sibling module (e.g. `src/chirpstack_dispatch.rs`) rather than growing the file. See [[feedback_source_file_size]].
- **Token leak:** `api_token` must never be logged by the new factory or dispatcher.

### Promotion Gate (DATA-PLANE story — AI-G-5 doctrine)

This changes the live command send path, so passing tests + clippy is necessary but **not sufficient**. Before flipping `review → done`:
1. Build the real binary and run the AI-G-5 smoke (a fresh-config boot that actually binds and serves) — the class of runtime bug (deadlock, task-never-spawned, panic on first signal) that tests+clippy miss. See [[incident_main_deadlock_2026_05_20]] and [[session_2026_06_29_aig5_146_onboarding_bug]].
2. On panoramix (or an equivalent real deployment): issue an OPC UA command and confirm it reaches the ChirpStack device queue **within seconds, not a poll interval**, and that the queue does not read as empty for a cycle (the E-0 AC#10 symptom this story fixes). Confirm no duplicate downlink and a clean soak (no dispatcher error spam, no `#152` NAS-latency regression). Deployment topology + log access: [[deployment_panoramix_nas]], [[deployment_scada_clients_ignition_fuxa]].

### Testing standards

Rust `#[tokio::test]` unit tests inline in `src/chirpstack.rs` `#[cfg(test)]`; `MockSink` + `InMemoryBackend` for delivery, no live gRPC. Match existing naming (`deliver_one_*`, `deliver_batch_*`). Assert real status transitions (`command_status_for_test`) and `MockSink.calls()` counts, not just "no longer pending". `cargo test` full run must be 0-fail and `cargo clippy --all-targets -- -D warnings` clean (project gate).

### Project Structure Notes

- New code lands in `src/chirpstack.rs` (dispatcher + factory) next to the sibling command tasks, or a new `src/chirpstack_dispatch.rs` if the 5000-line limit is crossed. Wiring changes in `src/main.rs` (`spawn_data_plane`, `DataPlaneHandles`, `join_data_plane`) and `src/opc_ua.rs` (`OpcUa` struct/`new` + write callback). Secondary dormant site in `src/opcua_topology_apply.rs`.
- No new files under `config/`, no schema/migration, no new env var or CLI flag, no new dependency (`tokio::sync::Notify`, `tokio_util::sync::CancellationToken`, `async_trait` are all already in use).

### References

- CR spec + sequencing: `_bmad-output/implementation-artifacts/sprint-status.yaml` (Epic J block, `J-1-decouple-command-dispatch` comment, line ~249).
- Poll loop & command drain: [Source: src/chirpstack.rs#poll_metrics (1426), #process_command_queue (2581), #deliver_command (2613), #deliver_one (2791), #DownlinkSink (2743), #create_channel (594)]
- OPC UA write path: [Source: src/opc_ua.rs#set_command (2086), #command-write-callback (1192), #OpcUa struct (159), #OpcUa::new (208)]
- Second write-callback site: [Source: src/opcua_topology_apply.rs (679)]
- Task spawn/join wiring: [Source: src/main.rs#spawn_data_plane (339), #DataPlaneHandles def (285) / ctor (460) / join_data_plane (474), #connection-pool (792, comment 787–791 stale), #apply_signal Notify precedent (762)]
- `OpcUa::new` test callers to update (8 sites / 6 files): tests/opc_ua_security_endpoints.rs:229, tests/opcua_history.rs:196+919, tests/opcua_dynamic_address_space_apply.rs:239, tests/opcua_subscription_spike.rs:287+1410, tests/opcua_dynamic_address_space_spike.rs:236, tests/opc_ua_connection_limit.rs:250
- Connection pool RAII checkout: [Source: src/storage/pool.rs#checkout / ConnectionGuard (363)]
- Storage command queue: [Source: src/storage/sqlite.rs#get_pending_commands (1026), #enqueue/dequeue_command (1913/1990); trait src/storage/mod.rs (353)]
- Previous story (continuity): `_bmad-output/implementation-artifacts/J-0-metric-mismatch-web-feed.md`

## Dev Agent Record

### Agent Model Used

Claude Opus 4.8 (1M context) — `bmad-dev-story`.

### Debug Log References

- `cargo build --all-targets` — clean (exit 0).
- `cargo test` — full suite **0 failed** (lib: 686 passed / 3 ignored; all integration binaries green).
- `cargo clippy --all-targets -- -D warnings` — clean (exit 0).
- J-1 tests confirmed by name: `chirpstack_dispatch::tests::{dispatch_delivers_a_queued_command, startup_drain_delivers_preexisting_command, burst_of_commands_all_delivered_once, cancellation_stops_the_dispatcher, double_drain_does_not_double_send}` (AC#10 a/b/c/f/g), `chirpstack::tests::poll_metrics_does_not_deliver_commands` (AC#10 d / AC#3 regression guard), `opc_ua::tests::set_command_signals_dispatch_only_on_good` (AC#10 e / AC#2).

### Completion Notes List

- **Design realised = CR #136 shape (a)**, event-driven. A successful (`Good`) `OpcUa::set_command` write fires a shared `Arc<tokio::sync::Notify>`; a dedicated `CommandDispatcher` task awaits it and drains the pending-command queue. The metrics poll no longer delivers commands (`process_command_queue` removed from the `poll_metrics` head).
- **File-size decision (Task 2 / anti-patterns note):** `src/chirpstack.rs` was already ~4.9k lines, so per [[feedback_source_file_size]] the dispatcher + production `DownlinkSink` + relocated drain were extracted into a new sibling module **`src/chirpstack_dispatch.rs`** (507 lines) rather than growing `chirpstack.rs` past the 5000-line limit. `chirpstack.rs` ended at 4876 lines (net shrink — the extraction removed more than the factory `pub(crate)` markers added).
- **Shared client factory (AC#8):** `create_channel` / `create_interceptor` / `create_device_client` / `create_application_client` were converted from inherent `ChirpstackPoller` methods into config-parameterized free functions (`*_from_config`) reused by both the poller and the dispatcher's `ChirpStackDownlinkSink`. `#148` port-defaulting, `normalize_chirpstack_endpoint`, the `AuthInterceptor`, the 5 s connect timeout, and the `chirpstack_connect` structured logs are preserved verbatim; `api_token` appears in no log line.
- **Single-owner delivery (AC#4):** the only drainer is the single `CommandDispatcher`; its drains are serialized and `deliver_one` marks `Sent` only after a successful enqueue. AC#10(g) `double_drain_does_not_double_send` mutation-guards this.
- **Drain error handling (deliberate contract change):** a `get_pending_commands` failure is now **logged (`command_dispatch_drain_error`) and swallowed**, not propagated — the long-lived dispatcher must not die or strand future commands on a transient storage fault; it retries on the next signal. (Under the old head-of-poll call a storage error aborted the poll cycle via `?`.)
- **Dormant second write-callback (`opcua_topology_apply.rs`):** the Story 9-8 live-mutation apply path is not spawned under F-0's staged-apply model, so plumbing the `Notify` through `apply_diff` was judged disproportionate for a dead path. A `// J-1:` decision comment marks it with instructions to wire the signal-on-`Good` there if that path is ever reactivated. The **live** path (`opc_ua.rs` add-command callback) is fully wired.
- **Connection pool:** bumped `5 → 7` and the stale `main.rs` comment corrected — the prior count of 5 already omitted the E-1 `events` claimer; J-1 adds the `command-dispatch` claimer. Per-op RAII checkout, so this is keep-the-invariant-honest sizing, not a starvation fix.
- **Docs (Task 6):** `docs/logging.md` (new `command_dispatch_drain` / `command_dispatch_drain_error` rows + Related-stories J-1 entry), `docs/manual/latex/body.tex` (Command-delivery section + architecture bullet now state immediate event-driven dispatch — no PDF rebuild required per story), `README.md` (Planning row flipped `J-1 backlog → J-1 review`), `CHANGELOG.md` (`[Unreleased]` 2.8.0 Changed entry, `Closes #136`).
- **⚠️ Promotion gate NOT yet satisfied (Task 7, DATA-PLANE story).** Tests (0-fail) + clippy (clean) are green — the code gate is met and the story is ready for **review**. The **review → done** flip still requires the AI-G-5 real-binary smoke + the panoramix/NAS soak (issue a real OPC UA command, confirm it reaches the ChirpStack device queue within seconds, no duplicate downlink, clean soak). See Dev Notes § Promotion Gate — this is intentionally left for the owner-run soak after code review, per the DATA-PLANE doctrine. Do **not** mark J-1 `done` until it passes.

### File List

- `src/chirpstack_dispatch.rs` — **NEW.** `CommandDispatcher` task, `ChirpStackDownlinkSink` (production sink via shared factory), `drain_pending_commands` (relocated drain), AC#10 tests (a/b/c/f/g).
- `src/chirpstack.rs` — extracted `*_from_config` client factory free functions; removed `process_command_queue` / `deliver_command` and the `ChirpstackPoller` `DownlinkSink` impl; `pub(crate)` on `DownlinkSink`, `deliver_one`, `find_command_cfg`, `AuthInterceptor`; removed the `process_command_queue` call from `poll_metrics`; AC#10(d) `poll_metrics_does_not_deliver_commands` test.
- `src/opc_ua.rs` — `dispatch_signal` field on `OpcUa`; new `OpcUa::new` param; `maybe_signal_dispatch` gate (fires only on `Good`); command write-callback wiring; doc-comment example fixed to 4-arg form; AC#10(e) test.
- `src/opcua_topology_apply.rs` — `// J-1:` decision comment at the dormant second command write-callback site.
- `src/main.rs` — `chirpstack_dispatch` module decl; per-cycle `dispatch_signal`; `OpcUa::new` call updated; `CommandDispatcher` spawned under `restart_token`; `cmd_dispatch` added to `DataPlaneHandles` (def + ctor) and `join_data_plane` (task array `5 → 6`); connection-pool size `5 → 7` + corrected comment.
- `src/lib.rs` — `pub mod chirpstack_dispatch;` for integration-test access.
- `tests/opc_ua_security_endpoints.rs`, `tests/opcua_history.rs`, `tests/opcua_dynamic_address_space_apply.rs`, `tests/opcua_subscription_spike.rs`, `tests/opcua_dynamic_address_space_spike.rs`, `tests/opc_ua_connection_limit.rs` — updated the 8 `OpcUa::new` call sites to pass a throwaway `Arc::new(tokio::sync::Notify::new())`.
- `docs/logging.md`, `docs/manual/latex/body.tex`, `README.md`, `CHANGELOG.md` — Task 6 docs sync.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — J-1 status → `review`.

### Change Log

| Date | Change |
|------|--------|
| 2026-07-24 | Story J-1 implemented: event-driven `CommandDispatcher` decouples downlink dispatch from the metrics poll (CR #136). Tasks 1–6 complete; Task 7 code gate (test 0-fail + clippy clean) green. Status `in-progress → review`. NAS/real-binary soak (review→done promotion gate) pending. |

## Review Findings

_bmad-code-review 2026-07-24 (3 adversarial layers: Blind Hunter, Edge Case Hunter, Acceptance Auditor). Acceptance Auditor found **no AC violations** — all 11 ACs verified satisfied. Findings below are robustness/quality items on the code, not spec gaps._

### Decision needed

- [ ] [Review][Decision] **Retry-gap: a `Pending` command can be stranded silently on a transient drain read-error (MEDIUM)** — In `drain_pending_commands` (`src/chirpstack_dispatch.rs`), a `get_pending_commands()` `Err` (e.g. transient SQLite lock/contention, cf. #152 NAS I/O) is logged as `command_dispatch_drain_error` and swallowed **after** the `Notify` permit was already consumed by the `notified()` that triggered the drain. `run()`'s `select!` has only `cancel` and `signal` arms — **no timer** — so nothing re-drives that drain. Verified the timeout sweep does NOT rescue it: `find_timed_out_commands` selects `WHERE status = 'Sent'` only (`src/storage/sqlite.rs:2231`), so a stranded `Pending` row is never swept to `Failed`; it stays `Pending`, invisible, until an unrelated later `Good` write fires a new permit (or a restart's startup drain). The old head-of-poll `process_command_queue` re-ran every `polling_frequency`, so this transient blip used to self-heal — J-1 removed that safety net. (`deliver_one` marks a failed/unmappable enqueue `Failed`, so per-command failures are NOT stranded; only the whole-batch read-error path is.) **Options:** (a) on the drain-error branch, re-arm `self.dispatch_signal.notify_one()` after a short backoff so the loop re-drains; (b) add a low-frequency `tokio::time::interval` safety-net arm to the `select!` (bounded re-drain; mildly dilutes the "purely event-driven, no cadence" design); (c) accept as-is and rely on the next write / restart (document the residual risk). Requires a design call because it touches AC#9's "purely event-driven, nothing to configure" intent.
- [ ] [Review][Decision] **Sole-path dispatcher has no per-task supervision — its death is a silent total command-delivery outage (MEDIUM)** — Since AC#3 removed the poll-loop delivery path, the `CommandDispatcher` is the **only** thing that delivers commands. Nothing re-spawns an individual data-plane task within a generation (the supervisor `select!`s only ctrl_c/sigterm/apply_signal; `join_data_plane` only runs at shutdown/Apply). If the task ever exits (panic through the drain path, or the `.expect()` on `SqliteBackend::with_pool` at `src/main.rs:561`), the OPC UA write callback keeps firing `notify_one()` into a permit nobody consumes and commands accumulate `Pending` forever with no error line. The unsupervised-task pattern is **pre-existing** (all four sibling data-plane tasks share it), but J-1 elevates the blast radius from "degraded" to "total outage" by making dispatch single-owner. Realistic probability is low (the loop is panic-free by design; `deliver_one` never panics; `.expect()` fails only at spawn, which the AI-G-5 smoke would catch), but the impact is high. **Options:** (a) accept/defer as a cross-cutting supervision concern for a later epic (recommended if the AI-G-5 smoke confirms the task spawns and delivers); (b) add a lightweight task-death watchdog now.

### Patch

- [ ] [Review][Patch] **AC#6 coalescing is asserted-in-comments but not actually test-driven** [src/chirpstack_dispatch.rs `burst_of_commands_all_delivered_once`] — all rows are `queue_command`'d **before** `run()` is spawned, so the startup drain (AC#5) delivers all three and the later `notify_one()` re-drains an empty queue. The test proves no-double-send but never exercises the "command enqueued **while a drain is in flight** → stored permit → re-drain" race that AC#6 is about. Improve it to enqueue after `run()` starts / while a drain could be in flight.
- [ ] [Review][Patch] **Extracted client-factory free functions keep impl-level 4-space indentation** [src/chirpstack.rs ~608–824] — `create_channel_from_config` / `create_interceptor_from_config` / `create_application_client_from_config` / `create_device_client_from_config` sit at module scope but retain their original in-`impl` indent. Compiles and passes clippy (the project gate), but `cargo fmt --check` would flag it. Run `cargo fmt` on the region.

### Deferred (pre-existing, LOW)

- [x] [Review][Defer] **Force-abort between a successful `enqueue` and the `Sent` write → double-send on the next generation's startup drain** [deliver_one mark-after-enqueue × join_data_plane force-abort] — deferred, pre-existing ordering (unchanged by J-1). Narrow window: requires a dispatcher overrunning the bounded join AND the `.abort()` landing in the microsecond gap after ChirpStack accepted the downlink but before the row is marked `Sent`; next boot's startup drain re-enqueues. No `chirpstack_result_id` idempotency guard on re-enqueue. Steady-state respawn is safe (verified `restart_token.cancel(); join_data_plane().await` before respawn at `src/main.rs:1659`).
- [x] [Review][Defer] **Unbounded per-signal drain re-scan** [src/chirpstack_dispatch.rs drain via `get_pending_commands` — no LIMIT] — deferred, pre-existing query shape. If a `Pending` backlog accumulates, every signal re-reads and re-attempts the whole set. Not J-1-introduced.

### Dismissed (6)

- Cross-generation double-send on Apply (Blind) — **refuted**: `restart_token.cancel(); join_data_plane(handles).await;` fully joins the old generation before the respawn (`src/main.rs:1659–1660`), so the old dispatcher cannot overlap the new startup drain in the normal path.
- `run()` never returns `Err`, making the `main.rs` error branch unreachable (Blind) — matches the deliberate sibling pattern (`CommandStatusPoller::run` also returns `Result<(), _>` and never `Err`).
- Frozen `AppConfig` clone ignores SIGHUP hot-reload (Blind) — moot under F-0: SIGHUP hot-reload was retired; every config change goes through an Apply respawn that builds a fresh dispatcher.
- Second (topology-apply) write-callback not wired to the signal (Blind/Edge) — dormant under F-0 and a documented `// J-1:` decision; Task 3 explicitly permits skipping the dead path.
- Double `AppConfig` clone duplicates the API token in memory (Blind) — negligible; the token already lives in every task's `AppConfig`.
- "Closes #136" must be in the commit message (Auditor) — process note carried to the commit step, not a code defect.

### Iteration 1 — resolution (2026-07-24)

Both `decision-needed` findings were routed to the owner, who chose to fix both; all 4 patches applied:

- [x] **P1 (Decision 1) — retry-gap FIXED.** `drain_pending_commands` now returns `bool` (`false` on a `get_pending_commands` read-error). `CommandDispatcher::run` gained a bounded, cancellation-aware retry arm: on a read-error the loop waits `DRAIN_RETRY_BACKOFF` (2 s) then re-drains, so a transient storage fault can no longer strand a `Pending` command until an unrelated future write. On a healthy drain the retry future is `std::future::pending()` (never fires) — the happy path stays purely event-driven, no cadence. [src/chirpstack_dispatch.rs]
- [x] **P2 (Decision 2) — task-death watchdog ADDED.** The `cmd_dispatch` spawn in `spawn_data_plane` is now a self-contained restart-on-death supervisor: it runs the dispatcher in a nested task, so a panic surfaces as a `JoinError`; on an unexpected exit it logs `command_dispatch_task_died` (error) and restarts after a bounded, cancellation-aware backoff; on cancellation it exits cleanly. It deliberately does NOT touch the main restart supervisor (kept minimal per the 2026-05-20 deadlock history). The `.expect()` on backend construction was replaced by a handled error path. [src/main.rs]
- [x] **P3 — AC#6 coalescing now test-driven.** Added `command_enqueued_mid_drain_is_delivered`: a `MidDrainInjectSink` enqueues a new command + fires the signal *during* the first drain, proving the stored `Notify` permit drives a re-drain that delivers the late command (the real lost-wakeup race, which the pre-existing burst test could not reach). The literal AC#10(c) burst test is retained. [src/chirpstack_dispatch.rs]
- [x] **P4 — extracted free functions de-indented.** The four `*_from_config` factory functions are now at module indentation (rustfmt-compliant). Note: this is a whitespace-only reindent of the moved block, so it accounts for the whitespace portion of the `chirpstack.rs` diff — the semantic diff (`git diff -w`) is 244 lines, matching the pre-review J-1 change. [src/chirpstack.rs]

**Gates re-run after patches:** `cargo build --all-targets` clean; `cargo clippy --all-targets -- -D warnings` clean; `cargo test` **1868 passed / 0 failed**. New/again-green J-1 tests include `command_enqueued_mid_drain_is_delivered` (P3).

**⚠️ Loop NOT yet terminated.** Two gates remain before `review → done`:
1. **Iter-2 re-review (mandatory).** P1 and P2 introduce brand-new flow-control (the retry `select!` arm; the nested-spawn watchdog loop), so per the [[feedback_iter3_validation]] doctrine an iteration-2 adversarial re-review of the patched code is required before the loop can be called clean.
2. **DATA-PLANE promotion soak.** The AI-G-5 real-binary smoke + panoramix/NAS soak still gate `review → done` (unchanged).

### Change Log — code review

| Date | Change |
|------|--------|
| 2026-07-24 | bmad-code-review iter-1: 3 adversarial layers; Acceptance Auditor found no AC violations. 2 MEDIUM decisions (retry-gap, sole-path supervision) + 2 LOW patches. Owner chose to fix both decisions. All 4 patches applied; gates green (1868 tests, clippy clean). Iter-2 re-review + NAS soak still pending → Status stays `review`. |

### Iteration 2 — re-review of the patches (2026-07-24)

Two adversarial layers (general + edge-case) re-reviewed the new flow-control (P1 retry, P2 watchdog, P3 test). **P1 and P3 were cleared as correct** (select! semantics sound, no lost wakeup, P3 deterministic and not a tautology, pool sizing fine). New findings, all on **P2**:

- [ ] [Review][Decision] **P2 watchdog introduces a restart-induced double-downlink, weakening AC#4 (MEDIUM).** `deliver_one` marks a row `Sent` only *after* the gRPC `enqueue` succeeds. If the dispatcher task panics/aborts in the window between a successful `enqueue_downlink` and `mark_command_sent`, the row stays `Pending`; P2 then restarts the dispatcher, whose startup drain (AC#5) re-reads that `Pending` row and **re-enqueues the same downlink** (double valve actuation). Before P2, task death simply stopped delivery — no double-send. The double-downlink is *inherent* to any "restart + startup-drain re-reads Pending" design and is only fully closable with `chirpstack_result_id` idempotency (the deferred iter-1 Cluster C item). Both iter-2 layers flagged this independently; the adversarial layer calls it "worth an explicit accept/defer decision since it weakens the AC#4 single-delivery guarantee the story is built around."
- [ ] [Review][Decision] **P2 nested inner spawn escapes the `join_data_plane` abort backstop (MEDIUM).** The F-0/D1 backstop collects `abort_handle()`s only for the six *outer* `DataPlaneHandles` tasks (`main.rs:591`); the dispatcher's real work runs in the **inner** `run_once` spawn. If the 10 s bounded join times out and force-aborts the outer `cmd_dispatch` handle while parked at `run_once.await`, tokio merely *detaches* (does not abort) the inner task, which can outlive the cycle holding a SQLite pool connection into the next rebind — a hole in the exact invariant D1 was written to guarantee.
- [ ] [Review][Patch] **Persistent-fault log spam violates the WARN/ERROR budget discipline (MEDIUM).** P1's retry re-drives every 2 s emitting a `command_dispatch_drain_error` WARN on each failed read (~43k WARN/day under a sustained storage outage, e.g. #152). (P2's restart path similarly logs ~86k ERROR/day, moot if P2 is reverted.) Given the project's WARN-budget history (#144/#149), add an escalating/capped backoff + rate-limited logging. [src/chirpstack_dispatch.rs]
- [ ] [Review][Patch] **Drains are not cancellation-aware (edge, improvement).** `drain_pending_commands` runs to completion ignoring cancellation; under a large `Pending` backlog + slow sink this delays teardown toward the 10 s force-abort (the trigger for the orphan above) and reduces the pre-existing force-abort double-send window (iter-1 Cluster C). Thread the `cancel_token` and check `is_cancelled()` between commands, matching the existing pattern (`chirpstack.rs:2177/2191`). Apply regardless of the P2 decision. [src/chirpstack_dispatch.rs]

### Iteration 2 — resolution (2026-07-24)

- [x] **P2 (both MEDIUM hazards) — RESOLVED by reverting the watchdog.** Owner decision: revert P2, defer per-task supervision. `cmd_dispatch` is back to a single registered `tokio::spawn` (like the sibling `cmd_status`/`cmd_timeout` tasks), so it is covered by `join_data_plane`'s force-abort backstop and cannot orphan an inner task or restart-into a double-downlink. The task-death-outage risk (iter-1) is now an **owner-accepted deferred** cross-cutting concern (see deferred-work.md) — the dispatch loop is panic-free by design. AC#4 single-delivery is intact again. [src/main.rs]
- [x] **P5 (MEDIUM log-spam) — FIXED.** The read-error retry backoff now **escalates** `2s→4s→8s→16s→32s→60s(cap)` via `drain_retry_backoff(consecutive_errors)`, resetting on any healthy drain. A sustained storage outage is re-driven (and `command_dispatch_drain_error`-WARN'd) at most once per cap interval instead of every 2 s — bounding both wasted work and WARN volume (project WARN-budget discipline). Unit test `drain_retry_backoff_escalates_and_caps`. [src/chirpstack_dispatch.rs]
- [x] **P6 (edge improvement) — FIXED.** `drain_pending_commands` is now cancellation-aware: it takes the `CancellationToken` and checks `is_cancelled()` between commands (matching the poller's pagination-loop pattern), so teardown/Apply is responsive under a large backlog + slow sink and the task returns well before `join_data_plane`'s 10 s force-abort — closing the trigger for the (now-reverted) orphan and shrinking the pre-existing force-abort double-send window. Interrupted commands stay `Pending` for the next startup drain (no loss). Test `cancelled_drain_delivers_nothing`. [src/chirpstack_dispatch.rs]

**Note on iteration-1 record:** the iter-1 P2 bullet above ("task-death watchdog ADDED") was **superseded** — the watchdog was reverted here after iter-2 showed it weakened AC#4.

**Gates re-run after iter-2 patches:** `cargo clippy --all-targets -- -D warnings` clean; `cargo test` **1872 passed / 0 failed**. New tests: `cancelled_drain_delivers_nothing`, `drain_retry_backoff_escalates_and_caps` (+ P3's `command_enqueued_mid_drain_is_delivered`).

### Change Log — code review (cont.)

| Date | Change |
|------|--------|
| 2026-07-24 | bmad-code-review iter-2 (mandatory re-review of new flow-control): P1/P3 cleared as correct; P2 watchdog found to introduce a restart double-downlink + abort-backstop hole (both MEDIUM, weaken AC#4). Owner chose to revert P2 + defer supervision. Applied P5 (escalating capped retry backoff) + P6 (cancellation-aware drain). Gates green (1872 tests, clippy clean). |

### Iteration 3 — fresh 3-layer re-review of commit f27b19f (2026-07-25)

Full re-run (Blind Hunter diff-only / Edge Case Hunter with project access / Acceptance Auditor vs this story) on a different LLM (Fable) than the implementer (Opus 4.8). 26 raw findings → deduped triage: 4 decision-needed (delivery-semantics cluster), 9 patches, 2 deferred, rest dismissed. The four decisions were resolved in an autonomous **party session** (Winston/Amelia/Quinn/Murat/John personas; owner pre-delegated 2026-07-25, decisions to be ratified via the end-of-epic report):

- [x] **D1 (MED, blind+edge) — transient sink failure was terminally `Failed` (no delivery retry; boot race with ChirpStack startup).** DECIDED: bounded retry. `deliver_one` now returns `DeliveryOutcome` {Delivered, Terminal, RetryLater}; a gRPC/enqueue error leaves the row `Pending` (`command_dispatch_retry` WARN) and the drain reports "unsettled" so `run()` re-drives it on the existing escalating backoff. Mapping failures stay Terminal. Mutation-verified (dropping retry scheduling fails `transient_sink_failure_is_retried_until_delivered`).
- [x] **D2 (MED, edge) — unbounded-age startup drain could actuate stale hardware commands.** DECIDED: delivery deadline. A `Pending` row older than `global.command_delivery_timeout_secs` (reused knob — no new config) is marked `Failed` (`command_dispatch_expired` WARN) and never delivered. Bounds D1's retries and the startup drain; AC#5 across-restart delivery holds within the deadline (the Apply soft-restart case). Mutation-verified (disabling the age gate fails `expired_command_is_failed_not_delivered`).
- [x] **D3 (MED, edge) — Apply-orphaned rows fell back to raw-byte unconfirmed downlink.** DECIDED: orphans are terminal. `find_command_cfg → None` now marks `Failed` (`command_dispatch_orphaned` WARN), never enqueues. Rationale: command nodes exist only for configured commands, so `None` at drain time proves de-configuration. Class-less commands are unaffected (their cfg resolves with `command_class: None`). Test `orphaned_command_is_failed_not_delivered`.
- [x] **D4 (MED, edge) — one poison `Pending` row livelocked all dispatch (fail-fast `collect`).** DECIDED: storage self-heal. `get_pending_commands` validates per-row; malformed rows are quarantined `Failed` with the parse reason (`command_queue_row_invalid` WARN, once per row) and valid rows still dispatch. Test `test_poison_pending_row_is_quarantined_not_fatal`.

Patches applied (all 9):

- [x] **P7 (MED, blind)** `.expect()` on `SqliteBackend::with_pool` in the dispatcher spawn → handled `match` (ERROR + task exit; commands stay `Pending` until next Apply/restart). Sole-delivery-path death is no longer a silent panic. Siblings keep their `expect` (not sole owners; pre-existing pattern). [src/main.rs]
- [x] **P8 (MED, blind)** Fresh gRPC channel per command → client cached in `ChirpStackDownlinkSink` behind a `tokio::sync::Mutex`, dropped on any RPC failure for a clean reconnect. [src/chirpstack_dispatch.rs]
- [x] **P9 (MED, edge)** No deadline on the `enqueue` RPC → `ENQUEUE_RPC_TIMEOUT` (10 s) via `tokio::time::timeout`; timeout treated as a transient (D1) failure. [src/chirpstack_dispatch.rs]
- [x] **P10 (MED, blind+auditor)** docs/logging.md drift → `command_dispatch_drain_cancelled` row added; `command_dispatch_drain_error` row rewritten (self-driven escalating backoff, not "next signal"); new rows for `command_dispatch_retry` / `command_dispatch_expired` / `command_dispatch_orphaned` / `command_queue_row_invalid`; J-1 summary paragraph updated. [docs/logging.md]
- [x] **P11 (LOW, blind)** AC#10(a) test's signal was decorative (startup drain delivered) → reworked: `run()` spawns first, startup drain completes empty, THEN queue+signal — deleting the signal arm now fails it. [src/chirpstack_dispatch.rs]
- [x] **P12 (LOW, blind)** `== StatusCode::Good` → `status.is_good()` in `maybe_signal_dispatch` (robust to Good-class sub-codes). [src/opc_ua.rs]
- [x] **P13 (LOW, blind)** Enqueue error detail discarded → `OpcGwError::ChirpStack` now carries the tonic status (code+message; no token), which becomes the operator-facing failure reason. [src/chirpstack_dispatch.rs]
- [x] **P14 (LOW, blind)** Test poll bound ~1 s (CI-flake class) → ~5 s (pass path exits early). [src/chirpstack_dispatch.rs]
- [x] **P15 (LOW, edge)** Pool comment claimed 7 claimers; documented the transient Apply-window burst (~9, bounded by the 5 s checkout busy-wait). [src/main.rs]

Deferred / dismissed:

- [ ] **DEF-iter3-J1-D5 (MED, edge ×2)** — at-least-once duplicate-send windows: (a) force-abort/crash between a successful `enqueue` and `mark_command_sent`; (b) `mark_command_sent` storage failure after a successful enqueue. Both leave an already-enqueued row `Pending` → re-enqueue on the next drain. Pre-existing E-0 contract (predates J-1; the poll-head drain had the identical windows); full fix = `chirpstack_result_id`-keyed idempotency check before enqueue (iter-1 Cluster C). → deferred-work.md + GH issue (filed this session). The drain comment no longer overclaims "never left half-delivered".
- Dismissed: backoff-reset-under-writes nuance (documented in-code, correctness-neutral); dormant `opcua_topology_apply` signal site (owner-sanctioned `// J-1:` decision comment, re-confirmed); AC#7 third select-arm deviation (owner-sanctioned iter-1/iter-2, AC text now amended below).

**AC amendments (iter-3):** AC#2/AC#10(e) — the gate is `is_good()`, not `== Good` (superset, same rejection behaviour). AC#5 — across-restart delivery holds **within the delivery deadline** (`command_delivery_timeout_secs`); beyond it commands expire `Failed` by design (D2). AC#7 — `run()`'s `select!` carries the sanctioned third (bounded-retry timer) arm; the happy path remains signal-only. AC#9 — `deliver_one` now returns `DeliveryOutcome`; orphan rows are no longer raw-delivered (D3). AC#10 — the dispatch tests use a module-local `MockSink` (accepted #102 inline-harness pattern), not `chirpstack::tests::MockSink` as the AC text literally suggested; recorded here as the deviation decision. AC#11 — the implementation commit says `Refs #136`; the **PR** carries `Closes #136` so the issue closes when the story lands on main (the NAS soak is tracked by the v2.8.0-rc1 release gate + owner report instead).

**File List additions (iter-3):** `Cargo.toml` (dev-deps tokio `test-util` for the paused-clock retry test), `src/storage/sqlite.rs` (lenient `get_pending_commands` + quarantine), `src/storage/sqlite_tests.rs` (poison-row test), `_bmad-output/implementation-artifacts/deferred-work.md` (D5 entry; also belatedly listed for the iter-2 supervision deferral the iter-2 File List missed).

**Gates after iter-3:** `cargo test` full suite + `cargo clippy --all-targets -- -D warnings` — see Change Log row below. New tests: `orphaned_command_is_failed_not_delivered`, `expired_command_is_failed_not_delivered`, `transient_sink_failure_is_retried_until_delivered`, `test_poison_pending_row_is_quarantined_not_fatal`, reworked `dispatch_delivers_a_queued_command`, renamed `deliver_one_enqueue_failure_leaves_pending_for_retry`. Mutation checks: D1 retry-scheduling and D2 age gate each sabotaged → target test failed → reverted.

### Change Log — code review (cont. 2)

| Date | Change |
|------|--------|
| 2026-07-25 | bmad-code-review iter-3 (fresh 3-layer pass on commit f27b19f, Fable vs Opus-4.8 implementer): 4-decision delivery-semantics cluster resolved by party session (bounded retry D1 / delivery deadline D2 / terminal orphans D3 / poison-row quarantine D4) + 9 patches (P7–P15). Duplicate-send at-least-once windows deferred with GH issue (DEF-iter3-J1-D5). Both new guards mutation-verified. |

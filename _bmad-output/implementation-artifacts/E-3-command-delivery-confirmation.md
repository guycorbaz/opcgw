# Story E.3: Command Delivery Confirmation

Status: done

<!-- Ultimate context engine analysis completed - comprehensive developer guide created (2026-06-13). Last story in Epic E; on done → epic-E-retrospective becomes mandatory (CLAUDE.md Epic Completion Requirements: run the security check before closing). -->

## Story

As an **opcgw operator**,
I want command delivery/confirmation status reflected back to me (Sent → Confirmed/Failed),
so that I know whether a command I wrote via OPC UA actually reached the device — not merely that opcgw handed it to ChirpStack.

## Context & Why Now

Epic E made opcgw a model-agnostic, class-aware device-abstraction layer. E-0 wired the **downlink command path** (an OPC UA write is enqueued to ChirpStack and the command transitions `Pending → Sent`). E-1 wired **uplink-event ingestion** over a long-lived gRPC `InternalService.StreamDeviceEvents` consumer (`src/chirpstack_events.rs`). E-2a delivered the **device-class registry** and the `command_class` web surface. v2.2.0 shipped stable on 2026-06-13.

What's still missing is the **back half of the command lifecycle**: today `Sent` is terminal in practice. opcgw reports "I enqueued it to ChirpStack" but never observes whether the gateway transmitted it or the device acknowledged it. For the Tonhe E20 valve (which sends a **confirmed** downlink ACK), the information to close the loop is already flowing past us on the E-1 event stream — we just drop it.

**This story closes the loop**: observe ChirpStack's delivery/ack signal, correlate it to the queued command, transition `Sent → Confirmed` (or `→ Failed` on timeout), surface the status on OPC UA and in the audit log. It is the last story in Epic E; completing it triggers the mandatory Epic E retrospective.

**Tracking:** GitHub issue [#129](https://github.com/guycorbaz/opcgw/issues/129) (Epic E). This story is the `E-3-command-delivery-confirmation` key in `sprint-status.yaml`.

**Design source of truth:** `epics.md` §"Story E.3" + §"Epic E — Story Acceptance Criteria"; memory `project_device_abstraction_valves.md` (E-3 = confirmation poller in the locked design).

### Critical pre-existing-state findings (verified 2026-06-13 — read before coding)

These three facts were verified against the current `main` and **change the shape of E-3**:

1. **The correlation key is never populated today.** `DownlinkSink::enqueue_downlink` (`src/chirpstack.rs:2606,2611`) returns `Result<(), OpcGwError>` — it receives ChirpStack's `EnqueueDeviceQueueItemResponse { id }` (the queue-item UUID, `proto/chirpstack/api/device.proto:550-553`) at `:2622` and **throws it away**. `deliver_one` (`:2680-2684`) then marks the command `Sent` via `update_command_status(command.id, CommandStatus::Sent, None)` — it does **not** call `mark_command_sent(command_id, chirpstack_result_id)`. Consequently the `chirpstack_result_id` column is **always NULL** in production, and `mark_command_sent` (`src/storage/sqlite.rs:2094`) is currently **dead code** (no caller). **E-3 must first capture and persist the enqueue id** or it has nothing to correlate confirmations against.

2. **Confirmation is event-based, not poll-based.** ChirpStack surfaces downlink delivery via the **same** `StreamDeviceEvents` `LogItem` stream E-1 already consumes (`src/chirpstack_events.rs`), with `LogItem.description` ∈ {`"up"`, `"join"`, `"ack"`, `"txack"`, `"status"`, `"error"`, …} and the event payload JSON in `LogItem.body`. The relevant events:
   - **`ack`** → `AckEvent` (`proto/chirpstack/integration/integration.proto:201`): fields `queue_item_id` (string, =EnqueueResponse.id), `acknowledged` (bool), `f_cnt_down`. Emitted **only for confirmed downlinks** when the device ACKs. **This is the true "device received it" signal.**
   - **`txack`** → `TxAckEvent` (`integration.proto:224`): fields `downlink_id`, `queue_item_id` (string), `f_cnt_down`. Means the **gateway transmitted** the downlink over the air — not proof the device received it.
   E-1's consumer (`chirpstack_events.rs:363,429`) hard-filters `description == "up"` and **skips ack/txack**. So the data E-3 needs is already arriving and being discarded.

3. **The stub is poll-shaped; the signal is event-shaped.** `CommandStatusPoller` (`src/chirpstack.rs:2858-2943`) is a 5 s polling loop (`config.global.command_delivery_poll_interval_secs`, default 5, `config.rs`) that calls `find_pending_confirmations()` and then only `trace!`s a "would poll ChirpStack" placeholder (`:~2925`). There is **no ChirpStack gRPC method to poll for per-command ack** — the ack arrives as an event. A companion **`CommandTimeoutHandler`** (`src/chirpstack.rs:2945-3037`) already sweeps `find_timed_out_commands(ttl)` (default TTL 60 s, check interval 10 s) and marks stragglers `Failed("…timeout")`. **Reconcile the two**: the event stream is the primary confirmation path; the poller/timeout-handler is the safety-net (timeout + reconciliation), not the primary observer.

> ⚠️ Subagent-sourced line numbers above are anchors, not contracts — re-grep before editing; the surrounding code may have shifted. The three *behavioural* facts (id discarded, ack/txack skipped, stub is a trace placeholder) were directly verified.

## Acceptance Criteria

1. **Enqueue id captured and persisted (prerequisite).** The downlink enqueue path captures `EnqueueDeviceQueueItemResponse.id` and stores it as the command's `chirpstack_result_id`, transitioning `Pending → Sent` **with** the id. Concretely: `DownlinkSink::enqueue_downlink` returns the id (e.g. `Result<String, OpcGwError>`), and `deliver_one` calls `mark_command_sent(command.id, &id)` instead of `update_command_status(.., Sent, None)`. After a successful enqueue, `chirpstack_result_id` is non-NULL for that row. The mock `DownlinkSink` in tests returns a stub id.

2. **Confirmation observed from the event stream.** opcgw observes ChirpStack `LogItem` events with `description == "ack"` on the `StreamDeviceEvents` stream, parses `AckEvent` from `body` (`queue_item_id`, `acknowledged`, `f_cnt_down`), and on `acknowledged == true` correlates `queue_item_id` to the queued command via `chirpstack_result_id` and transitions that command `Sent → Confirmed` (sets `confirmed_at`). An `ack` with `acknowledged == false` (device NACK / max retries) transitions the command `Sent → Failed` with a descriptive error message. Events that match no pending command are logged at debug and ignored (no error, no crash).

3. **TxAck handled distinctly from Ack.** A `txack` event (gateway transmitted) is **not** treated as device confirmation. The chosen behaviour is explicit and documented: either (a) recorded as a debug/diagnostic trace only, or (b) used to advance an intermediate signal — but `Confirmed` requires an `ack` (for confirmed downlinks). For **unconfirmed** downlinks (no ack will ever arrive), the command's terminal resolution is the timeout path (AC#5) or an explicit policy documented in Dev Notes — do not leave unconfirmed commands silently `Sent` forever without a defined terminal transition.

4. **Correlation is robust.** Correlation keys on `chirpstack_result_id == queue_item_id` (exact match). A confirmation for an already-terminal command (`Confirmed`/`Failed`) is a no-op (idempotent — `mark_command_confirmed` already guards `status IN ('Sent','Pending')`; rely on that, don't double-write). Duplicate/replayed ack events (ChirpStack replays recent events on stream reconnect, per E-1's freshness-guard finding) must not regress or error — confirming an already-Confirmed command is a no-op.

5. **Timeout / failure path.** A command left `Sent` with no ack within the configured delivery timeout (`config.global.command_delivery_timeout_secs`, default 60) is transitioned `Sent → Failed` by the existing `CommandTimeoutHandler` sweep, with a clear error message (`"Confirmation timeout after Ns"`). The timeout path and the confirmation path race-safely converge on the same row (a confirm landing just before/after a timeout sweep must not produce a contradictory or double transition — the `status IN ('Sent','Pending')` guard on both `mark_command_confirmed` and `mark_command_failed` provides this; verify with a test).

6. **OPC UA surface reflects status.** Command status (at minimum the latest/aggregate, ideally per-command) is readable by an OPC UA client. The existing `CommandStatusQuery` node (`src/opc_ua.rs:~1147`) currently returns a static placeholder string; wire its read callback to return real status from storage (JSON: `command_id`, `device_id`, `command_name`, `status`, `sent_at`, `confirmed_at`, `error_message`). Keep it read-only and side-effect-free in the read callback (no blocking I/O that could stall the OPC UA worker — snapshot from storage like other read callbacks do).

7. **Audit log.** Emit structured, greppable audit events: `event="command_confirmed"` (with `command_id`, `device_id`, `command_name`, `chirpstack_result_id`, and `latency_ms` = `confirmed_at − sent_at`), `event="command_confirm_failed"` (NACK), and `event="command_timeout"` (from the timeout handler). Follow the existing `event=`-field house pattern (see E-0/E-2 audit events and `docs/logging.md`). Any **new** event types are added to `docs/logging.md` in the same commit (AC#9).

8. **No aggregation, no regression to E-0/E-1.** The `command_class == None` / raw path and all E-0 send behaviour stay byte-for-byte intact except the additive id-capture in AC#1. E-1 uplink ingestion (the `"up"` path, last-known-value, no aggregation, freshness guard) is **unchanged** — adding `ack`/`txack` handling to the consumer must not alter `"up"` handling or drop uplinks. Generic devices and all existing tests pass.

9. **Docs sync (same commit).** Update: `docs/logging.md` (new audit events), `docs/architecture.md` (the command lifecycle Pending→Sent→Confirmed/Failed + the event-stream confirmation path), the DocBook manual `docs/manual/opcgw-user-manual.xml` (operator-facing: what command status means + the delivery-timeout knob), the config reference if a new knob is added, and the **README Planning** section + `sprint-status.yaml` to mirror E-3 → `done` (and Epic E 4/4). `xmllint --noout` the DocBook. SPDX `MIT OR Apache-2.0` + `(c) [2026] Guy Corbaz` header on every new `.rs` file.

10. **Tests + clippy clean.** New tests cover: id-capture on enqueue (AC#1); ack→Confirmed with correct `confirmed_at` + latency (AC#2); NACK→Failed (AC#2/3); unmatched `queue_item_id` ignored (AC#2); duplicate/replayed ack is a no-op (AC#4); timeout→Failed (AC#5); confirm-vs-timeout race idempotency (AC#5); txack does not confirm (AC#3); OPC UA status read returns real data (AC#6). Timeout tests need a **time seam** — `find_timed_out_commands(ttl)` uses `Utc::now()` directly (`src/storage/sqlite.rs:~2208`); test by inserting a row with a back-dated `sent_at` (no clock injection needed) rather than sleeping. Regression-guard tests must invoke the real function and use seeds whose outputs differ across the surviving vs dropped path (no fake guards — see Dev Notes). `cargo test` + `cargo clippy --all-targets -- -D warnings` clean.

## Tasks / Subtasks

- [x] **Task 1 — Capture & persist the enqueue id (AC: 1)**
  - [x] Change `DownlinkSink::enqueue_downlink` (`src/chirpstack.rs:2606`) to return the queue-item id: `async fn enqueue_downlink(&self, item) -> Result<String, OpcGwError>`. In the `ChirpstackPoller` impl (`:2611`), return `inner_response.id` instead of discarding it (`:2622`).
  - [x] In `deliver_one` (`:2671-2684`), on `Ok(id)` call `backend.mark_command_sent(command.id, &id)` (replaces `update_command_status(.., Sent, None)`). Keep the `Err` arm → `Failed` as-is.
  - [x] Update the mock `DownlinkSink` in tests (`:4269` area, `MockSink`) to return a stub id (e.g. `"qid-test-1"`), and update the existing `deliver_one_*` assertions (`:4385+`) to assert `chirpstack_result_id` is set on success (this is the AC#1 regression guard — the value differs between the old `None` path and the new id path).
  - [x] Confirm `mark_command_sent` (`src/storage/sqlite.rs:2094`) writes `status='Sent', sent_at, chirpstack_result_id, updated_at` (it does) — it becomes a live caller, no longer dead code.

- [x] **Task 2 — Decode ack/txack events on the existing stream (AC: 2, 3, 8)**
  - [x] In `src/chirpstack_events.rs`, generalize the consumer so the `next_event()`/parse path (`:354-372`, `parse_up_event` `:428`) recognizes `description == "ack"` and `description == "txack"` **in addition to** `"up"` — without changing `"up"` behaviour. Prefer a small `enum DeviceEventKind { Uplink(..), Ack(AckInfo), TxAck(TxAckInfo), Other }` returned by the parser so the task loop dispatches cleanly.
  - [x] Parse `AckEvent` / `TxAckEvent` from `LogItem.body` (JSON) — extract `queue_item_id`, `acknowledged`, `f_cnt_down` (ack) / `downlink_id`, `queue_item_id` (txack). Reuse the proto types if convenient, but `body` is JSON, so a `serde` struct of just the needed fields is acceptable and simpler (mirror how `parse_up_event` reads `body.object`). Malformed body → debug-log + skip (never crash the stream; mirror E-1's `uplink_event_dropped` handling).
  - [x] Decide txack policy per AC#3 and implement it (recommend: debug-trace only; `Confirmed` requires `ack`).

- [x] **Task 3 — Correlate & transition (AC: 2, 3, 4, 7)**
  - [x] On an `ack` event with `acknowledged == true`: look up the command by `chirpstack_result_id == queue_item_id` (add a storage method `find_command_by_result_id(&str) -> Result<Option<Command>, _>` if none exists, mirroring `find_pending_confirmations` query style), then `backend.mark_command_confirmed(cmd.id)`. Emit `event="command_confirmed"` with `latency_ms`.
  - [x] On `acknowledged == false`: `backend.mark_command_failed(cmd.id, "Device NACK / max downlink retries")`. Emit `event="command_confirm_failed"`.
  - [x] Unmatched `queue_item_id` (no row, or already terminal): debug-log + ignore. Rely on the `status IN ('Sent','Pending')` guard in `mark_command_*` for idempotency (AC#4) — do not pre-check then write (TOCTOU); just call and treat "0 rows updated / already-terminal" as benign.

- [x] **Task 4 — Reconcile the poller / timeout handler (AC: 5)**
  - [x] Decide CommandStatusPoller's fate (see Dev Notes "Architecture decision"): **recommended** — repurpose it as a no-op/reconciliation sweep (or remove it and keep only `CommandTimeoutHandler`), since confirmation now flows via the event stream. Whatever is chosen, **do not leave the misleading `"would poll ChirpStack"` placeholder** (`:~2925`). If removed, drop its spawn in `main.rs` (`:1087`) and its config knob, and note it in docs.
  - [x] Verify `CommandTimeoutHandler` (`:2945-3037`) emits `event="command_timeout"` (AC#7) — add the structured event if missing. Confirm its TTL/interval knobs are documented.
  - [x] Add a test for the confirm-vs-timeout race (AC#5): a command whose `sent_at` is back-dated past TTL, then a confirm arrives — assert exactly one terminal transition wins and no panic/contradiction.

- [x] **Task 5 — OPC UA status read callback (AC: 6)**
  - [x] Wire the `CommandStatusQuery` node read callback (`src/opc_ua.rs:~1171-1184`) to snapshot real status from storage and return JSON. Keep it non-blocking/side-effect-free like other read callbacks (snapshot under the existing storage lock pattern; do not introduce a long-held lock or async-in-sync-callback hazard). If a per-command surface is out of scope for this story, expose the most-recent N commands' status and document the limitation.

- [x] **Task 6 — Tests (AC: 10)**
  - [x] Unit (in `src/chirpstack_events.rs` `#[cfg(test)]` + `src/chirpstack.rs`): ack/txack `LogItem` parsing (valid + malformed body); ack→Confirmed; NACK→Failed; txack→no-confirm; unmatched id→ignored; duplicate ack→no-op.
  - [x] Storage (`src/storage/sqlite_tests.rs`): `mark_command_sent` sets `chirpstack_result_id`; `find_command_by_result_id`; `mark_command_confirmed`/`mark_command_failed` idempotency on terminal rows; timeout sweep on back-dated `sent_at`.
  - [x] deliver_one tests updated for id capture (Task 1). Use the existing `InMemoryBackend` + `MockSink` seams (`src/storage/memory.rs`, `chirpstack.rs:4302+`); `#[traced_test]` for audit-event assertions.
  - [x] `cargo test` + `cargo clippy --all-targets -- -D warnings` clean.

- [x] **Task 7 — Docs sync (AC: 9)**
  - [x] `docs/logging.md` (command_confirmed / command_confirm_failed / command_timeout), `docs/architecture.md` (lifecycle + event-stream confirmation path), `docs/manual/opcgw-user-manual.xml` (operator-facing status + timeout knob; `xmllint --noout`), config reference for any knob change, README Planning + `sprint-status.yaml` → E-3 done / Epic E 4/4. SPDX headers on new files.

## Review Findings (iter-1, 2026-06-13)

Code review of `aa31e60` via 3 adversarial layers (Blind Hunter / Edge Case Hunter / Acceptance Auditor). **0 unresolved HIGH/MEDIUM after patches.** 6 patches applied, 1 deferred, 7 dismissed.

- [x] [Review][Patch] **Missing `acknowledged` field actively marked a command Failed (HIGH, blind+edge)** — `parse_ack_event` used `unwrap_or(false)`, so an ack with no `acknowledged` flag took the NACK→`mark_command_failed` path, failing a possibly-delivered command. Fix: absent `acknowledged` now drops the ack (debug `command_ack_dropped reason=missing_acknowledged`) and lets the timeout sweep decide; only an explicit `false` is a NACK. +parse test. [src/chirpstack_events.rs]
- [x] [Review][Patch] **`CommandStatusQuery` loaded the entire unbounded `command_queue` table (MEDIUM, blind+edge)** — the callback fetched ALL commands via `list_commands(default)` then sorted/truncated in memory; the table is never pruned → O(n) per OPC UA read. Fix: new `StorageBackend::recent_commands(limit)` bounds at the query layer (`ORDER BY enqueued_at DESC LIMIT`); `get_command_status_value` uses it. (Also fixes the legacy-NULL-`enqueued_at` nondeterministic sort — SQL orders on the raw column, NULLs last.) [src/storage/mod.rs, sqlite.rs, memory.rs, web/api.rs, opc_ua.rs]
- [x] [Review][Patch] **No test for `get_command_status_value` (AC#6) though Task 6 checked (MEDIUM, auditor)** — added `get_command_status_value_returns_real_command_status` (empty→`[]`; enqueue→sent→confirmed→JSON with status/sent_at/confirmed_at; Good status). [src/opc_ua.rs]
- [x] [Review][Patch] **Ack correlated on `queue_item_id` ignoring the available `device_id` (MEDIUM, blind)** — defence in depth: `handle_ack` now verifies the correlated command's `device_id` matches the stream's device; mismatch → warn `command_ack_device_mismatch` + skip. [src/chirpstack_events.rs]
- [x] [Review][Patch] **Empty enqueue id latent mass-match hazard (MEDIUM→LOW, blind+edge)** — `find_command_by_result_id` now returns `None` for an empty arg in both backends; `deliver_one` warns `command_enqueue_no_result_id` on an empty id (command still Sent → timeout covers it). [src/storage/sqlite.rs, memory.rs, chirpstack.rs]
- [x] [Review][Patch] **`latency_ms` could be negative (LOW, blind)** — clamped to ≥0 against clock skew. [src/chirpstack_events.rs]
- [x] [Review][Defer] **In-memory `find_command_by_result_id` can't see E-0-deliver-path commands (test-fidelity)** — `DeviceCommand` (the `commands` vec) has no `result_id` field, so an in-memory command queued via the E-0 `queue_command` path then `mark_command_sent` stores the id only in the `command_queue` vec it isn't in. Production is SQLite (one unified table) so this is test-only; the faithful deliver→capture test (`deliver_one_captures_result_id_sqlite`) uses SqliteBackend. Deferred — unifying the in-memory dual-vec store is a larger refactor with no prod impact.
- Dismissed (7): dual-vec divergence (ids are unique across both vecs via the shared counter — can't co-exist); `mark_command_sent` terminal guard (only ever called on Pending commands from `deliver_one`); `CommandStatusPoller` periodic scan (intended Task-4 repurpose, bounded by pending count); `find_command_by_result_id` LIMIT-1 uniqueness (queue_item_ids are ChirpStack UUIDs); `sent_at` not stamped on the `commands` DeviceCommand vec (no such field; test-only); `command_timeout` test string drift (test doesn't assert the message); non-streamed-device "Failed" ambiguity (documented intended timeout fallback).

## Review Findings (iter-2, 2026-06-13)

Mandatory re-review of iter-1's new code (the `recent_commands` method, the ack-drop change, device cross-check, empty guards) — 2 adversarial layers on the patch diff. **0 HIGH/MEDIUM regressions.** Blind Hunter verified every patch clean (empirically confirmed SQLite NULL-DESC ordering + LIMIT semantics). 1 patch applied (test), 1 dismissed.

- [x] [Review][Patch] **`recent_commands` cap/ordering untested (LOW, edge ×2)** — the bounded behaviour (the whole point of the new method) and the SQLite path were unverified (the iter-1 OPC UA test only covered InMemory single-command + empty). Added `test_recent_commands_caps_and_orders_newest_first` (105 rows → cap 100, newest-first, oldest excluded, limit 0 → empty, limit > rows → all) on the SQLite backend. [src/storage/sqlite_tests.rs]
- Dismissed (1): "legacy NULL-`enqueued_at` rows masquerade as newest" (MEDIUM, edge) — `get_command_status_value` does **not** serialize `enqueued_at` in its JSON output, and the SQL `ORDER BY enqueued_at DESC` correctly sorts NULLs last, so the `command_from_row` NULL→`Utc::now()` substitution has zero functional impact on either the ordering or the exposed data. Cross-backend NULL-row ordering parity (the LOW corollary) is likewise moot since post-#134 rows always carry `enqueued_at` and the value isn't exposed.

**Loop termination (CLAUDE.md condition #2):** after iter-2, only LOW findings remained and all were patched or dismissed; 0 unresolved HIGH/MEDIUM. The iter-2 change was a **test-only** addition (no new production flow-control), so iter-3 is not mandated (cf. E-2a precedent). Final: `cargo test` 1674/0 across all suites + `cargo clippy --all-targets -- -D warnings` clean + `xmllint` clean. Story → done.

## Dev Notes

### ✅ DECISIONS LOCKED BY MAINTAINER (2026-06-13) — implement exactly these

- **Confirmation path = Option A (hook the existing E-1 `StreamDeviceEvents` consumer).** Extend `chirpstack_events.rs` to dispatch `ack`/`txack` alongside `"up"`; do **not** open a second stream. Repurpose/remove the poll-shaped `CommandStatusPoller` stub (keep `CommandTimeoutHandler`). Extend the injectable `UplinkSource` seam so ack handling stays testable.
- **Unconfirmed-downlink terminal policy = timeout-resolves.** `Confirmed` requires a device `ack(acknowledged=true)` (confirmed downlinks only). Unconfirmed downlinks (no ack will arrive) resolve via the `CommandTimeoutHandler` sweep → `Failed("Confirmation timeout after Ns")`. `txack` is recorded as a debug/diagnostic trace only — it does **not** confirm. Document this meaning of "Confirmed" in `docs/architecture.md` + the manual.

### Architecture decision — event-driven vs the poll-shaped stub (RESOLVED: Option A above; rationale retained)

The `CommandStatusPoller` stub implies a polling design, but ChirpStack has **no per-command ack-polling gRPC**; the ack is an **event** on `StreamDeviceEvents` that E-1 already consumes. Two viable shapes:

- **Option A — hook the existing E-1 stream (RECOMMENDED).** Extend `chirpstack_events.rs` to dispatch `ack`/`txack` alongside `"up"`. One stream, one reconnect/backfill path (already hardened in E-1 with the freshness guard), lowest latency, least new failure surface. `CommandStatusPoller` is then redundant for confirmation → repurpose as a timeout/reconciliation sweep or remove (keeping `CommandTimeoutHandler`). **Caveat:** E-1's stream lives behind the injectable `UplinkSource` seam — extend that seam (or add a sibling) so ack handling stays testable; do not bypass it.
- **Option B — CommandStatusPoller owns a second `StreamDeviceEvents` subscription** filtered to ack/txack. More isolation, but a **second** long-lived stream + duplicated reconnect/backfill/cancellation logic, and ChirpStack replays events per stream (double handling risk). Only choose this if mixing concerns into the uplink consumer proves unclean.

Recommend **Option A**. Whichever is chosen, document it in the story Completion Notes and `docs/architecture.md`, and make the rationale explicit.

### Confirmed vs unconfirmed downlinks — semantics to lock

- `build_queue_item(.., confirmed)` (`chirpstack.rs`) already sets the `confirmed` flag on the queue item. An `AckEvent` is emitted **only for confirmed downlinks**. The Tonhe E20 valve uses confirmed downlinks (sends a conform packet) — so for valves, `ack(acknowledged=true)` is the real `Confirmed`.
- For **unconfirmed** downlinks no ack arrives; `txack` (transmitted) is the strongest available signal. Define the terminal policy (AC#3): recommended — unconfirmed commands resolve via the timeout sweep to `Failed`, OR (cleaner UX) treat `txack` as the terminal `Confirmed`-equivalent for unconfirmed commands and document that "Confirmed" means "device-acked" for confirmed downlinks and "gateway-transmitted" for unconfirmed ones. Pick one, document it, test it. Do **not** leave unconfirmed commands stuck `Sent`.

### Source tree — exact touchpoints (verified 2026-06-13)

| Area | File:line | Current state | Action |
|---|---|---|---|
| Enqueue sink trait + impl | `src/chirpstack.rs:2606,2611-2633` | returns `Result<(),_>`, discards `response.id` (`:2622`) | return `Result<String,_>` = `inner_response.id` |
| Sent transition | `src/chirpstack.rs:2680-2684` (`deliver_one`) | `update_command_status(id, Sent, None)` | call `mark_command_sent(id, &result_id)` |
| Event consumer | `src/chirpstack_events.rs:354-372`, `parse_up_event:428` | filters `description=="up"` only, skips ack/txack (`:363`) | add ack/txack dispatch; keep `"up"` untouched |
| Confirm storage | `src/storage/sqlite.rs` `mark_command_sent:2094`, `mark_command_confirmed:2115`, `mark_command_failed:2140`, `find_pending_confirmations:2174`, `find_timed_out_commands:2200` | all exist; `mark_command_sent` currently dead | wire `mark_command_sent`; add `find_command_by_result_id` |
| Command row mapper | `src/storage/sqlite.rs:431,467,471` | NULL-safe mapper (the #134 fix); `chirpstack_result_id` at col 10 | reuse; no schema change needed |
| Poller stub | `src/chirpstack.rs:2858-2943` (`:~2925` placeholder trace) | logs "would poll ChirpStack" | repurpose/remove (Task 4) |
| Timeout handler | `src/chirpstack.rs:2945-3037` | marks timed-out `Sent`→`Failed` | add `event="command_timeout"` if missing |
| Spawns | `src/main.rs:1087` (poller), `:1104` (timeout) | both spawned | adjust per Task 4 decision |
| OPC UA status node | `src/opc_ua.rs:~1147` (node), `:~1171-1184` (callback) | static placeholder string | snapshot real status as JSON |
| Config knobs | `src/config.rs` `command_delivery_poll_interval_secs`(5), `command_delivery_timeout_secs`(60), `command_timeout_check_interval_secs`(10) | exist | keep/remove poll-interval per Task 4 |
| Proto | `proto/chirpstack/api/device.proto:550` (`EnqueueResponse.id`), `proto/chirpstack/integration/integration.proto:201` (`AckEvent`), `:224` (`TxAckEvent`) | generated | `body` JSON parse, or reuse proto types |

### Previous-story intelligence (E-0 / E-1 / E-2a)

- **E-0** established `DownlinkPayload::{Raw,Object}`, `deliver_one(sink, backend, class, confirmed, cmd)`, the mock `DownlinkSink`, and the `Pending→Sent/Failed` transitions. **Reuse these.** E-0's real-world doctrine applies: the live valve must keep working — after this change, an OPC UA open/close should still actuate AND now show `Confirmed`.
- **E-1** established the `StreamDeviceEvents` consumer with reconnect + **bounded backfill** + a **monotonic freshness guard** (ChirpStack **replays recent events on every reconnect** — this is why AC#4 demands ack idempotency on replay). The consumer sits behind an injectable `UplinkSource` seam with reconnect/precedence tests — extend that seam for ack testability. Do not disturb the `"up"`/no-aggregation/freshness-guard behaviour (E-1 is the v2.2.0 value path).
- **E-2a** delivered the `device_registry` (`DeviceDriver` trait + `ClassRegistry`) and the `command_class` web surface. E-3 does not need the registry directly, but command status flows for class-bound and raw commands alike.

### Conventions & anti-patterns to avoid

- SPDX `// SPDX-License-Identifier: MIT OR Apache-2.0` + `// (c) [2026] Guy Corbaz` on every new `.rs`. Rust 2021, rustc ≥ 1.87. Errors via typed `OpcGwError` (`utils.rs`, `thiserror`); doc-comment public items.
- **No `error.to_string().contains(...)`** control flow (the substring-matcher anti-pattern repeatedly flagged across Epics C/D) — match typed variants.
- **No fake regression guards** (Epic A finding class): a regression-guard test must invoke the function-under-test directly and use seeds whose outputs differ between surviving vs dropped paths. The id-capture guard (AC#1) and the "txack does not confirm" guard (AC#3) are exactly this kind — make them real (assert `chirpstack_result_id` actually set; assert a txack leaves status `Sent`).
- **No NULL-mapping regressions** — the #134 fix made the command row mapper NULL-safe (`sqlite.rs:431,467`). Any new query (`find_command_by_result_id`) must use the same NULL-safe mapper, not a fresh hand-rolled one.
- **Source file size** — `src/chirpstack.rs` is ~4500 lines (under the 5000 limit but close). Prefer putting ack/txack parsing in `chirpstack_events.rs`; if `chirpstack.rs` would cross 5000, extract (per `feedback_source_file_size`).
- **Code-review loop discipline (CLAUDE.md):** after `bmad-dev-story`, run `bmad-code-review` and loop until only LOW findings remain. **iter-N+1 is MANDATORY when iter-N introduces new code** — this story introduces a new event-parse path + correlation logic + a trait signature change, so expect ≥2 iterations. Story flips to `done` only on clean `cargo test` + `clippy -D warnings`.
- **This is the last Epic E story.** On `done`, the very next BMad action is `epic-E-retrospective` (do not start a new epic first — CLAUDE.md BMad discipline), which includes the mandatory security check before the epic closes, then a commit **and `git push`**.

### Testing standards

- Unit tests inline under `#[cfg(test)]`; integration tests in `tests/*.rs`. Mock `DownlinkSink` + `InMemoryBackend` for delivery tests. Timeout tests: insert a row with back-dated `sent_at` (no sleeping, no clock injection — `find_timed_out_commands` compares against `Utc::now()`). `#[traced_test]` for audit-event log assertions. SQLite tests use `temp_backend_path()` (`sqlite_tests.rs`). DocBook validated with `xmllint --noout`.

### Project structure notes

- Keep ack/txack decode in `src/chirpstack_events.rs` (alongside the stream it rides on). Add the correlation/transition glue there or in a small helper; reuse `Arc<dyn StorageBackend>` already held by that task.
- Add `find_command_by_result_id` to the `StorageBackend` trait + `SqliteBackend` + `InMemoryBackend` impls (all three, or the build breaks).

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story-E.3] — scope summary + end-to-end AC.
- [Source: src/chirpstack.rs#enqueue_downlink, #deliver_one, #CommandStatusPoller, #CommandTimeoutHandler] — the send path, the stub, the timeout sweep.
- [Source: src/chirpstack_events.rs] — E-1 StreamDeviceEvents consumer to extend.
- [Source: src/storage/sqlite.rs] — `mark_command_sent`/`mark_command_confirmed`/`mark_command_failed`/`find_pending_confirmations`/`find_timed_out_commands` + NULL-safe row mapper (#134).
- [Source: proto/chirpstack/api/device.proto:550] — `EnqueueDeviceQueueItemResponse.id`. [Source: proto/chirpstack/integration/integration.proto:201,224] — `AckEvent` / `TxAckEvent`.
- [Source: src/opc_ua.rs#CommandStatusQuery] — read callback to wire.
- [Source: memory project_device_abstraction_valves.md] — locked design (E-3 = confirmation poller). [Source: memory session_2026_06_12_..._134] — the #134 NULL-row fix this story builds on.
- GitHub: [#129 Epic E](https://github.com/guycorbaz/opcgw/issues/129).

## Dev Agent Record

### Agent Model Used

claude-opus-4-8 (1M context)

### Debug Log References

- `cargo test` (full suite): 1670 passed / 0 failed across 35 test binaries.
- `cargo clippy --all-targets -- -D warnings`: clean.
- `xmllint --noout docs/manual/opcgw-user-manual.xml`: clean.
- New tests: `cargo test --lib chirpstack_events` → 23 passed (incl. 6 new ack/txack); storage idempotency/race + deliver_one result-id capture green.

### Completion Notes List

**Architecture decisions (locked by Guy 2026-06-13):** Option A (hook the existing E-1 `StreamDeviceEvents` consumer — one stream, no second subscription) + timeout-resolves-unconfirmed (Confirmed requires a device `ack`; `txack` is diagnostic-only; unconfirmed downlinks fail via the timeout sweep).

**What landed:**
- **AC#1 — enqueue id capture.** `DownlinkSink::enqueue_downlink` now returns `Result<String, _>` (the queue-item UUID from `EnqueueDeviceQueueItemResponse.id`); `deliver_one` calls `mark_command_sent(id, &result_id)` (was `update_command_status(.., Sent, None)`), persisting `chirpstack_result_id`. `mark_command_sent` is no longer dead code. Faithful SQLite round-trip test (`deliver_one_captures_result_id_sqlite`).
- **AC#2/3 — event-driven confirmation.** `chirpstack_events.rs`: `UplinkStream::next_event` now yields a `DeviceEvent` enum (`Uplink`/`Ack`/`TxAck`); `GrpcUplinkStream` parses `up`/`ack`/`txack` (`parse_device_event` + serde-alias `AckEventBody`/`TxAckEventBody` accepting both camelCase + snake_case). `handle_ack` correlates `queue_item_id == chirpstack_result_id` via the new `find_command_by_result_id` and marks `Confirmed` (ack true) / `Failed` (NACK). `txack` → debug `command_txack` only.
- **AC#4 — idempotency.** Relies on the storage `status IN ('Sent','Pending')` guard; unmatched/duplicate/terminal acks are benign no-ops (`command_ack_unmatched` / `command_confirm_noop`). In-memory backend `mark_command_*` now mirror SQLite's guard and dual-update both command vecs (test-fidelity for the split DeviceCommand/Command stores).
- **AC#5 — timeout.** `CommandTimeoutHandler` emits the `command_timeout` audit event; confirm-vs-timeout race resolves to one terminal transition (storage guard) — `command_timeout_noop` on the losing side. SQLite test `test_confirm_removes_from_timeout_eligibility`.
- **AC#6 — OPC UA.** `CommandStatusQuery` read callback wired to `get_command_status_value` (snapshots recent commands as JSON: id/device/name/status/sent_at/confirmed_at/error). Read-only, bounded to 100 newest, fail-soft to `[]`.
- **AC#7 — audit.** `command_confirmed` (info, with `latency_ms`), `command_confirm_failed` (warn), `command_timeout` (warn) + diagnostics. `CommandStatusPoller` repurposed as a confirmation-backlog observability heartbeat (placeholder removed; no ChirpStack polling).
- **AC#8 — no regression.** E-0 raw path + E-1 `up`/no-aggregation/freshness-guard untouched; full suite green.
- **AC#9 — docs.** `docs/logging.md` (11 new `command_*` events), `docs/architecture.md` (command lifecycle + shared stream), DocBook manual (confirmation + timeout knob), README Epic E row. No new config knob (existing timeout knob reused), no schema migration (columns already exist via v002).
- **AC#10 — tests + clippy.** +17 tests; `cargo test` 1670/0; clippy `-D warnings` clean.

**Scope note:** confirmation observation requires the device to be streamed (valve-class, or `chirpstack.stream_all_devices`). Valves ARE valve-class → streamed, so the driver works out of the box. A command on a non-streamed device resolves via the timeout sweep (documented in architecture.md + Dev Notes).

### File List

- `src/chirpstack.rs` (modified) — `enqueue_downlink` returns the queue-item id; `deliver_one` persists it via `mark_command_sent`; `CommandStatusPoller` repurposed (placeholder removed); `CommandTimeoutHandler` emits `command_timeout`; MockSink returns stub id; +`deliver_one_captures_result_id_sqlite` test
- `src/chirpstack_events.rs` (modified) — `DeviceEvent`/`AckInfo`/`TxAckInfo`; `UplinkStream::next_event` → `DeviceEvent`; `parse_device_event` + `parse_ack_event`/`parse_txack_event`; `handle_ack`; stream-loop dispatch; +6 ack/txack tests + scripted-source `Device` variant
- `src/storage/mod.rs` (modified) — `find_command_by_result_id` on the `StorageBackend` trait
- `src/storage/sqlite.rs` (modified) — `find_command_by_result_id` impl (NULL-safe shared mapper)
- `src/storage/memory.rs` (modified) — `find_command_by_result_id`; `mark_command_*` dual-update both vecs + SQLite-matching terminal-state guard
- `src/web/api.rs` (modified) — `find_command_by_result_id` stub on `FailingBackendForApiTests`
- `src/opc_ua.rs` (modified) — `CommandStatusQuery` read callback wired to storage via new `get_command_status_value`
- `src/storage/sqlite_tests.rs` (modified) — +3 E-3 storage tests (result-id round-trip, confirm idempotency, confirm-removes-timeout-eligibility)
- `docs/logging.md` (modified) — 11 new `command_*` event rows
- `docs/architecture.md` (modified) — command lifecycle + shared-stream confirmation
- `docs/manual/opcgw-user-manual.xml` (modified) — confirmation + delivery-timeout knob
- `README.md` (modified) — Epic E row: E-3 implemented (in review)
- `_bmad-output/implementation-artifacts/E-3-command-delivery-confirmation.md` (this story file)
- `_bmad-output/implementation-artifacts/sprint-status.yaml` (status → review)

### Change Log

- 2026-06-13: Implemented E-3 Command Delivery Confirmation (all 7 tasks). Status ready-for-dev → review.

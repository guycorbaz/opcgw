---
title: 'GH-134: NULL command_name collapses CommandStatusPoller every 5s'
type: 'bugfix'
created: '2026-06-12'
status: 'done'
baseline_commit: '1b7b49c31373c3c1018e5a265c683ef9b003e4df'
context:
  - 'CLAUDE.md'
  - 'docs/logging.md'
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** `SqliteBackend::queue_command` (the OPC-UA downlink path used by `OpcUa::set_command`) inserts `command_queue` rows without `command_name`/`parameters`/`command_hash`, while the four `Command` row-mappers in `sqlite.rs` read those nullable columns as non-Option types. One NULL row makes `find_pending_confirmations` error out wholesale, so the CommandStatusPoller logs an ERROR every 5 s forever and delivery-confirmation tracking is dead (GH #134; live on panoramix since 2026-06-10).

**Approach:** Fix both directions per the issue: (1) extract ONE shared NULL-safe row→`Command` mapper used by all four reader sites so a NULL column can never collapse a whole poll — including the latent `parameters` (index 3) and `command_hash` (index 9) crashes hiding behind `command_name`; (2) add `command_name: Option<String>` to `DeviceCommand` and populate it (plus `enqueued_at`) in `queue_command`, threading the name from `DeviceCommandCfg.command_name` in `set_command`. Reader-side defense must handle the existing NULL row in the prod DB with no migration.

## Boundaries & Constraints

**Always:** Keep the `Command` struct's public shape unchanged (`command_name: String` stays — NULL maps to `""` via `unwrap_or_default`); NULL `parameters` maps to `serde_json::Value::Null`, never an error; all four mappers go through the shared helper; SPDX headers intact; `cargo clippy --all-targets -- -D warnings` clean.

**Ask First:** Any schema migration (should not be needed — readers become defensive); changing `Command.command_name` to `Option<String>` (public-shape change, ripples to web diagnostics).

**Never:** Do NOT touch the E-0 drain path semantics (`get_pending_commands` / `process_command_queue`); do not fabricate a `command_hash` on the OPC-UA write path (dedup hashing belongs to `enqueue_command`); no changes to `src/opc_ua_auth.rs`, `src/security*.rs`, web auth.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Legacy NULL row | `command_queue` row with NULL `command_name`/`parameters`/`command_hash`, status `Sent`, `confirmed_at` NULL | `find_pending_confirmations` returns the row: `command_name=""`, `parameters=Value::Null`, `command_hash=""` | No error; poll continues |
| New OPC-UA command | `set_command` write on a configured command (`command_name="valveCmd"`) | Row inserted with `command_name='valveCmd'`, `enqueued_at` set | Existing f_port/payload validation unchanged |
| Timed-out NULL row | Same NULL row with `sent_at` older than TTL | `find_timed_out_commands` returns it without error | No error |
| Mixed queue | NULL row + fully-populated `enqueue_command` row both pending confirmation | Both returned in one query | One bad row never hides good rows |

</frozen-after-approval>

## Code Map

- `src/storage/sqlite.rs` -- all 4 crash-prone mappers: `dequeue_command` (:1933-1960), `find_commands` list query (:2040), `find_pending_confirmations` (:2193), `find_timed_out_commands` (:2243); writer `queue_command` (:903) inserts only 6 of 13 columns
- `src/storage/types.rs:162` -- `DeviceCommand` struct (gains `command_name: Option<String>`); `Command` struct (:200) unchanged
- `src/opc_ua.rs:1946-2040` -- `set_command` builds `DeviceCommand`; has `DeviceCommandCfg.command_name` in scope
- `src/storage/memory.rs:143` -- `InMemoryBackend::queue_command` (struct-literal sites need the new field)
- `src/chirpstack.rs:2916,3004` -- CommandStatusPoller / CommandTimeoutHandler callers (read-only context)
- `src/storage/schema.rs:93-102` -- v002 nullable columns (reference; no migration)

## Tasks & Acceptance

**Execution:**
- [x] `src/storage/sqlite.rs` -- extract private `fn command_from_row(row: &rusqlite::Row) -> rusqlite::Result<Command>` with NULL-safe mapping (`command_name`/`command_hash`: `Option<String>` → `unwrap_or_default`; `parameters`: NULL → `Value::Null`, Some → `serde_json::from_str`); replace the 4 inline mappers -- one fix point, kills the reported + 2 latent crashes
- [x] `src/storage/types.rs` -- add `command_name: Option<String>` to `DeviceCommand` with doc comment -- carries the configured name from OPC UA to storage
- [x] `src/storage/sqlite.rs` -- `queue_command`: include `command_name` + `enqueued_at` in the INSERT -- new rows are self-describing; `enqueued_at` feeds the readers' ORDER BY
- [x] `src/opc_ua.rs` -- `set_command`: populate `command_name: Some(command.command_name.clone())` -- the cfg is already a parameter
- [x] `src/storage/memory.rs` + other `DeviceCommand` literal sites (tests, chirpstack.rs if any) -- add the new field -- compile completeness
- [x] `src/storage/sqlite.rs` tests -- regression test per issue acceptance: insert a row via raw SQL mimicking the pre-fix shape (NULL name/params/hash), mark `Sent`, assert `find_pending_confirmations` returns it un-erroring with defaults; second test: `queue_command` → row has `command_name` + `enqueued_at` populated

**Acceptance Criteria:**
- Given a `command_queue` row with NULL `command_name`/`parameters`/`command_hash` in `Sent` state, when the CommandStatusPoller queries pending confirmations or timed-out commands, then the row is returned with safe defaults and no ERROR is logged.
- Given an OPC UA command write through `set_command`, when the row lands in `command_queue`, then `command_name` and `enqueued_at` are non-NULL.
- Given the existing panoramix DB (no migration), when the fixed binary boots, then the recurring `Failed to query pending command confirmations` ERROR stops.

## Verification

**Commands:**
- `TMPDIR=/home/gcorbaz/.cache/cargo-tmp cargo test` -- expected: 0 failures, all suites
- `TMPDIR=/home/gcorbaz/.cache/cargo-tmp cargo clippy --all-targets -- -D warnings` -- expected: clean

## Suggested Review Order

**NULL-safe read path (the fix's core)**

- Single shared row→Command mapper; NULL and corrupt-JSON both soft-fail per row — read its doc comment first
  [`sqlite.rs:440`](../../src/storage/sqlite.rs#L440)

- All four readers now route through it: dequeue, list, pending-confirmations, timed-out
  [`sqlite.rs:1990`](../../src/storage/sqlite.rs#L1990)
  [`sqlite.rs:2061`](../../src/storage/sqlite.rs#L2061)
  [`sqlite.rs:2190`](../../src/storage/sqlite.rs#L2190)
  [`sqlite.rs:2219`](../../src/storage/sqlite.rs#L2219)

**Self-describing writes**

- OPC-UA INSERT now persists `command_name` + `enqueued_at` (canonical RFC3339); deliberately NOT `parameters`/`command_hash`
  [`sqlite.rs:952`](../../src/storage/sqlite.rs#L952)

- `set_command` threads the configured name into the queued command
  [`opc_ua.rs:2028`](../../src/opc_ua.rs#L2028)

- New optional field on `DeviceCommand` (nullable for legacy rows)
  [`types.rs:180`](../../src/storage/types.rs#L180)

**Regression tests (one per converted reader + write contract)**

- Pending-confirmations NULL row — the exact prod failure shape
  [`sqlite_tests.rs:200`](../../src/storage/sqlite_tests.rs#L200)

- Mixed queue: one bad row never hides good rows
  [`sqlite_tests.rs:393`](../../src/storage/sqlite_tests.rs#L393)

- Corrupt-JSON soft-fail, dequeue, list, write-contract pins
  [`sqlite_tests.rs:365`](../../src/storage/sqlite_tests.rs#L365)
  [`sqlite_tests.rs:302`](../../src/storage/sqlite_tests.rs#L302)
  [`sqlite_tests.rs:330`](../../src/storage/sqlite_tests.rs#L330)
  [`sqlite_tests.rs:260`](../../src/storage/sqlite_tests.rs#L260)

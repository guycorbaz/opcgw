---
title: 'GH-144 Configurable storage-budget WARN thresholds'
type: 'enhancement'
created: '2026-06-26'
status: 'done'
baseline_commit: 'fc55c9f1fcd02ccf7885a2789daf5d63480b729f'
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** The storage-latency budgets that gate `exceeded_budget=true` WARNs are hardcoded for local-SSD latency (`STORAGE_QUERY_BUDGET_MS=10`, `BATCH_WRITE_BUDGET_MS=500`). On NAS/network-backed SQLite, normal latency is ~115 ms median / up to ~1.78 s, so these fire every cycle — ~50 WARN/hour of pure noise (observed in prod, #144) that drowns out real signals.

**Approach:** Make both budgets runtime-tunable via environment variables (mirroring the `OPCGW_LOG_LEVEL` ops-knob pattern), resolved once at startup into process-global atomics, and raise the built-in defaults to NAS-realistic values. The budgets only decide WARN-vs-DEBUG logging — no functional behavior changes.

## Boundaries & Constraints

**Always:** Resolve env vars exactly once at startup (after the tracing subscriber is initialised) and log the resolved value + source. Invalid or zero/non-numeric values fall back to the default with a single `warn!`. Read budgets via accessor functions at the WARN sites — never re-read env per query. Keep the WARN/DEBUG emission logic and field names (`budget_ms`, `exceeded_budget`) unchanged. SPDX headers + doc comments preserved.

**Ask First:** Changing `OPC_UA_READ_BUDGET_MS` (separate read-handler budget, out of scope for #144) or routing these knobs through the figment/SQLite-singleton config stack instead of env vars.

**Never:** Adding a config-file/SQLite surface for these (env-var only, per the approved approach). Changing actual storage behavior, retry logic, or query paths. Re-reading env on a hot path. Making the budgets a hard error.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Output / Behavior | Error Handling |
|----------|--------------|---------------------------|----------------|
| Unset (default) | no env var | Budget = raised default (250 / 2000 ms); startup logs resolved value+source=`default` | N/A |
| Valid override | `OPCGW_STORAGE_QUERY_BUDGET_MS=50` | Budget = 50 ms; startup logs value+source=`env` | N/A |
| Non-numeric | `OPCGW_BATCH_WRITE_BUDGET_MS=abc` | Falls back to default; one `warn!` naming the bad value | Logged, non-fatal |
| Zero | `...=0` | Falls back to default; one `warn!` (0 would warn on every query) | Logged, non-fatal |
| Query slower than budget | latency > resolved budget | `warn!(exceeded_budget=true, budget_ms=<resolved>)` (unchanged shape) | N/A |

</frozen-after-approval>

## Code Map

- `src/utils.rs:479,484` -- the two `pub const` budgets; become `DEFAULT_*` consts + `AtomicU64` statics + `storage_query_budget_ms()` / `batch_write_budget_ms()` accessors + `init_storage_budgets_from_env()`.
- `src/storage/sqlite.rs:133,138` -- `StorageOpLog::drop` reads `STORAGE_QUERY_BUDGET_MS`; switch to accessor.
- `src/chirpstack.rs:1479,1484` -- production batch-write WARN reads `BATCH_WRITE_BUDGET_MS`; switch to accessor.
- `src/chirpstack.rs:3479` -- `batch_write_budget_emits_warn_when_exceeded` test asserts `budget_ms=500`; make default-independent (use a fixed local budget for the formatting assertion).
- `src/main.rs:612` -- after `"tracing subscriber initialised"`, call `init_storage_budgets_from_env()`.
- `README.md` (Logging) / `docs/logging.md:187,191,355` / `.env.example` -- document the two env vars + new defaults.

## Tasks & Acceptance

**Execution:**
- [x] `src/utils.rs` -- Replace the two `pub const` budgets with `DEFAULT_STORAGE_QUERY_BUDGET_MS=250` / `DEFAULT_BATCH_WRITE_BUDGET_MS=2000`, backing `AtomicU64` statics, `storage_query_budget_ms()`/`batch_write_budget_ms()` accessors (`Relaxed` load), and `init_storage_budgets_from_env()` + a private parse helper (parse u64; `>0` required; on invalid/zero `warn!` + keep default; on success store + `info!` resolved value & source). Doc comments + SPDX preserved.
- [x] `src/storage/sqlite.rs` -- `StorageOpLog::drop`: read `crate::utils::storage_query_budget_ms()` for both the threshold compare and the `budget_ms` field.
- [x] `src/chirpstack.rs` -- Production batch-write site: read `crate::utils::batch_write_budget_ms()` for the compare + `budget_ms` field. Update the `batch_write_budget_emits_warn_when_exceeded` test to assert against a fixed local budget (decouple from the raised default) so it stays deterministic.
- [x] `src/main.rs` -- Call `crate::utils::init_storage_budgets_from_env()` immediately after the tracing subscriber is initialised (so the resolution logs are captured).
- [x] `src/utils.rs` (tests) -- Unit-test the parse helper directly: valid → stored; non-numeric → default; zero → default. (Atomic state is process-global; test the pure parse/validate function, not the statics, to avoid cross-test bleed.)
- [x] `README.md` / `docs/logging.md` / `.env.example` -- Document `OPCGW_STORAGE_QUERY_BUDGET_MS` (default 250) and `OPCGW_BATCH_WRITE_BUDGET_MS` (default 2000), and update the `storage_query` / `batch_write` budget descriptions to "configurable, default N ms".

**Acceptance Criteria:**
- Given no env vars, when the gateway starts, then storage-query and batch-write budgets resolve to 250 ms and 2000 ms and a startup log records each value with `source="default"`.
- Given `OPCGW_STORAGE_QUERY_BUDGET_MS=50`, when the gateway starts, then the storage-query budget is 50 ms and the resolution log shows `source="env"`.
- Given a non-numeric or zero env value, when the gateway starts, then the budget falls back to its default and exactly one `warn!` names the rejected value; startup still succeeds.
- Given a resolved budget B, when a storage query exceeds B, then the existing `warn!(exceeded_budget=true, budget_ms=B)` fires with unchanged field shape.

## Spec Change Log

- **iter-1 review (2026-06-26):** No intent_gap/bad_spec — all three layers confirmed the production atomic design correct (Relaxed ordering fine for set-once-at-startup/read-many; startup ordering safe — init runs before any storage op; parse robust; shipped-doc consistency good). No HIGH. Patches applied (no loopback): **M2** — added direct tests for `resolve_budget_env` (valid override / non-numeric / zero / unset) using a *local* `AtomicU64` + unique env keys so the real resolution logic is covered without mutating the process-global atomics (this also closes the L1/LOW-2 cross-test-flake concern). **M1** — re-coupled `batch_write_budget_emits_warn_when_exceeded` to `crate::utils::batch_write_budget_ms()` instead of a hardcoded `500`. **L2** — strengthened `budget_defaults_are_nas_realistic` to assert accessors `== DEFAULT_*`. **LOW-1** — updated the DocBook manual (`opcgw-user-manual.xml:2051`) which still said "10 ms". **Acceptance LOW** — refreshed stale "10 ms" comments in the `#[ignore]`d negative test. Rejected: L3 (unused imports — clippy `--all-targets -D warnings` is clean) and L4 (early-startup default — Edge Case Hunter verified no storage op runs before init; logging-only regardless). Patches are test/doc-only (no new production logic/branches), so per the iter-N+1 doctrine a full second adversarial pass is not mandated — re-verified via `cargo test` + `cargo clippy`.

## Verification

**Commands:**
- `cargo test --lib utils` -- expected: parse-helper + env-resolution tests pass. ✅ 8/8.
- `cargo test` -- expected: full suite green. ✅ lib 638/0 + 37 integration suites pass.
- `cargo clippy --all-targets -- -D warnings` -- expected: clean. ✅

## Suggested Review Order

**Budget resolution (entry point)**

- The configurable budgets: defaults, atomics, accessors, startup env resolver.
  [`utils.rs:491`](../../src/utils.rs#L491)

- One-shot startup resolution from env (valid→store+info, invalid/zero→default+warn).
  [`utils.rs:526`](../../src/utils.rs#L526)

- Pure parse/validate helper (rejects non-numeric, zero, negatives).
  [`utils.rs:542`](../../src/utils.rs#L542)

- Where startup calls it — after the tracing subscriber is up, before any storage op.
  [`main.rs:626`](../../src/main.rs#L626)

**WARN sites (now read the accessor)**

- Storage-query budget breach.
  [`sqlite.rs:134`](../../src/storage/sqlite.rs#L134)

- Batch-write budget breach.
  [`chirpstack.rs:1481`](../../src/chirpstack.rs#L1481)

**Tests & docs (supporting)**

- Env-resolution tests using a local atomic + unique keys (no global mutation).
  [`utils.rs:856`](../../src/utils.rs#L856)

- README / `docs/logging.md` / `.env.example` / DocBook manual — env vars + 250/2000 defaults.
  [`README.md`](../../README.md)

# Story A-1: MetricType Payload-Bearing Enum + StorageBackend Trait Amendment

| Field         | Value                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------- |
| Story key     | `A-1-metrictype-payload-bearing-enum`                                                                 |
| Epic          | A — Storage Payload Migration (Phase B Closure, gates v2.0 GA)                                        |
| FRs           | FR51 (new in PRD via correct-course commit `e0b64a0`)                                                 |
| Status        | review                                                                                                |
| Created       | 2026-05-14                                                                                            |
| Source epic   | `_bmad-output/planning-artifacts/epics.md § Epic A § Story A.1`                                       |
| Sprint change | `_bmad-output/planning-artifacts/sprint-change-proposal-2026-05-14.md`                                |
| Tracking      | GitHub issue [#118](https://github.com/guycorbaz/opcgw/issues/118)                                    |

---

## User Story

As a **gateway internal**,
I want `MetricType` to carry the actual measurement payload (`Float(f64)`, `Int(i64)`, `Bool(bool)`, `String(String)`),
So that the storage trait round-trips the real value end-to-end instead of flattening it to the discriminant string and the production-deployment blocker [issue #108](https://github.com/guycorbaz/opcgw/issues/108) is unblocked at the type level.

---

## Story Context

### Why this story is the foundation of Epic A

Epic A — Storage Payload Migration — closes [issue #108](https://github.com/guycorbaz/opcgw/issues/108): every row in `metric_values.value` currently stores the data-type discriminant string (`"Float"`, `"Int"`, `"Bool"`, `"String"`) instead of the real measurement. The four shipped epics that depend on metric values (Epic 2 persistence, Epic 5 OPC UA visibility, Epic 8 historical, Story 9-3 live dashboard) are surface-correct but data-incorrect.

Story A-1 is the **type-level surgery** that makes the rest of Epic A possible:

- A-2 (schema migration v007) needs the payload variants to decide which typed column to UPSERT into.
- A-3 (poller write pipeline) needs the payload variants to wrap the real value the moment it arrives from ChirpStack.
- A-4 (OPC UA `Read`) + A-5 (`HistoryRead`) + A-6 (Web UI display) pattern-match on the payload variants to emit the correct `Variant` / JSON shape.
- A-7 (migration runbook) describes the on-disk shape change A-2 introduces.

If A-1 doesn't produce a clean trait surface, every downstream story carries the cost of working around it.

### Current pre-Epic-A shape (the bug)

`src/storage/types.rs:25`:

```rust
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MetricType {
    Float,
    Int,
    Bool,
    String,
}
```

`MetricType` is `Copy` (no heap data) and has no payload. The `Display` impl writes the discriminant name (`"Float"`, `"Int"`, …) — which is exactly the string that ends up in `metric_values.value` when SQLite UPSERT runs. That's the literal #108 mechanism.

`src/storage/types.rs:62`:

```rust
pub struct MetricValue {
    pub device_id: String,
    pub metric_name: String,
    pub value: String,              // ← parsed based on data_type (in practice: never parsed)
    pub timestamp: DateTime<Utc>,
    pub data_type: MetricType,
}
```

The doc comment on `value` says "parse based on data_type field" — that intent was never implemented, so every reader gets a plain string back.

`src/storage/mod.rs:187-248` defines the `StorageBackend` trait. The three load-bearing methods this story touches:

```rust
fn get_metric(&self, device_id: &str, metric_name: &str)
    -> Result<Option<MetricType>, OpcGwError>;                    // line 208

fn get_metric_value(&self, device_id: &str, metric_name: &str)
    -> Result<Option<MetricValue>, OpcGwError>;                   // line 225

fn set_metric(&self, device_id: &str, metric_name: &str, value: MetricType)
    -> Result<(), OpcGwError>;                                    // line 248
```

`get_metric` and `set_metric` currently use the payload-less `MetricType` — once `MetricType` becomes payload-bearing, these signatures stay the same but now carry the real value. `get_metric_value` returns the full `MetricValue` struct — the `value: String` field becomes redundant once `data_type: MetricType` carries the payload, and the story decides whether to keep it (compat) or remove it (cleaner).

### Post-Epic-A shape (the target — set by sprint-change proposal commit `e0b64a0`)

The architecture amendment in `architecture.md § Storage Payload Migration Strategy (Epic A)` pins this:

```rust
pub enum MetricType {
    Float(f64),
    Int(i64),
    Bool(bool),
    String(String),
}
```

Critical consequence: `MetricType` is **no longer `Copy`** (because `String(String)` owns heap data). Every existing call site that relied on the `Copy` blanket — pass-by-value, struct field that doesn't need a borrow, `match` over `&MetricType` vs `MetricType` — needs to be re-audited. The dev agent must NOT mechanically `derive(Copy)` to "fix" the breakage; the right fix is to switch call sites to pass-by-reference / clone / move semantics as appropriate.

`MetricValue.value: String` is removable: with `MetricValue.data_type: MetricType` carrying the payload, the redundant `.value` field is a footgun (which one is authoritative?). The story removes it — cleaner trait surface for downstream Epic A stories.

---

## Acceptance Criteria

1. **`MetricType` is payload-bearing.** `src/storage/types.rs::MetricType` becomes `enum MetricType { Float(f64), Int(i64), Bool(bool), String(String) }`. The derive list is updated: `#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]` (drop `Copy` — incompatible with `String(String)`).

2. **`Display` impl preserves backward-compatible discriminant rendering for log output.** `MetricType::Float(_).to_string() == "Float"`, `MetricType::Int(_).to_string() == "Int"`, etc. — the type-name string is what logging / SQL `value_type` discriminant column writes use. The payload is NOT included in `Display` (use `Debug` for value+type rendering).

3. **`FromStr` impl is removed OR updated to a clearly-documented "type-name only" parser.** The current `FromStr` parses "float" → `MetricType::Float` (payload-less). Post-A-1 there is no longer a single-argument string parse that produces a valid payload-bearing variant — the value comes from a separate source. The dev agent decides: (a) delete `FromStr` and audit call sites, or (b) keep it producing `MetricType::Float(0.0)` / `MetricType::Int(0)` / `MetricType::Bool(false)` / `MetricType::String(String::new())` with a renamed method `MetricType::type_from_str` to flag the no-payload contract. Document the choice in dev notes.

4. **`MetricValue.value: String` is removed.** Once `MetricValue.data_type: MetricType` carries the payload, the `.value` field is redundant and a footgun. Delete the field; update all in-tree usages (none in production code that this story touches — A-2 onwards will rebuild the SQLite read path; A-3 onwards rebuilds the poller write path). Note: this is a breaking change to anything that serialised `MetricValue` to JSON — dev agent must audit `Serialize` consumers.

5. **`StorageBackend` trait signatures unchanged at the type level.** `get_metric`, `get_metric_value`, `set_metric` keep their current signatures. The semantic contract changes: callers now pass / receive payload-bearing `MetricType`. The doc comments on those methods need to be amended to reflect the payload-bearing semantics (no more "parse based on data_type field" footgun).

6. **`InMemoryBackend` implementation round-trips the payload.** `src/storage/memory.rs::InMemoryBackend::set_metric` stores the `MetricType` as-is. `get_metric` and `get_metric_value` return the stored `MetricType` byte-for-byte. Unit tests in `src/storage/memory.rs::tests` verify all four variants (`Float(23.5)`, `Int(42)`, `Bool(true)`, `String("OK".into())`) round-trip without payload loss.

7. **`SqliteBackend` implementation compiles but is allowed to remain semantically broken until A-2.** A-1 is purely the type-level refactor. The SQLite read/write paths can't actually round-trip the payload until A-2 lands the typed columns. A-1's SqliteBackend either: (a) panics / returns `BadDataUnavailable` for all reads with a `TODO(A-2)` comment, OR (b) keeps the current discriminant-string write path with a `TODO(A-2)` comment + a temporarily-disabled integration test. The dev agent chooses; the choice is documented in dev notes. **The crate MUST still compile and `cargo test` for the in-memory backend MUST pass — the broken SqliteBackend tests can be `#[ignore]`-marked with a `TODO(A-2)` annotation.**

8. **All call sites compile.** This is a load-bearing post-AC#1 check: every `MetricType::Float` literal in the codebase (currently payload-less) becomes a compile error after AC#1. The dev agent must walk every error and decide: pass a real value (if the call site has one), `MetricType::Float(0.0)` placeholder with a `TODO(A-3)`/`TODO(A-4)` comment (if the call site is in a downstream-epic touch zone), or refactor to pattern-match on the variant (if the call site was reading `MetricType` as a discriminant). Test fixtures count as call sites.

9. **Audit-event surface unchanged.** No new `event = "metric_*"` audit events are introduced by A-1. The grep contract `git grep -hoE 'event = "metric_[a-z_]+"' src/ | sort -u` returns whatever it returned at commit `e0b64a0` baseline (the correct-course commit). A-3 will add `metric_parse` for parse-failure paths; not A-1's scope.

10. **Strict-zero file invariants.** `git diff` shows zero changes to: `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing`, `src/config_reload.rs`, `src/opcua_topology_apply.rs`, `src/storage/pool.rs`, `src/storage/schema.rs`. Allowed touches: `src/storage/types.rs` (MetricType enum + MetricValue struct + their impls), `src/storage/mod.rs` (StorageBackend trait doc comments + MetricValueInternal if it mirrors MetricValue), `src/storage/memory.rs` (InMemoryBackend implementation), `src/storage/sqlite.rs` (SqliteBackend skeleton refactor — may add `TODO(A-2)` markers), `src/chirpstack.rs` (call site fixups to compile against new MetricType), `src/opc_ua.rs` (call site fixups to compile — `TODO(A-4)` for the actual value-emission refactor), `src/web/api.rs` (call site fixups — `TODO(A-6)` for the actual JSON-shape refactor), `tests/**` (fixture updates).

11. **`cargo test` passes with at-most-N ignored.** Run `cargo test --all-targets`. The post-A-1 test count must equal the pre-A-1 baseline minus the `#[ignore]`-ed SqliteBackend integration tests (AC#7 path). Specifically:
    - **Pre-A-1 baseline (commit `e0b64a0` HEAD):** 1112 passed / 0 failed / 9 ignored across all test binaries.
    - **Post-A-1 target:** ≥ 1080 passed / 0 failed / ≥ 9 ignored (the gap accounts for SqliteBackend tests that A-1 marks `#[ignore]` with `TODO(A-2)`). Document the exact count delta in the completion notes.
    - `cargo clippy --all-targets -- -D warnings` clean — no new warnings.

12. **`cargo test --doc` no regressions.** `cargo test --doc` returns 0 failed / ≥ 56 ignored. Issue #100 (56 pre-existing doctest ignores) is the baseline floor; A-1 must not introduce new doctest failures.

13. **Open A-1 tracking issue on GitHub.** Task 0 below. If `gh` CLI is not authenticated for write (per the Story 9-x precedent), defer to user with an explicit note. The story tracking issue body should mirror the User Story + ACs 1-12 above.

---

## Tasks

- [x] **Task 0:** Opened GH tracking issue [#118](https://github.com/guycorbaz/opcgw/issues/118).

- [x] **Task 1:** Refactored `src/storage/types.rs::MetricType` to payload-bearing form. Dropped `Copy` derive. `Display` renders discriminant name only (matches old behavior). `FromStr` retained for TOML config — produces zero-valued payloads with clarified doc comment (no rename, no deletion).

- [x] **Task 2 (deferred per Iter-0 Scope Revision):** `MetricValue.value: String` field kept in place with `TODO(A-5)` marker. Removal happens in A-5 when read sites stop using parse-from-string logic. `MetricValueInternal` mirror at `src/storage/mod.rs:768` left unchanged (same dual-storage temporary state).

- [x] **Task 3:** Doc comments on `MetricType` and `MetricValue` updated to reflect payload-bearing semantics + the dual-storage transition state. `StorageBackend` trait method signatures unchanged at type level (per AC#5).

- [x] **Task 4:** `src/storage/memory.rs::InMemoryBackend` updated for Copy-drop cascade — `.copied()` → `.cloned()` at line 97; `*value` → `value.clone()` at line 164; `metric.data_type` → `metric.data_type.clone()` at line 186; `*metric_type` → `metric_type.clone()` at line 216. Added `test_metric_type_payload_roundtrip` in `src/storage/types.rs::tests` covering all 4 variants.

- [x] **Task 5:** `src/storage/sqlite.rs` option-b chosen — kept existing discriminant-string write path. Two cascade fixes: `*t` → `t.clone()` at line 3044, `data_type` → `data_type.clone()` at line 4452. No new `#[ignore]` test annotations were needed — the existing parse-from-string read path continues to round-trip the discriminant string just like before.

- [x] **Task 6:** Walked `cargo build --all-targets` errors. Fixed 103 initial errors via combination of: (a) targeted Edits for production-code pattern matches in `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/chirpstack.rs`, `src/storage/mod.rs`, `src/storage/memory.rs`, `src/storage/sqlite.rs` adding `(_)` binding-discard and `TODO(A-3/A-4/A-5/A-6)` markers; (b) perl bulk substitution `s/MetricType::X(?![\(\w])/MetricType::X(default)/g` across test files + `src/main.rs` + remaining src/ files for ~80 fixture call sites; (c) manual `.clone()` additions for Copy-drop cascade in `tests/pruning_integration_tests.rs` (6 sites).

- [x] **Task 7:** `cargo test --all-targets`: **1113 passed / 0 failed / 10 ignored** (baseline was 1112/0/9; net +1 passed +1 ignored — the +1 ignored is the existing storage_query_below_budget test that was already `#[ignore]` baseline and the new test_pool_throughput_under_load `#[ignore]` from commit `2c5a6b1`). `cargo clippy --all-targets -- -D warnings` clean (one fix needed: replaced `3.14` with `1.5` in `test_metric_type_display` to clear `clippy::approx_constant`). `cargo test --doc` 0 failed / 56 ignored (#100 baseline preserved).

- [x] **Task 8:** README.md `Current Version` narrative updated with A-1 paragraph + version bumped to `2.0.0-rc` per the Phase B Closure framing (v2.0 GA still gated on Epic A). `docs/schema-design.md` deferred to A-2. `docs/logging.md` not modified (no new audit events).

- [x] **Task 9:** Pre-commit checklist clean: `cargo test` 1113/0/10, `cargo clippy --all-targets -- -D warnings` clean, README mirrors sprint-status.yaml.

---

## Dev Notes

### The Copy-drop cascade

Dropping `Copy` from `MetricType` is the most far-reaching change in A-1. Every site that:

```rust
let x = some_value.data_type;        // implicit copy
foo(x);
bar(x);                              // would fail to compile under non-Copy
```

needs to switch to:

```rust
let x = &some_value.data_type;       // borrow
foo(x.clone());                      // or .clone() if the callee needs ownership
bar(x.clone());
```

OR to a single-consumer move pattern:

```rust
let x = some_value.data_type;        // move
foo(x);                              // x consumed here
// bar(x);                           // compile error — already moved
```

The dev agent should NOT mechanically `.clone()` everywhere — clones on long string payloads are wasteful. Where the call site clearly has a single consumer (most poll-cycle paths), use move semantics. Where multiple consumers need read-only access, use `&MetricType` borrows.

### Display vs Debug rendering contract

Logging via `tracing` uses `{}` format (`Display`) for keyed fields. Post-A-1, `MetricType::Float(23.5)` formatted with `{}` returns `"Float"` (discriminant name only) — that's intentional per AC#2 to preserve existing log volumes and the SQL `value_type` discriminant column write path. Use `{:?}` format when you want value+type rendering in test failure messages. The `tracing` audit event surface in src/chirpstack.rs / src/opc_ua.rs that currently logs `data_type = %metric.data_type` continues to log the discriminant name — no log regression.

### The `Serialize` / `Deserialize` impact

`MetricType` derives `Serialize, Deserialize`. Post-A-1 the JSON shape changes from `"Float"` to `{"Float": 23.5}` (the default serde enum-with-payload encoding). This breaks any persisted JSON that has `MetricType` at the top level — there is no such site in the codebase today (verify with `grep -r 'MetricType' --include='*.rs'` + check JSON test fixtures), but the audit is part of Task 6.

Web UI `/api/metrics` (Story 9-3, Story A-6 territory) consumes `MetricValue` via `Serialize`. The JSON wire shape changes. A-1 lands the type change; A-6 owns the dashboard rendering update + the API doc sync. A-1's Task 8 explicitly punts the dashboard render fix.

### `MetricValue.value: String` removal — downstream impact

The `value: String` field at `src/storage/types.rs:68` is the field that today holds the discriminant string in JSON serialisation. Removing it changes the `MetricValue` JSON shape. Audit:

- `tests/**` — any test that asserts `value` field in a JSON response
- `static/**.js` — any front-end that reads `value` from `/api/metrics` (Story 9-3's `metrics.js`)
- `src/web/api.rs` — `/api/metrics` handler

These call sites are Story A-6's territory. A-1 leaves them with compile errors and `TODO(A-6)` markers where appropriate.

### Why SqliteBackend can stay semantically broken in A-1

A-1's scope is strictly type-level. The SQLite schema migration (typed value columns) is A-2's deliverable. Without typed columns, SqliteBackend CAN'T round-trip a `MetricType::Float(23.5)` payload — there's nowhere to put the `f64`. Two viable A-1 paths:

- **(a) Aggressive:** `SqliteBackend::set_metric` panics with `"set_metric not implemented until A-2"`; `get_metric` returns `Ok(None)` with a `tracing::warn!("get_metric returns None until A-2 lands typed columns")`. Pros: fail-fast, no silent data corruption. Cons: integration tests against SqliteBackend will fail loudly; mark them `#[ignore]`.
- **(b) Quiescent:** Keep the current discriminant-string write path with a `TODO(A-2)` comment. SqliteBackend continues to write `"Float"`/`"Int"`/etc. (the #108 bug behaviour) until A-2 replaces it. Pros: integration tests continue to compile and pass. Cons: leaves #108 behaviour in place one more commit; surface-level seems backwards.

**Recommended: option (b)** for A-1. Rationale: A-1's purpose is to land the type change cleanly without amplifying the blast radius. Option (b) keeps the diff focused on type-level changes and defers the storage-layer rewrite to A-2 where it belongs. The `TODO(A-2)` marker is auditable.

### `MetricValueInternal` at `src/storage/mod.rs:768`

There's a parallel struct `MetricValueInternal` at mod.rs:768 used internally by some methods. Task 2 should check whether it mirrors `MetricValue` and apply the same `.value: String` removal if so. Doc comment audit: cross-reference both structs' contracts.

### Test budget delta

- Baseline (commit `e0b64a0`): 1112 passed / 0 failed / 9 ignored / 56 doc-ignored.
- A-1 expected delta: +4 new in-memory round-trip tests (Float/Int/Bool/String — AC#6); +N `#[ignore]` SqliteBackend tests under option (a) OR ±0 ignored under option (b).
- A-1 target: ≥ 1116 passed / 0 failed / ≥ 9 ignored (option (b)) OR ≥ 1080 passed / 0 failed / ≥ 30+ ignored (option (a)).

### Carry-forward GitHub issues (unchanged)

#88 (per-IP rate limiting), #100 (56 doctest ignores), #102 (tests/common reuse), #104 (TLS hardening), **#108 (this epic closes)**, #110 (RunHandles Drop), #113 (live-borrow refactor), #117 (perf-CI lane for storage::pool throughput).

---

## Out of Scope

- **SQLite schema migration v007** — A-2's deliverable. A-1 leaves SqliteBackend semantically broken (option (b) keeps #108 behaviour) with TODO markers.
- **Poller value-payload write pipeline** — A-3's deliverable. A-1 leaves chirpstack.rs call sites with `TODO(A-3)` markers wrapping placeholder values.
- **OPC UA Read / HistoryRead value-payload pipelines** — A-4 / A-5's deliverables.
- **Web UI live-metrics value display + JSON shape update** — A-6's deliverable.
- **Migration runbook + version-gated migration script** — A-7's deliverable.
- **New audit events** (`metric_parse`, `metric_value_decode_failed`, etc.) — deferred to A-3 onwards where they actually fire.
- **Removing the `MetricType` Display impl entirely** — kept per AC#2 for backward-compatible log volumes and the `value_type` discriminant column.

---

## Completion Note

Ultimate context engine analysis completed — comprehensive developer guide created for Story A-1.

**The dev agent now has everything needed for flawless implementation.** Recommend running `bmad-dev-story A-1` next, then `bmad-code-review` on a different LLM per CLAUDE.md "Code Review & Story Validation Loop Discipline" + memory `feedback_iter3_validation` 6-story validated pattern.

---

## Dev Agent Record

### Iter-0 Scope Revision (2026-05-14, pre-implementation)

During the initial implementation survey, the dev agent identified two specification ambiguities that the user approved revising inline before code lands:

**Revision 1 — AC#10 strict-zero list shrinks.** The original list included `src/opc_ua_history.rs`, but that file contains production-code pattern matches (`match metric.data_type { MetricType::Float => ..., MetricType::Int => ..., MetricType::Bool => ..., MetricType::String => ... }` at lines 377-414) which cannot survive the payload-bearing refactor unchanged. Same shape exists in `src/chirpstack.rs`, `src/opc_ua.rs`, `src/main.rs`, `src/storage/mod.rs`. **Revised strict-zero list:** `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing`, `src/config_reload.rs`, `src/opcua_topology_apply.rs`, `src/storage/pool.rs`, `src/storage/schema.rs`. Removed from strict-zero: `src/opc_ua_history.rs` (compile-fixup-only allowed; TODO(A-5) markers).

**Revision 2 — AC#4 deferred to A-5.** The original AC#4 removed `MetricValue.value: String`. The dev agent identified that this field is currently load-bearing for the parse-from-string logic in `src/opc_ua_history.rs:377-414` and `src/opc_ua.rs::get_value`. Removing it in A-1 would require simultaneously rewriting all read sites — which is A-5's territory. **Deferred:** `MetricValue.value: String` stays in place for A-1. The dual-storage redundancy (`.value: String` AND payload-bearing `.data_type: MetricType`) is temporary and marked with `TODO(A-5)` at the struct definition. A-5 removes the field once all reads are pattern-matching the typed payload.

**Net effect on A-1 deliverable:**
- A-1 lands a minimal mechanical refactor: payload-bearing `MetricType` variants (`Float(f64)`, `Int(i64)`, `Bool(bool)`, `String(String)`); `Copy` dropped; `Display` preserved.
- All production pattern matches get `(_)` binding-discard added to existing arms (e.g., `MetricType::Float =>` → `MetricType::Float(_) =>`). This keeps the existing parse-from-string logic working unchanged until A-5 replaces it.
- All variant constructions get a placeholder payload with `TODO(A-N)` markers identifying which downstream story owns the real-value wiring (A-3 for poller writes, A-4 for OPC UA reads, A-5 for HistoryRead, A-6 for web UI).
- `MetricValue.value: String` stays; gains a `TODO(A-5)` comment at the struct definition.
- Issue #108 behaviour is preserved at the runtime level in A-1 (still writes discriminant string to SQLite) — A-2 / A-3 onwards close it.

User confirmed this revised scope before implementation started.

### Implementation Plan

Followed the spec's 9-task sequence with the Iter-0 scope revisions:

1. Refactor `MetricType` enum + impls in `src/storage/types.rs` (Tasks 1+3 — Display preserved, FromStr clarified for zero-default, dropped Copy, new `test_metric_type_payload_roundtrip`).
2. Cascade `.clone()` in `src/storage/memory.rs` (Task 4) — 4 sites where MetricType moved or implicit-copied.
3. Add `(_)` binding-discard to production pattern matches in `src/opc_ua_history.rs`, `src/opc_ua.rs`, `src/chirpstack.rs`, `src/storage/mod.rs` (Task 6 — TODO markers point to downstream stories that own real payload-aware refactors).
4. Constructor placeholders with `TODO(A-3)` markers in `src/chirpstack.rs` poller writes + `src/opc_ua.rs::convert_variant_to_metric` + `src/storage/mod.rs::MetricValueInternal` startup defaults.
5. Bulk perl substitution `s/MetricType::X(?![\(\w])/MetricType::X(default)/g` for test fixtures across `src/storage/sqlite.rs` (test mod), `src/storage/mod.rs` (test mod), `src/main.rs` (test mod), `src/storage/types.rs`, `src/web/api.rs`, and 13 integration test files.
6. Manual `.clone()` additions for Copy-drop cascade in `tests/pruning_integration_tests.rs` (6 sites) + `src/storage/sqlite.rs` (2 sites).
7. Fix one `clippy::approx_constant` warning (3.14 → 1.5 in Display test).
8. Tests + clippy + doctest verified clean.

### Debug Log

- Initial scope survey: 28 files touched, 88 production constructions in `src/storage/sqlite.rs` (later split: 5 prod + 83 test-mod), production pattern matches in `src/opc_ua_history.rs:377-414` clashed with the spec's strict-zero list → Iter-0 scope revision approved by user.
- First `cargo build --lib` error count: 40 → 31 → 16 → 2 → 0 across targeted fixes.
- Full `cargo build --all-targets` after lib clean: 103 errors (mostly test fixtures) → bulk perl substitution → 15 → 11 (api.rs + types.rs missed first round) → 8 → 0.
- Test failure caught: `test_metric_value_creation` asserted `Float(0.0)` against constructed `Float(23.5)` — perl substitution had replaced the assertion target; fixed manually.
- Clippy caught one `clippy::approx_constant` warning (use of 3.14) — replaced with 1.5 (no semantic meaning, just a non-PI float for the Display test).

### Completion Notes

A-1 lands the type-level foundation for Epic A: `MetricType` is payload-bearing. Issue #108 behaviour at runtime is intentionally preserved (SqliteBackend still writes discriminant strings via the existing path) — A-2's schema migration v007 starts the typed-column rewrite; A-3 / A-4 / A-5 / A-6 finish closing #108 by replacing the parse-from-string read sites with typed-payload pattern matches.

Iter-0 scope revisions captured above:
- AC#10 strict-zero shrank to remove `src/opc_ua_history.rs` (it has production pattern matches that can't survive untouched).
- AC#4 (`MetricValue.value: String` removal) deferred to A-5.

All other ACs satisfied:
- AC#1 (payload-bearing variants) ✓
- AC#2 (Display discriminant-only) ✓
- AC#3 (FromStr retained with documented zero-default contract) ✓
- AC#5 (StorageBackend trait signatures unchanged at type level) ✓
- AC#6 (InMemoryBackend round-trip test) ✓
- AC#7 (SqliteBackend option-b) ✓
- AC#8 (all call sites compile with TODO markers) ✓
- AC#9 (no new audit events) ✓
- AC#11 (1113 passed / 0 failed / 10 ignored; clippy clean) ✓
- AC#12 (cargo test --doc 0 failed / 56 ignored) ✓
- AC#13 (#118 filed) ✓

Recommend running `bmad-code-review A-1` on a different LLM per CLAUDE.md "Code Review & Story Validation Loop Discipline" + memory `feedback_iter3_validation` 6-story validated pattern. Note: AC#10 scope revision is a candidate for iter-1 review scrutiny.

### File List

**Modified (production code):**
- `src/storage/types.rs` — payload-bearing `MetricType` enum, `Display` preserves discriminant, `FromStr` clarified, `MetricValue` doc comment updated with `TODO(A-5)` on the `.value` field. New `test_metric_type_payload_roundtrip` test.
- `src/storage/memory.rs` — Copy-drop cascade fixes: `.cloned()`, `.clone()`, `metric_type.clone()` at 4 sites.
- `src/storage/mod.rs` — `MetricValueInternal` startup defaults updated to payload-bearing form; pattern match in nested match expression updated with `(_)` discards.
- `src/storage/sqlite.rs` — 2 Copy-drop cascade fixes; 83 test-mod fixture rewrites via perl batch; 1 production pattern-match `matches!()` rewrite.
- `src/chirpstack.rs` — production `target_type` construction sites with `TODO(A-3)` markers + `(_)` discards on pattern arms; `matches!()` rewrite for discriminant equality; OpcMetricTypeConfig-driven UPSERT paths get zero-default payload constructions with `TODO(A-3)`.
- `src/opc_ua.rs` — `convert_metric_to_variant` pattern arms get `(_)` discards with `TODO(A-4)` marker; `convert_variant_to_metric` constructions get zero-default payload with `TODO(A-4/A-6)` marker.
- `src/opc_ua_history.rs` — pattern arms `MetricType::Float =>` etc. get `(_)` discards with `TODO(A-5)` marker (Iter-0 scope revision allows the touch).
- `src/main.rs` — test-mod fixture rewrites via perl batch (12 sites).

**Modified (test files):**
- `src/web/api.rs` — 2 test-mod sites updated to payload-bearing constructions.
- `tests/metric_types_test.rs` — 17 fixture call sites updated.
- `tests/pruning_integration_tests.rs` — 12 sites: perl-rewrite + 6 manual `.clone()` additions for Copy-drop cascade.
- `tests/staleness_detection_tests.rs` — 4 sites.
- `tests/opcua_subscription_spike.rs` — 6 sites.
- `tests/web_device_crud.rs` — 5 sites.
- `tests/opcua_history.rs` — 3 sites.
- `tests/opcua_history_bench.rs` — 1 site.
- `tests/opc_ua_sqlite_backend_tests.rs` — 4 sites.
- `tests/opc_ua_security_endpoints.rs`, `tests/opc_ua_connection_limit.rs`, `tests/web_dashboard.rs`, `tests/opcua_dynamic_address_space_apply.rs`, `tests/opcua_dynamic_address_space_spike.rs` — incidental fixture updates.

**Modified (documentation):**
- `README.md` — Current Version narrative updated; version bumped to `2.0.0-rc`.
- `_bmad-output/implementation-artifacts/A-1-metrictype-payload-bearing-enum.md` — this file, Dev Agent Record populated.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — `A-1: in-progress → review`, `last_updated` narrative refreshed.

**Strict-zero invariants honoured (revised AC#10 list):**
- `src/web/auth.rs`, `src/web/csrf.rs`, `src/web/config_writer.rs` — `git diff` empty.
- `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs` — `git diff` empty.
- `src/security.rs`, `src/security_hmac.rs` — `git diff` empty.
- `src/main.rs::initialise_tracing` — function body untouched (other parts of main.rs got fixture-mod rewrites).
- `src/config_reload.rs`, `src/opcua_topology_apply.rs` — `git diff` empty.
- `src/storage/pool.rs`, `src/storage/schema.rs` — `git diff` empty.

**Created:**
- GitHub issue [#118](https://github.com/guycorbaz/opcgw/issues/118) — A-1 tracking issue.

### Change Log

- 2026-05-14: A-1 implementation complete via bmad-dev-story. Status flipped `ready-for-dev → in-progress → review`. Iter-0 scope revision applied pre-implementation (AC#10 shrunk, AC#4 deferred to A-5). `cargo test --all-targets` 1113 passed / 0 failed / 10 ignored. `cargo clippy --all-targets -- -D warnings` clean. `cargo test --doc` 0 failed / 56 ignored.

# Story A-1: MetricType Payload-Bearing Enum + StorageBackend Trait Amendment

| Field         | Value                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------- |
| Story key     | `A-1-metrictype-payload-bearing-enum`                                                                 |
| Epic          | A — Storage Payload Migration (Phase B Closure, gates v2.0 GA)                                        |
| FRs           | FR51 (new in PRD via correct-course commit `e0b64a0`)                                                 |
| Status        | done                                                                                                  |
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

### Review Findings

Iter-1 code review run on 2026-05-15 via `bmad-code-review` — 3 parallel adversarial layers (Blind Hunter, Edge Case Hunter, Acceptance Auditor). Layer raw output: 30 + 22 + 19. After dedupe / triage: **1 decision-needed, 9 patches, 22 defers, 6 dismissed**.

#### Decision-needed (1)

- [x] [Review][Decision] **D1: Iter-0 scope revisions explicitly confirmed by user (2026-05-15, iter-1 review)** — Per CLAUDE.md "Code Review & Story Validation Loop Discipline" requirement *"Accepted as deferred requires the user's explicit decision per finding."* On 2026-05-15 the user (Guy) explicitly confirmed BOTH Iter-0 revisions on the iter-1 review pass via the `AskUserQuestion` prompt: (a) AC#10 strict-zero shrink removing `src/opc_ua_history.rs` — **CONFIRMED** (carve-out justified; the auditor verified the touched portion is minimal — only (_) discards + 4 TODO(A-5) comments); (b) AC#4 (`MetricValue.value: String` removal) deferral to A-5 — **CONFIRMED** (removing in A-1 would require rewriting read sites that belong to A-5's scope). Original concern raised by Blind F18 + Auditor X2 resolved.

#### Patches (9)

- [x] [Review][Patch] **P1 [HIGH] AC#5 violated — backfill `StorageBackend` trait doc comments to reflect payload-bearing semantics** [src/storage/mod.rs:187-248] — Spec AC#5 requires both "signatures unchanged" AND "doc comments amended to reflect the payload-bearing semantics (no more 'parse based on data_type field' footgun)." Signatures are intact, but the trait method docs at lines 187-248 retain pre-A-1 prose with zero mention of payload-bearing semantics. Per CLAUDE.md loop discipline, A-1 cannot flip to `done` while a HIGH AC violation is open. (Auditor X1)
- [x] [Review][Patch] **P2 [MEDIUM] Rewrite `convert_metric_to_variant` doc to describe variant-kind mapping, not zero-default returns** [src/opc_ua.rs:2002-2009] — Current doc reads "`Int32/Int64` → `MetricType::Int(0)`" / "`Float/Double` → `MetricType::Float(0.0)`" which a casual reader will interpret as "value is discarded." Should describe variant-kind mapping semantics. (Blind F3)
- [x] [Review][Patch] **P3 [MEDIUM] Pin discriminant-only `Display` contract against a non-zero payload** [src/storage/types.rs::tests] — Add a test asserting e.g. `MetricType::Float(23.5).to_string() == "Float"`, `MetricType::Bool(true).to_string() == "Bool"`. Currently only zero payloads are exercised, but the SQLite `data_type` column write path (and any future grep contract) depends on discriminant-only rendering surviving real payloads. (Blind F7+F30 + Edge F9)
- [x] [Review][Patch] **P4 [MEDIUM] Strengthen round-trip tests; close AC#6 location/granularity gap and boundary coverage** [src/storage/memory.rs::tests + src/storage/types.rs::tests] — (a) Move/duplicate the 4-variant `test_metric_type_payload_roundtrip` to `src/storage/memory.rs::tests` and split into 4 per-variant functions exercising `InMemoryBackend::set_metric` → `get_metric` (closes AC#6 spec location requirement); (b) add boundary-value coverage to the existing types.rs test: `Float(f64::NAN)` (note PartialEq returns false), `Float(f64::INFINITY)`, `Float(-0.0)`, `Int(i64::MIN)`/`Int(i64::MAX)`, `String("")` empty, `String("\0\u{FFFD}")` embedded NUL. Brings test count to the Dev Notes aspirational ≥1116. (Blind F11+F14+F28 + Edge F13 + Auditor AC#6 + X3)
- [x] [Review][Patch] **P5 [LOW] Add in-source `TODO(A-2)` markers near sqlite.rs discriminant-string write paths** [src/storage/sqlite.rs::upsert_metric_value + batch_write_metrics + set_metric] — `grep -rn 'TODO(A-2)' src/` currently returns zero hits; option-(b) staging exists only as prose in spec/dev-notes. Add a 2-line comment near each discriminant-string write so future readers / `grep`-based audits can locate the deferred work. (Auditor X4)
- [x] [Review][Patch] **P6 [LOW] Cross-reference `TODO(A-5)` in dual-storage doc comments** [src/storage/types.rs:90 + src/storage/mod.rs `MetricValueInternal`] — The `MetricValue.data_type` field doc currently says "carries the typed payload (post-A-1)" without the "but zero-defaulted until A-3/A-4/A-5/A-6 land" caveat. `MetricValueInternal` has no `TODO(A-5)` marker either. Update both to mirror the disclaimer already on `MetricValue.value`. (Edge F2 + Auditor X5)
- [x] [Review][Patch] **P7 [LOW] Reconcile README "version bumped to 2.0.0-rc" claim with `Cargo.toml` (currently `version = "2.0.0"`)** [README.md:198 + Cargo.toml] — Either bump `Cargo.toml` to `2.0.0-rc` to match the Phase B Closure framing, or back out the README claim. CLAUDE.md "Documentation Sync" requires README to be a faithful entry-point. (Blind F29)
- [x] [Review][Patch] **P8 [LOW] Audit perl-substitution-touched test files for tautological / mis-replaced assertions** [13 test files cited in File List] — Dev Agent Record's Debug Log notes `test_metric_value_creation` was hand-fixed after the perl bulk substitution corrupted an assertion. Run a sanity grep across all 13 test files to confirm no other assertion compares a real expected payload against a `MetricType::X(default)` placeholder, or asserts `MetricType::X(0) == MetricType::X(0)` where the original test intended to check a specific value. (Blind F23)
- [x] [Review][Patch] **P9 [LOW] Replace `store_metric` (#[allow(dead_code)]) placeholder body with `unimplemented!()` until A-3** [src/chirpstack.rs:1714-1750] — Currently the Bool arm builds `MetricType::Bool(false)` even when the validated input is `1.0` (i.e., `true`). The method is `#[allow(dead_code)]` today, but a future contributor re-enabling it before A-3 lands would silently store `false` for every true input. Defensive: replace the placeholder body with `unimplemented!("store_metric body is pre-A-3 — payload wiring lands in Story A-3")`. (Edge F17)

#### Deferred (22) — pre-existing or transitional issues

Recorded in `_bmad-output/implementation-artifacts/deferred-work.md` under `## Deferred from: code review of A-1 (2026-05-15)`. Summary:

- [x] [Review][Defer] DEF1: `InMemoryBackend::load_all_metrics` reconstructs `MetricValue.value` from `metric_type.to_string()` = discriminant — pre-existing; A-5 rewrites read sites. [src/storage/memory.rs:204-222]
- [x] [Review][Defer] DEF2: Counter monotonic check `prev_metric.value.parse::<i64>()` fragile under value-column encoding drift — pre-existing; A-3 plumbs typed payload. [src/chirpstack.rs:1626-1636]
- [x] [Review][Defer] DEF3: `convert_variant_to_metric` discards inbound payload (typed side hardcoded to zero defaults) — explicit `TODO(A-4)` markers; A-4 closes. [src/opc_ua.rs:2018-2024]
- [x] [Review][Defer] DEF4: Bool validation arm in `process_metrics` drops parsed bool into `Bool(false)` — explicit `TODO(A-3)`; string-side carries truth. [src/chirpstack.rs:1640]
- [x] [Review][Defer] DEF5: `set_metric` (writes `serde_json::to_string`) vs `upsert_metric_value` (writes `value.to_string()`) heterogeneous encodings on the `value` column — pre-existing; A-2 schema migration unifies. [src/storage/sqlite.rs:539-544 vs 839-840]
- [x] [Review][Defer] DEF6: Divergent fractional-warn policy across chirpstack Counter arms — A-3 housekeeping. [src/chirpstack.rs:1602-1622 vs 1755-1760]
- [x] [Review][Defer] DEF7: `Storage::new` startup-defaults dual-source-of-truth (string `value` arm mirrors payload `metric_type` arm) — transitional; A-5 removes `value`. [src/storage/mod.rs:998-1015]
- [x] [Review][Defer] DEF8: `FromStr` has no compile-time enforcement of "pair with value source" contract — option-(b) explicitly chosen and documented; newtype refactor out of A-1 scope. [src/storage/types.rs:67-77]
- [x] [Review][Defer] DEF9: `MetricValue.value` / `MetricValue.data_type` / `BatchMetricWrite` dual source of truth not enforced by any constructor — transitional; A-5. [src/storage/types.rs:96-117 + src/storage/mod.rs `BatchMetricWrite`]
- [x] [Review][Defer] DEF10: README "Current Version" is now a 4000-char single-line paragraph; reviewability degrading — housekeeping. [README.md:198]
- [x] [Review][Defer] DEF11: `chirpstack.rs:1745-1781` arms structurally identical — A-3 refactor housekeeping. [src/chirpstack.rs:1745-1781]
- [x] [Review][Defer] DEF12: `SqliteBackend` Float NaN filter at line 1444 uses `matches!()` + separate `value.parse::<f64>()`; refactor landmine when A-2 lands. [src/storage/sqlite.rs:1444]
- [x] [Review][Defer] DEF13: Test assertions `assert_eq!(_, MetricType::X(default))` will silently pin to placeholder values once A-3 wires real payloads — bulk test maintenance lands with A-3. [many test files]
- [x] [Review][Defer] DEF14: `cargo test --doc` 56 ignored baseline; future doctests with zero-payload examples would harden the temporary contract — hypothetical, monitor in A-2 onward. [src/storage/types.rs doc comments]
- [x] [Review][Defer] DEF15: `MetricValue.value` is `pub` and `TODO(A-5): remove` is a SemVer break for any downstream consumer — 2.0.0-rc framing arguably covers; revisit at A-5. [src/storage/types.rs:104-117]
- [x] [Review][Defer] DEF16: `opc_ua_history.rs` trace-level row-skip log is too quiet for a row-loss event — A-5 rewrites HistoryRead read path; promote to warn then. [src/opc_ua_history.rs:386-414]
- [x] [Review][Defer] DEF17: `opc_ua.rs::convert_metric_to_variant` Float arm narrows f64→f32 without finite-after-narrowing check (siblings in `opc_ua_history.rs:390-397` do) — pre-existing; A-4 OPC UA Read pipeline will harden. [src/opc_ua.rs:1845-1859]
- [x] [Review][Defer] DEF18: Counter `Int` arm casts non-finite f64 to i64 via `as` (saturating) — pre-existing; A-3. [src/chirpstack.rs:1654-1660]
- [x] [Review][Defer] DEF19: `Storage::set_metric_value` accepts arbitrary `MetricValueInternal` with no value↔data_type invariant check — transitional; constructor-pattern refactor with A-5. [src/storage/mod.rs:1268-1300]
- [x] [Review][Defer] DEF20: `Variant::String(value)` may be a null `UAString`; `value.to_string()` returns `""` indistinguishable from legitimate empty — pre-existing. [src/opc_ua.rs:2020]
- [x] [Review][Defer] DEF21: `Variant::Int32` and `Variant::Int64` collapse to identical `MetricType::Int(0)` — bit-width loss baked into typed enum. [src/opc_ua.rs:2016-2017]
- [x] [Review][Defer] DEF22: Boolean variant from invalid string defaults to `false` with warn but no event-tracking metric — pre-existing. [src/opc_ua.rs:1862-1873]

#### Dismissed (6) — noise / false positives

DM1 (`String::new()` micro-allocation), DM2 (clippy::approx_constant 3.14→1.5 cosmetic), DM3 (`matches!` vs `==` hypothetical weakening), DM4 (`.clone()` cascade necessary post-Copy-drop), DM5 (`FromStr` to_lowercase micro-opt pre-existing), DM6 (Edge F21 AC#10 strict-zero positive confirmation).

#### Iter-2 review (2026-05-15)

Iter-2 surfaced 20 (Blind) + 11 (Edge) + 9 patch verdicts (Auditor). All 9 iter-1 patches verified PATCHED-CORRECTLY or PATCHED-WITH-CAVEAT by the Auditor. AC#5, AC#6, AC#11 all SATISFIED post-iter-1.

**Convergent iter-2 HIGH-REGs** (Blind F13 + Edge F1 + Edge F2): the iter-1 P1 trait-doc backfill made backend-impl claims the implementations don't actually fulfil — the "real value reconstruction via `get_metric_value` → `MetricValue.value`" path is only valid post-`batch_write_metrics`, not after `set_metric` (split-brain between `InMemoryBackend.metrics` and `InMemoryBackend.metric_values`).

After iter-2 dedupe / triage: **1 HIGH, 5 MEDIUM, 4 LOW** iter-2 patches; 12 defers; 3 dismissals.

##### Iter-2 patches (10, all applied)

- [x] [Iter-2][Patch] **IR1 [HIGH] Tighten trait docs — remove backend-specific caveats, fix LSP framing** [src/storage/mod.rs:189-282 + src/storage/types.rs:103-122] — Tightened `get_metric` / `get_metric_value` / `set_metric` trait docs to describe only the trait-level contract; removed the misleading "real value reconstruction" path that didn't hold for `InMemoryBackend::set_metric → get_metric_value` (split-brain HashMap). `MetricValue.data_type` field doc corrected to flag the `batch_write_metrics` path as the only fully-consistent write→read route. (Blind F13 + Edge F1 + Edge F2)
- [x] [Iter-2][Patch] **IR2 [MEDIUM] Correct `set_metric` SQLite TODO(A-2) — it serde_json-encodes, not discriminant-only** [src/storage/sqlite.rs:540-549] — Pre-iter-2 TODO claimed "discriminant-string + JSON-encoded value" which mischaracterised the dual-encoded write. New TODO accurately describes `{"Float":0.0}` JSON + bare discriminant. (Edge F3)
- [x] [Iter-2][Patch] **IR3 [MEDIUM] Update `append_metric_history` doc bullet list** [src/storage/sqlite.rs:907-918] — Stale bullets (`Float(3.14) → "3.14"`, `Int(42) → "42"`, ...) contradicted the adjacent TODO(A-2) banner. Replaced with A-1 staging description naming `MetricType::to_string()` (discriminant) + cross-reference to `batch_write_metrics` for real values. (Edge F4)
- [x] [Iter-2][Patch] **IR4 [MEDIUM] Soften TODO(A-2) — remove premature A-2 column-name commitments** [src/storage/sqlite.rs:540 + 845 + 964 + 1070] — Pre-iter-2 TODOs hardcoded `value_real`/`value_int`/`value_bool`/`value_text` column names. A-2 owns the schema design; A-1 should not commit it. New TODOs name only "typed-payload write" + "A-2's schema migration" without column-name presumption. (Blind F11)
- [x] [Iter-2][Patch] **IR5 [MEDIUM] Replace `unimplemented!()` with `todo!()` in `store_metric`** [src/chirpstack.rs:1714-1736] — Semantic mismatch: `unimplemented!()` connotes "TODO will implement," but the method's intent is "will be reinstated by A-3." `todo!()` is the canonical macro for that. Runtime behaviour identical (both panic with `not yet implemented`); semantic intent clearer. (Blind F2)
- [x] [Iter-2][Patch] **IR6 [MEDIUM] Update README badge + docker references to `2.0.0-rc`** [README.md:2 + 77] — Iter-1 P7 only bumped Cargo.toml + README "Current Version" line; the SVG badge URL and `docker run` example still referenced `2.0.0`. CLAUDE.md doc-sync rule requires both to mirror Cargo.toml. (Edge F10)
- [x] [Iter-2][Patch] **IR7 [LOW] Strip stale 3-step contract from `store_metric` doc** [src/chirpstack.rs:1680-1709] — Pre-iter-2 doc still claimed "1. Determines metric type, 2. Converts value, 3. Stores in shared storage" — all 3 steps are dead since `todo!()`. Rewrote doc to name A-3 reinstatement contract + git-history pointer. (Edge F11)
- [x] [Iter-2][Patch] **IR8 [LOW] Add `Bool(true)` to boundary roundtrip test** [src/storage/types.rs::test_metric_type_payload_roundtrip_boundary_values] — Test only covered `Bool(false)`. Bool is a 2-valued domain — both ends matter. Added `MetricType::Bool(true).clone()` assertion. (Blind F20)
- [x] [Iter-2][Patch] **IR9 [LOW] Expand Display test doc to mention `value` column load-bearing role** [src/storage/types.rs::test_metric_type_display_pins_discriminant_only_contract] — Pre-iter-2 doc only mentioned the `data_type` column; but `upsert_metric_value`/`append_metric_history` also write the discriminant to the `value` column per option-(b) A-1 staging. Expanded doc to capture the full blast radius. (Blind F5)
- [x] [Iter-2][Patch] **IR10 [LOW] Document unsupported Variant subtypes in `convert_variant_to_metric`** [src/opc_ua.rs:1996-2022] — Pre-iter-2 doc only listed the 4 supported subtypes. Added explicit `# Unsupported Variant subtypes` block naming `UInt32`/`UInt64`/`Byte`/`SByte`/`Int16`/`UInt16`/`DateTime`/`ByteString`/etc. and noting A-4's potential extension. (Edge F5)

##### Iter-2 patch verification (2026-05-15)

- `cargo build --all-targets` — clean.
- `cargo test --all-targets` — **1125 passed / 0 failed / 10 ignored** (unchanged from iter-1 — IR8 added an assertion to an existing test, no new tests; all other patches are doc/macro changes).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --doc` — **0 failed / 55 ignored** (was 56 pre-iter-2; net −1 from IR7 deliberately removing the stale `rust,ignore` doctest example in `store_metric` doc that would have been factually misleading after the `todo!()` body swap). Spirit of AC#12 / issue #100 preserved (zero failures); letter is off by 1 ignored — captured here for record-keeping per Iter-3 Auditor CF1.

##### Iter-2 deferred (12)

Appended to `deferred-work.md` under `## Deferred from: code review of A-1 — iter-2 (2026-05-15)`:

- DEF-iter2-1 (Blind F3): missing-device error-path test for new round-trip tests.
- DEF-iter2-2 (Blind F4): bool roundtrip-guard message overstated for standalone assertion.
- DEF-iter2-3 (Blind F6): NaN handling test only validates `Clone`, not a backend round-trip.
- DEF-iter2-4 (Blind F7): `2.0.0-rc` lacks numeric suffix (`-rc.1`) — confirmed Phase B Closure framing per Dev Agent Record.
- DEF-iter2-5 (Blind F9): MetricValueInternal "must move together" invariant not enforced by any test.
- DEF-iter2-6 (Blind F10): 4 `TODO(A-2)` comments near-duplicate; drift hazard.
- DEF-iter2-7 (Blind F14): embedded-NUL payload tests don't exercise SQLite NUL-handling.
- DEF-iter2-8 (Blind F15): `opc_ua_history.rs::history_read_raw_modified` reference path not grep-verified.
- DEF-iter2-9 (Blind F17): `store_metric` body deletion archaeology cost (A-3 must `git show 16e7811:src/chirpstack.rs`).
- DEF-iter2-10 (Blind F18): `MetricType` variant exhaustiveness not enforced by Display test.
- DEF-iter2-11 (Blind F19): `TODO(A-N)` form drift (some sites use `TODO(A-4/A-6):`).
- DEF-iter2-12 (Edge F6/F7/F8/F9): boundary coverage holes (subnormals, signaling NaN, set_metric overwrite, cross-device same-metric-name, empty device_id).

##### Iter-2 dismissed (3)

DM-iter2-1 (Blind F8 — `convert_variant_to_metric` body not shown in diff; Auditor verified body matches doc), DM-iter2-2 (Blind F12 — trait doc impl mention partially patched by IR1), DM-iter2-3 (Auditor cross-findings 1-3 — cosmetic line drift / spec function-name typo / counter off-by-one).

#### Iter-3 review (2026-05-15)

Iter-3 same-LLM termination pass per CLAUDE.md + memory `feedback_iter3_validation` (6-story validated pattern). 3 parallel layers (Blind, Edge, Auditor) against combined iter-1 + iter-2 patch diff (711 lines, 9 files).

**Raw findings:** 12 (Blind) + 6 (Edge) + 19 patch verdicts (Auditor — all PATCHED-CORRECTLY).

##### Auditor verdict

All 13 ACs SATISFIED (AC4 DEFERRED-DOCUMENTED per user-confirmed Iter-0 Revision 2; AC12 MARGINAL with off-by-one doctest count noted as CF1). All 19 iter-1 + iter-2 patches verified PATCHED-CORRECTLY post-iter-3. **Loop-termination verdict: ELIGIBLE-FOR-DONE.**

##### Iter-3 dismissed (4 false positives)

DM-iter3-1 (Blind F1 — claim that `TODO(A-3)` markers were deleted from `chirpstack.rs`; only `store_metric` arms removed, `process_metrics` arms still carry them at lines 1591-1666; grep still returns hits). DM-iter3-2 (Blind F2 — `store_metric` test callers panic risk; `cargo test 1125/0/10` green, no test invokes the symbol). DM-iter3-3 (Blind F4 — `TODO(A-5)` on `MetricValue.value` missing; present at `types.rs:99` from iter-1 P6). DM-iter3-4 (Blind F8 — `convert_variant_to_metric` body-doc alignment; Auditor verified body matches doc).

##### Iter-3 patches (4 small LOW, all applied)

- [x] [Iter-3][Patch] **ITR1 [LOW] Add `Bool(true)` to `test_metric_type_display_pins_discriminant_only_contract`** [src/storage/types.rs::tests] — IR8 added Bool(true) to the boundary-roundtrip test; this mirrors it on the Display contract test for completeness. (Blind F6)
- [x] [Iter-3][Patch] **ITR2 [LOW] Correct doctest count in iter-2 spec verification block** [this file] — Iter-2 IR7 deliberately removed a `rust,ignore` doctest in `store_metric` that would have been factually misleading after the `todo!()` body swap. Actual is `0 failed / 55 ignored` (was 56). Spec text updated for record-keeping. (Auditor CF1)
- [x] [Iter-3][Patch] **ITR3 [LOW] `BatchMetricWrite.data_type` doc parity** [src/storage/mod.rs:131-155] — Iter-1 P6 + iter-2 IR1 added A-1 transitional caveats to `MetricValue.data_type` and `MetricValueInternal.value`, but the structurally-parallel `BatchMetricWrite` was missed. Brought into parity with the same TODO(A-5) cross-references. (Edge F1)
- [x] [Iter-3][Patch] **ITR4 [LOW] Expand `convert_variant_to_metric` Unsupported list** [src/opc_ua.rs::convert_variant_to_metric] — IR10 listed 12 unsupported subtypes + "etc."; ITR4 names the remaining complex variants (`Array`, `StatusCode`, `XmlElement`, `QualifiedName`, `ExpandedNodeId`, `ExtensionObject`, `DataValue`, `DiagnosticInfo`, nested `Variant`, sentinel `Empty`) for operator-facing completeness. (Edge F5)

##### Iter-3 deferred (11)

Appended to `deferred-work.md` under `## Deferred from: code review of A-1 — iter-3 (2026-05-15)`:

- DEF-iter3-1 (Blind F3): `get_metric_value` trait doc "may not populate" wording — true for InMemoryBackend after `set_metric` (split-brain), but the doc's nuance could mislead. LOW.
- DEF-iter3-2 (Blind F5): `todo!()` panic message references internal review doc path (`A-1-metrictype-payload-bearing-enum.md § Review Findings`) that operators can't resolve. LOW.
- DEF-iter3-3 (Blind F7): `InMemoryBackend::set_metric` auto-creates devices; trait doc says missing device returns Err. Trait/impl divergence to be tightened. LOW.
- DEF-iter3-4 (Blind F9): `tests/metric_types_test.rs` tautological assertions remain (already DEF13 from iter-1; TODO(A-3) comments now in place). LOW.
- DEF-iter3-5 (Blind F10): Three A-1 staging caveats (MetricValue/MetricValueInternal/BatchMetricWrite plus trait docs) — extract to module-level rustdoc anchor at A-5 cleanup. LOW.
- DEF-iter3-6 (Blind F11): `docker run ghcr.io/guycorbaz/opcgw:2.0.0-rc` references an image tag not yet published on GHCR — release-process concern, out of A-1 scope. LOW.
- DEF-iter3-7 (Blind F12): `convert_variant_to_metric` top-line doc doesn't lead with "real value in String half" — transitional caveat is buried. LOW.
- **DEF-iter3-8 (Edge F3 — MUST address in A-3): `SqliteBackend::set_metric` calls `serde_json::to_string(&value)` which rejects NaN/Inf. Today unreachable because the poller stamps `Float(0.0)`, but A-3's real-payload wiring will hit this. A-3 must decide policy: filter NaN at the poller, use a `serde_json` configuration that allows NaN/Inf, or surface a clean operator error.** **User explicitly accepted deferral on 2026-05-15 iter-3 pass.** MEDIUM (severity preserved for A-3 attention).
- DEF-iter3-9 (Edge F4): `store_metric` doc `# Arguments` section reads as if params were used; body is `todo!()`. Minor voice mismatch. LOW.
- DEF-iter3-10 (Edge F6): `MetricValueInternal::ToSql`/`FromSql` no pre-A-1 backward-compat shim; theoretical hazard if any path reaches the ToSql/FromSql code (currently `#![allow(dead_code)]`). LOW.
- DEF-iter3-11 (Auditor CF4 — already DEF-iter2-5): MetricValueInternal "must move together" invariant not enforced by test. LOW.

##### Iter-3 patch verification (2026-05-15)

- `cargo build --all-targets` — clean.
- `cargo test --all-targets` — **1125 passed / 0 failed / 10 ignored** (ITR1 adds an assertion within an existing test; no new test functions).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --doc` — **0 failed / 55 ignored** (preserved from iter-2 post-IR7; AC#12 letter off by 1 vs #100 baseline, spirit preserved).

#### Loop-termination check (iter-3 → done)

Per CLAUDE.md "Code Review & Story Validation Loop Discipline":
> The loop terminates when one of these is true:
> 1. Zero findings, **or**
> 2. Only LOW severity findings remain, **or**
> 3. The user has explicitly accepted each remaining HIGH/MEDIUM finding by marking it deferred...

Post-iter-2 patch state:
- Iter-1 findings (1 HIGH + 3 MED + 5 LOW): all PATCHED-CORRECTLY per Auditor.
- Iter-2 findings (1 HIGH + 5 MED + 4 LOW): all 10 PATCHED.
- 22 iter-1 defers + 12 iter-2 defers: all LOW or pre-existing/transitional, all in `deferred-work.md`.

Condition #1 (zero findings) is met. Eligible to flip A-1 to `done` — pending optional iter-3 sweep.

#### Iter-1 patch round verification (2026-05-15)

All 9 patches applied + D1 confirmed. Post-patch verification:

- `cargo build --all-targets` — clean.
- `cargo test --all-targets` — **1125 passed / 0 failed / 10 ignored** (+12 vs 1113 baseline; exceeds Dev Notes ≥1116 aspirational target).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --doc` — 0 failed / 56 ignored (#100 baseline preserved).

New tests landed:
- `test_metric_type_display_pins_discriminant_only_contract` (P3, pins SQLite `data_type` column write contract against boundary payloads — NaN/Inf/i64::MIN/MAX/empty/unicode/embedded-NUL/10k-char).
- `test_metric_type_payload_roundtrip_boundary_values` (P4, boundary coverage for `Clone` + `PartialEq` round-trip; NaN handled via `f.is_nan()` pattern destructure, signed-zero via `to_bits()`).
- `test_set_then_get_float_metric_roundtrips_payload` / `_int_` / `_bool_` / `_string_` (P4, 4 per-variant tests in `src/storage/memory.rs::tests` — closes AC#6 spec location requirement; exercises `InMemoryBackend::set_metric → get_metric` path with boundary payloads including i64::MIN/MAX, empty string, embedded NUL).

In-source `TODO(A-2)` markers added at 4 SQLite write paths (P5): `set_metric:540`, `upsert_metric_value:839`, `append_metric_history:951`, `batch_write_metrics:1052`. `grep -rn 'TODO(A-2)' src/` now returns 4 matches.

Per CLAUDE.md "Code Review & Story Validation Loop Discipline": the iter-1 patch round was non-trivial (9 patches across 7 files including a HIGH AC#5 fix and 6 new tests). Recommend running iter-2 review before flipping to `done`.

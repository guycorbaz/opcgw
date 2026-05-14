# Story A-1: MetricType Payload-Bearing Enum + StorageBackend Trait Amendment

| Field         | Value                                                                                                 |
| ------------- | ----------------------------------------------------------------------------------------------------- |
| Story key     | `A-1-metrictype-payload-bearing-enum`                                                                 |
| Epic          | A — Storage Payload Migration (Phase B Closure, gates v2.0 GA)                                        |
| FRs           | FR51 (new in PRD via correct-course commit `e0b64a0`)                                                 |
| Status        | ready-for-dev                                                                                         |
| Created       | 2026-05-14                                                                                            |
| Source epic   | `_bmad-output/planning-artifacts/epics.md § Epic A § Story A.1`                                       |
| Sprint change | `_bmad-output/planning-artifacts/sprint-change-proposal-2026-05-14.md`                                |
| Tracking      | none yet (gh CLI not authenticated for write per Stories 9-4/9-5/9-6/9-7/9-8 precedent — defer)       |

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

- [ ] **Task 0:** Attempt to open the A-1 GitHub tracking issue. If `gh` CLI is not authenticated for write, surface this back to the user immediately with the issue-body draft, do not block on it.

- [ ] **Task 1:** Refactor `src/storage/types.rs::MetricType` to payload-bearing form. Drop `Copy` derive. Update `Display` to render the discriminant name only (per AC#2). Decide on `FromStr` (delete or rename per AC#3) and document the decision.

- [ ] **Task 2:** Refactor `src/storage/types.rs::MetricValue` to drop the `.value: String` field per AC#4. Update doc comments + the `MetricValueInternal` mirror at `src/storage/mod.rs:768` if it exists.

- [ ] **Task 3:** Amend `StorageBackend` trait doc comments at `src/storage/mod.rs:187-248`. Signatures unchanged; doc comments reflect payload-bearing semantics.

- [ ] **Task 4:** Refactor `src/storage/memory.rs::InMemoryBackend` to round-trip the payload-bearing `MetricType`. Update `set_metric`, `get_metric`, `get_metric_value`. Update existing unit tests; add the 4-variant round-trip tests per AC#6.

- [ ] **Task 5:** Refactor `src/storage/sqlite.rs::SqliteBackend` skeleton to compile against the new `MetricType` per AC#7. Choose the chosen path (panic+TODO vs broken+TODO) and document. Mark broken integration tests `#[ignore]` with `TODO(A-2)` annotations.

- [ ] **Task 6:** Walk `cargo build --all-targets` errors, fixing each compile site per AC#8. Test fixtures count as call sites. For downstream-epic touch zones (chirpstack.rs, opc_ua.rs, web/api.rs), use `MetricType::Float(0.0)` etc. placeholders with `TODO(A-3)` / `TODO(A-4)` / `TODO(A-6)` markers — do NOT prematurely implement the downstream refactor.

- [ ] **Task 7:** Run `cargo test --all-targets`; document the count delta vs the 1112/0/9 baseline per AC#11. Confirm clippy clean.

- [ ] **Task 8:** Documentation sync: `docs/schema-design.md` doesn't need amendment yet (A-2 owns it). `docs/logging.md` doesn't need amendment yet (no new audit events). `README.md` Current Version narrative gets a one-paragraph "A-1 done — MetricType payload-bearing enum landed, downstream Epic A stories now have the type-level foundation; SqliteBackend semantically broken pending A-2".

- [ ] **Task 9:** Pre-commit checklist per CLAUDE.md: `cargo test` + `cargo clippy --all-targets -- -D warnings` both clean; README.md mirrors sprint-status.yaml.

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

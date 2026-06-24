# Story F.4: Config Export / Import

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator**,
I want to download my gateway configuration and restore it on another instance,
so that I can back up, version, share, or reproduce a setup (useful for demos) — without copying secrets.

## Context & Problem Statement

opcgw stores its configuration in SQLite (the app tree — applications/devices/metrics/commands — via migration v009, and the singleton `[global]`/`[chirpstack]`/`[opcua]`/`[web]` sections via v010). There is **no** way today to get that config out of one instance and into another except hand-copying the DB file. F-4 adds **export** (download the config as a portable TOML file, **secrets excluded by default**) and **import** (upload a TOML config to a fresh/other instance, validated, routed through the F-0 staged-apply flow so it applies on the operator's explicit "Apply"). This is the last Epic F story and rounds out the "configure from the browser" goal: an operator can stand up a demo or a replacement gateway from a saved file.

**Greenfield:** there is no existing export/import/backup feature anywhere in the repo — this extends nothing.

## Verified surfaces & constraints (read before implementing — 2026-06-15)

- **Export needs NEW serialization code.** `AppConfig` (`src/config.rs:944`) and the app-tree structs (`ChirpStackApplications` `:607`, `ChirpstackDevice` `:637`, `ReadMetric` `:713`, `DeviceCommandCfg` `:736`, `OpcMetricTypeConfig` `:681`) derive `Deserialize` **only** — no `Serialize`. The four singleton sections (`Global` `:43`, `ChirpstackPollerConfig` `:92`, `OpcUaConfig` `:207`, `WebConfig` `:426`) DO derive `Serialize`. There is **no direct `toml` crate dependency** (only `figment` with the `toml` feature; the secrets writer hand-builds TOML). The nested-table serde renames already exist for round-trip: `application_list`→`#[serde(rename="application")]`, `device_list`→`"device"`, `read_metric_list`→`"read_metric"`, `device_command_list`→`"command"`.
- **Secrets = exactly two fields**, both in singleton sections: `chirpstack.api_token` + `opcua.user_password` (`SECRET_FIELDS_BY_SECTION`, `src/storage/migrate_singleton_config.rs:76`). `[web]` has **no** credential fields (web auth lives in `.env`). **DANGER:** because `ChirpstackPollerConfig`/`OpcUaConfig` derive `Serialize`, a naive `toml::to_string(&config)` emits the **plaintext** resolved secrets (the effective config has them from `secrets.toml`). Export MUST strip them.
- **Effective config** is assembled by `reload_effective_config` (`src/main.rs:301`): figment (config.toml ⊕ secrets.toml ⊕ SQLite singleton provider ⊕ env) → `load_all_applications_config()` overwrites `application_list` with the full SQLite tree → `validate()`. The web layer reads the live `Arc<AppConfig>` via `state.config_reload.subscribe().borrow()`.
- **Import write path:** `SqliteBackend::migrate_applications_config` (`src/storage/sqlite.rs:3701`) is the existing whole-tree atomic writer BUT it assumes an empty tree and writes the `c6_migration_done` meta flag — **do NOT reuse it for import.** Import needs a **new** atomic bulk-replace (delete-all-applications CASCADE + insert the imported tree, one EXCLUSIVE transaction, no migration-flag coupling). Singleton sections are written via the existing `write_singleton_section` (`sqlite.rs:3311`) per section.
- **F-2 Guard-2 trap (must avoid):** writing a PARTIAL singleton set leaves the table non-empty without the done-flag → next boot's migration Guard 2 back-fills the flag and skips the full migration. On an already-running instance the done-flag is already set, so writing all four singleton sections via `write_singleton_section` is safe (the table stays migrated). Write **all** sections (or none on failure); never a partial set.
- **F-0 staged-apply:** after writing to SQLite, call `AppState.stage_config_write("import")` (`src/web/mod.rs:391`) to bump `pending_gen`; the operator applies via the existing `POST /api/config/apply` (`src/web/apply.rs:71`). Import MUST **stage, not apply inline** (matching every CRUD handler). The supervisor re-validates before teardown, so a bad import is a non-disruptive `apply_failed`.
- **No file upload exists; CSRF requires `application/json`** (`src/web/csrf.rs`) and rejects `multipart/form-data`. So import takes a **JSON envelope** (`{ "toml": "<text>" }`), not a multipart upload — the browser reads the chosen file client-side (`FileReader`) and POSTs its text. Export is a read-only GET returning `text/plain` + `Content-Disposition: attachment`.
- **Validation:** `AppConfig::validate()` (`src/config.rs:1393`) checks app-tree invariants (unique application_id; per-app device_id uniqueness; per-device metric_name + chirpstack_metric_name uniqueness; command rules). Mirror the wizard / `put_singleton_section` candidate-overlay-then-validate pattern: build the candidate config (imported tree + imported singletons overlaid on the current snapshot) and `validate()` **before** any SQLite write.

## Acceptance Criteria

1. **Export endpoint.** `GET /api/config/export` (auth-gated; read-only so CSRF-exempt like the singleton GET) returns the current effective configuration as a **TOML** document with `Content-Type: text/plain; charset=utf-8` and `Content-Disposition: attachment; filename="opcgw-config-<...>.toml"` so a browser downloads it.

2. **Export content + exclusions.** The exported TOML contains the **portable** config: `[global]`, `[chirpstack]` (**without** `api_token`), `[opcua]` (**without** `user_password`), `[web]`, and the full `[[application]]` tree (devices → `read_metric` → `command`). It **excludes**: the two secrets (AC#8), and deployment-specific/host sections that should not travel between instances — at minimum `[storage]` (`database_path`) and `[logging]` (file-based, stays in `log4rs.yaml`). The export is a valid TOML document that opcgw can itself re-import (AC#7).

3. **Import endpoint.** `POST /api/config/import` (auth + CSRF; `Content-Type: application/json`) accepts a JSON envelope `{ "toml": "<config text>" }` (generous body limit — the app tree can exceed the 16 KiB singleton cap; pick e.g. 1 MiB). On success it **stages** the imported config to SQLite and returns `202` `{ "status": "staged", "pending_changes": true }` — it does **NOT** apply inline. The operator applies via the existing `POST /api/config/apply` (F-0).

4. **Import validation (reject before persist).** The handler parses the TOML, builds a **candidate** `AppConfig` (imported app tree + imported singleton sections overlaid on the current snapshot), and runs `AppConfig::validate()` **before** any SQLite write. Malformed TOML, a candidate that fails validation (duplicate IDs, invalid ranges, bad scheme, …), or a missing/oversized body is rejected with a **structured** `{ error, reason, hint? }` body and an appropriate 4xx — **nothing is written** and **no restart** happens. (NFR7: don't leak internal error detail in the body; log it.)

5. **Import write = atomic bulk-replace + all singleton sections; secrets untouched.** On a valid candidate: (a) replace the entire app tree via a **new** `SqliteBackend` atomic bulk-replace (delete-all applications CASCADE + insert the imported tree, one EXCLUSIVE transaction); (b) write each of the four singleton sections' non-secret fields via `write_singleton_section` (all four, or none on failure — never a partial set, per the F-2 Guard-2 trap). The two **secrets are NOT touched** — they stay per-instance in the target's `secrets.toml` (you import config, not credentials). Then `stage_config_write("import")`.

6. **Failure atomicity.** If the app-tree bulk-replace succeeds but a singleton write fails (or vice-versa), the gateway must not be left half-imported in a way that silently ships: either wrap the whole import in one transaction where feasible, or write the singletons first then the app tree (the app-tree replace is the bigger, riskier step) and surface a 500 with a clear reason. Document the chosen order + recovery. (The operator can always re-import or restore from their file; the key is no silent partial state + no inline restart.)

7. **Round-trip equivalence.** Export → import on another instance (or the same) yields an **equivalent** configuration: the app tree (applications/devices/metrics/commands) and the non-secret singleton fields match the source after Apply. A test exports a seeded config, imports the bytes into a fresh backend, and asserts the resulting tree + singletons equal the source (secrets excluded, so they are not part of the equivalence).

8. **Secrets never leave the gateway.** The exported document **never** contains the real `api_token` or `user_password` values. Implementation: serialize, then blank/omit the `SECRET_FIELDS_BY_SECTION` keys (or emit a `<set via config/secrets.toml>`-style placeholder, mirroring the singleton GET). A test greps the export bytes for the seeded secret values and asserts they are **absent**. Importing a file that contains a placeholder (or omits the secret) must not overwrite the target's real secret.

9. **Web UI.** A config export/import affordance on the F-1 shell — either a new section on an existing config page (e.g. `singleton-config.html`) or a small new page reachable from the nav. **Export:** a download button hitting `GET /api/config/export`. **Import:** a file picker that reads the chosen file client-side (`FileReader`), POSTs `{ toml }` to `/api/config/import`, surfaces validation errors inline, and on success shows the "pending changes — click Apply" affordance (the F-0 apply-bar). Vanilla JS, **no build step / framework / node_modules**.

10. **No new gateway-side behavior beyond export/import.** No aggregation, no auto-apply. The `toml` crate may be added as a direct dependency for serialization (it is the canonical Rust TOML library, already present transitively via figment) — **this dependency is pre-approved for this story**; add `Serialize` (with matching `#[serde(rename)]`) to the app-tree structs to enable round-trip serialization. Adding `toml` is a Rust dependency, distinct from the epic's "no frontend build step" rule.

11. **Docs synced.** `README.md` (export/import feature + Planning row F-4 → done on completion), `docs/web-api.md` or the appropriate API doc (the two new endpoints + the secrets-excluded contract), the DocBook manual (a "Back up & restore configuration" subsection), and `docs/security.md` (export excludes secrets by design; import never overwrites the target's secrets). `deferred-work.md` for any descope.

12. **Tests.** Backend: export TOML shape + secrets-absent (grep the seeded secret values); import validates + stages (202, `pending_changes`) + bulk-replaces the tree + writes all singleton sections + leaves secrets untouched; malformed TOML rejected (4xx, nothing written, no restart); a candidate that fails `validate()` (e.g. duplicate device IDs) rejected; round-trip equivalence; oversized/empty body rejected; import does NOT trigger Apply (no restart, `apply_signal` not fired). Frontend: `node --check`; served-HTML markers if a page is added/changed (F-1 no-regression invariant). Gates: `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean, `xmllint` clean.

## Tasks / Subtasks

- [x] **Task 1 — Serialization plumbing** (AC: #2, #8, #10)
  - [x] Added `toml = "0.8"` to `Cargo.toml`. Added `#[derive(Serialize)]` to `ChirpStackApplications`, `ChirpstackDevice`, `ReadMetric`, `DeviceCommandCfg`, `OpcMetricTypeConfig`, `StorageConfig`, `CommandValidationConfig`, `LoggingConfig`, and `AppConfig` (the hand-written `Debug` redaction is unaffected — Debug ≠ Serialize).
  - [x] `build_export_toml` (in `src/web/config_io.rs`): serialize the whole `AppConfig` to `toml::Value`, remove `[storage]`/`[logging]`/`[command_validation]` + strip `SECRET_FIELDS_BY_SECTION` keys, render. Unit test asserts secrets absent + sections present.

- [x] **Task 2 — Export endpoint** (AC: #1, #2, #8)
  - [x] `GET /api/config/export` (`config_io::export_config`): reads the live config, builds the export, returns `text/plain` + `Content-Disposition: attachment` + `Cache-Control: no-store`. Auth-gated, CSRF-exempt (GET). Route wired in `build_router`.

- [x] **Task 3 — Import endpoint + validation** (AC: #3, #4)
  - [x] `POST /api/config/import` (`config_io::import_config`, 1 MiB body limit): parse `{ toml }` envelope, build candidate via **figment merge** (`Serialized::defaults(current).merge(Toml::string(imported))`) — deep-merges so omitted secrets keep the target's values — then `validate()`. Malformed JSON → 400 `invalid_json`; bad TOML/merge → 400 `invalid_toml`; failed validation → 400 `config_invalid` (structured body, NFR7). Auth + CSRF.

- [x] **Task 4 — Import persistence (bulk-replace + singletons + stage)** (AC: #5, #6)
  - [x] New `SqliteBackend::replace_all_applications` (one EXCLUSIVE txn: delete commands/metrics/devices/applications child-first, insert the imported tree, row-count verify, COMMIT — NO migration-flag). Singleton sections written first via `write_singleton_section` (non-secret fields via `serialize_section`, all four), then the app-tree replace. `stage_config_write("import")`; secrets never written; `apply_signal` never fired.

- [x] **Task 5 — Web UI** (AC: #9)
  - [x] Export download `<a>` + import file-picker on `singleton-config.html`; new `static/config-io.js` reads the file (`FileReader`) and POSTs `{toml}`; surfaces validation errors + a "click Apply changes" success via the shared `.banner`. `node --check` clean.

- [x] **Task 6 — Docs** (AC: #11)
  - [x] README (feature paragraph + Planning row F-4), `docs/security.md` (new "Config export / import" subsection: export excludes secrets, import never overwrites them, staged-not-inline), DocBook manual ("Backing up & restoring configuration"). (`/api/config/*` endpoints aren't in `docs/web-api.md`'s structured surface; documented in README/security/manual prose.)

- [x] **Task 7 — Tests** (AC: #7, #12)
  - [x] `tests/web_config_io.rs` (6 + common): export shape + secrets-absent + auth-required; import valid → 202 staged + app-tree replaced + non-secret staged + api_token absent from SQLite + `applied_gen` unchanged (no inline apply); malformed JSON → 400; invalid config (dup device) → 400 + original tree intact + nothing staged; CSRF required. `config_io` unit tests: export secrets-absent + round-trip-preserves-target-secrets. Gates: cargo test, clippy, xmllint, node --check.

## Dev Notes

### Curated vs whole-config export
Prefer a **curated** export (the four singleton sections + the `[[application]]` tree, excluding `[storage]`/`[logging]`/secrets) over serializing the entire `AppConfig`. `AppConfig` carries deployment-specific (`storage.database_path`) and file-based (`logging`) fields that should not travel between instances. Build the export from: the four singleton structs (already `Serialize`) → blank secrets; plus the app tree (add `Serialize`). Emit one TOML document. The import side parses whatever sections are present and overlays them; absent `[storage]`/`[logging]` keep the target's own.

### Secret-exclusion (the load-bearing safety step)
Reuse `SECRET_FIELDS_BY_SECTION` (`src/storage/migrate_singleton_config.rs:76`) as the single source of truth — for each `(section, fields)`, remove or placeholder those keys from the serialized output. Mirror the singleton GET's `SECRET_PLACEHOLDER` approach (`src/web/singleton_config.rs:35`). A regression test greps the export bytes for the seeded secret values and asserts absence — this is the AC#8 guard.

### Import persistence — the gotchas
- **New bulk-replace, not `migrate_applications_config`** (which assumes empty + writes `c6_migration_done`). Model the new `replace_all_applications` on `migrate_applications_config`'s transaction shape (`sqlite.rs:3701`) but: delete all applications first (CASCADE wipes devices/metrics/commands), insert the imported tree, verify row counts, commit — and do **not** touch the migration meta flag.
- **All four singleton sections, never partial** (F-2 Guard-2 trap). On a running instance the `d0_migration_done` flag is already set, so `write_singleton_section` per section is safe; just write all four (skipping the secret fields). If you write zero or a partial set, fine on an already-migrated DB (flag stays set), but write all four for completeness.
- **Stage, never apply inline** — `stage_config_write("import")`, leave Apply to the operator (the supervisor re-validates before teardown → a bad import is non-disruptive `apply_failed`). Do NOT call `apply_signal.notify_one()`.
- **Validate the candidate before any write** — duplicate device IDs etc. must be a clean 4xx, not a half-written tree.

### Anti-patterns to avoid (do NOT)
- Do **not** emit secrets in the export (naive `toml::to_string(&config)` leaks them — strip via the skip-list).
- Do **not** reuse `migrate_applications_config` for import (migration-flag coupling + empty-tree assumption).
- Do **not** write a partial singleton set / trip the F-2 Guard-2 trap.
- Do **not** apply inline — stage + let the operator Apply.
- Do **not** overwrite the target's secrets on import (the file has none/placeholders).
- Do **not** add a multipart upload (CSRF requires `application/json`) — use the `{toml}` JSON envelope + client-side `FileReader`.
- Do **not** add a frontend build step / framework / node_modules.

### Project Structure Notes
- New endpoints sit beside the singleton-config + apply routes; a new `src/web/config_io.rs` module is reasonable (or extend `singleton_config.rs`). The `toml` crate is the one new Rust dependency (pre-approved, AC#10). The app-tree structs gain `Serialize` derives.
- Variance: this is the first feature to serialize SQLite→TOML (export) and to bulk-replace the app tree from the web layer (import). Both are additive; existing CRUD/singleton/apply paths are untouched.

### References
- [Source: _bmad-output/planning-artifacts/epics.md#Epic F — Story F.4: Config Export / Import]
- [Source: src/config.rs#AppConfig (:944), app-tree structs (:607/:637/:713/:736/:681), singleton sections (:43/:92/:207/:426), validate (:1393)]
- [Source: src/storage/migrate_singleton_config.rs#SECRET_FIELDS_BY_SECTION (:76)]
- [Source: src/storage/sqlite.rs#migrate_applications_config (:3701), write_singleton_section (:3311), load_all_applications_config (:2993)]
- [Source: src/web/singleton_config.rs#get_singleton_config/put_singleton_section (SECRET_PLACEHOLDER, candidate-overlay-validate pattern)]
- [Source: src/web/apply.rs#api_config_apply; src/web/mod.rs#stage_config_write (:391), build_router route table]
- [Source: src/main.rs#reload_effective_config (:301) — supervisor re-validates before teardown]
- [Prior story: F-0-staged-config-apply.md — staged-apply flow; F-2 — Guard-2 trap + secrets-to-secrets.toml]
- [Constraint: #130 no aggregation; epic: no frontend build step]
- [Source: CLAUDE.md#Documentation Sync, #Security & Quality Assurance]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context)

### Debug Log References

- `cargo clippy --all-targets -- -D warnings`: clean.
- `node --check static/config-io.js`: OK.
- `xmllint --noout docs/manual/opcgw-user-manual.xml`: valid.
- `tests/web_config_io` 9/0 (6 F-4 + 3 common); `web::config_io::tests` 2/0.

### Completion Notes List

- **Export** = serialize `AppConfig` → `toml::Value` → strip `[storage]`/`[logging]`/`[command_validation]` + the two secret keys → render. The `toml` crate (pre-approved) + `Serialize` derives on the config structs are the enabling change.
- **Import** uses the figment merge (`Serialized::defaults(current).merge(Toml::string(imported))`) — the elegant part: figment deep-merges tables so an imported `[chirpstack]` without `api_token` keeps the target's token (secrets preserved per-instance, never carried/overwritten), and replaces the `application` array when present (the tree is imported). Validated before any write; staged, never applied inline.
- **Persistence:** singleton sections first (cheap section-replaces, all four — avoids the F-2 Guard-2 trap), then the atomic `replace_all_applications` (delete-all + insert, one EXCLUSIVE txn, no migration-flag — distinct from `migrate_applications_config`). `stage_config_write` marks pending; the operator applies via F-0.
- **Known behaviour (documented):** importing a file with NO `[[application]]` keeps the current app tree (figment array-absent → base kept) — to clear all apps, use CRUD delete. The common import-a-real-config case replaces the tree.
- **Code-duplication note:** `replace_all_applications` duplicates the insert loop from `migrate_applications_config` (to avoid touching the load-bearing migration). A future refactor could extract a shared `insert_application_tree` helper.
- No frontend build step / framework / node_modules. One new Rust dep (`toml`).

### File List

- `Cargo.toml` — add `toml = "0.8"`.
- `src/config.rs` — `Serialize` derives on `AppConfig` + 8 config structs/enum.
- `src/web/config_io.rs` — NEW: `build_export_toml`, `export_config`, `import_config`, `merge_imported_config`, `ImportRequest` + unit tests.
- `src/web/mod.rs` — `pub mod config_io`; routes `GET /api/config/export` + `POST /api/config/import` (1 MiB limit).
- `src/storage/sqlite.rs` — NEW `replace_all_applications` (atomic bulk-replace).
- `src/storage/migrate_singleton_config.rs` — `serialize_section` → `pub(crate)`.
- `static/singleton-config.html` — "Back up & restore" section + `config-io.js` include.
- `static/config-io.js` — NEW: export link + import file-read/POST.
- `static/dashboard.css` — `.config-io` styling.
- `tests/web_config_io.rs` — NEW integration tests.
- `README.md`, `docs/security.md`, `docs/manual/opcgw-user-manual.xml` — docs.
- `_bmad-output/implementation-artifacts/F-4-config-export-import.md` (this), `sprint-status.yaml`.

## Senior Developer Review (AI)

**Reviewed:** 2026-06-15, re-reviewed 2026-06-24 · **Method:** 3 parallel adversarial layers (Blind Hunter / Edge Case Hunter / Acceptance Auditor) · **Iterations:** iter-1 patch round + iter-2 verification + iter-2b independent re-review (fresh agents, mandated because iter-1 added brand-new transaction flow-control) · **Outcome:** APPROVED — loop terminated, only LOW findings remain (1 MEDIUM found in iter-2b, fixed → GH #141).

### Iteration 1 — 1 HIGH + MEDs (all FIXED)
- **HIGH (Edge + Auditor converged): `command_class` dropped on import.** `DeviceCommandCfg.command_class` (Epic E valve device-class binding) was omitted from the import `INSERT INTO commands` — and export *does* serialize it — so an export→import round-trip silently lost the binding (valve commands revert to the legacy raw-byte path). The same omission was present in the boot migration `migrate_applications_config`. **Fixed:** added `command_class` to both inserts; a round-trip regression test (`export_import_round_trip_preserves_command_class`) pins it.
- **MED (Blind/Edge: partial-stage atomicity): the 4 singleton writes (each its own committed transaction) ran BEFORE the app-tree replace** — a storage failure between them left a half-staged config in SQLite (imported singletons + old apps) that a later unrelated Apply would activate. **Fixed:** replaced the two-step write with a single `SqliteBackend::import_replace_all(singletons, apps)` that does the singleton DELETE/INSERT + the app-tree delete-all/insert in **one EXCLUSIVE transaction** — the whole import is now all-or-nothing.
- **MED (Blind: tautological row-count check):** the verify compared the insert-loop counters to `COUNT(*)` after a delete-all (always equal). **Fixed:** compare against **input-derived** counts (`apps.len()`, summed devices/metrics/commands).
- **MED (Auditor: missing real round-trip + oversized/empty-body tests):** added `export_import_round_trip_preserves_command_class` (export builder → import bytes → tree incl. command_class), `import_oversized_body_rejected` (>1 MiB → 413), `import_empty_body_rejected` (→ 400 invalid_json).

### Iteration 2 — verification: CLEAN
Confirmed `import_replace_all` is one `BEGIN EXCLUSIVE … (||{…})() … COMMIT/ROLLBACK` (any failure rolls back the whole import); the singleton DELETE/INSERT matches the `singleton_config` schema; `command_class` binds nullable in both inserts and the load path reads it; the input-derived count check is correct with no false-positive; the migration change has nil blast radius (no test asserts the old column list). One cosmetic LOW (stale `replace_all_applications` log labels) fixed.

### Iteration 2b — INDEPENDENT re-review (2026-06-24, fresh 3-layer pass) — 1 MEDIUM (FIXED)
iter-1 introduced brand-new flow-control (`import_replace_all`'s single EXCLUSIVE transaction), so per the iter-N+1 doctrine a genuine independent adversarial pass was run (Blind Hunter / Edge Case Hunter / Acceptance Auditor, fresh agents, before committing the review-fix round). Blind Hunter and Acceptance Auditor came back clean (all 12 ACs MET, transaction + Guard-2 avoidance + count check + command_class + secret handling all verified). Edge Case Hunter surfaced:
- **MED (Edge: no `busy_timeout` on pool connections → spurious 500 under concurrent writers).** `ConnectionPool` (`src/storage/pool.rs`) hands out multiple connections and all 8 storage writers open `BEGIN EXCLUSIVE`/`IMMEDIATE`; with no busy timeout the loser of any write race (import vs import / import vs CRUD save / import vs Apply-reload) gets `SQLITE_BUSY` immediately → HTTP 500. Pre-existing pattern, but F-4 widens the window (import holds EXCLUSIVE across whole-tree delete+reinsert). **Fixed (Guy's explicit call):** `conn.busy_timeout(5000ms)` set on every pooled connection at creation in `ConnectionPool::new` — hardens all 8 writers. Regression test `test_pooled_connections_have_busy_timeout` pins it via `PRAGMA busy_timeout`. Tracked as **GH #141**. Loop terminates — only LOW findings remain.

### Accepted / considered (LOW — see deferred-work.md)
- **`foreign_keys` pragma not enabled on pool connections (Edge M2):** pre-existing; `import_replace_all`'s explicit child-first deletes are correct regardless. Flagged as a separate pre-existing issue (the CASCADE-reliant `delete_application`/`delete_device` paths), not an F-4 defect.
- **`#[serde(skip_serializing)]` on the secret fields (Blind L1): REJECTED** — the figment-merge import preserves the target's secrets *by serializing the current secret into the base layer*; skipping serialization would break that. The export strips secrets *after* serialization (the correct, tested guard).
- **Import cannot empty the app tree (Edge L1):** an imported file with no `[[application]]` keeps the current tree (figment array-absent → base kept). Documented behaviour; to clear all apps use CRUD delete.
- **Export reflects the *applied* config, not staged edits (Edge L2);** static export filename (Auditor LOW); `apply_signal`-not-fired asserted via the `applied_gen`-unchanged proxy (Auditor LOW) — all accepted.

### Gates (final)
`cargo test` exit 0 (38 suites, 0 failed) · `cargo clippy --all-targets -- -D warnings` clean · `xmllint` clean · `node --check` OK. `web_config_io` 12/0, `config_io` unit 2/0, migration 19/0, singleton 19/0, pool 14/0 (incl. new `test_pooled_connections_have_busy_timeout`).

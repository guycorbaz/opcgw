# Story D-2: Decommission TOML Mutation Surface (must-land-last)

Status: ready-for-dev

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `D-2-decommission-toml-mutation`                                                                            |
| Epic            | D — Singleton Configuration → SQLite                                                                        |
| FRs             | none (Epic D is post-PRD)                                                                                   |
| Status          | ready-for-dev                                                                                               |
| Created         | 2026-05-27                                                                                                  |
| Source epic     | `_bmad-output/planning-artifacts/epics.md § Epic D § Story D.2`                                             |
| Depends on      | D-0 (SQLite singleton store + boot-time migration) and D-1 (web editor + boot-time AppConfig overlay + `seed_post_overlay`). Strictly: D-2 is the LAST story in Epic D and MUST land after D-0 + D-1 since it removes the TOML safety net that both of them fall back to. |
| Tracking        | GitHub issue `#__` — user opens out-of-band                                                                 |

---

## User Story

As an **opcgw operator running a post-D-2 binary**,
I want the gateway to read `config.toml` exactly once at the first post-D-0 boot (as a bootstrap seed) and never again at runtime,
So that all configuration writes converge on SQLite + `secrets.toml` as the canonical surfaces, my D-1 UI edits take effect on next boot without competing with stale TOML values, and a hand-edited `config.toml` produces a single audit warn instead of silently shadowing the SQLite snapshot.

---

## Story Context

### Why D-2 is the closing story of Epic D

Story C-6 (2026-05-26) moved the `[[application]]` collection tree from TOML to SQLite. Story D-0 (2026-05-27) moved the four singleton sections (`[global]` / `[chirpstack]` / `[opcua]` / `[web]` non-secret fields) to SQLite with a one-shot boot-time migration. Story D-1 (2026-05-27) added the web UI editor + the boot-time `Arc::make_mut`-based overlay onto AppConfig + the `ConfigReloadHandle::seed_post_overlay` method that closes the watch-channel-staleness gap for hot-reload subscribers.

D-1 left two known limitations documented in `deferred-work.md`:

1. **AC#8 precedence inversion** (D-1-FOLLOWUP-1, MED): D-1's overlay has SQLite winning over env-var at boot because the overlay runs AFTER figment's TOML + env-var layering. The proper `env > SQLite > TOML > default` ordering needs a figment Provider rework — a custom Provider that sits between the TOML and env-var layers of the figment stack.

2. **TOML is still authoritative for figment** between D-0's migration and the D-1 overlay step. A hand-edited `config.toml` on a post-D-0-but-pre-D-2 deployment still takes effect on next boot through figment-load → D-1-overlay-merge (with SQLite winning). The TOML file is also still load-bearing for the bootstrap-seed path on fresh deployments.

D-2 closes both gaps:

- **Replaces the boot-time `Arc::make_mut` overlay** with a custom figment Provider that reads `singleton_config` from SQLite and emits values into the figment stack between the TOML layer and the env-var layer. This delivers AC#8's `env > SQLite > TOML > default` ordering as a structural guarantee, not a post-hoc overlay.
- **Codifies the bootstrap-seed-only role of `config.toml`** at runtime. Once `singleton_config` is populated in SQLite (D-0's done-flag set), the figment TOML layer becomes a degraded fallback used only when SQLite values are absent for a key.
- **Emits a once-per-boot `config_toml_unused_warning`** when `config.toml` is present AND the SQLite singleton tables have data. Operators see the warn and know their hand-edits have no effect post-D-2.
- **Removes any remaining `toml_edit` usage** in `src/` and `tests/` (C-6 removed the bulk of it; D-2 sweeps the residue + the `Cargo.toml` dependency line if it isn't still serving a strict-required surface).
- **Rewrites `docs/architecture.md` and the DocBook user manual Configuration chapter** for the final SQLite-canonical model.

### The final three-surface architecture

Post-D-2, opcgw has exactly **three** persistence surfaces:

1. **SQLite** (`data/opcgw.db`, chmod 0o600 per AI-C-SEC-2) — authoritative for ALL non-secret configuration (`[[application]]` tree from C-6 + four singleton sections from D-0) + metric values + command queue + history. Edit via the D-1 web UI; reads happen on every boot via the new figment Provider (D-2 lands).
2. **`config/secrets.toml`** (chmod 0o600 via atomic-rename, established by Story C-0) — operator-supplied secrets: `[chirpstack].api_token` + `[opcua].user_password`. Read at boot via figment's `secrets.toml` provider; never mutated by opcgw at runtime.
3. **`config.toml`** — bootstrap-seed-only. Read at boot via figment's primary TOML provider; values OVERRIDDEN by SQLite for any key the singleton snapshot has set (D-2's figment Provider). Operators MAY delete `config.toml` post-bootstrap-migration; opcgw boots cleanly from SQLite + `secrets.toml` alone.

This is the end-state Guy articulated 2026-05-20: *"In the final version, all configuration should be in database."*

### What D-2 does NOT do

- **Does NOT delete operators' existing `config.toml` files.** Removal is operator action documented in the new D-2 migration runbook section.
- **Does NOT introduce hot-reload for restart-required knobs.** Issue #113's live-borrow refactor is still required for true hot-reload of PKI paths, ports, allowed_origins. D-1's confirmation-modal + supervisor-restart UX continues to handle those.
- **Does NOT migrate `secrets.toml` to SQLite.** Secrets stay in the chmod-0o600 file (post-D-0 threat-model decision per I1-F12).
- **Does NOT change `[command_delivery]` / `[storage]` / `[command_validation]` / `[logging]` semantics.** These were either covered by D-0's `[chirpstack].command_delivery` sub-field migration or are explicitly out of D-0's scope (logging stays env-var-first). D-2 doesn't re-litigate.
- **Does NOT introduce a CRUD endpoint for direct `config.toml` editing.** The file is read-once-at-bootstrap; no UI surface needs it.

---

## Acceptance Criteria

### Figment Provider rework (closes D-1-FOLLOWUP-1)

1. **New `SqliteSingletonProvider` figment Provider** in a new module (suggested: `src/config_provider.rs` or `src/storage/sqlite_singleton_provider.rs` — Dev Agent picks the home). Implements the `figment::Provider` trait. Reads from `SqliteBackend::load_singleton_config()` at provider-evaluation time and emits a `figment::value::Map` of `Profile → Dict → Value` keyed by `section.key` (e.g. `chirpstack.polling_frequency` → `Value::from(10)`). Failures are non-fatal — provider returns an empty Map and emits `config_provider_failed` warn so figment falls back to the next provider in the stack.

2. **Figment stack reorder in `AppConfig::from_path` (and equivalent boot-time loaders).** The new precedence ordering is:
   - **Top (highest)**: env-var overlay (`OPCGW_<SECTION>__<KEY>` via the existing figment env-var provider)
   - **Next**: `SqliteSingletonProvider` (NEW)
   - **Next**: `secrets.toml` (existing C-0 provider, for secret fields only — does not conflict with singleton non-secret keys)
   - **Next**: `config.toml` (existing TOML provider)
   - **Bottom**: serde-defined struct defaults (`#[serde(default = "...")]`)

   This delivers the D-0 spec AC#8 ordering as a structural guarantee. Operators who set `OPCGW_CHIRPSTACK__POLLING_FREQUENCY=5` see env-var win over D-1's SQLite value. Operators who set the value via D-1 UI see SQLite win over the TOML-loaded baseline. **Closes D-1-FOLLOWUP-1.**

3. **`Arc::make_mut` overlay block in `main.rs` is removed.** The D-1 boot-time overlay (`overlay_singletons_from_sqlite_rows` + `seed_post_overlay`) becomes redundant because figment now produces the correct AppConfig directly. The `overlay_singletons_from_sqlite_rows` helper in `src/config.rs` MAY be retained as a unit-tested utility if other code uses it (the D-1 PUT handler still calls it for candidate-AppConfig construction during validation — verify this is still the right shape post-D-2 or rework). `seed_post_overlay` in `ConfigReloadHandle` MAY be retained for forward-compatibility but is no longer called at boot.

4. **The PUT handler in `src/web/singleton_config.rs` retains candidate-AppConfig validation** via `overlay_singletons_from_sqlite_rows` on a cloned current snapshot, OR is reworked to construct the candidate via figment + the new provider with the proposed JSON values overlaid. Dev Agent picks the cleaner shape; consistency with the boot-time path is the requirement.

### `config_toml_unused_warning` audit event

5. **New `config_toml_unused_warning` audit event** emitted once-per-boot when ALL of:
   - `config.toml` exists on disk (the figment loader successfully read it), AND
   - `singleton_config` SQLite table has rows for at least one section (D-0 migration completed)
   
   Fields: `config_path=<str>` (the resolved `config.toml` path), `recommended_action=<str>` (a static string like "config.toml is no longer mutated by opcgw at runtime; verify it matches SQLite via `bash scripts/check-d0-migration.sh` or delete it to remove operator confusion"). Severity: `warn` per AC#24 doc-sync convention. Emitted from `main.rs` immediately after the SqliteSingletonProvider applies on a boot where the existence-check + non-empty-singleton condition both fire.

6. **A boot where `config.toml` is absent but SQLite singleton tables are populated** is the operator's intended post-migration state. No warn fires.

7. **A boot where both `config.toml` is present AND SQLite singleton tables are empty** is a fresh / pre-D-0 deployment. No warn fires; figment uses TOML as the canonical source per the D-0 migration's secondary-guard semantic.

### TOML mutation-surface removal

8. **Audit for residual `toml_edit` usage.** Grep `src/` and `tests/` for `toml_edit::` imports. C-6 commit `1c09911` removed Story 9-5's `src/web/config_writer.rs` (551 lines) and removed `toml_edit` from `Cargo.toml`. D-2 verifies the removal is complete and adds an explicit doc note in `docs/architecture.md` that `toml_edit` is intentionally absent from the dependency tree. If any `toml_edit` reference remains, D-2 deletes it.

9. **Audit for residual figment write paths.** Grep `src/` for `figment::` patterns that could write back to disk. Figment is read-only by design, but check for any custom code that writes the figment-loaded AppConfig back to a TOML file. C-6's spec said "ALL CRUD endpoints write to SQLite"; D-2 verifies no such write-back-to-TOML path survived.

10. **Audit for `std::fs::write` against `config.toml`.** Grep `src/` for any direct file-write code targeting the operator's TOML file. Story C-0's `write_secrets_toml` writes to `secrets.toml` (correct; not `config.toml`). Story 9-7's deleted SIGHUP listener used to read from `config.toml`; verify no write side ever existed.

### Documentation: architecture.md + DocBook + runbook

11. **`docs/architecture.md` rewritten** to reflect the post-D-2 final three-surface model:
    - § "Configuration architecture" section updated to describe SQLite as the authoritative configuration source for all runtime knobs (incl. singletons), with `secrets.toml` as the secret-store surface and `config.toml` as the bootstrap-seed surface read at first post-D-0 boot only.
    - Figment provider stack diagram updated.
    - Precedence ordering note (env > SQLite > TOML > default) explicit.
    - D-1's "in-transition" paragraph (added 2026-05-27) replaced with the final post-D-2 description.
    - `Last updated:` line updated.

12. **DocBook user manual `docs/manual/opcgw-user-manual.xml` Configuration chapter rewritten** (NOT just patched) for the final operator-facing model:
    - `<section id="sec-config-overview">` rewrites the Tier 1 / Tier 2 narrative for the final post-D-2 state (Tier 1 = SQLite via web UI; Tier 2 = `secrets.toml` for operator-supplied secrets; Tier 3 = `config.toml` bootstrap seed only).
    - `<section id="sec-singleton-config-migration">` (added by D-0 iter-0) updated with the D-1 + D-2 evolution (boot-time write done at first post-D-0 boot; figment Provider reads SQLite on every subsequent boot per D-2).
    - New `<section id="sec-config-toml-bootstrap-only">` documenting the post-D-2 `config.toml` operator contract: hand-edits no longer take effect; the file may be deleted; the `config_toml_unused_warning` event fires once-per-boot if both files are present.
    - `xmllint --noout --valid` clean.

13. **`docs/d-0-migration-runbook.md` updated** with a new section "Post-D-2 operator workflow": when D-2 lands, `config.toml` becomes inert; recommended-action: run `bash scripts/check-d0-migration.sh data/opcgw.db` to confirm SQLite is populated, then optionally delete `config.toml`. The `config_toml_unused_warning` event documented.

14. **`docs/logging.md`** gains a discrete event-table row for `config_toml_unused_warning` (closes part of D-1-FOLLOWUP-5 for this one event; the other 7 D-1 events remain section-summary-level per the deferred follow-up).

15. **`docs/security.md`** "Singleton config editor (Story D-1)" subsection finally added (closes D-1-FOLLOWUP-2). Documents the post-D-2 read-path swap, the CSRF + Basic-auth contracts on `/api/config/singleton/*`, the secret-field rejection contract, and the supervisor-restart semantic.

16. **`README.md` Planning row + Current Version** updated to reflect Epic D 3/3 done (post-D-2 review).

### Integration tests

17. **New integration tests in `tests/d2_figment_provider.rs`** (≥ 10 tests):

    1. `SqliteSingletonProvider::data()` returns an empty Map when `singleton_config` table is empty (pre-D-0 / fresh boot).
    2. `SqliteSingletonProvider::data()` returns a populated Map after `migrate_singleton_toml_to_sqlite` has run; keys are `section.key` (e.g. `chirpstack.polling_frequency`).
    3. **Precedence test (env > SQLite)**: SQLite has `polling_frequency=10`; env-var `OPCGW_CHIRPSTACK__POLLING_FREQUENCY=5` is set. Loaded `AppConfig.chirpstack.polling_frequency` is 5.
    4. **Precedence test (SQLite > TOML)**: TOML has `polling_frequency=10`; SQLite has `polling_frequency=20`. Loaded `AppConfig.chirpstack.polling_frequency` is 20.
    5. **Precedence test (TOML > default)**: TOML has a non-default value for a `#[serde(default = "...")]` field; SQLite is empty. Loaded value is the TOML value.
    6. **Precedence test (default fallback)**: All higher layers absent; `#[serde(default)]` value is used.
    7. `config_toml_unused_warning` event fires when `config.toml` is present AND `singleton_config` is non-empty.
    8. `config_toml_unused_warning` event does NOT fire when `config.toml` is absent.
    9. `config_toml_unused_warning` event does NOT fire when SQLite singleton tables are empty (fresh deployment).
    10. **Boot-cycle test**: fresh boot loads `config.toml`, D-0 migration runs, D-2 provider's first invocation returns the migrated values; SECOND boot of same DB returns the same values from SQLite without re-running migration.
    11. **D-1 PUT round-trip**: PUT a new `polling_frequency=15`, supervisor restart simulated by re-invoking the loader, assert the new value is loaded from SQLite via the D-2 provider.
    12. **Secret-field flow-through**: `secrets.toml` carries `api_token`; D-2 provider does NOT shadow it (singleton_config does not have api_token rows). Loaded `chirpstack.api_token` is the secrets.toml value.

### Regression invariants

18. **All 16 D-0 tests** in `tests/sqlite_singleton_config_migration.rs` still pass.

19. **All 12 D-1 tests** in `tests/web_singleton_config.rs` still pass — specifically Test 4 (the SQLite-readback round-trip) and Test 11 (the AppConfig overlay roundtrip — verify the helper is retained or the test is appropriately migrated).

20. **All 19 C-6 tests** in `tests/sqlite_config_migration.rs` still pass.

21. **`tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list`** still passes (the post-2026-05-20-incident regression guard).

22. **`cargo test --all-targets`** total ≥ 1531 / 0 / ≥ 73 (D-1 closed at 1521; D-2 adds ≥ 10 new tests).

23. **`cargo clippy --all-targets -- -D warnings`** clean.

24. **`cargo test --doc`** 0 failed / 73 ignored (no regression vs D-1 baseline).

### Strict-zero file invariants

25. **`Cargo.toml` / `Cargo.lock`** — verify `toml_edit` is absent from the dependency tree (C-6 removed it; D-2 confirms). NO new dependencies (the new figment Provider uses the existing `figment` crate; no new tonic / serde / etc.). Verify via `git diff Cargo.toml Cargo.lock` at flip time.

26. **`src/opc_ua.rs`** — strict-zero. D-2 does not touch OPC UA code.

27. **`src/storage/migrate_singleton_config.rs`** — minimal changes only. D-0's migration logic is preserved; D-2's provider reads via the existing `SqliteBackend::load_singleton_config` helper.

28. **`migrations/`** — strict-zero. No schema migration. The v010 schema from D-0 is sufficient.

### GitHub tracking issue

29. Open a GitHub issue with suggested title `"D-2: Decommission TOML mutation surface + figment Provider rework"`. User opens out-of-band; Dev Agent records the number in Dev Notes + every commit message carries `Refs #N`. Per Epic A/B/C/D precedent: if `gh` CLI is not authenticated for write in the dev session, leave a `Refs #__` placeholder and document in Completion Note.

---

## Tasks / Subtasks

- [ ] **Task 0 — Tracking issue acknowledgment (AC: #29)**
  - [ ] 0.1 Open issue (or document the `Refs #__` placeholder rationale).
  - [ ] 0.2 Capture number in Dev Notes.
  - [ ] 0.3 `Refs #N` in every commit.

- [ ] **Task 1 — `SqliteSingletonProvider` figment Provider (AC: #1, #2)**
  - [ ] 1.1 New module (Dev Agent picks `src/config_provider.rs` or `src/storage/sqlite_singleton_provider.rs`).
  - [ ] 1.2 Implement `figment::Provider` trait: `metadata()` + `data()`. Reads via `SqliteBackend::load_singleton_config()`; on Err returns empty Map + emits `config_provider_failed` warn.
  - [ ] 1.3 Build the `figment::value::Map<Profile, Dict>` shape — `Profile::Default` → nested Dict keyed by section, each section is itself a Dict keyed by field.
  - [ ] 1.4 Wire the provider into `AppConfig::from_path` (and any equivalent loader). Provider order: TOML → secrets.toml → SqliteSingletonProvider → env-var → struct defaults.

- [ ] **Task 2 — Boot-time overlay removal in main.rs (AC: #3, #4)**
  - [ ] 2.1 Remove the D-1 `Arc::make_mut` overlay block in `src/main.rs`. The figment Provider now produces the correct AppConfig directly.
  - [ ] 2.2 Remove the `seed_post_overlay` call in `src/main.rs` (the method itself MAY be retained in `ConfigReloadHandle` for forward-compat; document the decision in the Completion Note).
  - [ ] 2.3 Audit whether the D-1 PUT handler in `src/web/singleton_config.rs` still needs `overlay_singletons_from_sqlite_rows` for candidate-AppConfig validation. If yes, retain the helper; if no, delete it.
  - [ ] 2.4 Verify `application_config` is no longer `let mut` (revert to `let application_config = ...` if iter-1 of D-1's I1-F2 patch is no longer needed).

- [ ] **Task 3 — `config_toml_unused_warning` event (AC: #5, #6, #7)**
  - [ ] 3.1 In `src/main.rs`, after the SqliteSingletonProvider applies, check `Path::new(&config_path).exists()` AND `sqlite_backend.count_singleton_config()? > 0`.
  - [ ] 3.2 If both, emit `tracing::warn!(event="config_toml_unused_warning", config_path=?config_path, recommended_action=...)`.
  - [ ] 3.3 Use a `static AtomicBool` or `OnceLock` to ensure once-per-boot semantic if `ConnectionPool::new` is ever called multiple times in the same process.

- [ ] **Task 4 — TOML mutation-surface audit (AC: #8, #9, #10)**
  - [ ] 4.1 `grep -rn "toml_edit" src/ tests/` — expect 0 hits (C-6 removed).
  - [ ] 4.2 `grep -rn "figment::write\|figment.*save\|figment.*write" src/` — expect 0 hits.
  - [ ] 4.3 `grep -rn "std::fs::write.*config\.toml\|fs::write.*config_path" src/` — expect 0 hits.
  - [ ] 4.4 Document the audit in `docs/architecture.md`'s configuration section.

- [ ] **Task 5 — Documentation: architecture.md + DocBook + runbook (AC: #11, #12, #13, #14, #15)**
  - [ ] 5.1 Rewrite `docs/architecture.md` § "Configuration architecture" for the final three-surface model.
  - [ ] 5.2 Rewrite DocBook `<section id="sec-config-overview">` + update `<section id="sec-singleton-config-migration">` + add `<section id="sec-config-toml-bootstrap-only">`. `xmllint --noout --valid` clean.
  - [ ] 5.3 Add "Post-D-2 operator workflow" section to `docs/d-0-migration-runbook.md`.
  - [ ] 5.4 Add `config_toml_unused_warning` event-table row to `docs/logging.md` (closes part of D-1-FOLLOWUP-5).
  - [ ] 5.5 Add "Singleton config editor (Story D-1)" subsection to `docs/security.md` (closes D-1-FOLLOWUP-2).
  - [ ] 5.6 Update README Planning row + Current Version block.

- [ ] **Task 6 — Integration tests (AC: #17)**
  - [ ] 6.1 Create `tests/d2_figment_provider.rs`.
  - [ ] 6.2 Implement the 12 named tests from AC#17.

- [ ] **Task 7 — Regression gate + commit (AC: #18, #19, #20, #21, #22, #23, #24)**
  - [ ] 7.1 All 16 D-0 + 12 D-1 + 19 C-6 + deadlock-guard tests still pass.
  - [ ] 7.2 `cargo test --all-targets` ≥ 1531/0/≥73.
  - [ ] 7.3 `cargo clippy --all-targets -- -D warnings` clean.
  - [ ] 7.4 `cargo test --doc` 0 failed / 73 ignored.
  - [ ] 7.5 Manual smoke against Guy's real ChirpStack — DEFERRED per the 2026-05-20 main-deadlock incident doctrine.
  - [ ] 7.6 Commit: `Story D-2: Decommission TOML mutation surface - Implementation Complete` + `Refs #<issue>`.

- [ ] **Task 8 — Sprint-status + spec flip (AC: status semantics)**
  - [ ] 8.1 Flip sprint-status `D-2-decommission-toml-mutation: ready-for-dev → review`.
  - [ ] 8.2 Flip spec Status: `ready-for-dev → review`.
  - [ ] 8.3 Completion Note covering: figment Provider design (where it lives + provider order), overlay removal disposition (kept or deleted), `seed_post_overlay` disposition, `config.toml` operator-action recipe, any deferred items added to `deferred-work.md`.

---

## Dev Notes

### Why the figment Provider rework belongs in D-2, not earlier

D-0 deferred the AppConfig read-path swap to D-2 because the proper fix needs to coordinate with the TOML mutation-surface decommission: until the figment Provider is in place, the operator-edit path through D-1's UI writes to SQLite but boot-time figment still loads from TOML. D-1's `Arc::make_mut` overlay was a transitional bridge. D-2 replaces the bridge with the structural ordering.

A cleaner alternative would have been D-0 ships the Provider and D-1 just adds the UI. The reason D-0 didn't: the Provider needs `SqliteBackend::load_singleton_config()` AND figment's Provider trait interaction (which is its own learning curve), AND it has to coordinate with the env-var layer ordering. D-0's spec was tightly scoped; absorbing the Provider work would have widened the iter-1 review surface significantly. The C-6 + D-0 + D-1 cumulative review streak (27 + 28 + 29 = 84 doctrine validations across three stories) validates the per-story scoping discipline.

### Figment Provider shape — design call deferred to Dev Agent

The Dev Agent picks the exact module location + the Provider struct shape during implementation. Recommendations:

- **Module location**: `src/storage/sqlite_singleton_provider.rs` is the natural home alongside `migrate_singleton_config.rs` since the Provider reads via `SqliteBackend::load_singleton_config()`. An alternative `src/config_provider.rs` keeps it next to `src/config.rs` but creates a new top-level module file. Author recommendation: storage submodule.

- **Provider struct shape**: `pub struct SqliteSingletonProvider { backend: Arc<SqliteBackend> }`. The provider's `data()` method calls `backend.load_singleton_config()` and groups by section. Errors are logged + an empty Map is returned (non-fatal — figment falls through to the next provider).

- **`data()` return type**: figment expects `Result<figment::value::Map<figment::Profile, figment::value::Dict>, figment::Error>`. For a single Profile::Default deployment, the outer Map has one entry. The inner Dict is keyed by `section` (e.g. `"chirpstack"`) and each value is itself a `figment::value::Value::Dict` containing the section's fields.

- **Value conversion**: SQLite singleton_config stores JSON-encoded strings. The Provider parses each value via `serde_json::from_str::<serde_json::Value>` and converts the result to `figment::value::Value` via `figment::value::Value::from(serde_value)`. There's an existing `From` impl in figment for `serde_json::Value`; Dev Agent verifies during implementation.

### Overlay-helper disposition

D-1's `AppConfig::overlay_singletons_from_sqlite_rows` is used in two places:

1. **Boot-time** (`main.rs`) — REMOVED by D-2 (the figment Provider replaces this).
2. **PUT handler** (`src/web/singleton_config.rs`) — used to construct a candidate AppConfig for validation before persisting to SQLite. The candidate-construction logic could be refactored to use figment + the new provider in a test-only mode, but that's wider scope than D-2 needs. Author recommendation: retain the helper as a unit-tested utility used only by the PUT handler. The Completion Note documents the choice.

### `seed_post_overlay` disposition

D-1 introduced `ConfigReloadHandle::seed_post_overlay` to close the watch-channel-staleness gap. With the figment Provider in place, the watch channel is seeded post-figment-load with the correct (SQLite-influenced) AppConfig at line ~554; `seed_post_overlay` is no longer needed at boot.

Two options:

- **Delete the method.** Cleanest. Removes dead code. Minor risk: if a future story re-introduces a post-boot overlay pattern, the method would need to be re-created.
- **Retain the method.** Defensive. The implementation is small (~30 lines including the `try_lock` + warn). Future code paths that mutate AppConfig post-boot would have a primitive to reach for.

Author recommendation: **delete it**. Dead code rots; if a future story needs the primitive, it can be re-introduced. The Completion Note documents the choice.

### `config_toml_unused_warning` and the figment-falls-back path

The warn fires only when `config.toml` is PRESENT on disk AND `singleton_config` has data. The intent is to surface a specific operator confusion: "I edited config.toml but my changes aren't taking effect."

A subtler case: an operator edits `config.toml` for a knob that DOES exist in `singleton_config`. Figment loads the TOML value; the SqliteSingletonProvider overrides it with the SQLite value; the operator's hand-edit is silently shadowed. The warn surfaces the discrepancy at boot time so the operator knows their edit was ignored.

Conversely: an operator edits `config.toml` for a knob that does NOT exist in `singleton_config` (e.g. a `[storage]` or `[command_validation]` knob that D-0 did NOT migrate). The TOML value flows through to AppConfig because the SqliteSingletonProvider has no shadowing entry. The warn STILL fires because the existence-AND-non-empty check is unconditional. This is the right behavior: the operator's overall workflow is ambiguous post-D-2 if they continue to hand-edit `config.toml`; we surface the issue at every boot.

### Why NOT delete operators' `config.toml` files

opcgw should never delete operator-owned files. The `config.toml` may contain:

- Comments documenting the operator's deployment.
- Settings for keys that aren't yet in `singleton_config` (e.g. `[logging]`, `[storage]`, `[command_validation]`).
- A reference point the operator uses for documentation or rollback.

D-2 emits the warn and the operator decides whether to delete, archive, or trim the file. The runbook gives explicit operator-action recipes for each scenario.

### Closing the D-1 + D-2 finding-class lessons

D-2 also closes (via documentation rather than code):

- **D-1-FOLLOWUP-2**: `docs/security.md` "Singleton config editor (Story D-1)" subsection.
- **D-1-FOLLOWUP-3**: `docs/architecture.md` precedence-inversion paragraph (replaced with the post-D-2 final-state description; the in-transition phrasing is removed).
- **D-1-FOLLOWUP-4**: DocBook user manual section.

Partial close: **D-1-FOLLOWUP-5** (discrete event-table rows for the 7 D-1 events) is partially closed — `config_toml_unused_warning` gets a row in D-2's Task 5.4. The other 7 D-1 events (`config_overlay`, `config_overlay_failed`, `config_get_singleton`, `singleton_config_updated`, `singleton_config_rejected`, `singleton_config_restart_required`, `singleton_config_storage_error`) remain at section-summary level. **Reconsidering**: `config_overlay` and `config_overlay_failed` are now dead post-Task 2 (the overlay is gone). Their event-rows would document an event no longer emitted. Dev Agent confirms removal in `docs/logging.md` Task 5.4.

### Carry-forward GitHub issues / out-of-scope

- **#108** (storage payload-less MetricType) — closed in Epic A. D-2 does not touch.
- **#113** (live-borrow refactor for restart-required knobs) — D-2 does NOT close this; PKI paths / ports / allowed_origins remain restart-required.
- **AI-C-SEC-1** (`prune_old_metrics` SQL format-string) — NOT D-2's scope.
- **AI-C-SEC-3** (`setup_get` filename log) — NOT D-2's scope (Story C-0 territory; simple one-line fix).
- **AI-C-1 / AI-C-2 + AI-A-1/2/3 + AI-B-1/2/3 + D-1 LOW carry-forwards** — cumulative skill-codification debt unchanged by D-2. Continues to need a dedicated v2.x skill-codification epic.

### Test budget delta

D-2 adds ≥ 10 integration tests in `tests/d2_figment_provider.rs` + ~5 unit tests for the Provider (Provider::metadata / data / error handling). Net delta: ≥ +15 tests. Target ≥ 1531 / 0 / ≥ 73 (D-1 closed at 1521; +10 minimum).

### References

- Epic D scope: `_bmad-output/planning-artifacts/epics.md § Epic D § Story D.2`
- D-0 precedent + the SQLite store: `src/storage/migrate_singleton_config.rs`, `src/storage/sqlite.rs::load_singleton_config`
- D-1 precedent (overlay + UI): `src/config.rs::overlay_singletons_from_sqlite_rows`, `src/config_reload.rs::seed_post_overlay`, `src/web/singleton_config.rs`
- Figment Provider trait: see `figment` crate documentation; the existing TOML + env-var providers in `src/config.rs::AppConfig::from_path` are the closest in-codebase examples.
- D-1-FOLLOWUP-1 through D-1-FOLLOWUP-5 in `_bmad-output/implementation-artifacts/deferred-work.md`.
- Memory references (out-of-tree):
  - `project_epic_d_singleton_config_vision.md` — Epic D scope finalised 2026-05-26
  - `session_2026_05_27_d1_review_done.md` — D-1 closed 2026-05-27, 29th doctrine validation
  - `feedback_iter3_validation.md` — 29× streak; D-2 expected to extend it

### Project Structure Notes

- New files D-2 introduces:
  - `src/storage/sqlite_singleton_provider.rs` (or `src/config_provider.rs`) — figment Provider implementation
  - `tests/d2_figment_provider.rs` — 12 integration tests
  - DocBook `<section id="sec-config-toml-bootstrap-only">` (new section in the existing manual)
- Files D-2 modifies:
  - `src/main.rs` — remove overlay block + add config_toml_unused_warning emit
  - `src/config.rs` — wire SqliteSingletonProvider into the figment stack in `AppConfig::from_path`; optionally remove `overlay_singletons_from_sqlite_rows` if PUT handler is reworked
  - `src/config_reload.rs` — optionally remove `seed_post_overlay`
  - `src/web/singleton_config.rs` — minimal changes (PUT handler retains current candidate-validation shape per author recommendation)
  - `src/storage/mod.rs` — re-export the new Provider module
  - `docs/architecture.md`, `docs/logging.md`, `docs/security.md`, `docs/manual/opcgw-user-manual.xml`, `docs/d-0-migration-runbook.md`, `README.md`
  - `_bmad-output/implementation-artifacts/sprint-status.yaml` — flip D-2 ready-for-dev → review
- Files D-2 strict-zero touches:
  - `Cargo.toml`, `Cargo.lock` (no new dependencies)
  - `src/opc_ua.rs`, `src/storage/migrate_config.rs`, `src/storage/migrate_singleton_config.rs` (preserved)
  - `migrations/` (no new schema migration)

---

## Out of Scope

- **Migrating `secrets.toml` to SQLite.** Secrets stay in chmod-0o600 file.
- **Live hot-reload for restart-required knobs.** Issue #113 territory.
- **Migrating `[logging]` / `[storage]` / `[command_validation]` to SQLite.** Future v2.x stories.
- **Deleting operators' existing `config.toml` files.** Operator action, documented in runbook.
- **A web UI for `secrets.toml` rotation.** C-0 wizard handles first-run; rotation stays operator-side via `secrets.toml` or env-var.
- **`CR-EPIC-C-MQTT`** (MQTT real-time path) — still deferred per Epic C scope decision.
- **Closing AI-C-SEC-1 / AI-C-SEC-3 / AI-C-1/2 / AI-B-1/2/3 / AI-A-1/2/3** — cumulative skill-codification debt; needs dedicated v2.x epic.

---

## Completion Note

To be filled in by the Dev Agent at story completion. Should include:

- Actual test count delta (gross +N, net after any tests refactored).
- The Task 1.1 module location pick (`src/storage/sqlite_singleton_provider.rs` vs `src/config_provider.rs`).
- The Task 2.3 disposition of `overlay_singletons_from_sqlite_rows` (retained for PUT handler, or deleted).
- The Task 2.2 disposition of `seed_post_overlay` (retained for forward-compat, or deleted).
- The Task 5.4 disposition of the D-1 audit events (`config_overlay` + `config_overlay_failed` removal from logging.md if the overlay code is deleted).
- Confirmation that all 16 D-0 + 12 D-1 + 19 C-6 + deadlock-guard tests still pass.
- Confirmation that the figment Provider stack delivers the AC#8 precedence ordering (env > SQLite > TOML > default) via the integration tests.
- Manual smoke against Guy's real ChirpStack — DEFERRED per the 2026-05-20 main-deadlock incident doctrine; record the deferral.
- The GitHub tracking issue number (or `Refs #__` placeholder rationale).
- Any deferred follow-ups added to `deferred-work.md`.
- Any architectural decisions captured during implementation (e.g. figment Provider error-handling strategy, secret-field invariants in the Provider).

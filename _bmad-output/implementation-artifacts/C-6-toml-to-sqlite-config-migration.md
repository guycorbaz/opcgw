# Story C-6: TOML→SQLite Configuration Migration + SQLite-Driven Hot-Reload

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `C-6-toml-to-sqlite-config-migration`                                                                       |
| Epic            | C — Auto-Discovery and Web-First Configuration (post-v2.0 GA)                                               |
| FRs             | none (Epic C is post-PRD)                                                                                   |
| Status          | review                                                                                                      |
| Created         | 2026-05-21                                                                                                  |
| Source epic     | `_bmad-output/planning-artifacts/epics.md § Epic C § Story C.6`                                             |
| Depends on      | C-0 (empty-bootstrap so the gateway no longer treats TOML as a hard requirement), C-2 (web UI is the       |
|                 | canonical write surface), C-3 (server-side validation is storage-medium-independent). Strictly: this is    |
|                 | the LAST story in Epic C and must land after the other 5.                                                   |
| Tracking        | GitHub issue `#__` — user opens out-of-band                                                                 |

---

## User Story

As an **opcgw operator running v2.x+ with an established configuration**,
I want opcgw's authoritative `[[application]]` tree (applications, devices, metrics, commands) stored in SQLite alongside the metric values, with TOML reduced to a bootstrap-only seed file,
So that all writes (web UI CRUD, future automation APIs, eventual ChirpStack-driven auto-sync) hit a single canonical store, and so the gateway's "what is configured" answer comes from one place — not split between an in-memory snapshot and a TOML file.

---

## Story Context

### Why C-6 is the closing story of Epic C

Guy's articulated end-state for the configuration architecture (2026-05-20): *"In the final version, all configuration should be in database."* The Epic C vision document (`[[project_epic_c_auto_discovery_vision]]`) promotes this from "optional, may not be needed" to "explicitly part of the vision."

Today's shape is a hybrid:
- TOML file (`config/config.toml`) is the canonical source.
- `AppConfig` deserialises from TOML at boot.
- Story 9-5 added byte-preserving TOML mutation via `toml_edit` so CRUD writes update the file in place.
- Story 9-7 added a TOML file-watcher hot-reload that detects external edits.
- SQLite stores ONLY metric values + commands + gateway status (not the `[[application]]` tree itself).

C-6 moves the `[[application]]` tree (apps, devices, metrics, commands) from TOML to SQLite. Post-C-6:
- SQLite is the authoritative store for the `[[application]]` tree.
- The TOML file may continue to exist as a one-shot migration seed on fresh DBs, but opcgw no longer mutates it at runtime.
- The OPC UA address-space builder, ChirpStack poller, and web UI all read from SQLite (via the existing storage trait pattern).
- Hot-reload changes meaning: instead of "TOML file changed → rebuild snapshot," it becomes "SQLite tables changed → rebuild snapshot." The TOML file-watcher (Story 9-7's primitive) is REMOVED.

### What stays in TOML (out of C-6 scope)

The singleton sections `[global]`, `[chirpstack]`, `[opcua]`, `[web]` remain in TOML for v2.x:
- These are operator-environment config (server endpoints, ports, security settings) not application-tree state.
- They're typically set once during initial deployment and rarely change.
- C-0 already accommodates the OPC UA password via `secrets.toml`, which is the singleton-secret separation pattern.

A FUTURE story (post-Epic-C) may move the singletons into SQLite + an admin settings UI. That's not C-6.

### What "post-C-6 hot-reload" looks like

The scope-time decision (Guy 2026-05-21): **hot-reload after C-6 is SQLite-driven, not TOML-driven.**

- The existing TOML file-watcher primitive (Story 9-7) is removed entirely.
- Hot-reload now means: a SQLite write to the application/device/metric/command tables completes → opcgw rebuilds the in-memory snapshot from SQLite → triggers the same downstream rebuild path Story 9-7 wired up (OPC UA address-space rebuild, ChirpStack poller config refresh, dashboard cache invalidation).
- The trigger mechanism is the **CRUD write completion** — there's no file system to watch. The web CRUD handlers, after they commit to SQLite, call the same reload-emit that Story 9-7's file-watcher used to call.
- The operator's perspective is unchanged: "I edited config via the web UI; the live system caught up." Just the implementation underneath swaps file-watching for write-completion-notifying.

A future "restore from TOML backup" admin operation MAY be added later (post-C-6 follow-up story); C-6 itself does NOT ship that endpoint.

### Migration timing and one-way contract

The migration runs **once**, on the first boot of a gateway that has the C-6 binary AND an existing pre-C-6 SQLite DB. Detection: read the schema version from the meta table; if `< 9` AND the TOML `application_list` is non-empty AND the SQLite `applications` table is empty, perform the migration.

The migration is **one-way**: post-C-6, opcgw writes to SQLite as the authoritative store. Downgrading to a pre-C-6 binary means the operator's recent web-UI CRUD changes (made on the post-C-6 binary) won't be in the TOML file the older binary reads. Rollback requires restoring the pre-migration `opcgw.db` backup the runbook (AC#7) instructs the operator to take.

---

## Acceptance Criteria

### SQLite schema migration v009

1. **New migration file `migrations/v009_application_config_tables.sql`** introduces the application/device/metric/command tables.
    - Schema (Dev Agent may refine column types/names; this is the structural shape):
      ```sql
      CREATE TABLE applications (
          application_id TEXT PRIMARY KEY,
          application_name TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
      );
      
      CREATE TABLE devices (
          application_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          device_name TEXT NOT NULL,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          PRIMARY KEY (application_id, device_id),
          FOREIGN KEY (application_id) REFERENCES applications(application_id) ON DELETE CASCADE
      );
      
      CREATE TABLE metrics (
          application_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          chirpstack_metric_name TEXT NOT NULL,
          metric_name TEXT NOT NULL,
          metric_type TEXT NOT NULL,
          metric_unit TEXT,
          created_at TEXT NOT NULL,
          updated_at TEXT NOT NULL,
          PRIMARY KEY (application_id, device_id, chirpstack_metric_name),
          FOREIGN KEY (application_id, device_id) REFERENCES devices(application_id, device_id) ON DELETE CASCADE
      );
      
      CREATE TABLE commands (
          application_id TEXT NOT NULL,
          device_id TEXT NOT NULL,
          command_name TEXT NOT NULL,
          command_id INTEGER NOT NULL,
          PRIMARY KEY (application_id, device_id, command_name),
          FOREIGN KEY (application_id, device_id) REFERENCES devices(application_id, device_id) ON DELETE CASCADE
      );
      
      -- Indexes on common query patterns (Dev Agent profiles before committing)
      CREATE INDEX idx_metrics_by_device ON metrics(application_id, device_id);
      CREATE INDEX idx_commands_by_device ON commands(application_id, device_id);
      ```
    - `CASCADE` on FK delete: removing an application removes its devices, metrics, and commands. Matches the existing nested-TOML semantics.
    - `(application_id, device_id, chirpstack_metric_name)` composite PK enforces C-3's same-device-same-metric duplicate prevention at the schema level. C-3's validator continues to run; schema is defence-in-depth.
    - `(application_id, device_id)` composite PK on `devices` enforces same-app-same-device DevEUI uniqueness. Same DevEUI under DIFFERENT applications is ALLOWED because the PK includes `application_id`.

2. **Schema version bumps from 8 to 9** in `src/storage/schema.rs`. The `MIGRATION_V009` const is added; `run_migrations` gains a v9 branch.

3. **Migration is idempotent.** Running migrations on an already-v9 DB is a no-op. Standard schema-version check pattern from the existing codebase.

### One-shot data migration (TOML → SQLite)

4. **First-boot migration logic** lives in `src/storage/migrate_config.rs` (new module) OR `src/main.rs` (Dev Agent decides; Dev Notes documents).
    - At boot, AFTER schema migrations have run:
      - Read schema version from meta table.
      - If schema version == 9 AND `applications` table is empty AND `AppConfig.application_list` is non-empty (i.e., TOML has content):
        - Begin transaction.
        - INSERT each TOML application into `applications`.
        - INSERT each TOML device into `devices`.
        - INSERT each TOML metric into `metrics`.
        - INSERT each TOML command into `commands`.
        - Set a `config_migrated_from_toml_at` meta-table entry to the current timestamp.
        - Commit.
        - Emit `event="config_migration" stage="toml_to_sqlite" applications=<N> devices=<N> metrics=<N> commands=<N> duration_ms=<ms>`.
      - If `applications` table is non-empty: migration has already run, skip (idempotent).
      - If `application_list` is empty (C-0 empty-bootstrap path): no migration needed; emit `event="config_migration" stage="skipped_empty_source"`.

5. **Migration verification.**
    - After the INSERTs, perform a count-check: number of rows in each SQLite table MUST equal the count of corresponding nodes in `AppConfig.application_list`.
    - If counts mismatch: rollback transaction, leave SQLite empty, emit `event="config_migration_failed" reason="row_count_mismatch" expected=<N> actual=<M>`, fall back to TOML-driven boot (the OLD pre-C-6 codepath, kept available as a transition safety net for the migration boot only).
    - Hashlevel deeper check (Dev Agent's discretion): after INSERTs, READ from SQLite, deserialise into an `AppConfig` clone, byte-compare to the TOML-loaded `AppConfig`. If divergent: rollback + emit `event="config_migration_failed" reason="content_divergence"`.

### Post-migration runtime: SQLite as authoritative store

6. **The in-memory `AppConfig.application_list` is now built from SQLite, not TOML, post-migration.**
    - A new function `load_application_list_from_sqlite()` reads all four tables and constructs the `Vec<ChirpStackApplications>` (or whatever the in-memory type is) used by the OPC UA address-space builder + ChirpStack poller + web UI.
    - The function runs at boot AFTER the migration check, and at every CRUD-write completion (hot-reload trigger).
    - The function is the single point of truth for "rebuild the in-memory snapshot." Subscribers (OPC UA, poller, web UI) consume the result via the existing `Arc<RwLock<…>>` or `Arc<Mutex<…>>` pattern (verify against current code during implementation).

7. **All CRUD endpoints write to SQLite, not TOML.**
    - The application/device/metric/command CRUD handlers in `src/web/api.rs` (Stories 9-4 / 9-5 / 9-6) are refactored:
      - On POST: INSERT into the appropriate SQLite table.
      - On PUT: UPDATE the appropriate row.
      - On DELETE: DELETE (cascading per FK).
      - After the commit, call `load_application_list_from_sqlite()` to rebuild the in-memory snapshot, then trigger the existing reload-emit (OPC UA address-space rebuild, poller refresh).
    - The `toml_edit` mutation path from Story 9-5 is **removed**. The TOML file is no longer touched by opcgw at runtime.
    - Audit events emitted by CRUD handlers continue to fire with their existing shape — only the storage backend behind them changes.

8. **The OPC UA address-space builder and ChirpStack poller read from SQLite via the in-memory snapshot, NOT directly from SQLite each tick.**
    - The hot-reload mechanism is the bridge: writes invalidate the snapshot, the snapshot is rebuilt, subscribers see the new snapshot.
    - Direct SQLite reads from the poll loop would add per-cycle query overhead and complicate the existing read path. The in-memory snapshot pattern (already in place for `application_list`) is preserved.

### Post-C-6 hot-reload (SQLite-driven, not TOML-driven)

9. **The TOML file-watcher (Story 9-7's primitive) is removed.**
    - The file-watcher initialization in `src/main.rs` (or wherever Story 9-7 wired it up) is deleted.
    - The file-watcher module file (whatever Story 9-7 named it — `src/web/hot_reload.rs` or `src/config_reload.rs`) is either deleted entirely OR repurposed as the new SQLite-driven reload notifier.
    - The existing reload-emit interface that Story 9-7 introduced is preserved (subscribers continue to call `subscribe_to_reload(...)` or whatever the function is named); only the trigger source swaps.

10. **CRUD-completion triggers reload.**
    - After each successful CRUD write, the handler calls `notify_config_reload(trigger="crud_write", resource_type=..., source_ip=...)`.
    - The notification cascades: rebuild snapshot → notify OPC UA → notify poller → notify web UI dashboard cache → emit `event="config_reload" trigger="crud_write" ...`.
    - The reload completes WITHIN the HTTP response of the CRUD call (the operator's PUT returns 200 only after the new state is reflected in the in-memory snapshot). This avoids the "I edited but the dashboard hasn't refreshed" race window.

11. **No file-system trigger.**
    - Hand-editing `config.toml` post-C-6 has NO effect at runtime.
    - The TOML file may be stale-stale (rarely-updated bootstrap-seed) or may have been deleted entirely (operator who's gone fully SQLite-native).
    - A future story may add a "restore from TOML backup" admin endpoint per Dev Notes; not C-6.

### Migration runbook + backup contract

12. **`docs/c-6-migration-runbook.md` (NEW)** documents:
    - Pre-migration mandatory backup: `cp opcgw.db opcgw.db.pre-c6-backup` AND `cp config/config.toml config/config.toml.pre-c6-backup`.
    - Container-mount note: ensure the SQLite DB volume mount is the same as the TOML mount so backups can be taken on the host.
    - Expected migration timing on first post-C-6 boot: typically < 100 ms for inventories of < 100 devices; up to a few seconds for > 1000 devices.
    - Verification step post-migration: `sqlite3 opcgw.db 'SELECT COUNT(*) FROM applications, devices, metrics, commands;'` returns counts matching `wc -l < <(grep -c '^\[\[application\]\]' config/config.toml)` etc.
    - One-way contract: rollback to a pre-C-6 binary requires restoring `opcgw.db.pre-c6-backup`.
    - Failure-mode handling: if migration fails with `event="config_migration_failed"`, the gateway boots in TOML-driven mode (transition safety net). Operator's logs surface the failure; operator can either (a) fix the underlying issue and restart (idempotent migration retries on next boot), or (b) report the failure as a bug.

13. **`scripts/check-c6-migration.sh` (NEW)** is a small bash script that runs the verification step from AC#12 + emits a green/red summary. Operator runs it post-migration as a sanity check.

### Sprint-status + spec invariants

14. **Story 9-5 (`toml_edit` byte-preserving TOML mutation) becomes vestigial post-C-6.**
    - The code in `src/web/config_writer.rs` (550 lines, per the wc -l earlier) is removed entirely OR reduced to a no-op stub if any test fixture still imports it.
    - The tests for Story 9-5's TOML write paths (`tests/web_*_crud.rs` byte-preservation assertions) are removed or rewritten to assert SQLite state instead.
    - The Dev Agent must walk all of `tests/web_*_crud.rs` and identify TOML-byte assertions; either delete them (preferred) or rewrite to assert against the SQLite state.

15. **Story 9-7 (TOML hot-reload) becomes vestigial post-C-6.**
    - The file-watcher code is removed (AC#9).
    - Tests asserting "hand-edit TOML → snapshot reloads" are removed.
    - Tests asserting "snapshot reloads after CRUD write" are KEPT (and may be retooled if they previously tested via TOML-file mutation; the new test mutates via SQLite write).

16. **DocBook user manual rewrite for the Configuration chapter.**
    - The Configuration chapter (added in Story B-1) describes the TOML-based flow.
    - Post-C-6 it describes the web-UI-driven flow as canonical; mentions TOML only as the bootstrap-seed.
    - Significant content rewrite, NOT just edits — Dev Notes captures the scope estimate.
    - DocBook 4.5 syntax preserved per memory `[[project_user_manual_format]]`.

### Integration tests

17. **Integration tests** in a new `tests/sqlite_config_migration.rs` + extensions to existing `tests/web_*_crud.rs`. At minimum 14 tests covering:
    - Fresh DB + populated TOML on boot → migration runs, SQLite tables match TOML.
    - Re-boot with already-migrated SQLite → migration no-ops; counts unchanged.
    - Fresh DB + empty TOML on boot (C-0 path) → migration skipped, SQLite stays empty.
    - Migration with 1000+ devices → completes within 5 seconds (performance sanity).
    - Mid-migration crash simulation → next boot retries (idempotent — rollback worked).
    - Post-migration CRUD POST → writes to SQLite + in-memory snapshot reflects new state.
    - Post-migration CRUD DELETE on application → cascade removes devices+metrics+commands.
    - Post-migration: OPC UA browse tree rebuilds on CRUD completion (assert via mock OPC UA client).
    - Post-migration: dashboard `/api/applications` reads from SQLite (verify by direct SQLite write + GET response).
    - TOML file is NOT mutated post-migration (assert mtime unchanged after a CRUD write).
    - Hot-reload trigger fires on CRUD completion (assert via the existing reload subscriber).
    - Story 9-5 `toml_edit` code is unreachable post-C-6 (verify via `grep` that the module is removed OR via a "removed-marker" test).
    - C-3 duplicate-prevention still works post-C-6 (verify via the SQLite-level PK constraint AND the validator).
    - Migration failure path: simulate a row count mismatch, assert rollback + fall-back to TOML mode.

18. **`tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list` continues to pass.** C-0's invariant is preserved post-C-6.

### Regression invariants

19. **`cargo test --all-targets` passes.** Pre-C-6 baseline depends on C-0..C-4 deltas. Target floor: ≥ 1327 / 0 / ≥ 10 (assumes C-4 lands at ≥ 1313, plus C-6's +14 from AC#17, minus any tests removed per AC#14/AC#15). Document actual delta in Dev Notes.

20. **`cargo clippy --all-targets -- -D warnings` clean.**

21. **`cargo test --doc` no regressions.** ≥ 56 ignored, 0 failed.

22. **Strict-zero file invariants.** NO changes to: `src/opc_ua_auth.rs` (auth is unchanged), `src/storage/types.rs` (MetricValue, etc., unchanged), `src/storage/memory.rs` (in-memory backend continues to test). Mutable scope:
    - `migrations/v009_application_config_tables.sql` (NEW)
    - `src/storage/schema.rs` (V009 const + run_migrations branch)
    - `src/storage/sqlite.rs` (new methods for application-config table read/write)
    - `src/storage/migrate_config.rs` (NEW) OR `src/main.rs` (migration boot logic)
    - `src/main.rs` (remove file-watcher init; wire SQLite-driven snapshot rebuild)
    - `src/web/api.rs` (CRUD handlers refactored to SQLite)
    - `src/web/config_writer.rs` (DELETE — Story 9-5 vestigial; OR convert to stub)
    - Story 9-7's hot-reload module (DELETE or repurpose)
    - `tests/sqlite_config_migration.rs` (NEW)
    - `tests/web_*_crud.rs` (significant rewrites to assert SQLite state instead of TOML)
    - `docs/c-6-migration-runbook.md` (NEW)
    - `scripts/check-c6-migration.sh` (NEW)
    - `docs/architecture.md` (reflect SQLite as authoritative store)
    - `docs/manual/opcgw-user-manual.xml` (Configuration chapter rewrite per AC#16)
    - `docs/logging.md` (config_migration + config_migration_failed audit events)
    - `README.md` (Planning table + Configuration section overhaul)
    - `_bmad-output/implementation-artifacts/sprint-status.yaml`
    - This story spec file

### Documentation sync

23. **`docs/architecture.md`** rewritten to reflect SQLite as the configuration source of truth post-C-6. The current architecture doc describes the storage trait as just "metric values" — extend to "all configuration + metric values."

24. **`docs/logging.md`** — `config_migration`, `config_migration_failed`, `config_reload trigger=crud_write` audit events documented.

25. **`README.md` Planning table** Epic C row updated to "Epic C 6/6 done." Configuration section updated to acknowledge SQLite-driven config + bootstrap-seed TOML.

26. **DocBook user manual `docs/manual/opcgw-user-manual.xml`** Configuration chapter rewrite (AC#16). Verify with `xmllint --noout --valid`.

### GitHub tracking issue

27. GitHub tracking issue (suggested title: "C-6: TOML→SQLite configuration migration + SQLite-driven hot-reload") opened by user out-of-band. **Strongly recommended:** also open a dedicated retrospective issue for "Story 9-5 / 9-7 deprecation" so the historical context isn't lost — those stories' deliverables become inert with C-6.

---

## Tasks / Subtasks

- [ ] **Task 0 — Tracking issue acknowledgment (AC: #27)**
  - [ ] 0.1 Open issue.
  - [ ] 0.2 Capture number in Dev Notes.
  - [ ] 0.3 `Refs #N` in every commit.

- [ ] **Task 1 — Schema migration v009 (AC: #1, #2, #3)**
  - [ ] 1.1 Write `migrations/v009_application_config_tables.sql` per AC#1.
  - [ ] 1.2 Add `MIGRATION_V009` const + branch in `src/storage/schema.rs::run_migrations`.
  - [ ] 1.3 Schema-migration tests: fresh DB → v009 applied; v008 DB → v009 applied; v009 DB → no-op.

- [ ] **Task 2 — SQLite CRUD methods for the new tables (AC: #6, #7)**
  - [ ] 2.1 In `src/storage/sqlite.rs`, add methods: `insert_application`, `update_application`, `delete_application`, `insert_device`, `update_device`, `delete_device`, `insert_metric`, `update_metric`, `delete_metric` (and command equivalents per Story 9-6).
  - [ ] 2.2 Add `load_all_applications_config()` that reads all four tables and constructs the in-memory `Vec<ChirpStackApplications>`.
  - [ ] 2.3 Use prepared statements / `prepare_cached` per existing performance pattern in sqlite.rs.
  - [ ] 2.4 Transaction wrapping for the cascade-aware writes (delete app → delete devices → delete metrics+commands).
  - [ ] 2.5 Unit tests for each CRUD method.

- [ ] **Task 3 — One-shot TOML→SQLite migration (AC: #4, #5)**
  - [ ] 3.1 Create `src/storage/migrate_config.rs`.
  - [ ] 3.2 `migrate_toml_to_sqlite(app_config: &AppConfig, conn: &Connection) -> Result<MigrationReport, OpcGwError>` — detects need, runs the transaction, returns counts + duration.
  - [ ] 3.3 Wire into `src/main.rs` post-schema-migration, pre-poller-start.
  - [ ] 3.4 Audit event emission on success + failure.
  - [ ] 3.5 Row-count + content-hash verification per AC#5.
  - [ ] 3.6 Integration test: populate TOML, run migration, assert SQLite state matches.

- [ ] **Task 4 — Refactor CRUD endpoints to SQLite (AC: #7)**
  - [ ] 4.1 Walk every CRUD handler in `src/web/api.rs` (POST/PUT/DELETE for applications, devices, metrics, commands).
  - [ ] 4.2 Replace `toml_edit` mutation with SQLite CRUD via the new storage methods (Task 2).
  - [ ] 4.3 After the SQLite commit, call `notify_config_reload(...)` — see Task 5.
  - [ ] 4.4 Existing audit events preserved.
  - [ ] 4.5 Rewrite `tests/web_*_crud.rs` byte-preservation assertions into SQLite-state assertions.

- [ ] **Task 5 — SQLite-driven hot-reload (AC: #9, #10, #11)**
  - [ ] 5.1 Remove the TOML file-watcher initialisation from `src/main.rs` (or wherever Story 9-7 wired it).
  - [ ] 5.2 Delete or repurpose Story 9-7's file-watcher module (`src/web/hot_reload.rs` or `src/config_reload.rs`).
  - [ ] 5.3 New `notify_config_reload(trigger: &str, ...)` function that subscribers (OPC UA, poller, dashboard) call.
  - [ ] 5.4 Subscribers re-read from SQLite (via Task 2's `load_all_applications_config()`).
  - [ ] 5.5 Audit event `event="config_reload" trigger="crud_write"` per existing taxonomy.
  - [ ] 5.6 Tests: CRUD write → snapshot rebuild → OPC UA address space reflects new state.

- [ ] **Task 6 — Story 9-5 cleanup (AC: #14)**
  - [ ] 6.1 Delete `src/web/config_writer.rs` entirely (or reduce to a stub if any external import requires it).
  - [ ] 6.2 Remove `toml_edit` from `Cargo.toml` IF no other code uses it. Verify via `grep -r 'toml_edit'`. (Note: this is the one allowed `Cargo.toml` change.)
  - [ ] 6.3 Remove byte-preservation assertions from `tests/web_*_crud.rs`.

- [ ] **Task 7 — Migration runbook + verification script (AC: #12, #13)**
  - [ ] 7.1 Write `docs/c-6-migration-runbook.md`.
  - [ ] 7.2 Write `scripts/check-c6-migration.sh`.
  - [ ] 7.3 Manually test the script against a freshly-migrated DB.

- [ ] **Task 8 — DocBook + architecture doc rewrite (AC: #16, #23)**
  - [ ] 8.1 Rewrite `docs/manual/opcgw-user-manual.xml § Configuration` for the web-UI-driven flow + bootstrap-seed TOML mention.
  - [ ] 8.2 Update `docs/architecture.md` to reflect SQLite as authoritative store.
  - [ ] 8.3 Verify DocBook DTD with `xmllint`.

- [ ] **Task 9 — Documentation sync (AC: #24, #25)**
  - [ ] 9.1 `docs/logging.md` — new audit events.
  - [ ] 9.2 `README.md` — Planning table to "Epic C 6/6 done"; Configuration section overhaul.

- [ ] **Task 10 — Integration tests (AC: #17, #18)**
  - [ ] 10.1 Create `tests/sqlite_config_migration.rs`.
  - [ ] 10.2 Implement the 14 named tests from AC#17.
  - [ ] 10.3 Verify `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list` still passes.

- [ ] **Task 11 — Regression gate + commit (AC: #19, #20, #21, #22)**
  - [ ] 11.1 `cargo test --all-targets` → record count; target ≥ 1327/0/≥10 minus removed tests.
  - [ ] 11.2 `cargo clippy --all-targets -- -D warnings` → clean.
  - [ ] 11.3 `cargo test --doc` → no regressions.
  - [ ] 11.4 Manual smoke test against Guy's real ChirpStack: spin up gateway with pre-C-6 TOML, observe migration emit, do web-UI CRUD, observe SQLite state, verify TOML mtime unchanged.
  - [ ] 11.5 Commit message: `Story C-6: TOML→SQLite migration + SQLite-driven hot-reload - Implementation Complete` + `Refs #<issue>`.

---

## Dev Notes

### Why C-6 is the LAST story in Epic C (sequencing rationale)

C-6 changes the storage substrate underneath every other Epic C story:
- C-0's `is_first_run()` reads SQLite (post-C-6) to check for password presence.
- C-1's `/api/inventory/*` reads no opcgw state — unaffected by C-6.
- C-2's pickers write via existing CRUD endpoints — automatically benefit from C-6 once CRUD switches to SQLite.
- C-3's validator runs on the in-memory snapshot — substrate-independent.
- C-4's drift view compares opcgw snapshot to ChirpStack — substrate-independent.

By landing C-6 LAST, every other story has a chance to land + iterate against the simpler TOML substrate first. The C-6 migration then swaps the substrate once with all the CRUD endpoints + UX validated.

### Why one-way (no automated rollback)

A bidirectional migration (SQLite → TOML on downgrade) would be:
- Complex to implement (round-tripping TOML-edit byte-preservation across two storage backends).
- A foot-gun (operator runs downgrade, loses recent web-UI changes, can't tell which).
- Solving a problem that backups already solve (the runbook tells the operator to take a backup; restoring is a `cp`).

One-way + documented backup contract is simpler and safer.

### What happens to operators who hand-edit `config.toml` post-C-6

Nothing. The TOML file is no longer monitored. Their edits are ignored.

In a future story we may add:
- A `POST /api/admin/reimport-toml` endpoint that re-runs the TOML→SQLite migration (with operator confirmation) for "I want to restore from a TOML backup" cases.
- A dashboard warning when `config.toml.mtime > config_migrated_from_toml_at` — "your TOML file has been edited but opcgw is now SQLite-canonical; the edits are ignored."

Both are out of C-6 scope.

### Performance considerations

The migration runs INSIDE the boot path before the poller and OPC UA server start. For inventories of < 1000 devices, the migration completes in << 1 second (single transaction, batched inserts). For larger deployments, batch the inserts in chunks of 500 rows to avoid SQLite query size limits.

The boot-time impact is acceptable because the migration runs **once** per gateway lifetime (the schema-version check is idempotent on subsequent boots).

Post-migration, every CRUD write triggers `load_all_applications_config()` which re-reads all four tables. For inventories of < 1000 devices this is < 50 ms; acceptable. For > 10000 devices it may exceed the perceived-latency budget — that's a future-performance story.

### Test budget delta — partial subtraction from removal of Story 9-5 tests

The +14 in AC#17 is GROSS; net = +14 minus the removed Story 9-5 byte-preservation tests + Story 9-7 file-watcher tests. Estimate: ~5-8 tests are removed. Net delta: ~+6 to +9. Adjust the target floor in AC#19 accordingly at story-completion time.

### Why no Cargo.toml strict-zero (the toml_edit removal exception)

C-6 removes Story 9-5's `toml_edit` dependency from Cargo.toml. This is the ONLY allowed Cargo.toml mutation in C-6 — verified by grep-check that no other source file uses `toml_edit` post-Task 6.

### Carry-forward GitHub issues

#88 (rate limiting), #100 (doctest baseline), #102 (tests/common), #104 (TLS), #110 (RunHandles Drop), #117 (perf-CI lane).

NEW issue to consider opening as part of C-6: "Future: restore-from-TOML-backup admin endpoint" as a low-priority follow-up CR captured in `deferred-work.md`.

---

## Out of Scope

- **Singleton config in SQLite** — `[chirpstack]`, `[opcua]`, `[web]`, `[global]` stay in TOML for v2.x.
- **Restore-from-TOML-backup admin endpoint** — see Dev Notes.
- **Backward compat layer that lets the binary run in either TOML or SQLite mode based on a feature flag** — out of scope. C-6 is a one-way migration.
- **Performance optimisation for > 10000-device deployments** — measure first; if a problem, future story.
- **Multi-tenant config schemas** — v2.x is single-tenant.
- **Configuration version history / undo** — out of scope. SQLite is the canonical store; backup is the audit trail.
- **Migration UI** — the migration is automatic at boot. No operator-facing migration wizard.

---

## Completion Note

To be filled in by the dev agent at story completion. Should include: actual test count delta (gross +N minus removed M), the final disposition of Story 9-7's hot-reload module (deleted vs repurposed), confirmation that `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list` still passes, smoke-test results against Guy's real ChirpStack with TOML-mtime verification, the GitHub issue numbers (including the recommended Story 9-5 / 9-7 deprecation retrospective issue), any deferred follow-ups added to `deferred-work.md` (especially the restore-from-TOML-backup CR).

---

### Review Findings — Iter-1 (2026-05-25)

Sources: Blind Hunter (BH), Edge Case Hunter (ECH), Acceptance Auditor (AA). 1 dismissed, 6 deferred, 1 decision-needed, 9 patch.

#### Decision-needed

- [ ] [Review][Decision] **F5 — Dead SIGHUP code: delete vs annotate** — `src/config_reload.rs` carries 12 `#[allow(dead_code)]` annotations on SIGHUP-related types and functions (ReloadOutcome, ReloadError, reload(), reload_from_path(), classify_diff(), etc.) that are now unreachable since C-6 removed the SIGHUP listener. Options: (A) delete the dead items and shrink the module; (B) keep as annotated with allowlists in case a future story re-wires SIGHUP. CLAUDE.md "no backwards-compat shims" favours deletion. [Sources: BH-6, AA-6]

#### Patch

- [ ] [Review][Patch] **F1 — Watch channel not seeded from SQLite on boot** — `main.rs:609-640` creates `ConfigReloadHandle` from TOML before migration runs, then never calls `load_all_applications_config()` + `notify_crud_write()` after the migration completes. On every boot after migration (whether `Migrated` or `AlreadyMigrated`), all subsystems operate on stale TOML data until the next CRUD write. Fix: in both the `Migrated` and `AlreadyMigrated` arms, call `sqlite_backend.load_all_applications_config()` and pass the result to `reload_handle.notify_crud_write(apps).await`. [Sources: BH-1, AA-5]
- [ ] [Review][Patch] **F2 — Migration done-flag never written; idempotency guard uses row-count alone** — `migrate_applications_config()` never writes a persistent migration marker. Guard (`count_applications() > 0`) fails for deliberate empty-DB: if an operator deletes all applications via the web UI and restarts, `count_applications()` returns 0 and the migration re-triggers, overwriting operator state with the TOML seed. Fix: add a `meta(key TEXT PRIMARY KEY, value TEXT)` table to `migrations/v009_application_config_tables.sql`; in `migrate_applications_config()` INSERT `('c6_migration_done', datetime('now'))` inside the same EXCLUSIVE TRANSACTION; update the guard in `migrate_toml_to_sqlite()` to check `SELECT value FROM meta WHERE key='c6_migration_done'` before row-count. Resolves both F2 and the idempotency gap (formerly AA-5b). [Sources: BH-2, ECH-2, AA-1, AA-5]
- [ ] [Review][Patch] **F3 — `check-c6-migration.sh` queries nonexistent tables** — Line 45 queries `SELECT value FROM meta WHERE key='schemaVersion'`; line 59 queries `SELECT value FROM meta WHERE key='config_migrated_from_toml_at'`. No `meta` table exists in the opcgw schema (version tracked via `PRAGMA user_version`); the `schema_migrations` fallback (line 47) also doesn't exist. Script always falls through to "unknown" version → false-fail on version check. Fix: replace line 45 with `SCHEMA_VER=$(sqlite3 "$DB" "PRAGMA user_version;" 2>/dev/null || echo "0")`; replace line 59 with `MIG_TS=$(sqlite3 "$DB" "SELECT value FROM meta WHERE key='c6_migration_done';" 2>/dev/null || echo "")` (after F2 adds the meta table). [Sources: BH-3, ECH-1]
- [ ] [Review][Patch] **F4 — Row-count verification misses `metrics` and `commands`** — `migrate_applications_config()` verifies `applications` and `devices` counts post-INSERT but not `metrics` or `commands`. A partial write to those tables rolls back silently if the outer error path triggers, but the count mismatch for metrics/commands won't be detected if the error is swallowed elsewhere. Fix: add `SELECT COUNT(*) FROM metrics` and `SELECT COUNT(*) FROM commands` checks and compare to `met_count`/`cmd_count` in the verification block `src/storage/sqlite.rs:3394-3413`. [Sources: BH-5, ECH-6, AA-8]
- [ ] [Review][Patch] **F6 — Large-inventory test bypasses `validate()`** — `migration_large_inventory_completes_in_time` builds `apps` programmatically, calls `AppConfig::from_path()`, then assigns `cfg.application_list = apps` without calling `cfg.validate()`. A future `validate()` regression on this shape would pass silently. Fix: call `cfg.validate().expect("large-inventory cfg is valid")` after the assignment at `tests/sqlite_config_migration.rs:523`. [Sources: BH-8, ECH-7]
- [ ] [Review][Patch] **F7 — `stage="already_migrated"` undocumented in `logging.md`** — `migrate_config.rs:52` emits `event="config_migration" stage="already_migrated"` on every boot after the first migration. `docs/logging.md:199` documents only `stage="toml_to_sqlite"` and `stage="skipped_empty_source"`. Fix: add `stage="already_migrated"` (with description: fired when guard detects existing data, no migration ran) to the `config_migration` event row in `docs/logging.md`. [Sources: AA-3]
- [ ] [Review][Patch] **F9 — `notify_crud_write` silently discards send error** — `src/config_reload.rs:476`: `let _ = self.tx.send(Arc::new(candidate));` discards the `SendError` silently. `watch::Sender::send` fails only when all receivers have dropped — an abnormal shutdown state invisible in logs. Fix: replace with `if let Err(_) = self.tx.send(Arc::new(candidate)) { warn!(event = "config_reload_warn", reason = "no_subscribers", "Watch channel send failed — all subscribers dropped"); }`. [Sources: ECH-4]
- [ ] [Review][Patch] **F14 — No test for `AlreadyMigrated` boot path watch-channel seeding** — Once F1 is patched, the AlreadyMigrated code path also calls `notify_crud_write`. There is currently no test asserting that a second `migrate_toml_to_sqlite` call (simulating a restart) followed by a `notify_crud_write` leaves the watch channel correctly populated from SQLite. Fix: add `migration_already_migrated_seeds_watch_channel` integration test after F1 patch. [Sources: BH-13]

#### Deferred

- [x] [Review][Defer] **F10 — `command_port` not range-validated during migration** [`src/storage/sqlite.rs:3371-3383`] — deferred, pre-existing gap in command validation predating C-6; not a C-6-introduced regression.
- [x] [Review][Defer] **F11 — `commands` config table missing `created_at`/`updated_at` columns** [`migrations/v009_application_config_tables.sql`] — deferred, requires a v010 schema migration; out of C-6 scope.
- [x] [Review][Defer] **F12 — `config_reload.rs::reload()` unreachable and untested** [`src/config_reload.rs`] — deferred, moot if F5 resolves to deletion; else pre-existing coverage gap.
- [x] [Review][Defer] **F13 — FK pragma must be enabled per-connection** [`src/storage/sqlite.rs`] — deferred, pre-existing pattern across codebase; not a C-6-specific regression.
- [x] [Review][Defer] **F15 — `duration_ms` includes pool-checkout time** [`src/storage/migrate_config.rs:66`] — deferred, cosmetic accuracy issue; pool checkout is bounded and typically sub-millisecond.
- [x] [Review][Defer] **F17 — Unstructured `info!` startup log block in `main.rs`** [`src/main.rs`] — deferred, pre-existing style inconsistency; not a C-6 regression.

---

### Review Findings — Iter-2 (2026-05-26)

Sources: Blind Hunter (BH), Edge Case Hunter (ECH), Acceptance Auditor (AA — all 9 findings dismissed as hallucinations: AA was given a text summary, not file access, and claimed patches "not applied" that are provably present in the code). 15 dismissed total, 2 deferred, 0 decision-needed, 9 patch.

#### Patch

- [ ] [Review][Patch] **I2-F1 (HIGH) — `notify_crud_write` uses `send()` which silently fails at boot; F1 seeding intent broken** [`src/config_reload.rs:75`] — tokio 1.52.1's `watch::Sender::send()` returns `Err` WITHOUT updating the stored value when `receiver_count() == 0` (confirmed from tokio source: `if 0 == self.receiver_count() { return Err(…); }`). The F1 seeding call in `main.rs:651` runs before any `subscribe()` call (first subscriber at line 767). Result: `send()` returns `Err`, the WARN `reason="no_subscribers"` fires on every clean post-migration boot, and the TOML-initialised snapshot stays in the channel — all subsystems receive stale TOML data. Fix: change `self.tx.send(Arc::new(candidate)).is_err()` → `self.tx.send_replace(Arc::new(candidate))` in `notify_crud_write`; `send_replace` always stores the value regardless of receiver count. Remove the `is_err()` branch; always log the `info!` event. [Sources: BH-1, ECH-8]
- [ ] [Review][Patch] **I2-F2 (MEDIUM) — `trace!` instead of `warn!` for boot-critical pool-checkout failure in `is_c6_migration_done`** [`src/storage/sqlite.rs:3114`] — All other `pool.checkout()` failures in startup code paths use `warn!` or `error!`. A pool-exhaustion timeout in `is_c6_migration_done` triggers the `migrate_toml_to_sqlite` error path + TOML fallback with no observable signal at default log levels. Fix: change `trace!` → `warn!`. [Sources: BH-3]
- [ ] [Review][Patch] **I2-F3 (MEDIUM) — `count_applications().unwrap_or(0)` silently masks DB error in already-migrated audit log** [`src/storage/migrate_config.rs:53`] — When `is_c6_migration_done()` returns `Ok(true)`, the code calls `backend.count_applications().unwrap_or(0)` for the `applications=N` audit log field. Pool exhaustion is swallowed; log emits `applications=0` even when the DB has data. Fix: propagate the error (`backend.count_applications()?`) or log at `warn!` before using `0` as fallback. [Sources: BH-4, ECH-6]
- [ ] [Review][Patch] **I2-F4 (MEDIUM) — No test for secondary already-migrated guard (apps present, no done-flag)** [`tests/sqlite_config_migration.rs`] — The secondary guard at `migrate_config.rs:63-71` handles databases with applications but no meta done-flag (pre-iter-1 migrations; direct SQLite imports). No test seeds this state and asserts `AlreadyMigrated`. Without it, a regression removing the secondary guard would silently re-run migration on these DBs. Fix: add test that seeds SQLite with apps but no meta key, calls `migrate_toml_to_sqlite`, asserts `AlreadyMigrated`. [Sources: BH-5]
- [ ] [Review][Patch] **I2-F5 (MEDIUM) — Double-emit of `config_migration_failed` with inconsistent severity and missing `reason` field** [`src/storage/migrate_config.rs:108`, `src/main.rs:633`] — Inner emits `error!(event="config_migration_failed", reason=reason, error=%e)`. Outer emits `warn!(event="config_migration_failed", error=%e)` — without `reason`, at different severity. `docs/logging.md` documents this event as `warn`. Structured log consumers see two records per failure; one missing the `reason` field the docs say is always present. Fix: remove the inner `error!` emit from `migrate_config.rs` (let `main.rs` be the single emitter); add `reason` field to the `main.rs` `warn!` by inspecting the error string. [Sources: BH-6]
- [ ] [Review][Patch] **I2-F6 (MEDIUM) — Secondary guard returns `AlreadyMigrated` without writing meta done-flag** [`src/storage/migrate_config.rs:63-71`] — Databases migrated with pre-iter-1 C-6 code have no done-flag. On every subsequent boot: `is_c6_migration_done() → Ok(false)`, then `count_applications() → >0` → `AlreadyMigrated` without writing the key. The done-flag designed as the PRIMARY guard never becomes active for these DBs. `check-c6-migration.sh` permanently reports "no done-flag" for them. Fix: after the secondary guard fires, write the meta key via a new `SqliteBackend::write_c6_migration_done()` helper (simple `INSERT OR REPLACE INTO meta VALUES ('c6_migration_done', ?1)` outside of EXCLUSIVE TRANSACTION — the data is already there). [Sources: ECH-7]
- [ ] [Review][Patch] **I2-F7 (MEDIUM) — F14 test is a fake regression guard; doesn't verify primary meta guard or watch-channel seeding** [`tests/sqlite_config_migration.rs:637-669`] — Test docstring: "validates F1 boot-time watch-channel seeding." Actual body: calls `migrate_toml_to_sqlite` twice, then asserts `load_all_applications_config()` returns correct data — a path already covered by `migration_load_all_applications_reflects_data`. Never calls `is_c6_migration_done()`, never constructs a `ConfigReloadHandle`, never calls `notify_crud_write`, never asserts a watch receiver. Fix: (a) rename/redirect the test to accurately document what it covers (`migration_already_migrated_does_not_overwrite_sqlite_data`); (b) add a separate test `migration_meta_done_flag_is_written` that asserts `is_c6_migration_done() == Ok(true)` after a successful migration. [Sources: BH-10, ECH-10]
- [ ] [Review][Patch] **I2-F8 (LOW) — Dead `_config_path: PathBuf` parameter in `make_test_reload_handle`** [`tests/common/mod.rs:168`] — Parameter is ignored (prefixed `_`). The function has zero callers in the codebase (all callers use `make_test_reload_handle_and_writer` from `src/web/test_support.rs`). Fix: remove the `_config_path` parameter from the signature. [Sources: BH-2]
- [ ] [Review][Patch] **I2-F9 (LOW) — `check-c6-migration.sh` exits 0 without `pass` line for APP_COUNT=0 + done-flag branch** [`scripts/check-c6-migration.sh:91-93`] — When done-flag set but table empty (normal post-operator-deletion state), script emits `info` and `exit 0` — never reaching the final `pass "C-6 migration check complete"` at line 107. Automated runners expecting a PASS line see none. Fix: emit `pass "Migration done-flag set; applications table empty — operator deletion confirmed"` before `exit 0`. [Sources: BH-7]

#### Deferred

- [x] [Review][Defer] **I2-F10 (LOW) — Identical `stage="already_migrated"` for two distinct paths** [`src/storage/migrate_config.rs:56,65`] — Primary (done-flag) and secondary (apps > 0, no flag) paths emit identical `stage="already_migrated"` events. Operators cannot distinguish them. Deferred: F6 fix will eventually eliminate the secondary path for all DBs; distinguishing stages adds noise before that transition is complete.
- [x] [Review][Defer] **I2-F11 (LOW) — Two round-trips per AlreadyMigrated boot in primary guard** [`src/storage/sqlite.rs:3112`, `src/storage/migrate_config.rs:53`] — `is_c6_migration_done()` (1 query) + `count_applications()` (1 query) for audit log. Could be one query returning `Option<String>` (timestamp). Deferred: minor optimisation; pool checkout is sub-millisecond on warm boot.

---

### Review Findings — Iter-3 (2026-05-26)

Sources: Blind Hunter (BH), Edge Case Hunter (ECH), Acceptance Auditor (AA — **0 findings**, all 9 iter-2 patches verified spec-compliant). Scope: iter-2 commit `1cf2304` (290 lines, 7 files). 9 raw findings → 2 patch, 3 defer, 4 dismiss.

#### Patch

- [x] [Review][Patch] **I3-F1 (LOW) — `migration_meta_done_flag_is_written` docstring incorrectly attributes coverage to iter-2 `write_c6_migration_done` helper** [`tests/sqlite_config_migration.rs:668-676`] — Test asserts `is_c6_migration_done()` is false then true after `migrate_toml_to_sqlite` on a fresh DB. The flag is actually written by iter-1's `migrate_applications_config` inside the EXCLUSIVE TRANSACTION (`sqlite.rs:3463-3467`), NOT by the new iter-2 `write_c6_migration_done()` helper. The iter-2 helper is only called from the secondary already-migrated guard, which is exercised by `migration_secondary_guard_backfills_meta_key`. Risk: a future refactor that deletes `write_c6_migration_done()` would not be caught by this test (the suite stays green). Fix: clarify the docstring — "tests that `migrate_applications_config` (iter-1) writes the flag inside the EXCLUSIVE TRANSACTION; for the secondary-guard back-fill path see `migration_secondary_guard_backfills_meta_key`." [Sources: BH-4, ECH-F02]

- [x] [Review][Patch] **I3-F2 (MEDIUM) — Secondary guard back-fill failure mis-emits `config_migration_failed reason=insert_failed` despite intact data** [`src/storage/migrate_config.rs:66`, `src/main.rs:631-644`] — When `write_c6_migration_done()?` fails (pool exhaustion during boot; full-pool window is real with 5-connection limit), the `?` propagates the error → `migrate_toml_to_sqlite` returns `Err(e)` → `main.rs` emits `event="config_migration_failed", reason="insert_failed"` at `warn!`. But the apps table is intact (the secondary guard already confirmed `count_applications() > 0`); no insert was attempted. Result: misleading audit event that monitoring rules may false-alert on. `load_all_applications_config()` still runs after the warn so subsystems get the correct data; not boot-blocking. Fix: change `write_c6_migration_done()?` to best-effort — `if let Err(e) = backend.write_c6_migration_done() { warn!(event="config_migration", stage="already_migrated_backfill_failed", error=%e, "Meta key back-fill failed; data intact, next boot will retry"); }` — then continue to `AlreadyMigrated`. [Sources: BH-5, ECH-F01]

#### Deferred

- [x] [Review][Defer] **I3-F3 (MED-equivalent) — Substring `contains("row_count_mismatch")` classifier mechanism is fragile (anti-pattern)** [`src/main.rs:632`] — Pattern flagged repeatedly in iter-3 doctrine memory. ECH Investigation-3 verified current code is safe: only `OpcGwError::Database(String)` propagates from `migrate_applications_config`; only `sqlite.rs:3451` produces "row_count_mismatch" inside the message. NO current path leaks. However, the classifier is still substring-matching error Display strings, which is the documented "substring-matcher attribution leak" anti-pattern. Future code that wraps the error or adds a new variant whose Display happens to include the substring would silently misclassify. Fix would require adding a typed `OpcGwError::RowCountMismatch` variant and matching on it — a wider design change. **Requires user acceptance to defer** (MED-equivalent — see Required user input section).

- [x] [Review][Defer] **I3-F4 (LOW) — `send_replace` removes the `warn!(reason="no_subscribers")` emission, losing observability if all subscribers drop in steady state** [`src/config_reload.rs:75-83`] — Pre-iter-2 code emitted `warn!` when `receiver_count() == 0`; iter-2's `send_replace` always succeeds silently. In practice, the poller, OPC UA server, and web routes all subscribe at boot; steady-state `receiver_count() >= 3`. The warn would only fire if all 3 subsystems had crashed/dropped — a degenerate case where audit logging is the least of the operator's worries. Deferred: re-introducing the warn would add a per-CRUD-write check that fires under no realistic operational scenario.

- [x] [Review][Defer] **I3-F5 (LOW) — `migration_already_migrated_does_not_overwrite_sqlite_data` test scope narrower than the rename implies** [`tests/sqlite_config_migration.rs:639-666`] — Both calls to `migrate_toml_to_sqlite` use the identical `TOML_TWO_APPS` fixture. The test verifies the AlreadyMigrated path is taken but does not test the "stale TOML does not overwrite operator-modified SQLite" scenario directly (which would require a mutated TOML on the second call). Deferred: the AlreadyMigrated guard provably routes around `migrate_applications_config` regardless of TOML content (verified by code reading); a mutated-TOML test would re-exercise the same `AlreadyMigrated` branch with no additional path coverage.

#### Dismissed

- [Dismiss] BH-F01 (HIGH): `INSERT OR IGNORE` semantics — ECH Investigation-2 verified the meta table has `PRIMARY KEY` on `key`; `INSERT OR IGNORE` is the correct back-fill semantic (ensure flag exists, do not refresh timestamp). The "corrupted stale row" scenario has no path that produces it.
- [Dismiss] BH-F06 (LOW): Test rename breaks `git bisect` by old name — cosmetic; `git log --follow` and content-based bisection both work.
- [Dismiss] BH-F07 (LOW): `pass` function may not be defined in `check-c6-migration.sh` — ECH Investigation-8 verified `pass()` IS defined in the script.
- [Dismiss] BH-F08 (LOW): `warn!` import not visible in diff — BH self-flagged as unverifiable from diff alone; `cargo test` + `cargo clippy` pass, confirming the import is present.

# Story D-0: Singleton Configuration → SQLite Migration

Status: review

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `D-0-singleton-config-sqlite-migration`                                                                     |
| Epic            | D — Singleton Configuration → SQLite                                                                        |
| FRs             | none (Epic D is post-PRD)                                                                                   |
| Status          | ready-for-dev                                                                                               |
| Created         | 2026-05-26                                                                                                  |
| Source epic     | `_bmad-output/planning-artifacts/epics.md § Epic D § Story D.0`                                             |
| Depends on      | C-0 (`config/secrets.toml` infrastructure must exist so D-0 can leave secrets behind) and C-6 (the schema-migration / EXCLUSIVE-TRANSACTION / meta-done-flag / secondary-back-fill-guard / TOML-fall-back-safety-net pattern that D-0 mirrors). C-6 must be on `main` before D-0 implementation starts. |
| Tracking        | GitHub issue `#__` — user opens out-of-band                                                                 |

---

## User Story

As an **opcgw operator running v2.x+ with an established TOML-based configuration**,
I want the gateway's `[global]`, `[chirpstack]`, `[opcua]`, and `[web]` non-secret singleton config to migrate from `config/config.toml` into SQLite on first boot of a post-D-0 binary,
So that all non-secret configuration writes converge on the SQLite store that already holds the `[[application]]` tree (post-C-6), with `config.toml` reduced to a bootstrap-only seed file and `config/secrets.toml` continuing to carry only operator-supplied secrets.

---

## Story Context

### Why D-0 is the opening story of Epic D

Epic D was opened 2026-05-26 immediately after the Epic C retrospective (option-1 selection: "natural C-6 follow-up — singleton config → SQLite + UI for editing it"). C-6 (2026-05-26) moved the `[[application]]` collection tree (applications/devices/metrics/commands) from TOML to SQLite. The four remaining singleton sections — `[global]`, `[chirpstack]`, `[opcua]`, `[web]` — stayed in TOML "for v2.x" per C-6's explicit deferral. D-0 lands that v2.x follow-up. D-1 (UI) and D-2 (TOML decommission) depend on D-0; this story is the prerequisite for both.

### Three persistence surfaces — the end-state shape

Post-D-2 (epic close), opcgw will have exactly **three** persistence surfaces:

1. **SQLite** (`data/opcgw.db`, chmod 0o600 per **AI-C-SEC-2** which D-0 lands inline) — authoritative for all config + metric values
2. **`config/secrets.toml`** (chmod 0o600 via atomic-rename per Story C-0) — operator-supplied secrets (`[chirpstack].api_token`, `[opcua].user_password`)
3. **`config.toml`** — bootstrap seed only, read once at the first boot of a post-D-0 binary, never mutated at runtime

D-0 specifically lands surface (1)'s expansion (now load-bearing for singleton config, hence the file-permission hardening) and consumes surface (3) one final time (bootstrap migration). Surface (2) is unchanged by D-0.

### What stays in `config.toml` (out of D-0 scope)

- **Secrets** — `[chirpstack].api_token`, `[opcua].user_password`. These are operator-supplied secrets and continue to live in `config/secrets.toml` (chmod 0o600) or env-var overrides (`OPCGW_CHIRPSTACK__API_TOKEN`, `OPCGW_OPCUA__USER_PASSWORD`). D-0 does NOT introduce SQLite-encryption-at-rest; the operator threat model has not shifted to justify a key-management surface.
- **The `[[application]]` tree** — already in SQLite as of C-6 (schema v009). D-0 does not touch it.
- **`[command_delivery]`** — single sub-section of `[chirpstack]` that ships as an inner field of `ChirpstackPollerConfig`. Migrates **with** `[chirpstack]` (same struct → same SQLite row).
- **`[logging]`** — commented-out optional override; figment + the dedicated `OPCGW_LOG_DIR` / `OPCGW_LOG_LEVEL` env vars carry today's behaviour. D-0 keeps the existing log-config wiring untouched; logging stays an env-var-first knob in this story (would-be future scope: also migrate to SQLite).

### What "post-D-0 hot-reload" looks like for singletons

Most singleton knobs are **restart-required**. The few that are runtime-mutable (e.g. `[chirpstack].polling_frequency`, `[chirpstack].api_token` rotation via secrets.toml) continue to use the existing `notify_crud_write` rebuild path Story C-6 wired up — they just read from SQLite now instead of from TOML.

The restart-required-vs-hot-reloadable taxonomy locks in the v2.x story status:

| Knob | Status post-D-0 |
|---|---|
| `[opcua].host_ip_address` / `[opcua].host_port` | **restart-required** (OPC UA server bind) |
| `[opcua]` PKI paths (`pki_dir`, `certificate_path`, `private_key_path`) | **restart-required** (OPC UA TLS context) |
| `[web].port` / `[web].bind_address` / `[web].enabled` | **restart-required** (HTTP server bind) |
| `[web].allowed_origins` | **restart-required** (issue #113 live-borrow refactor territory) |
| `[chirpstack].server_address` | **restart-required** (poller gRPC channel) |
| `[chirpstack].polling_frequency` | hot-reloadable (existing poller pattern) |
| `[chirpstack].retry` / `[chirpstack].delay` / `[chirpstack].list_page_size` | hot-reloadable |
| `[chirpstack].inventory_cache_ttl_seconds` (C-1) | hot-reloadable |
| `[global].prune_interval_minutes` | hot-reloadable |
| `[global].command_delivery_poll_interval_secs` | hot-reloadable |
| `[opcua].stale_threshold_seconds` | hot-reloadable |
| `[chirpstack].api_token` (rotated via secrets.toml, not via D-0 surface) | restart-required (Story C-0 wizard pattern) |

D-0 does NOT change the restart-required-vs-hot-reloadable status of any knob. D-1's editor UI surfaces the distinction to operators (confirmation modal + supervisor-restart trigger for restart-required PUTs).

### Migration timing and one-way contract

The migration runs **once**, on the first boot of a gateway that has the D-0 binary AND a populated SQLite `singleton_config` schema absent OR a `d0_migration_done` meta-flag absent. Detection mirrors C-6: read the schema version + meta done-flag; if the singleton tables are empty AND the TOML singleton sections deserialise non-trivially, perform the migration.

The migration is **one-way**, symmetric with C-6: post-D-0, opcgw writes to SQLite as the authoritative store for singletons. Downgrading to a pre-D-0 binary means singleton-knob changes made via the (later-shipped) D-1 UI won't be in the TOML file the older binary reads. Rollback requires restoring the pre-migration `opcgw.db.pre-d0-backup` the runbook (AC#13) instructs the operator to take.

---

## Acceptance Criteria

### SQLite schema migration v010

1. **New migration file `migrations/v010_singleton_config_tables.sql`** introduces the singleton-config schema. **Design call deferred to Dev Notes (see § "Schema design call — generic K/V vs per-section typed" in Dev Notes below).** The Dev Agent picks ONE of the two shapes and documents the call in Completion Note. Whichever shape lands, the migration MUST: (a) be additive (no DROP / no destructive ALTER on the v009 tables); (b) include a `migrations/v010_singleton_config_tables.sql` file that is byte-stable across CI runs (no dynamic timestamps or per-run UUIDs inside the SQL); (c) add the `d0_migration_done` meta key only after the table-creation DDL succeeds.

2. **Schema version increment.** `src/storage/schema.rs::run_migrations` adds a v010 branch following the v009 branch's structural pattern. `PRAGMA user_version` becomes `10` after the migration runs. The v010 migration is atomic per the existing per-version `Connection::execute_batch` pattern.

3. **Schema-migration tests** (in `src/storage/schema.rs::tests` or `tests/sqlite_config_migration.rs` per the existing convention):
   - Fresh DB → migrations run in order through v010, `user_version == 10`.
   - v009 DB → v010 applied; v009 tables (`applications`, `devices`, `metrics`, `commands`, `meta`) untouched; no data loss.
   - v010 DB → second `run_migrations` call is a no-op.

### One-shot data migration (TOML → SQLite singletons)

4. **New module `src/storage/migrate_singleton_config.rs`** owns the migration logic, mirroring C-6's `src/storage/migrate_config.rs` shape:
   - Public entrypoint `migrate_singleton_toml_to_sqlite(app_config: &AppConfig, backend: &SqliteBackend) -> Result<SingletonMigrationOutcome, OpcGwError>`.
   - Outcomes: `Migrated(SingletonMigrationReport { sections: usize, duration_ms: u64 })`, `AlreadyMigrated`, `SkippedEmptyOrPlaceholder` (if the TOML carries placeholder values such as `REPLACE_ME_WITH_OPCGW_…`, the migration skips to avoid persisting placeholders into SQLite).
   - **Primary guard:** `is_d0_migration_done()` reads the `d0_migration_done` meta key (analogous to C-6's `is_c6_migration_done()`); if `Ok(true)`, emit `event="config_migration" stage="singleton_already_migrated"` and return `AlreadyMigrated`.
   - **Secondary guard:** singleton tables are non-empty but `d0_migration_done` is absent (direct SQLite import that bypassed `migrate_singleton_toml_to_sqlite`). Back-fill the meta key best-effort (mirrors C-6 iter-3 I3-F2 lesson — `if let Err(e) { warn!(event="config_migration", stage="singleton_already_migrated_backfill_failed", error=%e, "...") }` rather than `?`-propagate); emit `event="config_migration" stage="singleton_already_migrated"` and return `AlreadyMigrated`.
   - **Placeholder check:** before the EXCLUSIVE TRANSACTION begins, inspect `app_config.chirpstack.api_token` and `app_config.opcua.user_password`. If either holds a string matching `REPLACE_ME_WITH_OPCGW_…`, log `event="config_migration" stage="skipped_placeholder_singleton" missing_secret=<field>` at `info` and return `SkippedEmptyOrPlaceholder`. (Secrets being placeholder means the gateway is in pre-secrets-supplied state; migrating singletons to SQLite is safe to defer to next boot once secrets are supplied. This mirrors the C-0 first-run gating philosophy: don't lock state into SQLite until the operator has completed the secrets-supply step.)
   - Migration runs inside `BEGIN EXCLUSIVE TRANSACTION`. Per-section row counts are written + verified before COMMIT (mirror C-6 AC#5). On row-count mismatch, ROLLBACK + return `Err(OpcGwError::Database("singleton_row_count_mismatch: expected=N actual=M section=X".into()))`.
   - Success emits `event="config_migration" stage="singleton_toml_to_sqlite" sections=<N> duration_ms=<u64>` at `info`.

5. **`main.rs` integration.** Call `migrate_singleton_toml_to_sqlite` immediately AFTER the existing C-6 `migrate_toml_to_sqlite` call (C-6's call site is the established post-schema-migration / pre-poller-start position; D-0's call lands directly below it). The error handler that C-6's I2-F5 patch installed (`warn!` at `warn!` level with `reason="row_count_mismatch" | "insert_failed"` classifier) is **extended** to also cover D-0 failures: the error-string `contains("singleton_row_count_mismatch")` adds the new reason `"singleton_row_count_mismatch"` to the classifier. The Dev Agent MAY refactor to a typed `OpcGwError::RowCountMismatch { kind: ConfigKind }` variant if doing so is cheap — that closes the AI-C-1 substring-classifier anti-pattern (Epic C retro carry-forward) and removes the classifier string match entirely. The typed-variant refactor is RECOMMENDED but not REQUIRED; if deferred, document in Completion Note with explicit link to AI-C-1.

6. **Fall-back on failure.** On `Err` from `migrate_singleton_toml_to_sqlite`, the gateway falls back to TOML-driven singleton config for the current start-up only (transition safety net, symmetric with C-6 AC#5). The audit event documents the operator-actionable next step. The migration is retried idempotently on next boot (transient failure self-clears; persistent failure warns on every boot but does not block service).

### Post-migration runtime: SQLite as authoritative singleton-config store

7. **In-memory `AppConfig` snapshot is rebuilt from SQLite at boot post-D-0.** After `migrate_singleton_toml_to_sqlite` returns success (or `AlreadyMigrated`), the gateway reads the four singleton sections from SQLite into a `Global`, `ChirpstackPollerConfig`, `OpcUaConfig`, `WebConfig` set of structs and seeds the in-memory `AppConfig` from them. The figment provider stack continues to load `config.toml` on every boot for backward compatibility (operators with existing `config.toml` files don't need to migrate by hand), BUT once `is_d0_migration_done()` returns `Ok(true)`, the SQLite singleton values **override** the figment-derived values for the four covered sections. The TOML→SQLite migration runs once; the SQLite-as-authoritative-source semantic kicks in on every subsequent boot.

8. **Env-var overrides continue to work** (operational invariant). `OPCGW_CHIRPSTACK__POLLING_FREQUENCY`, `OPCGW_OPCUA__HOST_PORT`, etc., still apply on top of the SQLite snapshot via the existing figment env-var-provider layering. Order of precedence (highest to lowest, post-D-0):
   1. Env var (e.g. `OPCGW_CHIRPSTACK__POLLING_FREQUENCY`)
   2. SQLite singleton-config snapshot
   3. `config.toml` (fall-back only when SQLite singleton tables are empty or unmigrated)
   4. struct default (via `#[serde(default)]`)

   This is symmetric with C-6's `[[application]]` precedence: SQLite is authoritative when populated; TOML is fall-back; env-var wins over both for restart-required knobs that operators want to override at deploy-time.

9. **Secrets remain in `config/secrets.toml`** (out-of-scope invariant). `config/secrets.toml` continues to carry `[opcua].user_password` and `[chirpstack].api_token` exclusively. The figment provider stack post-D-0 is: `config.toml` (bootstrap seed) → `secrets.toml` (chmod 0o600 secrets) → SQLite singleton snapshot (authoritative) → env-var overrides. The secrets.toml layer is unchanged by D-0.

### Hot-reload for hot-reloadable singleton knobs (Story C-6 path reuse)

10. **Existing `notify_crud_write` path is reused for singleton-config writes** (in D-1, not in D-0). D-0 does NOT introduce new write surfaces for singletons — those land in D-1. What D-0 lands here: a new helper `SqliteBackend::write_singleton_section(section: SingletonSection, fields: &SingletonFieldSet) -> Result<(), OpcGwError>` that D-1's PUT handler will call, with the contract that after a successful write the existing `notify_crud_write(reload_handle)` is called (mirrors C-6 web-CRUD pattern). The helper is delivered but not exercised by D-0; D-0's tests assert the helper compiles, accepts the expected shape, and writes correct rows. D-1 wires it to a real HTTP endpoint.

11. **Restart-required knobs are read once at boot** (no live-reload primitive added in D-0). The existing in-memory `Arc<AppConfig>` snapshot continues to be the in-memory view that subsystems read; restart-required knobs read into the subsystem's per-task local at boot and stay there until process restart. D-0 does not change this; D-1's UI surfaces the restart-required confirmation modal.

### SQLite file permissions hardening (AI-C-SEC-2 from Epic C security review)

12. **`data/opcgw.db` file permissions tightened to 0o600 on first creation.** Epic C's security review (`AI-C-SEC-2`) flagged that `Connection::open(path)` inherits the process umask and on a typical deployment (umask 0o022) the file is world-readable 0o644. C-6 made the database load-bearing for `[[application]]` config; D-0 makes it load-bearing for all singleton config (including the `[web].allowed_origins` list which is operator-deployment-sensitive). D-0 closes this gap. Implementation:
    - In `src/storage/pool.rs`, after `Connection::open(path)` succeeds, IF the file was newly created (detect via the file's pre-open existence check) call `std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))` on Unix-family targets. Wrap in `#[cfg(unix)]` since Windows uses ACLs (skip on non-Unix; document the gap in `docs/security.md`).
    - Existing databases (pre-D-0 deployments) are NOT chmod'd retroactively to avoid surprising operators who have an existing umask + supervisor permission model. Instead, emit `event="storage_init" warn` on existing-database boot if the file mode is wider than 0o600 (and ONLY at `warn` once-per-boot — not on every connection-pool checkout). The runbook (AC#13) documents the operator-action recipe.
    - Add a `chmod` recipe to `docs/security.md` and to the D-0 runbook for operators to apply post-upgrade.

### Migration runbook + verification script

13. **New `docs/d-0-migration-runbook.md`** mirroring `docs/c-6-migration-runbook.md`. Sections (minimum): pre-migration backup recipe (`opcgw.db` → `opcgw.db.pre-d0-backup`); automatic migration on first boot post-D-0; verification step pointing at `scripts/check-d0-migration.sh`; one-way rollback contract (restore the pre-d0 backup); SQLite-file-permissions recipe per AC#12; troubleshooting recipe for `singleton_row_count_mismatch` events.

14. **New `scripts/check-d0-migration.sh`** mirroring `scripts/check-c6-migration.sh`:
    - Takes `data/opcgw.db` as its first positional arg (default to `data/opcgw.db`).
    - Reads `PRAGMA user_version` → expected `10` post-D-0.
    - Reads the `d0_migration_done` meta key → expected non-empty ISO-8601 timestamp.
    - Counts rows per singleton table/section → expected `> 0` for the four sections.
    - Verifies SQLite file mode `0o600` per AC#12 → warn if wider; non-fatal.
    - Emits `pass` lines on every successful check + a final `pass "D-0 migration check complete"` (mirrors C-6's iter-2 I2-F9 fix — every exit path emits a pass-line so automated runners detect success).

### Sprint-status + spec invariants

15. **`sprint-status.yaml`** already has `D-0-singleton-config-sqlite-migration: ready-for-dev` post-Epic-D-scoping (commit `0787859`). After implementation lands, the Dev Agent flips to `review` via the same workflow precedent.

16. **No new direct dependencies in `Cargo.toml`.** D-0 reuses existing rusqlite + tokio + figment + serde infrastructure. The implementation MUST verify `Cargo.toml` is unchanged by `git diff Cargo.toml Cargo.lock` after the story is complete (`Cargo.lock` may regenerate if the lockfile auto-resolves, but no new `[dependencies]` lines may be added by D-0).

### Integration tests

17. **New integration tests in `tests/sqlite_singleton_config_migration.rs`** (≥ 14 tests, mirroring C-6's 16-test structure). Required test coverage:

    1. Fresh DB + populated TOML (all 4 sections) → migration runs, all 4 SQLite singleton rows populate, `is_d0_migration_done() == Ok(true)`, `PRAGMA user_version == 10`.
    2. Already-migrated DB (`d0_migration_done` set) → second `migrate_singleton_toml_to_sqlite` call is no-op via primary guard; emits `stage="singleton_already_migrated"`.
    3. Direct SQLite import (singletons present but no `d0_migration_done`) → secondary guard fires, back-fills the meta key, returns `AlreadyMigrated`; second boot uses primary guard.
    4. Secondary-guard back-fill failure (pool exhaustion simulated) → warn at `stage="singleton_already_migrated_backfill_failed"`, returns `AlreadyMigrated` (best-effort wiring per C-6 I3-F2 lesson).
    5. Row-count mismatch (synthetic — modify migrate to insert N-1 rows then verify N expected) → ROLLBACK, return `Err`, gateway falls back to TOML for current boot per AC#6.
    6. Placeholder secrets present (TOML has `REPLACE_ME_WITH_OPCGW_…`) → `SkippedEmptyOrPlaceholder` returned; no SQLite singleton rows written.
    7. Post-migration boot: `AppConfig::load_from_sqlite` returns the migrated singleton values; figment TOML values are NOT used (override path active per AC#7).
    8. Env-var override applies on top of SQLite singleton: set `OPCGW_CHIRPSTACK__POLLING_FREQUENCY=5`, populate SQLite with `polling_frequency=10`, assert effective value is `5`.
    9. Secrets still come from secrets.toml (not from SQLite): populate secrets.toml with a real api_token + a placeholder in `config.toml`; assert `AppConfig.chirpstack.api_token` reflects the secrets.toml value.
    10. v009 → v010 upgrade path under 5s (test marker: `#[ignore = "perf-sensitive; run via cargo make tests"]` if perf timing is flaky, else inline timing assertion).
    11. SQLite file mode is 0o600 on fresh-creation boot (Unix-only via `#[cfg(unix)]`).
    12. Existing database with wider permissions boots cleanly + emits the once-per-boot `storage_init warn` event (Unix-only).
    13. `SqliteBackend::write_singleton_section` writes correct rows and increments the `updated_at` timestamp (D-0 delivers the helper; D-1 will exercise it via HTTP — D-0's test asserts the helper's row-shape contract).
    14. New stage values (`singleton_toml_to_sqlite`, `singleton_already_migrated`, `singleton_already_migrated_backfill_failed`) match `docs/logging.md` exactly (grep-style invariant test — read the doc, scan for the stage strings, assert all three present; mirrors C-6 iter-4 I4-F2 lesson — AC#24 doc-sync gate).

### Regression invariants

18. **`tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list`** still passes — the post-2026-05-20-incident regression guard remains green.

19. **Existing C-6 tests** (`tests/sqlite_config_migration.rs`, all 19 tests) still pass — D-0 does not regress the `[[application]]` migration path.

20. **`cargo test --all-targets`** total ≥ 1500 / 0 / ≥ 73 (Epic C closed at 1482; D-0 adds ≥ 14 integration tests + helper unit tests).

21. **`cargo clippy --all-targets -- -D warnings`** clean.

22. **`cargo test --doc`** 0 failed / 73 ignored (no regression vs C-6 baseline).

### Documentation sync

23. **`docs/logging.md`** updated **in the same commit as the code** (the AC#24 doc-sync gate from Epic C retro carry-forward). The `config_migration` event row gains three new stage values:
    - `stage="singleton_toml_to_sqlite"` (info) — sections=N, duration_ms=<u64>
    - `stage="singleton_already_migrated"` (info) — sections=N
    - `stage="singleton_already_migrated_backfill_failed"` (warn) — error=<str>; non-fatal; retry on next boot per backend health
    - Plus `stage="skipped_placeholder_singleton"` (info) — missing_secret=<str>; secrets.toml not yet populated

24. **`docs/security.md`** gains a paragraph in the file-permissions section documenting AC#12 (SQLite file mode 0o600 on fresh creation; chmod recipe for existing deployments).

25. **`docs/architecture.md`** updated to reflect the in-progress two-surface model: post-D-0, SQLite is authoritative for `[[application]]` (from C-6) AND singleton config (from D-0). The `config.toml`-as-bootstrap-only contract isn't fully realised until D-2; D-0's architecture.md update reflects "in transition" status and forward-references D-2.

26. **DocBook user manual** (`docs/manual/opcgw-user-manual.xml`) gains a new sub-section under the Configuration chapter describing the singleton-config migration on first post-D-0 boot. Light update (not a chapter rewrite — D-2 carries the full Configuration chapter rewrite); D-0 adds a `<section id="sec-singleton-config-migration">` block that describes the migration event, points at the runbook + check script, and notes the AI-C-SEC-2 chmod recipe. `xmllint --noout --valid` clean.

27. **`README.md`** Planning table row for Epic D updated to reflect D-0 status flip (in-progress → review at impl-complete, then review → done post-code-review). Current-version block date updated.

### GitHub tracking issue

28. Open a GitHub issue with suggested title `"D-0: Singleton config → SQLite migration"`. User opens out-of-band; Dev Agent records the number in Dev Notes + every commit message carries `Refs #N`. (Per Epic A/B/C precedent: if `gh` CLI is not authenticated for write in the dev session, leave a `Refs #__` placeholder and document in Completion Note.)

---

## Tasks / Subtasks

- [ ] **Task 0 — Tracking issue acknowledgment (AC: #28)**
  - [ ] 0.1 Open issue (or document the `Refs #__` placeholder rationale).
  - [ ] 0.2 Capture number in Dev Notes.
  - [ ] 0.3 `Refs #N` in every commit.

- [ ] **Task 1 — Schema design call + migration v010 (AC: #1, #2, #3)**
  - [ ] 1.1 Read Dev Notes § "Schema design call — generic K/V vs per-section typed" and pick ONE shape. Document the call in Completion Note with rationale.
  - [ ] 1.2 Write `migrations/v010_singleton_config_tables.sql` per AC#1 implementing the chosen shape.
  - [ ] 1.3 Add `MIGRATION_V010` const + branch in `src/storage/schema.rs::run_migrations`.
  - [ ] 1.4 Schema-migration tests: fresh DB → v010 applied; v009 DB → v010 applied; v010 DB → no-op.

- [ ] **Task 2 — SQLite read/write helpers for singleton config (AC: #7, #10)**
  - [ ] 2.1 In `src/storage/sqlite.rs`, add `load_singleton_config()` returning a typed struct (`SingletonConfigSnapshot { global: Global, chirpstack: ChirpstackPollerConfig, opcua: OpcUaConfig, web: WebConfig }`). Field-by-field read from whichever schema shape Task 1 chose.
  - [ ] 2.2 Add `write_singleton_section(section: SingletonSection, fields: &SingletonFieldSet)` per AC#10. D-0 ships the helper; D-1 exercises it.
  - [ ] 2.3 Add `is_d0_migration_done()` reading the meta key (mirrors C-6's `is_c6_migration_done()`).
  - [ ] 2.4 Add `write_d0_migration_done()` for secondary-guard back-fill (mirrors C-6 I2-F6).
  - [ ] 2.5 Use prepared statements / `prepare_cached` per existing performance pattern in sqlite.rs.
  - [ ] 2.6 Unit tests for each helper.

- [ ] **Task 3 — One-shot TOML→SQLite singleton migration (AC: #4, #5, #6)**
  - [ ] 3.1 Create `src/storage/migrate_singleton_config.rs`.
  - [ ] 3.2 `migrate_singleton_toml_to_sqlite` per AC#4 — primary guard + secondary guard + placeholder check + EXCLUSIVE TRANSACTION + per-section row-count verify + outcome enum.
  - [ ] 3.3 Wire into `src/main.rs` post-`migrate_toml_to_sqlite` (Task 3.5 in C-6) and pre-poller-start. Extend the existing C-6 `config_migration_failed` `warn!` classifier (or refactor to typed `OpcGwError::RowCountMismatch` variant per AC#5 / AI-C-1).
  - [ ] 3.4 Audit event emission on success + failure per AC#23 + AC#5.
  - [ ] 3.5 Integration test: populate TOML, run migration, assert SQLite singleton rows match.

- [ ] **Task 4 — AppConfig load-from-SQLite override path (AC: #7, #8, #9)**
  - [ ] 4.1 In `src/config.rs` (or a new helper module), add the precedence-order logic per AC#8: env-var > SQLite > TOML > struct default.
  - [ ] 4.2 After `migrate_singleton_toml_to_sqlite` returns Migrated/AlreadyMigrated, rebuild the in-memory `AppConfig` snapshot from SQLite per AC#7.
  - [ ] 4.3 Verify secrets path is unchanged — `secrets.toml` → `AppConfig.chirpstack.api_token` + `AppConfig.opcua.user_password` continue to flow through figment per AC#9.
  - [ ] 4.4 Integration tests: env-var override + secrets.toml-still-wins-for-secrets per AC#17 tests #8 + #9.

- [ ] **Task 5 — SQLite file-permission hardening (AI-C-SEC-2 incorporation, AC: #12)**
  - [ ] 5.1 In `src/storage/pool.rs`, post-`Connection::open` chmod 0o600 on fresh creation (Unix-only via `#[cfg(unix)]`).
  - [ ] 5.2 Emit `event="storage_init" warn` once-per-boot if existing database has mode wider than 0o600.
  - [ ] 5.3 Integration tests #11 + #12 per AC#17.
  - [ ] 5.4 Update `docs/security.md` per AC#24.

- [ ] **Task 6 — Migration runbook + verification script (AC: #13, #14)**
  - [ ] 6.1 Write `docs/d-0-migration-runbook.md` mirroring `docs/c-6-migration-runbook.md` shape.
  - [ ] 6.2 Write `scripts/check-d0-migration.sh` mirroring `scripts/check-c6-migration.sh` shape, including the pass-line-on-every-exit invariant from C-6 iter-2 I2-F9.
  - [ ] 6.3 Make the script executable (`chmod +x`).
  - [ ] 6.4 Manually test the script against a freshly-migrated DB.

- [ ] **Task 7 — Documentation sync (AC: #23, #25, #26, #27)**
  - [ ] 7.1 `docs/logging.md` — new stage values per AC#23, in the same commit as the code per Epic C retro AC#24 doc-sync gate.
  - [ ] 7.2 `docs/architecture.md` — in-transition two-surface model paragraph per AC#25.
  - [ ] 7.3 DocBook user manual — new `<section id="sec-singleton-config-migration">` per AC#26; `xmllint --noout --valid` clean.
  - [ ] 7.4 `README.md` — Planning table + Current Version per AC#27.

- [ ] **Task 8 — Integration tests (AC: #17)**
  - [ ] 8.1 Create `tests/sqlite_singleton_config_migration.rs`.
  - [ ] 8.2 Implement the 14 named tests from AC#17.

- [ ] **Task 9 — Regression gate + commit (AC: #18, #19, #20, #21, #22)**
  - [ ] 9.1 `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list` still passes.
  - [ ] 9.2 All 19 C-6 tests in `tests/sqlite_config_migration.rs` still pass.
  - [ ] 9.3 `cargo test --all-targets` → record count; target ≥ 1500/0/≥73.
  - [ ] 9.4 `cargo clippy --all-targets -- -D warnings` → clean.
  - [ ] 9.5 `cargo test --doc` → no regressions.
  - [ ] 9.6 Manual smoke test (DEFERRED per the 2026-05-20 main-deadlock incident doctrine; document in Completion Note that real-world smoke against Guy's real ChirpStack is on Guy's call timing).
  - [ ] 9.7 Commit message: `Story D-0: Singleton config → SQLite migration - Implementation Complete` + `Refs #<issue>`.

- [ ] **Task 10 — Sprint-status flip + spec status (AC: #15)**
  - [ ] 10.1 Flip sprint-status `D-0-singleton-config-sqlite-migration: ready-for-dev` → `review` in the same commit as Task 9.7.
  - [ ] 10.2 Flip spec Status: `ready-for-dev` → `review` (this file).
  - [ ] 10.3 Completion Note describes the schema design call (Task 1.1) + the typed-error-variant disposition (Task 3.3) + any deferred items.

---

## Dev Notes

### Schema design call — generic K/V vs per-section typed

D-0 is the first story to introduce singleton config in SQLite. The shape is left to Dev Agent judgment with the following analysis:

**Option A — Generic key-value table:**
```sql
CREATE TABLE singleton_config (
    section TEXT NOT NULL,    -- 'global' | 'chirpstack' | 'opcua' | 'web'
    key TEXT NOT NULL,         -- 'polling_frequency' | 'host_port' | ...
    value TEXT NOT NULL,       -- TOML-serialised scalar (string / number / bool)
    updated_at TEXT NOT NULL,
    PRIMARY KEY (section, key)
);
```

Pros: one schema for all future singleton additions; new fields are zero-DDL (just INSERT new rows); easy to grep / dump for diagnostics.

Cons: weak type safety (everything is TEXT); per-section atomic writes require multi-row transactions; reads need a type-tag or out-of-band schema knowledge to deserialise correctly; harder to validate constraints at the schema level.

**Option B — Per-section typed tables:**
```sql
CREATE TABLE global_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton row enforcement
    debug INTEGER NOT NULL,
    prune_interval_minutes INTEGER NOT NULL,
    command_delivery_poll_interval_secs INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE chirpstack_config (
    id INTEGER PRIMARY KEY CHECK (id = 1),
    server_address TEXT NOT NULL,
    tenant_id TEXT NOT NULL,
    polling_frequency INTEGER NOT NULL,
    retry INTEGER NOT NULL,
    delay INTEGER NOT NULL,
    list_page_size INTEGER NOT NULL,
    inventory_cache_ttl_seconds INTEGER NOT NULL,
    inventory_uplink_max_wait_seconds INTEGER NOT NULL,
    updated_at TEXT NOT NULL
);

-- ... opcua_config + web_config similarly
```

Pros: typed-column constraints; reads + writes are typed at the DDL level; per-section atomic writes are single-row UPDATE (CHECK id=1 enforces singleton); SQLite's column types catch type mismatches at INSERT time.

Cons: every future singleton field addition requires a v011/v012/... schema migration; the four tables share the singleton-id-1 pattern but otherwise diverge.

**Precedent:** C-6 chose per-entity typed tables for `[[application]]` (Option B-equivalent). The choice was load-bearing for FK CASCADE semantics on delete; singleton config has no FK relationships, so the same forcing function doesn't apply.

**Author recommendation (not binding):** Option B (typed tables) wins on type-safety and matches the C-6 precedent. The "every new field needs a migration" cost is real but small — singleton fields don't change often, and migrations are cheap. Option A's flexibility advantage matters only if Epic D's scope expands to "every future scalar config knob lives in SQLite" — which is not the current vision.

**Dev Agent: pick one, document in Completion Note with one-sentence rationale. The picked shape locks in for D-1's UI.**

### Why D-0 fires the placeholder check

The `REPLACE_ME_WITH_OPCGW_…` placeholders in `config.toml` aren't valid runtime values — the gateway already refuses to start with them in place (`AppConfig::validate` rejects them). But D-0's migration runs BEFORE the validator (it's part of the boot sequence post-schema-migration and pre-poller-start; validation happens at a later stage). If the migration ran unconditionally, the placeholders would end up in SQLite, and a subsequent operator-supplied env-var override would have to override SQLite (the order-of-precedence logic in AC#8 would handle this correctly), but the SQLite state would carry stale placeholder values forever.

Cleaner: skip the migration when placeholders are detected. Next boot (after operator supplies the secret via env-var or secrets.toml), the placeholder is gone, and the migration runs normally. This mirrors Story C-0's "first-run wizard before any secrets-locked state is persisted" philosophy.

### Why the typed-error-variant refactor (AI-C-1) is RECOMMENDED but not REQUIRED here

Epic C retro action item **AI-C-1** flagged the substring-classifier anti-pattern (`error.to_string().contains("row_count_mismatch")` in `src/main.rs:632` for the C-6 migration). D-0 extends this classifier with a `"singleton_row_count_mismatch"` arm. Three options:

1. **Add the arm and defer the refactor** (cheapest; perpetuates the anti-pattern).
2. **Refactor to `OpcGwError::RowCountMismatch { kind: ConfigKind }` typed variant** in the same story (cleanest; closes AI-C-1 inline; small code change).
3. **Defer the entire RowCountMismatch handling to a v2.x cleanup epic** (AI-C-5 territory; doesn't help D-0 land).

Option 2 is RECOMMENDED. The refactor is small (single enum variant addition, single match-arm change, two `.into()` call-sites). Doing it now closes the AI-C-1 carry-forward and removes the classifier-string-match entirely. If Dev Agent chooses Option 1 (defer), document the rationale + link to AI-C-1 in Completion Note so the carry-forward stays visible.

### Why D-0 deliberately does NOT add a singleton-config-write surface

D-0 lands the `write_singleton_section` helper but does NOT expose it via any HTTP endpoint or other write surface. The reason: the write surface is D-1's scope (UI + endpoints). Splitting the write helper into D-0 lets D-1 focus on UI + restart-required-knob UX without re-litigating the storage helper. D-0's tests assert the helper compiles + writes correct rows; D-1 wires it to a real PUT handler.

This mirrors the C-6 → C-2 sequencing (C-6's CRUD helpers were already there from Stories 9-4/9-5/9-6; C-2's pickers consumed them). For Epic D, the storage layer (D-0) precedes the UI layer (D-1) by one story.

### Why `[opcua].user_name` is migrated to SQLite (not skipped like `user_password`)

Iter-1 review finding I1-F12 (BH-F05) flagged that the skip-list for `[opcua]` contains only `"user_password"` — `user_name` (the OPC UA client authentication username) is migrated to the SQLite `singleton_config` table. User accepted the deferral with explicit documentation:

- The OPC UA security model treats usernames as **not-secret**: they appear in audit logs, OPC UA browse-tree variable values (`opcua_session_count` exposes session usernames), and the `WWW-Authenticate: Basic realm="…"` header for web auth.
- The OPC UA threat model assumes the server already knows the username list; an attacker brute-forcing OPC UA auth has to defeat the password, not the username.
- The new AI-C-SEC-2 chmod 0o600 hardening on `data/opcgw.db` provides the practical mitigation against local-user read access on multi-user hosts.
- Symmetry with `user_password` would require moving `user_name` to `config/secrets.toml` (wider scope) or to env-var-only (worse operator UX).

Revisit if the operator threat model shifts — e.g. a multi-user shared host where the OPC UA gateway runs alongside untrusted local services. The chmod 0o600 + the existing security.md threat-model discussion cover the current model.

### Why `[command_delivery]` migrates with `[chirpstack]`

`[command_delivery]` is a sub-section that lives inside `ChirpstackPollerConfig` as `pub command_delivery: CommandDeliveryConfig` (Story 3-3). It's deserialised as part of the same struct, not as a separate config section. D-0 migrates it as part of the `[chirpstack]` row — the field list in the chirpstack typed table (if Dev Agent picks Option B) includes the command_delivery sub-fields with a prefix (`command_delivery_<field>`) OR the migration writes a separate `command_delivery_config` table mirroring the per-section pattern. Dev Agent picks one shape; the choice does NOT change observable runtime behaviour (the in-memory `AppConfig` struct is unchanged).

### Carry-forward GitHub issues / unrelated state

- **#108** (storage payload-less MetricType) — closed in Epic A. D-0 does not touch.
- **#113** (live-borrow refactor for restart-required knobs) — D-0 does NOT close this; `[web].allowed_origins` continues to be restart-required post-D-0. The D-1 UI will surface "restart required" explicitly for these knobs.
- **AI-C-SEC-1** (`prune_old_metrics` SQL format-string) — NOT D-0's scope (Epic A/storage territory).
- **AI-C-SEC-3** (`setup_get` filename log) — NOT D-0's scope (Story C-0 territory; simple one-line fix).
- **AI-C-1 / AI-C-2** (skill codification) — NOT D-0's scope; needs dedicated v2.x skill-codification epic.
- **#102** (tests/common extraction) — NOT D-0's scope; v2.x carry-forward continues.

### Performance considerations

Singleton config has at most ~30 scalar fields total across the 4 sections. The migration is bounded: <100 rows even in the worst case (Option A K/V shape). Migration duration should be <10ms on any reasonable hardware. Boot-time impact is negligible.

The `load_singleton_config` read happens once per boot. Post-boot, the in-memory `Arc<AppConfig>` snapshot serves all reads — no per-request SQLite hit on the hot path.

### Test budget delta

- D-0 adds ≥ 14 integration tests in `tests/sqlite_singleton_config_migration.rs` (per AC#17).
- D-0 adds ≥ 6 unit tests in `src/storage/sqlite.rs::tests` for the new helpers (Task 2.6).
- Net delta: ≥ +20 tests. Test target ≥ 1500/0/≥73 (C-6 closed at 1482/0/73; +18 = 1500 is the floor).

### Why no `Cargo.toml` change (strict-zero invariant)

C-6 removed `toml_edit` from `Cargo.toml` (the only allowed change in Epic C). Epic D does NOT add or remove dependencies — rusqlite + tokio + figment + serde already provide everything D-0 needs. Verify via `git diff Cargo.toml Cargo.lock` post-implementation.

### References

- Epic D scope: `_bmad-output/planning-artifacts/epics.md § Epic D § Story D.0`
- C-6 precedent (mirror this shape):
  - `_bmad-output/implementation-artifacts/C-6-toml-to-sqlite-config-migration.md` (529 lines, 27 ACs, 4-iter review loop)
  - `src/storage/migrate_config.rs` (mirror for `migrate_singleton_config.rs`)
  - `src/storage/sqlite.rs::is_c6_migration_done` + `::write_c6_migration_done` (mirror for D-0 equivalents)
  - `migrations/v009_application_config_tables.sql` (mirror for `v010_singleton_config_tables.sql`)
  - `docs/c-6-migration-runbook.md` (mirror for `d-0-migration-runbook.md`)
  - `scripts/check-c6-migration.sh` (mirror for `check-d0-migration.sh`)
- C-0 precedent (secrets.toml + chmod-0600 atomic-rename):
  - `_bmad-output/implementation-artifacts/C-0-empty-config-bootstrap.md`
  - `src/web/setup.rs::write_secrets_toml` (chmod 0o600 atomic-rename pattern; D-0 uses the same `std::fs::Permissions::from_mode(0o600)` approach for the DB file)
- Epic C retrospective (carry-forward items):
  - `_bmad-output/implementation-artifacts/epic-C-retro-2026-05-26.md`
  - AI-C-SEC-2: SQLite file permissions hardening (D-0 lands this inline)
  - AI-C-1: substring-classifier anti-pattern (D-0 optionally refactors per Dev Notes)
- Iter-3 doctrine: `_bmad-output/implementation-artifacts/deferred-work.md` § "code review of C-6-toml-to-sqlite-config-migration — iter-1..iter-4"
- Memory references (out-of-tree):
  - `project_epic_d_singleton_config_vision.md` — scope finalised 2026-05-26
  - `feedback_iter3_validation.md` — 24× doctrine streak; expect D-0 to extend it
  - `session_2026_05_26_epic_C_retro_done.md` — Epic C close + Epic D opening session context

### Project Structure Notes

- New files D-0 introduces (anticipated, pending Dev Agent decisions):
  - `migrations/v010_singleton_config_tables.sql`
  - `src/storage/migrate_singleton_config.rs` (~150-250 LOC inc. tests)
  - `tests/sqlite_singleton_config_migration.rs` (~400-500 LOC)
  - `docs/d-0-migration-runbook.md`
  - `scripts/check-d0-migration.sh` (executable; mirrors `check-c6-migration.sh`)
- Files D-0 modifies:
  - `src/storage/schema.rs` — new `MIGRATION_V010` branch
  - `src/storage/sqlite.rs` — new `load_singleton_config` / `write_singleton_section` / `is_d0_migration_done` / `write_d0_migration_done` helpers
  - `src/storage/pool.rs` — chmod 0o600 on fresh creation (AI-C-SEC-2)
  - `src/storage/mod.rs` — re-export the new module
  - `src/main.rs` — call `migrate_singleton_toml_to_sqlite` post-`migrate_toml_to_sqlite`; optionally extend the `reason=` classifier or refactor to typed variant
  - `src/config.rs` — `AppConfig::load_from_sqlite` or equivalent precedence-aware loader
  - `docs/logging.md` — new stage values
  - `docs/security.md` — file-permissions paragraph
  - `docs/architecture.md` — in-transition two-surface model paragraph
  - `docs/manual/opcgw-user-manual.xml` — new `<section id="sec-singleton-config-migration">`
  - `README.md` — Planning + Current Version
  - `_bmad-output/implementation-artifacts/sprint-status.yaml` — flip D-0 ready-for-dev → review
- Files D-0 strict-zero touches (verify by `git diff --stat` at flip time):
  - `Cargo.toml`, `Cargo.lock` (no new dependencies; AC#16)
  - `src/web/api.rs` (D-1 territory — no HTTP endpoints in D-0)
  - `src/web/csrf.rs`, `src/web/auth.rs` (no auth/CSRF changes in D-0)
  - `src/web/test_support.rs` (no new test fixtures touching web layer)
  - `tests/web_*.rs` (D-0 ships zero web-layer tests; D-1 adds them)

---

## Out of Scope

- **Web UI for editing singleton config.** That's D-1.
- **Decommissioning the TOML mutation surface entirely.** That's D-2.
- **Migrating secrets (`api_token`, `user_password`) into SQLite.** Secrets stay in `config/secrets.toml` (chmod 0o600). Encryption-at-rest in SQLite is explicitly deferred (no key-management surface in this epic).
- **Live hot-reload for restart-required knobs (PKI paths, ports, `allowed_origins`).** Issue #113 territory; D-0 surfaces the restart-required taxonomy but does not change which knobs are restart-required.
- **Migrating `[logging]` to SQLite.** Logging stays env-var-first via `OPCGW_LOG_DIR` / `OPCGW_LOG_LEVEL`; a future story may absorb it into SQLite + the D-1 UI.
- **Closing AI-C-SEC-1 (`prune_old_metrics` SQL format-string).** Epic A / storage territory; not bundled into D-0.
- **Closing AI-C-SEC-3 (`setup_get` filename log).** Story C-0 territory; one-line fix that can happen out-of-band.
- **Closing the skill-codification carry-forward (AI-A-1/2/3 + AI-B-1/2/3 + AI-C-1/2).** Needs a dedicated v2.x skill-codification epic.
- **`CR-EPIC-C-MQTT` (MQTT real-time path).** Still deferred per Epic C scope decision.

---

## Completion Note

**Implemented 2026-05-26 (same-session as Epic D scoping + spec drafting).**

### Test deltas

- `cargo test --all-targets`: **1504 / 0 / 73** (baseline post-C-6: 1482 / 0 / 73; gross +22 net new tests from D-0).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list`: still passes (verified in the regression suite).
- All 19 C-6 tests in `tests/sqlite_config_migration.rs`: still pass.
- New D-0 tests: 14 integration tests in `tests/sqlite_singleton_config_migration.rs` + 4 unit tests in `src/storage/schema.rs::tests` (v010 schema fresh DB / idempotent / section CHECK / composite PK).

### Task 1.1 — Schema design call

**Picked Option A (generic K/V).** Schema is a single `singleton_config(section TEXT, key TEXT, value TEXT, updated_at TEXT)` table with composite PK `(section, key)` + `CHECK (section IN ('global','chirpstack','opcua','web'))`.

Rationale (deviates from the spec's author-recommendation of Option B):

- **Heterogeneous fields handled uniformly** — `[opcua]` has 20+ Option-typed fields including `Option<u32>` ceilings, `Option<u16>` ports, and `[web].allowed_origins: Option<Vec<String>>` (JSON-encoded as a string). Option B would require 4 wide tables with 50+ total nullable columns + CHECK constraints replicating `AppConfig::validate`.
- **Type safety lives in Rust** — `AppConfig::validate` already enforces typed validation. SQLite is transport-only; per-section typed tables would duplicate that surface.
- **Future singleton fields = zero schema change** — no v011/v012/… migrations needed when adding a new knob.
- **D-1's UI iteration is uniform** — `SELECT key, value FROM singleton_config WHERE section=?1` is the canonical per-section read; no per-section typed deserialisation.

The choice locks in for D-1's UI (which iterates K/V pairs per section). Future stories may reconsider if a typed-table need emerges.

### Task 3.3 — Typed `OpcGwError::RowCountMismatch` refactor disposition

**Deferred to v2.x.** The substring classifier extension (adding `"singleton_row_count_mismatch"` to `main.rs`'s `reason=` arm) was the cheap path. The typed-variant refactor closes AI-C-1 but requires touching `OpcGwError` definition + the C-6 error site + the new D-0 site + the classifier match-arm at `main.rs:631-644`. Wider scope than D-0 should absorb; left for the dedicated v2.x skill-codification + cleanup epic (AI-C-5 territory). The current code adds a NEW substring matcher arm — re-extending the documented anti-pattern, which iter-N+1 reviewers should flag again.

### Task 4 — AppConfig precedence path (partial fulfillment of AC#7)

AC#7 says "the in-memory `AppConfig` snapshot is rebuilt from SQLite at boot post-D-0." D-0 ships the **write side** (TOML → SQLite via `migrate_singleton_toml_to_sqlite`) + the **read helper** (`SqliteBackend::load_singleton_config`), but does **NOT** swap the AppConfig read path. The full overlay-from-SQLite-into-AppConfig is deferred to D-2 alongside the TOML mutation-surface decommission.

Reasons documented inline at `src/main.rs` post-migration:

1. `application_config` is wrapped in `Arc<AppConfig>` at line ~444 and cloned into `reload_handle`, `storage`, `OpcUa::new`, `ChirpstackPoller::new`, `WebAuthState`, etc., **before** the SqliteBackend is available. Overlay-here would only mutate the binding; subsystem clones would still hold the figment-loaded snapshot.
2. Subsystems read restart-required knobs (PKI paths, ports, allowed_origins) once at construction; a post-construction overlay doesn't reach them.
3. On the first post-D-0 boot, the figment-loaded `application_config` and the SQLite singleton snapshot are byte-equivalent (migration just wrote one from the other). Overlay would be a no-op for this boot.
4. On subsequent boots, `config.toml` is still authoritative for the figment loader until D-2 lands. D-2 is the proper home for the figment Provider rework that puts SQLite between TOML and env-var layers (preserving AC#8 precedence ordering).

**This is a meaningful scope reduction from AC#7's plain reading.** Iter-1 reviewers may want to either: (a) accept the deferral and update AC#7's wording to clarify "D-0 writes SQLite, D-2 reads it"; or (b) reject the deferral and require a wider implementation that reorders subsystem construction to happen after the SQLite read.

### Manual smoke against Guy's real ChirpStack

DEFERRED per the 2026-05-20 main-deadlock incident doctrine (`cargo test does NOT replace real-world testing`). Same precedent as C-1 Task 10.4 / C-2 Task 8.4 / C-4 Task 7.4 / C-6 Task 11.4. Recommended for Guy's batched-validation window when the next v2.x version is ready to tag.

### GitHub tracking issue

`Refs #__` placeholder per Epic A/B/C precedent. `gh` CLI is not authenticated for write in the dev session; the issue is opened out-of-band by the user. Until then the commits carry `Refs #__`.

### `[command_delivery]` sub-section placement

The serde_json approach in `serialize_section` flattens the parent `ChirpstackPollerConfig` struct to a JSON object. `command_delivery` appears as a nested JSON object value under the `command_delivery` key in the `chirpstack` section (single row `(chirpstack, command_delivery, {...json...})`). The Rust-side load path would deserialize this back into a `CommandDeliveryConfig` struct. No per-field flattening; the sub-section round-trips as one JSON-encoded value. This is functionally equivalent to a prefix-fields approach and avoids exposing the internal sub-struct layout to D-1's UI iteration.

### Architectural decisions captured

- **chmod 0o600 fresh-creation-only**, per the spec's "don't surprise operators with retroactive chmod" guidance. Existing wider-mode databases emit a once-per-boot `storage_init` warn with the operator-action recipe in the runbook.
- **Two transaction surfaces, two isolation levels** (updated 2026-05-27 per iter-1 I1-F1 + iter-2 I2-F6 corrections): The boot-time **migration path** uses `BEGIN EXCLUSIVE TRANSACTION` via the new `SqliteBackend::migrate_singleton_sections_atomic` helper — all 4 section DELETEs + INSERTs + post-write row-count verify + done-flag write are atomic; a row-count mismatch or any insert error ROLLs BACK so the table reverts to its pre-call state (AC#4 contract). The **D-1 per-section write helper** `write_singleton_section` continues to use `BEGIN IMMEDIATE TRANSACTION` since it writes a single section atomically (delete-then-insert) and does NOT touch the meta key; IMMEDIATE avoids serialising against C-6 application-tree writers unnecessarily during runtime CRUD.
- **The placeholder-secrets guard in `migrate_singleton_toml_to_sqlite` is defense-in-depth.** `AppConfig::from_path` already rejects placeholders at parse time, so the guard is unreachable through the normal load path. Kept as belt-and-suspenders per AC#4 + because a future code path that bypasses validation (e.g. a test fixture loaded by hand) would otherwise persist placeholder strings to SQLite.

### Deferred follow-ups (added to `deferred-work.md`)

- **D-0-FOLLOWUP-1 (MED)** — Typed `OpcGwError::RowCountMismatch { kind: ConfigKind }` refactor to close AI-C-1 substring-classifier anti-pattern (now extended by D-0's `"singleton_row_count_mismatch"` arm). Recommended for v2.x skill-codification epic.
- **D-0-FOLLOWUP-2 (MED)** — Figment Provider rework so SQLite singleton snapshot overrides TOML between boots (proper AC#7 + AC#8 implementation). Land in D-2 alongside TOML mutation-surface decommission. Until then, hand-edits to `config.toml` still take effect on next boot.
- **D-0-FOLLOWUP-3 (LOW)** — Extend Test 11 (`singleton_write_done_is_idempotent`) to assert the original timestamp is preserved across re-calls (not just that the flag stays set). `INSERT OR IGNORE` preserves the original; `INSERT OR REPLACE` would refresh. The current test pins the flag presence but not the timestamp invariant. Add to iter-1 reviewer Blind Hunter prompt.

---

### Review Findings — Iter-1 (2026-05-26)

Sources: Blind Hunter (BH, 12 findings), Edge Case Hunter (ECH, 5 findings + 10 investigations), Acceptance Auditor (AA, 5 findings). **The 25th cumulative iter-N+1 doctrine validation** — three reviewers converged on 3 HIGH findings on the implementation commit. 22 raw findings → 11 patch, 4 defer, 3 dismiss.

#### Patch

- [x] [Review][Patch] **I1-F1 (HIGH) — No outer EXCLUSIVE TRANSACTION wrapping the 4-section migration; partial state not rolled back; Guard 2 misclassifies as `AlreadyMigrated`; module docstring claims behavior the code does not implement** [`src/storage/migrate_singleton_config.rs`] — Reviewers converged: BH-F01 + AA-F1 (HIGH; AC#4 violation), BH-F03 (HIGH; Guard 2 over-broad), BH-F08 (MED; no rollback), AA-F5 (LOW; stale docstring). Current code calls `write_singleton_section` four times — each with its own `IMMEDIATE TRANSACTION` that commits independently. If the 3rd section's write succeeds but the 4th fails (or the post-write `count_singleton_config` reports a mismatch), the first 3 sections are durably committed; no rollback unwinds them. Guard 2 on the NEXT boot sees `existing > 0`, back-fills the done-flag, and permanently stamps the DB as `AlreadyMigrated` with partial singleton state. AC#4 explicitly mandates `BEGIN EXCLUSIVE TRANSACTION`. Fix: add `SqliteBackend::migrate_singleton_sections_atomic(sections: &[(String, Vec<(String, String)>)]) -> Result<usize, OpcGwError>` that holds a single EXCLUSIVE transaction across all 4 section DELETEs + INSERTs + the post-write count verification + the `d0_migration_done` meta-key write. Atomic guarantee: data and flag both commit, or neither does. Module docstring updated to reflect "EXCLUSIVE TRANSACTION across all sections."

- [x] [Review][Patch] **I1-F2 (HIGH) — TOCTOU race in `pool.rs` between `Path::exists()` and `Connection::open` + `set_permissions` follows symlinks** [`src/storage/pool.rs:82-144`] — BH-F02. `was_fresh_creation = !Path::new(path).exists()` is sampled BEFORE `Connection::open`. A concurrent process (or symlink-attack via operator misconfiguration) could create the file between the two calls. `std::fs::set_permissions` follows symlinks on Linux — chmod could land on the wrong file. Fix: atomically claim file creation via `std::fs::OpenOptions::new().mode(0o600).create_new(true).open(path)` — if it succeeds, we created the file with the right mode already (no chmod needed); if it fails with `AlreadyExists`, file pre-existed (no chmod). Closes the TOCTOU window and the symlink-follow attack surface in one step.

- [x] [Review][Patch] **I1-F3 (MED) — Guard 1 `count_singleton_config().unwrap_or(0)` silently masks pool-checkout errors → logs `rows=0`** [`src/storage/migrate_singleton_config.rs:67`] — BH-F06 + ECH-F4 converged. Repeats the C-6 I2-F3 mistake (which patched the equivalent `count_applications().unwrap_or(0)` to `?` propagation). Fix: change to `?` propagation. The migration call returns `Err` on transient pool exhaustion (next boot retries idempotently); audit log no longer lies about row count.

- [x] [Review][Patch] **I1-F4 (MED) — `check-d0-migration.sh` missing `pass` line on the empty+no-done-flag exit path** [`scripts/check-d0-migration.sh:87-89`] — AA-F3. Exact repeat of the C-6 iter-2 I2-F9 finding ("every exit path emits a pass-line so automated runners detect success"). Fix: add `pass "D-0 migration check complete (singleton tables empty — gateway not yet started on post-D-0 binary, or placeholder-secrets mode active)."` before `exit 0`.

- [x] [Review][Patch] **I1-F5 (MED) — Test 11 (`singleton_write_done_is_idempotent`) is a fake regression guard** [`tests/sqlite_singleton_config_migration.rs:303-313`] — BH-F07. The test asserts `ts1 == ts2` where both are `bool`, not timestamps. The test passes regardless of whether `write_d0_migration_done` uses `INSERT OR IGNORE` or `INSERT OR REPLACE` (both leave the row present). Closes D-0-FOLLOWUP-3 from the impl commit. Fix: query the raw timestamp via a direct `rusqlite::Connection` against the temp DB BEFORE and AFTER the second `write_d0_migration_done` call; assert the timestamp strings are byte-identical (which `INSERT OR REPLACE` would break).

- [x] [Review][Patch] **I1-F6 (MED) — Missing AC#17 enumerated tests** [`tests/sqlite_singleton_config_migration.rs`] — AA-F2. The file has 14 tests numerically but substitutes 5 of the spec's enumerated tests (tests 4 / 5 / 10 / 11 / 12). Partial patch: add tests 11 (fresh-creation chmod 0o600 verified) + 12 (existing-wider-mode warn fires) — the simpler-to-test pair. Defer tests 4 (pool exhaustion mock requires a mock-backend layer that doesn't exist), 5 (row-count mismatch becomes hard to test after I1-F1 outer-transaction refactor — the EXCLUSIVE txn prevents partial commits, so synthesising a mismatch requires injection), and 10 (perf-sensitive timing; would need `#[ignore = "perf"]`). Deferred tests captured in deferred-work.md.

- [x] [Review][Patch] **I1-F7 (LOW) — Substring classifier ordering fragile + lacks comment** [`src/main.rs:674-680`] — BH-F04. The two arms `contains("singleton_row_count_mismatch")` and `contains("row_count_mismatch")` are correctly ordered (more-specific first) but the substring relationship makes the ordering load-bearing for correctness. Fix: add an inline comment explaining the ordering dependency + cross-reference D-0-FOLLOWUP-1 (typed-error-variant refactor).

- [x] [Review][Patch] **I1-F8 (LOW) — `storage_init` warn for chmod-failure includes `error=` field not documented in `docs/logging.md`** [`docs/logging.md:200`] — ECH-F3. The `error=<str>` field on the `storage_init` warn-failure variant is undocumented. Fix: extend the `storage_init` row in `docs/logging.md` to list the `error=<str>` field.

- [x] [Review][Patch] **I1-F9 (LOW) — Test 1 docstring claims `PRAGMA user_version == 10` assertion but body omits it** [`tests/sqlite_singleton_config_migration.rs:83-98`] — AA-F4. Docstring/body drift. Fix: open a raw `rusqlite::Connection` against the temp DB and assert `PRAGMA user_version == 10` at the end of `singleton_fresh_db_populated_toml_returns_migrated`.

- [x] [Review][Patch] **I1-F10 (LOW) — SQL comment in v010 migration shows port 8088 but default is 8080** [`migrations/v010_singleton_config_tables.sql`] — BH-F09. Stale-doc cosmetic. Fix: change `8088` to `8080` in the SQL comment.

- [x] [Review][Patch] **I1-F11 (LOW) — `serialize_section` error format uses `{}` for a serde-derived field name** [`src/storage/migrate_singleton_config.rs:201-205`] — BH-F11. No injection risk today (field names are Rust identifiers) but defensive `?`-Debug consistency. Fix: change `for key={}` → `for key={:?}`.

#### Deferred

- [x] [Review][Defer] **I1-F12 (MED-equivalent, user-accepted) — `[opcua].user_name` migrated to SQLite** [`src/storage/migrate_singleton_config.rs:138`] — BH-F05. The OPC UA security model treats usernames as not-secret (they appear in audit logs + browse trees). The new AI-C-SEC-2 chmod 0o600 hardening provides the practical mitigation. Deferred per explicit user acceptance with documentation: D-0 Dev Notes will add a paragraph explaining the design call (`user_name` is migrated; `user_password` stays in `secrets.toml`); revisit if the operator threat model shifts (e.g. multi-user shared host).

- [x] [Review][Defer] **I1-F13 (LOW) — `missing_secret` uses `%`-Display on a comma-joined hardcoded literal** [`src/storage/migrate_singleton_config.rs:118`] — BH-F10. No injection risk (the value comes from two literal strings hardcoded in the function); consistency-only concern with the codebase's `?`-Debug defensive convention on structured-log fields. Deferred as cosmetic.

- [x] [Review][Defer] **I1-F14 (LOW) — ECH-F5 race between 4 sequential writes and `count_singleton_config`** [`src/storage/migrate_singleton_config.rs:131-156`] — Dissolves automatically after I1-F1 (outer EXCLUSIVE transaction wraps the writes + count). Deferred as moot once I1-F1 patches land.

- [x] [Review][Defer] **I1-F15 (LOW) — AC#17 tests 4 (pool exhaustion), 5 (row-count mismatch), 10 (perf) not implemented** — Partial deferral of AA-F2. Test 4 requires a mock-backend layer that doesn't exist; test 5 becomes structurally difficult after the I1-F1 outer-transaction refactor (EXCLUSIVE prevents partial commits, so mismatch is only triggerable via injection); test 10 is perf-sensitive timing. Tests 11 + 12 are patched per I1-F6. Captured for v2.x.

#### Dismissed

- [Dismiss] BH-F12 (LOW): Missing partial-migration-test — becomes moot after I1-F1 outer-transaction wrapping (partial state can no longer exist).
- [Dismiss] ECH-F1 (LOW): Concurrent-start TOCTOU — running two opcgw processes against the same DB path is operator misconfiguration; outside the documented deployment model.
- [Dismiss] ECH-F2 (LOW): "once-per-boot" warn is "once-per-`ConnectionPool::new()`" — only matters in test scenarios that create multiple pools per process; not a runtime concern.

---

### Review Findings — Iter-2 (2026-05-27)

Sources: Blind Hunter (BH, 10 findings), Edge Case Hunter (ECH, 1 finding focused on pool-poisoning), Acceptance Auditor (AA, 3 findings). **26th cumulative iter-N+1 doctrine validation** — iter-2 caught 2 real HIGH findings in iter-1's own brand-new EXCLUSIVE-transaction logic. 14 raw findings → 9 patch, 2 defer, 3 dismiss.

#### Patch

- [x] [Review][Patch] **I2-F1 (HIGH) — `migrate_singleton_sections_atomic` row-count check uses unbounded `SELECT COUNT(*)` + no empty-slice guard** [`src/storage/sqlite.rs:3384-3401`] — BH-F01 + BH-F06 converged. The post-write count query had no WHERE clause; if any orphaned rows existed pre-call (future Section #5 direct-SQLite-hack) or if a partial caller passed only some sections, the count would include unrelated rows and mismatch spuriously. Also: an empty `sections` slice would silently write the done-flag for a no-op migration, permanently stamping the DB as migrated with zero rows. Fix: (a) added `if sections.is_empty() { return Err(...) }` precondition guard; (b) replaced the global COUNT with per-section `SELECT COUNT(*) FROM singleton_config WHERE section = ?1` loop — bounded to the input sections, error message now includes `section=<name>` for diagnostic precision.

- [x] [Review][Patch] **I2-F2 + I2-F5 (HIGH + MED) — Test 11 raw-conn WAL snapshot fake guard + 1.1s sleep insufficient under load** [`tests/sqlite_singleton_config_migration.rs`] — BH-F02 + BH-F04 converged. The iter-1 rewrite used a long-lived `rusqlite::Connection` whose WAL snapshot may not refresh between reads (defeating the byte-equality assertion). Plus the 1.1s sleep is wall-clock-bound — if both writes fall in the same second, `strftime('now')` returns the same value and the test passes under `INSERT OR REPLACE`. Fix: (a) open a fresh raw Connection for EACH read so each gets a new snapshot; (b) compute the duration to the next wall-clock second + 100ms slack so the two writes are guaranteed to observe different `strftime('now')` values.

- [x] [Review][Patch] **I2-F3 (MED) — COMMIT failure in `migrate_singleton_sections_atomic` doesn't ROLLBACK; pool connection poisoned** [`src/storage/sqlite.rs:3423-3431`] — ECH-F01. If `conn.execute_batch("COMMIT")` fails (disk full, I/O error mid-commit), the connection has a pending uncommitted EXCLUSIVE transaction. The next `BEGIN` on this pooled connection fails with "cannot start a transaction within a transaction" — pool capacity degraded by 1 slot until process restart. Pre-existing precedent at `src/storage/sqlite.rs:1387-1398` shows the correct pattern (ROLLBACK on COMMIT failure). Fix: `let _ = conn.execute_batch("ROLLBACK");` immediately inside the COMMIT-failure `.map_err` closure.

- [x] [Review][Patch] **I2-F4 (MED) — Test 16 doesn't assert warn fires; regression deleting the warn block would still pass** [`tests/sqlite_singleton_config_migration.rs`] — BH-F05. AC#12 has two parts (don't chmod retroactively + emit `storage_init` warn); iter-1's Test 16 covered only part 1. Fix: import the tracing-test capture helpers (`init_test_subscriber` / `captured_logs` / `clear_captured_logs`) from the C-6 test suite pattern; clear the buffer before the second backend creation; assert `storage_init` event + `mode="644"` field both appear in the captured logs.

- [x] [Review][Patch] **I2-F6 (MED) — Completion Note bullet says "Migration uses IMMEDIATE TRANSACTION" — factually inverted after I1-F1** [`_bmad-output/implementation-artifacts/D-0-singleton-config-sqlite-migration.md`] — AA-F02. After the iter-1 EXCLUSIVE-transaction refactor, the migration path uses EXCLUSIVE; the IMMEDIATE-only bullet was not updated. Fix: rewrite the bullet to clearly distinguish the two transaction surfaces — migration (EXCLUSIVE via `migrate_singleton_sections_atomic`) vs D-1 per-section write helper (IMMEDIATE via `write_singleton_section`).

- [x] [Review][Patch] **I2-F7 (LOW) — AC#17 tests 7, 8, 9 absent and not tracked in `deferred-work.md`** — AA-F01. Tests 7 (AppConfig read from SQLite), 8 (env-var override on top of SQLite), 9 (secrets.toml still wins) map directly to the AC#7 deferral but `deferred-work.md` I1-F15 only names tests 4/5/10. Fix: add explicit deferred-work entry linking AC#17 tests 7/8/9 to D-0-FOLLOWUP-2 (figment Provider rework for AC#7+AC#8 lands in D-2).

- [x] [Review][Patch] **I2-F8 (LOW) — Spec AC#4 error format includes `section=X` field; impl now produces it after I2-F1 per-section counts** [`spec AC#4`, `src/storage/sqlite.rs:3397-3403`] — AA-F03. Spec wording was ahead of the impl in the original D-0 spec; iter-2 I2-F1 added `section=X` to the error message as a side effect of the per-section count refactor. Iter-2 closes the gap by ensuring the spec and impl agree.

- [x] [Review][Patch] **I2-F9 (MED) — Atomic-create probe failure logs at `debug!` only; silent degraded-security path** [`src/storage/pool.rs:104-116`] — BH-F03. When the probe fails with `PermissionDenied` or other unexpected error (other than `AlreadyExists`), the chmod 0o600 guarantee is NOT delivered but the failure is logged only at `debug` — invisible at default log levels. Operators investigating "why does the DB file have unexpected permissions" need this signal. Fix: change `tracing::debug!` to `tracing::warn!` + extend the message to mention the missed guarantee.

#### Deferred

- [x] [Review][Defer] **I2-F10 (MED) — `BEGIN EXCLUSIVE TRANSACTION` fails with `SQLITE_BUSY` on concurrent connections** [`src/storage/sqlite.rs`] — BH-F07. SQLite EXCLUSIVE prevents other connections from acquiring any lock, including SHARED for reads. With a pool of N > 1 connections, if another task holds an open connection, BEGIN EXCLUSIVE fails immediately (not retryable). Documented behavior of SQLite + pool interaction; the migration retries idempotently on next boot per the AC#5 safety net. Deferred as a documented design-level limitation; mentioned in the new method docstring.

- [x] [Review][Defer] **I2-F11 (LOW) — Test 15 may fail on filesystems with forced mode bits (tmpfs no-permission, root-CI)** [`tests/sqlite_singleton_config_migration.rs`] — BH-F09. Test reliability concern on certain CI configurations; the gateway code is correct. Deferred as test-environment-dependent; if CI surfaces a real failure, add `#[ignore = "filesystem-dependent"]` or a CI-specific override.

#### Dismissed

- [Dismiss] BH-F08 (LOW): "the wider-mode warn block may have been deleted in the iter-1 diff" — ECH investigation 7 verified the warn block at `src/storage/pool.rs:140-159` is intact; not regression-affected.
- [Dismiss] BH-F10 (LOW): `drop(stmt)` is load-bearing for the borrow checker (releases shared borrow on `conn` so `query_row` can proceed); not a bug, compile-time enforced.
- [Dismiss] ECH-others: ECH was sharply focused on a single high-signal finding (COMMIT poisoning); no other findings worth dismissing.

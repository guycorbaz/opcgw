# Story C-6: TOML→SQLite Configuration Migration Runbook

This runbook covers the one-time data migration introduced in opcgw v2.x Story C-6, which moves the `[[application]]` tree (applications, devices, metrics, commands) from `config.toml` into the SQLite database.

---

## Pre-migration checklist

**Complete these steps before upgrading to the first post-C-6 binary.**

1. **Back up the SQLite database:**
   ```bash
   cp opcgw.db opcgw.db.pre-c6-backup
   ```
   Docker operators: run this command inside the container or against the mounted volume path on the host.

2. **Back up the TOML config file:**
   ```bash
   cp config/config.toml config/config.toml.pre-c6-backup
   ```

3. **Verify both backups are readable:**
   ```bash
   sqlite3 opcgw.db.pre-c6-backup 'SELECT COUNT(*) FROM meta;'
   head -5 config/config.toml.pre-c6-backup
   ```

4. **Note the current application count** for post-migration verification:
   ```bash
   grep -c '^\[\[application\]\]' config/config.toml
   ```

---

## What the migration does

On the first boot of the post-C-6 binary, opcgw detects that the `applications` SQLite table is empty AND the TOML `application_list` is non-empty, then automatically:

1. Opens a transaction.
2. Inserts each TOML application, device, metric, and command into SQLite.
3. Verifies row counts match the TOML source.
4. Commits and records a `config_migrated_from_toml_at` timestamp in the meta table.
5. Emits `event="config_migration" stage="toml_to_sqlite"` to the audit log with counts and duration.

The migration runs in < 100 ms for inventories under 100 devices; up to a few seconds for 1000+ devices.

---

## Migration triggers in the log

A successful migration produces:

```
event="config_migration" stage="toml_to_sqlite" applications=N devices=N metrics=N commands=N duration_ms=N
```

A skipped migration (C-0 empty-bootstrap path or already-migrated DB) produces:

```
event="config_migration" stage="skipped_empty_source"
```
or simply nothing (already-migrated DB is silently a no-op at schema version 9+).

A secondary already-migrated guard back-fill failure (apps present but done-flag write fails — e.g. a direct SQLite import on a pool-exhausted boot) produces:

```
event="config_migration" stage="already_migrated_backfill_failed" error="..."
```

Non-fatal — the applications table is intact and the gateway boots normally. The secondary guard re-fires on every subsequent boot until the done-flag is successfully written, so a transient pool-exhaustion clears itself; a persistent failure (e.g. disk full) warns on every boot but does not block service.

A failed migration produces:

```
event="config_migration_failed" reason="row_count_mismatch" expected=N actual=M
```
or
```
event="config_migration_failed" reason="insert_failed" ...
```

On failure, the gateway falls back to TOML-driven boot (the legacy path) for this start-up only. The migration is retried idempotently on the next boot.

---

## Post-migration verification

Run the included `scripts/check-c6-migration.sh` for an automated summary:

```bash
bash scripts/check-c6-migration.sh opcgw.db
```

Or verify manually:

```bash
# Count migrated rows
sqlite3 opcgw.db 'SELECT COUNT(*) FROM applications;'
sqlite3 opcgw.db 'SELECT COUNT(*) FROM devices;'
sqlite3 opcgw.db 'SELECT COUNT(*) FROM metrics;'
sqlite3 opcgw.db 'SELECT COUNT(*) FROM commands;'

# Confirm migration timestamp recorded
sqlite3 opcgw.db "SELECT value FROM meta WHERE key='config_migrated_from_toml_at';"
```

Cross-check against TOML source:

```bash
# Applications
grep -c '^\[\[application\]\]' config/config.toml

# Devices (nested under application blocks)
grep -c '^\[\[application\.device\]\]' config/config.toml
```

---

## Post-migration runtime behaviour

- **The TOML file is no longer monitored.** Hand-edits to `config.toml`'s `[[application]]` section have no effect at runtime. Use the web UI (`/applications.html`, `/devices-config.html`) for CRUD operations.
- **Singleton sections** (`[global]`, `[chirpstack]`, `[opcua]`, `[web]`) remain in `config.toml` and are read at startup. Changing them requires a process restart.
- **Hot-reload** is now triggered by web-UI CRUD writes, not SIGHUP or file-watching. The `notify_crud_write` mechanism updates the in-memory snapshot immediately after each SQLite commit.

---

## Rollback procedure

C-6 is a **one-way migration**. To roll back to a pre-C-6 binary:

1. Stop opcgw.
2. Restore the pre-migration SQLite backup:
   ```bash
   cp opcgw.db.pre-c6-backup opcgw.db
   ```
3. Restore the TOML backup:
   ```bash
   cp config/config.toml.pre-c6-backup config/config.toml
   ```
4. Deploy the pre-C-6 binary and restart.

**Important:** Any web-UI CRUD changes made while running the post-C-6 binary will be in SQLite but NOT in the TOML backup. Those changes will be lost on rollback.

---

## Migration timing expectations

| Inventory size       | Expected duration |
|----------------------|-------------------|
| < 100 devices        | < 100 ms          |
| 100 – 1000 devices   | < 1 second        |
| 1000 – 10000 devices | 1–5 seconds       |
| > 10000 devices      | Contact support   |

The gateway's OPC UA server and ChirpStack poller do not start until after the migration completes.

---

## Troubleshooting

**Migration failed with `row_count_mismatch`**

The TOML parser and SQLite insert round-tripped with different counts. Typical cause: malformed `[[application]]` block in `config.toml`. Check the log for the failed `config_migration_failed` line, fix the TOML, and restart.

**Gateway boots but `applications` table is empty and no migration event logged**

The migration detection saw an already-migrated DB (schema version ≥ 9 AND `applications` is non-empty from a prior run) or an empty `application_list` in TOML. Verify via:
```bash
sqlite3 opcgw.db 'SELECT schemaVersion FROM meta WHERE key="schemaVersion";'
sqlite3 opcgw.db 'SELECT COUNT(*) FROM applications;'
```

**Web UI shows no applications after migration**

Verify the migration actually ran by checking the log for `event="config_migration"`. If it ran, verify the `applications` table has rows. If both are true, check the web server is reachable and the API token is correct.

---

## Related documentation

- `scripts/check-c6-migration.sh` — automated verification script
- `docs/architecture.md` — post-C-6 storage architecture
- `docs/logging.md` — `config_migration` and `config_migration_failed` audit events

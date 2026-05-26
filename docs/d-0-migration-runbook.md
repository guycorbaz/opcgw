# D-0 Singleton-Config Migration Runbook

**Story D-0 — Singleton Configuration → SQLite Migration**

This runbook covers the operator-side procedure for the first boot of a
post-D-0 opcgw binary on a gateway that already has a populated
`config/config.toml`. The migration is one-shot, idempotent, and runs
automatically on the first boot — operators do not invoke it manually.

---

## What D-0 changes

Story D-0 extends the C-6 work that moved the `[[application]]` collection
tree (applications/devices/metrics/commands) from `config.toml` into
SQLite. D-0 does the same for the four **singleton** sections:

- `[global]`
- `[chirpstack]` (excluding `api_token`, which stays in `secrets.toml`)
- `[opcua]` (excluding `user_password`, which stays in `secrets.toml`)
- `[web]`

After D-0 lands, opcgw has three persistence surfaces:

1. **SQLite** (`data/opcgw.db`, chmod 0o600 per AI-C-SEC-2 — see § File permissions below) — authoritative for non-secret configuration + metric values.
2. **`config/secrets.toml`** (chmod 0o600 via atomic-rename, established by Story C-0) — operator-supplied secrets.
3. **`config.toml`** — bootstrap seed only. After D-0 writes the singleton config to SQLite, the TOML values are still used by figment at boot time, but D-2 (a future story) will flip the precedence so SQLite is canonical. Until D-2 lands, hand-edits to `config.toml` continue to take effect on next boot.

---

## Pre-migration backup (recommended)

Before deploying the post-D-0 binary, back up the existing database:

```bash
cp data/opcgw.db data/opcgw.db.pre-d0-backup
```

This is the rollback artefact. If anything goes wrong post-migration,
stop opcgw, restore the backup, and downgrade the binary:

```bash
systemctl stop opcgw         # or `docker compose down`
cp data/opcgw.db.pre-d0-backup data/opcgw.db
# downgrade binary to pre-D-0 version, then restart
```

---

## The migration itself

On the first boot of a post-D-0 binary, opcgw automatically:

1. Runs schema migration v010 (adds the `singleton_config` table to the
   existing v009 SQLite schema). This is idempotent — re-running is a
   no-op on a v010+ database.
2. Checks the `d0_migration_done` meta key. If present, the migration
   path short-circuits as `AlreadyMigrated` (primary guard).
3. If the meta key is absent but `singleton_config` already has rows,
   the secondary guard fires: back-fills the meta key (best-effort) and
   short-circuits as `AlreadyMigrated`. Subsequent boots use the
   faster primary guard.
4. Checks whether the in-memory `AppConfig` carries placeholder
   secrets (`REPLACE_ME_WITH_OPCGW_…`). If yes, the migration is
   deferred to the next boot — the operator has not yet supplied
   `[chirpstack].api_token` and/or `[opcua].user_password` via
   env-var or `config/secrets.toml`. The gateway runs from the
   in-memory snapshot for this boot.
5. Otherwise, opens an `IMMEDIATE` transaction (via
   `write_singleton_section`), bulk-writes the four sections' non-secret
   fields to `singleton_config`, verifies the row count, writes the
   `d0_migration_done` meta key, and commits.

On row-count mismatch or write failure, opcgw falls back to TOML-driven
boot for the current start-up only. The migration retries idempotently on
next boot.

---

## Migration triggers in the log

A successful singleton migration produces:

```
event="config_migration" stage="singleton_toml_to_sqlite" sections=4 rows=N duration_ms=N
```

A skipped migration (already-migrated DB) produces:

```
event="config_migration" stage="singleton_already_migrated" rows=N
```

A skipped migration (placeholder-secrets path) produces:

```
event="config_migration" stage="skipped_placeholder_singleton" missing_secret="chirpstack.api_token,opcua.user_password"
```

A secondary already-migrated guard back-fill failure (rare; pool exhaustion at boot) produces:

```
event="config_migration" stage="singleton_already_migrated_backfill_failed" error="..."
```

Non-fatal — the gateway boots normally. The back-fill is retried on every subsequent boot until the meta key write succeeds.

A failed migration produces:

```
event="config_migration_failed" reason="singleton_row_count_mismatch" error="..."
```

or

```
event="config_migration_failed" reason="insert_failed" error="..."
```

On failure, the gateway falls back to TOML-driven boot for this start-up
only. The migration is retried idempotently on the next boot.

---

## Post-migration verification

Run the verification script bundled with opcgw:

```bash
bash scripts/check-d0-migration.sh data/opcgw.db
```

The script reports:

- The SQLite schema version (expected: 10 or higher).
- The `d0_migration_done` meta key (expected: non-empty ISO-8601 timestamp).
- Per-section row counts (expected: > 0 for each of `global`, `chirpstack`, `opcua`, `web`).
- The SQLite file mode (expected: 0o600).
- Sample rows from each section.

Exit code 0 = pass; non-zero = something to investigate.

---

## File permissions (AI-C-SEC-2)

Story D-0 lands the SQLite file-permission tightening flagged by the
Epic C security review (`AI-C-SEC-2`). On fresh creation of `data/opcgw.db`,
opcgw chmod's the file to 0o600 so configuration and metric values are
readable only by the gateway user.

**Existing databases (pre-D-0 deployments) are NOT chmod'd retroactively** —
operator-defined umask + supervisor permission models are preserved.
If the file mode is wider than 0o600 on an existing deployment, opcgw
emits a once-per-boot warn at `event="storage_init"`:

```
event="storage_init" path="data/opcgw.db" mode="644" "SQLite DB file mode is wider than 0o600..."
```

To apply the tightening to an existing deployment:

```bash
chmod 0600 data/opcgw.db
```

For Docker deployments, ensure the bind-mounted volume preserves the mode
(e.g. `:rw,Z` SELinux relabel + a tight host-side `chmod`). For systemd
deployments, consider setting `UMask=0077` in the service unit so future
opcgw-created files inherit a tight default.

Windows deployments use ACLs rather than POSIX mode bits; the chmod
recipe does not apply. See `docs/security.md` for the Windows guidance.

---

## Rollback

D-0 is a **one-way** migration. The figment loader still reads
`config.toml` on every boot for backward compatibility, but the
`singleton_config` SQLite rows take precedence on future stories (D-2)
and operators editing via the future D-1 web UI write to SQLite
exclusively.

To downgrade to a pre-D-0 binary:

1. Stop opcgw.
2. Restore the pre-D-0 backup:
   ```bash
   cp data/opcgw.db.pre-d0-backup data/opcgw.db
   ```
3. Replace the binary with the pre-D-0 version.
4. Start opcgw. It will read singletons from `config.toml` per the
   pre-D-0 contract.

Note: any singleton changes made via the (future) D-1 web UI on the
post-D-0 binary will be lost on rollback — that's the price of the
one-way migration. For long-term safety, keep `config.toml` in sync
with the SQLite state until D-2 lands and the read-path swap makes
TOML inert.

---

## Common issues

### `singleton_row_count_mismatch` warning at boot

The migration wrote fewer rows than expected — typically a serialisation
edge case for a non-default Option field. Check the warn log for the
specific error string. The gateway falls back to TOML for the current
boot; the migration retries on next boot. If the issue recurs, file a
GitHub issue with the warn-log excerpt and your `config.toml` (with
secrets redacted).

### Migration never runs (no log line at all)

Likely cause: placeholder secrets in `config.toml`. Check whether
`[chirpstack].api_token` or `[opcua].user_password` still carries the
`REPLACE_ME_WITH_OPCGW_…` placeholder. Supply real values via env-var
or `config/secrets.toml` and restart.

### SQLite file is world-readable after the migration

Check whether the database existed BEFORE the post-D-0 boot. The D-0
chmod 0o600 only applies on fresh creation. Apply the chmod manually:

```bash
chmod 0600 data/opcgw.db
```

Or set a tighter umask (`UMask=0077` for systemd, `:rw,Z` for SELinux
Docker, etc.) for future deployments.

### Migration runs on every boot

Check that the `d0_migration_done` meta key is being written:

```bash
sqlite3 data/opcgw.db "SELECT key, value FROM meta WHERE key='d0_migration_done';"
```

If empty, run the verification script and check the warn log for a
`stage="singleton_already_migrated_backfill_failed"` event. Resolve the
underlying SQLite write failure (disk full, permission denied, etc.).

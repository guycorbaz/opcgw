# Deployment Guide — opcgw

> Generated: 2026-04-01 | Updated: 2026-06-25 for v2.3.0 | Scan Level: Exhaustive

## Deployment Options

### 1. Native Binary

```bash
# Build release binary
cargo build --release

# Binary location
./target/release/opcgw

# Run with default config
./target/release/opcgw

# Run with custom config
./target/release/opcgw -c /path/to/config.toml
```

**Required files at runtime:**
- `config/config.toml` — Bootstrap-seed configuration (authoritative config lives in SQLite; see [Configuration model](#configuration-model-sqlite-authoritative) below)
- `config/secrets.toml` — Operator secrets (`[chirpstack].api_token`, `[opcua].user_password`), chmod 0600
- `pki/` — OPC UA certificates directory (with own/, private/, trusted/, rejected/ subdirs)
- `data/` — SQLite database directory (authoritative configuration + metric history)
- `log/` — Log output directory (created automatically)

### 2. Docker

**Dockerfile** uses a multi-stage build:
1. **Builder stage:** `rust:1.94.0` — installs protobuf compiler, builds release binary
2. **Runtime stage:** `ubuntu:24.04` — minimal runtime with `iputils-ping`, runs as **non-root user `opcgw` (UID 10001)**

The container exposes **4840** (OPC UA) and **8080** (web UI). Because it runs non-root, the host-side bind-mount targets must be owned by UID 10001 before first start:

```bash
sudo chown -R 10001:10001 ./log ./config ./pki ./data
sudo chmod 700 ./pki/private
sudo chmod 600 ./pki/private/*
```

```bash
# Build image
docker build -t opcgw .

# Run standalone (pull the published image, or use the local build tag)
docker run -d \
  --name opcgw \
  -p 4840:4840 \
  -p 8080:8080 \
  --env-file ./.env \
  -v ./config:/usr/local/bin/config \
  -v ./pki:/usr/local/bin/pki \
  -v ./log:/usr/local/bin/log \
  -v ./data:/usr/local/bin/data \
  gcorbaz/opcgw:2.3
```

> The `./data` mount is **required** — it holds the SQLite database. Without it the database lives in the ephemeral container layer and is lost on `docker compose down` / container replacement.

### 3. Docker Compose

```bash
docker compose up -d
```

**docker-compose.yml configuration:**
- Service: `opcgw`
- Image: `docker.io/gcorbaz/opcgw:2.3`
- Port mappings: `4840:4840` (OPC UA) + `8080:8080` (web UI), driven by `OPCGW_OPCUA__HOST_PORT` / `OPCGW_WEB__PORT` (defaults 4840 / 8080)
- Environment: `env_file: .env` — secret credentials and any `OPCGW_*` overrides live in a single `.env` file (copy `.env.example` → `.env`, fill in values, `chmod 600 .env`)
- Restart policy: `always`
- Healthcheck: TCP liveness probe against the OPC UA port (the runtime image has no curl/wget)
- Volume mounts: `log/`, `config/`, `pki/`, **`data/`** (the `data/` mount is required for SQLite persistence — see note above)

## Network Requirements

| Connection | Protocol | Default Port | Direction | Purpose |
|-----------|----------|-------------|-----------|---------|
| ChirpStack | gRPC (HTTP/2) | 8080 | Outbound | Device metrics polling, uplink event stream, command enqueue |
| OPC UA Clients | OPC UA (TCP) | 4840 | Inbound | SCADA/client connections |
| Web UI | HTTP (Axum) | 8080 | Inbound | Setup wizard + configuration dashboard |

## Configuration for Production

### Security Hardening

1. **Disable null endpoint:** Remove the `null` security endpoint from OPC UA config
2. **Use proper certificates:** Set `create_sample_keypair = false` and provide CA-signed certificates
3. **Strong credentials:** Change default `user1`/`user1` username/password
4. **API token security:** Use environment variables for `api_token` instead of config file:
   ```bash
   export OPCGW_CHIRPSTACK__API_TOKEN="your-secure-token"
   ```
5. **Enable cert time validation:** Ensure `check_cert_time = true`

### Performance Tuning

- **Polling frequency:** Adjust `chirpstack.polling_frequency` based on device update rates
- **Retry settings:** Configure `chirpstack.retry` and `chirpstack.delay` for network resilience
- **Logging levels:** Reduce to `info` or `warn` in production via the `RUST_LOG` env filter (tracing / tracing-subscriber)

## Configuration model (SQLite authoritative)

From v2.x the gateway stores its configuration in **SQLite**, not in text files:

- **SQLite** (in the mounted `data/` directory) is **authoritative** for all non-secret runtime configuration. Schema migrations v001–v013 run automatically and forward-only on boot.
- **`config/config.toml`** is a **bootstrap seed only** — read at boot to populate a fresh database, then overridden by SQLite for any key the operator has set through the web UI. Operators may delete it post-migration.
- **`config/secrets.toml`** (chmod 0600) holds the operator secrets (`[chirpstack].api_token`, `[opcua].user_password`); the gateway never mutates this file at runtime.
- **Precedence:** env (`OPCGW_*`) > SQLite > `config.toml` > built-in default.

**First boot uses the browser `/setup` wizard** — no text-file editing required. Point a browser at the web UI port (`http://<host>:8080`) and the gateway serves the first-run wizard to capture the ChirpStack server/tenant/token and the OPC UA password.

**Staged-apply.** Configuration changes made through the web UI are **staged** to SQLite without disturbing the running gateway; `GET /api/status` reports `pending_changes: true` until the operator clicks **Apply changes** (`POST /api/config/apply`), which performs a single in-process soft restart of the data-plane. The Docker **container is never restarted** on a config change.

## PKI Certificate Management

```
pki/
├── own/           # Server's own certificate (cert.der)
├── private/       # Server's private key (private.pem)
├── trusted/       # Trusted client CA certificates
└── rejected/      # Auto-rejected unknown client certificates
```

When `create_sample_keypair = true`, the server auto-generates self-signed certificates on first run. For production, place proper certificates in the appropriate directories.

## Health Monitoring

The web server exposes dedicated health endpoints (web UI must be enabled):

- **`GET /api/health`** (added v2.1.0) — minimal smoke endpoint returning `{"status":"ok","version":"..."}`. Suitable for external uptime/load-balancer probes.
- **`GET /api/status`** — richer status: health metrics, `pending_changes` (staged-but-unapplied config), and `poll_interval_secs`.

The container healthcheck in `docker-compose.yml` is a TCP liveness probe against the OPC UA port (the runtime image ships no curl/wget). The web dashboard (Story F-3) derives a client-side health verdict and per-device freshness panel from `/api/status` + `/api/devices`.

Additional signals:
- **Log output:** structured tracing logs (`log/` directory and/or stdout)
- **OPC UA diagnostics:** Connect an OPC UA client with diagnostics enabled
- **ChirpStack status:** The gateway tracks server availability internally (exposed as internal metric `cp0`)

## Epic A migration

> **Who this section is for:** operators upgrading an existing opcgw deployment from v2.0-rc (pre-Epic-A schema, v006) to v2.0 GA (post-Epic-A schema, v008). New deployments do not need this section — a fresh gateway creates the current-schema database on first startup.
>
> **Note (v2.1+):** the later migrations (v009–v013, covering SQLite-authoritative singleton config and Epic E/F additions) likewise run **automatically and forward-only** on boot — the same mechanism described below, no operator intervention.

[Issue #108](https://github.com/guycorbaz/opcgw/issues/108) shipped a payload-less `MetricType` enum across Phase A and Phase B that flattened every persisted metric value to its discriminant string (`"Float"`, `"Int"`, `"Bool"`, `"String"`) instead of the real measurement. Epic A (stories A-1 through A-7) re-shapes the storage layer to carry real measurement payloads end-to-end. The schema changes land in two SQLite migrations:

- **v007** — adds typed value columns (`value_real REAL`, `value_int INTEGER`, `value_bool INTEGER`, `value_text TEXT`) plus a `value_type` discriminant column to `metric_values` and `metric_history`. Pre-Epic-A rows are tagged `value_type = 'legacy'` via the column default; no rows are dropped.
- **v008** — adds an exactly-one-non-NULL `CHECK` constraint on the typed columns (via `CREATE TABLE … AS SELECT` pattern, wrapped in `BEGIN/COMMIT`).

Both migrations are applied **automatically on the first startup of the v2.0 binary** via `src/storage/schema.rs::run_migrations`. No CLI flag, no operator intervention required.

The architectural rationale lives in [`_bmad-output/planning-artifacts/architecture.md` § "Storage Payload Migration Strategy"](../_bmad-output/planning-artifacts/architecture.md); the per-event audit-log taxonomy lives in [`docs/logging.md`](logging.md); the per-story specs live in [`_bmad-output/planning-artifacts/epics.md` § Epic A](../_bmad-output/planning-artifacts/epics.md).

**Migration runner sequence (load-bearing for crash-safety).** Both migrations land via `src/storage/schema.rs::run_migrations` in a strict order:

1. v007 — `ALTER TABLE ADD COLUMN` (metadata-only, no row copy) executes.
2. `PRAGMA user_version = 7` is bumped — v007 is **committed independently** before v008 starts.
3. v008 — `CREATE TABLE … AS SELECT` (full row copy, BEGIN/COMMIT-wrapped) executes.
4. `PRAGMA user_version = 8` is bumped after v008's transaction commits.

**Crash-safety:** if the gateway dies mid-v008, v008's BEGIN/COMMIT rolls back to the v007 shape — the operator's database is left at `user_version = 7` with v007 columns added but v008's CHECK constraint absent. Restarting the gateway is the recovery: `run_migrations` is idempotent and will retry v008 against the v007-shaped database. The "version 7 = interrupted upgrade" state is therefore a real, recoverable intermediate state, NOT a bug.

### Pre-upgrade checklist

Before stopping the v2.0-rc gateway:

1. **Take a file-level backup of `opcgw.db`.** The database path is configured via `[storage].database_path` in `config.toml` (default `./opcgw.db`). The backup is your **only** rollback path — the migration is one-way (see [Rollback contract](#rollback-contract) below).

   ```bash
   cp /path/to/opcgw.db /path/to/opcgw.db.pre-epic-a.bak
   # Also copy the WAL + SHM sidecar files if present:
   cp /path/to/opcgw.db-wal /path/to/opcgw.db-wal.pre-epic-a.bak 2>/dev/null || true
   cp /path/to/opcgw.db-shm /path/to/opcgw.db-shm.pre-epic-a.bak 2>/dev/null || true
   ```

2. **Note the current schema version.** Either run the pre-flight script:

   ```bash
   ./scripts/check-schema-version.sh /path/to/opcgw.db
   ```

   Or run the equivalent SQLite one-liner directly (the pre-flight script wraps this with a human-readable recommendation):

   ```bash
   sqlite3 /path/to/opcgw.db "PRAGMA user_version;"
   ```

   - Version **`6`** → pre-Epic-A. You will run the migration.
   - Version **`7`** → an interrupted prior upgrade left the database at v007 without v008. Starting the v2.0 gateway will complete the migration.
   - Version **`8`** → already migrated. No action needed.

3. **Decide which migration path to take.** Two paths are supported (see below); the difference is whether you preserve pre-Epic-A historical rows or start fresh.

### Path A — Default: in-place auto-migration

This is the recommended path for operators who want to preserve historical metric rows captured under the v2.0-rc schema.

1. Stop the v2.0-rc gateway (`docker compose stop` / `systemctl stop opcgw` / `Ctrl-C` against the foreground process).
2. Replace the binary, or pull the new Docker image:
   ```bash
   docker compose pull opcgw   # if running via compose
   # or
   cargo install --path .       # if running from source
   ```
3. Start the v2.0 gateway pointing at the **same** `opcgw.db` file.
4. On startup, `run_migrations()` automatically applies v007 (typed value columns) and v008 (exactly-one-non-NULL CHECK constraint). Pre-Epic-A rows are tagged `value_type = 'legacy'` with `NULL` typed columns by the v007 column default; no rows are dropped.
5. Verify the migration succeeded by inspecting the gateway's startup logs. Pick the recipe matching your deployment shape:
   ```bash
   # systemd-managed gateway:
   journalctl -u opcgw -n 200 | grep -E "Applied migration v00(7|8)"

   # Docker Compose:
   docker compose logs opcgw | grep -E "Applied migration v00(7|8)"

   # Plain Docker:
   docker logs opcgw 2>&1 | grep -E "Applied migration v00(7|8)"

   # Foreground binary writing to log/opcgw.log:
   tail -n 200 log/opcgw.log | grep -E "Applied migration v00(7|8)"
   ```
   You should see two `info!` lines emitted by `src/storage/schema.rs:222 + :251`:
   ```
   Applied migration v007_typed_value_columns          version=7
   Applied migration v008_typed_value_constraints      version=8
   ```
6. **Wait for the first poll cycle to complete.** OPC UA clients will see `BadDataUnavailable` on legacy rows for the duration of one poll interval. Once the poller UPSERTs each metric with a typed payload, the legacy rows are replaced with real values and OPC UA Reads return the actual measurement.

### Path B — Alternate: drop-and-recreate

This is the right path for operators who do not need pre-Epic-A history (test deployments, dev gateways, or production gateways where the historical rows are uninteresting under the new typed-payload contract).

1. Stop the v2.0-rc gateway.
2. Remove the database file and its sidecar files:
   ```bash
   rm /path/to/opcgw.db /path/to/opcgw.db-wal /path/to/opcgw.db-shm
   ```
   (The `db-wal` and `db-shm` sidecar files exist if the database was opened in WAL mode and the gateway shut down without a checkpoint — typically harmless to remove together with the main file.)
3. Replace the binary / pull the new Docker image.
4. Start the v2.0 gateway. It creates a fresh database at v008 schema with zero rows; the next poll cycle populates `metric_values` with real typed payloads from the start.

### Post-migration verification

After either Path A or Path B, the following checks confirm the gateway is operating on the v008 schema:

1. **Schema version is 8.**
   ```bash
   sqlite3 /path/to/opcgw.db "PRAGMA user_version;"
   # Expected output: 8
   ```

2. **First poll cycle completed.** Look for `operation="poll_cycle_end"` (per `src/chirpstack.rs:1523`) in the gateway logs. Pick the recipe matching your deployment shape (same four shapes as the Path A step 5 verification):
   ```bash
   # systemd:
   journalctl -u opcgw | grep "poll_cycle_end" | head -3

   # Docker Compose:
   docker compose logs opcgw 2>&1 | grep "poll_cycle_end" | head -3

   # Plain Docker:
   docker logs opcgw 2>&1 | grep "poll_cycle_end" | head -3

   # Foreground binary writing to log/opcgw.log:
   tail -n 200 log/opcgw.log | grep "poll_cycle_end" | head -3
   ```
   The default poll frequency is configured via `[chirpstack].polling_frequency` in `config.toml` (default 60 s). On a 1-hour interval, expect to wait up to an hour before legacy rows are replaced with typed payloads.

3. **OPC UA Read returns the actual measurement payload** (not `BadDataUnavailable` for typed rows, not the discriminant string for any row). Connect an OPC UA client (UaExpert, FUXA, Ignition) and Read a metric variable. Expected `DataValue.value` per `MetricType` variant (verified against `src/opc_ua.rs::convert_metric_to_variant`):

   | Storage `MetricType` | OPC UA `Variant`                                                |
   | -------------------- | --------------------------------------------------------------- |
   | `Float(f64)`         | `Variant::Float(f32)` — A-4 narrows f64 → f32 at the OPC UA boundary; expect f32 precision in the SCADA client |
   | `Int(i64)` where `\|i\| ≤ i32::MAX` | `Variant::Int32(i32)` — `i32::try_from(i64)` succeeds; the gateway prefers the narrower Int32 |
   | `Int(i64)` where `\|i\| > i32::MAX` | `Variant::Int64(i64)` — fallback when `i32::try_from` fails (counters, large-magnitude metrics) |
   | `Bool(bool)`         | `Variant::Boolean(bool)`                                        |
   | `String(String)`     | `Variant::String(UAString)` — UTF-8 preserved                   |

   What you should NOT see post-migration: `Variant::String("Float")` (the pre-Epic-A discriminant-string symptom), or `BadDataUnavailable` for typed rows (only legacy rows should surface that status, and only for one poll interval).

4. **(Path A only) Legacy rows clear within one poll interval.** Re-Read the same metric variable a poll-interval later; if it previously returned `BadDataUnavailable`, it should now return the freshly UPSERTed typed payload.

### Rollback contract

The Epic A migration is **one-way**:

- v007 column additions cannot be cleanly dropped without breaking the post-Epic-A data integrity contracts that the rest of the codebase depends on (the typed columns are NOT NULL-able after v008's `CHECK` lands, and `MetricType` no longer carries a string representation).
- v008's exactly-one-non-NULL `CHECK` constraint is not in v006 and cannot be added to a v006-shaped database without first running v007.

**The only rollback path is to restore the pre-upgrade backup file** taken in step 1 of the [Pre-upgrade checklist](#pre-upgrade-checklist), then run the v2.0-rc binary against it:

```bash
# Stop the v2.0 gateway first.
cp /path/to/opcgw.db.pre-epic-a.bak /path/to/opcgw.db
# Restore the v2.0-rc binary (or revert the Docker image tag).
# Start the v2.0-rc gateway.
```

There is no in-tree rollback tool. **Take the backup BEFORE starting the upgrade** — the migration starts the moment you launch the v2.0 binary against a v006 database.

### SLA expectation

For **typical residential / small-scale deployments** (≤10k rows per table, ≤10MB total database size), the v007 + v008 migrations complete in **under 5 seconds**. This SLA is pinned by `src/storage/schema.rs::tests::test_v006_to_v008_full_upgrade_path_under_5s` and the per-migration siblings (`test_v007_migration_under_5s_for_10k_rows`, `test_v008_migration_under_30s_for_10k_rows`).

For **larger databases** (≥100MB, typically ≥500k historical rows), the v008 migration's `CREATE TABLE … AS SELECT` pattern is the dominant cost and scales roughly linearly with row count. **Expect a multi-minute startup delay on the first upgrade run** — the gateway will appear unresponsive (OPC UA port not yet bound) until the migration completes. Operators with large databases who cannot tolerate this delay should consider Path B (drop-and-recreate), running the migration in a maintenance window, or pre-validating against `test_v007_migration_under_5s_for_10k_rows`'s extrapolation on their hardware.

The auto-migration is **synchronous** before the OPC UA server binds its port — operator-visible behaviour is "the gateway takes longer than usual to start the first time after upgrading", which is expected and not a defect.

### Common gotchas

1. **Legacy rows look like "missing data" in the web dashboard and as `BadDataUnavailable` in OPC UA Reads — for one poll interval.** This is the documented contract from Stories A-4 / A-5 / A-6. Wait one poll cycle and the values populate. Pre-Epic-A rows are NOT broken data; they are "real value not yet captured under v008 schema". The web dashboard renders them as `—` with the "missing" badge (Story 9-3 + A-6 contract).

2. **The `metric_history` table is migrated the same way as `metric_values`.** Pre-Epic-A historical rows surface in OPC UA `HistoryRead` responses as `DataValue { value: None, status: BadDataUnavailable }` per the Story A-5 contract — they are **NOT silently dropped**. A SCADA client doing a 7-day HistoryRead against a freshly-migrated database will see a row stream where pre-Epic-A entries are flagged `BadDataUnavailable` and post-poll entries carry real typed payloads.

3. **New audit events may fire on first startup.** Epic A introduced five new `event="metric_*"` audit lines (`metric_parse`, `metric_read`, `metric_history_read`, `metric_history_summary`, `metric_view_serialize`); see `docs/logging.md` for the full taxonomy. If a v006 database happens to contain rows the v2.0 NaN/Inf filter would normally reject (a pre-A-3 era bug), the defensive guards may emit warn lines on first startup. These are operator-actionable but not blocking — investigate via the per-row `device_id` + `metric_name` in the log fields.

4. **`run_migrations()` is idempotent.** Re-running the v2.0 binary against a v007 or v008 database is a no-op (the `if current_version < N` guards in `src/storage/schema.rs` skip already-applied migrations). Pinned by `test_run_migrations_idempotent` at `src/storage/schema.rs:321`.

5. **v008's `CREATE TABLE … AS SELECT` is BEGIN/COMMIT-wrapped.** Per A-2-iter1-DEF-IH1 (user-confirmed deferral): an interrupted v008 migration (process killed mid-`CREATE TABLE`) leaves the database in a consistent pre-v008 state — the operator can simply restart the gateway to retry. The v007 → v008 pair therefore behaves transactionally for crash-safety even though the runner-level v001 → v008 chain is not atomic.

6. **Docker bind mounts preserve `opcgw.db` across container replacement.** Path A works transparently with the standard `docker compose pull && docker compose up -d` workflow because the database file lives on the host filesystem (per the `docker-compose.yml` volume mount). Path B requires removing the file from the host side (`rm ./opcgw.db ./opcgw.db-wal ./opcgw.db-shm` in the directory that's bind-mounted; avoid the `opcgw.db*` glob so unrelated backup files like `opcgw.db.bak.YYYY-MM-DD` are not removed).

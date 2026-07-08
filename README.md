<p align="center">
  <img src="docs/logo/opcgw-horizontal.svg" alt="opcgw — ChirpStack to OPC UA Gateway" width="400">
</p>

<p align="center">
  <a href="https://github.com/guycorbaz/opcgw/actions/workflows/ci.yml"><img src="https://github.com/guycorbaz/opcgw/actions/workflows/ci.yml/badge.svg" alt="build and test"></a>
  <img src="https://img.shields.io/badge/version-2.6.1-blue" alt="Version">
  <img src="https://img.shields.io/badge/license-MIT%2FApache--2.0-green" alt="License">
  <img src="https://img.shields.io/badge/arch-amd64%20%7C%20arm64-brightgreen" alt="Architectures">
</p>

# opcgw — ChirpStack to OPC UA Gateway

Bridge LoRaWAN device data from ChirpStack to industrial automation systems via OPC UA.

**opcgw** is a production-ready gateway that connects ChirpStack 4 LoRaWAN Network Server with OPC UA industrial clients, enabling seamless integration of wireless IoT devices into SCADA, MES, and industrial edge systems.

> 📖 **Full Documentation**: Visit the [GitHub Pages](https://guycorbaz.github.io/opcgw/) for detailed guides, architecture diagrams, and real-world use cases.

## Quick Links

- 🚀 [Quick Start Guide](https://guycorbaz.github.io/opcgw/quickstart/)
- 🏗️ [Architecture & Design](https://guycorbaz.github.io/opcgw/architecture/)
- ⚙️ [Configuration Reference](https://guycorbaz.github.io/opcgw/configuration/)
- 📋 [Features & Roadmap](https://guycorbaz.github.io/opcgw/features/)
- 💼 [Use Cases](https://guycorbaz.github.io/opcgw/usecases/)

## What is opcgw?

opcgw solves a critical integration challenge: connecting wireless LoRaWAN IoT networks managed by ChirpStack to industrial automation systems that speak OPC UA. 

**The Problem**:
- ChirpStack manages LoRaWAN devices but doesn't speak industrial protocols
- SCADA/MES systems expect OPC UA but don't understand LoRaWAN
- Building custom integrations is time-consuming and fragile

**The Solution**:
```
ChirpStack Server (LoRaWAN) ──→ opcgw Gateway ──→ OPC UA Clients (SCADA/MES)
   (gRPC polling)                (Rust, async)      (Ignition, KEPServerEx, etc.)
```

## Key Features

✨ **Real-Time Data Collection**
- Polls device metrics from ChirpStack at configurable intervals
- Supports multiple applications and hundreds of devices
- Handles network failures with automatic retries

🏭 **OPC UA Server**
- OPC UA 1.04 compliant server
- Dynamically builds address space from configuration
- Support for Float, Int, Bool, and String metrics
- Compatible with any standard OPC UA client

🔐 **Enterprise-Grade**
- Configuration validation on startup with clear error messages
- Environment variable credential management (no hardcoded secrets)
- Structured logging (tokio-tracing) for operational visibility
- Graceful shutdown handling
- Comprehensive error handling (no panics in production code)

🐳 **Container-Native**
- Official Docker image with multi-stage build
- Docker Compose for quick local development
- Kubernetes-ready with health checks

## Installation

### From Source (Rust 1.94.0+)

```bash
git clone https://github.com/guycorbaz/opcgw.git
cd opcgw
cargo build --release
./target/release/opcgw -c config/config.toml
```

### Via Docker (published image)

Pre-built images are published to two registries on every `v*` tag. Both registries receive identical multi-architecture manifests covering `linux/amd64` and `linux/arm64`.

**Docker Hub** (primary):

```bash
docker pull docker.io/gcorbaz/opcgw:2.1
```

**GitHub Container Registry** (mirror):

```bash
docker pull ghcr.io/guycorbaz/opcgw:2.1
```

The container runs as non-root user `opcgw` (UID 10001) and exposes port `4840` (OPC UA). Minimal `docker run` example:

```bash
docker run -d \
  --name opcgw \
  --restart unless-stopped \
  -p 4840:4840 \
  -e OPCGW_CHIRPSTACK__API_TOKEN='<your-chirpstack-api-token>' \
  -e OPCGW_OPCUA__USER_PASSWORD='<your-opc-ua-user-password>' \
  -v "$(pwd)/config:/usr/local/bin/config" \
  -v "$(pwd)/pki:/usr/local/bin/pki" \
  -v "$(pwd)/log:/usr/local/bin/log" \
  -v "$(pwd)/data:/usr/local/bin/data" \
  -e OPCGW_WEB__ENABLED=true \
  -p 8080:8080 \
  gcorbaz/opcgw:2.1
```

The `data/` bind mount is **required** so the SQLite database (and its 7-day metric history) survives container restarts. Without it, persisted metrics live in the ephemeral container layer and are destroyed on every `docker rm`.

#### First-run wizard (Epic C C-0; zero-touch in Story F-2)

**Story F-2 makes first boot zero-touch — no text-file editing required.** If you start the gateway from a fresh checkout (the shipped `config/config.toml` carries `REPLACE_ME_WITH_*` placeholders for both the ChirpStack token and the OPC UA password) and you have NOT set `OPCGW_CHIRPSTACK__API_TOKEN` / `OPCGW_OPCUA__USER_PASSWORD`, opcgw enters **first-run mode**: the OPC UA server rejects every authentication attempt while the web UI at `http://<host>:<web-port>/` serves a one-shot setup wizard at `/setup`. The wizard collects the **ChirpStack connection** (server address, tenant ID, API token) **and** the **OPC UA password**. On submit, opcgw will:

1. Persist the two secrets (ChirpStack `api_token` + OPC UA `user_password`) to `config/secrets.toml` (file mode `0600`, gitignored), in a single atomic write.
2. Persist the non-secret ChirpStack `server_address` + `tenant_id` to the gateway's SQLite database (secrets are never written to SQLite).
3. Signal a graceful shutdown.
4. Be restarted automatically by Docker / systemd (per your restart policy).
5. Boot in normal mode with the ChirpStack connection + password loaded via the figment provider stack (precedence: env-var > `secrets.toml`/SQLite > `config.toml`).

The wizard does **not** touch the web-UI login credentials or the log-file location — those stay in `.env` by design (see below). In first-run mode the validator carves out the missing ChirpStack/OPC UA credentials so the gateway no longer aborts before the wizard is reachable — this applies whether the credentials are **empty** or left as the shipped `REPLACE_ME_WITH_*` placeholders ([#146](https://github.com/guycorbaz/opcgw/issues/146)). Either way, while in first-run the OPC UA server rejects all authentication, so a placeholder password never becomes a live credential.

You can still skip the wizard by pre-filling `config/config.toml` (or the `OPCGW_*` env-vars) before first boot:

```toml
[chirpstack]
server_address = "http://127.0.0.1:8080"
api_token = "<your-token>"            # or set OPCGW_CHIRPSTACK__API_TOKEN, or use the wizard
tenant_id = "<your-tenant-id>"

[opcua]
application_name = "Chirpstack OPC UA Gateway"
pki_dir = "./pki"
# host_port defaults to 4840 (standard OPC UA port); set it to change.
# user_name defaults to "opcua-user".
# user_password is collected by the /setup wizard on first boot (or set
# OPCGW_OPCUA__USER_PASSWORD).

[web]
enabled = true
bind_address = "0.0.0.0"
port = 8080
# Web UI auth reuses [opcua].user_name / user_password — there is no separate
# admin account.
```

The wizard is one-shot: subsequent `/setup` requests return HTTP 410 Gone. To rotate a secret later, either override via the `OPCGW_CHIRPSTACK__API_TOKEN` / `OPCGW_OPCUA__USER_PASSWORD` env-vars or hand-edit `config/secrets.toml` and restart.

**After the first boot, you no longer edit `config.toml`.** opcgw reads it once to seed its SQLite database, then ignores it. From then on, manage everything from the web UI: the singleton-configuration editor (`[global]` / `[chirpstack]` / `[opcua]` / `[web]`), the ChirpStack inventory pickers that map applications/devices/metrics by name, and the drift view that diffs your configured inventory against ChirpStack. See the [Configuration](#configuration) section below.

Bind-mounted host directories must be owned by UID 10001 before first start (the container can't `chown` host files):

```bash
sudo mkdir -p ./config ./pki/{own,private,trusted,rejected} ./log ./data
sudo chown -R 10001:10001 ./config ./pki ./log ./data
sudo chmod 700 ./pki/private
# Tighten any pre-existing private-key files (no-op on a fresh install):
sudo find ./pki/private -type f -name '*.pem' -exec chmod 600 {} +
```

### Via `docker compose`

```bash
git clone https://github.com/guycorbaz/opcgw.git
cd opcgw
cp .env.example .env
chmod 600 .env
# Edit .env to set OPCGW_CHIRPSTACK__API_TOKEN + OPCGW_OPCUA__USER_PASSWORD
docker compose up -d
```

`docker-compose.yml` loads **all** environment variables from `.env` via `env_file`, so `.env` is the single place to set every `OPCGW_*` variable — nothing is configured in the compose file itself. See [`.env.example`](./.env.example) for the template and the user manual's *Environment Variable Reference* appendix for the full list; see [`docs/security.md`](./docs/security.md) for the secret-management contract.

### Documentation

The complete operator-facing user manual is authored in **LaTeX** under [`docs/manual/latex/`](./docs/manual/latex/) (edit `body.tex` for content, `preamble.tex` for styling). Build the polished PDF via the bundled Makefile:

```bash
cd docs/manual
make pdf      # → docs/manual/latex/opcgw-user-manual.pdf
```

Prerequisites: a LuaLaTeX TeX Live install plus `graphviz` and `mupdf-tools` for the figures. See [`docs/manual/README.md`](./docs/manual/README.md) for build details. (The manual was migrated from DocBook to LaTeX in 2026-06; LaTeX is now the canonical format.)

Additional reference docs:

- [`docs/deployment-guide.md`](./docs/deployment-guide.md) — runbook + Epic A migration procedure
- [`docs/c-6-migration-runbook.md`](./docs/c-6-migration-runbook.md) — Story C-6 TOML→SQLite one-shot migration runbook + rollback procedure
- [`docs/security.md`](./docs/security.md) — credential management, PKI, audit-event recipes
- [`docs/logging.md`](./docs/logging.md) — structured-log taxonomy
- [`CHANGELOG.md`](./CHANGELOG.md) — release notes

## Configuration

As of v2.1.0, opcgw stores its configuration in SQLite and is managed from the web UI. On first boot it seeds that database from a TOML file (read once, then ignored at runtime). A bootstrap `config.toml` looks like:

```toml
[chirpstack]
server_address = "http://chirpstack.local:8080"
api_token = "your-api-token"
tenant_id = "your-tenant-id"
polling_frequency = 10
# Story C-1: server-side TTL cache for /api/inventory/{applications,devices}.
# Default 60. Set to 0 to disable caching (every inventory request hits
# ChirpStack — useful for development). Env-var override:
# OPCGW_CHIRPSTACK__INVENTORY_CACHE_TTL_SECONDS. Restart-required.
inventory_cache_ttl_seconds = 60
# Story C-1: max wait window for /api/inventory/uplinks (bounded read
# against InternalService.StreamDeviceEvents). Default 5; range 1..=60.
# Env-var override: OPCGW_CHIRPSTACK__INVENTORY_UPLINK_MAX_WAIT_SECONDS.
inventory_uplink_max_wait_seconds = 5

[opcua]
application_name = "My IoT Gateway"
host_port = 4840
user_name = "admin"
user_password = "secure-password"

[[application]]
application_name = "Farm Sensors"
application_id = "1"

[[application.device]]
device_name = "Field A"
device_id = "sensor_001"

[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "soil_moisture"
metric_type = "Float"
metric_unit = "%"
```

> **Configuration lives in SQLite (v2.1.0).** The TOML above is a **bootstrap seed**: on first boot opcgw migrates it into its SQLite database and thereafter ignores `config.toml` at runtime. Manage configuration from the web UI instead — applications/devices/metrics/commands via the **Configuration** drill-down (`/config.html`, an Application → Device → Metrics/Commands hierarchy with ChirpStack inventory pickers), and the singleton sections (`[global]`, `[chirpstack]`, `[opcua]`, `[web]`) via the singleton-configuration editor (Epic D). A few `[opcua]` / `[web]` knobs only take effect after a gateway restart; the editor flags those. See `docs/c-6-migration-runbook.md` (applications/devices) and `docs/d-0-migration-runbook.md` (singleton sections).

**For complete configuration details**, see the [Configuration Reference](https://guycorbaz.github.io/opcgw/configuration/).

> 🔐 **Secrets:** the shipped `config/config.toml` ships with placeholder
> values for `api_token` and `user_password`. The gateway refuses to
> start with the placeholders in place — inject the real values via the
> `OPCGW_CHIRPSTACK__API_TOKEN` and `OPCGW_OPCUA__USER_PASSWORD` env vars
> (or via the `.env` file consumed by Docker Compose). See
> [`docs/security.md`](./docs/security.md) for the full env-var
> convention, Docker / Kubernetes recipes, and the migration path for
> existing deployments.
>
> 🛡️ **OPC UA security:** for production OPC UA setup (endpoint matrix,
> PKI layout, `0o600` private-key requirement, audit-trail recipe,
> anti-patterns), see
> [`docs/security.md#opc-ua-security-endpoints-and-authentication`](./docs/security.md#opc-ua-security-endpoints-and-authentication).
> For OPC UA session-cap sizing (`max_connections` knob, the
> `event="opcua_session_count"` gauge, and the at-limit warn), see
> [`docs/security.md#opc-ua-connection-limiting`](./docs/security.md#opc-ua-connection-limiting).
> For subscription / message-size limits (`max_subscriptions_per_session`,
> `max_monitored_items_per_sub`, `max_message_size`, `max_chunk_count`)
> and the `DataChangeFilter` contract for stale-status notifications,
> see [`docs/security.md#subscription-and-message-size-limits`](./docs/security.md#subscription-and-message-size-limits).
> For OPC UA `HistoryRead` (FR22) — `[storage].retention_days` floor /
> hard-cap, the `[opcua].max_history_data_results_per_node` per-call cap,
> the manual-paging recipe (continuation points are not implemented), and
> NFR15's <2s 7-day query budget — see
> [`docs/security.md#historical-data-access`](./docs/security.md#historical-data-access).
> For the embedded **Web UI** (Story 9-1; FR50, NFR11, NFR12, FR41) — the
> `[web]` config block (port / bind_address / auth_realm / enabled —
> defaults to off), Basic auth shared with `[opcua]` credentials, the
> `event="web_auth_failed"` audit event with direct source-IP via Axum's
> `ConnectInfo`, and the TLS-via-reverse-proxy stance — see
> [`docs/security.md#web-ui-authentication`](./docs/security.md#web-ui-authentication).

### Web UI

The embedded Axum web server is the primary way to configure and monitor
opcgw, and is **enabled by default** in the shipped `config/config.toml`
(the binary's built-in default is off; disable with `enabled = false` or
`OPCGW_WEB__ENABLED=false`). It is gated by HTTP Basic auth:

```toml
[web]
enabled = true              # shipped default true; binary default false
port = 8080                 # default 8080; range 1024-65535
bind_address = "0.0.0.0"    # default "0.0.0.0"
auth_realm = "opcgw"        # default "opcgw"; max 64 chars
```

Credentials are **shared with `[opcua]`** (same `user_name` /
`user_password`); one rotation step covers both auth surfaces. HTTP-only
— deploy a reverse proxy (nginx, Caddy, Traefik) for TLS termination if
your environment requires it (GH #104). See
[`docs/security.md#web-ui-authentication`](./docs/security.md#web-ui-authentication).

The web UI hosts the first-run setup wizard, the **singleton-configuration
editor** (`[global]` / `[chirpstack]` / `[opcua]` / `[web]`), full
application/device/metric CRUD with **ChirpStack inventory pickers**, the
**inventory drift view**, plus the read-only monitoring views described
below (status dashboard + live metric values).

**The gateway status dashboard (Story 9-2, redesigned in Story F-3).** Once
enabled, browse to `http://<gateway-host>:8080/` for a single-screen view of
gateway health. **F-3 leads with an at-a-glance health verdict** — a banner
that reads *All systems operational* or a specific degraded reason (ChirpStack
unreachable, poller stalled, an apply that failed, devices going stale, staged
changes pending) — plus tiles for ChirpStack state, **poller status** (active
vs stalled, judged against the configured poll interval), last-poll timestamp,
cumulative error count, application/device counts, and uptime, and a
**device-data-freshness** panel (counts of devices that are fresh / stale / bad
/ never reported). The verdict and the freshness counts are derived
**client-side** from the existing `GET /api/status` and `GET /api/devices`
payloads — the gateway computes no new aggregates (the #130 no-aggregation
rule); `error_count` is cumulative-since-startup, and there is no per-error
list (that would need a new event store). The dashboard auto-refreshes every
10 seconds (hard-coded in `static/dashboard.js`). Mobile-responsive layout and
OS-driven dark mode (`prefers-color-scheme: dark`) ship out of the box. The
JSON shape is exposed as `GET /api/status` (now including `poll_interval_secs`)
for `curl | jq` / Prometheus textfile exporters / custom dashboards (auth-gated
identically to the HTML).

**Story 9-3 ships the live metric values page.** Browse to
`http://<gateway-host>:8080/metrics.html` for the per-application,
per-device grid of current metric values. Each row shows the metric
name, value, data type, last-updated timestamp, and a staleness badge
(`Good` / `Uncertain` / `Bad` / `Missing`) computed from the
`[opcua].stale_threshold_seconds` knob (default 120 s) and the
hard-coded 24 h "bad" cutoff. Configured-but-never-polled metrics
appear as "Missing" rather than being silently omitted — operators
spot mis-configured devices at a glance. Same 10 s refresh + responsive
layout + dark mode as the dashboard. JSON contract at `GET /api/devices`.

**Story F-4 adds config export / import.** From the singleton-configuration
page, **Export config** downloads the whole configuration — the `[global]` /
`[chirpstack]` / `[opcua]` / `[web]` sections plus the application/device/metric/
command tree — as a portable TOML file (`GET /api/config/export`). **Secrets
are never included** (`api_token` / `user_password` are stripped). **Import
config** uploads a previously exported file: the browser reads it client-side
and POSTs it to `POST /api/config/import`, which merges it over the running
configuration (so this instance keeps its own secrets — import never carries or
overwrites them), validates it, and **stages** it. Click **Apply changes** to
activate the imported config (one soft restart — the container is never
restarted). Useful for backups, versioning, and reproducing a setup on another
instance.

## Planning

> **Status:** the payload-less metric-storage bug ([#108](https://github.com/guycorbaz/opcgw/issues/108)) was resolved by **Epic A** — opcgw now persists and serves real measurement values end-to-end (OPC UA Read / HistoryRead and the web dashboard all return typed values, not type-name strings). The gateway is suitable for production measurement collection.

**Current Version:** 2.6.1 (stable; Docker `:2.6.1` / `:2.6` / `:latest`) — a patch release fixing a storage-latency budget regression ([#149](https://github.com/guycorbaz/opcgw/issues/149)): `batch_write_metrics` was checked against the generic 250ms storage-query budget instead of its own 2000ms batch-write budget ([#144](https://github.com/guycorbaz/opcgw/issues/144)), producing ~700 misleading `exceeded_budget=true` WARNs/day in production. Found during the Ignition SCADA go-live soak-check log review on panoramix. Pure observability fix — no functional or data-loss impact. The prior release: **2.6.0** — the first slice of the **web UI refresh (Epic I)**, promoted from `v2.6.0-rc1` after a clean multi-day soak on the panoramix NAS (~42 h continuous uptime, zero restarts, zero ERROR/WARN-level lines Jul 2–4, no panics, OPC UA steadily serving on `:4855`). A **partial-epic release** delivering three of five stories — pure presentation with **no `/api`, write-model, or behavioural change**, and still **no build step / framework / `node_modules`** (hand-written vanilla CSS on the F-1 shell): **I-1** design-token foundation (`static/dashboard.css` refactored onto `:root` CSS custom properties in a ChirpStack-v4 / Ant Design palette with token-driven dark mode) and **I-2** navigation & shell refresh (fixed navy left sider on desktop, accessible hamburger-drawer app-bar on mobile, page titles in a light top strip; the `has-shell` gate keeps the first-run wizard unaffected). Served-HTML DOM-ID markers + G-2 field-help accessibility preserved; WCAG AA verified light and dark. **I-3** (component refresh) and **I-4** (cross-page rollout + QA) remain for a later cut. The prior stable line: **2.5.2** (Docker `:2.5.2` / `:2.5`) — a patch release carrying the **Epic H / H-0 async storage facade** ([#73](https://github.com/guycorbaz/opcgw/issues/73)): the synchronous `StorageBackend` was being called directly from ~30 async tokio call sites, blocking worker threads on SQL and on the pool-retry `std::thread::sleep` backoffs — survivable on the multi-threaded runtime but degrading on CPU-constrained deployments. All such calls now run off the async workers via a `spawn_blocking` facade; genuinely-synchronous OPC UA callbacks use a `block_in_place`-safe helper. No behavioural change (identical return types, error mapping, ordering). Because it touches the data-plane hot paths, it was promoted only after the real-binary smoke/soak on the NAS (per the AI-G-5 / main-deadlock-incident doctrine — runtime regressions evade unit tests + review). Earlier: **2.5.1** (stable; Docker `:2.5.1` / `:2.5` / `:latest`) — a patch release fixing a first-run onboarding regression ([#146](https://github.com/guycorbaz/opcgw/issues/146)) that shipped in v2.5.0: a fresh clone following the quickstart aborted on the shipped `REPLACE_ME_WITH_*` OPC UA password placeholder instead of booting the `/setup` wizard (the validator only carved out an *empty* password, not the shipped *placeholder*). Found by the post-release real-world onboarding smoke; the fix also hardens the OPC UA auth gate so the placeholder can never become a live credential in first-run. **v2.5.0** **completes Epic G, Web UX & Usability** on top of v2.4.0's drill-down config (G-0): **G-1** device-profile metric picker (choose metrics from the ChirpStack device profile, not only observed uplinks), **G-2** contextual field help (accessible inline help on every config field), **G-3** per-device OPC UA stale threshold (settable from the web UI), and **G-4** dashboard error drill-down (the error count links to a list of the actual recent errors). Epic G security review CLEAN (0 HIGH / 0 MED / 2 LOW). Earlier in the 2.3 line: **v2.3.0** delivered **Epic F — onboarding & web UX for public release** (zero-touch first-run wizard, unified vanilla web shell, staged "Apply changes" config model, redesigned dashboard); **v2.3.1** consolidated logging to a single retention-capped daily file ([#143](https://github.com/guycorbaz/opcgw/issues/143)); **v2.3.2** adds storage hardening — a startup integrity check for the `metric_history` index ([#74](https://github.com/guycorbaz/opcgw/issues/74)) and configurable, NAS-realistic storage-latency WARN budgets (env `OPCGW_STORAGE_QUERY_BUDGET_MS` / `OPCGW_BATCH_WRITE_BUDGET_MS`, [#144](https://github.com/guycorbaz/opcgw/issues/144)). Earlier: **v2.1.0** shipped web-first configuration & auto-discovery (Epic C wizard/pickers/drift + TOML→SQLite migration; Epic D SQLite singleton config + web editor); **v2.2.0** brought **Epic E — the model-agnostic, class-aware device-abstraction layer** (downlink command path end-to-end with real Tonhe E20 valve actuation, uplink-event ingestion with raw last-known values and no gateway-side aggregation ([#130](https://github.com/guycorbaz/opcgw/issues/130)), optional per-device `stale_threshold_seconds` ([#132](https://github.com/guycorbaz/opcgw/issues/132))). See [`CHANGELOG.md`](./CHANGELOG.md) for full release notes.

The roadmap is tracked in [`_bmad-output/implementation-artifacts/sprint-status.yaml`](./_bmad-output/implementation-artifacts/sprint-status.yaml). The table below mirrors the current state of every epic; story-level detail lives in the sprint status file and the per-story documents under `_bmad-output/implementation-artifacts/`.

| Epic | Status | Scope |
|------|--------|-------|
| **Epic 1 — Crash-Free Gateway Foundation** | ✅ done | Dependency refresh + Rust 1.94, `log4rs → tracing` migration, comprehensive error handling, graceful shutdown via `CancellationToken`, configuration validation. |
| **Epic 2 — Data Persistence** | ✅ done | `StorageBackend` trait, SQLite backend with WAL mode + per-task connection pool, batch writes, append-only history table, startup restore, graceful degradation, retention pruning. |
| **Epic 3 — Reliable Command Execution** | ✅ done | SQLite-backed FIFO command queue, parameter validation, command-delivery status reporting (sent / confirmed / failed / timed-out). |
| **Epic 4 — Scalable Data Collection** | ✅ done | Poller refactored onto `StorageBackend`, support for all ChirpStack metric types, gRPC pagination. **4-4 done (2026-05-06 — auto-recovery from ChirpStack outages; Phase A carry-forward closed after iter-3 code review tightened shipped `config/config.toml` defaults from `retry=10, delay=10` to `retry=30, delay=1` to satisfy AC#4's `retry × delay ≤ 30s` clause + 4 supporting patches).** Closes PRD FR6 (TCP connectivity check), FR7 (auto-reconnect without manual intervention), FR8 (configurable retry count + delay), and NFR17 (30s auto-recovery SLA). New `recover_from_chirpstack_outage` helper on `ChirpstackPoller` (~120 LOC at `src/chirpstack.rs`) layered on top of the existing `chirpstack_outage` warn (Story 6-3): reads `chirpstack.retry` + `chirpstack.delay` at loop entry, calls `update_gateway_status(None, error_count, false)` to surface the outage to OPC UA (Story 5-3) + web dashboard (Story 9-2), then probes `check_server_availability` up to R times with cancel-token-paired `tokio::time::sleep` budget (Ctrl+C aborts cleanly). Three reserved log operations from `docs/logging.md:240-242` promoted to implemented: `recovery_attempt` (info per attempt), `recovery_complete` (info on success with `downtime_secs` from `last_successful_poll` math, or `from_startup=true` on cold start), `recovery_failed` (warn on budget exhaustion). Picked up the 6-3 carry-forward from `deferred-work.md:86`: `Channel::connect()` now wrapped in a 5s `tokio::time::timeout` (smaller than NFR17's 30s SLA, larger than the TCP probe's 1s timeout). 7 new unit tests in `src/chirpstack.rs::tests::recovery_*` covering AC#1 (loop fires + emits operations), AC#2 (retry count + delay), AC#3 (gateway_status update with `chirpstack_available=false` + `error_count` propagation), AC#5 (cancel-safety), AC#6 (downtime_secs + cold-start `from_startup=true`). Existing Story 5-2 stale-data semantics + Story 7-2/7-3/8-3/9-0/9-1/9-2/9-3 invariants preserved (zero changes to `src/web/`, `static/`, `tests/web_*.rs`, `src/opc_ua_*.rs`). |
| **Epic 5 — Operational Visibility** | ✅ done | OPC UA server refactored onto SQLite backend, stale-data detection with OPC UA `Good`/`Uncertain`/`Bad` status codes, gateway health metrics (last poll timestamp, error count, ChirpStack availability) exposed under the `Gateway` folder. |
| **Epic 6 — Production Observability & Diagnostics** | ✅ done | **6-1 done** (structured logging, correlation IDs on every OPC UA read, staleness-transition logs, poller-cycle structured logs, storage-query timing, configurable log directory via `OPCGW_LOG_DIR` and `[logging].dir`); **6-2 done** (configurable log verbosity via `OPCGW_LOG_LEVEL` and `[logging].level`); **6-3 done** (microsecond UTC timestamps; performance-budget warnings on `opc_ua_read`/`storage_query`/`batch_write`; data-anomaly logs — NULL `last_poll_timestamp`, staleness-boundary, error-count spike; ChirpStack `chirpstack_connect` / `chirpstack_outage` / `retry_schedule` diagnostics; edge-case logs — `gateway_status_init`, `chirpstack_request` timeout, `metric_parse`; transient-failure logs — `device_poll`, SQLITE_BUSY (with `sqlite_error_code` sibling field for differentiating BUSY/LOCKED); end-to-end `request_id` correlation verified via integration test; expanded operations reference + symptom cookbook in `docs/logging.md`. Code review complete in 3 iterations: clippy-clean across the workspace, `Mutex<HashMap>` staleness cache replaced with `DashMap` for lock-free concurrent reads, 5 helpers extracted (`maybe_emit_error_spike`, `maybe_emit_chirpstack_outage`, `validate_bool_metric_value`, `classify_and_log_grpc_error`, `format_last_successful_poll`) so synthetic tests now drive production paths; 188 lib + 209 bin + 79 integration tests pass). |
| **Epic 7 — Security Hardening** | ✅ done | **7-1 done** — sanitized `config/config.toml` with `REPLACE_ME_WITH_*` placeholders + placeholder-detection in `validate()`, hand-written redacting `Debug` impls on `ChirpstackPollerConfig` / `OpcUaConfig`, `secrets_not_logged_when_full_config_debug_formatted` regression test, tonic 0.14.5 metadata-leak audit (clean — no `EnvFilter` mitigation needed today), `.env.example` + Compose recipe with `:?err` guards, `docs/security.md` with reversible-migration alternative, scrubbed `config/config.example.toml` (synthetic IDs only). **7-2 done** — `OPCUA_USER_TOKEN_ID = "default-user"` constant replacing the hardcoded `"user1"` token id (4 call sites), custom `OpcgwAuthManager` (`src/opc_ua_auth.rs`) implementing async-opcua's `AuthManager` trait with **HMAC-SHA-256-keyed credential digests** for fully-constant-time comparison (no length oracle) and sanitised-username `event="opcua_auth_failed"` audit logging (NFR12 via two-event correlation with async-opcua's accept event); `validate_private_key_permissions` (NFR9 — `0o600` file + `0o700` parent dir enforced at startup with both-violation accumulation, fail-closed) and `ensure_pki_directories` (FR45 — auto-creates `own/`, `private/`, `trusted/`, `rejected/` race-free via atomic `DirBuilder::mode()`) in new `src/security.rs` module; path-traversal guards rejecting absolute / `..` `private_key_path` and empty `pki_dir`; trim checks on `user_name` / `user_password` for copy-paste-from-`.env` resilience; shipped `create_sample_keypair = false` in both `config/config.toml` and `config/config.example.toml`, release-build warning when the flag is `true`; integration tests pinning the three endpoints (None / Basic256 Sign / Basic256 SignAndEncrypt) with line-scoped audit-event assertions and the wrong-password rejection path; smoke-test client at `examples/opcua_client_smoke.rs`; `async-opcua-client` moved to `[dev-dependencies]` to keep the production binary lean; extended `docs/security.md` with endpoint matrix + PKI layout + audit-trail recipe + log-level-required-for-NFR12 hard-statement + create-sample-keypair regen anti-pattern + Story-7-1 migration path. Code review closed all HIGH/MEDIUM findings over three iterations. **7-3 done** — `[opcua].max_connections: Option<usize>` config knob (default 10, hard cap 4096) wired through `ServerBuilder::max_sessions(N)` for FR44; `OPCGW_OPCUA__MAX_CONNECTIONS` env-var override; new `src/opc_ua_session_monitor.rs` module exposing a periodic `info!(event="opcua_session_count", current, limit)` gauge (5s tick) and an `AtLimitAcceptLayer` tracing-Layer that emits `warn!(event="opcua_session_count_at_limit", source_ip, limit, current)` correlated against async-opcua's `Accept new connection from {addr}` event (NFR12 two-event pattern); 8 unit tests + 4 integration tests; 581 tests / 0 fail / 7 ignored, clippy clean; extended `docs/security.md` with `## OPC UA connection limiting` section. **Epic 7 retrospective complete (2026-04-29)** — Phase A security baseline (FR42, FR44, FR45, NFR7-9, NFR12, NFR24) satisfied. Decided next steps before `bmad-create-story 8-1`: NFR12 silent-degradation startup-warn in `src/main.rs::initialise_tracing` (~5 LOC) + Epic 8 spec polish in `epics.md` (incorporates per-IP throttling #88, message-size / monitored-item limits #89, gauge tunability, NFR12 logging hard-statement). Eight follow-up GitHub issues (#82, #83, #85–#90) carry forward; none block Phase B entry. Doctest baseline (56 pre-existing failures) escalates to a defined cleanup story before Epic 9. See [`epic-7-retro-2026-04-29.md`](./_bmad-output/implementation-artifacts/epic-7-retro-2026-04-29.md). |
| **Epic 8 — Real-Time Subscriptions & Historical Data (Phase B)** | ✅ done (8-1 / 8-2 / 8-3 shipped; 8-4 descoped from Epic 8 on 2026-05-14 and tracked as a Known Failure — see retro § Known Failures + 2026-05-14 descope addendum) | **Epic 8 retrospective complete (2026-05-01)** — Phase B subscription + historical-data baseline (FR21, FR22) satisfied. Story 8-4 (threshold-based alarm conditions, FR23) classified as Known Failure (KF) with operator-visible diagnostics treatment per Story 6-3 convention; SCADA-side alarm thresholds in FUXA / Ignition is the documented operator workaround. Key decided action items before Epic 9 starts: NodeId metric-name-only collision bug fix (HIGH severity, pre-existing latent bug 8-3 surfaced via HistoryRead), `test_concurrent_write_read_isolation` flake fix (`#[serial_test::serial]`), doctest cleanup story (BLOCKING — 4th epic in a row), spike-test productionisation, `tests/common/mod.rs` extraction, CLAUDE.md per-iteration commit-rule clarification. Carry-forward GH issues #88, #93, #94, #95, #98. See [`epic-8-retro-2026-05-01.md`](./_bmad-output/implementation-artifacts/epic-8-retro-2026-05-01.md). **8-1 / 8-2 / 8-3 done.** **8-3 done (2026-04-30 after iter-1 + iter-2 code review)** — OPC UA `HistoryRead` (FR22) end-to-end: new `StorageBackend::query_metric_history` method on `SqliteBackend` + `InMemoryBackend` with half-open `[start, end)` interval semantics (start inclusive, end exclusive — matches OPC UA Part 11 §6.4), microsecond-precision UTC timestamps, partial-success on bad rows (NaN / unknown data_type silently skipped with `trace!`); new `src/opc_ua_history.rs` module wrapping async-opcua's `SimpleNodeManagerImpl` and overriding `history_read_raw_modified` (the wrap-don't-subclass pattern documented in Story 8-1's spike report); reverse-lookup `NodeId → (device_id, metric_name)` map built once at server-construction time; metric variables now carry `AccessLevel::HISTORY_READ` + `historizing = true`; new `[storage].retention_days` validation (FR22 floor of 7 days, hard cap 365) + new `[opcua].max_history_data_results_per_node` per-call cap (default 10000, hard cap 1_000_000) — both wired through env-var overrides; the configured retention is now written to `retention_config` at startup via `INSERT OR REPLACE` (was migration-default 90 days, now operator-config-driven); **continuation points NOT implemented** (manual-paging contract documented in `docs/security.md`); NFR15 release-build benchmark in `tests/opcua_history_bench.rs` (`#[ignore]` by default; targets 600k-row 7-day query <2s); NFR12 carry-forward intact (zero new audit events, src/opc_ua_auth.rs / src/opc_ua_session_monitor.rs production code unchanged); 11 unit tests on `query_metric_history` + 11 config-validation tests + 5 integration tests on the HistoryRead pipeline. See [`8-3-historical-data-access-via-opc-ua.md`](./_bmad-output/implementation-artifacts/8-3-historical-data-access-via-opc-ua.md) for the spec; the new docs section is `docs/security.md#historical-data-access`. **8-4** (threshold-based alarm conditions, FR23) **classified as Known Failure** — see retro § Known Failures + `deferred-work.md`. |
| **Epic 9 — Web Configuration & Hot-Reload (Phase B)** | ✅ done (closed 2026-05-14 — all 9 stories 9-0 through 9-8 done + retrospective complete; production deployment remains blocked on issue [#108](https://github.com/guycorbaz/opcgw/issues/108), to be addressed by Epic A — Storage Payload Migration) | **9-8 review (2026-05-13 — Implementation + 5 integration tests complete via bmad-dev-story; awaiting code review)** — Dynamic OPC UA Address Space Mutation (FR24). Closes Story 9-7's documented v1 limitation. New `src/opcua_topology_apply.rs` module hosts `compute_diff` + `apply_diff_to_address_space` (4-phase mutation envelope). Story 9-7 stub seam wired. 2 new audit events. Critical empirical Q2-mitigation refinement: `value: Some(Variant::Empty)` sentinel forces DataChange notification under async-opcua's default value-only filter. Task 6 option (b) — callback registry private upstream, stub functions emit one-time deferred-leak info log. AC#9 strict-zero file invariants honoured. 5 integration tests in `tests/opcua_dynamic_address_space_apply.rs` against a real OPC UA server. Recommend running bmad-code-review on a different LLM per CLAUDE.md doctrine. **9-6 done (2026-05-12 — Implementation complete via bmad-dev-story, all Tasks 0-11; awaiting code review)** — Command CRUD via Web UI (FR36, FR40, FR41). Five new endpoints under `/api/applications/:application_id/devices/:device_id/commands*` (GET list/by-id, POST, PUT, DELETE) closing the FR34/35/36 cluster. `:command_id` path segment is `i32` parsed by new `validate_path_command_id` helper (rejects non-numeric/≤0 with `event="command_crud_rejected" reason="validation" field="command_id"`). CSRF literal-arm completion in `src/web/csrf.rs` (adds `"command" =>` warn arm at both rejection-emission match blocks per the explicit Story 9-6 hand-off comment); `validate_path_device_id` widened with `resource: &'static str` parameter (parallel to iter-3 Blind#3 pattern for `find_application_index`) — all 9-5 device-handler call sites pass `"device"` byte-for-byte. New `validate_command_name` / `validate_command_port` (delegates to `DeviceCommand::validate_f_port` enforcing LoRaWAN 1..=223) / `validate_command_id_value` / `command_not_found_response` / `build_command_table` helpers in `src/web/api.rs`. `AppConfig::validate` additive amendment for per-device `command_id` + `command_name` uniqueness HashSets (modelled on Story 9-5's `seen_metric_names` pattern; cross-device same-`command_id` remains allowed per device-folder-NodeId-namespace argument). TOML mutation preserves sibling `[[application.device.read_metric]]` byte-for-byte (regression guard for Story 9-5); DELETE-last-command leaves empty `command` `ArrayOfTables` in place (Task 6 pinned decision — verified by `delete_last_command_leaves_clean_toml_round_trip` test). 4 new audit events: `command_created` / `command_updated` / `command_deleted` (info) + `command_crud_rejected` (warn) with 1 new reason value `command_not_found`. AC#10 strict-zero file invariants: `src/web/auth.rs`, `src/opc_ua.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing` untouched. NEW `static/commands.html` + `static/commands.js` (vanilla JS, no framework, no build step); Commands nav links added to `static/applications.html` + `static/devices-config.html` + `static/devices.html`. NEW `tests/web_command_crud.rs` with 45 integration tests covering AC#1-12 + AC#11 cross-resource regression suite (Stories 9-4 + 9-5 grep contracts preserved); 7 new unit tests in `src/web/api.rs::tests` (3 `validate_path_command_id` + 2 `validate_path_device_id` widening + 1 already-existing widened); 2 new unit tests in `src/web/csrf.rs::tests`; 3 new unit tests in `src/config.rs::tests` (per-device command_id/command_name uniqueness + cross-device-allowed). docs/logging.md +4 command_* rows + path-aware dispatch note; docs/security.md "Command CRUD (Story 9-6)" subsection covering endpoint surface, CSRF, validate-side amendments, audit events, v1 limitations, anti-patterns. No new dependencies — `scopeguard` crate is NOT a dep (chmod-cleanup uses hand-rolled Drop-impl RAII guard per Story 9-5 `tests/web_device_crud.rs:1578` precedent). Final grep contracts intact: `command_*` = 4, `device_*` = 4, `application_*` = 4, `config_reload_*` = 3. Task 0 (open tracking GH issue) deferred to user — gh CLI not authenticated for write per Stories 9-4 / 9-5 precedent. **9-7 done (2026-05-06 — 3 code-review iterations terminated under CLAUDE.md condition #2)** — Configuration Hot-Reload (FR39 + FR40). New `src/config_reload.rs` module owns the `tokio::sync::watch::Sender<Arc<AppConfig>>` channel, the `ConfigReloadHandle::reload()` routine (validate-then-swap discipline), and the knob-taxonomy classifier that distinguishes hot-reload-safe from restart-required from address-space-mutating changes. `src/main.rs` spawns a SIGHUP listener (3 new audit events: `event="config_reload_attempted"` info, `"config_reload_succeeded"` info with `changed_section_count` + `duration_ms`, `"config_reload_failed"` warn with `reason ∈ {validation, io, restart_required}` + `changed_knob`), a web-config-listener task that atomically swaps `AppState.dashboard_snapshot` + `AppState.stale_threshold_secs` (now `RwLock<Arc<...>>` + `AtomicU64`), and an OPC UA config-listener stub for Story 9-8 that logs `topology_change_detected` with device counts. Poller's outer-loop `tokio::select!` gains a `config_rx.changed()` arm; Story 4-4's recovery loop unaffected (read-at-entry semantics naturally pick up new `retry`/`delay` values). AC#8 invariants honoured: zero changes to `src/web/auth.rs`, `src/opc_ua.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing`. v1 limitations explicitly documented in `docs/security.md § Configuration hot-reload`: (1) OPC UA address-space mutation stubbed (Story 9-8 territory — the dashboard updates but OPC UA stays frozen), (2) credential rotation restart-required (auth-middleware refactor deferred), (3) `[opcua].stale_threshold_seconds` hot-reload affects only the web dashboard (OPC UA per-variable closures captured at startup), (4) no HTTP trigger (Stories 9-4/9-5/9-6 will add CRUD-driven reload), (5) no filesystem watch. 13 new integration tests in `tests/config_hot_reload.rs` covering AC#1 (validation-first / atomic swap), AC#2 (hot-reload-safe propagation), AC#3 (restart-required rejection with `changed_knob`), AC#4 (dashboard reflection), AC#9 (loose-perm rejection + secret-hygiene), AC#10 (stale-threshold propagation), plus io-reason path and equal-config NoChange. 6 new `src/config_reload.rs::tests` unit tests + 1 new `src/chirpstack.rs::tests::poller_picks_up_new_retry_at_next_cycle` + 3 new clamp-helper unit tests. AC#7 grep contract: `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` returns exactly 3 lines. Issue #110 (RunHandles missing Drop impl) verdict: keep open — listener tasks all cooperate with `cancel_token` explicitly via `tokio::select!`, RAII drop would be redundant. Tracking issue #112. **9-1 done (2026-05-02 after iter-1 + iter-2 code review)** — Axum 0.8 embedded web server gated by HTTP Basic auth (FR50 / NFR11), `event="web_auth_failed"` warn-level audit event with **direct** source-IP via Axum's `ConnectInfo<SocketAddr>` (NFR12 — strict improvement over the OPC UA path's two-event correlation), `tower-http::services::ServeDir` static-file mount with placeholder HTML for Stories 9-2+ (FR41 mobile-responsive viewport tag in place), `[web].enabled = false` master switch (opt-in to avoid surprise listening port on upgrade), credentials shared with `[opcua]` (single source of truth — one rotation step covers both surfaces), HMAC-SHA-256 keyed credential digest extracted from Story 7-2's `OpcgwAuthManager` into `src/security_hmac.rs` (Phase-B carry-forward rule per `epics.md:782`), `[web]` config validation (port range 1024-65535, parseable bind address, realm ≤ 64 chars + no `"`), `OPCGW_WEB__*` env-var overrides via figment's nested-key convention, `CancellationToken`-driven graceful shutdown joining the existing `tokio::select!` sequence in `src/main.rs`, 5 web-config validation unit tests + 10 web-auth middleware unit tests + 7 web integration tests + 4 security_hmac unit tests, clippy clean, TLS-via-reverse-proxy explicitly out of scope (tracked at #104). **9-2 review (2026-05-03 — implementation complete, awaiting code review)** — Gateway Status Dashboard (FR38) + responsive HTML/CSS/JS dashboard (FR41 fully closed at the content level). New `AppState` struct lands the deferred-from-9-1 shape (auth + per-task `SqliteBackend` + frozen `DashboardConfigSnapshot` + `start_time`); new `GET /api/status` JSON endpoint reads `get_gateway_health_metrics()` and returns the 6-field dashboard payload (`chirpstack_available`, `last_poll_time`, `error_count`, `application_count`, `device_count`, `uptime_secs`); new `src/web/api.rs` module hosts the handler (lands on the directory slot `architecture.md:417-421` reserved for "Stories 9-2 onwards"); `static/index.html` replaced with a real 6-tile dashboard backed by `static/dashboard.css` (mobile-first responsive grid + `prefers-color-scheme: dark`) and `static/dashboard.js` (`fetch('/api/status')` on load + 10 s `setInterval` + inline error banner on 401/5xx); `tests/web_dashboard.rs` adds 4 integration tests (auth carry-forward + JSON shape pin + HTML markup + CSS responsive marker); 4 new unit tests for `api_status` (success / 500-with-generic-body / first-startup defaults / `null`-serialisation pin); 3 new unit tests for `DashboardConfigSnapshot::from_config`. Audit-event surface: ZERO new web-side events; ONE new diagnostic event `event="api_status_storage_error"` (warn) on storage-read failure. AC#6 file invariants honoured: `git diff` shows zero changes to `src/opc_ua*.rs` and to `src/web/auth.rs` production code (only `tests/web_auth.rs` fixture wraps `WebAuthState` in `AppState` for the new `build_router` signature). **9-0 done (2026-05-05 — async-opcua runtime address-space mutation spike, code-review iter-1 + iter-2 complete).** Three load-bearing questions from `epics.md:784-787` resolved empirically against async-opcua 0.17.1 under live subscriptions: **Q1 add path RESOLVED FAVOURABLY** (a runtime-added variable's first subscription notification carries the registered sentinel; the `SyncSampler` honours post-init `add_read_callback` registrations without restart), **Q2 remove path Behaviour B (frozen-last-good)** (deleting a variable while subscribed produces no observable client error — Story 9-8 must emit an explicit `BadNodeIdUnknown` via `manager.set_attributes` before `delete()`), **Q3 sibling isolation RESOLVED FAVOURABLY** (bulk mutation of 11 nodes under a single `address_space.write()` = 117 µs hold time, ~850× shorter than the 100 ms sampler tick — no risk of starvation at typical 100-1000-node-reload scales). Implementation: split `OpcUa::run` into `build` + `run_handles` + backward-compat `run` wrapper (Shape B per AC#5; the `RunHandles` struct is the integration seam Story 9-7 hot-reload will reuse). All existing call sites compile unchanged; lib + bins test count holds at 322 + 345 = 667 / 0 fail / 5 ignored after iter-1 + iter-2 patches. New: `tests/opcua_dynamic_address_space_spike.rs` (3 spike tests), `_bmad-output/implementation-artifacts/9-0-spike-report.md` (12-section architecture-reference doc). Known limitation surfaced: `SimpleNodeManagerImpl` does not expose `remove_read_callback`, so deleting a node leaks the closure — 9-8 must extend `OpcgwHistoryNodeManager` with a remove method or file an upstream FR (precedent: Story 8-1's session-rejected callback FR at issue #94). Code-review iter-1 applied 10 patches (P2-P11: spike test asserts hardened, spike report § 7 restructured to "Pattern reuse", AC#1/#2/#3 measurement substitutions ratified in spec AC Amendments + spike report addenda); P1 (RunHandles missing Drop impl) blocked by rustc E0509 and tracked at GitHub KF issue #110 for Story 9-7 evaluation. Iter-2 re-ran all three reviewer layers per CLAUDE.md and applied 6 follow-up patches (IP1-IP6: residual "~110 LOC" claim, 30s sanity ceiling on lock-hold, header arithmetic, `assert_eq!` sentinel-comparison hardening, doc-comment cleanup, deferred-work.md path bracket). Remaining stories: 9-7 hot-reload, 9-8 dynamic address space, 9-4/9-5/9-6 CRUD. |
| **Epic I — Web UI Refresh** | 🚧 in progress (3/5 stories done; **v2.6.0 stable shipped the I-0/I-1/I-2 slice** 2026-07-04 after a clean NAS soak — I-3/I-4 remain for a later 2.6.x/2.7 cut; CR [#147](https://github.com/guycorbaz/opcgw/issues/147)) — refresh the web UI to a modern, **ChirpStack-v4 / Ant Design-adjacent** look so opcgw reads as a natural companion to the network server operators already run. Direction (a) locked: hand-written **vanilla CSS design system** on the F-1 shell — no framework, no build step, no `node_modules`; **pure presentation** (zero `/api/*` or behaviour change; served-HTML DOM-ID test markers + G-2 accessibility preserved). **I-0 done (2026-06-30)** — owner-approved visual-target mockup (`_bmad-output/implementation-artifacts/I-0-mockup.html`, dashboard in light+dark; its `:root` custom properties are the agreed token set). **I-1 done (2026-06-30 — impl + focused CSS code review; 3 HIGH + 1 MED WCAG AA contrast fixes; loop terminated LOW-only)** — design-token foundation: `static/dashboard.css` refactored onto `:root` CSS custom properties (antd palette) with fully token-driven dark mode (six per-component dark blocks collapsed to one). **I-2 done (2026-07-02 — impl + code review, 0 HIGH / 0 MED, loop terminated LOW-only; full test suite 1803/0, clippy clean)** — navigation & shell refresh: the F-1 shell (`shell.js` + CSS only) restyled into a ChirpStack-style fixed left sider on desktop and an app-bar with an accessible hamburger drawer (`aria-expanded`/`aria-controls`, keyboard operable) on mobile; `.page-header` restyled as a light top strip; nav link set and markup contract unchanged (the sider layout is gated on a `has-shell` body class so the shell-less first-run wizard is unaffected). WCAG AA contrast verified in light and dark. **I-3 backlog** — component refresh (cards/tables/forms/buttons/badges/banners on the I-1 tokens). **I-4 backlog** — rollout + dark-mode/responsive/accessibility QA across all pages (heaviest; defer candidate if v2.6.0 tightens). |
| **Epic G — Web UX & Usability** | ✅ done (5/5 stories + retrospective 2026-06-28; epic security review CLEAN 0 HIGH / 0 MED / 2 LOW; milestone #4 closed) — **G-0 + G-1 + G-2 + G-3 + G-4 done** 2026-06-27/28, Docker `:2.4`/`:latest`; GitHub milestone [#4](https://github.com/guycorbaz/opcgw/milestones) — the first post-announcement release line, polishing the web UI for newcomers arriving from the ChirpStack community. Five stories, each mapping to an existing CR, all built on the F-1 shell + F-0 staged-apply with no build step: **G-0 done (2026-06-27 — impl + code review iter-1+2, 0 HIGH, loop terminated LOW-only)** — drill-down configuration navigation: the three flat pages (Applications, Devices configuration, Commands) consolidate into one **Configuration** page (`/config.html`) presenting Application → Device → Metrics/Commands with a breadcrumb and deep-linkable `location.hash` routing; the nav collapses 3 links → one "Configuration"; retired pages become redirect stubs; frontend-only, reusing the existing staged-apply CRUD endpoints + inventory pickers ([#139](https://github.com/guycorbaz/opcgw/issues/139), foundational); **G-1 done (2026-06-28 — impl + code review 3 layers + iter-2, 0 HIGH, loop terminated LOW-only)** — device-profile metric picker: the metric picker now draws candidates from the device's ChirpStack device-profile measurements (new `GET /api/inventory/measurements?dev_eui=…`, `MeasurementKind` mapped to a suggested metric type) merged with the recently-observed uplink keys and de-duplicated by key, each row tagged with its source — so a freshly-added device is configurable before it has ever transmitted ([#124](https://github.com/guycorbaz/opcgw/issues/124), extends the C-2 picker, no write-path change); **G-2 done (2026-06-28 — impl + code review 3 layers + iter-2, 0 HIGH, loop terminated LOW-only)** — contextual field help: every config field across the first-run wizard, the gateway-settings editor, and the device/metric/command forms gets an accessible info-icon affordance (`aria-describedby`, keyboard + screen-reader reachable) whose text comes from one shared catalog (`static/field-help.js`) derived from `docs/configuration.md` so the UI and docs don't drift ([#142](https://github.com/guycorbaz/opcgw/issues/142)); **G-3 done (2026-06-27 — impl + code review iter-1+2, 0 HIGH; Closes [#132](https://github.com/guycorbaz/opcgw/issues/132))** — per-device OPC UA stale threshold (finishes the work descoped from F-3 to global-only); **G-4 done (2026-06-28 — impl + code review 3 layers + iter-2, 0 HIGH, loop terminated LOW-only)** — dashboard error drill-down: the "Errors (cumulative)" tile now links to an errors view (`/errors.html`) listing recent error events (time, category, device, sanitized message) newest-first. New bounded error-event store (migration v013 `error_events` table + in-memory ring buffer, cap `OPCGW_ERROR_EVENT_CAP`, default 500), captured at the poller's existing error sites (`device_poll` / `chirpstack_poll` / `metric_write`), exposed at `GET /api/errors?limit=…`; messages sanitized (no secrets), no aggregation (#130) — [#127](https://github.com/guycorbaz/opcgw/issues/127). Recommended order G-0 → G-4. Full epic spec in [`epics.md`](./_bmad-output/planning-artifacts/epics.md); per-story ACs drafted when `bmad-create-story G-N` runs. |
| **Epic H — Runtime Correctness & Tech-Debt** | 🚧 in progress (H-0 done + shipped; candidate follow-up stories open) — the v2.x tech-debt line flagged at the Epic G retrospective: latent runtime-correctness defects the test suite and adversarial review structurally cannot catch. **H-0 done (2026-06-30 — impl + code review complete; shipped stable in v2.5.2)** — **async storage facade** ([#73](https://github.com/guycorbaz/opcgw/issues/73)): the synchronous `StorageBackend` trait (~30 blocking `rusqlite` methods, shared as `Arc<dyn StorageBackend>`) was being called directly from ~30 async tokio call sites (poller, event-stream ingestion, OPC UA history, web handlers, command pollers), blocking worker threads on SQL and on the two `std::thread::sleep` pool-retry backoffs — survivable only because the runtime is multi-threaded, degrading on CPU-constrained Docker deployments. New `src/storage/async_facade.rs` introduces `AsyncStorage` (wraps one `Arc<dyn StorageBackend>`, runs every call via `tokio::task::spawn_blocking`), reached at call sites via the `AsyncStorageExt::async_store()` extension — no struct/constructor/test-fixture churn. Genuinely synchronous async-opcua read/method callbacks (which cannot `.await`) use a `run_blocking_storage()` helper that applies `tokio::task::block_in_place` only on a multi-thread worker and runs inline otherwise. No behavioural change (identical return types, `OpcGwError` mapping, ordering); the two retry sleeps now run inside `spawn_blocking`. 1803 tests pass / 0 failed, clippy `-D warnings` clean, release build clean. Candidate future H-stories: [#110](https://github.com/guycorbaz/opcgw/issues/110) (RunHandles `Drop`), [#79](https://github.com/guycorbaz/opcgw/issues/79) (queue-capacity enforcement). |
| **Epic F — Onboarding & Web UX for Public Release** | ✅ done (all 5 stories F-0..F-4 + retrospective complete 2026-06-24; epic security review CLEAN 0H/0M/3L; targeted for **v2.3.0**) — make opcgw effortless to configure and pleasant to use **before announcing it to the ChirpStack team**. Config-in-database is already done (Epics C/D); this epic targets the **first-run experience** and the **apply model**. **F-0 done (2026-06-14 — code review iter-1 + mandatory iter-2 complete; iter-1 caught 2 HIGH invisible to the full test suite — an `applied_gen` lost-update race and a build-time respawn failure that exited the process — both patched, iter-2 verified)** — staged config + explicit **"Apply changes"** soft restart. Every config-write surface (the singleton-config editor **and** the application/device/metric/command CRUD handlers) now **stages** to SQLite — edits no longer mutate the running gateway or restart anything; `GET /api/status` reports `pending_changes: true` until applied. A single `POST /api/config/apply` wakes a new **in-process restart supervisor** in `main.rs` that re-reads the effective config from SQLite and performs one graceful **in-process** soft restart of the data-plane (poller, OPC UA server, gRPC event stream, command-timeout handler) — the Docker container is **never** restarted; OPC UA clients briefly reconnect once per batch. The re-read happens *before* teardown, so a bad config is non-disruptive (`apply_failed`, current data-plane keeps running). Because the gRPC stream scope is recomputed from the freshly-read config on every Apply, this **closes CR [#138](https://github.com/guycorbaz/opcgw/issues/138)** (stream scope no longer frozen at boot). New audit events `config_staged` / `apply_invoked` / `apply_requested` / `apply_completed` / `apply_failed` / `config_apply_rejected` (see `docs/logging.md`); `singleton_config_restart_required` retired. The legacy live-reload path (`config_reload.rs` / `notify_crud_write`) goes dormant under the unified apply model (full removal is an F-0 follow-up). Two new subprocess integration tests (`tests/main_apply_restart.rs` proves the in-process re-entrant soft restart + stable PID; `tests/main_138_rescope.rs` proves the #138 stream re-scope end-to-end). **F-1 done (2026-06-14 — code review iter-1; the doubled-header-bar regression it caught was patched; component-class adoption + forms/tables primitives accepted as deferred to F-3)** — unified web shell (vanilla, **no build step / framework / `node_modules`** — a deliberate asset for an auditable industrial gateway): a shared `static/shell.js` injects one nav/header bar on every operator page (active link from `location.pathname`), removing the 7-link `<nav>` that was hand-duplicated across 9 pages; shared component CSS (`.app-shell` / `.btn` / `.status-badge` / `.banner` + light `.page-header` title strip) in `dashboard.css`; the F-0 Apply bar is hosted consistently; the first-run wizard stays standalone. No served-HTML regression (existing markup assertions still pass). **F-2 done (2026-06-15 — code review iter-1 + mandatory iter-2 + iter-3 verification; each iteration caught a HIGH invisible to the full test suite: iter-1 a partial SQLite write that poisoned the D-0 singleton migration (Guard 2 short-circuits next boot → global/opcua/web never migrate, no self-heal), iter-2 a placeholder prefix-vs-marker asymmetry that let a mid-string-marker secret pass validation yet jam the migration and dead-end the wizard — both fixed, iter-3 clean)** — zero-touch first-run wizard: `/setup` now captures the ChirpStack connection (`server_address` / `tenant_id` / `api_token`) **and** the OPC UA password so a fresh checkout boots browser-only with no text-file editing. The submit route was renamed `/api/setup/password` → `/api/setup`; both secrets are written to `config/secrets.toml` (0600) in one atomic write while the non-secret server address + tenant go to SQLite (secrets never touch SQLite). The crux: `AppConfig::validate()` and `is_first_run()` were extended to carve out missing ChirpStack credentials (mirroring the long-standing OPC UA password carve-out), so a pristine `config.toml` boots into the wizard instead of aborting at validation. Web-UI login user/password and the log-file location stay in `.env` by design — the wizard never touches them (a test asserts no `.env` is written). Restart-on-submit reuses the existing container-restart path (not the F-0 soft restart, which is post-first-run only); a write failure reverts the first-run flip without restarting so the operator can retry. **F-3 done (2026-06-15 — code review iter-1 + iter-2 verification; no HIGH, the iter-1 MEDs were fixed: a shared-error-banner clobber between the two pollers → per-poller error slots, unvalidated freshness thresholds that could flip the verdict → defensive fallbacks, and a band-model drift where `metricBand` treated empty-string as missing while `metrics.js` did not → aligned exactly)** — dashboard landing redesign: the landing page now leads with an at-a-glance health verdict (OK / specific degraded reason), adds a poller-status tile (stall detection vs the configured poll interval, surfaced as a new `poll_interval_secs` field on `GET /api/status`) and a per-device data-freshness panel (fresh / stale / bad / never), and adopts the F-1 shared `.status-badge` / `.banner` components (the component-adoption F-1 deferred for this page). All rollups are derived client-side from the existing `/api/status` + `/api/devices` payloads — no gateway-side aggregation (#130); no recent-errors list (no event store). The Story 9-2 fetch hardening is preserved and factored to cover both endpoints. **F-4 done (2026-06-24 — code review iter-1 + iter-2 + mandatory iter-2b independent re-review; iter-1 caught a HIGH the test suite missed — `command_class` silently dropped on import, breaking the Epic E valve device-class binding on round-trip — plus 3 MEDs all fixed; iter-2b, mandated because iter-1 introduced brand-new transaction flow-control, caught 1 MEDIUM — pooled SQLite connections had no `busy_timeout`, so concurrent `BEGIN EXCLUSIVE` writers (import vs import / import vs a CRUD save / import vs the Apply-reload) got `SQLITE_BUSY` immediately and surfaced a spurious 500; fixed by setting a 5 s `busy_timeout` on every pooled connection, hardening all eight writers — issue [#141](https://github.com/guycorbaz/opcgw/issues/141))** — config export/import: `GET /api/config/export` downloads the full configuration (the four singleton sections + the application/device/metric/command tree) as a portable TOML file with **secrets excluded** (`api_token`/`user_password` are never serialized — stripped via the existing `SECRET_FIELDS_BY_SECTION` skip-list), and `POST /api/config/import` accepts a `{ "toml": ... }` JSON envelope (no multipart — CSRF requires `application/json`, so the browser reads the file client-side), **merges it over the current config via figment** (so the target instance keeps its own secrets — import never carries or overwrites them), validates the candidate, and **stages** it through the F-0 Apply flow (atomic app-tree bulk-replace + singleton-section writes; the operator clicks **Apply changes** to activate — import never applies inline). New `toml` crate dependency + `Serialize` derives on the config structs enable SQLite→TOML serialization. Recommended order F-0 → F-1 → F-2 → F-3 → F-4. Tracked as GH [#140](https://github.com/guycorbaz/opcgw/issues/140). |
| **Epic E — Model-Agnostic, Class-Aware Device-Abstraction Layer** | ✅ done (4/4 stories + retrospective, 2026-06-13 — retro at `_bmad-output/implementation-artifacts/epic-E-retro-2026-06-13.md`; security review CLEAN 0H/0M/0L; E-2b deferred to backlog; tracked as GH [#129](https://github.com/guycorbaz/opcgw/issues/129)) — presents heterogeneous LoRaWAN devices with a **common OPC UA view**: per-model protocol stays in the ChirpStack codec, per-class canonical command/status semantics live in opcgw. First driver: Tonhe E20 motorized valves. **E-0 done (2026-06-11)** — Downlink Command Path: wires the previously-unwired `process_command_queue` so an OPC UA command write is actually delivered to the device. The poller now drains the `DeviceCommand` queue that `OpcUa::set_command` feeds, maps each command to either a **semantic command object** (class-bound) or **raw bytes** (fallback), enqueues it to ChirpStack's `DeviceService.Enqueue`, and transitions status `Pending → Sent` (failures → `Failed`, batch continues). New optional `command_class` field on `[[application.device.command]]` (`"valve"` → canonical `1`=open / `0`=close mapped to `{"command":"open"/"close"}` for the device-profile codec; absent = legacy raw-byte path). New schema migration **v011** adds the `command_class` column (round-trips through the SQLite application store). 12 new unit/async tests (mapping, queue-item shape, success/failure status transitions, raw fallback, config lookup). **Real-world valve gate PASSED 2026-06-11 on v2.2.0-rc4** (main-deadlock incident doctrine): a full OPEN+CLOSE cycle driven from Fuxa/OPC UA was delivered through ChirpStack and physically actuated the valve both ways. Observed during the test: command dispatch waits for the next metrics-poll cycle (up to `polling_frequency` seconds) because the queue drain is coupled to `poll_metrics` — CR [#136](https://github.com/guycorbaz/opcgw/issues/136) filed to decouple it (backlog, adjacent to E-3). **E-1 in-progress (slice E-1a, 2026-06-09)** — uplink event ingestion: a long-lived `InternalService.StreamDeviceEvents` consumer stores valve-class devices' decoded uplink values as **last-known values stamped with the device's source timestamp, with no aggregation** ([#130](https://github.com/guycorbaz/opcgw/issues/130): the metrics poll's time-aggregation corrupted discrete valve state to a nonsense `391` / `1.5`); the metrics poll now skips valve-class devices so the stream is authoritative, and the OPC UA `source_timestamp` now reflects the device report time. **E-1 done (2026-06-12)** — the E-1b remainder landed: fleet-wide de-aggregation via `chirpstack.stream_all_devices` (validated on rc2), per-device `stale_threshold_seconds` ([#132](https://github.com/guycorbaz/opcgw/issues/132)), orphan-field warnings with self-correction, and **startup/reconnect backfill** — on every stream (re)connect opcgw fetches the device's newest recent event (bounded, never `GetMetrics`) and stores it under a freshness guard so an older backfill can never overwrite a newer live value; the stream consumer is now behind an injectable `UplinkSource` seam with reconnect/backfill/precedence tests. The final `done` gate — **the AC#11 cold-start check — PASSED on v2.2.0-rc5 in production (2026-06-12)**: SQLite metric restore (40/40, 0 orphans) completed before the poller started, the backfill freshness guard correctly skipped the already-fresh restored values, and live uplinks applied continuously with zero stream drops. E-1 was the last v2.2.0 release blocker — **v2.2.0 stable is now unblocked**. **E-2a done (2026-06-10)** — device-class registry: delivered in two increments. **(1)** wires the `command_class` device-class binding through the web command editor (`POST`/`PUT /api/applications/:app/devices/:dev/commands`, the `commands.html` editor's class selector, and the `GET` views), closing [#135](https://github.com/guycorbaz/opcgw/issues/135) so a valve command set to `command_class = "valve"` is delivered as the semantic `{"command":"open"/"close"}` object (codec → `0x01`/`0x02`) instead of an invalid raw byte; the class is validated at both the web API and config-load against the registry. **(2)** extracts the concrete valve mapping into a device-class registry (`src/device_registry.rs`: a `DeviceDriver` trait + `ClassRegistry`, with the valve as the first Tier-1 driver) so `chirpstack::map_command_to_downlink` is now thin dispatch — zero behaviour change. **E-2b** (Tier-2 object-remap adapter + SetLevel/`command_kind` + a second class) is deferred to backlog until a concrete second model/class exists. **E-3 implemented (2026-06-13, in review)** — Command Delivery Confirmation: completes the command lifecycle `Pending → Sent → Confirmed/Failed`. The enqueue path now captures ChirpStack's queue-item id (`EnqueueDeviceQueueItemResponse.id`) as the command's `chirpstack_result_id` (the correlation key — previously discarded). Delivery confirmation is **event-driven**: the same `InternalService.StreamDeviceEvents` consumer (E-1) now also handles `ack` / `txack` device events, correlates the ack's `queue_item_id` to the queued command, and marks it `Confirmed` (device acknowledged) or `Failed` (NACK); a `txack` is a transmit diagnostic only. Unconfirmed downlinks (no ack) resolve via the existing timeout sweep (`command_delivery_timeout_secs`). All transitions are idempotent so replayed acks on stream reconnect can't regress a terminal command. The poll-shaped `CommandStatusPoller` stub is repurposed as a confirmation-backlog observability heartbeat (no more ChirpStack polling). Command status is exposed read-only on OPC UA via `CommandStatusQuery`. New audit events `command_confirmed` / `command_confirm_failed` / `command_timeout`. +17 tests (ack→confirm, NACK→fail, txack-no-confirm, unmatched/duplicate idempotency, result-id capture, confirm-vs-timeout race); `cargo test` 0-fail + clippy `-D warnings` clean. Next: code review, then Epic E retrospective. |
| **Epic D — Singleton Configuration → SQLite** | ✅ done (3/3 stories + retrospective, 2026-05-28 — retro at `_bmad-output/implementation-artifacts/epic-D-retro-2026-05-28.md`; doctrine streak now 32× cumulative across 8 Epic D review iterations; security review CLEAN with 0 HIGH / 0 MED / 2 LOW; **AI-C-SEC-2 resolved** this epic; next direction = v2.x skill-codification + cleanup epic; **AI-D-3 real-world smoke deferred as critical-path gate before prod cutover**) — 3-story epic opened 2026-05-26 (scoping commit `0787859`); natural follow-up to C-6, migrating the remaining `[global]` / `[chirpstack]` / `[opcua]` / `[web]` singleton sections from `config.toml` to SQLite. Secrets stay in `secrets.toml`. **D-0 ✅ done 2026-05-27** (3-iteration code-review loop on Sonnet, 27th cumulative iter-N+1 doctrine validation; commits `cdba5e6` impl + `9adf84c` iter-1 + `6a753cf` iter-2 + `00b61e6` iter-3 + flip; iter-1 caught 3 HIGH (no outer EXCLUSIVE transaction + TOCTOU + AC#4 violation converged), iter-2 caught 2 HIGH in iter-1's brand-new EXCLUSIVE-transaction logic (unbounded count + Test 11 WAL fake guard), iter-3 caught 2 converged doc-sync gaps; final 1506/0/73 / clippy clean — singleton-config migration: schema v010 + `singleton_config(section, key, value)` K/V table + boot-time one-shot migration via `migrate_singleton_toml_to_sqlite` mirroring C-6's pattern (primary done-flag guard + secondary back-fill guard + placeholder-secrets skip + EXCLUSIVE-style writes with row-count verification + fall-back to TOML on mismatch); new `SqliteBackend::load_singleton_config` / `write_singleton_section` / `is_d0_migration_done` / `write_d0_migration_done` / `count_singleton_config` helpers; new `migrate_singleton_config.rs` module; new `docs/d-0-migration-runbook.md` + `scripts/check-d0-migration.sh`; SQLite file-permission tightening per AI-C-SEC-2 from Epic C security review (chmod 0o600 on fresh DB creation + once-per-boot `storage_init` warn for wider modes on existing DBs); 4 new `config_migration` stage values (`singleton_toml_to_sqlite`, `singleton_already_migrated`, `singleton_already_migrated_backfill_failed`, `skipped_placeholder_singleton`) + new `singleton_row_count_mismatch` reason on `config_migration_failed` + new `storage_init` event documented in `docs/logging.md`. AC#7 partially fulfilled: D-0 writes TOML → SQLite but the AppConfig **read-path swap** is deferred to D-2 alongside the TOML mutation-surface decommission, where the figment Provider stack can be reworked to put SQLite between TOML and env-var layers. **D-1 done 2026-05-27** (singleton config editor UI — `GET`/`PUT /api/config/singleton`, `static/singleton-config.{html,js}`, boot-time SQLite overlay; 2-iteration review, 28th–29th cumulative validations; 1521/0/73). **D-2 done 2026-05-28** (decommission TOML mutation: new `SqliteSingletonProvider` figment Provider delivers `env > SQLite > TOML > default` as a structural guarantee, `toml_edit` removed, orphan `config_writer.rs` deleted, `config_toml_unused_warning` once-per-boot; 3-iteration review with a clean iter-3, 30th–32nd validations; final 1544/0/73 / clippy / xmllint clean). `config.toml` is now bootstrap-seed-only; opcgw has exactly three persistence surfaces (SQLite + secrets.toml + config.toml-bootstrap). **Epic D retrospective done 2026-05-28** — see `epic-D-retro-2026-05-28.md`.). |
| **Epic C — Auto-Discovery and Web-First Configuration (post-v2.0 GA)** | ✅ done (6/6 stories + retrospective, 2026-05-26 — retro at `_bmad-output/implementation-artifacts/epic-C-retro-2026-05-26.md`; doctrine streak now 24× cumulative across 17 Epic C review iterations; security review CLEAN with 3 LOW v2.x carry-forward items AI-C-SEC-1/2/3) — **C-6 done 2026-05-26** (TOML→SQLite migration loop terminated after iter-4 — 4 iterations total: iter-1 14 patches + iter-2 9 patches + iter-3 2 patches + iter-4 2 doc-only patches; 24th cumulative doctrine validation; new finding-class refinement = "small iter-N patch rounds (8 lines) STILL warrant iter-N+1"; final 1482/0/73 tests, clippy clean). 6-story epic scoping commit `4f59592` + spec-drafting commit `afe6869` landed 2026-05-21. **C-4 done 2026-05-24** (inventory drift view: new `/inventory-drift.html` page + `GET /api/inventory/drift` endpoint + `POST /api/audit/drift-action` thin audit endpoint; 4-class diff (`ok`/`stale`/`available`/`drifted`) at application + device + metric levels; metric drift includes soft-stale `not_in_recent_uplinks` reason for codecs that emit keys conditionally and `wire_type_mismatch` reason when the configured `metric_type` diverges from observed uplink inference; operator-triggered refresh ONLY (no background polling — `?refresh=true` forwarded to every C-1 inventory fetch); read-only endpoint (action buttons dispatch through existing CRUD paths); ChirpStack-unreachable graceful degradation marks all rows as `ok` placeholders + disables destructive buttons + hides `[Add to opcgw]` + surfaces retry banner; deep-link from drift view to C-2 picker pages via `prefill_app_id` / `prefill_dev_eui` / `prefill_name` / `prefill_metric_key` query params consumed by `applications.js` + `devices-config.js`; new audit events `drift_view_opened`, `drift_action`, `drift_dismissed`, `drift_audit_rejected`, `inventory_drift_succeeded`, `inventory_drift_unreachable`; new `src/web/drift.rs` module with 13 unit tests covering the 4-class diff matrix exhaustively + 10 integration tests in `tests/web_inventory_drift.rs` covering the audit endpoint + unreachable degradation + auth/CSRF carry-forward; see `docs/web-api.md § Story C-4` and DocBook user manual `<sect1 id="sec-inventory-drift">`). **C-3 done 2026-05-23** (server-side duplicate-prevention validator + cross-path consistency: validator's `seen_device_ids` HashSet moved INSIDE the per-application loop so same DevEUI across applications is now ALLOWED per AC#5; new per-device pre-flight duplicate-check blocks in `create_device` / `update_device` for `chirpstack_metric_name` + `metric_name` catch the hazard at the CRUD-handler layer (clean 409) instead of letting it traverse to reload-time validate-fail (422 + rollback); new structured JSON error body `{"error":"duplicate","field","value","scope","hint"}` returned by every duplicate-rejection 409; new `conflict_kind` sub-field on the `reason="conflict"` audit-event family (values `duplicate`, `malformed_existing_block`, `cascade_blocked`, `empty_application_list`) disambiguates the two semantically-different existing uses without breaking grep contracts; new `event="config_reload_rejected"` audit event emitted alongside `config_reload_failed` when a SIGHUP-triggered hot-reload fails specifically because the operator's TOML hand-edit introduced a duplicate (drives `ReloadError::is_duplicate()` predicate); 12 new integration tests in `tests/web_duplicate_prevention.rs` covering the application / device / metric / command duplicate paths + cross-application same-DevEUI positive case + same-`chirpstack_metric_name`-across-devices positive case + hot-reload rejection + audit-event taxonomy assertion; see `docs/web-api.md § Story C-3 — duplicate-rejection contract` + `docs/logging.md § Story C-3 — conflict_kind sub-field of reason="conflict"`). **C-2 done 2026-05-23** (inventory pickers in the web UI: `/applications.html` + `/devices-config.html` replace the free-form UUID/DevEUI inputs with cascading name-driven `<select>` dropdowns fed by C-1's `/api/inventory/*` endpoints; load-bearing manual-fallback toggle with localStorage mode persistence per-page; new shared `static/inventory-picker.js` module; new `POST /api/audit/picker-event` thin endpoint for client-attributable picker audit events with allowlist validation + per-event field sanitisation; new optional `picker_metadata` field on the metric-create path → emits `event="metric_wire_type_inferred"` per picker-attributed metric; new audit events `picker_opened`, `picker_manual_fallback`, `picker_audit_rejected`, `metric_wire_type_inferred`; 13 new integration tests in `tests/web_picker.rs`; see `docs/inventory-api.md § Picker-event audit endpoint`). **C-1 done 2026-05-22** (ChirpStack inventory query layer: 3 new GET endpoints `/api/inventory/{applications,devices,uplinks}` + server-side TTL cache + race-free fetch + CRUD invalidation hooks + wire-type inference + 5 audit events including `shutdown_cancellation` + `inventory_uplink_dropped`; see `docs/inventory-api.md`. Loop terminated after 3 review iterations — 17th iter-N+1 doctrine validation. Commits `b5df23f` + `372280f` + `3f35ddb` + `5840c04` + flip). **C-0 done 2026-05-22** (empty-config bootstrap + first-run wizard at `/setup`) — implementation `c200089` + iter-1 review fixes `7ec2fc1` + iter-2 review fixes `d1d1332` + iter-3 review fixes `d84fa7d`; 15th iter-N+1 doctrine validation; 1326/0/10 tests / clippy clean. Validator amended to accept empty `[opcua].user_password` when no env-var is set; new `is_first_run()` method; figment provider stack extended with sibling `config/secrets.toml` between main TOML and env-var layer; new `src/web/setup.rs` module providing the first-run gate middleware, wizard GET handler, POST persistence handler with HTTP 4 KiB body limit + strict `application/json` Content-Type + same-origin Origin check + race-free `Arc<AtomicBool>` compare_exchange one-shot guarantee; new `static/setup.html` wizard frontend with categorised error-reason UX (readonly_filesystem / disk_full / permission_denied / parent_directory_missing); OPC UA server emits `event="opcua_first_run_mode"` audit + rejects all auth via existing `OpcgwAuthManager::is_configured = false` path until restart; gateway shuts down gracefully via `CancellationToken` on successful wizard submit so the supervisor (Docker / systemd) restarts and the figment provider stack picks up the new `secrets.toml`. **C-5 (MQTT real-time path) intentionally removed from epic** — tracked as `CR-EPIC-C-MQTT` in `deferred-work.md`. **C-6 impl done 2026-05-25** (TOML→SQLite configuration migration + SQLite-driven hot-reload: one-shot boot-time migration moves `[[application]]` tree from `config.toml` into SQLite on first boot of post-C-6 binary; boot detects empty `applications` table + non-empty TOML list → opens transaction, inserts all apps/devices/metrics/commands, verifies row counts, commits, records `config_migrated_from_toml_at` timestamp in `meta`; migration fallback to TOML-driven boot on row-count mismatch or insert failure; SIGHUP-triggered TOML hot-reload path (Story 9-7) fully removed — `src/config_hot_reload.rs` and `src/web/config_writer.rs` deleted; `notify_crud_write` now emits `event="config_reload" trigger="crud_write"` after every CRUD write; new `docs/c-6-migration-runbook.md` + `scripts/check-c6-migration.sh` operator tool; architecture.md fully rewritten to reflect SQLite as single authoritative store; DocBook manual `sec-config-overview` / `sec-application-config` updated to describe two-tier model; `docs/logging.md` updated with `config_migration` / `config_migration_failed` / `config_reload trigger="crud_write"` audit events; 14 new integration tests in `tests/sqlite_config_migration.rs`; pending code review). |
| **Epic B — v2.0 GA Release Packaging** | ✅ done | Docker Hub + GHCR multi-arch (amd64/arm64) publishing, DocBook user manual, v2.0 GA packaging — story `B-1` + retrospective complete. |
| **Epic A — Storage Payload Migration (Phase B closure — v2.0 GA gating epic)** | ✅ done — **all 7 stories done (A-1 / A-2 / A-3 / A-4 / A-5 / A-6 / A-7) + epic-A-retrospective done (2026-05-19)**. **Issue #108 fully closed across all 7 stories.** Retrospective conducted inline security review (CLEAN with 1 LOW patched); 11 action items captured (AI-A-1 to AI-A-11). Critical path before `v2.0` tag: (a) CHANGELOG entry for `MetricValue.value` SemVer-major retire (AI-A-4); (b) `git push` per CLAUDE.md "After an epic retrospective" rule. | Closes issue [#108](https://github.com/guycorbaz/opcgw/issues/108) — the payload-less `MetricType` enum that flattened every persisted metric value to its discriminant string ("Float" / "Int" / "Bool" / "String") instead of the real measurement. **A-1 done** — `MetricType` refactored to payload-bearing (`Float(f64)` / `Int(i64)` / `Bool(bool)` / `String(String)`); `Copy` dropped; `Display` + `FromStr` preserved with zero-default contract; `MetricValue.value: String` retained dual-storage with `TODO(A-5)` marker. **A-2 done** — v007 SQLite migration adds five typed columns (`value_real REAL NULL` / `value_int INTEGER NULL` / `value_bool INTEGER NULL` / `value_text TEXT NULL` / `value_type TEXT NOT NULL DEFAULT 'legacy' CHECK(value_type IN ('legacy','Float','Int','Bool','String'))`) to both `metric_values` and `metric_history`. Pre-Epic-A rows tagged `value_type='legacy'` via column default; writers + readers strict-zero per option-b staging. **A-3 done** — Poller value-payload write pipeline: all 7 `TODO(A-3)` sites in `prepare_metric_for_batch` wrap real ChirpStack values into the typed payload; NaN/Inf option-(a) filter at poller with `event="metric_parse"` `reason="non_finite"` warn; all 4 SqliteBackend writers populate typed columns + `value_type` via private helper `typed_value_columns()`; new v008 migration adds exactly-one-non-NULL CHECK via CREATE TABLE … AS SELECT pattern wrapped in BEGIN/COMMIT; `chirpstack.rs::store_metric` body reinstated with NaN/Inf guard. **A-5 done (2026-05-17)** — OPC UA HistoryRead value-payload pipeline: `SqliteBackend::query_metric_history` rewires SELECT to project v007 typed columns via the A-4-shared `metric_type_from_typed_columns` helper; `HistoricalMetricRow` restructured to `payload: Option<MetricType>` so legacy rows are a first-class outcome. `OpcgwHistoryNodeManagerImpl::build_data_values` pattern-matches the typed payload directly; legacy rows emit `DataValue { value: None, status: BadDataUnavailable }` per epic AC#1 (NOT silently dropped). The transitional `MetricValue.value: String` field was retired across all 4 public storage structs (`MetricValue`, `MetricValueInternal`, `BatchMetricWrite`, `HistoricalMetricRow`); compile-time field-shape pins via non-exhaustive destructure patterns lock the shape against future re-introduction. New audit event `metric_history_read` (closed reason enum: `{schema_drift, unparseable_timestamp, narrowing_overflow, narrowing_underflow}`) + iter-2 `metric_history_summary` (trace-level aggregate skip telemetry). `convert_variant_to_metric` signature simplified to `Result<MetricType, OpcGwError>`. Story 9-3 dashboard wire-format preserved via `metric_view_display_string` shim (Bool `"0"`/`"1"`, Float f32-precision). 1230 tests / 0 fail / 10 ignored; clippy clean; doctest clean. **Issue #108 fully closes — Epic A now 5/7 done.** **A-4 done (2026-05-17)** — OPC UA Read value-payload pipeline: SqliteBackend readers (`get_metric_value` / `get_metric` / `load_all_metrics`) rewire SELECT to project typed columns via new private helper `metric_type_from_typed_columns()`; `OpcUa::convert_metric_to_variant` rewrites to pattern-match the typed payload directly (no more `metric.value.parse::<…>()`); legacy rows surface as `Ok(None)` mapping transitively to `BadDataUnavailable` per architecture.md:182. Float narrowing-overflow guard (A-1-iter1-DEF17 closure) emits new `event="metric_read"` `reason="narrowing_overflow"` warn (plus iter-1 IR7 `reason="narrowing_underflow"` and iter-2 JR4 `reason="no_payload"` siblings). AC#9 from A-3 (Counter monotonic typed-path preference) closed at `chirpstack.rs:1707-1727`. End-to-end live OPC UA server proof: `tests/opcua_subscription_spike.rs::test_subscription_datavalue_payload_carries_seeded_value` passes — real client receives `Variant::Float(42.5)` with `Good` status through the new typed-payload Read pipeline. 1214 tests / 0 fail / 10 ignored (post-iter-2 final); clippy clean; doctest 0 failed / 55 ignored. **A-6 done (2026-05-18)** — Web UI live-metrics typed value display. `bmad-dev-story A-6` implementation complete; status flipped `ready-for-dev → in-progress → review`. All 14 ACs SATISFIED. `MetricView` JSON shape widened from `value: Option<String>` to typed `value: Option<serde_json::Value>` + new `unit: Option<String>` field. New helper `metric_type_to_json_value` in `src/web/api.rs` emits typed JSON primitives; non-finite Float yields `None` + emits new `event="metric_view_serialize"` `reason="non_finite"` warn. `web::MetricSpec` widened with `metric_unit`; hot-reload propagation flows through Story 9-7 without `config_reload.rs` changes. **Retired `metric_view_display_string` transitional shim** + its 2 unit tests (closes `DEF-iter1-A5-D1`); replaced with 5 typed-JSON unit tests. `static/metrics.js::renderMetricRow` gets new `formatValue(value, dataType)` helper + value-plus-unit composition; Bool wire shifts from `"1"`/`"0"` to native `true`/`false`. **Task 1 finding (load-bearing):** post-A-5 `load_all_metrics` silently SKIPS legacy rows at `src/storage/sqlite.rs:1530-1540` (`legacy_skipped_count += 1; continue;`) — legacy rows never reach the dashboard handler. Consequence: `metric_view_serialize` event has ONLY the `non_finite` reason (legacy_row dropped from the closed enum). Closes deferred-work `9-5-iter1-D4` (empty-string `metric_unit` coalesces to no-unit-suffix at the renderer via JS truthy check). `MetricView` lost `PartialEq, Eq` derives (cascaded to `DevicesResponse` / `ApplicationView` / `DeviceView`) because `serde_json::Value` doesn't impl `Eq`. cargo test 1242 / 0 / 10 (was 1230 A-5 baseline; +12; exceeds AC#12 target ≥1240 by 2); clippy clean; doctest clean. AC#5 grep returns 5 lines (`metric_history_read` + `metric_history_summary` + `metric_parse` + `metric_read` + `metric_view_serialize`); AC#7 grep returns 0 `metric_view_display_string` references. Recommend `bmad-code-review A-6` on a different LLM. Next: `bmad-code-review A-6`. **A-7 done (2026-05-18)** — Migration runbook + version-gated script. `bmad-create-story A-7` produced spec at `A-7-migration-runbook-and-script.md` (~9 ACs, 6 tasks, 1 decision-needed). **FINAL Epic A story.** Pure docs + tests + small shell script — NO Rust production-code changes. Deliverables: (1) `docs/deployment-guide.md § "Epic A migration"` operator runbook covering Path A (auto-migration on next gateway startup, preserves legacy rows as `value_type='legacy'`) / Path B (drop `opcgw.db*` and start fresh) / pre-upgrade checklist / post-migration verification / one-way rollback contract (restore pre-upgrade backup) / SLA expectation / common gotchas; (2) NEW `scripts/check-schema-version.sh` shell script wrapping `sqlite3 PRAGMA user_version` with operator-friendly output + Path A/B recommendations (sqlite3 CLI dependency, introduces new top-level `scripts/` directory); (3) NEW end-to-end regression test `test_v006_to_v008_full_upgrade_path_under_5s` in `src/storage/schema.rs::tests` pinning the chained v006→v007→v008 path via a single `run_migrations(&conn)` call (NOT the manual `execute_batch` shortcut — captures the docstring-vs-body drift lesson from A-5). Decision D1 (load-bearing): epic AC's literal "5s for 100MB" target doesn't align with existing `test_v008_migration_under_30s_for_10k_rows` baseline; dev-agent picks (a) keep 5s-for-10k-row baseline + document larger-DB caveat in the runbook (recommended), (b) 500k-row `#[ignore]` test, or (c) tune v008 (out of A-7 scope — would re-open A-3 review loop). AC#7 strict-zero invariants: A-7 NARROW-MUTABLE on `src/storage/schema.rs::tests` only (production-code body untouched); MUTABLE on `docs/deployment-guide.md` + NEW `scripts/check-schema-version.sh` + `README.md` + `sprint-status.yaml` + this spec file. Test budget ≥1255 (was 1254 A-6 baseline + 1 new fn). Grep contracts unchanged. **Epic A completion + retrospective trigger:** after A-7 ships, Epic A is 7/7 done; per CLAUDE.md "Do not skip the retrospective" the `epic-A-retrospective: optional` line becomes mandatory — the very next BMad action after A-7's Code Review Complete commit MUST be `bmad-retrospective` for Epic A, the gate to v2.0 GA release. **v2.0 GA gated on A-7 + Epic A retrospective landing.** |

### How to read this section

- **Status legend:** ✅ done · 🔄 in-progress · 📋 backlog (and 📝 ready-for-dev / 👀 review for individual stories).
- **Phase A** covers Epics 1–7 — production hardening of the existing one-way (read) gateway.
- **Phase B** covers Epics 8–9 — adds real-time subscriptions, historical data access, and a web admin surface. Story 4-4 is deferred to a Phase B resilience epic.
- For the canonical, machine-readable view, see [`sprint-status.yaml`](./_bmad-output/implementation-artifacts/sprint-status.yaml). The sprint-status file is the source of truth; this table is updated alongside it.
- Per-story details, acceptance criteria, dev notes, and review findings live in `_bmad-output/implementation-artifacts/<epic>-<story>-<slug>.md`.

A long-form roadmap with marketing-friendly language is available at [Roadmap](https://guycorbaz.github.io/opcgw/features/#roadmap).

## Use Cases

- 🌱 **Smart Agriculture**: Monitor soil conditions across farms via wireless sensors
- 🏭 **Industrial IoT**: Asset tracking and equipment monitoring
- 🌍 **Environmental Monitoring**: Air quality, weather stations, environmental sensors
- 🏢 **Building Automation**: HVAC, occupancy, energy management
- ⚡ **Renewable Energy**: Solar + battery microgrid optimization

→ See [Real-World Use Cases](https://guycorbaz.github.io/opcgw/usecases/) for detailed scenarios.

## Logging

opcgw is built on `tracing` with a single daily-rolling file appender (retention-capped, self-limiting) and a stderr console layer. The global verbosity is configurable at runtime — no rebuild required.

```bash
# Set verbosity for a single run
OPCGW_LOG_LEVEL=debug ./target/release/opcgw

# Or persist in config.toml
[logging]
level = "debug"
dir = "/var/log/opcgw"
```

Valid levels: `trace`, `debug`, `info` (default), `warn`, `error`. Per-module file appenders capture independently of the global level — see [`docs/logging.md`](./docs/logging.md) for the operator-facing reference, including the structured-field schema, correlation-ID tracing, and the env-var override convention.

At startup, opcgw also verifies that the performance-critical `idx_metric_history_device_timestamp` index exists. If it is missing (e.g. dropped manually or left absent by a partially-applied migration), the gateway logs a single `warn!` with `event="metric_history_index_missing"` and a remediation hint, then continues — a missing performance index degrades history-query speed but never aborts startup.

**Storage-latency budgets.** Slow SQLite queries and batch writes are surfaced as `exceeded_budget=true` warnings. The thresholds default to NAS-realistic values (storage query **250 ms**, batch write **2000 ms**) and are tunable per deployment via the `OPCGW_STORAGE_QUERY_BUDGET_MS` and `OPCGW_BATCH_WRITE_BUDGET_MS` environment variables (positive integer milliseconds) — lower them on fast local SSDs for earlier regression detection. They only affect logging, never storage behaviour, and are resolved once at startup (logged as `operation="storage_budget_init"`).

## Architecture

opcgw consists of two main components running concurrently:

- **ChirpStack Poller**: Polls device metrics from ChirpStack via gRPC at configurable intervals
- **OPC UA Server**: Exposes collected metrics as OPC UA variables for industrial clients

Both components share thread-safe in-memory storage via `Arc<Mutex<Storage>>`.

→ [See full architecture](https://guycorbaz.github.io/opcgw/architecture/)

## Technology Stack

- **Language**: Rust 1.94.0+ with async/await
- **Protocols**: gRPC for ChirpStack, OPC UA 1.04 for industrial clients
- **Storage**: SQLite (WAL mode) for metric values, history, command queue, and configuration; in-memory cache for current values
- **Logging**: Tokio-tracing with structured fields, a single retention-capped daily log file, and a stderr console layer
- **Async Runtime**: Tokio for high-performance I/O
- **Build**: Multi-stage Docker build for minimal image size

## Contributing

Contributions are welcome! Please:

1. Check [existing issues](https://github.com/guycorbaz/opcgw/issues) first
2. Open an issue to discuss your idea before implementing
3. Follow the code style and conventions in CLAUDE.md
4. Ensure tests pass: `cargo test && cargo clippy`
5. Submit a pull request with a clear description

## Development

```bash
# Build and test
cargo build --release
cargo test
cargo clippy

# Run with debug logging
RUST_LOG=debug cargo run -c config/config.toml

# Watch logs
tail -f log/*.log
```

## License

Licensed under either MIT or Apache-2.0 at your option.

---

## Support

- 📖 [Documentation](https://guycorbaz.github.io/opcgw/)
- 🐛 [Issues](https://github.com/guycorbaz/opcgw/issues)
- 💬 [Discussions](https://github.com/guycorbaz/opcgw/discussions)
- 📧 Contact: gcorbaz@gmail.com

## Contributing

Any contributions you make are greatly appreciated. If you identify any errors,
or have an idea for an improvement, please open an [issue](https://github.com/guycorbaz/opcgw/issues).
But before filing a new issue, please look through already existing issues. Search open and closed issues first.

Non-code contributions are also highly appreciated, such as improving the documentation
or promoting opcgw on social media.


## License

MIT OR Apache-2.0.

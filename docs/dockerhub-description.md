<!--
  This Markdown is the canonical source for the Overview page rendered at
  https://hub.docker.com/r/gcorbaz/opcgw

  Auto-synced via `peter-evans/dockerhub-description@v4` on every `v*` tag
  push (see .github/workflows/docker-build.yml). Do NOT edit the Overview
  page directly on hub.docker.com — edits will be overwritten on the next
  release. Edit this file and push a tag (or trigger the workflow) to
  update the live page.
-->

<p align="center">
  <img src="https://raw.githubusercontent.com/guycorbaz/opcgw/main/docs/logo/opcgw-horizontal.svg" alt="opcgw — ChirpStack to OPC UA Gateway" width="400">
</p>

# opcgw — ChirpStack to OPC UA Gateway

[![License](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](https://github.com/guycorbaz/opcgw)
[![Architecture](https://img.shields.io/badge/arch-amd64%20%7C%20arm64-brightgreen.svg)](#supported-architectures)

**opcgw** is a Rust application that bridges a [ChirpStack](https://www.chirpstack.io/) LoRaWAN Network Server to OPC UA clients for industrial automation and SCADA systems. It polls device metrics from ChirpStack's gRPC API at configurable intervals, persists them to SQLite with a 7-day rolling history, and exposes them as OPC UA variables with real-time `Read`, `HistoryRead`, and live subscription support.

The image is published in lockstep to **two registries**:

- **Docker Hub** (primary): `docker.io/gcorbaz/opcgw`
- **GHCR** (mirror): `ghcr.io/guycorbaz/opcgw`

Both registries receive identical multi-architecture manifests built from the same workflow run.

## Why opcgw vs. ChirpStack's built-in integrations?

ChirpStack ships with MQTT and HTTP integrations that already deliver device data to external systems — so why a dedicated gateway?

**opcgw is a name-translation layer.** Bare ChirpStack integrations emit events keyed by UUIDs (`application_id`, `tenant_id`) and DevEUIs. Downstream SCADA operators see opaque identifiers and have no way to map `52f14cd4-c6f1-4fbd-8f87-4025e1d49242 / a840414bf185f365` to "the temperature sensor in the storeroom."

opcgw polls ChirpStack for both the **configuration** (tenant / application / device names) **and** the **data** (metric values), then exposes the result via OPC UA — the de-facto standard for industrial automation — with a **named browse tree** that operators can read directly:

```
Server
└── Objects
    └── Bâtiments                ← your application_name (any Unicode)
        └── lsn50-magasin        ← your device_name
            ├── Humidity         ← Float, 72.36
            ├── Temperature      ← Float, 15.13 °C
            └── BatV             ← Float, 3.67
```

If you're already running OPC UA SCADA software (Ignition, KEPServerEX, B&R, Siemens, etc.), opcgw plugs straight into it — no custom MQTT-to-tag glue code, no UUID translation table to maintain.

## Architecture

```
LoRaWAN device                                            OPC UA client
(uplinks ~every                                           (SCADA / HMI /
20 minutes)                                                historian)
       │                                                          ▲
       ▼                                                          │
 ┌──────────────┐    gRPC      ┌────────────────────┐  OPC UA TCP │
 │  ChirpStack  │◄─── poll ────│       opcgw        │─────────────┘
 │  v4 server   │  every 60s   │  • Name translation│  (port 4840)
 │              │              │  • SQLite history  │
 └──────────────┘              │  • Optional web UI │  HTTP(S)
                               └────────────────────┘─────────────┐
                                                                  ▼
                                                          Operator browser
                                                          (port 8080)
```

## Features

### OPC UA server (port 4840)

- **Read** — current value of any configured metric, with timestamp + quality
- **HistoryRead** — time-series reads over the last 7 days (configurable retention)
- **Subscriptions / MonitoredItems** — push-based updates to clients on metric change
- **Standard data types** — `Float`, `Int32`/`Int64`, `Boolean`, `String`
- **Anonymous + username/password security profiles** (`None` security policy out of the box; PKI-validated profiles supported)
- **Session-count + connection-limit telemetry** — opcgw caps concurrent sessions per `[opcua].max_connections` and emits a `opcua_session_count_at_limit` warn when the limit is reached

### Embedded web UI (port 8080 — enabled by default; toggle via `OPCGW_WEB__ENABLED`)

- **Zero-touch first-run setup wizard** — on first boot, collects the ChirpStack connection (server address, tenant ID, API token) **and** the OPC UA password from the browser; secrets are written to `config/secrets.toml` (mode `0600`) and the rest to SQLite — no text-file editing required to stand up a fresh gateway
- **Live metrics dashboard** — current value + last-uplink timestamp for every configured metric, auto-refreshing
- **Application / Device / Metric CRUD with ChirpStack inventory pickers** — build the topology by selecting applications/devices/metrics from your live ChirpStack inventory by name; changes stage to SQLite and apply together via an explicit **"Apply changes"** soft restart (see below)
- **Inventory drift view** — diff your configured inventory against ChirpStack and reconcile from the UI
- **ChirpStack status tile** — last poll outcome, cumulative error count, gateway uptime
- **Commands page** — downlink command queue + delivery-status tracking
- **HTTP basic-auth gating** — single set of credentials shared with the OPC UA server (no separate web account to manage)

### Gateway operations

- **Configurable poll cadence** — `polling_frequency` per ChirpStack, default 60s
- **Failure-isolating per-device polling** — a single failing device cannot stop the cycle for the rest
- **Auto-recovery loop** — opcgw retries ChirpStack connection on transient outages with configurable backoff
- **Staged configuration editor with explicit "Apply changes"** — edit the `[global]` / `[chirpstack]` / `[opcua]` / `[web]` settings from the web UI; edits accumulate as pending changes (`GET /api/status` reports `pending_changes: true`) and apply together via one `POST /api/config/apply` in-process soft restart of the data plane — the container is **never** restarted, so OPC UA clients aren't dropped on every individual save
- **Config export / import** — download the full configuration as portable TOML (secrets excluded) and restore it on another instance through the staged-apply flow
- **Structured JSON logs** — every operationally-meaningful event is emitted at `info` or higher with a closed-enum `event=` taxonomy suitable for SIEM / log aggregation

### Persistence

- **SQLite with WAL mode** — concurrent read/write, crash-safe
- **7-day metric history** (configurable via `[opcua].history_retention_days`) — auto-pruned by background task
- **Atomic schema migrations** — versioned (`v001`–`v012`), per-startup forward-only

## Who is this for?

| You are… | opcgw fits if… |
|---|---|
| A SCADA integrator with an existing OPC UA HMI | You need LoRaWAN devices to appear as native tags without rewriting your HMI for MQTT |
| An OT engineer running ChirpStack on-prem | You want a single deployable unit between your LoRaWAN stack and your control system |
| A facility operator | You want a web dashboard for live LoRaWAN values without standing up a separate visualization tool |
| A devops/platform team | You want a Rust binary (~30 MB) that runs as a non-root container with a small attack surface — no Node/Python runtime to keep patched |

## Supported architectures

- `linux/amd64` — x86_64 servers, VMs, industrial PCs
- `linux/arm64` — Raspberry Pi 4/5, AWS Graviton, Apple Silicon dev workstations

32-bit ARM (`linux/arm/v7`) is **not** currently published.

## Supported tags

Tags follow [Semantic Versioning](https://semver.org/) and are generated by `docker/metadata-action@v5` from the git tag that triggered the build:

| Tag pattern        | Example   | Meaning                                                            |
|--------------------|-----------|--------------------------------------------------------------------|
| `<major>.<minor>.<patch>` | `2.3.1`  | The exact release. Pin to this for fully reproducible deployments. |
| `<major>.<minor>`         | `2.3`    | Latest patch within a minor line. Auto-updates on patch releases.  |
| `latest`                  | `latest` | The newest stable release. Convenient for trying it out; pin a version for production. |
| `sha-<short>`             | `sha-7a26227` | Commit-sha-pinned build. Used for tracing back to a specific commit. |

For reproducible production deployments, pin to a specific minor (`:2.3`) or patch (`:2.3.1`) rather than `:latest`.

> **Note on `v2.0.0`**: the `2.0.0` tag exists in git history but the corresponding Docker image was never published — the v2.0.0 publishing workflow failed at schema validation due to a GitHub Actions context bug (fixed in v2.0.1). `gcorbaz/opcgw:2.0.0` will return `manifest unknown`. Use `:2.3` (recommended) or `:2.3.1` (exact) instead.

## Quick start

### `docker run`

```sh
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
  gcorbaz/opcgw:2.3
```

> ⚠️ The `data/` bind mount is **required** so the SQLite database (metric values + 7-day history) survives container restart. Without it, persisted data lives in the ephemeral container layer.

### `docker compose`

Use the bundled `docker-compose.yml` and `.env.example` from the GitHub repository — see the [Quick start in the user manual](https://github.com/guycorbaz/opcgw/blob/main/docs/manual/opcgw-user-manual.xml) for the canonical recipe.

```sh
git clone https://github.com/guycorbaz/opcgw.git
cd opcgw
cp .env.example .env
# Edit .env to set OPCGW_CHIRPSTACK__API_TOKEN + OPCGW_OPCUA__USER_PASSWORD
docker compose up -d
```

## Environment variables

opcgw stores its configuration in SQLite, seeded once from `config/config.toml` on first boot, and accepts environment-variable overrides following the `OPCGW_<SECTION>__<FIELD>` pattern (double underscore separates the section from the field; figment-driven). Precedence: env var > SQLite > `config.toml` > default.

The most commonly overridden values:

| Variable | Required | Description |
|---|---|---|
| `OPCGW_CHIRPSTACK__API_TOKEN` | **Yes** | ChirpStack API token. Generate at ChirpStack → API Keys. |
| `OPCGW_OPCUA__USER_PASSWORD` | **Yes** | Password for OPC UA basic-auth user (Story 7-2 secure store). The embedded Web UI shares these credentials — there is no separate `OPCGW_WEB__USER_PASSWORD`. |
| `OPCGW_OPCUA__USER_NAME` | No | Username for OPC UA / Web UI basic auth (default `opcua-user`). |
| `OPCGW_CHIRPSTACK__SERVER_ADDRESS` | No | ChirpStack gRPC endpoint (default per config.toml). |
| `OPCGW_OPCUA__HOST_IP_ADDRESS` | No | OPC UA listen address (default `0.0.0.0`). |
| `OPCGW_OPCUA__HOST_PORT` | No | OPC UA TCP port (default `4840`). |

The `:?err` shell guard in `docker-compose.yml` causes Compose parsing to fail fast if required env-vars are **unset or empty** — see the security guide. Note: `:?err` does **not** detect literal placeholder strings (e.g. `REPLACE_ME_WITH_…`); operators who copy `.env.example` verbatim without filling the values get a container that starts successfully and then errors at gateway startup. Replace every placeholder before `docker compose up`.

## Exposed ports

- **4840** — OPC UA endpoint (`opc.tcp://`). Configurable via `[opcua].host_ip_address` (default `0.0.0.0`) + `[opcua].host_port` (default `4840`) in `config.toml`.
- **8080** — Embedded Axum web UI. Enabled by default in the shipped `config.toml` (toggle via `[web].enabled` / `OPCGW_WEB__ENABLED`; configurable via `[web].port`). Publish with `-p 8080:8080` on `docker run` to expose it on the host.

### Quick-start variant: with the Web UI

```sh
docker run -d \
  --name opcgw \
  --restart unless-stopped \
  -p 4840:4840 \
  -p 8080:8080 \
  -e OPCGW_CHIRPSTACK__API_TOKEN='<your-chirpstack-api-token>' \
  -e OPCGW_OPCUA__USER_PASSWORD='<shared-opcua-and-web-password>' \
  -e OPCGW_WEB__ENABLED=true \
  -v "$(pwd)/config:/usr/local/bin/config" \
  -v "$(pwd)/pki:/usr/local/bin/pki" \
  -v "$(pwd)/log:/usr/local/bin/log" \
  -v "$(pwd)/data:/usr/local/bin/data" \
  gcorbaz/opcgw:2.3
```

The web UI shares OPC UA credentials (single source of truth — `OPCGW_OPCUA__USER_NAME` + `OPCGW_OPCUA__USER_PASSWORD`). Place it behind a TLS-terminating reverse proxy in production deployments.

## Volume mounts

The container's `WORKDIR` is `/usr/local/bin`. Standard bind-mount layout:

| Container path | Purpose | Required |
|---|---|---|
| `/usr/local/bin/config` | `config.toml` (bootstrap seed) + `secrets.toml` (chmod `0600`) | Yes |
| `/usr/local/bin/pki` | OPC UA PKI directory (`own/`, `private/`, `trusted/`, `rejected/`) | Yes (unless security = None) |
| `/usr/local/bin/log` | Log files (tracing; level via `RUST_LOG`) | No (logs go to stdout otherwise) |
| `/usr/local/bin/data` | SQLite database + 7-day metric history | **Yes — without this mount, data is lost on every container restart** |

## Non-root operation

opcgw runs as user **`opcgw` (UID 10001)**, NOT root. Bind-mounted host directories must be readable / writable by UID 10001:

```sh
# One-time host-side ownership fix before first start:
sudo mkdir -p ./config ./pki/{own,private,trusted,rejected} ./log ./data
sudo chown -R 10001:10001 ./config ./pki ./log ./data
sudo chmod 700 ./pki/private    # NFR9 - OPC UA private key parent dir
# Tighten any pre-existing private key files (no-op on a fresh install):
sudo find ./pki/private -type f -name '*.pem' -exec chmod 600 {} +
```

## Configuration

On first boot the container reads `config/config.toml` from the bind-mounted config directory to seed its SQLite database; after that, configure the gateway from the web UI. A complete example with field-by-field documentation lives in [the user manual](https://github.com/guycorbaz/opcgw/blob/main/docs/manual/opcgw-user-manual.xml). The minimal bootstrap sections are `[chirpstack]` and `[opcua]`; applications/devices are most easily added through the web UI pickers (or seeded via `[[application]]` → `[[application.device]]` → `[[application.device.read_metric]]` sub-tables).

## Health check / verification

After `docker compose up -d` (or `docker run`), verify the gateway is running correctly:

```sh
# Process runs as UID 10001 (not root)
docker exec opcgw id

# OPC UA endpoint is bound on port 4840
nc -z localhost 4840 && echo "OPC UA endpoint reachable"

# Tail logs to watch the first ChirpStack poll cycle
docker logs -f opcgw
```

Look for the structured-log event `operation="poll_cycle_start"` within ~30 seconds of startup, followed by `operation="poll_cycle_end"` on each successful cycle. When `[opcua].diagnostics_enabled = true` is set, the periodic `event="opcua_session_count"` gauge (fired every ~5 s) additionally confirms the OPC UA server is listening; with diagnostics disabled the gauge does not fire, so use `event="opcua_limits_configured"` from startup as the alternative listener-up signal.

## Troubleshooting

| Symptom | Likely cause | Quick fix |
|---|---|---|
| Container crashes on first start with "Permission denied" creating `./log` or `./data` | Bind-mount host dirs owned by `root` / your UID, but the container runs as `10001` | `sudo chown -R 10001:10001 ./log ./data ./pki ./config` |
| `nc -z localhost 4840` returns "Connection refused" but container is running | OPC UA bound to a specific interface (default `0.0.0.0` so this is rare) | Check the startup log for `event="opcua_limits_configured"`; if absent, the OPC UA server failed to start — inspect logs for the actual error |
| `poll_cycle_end errors=N chirpstack_available=false` repeatedly | Wrong `OPCGW_CHIRPSTACK__API_TOKEN`, expired PAT, or token from a different ChirpStack instance | Generate a new tenant-scoped API key in ChirpStack UI → Tenants → your tenant → API Keys → Add. Update the env var. |
| Live Metrics page shows "Never reported" for every metric | `chirpstack_metric_name` doesn't match the codec's emitted key | Check ChirpStack UI → device → Metrics tab; the column headers are the case-sensitive keys you must put in `chirpstack_metric_name` |
| Web UI form returns "CSRF check failed: Origin header missing, null, or not in allow-list" | Browser uses `localhost` but allow-list contains only `127.0.0.1` (or vice versa) | The shipped config sets `[web].allowed_origins = ["http://127.0.0.1:8080", "http://localhost:8080"]`; adjust to match the host/port you browse to |
| `manifest unknown` on `gcorbaz/opcgw:2.0.0` | No image was ever published for v2.0.0 (workflow bug; see the "Supported tags" note above) | Pull `:2.3` (recommended) or `:2.3.1` (exact) instead |

For anything else, file an issue at <https://github.com/guycorbaz/opcgw/issues>.

## Scale & performance

Indicative numbers from a Raspberry Pi 4 / 8 GB running opcgw against a local ChirpStack v4.x:

| Dimension | Value | Notes |
|---|---|---|
| Memory footprint | ~30 MB RSS, ~80 MB peak with web UI enabled | Rust binary, no runtime |
| Image size | ~75 MB compressed | Multi-stage build, runtime is ubuntu:24.04 + the static-linked binary |
| Poll cycle latency | < 15 ms per cycle, single-digit ms per device | gRPC over loopback; LAN-attached ChirpStack adds RTT |
| OPC UA Read latency | < 1 ms (in-memory) | Values served from the SQLite latest-value path |
| OPC UA HistoryRead | bounded by `[opcua].max_history_data_results_per_node` (default 10000) | Manual pagination via follow-up calls |
| Sustained device count | tested up to ~200 devices × 5 metrics each per gateway | Above this scale, consider tuning `polling_frequency` or sharding |

## Upgrading from v2.0-rc

Use the [migration runbook](https://github.com/guycorbaz/opcgw/blob/main/docs/deployment-guide.md) at `docs/deployment-guide.md § "Epic A migration"`. The repository ships [`scripts/check-schema-version.sh`](https://github.com/guycorbaz/opcgw/blob/main/scripts/check-schema-version.sh) as a pre-flight tool that inspects an existing SQLite file and recommends Path A (in-place upgrade) or Path B (drop-and-recreate). Migration is one-way; restoring from a pre-upgrade backup is the only rollback.

## Links

- **Source code:** <https://github.com/guycorbaz/opcgw>
- **Full user manual:** <https://github.com/guycorbaz/opcgw/blob/main/docs/manual/opcgw-user-manual.xml> (DocBook 4.5 XML; build to HTML/PDF via `docs/manual/Makefile`)
- **Deployment guide:** <https://github.com/guycorbaz/opcgw/blob/main/docs/deployment-guide.md>
- **Security guide:** <https://github.com/guycorbaz/opcgw/blob/main/docs/security.md>
- **Changelog:** <https://github.com/guycorbaz/opcgw/blob/main/CHANGELOG.md>
- **GHCR mirror:** <https://github.com/guycorbaz/opcgw/pkgs/container/opcgw>
- **Issue tracker:** <https://github.com/guycorbaz/opcgw/issues>

## License

Dual-licensed under either:

- MIT License — <https://github.com/guycorbaz/opcgw/blob/main/LICENSE-MIT>
- Apache License 2.0 — <https://github.com/guycorbaz/opcgw/blob/main/LICENSE-APACHE>

at your option.

---
layout: default
title: Architecture & Design
permalink: /architecture/
---

# Architecture Documentation — opcgw

> Last updated: 2026-05-27 (Story D-2: figment Provider rework + TOML mutation-surface decommission — Epic D 3/3 done)

## Executive Summary

opcgw is a Rust-based gateway that bridges ChirpStack 4 (LoRaWAN Network Server) with OPC UA industrial automation clients. It runs concurrent async tasks — a ChirpStack gRPC poller, an OPC UA server, and an embedded web UI — that communicate through shared in-memory state backed by a SQLite database.

**Configuration architecture (post-Story D-2 — final three-surface model):** opcgw has exactly three persistence surfaces for configuration:

1. **SQLite** (`data/opcgw.db`, chmod 0o600) — **authoritative** for all non-secret runtime configuration. Holds the `[[application]]` tree (Story C-6) AND the four singleton sections `[global]`, `[chirpstack]`, `[opcua]`, `[web]` (Story D-0, `singleton_config` K/V table at schema v010). The web UI singleton editor (Story D-1) writes through `SqliteBackend::write_singleton_section`; the application CRUD endpoints write through the `applications` / `devices` / `metrics` / `commands` tables.
2. **`config/secrets.toml`** (chmod 0o600 via atomic-rename, established by Story C-0) — operator-supplied secrets: `[chirpstack].api_token` + `[opcua].user_password`. Read at boot via figment's secrets.toml provider; opcgw never mutates this file at runtime.
3. **`config.toml`** — **bootstrap-seed-only**. Read at boot via figment's primary TOML provider; values OVERRIDDEN by SQLite for any key the singleton snapshot has set (Story D-2's `SqliteSingletonProvider`). Operators MAY delete `config.toml` post-migration; opcgw boots cleanly from SQLite + `secrets.toml` alone.

**Figment Provider stack (final precedence ordering, top = highest priority):**

1. `Env::prefixed("OPCGW_").split("__")` — env-var overrides
2. `SqliteSingletonProvider` (Story D-2) — non-secret runtime config from SQLite
3. `Toml::string(secrets.toml)` — secret fields only
4. `Toml::file(config.toml)` — bootstrap seed (lowest, default-overridable)
5. `#[serde(default = "...")]` — struct defaults

This delivers proper `env > SQLite > TOML > default` precedence as a structural figment guarantee. The post-D-1 Arc::make_mut overlay is gone — figment produces the correct AppConfig directly on every load.

**Operator-facing impact of the three-surface model:** Hand-edits to `config.toml` for keys covered by `singleton_config` are silently shadowed by the SQLite values. To surface that confusion, opcgw emits `config_toml_unused_warning` once-per-boot when `config.toml` is present alongside a populated `singleton_config` table. The runbook at `docs/d-0-migration-runbook.md` documents the explicit operator workflow for verifying-and-optionally-deleting `config.toml` post-migration.

**toml_edit dependency:** intentionally absent from the dependency tree. Story 9-4's `src/web/config_writer.rs` (the only `toml_edit` consumer in production code) was decommissioned by Story C-6 + the residual import path in `src/config.rs`'s secrets.toml pre-validator was rewritten to use figment's own TOML parser by Story D-2. The dep tree contains `toml 0.8.x` transitively via figment's `toml` feature only.

## System Architecture

```
┌──────────────────────────────────────────────────────────────────────────┐
│                              opcgw Process                               │
│                                                                          │
│  ┌──────────────────┐   ┌──────────────┐   ┌──────────────────────────┐  │
│  │  ChirpstackPoller│   │   SQLite DB  │   │      OPC UA Server       │  │
│  │  (tokio task)    │   │              │   │      (tokio task)         │  │
│  │                  │   │ metric values│   │                          │  │
│  │  - poll_metrics()│   │ applications │   │  - read/write callbacks  │  │
│  │  - store_metric()│   │ devices      │   │  - address space builder │  │
│  │  - process_cmds()│   │ metrics      │   │  - subscription cache    │  │
│  └────────┬─────────┘   │ commands     │   └──────────────────────────┘  │
│           │             └──────┬───────┘              │                  │
│           │                   │                       │                  │
│  ┌────────┴────────────────────┴───────────────────────┴───────────────┐  │
│  │           In-memory snapshot (Arc<watch::Sender<Arc<AppConfig>>>)   │  │
│  │  Rebuilt from SQLite on every CRUD write (notify_crud_write).       │  │
│  │  Read by poller, OPC UA, and web dashboard (dashboard_snapshot).    │  │
│  └────────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌───────────────────────────────────────────────────────────────────┐   │
│  │                     Embedded Web Server (axum)                    │   │
│  │  CRUD API: POST/PUT/DELETE /api/applications|devices|commands     │   │
│  │  Writes → SQLite → notify_crud_write → in-memory snapshot rebuilt │   │
│  │  ChirpStack inventory: GET /api/inventory/*  (C-1 TTL cache)      │   │
│  │  Drift view: GET /api/inventory/drift  (C-4)                      │   │
│  └───────────────────────────────────────────────────────────────────┘   │
└──────────────────────────────────────────────────────────────────────────┘
            │                                            │
            ▼                                            ▼
  ┌───────────────────┐                       ┌───────────────────┐
  │  ChirpStack 4     │                       │  OPC UA Clients   │
  │  gRPC API         │                       │  (FUXA, etc.)     │
  └───────────────────┘                       └───────────────────┘
```

## Storage Architecture (post-Story C-6)

SQLite is the **single authoritative store** for all opcgw state:

| Data category            | SQLite table(s)                              | Write path                        |
|--------------------------|----------------------------------------------|-----------------------------------|
| Metric values (live)     | `device_metrics`                             | ChirpStack poller                 |
| Metric history           | `device_metrics` (time-series rows)          | ChirpStack poller                 |
| Applications             | `applications`                               | Web UI CRUD (`notify_crud_write`) |
| Devices                  | `devices`                                    | Web UI CRUD (`notify_crud_write`) |
| Metric mappings          | `metrics`                                    | Web UI CRUD (`notify_crud_write`) |
| Commands                 | `commands`                                   | Web UI CRUD (`notify_crud_write`) |
| Gateway status           | `gateway_status`                             | ChirpStack poller                 |
| Schema version           | `meta`                                       | Migration runner at boot          |

The in-memory snapshot (`Arc<watch::Sender<Arc<AppConfig>>>`) is rebuilt from SQLite after every CRUD write via `ConfigReloadHandle::notify_crud_write`. Subscribers (OPC UA address-space builder, ChirpStack poller, web dashboard) receive the new snapshot through the watch channel.

> **Apply model (Story F-0, 2026-06-14).** The live `notify_crud_write` rebuild above is **dormant** under the F-0 staged-apply model. Every config-write surface — the singleton-config editor *and* the application/device/metric/command CRUD handlers — now **stages** to SQLite (`AppState::stage_config_write` bumps a `pending_gen` counter and emits `config_staged`) without mutating the running gateway. `GET /api/status` reports `pending_changes: true` until the operator applies. A single `POST /api/config/apply` wakes the in-process restart supervisor (see below), which re-reads the effective config from SQLite and soft-restarts the data-plane once for the whole batch. The legacy live-reload path is kept compiled-but-idle in F-0; its full removal is an F-0 follow-up.

### In-process data-plane restart supervisor (Story F-0)

`main.rs` runs a `'supervisor` loop that owns the data-plane lifecycle. The web server, `AppState`, and the SQLite connection pool are constructed **once, outside** the loop. Each loop iteration calls `spawn_data_plane()` to build a fresh `Barrier::new(2)` and spawn the poller, OPC UA server, gRPC event-ingestion task, and command-timeout handler — preserving the deadlock-safe spawn order (poller **before** `restore_barrier.wait()`, OPC UA server **after**) on **every** cycle. The loop then `select!`s on three signals:

- **SIGINT / SIGTERM** → cancel the parent token, join the data-plane (bounded), close the pool, exit the process (the only path that exits — and only on a real OS signal).
- **`apply_signal`** (fired by `POST /api/config/apply`) → re-read the effective config from SQLite *first* (a bad read is non-disruptive: log `apply_failed` and keep the current data-plane running), then cancel the per-cycle `restart_token`, join the data-plane (bounded 10 s), and `continue` the loop to respawn with the new config. Emits `apply_requested` → `apply_completed`.

Because `spawn_data_plane()` recomputes `streamed_devices(&config)` from the freshly-read config each cycle, the gRPC uplink-stream **scope** is re-derived on every Apply — this is the mechanism that closes CR #138 (the stream scope is no longer frozen at boot). The Docker container is never restarted; OPC UA clients disconnect and reconnect once per applied batch.

## Module Breakdown

### `main.rs` — Entry Point

- Parses CLI arguments via `clap` (`-c` config path)
- Initializes structured logging (tracing + tracing-subscriber) from `config/log4rs.yaml`
- Loads `AppConfig` from TOML + `OPCGW_*` environment variables (figment)
- Runs SQLite schema migrations (`src/storage/schema.rs`)
- Runs one-shot TOML→SQLite data migration if needed (`src/storage/migrate_config.rs`)
- Creates `ConfigReloadHandle` + `Arc<SqliteBackend>`
- Spawns ChirpStack poller, OPC UA server, embedded web server, config listeners as tokio tasks
- Awaits all tasks with graceful shutdown via `CancellationToken` + `tokio::try_join!`

### `chirpstack.rs` — ChirpStack Poller (~1225 lines)

**Responsibility:** Polls ChirpStack gRPC API for device metrics at configurable intervals and processes outbound device commands.

**Key types:**
- `ChirpstackPoller` — Main polling service, holds config + `Arc<SqliteBackend>`
- `AuthInterceptor` — Injects Bearer token into gRPC requests
- `ApplicationDetail`, `DeviceListDetail`, `DeviceMetric` — API response DTOs

**Data flow:**
1. `run()` loops forever, calling `poll_metrics()` every `polling_frequency` seconds
2. `poll_metrics()` first processes the command queue, then iterates all configured devices
3. For each device: calls `get_device_metrics_from_server()` → `store_metric()`
4. `store_metric()` converts ChirpStack metric values to typed `MetricValue` and writes to SQLite
5. Server availability is checked via TCP connection before each gRPC call, with retry logic

**Command processing (downlink path, Story E-0 + E-3):**
- `process_command_queue()` drains the `DeviceCommand` queue (fed by the OPC UA `set_command` write path) one by one.
- `deliver_one()` maps each command to a semantic object (class-bound, e.g. valve `1`→`{"command":"open"}`) or raw bytes, enqueues it via the `DownlinkSink` (`DeviceService.Enqueue`), and on success calls `mark_command_sent(id, result_id)` — persisting ChirpStack's returned **queue-item id** (`EnqueueDeviceQueueItemResponse.id`) as the command's `chirpstack_result_id`. This is the correlation key for delivery confirmation.

**Command lifecycle:** `Pending → Sent → Confirmed | Failed`.
- `Pending → Sent`: on successful enqueue (id captured).
- `Sent → Confirmed`: **event-driven** (Story E-3). ChirpStack delivers the device's downlink ack as an `ack` event on the **same** `InternalService.StreamDeviceEvents` stream the uplink consumer (`chirpstack_events.rs`) already runs; `handle_ack()` correlates `queue_item_id == chirpstack_result_id` and confirms. There is no per-command ack-polling gRPC — the signal is the event.
- `Sent → Failed`: a device NACK (`ack` with `acknowledged=false`), or the `CommandTimeoutHandler` sweep when no ack arrives within `[global].command_delivery_timeout_secs` (the terminal path for **unconfirmed** downlinks). `txack` (gateway transmitted) is diagnostic only and never confirms.
- `CommandStatusPoller` no longer polls ChirpStack; it is a lightweight reconciliation/observability heartbeat over the confirmation backlog. All transitions are idempotent (storage guards `status IN ('Sent','Pending')`) so replayed ack events on stream reconnect cannot regress a terminal command.
- Command status is exposed read-only on OPC UA via the `CommandStatusQuery` variable (recent commands + status + sent/confirmed timestamps as JSON).

### `chirpstack_events.rs` — Uplink Event Ingestion (Story E-1, #130)

**Responsibility:** Expose each device value as its **raw last-known value** with the device's **source timestamp** — no gateway-side aggregation. Aggregation/trending is the SCADA's job; the `GetMetrics` poll time-aggregates (Gauge→avg, Absolute→sum) and therefore cannot faithfully carry discrete state (e.g. a valve's `valveStatusCode` aggregates to a nonsense `391`).

**Data flow:**
1. `run_event_ingestion()` (a tokio task spawned from `main.rs`) supervises one long-lived stream per streamed device: every **valve-class** device (`command_class = "valve"`), plus — when `chirpstack.stream_all_devices` is set — every device with configured read metrics.
2. Each `run_device_stream()` opens `InternalService.StreamDeviceEvents` (reusing the `chirpstack_inventory` consumer pattern) and reconnects with capped-exponential backoff; it honours the shared `CancellationToken`. The gRPC stream sits behind the injectable `UplinkSource` / `UplinkStream` trait seam (mirroring E-0's `DownlinkSink`) so reconnect/backfill/precedence are tested without a live ChirpStack.
3. On every (re)connect, `backfill_device()` fetches the device's newest recent event via the bounded `chirpstack_inventory::stream_recent_device_uplinks` fetch (never `GetMetrics`) and stores it under the `is_fresher` timestamp guard — a backfill can never overwrite a newer live value, and a value is present before the next live event.
4. `map_uplink_to_writes()` (pure, testable) maps each configured `read_metric` whose `chirpstack_metric_name` is present in the decoded `object` to a `BatchMetricWrite` stamped with the device event time (`LogItem.time`) — the value verbatim, never aggregated.
5. `poll_metrics()` **skips streamed devices** so the stream is the sole, authoritative writer for them. The OPC UA read path exposes `MetricValue.timestamp` as the DataValue `source_timestamp`; staleness quality is computed from real device-report age (per-device `stale_threshold_seconds` override, #132).
6. **Shared with Story E-3:** the same `StreamDeviceEvents` consumer also dispatches `ack` / `txack` device events (not just `up`). `handle_ack()` correlates a downlink delivery ack to its queued command and advances the command lifecycle — so command delivery confirmation rides this one stream, with no second subscription (see "Command lifecycle" under `chirpstack.rs`). Valve-class devices are streamed, so valve command confirmation works out of the box; a command on a device that is not streamed relies on the timeout sweep.

### `storage/` — Storage Layer

**Responsibility:** All persistent state — metric values, application configuration, and gateway status.

**Key types:**
- `SqliteBackend` — Primary backend; wraps `ConnectionPool` (WAL mode, per-task connections)
- `ConnectionPool` — `Arc<ConnectionPool>` manages multiple SQLite connections (Story 2-2x)
- `StorageBackend` trait — Abstraction for `SqliteBackend` and `InMemoryBackend` (tests)
- `MetricValueInternal` — Typed metric value: `Float(f64)`, `Int(i64)`, `Bool(bool)`, `String(String)`
- `migrate_config.rs` — One-shot TOML→SQLite migration logic (Story C-6)
- `schema.rs` — Schema version constants and `run_migrations()` dispatcher

**Application-config CRUD methods on `SqliteBackend`:**
- `insert_application`, `update_application`, `delete_application`
- `insert_device_with_metrics`, `update_device`, `delete_device`
- `insert_command`, `update_command`, `update_command_by_id`, `delete_command`
- `load_all_applications_config()` — Reconstructs `Vec<ChirpStackApplications>` from the four config tables; called after every CRUD write

**Concurrency:** Each async task gets its own connection from the pool via `pool.checkout()`. SQLite WAL mode enables true concurrent readers with single writer — no Rust-level Mutex bottleneck.

### `opc_ua.rs` — OPC UA Server (~873 lines)

**Responsibility:** Exposes device metrics as an OPC UA 1.04 server using `async-opcua`.

**Key type:** `OpcUa` — Holds config, storage ref, host IP/port.

**Server setup (`create_server`):**
1. Builds server via `ServerBuilder` with application identity, network, PKI, user tokens, endpoints
2. Creates `SimpleNodeManager` with custom namespace `urn:UpcUaG`
3. Calls `add_nodes()` to populate address space from SQLite-backed in-memory snapshot

**Address space structure:**
```
Objects/
├── {Application_Name}/           (folder)
│   ├── {Device_Name}/            (folder)
│   │   ├── {Metric_Name}         (variable, read callback)
│   │   ├── {Command_Name}        (variable, read+write, writable)
│   │   └── ...
│   └── ...
└── ...
```

**Read path:** Read callbacks → `get_value()` → SQLite metric store → `convert_metric_to_variant()`
**Write path:** Write callbacks → `set_command()` → creates `DeviceCommand` → pushed to SQLite command queue

**Security endpoints:**
- `null` — No security (development)
- `basic256_sign` — Basic256 Sign (security level 3)
- `basic256_sign_encrypt` — Basic256 SignAndEncrypt (security level 13)

### `config.rs` — Configuration (~913 lines)

**Responsibility:** Deserialise `config.toml` singleton sections; define the `AppConfig` struct tree used throughout the codebase.

**Key types:**
- `AppConfig` — Top-level: `Global`, `ChirpstackPollerConfig`, `OpcUaConfig`, `Vec<ChirpStackApplications>`
- `ChirpStackApplications` — `application_name`, `application_id`, `Vec<ChirpstackDevice>`
- `ChirpstackDevice` — `device_id`, `device_name`, `Vec<ReadMetric>`, `Option<Vec<DeviceCommandCfg>>`
- `ReadMetric` — `metric_name`, `chirpstack_metric_name`, `metric_type: OpcMetricTypeConfig`, optional `metric_unit`
- `DeviceCommandCfg` — `command_id`, `command_name`, `command_confirmed`, `command_port`
- `OpcMetricTypeConfig` — Enum: `Bool`, `Int`, `Float`, `String`

**Loading:** `Figment::new().merge(Toml::file(...)).merge(Env::prefixed("OPCGW_"))` with `CONFIG_PATH` env override.

**Post-C-6 note:** The `application_list` field of `AppConfig` is populated from TOML at boot (bootstrap seed / one-shot migration source), but at runtime the authoritative `[[application]]` state lives in SQLite. `SqliteBackend::load_all_applications_config()` reconstructs the `Vec<ChirpStackApplications>` from SQLite for the in-memory snapshot after every CRUD write.

### `config_reload.rs` — Configuration Watch Channel (~2000 lines)

**Responsibility:** Owns the `tokio::sync::watch::Sender<Arc<AppConfig>>` propagation channel so all subsystems observe the live config.

**Key type:** `ConfigReloadHandle` — wraps the watch sender; provides:
- `subscribe()` — returns a `Receiver` clone for a subscriber
- `notify_crud_write(new_apps)` — atomically swaps the `application_list` in the channel after a SQLite CRUD write; emits `event="config_reload" trigger="crud_write"` audit log

**Listener functions:**
- `run_web_config_listener()` — rebuilds `DashboardConfigSnapshot` on each channel update
- `run_opcua_config_listener()` — triggers OPC UA address-space diff-apply on each channel update

**Note:** The SIGHUP-triggered TOML reload path (Story 9-7) was removed in Story C-6. Config changes to the application tree are now exclusively driven by web UI CRUD writes.

### `web/` — Embedded Web Server (~6000 lines total)

**Responsibility:** HTTP management API + static web UI for configuration, inventory, and audit.

**Key modules:**
- `api.rs` — All CRUD handlers (applications, devices, metrics, commands); ChirpStack inventory proxy; audit endpoints
- `auth.rs` — HTTP Basic Auth middleware; `OpcgwAuthManager`
- `csrf.rs` — CSRF token generation + validation per resource bucket
- `setup.rs` — First-run password wizard (Story C-0)
- `inventory.rs` — ChirpStack inventory proxy with TTL cache (Story C-1)
- `drift.rs` — Inventory drift computation (Story C-4)
- `mod.rs` — `AppState`, route wiring, embedded static files

**CRUD write path (post-C-6):**
```
POST /api/applications/:id/devices
  → validate body
  → sqlite_config.insert_device_with_metrics(...)
  → sqlite_config.load_all_applications_config()
  → config_reload.notify_crud_write(all_apps)   ← rebuilds in-memory snapshot
  → emit audit event
  → 201 Created
```

### `utils.rs` — Utilities (~365 lines)

**Constants:**
- `OPCUA_ADDRESS_SPACE` = `"urn:chirpstack_opcua"`
- `OPCUA_NAMESPACE_URI` = `"urn:UpcUaG"`
- `OPCUA_DEFAULT_PORT` = 4840
- `OPCGW_CONFIG_PATH` = `"config"`
- `OPCGW_CP_*` — ChirpStack monitoring constants

**Error type:** `OpcGwError` enum with variants: `Configuration`, `ChirpStack`, `OpcUa`, `Storage`, `Database` — using `thiserror`.

## Build System

**`build.rs`** compiles 10 ChirpStack API `.proto` files from `proto/chirpstack/api/` using `tonic_build::configure().build_server(true).compile_protos(...)`. The generated Rust code provides typed gRPC client stubs.

**`Makefile.toml`** (cargo-make) defines:
- `tests` — clean + cargo test
- `cover` — instrumented build + grcov HTML coverage report

## Deployment

**Docker:** Multi-stage build (`rust:1.87` builder → `ubuntu:latest` runtime). Exposes ports 4855 (OPC UA) and 8080 (web UI). Mounts `log/`, `config/`, `pki/`, `data/` as volumes.

**docker-compose.yml:** Single service `opcgw`, restart always, ports 4855:4855 + 8080:8080.

## Testing Strategy

- **Unit tests** in individual source modules via `#[cfg(test)]`
- **Integration tests** in `tests/` covering CRUD APIs, authentication, inventory, drift, migration
- `cargo test --all-targets` (≥ 1480 tests passing as of Story C-6)
- `cargo clippy --all-targets -- -D warnings` clean

## Known Architectural Considerations

1. **Incomplete OPC UA feature set:** The OPC UA server currently supports basic Browse/Read/Write/History. Alarms and conditions, complex type support, and monitored items tuning are not yet implemented.
2. **Configuration architecture (post-D-2):** All non-secret runtime configuration is authoritative in SQLite. The four singleton sections (`[global]` / `[chirpstack]` / `[opcua]` / `[web]`) live in the `singleton_config` K/V table and are read at boot via the `SqliteSingletonProvider` figment Provider. `config.toml` is a bootstrap seed only — figment continues to load it but SQLite values override on every key the singleton snapshot has set. Most singleton knobs are restart-required (the `Arc<AppConfig>` snapshot is captured at boot); see issue #113 for the live-borrow refactor that would enable true hot-reload of PKI paths / ports / allowed_origins.
3. **SQLite is single-process:** The current connection pool assumes a single opcgw process per SQLite file. Multi-process deployments (active-active HA) require an alternative backend.
4. **Linear config lookups:** `get_device_name()`, `get_metric_type()` etc. do O(n) scans over the in-memory snapshot — acceptable for < 1000 devices but not designed for large-scale deployments.
5. **Single metric type support:** Only ChirpStack "Gauge" metric type is supported; Counter, Absolute, Unknown are not handled.
6. **Command queue is LIFO:** `Vec::pop()` processes most-recent command first — may need FIFO semantics (`VecDeque`) for strict ordering.

---
layout: default
title: Architecture & Design
permalink: /architecture/
---

# Architecture Documentation — opcgw

> Last updated: 2026-05-25 (Story C-6: SQLite as authoritative configuration store)

## Executive Summary

opcgw is a Rust-based gateway that bridges ChirpStack 4 (LoRaWAN Network Server) with OPC UA industrial automation clients. It runs concurrent async tasks — a ChirpStack gRPC poller, an OPC UA server, and an embedded web UI — that communicate through shared in-memory state backed by a SQLite database.

**Configuration architecture (post-Story C-6):** The `[[application]]` tree (applications, devices, metrics, commands) is stored authoritatively in SQLite. The singleton sections (`[global]`, `[chirpstack]`, `[opcua]`, `[web]`) remain in `config.toml` for v2.x. All CRUD operations go through the web UI → SQLite → in-memory snapshot path. The TOML file is read once at boot to seed the initial config; the `[[application]]` section is no longer written or watched at runtime.

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

**Command processing:**
- `process_command_queue()` drains commands from storage queue one by one
- Each command is sent to ChirpStack via `enqueue_device_request_to_server()` (DeviceQueueItem gRPC)

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
2. **Configuration architecture is intentionally layered:** Singleton sections (`[chirpstack]`, `[opcua]`, etc.) remain in TOML and require a process restart to update. A future story may move these into SQLite + an admin settings UI.
3. **SQLite is single-process:** The current connection pool assumes a single opcgw process per SQLite file. Multi-process deployments (active-active HA) require an alternative backend.
4. **Linear config lookups:** `get_device_name()`, `get_metric_type()` etc. do O(n) scans over the in-memory snapshot — acceptable for < 1000 devices but not designed for large-scale deployments.
5. **Single metric type support:** Only ChirpStack "Gauge" metric type is supported; Counter, Absolute, Unknown are not handled.
6. **Command queue is LIFO:** `Vec::pop()` processes most-recent command first — may need FIFO semantics (`VecDeque`) for strict ordering.

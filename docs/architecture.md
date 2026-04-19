---
layout: default
title: Architecture & Design
permalink: /architecture/
---

# Architecture Documentation — opcgw

> Generated: 2026-04-01 | Scan Level: Exhaustive

## Executive Summary

opcgw is a Rust-based gateway that bridges ChirpStack 4 (LoRaWAN Network Server) with OPC UA industrial automation clients. It runs two concurrent async tasks — a ChirpStack gRPC poller and an OPC UA server — that communicate through shared in-memory storage.

## System Architecture

```
┌─────────────────────────────────────────────────────────────────────┐
│                          opcgw Process                              │
│                                                                     │
│  ┌──────────────────┐    ┌──────────────┐    ┌───────────────────┐  │
│  │  ChirpstackPoller│    │   Storage    │    │    OPC UA Server  │  │
│  │  (tokio task)    │───►│ Arc<Mutex<>> │◄───│    (tokio task)   │  │
│  │                  │    │              │    │                   │  │
│  │  - poll_metrics()│    │ - devices    │    │  - read callbacks │  │
│  │  - store_metric()│    │   HashMap    │    │  - write callbacks│  │
│  │  - process_cmds()│    │ - cmd queue  │    │  - address space  │  │
│  └────────┬─────────┘    │ - CS status  │    └─────────┬─────────┘  │
│           │              └──────────────┘              │            │
└───────────┼────────────────────────────────────────────┼────────────┘
            │                                            │
            ▼                                            ▼
  ┌───────────────────┐                       ┌───────────────────┐
  │  ChirpStack 4     │                       │  OPC UA Clients   │
  │  gRPC API         │                       │  (FUXA, etc.)     │
  └───────────────────┘                       └───────────────────┘
```

## Module Breakdown

### `main.rs` — Entry Point

- Parses CLI arguments via `clap` (`-c` config path, `-d` debug level)
- Initializes `log4rs` logging from `config/log4rs.yaml`
- Loads `AppConfig` from TOML + environment variables
- Creates shared `Arc<Mutex<Storage>>`
- Spawns `ChirpstackPoller::run()` and `OpcUa::run()` as separate tokio tasks
- Awaits both with `tokio::try_join!`

### `chirpstack.rs` — ChirpStack Poller (~1225 lines)

**Responsibility:** Polls ChirpStack gRPC API for device metrics at configurable intervals and processes outbound device commands.

**Key types:**
- `ChirpstackPoller` — Main polling service, holds config + `Arc<Mutex<Storage>>`
- `AuthInterceptor` — Injects Bearer token into gRPC requests
- `ApplicationDetail`, `DeviceListDetail`, `DeviceMetric` — API response DTOs

**Data flow:**
1. `run()` loops forever, calling `poll_metrics()` every `polling_frequency` seconds
2. `poll_metrics()` first processes the command queue, then iterates all configured devices
3. For each device: calls `get_device_metrics_from_server()` → `store_metric()`
4. `store_metric()` converts ChirpStack metric values to typed `MetricType` and writes to storage
5. Server availability is checked via TCP connection before each gRPC call, with retry logic

**Command processing:**
- `process_command_queue()` drains commands from storage queue one by one
- Each command is sent to ChirpStack via `enqueue_device_request_to_server()` (DeviceQueueItem gRPC)

### `storage.rs` — In-Memory Storage (~1097 lines)

**Responsibility:** Thread-safe in-memory data store for device metrics and ChirpStack status.

**Key types:**
- `Storage` — Main store with `HashMap<String, Device>`, `ChirpstackStatus`, command queue
- `Device` — Name + `HashMap<String, MetricType>` of current metric values
- `MetricType` — Enum: `Bool(bool)`, `Int(i64)`, `Float(f64)`, `String(String)`
- `ChirpstackStatus` — `server_available: bool`, `response_time: f64`
- `DeviceCommand` — `device_id`, `confirmed`, `f_port`, `data: Vec<u8>`

**Initialization:** `Storage::new()` pre-allocates all devices and metrics from config with type-appropriate defaults (false, 0, 0.0, "").

**Thread safety:** Wrapped in `Arc<Mutex<Storage>>` at the application level. Not internally synchronized.

### `opc_ua.rs` — OPC UA Server (~873 lines)

**Responsibility:** Exposes device metrics as an OPC UA 1.04 server using `async-opcua`.

**Key type:** `OpcUa` — Holds config, storage ref, host IP/port.

**Server setup (`create_server`):**
1. Builds server via `ServerBuilder` with application identity, network, PKI, user tokens, endpoints
2. Creates `SimpleNodeManager` with custom namespace `urn:UpcUaG`
3. Calls `add_nodes()` to populate address space

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

**Read path:** Read callbacks → `get_value()` → locks storage → `get_metric_value()` → `convert_metric_to_variant()`
**Write path:** Write callbacks → `set_command()` → `convert_variant_to_metric()` → creates `DeviceCommand` → `push_command()` to storage queue

**Security endpoints:**
- `null` — No security (development)
- `basic256_sign` — Basic256 Sign (security level 3)
- `basic256_sign_encrypt` — Basic256 SignAndEncrypt (security level 13)

### `config.rs` — Configuration (~913 lines)

**Responsibility:** Load and expose hierarchical TOML configuration via figment.

**Key types:**
- `AppConfig` — Top-level: `Global`, `ChirpstackPollerConfig`, `OpcUaConfig`, `Vec<ChirpStackApplications>`
- `ChirpStackApplications` — `application_name`, `application_id`, `Vec<ChirpstackDevice>`
- `ChirpstackDevice` — `device_id`, `device_name`, `Vec<ReadMetric>`, `Option<Vec<DeviceCommandCfg>>`
- `ReadMetric` — `metric_name`, `chirpstack_metric_name`, `metric_type: OpcMetricTypeConfig`, optional `metric_unit`
- `DeviceCommandCfg` — `command_id`, `command_name`, `command_confirmed`, `command_port`
- `OpcMetricTypeConfig` — Enum: `Bool`, `Int`, `Float`, `String`

**Loading:** `Figment::new().merge(Toml::file(...)).merge(Env::prefixed("OPCGW_"))` with `CONFIG_PATH` env override.

**Lookup methods:** `get_application_name()`, `get_application_id()`, `get_device_name()`, `get_device_id()`, `get_metric_list()`, `get_metric_type()` — all linear scans over config vectors.

### `utils.rs` — Utilities (~365 lines)

**Constants:**
- `OPCUA_ADDRESS_SPACE` = `"urn:chirpstack_opcua"`
- `OPCUA_NAMESPACE_URI` = `"urn:UpcUaG"`
- `OPCUA_DEFAULT_PORT` = 4840
- `OPCUA_DEFAULT_IP_ADDRESS` = `"127.0.0.1"`
- `OPCUA_DEFAULT_NETWORK_TIMEOUT` = 5 seconds
- `OPCGW_CONFIG_PATH` = `"config"`
- `OPCGW_CP_*` — ChirpStack monitoring constants (name, availability, response time, internal device ID `"cp0"`)

**Error type:** `OpcGwError` enum with variants: `Configuration`, `ChirpStack`, `OpcUa`, `Storage` — using `thiserror`.

## Build System

**`build.rs`** compiles 10 ChirpStack API `.proto` files from `proto/chirpstack/api/` using `tonic_build::configure().build_server(true).compile_protos(...)`. The generated Rust code provides typed gRPC client stubs.

**`Makefile.toml`** (cargo-make) defines:
- `tests` — clean + cargo test
- `cover` — instrumented build + grcov HTML coverage report

## Deployment

**Docker:** Multi-stage build (`rust:1.87` builder → `ubuntu:latest` runtime). Exposes port 4855. Mounts `log/`, `config/`, `pki/` as volumes.

**docker-compose.yml:** Single service `opcgw`, restart always, port 4855:4855.

## Testing Strategy

- **Unit tests** in `config.rs` and `storage.rs` via `#[cfg(test)]` modules
- Test configuration in `tests/config/config.toml` (isolated from production)
- Tests cover: config lookup methods, storage CRUD, ChirpStack status lifecycle, command queue, panic on invalid operations
- No integration tests against real ChirpStack/OPC UA yet

## Known Architectural Considerations

1. **Incomplete OPC UA feature set:** The OPC UA server currently supports basic Browse/Read/Write. Many OPC UA features are missing: subscriptions and data change notifications, historical data access, alarms and conditions, method nodes, complex type support, and monitored items tuning. These are required for full industrial SCADA interoperability.
2. **File-only configuration:** All configuration is done via TOML files. A future web-based configuration interface is planned to allow managing applications, devices, and metric mappings without editing files and restarting the service.
3. **In-memory storage only:** All device metrics and state are stored in a `HashMap` and lost on restart. A local database (e.g., SQLite) is planned for persistent storage of metrics, configuration, and historical data.
4. **Mutex contention:** `std::sync::Mutex` used for shared storage — could become a bottleneck under heavy load. Consider `tokio::sync::Mutex` or `RwLock`.
5. **Panic behavior:** Several methods (`store_metric`, `set_metric_value`) panic on missing devices — production code should handle these gracefully.
6. **Linear config lookups:** `get_device_name()`, `get_metric_type()` etc. do O(n) scans — acceptable for small configs but won't scale to thousands of devices.
7. **Single metric type support:** Only ChirpStack "Gauge" metric type is supported; Counter, Absolute, Unknown are not handled.
8. **Command queue is LIFO:** `Vec::pop()` processes most-recent first — may need FIFO semantics (`VecDeque`).

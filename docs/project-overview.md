# Project Overview — opcgw

> Generated: 2026-04-01 | Scan Level: Exhaustive

## Purpose

**opcgw** (OPC UA ChirpStack Gateway) is a Rust application that bridges ChirpStack 4 (an open-source LoRaWAN Network Server) with OPC UA clients for industrial automation and SCADA systems. It polls device metrics from ChirpStack's gRPC API, stores them in memory, and exposes them as OPC UA variables. It also supports sending commands to LoRaWAN devices via OPC UA write operations.

The project was born from a real-world need: controlling LoRa watering valves in fruit tree orchards via a SCADA system to optimize water use.

## Technology Stack

| Category | Technology | Version | Notes |
|----------|-----------|---------|-------|
| Language | Rust | 2021 edition | Min rustc 1.87.0 |
| Async Runtime | Tokio | 1.47.1 | Full features, multi-thread |
| gRPC | Tonic | 0.13.1 | ChirpStack API client |
| Protobuf | tonic-build | 0.13.1 | Build-time proto compilation |
| ChirpStack SDK | chirpstack_api | 4.13.0 | Generated API types |
| OPC UA | async-opcua | 0.16.x | Server feature enabled |
| Configuration | Figment | 0.10.19 | TOML + env var merging |
| Serialization | Serde | 1.0.219 | Derive feature |
| CLI | Clap | 4.5.47 | Derive feature |
| Logging | log + log4rs | 0.4.28 / 1.4.0 | Per-module file appenders |
| Error Handling | thiserror | 2.0.16 | Custom error enum |
| Networking | url, local-ip-address | 2.5.7 / 0.6.5 | URL parsing, local IP detection |
| Containerization | Docker | - | Multi-stage build |

## Architecture Pattern

**Concurrent Service Architecture** — Two long-running async tasks (ChirpStack Poller and OPC UA Server) communicate through shared in-memory storage protected by `Arc<Mutex<Storage>>`.

```
ChirpStack gRPC API  ──►  ChirpstackPoller  ──►  Storage (HashMap)  ──►  OPC UA Server  ──►  OPC UA Clients
                                                       ▲                       │
                                                       └───── Command Queue ◄──┘
```

## Repository Type

**Monolith** — Single cohesive Rust crate with 6 source modules.

## Current Status (v1.0.0)

**Implemented:**
- ChirpStack gRPC polling with auth, retries, and server availability monitoring
- In-memory storage with typed metrics (Bool, Int, Float, String)
- OPC UA server with dynamic address space (Application > Device > Metric hierarchy)
- Read callbacks for real-time metric access from OPC UA clients
- Write callbacks for sending commands to LoRaWAN devices via ChirpStack
- TOML + environment variable configuration
- Docker deployment support
- Per-module logging with file appenders

**Not Yet Implemented / In Progress:**
- Many OPC UA features missing (subscriptions/data change notifications, historical data access, alarms & conditions, method nodes, complex type support, monitored items tuning)
- Full data type conversions (only Gauge metric type supported from ChirpStack)
- Web-based configuration interface (currently file-only via TOML; target: web UI for managing applications, devices, and metrics)
- Persistent storage in a local database (currently in-memory HashMap only; all data is lost on restart)
- Enhanced error handling (some methods still panic)
- Load testing and performance optimization
- Unit conversion with configurable factors
- Health check endpoints
- Comprehensive integration tests

## License

Dual-licensed under MIT OR Apache-2.0. Copyright (c) 2024 Guy Corbaz.

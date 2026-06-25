# Project Overview — opcgw

> Generated: 2026-04-01 | Scan Level: Exhaustive

## Purpose

**opcgw** (OPC UA ChirpStack Gateway) is a Rust application that bridges ChirpStack 4 (an open-source LoRaWAN Network Server) with OPC UA clients for industrial automation and SCADA systems. It polls device metrics from ChirpStack's gRPC API and ingests real-time uplink events, persisting them in a SQLite database (with an in-memory cache for fast reads) and exposing them as OPC UA variables. It also supports sending commands to LoRaWAN devices via OPC UA write operations, which are translated into LoRaWAN downlinks through ChirpStack.

The project was born from a real-world need: controlling LoRa watering valves in fruit tree orchards via a SCADA system to optimize water use.

## Technology Stack

| Category | Technology | Version | Notes |
|----------|-----------|---------|-------|
| Language | Rust | 2021 edition | Builder/MSRV rustc 1.94.0 |
| Async Runtime | Tokio | 1.50.0 | Full features, multi-thread |
| gRPC | Tonic | 0.14.5 | ChirpStack API client |
| Protobuf | tonic-build | 0.14.5 | Build-time proto compilation |
| ChirpStack SDK | chirpstack_api | 4.17.0 | Generated API types |
| OPC UA | async-opcua | 0.17.1 | Server feature enabled |
| Web Framework | axum | 0.8 | Web UI + REST API |
| Persistence | rusqlite | 0.38.0 | Bundled SQLite, schema migrations v001–v012 |
| Configuration | Figment | 0.10.19 | TOML + env var merging |
| Serialization | Serde | 1.0.228 | Derive feature |
| CLI | Clap | 4.6.0 | Derive feature |
| Logging | tracing + tracing-subscriber | 0.1.41 / 0.3.19 | Structured logging, env-filter |
| Error Handling | thiserror | 2.0.18 | Custom error enum |
| Containerization | Docker | - | Multi-stage build |

## Architecture Pattern

**Concurrent Service Architecture** — Long-running async tasks (ChirpStack poller, gRPC uplink event stream, OPC UA server, command-timeout handler, and the axum web server) communicate through shared storage. Storage is authoritative in SQLite (data persists across restarts) and fronted by an in-memory cache protected by `Arc<Mutex<Storage>>`.

```
                          ┌─── Uplink event stream (StreamDeviceEvents) ───┐
                          │                                                ▼
ChirpStack gRPC API  ──►  ChirpstackPoller  ──►  Storage (SQLite + in-memory cache)  ──►  OPC UA Server  ──►  OPC UA Clients
        ▲                                                                                       │
        │                                                                                       │
        └──── Downlink command path (ChirpStack Enqueue) ◄──── OPC UA write (command node) ◄────┘
```

The web layer (axum) stages configuration edits to SQLite; a single `POST /api/config/apply` performs an in-process soft restart of the data-plane tasks. The Docker container itself is never restarted.

## Repository Type

**Monolith** — Single cohesive Rust crate (38 source files).

## Current Status (v2.3.0)

**Implemented in v2.3.0:**
- ChirpStack gRPC polling with auth, retries, and server availability monitoring
- Real-time uplink ingestion via the gRPC `StreamDeviceEvents` stream, storing the RAW last value (no aggregation)
- Downlink command path: OPC UA write on a command node → LoRaWAN downlink via ChirpStack `Enqueue`, with command lifecycle tracking (Pending → Sent → Confirmed/Failed)
- Model-agnostic, class-aware device-class registry (`command_class`, e.g. `valve`); the Tonhe valve is the first driver
- Persistent SQLite storage (authoritative), with forward-only migrations (v001–v012) applied automatically on boot, plus an in-memory cache; data survives restarts (verified by cold-start restore)
- Typed metrics (Bool, Int, Float, String) and a dynamic OPC UA address space (Application > Device > Metric hierarchy)
- OPC UA subscriptions / data-change notifications and HistoryRead
- Zero-touch first-run wizard: configure ChirpStack server / tenant / API token and the OPC UA password entirely from the browser at `/setup` — no text-file editing
- Web-based configuration UI (unified vanilla web shell, no build step / framework / node_modules): ChirpStack inventory auto-discovery pickers and a live config editor backed by SQLite
- Staged-apply configuration model: edits stage to SQLite (`GET /api/status` → `pending_changes: true`) and apply via a single in-process soft restart
- Config export / import: `GET /api/config/export` (TOML, secrets excluded) and `POST /api/config/import`
- Health and status endpoints: `GET /api/health` and `GET /api/status` (health metrics, `pending_changes`, `poll_interval_secs`), plus a redesigned dashboard (at-a-glance health verdict, poller-stall tile, per-device freshness)
- Security hardening: HMAC web auth, OPC UA PKI, and audit events
- Configuration precedence: env > SQLite > `config.toml` (bootstrap seed, read once into SQLite) > default; secrets stored separately in `config/secrets.toml` (chmod 0600)
- Structured logging via `tracing` / `tracing-subscriber`
- Docker deployment support

**Deferred / backlog:**
- OPC UA threshold-alarm conditions (alarms & conditions) — not implemented
- Dispatch decoupling of the command path (CR #136)
- A second device class beyond the valve driver (E-2b)

## License

Dual-licensed under MIT OR Apache-2.0. Copyright (c) 2024 Guy Corbaz.

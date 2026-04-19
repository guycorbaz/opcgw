---
title: "Product Brief Distillate: opcgw"
type: llm-distillate
source: "product-brief-opcgw.md"
created: "2026-04-01"
purpose: "Token-efficient context for downstream PRD creation"
---

# Product Brief Distillate: opcgw

## Code-Level Issues (Phase A Targets)

- **Panics in production paths:** `storage.rs:set_metric_value()` panics if device_id not found. `chirpstack.rs:store_metric()` panics if device name lookup fails. `opc_ua.rs:convert_metric_to_variant()` uses `try_into().unwrap()` for i64→i32 conversion — overflow crashes gateway. All must become `Result` returns with graceful degradation.
- **LIFO command queue:** `storage.rs` uses `Vec::push()`/`Vec::pop()` — processes newest command first. Must switch to `VecDeque` for FIFO semantics. Affects `push_command()`, `pop_command()`, `get_device_command_queue()`.
- **Hardcoded pagination:** `chirpstack.rs:get_applications_list_from_server()` and `get_devices_list_from_server()` both use `limit: 100, offset: 0`. Deployments with 100+ apps or devices silently lose data. Need pagination loop.
- **Only Gauge metric type:** `chirpstack.rs:store_metric()` only extracts `datasets[0].data[0]` — works for Gauge but not Counter (cumulative), Absolute (instantaneous counter), or Unknown. String metric conversion logged as "not implemented".
- **Mutex contention:** `Arc<std::sync::Mutex<Storage>>` blocks the async runtime on every read/write. For current scale (1-2 clients, ~100 devices) acceptable. Monitor if scaling to 500 devices — consider `tokio::sync::RwLock` if profiling shows contention.
- **Linear config lookups:** `config.rs` methods `get_device_name()`, `get_metric_type()`, `get_metric_list()` all do O(n) scans over config vectors. Acceptable for ~100 devices; add HashMap index if scaling to 500+.
- **No stale-data indication:** When ChirpStack goes unreachable, OPC UA variables keep serving last-known values with no indication they're stale. Need OPC UA status codes (e.g., `UncertainLastUsableValue`) and source timestamps to signal staleness.
- **No gateway self-monitoring:** No health metrics exposed. Operators can't detect silent polling failures. Add OPC UA variables: last successful poll timestamp, consecutive error count, ChirpStack connection state.
- **API token in config file:** `config/config.toml` contains plain-text `api_token`. Already supports env var override (`OPCGW_CHIRPSTACK__API_TOKEN`) but default config template should not contain real tokens.
- **No input validation on OPC UA writes:** `opc_ua.rs:set_command()` accepts any Int32 value without range checking. `command.f_port` validated (>= 1) but `data` payload not validated.

## Rejected Ideas & Scope Decisions

- **REJECTED: OPC UA PubSub** — Modern push-based model considered too complex for initial scope. Pull/poll model retained. Revisit only if scalability demands grow significantly.
- **REJECTED: Cloud connectivity / multi-gateway clustering** — Explicitly out of scope. System stays on-premises, single-instance. Docker restart policy provides basic recovery.
- **REJECTED: Multi-tenant support** — Single-tenant only. One ChirpStack tenant per gateway instance.
- **REJECTED: Mobile app** — No mobile configuration or monitoring app planned.
- **REJECTED: Industrial certifications** — No IEC 61508, ISO 26262, or similar. Project is open-source for personal use and community.
- **REJECTED: OPC Foundation formal engagement** — Not pursuing standards body participation, certification programs, or reference implementation status. However, OPC Foundation specifications (especially January 2026 LoRaWAN support announcement) should be monitored and followed where practical to maintain alignment with emerging standards.
- **REJECTED: Commercial SCADA vendor partnerships** — Not pursuing ProSoft, HMS, Siemens, etc. Open-source community model only.
- **REJECTED: Frontend framework for web UI** — No React/Svelte/Vue. Web config interface will be lightweight embedded static HTML pages served by the gateway, with dynamic messaging where needed (e.g., WebSocket or SSE for status updates).
- **ACCEPTED: SQLite for persistence** — Preferred over heavier databases. Evaluate write performance under load (100 devices * N metrics * polling_frequency). Define retention policy (7 days target).
- **ACCEPTED: Subscriptions as top OPC UA priority** — Before historical data or alarms. Most immediately useful for SCADA clients.

## Requirements Hints

- **Failure-mode behavior must be explicit:** When ChirpStack is down, SCADA must see stale-data indicators, not silently stale values. OPC UA status codes + timestamps required.
- **Hot-reload for configuration:** Web UI config changes must apply without restarting the gateway or dropping active OPC UA client connections. Atomic config updates with fallback on failure.
- **Command ordering matters:** Irrigation valve open/close commands must execute in order issued. FIFO is non-negotiable.
- **Data migration v1→v2:** Users running v1 in production (in-memory) must have a documented upgrade path to v2 (SQLite) without losing current configuration. No data to migrate (in-memory is volatile), but config format changes must be handled.
- **Historical data queryable:** Phase B persistence must support OPC UA Historical Access — not just "store values" but expose them via standard OPC UA HA services.
- **Health metrics in OPC UA:** Self-monitoring must be accessible to SCADA clients without separate monitoring infrastructure. Expose as variables in the OPC UA address space under a dedicated "Gateway" or "Diagnostics" folder.

## Technical Context & Constraints

- **Rust 2021 edition, min rustc 1.87.0** — No plans to lower minimum version.
- **async-opcua v0.16.x** — Newer library, smaller community than locka99/opcua. Monitor upstream. Provides `SimpleNodeManager` with read/write callbacks. Subscription support status needs investigation for Phase B.
- **chirpstack_api v4.13.0** — Generated gRPC stubs. ChirpStack 4 API. Services used: ApplicationService.List, DeviceService.{List, GetMetrics, Enqueue}.
- **Figment v0.10.19** — Config merging (TOML + env vars). No hot-reload support built-in. Phase B web UI will need a separate config management layer.
- **Docker deployment:** Multi-stage build, rust:1.87 → ubuntu:latest. Port 4855 (non-standard, differs from OPC UA default 4840). GitHub Actions CI builds + pushes to Docker Hub on release tags.
- **OPC UA namespace:** `urn:UpcUaG` (address space), `urn:chirpstack_opcua` (server identity). Three endpoints: null (dev), basic256_sign, basic256_sign_encrypt.
- **Target deployment:** Linux primarily. Raspberry Pi to Docker on x86_64. Windows/macOS not tested or targeted.
- **Real-world scale:** 1-2 SCADA clients (FUXA), ~100 LoRaWAN devices across 4 applications (Arrosage, Batiments, Cultures, Meteo). Device count growing quickly.

## Competitive Landscape

- **No direct open-source competitor** for ChirpStack → OPC UA. Niche is uncontested.
- **Node-RED + node-red-contrib-iiot-opcua:** Generic flow-based approach. Requires manual scripting for ChirpStack device mapping. No structured gateway abstraction. Browser-based config is a UX advantage opcgw should match with its web UI.
- **ThingsBoard IoT Gateway:** Multi-protocol (Modbus, OPC-UA, MQTT, BLE). No ChirpStack-specific integration. Heavy, cloud-oriented.
- **ProSoft PLX32, HMS Anybus, Matrikon:** Commercial proprietary gateways. Modbus/EtherNet-IP focused, not LoRaWAN. Expensive, closed-source.
- **ChirpStack native integrations:** MQTT publishing + GraphQL queries. No OPC UA bridge. Users must build their own adapter stack.
- **Commercial LoRaWAN-to-OPC-UA converters exist** (ADF Web, CONSTEEL Electronics, oeeTechTools) — proprietary, hardware-based, not software gateways. Different market segment but validate the demand.

## Market & Timing Context

- **OPC Foundation + LoRa Alliance:** January 2026 announcement of official OPC UA LoRaWAN support. Validates opcgw's integration model. Risk: if formal spec diverges from opcgw's architecture, refactoring may be needed. **Action: monitor OPC Foundation specifications and align where practical.**
- **Smart agriculture LoRaWAN:** Transitioning from pilots to production (2026-2027). 50% water savings demonstrated in commercial deployments. Dense sensor arrays + edge decision engines = opcgw's exact architecture.
- **SCADA market:** US market projected $4.73B by 2030. OPC UA emerging as unifying standard between legacy SCADA and modern IoT.
- **IIoT security:** Attacks increased 75% over past two years. Gateways are primary attack vectors. Security hardening in Phase A is not optional.
- **Rust in IIoT:** Niche but growing. Two OPC UA crates exist (locka99/opcua, async-opcua). No dominant Rust IIoT gateway framework — opportunity for opcgw to be the reference.

## User Profile

- **Primary user:** Guy Corbaz — solo developer, owns the production deployment (fruit orchards, irrigation control).
- **Skill level:** Intermediate Rust developer. Learning Rust through this project. Code has extensive doc comments but some non-idiomatic patterns (e.g., `unwrap()` in production paths, `&String` parameters instead of `&str`).
- **Motivation:** Practical — control irrigation valves via SCADA. Also learning Rust. Open-sourced for community benefit, not commercial intent.
- **SCADA tool:** FUXA (open-source, web-based). Only client tested. Phase B should validate against at least one other (e.g., Ignition, RapidSCADA, or a commercial client).

## Open Questions for PRD

- **SQLite vs alternatives:** Is SQLite sufficient for 100+ devices * N metrics at 10-second polling? Need write throughput estimate. Alternative: sled (embedded Rust DB) — simpler integration but less mature.
- **Web UI serving:** Gateway embeds an HTTP server for config UI. Which Rust HTTP framework? Axum (Tokio-native, lightweight), Actix-web (mature), or warp? Must coexist with OPC UA server in same process.
- **Hot-reload mechanics:** How to atomically update config while OPC UA server is running? Options: (a) rebuild address space on change (disruptive), (b) add/remove individual nodes (complex), (c) signal restart of OPC UA task only (middle ground).
- **OPC UA subscription support in async-opcua:** Does async-opcua v0.16.x support server-side subscriptions? If not, this is a blocking dependency for Phase B — may need to contribute upstream or find workaround.
- **Historical data architecture:** Store in SQLite and expose via OPC UA Historical Access? Or use OPC UA's built-in historian capabilities in async-opcua (if supported)?
- **Alarm model:** What OPC UA alarm types are needed? Simple threshold alarms (metric > value)? Or full OPC UA Alarms & Conditions model? The former is much simpler.

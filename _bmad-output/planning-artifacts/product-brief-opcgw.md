---
title: "Product Brief: opcgw"
status: "complete"
created: "2026-04-01"
updated: "2026-04-01"
inputs:
  - docs/index.md
  - docs/architecture.md
  - docs/project-overview.md
  - docs/api-contracts.md
  - doc/planning.md
  - doc/requirements.md
  - doc/architecture.md
  - README.md
  - config/config.toml
  - src/*.rs (exhaustive scan)
---

# Product Brief: opcgw — ChirpStack to OPC UA Gateway

## Executive Summary

opcgw is an open-source Rust gateway that bridges ChirpStack 4 (LoRaWAN Network Server) with OPC UA industrial automation clients. It enables SCADA systems to read LoRaWAN device metrics and send commands to LoRaWAN devices through the OPC UA protocol — a critical integration point for industrial IoT deployments where legacy automation meets modern sensor networks.

The gateway is currently running in production, controlling irrigation valves in fruit orchards via a FUXA SCADA system. However, v1.0.0 has production-grade stability issues (panics on error conditions, no data persistence, insufficient input validation) and significant feature gaps (missing core OPC UA capabilities, file-only configuration, in-memory storage). These issues limit reliability, scalability, and adoption.

This brief proposes a two-phase evolution: **Phase A** stabilizes the current codebase into a hardened v1.x release, while **Phase B** addresses the architectural gaps to deliver a fully-featured v2.0. The timing is ideal — the OPC Foundation announced official LoRaWAN support in OPC UA in January 2026, validating the exact integration opcgw provides, and no open-source alternative exists for this niche.

## The Problem

Industrial operators managing LoRaWAN sensor networks through ChirpStack have no standardized way to expose that data to SCADA systems via OPC UA. Today, they must either:

- **Build custom bridges** from scratch using Node-RED flows or ad-hoc MQTT-to-OPC-UA adapters — fragile, unmaintainable, and requiring deep protocol expertise.
- **Buy proprietary gateways** from vendors like ProSoft, HMS, or Matrikon — expensive, closed-source, and focused on Modbus rather than LoRaWAN.
- **Forego integration entirely** — running ChirpStack and SCADA as disconnected systems, losing the ability to make automated decisions (e.g., moisture sensor triggers irrigation valve).

opcgw solves this, but the current v1.0.0 introduces its own risks: the gateway can crash from unexpected device data, loses all metric state on restart, and lacks OPC UA features that industrial clients expect (subscriptions, alarms, historical data).

## The Solution

### Phase A: Stabilize v1.x (Foundation Hardening)

Fix production issues that undermine reliability and security:

- **Eliminate panics** — Replace all `panic!`/`unwrap()` in production paths with proper error handling and graceful degradation. Currently, a single misbehaving LoRaWAN device can crash the entire gateway, leaving irrigation valves stuck in their last state.
- **Fix command queue semantics** — Switch from LIFO (`Vec::pop`) to FIFO (`VecDeque`) so commands execute in the order they were issued.
- **Add ChirpStack API pagination** — Remove hardcoded `limit=100` to support deployments with 100+ applications or devices.
- **Support all ChirpStack metric types** — Handle Counter, Absolute, and Unknown in addition to Gauge.
- **Define failure-mode behavior** — When ChirpStack is unreachable, expose stale-data indicators to SCADA clients (OPC UA status codes, last-update timestamps) so operators know data is not current.
- **Security hardening** — Move API token to environment variables by default, add input validation on all external data (OPC UA writes, ChirpStack API responses, config file parsing), implement rate limiting on OPC UA connections.
- **Gateway health metrics** — Expose self-monitoring variables in OPC UA (last successful poll timestamp, error counts, ChirpStack connection state) so operators can detect silent failures.
- **Performance validation** — Load test against realistic targets: 5 concurrent OPC UA clients, 100 devices (with headroom to 500 devices), <100ms OPC UA read response latency, <50% CPU.

### Phase B: Evolve to v2.0 (Feature Gaps)

Address the three major architectural gaps:

1. **OPC UA feature completion** — Subscriptions and data change notifications, historical data access, alarms and conditions, method nodes, monitored items tuning. These are expected by any industrial SCADA client beyond basic Browse/Read. Priority: subscriptions first (most requested), then historical data, then alarms.

2. **Web-based configuration interface** — Replace file-only TOML configuration with a lightweight embedded web UI (static HTML pages served by the gateway, with dynamic messaging where needed) for managing applications, devices, metric mappings, and commands. No frontend framework required — keep it simple. Enable hot-reload so configuration changes don't require gateway restarts.

3. **Persistent local storage** — Replace the in-memory HashMap with a local database (e.g., SQLite) for metric persistence across restarts, historical data retention (target: at least 7 days), and configuration storage.

## What Makes This Different

- **Only open-source ChirpStack-to-OPC-UA bridge** — No existing project fills this niche. Node-RED requires manual flow building; commercial gateways (ProSoft, HMS, Matrikon) don't speak ChirpStack and cost significantly more.
- **Purpose-built, not generic** — Designed specifically for the ChirpStack hierarchy (Applications > Devices > Metrics) mapped directly to OPC UA address space. Zero-config device mapping compared to generic protocol adapters.
- **Rust performance and safety** — Memory-safe, async, no GC pauses. Deterministic performance suitable for resource-constrained edge deployments (Raspberry Pi to Docker cluster).
- **Bidirectional control loops** — Not just monitoring; enables closed-loop automation (e.g., soil moisture sensor triggers irrigation valve open/close) via OPC UA Write operations routed to ChirpStack device commands.
- **OPC Foundation alignment** — January 2026 announcement of official LoRaWAN support in OPC UA validates the exact integration model opcgw implements.

## Who This Serves

**Primary user:** The author — managing a LoRaWAN sensor network through ChirpStack for smart agriculture (fruit orchards). Typical deployment: 1-2 SCADA clients, ~100 devices (growing quickly).

**Secondary users:** Open-source community members with similar ChirpStack-to-SCADA integration needs. The project is published as open source for anyone interested, but is not pursuing commercial adoption, certifications, or standards body engagement.

**Current deployment:** Smart agriculture — controlling LoRa irrigation valves and monitoring environmental sensors (temperature, humidity, soil moisture, water levels) in fruit orchards via FUXA SCADA.

**Potential expansion:** Any domain where LoRaWAN sensors feed into industrial control — building management, energy monitoring, water treatment, environmental compliance.

## Dependencies & Constraints

- **ChirpStack API stability** — opcgw depends on ChirpStack 4's gRPC API. Major API changes could require adaptation.
- **async-opcua library maturity** — OPC UA server relies on `async-opcua` (v0.16.x). Library is newer and less battle-tested than alternatives. Monitor upstream maintenance.
- **SCADA client compatibility** — Only tested with FUXA. Phase B OPC UA features must be validated against at least one additional SCADA client.
- **Single-gateway architecture** — No failover or redundancy. If opcgw goes down, SCADA loses visibility and control. Docker restart policy (`always`) provides basic recovery, but not zero-downtime.
- **CI/CD** — GitHub Actions pipeline builds Docker images and pushes to Docker Hub on release tags. Upgrade path for production deployments needs definition.

## Success Criteria

**Phase A (v1.x):**
- Zero panics in 30 days of continuous operation under production load
- All ChirpStack metric types (Gauge, Counter, Absolute) handled correctly
- Load test passes: 5 concurrent OPC UA clients, 100 devices (headroom to 500), <100ms OPC UA read latency, <50% CPU
- Security: no plain-text secrets in default config, input validation on all external data paths
- Gateway health metrics visible in OPC UA address space

**Phase B (v2.0):**
- OPC UA subscriptions, historical data, and alarms functional with at least 2 SCADA clients (FUXA + one other)
- Web UI: configure applications/devices/metrics without editing files or restarting the gateway
- Persistence: gateway restart preserves last-known metric values; historical data queryable for at least 7 days
- Data migration: documented upgrade path from v1.x to v2.0 without data loss

## Scope & Roadmap

**Phase A — In scope:**
- Error handling overhaul (panics to Result types)
- FIFO command queue
- API pagination
- All metric type support
- Failure-mode behavior and stale-data indicators
- Gateway self-monitoring / health metrics
- Security hardening and input validation
- Load testing

**Phase B — In scope:**
- OPC UA subscriptions, historical data, alarms (in that priority order)
- Web configuration UI (lightweight embedded static HTML + dynamic messaging, no frontend framework)
- SQLite or similar local database for persistence
- Configuration hot-reload
- v1.x to v2.0 migration path

**Explicitly out of scope:**
- Cloud connectivity or multi-gateway clustering
- ChirpStack PubSub / real-time event streaming (pull model retained)
- Multi-tenant support
- Mobile app
- Industrial certifications (IEC 61508, etc.)
- OPC Foundation engagement or formal standards participation

## Vision

A reliable, feature-complete open-source bridge between ChirpStack and OPC UA that just works. Stable enough for unattended production use, simple enough to configure via a web browser, and useful enough that other LoRaWAN-to-SCADA users can adopt it as-is. The smart agriculture use case is the proving ground; the open-source model lets others apply it to building management, energy, water treatment, or any domain where LoRaWAN sensors meet industrial control.

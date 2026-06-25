---
layout: default
title: Features & Use Cases
subtitle: What opcgw does and where it fits in your stack
permalink: /features/
---

## Core Features

### 💾 Reliable Data Collection

- **Continuous Polling**: Configurable polling intervals from ChirpStack API
- **Automatic Retries**: Built-in retry logic with exponential backoff
- **Error Resilience**: Graceful handling of network interruptions and API failures
- **Status Tracking**: Monitor ChirpStack server availability and respond to outages

### 🏭 OPC UA Industrial Gateway

- **OPC UA 1.04 Compliant**: Full compliance with OPC Unified Architecture standard
- **Dynamic Address Space**: Automatically build OPC UA variable tree from device configuration
- **Multiple Data Types**: Support for Float, Int, Bool, and String metric types
- **Hierarchical Organization**: Applications → Devices → Metrics structure
- **Real-Time Subscriptions**: Push value changes to clients via OPC UA subscriptions / monitored items
- **Historical Data Access**: Serve time-series history to SCADA clients via OPC UA HistoryRead
- **Stale-Data Detection**: Good / Uncertain / Bad status codes from a configurable staleness threshold
- **Connection Limiting & Auth**: Session caps, security endpoints, and authenticated access

### 🎛️ Device Control & Class-Aware Abstraction

- **OPC UA Write → LoRaWAN Downlink**: A client write to a command node is turned into a downlink to the device via ChirpStack's `DeviceService.Enqueue`
- **Command Lifecycle Tracking**: Each command moves through Pending → Sent → Confirmed / Failed, with delivery confirmation from `ack` / `txack` events on the device stream
- **Class-Aware, Model-Agnostic**: A device-class registry maps per-class command semantics via `command_class` (e.g. `"valve"`) — the Tonhe E20 valve is the first driver, but the model is open to sensors, meters, and actuators
- **Uplink Event-Stream Ingestion**: Devices stream uplinks over gRPC (`StreamDeviceEvents`); each metric is stored as its **raw last-known value** stamped with the device's source timestamp — **no aggregation** (the time-aggregating metrics-poll path is bypassed for streamed devices, so discrete state is never averaged into nonsense)

### 🔐 Web-First Configuration & Auto-Discovery

- **Browser-Based Setup**: First-run web wizard — no hand-editing after the initial bootstrap seed
- **ChirpStack Auto-Discovery**: Pick applications, devices, and metrics from your live ChirpStack inventory by name instead of pasting UUIDs / DevEUIs
- **SQLite-Backed Config**: All configuration stored in SQLite; `config.toml` is a one-time bootstrap seed
- **Staged Apply Model**: Config edits accumulate as pending changes in SQLite and take effect only when you press **Apply changes**, which performs a single graceful in-process soft restart of the data plane — no restart-per-save churn, and the container is never restarted
- **Config Export / Import**: Download your full configuration as portable TOML (`GET /api/config/export`, **secrets excluded**) and restore it elsewhere (`POST /api/config/import`) — the whole import is staged atomically through the Apply flow
- **Drift Detection**: Diff your configured inventory against ChirpStack and reconcile from the UI
- **Duplicate Prevention**: Validation blocks duplicate names / OPC UA node collisions before they persist
- **Environment Overrides**: `OPCGW_*` environment variables override stored config (double-underscore between section and field)
- **No Hardcoded Credentials**: Secrets via environment variables or a `0600` `secrets.toml`

### 📈 Health Dashboard

- **At-a-Glance Verdict**: The landing page leads with a single overall health verdict instead of raw counters
- **Poller-Stall Tile**: Surfaces whether the poller is keeping up with the configured poll interval
- **Per-Device Freshness**: A per-device data-freshness panel classifies each device as fresh / stale / bad / never, all derived client-side from the existing status / device APIs

### 📊 Comprehensive Logging

- **Structured Logging**: Tokio-tracing for rich, queryable log data
- **Per-Module Logs**: Separate log files for ChirpStack, OPC UA, Storage, Config
- **Daily Rotation**: Automatic log file rotation to prevent disk overflow
- **Debug Levels**: Configurable verbosity with per-module control

### 🛑 Graceful Shutdown

- **Signal Handling**: SIGINT (Ctrl+C) and SIGTERM for clean termination
- **Cancellation Tokens**: Propagate shutdown signal to all async tasks
- **Timeout Protection**: Forced exit if cleanup exceeds timeout window
- **State Preservation**: Ensure in-flight operations complete before exit

### 🐳 Container-Native

- **Docker Support**: Official Dockerfile with multi-stage build
- **Docker Compose**: Quick local development with docker-compose.yml
- **Health Checks**: Ready for Kubernetes liveness/readiness probes
- **Lightweight**: ~60MB final image with minimal dependencies

---

## Use Cases

### 🌱 Smart Agriculture

**Scenario**: Monitor soil conditions across multiple fields via LoRaWAN sensors.

- Deploy wireless soil moisture, temperature, pH sensors throughout farm
- Gateway collects data every 5 minutes from ChirpStack
- Connect OPC UA client (e.g., Ignition) to gateway
- Real-time dashboard in farm management system
- Trigger irrigation or fertilization alerts based on soil data

**Benefits**: Reduce water waste, optimize fertilizer use, prevent crop loss from poor conditions.

---

### 🏭 Industrial Asset Tracking

**Scenario**: Track equipment and material movement within a factory.

- LoRaWAN tags on critical machines, raw materials, work-in-progress
- ChirpStack provides real-time position and condition data
- Gateway exposes via OPC UA to MES (Manufacturing Execution System)
- MES integrates data into production planning and traceability
- Real-time inventory visibility

**Benefits**: Reduce lost materials, improve production scheduling, enable compliance reporting.

---

### 🌍 Environmental Monitoring

**Scenario**: Distributed air quality, noise, weather monitoring in urban areas.

- Deploy LoRaWAN environmental sensors across city neighborhoods
- ChirpStack aggregates sensor data
- Gateway streams to analytics platform via OPC UA
- Real-time public dashboard and alerts
- Historical data for trend analysis

**Benefits**: Public health monitoring, regulatory compliance, urban planning insights.

---

### 🏢 Building Automation

**Scenario**: Integrate wireless HVAC, occupancy, and energy sensors.

- Wireless temperature sensors in each zone
- Occupancy sensors for demand-controlled ventilation
- Energy meters via LoRaWAN
- Gateway connects to Building Management System (BMS)
- Automatic HVAC adjustments based on occupancy and temperature

**Benefits**: 20-30% energy savings, improved comfort, easier expansion (no wiring needed).

---

### ⚡ Energy Management

**Scenario**: Monitor distributed renewable energy and battery systems.

- Solar inverters, battery packs with LoRaWAN modems
- Gateway provides unified view to energy management platform
- Real-time generation/consumption balancing
- Detect faults or degradation early
- Optimize energy storage charging/discharging

**Benefits**: Maximize self-consumption, reduce grid dependency, extend equipment life.

---

## Technical Highlights

### Performance
- **Low Latency**: Async/await I/O with Tokio runtime
- **Memory Efficient**: Zero-copy where possible, bounded in-memory buffers
- **Scalable**: Support for hundreds of devices with configurable polling intervals

### Reliability
- **Crash Prevention**: No unsafe code (unless justified), comprehensive error handling
- **Graceful Degradation**: Continues operation despite partial failures
- **Observability**: Deep logging for post-mortem analysis

### Security
- **No Hardcoded Secrets**: Environment variables and secure config handling
- **Input Validation**: Configuration and API input validation
- **Safe Async**: Tokio-based safe async without data races

### Maintainability
- **Well-Documented**: Doc comments on public APIs
- **Modular Design**: Clear separation of ChirpStack, OPC UA, Storage concerns
- **Comprehensive Tests**: Unit tests for critical paths
- **CI/CD**: Automated testing, linting, security checks on every PR

---

## Roadmap

Most of the original roadmap has already shipped. Highlights now in the released gateway:

- ✅ **SQLite persistence** for metric values, history, and the command queue (Epic 2)
- ✅ **End-to-end downlink command path** — an OPC UA write becomes a LoRaWAN downlink via ChirpStack `Enqueue`, with a Pending → Sent → Confirmed / Failed command lifecycle (Epic E, v2.2.0)
- ✅ **Real-time OPC UA subscriptions** and **historical data access** (Epic 8)
- ✅ **Web UI** for configuration and monitoring (Epic 9)
- ✅ **Auto-discovery and web-first configuration**, SQLite-backed config (Epics C + D)
- ✅ **Class-aware device abstraction** — model-agnostic `command_class` registry (Tonhe valve first driver) and raw, no-aggregation uplink event-stream ingestion (Epic E, v2.2.0)
- ✅ **Onboarding & web UX for public release** — zero-touch first-run wizard, staged "Apply changes" soft restart, config export/import, and a redesigned health dashboard (Epic F, v2.3.0)

See the [Development Roadmap](roadmap.html) for the current plan and what comes next.

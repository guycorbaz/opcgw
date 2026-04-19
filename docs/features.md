---
layout: default
layout: page
title: Features & Use Cases
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

### 🔐 Enterprise-Grade Configuration

- **TOML Configuration**: Human-readable configuration format with schema validation
- **Environment Overrides**: OPCGW_ prefixed environment variables override config file settings
- **Validation on Startup**: Clear, actionable error messages for configuration issues
- **No Hardcoded Credentials**: All secrets via config or environment variables

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

## Roadmap (Planned)

- **v2.1**: SQLite persistence for historical data
- **v2.2**: Command queue for write-back to ChirpStack devices
- **v2.3**: Real-time OPC UA subscriptions
- **v3.0**: Web UI for configuration and monitoring
- **v3.1**: Hot-reload configuration without restart

See the [GitHub Projects](https://github.com/guycorbaz/opcgw/projects) for detailed tracking.

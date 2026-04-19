---
stepsCompleted:
  - step-01-init
  - step-02-discovery
  - step-02b-vision
  - step-02c-executive-summary
  - step-03-success
  - step-04-journeys
  - step-05-domain
  - step-06-innovation-skipped
  - step-07-project-type
  - step-08-scoping
  - step-09-functional
  - step-10-nonfunctional
  - step-11-polish
  - step-12-complete
inputDocuments:
  - _bmad-output/planning-artifacts/product-brief-opcgw.md
  - _bmad-output/planning-artifacts/product-brief-opcgw-distillate.md
  - docs/index.md
  - docs/project-overview.md
  - docs/architecture.md
  - docs/source-tree-analysis.md
  - docs/api-contracts.md
  - docs/development-guide.md
  - docs/deployment-guide.md
documentCounts:
  briefs: 2
  research: 0
  brainstorming: 0
  projectDocs: 7
classification:
  projectType: iot_embedded
  projectTypeFraming: "IoT gateway middleware (protocol bridge), not embedded firmware"
  domain: process_control
  complexity: high
  projectContext: brownfield
  prdApproach: "Delta from current state — requirements as 'Currently X, must become Y, because Z'"
workflowType: 'prd'
---

# Product Requirements Document — opcgw

**Author:** Guy Corbaz
**Date:** 2026-04-01
**Version:** 1.0
**Status:** Complete

## Executive Summary

opcgw is an open-source Rust gateway that bridges ChirpStack 4 (LoRaWAN Network Server) with OPC UA industrial automation clients. It enables SCADA systems to read LoRaWAN device metrics and send commands to LoRaWAN devices through the OPC UA protocol — eliminating the need for custom bridges, expensive proprietary hardware, or deep protocol expertise.

The gateway is running in production today, controlling irrigation valves and monitoring environmental sensors across fruit orchards via FUXA SCADA. The current v1.0.0 works but has production stability risks (crash-on-error behavior, in-memory-only storage, insufficient input validation) and significant feature gaps (missing OPC UA subscriptions/alarms/historical data, file-only configuration, no data persistence).

This PRD defines a two-phase evolution:

- **Phase A (Stabilize v1.x):** Harden the existing codebase — eliminate panics, fix command ordering, add pagination, support all metric types, implement stale-data detection, expose gateway health metrics, harden security, add SQLite persistence, and establish a test suite. Includes a critical technical spike to validate async-opcua subscription capabilities.
- **Phase B (Evolve to v2.0):** Complete OPC UA feature set (subscriptions, historical data, threshold alarms), add a lightweight embedded web configuration UI with hot-reload, and implement the v1→v2 migration path.

The core value proposition is simplicity: there is no easy way to connect ChirpStack to a SCADA system today. opcgw makes it as simple as configuring your devices and starting the gateway — your SCADA just sees your LoRaWAN devices as OPC UA variables.

### What Makes This Special

- **Only open-source ChirpStack-to-OPC-UA bridge** — No competing project fills this niche. Alternatives require manual flow-building (Node-RED), expensive proprietary hardware (ProSoft, HMS), or building a custom adapter stack from scratch.
- **Purpose-built simplicity** — Maps the ChirpStack hierarchy (Applications > Devices > Metrics) directly to OPC UA address space. Configure devices, start the gateway, forget about it.
- **Bidirectional control loops** — Not just monitoring; enables closed-loop automation (soil moisture → valve command) via OPC UA Write operations routed through ChirpStack.
- **Rust performance and safety** — Async, memory-safe, no GC pauses. Suitable for deployment from Raspberry Pi to Docker clusters.
- **OPC Foundation alignment** — January 2026 OPC Foundation announcement of official LoRaWAN support validates this exact integration model. Specs will be monitored and followed where practical.

## Project Classification

- **Project Type:** IoT gateway middleware (protocol bridge between ChirpStack gRPC and OPC UA)
- **Domain:** Process control / industrial automation (OPC UA, SCADA, physical actuator control)
- **Complexity:** High — physical consequences of failure (irrigation), OT security concerns, industrial protocol compliance, real-time requirements
- **Project Context:** Brownfield — v1.0.0 running in production. Requirements defined as delta from current documented state.
- **Primary User:** Author (solo developer, personal smart agriculture deployment, ~100 devices, 1-2 SCADA clients)
- **Secondary Users:** Open-source community with similar ChirpStack-to-SCADA needs

## Success Criteria

### User Success

- **Invisible operation:** Gateway runs unattended for 30+ days. Auto-recovers from ChirpStack outages without manual intervention.
- **Stale-data awareness:** When ChirpStack is unreachable, SCADA shows clear stale-data indicators — not silently stale values.
- **Instant device onboarding (Phase B):** Add a new LoRaWAN device via web browser, see it in FUXA within one poll cycle. No file editing, no restart.
- **Debug visibility (Phase B):** Open the web UI, see live metric values and gateway status at a glance — useful when installing new sensors in the field.

### Business Success

- **Personal productivity:** Time from "new sensor installed" to "visible in SCADA" drops from minutes (edit TOML, restart, verify) to seconds (web UI, done).
- **Open-source credibility:** Stable, well-documented gateway that others deploy without asking for support. Zero open "crash" issues, clean README-to-running experience.
- **Production confidence:** Gateway controls irrigation infrastructure reliably through a full growing season without backup monitoring.

### Technical Success

**Phase A:**
- Zero panics in 30 days of continuous production operation
- Automatic recovery from ChirpStack outages (reconnect + resume polling without restart)
- All ChirpStack metric types handled (Gauge, Counter, Absolute)
- FIFO command execution guaranteed under concurrent writes
- Last-known metric values survive gateway restart (SQLite persistence)
- Gateway health metrics visible in OPC UA address space (last poll time, error count, connection state)
- Load test passes: 5 concurrent OPC UA clients, 100 devices (headroom to 500), <100ms OPC UA read latency
- No plain-text secrets in default config; input validation on all external data paths
- async-opcua subscription spike completed with documented findings and Plan B if needed
- Test suite covering error paths, command ordering, persistence, and failure injection

**Phase B:**
- OPC UA subscriptions functional with FUXA + one additional client (e.g., UaExpert)
- OPC UA historical data access serving at least 7 days of stored metrics
- Threshold-based alarms (metric crosses configured value → OPC UA Bad/Warning status)
- Web UI: CRUD for applications/devices/metrics/commands + live metric values + gateway status
- Hot-reload: config changes via web UI apply without dropping OPC UA client connections
- Documented v1.x → v2.0 upgrade path

### Measurable Outcomes

| Metric | Phase A Target | Phase B Target |
|--------|---------------|----------------|
| Uptime (unattended) | 30 days, zero crashes | Full growing season |
| Recovery from ChirpStack outage | Automatic, <30s after server returns | Same |
| Device onboarding time | Edit TOML + restart (~5 min) | Web UI click (~30s) |
| Data loss on restart | Zero (last-known values persisted) | Zero + 7-day history |
| OPC UA client compatibility | FUXA (Browse/Read/Write) | FUXA + UaExpert (subscriptions, history) |

## Product Scope & Phased Development

### MVP Strategy

**Approach:** Problem-solving MVP — make the existing running system reliable, persistent, and safe before adding new capabilities. The gateway already works; the MVP is about making it *trustworthy*.

**Resource:** Solo developer. Estimated ~6 weeks for Phase A, ~6-8 weeks for Phase B.

### Phase A — Stabilize v1.x

**Must-Have (v1.1 minimum viable release):**

| Priority | Item | Rationale |
|----------|------|-----------|
| 1 | Error handling overhaul | Gateway cannot crash in production. Foundation for everything else. |
| 2 | FIFO command queue + RwLock evaluation | Command integrity for physical actuators. |
| 3 | Storage abstraction trait | Architecture for SQLite. Enables testability. |
| 4 | SQLite persistence | No more data loss on restart. Persistent command queue. |
| 5 | async-opcua quick spike | De-risk Phase B before investing further. |

**Should-Have (v1.2 follow-up):**

| Priority | Item | Rationale |
|----------|------|-----------|
| 6 | Pagination + all metric types | Supports growth beyond 100 devices. |
| 7 | Stale-data indicators + health metrics | Operational visibility for SCADA operators. |
| 8 | Security hardening | Input validation, token handling, rate limiting. |
| 9 | Config hot-reload preparation | Architectural groundwork for Phase B web UI. |
| 10 | Test coverage review + gap filling | Quality gate before Phase B. |
| 11 | Load testing baseline | Performance regression target. |
| 12 | Full async-opcua spike | Detailed subscription architecture + Plan B. |

**Release strategy:** v1.1 (items 1-5) deployed to production first. v1.2 (items 6-12) follows. Early production value without waiting for full Phase A completion.

**Journeys supported:** Journey 1 (Daily Operations), Journey 2 (Something Goes Wrong).

### Phase B — Evolve to v2.0

1. OPC UA subscriptions and data change notifications
2. OPC UA historical data access (7-day retention, SQLite-backed)
3. Threshold-based alarms (status codes, not full Alarms & Conditions)
4. Embedded web configuration UI (static HTML + dynamic messaging, Axum)
5. Live metric values and gateway status in web UI
6. Configuration hot-reload with validation and rollback
7. OPC UA address space dynamic mutation (add/remove nodes at runtime)
8. Multi-client OPC UA compliance testing (FUXA + UaExpert)
9. v1.x → v2.0 migration path documentation

**Journeys unlocked:** Journey 3 (Adding a Sensor via Web UI), Journey 4 (Open-Source Adopter).

### Vision (Future — Out of Scope)

- Cloud connectivity or multi-gateway clustering
- ChirpStack PubSub / real-time event streaming
- Multi-tenant support
- Pre-built device profile library for common LoRaWAN sensors
- Mobile app
- Industrial certifications
- OPC Foundation formal engagement

### Risk Mitigation

| Risk | Impact | Mitigation |
|------|--------|------------|
| Gateway crash during irrigation | Valves stuck in last state | Eliminate all panics; Docker restart policy; persistent command queue |
| Stale data driving wrong decisions | Over/under watering crops | Staleness threshold + OPC UA status codes; health metrics in address space |
| ChirpStack API breaking change | Gateway stops polling | Pin API version; test against upgrades before releasing |
| async-opcua can't do subscriptions | Phase B blocked | Early spike (Phase A item 5); documented Plan B (locka99/opcua or upstream contribution) |
| SQLite corruption on crash | Data loss | WAL journal mode; periodic integrity checks |
| Unauthorized valve commands | Physical damage | Input validation; OPC UA authentication; rate limiting |

**Resource risk:** Solo developer. v1.1 (items 1-5) is a meaningful standalone release if time is constrained. Phase B can be tackled incrementally.

**Operational risk:** Production system must keep running during development. All Phase A changes backward-compatible with current `config.toml` format. No breaking changes until v2.0.

## User Journeys

### Journey 1: Guy — Daily Operations (Primary, Happy Path)

**Opening Scene:** It's 6 AM in the orchard. Guy checks his phone — FUXA dashboard shows all sensor readings from last night. Soil moisture in the cherry plot is dropping. Temperature in the tunnel greenhouse is fine. Battery levels on all devices look healthy.

**Rising Action:** He notices soil moisture in Verger2 has dropped below threshold. He opens FUXA on his laptop, navigates to the Arrosage application, finds Vanne01, and writes a command to open the valve. The command appears as "sent" almost immediately.

**Climax:** 30 minutes later, the soil moisture sensor shows the value climbing. He sends the close command. The entire interaction happened through FUXA — he never opened a terminal, never SSH'd into the gateway, never thought about opcgw at all.

**Resolution:** By the end of the day, all orchards are watered, all sensors are reporting, all valves responded. The gateway was invisible infrastructure.

**Requirements revealed:** FR1-3, FR9-10, FR14-16, FR46; NFR1, NFR16-17.

### Journey 2: Guy — Something Goes Wrong (Primary, Edge Case)

**Opening Scene:** Guy opens FUXA and sees that soil moisture readings for all devices haven't updated since 3 AM. The values look normal — but the timestamps are 4 hours old.

**Rising Action:** He checks the gateway health metrics in FUXA — "ChirpStack connection: unavailable" and "last successful poll: 03:12." OPC UA variables show `UncertainLastUsableValue` status codes. Valve commands are queued but not delivered.

**Climax:** He restarts the ChirpStack server. Within 30 seconds, the gateway auto-reconnects, polls fresh data, and all metrics update. Queued commands execute in order. The valves respond correctly.

**Resolution:** No data was lost — last-known values were persisted in SQLite. The command queue survived. Guy didn't need to restart the gateway.

**Requirements revealed:** FR6-8, FR11, FR17-18, FR25-26; NFR16-20.

### Journey 3: Guy — Adding a New Sensor (Phase A → Phase B)

**Phase A:** Guy registers a new sensor in ChirpStack, edits `config/config.toml`, restarts Docker container. FUXA shows the new device. ~5 minutes.

**Phase B:** Guy opens the web UI on his phone in the field. Adds the device, maps metrics, clicks save. FUXA shows the new device within one poll cycle. Checks live metrics in web UI to verify. No restart. ~30 seconds.

**Requirements revealed:** FR31-33 (Phase A); FR34-41, FR50 (Phase B).

### Journey 4: Open-Source Adopter — First Deployment

**Opening Scene:** Alex, a building automation engineer, finds opcgw on GitHub. README is clear — understands what it does in 30 seconds.

**Rising Action:** Clones repo, edits config, runs `docker compose up`. Logs show successful connection.

**Climax:** Opens their SCADA client (Ignition), browses OPC UA address space, sees devices organized by application. Data flows. Commands work.

**Resolution:** Up and running in under 15 minutes without filing an issue.

**Requirements revealed:** FR33, FR46-49; NFR21-24.

### Journey 5: Guy — Phase B Daily Operations (Subscriptions, History, Alarms)

**Opening Scene:** It's early morning. Guy's phone buzzes — FUXA has pushed a notification that soil moisture in Verger1 crossed below the configured threshold overnight. He didn't have to check; the gateway's subscription-based data change notifications triggered the alarm automatically.

**Rising Action:** He opens FUXA on his laptop. The dashboard updates in real time — no manual refresh, no polling delay. Soil moisture values tick down as he watches. He pulls up the historical trend for Verger1 over the past week: a clear downward slope since the last rainfall five days ago. The 7-day chart confirms this isn't a sensor glitch — the soil is genuinely drying out.

**Climax:** He sends the valve-open command to Vanne01 via FUXA. The command status shows "sent" immediately. Within minutes, the real-time subscription feed shows soil moisture beginning to climb. The alarm clears automatically when the value crosses back above the threshold. He checks Verger2's historical data — moisture is holding steady, no action needed.

**Resolution:** The entire workflow — alarm notification, historical analysis, command execution, real-time confirmation — happened through FUXA without Guy ever thinking about opcgw. Subscriptions eliminated polling lag. Historical data replaced guesswork with trend analysis. Threshold alarms caught the problem before Guy even opened the dashboard.

**Requirements revealed:** FR21-24, FR27-28; NFR15, NFR22.

### Journey Requirements Traceability

| Journey | Key FRs | Key NFRs |
|---------|---------|----------|
| Daily Operations | FR1-3, FR9-10, FR14-16, FR46 | NFR1, NFR16-17 |
| Something Goes Wrong | FR6-8, FR11, FR17-18, FR25-26 | NFR16-20 |
| Adding a Sensor | FR31-41, FR50 | — |
| First Deployment | FR33, FR46-49 | NFR21-24 |
| Phase B Daily Operations | FR21-24, FR27-28 | NFR15, NFR22 |

## Domain-Specific Requirements

### OT Security

- **Credential management:** API tokens must not appear in default config files. Environment variable override must become the documented default. Config template contains placeholder values only.
- **OPC UA certificate security:** Self-signed certificates acceptable for development. Production deployment guide must document CA-signed certificate setup. `create_sample_keypair` defaults to `false` in release builds.
- **Input validation on actuator commands:** All OPC UA Write values destined for physical devices must be validated — type checking, range checking, f_port validation. Malformed commands rejected with clear OPC UA status codes, never forwarded to ChirpStack.
- **Connection rate limiting:** OPC UA server limits concurrent client connections to a configurable maximum.

### Real-Time & Reliability

- **Physical consequence awareness:** The gateway controls irrigation valves. A crash means valves stay in their last state indefinitely. All error paths degrade gracefully — never panic, never leave the system in an unknown state.
- **Stale-data detection:** Every metric carries a last-updated timestamp. Configurable staleness threshold (default: 2x polling frequency) triggers `UncertainLastUsableValue` OPC UA status code. SCADA operators never see silently stale data.
- **Auto-recovery:** Gateway automatically reconnects to ChirpStack after outages. Recovery target: <30 seconds.
- **Command integrity:** Commands to physical actuators execute in FIFO order. Persistent command queue survives restarts.
- **Single point of failure acknowledged:** No redundancy in scope. Docker `restart: always` provides basic crash recovery. Gateway starts cleanly from SQLite state after unexpected termination.

### Industrial Protocol Compliance

- **OPC UA feature gaps (Phase B):** Subscriptions, historical data access, and threshold-based alarms expected by industrial SCADA clients. Priority: subscriptions first.
- **Multi-client compatibility:** Currently tested with FUXA only. Phase B validates with at least one additional client (UaExpert recommended).
- **OPC Foundation alignment:** Monitor January 2026 OPC UA LoRaWAN specification. Align where practical without pursuing formal certification.

### Integration Constraints

- **ChirpStack API dependency:** Depends on ChirpStack 4 gRPC API (chirpstack_api v4.13.0). Pin API version, document supported versions, test against upgrades.
- **async-opcua library risk:** Depends on async-opcua v0.16.x. Subscription support unverified. Phase A spike to validate; Plan B documented.
- **Polling model:** ChirpStack metrics polled at configurable intervals (default 10s). No real-time event streaming. Acceptable for agriculture sensors.

## IoT Gateway Specific Requirements

### Deployment Architecture

- **Runtime:** Docker container on Synology NAS (x86_64)
- **Network:** Local LAN, no firewalls between components
- **Topology:** opcgw on NAS-1, FUXA (SCADA) on NAS-2, ChirpStack on separate host — all same LAN
- **Always-on:** NAS provides 24/7 operation, no power constraints
- **Resource constraints:** NAS is shared infrastructure — opcgw must be a good neighbor (<50% CPU, bounded memory)

### Connectivity Protocols

| Protocol | Direction | Endpoint | Port |
|----------|-----------|----------|------|
| ChirpStack gRPC | Outbound | ChirpStack server | 8080 |
| OPC UA TCP | Inbound | FUXA on separate NAS | 4855 (Docker) / 4840 (native) |
| HTTP (Phase B) | Inbound | Web UI for configuration | TBD (configurable) |

- OPC UA server reachable across the LAN (not just localhost)
- Phase B web UI also LAN-accessible for configuration from any device
- All three protocols coexist in the same Docker container and Tokio runtime

### Security Model

- **Network:** Open LAN. OPC UA security endpoints (Basic256 Sign/SignAndEncrypt) provide transport security.
- **Authentication:** OPC UA username/password + optional certificate. ChirpStack Bearer token via environment variable.
- **Docker isolation:** Mapped volumes (config/, pki/, log/, data/). No host network mode — ports explicitly mapped.
- **Credentials:** API tokens and passwords via environment variables. Docker Compose `environment` or `.env` file for secrets.

### Update Mechanism

- **Process:** Manual — pin image version in `docker-compose.yml`, pull new image, `docker compose up -d`
- **No auto-update:** Intentional — avoids unexpected upgrades on infrastructure controlling physical devices
- **Rollback:** Change version tag back and restart
- **Compatibility:** No migration scripts between patch versions (v1.1 → v1.2). Major versions (v1.x → v2.0) require documented migration path.
- **CI/CD:** GitHub Actions builds and pushes Docker images to Docker Hub on release tags.

### Implementation Considerations

- **Docker-first development:** All testing and deployment assumes Docker. Native binary is secondary.
- **Volume persistence:** SQLite database on mapped volume (`./data:/usr/local/bin/data`) to survive container replacement.
- **Port configuration:** OPC UA port and web UI port both configurable and mappable in `docker-compose.yml`.
- **Logging:** File-based logging (log4rs → `./log/`) with Docker volume mapping. No changes needed.
- **Graceful shutdown:** SIGTERM triggers clean shutdown — flush SQLite writes, complete in-progress poll, close OPC UA connections.

## Functional Requirements

**This FR list is the capability contract for all downstream work. Any feature not listed here will not exist in the final product unless explicitly added.**

### ChirpStack Data Collection

- **FR1:** System can poll device metrics from ChirpStack gRPC API at configurable intervals
- **FR2:** System can authenticate with ChirpStack using a Bearer API token
- **FR3:** System can retrieve metrics for all configured devices across multiple applications
- **FR4:** System can handle all ChirpStack metric types (Gauge, Counter, Absolute, Unknown)
- **FR5:** System can paginate through ChirpStack API responses when applications or devices exceed 100
- **FR6:** System can detect ChirpStack server unavailability via TCP connectivity check
- **FR7:** System can automatically reconnect to ChirpStack after an outage without manual intervention (recovery target: <30 seconds)
- **FR8:** System can retry ChirpStack connections with configurable retry count and delay

### Device Command Execution

- **FR9:** SCADA operator can send commands to LoRaWAN devices via OPC UA Write operations
- **FR10:** System can queue commands in FIFO order and deliver them to ChirpStack for transmission
- **FR11:** System can persist the command queue across gateway restarts
- **FR12:** System can validate command parameters (type, range, f_port) before forwarding to ChirpStack
- **FR13:** System can report command delivery status (pending, sent, failed)

### OPC UA Server — Current (Phase A)

- **FR14:** System can expose device metrics as OPC UA variables organized by Application > Device > Metric hierarchy
- **FR15:** SCADA client can browse the OPC UA address space and discover all configured devices and metrics
- **FR16:** SCADA client can read current metric values with appropriate OPC UA data types (Boolean, Int32, Float, String)
- **FR17:** System can indicate stale data via OPC UA status codes (UncertainLastUsableValue) when metrics exceed a configurable staleness threshold
- **FR18:** System can expose gateway health metrics in the OPC UA address space (last poll timestamp, error count, ChirpStack connection state)
- **FR19:** System can serve OPC UA connections over multiple security endpoints (None, Basic256 Sign, Basic256 SignAndEncrypt)
- **FR20:** System can authenticate OPC UA clients via username/password

### OPC UA Server — Extended (Phase B)

- **FR21:** SCADA client can subscribe to metric value changes and receive data change notifications
- **FR22:** SCADA client can query historical metric data for a configurable retention period (minimum 7 days)
- **FR23:** System can signal threshold-based alarm conditions via OPC UA status codes when metrics cross configured values
- **FR24:** System can add and remove OPC UA nodes at runtime when configuration changes (dynamic address space mutation)

### Data Persistence

- **FR25:** System can persist last-known metric values in a local embedded database
- **FR26:** System can restore last-known metric values from persistent storage on gateway startup
- **FR27:** System can store historical metric data with timestamps in an append-only fashion
- **FR28:** System can prune historical data older than the configured retention period
- **FR29:** System can support concurrent read/write access to the persistence layer without blocking
- **FR30:** System can batch metric writes per poll cycle for write efficiency

### Configuration Management — Current (Phase A)

- **FR31:** Operator can configure applications, devices, metrics, and commands via TOML file
- **FR32:** Operator can override configuration values via environment variables (OPCGW_ prefix)
- **FR33:** System can validate configuration on startup and report clear error messages for invalid config

### Configuration Management — Web UI (Phase B)

- **FR34:** Operator can view, create, edit, and delete applications via web interface
- **FR35:** Operator can view, create, edit, and delete devices and their metric mappings via web interface
- **FR36:** Operator can view, create, edit, and delete device commands via web interface
- **FR37:** Operator can view live metric values for all devices via web interface (debugging)
- **FR38:** Operator can view gateway status (ChirpStack connection, last poll, error counts) via web interface
- **FR39:** System can apply configuration changes without requiring a gateway restart (hot-reload)
- **FR40:** System can validate configuration changes before applying and rollback on failure
- **FR41:** Web interface can be accessed from any device on the LAN (mobile-responsive)

### Security

- **FR42:** System can load API tokens and passwords from environment variables (not plain-text config by default)
- **FR43:** System can validate all input from OPC UA Write operations before forwarding to ChirpStack
- **FR44:** System can limit concurrent OPC UA client connections to a configurable maximum
- **FR45:** System can manage OPC UA certificates (own, private, trusted, rejected) via PKI directory

### Operational Reliability

- **FR46:** System can handle all error conditions without crashing (no panics in production paths)
- **FR47:** System can shut down gracefully on SIGTERM (flush persistence writes, complete in-progress poll, close connections)
- **FR48:** System can start cleanly from persisted state after container replacement or unexpected termination
- **FR49:** System can log operations per module to separate files (chirpstack, opc_ua, storage, config)
- **FR50:** Web interface can require basic authentication (username/password) to access configuration and status pages

## Non-Functional Requirements

### Performance

- **NFR1:** OPC UA Read operations complete in <100ms for any single metric value, as measured by OPC UA client timing or load testing harness
- **NFR2:** Full poll cycle (100 devices × average 4 metrics) completes within the configured polling interval (default 10s), as measured by gateway internal timing logs
- **NFR3:** Persistence write batch (400 metrics per poll cycle) completes in <500ms, as measured by gateway internal timing logs
- **NFR4:** Gateway startup from persisted state completes in <10 seconds (ready to serve OPC UA clients), as measured by time from process start to first successful OPC UA connection
- **NFR5:** Memory usage remains bounded — no unbounded growth over weeks of operation. Target: <256MB RSS for 100 devices, as measured by container resource monitoring (e.g., `docker stats`)
- **NFR6:** CPU usage below 50% on NAS-class x86_64 during normal operation (100 devices, 5 clients, 10s polling), as measured by container resource monitoring

### Security

- **NFR7:** API tokens and passwords never appear in log output at any log level
- **NFR8:** Default configuration template contains no real credentials — placeholders only
- **NFR9:** OPC UA certificate private keys stored with restricted file permissions (600)
- **NFR10:** All OPC UA Write values destined for physical actuators validated before transmission — no raw passthrough
- **NFR11:** Web UI requires authentication before any configuration change (basic auth minimum)
- **NFR12:** Failed authentication attempts (OPC UA and web UI) logged with source IP

### Scalability

- **NFR13:** System handles 100 devices with 5 concurrent OPC UA clients at performance targets
- **NFR14:** System degrades gracefully (increased latency, not crash) at 500 devices
- **NFR15:** Historical data storage handles 7 days retention (~24 million rows at 10s polling) — historical queries return in <2 seconds

### Reliability

- **NFR16:** 30 days continuous operation without crash or manual intervention under production load
- **NFR17:** Auto-recover from ChirpStack outages within 30 seconds of server availability returning
- **NFR18:** No single malformed metric or device response crashes the gateway — errors logged and skipped
- **NFR19:** Persistent database survives unclean shutdown (power loss, OOM kill) without data corruption
- **NFR20:** Command queue guarantees FIFO ordering under all conditions including concurrent OPC UA writes

### Integration

- **NFR21:** Compatible with ChirpStack 4.x gRPC API
- **NFR22:** OPC UA server compatible with FUXA SCADA and at least one additional OPC UA client (Phase B)
- **NFR23:** Docker container supports standard lifecycle (start, stop, restart, logs) with mapped volumes for persistence
- **NFR24:** Configuration supports environment variable overrides for all secrets (Docker Compose, Kubernetes, CI/CD compatible)

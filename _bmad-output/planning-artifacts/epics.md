---
stepsCompleted:
  - step-01-validate-prerequisites
  - step-02-design-epics
  - step-03-create-stories
  - step-04-final-validation
status: complete
completedAt: '2026-04-02'
inputDocuments:
  - _bmad-output/planning-artifacts/prd.md
  - _bmad-output/planning-artifacts/architecture.md
---

# opcgw - Epic Breakdown

## Overview

This document provides the complete epic and story breakdown for opcgw, decomposing the requirements from the PRD and Architecture into implementable stories.

### Key Decision: No Migration Path — Parallel Installation + Cutover

- v1→v2 migration path is **dropped** from scope
- Phase A is a development-only milestone — **not deployed to production**
- Current v1.0.0 continues running in production throughout Phase A and B development
- Phase B is the first candidate for production deployment, run in **parallel** alongside v1.0 (separate Docker container, different ports)
- Operator configures Phase B fresh via web UI, validates, then cuts over
- No backward-compatibility constraints on config.toml format in any phase
- PRD deliverable "documented v1.x → v2.0 upgrade path" is **removed**
- Architecture constraint "no breaking config changes until v2.0" is **removed**

## Requirements Inventory

### Functional Requirements

**ChirpStack Data Collection**
FR1: System can poll device metrics from ChirpStack gRPC API at configurable intervals
FR2: System can authenticate with ChirpStack using a Bearer API token
FR3: System can retrieve metrics for all configured devices across multiple applications
FR4: System can handle all ChirpStack metric types (Gauge, Counter, Absolute, Unknown)
FR5: System can paginate through ChirpStack API responses when applications or devices exceed 100
FR6: System can detect ChirpStack server unavailability via TCP connectivity check
FR7: System can automatically reconnect to ChirpStack after an outage without manual intervention (recovery target: <30 seconds)
FR8: System can retry ChirpStack connections with configurable retry count and delay

**Device Command Execution**
FR9: SCADA operator can send commands to LoRaWAN devices via OPC UA Write operations
FR10: System can queue commands in FIFO order and deliver them to ChirpStack for transmission
FR11: System can persist the command queue across gateway restarts
FR12: System can validate command parameters (type, range, f_port) before forwarding to ChirpStack
FR13: System can report command delivery status (pending, sent, failed)

**OPC UA Server - Current (Phase A)**
FR14: System can expose device metrics as OPC UA variables organized by Application > Device > Metric hierarchy
FR15: SCADA client can browse the OPC UA address space and discover all configured devices and metrics
FR16: SCADA client can read current metric values with appropriate OPC UA data types (Boolean, Int32, Float, String)
FR17: System can indicate stale data via OPC UA status codes (UncertainLastUsableValue) when metrics exceed a configurable staleness threshold
FR18: System can expose gateway health metrics in the OPC UA address space (last poll timestamp, error count, ChirpStack connection state)
FR19: System can serve OPC UA connections over multiple security endpoints (None, Basic256 Sign, Basic256 SignAndEncrypt)
FR20: System can authenticate OPC UA clients via username/password

**OPC UA Server - Extended (Phase B)**
FR21: SCADA client can subscribe to metric value changes and receive data change notifications
FR22: SCADA client can query historical metric data for a configurable retention period (minimum 7 days)
FR23: System can signal threshold-based alarm conditions via OPC UA status codes when metrics cross configured values
FR24: System can add and remove OPC UA nodes at runtime when configuration changes (dynamic address space mutation)

**Data Persistence**
FR25: System can persist last-known metric values in a local embedded database
FR26: System can restore last-known metric values from persistent storage on gateway startup
FR27: System can store historical metric data with timestamps in an append-only fashion
FR28: System can prune historical data older than the configured retention period
FR29: System can support concurrent read/write access to the persistence layer without blocking
FR30: System can batch metric writes per poll cycle for write efficiency

**Configuration Management - Current (Phase A)**
FR31: Operator can configure applications, devices, metrics, and commands via TOML file
FR32: Operator can override configuration values via environment variables (OPCGW_ prefix)
FR33: System can validate configuration on startup and report clear error messages for invalid config

**Configuration Management - Web UI (Phase B)**
FR34: Operator can view, create, edit, and delete applications via web interface
FR35: Operator can view, create, edit, and delete devices and their metric mappings via web interface
FR36: Operator can view, create, edit, and delete device commands via web interface
FR37: Operator can view live metric values for all devices via web interface (debugging)
FR38: Operator can view gateway status (ChirpStack connection, last poll, error counts) via web interface
FR39: System can apply configuration changes without requiring a gateway restart (hot-reload)
FR40: System can validate configuration changes before applying and rollback on failure
FR41: Web interface can be accessed from any device on the LAN (mobile-responsive)

**Security**
FR42: System can load API tokens and passwords from environment variables (not plain-text config by default)
FR43: System can validate all input from OPC UA Write operations before forwarding to ChirpStack
FR44: System can limit concurrent OPC UA client connections to a configurable maximum
FR45: System can manage OPC UA certificates (own, private, trusted, rejected) via PKI directory

**Operational Reliability**
FR46: System can handle all error conditions without crashing (no panics in production paths)
FR47: System can shut down gracefully on SIGTERM (flush persistence writes, complete in-progress poll, close connections)
FR48: System can start cleanly from persisted state after container replacement or unexpected termination
FR49: System can log operations per module to separate files (chirpstack, opc_ua, storage, config)
FR50: Web interface can require basic authentication (username/password) to access configuration and status pages

### NonFunctional Requirements

**Performance**
NFR1: OPC UA Read operations complete in <100ms for any single metric value
NFR2: Full poll cycle (100 devices x 4 metrics) completes within the configured polling interval (default 10s)
NFR3: Persistence write batch (400 metrics per poll cycle) completes in <500ms
NFR4: Gateway startup from persisted state completes in <10 seconds
NFR5: Memory usage remains bounded — target <256MB RSS for 100 devices
NFR6: CPU usage below 50% on NAS-class x86_64 during normal operation

**Security**
NFR7: API tokens and passwords never appear in log output at any log level
NFR8: Default configuration template contains no real credentials — placeholders only
NFR9: OPC UA certificate private keys stored with restricted file permissions (600)
NFR10: All OPC UA Write values destined for physical actuators validated before transmission
NFR11: Web UI requires authentication before any configuration change (basic auth minimum)
NFR12: Failed authentication attempts (OPC UA and web UI) logged with source IP

**Scalability**
NFR13: System handles 100 devices with 5 concurrent OPC UA clients at performance targets
NFR14: System degrades gracefully (increased latency, not crash) at 500 devices
NFR15: Historical data storage handles 7 days retention (~24M rows at 10s polling) — queries return in <2s

**Reliability**
NFR16: 30 days continuous operation without crash or manual intervention under production load
NFR17: Auto-recover from ChirpStack outages within 30 seconds of server availability returning
NFR18: No single malformed metric or device response crashes the gateway — errors logged and skipped
NFR19: Persistent database survives unclean shutdown (power loss, OOM kill) without data corruption
NFR20: Command queue guarantees FIFO ordering under all conditions including concurrent OPC UA writes

**Integration**
NFR21: Compatible with ChirpStack 4.x gRPC API
NFR22: OPC UA server compatible with FUXA SCADA and at least one additional OPC UA client (Phase B)
NFR23: Docker container supports standard lifecycle (start, stop, restart, logs) with mapped volumes
NFR24: Configuration supports environment variable overrides for all secrets

### Additional Requirements

From Architecture document:

- **Dependency updates:** Rust 1.87.0 → 1.94.0, tokio 1.47.1 → 1.50.0, tonic 0.13.1 → 0.14.5, tonic-build 0.13.1 → 0.14.5, chirpstack_api 4.13.0 → 4.15.0, async-opcua 0.16.x → 0.17.1, serde 1.0.219 → 1.0.228, clap 4.5.47 → 4.6.0, thiserror 2.0.16 → 2.0.18
- **log → tracing migration:** Replace log + log4rs with tracing + tracing-subscriber + tracing-appender. Mechanical find-and-replace of macro imports, programmatic config replaces log4rs.yaml. Remove log4rs.yaml.
- **New dependency: rusqlite 0.38.0** with `bundled` feature for SQLite persistence
- **New dependency: tokio-util 0.7.18** for CancellationToken graceful shutdown
- **New dependency: axum 0.8.8** (Phase B) for web UI server
- **Storage trait architecture:** Thin `StorageBackend` trait with simple get/set methods. `SqliteBackend` for production, `InMemoryBackend` for tests only.
- **SQLite schema (5 tables):** metric_values (UPSERT per poll), metric_history (append-only), command_queue (persistent FIFO), gateway_status (key-value), retention_config (pruning rules)
- **Concurrency model:** Each async task owns its own SQLite Connection — no shared lock. WAL mode enables concurrent readers + single writer.
- **Graceful shutdown:** CancellationToken from tokio-util, cloned to each task. SIGTERM handler cancels the token. Shutdown sequence: stop accepting connections → complete poll → flush SQLite → exit.
- **Config hot-reload preparation (Phase A):** Plumb tokio::sync::watch channel but don't activate until Phase B.
- **Docker deployment update:** New `./data` volume mapping for SQLite persistence.
- **New config section:** `[storage]` with database_path, retention_days, prune_interval_minutes.
- **tonic 0.13 → 0.14 migration:** Major version bump, verify AuthInterceptor API, test chirpstack_api compatibility.
- **async-opcua 0.16 → 0.17:** Check SimpleNodeManager API changes. Spike subscription support.
- **Migrations embedded via include_str!()** — no runtime file dependency.
- **Anti-patterns enforced:** No unwrap()/panic!() in production, no format!() in SQL, no shared SQLite connections, no logging without structured fields, no holding locks across .await.

### UX Design Requirements

N/A — opcgw is a headless gateway with no user interface in Phase A. Phase B web UI requirements are covered by FR34-41, FR50.

### FR Coverage Map

| FR | Epic | Description |
|----|------|-------------|
| FR1 | Epic 4 | Poll device metrics at configurable intervals |
| FR2 | Epic 4 | Authenticate with ChirpStack Bearer token |
| FR3 | Epic 4 | Retrieve metrics across multiple applications |
| FR4 | Epic 4 | Handle all metric types (Gauge, Counter, Absolute, Unknown) |
| FR5 | Epic 4 | Paginate API responses beyond 100 items |
| FR6 | Epic 4 | Detect ChirpStack unavailability |
| FR7 | Epic 4 | Auto-reconnect after outage (<30s) |
| FR8 | Epic 4 | Configurable retry count and delay |
| FR9 | Epic 3 | Send commands via OPC UA Write |
| FR10 | Epic 3 | FIFO command queue |
| FR11 | Epic 3 | Persist command queue across restarts |
| FR12 | Epic 3 | Validate command parameters |
| FR13 | Epic 3 | Report command delivery status |
| FR14 | Epic 5 | Expose metrics as OPC UA variables |
| FR15 | Epic 5 | Browse OPC UA address space |
| FR16 | Epic 5 | Read metrics with appropriate data types |
| FR17 | Epic 5 | Stale data via OPC UA status codes |
| FR18 | Epic 5 | Gateway health metrics in OPC UA |
| FR19 | Epic 6 | Multiple OPC UA security endpoints |
| FR20 | Epic 6 | OPC UA username/password authentication |
| FR21 | Epic 7 | Subscription-based data change notifications |
| FR22 | Epic 7 | Historical data queries (7-day retention) |
| FR23 | Epic 7 | Threshold-based alarm conditions |
| FR24 | Epic 8 | Dynamic OPC UA address space mutation |
| FR25 | Epic 2 | Persist last-known metric values |
| FR26 | Epic 2 | Restore values from storage on startup |
| FR27 | Epic 2 | Store historical metric data with timestamps |
| FR28 | Epic 2 | Prune historical data beyond retention period |
| FR29 | Epic 2 | Concurrent read/write without blocking |
| FR30 | Epic 2 | Batch metric writes per poll cycle |
| FR31 | Epic 1 | TOML file configuration |
| FR32 | Epic 1 | Environment variable overrides |
| FR33 | Epic 1 | Config validation with clear error messages |
| FR34 | Epic 8 | Web UI: application CRUD |
| FR35 | Epic 8 | Web UI: device/metric CRUD |
| FR36 | Epic 8 | Web UI: command CRUD |
| FR37 | Epic 8 | Web UI: live metric values |
| FR38 | Epic 8 | Web UI: gateway status |
| FR39 | Epic 8 | Hot-reload without restart |
| FR40 | Epic 8 | Config validation + rollback |
| FR41 | Epic 8 | Mobile-responsive LAN access |
| FR42 | Epic 6 | Load secrets from environment variables |
| FR43 | Epic 3 | Validate OPC UA Write inputs |
| FR44 | Epic 6 | Limit concurrent OPC UA connections |
| FR45 | Epic 6 | PKI certificate management |
| FR46 | Epic 1 | No panics in production paths |
| FR47 | Epic 1 | Graceful shutdown on SIGTERM |
| FR48 | Epic 1 | Clean startup from persisted state |
| FR49 | Epic 1 | Per-module logging |
| FR50 | Epic 8 | Web UI basic authentication |

## Epic 1: Crash-Free Gateway Foundation

Gateway runs 30+ days without crashing, handles all errors gracefully, provides structured logging for troubleshooting, and shuts down cleanly.
**FRs covered:** FR31, FR32, FR33, FR46, FR47, FR48, FR49

### Story 1.1: Update Dependencies and Rust Toolchain

As a **developer**,
I want all project dependencies updated to their target stable versions,
So that the codebase has the foundation needed for Phase A work (tonic 0.14, async-opcua 0.17, tracing, tokio-util).

**Acceptance Criteria:**

**Given** the current Cargo.toml with outdated dependencies
**When** I update Rust to 1.94.0 and all crate versions per the Architecture document
**Then** `cargo build` compiles successfully with zero errors
**And** `cargo test` passes all existing tests
**And** `cargo clippy` produces no warnings
**And** the tonic 0.14 AuthInterceptor API is verified working in chirpstack.rs
**And** async-opcua 0.17 SimpleNodeManager API changes are adapted in opc_ua.rs
**And** new dependencies added: tracing, tracing-subscriber, tracing-appender, tokio-util, rusqlite (bundled)

### Story 1.2: Migrate Logging from log4rs to Tracing

As an **operator**,
I want structured, per-module logging using the tracing ecosystem,
So that I can troubleshoot gateway issues with clear, filterable, async-native log output.

**Acceptance Criteria:**

**Given** the current log + log4rs logging setup
**When** I replace all `use log::{...}` imports with `use tracing::{...}` across all source files
**Then** all log macros (debug!, info!, warn!, error!, trace!) use structured key-value fields instead of format strings
**And** log4rs.yaml is removed and replaced with programmatic tracing-subscriber configuration in main.rs
**And** per-module file appenders are configured via tracing-appender (chirpstack, opc_ua, storage, config)
**And** `log` and `log4rs` crates are removed from Cargo.toml
**And** API tokens and passwords never appear in log output at any level (NFR7)
**And** `cargo test` passes all existing tests
**And** FR49 is satisfied (per-module logging to separate files)

### Story 1.3: Comprehensive Error Handling

As an **operator**,
I want the gateway to handle all errors gracefully without crashing,
So that the gateway runs continuously in production without manual intervention.

**Acceptance Criteria:**

**Given** the current codebase with potential unwrap()/panic!() calls
**When** I audit and replace all unwrap(), expect(), and panic!() in non-test code with Result<T, OpcGwError> propagation
**Then** zero unwrap()/expect()/panic!() calls remain in production code paths
**And** `OpcGwError` enum is extended with a `Database(String)` variant for SQLite errors
**And** non-fatal errors (single device fetch failure, malformed metric) are logged and skipped, not propagated (NFR18)
**And** fatal errors (SQLite corruption, OPC UA bind failure) propagate to main() for clean shutdown
**And** all error messages include relevant context (device_id, metric_name) via tracing structured fields
**And** `cargo clippy` produces no warnings
**And** FR46 is satisfied (no panics in production paths)

### Story 1.4: Graceful Shutdown with CancellationToken

As an **operator**,
I want the gateway to shut down cleanly on SIGTERM,
So that in-progress operations complete and no data is lost during Docker container restarts.

**Acceptance Criteria:**

**Given** the gateway is running with active poller and OPC UA server tasks
**When** a SIGTERM signal is received
**Then** a CancellationToken (from tokio-util) is cancelled in main()
**And** each spawned task checks `token.is_cancelled()` at its safe points (poller: between poll cycles; OPC UA: between request handling)
**And** the shutdown sequence executes: stop accepting new OPC UA connections → complete in-progress poll → exit
**And** the gateway exits with code 0 after clean shutdown
**And** a test validates that CancellationToken propagation works across tasks
**And** FR47 is satisfied (graceful shutdown on SIGTERM)

### Story 1.5: Configuration Validation and Clean Startup

As an **operator**,
I want clear, actionable error messages when configuration is invalid,
So that I can quickly fix problems and get the gateway running.

**Acceptance Criteria:**

**Given** the gateway is starting up
**When** the configuration file is loaded and validated
**Then** missing required fields produce error messages naming the specific field and expected format
**And** invalid values (bad URLs, negative intervals, unknown metric types) produce error messages with the invalid value and valid alternatives
**And** the config.toml format may differ from v1.0 — no backward-compatibility with the previous format is required (see Key Decision: Parallel Installation + Cutover)
**And** environment variable overrides (OPCGW_ prefix) are applied after file loading and before validation
**And** the gateway exits with a non-zero code and clear error message on invalid config (not a panic)
**And** valid configuration produces an info-level log confirming successful load with key parameters (poll interval, device count, OPC UA endpoint)
**And** FR31, FR32, FR33 are satisfied
**And** FR48 is partially satisfied (clean startup path; full persistence startup deferred to Epic 2)

---

## Epic 2: Data Persistence

All metric data and gateway state survive restarts. No data loss on container replacement or power failure. Historical data stored for later retrieval.
**FRs covered:** FR25, FR26, FR27, FR28, FR29, FR30

### Story 2.1: StorageBackend Trait and InMemoryBackend

As a **developer**,
I want a thin StorageBackend trait with an in-memory implementation,
So that all modules can be refactored against a clean storage interface and unit tests run without SQLite.

**Acceptance Criteria:**

**Given** the current `Arc<Mutex<Storage>>` with HashMap-based storage
**When** I create a `storage/` module directory with `mod.rs`, `memory.rs`
**Then** `StorageBackend` trait defines simple get/set methods for metric values, gateway status, and command queue operations
**And** `InMemoryBackend` implements the trait using HashMap (for tests only)
**And** existing data types (MetricType: Float, Int, Bool, String) are defined in `storage/mod.rs`
**And** `DeviceCommand` struct is defined with fields for FIFO queue (device_id, payload, status, created_at)
**And** `ChirpstackStatus` struct is defined for gateway health tracking
**And** all existing tests pass using InMemoryBackend
**And** FR29 is addressed (trait designed for concurrent access)

### Story 2.2: SQLite Backend and Schema Migration

As an **operator**,
I want metric data persisted in SQLite so values survive gateway restarts,
So that I never lose data on container replacement or power failure.

**Acceptance Criteria:**

**Given** the StorageBackend trait from Story 2.1
**When** I create `storage/sqlite.rs` and `storage/schema.rs`
**Then** `SqliteBackend` implements `StorageBackend` using rusqlite with `bundled` feature
**And** SQLite is opened in WAL journal mode for crash safety (NFR19)
**And** schema creates 5 tables: metric_values, metric_history, command_queue, gateway_status, retention_config
**And** migration SQL files in `migrations/` are embedded via `include_str!()` — no runtime file dependency
**And** schema versioning supports future migrations (version table or pragma)
**And** `[storage]` config section added: database_path, retention_days, prune_interval_minutes
**And** Docker volume mapping `./data:/usr/local/bin/data` documented
**And** integration tests verify schema creation and basic CRUD operations
**And** FR25 is satisfied (persist metric values in embedded database)

### Story 2.3: Metric Persistence and Batch Writes

As an **operator**,
I want all polled metrics persisted efficiently in SQLite each poll cycle,
So that metric values are always durable and write performance doesn't impact polling.

**Acceptance Criteria:**

**Given** a running gateway with SqliteBackend
**When** the poller completes a poll cycle with metrics for N devices
**Then** all metric values are written in a single SQLite transaction (batch write)
**And** metric_values table uses UPSERT (INSERT OR REPLACE) keyed on (device_id, metric_name)
**And** metric_history table receives append-only INSERTs with timestamps
**And** batch write for 400 metrics completes in <500ms (NFR3)
**And** prepared statements are used for all queries — no format!() in SQL
**And** the poller owns its own SQLite Connection (not shared with other tasks)
**And** FR27 is satisfied (historical data with timestamps)
**And** FR30 is satisfied (batch writes per poll cycle)

### Story 2.4: Metric Restore on Startup

As an **operator**,
I want the gateway to restore last-known metric values from SQLite on startup,
So that SCADA clients see valid data immediately instead of empty values after a restart.

**Acceptance Criteria:**

**Given** a gateway that previously persisted metric values to SQLite
**When** the gateway starts and loads configuration
**Then** last-known metric values are loaded from metric_values table into the OPC UA address space
**And** startup with 100 devices completes in <10 seconds (NFR4)
**And** if the database file doesn't exist, the gateway starts cleanly with empty state (no crash)
**And** if the database is corrupted, the error is logged and the gateway starts with empty state (graceful degradation)
**And** FR26 is satisfied (restore values from persistent storage)
**And** FR48 is fully satisfied (clean startup from persisted state)

### Story 2.5: Historical Data Pruning

As an **operator**,
I want historical data automatically pruned beyond the configured retention period,
So that SQLite storage doesn't grow unbounded and the NAS disk stays healthy.

**Acceptance Criteria:**

**Given** a running gateway with historical data accumulating in metric_history
**When** the configured prune_interval_minutes elapses
**Then** rows older than retention_days are deleted from metric_history
**And** pruning runs as a periodic task within the poller (reuses poller's SQLite connection)
**And** pruning uses DELETE with a date comparison, not TRUNCATE
**And** pruning logs the number of rows deleted at debug level
**And** memory usage remains bounded over weeks of operation (NFR5)
**And** FR28 is satisfied (prune historical data beyond retention)

---

## Epic 3: Reliable Command Execution

Valve and actuator commands execute in correct FIFO order, survive gateway restarts, are validated before forwarding to ChirpStack, and report delivery status.
**FRs covered:** FR9, FR10, FR11, FR12, FR13, FR43

### Story 3.1: SQLite-Backed FIFO Command Queue

As a **SCADA operator**,
I want valve commands queued persistently in FIFO order,
So that commands execute in the correct sequence and survive gateway restarts.

**Acceptance Criteria:**

**Given** the command_queue table from Epic 2's schema
**When** an OPC UA Write operation inserts a command
**Then** the command is persisted in SQLite with status "pending" and a created_at timestamp
**And** the poller selects pending commands ordered by created_at ASC (FIFO)
**And** after successful delivery to ChirpStack, the command status is updated to "sent"
**And** on delivery failure, the command status is updated to "failed" with error details
**And** FIFO ordering is maintained under concurrent OPC UA writes from multiple clients (NFR20)
**And** commands persisted before a restart are still present and processed after restart (FR11)
**And** integration test verifies FIFO ordering with concurrent inserts

### Story 3.2: Command Parameter Validation

As a **SCADA operator**,
I want command parameters validated before forwarding to ChirpStack,
So that malformed commands never reach physical actuators and cause damage.

**Acceptance Criteria:**

**Given** an OPC UA Write operation with command parameters
**When** the system receives the command
**Then** type checking validates the payload matches the expected data type for the target device
**And** range checking validates values are within configured min/max bounds
**And** f_port validation ensures the port number is valid for the target device
**And** invalid commands are rejected with a clear OPC UA status code (BadTypeMismatch, BadOutOfRange)
**And** rejected commands are logged with device_id, parameter details, and rejection reason
**And** valid commands proceed to the FIFO queue from Story 3.1
**And** FR12 is satisfied (validate type, range, f_port)
**And** FR43 is satisfied (validate all OPC UA Write inputs)
**And** NFR10 is satisfied (all actuator commands validated before transmission)

### Story 3.3: Command Delivery Status Reporting

As a **SCADA operator**,
I want to see the delivery status of commands I've sent,
So that I know whether a valve command was actually delivered to ChirpStack.

**Acceptance Criteria:**

**Given** commands in the command_queue with various statuses
**When** a SCADA client reads the command status OPC UA variable for a device
**Then** the most recent command's status is visible: "pending", "sent", or "failed"
**And** the status is read from SQLite (not cached in memory)
**And** failed commands include an error description accessible via OPC UA
**And** FR9 is satisfied (send commands via OPC UA Write)
**And** FR10 is satisfied (FIFO queue delivery to ChirpStack)
**And** FR13 is satisfied (report delivery status)

---

## Epic 4: Scalable Data Collection

All ChirpStack metric types work correctly, system handles 100+ devices with pagination, and auto-recovers from ChirpStack outages within 30 seconds.
**FRs covered:** FR1, FR2, FR3, FR4, FR5, FR6, FR7, FR8

### Story 4.1: Poller Refactoring to SQLite Backend

As a **developer**,
I want the ChirpStack poller refactored to use its own SQLite connection via the StorageBackend trait,
So that the poller no longer depends on shared in-memory storage and data is persisted every poll cycle.

**Acceptance Criteria:**

**Given** the existing poller using `Arc<Mutex<Storage>>`
**When** I refactor chirpstack.rs to accept a `SqliteBackend` with its own Connection
**Then** the poller writes metrics to SQLite via the StorageBackend trait (batch writes from Story 2.3)
**And** the poller updates gateway_status table with poll timestamps and connection state
**And** the `Arc<Mutex<Storage>>` is removed from the poller path
**And** no locks are held across `.await` points
**And** existing polling logic (FR1, FR2, FR3) continues to work correctly
**And** all existing poller tests are updated to use InMemoryBackend

### Story 4.2: Support All ChirpStack Metric Types

As an **operator**,
I want all ChirpStack metric types handled correctly,
So that every sensor reports accurate data regardless of its metric format.

**Acceptance Criteria:**

**Given** ChirpStack devices reporting metrics of various types
**When** the poller fetches metrics from the gRPC API
**Then** Gauge metrics are stored as Float values
**And** Counter metrics are stored as Int values with monotonic tracking
**And** Absolute metrics are stored as Float values
**And** Unknown metric types are logged at warn level and skipped (not crash)
**And** each metric type maps to the correct OPC UA data type (Boolean, Int32, Float, String)
**And** unit tests cover all four metric type conversions
**And** FR4 is satisfied (handle Gauge, Counter, Absolute, Unknown)

### Story 4.3: API Pagination for Large Deployments

As an **operator**,
I want the gateway to handle more than 100 devices and applications,
So that the system scales to my full deployment without silently missing devices.

**Acceptance Criteria:**

**Given** a ChirpStack instance with more than 100 devices or applications
**When** the poller fetches device and application lists
**Then** the poller paginates through all pages until no more results are returned
**And** page size is configurable (default 100)
**And** total device/application count is logged at info level after pagination completes
**And** the full poll cycle for 100 devices x 4 metrics completes within the polling interval (NFR2)
**And** the system degrades gracefully at 500 devices — increased latency, not crash (NFR14)
**And** FR5 is satisfied (paginate beyond 100 items)

### Story 4.4: Auto-Recovery from ChirpStack Outages

As an **operator**,
I want the gateway to automatically reconnect after a ChirpStack outage,
So that I never need to manually restart the gateway after network or server issues.

**Acceptance Criteria:**

**Given** the gateway is polling and ChirpStack becomes unavailable
**When** a connection failure is detected
**Then** the failure is detected via TCP connectivity check (FR6)
**And** the gateway retries with configurable retry count and delay (FR8)
**And** between retries, the poller updates gateway_status to "unavailable" in SQLite
**And** when ChirpStack returns, the gateway reconnects and resumes polling within 30 seconds (NFR17)
**And** no manual intervention is required — recovery is fully automatic (FR7)
**And** the reconnection is logged at info level with downtime duration
**And** during outage, existing last-known values remain available to OPC UA clients (no data cleared)
**And** a test validates the retry → reconnect → resume cycle

---

## Epic 5: Operational Visibility

SCADA operators see clear stale-data warnings, can monitor gateway health directly in FUXA, and never act on silently stale values.
**FRs covered:** FR14, FR15, FR16, FR17, FR18

### Story 5.1: OPC UA Server Refactoring to SQLite Backend

As a **developer**,
I want the OPC UA server refactored to read metrics from its own SQLite connection,
So that OPC UA reads are decoupled from the poller and no shared locks exist between tasks.

**Acceptance Criteria:**

**Given** the existing OPC UA server reading from `Arc<Mutex<Storage>>`
**When** I refactor opc_ua.rs to accept a `SqliteBackend` with its own read Connection
**Then** OPC UA variable reads query SQLite directly via the StorageBackend trait
**And** the `Arc<Mutex<Storage>>` is fully removed from the codebase (both poller and OPC UA migrated)
**And** OPC UA Read operations complete in <100ms (NFR1)
**And** no locks are held across `.await` points
**And** the OPC UA address space still organizes variables by Application > Device > Metric hierarchy (FR14)
**And** SCADA clients can browse and discover all configured devices and metrics (FR15)
**And** metric values are served with correct OPC UA data types: Boolean, Int32, Float, String (FR16)

### Story 5.2: Stale Data Detection and Status Codes

As a **SCADA operator**,
I want to see clear warnings when sensor data is stale,
So that I never make irrigation decisions based on outdated readings.

**Acceptance Criteria:**

**Given** metrics with last-updated timestamps in SQLite
**When** a metric's age exceeds a configurable staleness threshold (default: 2x polling frequency)
**Then** the OPC UA variable's status code is set to `UncertainLastUsableValue`
**And** the staleness threshold is configurable per device or globally in config.toml
**And** when fresh data arrives, the status code returns to `Good`
**And** stale metrics still return their last-known value (not empty)
**And** the staleness check is performed on each OPC UA read (not cached)
**And** FR17 is satisfied (stale data via OPC UA status codes)

### Story 5.3: Gateway Health Metrics in OPC UA

As a **SCADA operator**,
I want to see gateway health directly in FUXA,
So that I can monitor the gateway's status without SSH or log access.

**Acceptance Criteria:**

**Given** the gateway is running and polling ChirpStack
**When** a SCADA client browses the OPC UA address space
**Then** an `Objects/Gateway/` folder is visible containing health variables
**And** `LastPollTimestamp` shows the timestamp of the last successful poll cycle
**And** `ErrorCount` shows the cumulative error count since startup
**And** `ChirpStackAvailable` shows a Boolean indicating current connection state
**And** health metrics are read from the gateway_status SQLite table
**And** health variables update every poll cycle
**And** FR18 is satisfied (gateway health metrics in OPC UA address space)

---

## Epic 6: Security Hardening

Production deployment is secure by default — no exposed credentials, validated inputs, controlled access, proper OPC UA security endpoints.
**FRs covered:** FR19, FR20, FR42, FR44, FR45

### Story 6.1: Credential Management via Environment Variables

As an **operator**,
I want API tokens and passwords loaded from environment variables by default,
So that secrets are never committed to config files or exposed in logs.

**Acceptance Criteria:**

**Given** a production deployment with Docker Compose
**When** the gateway loads configuration
**Then** ChirpStack API token is read from environment variable (OPCGW_CHIRPSTACK_API_TOKEN or similar)
**And** OPC UA passwords are read from environment variables
**And** the default config.toml template contains placeholder values only — no real credentials (NFR8)
**And** if a secret is configured in both env var and config file, the env var takes precedence
**And** API tokens and passwords never appear in log output at any level (NFR7)
**And** missing required secrets produce a clear startup error (not a panic)
**And** FR42 is satisfied (load secrets from env vars)

### Story 6.2: OPC UA Security Endpoints and Authentication

As an **operator**,
I want the OPC UA server to support multiple security levels and authenticate clients,
So that unauthorized SCADA clients cannot read sensor data or send commands.

**Acceptance Criteria:**

**Given** the OPC UA server configuration
**When** the server starts
**Then** three security endpoints are available: None, Basic256 Sign, Basic256 SignAndEncrypt (FR19)
**And** username/password authentication can be enabled via configuration (FR20)
**And** failed authentication attempts are logged with source IP address (NFR12)
**And** OPC UA certificate private keys are stored with restricted file permissions (600) (NFR9)
**And** the PKI directory structure (own/, private/, trusted/, rejected/) is managed correctly (FR45)
**And** `create_sample_keypair` defaults to `false` in release builds

### Story 6.3: Connection Limiting

As an **operator**,
I want OPC UA client connections limited to a configurable maximum,
So that the gateway is protected from resource exhaustion by too many concurrent clients.

**Acceptance Criteria:**

**Given** the OPC UA server is running
**When** concurrent client connections reach the configured maximum
**Then** new connection attempts are rejected with an appropriate OPC UA status code
**And** the maximum is configurable in config.toml (default: 10)
**And** rejected connections are logged at warn level with source IP
**And** existing connected clients are not affected by the limit
**And** FR44 is satisfied (limit concurrent OPC UA connections)

---

## Epic 7: Real-Time Subscriptions & Historical Data (Phase B)

SCADA receives real-time data change notifications (no polling delay), can view 7-day historical trends, and gets threshold-based alarm conditions.
**FRs covered:** FR21, FR22, FR23

### Story 7.1: async-opcua Subscription Spike

As a **developer**,
I want to validate that async-opcua supports OPC UA subscriptions,
So that Phase B can proceed with confidence or trigger Plan B early.

**Acceptance Criteria:**

**Given** async-opcua 0.17.x as the OPC UA server library
**When** I create a minimal proof-of-concept that pushes DataValue changes and tests subscription notification delivery
**Then** the spike documents whether subscriptions work with async-opcua's API
**And** if subscriptions work: document the API surface, push model integration points, and any limitations
**And** if subscriptions don't work: document Plan B options (locka99/opcua crate, upstream contribution, change-detection polling layer)
**And** the spike is tested with at least FUXA as the subscribing client
**And** findings are documented in a spike report for architecture reference
**And** the spike code is kept as a reference test, not production code

### Story 7.2: OPC UA Subscription Support

As a **SCADA operator**,
I want real-time data change notifications from the gateway,
So that FUXA updates instantly when sensor values change without polling delay.

**Acceptance Criteria:**

**Given** the spike from Story 7.1 confirms subscription support (or Plan B is implemented)
**When** a SCADA client creates a subscription on one or more metric variables
**Then** the client receives data change notifications when the poller updates metric values
**And** the poller pushes DataValues (with timestamps + status codes) into the OPC UA address space after each poll cycle
**And** async-opcua's subscription engine detects value changes and notifies subscribed clients automatically
**And** multiple clients can subscribe to the same variables simultaneously
**And** subscriptions survive temporary ChirpStack outages (stale status codes are notified, not subscription drops)
**And** tested with FUXA and at least one additional client — UaExpert recommended (NFR22)
**And** FR21 is satisfied (subscription-based data change notifications)

### Story 7.3: Historical Data Access via OPC UA

As a **SCADA operator**,
I want to view historical metric trends in FUXA,
So that I can analyze soil moisture patterns over the past week instead of guessing.

**Acceptance Criteria:**

**Given** historical metric data accumulated in metric_history table (from Epic 2)
**When** a SCADA client issues an OPC UA Historical Read request for a metric variable
**Then** the server returns timestamped values for the requested time range
**And** data is served from SQLite metric_history table via the OPC UA server's read connection
**And** queries for 7-day ranges across 24M rows return in <2 seconds (NFR15)
**And** time range boundaries are respected — no data outside the requested range
**And** if no data exists for the range, an empty result is returned (not an error)
**And** FR22 is satisfied (historical data queries, 7-day retention)

### Story 7.4: Threshold-Based Alarm Conditions

As a **SCADA operator**,
I want the gateway to flag alarm conditions when metrics cross configured thresholds,
So that FUXA can alert me when soil moisture drops too low or temperature rises too high.

**Acceptance Criteria:**

**Given** configurable threshold values per metric in config.toml (low_alarm, high_alarm)
**When** a metric value crosses a configured threshold
**Then** the OPC UA variable's status code changes to Bad or Warning (not full Alarms & Conditions)
**And** when the value returns within normal range, the status code returns to Good
**And** threshold crossings are logged at info level with device_id, metric_name, value, and threshold
**And** thresholds are optional per metric — metrics without thresholds always show Good status
**And** stale-data status (from Epic 5) takes precedence over threshold status
**And** FR23 is satisfied (threshold-based alarm conditions via status codes)

---

## Epic 8: Web Configuration & Hot-Reload (Phase B)

Configure devices from any browser on the LAN, see live metric values for debugging, changes apply without gateway restart.

### Story 8.1: Axum Web Server and Basic Authentication

As an **operator**,
I want a lightweight web server embedded in the gateway with authentication,
So that I can access configuration and status pages securely from any device on the LAN.

**Acceptance Criteria:**

**Given** the gateway running with the new `web/` module (Axum 0.8)
**When** the web server starts alongside the poller and OPC UA server
**Then** the web server listens on a configurable HTTP port (LAN-accessible)
**And** all routes require basic authentication (username/password from config) (FR50, NFR11)
**And** failed authentication attempts are logged with source IP (NFR12)
**And** the web server shares the Tokio runtime with other tasks
**And** the web server respects the CancellationToken for graceful shutdown
**And** static HTML files are served from `static/` directory
**And** the web UI is mobile-responsive (FR41)

### Story 8.2: Gateway Status Dashboard

As an **operator**,
I want to see gateway status at a glance in a web browser,
So that I can check health from my phone while in the field.

**Acceptance Criteria:**

**Given** the authenticated web server from Story 8.1
**When** I navigate to the dashboard page
**Then** I see ChirpStack connection state (available/unavailable)
**And** I see last successful poll timestamp
**And** I see cumulative error count
**And** I see total device and application counts
**And** status data is read from the gateway_status SQLite table via the web server's own connection
**And** FR38 is satisfied (gateway status via web interface)

### Story 8.3: Live Metric Values Display

As an **operator**,
I want to see live metric values for all devices in the web UI,
So that I can verify a newly installed sensor is reporting correctly from the field.

**Acceptance Criteria:**

**Given** the authenticated web server
**When** I navigate to the live metrics page
**Then** I see all devices organized by application with their current metric values
**And** each metric shows its value, data type, last-updated timestamp, and staleness status
**And** values are read from the metric_values SQLite table
**And** the page auto-refreshes or provides a manual refresh button
**And** FR37 is satisfied (live metric values via web interface)

### Story 8.4: Application CRUD via Web UI

As an **operator**,
I want to manage applications through the web interface,
So that I can add or modify application configurations without editing TOML files.

**Acceptance Criteria:**

**Given** the authenticated web server
**When** I navigate to the applications page
**Then** I can view all configured applications
**And** I can create a new application with name and ChirpStack application ID
**And** I can edit an existing application's properties
**And** I can delete an application (with confirmation)
**And** changes are validated before saving
**And** changes are persisted to both SQLite and the TOML config file
**And** FR34 is satisfied (application CRUD via web interface)

### Story 8.5: Device and Metric Mapping CRUD via Web UI

As an **operator**,
I want to manage devices and their metric mappings through the web interface,
So that I can onboard a new sensor in 30 seconds from my phone in the field.

**Acceptance Criteria:**

**Given** the authenticated web server with applications configured
**When** I navigate to the devices page for an application
**Then** I can view all devices with their metric mappings
**And** I can create a new device with name, DevEUI, and metric mappings
**And** I can edit device properties and add/remove/modify metric mappings
**And** I can delete a device (with confirmation)
**And** FR35 is satisfied (device/metric CRUD via web interface)

### Story 8.6: Command CRUD via Web UI

As an **operator**,
I want to manage device commands through the web interface,
So that I can configure new valve commands without editing config files.

**Acceptance Criteria:**

**Given** the authenticated web server with devices configured
**When** I navigate to the commands page for a device
**Then** I can view all configured commands for the device
**And** I can create a new command with name, f_port, payload template, and validation rules
**And** I can edit command properties
**And** I can delete a command (with confirmation)
**And** FR36 is satisfied (command CRUD via web interface)

### Story 8.7: Configuration Hot-Reload

As an **operator**,
I want configuration changes applied without restarting the gateway,
So that I can add a device from the web UI and see it in FUXA within one poll cycle.

**Acceptance Criteria:**

**Given** a running gateway with active OPC UA client connections
**When** a configuration change is saved via the web UI
**Then** the change is validated before applying (FR40)
**And** on validation failure, the change is rejected with a clear error message and no state is modified
**And** on success, a `tokio::sync::watch` notification is sent to all tasks
**And** the poller picks up new/modified devices at the next poll cycle boundary
**And** existing OPC UA client connections are not dropped during reload
**And** FR39 is satisfied (hot-reload without restart)

### Story 8.8: Dynamic OPC UA Address Space Mutation

As an **operator**,
I want new devices to appear in the OPC UA address space after hot-reload,
So that FUXA sees the new sensor without reconnecting.

**Acceptance Criteria:**

**Given** a configuration change adding a new device (via web UI hot-reload)
**When** the OPC UA server receives the config change notification
**Then** new OPC UA nodes (folders + variables) are added for the new device
**And** removed devices have their OPC UA nodes removed from the address space
**And** modified devices have their nodes updated (metric additions/removals)
**And** existing subscriptions on unaffected nodes continue without interruption
**And** the new device's metrics are available within one poll cycle
**And** FR24 is satisfied (add/remove OPC UA nodes at runtime)

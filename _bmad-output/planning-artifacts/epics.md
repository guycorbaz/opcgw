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

**Numbering offset:** this file's "Epic 7" maps to `_bmad-output/implementation-artifacts/sprint-status.yaml`'s **"Epic 8: Real-Time Subscriptions & Historical Data (Phase B)"**, because Phase A was renumbered after this file was authored. Stories created via `bmad-create-story` use the sprint-status numbering (`8-1`, `8-2`, `8-3`, `8-4`).

**Phase A carry-forward (Epic 7 retrospective, 2026-04-29 — see [`epic-7-retro-2026-04-29.md`](../implementation-artifacts/epic-7-retro-2026-04-29.md)):**

- **NFR12 source-IP audit applies to every session, including subscription clients.** Subscription clients pass through `OpcgwAuthManager` (Story 6.2 → sprint-status 7-2) without modification — the existing `event="opcua_auth_failed"` warn + async-opcua `info!` accept event two-event correlation covers them. Connection-cap rejection is covered by `AtLimitAcceptLayer`. Issue #91's startup warn alerts operators when the global level filters out info. **No new audit infrastructure required for Epic 7;** stories below acknowledge this explicitly so reviewers don't expect new auth/audit code.
- **Per-IP rate limiting (#88) becomes structurally relevant once subscriptions land.** A single authenticated client can spawn N sessions × M subscriptions × K monitored items × R notifications/s — the flat global `max_connections` cap from sprint-status 7-3 doesn't shape this load. Surface as a known gap; **promote to a story precondition only if subscription-flood becomes a near-term operator concern.** Current LAN-internal threat model defers it. Tracked at GitHub issue #88.
- **Subscription / message-size limits (#89) must be config knobs.** async-opcua's `Limits` struct exposes `max_subscriptions_per_session`, `max_monitored_items_per_subscription`, `max_message_size`, `max_chunk_count`, `max_pending_publish_requests`, and similar caps. Today they default to async-opcua's library values, which are unknown-to-the-operator for a 1000-monitored-items deployment shape. **Story 7.2 must surface at least the first four as `[opcua]` config knobs with documented defaults.** The spike (Story 7.1) is responsible for confirming which `Limits` fields are reachable through async-opcua 0.17.x's `ServerBuilder`. Tracked at GitHub issue #89.
- **Gauge interval tunability** — `OPCUA_SESSION_GAUGE_INTERVAL_SECS` (currently a hard-coded 5-second constant in `src/utils.rs`) may want sub-second cadence under subscription load for ops visibility. Minor; promote to an `[diagnostics]` config knob in Story 7.2 only if the spike confirms the gauge remains a useful operator signal once notifications are flowing.
- **Upstream feature request: async-opcua session-rejected callback.** sprint-status 7-3's `AtLimitAcceptLayer` is a `tracing-subscriber::Layer` bridging a missing upstream callback (async-opcua's `SessionManager::create_session` rejects (N+1)th sessions silently). File an upstream feature request **before Story 7.2 begins**; if upstream ships a callback in time, 7.2 can simplify by removing the layer-as-control-plane workaround. If not, the workaround stays — bounded blast radius.

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
**And** the spike enumerates which `async_opcua::server::config::Limits` fields are reachable through `ServerBuilder` in 0.17.x and which are not (input for Story 7.2's config-knob surface — see Phase A carry-forward bullet on issue #89)
**And** the spike checks whether async-opcua 0.17.x exposes a session-rejected callback (or any new hook the `AtLimitAcceptLayer` workaround could be retired in favour of); if not, an upstream feature request is filed before Story 7.2 begins (Phase A carry-forward action item)
**And** the spike measures notification throughput under a representative load (e.g. 100 monitored items × 1 Hz update rate × 1 subscriber) to surface whether the 5 s session-count gauge cadence remains a useful operator signal under subscription load (input for the gauge-tunability decision in Story 7.2)
**And** the spike confirms whether subscription clients flow through `OpcgwAuthManager` and `AtLimitAcceptLayer` without modification (Phase A carry-forward NFR12 acknowledgment — expected: yes, since both gates run at the session layer below subscription state)

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
**And** the following async-opcua `Limits` fields are surfaced as `[opcua]` config knobs with documented defaults and `OPCGW_OPCUA__*` env-var override support (Phase A carry-forward / GitHub issue #89): `max_subscriptions_per_session`, `max_monitored_items_per_subscription`, `max_message_size`, `max_chunk_count`. The four-knob list is the **minimum**; the spike (Story 7.1) may surface additional `Limits` fields that should also be exposed — extend the list at story-creation time.
**And** the four config knobs are documented in `docs/security.md` "OPC UA connection limiting" section (extension of the existing connection-cap subsection) with: per-knob default value, threat-model rationale, recommended tuning ceiling, env-var override name, and a grep recipe for the `info!` startup log line that reports the resolved values. `AppConfig::validate` rejects zero / negative / above-hard-cap values with the same accumulation pattern used for `max_connections`.
**And** subscription clients pass through `OpcgwAuthManager` and `AtLimitAcceptLayer` without modification (Phase A carry-forward NFR12 acknowledgment — no new auth or audit-event infrastructure introduced by this story; the existing `event="opcua_auth_failed"` and `event="opcua_session_count_at_limit"` warns cover subscription clients identically to read-only clients). An integration test pins this contract by exercising a wrong-password subscription-creating client against the `null` endpoint and asserting the existing audit-event line fires on the failed-auth path.
**And** **(conditional)** if the Story 7.1 spike confirms the periodic `event="opcua_session_count"` gauge remains a useful operator signal under representative subscription load, `OPCUA_SESSION_GAUGE_INTERVAL_SECS` (currently a hard-coded 5 s constant in `src/utils.rs`) is promoted to a `[diagnostics].session_gauge_interval_secs` config knob with `OPCGW_DIAGNOSTICS__SESSION_GAUGE_INTERVAL_SECS` env-var override; otherwise the constant remains hard-coded and a deferred-work entry records the rationale (subscription-flush noise drowned the gauge / gauge replaced by a per-subscription notification-rate metric / etc.).
**And** **(out-of-scope reminder for reviewers)** per-IP rate limiting / token-bucket throttling is **not** in scope for this story (Phase A carry-forward bullet — issue #88). The flat global `max_connections` cap and per-session `max_subscriptions_per_session` / `max_monitored_items_per_subscription` knobs constitute the load-shaping surface in this story; per-IP throttling is a separate Phase B story to be opened only if subscription-flood becomes a near-term operator concern.

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

## Epic A: Storage Payload Migration (Phase B Closure)

**Numbering offset:** this epic uses the literal name `Epic A` in `sprint-status.yaml`, not a numeric index, to flag its corrective nature (it closes a gap discovered after Phase B's stories shipped, rather than introducing new capability). Stories created via `bmad-create-story` use sprint-status keys `A-1` through `A-7`.

**Why it exists:** Issue [#108](https://github.com/guycorbaz/opcgw/issues/108) surfaced during Story 9-3 code review (2026-05-03). The `MetricType` enum shipped in Epic 1 of this file (sprint-status Epic 2, Data Persistence) is payload-less; every row in `metric_values.value` stores the data-type discriminant string (`"Float"`, `"Int"`, `"Bool"`, `"String"`) instead of the real measurement. Four shipped epics — Epic 1 (persistence), Epic 3 (sprint-status 5, OPC UA visibility), Epic 7 (sprint-status 8, real-time + historical), and Story 7.3 (sprint-status Story 9-3, live metrics dashboard) — are surface-correct but data-incorrect. opcgw has never persisted real metric values. The Epic 9 retrospective (2026-05-14) action item AI6 identifies Epic A as the immediate next epic and the production-deployment blocker for v2.0 GA.

**FRs covered:** FR51 (new — see PRD § Data Persistence).

**Sequencing:** Immediate next epic. Gates the v2.0 GA release. All shipped Phase A + Phase B functionality continues to work structurally — only the value-payload contract is changed. Story 8-4 (sprint-status 8-4, threshold-based alarms, descoped 2026-05-14 from Epic 7 of this file / Epic 8 of sprint-status) revival depends on Epic A landing.

### Story A.1: MetricType Payload-Bearing Enum + StorageBackend Trait Amendment

As a **gateway internal**,
I want `MetricType` to carry the actual measurement payload (`Float(f64)`, `Int(i64)`, `Bool(bool)`, `String(String)`),
So that the storage trait round-trips the real value end-to-end instead of flattening to the discriminant string.

**Acceptance Criteria:**

**Given** the storage trait `StorageBackend::set_metric_value(&MetricValue)` accepts a `MetricValue` carrying the new payload-bearing `MetricType`,
**When** a poller writes a Float metric with value `23.5`,
**Then** the round-trip through `get_metric_value(...)` returns `MetricType::Float(23.5)` (not `MetricType::Float` with no payload).
**And** all four variants are covered by unit tests in `src/storage/in_memory.rs::tests` + `src/storage/sqlite.rs::tests`.
**And** the `OpcGwError::Storage` variant covers any payload-conversion error path.

### Story A.2: SQLite Schema Migration v007 (Typed Value Columns)

As a **deployed opcgw gateway**,
I want the SQLite schema upgraded to store metric values in typed columns (`value_real REAL NULL`, `value_int INTEGER NULL`, `value_bool INTEGER NULL`, `value_text TEXT NULL`) keyed by `value_type`,
So that the value payload survives the persistence layer with type fidelity.

**Acceptance Criteria:**

**Given** an existing v006 database with pre-Epic-A rows (`value TEXT` holding the discriminant string),
**When** the gateway starts against the database,
**Then** migration v007 adds the four typed columns + a `value_type` discriminant column without dropping rows.
**And** pre-Epic-A rows have `value_type = 'legacy'` and NULL typed columns; the OPC UA reader returns `BadDataUnavailable` for these until the next poll cycle UPSERTs a real payload.
**And** the `metric_history` table receives the same column additions for the HistoryRead path.
**And** rollback path: dropping the database file before upgrading is a documented operator option for instances with no production-value history.

### Story A.3: Poller Value-Payload Write Pipeline

As a **gateway poller**,
I want `ChirpstackPoller` to wrap real measurement values into the payload-bearing `MetricType` variants at the point of reception,
So that the value persisted by `SqliteBackend::set_metric_value` carries the real measurement.

**Acceptance Criteria:**

**Given** a ChirpStack metric arrives with a Float value `23.5`,
**When** the poller constructs the `MetricValue`,
**Then** the resulting `MetricType::Float(23.5)` is persisted (not `MetricType::Float`).
**And** all four ChirpStack metric types (Gauge, Counter, Absolute, Unknown) map to the correct `MetricType` variant.
**And** parse failures emit `event = "metric_parse"` at `warn` level with structured `device_id`, `metric_name`, `raw_value`, `expected_type` (Story 6-3 pattern carry-forward).
**And** existing Story 4-4 outage-recovery semantics are unchanged.

### Story A.4: OPC UA Read Value-Payload Pipeline

As a **SCADA client connected to opcgw**,
I want `OpcUa::get_value` to return the actual measurement payload in the OPC UA `Variant`,
So that `Read` operations return `Variant::Double(23.5)` / `Variant::Int64(42)` / `Variant::Boolean(true)` / `Variant::String("OK")` instead of the discriminant string.

**Acceptance Criteria:**

**Given** a Float metric persisted with value `23.5`,
**When** an OPC UA `Read` fires on that metric variable,
**Then** the returned `DataValue` carries `Variant::Double(23.5)` with `StatusCode::Good`.
**And** Story 5-2 stale-data status codes continue to apply when the metric is stale (precedence rule preserved).
**And** Story 9-7 hot-reload of `stale_threshold_seconds` continues to work (post-#113 closure-capture limitation preserved).
**And** all four payload variants are covered by integration tests.
**And** strict-zero file invariants: `src/web/auth.rs`, `src/security*.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/main.rs::initialise_tracing` untouched.

### Story A.5: OPC UA HistoryRead Value-Payload Pipeline

As a **SCADA client connected to opcgw**,
I want `OpcgwHistoryNodeManagerImpl::history_read_raw_modified` to return historical rows with real measurement payloads in each `DataValue`,
So that `HistoryRead` returns the value-over-time series instead of a wall of discriminant strings.

**Acceptance Criteria:**

**Given** a Float metric with 7 days of historical rows in `metric_history`,
**When** an OPC UA `HistoryRead` fires for the 7-day range,
**Then** each returned `DataValue` carries the corresponding `Variant::Double` payload with the original `recorded_at` timestamp (microsecond precision preserved from Story 8-3).
**And** pre-Epic-A rows (`value_type = 'legacy'`) appear with `StatusCode::BadDataUnavailable` and NULL `Variant`, matching the migration-strategy contract.
**And** the partial-success behaviour (Story 8-3) is preserved — a bad row in the middle of the range does not abort the read.

### Story A.6: Web UI Live-Metrics Value Display

As an **operator browsing the web dashboard**,
I want `/api/metrics` to return JSON with real measurement values and `static/metrics.js` to render them with the configured `metric_unit`,
So that the live dashboard shows `23.5 °C` instead of `"Float"`.

**Acceptance Criteria:**

**Given** a Float metric `Moisture` with value `34.2` and `metric_unit = "%"`,
**When** the operator loads `/metrics.html`,
**Then** the row renders `34.2 %` with the staleness badge inherited from Story 9-3.
**And** the JSON payload from `/api/metrics` round-trips the typed value (e.g., `{"value": 34.2, "type": "Float", "unit": "%"}`).
**And** Story 9-3's per-row staleness badges continue to display correctly.
**And** the existing Story 9-4/9-5/9-6 CRUD pages do not break (regression suite passes).

### Story A.7: Migration Runbook + Version-Gated Migration Script

As an **operator upgrading an existing opcgw deployment from v2.0-rc to v2.0 GA**,
I want a documented migration path that either preserves my legacy database or cleanly drops it,
So that the upgrade does not require manual schema surgery.

**Acceptance Criteria:**

**Given** an existing v006 database from a v2.0-rc deployment,
**When** the operator follows `docs/deployment-guide.md § "Epic A migration"`,
**Then** the runbook documents: (a) automatic in-place schema bump preserving legacy rows as `value_type = 'legacy'` with NULL typed columns (default path), (b) explicit "drop the database file before upgrading" as the alternate path for operators who don't need pre-Epic-A history.
**And** the migration completes within 5 seconds for databases up to 100MB.
**And** OPC UA clients see `BadDataUnavailable` on legacy rows until the next poll cycle replaces them.
**And** the migration is one-way (no rollback path documented; rollback would mean restoring from a pre-upgrade backup file).

### Epic A — Story Acceptance Criteria

**Given** an opcgw instance running against a real ChirpStack with real metric values flowing in,
**When** an OPC UA client `Read`s any metric variable or `HistoryRead`s any range,
**Then** the returned `DataValue` carries the actual measurement payload (not the data-type discriminant string).
**And** this holds across poller restart and gateway upgrade from a v2.0-rc database.
**And** Story 8-4 (threshold-based alarm conditions, currently descoped) becomes a meaningfully-scopable story under a new Phase B name (not "8-4") once Epic A lands.

**Epic 9 retro (2026-05-14) AI6 reference:** see [`../implementation-artifacts/epic-9-retro-2026-05-14.md`](../implementation-artifacts/epic-9-retro-2026-05-14.md).

**Sprint change proposal:** [`sprint-change-proposal-2026-05-14.md`](sprint-change-proposal-2026-05-14.md).

---

## Epic 8: Web Configuration & Hot-Reload (Phase B)

Configure devices from any browser on the LAN, see live metric values for debugging, changes apply without gateway restart.
**FRs covered:** FR24, FR34, FR35, FR36, FR37, FR38, FR39, FR40, FR41, FR50, NFR11, NFR12.

**Numbering offset:** this file's "Epic 8" maps to `_bmad-output/implementation-artifacts/sprint-status.yaml`'s **"Epic 9: Web Configuration & Hot-Reload (Phase B)"**, because Phase A was renumbered after this file was authored. Stories created via `bmad-create-story` use the sprint-status numbering (`9-1`, `9-2`, …, `9-8`).

**Phase B carry-forward (Epic 8 retrospective, 2026-05-01 — see [`epic-8-retro-2026-05-01.md`](../implementation-artifacts/epic-8-retro-2026-05-01.md)):**

- **NodeId metric-name-only collision is a HIGH-severity pre-existing latent bug — must be fixed before Story 9.5 (Device CRUD) ships.** Surfaced via Story 8-3's HistoryRead path. Two devices with the same metric name (e.g., "Moisture") share a single NodeId across the entire OPC UA namespace; reads + subscriptions + HistoryRead all collapse to whichever device was registered last. Single-device deployments are unaffected, which is why this bug never surfaced before. **Epic 8 explicitly enables multi-device shapes via the Web UI (Story 9.5)**, so the bug will manifest the moment an operator adds a second device with overlapping metric names. Tracked at [GitHub issue #99](https://github.com/guycorbaz/opcgw/issues/99). Proposed fix: embed `device_id` in the NodeId string (e.g., `format!("{}/{}", device.device_id, metric_name)`); schema-migration NOT required, but address-space-observable to existing SCADA configurations. **Story 9.5 must depend on issue #99 being resolved first; the Story 9.5 spec must include a regression integration test that registers two devices with the same metric name and asserts both reads + HistoryRead return correct device-specific data.**
- **Access-control is enforced *before* node-manager overrides run.** Story 8-3 discovered that without `AccessLevel::HISTORY_READ` + `historizing = true` on the variable, async-opcua's session dispatch returns `BadUserAccessDenied` *before* the node manager's `history_read_raw_modified` override is reached. **Generalises to Epic 8: any future story that adds a node-manager-side override (e.g., `history_update`, `query` services, `write` services on operator-mutable nodes) must set the corresponding access-level bit on the variable at registration time, *not* implement the override and assume it will fire.** Affects Story 9.8 (dynamic OPC UA address-space mutation) directly: any new variable added at runtime must inherit the same `AccessLevel::CURRENT_READ | AccessLevel::HISTORY_READ` mask, or HistoryRead breaks for the new variable while continuing to work for variables registered at startup.
- **Spike-test productionisation is overdue.** Story 8-2's iter-1 review enumerated 7 distinct findings against the test infrastructure inherited from Story 8-1's spike (`tests/opcua_subscription_spike.rs`); all deferred. Tracked at [GitHub issue #101](https://github.com/guycorbaz/opcgw/issues/101). **Stories 9-1 / 9-2 / 9-3 will inherit the brittle infrastructure unless this lands first** — the new Axum-route integration tests will copy the helper shape and amplify the findings (`open_session_held` returning `None` for any failure, 300ms sleep insufficient on loaded CI for tracing flush, `HeldSession::Drop` aborting tokio tasks without await, etc.). **Recommended sequencing: land issue #101 fixes before `bmad-create-story 9-1`.**
- **`tests/common/mod.rs` extraction is BLOCKING before the 5th integration-test file appears.** Story 8-3 created the 4th integration-test file with significant `setup_test_server` / `pick_free_port` / `build_client` overlap; deferred because the four files diverge in subtle ways (`SubscriberMode` differences, `TestUserFixture` differences, `#[serial_test::serial]` differences). Tracked at [GitHub issue #102](https://github.com/guycorbaz/opcgw/issues/102). **Stories 9-1 / 9-2 / 9-3 will each likely add at least one integration-test file** — without extraction, the duplication grows from 4 callers to 7+ across one Epic. **Recommended sequencing: land issue #102 before `bmad-create-story 9-2`** (9-1 can use the existing harness; 9-2 onwards consumes the extracted module).
- **Doctest cleanup is BLOCKING before any Epic 8 implementation story starts.** 56 pre-existing doctest failures, fourth epic in a row. Tracked at [GitHub issue #100](https://github.com/guycorbaz/opcgw/issues/100). **Recommended sequencing: land issue #100 before `bmad-create-story 9-1`**, OR schedule as `epic-A-consolidation` between Epic 7 (sprint-status Epic 8) and Epic 8 (sprint-status Epic 9). Either approach. The cleanup adds `cargo test --doc` to the CI lane so future doctest regressions fail the build instead of silently joining the baseline.
- **Story 9.8 (dynamic OPC UA address-space mutation) gets its own spike — Story 9-0 spike.** Decision 2026-05-02 (Epic 8 retro action item #5):

  - **Library audit (2026-05-02):** async-opcua 0.17.1 exposes runtime mutation through `InMemoryNodeManager::address_space() -> &Arc<RwLock<AddressSpace>>` (`async-opcua-server-0.17.1/src/node_manager/memory/mod.rs:108`). The public `AddressSpace` API supports `add_folder` (`mod.rs:443`), `add_variables` (`:458`), `delete(node_id, delete_target_references)` (`:434`), `insert_reference` (`:230`), and `delete_reference` (`:249`). `set_attributes` (`mod.rs:122`) propagates value changes to subscribers via the `SubscriptionCache`. **Runtime mutation IS supported through public API; no fork is required.** The wrap pattern (analogous to `OpcgwHistoryNodeManager` from Story 8-3) remains the right shape for opcgw's use-case.

  - **Why a spike anyway:** the *contract* under live subscriptions is empirically unverified. Three load-bearing questions remain:
    1. **Add path:** when a new variable is added to the address space + an `add_read_callback` is registered for it on a running server, does a fresh subscription on the new node receive `DataChangeNotification`s correctly? Or does `SyncSampler` need a restart / re-init to pick up the new registration?
    2. **Remove path:** when a variable is deleted while a subscription's monitored item is targeting it, what status code does the client see — `BadNodeIdUnknown`, frozen-last-good, or notification stream silently drops? FR24 implies a clean status transition.
    3. **Sibling isolation:** do subscriptions on unaffected nodes continue uninterrupted while the address space is being mutated under the `RwLock`'s write guard? Long write-locks during bulk add/remove could pause sampling for all subscribers.

  - **Spike scope (Story 9-0):** ~120 LOC reference test (similar shape to `tests/opcua_subscription_spike.rs`), 3 integration tests pinning the three questions, ~1-2 days of work. Smaller than Story 8-1 (no separate binary, no `--load-probe` mode). Output: a short spike report (`9-0-spike-report.md`) that becomes the input for Story 9.7 (hot-reload) AC drafting and Story 9.8 (dynamic address-space) AC drafting.

  - **Why not inline in 9-8:** running this audit inline in 9-8 would entangle empirical discovery with implementation; if a Plan B emerges (e.g., add-only mutation safe but delete unsafe), 9-8 would need a mid-flight redesign. Front-loading the spike isolates the unknown into its own story-sized box, identical to how 8-1 isolated subscription-engine unknowns from 8-2's config-plumbing work.

  - **Sequencing:** Story 9-0 spike runs **after** Stories 9-1 (Axum web server) and 9-2 (status dashboard) since those don't touch the OPC UA address space and can proceed in parallel. The spike must complete **before** Story 9-7 (hot-reload) since hot-reload's "add a device → variable appears in address space" path is the same code path 9-8 owns. Recommended order: 9-1 → 9-2 → 9-3 → 9-0 spike → 9-7 → 9-8 → 9-4 / 9-5 / 9-6 (CRUD; depends on 9-7 hot-reload).
- **Subscription clients amplify the auth-and-cap surface.** Phase A carry-forward bullets at lines 678–684 of this file documented per-IP rate limiting (#88) and subscription/message-size limits (#89) as Phase B concerns. Epic 7 (sprint-status Epic 8) closed #89 by exposing the four mandatory `Limits` knobs. #88 (per-IP rate limiting) remains open and **becomes structurally relevant once the Web UI auth surface (Story 9.1) lands** — basic-auth probing from a single source IP can now exercise both the OPC UA gate and the Web UI gate at sustained rates. Recommended: file an Epic 8 sub-issue cross-referencing #88 if subscription-flood + Web-UI-auth-flood interaction surfaces a near-term operator concern.
- **Story 9.1 should reuse the HMAC keying pattern from Story 7-2 for basic-auth credential comparison.** `OpcgwAuthManager` (`src/opc_ua_auth.rs`) ships HMAC-SHA-256-keyed credential digests with `subtle::ConstantTimeEq`. Web-server basic-auth credential comparison should follow the same pattern (extract the keying primitive into a small `src/security_hmac.rs` shared module if needed, OR copy the contract with a clear "see opc_ua_auth.rs for the authoritative implementation" comment). **Do NOT roll a new credential comparison from scratch** — this was an Epic 7 retro lesson and Epic 8 should respect it.
- **The library-wrap-not-fork pattern is the default for any missing async-opcua callback.** Three Epics in a row (Stories 7-2, 7-3, 8-3) used composition + targeted override rather than forking async-opcua. Story 9.8 (dynamic address-space mutation) is the most likely candidate for a new wrap; if `SimpleNodeManagerImpl` doesn't expose runtime-mutation hooks on its public API, the wrap pattern (analogous to `OpcgwHistoryNodeManager`) is preferred over a fork. Document the chosen approach in the Story 9.8 spec.

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

---

## Epic B: v2.0 GA Release Packaging

**Numbering offset:** this epic uses the literal name `Epic B` in `sprint-status.yaml`, not a numeric index, to flag its release-packaging nature (it polishes deployment artifacts and operator documentation in preparation for the `v2.0` GA tag, rather than introducing new product capability). Stories created via `bmad-create-story` use sprint-status key `B-1`.

**Why it exists:** the [Epic A retrospective](../implementation-artifacts/epic-A-retro-2026-05-19.md) (2026-05-19) identified the final pre-GA gaps. The existing `.github/workflows/docker-build.yml` publishes only to GHCR (`ghcr.io/guycorbaz/opcgw`) and only for `linux/amd64`; v2.0 GA should publish to BOTH Docker Hub (`docker.io/gcorbaz/opcgw`) and GHCR with multi-arch manifests covering `linux/amd64` + `linux/arm64` so industrial Raspberry-Pi-class edge gateways are first-class deployment targets. The DocBook user manual at `docs/manual/opcgw-user-manual.xml` is four epics behind reality (per `deferred-work.md` standing entry; closes retro action item AI-A-8) and has no step-by-step installation or configuration chapters — operators currently have to read source code and configuration examples to figure out how to deploy. The Dockerfile final-stage container also runs as **root** (the non-root `opcgw` UID 10001 user block is commented out at lines 34-40) and pins its runtime base to `ubuntu:latest` (unpinned, non-reproducible). Epic B closes all of these in a single multi-deliverable story before the `v2.0` tag fires the dual-registry CI pipeline.

**FRs covered:** none directly (release-packaging epic).

**Sequencing:** immediate next epic after Epic A. Gates the `v2.0` GA tag. Zero functional-requirement impact — entirely deployment infrastructure + operator documentation + container hardening. Story 8-4 (sprint-status `8-4`, threshold-based alarms, functionally unblocked by Epic A) revival remains downstream under a new story name in a future Phase B+ epic.

### Story B.1: Docker Hub Publishing + DocBook User Manual Update

As an **opcgw operator deploying v2.0 to a production gateway**,
I want a published Docker image on Docker Hub (alongside the existing GHCR mirror) for both `linux/amd64` and `linux/arm64`, a non-root pinned-base container, and a current step-by-step installation + configuration user manual,
So that I can deploy opcgw without specialised git/cargo knowledge, without arch-specific build steps, and without consulting source code.

**Acceptance Criteria:**

**Given** a `v*` tag is pushed to the GitHub repository,
**When** the `.github/workflows/docker-build.yml` workflow fires,
**Then** the resulting Docker image is published to BOTH `docker.io/gcorbaz/opcgw` AND `ghcr.io/guycorbaz/opcgw` with identical manifests, sourced from the same single workflow run.
**And** each registry receives a multi-arch manifest covering `linux/amd64` + `linux/arm64`.
**And** Docker Hub login uses the `DOCKERHUB_USERNAME` (value `gcorbaz`) + `DOCKERHUB_TOKEN` (Personal Access Token, Read/Write/Delete scope) GitHub repository secrets via `docker/login-action@v3`.
**And** Docker Hub long-description (the "Overview" page rendered at <https://hub.docker.com/r/gcorbaz/opcgw>) is sourced from `docs/dockerhub-description.md` and either (a) auto-synced via `peter-evans/dockerhub-description@v4` step in the same workflow (preferred — keeps the page version-controlled), or (b) manually copy-pasted per major release (acceptable if the action's auth model doesn't fit). The chosen approach is documented in the Story Dev Notes.
**And** the `Dockerfile` final-stage container runs as non-root user `opcgw` (UID 10001) — the existing commented-out `useradd` + `USER opcgw` block at lines 34-40 is enabled; `WORKDIR` and bind-mount target permissions remain compatible.
**And** the `Dockerfile` runtime base is pinned from `ubuntu:latest` to `ubuntu:24.04` (or a documented alternative LTS tag).
**And** the repository `README.md` documents both `docker pull` paths (Docker Hub as the primary publish target, GHCR as the ecosystem mirror) with concrete `docker run` examples; the Planning table gains an Epic B row reflecting Story B-1 status; the Current Version block is updated in the same commit transitions per the CLAUDE.md "Documentation Sync" rule.
**And** the DocBook user manual at `docs/manual/opcgw-user-manual.xml` gains a complete step-by-step **Installation** chapter covering (a) Docker pull from Docker Hub, (b) Docker pull from GHCR mirror, (c) Docker Compose with the bundled `docker-compose.yml` + `.env` setup, (d) systemd service deployment from a built binary, (e) build-from-source via `cargo install` and `cargo build --release` (Rust 1.94+, `protoc`, `libssl-dev` prerequisites).
**And** the manual gains a complete step-by-step **Configuration** chapter covering (a) `config/config.toml` location and the `OPCGW_<SECTION>__<FIELD>` env-var override pattern; (b) `[chirpstack]` section field-by-field (server_address, api_token via env-var, tenant_id, polling_frequency, retry, delay); (c) `[opcua]` section (endpoint URL, security modes + policies matrix, user authentication, PKI directory layout, `create_sample_keypair`, `max_connections`); (d) `[web]` section (enable flag, port, basic auth credentials via env-var, hot-reload semantics); (e) `[[application]]` arrays with `[[application.device]]` and `[[application.metric]]` sub-structures (metric_type variants, `metric_unit` field, device-to-OPC-UA NodeId mapping); (f) logging configuration (per-module log levels, audit-event taxonomy referencing `docs/logging.md`).
**And** the manual gains a **Troubleshooting** appendix covering operator scenarios with structured-log-event grep recipes (ChirpStack auth failure → `event="chirpstack_auth_failed"`; OPC UA connection refused → check `opcua_session_count_at_limit` + `opcua_auth_failed`; polled metrics not appearing → check `metric_parse` warns; web UI 404 → check `web_request_rejected`; certificate errors → check `opcua_session_pki_violation`).
**And** the manual gains an **Upgrade and migration** chapter referencing `docs/deployment-guide.md § "Epic A migration"` for the v2.0-rc → v2.0 GA path (Path A in-place + Path B drop-and-recreate); also documents the fresh-install v2.0 path.
**And** the existing DocBook 4.5 syntax + DTD reference (`-//OASIS//DTD DocBook XML V4.5//EN`) is preserved; the manual is NOT migrated to LaTeX / Markdown / AsciiDoc / DocBook 5.
**And** `docs/manual/index.xml` is updated to reflect the new chapters in the manual's table of contents; the manual passes DocBook 4.5 DTD validation (`xmllint --noout --valid docs/manual/opcgw-user-manual.xml`).
**And** a local Docker smoke test passes: `docker build -t opcgw:smoke .` succeeds against the hardened Dockerfile; `docker run --rm` with a minimal config.toml + valid env-vars starts the container; `docker exec ... id` confirms the gateway process runs as UID 10001 (not root); the container binds port 4855; the gateway logs at least one `chirpstack_poll_start` event within 30 s.
**And** `CHANGELOG.md` is updated under `[Unreleased] — v2.0.0` (Added section) to document dual-registry publishing, multi-arch images, Dockerfile hardening, and the user-manual installation/configuration chapters.
**And** `cargo test --all-targets` continues to pass **1256 / 0 / 10** unchanged (no Rust code changes are expected in this story — Rust production code is OUT of scope).
**And** `cargo clippy --all-targets -- -D warnings` remains clean; `cargo test --doc` remains 0 failed / 55 ignored.
**And** strict-zero file invariants: NO changes to any `src/**/*.rs` file, any `tests/**/*.rs` file, `Cargo.toml`, `Cargo.lock`, `migrations/*.sql`, or any file under `config/`. Mutable scope is: `.github/workflows/docker-build.yml`, `Dockerfile`, `README.md`, `CHANGELOG.md`, NEW `docs/dockerhub-description.md`, `docs/manual/opcgw-user-manual.xml`, `docs/manual/index.xml`, and BMad bookkeeping files (`_bmad-output/planning-artifacts/epics.md`, `_bmad-output/implementation-artifacts/sprint-status.yaml`, the B-1 story spec file itself).
**And** the GitHub tracking issue is referenced in the implementation commit via `Refs #N` (user opens the issue out-of-band; story Dev Notes captures the issue number once provided).

### Epic B — Story Acceptance Criteria

**Given** the `v2.0` GA tag fires the CI pipeline,
**When** an operator runs `docker pull docker.io/gcorbaz/opcgw:2.0` (or the GHCR equivalent `docker pull ghcr.io/guycorbaz/opcgw:2.0`),
**Then** the operator receives a non-root, base-pinned image matching their host architecture (linux/amd64 or linux/arm64),
**And** the operator can follow `docs/manual/opcgw-user-manual.xml` (built to HTML or PDF via the standard DocBook XSL toolchain) end-to-end to install, configure, run, and troubleshoot opcgw without consulting source code or asking the maintainer.

**Epic A retro (2026-05-19) reference:** see [`../implementation-artifacts/epic-A-retro-2026-05-19.md`](../implementation-artifacts/epic-A-retro-2026-05-19.md) action item AI-A-8 (manual XML sync) — Epic B closes it.

## Epic C: Auto-Discovery and Web-First Configuration

**Numbering offset:** literal name `Epic C` in `sprint-status.yaml`, following the lettered post-numeric-epic convention established by Epic A and Epic B. Stories use sprint-status keys `C-0`, `C-1`, `C-2`, `C-3`, `C-4`, `C-6`. **C-5 is intentionally absent** — MQTT real-time path was scoped here originally (as D-5 during the 2026-05-20 vision capture, under the working name "Epic D") but removed from the epic on 2026-05-21 to keep the epic finishable; tracked as `CR-EPIC-C-MQTT` in `_bmad-output/implementation-artifacts/deferred-work.md`. The internal-document working name "Epic D" used during 2026-05-20/21 scope capture has been renamed to Epic C as of 2026-05-21 to avoid letter-skipping confusion.

**Why it exists:** Today operators configure opcgw by hand-editing `config/config.toml`, typing ChirpStack application UUIDs, device DevEUIs, and codec metric-key names from a side-by-side ChirpStack web UI tab. The v2.0 GA walkthrough on 2026-05-20 surfaced this UX gap directly — the operator must context-switch between two UIs and copy strings by sight. Epic C turns opcgw into a self-driving configuration surface: the operator opens the web UI, picks an application *by name* from a list opcgw fetched from ChirpStack, picks a device *by name* under that application, picks metrics *by observed key* from recent uplinks, and optionally overrides display names for the OPC UA browse tree. The load-bearing design call (memorialised in memory `project_epic_c_auto_discovery_vision.md`): **opcgw is a name-translation gateway** — operator-facing pickers always show names, never UUIDs. UUIDs may appear in detail views or audit logs but never as the primary picker key. This is why MQTT-direct integration was explicitly rejected as the primary architecture: MQTT events carry only UUIDs, which are useless to SCADA operators. Epic C also lays the foundation for the long-term end-state architecture Guy articulated 2026-05-20: *"In the final version, all configuration should be in database."* Story C-6 lands that migration.

**FRs covered:** none from the original PRD (Epic C is a post-PRD addition driven by the 2026-05-20 walkthrough; design decisions are captured in memory rather than retro-fitted into `prd.md`). Implicit functional contract per story: C-0 = empty-config startup; C-1 = inventory query API (server-side helpers + endpoints); C-2 = pickers UI; C-3 = duplicate-prevention server-side validator; C-4 = drift-view UI; C-6 = TOML→SQLite config migration.

**Sequencing:** C-0 is the prerequisite for all subsequent stories — C-1's `/api/inventory/*` endpoints are only operator-visible if the gateway can start with no `[[application]]` entries (today's validator rejects `application_list.len() == 0`). C-0 is **partially landed** (commit `cecd100` 2026-05-20 added `#[serde(default)]` to `application_list` so empty TOML deserialises; remaining work is the first-run web wizard for `[opcua].user_password` and dashboard empty-state). C-1 precedes C-2 (C-2 is a thin wrapper over C-1's endpoints). C-3 hardens C-2's writes server-side. C-4 is a read-only diff view consuming C-1's inventory endpoints; can land before or after C-3. C-6 is **last in the epic** — the TOML→SQLite migration depends on C-2's pickers being the operator-facing canonical write surface, on C-3's server-side validation being storage-independent, and on C-0's empty-bootstrap proving the gateway no longer treats TOML as a hard requirement.

### Story C.0: Empty-Config Bootstrap + First-Run Setup Wizard

As an **opcgw operator deploying a fresh container with no pre-existing `config.toml`**,
I want the gateway to start with an empty configuration and present a first-run web wizard for the OPC UA user password,
So that I can configure opcgw entirely through the web UI without writing TOML by hand and without learning the env-var override conventions before I can even reach the dashboard.

**Acceptance Criteria:**

**Given** a fresh container starts with a `config.toml` containing only `[global]`, `[chirpstack]`, `[opcua]`, `[web]` baseline sections and no `[[application]]` entries at all,
**When** `cargo run` (or `docker run`) bootstraps the gateway,
**Then** startup succeeds: `AppConfig::validate()` accepts `application_list.len() == 0` as a valid state, the ChirpStack poller spawns and no-ops while there are no applications to poll, the OPC UA server binds on the configured endpoint with an empty browse tree (no `Applications` folder yet, or a placeholder empty folder — Dev Agent picks the simpler option in Dev Notes), and the web server binds on the configured port and serves `/`.
**And given** the same fresh container also has `[opcua].user_password` unset (neither in TOML, nor via the `OPCGW_OPCUA__USER_PASSWORD` env var),
**When** the operator opens `http://<host>:<web_port>/` in a browser,
**Then** instead of the dashboard, opcgw renders a **first-run setup wizard** page that (a) explains the gateway has no OPC UA password configured, (b) prompts the operator for a password, (c) confirms the password via a second input, (d) writes the password to a persistent location compatible with the existing OPC UA auth pipeline (Dev Agent's choice: persist into the env-var equivalent secrets file, or into a new `config/secrets.toml` that the gateway loads alongside `config.toml` — documented in Dev Notes), (e) reloads or restarts the OPC UA server so the new password takes effect, (f) on submit success redirects to `/` which now renders the regular dashboard.
**And** the wizard validates the password against the same rules used by today's `OPCGW_OPCUA__USER_PASSWORD` env-var path (minimum length, allowed character set — copy from `src/opc_ua_auth.rs` rather than diverging).
**And** the dashboard, when first reached after the wizard, shows an empty state for the "Applications" tile (e.g., "No applications configured — add one from the Applications page") rather than crashing or rendering a blank table.
**And** the Applications page (today's `static/applications.html`) renders an empty list with a prominent "Add application" button (which today's static HTML already exposes, but ensure the empty-state copy is operator-meaningful, not a blank `<tbody>`).
**And** the existing env-var override path is preserved unchanged: if `OPCGW_OPCUA__USER_PASSWORD` is set at startup, the wizard is suppressed and the dashboard renders directly.
**And** `cargo test --all-targets` continues to pass with no regressions; new integration tests cover (a) `validate()` accepts empty `application_list`, (b) wizard renders on first reach when no password is set, (c) wizard's POST endpoint persists the password and unblocks the dashboard, (d) env-var path bypasses the wizard.
**And** `cargo clippy --all-targets -- -D warnings` remains clean.
**And** `README.md` documents the empty-bootstrap path in the Docker section (operator can `docker run` with no `config.toml` mount and configure entirely through the web UI).
**And** the partially-landed serde-default fix at `src/config.rs` (commit `cecd100`, 2026-05-20) is preserved and extended — C-0 does not regress empty-TOML deserialisation.

### Story C.1: ChirpStack Inventory Query Layer

As **opcgw's web UI**,
I want internal Rust helpers and HTTP endpoints that proxy ChirpStack's `ListApplications`, `ListDevices`, and recent-uplinks gRPC calls,
So that the inventory picker UI (Story C-2) can render lists of named ChirpStack resources without the browser having to talk to ChirpStack directly.

**Acceptance Criteria:**

**Given** the gateway is running with a valid `[chirpstack]` configuration (server_address, api_token, tenant_id),
**When** the web UI calls `GET /api/inventory/applications`,
**Then** opcgw queries ChirpStack's gRPC `ApplicationService.List` for the configured tenant, returns a JSON array of `{id, name}` objects sorted by name (case-insensitive), and the response includes a `count` field for paging signalling.
**And given** the operator has chosen an application in the UI,
**When** the web UI calls `GET /api/inventory/devices?application_id=<uuid>`,
**Then** opcgw queries ChirpStack's `DeviceService.List` for that application, returns a JSON array of `{dev_eui, name, description, profile_name, last_seen_at}` objects sorted by name.
**And given** the operator has chosen a device in the UI,
**When** the web UI calls `GET /api/inventory/uplinks?dev_eui=<eui>&limit=<N>` (default N=10, max N=50),
**Then** opcgw queries ChirpStack's event-log / device-events endpoint for the last N uplinks, returns a JSON array of `{received_at, decoded_object}` where `decoded_object` is the codec output JSON, plus a derived `observed_keys` field listing the union of all top-level keys seen across the N uplinks with a per-key value-type hint (`int` / `float` / `bool` / `string`) inferred from observed values.
**And** all three endpoints are read-only (no config mutation) and require the same web-UI authentication as the rest of the API surface (basic auth + CSRF per current `src/web/` patterns).
**And** errors are mapped sensibly: ChirpStack unreachable → 502 + `event="inventory_query_failed"` with `reason="chirpstack_unreachable"`; ChirpStack authentication failure → 502 + `reason="chirpstack_auth_failed"`; ChirpStack returns empty list → 200 with `[]` (not an error).
**And** **server-side TTL cache** mitigates ChirpStack call volume: `/api/inventory/applications` and `/api/inventory/devices` results are cached in-memory keyed by `(tenant_id)` and `(tenant_id, application_id)` respectively, with a default TTL of **60 seconds** (configurable via a new `[chirpstack].inventory_cache_ttl_seconds` field, default 60, settable to 0 to disable). Cache hits do NOT call ChirpStack. `/api/inventory/uplinks` is **NOT cached** (freshness-sensitive — operator needs the latest uplinks to discover newly-emitted metric keys).
**And** the operator can bypass the cache with `?refresh=true` on any inventory endpoint — forces a fresh ChirpStack call, repopulates the cache with the new result and a refreshed TTL stamp. The drift-view page (Story C-4) uses `?refresh=true` so the drift comparison is never against stale cache. Static pickers (Story C-2) honor cache by default.
**And** the cache is invalidated on POST/PUT/DELETE to the corresponding CRUD endpoints (`/api/applications` etc.) — when opcgw's own write changes the tenant's inventory shape (or when the operator runs the drift-view "Add to opcgw" flow), the next picker open serves fresh data even if TTL hasn't expired.
**And** **audit events fire on cache MISSES only**, not on cache HITS: `event="inventory_query"` carries fields `resource={applications|devices|uplinks}`, `cache_status={miss|refresh|bypassed}`, and the response status. Cache HITS are silent in the audit log (no event). Each cache MISS represents one actual ChirpStack call, so the audit-log volume is bounded by `1 / inventory_cache_ttl_seconds × active_operator_sessions` rather than `clicks_per_session`.
**And** the helpers (`list_chirpstack_applications`, `list_chirpstack_devices`, `list_chirpstack_recent_uplinks`) live in a new `src/chirpstack_inventory.rs` (or are added to existing `src/chirpstack.rs` — Dev Agent's choice based on file-size) and are exercised by unit tests with a mocked gRPC client.
**And** the endpoint handlers in `src/web/api.rs` (or a new `src/web/inventory.rs`) are exercised by integration tests using the existing `tests/web_*.rs` test harness pattern.
**And** `cargo test --all-targets` passes; `cargo clippy --all-targets -- -D warnings` clean.
**And** `docs/api.md` (if it exists) or `docs/web-api.md` is updated to document the three new endpoints; otherwise add a new section to `README.md` or a new file under `docs/`.

### Story C.2: Inventory Pickers in the Web UI

As an **opcgw operator adding a new application or device through the web UI**,
I want to pick the ChirpStack application, device, and metrics from named lists fetched from ChirpStack at the moment I'm filling in the form,
So that I never have to type a UUID or DevEUI by hand and never have to switch to the ChirpStack web UI to look up an ID.

**Acceptance Criteria:**

**Given** Story C-1's inventory endpoints are live,
**When** the operator clicks "Add application" on the Applications page,
**Then** instead of (or in addition to) the existing free-form `application_id` text input, opcgw fetches `/api/inventory/applications`, renders a name-sorted dropdown (or searchable picker), and on selection pre-fills the form's hidden `application_id` field plus the editable `application_name` field (defaulting to ChirpStack's name; operator may override for the OPC UA-side display).
**And given** the operator has selected an application and clicks "Add device",
**When** the device-add form loads,
**Then** opcgw fetches `/api/inventory/devices?application_id=<chosen-app>` and presents the same name-picker pattern; on selection pre-fills `device_id` (DevEUI, hidden) and `device_name` (editable, defaults to ChirpStack's name).
**And given** the operator has selected a device and clicks "Add metric",
**When** the metric-add form loads,
**Then** opcgw fetches `/api/inventory/uplinks?dev_eui=<chosen-eui>&limit=10` and renders a multi-select list of the observed top-level keys, each row showing the key name and the inferred wire type with a per-key wire-type override dropdown (Float / Int / Bool / String) defaulting to the observation-driven inference. The operator picks one or more keys; on submit, opcgw creates one metric per picked key with `chirpstack_metric_name = <key>` and `metric_name = <key>` (operator may override the OPC UA display name in the form before submit).
**And** the existing "type a UUID by hand" escape hatch remains available — the dropdown is preferred, but if ChirpStack is unreachable or the operator wants to pre-create config for a device not yet enrolled, they can switch to a "Type manually" mode and enter the IDs as today. The UI surfaces this fallback prominently when an inventory fetch returns 502.
**And** the pickers honor opcgw's existing CSRF discipline; no GET-then-immediate-POST race where a stale picker selection submits with a mismatched CSRF token.
**And** the wire-type inference at metric-pick time logs both the inferred type and the per-uplink sample values into the gateway's audit trail (`event="metric_wire_type_inferred"`) so the choice is auditable.
**And** integration tests (extending `tests/web_application_crud.rs`, `tests/web_device_crud.rs`, or new `tests/web_inventory_pickers.rs`) cover the happy path for each picker and the fallback-to-manual path on inventory-fetch failure.
**And** `cargo test --all-targets` passes; `cargo clippy --all-targets -- -D warnings` clean.
**And** the existing static HTML (`static/applications.html`, `static/devices-config.html`) is updated; if the inline-script complexity grows past ~200 lines, the Dev Agent extracts into a new `static/inventory-picker.js` module.

### Story C.3: Server-Side Duplicate-Prevention Validator

As an **opcgw operator who might accidentally try to add the same ChirpStack application or device twice through the web UI**,
I want opcgw to reject duplicate IDs at the same level with a clear error message,
So that I cannot create silent ambiguity in the OPC UA browse tree (two nodes claiming to map to the same ChirpStack resource) and so the picker doesn't accidentally let me add a duplicate by hand-typing.

**Acceptance Criteria:**

**Given** opcgw already has an `[[application]]` entry with `application_id = "abc-123"`,
**When** the operator POSTs another `[[application]]` with the same `application_id = "abc-123"`,
**Then** the server rejects the request with HTTP 409 (or 400 — Dev Agent picks the better-fitting code), emits `event="application_crud_rejected" reason="conflict" conflict_kind="duplicate"` (audit-shape course-correction from scope-time `reason="duplicate"` to preserve the existing `reason="conflict"` grep contract — see C-3 spec Dev Notes for full rationale), and the UI renders the error inline near the conflicting field (not as a generic toast).
**And given** the operator is adding a device under application `abc-123` and `device_id = "DEADBEEF00000001"` already exists under that same application,
**When** the POST fires,
**Then** the server rejects with the same 409/400 + `event="device_crud_rejected" reason="conflict" conflict_kind="duplicate"` audit pattern, scoped to "same DevEUI under same application."
**And given** the operator is adding a metric under device `DEADBEEF00000001` and `chirpstack_metric_name = "temperature"` already exists on that device,
**When** the POST fires,
**Then** the server rejects with the same pattern at the metric level.
**And** the rule is **scoped to the same level only**: it is EXPLICITLY ALLOWED for the same DevEUI to appear under two different applications in opcgw's config (an operator might want to expose one physical device under multiple OPC UA namespaces). Test: add `DEADBEEF00000001` under `application_a`, then add `DEADBEEF00000001` under `application_b` — both succeed; both appear in the OPC UA browse tree under their respective parent applications.
**And** the validator runs on POST and PUT (not just POST) — editing an application to set its `application_id` to a value already used by another application is also rejected.
**And** the rule is enforced regardless of which write path triggers it: the picker-driven flow from Story C-2, the manual free-form input fallback, AND a direct TOML hot-reload that introduces a duplicate (the hot-reload path emits the same audit event and refuses the load with the snapshot reverting to the pre-load state).
**And** existing duplicate-tolerance behavior anywhere else in the codebase is audited and either documented as intentional ("duplicate metric_name across two different devices is FINE — that's how operators expose multiple sensors of the same kind under different devices") or fixed.
**And** integration tests cover: (a) same `application_id` rejected, (b) same `device_id` under same application rejected, (c) same `device_id` under different application ACCEPTED, (d) same `chirpstack_metric_name` on same device rejected, (e) PUT-rename collision rejected, (f) hot-reload-introduced duplicate rejected.
**And** `cargo test --all-targets` passes; `cargo clippy --all-targets -- -D warnings` clean.

### Story C.4: Inventory Drift View

As an **opcgw operator running a gateway whose ChirpStack inventory has drifted since I last edited the opcgw config**,
I want a "drift view" page that compares my opcgw config against ChirpStack's current inventory and highlights stale, missing, available, and renamed resources,
So that I can reconcile divergence without manually cross-referencing two web UIs row by row.

**Acceptance Criteria:**

**Given** opcgw's config contains applications, devices, and metrics, and ChirpStack's actual inventory has diverged (some resources deleted, some new ones added, some renamed),
**When** the operator opens `/inventory-drift` (or whatever URL the Dev Agent chooses) in the web UI,
**Then** opcgw queries ChirpStack via Story C-1's endpoints, diffs against its in-memory config snapshot, and renders a table with rows in four classes:
  - **OK** — configured in opcgw, present in ChirpStack, names match → row is normal-styled.
  - **Stale** — configured in opcgw, MISSING from ChirpStack (deleted there) → row is yellow/warn-styled with a "Remove from opcgw" button.
  - **Available** — present in ChirpStack, NOT configured in opcgw → row is blue/info-styled with an "Add to opcgw" button that opens the existing add-flow pre-filled.
  - **Drifted** — configured in opcgw, present in ChirpStack, but the ChirpStack name differs from opcgw's `application_name` / `device_name` → row shows both names and offers "Update opcgw to ChirpStack's current name" + "Keep opcgw alias as-is" buttons.
**And** drift detection runs at three levels: application, device, metric (metric drift = a configured metric's `chirpstack_metric_name` no longer appears in the device's recent uplinks).
**And** clicking "Remove from opcgw" prompts a confirmation dialog before deleting, and the delete flows through the existing CRUD audit events (`event="application_crud" action="delete"` etc.) — drift-view is a thin UI, not a parallel write path.
**And** clicking "Add to opcgw" routes the operator to the standard add-flow (Story C-2 pickers) with the chosen resource pre-selected.
**And** clicking "Update opcgw to ChirpStack's current name" issues a standard PUT through the existing CRUD path.
**And** the drift-view page surfaces a "last refreshed at <timestamp>" indicator and a manual refresh button; no background polling (operator-triggered only, to keep ChirpStack API call volume predictable).
**And** the page is read-only safe: if ChirpStack is unreachable, the page renders with the cached OPC UA-side names plus an "Unable to reach ChirpStack — drift cannot be computed" banner; no destructive actions are offered.
**And** integration tests cover the four drift classes (OK / Stale / Available / Drifted) with a mock ChirpStack inventory and assert the JSON returned by a new `GET /api/inventory/drift` endpoint.
**And** the existing static-HTML pages get a new "Drift" navigation link or sub-tab; the implementation may be a new `static/inventory-drift.html` or a tab on an existing page (Dev Agent's choice).
**And** `cargo test --all-targets` passes; `cargo clippy --all-targets -- -D warnings` clean.

### Story C.6: TOML→SQLite Configuration Migration

As an **opcgw operator running v2.0+ with an established config**,
I want opcgw's authoritative configuration to live in SQLite alongside the metric values, with TOML reduced to a bootstrap-only seed file,
So that all writes (web UI, future automation APIs, eventual ChirpStack-driven auto-sync) hit a single canonical store, and so the gateway's "what's configured" answer comes from one place not two.

**Acceptance Criteria:**

**Given** opcgw starts with both a `config.toml` and an existing SQLite database from a prior version,
**When** the gateway boots,
**Then** opcgw migrates the TOML-side `[[application]]` / device / metric / command tree into a new set of SQLite tables (schema migration `v008` — increment from Epic A's `v007`), one-shot on first boot of the v2.0.x version that includes this story, and emits `event="config_migration" stage="toml_to_sqlite"` with row counts.
**And** post-migration, all CRUD endpoints (Story 9-4 / 9-5 / 9-6 application/device/metric/command CRUD, Story C-2 picker writes, Story C-3 validator) write to SQLite as the authoritative store; the TOML file is no longer mutated by opcgw at runtime.
**And** the existing `[chirpstack]`, `[opcua]`, `[web]`, `[global]` config-file sections (singleton config not tied to applications/devices/metrics) remain in TOML for v2.x and are migrated to SQLite in a future story — Story C-6 scope is the `[[application]]` tree only.
**And** the OPC UA address-space builder, ChirpStack poller, and web UI all read from SQLite (not from the in-memory `AppConfig.application_list`) post-migration; the in-memory snapshot is rebuilt from SQLite at boot and on every CRUD write.
**And** **hot-reload after C-6 is SQLite-driven, not TOML-driven**: the existing TOML file-watcher hot-reload primitive (Story 9-7, `src/web/hot_reload.rs` or wherever the watcher lives today) is **removed**. Hot-reload now means: when a SQLite write to the configuration tables completes (via any of the CRUD endpoints or the migration itself), opcgw rebuilds the in-memory `application_list` snapshot from SQLite, then triggers the same downstream rebuild path Story 9-7 wired up (OPC UA address-space rebuild, ChirpStack poller config refresh, dashboard cache invalidation). The semantics from an operator perspective are unchanged ("config change → live system catches up without restart"); the trigger source is the database write, not a file-system event. A new optional admin endpoint `POST /api/admin/reimport-toml` may be added in a future story to support emergency "restore config from a backup TOML" flows, but Story C-6 does NOT ship that endpoint — pre-C-6 operators who relied on TOML hot-reload as a config-edit primitive will use the web UI CRUD instead post-C-6.
**And** a migration runbook lives at `docs/c-6-migration-runbook.md` covering: (a) pre-migration backup of `opcgw.db` + `config.toml`, (b) automatic migration on first v2.x boot with this story shipped, (c) verification step (`scripts/check-c6-migration.sh` or similar — confirms row counts match between TOML and the new SQLite tables), (d) one-way rollback contract (if the operator wants to downgrade to a pre-C-6 v2.x, they must restore from the pre-migration `opcgw.db` backup).
**And** integration tests cover: (a) fresh-DB boot with a populated TOML — migration runs, SQLite tables populate, in-memory snapshot matches TOML byte-for-byte; (b) re-boot with already-migrated SQLite — migration no-ops; (c) CRUD writes go to SQLite, not TOML; (d) OPC UA browse tree reflects SQLite state post-CRUD.
**And** `cargo test --all-targets` passes; `cargo clippy --all-targets -- -D warnings` clean.
**And** `docs/architecture.md` is updated to reflect SQLite as the configuration source-of-truth post-C-6; `README.md` Planning table and Configuration section reflect the new state.

### Epic C — Story Acceptance Criteria

**Given** opcgw is freshly deployed with no `config.toml` (or a near-empty one),
**When** an operator opens the web UI for the first time,
**Then** they complete the first-run setup wizard (C-0), are routed to the empty dashboard (C-0), add applications/devices/metrics by name-picking from ChirpStack (C-1+C-2), are protected from creating duplicates (C-3), and reconcile drift between opcgw and ChirpStack via the drift-view page (C-4) — all without hand-editing TOML, all by-name not by-UUID.
**And** the resulting configuration is stored authoritatively in SQLite (C-6), with TOML reduced to optional bootstrap seed.

**Vision capture reference:** see memory `project_epic_c_auto_discovery_vision.md` (Guy 2026-05-20, scope finalised 2026-05-21) for the load-bearing name-translation rationale and the three design decisions resolved at scope time (duplicate scope = same-level only; metric inventory discovery = scrape recent uplinks; wire type default = observation-driven).

**Deferred CR:** `CR-EPIC-C-MQTT` in `../implementation-artifacts/deferred-work.md` — the originally-proposed Story C-5 (MQTT-based real-time path) was removed from Epic C on 2026-05-21 ("not willing to implement MQTT, at least now") to keep this epic finishable. Re-promote to a story in a future epic only on explicit operator request.

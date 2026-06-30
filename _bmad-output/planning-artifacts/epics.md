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

---

## Epic D: Singleton Configuration → SQLite

**Numbering offset:** literal name `Epic D` in `sprint-status.yaml`, continuing the lettered post-numeric-epic convention (A/B/C/D). Stories use sprint-status keys `D-0`, `D-1`, `D-2`. 3-story epic — deliberately narrower than Epic C to keep the scope tight and finishable.

**Why it exists:** Story C-6 (2026-05-26) moved the `[[application]]` tree (applications/devices/metrics/commands) from `config.toml` into SQLite as the canonical store. The remaining `config.toml` sections — `[global]`, `[chirpstack]`, `[opcua]`, `[web]` — are *singleton* config (one row per section, not a collection) and stayed in TOML for v2.x per the C-6 spec's explicit deferral. Epic D lands the natural follow-up: migrate the singleton sections to SQLite so `config.toml` becomes a pure bootstrap-seed file with no runtime mutation surface. Secrets (`[chirpstack].api_token`, `[opcua].user_password`) stay in `config/secrets.toml` per the Story C-0 pattern — chmod 0600 via atomic-rename keeps secrets out of the world-readable SQLite database (cf. AI-C-SEC-2 from the Epic C security review). The end state: opcgw has exactly **three** persistence surfaces — SQLite (configuration + metric values), `secrets.toml` (operator-supplied secrets, chmod 0600), and `config.toml` (bootstrap seed only, never mutated at runtime). Guy's articulated end-state from 2026-05-20 — *"In the final version, all configuration should be in database"* — is fully realised by Epic D combined with what C-6 already shipped.

**FRs covered:** none from the original PRD (Epic D is a post-PRD addition driven by the 2026-05-26 Epic C retrospective's "natural C-6 follow-up" recommendation). Implicit functional contract per story: D-0 = bulk one-shot migration of `[global]`/`[chirpstack]`/`[opcua]`/`[web]` non-secret fields into SQLite, mirroring C-6's pattern; D-1 = web UI page for editing singleton config with restart-required-knob awareness; D-2 = decommission TOML mutation surface, making `config.toml` bootstrap-seed-only.

**Sequencing:** D-0 is the prerequisite for D-1 + D-2. D-1 and D-2 can run in parallel after D-0 (D-1 is additive — new UI page + new POST handlers; D-2 is subtractive — delete remaining TOML write paths). D-2 should land **last** in the epic, symmetric with C-6's "must-land-last" constraint in Epic C, because it removes the TOML safety net that D-0's migration falls back to on row-count mismatch.

**Carry-forward from Epic C retro:** Epic D is the natural home for partial resolution of these v2.x action items: **AI-C-SEC-2** (SQLite database file permissions — D-0's migration adds singleton config to `data/opcgw.db`, making the file-permission tightening load-bearing). It is NOT the home for **AI-C-SEC-1** (`prune_old_metrics` SQL format-string — Epic A/storage territory), **AI-C-SEC-3** (`setup_get` filename log — Story C-0 territory, simple one-line fix), or the cumulative skill-codification debt (AI-A-1/2/3 + AI-B-1/2/3 + AI-C-1/2 — needs a dedicated skill-codification epic, not bolted onto a data-migration epic).

### Story D.0: Singleton Config → SQLite Migration

As an **opcgw operator running a post-D-0 binary**,
I want the gateway's `[global]`, `[chirpstack]`, `[opcua]`, and `[web]` non-secret singleton config to migrate from `config.toml` into SQLite on first boot,
So that all configuration writes converge on a single canonical store and `config.toml` no longer needs runtime mutation.

**Scope summary (full Acceptance Criteria to be drafted when `bmad-create-story D-0` is invoked):**

- Schema migration **v010** (incrementing from C-6's v009) adds a `singleton_config` table — design call between (a) generic key-value `(section TEXT, key TEXT, value TEXT)` with composite PK and (b) per-section typed tables (`global_config`, `chirpstack_config`, `opcua_config`, `web_config`) is **deferred to Dev Notes during D-0 implementation**. The C-6 precedent for `[[application]]` was per-entity typed tables; the singleton sections may benefit from a single typed-by-section table since each section has its own fixed schema.
- Boot-time one-shot migration mirrors C-6's pattern: detect empty SQLite singleton tables + non-empty TOML singleton sections → open `BEGIN EXCLUSIVE TRANSACTION`, insert all rows, verify count per section, commit + write `d0_migration_done` meta done-flag. Fall back to TOML-driven boot on row-count mismatch or insert failure (transition safety net).
- Secondary already-migrated guard with meta-key back-fill (C-6 iter-2/iter-3 doctrine carry-over).
- Post-migration: ALL reads of singleton config come from SQLite (rebuilt into the in-memory `AppConfig` snapshot at boot); TOML is read once at boot to seed the migration on the first post-D-0 startup and never again afterwards.
- **Secrets stay in `config/secrets.toml`** — `[chirpstack].api_token`, `[opcua].user_password` are explicitly out of D-0's scope. The figment provider stack (TOML + secrets.toml + env-var) keeps the secrets.toml layer; only the TOML layer changes from "authoritative for singletons" to "bootstrap seed only."
- Restart-required knobs (`[opcua].host_port`, `[opcua].host_ip_address`, `[web].port`, `[chirpstack].server_address`, `[web].allowed_origins` from issue #113, all PKI paths) are **read once at boot from SQLite** and continue to require a process restart to take effect. D-0 does NOT introduce live hot-reload for these knobs — that surface stays restart-required for v2.x.
- Hot-reloadable knobs (e.g. `[chirpstack].poll_frequency`, `[global]` log-level if applicable) continue to hot-reload via the existing `notify_crud_write` path Story C-6 wired up — they just read from SQLite now instead of from TOML watcher events.
- New `docs/d-0-migration-runbook.md` + `scripts/check-d0-migration.sh` operator tools (mirrors C-6 deliverables).
- DocBook user manual Configuration chapter updated to reflect the two-surface model: SQLite for runtime config, `config.toml` for bootstrap-seed only, `config/secrets.toml` for operator-supplied secrets.
- Integration tests cover: (a) fresh-DB boot with populated TOML — migration runs, SQLite singleton tables populate, in-memory snapshot matches TOML byte-for-byte; (b) re-boot with already-migrated SQLite — migration no-ops via primary guard; (c) secondary guard fires with apps present but no flag (mirrors C-6 I2-F4 test); (d) restart-required knob change via D-1 UI surfaces operator warning + supervisor restart.
- AC#24 doc-sync gate (Epic C retro carry-forward): `docs/logging.md` is updated to document the new `config_migration` stage values (`stage="singleton_toml_to_sqlite"`, `stage="singleton_already_migrated"`, `stage="singleton_already_migrated_backfill_failed"`) in the same commit as the code.

### Story D.1: Singleton Config Editor in the Web UI

As an **opcgw operator running a post-D-1 binary**,
I want to edit `[global]`, `[chirpstack]`, `[opcua]`, and `[web]` config knobs through the web UI,
So that I can adjust gateway behaviour without SSH-ing in to edit TOML files.

**Scope summary:**

- New `static/singleton-config.html` page covering all 4 sections (one collapsible section per `[X]` group). Mirrors C-2's CRUD UI pattern (basic Auth + CSRF gate).
- New `GET /api/config/singleton` endpoint returns the current snapshot (basic-auth required; CSRF-exempt because GET).
- New `PUT /api/config/singleton/<section>` endpoint replaces one section's values (basic-auth + CSRF-gated; validates the section's value shape via the existing `AppConfig::validate` per-section helpers).
- **Restart-required knob handling:** the UI explicitly labels which fields are restart-required (PKI paths, ports, allowed_origins) and presents a confirmation modal explaining "this change requires a gateway restart to take effect." Submit triggers a graceful `CancellationToken` restart via the same mechanism C-0's wizard uses, so the supervisor (Docker / systemd) restarts and the new SQLite values are read.
- Hot-reloadable knobs (e.g. `[chirpstack].poll_frequency`, log levels) take effect immediately via the existing `notify_crud_write` → in-memory snapshot rebuild path (no restart required; UI surfaces this distinction clearly).
- Secrets (`api_token`, `user_password`) are NOT editable from this UI — they remain operator-supplied via `secrets.toml` or env-var overrides. The UI shows masked placeholders + a note "secrets are managed via `config/secrets.toml`."
- New audit events: `singleton_config_updated` (info), `singleton_config_rejected` (warn with `reason="validation"|"csrf"|"reload_failed"`), `singleton_config_restart_required` (info).
- Nav strip extended on all 7 sites (`index/applications/devices-config/devices/metrics/commands/inventory-drift/singleton-config`).
- Integration tests cover: GET returns current snapshot; PUT validates and persists; restart-required PUT triggers shutdown_token cancellation; hot-reloadable PUT triggers notify_crud_write; auth + CSRF carry-forward intact.

### Story D.2: Decommission TOML Mutation Surface (must-land-last)

As an **opcgw operator running a post-D-2 binary**,
I want `config.toml` to be exclusively a bootstrap-seed file that opcgw never mutates at runtime,
So that the operator contract is unambiguous: edit SQLite via the web UI (or `secrets.toml` for secrets); `config.toml` is read once at first boot and is otherwise inert.

**Scope summary:**

- Delete any remaining figment write paths, TOML mutation code, or `toml_edit` usage (`toml_edit` was already removed in C-6 per spec; D-2 is the final sweep).
- `config.toml` becomes a **read-once-at-bootstrap** file: the figment provider stack still loads it on every boot for backward compatibility (operators with existing `config.toml` files don't need to migrate by hand), but once the SQLite singleton tables are populated, the TOML values are ignored at runtime in favour of the SQLite snapshot.
- Update `architecture.md` to reflect the final two-surface model (SQLite authoritative; `secrets.toml` for secrets; `config.toml` bootstrap-seed-only).
- DocBook user manual Configuration chapter is rewritten — NOT just patched — to describe the new operator-facing contract end-to-end. Operators who learned opcgw pre-D-2 need a clear migration narrative.
- Audit log: emit `event="config_toml_unused_warning"` at `warn` level on every boot where `config.toml` is present AND the SQLite singleton tables are already populated, with a one-time-per-boot guard. The warning includes a recommended-action field ("config.toml is no longer mutated; verify it matches SQLite or delete it to remove operator confusion").
- D-2 deliberately does NOT delete operators' existing `config.toml` files. Removal is left to operator action (documented in the runbook).
- Integration tests cover: (a) operator with pre-D-2 `config.toml` + populated SQLite boots cleanly with the warn event firing once; (b) operator deletes `config.toml` post-migration → boot succeeds (SQLite is authoritative); (c) operator edits `config.toml` post-D-2 has no effect on runtime (proves the TOML is inert).

### Epic D — Story Acceptance Criteria

**Given** opcgw is freshly deployed with the existing `config.toml` + `config/secrets.toml` + `data/opcgw.db` files from a pre-Epic-D version,
**When** the operator deploys the post-D-2 binary,
**Then** the gateway migrates singleton config from TOML to SQLite on first boot (D-0), provides a web UI for editing it (D-1), and explicitly disclaims any TOML mutation at runtime (D-2). The operator's `config.toml` file may be deleted after the migration is verified; the gateway operates from SQLite + `secrets.toml` alone.
**And** the AI-C-SEC-2 follow-up (SQLite file permissions chmod 0600) is incorporated into D-0's deliverables so the now-load-bearing `data/opcgw.db` is not world-readable.

**Vision capture reference:** Epic C retrospective recommendation, 2026-05-26 — "natural C-6 follow-up: migrate the singleton config sections to SQLite + add web UI for editing them." Memory `session_2026_05_26_epic_C_retro_done.md` carries the option-1 recommendation context.

**Deferred / out-of-scope:**

- Secrets-in-SQLite (encrypted-at-rest) — operator threat model has not shifted to justify the key-management surface; secrets stay in `secrets.toml`.
- Live hot-reload for restart-required knobs (PKI paths, ports, `allowed_origins`) — issue #113 territory; D-1 surfaces restart-required UX clearly but does NOT change the underlying restart-required semantics.
- `CR-EPIC-C-MQTT` (MQTT real-time path) — still deferred per Epic C scope decision; re-promote only on explicit operator request.

## Epic E: Model-Agnostic, Class-Aware Device-Abstraction Layer

**Tracking:** GitHub issue [#129](https://github.com/guycorbaz/opcgw/issues/129). Literal name `Epic E` in `sprint-status.yaml`, continuing the lettered post-numeric-epic convention (A/B/C/D → E). Stories use sprint-status keys `E-0`, `E-1`, `E-2`, `E-3`.

**Why it exists:** opcgw today exposes whatever metrics and (queued-but-never-sent) commands a device's config declares, with the device's protocol details leaking onto the OPC UA side — the OPC UA client must know each model's raw fPort and byte codes. Guy's 2026-06-06 requirement: opcgw should present heterogeneous devices with a **common OPC UA view** — every *model* of a given *device kind* looks identical on OPC UA. An "open/close" command for a motorized valve is the same regardless of valve vendor/model; valve status is a normalized state, not a raw byte. This extends opcgw's name-translation role (Epic C) into a full **semantic device-abstraction layer**. First concrete driver: 3 Tonhe E20 motorized valves (LoRaWAN 868, Class A, single-byte open/close); protocol + ChirpStack codec in `docs/LoRa/TONHE Valve/`. **Crucially NOT valve-specific** — the architecture generalizes to other device kinds (sensors, meters, other actuators); valves are simply the first kind used to prove the pattern.

**Two extension axes (the load-bearing design call):** (a) **Model** — byte layout, fPort, raw codes — translated by a per-(class,model) **adapter**. *Preferably* in the ChirpStack codec (**Tier 1**, when the codec is editable — a new model is then just a codec, zero opcgw change); otherwise **owned by opcgw** (**Tier 2** vendor-object remap / **Tier 3** native bytes) for the common case where a codec is installed but **cannot be edited** to opcgw's canonical shape. opcgw is model-*aware* via declarative profiles, not model-coupled in core. (b) **Class / device kind** — canonical command kinds + canonical status/measurement semantics — handled IN opcgw; a new kind defines its canonical OPC UA surface once. The class layer is **additive**: devices not bound to a class keep working exactly as today (arbitrary numeric metrics + raw writable command nodes).

**Locked design decisions (2026-06-06, via design dialogue):**

- Command surface (revised 2026-06-09): one or more **writable OPC UA Variables** per device — **NOT** OPC UA Methods (universal SCADA/PLC client compatibility + reuse of E-0's writable command node). Canonical command **kinds**: **On/Off** (binary `1`/`0`, value→lookup→payload — the primitive shared across valves/switches/relays/pumps/motors; `on`→`open` polarity is **per-model configurable**, default valve `1`=open) and **SetLevel** (analog, value→scale/encode→payload — proportional valves/dimmers/VFD). `raw` legacy preserved; pure sensors have no command. OPC UA **Methods reserved** for future momentary/parameterless actions (Reset/Trigger/Home) only.
- Status surface (revised 2026-06-09): a small **uniform core** — `Active` (on/off [+`Unknown`]), `Transitioning`, `Fault` — generalizing across binary actuators (valve "opening" = Active-target-on + Transitioning; "closed" = Active off, steady), **plus** class/model extras (e.g. `LowBattery`). Canonical state vocabularies per class (valve: open/opening/closed/closing/blocked/fault/unknown; switch: on/off). **Discipline:** keep the status ontology light until a **second** class (switch/motor) forces the shape.
- Per-model protocol (revised 2026-06-09 — **supersedes** the original "lives entirely in the ChirpStack codec"): lives in a per-(class,model) **adapter** with 3 tiers chosen **independently per direction** (uplink/downlink): **T1 codec-canonical** (editable codec — the Tonhe valve), **T2 object-remap** (vendor decoded object ↔ canonical via field rename [already via `chirpstack_metric_name`→`metric_name`] + value transforms: enum map, linear scale+offset, bitmask/shift), **T3 native-bytes** (opcgw decodes raw uplink `data` / encodes raw `bytes`+`fPort` — fallback when the codec object/input is unusable). **Adapter expressiveness:** hybrid — declarative profiles (config/SQLite) for simple models + a Rust `trait DeviceDriver { encode; decode }` escape hatch for complex ones (multi-byte, CRC, stateful).
- Downlink mechanism: the adapter enqueues **either** a semantic object (T1/T2) **or** raw `bytes`+`fPort` (T3) — E-0's `build_queue_item` already supports both forms.
- Uplink mechanism: **Route B** — consume ChirpStack's decoded uplink EVENTS via gRPC `InternalService.StreamDeviceEvents` (reusing inventory stream code), NOT the metrics poll, NOT MQTT. (Route A — codec emits numeric state-code via metrics poll — was rejected and **empirically disproven 2026-06-08**: `GetMetrics` aggregation produced impossible values (`valveStatusCode=391` sum, `valvePosition=1.5` avg). No measurement kind survives aggregation; see #130.)
- **No gateway-side aggregation (locked 2026-06-08, #130):** opcgw exposes the **raw last-known value** of every measurement + the device's **source timestamp** + **quality**; aggregation/trending is the SCADA's responsibility, never the gateway's. The metrics poll (`GetMetrics`) time-aggregates (Gauge=avg, Absolute=sum, Counter=delta) and is therefore unsuitable as the value path. This generalizes Route B from valve status to **all** measurements; see Story E.1.

**Feasibility baseline (verified in code 2026-06-06):** the downlink command path is built but UNWIRED — `process_command_queue` (`src/chirpstack.rs:2430`, called each poll at `:1317`) dequeues each command and silently discards it behind a "Story 4-1 Phase 3" TODO (`:2437-2442`); `enqueue_device_request_to_server` (`:2511`) is `#[allow(dead_code)]` with zero call sites and currently builds raw bytes (`data`) not an object (`:2523-2534`). The decoded uplink object cannot flow through the metrics poll (`GetDeviceMetrics`, `:2376`; string metrics unsupported at `:1733`) — it is only available via `StreamDeviceEvents`, already read for inventory at `src/chirpstack_inventory.rs:373` but not in the runtime poller.

**FRs covered:** none from the original PRD (Epic E is a post-PRD addition driven by the 2026-06-06 valve-piloting requirement; design captured in GH #129 + memory `project_device_abstraction_valves.md`). Implicit functional contract per story below.

**Sequencing:** E-0 is the first slice and is independently testable (it makes OPC UA command writes actually reach the device). E-1 (uplink ingestion) and E-2 (class registry) build on the patterns E-0 establishes; E-2 generalizes the canonical-mapping concept that E-0 and E-1 each introduce concretely for the valve. E-3 (delivery confirmation) depends on E-0's send path. Recommended order: **E-0 → E-1 → E-2 → E-3**, with E-2 starting only once E-0 + E-1 have proven the valve mapping concretely (avoid over-abstracting before one driver works end-to-end).

### Story E.0: Downlink Command Path (first testable slice)

As an **opcgw operator with a device command defined**,
I want an OPC UA write to a command node to actually be delivered to the device via ChirpStack,
So that I can pilot an actuator (e.g. open/close a valve) from the OPC UA / SCADA side.

**Scope summary (full Acceptance Criteria to be drafted when `bmad-create-story E-0` is invoked):**

- Finish the Story 4-1 Phase 3 wiring: `process_command_queue` dequeues a command and calls `enqueue_device_request_to_server` (remove the drop-and-skip TODO + the `#[allow(dead_code)]`). Resolve the `Command` ↔ `DeviceCommandInternal` type unification the TODO references.
- Switch the enqueue from raw-byte `data` to **semantic object** (`object: Some(Struct)`, empty `data`) so the device profile's `encodeDownlink` produces the wire bytes — keeping opcgw model-agnostic. Map the canonical OPC UA node value to the command object (valve: `1` → `{"command":"open"}`, `0` → `{"command":"close"}`).
- Preserve the existing raw-byte path as a fallback for devices/commands not bound to a class (additive, not a replacement).
- Command status transitions Pending → Sent on successful enqueue; enqueue failure is logged and reflected in status (full delivery confirmation is E-3).
- Integration tests: a queued command is enqueued to a mock/stub ChirpStack `DeviceService` with the expected object + fPort; the failure path; the canonical-value → object mapping; the raw-byte fallback still works.
- **Real-world validation gate** (per the main-deadlock incident doctrine, memory `incident_main_deadlock_2026_05_20`): OPEN/CLOSE one of the 3 Tonhe E20 valves from opcgw end-to-end before E-0 flips to done.

### Story E.1: Uplink-Event Ingestion — last-known value for all measurements (no aggregation)

As an **opcgw operator**,
I want the gateway to ingest ChirpStack's decoded uplink events and expose each device value as its last-known value with the device's source timestamp,
So that OPC UA reflects true device state (discrete and analog) without gateway-side aggregation — the SCADA does any averaging/trending.

**Scope summary (elevated per #130; full ACs drafted at `bmad-create-story E-1`):**

- New runtime task consuming `InternalService.StreamDeviceEvents` (reuse the `src/chirpstack_inventory.rs:373` `stream_recent_device_uplinks` patterns), running alongside the metrics path; reconnect/backoff on stream drop mirroring the Epic 4 auto-recovery resilience.
- For **every** decoded field, store the **last value** with the device's **source timestamp** (event time, not poll time) — this is the canonical value path for all measurements, not just valve status. Storage already supports `MetricType::String`; this is the first poller-side path that populates it.
- For class-bound devices, additionally map decoded-object fields → canonical status variables (valve: `ValveState` + `Moving`/`Fault`/`LowBattery`).
- **No aggregation on the value path.** The metrics poll (`GetMetrics`) is **demoted to a backfill** — initial value before the first stream event, and re-sync after a stream reconnect — or retired; opcgw never exposes an averaged/summed value on OPC UA.
- Migrate existing analog metric mappings (temperature, water level, flow) from GetMetrics measurement names to decoded-object field names; validate against the live ChirpStack (some values may have no uplink object and must keep the backfill).
- OPC UA quality (Good/Uncertain/Bad) driven by real device-report age (consistent with Story 5-2).
- gRPC, **NOT** MQTT — does not reopen `CR-EPIC-C-MQTT`.
- Config to enable/scope the stream (which applications/devices) to avoid unbounded event volume.
- May split into **E-1a** (stream mechanism + last-value store + valve class) and **E-1b** (migrate all mappings, demote poll) if oversized.
- Tests: stream event → last-value Storage write with source timestamp → OPC UA variant; reconnect after drop; quality reflects report age; no aggregation anywhere in the value path; backfill still serves a value before the first event.
- **Adapter note (2026-06-09, → E-1b):** the uplink mapping must later grow a **value-transform hook** (enum map / scale+offset / bitmask-shift) for **Tier-2** object-remap devices (codec installed but uneditable; see E-2). **E-1a (Tier-1 Tonhe valve) needs none and is unaffected** — it ships as-is.
- **Per-device stale threshold (2026-06-09, → E-1b; [#132](https://github.com/guycorbaz/opcgw/issues/132)):** now that quality ages from the device's real report timestamp, slow LoRaWAN sensors (~15–20 min cadence) read *Uncertain* between uplinks under the global 120 s threshold. Add an optional per-device `stale_threshold_secs` (override of the global default; absent = global; restart-required; schema migration v012). Companion to the de-aggregation so the read path is usable. Validated need on rc2 pre-prod 2026-06-09.
- **Release gate:** E-1 must land before tagging **v2.2.0** stable (#130).

### Story E.2: Device-Class + Per-Model Adapter Registry

As an **opcgw maintainer**,
I want a registry of device **classes** (canonical OPC UA surface) and per-(class,model) **adapters** (canonical↔ChirpStack translation),
So that every model of a class — even one whose ChirpStack codec I cannot edit — presents one identical On/Off/SetLevel + status surface to SCADA, and adding a model is a declarative addition.

**Scope summary (revised 2026-06-09; full ACs at `bmad-create-story E-2`):**

- **Class** = the canonical OPC UA surface: command **kinds** (On/Off binary, SetLevel analog; `raw` legacy) exposed as **writable Variables** (not Methods) + a normalized **status** vocabulary (uniform `Active`/`Transitioning`/`Fault` core + class extras). A device is optionally tagged with a `(class, model)`; default none = generic device, behaviour unchanged.
- **Adapter** per `(class, model)`, with 3 tiers chosen **independently per direction**: **T1 codec-canonical**, **T2 vendor-object remap** (field rename + value transforms: enum map / linear scale+offset / bitmask-shift), **T3 native bytes** (decode raw `data` / encode raw `bytes`+`fPort`). Hybrid expressiveness: **declarative profiles** for simple models + a Rust `trait DeviceDriver { encode; decode }` for complex ones.
- **Command bindings:** generalize E-0's `[[application.device.command]]` into `command_kind`-tagged bindings (onoff/setlevel/raw); the writable Variable's value drives the adapter's downlink (value→lookup for onoff, value→scale/encode for setlevel). `on→open` polarity per-model configurable.
- **Drivers to ship:** refactor the concrete valve mapping (E-0/E-1a) into the registry as the **first T1 driver**; add **one T2 object-remap profile** (a second valve/switch model with an uneditable codec) to prove the can't-edit-the-codec path; add a **stub second class** (e.g. switch On/Off) to prove class extensibility.
- Web/config UI to assign a device's `(class, model)` (extends device-config pages; C-2 picker pattern).
- Generic + T1 devices remain fully functional (additive). Tests: valve T1 round-trip; T2 remap round-trip (enum + scale + bitmask); SetLevel encode; generic device unaffected; a 2nd class validates extensibility.
- Docs: `docs/architecture.md` (adapter/registry model), config + DocBook manual (class/model + command_kind surface), `docs/logging.md` if new events.

### Story E.3: Command Delivery Confirmation

As an **opcgw operator**,
I want command delivery/confirmation status reflected back to me,
So that I know whether a command actually reached the device.

**Scope summary:**

- Implement the `CommandStatusPoller` stub (`src/chirpstack.rs:~2749`) to observe ChirpStack for delivery/ack of confirmed downlinks and update command status Sent → Confirmed/Failed.
- For Class-A confirmed uplinks (the Tonhe valve sends a conform packet), correlate the next status uplink with the queued command where feasible.
- Surface command status on OPC UA and in the audit log.
- Tests: confirmation updates status; timeout / failure path.

### Epic E — Story Acceptance Criteria

**Given** a device bound to the valve class with a command defined on the canonical command node,
**When** the operator writes `1` (open) or `0` (close) to that node via an OPC UA client,
**Then** opcgw enqueues the corresponding semantic command object to ChirpStack (E-0), the device profile's codec encodes the model-specific bytes, the valve actuates, and the resulting decoded status uplink flows back via the gRPC event stream (E-1) to update the normalized `ValveState` + flags on OPC UA — all without the OPC UA client knowing the model's raw protocol.
**And** adding another valve model requires only a new ChirpStack codec (zero opcgw change), adding another device kind requires only a new class-registry entry (E-2), and command delivery is reflected in command status (E-3).
**And** generic devices not bound to any class continue to expose arbitrary numeric metrics + raw writable command nodes exactly as before Epic E.

**Release gate (#130):** E-1 must land before tagging **v2.2.0** stable. opcgw must expose raw last-known values with the device's source timestamp and no aggregation (aggregation is the SCADA's job); v2.2.0-rc1 must not be promoted to production with gateway-side aggregation in the value path.

**Vision capture reference:** GitHub issue #129; memory `project_device_abstraction_valves.md`; the 2026-06-06 valve-piloting design dialogue (AskUserQuestion decisions: one command node `1`/`0`, normalized `ValveState` + flags, per-model mapping in the ChirpStack codec, Route B gRPC uplink ingestion).

**Deferred / out-of-scope:**

- `CR-EPIC-C-MQTT` (MQTT real-time path) — Route B uses gRPC `StreamDeviceEvents` instead; MQTT stays deferred.
- Modulating / proportional actuators (0–100% position) — the valve class is binary open/close for now; a `SetPosition` abstraction is deferred until a proportional device exists.
- Encrypted-secrets-in-SQLite and other unrelated v2.x carry-forwards.

## Epic F: Onboarding & Web UX for Public Release

**Tracking:** GitHub issue [#140](https://github.com/guycorbaz/opcgw/issues/140). Literal name `Epic F` in `sprint-status.yaml`, continuing the lettered convention (A/B/C/D/E → F). Stories use sprint-status keys `F-0`..`F-4`. Absorbs CR [#138](https://github.com/guycorbaz/opcgw/issues/138) (uplink stream-set hot-reload — the root cause of "restart after every config change").

**Why it exists:** opcgw is functionally complete (v2.2.0 stable, device-abstraction layer shipped) and the next step is to **announce it to the ChirpStack team**. Before that, the *first-touch* experience — configuration and the web UI, the two things a newcomer judges in the first five minutes — must be smooth. Three friction points today: **(a)** a newcomer must hand-edit `config.toml`/`.env` to supply ChirpStack address/token/tenant before the gateway will even boot — the first-run wizard (`/setup`) only collects the OPC UA password; **(b)** every config save triggers an immediate in-process supervisor restart (`singleton_config.rs` → `"restart_pending"`; the C-0 wizard pattern), so OPC UA clients and the poller are dropped on *each* edit — this is also #138's root cause; **(c)** the web UI is 8 separately hand-rolled vanilla pages (`static/*.html` + `*.js`, shared `dashboard.css`) with no unified nav/shell. This epic makes opcgw effortless to configure (browser-only, zero text-file editing) and pleasant to use, without weakening its lightweight, auditable, no-build-step deployment story.

**Starting point (verified in code 2026-06-14):** config-in-database is *already* ~done from Epics C and D — migration `v009` put applications/devices/metrics/commands in SQLite; `v010` put the singleton `[global]`/`[chirpstack]`/`[opcua]`/`[web]` sections in SQLite; `config.toml` is already **bootstrap-seed-only** (read once on first boot, never mutated). The remaining work is therefore the **first-run experience** and the **apply model**, *not* the data model.

**Locked design decisions (2026-06-14, via design dialogue — AskUserQuestion):**

- **`.env` boundary (Guy's call):** the **opcgw web-UI login user/password** and the **log-file location** STAY in `.env` (infra-level access control + log path; deliberately kept out of the very UI they gate, and out of SQLite). Secrets (ChirpStack API token, OPC UA server password) stay in `config/secrets.toml` (chmod 0600). Logging configuration stays in `log4rs.yaml`. The first-run wizard does **not** touch any of these — it only captures ChirpStack connection + OPC UA.
- **Apply model — staged config + explicit "Apply changes" (the load-bearing call):** rejected both extremes — *live hot-mutation* (too fragile: live OPC UA address-space mutation + live gRPC re-subscribe) and *per-save restart* (the current churn). Instead: config edits write to SQLite (already the authoritative store) **without restarting**; a **"pending changes" affordance** surfaces in the shell; an explicit **"Apply changes"** button triggers **one** graceful **in-process soft restart** that reloads the full config (poller, OPC UA server, gRPC event stream). **The container is never restarted.** No draft/active dual-version schema — "pending" is simply "SQLite has been written since the running processes last loaded their snapshot." This collapses the hard live-mutation work, **auto-fixes #138** (the stream task is torn down and respawned → re-subscribes with the new device set, no bespoke re-subscribe logic), and **eliminates the restart-required allowlist** special-casing (every setting — including the inherently-disruptive OPC UA endpoint/port/security — applies uniformly on Apply). Honest trade-off: a soft restart briefly drops OPC UA sessions + the poller, but **once per batch, operator-initiated** when they choose — which is how SCADA operators expect to apply config.
  - **Two facts verified in code 2026-06-14 (F-0 story-creation analysis) that reshape F-0:** (1) **today's "restart" is a *container* restart, not in-process** — `singleton_config.rs`/`setup.rs` call `shutdown_token.cancel()` → `main()` exits → `docker-compose.yml` `restart: always` relaunches the container; there is **no** in-process restart loop, so **F-0 must build one** (a per-cycle `restart_token` + a supervisor loop wrapping the data-plane spawn in `main.rs`, the deadlock-prone zone — see memory `incident_main_deadlock_2026_05_20`). (2) **config changes are not uniformly "restart on every save" today** — device/metric/app/command CRUD **already applies live** (`notify_crud_write` → watch channel → poller + Story 9-8 OPC UA address-space mutation), only the gRPC stream scope is frozen at boot (= #138) and only the singleton editor + wizard container-restart. **Decision (Guy, 2026-06-14): unify ALL changes under staged + Apply** — CRUD's live `notify_crud_write` apply is removed too; the soft restart becomes the single apply path; the 9-7/9-8 live-reload plumbing goes dormant (full removal = F-0-FOLLOWUP). Full detail in `F-0-staged-config-apply.md`.
- **Web UI — vanilla + shell, no build step:** the no-`npm`, no-build, nothing-to-`install` nature is treated as an **asset** for an auditable industrial gateway shown to a network-server team, not a limitation to fix. Introduce one shared **nav/header/layout shell + component CSS** (responsive) and refactor the 8 existing pages onto it. **No SPA framework**, no build pipeline, no `node_modules`.
- **First-run wizard — zero text-file editing:** extend `/setup` to capture **everything** needed for first boot (ChirpStack `server_address` / `tenant_id` / `api_token` + OPC UA), so an empty `config.toml`/`.env` boots clean entirely from the browser. Secrets → `secrets.toml` 0600, the rest → SQLite. Restart-on-submit is acceptable here — there are no connected clients at first boot, so the disruption is free.

**FRs covered:** none from the original PRD (Epic F is a post-PRD addition driven by the 2026-06-14 pre-announcement readiness requirement; design captured here + in this session's design dialogue). Implicit functional contract per story below.

**Sequencing:** **F-0 → F-1 → F-2 → F-3 → F-4.** F-0 is foundational — it changes the config *apply contract* that everything else relies on, and is the highest-risk story, so it lands first. F-1 (shell) underpins the visuals of F-2/F-3. F-1 and F-2 are fairly independent and could swap. F-3 (dashboard) and F-4 (export/import) sit on top of the shell. Per-story full Acceptance Criteria are drafted when `bmad-create-story F-N` is invoked.

### Story F.0: Staged Config with Explicit "Apply Changes"

As an **opcgw operator editing configuration from the web UI**,
I want my edits to accumulate without restarting the gateway, and to apply them all at once with an explicit button,
So that connected OPC UA clients and the poller aren't dropped on every individual save, and config never requires a container restart.

**Scope summary (full ACs at `bmad-create-story F-0`):**

- Config-write endpoints (singleton-config editor + inventory pickers) persist to SQLite **without** triggering a supervisor restart; the immediate `"restart_pending"` / `CancellationToken.cancel()` reaction is removed from the per-save path.
- A **"pending changes"** signal: the gateway exposes whether SQLite has been written since the running processes last loaded their snapshot (no draft/active dual-version schema — derive it from a write-generation marker). Surface a banner/affordance in the shell (depends on F-1 for placement; ship a minimal indicator if F-1 not yet landed).
- An **"Apply changes"** endpoint + button triggers **one** graceful in-process soft restart that reloads the full config; the container is never restarted.
- Closes **#138**: the gRPC `StreamDeviceEvents` task is respawned on apply and re-subscribes with the current device set — no bespoke hot re-subscribe.
- Restart-required allowlist special-casing is **removed** — all settings (incl. OPC UA endpoint/port/security) apply uniformly via Apply.
- Tests: edits persist without restart; pending-changes marker flips on write and clears after apply; Apply reloads poller + OPC UA + stream; stream re-subscribes to a changed device set; soft restart never escalates to process exit.

### Story F.1: Unified Web Shell (vanilla, no build step)

As an **operator using the opcgw web UI**,
I want a consistent navigation, header, and layout across all pages,
So that the gateway feels like one cohesive application rather than 8 separate pages.

**Scope summary (full ACs at `bmad-create-story F-1`):**

- A shared nav/header/layout shell + component CSS (buttons, forms, tables, status badges, banners), responsive. No build step, no framework, no `node_modules`.
- Refactor the 8 existing pages (`index`, `applications`, `devices-config`, `metrics`, `commands`, `singleton-config`, `inventory-drift`, plus `setup`) onto the shell without behavioural regression.
- Hosts the F-0 "pending changes / Apply" affordance.
- Tests / checks: each page renders on the shell; existing API interactions unchanged; basic responsive layout; no new runtime dependency added.

### Story F.2: Zero-Touch First-Run Wizard

As a **new operator installing opcgw**,
I want to configure everything needed for first boot from the browser,
So that I never have to hand-edit `config.toml` or `.env` to get a working gateway.

**Scope summary (full ACs at `bmad-create-story F-2`):**

- Extend `/setup` to capture ChirpStack `server_address` / `tenant_id` / `api_token` **and** OPC UA settings, so an empty `config.toml`/`.env` boots clean. Secrets → `secrets.toml` 0600; non-secret config → SQLite.
- Does **not** touch web-UI login user/password or log-file location (those stay `.env`).
- Restart-on-submit is acceptable (no connected clients at first boot) — reuses the existing wizard submit → graceful restart path.
- Validation: refuse to complete with placeholder/empty ChirpStack credentials (mirrors the current startup guard); confirm connectivity where feasible.
- Tests: wizard writes secrets to `secrets.toml` (0600) + config to SQLite; empty-bootstrap end-to-end boot; placeholder rejection; web-auth/log-path untouched.

### Story F.3: Dashboard Landing Redesign

As an **operator opening opcgw**,
I want an at-a-glance view of gateway health on the landing page,
So that I can immediately see whether everything is working.

**Scope summary (full ACs at `bmad-create-story F-3`):**

- Redesign the landing/dashboard on the F-1 shell: ChirpStack connection / poller status, device count, last-update / freshness, recent errors — sourced from existing storage + status surfaces (e.g. the `cp0` server-availability device, gateway-status tables).
- No new aggregation in the gateway (consistent with the #130 no-aggregation rule) — display last-known values + status only.
- Tests / checks: dashboard renders real status; degraded states (CS disconnected, stale devices) surface clearly.

### Story F.4: Config Export / Import

As an **operator**,
I want to download my gateway configuration and restore it on another instance,
So that I can back up, version, share, or reproduce a setup (useful for demos).

**Scope summary (full ACs at `bmad-create-story F-4`):**

- Export the gateway config (applications/devices/metrics/commands + singleton sections) to a file (TOML), **secrets excluded by default**.
- Import on a fresh instance, with validation + the F-0 staged-apply flow (import = a batch of pending changes → Apply).
- Tests: export round-trips through import to an equivalent config; secrets are excluded; malformed/partial import is rejected safely.

### Epic F — Story Acceptance Criteria

**Given** a fresh opcgw with an empty `config.toml`/`.env`,
**When** an operator runs `docker compose up` and opens the browser,
**Then** the first-run wizard (F-2) collects ChirpStack connection + OPC UA and the gateway boots fully configured **without editing any text file** (web-auth creds + log path remain `.env` by design).
**And** subsequent config edits (F-0) accumulate as "pending changes" without disrupting connected OPC UA clients, and apply together via an explicit **"Apply changes"** soft restart that never restarts the container (closing #138).
**And** every page presents a consistent shell with unified navigation (F-1), the landing page shows gateway health at a glance (F-3), and the operator can export/import a configuration (F-4).
**And** none of this adds a build step, framework, or `node_modules` to the deployment.

**Vision capture reference:** GitHub issue [#140](https://github.com/guycorbaz/opcgw/issues/140); the 2026-06-14 pre-announcement-readiness design dialogue (AskUserQuestion decisions: vanilla + shell; `.env` boundary for web-auth + log path; staged-config + explicit Apply soft-restart in place of live hot-mutation or per-save restart; zero-touch first-run wizard; config export/import).

**Deferred / out-of-scope:**

- Logging configuration in the DB/UI — `log4rs.yaml` stays file-based (Guy's call 2026-06-14).
- Live, zero-disruption hot-mutation of devices/metrics without any soft restart — explicitly rejected in favour of the staged-apply soft-restart model; revisit only if operators report the brief apply-time blip is unacceptable.
- SPA framework / build pipeline adoption — explicitly rejected to preserve the no-build, auditable deployment story.
- Encrypted-secrets-in-SQLite and other unrelated v2.x carry-forwards.

## Epic G: Web UX & Usability

**Tracking:** GitHub milestone [v2.4.0 — Web UX & Usability](https://github.com/guycorbaz/opcgw/milestones) (#4). Literal name `Epic G` in `sprint-status.yaml`, continuing the lettered convention (A/B/C/D/E/F → G). Stories use sprint-status keys `G-0`..`G-4`, each mapping to an existing GitHub issue: G-0 → [#139](https://github.com/guycorbaz/opcgw/issues/139), G-1 → [#124](https://github.com/guycorbaz/opcgw/issues/124), G-2 → [#142](https://github.com/guycorbaz/opcgw/issues/142), G-3 → [#132](https://github.com/guycorbaz/opcgw/issues/132), G-4 → [#127](https://github.com/guycorbaz/opcgw/issues/127).

**Why it exists:** opcgw was **announced to the ChirpStack community on 2026-06-27** (Epic F delivered the onboarding/first-run polish that gated the announcement). The audience now arriving from the forum judges the gateway by its **web UI** — and the post-Epic-F UI, while functional and unified under the F-1 shell, still has rough edges a newcomer hits within minutes: the configuration screens are **flat** (separate Applications / Devices / Metrics / Commands pages rather than a navigable hierarchy), config fields carry **no inline explanation** of what they mean or what values are valid, metrics can only be chosen from **recently-observed uplink keys** (not the device's declared ChirpStack profile), the OPC UA stale threshold is **global-only** (slow LoRaWAN sensors flap to `Uncertain` between uplinks), and the dashboard shows an **error count with no way to see the actual errors**. Epic G is the first post-announcement release (**v2.4.0**) and turns these into a coherent, self-explanatory configuration experience — at the moment of maximum new-user attention.

**Starting point (verified 2026-06-27):** builds entirely on Epic F. The **F-1 vanilla shell** (`static/shell.js` + component CSS in `dashboard.css`) is the layout substrate; the **C-1/C-2 inventory query layer + pickers** (`/api/inventory/*`, `static/inventory-picker.js`) are the data substrate G-1 extends; the **F-0 staged-apply** model is the write contract every config edit flows through; the **F-3 dashboard** (`/api/status` + `/api/devices`, client-derived health) is what G-4 extends. Two facts from the F-3 story-creation analysis shape G-3/G-4: (1) the per-device stale threshold was **descoped from F-3 to global-only** — G-3 finishes it; (2) **no recent-errors store exists** — `gateway_status.error_count` is a single cumulative `i32` with no event table/ring-buffer, so G-4's error list **requires new storage** (its heaviest piece, and the natural defer candidate if v2.4.0 scope tightens).

**Design principles (carried from Epic F):** vanilla + shared shell, **no build step / framework / `node_modules`**; every config write goes through the **F-0 staged-apply** path (no per-save restart, no live hot-mutation); **no new aggregation** in the gateway (#130 — display last-known values + status only); secrets stay in `secrets.toml`, web-auth + log path stay in `.env`.

**FRs covered:** none from the original PRD (Epic G is a post-PRD, CR-driven addition; the functional contract is the five GitHub issues above + the per-story scope below).

**Sequencing:** **G-0 → G-1 → G-2 → G-3 → G-4.** G-0 is foundational — it restructures the config UI into the Application → Device → Metrics/Commands hierarchy that G-1 (metric picker lives in the device→metrics view) and G-2 (help attaches to the restructured forms) build on. G-3 is small and largely independent. G-4 is the heaviest (new error-event storage) and last; it may be deferred to a later release if v2.4.0 scope tightens. Per-story full Acceptance Criteria are drafted when `bmad-create-story G-N` is invoked.

### Story G.0: Drill-Down Configuration Navigation

As an **operator configuring opcgw from the web UI**,
I want to navigate my configuration as a hierarchy (Application → Device → Metrics/Commands) instead of flat, separate pages,
So that I can see and edit the structure of my deployment in context, the way it actually maps to ChirpStack.

**Scope summary (full ACs at `bmad-create-story G-0`):**

- Restructure the config UI on the F-1 shell into a navigable hierarchy: pick an Application → see/edit its Devices → drill into a Device to see/edit its Metrics and Commands. Replaces the flat `applications` / `devices-config` / `metrics` / `commands` page model with in-context drill-down.
- All writes continue to flow through the F-0 staged-apply path (no per-save restart); existing CRUD endpoints and validation reused.
- No-regression invariant: served-HTML DOM-ID markers and `/api/*` interactions the server-side tests assert must be preserved (shell decorates, doesn't relocate content).
- No build step, no framework, no `node_modules`.
- Tests / checks: each level renders and round-trips its CRUD; navigation reflects the current SQLite config; existing API contracts unchanged.

### Story G.1: Device-Profile Metric Picker

As an **operator adding metrics to a device**,
I want to choose from the measurements declared in the device's ChirpStack device profile,
So that I can configure metrics that haven't been observed in a recent uplink yet (and avoid typos / guessing keys).

**Scope summary (full ACs at `bmad-create-story G-1`):**

- Extend the C-2 inventory metric picker to source candidate metrics from the **ChirpStack device-profile measurements**, not only recently-observed uplink keys; surface both, clearly distinguished. Reuse the C-1 inventory query layer + TTL cache + `?refresh=true` bypass.
- Wire-type inference carried over from C-2; manual-entry fallback preserved.
- Lives in the device → metrics view introduced by G-0.
- Tests / checks: profile-declared measurements appear as choices with no recent uplink; manual fallback still works; cache + refresh behave per C-1.

### Story G.2: Contextual Field Help

As a **newcomer configuring opcgw**,
I want inline help on each configuration field explaining what it does and what values are valid,
So that I can configure the gateway correctly without leaving the UI to read the manual.

**Scope summary (full ACs at `bmad-create-story G-2`):**

- Add contextual/online help (tooltips / inline hints) to each config field across the G-0 restructured forms — what the field means, valid range/format, and the consequence of changing it.
- Help content is static, shipped with the page (no build step); consistent component on the F-1 shell.
- Where useful, link to the relevant section of the LaTeX user manual / docs.
- Tests / checks: help is present and associated with each documented field; accessible (keyboard/screen-reader reachable); no new runtime dependency.

### Story G.3: Per-Device OPC UA Stale Threshold

As an **operator with slow-reporting LoRaWAN sensors**,
I want to set the OPC UA stale threshold per device (not just globally),
So that infrequent sensors don't read `Uncertain` between their normal uplinks while fast devices still flag genuine staleness quickly.

**Scope summary (full ACs at `bmad-create-story G-3`):**

- Add a per-device `stale_threshold_seconds` (overriding the global default, currently 120 s) — finishes the work descoped from F-3. Persisted in SQLite via the existing device config; edited in the G-0 device view; applied via F-0 staged-apply.
- The OPC UA status-code derivation (Good/Uncertain/Bad) honours the per-device threshold; falls back to the global default when unset. `/api/devices` already carries `stale_threshold_seconds` (F-3 pass-through) — extend to per-device.
- Tests / checks: a device with a long threshold stays `Good` past the global window; unset devices use the global default; threshold validated.

### Story G.4: Dashboard Error Drill-Down

As an **operator seeing an error count on the dashboard**,
I want to drill into the actual list of recent errors,
So that I can diagnose what's failing instead of only knowing how many failures occurred.

**Scope summary (full ACs at `bmad-create-story G-4`):**

- Replace the cumulative `error_count` integer on the F-3 dashboard with a drill-down to a list of **recent actual errors** (timestamp, category, message). **Requires NEW storage** — today only a single cumulative `i32` exists in `gateway_status`; G-4 adds a bounded error-event store (event table or ring-buffer) plus an endpoint to read it.
- Consistent with #130: surface recorded events, no new aggregation. Bounded retention (cap + prune) like the existing metric-history / command-history stores.
- **Heaviest story; defer candidate** if v2.4.0 scope tightens (the rest of Epic G ships without it).
- Tests / checks: errors are recorded and capped; the dashboard lists them newest-first; the store prunes; degraded states surface clearly.

### Epic G — Story Acceptance Criteria

**Given** an operator arriving from the ChirpStack community to a running opcgw,
**When** they open the web UI to configure the gateway,
**Then** they navigate their configuration as an Application → Device → Metrics/Commands hierarchy (G-0), choose metrics from the device's ChirpStack profile rather than only observed uplinks (G-1), and understand each field from inline contextual help (G-2).
**And** slow sensors can be given a per-device stale threshold so they don't flap to `Uncertain` between uplinks (G-3), and the dashboard error count drills down to the actual recent errors (G-4).
**And** all of it builds on the F-1 shell and the F-0 staged-apply model, adding no build step, framework, or `node_modules`.

**Vision capture reference:** GitHub milestone #4 (v2.4.0); CRs [#139](https://github.com/guycorbaz/opcgw/issues/139) / [#124](https://github.com/guycorbaz/opcgw/issues/124) / [#142](https://github.com/guycorbaz/opcgw/issues/142) / [#132](https://github.com/guycorbaz/opcgw/issues/132) / [#127](https://github.com/guycorbaz/opcgw/issues/127); the 2026-06-27 post-announcement planning dialogue (AskUserQuestion: theme = Web UX & onboarding; verify-and-close stale issues).

**Deferred / out-of-scope:**

- G-4 (dashboard error drill-down) may slip to a later release if v2.4.0 scope tightens — it is the only story needing new storage.
- Logging configuration in the DB/UI — stays file-based (Epic F decision).
- SPA framework / build pipeline — still rejected.
- #137 (multi-manufacturer device-class registry) and #136 (decouple downlink dispatch) — device-abstraction work, a separate future epic, not part of the Web UX release.

## Epic H: Runtime Correctness & Tech-Debt

**Tracking:** Literal name `Epic H` in `sprint-status.yaml`, continuing the lettered convention (A/B/C/D/E/F/G → H). Stories use sprint-status keys `H-0`, `H-1`, …, each mapping to an existing GitHub issue: H-0 → [#73](https://github.com/guycorbaz/opcgw/issues/73). This is the **v2.x tech-debt epic** flagged as a candidate direction at the close of the Epic G retrospective (2026-06-28).

**Why it exists:** with the feature epics (1–9, A–G) all delivered and opcgw public-facing on the ChirpStack forum, the next risk is no longer missing features — it is **latent runtime-correctness defects** that the test suite and adversarial code review structurally cannot catch (cf. the 2026-05-20 main-deadlock incident and the #146 onboarding bug — both surfaced only by running the real binary). Epic H is the home for that class of work: blocking I/O on the async runtime, resource-lifecycle leaks, and codification of recurring review findings. It carries no new user-facing feature; its deliverable is a gateway that behaves correctly under load and on CPU-constrained deployments.

**Starting point (verified 2026-06-29):** the `StorageBackend` trait (`src/storage/mod.rs:182`) is a **fully synchronous** ~30-method trait (blocking rusqlite), shared as `Arc<dyn StorageBackend>` and invoked from ~30–50 **async** call sites across `src/chirpstack.rs`, `src/chirpstack_events.rs`, `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/web/api.rs`, and `src/main.rs`. Every such call blocks a tokio worker thread on SQL; two deliberate `std::thread::sleep` backoffs (`src/storage/sqlite.rs:1265`, `:1346`) block on pool-exhaustion retry. This has been survivable only because `#[tokio::main]` defaults to the multi-threaded runtime (~32 workers on the dev host) — it degrades sharply on CPU-limited Docker deployments. G-4's code review explicitly deferred "blocking `pool.checkout` #73" as a LOW pointing here.

**Design principles:** correctness first; **no behavioural change** to storage semantics (same return types, same error mapping, same ordering); prefer a **centralised async facade** over scattering `spawn_blocking` at every call site, so the blocking boundary is in exactly one place and call sites read idiomatically; reuse the existing `Arc<dyn StorageBackend>` so both `SqliteBackend` and `InMemoryBackend` are covered without per-backend changes; keep the test suite green at every step (no regression in the 1700+ existing tests).

**FRs covered:** none (Epic H is a post-PRD, tech-debt-driven addition; the contract is the GitHub issues above + the per-story scope below).

**Sequencing:** **H-0 first** (foundational — establishes the async-facade boundary every other storage-touching fix builds on). Later stories (candidates: #110 RunHandles `Drop`, #79 queue-capacity enforcement, substring-matcher / error-classification codification) are added via `correct-course` when scoped. Per-story full Acceptance Criteria are drafted when `bmad-create-story H-N` is invoked.

### Story H.0: Async Storage Facade (spawn_blocking boundary)

As an **operator running opcgw on a CPU-constrained host (e.g. a small Docker container)**,
I want the gateway's database access to never block the async runtime's worker threads,
So that the poller, OPC UA server, and web UI stay responsive under load instead of stalling on SQL.

**Scope summary (full ACs at `bmad-create-story H-0`):**

- Introduce an **async facade** that wraps `Arc<dyn StorageBackend>` and runs every backend call via `tokio::task::spawn_blocking` (the multi-threaded runtime is already in use). The synchronous `StorageBackend` trait and both backend impls (`SqliteBackend`, `InMemoryBackend`) stay unchanged — the facade is the only new abstraction.
- Convert all ~30–50 async call sites (`chirpstack.rs`, `chirpstack_events.rs`, `opc_ua.rs`, `opc_ua_history.rs`, `web/api.rs`, `main.rs`) from direct blocking `backend.method(...)` calls to `facade.method(...).await`. Args crossing the `spawn_blocking` boundary must be owned / `Send + 'static`.
- The two deliberate `std::thread::sleep` pool-retry backoffs (`sqlite.rs:1265`, `:1346`) now run inside `spawn_blocking`, off the async executor — acceptable as-is once moved off the runtime (a blocking sleep inside `spawn_blocking` is correct).
- **No behavioural change:** identical return types, error mapping (`OpcGwError`), and result ordering; purely an execution-context change. Synchronous (non-async) call sites — e.g. tests, startup migration — may continue calling the backend directly.
- Tests / checks: existing 1700+ tests stay green; add coverage proving storage calls run off the runtime worker threads (e.g. a blocking-detection or concurrency test); `cargo clippy --all-targets -- -D warnings` clean. Closes #73.

### Epic H — Story Acceptance Criteria

**Given** opcgw running on a multi-threaded tokio runtime with a synchronous SQLite backend,
**When** any async task (poller, event stream, OPC UA server, web handler) reads or writes storage,
**Then** the blocking SQL and pool-retry sleeps execute on the `spawn_blocking` pool via the async facade (H-0), never on an async worker thread,
**And** storage semantics — return values, error mapping, and ordering — are byte-for-byte unchanged, with the full existing test suite green.

**Vision capture reference:** GH issue [#73](https://github.com/guycorbaz/opcgw/issues/73) (rescoped 2026-06-29 from a mis-aimed one-liner to the real sync-on-async bug); the Epic G retrospective "NEXT" note (2026-06-28) naming a v2.x tech-debt epic with #73 as the headline item; the 2026-06-29 owner decision (Strategy A = async facade, full BMad story flow).

**Deferred / out-of-scope:**

- #110 (RunHandles lacks `Drop` impl — gauge task leak) — candidate future H-story; not in H-0.
- #79 (queue-capacity enforcement, 10 000 max) — candidate future H-story.
- Substring-matcher / error-classification codification (recurring review-finding class) — candidate future H-story.
- Migrating `StorageBackend` to a natively-`async` trait or an async SQLite driver — explicitly rejected for H-0 (far larger blast radius; the `spawn_blocking` facade achieves correctness without rewriting the backends).

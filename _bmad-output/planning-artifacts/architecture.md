---
stepsCompleted:
  - step-01-init
  - step-02-context
  - step-03-starter
  - step-04-decisions
  - step-05-patterns
  - step-06-structure
  - step-07-validation
  - step-08-complete
status: 'complete'
completedAt: '2026-04-01'
inputDocuments:
  - _bmad-output/planning-artifacts/prd.md
  - _bmad-output/planning-artifacts/product-brief-opcgw.md
  - _bmad-output/planning-artifacts/product-brief-opcgw-distillate.md
  - docs/index.md
  - docs/project-overview.md
  - docs/architecture.md
  - docs/source-tree-analysis.md
  - docs/api-contracts.md
  - docs/development-guide.md
  - docs/deployment-guide.md
workflowType: 'architecture'
project_name: 'opcgw'
user_name: 'Guy'
date: '2026-04-01'
---

# Architecture Decision Document — opcgw

_This document builds collaboratively through step-by-step discovery. Sections are appended as we work through each architectural decision together._

## Project Context Analysis

### Requirements Overview

**Functional Requirements:**
50 FRs across 8 capability areas. Architecturally, they break into three tiers:

- **Tier 1 — Refactor existing code (Phase A):** FR1-8 (ChirpStack polling — needs error handling + pagination + metric types), FR9-13 (commands — needs FIFO + persistence + validation), FR14-20 (OPC UA current — needs stale-data + health metrics), FR25-30 (persistence — new), FR31-33 (config — exists), FR42-49 (security + reliability — refactoring)
- **Tier 2 — New capabilities on refactored foundation (Phase B):** FR21-24 (OPC UA extended — subscriptions, historical, alarms, dynamic nodes), FR34-41 + FR50 (web UI — entirely new subsystem)
- **Tier 3 — Cross-cutting (both phases):** FR46 (no panics), FR47 (graceful shutdown), FR48 (clean startup from SQLite)

**Non-Functional Requirements:**
24 NFRs. Architecture-driving NFRs:

- NFR1 (<100ms OPC UA reads) — SQLite with WAL mode provides sufficient read performance for this scale (~400 rows). In-memory backend for tests only; no dual-layer cache needed.
- NFR5 (<256MB RSS) — Bounded memory. Historical data lives in SQLite only; no in-memory accumulation.
- NFR16 (30-day crash-free) — Every error path must be graceful. No unwrap() in production code.
- NFR17 (<30s auto-recovery) — ChirpStack reconnection must be non-blocking and automatic.
- NFR19 (survive unclean shutdown) — SQLite WAL mode mandatory. Command queue must be durable.
- NFR20 (FIFO under concurrency) — Command queue ordering guaranteed by database ordering (ORDER BY created_at ASC), not in-memory data structure.

**Scale & Complexity:**

- Primary domain: IoT gateway middleware (Rust async)
- Complexity level: Medium-high
- Estimated architectural components: 7 (ChirpStack poller, OPC UA server, storage trait + backends, command processor, config manager, web server, health monitor)

### Technical Constraints & Dependencies

| Constraint | Impact | Source |
|-----------|--------|--------|
| Rust 2021, min rustc 1.87.0 | Language-level constraint, no nightly features | Cargo.toml |
| async-opcua v0.16.x | OPC UA server API surface; subscription support unverified | Cargo.toml, PRD risk matrix |
| chirpstack_api v4.13.0 | gRPC client stubs; API version pinned | Cargo.toml |
| Tokio 1.47.1 full | Async runtime for all concurrent tasks | Cargo.toml |
| Docker on Synology NAS | Deployment target; mapped volumes for persistence | PRD IoT Gateway Requirements |
| Single-process architecture | All services (poller, OPC UA, web) in one binary/container | Current architecture |
| No breaking config changes in Phase A | config.toml format must remain backward-compatible until v2.0 | PRD operational risk |

### Cross-Cutting Concerns Identified

1. **Error handling** — Touches every module. Must be addressed first as foundation. Pattern: `Result<T, OpcGwError>` propagation, log + skip for non-fatal errors, graceful degradation for all external failures.
2. **Storage access** — 3 concurrent consumers (poller writes, OPC UA reads, web UI reads/writes). Current `Arc<Mutex<Storage>>` must become `Arc<RwLock<dyn StorageBackend>>` or similar. Lock contention is the primary concurrency concern.
3. **Configuration lifecycle** — Phase A: static at startup. Phase B: dynamic with hot-reload. Architecture must support both without redesign. Pattern: `Arc<RwLock<AppConfig>>` with change notification.
4. **Graceful shutdown coordination** — SIGTERM must coordinate shutdown across 2 tasks (Phase A) or 3 tasks (Phase B). Pattern: `tokio::sync::broadcast` or `CancellationToken`.
5. **Logging and observability** — Per-module file appenders already in place. Gateway health metrics (FR18) add OPC UA-visible self-monitoring. No external observability stack needed.
6. **Data freshness tracking** — Every metric value must carry a timestamp. Staleness detection (FR17) requires comparing current time vs last-update. Pervades the storage layer and OPC UA value serving.

## Technology Stack & New Dependencies

### Dependency Policy

Use latest stable versions. When adding new dependencies, use current version. When touching existing dependencies during Phase A, update to latest. Pin minimum Rust version to current stable.

### Updated Stack — Current → Target

| Component | Crate | Current | Target | Change |
|-----------|-------|---------|--------|--------|
| **Rust** | rustc | 1.87.0 | **1.94.0** | +7 releases |
| Async Runtime | tokio | 1.47.1 | **1.50.0** | Update |
| gRPC Client | tonic | 0.13.1 | **0.14.5** | Major update — verify interceptor API |
| Protobuf Build | tonic-build | 0.13.1 | **0.14.5** | Major update |
| ChirpStack SDK | chirpstack_api | 4.13.0 | **4.15.0** | Update |
| OPC UA Server | async-opcua | 0.16.x | **0.17.1** | Update — check SimpleNodeManager API changes |
| Configuration | figment | 0.10.19 | 0.10.19 | No change (mature, stable) |
| Serialization | serde | 1.0.219 | **1.0.228** | Update |
| CLI | clap | 4.5.47 | **4.6.0** | Update |
| Logging facade | ~~log~~ | ~~0.4.28~~ | **tracing** | **MIGRATE** — async-native structured logging |
| Logging framework | ~~log4rs~~ | ~~1.4.0~~ | **tracing-subscriber + tracing-appender** | **MIGRATE** — replace log4rs.yaml with programmatic config |
| Error Handling | thiserror | 2.0.16 | **2.0.18** | Update |
| URL | url | 2.5.7 | 2.5.7 | No change |
| IP Detection | local-ip-address | 0.6.5 | 0.6.5 | No change (small utility, low risk) |

### New Dependencies

| Component | Crate | Version | Phase | Rationale |
|-----------|-------|---------|-------|-----------|
| SQLite | rusqlite | **0.38.0** | A | Mature SQLite wrapper. `bundled` feature — no system dependency. |
| Shutdown | tokio-util | **0.7.18** | A | `CancellationToken` for graceful shutdown coordination. |
| Logging facade | tracing | latest | A | Tokio-native structured logging. Replaces `log` crate. |
| Logging output | tracing-subscriber | latest | A | Formatting + filtering. Replaces log4rs config. |
| Logging files | tracing-appender | latest | A | Per-module file appenders. Non-blocking writes. |
| Web Server | axum | **0.8.8** | B | Tokio-native web framework. Tower middleware. |

### Migration Notes

**log → tracing migration (Phase A, early task):**
- Replace `use log::{debug, info, error, trace, warn}` with `use tracing::{debug, info, error, trace, warn}` — same macro names, mechanical find-and-replace.
- Replace `log4rs::init_file()` in `main.rs` with programmatic `tracing_subscriber` setup.
- Replace `config/log4rs.yaml` with Rust-based configuration of per-module file appenders using `tracing-appender`.
- Remove `log` and `log4rs` from `Cargo.toml`.
- Benefits: async-native (no blocking in hot logging paths), structured key-value logging, maintained by Tokio team.

**tonic 0.13 → 0.14 (Phase A, dependency update task):**
- Major version bump. Interceptor API may have changed — verify `AuthInterceptor` in `chirpstack.rs` compiles. Channel creation API may differ.
- Test against chirpstack_api 4.15.0 compatibility.

**async-opcua 0.16 → 0.17 (Phase A, dependency update task):**
- Check changelog for `SimpleNodeManager` API changes. This is the library where subscription support will be spiked.

### Dependency Decisions

**rusqlite** over sqlx (async overhead unnecessary) and sled (no SQL queries). `bundled` feature compiles SQLite from source.

**axum** over actix-web (not Tokio-native) and warp (less maintained). Tower middleware ecosystem.

**tracing** over log+log4rs: log4rs stale since Sep 2025 with known performance issues. tracing is the modern Rust standard, async-native, maintained by Tokio team.

**figment** kept: mature, stable, reputable author (Sergio Benitez). No better alternative.

**local-ip-address** kept: trivial usage (3 lines in opc_ua.rs), low risk.

## Core Architectural Decisions

### Decision Priority Analysis

**Critical Decisions (Block Implementation):**
1. Storage trait design — thin trait, simple get/set
2. Concurrency model — separate SQLite connections per task, no shared lock
3. Error handling — single `OpcGwError` enum, add variants as needed
4. OPC UA data flow — poller pushes DataValues into address space (pending spike validation)

**Important Decisions (Shape Architecture):**
5. Config hot-reload — watch channel notification, tasks reload independently
6. Graceful shutdown — flat `CancellationToken`, all tasks check at safe points

**Deferred Decisions (Phase B):**
- Web UI authentication mechanism (basic auth decided, implementation details deferred)
- OPC UA alarm threshold configuration model
- Historical data query API design

### Data Architecture

**Storage Backend:**
- Thin `StorageBackend` trait with simple get/set methods
- `SqliteBackend` for production (rusqlite 0.38.0, WAL mode, `bundled` feature)
- `InMemoryBackend` (HashMap) for unit tests only
- No dual-layer cache — SQLite read performance sufficient at this scale

**SQLite Schema (5 tables):**
- `metric_values` — Hot table, UPSERT per poll cycle. Primary key: (device_id, metric_name)
- `metric_history` — Append-only log with timestamps. Indexed on (device_id, metric_name, recorded_at)
- `command_queue` — Persistent FIFO. Status column (pending/sent/failed). Ordered by created_at ASC
- `gateway_status` — Key-value store for health metrics
- `retention_config` — Pruning rules (max_age_days per table)

**Concurrency Model:**
- Each async task owns its own SQLite `Connection` — no shared lock for data access
- Poller: write connection (metrics + command processing)
- OPC UA server: read connection (metric reads + health status)
- Web UI (Phase B): read/write connection (config CRUD + live metrics)
- SQLite WAL mode enables concurrent readers + single writer across connections
- Shared state limited to: `CancellationToken` (shutdown) + `Arc<RwLock<AppConfig>>` (Phase B config)

### Error Handling

**Single error type:** `OpcGwError` enum with variants:
- `Configuration(String)` — config loading, parsing, validation
- `ChirpStack(String)` — gRPC communication, API errors
- `OpcUa(String)` — OPC UA server errors
- `Storage(String)` — SQLite operations, data access
- `Database(String)` — new: SQLite-specific errors (migration, corruption)
- `WebServer(String)` — new (Phase B): Axum server errors

**Error propagation:** `Result<T, OpcGwError>` throughout. No `unwrap()` or `panic!()` in production paths. Non-fatal errors logged and skipped (e.g., single device metric fetch failure doesn't stop the poll cycle).

### Communication Patterns

**Config hot-reload (Phase B):**
- Web UI writes validated config to SQLite + TOML file
- Sends `tokio::sync::watch` notification to all tasks
- Each task reloads config at a safe point in its cycle:
  - Poller: between poll cycles
  - OPC UA: rebuilds affected address space nodes
- Atomic: validation before apply, rollback on failure
- Phase A: static config at startup, `watch` channel plumbed but not triggered

**OPC UA data flow (Phase B, pending spike):**
- Design for push model: poller writes `DataValue`s (with timestamps + status codes) into OPC UA address space after each poll cycle
- async-opcua's subscription engine detects value changes and notifies subscribed clients
- Fallback if spike fails: change-detection layer with manual notification triggering
- Phase A: keep existing read callbacks (pull model)

### Infrastructure & Deployment

**Graceful shutdown:**
- `CancellationToken` from tokio-util, created in `main()`
- Clone passed to each task (poller, OPC UA, web UI)
- SIGTERM handler cancels the token
- Each task checks `token.is_cancelled()` at safe points
- Shutdown sequence: stop accepting new OPC UA connections → complete in-progress poll → flush SQLite → exit

**Docker deployment (unchanged):**
- Multi-stage build: rust:1.94 → ubuntu
- Mapped volumes: `./config`, `./pki`, `./log`, `./data` (new: SQLite)
- Port mapping: 4855 (OPC UA), TBD (web UI, Phase B)
- `restart: always` policy
- Manual image version pinning in docker-compose.yml

### Decision Impact Analysis

**Implementation Sequence:**
1. Update all dependencies + Rust version (foundation)
2. Migrate log → tracing (touches every file, do early)
3. Error handling overhaul (foundation for everything)
4. Storage trait + SQLite implementation (new data layer)
5. Refactor poller to use own SQLite connection (remove Arc<Mutex<Storage>>)
6. Refactor OPC UA to use own SQLite connection
7. FIFO command queue (now SQLite-backed)
8. Remaining Phase A items (pagination, metric types, stale-data, health, security)
9. async-opcua subscription spike

**Cross-Component Dependencies:**
- Storage trait must be designed before any module refactoring
- Error handling overhaul must precede storage refactoring (clean error paths needed)
- tracing migration is independent — can be done in parallel with error handling
- Concurrency model change (separate connections) depends on storage trait being complete
- Config watch channel can be plumbed in Phase A but only activated in Phase B

## Implementation Patterns & Consistency Rules

### Naming Conventions

**Rust code:** Standard Rust conventions (compiler-enforced):
- Functions/variables: `snake_case`
- Types/structs/enums: `PascalCase`
- Constants: `SCREAMING_SNAKE_CASE`
- Modules/files: `snake_case`

**SQLite:**
- Tables: `snake_case` plural (`metric_values`, `command_queue`)
- Columns: `snake_case` (`device_id`, `metric_name`, `updated_at`)
- Indexes: `idx_{table}_{columns}` (`idx_history_device_time`)
- No ORM — raw SQL with prepared statements via rusqlite

**OPC UA nodes:**
- Application folders: exact `application_name` from config
- Device folders: exact `device_name` from config
- Metric variables: exact `metric_name` from config
- Health metrics: constants from `utils.rs` (`OPCGW_CP_*`)

**Web UI (Phase B):**
- REST endpoints: `/api/{resource}` (plural, lowercase)
- Static files: `/static/{filename}`
- HTML templates: `snake_case.html`

### Error Handling Patterns

**Rule 1: No `unwrap()` or `panic!()` in production paths.** Use `?` operator with `Result<T, OpcGwError>`.

**Rule 2: Non-fatal errors are logged and skipped.** A single device failure must not stop the poll cycle. A single OPC UA client error must not crash the server.

```rust
// Pattern: log + skip for non-fatal
for dev_id in device_ids {
    match self.fetch_metrics(&dev_id).await {
        Ok(metrics) => self.store_metrics(&dev_id, metrics)?,
        Err(e) => {
            tracing::warn!(device_id = %dev_id, error = %e, "Skipping device");
            continue;
        }
    }
}
```

**Rule 3: Fatal errors propagate to main.** If SQLite is corrupted or the OPC UA server can't bind its port, propagate the error up to `main()` for clean shutdown.

**Rule 4: Error context matters.** Always include relevant identifiers (device_id, metric_name) in error messages and tracing spans.

### Tracing Patterns

**Structured logging with key-value fields — not format strings:**

```rust
// CORRECT
tracing::debug!(device_id = %dev_id, metric = %name, "Stored metric");
tracing::warn!(error = %e, retry = count, "ChirpStack connection failed");

// WRONG
tracing::debug!("Stored metric {} for {}", name, dev_id);
```

**Log levels:**
- `error!` — Fatal or requires attention (ChirpStack down, SQLite error, OPC UA bind failure)
- `warn!` — Recovered from error (skipped device, retrying connection)
- `info!` — Lifecycle events (gateway starting, stopping, config reloaded)
- `debug!` — Operational detail (poll cycle complete, device metrics stored)
- `trace!` — Verbose debugging (individual metric values, gRPC request/response)

### SQLite Access Patterns

**Prepared statements only — never format SQL strings:**

```rust
// CORRECT
let mut stmt = self.conn.prepare_cached("SELECT ... WHERE device_id = ?1")?;
stmt.query_row(params![device_id], |row| { ... })

// FORBIDDEN — SQL injection risk
let q = format!("SELECT ... WHERE device_id = '{}'", device_id);
```

**Batch writes — one transaction per poll cycle:**

```rust
let tx = self.conn.transaction()?;
for (dev_id, metrics) in poll_results {
    for (name, value) in metrics {
        tx.execute("INSERT OR REPLACE INTO metric_values ...", params![...])?;
        tx.execute("INSERT INTO metric_history ...", params![...])?;
    }
}
tx.commit()?;
```

**Connection ownership — one per task, never shared:**

```rust
// main.rs
let poller_conn = Connection::open("data/opcgw.db")?;
let opcua_conn = Connection::open("data/opcgw.db")?;
// Each task owns its connection, SQLite WAL handles concurrency
```

### Test Patterns

**Unit tests:** Use `InMemoryBackend`, co-located in `#[cfg(test)]` modules.

**Integration tests:** In `tests/` directory, use `SqliteBackend` with temporary database file. Clean up after test.

**Test naming:** `test_{module}_{behavior}` — e.g., `test_storage_metric_roundtrip`, `test_poller_recovers_from_chirpstack_outage`.

**Each Phase A item's definition-of-done includes its tests.** No separate "test writing" phase.

### File Organization

See "Project Structure & Boundaries" section below for the complete directory tree and module organization.

### Anti-Patterns (Agents Must Avoid)

- `unwrap()`, `expect()`, `panic!()` in any non-test code
- `format!()` in SQL queries
- Sharing SQLite `Connection` across tasks
- Logging without structured fields
- Adding new modules without updating `OpcGwError` variants
- Holding locks across `.await` points
- Writing tests that depend on real ChirpStack or OPC UA connections (use mocks)

### Enforcement

- `cargo clippy` must pass with no warnings
- `cargo test` must pass 100% before any commit
- CI pipeline (GitHub Actions) enforces both checks on every PR

## Project Structure & Boundaries

### Complete Project Directory Structure

```
opcgw/
├── Cargo.toml                         # Package manifest (updated dependencies)
├── Cargo.lock                         # Dependency lock file
├── build.rs                           # Proto compilation (tonic-build)
├── Dockerfile                         # Multi-stage build (rust:1.94 → ubuntu)
├── docker-compose.yml                 # Service definition (+ data/ volume)
├── Makefile.toml                      # cargo-make tasks (test, coverage)
├── README.md                          # Project documentation
├── CLAUDE.md                          # AI assistant instructions
├── CONTRIBUTING.md                    # Contribution guidelines
│
├── src/
│   ├── main.rs                        # Entry point, task spawning, CancellationToken, shutdown
│   ├── chirpstack.rs                  # ChirpStack gRPC poller (FR1-8, FR9-13)
│   ├── opc_ua.rs                      # OPC UA server (FR14-24)
│   ├── config.rs                      # AppConfig loading + validation (FR31-33)
│   ├── utils.rs                       # OpcGwError, constants
│   │
│   ├── storage/                       # Storage module directory
│   │   ├── mod.rs                     # StorageBackend trait, MetricType, DeviceCommand, ChirpstackStatus
│   │   ├── sqlite.rs                  # SqliteBackend implementation (FR25-30)
│   │   ├── memory.rs                  # InMemoryBackend (tests only)
│   │   └── schema.rs                  # SQLite schema creation + version migrations
│   │
│   └── web/                           # [Phase B] Web UI module directory
│       ├── mod.rs                     # Axum router, server startup (FR34-41)
│       ├── api.rs                     # REST API endpoints (config CRUD)
│       ├── auth.rs                    # Basic auth middleware (FR50)
│       └── static_files.rs            # Static file serving
│
├── migrations/                        # SQLite migration SQL files
│   ├── 001_initial_schema.sql         # metric_values, metric_history, command_queue, gateway_status
│   └── 002_retention_config.sql       # retention_config table
│   # Embedded in binary via include_str!() — no runtime file dependency
│
├── proto/                             # ChirpStack protobuf definitions (unchanged)
│   ├── chirpstack/api/                # Device, Application, Gateway services
│   ├── chirpstack/stream/             # Frame, meta, backend interfaces
│   ├── chirpstack/integration/        # Integration service
│   ├── chirpstack/internal/           # Internal service
│   ├── chirpstack/gw/                 # Gateway proto
│   ├── google/api/                    # HTTP annotations
│   └── common/                        # Common definitions
│
├── config/
│   └── config.toml                    # Application configuration
│   # Note: log4rs.yaml REMOVED after tracing migration
│
├── data/                              # SQLite database (Docker volume: ./data)
│   └── opcgw.db                       # Created at runtime (path configurable via config.toml)
│
├── static/                            # [Phase B] Web UI static files
│   ├── index.html                     # Dashboard (gateway status + live metrics)
│   ├── applications.html              # Application CRUD
│   ├── devices.html                   # Device + metric mapping CRUD
│   ├── commands.html                  # Command CRUD
│   └── css/
│       └── style.css                  # Minimal styling
│
├── pki/                               # OPC UA certificates (Docker volume: ./pki)
│   ├── own/                           # Server certificate
│   ├── private/                       # Server private key
│   ├── trusted/                       # Trusted client certs
│   └── rejected/                      # Rejected certs
│
├── log/                               # Runtime log output (Docker volume: ./log)
│
├── tests/
│   ├── config/
│   │   └── config.toml                # Test-specific configuration (existing)
│   ├── common/
│   │   └── mod.rs                     # Shared test helpers (config loading, temp DB)
│   ├── storage_sqlite.rs              # SQLite backend integration tests
│   ├── command_queue.rs               # FIFO ordering under concurrency
│   └── graceful_shutdown.rs           # Shutdown coordination tests
│
├── doc/                               # Hand-written project documentation (unchanged)
├── docs/                              # Generated project documentation (unchanged)
└── _bmad-output/                      # BMad planning artifacts
```

### Configuration Addition

```toml
# config.toml — new [storage] section (Phase A)
[storage]
database_path = "data/opcgw.db"
retention_days = 7
```

### Architectural Boundaries

**Protocol Boundaries (external):**

| Boundary | Module | Protocol | Direction |
|----------|--------|----------|-----------|
| ChirpStack API | `chirpstack.rs` | gRPC (tonic) | Outbound |
| OPC UA Clients | `opc_ua.rs` | OPC UA TCP (async-opcua) | Inbound |
| Web UI Clients | `web/mod.rs` (Phase B) | HTTP (axum) | Inbound |

**Internal Module Boundaries:**

```
main.rs (orchestrator)
  ├── Creates SQLite connections (one per task)
  ├── Creates CancellationToken
  ├── Runs schema migrations (schema.rs, embedded SQL)
  ├── Loads AppConfig
  └── Spawns tasks:
      │
      ├── chirpstack.rs (write path)
      │   ├── Owns: SQLite write Connection
      │   ├── Reads: AppConfig (static Phase A, watch channel Phase B)
      │   ├── Writes: metric_values, metric_history, gateway_status
      │   ├── Reads/Writes: command_queue (SELECT pending → UPDATE sent)
      │   └── Checks: CancellationToken between poll cycles
      │
      ├── opc_ua.rs (read path)
      │   ├── Owns: SQLite read Connection
      │   ├── Reads: AppConfig
      │   ├── Reads: metric_values, gateway_status
      │   ├── Writes: command_queue (INSERT on OPC UA Write)
      │   └── Checks: CancellationToken between request handling
      │
      └── web/ [Phase B] (config path)
          ├── Owns: SQLite read/write Connection
          ├── Reads/Writes: AppConfig via watch channel
          ├── Reads: metric_values, gateway_status (live display)
          ├── Writes: config changes (CRUD)
          ├── Sends: config change notification via watch channel
          └── Checks: CancellationToken on shutdown
```

**Data Boundaries:**

| Table | Writer | Readers | Access Pattern |
|-------|--------|---------|---------------|
| `metric_values` | Poller (UPSERT) | OPC UA, Web UI | Hot table, keyed lookup |
| `metric_history` | Poller (INSERT) | OPC UA (Phase B HA), Web UI | Append-only, time-range queries |
| `command_queue` | OPC UA (INSERT), Web UI | Poller (SELECT + UPDATE) | FIFO, status transitions |
| `gateway_status` | Poller (UPDATE) | OPC UA, Web UI | Key-value, small |
| `retention_config` | Web UI (Phase B) | Poller (prune job) | Rarely accessed |

### Requirements to Structure Mapping

| FR Category | Primary Module | Supporting Modules |
|-------------|---------------|-------------------|
| ChirpStack Data Collection (FR1-8) | `chirpstack.rs` | `storage/sqlite.rs`, `config.rs` |
| Device Command Execution (FR9-13) | `chirpstack.rs` + `opc_ua.rs` | `storage/sqlite.rs` (command_queue) |
| OPC UA Server Current (FR14-20) | `opc_ua.rs` | `storage/mod.rs`, `utils.rs` |
| OPC UA Server Extended (FR21-24) | `opc_ua.rs` | `storage/sqlite.rs` (historical) |
| Data Persistence (FR25-30) | `storage/sqlite.rs` | `storage/schema.rs` |
| Config Current (FR31-33) | `config.rs` | `main.rs` |
| Config Web UI (FR34-41, FR50) | `web/` | `config.rs`, `storage/sqlite.rs` |
| Security (FR42-45) | Distributed | `config.rs`, `opc_ua.rs`, `web/auth.rs` |
| Operational Reliability (FR46-49) | Distributed | All modules |

### Data Flow

```
ChirpStack Server
       │
       ▼ (gRPC poll every N seconds)
┌──────────────┐
│ chirpstack.rs │──► SQLite: metric_values (UPSERT)
│  (poller)     │──► SQLite: metric_history (INSERT)
│               │──► SQLite: gateway_status (UPDATE)
│               │◄── SQLite: command_queue (SELECT pending → UPDATE sent)
└──────────────┘──► ChirpStack: Enqueue command (gRPC)

SQLite DB (WAL mode, separate connections per task)
       │
       ▼ (read on demand)
┌──────────────┐
│  opc_ua.rs   │◄── SQLite: metric_values (SELECT)
│  (server)    │◄── SQLite: gateway_status (SELECT)
│              │──► SQLite: command_queue (INSERT on OPC UA Write)
└──────────────┘
       │
       ▼ (OPC UA TCP)
  SCADA Clients (FUXA)

┌──────────────┐ [Phase B]
│   web/       │◄── SQLite: metric_values (SELECT, live display)
│ (Axum HTTP)  │◄── SQLite: gateway_status (SELECT)
│              │──► SQLite: config (CRUD)
└──────────────┘──► watch channel: config change notification
       │
       ▼ (HTTP)
  Browser (config UI)
```

### Docker Deployment

```yaml
# docker-compose.yml (updated for Phase A)
services:
  opcgw:
    container_name: opcgw
    image: opcgw:1.1.0    # pinned version
    restart: always
    ports:
      - "4855:4855"        # OPC UA
      # - "8080:8080"      # Web UI (Phase B)
    volumes:
      - ./log:/usr/local/bin/log
      - ./config:/usr/local/bin/config
      - ./pki:/usr/local/bin/pki
      - ./data:/usr/local/bin/data     # NEW: SQLite persistence
```

## Architecture Validation Results

### Coherence Validation

**Decision Compatibility:** All technology choices verified compatible — Tokio 1.50 + async-opcua 0.17 + tonic 0.14 + axum 0.8 share the same async runtime. rusqlite (synchronous) acceptable since each task owns its connection and operations are sub-millisecond at current scale. tracing ecosystem integrates natively with Tokio.

**Pattern Consistency:** Error handling (single OpcGwError) aligns with thin storage trait. Structured tracing aligns with per-module appenders. SQLite patterns (prepared statements, batch writes) align with poll-cycle architecture. Rust naming enforced by compiler.

**Structure Alignment:** `storage/` module directory cleanly separates trait from backends. Separate SQLite connections per task match module boundaries. `migrations/` with `include_str!()` produces self-contained binary. Phase B structure (`web/`, `static/`) prepared but not premature.

**Result:** No conflicts found.

### Requirements Coverage

All 50 FRs mapped to architectural components. All 24 NFRs addressed by architectural decisions. One risk flag: FR21-24 (OPC UA subscriptions/historical/alarms) depend on async-opcua spike — architecture designed for push model with documented fallback.

### Gap Resolutions

1. **OPC UA health metrics:** Exposed under `Objects/Gateway/` folder with variables `LastPollTimestamp`, `ErrorCount`, `ChirpStackAvailable`.
2. **Retention pruning:** Configurable interval via `[storage] prune_interval_minutes` (default 60). Runs as periodic task within poller, deletes rows older than `retention_days`.

### Configuration Addition (Final)

```toml
# config.toml — [storage] section
[storage]
database_path = "data/opcgw.db"
retention_days = 7
prune_interval_minutes = 60
```

### Architecture Completeness Checklist

**Requirements Analysis**
- [x] Project context thoroughly analyzed (brownfield, 50 FRs, 24 NFRs)
- [x] Scale and complexity assessed (100 devices, 5 clients, medium-high)
- [x] Technical constraints identified (async-opcua risk, Docker/NAS deployment)
- [x] Cross-cutting concerns mapped (error handling, storage, config, shutdown, logging, freshness)

**Architectural Decisions**
- [x] 6 critical/important decisions documented with rationale
- [x] Technology stack fully specified with verified latest versions
- [x] Dependency updates + tracing migration planned
- [x] Integration patterns defined (separate connections, watch channel, CancellationToken)

**Implementation Patterns**
- [x] Naming conventions (Rust standard + SQLite + OPC UA nodes)
- [x] Error handling rules with code examples (4 rules)
- [x] Tracing patterns with structured fields
- [x] SQLite access patterns (prepared statements, batch writes, connection ownership)
- [x] Test patterns (unit with InMemoryBackend, integration with SqliteBackend)
- [x] Anti-patterns documented (7 forbidden practices)
- [x] Enforcement via cargo clippy + cargo test + CI

**Project Structure**
- [x] Complete directory structure with module directories
- [x] Component boundaries (protocol + internal + data)
- [x] Requirements-to-structure mapping (all 50 FRs)
- [x] Migrations embedded via include_str!()
- [x] Docker deployment updated with data volume

### Architecture Readiness Assessment

**Overall Status:** READY FOR IMPLEMENTATION

**Confidence Level:** High

**Key Strengths:**
- No shared locks for data access — SQLite WAL handles concurrency natively
- Clean module boundaries — each task owns its resources
- Thin abstractions — StorageBackend trait, single error type, flat CancellationToken
- Brownfield-aware — preserves config format, incremental migration path

**Areas for Future Enhancement:**
- Web UI REST API contract (Phase B design)
- OPC UA Alarms threshold configuration model (Phase B)
- tokio::sync::RwLock evaluation if SQLite blocking becomes measurable

### Implementation Handoff

**AI Agent Guidelines:**
- Follow all architectural decisions exactly as documented
- Use implementation patterns consistently across all components
- Respect project structure and module boundaries
- Refer to this architecture document for all design questions
- Each item's definition-of-done includes its tests

**First Implementation Priority:**
1. Update Cargo.toml: Rust 1.94, all dependency versions
2. Migrate log → tracing (mechanical, touches every file)
3. Error handling overhaul (foundation for everything)
4. Storage module directory + trait + SqliteBackend

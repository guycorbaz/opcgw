![build and test](https://github.com/guycorbaz/opcgw/actions/workflows/ci.yml/badge.svg)
![Version](https://img.shields.io/badge/version-2.0.0-blue)
![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-green)

# opcgw - ChirpStack to OPC UA Gateway

Bridge LoRaWAN device data from ChirpStack to industrial automation systems via OPC UA.

**opcgw** is a production-ready gateway that connects ChirpStack 4 LoRaWAN Network Server with OPC UA industrial clients, enabling seamless integration of wireless IoT devices into SCADA, MES, and industrial edge systems.

> 📖 **Full Documentation**: Visit the [GitHub Pages](https://guycorbaz.github.io/opcgw/) for detailed guides, architecture diagrams, and real-world use cases.

## Quick Links

- 🚀 [Quick Start Guide](https://guycorbaz.github.io/opcgw/quickstart/)
- 🏗️ [Architecture & Design](https://guycorbaz.github.io/opcgw/architecture/)
- ⚙️ [Configuration Reference](https://guycorbaz.github.io/opcgw/configuration/)
- 📋 [Features & Roadmap](https://guycorbaz.github.io/opcgw/features/)
- 💼 [Use Cases](https://guycorbaz.github.io/opcgw/usecases/)

## What is opcgw?

opcgw solves a critical integration challenge: connecting wireless LoRaWAN IoT networks managed by ChirpStack to industrial automation systems that speak OPC UA. 

**The Problem**:
- ChirpStack manages LoRaWAN devices but doesn't speak industrial protocols
- SCADA/MES systems expect OPC UA but don't understand LoRaWAN
- Building custom integrations is time-consuming and fragile

**The Solution**:
```
ChirpStack Server (LoRaWAN) ──→ opcgw Gateway ──→ OPC UA Clients (SCADA/MES)
   (gRPC polling)                (Rust, async)      (Ignition, KEPServerEx, etc.)
```

## Key Features

✨ **Real-Time Data Collection**
- Polls device metrics from ChirpStack at configurable intervals
- Supports multiple applications and hundreds of devices
- Handles network failures with automatic retries

🏭 **OPC UA Server**
- OPC UA 1.04 compliant server
- Dynamically builds address space from configuration
- Support for Float, Int, Bool, and String metrics
- Compatible with any standard OPC UA client

🔐 **Enterprise-Grade**
- Configuration validation on startup with clear error messages
- Environment variable credential management (no hardcoded secrets)
- Structured logging (tokio-tracing) for operational visibility
- Graceful shutdown handling
- Comprehensive error handling (no panics in production code)

🐳 **Container-Native**
- Official Docker image with multi-stage build
- Docker Compose for quick local development
- Kubernetes-ready with health checks

## Installation

### From Source (Rust 1.94.0+)

```bash
git clone https://github.com/guycorbaz/opcgw.git
cd opcgw
cargo build --release
./target/release/opcgw -c config/config.toml
```

### Via Docker

```bash
docker compose up
# Or use pre-built image from GitHub Container Registry
docker run ghcr.io/guycorbaz/opcgw:2.0.0
```

## Configuration

opcgw uses a single TOML configuration file:

```toml
[chirpstack]
server_address = "http://chirpstack.local:8080"
api_token = "your-api-token"
tenant_id = "your-tenant-id"
polling_frequency = 10

[opcua]
application_name = "My IoT Gateway"
host_port = 4855
user_name = "admin"
user_password = "secure-password"

[[application]]
application_name = "Farm Sensors"
application_id = "1"

[[application.device]]
device_name = "Field A"
device_id = "sensor_001"

[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "soil_moisture"
metric_type = "Float"
metric_unit = "%"
```

**For complete configuration details**, see the [Configuration Reference](https://guycorbaz.github.io/opcgw/configuration/).

> 🔐 **Secrets:** the shipped `config/config.toml` ships with placeholder
> values for `api_token` and `user_password`. The gateway refuses to
> start with the placeholders in place — inject the real values via the
> `OPCGW_CHIRPSTACK__API_TOKEN` and `OPCGW_OPCUA__USER_PASSWORD` env vars
> (or via the `.env` file consumed by Docker Compose). See
> [`docs/security.md`](./docs/security.md) for the full env-var
> convention, Docker / Kubernetes recipes, and the migration path for
> existing deployments.
>
> 🛡️ **OPC UA security:** for production OPC UA setup (endpoint matrix,
> PKI layout, `0o600` private-key requirement, audit-trail recipe,
> anti-patterns), see
> [`docs/security.md#opc-ua-security-endpoints-and-authentication`](./docs/security.md#opc-ua-security-endpoints-and-authentication).
> For OPC UA session-cap sizing (`max_connections` knob, the
> `event="opcua_session_count"` gauge, and the at-limit warn), see
> [`docs/security.md#opc-ua-connection-limiting`](./docs/security.md#opc-ua-connection-limiting).
> For subscription / message-size limits (`max_subscriptions_per_session`,
> `max_monitored_items_per_sub`, `max_message_size`, `max_chunk_count`)
> and the `DataChangeFilter` contract for stale-status notifications,
> see [`docs/security.md#subscription-and-message-size-limits`](./docs/security.md#subscription-and-message-size-limits).
> For OPC UA `HistoryRead` (FR22) — `[storage].retention_days` floor /
> hard-cap, the `[opcua].max_history_data_results_per_node` per-call cap,
> the manual-paging recipe (continuation points are not implemented), and
> NFR15's <2s 7-day query budget — see
> [`docs/security.md#historical-data-access`](./docs/security.md#historical-data-access).

## Planning

**Current Version:** 2.0.0 — last updated 2026-04-30 (Stories 8-1 / 8-2 / 8-3 done; Epic 8 retrospective pending).

The roadmap is tracked in [`_bmad-output/implementation-artifacts/sprint-status.yaml`](./_bmad-output/implementation-artifacts/sprint-status.yaml). The table below mirrors the current state of every epic; story-level detail lives in the sprint status file and the per-story documents under `_bmad-output/implementation-artifacts/`.

| Epic | Status | Scope |
|------|--------|-------|
| **Epic 1 — Crash-Free Gateway Foundation** | ✅ done | Dependency refresh + Rust 1.94, `log4rs → tracing` migration, comprehensive error handling, graceful shutdown via `CancellationToken`, configuration validation. |
| **Epic 2 — Data Persistence** | ✅ done | `StorageBackend` trait, SQLite backend with WAL mode + per-task connection pool, batch writes, append-only history table, startup restore, graceful degradation, retention pruning. |
| **Epic 3 — Reliable Command Execution** | ✅ done | SQLite-backed FIFO command queue, parameter validation, command-delivery status reporting (sent / confirmed / failed / timed-out). |
| **Epic 4 — Scalable Data Collection** | ✅ done (4-4 deferred to Phase B) | Poller refactored onto `StorageBackend`, support for all ChirpStack metric types, gRPC pagination. Story 4-4 (auto-recovery from ChirpStack outages) is deferred to a Phase B resilience epic. |
| **Epic 5 — Operational Visibility** | ✅ done | OPC UA server refactored onto SQLite backend, stale-data detection with OPC UA `Good`/`Uncertain`/`Bad` status codes, gateway health metrics (last poll timestamp, error count, ChirpStack availability) exposed under the `Gateway` folder. |
| **Epic 6 — Production Observability & Diagnostics** | ✅ done | **6-1 done** (structured logging, correlation IDs on every OPC UA read, staleness-transition logs, poller-cycle structured logs, storage-query timing, configurable log directory via `OPCGW_LOG_DIR` and `[logging].dir`); **6-2 done** (configurable log verbosity via `OPCGW_LOG_LEVEL` and `[logging].level`); **6-3 done** (microsecond UTC timestamps; performance-budget warnings on `opc_ua_read`/`storage_query`/`batch_write`; data-anomaly logs — NULL `last_poll_timestamp`, staleness-boundary, error-count spike; ChirpStack `chirpstack_connect` / `chirpstack_outage` / `retry_schedule` diagnostics; edge-case logs — `gateway_status_init`, `chirpstack_request` timeout, `metric_parse`; transient-failure logs — `device_poll`, SQLITE_BUSY (with `sqlite_error_code` sibling field for differentiating BUSY/LOCKED); end-to-end `request_id` correlation verified via integration test; expanded operations reference + symptom cookbook in `docs/logging.md`. Code review complete in 3 iterations: clippy-clean across the workspace, `Mutex<HashMap>` staleness cache replaced with `DashMap` for lock-free concurrent reads, 5 helpers extracted (`maybe_emit_error_spike`, `maybe_emit_chirpstack_outage`, `validate_bool_metric_value`, `classify_and_log_grpc_error`, `format_last_successful_poll`) so synthetic tests now drive production paths; 188 lib + 209 bin + 79 integration tests pass). |
| **Epic 7 — Security Hardening** | ✅ done | **7-1 done** — sanitized `config/config.toml` with `REPLACE_ME_WITH_*` placeholders + placeholder-detection in `validate()`, hand-written redacting `Debug` impls on `ChirpstackPollerConfig` / `OpcUaConfig`, `secrets_not_logged_when_full_config_debug_formatted` regression test, tonic 0.14.5 metadata-leak audit (clean — no `EnvFilter` mitigation needed today), `.env.example` + Compose recipe with `:?err` guards, `docs/security.md` with reversible-migration alternative, scrubbed `config/config.example.toml` (synthetic IDs only). **7-2 done** — `OPCUA_USER_TOKEN_ID = "default-user"` constant replacing the hardcoded `"user1"` token id (4 call sites), custom `OpcgwAuthManager` (`src/opc_ua_auth.rs`) implementing async-opcua's `AuthManager` trait with **HMAC-SHA-256-keyed credential digests** for fully-constant-time comparison (no length oracle) and sanitised-username `event="opcua_auth_failed"` audit logging (NFR12 via two-event correlation with async-opcua's accept event); `validate_private_key_permissions` (NFR9 — `0o600` file + `0o700` parent dir enforced at startup with both-violation accumulation, fail-closed) and `ensure_pki_directories` (FR45 — auto-creates `own/`, `private/`, `trusted/`, `rejected/` race-free via atomic `DirBuilder::mode()`) in new `src/security.rs` module; path-traversal guards rejecting absolute / `..` `private_key_path` and empty `pki_dir`; trim checks on `user_name` / `user_password` for copy-paste-from-`.env` resilience; shipped `create_sample_keypair = false` in both `config/config.toml` and `config/config.example.toml`, release-build warning when the flag is `true`; integration tests pinning the three endpoints (None / Basic256 Sign / Basic256 SignAndEncrypt) with line-scoped audit-event assertions and the wrong-password rejection path; smoke-test client at `examples/opcua_client_smoke.rs`; `async-opcua-client` moved to `[dev-dependencies]` to keep the production binary lean; extended `docs/security.md` with endpoint matrix + PKI layout + audit-trail recipe + log-level-required-for-NFR12 hard-statement + create-sample-keypair regen anti-pattern + Story-7-1 migration path. Code review closed all HIGH/MEDIUM findings over three iterations. **7-3 done** — `[opcua].max_connections: Option<usize>` config knob (default 10, hard cap 4096) wired through `ServerBuilder::max_sessions(N)` for FR44; `OPCGW_OPCUA__MAX_CONNECTIONS` env-var override; new `src/opc_ua_session_monitor.rs` module exposing a periodic `info!(event="opcua_session_count", current, limit)` gauge (5s tick) and an `AtLimitAcceptLayer` tracing-Layer that emits `warn!(event="opcua_session_count_at_limit", source_ip, limit, current)` correlated against async-opcua's `Accept new connection from {addr}` event (NFR12 two-event pattern); 8 unit tests + 4 integration tests; 581 tests / 0 fail / 7 ignored, clippy clean; extended `docs/security.md` with `## OPC UA connection limiting` section. **Epic 7 retrospective complete (2026-04-29)** — Phase A security baseline (FR42, FR44, FR45, NFR7-9, NFR12, NFR24) satisfied. Decided next steps before `bmad-create-story 8-1`: NFR12 silent-degradation startup-warn in `src/main.rs::initialise_tracing` (~5 LOC) + Epic 8 spec polish in `epics.md` (incorporates per-IP throttling #88, message-size / monitored-item limits #89, gauge tunability, NFR12 logging hard-statement). Eight follow-up GitHub issues (#82, #83, #85–#90) carry forward; none block Phase B entry. Doctest baseline (56 pre-existing failures) escalates to a defined cleanup story before Epic 9. See [`epic-7-retro-2026-04-29.md`](./_bmad-output/implementation-artifacts/epic-7-retro-2026-04-29.md). |
| **Epic 8 — Real-Time Subscriptions & Historical Data (Phase B)** | 🔄 in-progress (8-1 / 8-2 / 8-3 done; epic retrospective pending) | **8-1 / 8-2 / 8-3 done.** **8-3 done (2026-04-30 after iter-1 + iter-2 code review)** — OPC UA `HistoryRead` (FR22) end-to-end: new `StorageBackend::query_metric_history` method on `SqliteBackend` + `InMemoryBackend` with half-open `[start, end)` interval semantics (start inclusive, end exclusive — matches OPC UA Part 11 §6.4), microsecond-precision UTC timestamps, partial-success on bad rows (NaN / unknown data_type silently skipped with `trace!`); new `src/opc_ua_history.rs` module wrapping async-opcua's `SimpleNodeManagerImpl` and overriding `history_read_raw_modified` (the wrap-don't-subclass pattern documented in Story 8-1's spike report); reverse-lookup `NodeId → (device_id, metric_name)` map built once at server-construction time; metric variables now carry `AccessLevel::HISTORY_READ` + `historizing = true`; new `[storage].retention_days` validation (FR22 floor of 7 days, hard cap 365) + new `[opcua].max_history_data_results_per_node` per-call cap (default 10000, hard cap 1_000_000) — both wired through env-var overrides; the configured retention is now written to `retention_config` at startup via `INSERT OR REPLACE` (was migration-default 90 days, now operator-config-driven); **continuation points NOT implemented** (manual-paging contract documented in `docs/security.md`); NFR15 release-build benchmark in `tests/opcua_history_bench.rs` (`#[ignore]` by default; targets 600k-row 7-day query <2s); NFR12 carry-forward intact (zero new audit events, src/opc_ua_auth.rs / src/opc_ua_session_monitor.rs production code unchanged); 11 unit tests on `query_metric_history` + 11 config-validation tests + 5 integration tests on the HistoryRead pipeline. See [`8-3-historical-data-access-via-opc-ua.md`](./_bmad-output/implementation-artifacts/8-3-historical-data-access-via-opc-ua.md) for the spec; the new docs section is `docs/security.md#historical-data-access`. **8-4** (threshold-based alarm conditions, FR23) still backlog. |
| **Epic 9 — Web Configuration & Hot-Reload (Phase B)** | 📋 backlog | Axum web server + basic auth, gateway status dashboard, live metric values, application/device/metric/command CRUD via web UI, configuration hot-reload, dynamic OPC UA address-space mutation. |

### How to read this section

- **Status legend:** ✅ done · 🔄 in-progress · 📋 backlog (and 📝 ready-for-dev / 👀 review for individual stories).
- **Phase A** covers Epics 1–7 — production hardening of the existing one-way (read) gateway.
- **Phase B** covers Epics 8–9 — adds real-time subscriptions, historical data access, and a web admin surface. Story 4-4 is deferred to a Phase B resilience epic.
- For the canonical, machine-readable view, see [`sprint-status.yaml`](./_bmad-output/implementation-artifacts/sprint-status.yaml). The sprint-status file is the source of truth; this table is updated alongside it.
- Per-story details, acceptance criteria, dev notes, and review findings live in `_bmad-output/implementation-artifacts/<epic>-<story>-<slug>.md`.

A long-form roadmap with marketing-friendly language is available at [Roadmap](https://guycorbaz.github.io/opcgw/features/#roadmap).

## Use Cases

- 🌱 **Smart Agriculture**: Monitor soil conditions across farms via wireless sensors
- 🏭 **Industrial IoT**: Asset tracking and equipment monitoring
- 🌍 **Environmental Monitoring**: Air quality, weather stations, environmental sensors
- 🏢 **Building Automation**: HVAC, occupancy, energy management
- ⚡ **Renewable Energy**: Solar + battery microgrid optimization

→ See [Real-World Use Cases](https://guycorbaz.github.io/opcgw/usecases/) for detailed scenarios.

## Logging

opcgw is built on `tracing` with per-module file appenders and a stderr console layer. The global verbosity is configurable at runtime — no rebuild required.

```bash
# Set verbosity for a single run
OPCGW_LOG_LEVEL=debug ./target/release/opcgw

# Or persist in config.toml
[logging]
level = "debug"
dir = "/var/log/opcgw"
```

Valid levels: `trace`, `debug`, `info` (default), `warn`, `error`. Per-module file appenders capture independently of the global level — see [`docs/logging.md`](./docs/logging.md) for the operator-facing reference, including the structured-field schema, correlation-ID tracing, and the env-var override convention.

## Architecture

opcgw consists of two main components running concurrently:

- **ChirpStack Poller**: Polls device metrics from ChirpStack via gRPC at configurable intervals
- **OPC UA Server**: Exposes collected metrics as OPC UA variables for industrial clients

Both components share thread-safe in-memory storage via `Arc<Mutex<Storage>>`.

→ [See full architecture](https://guycorbaz.github.io/opcgw/architecture/)

## Technology Stack

- **Language**: Rust 1.94.0+ with async/await
- **Protocols**: gRPC for ChirpStack, OPC UA 1.04 for industrial clients
- **Storage**: In-memory HashMap (v2.0) with SQLite persistence planned
- **Logging**: Tokio-tracing with structured fields and per-module log files
- **Async Runtime**: Tokio for high-performance I/O
- **Build**: Multi-stage Docker build for minimal image size

## Contributing

Contributions are welcome! Please:

1. Check [existing issues](https://github.com/guycorbaz/opcgw/issues) first
2. Open an issue to discuss your idea before implementing
3. Follow the code style and conventions in CLAUDE.md
4. Ensure tests pass: `cargo test && cargo clippy`
5. Submit a pull request with a clear description

## Development

```bash
# Build and test
cargo build --release
cargo test
cargo clippy

# Run with debug logging
RUST_LOG=debug cargo run -c config/config.toml

# Watch logs
tail -f log/*.log
```

## License

Licensed under either MIT or Apache-2.0 at your option.

---

## Support

- 📖 [Documentation](https://guycorbaz.github.io/opcgw/)
- 🐛 [Issues](https://github.com/guycorbaz/opcgw/issues)
- 💬 [Discussions](https://github.com/guycorbaz/opcgw/discussions)
- 📧 Contact: gcorbaz@gmail.com

## Contributing

Any contributions you make are greatly appreciated. If you identify any errors,
or have an idea for an improvement, please open an [issue](https://github.com/guycorbaz/opcgw/issues).
But before filing a new issue, please look through already existing issues. Search open and closed issues first.

Non-code contributions are also highly appreciated, such as improving the documentation
or promoting opcgw on social media.


## License

MIT OR Apache-2.0.

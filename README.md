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
> For the embedded **Web UI** (Story 9-1; FR50, NFR11, NFR12, FR41) — the
> `[web]` config block (port / bind_address / auth_realm / enabled —
> defaults to off), Basic auth shared with `[opcua]` credentials, the
> `event="web_auth_failed"` audit event with direct source-IP via Axum's
> `ConnectInfo`, and the TLS-via-reverse-proxy stance — see
> [`docs/security.md#web-ui-authentication`](./docs/security.md#web-ui-authentication).

### Web UI (Story 9-1, opt-in)

Story 9-1 ships an embedded Axum web server gated by HTTP Basic auth.
**Off by default.** To enable, add the `[web]` block (or set the
`OPCGW_WEB__*` env vars):

```toml
[web]
enabled = true              # default false
port = 8080                 # default 8080; range 1024-65535
bind_address = "0.0.0.0"    # default "0.0.0.0"
auth_realm = "opcgw"        # default "opcgw"; max 64 chars
```

Credentials are **shared with `[opcua]`** (same `user_name` /
`user_password`); one rotation step covers both auth surfaces. HTTP-only
— deploy a reverse proxy (nginx, Caddy, Traefik) for TLS termination if
your environment requires it. See
[`docs/security.md#web-ui-authentication`](./docs/security.md#web-ui-authentication).

**Story 9-2 ships the gateway status dashboard.** Once enabled, browse
to `http://<gateway-host>:8080/` for a single-screen view of ChirpStack
connection state, last-poll timestamp, cumulative error count, and
configured application + device counts. The dashboard auto-refreshes
every 10 seconds (hard-coded in `static/dashboard.js` — edit the file
to change the cadence). Mobile-responsive layout (single-column ≤ 600 px,
two-column above) and OS-driven dark mode (`prefers-color-scheme: dark`)
ship out of the box; no in-page toggle. The underlying JSON shape is
exposed as `GET /api/status` for `curl | jq` / Prometheus textfile
exporters / custom dashboards (auth-gated identically to the HTML).

**Story 9-3 ships the live metric values page.** Browse to
`http://<gateway-host>:8080/metrics.html` for the per-application,
per-device grid of current metric values. Each row shows the metric
name, value, data type, last-updated timestamp, and a staleness badge
(`Good` / `Uncertain` / `Bad` / `Missing`) computed from the
`[opcua].stale_threshold_seconds` knob (default 120 s) and the
hard-coded 24 h "bad" cutoff. Configured-but-never-polled metrics
appear as "Missing" rather than being silently omitted — operators
spot mis-configured devices at a glance. Same 10 s refresh + responsive
layout + dark mode as the dashboard. JSON contract at `GET /api/devices`.

## Planning

> ### ⚠️ Production-deployment blocker
>
> **GitHub issue [#108](https://github.com/guycorbaz/opcgw/issues/108)** — surfaced during Story 9-3 code review (2026-05-03) — flags a pre-existing project-wide storage bug: the `MetricType` enum is payload-less, so every row in `metric_values` has `value == data_type` (literally the string `"Float"` / `"Int"` / `"Bool"` / `"String"`) instead of the actual measurement. **This means opcgw has never persisted real metric values.** Affects 4 epics (2, 5, 8, 9-3); SCADA clients see literal type-name strings via OPC UA, dashboards show `"Float"` instead of `23.5`, HistoryRead returns type-strings.
>
> **Fix is an Epic-1-scale storage-trait refactor** (provisionally tracked as "Epic A — Storage Payload Migration"). Until #108 lands, **opcgw is suitable for device-presence monitoring only** ("is the sensor reporting?") — not for actual measurement collection.
>
> Epic 9 retrospective is blocked on #108. See `_bmad-output/implementation-artifacts/sprint-status.yaml` for the formal annotation.

**Current Version:** 2.0.0 — last updated 2026-05-06 (Story 9-7 done — Configuration Hot-Reload (FR39 + FR40) shipped through 3 code-review iterations, terminated under CLAUDE.md condition #2 (only LOW remains). Iter-1: 22 patches (5 HIGH + 11 MED + 5 LOW + 3 from 5 decisions resolved). Iter-2: caught 2 HIGH-REGRESSIONs in iter-1's own patches (P25 tautological subscription test, P26 topology_device_diff/classify_diff disagreement on `device_command_list`) + 3 HIGH (None≡Some([]), destructure landmines on `chirpstack_equal`/`opcua_equal`, NaN-safe `device_schemas_equal` via `to_bits()`) + 9 MED; 11 applied. Iter-3: caught 1 MED (P42 `param_type_equal` `_ => false` → exhaustive cross-variant pairs) + 9 LOW; P42 applied. Final: 876 passed / 0 failed; clippy clean; AC#7 grep contract = 3 lines; AC#8 file invariants intact. GH issues filed: #112 (tracker), #113 (live-borrow refactor for AC#10 + credential rotation), #114/#115/#116 (per-section future hot-reload). 5 v1 limitations backfilled to deferred-work.md per AC#11. Implementation surface: new `src/config_reload.rs` module with `ConfigReloadHandle` owning a `tokio::sync::watch::Sender<Arc<AppConfig>>`, validate-then-swap discipline, knob-taxonomy classifier; 3 new audit events `event="config_reload_attempted/succeeded/failed"`; SIGHUP listener in `src/main.rs`; poller `config_rx` outer-loop arm honouring Story 4-4 read-at-entry semantics; web `AppState` atomic swap of `dashboard_snapshot` + `stale_threshold_secs`; OPC UA listener stub logging `topology_change_detected` for Story 9-8. AC#8 invariants honoured: zero changes to `src/web/auth.rs`, `src/opc_ua_*.rs`, `src/security*.rs`, `src/main.rs::initialise_tracing`. v1 limitations documented in `docs/security.md § Configuration hot-reload`: credential rotation restart-required, OPC UA-side stale-threshold + address-space mutation deferred to follow-up stories, no HTTP trigger / filesystem watch. 19 integration tests in `tests/config_hot_reload.rs` + 6 unit tests in `src/config_reload.rs::tests` + 1 poller hot-reload unit + 3 clamp-helper unit tests. Previous narrative — Story 4-4 done — Phase A carry-forward auto-recovery from ChirpStack outages closes FR6/7/8 + NFR17 30s SLA; loop terminated under CLAUDE.md condition 1 after iter-3 code review. Iter-3 (over-reviewing pass) surfaced 1 HIGH + 4 MED/LOW that iter-1/iter-2 missed: P14 tightened shipped `config/config.toml` from `retry=10, delay=10` (worst-case 100s — violated AC#4) to `retry=30, delay=1` (worst-case 30s — satisfies AC#4); P10 preserves original gRPC outage cause separate from last probe error in cancel branch; P11 clarified retry/attempt semantics in `docs/logging.md`; P12 updated `chirpstack_outage` operator runbook; P13 replaced fragile field-order-coupled test assertion with token-boundary scan. cargo test 837 passed / 0 failed / 8 ignored across 17 suites; clippy clean. Epic 4 returned to done. Previous narrative — Story 4-4 review — Phase A carry-forward auto-recovery from ChirpStack outages; closes FR6/7/8 + NFR17 30s SLA. New `recover_from_chirpstack_outage` helper layered on Story 6-3's `chirpstack_outage` warn; 3 reserved log operations promoted to implemented (`recovery_attempt` / `recovery_complete` / `recovery_failed`); `Channel::connect()` 5s timeout wrap (resolves deferred-work.md:86 6-3 carry-forward); 7 new unit tests. Previous narrative — Story 9-0 done — async-opcua runtime address-space mutation spike, code-review iter-1 + iter-2 complete; loop terminated per CLAUDE.md condition 3. Three load-bearing questions resolved empirically: Q1 add path RESOLVED FAVOURABLY, Q2 remove path Behaviour B (frozen-last-good — 9-8 must arrange explicit cleanup), Q3 sibling isolation RESOLVED FAVOURABLY (write-lock-hold = 117 µs for 11 mutations). `OpcUa::run` split into `build` + `run_handles` to give 9-7 hot-reload a clean integration seam. Spike report at `_bmad-output/implementation-artifacts/9-0-spike-report.md`. Code review applied 10 iter-1 patches + 6 iter-2 patches (test-side hardening + spec/spike-report doc reconciliation); P1 (RunHandles missing Drop impl) blocked by rustc E0509 and tracked at GitHub KF issue #110 for Story 9-7 to revisit. Previous: Story 9-3 done after iter-1 + iter-2 code review; live metric values page at /metrics.html with per-row staleness badges; #107 duplicate-metric_name validation gap and #108 payload-less MetricType which BLOCKS production deployment until storage trait refactor lands.).

The roadmap is tracked in [`_bmad-output/implementation-artifacts/sprint-status.yaml`](./_bmad-output/implementation-artifacts/sprint-status.yaml). The table below mirrors the current state of every epic; story-level detail lives in the sprint status file and the per-story documents under `_bmad-output/implementation-artifacts/`.

| Epic | Status | Scope |
|------|--------|-------|
| **Epic 1 — Crash-Free Gateway Foundation** | ✅ done | Dependency refresh + Rust 1.94, `log4rs → tracing` migration, comprehensive error handling, graceful shutdown via `CancellationToken`, configuration validation. |
| **Epic 2 — Data Persistence** | ✅ done | `StorageBackend` trait, SQLite backend with WAL mode + per-task connection pool, batch writes, append-only history table, startup restore, graceful degradation, retention pruning. |
| **Epic 3 — Reliable Command Execution** | ✅ done | SQLite-backed FIFO command queue, parameter validation, command-delivery status reporting (sent / confirmed / failed / timed-out). |
| **Epic 4 — Scalable Data Collection** | ✅ done | Poller refactored onto `StorageBackend`, support for all ChirpStack metric types, gRPC pagination. **4-4 done (2026-05-06 — auto-recovery from ChirpStack outages; Phase A carry-forward closed after iter-3 code review tightened shipped `config/config.toml` defaults from `retry=10, delay=10` to `retry=30, delay=1` to satisfy AC#4's `retry × delay ≤ 30s` clause + 4 supporting patches).** Closes PRD FR6 (TCP connectivity check), FR7 (auto-reconnect without manual intervention), FR8 (configurable retry count + delay), and NFR17 (30s auto-recovery SLA). New `recover_from_chirpstack_outage` helper on `ChirpstackPoller` (~120 LOC at `src/chirpstack.rs`) layered on top of the existing `chirpstack_outage` warn (Story 6-3): reads `chirpstack.retry` + `chirpstack.delay` at loop entry, calls `update_gateway_status(None, error_count, false)` to surface the outage to OPC UA (Story 5-3) + web dashboard (Story 9-2), then probes `check_server_availability` up to R times with cancel-token-paired `tokio::time::sleep` budget (Ctrl+C aborts cleanly). Three reserved log operations from `docs/logging.md:240-242` promoted to implemented: `recovery_attempt` (info per attempt), `recovery_complete` (info on success with `downtime_secs` from `last_successful_poll` math, or `from_startup=true` on cold start), `recovery_failed` (warn on budget exhaustion). Picked up the 6-3 carry-forward from `deferred-work.md:86`: `Channel::connect()` now wrapped in a 5s `tokio::time::timeout` (smaller than NFR17's 30s SLA, larger than the TCP probe's 1s timeout). 7 new unit tests in `src/chirpstack.rs::tests::recovery_*` covering AC#1 (loop fires + emits operations), AC#2 (retry count + delay), AC#3 (gateway_status update with `chirpstack_available=false` + `error_count` propagation), AC#5 (cancel-safety), AC#6 (downtime_secs + cold-start `from_startup=true`). Existing Story 5-2 stale-data semantics + Story 7-2/7-3/8-3/9-0/9-1/9-2/9-3 invariants preserved (zero changes to `src/web/`, `static/`, `tests/web_*.rs`, `src/opc_ua_*.rs`). |
| **Epic 5 — Operational Visibility** | ✅ done | OPC UA server refactored onto SQLite backend, stale-data detection with OPC UA `Good`/`Uncertain`/`Bad` status codes, gateway health metrics (last poll timestamp, error count, ChirpStack availability) exposed under the `Gateway` folder. |
| **Epic 6 — Production Observability & Diagnostics** | ✅ done | **6-1 done** (structured logging, correlation IDs on every OPC UA read, staleness-transition logs, poller-cycle structured logs, storage-query timing, configurable log directory via `OPCGW_LOG_DIR` and `[logging].dir`); **6-2 done** (configurable log verbosity via `OPCGW_LOG_LEVEL` and `[logging].level`); **6-3 done** (microsecond UTC timestamps; performance-budget warnings on `opc_ua_read`/`storage_query`/`batch_write`; data-anomaly logs — NULL `last_poll_timestamp`, staleness-boundary, error-count spike; ChirpStack `chirpstack_connect` / `chirpstack_outage` / `retry_schedule` diagnostics; edge-case logs — `gateway_status_init`, `chirpstack_request` timeout, `metric_parse`; transient-failure logs — `device_poll`, SQLITE_BUSY (with `sqlite_error_code` sibling field for differentiating BUSY/LOCKED); end-to-end `request_id` correlation verified via integration test; expanded operations reference + symptom cookbook in `docs/logging.md`. Code review complete in 3 iterations: clippy-clean across the workspace, `Mutex<HashMap>` staleness cache replaced with `DashMap` for lock-free concurrent reads, 5 helpers extracted (`maybe_emit_error_spike`, `maybe_emit_chirpstack_outage`, `validate_bool_metric_value`, `classify_and_log_grpc_error`, `format_last_successful_poll`) so synthetic tests now drive production paths; 188 lib + 209 bin + 79 integration tests pass). |
| **Epic 7 — Security Hardening** | ✅ done | **7-1 done** — sanitized `config/config.toml` with `REPLACE_ME_WITH_*` placeholders + placeholder-detection in `validate()`, hand-written redacting `Debug` impls on `ChirpstackPollerConfig` / `OpcUaConfig`, `secrets_not_logged_when_full_config_debug_formatted` regression test, tonic 0.14.5 metadata-leak audit (clean — no `EnvFilter` mitigation needed today), `.env.example` + Compose recipe with `:?err` guards, `docs/security.md` with reversible-migration alternative, scrubbed `config/config.example.toml` (synthetic IDs only). **7-2 done** — `OPCUA_USER_TOKEN_ID = "default-user"` constant replacing the hardcoded `"user1"` token id (4 call sites), custom `OpcgwAuthManager` (`src/opc_ua_auth.rs`) implementing async-opcua's `AuthManager` trait with **HMAC-SHA-256-keyed credential digests** for fully-constant-time comparison (no length oracle) and sanitised-username `event="opcua_auth_failed"` audit logging (NFR12 via two-event correlation with async-opcua's accept event); `validate_private_key_permissions` (NFR9 — `0o600` file + `0o700` parent dir enforced at startup with both-violation accumulation, fail-closed) and `ensure_pki_directories` (FR45 — auto-creates `own/`, `private/`, `trusted/`, `rejected/` race-free via atomic `DirBuilder::mode()`) in new `src/security.rs` module; path-traversal guards rejecting absolute / `..` `private_key_path` and empty `pki_dir`; trim checks on `user_name` / `user_password` for copy-paste-from-`.env` resilience; shipped `create_sample_keypair = false` in both `config/config.toml` and `config/config.example.toml`, release-build warning when the flag is `true`; integration tests pinning the three endpoints (None / Basic256 Sign / Basic256 SignAndEncrypt) with line-scoped audit-event assertions and the wrong-password rejection path; smoke-test client at `examples/opcua_client_smoke.rs`; `async-opcua-client` moved to `[dev-dependencies]` to keep the production binary lean; extended `docs/security.md` with endpoint matrix + PKI layout + audit-trail recipe + log-level-required-for-NFR12 hard-statement + create-sample-keypair regen anti-pattern + Story-7-1 migration path. Code review closed all HIGH/MEDIUM findings over three iterations. **7-3 done** — `[opcua].max_connections: Option<usize>` config knob (default 10, hard cap 4096) wired through `ServerBuilder::max_sessions(N)` for FR44; `OPCGW_OPCUA__MAX_CONNECTIONS` env-var override; new `src/opc_ua_session_monitor.rs` module exposing a periodic `info!(event="opcua_session_count", current, limit)` gauge (5s tick) and an `AtLimitAcceptLayer` tracing-Layer that emits `warn!(event="opcua_session_count_at_limit", source_ip, limit, current)` correlated against async-opcua's `Accept new connection from {addr}` event (NFR12 two-event pattern); 8 unit tests + 4 integration tests; 581 tests / 0 fail / 7 ignored, clippy clean; extended `docs/security.md` with `## OPC UA connection limiting` section. **Epic 7 retrospective complete (2026-04-29)** — Phase A security baseline (FR42, FR44, FR45, NFR7-9, NFR12, NFR24) satisfied. Decided next steps before `bmad-create-story 8-1`: NFR12 silent-degradation startup-warn in `src/main.rs::initialise_tracing` (~5 LOC) + Epic 8 spec polish in `epics.md` (incorporates per-IP throttling #88, message-size / monitored-item limits #89, gauge tunability, NFR12 logging hard-statement). Eight follow-up GitHub issues (#82, #83, #85–#90) carry forward; none block Phase B entry. Doctest baseline (56 pre-existing failures) escalates to a defined cleanup story before Epic 9. See [`epic-7-retro-2026-04-29.md`](./_bmad-output/implementation-artifacts/epic-7-retro-2026-04-29.md). |
| **Epic 8 — Real-Time Subscriptions & Historical Data (Phase B)** | ✅ done (8-1 / 8-2 / 8-3 shipped; 8-4 classified as Known Failure — see retro) | **Epic 8 retrospective complete (2026-05-01)** — Phase B subscription + historical-data baseline (FR21, FR22) satisfied. Story 8-4 (threshold-based alarm conditions, FR23) classified as Known Failure (KF) with operator-visible diagnostics treatment per Story 6-3 convention; SCADA-side alarm thresholds in FUXA / Ignition is the documented operator workaround. Key decided action items before Epic 9 starts: NodeId metric-name-only collision bug fix (HIGH severity, pre-existing latent bug 8-3 surfaced via HistoryRead), `test_concurrent_write_read_isolation` flake fix (`#[serial_test::serial]`), doctest cleanup story (BLOCKING — 4th epic in a row), spike-test productionisation, `tests/common/mod.rs` extraction, CLAUDE.md per-iteration commit-rule clarification. Carry-forward GH issues #88, #93, #94, #95, #98. See [`epic-8-retro-2026-05-01.md`](./_bmad-output/implementation-artifacts/epic-8-retro-2026-05-01.md). **8-1 / 8-2 / 8-3 done.** **8-3 done (2026-04-30 after iter-1 + iter-2 code review)** — OPC UA `HistoryRead` (FR22) end-to-end: new `StorageBackend::query_metric_history` method on `SqliteBackend` + `InMemoryBackend` with half-open `[start, end)` interval semantics (start inclusive, end exclusive — matches OPC UA Part 11 §6.4), microsecond-precision UTC timestamps, partial-success on bad rows (NaN / unknown data_type silently skipped with `trace!`); new `src/opc_ua_history.rs` module wrapping async-opcua's `SimpleNodeManagerImpl` and overriding `history_read_raw_modified` (the wrap-don't-subclass pattern documented in Story 8-1's spike report); reverse-lookup `NodeId → (device_id, metric_name)` map built once at server-construction time; metric variables now carry `AccessLevel::HISTORY_READ` + `historizing = true`; new `[storage].retention_days` validation (FR22 floor of 7 days, hard cap 365) + new `[opcua].max_history_data_results_per_node` per-call cap (default 10000, hard cap 1_000_000) — both wired through env-var overrides; the configured retention is now written to `retention_config` at startup via `INSERT OR REPLACE` (was migration-default 90 days, now operator-config-driven); **continuation points NOT implemented** (manual-paging contract documented in `docs/security.md`); NFR15 release-build benchmark in `tests/opcua_history_bench.rs` (`#[ignore]` by default; targets 600k-row 7-day query <2s); NFR12 carry-forward intact (zero new audit events, src/opc_ua_auth.rs / src/opc_ua_session_monitor.rs production code unchanged); 11 unit tests on `query_metric_history` + 11 config-validation tests + 5 integration tests on the HistoryRead pipeline. See [`8-3-historical-data-access-via-opc-ua.md`](./_bmad-output/implementation-artifacts/8-3-historical-data-access-via-opc-ua.md) for the spec; the new docs section is `docs/security.md#historical-data-access`. **8-4** (threshold-based alarm conditions, FR23) **classified as Known Failure** — see retro § Known Failures + `deferred-work.md`. |
| **Epic 9 — Web Configuration & Hot-Reload (Phase B)** | 🔄 in-progress (9-1 done · 9-2 done · 9-3 done · 9-0 done · **9-7 done**) | **9-7 done (2026-05-06 — 3 code-review iterations terminated under CLAUDE.md condition #2)** — Configuration Hot-Reload (FR39 + FR40). New `src/config_reload.rs` module owns the `tokio::sync::watch::Sender<Arc<AppConfig>>` channel, the `ConfigReloadHandle::reload()` routine (validate-then-swap discipline), and the knob-taxonomy classifier that distinguishes hot-reload-safe from restart-required from address-space-mutating changes. `src/main.rs` spawns a SIGHUP listener (3 new audit events: `event="config_reload_attempted"` info, `"config_reload_succeeded"` info with `changed_section_count` + `duration_ms`, `"config_reload_failed"` warn with `reason ∈ {validation, io, restart_required}` + `changed_knob`), a web-config-listener task that atomically swaps `AppState.dashboard_snapshot` + `AppState.stale_threshold_secs` (now `RwLock<Arc<...>>` + `AtomicU64`), and an OPC UA config-listener stub for Story 9-8 that logs `topology_change_detected` with device counts. Poller's outer-loop `tokio::select!` gains a `config_rx.changed()` arm; Story 4-4's recovery loop unaffected (read-at-entry semantics naturally pick up new `retry`/`delay` values). AC#8 invariants honoured: zero changes to `src/web/auth.rs`, `src/opc_ua.rs`, `src/opc_ua_auth.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_history.rs`, `src/security.rs`, `src/security_hmac.rs`, `src/main.rs::initialise_tracing`. v1 limitations explicitly documented in `docs/security.md § Configuration hot-reload`: (1) OPC UA address-space mutation stubbed (Story 9-8 territory — the dashboard updates but OPC UA stays frozen), (2) credential rotation restart-required (auth-middleware refactor deferred), (3) `[opcua].stale_threshold_seconds` hot-reload affects only the web dashboard (OPC UA per-variable closures captured at startup), (4) no HTTP trigger (Stories 9-4/9-5/9-6 will add CRUD-driven reload), (5) no filesystem watch. 13 new integration tests in `tests/config_hot_reload.rs` covering AC#1 (validation-first / atomic swap), AC#2 (hot-reload-safe propagation), AC#3 (restart-required rejection with `changed_knob`), AC#4 (dashboard reflection), AC#9 (loose-perm rejection + secret-hygiene), AC#10 (stale-threshold propagation), plus io-reason path and equal-config NoChange. 6 new `src/config_reload.rs::tests` unit tests + 1 new `src/chirpstack.rs::tests::poller_picks_up_new_retry_at_next_cycle` + 3 new clamp-helper unit tests. AC#7 grep contract: `git grep -hoE 'event = "config_reload_[a-z]+"' src/ | sort -u` returns exactly 3 lines. Issue #110 (RunHandles missing Drop impl) verdict: keep open — listener tasks all cooperate with `cancel_token` explicitly via `tokio::select!`, RAII drop would be redundant. Tracking issue #112. **9-1 done (2026-05-02 after iter-1 + iter-2 code review)** — Axum 0.8 embedded web server gated by HTTP Basic auth (FR50 / NFR11), `event="web_auth_failed"` warn-level audit event with **direct** source-IP via Axum's `ConnectInfo<SocketAddr>` (NFR12 — strict improvement over the OPC UA path's two-event correlation), `tower-http::services::ServeDir` static-file mount with placeholder HTML for Stories 9-2+ (FR41 mobile-responsive viewport tag in place), `[web].enabled = false` master switch (opt-in to avoid surprise listening port on upgrade), credentials shared with `[opcua]` (single source of truth — one rotation step covers both surfaces), HMAC-SHA-256 keyed credential digest extracted from Story 7-2's `OpcgwAuthManager` into `src/security_hmac.rs` (Phase-B carry-forward rule per `epics.md:782`), `[web]` config validation (port range 1024-65535, parseable bind address, realm ≤ 64 chars + no `"`), `OPCGW_WEB__*` env-var overrides via figment's nested-key convention, `CancellationToken`-driven graceful shutdown joining the existing `tokio::select!` sequence in `src/main.rs`, 5 web-config validation unit tests + 10 web-auth middleware unit tests + 7 web integration tests + 4 security_hmac unit tests, clippy clean, TLS-via-reverse-proxy explicitly out of scope (tracked at #104). **9-2 review (2026-05-03 — implementation complete, awaiting code review)** — Gateway Status Dashboard (FR38) + responsive HTML/CSS/JS dashboard (FR41 fully closed at the content level). New `AppState` struct lands the deferred-from-9-1 shape (auth + per-task `SqliteBackend` + frozen `DashboardConfigSnapshot` + `start_time`); new `GET /api/status` JSON endpoint reads `get_gateway_health_metrics()` and returns the 6-field dashboard payload (`chirpstack_available`, `last_poll_time`, `error_count`, `application_count`, `device_count`, `uptime_secs`); new `src/web/api.rs` module hosts the handler (lands on the directory slot `architecture.md:417-421` reserved for "Stories 9-2 onwards"); `static/index.html` replaced with a real 6-tile dashboard backed by `static/dashboard.css` (mobile-first responsive grid + `prefers-color-scheme: dark`) and `static/dashboard.js` (`fetch('/api/status')` on load + 10 s `setInterval` + inline error banner on 401/5xx); `tests/web_dashboard.rs` adds 4 integration tests (auth carry-forward + JSON shape pin + HTML markup + CSS responsive marker); 4 new unit tests for `api_status` (success / 500-with-generic-body / first-startup defaults / `null`-serialisation pin); 3 new unit tests for `DashboardConfigSnapshot::from_config`. Audit-event surface: ZERO new web-side events; ONE new diagnostic event `event="api_status_storage_error"` (warn) on storage-read failure. AC#6 file invariants honoured: `git diff` shows zero changes to `src/opc_ua*.rs` and to `src/web/auth.rs` production code (only `tests/web_auth.rs` fixture wraps `WebAuthState` in `AppState` for the new `build_router` signature). **9-0 done (2026-05-05 — async-opcua runtime address-space mutation spike, code-review iter-1 + iter-2 complete).** Three load-bearing questions from `epics.md:784-787` resolved empirically against async-opcua 0.17.1 under live subscriptions: **Q1 add path RESOLVED FAVOURABLY** (a runtime-added variable's first subscription notification carries the registered sentinel; the `SyncSampler` honours post-init `add_read_callback` registrations without restart), **Q2 remove path Behaviour B (frozen-last-good)** (deleting a variable while subscribed produces no observable client error — Story 9-8 must emit an explicit `BadNodeIdUnknown` via `manager.set_attributes` before `delete()`), **Q3 sibling isolation RESOLVED FAVOURABLY** (bulk mutation of 11 nodes under a single `address_space.write()` = 117 µs hold time, ~850× shorter than the 100 ms sampler tick — no risk of starvation at typical 100-1000-node-reload scales). Implementation: split `OpcUa::run` into `build` + `run_handles` + backward-compat `run` wrapper (Shape B per AC#5; the `RunHandles` struct is the integration seam Story 9-7 hot-reload will reuse). All existing call sites compile unchanged; lib + bins test count holds at 322 + 345 = 667 / 0 fail / 5 ignored after iter-1 + iter-2 patches. New: `tests/opcua_dynamic_address_space_spike.rs` (3 spike tests), `_bmad-output/implementation-artifacts/9-0-spike-report.md` (12-section architecture-reference doc). Known limitation surfaced: `SimpleNodeManagerImpl` does not expose `remove_read_callback`, so deleting a node leaks the closure — 9-8 must extend `OpcgwHistoryNodeManager` with a remove method or file an upstream FR (precedent: Story 8-1's session-rejected callback FR at issue #94). Code-review iter-1 applied 10 patches (P2-P11: spike test asserts hardened, spike report § 7 restructured to "Pattern reuse", AC#1/#2/#3 measurement substitutions ratified in spec AC Amendments + spike report addenda); P1 (RunHandles missing Drop impl) blocked by rustc E0509 and tracked at GitHub KF issue #110 for Story 9-7 evaluation. Iter-2 re-ran all three reviewer layers per CLAUDE.md and applied 6 follow-up patches (IP1-IP6: residual "~110 LOC" claim, 30s sanity ceiling on lock-hold, header arithmetic, `assert_eq!` sentinel-comparison hardening, doc-comment cleanup, deferred-work.md path bracket). Remaining stories: 9-7 hot-reload, 9-8 dynamic address space, 9-4/9-5/9-6 CRUD. |

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

---
layout: default
title: Development Roadmap
permalink: /roadmap/
---

## opcgw Development Roadmap

**Strategic Goal:** Deliver production-ready v2.0 with feature parity to v1.0 plus new capabilities (data persistence, real-time subscriptions, historical access, web UI, configuration hot-reload).

> **Principle:** Quality over Speed. v2.0 must be genuinely better than v1.0 — not just different.

> **Production-deployment blocker:** GitHub issue [#108](https://github.com/guycorbaz/opcgw/issues/108) — the `MetricType` enum is payload-less, so every row in `metric_values` stores the literal data-type string (`"Float"`, `"Int"`, …) instead of the actual measurement. opcgw has never persisted real metric values. Affects four shipped epics (2, 5, 8, 9-3); SCADA clients see literal type-name strings via OPC UA, dashboards show `"Float"` instead of `23.5`, HistoryRead returns type-strings. The fix is **Epic A — Storage Payload Migration**, an Epic-1-scale storage-trait refactor. Until #108 lands, opcgw is suitable for device-presence monitoring only ("is the sensor reporting?") — not for actual measurement collection.

---

## Timeline Overview

```
Phase A (Crash-free + persistence + commands + scalability + visibility + diagnostics + security)
  Epic 1 ✅ → Epic 2 ✅ → Epic 3 ✅ → Epic 4 ✅ → Epic 5 ✅ → Epic 6 ✅ → Epic 7 ✅

Phase B (Real-time subscriptions + historical + web UI + hot-reload + dynamic mutation)
  Epic 8 ✅ → Epic 9 ✅

Next (Production-deployment blocker)
  Epic A — Storage Payload Migration ⏳ (issue #108)
```

All 9 in-plan epics are complete. **Epic A is the immediate next epic** per the Epic 9 retrospective action item AI6 and is the gating work before production deployment.

---

## Phase A — Production Foundation ✅

### Epic 1: Crash-Free Gateway Foundation ✅

- Dependency refresh + Rust 1.94
- `log4rs → tracing` migration with structured fields
- Comprehensive error handling (15 production panic sites eliminated)
- Graceful shutdown via `CancellationToken` (SIGINT + SIGTERM, Docker-safe)
- Configuration validation with field-level error reporting

### Epic 2: Data Persistence ✅

- `StorageBackend` trait + `InMemoryBackend` + `SqliteBackend` (WAL mode, per-task connection pool)
- Hierarchical schema: applications → devices → metrics
- Batch writes, append-only history table
- Metric restore on startup
- Graceful degradation on storage errors
- Configurable retention pruning

### Epic 3: Reliable Command Execution ✅

- SQLite-backed FIFO command queue
- Parameter validation before transmission
- Command-delivery status reporting (`sent` / `confirmed` / `failed` / `timed-out`)

### Epic 4: Scalable Data Collection ✅

- Poller refactored onto `StorageBackend`
- Support for all ChirpStack metric types (Gauge, Counter, Absolute, Unknown)
- gRPC pagination for large deployments (100+ devices)
- Auto-recovery from ChirpStack outages with 30 s SLA (FR6 / FR7 / FR8 / NFR17)

### Epic 5: Operational Visibility ✅

- OPC UA server refactored onto SQLite backend
- Stale-data detection with `Good` / `Uncertain` / `Bad` OPC UA status codes
- Gateway health metrics under the `Gateway` folder (last poll, error count, ChirpStack availability)

### Epic 6: Production Observability & Diagnostics ✅

- Structured logging with correlation IDs on every OPC UA read
- Configurable log verbosity (`OPCGW_LOG_LEVEL`) and directory (`OPCGW_LOG_DIR`)
- Microsecond UTC timestamps; performance-budget warnings on hot paths
- ChirpStack and OPC UA diagnostic event taxonomy
- Symptom cookbook in `docs/logging.md`

### Epic 7: Security Hardening ✅

- Credential management via environment variables (`OPCGW_*`) with `REPLACE_ME_WITH_*` placeholders + startup detection
- Three OPC UA security endpoints (None / Basic256 Sign / Basic256 SignAndEncrypt) with HMAC-SHA-256 keyed credential digests
- PKI directory layout enforcement (`0o600` files + `0o700` directories, fail-closed)
- OPC UA connection limiting (`[opcua].max_connections`, default 10)
- Sanitised audit events with constant-time comparisons

---

## Phase B — Real-Time, Historical, and Web UI ✅

### Epic 8: Real-Time Subscriptions & Historical Data ✅

**Closed 2026-05-14** (with Story 8-4 descoped on the same day — see Known Failures below).

- async-opcua subscription support (FR21) — DataChange notifications with `DataChangeFilter`
- OPC UA `HistoryRead` for raw historical data (FR22) with 7-day retention floor
- Wrap-don't-subclass pattern around `SimpleNodeManagerImpl` (documented in Story 8-1 spike report)
- `[storage].retention_days` + `[opcua].max_history_data_results_per_node` config knobs
- Microsecond-precision UTC timestamps in stored history
- `AccessLevel::HISTORY_READ` + `historizing = true` on every metric variable

### Epic 9: Web Configuration & Hot-Reload ✅

**Closed 2026-05-14** — all 9 stories (9-0 through 9-8) shipped + retrospective complete.

- Axum 0.8 embedded web server gated by HTTP Basic auth (FR50 / NFR11)
- Gateway status dashboard with live ChirpStack health, error counts, application/device counts (FR38)
- Live metric values page with per-row staleness badges (FR37) — *currently shows the #108 data-type-string bug until Epic A lands*
- CRUD for applications, devices + metric mappings, and commands via the web UI (FR34 / FR35 / FR36 / FR40)
- CSRF defence (Origin/Referer same-origin + JSON-only Content-Type)
- TOML round-trip persistence via `toml_edit` with atomic tempfile + rename and lock-held-across-reload
- Configuration hot-reload triggered by SIGHUP, validate-then-swap discipline, knob taxonomy (hot-reload-safe / restart-required / address-space-mutating) — FR39 / FR40
- Dynamic OPC UA address-space mutation under live subscriptions (FR24) with the 4-phase mutation envelope from the 9-0 spike (Q2 `BadNodeIdUnknown` transition → delete → add → DisplayName rename)
- Three new audit-event families: `config_reload_*`, `address_space_mutation_*`, plus per-resource CRUD events (`application_*`, `device_*`, `command_*`)

---

## Known Failures

### Story 8-4: Threshold-Based Alarm Conditions (FR23) — descoped 2026-05-14

Story 8-4 was originally scoped inside Epic 8 but **was not implemented**. It was first parked as a Known Failure on 2026-05-01 (Epic 8 retro) and then **explicitly descoped from Epic 8 on 2026-05-14** so Epic 8 could close cleanly.

- **What's missing:** the gateway does NOT propagate `Bad` / `Warning` OPC UA status codes when a metric crosses a configured `low_alarm` / `high_alarm` threshold. SCADA clients see `Good` status for every metric read regardless of threshold proximity.
- **What still works:** Story 5-2's stale-data status codes (`Uncertain` after 1× poll interval, `Bad` after 3× poll interval) function unchanged. Story 8-2's `DataChangeFilter`-driven subscriptions deliver these status transitions correctly.
- **Operator workaround:** define alarm thresholds in the SCADA application (FUXA / Ignition) rather than in opcgw. This is the operationally sound choice anyway — alarm-condition state belongs in the SCADA client which has full context of acknowledged / suppressed / shelved state.
- **Functional block:** revival is gated by issue [#108](https://github.com/guycorbaz/opcgw/issues/108). Threshold alarms need real metric values to alarm on; until Epic A ships, an alarm story has nothing meaningful to compare against.
- **Future revival:** if surfaced, the work lands under a **new story name** in a future Phase B epic (NOT `8-4`); the original spec was scoped for the wrong epic phase and should be redrafted with the Stories 8-2 / 8-3 lessons baked in (specifically: the `DataChangeFilter` `trigger: StatusValue` path is the correct integration point).

Canonical narrative: `_bmad-output/implementation-artifacts/deferred-work.md` and `_bmad-output/implementation-artifacts/epic-8-retro-2026-05-01.md` § Known Failures + the 2026-05-14 descope addendum.

---

## Next: Epic A — Storage Payload Migration ⏳

**Status:** not yet opened. Identified as the immediate next epic per Epic 9 retrospective action item AI6 (2026-05-14).

- **Why it matters:** Issue [#108](https://github.com/guycorbaz/opcgw/issues/108) is the production-deployment blocker. The `MetricType` enum is payload-less, so the `metric_values.value` column stores the data-type string instead of the measurement. Every shipped feature that reads metric values back (dashboard, OPC UA Read, OPC UA HistoryRead) returns type-strings, not real data.
- **Shape:** Epic-1-scale storage-trait refactor. `MetricType` carries the value payload; the storage layer round-trips it; readers no longer fall back to the type-name string.
- **Affects:** Epics 2, 5, 8, and Story 9-3 (surface-correct but data-incorrect until #108 lands).
- **Gate for:** production deployment of v2.0, and any future revival of Story 8-4's threshold-alarm functionality.

---

## How to Track Progress

- **Epic Status (canonical):** [`_bmad-output/implementation-artifacts/sprint-status.yaml`](https://github.com/guycorbaz/opcgw/blob/main/_bmad-output/implementation-artifacts/sprint-status.yaml)
- **Story Details:** individual story files in [`_bmad-output/implementation-artifacts/`](https://github.com/guycorbaz/opcgw/tree/main/_bmad-output/implementation-artifacts)
- **Retrospectives:** `epic-N-retro-YYYY-MM-DD.md` under the same folder
- **GitHub:** [opcgw on GitHub](https://github.com/guycorbaz/opcgw) for issue tracking and pull requests

---

## Questions?

See the [Architecture](architecture.html) page for system design details, or the [Quick Start](quickstart.html) guide to get opcgw running today.

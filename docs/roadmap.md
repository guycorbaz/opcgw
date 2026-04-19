---
layout: default
title: Development Roadmap
permalink: /roadmap/
---

## opcgw Development Roadmap

**Strategic Goal:** Deliver production-ready v2.0 with feature parity to v1.0 + new capabilities (data persistence, web UI, security hardening).

> **Principle:** Quality over Speed. We're investing time to ensure v2.0 is genuinely better than v1.0, not just different.

---

## Timeline Overview

```
Epic 1: Foundation ✅ → Epic 2: Persistence → Epic 3: Commands → Epic 4: Scalability 
    ↓                        ↓                    ↓                   ↓
  (Done)              (In Planning)          (Queued)             (Queued)
                     
        ↓                    ↓                    ↓
    Epic 5: Visibility → Epic 6: Security → Epic 8: Web UI → Production v2.0
    (Queued)          (Queued)            (Queued)          (Q3 2026)
    
    
Post-Launch:
    ↓
Epic 7: Real-Time Subscriptions & Historical Data
```

---

## Current Status: Epic 1 ✅ COMPLETE

**Epic:** Crash-Free Gateway Foundation  
**Duration:** April 2-19, 2026 (2 weeks)  
**Status:** ✅ All 5 stories complete

### Completed Stories

1. ✅ **1-1: Update Dependencies and Rust Toolchain**
   - Rust 1.87.0 → 1.94.0
   - tonic 0.13 → 0.14 (major), async-opcua 0.16 → 0.17 (major)
   - 3 breaking changes managed systematically

2. ✅ **1-2: Migrate Logging from log4rs to Tracing**
   - Replaced log4rs with tracing ecosystem
   - 136 log calls converted to structured fields
   - Per-module logging via tracing-appender

3. ✅ **1-3: Comprehensive Error Handling**
   - 15 production panic sites eliminated
   - Zero panics in production code paths
   - Graceful degradation on errors

4. ✅ **1-4: Graceful Shutdown with CancellationToken**
   - SIGINT and SIGTERM handling (Docker-safe)
   - Clean shutdown sequence with timeout protection
   - Tested token propagation across tasks

5. ✅ **1-5: Configuration Validation and Clean Startup**
   - Comprehensive validation with clear error messages
   - Field-level error reporting
   - Environment variable override support

### Key Achievements

- **Test Coverage:** 18/18 tests passing (up from 17)
- **Code Quality:** Zero clippy warnings
- **Reliability:** Zero production panics (down from 15)
- **Team Confidence:** High — no blockers throughout
- **Quality Planning:** Thorough upfront work enabled smooth execution

---

## Next Phase: Epics 2-6 (Production Features)

### Epic 2: Data Persistence (Next)

**Timeline:** ~2-3 weeks (10-12 stories, tighter granularity)  
**Why It Matters:** Metrics must survive gateway restarts  
**What It Delivers:**
- SQLite backend with WAL mode (crash-safe)
- Last-known metric values persisted and restored
- Historical data with configurable retention
- Automatic pruning of old data

**Key Stories:**
1. StorageBackend trait + InMemoryBackend
2. SQLite backend and schema (5 tables)
3. Metric persistence and batch writes
4. Metric restore on startup
5. Historical data pruning + more (broken into smaller pieces)

---

### Epic 3: Reliable Command Execution (After Epic 2)

**Timeline:** ~1-2 weeks (3 stories)  
**Why It Matters:** Operators need to send commands to devices (feature parity with v1.0)  
**What It Delivers:**
- FIFO command queue in SQLite
- Parameter validation before transmission
- Command delivery status reporting to OPC UA clients

**Key Stories:**
1. SQLite-backed FIFO command queue
2. Command parameter validation
3. Command delivery status reporting

---

### Epic 4: Scalable Data Collection (After Epic 3)

**Timeline:** ~2 weeks (4 stories)  
**Why It Matters:** Handle 100+ devices with all metric types  
**What It Delivers:**
- Support for all ChirpStack metric types (Gauge, Counter, Absolute, Unknown)
- API pagination for large deployments
- Auto-recovery from ChirpStack outages (<30 seconds)
- Poller refactoring to use SQLite backend

**Key Stories:**
1. Poller refactoring to SQLite backend
2. Support all ChirpStack metric types
3. API pagination for large deployments (100+ devices)
4. Auto-recovery from ChirpStack outages

---

### Epic 5: Operational Visibility (After Epic 4)

**Timeline:** ~1-2 weeks (3 stories)  
**Why It Matters:** Operators see clear warnings about stale data and gateway health  
**What It Delivers:**
- OPC UA server refactoring to SQLite backend
- Stale data detection (UncertainLastUsableValue status codes)
- Gateway health metrics visible in OPC UA (last poll, error count, connection state)

**Key Stories:**
1. OPC UA server refactoring to SQLite backend
2. Stale data detection and status codes
3. Gateway health metrics in OPC UA

---

### Epic 6: Security Hardening (After Epic 5)

**Timeline:** ~1-2 weeks (3 stories)  
**Why It Matters:** Production security — no exposed credentials, controlled access  
**What It Delivers:**
- Credential management via environment variables
- Multiple OPC UA security endpoints (None, Basic256 Sign, Basic256 SignAndEncrypt)
- OPC UA client connection limiting

**Key Stories:**
1. Credential management via environment variables
2. OPC UA security endpoints and authentication
3. Connection limiting

---

## Final Phase: Epic 8 (Web UI)

### Epic 8: Web Configuration & Hot-Reload (After Epic 6)

**Timeline:** ~3 weeks (8 stories)  
**Why It Matters:** Major break from v1.0 — web UI instead of TOML file editing  
**What It Delivers:**
- Embedded Axum web server with basic authentication
- Status dashboard (gateway health, ChirpStack connection, error counts)
- Live metric values display
- CRUD for applications, devices, commands
- Configuration hot-reload without gateway restart
- Dynamic OPC UA address space mutation

**Key Stories:**
1. Axum web server and basic authentication
2. Gateway status dashboard
3. Live metric values display
4. Application CRUD via web UI
5. Device and metric mapping CRUD
6. Command CRUD via web UI
7. Configuration hot-reload
8. Dynamic OPC UA address space mutation

---

## Post-Launch: Epic 7

### Epic 7: Real-Time Subscriptions & Historical Data

**Timeline:** 2-3 weeks (4 stories) — after production deployment  
**What It Delivers:**
- OPC UA subscription support (real-time push notifications)
- Historical data queries (7-day retention)
- Threshold-based alarm conditions

---

## Production Readiness Checklist

Before v2.0 is deployed to production:

- ✅ Epic 1: Crash-free foundation
- ⏳ Epic 2: Data persistence (no data loss on restart)
- ⏳ Epic 3: Command execution (feature parity with v1.0)
- ⏳ Epic 4: Scalable data collection (handles 100+ devices)
- ⏳ Epic 5: Operational visibility (stale-data detection, health metrics)
- ⏳ Epic 6: Security hardening (credentials, OPC UA security, connection limits)
- ⏳ Epic 8: Web UI (configure without TOML editing)
- ⏳ All tests passing, zero panics
- ⏳ Security review complete
- ⏳ Documentation updated

**Target Deployment:** Q3 2026 (6-9 months from Epic 2 start)

---

## Key Principles

### 1. Quality Over Speed
We're investing time to deliver v2.0 that's genuinely better than v1.0, not just different. A v2.0 that feels worse than v1.0 would undermine adoption.

### 2. Feature Parity Baseline
Every feature available in v1.0 should be available in v2.0. This ensures users don't experience regression.

### 3. Tighter Story Granularity
Epics 2-8 use 10-12 stories instead of 5-6 to provide better visibility and control. Smaller stories = more frequent validation points.

### 4. Thorough Planning
Replicate Epic 1's approach: PRD validation, architecture design, detailed story dev notes. This enables confident execution.

---

## How to Track Progress

- **Epic Status:** Check `_bmad-output/implementation-artifacts/sprint-status.yaml`
- **Story Details:** Read individual story files in `_bmad-output/implementation-artifacts/`
- **Retrospectives:** Review epic retrospectives for lessons learned
- **GitHub:** See [opcgw on GitHub](https://github.com/guycorbaz/opcgw) for issue tracking and pull requests

---

## Questions?

See the [Architecture](architecture.html) page for system design details, or the [Quick Start](quickstart.html) guide to get opcgw running today.


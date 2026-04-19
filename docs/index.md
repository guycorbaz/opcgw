# opcgw — Project Documentation Index

> Generated: 2026-04-01 | Scan Level: Exhaustive | Workflow: initial_scan

## Project Overview

- **Type:** Monolith (single Rust crate)
- **Primary Language:** Rust (2021 edition, min rustc 1.87.0)
- **Architecture:** Concurrent Service Architecture (ChirpStack Poller + OPC UA Server via shared storage)
- **Version:** 1.0.0
- **License:** MIT OR Apache-2.0

### Quick Reference

- **Tech Stack:** Rust + Tokio + Tonic (gRPC) + async-opcua + Figment (config) + log4rs
- **Entry Point:** `src/main.rs`
- **Architecture Pattern:** Two concurrent tokio tasks communicating via `Arc<Mutex<Storage>>`
- **Default OPC UA Port:** 4840 (Docker: 4855)

## Generated Documentation

- [Project Overview](./project-overview.md) — Purpose, tech stack, status, and architecture summary
- [Architecture](./architecture.md) — Detailed module breakdown, data flow, and design decisions
- [Source Tree Analysis](./source-tree-analysis.md) — Annotated directory structure and critical paths
- [API Contracts](./api-contracts.md) — ChirpStack gRPC client API and OPC UA server interface
- [Development Guide](./development-guide.md) — Prerequisites, build/run/test commands, configuration
- [Deployment Guide](./deployment-guide.md) — Docker, native binary, network requirements, security hardening

## Existing Documentation

- [README](../README.md) — Project introduction, features, limitations, and setup
- [Architecture (original)](../doc/architecture.md) — Hand-written architecture documentation
- [Planning](../doc/planning.md) — Development roadmap with phase milestones and checkboxes
- [Requirements](../doc/requirements.md) — Project requirements specification
- [Contributing](../CONTRIBUTING.md) — Contribution guidelines and PR process
- [Security](../SECURITY.md) — Security policy
- [Code of Conduct](../CODE_OF_CONDUCT.md) — Community code of conduct
- [CLAUDE.md](../CLAUDE.md) — Claude Code AI assistant instructions

## Known Gaps & Planned Evolution

- **OPC UA features:** Many OPC UA capabilities are missing (subscriptions, historical data, alarms, method nodes, complex types). Currently basic Browse/Read/Write only.
- **Configuration:** Currently file-based TOML only. Planned: web-based configuration interface for managing applications, devices, and metrics without restarts.
- **Storage:** Currently in-memory HashMap (data lost on restart). Planned: local database for persistent storage and historical data.

## Getting Started

1. Install Rust >= 1.87.0 and protobuf compiler
2. Clone the repo and run `cargo build`
3. Configure `config/config.toml` with your ChirpStack server address, API token, and device mappings
4. Run with `cargo run` or via Docker (`docker compose up`)
5. Connect an OPC UA client (e.g., FUXA) to `opc.tcp://<host>:4840/`

For detailed setup instructions, see the [Development Guide](./development-guide.md).

# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**opcgw** is a Rust application that bridges ChirpStack (LoRaWAN Network Server) with OPC UA clients for industrial automation/SCADA systems. It polls device metrics from ChirpStack's gRPC API, stores them in memory, and exposes them as OPC UA variables.

## Build & Development Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Release build

# Run
cargo run                      # Run with default config
cargo run -- -c path/to/config.toml  # Run with custom config

# Test
cargo test                     # Run all tests
cargo test <test_name>         # Run a single test

# Via cargo-make (install: cargo install cargo-make)
cargo make tests               # Clean + run tests
cargo make cover               # Generate coverage report (requires grcov)
cargo make clean               # Clean build artifacts

# Docker
docker compose up              # Run via Docker (exposes port 4855)
```

The build script (`build.rs`) compiles Protocol Buffer definitions from `proto/chirpstack/` using `tonic-build`.

## Architecture

**Data flow:** ChirpStack gRPC API → ChirpstackPoller → Storage (in-memory HashMap) → OPC UA Server → OPC UA Clients

### Core modules (all in `src/`):

- **`main.rs`** — Entry point. Parses CLI args (clap), initializes logging (log4rs), creates shared storage, spawns poller and OPC UA server as separate tokio tasks, handles graceful shutdown via Ctrl+C.
- **`chirpstack.rs`** (~1200 lines) — `ChirpstackPoller` polls ChirpStack's gRPC API at configurable intervals. Handles authentication, connection retries, and transforms ChirpStack metrics into the internal format. Tracks server availability with internal device ID `cp0`.
- **`storage.rs`** (~1100 lines) — Thread-safe in-memory storage using `HashMap` behind `Mutex`. Hierarchical: Devices → Metrics. Metric types: Float, Int, Bool, String. Shared between poller (writer) and OPC UA server (reader) via `Arc<Mutex<Storage>>`.
- **`opc_ua.rs`** (~870 lines) — OPC UA 1.04 server using `async-opcua`. Dynamically builds address space from configuration. Exposes device metrics as OPC UA variables.
- **`config.rs`** (~910 lines) — Configuration via `figment` (TOML file + environment variable overrides). Defines structures for applications, devices, and metric mappings.
- **`utils.rs`** (~360 lines) — Constants (default ports, URIs, timeouts), `OpcGwError` enum (Configuration, ChirpStack, OpcUa, Storage variants).

## Configuration

- **Main config:** `config/config.toml` — sections: `[global]`, `[chirpstack]` (server address, API token, tenant ID, poll frequency, retries), `[opcua]` (endpoint, security, PKI), `[[application]]` (array of apps with devices and metrics).
- **Logging:** `config/log4rs.yaml` — per-module log levels, console + file appenders.
- **PKI:** `pki/` directory holds OPC UA certificates (own, private, trusted, rejected).
- Environment variables can override TOML configuration values (via figment).

## Code Conventions

- SPDX license headers (MIT OR Apache-2.0) and copyright `(c) [2024] Guy Corbaz` in each source file.
- Rust 2021 edition, minimum rustc 1.87.0.
- Custom error type `OpcGwError` in `utils.rs` using `thiserror`.
- Extensive doc comments on all public items.

## Development Status

The project is v1.0.0 and under active development. Basic polling, storage, configuration, and OPC UA server setup are implemented. OPC UA address space construction is partially complete. Data type conversions, real-time subscriptions, and write-back to ChirpStack are not yet implemented. See `doc/planning.md` for the roadmap.

## Issue Management

All bugs, known failures, change requests, and other work items must be managed via GitHub issues. This ensures:
- Clear tracking and visibility of all work
- Proper prioritization and scheduling
- Historical record of decisions and changes
- Integration with pull requests and code review

Do not implement fixes or changes without a corresponding GitHub issue.

## Security & Quality Assurance

### Epic Completion Requirements

Before closing an epic retrospective:

1. **Run security check** — Execute a comprehensive security review of all changes made during the epic
   - Verify no hardcoded credentials or secrets in code
   - Check for input validation on all external data (ChirpStack API, OPC UA writes, config files)
   - Validate error messages don't leak sensitive information
   - Confirm no SQL injection, command injection, or similar vulnerabilities
   - Review permission handling and access control

2. **Code quality verification**
   - All tests passing (`cargo test`)
   - No clippy warnings (`cargo clippy`)
   - No unsafe code blocks without documented justification
   - SPDX license headers present on all files

3. **Documentation review**
   - Acceptance criteria fully satisfied
   - File list complete and accurate
   - Dev notes document architectural decisions
   - References to planning documents included

Do not mark an epic as done without completing the security check.

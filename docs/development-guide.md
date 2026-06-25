# Development Guide — opcgw

> Generated: 2026-04-01 | Updated: 2026-06-25 for v2.3.0 | Scan Level: Exhaustive

## Prerequisites

| Requirement | Version | Notes |
|------------|---------|-------|
| Rust toolchain | >= 1.94.0 | Install via [rustup](https://rustup.rs/) |
| Protobuf compiler | protoc | Required for building proto definitions |
| cargo-make | Latest | Optional, for task runner (`cargo install cargo-make`) |
| grcov | Latest | Optional, for coverage reports (`cargo install grcov`) |
| Docker | Latest | Optional, for containerized deployment |

## Environment Setup

1. **Clone the repository:**
   ```bash
   git clone https://github.com/guycorbaz/opcgw.git
   cd opcgw
   ```

2. **Install protobuf compiler** (if not already installed):
   ```bash
   # Ubuntu/Debian
   sudo apt-get install protobuf-compiler
   
   # macOS
   brew install protobuf
   ```

3. **Verify Rust toolchain:**
   ```bash
   rustc --version   # Should be >= 1.94.0
   cargo --version
   ```

## Build Commands

```bash
# Debug build
cargo build

# Release build (optimized, with LTO)
cargo build --release

# Build will automatically compile proto files via build.rs
```

## Run Commands

```bash
# Run with default config (config/config.toml)
cargo run

# Run with custom config file
cargo run -- -c path/to/config.toml

# Run in Docker
docker compose up
```

## Test Commands

```bash
# Run all tests
cargo test

# Run a specific test
cargo test test_chirpstack_status

# Via cargo-make: clean + test
cargo make tests

# Generate HTML coverage report (requires grcov)
cargo make cover
# Report output: target/coverage/html/
```

## Configuration

### Configuration model (SQLite authoritative)

From v2.x **SQLite is the authoritative configuration store** (the database lives in the mounted `data/` directory; schema migrations v001–v012 run automatically and forward-only on boot). The real configuration path for operators is the **SQLite-backed web UI** — first boot serves the browser `/setup` wizard, and configuration is edited live through the dashboard (no text-file editing required).

- **`config/config.toml`** is a **bootstrap seed only** — read at boot to populate a fresh database, then overridden by SQLite for any key set through the web UI. Operators may delete it post-migration.
- **`config/secrets.toml`** (chmod 0600) holds operator secrets (`[chirpstack].api_token`, `[opcua].user_password`); never mutated at runtime.
- **Precedence:** env (`OPCGW_*`) > SQLite > `config.toml` > built-in default.

### Bootstrap-seed file (`config/config.toml`)

The seed file is organized in sections:

- **`[global]`** — Application-wide settings (`debug`)
- **`[chirpstack]`** — ChirpStack connection (`server_address`, `tenant_id`, `polling_frequency`, `retry`, `delay`; `api_token` lives in `secrets.toml`)
- **`[opcua]`** — OPC UA server settings (application identity, network, PKI, authentication)
- **`[[application]]`** — Array of ChirpStack applications, each with nested `[[application.device]]` and `[[application.device.read_metric]]` / `[[application.device.command]]`. A `[[application.device.command]]` entry carries an optional **`command_class`** field (e.g. `"valve"`) that selects the device-class driver used to translate an OPC UA write into a LoRaWAN downlink.

### Environment Variable Overrides

Any config value can be overridden with `OPCGW_` prefix and double underscores for nesting:
```bash
OPCGW_CHIRPSTACK__SERVER_ADDRESS=http://10.0.0.1:8080
OPCGW_OPCUA__HOST_PORT=4841
```

The config file path itself can be overridden with `CONFIG_PATH` env var.

### Logging (tracing)

Logging uses the `tracing` + `tracing-subscriber` stack. Levels are controlled with an env filter (`RUST_LOG` / `EnvFilter` style), e.g.:

```bash
RUST_LOG=info,opcgw::chirpstack=debug cargo run
```

Set per-module verbosity by adding `target=level` directives to the filter. Reduce to `info` or `warn` for production.

## Project Conventions

- **License headers:** Every `.rs` file starts with `// SPDX-License-Identifier: MIT OR Apache-2.0` and `// Copyright (c) [2024] [Guy Corbaz]`
- **Edition:** Rust 2021
- **Error handling:** Custom `OpcGwError` enum in `utils.rs` using `thiserror`
- **Doc comments:** Extensive `///` doc comments on all public items
- **Logging:** `tracing` macros (`debug!`, `trace!`, `info!`, `warn!`, `error!`) throughout, via `tracing-subscriber`

## Test Configuration

Tests use a separate config at `tests/config/config.toml` with synthetic application/device/metric data. The test config is loaded via the same `Figment` mechanism, with `CONFIG_PATH` env var support for CI overrides.

## Docker Deployment

```bash
# Build the Docker image
docker build -t opcgw .

# Run with Docker Compose (exposes 4840 OPC UA + 8080 web UI)
docker compose up -d

# Volumes mounted:
#   ./log    → /usr/local/bin/log
#   ./config → /usr/local/bin/config
#   ./pki    → /usr/local/bin/pki
#   ./data   → /usr/local/bin/data   (required — SQLite DB persistence)
```

The Docker image uses a multi-stage build: Rust 1.94.0 for compilation, `ubuntu:24.04` for runtime, running as **non-root user `opcgw` (UID 10001)**. It exposes **4840** (OPC UA) and **8080** (web UI). Bind-mount targets must be `chown`'d to UID 10001 before first start; see the deployment guide for the full first-deploy checklist.

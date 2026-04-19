# Development Guide — opcgw

> Generated: 2026-04-01 | Scan Level: Exhaustive

## Prerequisites

| Requirement | Version | Notes |
|------------|---------|-------|
| Rust toolchain | >= 1.87.0 | Install via [rustup](https://rustup.rs/) |
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
   rustc --version   # Should be >= 1.87.0
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

### Main Configuration (`config/config.toml`)

The configuration file is organized in sections:

- **`[global]`** — Application-wide settings (`debug`)
- **`[chirpstack]`** — ChirpStack connection (`server_address`, `api_token`, `tenant_id`, `polling_frequency`, `retry`, `delay`)
- **`[opcua]`** — OPC UA server settings (application identity, network, PKI, authentication)
- **`[[application]]`** — Array of ChirpStack applications, each with nested `[[application.device]]` and `[[application.device.read_metric]]` / `[[application.device.command]]`

### Environment Variable Overrides

Any config value can be overridden with `OPCGW_` prefix and double underscores for nesting:
```bash
OPCGW_CHIRPSTACK__SERVER_ADDRESS=http://10.0.0.1:8080
OPCGW_OPCUA__HOST_PORT=4841
```

The config file path itself can be overridden with `CONFIG_PATH` env var.

### Logging (`config/log4rs.yaml`)

Per-module logging with separate file appenders:
- `opcgw::chirpstack` → `log/chirpstack.log`
- `opcgw::opc_ua` → `log/opc_ua.log`
- `opcgw::storage` → `log/storage.log`
- `opcgw::config` → `log/config.log`
- Root logger → stdout + `log/opc_ua_gw.log`

## Project Conventions

- **License headers:** Every `.rs` file starts with `// SPDX-License-Identifier: MIT OR Apache-2.0` and `// Copyright (c) [2024] [Guy Corbaz]`
- **Edition:** Rust 2021
- **Error handling:** Custom `OpcGwError` enum in `utils.rs` using `thiserror`
- **Doc comments:** Extensive `///` doc comments on all public items
- **Logging:** `log` crate macros (`debug!`, `trace!`, `info!`, `warn!`, `error!`) throughout

## Test Configuration

Tests use a separate config at `tests/config/config.toml` with synthetic application/device/metric data. The test config is loaded via the same `Figment` mechanism, with `CONFIG_PATH` env var support for CI overrides.

## Docker Deployment

```bash
# Build the Docker image
docker build -t opcgw .

# Run with Docker Compose (exposes port 4855)
docker compose up -d

# Volumes mounted:
#   ./log    → /usr/local/bin/log
#   ./config → /usr/local/bin/config
#   ./pki    → /usr/local/bin/pki
```

The Docker image uses a multi-stage build: Rust 1.87 for compilation, Ubuntu for runtime. Port 4855 is exposed (note: differs from default OPC UA port 4840).

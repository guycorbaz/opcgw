# Source Tree Analysis — opcgw

> Generated: 2026-04-01 | Scan Level: Exhaustive

## Directory Structure

```text
opcgw/
├── src/                          # Application source code (Rust)
│   ├── main.rs                   # Entry point: CLI args, logger init, spawns poller + OPC UA server
│   ├── chirpstack.rs             # ChirpStack gRPC client & polling service (~1225 lines)
│   ├── config.rs                 # Configuration loading via figment (TOML + env vars) (~913 lines)
│   ├── opc_ua.rs                 # OPC UA server using async-opcua (~873 lines)
│   ├── storage.rs                # In-memory device/metric storage with HashMap (~1097 lines)
│   └── utils.rs                  # Constants, OpcGwError enum, debug utilities (~365 lines)
│
├── proto/                        # Protocol Buffer definitions (ChirpStack gRPC API)
│   ├── chirpstack/
│   │   ├── api/                  # Device, Application, Gateway, Tenant, etc. service protos
│   │   ├── stream/               # Frame, meta, backend interfaces
│   │   ├── integration/          # Integration service proto
│   │   ├── internal/             # Internal service proto
│   │   └── gw/                   # Gateway proto
│   ├── google/api/               # Google API annotations (HTTP mapping)
│   └── common/                   # Common protobuf definitions
│
├── config/                       # Runtime configuration
│   ├── config.toml               # Main application config (ChirpStack, OPC UA, applications)
│   └── log4rs.yaml               # Logging configuration (per-module appenders)
│
├── tests/                        # Test fixtures
│   └── config/
│       └── config.toml           # Test-specific configuration file
│
├── pki/                          # OPC UA Public Key Infrastructure
│   ├── own/                      # Server's own certificate
│   ├── private/                  # Server's private key
│   ├── trusted/                  # Trusted client certificates
│   └── rejected/                 # Rejected certificates
│
├── doc/                          # Project documentation (hand-written)
│   ├── architecture.md           # Architecture documentation
│   ├── planning.md               # Development roadmap and milestones
│   └── requirements.md           # Project requirements
│
├── docs/                         # Generated documentation (this folder)
│   └── _config.yml               # GitHub Pages configuration
│
├── log/                          # Runtime log output directory
│
├── design-artifacts/             # BMad design artifact folders (empty/scaffolded)
│   ├── A-Product-Brief/
│   ├── B-Trigger-Map/
│   ├── C-UX-Scenarios/
│   ├── D-Design-System/
│   ├── E-PRD/
│   ├── F-Testing/
│   └── G-Product-Development/
│
├── build.rs                      # Build script: compiles .proto files with tonic-build
├── Cargo.toml                    # Rust package manifest (dependencies, profiles)
├── Cargo.lock                    # Dependency lock file
├── Dockerfile                    # Multi-stage Docker build (rust:1.87 → ubuntu)
├── docker-compose.yml            # Docker Compose: exposes port 4855, mounts config/log/pki
├── Makefile.toml                 # cargo-make task definitions (test, coverage)
├── README.md                     # Project README with setup and usage instructions
├── CLAUDE.md                     # Claude Code AI assistant instructions
├── CONTRIBUTING.md               # Contribution guidelines
├── CODE_OF_CONDUCT.md            # Code of conduct
├── SECURITY.md                   # Security policy
├── LICENSE-APACHE                # Apache 2.0 license text
├── LICENSE-MIT                   # MIT license text
└── write.md                      # (Unknown purpose)
```

## Critical Directories

| Directory | Purpose |
|-----------|---------|
| `src/` | All application Rust source code (6 files, ~4,473 lines total) |
| `proto/` | ChirpStack gRPC protobuf definitions compiled at build time |
| `config/` | Runtime TOML and YAML configuration files |
| `tests/config/` | Test-specific configuration fixtures |
| `pki/` | OPC UA certificate management (own, private, trusted, rejected) |
| `doc/` | Hand-written architecture, planning, and requirements docs |

## Entry Points

| File | Role |
|------|------|
| `src/main.rs` | Application entry point — parses CLI args, inits logger, creates storage, spawns tokio tasks |
| `build.rs` | Build-time entry point — compiles `.proto` files into Rust code via `tonic-build` |
| `Dockerfile` | Container entry point — `ENTRYPOINT ["/usr/local/bin/opcgw"]` on port 4855 |

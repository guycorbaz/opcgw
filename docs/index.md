---
layout: home
title: opcgw - ChirpStack to OPC UA Gateway
---

## Bridge IoT Data to Industrial Systems

**opcgw** is an open-source gateway that connects ChirpStack LoRaWAN Network Server with OPC UA industrial automation systems. It enables seamless integration of long-range wireless IoT devices into SCADA, MES, and industrial edge systems.

### Why opcgw?

- **Reliable Data Flow**: Continuous polling from ChirpStack with configurable retry logic and graceful error handling
- **Industrial Standards**: Native OPC UA 1.04 server for seamless integration with standard SCADA software
- **Smart IoT**: Support for multiple metric types (Float, Int, Bool, String) from diverse LoRaWAN sensors
- **Battle-Tested Architecture**: Built with Rust for memory safety, zero-copy performance, and production reliability
- **Easy Configuration**: TOML-based configuration with environment variable overrides
- **Observable**: Structured logging (tracing) for deep visibility into gateway operation
- **Version 2.0**: Modern foundation with configuration validation, graceful shutdown, and comprehensive error handling

### Key Capabilities

✨ **Real-Time Data Collection**
- Poll device metrics from ChirpStack at configurable intervals
- Support for multiple applications and devices
- Hierarchical data organization (Applications → Devices → Metrics)

🏭 **OPC UA Server**
- Expose collected metrics as OPC UA variables
- Dynamic address space from configuration
- Support for standard OPC UA clients (Ignition, KEPServerEx, etc.)

🔧 **Enterprise Integration**
- Environment-based credential management
- Connection resilience and auto-recovery
- Comprehensive error logging and diagnostics
- Graceful shutdown handling

### Quick Start

\`\`\`bash
# Clone and enter the repository
git clone https://github.com/guycorbaz/opcgw.git
cd opcgw

# Configure with your ChirpStack and OPC UA settings
cp config/config.example.toml config/config.toml
# Edit config/config.toml with your parameters

# Run the gateway
cargo run -- -c config/config.toml

# Or via Docker
docker compose up
\`\`\`

The gateway will:
1. Connect to your ChirpStack server
2. Retrieve device metrics
3. Store them in memory
4. Expose via OPC UA on port 4855

### Use Cases

- **Smart Agriculture**: Collect soil moisture, temperature, humidity from LoRaWAN sensors → feed into farm management systems
- **Industrial IoT**: Connect wireless asset trackers, equipment sensors → integrate with existing SCADA infrastructure
- **Environmental Monitoring**: Real-time environmental data from distributed sensor networks → centralized data systems
- **Building Management**: Building automation via wireless sensors → facility management systems

### Technology Stack

- **Language**: Rust 1.94.0+ (memory safety, zero-cost abstractions)
- **Protocols**: gRPC/Protobuf for ChirpStack, OPC UA 1.04 for industrial clients
- **Storage**: Async-safe in-memory storage with SQLite persistence planned
- **Logging**: Structured tracing (tokio-tracing) with per-module log files
- **Runtime**: Tokio for high-performance async I/O

---

**Status**: v2.0.0 - Production Ready  
**License**: MIT OR Apache-2.0  
**Latest Release**: [GitHub Releases](https://github.com/guycorbaz/opcgw/releases)

Get started with the [Quick Start Guide →](quickstart.html)

# Requirements for ChirpStack to OPC UA Gateway

## Overview
This document outlines the requirements for the ChirpStack to OPC UA Gateway application, which serves as a bridge between ChirpStack IoT server and OPC UA clients.

## Functional Requirements

### ChirpStack Integration
1. The application must connect to a ChirpStack server using the provided API token and server address.
2. The application must poll ChirpStack at configurable intervals to retrieve device metrics.
3. The application must support retrieving metrics from multiple ChirpStack applications and devices.
4. The application must handle connection failures to ChirpStack gracefully with appropriate error reporting.
5. The application must verify ChirpStack server availability before attempting to poll metrics.

### OPC UA Server
1. The application must implement an OPC UA server that exposes ChirpStack device metrics.
2. The OPC UA server must organize metrics in a hierarchical address space (Applications → Devices → Metrics).
3. The OPC UA server must support standard OPC UA data types for representing device metrics.
4. The OPC UA server must provide real-time updates of device metrics based on the polling frequency.
5. The OPC UA server must be configurable with appropriate security settings.
6. The OPC UA server must support writing values to writable nodes that correspond to ChirpStack device parameters.

### Data Storage and Management
1. The application must maintain an in-memory storage of device metrics.
2. The application must support mapping between ChirpStack metric names and OPC UA variable names.
3. The application must handle different metric types (Float, Integer, Boolean, String).
4. The application must provide methods to retrieve and update metric values.
5. The application must track which metrics are read-only and which are writable.

### Configuration
1. The application must support configuration via TOML files.
2. The application must support environment variable overrides for configuration.
3. The configuration must include:
   - ChirpStack connection details (server address, API token, polling frequency)
   - OPC UA server settings (endpoint URL, security settings)
   - Application and device mapping definitions
   - Metric type definitions and mappings

## Non-Functional Requirements

### Performance
1. The application must handle multiple concurrent OPC UA client connections.
2. The application must efficiently process and store metrics from numerous devices.
3. The application must minimize resource usage during idle periods.

### Reliability
1. The application must recover from temporary ChirpStack server unavailability.
2. The application must maintain the last known good values when ChirpStack is unavailable.
3. The application must log errors and operational status for troubleshooting.

### Security
1. The application must support secure communication with the ChirpStack API.
2. The OPC UA server must implement appropriate security measures (authentication, encryption).
3. The application must not expose sensitive information in logs or error messages.

### Maintainability
1. The code must be well-documented with clear comments.
2. The application must provide comprehensive logging for operational monitoring.
3. The application must be testable with unit and integration tests.

## Constraints
1. The application must be implemented in Rust.
2. The application must be compatible with ChirpStack API v3 and above.
3. The application must comply with OPC UA specification standards.
4. The application must be containerizable for deployment in Docker environments.

### Bidirectional Communication
1. The application must support writing data back to ChirpStack devices from the OPC UA interface.
2. The application must allow sending commands to ChirpStack devices through OPC UA method calls.
3. The application must validate write operations and command parameters before sending to ChirpStack.
4. The application must provide feedback on the success or failure of write operations and commands.
5. The application must support configurable access control for write operations.

## Future Considerations
1. Support for historical data access in the OPC UA server.
2. Integration with additional IoT platforms beyond ChirpStack.
3. Web-based administration interface for configuration management.
4. Support for complex command sequences and batch operations.

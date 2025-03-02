# OPC UA Gateway for ChirpStack - Architecture Documentation

## Overview

The OPC UA Gateway for ChirpStack is a bridge application that connects ChirpStack (an open-source LoRaWAN Network Server) with industrial systems that use the OPC UA protocol. This gateway enables seamless integration of IoT data from LoRaWAN devices into industrial automation systems, SCADA, and other OPC UA compatible clients.

The application polls the ChirpStack API for device metrics, stores them in memory, and exposes them through an OPC UA server interface, creating a standardized way to access LoRaWAN device data.

## System Architecture

The system follows a modular architecture with clear separation of concerns:

```
┌─────────────────────────────────────────────────────────────────┐
│                        OPC UA Gateway                           │
│                                                                 │
│  ┌───────────────┐      ┌───────────────┐     ┌──────────────┐  │
│  │  ChirpStack   │      │    Storage    │     │   OPC UA     │  │
│  │    Poller     │◄────►│    Module     │◄───►│   Server     │  │
│  └───────────────┘      └───────────────┘     └──────────────┘  │
│          ▲                                           ▲          │
└──────────┼───────────────────────────────────────────┼──────────┘
           │                                           │
           ▼                                           ▼
┌─────────────────────┐                     ┌─────────────────────┐
│    ChirpStack API   │                     │    OPC UA Clients   │
└─────────────────────┘                     └─────────────────────┘
```

### Core Components

#### 1. ChirpStack Poller (`src/chirpstack.rs`)

The ChirpStack Poller is responsible for:

- Establishing and maintaining a connection to the ChirpStack API
- Authenticating with the API using the configured API token
- Periodically polling the ChirpStack server for device metrics
- Processing and transforming the received data
- Storing the metrics in the shared Storage module
- Monitoring the ChirpStack server availability and response time

The poller runs in a continuous loop with a configurable polling frequency, fetching data for all configured applications and devices.

#### 2. Storage Module (`src/storage.rs`)

The Storage module serves as the central data repository:

- Maintains an in-memory store of all device metrics
- Provides thread-safe access to the data through mutex locks
- Tracks the ChirpStack server status (availability and response time)
- Organizes data in a hierarchical structure (applications → devices → metrics)
- Provides methods to get and set metric values
- Acts as the bridge between the ChirpStack Poller and OPC UA Server

#### 3. OPC UA Server (`src/opc_ua.rs`)

The OPC UA Server component:

- Creates and configures an OPC UA server instance
- Builds an address space that mirrors the ChirpStack application/device hierarchy
- Exposes device metrics as OPC UA variables with appropriate data types
- Handles OPC UA client connections and requests
- Retrieves current metric values from the Storage module when clients read variables
- Implements the OPC UA server security configuration

#### 4. Configuration Module (`src/config.rs`)

The Configuration module:

- Loads application settings from TOML files and environment variables
- Defines the structure for applications, devices, and metrics
- Provides helper methods to access configuration elements
- Validates the configuration during application startup

## Data Flow

1. **Configuration Loading**:
   - Application loads configuration from TOML files and environment variables
   - Configuration defines ChirpStack connection parameters, OPC UA server settings, and the list of applications/devices to monitor

2. **Initialization**:
   - ChirpStack Poller, Storage, and OPC UA Server components are initialized
   - OPC UA Server builds its address space based on the configured applications and devices
   - Storage prepares data structures for all configured metrics

3. **Runtime Operation**:
   - ChirpStack Poller periodically connects to the ChirpStack API
   - Poller retrieves metrics for all configured devices
   - Retrieved metrics are stored in the Storage module
   - OPC UA Server responds to client requests by retrieving current values from Storage
   - OPC UA clients can browse the address space and read device metrics

## Data Model

### ChirpStack Data Model

The gateway monitors ChirpStack applications and devices as defined in the configuration:

- **Applications**: Logical groupings of devices in ChirpStack
- **Devices**: Individual LoRaWAN devices with unique identifiers (DevEUI)
- **Metrics**: Measurements or status information from devices (e.g., temperature, humidity, battery level)

### OPC UA Address Space

The OPC UA address space is structured to reflect the ChirpStack hierarchy:

```
Root
└── Applications
    └── Application_1
        ├── Device_1
        │   ├── Metric_1
        │   ├── Metric_2
        │   └── ...
        ├── Device_2
        │   └── ...
        └── ...
    └── Application_2
        └── ...
```

Each metric is represented as an OPC UA variable with appropriate data type (Float, Integer, Boolean, etc.) based on the configuration.

## Configuration

The application is configured through TOML files and environment variables:

- **Global Settings**: Application-wide parameters
- **ChirpStack Connection**: Server address, API token, polling frequency
- **OPC UA Server**: Endpoint URL, security settings, server name
- **Applications and Devices**: List of applications and devices to monitor, with their respective metrics

Example configuration structure:
```toml
[global]
# Global application settings

[chirpstack]
server_address = "http://chirpstack.example.com:8080"
api_token = "your-api-token"
polling_frequency = 60  # seconds

[opcua]
endpoint_url = "opc.tcp://0.0.0.0:4840"
server_name = "ChirpStack OPC UA Gateway"

[[application]]
application_id = "1"
application_name = "Water Meters"

  [[application.device]]
  device_id = "device_1"
  device_name = "Water Meter 1"
  
    [[application.device.metric]]
    metric_name = "water_consumption"
    chirpstack_metric_name = "water_consumption"
    metric_type = "Float"
```

## Security

The gateway implements security at multiple levels:

1. **ChirpStack API Security**:
   - Uses API token authentication for ChirpStack API access
   - Supports HTTPS for secure communication with ChirpStack

2. **OPC UA Security**:
   - Implements OPC UA security profiles
   - Supports certificate-based authentication
   - Configurable security policies and message security modes

## Error Handling

The application implements comprehensive error handling:

- Connection failures to ChirpStack are detected and reported
- ChirpStack server availability is monitored
- API errors are logged and handled gracefully
- Configuration errors are validated during startup

## Extensibility

The architecture is designed for extensibility:

- New metric types can be added with minimal code changes
- Additional OPC UA features can be implemented by extending the server component
- Support for new ChirpStack API features can be added to the poller component

## Dependencies

The application relies on several key dependencies:

- **Tonic**: For gRPC communication with ChirpStack API
- **OPC UA**: For implementing the OPC UA server
- **Tokio**: For asynchronous runtime
- **Figment**: For configuration management
- **Log4rs**: For logging

## Deployment

The application can be deployed as:

- A standalone binary
- A Docker container (using the provided Dockerfile)
- A service in a Docker Compose environment (using docker-compose.yml)

## Monitoring and Maintenance

The application provides:

- Detailed logging with configurable log levels
- Metrics about its own operation
- Status information about the ChirpStack connection

## Conclusion

The OPC UA Gateway for ChirpStack provides a robust bridge between LoRaWAN devices and industrial systems using OPC UA. Its modular architecture ensures clear separation of concerns, while the shared storage provides efficient data exchange between components. The configuration-driven approach allows for flexible deployment in various environments without code changes.

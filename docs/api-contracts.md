# API Contracts — opcgw

> Generated: 2026-04-01 | Scan Level: Exhaustive

## Overview

opcgw acts as both a **gRPC client** (to ChirpStack) and an **OPC UA server** (to industrial clients). There are no REST or HTTP APIs.

---

## ChirpStack gRPC Client API (Outbound)

opcgw consumes the following ChirpStack 4 gRPC services:

### ApplicationService

| RPC Method | Request Type | Response Type | Used By |
|-----------|-------------|---------------|---------|
| `List` | `ListApplicationsRequest` | `ListApplicationsResponse` | `get_applications_list_from_server()` |

**Request fields:**
- `limit`: 100 (hardcoded)
- `offset`: 0
- `search`: empty
- `tenant_id`: from config

### DeviceService

| RPC Method | Request Type | Response Type | Used By |
|-----------|-------------|---------------|---------|
| `List` | `ListDevicesRequest` | `ListDevicesResponse` | `get_devices_list_from_server()` |
| `GetMetrics` | `GetDeviceMetricsRequest` | `GetDeviceMetricsResponse` | `get_device_metrics_from_server()` |
| `Enqueue` | `EnqueueDeviceQueueItemRequest` | `EnqueueDeviceQueueItemResponse` | `enqueue_device_request_to_server()` |

**GetMetrics request fields:**
- `dev_eui`: device identifier
- `start`: current system time
- `end`: current time + duration
- `aggregation`: 1 (raw data, no aggregation)

**Enqueue request fields (DeviceQueueItem):**
- `dev_eui`: target device
- `confirmed`: from command config
- `f_port`: from command config (must be >= 1)
- `data`: command payload bytes
- `is_pending`: true

### Authentication

All gRPC calls use a Bearer token injected via `AuthInterceptor`:
```
authorization: Bearer {config.chirpstack.api_token}
```

### Connection Management

- gRPC channel created per-request via `Channel::from_shared().connect()`
- TCP connectivity check before metrics requests with configurable retry (`config.chirpstack.retry`) and delay (`config.chirpstack.delay`)
- Default ChirpStack port: 8080

---

## OPC UA Server API (Inbound)

### Server Identity

| Property | Value |
|----------|-------|
| Application Name | Configurable (`config.opcua.application_name`) |
| Application URI | Configurable (`config.opcua.application_uri`) |
| Product URI | Configurable (`config.opcua.product_uri`) |
| Namespace URI | `urn:UpcUaG` |
| Default Port | 4840 (configurable via `config.opcua.host_port`) |

### Endpoints

| Endpoint ID | Security Policy | Security Mode | Security Level |
|------------|----------------|---------------|----------------|
| `null` (default) | None | None | 0 |
| `basic256_sign` | Basic256 | Sign | 3 |
| `basic256_sign_encrypt` | Basic256 | SignAndEncrypt | 13 |

All endpoints require user token `"user1"` (username/password from config).

### Address Space

The OPC UA address space is dynamically built from configuration:

```
Objects/
├── {application_name}/                    (Folder node)
│   ├── {device_name}/                     (Folder node)
│   │   ├── {metric_name}                  (Variable, read-only)
│   │   │   NodeId: ns={ns};s={metric_name}
│   │   │   Type: Int32 | Float | String | Boolean
│   │   │
│   │   ├── {command_name}                 (Variable, read-write)
│   │   │   NodeId: ns={ns};i={command_id}
│   │   │   Type: Int32 (command value)
│   │   └── ...
│   └── ...
└── ...
```

### Read Operations

- Variables expose live metric values via read callbacks
- Values fetched from in-memory storage on each read
- Returns `BadDataUnavailable` if metric not found
- Returns `BadInternalError` if storage lock fails

### Write Operations

- Command variables accept Int32 values
- Writes create a `DeviceCommand` queued for ChirpStack transmission
- Returns `Bad` if no value provided
- Returns `BadTypeMismatch` if value is not convertible to Int
- Returns `BadInternalError` if storage lock fails

### Data Type Mapping

| Internal Type | OPC UA Variant | Direction |
|--------------|----------------|-----------|
| `MetricType::Bool` | `Variant::Boolean` | Read |
| `MetricType::Int` | `Variant::Int32` | Read/Write |
| `MetricType::Float` | `Variant::Float` (f32) | Read |
| `MetricType::String` | `Variant::String` | Read |

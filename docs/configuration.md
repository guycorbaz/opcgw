---
layout: default
title: Configuration Reference
permalink: /configuration/
---

## Configuration File Format

opcgw uses TOML configuration files. The main config file is typically `config/config.toml`.

### Secrets

`api_token` and `user_password` ship as `REPLACE_ME_WITH_*` placeholders
that the gateway refuses to start with. Inject the real values via
environment variables — see [`docs/security.md`](security.md) for the env
var convention, the Docker / Kubernetes recipe, and the migration path
for existing deployments.

### Global Structure

```toml
[global]
# Global settings

[chirpstack]
# ChirpStack connection parameters

[opcua]
# OPC UA server parameters

[[application]]
# One or more applications
```

---

## [global] Section

Global application settings.

### Parameters

| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `debug` | bool | false | Enable debug logging (more verbose) |

### Example

```toml
[global]
debug = true  # Set to false in production for better performance
```

---

## [chirpstack] Section

Configuration for ChirpStack connection and polling behavior.

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `server_address` | string | ✓ | ChirpStack gRPC server address (format: `http://host:port`) |
| `api_token` | string | ✓ | API authentication token (from ChirpStack UI → Settings → API Keys) |
| `tenant_id` | string | ✓ | Tenant ID (from ChirpStack UI → Tenants) |
| `polling_frequency` | u64 | ✓ | Seconds between polls (must be > 0) |
| `retry` | u32 | ✓ | Maximum retry attempts on connection failure (must be > 0) |
| `delay` | u64 | ✓ | Milliseconds to wait between retry attempts (must be > 0) |

### Validation Rules

- `server_address`: Must be valid URL with http:// or https://
- `api_token`: Non-empty string
- `tenant_id`: Non-empty string
- `polling_frequency`: > 0 (recommended: 5-300 seconds)
- `retry`: > 0 (recommended: 3-10)
- `delay`: > 0 (recommended: 100-1000 ms)

### Example

```toml
[chirpstack]
server_address = "http://chirpstack.example.com:8080"
api_token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9..."  # Get from ChirpStack UI
tenant_id = "tes-tenant-id"
polling_frequency = 10      # Poll every 10 seconds
retry = 3                   # Retry 3 times on failure
delay = 100                 # Wait 100ms between retries
```

### Obtaining Credentials

**API Token**:
1. Login to ChirpStack UI
2. Navigate to Settings → API Keys
3. Create new API key with "api" permission
4. Copy token value

**Tenant ID**:
1. In ChirpStack UI, navigate to Tenants
2. Click on your tenant
3. Copy the tenant ID from URL or details page

---

## [opcua] Section

Configuration for OPC UA server.

### Parameters

| Parameter | Type | Required | Default | Description |
|-----------|------|----------|---------|-------------|
| `application_name` | string | ✓ | - | Display name for OPC UA server |
| `application_uri` | string | ✓ | - | Unique URI (e.g., `urn:company:opcua:gateway`) |
| `product_uri` | string | ✗ | - | Product identifier |
| `host_ip_address` | string | ✗ | 0.0.0.0 | IP address to bind to |
| `host_port` | u16 | ✗ | 4840 | Port to listen on (0-65535, must not be 0) |
| `certificate_path` | string | ✗ | `own/cert.der` | Path to certificate file |
| `private_key_path` | string | ✗ | `private/private.pem` | Path to private key file |
| `pki_dir` | string | ✗ | `./pki` | Directory for PKI files |
| `create_sample_keypair` | bool | ✗ | false | Auto-generate self-signed cert if missing |
| `user_name` | string | ✓ | - | OPC UA client username |
| `user_password` | string | ✓ | - | OPC UA client password |
| `diagnostics_enabled` | bool | ✗ | false | Enable OPC UA diagnostics |
| `trust_client_cert` | bool | ✗ | false | Accept any client certificate |
| `check_cert_time` | bool | ✗ | false | Validate certificate expiration |
| `hello_timeout` | u64 | ✗ | 5 | Seconds to wait for hello message |

### Validation Rules

- `application_name`: Non-empty string
- `application_uri`: Non-empty string (typically URN format)
- `user_name`: Non-empty string
- `user_password`: Non-empty string
- `host_port`: > 0 if specified

### Security Recommendations

```toml
[opcua]
application_name = "My IoT Gateway"
application_uri = "urn:my-company:opcua:gateway"

# Network binding
host_ip_address = "10.0.1.50"   # Bind to specific IP on private network
host_port = 4855

# Authentication
user_name = "operator"
user_password = "strong_password_123"  # or use env var: OPCGW_OPCUA_USER_PASSWORD

# Security (production)
create_sample_keypair = false   # Use proper certs, not sample
trust_client_cert = false       # Verify client certificates
check_cert_time = true          # Reject expired certs
diagnostics_enabled = false     # Reduce attack surface

# PKI
pki_dir = "/etc/opcgw/pki"      # Restricted permissions
certificate_path = "/etc/opcgw/pki/cert.der"
private_key_path = "/etc/opcgw/pki/key.pem"
```

### Example (Development)

```toml
[opcua]
application_name = "My Test Gateway"
application_uri = "urn:my-test:gateway"
host_ip_address = "0.0.0.0"
host_port = 4855
user_name = "admin"
user_password = "password"
pki_dir = "./pki"
create_sample_keypair = true   # OK for testing
```

---

## [[application]] Section

Define ChirpStack applications to expose in OPC UA.

### Top-Level Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `application_name` | string | ✓ | Display name in OPC UA address space |
| `application_id` | string | ✓ | ChirpStack application ID (as string) |

### Validation Rules

- Each application must have unique `application_id`
- `application_name`: Non-empty
- `application_id`: Non-empty
- At least one device per application

### Example

```toml
[[application]]
application_name = "Farm Network"
application_id = "1"
# ... devices follow below

[[application]]
application_name = "Building Automation"
application_id = "2"
# ... devices follow below
```

---

## [[application.device]] Section

Define devices under an application.

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `device_name` | string | ✓ | Display name in OPC UA |
| `device_id` | string | ✓ | ChirpStack device ID |

### Validation Rules

- Each `device_id` must be unique across ALL applications
- `device_name`: Non-empty
- `device_id`: Non-empty
- At least one metric per device

### Example

```toml
[[application]]
application_name = "Farm Network"
application_id = "1"

[[application.device]]
device_name = "Field A Sensor"
device_id = "sensor_001"
# ... metrics follow below

[[application.device]]
device_name = "Field B Sensor"
device_id = "sensor_002"
# ... metrics follow below
```

---

## [[application.device.read_metric]] Section

Define metrics (data points) from a device.

### Parameters

| Parameter | Type | Required | Description |
|-----------|------|----------|-------------|
| `metric_name` | string | ✓ | Display name in OPC UA (the variable name) |
| `chirpstack_metric_name` | string | ✓ | Field name from ChirpStack device data |
| `metric_type` | string | ✓ | Data type: `Float`, `Int`, `Bool`, `String` |
| `metric_unit` | string | ✗ | Unit of measurement (e.g., "°C", "%", "kW") |

### Validation Rules

- `metric_name`: Non-empty
- `chirpstack_metric_name`: Non-empty (must match ChirpStack field exactly)
- `metric_type`: One of `Float`, `Int`, `Bool`, `String`
- `metric_unit`: Optional, any string

### Metric Types

| Type | Example | OPC UA Type | Notes |
|------|---------|------------|-------|
| Float | 23.5, 85.0 | Double | Temperature, humidity, percentages |
| Int | 42, 1000 | Int32 | Counts, thresholds, status codes |
| Bool | true, false | Boolean | On/off, running/stopped, present/absent |
| String | "OK", "ERROR" | String | Status messages, device names |

### Example

```toml
[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "soil_moisture_pct"
metric_type = "Float"
metric_unit = "%"

[[application.device.read_metric]]
metric_name = "Device Status"
chirpstack_metric_name = "status"
metric_type = "String"

[[application.device.read_metric]]
metric_name = "Alert Flag"
chirpstack_metric_name = "alert_active"
metric_type = "Bool"

[[application.device.read_metric]]
metric_name = "Message Count"
chirpstack_metric_name = "message_count"
metric_type = "Int"
```

---

## Complete Configuration Example

```toml
[global]
debug = true

[chirpstack]
server_address = "http://chirpstack.local:8080"
api_token = "your-api-token-here"
tenant_id = "your-tenant-id"
polling_frequency = 10
retry = 3
delay = 100

[opcua]
application_name = "IoT Gateway"
application_uri = "urn:mycompany:opcua:gateway"
host_ip_address = "0.0.0.0"
host_port = 4855
user_name = "admin"
user_password = "changeme"
pki_dir = "./pki"
create_sample_keypair = true

# Application 1: Farm Sensors
[[application]]
application_name = "Farm Network"
application_id = "1"

[[application.device]]
device_name = "Field A - North"
device_id = "farm_001"

[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "moisture"
metric_type = "Float"
metric_unit = "%"

[[application.device.read_metric]]
metric_name = "Temperature"
chirpstack_metric_name = "temp"
metric_type = "Float"
metric_unit = "°C"

[[application.device]]
device_name = "Field B - South"
device_id = "farm_002"

[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "moisture"
metric_type = "Float"
metric_unit = "%"

# Application 2: Building Management
[[application]]
application_name = "Building Automation"
application_id = "2"

[[application.device]]
device_name = "Floor 1 HVAC"
device_id = "hvac_f1"

[[application.device.read_metric]]
metric_name = "Temperature Setpoint"
chirpstack_metric_name = "temp_setpoint"
metric_type = "Float"
metric_unit = "°C"

[[application.device.read_metric]]
metric_name = "Fan Status"
chirpstack_metric_name = "fan_on"
metric_type = "Bool"
```

---

## Environment Variable Overrides

Override any config value via environment variables. Format: `OPCGW_<SECTION>_<FIELD>`

### Examples

```bash
# Override ChirpStack server
export OPCGW_CHIRPSTACK_SERVER_ADDRESS="http://prod-chirpstack:8080"

# Override OPC UA port
export OPCGW_OPCUA_HOST_PORT="4860"

# Override polling frequency
export OPCGW_CHIRPSTACK_POLLING_FREQUENCY="30"

# Run with overrides
cargo run --release
```

### Precedence

1. Environment variables (highest priority)
2. config.toml values
3. Built-in defaults (lowest priority)

---

## Troubleshooting Configuration

### Validation Error: "URL format"

```
Configuration validation failed:
  - chirpstack.server_address: invalid URL format
```

**Fix**: Ensure server_address includes protocol:
```toml
# Wrong
server_address = "localhost:8080"

# Correct
server_address = "http://localhost:8080"
```

### Error: "No applications configured"

```
Configuration validation failed:
  - application_list: must have at least 1 application
```

**Fix**: Add at least one `[[application]]` section with a device and metric.

### Metrics not appearing in OPC UA

**Check**:
1. Are metric names spelled correctly in config vs. ChirpStack?
2. Are they in correct metric type (Float, Int, Bool, String)?
3. Check ChirpStack device details for exact field names
4. Run with `debug = true` and check logs

### "Port already in use"

```
error: Failed to bind to 0.0.0.0:4855
```

**Fix**: Change `host_port` or kill process using it:
```bash
lsof -i :4855
kill <PID>
```

---

## Best Practices

1. **Credentials**: Never hardcode in config files. Use environment variables for production.
   ```bash
   export OPCGW_CHIRPSTACK_API_TOKEN="your-token"
   export OPCGW_OPCUA_USER_PASSWORD="your-password"
   ```

2. **Polling Interval**: Start with 10-30 seconds, adjust based on:
   - How fresh do you need data? (lower = fresher but more load)
   - How many devices? (more devices = less frequent polling)
   - ChirpStack load capacity (ask your admin)

3. **Naming**: Use consistent, clear names
   ```toml
   # Good: descriptive, matches actual location/purpose
   device_name = "Greenhouse A - Temperature Probe"
   metric_name = "Internal Air Temperature"

   # Bad: vague or generic
   device_name = "Sensor 1"
   metric_name = "Data"
   ```

4. **Organization**: Group related applications
   ```toml
   [[application]]
   application_name = "Production - Line 1"
   application_id = "prod_line_1"
   
   [[application]]
   application_name = "Production - Line 2"
   application_id = "prod_line_2"
   ```

5. **Testing**: Validate config before deployment
   ```bash
   cargo run --release 2>&1 | head -20  # Check startup messages
   ```

---

For more details, see the [Quick Start Guide](quickstart.html) and [Architecture](architecture.html) sections.

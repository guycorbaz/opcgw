---
layout: default
title: Quick Start Guide
permalink: /quickstart/
---

## Installation

### Prerequisites

- Rust 1.94.0+ ([Install](https://rustup.rs/))
- Docker & Docker Compose (optional, for containerized deployment)
- Running ChirpStack 4 instance with accessible gRPC API
- OPC UA client software (Ignition, KEPServerEx, UA Expert, etc.) for testing

### From Source

```bash
# Clone repository
git clone https://github.com/guycorbaz/opcgw.git
cd opcgw

# Build
cargo build --release

# Binary location
./target/release/opcgw
```

### Via Docker

```bash
# Build image
docker build -t opcgw:latest .

# Or use pre-built (when available)
docker pull ghcr.io/guycorbaz/opcgw:2.0.0
```

---

## Configuration

### 1. Create Configuration File

```bash
cp config/config.example.toml config/config.toml
```

### 2. Edit Configuration

```toml
# Global settings
[global]
debug = true  # Set to false in production

# ChirpStack connection
[chirpstack]
server_address = "http://your-chirpstack-server:8080"
api_token = "your-api-token-here"           # Get from ChirpStack UI
tenant_id = "your-tenant-id"                # From ChirpStack
polling_frequency = 10                       # Poll every 10 seconds
retry = 3                                    # Retry 3 times
delay = 100                                  # 100ms between retries

# OPC UA server
[opcua]
application_name = "My IoT Gateway"
application_uri = "urn:my-company:opcua:gateway"
host_ip_address = "0.0.0.0"                 # Listen on all interfaces
host_port = 4855                             # Standard OPC UA port
user_name = "admin"
user_password = "secure_password"
pki_dir = "./pki"                            # Certificate storage
create_sample_keypair = true                 # Auto-create certs

# Applications (from ChirpStack)
# Each [[application]] block represents one ChirpStack application
[[application]]
application_name = "Farm Sensors"           # Display name in OPC UA
application_id = "1"                        # ChirpStack app ID

# Devices under this application
[[application.device]]
device_name = "Field A Sensor"
device_id = "device001"

# Metrics from this device
[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "soil_moisture"    # Field name in ChirpStack
metric_type = "Float"
metric_unit = "%"

[[application.device.read_metric]]
metric_name = "Temperature"
chirpstack_metric_name = "temp_celsius"
metric_type = "Float"
metric_unit = "°C"

# Another device in same application
[[application.device]]
device_name = "Field B Sensor"
device_id = "device002"

[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "soil_moisture"
metric_type = "Float"
metric_unit = "%"

# Another application
[[application]]
application_name = "Building Management"
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
metric_name = "Fan Running"
chirpstack_metric_name = "fan_on"
metric_type = "Bool"
```

### 3. Validate Configuration

The gateway validates configuration on startup:
- All required fields present
- server_address is valid URL format
- polling_frequency > 0
- Each device has at least one metric
- No duplicate device IDs
- user_name and user_password not empty

If validation fails, error message will show which field is invalid.

---

## Running the Gateway

### From Source

```bash
# Default config location (./config/config.toml)
cargo run --release

# Custom config location
cargo run --release -- -c /etc/opcgw/config.toml

# With debug logging
cargo run --release -- -c config/config.toml -d
```

### Via Docker

```bash
# Using docker-compose (recommended)
docker-compose up

# Logs will show:
# [INFO] Gateway started successfully
# [INFO] Poll interval: 10s
# [INFO] Applications: 2, Devices: 3
# [INFO] OPC UA endpoint: 0.0.0.0:4855
```

### Output

Successful startup looks like:
```
2026-04-19T12:34:56.789Z  INFO opcgw: starting opcgw
2026-04-19T12:34:56.850Z  INFO opcgw: Gateway started successfully 
                                poll_interval_seconds=10 
                                application_count=2 
                                device_count=3 
                                opc_ua_endpoint=0.0.0.0:4855 
                                chirpstack_server=http://localhost:8080
```

If startup fails, check:
- ChirpStack server is reachable
- API token is correct
- Tenant ID exists in ChirpStack
- No other service on port 4855
- Configuration file is valid TOML

---

## Testing the Gateway

### 1. Verify ChirpStack Connection

Check logs for successful polling:
```
2026-04-19T12:35:06.789Z  INFO opcgw::chirpstack: polled 3 devices
```

### 2. Connect OPC UA Client

**Using UA Expert (free download)**:
1. Launch UA Expert
2. Double-click "Add Server"
3. Enter address: `opc.tcp://localhost:4855`
4. Click "OK"
5. Browse tree:
   ```
   Server
   └── Devices
       ├── Farm Sensors (Application)
       │   ├── Field A Sensor (Device)
       │   │   ├── Soil Moisture (85.3)
       │   │   └── Temperature (22.5)
       │   └── Field B Sensor
       │       └── Soil Moisture (78.2)
       └── Building Management
           └── Floor 1 HVAC
               ├── Temperature Setpoint (21.0)
               └── Fan Running (true)
   ```

**Using Ignition**:
1. Create OPC UA connection in Gateway
2. Address: `opc.tcp://localhost:4855`
3. Username: `admin`
4. Password: (from config)
5. Browse tags and drag to windows

### 3. Check Logs

```bash
# Watch ChirpStack polling
tail -f log/chirpstack.log

# Watch OPC UA activity
tail -f log/opc_ua.log

# Watch all activity
tail -f log/opc_ua_gw.log
```

### 4. Simulate Data Changes

In ChirpStack UI:
1. Navigate to device
2. Change a metric value
3. Within 10 seconds, OPC UA client should show new value
4. Check logs for "metric updated" events

---

## Environment Variables

Override config values via environment variables:

```bash
export OPCGW_CHIRPSTACK_SERVER_ADDRESS="http://prod-chirpstack:8080"
export OPCGW_CHIRPSTACK_API_TOKEN="secret-token"
export OPCGW_OPCUA_HOST_PORT="4860"

cargo run --release
```

**Naming Convention**: `OPCGW_<SECTION>_<FIELD>`
- Replace dots with underscores
- Uppercase all letters
- Example: `opcua.host_port` → `OPCGW_OPCUA_HOST_PORT`

---

## Docker Deployment

### docker-compose.yml

```yaml
version: '3.8'

services:
  opcgw:
    image: opcgw:latest
    container_name: opcgw
    restart: always
    ports:
      - "4855:4855"  # OPC UA
    volumes:
      - ./config:/usr/local/bin/config
      - ./pki:/usr/local/bin/pki
      - ./log:/usr/local/bin/log
    environment:
      OPCGW_CHIRPSTACK_SERVER_ADDRESS: "http://chirpstack:8080"
      OPCGW_CHIRPSTACK_API_TOKEN: "${CHIRPSTACK_TOKEN}"
    depends_on:
      - chirpstack  # Optional, if running locally
```

### Run

```bash
docker-compose up -d

# View logs
docker-compose logs -f opcgw

# Stop
docker-compose down
```

---

## Troubleshooting

### "Failed to connect to ChirpStack"

```
error: connection refused to http://localhost:8080
```

**Check**:
- ChirpStack server is running: `telnet localhost 8080`
- Correct IP/port in config
- Network connectivity (firewall, docker network)

### "Configuration validation failed"

```
error: Configuration validation failed:
  - chirpstack.polling_frequency: must be greater than 0
  - application_list: must have at least 1 application
```

**Fix**: Check config file syntax, required fields, value ranges. See [Configuration](configuration.html).

### "OPC UA port in use"

```
error: Failed to bind OPC UA server to 0.0.0.0:4855
```

**Fix**: Change port in config or kill process using 4855:
```bash
lsof -i :4855
kill <PID>
```

### "Metrics not updating"

**Check**:
1. Is polling working? Look for "polled X devices" in logs
2. Is ChirpStack returning data? Check via `grpcurl` or ChirpStack UI
3. Are metric names correct? Compare with ChirpStack device data
4. Are you connecting to right OPC UA server?

### "OPC UA client can't connect"

```
Connection refused or timeout
```

**Check**:
- Gateway is running: `docker ps` or `ps aux | grep opcgw`
- Port is exposed: `netstat -tlnp | grep 4855`
- Firewall allows port: check local firewall and network ACLs
- Client using correct address: `opc.tcp://host:4855`

---

## Next Steps

- Configure for your specific ChirpStack instance
- Integrate with your SCADA/MES system
- Review [Architecture](architecture.html) for deeper understanding
- Check [Features](features.html) for use case inspiration
- Monitor logs for operational insights

**Questions?** Open an issue on [GitHub](https://github.com/guycorbaz/opcgw/issues).

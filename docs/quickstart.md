---
layout: default
title: Quick Start Guide
subtitle: Install opcgw and reach the web setup wizard in minutes
permalink: /quickstart/
---

> **opcgw is configured from the web UI (v2.3.1).** You provide a bootstrap
> `config.toml` (or the matching `OPCGW_*` environment variables) for the first
> boot; opcgw reads it once into its SQLite database and from then on you manage
> everything — ChirpStack connection, OPC UA settings, and device/metric
> mappings — from the browser. Editing `config.toml` after the first boot has no
> effect.

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

# Or use the published image (Docker Hub primary, GHCR mirror)
docker pull docker.io/gcorbaz/opcgw:2.3
```

---

## Configuration

opcgw is **zero-touch on first boot (v2.3.1)**: you do not edit a config file to
get started. Start the container (or binary) with the shipped placeholder
`config.toml`, open the web UI, and the setup wizard captures everything it needs.

### 1. First boot — the setup wizard (recommended)

1. Start the gateway (see [Running the Gateway](#running-the-gateway) below). With
   a pristine config, opcgw detects that ChirpStack credentials are missing and
   boots straight into first-run mode instead of aborting.
2. Open the web UI in a browser: **`http://<host>:8080/`**. You are redirected to
   the `/setup` wizard.
3. In the wizard, enter:
   - ChirpStack **server address** (`http://your-chirpstack-server:8080`)
   - ChirpStack **tenant ID**
   - ChirpStack **API token**
   - OPC UA **password** (for the OPC UA / web-UI login)
4. Submit. The wizard writes the secrets to `config/secrets.toml` (chmod `0600`)
   and the rest of the configuration to opcgw's SQLite database, then performs an
   in-process soft restart. The container itself is **never** restarted.
5. After the wizard, add your ChirpStack applications, devices and metrics from
   the web UI's **ChirpStack inventory pickers** — no hand-written `[[application]]`
   blocks required.

> No text-file editing is needed for a first boot. The shipped `config.toml` ships
> with `REPLACE_ME_WITH_*` placeholders that the gateway recognises, so it starts,
> serves the wizard, and waits for you in the browser.

### 2. Optional — seed configuration from a TOML file (advanced)

`config.toml` is a **bootstrap seed**: opcgw reads it once on first start to
populate its SQLite database, then ignores it. Editing it after the first boot
has no effect — use the web UI. If you prefer to pre-seed instead of using the
wizard, copy the example and fill in the non-secret fields:

```bash
cp config/config.example.toml config/config.toml
```

```toml
# Global settings
[global]
debug = true  # Set to false in production

# ChirpStack connection
[chirpstack]
server_address = "http://your-chirpstack-server:8080"
# NEVER put a real token inline. Keep the placeholder and inject the real value
# via the wizard, config/secrets.toml, or the OPCGW_CHIRPSTACK__API_TOKEN env var.
api_token = "REPLACE_ME_WITH_OPCGW_CHIRPSTACK__API_TOKEN_ENV_VAR"
tenant_id = "your-tenant-id"                # From ChirpStack
polling_frequency = 10                       # Poll every 10 seconds
retry = 3                                    # Retry 3 times
delay = 100                                  # 100ms between retries

# OPC UA server
[opcua]
application_name = "My IoT Gateway"
application_uri = "urn:my-company:opcua:gateway"
host_ip_address = "0.0.0.0"                 # Listen on all interfaces
host_port = 4840                             # Standard OPC UA port
user_name = "admin"
# NEVER put a real password inline. Keep the placeholder and inject the real value
# via the wizard, config/secrets.toml, or the OPCGW_OPCUA__USER_PASSWORD env var.
user_password = "REPLACE_ME_WITH_OPCGW_OPCUA__USER_PASSWORD_ENV_VAR"
pki_dir = "./pki"                            # Certificate storage
create_sample_keypair = true                 # Auto-create certs

# Applications are optional in the seed — leave the list empty and populate it
# from the web UI's ChirpStack inventory pickers after first boot. A block like
# the one below pre-seeds one application with one device and one metric:
[[application]]
application_name = "Farm Sensors"           # Display name in OPC UA
application_id = "1"                        # ChirpStack app ID

[[application.device]]
device_name = "Field A Sensor"
device_id = "device001"

[[application.device.read_metric]]
metric_name = "Soil Moisture"
chirpstack_metric_name = "soil_moisture"    # Field name in ChirpStack
metric_type = "Float"
metric_unit = "%"
```

> **Secrets are never stored inline in examples.** The `REPLACE_ME_WITH_*`
> placeholders are recognised by the gateway; provide the real values through the
> wizard (which writes `config/secrets.toml`, chmod `0600`) or the
> `OPCGW_*` environment variables.

### 3. Staged "Apply changes" model

After the first boot, configuration edits made in the web UI **stage** into SQLite
rather than taking effect immediately. While changes are pending,
`GET /api/status` reports `pending_changes: true`. You apply all staged edits
together with a single explicit **Apply changes** action in the UI
(`POST /api/config/apply`), which triggers an in-process soft restart of the
data plane. The container is **never** restarted — there is no "restart to pick
up changes" step.

### 4. Validation

The gateway validates configuration on startup:
- server_address is valid URL format
- polling_frequency > 0
- No duplicate device IDs
- Secrets are not left as `REPLACE_ME_WITH_*` placeholders (once configured)

A **pristine install with no applications and missing ChirpStack credentials is
valid** — it boots into the setup wizard rather than failing. An empty
application list is expected before you have run the wizard / inventory discovery.
If validation fails, the error message shows which field is invalid.

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
# [INFO] OPC UA endpoint: 0.0.0.0:4840
```

### Output

Successful startup looks like:
```
2026-04-19T12:34:56.789Z  INFO opcgw: starting opcgw
2026-04-19T12:34:56.850Z  INFO opcgw: Gateway started successfully 
                                poll_interval_seconds=10 
                                application_count=2 
                                device_count=3 
                                opc_ua_endpoint=0.0.0.0:4840 
                                chirpstack_server=http://localhost:8080
```

If startup fails, check:
- ChirpStack server is reachable
- API token is correct
- Tenant ID exists in ChirpStack
- No other service on port 4840
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
3. Enter address: `opc.tcp://localhost:4840`
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
2. Address: `opc.tcp://localhost:4840`
3. Username: `admin`
4. Password: (from config)
5. Browse tags and drag to windows

### 3. Check Logs

```bash
# Watch all gateway activity (one log file; rotates daily)
tail -f log/opcgw.log.*

# Filter to one subsystem
tail -f log/opcgw.log.* | grep 'opcgw::chirpstack'   # ChirpStack polling
tail -f log/opcgw.log.* | grep 'opcgw::opc_ua'       # OPC UA activity

# For deep per-module detail, raise the level (then recreate/restart):
#   OPCGW_LOG_LEVEL=debug
```

### 4. Simulate Data Changes

In ChirpStack UI:
1. Navigate to device
2. Change a metric value
3. Within 10 seconds, OPC UA client should show new value
4. Check logs for "metric updated" events

---

## Environment Variables

Override config values via environment variables. This is the recommended way
to inject secrets (the ChirpStack API token and OPC UA password):

```bash
export OPCGW_CHIRPSTACK__SERVER_ADDRESS="http://prod-chirpstack:8080"
export OPCGW_CHIRPSTACK__API_TOKEN="secret-token"
export OPCGW_OPCUA__HOST_PORT="4860"

cargo run --release
```

**Naming Convention**: `OPCGW_<SECTION>__<FIELD>` — note the **double underscore**
(`__`) between the section and the field.
- Section and field are joined by `__` (two underscores); nested keys use `__` too
- Uppercase all letters
- Examples: `opcua.host_port` → `OPCGW_OPCUA__HOST_PORT`; `chirpstack.api_token` → `OPCGW_CHIRPSTACK__API_TOKEN`; `web.enabled` → `OPCGW_WEB__ENABLED`

Environment variables take precedence over SQLite and `config.toml`.

---

## Docker Deployment

### docker-compose.yml

The repository ships a canonical [`docker-compose.yml`](https://github.com/guycorbaz/opcgw/blob/main/docker-compose.yml); the essentials:

```yaml
services:
  opcgw:
    image: docker.io/gcorbaz/opcgw:2.3
    container_name: opcgw
    restart: always
    ports:
      - "4840:4840"   # OPC UA
      - "8080:8080"   # Web UI (setup wizard + configuration)
    volumes:
      - ./config:/usr/local/bin/config
      - ./pki:/usr/local/bin/pki
      - ./log:/usr/local/bin/log
      - ./data:/usr/local/bin/data   # SQLite DB — REQUIRED so metrics + config persist
    environment:
      # Secrets injected from .env (double underscore between section and field):
      OPCGW_CHIRPSTACK__API_TOKEN: "${OPCGW_CHIRPSTACK__API_TOKEN}"
      OPCGW_OPCUA__USER_PASSWORD: "${OPCGW_OPCUA__USER_PASSWORD}"
      OPCGW_WEB__ENABLED: "true"
```

Bind-mounted directories must be owned by UID 10001 before first start (`sudo chown -R 10001:10001 ./config ./pki ./log ./data`). Without the `./data` mount the SQLite database lives in the ephemeral container layer and is lost on `docker compose down`.

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
```

**Fix**: Check config file syntax, required fields, value ranges. See [Configuration](configuration.html).

Note: an **empty application list is not an error** on a fresh install — opcgw
boots into the setup wizard and you add applications/devices from the web UI's
ChirpStack inventory pickers. You only see validation failures for malformed
values (bad URL, zero polling interval, duplicate device IDs, etc.).

### "OPC UA port in use"

```
error: Failed to bind OPC UA server to 0.0.0.0:4840
```

**Fix**: Change port in config or kill process using 4840:
```bash
lsof -i :4840
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
- Port is exposed: `netstat -tlnp | grep 4840`
- Firewall allows port: check local firewall and network ACLs
- Client using correct address: `opc.tcp://host:4840`

---

## Next Steps

- Configure for your specific ChirpStack instance
- Integrate with your SCADA/MES system
- Review [Architecture](architecture.html) for deeper understanding
- Check [Features](features.html) for use case inspiration
- Monitor logs for operational insights

**Questions?** Open an issue on [GitHub](https://github.com/guycorbaz/opcgw/issues).

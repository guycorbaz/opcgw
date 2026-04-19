# Deployment Guide — opcgw

> Generated: 2026-04-01 | Scan Level: Exhaustive

## Deployment Options

### 1. Native Binary

```bash
# Build release binary
cargo build --release

# Binary location
./target/release/opcgw

# Run with default config
./target/release/opcgw

# Run with custom config
./target/release/opcgw -c /path/to/config.toml
```

**Required files at runtime:**
- `config/config.toml` — Application configuration
- `config/log4rs.yaml` — Logging configuration
- `pki/` — OPC UA certificates directory (with own/, private/, trusted/, rejected/ subdirs)
- `log/` — Log output directory (created automatically)

### 2. Docker

**Dockerfile** uses a multi-stage build:
1. **Builder stage:** `rust:1.87` — installs protobuf compiler, builds release binary
2. **Runtime stage:** `ubuntu:latest` — minimal runtime with `iputils-ping`

```bash
# Build image
docker build -t opcgw .

# Run standalone
docker run -d \
  --name opcgw \
  -p 4855:4855 \
  -v ./config:/usr/local/bin/config \
  -v ./pki:/usr/local/bin/pki \
  -v ./log:/usr/local/bin/log \
  opcgw
```

### 3. Docker Compose

```bash
docker compose up -d
```

**docker-compose.yml configuration:**
- Service: `opcgw`
- Port mapping: `4855:4855`
- Restart policy: `always`
- Volume mounts: `log/`, `config/`, `pki/`

## Network Requirements

| Connection | Protocol | Default Port | Direction | Purpose |
|-----------|----------|-------------|-----------|---------|
| ChirpStack | gRPC (HTTP/2) | 8080 | Outbound | Device metrics polling, command enqueue |
| OPC UA Clients | OPC UA (TCP) | 4840 | Inbound | SCADA/client connections |
| Docker exposed | TCP | 4855 | Inbound | Docker port mapping |

## Configuration for Production

### Security Hardening

1. **Disable null endpoint:** Remove the `null` security endpoint from OPC UA config
2. **Use proper certificates:** Set `create_sample_keypair = false` and provide CA-signed certificates
3. **Strong credentials:** Change default `user1`/`user1` username/password
4. **API token security:** Use environment variables for `api_token` instead of config file:
   ```bash
   export OPCGW_CHIRPSTACK__API_TOKEN="your-secure-token"
   ```
5. **Enable cert time validation:** Ensure `check_cert_time = true`

### Performance Tuning

- **Polling frequency:** Adjust `chirpstack.polling_frequency` based on device update rates
- **Retry settings:** Configure `chirpstack.retry` and `chirpstack.delay` for network resilience
- **Logging levels:** Reduce to `info` or `warn` in production (edit `log4rs.yaml`)

## PKI Certificate Management

```
pki/
├── own/           # Server's own certificate (cert.der)
├── private/       # Server's private key (private.pem)
├── trusted/       # Trusted client CA certificates
└── rejected/      # Auto-rejected unknown client certificates
```

When `create_sample_keypair = true`, the server auto-generates self-signed certificates on first run. For production, place proper certificates in the appropriate directories.

## Health Monitoring

Currently, no dedicated health endpoint exists. Monitor via:
- **Log files:** Check `log/` directory for errors
- **OPC UA diagnostics:** Connect an OPC UA client with diagnostics enabled
- **ChirpStack status:** The gateway tracks server availability internally (exposed as internal metric `cp0`)

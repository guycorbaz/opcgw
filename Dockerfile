#
# Docker file for opc ua chirpstack gateway container
#
# Story B-1 (2026-05-19): runtime base pinned from `ubuntu:latest` to `ubuntu:24.04` (LTS, Noble);
# non-root user `opcgw` (UID 10001) enabled — the binary, static/ assets, and bind-mount
# targets (./pki, ./log, ./config per docker-compose.yml) must be readable by UID 10001.
# Operator-side: chown 10001:10001 ./log ./config ./pki BEFORE the first `docker compose up`
# so the gateway can write log files + read+write SQLite database + read PKI.

ARG RUST_VERSION=1.94.0
ARG APP_NAME=opcgw

# ─────────────────────────────────────────────────────────────────────────────
# Builder stage — compiles the gateway binary against the pinned Rust toolchain.
# ─────────────────────────────────────────────────────────────────────────────
FROM rust:${RUST_VERSION} AS builder
RUN apt-get update && apt-get install protobuf-compiler -y
WORKDIR /usr/src/opcgw
COPY . .
# Build the application
RUN cargo install --path .


# ─────────────────────────────────────────────────────────────────────────────
# Runtime stage — minimal ubuntu base, non-root opcgw user, ENTRYPOINT.
# ─────────────────────────────────────────────────────────────────────────────
FROM ubuntu:24.04

LABEL authors="Guy Corbaz"

RUN apt-get update && apt-get install -y iputils-ping && rm -rf /var/lib/apt/lists/*

# Define work folder
WORKDIR /usr/local/bin

# Create a non-privileged user that opcgw will run under.
# UID 10001 is the convention for application-runtime users (>1000, well outside
# system-UID range, low enough to fit in any reasonable container UID-map).
ARG UID=10001
# `--user-group` (-U) creates a matching group with the same GID as the user,
# so `docker exec opcgw id` reliably reports `gid=10001(opcgw) groups=10001(opcgw)`
# regardless of `/etc/login.defs USERGROUPS_ENAB` setting on the base image.
RUN useradd \
    --home "/nonexistent" \
    --shell "/sbin/nologin" \
    --no-create-home \
    --user-group \
    --uid "${UID}" \
    opcgw

# Pre-create the runtime directories the gateway writes to or reads from,
# chown'd to the non-root user. In production these are typically bind-mounted
# from the host (per docker-compose.yml); pre-creating them lets the container
# start cleanly without an explicit `-v` for `log/` and ensures host-side
# bind mounts inherit a sane reference layout if the host directory is empty.
# Operators bind-mounting host directories MUST still `chown -R 10001:10001`
# those host paths before first start.
RUN mkdir -p /usr/local/bin/log /usr/local/bin/config /usr/local/bin/pki /usr/local/bin/data \
    && chown -R opcgw:opcgw /usr/local/bin/log /usr/local/bin/config /usr/local/bin/pki /usr/local/bin/data

# Copy the executable from the build stage.
# COPY preserves file ownership; we explicitly chown to the non-root user so the
# gateway process can read its own binary.
COPY --from=builder --chown=opcgw:opcgw /usr/local/cargo/bin/opcgw /usr/local/bin/opcgw

# Story 9-1: copy the embedded web server's static placeholder HTML
# next to the binary. Without this, a Docker deployment with
# `[web].enabled = true` would 404 every static file (auth still
# fires correctly; the file dispatch fails behind it). `ServeDir`
# resolves `static/` relative to the gateway's WORKDIR
# (`/usr/local/bin`), so the directory must live there.
# See `docs/security.md § Web UI authentication § Deployment
# requirements` for the systemd / non-container equivalent.
COPY --from=builder --chown=opcgw:opcgw /usr/src/opcgw/static /usr/local/bin/static

# Drop privileges before launching the entrypoint.
USER opcgw

# OPC UA endpoint (default; configurable via `[opcua].host_port` in config.toml).
EXPOSE 4855
# Embedded Axum web UI port (only bound when `[web].enabled = true`; configurable
# via `[web].port`). Declared here as informational metadata; not auto-published.
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/opcgw"]

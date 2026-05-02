#
# Docker file for opc ua chirpstack gateway container
#

ARG RUST_VERSION=1.94.0
ARG APP_NAME=opcgw

# Builder stage
FROM rust:${RUST_VERSION} AS builder
RUN apt-get update && apt-get install protobuf-compiler -y
WORKDIR /usr/src/opcgw
COPY . .
# Build the application
RUN cargo install --path .


# Create the application container
FROM ubuntu:latest

LABEL authors="Guy Corbaz"

RUN apt-get update && apt-get install -y iputils-ping && rm -rf /var/lib/apt/lists/*

# Define work folder
WORKDIR /usr/local/bin


# Create a non-privileged user that opcgw will run under
ARG UID=10001
#RUN useradd \
#    --home "/nonexistant" \
#    --shell "/sbin/nologin" \
#    --no-create-home \
#    --uid "${UID}" \
#    opcgw
# USER opcgw

# Copy the executable from the build stage
COPY --from=builder /usr/local/cargo/bin/opcgw /usr/local/bin/opcgw

# Story 9-1: copy the embedded web server's static placeholder HTML
# next to the binary. Without this, a Docker deployment with
# `[web].enabled = true` would 404 every static file (auth still
# fires correctly; the file dispatch fails behind it). `ServeDir`
# resolves `static/` relative to the gateway's WORKDIR
# (`/usr/local/bin`), so the directory must live there.
# See `docs/security.md § Web UI authentication § Deployment
# requirements` for the systemd / non-container equivalent.
COPY --from=builder /usr/src/opcgw/static /usr/local/bin/static

EXPOSE 4855

ENTRYPOINT ["/usr/local/bin/opcgw"]


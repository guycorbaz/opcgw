# ChirpStack to OPC UA Gateway

This is a gateway between ChirpStack and OPC UA,
enabling communication between IoT devices managed by ChirpStack
and industrial systems using OPC UA.

It implements an opc ua server, where opc ua clients can connect on,
and a chirpstack client, used to poll the chirpstack server.


## Table of Contents

1. [Introduction](#introduction)
2. [Features](#features)
3. [Prerequisites](#prerequisites)
4. [Installation](#installation)
5. [Configuration](#configuration)
6. [Usage](#usage)
7. [Project Structure](#project-structure)
8. [Contributing](#contributing)
9. [License](#license)

## Introduction

The main goal of the Chirpstack to OPC UA gateway is to allow connecting
an SCADA to Chirpstack, in order to collect metrics and send commands
that will be enqueues on the device.

Some SCADA only understand Sparkplug specification. On the other hand,
it's rather difficult to use, using tenant id, application id or
device eui: they are not ver human friendly and can easily lead
to mistakes which can be quite tricky to identify and solve.


## Features

- Communication with ChirpStack server via gRPC API
- Implementation of an OPC UA server
- Management of device metrics
- [Other main features]


## Limitations

Chirpstack propose 5 metric types:
- Unknown/Unset
- Counter
- Absolute
- Gauge
- String
At the time being, only gauge is supported.


## Prerequisites

- Rust [version]
- [Other dependencies]


## Installation

[Detailed installation instructions]


## Configuration

At the moment, opc_ua_chirpstack_gateway is configured via two configuration files
- One configuration file for the gateway
- One configuration file for the opc ua server. This might change in the future.


## Usage
 
[Instructions on how to use the application]


## Project Structure

[Description of folder structure and main files]


## Contributing

[Instructions for contributing to the project]


## License

[License information]

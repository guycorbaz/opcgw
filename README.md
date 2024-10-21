![build and test](https://github.com/guycorbaz/opcgw/actions/workflows/ci.yml/badge.svg)
![clippy](https://github.com/guycorbaz/opcgw/actions/workflows/clippy.yml/badge.svg)

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

Some SCADA only understand Sparkplug mqtt specification.
On the other hand chirpstack entities mainly relies on chirpstack generated ids,
that are not really user friendly, which might lead to mistakes when configuring systems.

I wanted to learn rust and decided to develop this gateway with this language.
The program is certainly not designed with the best practices of rust language and
certainly needs some improvements. That's why opc_ua_chirpstack_gateway is under heavy development.

## Features

- Communication with ChirpStack server via gRPC API
- Implementation of an OPC UA server
- Management of device metrics via configuration file


## Limitations

Chirpstack propose 5 metric types:
- Unknown/Unset
- Counter
- Absolute
- Gauge
- String

At the time being, only gauge is supported.


## Prerequisites

At the time being, the application works only on linux.


## Installation

Installation consists in:
- creating a gateway configuration file
- creating an opc ua configuration file
- launching the program.
Log files are under log folder just below opc_ua_chirpstack_gateway binary.


## Configuration

At the moment, opc_ua_chirpstack_gateway is configured via two configuration files.

- One configuration file for the gateway
- One configuration file for the opc ua server. This might change in the future.

Default location of these files is config folder, located just below the root of the
opc_ua_chirpstack_gateway binary file. It is possible to configure the path of
the gateway configuration file via "CONFIG_PATH" environment variable. However, the path
of the opc ua configuration file is defined in the gateway configuration file.
This is not ideal and will certainly be changed in the future.

## Usage
 
[Instructions on how to use the application][]()


## Project Structure

The project is organized in the following way:
- main.rs: the main rust file
- config.rs: to manage configurations
- chirpstack.rs: containing  structures and methods for communications with chirpstack server
- opc_ua.rs: containing the code for the opc ua server
- storage.rs: managing data storage
- utils.rs: definition for the  whole project

This organization might change in the future.

## Contributing

[Instructions for contributing to the project]


## License

MIT OR Apache-2.0.

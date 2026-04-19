![build and test](https://github.com/guycorbaz/opcgw/actions/workflows/ci.yml/badge.svg)

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
On the other hand Chirpstack entities mainly relies on Chirpstack generated ids,
that are not really user friendly, which might lead to mistakes when configuring systems.

I wanted to control my LoRa watering valves, in my fruit tree orchards, via my [SCADA](https://en.wikipedia.org/wiki/SCADA),
in order to optimize water use.
I also wanted to learn rust. Therefore I decided to develop this gateway with this language.
The program is certainly not designed with the best practices of rust language and
certainly needs some improvements. That's why *opcgw* is under heavy development.

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

The gateway has been tested only with [fuxa](https://github.com/frangoteam/FUXA) scada for the moment.


## Prerequisites

At the time being, the application has only been tested on linux.


## Installation

The software installation steps are:
- install rust
- clone opcgw repository (git clone https://github.com/guycorbaz/opcgw.git)
- edit the gateway configuration file
- edit the opcua opc ua configuration file
- launching the program.
Log files are under log folder just below opc_ua_chirpstack_gateway binary.

It also possible to run opcgw as a docker container.cargo clean

## Configuration

At the moment, opc_ua_chirpstack_gateway is configured via two configuration files.

- One configuration file for the gateway
- One configuration file for the opc ua server. This might change in the future.

Default location of these files is config folder, located just below the root of the
*opc_ua_chirpstack_gateway* binary file. It is possible to configure the path of
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

Any contributions you make are greatly appreciated. If you identify any errors,
or have an idea for an improvement, please open an [issue](https://github.com/guycorbaz/opcgw/issues).
But before filing a new issue, please look through already existing issues. Search open and closed issues first.

Non-code contributions are also highly appreciated, such as improving the documentation
or promoting opcgw on social media.


## License

MIT OR Apache-2.0.

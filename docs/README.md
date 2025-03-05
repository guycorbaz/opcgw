# opcgw

**opcgw** is a [Chirptack](https://www.chirpstack.io/) to [OPC UA](https://opcfoundation.org) gateway,
enabling communication between IoT devices managed by *ChirpStack*
and industrial systems using *OPC UA*.

It implements an opc ua server, where opc ua clients can connect on,
and a *Chirptack* client, used to poll the *Chirptack* server.


## Introduction

The main goal of the *Chirptack* to OPC UA gateway is to allow connecting
an SCADA to *Chirptack*, in order to collect metrics and send commands
that will be enqueues on the device.

Some SCADA only understand *Sparkplug" mqtt specification.
On the other hand *Chirptack* entities mainly relies on *Chirptack* generated ids,
that are not really user friendly, which might lead to mistakes when configuring systems.

I wanted to control my LoRa watering valves, in my fruit tree orchards, via my SCADA, in order to optimize water use.
I also wanted to learn rust. Therefore I decided to develop this gateway with this language.
The program is certainly not designed with the best practices of rust language and
certainly needs some improvements. That's why *opc_ua_chirpstack_gateway* is under heavy development.


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
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

## Status

**opcgw** has reach a maturity level that allows to use it in production, with certain car. It does not implement all
OPC UA features, but it is enough for my needs and it do the job.

## Features

- Communication with ChirpStack server via [gRPC](https://grpc.io/) API
- Implementation of an [OPC UA](https://opcfoundation.org) server
- Management of device metrics via configuration file

## Improvements

If you have any idea, feel free to open an issue, or better, a pull request.
// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! OpcGW library interface for integration testing.
//!
//! This module re-exports the main components for use in integration tests.

pub mod chirpstack;
/// Story C-1: ChirpStack inventory layer — types + cache + stream helper.
pub mod chirpstack_inventory;
/// Story C-1: locally-compiled ChirpStack proto types.
///
/// The `chirpstack_api` cargo dependency does NOT export
/// `InternalServiceClient` / `StreamDeviceEventsRequest` / `LogItem` — its
/// `internal` module is the LoRaWAN-stack types (DeviceSession etc.), not
/// the gRPC client for `InternalService`. `build.rs` already compiles
/// `proto/chirpstack/api/internal.proto`; this module exposes those
/// generated types under `crate::chirpstack_internal_proto::*` so the
/// inventory layer (`src/chirpstack_inventory.rs`) can open the
/// `InternalService.StreamDeviceEvents` stream.
///
/// Note: this includes ALL types from the `api` proto package
/// (ApplicationServiceClient, DeviceServiceClient, etc. — duplicate of the
/// chirpstack_api crate's definitions). The duplicate types are
/// functionally identical but distinct at the type-system level; cross-
/// module use requires explicit `From` impls. The inventory layer ONLY
/// consumes `InternalServiceClient` + `StreamDeviceEventsRequest` +
/// `LogItem` from this module — the rest of the codebase continues to use
/// the `chirpstack_api` crate.
pub mod chirpstack_internal_proto {
    // api.rs references `super::common::*` so `common` must sit at the
    // same module level (a sibling of `api`). build.rs compiles both
    // proto packages; nest them so `super::common::*` resolves correctly
    // from inside `api`.
    pub mod common {
        tonic::include_proto!("common");
    }
    pub mod api {
        tonic::include_proto!("api");
    }
}
pub mod command_validation;
pub mod config;
pub mod config_reload;
pub mod opc_ua;
pub mod opc_ua_auth;
pub mod opc_ua_history;
pub mod opc_ua_session_monitor;
pub mod opcua_topology_apply;
pub mod security;
pub mod security_hmac;
pub mod storage;
pub mod utils;
pub mod web;

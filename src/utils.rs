// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Global utilities for the program
//!

#![allow(unused)]


/// opc ua address space in which chirpstack variables
/// are registered in
pub const OPCUA_ADDRESS_SPACE: &str = "urn:chirpstack_opcua";

use std::string::ToString;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpcGwError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
    #[error("ChirpStack error: {0}")]
    ChirpStackError(String),
    #[error("OPC UA error: {0}")]
    OpcUaError(String),
    #[error("Storage error: {0}")]
    StorageError(String),
}

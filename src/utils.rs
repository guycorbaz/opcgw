// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Global utilities for the program
//!

#![allow(unused)]

pub const OPCUA_ADDRESS_SPACE: &str = "urn:chirpstack_opcua";
pub const OPCUA_NAMESPACE_URI: &str = "urn:UpcUaG";
pub const OPCUA_DEFAULT_NETWORK_TIMEOUT: u32 = 5;
pub const OPCUA_DEFAULT_IP_ADDRESS: &str = "127.0.0.1";
pub const OPCUA_DEFAULT_PORT: u16 = 4840;

/// Configuration files are stored in the following folder
pub const OPCGW_CONFIG_PATH: &str = "config";
pub const OPCGW_CONFIG_FILE: &str = "config.toml";

/// Chirpstack metrics configuration
/// opcgw chirpstack server name
pub const OPCGW_CP_NAME: &str = "Chirpstack";
/// opc ua variable name for chirpstack availability
pub const OPCGW_CP_AVAILABILITY_NAME: &str = "Chirpstack_available";
/// opc ua variable name for response time
pub const OPCGW_CP_RESPONSE_TIME_NAME: &str = "ResponseTime";
/// Chirpstack device id for opcgw internal use
pub const OPCGW_CP_ID: &str = "cp0";

use std::string::ToString;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum OpcGwError {
    #[error("Configuration error: {0}")]
    ConfigurationError(String),
    #[error("ChirpStack poller error: {0}")]
    ChirpStackError(String),
    #[error("OPC UA error: {0}")]
    OpcUaError(String),
    #[error("Storage error: {0}")]
    StorageError(String),
}

/// Prints the type name of the provided reference.
///
/// # Arguments
///
/// * `_` - A reference to any type
///
/// # Examples
///
/// ```
/// let x = 42;
/// print_type_of(&x); // This will print "i32"
/// ```
///
/// This function uses Rust's `std::any::type_name` to determine the type name
/// of the referenced value and prints it to the standard output.
pub fn print_type_of<T>(_: &T) {
    println!("{}", std::any::type_name::<T>())
}

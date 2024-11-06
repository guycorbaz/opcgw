// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]

//! Global utilities for the program
//!

#![allow(unused)]


/// opc ua address space in which chirpstack variables
/// are registered in
pub const OPCUA_ADDRESS_SPACE: &str = "urn:chirpstack_opcua";

/// Configuration files are stored in the following folder
pub const OPCGW_CONFIG_PATH: &str = "config";

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

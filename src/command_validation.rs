// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Command Parameter Validation Module
//!
//! Provides schema-driven validation of command parameters before enqueuing.
//! Ensures that commands match device command definitions and enforces type safety.

// Module is scaffolded for Epic 7 / Epic 9 wiring (validation flow not yet
// connected end-to-end). Allow dead_code at module scope so the scaffold
// stays without per-item annotations; reassess when the validation flow
// gets its first real call site.
#![allow(dead_code)]

use serde_json::Value;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use crate::utils::OpcGwError;

/// Parameter type definitions for command schemas.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub enum ParameterType {
    /// String parameter with optional max length constraint
    String {
        #[serde(default = "default_string_max_length")]
        max_length: usize,
    },
    /// Integer parameter with range constraints
    Int { min: i64, max: i64 },
    /// Float parameter with range and optional precision constraints
    Float {
        min: f64,
        max: f64,
        #[serde(default)]
        decimal_places: Option<u32>,
    },
    /// Boolean parameter (no constraints)
    Bool,
    /// Enumeration parameter with allowed values
    Enum {
        values: Vec<String>,
        #[serde(default)]
        case_sensitive: bool,
    },
}

const DEFAULT_STRING_MAX_LENGTH: usize = 256;

fn default_string_max_length() -> usize {
    DEFAULT_STRING_MAX_LENGTH
}

/// Definition of a single parameter in a command schema
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ParameterDef {
    /// Parameter name
    pub name: String,
    /// Parameter type and constraints
    pub param_type: ParameterType,
    /// Whether this parameter is required
    pub required: bool,
    /// Optional description for documentation
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Command schema definition for a single command
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct CommandSchema {
    /// Command name (e.g., "set_temperature", "toggle_relay")
    pub command_name: String,
    /// List of parameter definitions
    pub parameters: Vec<ParameterDef>,
    /// Optional description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl CommandSchema {
    /// Validates the schema itself for consistency
    pub fn validate_schema(&self) -> Result<(), String> {
        for param in &self.parameters {
            match &param.param_type {
                ParameterType::Enum {
                    values,
                    case_sensitive,
                } => {
                    // Reject empty enum values
                    if values.is_empty() {
                        return Err(format!(
                            "Enum parameter '{}' has no allowed values",
                            param.name
                        ));
                    }
                    // Check for duplicate enum values (case-insensitive)
                    if !case_sensitive {
                        let mut seen = std::collections::HashSet::new();
                        for val in values {
                            let lower = val.to_lowercase();
                            if !seen.insert(lower) {
                                return Err(format!(
                                    "Enum parameter '{}' has duplicate values (case-insensitive): {}",
                                    param.name, val
                                ));
                            }
                        }
                    }
                }
                ParameterType::Float {
                    decimal_places: Some(places),
                    ..
                }
                    // Validate decimal places is in safe range
                    if *places > 20 => {
                        return Err(format!(
                            "Float parameter '{}' decimal_places {} exceeds safe limit of 20",
                            param.name, places
                        ));
                    }
                _ => {}
            }
        }
        Ok(())
    }

    /// Validates command parameters against this schema
    pub fn validate_parameters(&self, params: &Value) -> Result<(), String> {
        // Convert JSON value to map if it's an object
        let params_map = match params {
            Value::Object(map) => {
                let mut result = HashMap::new();
                for (k, v) in map.iter() {
                    result.insert(k.clone(), v.clone());
                }
                result
            }
            Value::Null => HashMap::new(),
            _ => return Err("Parameters must be a JSON object or null".to_string()),
        };

        // Check required parameters
        for param_def in &self.parameters {
            if param_def.required && !params_map.contains_key(&param_def.name) {
                return Err(format!(
                    "Required parameter '{}' is missing",
                    param_def.name
                ));
            }
        }

        // Validate each provided parameter
        for (name, value) in &params_map {
            // Find the parameter definition
            let param_def = self
                .parameters
                .iter()
                .find(|p| &p.name == name)
                .ok_or_else(|| format!("Unknown parameter '{}'", name))?;

            // Validate the parameter value
            validate_parameter_value(param_def, value)?;
        }

        Ok(())
    }
}

/// Validates a single parameter value against its definition
fn validate_parameter_value(param_def: &ParameterDef, value: &Value) -> Result<(), String> {
    // Check null values - reject if required, allow if optional
    if value.is_null() {
        if param_def.required {
            return Err(format!("Required parameter '{}' cannot be null", param_def.name));
        }
        return Ok(());
    }

    match &param_def.param_type {
        ParameterType::String { max_length } => {
            let s = value
                .as_str()
                .ok_or_else(|| format!("Parameter '{}' must be a string", param_def.name))?;

            let char_count = s.chars().count();
            if char_count > *max_length {
                return Err(format!(
                    "Parameter '{}' exceeds max length of {} characters (got {})",
                    param_def.name,
                    max_length,
                    char_count
                ));
            }
            Ok(())
        }
        ParameterType::Int { min, max } => {
            let n = value.as_i64().ok_or_else(|| {
                format!("Parameter '{}' must be an integer", param_def.name)
            })?;

            if n < *min || n > *max {
                return Err(format!(
                    "Parameter '{}' must be in range [{}, {}], got {}",
                    param_def.name, min, max, n
                ));
            }
            Ok(())
        }
        ParameterType::Float {
            min,
            max,
            decimal_places,
        } => {
            let f = value.as_f64().ok_or_else(|| {
                format!("Parameter '{}' must be a number", param_def.name)
            })?;

            // Reject NaN and Infinity
            if f.is_nan() || f.is_infinite() {
                return Err(format!(
                    "Parameter '{}' must be a valid number (not NaN or Infinity)",
                    param_def.name
                ));
            }

            if f < *min || f > *max {
                return Err(format!(
                    "Parameter '{}' must be in range [{}, {}], got {}",
                    param_def.name, min, max, f
                ));
            }

            // Check decimal places if specified
            if let Some(places) = decimal_places {
                // Clamp places to safe range to prevent overflow
                if *places > 20 {
                    return Err(format!(
                        "Parameter '{}' configuration error: decimal_places exceeds safe limit",
                        param_def.name
                    ));
                }
                let multiplier = 10_f64.powi(*places as i32);
                let rounded = (f * multiplier).round() / multiplier;
                // Use adaptive relative tolerance instead of fixed epsilon
                let tolerance = (f.abs() * 1e-9).max(1e-9);
                if (f - rounded).abs() > tolerance {
                    return Err(format!(
                        "Parameter '{}' has too many decimal places (max {})",
                        param_def.name, places
                    ));
                }
            }

            Ok(())
        }
        ParameterType::Bool => {
            value
                .as_bool()
                .ok_or_else(|| format!("Parameter '{}' must be a boolean", param_def.name))?;
            Ok(())
        }
        ParameterType::Enum {
            values,
            case_sensitive,
        } => {
            // Empty enum validation done in CommandSchema::validate_schema()
            let s = value
                .as_str()
                .ok_or_else(|| format!("Parameter '{}' must be a string", param_def.name))?;

            let matched = if *case_sensitive {
                values.iter().any(|v| v == s)
            } else {
                let s_lower = s.to_lowercase();
                values
                    .iter()
                    .any(|v| v.to_lowercase() == s_lower)
            };

            if !matched {
                return Err(format!(
                    "Parameter '{}' has invalid enum value '{}', allowed values: {}",
                    param_def.name,
                    s,
                    values.join(", ")
                ));
            }

            Ok(())
        }
    }
}

/// `(schemas, last-refresh)` entries kept in `CommandSchemaCache`. Aliased so
/// the storage type stays under clippy's `type_complexity` threshold.
type SchemaCacheEntries = HashMap<String, (Vec<CommandSchema>, SystemTime)>;

/// Caches command schemas with TTL-based expiration
pub struct CommandSchemaCache {
    schemas: Arc<Mutex<SchemaCacheEntries>>,
    ttl: Duration,
}

impl CommandSchemaCache {
    /// Creates a new command schema cache with the specified TTL
    pub fn new(ttl_secs: u64) -> Self {
        Self {
            schemas: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_secs(ttl_secs),
        }
    }

    /// Gets schemas for a device, returning None if expired or not cached
    pub fn get(&self, device_id: &str) -> Option<Vec<CommandSchema>> {
        match self.schemas.lock() {
            Ok(cache) => {
                if let Some((schemas, timestamp)) = cache.get(device_id) {
                    // Check if entry is still valid (check and clone must be atomic under lock)
                    if let Ok(elapsed) = timestamp.elapsed() {
                        if elapsed < self.ttl {
                            // Clone while holding lock to avoid race condition
                            return Some(schemas.clone());
                        }
                    }
                }
                None
            }
            Err(_) => {
                // Lock poisoned - cache is unusable
                None
            }
        }
    }

    /// Stores schemas for a device
    pub fn insert(&self, device_id: String, schemas: Vec<CommandSchema>) {
        // Log but don't panic on lock poisoning
        if let Ok(mut cache) = self.schemas.lock() {
            cache.insert(device_id, (schemas, SystemTime::now()));
        }
    }

    /// Clears all cached schemas
    pub fn clear(&self) {
        // Log but don't panic on lock poisoning
        if let Ok(mut cache) = self.schemas.lock() {
            cache.clear();
        }
    }
}

/// Command validator with schema-based validation
pub struct CommandValidator {
    schema_cache: CommandSchemaCache,
    device_schemas: HashMap<String, Vec<CommandSchema>>,
    #[allow(dead_code)]
    strict_precision_mode: bool,
}

impl CommandValidator {
    /// Creates a new command validator with configuration
    pub fn new(
        cache_ttl_secs: u64,
        strict_precision_mode: bool,
        device_schemas: HashMap<String, Vec<CommandSchema>>,
    ) -> Self {
        // Clone device_schemas to prevent external mutations from affecting the validator
        let device_schemas = device_schemas.clone();
        Self {
            schema_cache: CommandSchemaCache::new(cache_ttl_secs),
            device_schemas,
            strict_precision_mode,
        }
    }

    /// Validates command parameters against the device's schema
    pub fn validate_command_parameters(
        &self,
        device_id: &str,
        command_name: &str,
        parameters: &Value,
    ) -> Result<(), OpcGwError> {
        // Get schemas for this device
        let schemas = self.get_device_schemas(device_id)?;

        // Find the command schema
        let schema = schemas
            .iter()
            .find(|s| s.command_name == command_name)
            .ok_or_else(|| {
                OpcGwError::CommandValidation {
                    device_id: device_id.to_string(),
                    command_name: command_name.to_string(),
                    reason: format!(
                        "Command '{}' not found in device schema",
                        command_name
                    ),
                }
            })?;

        // Validate parameters
        schema.validate_parameters(parameters).map_err(|reason| {
            OpcGwError::CommandValidation {
                device_id: device_id.to_string(),
                command_name: command_name.to_string(),
                reason,
            }
        })?;

        Ok(())
    }

    /// Gets schemas for a device, checking cache first
    fn get_device_schemas(&self, device_id: &str) -> Result<Vec<CommandSchema>, OpcGwError> {
        // Check cache first
        if let Some(schemas) = self.schema_cache.get(device_id) {
            return Ok(schemas);
        }

        // Check configured schemas
        if let Some(schemas) = self.device_schemas.get(device_id) {
            // Cache the schemas
            self.schema_cache.insert(device_id.to_string(), schemas.clone());
            return Ok(schemas.clone());
        }

        Err(OpcGwError::CommandValidation {
            device_id: device_id.to_string(),
            command_name: String::new(),
            reason: format!("No command schema found for device '{}'", device_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_parameter_validation_valid() {
        let param_def = ParameterDef {
            name: "name".to_string(),
            param_type: ParameterType::String { max_length: 256 },
            required: true,
            description: None,
        };

        let value = Value::String("test".to_string());
        assert!(validate_parameter_value(&param_def, &value).is_ok());
    }

    #[test]
    fn test_string_parameter_validation_too_long() {
        let param_def = ParameterDef {
            name: "name".to_string(),
            param_type: ParameterType::String { max_length: 5 },
            required: true,
            description: None,
        };

        let value = Value::String("toolong".to_string());
        let result = validate_parameter_value(&param_def, &value);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds max length"));
    }

    #[test]
    fn test_int_parameter_validation_in_range() {
        let param_def = ParameterDef {
            name: "count".to_string(),
            param_type: ParameterType::Int { min: 0, max: 255 },
            required: true,
            description: None,
        };

        let value = Value::Number(42.into());
        assert!(validate_parameter_value(&param_def, &value).is_ok());
    }

    #[test]
    fn test_int_parameter_validation_out_of_range() {
        let param_def = ParameterDef {
            name: "count".to_string(),
            param_type: ParameterType::Int { min: 0, max: 255 },
            required: true,
            description: None,
        };

        let value = Value::Number(300.into());
        let result = validate_parameter_value(&param_def, &value);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must be in range"));
    }

    #[test]
    fn test_float_parameter_validation() {
        let param_def = ParameterDef {
            name: "temp".to_string(),
            param_type: ParameterType::Float {
                min: 0.0,
                max: 100.0,
                decimal_places: Some(2),
            },
            required: true,
            description: None,
        };

        let value = serde_json::json!(25.5);
        assert!(validate_parameter_value(&param_def, &value).is_ok());
    }

    #[test]
    fn test_enum_parameter_validation_case_sensitive() {
        let param_def = ParameterDef {
            name: "state".to_string(),
            param_type: ParameterType::Enum {
                values: vec!["on".to_string(), "off".to_string()],
                case_sensitive: true,
            },
            required: true,
            description: None,
        };

        let value = Value::String("on".to_string());
        assert!(validate_parameter_value(&param_def, &value).is_ok());

        let value = Value::String("ON".to_string());
        assert!(validate_parameter_value(&param_def, &value).is_err());
    }

    #[test]
    fn test_enum_parameter_validation_case_insensitive() {
        let param_def = ParameterDef {
            name: "state".to_string(),
            param_type: ParameterType::Enum {
                values: vec!["on".to_string(), "off".to_string()],
                case_sensitive: false,
            },
            required: true,
            description: None,
        };

        let value = Value::String("ON".to_string());
        assert!(validate_parameter_value(&param_def, &value).is_ok());
    }

    #[test]
    fn test_required_parameter_enforcement() {
        let schema = CommandSchema {
            command_name: "toggle".to_string(),
            parameters: vec![
                ParameterDef {
                    name: "state".to_string(),
                    param_type: ParameterType::Bool,
                    required: true,
                    description: None,
                },
            ],
            description: None,
        };

        // Valid: parameter provided
        let params = serde_json::json!({"state": true});
        assert!(schema.validate_parameters(&params).is_ok());

        // Invalid: required parameter missing
        let params = serde_json::json!({});
        assert!(schema.validate_parameters(&params).is_err());
    }

    #[test]
    fn test_optional_parameter_enforcement() {
        let schema = CommandSchema {
            command_name: "set_config".to_string(),
            parameters: vec![
                ParameterDef {
                    name: "value".to_string(),
                    param_type: ParameterType::String { max_length: 256 },
                    required: false,
                    description: None,
                },
            ],
            description: None,
        };

        // Valid: optional parameter omitted
        let params = serde_json::json!({});
        assert!(schema.validate_parameters(&params).is_ok());

        // Valid: optional parameter provided
        let params = serde_json::json!({"value": "test"});
        assert!(schema.validate_parameters(&params).is_ok());
    }

    #[test]
    fn test_command_schema_cache() {
        let cache = CommandSchemaCache::new(3600);
        let schema = CommandSchema {
            command_name: "test".to_string(),
            parameters: vec![],
            description: None,
        };

        // Insert and retrieve
        cache.insert("device1".to_string(), vec![schema.clone()]);
        let retrieved = cache.get("device1");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap()[0].command_name, "test");

        // Non-existent device returns None
        assert!(cache.get("device2").is_none());
    }

    #[test]
    fn test_validator_validates_command() {
        let mut device_schemas = HashMap::new();
        device_schemas.insert(
            "dev1".to_string(),
            vec![CommandSchema {
                command_name: "set_temp".to_string(),
                parameters: vec![
                    ParameterDef {
                        name: "value".to_string(),
                        param_type: ParameterType::Float {
                            min: 0.0,
                            max: 100.0,
                            decimal_places: None,
                        },
                        required: true,
                        description: None,
                    },
                ],
                description: None,
            }],
        );

        let validator = CommandValidator::new(3600, false, device_schemas);
        let params = serde_json::json!({"value": 25.5});
        assert!(validator
            .validate_command_parameters("dev1", "set_temp", &params)
            .is_ok());
    }

    #[test]
    fn test_validator_rejects_unknown_device() {
        let device_schemas = HashMap::new();
        let validator = CommandValidator::new(3600, false, device_schemas);
        let params = serde_json::json!({});
        let result = validator.validate_command_parameters("unknown_dev", "cmd", &params);
        assert!(result.is_err());
        match result.unwrap_err() {
            OpcGwError::CommandValidation { device_id, .. } => {
                assert_eq!(device_id, "unknown_dev");
            }
            _ => panic!("Expected CommandValidation error"),
        }
    }
}

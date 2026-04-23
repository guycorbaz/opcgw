// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] Guy Corbaz

//! Command Validation Tests
//!
//! Comprehensive test suite for Story 3-2: Command Parameter Validation
//! Tests all acceptance criteria and edge cases

use opcgw::command_validation::{
    CommandSchema, CommandValidator, ParameterDef, ParameterType, CommandSchemaCache,
};
use opcgw::utils::OpcGwError;
use std::collections::HashMap;
use std::sync::Arc;

// ============================================================================
// AC#1: Command Schema Binding
// ============================================================================

#[test]
fn test_ac1_load_command_schema() {
    let schema = CommandSchema {
        command_name: "set_temperature".to_string(),
        parameters: vec![
            ParameterDef {
                name: "value".to_string(),
                param_type: ParameterType::Float {
                    min: 0.0,
                    max: 100.0,
                    decimal_places: Some(2),
                },
                required: true,
                description: Some("Temperature in Celsius".to_string()),
            },
        ],
        description: Some("Set device temperature".to_string()),
    };

    assert_eq!(schema.command_name, "set_temperature");
    assert_eq!(schema.parameters.len(), 1);
    assert_eq!(schema.parameters[0].name, "value");
    match &schema.parameters[0].param_type {
        ParameterType::Float { min, max, .. } => {
            assert_eq!(*min, 0.0);
            assert_eq!(*max, 100.0);
        }
        _ => panic!("Expected Float parameter type"),
    }
}

// ============================================================================
// AC#2: Parameter Type Validation
// ============================================================================

#[test]
fn test_ac2_string_parameter_validation_valid() {
    let schema = CommandSchema {
        command_name: "set_name".to_string(),
        parameters: vec![ParameterDef {
            name: "name".to_string(),
            param_type: ParameterType::String { max_length: 256 },
            required: true,
            description: None,
        }],
        description: None,
    };

    let params = serde_json::json!({"name": "Device01"});
    assert!(schema.validate_parameters(&params).is_ok());
}

#[test]
fn test_ac2_string_parameter_out_of_range_length() {
    let schema = CommandSchema {
        command_name: "set_name".to_string(),
        parameters: vec![ParameterDef {
            name: "name".to_string(),
            param_type: ParameterType::String { max_length: 5 },
            required: true,
            description: None,
        }],
        description: None,
    };

    let params = serde_json::json!({"name": "VeryLongNameString"});
    assert!(schema.validate_parameters(&params).is_err());
}

#[test]
fn test_ac2_int_parameter_range_validation() {
    let schema = CommandSchema {
        command_name: "set_count".to_string(),
        parameters: vec![ParameterDef {
            name: "count".to_string(),
            param_type: ParameterType::Int { min: 0, max: 255 },
            required: true,
            description: None,
        }],
        description: None,
    };

    // Valid: within range
    assert!(schema.validate_parameters(&serde_json::json!({"count": 100})).is_ok());

    // Invalid: below min
    assert!(schema
        .validate_parameters(&serde_json::json!({"count": -1}))
        .is_err());

    // Invalid: above max
    assert!(schema
        .validate_parameters(&serde_json::json!({"count": 256}))
        .is_err());
}

#[test]
fn test_ac2_float_parameter_range_validation() {
    let schema = CommandSchema {
        command_name: "set_temp".to_string(),
        parameters: vec![ParameterDef {
            name: "temp".to_string(),
            param_type: ParameterType::Float {
                min: 0.0,
                max: 100.0,
                decimal_places: None,
            },
            required: true,
            description: None,
        }],
        description: None,
    };

    // Valid: within range
    assert!(schema.validate_parameters(&serde_json::json!({"temp": 25.5})).is_ok());

    // Invalid: out of range
    assert!(schema.validate_parameters(&serde_json::json!({"temp": 150.0})).is_err());
}

#[test]
fn test_ac2_bool_parameter_validation() {
    let schema = CommandSchema {
        command_name: "toggle".to_string(),
        parameters: vec![ParameterDef {
            name: "state".to_string(),
            param_type: ParameterType::Bool,
            required: true,
            description: None,
        }],
        description: None,
    };

    assert!(schema.validate_parameters(&serde_json::json!({"state": true})).is_ok());
    assert!(schema.validate_parameters(&serde_json::json!({"state": false})).is_ok());
    assert!(schema.validate_parameters(&serde_json::json!({"state": "invalid"})).is_err());
}

#[test]
fn test_ac2_enum_parameter_validation() {
    let schema = CommandSchema {
        command_name: "set_mode".to_string(),
        parameters: vec![ParameterDef {
            name: "mode".to_string(),
            param_type: ParameterType::Enum {
                values: vec!["on".to_string(), "off".to_string(), "auto".to_string()],
                case_sensitive: true,
            },
            required: true,
            description: None,
        }],
        description: None,
    };

    assert!(schema.validate_parameters(&serde_json::json!({"mode": "on"})).is_ok());
    assert!(schema.validate_parameters(&serde_json::json!({"mode": "invalid"})).is_err());
}

// ============================================================================
// AC#3: Required Parameter Enforcement
// ============================================================================

#[test]
fn test_ac3_required_parameter_missing() {
    let schema = CommandSchema {
        command_name: "set_value".to_string(),
        parameters: vec![ParameterDef {
            name: "value".to_string(),
            param_type: ParameterType::Float {
                min: 0.0,
                max: 100.0,
                decimal_places: None,
            },
            required: true,
            description: None,
        }],
        description: None,
    };

    // Missing required parameter
    assert!(schema.validate_parameters(&serde_json::json!({})).is_err());
}

#[test]
fn test_ac3_optional_parameter_omitted() {
    let schema = CommandSchema {
        command_name: "set_config".to_string(),
        parameters: vec![ParameterDef {
            name: "config".to_string(),
            param_type: ParameterType::String { max_length: 256 },
            required: false,
            description: None,
        }],
        description: None,
    };

    // Optional parameter omitted should be OK
    assert!(schema.validate_parameters(&serde_json::json!({})).is_ok());

    // Optional parameter provided should also be OK
    assert!(schema.validate_parameters(&serde_json::json!({"config": "value"})).is_ok());
}

#[test]
fn test_ac3_multiple_parameters_mixed() {
    let schema = CommandSchema {
        command_name: "complex_cmd".to_string(),
        parameters: vec![
            ParameterDef {
                name: "required_param".to_string(),
                param_type: ParameterType::String { max_length: 100 },
                required: true,
                description: None,
            },
            ParameterDef {
                name: "optional_param".to_string(),
                param_type: ParameterType::Int { min: 0, max: 100 },
                required: false,
                description: None,
            },
        ],
        description: None,
    };

    // Missing required param
    assert!(schema
        .validate_parameters(&serde_json::json!({"optional_param": 50}))
        .is_err());

    // With required param
    assert!(schema
        .validate_parameters(&serde_json::json!({"required_param": "value"}))
        .is_ok());

    // With both
    assert!(schema
        .validate_parameters(&serde_json::json!({"required_param": "value", "optional_param": 50}))
        .is_ok());
}

// ============================================================================
// AC#4: Validation Error Messages
// ============================================================================

#[test]
fn test_ac4_error_message_clarity_string_length() {
    let schema = CommandSchema {
        command_name: "test_cmd".to_string(),
        parameters: vec![ParameterDef {
            name: "param".to_string(),
            param_type: ParameterType::String { max_length: 10 },
            required: true,
            description: None,
        }],
        description: None,
    };

    let result = schema.validate_parameters(&serde_json::json!({"param": "toolongstring"}));
    assert!(result.is_err());
    let error_msg = result.unwrap_err();
    assert!(error_msg.contains("exceeds max length"));
    assert!(error_msg.contains("param"));
}

#[test]
fn test_ac4_error_message_clarity_int_range() {
    let schema = CommandSchema {
        command_name: "test_cmd".to_string(),
        parameters: vec![ParameterDef {
            name: "count".to_string(),
            param_type: ParameterType::Int { min: 0, max: 100 },
            required: true,
            description: None,
        }],
        description: None,
    };

    let result = schema.validate_parameters(&serde_json::json!({"count": 150}));
    assert!(result.is_err());
    let error_msg = result.unwrap_err();
    assert!(error_msg.contains("must be in range"));
    assert!(error_msg.contains("count"));
}

#[test]
fn test_ac4_error_message_clarity_enum() {
    let schema = CommandSchema {
        command_name: "test_cmd".to_string(),
        parameters: vec![ParameterDef {
            name: "state".to_string(),
            param_type: ParameterType::Enum {
                values: vec!["on".to_string(), "off".to_string()],
                case_sensitive: false,
            },
            required: true,
            description: None,
        }],
        description: None,
    };

    let result = schema.validate_parameters(&serde_json::json!({"state": "invalid"}));
    assert!(result.is_err());
    let error_msg = result.unwrap_err();
    assert!(error_msg.contains("invalid enum value"));
}

// ============================================================================
// AC#5: Schema Caching & Refresh
// ============================================================================

#[test]
fn test_ac5_schema_cache_insertion_and_retrieval() {
    let cache = CommandSchemaCache::new(3600);
    let schema = CommandSchema {
        command_name: "test_cmd".to_string(),
        parameters: vec![],
        description: None,
    };

    cache.insert("device1".to_string(), vec![schema.clone()]);
    let retrieved = cache.get("device1");
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap()[0].command_name, "test_cmd");
}

#[test]
fn test_ac5_schema_cache_miss() {
    let cache = CommandSchemaCache::new(3600);
    assert!(cache.get("nonexistent").is_none());
}

#[test]
fn test_ac5_schema_cache_clear() {
    let cache = CommandSchemaCache::new(3600);
    let schema = CommandSchema {
        command_name: "test".to_string(),
        parameters: vec![],
        description: None,
    };

    cache.insert("device1".to_string(), vec![schema]);
    assert!(cache.get("device1").is_some());

    cache.clear();
    assert!(cache.get("device1").is_none());
}

// ============================================================================
// AC#6: Enum Parameter Handling
// ============================================================================

#[test]
fn test_ac6_enum_case_sensitive() {
    let schema = CommandSchema {
        command_name: "test".to_string(),
        parameters: vec![ParameterDef {
            name: "state".to_string(),
            param_type: ParameterType::Enum {
                values: vec!["on".to_string(), "off".to_string()],
                case_sensitive: true,
            },
            required: true,
            description: None,
        }],
        description: None,
    };

    assert!(schema.validate_parameters(&serde_json::json!({"state": "on"})).is_ok());
    assert!(schema.validate_parameters(&serde_json::json!({"state": "ON"})).is_err());
}

#[test]
fn test_ac6_enum_case_insensitive() {
    let schema = CommandSchema {
        command_name: "test".to_string(),
        parameters: vec![ParameterDef {
            name: "state".to_string(),
            param_type: ParameterType::Enum {
                values: vec!["on".to_string(), "off".to_string()],
                case_sensitive: false,
            },
            required: true,
            description: None,
        }],
        description: None,
    };

    assert!(schema.validate_parameters(&serde_json::json!({"state": "on"})).is_ok());
    assert!(schema.validate_parameters(&serde_json::json!({"state": "ON"})).is_ok());
    assert!(schema.validate_parameters(&serde_json::json!({"state": "On"})).is_ok());
}

// ============================================================================
// AC#7: Numeric Precision Validation
// ============================================================================

#[test]
fn test_ac7_float_decimal_places_strict() {
    let schema = CommandSchema {
        command_name: "test".to_string(),
        parameters: vec![ParameterDef {
            name: "temp".to_string(),
            param_type: ParameterType::Float {
                min: 0.0,
                max: 100.0,
                decimal_places: Some(2),
            },
            required: true,
            description: None,
        }],
        description: None,
    };

    // Valid: 2 decimal places
    assert!(schema.validate_parameters(&serde_json::json!({"temp": 25.50})).is_ok());

    // Valid: 1 decimal place (within limit)
    assert!(schema.validate_parameters(&serde_json::json!({"temp": 25.5})).is_ok());

    // Invalid: 3 decimal places (exceeds limit)
    assert!(schema.validate_parameters(&serde_json::json!({"temp": 25.123})).is_err());
}

// ============================================================================
// AC#8: Command Availability Check
// ============================================================================

#[test]
fn test_ac8_command_not_in_schema() {
    let schema = CommandSchema {
        command_name: "known_command".to_string(),
        parameters: vec![],
        description: None,
    };

    let mut device_schemas = HashMap::new();
    device_schemas.insert("device1".to_string(), vec![schema]);

    let validator = CommandValidator::new(3600, false, device_schemas);
    let params = serde_json::json!({});

    let result = validator.validate_command_parameters("device1", "unknown_command", &params);
    assert!(result.is_err());
    match result.unwrap_err() {
        OpcGwError::CommandValidation {
            command_name,
            reason,
            ..
        } => {
            assert_eq!(command_name, "unknown_command");
            assert!(reason.contains("not found"));
        }
        _ => panic!("Expected CommandValidation error"),
    }
}

#[test]
fn test_ac8_device_schema_not_found() {
    let device_schemas = HashMap::new();
    let validator = CommandValidator::new(3600, false, device_schemas);
    let params = serde_json::json!({});

    let result = validator.validate_command_parameters("unknown_device", "cmd", &params);
    assert!(result.is_err());
    match result.unwrap_err() {
        OpcGwError::CommandValidation { device_id, .. } => {
            assert_eq!(device_id, "unknown_device");
        }
        _ => panic!("Expected CommandValidation error"),
    }
}

// ============================================================================
// Integration Tests
// ============================================================================

#[test]
fn test_integration_complete_command_validation_flow() {
    // Create a realistic device schema
    let device_schemas = {
        let mut map = HashMap::new();
        map.insert(
            "device_001".to_string(),
            vec![
                CommandSchema {
                    command_name: "set_temperature".to_string(),
                    parameters: vec![
                        ParameterDef {
                            name: "temperature".to_string(),
                            param_type: ParameterType::Float {
                                min: -40.0,
                                max: 85.0,
                                decimal_places: Some(1),
                            },
                            required: true,
                            description: Some("Temperature in Celsius".to_string()),
                        },
                    ],
                    description: Some("Set device temperature setpoint".to_string()),
                },
                CommandSchema {
                    command_name: "toggle_relay".to_string(),
                    parameters: vec![
                        ParameterDef {
                            name: "relay_id".to_string(),
                            param_type: ParameterType::Int { min: 1, max: 8 },
                            required: true,
                            description: None,
                        },
                        ParameterDef {
                            name: "state".to_string(),
                            param_type: ParameterType::Enum {
                                values: vec!["on".to_string(), "off".to_string()],
                                case_sensitive: false,
                            },
                            required: true,
                            description: None,
                        },
                    ],
                    description: Some("Toggle a relay".to_string()),
                },
            ],
        );
        map
    };

    let validator = CommandValidator::new(3600, false, device_schemas);

    // Valid set_temperature command
    let params = serde_json::json!({"temperature": 22.5});
    assert!(validator
        .validate_command_parameters("device_001", "set_temperature", &params)
        .is_ok());

    // Invalid: temperature out of range
    let params = serde_json::json!({"temperature": 100.0});
    assert!(validator
        .validate_command_parameters("device_001", "set_temperature", &params)
        .is_err());

    // Valid toggle_relay command
    let params = serde_json::json!({"relay_id": 1, "state": "on"});
    assert!(validator
        .validate_command_parameters("device_001", "toggle_relay", &params)
        .is_ok());

    // Valid with case variation
    let params = serde_json::json!({"relay_id": 3, "state": "OFF"});
    assert!(validator
        .validate_command_parameters("device_001", "toggle_relay", &params)
        .is_ok());

    // Invalid: relay_id out of range
    let params = serde_json::json!({"relay_id": 10, "state": "on"});
    assert!(validator
        .validate_command_parameters("device_001", "toggle_relay", &params)
        .is_err());
}

#[test]
fn test_integration_schema_cache_with_validator() {
    let mut device_schemas = HashMap::new();
    let schema = CommandSchema {
        command_name: "test_cmd".to_string(),
        parameters: vec![ParameterDef {
            name: "value".to_string(),
            param_type: ParameterType::Int { min: 0, max: 100 },
            required: true,
            description: None,
        }],
        description: None,
    };

    device_schemas.insert("device1".to_string(), vec![schema]);

    let validator = CommandValidator::new(3600, false, device_schemas);
    let params = serde_json::json!({"value": 50});

    // First call should cache the schema
    assert!(validator
        .validate_command_parameters("device1", "test_cmd", &params)
        .is_ok());

    // Second call should use cache (same result)
    assert!(validator
        .validate_command_parameters("device1", "test_cmd", &params)
        .is_ok());
}

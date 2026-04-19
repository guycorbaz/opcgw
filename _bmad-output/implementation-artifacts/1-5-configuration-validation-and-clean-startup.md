# Story 1.5: Configuration Validation and Clean Startup

Status: review

## Story

As an **operator**,
I want clear, actionable error messages when configuration is invalid,
so that I can quickly fix problems and get the gateway running.

## Acceptance Criteria

1. **Given** a configuration file with missing required fields, **When** the gateway starts, **Then** error messages name the specific field and expected format.
2. **Given** a configuration file with invalid values (bad URLs, negative intervals, unknown metric types), **When** the gateway starts, **Then** error messages show the invalid value and valid alternatives.
3. **Given** the config.toml format, **When** compared to v1.0, **Then** no backward-compatibility is required (see Key Decision: Parallel Installation + Cutover).
4. **Given** environment variable overrides (OPCGW_ prefix), **When** the gateway starts, **Then** they are applied after file loading and before validation.
5. **Given** an invalid configuration, **When** the gateway exits, **Then** the exit code is non-zero and the error message is clear (not a panic).
6. **Given** a valid configuration, **When** the gateway starts, **Then** an info-level log confirms successful load with key parameters (poll interval, device count, OPC UA endpoint).
7. **Given** the implementation is complete, **When** I run tests, **Then** `cargo test` passes all tests.
8. **Given** the implementation is complete, **When** I run clippy, **Then** `cargo clippy` produces no warnings.

## Tasks / Subtasks

- [x] Task 1: Add validation method to AppConfig (AC: #1, #2)
  - [x] Add `pub fn validate(&self) -> Result<(), OpcGwError>` method to AppConfig
  - [x] Validate ChirpstackPollerConfig:
    - `server_address` is non-empty and parseable as URL
    - `api_token` is non-empty
    - `tenant_id` is non-empty
    - `polling_frequency` > 0
    - `retry` > 0
    - `delay` > 0
  - [x] Validate OpcUaConfig:
    - `host_port` != 0 if specified (u16 already constrains upper bound to 65535)
    - `application_name` is non-empty
    - `application_uri` is non-empty
    - `user_name` is non-empty
    - `user_password` is non-empty
  - [x] Validate application_list:
    - At least one application configured
    - Each application has non-empty `application_name` and `application_id`
    - Each application has at least one device
    - Each device has non-empty `device_id` and `device_name`
    - Each device has at least one read_metric
    - No duplicate device_ids across applications
  - [x] Return collected validation errors with field names and invalid values

- [x] Task 2: Improve error messages for deserialization failures (AC: #1, #2)
  - [x] Wrap Figment deserialization errors with more context: which section failed, what field is missing/invalid
  - [x] Map common Figment errors to user-friendly messages (e.g., "missing field 'server_address' in [chirpstack] section")

- [x] Task 3: Call validate() after loading in AppConfig::new() (AC: #4, #5)
  - [x] After `Figment::extract()`, call `self.validate()?`
  - [x] Ensure env var overrides (OPCGW_ prefix) are applied before validation (Figment handles this — verify)
  - [x] On validation failure, return `OpcGwError::Configuration` with collected messages

- [x] Task 4: Add startup confirmation log (AC: #6)
  - [x] After successful config load in main.rs, log info with:
    - Polling frequency
    - Number of applications and total devices
    - OPC UA endpoint (host:port)
    - ChirpStack server address (without API token)

- [x] Task 5: Add validation tests (AC: #7)
  - [x] Test: missing required field produces clear error
  - [x] Test: invalid polling_frequency (0) produces clear error
  - [x] Test: empty application_list produces clear error
  - [x] Test: duplicate device_ids produces clear error
  - [x] Test: valid config passes validation

- [x] Task 6: Build, test, lint (AC: #7, #8)
  - [x] `cargo build` — zero errors
  - [x] `cargo test` — all tests pass
  - [x] `cargo clippy` — zero warnings

## Dev Notes

### Current Config Loading (AppConfig::new)

```rust
pub fn new() -> Result<Self, OpcGwError> {
    let config_path = std::env::var("CONFIG_PATH")
        .unwrap_or_else(|_| format!("{}/config.toml", OPCGW_CONFIG_PATH));
    trace!(config_path = %config_path, "Loading configuration");
    let config: AppConfig = Figment::new()
        .merge(Toml::file(&config_path))
        .merge(Env::prefixed("OPCGW_").global())
        .extract()
        .map_err(|e| OpcGwError::Configuration(format!("Configuration loading failed: {}", e)))?;
    Ok(config)
}
```

**After this story**, the flow becomes:
1. Figment loads and merges TOML + env vars
2. `.extract()` deserializes into AppConfig
3. `config.validate()` checks business rules
4. Return validated config or collected errors

### Current Validation: NONE

Zero runtime validation beyond Figment deserialization (type checking + required fields). No range checks, no uniqueness, no format validation.

### Config Structs (key fields to validate)

| Struct | Field | Type | Validation Needed |
|--------|-------|------|------------------|
| ChirpstackPollerConfig | server_address | String | Non-empty, URL format |
| ChirpstackPollerConfig | api_token | String | Non-empty |
| ChirpstackPollerConfig | tenant_id | String | Non-empty |
| ChirpstackPollerConfig | polling_frequency | u64 | > 0 |
| ChirpstackPollerConfig | retry | u32 | > 0 |
| ChirpstackPollerConfig | delay | u64 | > 0 |
| OpcUaConfig | host_port | Option<u16> | != 0 if set (u16 caps at 65535) |
| OpcUaConfig | application_name | String | Non-empty |
| OpcUaConfig | user_name | String | Non-empty |
| OpcUaConfig | user_password | String | Non-empty |
| application_list | (vec) | Vec | At least 1 entry |
| ChirpStackApplications | application_name | String | Non-empty |
| ChirpStackApplications | device_list | Vec | At least 1 entry |
| ChirpstackDevice | device_id | String | Non-empty, unique |
| ChirpstackDevice | device_name | String | Non-empty |

### Key Decision: No Backward Compatibility

The config.toml format may differ from v1.0 — no migration path required. This story can freely restructure validation without worrying about old config files.

### Validation Pattern

```rust
impl AppConfig {
    pub fn validate(&self) -> Result<(), OpcGwError> {
        let mut errors = Vec::new();

        if self.chirpstack.server_address.is_empty() {
            errors.push("chirpstack.server_address: must not be empty".to_string());
        }
        if self.chirpstack.polling_frequency == 0 {
            errors.push("chirpstack.polling_frequency: must be greater than 0".to_string());
        }
        // ... more checks ...

        if errors.is_empty() {
            Ok(())
        } else {
            Err(OpcGwError::Configuration(
                format!("Configuration validation failed:\n  - {}", errors.join("\n  - "))
            ))
        }
    }
}
```

### Previous Story Intelligence

**Story 1.3:** Error handling overhaul — main.rs now uses `error!` + `return Err(e.into())` for config failures. No panics.

**Story 1.2:** All logging uses tracing with structured fields. Startup confirmation should use `info!(poll_interval = ..., device_count = ..., ...)`.

**Story 1.4:** CancellationToken added to ChirpstackPoller::new() and OpcUa::new() — constructor signatures changed. Tests that create these need the token parameter.

### Test Helper Note

The existing test helper `get_config()` at config.rs:690 uses `Figment::extract()` directly — it does NOT call `AppConfig::new()` or `validate()`. This is intentional: the test config (`tests/config/config.toml`) may not pass all production validations. Do NOT add `validate()` to `get_config()`. Instead:
- Validation tests in Task 5 should call `validate()` directly on constructed or modified configs
- Existing tests continue to use `get_config()` without validation (unchanged)

### What NOT to Do

- Do NOT add a `[storage]` config section yet — that's Story 2.2
- Do NOT change the Figment loading order — it already applies env vars after TOML
- Do NOT validate file paths (certificate paths, PKI dir) — filesystem checks are deployment-specific
- Do NOT add new config fields — only validate existing ones
- Do NOT change struct field types — only add validation logic

### Project Structure Notes

Files to modify:
- `src/config.rs` — add validate() method, improve error wrapping
- `src/main.rs` — add startup confirmation log after config load

### Testing Standards

- All existing tests must pass (`cargo test`)
- New validation tests for common error cases
- `cargo clippy` must pass with zero warnings

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Story 1.5] — Original story and ACs
- [Source: _bmad-output/planning-artifacts/epics.md#Key Decision] — No backward-compat on config format
- [Source: _bmad-output/planning-artifacts/architecture.md#Error Handling Patterns] — Error context matters

## Dev Agent Record

### Agent Model Used
Claude Haiku 4.5

### Debug Log References
- Compilation: successful with zero warnings/errors
- Test execution: all 26 tests passing
- Clippy checks: zero warnings

### Completion Notes
1. Implemented comprehensive validation method in AppConfig with proper error collection and context
2. Added 8 new validation tests covering missing fields, invalid ranges, duplicates, and edge cases
3. Updated test config to use valid URL format (http://localhost:8080)
4. Integrated validation into AppConfig::new() with proper error handling
5. Added structured startup log with key configuration parameters

### File List
- `src/config.rs` — added validate() method with comprehensive business rule validation, improved error messages, added 8 validation tests
- `src/main.rs` — added startup confirmation log with poll interval, application count, device count, and OPC UA endpoint
- `tests/config/config.toml` — updated server_address to valid URL format with http:// protocol prefix

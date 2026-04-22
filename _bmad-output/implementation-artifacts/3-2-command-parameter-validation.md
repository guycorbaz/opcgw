# Story 3-2: Command Parameter Validation

**Epic:** 3 (Reliable Command Execution)  
**Phase:** Phase 3 (Phase A)  
**Status:** ready-for-dev  
**Created:** 2026-04-22  
**Author:** Guy Corbaz (Project Lead)  

---

## Objective

Implement schema-driven command parameter validation to ensure only valid commands are enqueued. Parameters must match the device's command definition from ChirpStack, enforcing type safety at the gateway boundary before attempting delivery.

---

## Acceptance Criteria

### AC#1: Command Schema Binding
- Each device has a command manifest (device type → available commands)
- Command manifest fetched from ChirpStack device profile or cached locally
- Command schema includes: parameter names, types (int, float, bool, string), required/optional flags
- **Verification:** Unit test: load command schema, inspect parameter types for known device

### AC#2: Parameter Type Validation
- String parameters: max length enforced (config: 256 chars default)
- Int parameters: min/max range enforced (e.g., 0-255 for int8)
- Float parameters: min/max range enforced, precision checked (e.g., 2 decimal places)
- Bool parameters: any JSON boolean accepted
- Enum parameters: value must be in allowed set (e.g., "on", "off", "auto")
- **Verification:** Unit test: enqueue command with out-of-range int, float with excess precision, out-of-enum value → all rejected

### AC#3: Required Parameter Enforcement
- Schema marks parameters as required or optional
- Enqueue rejects commands missing required parameters
- Optional parameters can be omitted (NULL in JSON)
- **Verification:** Unit test: enqueue command missing required param → rejected; optional param omitted → accepted

### AC#4: Validation Error Messages
- Error messages human-readable, identify failed parameter and reason
- Example: `"Parameter 'temperature' must be float in range [0.0, 100.0], got 150.5"`
- Errors logged (structured, with device_id + command_name context)
- **Verification:** Unit test: trigger 5 different validation errors, verify message clarity

### AC#5: Schema Caching & Refresh
- Command schemas cached in memory (HashMap<device_id, CommandSchema>)
- Cache TTL: 1 hour (configurable)
- Manual refresh via operator CLI command (not in scope for Story 3-2, but architecture allows)
- On ChirpStack connection failure, use stale cache (graceful degradation)
- **Verification:** Unit test: schema fetch, cache hit, expiry after TTL

### AC#6: Enum Parameter Handling
- Enums defined in command schema with allowed values
- Case-sensitive matching (enforce casing rules per device profile)
- Unknown enum values rejected with hint (suggest closest match?)
- **Verification:** Unit test: valid enum, invalid enum, case mismatch → proper rejection

### AC#7: Numeric Precision Validation
- Float parameters: max decimal places validated (e.g., 2 places = 0.01 min increment)
- Excess precision accepted (silently rounded to spec) or rejected (strict mode)
- Config flag: `validate_strict_precision` (default: false = silent rounding)
- **Verification:** Unit test: float param with excess precision, both modes

### AC#8: Command Availability Check
- Device type must have command definition (error if command not in schema)
- Device profile must be loaded (error if device type unknown)
- **Verification:** Unit test: enqueue command not in device schema → rejected

---

## Technical Approach

### Data Model

```rust
pub struct CommandSchema {
    pub command_name: String,
    pub description: Option<String>,
    pub parameters: Vec<ParameterDef>,
    pub cached_at: SystemTime,
    pub fetched_from: String,  // "device_profile" or "chirpstack_api"
}

pub struct ParameterDef {
    pub name: String,
    pub param_type: ParameterType,
    pub required: bool,
    pub description: Option<String>,
}

pub enum ParameterType {
    String { max_length: usize },
    Int { min: i64, max: i64 },
    Float { min: f64, max: f64, decimal_places: Option<u32> },
    Bool,
    Enum { values: Vec<String>, case_sensitive: bool },
}

pub struct CommandSchemaCache {
    schemas: Arc<Mutex<HashMap<String, Vec<CommandSchema>>>>,  // device_id → commands
    ttl: Duration,
    last_refresh: Arc<Mutex<HashMap<String, SystemTime>>>,
}
```

### Validation Engine

Create new module `src/command_validation.rs`:
- `CommandValidator` struct with schema cache
- Methods:
  - `validate_command_parameters(device_id, command_name, params_json) -> Result<()>`
  - `fetch_and_cache_schema(device_id) -> Result<Vec<CommandSchema>>`
  - `validate_parameter(param_def, value_json) -> Result<()>`

### Error Type Extension

Add to `OpcGwError` enum:
```rust
CommandValidation {
    device_id: String,
    command_name: String,
    reason: String,
}
```

### Integration with Story 3-1

Call `CommandValidator::validate_command_parameters()` in `enqueue_command()` before inserting into SQLite. If validation fails, return error (command rejected, not queued).

---

## Schema Source Options

### Option A: Device Profile (Recommended)
- ChirpStack device profiles include command definitions
- Fetch via ChirpStack API: `GetDeviceProfile(device_id)`
- Parse YAML/JSON command section
- Pro: Single source of truth with device definition
- Con: Requires additional API call on first enqueue

### Option B: Hard-Coded Manifest
- Config file defines device types and command schemas
- Update TOML on schema changes (operator responsibility)
- Pro: No runtime API calls, fast
- Con: Manual sync required, schema drift risk

### Option C: Hybrid (Cache + Lazy Load)
- Start with hard-coded manifest in config
- On-demand fetch from ChirpStack API if command not found
- Cache result for 1 hour
- Pro: Balance of performance and flexibility

**Decision: Option C (Hybrid)** — Load from config, lazy-fetch from ChirpStack API, cache 1 hour.

---

## Implementation Steps

1. **Define CommandSchema and ParameterType types**
   - Implement serde for TOML deserialization (config section: `[[command_schema]]`)
   - Add Display impl for user-friendly error messages

2. **Create CommandValidator struct**
   - Initialize with Arc<Mutex<HashMap>> for thread-safe caching
   - Implement `validate_command_parameters()` as entry point
   - Hook into TTL expiry logic

3. **Implement parameter type validators**
   - `validate_string()`: length check
   - `validate_int()`: range check
   - `validate_float()`: range + precision check
   - `validate_enum()`: membership check

4. **Add ChirpStack schema fetch** (calls existing chirpstack.rs API methods)
   - Query device profile on first enqueue
   - Cache result with timestamp
   - Fall back to config schema if API unavailable

5. **Integration with EnqueueCommand**
   - Call validator before SQLite insert
   - Return validation error if params invalid
   - Log validation failure (device_id, command_name, reason)

6. **Configuration**
   - Add `[command_validation]` section to config.toml
   - Fields: `cache_ttl_secs`, `strict_precision_mode`, `default_string_max_length`
   - Schema definitions: `[[command_schema.device_type_name]]`

7. **Test parameter validators thoroughly**
   - Boundary cases (min/max, precision edge cases)
   - Type mismatches (string where int expected)
   - Missing required params
   - Unknown enum values

---

## Configuration Schema (TOML)

```toml
[command_validation]
cache_ttl_secs = 3600
strict_precision_mode = false
default_string_max_length = 256

# Device type schemas (one per device type in deployment)
[[command_schema.modbus_device]]
command_name = "set_temperature"
required_parameters = [
  { name = "value", type = "float", min = 0.0, max = 100.0, decimal_places = 2 }
]

[[command_schema.modbus_device]]
command_name = "toggle_relay"
required_parameters = [
  { name = "relay_id", type = "int", min = 1, max = 8 },
  { name = "state", type = "enum", values = ["on", "off"], case_sensitive = false }
]
```

---

## Assumptions & Constraints

- **Schema authority:** ChirpStack device profiles define valid commands (or local config override)
- **Type system:** Only JSON-serializable types (string, number, boolean, null)
- **Enum case:** Case sensitivity configurable per schema (default: case-insensitive)
- **Precision rounding:** Default to silent rounding; strict mode rejects excess precision
- **Unavailable schema:** If device profile can't be fetched and no local cache, reject command (fail-safe)

---

## File List

**New Files:**
- `src/command_validation.rs` — CommandValidator, ParameterType, validation logic

**Modified Files:**
- `src/utils.rs` — Add CommandValidation variant to OpcGwError
- `src/storage/sqlite.rs` — Call validator in enqueue_command()
- `src/storage/inmemory.rs` — Call validator in enqueue_command()
- `config/config.toml` — Add `[command_validation]` section with sample schemas
- `Cargo.toml` — No new dependencies (serde already present)

**Test Files:**
- `tests/command_validation_tests.rs` — New file: 20+ test cases covering all AC

---

## Dev Notes

### Decision: Strict vs Lenient Precision
Strict mode (reject excess decimal places) is safer but may reject valid commands from imprecise sources. Lenient mode (silent rounding) is more forgiving but may mask data loss. Default to lenient with config flag for strict deployments.

### Decision: Enum Case Sensitivity
ChirpStack typically uses lowercase enums ("on", "off"). Case-insensitive matching (with normalization) prevents operator typos ("On" vs "on"). Configurable per schema for compatibility.

### Schema Caching Strategy
In-memory HashMap with TTL prevents cache staleness if device profiles change. 1-hour TTL balances freshness vs API load. On TTL expiry, next enqueue triggers refresh automatically. If refresh fails, use stale cache (fail-open for resilience).

### Error Clarity
Validation errors must be specific enough for operators to fix without reading code. Include parameter name, expected type/range, and actual value received. Log structured (device_id, command_name, param_name) for debugging.

---

## Acceptance Checklist

- [ ] CommandSchema and ParameterType types defined with serde support
- [ ] CommandValidator struct with TTL-based caching
- [ ] All 5 parameter type validators implemented (string, int, float, bool, enum)
- [ ] Required/optional parameter enforcement working
- [ ] Integration with enqueue_command() in Story 3-1
- [ ] Error messages human-readable and specific
- [ ] Configuration section in config.toml documented
- [ ] All 20+ unit tests passing (boundary cases, type mismatches, precision edge cases)
- [ ] Integration test: schema cache TTL expiry
- [ ] Code review signoff: no clippy warnings, no unsafe code
- [ ] SPDX license headers on all new code

---

## References

- **Story 3-1:** Integration point, enqueue_command() validation hook
- **CLAUDE.md:** ChirpStack API, device profiles
- **ChirpStack API Docs:** Device profile schema format

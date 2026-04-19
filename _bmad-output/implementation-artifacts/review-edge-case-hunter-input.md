# Edge Case Hunter Review Input
## Story 2-2: Type System & SQL Alignment
## With Project Read Access

### Context: Type System Refactoring

**What changed:**
- Metric values: `MetricType::Float(23.5)` → `MetricValueInternal { value: "23.5", data_type: Float }`
- All values now stored as `String`, parsed to native types on access
- Command types: f_port changed from `u32` to `u8`, data renamed to payload
- Status tracking: response_time (f64) → last_poll_time (Option<DateTime>)

### Key Conversions Under Review

**1. chirpstack.rs - Metric Value Storage**
```rust
// Bool conversion
let bool_value = match value {
    0.0 => false,
    1.0 => true,
    _ => { error!(...); return; }
};
let metric_val = MetricValueInternal {
    value: bool_value.to_string(),  // "true" or "false"
    data_type: MetricType::Bool,
    ...
};
```

**2. chirpstack.rs - Integer Conversion**
```rust
let int_value = match value.trunc() as i64 {
    Ok(v) => v,
    Err(_) => { error!(...); return; }
};
let metric_val = MetricValueInternal {
    value: int_value.to_string(),  // e.g., "42"
    ...
};
```

**3. opc_ua.rs - Reverse Conversion (String → Type)**
```rust
fn convert_metric_to_variant(metric: MetricValueInternal) -> Variant {
    match metric.data_type {
        MetricType::Int => {
            match metric.value.parse::<i64>() {
                Ok(value) => {
                    match i32::try_from(value) {
                        Ok(v) => Variant::Int32(v),
                        Err(_) => Variant::Int64(value),
                    }
                }
                Err(_) => Variant::Int32(0),  // Returns 0 on parse failure
            }
        }
        MetricType::Bool => {
            let bool_value = metric.value.to_lowercase() == "true";
            Variant::Boolean(bool_value)
        }
        // ...
    }
}
```

**4. opc_ua.rs - Command f_port Handling**
```rust
let f_port = match u8::try_from(command.command_port) {
    Ok(port) => port,
    Err(_) => {
        warn!(port = %command.command_port, "Command port out of u8 range [1-223]");
        return opcua::types::StatusCode::BadOutOfRange;
    }
};
```

### Boundary Conditions to Check

1. **Numeric boundaries** (from ChirpStack floats):
   - What if a metric value is `NaN` or `Infinity`? Does `.to_string()` produce valid JSON?
   - What if an integer is `f64::MAX` or `f64::MIN`?
   - Conversions: f64 → i64 → String → parsing back: Reversible?

2. **String storage and parsing**:
   - Float metric: stores "23.5", retrieves as i32 (truncates). Lost precision—intended?
   - Bool metric: stores "true"/"false", parsed with `.to_lowercase() == "true"`. What if stored as "True"?
   - String metric: direct conversion. What about special chars, escaping, null bytes?

3. **Command f_port**:
   - Old: `f_port: u32` (0-2^32-1)
   - New: `f_port: u8` (0-255), validated as 1-223
   - Existing commands in queue with f_port > 255: Will they truncate silently on storage or conversion?

4. **Datetime handling**:
   - `chrono::Utc::now()` is called on every metric store. Clock skew/backward jumps?
   - Serialization to JSON: What if timestamp exceeds serde_json capacity?
   - Deserialization from SQLite: What if stored as different format?

5. **Error handling on type mismatch**:
   - Bool: metric.value = "not-a-bool" → returns `Variant::Boolean(false)` silently
   - Int: metric.value = "not-an-int" → returns `Variant::Int32(0)` silently
   - Float: metric.value = "not-a-float" → returns `Variant::Float(0.0)` silently
   - Silent defaults mask data corruption. Log level sufficient?

6. **Empty/null handling**:
   - What if metric.value is empty string ""?
   - Bool: "".to_lowercase() == "true" → false (correct fallback?)
   - Int: "".parse::<i64>() → Err, returns 0
   - Float: "".parse::<f64>() → Err, returns 0.0

7. **ChirpStack integration**:
   - Old flow: receive float from API → store as MetricType::Int(value as i64)
   - New flow: receive float → to_string() → store → parse back as i64
   - Example: 42.7 → "42.7" → parse as i64 → "42" (truncation, OK?) vs. old: 42.7 as i64 → 42 (same)
   - But bool: 1.0/0.0 → true/false (stored) → only recoverable if OPC UA knows it's Bool. Correct?

### Integration Risks

1. **Round-trip integrity**:
   - Metric stored: device=d1, name="temp", value="23.5", type=Float
   - OPC UA retrieves: calls convert_metric_to_variant
   - Returns: Float(23.5)
   - If persisted later: still "23.5" or reformatted?

2. **Type mismatch scenarios**:
   - If a device has metric "pressure" stored as Float, can OPC UA node be reconfigured to read it as Int?
   - Old system would error on type mismatch. New system silently defaults to 0—intended?

3. **SQL serialization round-trip**:
   - MetricValueInternal: Serialize to JSON → SQLite TEXT → Deserialize
   - If JSON encoding uses different float precision, is it recoverable?
   - DeviceCommandInternal: Serialized with status (Pending/Sent/Failed). Does deserialization maintain enum correctly?

4. **Chirpstack polling robustness**:
   - Poller updates `last_poll_time: Some(Utc::now())`
   - What if clock is adjusted backward? Last_poll_time in future?
   - Error count increments but never resets. Intended?

---

## REVIEW TASK

Walk every branching path and boundary in the diff. Report ONLY unhandled paths:

Format each finding as:
```json
{
  "location": "file:line-range",
  "trigger_condition": "one-line description",
  "guard_snippet": "minimal code to close the gap",
  "potential_consequence": "what could go wrong"
}
```

Exhaustive coverage: parse failures, type coercions, empty values, numeric boundaries, round-trip serialization, SQL escaping, null/None handling, backward time, command queue state.

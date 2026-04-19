# Blind Hunter Review Input
## Story 2-2: Type System & SQL Alignment
## Diff (No Context Provided)

### Cargo.toml Changes
```toml
+chrono = { version = "0.4.26", features = ["serde"] }
+async-trait = "0.1.81"
+serde_json = "1.0.132"
```

### Type Definition Changes (src/storage/types.rs)
**NEW: MetricType enum (simplified)**
```rust
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum MetricType {
    Float, Int, Bool, String,
}
```

**NEW: MetricValue struct**
```rust
pub struct MetricValue {
    pub device_id: String,
    pub metric_name: String,
    pub value: String,  // All values as text
    pub timestamp: DateTime<Utc>,
    pub data_type: MetricType,
}
```

**NEW: DeviceCommand struct**
```rust
pub struct DeviceCommand {
    pub id: u64,
    pub device_id: String,
    pub payload: Vec<u8>,  // Changed from: data
    pub f_port: u8,        // Changed from: u32
    pub status: CommandStatus,
    pub created_at: DateTime<Utc>,
    pub error_message: Option<String>,
}
```

**NEW: ChirpstackStatus struct**
```rust
pub struct ChirpstackStatus {
    pub server_available: bool,
    pub last_poll_time: Option<DateTime<Utc>>,  // Changed from: response_time (f64)
    pub error_count: u32,
}
```

### chirpstack.rs Changes - Type Conversions
**OLD:**
```rust
storage.set_metric_value(device_id, &metric_name, MetricType::Bool(bool_value));
```

**NEW:**
```rust
let metric_val = crate::storage::MetricValueInternal {
    device_id: device_id.clone(),
    metric_name: metric_name.clone(),
    value: bool_value.to_string(),
    timestamp: chrono::Utc::now(),
    data_type: crate::storage::MetricType::Bool,
};
storage.set_metric_value(device_id, &metric_name, metric_val);
```

**Command handling:**
```rust
// OLD
let command: DeviceCommand { confirmed: bool, f_port: u32, data: Vec<u8> }

// NEW
let command: crate::storage::DeviceCommandInternal {
    id: u64,
    payload: Vec<u8>,  // renamed from data
    f_port: u8,        // changed from u32
    status: CommandStatus::Pending,
    created_at: Utc::now(),
    error_message: None,
}
```

### opc_ua.rs Changes - Type Conversions
**OLD convert function:**
```rust
fn convert_metric_to_variant(metric_type: MetricType) -> Variant {
    match metric_type {
        MetricType::Int(value) => Variant::Int32(value as i32),
        MetricType::Float(value) => Variant::Float(value as f32),
        // ...
    }
}
```

**NEW convert function:**
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
                Err(_) => Variant::Int32(0),
            }
        }
        // Similar for other types...
    }
}
```

### SQL Serialization (src/storage/mod.rs)
```rust
impl ToSql for MetricValueInternal {
    fn to_sql(&self) -> SqliteResult<...> {
        let json = serde_json::to_string(self)?;
        Ok(rusqlite::types::ToSqlOutput::Owned(
            rusqlite::types::Value::Text(json)
        ))
    }
}
```

---

## REVIEW TASK

Perform adversarial code review focusing on:
1. **Logic errors** — Are type conversions correct? Any off-by-one or conversion bugs?
2. **Type safety** — Do all type transitions maintain invariants?
3. **Error handling** — What happens when parsing fails (e.g., invalid i64)?
4. **Data loss** — Any conversions that drop information or truncate values?
5. **Security** — Any injection risks via JSON serialization/deserialization?

Output as Markdown list, one issue per line with evidence from the diff.

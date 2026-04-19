# Deferred Work

## Deferred from: code review of story-1.1 (2026-04-02)

- `command_id as u32` lossy cast at opc_ua.rs:592 — i32 cast to u32 without validation, negative values wrap. Target: Story 3.2
- `create_device_client().await.unwrap()` at chirpstack.rs:1084 — panic on client creation failure. Target: Story 1.3
- `flush_queue: false` hardcoded at chirpstack.rs:1081 — no mechanism to flush stale commands. Target: Story 3.1
- `try_into().unwrap()` at opc_ua.rs:824 — panic on out-of-range command value. Target: Story 1.3
- `command_port as u32` lossy cast at opc_ua.rs:823 — i32 port cast without validation. Target: Story 3.2

## Deferred from: code review of story-1.2 (2026-04-02)

- Console layer hardcoded to DEBUG with no runtime override (e.g., RUST_LOG). Future enhancement.
- Log file path "log/" hardcoded — no config option. Future enhancement.
- No fallback if log directory missing — logs silently drop. Same as old behavior.

## Deferred from: code review of story-1.3 (2026-04-02)

- Mutex poison silently drops metric writes — full fix when storage migrates to separate SQLite connections (Epic 2). Target: Story 4.1
- set_metric() on missing device returns () with no error signal — signature change to Result when StorageBackend trait introduced. Target: Story 4.1

## Deferred from: code review of story-1.4 (2026-04-02)

- Cancellation not checked inside poll_metrics() retry loop — long gRPC retries can delay shutdown. Target: Story 4.4

## Deferred from: code review of story-2-1b-core-storage-data-types (2026-04-19)

- **Missing Serde derives on DateTime-containing structs** — MetricValue/DeviceCommand need #[derive(Serialize, Deserialize)] if serde feature is used. Moot if serde feature removed per spec constraint. Target: Story 2-1c or later storage implementation.

- **Timestamp overflow risk in metric queries** — Duration addition in chirpstack.rs lacks bounds checking. Monitor when implementing full storage layer. Target: Story 4.1 (Poller refactoring).

- **Implicit UTC timezone assumption** — Code assumes ChirpStack API guarantees UTC timezones. Add validation when implementing full storage integration. Target: Story 2-2 (SQLite schema).

- **Chrono serde version compatibility** — chrono 0.4.x versions may have different serde behavior. Pin version and add integration tests for serialization round-trips. Target: Story 2-3 (Metric persistence).

- **Precision loss on prost-types conversion** — prost-types::Timestamp (seconds+nanoseconds) vs chrono::DateTime<Utc> (100-nanosecond precision) may lose precision. Add tests for round-trip conversion. Target: Story 2-3 (Metric persistence).

- **Option<DateTime> deserialization null semantics** — ChirpstackStatus.last_poll_time may need custom deserializer to distinguish "never polled" from "explicitly null". Address in full storage implementation. Target: Story 2-4 (Graceful degradation).

## Deferred from: code review of story-2-2b-schema-creation-and-migration (2026-04-19)

- **No prepared statement caching** [sqlite.rs:372-376] — Performance optimization. Each `get_pending_commands()` call parses SQL string via `conn.prepare()`. Use `prepare_cached()` for repeated queries. Target: Story 2-3 (Performance optimization).

- **Three separate queries instead of one** [sqlite.rs:225-276] — `get_status()` executes three separate SELECT queries for server_available, last_poll_time, and error_count. Combine into single query with multi-column SELECT. Target: Story 2-3 (Performance optimization).

- **device_id/metric_name length not validated** [sqlite.rs] — No explicit length constraints. May require validation when StorageBackend interface is stabilized. Monitor for extremely long IDs causing database bloat. Target: Story 2-2c or later.

- **Empty payload allowed in queue_command** [sqlite.rs:318-331] — No explicit constraint against zero-byte LoRaWAN payloads. May be acceptable per spec. Clarify intent with LoRaWAN specialists. Target: Story 3.1 (Command execution).

- **Config path validation incomplete** [config.rs] — StorageConfig struct created with database_path field, but full validation logic in config.rs not visible in diff. Verify that path validation is performed at config load time, not just at database open time. Target: Story 2-2c or concurrent review.

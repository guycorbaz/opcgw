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

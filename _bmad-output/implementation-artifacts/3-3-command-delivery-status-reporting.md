# Story 3-3: Command Delivery Status Reporting

**Epic:** 3 (Reliable Command Execution)  
**Phase:** Phase 3 (Phase A)  
**Status:** ready-for-dev  
**Created:** 2026-04-22  
**Author:** Guy Corbaz (Project Lead)  

---

## Objective

Implement status tracking and reporting for command delivery lifecycle. OPC UA clients can query command status (pending, sent, confirmed, failed) and receive notifications of status changes. ChirpStack delivery confirmations are processed and mapped back to queued commands for end-to-end visibility.

---

## Acceptance Criteria

### AC#1: Command Status Lifecycle
- Commands transition through states: `pending` → `sent` → `confirmed` (or `failed`)
- State machine enforced (no invalid transitions: confirmed ↛ pending, failed ↛ sent)
- Timestamp recorded for each state transition (audit trail)
- **Verification:** Unit test: verify state machine transitions, timestamp presence on each state

### AC#2: ChirpStack Delivery Confirmation
- Poll ChirpStack command status API to verify command delivery
- On confirmation from ChirpStack, update queued command status to `confirmed`
- Polling interval: configurable (default 5s, same as metric polling)
- **Verification:** Integration test: enqueue command, verify sent to ChirpStack, poll confirms delivery

### AC#3: Delivery Timeout & Failure Handling
- Commands remain in `sent` state for max TTL (configurable: default 60s)
- After TTL expiry with no confirmation, mark as `failed` with reason "Confirmation timeout"
- Timeout clock starts when command is sent, resets on status update
- **Verification:** Unit test: simulate timeout, verify failed state + error message

### AC#4: OPC UA Status Query API
- OPC UA method or variable allows querying command status by command_id
- Returns struct: {id, device_id, command_name, status, sent_at, confirmed_at, error_message}
- Query supports filtering by device_id (list all commands for a device)
- **Verification:** Integration test: OPC UA client queries command status, receives correct struct

### AC#5: OPC UA Status Notification
- OPC UA event triggered when command status changes (pending→sent, sent→confirmed, sent→failed)
- Event includes command_id, device_id, command_name, new_status, timestamp
- Client can subscribe to command status events (OPC UA subscription mechanism)
- **Verification:** Integration test: OPC UA client subscribes, receives notification on state change

### AC#6: Command History Retention
- Completed commands (confirmed or failed) remain in SQLite for audit trail
- Retention period: configurable (default 7 days)
- Automatic cleanup of expired commands (daily job, separate from pruning task)
- **Verification:** Integration test: mark command confirmed, wait for retention expiry, verify cleanup

### AC#7: Concurrent Status Updates
- Multiple sources can update command status (poller confirms, timeout handler marks failed)
- Updates are atomic, no race conditions on SQLite writes
- Last-update-wins if simultaneous updates on same command (timestamp-based)
- **Verification:** Concurrent unit test: 5 tasks attempt status update on same command, verify atomicity

### AC#8: Error Reporting
- Failed commands include error_message field (reason for failure)
- Examples: "Confirmation timeout", "Device unreachable", "Invalid command signature"
- Error messages logged with context (device_id, command_id, timestamp)
- **Verification:** Unit test: verify error messages on various failure scenarios

---

## Technical Approach

### Data Model Extensions

Extend `command_queue` table from Story 3-1:
```sql
ALTER TABLE command_queue ADD COLUMN confirmed_at TEXT;  -- RFC3339 microsecond
ALTER TABLE command_queue ADD COLUMN error_message TEXT;
ALTER TABLE command_queue ADD COLUMN chirpstack_result_id TEXT;  -- For mapping ChirpStack responses
```

Extend Command struct:
```rust
pub struct Command {
    pub id: u32,
    pub device_id: String,
    pub metric_id: String,
    pub command_name: String,
    pub parameters: serde_json::Value,
    pub enqueued_at: SystemTime,
    pub sent_at: Option<SystemTime>,
    pub confirmed_at: Option<SystemTime>,
    pub status: CommandStatus,
    pub error_message: Option<String>,
    pub chirpstack_result_id: Option<String>,
}

pub enum CommandStatus {
    Pending,
    Sent,
    Confirmed,
    Failed(String),
}
```

### Delivery Confirmation Polling

Extend `chirpstack.rs` with new task:
- `CommandStatusPoller` struct (similar to `ChirpstackPoller`)
- Polls ChirpStack command status API every 5s
- Queries for sent commands by `chirpstack_result_id`
- Calls `update_command_status(cmd_id, Confirmed)` on successful response

### OPC UA Address Space Extensions

Add to OPC UA server in `opc_ua.rs`:
- **Method:** `QueryCommandStatus(command_id: UInt32) → CommandStatusStruct`
- **Method:** `ListCommandsByDevice(device_id: String) → Array<CommandStatusStruct>`
- **Event:** `CommandStatusChanged` event type (BaseEventType)
- **Variable:** `QueuedCommandCount` (UInt32, gauge metric)

### Timeout Handler

Background task in `chirpstack.rs`:
- Wakes every 10s to scan for timed-out commands
- Query: `SELECT * FROM command_queue WHERE status='sent' AND sent_at < NOW() - 60s`
- Updates status to `failed` with reason "Confirmation timeout"
- Triggers OPC UA event notification

### OPC UA Event Notification

Use async-opcua event triggering:
- On status change, create event with EventId, SourceNode, EventType
- Populate EventData with command struct
- Emit to subscribed clients

---

## Implementation Steps

1. **Extend command_queue table schema**
   - Add confirmed_at, error_message, chirpstack_result_id columns
   - Index on (status, sent_at) for timeout query efficiency

2. **Extend Command struct & StorageBackend trait**
   - Add confirmed_at, error_message, chirpstack_result_id fields
   - Implement CommandStatus enum with Display trait
   - Add methods:
     - `mark_command_sent(cmd_id, chirpstack_result_id) -> Result<()>`
     - `mark_command_confirmed(cmd_id) -> Result<()>`
     - `mark_command_failed(cmd_id, error_msg) -> Result<()>`
     - `find_pending_confirmations() -> Result<Vec<Command>>`
     - `find_timed_out_commands(ttl_secs) -> Result<Vec<Command>>`

3. **Implement CommandStatusPoller in chirpstack.rs**
   - Spawn as separate tokio task (like metric poller)
   - Query ChirpStack command status API by command ID
   - Parse response, map to local command by chirpstack_result_id
   - Call mark_command_confirmed()
   - Handle API errors gracefully (retry logic from Story 2-5b)

4. **Implement timeout handler in chirpstack.rs**
   - Spawn as background task (wakes every 10s)
   - Query timed-out commands (sent > 60s ago)
   - Mark as failed with "Confirmation timeout" error
   - Log context (device_id, command_id, sent_at)

5. **Extend OPC UA server in opc_ua.rs**
   - Add QueryCommandStatus() method node
   - Add ListCommandsByDevice() method node
   - Create CommandStatusChanged event type
   - Implement event emission on status updates

6. **Integration with command send**
   - In ChirpstackPoller::check_and_execute_send(), after sending command to ChirpStack:
     - Extract ChirpStack result ID from response
     - Call mark_command_sent(cmd_id, chirpstack_result_id)
     - Update command status from 'pending' to 'sent'

7. **Configuration**
   - Add `[command_delivery]` section to config.toml
   - Fields: `status_poll_interval_secs`, `confirmation_timeout_secs`, `history_retention_days`

8. **Comprehensive testing**
   - Unit tests: state machine transitions, timeout logic, atomic updates
   - Integration tests: OPC UA method calls, event subscriptions, status changes
   - Stress test: concurrent confirmations from poller

---

## Configuration Schema (TOML)

```toml
[command_delivery]
status_poll_interval_secs = 5
confirmation_timeout_secs = 60
history_retention_days = 7
timeout_check_interval_secs = 10
```

---

## ChirpStack Integration Points

### Sending Commands to ChirpStack

Assume ChirpStack gRPC API provides:
```proto
service ChirpstackService {
  rpc SendDeviceCommand(SendDeviceCommandRequest) returns (SendDeviceCommandResponse) {}
  rpc GetDeviceCommandStatus(GetDeviceCommandStatusRequest) returns (GetDeviceCommandStatusResponse) {}
}
```

Implementation will need:
- Extract command ID from SendDeviceCommandResponse (for tracking)
- Query status via GetDeviceCommandStatus(command_id)
- Handle status enums from ChirpStack → map to local CommandStatus

---

## Assumptions & Constraints

- **ChirpStack support:** Device commands are supported in target deployment (may not exist in older versions)
- **Single gateway:** No distributed confirmation (one gateway owns all commands)
- **Confirmation semantics:** ChirpStack confirmation means device ACK'd, not that command executed successfully
- **Timeout TTL:** 60s assumed sufficient for most devices; configurable if needed
- **Event ordering:** OPC UA events are delivered in chronological order (async-opcua guarantee)
- **Error messages:** User-facing, no internal stack traces

---

## File List

**New Files:**
- `src/command_delivery.rs` — CommandStatusPoller, timeout handler logic

**Modified Files:**
- `src/storage/sqlite.rs` — Add mark_command_sent/confirmed/failed methods, extend table schema
- `src/storage/inmemory.rs` — Implement same delivery tracking methods (in-memory)
- `src/chirpstack.rs` — Add CommandStatusPoller task, timeout handler task
- `src/opc_ua.rs` — Add status query methods, event emission
- `src/utils.rs` — Extend OpcGwError for delivery-related errors
- `config/config.toml` — Add `[command_delivery]` section
- `Cargo.toml` — No new dependencies

**Test Files:**
- `tests/command_delivery_tests.rs` — New file: 25+ test cases covering all AC
- Integration tests in existing suite

---

## Dev Notes

### Decision: Status Polling vs Push Notifications
ChirpStack may not support push notifications for command status. Polling every 5s keeps implementation simple and resilient to ChirpStack API changes. If push becomes available, polling can be replaced with subscription (backward compatible).

### Decision: TTL Expiry Strategy
Timeout handler runs every 10s (not on every status check) to reduce query load. Commands with no confirmation after 60s are marked failed. 60s allows for transient network issues (metric polling is 30s interval, so 2 polls worth of time). Configurable per deployment.

### Decision: Last-Update-Wins
If simultaneous updates occur (poller confirms + timeout marks failed), last timestamp wins. In practice, this shouldn't happen (sent → confirmed is linear), but atomic UPDATE + timestamp comparison prevents races.

### Error Messages
Include actionable information: "Confirmation timeout" tells operator to check device connectivity, "Device unreachable" suggests ChirpStack API issue. Structured logging with command context (device_id, command_id) helps debugging.

### OPC UA Event Routing
Events emitted to subscribed clients via async-opcua event loop. Each status change creates one event (not batched). Clients can filter by SourceNode or EventType to receive only relevant events.

---

## Acceptance Checklist

- [ ] command_queue table extended with confirmed_at, error_message, chirpstack_result_id
- [ ] Command struct updated with delivery tracking fields
- [ ] StorageBackend trait has all 6 delivery tracking methods
- [ ] CommandStatusPoller implemented and spawned as tokio task
- [ ] Timeout handler implemented (scans every 10s, marks expired as failed)
- [ ] OPC UA QueryCommandStatus() method working (returns correct struct)
- [ ] OPC UA ListCommandsByDevice() method working (filters by device_id)
- [ ] OPC UA event emission on status change (events received by subscribed clients)
- [ ] Configuration section `[command_delivery]` in config.toml
- [ ] All 25+ unit tests passing (state machine, timeouts, concurrent updates)
- [ ] Integration test: command sent, polled, confirmed, status visible to OPC UA client
- [ ] Code review signoff: no clippy warnings, no unsafe code
- [ ] SPDX license headers on all new code

---

## References

- **Story 3-1:** Command queue persistence, enqueue/dequeue API
- **Story 3-2:** Parameter validation before sending
- **CLAUDE.md:** ChirpStack API, OPC UA server architecture
- **Story 2-5b:** Timeout handling patterns, exponential backoff (reuse if needed)
- **ChirpStack gRPC API:** Command status endpoint definition

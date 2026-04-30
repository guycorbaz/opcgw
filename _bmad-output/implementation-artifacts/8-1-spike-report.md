# Story 8-1 Spike Report: async-opcua Subscription Support

**Date:** 2026-04-29 (initial); 2026-04-30 (test-depth additions, AC#3 deferral)
**Author:** Claude Code (`bmad-dev-story 8-1`)
**Story spec:** [`8-1-async-opcua-subscription-spike.md`](./8-1-async-opcua-subscription-spike.md)
**Status:** Plan A confirmed empirically. **9 integration tests** in `tests/opcua_subscription_spike.rs` cover Plan-A confirmation, Story 7-2/7-3 invariant pins, multi-client, multi-monitored-item, sampling-interval revision, value-flow payload, teardown idempotency, and per-session state isolation. **AC#3 explicitly deferred** to a single integration pass after Epic 9 lands (user decision 2026-04-30; documented in `deferred-work.md` "Story 8-1" block). AC#7 (upstream FR filing) and AC#8 (`--load-probe` 5-min run) remain operator-action carry-forward.

---

## 1. Executive summary

**Plan A is confirmed.** OPC UA subscriptions work end-to-end against opcgw at HEAD with **zero `src/` changes**. The integration test `test_subscription_basic_data_change_notification` passes against a fresh gateway: `CreateSubscription` → `CreateMonitoredItems` → `Publish` → `DataChangeNotification` flows through opcgw's existing `SimpleNodeManager` + `add_read_callback` pipeline thanks to async-opcua 0.17.1's `SimpleNodeManagerImpl` already implementing the four monitored-item lifecycle hooks (`create_value_monitored_items`, `modify_monitored_items`, `set_monitoring_mode`, `delete_monitored_items`) and auto-wiring a `SyncSampler` against the existing read-callback registrations.

**For Story 8-2's spec:** the load-bearing inputs are
1. **Four-knob `Limits` minimum** — `max_subscriptions_per_session` (default 10), `max_monitored_items_per_sub` (default 1000; **note the field-name asymmetry vs. `epics.md:682, 724`'s `max_monitored_items_per_subscription`**), `max_message_size`, `max_chunk_count`. The first two require `ServerBuilder::limits_mut()` (no direct setter); the last two have direct setters.
2. **`AtLimitAcceptLayer` workaround stays.** Re-grep of `~/.cargo/registry/src/.../async-opcua-server-0.17.1/src/session/manager.rs:64-72` confirms no session-rejected callback in 0.17.1. The library's own source carries a `// TODO: Auditing and diagnostics` comment at `manager.rs:74` — strong hook for an upstream FR.
3. **Auth + connection-cap composition holds.** Subscription-creating clients pass through `OpcgwAuthManager` (Story 7-2) and `AtLimitAcceptLayer` (Story 7-3) identically to read-only clients. No new audit-event infrastructure for Epic 8.

**No regressions expected.** The integration tests added by 8-1 are additive (9 new integration tests under `tests/opcua_subscription_spike.rs`); no production code in `src/` was modified.

**Test-depth update 2026-04-30.** After the user decision to defer manual FUXA + Ignition verification to a single integration pass after Epic 9, the spike's automated test surface was expanded from 3 tests (the original AC#1 + AC#9 trio) to 9 tests (added: multi-client, ten-monitored-items-per-subscription, sampling-interval revision, value-flow payload, teardown idempotency, sibling-session isolation). All 9 pass. Test count: 581 baseline → 592 post-spike. See § 8 for the full test inventory.

---

## 2. Reproduction recipe

**Plan A confirmation (automated, in CI):**
```bash
cargo test --test opcua_subscription_spike test_subscription_basic_data_change_notification
```
Expected: `test result: ok. 1 passed; 0 failed`. Wall clock ≈ 4–6 s.

**Plan A confirmation against a running gateway (manual):**
```bash
# 1. Start the gateway in another terminal:
OPCGW_OPCUA__USER_PASSWORD=test-pass cargo run

# 2. Run the spike binary:
cargo run --example opcua_subscription_spike -- \
    --plan-a \
    --user test-user \
    --password test-pass \
    --plan-a-node Metric01
```
Expected stdout: `plan-a: PASS — first notification at <N> ms, total received <M>`. Expected stderr (JSON summary, single line):
```json
{"plan":"A","ok":true,"error":null,"first_notification_latency_ms":<ms>,"publish_count":<N>}
```

**Auth + cap composition (automated, in CI):**
```bash
cargo test --test opcua_subscription_spike test_subscription_client_rejected_by_auth_manager
cargo test --test opcua_subscription_spike test_subscription_client_rejected_by_at_limit_layer
```

**Load probe (manual, opt-in, 5 min wall clock):**
```bash
cargo run --release --example opcua_subscription_spike -- \
    --load-probe \
    --user test-user \
    --password test-pass \
    --load-items 100 \
    --load-secs 300 \
    --load-publish-ms 1000 \
    --load-nodes "Metric01,Metric02,Metric03,Metric04,Metric05,Metric06"
```
Expected: long-running. Final stderr line is a single JSON summary with notification counts + p50/p95/p99 inter-notification interval. **This run was NOT performed during automated implementation** — see § 7 "Throughput measurement" for the open AC#8 deliverable.

---

## 3. Plan A confirmation

**Empirical evidence.** Test suite output 2026-04-29:
```
running 3 tests
test test_subscription_basic_data_change_notification ... ok
test test_subscription_client_rejected_by_at_limit_layer ... ok
test test_subscription_client_rejected_by_auth_manager ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 13.62s
```

**`git diff --stat src/` over the spike branch:** 0 `.rs` files modified. No production code was touched. The optional `OPCUA_SPIKE_8_1_EVENT_TARGET` constant from AC#5 was **not** added — the spike binary uses `tracing::info!` / `tracing::warn!` against the default targets without needing a custom filter name.

**What this proves:**
- The CreateSubscription service path through async-opcua's `SubscriptionService` is wired and accepts opcgw's `null`-endpoint sessions.
- The CreateMonitoredItems service path through `SimpleNodeManagerImpl::create_value_monitored_items` accepts the test fixture's `NodeId::new(2, "Temperature")` (validates that `add_read_callback` registrations are visible to the subscription engine).
- The Publish service path (driven by `SyncSampler` at the `min_sampling_interval_ms = 100` default cadence, gated by the requested 1000 ms publishing interval) delivers `DataChangeNotification` messages to the client's `DataChangeCallback` within 1 publish-interval of monitored-item creation.
- Subscription teardown (`delete_subscription`) succeeds without leaving residue in async-opcua's subscription state.

**No `src/` changes required.** Strong-prior reading of the async-opcua-server source was correct — `SimpleNodeManagerImpl` already implements the full monitored-item lifecycle and the existing `add_read_callback` registrations are the only opcgw-side wiring needed.

---

## 4. API surface (validated against 0.17.1)

The spike empirically validated the following API surfaces:

| Concept | Type/path | Notes |
|---|---|---|
| Server-side namespace registration | `simple_node_manager(NamespaceMetadata, name)` builder fn at `src/node_manager/memory/simple.rs:99` | opcgw's `OPCUA_NAMESPACE_URI = "urn:UpcUaG"` resolves to namespace index `2` in async-opcua's first user-supplied namespace under SimpleNodeManager. |
| Server-side read callback | `SimpleNodeManagerImpl::add_read_callback(node_id, fn(&NumericRange, TimestampsToReturn, f64) -> Result<DataValue, StatusCode>)` at `simple.rs:412` | Same callback signature opcgw already uses for read service. The sampler invokes this on each tick when a monitored item exists. |
| Server-side sampler | `SyncSampler::run(min_sampling_interval, subscriptions)` at `simple.rs:127, 132–144`, plus `add_sampler` / `update_sampler` / `set_sampler_mode` / `remove_sampler` | The sampler ticks at `Limits::subscriptions::min_sampling_interval_ms` (default `100 ms`); per-monitored-item sampling intervals are honoured but rounded up to that floor. |
| Client-side subscription creation | `Session::create_subscription(publishing_interval, lifetime_count, max_keep_alive_count, max_notifications_per_publish, priority, publishing_enabled, callback) -> Result<u32, StatusCode>` at `service.rs:1502` | Returns the server-assigned subscription_id. Callback is `OnSubscriptionNotificationCore + 'static` — typically a `DataChangeCallback::new(closure)`. |
| Client-side monitored-item creation | `Session::create_monitored_items(subscription_id, TimestampsToReturn, Vec<MonitoredItemCreateRequest>) -> Result<Vec<CreatedMonitoredItem>, StatusCode>` at `service.rs:1775` | Returns one result per request; result has `.result.status_code` and `.result.monitored_item_id`. |
| Client-side notification callback | `DataChangeCallback::new(impl FnMut(DataValue, &MonitoredItem) + Send + Sync + 'static)` at `callbacks.rs:137` | Convenience wrapper; for full status-change + event handling, implement `OnSubscriptionNotification` directly. |
| Subscription deletion | `Session::delete_subscription(subscription_id)` (and `delete_subscriptions(&[id])`) at `service.rs:1703` | Idempotent. Test cleanup uses this; production teardown should as well. |

**Surfaces NOT validated** (out of scope for spike, deferred to 8-2):
- `modify_subscription` / `modify_monitored_items` / `set_publishing_mode` / `transfer_subscriptions`: validated by source-grep but not exercised live.
- Event-type monitored items (`OnSubscriptionNotification::on_event`): out of scope (Story 8-4 territory for alarm conditions).
- HistoryRead service: out of scope (Story 8-3 territory).

---

## 5. `Limits` reachability table (AC#6)

The async-opcua 0.17.1 `Limits` and `SubscriptionLimits` structures reach `ServerBuilder` via three paths:
- **Direct setter** — `ServerBuilder::<field_name>(value)` available; preferred.
- **`limits_mut()`** — `builder.limits_mut().<field> = value` or `builder.limits_mut().subscriptions.<field> = value`. Field still gets set, but the builder fluent chain is interrupted.
- **`limits(Limits)`** — pass a fully-built `Limits` struct, replacing the entire defaults set. Heaviest hammer; appropriate when most fields need overrides.

### Section A — subscription-relevant (load-shaping) knobs for Story 8-2

| Field path | Default | Direct setter? | Recommended exposure as `[opcua].xxx` config knob |
|---|---|---|---|
| `Limits.subscriptions.max_subscriptions_per_session` | 10 (`MAX_SUBSCRIPTIONS_PER_SESSION`) | **No** — only `limits_mut().subscriptions.max_subscriptions_per_session = N` | **Must** — listed in `epics.md:682, 724` |
| `Limits.subscriptions.max_monitored_items_per_sub` | 1000 (`DEFAULT_MAX_MONITORED_ITEMS_PER_SUB`) | **No** — only `limits_mut().subscriptions.max_monitored_items_per_sub = N` | **Must** — listed in `epics.md:682, 724`. ⚠ **Field-name asymmetry**: spec calls it `max_monitored_items_per_subscription`; library calls it `max_monitored_items_per_sub`. Story 8-2 must use the library name. |
| `Limits.max_message_size` | comes from `opcua_types::constants::MAX_MESSAGE_SIZE` (resolved at runtime; not validated by spike) | **Yes** — `.max_message_size(usize)` | **Must** — listed in `epics.md:682, 724` |
| `Limits.max_chunk_count` | comes from `opcua_types::constants::MAX_CHUNK_COUNT` (resolved at runtime; not validated by spike) | **Yes** — `.max_chunk_count(usize)` | **Must** — listed in `epics.md:682, 724` |
| `Limits.subscriptions.max_pending_publish_requests` | 20 (`MAX_PENDING_PUBLISH_REQUESTS`) | **No** — `limits_mut()` only | **Should** — caps client-side flow control. Subscription-flood protection at the publish-pipe layer. Surface so operators can lower from 20 if they want stricter back-pressure under load. |
| `Limits.subscriptions.max_publish_requests_per_subscription` | 4 (`MAX_PUBLISH_REQUESTS_PER_SUBSCRIPTION`) | **No** — `limits_mut()` only | **Should** — interacts with `max_pending_publish_requests` (the smaller of the two wins). Surface as documentation reminder; rarely tuned. |
| `Limits.subscriptions.min_sampling_interval_ms` | 100.0 (`MIN_SAMPLING_INTERVAL_MS = SUBSCRIPTION_TIMER_RATE_MS`) | **No** — `limits_mut()` only | **Should** — sets the floor for client-requested sampling intervals. Operators may want to **raise** this (e.g., to 500 ms) to prevent over-sampling under high-monitored-item load. |
| `Limits.subscriptions.max_keep_alive_count` | 30000 (`MAX_KEEP_ALIVE_COUNT`) | **No** — `limits_mut()` only | **Could** — operator-only debug. Default is 30000 publish-intervals; rarely tuned. |
| `Limits.subscriptions.max_queued_notifications` | 20 (`MAX_QUEUED_NOTIFICATIONS`) | **No** — `limits_mut()` only | **Should** — back-pressure cap on per-subscription notification queue. Beyond this, notifications are dropped. Surface so operators can detect drops. |

### Section B — other limits (lower priority for Story 8-2)

| Field path | Direct setter? | Notes |
|---|---|---|
| `Limits.max_array_length` | Yes — `.max_array_length(usize)` | Top-level. Operator-tunable for large-payload metrics; out of scope unless metric-array values land in Phase B. |
| `Limits.max_string_length` | Yes — `.max_string_length(usize)` | Top-level. Out of scope. |
| `Limits.max_byte_string_length` | Yes — `.max_byte_string_length(usize)` | Top-level. Out of scope. |
| `Limits.send_buffer_size` | Yes — `.send_buffer_size(usize)` | Default 65535. Operator-only debug. |
| `Limits.receive_buffer_size` | Yes — `.receive_buffer_size(usize)` | Default 65535. Operator-only debug. |
| `Limits.max_browse_continuation_points` | Yes — `.max_browse_continuation_points(usize)` | Default 5000. Out of scope until address-space mutation lands (Epic 9). |
| `Limits.max_history_continuation_points` | Yes — `.max_history_continuation_points(usize)` | Default 500. **Story 8-3 territory** — re-evaluate when HistoryRead lands. |
| `Limits.max_query_continuation_points` | Yes — `.max_query_continuation_points(usize)` | Default 500. Out of scope. |
| `Limits.max_sessions` | Yes — `.max_sessions(usize)` | **Already wired by Story 7-3** as `[opcua].max_connections`. Do not duplicate. |
| `Limits.subscriptions.max_monitored_item_queue_size` | No — `limits_mut()` only | Default 10. Per-monitored-item buffer. Operator-only debug. |
| `Limits.subscriptions.max_lifetime_count` | No — `limits_mut()` only | Default `MAX_KEEP_ALIVE_COUNT * 3 = 90000`. Operator-only debug. |
| `Limits.subscriptions.max_notifications_per_publish` | No — `limits_mut()` only | Default 0 (= unlimited). Operator-only debug. |
| `Limits.subscriptions.default_keep_alive_count` | No — `limits_mut()` only | Default 10. Operator-only debug. |
| `Limits.subscriptions.min_publishing_interval_ms` | No — `limits_mut()` only | Default 100.0. Symmetric to `min_sampling_interval_ms`. |
| `Limits.operational.*` (17 fields: `max_nodes_per_read`, `max_nodes_per_write`, `max_subscriptions_per_call`, etc.) | No — `limits_mut()` only | Default values are generally permissive (10000 for reads/writes, 100 for various). Out of scope for Phase B — operators rarely tune these. |
| `ServerBuilder::subscription_poll_interval_ms` | Direct setter (top-level, not in Limits) | Sets `config.subscription_poll_interval_ms`. **Distinct from `Limits.subscriptions.min_publishing_interval_ms`**. Default behaviour: not validated by spike. |
| `ServerBuilder::publish_timeout_default_ms` | Direct setter (top-level) | Default 30000 (`DEFAULT_PUBLISH_TIMEOUT_MS`). |
| `ServerBuilder::max_timeout_ms` | Direct setter (top-level) | Operator-only. |
| `ServerBuilder::max_secure_channel_token_lifetime_ms` | Direct setter (top-level) | Operator-only. |
| `ServerBuilder::max_session_timeout_ms` | Direct setter (top-level) | Default 60000. **Distinct from `Limits.max_sessions`**. |

**Source confirmations:**
- Default constants — `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/lib.rs:60–145`.
- Field paths — `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/config/limits.rs:1–127`.
- Direct-setter availability — `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/builder.rs` (16 setters in the `max_*` / `min_*` / `*_buffer_size` / `*_count` / `*_size` / `*_timeout_*` / `subscription_poll_interval_ms` / `publish_timeout_default_ms` ranges enumerated by grep).

---

## 6. Pull vs push (AC#2)

**Recommendation for Story 8-2: stay pull. The existing `add_read_callback` + `SyncSampler` model is sufficient for Phase B's sizing target.**

**Rationale:**
- The integration test confirms subscriptions fire end-to-end at the AC#1 sizing (1 monitored item × 1 subscriber, 1 Hz publishing interval). The throughput probe at the full AC#8 sizing (100 items × 1 Hz × 1 subscriber × 5 min) was **NOT** automated during this dev-story run — it requires a running gateway and 5 minutes of wall clock. **The pull-vs-push call below is provisional pending the operator's `--load-probe` execution.**
- Provisional reasoning: the sampler runs at `min_sampling_interval_ms = 100 ms` regardless of monitored-item count (a single global timer drives all samplers — see `simple.rs:127, 133`). At 100 items × 1 Hz, the sampler does ~1000 read-callback invocations per second; each callback is a SQLite `metric_values` lookup (sub-millisecond per Story 5-2 measurements). Total expected CPU: well under 5%. **Lock contention is the key risk** — the SQLite read pool checkout could starve with 1000 ops/sec, but Story 5-1 / 5-2 sized the pool for poll-cycle write bursts of 100s of writes; 1000 reads/sec is well within capacity.
- **Push model adds complexity without clear benefit.** A push pipeline (poller → channel → opc_ua-side consumer → `SimpleAddressSpace::set_variable_value` per cycle) would replace 1000 sampler-driven SQLite reads with 100 pushed updates per poll cycle — net **fewer** SQLite reads, but it adds (a) a new `mpsc` channel between subsystems, (b) a consumer task in opc_ua, (c) a push-set call site that needs error handling, and (d) the existing pull model becomes a fallback rather than the only path. The complexity cost is ~150–300 LOC of new code in `src/opc_ua.rs` + `src/chirpstack.rs` for an optimisation that is not yet needed.

**Decision rule for Story 8-2:** plan for pull. **If the operator's actual `--load-probe` run reveals p99 inter-notification interval > 3000 ms (= 3 × publishing interval) or notification drop count > 5% of expected (= 1500 of 30000), Story 8-2 must re-evaluate the push model.** The operator's run output should be appended to this report's § 7 below.

**Push-model integration sketch (for completeness, not for 8-2 implementation unless triggered):**
```rust
// Direction: chirpstack.rs (poller, writer) → opc_ua.rs (consumer, reader)
// Current pull: opc_ua.rs::add_read_callback(node_id, |...| storage.get_metric_value(...))
// Push variant:
//   1. Poller emits Vec<(NodeId, DataValue)> on tx every poll cycle.
//   2. opc_ua-side consumer task: while let Some(updates) = rx.recv().await {
//        for (id, dv) in updates {
//            address_space.write().set_variable_value(&id, dv);
//            // The sampler observes the write on its next tick and emits notifications.
//        }
//      }
//   3. Read callback stays for clients that hit the OPC UA Read service directly
//      (subscription engine uses the address-space value, not the callback).
// Net: 80–150 LOC + a `tokio::sync::mpsc` channel (no new crate).
```
This sketch is in the report, not in code, per AC#2.

---

## 7. Throughput measurement (AC#8) — DEFERRED

**Status: DEFERRED — operator action required.** The `--load-probe` run is automated end-to-end in the spike binary but requires a running gateway and 5 minutes of wall clock. It was not executed during this dev-story run.

**Operator instructions:**
```bash
# Terminal A — start the gateway with diagnostic logging:
RUST_LOG=opcgw=info,opcua_server=warn cargo run -- -c tests/config/config.toml \
    > log/gateway-load-probe.log 2>&1 &

# Terminal B — wait ~5 s for bind, then run the probe:
cargo run --release --example opcua_subscription_spike -- \
    --load-probe \
    --user test-user \
    --password test-pass \
    --load-items 100 \
    --load-secs 300 \
    --load-publish-ms 1000 \
    --load-nodes "Metric01,Metric02,Metric03,Metric04,Metric05,Metric06" \
    2> /tmp/load-probe-summary.json

# Terminal A: capture peak resource usage manually during the run:
top -p $(pgrep opcgw) -b -n 60 -d 5 > /tmp/load-probe-top.log
ls /proc/$(pgrep opcgw)/fd | wc -l   # at start, mid, end
```

**Pre-population required:** the gateway's address space must contain at least one of the named metrics (`Metric01` through `Metric06` in the `--load-nodes` list above). Use `tests/config/config.toml` or extend with a load-probe-specific config file.

**Expected result columns** (table to be filled in by operator):

| Metric | Target (PASS) | Yellow flag | Red flag | Actual |
|---|---|---|---|---|
| Total notifications received | ≥ 28,500 | 25,000–28,499 | < 25,000 | _TBD_ |
| Median first-notification latency | < 1500 ms | 1500–3000 ms | > 3000 ms | _TBD_ |
| p95 inter-notification interval | < 1500 ms | 1500–3000 ms | > 3000 ms | _TBD_ |
| p99 inter-notification interval | < 3000 ms | 3000–10000 ms | > 10000 ms | _TBD_ |
| Notifications dropped | 0 | 1–1500 | > 1500 | _TBD_ |
| Peak CPU% | (operator records) | — | — | _TBD_ |
| Peak RSS | (operator records) | — | — | _TBD_ |
| Peak fd count | (operator records) | — | — | _TBD_ |

**Gauge usefulness verdict** (AC#8 sub-criterion):
- Procedure: count `event="opcua_session_count"` lines in `log/gateway-load-probe.log` (expected ~60 over 5 min). Inspect the surrounding 5-second window of each gauge tick and count unrelated `info!` lines.
- Verdict if **signal-clear** (< 50 unrelated `info!` per gauge tick): "gauge stays hard-coded — Story 8-2 leaves `OPCUA_SESSION_GAUGE_INTERVAL_SECS` as a constant".
- Verdict if **noise-buried** (≥ 50 unrelated `info!` per gauge tick): "Story 8-2 promotes `OPCUA_SESSION_GAUGE_INTERVAL_SECS` to `[diagnostics].session_gauge_interval_secs` per `epics.md:727`, OR replaces the gauge with a per-subscription notification-rate metric. Choice is at 8-2's discretion."
- _Operator-supplied verdict here: TBD._

---

## 8. Automated test inventory (AC#9 + test-depth additions)

**Nine integration tests in `tests/opcua_subscription_spike.rs` — all passing as of 2026-04-30.** The first three pin AC#1 + AC#9; the remaining six were added as test-depth compensation for the deferred manual SCADA verification (per user decision 2026-04-30; AC#3 explicitly deferred to a single integration pass after Epic 9 — see `deferred-work.md` "Story 8-1" block).

| # | Test | Pins / Asserts | Outcome |
|---|---|---|---|
| 1 | `test_subscription_basic_data_change_notification` | **AC#1.** End-to-end `CreateSubscription` → `CreateMonitoredItems` → `Publish` → `DataChangeNotification` against a single `Temperature` NodeId. Within 10 s of monitored-item creation. | ✅ |
| 2 | `test_subscription_client_rejected_by_auth_manager` | **AC#9 (auth).** Wrong password → `BadUserAccessDenied`. Captured log line co-occurrence: `event="opcua_auth_failed"` AND `user=opcua-user`. **Field-name correction recorded:** the actual emit field is `user=` (not `username=`); Story 8-2 reviewers should assume the `user=` field name. | ✅ |
| 3 | `test_subscription_client_rejected_by_at_limit_layer` | **AC#9 (cap).** One-over-cap session → fails to activate. Captured log line co-occurrence: `event="opcua_session_count_at_limit"` AND `source_ip=127.0.0.1` AND `limit=1` AND `current=1`. | ✅ |
| 4 | `test_subscription_two_clients_share_node` | **`epics.md:720` multi-client invariant.** Two simultaneous sessions create independent subscriptions on the same NodeId; both receive notifications within 10 s. Pins per-client publish-loop independence. | ✅ |
| 5 | `test_subscription_ten_monitored_items_per_subscription` | **Per-monitored-item branch.** One subscription with 10 monitored items (client_handles 1..=10); every handle receives at least one notification within 15 s. Exercises the publish loop's per-item iteration without needing a 100-item fixture. | ✅ |
| 6 | `test_subscription_sampling_interval_revised_to_minimum` | **Server-side limit revision.** Request `sampling_interval = 50.0` (sub-floor); server revises to ≥ 100.0 (`MIN_SAMPLING_INTERVAL_MS = SUBSCRIPTION_TIMER_RATE_MS = 100` per `lib.rs:73, 77`). Important reference for Story 8-2's `[opcua].min_sampling_interval_ms` config knob: **operators get silent revision, not rejection, when their requested value is below the floor.** | ✅ |
| 7 | `test_subscription_datavalue_payload_carries_seeded_value` | **Value-flow path.** Pre-seed `42.5` via `backend.batch_write_metrics`; subscribe; assert the delivered DataValue carries `Variant::Float(42.5)` AND a Good status code. Pins that the pipeline is not just *firing* but actually *delivering values* end-to-end. | ✅ |
| 8 | `test_subscription_double_delete_is_safe` | **Teardown idempotency.** First `delete_subscription(id)` succeeds; second on the same id returns `Err(BadInvalidArgument)` (no panic, no Ok). Important for Story 8-2's production teardown which can run twice across cancellation paths. | ✅ |
| 9 | `test_subscription_survives_sibling_session_disconnect` | **Per-session state isolation.** Two sessions × subscriptions on same NodeId; seed initial value `1.0`; baseline notifications fire; disconnect session 1; seed change to `2.0`; assert session 2 receives `Variant::Float(2.0)` notification within 10 s. Pins the invariant that a misbehaving SCADA client closing its session does not cascade-fail other clients' subscriptions. **Implementation note:** async-opcua's MonitoredItem dedupes successive identical samples per OPC UA spec; the value-change seed sequence is what triggers the second notification (and was the diagnostic for an early test failure — recorded in spike-binary Debug Log References for future test-author reference). | ✅ |

**Wall-clock cost:** all 9 tests run in ~22 s on dev hardware (sequential via `#[serial_test::serial]` — three of them share the global tracing-test buffer). CI-friendly.

**No new auth or audit-event infrastructure introduced by Epic 8.** The Phase A carry-forward NFR12 acknowledgment from `epics.md:705` is satisfied.

**Test-depth rationale.** Per the user decision 2026-04-30 to defer manual FUXA + Ignition / UaExpert verification to a single integration pass after Epic 9 lands, tests #4–#9 above were added to compensate. The areas they cover (concurrency, cleanup, value-flow, server-side limit behaviour) are the regression risks most likely to surface in a real SCADA deployment that an automated test can detect ahead of time. Tests #4–#9 are deliberately not 8-2 scope (no production-code-driven assertions, no config-knob plumbing tests) — they exercise behaviour that is *already wired in opcgw at HEAD* but was previously only covered indirectly. This widens the spike's regression net without crossing into 8-2's spec ownership.

---

## 9. Session-rejected callback re-check (AC#7)

**Result: NO HOOK in async-opcua 0.17.1.** Re-confirmed via direct source inspection on 2026-04-29.

**Evidence:**
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/session/manager.rs:64-76`:
  ```rust
  pub(crate) fn create_session(
      &mut self,
      channel: &mut SecureChannel,
      certificate_store: &RwLock<CertificateStore>,
      request: &CreateSessionRequest,
  ) -> Result<CreateSessionResponse, StatusCode> {
      if self.sessions.len() >= self.info.config.limits.max_sessions {
          return Err(StatusCode::BadTooManySessions);
      }

      // TODO: Auditing and diagnostics.
      ...
  }
  ```
  - The function is `pub(crate)` — not externally callable.
  - The rejection path emits no log and invokes no callback.
  - **The library's own `// TODO: Auditing and diagnostics` comment at `manager.rs:74` is the strongest possible upstream-FR hook.** The maintainers themselves flag the gap.
- `~/.cargo/registry/src/.../async-opcua-server-0.17.1/src/builder.rs`: no `on_session_rejected` / `register_listener` / `with_session_callback` builder method.
- Disk inventory shows only `async-opcua-server-0.17.1` — no patch releases or 0.18.x available.

**Action item (HALT-blocking — operator action required):**

File an upstream feature request at `https://github.com/freeopcua/async-opcua/issues` **before Story 8-2 begins**. Suggested issue body:

```
Title: Session-rejected callback / structured-emit on BadTooManySessions

Use case: downstream gateways (e.g. opcgw, https://github.com/<...>/opcgw) need
operator-visible audit events when sessions are rejected for hitting
`max_sessions`. Today the rejection at `SessionManager::create_session`
(src/session/manager.rs:70-72) returns Err(StatusCode::BadTooManySessions)
silently, with the existing `// TODO: Auditing and diagnostics` comment at
manager.rs:74 acknowledging the gap.

Current workaround: a tracing-subscriber Layer that observes the
pre-existing `info!("Accept new connection from {addr}")` event at server.rs:367
and re-emits at `warn!` when the session count is at the cap. See
opcgw/src/opc_ua_session_monitor.rs (commit <SHA>) for the implementation.
This works but is brittle to async-opcua's emission style — a future change
to that info-event format silently breaks the correlation.

Proposed shape (non-prescriptive — choose whichever fits the library's
internal model):
  (a) A callback registerable via ServerBuilder, e.g.
      `.on_session_rejected(|session_creation_request, status_code| { ... })`,
      invoked at the rejection branch with the request context.
  (b) A structured tracing event at the rejection point, e.g.
      `tracing::warn!(event = "session_rejected", reason = ?status_code,
                       endpoint_url = %request.endpoint_url, ...)`.
  Either lets downstream gateways observe rejections without parsing
  unstructured log strings.

Linked downstream tracking entry:
  opcgw/_bmad-output/implementation-artifacts/deferred-work.md
  ("First-class session-rejected event in async-opcua")
```

**Once filed, update `deferred-work.md`** in-place: the existing entry under "Story 7-3 deferred" → "First-class session-rejected event in async-opcua" should have its closing sentence "File an upstream feature request to extend `SessionManager` with a rejection callback or log emission; revisit when async-opcua ships such a hook." replaced with "Filed upstream FR: <URL>."

---

## 10. Plan B — NOT TRIGGERED

Plan A is confirmed. Plan B (locka99/opcua migration / upstream contribution / change-detection polling layer) is not engaged.

**Future-engagement footnote:** if a later async-opcua upgrade (0.18.x, 0.19.x) breaks Plan A, this report's § 6 push-model sketch and Story 8-1's spec § "Plan A failure-mode decision tree" are the entry points for re-triggering Plan B planning. No Plan B prep work is required at this time.

---

## 11. Implications for Story 8-2

Concrete spec hooks for `bmad-create-story 8-2`:

1. **Plumb four `Limits` knobs through `OpcUaConfig`** — `max_subscriptions_per_session`, `max_monitored_items_per_sub` (note: NOT `max_monitored_items_per_subscription`), `max_message_size`, `max_chunk_count`. Default values from `lib.rs:60-145`. Validate non-zero and an upper bound (mirror Story 7-3's `max_connections` pattern: `OPCUA_DEFAULT_*` + `OPCUA_*_HARD_CAP` constants in `src/utils.rs`, fail-closed `validate()` checks in `src/config.rs`).
2. **Use `ServerBuilder::limits_mut()` for the two subscription-limit fields** that have no direct setter; use `.max_message_size(...)` and `.max_chunk_count(...)` directly for the other two. Do NOT pass a fully-built `Limits` struct via `.limits(...)` — that overrides every default and risks regressions on fields the spec doesn't touch.
3. **Add Section A's "should-expose" knobs** as a **second tier** of config knobs, gated under a new `[opcua.subscription_advanced]` block (or as five additional top-level `[opcua]` fields with clear "advanced; default usually fine" doc comments). Operators tuning subscription-flood scenarios will need them; default deployments should never touch them. The five candidates: `max_pending_publish_requests`, `max_publish_requests_per_subscription`, `min_sampling_interval_ms`, `max_keep_alive_count`, `max_queued_notifications`.
4. **Auth + cap composition test** — keep the two AC#9 integration tests from `tests/opcua_subscription_spike.rs` as Story 8-2's regression baseline. Story 8-2's own integration tests should add: subscription-flood from a single client (verifies `max_subscriptions_per_session` enforcement), monitored-item-flood from a single subscription (verifies `max_monitored_items_per_sub` enforcement), and a "subscription survives ChirpStack outage" smoke (verifies `epics.md:721` AC).
5. **NFR12 silent-degradation reminder** — the gateway's startup-warn from commit `344902d` already alerts operators when log level filters out info. Story 8-2 should add a one-liner to `docs/security.md` "OPC UA connection limiting" extending the audit-trail guidance to subscription clients (no new audit infrastructure, but documentation must reflect that subscription-creating clients hit the same auth + cap paths).
6. **Per-IP rate limiting (#88)** — out of scope for 8-2 unless a real subscription-flood operator scenario surfaces by 8-2's start. The `--load-probe` numbers (when collected) are the empirical input for this decision.
7. **Field-name correction in spec body** — use `max_monitored_items_per_sub`, not `max_monitored_items_per_subscription`. Cross-reference `epics.md:682, 724` and update those bullets in 8-2's spec polish if not already done.
8. **Subscription engine works at HEAD with no `src/` changes.** Story 8-2's only mandatory `src/` work is config plumbing + ServerBuilder limits wiring. The subscription delivery path is already wired by async-opcua's `SimpleNodeManagerImpl`.

**Test baseline carry-forward:** Epic 7 baseline 581 pass / 0 fail / 7 ignored. Story 8-1 added 9 integration tests in `tests/opcua_subscription_spike.rs`. **Final post-spike count: 592 pass / 0 fail / 7 ignored, clippy clean** (verified 2026-04-30 via `cargo test --lib --bins --tests` + `cargo clippy --all-targets -- -D warnings`). Story 8-2 should target ≥ 600 pass with the subscription-cap and monitored-item-cap pins added.

---

## 12. References

- [Story 8-1 spec: `_bmad-output/implementation-artifacts/8-1-async-opcua-subscription-spike.md`]
- [Epic 8 spec: `_bmad-output/planning-artifacts/epics.md` lines 671–705 (file's "Epic 7" = sprint-status's "Epic 8")]
- [Phase A carry-forward bullets: `epics.md:678–684`]
- [PRD risk: `prd.md:193` — "async-opcua can't do subscriptions"]
- [PRD FR21: `prd.md:378` — subscription-based data change notifications]
- [PRD NFR22: `prd.md:461` — FUXA + at least one additional OPC UA client]
- [Architecture push-model design: `architecture.md:211–215`]
- [Epic 7 retro Discovery 1 (NFR12 silent degradation): `_bmad-output/implementation-artifacts/epic-7-retro-2026-04-29.md` lines 211–222]
- [Story 7-2 deferred entry "First-class session-rejected event in async-opcua": `_bmad-output/implementation-artifacts/deferred-work.md` ~line 119]
- [Story 7-3 deferred entries (#88, #89, #90 carry-forward): `_bmad-output/implementation-artifacts/deferred-work.md` lines 139–144]
- [async-opcua-server 0.17.1 source root: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/`]
- [`SimpleNodeManagerImpl` subscription wiring: `src/node_manager/memory/simple.rs:127, 132–144, 180–262, 304–314`]
- [`ServerBuilder` setters: `src/builder.rs:246, 254, 263, 380, 422, 428, 439, 460–530`]
- [`Limits` and `SubscriptionLimits` field definitions: `src/config/limits.rs:1–127`]
- [Library default constants: `src/lib.rs:60–145`]
- [Session-rejection path (no callback): `src/session/manager.rs:64–76`]
- [async-opcua-client 0.17.1 subscription API: `src/session/services/subscriptions/service.rs:1502, 1775, 1703`]
- [async-opcua-client 0.17.1 callback wrappers: `src/session/services/subscriptions/callbacks.rs:127–149`]
- [opcgw subscription wire points: `src/opc_ua.rs:194 (namespace), 723/810/872/880/888 (read callbacks)`]
- [opcgw auth manager: `src/opc_ua_auth.rs` (Story 7-2)]
- [opcgw at-limit layer: `src/opc_ua_session_monitor.rs` (Story 7-3)]
- [Spike binary: `examples/opcua_subscription_spike.rs`]
- [Spike integration tests: `tests/opcua_subscription_spike.rs`]
- [CLAUDE.md scope-discipline rule, per-story commit rule, code-review loop discipline]

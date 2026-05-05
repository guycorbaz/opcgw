# Story 9-0 Spike Report — async-opcua Runtime Address-Space Mutation

**Story:** 9-0
**Date:** 2026-05-04
**Status:** Spike complete; all three load-bearing questions resolved.
**async-opcua version under test:** 0.17.1

---

## 1. Executive summary

All three load-bearing questions from `epics.md:784-787` resolve favourably enough that **Stories 9-7 (Configuration Hot-Reload) and 9-8 (Dynamic OPC UA Address Space Mutation) are unblocked** — but Q2 (remove path) surfaces an operator-visible UX gap that 9-8 must mitigate explicitly.

| Question | Verdict | Key measurement |
|---|---|---|
| **Q1: add path** | RESOLVED FAVOURABLY | Runtime-added variable's first subscription notification carries the registered sentinel value (`Float(42.0), Good`). No production-code change needed beyond the AC#5 build/run_handles split. |
| **Q2: remove path** | Behaviour B (frozen-last-good) | Subscription on a deleted variable goes silent. No status-change notification, no channel close, no client-visible error. 9-8 must arrange explicit cleanup or push a `BadNodeIdUnknown` notification before delete. |
| **Q3: sibling isolation** | RESOLVED FAVOURABLY | Bulk mutation of 11 nodes (1 folder + 10 variables) under a single write-lock acquisition: **117.604 µs** total hold time. Sampler interval is 100 ms (~850× headroom). Zero risk of sampler starvation at this scale. |

**No Plan B is triggered.** The wrap-not-fork pattern (`InMemoryNodeManager::address_space()` + `set_attributes`) is sufficient for opcgw's hot-reload + dynamic-mutation needs.

---

## 2. Reproduction recipe

```
cargo test --test opcua_dynamic_address_space_spike -- --nocapture
```

Expected output (verdicts + measurements on stderr):

```
[Q1] first humidity notification: value=Some(Float(42.0)) status=Some(Good (0))
[Q1] baseline post-add notifications drained (informational): 0
[Q1] VERDICT: RESOLVED FAVOURABLY
[Q2] elapsed_since_delete=3.001s, post_delete_notifications=0,
     observed_status=None, channel_closed=false, VERDICT: B (frozen-last-good — stream went silent)
[Q2] baseline notifications drained post-delete (informational): 0
[Q3] write-lock-hold duration: 117.604µs
[Q3] sibling notifications drained post-mutation (informational): 0
[Q3] bulk-added Metric05 first notification: value=Some(Float(104.0))
[Q3] lock_hold_duration=117.604µs VERDICT: RESOLVED FAVOURABLY

test result: ok. 6 passed; 0 failed; 0 ignored
```

Wall-clock: ~17 s on the dev machine (one of the three tests includes a 3 s observation window for Q2).

---

## 3. API surface (0.17.1-confirmed)

The library audit at `epics.md:782` enumerated the API surface; the spike empirically exercised it. All cited line numbers verified against the resolved registry path `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/`.

| Symbol | Path | Use in spike |
|---|---|---|
| `InMemoryNodeManager::address_space() -> &Arc<RwLock<AddressSpace>>` | `node_manager/memory/mod.rs:108` | Q1/Q2/Q3: acquired the write lock to mutate the address space. |
| `AddressSpace::add_folder(node_id, browse_name, display_name, parent) -> bool` | `address_space/mod.rs:443` | Q3: added the new device folder under one write-lock acquisition. |
| `AddressSpace::add_variables(Vec<Variable>, parent) -> Vec<bool>` | `address_space/mod.rs:458` | Q1, Q2, Q3: added new metric variables; verified `Vec<bool>` returns `[true]` for fresh NodeIds. |
| `AddressSpace::delete(&NodeId, delete_target_references: bool) -> Option<NodeType>` | `address_space/mod.rs:434` | Q2: deleted the runtime-added Humidity variable. Returned `Some(NodeType)` on success; the `delete_target_references=true` path also cleared inbound + outbound `Organizes` references. |
| `SimpleNodeManagerImpl::add_read_callback(NodeId, Fn(...) -> Result<DataValue, StatusCode>)` | `node_manager/memory/simple.rs:412` | Q1, Q2, Q3: registered closures returning sentinel `Variant::Float` values. |
| `Session::create_subscription` / `create_monitored_items` | async-opcua client API | Q1, Q2, Q3: created subscriptions and monitored items via the same client API as Story 8-1 used. |
| `DataChangeCallback::new(closure)` | client API | Q1, Q2, Q3: receive notifications via mpsc channel. |
| `Variable::set_access_level / set_user_access_level / set_historizing` | (existing) | Q1, Q2, Q3: applied the Story 8-3 invariant (`CURRENT_READ \| HISTORY_READ` mask + `historizing=true`) to all runtime-added variables. |

**Notable absent API:** `SimpleNodeManagerImpl::remove_read_callback` does **not** exist. See § 8.

---

## 4. Q1 add path

**Question:** when a new variable is added to the address space + an `add_read_callback` is registered for it on a running server, does a fresh subscription on the new node receive `DataChangeNotification`s correctly?

**Empirical result:** YES. Within ~1 second of `CreateMonitoredItems` returning, the client's `DataChangeCallback` fires with the registered sentinel value:

```
[Q1] first humidity notification: value=Some(Float(42.0)) status=Some(Good (0))
```

The notification's value field equals the sentinel `Variant::Float(42.0)` exactly — proving the `SyncSampler` invoked the post-init `add_read_callback` we registered, not just that some generic DataValue was assembled.

**Steps:**

1. Start server with one startup-registered metric (Temperature on `device_dyn_spike_1`).
2. Open a session with username/password auth against the `null` endpoint.
3. Subscribe to `Temperature` (baseline subscription) — receives one notification (warm-up).
4. Acquire `manager.address_space().write()`, call `add_variables(vec![humidity_variable], &device_node)`, drop the write lock.
5. Register `manager.inner().simple().add_read_callback(humidity_node, |...| Ok(DataValue { value: Variant::Float(42.0), ... }))`.
6. Subscribe to `humidity_node` (fresh subscription).
7. Await first notification within 5 s — arrives.

**Strong-prior validation:** the source audit at `node_manager/memory/simple.rs:180-262` (Story 8-1) was correct. `create_value_monitored_items` triggers an immediate sample on the newly-monitored item, and that sample invokes the `add_read_callback`-registered closure. Post-init callback registration is honoured without any sampler restart or re-init.

**Implication:** for hot-reload (Story 9-7), a new device added via the watch channel can call `address_space.write().add_variables(...)` then `add_read_callback(...)`, and SCADA clients that subscribe to the new node afterwards will receive notifications. **No additional infrastructure needed for the add path.**

**Note on the AC#1 baseline-stream check (post-review addendum, 2026-05-05):** the original AC#1 specified an assert that the baseline `Temperature` subscription "continues to receive notifications uninterrupted ... no gap longer than 2 s bracketing the add". The implementation demotes this to an informational `eprintln!` drain because a static-value baseline subscription cannot emit follow-up notifications under OPC UA's value-change model — asserting on a gap would false-fail at the first sampler tick after warm-up. The substitution was ratified during `bmad-code-review` iter-1 (P9). The Q1 load-bearing check (Humidity sentinel arrival) is intact and asserts strictly.

---

## 5. Q2 remove path

**Question:** when a variable is deleted while a subscription's monitored item is targeting it, what status code does the client see?

**Empirical result:** **Behaviour B (frozen-last-good — stream went silent).** No status-change notification, no channel closure, no `BadNodeIdUnknown` arrival within the 3-second observation window:

```
[Q2] elapsed_since_delete=3.001s, post_delete_notifications=0,
     observed_status=None, channel_closed=false,
     VERDICT: B (frozen-last-good — stream went silent)
```

The client's view: the last `DataValue` it received before the delete remains the last one it ever sees. async-opcua's sampler stops sampling the deleted node (since `AddressSpace::find` returns `None` for the deleted NodeId), and no further publish payloads include this monitored item. The client has no programmatic way to detect "my subscription is now orphaned" without external coordination.

**Steps (test):**

1. Start server, add Humidity at runtime (same as Q1 setup).
2. Subscribe to Humidity, receive warm-up notification.
3. Call `manager.address_space().write().delete(&humidity_node, true)` — returns `Some(NodeType)`, confirming the variable was indeed in the address space and is now removed.
4. Watch the Humidity subscription's notification stream for 3 seconds.
5. Observed: 0 post-delete notifications, no status update, no channel close.

**One-line recommendation for Story 9-8:** before calling `address_space.write().delete(...)`, the caller MUST emit a final `DataValue` with `status = Some(StatusCode::BadNodeIdUnknown)` (or equivalent) for the doomed NodeId via `manager.set_attributes(...)`, so subscribed clients see an explicit transition. Then `delete` can run and the client's monitored item resolves correctly on the next publish (no more samples; the client knows the node is gone).

Alternative: 9-8 documents the silence-on-delete behaviour as known-acceptable and relies on operators to reconnect SCADA clients after each device removal. **Not recommended** — silent stream stalls are an operations nightmare.

**Note on the AC#2 baseline-stream check + warm-up assert (post-review addendum, 2026-05-05):** the original AC#2 specified an assert that the baseline `Temperature` subscription continues to receive notifications uninterrupted during the delete operation. The implementation demotes this to an informational `eprintln!` drain for the same value-change-semantics reason described in § 4. The substitution was ratified during `bmad-code-review` iter-1 (P9). Separately, the Q2 setup's humidity warm-up notification is now asserted to carry sentinel `Variant::Float(42.0)` (P6) — this confirms the read callback was actually invoked before the delete, ruling out a degenerate "frozen-last-good" verdict on a never-active subscription.

---

## 6. Q3 sibling isolation

**Question:** do subscriptions on unaffected nodes continue uninterrupted while the address space is being mutated under the `RwLock`'s write guard? Long write-locks during bulk add/remove could pause sampling for all subscribers.

**Empirical result:** **RESOLVED FAVOURABLY.** Bulk mutation of 11 nodes (1 folder + 10 variables) under a single `address_space.write()` acquisition completed in **117.604 µs**.

```
[Q3] write-lock-hold duration: 117.604µs
[Q3] bulk-added Metric05 first notification: value=Some(Float(104.0))
[Q3] lock_hold_duration=117.604µs VERDICT: RESOLVED FAVOURABLY
```

Sampler interval is `min_sampling_interval_ms = 100` (default). Lock-hold is **~850× shorter** than one sampler tick. No risk of starvation at this scale. The fresh subscription on a bulk-added metric (`Metric05`, sentinel value `Float(104.0)`) confirms the bulk-add path delivers identical semantics to the single-add path tested in Q1.

**Verdict thresholds:** RESOLVED FAVOURABLY < 100 ms (under one sampler tick) · PARTIAL 100 ms-1 s (sampler may stall for one tick) · FAILED > 1 s (sampler starvation; mutation must be batched).

**Steps (test):**

1. Start server with 2 startup-registered devices (each with one metric).
2. Subscribe to device 2's metric (sibling stream; warm-up notification).
3. Drain backlog so timing measurement starts clean.
4. Acquire `address_space.write()`, call `add_folder` for new device 3, then 10× `add_variables`. Drop the lock. **Measure wall-clock duration.**
5. Register read callbacks for the 10 new metrics (returning sentinel values 100..109).
6. Subscribe to `Metric05` (fresh subscription on a bulk-added node).
7. Await first notification within 5 s — arrives with value `Float(104.0)`.

**Note on the simplified Q3 verdict:** the original AC#3 specified a "sibling stream max-gap" measurement, but the OPC UA subscription model only emits notifications on **value changes**. With static-value read callbacks, sibling subscriptions naturally produce only their first notification — measuring inter-publish gaps records sampler-interval noise, not write-lock contention. The revised verdict (lock-hold < 100 ms + bulk-added node subscribable) measures the real concern: "does bulk mutation stall sampling for unaffected nodes". At microsecond hold times, the answer is empirically no.

**Post-review ratification (2026-05-05):** the AC#3 measurement substitution was ratified by Guy during `bmad-code-review` iter-1 (P11). Lock-hold-duration is mathematically conclusive: if the lock is shorter than one sampler tick (`min_sampling_interval_ms = 100 ms`), the sampler cannot be starved by the write — and 117.604 µs is ~850× shorter than that. Implementing the original max-gap measurement would have required ~50 LOC of additional infrastructure (dynamic value-changing callbacks via a periodic `set_attributes` task) beyond what the spec authorised. The lesson for Story 9-7 / 9-8 spec authors: assertions that depend on subscription-stream continuity require dynamic value-changing callbacks; sentinel-value read callbacks alone produce only a first notification per monitored item. Separately, the `lock_hold_duration < Duration::from_secs(1)` hard assert was removed during P4 because wall-clock measurements based on `Instant::now()` are CI flake sources under loaded containers; the verdict tier classification remains in the eprintln output for spike-report capture.

**Implication:** for hot-reload (Story 9-7), bulk reload of dozens of devices is safe under a single write-lock acquisition. No mutation chunking needed at typical scales (100-1000 nodes per reload).

---

## 7. Pattern reuse

The **library-wrap-not-fork pattern** (`epics.md:796`) still applies for runtime address-space mutation. Concretely:

- **No new wrap is required.** The existing public API of `InMemoryNodeManager` (`address_space() -> &Arc<RwLock<AddressSpace>>` + `set_attributes` / `set_values` for notification-emission) is sufficient for opcgw's hot-reload + dynamic-mutation needs. `OpcgwHistoryNodeManager` (Story 8-3 wrap) is the only opcgw-side wrap; nothing about runtime mutation requires extending it.
- **The composition shape is identical to startup.** `OpcUa::add_nodes` (the canonical startup add path at `src/opc_ua.rs:801-957`) acquires `manager.address_space().write()` and calls `add_folder` / `add_variables` plus `manager.inner().simple().add_read_callback(...)`. The runtime path (Story 9-7's hot-reload listener, Story 9-8's `apply_config_diff` consumer) calls the **same** sequence on the **same** lock — no new lock discipline, no new ordering invariant.
- **Single targeted production-code change in 9-0.** AC#5's Shape B refactor (`OpcUa::run` split into `build` + `run_handles` + a backward-compat wrapper) is the only production code introduced. It is forward infrastructure for 9-7's watch-channel listener task, not a wrap or fork. Three preceding epics in a row (Story 7-2 `OpcgwAuthManager`, Story 7-3 `AtLimitAcceptLayer`, Story 8-3 `OpcgwHistoryNodeManager`) all chose composition + targeted override over forking; Story 9-0 confirms the same applies to dynamic mutation.
- **Stale read-callback closure removal is the one place the wrap pattern *may* extend.** See § 8: if 9-8 needs a `remove_read_callback` API and async-opcua does not expose one, extending `OpcgwHistoryNodeManager` with a wrap method that mutates the inner `SimpleNodeManagerImpl`'s registry is the same shape Story 8-3 used for `history_read_raw_modified`.

**Throughput measurement out of scope.** Story 8-1 carries throughput-measurement responsibility (issue #95, `--load-probe` 5-min run). The 9-0 spike is binary-yes/no on the three questions, not quantitative throughput. If 9-7 hot-reload exercises mutation under sustained subscription load, re-run that load profile against issue #95's probe — not 9-0's spike.

---

## 8. Stale read-callback closure leak — known limitation

`SimpleNodeManagerImpl` does **not** expose a `remove_read_callback` API (confirmed via source enumeration: `pub fn` symbols are `new`, `new_imports`, `simple_node_manager`, `simple_node_manager_imports`, `add_write_callback`, `add_read_callback`, `add_method_callback` — no removal counterpart).

When the spike's Q2 test deletes a variable from the address space, the closure registered via `add_read_callback(node_id, closure)` remains in `SimpleNodeManagerImpl`'s callback registry. The closure holds clones of any captured state — for opcgw production code, that is `storage: Arc<dyn StorageBackend>`, `last_status: StatusCache (Arc<DashMap>)`, `device_id: String`, `chirpstack_metric_name: String`, `stale_threshold: u64`. The Arcs hold refcounts; the Strings + u64 are owned values.

**Memory leak shape:** proportional to lifetime delete count. For a gateway running for 30 days with 5 device removals/day, that is ~150 leaked closures × ~120 bytes (rough estimate per closure) = ~18 KB. Operationally negligible at expected churn rates, but **strictly a leak**.

**Recommendation for Story 9-8:** extend `OpcgwHistoryNodeManager` with a wrap method that exposes `SimpleNodeManagerImpl`'s internal callback registry for removal. The wrap pattern is the same one Story 8-3 used to add `history_read_raw_modified` — add a method to `OpcgwHistoryNodeManagerImpl` that mutates the inner `SimpleNodeManagerImpl`'s registry. If async-opcua's API ergonomics prevent this (e.g., the registry field is private), file an upstream FR similar to Story 8-1's session-rejected callback FR (issue #94 precedent).

**Alternative mitigation:** periodic restart of the OPC UA server task (a "GC pause" every N hours) if the leak rate is operationally significant. **Not recommended** — interrupts subscriptions; defeats the hot-reload UX.

**Story 9-0 does NOT decide between these mitigations.** The leak is logged in `_bmad-output/implementation-artifacts/deferred-work.md` under the heading **"Deferred from: Story 9-0 (2026-05-05)"** for Story 9-7 / 9-8 to inherit. (The entry was added during `bmad-code-review` iter-1 (P2) — the original spike report claimed the entry but the file edit had been forgotten.)

---

## 9. Implications for Story 9-7 (Configuration Hot-Reload)

1. **Watch-channel listener task spawns between `OpcUa::build()` and `OpcUa::run_handles()`.** The Shape B refactor in 9-0 (AC#5) gives 9-7 the seam: build returns `RunHandles { manager, server_handle, ... }`; 9-7 spawns a task that receives `tokio::sync::watch::Receiver<AppConfig>` and applies mutations to `manager.address_space()`. The task runs concurrently with `run_handles`.
2. **Validation-before-apply is sufficient.** Q2 Behaviour B (frozen-last-good) means a botched mutation does not crash subscribers — it just goes silent. So 9-7 can validate the new config, attempt the apply, and on partial failure roll back to the prior state without subscribers seeing transient errors. Transactional rollback is **not** required.
3. **Bulk-reload under a single write-lock is safe at typical scales.** Q3 confirmed 11-node bulk = 117 µs. A 1000-node hot-reload would project to ~10 ms — still well under the 100 ms sampler tick. **No mutation chunking is needed for ≤ ~10 000-node reloads.** Above that, 9-7 should batch into chunks of ~1000 to keep individual lock-holds < 10 ms.
4. **Test-harness reuse.** 9-7's integration tests can reuse `tests/opcua_dynamic_address_space_spike.rs` shape — `setup_dyn_test_server` (with the AC#5 manager-handle exposure), the `subscribe_one` helper, the `build_metric_variable` helper. The Story 9-0 test file will become a regression-pin test for 9-7's apply path (the test file's assertions stay valid; 9-7 adds the watch-channel layer on top).

---

## 10. Implications for Story 9-8 (Dynamic OPC UA Address Space Mutation)

1. **`apply_config_diff(old, new) -> AddressSpaceDiff` algorithm shape.** The diff function walks `old.application_list` × `new.application_list` and produces (a) `add_devices: Vec<DeviceFixture>`, (b) `delete_devices: Vec<DeviceId>`, (c) `add_metrics: Vec<(DeviceId, MetricFixture)>`, (d) `delete_metrics: Vec<(DeviceId, MetricName)>`. Issue #99 NodeId scheme + Story 8-3 access-level/historizing invariants apply to every (a)/(c).
2. **`delete` MUST be coordinated with subscription state.** Q2 surfaced the silent-stream-on-delete behaviour. Before calling `address_space.write().delete(node_id, true)`, 9-8 must emit `manager.set_attributes(subscriptions_cache, [(node_id, AttributeId::Value, Variant::StatusCode(BadNodeIdUnknown))])` so subscribed clients see an explicit transition. Then delete can run.
3. **Access-level + historizing=true invariant carry-forward (`epics.md:776`).** Every runtime-added variable MUST inherit `AccessLevel::CURRENT_READ \| AccessLevel::HISTORY_READ` + `historizing = true`. The spike test's `build_metric_variable` helper is the reference implementation.
4. **Stale read-callback closure leak (§ 8 above).** 9-8 must extend `OpcgwHistoryNodeManager` with a remove-callback method, OR document the leak as known-acceptable with operator monitoring. The wrap pattern for the extension is sketched in § 8.
5. **Issue #99 NodeId scheme.** Every runtime-added metric NodeId MUST use `format!("{device_id}/{metric_name}")` per `src/opc_ua.rs:846`. The spike's `metric_node_id` helper enforces this.

---

## 11. Plan B options

**Plan B not triggered.** All three questions resolved within Plan A semantics. The library-wrap-not-fork pattern (`epics.md:796`) holds.

This section would have enumerated three Plan B options (locka99/opcua fork, upstream contribution, deferred-mutation queue) if any of Q1/Q2/Q3 had failed. Since Q1 and Q3 resolved favourably and Q2's Behaviour B is a documentable UX gap (not a blocker), no Plan B is needed.

---

## 12. References

- `_bmad-output/planning-artifacts/epics.md:780-797` — Phase B carry-forward Story 9-0 spike decision (Epic 8 retro 2026-05-02 action item #5)
- `_bmad-output/planning-artifacts/epics.md:776` — access-level + historizing=true invariant
- `_bmad-output/planning-artifacts/epics.md:775` — Issue #99 NodeId scheme
- `_bmad-output/planning-artifacts/epics.md:796` — library-wrap-not-fork pattern
- `_bmad-output/implementation-artifacts/9-0-async-opcua-runtime-address-space-mutation-spike.md` — Story spec
- `_bmad-output/implementation-artifacts/8-1-async-opcua-subscription-spike.md` — Story 8-1 precedent
- `_bmad-output/implementation-artifacts/8-1-spike-report.md` — Story 8-1 spike report (size + structure precedent)
- `tests/opcua_dynamic_address_space_spike.rs` — the 3 integration tests pinning Q1/Q2/Q3
- `src/opc_ua.rs:155-280` — `OpcUa::build` (Shape B build phase) + `create_server` (returns `(Server, ServerHandle, Arc<OpcgwHistoryNodeManager>)`)
- `src/opc_ua.rs:680-770` — `OpcUa::run_handles` (Shape B run phase) + `OpcUa::run` (backward-compat wrapper)
- `src/opc_ua.rs:60-115` — `RunHandles` struct (Shape B handoff)
- `src/opc_ua.rs:801-957` — `OpcUa::add_nodes` (the canonical startup add path; spike's runtime add mirrors this shape)
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/node_manager/memory/mod.rs:108` — `InMemoryNodeManager::address_space()`
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/node_manager/memory/mod.rs:122,176` — `set_attributes` / `set_values` (notification-emission path; recommended for Q2 mitigation in § 5)
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/address_space/mod.rs:434,443,458` — `delete` / `add_folder` / `add_variables`
- `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/node_manager/memory/simple.rs:412` — `add_read_callback` (and the absent `remove_read_callback`)

**Test baseline post-spike:**
- Lib + bins: 322 + 345 = **667 passed / 0 failed / 5 ignored** (unchanged from Story 9-3 baseline — Shape B refactor is regression-free)
- Integration: 154 passed / 0 failed across 15 binaries (was 14 binaries; +1 = `opcua_dynamic_address_space_spike.rs` with 3 spike tests + 3 inherited common helpers)
- Doctests: **0 passed / 0 failed / 56 ignored** (issue #100 baseline unchanged)
- `cargo clippy --all-targets -- -D warnings`: clean
- `git grep -hoE 'event = "[a-z_]+"' src/web/ | sort -u | wc -l` = **4** (AC#7 web-scope regression pin holds)
- `git grep -hoE 'event = "[a-z_]+"' src/ | sort -u | wc -l` = **16** (broader-scope unchanged)

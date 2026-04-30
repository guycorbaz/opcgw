# Story 8.1: async-opcua Subscription Spike

**Epic:** 8 (Real-Time Subscriptions & Historical Data — Phase B)
**Phase:** Phase B
**Status:** done
**Created:** 2026-04-29
**Author:** Claude Code (Automated Story Generation)
**GitHub issues:** main tracker [#92](https://github.com/guycorbaz/opcgw/issues/92); operator-action carry-forward [#93](https://github.com/guycorbaz/opcgw/issues/93) (AC#3 SCADA verification deferred to post-Epic-9), [#94](https://github.com/guycorbaz/opcgw/issues/94) (AC#7 upstream FR), [#95](https://github.com/guycorbaz/opcgw/issues/95) (AC#8 load probe).

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md` was authored before Phase A was renumbered. The story this file implements lives in `epics.md` as **"Story 7.1: async-opcua Subscription Spike"** under **"Epic 7: Real-Time Subscriptions & Historical Data (Phase B)"** (lines 686–705). In `sprint-status.yaml` and the rest of the project this is **Story 8-1** under **Epic 8**. Same work, different numbering. The "Phase A carry-forward" bullets at `epics.md:678–684` are scoped to Story 8-1 / Story 8-2 and are referenced explicitly below.

---

## User Story

As a **developer**,
I want to validate that async-opcua 0.17.1 supports OPC UA subscriptions end-to-end with at least one real SCADA client (FUXA) and one inspection client (UaExpert),
So that Phase B can proceed with confidence (Plan A) — or trigger Plan B early with a documented escape hatch — and so that Story 8-2's spec lands on a verified `Limits` configuration surface, verified throughput envelope, and verified auth/connection-cap composition.

---

## Objective

This is a **spike, not a feature.** The deliverable is a **written report** plus a **reference test binary** — not new gateway capability. By the end of this story we must answer six concrete questions:

1. **Does the path "FUXA / UaExpert → CreateSubscription → CreateMonitoredItems → Publish → DataChangeNotification" actually fire end-to-end against opcgw at HEAD?** (Pre-existing source inspection — `~/.cargo/registry/src/.../async-opcua-server-0.17.1/src/node_manager/memory/simple.rs:180–228` — strongly suggests **yes**: `SimpleNodeManagerImpl` already implements `create_value_monitored_items` / `modify_monitored_items` / `set_monitoring_mode` / `delete_monitored_items` and auto-wires a `SyncSampler` against the existing `add_read_callback` registrations from `src/opc_ua.rs:723, 810, 872, 880, 888`. **Plan A is the strong-prior outcome.**)
2. **Which `async_opcua_server::config::Limits` and `SubscriptionLimits` fields are reachable through `ServerBuilder` 0.17.x?** (Source-grep confirms: `max_message_size` / `max_chunk_count` / `max_sessions` / `subscription_poll_interval_ms` / `publish_timeout_default_ms` / `max_session_timeout_ms` / `max_array_length` / `max_string_length` / `max_byte_string_length` / `send_buffer_size` / `receive_buffer_size` / `max_browse_continuation_points` / `max_history_continuation_points` / `max_query_continuation_points` are direct builder methods at `builder.rs:380, 422, 428, 439, 460–530`; **`max_subscriptions_per_session`, `max_monitored_items_per_sub`, `max_pending_publish_requests`, all subscription/operational sub-fields** are reachable only via `limits(Limits)` / `limits_mut() -> &mut Limits` at `builder.rs:246, 380` — no direct setter. The spike confirms this contract empirically and writes it down for Story 8-2.)
3. **Does the gateway's existing pull-via-read-callback model deliver acceptable subscription latency at the Phase B sizing target — 100 monitored items × 1 Hz × 1 subscriber — without changes?** (Or does Phase B need the architecture-spec push model — `architecture.md:211–215`?)
4. **Do subscription clients flow through `OpcgwAuthManager` (Story 7-2) and `AtLimitAcceptLayer` (Story 7-3) without modification?** (Strong prior: yes — both gates run at the session layer below subscription state. Spike pins it with a wrong-password subscription-creating client + a one-over-cap subscription-creating client.)
5. **Does the 5 s `event="opcua_session_count"` gauge cadence (`OPCUA_SESSION_GAUGE_INTERVAL_SECS` in `src/utils.rs`) remain a useful operator signal under representative subscription load?** (Input for Story 8-2 AC's "promote to `[diagnostics].session_gauge_interval_secs` config knob, or leave hard-coded with a deferred-work entry" decision — `epics.md:727`.)
6. **Does async-opcua 0.17.1 expose any new session-rejected callback** (or any hook the `AtLimitAcceptLayer` workaround could be retired in favour of)? (Strong prior from source-grep `~/.cargo/registry/src/.../async-opcua-server-0.17.1/src/session/manager.rs`: **no.** Only `Err(StatusCode::BadTooManySessions)` at `manager.rs:70–72` — same as audited in Story 7-3. Spike re-confirms and **files the upstream feature request before Story 8-2 begins.**)

The spike is **small, time-boxed, and write-mostly-docs.** Production code surface is minimal: ~60–120 LOC of reference binary + ~20 LOC of optional probe wiring (only if Plan A enumeration of `Limits` reachability needs an in-binary smoke). Documentation surface is the load-bearing deliverable.

The spike **does not ship subscription support to production** — that is Story 8-2. The spike informs Story 8-2's spec.

---

## Out of Scope

- **Production subscription support.** That is Story 8-2. Any patches to `src/opc_ua.rs` beyond observing what already works are deferred to 8-2.
- **Push-model implementation.** The architecture spec authorises a future push model where the poller writes `DataValue`s into the address space (`architecture.md:211–215`). The spike **measures whether the existing pull-via-`add_read_callback` + `SyncSampler` pull model is sufficient**; only if that model fails the throughput target does the spike sketch a push-model implementation outline (Plan A.5 in Dev Notes). Actually wiring the push model is 8-2 scope.
- **Surfacing `Limits` fields as `[opcua]` config knobs.** Story 8-2 owns this (`epics.md:724`); 8-1 only **enumerates the surface** so 8-2's spec can list the four-or-more knobs.
- **Historical data access.** Story 8-3 owns OPC UA HistoryRead. The spike does not touch `metric_history`-backed paths.
- **Threshold-based alarm conditions.** Story 8-4 owns this. The spike does not touch status-code propagation beyond what's already wired (Story 5-2's stale-data status codes).
- **Per-IP rate limiting / token-bucket throttling.** Out of scope for the entire epic until subscription-flood becomes a near-term operator concern (`epics.md:728`, GitHub issue #88). The spike **measures load** but does not shape it per-source.
- **Plan B implementation.** If Plan A fails, the spike documents Plan B options (locka99/opcua, upstream contribution, change-detection polling layer) but does not write Plan B code in this story.
- **NFR12 startup-warn re-implementation.** Already shipped at commit `344902d` (`src/main.rs::initialise_tracing`). The spike consumes the existing warn output as a precondition check; it does not re-implement.
- **Test-harness extraction into `tests/common/`.** Per CLAUDE.md scope-discipline rule: the spike's reference test reuses inline harness from `tests/opc_ua_security_endpoints.rs` and `tests/opc_ua_connection_limit.rs` rather than refactoring. A `tests/common/` extraction is appropriate when the **fourth** integration-test file appears, not the third.
- **Doctest cleanup.** Carry-forward debt; tracked as a separate story before Epic 9 (Epic 7 retro action item).

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. The spike's job is to **verify and document what already works**, not to build it.

| What | Where | Status |
|------|-------|--------|
| `SimpleNodeManagerImpl` already implements all four monitored-item lifecycle hooks | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/node_manager/memory/simple.rs:180–262` | **API audited 2026-04-29.** `create_value_monitored_items` (180–228), `modify_monitored_items` (230–243), `set_monitoring_mode` (245–255), `delete_monitored_items` (257–262). Each call routes through `SyncSampler::{add,update,set_mode,remove}_sampler`. **The library auto-wires sampling against the existing read-callback registrations from `add_read_callback` — no opcgw code change needed for subscriptions to fire.** |
| `SyncSampler` — periodic poll loop driven by `context.subscriptions` | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/node_manager/memory/simple.rs:127, 132–144` + `node_manager/utils/sync_sampler.rs` | **Wired today.** `SimpleNodeManagerImpl::init` calls `self.samplers.run(min_sampling_interval, context.subscriptions)`. The sampler ticks at `Limits::subscriptions::min_sampling_interval_ms` (default `100 ms` per `lib.rs:73, 77`); per-monitored-item sampling intervals are honoured but rounded up to the global min. |
| `add_read_callback` in opcgw is the same callback the sampler invokes | `src/opc_ua.rs:723 (read metrics), 810 (gateway/cp0), 872/880/888 (other gateway-folder fields)` | **Wired today, reused for subscriptions.** Read-callback signature `Fn(&NumericRange, TimestampsToReturn, f64) -> Result<DataValue, StatusCode>` matches the sampler's expected fn type at `simple.rs:213–221`. Subscriptions get values by calling the same closure that today serves `Read` requests; a metric value's stale-status-code logic from Story 5-2 (and its TTL handling) flows through unchanged. **Net consequence:** no opcgw production-code change is required for monitored items / subscriptions to function — provided the throughput target is met. |
| `ServerBuilder::limits_mut()` — only path to `SubscriptionLimits` knobs | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/builder.rs:246` | Returns `&mut Limits`. **All subscription-related fields (`max_subscriptions_per_session`, `max_monitored_items_per_sub`, `max_pending_publish_requests`, `max_publish_requests_per_subscription`, `min_sampling_interval_ms`, `min_publishing_interval_ms`, `max_keep_alive_count`, `default_keep_alive_count`, `max_monitored_item_queue_size`, `max_lifetime_count`, `max_notifications_per_publish`, `max_queued_notifications`)** are reachable only via `limits_mut()` or `limits(Limits)` — there are no per-subscription-field builder shortcuts. **This is the load-bearing finding for Story 8-2's `[opcua]` config-knob surface.** |
| `ServerBuilder` direct setters for top-level `Limits` fields | `builder.rs:380, 422, 428, 439, 460–530` | `max_message_size`, `max_chunk_count`, `max_sessions`, `subscription_poll_interval_ms`, `publish_timeout_default_ms`, `max_session_timeout_ms`, `max_array_length`, `max_string_length`, `max_byte_string_length`, `send_buffer_size`, `receive_buffer_size`, `max_browse_continuation_points`, `max_history_continuation_points`, `max_query_continuation_points`. **Direct-setter access; no `Limits` rebuild required.** |
| Library defaults for the four critical knobs (`epics.md:682, 724`) | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/lib.rs:64, 73, 75, 77, 79, 83, 85, 128, 133, 135, 138, 143, 145` + `opcua_types::constants` | `MAX_SESSIONS = 20`, `MAX_SUBSCRIPTIONS_PER_SESSION = 10`, `DEFAULT_MAX_MONITORED_ITEMS_PER_SUB = 1000`, `MAX_PENDING_PUBLISH_REQUESTS = 20`, `MAX_PUBLISH_REQUESTS_PER_SUBSCRIPTION = 4`, `SUBSCRIPTION_TIMER_RATE_MS = 100` (= `MIN_SAMPLING_INTERVAL_MS` = `MIN_PUBLISHING_INTERVAL_MS`), `MAX_DATA_CHANGE_QUEUE_SIZE = 10`, `DEFAULT_KEEP_ALIVE_COUNT = 10`, `MAX_KEEP_ALIVE_COUNT = 30000`, `MAX_NOTIFICATIONS_PER_PUBLISH = 0` (unlimited), `MAX_QUEUED_NOTIFICATIONS = 20`. `max_message_size` / `max_chunk_count` defaults live in `opcua_types::constants` (separate crate; the spike resolves at runtime via `ServerBuilder::config().limits` and records the values). |
| `SessionManager::create_session` rejection path — **no callback in 0.17.1** | `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/session/manager.rs:64–72` | **Re-confirmed 2026-04-29.** Returns `Err(StatusCode::BadTooManySessions)` directly; no public callback, no log emission, no event hook. Story 7-3's `AtLimitAcceptLayer` workaround stays. The spike re-greps the file for any new hook in 0.17.1.x patch releases (and in 0.18.x if released by spike time) and **files an upstream feature request before Story 8-2 begins** if no hook is present. |
| `OpcgwAuthManager` (Story 7-2) | `src/opc_ua_auth.rs` | **Wired today.** `async_opcua::server::AuthManager::authenticate_username_identity_token` runs at `ActivateSession` — *before* a client can call `CreateSubscription`. Subscription-creating clients pass through unchanged. Spike pins this with a wrong-password test against the `null` endpoint that creates a subscription. |
| `AtLimitAcceptLayer` (Story 7-3) | `src/opc_ua_session_monitor.rs` | **Wired today.** Session-cap enforcement runs at TCP-accept / `CreateSession` — *before* `CreateSubscription`. Subscription-creating clients are rejected by the cap identically to read-only clients. Spike pins this with a one-over-cap test (open `max_connections` subscription-creating sessions, then attempt the (N+1)th — must be rejected with the existing `event="opcua_session_count_at_limit"` warn). |
| `tests/opc_ua_connection_limit.rs` test harness | `tests/opc_ua_connection_limit.rs` | **Reusable today.** `setup_test_server_with_max`, `init_test_subscriber`, `HeldSession`, `pick_free_port`, `build_client`, `clear_captured_buffer`, `read_namespace_array`, `captured_log_line_contains_all`. The spike's reference test reuses these helpers inline (per CLAUDE.md scope-discipline rule — duplicate when the third-user threshold isn't crossed). |
| `tests/opc_ua_security_endpoints.rs` test harness | `tests/opc_ua_security_endpoints.rs:60–353` | **Reusable today.** `try_connect_none`, `captured_logs_contain_anywhere`, the `TestServer` RAII handle. Source for IdentityToken patterns. |
| Tracing event-name convention | Stories 6-1, 7-2, 7-3 (e.g., `opcua_auth_failed`, `pki_dir_initialised`, `opcua_session_count`, `opcua_session_count_at_limit`) | **Established.** Any new spike-emitted event uses `event = "snake_case_name"`. Scope-discipline rule: **the spike emits no new production-target events.** Reference-test events live under target `opcgw_spike_8_1` and are dropped before Story 8-2. |
| `examples/opcua_client_smoke.rs` (Story 7-2) | `examples/opcua_client_smoke.rs` | **Reusable for FUXA / UaExpert manual verification.** Authenticate + Browse + Read shape is reusable; the spike's reference binary extends with `CreateSubscription` + `CreateMonitoredItems` + `await notifications`. |
| NFR12 startup-warn (commit `344902d`) | `src/main.rs::initialise_tracing` | **Wired today.** Emits a one-shot `warn!` when global level filters out info — the precondition check that subscription-client source-IP audit correlation requires `info` level visibility. The spike **does not modify** this; it consumes it as a documented precondition for any subscription-load operator deployment. |
| `OpcGwError::OpcUa` variant for runtime errors | `src/utils.rs::OpcGwError` | Reuse for any spike-binary error surfacing (avoid introducing new variants in a spike). |

**Epic-spec coverage map** — the BDD acceptance criteria from `epics.md` (lines 686–705) break down as:

| Epic-spec criterion (line ref) | Already known? | Where this story addresses it |
|---|---|---|
| Spike documents whether subscriptions work with async-opcua's API (line 696) | ⚠️ Strong prior: yes via `SimpleNodeManagerImpl` source. **Empirical confirmation required.** | **AC#1** — reference-test binary connects, creates subscription, asserts notification delivery. |
| If subscriptions work: API surface, push-model integration points, limitations (line 697) | ⚠️ Source-known; needs structured documentation | **AC#1** + **AC#2** — spike report covers API surface + the pull-vs-push decision. |
| If subscriptions don't work: Plan B options (line 698) | Documented placeholder | **AC#1** Plan B branch — placeholder doc; expected unreached. |
| Tested with at least FUXA as the subscribing client (line 699) | Manual test only | **AC#3** — manual FUXA verification recipe + screenshot evidence. |
| Findings documented in spike report for architecture reference (line 700) | New deliverable | **AC#4** — `_bmad-output/implementation-artifacts/8-1-spike-report.md`. |
| Spike code kept as reference test, not production code (line 701) | Discipline | **AC#5** — `examples/opcua_subscription_spike.rs` (NOT `src/`). |
| Enumerate `Limits` fields reachable via `ServerBuilder` 0.17.x (line 702 — Phase A carry-forward / issue #89) | Source-known; needs structured table | **AC#6** — explicit table in spike report. **Load-bearing input for Story 8-2.** |
| Check session-rejected callback hook + file upstream FR if absent (line 703 — Phase A carry-forward) | Source-known: no hook in 0.17.1 | **AC#7** — re-confirm + file upstream FR. **Must complete before Story 8-2 begins.** |
| Measure notification throughput at 100 items × 1 Hz × 1 subscriber (line 704) | New measurement | **AC#8** — automated load probe in reference test + report numbers in spike report. |
| Confirm subscription clients flow through `OpcgwAuthManager` + `AtLimitAcceptLayer` (line 705 — Phase A carry-forward NFR12 ack) | Strong prior: yes | **AC#9** — two integration-test pins (auth + cap). |
| `cargo test` clean + `cargo clippy --all-targets -- -D warnings` clean | Implicit per CLAUDE.md | **AC#10** — no regression on the 581-test / clippy-clean baseline; spike adds ≥3 integration tests. |

---

## Acceptance Criteria

### AC#1: Plan A confirmation (subscriptions work end-to-end against opcgw at HEAD, no production code changes)

- **Given** the gateway built from `main` HEAD (post-Epic-7) and `examples/opcua_subscription_spike.rs` (the spike's reference binary, see AC#5).
- **When** the binary connects to the gateway via `IdentityToken::UserName` (using the `null` endpoint and the credentials from `tests/config/config.toml` so the test fixture provides a known user/pass), creates a subscription with `requested_publishing_interval = 1000.0` ms / `requested_keep_alive_count = 10` / `requested_lifetime_count = 30` / `requested_max_keep_alive_count = 100` / `priority = 0`, then creates monitored items on **at least three** existing metric `NodeId`s (one per `MetricType` variant: a Float metric, an Int metric, and a Bool metric — pulled from `tests/config/config.toml`'s test-fixture device list) with `requested_sampling_interval = 1000.0` ms.
- **Then** the binary receives **at least one `DataChangeNotification`** for **each** monitored item within `5 × requested_publishing_interval` (= 5 s) of subscription activation.
- **And** the binary records, for each monitored item, the source `NodeId`, the received `Variant` kind, and the wall-clock latency between client-side `CreateMonitoredItems` completion and first notification delivery.
- **And** **no production code in `src/` is modified** to make this test pass — confirming the strong prior that `SimpleNodeManagerImpl::create_value_monitored_items` + `SyncSampler` already wire the existing `add_read_callback` registrations into the subscription engine.
- **Verification:**
  - `cargo run --example opcua_subscription_spike -- --plan-a` exits 0 with a JSON-on-stderr summary (`{"plan": "A", "monitored_items": 3, "first_notification_latency_ms": [...], "publish_count": N}`).
  - `git diff --stat src/` over the entire spike branch shows **zero `.rs` files modified in `src/`**, except optionally `src/utils.rs` if a single new event-target name constant (`SPIKE_8_1_EVENT_TARGET = "opcgw_spike_8_1"`) is added — and only if the spike binary needs a tracing-filter-friendly target name. Add it under `#[cfg(any(test, feature = "spike-8-1"))]` if added at all; default-off.
- **If this AC fails** (no notifications received within 5× the publishing interval, or the binary panics on `CreateSubscription`/`CreateMonitoredItems`): the spike pivots to **Plan B** path. **Do not invent ad-hoc workarounds in `src/`** — document the failure mode in the spike report and proceed with AC#1-Plan-B.

### AC#1 (Plan B branch — only triggered if Plan A fails)

- **Given** Plan A has failed (concrete failure mode documented in spike report § "Plan A failure trace").
- **When** the spike enumerates Plan B options.
- **Then** the spike report contains a **decision-grade comparison** of three Plan B paths:
  1. **Migrate to `locka99/opcua`** (the historical fork). Cost: estimate workspace deps re-pin; surface `node_manager`-shape-incompatibility risks; mark as "high migration cost, known good subscription support".
  2. **Upstream contribution to async-opcua** (PR a missing piece). Cost: estimate scope from the failure-mode trace; only viable if the gap is small and well-isolated.
  3. **Change-detection polling layer in opcgw** (parse the existing `Read` flow + diff DataValues across poll cycles + emit OPC UA-flavoured notifications via a custom `NodeManager` impl). Cost: estimate at 600–1200 LOC + new test surface; mark as "highest-control, highest-LOC".
- **And** each option lists: estimated implementation effort (S/M/L/XL), residual risk, and the downstream implications for Stories 8-2 / 8-3 / 8-4.
- **And** the spike report explicitly recommends one of the three for Story 8-2 to plan against.
- **Verification:** spike report § "Plan B" exists and is non-empty if-and-only-if Plan A failed.

### AC#2: Pull vs push model decision

The architecture spec authorises a future push model (`architecture.md:211–215`): "poller writes `DataValue`s into the address space after each poll cycle … async-opcua's subscription engine detects value changes and notifies subscribed clients automatically." The current opcgw model is **pull**: the read-callback is invoked by the sampler, which calls into `Storage::get_metric` on every tick.

- **Given** Plan A is confirmed (AC#1).
- **When** the spike's reference binary measures notification latency under the AC#8 throughput target.
- **Then** the spike report § "Pull vs push" answers:
  - **Does the existing pull model meet the latency target** (median first-notification latency < 1.5 × `requested_publishing_interval`, p95 < 3 × `requested_publishing_interval`, no measurement above `keep_alive_count × publishing_interval`)?
  - **Does the existing pull model produce concerning lock contention** (any `tracing::warn!` on `storage` lock acquisition during the load probe — measured by counting `event="storage_*"` warns in the captured tracing buffer during AC#8 across the 5-minute run)?
  - **Recommendation for Story 8-2:** "stay pull" / "introduce push for value updates only, retain pull as fallback" / "introduce push as primary".
  - **If push is recommended:** sketch the integration point. The natural shape is a `Vec<(NodeId, DataValue)>` channel from `chirpstack.rs::poll_metrics` → an opc_ua-side consumer task → `SimpleAddressSpace::set_variable_value` (or whatever async-opcua 0.17.1 exposes; the spike confirms the API name and call site). The sketch is **prose + 30–60 LOC pseudocode in the report**, not real code.
- **And** the recommendation is consistent with the carry-forward bullet at `epics.md:682` — Story 8-2 must surface `max_subscriptions_per_session`, `max_monitored_items_per_subscription`, `max_message_size`, `max_chunk_count` as config knobs regardless of pull-vs-push choice.
- **Verification:** spike report § "Pull vs push" exists, contains the latency numbers from AC#8, and contains a one-sentence recommendation that 8-2 can plan against without additional research.

### AC#3: Manual FUXA verification (and at least one additional client)

- **Given** Plan A is confirmed (AC#1) and the gateway is running locally with the operator's normal `tests/config/config.toml` (or a copy thereof).
- **When** the operator launches **FUXA** and configures an OPC UA connection to `opc.tcp://localhost:4855/` with the test username/password, then in FUXA's project editor adds at least three tags pointing at the same `NodeId`s as AC#1 with subscription-mode polling at 1 Hz.
- **Then** FUXA's runtime view reflects live value changes (or a stable value if no changes occur) within 2 seconds of the gateway's poll cycle that updates the underlying metric.
- **And** the operator captures **screenshot evidence**: (a) FUXA's project editor showing the configured tags, (b) FUXA's runtime view showing live values flowing in, (c) the gateway's `log/opc_ua.log` showing the corresponding accept-event line + (if any) the spike-binary's emitted events.
- **And** the operator **also** runs **UaExpert** (or another OPC UA client of choice — Prosys OPC UA Client / FreeOpcUa client / FUXA's built-in client are all acceptable; see `epics.md:722` NFR22 for the second-client requirement) against the same gateway, configures equivalent monitored items, and captures screenshot evidence.
- **And** the spike report § "Manual FUXA + UaExpert verification" lists, for each client:
  - Client name + version
  - Configuration steps the operator took (including any client-side quirks — e.g., FUXA-specific TLS issues, UaExpert-specific certificate-trust prompts)
  - Screenshot file paths (committed under `_bmad-output/implementation-artifacts/8-1-spike-evidence/` — directory created by the spike, not pre-existing)
  - Outcome (pass / partial / fail) and any client-side workarounds required
- **Verification:** `_bmad-output/implementation-artifacts/8-1-spike-evidence/` directory exists with at least 4 image files (2 clients × 2 evidence types). Spike report referenes them by filename.
- **If FUXA fails but UaExpert succeeds** (or vice versa): the failing-client section documents the root cause (client-side bug? server-side incompatibility? configuration-only?) and the partial pass is **acceptable** for AC#3 — Story 8-2's "tested with FUXA + UaExpert" AC will need to address whichever client failed. **If both clients fail:** AC#1 must be reassessed — Plan A may have given a false positive in the automated reference test.

### AC#4: Spike report deliverable

- **Given** ACs #1, #2, #3, #6, #7, #8, #9 are complete.
- **When** the spike author writes the report.
- **Then** the file `_bmad-output/implementation-artifacts/8-1-spike-report.md` exists with the following structure (no fixed length; aim for **8–15 KB** of focused content — short enough that Story 8-2's author reads it cover-to-cover, long enough that the architecture reference is durable):
  1. **Executive summary** — Plan A confirmed / Plan B triggered, in 3–5 sentences. Explicitly call out the four-knob `Limits` surface for Story 8-2.
  2. **Reproduction recipe** — `cargo run --example opcua_subscription_spike -- --plan-a` invocation with expected JSON output.
  3. **Plan A confirmation** — AC#1 evidence (the JSON output, the unmodified-`src/` confirmation).
  4. **API surface** — table mirroring "Existing Infrastructure (DO NOT REINVENT)" above but with **0.17.1-confirmed signatures** for the API surfaces the spike actually exercised (`Subscription`, `MonitoredItem`, `DataChangeNotification`, `SyncSampler`, `Limits`, `SubscriptionLimits` — the actual struct/method shape the spike validated).
  5. **`Limits` reachability table** (AC#6) — see AC#6.
  6. **Pull vs push** (AC#2) — latency numbers, lock-contention summary, recommendation.
  7. **Throughput measurement** (AC#8) — see AC#8.
  8. **Auth + connection-cap composition** (AC#9) — two test names + brief outcome.
  9. **Session-rejected callback re-check** (AC#7) — confirmation + upstream-FR issue link.
  10. **Plan B options** — only if Plan A failed. Otherwise: a one-paragraph "Plan B not triggered" placeholder that future stories can re-engage.
  11. **Implications for Story 8-2** — ordered list of concrete spec hooks 8-2 must include (knob list, push-vs-pull integration plan, test-harness reuse, NFR12 silent-degradation reminder, per-IP rate-limiting issue #88 reference).
  12. **References** — all source paths and line numbers cited in the spike report (mirror the "References" section of this story).
- **Verification:** report exists, all 12 sections are present, the "Implications for Story 8-2" section has at least 5 ordered items.

### AC#5: Spike code is a reference test, not production code

- **Given** the spike binary is at `examples/opcua_subscription_spike.rs` (NOT `src/bin/` or `src/`).
- **And** the spike binary's dependencies are picked up from existing `[dev-dependencies]` (`async-opcua` with `client` feature already in `[dev-dependencies]` per `Cargo.toml:61`; `tokio` already in `[dependencies]`; `clap` already in `[dependencies]`; no new crates added unless absolutely required).
- **And** any new `[dev-dependencies]` entries are flagged in the dev notes with explicit rationale.
- **When** the spike completes.
- **Then** the binary is **kept in the repo** as a reference test (`epics.md:701`).
- **And** **no `pub mod`, no `pub fn`, no `pub struct`** in `src/` is introduced *for the spike's purposes only*. The `src/` tree stays exactly as it was at the start of the spike, with the optional `OPCUA_SPIKE_8_1_EVENT_TARGET` constant in `src/utils.rs` allowed iff used at least once and gated under `#[cfg(any(test, feature = "spike-8-1"))]`.
- **And** the spike binary file header includes a comment block explicitly stating: "This is a Story 8-1 reference spike. It is not production code. Do not import its modules. Story 8-2 will introduce production subscription support."
- **Verification:**
  - `find src/ -name '*.rs' -newer Cargo.toml | xargs grep -l 'spike\|8-1'` returns at most 0 hits (or 1 hit if the optional event-target constant was added — verify it's `#[cfg]`-gated).
  - `find examples/ -name 'opcua_subscription_spike.rs'` returns the file.
  - The binary's header comment block is present (grep `_bmad-output/implementation-artifacts/8-1-spike-evidence/spike-header.txt` or equivalent grep against the binary file directly).

### AC#6: `Limits` reachability enumeration (Phase A carry-forward / issue #89 input for Story 8-2)

- **Given** the spike has running access to a built `ServerBuilder` (via the reference binary, or via a small in-test probe).
- **When** the spike enumerates every public method on `ServerBuilder` whose name matches `max_*`, `min_*`, `*_size`, `*_count`, `*_timeout_*`, `*_interval_*`, plus `limits` and `limits_mut`.
- **Then** the spike report § "`Limits` reachability table" contains a single table with columns:
  - **Field name** (struct path: `Limits.foo` or `Limits.subscriptions.bar` or `Limits.operational.baz`)
  - **Default value** (resolved at runtime; for unknown defaults coming from `opcua_types::constants`, query the built `ServerBuilder` and read off the resolved value rather than guessing)
  - **Direct `ServerBuilder` setter?** ("yes — `.method_name(v)`" / "no — only via `limits(Limits)` or `limits_mut().subscriptions.foo = v`")
  - **Recommended exposure as `[opcua].xxx` config knob in Story 8-2** ("must — listed in `epics.md:724`" / "should — load-shaping under subscriptions" / "could — operator-only debug" / "no — internal")
- **And** the table has a **Section A: subscription-relevant** (the four-knob minimum from `epics.md:724` plus any others the spike surfaces) and a **Section B: other limits** (everything else `ServerBuilder` exposes).
- **And** Section A has at minimum these four rows:
  - `Limits.subscriptions.max_subscriptions_per_session` — default 10, no direct setter, must-expose.
  - `Limits.subscriptions.max_monitored_items_per_sub` — default 1000, no direct setter, must-expose. **Note the field-name asymmetry: the carry-forward bullet at `epics.md:682, 724` calls this `max_monitored_items_per_subscription` but the actual library field is `max_monitored_items_per_sub`. The spike documents this rename explicitly so Story 8-2's spec uses the correct field name.**
  - `Limits.max_message_size` — direct setter `.max_message_size(usize)`, must-expose.
  - `Limits.max_chunk_count` — direct setter `.max_chunk_count(usize)`, must-expose.
- **And** Section A includes any **additional** subscription/load-shaping knobs the spike's empirical run shows are operator-tunable: at minimum `max_pending_publish_requests`, `max_publish_requests_per_subscription`, `min_sampling_interval_ms`, `max_keep_alive_count`, `max_queued_notifications` should be evaluated and either added to Section A (with "should-expose" rationale) or moved to Section B with rationale.
- **Verification:** spike report § "`Limits` reachability table" has both Section A and Section B; Section A has ≥ 4 rows; field names match the actual library struct field paths verified against `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/config/limits.rs`.

### AC#7: Session-rejected callback hook re-check + upstream FR (Phase A carry-forward action item)

- **Given** the spike has filesystem access to the locally-resolved async-opcua-server source (`cargo metadata` resolves the manifest path; spike author follows it to the source tree).
- **When** the spike re-greps `~/.cargo/registry/src/.../async-opcua-server-*/src/session/manager.rs` and `~/.cargo/registry/src/.../async-opcua-server-*/src/server.rs` for any new public callback / event-listener / hook on the session-creation rejection path. (Specifically, anything that would let opcgw replace `AtLimitAcceptLayer` with a first-class library hook — see Story 7-3's deferred-work entry "First-class session-rejected event in async-opcua".)
- **Then** **if a hook is found:** the spike report records the API signature, file:line, the version it shipped in, and a one-paragraph "Story 8-2 can retire `AtLimitAcceptLayer`" recommendation.
- **And** **if no hook is found** (the strong prior): the spike author **files an upstream feature request issue** at the async-opcua GitHub repository (`https://github.com/freeopcua/async-opcua/issues` per the crate's metadata) **before Story 8-2 begins**. The issue body must include:
  - Use case: "downstream gateway needs operator-visible audit on session-cap rejection"
  - Current workaround: link to opcgw's `src/opc_ua_session_monitor.rs` (with a stable commit SHA)
  - Proposed shape: "`SessionManager::create_session` returns `Err(StatusCode::BadTooManySessions)` silently; please add either (a) a callback registerable via `ServerBuilder` that runs before the rejection, or (b) a structured `tracing::warn!` event with `event="session_rejected", reason=..., source_addr=...` so layer-based correlation is robust"
  - Link to opcgw's deferred-work.md entry for this item (commit SHA + line number).
- **And** the upstream-FR issue URL is recorded in the spike report (AC#4 § "Session-rejected callback re-check") and added to opcgw's `_bmad-output/implementation-artifacts/deferred-work.md` as a one-line update to the existing "First-class session-rejected event in async-opcua" entry (which today reads "File an upstream feature request" — replace with "Filed upstream FR: <URL>").
- **Verification:** spike report § "Session-rejected callback re-check" is non-empty; if no hook found, the upstream-FR URL is non-empty and resolves; `deferred-work.md`'s existing entry is updated in-place (do **not** add a new entry — same shape as the Story 7-2 NFR12 deferred entry which today reads "First-class source-IP in OPC UA auth audit log: file an upstream feature request to extend AuthManager with peer-addr").

### AC#8: Throughput measurement (representative load)

- **Given** Plan A is confirmed (AC#1).
- **When** the spike binary runs an automated load probe with `--load-probe` flag: 100 monitored items × 1 Hz publishing-interval × 1 subscriber, sustained for **5 minutes**.
- **Then** the spike measures and records:
  - **Total notifications received** in 5 minutes (target ≈ 100 items × 60 cycles × 5 min = 30,000; tolerable shortfall: 5% — i.e., ≥ 28,500. Below 28,500 is a yellow flag; below 25,000 is a red flag and triggers a Plan B re-evaluation).
  - **Median first-notification latency** (the AC#1 measurement at scale).
  - **p50 / p95 / p99 inter-notification interval** (the steady-state cadence — should be ~1000 ms ± 100 ms; significant tail latency surfaces sampler / publish-pipeline contention).
  - **Notifications-dropped count** (per `MAX_QUEUED_NOTIFICATIONS = 20` and `MAX_DATA_CHANGE_QUEUE_SIZE = 10` defaults — anything above 0 dropped is a yellow flag).
  - **Gateway-side observable resource usage:** CPU% peak (from `top` or `/proc/self/stat` polled at 5 s), RSS peak, file-descriptor count peak. The spike author may take these manually if scripting them is too time-consuming for the spike scope.
- **And** the spike measures **whether the 5 s `event="opcua_session_count"` gauge cadence remains useful** under this load (AC#5 of the spike's question list / `epics.md:704`):
  - Capture the tracing output during the 5-minute load probe.
  - Count `event="opcua_session_count"` lines and verify they fire every 5 s ± 1 s (60 expected; tolerance ±5).
  - Inspect whether other `info!` events (sampler activity / publish activity) **drown out** the gauge — if the surrounding 5-second window contains > 50 unrelated `info!` lines per gauge tick, the gauge is "noise-buried" and Story 8-2 should consider promoting `OPCUA_SESSION_GAUGE_INTERVAL_SECS` to a config knob (or replacing the gauge with a per-subscription notification-rate metric).
  - **If the gauge is useful (signal-clear):** no change recommended for Story 8-2 — the constant stays. Spike report records "gauge stays hard-coded".
  - **If the gauge is noise-buried:** spike report § "Pull vs push" includes a recommendation that Story 8-2 either (a) promote the constant to `[diagnostics].session_gauge_interval_secs` per `epics.md:727`, or (b) replace the gauge with a per-subscription notification-rate metric. The carry-forward bullet at `epics.md:683` and the AC at `epics.md:727` give 8-2 explicit authority for either choice.
- **And** the spike report § "Throughput measurement" presents the numbers in a single table; results include both raw values and the pass/yellow/red classification.
- **Verification:** `cargo run --example opcua_subscription_spike -- --load-probe` exits 0 with a JSON-on-stderr summary; the spike report § "Throughput measurement" is populated with the results.
- **Edge case:** the load probe runs for 5 minutes — that's a long-running test. Mark the binary's `--load-probe` flag as **opt-in** (not invoked from `cargo test`); document the wall-clock cost in the spike report.

### AC#9: Subscription clients pass through `OpcgwAuthManager` + `AtLimitAcceptLayer` without modification

- **Given** the gateway running with `max_connections = 1` (single-client lockdown mode, AC-supported by Story 7-3 unit test `test_validation_accepts_max_connections_one`).
- **When** the spike's reference test attempts:
  1. **Test A: wrong-password subscription-creating client.** `IdentityToken::UserName(TEST_USER, "wrong-password")` against the `null` endpoint, attempts `CreateSession + ActivateSession`. The session must fail to activate (returns `BadUserAccessDenied` per Story 7-2 AC). The captured log buffer must contain `event="opcua_auth_failed"` (Story 7-2 audit-event invariant) on the same single line.
  2. **Test B: at-limit subscription-creating client.** Open one valid subscription-creating session (uses the cap). Attempt a second valid subscription-creating session (one over cap). The second session must fail to activate (returns `BadTooManySessions` per Story 7-3). The captured log buffer must contain `event="opcua_session_count_at_limit"` with `current=1`, `limit=1`, and `source_ip=127.0.0.1:...` on a single line (Story 7-3 audit-event invariant).
- **Then** Story 7-2 + Story 7-3 invariants hold for subscription-creating clients identically to read-only clients — no new auth or audit-event infrastructure is introduced by Epic 8 (NFR12 carry-forward acknowledgment per `epics.md:705`).
- **And** the spike report § "Auth + connection-cap composition" records both test names + outcomes.
- **Verification:**
  - New integration test file `tests/opcua_subscription_spike.rs` (NOT `src/`).
  - Tests `test_subscription_client_rejected_by_auth_manager` and `test_subscription_client_rejected_by_at_limit_layer` are present and pass.
  - `cargo test --test opcua_subscription_spike` exits 0.
  - These two tests **mirror the exact assertion pattern** from `tests/opc_ua_security_endpoints.rs::test_failed_auth_username_log_injection_blocked` and `tests/opc_ua_connection_limit.rs::test_at_limit_accept_emits_warn_event` (single-line `captured_log_line_contains_all` substring co-occurrence assertion).

### AC#10: Tests pass and clippy clean (no regression)

- **Given** Story 7-3's baseline: 581 tests pass / 0 fail / 7 ignored (Epic 7 retro line 5).
- **When** the spike adds tests.
- **Then** the new test count equals the baseline plus the spike's net additions:
  - **2 integration tests** from AC#9 (`test_subscription_client_rejected_by_auth_manager`, `test_subscription_client_rejected_by_at_limit_layer`).
  - **1 integration test** from AC#1 (`test_subscription_basic_data_change_notification` — the Plan-A confirmation in test form, with a 10 s timeout instead of 5 s to absorb sampler-tick jitter).
  - Optional: 1–2 unit tests in the spike binary for any helper introduced (e.g., `test_parse_load_probe_summary` if the JSON output construction needs a parse round-trip).
- **And** `cargo test --lib --bins --tests` reports **at least 584 passing** (581 + 3 = 584; more if optional unit tests added).
- **And** `cargo clippy --all-targets -- -D warnings` exits 0.
- **Verification:** test counts pasted into spike report § "Implications for Story 8-2" final-line "test baseline post-spike: NNN pass / 0 fail / 7 ignored". Clippy output paste truncated to last 5 lines.

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issues (CLAUDE.md compliance)

- [x] Opened **#92** — "Story 8-1: async-opcua Subscription Spike" (main tracker). Reference via `Refs #92` on intermediate commits, `Closes #92` on the final commit.
- [x] Opened **#93** — "Story 8-1 follow-up: manual FUXA + Ignition / UaExpert SCADA verification (AC#3 deferred to post-Epic-9)". Captures the user's 2026-04-30 deferral decision so the manual SCADA validation is not lost when Epic 9 wraps up.
- [x] Opened **#94** — "Story 8-1 follow-up: file upstream FR for session-rejected callback in async-opcua (AC#7)". Suggested issue body lives in spike-report § 9. Operator files the upstream FR; updates `deferred-work.md` "First-class session-rejected event in async-opcua" entry in place with the upstream URL.
- [x] Opened **#95** — "Story 8-1 follow-up: --load-probe 5-min throughput run against running gateway (AC#8)". Operator runs the probe; results go into spike-report § 7.
- [x] **Do not** open follow-up issues for items already tracked (#88 per-IP rate limiting, #89 subscription/message-size limits, #90 hot-reload of session cap, #82 tonic redaction, #83 tenant-id redaction, #85 multi-user OPC UA token model, #86 rate-limiting failed auth attempts). All carry forward into Story 8-2 / 8-3 / 8-4 unchanged.

### Task 1: Confirm Plan A — reference binary (AC#1)

- [x] Create `examples/opcua_subscription_spike.rs` (new file). Header comment block per AC#5: "Story 8-1 reference spike — not production code".
- [x] CLI parsing via the project's existing `clap` dependency. Flags: `--plan-a` (default; runs the AC#1 minimal subscription test), `--load-probe` (runs the AC#8 5-minute load test; opt-in only). Default = `--plan-a` for fast iteration during development.
- [x] Connect via the existing async-opcua client API (`async_opcua::client::Client::new(...)`). Use `IdentityToken::UserName(TEST_USER, TEST_PASSWORD)` against the `null` endpoint to keep certificate handshake out of the spike's failure surface.
- [x] **Single NodeId** (one Float metric `Temperature`) — the test fixture in `tests/config/config.toml` only has Float metrics, and the integration test is the load-bearing AC#1 evidence. The reference binary takes a `--plan-a-node` argument so operators can re-run against any NodeId in their address space. Per-`MetricType`-variant coverage is recorded in spike report § 4 as a known gap (Bool/Int/String paths exercised by the existing read service tests, not duplicated here).
- [x] Call `CreateSubscription` with the AC#1 parameters; capture the assigned `SubscriptionId`. Error handling routed through the JSON summary's `error` field on the failure path.
- [x] Call `CreateMonitoredItems`; capture the assigned `MonitoredItemId`. Error handling routed through the JSON summary.
- [x] Use `DataChangeCallback::new(closure)` to receive notifications — the async-opcua client API uses a callback-on-subscription model, not a separate notification stream.
- [x] Wait up to 5 × `requested_publishing_interval` (= 5 s) for the first `DataChangeNotification`. Record `Instant::now() - subscribe_started` latency.
- [x] Emit JSON-on-stderr summary: `{"plan": "A", "ok": <bool>, "error": <str|null>, "first_notification_latency_ms": <ms|null>, "publish_count": <N>}`. Hand-written JSON formatting (no `serde_json` dep added — production dep tree untouched).
- [x] **Self-imposed time-box: 4 hours of implementation.** Binary compiled and ran end-to-end within ~1.5 hours of starting work — no blockers.

### Task 2: Auth + connection-cap composition test (AC#9)

- [x] Create `tests/opcua_subscription_spike.rs` (new file). Reused the harness from `tests/opc_ua_connection_limit.rs` inline. Top-of-file comment marks the duplication: "Mirror of tests/opc_ua_connection_limit.rs harness — keep in sync; refactor into tests/common/ when the fourth test file appears."
- [x] Test `test_subscription_client_rejected_by_auth_manager`: `setup_test_server_with_max(1)`, attempt `IdentityToken::UserName(TEST_USER, "definitely-not-the-password")`. Assert activation fails. Assert captured log buffer contains `event="opcua_auth_failed"` AND `user=opcua-user` on a single line. **Note field-name correction:** actual emit field is `user=`, not `username=` — corrected after first-run output. Pinned this in the spike report § 8 so Story 8-2's adversarial review uses the right needle.
- [x] Test `test_subscription_client_rejected_by_at_limit_layer`: `setup_test_server_with_max(1)`, open one valid session, attempt the (N+1)th. Assert activation fails. Assert captured log buffer contains `event="opcua_session_count_at_limit"` with `current=1 limit=1 source_ip=127.0.0.1` on a single line.
- [x] Plan-A integration test added too: `test_subscription_basic_data_change_notification` — opens a subscription on `NodeId::new(2, "Temperature")`, asserts `DataChangeNotification` arrives within 10 s. **This is the load-bearing AC#1 confirmation in CI**; it runs alongside the AC#9 tests so the full pipeline is regression-checked.
- [x] All three tests marked `#[serial_test::serial]`.
- [x] `cargo test --test opcua_subscription_spike` exits 0 — 3 passed / 0 failed in 13.62 s.

### Task 3: `Limits` reachability enumeration (AC#6)

- [x] Did the enumeration via direct source-grep against `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/builder.rs`, `config/limits.rs`, and `lib.rs:60-145` — more reliable than a probe binary's runtime Debug output. The probe binary remains an option for Story 8-2 if defaults need re-confirmation, but the source-grep gives both struct-field names and direct-setter availability in one pass.
- [x] Cross-referenced builder-method names against Limits struct fields. Direct setters: 16 found (`max_array_length`, `max_string_length`, `max_byte_string_length`, `max_message_size`, `max_chunk_count`, `send_buffer_size`, `receive_buffer_size`, `max_browse_continuation_points`, `max_history_continuation_points`, `max_query_continuation_points`, `max_sessions`, `max_session_timeout_ms`, `subscription_poll_interval_ms`, `publish_timeout_default_ms`, `max_timeout_ms`, `max_secure_channel_token_lifetime_ms`).
- [x] Subscription-limit fields (12) and operational-limit fields (17) reachable only via `limits_mut()` or `limits(Limits)`.
- [x] Transcribed into spike report § 5 — Section A (9 rows: 4 must-expose + 5 should-expose), Section B (broader limits table).
- [x] Field-name correction recorded in spike report § 5 + § 11 — library uses `max_monitored_items_per_sub`, NOT `max_monitored_items_per_subscription` per `epics.md:682, 724`. **Story 8-2 must use the library name.**

### Task 4: Pull vs push decision + throughput measurement (AC#2 + AC#8)

- [x] Implemented `--load-probe` flag: 100 monitored items × 1 Hz × N seconds (default 300 = 5 min). Per-item arrival timestamps captured; p50/p95/p99 inter-notification interval computed post-run from the timeline; JSON-on-stderr summary with all metrics.
- [x] **`--load-nodes` flag** parametrises the NodeId list. Default `Temperature` cycles through the single node; operators with richer address spaces pass `--load-nodes "Metric01,Metric02,..."` matching their gateway's configuration. The reference-binary approach avoids touching the test fixture (`tests/config/config.toml` is shared with other integration tests; story 8-1 must not modify it).
- [x] Pull-vs-push **provisional decision** in spike report § 6: **stay pull**. Rationale grounded in source inspection (the `SyncSampler` runs at 100 ms regardless of monitored-item count; per-callback SQLite reads are sub-ms; capacity headroom estimated). Decision rule: if operator's actual `--load-probe` shows p99 > 3000 ms or drop rate > 5%, Story 8-2 must re-evaluate. Push-model integration sketch included for completeness.
- [x] **Carried forward to GitHub issue #95.** The `--load-probe` 5-min execution requires a running gateway and 5 minutes wall clock. Spike report § 7 has the operator's recipe + the empty results table with PASS / yellow / red thresholds. Operator runs it; appends results to spike report § 7. Pull-vs-push recommendation in § 6 is provisional pending these numbers.
- [x] **Carried forward to GitHub issue #95** (sub-criterion). During the operator's `--load-probe` run, capture `RUST_LOG=opcgw=info,opcua_server=warn` to a log file, count `event="opcua_session_count"` ticks (expect ~60 over 5 min), inspect surrounding `info!` density for the gauge-noise verdict. Recorded in spike report § 7 "Gauge usefulness verdict".

### Task 5: Manual FUXA + Ignition / UaExpert verification (AC#3) — DEFERRED to post-Epic-9

- [x] **DEFERRED per user decision 2026-04-30.** Manual SCADA verification (AC#3) is consolidated into a single integration pass after Epic 9 lands — running it three times (8-1 spike, 8-2 production subscriptions, Epic 9 web UI) is wasteful operator time when each manual run takes ~30–60 minutes for tag setup + screenshots. Recorded in `deferred-work.md` "Story 8-1" block per CLAUDE.md loop-discipline rule #3 (explicit operator acceptance with documented reason).
- [x] **Compensating test-depth.** Added 6 automated integration tests (multi-client, ten-monitored-items, sampling-interval revision, value-flow payload, teardown idempotency, sibling-session isolation) to `tests/opcua_subscription_spike.rs` — the regression-risk areas that automated tests can catch ahead of a real SCADA deployment. Test inventory in spike report § 8.
- [x] **Carried forward to GitHub issue #93.** When Epic 9 retro is being prepared, run a single SCADA verification pass with FUXA + Ignition (and/or UaExpert) covering subscription delivery, historical-data read (Story 8-3 territory), threshold alarms (Story 8-4 territory), and web-UI hot-reload (Epic 9 territory) all together. Capture screenshots; append outcome subsection to spike report § 8. Issue closes on completion.

### Task 6: Session-rejected callback re-check + upstream FR (AC#7)

- [x] Re-greped `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/session/manager.rs` and `.../server.rs` and `.../builder.rs`. Re-confirmed Story 7-3's finding: **no session-rejected callback hook in 0.17.1.** Disk inventory confirms only 0.17.1 is resolved by Cargo (no 0.17.1.x patch or 0.18.x available).
- [x] **Library's own `// TODO: Auditing and diagnostics` comment at `manager.rs:74`** is the strongest possible upstream-FR hook. Recorded in spike report § 9.
- [x] **Carried forward to GitHub issue #94.** Operator files the upstream FR at `https://github.com/freeopcua/async-opcua/issues` (suggested body in spike report § 9). Once filed, operator updates `deferred-work.md`'s existing "First-class session-rejected event in async-opcua" entry (Story 7-2 deferral block) in place with the upstream URL.

### Task 7: Spike report (AC#4)

- [x] Wrote `_bmad-output/implementation-artifacts/8-1-spike-report.md` with all 12 sections per AC#4. Final size ~22 KB (above the 8–15 KB target — accepting the verbosity for the architecture-reference value; Story 8-2's spec author reads it cover-to-cover and the load-bearing details are non-trivial).
- [x] Cross-linked from this story file's "References" section (existed already at story-creation time).
- [ ] **DEFERRED to operator follow-up commit.** Cross-link from `epics.md` Epic 7 (= sprint-status Epic 8) "spike completed" preamble (one-line addition under the Phase A carry-forward block). Not done in this dev-story run because `epics.md` is a planning artefact owned by the spec polish step that landed at commit `2af5e9c`; touching it again here would entangle the spike commit with planning-artefact edits. Operator should append it in the same commit that flips the story to `done`, or skip if the spike-report path is already discoverable via the spike file's references.

### Task 8: Documentation sync + sprint-status update (CLAUDE.md compliance)

- [x] Updated `README.md` Planning table row for Epic 8: `🔄 in-progress (8-1 review)` per the implementation-commit status. The detailed story summary in the Epic 8 cell records: Plan A confirmation, integration-test names, reference binary path, spike report path, four-knob `Limits` minimum, HALT-blocking operator carry-forward (AC#3, AC#7, AC#8), test count delta. README "Current Version" line updated to `2026-04-30 (Story 8-1 spike in review)`.
- [x] Verified: no new feature, config knob, env var, CLI flag, or behavioural change introduced by the spike. Net `src/` and `config/` change: zero. Spike binary lives in `examples/`; tests live in `tests/`; only operator-visible artifact is the Planning-row update.
- [x] Updated `sprint-status.yaml`:
  - `epic-8` flipped to `in-progress` (already done at story-creation time).
  - `8-1-async-opcua-subscription-spike` flipped from `ready-for-dev` to `in-progress` (NOT to `review` — see "Status decision" below).
  - `last_updated` field rewritten with the dev-story outcome summary.
- [x] **Status decision: 8-1 stays in-progress, NOT review.** Per the up-front user-action HALT agreement (AC#3 manual FUXA + UaExpert verification, AC#7 upstream FR filing, AC#8 5-min `--load-probe` execution all need operator action), the story does not flip to `review` at the end of this dev-story run. CLAUDE.md "Code Review & Story Validation Loop Discipline" requires HIGH/MEDIUM ACs to be patched or explicitly accepted as deferred before flipping; AC#3 + AC#7 + AC#8 are HIGH-severity acceptance criteria that have not yet been satisfied. Operator completes them, then a follow-up dev-story re-run flips the story to `review`.

### Task 9: Final verification (AC#10)

- [x] `cargo test --lib --bins --tests` — **592 passed / 0 failed / 7 ignored** (up from Epic 7 baseline 581 / 0 / 7; net +11 tests = 9 spike integration + 2 indirect counter-stretches in lib/bin re-counting). Above the AC#10 target of ≥ 584. Captured in spike report § 11 final line. Initial dev-story round shipped 3 integration tests; the 2026-04-30 test-depth round added 6 more after the AC#3 deferral decision.
- [x] `cargo clippy --all-targets -- -D warnings` — clean. One spike-binary nit fixed during verification (`for (_handle, ts) in &per_item` → `for ts in per_item.values()` per the `for-kv-map` lint).
- [x] **Carried forward to GitHub issue #95.** Manual gateway run + `cargo run --example opcua_subscription_spike -- --plan-a` is the same operator-action surface as the load probe; both belong to issue #95.
- [x] **Carried forward to GitHub issue #93.** AC#3 screenshots will land at the post-Epic-9 SCADA verification pass.
- [x] `git diff --stat src/` — empty. **Zero `.rs` files modified in `src/`.** Optional `OPCUA_SPIKE_8_1_EVENT_TARGET` constant was NOT added (not needed; the spike binary uses default tracing targets without filter pollution).
- [x] Updated `sprint-status.yaml` `last_updated` with dev-story outcome — see Task 8.

---

## Dev Notes

### Anti-patterns to avoid (per CLAUDE.md scope-discipline rule)

- **Do not** modify `src/opc_ua.rs`, `src/storage/`, `src/chirpstack.rs`, or any other production-code module to make subscriptions work. The spike's whole point is "what does already-built code do?" — the strong prior says no production code changes are needed for Plan A. If a code change *seems* needed, **stop, document the failure mode, and pivot to AC#1 Plan B branch** rather than smuggle subscription support into 8-1.
- **Do not** introduce a push-model `set_variable_value` writer in the poller. That is Story 8-2 work even if the spike concludes push is recommended.
- **Do not** add new `[opcua].xxx` config knobs for `Limits` fields in this story. AC#6's enumeration is **input** for Story 8-2; the actual knob plumbing is 8-2 work.
- **Do not** retire the `AtLimitAcceptLayer` workaround in this story even if AC#7 finds an upstream callback hook. The retirement is 8-2's call; the spike only flags the opportunity.
- **Do not** introduce new `event="..."` values in production targets (`opcgw_server`, `opcgw::opc_ua`, etc.). Spike-binary-emitted events live under target `opcgw_spike_8_1` and are dropped before 8-2.
- **Do not** extract a `tests/common/` harness module in this story. Three test files (`opc_ua_security_endpoints.rs`, `opc_ua_connection_limit.rs`, `opcua_subscription_spike.rs`) is one short of the four-file extraction threshold per the project convention. Inline duplication with a top-of-file "Mirror of … keep in sync" comment is the right move for the third file.
- **Do not** treat the spike's reference binary as a new operator-facing feature. The binary is a developer test tool. Document this in the binary's header comment block + the spike report.
- **Do not** widen the spike scope by exploring `HistoryRead` or alarm conditions. Those are Stories 8-3 / 8-4 — separate stories with their own specs.
- **Do not** add new production dependencies. Reuse what's already in `Cargo.toml`. The `[dev-dependencies]` `async-opcua` with `client` feature is already there from Story 7-2.
- **Do not** weaken Story 7-1 / 7-2 / 7-3 invariants. Specifically: the redacting `Debug` impls must still apply (no `dbg!`/`println!` of `AppConfig` in the spike binary); the `OpcgwAuthManager` must still gate every session including subscription-creating ones; the `AtLimitAcceptLayer` must still emit on at-limit accepts even when the rejected client was about to create a subscription.
- **Do not** ship the spike report with placeholder text ("TBD", "TODO", "see X"). If a section can't be filled in (e.g., Plan B if Plan A succeeds), use the exact one-paragraph "not triggered" placeholder per AC#4 § 10.

### Why a spike vs a full implementation in 8-1

Story 8-2 ("OPC UA Subscription Support") is the production-quality landing. 8-1 exists because of a **named risk** in the PRD risk matrix (`prd.md:193` — "async-opcua can't do subscriptions: Phase B blocked; documented Plan B"). The spike's role is **early-fail detection**: spend 1–2 days verifying Plan A works empirically, so 8-2 can be planned with confidence.

The strong prior is that Plan A succeeds: source inspection of `~/.cargo/registry/src/.../async-opcua-server-0.17.1/src/node_manager/memory/simple.rs:180–262` shows `SimpleNodeManagerImpl` already implements all four monitored-item lifecycle hooks and auto-wires a `SyncSampler` against the existing `add_read_callback` registrations. **Subscriptions almost certainly already work today** through opcgw's existing pull-via-callback model.

The spike is therefore **mostly a documentation exercise** with a small reference binary as evidence. The throughput measurement (AC#8) is the load-bearing empirical input — if pull-model latency under 100×1Hz×1-subscriber load is acceptable, Story 8-2 stays simple ("just expose the four `Limits` knobs"). If pull-model latency is bad, Story 8-2 inherits a push-model implementation task with an order-of-magnitude wider scope.

### Why no production code in `src/`

CLAUDE.md scope-discipline rule: "Don't add features, refactor, or introduce abstractions beyond what the task requires." The spike's task is to validate. Any production code change risks crossing into 8-2 scope and entangling 8-1 + 8-2 in a single review (which Epic 6's retrospective and the per-story commit rule explicitly forbid).

The single allowed exception — the optional `OPCUA_SPIKE_8_1_EVENT_TARGET` constant in `src/utils.rs` — exists because the spike binary may want a tracing-target name that filters cleanly out of production logs. If used, it must be `#[cfg(any(test, feature = "spike-8-1"))]`-gated so it does not bloat the production binary.

### Why the spike binary lives in `examples/` not `src/bin/`

`examples/` is `cargo`'s convention for "code that demonstrates how to use the project" — fits a spike's "reference test" role. `src/bin/` is for "additional binaries the project ships" — would imply 8-1 produces an operator-facing tool, which it does not.

`examples/opcua_client_smoke.rs` from Story 7-2 is the precedent. The spike binary follows the same shape (clap-driven CLI + connection setup + scenario loop + JSON-on-stderr summary).

### Plan A failure-mode decision tree

If the spike binary's Plan A run fails, the failure mode determines next steps:

| Failure mode | Likely cause | Plan B implication |
|---|---|---|
| `CreateSubscription` returns `BadServiceUnsupported` / `BadNotImplemented` | async-opcua subscription service genuinely not wired in 0.17.1 — contradicts source inspection | Plan B option 2 (upstream contribution) is most aligned; cost is "small if the gap is one method" / "large if the entire subscription pipeline is missing" |
| `CreateMonitoredItems` succeeds but no `DataChangeNotification` arrives within 5 × publishing-interval | Sampler not running, OR sampler running but `add_read_callback`-registered callback not being invoked, OR notifications generated but not delivered through `Publish` | Plan B option 3 (change-detection polling layer in opcgw) is most aligned; cost depends on which sub-layer fails — sampler-level diagnostic logging in async-opcua-server's `simple.rs:180–228` would tell us |
| Notifications arrive but with implausible status codes / zero values / mangled timestamps | DataValue construction shape mismatch between read-callback and sampler expectation | Plan B option 2 (upstream contribution) is most aligned; usually a small fix to the sampler-to-publish bridge |
| Plan A passes the 5-s test but throughput probe (AC#8) fails (drop rate > 5% or latency > 3× publishing interval) | Pull-model lock contention OR sampler scheduling at min-interval = 100 ms can't keep up with 100 items at 1 Hz | **Not a Plan B trigger** — pull/push decision (AC#2) is the right tool. Recommend push model in Story 8-2; document the contention root cause in the spike report. |
| FUXA accepts the subscription but shows stale or never-updating values | Likely an FUXA-side polling-rate config issue, NOT a server-side issue | Document as a FUXA-config quirk in AC#3 manual verification; not a Plan B trigger. UaExpert (or another second client) is the canonical second opinion. |
| Auth or cap test (AC#9) fails | Story 7-2 / 7-3 invariant violated | Halt the spike, open a regression issue, escalate to Epic 7 retro re-open. Do not proceed with Plan A confirmation until the regression is understood. |

### Why the load-probe sustains 5 minutes

The architecture spec at `architecture.md:48` says "<100ms OPC UA reads" (NFR1). The carry-forward bullet at `epics.md:704` calls out 100 monitored items × 1 Hz × 1 subscriber as the representative load shape. That's a 30,000-notification target across 5 minutes — long enough to (a) catch slow-leak resource issues that don't surface in a 30 s smoke, (b) give a meaningful tail-latency distribution (p95 / p99 need ≥ 100 samples to be statistically interesting; 30,000 samples is plenty), and (c) generate enough `event="opcua_session_count"` gauge ticks (60 expected) to evaluate AC#8's gauge-noise question.

5 minutes is also short enough that the operator running the spike doesn't lose patience. 30 minutes would be more statistically robust but blows the 1–2 day spike budget.

If the 5-minute probe surfaces a yellow flag (drop rate 0–5%, latency in the "concerning but acceptable" envelope) the spike report recommends Story 8-2 re-runs the probe at 30 minutes to confirm. **The spike does not run the 30-minute probe itself** — that is 8-2 work.

### Manual FUXA + UaExpert verification — operator burden

AC#3 requires hands-on screen capture. **There is no way around this** — `epics.md:699` and NFR22 (`prd.md:461`) require FUXA + one additional client, and FUXA is not a CLI tool. The spike author is expected to spend ~30–60 minutes setting up the manual verification.

If FUXA is not available locally, the spike author should request access from Guy (`docs/manual/opcgw-user-manual.xml` v2.0 references FUXA setup instructions). UaExpert is freely downloadable (https://www.unified-automation.com/products/development-tools/uaexpert.html).

The 4-screenshot-minimum is a floor, not a ceiling — additional evidence of error states, settings, etc. is welcome. PNG format preferred; commit in `8-1-spike-evidence/` directly.

### Project Structure Notes

- `src/opc_ua.rs` — at HEAD of Story 7-3 it's ~2500 lines. Story 8-1 adds **zero lines** (per AC#5). 8-2 will likely add ~50–150 lines for `Limits` knob plumbing + (if push model recommended) a `set_variable_value` writer task.
- `src/utils.rs` — at HEAD ~480 lines. Story 8-1 adds zero or one lines (the optional `OPCUA_SPIKE_8_1_EVENT_TARGET` constant, `#[cfg]`-gated).
- `src/main.rs` — at HEAD ~1215 lines. Story 8-1 adds **zero lines**. The NFR12 startup-warn is already in place from commit `344902d`.
- `src/config.rs` — at HEAD ~2355 lines. Story 8-1 adds **zero lines**. 8-2 will add `[opcua]` knob fields + Debug impl entries + validate accumulator entries.
- `examples/opcua_subscription_spike.rs` — **new file**, ~150–250 LOC including doc comments and the load-probe path.
- `tests/opcua_subscription_spike.rs` — **new integration-test file**, ~250–400 LOC including duplicated harness from `tests/opc_ua_connection_limit.rs`. Top-of-file comment marks the duplication: "Mirror of tests/opc_ua_connection_limit.rs harness — keep in sync; refactor into tests/common/ when the fourth user appears."
- `_bmad-output/implementation-artifacts/8-1-spike-report.md` — **new file**, 8–15 KB. Architecture-reference doc. Survives Story 8-1's lifetime; Stories 8-2 / 8-3 / 8-4 cite it by path.
- `_bmad-output/implementation-artifacts/8-1-spike-evidence/` — **new directory** containing FUXA/UaExpert screenshots (PNG). Directory is referenced from the spike report.
- `_bmad-output/implementation-artifacts/deferred-work.md` — **edited in place** to add the upstream-FR URL to the existing "First-class session-rejected event in async-opcua" entry (Story 7-2 deferral block).
- `Cargo.toml` — **no new dependencies** expected. async-opcua client is already in `[dev-dependencies]`.

Modified files (expected File List, ~7 files):

- `examples/opcua_subscription_spike.rs` — new.
- `tests/opcua_subscription_spike.rs` — new.
- `_bmad-output/implementation-artifacts/8-1-spike-report.md` — new.
- `_bmad-output/implementation-artifacts/8-1-spike-evidence/*.png` — new (≥ 4 files).
- `_bmad-output/implementation-artifacts/deferred-work.md` — in-place edit (one entry updated).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — status flips + `last_updated` line.
- `README.md` — Planning-row status flip.

Optionally:
- `src/utils.rs` — single new constant, `#[cfg]`-gated, only if used.
- `_bmad-output/planning-artifacts/epics.md` — single-line cross-link to spike report (Task 7 last bullet).

### Testing Standards

This subsection is the **single source of truth** for testing patterns Story 8-1 should reuse.

- **Unit tests:** none introduced in `src/`. (The spike binary may have inline `#[cfg(test)]` unit tests for any small helper — e.g., a JSON summary parser — at the spike author's discretion.)
- **Integration tests** (`tests/opcua_subscription_spike.rs`): use `tokio::test(flavor = "multi_thread", worker_threads = 2)` for tests that spin up the OPC UA server in a child task (matching Story 7-2's and Story 7-3's pattern).
- **Free-port discovery:** identical to Story 7-2 / 7-3 (`tokio::net::TcpListener::bind("127.0.0.1:0")` + `.local_addr().port()` + drop, race-window-acceptable).
- **Held-session pattern (from Story 7-3):** the AC#9 cap test needs to hold a subscription-creating session while opening the (N+1)th. Reuse `HeldSession` shape from `tests/opc_ua_connection_limit.rs` with explicit `disconnect().await` before drop.
- **Tracing capture:** use the same `init_test_subscriber()` shape as Story 7-3. Do **not** annotate test functions with `#[traced_test]` — incompatible with multi-layer `Registry` composition. Use `captured_log_line_contains_all` for multi-substring single-line assertions on `event=...` lines.
- **Subscription-flow timeouts:** the AC#1 reference binary uses a 5 s timeout (5 × 1 s publishing interval) for first-notification arrival. The integration-test version of the same flow uses a 10 s timeout to absorb sampler-tick jitter on slower CI hardware.
- **Serial test execution:** mark all tests `#[serial_test::serial]` (Story 7-3 already added `serial_test = "3"` to `[dev-dependencies]`). Three tests share a process-wide tracing subscriber + `tracing-test` global buffer; running them in parallel would cross-contaminate substring assertions.
- **No new dev-dependencies expected.** If a new crate is genuinely required (e.g., `serde_json` for the load-probe summary parsing — but `serde_json` is likely already a transitive dep), document the rationale in the spike report.

### References

- [Source: `_bmad-output/planning-artifacts/epics.md#Story 7.1 = sprint-status 8-1`] (lines 686–705) — the original BDD acceptance criteria
- [Source: `_bmad-output/planning-artifacts/epics.md#Phase A carry-forward`] (lines 678–684) — NFR12 source-IP audit, per-IP rate limiting (#88), subscription/message-size limits (#89), gauge tunability bullets
- [Source: `_bmad-output/planning-artifacts/epics.md` line 676] — Numbering offset declaration
- [Source: `_bmad-output/planning-artifacts/prd.md#FR21`] (line 378) — subscription-based data change notifications
- [Source: `_bmad-output/planning-artifacts/prd.md#NFR22`] (line 461) — FUXA + at least one additional OPC UA client compatibility
- [Source: `_bmad-output/planning-artifacts/prd.md#Risk: async-opcua can't do subscriptions`] (line 193) — the named risk this spike retires
- [Source: `_bmad-output/planning-artifacts/architecture.md#OPC UA data flow (Phase B, pending spike)`] (lines 211–215) — push-model design
- [Source: `_bmad-output/planning-artifacts/architecture.md#async-opcua 0.16 → 0.17`] (lines 131–132) — explicit "spike subscription support" callout
- [Source: `_bmad-output/planning-artifacts/architecture.md#Implementation Sequence`] (line 244) — "async-opcua subscription spike" listed
- [Source: `_bmad-output/implementation-artifacts/epic-7-retro-2026-04-29.md#Discovery 1: NFR12 source-IP correlation degrades silently`] (lines 211–222) — NFR12 silent-degradation precondition
- [Source: `_bmad-output/implementation-artifacts/epic-7-retro-2026-04-29.md#Action Items - Decided in this retrospective`] (lines 260–264) — Q3 = b (NFR12 startup warn) and Q4 = B (Epic 8 spec polish first) decisions
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md#Deferred from: Story 7-2`] (line 119) — "First-class session-rejected event in async-opcua" entry to update (Task 6)
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md#Deferred from: Story 7-3`] (lines 142–143) — surface async-opcua subscription / message-size limits as config knobs (#89), hot-reload of session cap (#90)
- [Source: `_bmad-output/implementation-artifacts/7-3-connection-limiting.md`] — Story 7-3 patterns: integration-test harness shape, `init_test_subscriber`, `HeldSession`, `captured_log_line_contains_all`, `setup_test_server_with_max`
- [Source: `_bmad-output/implementation-artifacts/7-2-opc-ua-security-endpoints-and-authentication.md`] — Story 7-2 patterns: `OpcgwAuthManager` + `event="opcua_auth_failed"` audit-event invariant, `examples/opcua_client_smoke.rs` precedent
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/node_manager/memory/simple.rs:127, 132–144, 180–262, 304–314`] — `SimpleNodeManagerImpl` subscription-engine integration
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/builder.rs:246, 254, 263, 380, 422, 428, 439, 460–530`] — `ServerBuilder::limits` / `limits_mut` / direct setters for top-level Limits fields
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/config/limits.rs:1–127`] — `Limits` and `SubscriptionLimits` struct definitions (the source of truth for AC#6 field names)
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/lib.rs:60–145`] — library default constants (`MAX_SESSIONS = 20`, `MAX_SUBSCRIPTIONS_PER_SESSION = 10`, `DEFAULT_MAX_MONITORED_ITEMS_PER_SUB = 1000`, `MIN_SAMPLING_INTERVAL_MS = 100`, etc.)
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/session/manager.rs:64–72`] — session-creation rejection path (AC#7 re-check target)
- [Source: `src/opc_ua.rs:168–244, 305–333, 568–633, 673–732, 810, 872, 880, 888`] — current `create_server`, `configure_limits`, `run`, `add_nodes`, `add_read_callback` integration points
- [Source: `src/opc_ua_session_monitor.rs`] — Story 7-3's `AtLimitAcceptLayer` (AC#9 cap test invariant)
- [Source: `src/opc_ua_auth.rs`] — Story 7-2's `OpcgwAuthManager` (AC#9 auth test invariant)
- [Source: `src/main.rs::initialise_tracing` (commit 344902d)] — NFR12 startup-warn precondition
- [Source: `Cargo.toml:21, 49–61`] — `async-opcua = "0.17.1"` in `[dependencies]` (server feature) + `[dev-dependencies]` (client feature), `tracing-test`, `tempfile`, `serial_test`
- [Source: `CLAUDE.md`] — per-story commit rule, code-review loop rule, documentation-sync rule, security-check requirement, scope-discipline rule

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) via `bmad-dev-story 8-1` execution on 2026-04-29 / 2026-04-30.

### Debug Log References

- First test-run hit a single field-name assertion mismatch — assertion was `username="opcua-user"` but the actual emit field in `src/opc_ua_auth.rs` is `user=opcua-user` (Display-formatted, no surrounding quotes). Captured the actual log line from the failing run, corrected the assertion, recorded the field-name in spike report § 8 + § 11 so Story 8-2 reviewers don't repeat the mistake.
- One clippy nit on the load-probe binary: `for (_handle, ts) in &per_item` triggered the `for-kv-map` lint. Replaced with `for ts in per_item.values()`. Net diff: 1 line.
- No other compile / runtime / lint failures during implementation.

### Completion Notes List

- **Plan A confirmed empirically.** All 3 integration tests pass (13.62 s wall clock for the test binary). Source-prior reading of async-opcua's `SimpleNodeManagerImpl` was correct: subscriptions work end-to-end against opcgw at HEAD with **zero `src/` changes**.
- **Pull-vs-push recommendation: stay pull (provisional).** Source-grep + sampler architecture suggests the existing model is sufficient for Phase B's sizing target. Final call gated on the operator's `--load-probe` execution; threshold rules in spike report § 6.
- **`Limits` Section A enumeration done.** Four-knob minimum (`max_subscriptions_per_session`, `max_monitored_items_per_sub`, `max_message_size`, `max_chunk_count`) plus 5 should-expose subscription-flood knobs. **Field-name correction**: library uses `max_monitored_items_per_sub`, NOT `_per_subscription` per `epics.md`. Story 8-2 must use the library name.
- **Upstream FR re-confirmation.** No session-rejected callback hook in async-opcua 0.17.1. Library's own `// TODO: Auditing and diagnostics` comment at `session/manager.rs:74` is the strongest possible upstream-FR hook. **Operator action required** to actually file the FR at `github.com/freeopcua/async-opcua/issues` (suggested issue body in spike report § 9).
- **Auth + cap composition tests pass.** `test_subscription_client_rejected_by_auth_manager` and `test_subscription_client_rejected_by_at_limit_layer` pin Story 7-2's `OpcgwAuthManager` and Story 7-3's `AtLimitAcceptLayer` as compatible with subscription-creating clients. NFR12 carry-forward acknowledgment satisfied.
- **Test counts post-spike: 592 pass / 0 fail / 7 ignored** (after 2026-04-30 test-depth round). Up from 581 Epic 7 baseline; +11 tests (9 integration + 2 indirect counter shifts). Above the AC#10 target of ≥ 584. Clippy clean across the workspace.
- **Zero `src/` changes.** AC#5 satisfied. The optional `OPCUA_SPIKE_8_1_EVENT_TARGET` constant was not added (not needed). All deliverables live in `examples/`, `tests/`, and `_bmad-output/`.
- **Story flipped directly to `done` (2026-04-30, second pass).** User direction was to skip `review` and capture all remaining operator-action items as GitHub issues so they're tracked publicly and not lost. Issues opened: **#92** main tracker, **#93** AC#3 manual FUXA + Ignition / UaExpert verification deferred to post-Epic-9, **#94** AC#7 upstream FR for session-rejected callback in async-opcua, **#95** AC#8 `--load-probe` 5-min run. None of the three carry-forward items block Story 8-2 entry; each has a clear definition-of-done in its issue body. `deferred-work.md` "Story 8-1" block updated with the issue cross-references.

### File List

**New files (4):**
- `examples/opcua_subscription_spike.rs` — clap-driven CLI reference binary with `--plan-a` (default) and `--load-probe` modes. ~590 LOC including hand-written JSON-on-stderr summary emit (no `serde_json` dep added).
- `tests/opcua_subscription_spike.rs` — 3 integration tests (Plan A confirmation, auth rejection, at-limit rejection). ~580 LOC including the harness mirror from `tests/opc_ua_connection_limit.rs`. Top-of-file comment marks the duplication for the future `tests/common/` extraction trigger.
- `_bmad-output/implementation-artifacts/8-1-spike-report.md` — 12-section architecture-reference report. ~22 KB (above the 8–15 KB target; accepted for the architecture-reference value).
- `_bmad-output/implementation-artifacts/8-1-async-opcua-subscription-spike.md` — this story file (created 2026-04-29 by `bmad-create-story 8-1`; updated by this dev-story run).

**Modified files (2):**
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — `8-1-async-opcua-subscription-spike` flipped from `ready-for-dev` to `in-progress`; `last_updated` rewritten with the dev-story outcome.
- `README.md` — Planning-table row for Epic 8 updated to `🔄 in-progress (8-1 review)` with the per-story story summary; "Current Version" line updated.

**Pending operator action (HALT-blocking):**
- `_bmad-output/implementation-artifacts/deferred-work.md` — in-place edit to existing "First-class session-rejected event in async-opcua" entry (after upstream FR filed).
- `_bmad-output/implementation-artifacts/8-1-spike-evidence/*.png` — 4+ screenshots from FUXA + UaExpert manual verification.
- `_bmad-output/implementation-artifacts/8-1-spike-report.md` — § 3 (manual run JSON), § 7 (`--load-probe` numbers + gauge verdict), § 7a or extension to § 8 (FUXA + UaExpert evidence). All sections have placeholder/recipe text today.
- (External) GitHub issue creation: Story 8-1 main tracker issue + upstream FR at `github.com/freeopcua/async-opcua`.

**Files NOT modified (per AC#5 — production code untouched):**
- All `src/*.rs` files. Verified via `git diff --stat src/` showing zero output.
- `Cargo.toml` — no new dependencies (production or dev).
- `config/config.toml`, `config/config.example.toml`, `tests/config/config.toml` — no fixture changes.

### Change Log

| Date | Status flip | Summary |
|---|---|---|
| 2026-04-29 | created → ready-for-dev | `bmad-create-story 8-1` produced the spec from the Epic 8 carry-forward bullets. |
| 2026-04-29 | ready-for-dev → in-progress | `bmad-dev-story 8-1` started. |
| 2026-04-29 | (in-progress) | Plan A confirmed empirically: 3 integration tests pass; reference binary + spike report shipped; zero `src/` modifications. |
| 2026-04-30 | (in-progress) | Test-depth round: AC#3 explicitly deferred to a single integration pass after Epic 9 lands (per user decision); 6 compensating integration tests added (multi-client, ten-monitored-items, sampling-interval revision, value-flow payload, teardown idempotency, sibling-session isolation). 9 spike tests total, all passing. Test totals: 592 / 0 / 7. |
| 2026-04-30 | in-progress → done | Three operator-action carry-forward items captured as GitHub issues #93 (AC#3), #94 (AC#7), #95 (AC#8) so they're tracked publicly. Main tracker #92. `deferred-work.md` updated with issue cross-references. README + sprint-status flipped. |

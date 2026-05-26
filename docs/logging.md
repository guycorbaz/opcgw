# Logging

opcgw is built on [`tracing`](https://docs.rs/tracing) + [`tracing-subscriber`](https://docs.rs/tracing-subscriber). This document covers what an operator needs to know to tune verbosity, locate log files, and interpret structured log fields without rebuilding the gateway.

## Quick reference

| Level | Use case | Expected volume (10 devices, 1 Hz poll) |
|-------|----------|------------------------------------------|
| `trace` | Deepest debugging — every operation, every span entry, every storage query timing | very high |
| `debug` | Production troubleshooting — key decisions, timings, correlation IDs, staleness checks | moderate |
| `info` (default) | Normal operations — cycle starts/ends, state transitions, errors counted | low |
| `warn` | Anomalies and retries that succeeded — early warning | sparse |
| `error` | Unrecoverable conditions only | silent if healthy |

> **Rule of thumb:** stay on `info` in production. Move to `debug` to investigate a specific incident, then back to `info`. Use `trace` only when debugging the gateway itself, never for routine operations — it can produce thousands of lines per minute.

## Setting the level

The global default level is resolved at startup, in this precedence order (highest wins):

1. `OPCGW_LOG_LEVEL` environment variable
2. `[logging].level` in `config/config.toml`
3. Hard-coded default `info`

Valid values are `trace`, `debug`, `info`, `warn`, `error` — case-insensitive (`TRACE`, `Debug`, `iNfO` are all accepted). Invalid values produce a single warning line on stderr and fall through to the next layer in the chain — startup never aborts.

A restart is required after changing the level (no hot-reload). The level filter is evaluated once at subscriber init; `tracing` macros short-circuit with near-zero overhead when the level is below threshold — measured at ~0.46 ns per filtered call (effectively a single comparison + branch), so a `trace!` line in a hot path is cheap, but not literally free, when running at `info`.

### Worked examples

**Run with extra detail for an incident:**
```bash
OPCGW_LOG_LEVEL=debug ./target/release/opcgw
```

**Run quietly — only errors on the global console / root file:**
```bash
OPCGW_LOG_LEVEL=error ./target/release/opcgw
```

**Persist the choice in `config.toml` instead of an env var:**
```toml
[logging]
level = "debug"
```

**Override config-file value temporarily:**
```bash
# Even with [logging].level = "info" in config.toml,
# the env var wins for this run:
OPCGW_LOG_LEVEL=trace ./target/release/opcgw
```

### Where the level is reported back

After the subscriber is up, opcgw emits a `logging_init` line so you can confirm what actually took effect:

```
INFO opcgw: Resolved global log level operation="logging_init" level=DEBUG source="env"
```

`source` is one of `"env"`, `"config"`, or `"default"`.

> **Note:** if you set `OPCGW_LOG_LEVEL=error`, this `logging_init` line is itself suppressed (it's emitted at `info`). That is intentional — `error` means *only* errors.

## Per-module file appenders are independent

Setting `OPCGW_LOG_LEVEL=error` only affects the **global** console (stderr) layer and the root file appender (`opc_ua_gw.log`). The per-module file appenders are configured separately at TRACE level and continue to capture deep detail regardless of the global setting:

| File | Captures everything from |
|------|---------------------------|
| `chirpstack.log` | `opcgw::chirpstack` (poller, ChirpStack gRPC) |
| `opc_ua.log` | `opcgw::opc_ua` (OPC UA server) + `async_opcua` |
| `storage.log` | `opcgw::storage` (SQLite backend, pool) |
| `config.log` | `opcgw::config` (configuration loader) |
| `opc_ua_gw.log` | everything (root, filtered by `OPCGW_LOG_LEVEL`) |

This separation is deliberate: the global level is for stderr noise; the per-module files are forensic. If something goes wrong and you set `OPCGW_LOG_LEVEL=error`, you still have full-fidelity per-module logs to dig into after the fact.

## Log directory

By default, log files land in `./log/` (relative to the working directory). The directory is configurable, again with the same precedence pattern:

1. `OPCGW_LOG_DIR` environment variable
2. `[logging].dir` in `config/config.toml`
3. Default `./log`

Empty / whitespace-only env values are treated as unset. The directory is created if missing and probed for writability at startup; if the requested location isn't writable, opcgw warns on stderr and falls back to `./log`.

```bash
# Send logs somewhere central
OPCGW_LOG_DIR=/var/log/opcgw ./target/release/opcgw

# Or via config:
[logging]
dir = "/var/log/opcgw"
```

## Structured fields you'll see

Every log line uses canonical structured fields (Story 6-1, AC#7). Common fields:

| Field | Meaning |
|-------|---------|
| `operation` | What happened (`poll_cycle_start`, `staleness_check`, `storage_query`, …) |
| `device_id` | LoRaWAN device identifier |
| `metric_name` | OPC UA / ChirpStack metric name |
| `request_id` | UUIDv4 correlation ID — **the same UUID appears on every log line emitted while serving one OPC UA read**, including downstream storage and staleness logs |
| `duration_ms` / `latency_ms` | Wall-clock timing in milliseconds |
| `status_code` | OPC UA status (`Good`, `Uncertain`, `Bad…`) |
| `success` | `true` / `false` for the operation outcome |

To trace one OPC UA read end-to-end, find its `request_id` in the console output and grep the per-module log files:

```bash
grep "request_id=df1c…" log/*.log
```

You'll see entries from `opc_ua.log` (read entry/exit, staleness check), `storage.log` (storage query timing), and the root `opc_ua_gw.log` — all carrying the same UUID.

## Nested env-var overrides

Any nested config field can be overridden via env vars using the figment double-underscore convention:

| Field | Env var |
|-------|---------|
| `[logging].dir` | `OPCGW_LOGGING__DIR` |
| `[logging].level` | `OPCGW_LOGGING__LEVEL` |
| `[chirpstack].api_token` | `OPCGW_CHIRPSTACK__API_TOKEN` |
| `[opcua].user_password` | `OPCGW_OPCUA__USER_PASSWORD` |

For the `[logging].dir` and `[logging].level` paths specifically, the short forms `OPCGW_LOG_DIR` and `OPCGW_LOG_LEVEL` are also accepted (read directly during the bootstrap phase, before figment parsing).

## Operations reference

Every event log line carries a structured `operation=` field. The table below is the canonical list — what each operation tells you, and what (if anything) you should do about it.

| `operation=` | Level | What it means | Typical action |
|--------------|-------|---------------|----------------|
| `logging_init` | `info` | Subscriber installed; reports the resolved global log level and its source (env / config / default). | None — confirmation line. |
| `poll_cycle_start` | `info` | A new ChirpStack poll cycle has begun; carries `device_count`. | None unless you're tracing cycle behavior. |
| `poll_cycle_end` | `info` | A poll cycle ended; carries `devices_polled`, `metrics_collected`, `errors`, `chirpstack_available`, `cycle_duration_ms`. | If `cycle_duration_ms > polling_frequency_secs * 1000` for several cycles, raise the polling interval. |
| `device_polled` | `debug` | Per-device cycle outcome (Story 6-1). | Cross-reference with `device_poll` warns to see which devices repeatedly fail. |
| `device_poll` | `warn` | A specific device failed inside a cycle (Story 6-3, AC#7). | Investigate the device's connectivity. Multiple devices failing → likely fleet-wide issue. |
| `chirpstack_connect` | `info` (attempt/success) / `warn` (failure) | TCP availability probe or gRPC channel connect. Carries `attempt`, `endpoint`, `latency_ms`/`error`, `success`. | Repeated `success=false` → ChirpStack server reachability. |
| `retry_schedule` | `info` | Logged just before the retry sleep; carries `attempt`, `delay_secs`, `next_retry`. | Operator visibility only. |
| `chirpstack_outage` | `warn` | First per-device connectivity failure of a cycle that flips `chirpstack_available` to false. Carries `last_successful_poll`. | Triggers the Story 4-4 auto-recovery loop; expect a sequence of `recovery_attempt` info lines bounded by `chirpstack.retry × chirpstack.delay` (shipped defaults: 30 × 1s = 30s budget). The recovery sequence ends with either `recovery_complete` (success) or `recovery_failed` (budget exhausted). If `last_successful_poll` is far in the past AND `recovery_failed` warns are recurring across cycles, ChirpStack is persistently unreachable — manual investigation needed (server status, network, credentials). |
| `recovery_attempt` | `info` | Story 4-4: a single attempt within the ChirpStack recovery loop. Carries `attempt` (1-indexed), `max_retries` (semantically the **attempt budget** — `chirpstack.retry = N` means N total probes, not N retries after the original failure; e.g. `retry = 1` means one probe before giving up, with `recovery_attempt attempt=1, max_retries=1` followed immediately by `recovery_failed attempts_used=1` if the probe fails), `delay_secs`, `last_error`. | Expected during a ChirpStack outage. If frequency is high during normal operation (>1/hour without a preceding `chirpstack_outage` warn), check `chirpstack.delay` config and upstream health. |
| `recovery_complete` | `info` | Story 4-4: recovery loop succeeded. Carries `attempts_used`, `downtime_secs` (or `from_startup=true` on cold start), `last_error`. | `downtime_secs` identifies outage duration; investigate ChirpStack-side root cause if persistent. |
| `recovery_failed` | `warn` | Story 4-4: recovery loop exhausted its retry budget without restoring connectivity. Carries `attempts_used`, `last_error`. | Manual intervention may be needed — check ChirpStack server status, network, credentials. |
| `chirpstack_request` | `warn` | gRPC request to ChirpStack returned an error. `exceeded=true` flags timeout (DeadlineExceeded); otherwise transient (Unavailable / Cancelled). | Repeated timeouts → upstream slow; repeated Unavailable → network partition. |
| `inventory_query` | `info` | Story C-1 (FR-Epic-C): a `/api/inventory/{applications,devices,uplinks}` query resulted in a fresh ChirpStack call (cache miss / refresh / bypassed / uncached path). **Fires on cache MISSES only** for applications + devices; ALWAYS fires for uplinks (uncached). **Field schema:** `event="inventory_query"`, `resource ∈ {applications, devices, uplinks}`, `cache_status ∈ {miss, refresh, bypassed}`, `tenant_id=<str>`, `application_id=<str when applicable>`, `dev_eui=<str when applicable>`, `response_status=<HTTP code>`, `chirpstack_response ∈ {ok, empty}`, `item_count=<usize>`, `duration_ms=<u64>`. Cache HITS are silent — log volume is bounded by `1 / inventory_cache_ttl_seconds × active_sessions`, not by clicks. | Grep `event="inventory_query"` to get an exact count of outbound ChirpStack inventory calls. If `chirpstack_response=empty` recurs on an `application_id` the operator just added, suspect cache-invalidation drift; cross-check with `event="inventory_cache_invalidated"`. |
| `inventory_query_failed` | `warn` | Story C-1: a `/api/inventory/*` query failed at the ChirpStack call. **Field schema:** `event="inventory_query_failed"`, `resource ∈ {applications, devices, uplinks}`, `reason ∈ {chirpstack_unreachable, chirpstack_auth_failed, chirpstack_grpc_error, shutdown_cancellation}`, `tenant_id=<str>`, `application_id=<str when applicable>`, `dev_eui=<str when applicable>`, `error=<str>`, `duration_ms=<u64>`. Returns HTTP 502 to the client. Iter-2 P2 added the `shutdown_cancellation` value — fires when a picker request is in flight during graceful gateway shutdown; not a real ChirpStack fault. | `chirpstack_unreachable` → check ChirpStack reachability (same triage as `chirpstack_outage`). `chirpstack_auth_failed` → check `[chirpstack].api_token` and the token's tenant scope. `chirpstack_grpc_error` → inspect the `error=` field for the gRPC status code. `shutdown_cancellation` → expected during gateway restart; suppress alerts on this reason during planned restarts. |
| `inventory_uplink_dropped` | `warn` | Story C-1 iter-2 P3: a single `LogItem` from `InternalService.StreamDeviceEvents` was dropped from the uplink result list because its proto timestamp was missing or malformed (negative seconds/nanos, nanos ≥ 10⁹). The rest of the stream is still emitted; only this one item is skipped. **Field schema:** `event="inventory_uplink_dropped"`, `reason="malformed_proto_timestamp"`, `description=<LogItem description, e.g. "up">`, `timestamp=<seconds=N,nanos=M> OR "missing">`. | Inspect the device codec or upstream ChirpStack — a stream of these for one device suggests a codec bug or proto-stream corruption. If only occasional, ChirpStack's `last_seen_at = epoch_zero` returns for never-seen devices may be the source; investigate device enrollment. |
| `inventory_cache_invalidated` | `info` | Story C-1: a CRUD write on `/api/applications` or `/api/devices` triggered an inventory cache invalidation. **Field schema:** `event="inventory_cache_invalidated"`, `cache_scope ∈ {applications, devices}`, `triggered_by ∈ {crud_post, crud_put, crud_delete}`. Application-scope invalidations don't include `application_id`; device-scope invalidations include the affected `application_id` (visible on adjacent log lines from the same handler). | Use to correlate operator workflows: "I added an application, then the picker showed it" should produce `application_created` followed by `inventory_cache_invalidated cache_scope=applications`. If the picker still shows stale data after this event, suspect a client-side cache (browser, reverse proxy). |
| `inventory_observed_key_heterogeneous` | `warn` | Story C-1: the wire-type inference for a key in `/api/inventory/uplinks?dev_eui=...` observed heterogeneous JSON types across recent uplinks and fell back to `String`. **Field schema:** `event="inventory_observed_key_heterogeneous"`, `dev_eui=<str>`, `key=<top-level key>`, `types_seen=<comma-separated JSON types, e.g. "number,string">`. The picker UI (C-2) lets the operator override the wire type. | Investigate the device codec — heterogeneous types usually mean a codec bug (sometimes-int / sometimes-string for the same key) or a firmware update mid-collection. The operator can override in the picker; the warn is a hint, not an error. |
| `metric_history_summary` | `trace` | A-5 iter-2 K6 review fix: aggregate-skip telemetry for `query_metric_history`. **Field schema:** `event="metric_history_summary"`, `device_id=<str>`, `metric_name=<str>`, `schema_drift_skipped=<u32>`, `unparseable_timestamp_skipped=<u32>`. Fires once per `query_metric_history` call when ANY rows were skipped (sum of per-row `metric_history_read` warns). Sibling of the per-row `metric_history_read` event — gives ops dashboards a single grep-recoverable line for cumulative counts without re-aggregating the per-row warns. Trace-level by default since per-row warns already provide the actionable signal. | For routine ops: filter trace logs by `event=metric_history_summary` to compute per-query skip rates over time. |
| `metric_history_read` | `warn` | A-5 (FR51): umbrella event for OPC UA HistoryRead-side diagnostics — sibling of `metric_read` for the live-read path. **Field schema (closed enum, A-5 iter-1 P2 review fix promoted `unparseable_timestamp` from a `reason_detail` sub-field to a first-class reason value):** `event="metric_history_read"`, `device_id=<str>`, `metric_name=<str>`, `reason ∈ {schema_drift, unparseable_timestamp, narrowing_overflow, narrowing_underflow}`. `schema_drift` (warn, A-5): fires from `query_metric_history` row-skip paths when a row has an unknown `value_type`, multi-set typed columns, or a missing discriminant column — defensive guard; should be unreachable post-A-3 v008 CHECK constraints, but a restored backup or raw SQL bypass could trigger it. `unparseable_timestamp` (warn, A-5 iter-1 P2): fires when `metric_history.timestamp` is not a parseable RFC3339 string — pre-existing carry-forward; should be unreachable post-A-3 since writers always emit `chrono::DateTime::to_rfc3339()`. `narrowing_overflow` / `narrowing_underflow` (warn, A-5): identical contract to the `metric_read` sibling but fired from `build_data_values` in the HistoryRead variant projection. The gateway returns `Variant::Float(0.0)` with `StatusCode::Good` on narrowing failure (see deferred-work.md DEF-iter1-A5-D2 for the status-code follow-up). **Legacy-row note:** pre-Epic-A rows tagged `value_type='legacy'` are NOT logged at this event — they surface as a `DataValue { value: None, status: BadDataUnavailable }` (Story A-5 AC#2 / AC#3, epic AC#1) without an event emission because their presence is expected during the Epic A migration window. | For `schema_drift` / `unparseable_timestamp`: inspect the offending row via raw SQL; restored-backup or manual-mutation diagnostic. For `narrowing_*`: same triage as the `metric_read` sibling — expose the metric via `Variant::Double` if extreme f64 magnitudes are legitimate measurements. |
| `metric_read` | `info` (no_payload) / `warn` (narrowing_*) | A-4 (FR51): umbrella event for OPC UA Read-side diagnostics. **Field schema (closed enum):** `event="metric_read"`, `device_id=<str>`, `metric_name=<str>`, `reason ∈ {no_payload, narrowing_overflow, narrowing_underflow}`. `no_payload` (info, iter-2 JR4): the requested metric has no payload available — either the row is absent OR the row is tagged `value_type='legacy'` and is awaiting the first poll-cycle UPSERT that replaces it with a typed payload (architecture.md:182). Carries `device_id`, `metric_name`. `narrowing_overflow` (warn, iter-1 IR7): f64 value finite but magnitude > `f32::MAX ≈ 3.4×10³⁸`, narrows to ±Inf. Carries the additional `f64_value=<f64>` field. `narrowing_underflow` (warn, iter-1 IR7): f64 magnitude below the f32 subnormal floor (~1.4×10⁻⁴⁵), narrows silently to `0.0_f32` losing the real value. The gateway returns `Variant::Float(0.0)` for both narrowing cases. | For `no_payload` on first startup after Epic A upgrade: expected — wait for next poll cycle. If persistent for >1 poll interval, check device connectivity / metric_name typo. For `narrowing_overflow` legitimate cases (rare scientific measurement at extreme magnitude), expose the metric via `Variant::Double` rather than `Variant::Float`. For `narrowing_underflow` (industrial chemistry, low-current sensors, scientific instruments below 1e-45), same recommendation applies. |
| `metric_view_serialize` | `debug` (per-row) / `warn` (aggregate) / `info` (int_precision_lossy) | A-6 (FR51): umbrella event from `metric_type_to_json_value` (the `/api/devices` dashboard wire path). **Field schema (closed enum):** `event="metric_view_serialize"`, `reason ∈ {non_finite, int_precision_lossy}`, `device_id=<str>`, `metric_name=<str>`. `non_finite` (debug per-row, A-3-poller-filtered, unreachable in production): a `MetricType::Float` payload is NaN / ±Inf and not representable as a bare JSON number. Carries `f64_value=<f64>`. Reaches the dashboard as `value: null` for the offending row (renders as "—" + "missing" badge per Story 9-3). Per-row emission is `debug!` for forensics; **one aggregate `warn!(event="metric_view_serialize", reason="non_finite", non_finite_count=N)` fires per `/api/devices` request** when any non-finite is encountered, so log volume stays bounded on a regressed sensor producing N non-finites per poll cycle (A-5 `metric_history_summary` aggregate pattern). Sibling debug field `f32_narrowed=<f32>` may also appear on the defensive P0-D1 f32-narrowing-to-infinity path (rare; only reachable for a finite f64 whose magnitude exceeds f32::MAX). `int_precision_lossy` (info, A-6 iter-1 P8): a `MetricType::Int` payload has `|i| > 2^53`; the wire is bit-exact JSON but JavaScript clients silently truncate to IEEE-754 double precision. Carries `i64_value=<i64>`. Operator-informational, not a defect — legitimate counters can exceed 2^53. **Note on legacy rows:** post-A-5 `load_all_metrics` silently skips `value_type='legacy'` rows BEFORE they reach the dashboard handler, so there is NO `legacy_row` reason — legacy rows surface as the same "configured but never polled" outcome at the wire (`value: null, timestamp: null`). | For `non_finite`: A-3 poller filter has regressed OR the database was mutated outside the gateway. Inspect `metric_history` for the same `device_id` + `metric_name`. For `int_precision_lossy`: if the SCADA dashboard needs bit-exact integer precision above 2^53, switch the consumer to a BigInt-aware JSON parser; otherwise accept the documented JS truncation. |
| `metric_parse` | `warn` | A metric value couldn't be coerced to its declared type, OR the value is non-finite (NaN/Inf), OR an Int-target value would saturate the i64 cast. **Field schema (A-3, FR51):** `event="metric_parse"`, `device_id=<str>`, `metric_name=<str>`, `raw_value=<f32>`, `expected_type ∈ {Float, Int, Bool, String, Unknown}`, `reason ∈ {invalid_bool, non_finite, int_overflow}`. The metric is skipped for this poll cycle. **Emission sites (iter-2 F-I):** `reason=invalid_bool` fires from `validate_bool_metric_value` (Bool target with non-`{0.0, 1.0}` raw); `reason=non_finite` and `reason=int_overflow` fire from `prepare_metric_for_batch` (production batch path) and `store_metric` (config-driven path, `#[allow(dead_code)]`). `expected_type="Unknown"` only emits from `store_metric` when the metric has no operator-config entry. **A-3 schema migration note:** the legacy `fallback_value` + `error` fields emitted by `validate_bool_metric_value` in pre-A-3 builds are gone — the field schema is now a closed enum per spec AC#10. Downstream log-grep pipelines that filtered on `fallback_value=none` must switch to `reason=invalid_bool`. | Verify the device firmware's emit format matches the configured metric type. For `reason=non_finite`: sensor calibration or numerical fault. For `reason=int_overflow`: Int-target value exceeds i64 range — investigate the device's emit precision. |
| `error_spike` | `warn` | Error count jumped by `>= 5` between consecutive cycles. Carries `previous`, `current`, `delta`. | Cross-reference with `device_poll` / `chirpstack_outage` from the same cycle. |
| `health_update` | `debug` | Poller wrote `gateway_status` (last_poll_timestamp, error_count, chirpstack_available). | Operator visibility only. |
| `health_metric_read` | `debug` (normal) / `warn` (NULL last_poll_timestamp) | OPC UA read of `last_poll_timestamp` / `error_count` / `chirpstack_available`. The warn variant fires before any successful poll has populated the row. | If the warn persists past the first poll cycle, the poller isn't writing health. |
| `gateway_status_init` | `info` | First read of `gateway_status` before any poll has populated it. Once per process. | None — startup signal. |
| `opc_ua_read` | `info` (span) / `warn` (budget exceeded) | OPC UA read of a metric. Span fields capture entry/exit. The warn fires when `duration_ms > 100`. | A consistent budget-exceeded warn → SQLite contention or a slow staleness check. |
| `staleness_check` | `debug` | Per-read staleness computation; carries `metric_age_secs`, `threshold_secs`, `is_stale`, `status_code`. | Filter by device to see if a single source is silent. |
| `staleness_boundary` | `debug` | Metric age within ±5 s of the staleness threshold — flickering between Good and Uncertain. | If a metric is constantly near-boundary, raise the threshold or investigate the device's emit cadence. |
| `staleness_transition` | `info` | Metric crossed Good ↔ Uncertain (or Uncertain ↔ Bad). | Indicates source health changed — confirm via the device's connectivity. |
| `storage_query` | `debug` (normal) / `warn` (slow / SQLITE_BUSY) | One SQLite query. Carries `query_type`, `latency_ms`. The warn variant fires when `latency_ms > 10` (`exceeded_budget=true`) or when SQLite returned `SQLITE_BUSY`. | Sustained budget exceeded → schema or index issue. SQLITE_BUSY → connection pool exhaustion. |
| `batch_write` | `debug` (normal) / `warn` (slow) | End-of-cycle batch persistence. Carries `metrics_count`, `latency_ms`. The warn variant fires when `latency_ms > 500`. | A slow batch_write blocks the next poll cycle — investigate disk health. |
| `txn_begin` / `txn_commit` / `txn_rollback` | `trace` | SQLite transaction boundaries inside `batch_write_metrics`. | Diagnostics only — captured in `storage.log`. |

### Audit and diagnostic events (`event=`)

Stories from 7-2 onward use a separate `event=` field instead of
`operation=` for security-relevant audit events and one-shot
diagnostic events (the `event=` prefix makes them easy to filter via
`grep 'event="..."' log/*.log`). The full audit-trail catalogue lives
in [`docs/security.md`](security.md); this is a quick-reference
index of the event names introduced so far.

| `event=` | Level | Story | Where documented |
|---|---|---|---|
| `opcua_auth_failed` | `warn` (audit) | 7-2 | `security.md` § OPC UA security endpoints and authentication |
| `opcua_session_count` | `info` (diag) | 7-3 | `security.md` § OPC UA connection limiting |
| `opcua_session_count_at_limit` | `warn` (audit) | 7-3 | `security.md` § OPC UA connection limiting |
| `opcua_limits_configured` | `info` (diag) | 8-2 | `security.md` § OPC UA subscription limits |
| `nfr12_correlation_check` | `warn` (one-shot) | 7-2 retro | `security.md` § OPC UA security endpoints and authentication |
| `web_auth_failed` | `warn` (audit) | 9-1 | `security.md` § Web UI authentication |
| `web_server_started` | `info` (diag) | 9-1 | `security.md` § Web UI authentication |
| `api_status_storage_error` | `warn` (diag) | 9-2 | `security.md` § Web UI authentication → API endpoints |
| `api_devices_storage_error` | `warn` (diag) | 9-3 | `security.md` § Web UI authentication → API endpoints |
| `config_reload_attempted` | `info` (diag) | 9-7 | ~~`security.md` § Configuration hot-reload~~ **(removed C-6 — SIGHUP listener removed; event no longer emitted)** |
| `config_reload_succeeded` | `info` (diag) | 9-7 | ~~`security.md` § Configuration hot-reload~~ **(removed C-6 — SIGHUP listener removed; event no longer emitted)** |
| `config_reload_failed` | `warn` (audit) | 9-7 | ~~`security.md` § Configuration hot-reload~~ **(removed C-6 — SIGHUP listener removed; event no longer emitted)** |
| `config_reload_rejected` | `warn` (audit) | C-3 | ~~`web-api.md` § Duplicate-rejection contract~~ **(removed C-6 — this event was only emitted from the SIGHUP duplicate-validation path, which was removed in C-6)** |
| `config_reload` | `info` (audit) | C-6 | Emitted by `notify_crud_write` after each successful CRUD write (POST / PUT / DELETE on applications, devices, metrics, or commands) rebuilds the in-memory snapshot from SQLite. Fields: `trigger="crud_write"`, `application_count=<usize>`. Replaces the now-removed SIGHUP-triggered `config_reload_attempted` / `config_reload_succeeded` pair. Grep: `event="config_reload" trigger="crud_write"`. |
| `config_migration` | `info` (audit) / `warn` for back-fill failure | C-6, D-0 | Boot-time TOML→SQLite one-shot migration audit. **C-6 stages (application tree):** `stage="toml_to_sqlite"` (migration ran): carries `applications=<N>`, `devices=<N>`, `metrics=<N>`, `commands=<N>`, `duration_ms=<u64>`. `stage="already_migrated"` fires on every boot after the first migration (meta done-flag is set); carries `applications=<N>` (current row count). `stage="skipped_empty_source"` fires when the TOML `application_list` is empty (C-0 fresh-bootstrap path). `stage="already_migrated_backfill_failed"` (**`warn`**) fires when the secondary already-migrated guard (apps present, no done-flag — e.g. direct SQLite import that bypassed migrate_applications_config) cannot write the back-fill meta key; field `error=<str>`; non-fatal — apps data is intact, retry attempted on subsequent boots if backend is healthy. **D-0 stages (singleton config):** `stage="singleton_toml_to_sqlite"` (migration ran): carries `sections=<N>`, `rows=<N>`, `duration_ms=<u64>`. `stage="singleton_already_migrated"` fires on every boot after the first singleton migration (D-0 done-flag set); carries `rows=<N>`. `stage="singleton_already_migrated_backfill_failed"` (**`warn`**) fires when the D-0 secondary already-migrated guard cannot write the back-fill meta key; field `error=<str>`; non-fatal — singleton data intact, retry attempted on subsequent boots. `stage="skipped_placeholder_singleton"` fires when `[chirpstack].api_token` or `[opcua].user_password` still carries the `REPLACE_ME_WITH_OPCGW_` placeholder string; field `missing_secret=<comma-separated list>`; migration deferred until secrets are supplied. See `docs/c-6-migration-runbook.md` + `docs/d-0-migration-runbook.md`. |
| `storage_init` | `info` (audit) / `warn` for wider-than-0o600 mode / `debug` for atomic-create probe failure | D-0 | SQLite file-permission diagnostics (AI-C-SEC-2 hardening). `info` fires on fresh creation of `data/opcgw.db` when opcgw successfully races the atomic-create probe (file created with mode 0o600); fields `path=<str>`. `warn` fires once-per-`ConnectionPool::new()` call on an existing database whose file mode is wider than 0o600 (e.g. inherited 0o644 from a 0o022 umask deployment); fields `path=<str>`, `mode=<oct>`. **`debug`** fires when the atomic-create probe failed with an unexpected I/O error (other than `AlreadyExists`) — e.g. read-only filesystem, permission denied; field `error=<str>`; `Connection::open` is allowed to proceed and will produce a more contextual error if the underlying issue is persistent. The warn variant is non-fatal — the gateway continues normally; the runbook (`docs/d-0-migration-runbook.md`) documents the operator chmod recipe. Windows deployments use ACLs rather than POSIX mode bits and do not emit this event (the atomic-create probe is gated by `#[cfg(unix)]`). |
| `config_migration_failed` | `warn` (audit) | C-6, D-0 | Boot-time migration failure. `reason="row_count_mismatch"` (C-6): SQLite row count after insert differs from the TOML source count for the application tree — transaction rolled back; fields `expected=<N>`, `actual=<N>`. `reason="singleton_row_count_mismatch"` (D-0): same shape but for the singleton-config migration (rows written ≠ rows expected across the four sections). `reason="insert_failed"`: SQLite threw an error during insert; field `error=<str>`. On failure the gateway falls back to TOML-driven boot for the current start-up only; the migration is retried idempotently on the next boot. See `docs/c-6-migration-runbook.md` + `docs/d-0-migration-runbook.md`. |
| `application_created` | `info` (audit) | 9-4 | `security.md` § Configuration mutations |
| `application_updated` | `info` (audit) | 9-4 | `security.md` § Configuration mutations |
| `application_deleted` | `info` (audit) | 9-4 | `security.md` § Configuration mutations |
| `application_crud_rejected` | `warn` (audit) | 9-4 | `security.md` § Configuration mutations |
| `device_created` | `info` (audit) | 9-5 | `security.md` § Configuration mutations |
| `device_updated` | `info` (audit) | 9-5 | `security.md` § Configuration mutations |
| `device_deleted` | `info` (audit) | 9-5 | `security.md` § Configuration mutations |
| `device_crud_rejected` | `warn` (audit) | 9-5 | `security.md` § Configuration mutations |
| `command_created` | `info` (audit) | 9-6 | `security.md` § Configuration mutations |
| `command_updated` | `info` (audit) | 9-6 | `security.md` § Configuration mutations |
| `command_deleted` | `info` (audit) | 9-6 | `security.md` § Configuration mutations |
| `command_crud_rejected` | `warn` (audit) | 9-6 | `security.md` § Configuration mutations |
| `address_space_mutation_succeeded` | `info` (diag) | 9-8 | `security.md` § Dynamic OPC UA address-space mutation |
| `address_space_mutation_failed` | `warn` (audit) | 9-8 | `security.md` § Dynamic OPC UA address-space mutation |
| `address_space_rename_failed` | `warn` (diag) | 9-8 (iter-2 IP1) | `security.md` § Dynamic OPC UA address-space mutation — Phase 4 demoted to warn-and-continue; failure-event surfaces silent rename errors without failing the apply |
| `topology_change_detected` | `info` (diag) | 9-7 (+9-8 fields) | `security.md` § Dynamic OPC UA address-space mutation |
| `opcgw_stale_read_callback_leak_observed` | `info` (diag, one-shot) | 9-8 | `security.md` § Dynamic OPC UA address-space mutation (Task 6 option-b limitation) |
| `opcgw_stale_write_callback_leak_observed` | `info` (diag, one-shot) | 9-8 | `security.md` § Dynamic OPC UA address-space mutation (Task 6 option-b limitation) |
| `inventory_query` | `info` (audit) | C-1 | `inventory-api.md` § Audit events — cache miss / refresh / bypassed inventory read |
| `inventory_query_failed` | `warn` (audit) | C-1 | `inventory-api.md` § Audit events — ChirpStack call failed |
| `inventory_cache_invalidated` | `info` (audit) | C-1 | `inventory-api.md` § Caching contract — CRUD write invalidated the cache |
| `inventory_observed_key_heterogeneous` | `warn` (audit) | C-1 | `inventory-api.md` § Audit events — wire-type fell back to String |
| `picker_opened` | `info` (audit) | C-2 | `inventory-api.md` § Picker-event audit endpoint — operator opened a picker (carries `picker_resource` + `cache_status`) |
| `picker_manual_fallback` | `info` (audit) | C-2 | `inventory-api.md` § Picker-event audit endpoint — picker flipped to manual entry (carries `picker_resource` + `reason`) |
| `picker_audit_rejected` | `warn` (audit) | C-2 | `inventory-api.md` § Picker-event audit endpoint — unknown-event or CSRF rejection |
| `metric_wire_type_inferred` | `info` (audit) | C-2 | `inventory-api.md` § `picker_metadata` field — per-metric inference recorded at create/update time (carries `inferred_type` + `operator_chosen_type` + `sample_values_count`) |
| `drift_view_opened` | `info` (audit) | C-4 | `web-api.md` § Story C-4 — operator opened `GET /api/inventory/drift`; carries `source_ip` + `chirpstack_reachable` + `summary_{ok,stale,available,drifted,total}`. Useful for "how often is the drift view consulted" analytics. |
| `drift_action` | `info` (audit) | C-4 | `web-api.md` § Story C-4 `POST /api/audit/drift-action` — operator clicked a drift action button. Fields: `action={remove\|update_name\|update_wire_type\|keep\|deep_link_add}`, `resource_type={application\|device\|metric}`, scope ids, `operator_choice`. The actual CRUD execution emits its own `<resource>_crud` event — this is the layer-above intent signal. |
| `drift_dismissed` | `info` (audit) | C-4 | `web-api.md` § Story C-4 `POST /api/audit/drift-action` — operator clicked `[Keep as alias]` / `[Keep opcgw alias]`. Documents the deliberate "I know about this divergence, I'm keeping it" choice for audit forensics. Fields: `class`, `resource_type`, scope ids, `drift_reason`. |
| `drift_audit_rejected` | `warn` (audit) | C-4 | `web-api.md` § Story C-4 — drift-action endpoint rejected the request (unknown event name OR CSRF Origin/Content-Type violation). Fields vary by reason: `reason="unknown_event"` carries `received_event`; `reason="csrf"` carries `path`, `method`, `origin`. |
| `inventory_drift_succeeded` | `info` (audit) | C-4 | Successful drift computation. Fields: `tenant_id`, `application_count`, `device_count`, `metric_count`, `duration_ms`. Sibling of `drift_view_opened`; fires only on the success path so dashboards can compute compute-cost percentiles. |
| `inventory_drift_unreachable` | `warn` (audit) | C-4 | ChirpStack fetch failed during drift computation. Fields: `stage={applications\|devices\|uplinks}`, `tenant_id`, optional `application_id` / `dev_eui`, `error`, `duration_ms`. The endpoint still returns HTTP 200 + the degraded response shape per AC#10. |

The CSRF middleware dispatches between `application_crud_rejected`,
`device_crud_rejected`, and `command_crud_rejected` by URL path
prefix (Story 9-5 path-aware dispatch + Story 9-6 literal-arm
completion — see `src/web/csrf.rs::csrf_event_resource_for_path`):
requests under `/api/applications/:application_id/devices/:device_id/commands*`
emit the `command_*` name; requests under
`/api/applications/:application_id/devices*` emit the `device_*`
name; everything else under `/api/applications*` emits the
`application_*` name. The catch-all `_ =>` arm remains as a
defensive future-proofing guard for any un-routed resource
(currently unreachable in normal operation).

**Story C-3 — `conflict_kind` sub-field of `reason="conflict"`:**
the `application_crud_rejected`, `device_crud_rejected`,
`command_crud_rejected`, and `config_reload_rejected` events carry
`reason="conflict"` for two semantically different conditions. C-3
keeps the `reason` value stable (existing grep contract) and adds a
`conflict_kind` sub-field to disambiguate:

| `conflict_kind=` | Meaning | Operator action |
|---|---|---|
| `duplicate` | The CRUD request — or a hot-reloaded TOML — attempted to introduce a same-level duplicate that the C-3 validator rejects (e.g. duplicate `application_id`, duplicate `device_id` within an application, duplicate `metric_name` / `chirpstack_metric_name` within a device, duplicate `command_id` / `command_name` within a device). HTTP body shape: `{ "error": "duplicate", "field": "...", "value": "...", "scope": "...", "hint": "..." }`. | Pick a different identifier (or DELETE the existing entry first); fix the TOML hand-edit. |
| `malformed_existing_block` | The on-disk TOML already contains a block whose shape doesn't match the schema (missing required field, wrong type, non-array-of-tables where one is expected). Pre-existing state corruption surfaced by a subsequent CRUD attempt. HTTP body keeps the pre-C-3 `ErrorResponse::with_hint` shape pointing at manual TOML cleanup. | Hand-edit `config/config.toml` to fix the malformed block, then retry. |

Audit consumers grepping `reason="conflict"` continue to work; consumers wanting to distinguish add the `conflict_kind` filter. Other existing `reason=` values on these events (e.g. `cascade_blocked`, `empty_application_list`) carry their own `conflict_kind` value that mirrors the reason for grep-uniformity.

**`error=` field format on C-3 emits** — `config_reload_failed`, `config_reload_rejected`, and the C-3 hot-reload-rejected branches of `application_crud_rejected` / `device_crud_rejected` / `command_crud_rejected` Debug-format the underlying error (the `error = ?e` field syntax in `tracing::warn!`), not Display-format. This was an iter-1 hardening against the "new audit-emit field is a new injection sink" finding-class — operator-controlled field values embedded in the validator's error message (e.g. a TOML multi-line string carrying `\n` or other control chars) are escaped by Debug-format before reaching the structured-log line, preventing log-line forgery. The on-the-wire shape is `error=Validation("config validation error: <msg>")` rather than `error=config validation error: <msg>`. Downstream log-grep pipelines that previously matched the Display form must shift to either (a) parsing `error=Validation(...)` for the inner string, or (b) using the new `duplicate_field=` / `duplicate_value=` sibling fields on `conflict_kind="duplicate"` events directly.

**`duplicate_field=` and `duplicate_value=` fields on C-3 duplicate-class emits** — `<resource>_crud_rejected reason="conflict" conflict_kind="duplicate"` events that originate from the post-write reload-time detection path (`src/web/api.rs::reload_error_response`, iter-2 BH-H1 fix) carry two extra fields: `duplicate_field` (the schema field name, e.g. `application_id`) and `duplicate_value` (the conflicting value, structurally extracted from the validator's known six message patterns). These mirror the HTTP body's `field` / `value` so an audit consumer doesn't need to re-parse the `error=` payload. Pre-flight emits (the common case) do not carry these fields — they carry resource-specific fields like `application_id=`, `device_id=`, `chirpstack_metric_name=` directly per the originating handler.

**Note (iter-2 review M1):** the `_crud_rejected` event family
fires on **any** path-shape rejection, regardless of HTTP method —
including GETs whose URL path-segment is malformed (CRLF, oversize,
invalid char). This is intentional: a rejected request is a rejected
request. The `_crud_rejected` family represents "request rejected by
the CRUD-surface validation", NOT "mutation rejected". Operators
filtering on `_crud_rejected` for security alerts will see a small
amount of GET-noise from typoed URLs; this is expected. The "GET 404
does not emit `_crud_rejected`" semantic (documented in
`src/web/api.rs::device_not_found_response`) applies specifically to
*resource-not-found* responses, not to *path-shape* rejections.

Pinning rules (apply to every entry above):

- The wire response to a client never depends on whether the audit
  event fired — failure modes that surface as `event="..._failed"`
  always return the same status code + headers regardless of which
  internal `reason` they recorded. The discrimination exists only
  in the audit log.
- `warn` is the audit-event minimum. Operators running at
  `error`/`off` lose the audit trail entirely (their explicit
  choice — Story 7-2 emits a one-shot `event="nfr12_correlation_check"`
  warn at startup if the resolved level filters out `info`, since
  source-IP correlation breaks at that level).
- `info` is the diagnostic-event minimum. They are not security
  signals; they exist so operators can confirm a startup landed
  cleanly without grepping multiple lines.

## Diagnosing common symptoms

A symptom-first cookbook for the most common production incidents:

### "OPC UA reads are returning Uncertain / Bad" — stale data

1. Find the `request_id` of one slow read from the console.
2. `grep "request_id=<uuid>" log/*.log` to see the full read trail.
3. The `staleness_check` line tells you the metric's age vs. threshold. If `metric_age_secs` is enormous, the **source** has stopped emitting.
4. Cross-reference with the matching `poll_cycle_end` and the device's `device_polled` line. If `device_polled` has `success=false`, the poller can't reach that device.

### "Console is silent but I expect logs" — log level too low

1. Check the `logging_init` line at startup — `level=…` shows the resolved level.
2. If `level=ERROR` and you expected debug detail, set `OPCGW_LOG_LEVEL=debug` and restart.
3. Per-module files (`chirpstack.log`, `storage.log`, …) are at TRACE regardless — open those for forensic detail.

### "Polls keep failing" — connectivity

1. Look for `chirpstack_outage` warns — the first one of a cycle reports `last_successful_poll` and the underlying error.
2. Walk back through `chirpstack_connect` warns to see the failure mode (timeout vs. connection refused).
3. If `error_spike` fires, multiple devices failed at once → upstream issue, not single-device.
4. Story 4-4 recovery loop fires automatically: look for `recovery_attempt` info lines (cycle entries) and `recovery_complete` (success — `downtime_secs` carries the outage duration) or `recovery_failed` (retry budget exhausted — manual investigation needed).

### "Reads are slow" — budget warnings

1. Filter the console for `exceeded_budget=true`. Each line names the operation (`opc_ua_read`, `storage_query`, `batch_write`) and the breached threshold.
2. `storage_query` exceeded → SQLite contention; check pool size and look for `SQLITE_BUSY` warns.
3. `batch_write` exceeded → disk I/O. Verify the database is on local SSD, not a network mount.
4. `opc_ua_read` exceeded but `storage_query` is fine → likely staleness computation overhead; rare.

### "I see `staleness_boundary` lines" — flickering metrics

A metric near the staleness threshold flips between Good and Uncertain on consecutive reads. Either raise `[opcua].stale_threshold_secs` (in `config.toml`) above the device's actual emit interval, or fix the device's emit cadence to be faster than the threshold.

## Related stories

- **Story 1-2** — initial migration from `log4rs` to `tracing`.
- **Story 6-1** — structured fields, correlation IDs, per-module appenders, configurable log directory.
- **Story 6-2** — configurable log level via `OPCGW_LOG_LEVEL` and `[logging].level` (this document).
- **Story 6-3** — microsecond-precision timestamps and remote diagnostics for known failures (this document, sections "Future operations" and the operations table).
- **Story 4-4** (implemented 2026-05-05) — auto-recovery from ChirpStack outages; defines `recovery_attempt` / `recovery_complete` / `recovery_failed` operations (catalogued above).
- **Story C-6** — TOML→SQLite configuration migration + SQLite-driven hot-reload. Removes the SIGHUP listener (and its `config_reload_attempted/succeeded/failed` events); adds `config_reload trigger="crud_write"`, `config_migration`, and `config_migration_failed` events.
- **Story D-0** — Singleton config → SQLite migration. Extends the `config_migration` event with new D-0 stages (`singleton_toml_to_sqlite`, `singleton_already_migrated`, `singleton_already_migrated_backfill_failed`, `skipped_placeholder_singleton`) and the `config_migration_failed` event with the new `singleton_row_count_mismatch` reason. Adds the `storage_init` event for SQLite file-permission diagnostics (AI-C-SEC-2 hardening).

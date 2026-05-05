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
| `chirpstack_outage` | `warn` | First per-device connectivity failure of a cycle that flips `chirpstack_available` to false. Carries `last_successful_poll`. | If `last_successful_poll` is far in the past, the recovery loop has been failing — check `recovery_failed` warns. |
| `recovery_attempt` | `info` | Story 4-4: a single attempt within the ChirpStack recovery loop. Carries `attempt`, `max_retries`, `delay_secs`, `last_error`. | Monitor frequency. A spike of `recovery_attempt` lines correlates with a `chirpstack_outage` warn — investigate ChirpStack server health. |
| `recovery_complete` | `info` | Story 4-4: recovery loop succeeded. Carries `attempts_used`, `downtime_secs` (or `from_startup=true` on cold start), `last_error`. | `downtime_secs` identifies outage duration; investigate ChirpStack-side root cause if persistent. |
| `recovery_failed` | `warn` | Story 4-4: recovery loop exhausted its retry budget without restoring connectivity. Carries `attempts_used`, `last_error`. | Manual intervention may be needed — check ChirpStack server status, network, credentials. |
| `chirpstack_request` | `warn` | gRPC request to ChirpStack returned an error. `exceeded=true` flags timeout (DeadlineExceeded); otherwise transient (Unavailable / Cancelled). | Repeated timeouts → upstream slow; repeated Unavailable → network partition. |
| `metric_parse` | `warn` | A metric value couldn't be coerced to its declared type (e.g. boolean got 0.5). Drop with `fallback_value=none`. | Verify the device firmware's emit format matches the configured metric type. |
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
- **Story 4-4** (planned) — auto-recovery from ChirpStack outages; will use the operations reserved above.

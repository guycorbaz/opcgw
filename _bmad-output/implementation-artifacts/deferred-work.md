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

## Deferred from: code review of Story 5-1 (2026-04-24)

- **Trait object vtable indirection cost vs <100ms latency goal** — Converting SqliteBackend to Arc<dyn StorageBackend> adds vtable overhead. This is pre-existing architecture choice (all subsystems use trait objects), not introduced by this story. Monitor in production to verify <100ms latency is maintained. If latency concerns arise, consider concrete Arc<SqliteBackend> for OPC UA reads.

- **Pool cloning efficiency (double wrapping in Arc)** — SqliteBackend::with_pool(pool.clone()) then wrapped in Arc::new(). Double wrapping may be inefficient. This is pre-existing pattern (not introduced by this story). Candidate for refactoring: evaluate whether single Arc::clone() suffices or if separate pool instances are needed per subsystem.


## Deferred from: code review of story-6.1 (2026-04-27)

- **Unbounded growth of `last_status` HashMap** [src/opc_ua.rs] — bounded by configured device×metric matrix today; revisit when Epic 9 introduces dynamic config reload (Blind+Edge).
- **AC#10 `cargo clippy --all-targets -- -D warnings` not clean project-wide** — 58 pre-existing dead-code/unused-import warnings on `main` HEAD. Open a separate cleanup story.
- **SQLITE_BUSY `warn!` not emitted at storage layer** [src/storage/sqlite.rs] — covered today by parent retry `warn!` in `chirpstack.rs::poll_metrics`. AC#6 wording demands storage-layer `warn!`.
- **`info_span!` + `let _enter = span.enter()` is fragile if function ever becomes async** [src/opc_ua.rs:get_value, get_health_value] — currently sync-only; rewrite to `span.in_scope(|| ...)` if any `.await` is added inside.
- **`batch_metrics.clone()` on every retry attempt** [src/chirpstack.rs:822-848] — pre-existing pattern; performance optimisation only.
- **AC#5 `errors` field name vs AC#7 canonical-list `error` (singular) inconsistency** — spec ambiguity; track in 6-2 spec review or open a docs issue.
- **`error_count == i32::MAX` only catches saturated state once before wrapping in release** [src/opc_ua.rs:get_health_value] — proper fix is `saturating_add` at the increment site, not a comparison change. Open follow-up issue scoped to gateway health-metric overflow handling.

## Deferred from: code review of story-6.2 (2026-04-27)

- **`prepare_log_dir` fallback returns `"./log"` even when `create_dir_all("./log")` fails** [src/main.rs:152-178] — pre-existing from Story 6-1. Both fallback branches use `let _ = std::fs::create_dir_all("./log")` and return `"./log"` regardless of success. Read-only FS, `./log` existing as a file, or permission denied would all leave the non-blocking writer to fail silently. Probe + nested fallback (e.g., `/tmp/opcgw-log`) needed.
- **Stale `#[allow(dead_code)]` on `LoggingConfig`** [src/config.rs:132] — struct now has runtime consumers via `peek_logging_config` and `AppConfig`. Blanket allow will mask future genuine dead fields. Remove or scope to specific fields.
- **Init-time stderr warnings (invalid env, invalid config, log-dir fallback) never reach log files** [src/main.rs:127-130, 137-141, 156-160, 172] — by design (tracing not yet initialised at that point). Fix is buffer-and-replay: collect warnings into a `Vec<String>` during the bootstrap phase, then replay them via `warn!` immediately after `.init()`. Deferred as a design follow-up; affects post-mortem from log files only.
- **No automated test asserts the format of the post-init `logging_init` info line** [src/main.rs:308-313] — `docs/logging.md` advertises this line as the operator-visible signal; only verified by smoke test in Dev Notes. Capturing requires a test-only `Layer` that records events; out of scope for 6-2.
- **Test gap: `[logging].level = "INFO"` (uppercase in TOML)** [src/config.rs] — implementation lowercases internally so it works, but the contract isn't pinned by a test. One-line addition to existing config tests.
- **Test gap: `[logging]` block with `dir` only and no `level`** [src/config.rs] — no test exercises the `LoggingConfig { dir: Some(_), level: None }` path. Common operator config; deserves coverage.

## Deferred from: code review of story-6-3-remote-diagnostics-for-known-failures (2026-04-27)

- **`error_delta` oscillation re-fires spike warn** [src/chirpstack.rs:996-1004] — no hysteresis or debounce; oscillating error counts (0→6→0→6) re-trigger the warn every odd cycle. Design enhancement, not a bug.
- **`last_status` cache grows unboundedly** [src/opc_ua.rs:1462,1631-1641] — no TTL/LRU; long-running gateways with rotating device IDs accumulate map entries. Bounded eviction enhancement (also flagged in 6-1 review under a similar formulation).
- **`gateway_status_init` is per-process not per-instance** [src/opc_ua.rs:1456-1459] — spec authorizes process-wide latching; document the limitation in `docs/logging.md` so operators understand test/restart semantics.
- **NaN/Inf boolean parse falls into "invalid boolean" branch** [src/chirpstack.rs:1175-1200] — message technically correct; cosmetic refinement only.
- **`peek_logging_config` swallows TOML parse errors silently** [src/main.rs:745-752] — 6-2 carryover; downstream `AppConfig::from_path` does surface the error.
- **`prepare_log_dir` falls back to `./log` even when `create_dir_all("./log")` itself fails** [src/main.rs:853-879] — 6-2 carryover (already in 6-2 deferred list above).
- **`Channel::connect()` has no explicit timeout** [src/chirpstack.rs:317-365] — pre-existing infrastructure; out of 6-3's instrumentation-only scope. Story 4-4 territory.
- **`chirpstack_outage` reads `last_successful_poll` after the cycle has potentially updated it** [src/chirpstack.rs:892-901,1010-1015] — cycle-local consistency drift; minor.
- **`log_dir` mismatch warning compares strings without canonicalisation** [src/main.rs:376-395] — 6-2 carryover; produces false-positive "restart to apply" when paths are equivalent but not byte-identical.
- **`parse_log_level` eprintln may echo ANSI escape sequences from env** [src/main.rs:782-792] — 6-2 carryover; terminal injection via crafted `OPCGW_LOG_LEVEL`.
- **`NonBlocking` guards drop ordering** [src/main.rs:248-310] — 6-1 carryover; tracing-appender contract requires guards live to end of `main`.
- **Span re-entrancy in `OpcUa::get_value` via `add_read_callback`** [src/opc_ua.rs:608-625] — pre-existing; no recursion in current code, but `let _enter = span.enter()` is fragile if `get_value` ever becomes async (already in 6-1 deferred list).
- **`ChronoUtc` non-monotonic across NTP step-backward** [src/main.rs:307-320] — system-level; document as known limitation. Augment with monotonic counter only if grep-by-timestamp ordering becomes a real operator pain point.
- **Other tonic codes (`Unauthenticated`, `ResourceExhausted`) not classified** [src/chirpstack.rs:1708-1755] — follow-up enhancement; not on 6-3's instrumentation path.
- **`rollback_err` not classified as SQLITE_BUSY** [src/storage/sqlite.rs:2882-2898] — cascading busy on rollback path silently swallowed; minor.
- **`STORAGE_QUERY_BUDGET_MS` excludes commit/rollback paths** [src/storage/sqlite.rs:62-93] — slow commits never surface as `exceeded_budget=true`. Wrap commit/rollback in `StorageOpLog` or compare elapsed against the budget there.
- **Far-future `metric.timestamp` clock-skew handling** [src/opc_ua.rs:893-983] — current code treats negative ages as fresh, hiding the anomaly. Rare but worth a `clock_skew_extreme` debug for diagnosability.
- **`extract_request_ids` cursor advance after closing quote** [src/opc_ua.rs:2003-2032] — works for current emit format; brittle if quoting changes.
- **`microsecond_timestamp_format_matches_pattern` test missing monotonicity assertion** [src/main.rs:1378-1413] — AC#2 is about ordering, not just digit count. Add a monotonic-pair assertion alongside the regex.

## Deferred from: implementation of story-7-1-credential-management-via-environment-variables (2026-04-28)

- **`tenant_id` redaction in `Debug` impl** [src/config.rs::ChirpstackPollerConfig] — out of scope per epic spec (Story 7-1, AC#3 matrix); only `api_token` and `user_password` are classified as secrets. Story 7-1 substitutes the all-zeros placeholder UUID in the shipped `config/config.toml` template (so the operator's tenant identity isn't published) but does not redact the value at log time. Tracked at GitHub issue #83.
- **tonic / tower-http metadata redaction strategy** [src/chirpstack.rs::AuthInterceptor] — Story 7-1 AC#5 audit found tonic 0.14.5 does not log request metadata and opcgw has no `tower-http` / `TraceLayer` wiring, so no `EnvFilter` mitigation is needed today. The proactive mitigation (a `tower::Layer` that strips the `authorization` header before logging, so future `TraceLayer` additions are safe-by-default) is deferred. Tracked at GitHub issue #82.
- **Operator migration shim `scripts/migrate-config-7-1.sh`** — considered for the `git pull` / merge-conflict path on operators' local `config/config.toml`. Documented manually in `docs/security.md` "Migration path" instead; helper is deferred unless adoption signals demand it.
- **`secrecy::SecretString` newtype for `api_token` / `user_password`** [src/config.rs] — Story 7-1 Out of Scope. The redacting `Debug` impl achieves ~95% of the protection at ~5% of the diff cost. Adopting `secrecy` would add `.expose_secret()` calls at every consumer in `src/chirpstack.rs` and `src/opc_ua.rs`; defer unless a follow-up story requires zeroize-on-drop guarantees.

## Deferred from: code review of story-7-1-credential-management-via-environment-variables (2026-04-28)

- **Manual `Debug` impls have no compile-time pin against future field-add omissions** [src/config.rs:259-308] — `ChirpstackPollerConfig::Debug` and `OpcUaConfig::Debug` enumerate every field by hand. A future contributor adding a new (potentially secret) field will not get a compile-time warning; they must remember to update the AC#3 matrix. Procedural-macro or serde-roundtrip-driven test would close the gap at the cost of a larger refactor. AC#3 matrix is the explicit contract for now.
- **`tenant_id` all-zeros placeholder UUID bypasses startup validation** [src/config.rs::AppConfig::validate, config/config.toml:55] — the shipped `tenant_id = "00000000-0000-0000-0000-000000000000"` is a valid UUID format and passes `validate()`, so an operator who forgets to override it boots the gateway and only sees the failure on the first gRPC call. A 3-line check rejecting the all-zeros UUID with an actionable message would close the gap; deferred per Story 7-1 AC#3 matrix and tracked at GitHub issue #83.
- **Validation-error prefix duplicated between code constant and docs** [src/utils.rs::PLACEHOLDER_PREFIX, docs/security.md] — code uses the centralised constant, but `docs/security.md` hardcodes the literal `"REPLACE_ME_WITH_"`. If the constant ever changes, docs go stale. Cosmetic doc-rot risk; not a code defect.

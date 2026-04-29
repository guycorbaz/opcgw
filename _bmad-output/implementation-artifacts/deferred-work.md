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

## Deferred from: Story 7-2 (OPC UA Security Endpoints and Authentication, 2026-04-28)

- **Multi-user OPC UA token model** — Story 7-2 keeps the single-user model already in place (`OPCUA_USER_TOKEN_ID = "default-user"`). The OPC UA spec supports multiple `ServerUserToken` records with independent credentials and permissions. Out of Scope per Story 7-2; tracked at GitHub issue #85. Pick up when role-based access control becomes a requirement.
- **Rate-limiting OPC UA failed auth attempts** — Story 7-2 emits `warn!` events for every failed authentication so operators can spot brute-force probing in logs, but does not throttle. Out of Scope per Story 7-2; tracked at GitHub issue #86. Pick up when network-layer protection (firewall / fail2ban) is judged insufficient.
- **mTLS / X.509 user-token authentication** — Story 7-2 enforces username/password (FR20) only. The `ServerUserToken { x509: None, thumbprint: None, … }` shape stays. Out of Scope; revisit when an SCADA client requires certificate-based auth.
- **CA-signed certificate workflow integration** — Story 7-2 documents manual `openssl req` setup; an automated CA-signing pipeline integration is deferred. Operators with a real CA hierarchy can drop CA-signed certs into `pki/own/` manually today.
- **First-class source-IP in OPC UA auth audit log** — async-opcua 0.17.1's `AuthManager` does not receive the peer's `SocketAddr`; NFR12 is satisfied via two-event correlation (accept event + auth-failed event). File an upstream feature request to extend `AuthManager` with peer-addr; revisit when async-opcua releases such a hook.
- **Replace `String` `user_password` with `secrecy::SecretString`** — Story 7-1 deferred this on the rationale that the redacting `Debug` impls already cover NFR7. Same rationale holds in Story 7-2. Revisit if a future story introduces a third field whose Debug exposure is harder to audit.

## Deferred from: code review of story-7-2-opc-ua-security-endpoints-and-authentication (2026-04-28)

- **Endpoint path logged unsanitised in `OpcgwAuthManager`** [`src/opc_ua_auth.rs:101,107,115`] — defensive coding for future-proof; today only registered endpoint names (`null`, `Basic256/Sign`, `Basic256/SignAndEncrypt`) reach this field, so log-injection via endpoint is impossible in practice. Apply `sanitise_user`-style escaping to `endpoint.path` if/when dynamic endpoint registration becomes possible.
- **`tracing_test::internal::global_buf()` is a private API** [`tests/opc_ua_security_endpoints.rs:48`] — pre-disclosed deviation in Dev Agent Record; required because `traced_test`'s scope filter is incompatible with async-opcua's spawned-task spans. Pin `tracing-test = "=0.2.6"` exact-version or write a custom subscriber-layer if a patch release breaks the build.
- **TOCTOU between `validate_private_key_permissions` (startup) and async-opcua's runtime read** [`src/security.rs`] — relies on `<pki_dir>/private/` being `0o700` (which `ensure_pki_directories` does enforce). Re-stat-and-verify immediately before key open would close the race; not justified for current LAN threat model.
- **NFC vs NFD username normalisation** [`src/opc_ua_auth.rs:103`] — usability concern; configured user `"café"` (NFC) rejects client `"café"` (NFD). ASCII-only username convention is the workaround today.
- **`pki_dir` symlink-followed silently** [`src/security.rs:135-214`] — niche shared-host attacker threat; `O_NOFOLLOW`-style defence is non-trivial in stable Rust. Revisit if multi-tenant deployment becomes a use case.
- **`set_mode` discards setuid/setgid/sticky bits** [`src/security.rs:88,168`] — the `mode & 0o777` mask preserves only the basic perm triplet. Operators using `g+s` for group inheritance on `pki/private/` lose it after `ensure_pki_directories`. Preserve high bits with `(actual_mode & 0o7000) | expected_mode` in a follow-up.
- **`pick_free_port` race window in integration tests** [`tests/opc_ua_security_endpoints.rs:101-106`] — TOCTOU between listener-drop and OPC UA bind. Spec already recommends `serial_test` if flakes appear; `setup_test_server` doesn't yet hit them on dev hardware.
- **`ServerUserToken` keeps a duplicate plaintext password alongside `OpcgwAuthManager`** [`src/opc_ua.rs:1547-1548`] — async-opcua's `ServerConfig::validate` requires the entry; the `pass` field is decorative since `OpcgwAuthManager` is the gatekeeper. Pass `None` if a future async-opcua release allows it. Memory footprint is one extra clone of an already-redacted secret — minor.
- **`TestServer::Drop` race with `TempDir`** [`tests/opc_ua_security_endpoints.rs:2462-2467`] — `handle.abort()` is non-blocking; async-opcua may still be writing when `TempDir::Drop` deletes the dir. Cosmetic stderr noise on test panic; cleanup is automatic on next CI tmpdir sweep.
- **AC#2 ships only `null`-endpoint wrong-password sub-test (deviation accepted)** [`tests/opc_ua_security_endpoints.rs:402`] — spec required three named sub-tests (`_null`, `_basic256_sign`, `_basic256_sign_encrypt`); only `_null` shipped. **Accepted as deferred:** the auth path in `OpcgwAuthManager` is endpoint-agnostic (the manager does not see channel security, only `endpoint`/`username`/`password`); Basic256 client-side PKI handshake from the test harness adds significant brittleness without adding auth-rejection coverage. The Basic256 endpoints are still pinned by AC#1's discovery-based shape test.
- **Trojan-source Unicode (RTL overrides, zero-width joiners) in usernames not sanitised** [`src/opc_ua_auth.rs:76-78`] — `escape_default()` only escapes ASCII control chars + backslash + quote. Code points like `U+202E` (RIGHT-TO-LEFT OVERRIDE), `U+200B` (zero-width space), `U+2066/2067` (bidi isolates) pass through and can deceive RTL-aware log viewers. **Accepted as deferred:** LAN threat model + plain-text log readers (grep, tail) make the practical impact low; introducing `unicode-security` crate dependency is over-engineering for current deployment. Revisit when log analysis tooling expands or any client traffic crosses an untrusted boundary.
- **`create_sample_keypair=true` regen produces world-readable file for one boot cycle** [`src/security.rs:84-90`] — when the configured key file is missing and `create_sample_keypair=true`, validation short-circuits `Ok(())` and async-opcua regenerates the keypair with default umask (typically `0o644`). The next-restart validation catches it, but the gateway runs once with a world-readable key. **Accepted as deferred** — the runtime fix (post-create chmod or re-validation hook) is non-trivial and the documented anti-pattern note in `docs/security.md` is the operator-facing mitigation. Production deployments must run with `create_sample_keypair=false`; the boot-once-world-readable window only affects development workflows.
- **HMAC key not zeroized on drop** [`src/opc_ua_auth.rs::OpcgwAuthManager::hmac_key`] — pass-3 review finding E1: the per-process HMAC keying material lingers in heap memory until the allocator reuses the page. A memory-dump attacker who captures both the key and the digests can precompute candidate credentials offline. Defence is to add `zeroize` crate and wrap `hmac_key` in `Zeroizing<[u8; 32]>` (~5 lines + new dep). LAN threat model and the difficulty of obtaining process-memory access make this strategically marginal; revisit if the gateway is deployed in untrusted multi-tenant environments.
- **`getrandom` version not exact-pinned** [`Cargo.toml::getrandom = "0.2"`] — pass-3 review finding E4: the `^0.2` range allows minor version drift. `Cargo.lock` provides reproducibility today; if a future patch release introduces a regression on Linux x86-64 (the gateway's target), the lockfile keeps the build stable until intentional update. Pinning would be `getrandom = "=0.2.x"` once a known-good version is chosen.

## Deferred from: Story 7-3 (OPC UA Connection Limiting, 2026-04-29)

- **Per-source-IP rate limiting / token-bucket throttling** — Story 7-3 ships a flat global cap (`max_sessions`) per FR44. A single misbehaving SCADA client can saturate the cap on its own and starve other operators; a distributed flood is also unaddressed. Out of Scope per Story 7-3; tracked at GitHub issue #88. Pick up alongside the Story 7-2 follow-up (#86, "rate-limiting failed auth attempts") since both share the per-IP state machine.
- **Surface async-opcua subscription / message-size limits as config knobs** — `max_subscriptions_per_session`, `max_monitored_items_per_subscription`, `max_message_size`, `max_chunk_count`, etc. all default to async-opcua's library defaults. Out of Scope per Story 7-3; tracked at GitHub issue #89. Pick up when Epic 8 (subscriptions) lands.
- **Hot-reload of session cap** — `max_connections` is read at startup only; async-opcua's `Limits::max_sessions` is fixed for the server's lifetime via `ServerBuilder`. Out of Scope per Story 7-3; tracked at GitHub issue #90. Pick up alongside Epic 9 story 9-7 (configuration hot-reload). May also require an upstream feature request to async-opcua for runtime-mutable `Limits`.
- **First-class session-rejected event in async-opcua** — `SessionManager::create_session` rejects (N+1)th sessions with `Err(BadTooManySessions)` but does not log. Story 7-3 works around this via a tracing-Layer correlated against the library's pre-existing `Accept new connection from {addr}` event (NFR12-style two-event pattern). File an upstream feature request to extend `SessionManager` with a rejection callback or log emission; revisit when async-opcua ships such a hook. Same shape as the Story 7-2 deferred entry "First-class source-IP in OPC UA auth audit log".

## Deferred from: code review of 7-3-connection-limiting (2026-04-29)

- **`MessageVisitor` brittle to async-opcua emission style** [src/opc_ua_session_monitor.rs:142-153] — Visitor matches `field.name() == "message"` and uses `record_debug` to format. If a future async-opcua release adds a structured `message = "..."` field on the same target, or changes the format-string emission to use `record_str` with quote-wrapping, the at-limit warn silently breaks (the `starts_with("Accept new connection from ")` guard fails). Works today; revisit on async-opcua upgrade.
- **At-limit warn rate-limiting under TCP-accept floods** [src/opc_ua_session_monitor.rs:182-196] — When the cap is hit, every TCP accept produces one warn line, including port scans, healthchecks, and partial-handshake probes that never request a session. Spec already documents this trade-off (`docs/security.md` "Expected at-limit log noise"), but no in-process throttle / sampling exists. Subsumed by per-source-IP follow-up at GitHub issue #88; revisit when implementing per-IP token bucket.
- **`OPCUA_SESSION_GAUGE_INTERVAL_SECS` not tunable via env or config** [src/utils.rs] — Hard-coded compile-time constant. Operators wanting a different gauge cadence (e.g., 1s for short bursts, 60s for low-noise production) must rebuild. Spec did not require tunability. File a follow-up issue when the first operator asks; otherwise live with the compile-time default.
- **At-limit warn message wording: "will be rejected" misleading for partial-handshake peers** [src/opc_ua_session_monitor.rs:182-196] — When `current == limit`, the warn fires on the (N+1)th TCP accept BEFORE async-opcua's `CreateSession` rejection. If the peer disconnects mid-handshake (port scan, half-open connection), it never gets rejected — the warn's "will be rejected" phrasing is inaccurate for that case. Cosmetic; revisit when refining log copy or migrating to a true rejection callback (see "First-class session-rejected event in async-opcua" above).
- **Test count claim of 574 plausible but not directly re-verified during code review** — Completion Notes line 566 reports 574 pass / 0 fail / 7 ignored after the implementation run; reviewer counted the 12 net-new tests individually but did not re-execute the suite. Re-run `cargo test --lib --bins --tests` as part of the pre-commit verification before flipping the story to `done`.
- **Spec body still asserts `source_ip="127.0.0.1` (quoted) while integration tests use unquoted form** [_bmad-output/implementation-artifacts/7-3-connection-limiting.md:262] — Debug Log References (line 559) acknowledges the spec was wrong and the actual format is `source_ip=127.0.0.1` (no quote, since `tracing` formats `%val` via Display). Tests assert the corrected form. Spec body itself was not updated. Pure spec-hygiene, no code change required.

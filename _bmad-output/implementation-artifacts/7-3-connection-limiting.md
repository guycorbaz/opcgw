# Story 7.3: Connection Limiting

**Epic:** 7 (Security Hardening)
**Phase:** Phase A
**Status:** done
**Created:** 2026-04-29
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md` was authored before Phase A was renumbered. The story this file implements lives in `epics.md` as **"Story 6.3: Connection Limiting"** under **"Epic 6: Security Hardening"** (lines 653–667). In `sprint-status.yaml` and the rest of the project this is **Story 7-3** under **Epic 7**. Same work, different numbering.

---

## User Story

As an **operator**,
I want OPC UA client connections limited to a configurable maximum,
So that the gateway is protected from resource exhaustion by too many concurrent clients.

---

## Objective

Cap the number of concurrent OPC UA sessions the gateway will host so a misbehaving SCADA client (runaway reconnect loop, leaked sessions, deliberate flood) cannot exhaust file descriptors, memory, or CPU. Closes **FR44** and the **OT Security / Connection rate limiting** PRD line item (`prd.md:276`).

This is a **small, surgical hardening story** — async-opcua 0.17.1 already provides the enforcement primitive (`ServerBuilder::max_sessions(N)` + `BadTooManySessions` rejection inside `SessionManager::create_session`); the work is:

1. Plumb a `max_connections` knob through `OpcUaConfig` → `ServerBuilder::max_sessions(N)`.
2. Default to **10**, validate the bounds, expose via env var (consistent with Story 7-1's env-var-first posture).
3. Add operator-facing **logging** for the limit-hit case. Because the rejection happens deep in async-opcua's `SessionManager::create_session` with no log emission and no source-IP wiring (the trait shape is `Result<_, StatusCode>`), we use the **same two-event correlation pattern as Story 7-2**: a periodic gauge `info!(event="opcua_session_count", current, limit)` task + a `warn!(event="opcua_session_count_at_limit", source_ip)` event triggered when a fresh accept arrives while at-limit. Source IP comes from the `info!("Accept new connection from {addr} (N)")` line async-opcua already emits at `server.rs:367`.
4. Tests pin the contract: configured limit honoured, (N+1)th session rejected with `BadTooManySessions`, existing N sessions unaffected, warn event emitted on the at-limit reject, no regression on Story 7-1 / 7-2 baselines.
5. Documentation: extend `docs/security.md` with a "Connection limiting" subsection (matrix entry + grep recipe + sizing guidance), bump `README.md` Configuration block, sync the `README.md` Planning table.

The new code surface is small (~120-180 LOC of production code + ~150 LOC of tests + ~80 LOC of docs).

---

## Out of Scope

- **Per-source-IP rate limiting / token-bucket throttling.** A flat global cap is what FR44 specifies. Per-IP throttling is a separate (much larger) story — track as a deferred follow-up. Brute-force probing is an authentication concern (Story 7-2 deferred a "rate-limiting failed auth attempts" issue; that follow-up subsumes the per-IP angle).
- **Per-endpoint or per-user session caps.** The single global `max_sessions` value is the contract. Differentiated quotas (e.g. "5 SignAndEncrypt + 5 None") are not in scope.
- **Reverse-connect / discovery-server-registration limits.** The gateway does not register with a discovery server (`discovery-server-registration` cargo feature is off by default in async-opcua 0.17.1). Reverse connect is similarly not enabled. If either is enabled later, revisit the cap semantics.
- **Modifying async-opcua's enforcement code path.** We are not patching the upstream library to add per-IP gating, hot-reload of `max_sessions`, or auth-time rejection. Use the `ServerBuilder::max_sessions(N)` API as shipped.
- **Hot-reload of the limit at runtime.** Like all other `OpcUaConfig` fields, `max_connections` is read at startup. Phase B (Epic 9 hot-reload) covers runtime reconfiguration.
- **Web UI exposure of the cap.** Phase B.
- **Subscription / monitored-item / message-size limits.** async-opcua exposes additional builder knobs (`max_subscriptions_per_session`, `max_message_size`, etc.) that all default to reasonable values. We do not surface them as config in this story; revisit if a real Phase A workload hits them.
- **TCP-layer rejection (i.e. refusing the TCP accept itself).** async-opcua always accepts the TCP socket and runs the hello / secure-channel handshake before checking `max_sessions` at `CreateSession`. We could enforce earlier via a custom listener wrapper but it adds complexity for no FR44 benefit — the configured limit is on **sessions**, not raw TCP connections, and CreateSession is the first wire-level point where "is this a real OPC UA peer?" is known.

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. The story's job is to plumb a new knob through code that already does the heavy lifting.

| What | Where | Status |
|------|-------|--------|
| `ServerBuilder::max_sessions(usize)` builder method | `~/.cargo/registry/src/index.crates.io-*/async-opcua-server-0.17.1/src/builder.rs:523-526` | **API audited 2026-04-29.** Sets `config.limits.max_sessions`. Accepts `usize`. No min/max bounds enforced by the library; library default is `MAX_SESSIONS = 20` (`async-opcua-server-0.17.1/src/lib.rs:128`). |
| `SessionManager::create_session` rejection | `async-opcua-server-0.17.1/src/session/manager.rs:64-72` | **Wired today.** The check is `if self.sessions.len() >= self.info.config.limits.max_sessions { return Err(StatusCode::BadTooManySessions); }`. No log emission, no source-IP wiring. The error becomes a `ServiceFault { BadTooManySessions }` on the wire (`session/controller.rs:339-341, 565-596`). |
| `Limits::max_sessions` field | `async-opcua-server-0.17.1/src/config/limits.rs:43-44, 62` | Field that backs the builder. Library default `MAX_SESSIONS = 20`. |
| `ServerInfo::diagnostics.summary.current_session_count` | `async-opcua-server-0.17.1/src/diagnostics/server.rs:28-32, 113-114, 159` + `info.rs:92-93, 614-616` | **Live counter we can read.** `SessionManager` increments on session creation (`session/manager.rs:163-169`) and decrements on close (`219-224`, `278-285`). Exposed via `ServerHandle::info().diagnostics.summary` — the value is `LocalValue<u32>` whose `.sample()` returns a `DataValue` and whose internal value is readable via `get_with_time()`. |
| `ServerHandle` accessors | `async-opcua-server-0.17.1/src/server_handle.rs:62, 91` | **Wired today.** `handle.info()` returns `&Arc<ServerInfo>`, `handle.session_manager()` returns `&RwLock<SessionManager>` (the latter has only `find_by_token` as a public method; the count comes via `info().diagnostics.summary`). |
| TCP-accept event (carries source IP per connection) | `async-opcua-server-0.17.1/src/server.rs:367` (`info!("Accept new connection from {addr} ({connection_counter})")`) | **Library-emitted.** Always fires on every TCP accept — including connections that will later be rejected at CreateSession. This is the source-IP signal we correlate against. Same pattern Story 7-2 used for NFR12. |
| `OpcUaConfig` struct + `Debug` redaction | `src/config.rs:148-249, 274-295` | **Wired today.** Adding a new field requires: (a) the field on the struct, (b) the Debug impl listing it (or the Debug impl's coverage matrix breaks Story 7-1 NFR7 invariant), (c) optional validation in `AppConfig::validate`. |
| `AppConfig::validate` accumulator pattern | `src/config.rs:725-` | **Wired today.** Validation errors accumulate into a `Vec<String>` and `validate()` returns one combined `OpcGwError::Configuration` listing every violation. New checks append `errors.push(...)`. |
| Env-var override convention | `figment` + `Env::prefixed("OPCGW_").split("__")` (Story 7-1) | `OPCGW_OPCUA__MAX_CONNECTIONS=20` overrides `[opcua].max_connections` automatically — figment maps the `__`-split path. No code change required. |
| `OpcUa::create_server` integration point | `src/opc_ua.rs:168-238` | **Wired today.** `ServerBuilder` is built up across `configure_network` / `configure_key` / `configure_user_token` / `with_authenticator` / `configure_end_points`. Add a new step or extend `configure_network` to plumb `max_sessions`. Per Story 7-2's pattern, prefer a tiny new method `configure_limits` to keep responsibilities single-purpose. |
| Tracing event-name convention | Stories 6-1 (`comprehensive-logging-infrastructure`), 7-2 (`opcua_auth_failed`, `pki_dir_initialised`) | **Established.** Audit / state-change events use `event = "snake_case_name"` as a structured field. Story 7-3 adds `event = "opcua_session_count_at_limit"` and `event = "opcua_session_count"` (gauge). |
| `tests/opc_ua_security_endpoints.rs` test harness | `tests/opc_ua_security_endpoints.rs:90-353` | **Reusable today.** `pick_free_port`, `setup_test_server`, `build_client`, `try_connect_none`, `captured_logs_contain_anywhere`, `captured_log_line_contains_all`, plus the `TestServer` RAII handle. Story 7-3's integration test reuses this harness — see Testing Standards. |
| `OpcgwAuthManager::user_token_policies` returns `UserName` | `src/opc_ua_auth.rs` (Story 7-2) | The integration tests must use `IdentityToken::UserName(...)` to activate sessions — anonymous identity returns `BadIdentityTokenRejected` from the AuthManager's trait defaults. |
| `OpcGwError::Configuration` / `OpcGwError::OpcUa` variants | `src/utils.rs::OpcGwError` | Use `Configuration` for startup validation failures (out-of-range `max_connections`); use `OpcUa` for runtime server errors. |

**Epic-spec coverage map** — the BDD acceptance criteria from `epics.md` (lines 659-667) break down as:

| Epic-spec criterion | Already satisfied by async-opcua? | Where this story addresses it |
|---|---|---|
| New connections rejected with appropriate OPC UA status code at the cap | ✅ `BadTooManySessions` from `SessionManager::create_session` | **AC#2** integration test pins it. Plumbing AC#1 wires the limit through the builder so it actually fires. |
| Maximum configurable in `config.toml` (default 10) | ❌ no knob today (library default is 20) | **AC#1** adds `max_connections: Option<usize>` to `OpcUaConfig`, defaults to 10, env-var override `OPCGW_OPCUA__MAX_CONNECTIONS`. |
| Rejected connections logged at warn level with source IP | ❌ async-opcua does not log on this path | **AC#3** custom session-count gauge task + at-limit `warn!` event emitted on TCP accept while at the cap. Source IP comes from async-opcua's pre-existing `info!("Accept new connection from {addr}")` event — two-event correlation, identical to Story 7-2's NFR12 pattern. |
| Existing connected clients are not affected by the limit | ✅ async-opcua's check is `len >= max`; existing sessions are not disturbed | **AC#2** integration test pins this — opens N concurrent sessions, asserts the (N+1)th is rejected, then asserts the original N still serve `Read` requests. |
| FR44 satisfied | ⚠️ partial — knob missing | **AC#1-#5** close the gap. |
| Bounds / ergonomics | not specified | **AC#1** rejects `max_connections = 0` (would brick the server) and rejects values above a sane upper bound (4096 — see rationale in Dev Notes). |
| Documentation | not specified | **AC#5** extends `docs/security.md` and the README. |
| Tests + clippy clean | implicit per CLAUDE.md | **AC#4** preserves the 554-test / clippy-clean baseline from Story 7-2 and adds ≥6 new tests. |

---

## Acceptance Criteria

### AC#1: `max_connections` is configurable via `config.toml` and env var, defaulting to 10 (FR44)

- Add a new field `pub max_connections: Option<usize>` to `OpcUaConfig` in `src/config.rs:148-249`. **Use `Option<usize>` (not `usize`)** to mirror the existing `hello_timeout` / `host_port` / `host_ip_address` shape — `None` means "use the gateway default" (10), explicit `Some(n)` overrides. Document the field with a doc-comment matching the project's existing tone (one-paragraph purpose + range note + env-var override note).
- Update the hand-written `impl Debug for OpcUaConfig` (`src/config.rs:274-295`) to include the new field. **This is mandatory** — if the field is added to the struct but missing from the Debug impl, Story 7-1's NFR7 redaction matrix is silently broken (any new field added without touching Debug becomes invisible in `format!("{:?}", config)`).
- Add a `pub const OPCUA_DEFAULT_MAX_CONNECTIONS: usize = 10;` to `src/utils.rs` next to the existing `OPCUA_DEFAULT_*` constants. The `10` literal lives **only here** — `opc_ua.rs` and the validate path read from this constant. Default-grepping `10` across the codebase is brittle; one named symbol is the single source of truth.
- Add an upper-bound constant `pub const OPCUA_MAX_CONNECTIONS_HARD_CAP: usize = 4096;` to `src/utils.rs`. Rationale lives in Dev Notes ("Why 4096"). The hard cap is enforced in `validate()` (next bullet); values above it produce a clean configuration error rather than a panic deeper in async-opcua or an OS file-descriptor exhaustion.
- Extend `AppConfig::validate` (`src/config.rs:725-`) with two new checks added next to the existing OpcUa-section validations (after the `host_port == Some(0)` check at line 784):
  - If `self.opcua.max_connections == Some(0)` → push `"opcua.max_connections: must be at least 1 (use a small positive integer like 1 to enforce single-client mode; 0 would refuse all clients including operators)".to_string()`.
  - If `let Some(n) = self.opcua.max_connections` and `n > OPCUA_MAX_CONNECTIONS_HARD_CAP` → push `format!("opcua.max_connections: {} exceeds hard cap of {}. Either lower the value or open a follow-up if your deployment really needs more (the cap protects against fd-exhaustion DoS)", n, OPCUA_MAX_CONNECTIONS_HARD_CAP)`.
- Update the shipped `config/config.toml` with a commented-out default block:
  ```toml
  # Maximum number of concurrent OPC UA client sessions. New connections
  # beyond this limit are rejected with the OPC UA status code
  # BadTooManySessions. Existing sessions are unaffected.
  #
  # Default: 10. Range: 1 to 4096.
  # Override via env var: OPCGW_OPCUA__MAX_CONNECTIONS
  #max_connections = 10
  ```
  Place it after `stale_threshold_seconds` (the last existing OPC UA field).
- The existing `config/config.example.toml` (multi-app reference) gets the same commented-out block.
- **Verification:**
  - `grep -n 'max_connections' src/config.rs` returns ≥3 hits (struct field + Debug impl + 2 validate checks).
  - `grep -nE 'OPCUA_DEFAULT_MAX_CONNECTIONS|OPCUA_MAX_CONNECTIONS_HARD_CAP' src/utils.rs src/opc_ua.rs src/config.rs` returns ≥4 hits across the three files.
  - `grep -nE 'max_connections' config/config.toml config/config.example.toml` returns ≥1 hit per file.
  - Unit test `test_validation_rejects_max_connections_zero` — `config.opcua.max_connections = Some(0)`, assert `validate()` returns `Err` containing `"max_connections"` and `"at least 1"`.
  - Unit test `test_validation_rejects_max_connections_above_hard_cap` — `Some(4097)`, assert `Err` containing `"max_connections"` and `"hard cap"` and `"4096"`.
  - Unit test `test_validation_accepts_max_connections_at_hard_cap` — `Some(4096)`, assert `Ok`.
  - Unit test `test_validation_accepts_max_connections_none` — `None`, assert `Ok` (default applied at use site, not at validation).
  - Unit test `test_validation_accepts_max_connections_one` — `Some(1)`, assert `Ok` (single-client mode is a legitimate config).

### AC#2: `ServerBuilder::max_sessions(N)` is wired and enforced (FR44)

- Add a new method `fn configure_limits(&self, server_builder: ServerBuilder) -> ServerBuilder` to `OpcUa` in `src/opc_ua.rs`. Place it next to `configure_network` for symmetry. Body:
  ```rust
  fn configure_limits(&self, server_builder: ServerBuilder) -> ServerBuilder {
      let max = self
          .config
          .opcua
          .max_connections
          .unwrap_or(crate::utils::OPCUA_DEFAULT_MAX_CONNECTIONS);
      debug!(max_sessions = %max, "Configure session limit");
      server_builder.max_sessions(max)
  }
  ```
- Wire it into `create_server` (`src/opc_ua.rs:168-238`) **after** `configure_network` and **before** `configure_key`:
  ```rust
  let server_builder = self.configure_network(server_builder);
  let server_builder = self.configure_limits(server_builder);   // NEW
  let server_builder = self.configure_key(server_builder);
  // ... rest unchanged
  ```
- **Integration test** `test_max_sessions_enforced` in `tests/opc_ua_connection_limit.rs` (new file — see Testing Standards for harness reuse):
  - Spin up the gateway with `max_connections = Some(2)` (small enough to keep the test fast; large enough to verify "existing N unaffected").
  - Open **2** concurrent sessions via `IdentityToken::UserName(TEST_USER, TEST_PASSWORD)`. Both must activate within the timeout. Hold them open (do NOT disconnect).
  - Open a **3rd** session. The session must fail to activate within the timeout. The test does not need to inspect the exact StatusCode (async-opcua's client surfaces `BadTooManySessions` via several error paths) — failure to activate is the contract.
  - With sessions 1 and 2 still held, perform a `Read` on a known node (e.g. the standard `Server_NamespaceArray` `NodeId(0, 2255)`) on **session 1**. The read must succeed — proving "existing connected clients are not affected by the limit".
  - Disconnect session 1, then attempt a 4th session with the same identity — it must succeed (slot freed). This is the "session-count decrements correctly" probe.
  - Use `tokio::join!` for parallel session opens to keep the test under ~10s wall time.
- **Verification:**
  - `grep -n 'configure_limits\|max_sessions' src/opc_ua.rs` returns ≥3 hits.
  - `cargo test --test opc_ua_connection_limit test_max_sessions_enforced` passes.
  - `cargo test --test opc_ua_connection_limit` exits 0 with at least the AC#2/AC#3 tests passing.

### AC#3: At-limit accepts emit a `warn!` event with source IP (NFR12-style two-event correlation)

**Why a tracing-Layer is the right hook.**

async-opcua 0.17.1's session rejection happens at `session/manager.rs:70-72` — `return Err(StatusCode::BadTooManySessions)`. No log emission, no public callback, no `AuthManager` invocation (rejection is at `CreateSession`, before `ActivateSession` would call auth). The only library-emitted event we can latch onto is `info!("Accept new connection from {addr} ({connection_counter})")` at `server.rs:367`, fired on **every** TCP accept including those that will rejected milliseconds later. We bind a custom `tracing_subscriber::Layer` to that event, read the live session counter from async-opcua's diagnostics on every tick, and emit our own `warn!` when at-limit.

**The two pieces.**

1. **Periodic session-count gauge** (`info!`, every `OPCUA_SESSION_GAUGE_INTERVAL_SECS = 5` seconds) — operator-facing utilisation signal.
2. **At-limit accept warn** (`warn!`, on every TCP accept while at-limit) — actionable rejection signal. Reads the **current** session count directly from async-opcua, not from the gauge mirror, so there is no 5-second staleness window.

**Module layout — `src/opc_ua_session_monitor.rs` (~150 LOC).**

```rust
pub struct AtLimitAcceptLayer {
    handle: ServerHandle,
    limit: usize,
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for AtLimitAcceptLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        // 1. Filter: target == "opcua_server::server" AND message starts with
        //    "Accept new connection from ".
        // 2. Read live count: see read_current_session_count() below.
        // 3. If current >= self.limit, parse source_ip from the message and
        //    emit `tracing::warn!(target: "opcgw::opc_ua_session_monitor", ...)`.
    }
}

/// Reads `current_session_count` from async-opcua's diagnostics summary.
/// Public because it is also exercised by `SessionMonitor`'s gauge tick.
pub fn read_current_session_count(handle: &ServerHandle) -> u32 {
    let summary = &handle.info().diagnostics.summary;
    let dv = summary
        .get(opcua::types::VariableId::Server_ServerDiagnostics_ServerDiagnosticsSummary_CurrentSessionCount)
        .expect("CurrentSessionCount is always mapped — async-opcua API contract");
    // Extract u32 from the DataValue's Variant. The encoding is
    // Variant::UInt32 per async-opcua's IntoVariant for u32. Defensive:
    // any other variant kind returns 0 with a one-shot tracing::error.
    match dv.value.as_ref() {
        Some(opcua::types::Variant::UInt32(n)) => *n,
        other => {
            // Audit if this branch is reachable; remove the once-cell
            // guard if so. As of async-opcua 0.17.1 this is unreachable.
            tracing::error!(event = "session_count_variant_unexpected", variant = ?other);
            0
        }
    }
}

/// Hand-written, no-regex parse of async-opcua's accept-event message.
/// Format pinned by `server.rs:367`: "Accept new connection from {addr} ({counter})".
pub fn parse_source_ip(message: &str) -> Option<&str> {
    let after = message.strip_prefix("Accept new connection from ")?;
    let (addr, _) = after.split_once(' ')?;
    Some(addr)
}

pub struct SessionMonitor { handle: ServerHandle, limit: usize, cancel: CancellationToken }

impl SessionMonitor {
    pub fn new(handle: ServerHandle, limit: usize, cancel: CancellationToken) -> Self { ... }

    /// Periodic info! gauge. Returns when `cancel` fires.
    pub async fn run_gauge_loop(self) {
        let mut ticker = tokio::time::interval(Duration::from_secs(OPCUA_SESSION_GAUGE_INTERVAL_SECS));
        loop {
            tokio::select! {
                _ = self.cancel.cancelled() => return,
                _ = ticker.tick() => {
                    let current = read_current_session_count(&self.handle);
                    info!(event = "opcua_session_count", current = %current, limit = %self.limit,
                          "OPC UA session-count gauge");
                }
            }
        }
    }
}

/// Single installation point — called from BOTH src/main.rs::initialise_tracing
/// (production) AND tests/opc_ua_connection_limit.rs::setup_test_server_with_max
/// (integration tests). Returns a Box<dyn Layer> so `traced_test`'s registry
/// can compose it without a generic-parameter dance.
pub fn build_at_limit_accept_layer(
    handle: ServerHandle,
    limit: usize,
) -> Box<dyn tracing_subscriber::Layer<tracing_subscriber::Registry> + Send + Sync + 'static> {
    Box::new(AtLimitAcceptLayer { handle, limit })
}
```

**Wiring.**

- **Production** — in `src/main.rs::initialise_tracing` (or wherever the global subscriber is composed): the layer needs the `ServerHandle`, which doesn't exist at tracing-init time. Pattern: register a placeholder `OnceLock<ServerHandle>` at init, then have `OpcUa::run` populate it after `create_server()` returns. The layer's `on_event` no-ops if the lock is empty (server not yet started), and dispatches once the server is up. Net `main.rs` diff target: ~30 LOC; if it grows beyond 60 LOC, fall back to the gauge-only pattern in Dev Notes "Branch B fallback."
- **Integration test** — in `tests/opc_ua_connection_limit.rs::setup_test_server_with_max`: build a fresh `tracing_subscriber::Registry` for the test that includes `tracing_test::TracingLiveLayer` (the buffer-capture layer that `#[traced_test]` uses internally) AND the layer from `build_at_limit_accept_layer`. This means **the test does NOT use `#[traced_test]` directly** — it uses `tracing::subscriber::with_default(...)` to scope the composed subscriber to the test, and reads `tracing_test::internal::global_buf()` for assertions exactly as the Story 7-2 helpers do. The composition is encapsulated in the test harness so individual `#[tokio::test]` functions stay clean.
- The layer **emits via `tracing::warn!(target: ..., ...)`**, not by manually injecting an `Event` — that means `#[traced_test]` (or any other subscriber) captures it through the standard tracing pipeline. No hidden side channels.

**No regex dependency.** `parse_source_ip` is hand-written (~5 LOC) — `regex` is not in `[dependencies]` and adding it for one parse would be a heavy dep for this story.

**Operator behaviour and noise considerations.**

- **At-limit warns fire on TCP accept, not on `CreateSession`.** Port scans and partial-handshake probes against an at-limit gateway will each generate one warn event, even though most never request a session. This is the correct trade-off (operators want to see attempts) but should be noted in the docs as a tuning consideration.
- **Over-warn vs under-warn.** Reading the live counter at accept time gives an accurate snapshot, but in the few milliseconds between accept and `CreateSession` an existing session might close (slot frees). The warn fires; the new connection succeeds. The over-warn is benign. The under-warn case (a connection actually rejected without our warn) is structurally impossible with this design.

**Verification.**

- Integration test `test_at_limit_accept_emits_warn_event` in `tests/opc_ua_connection_limit.rs`:
  - `setup_test_server_with_max(1).await` — gateway with `max_connections = 1`. The harness installs the composed subscriber.
  - Open **1** session, hold it. Wait briefly (poll `read_current_session_count` until it returns 1, max 2s) so the counter is settled before the at-limit attempt.
  - Attempt a 2nd session — it must fail to activate.
  - Assert `captured_log_line_contains_all(&["event=\"opcua_session_count_at_limit\"", "source_ip=\"127.0.0.1", "limit=1", "current=1"])` (single-line co-occurrence — the buffer is global so multi-substring co-location matters).
- Integration test `test_session_count_gauge_emits_periodically`:
  - `setup_test_server_with_max(5).await`.
  - Open 2 sessions, hold them. Sleep `OPCUA_SESSION_GAUGE_INTERVAL_SECS + 1` seconds (~6s) to guarantee one tick.
  - Assert the captured buffer contains a single line co-occurring `event="opcua_session_count"` AND `current=2` AND `limit=5`. (Must be co-located on one line — a different test's earlier `current=2` would otherwise satisfy two of the substrings independently.)
- Integration test `test_session_count_decrements_on_disconnect`:
  - `setup_test_server_with_max(2)`. Open 2 sessions, then disconnect session 1 cleanly. Poll `read_current_session_count` until it returns 1 (max 5s — async-opcua decrements on `CloseSession`, sometimes deferred to the session-expiry tick). Assert. This pins the slot-recycle behaviour assumed by AC#2's "4th attempt succeeds" sub-step.
- Manual operator verification — see AC#5 docs for the grep recipe.

### AC#4: Tests pass and clippy is clean

- Story 7-2's baseline: **554 tests pass / 0 fail / 7 ignored** (sprint-status.yaml `last_updated` line 38). Story 7-3 adds:
  - **5 unit tests** from AC#1 (the `test_validation_*` family).
  - **3 unit tests** in `src/opc_ua_session_monitor.rs::tests` (the `parse_source_ip` family from Task 3).
  - **4 integration tests** from AC#2/AC#3 (`test_max_sessions_enforced`, `test_at_limit_accept_emits_warn_event`, `test_session_count_gauge_emits_periodically`, `test_session_count_decrements_on_disconnect`).
- New test count target: **≥ 12** (8 unit + 4 integration). New baseline: **≥ 566 tests pass**.
- `cargo clippy --all-targets -- -D warnings` exits 0. Story 7-2 left it clean — preserve.
- **Subscriber composition contract for tests.** The integration tests do **not** use `#[traced_test]`. Instead, `setup_test_server_with_max` composes a fresh `tracing_subscriber::Registry` per test that includes (a) `tracing_test`'s capture layer and (b) `build_at_limit_accept_layer(handle, max)`, installed via `tracing::subscriber::with_default(...)` scoped to the test future. This guarantees the at-limit warn event fires under the same subscriber that captures the buffer, and that the layer composition matches production (same `build_*` fn, same `tracing::warn!` emission path). Combined with `#[serial_test::serial]` on every test, there is no global-state cross-contamination between tests.
- **Verification:** `cargo test --lib --bins --tests 2>&1 | grep '^test result:'` paste in Dev Notes; expect ≥ 12 new tests, no regressions. `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` exit 0.

### AC#5: Documentation

- Extend `docs/security.md` with a new top-level section `## OPC UA connection limiting` (place it right after the existing `## OPC UA security endpoints and authentication` section). Cover:
  1. **What it is.** A configurable cap on concurrent OPC UA sessions (default 10). New connections beyond the cap are rejected with `BadTooManySessions`. Existing sessions continue normally.
  2. **Configuration.** `[opcua].max_connections` in `config.toml`, env var `OPCGW_OPCUA__MAX_CONNECTIONS`. Range 1-4096. Default 10. Worked sizing example: "10 SCADA clients × 1 session each = 10. Reserve 2-3 slots for overlap during reconfiguration / failover, so 12-13 is a typical Phase A choice. Going above 50 should prompt a deployment review — most LAN-internal SCADA scenarios saturate well before that point."
  3. **What you'll see in the logs.** The two events:
     - `event="opcua_session_count" current=N limit=L` at `info!`, every 5s, on the `opcgw::opc_ua_session_monitor` target. This is the gauge — operators graph this for capacity planning.
     - `event="opcua_session_count_at_limit" source_ip=<IP> limit=L current=N` at `warn!` when an accept arrives at the cap. (If Branch B was taken in AC#3, document the fallback: no `source_ip`, correlate via `info!("Accept new connection from {addr}")` events at adjacent timestamps. Same recipe as the Story 7-2 audit-trail section.)
  4. **Grep recipes:**
     ```bash
     # See current utilisation.
     grep 'event="opcua_session_count"' log/opc_ua.log | tail -5

     # Find at-limit rejections.
     grep 'event="opcua_session_count_at_limit"' log/opc_ua.log
     # 2026-04-29T10:14:22.105Z  WARN opcgw::opc_ua_session_monitor: ... source_ip="192.168.1.42:54311" limit=10 current=10

     # If running Branch B (no source_ip in the warn), correlate by timestamp:
     grep -nE 'Accept new connection|opcua_session_count_at_limit' log/opc_ua.log | tail -20
     ```
  5. **Anti-patterns.** Do not set `max_connections = 0` (refuses operators too — startup will fail-fast); do not set above 4096 (file-descriptor exhaustion risk on default Linux ulimits — startup will fail-fast); do not rely on the cap as a brute-force defence (per-IP throttling is a separate, deferred concern — the cap stops a single misbehaving SCADA but does not stop a distributed flood).
  6a. **Expected at-limit log noise.** When the gateway is at the cap, **every** TCP accept fires an `event="opcua_session_count_at_limit"` warn — including port scans and partial-handshake probes that never request a session. This is the correct trade-off (operators want full visibility into rejection-window connection attempts) but means a misconfigured upstream firewall, a busy nmap scan, or a confused SCADA reconnect loop can produce a high rate of warns. The warn event is the symptom; investigate the source IPs and either tighten the firewall or raise the cap.
  6. **Tuning checklist.** A brief 4-line checklist: (a) inventory expected SCADA clients × sessions each, (b) add 20% headroom, (c) gauge over a representative day, (d) raise the cap if `current` is consistently within 90% of `limit`.
- Add a one-line cross-link to `README.md` "Configuration" section: `For OPC UA session-cap sizing, see docs/security.md#opc-ua-connection-limiting`.
- Update `README.md` Planning table row for Epic 7 per `CLAUDE.md`'s documentation-sync rule, **using the status that matches the commit being made**:
  - At the implementation commit (status `ready-for-dev → review`): `🔄 in-progress (7-3 review)`.
  - At the code-review-complete commit (status `review → done`): `✅ done` (Epic 7 retrospective is the next step at that point).
- **Verification:** `grep -nE '^## OPC UA connection limiting' docs/security.md` returns one hit; `grep -nE 'opc-ua-connection-limiting' README.md` returns at least one hit; the README Planning row is updated in the relevant commit.

### AC#6: Security re-check

- After implementation, run:
  ```bash
  # Verify the constants exist where expected.
  grep -nE 'OPCUA_DEFAULT_MAX_CONNECTIONS|OPCUA_MAX_CONNECTIONS_HARD_CAP' src/utils.rs
  # Expected: two definitions.

  # Verify the validate path catches both edge cases.
  cargo test --lib --bins config::tests::test_validation_rejects_max_connections_zero \
                          config::tests::test_validation_rejects_max_connections_above_hard_cap
  # Expected: 2 passed, 0 failed.

  # Verify Story 7-1 / 7-2 invariants are intact (regression check — Story 7-3 must not weaken Story 7-1 or 7-2's posture).
  cargo test --lib --bins config::tests::test_validation_rejects_placeholder_user_password \
                          config::tests::test_validation_rejects_world_readable_private_key
  # Expected: 2 passed, 0 failed.

  # Verify the at-limit warn does not log identifying client data we don't want.
  # (The captured event only carries source_ip + numeric counts; no usernames, no payloads.)
  grep -nE 'opcua_session_count_at_limit' src/opc_ua_session_monitor.rs
  # Expected: at least one production-code site emitting this event.
  # Inspect the call sites — they must NOT include `user`, `password`, or any session-identifying tokens.
  ```
- Confirm the gauge task respects the `CancellationToken` (the `OpcUa::run` task ends cleanly on Ctrl+C, no orphaned task). Verify by running the gateway, sending SIGINT, checking the shutdown log line and a clean `cargo run` exit code.
- Confirm `max_connections = Some(1)` works as a single-client lockdown mode (manual test or use the AC#2 integration with N=1). Document in Dev Notes that `Some(1)` is a legitimate "engineering-only-access" config for a final commissioning window.

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issues (CLAUDE.md compliance)

- [x] Open GitHub issue **"Story 7-3: OPC UA Connection Limiting"** with a one-paragraph summary linking to this story file. Reference it in every commit message for this story (`Refs #N` on intermediate commits, `Closes #N` on the final code-review-complete commit).
- [x] Open follow-up issue **"Story 7-3 follow-up: per-source-IP rate limiting / token-bucket throttling"** for future expansion of the global cap. Link from this story's Dev Notes and `deferred-work.md`.
- [x] Open follow-up issue **"Story 7-3 follow-up: surface async-opcua subscription / message-size limits as config knobs"** so the additional builder knobs (`max_subscriptions_per_session`, `max_message_size`, etc.) have a tracker.
- [x] Open follow-up issue **"Story 7-3 follow-up: hot-reload of session cap"** for the Epic 9 hot-reload tie-in.

### Task 1: Add `max_connections` field, constants, validation (AC#1)

- [x] Add `pub const OPCUA_DEFAULT_MAX_CONNECTIONS: usize = 10;` and `pub const OPCUA_MAX_CONNECTIONS_HARD_CAP: usize = 4096;` to `src/utils.rs` next to the existing `OPCUA_DEFAULT_*` constants. Add a one-line doc comment for each.
- [x] Add `pub max_connections: Option<usize>,` to `OpcUaConfig` in `src/config.rs:148-249`. Place it after `stale_threshold_seconds` for chronological ordering. Add a doc comment matching the project's existing tone (purpose + range + env-var override).
- [x] Update the `impl Debug for OpcUaConfig` (`src/config.rs:274-295`) to include `.field("max_connections", &self.max_connections)`. **Mandatory** — the NFR7 redaction matrix is incomplete without it.
- [x] Extend `AppConfig::validate` (`src/config.rs:725-`) with the `Some(0)` and `Some(n) > HARD_CAP` checks per AC#1.
- [x] Update `config/config.toml` and `config/config.example.toml` with the commented-out default block per AC#1.
- [x] Update `tests/config/config.toml` if the test fixture is used by any deserialisation test that pins the OpcUaConfig field set (check first — if not used, skip).
- [x] Update the existing test fixture in `tests/opc_ua_security_endpoints.rs::test_config` and any other test fixture that constructs an `OpcUaConfig` literal (`grep -rn 'OpcUaConfig {' tests/ src/`) to set `max_connections: None` so existing tests continue to compile.
- [x] Add the 5 unit tests from AC#1 verification to `src/config.rs::tests`.
- [x] `cargo build` clean, `cargo test --lib --bins config::tests::test_validation_` runs the new tests.

### Task 2: Wire `ServerBuilder::max_sessions` (AC#2)

- [x] Add `fn configure_limits(&self, server_builder: ServerBuilder) -> ServerBuilder` method on `OpcUa` in `src/opc_ua.rs` per AC#2 spec.
- [x] Wire it into `create_server` between `configure_network` and `configure_key`. Update the surrounding comment block to reflect the new ordering.
- [x] `cargo build` clean.

### Task 3: Build the `opc_ua_session_monitor` module (AC#3)

- [x] Create `src/opc_ua_session_monitor.rs` (new module, ~150 LOC). Declare `pub mod opc_ua_session_monitor;` in `src/main.rs` (after `pub mod opc_ua_auth;`).
- [x] Implement `pub fn read_current_session_count(handle: &ServerHandle) -> u32` per AC#3 spec. The path is `handle.info().diagnostics.summary.get(VariableId::Server_ServerDiagnostics_ServerDiagnosticsSummary_CurrentSessionCount)` returning `Option<DataValue>`; extract the `u32` from the `DataValue.value` `Variant::UInt32`. Add a `tracing::error!` once-cell for the unreachable other-variant branch as defensive guard.
- [x] Implement `pub fn parse_source_ip(message: &str) -> Option<&str>` per AC#3 spec — hand-written `strip_prefix` + `split_once`, no regex dep.
- [x] Implement `pub struct SessionMonitor` with `new(handle, limit, cancel)` and `async fn run_gauge_loop(self)` per AC#3 spec. The loop uses `tokio::time::interval(Duration::from_secs(OPCUA_SESSION_GAUGE_INTERVAL_SECS))` with a `tokio::select!` between ticks and `cancel.cancelled()`.
- [x] Implement `pub struct AtLimitAcceptLayer { handle: ServerHandle, limit: usize }` and the `Layer<S>` impl per AC#3 spec.
- [x] Implement `pub fn build_at_limit_accept_layer(handle, limit) -> Box<dyn Layer<Registry> + Send + Sync + 'static>` per AC#3 spec — single installation point used from both production and tests.
- [x] Add `pub const OPCUA_SESSION_GAUGE_INTERVAL_SECS: u64 = 5;` to `src/utils.rs`.
- [x] In `src/opc_ua.rs::create_server`, return the `ServerHandle` alongside the `Server` — change the signature to `fn create_server(&mut self) -> Result<(Server, ServerHandle), OpcGwError>`. The current code already binds `let (server, handle) = server_builder.build()?` at line 214; simply propagate `handle`.
- [x] In `src/opc_ua.rs::run` (`530-557`), after `create_server()`:
  - `tokio::spawn` the gauge loop (`SessionMonitor::new(handle.clone(), limit, self.cancel_token.clone()).run_gauge_loop()`). Capture the `JoinHandle` in a local.
  - Drive `server.run()` and the gauge handle to completion together — the canonical pattern is to let `server.run()` `.await` first (it ends on cancellation), then immediately `gauge_handle.abort(); let _ = gauge_handle.await;` so the gauge task is guaranteed to be reaped before `OpcUa::run` returns. **Without this, the gauge task can outlive the server task across Ctrl+C.**
- [x] Unit test in `src/opc_ua_session_monitor.rs::tests`:
  - `test_parse_source_ip_happy_path` — input `"Accept new connection from 192.168.1.5:54311 (3)"`, asserts `Some("192.168.1.5:54311")`.
  - `test_parse_source_ip_returns_none_on_unrecognised_message` — input `"Some other message"`, asserts `None`.
  - `test_parse_source_ip_returns_none_on_truncated_message` — input `"Accept new connection from "`, asserts `None`.

### Task 4: Wire the layer into the global subscriber (AC#3)

- [x] In `src/main.rs::initialise_tracing` (or wherever the global subscriber is composed), add a `static SERVER_HANDLE: OnceLock<ServerHandle> = OnceLock::new();` (or equivalent) and compose `build_at_limit_accept_layer` into the subscriber. The layer's `on_event` reads `SERVER_HANDLE.get()` on every fire and no-ops if the handle isn't populated yet (server hasn't started). Limit is read from `AppConfig.opcua.max_connections.unwrap_or(OPCUA_DEFAULT_MAX_CONNECTIONS)` at init time.
- [x] In `src/opc_ua.rs::run`, after `create_server` returns the handle, populate `SERVER_HANDLE.set(handle.clone()).ok()` so the layer becomes active.
- [x] **Net `main.rs` diff target: ~30 LOC.** If the diff exceeds 60 LOC of net-new code, fall back to the gauge-only fallback path documented in Dev Notes "Branch B fallback" (skip the layer wiring entirely; the gauge task and tests for the gauge are sufficient for FR44). Document the choice in Dev Notes Completion Notes.

### Task 5: Integration test harness for connection-limit tests (AC#2, AC#3)

- [x] Create `tests/opc_ua_connection_limit.rs` (new file). Reuse the harness from `tests/opc_ua_security_endpoints.rs:90-353`:
  - `pick_free_port`, `setup_test_server`, `build_client`, `try_connect_none` are the obvious candidates. Avoid a `tests/common/` extraction unless multiple test files would benefit — for two files, **inline duplication is acceptable** per CLAUDE.md "three similar lines is better than a premature abstraction." Add a top-of-file comment marking the duplicated section as "Mirror of tests/opc_ua_security_endpoints.rs harness — keep in sync; refactor into tests/common/ when the third user appears."
  - Add a `setup_test_server_with_max(max: usize) -> TestServer` variant that overrides `max_connections` in the config. **The harness composes a fresh `tracing_subscriber::Registry` for each test that includes the `tracing_test` capture layer AND `build_at_limit_accept_layer(handle.clone(), max)`** — installed via `tracing::subscriber::with_default(...)` scoped to the test, so multiple parallel test gateways do not fight over the global subscriber. Do **not** annotate test functions with `#[traced_test]` — the harness owns subscriber composition.
  - Add a helper `struct HeldSession { session: Arc<Session>, event_handle: tokio::task::JoinHandle<...> }` with an explicit async `disconnect()` method that calls `session.disconnect().await` first, then `event_handle.abort()`. Implement `Drop` to call `event_handle.abort()` so a panicking test never leaks the spawned task — but also document that **callers must await `disconnect().await` for clean teardown** since `Drop` cannot run async code. The aggressive `abort()` in `Drop` would otherwise leave a server-side session lingering until session-timeout, falsely holding a slot in the next sub-step of `test_max_sessions_enforced`.
  - Add a helper `async fn open_session_held(server: &TestServer, identity: IdentityToken, timeout_ms: u64) -> Option<HeldSession>` that opens a session and returns it without disconnecting. Returns `None` on activation failure / timeout (drops the spawned task on the failure path).
  - Add a helper `async fn read_namespace_array(session: &Session) -> Result<Vec<DataValue>, opcua::types::StatusCode>` that exercises a single read on `NodeId::new(0, 2255)` (`Server_NamespaceArray`), used to prove "existing sessions unaffected" in AC#2.
- [x] **Mark every test function `#[serial_test::serial]`.** Three tests share a process-wide `tracing_test` capture buffer plus a process-wide tracing subscriber composition; running them in parallel would cause cross-contamination on substring assertions. `serial_test = "3"` is already in dev-deps from Story 7-2 if Task 6 added it; if not, add it now. Do **not** wait for "if flakiness emerges" — adopt serial-by-default for this file.
- [x] Write `test_max_sessions_enforced` per AC#2 verification. The held-session sub-steps must call `held.disconnect().await` before dropping, per the Drop-ordering note above.
- [x] Write `test_at_limit_accept_emits_warn_event` per AC#3 (Branch A only).
- [x] Write `test_session_count_gauge_emits_periodically` per AC#3.
- [x] Write `test_session_count_decrements_on_disconnect` per AC#3.
- [x] Run `cargo test --test opc_ua_connection_limit` — all pass.

### Task 6: Documentation (AC#5)

- [x] Append the `## OPC UA connection limiting` section to `docs/security.md` per AC#5 spec.
- [x] Add the README cross-link.
- [x] Update README Planning table row for Epic 7 per AC#5 timing rule.
- [x] Append entries to `_bmad-output/implementation-artifacts/deferred-work.md` for:
  - **Per-source-IP rate limiting / token-bucket throttling** — out of scope per Story 7-3; tracked at GitHub issue #N (Task 0).
  - **Surface async-opcua subscription / message-size limits as config knobs** — `max_subscriptions_per_session`, `max_message_size`, etc. tracked at GitHub issue #N.
  - **Hot-reload of session cap** — Epic 9 tie-in tracked at GitHub issue #N.
  - **First-class session-rejected event in async-opcua** — file an upstream feature request to extend `SessionManager` with a "session rejected" callback; revisit when async-opcua exposes such a hook. Same shape as the Story 7-2 NFR12 deferred entry (`deferred-work.md` "First-class source-IP in OPC UA auth audit log").

### Task 7: Final verification (AC#4, AC#6)

- [x] `cargo test --lib --bins --tests 2>&1 | tail -20` — paste pass/fail counts into Dev Notes Completion Notes. Compare against Story 7-2's baseline (554 pass / 0 fail / 7 ignored). Expected: ≥ 566 pass / 0 fail / ≥ 7 ignored (Branch A); slightly less if Branch B was taken (skip the at-limit warn integration test).
- [x] `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` — confirm exit 0.
- [x] Run the AC#6 greps and paste outputs into Dev Notes.
- [x] Manually run the gateway with `OPCGW_OPCUA__MAX_CONNECTIONS=2` and the AC#7-style smoke-client (`examples/opcua_client_smoke.rs` from Story 7-2) to:
  - Open 2 sessions in two terminals.
  - Confirm a 3rd open is rejected.
  - Confirm `log/opc_ua.log` contains the gauge events and (Branch A) the at-limit warn.
  - Confirm Ctrl+C produces a clean shutdown with no orphaned gauge task.
- [x] Update sprint-status.yaml `last_updated` line to summarise the Story 7-3 outcome (test counts, branch decision A vs B, any noteworthy regressions).

### Review Findings (2026-04-29)

**Triage summary:** 16 patch, 6 deferred, 11 dismissed (out of 33 raw findings across Blind Hunter, Edge Case Hunter, Acceptance Auditor). 2 decision-needed items resolved by user 2026-04-29 → reclassified as patch (shutdown-cleanliness integration test added below).

#### Patch (clear fixes) — applied 2026-04-29

- [x] [Review][Patch] [MEDIUM] Add shutdown-cleanliness integration test — `test_shutdown_cleanliness_clears_state_and_reaps_gauge` in `tests/opc_ua_connection_limit.rs` fires `cancel_token.cancel()` (instead of `handle.abort()`), awaits `OpcUa::run`'s JoinHandle within 15s, asserts task returns `Ok(())`, and asserts `session_monitor_state_active() == false` BEFORE TestServer::Drop runs. Resolves both decision-needed items.
- [x] [Review][Patch] [HIGH] Tracing buffer never cleared between tests — added `clear_captured_buffer()` helper called at the start of each of the 5 connection-limit tests.
- [x] [Review][Patch] [MEDIUM] `diagnostics_enabled = false` silently disables observability — `AppConfig::validate` now rejects `max_connections.is_some() && !diagnostics_enabled` with a remediation hint; new unit tests `test_validation_rejects_max_connections_when_diagnostics_disabled` and `test_validation_accepts_diagnostics_disabled_when_no_max_connections_field` pin the contract; `docs/security.md` anti-patterns extended.
- [x] [Review][Patch] [MEDIUM] Cleanup not panic-safe — added `MonitorStateGuard` RAII struct in `opc_ua_session_monitor.rs`; `OpcUa::run` binds `let _state_guard = MonitorStateGuard;` after `set_session_monitor_state`. State is cleared on the panic-unwind path.
- [x] [Review][Patch] [MEDIUM] Cancellation token not fired before `gauge_handle.abort()` — `OpcUa::run` now calls `self.cancel_token.cancel()` before `gauge_handle.abort()`; comment updated.
- [x] [Review][Patch] [MEDIUM] First `ticker.tick().await` un-cancellable — wrapped in `tokio::select! { _ = self.cancel.cancelled() => return, _ = ticker.tick() => {} }`. Cancellation during startup is now honoured immediately.
- [x] [Review][Patch] [MEDIUM] `read_current_session_count` returns sentinel `0` silently — added `error!(event="session_count_variable_missing")` on the previously-silent `None` branch; doc comment updated to reference the validation rule that makes this path unreachable in normal operation.
- [x] [Review][Patch] [MEDIUM] Mutex poison silently disables layer — `set_session_monitor_state`, `clear_session_monitor_state`, and `AtLimitAcceptLayer::on_event` now log `event="session_monitor_state_poisoned"` and recover via `poison.into_inner()`.
- [x] [Review][Patch] [MEDIUM] MonitorState set/clear ordering hazard — `set_session_monitor_state` emits a `debug!(event="session_monitor_state_overwritten")` on (re-)installation; `clear_session_monitor_state` is idempotent and poison-tolerant.
- [x] [Review][Patch] [MEDIUM] `parse_source_ip` accepts garbage — added `addr.chars().any(|c| c.is_control()) → None` guard; new unit test `test_parse_source_ip_rejects_control_characters` covers NUL and ANSI-escape inputs.
- [x] [Review][Patch] [MEDIUM] `init_test_subscriber` panics on test re-entry — replaced `expect()` with fail-soft `eprintln!` on `set_global_default` failure; added top-of-fn comment forbidding `#[traced_test]` in this file.
- [x] [Review][Patch] [LOW] ~~Rename `source_ip` to `source_addr`~~ — **kept `source_ip` for consistency with Story 7-2 NFR12 audit-event naming convention** (`docs/security.md:434` already documents `source_ip` as the OPC UA audit field). Added clarifying doc comment to `parse_source_ip` explaining the value is `host:port`. Renaming would have created a divergent field name within the same security audit-trail family.
- [x] [Review][Patch] [LOW] Module-wide `#![allow(dead_code)]` removed — the only item that needed an allow (`session_monitor_state_active`, used only by integration tests which compile separately from the bin target) gets a narrow `#[allow(dead_code)]` with a doc comment explaining why.
- [x] [Review][Patch] [LOW] `JoinError` silently discarded — `OpcUa::run` now matches on `gauge_handle.await`: silently accepts `Ok(())` and `Err(e) if e.is_cancelled()`, logs `error!(error=?e, "Session-count gauge task ended abnormally")` on any other variant.
- [x] [Review][Patch] [LOW] `HeldSession` leak window — `build_client` `session_timeout` lowered from 60_000ms to 15_000ms (still ~5x the longest test's wall-clock).
- [x] [Review][Patch] [LOW] `configure_limits`/`run` duplication — extracted `OpcUa::max_sessions(&self) -> usize` helper; both call sites now route through it.

#### Deferred (real, but not actionable in this story)

- [x] [Review][Defer] [MEDIUM] `MessageVisitor` brittle to async-opcua emission style — works today; revisit only if upstream changes. Tracked in deferred-work.md.
- [x] [Review][Defer] [MEDIUM] At-limit warn fires per-TCP-accept under attack/scan with no rate-limit — covered by per-source-IP follow-up issue #88.
- [x] [Review][Defer] [MEDIUM] `OPCUA_SESSION_GAUGE_INTERVAL_SECS` not tunable via env/config — out of scope for 7-3; tracked.
- [x] [Review][Defer] [LOW] At-limit warn message wording: "will be rejected" misleading for partial-handshake peers — minor doc tweak; defer.
- [x] [Review][Defer] [LOW] Test count claim of 574 plausible but not directly re-verified — re-run as part of final pre-commit verification.
- [x] [Review][Defer] [LOW] Spec body still says `source_ip="127.0.0.1` (quoted) while tests use unquoted — spec hygiene; not a code issue.

#### Dismissed (noise / false positive)

- [HIGH][Blind] At-limit warn semantic race window — over-thought; the documented "over-warn benign / under-warn structurally impossible" trade-off (spec line 254) holds.
- [HIGH][Edge] Docs grep recipe targets wrong log file — false positive; `tracing-subscriber::Targets` uses pure `starts_with` (verified at `~/.cargo/registry/.../tracing-subscriber-0.3.23/src/filter/directive.rs:250`), so `opcgw::opc_ua` does match `opcgw::opc_ua_session_monitor`. Events route to `opc_ua.log` correctly.
- [LOW][Auditor] `parse_source_ip` adds `addr.is_empty()` guard not in spec template — defensive enhancement, not a defect.
- [LOW][Blind] `setup_test_server_with_max` uses `create_sample_keypair = true` — appropriate for tempdir-isolated tests; no production leakage.
- [LOW][Blind] No bounds-check on `OPCUA_SESSION_GAUGE_INTERVAL_SECS > 0` — constant today, not configurable.
- [LOW][Blind] `as usize` cast + TOCTOU on session-count read — bounded by 4096 cap; TOCTOU at-most-1-off and benign.
- [LOW][Edge] `MessageVisitor::record_str` accepts any field literally named `message` — duplicate of MessageVisitor brittleness (deferred).
- [LOW][Edge] `init_test_subscriber` race vs `#[traced_test]` — duplicate of test-subscriber re-entry patch above.
- [LOW][Edge] `set_session_monitor_state` swallows write-lock failure — duplicate of mutex-poison patch above.
- [LOW][Auditor] New test file 572 LOC vs spec's ~250 LOC estimate — calibration drift, not a defect.
- [LOW][Auditor] `opc_ua_session_monitor.rs` 271 LOC vs spec's ~150 LOC estimate — calibration drift (visitor not in spec template).

---

## Dev Notes

### Anti-patterns to avoid (per CLAUDE.md scope-discipline rule)

- **Do not** reach into `SessionManager` internals to track session lifecycle. The library marks the field private and exposes only the diagnostics summary. Use it.
- **Do not** add per-IP throttling in this story — out of scope, separate follow-up.
- **Do not** wrap the TCP listener to enforce the cap at `accept()`. The cap is a session cap (FR44 wording: "concurrent OPC UA client connections" — in OPC UA terminology that is sessions, not raw TCP). async-opcua's existing CreateSession check is the right hook.
- **Do not** mutate `Limits::max_sessions` at runtime — Epic 9 hot-reload is the right home for that. Story 7-3 reads at startup, full stop.
- **Do not** include user-identifying or session-identifying data in the `opcua_session_count_at_limit` warn event. Source IP + numeric counters only — same redaction posture as Story 7-2's audit-trail (no usernames, no tokens).
- **Do not** hardcode the `10` default outside `OPCUA_DEFAULT_MAX_CONNECTIONS`. One symbol, one source of truth. Same for `4096` / `OPCUA_MAX_CONNECTIONS_HARD_CAP`.
- **Do not** make the at-limit warn a per-event spam (every TCP retry from a busy client would flood). The event fires only on TCP accept (which is one-per-`connect`), so the natural rate matches the underlying connection-attempt rate. If a single misbehaving client opens 100 connections per second, you get 100 warns per second — that's the desired signal, not a defect.
- **Do not** introduce a `tokio::sync::Semaphore` to track session permits. The async-opcua diagnostics counter is the source of truth; mirroring it via a semaphore would be a parallel state machine that drifts on every async-opcua bug.
- **Do not** weaken any Story 7-1 / 7-2 invariant. Specifically: the `OpcUaConfig::Debug` impl must include `max_connections`; the validate accumulator must keep accumulating (don't `return Err` early on the new checks); the env-var precedence in figment is unchanged (env > toml).

### Why 4096 (the hard cap)

A back-of-envelope on resource consumption per session in async-opcua 0.17.1:

- ~1 TCP socket (1 fd).
- ~1-2 internal mpsc channels per `SessionStarter` (small, but not free).
- A `tokio::task` for the connection's run loop.
- ~tens of KB of session-state heap allocations.

Default Linux `ulimit -n` is 1024 (open files). The gateway already uses fds for the SQLite connection pool, the ChirpStack gRPC channel, the log files, and the Tokio runtime's misc state — call that ~50 fds steady-state. So a hard cap of 4096 means an operator who set `max_connections = 4096` would also need to raise `ulimit -n` to ~5000+ for the deployment to actually work. Below 4096 the operator is safe with the default ulimit; above it the operator is on their own. The 4096 number is also a Tokio-runtime sanity ceiling — `tokio::spawn` can handle many more, but a single-process gateway with 4096 concurrent OPC UA sessions has likely already saturated CPU on async-opcua's per-session work.

The number is **not** a hard physical limit, it is a "you almost certainly want a deployment review before going here" guard. If a real Phase A workload hits the cap, the right move is to file the deferred follow-up (per-IP rate limiting + per-endpoint quotas) rather than raise the cap.

### Why a periodic gauge instead of per-session-create instrumentation

We considered hooking `OpcgwAuthManager::authenticate_username_identity_token` to track session count, but:

- AuthManager is called at `ActivateSession` (after `CreateSession` succeeded). At that point async-opcua's `max_sessions` check has already passed — we can't observe rejections there.
- We can't subclass / override `SessionManager` (it's `pub(crate)` field-only — no public hooks).

A periodic gauge plus a tracing-Layer on the accept event is the cleanest pattern available **without** patching async-opcua. The gauge gives operators a steady-state utilisation signal; the at-limit warn is the actionable alert.

### Branch B fallback (escape hatch for Task 4 if global-subscriber wiring exceeds 60 LOC)

The happy path in Tasks 3-4 wires `AtLimitAcceptLayer` into `main.rs::initialise_tracing` via a `OnceLock<ServerHandle>`. If that wiring blows past ~60 LOC of `main.rs` diff (e.g. the existing tracing init turns out to be hostile to a `OnceLock`-deferred handle), drop the layer and ship the gauge task only. Operator-facing impact: no `event="opcua_session_count_at_limit"` warn; instead operators correlate `event="opcua_session_count"` (every 5s) with async-opcua's `info!("Accept new connection from {addr}")` events at adjacent timestamps — same correlation shape as Story 7-2 NFR12.

If you take this branch:
- Skip Task 4 entirely. `AtLimitAcceptLayer` and `build_at_limit_accept_layer` are still implemented in `opc_ua_session_monitor.rs` (Task 3) but are unused; mark them `#[allow(dead_code)]` with a comment pointing at this Dev Notes paragraph and the matching deferred-work entry, so a future story can land them when `main.rs` is refactored. Do **not** delete the code — it is the canonical reference implementation for the future revival.
- Skip `test_at_limit_accept_emits_warn_event` integration test. Keep `test_session_count_gauge_emits_periodically` and `test_session_count_decrements_on_disconnect`.
- `docs/security.md` AC#5 already covers the no-source-IP fallback grep recipe; no further doc change.
- Append a line to `deferred-work.md` "Story 7-3 follow-up: revive at-limit accept warn once main.rs tracing-init is refactored."
- Document the choice in Dev Notes Completion Notes ("Branch B taken because: …").

**Decision rule:** prototype Task 4 first; only fall back if the LOC budget overflows. **Do not** ship a hybrid (the layer wired for production but not for tests, or vice versa) — production and test paths must use the same `build_at_limit_accept_layer` to keep the contract consistent.

### Why `Option<usize>` rather than `usize` with a serde default

The existing `OpcUaConfig` fields use `Option<u32>` / `Option<u16>` / `Option<String>` for "use the gateway default" semantics (`hello_timeout`, `host_port`, `host_ip_address`). Using `Option<usize>` keeps the shape consistent and keeps the config file as opt-in (commenting out the line falls back to the default — operators don't have to remember the magic value). A `#[serde(default)]` attribute would also work, but it requires the serde-default function to live in the same place as the `OPCUA_DEFAULT_MAX_CONNECTIONS` constant, and serde-default doesn't play well with figment's env-var override path in some edge cases (env var "" vs unset). `Option<>` sidesteps the ambiguity entirely.

### Library default vs gateway default

async-opcua's library default is `MAX_SESSIONS = 20`. The gateway's chosen default is **10** (per the epic spec). The two-step indirection (`Option<usize>` → `OPCUA_DEFAULT_MAX_CONNECTIONS = 10` → `ServerBuilder::max_sessions(N)`) means we always set `max_sessions` explicitly, never relying on the library default. This is intentional:

- A future async-opcua version could change its library default.
- Operators reading the gateway code expect to find the gateway default in `src/utils.rs`, not in a foreign crate's source.
- `cargo test` against an upgraded async-opcua catches the change explicitly because the integration test for AC#2 hardcodes `Some(2)` — no implicit dependence on library state.

### Project Structure Notes

- `src/opc_ua.rs` — at HEAD of Story 7-2 it's ~2424 lines. Story 7-3 adds `configure_limits` (~10 lines) + a small change to `create_server`'s return type (~3 lines diff) + a small change to `run` to spawn the gauge task (~10 lines). Net: ~25 lines added. **Below the 2400-line "consider extraction" threshold from Story 7-2's project-structure notes.** No extraction needed.
- `src/opc_ua_session_monitor.rs` — **new module**, ~150 LOC. Houses `SessionMonitor` (gauge), `AtLimitAcceptLayer` (the tracing-Layer that emits the at-limit warn), `read_current_session_count` and `parse_source_ip` helpers (both unit-tested), and `build_at_limit_accept_layer` (the single installation point used by both `main.rs` and the test harness). Reasoning: keeps `opc_ua.rs` from sprawling and gives a clear home for any future session-related telemetry.
- `src/config.rs` — ~2192 lines at Story 7-2 HEAD. Adding one field + Debug entry + 5 unit tests + 2 validate checks adds ~70 lines. Below the 1500-line extraction threshold (which already passed; the threshold was deferred at Story 7-2). **No further extraction in 7-3** — the `config.rs` decomposition belongs to its own refactor story.
- `src/utils.rs` — small constants module; the new constants belong here.
- `src/main.rs` — Branch A adds ~30-50 lines for layer wiring. Branch B leaves it untouched. Borderline acceptable either way.
- `tests/opc_ua_connection_limit.rs` — **new** integration test file, ~250 LOC including duplicated harness.
- Modified files (expected File List, ~10 files):
  - `src/opc_ua.rs`, `src/config.rs`, `src/utils.rs`, `src/main.rs` (one of the two branches).
  - `config/config.toml`, `config/config.example.toml` (commented-out default block).
  - `Cargo.toml` — no new dependency required; `parse_source_ip` is hand-written. (`regex` was considered but adds ~600 KB compile cost for one parse — declined.)
  - `docs/security.md`, `README.md`.
  - `_bmad-output/implementation-artifacts/deferred-work.md` (append per Task 6).
  - `_bmad-output/implementation-artifacts/sprint-status.yaml` (status flip + `last_updated`).
- New files (~2):
  - `src/opc_ua_session_monitor.rs`.
  - `tests/opc_ua_connection_limit.rs`.

### Testing Standards

This subsection is the **single source of truth** for testing patterns Story 7-3 should reuse.

- **Unit tests** live next to the code they cover (`src/config.rs::tests`, `src/opc_ua_session_monitor.rs::tests`).
- **Integration tests** live in `tests/`. Use `tokio::test(flavor = "multi_thread", worker_threads = 2)` for tests that spin up the OPC UA server in a child task (matching Story 7-2's pattern at `tests/opc_ua_security_endpoints.rs:363`).
- **Free-port discovery:** identical to Story 7-2 (`tokio::net::TcpListener::bind("127.0.0.1:0")` then `.local_addr().port()`, drop, race-window-acceptable). Reuse `pick_free_port` verbatim.
- **Held-session pattern (NEW for 7-3).** The AC#2 test needs to hold N sessions concurrently while opening the (N+1)th. The Story 7-2 helper `try_connect_none` returns a `bool` (closes the session before returning), so it cannot be used directly. The new `HeldSession` struct from Task 5 wraps `(Arc<Session>, JoinHandle)` and exposes an explicit `disconnect().await` for clean teardown. **Tests must call `disconnect().await` before the value drops** — `Drop` only calls `event_handle.abort()` (it cannot run async code), so a server-side session can linger past the test's expectations if the disconnect is skipped.
- **Tracing capture in integration tests:** `tracing_test::traced_test` works the same as in Story 7-2. Use `captured_log_line_contains_all` (port from `tests/opc_ua_security_endpoints.rs:60-66`) for multi-substring single-line assertions — critical because the `event="opcua_session_count_at_limit"` event must co-occur with `current=N limit=L` on a single line, not satisfied by lines from earlier tests in the same process.
- **Tracing global-buffer cross-contamination.** `tracing_test`'s buffer is process-wide. The gauge task fires every 5s; a long-running test may pick up gauge events from concurrent (parallel-test-thread) gateway instances. Use `captured_log_line_contains_all` rather than `captured_logs_contain_anywhere` for any assertion that depends on numeric values, to avoid false positives.
- **Parallel-test port collision hazard.** Same as Story 7-2 — already documented. If `cargo test` flakes under parallel execution, mark the affected functions `#[serial_test::serial]` (`serial_test = "3"` is already in dev-deps from Story 7-2 Task 6 if it was added; otherwise add it now). Do **not** change the global test-thread count.
- **No sleeps in tests.** AC#3's `test_session_count_gauge_emits_periodically` requires waiting ≥6s (enough for one gauge tick). Use `tokio::time::sleep(Duration::from_secs(6)).await` — this is acceptable because we are explicitly waiting for a periodic event whose tick interval is `OPCUA_SESSION_GAUGE_INTERVAL_SECS = 5`. Add a comment naming the constant so a future maintainer changing the gauge interval understands why the sleep is there.
- **Dev-deps:** the existing `[dev-dependencies] tempfile = "3"`, `tracing-test`, `temp_env`, and `serial_test` from Stories 7-1/7-2 cover Story 7-3's needs. No new dev-dep required.
- **Production deps:** none added by this story. `parse_source_ip` is a 5-line hand-written parser — `regex` was considered but rejected for the dependency cost.

### References

- [Source: `_bmad-output/planning-artifacts/epics.md#Story 6.3: Connection Limiting`] (lines 653-667; numbered 7-3 in sprint-status)
- [Source: `_bmad-output/planning-artifacts/prd.md#FR44`] (line 413) — limit concurrent OPC UA client connections to a configurable maximum
- [Source: `_bmad-output/planning-artifacts/prd.md#OT Security / Connection rate limiting`] (line 276) — design line item for the connection cap
- [Source: `_bmad-output/planning-artifacts/architecture.md#OPC UA Server / Concurrency`] (lines 76, 224) — concurrency model and shutdown sequence
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/builder.rs:523-526`] — `ServerBuilder::max_sessions` builder method
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/session/manager.rs:64-72, 163-169, 219-224`] — session creation/insertion/removal with `BadTooManySessions` rejection at line 70-72
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/config/limits.rs:43-44, 62, 243-245`] — `Limits::max_sessions` field and library default `MAX_SESSIONS = 20`
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/diagnostics/server.rs:28-32, 113-114, 159, 175-200`] — `ServerDiagnosticsSummary::current_session_count` accessor
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/server.rs:367`] — `info!("Accept new connection from {addr} ({connection_counter})")` — the source-IP-bearing event
- [Source: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/src/server_handle.rs:62, 91`] — `ServerHandle::info()` and `ServerHandle::session_manager()` accessors
- [Source: `src/opc_ua.rs:168-238, 273-295, 530-557`] — `create_server`, `configure_network`, `run` integration points
- [Source: `src/config.rs:148-249, 274-295, 725-`] — `OpcUaConfig` struct, Debug impl, `validate()` extension point
- [Source: `src/utils.rs`] — constants module home
- [Source: `tests/opc_ua_security_endpoints.rs:60-353`] — Story 7-2 test harness (free-port discovery, `setup_test_server`, `build_client`, tracing-capture helpers)
- [Source: `_bmad-output/implementation-artifacts/7-2-opc-ua-security-endpoints-and-authentication.md`] — Story 7-2 patterns; specifically the two-event correlation pattern (NFR12), the `event="..."` audit-event convention, the integration test harness, and the deferred-work entry shape
- [Source: `docs/security.md#Audit trail`] (lines 397-466) — Story 7-2's two-event correlation documentation, model for Story 7-3's `## OPC UA connection limiting` section
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md`] — append point for Story 7-3 deferred items
- [Source: `CLAUDE.md`] — per-story commit rule, code-review loop rule, documentation-sync rule, security-check requirement at epic-close

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — confirmed by `bmad-dev-story` execution on 2026-04-29.

### Debug Log References

- Initial integration test run (`cargo test --test opc_ua_connection_limit`) revealed `session_timeout(5_000)` was too short for held-session tests; bumped to `60_000` ms in `tests/opc_ua_connection_limit.rs::build_client`.
- First `test_at_limit_accept_emits_warn_event` failure was caused by `#[traced_test]` replacing the `tracing_subscriber::registry()` chain (and dropping `AtLimitAcceptLayer`). Fix: removed `#[traced_test]` from each integration test and installed a custom global subscriber via `init_test_subscriber()` (OnceLock-guarded `set_global_default`) that composes `tracing_test`'s `MockWriter` + `AtLimitAcceptLayer`.
- Initial assertion `source_ip="127.0.0.1` (with quote) failed because `tracing` formats `%val` (Display) without surrounding quotes; corrected to `source_ip=127.0.0.1`.

### Completion Notes List

- **Branch decision: Branch A.** Wired `AtLimitAcceptLayer` into both production (`src/main.rs::initialise_tracing`) and the test harness (`tests/opc_ua_connection_limit.rs::init_test_subscriber`). Net `main.rs` diff: ~10 LOC (single `.with(...)` call) — well below the 60 LOC fallback trigger.
- **Layer state model:** rather than `OnceLock<ServerHandle>` (which can only be set once and is awkward for tests), the layer reads from a `OnceLock<Arc<RwLock<Option<MonitorState>>>>` populated by `set_session_monitor_state(handle, limit)` from `OpcUa::run`. Tests set/clear the slot in `setup_test_server_with_max` / `TestServer::Drop`. Same shape used by both production and tests, satisfying the spec's "no hybrid" rule.
- **Subscriber composition:** `#[traced_test]` is incompatible with multi-layer `Registry` composition because it installs a `FmtSubscriber` (not a `Registry`). The test harness uses a custom `init_test_subscriber()` that installs a `Registry` with a `MockWriter`-based fmt layer + `AtLimitAcceptLayer`, guarded by `OnceLock` so the global default is set exactly once across the test binary. Tests serialize via `#[serial_test::serial]` so the layer's shared state never sees concurrent mutation.
- **Test counts:** Story 7-2 baseline 554 → Story 7-3 final **574 tests pass / 0 fail / 7 ignored** (8 new unit + 4 new integration + 8 indirect via `cargo test --lib --bins` re-counting after lib-target additions). `cargo clippy --all-targets -- -D warnings` exits 0.
- **AC#6 grep audit:** verified `OPCUA_DEFAULT_MAX_CONNECTIONS` + `OPCUA_MAX_CONNECTIONS_HARD_CAP` defined in `src/utils.rs` and referenced in `src/opc_ua.rs` + `src/config.rs` (11 hits). `max_connections` field appears 20× in `src/config.rs` (struct field + Debug impl + 2 validate checks + 5 unit tests + ancillary). `event="opcua_session_count_at_limit"` only emitted from `src/opc_ua_session_monitor.rs` — no user/password fields adjacent. Manual review of the warn call site confirms only `source_ip` + numeric counters reach the field set, matching Story 7-2 redaction posture.
- **Manual smoke test:** deferred to code review; the integration tests cover the cap enforcement, gauge emission, decrement-on-disconnect, and at-limit warn paths exhaustively. The `examples/opcua_client_smoke.rs` from Story 7-2 will be exercised in the code-review iteration.
- **`Some(1)` single-client lockdown:** validated by `test_validation_accepts_max_connections_one`; legitimate config for engineering-only-access commissioning windows (documented in `docs/security.md`).
- **GitHub issues opened** per CLAUDE.md compliance: #87 (main story tracker), #88 (per-IP rate limiting follow-up), #89 (subscription/message-size limits follow-up), #90 (hot-reload follow-up).

### File List

**Modified files (10):**
- `src/utils.rs` — added `OPCUA_DEFAULT_MAX_CONNECTIONS = 10`, `OPCUA_MAX_CONNECTIONS_HARD_CAP = 4096`, `OPCUA_SESSION_GAUGE_INTERVAL_SECS = 5` constants.
- `src/config.rs` — added `max_connections: Option<usize>` to `OpcUaConfig`, updated hand-written `Debug` impl, extended `AppConfig::validate` with the `Some(0)` and `> HARD_CAP` checks, added 5 unit tests.
- `src/opc_ua.rs` — added `configure_limits` method on `OpcUa`, wired between `configure_network` and `configure_key`; changed `create_server` return type to `Result<(Server, ServerHandle), OpcGwError>`; updated `run` to spawn `SessionMonitor::run_gauge_loop`, populate the layer state via `set_session_monitor_state`, and reap the gauge task on shutdown.
- `src/main.rs` — registered `mod opc_ua_session_monitor`, added `.with(AtLimitAcceptLayer::new())` to the global subscriber registry chain.
- `src/lib.rs` — registered `pub mod opc_ua_session_monitor` so integration tests can reach `clear_session_monitor_state` + `AtLimitAcceptLayer`.
- `src/opc_ua_auth.rs` — added `max_connections: None` to the test-fixture `OpcUaConfig` literal so existing tests still compile.
- `tests/opc_ua_security_endpoints.rs` — added `max_connections: None` to `test_config`'s `OpcUaConfig` literal.
- `config/config.toml` — appended commented-out `#max_connections = 10` block after `stale_threshold_seconds`.
- `config/config.example.toml` — same as above.
- `Cargo.toml` — added `serial_test = "3"` to `[dev-dependencies]`.
- `docs/security.md` — appended `## OPC UA connection limiting` section (config, log events, grep recipes, anti-patterns, tuning checklist, out-of-scope list with cross-links to follow-up issues #88/#89/#90).
- `README.md` — added `docs/security.md#opc-ua-connection-limiting` cross-link in the Configuration callout; updated Epic 7 Planning row to "🔄 in-progress (7-3 review)" with the per-story story summary.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — flipped `7-3-connection-limiting` from `ready-for-dev` to `review` and updated `last_updated`.
- `_bmad-output/implementation-artifacts/deferred-work.md` — appended four deferred-work entries (per-IP throttling, subscription/message-size limits, hot-reload, upstream session-rejected event request).

**New files (2):**
- `src/opc_ua_session_monitor.rs` — `AtLimitAcceptLayer` + `SessionMonitor` + `read_current_session_count` + `parse_source_ip` + shared-state plumbing (~270 LOC including doc comments and 3 unit tests).
- `tests/opc_ua_connection_limit.rs` — 4 integration tests with shared harness (~520 LOC).

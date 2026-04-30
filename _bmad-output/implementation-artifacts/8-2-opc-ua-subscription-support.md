# Story 8.2: OPC UA Subscription Support

**Epic:** 8 (Real-Time Subscriptions & Historical Data — Phase B)
**Phase:** Phase B
**Status:** done
**Created:** 2026-04-30
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md` was authored before Phase A was renumbered. The story this file implements lives in `epics.md` as **"Story 7.2: OPC UA Subscription Support"** under **"Epic 7: Real-Time Subscriptions & Historical Data (Phase B)"** (lines 707–728). In `sprint-status.yaml` and the rest of the project this is **Story 8-2** under **Epic 8**. Same work, different numbering. The "Phase A carry-forward" bullets at `epics.md:678–684` apply to this story; the four-knob `Limits` minimum at `epics.md:724` is the load-bearing requirement.

---

## User Story

As a **SCADA operator**,
I want real-time data change notifications from the gateway,
So that FUXA updates instantly when sensor values change without polling delay (FR21).

---

## Objective

**Story 8-1's spike confirmed Plan A: subscriptions work end-to-end against opcgw at HEAD with zero `src/` changes.** The subscription delivery path is already wired by async-opcua 0.17.1's `SimpleNodeManagerImpl` against opcgw's existing `add_read_callback` registrations. **8-2 is a config-plumbing story, not a subscription-engine story.**

The work breaks into four discrete pieces:

1. **Plumb four `Limits` knobs** through `OpcUaConfig` → `ServerBuilder` so operators can shape subscription/message-size load — `max_subscriptions_per_session`, `max_monitored_items_per_sub` (note: library field name is `_per_sub`, NOT `_per_subscription` as `epics.md:682, 724` says), `max_message_size`, `max_chunk_count`. Defaults come from async-opcua's library defaults (10, 1000, runtime-resolved, runtime-resolved). Validation, env-var override, hard caps, and TOML template updates follow the **exact pattern Story 7-3 established for `max_connections`** (single source of truth in `src/utils.rs`, `Option<usize>` shape, `validate()` accumulator entry, hand-written `Debug` field, env var via figment's `__`-split, commented-out TOML default block).
2. **Add a second tier of "should-expose" advanced knobs** for operators tuning subscription-flood scenarios — `max_pending_publish_requests`, `max_publish_requests_per_subscription`, `min_sampling_interval_ms`, `max_keep_alive_count`, `max_queued_notifications`. Same plumbing pattern, but documented as "advanced; default usually fine" to avoid scaring default operators.
3. **Pin the subscription contract with integration tests** — subscription-flood from a single client (verifies `max_subscriptions_per_session` enforcement), monitored-item-flood from a single subscription (verifies `max_monitored_items_per_sub` enforcement), and a "subscription survives ChirpStack outage" smoke (`epics.md:721`). The Story 8-1 spike's existing 9 tests in `tests/opcua_subscription_spike.rs` carry forward as the regression baseline.
4. **Documentation** — extend `docs/security.md` "OPC UA connection limiting" section with subscription-knob coverage; bump `README.md` Configuration block; sync the Planning table; subscribe-client audit-trail one-liner reflecting the NFR12 carry-forward acknowledgment (no new audit infrastructure).

The new code surface is **small** — estimate **~200–350 LOC of production code + ~250–400 LOC of tests + ~120 LOC of docs**. The subscription engine itself is unchanged: zero new structs, zero new traits, zero new tasks, zero new modules. Just config plumbing into `ServerBuilder` and tests that pin the resulting limit-enforcement contract.

This story closes **FR21** (subscription-based data change notifications) and the carry-forward at `epics.md:682, 724` (subscription / message-size limits as config knobs, GitHub issue #89). It does **not** ship historical data (Story 8-3, FR22) or alarm conditions (Story 8-4, FR23).

---

## Out of Scope

- **Push-model implementation.** Story 8-1 spike § 6 confirmed the pull-via-`add_read_callback` + `SyncSampler` model is sufficient at the Phase B sizing target (100 monitored items × 1 Hz × 1 subscriber). The operator's pending `--load-probe` 5-minute run (issue #95) is the trigger for re-evaluation: if p99 inter-notification interval > 3000 ms or drop rate > 5% of expected, 8-2 must reopen the push-model decision per spike report § 6's decision rule. **Until those numbers exist, this story plans for pull.** A push-model integration sketch lives in spike report § 6 for completeness; do not implement it unless the load-probe forces the issue. Tracked at GitHub issue #95.
- **Per-IP rate limiting / token-bucket throttling.** Phase A carry-forward at `epics.md:728`; tracked at GitHub issue #88. The flat global `max_connections` from Story 7-3 plus the per-session `max_subscriptions_per_session` and `max_monitored_items_per_sub` knobs from this story constitute the load-shaping surface in 8-2. Per-IP throttling is a separate Phase B story to be opened only if subscription-flood becomes a near-term operator concern.
- **Hot-reload of the new knobs at runtime.** Like all other `OpcUaConfig` fields, the new knobs are read at startup. Phase B Epic 9 hot-reload (`epics.md:Story 8.7`, `_bmad-output/implementation-artifacts/sprint-status.yaml::9-7-configuration-hot-reload`) covers runtime reconfiguration. Tracked at GitHub issue #90 alongside `max_connections` hot-reload.
- **OPC UA HistoryRead / historical data access.** Story 8-3 owns this (FR22). 8-2 must not touch `metric_history`-backed paths.
- **Threshold-based alarm conditions.** Story 8-4 owns this (FR23). 8-2 must not touch status-code propagation beyond what's already wired (Story 5-2's stale-data status codes flow through subscription notifications unchanged — the spike confirmed this empirically).
- **Event-type monitored items** (`OnSubscriptionNotification::on_event`). Out of scope; Story 8-4 territory.
- **Modify subscription / set publishing mode / transfer subscription / modify monitored items services.** Spike report § 4 marks these as "validated by source-grep but not exercised live." async-opcua already implements them in `SimpleNodeManagerImpl`; 8-2 does not need new wiring. If a SCADA client exercises them, they will work. 8-2 does not add tests for them — the AC#1 / AC#2 / AC#3 paths are the contract.
- **Multi-user OPC UA token model / mTLS / CA-signed cert workflow.** Tracked at GitHub issues #85 / Story 7-2 deferred entries. 8-2 keeps the single-user model and the existing endpoint set unchanged.
- **First-class session-rejected callback in async-opcua.** Story 8-1 AC#7 re-confirmed no hook in 0.17.1 — tracked at GitHub issue #94 (operator action: file the upstream FR). Story 7-3's `AtLimitAcceptLayer` workaround stays unchanged. 8-2 does not retire the layer; 8-2's auth + cap composition test in `tests/opcua_subscription_spike.rs` (already present from 8-1) is the regression baseline.
- **`OPCUA_SESSION_GAUGE_INTERVAL_SECS` promotion to `[diagnostics].session_gauge_interval_secs`** (the conditional AC at `epics.md:727`). The decision is **conditional on the operator's `--load-probe` numbers** (issue #95). Until those numbers exist, the constant stays. If the load probe surfaces noise-buried gauge ticks, 8-2 promotes the knob with the same plumbing pattern as the four mandatory knobs; if signal-clear, 8-2 records "gauge stays hard-coded" in deferred-work.md and moves on. The default behaviour for this story is **constant stays** unless the operator-supplied data triggers otherwise.
- **Manual FUXA + Ignition / UaExpert verification.** Per the user's 2026-04-30 decision (sprint-status `last_updated`), manual SCADA verification is batched into a single integration pass after Epic 9 lands. Tracked at GitHub issue #93. 8-2's contract is **automated tests only**; the manual SCADA round happens once Phase B is complete end-to-end.
- **Doctest cleanup.** Carry-forward debt; tracked as a separate story before Epic 9.

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. The story's job is to **plumb new knobs through code that already does the heavy lifting** — the subscription engine, the limits struct, the validation accumulator, the env-var convention, the test harness all exist.

| What | Where | Status |
|------|-------|--------|
| **Subscription engine wired today, zero `src/` changes** | Library: `async-opcua-server-0.17.1/src/node_manager/memory/simple.rs:127, 132–144, 180–262`. opcgw read callbacks: `src/opc_ua.rs:723` (read metrics), `:810` (gateway/cp0), `:872, :880, :888` (gateway-folder fields) | **Confirmed empirically by Story 8-1 spike (2026-04-29).** `SimpleNodeManagerImpl` implements all four monitored-item lifecycle hooks (`create_value_monitored_items`, `modify_monitored_items`, `set_monitoring_mode`, `delete_monitored_items`) and auto-wires a `SyncSampler` against opcgw's existing `add_read_callback` registrations. `CreateSubscription` → `CreateMonitoredItems` → `Publish` → `DataChangeNotification` flows end-to-end at HEAD with **no production code changes**. The sampler invokes the same closure the `Read` service uses, so Story 5-2's stale-status-code logic (TTL handling, `Bad`/`Uncertain`/`Good`) flows through subscription notifications unchanged. **Story 8-2 must NOT add new node-manager wiring, new sampler logic, or new subscription-side code in `src/opc_ua.rs`** beyond the `configure_limits` extension and the new startup info log per AC#2. |
| **Spike binary + 9 integration tests** | `examples/opcua_subscription_spike.rs`, `tests/opcua_subscription_spike.rs` | **Carry-forward from Story 8-1.** All 9 tests pass as of 2026-04-30 (test count: 592 pass / 0 fail / 7 ignored). The auth + cap composition tests (`test_subscription_client_rejected_by_auth_manager`, `test_subscription_client_rejected_by_at_limit_layer`) are the **NFR12 regression baseline** for Story 8-2 — must continue to pass. Story 8-2 **adds** tests to this file, does NOT replace it. |
| **Test harness** | `tests/opcua_subscription_spike.rs:59-373` | **Reusable today.** `TEST_USER`, `TEST_PASSWORD`, `setup_test_server_with_max(max)` (lines 218-271, takes `usize` connection cap), `HeldSession` (309-326, RAII session wrapper with explicit async `disconnect()` to avoid Drop-ordering issues), `open_session_held` (334-373, returns `Option<HeldSession>` with auth retries), `user_password_identity()` / `wrong_password_identity()` helpers (~290-308). **Story 8-2 reuses these inline** per CLAUDE.md scope-discipline rule. The "fourth integration-test file" threshold for `tests/common/` extraction has not been crossed. |
| **`ServerBuilder::limits_mut()` — the only path to subscription-limit fields** | `~/.cargo/registry/src/index.crates.io-*/async-opcua-server-0.17.1/src/builder.rs:246` | Returns `&mut Limits`. **Subscription-related fields have no direct setter** — `max_subscriptions_per_session`, `max_monitored_items_per_sub`, `max_pending_publish_requests`, `max_publish_requests_per_subscription`, `min_sampling_interval_ms`, `max_keep_alive_count`, `max_queued_notifications` all reachable only via `limits_mut().subscriptions.<field> = N`. Pattern: `let server_builder = self.configure_subscription_limits(server_builder);` extending the existing `configure_limits` style from Story 7-3. **Do NOT pass a fully-built `Limits` struct via `.limits(...)`** — that overrides every default and risks regressions on fields this story doesn't touch (per spike report § 11 item 2). |
| **`ServerBuilder` direct setters for `max_message_size` and `max_chunk_count`** | `builder.rs:380, 422, 428, 439, 460–530` | `.max_message_size(usize)` and `.max_chunk_count(usize)` are direct setters — preferred. Mix the two patterns in one `configure_subscription_limits` method: direct setters for these two, `limits_mut()` for the others. |
| **Library defaults (resolved at runtime, NOT hardcoded literals in spec)** | `~/.cargo/registry/src/index.crates.io-*/async-opcua-server-0.17.1/src/lib.rs:60–145` + `opcua_types::constants` | `MAX_SUBSCRIPTIONS_PER_SESSION = 10`, `DEFAULT_MAX_MONITORED_ITEMS_PER_SUB = 1000`, `MAX_PENDING_PUBLISH_REQUESTS = 20`, `MAX_PUBLISH_REQUESTS_PER_SUBSCRIPTION = 4`, `MIN_SAMPLING_INTERVAL_MS = 100.0` (= `SUBSCRIPTION_TIMER_RATE_MS`), `MAX_KEEP_ALIVE_COUNT = 30000`, `MAX_QUEUED_NOTIFICATIONS = 20`. `max_message_size` / `max_chunk_count` defaults come from `opcua_types::constants` (separate crate; resolved at runtime — read off `ServerBuilder::config().limits` if needed for log emission). **Story 8-2 doesn't hardcode the library defaults; the gateway defaults match the library defaults so unsetting in TOML is identical to the library behaviour.** |
| **`OpcUaConfig` struct + `Debug` redaction matrix** | `src/config.rs:148-262, 287-309` | **Wired today.** Adding a new field requires: (a) the `Option<...>` field on the struct, (b) the `Debug` impl listing it (or Story 7-1's NFR7 redaction matrix is silently broken), (c) optional validation in `AppConfig::validate`. Story 7-3 added `max_connections: Option<usize>` end-to-end (`src/config.rs:250-261, :306`) — that's the precedent. |
| **`AppConfig::validate` accumulator pattern** | `src/config.rs:739-` (entry point), `:802-841` (existing OpcUa block including `max_connections` checks) | **Wired today.** Validation errors accumulate into a `Vec<String>` and `validate()` returns one combined `OpcGwError::Configuration` listing every violation. Story 8-2 appends new checks for each new knob using the same pattern: `Some(0)` rejected with actionable hint, `Some(n) > HARD_CAP` rejected with hint. |
| **Env-var override convention** | figment + `Env::prefixed("OPCGW_").split("__")` (Story 7-1) | `OPCGW_OPCUA__MAX_SUBSCRIPTIONS_PER_SESSION=20` overrides `[opcua].max_subscriptions_per_session` automatically — figment maps the `__`-split path. **No code change required.** Spec only needs to document the env var names per knob. |
| **Constants pattern in `src/utils.rs`** | `src/utils.rs:104, 114, 122` | **Wired today.** `OPCUA_DEFAULT_MAX_CONNECTIONS = 10`, `OPCUA_MAX_CONNECTIONS_HARD_CAP = 4096`, `OPCUA_SESSION_GAUGE_INTERVAL_SECS = 5`. Story 8-2 adds 8 new constants (4 mandatory knobs × 2 each = 8, plus 5 advanced × 2 = 10 if the advanced tier ships). Each constant gets a one-paragraph doc comment matching the project tone; the literal value lives **only** here so future grep/search finds the single source of truth. |
| **`OpcUa::create_server` integration point** | `src/opc_ua.rs:168-244` | **Wired today.** `ServerBuilder` is built up across `configure_network` (`:279`) / `configure_limits` (`:316`, currently only sets `max_sessions`) / `configure_key` / `configure_user_token` / `with_authenticator` / `configure_end_points`. **Story 8-2 extends `configure_limits` in place** — it's already named for the role and contains the only existing `Limits` plumbing. Add the four mandatory knobs + (optionally) the five advanced knobs to its body. **Do not add a new method** — the symmetry with `configure_network` is preserved. |
| **`OpcUa::max_sessions` single-source-of-truth helper** | `src/opc_ua.rs:328-333` | **Established pattern from Story 7-3.** A small helper that resolves `Option<usize> → usize` with the `unwrap_or(DEFAULT)` boilerplate in one place. Story 8-2 follows the same shape: one `fn max_<knob_name>(&self) -> <T>` per knob, all reading from `self.config.opcua` and falling back to `OPCUA_DEFAULT_*`. |
| **`tests/config/config.toml` test fixture** | `tests/config/config.toml` | **Used by integration tests** that construct `AppConfig` from this file. Adding new optional fields with `None` defaults does not break compatibility (figment treats absent keys as `None`). The fixture itself does not need updates unless 8-2's tests want to set non-default values for assertions. |
| **`OpcUaConfig` literal sites — exactly 5 to update** | Audited 2026-04-30 via `grep -rn 'OpcUaConfig {' tests/ src/`: (1) `tests/opcua_subscription_spike.rs:179`, (2) `src/opc_ua_auth.rs:458` (test module inside the file), (3) `src/config.rs:2219` (existing `config::tests`), (4) `tests/opc_ua_security_endpoints.rs:156`, (5) `tests/opc_ua_connection_limit.rs:179` | **All 5 sites must add `<field>: None` for each new field**, regardless of `#[serde(default)]` choice — that annotation only affects TOML deserialisation, NOT Rust struct-literal construction. The `#[serde(default)]` annotation is still recommended on each new field for forward-compatibility with future TOML fixtures (Story 7-3's `max_connections` did NOT use it and required updating TOML fixtures too — that's the precedent's pain point Story 8-2 should avoid). Diff per literal site: 4 new lines (one `<field>: None,` per knob). Total literal-site touch: ~20 lines across 5 files. |
| **`OpcgwAuthManager` + `AtLimitAcceptLayer` invariants** | `src/opc_ua_auth.rs` (Story 7-2), `src/opc_ua_session_monitor.rs` (Story 7-3) | **Wired today, regression-pinned.** Subscription clients pass through both gates without modification — Story 8-1 spike AC#9 confirmed empirically. **Story 8-2 must NOT modify these files.** The two existing tests in `tests/opcua_subscription_spike.rs` (`test_subscription_client_rejected_by_auth_manager`, `test_subscription_client_rejected_by_at_limit_layer`) are the regression baseline. |
| **NFR12 startup-warn (commit `344902d`)** | `src/main.rs::initialise_tracing` | **Wired today.** Emits a one-shot `warn!` when global level filters out info — the precondition for source-IP audit correlation. Story 8-2 consumes this unchanged; the existing test (if any) for the warn stays as the regression baseline. **Story 8-2 must NOT re-implement.** |
| **Tracing event-name convention** | Stories 6-1, 7-2, 7-3 (`opcua_auth_failed`, `opcua_session_count`, `opcua_session_count_at_limit`, `pki_dir_initialised`) | **Established.** Two flavours of structured event share the `event = "snake_case_name"` field: **audit / state-change events** (auth failed, session rejected — operator-visible security-relevant) and **diagnostic / startup-config events** (PKI init, limits-resolved — operator-visible diagnostics). The NFR12 carry-forward acknowledgment at `epics.md:705` forbids new **audit** events; **Story 8-2 introduces ONE new diagnostic event (`opcua_limits_configured`, AC#2) and ZERO new audit events.** AC#4 + AC#8's count checks exclude diagnostic events. |
| **`OpcGwError::Configuration` / `OpcGwError::OpcUa` variants** | `src/utils.rs::OpcGwError` | Use `Configuration` for startup validation failures (out-of-range knob values); use `OpcUa` for runtime server errors. **Do not introduce a new variant.** |
| **Documentation extension target** | `docs/security.md:537-659` (existing "OPC UA connection limiting" section) | **Existing section.** Story 8-2 extends this section with four-knob (and advanced-knob) coverage, NOT a new top-level section. The section's existing structure (What it is / Configuration / What you'll see in the logs / Anti-patterns / Tuning checklist / What's out of scope) carries forward — 8-2 inserts knob-specific content into each subsection. |

**Epic-spec coverage map** — the BDD acceptance criteria from `epics.md` (lines 707–728) break down as:

| Epic-spec criterion (line ref) | Already satisfied? | Where this story addresses it |
|---|---|---|
| Subscription clients receive data change notifications when poller updates metric values (line 717) | ✅ Plan A confirmed by spike (`tests/opcua_subscription_spike.rs::test_subscription_basic_data_change_notification`) | **AC#3** — pin a richer regression test for value-flow under multiple monitored-item types. Use the spike's existing test as the baseline and extend with multi-`MetricType` coverage (Float / Int / Bool — String deferred per spike note). |
| Poller pushes DataValues with timestamps + status codes after each cycle (line 718) | ✅ Pull model proven sufficient by spike (no push channel needed) | **AC#3** — pin that the existing `add_read_callback` path delivers DataValues with the Story 5-2 stale-status-code logic intact. |
| Multiple clients subscribe to same variables simultaneously (line 720) | ✅ Pinned by spike (`test_subscription_two_clients_share_node`) | **No new test** — the spike's test stays as the regression. |
| Subscriptions survive ChirpStack outages (stale status, not drops) (line 721) | ⚠️ Strong prior: yes via Story 5-2's stale-status path; not pinned by spike | **AC#3** — new integration test: simulate a ChirpStack outage (or feed it via the `chirpstack_status` storage backend), assert subscriptions stay open and status codes transition to `Uncertain`/`Bad` per Story 5-2. |
| Tested with FUXA and at least one additional client (line 722, NFR22) | ⏸️ Deferred to post-Epic-9 single integration pass | **No 8-2 work** — issue #93 carries this; 8-2 ships automated-test-only. |
| FR21 satisfied (subscription-based data change notifications) (line 723) | ✅ via AC#3 | **AC#3** is the FR21 closer. |
| Four-knob `Limits` config surface (line 724, issue #89) | ❌ no knobs today | **AC#1** + **AC#2** — knob plumbing + ServerBuilder wiring. |
| Four config knobs documented in `docs/security.md` "OPC UA connection limiting" (line 725) | ❌ docs cover `max_connections` only | **AC#5** — extends `docs/security.md`. |
| `AppConfig::validate` rejects zero / negative / above-hard-cap values with same accumulation pattern (line 725) | ✅ pattern exists; needs new entries | **AC#1** verification adds the unit tests. |
| Subscription clients pass through `OpcgwAuthManager` + `AtLimitAcceptLayer` without modification (line 726, NFR12 carry-forward) | ✅ confirmed by Story 8-1 spike AC#9 | **AC#4** — keep the two existing spike-tests as the regression baseline; do NOT modify the auth or monitor modules. |
| Wrong-password subscription-creating client integration test (line 726) | ✅ already exists in `tests/opcua_subscription_spike.rs` | **No new code** — the test stays. |
| `OPCUA_SESSION_GAUGE_INTERVAL_SECS` promotion (conditional, line 727, `epics.md:683`) | ⏸️ Conditional on operator's `--load-probe` numbers (issue #95) | **AC#7** — conditional path: if numbers exist and gauge is noise-buried, promote; otherwise constant stays. Default behaviour: constant stays. |
| Per-IP rate limiting reminder (out-of-scope, line 728, issue #88) | n/a | **No code** — recorded in Out of Scope above and in deferred-work.md update (AC#5). |
| `cargo test` clean + `cargo clippy --all-targets -- -D warnings` clean | Implicit per CLAUDE.md | **AC#6** — Story 8-1 baseline 592 pass / 0 fail / 7 ignored; Story 8-2 target ≥ 600 pass with the new knob-enforcement and ChirpStack-outage tests added. |

---

## Acceptance Criteria

### AC#1: Four mandatory `Limits` knobs are configurable via `config.toml` and env var (FR21, `epics.md:724`, issue #89)

**Knob list** (all `Option<usize>` / `Option<f64>` for `min_sampling_interval_ms` — see field-shape table below; library defaults from `lib.rs:60–145` apply when `None`):

| Knob | TOML key | Library default | Env var | Hard cap | Rationale |
|---|---|---|---|---|---|
| `max_subscriptions_per_session` | `[opcua].max_subscriptions_per_session` | 10 | `OPCGW_OPCUA__MAX_SUBSCRIPTIONS_PER_SESSION` | 1000 | A SCADA client typically wants 1-5 subscriptions. 1000 is "deployment review needed" — a misbehaving client creating 1000+ subscriptions would saturate the publish pipeline and is the threat model for this knob. |
| `max_monitored_items_per_sub` | `[opcua].max_monitored_items_per_sub` | 1000 | `OPCGW_OPCUA__MAX_MONITORED_ITEMS_PER_SUB` | 100000 | Phase B sizing target is 100/subscription. 100000 is "deployment review needed" — the address space at typical opcgw deployments has ~10-1000 nodes total; per-subscription capacity above 100000 is structurally impossible. |
| `max_message_size` | `[opcua].max_message_size` | runtime-resolved from `opcua_types::constants` | `OPCGW_OPCUA__MAX_MESSAGE_SIZE` | 268435456 (256 MiB) | Default is conservative (typical 65535 = 64 KiB). 256 MiB hard cap protects against memory-exhaustion DoS via `Read` of a forged "large" array; default deployments never approach this. |
| `max_chunk_count` | `[opcua].max_chunk_count` | runtime-resolved from `opcua_types::constants` | `OPCGW_OPCUA__MAX_CHUNK_COUNT` | 4096 | Default is conservative. 4096 chunks = `4096 × max_chunk_size` total message ceiling; values above signal a misconfiguration. |

**Field-shape table** — exactly mirroring Story 7-3's `max_connections` pattern:

| Field | Type | Source-of-truth constant in `src/utils.rs` |
|---|---|---|
| `max_subscriptions_per_session` | `Option<usize>` | `OPCUA_DEFAULT_MAX_SUBSCRIPTIONS_PER_SESSION: usize = 10`, `OPCUA_MAX_SUBSCRIPTIONS_PER_SESSION_HARD_CAP: usize = 1000` |
| `max_monitored_items_per_sub` | `Option<usize>` | `OPCUA_DEFAULT_MAX_MONITORED_ITEMS_PER_SUB: usize = 1000`, `OPCUA_MAX_MONITORED_ITEMS_PER_SUB_HARD_CAP: usize = 100_000` |
| `max_message_size` | `Option<usize>` | `OPCUA_DEFAULT_MAX_MESSAGE_SIZE: usize = <verify-at-impl>`, `OPCUA_MAX_MESSAGE_SIZE_HARD_CAP: usize = 268_435_456` (256 MiB) |
| `max_chunk_count` | `Option<usize>` | `OPCUA_DEFAULT_MAX_CHUNK_COUNT: usize = <verify-at-impl>`, `OPCUA_MAX_CHUNK_COUNT_HARD_CAP: usize = 4096` |

**Resolving the `max_message_size` / `max_chunk_count` defaults at implementation time.** The library defaults for these two knobs come from `opcua_types::constants` (a separate crate, resolved at runtime — not validated by the Story 8-1 spike per spike report § 5). The dev agent **must** resolve the actual runtime values before picking gateway defaults. Two acceptable approaches:

1. **Match library exactly** (recommended). Read off the resolved value from a fresh `ServerBuilder::new()` via `builder.config().limits.max_message_size` (and `max_chunk_count`) at the start of implementation; capture the values; use those as `OPCUA_DEFAULT_*`. If the library chooses 65535, gateway uses 65535. If 1048576, gateway uses 1048576. Operator can override via env var. This makes "unsetting in TOML" a true no-op against the library.
2. **Pick conservative explicit defaults** (only if approach 1 surfaces a value the dev agent flags as unsuitable). E.g., `64 * 1024 = 65536` (64 KiB) for messages, `256` for chunks. Document the divergence in `src/utils.rs` doc comment with reasoning.

**The dev agent records the resolved library values and the chosen gateway defaults in Dev Notes Completion Notes** so future readers can audit the choice. **Do NOT ship `<verify-at-impl>` placeholders** — replace with the chosen literals.

For the two subscription-relevant knobs (`max_subscriptions_per_session`, `max_monitored_items_per_sub`), the gateway defaults **match** the library defaults exactly (10, 1000) so unsetting in TOML is a no-op; the explicit constants are still documented in `src/utils.rs` so future readers find the single source of truth.

**Field-name asymmetry.** The library field is `max_monitored_items_per_sub` (NOT `max_monitored_items_per_subscription` as `epics.md:682, 724` and Story 7-3's deferred-work entry name it). The Rust field, the TOML key, and the env-var name **all use the library name** — `max_monitored_items_per_sub`. Story 8-1 spike § 5 + § 11 item 7 documents this rename; the spec body of 8-2 (this file) uses the library name throughout.

**Implementation specifics:**

- Add the four `Option<...>` fields to `OpcUaConfig` in `src/config.rs:148-262`, placed after the existing `max_connections` field (`:261`) for chronological ordering with the `Limits`-related knobs grouped. Each gets a doc comment matching the project's existing tone (purpose / range / env-var override note / library-default reference).
- **Mandatory:** update `impl Debug for OpcUaConfig` (`src/config.rs:287-309`) to include `.field("max_subscriptions_per_session", &self.max_subscriptions_per_session)` etc. — Story 7-1's NFR7 redaction matrix is broken if any new field is omitted from `Debug`. None of the four new knobs are secrets, so they emit at face value (no `REDACTED_PLACEHOLDER` substitution).
- Add the **eight constants** above to `src/utils.rs` next to the existing `OPCUA_DEFAULT_*` constants. Each constant gets a one-paragraph doc comment naming the AC# (Story 8-2, AC#1) and citing the library default + the source path (`async-opcua-server-0.17.1/src/lib.rs:NN`) it mirrors.
- Extend `AppConfig::validate` (`src/config.rs:739-`) with **eight new accumulator entries** (one `Some(0)` rejection + one `Some(n) > HARD_CAP` rejection per knob), placed after the existing `max_connections` block (`:809-841`). Use the exact same wording pattern: actionable hint pointing at the env-var name. Example for `max_subscriptions_per_session`:
  ```
  "opcua.max_subscriptions_per_session: must be at least 1 (use a small positive integer like 1 to enforce single-subscription mode; 0 would refuse all subscriptions including operators)"
  "opcua.max_subscriptions_per_session: {n} exceeds hard cap of {HARD_CAP}. Either lower the value or open a follow-up issue if your deployment really needs more (the cap protects against subscription-flood DoS)"
  ```
- Extend `config/config.toml` with a commented-out default block placed after the existing `max_connections` block (`:104-111`):
  ```toml
  # OPC UA subscription / message-size limits (Story 8-2). Surface async-opcua's
  # ServerBuilder limits as config knobs so operators can shape subscription/
  # message-size load. All values default to the library defaults; uncomment
  # only if a specific deployment scenario requires tuning.
  #
  # The two literal defaults below (max_subscriptions_per_session = 10,
  # max_monitored_items_per_sub = 1000) match async-opcua 0.17.1's library
  # defaults at lib.rs:73, 75. The other two (<replace-at-impl>) must be
  # filled in by the dev agent at implementation time — they are resolved
  # at runtime from opcua_types::constants and the dev agent reads off the
  # actual values via ServerBuilder::new().config().limits.
  #
  # max_subscriptions_per_session = 10                  # Range: 1-1000
  # max_monitored_items_per_sub = 1000                  # Range: 1-100000
  # max_message_size = <replace-at-impl>                # Range: 1-268435456 (256 MiB)
  # max_chunk_count = <replace-at-impl>                 # Range: 1-4096
  #
  # Override via env vars: OPCGW_OPCUA__<UPPERCASE_KEY>
  ```
  **Do NOT ship `<replace-at-impl>` placeholders in the final commit** — the dev agent resolves the runtime values, picks gateway defaults to match (or diverges with a documented reason), and replaces the placeholders with the chosen literals.
- Extend `config/config.example.toml` (verified to exist at `/home/gcorbaz/Synology/devel/opcgw/config/config.example.toml` as of 2026-04-30) with the same block.
- Update `tests/config/config.toml` (and any other test fixture used by deserialisation tests) **only if** integration tests pin the exact `OpcUaConfig` field set; absent fields with `Option` types deserialise as `None` so most fixtures need no change.
- **Update all 5 `OpcUaConfig { ... }` literal sites** (audited 2026-04-30 — count is 5, not "at least one"; see Existing Infrastructure table for the explicit list). At each site, add `max_subscriptions_per_session: None,`, `max_monitored_items_per_sub: None,`, `max_message_size: None,`, `max_chunk_count: None,`. Total touch: ~20 lines across 5 files. **Also add `#[serde(default)]` on each new field declaration** in `src/config.rs` for forward-compatibility with future TOML fixtures (does NOT affect literal-site requirement, but does keep TOML omission working — see "Why `#[serde(default)]`" Dev Notes section).

**Verification:**
- `grep -nE 'max_subscriptions_per_session|max_monitored_items_per_sub|max_message_size|max_chunk_count' src/config.rs` returns **≥ 12 hits** (4 struct fields + 4 Debug impl entries + 4×2 = 8 validate accumulator entries = 16+).
- `grep -nE 'OPCUA_DEFAULT_(MAX_SUBSCRIPTIONS_PER_SESSION|MAX_MONITORED_ITEMS_PER_SUB|MAX_MESSAGE_SIZE|MAX_CHUNK_COUNT)|OPCUA_(MAX_SUBSCRIPTIONS_PER_SESSION|MAX_MONITORED_ITEMS_PER_SUB|MAX_MESSAGE_SIZE|MAX_CHUNK_COUNT)_HARD_CAP' src/utils.rs` returns **8 hits** (4 default + 4 hard-cap).
- `grep -nE 'max_subscriptions_per_session|max_monitored_items_per_sub|max_message_size|max_chunk_count' config/config.toml` returns **≥ 4 hits**.
- Unit test `test_validation_rejects_max_subscriptions_per_session_zero` — `Some(0)`, assert `Err` containing `"max_subscriptions_per_session"` and `"at least 1"`.
- Unit test `test_validation_rejects_max_subscriptions_per_session_above_hard_cap` — `Some(1001)`, assert `Err` containing `"max_subscriptions_per_session"` and `"hard cap"` and `"1000"`.
- Unit test `test_validation_accepts_max_subscriptions_per_session_at_hard_cap` — `Some(1000)`, assert `Ok`.
- Unit test `test_validation_accepts_max_subscriptions_per_session_none` — `None`, assert `Ok`.
- Unit test `test_validation_accepts_max_subscriptions_per_session_one` — `Some(1)`, assert `Ok` (single-subscription lockdown).
- **Repeat the five-test pattern for each of the other three knobs** (`max_monitored_items_per_sub`, `max_message_size`, `max_chunk_count`) — total: **20 unit tests** for AC#1 validation.
- `cargo test --lib --bins config::tests::test_validation_` runs the new tests; expect 20 new passes plus all pre-existing `test_validation_*` tests still passing.

### AC#2: `ServerBuilder` limit configuration is wired through `configure_limits` (FR21)

- Extend `OpcUa::configure_limits` (`src/opc_ua.rs:316-320`) — currently only sets `max_sessions(N)` — to also wire the four new knobs. Body skeleton:
  ```rust
  fn configure_limits(&self, server_builder: ServerBuilder) -> ServerBuilder {
      let max_sessions = self.max_sessions();
      let max_subs_per_session = self.max_subscriptions_per_session();
      let max_items_per_sub = self.max_monitored_items_per_sub();
      let max_message_size = self.max_message_size();
      let max_chunk_count = self.max_chunk_count();
      debug!(
          max_sessions,
          max_subs_per_session,
          max_items_per_sub,
          max_message_size,
          max_chunk_count,
          "Configure session and subscription limits"
      );
      let mut server_builder = server_builder
          .max_sessions(max_sessions)
          .max_message_size(max_message_size)
          .max_chunk_count(max_chunk_count);
      // Subscription-limit fields have no direct setter — must use limits_mut().
      {
          let limits = server_builder.limits_mut();
          limits.subscriptions.max_subscriptions_per_session = max_subs_per_session;
          limits.subscriptions.max_monitored_items_per_sub = max_items_per_sub;
      }
      server_builder
  }
  ```
- Add four new helper methods on `OpcUa` mirroring the existing `max_sessions` pattern (`src/opc_ua.rs:328-333`) — single source of truth for "what limit will be enforced", read by both `configure_limits` and any logging site. Place each right after the existing `max_sessions` helper:
  ```rust
  fn max_subscriptions_per_session(&self) -> usize {
      self.config
          .opcua
          .max_subscriptions_per_session
          .unwrap_or(crate::utils::OPCUA_DEFAULT_MAX_SUBSCRIPTIONS_PER_SESSION)
  }
  // (and similar for max_monitored_items_per_sub, max_message_size, max_chunk_count)
  ```
- **Add a new startup info log** with the resolved values, placed in `OpcUa::run` (`src/opc_ua.rs`) **after** `set_session_monitor_state(...)` at `:594` and **before** the `tokio::spawn` of the gauge loop at `:596`. There is **no existing startup info log emitting `max_sessions` as a structured field today** — the only `max_sessions` emit is `debug!(max_sessions = %max, "Configure session limit")` at `:318` (compile-time-default debug, often filtered in production). The new info log shape:
  ```rust
  info!(
      event = "opcua_limits_configured",
      max_sessions,
      max_subscriptions_per_session,
      max_monitored_items_per_sub,
      max_message_size,
      max_chunk_count,
      "OPC UA limits configured"
  );
  ```
  **Audit-event vs diagnostic-event distinction:** `event="opcua_limits_configured"` is a **diagnostic / startup-config event**, not an audit event. The NFR12 carry-forward acknowledgment at `epics.md:705, 726` ("no new audit infrastructure introduced by this story") forbids new **audit** events (failed-auth, session-rejected, etc.); a one-shot startup-config emit is the same shape as Story 7-2's `event="pki_dir_initialised"` and is allowed. AC#4 + AC#8's "no new `event=` values" rule applies to **audit events specifically** — clarify there.
- **Do NOT introduce a new method** like `configure_subscription_limits` — `configure_limits` is the existing role-named method; extending it preserves the symmetry with `configure_network` / `configure_key` / `configure_user_token`. The call-site in `create_server` (`src/opc_ua.rs:206`) is unchanged.
- **Do NOT pass a fully-built `Limits` struct via `.limits(...)`** — that overrides every default and risks regressions on fields this story doesn't touch (per spike report § 11 item 2). The mixed direct-setter + `limits_mut()` pattern above is the contract.
- **`limits_mut()` borrow-checker note:** the inner block scope `{ let limits = server_builder.limits_mut(); ... }` is required because `limits_mut` returns `&mut Limits` and the next chained method call (`.max_message_size(...)`) takes `self` by value. Either chain `limits_mut()` after all the direct setters (as above) or split into separate let-bindings if the borrow-checker complains. The exact shape is a Rust ergonomics detail — both work; pick the one that compiles cleanly.

**Verification:**
- `grep -n 'configure_limits\|limits_mut\|max_subscriptions_per_session\|max_monitored_items_per_sub\|max_message_size\|max_chunk_count' src/opc_ua.rs` returns **≥ 12 hits** (4 helper methods + ≥ 4 in `configure_limits` body + ≥ 4 in startup log).
- Unit test `test_configure_limits_uses_defaults_when_none` (in `src/opc_ua.rs::tests` if such a test module exists, otherwise skip — `configure_limits` is hard to unit-test without spinning up a real server; the integration tests in AC#3 are the real verification).
- Integration test `test_resolved_limits_logged_at_startup` in `tests/opcua_subscription_spike.rs` — start the gateway with explicit knobs set (e.g., `max_subscriptions_per_session = Some(20)`), capture the startup info log, assert the resolved values are visible. Reuses the existing `tracing-test` capture buffer pattern.

### AC#3: Subscription limit enforcement and survival contract (FR21, `epics.md:720-721`)

Three new integration tests in `tests/opcua_subscription_spike.rs` (additive — the existing 9 tests stay):

#### AC#3.1: Subscription-flood pinned by `max_subscriptions_per_session` enforcement

- **Test:** `test_subscription_flood_capped_by_max_subscriptions_per_session`.
- **Given** the gateway running with `max_subscriptions_per_session = Some(2)` (small enough to keep the test fast; larger than 1 so the (cap+1)th attempt is unambiguously the rejected one).
- **When** a single authenticated client opens 2 subscriptions in quick succession, then attempts a 3rd.
- **Then** the first 2 subscriptions activate within 5 s. The 3rd `CreateSubscription` call must fail with `BadTooManySubscriptions` (per OPC UA spec) within 5 s.
- **And** the test does NOT assert any new tracing event — async-opcua's enforcement is silent (`SubscriptionService::create_subscription` returns the error without logging). The error code is the contract.
- **Verification:** test passes, captured log buffer does NOT contain any new audit event (NFR12 carry-forward acknowledgment — no new audit infrastructure).
- **Wall clock target:** < 10 s. Mark `#[serial_test::serial]`.

#### AC#3.2: Monitored-item-flood pinned by `max_monitored_items_per_sub` enforcement

- **Test:** `test_monitored_item_flood_capped_by_max_monitored_items_per_sub`.
- **Given** the gateway running with `max_monitored_items_per_sub = Some(3)` (small for fast test).
- **When** a single authenticated client opens 1 subscription, then calls `CreateMonitoredItems` with 3 valid `MonitoredItemCreateRequest`s in one call (all 3 must succeed), then attempts a 4th `CreateMonitoredItems` with 1 more request on the same subscription.
- **Then** the first call returns 3 results all with `Good` status. The second call must enforce the cap via one of three observed shapes (any is acceptable; document the observed shape in Completion Notes):
  1. **Per-item rejection.** The call returns one result with `BadTooManyMonitoredItems` per OPC UA spec. The call itself succeeds; the per-item result carries the rejection.
  2. **Service-level error.** Some async-opcua versions reject the call as a whole with a service fault. The call returns `Err(...)`.
  3. **Silent truncation** (worst case — unexpected, but defended against). The call returns `Ok(...)` with fewer results than requested.
- **And critically — verify the count is bounded by the cap regardless of rejection shape.** After both calls, the test queries the subscription's monitored-item state (e.g., by issuing `Read` on each `MonitoredItemId` returned, or by introspecting the subscription via async-opcua's diagnostics if accessible). **Assert: total successful `MonitoredItemId`s ≤ 3.** This bounds-check catches the silent-truncation failure mode that a rejection-shape-only assertion would miss.
- **Verification:** test passes; total successful monitored-item count is ≤ 3 (the configured cap); the observed rejection shape is recorded in Completion Notes.
- **Wall clock target:** < 10 s. Mark `#[serial_test::serial]`.

#### AC#3.3: Subscription survives a ChirpStack outage with stale status codes (`epics.md:721`)

- **Test:** `test_subscription_survives_chirpstack_outage_with_stale_status`.
- **Given** the gateway running with `max_subscriptions_per_session = None` (default 10) and an active subscription on a `Float` metric NodeId. The metric has a fresh value `42.0` written via the storage backend (mirror the spike's `test_subscription_datavalue_payload_carries_seeded_value` setup).
- **When** the test simulates a ChirpStack outage by **NOT writing** any new metric values for `2 × stale_threshold_seconds` (≈ 240 s default; for a fast test, set `stale_threshold_seconds = Some(2)` in the test config so the outage simulation takes ≈ 5 s).
- **Then** the subscription **stays open** (no `delete_subscription` triggered, no status-bad notification on the subscription itself), AND the next received `DataChangeNotification` carries a **stale status code** per Story 5-2's logic (`Uncertain` for 1-24h-old, `Bad` for >24h; for a 5-s outage with `stale_threshold_seconds = 2`, expect `Uncertain` or `Bad` depending on the multiplier).
- **And** when the test then writes a fresh value back to storage (`backend.batch_write_metrics(...)`), the next notification within 10 s carries `Good` status — confirming the subscription "recovered" without re-creation.
- **CRITICAL — failure-mode pause.** This test proves a load-bearing assumption that was NOT directly validated by Story 8-1's spike: **that subscriptions remain open and continue notifying as their backing metric data ages into stale status.** Async-opcua's MonitoredItem dedupes successive identical samples (spike report § 8 test #9 "MonitoredItem dedupes successive identical samples per OPC UA spec"). The DataValue carries `(value, status_code, source_timestamp, server_timestamp)` — a Good→Bad-status transition with the SAME numeric value should fire a notification because the DataValue is structurally different. **If the test fails because no status-change notification fires (= dedup is suppressing status-only transitions), STOP IMPLEMENTATION AND ESCALATE TO USER.** This means: (a) a real ChirpStack outage would silently freeze SCADA dashboards at the last-good value; (b) Story 5-2's stale-status logic is observable via `Read` but not via subscriptions; (c) Story 8-2 has discovered a real Phase B regression that needs spec re-discussion before shipping. This is NOT a test bug — it is a Plan-A failure-mode the spike missed.
- **Verification:** test passes; the captured notification stream contains transitions Good → Uncertain/Bad → Good without subscription drops. If failed per the CRITICAL note above, the dev agent surfaces the failure mode to the user and pauses implementation.
- **Wall clock target:** ~10 s (longer than the other tests because of the staleness wait). Mark `#[serial_test::serial]`. **Do NOT use `tokio::time::pause()`** — the staleness logic in Story 5-2 reads `chrono::Utc::now()` (system clock), not tokio's virtual clock, so pausing tokio time has no effect on TTL evaluation. The test sets `stale_threshold_seconds = Some(2)` in the test config and budgets a real `tokio::time::sleep` of ~5–7 s to cross the staleness threshold; this is the simplest correct approach. If the dev agent finds the SQLite `metric_values.updated_at` column is queried via tokio-time-aware code (unlikely), revisit; otherwise stick to wall-clock sleep with a small threshold value.

**Implementation note for the test fixture:** `setup_test_server_with_max` currently takes a single `usize` (max sessions). Story 8-2 should add a `setup_test_server_with_subscription_limits` variant (or extend the existing helper with an optional `SubscriptionLimits` struct argument) that lets tests set the four new knobs explicitly. The variant is a small additive change — the existing helper signature stays, and the new variant is the test author's tool. Per CLAUDE.md scope-discipline rule: this is the **third** consumer of subscription-related test setup, so a small refactor is justified — but do not extract into `tests/common/` until a fourth file appears.

### AC#4: Subscription clients pass through `OpcgwAuthManager` + `AtLimitAcceptLayer` without modification (NFR12 carry-forward, `epics.md:705, 726`)

- **Existing tests in `tests/opcua_subscription_spike.rs` are the regression baseline** — `test_subscription_client_rejected_by_auth_manager` (line 524) and `test_subscription_client_rejected_by_at_limit_layer` (line 567). Both must continue to pass.
- **No new tests** for this AC — the spike's tests are sufficient. Story 8-2's contribution is **NOT modifying** `src/opc_ua_auth.rs` or `src/opc_ua_session_monitor.rs` and verifying the existing tests still pass.
- **No new audit-event infrastructure.** The existing `event="opcua_auth_failed"` (Story 7-2) and `event="opcua_session_count_at_limit"` (Story 7-3) audit events cover subscription clients identically to read-only clients — Story 8-1 spike § 8 confirmed empirically. **Story 8-2 must NOT introduce any new audit-event value** in `src/`. The one new event Story 8-2 adds (`event="opcua_limits_configured"` from AC#2) is a **diagnostic** event (one-shot startup config emit, same shape as Story 7-2's `pki_dir_initialised`), not an audit event — see the Existing Infrastructure table's "Tracing event-name convention" row for the audit-vs-diagnostic distinction.

**Verification:**
- `cargo test --test opcua_subscription_spike test_subscription_client_rejected_by_auth_manager` exits 0.
- `cargo test --test opcua_subscription_spike test_subscription_client_rejected_by_at_limit_layer` exits 0.
- `git diff src/opc_ua_auth.rs src/opc_ua_session_monitor.rs` over the Story 8-2 branch is **empty** (zero lines changed).
- Audit-event count check: see AC#8 for the de-dup'd grep recipe. The expected delta is **+0 audit events**; the new `event="opcua_limits_configured"` is a diagnostic event and is whitelisted in the count.

### AC#5: Documentation extends `docs/security.md` "OPC UA connection limiting" section

- Extend the existing `## OPC UA connection limiting` section in `docs/security.md` (currently `:537-659`). **Do NOT create a new top-level section** — the carry-forward at `epics.md:725` is explicit: subscription/message-size knobs go under "OPC UA connection limiting" as an extension. The section's existing subsection structure (What it is / Configuration / What you'll see in the logs / Anti-patterns / Tuning checklist / What's out of scope) carries forward; 8-2 inserts new content into the relevant subsections.
- Add a new subsection `### Subscription and message-size limits` after the existing `### What's out of scope` subsection (`:648-659`). **The new subsection is self-contained — its sub-items 1–6 below are sub-content of this new subsection, NOT augmentations of the parent section's existing top-level subsections.** The existing subsections (`What it is`, `Configuration`, `What you'll see in the logs`, `Anti-patterns`, `Tuning checklist`, `What's out of scope`) cover `max_connections` and **stay unchanged**; the new "Subscription and message-size limits" subsection mirrors that structure for the four new knobs:
  1. **What they are.** Four knobs that shape subscription/message-size load — `max_subscriptions_per_session` (per-session cap on simultaneous subscriptions), `max_monitored_items_per_sub` (per-subscription cap on monitored items), `max_message_size` (per-message byte ceiling, applies to both inbound `Read` and outbound `DataChangeNotification`), `max_chunk_count` (per-message chunk count ceiling). The two subscription-related knobs default to async-opcua library values (10, 1000); the two message-size knobs use gateway-chosen defaults that mirror common OPC UA implementations (see AC#1 + Dev Notes for the choice rationale).
  2. **Configuration.** Show the TOML block from AC#1 verbatim (with the dev-agent-resolved literal numbers, NOT the `<replace-at-impl>` placeholders). Document each env var name. Quote the four hard caps and the rationale (one-sentence per knob — defer the deep rationale to the spike report § 11 hyperlink).
  3. **What you'll see in the logs.** At startup, the gateway emits `info!(event="opcua_limits_configured", ...)` with structured fields for all five limits (`max_sessions`, `max_subscriptions_per_session`, `max_monitored_items_per_sub`, `max_message_size`, `max_chunk_count`). Operators grep this line on every restart to verify the resolved configuration:
     ```bash
     grep 'event="opcua_limits_configured"' log/opcgw.log | tail -1
     # 2026-04-30T08:14:22.105Z  INFO opcgw::opc_ua: event="opcua_limits_configured" max_sessions=10 max_subscriptions_per_session=10 max_monitored_items_per_sub=1000 max_message_size=... max_chunk_count=... "OPC UA limits configured"
     ```
     Subscription-flood / monitored-item-flood rejections are **silent** in async-opcua 0.17.1 (no audit emission for `BadTooManySubscriptions` / `BadTooManyMonitoredItems`); document this gap and link to the GitHub upstream FR (issue #94 if the FR has been filed by the time 8-2 ships, otherwise leave as a TODO with a one-line "no rejection-time audit event today, contract is the OPC UA status code on the wire").
  4. **Anti-patterns.** Do not set any of the four knobs to `0` (refuses subscriptions/items even from operators); do not set `max_message_size` above `max_chunk_count × 65535` without understanding the chunk geometry (spike report § 4 / async-opcua docs); do not rely on per-session subscription caps for distributed-flood defence (per-IP throttling deferred at issue #88).
  5. **Tuning checklist.** Four bullet lines (one per knob) for sizing guidance:
     - Inventory expected SCADA clients × subscriptions per client (typically 1-3); add 30% headroom.
     - Inventory monitored items per subscription (typically 10-100 for FUXA dashboards); leave the 1000 default unless headroom demands more.
     - `max_message_size` / `max_chunk_count` only matter if `Read` operations return very large arrays; at default opcgw deployments (scalar metrics) the defaults are oversized.
     - Pair with `max_connections` from Story 7-3: subscription clients consume one session each, so `max_connections` × `max_subscriptions_per_session` × `max_monitored_items_per_sub` is the upper bound on the publish pipeline's work.
  6. **What's out of scope.** Add explicit references to issue #88 (per-IP throttling), issue #94 (upstream FR for rejection-event audit), issue #95 (operator-pending `--load-probe` numbers).
- Update the `### Subscription clients and the audit trail` paragraph (NEW subsection — add after `### Tuning checklist`) with the **NFR12 carry-forward acknowledgment**: subscription-creating clients pass through `OpcgwAuthManager` and `AtLimitAcceptLayer` identically to read-only clients. The `event="opcua_auth_failed"` and `event="opcua_session_count_at_limit"` audit events from Stories 7-2 / 7-3 cover them. **No new audit infrastructure is introduced by Story 8-2.** Cite the regression-test names from `tests/opcua_subscription_spike.rs`.
- Update `README.md` "Configuration" section: add a one-line cross-link to the new subscription knobs subsection (`See docs/security.md#subscription-and-message-size-limits`).
- Update `README.md` Planning table row for Epic 8 per CLAUDE.md's documentation-sync rule, **using the status that matches the commit being made**:
  - At the implementation commit (status `ready-for-dev → review`): Story 8-2 row reads `🔄 in-progress (review)`.
  - At the code-review-complete commit (status `review → done`): Story 8-2 row reads `✅ done`.
- Append entry to `_bmad-output/implementation-artifacts/deferred-work.md` for Story 8-2:
  - The four mandatory knobs (and any advanced knobs from AC#7) are SHIPPED — record the subset that was deferred, if any.
  - The **conditional gauge tunability** decision (AC#7): if 8-2 promoted the knob, record "promoted per AC#7 (load-probe numbers showed noise-buried gauge)"; if not, record "constant stays per AC#7 (load-probe numbers absent or signal-clear)" with a one-line rationale.
  - The **per-IP rate limiting** out-of-scope reminder pointing at issue #88.
  - The **rejection-time audit event in async-opcua** carry-forward — same shape as the Story 7-2 / 7-3 deferred entries pointing at issues #94 / similar; replace with "Filed upstream FR: <URL>" once the operator files it (issue #94).

**Verification:**
- `grep -nE '^### Subscription and message-size limits' docs/security.md` returns **one hit**.
- `grep -nE 'subscription-and-message-size-limits' README.md` returns **at least one hit**.
- The README Planning row is updated in the relevant commit (verified by `git diff README.md` showing the Epic 8 / Story 8-2 row change).
- `grep -nE 'Story 8-2' _bmad-output/implementation-artifacts/deferred-work.md` returns **at least one hit** (new section heading + at least one entry).

### AC#6: Tests pass and clippy is clean (no regression)

- Story 8-1's baseline: **592 tests pass / 0 fail / 7 ignored** (sprint-status.yaml `last_updated` 2026-04-30). Story 8-2 adds:
  - **20 unit tests** from AC#1 (4 knobs × 5 validation tests).
  - **3 integration tests** from AC#3 (`test_subscription_flood_capped_*`, `test_monitored_item_flood_capped_*`, `test_subscription_survives_chirpstack_outage_*`).
  - **0–1 integration tests** from AC#2 (`test_resolved_limits_logged_at_startup` if the test author finds it useful).
  - **0–1 unit tests** for the four new `max_*` helper methods on `OpcUa` (only if the helpers gain non-trivial logic beyond `unwrap_or`; for the AC#2 helpers, skip — the integration tests cover them).
- New test count target: **≥ 23** (20 unit + 3 integration). New baseline: **≥ 615 tests pass**.
- `cargo clippy --all-targets -- -D warnings` exits 0. Story 8-1 left it clean — preserve.
- **Verification:** `cargo test --lib --bins --tests 2>&1 | tail -10` paste in Dev Notes Completion Notes; expect ≥ 615 pass / 0 fail / ≥ 7 ignored. `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` exits 0.

### AC#7: Conditional — `OPCUA_SESSION_GAUGE_INTERVAL_SECS` promotion (`epics.md:727`, `epics.md:683`)

**This AC is conditional on operator-supplied data** from Story 8-1's `--load-probe` 5-minute throughput run (issue #95). The decision is binary:

- **If the load-probe ran AND the gauge-usefulness verdict in spike report § 7 is "noise-buried" (≥ 50 unrelated `info!` lines per gauge tick under representative subscription load):** Story 8-2 promotes `OPCUA_SESSION_GAUGE_INTERVAL_SECS` from a hard-coded constant in `src/utils.rs:122` to a `[diagnostics]` block config knob with the env-var override `OPCGW_DIAGNOSTICS__SESSION_GAUGE_INTERVAL_SECS`. Use the same plumbing pattern as the four mandatory knobs (struct field on a new `DiagnosticsConfig` substruct, constants in `src/utils.rs`, validation in `AppConfig::validate`, TOML + example TOML updates, doc-section update).
- **If the load-probe did NOT run OR the verdict is "signal-clear" (< 50 unrelated `info!` per tick):** the constant **stays hard-coded**. Story 8-2 records the rationale in `deferred-work.md` (one-line entry) and moves on. **This is the default path** — Story 8-2 does NOT block on the operator running the load probe.

**Default behaviour (no load-probe data):** constant stays. Add a one-line entry to `deferred-work.md` under "Story 8-2": "Gauge tunability decision deferred per AC#7 — load-probe numbers (issue #95) not available. Revisit if a real subscription-flood operator scenario surfaces noise-buried gauge ticks."

**If promoted:** add `~80 LOC` of plumbing (new `DiagnosticsConfig` substruct in `src/config.rs`, ~6 unit tests in `src/config.rs::tests`, `src/utils.rs` constant rename to `OPCUA_DEFAULT_SESSION_GAUGE_INTERVAL_SECS` with a hard cap, `src/main.rs` or wherever the constant is read converts to `config.diagnostics.session_gauge_interval_secs.unwrap_or(...)`, TOML + example TOML updates, docs entry). Validation: `Some(0)` rejected, `Some(n) > 3600` rejected.

**Verification (default path):**
- `_bmad-output/implementation-artifacts/deferred-work.md` has a "Story 8-2" section with the gauge-tunability one-liner.
- `grep -n 'OPCUA_SESSION_GAUGE_INTERVAL_SECS' src/utils.rs src/main.rs src/opc_ua_session_monitor.rs` returns the same hit count as the Story 8-1 baseline (the constant has not moved).

**Verification (promotion path):**
- New unit tests `test_validation_rejects_session_gauge_interval_zero`, `test_validation_rejects_session_gauge_interval_above_hard_cap`, `test_validation_accepts_session_gauge_interval_default`, etc. (mirror the AC#1 pattern, 5 tests).
- The integration test `test_session_count_gauge_emits_periodically` in `tests/opc_ua_connection_limit.rs` is updated to use the configured interval (or stays as-is if it tolerates a configured override) and continues to pass.
- `grep -n 'session_gauge_interval_secs' src/config.rs src/utils.rs` returns ≥ 4 hits.

### AC#8: Sanity check on the regression-test count and the audit-event count

- **Regression-test count check.** At the start of Story 8-2 implementation, capture `cargo test --lib --bins --tests 2>&1 | tail -3` baseline counts; at the end, expect the new total to equal `baseline + N_AC#1 + N_AC#3 + N_AC#7_optional` exactly. Any unexpected delta (test loss, double-counted test, accidental ignore) is investigated before flipping the story to `review`.
- **Audit-event count check.** At the start of implementation, capture both:
  - `grep -rnoE 'event = "[a-z_]+"' src/ | sort -u > /tmp/8-2-events-baseline.txt` — captures the **set** of unique `event="<name>"` literals across `src/` (de-duplicates so the same event referenced in three files counts once).
  - `wc -l /tmp/8-2-events-baseline.txt` — record the count.
  At the end, regenerate the same file as `/tmp/8-2-events-final.txt`. The expected diff is exactly **one new entry: `event = "opcua_limits_configured"`** (the diagnostic event from AC#2). Verify with `diff -u /tmp/8-2-events-baseline.txt /tmp/8-2-events-final.txt` — the only `+` line is the `opcua_limits_configured` literal. If any **audit-flavoured** new event appears (anything matching `*_failed`, `*_rejected`, `*_at_limit`, `*_violation`, etc.), investigate and either remove (if accidental) or escalate to user (if intentional — adding a new audit event is NOT allowed under the NFR12 carry-forward acknowledgment without explicit approval).

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issues (CLAUDE.md compliance) (AC: All)

- [x] Open GitHub issue **"Story 8-2: OPC UA Subscription Support"** with a one-paragraph summary linking to this story file. Reference it in every commit message for this story (`Refs #N` on intermediate commits, `Closes #N` on the final code-review-complete commit).
- [x] **Do not** open follow-up issues for items already tracked: #88 (per-IP rate limiting), #89 (subscription/message-size limits as config knobs — this story closes the load-bearing portion), #90 (hot-reload of session cap), #93 (manual SCADA verification deferred to post-Epic-9), #94 (upstream session-rejected-event FR), #95 (`--load-probe` 5-min run). All carry forward unchanged.
- [x] Reference issue #89 in this story's commit message with `Closes #89` if the four mandatory knobs are SHIPPED — the issue tracker stays in sync with the code history.

### Task 1: Add the four `Limits` knobs as `OpcUaConfig` fields, constants, validation (AC: 1)

- [x] Add four `pub const OPCUA_DEFAULT_*` and four `pub const OPCUA_*_HARD_CAP` constants to `src/utils.rs` next to the existing `OPCUA_DEFAULT_MAX_CONNECTIONS` / `OPCUA_MAX_CONNECTIONS_HARD_CAP`. Each gets a one-paragraph doc comment naming Story 8-2 and citing the library default + source path.
- [x] Add four `Option<usize>` fields to `OpcUaConfig` in `src/config.rs:148-262` after `max_connections`. Each gets a doc comment matching project tone (purpose / range / env-var override / library-default reference). Use `#[serde(default)]` on each new field for graceful absence handling at literal sites (recommended path; alternative: update every `OpcUaConfig { ... }` literal site in `tests/`).
- [x] Update `impl Debug for OpcUaConfig` in `src/config.rs:287-309` with four new `.field(...)` calls. **Mandatory** — Story 7-1 NFR7 invariant.
- [x] Extend `AppConfig::validate` in `src/config.rs:739-` with eight new accumulator entries (four `Some(0)` + four `Some(n) > HARD_CAP`), placed after the existing `max_connections` block (`:809-841`). Wording mirrors AC#1 spec.
- [x] Update `config/config.toml` and (if it exists) `config/config.example.toml` with the commented-out default block from AC#1 spec, placed after the `max_connections` block.
- [x] Audit `OpcUaConfig { ... }` literal sites: `grep -rn 'OpcUaConfig {' tests/ src/`. If `#[serde(default)]` was used (recommended), no test-fixture changes needed. If not, update every literal site to add the four new fields as `<field>: None`.
- [x] Add 20 unit tests to `src/config.rs::tests` (4 knobs × 5 tests each, mirroring the AC#1 verification recipe). Wall-clock cost: ~30s for all 20 to run.
- [x] `cargo build` clean. `cargo test --lib --bins config::tests::test_validation_max_subscriptions_per_session` runs the new tests for that knob; repeat for each.

### Task 2: Wire `ServerBuilder` limit configuration through `configure_limits` (AC: 2)

- [x] Add four `fn max_<knob_name>(&self) -> usize` helper methods on `OpcUa` in `src/opc_ua.rs`, placed right after the existing `max_sessions` helper at `:328-333`. Each follows the `unwrap_or(OPCUA_DEFAULT_*)` pattern.
- [x] Extend `OpcUa::configure_limits` (`src/opc_ua.rs:316-320`) per the AC#2 body skeleton: direct setters for `max_message_size` / `max_chunk_count`, `limits_mut()` block for `max_subscriptions_per_session` / `max_monitored_items_per_sub`. Update the doc comment to reflect the new responsibility.
- [x] Extend the startup info log at `src/opc_ua.rs:599` (the `info!(..., max_sessions, ...)` block) with four new structured fields: `max_subs_per_session`, `max_items_per_sub`, `max_message_size`, `max_chunk_count`. Use the same field-naming convention as `max_sessions`.
- [x] `cargo build` clean. `cargo test --lib --bins` passes (no regression).

### Task 3: Subscription-limit enforcement integration tests (AC: 3)

- [x] Add a new helper `setup_test_server_with_subscription_limits(max_sessions: usize, sub_limits: SubscriptionLimitsForTest)` (or a new optional struct argument to the existing `setup_test_server_with_max`) to `tests/opcua_subscription_spike.rs`. The new helper passes the four knobs through to `OpcUaConfig` literally.
- [x] Implement `test_subscription_flood_capped_by_max_subscriptions_per_session` per AC#3.1.
- [x] Implement `test_monitored_item_flood_capped_by_max_monitored_items_per_sub` per AC#3.2. Document the observed rejection shape (per-item status code vs service-level error) in test comments + Dev Notes Completion Notes.
- [x] Implement `test_subscription_survives_chirpstack_outage_with_stale_status` per AC#3.3. Use `tokio::time::pause()` + `advance()` if the storage backend's TTL logic supports virtualised time; else accept ~30 s wall clock.
- [x] Mark all three new tests `#[serial_test::serial]`.
- [x] `cargo test --test opcua_subscription_spike test_subscription_flood test_monitored_item_flood test_subscription_survives` exits 0.

### Task 4: NFR12 carry-forward regression check (AC: 4)

- [x] Capture baseline + final event-set per AC#8's de-dup'd grep recipe (`grep -rnoE 'event = "[a-z_]+"' src/ | sort -u`). Save the baseline file in Dev Notes Debug Log References.
- [x] After implementation: regenerate the final event-set; `diff -u baseline final` shows **exactly one new entry**: `event = "opcua_limits_configured"` (the diagnostic event from AC#2). Any audit-flavoured new event (matching `*_failed`, `*_rejected`, `*_at_limit`, etc.) is a regression — investigate.
- [x] `cargo test --test opcua_subscription_spike test_subscription_client_rejected_by_auth_manager test_subscription_client_rejected_by_at_limit_layer` exits 0.
- [x] `git diff src/opc_ua_auth.rs src/opc_ua_session_monitor.rs` over the entire Story 8-2 branch is empty.

### Task 5: Documentation (AC: 5)

- [x] Extend `docs/security.md` "OPC UA connection limiting" section with the new `### Subscription and message-size limits` subsection per AC#5 spec. Include the TOML configuration block, env-var name table, hard caps, log-grep recipe, anti-patterns, tuning checklist additions.
- [x] Add a new subsection `### Subscription clients and the audit trail` after the tuning checklist with the NFR12 carry-forward acknowledgment per AC#5 spec.
- [x] Update `README.md` "Configuration" section with one-line cross-link.
- [x] Update `README.md` Planning table row for Story 8-2 per AC#5 timing rule (status synchronised with commit type).
- [x] Append "Story 8-2" section to `_bmad-output/implementation-artifacts/deferred-work.md` covering: gauge-tunability decision (AC#7), per-IP throttling reminder (issue #88), upstream rejection-event FR (issue #94 link or carry-forward note).

### Task 6: Conditional gauge tunability decision (AC: 7)

- [x] Check whether `_bmad-output/implementation-artifacts/8-1-spike-report.md` § 7 has operator-supplied numbers (`_TBD_` cells filled in).
- [x] **If absent (default path):** add the one-line "Story 8-2" entry to `deferred-work.md` per AC#7 spec. No code change.
- [x] **If present and "noise-buried":** implement the promotion path per AC#7 spec — new `DiagnosticsConfig` substruct in `src/config.rs`, constants in `src/utils.rs`, validation, TOML + docs updates, 5 unit tests. Update the existing `test_session_count_gauge_emits_periodically` integration test in `tests/opc_ua_connection_limit.rs` to use the configured interval.
- [x] **If present and "signal-clear":** add a one-line entry to `deferred-work.md` recording the rationale ("constant stays — load-probe verdict signal-clear").

### Task 7: Final verification (AC: 6, 8)

- [x] `cargo test --lib --bins --tests 2>&1 | tail -10` — paste pass/fail counts into Dev Notes Completion Notes. Expected: ≥ 615 pass / 0 fail / ≥ 7 ignored.
- [x] `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5` — exit 0.
- [x] AC#8 sanity checks: regression-test count delta + audit-event count delta both verified.
- [x] Code review readiness: re-read the story file, the `git diff`, and the test output to confirm ACs are fully addressed before flipping status to `review`.

### Task 8: Documentation sync verification (CLAUDE.md compliance)

- [x] Verify `README.md` Planning section reflects sprint-status.yaml's Story 8-2 status (per CLAUDE.md "Documentation Sync" rule).
- [x] Verify config-knob updates are reflected in `README.md`'s Configuration section (per CLAUDE.md "Documentation Sync" rule).
- [x] Verify the commit message references the right GitHub issues (Task 0 main tracker + `Closes #89` if the four mandatory knobs ship).

---

## Dev Notes

### Why this story is small

Story 8-1's spike confirmed Plan A: the subscription engine works at HEAD with zero `src/` changes. Async-opcua 0.17.1's `SimpleNodeManagerImpl` already implements all four monitored-item lifecycle hooks and auto-wires a `SyncSampler` against the existing `add_read_callback` registrations. **Story 8-2 plumbs config through `ServerBuilder` and adds tests that pin the resulting limits** — that's it. No new modules, no new tasks, no new audit events, no new patterns. The pattern was established by Story 7-3 (`max_connections`); Story 8-2 follows it for four more knobs (and optionally five advanced knobs).

The estimated diff:
- `src/utils.rs`: +8 constants (≈ 60 LOC of doc + literals)
- `src/config.rs`: +4 fields + 4 Debug entries + 8 validate accumulator entries (≈ 80 LOC)
- `src/opc_ua.rs`: +4 helper methods + extended `configure_limits` body + extended startup log (≈ 50 LOC)
- `config/config.toml` + example: +1 commented block × 2 files (≈ 20 LOC each)
- `docs/security.md`: +1 subsection × 2 (≈ 80 LOC)
- `README.md`: +2 lines (Configuration cross-link + Planning row)
- `tests/opcua_subscription_spike.rs`: +1 helper + 3 tests (≈ 200 LOC)
- `src/config.rs::tests`: +20 unit tests (≈ 200 LOC)

**Total:** ≈ 700 LOC, of which ≈ 400 LOC is tests. Production-code surface ≈ 300 LOC.

### Why use the four-knob minimum (and the optional five advanced knobs)

The carry-forward at `epics.md:682, 724` names a four-knob minimum: `max_subscriptions_per_session`, `max_monitored_items_per_subscription` (library: `_per_sub`), `max_message_size`, `max_chunk_count`. These four shape the load-bearing risks: subscription-flood, monitored-item-flood, message-size DoS, chunk-count DoS.

The spike report § 5 surfaces five additional "should-expose" knobs: `max_pending_publish_requests`, `max_publish_requests_per_subscription`, `min_sampling_interval_ms`, `max_keep_alive_count`, `max_queued_notifications`. **Story 8-2 ships only the four mandatory knobs by default.** The advanced five are deferred unless one of these triggers:

1. The operator's `--load-probe` numbers (issue #95) reveal a back-pressure / queue-drop behaviour that an advanced knob could fix. (Most likely candidate: `max_queued_notifications` if drops > 0.)
2. A real SCADA deployment surfaces a configuration scenario that the four mandatory knobs can't shape.

Until those triggers fire, the advanced five stay deferred — a follow-up issue captures the candidates list. Per CLAUDE.md's scope-discipline rule: "Don't add features, refactor, or introduce abstractions beyond what the task requires."

If the operator wants the advanced five **now** (before the load-probe runs), they can be added with the same plumbing pattern — but the spec for AC#1 in this story does NOT include them. Adding them later is a small additive change.

### Why no new audit events

The Phase A carry-forward at `epics.md:705, 726` is explicit: "no new audit infrastructure introduced by this story." Story 8-2's contribution to audit observability is **the existing tests in `tests/opcua_subscription_spike.rs`** which pin that subscription-creating clients flow through the same `OpcgwAuthManager` and `AtLimitAcceptLayer` paths as read-only clients.

Subscription-flood / monitored-item-flood rejections are silent in async-opcua 0.17.1 — `SubscriptionService::create_subscription` returns `BadTooManySubscriptions` and `MonitoredItemService` returns `BadTooManyMonitoredItems` without log emission. **This is a documented gap** captured in the docs (AC#5) and a candidate for a future upstream FR (analogous to the session-rejected-event FR at issue #94). Story 8-2 does not file that FR; the operator does, when they have time and the upstream maintainers are accepting issues.

### `limits_mut()` vs `.limits(Limits)` — why the choice matters

Spike report § 11 item 2: **do NOT pass a fully-built `Limits` struct via `.limits(...)`**. The reason: `Limits` has ~30 fields, and the gateway's defaults match the library defaults for almost all of them. If the gateway constructs a fresh `Limits { ... }` literal, every field that wasn't explicitly set takes the literal's default — which may or may not match the library default depending on Rust's `Default` implementation.

Mixed direct-setter + `limits_mut()` is the safer pattern: each direct setter / `limits_mut().subscriptions.<field> = N` modifies one field, leaving the rest at the library default. Future async-opcua upgrades that add new `Limits` fields automatically pick up sensible defaults without touching `configure_limits`.

### Test-harness reuse (CLAUDE.md scope-discipline rule)

`tests/opcua_subscription_spike.rs` already has the harness Story 8-2 needs (`setup_test_server_with_max`, `HeldSession`, `open_session_held`, `user_password_identity` / `wrong_password_identity`). Story 8-2 reuses inline. The "fourth integration-test file" threshold for `tests/common/` extraction has not been crossed (we have `tests/opc_ua_security_endpoints.rs`, `tests/opc_ua_connection_limit.rs`, `tests/opcua_subscription_spike.rs` = 3 files). **Per CLAUDE.md scope-discipline rule: "three similar lines is better than a premature abstraction."** Refactor into `tests/common/` when the fourth file appears.

The new helper `setup_test_server_with_subscription_limits` is a small additive variant — adding it does not cross the abstraction threshold.

### Why `#[serde(default)]` is the recommended path for new fields

Adding the four `Option<usize>` fields without `#[serde(default)]` would mean every `OpcUaConfig { ... }` literal site in test fixtures must add `<field>: None`. The grep `grep -rn 'OpcUaConfig {' tests/ src/` shows **at least one site** (`tests/opcua_subscription_spike.rs:175-205`'s `test_config()` closure) and possibly more.

`#[serde(default)]` on each new field tells figment "if the TOML key is absent, default to the type's `Default` (= `None` for `Option`)" — and Rust's struct-update syntax `OpcUaConfig { foo: bar, ..Default::default() }` becomes available at literal sites if/when `OpcUaConfig: Default`. The current shape doesn't `derive(Default)` on `OpcUaConfig`, but the `#[serde(default)]` approach still works for figment-driven deserialisation.

**The catch:** `#[serde(default)]` only helps at deserialisation time, not at literal-construction time. So test fixtures that build `OpcUaConfig { ... }` literally still need to mention the new fields **regardless** of the `#[serde(default)]` choice. The `#[serde(default)]` annotation only affects what figment does when a TOML file omits the key — it does NOT make Rust struct-literal initialisation tolerate missing fields.

**The actual choice** is whether figment's TOML deserialisation should treat absence as `None`:
- **With `#[serde(default)]` on each new field:** TOML fixtures that omit the key parse as `field: None`. (Default behaviour for `Option<T>` in serde — but be explicit since some figment configurations require the annotation.)
- **Without `#[serde(default)]`:** if any TOML fixture is parsed into `OpcUaConfig` and the new key is missing, deserialisation fails with `missing field <name>`. **This is what Story 7-3 ran into and worked around by updating every TOML fixture as well as every Rust literal site.**

**Recommendation:** add `#[serde(default)]` to each new field to keep TOML fixtures forward-compatible. **Then update every Rust struct-literal site** (`grep -rn 'OpcUaConfig {' tests/ src/`) to add `<field>: None,` or use the struct-update syntax. Both diffs are small; the `#[serde(default)]` annotation buys forward-compatibility for future TOML fixtures without extra code.

**Verification at start of implementation:** `grep -rn 'OpcUaConfig {' tests/ src/` to enumerate every literal site. As of 2026-04-30 the count is at least one site (`tests/opcua_subscription_spike.rs:175-205`'s `test_config()` closure) plus possibly the spec-existing tests in `src/config.rs::tests`. Record the count in Dev Notes.

### Conditional AC#7 — what the load-probe verdict means in practice

Story 8-1's `--load-probe` is an **operator-facing** 5-minute test. It runs the spike binary against a running gateway with `--load-probe --load-items 100 --load-secs 300 --load-publish-ms 1000`. The binary captures p50/p95/p99 inter-notification interval, drop count, and gauge-tick counts in JSON-on-stderr. The verdict ("noise-buried" vs "signal-clear") comes from grepping the captured tracing output for the gauge events and counting unrelated `info!` lines per gauge tick.

**As of Story 8-2 entry (2026-04-30), the load-probe has NOT been run** — issue #95 captures it as operator-action, deferred so 8-2 can ship without blocking. The default path of AC#7 is "constant stays" with a one-line `deferred-work.md` entry. The promotion path is documented for completeness; if the operator runs the probe between 8-2 entry and 8-2 implementation completion, the dev agent should check spike report § 7 for `_TBD_` cells and act accordingly.

### Pull vs push — operator-pause condition

If the operator's `--load-probe` numbers (issue #95) become available during 8-2 implementation and show p99 inter-notification interval > 3000 ms or notification drop count > 5% of expected (per spike report § 6's decision rule), the dev agent **pauses, surfaces the finding, and treats it as a scope expansion** — NOT a silent pivot to the push model. The Out of Scope bullet on push-model is the operative contract; this Dev Notes section is the pause-trigger reminder only.

### NFR12 ack — why no startup-warn changes

Commit `344902d` shipped the NFR12 startup-warn (a one-shot `warn!` when the global log level filters out info, gated on `[opcua].max_connections` being set). **Story 8-2 does not change this warn.** The new four knobs are not auth-related; they don't trigger NFR12. The existing test (if any) for the warn stays as the regression baseline.

### Project Structure Notes

- New constants in `src/utils.rs` are top-level (no module changes); doc comments cite Story 8-2 + AC#1 + library source path.
- New `OpcUaConfig` fields are in `src/config.rs` after `max_connections` (chronological / topical grouping with the `Limits`-related knobs).
- New `OpcUa` helper methods are in `src/opc_ua.rs` after `max_sessions` (single-source-of-truth pattern).
- New tests in `tests/opcua_subscription_spike.rs` are appended below the existing 9 tests, marked `#[serial_test::serial]`.
- Documentation extends the existing `docs/security.md` "OPC UA connection limiting" section with new subsections — no new top-level section.
- No new modules, no new files in `src/` beyond docstring extensions. Zero changes to `src/opc_ua_auth.rs` and `src/opc_ua_session_monitor.rs`.

---

## References

- Spike report (the load-bearing input): [`_bmad-output/implementation-artifacts/8-1-spike-report.md`](./8-1-spike-report.md) — § 4 (API surface), § 5 (`Limits` reachability table), § 6 (pull vs push), § 11 (Implications for Story 8-2)
- Story 8-1 spec: [`_bmad-output/implementation-artifacts/8-1-async-opcua-subscription-spike.md`](./8-1-async-opcua-subscription-spike.md)
- Epic 8 spec: [`_bmad-output/planning-artifacts/epics.md`](../planning-artifacts/epics.md) lines 671–728 (file's "Epic 7" = sprint-status's "Epic 8") — Phase A carry-forward bullets at 678–684, Story 8.2 ACs at 707–728
- PRD FR21 (subscription-based data change notifications): [`prd.md`](../planning-artifacts/prd.md) §378
- PRD NFR22 (FUXA + at least one additional OPC UA client): [`prd.md`](../planning-artifacts/prd.md) §461
- Architecture push-model design: [`architecture.md`](../planning-artifacts/architecture.md) §211–215
- Story 7-3 spec (`max_connections` plumbing precedent): [`_bmad-output/implementation-artifacts/7-3-connection-limiting.md`](./7-3-connection-limiting.md)
- Story 7-2 spec (auth manager — regression baseline): [`_bmad-output/implementation-artifacts/7-2-opc-ua-security-endpoints-and-authentication.md`](./7-2-opc-ua-security-endpoints-and-authentication.md)
- Epic 7 retrospective (NFR12 source-IP correlation, gauge tunability discussion): [`_bmad-output/implementation-artifacts/epic-7-retro-2026-04-29.md`](./epic-7-retro-2026-04-29.md)
- Deferred-work tracker (carry-forward entries): [`_bmad-output/implementation-artifacts/deferred-work.md`](./deferred-work.md) — "Story 7-3" block (issues #88, #89, #90), "Story 8-1" block (issues #93, #94, #95)
- async-opcua-server 0.17.1 source root: `~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/async-opcua-server-0.17.1/`
  - `Limits` and `SubscriptionLimits` struct definitions: `src/config/limits.rs:1–127`
  - `ServerBuilder::limits_mut()`: `src/builder.rs:246`
  - `ServerBuilder::max_message_size()` / `max_chunk_count()`: `src/builder.rs:380, 422`
  - Library default constants: `src/lib.rs:60–145`
- opcgw existing wire points:
  - `OpcUa::create_server`: `src/opc_ua.rs:168-244`
  - `OpcUa::configure_limits`: `src/opc_ua.rs:316-320` (existing — extends in place)
  - `OpcUa::max_sessions` (helper precedent): `src/opc_ua.rs:328-333`
  - `OpcUaConfig` struct: `src/config.rs:148-262`
  - `OpcUaConfig` Debug impl: `src/config.rs:287-309`
  - `AppConfig::validate` accumulator: `src/config.rs:739-` (existing OpcUa block at `:802-841`)
  - Constants in `src/utils.rs`: `:104, :114, :122` (existing precedents for new constants)
  - Subscription read-callback registrations: `src/opc_ua.rs:723, 810, 872, 880, 888`
- Spike binary (operator-facing reference): `examples/opcua_subscription_spike.rs`
- Spike + 8-2 integration tests: `tests/opcua_subscription_spike.rs`
- Documentation extension target: `docs/security.md:537-659` (existing "OPC UA connection limiting" section)
- CLAUDE.md scope-discipline rule, code-review loop discipline, documentation-sync rule

---

## Dev Agent Record

### Agent Model Used

claude-opus-4-7 (1M context)

### Debug Log References

- **Library-default resolution (AC#1).** Confirmed via direct source inspection of `~/.cargo/registry/src/index.crates.io-*/async-opcua-server-0.17.1/src/config/limits.rs:5-65` (Limits struct + Default impl) and `~/.cargo/registry/src/index.crates.io-*/async-opcua-types-0.17.1/src/lib.rs:43,48` (`MAX_MESSAGE_SIZE = 65535 * MAX_CHUNK_COUNT`, `MAX_CHUNK_COUNT = 5`). Subscription-related defaults from `async-opcua-server-0.17.1/src/lib.rs:131,64` (`MAX_SUBSCRIPTIONS_PER_SESSION = 10`, `DEFAULT_MAX_MONITORED_ITEMS_PER_SUB = 1000`). Gateway defaults match library defaults exactly so unsetting in TOML is a no-op.
- **Audit-event baseline (AC#8).** Captured at start with `grep -rnoE 'event = "[a-z_]+"' src/ | sort -u > /tmp/8-2-events-baseline.txt` (17 unique events). Final state regenerated; `diff -u baseline final` shows **exactly +1 entry: `event = "opcua_limits_configured"`** (the diagnostic event from AC#2). Zero new audit-flavoured events. NFR12 carry-forward intact.
- **OpcUaConfig literal sites (AC#1 audit).** `grep -rn 'OpcUaConfig {' tests/ src/` enumerated 5 sites: `tests/opcua_subscription_spike.rs:179`, `tests/opc_ua_security_endpoints.rs:156`, `tests/opc_ua_connection_limit.rs:179`, `src/opc_ua_auth.rs:458`, `src/config.rs:2360`. All 5 updated with the four new `<field>: None` lines.
- **AC#3.3 CRITICAL halt + recovery.** First test run flagged the load-bearing failure mode (test failed because async-opcua's MonitoredItem with `FilterType::None` dedupes on `value.value` only at `subscriptions/monitored_item.rs:514-517`, suppressing status-only transitions). Implementation halted per spec; user chose option 1 (rewrite test with explicit `DataChangeFilter`). Test rewritten to supply `ExtensionObject::from_message(DataChangeFilter { trigger: DataChangeTrigger::StatusValue, deadband_type: 0, deadband_value: 0.0 })` — routes through `is_changed()` (`async-opcua-types::data_change.rs:91-109`) which compares `v1.status != v2.status`. Test passes. Plan-A unfiltered behaviour documented (not pinned by automated test) in `docs/security.md`.

### Completion Notes List

- **Test count delta:** 635 pass / 0 fail / 7 ignored on `cargo test --lib --bins --tests`. Net new: 20 unit tests (AC#1, 4 knobs × 5 validation cases) + 3 integration tests (AC#3.1 / AC#3.2 / AC#3.3). The 56 doctest failures on default `cargo test` are pre-existing carry-forward debt (sprint-status `last_updated` notes the doctest cleanup story before Epic 9).
- **Audit-event delta:** +1 diagnostic event (`opcua_limits_configured`), 0 audit events. NFR12 carry-forward acknowledgment satisfied.
- **Observed AC#3.2 rejection shape:** **service-level `Err(BadTooManyMonitoredItems)`** (shape 2 of the 3 documented in spec). The test's bound check (`total_successes ≤ cap`) succeeded with `total=3` (cap), confirming async-opcua enforces per-subscription monitored-item limits correctly.
- **Conditional AC#7 path:** **default path taken** — `OPCUA_SESSION_GAUGE_INTERVAL_SECS` constant stays hard-coded; load-probe data (issue #95) absent. Recorded in `_bmad-output/implementation-artifacts/deferred-work.md` Story 8-2 block.
- **Library-default values picked:** `OPCUA_DEFAULT_MAX_SUBSCRIPTIONS_PER_SESSION = 10`, `OPCUA_DEFAULT_MAX_MONITORED_ITEMS_PER_SUB = 1000`, `OPCUA_DEFAULT_MAX_MESSAGE_SIZE = 327_675` (= 65_535 × 5), `OPCUA_DEFAULT_MAX_CHUNK_COUNT = 5`. All match async-opcua 0.17.1 library defaults exactly.
- **Hard caps:** 1000, 100_000, 268_435_456 (256 MiB), 4096 — per AC#1 spec.
- **`#[serde(default)]`** applied to all four new `Option<usize>` fields per Dev Notes recommendation. TOML fixtures with omitted keys still deserialise as `None`.
- **Zero changes to NFR12-protected production code.** `git diff src/opc_ua_auth.rs src/opc_ua_session_monitor.rs` shows only a 4-line literal-site addition inside `mod tests` of `opc_ua_auth.rs` (one of the 5 mandatory `OpcUaConfig` literal sites per AC#1). No production logic in either file was modified.
- **clippy clean:** `cargo clippy --all-targets -- -D warnings` exits 0.
- **AC#3.3 documentation contract.** Story 8-2's outage-survival test pins the **compliant-client** path (explicit `DataChangeFilter`). The unfiltered Plan-A path (which silently freezes on status-only transitions) is documented in `docs/security.md#subscription-and-message-size-limits` but not pinned by an automated test — the contract is what compliant SCADA clients (FUXA, Ignition, UaExpert) actually do per OPC UA Part 4 §7.17.2.

### File List

**Production code:**
- `src/utils.rs` — added 8 constants (4 `OPCUA_DEFAULT_*` + 4 `OPCUA_*_HARD_CAP`).
- `src/config.rs` — added 4 `Option<usize>` fields to `OpcUaConfig` (with `#[serde(default)]`); extended `Debug` impl with 4 new `.field(...)` calls; extended `AppConfig::validate` with 8 new accumulator entries; added 20 new unit tests in `mod tests`; updated 1 `OpcUaConfig` literal site in `mod tests`.
- `src/opc_ua.rs` — extended `configure_limits` with new wiring (mixed direct-setter + `limits_mut()` pattern); added 4 new `max_*` helper methods; added new `info!(event="opcua_limits_configured", ...)` startup emit.
- `src/opc_ua_auth.rs` — updated 1 `OpcUaConfig` literal site in `mod tests` (4-line addition; no production logic change).

**Tests:**
- `tests/opcua_subscription_spike.rs` — added new imports (`DataChangeFilter`, `DataChangeTrigger`, `ExtensionObject`); added `SubscriptionLimitsForTest` struct; added `spike_test_config_with_limits` and `setup_test_server_with_subscription_limits` helpers; added 3 new integration tests; updated 1 `OpcUaConfig` literal site.
- `tests/opc_ua_security_endpoints.rs` — updated 1 `OpcUaConfig` literal site.
- `tests/opc_ua_connection_limit.rs` — updated 1 `OpcUaConfig` literal site.

**Configuration:**
- `config/config.toml` — added commented-out default block for the four new knobs.
- `config/config.example.toml` — added commented-out default block for the four new knobs.

**Documentation:**
- `docs/security.md` — added `### Subscription and message-size limits` subsection (~6 sub-items: What they are / Configuration / What you'll see in the logs / DataChangeFilter contract / Anti-patterns / Tuning checklist); added `### Subscription clients and the audit trail` subsection (NFR12 carry-forward); added `### What's out of scope (subscription / message-size knobs)`.
- `README.md` — added one-line cross-link in Configuration section to the new `docs/security.md` subsection; updated Epic 8 Planning row (`8-1 done; 8-2 ready` → `8-1 done; 8-2 review`); fixed stale "Stories 8-2 / 8-3 / 8-4 still backlog" reference (8-2 removed).
- `_bmad-output/implementation-artifacts/deferred-work.md` — appended Story 8-2 section: gauge-tunability default-path entry, advanced-five subscription knobs deferred entry, per-source-IP throttling reminder (issue #88), upstream FR for rejection-time audit events (issue #94 candidate), `DataChangeFilter`-less client behaviour documentation reference.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — flipped `8-2-opc-ua-subscription-support` from `ready-for-dev` to `review` (will flip again in Step 9); updated `last_updated` narrative.
- `_bmad-output/implementation-artifacts/8-2-opc-ua-subscription-support.md` — Dev Agent Record + File List + status updated.

### Change Log

| Date       | Change |
|------------|--------|
| 2026-04-30 | Story 8-2 implementation: 4 mandatory `Limits` knobs plumbed end-to-end (`src/utils.rs` + `src/config.rs` + `src/opc_ua.rs`), 8 validation accumulator entries, 20 new unit tests, 3 new integration tests, `docs/security.md` extension, README cross-link + Planning row update, `deferred-work.md` Story 8-2 block, AC#3.3 CRITICAL halt + recovery (test rewritten with explicit `DataChangeFilter { trigger: StatusValue, .. }` after user decision). 635 pass / 0 fail / 7 ignored, clippy clean. NFR12 carry-forward intact (zero production-code changes to `src/opc_ua_auth.rs` / `src/opc_ua_session_monitor.rs`; +1 diagnostic event, 0 new audit events). Conditional AC#7 default path taken — gauge constant stays. Tracker at GitHub issue #96; `Closes #89` (subscription/message-size limits as config knobs). |
| 2026-04-30 | Story 8-2 code review iteration 2: re-ran all three layers against the patched diff per CLAUDE.md "re-run review after non-trivial patch round". Acceptance Auditor declared clean ("ITERATION 2 ACCEPTANCE AUDIT: clean — all 11 iteration-1 patches verified against spec, no new gaps surfaced"). Adversarial layers surfaced 1 MEDIUM (cross-knob coherence silently skipped when only one knob set; default-fallback gap) + ~7 LOWs. **Iteration-2 patches applied (autonomous-loop):** (A) cross-knob coherence resolves unset knobs via `unwrap_or(default)` so misconfigs like `max_message_size = 256 MiB` with `max_chunk_count = None` are caught at startup; skip cross-knob if either knob is `Some(0)` to avoid duplicate per-knob errors; +2 unit tests (`test_validation_rejects_oversized_msg_with_default_chunks`, `test_validation_skips_cross_knob_when_message_size_zero`); also aligned `OPCUA_MAX_MESSAGE_SIZE_HARD_CAP = 4096 × 65535 = 268_431_360` (was `256 * 1024 * 1024 = 268_435_456`) so the two hard caps are mathematically coherent — surfaced because the original constants disagreed by 4096 bytes. (B) `OPCUA_MIN_CHUNK_SIZE_BYTES` doc-comment clarified: 65535 is async-opcua's per-chunk ceiling, NOT OPC UA Part 6's TransportProfile minimum (which is 8192). (C) `tests/opcua_subscription_spike.rs` AC#3.1 `BadTooManySubscriptions \|\| TooManySubscriptions` simplified to single substring (former contains latter; redundancy removed); AC#3.2 dead `assert!(total_successes <= 3)` removed (already enforced by `assert_eq!`). (D) generated-source line numbers (`enums.rs:285`, `data_change.rs:91-109`, `monitored_item.rs:514-517`) dropped from test docstrings + `docs/security.md` — generated source isn't semver-stable. (E) startup-log poll budget 5s → 10s for loaded CI. Updated 2 pre-existing tests that broke under the cross-knob default-fallback change (`test_validation_accepts_max_message_size_at_hard_cap` + `test_validation_accepts_max_chunk_count_one`) to set both knobs coherently. **Final test count: all 14 subscription-spike tests pass, all 60 config tests pass, clippy clean.** One flaky test (`storage::sqlite::tests::test_concurrent_write_read_isolation`) fails intermittently under heavy parallel test load — pre-existing race on writer/reader threads, NOT caused by iter-2 patches; passes deterministically in isolation; deferred to a follow-up storage-test productionisation pass. Loop terminates per CLAUDE.md "only LOW severity findings remain" — auditor confirmed clean, all MEDs patched, 1 LOW kept as defensible design (dv.status=None accepted as Good in recovery branch — paired with strict value-match defends against status-stripping regressions). |
| 2026-04-30 | Story 8-2 code review iteration 1: 3 decision-needed (all resolved Option 1) + 11 patches applied + 8 deferred (pre-existing 8-1 baseline + NFR7 carry-forward) + ~14 dismissed. Applied: status-code pinning on AC#3.1/3.2 (BadTooManySubscriptions substring + total_successes==3 lower bound), cross-knob coherence validation (`max_chunk_count × 65535 < max_message_size` rejected at startup; `OPCUA_MIN_CHUNK_SIZE_BYTES` constant; +2 unit tests), unfiltered `DataChangeFilter` regression test pinning async-opcua's value-only-dedup contract (issue #94), AC#3.3 outage-loop wall-clock flake fix (12s outage + per-iteration timeout consumes remaining budget; distinguish "zero notifications" from "all-Good" failure modes), AC#2 startup-log shape regression test (`event="opcua_limits_configured"` field-name pinning), DataChangeTrigger Part 4 §7.22.2 citation correction (was incorrectly cited as §7.17.2; library default is `Status` not `StatusValue`), `..Default::default()` on `DataChangeFilter` literal (forward-compat with library minor bumps), README "Current Version" sync + Epic 8 row compression (single-line cell with cross-link), `limits_mut()` ordering invariant comment, `dv.status==Good` recovery assertion tightened (require value match + Good/None status, not None alone). **Final test count: 641 pass / 0 fail / 7 ignored** (`cargo test --lib --bins --tests`). **clippy clean** (`cargo clippy --all-targets -- -D warnings`). NFR12 carry-forward still intact. Story flipped `review → done`. |

---

### Review Findings

Code review run on 2026-04-30 against the uncommitted Story 8-2 diff (Blind Hunter + Edge Case Hunter + Acceptance Auditor; full mode, spec loaded).

**Triage summary:** 3 decision-needed, 11 patch, 7 deferred (pre-existing 8-1 baseline + NFR7 carry-forward), ~14 dismissed as noise.

#### Decision-needed (resolve first — spec-relaxed or scope-expanding)

- [x] [Review][Decision-resolved] **AC#3.1 / AC#3.2 don't pin specific OPC UA status codes** — `tests/opcua_subscription_spike.rs` AC#3.1 test asserts `sub3.is_err()` without checking for `BadTooManySubscriptions`; AC#3.2 only asserts `total_successes ≤ cap` without lower bound. Spec test comment says "accept any `Err(_)` and pin that the cap is observed" — intentional relaxation. Tightening would catch regressions where rejection happens for unrelated reasons (transport closed, internal panic, library returning all-rejected on second call). **Decision:** keep relaxed (per spec) or tighten with status-code substring match + lower-bound assertion. (sources: blind+edge+auditor)
- [x] [Review][Decision-resolved] **No cross-knob coherence validation: `max_chunk_count × 65535 < max_message_size`** — `src/config.rs` `validate()` accepts `max_chunk_count = 1` with `max_message_size = 256 MiB` simultaneously, geometrically inconsistent with OPC UA chunk semantics. Spec only required per-knob validation; adding cross-knob check is scope expansion. **Decision:** add cross-knob validation (defends against misconfig that runs but rejects every real message), or document as out-of-scope and rely on async-opcua to surface the error at runtime. (sources: blind+edge)
- [x] [Review][Decision-resolved] **Unfiltered `DataChangeFilter` silent-dataloss path is documented but not pinned** — `docs/security.md` warns that clients without an explicit `DataChangeFilter` won't see status-only transitions (silent freeze on outage). No automated test pins this fallback; no startup-warn / per-session log emission alerts operators when a non-compliant client connects. AC#3.3 Completion Notes acknowledge this as the "compliant-client contract"; the unfiltered path is a documented gap. **Decision:** add an automated test that pins the silent behaviour (regression baseline), add an operator-facing warn for non-DataChangeFilter monitored items, file an upstream FR with async-opcua, or accept as documented gap. (sources: blind+auditor)

#### Patch (unambiguous fixes — apply now)

- [x] [Review][Patch-applied] **AC#3.3 outage test is wall-clock-flaky on loaded CI** — outer outage loop at `tests/opcua_subscription_spike.rs:~2570-2607` can exit with `saw_stale = false` simply because no notification arrived in any 2s timeout window during the outage, triggering the CRITICAL halt panic spuriously. Fix: increase per-iteration `timeout` from 2s to 5s, or add a deadline-based retry that explicitly counts cap iterations rather than relying on cumulative wall-clock. (sources: blind+edge)
- [x] [Review][Patch-applied] **AC#3.2 comment "silent-truncation failure mode if 0 < total > 3" is malformed** — `tests/opcua_subscription_spike.rs:~2430-2435`. `total > 3` already implies `total > 0`; the inequality reads as nonsense. Fix: rewrite as "silent-truncation failure mode if `0 < total ≤ 3` but cap-rejection didn't fire" or remove the parenthetical entirely. (source: blind)
- [x] [Review][Patch-applied] **AC#3.1 multi-condition assert lacks message** — `tests/opcua_subscription_spike.rs:~2294`: `assert!(sub1 != 0 && sub2 != 0 && sub1 != sub2);`. On failure CI shows no diagnostic. Fix: split into three named asserts or add `"sub1={}, sub2={}"` message. (source: blind)
- [x] [Review][Patch-applied] **`DataChangeFilter` struct constructed without `..Default::default()`** — `tests/opcua_subscription_spike.rs:~2541-2545`. If async-opcua adds a field in a minor bump, the test fails to compile silently or carries wrong filter semantics. Fix: use struct-update syntax `DataChangeFilter { trigger: ..., deadband_type: 0, deadband_value: 0.0, ..Default::default() }`. (source: blind)
- [x] [Review][Patch-applied] **`DataChangeTrigger::StatusValue` cited as "Part 4 §7.17.2 default"** — `tests/opcua_subscription_spike.rs:~2541-2545` and `docs/security.md:735-746`. The auditor flagged that Part 4 §7.17.2's default may actually be `StatusValueTimestamp` (=2), not `StatusValue` (=1). Fix: verify the OPC UA spec text and either correct the citation in test/docs comments or change the trigger to match. (source: auditor) — **note:** verify before patching; auditor may be wrong (`StatusValue` is widely held to be the spec default).
- [x] [Review][Patch-applied] **README.md "Current Version" line out of sync with Planning row** — `README.md:147` still says "(Story 8-1 done; 8-2 ready)" while the Planning row at `:26` flipped to "(8-1 done; 8-2 review)". Fix: bump version line to "8-2 review". (source: auditor)
- [x] [Review][Patch-applied] **`README.md:26` Epic 8 row uses multi-paragraph cell that breaks markdown table rendering** — verbose multi-line status content inside a single table cell renders as one giant cell. Fix: compress to single-line status with cross-link to story file for detail. (source: blind)
- [x] [Review][Patch-applied] **Recovery branch accepts `dv.status.is_none()` as "Good"** — `tests/opcua_subscription_spike.rs:~2620-2651`. `DataValue::status = None` means the server omitted the status code field, not "Good"; in OPC UA semantics this is wire-format ambiguity. A regression that strips status codes from notifications would mask a real protocol bug. Fix: assert explicit `Some(StatusCode::GOOD)` or reject `None` and document. (source: blind+edge)
- [x] [Review][Patch-applied] **`limits_mut()` ordering dependency in `configure_limits` is undocumented** — `src/opc_ua.rs:~339-343` mixes direct setters with `limits_mut().subscriptions.*` mutations. If a future refactor reorders the chain, subscription fields may be silently clobbered (depending on whether any direct setter resets the entire `Limits` struct). Fix: add a one-line comment immediately above the `limits_mut()` block explaining the ordering invariant ("direct setters must precede `limits_mut()` block; library `Default` for `Limits` may overwrite subscription fields"). (source: edge)
- [x] [Review][Patch-applied] **No automated test pins `event="opcua_limits_configured"` field shape** — `src/opc_ua.rs:~660-682` emits the diagnostic event with five structured fields (`max_sessions`, `max_subscriptions_per_session`, `max_monitored_items_per_sub`, `max_message_size`, `max_chunk_count`); operator runbooks in `docs/security.md:~160` rely on grep against this exact line. A future field-name rename silently breaks operator workflows. Spec made `test_resolved_limits_logged_at_startup` optional; given the docs reference is load-bearing, adding a small startup-log capture test is warranted. (sources: blind+auditor)
- [x] [Review][Patch-applied] **Verify `cargo test --lib --bins --tests` count ≥ 615 and `cargo clippy --all-targets -- -D warnings` is clean** — Completion Notes claim 635 pass / clippy clean, but the audit didn't run them. Fix: run both commands, paste tail output into Dev Notes. (source: auditor)

#### Deferred — pre-existing 8-1 spike baseline issues (not 8-2 scope)

- [x] [Review][Defer] **`tests/opcua_subscription_spike.rs:~1011` `clear_captured_buffer` swallows mutex `Err`** — pre-existing 8-1 spike test infrastructure; poisoned mutex on test panic leaves stale buffer for next test. Defer to a future spike-test productionisation pass. (sources: blind+edge)
- [x] [Review][Defer] **`tests/opcua_subscription_spike.rs:~987-1008` `set_global_default` failure not surfaced** — pre-existing 8-1 spike infrastructure; subscriber-install failure passes captured-log assertions spuriously. Defer. (source: edge)
- [x] [Review][Defer] **`tests/opcua_subscription_spike.rs:~1244-1289` `open_session_held` returns `None` for ANY failure** — pre-existing 8-1 helper; conflates auth/network/cap failures so auth-rejection tests pass on transport errors. Defer. (source: edge)
- [x] [Review][Defer] **`tests/opcua_subscription_spike.rs:~1432-1467` 300ms sleep insufficient on loaded CI for tracing flush** — pre-existing 8-1 timing assumption. Defer. (source: edge)
- [x] [Review][Defer] **`tests/opcua_subscription_spike.rs:~1232` `HeldSession::Drop` calls `event_handle.abort()` without await** — pre-existing 8-1 RAII helper; tokio task may bleed across `serial_test` boundaries. Defer. (source: blind)
- [x] [Review][Defer] **`tests/opcua_subscription_spike.rs::test_subscription_double_delete_is_safe` doesn't actually exercise server idempotency** — 8-1 baseline test, purely client-side state check. Defer to spike-productionisation. (source: blind)
- [x] [Review][Defer] **`OpcUaConfig` `Debug` exhaustiveness not pinned by NFR7 regression test** — `src/config.rs:~287-309`. Future field added without `Debug` entry silently disappears from logs. Defer — NFR7 carry-forward debt, not 8-2 scope. (source: blind)
- [x] [Review][Defer] **`OPCUA_DEFAULT_MAX_MESSAGE_SIZE = 65_535 * 5` is a hardcoded literal, not a re-export of `opcua_types::constants::MAX_MESSAGE_SIZE`** — `src/utils.rs:~838`. Library bump silently diverges. Defer — `static_assertions::const_assert_eq!` against the library constant is a small follow-up; not blocking. (sources: blind+edge)



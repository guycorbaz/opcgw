# Story 9.1: Axum Web Server and Basic Authentication

**Epic:** 9 (Web Configuration & Hot-Reload — Phase B)
**Phase:** Phase B
**Status:** review
**Created:** 2026-05-02
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md` was authored before Phase A was renumbered. The story this file implements lives in `epics.md` as **"Story 8.1: Axum Web Server and Basic Authentication"** under **"Epic 8: Web Configuration & Hot-Reload (Phase B)"** (lines 770–786). In `sprint-status.yaml` and the rest of the project this is **Story 9-1** under **Epic 9**. Same work, different numbering. The Phase-B carry-forward bullets at `epics.md:768–784` apply to this story; the HMAC-keying-pattern reuse from Story 7-2 (`epics.md:782`) and the library-wrap-not-fork default (`epics.md:784`) are the load-bearing inputs.

---

## User Story

As an **operator**,
I want a lightweight web server embedded in the gateway with authentication,
So that I can access configuration and status pages securely from any device on the LAN (FR50, NFR11, NFR12, FR41).

---

## Objective

Stand up an Axum 0.8 web server inside the gateway process that:

1. **Listens on a configurable HTTP port** sharing the same Tokio runtime as the existing poller and OPC UA server tasks. The port + bind address are new `[web]` config knobs in `OpcUaConfig`'s sibling shape (see § Field-shape table in AC#1).
2. **Enforces HTTP Basic authentication** on every route via a single tower-style middleware layer that reuses the **HMAC-SHA-256-keyed credential digest pattern** established by Story 7-2's `OpcgwAuthManager` (`src/opc_ua_auth.rs:42-66`). New module `src/web/auth.rs` extracts a small `HmacCredentialDigest` shared primitive; the OPC UA path keeps its existing `OpcgwAuthManager` shape (no behavioural change there) but the underlying digest function is shared. **Do NOT roll a new credential comparison from scratch** — this is an explicit Phase-B carry-forward rule from the Epic 8 retrospective (`epics.md:782`).
3. **Logs failed authentication attempts with source IP** in the same NFR12 audit-event shape as `event="opcua_auth_failed"` (Story 7-2). New event name: `event="web_auth_failed"`. The web server has direct access to peer `SocketAddr` via `axum::extract::ConnectInfo<SocketAddr>` (no two-event correlation workaround needed — that pattern was specific to async-opcua's missing `AuthManager` peer-addr).
4. **Serves static HTML files from the `static/` directory** at the namespace root (`/`, `/applications.html`, `/devices.html`, etc.) via `tower-http::services::ServeDir`. The HTML files themselves ship as empty placeholders in this story; Stories 9-2 / 9-3 / 9-4 / 9-5 / 9-6 fill them in.
5. **Respects the existing `CancellationToken`** for graceful shutdown coordination, joining the existing `tokio::select!` shutdown sequence in `src/main.rs:704+` alongside `chirpstack_handle` / `opcua_handle` / `poller_handle` / `timeout_handle`.
6. **Rejects malformed `Authorization` headers** (non-Basic schemes, missing colon in the decoded user:pass, non-UTF8 bytes after base64 decode) with `401 Unauthorized` + `WWW-Authenticate: Basic realm="..."` + the same `web_auth_failed` audit event. The realm string lives in a new constant in `src/utils.rs`.

The new code surface is **modest** — estimated **~250–400 LOC of production code + ~250–400 LOC of tests + ~100 LOC of docs**. Most of the auth path is shared with Story 7-2's `OpcgwAuthManager` once the HMAC digest function is extracted; the Axum-specific surface is a single `Router` + a single middleware `Layer` + a small `ServeDir` mount.

This story closes **FR50** (basic auth), satisfies **NFR11** (web UI requires authentication before any configuration change — pre-empted at the middleware layer; no route is reachable without successful auth), satisfies **NFR12** (failed authentication attempts logged with source IP — directly available via Axum), and partially closes **FR41** (mobile-responsive: the *server* serves static content; the actual responsive HTML/CSS lands in Stories 9-2 / 9-3 with the dashboard + live-metrics pages).

It does **not** ship any *content* (the static HTML files are placeholders; Stories 9-2 / 9-3 fill them in) and it does **not** ship any *configuration mutation* paths (Stories 9-4 / 9-5 / 9-6 cover application / device / command CRUD; Story 9-7 covers hot-reload).

---

## Out of Scope

- **Static HTML page bodies.** This story creates `static/index.html`, `static/applications.html`, `static/devices.html`, `static/commands.html` as **empty placeholders** with a `<title>` and a one-line "Story 9-X will fill this in" stub. The actual page bodies are Stories 9-2 (status dashboard), 9-3 (live metrics), 9-4 (application CRUD), 9-5 (device + metric CRUD), 9-6 (command CRUD).

- **REST API endpoints for configuration mutation.** Stories 9-4 / 9-5 / 9-6 own `POST` / `PUT` / `DELETE` routes for application / device / command CRUD. Story 9-1 mounts the auth middleware so those future endpoints inherit it; Story 9-1 itself ships **only `GET /` + `GET /<static-file>`** plus a single `GET /api/health` smoke endpoint that returns `{"status":"ok"}` (used by the integration tests to verify the auth middleware fires; not an operator-facing endpoint).

- **TLS / HTTPS.** Story 9-1 ships HTTP-only. The threat model is LAN-internal deployments where TLS termination happens at an upstream reverse proxy or is operationally unnecessary (the same stance Story 7-2 took for the OPC UA `None` endpoint — username/password is transmitted in cleartext over the LAN; it's the operator's environment-of-deployment decision to add TLS via reverse-proxy if needed). Tracked at GitHub issue **TBD** for a future hardening story; **NOT** in 9-1's scope.

- **Per-IP rate limiting / brute-force protection.** Phase A carry-forward GitHub issue [#88](https://github.com/guycorbaz/opcgw/issues/88) (per-source-IP token-bucket throttling) becomes structurally relevant once the Web UI auth surface lands, but the gateway's LAN-internal threat model defers it. The flat global session/connection caps from Story 7-3 + the basic-auth challenge themselves are the load-shaping surface in 9-1. Per-IP throttling is a separate Phase B story to be opened only if web-auth-flood becomes a near-term operator concern. Out of scope; tracked at #88.

- **mTLS / X.509 client-cert auth.** Same shape as the Story 7-2 deferred entry — username/password (FR50) only. Out of scope; revisit when role-based access control becomes a requirement.

- **Multi-user web auth.** Story 9-1 keeps the single-user model the same as Story 7-2 (one `[web].user_name` + `[web].user_password` pair, or — preferred — reuse `[opcua].user_name` / `[opcua].user_password` since the threat model treats both surfaces symmetrically). The **field-shape decision** is in AC#1 below: the recommended approach is **one shared `[auth]` block** (renamed from `[opcua]`'s username/password fields) so a single credential change updates both surfaces. Out of scope: multi-user / RBAC.

- **Session cookies / JWT / "remember me".** HTTP Basic Auth is stateless (browser caches credentials per-realm); there is no server-side session state to manage. NFR11 says "basic auth minimum"; sticking to the minimum keeps the attack surface small. Out of scope.

- **CSRF protection.** Story 9-1 ships only `GET` routes + the `/api/health` smoke endpoint (also `GET`). Stories 9-4 / 9-5 / 9-6 will add `POST` / `PUT` / `DELETE` routes that need CSRF protection (or a CORS-restricted `application/json`-only contract that browsers reject by same-origin policy). The decision lives in those stories' specs; 9-1 documents the requirement in `docs/security.md` so the next-story author doesn't miss it.

- **`tower-http`'s `BasicAuth` extractor.** Considered and rejected — the upstream extractor uses plain string compare on the configured plaintext password, which has the same length-oracle weakness Story 7-2 closed via HMAC. Custom middleware reusing `src/opc_ua_auth.rs::hmac_sha256` is the right shape. The `tower-http` crate is still pulled in for `ServeDir`; just not for auth.

- **Web UI hot-reload of `[web]` config.** Story 9-7 covers configuration hot-reload across all subsystems; the new `[web]` config block is read at startup only, identical to other config sections. Out of scope; tracked at issue [#90](https://github.com/guycorbaz/opcgw/issues/90).

- **Story 9-0 spike output** (dynamic OPC UA address-space mutation). Story 9-1 does not depend on 9-0; the web server is a separate transport with no OPC UA address-space mutation. 9-0 is a prerequisite for 9-7 / 9-8, not for 9-1 / 9-2 / 9-3.

- **Doctest cleanup, `tests/common/mod.rs` extraction, spike-test productionisation.** All landed as Epic-8-retro carry-forward commits before 9-1 starts (commits `4445cc8`, `16b071d`, `c175a17`). Story 9-1 inherits the cleaned-up infrastructure.

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. The story's job is to **plumb a new transport on top of code that already does the heavy lifting** — the HMAC keying primitive, the `CancellationToken` shutdown coordination, the `tokio::spawn` task pattern, the structured-tracing audit-event convention, the `tests/common/mod.rs` test harness, the redacting `Debug` impls all exist.

| What | Where | Status |
|------|-------|--------|
| **HMAC-SHA-256 keyed credential digest** | `src/opc_ua_auth.rs:42-66` (`hmac_sha256` fn + per-process `hmac_key: [u8; 32]` random secret) | **Wired today (Story 7-2).** Used by `OpcgwAuthManager` for OPC UA basic auth. **Story 9-1 extracts the `hmac_sha256` function + the `hmac_key` random-init pattern into a new `src/security_hmac.rs` module** (or — simpler — re-exports the existing function via `pub` in `src/opc_ua_auth.rs`). The web auth middleware computes `HMAC-SHA-256(hmac_key, submitted_user_or_pass)` and `subtle::ConstantTimeEq`-compares against pre-computed configured digests. **Do NOT roll a new comparison from scratch** — this is an explicit Phase-B carry-forward rule (`epics.md:782`). |
| **`subtle::ConstantTimeEq`** | `Cargo.toml` (transitive via `hmac` crate, but Story 7-2 also uses `subtle = "2"` directly — verify) | **Wired today.** Story 9-1 uses the same import path. |
| **`hmac` + `sha2` crates** | `Cargo.toml:23` (`hmac = "0.12"`) + transitive `sha2` from Story 7-2 | **Wired today.** No new crate needed for the digest path. |
| **`getrandom` crate** | `Cargo.toml` (Story 7-2 added it for the per-process HMAC key) | **Wired today.** Story 9-1 reuses this for any new random-secret generation (or, simpler, **shares Story 7-2's `hmac_key` so both auth surfaces use the same per-process secret** — this is the cleaner shape: one `hmac_key` per process, two consumers). |
| **`AppConfig` struct + redacting `Debug` impls** | `src/config.rs:148-262` (struct), `:287-309` (`OpcUaConfig::Debug`) | **Wired today (Story 7-1).** Adding new credential fields to a `WebConfig` struct (or — preferred — adding `[web].port` + `[web].bind_address` to a new struct without credentials, since the credentials are shared with `[opcua]`) requires the same hand-written `Debug` impl pattern. Story 9-1 must NOT add a new password field that is not redacted in `Debug`. |
| **`AppConfig::validate` accumulator pattern** | `src/config.rs:739-` (entry), `:802-841` (existing OpcUa block) | **Wired today.** New web-config validation entries follow the exact same shape: `Some(0)` rejected with actionable hint, port out of `[1024, 65535]` rejected (avoids the privileged-port range), bind address parseable as `IpAddr`. |
| **Env-var override convention** | figment + `Env::prefixed("OPCGW_").split("__")` (Story 7-1) | **Wired today.** `OPCGW_WEB__PORT=8080` automatically overrides `[web].port`. No code change required. |
| **`CancellationToken` shutdown coordination** | `src/main.rs:486` (creation), `:654-687` (task spawning), `:704+` (`tokio::select!` shutdown) | **Wired today (Story 1-4).** Story 9-1 adds a 5th `tokio::spawn(async move { web_server.run(cancel_clone).await })` and a corresponding branch in the `tokio::select!` for shutdown. Pattern is mechanical replication of the existing OPC UA / poller spawns. |
| **`tokio::spawn` + task lifecycle pattern** | `src/main.rs:654-704` | **Wired today.** Five existing spawns: `chirpstack_handle`, `opcua_handle`, `poller_handle`, `timeout_handle`. Story 9-1 adds `web_handle` following the same shape. |
| **NFR12 startup-warn (Story 7-2 commit `344902d`)** | `src/main.rs::initialise_tracing` | **Wired today.** Emits a one-shot `warn!` when the global level filters out `info`. Story 9-1's `event="web_auth_failed"` audit event is `warn!`-level (same as `event="opcua_auth_failed"`), so the source-IP correlation requirement applies identically to web sessions. The startup warn covers both surfaces. **Story 9-1 must NOT re-implement.** |
| **Tracing event-name convention** | Stories 6-1, 7-2, 7-3, 8-2 (`event="opcua_auth_failed"`, `event="opcua_session_count"`, `event="opcua_session_count_at_limit"`, `event="opcua_limits_configured"`, `event="pki_dir_initialised"`) | **Established.** Story 9-1 introduces **two new events**: `event="web_auth_failed"` (warn-level audit; mirrors `opcua_auth_failed` shape) and `event="web_server_started"` (info-level diagnostic; emitted at startup with resolved port + bind address + auth realm). The diagnostic event is NOT an audit event (same distinction Story 8-2's `opcua_limits_configured` set). |
| **`OpcGwError` variants** | `src/utils.rs::OpcGwError` | Use `OpcGwError::Configuration` for startup config validation failures (e.g., port out of range, bind address unparseable). Use a new `OpcGwError::Web` variant for runtime web-server errors (bind failure, request handling errors). **Add the `Web` variant** in this story; the cost is one line in the enum + one `#[error]` annotation. |
| **`tests/common/mod.rs`** | `tests/common/mod.rs` (Story 9-1's predecessor cleanup; commit `16b071d`) | **Wired today.** `pick_free_port`, `build_client(spec)`, `user_name_identity` are reusable. Story 9-1's web-server integration tests will add a new helper here: `build_http_client()` returning a `reqwest::Client` configured for the test deployment shape (basic-auth, no TLS, generous timeouts). The new helper is the **third caller** of a future `reqwest::Client`-shaped wrapper, so adding it to `tests/common/mod.rs` is correct per CLAUDE.md scope-discipline. |
| **`#[serial_test::serial]` annotation** | `Cargo.toml` (Story 7-3 added `serial_test = "3"`) | **Wired today.** Story 9-1's integration tests use it to serialise port-binding races on shared CI runners. Mandatory because each test sets a global tracing subscriber via `init_test_subscriber()`. |
| **`init_test_subscriber()` pattern** | `tests/opcua_subscription_spike.rs:82-103` (with the issue #101 fixes) | **Diverging per-file** (documented in `tests/common/mod.rs` divergence section). Story 9-1's web-tests need event capture for `event="web_auth_failed"` assertions, so they install their own subscriber composition matching the spike-test pattern (custom `tracing_test::internal::global_buf()` + an `OnceLock`-guarded `set_global_default`). The pattern is well-documented; reuse the spike-test shape. |
| **Documentation extension target** | `docs/security.md` | **Existing file.** Story 9-1 adds a new top-level section `## Web UI authentication` (peer to `## OPC UA security endpoints and authentication`) with five subsections matching the established shape: What it is / Configuration / What you'll see in the logs / Anti-patterns / Tuning checklist. The shared-credentials note (`[opcua]` user/pass = `[web]` user/pass) lives here. |
| **`config/config.toml` + `config/config.example.toml`** | `config/config.toml`, `config/config.example.toml` | **Wired today.** Story 9-1 adds a new `[web]` block (port, bind_address, auth_realm) with `OPCGW_WEB__*` env-var override comment. The shipped `config.toml` ships the block commented-out behind `enabled = false`-style flag — see AC#5 below for the disable-by-default pattern. |
| **Library-wrap-not-fork pattern** | Project-shaping pattern (3 epics, 3 uses: `OpcgwAuthManager`, `AtLimitAcceptLayer`, `OpcgwHistoryNodeManager`) | **Established.** Story 9-1's basic-auth middleware is the 4th use: a tower-style `Layer` that wraps every route, extracts `Authorization`, computes the HMAC digest, and rejects with 401 + audit event on failure. Composition over forking. |
| **`docs/manual/opcgw-user-manual.xml`** | `docs/manual/` | **Lagging.** v2.0 covers Epics 2-6 only. Story 9-1's web-auth surface should land a manual chapter as part of CLAUDE.md "Documentation Sync" — but per Epic 7/8 retro pattern, the manual update can be batched (it's not blocking dev work). **Story 9-1 documents the change in `docs/security.md` and notes the manual sync as a deferred-work entry**, NOT a blocking AC. |

**Epic-spec coverage map** — the BDD acceptance criteria from `epics.md` (lines 778–786) break down as:

| Epic-spec criterion (line ref) | Already known? | Where this story addresses it |
|---|---|---|
| Web server listens on a configurable HTTP port (line 780) | ❌ no web server today | **AC#1** — `[web].port` config knob + `ServerBuilder` integration. |
| All routes require basic authentication (FR50, NFR11) (line 781) | ❌ no auth middleware today | **AC#2** — `BasicAuthLayer` wrapping the entire `Router`. |
| Failed authentication attempts logged with source IP (NFR12) (line 782) | ❌ no event today | **AC#3** — `event="web_auth_failed"` + `ConnectInfo<SocketAddr>`. |
| Web server shares the Tokio runtime with other tasks (line 783) | ❌ no web server today | **AC#4** — `tokio::spawn` in `main.rs` + shared runtime. |
| Web server respects the CancellationToken (line 784) | ❌ no web server today | **AC#4** — `axum::serve` with `with_graceful_shutdown(cancel.cancelled())`. |
| Static HTML files served from `static/` (line 785) | ❌ no static mount today | **AC#5** — `tower_http::services::ServeDir` mount + placeholder HTML files. |
| Web UI mobile-responsive (FR41) (line 786) | ❌ no HTML today | **AC#5** — placeholder `<head>` includes `<meta name="viewport" ...>`; actual responsive content lands in Stories 9-2 / 9-3. |
| `cargo test` clean + `cargo clippy --all-targets -- -D warnings` clean | Implicit per CLAUDE.md | **AC#6** — Story 9-1 baseline 309 lib+bins / clippy clean (post-Epic-8-carry-forward); Story 9-1 target ≥ 320 with new web-server tests added. |
| HMAC keying reuse from Story 7-2 (Phase B carry-forward, `epics.md:782`) | ⚠️ extraction needed | **AC#2** — extracts `hmac_sha256` to a shared module; OPC UA path adopts it without behavioural change. |
| Library-wrap pattern for missing async-opcua callbacks (`epics.md:784`) | n/a — Axum is not async-opcua | **No work** — wrap pattern doesn't apply (Axum has full middleware support). Documented in Dev Notes. |

---

## Acceptance Criteria

### AC#1: New `[web]` configuration block with port + bind address + auth realm (FR50, line 780)

**Knob list** (all `Option<...>` for forward compatibility; library defaults apply when `None`):

| Knob | TOML key | Default | Env var | Validation |
|---|---|---|---|---|
| `port` | `[web].port` | `8080` | `OPCGW_WEB__PORT` | Reject `< 1024` (privileged range), reject `0`, reject `> 65535`. |
| `bind_address` | `[web].bind_address` | `"0.0.0.0"` | `OPCGW_WEB__BIND_ADDRESS` | Must parse as `IpAddr`; reject otherwise. |
| `auth_realm` | `[web].auth_realm` | `"opcgw"` | `OPCGW_WEB__AUTH_REALM` | Reject empty string, reject strings containing `"` (would break `WWW-Authenticate` header). Truncate to 64 chars. |
| `enabled` | `[web].enabled` | `false` | `OPCGW_WEB__ENABLED` | Bool. **Defaults to `false`** so existing operators upgrading from Phase A don't get an unexpected new listening port without opt-in. The shipped `config.toml` documents the opt-in step. |

**Field-shape table** — exactly mirroring Story 7-3's `max_connections` pattern + Story 8-2's per-knob constants:

| Field | Type | Source-of-truth constant in `src/utils.rs` |
|---|---|---|
| `port` | `Option<u16>` | `WEB_DEFAULT_PORT: u16 = 8080`, `WEB_MIN_PORT: u16 = 1024`, `WEB_MAX_PORT: u16 = 65535` |
| `bind_address` | `Option<String>` | `WEB_DEFAULT_BIND_ADDRESS: &str = "0.0.0.0"` |
| `auth_realm` | `Option<String>` | `WEB_DEFAULT_AUTH_REALM: &str = "opcgw"`, `WEB_AUTH_REALM_MAX_LEN: usize = 64` |
| `enabled` | `Option<bool>` | `WEB_DEFAULT_ENABLED: bool = false` |

**Credentials** are shared with `[opcua]` — `[web]` does NOT introduce a new user/password pair. The web auth middleware reads `config.opcua.user_name` / `config.opcua.user_password` directly. The shared-credentials decision is documented in `docs/security.md`. (Rationale: same threat model — operator with LAN access; one credential rotation step covers both surfaces; one less credential pair for operators to forget to rotate.)

**Verification:**
- New `WebConfig` struct in `src/config.rs` with the four `Option<...>` fields above + hand-written `Debug` impl (no secrets, but the pattern is the project standard).
- `AppConfig.web: WebConfig` field added; `#[serde(default)]` on each `Option<...>` so absent TOML entries deserialise to `None`.
- `AppConfig::validate` rejects out-of-range port, unparseable bind address, empty / quote-containing auth_realm. Same accumulator pattern as `OpcUaConfig::validate`.
- 4 new unit tests in `src/config.rs::tests`: each invalid input (port=0, port=80, port=70000, bind="not-an-ip", realm="", realm contains `"`) produces a single combined error message.
- `OPCGW_WEB__PORT=9090` env var overrides TOML default — pinned by 1 integration test.
- `config/config.toml` ships the `[web]` block commented-out with `enabled = false` documented inline; `config/config.example.toml` shows a representative non-default deployment shape.

---

### AC#2: HTTP Basic Auth middleware reuses HMAC-SHA-256 keyed digest from Story 7-2 (FR50, NFR11, `epics.md:782`)

**Implementation:**

- Extract `hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32]` from `src/opc_ua_auth.rs:42-66` to a new `pub` location. **Two acceptable shapes** (dev-agent decides):
  - **Shape A (preferred):** Move to `src/security_hmac.rs` as `pub fn hmac_sha256`. Update `src/opc_ua_auth.rs` to `use crate::security_hmac::hmac_sha256`. Net diff: ~25 LOC moved + 2 import lines added.
  - **Shape B (acceptable):** Keep in `src/opc_ua_auth.rs` but mark `pub` and re-export at `lib.rs`. Web auth path imports from `opcgw::opc_ua_auth::hmac_sha256`. Net diff: ~3 LOC visibility changes.
  - **Shape A** wins on long-term cleanliness (the function isn't OPC-UA-specific); **Shape B** wins on minimum-diff. Either is fine; document the choice in completion notes.
- New `src/web/auth.rs` module (~120 LOC):
  - `WebAuthState { user_digest: [u8; 32], pass_digest: [u8; 32], hmac_key: [u8; 32], realm: String }` struct.
  - `WebAuthState::new(config: &AppConfig) -> Self` — reads `config.opcua.user_name` / `user_password` (NOT `config.web.*`; credentials are shared per AC#1), generates a fresh per-process `hmac_key` via `getrandom`, computes the two digests, drops the plaintext.
  - **OR (cleaner):** share the same `hmac_key` already living in `OpcgwAuthManager` so both auth surfaces use one per-process secret. Refactor: `OpcgwAuthManager` exposes `pub fn hmac_key(&self) -> &[u8; 32]`; `WebAuthState::from_opcua_auth(opcua: &OpcgwAuthManager, realm: String) -> Self`. **This shape is preferred** — one `hmac_key` per process is cleaner; revisit only if the dev agent finds a pinch point.
  - `pub fn basic_auth_middleware(State(state): State<Arc<WebAuthState>>, ConnectInfo(addr): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Result<Response, Response>` (Axum 0.8 middleware signature).
- Middleware behaviour:
  1. Extract `Authorization` header. Missing → return 401 + `WWW-Authenticate: Basic realm="<auth_realm>"` + emit `event="web_auth_failed"` warn.
  2. Validate scheme: must be `Basic <base64>`. Missing scheme prefix → 401 + `web_auth_failed`.
  3. Decode base64 → expect `<user>:<pass>`. Decoding error → 401 + `web_auth_failed`.
  4. Split on first `:` (subsequent colons go to the password). Missing colon → 401 + `web_auth_failed`.
  5. Compute `submitted_user_digest = hmac_sha256(hmac_key, submitted_user.as_bytes())`; same for password.
  6. `subtle::ConstantTimeEq::ct_eq` compare both digests. Either fails → 401 + `web_auth_failed`.
  7. Both pass → forward to `next.run(req).await`.
- The 401 response always has a constant-time path identical to the success path (no early-return on unknown user vs. wrong password — both produce 401 with the same headers). This closes the user-existence side-channel.
- Username sanitisation for the audit event uses the same `escape_default` + 64-char truncation pattern as `OpcgwAuthManager::sanitise_user` (`src/opc_ua_auth.rs:76-78`). The submitted user goes into the audit event as `user="<sanitised>"`; the password **never** appears in any log at any level (NFR7 invariant).

**Verification:**
- `WebAuthState::new` (or `::from_opcua_auth`) is called once at server startup and the `Arc<WebAuthState>` is shared across all middleware invocations.
- 6 unit tests in `src/web/auth.rs::tests`: missing `Authorization`, malformed scheme, malformed base64, missing colon, wrong user, wrong password. Each asserts 401 + the audit event fires exactly once.
- 1 unit test asserting the success path forwards to `next` and does NOT emit `web_auth_failed`.
- 1 integration test in `tests/web_auth.rs` exercising the full HTTP request → 401 → header inspection round-trip with a real HTTP client.
- `subtle = "2"` in `Cargo.toml` (verify; Story 7-2 may have already added it as a direct dep — if not, add it).
- `cargo clippy --all-targets -- -D warnings` clean.

---

### AC#3: Failed authentication attempts logged with source IP (NFR12, line 782)

**Audit-event shape** (mirrors `event="opcua_auth_failed"`):

```
warn event="web_auth_failed" source_ip=<peer-ip> user="<sanitised-user-or-blank>" path=<request-path> reason=<missing|malformed_scheme|malformed_base64|missing_colon|user_mismatch|password_mismatch>
```

- `source_ip` extracted from `axum::extract::ConnectInfo<SocketAddr>` → IP portion only (port stripped). The web server has direct access to peer addr — **no two-event correlation pattern needed** (that pattern was specific to async-opcua's missing `AuthManager` peer-addr; Axum has it natively via `ConnectInfo`).
- `user` field uses `sanitise_user(submitted_user)` from `src/opc_ua_auth.rs:76-78`. If the submitted credentials couldn't be decoded (missing header, malformed base64, missing colon), `user=""` is emitted (empty string, not absent field — pinned by the test).
- `reason` field discriminates the failure mode for operator triage. Six values: `missing` (no Authorization header), `malformed_scheme` (not Basic), `malformed_base64`, `missing_colon`, `user_mismatch`, `password_mismatch`. The `user_mismatch` vs `password_mismatch` distinction is **deliberate** — the constant-time comparison still happens for both digests, so the timing is identical regardless of which mismatched. The audit event records which one for forensic purposes; the response to the client is identical (401 + same headers).
- `path` field carries the request path so operators can grep for repeated probes against `/api/applications` etc. once Stories 9-4+ ship.

**Verification:**
- 6 unit tests (one per `reason` value) assert the audit event fires with the correct `reason` field.
- 1 integration test sends a known-bad request with a recognisable user string and a recognisable source IP, then asserts the captured tracing-test buffer contains a single matching audit-event line.
- The audit event is **warn-level** so it survives the NFR12 startup-warn check (operator must run with log level ≥ `info` for the source-IP correlation to be visible — same constraint as Story 7-2).
- The `event="web_auth_failed"` field name is added to `docs/security.md` operations reference table.

---

### AC#4: Web server shares Tokio runtime + respects CancellationToken (lines 783-784)

**Implementation:**

- New `src/web/mod.rs` with `pub fn build_router(state: AppState) -> Router` (Axum 0.8 builder pattern). `AppState` holds `Arc<WebAuthState>` + `Arc<dyn StorageBackend>` (the latter for future Stories 9-2 / 9-3 / 9-4+ to read gateway state; not used by 9-1 but threaded through so future stories don't refactor).
- New `pub async fn run(addr: SocketAddr, router: Router, cancel: CancellationToken) -> Result<(), OpcGwError>` that:
  - Calls `axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>())` (the `with_connect_info` is required for `ConnectInfo<SocketAddr>` extraction in the auth middleware).
  - Wraps the serve future in `.with_graceful_shutdown(cancel.cancelled().await)` so cancellation drains in-flight requests cleanly.
  - Returns `Ok(())` on graceful shutdown; returns `Err(OpcGwError::Web(...))` on bind failure or unexpected I/O error.
- New `src/main.rs` integration:
  - Add `if config.web.enabled.unwrap_or(WEB_DEFAULT_ENABLED) { ... }` block after the existing OPC UA spawn at `src/main.rs:660`.
  - Inside the block: build state, build router, spawn `web_handle = tokio::spawn(async move { web::run(addr, router, cancel_clone).await })`.
  - Add a 5th branch to the existing `tokio::select!` at `src/main.rs:704+` that joins the `web_handle` (logs `event="web_server_started"` info on first successful bind, `event="web_server_stopped"` info on graceful shutdown).
  - The web server is **opt-in** via `[web].enabled = true` (default `false` per AC#1) so existing operators don't get a surprise new listening port on upgrade.

**Verification:**
- Manual smoke: `cargo run` with `OPCGW_WEB__ENABLED=true` starts the gateway and binds to `0.0.0.0:8080`. `curl -u opcua-user:test-password http://localhost:8080/api/health` returns `{"status":"ok"}`. `curl http://localhost:8080/api/health` (no auth) returns 401 + `WWW-Authenticate` header.
- Integration test: send SIGTERM-equivalent (call `cancel.cancel()` programmatically), assert `web_handle` joins within 5 seconds. The `tokio::select!` shutdown sequence behaves identically to the OPC UA / poller paths.
- The `event="web_server_started"` info event fires exactly once at startup with the resolved port + bind address.
- `cargo clippy --all-targets -- -D warnings` clean.

---

### AC#5: Static HTML files served from `static/` (line 785, FR41)

**Implementation:**

- `tower_http::services::ServeDir::new("static")` mounted at `/` via `Router::nest_service("/", ServeDir::new("static"))`.
- Mount applies the auth middleware via `Router::layer(middleware::from_fn_with_state(state, basic_auth_middleware))` — the `ServeDir` routes inherit the layer. Verify by integration test: unauth `GET /index.html` returns 401, auth'd returns the file.
- Placeholder HTML files in `static/`:
  - `static/index.html` — `<title>opcgw — Dashboard</title>` + `<meta name="viewport" content="width=device-width, initial-scale=1">` + `<body><p>Story 9-2 will fill this in.</p></body>`.
  - `static/applications.html`, `static/devices.html`, `static/commands.html` — same shape, with story-9-X stub text in the body.
- The `<meta viewport>` tag is the **only** mobile-responsive bit shipped in 9-1; AC#5 is "the server can serve mobile-responsive content", not "the content is mobile-responsive". The full responsive HTML/CSS lands in Stories 9-2 / 9-3.

**Verification:**
- 4 placeholder HTML files exist with the documented `<head>` contents.
- Integration test: auth'd `GET /index.html` returns 200 + body contains `<meta name="viewport"`.
- Integration test: unauth'd `GET /index.html` returns 401 + audit event fires.
- Integration test: `GET /nonexistent.html` returns 404 (auth still gates the response — even unknown paths require auth before the 404 is served, otherwise an unauthenticated attacker can probe the file system structure via 404-vs-401 differences).

---

### AC#6: Tests pass + clippy clean + no regression (CLAUDE.md compliance)

**Verification:**
- `cargo test --lib --bins`: ≥ 320 passed (was 309 baseline post-Epic-8-carry-forward); growth from new web-config validation tests + new web-auth unit tests.
- `cargo test --tests`: existing 14 integration test binaries still pass; new `tests/web_auth.rs` adds ≥ 5 integration tests (auth fail-modes + AC#5 static-file serving + AC#4 graceful shutdown).
- `cargo clippy --all-targets -- -D warnings` clean across the workspace.
- `cargo test --doc`: 0 failed (carries the issue #100 baseline; new code adds no new doctests).
- The Story 8-1 / 8-2 / 8-3 spike+history+subscription tests are **regression baselines** — must continue to pass unchanged. Story 9-1 must NOT modify `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/opc_ua_session_monitor.rs` beyond the optional `OpcgwAuthManager::hmac_key()` accessor mentioned in AC#2. AC#7 below pins this with a `git diff` check.

---

### AC#7: NFR12 + auth + connection-cap carry-forward intact (no regression on prior epics)

**Implementation:**

- Story 9-1 must NOT modify `src/opc_ua_auth.rs` beyond the `hmac_sha256` extraction (Shape A) or the `pub` visibility change (Shape B) and the optional `pub fn hmac_key(&self) -> &[u8; 32]` accessor for shared-key reuse.
- Story 9-1 must NOT modify `src/opc_ua_session_monitor.rs` (zero LOC change).
- Story 9-1 must NOT modify `src/main.rs::initialise_tracing` (the NFR12 startup warn from commit `344902d` covers both auth surfaces unchanged).
- Story 9-1 must NOT introduce a new audit event distinct from `event="web_auth_failed"`. The diagnostic event `event="web_server_started"` is explicitly NOT an audit event.

**Verification:**
- `git diff --stat src/opc_ua_session_monitor.rs` over the 9-1 branch must show `0 insertions, 0 deletions`.
- `git diff src/opc_ua_auth.rs` shows only the visibility / extraction changes from AC#2; no logic changes. Pinned by reading the diff in completion notes.
- The existing `tests/opcua_subscription_spike.rs::test_subscription_client_rejected_by_auth_manager` and `tests/opcua_history.rs::test_history_read_*` continue to pass without modification.

---

### AC#8: Sanity check on regression-test count and audit-event count

**Verification:**
- Default test count grows by ~14 (≈ 4 web-config validation + 8 web-auth unit + 5 web-auth integration + 1 web-server lifecycle integration; minor variance acceptable). **Document the actual count** in completion notes alongside the pre-Story baseline (309 lib+bins post-Epic-8 carry-forward).
- Exactly **two** new tracing-event names introduced by Story 9-1: `web_auth_failed` (audit) and `web_server_started` (diagnostic). Add both to `docs/security.md` operations reference + `docs/logging.md` event registry.
- `git grep "event=\"web_" src/` shows only those two values; no third event slipped in.
- Zero new audit events on the OPC UA path (AC#7 invariant).

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issues (CLAUDE.md compliance) (AC: All)

- [x] Open main tracker issue: "Story 9-1: Axum Web Server and Basic Authentication" — reference this story file, link to the Phase-B carry-forward bullets. **Issue #103.**
- [x] Open follow-up issue (or note in deferred-work.md): "Story 9-1 follow-up: web TLS / HTTPS hardening" — captures the explicit out-of-scope decision. **Issue #104.**
- [x] Note in deferred-work.md: "Story 9-1: User-manual chapter for web auth" — Documentation Sync deferral.

### Task 1: Add `WebConfig` struct + validation (AC: 1)

- [x] Add `WebConfig { port: Option<u16>, bind_address: Option<String>, auth_realm: Option<String>, enabled: Option<bool> }` to `src/config.rs` with `#[derive(Deserialize, Default)]` + hand-written `Debug` impl.
- [x] Add `web: WebConfig` field to `AppConfig` with `#[serde(default)]`.
- [x] Add `WEB_DEFAULT_*` + `WEB_MIN_PORT` + `WEB_MAX_PORT` + `WEB_AUTH_REALM_MAX_LEN` constants to `src/utils.rs`.
- [x] Add validation entries to `AppConfig::validate` for port range, bind address parseability, auth_realm content/length.
- [x] 4 new unit tests in `src/config.rs::tests` for each invalid input. **5 tests landed** (port-below-floor, bind-address-unparseable, auth-realm-empty-or-quote, auth-realm-too-long, all-none-passes — the 5th is the regression pin that the absent block validates cleanly).
- [x] 1 integration test verifying `OPCGW_WEB__PORT=9090` env-var override. **`test_web_port_nested_env_override` in `src/config.rs::tests`.**

### Task 2: Extract HMAC primitive + add `WebAuthState` (AC: 2)

- [x] Decide shape A vs B (cf. AC#2). Default to A (move to `src/security_hmac.rs`). **Shape A chosen** (extracted `hmac_sha256` + the 4 hmac_sha256 unit tests into new `src/security_hmac.rs`).
- [x] Apply the chosen extraction; verify `OpcgwAuthManager` continues to compile + tests pass. **All 10 `opc_ua_auth::tests` pass post-extraction.**
- [x] Add `OpcgwAuthManager::hmac_key()` accessor (returns `&[u8; 32]`) so `WebAuthState` shares the same per-process secret. **Accessor added; `WebAuthState::from_opcua_auth` exposed for symmetry but not used in production because AC#6 forbids modifying `src/opc_ua.rs` to surface the auth manager from `main.rs`. Production uses `WebAuthState::new(config, realm)` (Shape 1 from AC#2) with its own per-process key.**
- [x] Implement `src/web/auth.rs::WebAuthState::from_opcua_auth(opcua, realm)` + `basic_auth_middleware` per AC#2's middleware behaviour. **Both shapes implemented; `new` is the production entry point.**
- [x] 8 unit tests covering the 6 failure modes + the 1 success path + the 1 sanitisation path. **10 tests landed** (6 failure modes + 1 success + 1 is_configured-false defence + sanitiser + AuthFailureReason::as_str stable).

### Task 3: Wire web server in `main.rs` (AC: 4, 5)

- [x] Add `src/web/mod.rs` with `build_router`, `run`, `AppState`. **`AppState` deferred — not needed by Story 9-1's surface (only the auth state is shared); Stories 9-2+ can introduce it when they add data routes.**
- [x] Add `OpcGwError::Web(String)` variant in `src/utils.rs`.
- [x] Spawn `web_handle` in `src/main.rs` after the OPC UA spawn (only when `[web].enabled`).
- [x] Add 5th `tokio::select!` branch for graceful shutdown. **Implemented as a conditional `await` after the existing 4-handle `try_join!` rather than a 5th select-arm — the web handle is `Option<JoinHandle>` because it may not exist when `[web].enabled = false`. Operationally equivalent: the `CancellationToken` cancel still fans out to all five tasks; the difference is the join shape.**
- [x] Mount `tower_http::services::ServeDir` at `/` with the auth layer applied. **Mounted via `Router::fallback_service` so the auth middleware layer at the router level applies before the static-file dispatch.**
- [x] Mount `/api/health` returning `{"status":"ok"}` (smoke endpoint for tests).
- [x] Create `static/index.html`, `static/applications.html`, `static/devices.html`, `static/commands.html` placeholders with `<meta viewport>` + Story 9-X stub.
- [x] Add `event="web_server_started"` info event at startup (resolved port + bind + realm).

### Task 4: Integration tests (AC: 4, 5, 8)

- [x] Add `tests/web_auth.rs` — modeled on `tests/opcua_subscription_spike.rs` shape (`mod common;`, `init_test_subscriber`, `serial_test::serial`, `tracing-test` capture).
- [x] Add `build_http_client()` helper to `tests/common/mod.rs` returning a `reqwest::Client` configured for the test deployment shape.
- [x] 5+ integration tests: missing-auth-401, malformed-scheme-401, success-200, static-file-served, graceful-shutdown. **7 tests landed** (the 5 listed + wrong-password + web-defaults-stable).
- [x] All tests `#[serial_test::serial]` (shared global tracing subscriber).

### Task 5: Documentation (AC: 5, 8)

- [x] Add `## Web UI authentication` section to `docs/security.md` with: What it is / Configuration / What you'll see in the logs / Anti-patterns / Tuning checklist. Include the shared-credentials note + the TLS-deferred-to-reverse-proxy stance.
- [x] Register `event="web_auth_failed"` and `event="web_server_started"` in `docs/logging.md` operations reference. **Added a new "Audit and diagnostic events (`event=`)" subsection that catalogues all `event=` names introduced by Stories 7-2 onward, including the two new ones.**
- [x] Update `README.md` Configuration section with the new `[web]` block.
- [x] Sync `README.md` Planning table — Epic 9 row updated to `🔄 in-progress (9-1 review)`.
- [x] Add entry to `_bmad-output/implementation-artifacts/deferred-work.md`: "Story 9-1: User-manual chapter for web auth" + "Story 9-1: TLS / HTTPS hardening". **Plus per-IP rate limiting (#88) carry-forward note + CSRF-for-9-4+ note.**

### Task 6: Final verification (AC: 6, 7, 8)

- [x] `cargo test --lib --bins`: ≥ 320 passed / 0 failed. **Result: 326 passed / 0 failed / 3 ignored.**
- [x] `cargo test --tests`: all 14 prior integration test binaries pass + new `tests/web_auth.rs` passes. **Result: 0 failures across all 15 integration test binaries (14 prior + new web_auth).**
- [x] `cargo clippy --all-targets -- -D warnings`: clean.
- [x] `cargo test --doc`: 0 failed (56 ignored — issue #100 baseline, untouched).
- [x] `git diff --stat src/opc_ua_session_monitor.rs`: 0 changes.
- [x] `git diff src/opc_ua_auth.rs`: only the visibility / extraction changes (no logic changes). **Confirmed: removed local `hmac_sha256` (extracted to `src/security_hmac.rs`); added `pub fn hmac_key()`; removed 4 hmac_sha256 unit tests (moved to `src/security_hmac.rs::tests`); added `web: WebConfig::default()` to the test fixture.**
- [x] `git grep "event=\"web_" src/`: exactly 2 distinct values. **Confirmed: `web_auth_failed` (1 emit site, in `web::auth::emit_auth_failure_event`) and `web_server_started` (1 emit site, in `web::run`). The graceful-shutdown line and the disabled line are plain `info!` lines without an `event=` field.**

### Task 7: Documentation sync verification (CLAUDE.md compliance)

- [x] README.md updated with the new `[web]` config block + Planning row update.
- [x] docs/security.md `## Web UI authentication` section landed.
- [x] docs/logging.md operations reference updated with the two new events.
- [x] deferred-work.md updated with the 2 carry-forward entries (+ 2 extras: per-IP rate limiting and CSRF-for-9-4+).
- [x] sprint-status.yaml `last_updated` narrative reflects the Story 9-1 ship.

---

## Dev Notes

### Architecture compliance

- Axum **0.8.8** per `architecture.md:116` — verify the latest 0.8.x in `cargo search axum` at implementation time and use that version exactly. The async-opcua / tokio versions are unchanged from Phase A.
- The `src/web/` module directory is reserved in `architecture.md:417-421` (`mod.rs`, `api.rs`, `auth.rs`, `static_files.rs`). Story 9-1 creates `mod.rs` + `auth.rs` + (optionally) `static_files.rs`; `api.rs` is reserved for Stories 9-2 onwards.
- `tower-http` is the only new dep: `axum = "0.8"`, `tower-http = { version = "0.5", features = ["fs"] }` (or whatever current is — verify `cargo search`), `subtle = "2"` (likely already pulled by `hmac` crate; verify direct dep).
- `reqwest = "0.12"` as a `[dev-dependencies]` entry (test-only HTTP client).

### Library-wrap-not-fork pattern

Established 3-epic pattern (`OpcgwAuthManager`, `AtLimitAcceptLayer`, `OpcgwHistoryNodeManager`). Axum's middleware system is rich enough that no wrap is needed — `from_fn_with_state` handles the auth layer cleanly. The pattern is mentioned only because the Phase-B carry-forward (`epics.md:784`) noted it as the default for missing async-opcua callbacks; for Axum it doesn't apply.

### NFR12 source-IP — direct vs. correlated

Story 7-2 / 7-3 needed two-event correlation because async-opcua's `AuthManager` doesn't receive peer `SocketAddr`. **Axum has direct access** via `ConnectInfo<SocketAddr>`. The audit-event payload carries `source_ip` directly; no correlation step is needed for the web surface. This is a strict improvement over the OPC UA path. Document the asymmetry in `docs/security.md` so operators understand why the web log lines look different from the OPC UA log lines.

### HMAC keying reuse — the right shape

Per the Epic 8 retro and `epics.md:782`: reuse, don't roll new. The cleanest reuse is **share the same `hmac_key`** (one per process) between `OpcgwAuthManager` and `WebAuthState`. This means the OPC UA auth manager's startup path computes the digest first, then `WebAuthState::from_opcua_auth` borrows the key. If a future story needs a third auth surface (e.g., separate admin endpoint), it joins the same single `hmac_key` rather than each surface generating its own.

### Constant-time path on rejection

The 401 response constructor must return identical headers + body on every failure mode. Specifically:

- `Authorization: Basic dXNlcjp3cm9uZw==` (wrong password for "user") and `Authorization: Basic d3JvbmdfdXNlcjpwYXNz` (wrong user "wrong_user" with right password "pass") must produce **identical wire responses**. The audit event differentiates them via `reason=user_mismatch` vs `reason=password_mismatch`, but the wire response is identical (constant-time path).
- Implementation: build the 401 response *after* the constant-time digest comparison completes for both fields, regardless of which one mismatched.

### CSRF — what 9-4+ will need

Story 9-1 ships only `GET` routes plus `GET /api/health`. Stories 9-4 / 9-5 / 9-6 will introduce `POST` / `PUT` / `DELETE` for CRUD operations. CSRF protection is required for those — either:
- Strict same-origin policy enforcement via CORS (no `Access-Control-Allow-Origin: *`; only allow same-origin XHR), OR
- A double-submit cookie / synchronizer-token pattern.

Document this in `docs/security.md` under "Anti-patterns" so the author of 9-4 doesn't miss it.

### Per-IP rate limiting (#88) becomes structurally relevant

Once the web auth surface lands, a brute-force attacker can probe basic-auth credentials at HTTP rate (orders of magnitude faster than OPC UA session creation). The flat global session cap from Story 7-3 doesn't apply to web requests. Tracked at GitHub issue [#88](https://github.com/guycorbaz/opcgw/issues/88); explicitly out of scope for 9-1 per the LAN-internal threat model. Open a separate "Story 9-1 follow-up: web auth rate limiting" issue if a near-term operator concern surfaces.

### Carry-forward debt acknowledged but unchanged

- `tracing-test = "=0.2.6"` exact-pin from issue #101 — Story 9-1 inherits unchanged.
- `tests/common/mod.rs` from issue #102 — Story 9-1 adds `build_http_client()` helper as the third caller; no extraction of more helpers.
- 56 ignored doctests from issue #100 — Story 9-1 adds no new doctests; the baseline stays.
- NodeId format from issue #99 — irrelevant to Story 9-1 (no OPC UA address-space construction).

### File List (expected post-implementation)

- `Cargo.toml` (modified) — add `axum`, `tower-http`, `subtle` (verify direct), `reqwest` (dev-deps).
- `src/config.rs` (modified) — `WebConfig` struct + `AppConfig.web` field + validation.
- `src/utils.rs` (modified) — `WEB_DEFAULT_*` / `WEB_MIN_PORT` / `WEB_MAX_PORT` / `WEB_AUTH_REALM_MAX_LEN` constants + `OpcGwError::Web` variant.
- `src/opc_ua_auth.rs` (modified) — `pub fn hmac_key(&self) -> &[u8; 32]` accessor; possibly `pub` on `hmac_sha256` (Shape B) or extraction (Shape A).
- `src/security_hmac.rs` (NEW, optional per Shape A) — extracted `hmac_sha256` function.
- `src/web/mod.rs` (NEW) — `build_router`, `run`, `AppState`, ~80 LOC.
- `src/web/auth.rs` (NEW) — `WebAuthState` + `basic_auth_middleware` + `sanitise_user` reuse, ~150 LOC.
- `src/main.rs` (modified) — 5th `tokio::spawn` + 5th `tokio::select!` branch + `event="web_server_started"` info event.
- `src/lib.rs` (modified) — `pub mod web;` + `pub mod security_hmac;` (if Shape A).
- `static/index.html`, `static/applications.html`, `static/devices.html`, `static/commands.html` (NEW) — placeholders with `<meta viewport>`.
- `tests/common/mod.rs` (modified) — `build_http_client()` helper.
- `tests/web_auth.rs` (NEW) — ≥ 5 integration tests.
- `config/config.toml` (modified) — commented `[web]` block.
- `config/config.example.toml` (modified) — uncommented representative `[web]` block.
- `docs/security.md` (modified) — `## Web UI authentication` section.
- `docs/logging.md` (modified) — event registry update.
- `README.md` (modified) — Configuration section + Planning row.
- `_bmad-output/implementation-artifacts/deferred-work.md` (modified) — 2 new entries.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` (modified) — `last_updated` narrative + 9-1 status.
- This story file (modified) — Dev Agent Record / Completion Notes / File List filled in by the dev agent.

### Project Structure Notes

- Aligns with `architecture.md:417-421` reservation of `src/web/`.
- Sequencing per `epics.md` Phase-B polish: 9-1 is first; 9-2 + 9-3 can run in parallel after 9-1; 9-0 spike runs before 9-7 / 9-8.
- No conflicts with existing structure.

### References

- [Source: `_bmad-output/planning-artifacts/epics.md#Epic-8` (= sprint-status Epic 9), lines 766-784 — Phase-B carry-forward bullets, especially line 782 (HMAC keying reuse) + line 784 (library-wrap-not-fork)].
- [Source: `_bmad-output/planning-artifacts/epics.md#Story-8.1` (= sprint-status 9-1), lines 770-786 — BDD acceptance criteria].
- [Source: `_bmad-output/planning-artifacts/architecture.md:88-117` — dependency stack including `axum = 0.8.8`].
- [Source: `_bmad-output/planning-artifacts/architecture.md:217-225` — graceful shutdown via `CancellationToken`].
- [Source: `_bmad-output/planning-artifacts/architecture.md:388-472` — directory structure with `src/web/` reservation].
- [Source: `_bmad-output/planning-artifacts/prd.md#FR50`, line 422 — basic auth requirement].
- [Source: `_bmad-output/planning-artifacts/prd.md#NFR11`, line 441 — auth before any change].
- [Source: `_bmad-output/planning-artifacts/prd.md#NFR12`, line 442 — failed-auth source-IP logging].
- [Source: `_bmad-output/planning-artifacts/prd.md#FR41`, line 407 — mobile-responsive].
- [Source: `_bmad-output/implementation-artifacts/epic-7-retro-2026-04-29.md#Lessons-Learned`, item 4 — HMAC + constant-time-compare is the new default for authentication].
- [Source: `_bmad-output/implementation-artifacts/epic-8-retro-2026-05-01.md#Lessons-Learned`, item 2 — library-wrap pattern is project-shaping].
- [Source: `src/opc_ua_auth.rs:42-66` — `hmac_sha256` reference implementation; `:76-78` — `sanitise_user` reuse target].
- [Source: `src/main.rs:486, :654-704` — CancellationToken + tokio::spawn + tokio::select! pattern to replicate].
- [Source: GitHub issue #88 — per-IP rate limiting carry-forward, structurally relevant once Web UI lands].
- [Source: GitHub issue #100 — doctest cleanup, Story 9-1 inherits unchanged].
- [Source: GitHub issue #102 — tests/common/mod.rs extraction; Story 9-1 adds the third caller].

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — `claude-opus-4-7[1m]` — single-execution
`bmad-dev-story` run on 2026-05-02 from a fresh `/clear` state.

### Debug Log References

- Test runs documented inline in Task 6 above; no debug-log capture
  required outside the standard `cargo test` output.
- One mid-implementation regression iteration: the `password_mismatch`
  audit-event assertion in `test_wrong_password_returns_401_and_emits_audit_event_with_user`
  initially compared against `user="opcua-user"` (quoted), but the
  tracing layer renders `%`-formatted (Display) string fields without
  quotes — `user=opcua-user`. Fixed the assertion to match the actual
  format. Documented inline in the test file.

### Completion Notes List

- **AC#1 (config block) — COMPLETE.** `WebConfig` added to
  `src/config.rs` with hand-written `Debug` impl. All four knobs
  (`port`, `bind_address`, `auth_realm`, `enabled`) are
  `Option<...>` with `#[serde(default)]`. `WEB_DEFAULT_*` /
  `WEB_MIN_PORT` / `WEB_MAX_PORT` / `WEB_AUTH_REALM_MAX_LEN` /
  `WEB_DEFAULT_ENABLED` constants in `src/utils.rs` are the single
  source of truth. `AppConfig::validate` rejects every invalid input
  with a discriminating error message. Five validation unit tests +
  one env-var override integration-style test in `src/config.rs::tests`.
- **AC#2 (HMAC reuse + Basic auth middleware) — COMPLETE.** Shape A
  chosen: `hmac_sha256` extracted from `src/opc_ua_auth.rs` into a
  new `src/security_hmac.rs` module + the four primitive unit tests
  moved with it. `OpcgwAuthManager` adopts the shared primitive via
  `use crate::security_hmac::hmac_sha256` (no behavioural change).
  `OpcgwAuthManager::hmac_key()` accessor added per AC#7's allowance.
  `WebAuthState` exposes both `new(config, realm)` (Shape 1: fresh
  per-process key) and `from_opcua_auth(opcua, user, password, realm)`
  (Shape 2: shared key). **Production uses Shape 1** because
  `OpcgwAuthManager` is constructed inside `OpcUa::run` rather than
  `main.rs`, and AC#6 forbids modifying `src/opc_ua.rs` to surface it.
  `Shape 2` is kept as `pub` (with `#[allow(dead_code)]`) for symmetry
  with the spec and so a future story can refactor the construction
  order without re-introducing it. The middleware
  (`basic_auth_middleware`) uses `axum::middleware::from_fn_with_state`
  + `ConnectInfo<SocketAddr>` so peer IP is available natively (no
  two-event correlation pattern needed). Constant-time path verified:
  both digests are computed and `constant_time_eq`-compared
  unconditionally before the `&` combine; the 401 response is
  identical across all six failure modes (the `reason` field
  discriminates only in the audit log). Ten unit tests in
  `src/web/auth.rs::tests` cover all six failure modes + success +
  is_configured-false defence + sanitiser + `AuthFailureReason::as_str`
  stability.
- **AC#3 (NFR12 source-IP) — COMPLETE.** Audit event shape:
  `event="web_auth_failed" source_ip=<ip> user=<sanitised> path=<req-path> reason=<discriminator>`.
  `source_ip` is the peer IP (not the IP+port) extracted directly
  from `ConnectInfo<SocketAddr>`. The `user` field uses the
  `sanitise_user` helper duplicated in `src/web/auth.rs` (3 lines —
  per CLAUDE.md scope-discipline a tiny duplicate is preferable to
  widening `OpcgwAuthManager`'s public surface beyond AC#7's
  allowance). The `reason` discriminator has six stable values
  (`missing` / `malformed_scheme` / `malformed_base64` /
  `missing_colon` / `user_mismatch` / `password_mismatch`) pinned by
  `AuthFailureReason::as_str` and a dedicated unit test. Three
  integration tests in `tests/web_auth.rs` exercise the full audit
  pipeline (`source_ip=127.0.0.1` from a real reqwest client).
- **AC#4 (CancellationToken + Tokio runtime) — COMPLETE.** The web
  handle is spawned as the 5th `tokio::spawn` in `src/main.rs`
  conditionally on `[web].enabled = true`. Implementation note: a
  literal "5th `tokio::select!` branch" wasn't added — instead the
  web handle is `Option<JoinHandle>` joined after the existing 4-handle
  `tokio::try_join!` (cancellation still fans out to all five tasks
  via the shared `CancellationToken`; the difference is in the
  shutdown-join shape, not the cancellation semantics). The
  `test_graceful_shutdown_via_cancellation_token` integration test
  asserts the handle joins within 5s of `cancel()` and the bound
  port is released. `event="web_server_started"` info event fires
  exactly once at startup with resolved bind address + port + realm.
- **AC#5 (static files + FR41) — COMPLETE.** `tower_http::services::ServeDir`
  mounted via `Router::fallback_service("/", ServeDir::new(static_dir))`
  so the global auth layer applies before static-file dispatch (the
  standard nesting pattern for tower-http v0.6 — `nest_service` at the
  empty path conflicts with the layer state binding in axum 0.8).
  Four placeholder HTML files in `static/`
  (`index.html` / `applications.html` / `devices.html` / `commands.html`)
  each carry the `<meta name="viewport" content="width=device-width, initial-scale=1">`
  tag for FR41. Two integration tests pin the auth+static behaviour:
  unauth `GET /index.html` returns 401, auth'd `GET /index.html`
  returns 200 with the viewport meta in the body.
- **AC#6 (regression baseline) — COMPLETE.** Pre-Story baseline was
  309 lib+bins post-Epic-8 carry-forward. Post-Story:
  - `cargo test --lib --bins`: **326 passed** / 0 failed / 3 ignored.
    Δ = +17 from baseline (5 web-config validation + 1 web env-override +
    4 security_hmac + 10 web::auth unit + 1 build_router smoke + a
    few delta from existing fixture changes). Within the AC#8
    "≈14 ± minor variance" budget.
  - `cargo test --tests`: all 14 prior integration test binaries
    pass + new `tests/web_auth.rs` (7 web-specific + 3 reused from
    `tests/common/`) = **15 binaries / 0 failures**.
  - `cargo clippy --all-targets -- -D warnings`: **clean**.
  - `cargo test --doc`: **0 failed** (56 ignored — issue #100
    baseline, untouched).
- **AC#7 (NFR12 + auth + connection-cap carry-forward intact) — COMPLETE.**
  - `git diff --stat src/opc_ua_session_monitor.rs`: zero changes.
  - `git diff --stat src/opc_ua.rs`: zero changes.
  - `git diff --stat src/opc_ua_history.rs`: zero changes.
  - `git diff src/opc_ua_auth.rs`: only the allowed extraction +
    `pub fn hmac_key()` accessor + the `WebConfig::default()` test
    fixture import. **No logic changes.**
  - The Story 8-1/8-2/8-3 spike+history+subscription tests
    (`tests/opcua_subscription_spike.rs` 17 tests,
    `tests/opcua_history.rs` 11 tests) all pass unchanged.
- **AC#8 (sanity check) — COMPLETE.**
  - Default test count grew by **+17** lib+bins (within the spec's
    "≈14 ± minor variance" budget).
  - Exactly **two** new tracing-event names introduced:
    `web_auth_failed` (audit warn) and `web_server_started`
    (diagnostic info). `git grep 'event = "web_\|event="web_' src/`
    produces exactly those two values, both with a single emit
    site. The graceful-shutdown log line and the disabled log
    line are plain `info!` lines without an `event=` field — the
    spec's strict "exactly two" rule was honoured by trimming what
    would have been `web_server_stopped` and `web_server_disabled`
    down to plain log lines.
  - Both events registered in `docs/security.md` § "Web UI
    authentication" + `docs/logging.md` § "Audit and diagnostic
    events (`event=`)".
  - **Zero new audit events on the OPC UA path** (AC#7 invariant):
    `git diff src/opc_ua_auth.rs` shows no new `warn!` / `event=`
    sites; the existing `event="opcua_auth_failed"` and
    `event="opcua_auth_succeeded"` are unchanged.

#### Field-shape divergence from spec

- **Shape 1 vs Shape 2 in AC#2.** Spec marked `from_opcua_auth`
  (Shape 2: shared HMAC key) as "preferred". Implementation uses
  Shape 1 (fresh per-process key for the web surface) because
  `OpcgwAuthManager` is constructed inside `OpcUa::run` rather than
  `main.rs`, and AC#6 forbids modifying `src/opc_ua.rs` to surface
  it. Shape 2 is exported (with `#[allow(dead_code)]`) so a future
  story can refactor the construction order without re-introducing
  the function. **Functional impact: none** — both shapes give the
  same security properties (per-process random HMAC key, fixed-length
  digests, constant-time compare); the only difference is whether
  the OPC UA surface and the web surface share *one* key or have
  *two* independent keys. Two-key mode means an attacker that
  somehow extracts one key can't replay against the other surface,
  which is arguably *more* defensive — though Story 8-1's spike
  test for cross-instance key replay confirms the per-process
  randomness is the load-bearing property either way.
- **5th `tokio::select!` branch (AC#4).** Implementation joins the
  web handle as `Option<JoinHandle>` after the existing 4-handle
  `try_join!` rather than as a 5th select-arm. Functionally
  equivalent (same `CancellationToken` fan-out, same
  graceful-shutdown semantics); the `Option` shape correctly
  models "this handle may not exist when `[web].enabled = false`".
- **Spec's "exactly 2 events" vs initial 4-event implementation.**
  First implementation pass added `web_server_stopped` and
  `web_server_disabled` as additional `event=` names for
  symmetric lifecycle visibility. AC#8's grep contract called this
  out as a violation; trimmed to plain `info!` lines for those two
  cases. Net result: exactly 2 structured event names, with the
  lifecycle visibility preserved as plain log lines.
- **`subtle = "2"` direct dep (AC#2).** Spec wording prescribed
  `subtle::ConstantTimeEq::ct_eq`. Implementation uses
  `constant_time_eq::constant_time_eq` (the existing direct dep
  Story 7-2 added) for consistency with the OPC UA auth path. Both
  crates are vetted constant-time-comparison primitives;
  functionally identical.
- **`tower-http = "0.5"` vs `0.6` (Dev Notes).** Spec referenced
  `tower-http = "0.5"`; latest stable is `0.6.8` and is what
  `axum = "0.8"` expects. Used `0.6` to avoid version-skew warnings.
- **`AppState` deferred to Stories 9-2+ (Task 3).** Spec's `AppState`
  was meant to hold `Arc<dyn StorageBackend>` for future stories.
  Story 9-1 doesn't read from storage — only the auth state is
  needed — so `AppState` was not introduced. Stories 9-2+ can add it
  cleanly when they introduce data routes; the change is local to
  `src/web/mod.rs::build_router`.

### File List

**New files:**
- `src/security_hmac.rs` — extracted `hmac_sha256` primitive + 4 unit tests.
- `src/web/mod.rs` — `build_router`, `run`, `api_health` smoke endpoint, `build_router_smoke` test.
- `src/web/auth.rs` — `WebAuthState`, `basic_auth_middleware`, `AuthFailureReason`, helpers, 10 unit tests.
- `static/index.html`, `static/applications.html`, `static/devices.html`, `static/commands.html` — placeholder HTML with `<meta viewport>` for FR41.
- `tests/web_auth.rs` — 7 integration tests + helpers.

**Modified files:**
- `Cargo.toml` — added `axum = "0.8"`, `tower-http = "0.6"` (with `fs` feature), `base64 = "0.22"` as direct deps; added `reqwest = "0.12"` and `tower = "0.5"` as dev-deps.
- `src/utils.rs` — added `WEB_DEFAULT_PORT` / `WEB_MIN_PORT` / `WEB_MAX_PORT` / `WEB_DEFAULT_BIND_ADDRESS` / `WEB_DEFAULT_AUTH_REALM` / `WEB_AUTH_REALM_MAX_LEN` / `WEB_DEFAULT_ENABLED` constants; added `OpcGwError::Web(String)` variant.
- `src/config.rs` — added `WebConfig` struct (with hand-written `Debug` impl); added `web: WebConfig` field to `AppConfig`; added validation entries for the four `[web]` knobs in `AppConfig::validate`; added 6 unit tests.
- `src/opc_ua_auth.rs` — removed local `hmac_sha256` + 4 unit tests (moved to `src/security_hmac.rs`); added `pub fn hmac_key(&self) -> &[u8; 32]` accessor; updated test fixture to include `web: WebConfig::default()`.
- `src/lib.rs` — added `pub mod security_hmac;` and `pub mod web;`.
- `src/main.rs` — added `mod security_hmac;` and `mod web;`; spawned the conditional 5th task for the embedded web server; added `Option<JoinHandle>`-based shutdown join.
- `tests/common/mod.rs` — added `build_http_client(timeout)` helper for the web integration tests.
- `tests/opcua_subscription_spike.rs`, `tests/opc_ua_connection_limit.rs`, `tests/opc_ua_security_endpoints.rs`, `tests/opcua_history.rs` — added `WebConfig` to the `opcgw::config::*` import + `web: WebConfig::default()` to the test fixtures (test-fixture-only changes, no production code modified per AC#6).
- `config/config.toml` — added commented-out `[web]` block with documentation.
- `config/config.example.toml` — added enabled `[web]` block with documentation (representative non-default deployment shape).
- `docs/security.md` — appended `## Web UI authentication` section + reference list update.
- `docs/logging.md` — added `### Audit and diagnostic events (event=)` subsection cataloguing all `event=` names from Stories 7-2 onward, including the two new ones.
- `README.md` — added Web UI subsection in Configuration; updated Planning row for Epic 9; bumped `last_updated` to 2026-05-02.
- `_bmad-output/implementation-artifacts/deferred-work.md` — added 4 entries (user-manual chapter, TLS hardening, per-IP rate limiting, CSRF for 9-4+).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — flipped `9-1-axum-web-server-and-basic-authentication: ready-for-dev → in-progress → review`; refreshed `last_updated` narrative.
- `_bmad-output/implementation-artifacts/9-1-axum-web-server-and-basic-authentication.md` — this file: status flipped to `review`, all task checkboxes filled, Dev Agent Record + completion notes + file list populated.

### Change Log

| Date | Change | Detail |
|------|--------|--------|
| 2026-05-02 | Status flipped `ready-for-dev → in-progress → review` | Single-execution `bmad-dev-story` run from `/clear` state. All 8 ACs satisfied; loop terminates on first pass. |

# Story 9.2: Gateway Status Dashboard

**Epic:** 9 (Web Configuration & Hot-Reload — Phase B)
**Phase:** Phase B
**Status:** done
**Created:** 2026-05-03
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md` was authored before Phase A was renumbered. The story this file implements lives in `epics.md` as **"Story 8.2: Gateway Status Dashboard"** under **"Epic 8: Web Configuration & Hot-Reload (Phase B)"** (lines 816–831). In `sprint-status.yaml` and the rest of the project this is **Story 9-2** under **Epic 9**. Same work, different numbering. The Phase-B carry-forward bullets at `epics.md:773–796` apply to this story; the library-wrap-not-fork rule (`epics.md:796`) is informational here (Axum is not async-opcua).

---

## User Story

As an **operator**,
I want to see gateway status at a glance in a web browser,
So that I can check health from my phone while in the field (FR38, FR41).

---

## Objective

Replace the Story 9-1 placeholder `static/index.html` with a real **gateway status dashboard** and add the **JSON API endpoint** the dashboard reads from. The dashboard answers five operator questions in one screen:

1. Is ChirpStack reachable right now? (boolean — green/red badge)
2. When did the last successful poll complete? (relative + absolute timestamp)
3. How many errors has the gateway accumulated since startup? (cumulative i32)
4. How many applications are configured?
5. How many devices are configured (across all applications)?

**Data flow:**

```
SQLite gateway_status table  ─┐
                              ├─►  GET /api/status (JSON)  ─►  static/index.html (vanilla JS fetch)
AppConfig.application_list  ──┘
```

The dashboard is **server-rendered placeholders + client-side fetch**: the static HTML ships with empty value cells, and a small `<script>` block fetches `/api/status` on load + every 10 s thereafter. No SPA framework, no build step, no `npm install`. Mobile-responsive via plain CSS (single-column layout below 600 px viewport, two-column above) — FR41 satisfied at the dashboard level (Story 9-1 only satisfied it at the *server* level, by serving the `<meta viewport>` tag).

The new code surface is **modest** — estimated **~150–250 LOC of production code (Rust + HTML + CSS + JS) + ~200–300 LOC of tests + ~50–80 LOC of docs**. The Rust side is small (one new endpoint, one new `AppState` struct that finally lands the per-Story 9-1 deferred shape, one new `SqliteBackend` per the Story 4-1 / 5-1 / 8-3 per-task pattern); the static-asset side is the real surface (one HTML file, one CSS file, one JS file, all small).

This story closes **FR38** (gateway status via web interface), advances **FR41** (mobile-responsive: dashboard ships responsive CSS), and consumes the Story 9-1 web server + auth middleware infrastructure unchanged. It does **not** ship configuration-mutation paths (Stories 9-4 / 9-5 / 9-6 own those) and does **not** ship live metric values (Story 9-3 owns the per-device metric grid).

---

## Out of Scope

- **Live metric values per device.** Story 9-3 owns the per-device metric value grid (`static/devices.html` body + `/api/devices` endpoint). Story 9-2 only ships the *gateway-level* health summary; per-device data is a separate story so the dashboard stays small + the JSON contract for `/api/status` stays narrow.
- **Application / device / command CRUD.** Stories 9-4 / 9-5 / 9-6 own `POST` / `PUT` / `DELETE` for configuration mutation. Story 9-2 ships **read-only `GET` endpoints only** — no CSRF surface introduced.
- **Server-Sent Events / WebSocket push.** The dashboard polls `/api/status` every 10 s via `setInterval`. SSE/WebSocket would be cleaner but adds Axum-side state machinery (broadcast channels, per-connection lifecycle) that's overkill for one number. Revisit if the 10 s refresh becomes operator-visible latency. Tracked as a dashboard-UX follow-up.
- **Historical chart of error count / poll cadence.** Story 9-2 shows the *current* values. A time-series chart (last 24 h of poll cadence + error rate) belongs in a future "operations history" story or in an external Grafana/Prometheus deployment. Out of scope; tracked as a future enhancement.
- **Auth-state-per-route differentiation.** All routes — `GET /`, `GET /api/health`, `GET /api/status`, `GET /<static>` — require Basic auth via the Story 9-1 middleware. No anonymous / read-only role split. Single-user model preserved.
- **TLS / HTTPS.** Inherited from Story 9-1: HTTP-only, reverse-proxy TLS termination is the documented stance. Tracked at GitHub issue [#104](https://github.com/guycorbaz/opcgw/issues/104).
- **Per-IP rate limiting on `/api/status`.** Inherited from Story 9-1: tracked at GitHub issue [#88](https://github.com/guycorbaz/opcgw/issues/88). Not a Story 9-2 concern; the dashboard's poll cadence is operator-controlled (one browser tab = 6 req/min).
- **Hot-reload of dashboard CSS / HTML.** The static files are read by `tower_http::services::ServeDir` from disk on every request; no caching layer, no template compilation. An operator who edits `static/index.html` while the gateway is running sees the new content on the next request — no opt-in needed. The Story 9-7 "configuration hot-reload" story covers TOML / SQLite mutation; static-file hot-reload is already free.
- **Internationalisation / locale switching.** Dashboard ships English-only. Date/time format uses RFC 3339 server-side + `Intl.DateTimeFormat` client-side (browser-locale-aware). No translation framework.
- **Dark mode toggle.** Plain CSS using `prefers-color-scheme: dark` media query — no in-page toggle, no preference persistence. Browser/OS preference wins.
- **Per-request access log.** Inherited deferral from Story 9-1 (deferred-work.md:226). Story 9-2 does not add `tower-http::trace::TraceLayer`.
- **Drop-zeroize HMAC keys.** Inherited deferral from Story 9-1 (deferred-work.md:225). Story 9-2 must not add the `zeroize` crate or modify `src/web/auth.rs::WebAuthState` beyond what the new `AppState` shape requires.

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. The story's job is to **plumb a new endpoint + a new HTML view on top of code that already does the heavy lifting** — the web server, auth middleware, storage trait, gateway_status table, per-task SQLite backend pattern, redacting `Debug` impls, integration-test harness, all exist.

| What | Where | Status |
|------|-------|--------|
| **Embedded Axum web server + Basic auth middleware** | `src/web/mod.rs:111-119` (`build_router`), `src/web/auth.rs::basic_auth_middleware` | **Wired today (Story 9-1).** Story 9-2 adds a new `Router::route("/api/status", get(api_status))` call inside `build_router`. The auth middleware wraps every route via `.layer(...)` after the routes are registered; the new `/api/status` route inherits the layer automatically (load-bearing per Story 9-1 AC#5 — verified by the layer-after-route invariant comment at `src/web/mod.rs:72-83`). |
| **`get_gateway_health_metrics()` on `StorageBackend`** | `src/storage/mod.rs:727`, impl `src/storage/sqlite.rs:1968-1995` | **Wired today (Story 5-3).** Returns `Result<(Option<DateTime<Utc>>, i32, bool), OpcGwError>` — exactly the three values the dashboard needs (`last_poll_time`, `error_count`, `chirpstack_available`). **Story 9-2's `/api/status` handler reads via this method directly** — no new storage method is added. |
| **`gateway_status` SQLite table** | `migrations/v005_gateway_status.sql` + `migrations/v006_gateway_status_health_metrics.sql` (referenced from `src/storage/schema.rs:21-22`) | **Wired today (Story 5-3).** Single-row table (id=1) holding `last_poll_timestamp`, `error_count`, `chirpstack_available`. Updated by the ChirpStack poller after every poll cycle via `update_gateway_status`. **Story 9-2 reads from this table via the `StorageBackend` trait — never directly via `rusqlite`.** |
| **Per-task SQLite backend pattern** | `src/main.rs:614-641` (poller, OPC UA), `:673,690` (command status, command timeout) | **Established (Stories 4-1, 5-1, 8-3).** Each async task owns its own `SqliteBackend` constructed via `SqliteBackend::with_pool(pool.clone())`. **Story 9-2 adds a 5th call site for the web server** inside the existing `if web_enabled { ... }` block in `src/main.rs:721+`. The pool is already `Arc`-cloneable; no shared lock issues. |
| **`SqliteBackend::with_pool`** | `src/storage/sqlite.rs` (constructor) | **Wired today.** Constructs a `SqliteBackend` that takes a connection from the shared `ConnectionPool` per request. Same pattern as the OPC UA / poller / command paths. |
| **`AppConfig.application_list`** | `src/config.rs:783` (`Vec<ChirpStackApplications>`) | **Wired today.** Each `ChirpStackApplications` (defined `:465-484`) carries `application_name`, `application_id`, and `device_list: Vec<ChirpstackDevice>`. The dashboard's `application_count` = `application_list.len()`; `device_count` = `application_list.iter().map(|a| a.device_list.len()).sum::<usize>()`. **Story 9-2 reads these from a snapshot held in `AppState`** — see AC#1. The counts are **configured** counts (not live ChirpStack-discovered) — that's the correct semantic for the dashboard ("how many devices is the gateway *trying to* poll"). |
| **`AppState` (deferred from Story 9-1, Field-shape divergence #6)** | None today; Story 9-1 explicitly deferred it (`9-1-...md:738-743`) | **Lands in Story 9-2.** The 9-1 spec asked for `AppState { auth: Arc<WebAuthState>, backend: Arc<dyn StorageBackend> }`, but 9-1 didn't need the `backend` field so the struct was skipped. **Story 9-2 introduces it** with three fields: `auth: Arc<WebAuthState>`, `backend: Arc<dyn StorageBackend>`, `dashboard_snapshot: Arc<DashboardConfigSnapshot>`. The `dashboard_snapshot` is a frozen-at-startup capture of `(application_count, device_count, application_summaries)` so the dashboard doesn't lock the live config on every request (Story 9-7 hot-reload will refresh it via `tokio::sync::watch`; Story 9-2 ships the read-side only). |
| **Story 9-1 web server bind + spawn shape** | `src/main.rs:721-782` | **Wired today.** Story 9-2 modifies the `auth_state` line (renamed to `app_state`) and the `build_router` call signature; everything else (bind, spawn, graceful shutdown, `event="web_server_started"`) stays identical. The function signature of `build_router` changes from `(Arc<WebAuthState>, PathBuf)` to `(Arc<AppState>, PathBuf)` — one line in `main.rs`, one line in `web::build_router`, plus the body refactor inside `build_router` to extract `app_state.auth` for the auth-middleware `from_fn_with_state` call. |
| **`tower-http::services::ServeDir`** | `src/web/mod.rs:114` (mount) | **Wired today.** Story 9-2 adds new files to `static/` (CSS + JS) which `ServeDir` picks up automatically. No router changes needed for the new static assets — `ServeDir` serves any file under `static/` that the URL path matches. The `<script src="/dashboard.js">` and `<link rel="stylesheet" href="/dashboard.css">` references in the new `static/index.html` resolve through the existing fallback service. |
| **`reqwest` integration-test client** | `tests/common/mod.rs:155-162` (`build_http_client`) | **Wired today (Story 9-1).** Story 9-2's new `tests/web_dashboard.rs` integration tests reuse this helper unchanged. **Do NOT add a new HTTP-client constructor.** |
| **Tracing event-name convention + grep contract** | Stories 6-1 → 9-1 (`event="..."` field on every audit/diagnostic event) | **Established.** Story 9-1's grep contract `git grep 'event="web_'` returns exactly 2 values. **Story 9-2 introduces ZERO new `event=` names** (success path on `/api/status` is a routine GET — no audit-event needed; the `web_auth_failed` event from Story 9-1 still fires for unauth'd dashboard requests, which is the correct behaviour). The Story 9-2 grep invariant: `git grep 'event="web_' src/` continues to return exactly the same 2 values — `web_auth_failed` and `web_server_started`. |
| **`OpcGwError::Web` variant** | `src/utils.rs:618-625` | **Wired today (Story 9-1).** Story 9-2 reuses for any `/api/status` runtime errors: `Err(OpcGwError::Storage(...))` propagation from `get_gateway_health_metrics()` is wrapped or re-emitted as needed. The handler returns `Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)>` — convert `OpcGwError` into `(StatusCode::INTERNAL_SERVER_ERROR, Json(...))` at the handler boundary. |
| **Documentation extension target** | `docs/security.md`, `docs/logging.md` | **Existing files.** Story 9-2 adds a new section to `docs/security.md` § "Web UI authentication" — a sub-bullet documenting that `/api/status` is auth-gated (no anonymous probe); no new event registry entries in `docs/logging.md` (zero new events). |
| **`config/config.toml` + `config/config.example.toml`** | `config/config.toml`, `config/config.example.toml` | **No new config knob.** Story 9-2 reuses the Story 9-1 `[web]` block unchanged. The dashboard refresh interval is hard-coded to 10 s in JS (operator-tunable by editing `static/dashboard.js` — that's the right granularity for a static-asset dashboard). If multiple operators surface a "make refresh interval configurable" request, a future story can add `[web].dashboard_refresh_secs`. |
| **`README.md` Planning section** | `README.md` | **Wired today.** Story 9-2 flips the Story 9-2 row from `backlog` → `ready-for-dev` → `in-progress` → `review` → `done` in the Planning table, mirroring sprint-status.yaml. Story 9-2 also updates the README's Configuration section (no new config) and the "What's new" / Web UI subsection introduced by Story 9-1 to mention the dashboard. |
| **Manual chapter for the embedded web UI** | `docs/manual/opcgw-user-manual.xml` | **Lagging — deferred from Story 9-1 (deferred-work.md:218).** Story 9-2 inherits the deferral; the dashboard surface adds to the deferred manual chapter rather than blocking on it. |
| **Library-wrap-not-fork pattern** | `OpcgwAuthManager`, `AtLimitAcceptLayer`, `OpcgwHistoryNodeManager` | **Established but not applicable here.** Axum's middleware system is rich enough that no wrap is needed for a `GET /api/status` handler. No async-opcua callbacks involved. Mentioned only because the Phase-B carry-forward (`epics.md:796`) lists it as the default for missing async-opcua callbacks; Story 9-2 has none. |

**Epic-spec coverage map** — the BDD acceptance criteria from `epics.md` (lines 816–831) break down as:

| Epic-spec criterion (line ref) | Already known? | Where this story addresses it |
|---|---|---|
| Authenticated web server from Story 8.1 (= Story 9-1) (line 824) | ✅ wired today | **AC#3** — auth middleware from 9-1 wraps `/api/status` automatically. |
| ChirpStack connection state visible (available/unavailable) (line 826) | ⚠️ data exists; no UI | **AC#2 + AC#4** — `chirpstack_available: bool` field in `/api/status` JSON; rendered as a green "Available" / red "Unavailable" badge in `static/index.html`. |
| Last successful poll timestamp visible (line 827) | ⚠️ data exists; no UI | **AC#2 + AC#4** — `last_poll_time: Option<DateTime<Utc>>` (RFC 3339 string in JSON, `null` when never polled); rendered as both relative ("3 seconds ago") and absolute (RFC 3339 in browser locale). |
| Cumulative error count visible (line 828) | ⚠️ data exists; no UI | **AC#2 + AC#4** — `error_count: i32` field; rendered as a number with thousands separator. |
| Total device + application counts visible (line 829) | ⚠️ data in config; no aggregator | **AC#1 + AC#2 + AC#4** — `DashboardConfigSnapshot` aggregates at startup; surfaced as `application_count: usize` / `device_count: usize` in `/api/status` JSON; rendered as two number tiles. |
| Status data read from `gateway_status` table via web server's own connection (line 830) | ⚠️ pattern exists, not yet applied to web server | **AC#1** — new `SqliteBackend::with_pool(pool.clone())` call site in `main.rs::if web_enabled { ... }` block, mirroring the Story 4-1 / 5-1 / 8-3 per-task pattern. |
| FR38 satisfied (gateway status via web interface) (line 831) | ❌ Phase A had OPC UA gateway health (FR18) but no web-UI surface | **All ACs** collectively satisfy FR38. |
| Web UI mobile-responsive (FR41, line 786 carry-forward from Story 8.1) | ⚠️ server-side `<meta viewport>` shipped in 9-1; content was placeholder | **AC#4** — single-column ≤ 600 px / two-column > 600 px CSS; tested in Chrome DevTools mobile-emulation manual smoke + an automated assertion that the CSS contains the `@media` query (compile-time-equivalent pin). |
| `cargo test` clean + `cargo clippy --all-targets -- -D warnings` clean | Implicit per CLAUDE.md | **AC#5** — Story 9-2 baseline 333 lib+bins / clippy clean (post-Story 9-1); Story 9-2 target ≥ 343 with new dashboard tests. |
| Library-wrap-not-fork pattern (`epics.md:796`) | n/a — not async-opcua | **No work.** |

---

## Acceptance Criteria

### AC#1: `AppState` introduced; web server gets its own `SqliteBackend` + frozen config snapshot

**Implementation:**

- New `pub struct AppState` in `src/web/mod.rs`:
  ```rust
  pub struct AppState {
      pub auth: Arc<WebAuthState>,
      pub backend: Arc<dyn StorageBackend>,
      pub dashboard_snapshot: Arc<DashboardConfigSnapshot>,
  }
  ```
- New `pub struct DashboardConfigSnapshot` in `src/web/mod.rs` (or `src/web/state.rs` if the dev agent prefers a separate file — either is acceptable; `mod.rs` keeps the module tree flat):
  ```rust
  pub struct DashboardConfigSnapshot {
      pub application_count: usize,
      pub device_count: usize,
      // Future stories (9-3 live metrics, 9-4 application CRUD) will
      // need per-application/per-device summaries; ship them now so
      // 9-3's PR doesn't need to re-walk this struct.
      pub applications: Vec<ApplicationSummary>,
  }
  pub struct ApplicationSummary {
      pub application_id: String,
      pub application_name: String,
      pub device_count: usize,
  }
  ```
- New constructor `DashboardConfigSnapshot::from_config(config: &AppConfig) -> Self` walks `config.application_list` once and builds the summary. **Pure function; no I/O; no allocations beyond the obvious clones.**
- `build_router` signature changes from `(Arc<WebAuthState>, PathBuf)` to `(Arc<AppState>, PathBuf)`. Inside, the auth-middleware call extracts `state.auth.clone()` for `from_fn_with_state(...)` (the middleware signature in `src/web/auth.rs::basic_auth_middleware` does NOT change — it still takes `State<Arc<WebAuthState>>`, not `State<Arc<AppState>>`, because Axum allows nested state extraction via per-route `with_state` if needed; for now the auth state is extracted at router-construction time and threaded into the middleware closure).
- `src/main.rs` changes (in the `if web_enabled { ... }` block at `:721+`):
  - Add a new `let web_backend = Arc::new(SqliteBackend::with_pool(pool.clone())?)` line (mirroring the patterns at `:614-641`, `:673`, `:690`). Map the `Result` failure case to an `error!` + `return Err(e.into())` per the Story 9-1 fail-closed pattern.
  - Build a `DashboardConfigSnapshot::from_config(&application_config)` once (frozen-at-startup; Story 9-7 will swap in a `tokio::sync::watch::Receiver` for hot-reload).
  - Construct `let app_state = Arc::new(AppState { auth: auth_state, backend: web_backend, dashboard_snapshot: Arc::new(snapshot) })`.
  - Pass `app_state` (instead of `auth_state`) to `web::build_router`.

**Verification:**

- 1 unit test `DashboardConfigSnapshot::from_config_walks_application_list_once` that constructs an `AppConfig` with 2 applications × 3 devices each and asserts `application_count == 2`, `device_count == 6`, `applications.len() == 2`, each `ApplicationSummary.device_count == 3`.
- 1 unit test `DashboardConfigSnapshot::from_config_handles_empty_application_list` that passes an empty `Vec` and asserts `application_count == 0`, `device_count == 0`, `applications.is_empty()`.
- 1 unit test `DashboardConfigSnapshot::from_config_handles_application_with_zero_devices` — pins the `device_count` summation behaviour for an application with `device_list: vec![]`.
- 1 smoke test `build_router_smoke` (extend the Story 9-1 test at `src/web/mod.rs:274-282`) that constructs an `AppState` with a stub `InMemoryBackend` and verifies the `Router` type-checks.
- `cargo clippy --all-targets -- -D warnings` clean.

---

### AC#2: `GET /api/status` JSON endpoint returns the 5 dashboard fields

**Endpoint contract** (versioned implicitly — no `/v1/` prefix; if a v2 surface ever lands, it's `/api/status` v2 vs `/api/v2/status`, and the change is operator-visible. Same convention as `/api/health` from Story 9-1):

```http
GET /api/status HTTP/1.1
Authorization: Basic <base64(user:pass)>

HTTP/1.1 200 OK
Content-Type: application/json

{
  "chirpstack_available": true,
  "last_poll_time": "2026-05-03T09:14:22.001Z",
  "error_count": 0,
  "application_count": 2,
  "device_count": 6,
  "uptime_secs": 3742
}
```

**Field semantics:**

| Field | Type | Source | Semantics |
|---|---|---|---|
| `chirpstack_available` | `bool` | `get_gateway_health_metrics().2` | `true` if the most recent poll succeeded; `false` if ChirpStack was unreachable during the most recent poll cycle (or if no poll has happened yet — first-startup default per `src/storage/mod.rs:721-724`). |
| `last_poll_time` | `Option<DateTime<Utc>>` → JSON `string \| null` | `get_gateway_health_metrics().0` | RFC 3339 string with millisecond precision. `null` if no poll has succeeded yet (first startup, or chronic ChirpStack outage from boot). |
| `error_count` | `i32` | `get_gateway_health_metrics().1` | Cumulative count since gateway startup; never resets. Per `src/storage/types.rs:241-242`, wraps at `i32::MAX` (acknowledged but not addressable in 9-2). |
| `application_count` | `usize` → JSON `number` | `app_state.dashboard_snapshot.application_count` | Configured-application count from `application_list.len()`. Frozen-at-startup; Story 9-7 will refresh on hot-reload. |
| `device_count` | `usize` → JSON `number` | `app_state.dashboard_snapshot.device_count` | Sum of `device_list.len()` across all applications. Same frozen-at-startup semantics. |
| `uptime_secs` | `u64` → JSON `number` | `Instant::now().duration_since(start_time)` | Process uptime in whole seconds. Simple, correct, no time-zone gotchas. The `start_time` is captured at server-spawn time and stored in `AppState` (or in a `OnceLock<Instant>` in `src/web/mod.rs` — either is acceptable). |

**Implementation:**

- New `src/web/api.rs` module (or extend `src/web/mod.rs` directly — `api.rs` keeps the route handlers in one file as the surface grows in Stories 9-3 / 9-4 / 9-5 / 9-6; **prefer the new file**). New module declared in `src/web/mod.rs` as `pub mod api;`.
- `pub async fn api_status(State(state): State<Arc<AppState>>) -> Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)>`:
  1. Call `state.backend.get_gateway_health_metrics()`.
  2. On `Ok((last_poll, error_count, available))`: build `StatusResponse { chirpstack_available, last_poll_time, error_count, application_count, device_count, uptime_secs }` and return `Ok(Json(...))`.
  3. On `Err(e)`: log `warn!(error = %e, event = "api_status_storage_error", "GET /api/status: failed to read gateway_status table")` and return `Err((StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorResponse { error: "internal server error".to_string() })))`.
- `serde::Serialize`-derived `StatusResponse` and `ErrorResponse` structs in `src/web/api.rs`. `chrono::serde::ts_seconds_option` or `serde_with::With::<chrono::serde::ts_milliseconds_option>` for the `Option<DateTime<Utc>>` field — **prefer the simpler `Option<String>` shape that calls `.to_rfc3339()` at serialise time** to avoid pulling new serde adapter crates. The dev agent picks; document the choice in completion notes.
- The error response body is intentionally generic (`"internal server error"`) — operators see the underlying cause in the gateway log, not in the HTTP response body. **Do NOT leak `e.to_string()` to the client** (NFR7-aligned: error messages must not leak sensitive internal state, e.g. SQLite paths, table names, or session identifiers).
- New `event="api_status_storage_error"` IS added to the `event=` registry — that's a third audit-style event in the web subsystem. **AC#7 below (sanity check) updates the count from "exactly 2" to "exactly 3"** and adds the new event to `docs/logging.md`. The handler should NOT introduce success-path events (no `event="api_status_returned"` or similar) — successful GETs are routine and don't warrant audit logging.
- `Cargo.toml` may need `serde_json = "1"` as a direct dep if it's currently transitive only. Verify; add if needed.

**Verification:**

- 4 unit tests in `src/web/api.rs::tests` (using a mocked `Arc<dyn StorageBackend>` constructed from `InMemoryBackend`):
  1. `api_status_returns_200_with_all_fields_when_storage_healthy`
  2. `api_status_returns_500_with_generic_body_when_storage_errors` (use a `failing_backend` mock that returns `Err(OpcGwError::Storage(...))` and assert the body does NOT contain the inner error string)
  3. `api_status_returns_chirpstack_unavailable_first_startup` (storage returns `(None, 0, false)` — the default)
  4. `api_status_serialises_last_poll_time_as_null_when_none`
- 2 integration tests in `tests/web_dashboard.rs` (a new file):
  1. `auth_required_for_api_status` — unauth'd GET returns 401 + emits `event="web_auth_failed" path=/api/status reason=missing` (proves the auth middleware wraps the new route).
  2. `api_status_returns_json_with_expected_shape_when_authed` — auth'd GET returns 200, body parses as JSON, contains all 6 expected fields with correct types.
- Smoke test (manual): `OPCGW_WEB__ENABLED=true cargo run` then `curl -u opcua-user:test-password http://localhost:8080/api/status` returns the JSON above.

---

### AC#3: Auth middleware applies to `/api/status` (carry-forward from Story 9-1, no regression)

**Implementation:**

- The Story 9-1 layer-after-route invariant (`src/web/mod.rs:72-83`) is the load-bearing property — `.layer(...)` runs AFTER `.route(...)` AND `.fallback_service(...)`, so any new `Router::route("/api/status", get(api_status))` call inside `build_router` automatically inherits the auth middleware.
- **No behaviour change to `src/web/auth.rs`** — the middleware signature, the `WebAuthState` struct, and the `event="web_auth_failed"` audit event all stay identical.

**Verification:**

- The `auth_required_for_api_status` integration test from AC#2 doubles as the AC#3 verification.
- Existing Story 9-1 tests (`tests/web_auth.rs`) continue to pass unchanged — no regression on auth-fail modes, on `event="web_auth_failed"` audit-event shape, or on the constant-time path.
- `git diff src/web/auth.rs` shows zero changes (test fixture changes via the new `AppState` shape are acceptable; production-code changes are not).

---

### AC#4: `static/index.html` renders the dashboard; mobile-responsive (FR41)

**Implementation:**

- Replace `static/index.html` (currently a 12-line placeholder with `Story 9-2 will fill this in`) with a real dashboard. Keep the file plain HTML5 — no SPA framework, no build step, no `npm install`. Inline `<style>` and `<script>` are acceptable for the dashboard-specific bits, **but** prefer separate `static/dashboard.css` and `static/dashboard.js` files (served by the existing `ServeDir` mount) so the HTML is small and the assets are individually cacheable.
- HTML structure:
  - `<header>` with the gateway name (hard-coded "opcgw" — no need to fetch from `/api/status` since application name is operator-deployment-specific and the dashboard is gateway-level).
  - `<main>` with 5 status tiles in a CSS Grid:
    1. **ChirpStack** — green "Available" badge or red "Unavailable" badge.
    2. **Last poll** — relative time ("3 seconds ago") + absolute time (RFC 3339 in browser locale, in a `<time datetime="...">` element).
    3. **Errors** — number with thousands separator (`Intl.NumberFormat`).
    4. **Applications** — number tile.
    5. **Devices** — number tile.
  - Optional 6th tile: **Uptime** — formatted as `Hh Mm Ss` or `Dd Hh` for longer runtimes (operator-friendly, computed in JS from the `uptime_secs` field).
  - `<footer>` with the `last_poll_time` and a "Refresh now" button (calls the same fetch as the 10 s `setInterval`).
- CSS:
  - Default single-column layout for narrow viewports (`max-width: 600px`).
  - Two-column grid for `min-width: 601px`.
  - `prefers-color-scheme: dark` media query for dark mode.
  - System-stack font: `system-ui, -apple-system, sans-serif`. No webfonts — no external HTTP dependency, no FOIT/FOUT.
- JS:
  - On `DOMContentLoaded`: call `fetch('/api/status')` and render the tiles.
  - `setInterval(fetchStatus, 10_000)` for the refresh loop.
  - On HTTP 401: redirect to a `/login.html` page **OR** display an inline "Session expired — please reload" message. **Prefer the inline message** — Basic auth credentials are browser-cached per realm; a 401 from `/api/status` is unusual (operator deleted the credentials between page load + first poll) and a reload is the simplest recovery.
  - On HTTP 5xx or network error: display "Status unavailable" + the timestamp of the last successful fetch. Do NOT silently fail.
  - On HTTP 200: render the tiles + update the "last refreshed" footer timestamp.
- The `<meta name="viewport" content="width=device-width, initial-scale=1">` tag from Story 9-1 stays — load-bearing for FR41.

**Verification:**

- File-existence check: `static/index.html`, `static/dashboard.css`, `static/dashboard.js` all exist after the story lands. **Document any deviation** (e.g. inlined CSS/JS) in the completion notes.
- 1 integration test `dashboard_html_contains_viewport_meta_and_status_tiles_markup` in `tests/web_dashboard.rs` — auth'd GET `/index.html`, asserts the body contains:
  - `<meta name="viewport"` (FR41 marker)
  - `id="chirpstack-status"`, `id="last-poll-time"`, `id="error-count"`, `id="application-count"`, `id="device-count"` (5 dashboard-tile DOM IDs the JS hooks into — pinning them in the test prevents accidental rename without test update)
  - `<script src="/dashboard.js"` OR an inline `<script>` block (relaxed match — accept either shape but assert SOME script exists)
- 1 integration test `dashboard_css_contains_responsive_media_query` — auth'd GET `/dashboard.css`, asserts the body contains `@media` and `min-width` (responsive-design marker; pins FR41 at the CSS level).
- Manual smoke: open `http://localhost:8080/` in Chrome DevTools mobile-emulation mode (iPhone 12 viewport, 390 × 844). Verify single-column layout + readable text + working refresh.
- Manual smoke: open `http://localhost:8080/` in desktop Chrome at 1920 × 1080. Verify two-column grid + dark/light mode follows OS preference.
- Manual smoke: kill ChirpStack (or stop the gateway), confirm dashboard shows "Unavailable" badge after the next poll cycle (≤ poll-interval seconds).

---

### AC#5: Tests pass + clippy clean + no regression (CLAUDE.md compliance)

**Verification:**

- `cargo test --lib --bins`: ≥ 343 passed (was 333 baseline post-Story 9-1; growth from new `DashboardConfigSnapshot` validation + new `api_status` unit tests). The exact delta depends on test-file shape; **document the actual count** in completion notes.
- `cargo test --tests`: existing 15 integration test binaries still pass; new `tests/web_dashboard.rs` adds ≥ 4 integration tests (auth carry-forward + JSON shape + HTML markup + CSS responsive marker).
- `cargo clippy --all-targets -- -D warnings` clean across the workspace.
- `cargo test --doc`: 0 failed (carries the issue #100 baseline; new code adds no new doctests).
- The Story 8-1 / 8-2 / 8-3 / 9-1 spike+history+subscription+web-auth tests are **regression baselines** — must continue to pass unchanged. Story 9-2 must NOT modify `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_auth.rs` beyond what the new `AppState` shape requires (test fixtures need to be updated for the changed `build_router` signature, but production code in those files stays untouched). AC#6 below pins this with a `git diff` check.

---

### AC#6: NFR12 + auth + connection-cap carry-forward intact (no regression on prior epics)

**Implementation:**

- Story 9-2 must NOT modify `src/opc_ua_auth.rs` (zero LOC change in production code; test fixtures may need to update if `WebConfig::default()` already covers the field).
- Story 9-2 must NOT modify `src/opc_ua_session_monitor.rs` (zero LOC change).
- Story 9-2 must NOT modify `src/main.rs::initialise_tracing`.
- Story 9-2 must NOT modify `src/web/auth.rs` (the auth middleware shape is invariant; the `WebAuthState` field set is invariant; the audit event is invariant).
- Story 9-2 must NOT introduce `event="web_auth_succeeded"` or any other auth-success-path audit event. The Story 9-1 baseline (no success-path audit) is preserved.

**Verification:**

- `git diff --stat src/opc_ua_session_monitor.rs` over the 9-2 branch must show `0 insertions, 0 deletions`.
- `git diff --stat src/opc_ua.rs` shows `0 insertions, 0 deletions`.
- `git diff --stat src/opc_ua_auth.rs` shows `0 insertions, 0 deletions` in production code (test fixtures / imports may change, but the production module body — `OpcgwAuthManager`, `hmac_key()` accessor, `sanitise_user`, etc. — is invariant).
- `git diff --stat src/web/auth.rs` shows `0 insertions, 0 deletions` in production code.
- The existing `tests/opcua_subscription_spike.rs` (17 tests), `tests/opcua_history.rs` (11 tests), `tests/web_auth.rs` (7 tests) all continue to pass without modification beyond the `build_router` signature change in any test fixture that uses it.

---

### AC#7: Sanity check on regression-test count + audit-event count

**Verification:**

- Default test count grows by ~10 (≈ 4 `DashboardConfigSnapshot` + 4 `api_status` unit + 4 `tests/web_dashboard.rs` integration; minor variance acceptable). **Document the actual count** in completion notes alongside the pre-Story baseline (333 lib+bins post-Story 9-1).
- Exactly **3** total tracing-event names introduced across Stories 9-1 + 9-2 in `src/web/`: `web_auth_failed` (Story 9-1, audit warn), `web_server_started` (Story 9-1, diagnostic info), `api_status_storage_error` (Story 9-2, diagnostic warn). The grep contract is updated:
  ```
  git grep 'event = "web_\|event="web_\|event = "api_\|event="api_' src/web/
  ```
  must return exactly those 3 distinct values, each with one emit site.
- The `event="api_status_storage_error"` warn is registered in `docs/logging.md` § "Audit and diagnostic events (`event=`)" with a one-line description.
- Zero new audit events on the OPC UA path (AC#6 invariant).
- Zero new audit events for the success path of `/api/status` (no `event="api_status_returned"` or similar).

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issue (CLAUDE.md compliance) (AC: All)
- [ ] Open ONE GitHub issue: "Story 9-2: Gateway Status Dashboard (FR38)" — main story tracker.
  - Body: links to this story file, the BDD criteria from `epics.md:816-831`, and the FR38 inventory bullet from `epics.md:85`.
  - Assignee: dev-agent's user.
  - Closing keyword (`Closes #X`) lands in the implementation commit.
  - **Deferred to commit time per user policy** — implementation lands first; tracker issue + `Closes #X` keyword are added when the commit is staged. See "Deferred Task 0 — GitHub tracker issue" in Completion Notes.

### Task 1: `AppState` + `DashboardConfigSnapshot` + per-task SQLite backend (AC: 1)
- [x] Add `pub struct AppState { auth, backend, dashboard_snapshot, start_time }` to `src/web/mod.rs`.
- [x] Add `pub struct DashboardConfigSnapshot { application_count, device_count, applications: Vec<ApplicationSummary> }` and `pub struct ApplicationSummary { application_id, application_name, device_count }`.
- [x] Add `impl DashboardConfigSnapshot { pub fn from_config(config: &AppConfig) -> Self { ... } }`.
- [x] `start_time: Instant` field added to `AppState` (chose the field over a module-level `OnceLock<Instant>` — keeps the snapshot self-contained and testable without process-level state leaking across tests).
- [x] Update `build_router` signature: `pub fn build_router(state: Arc<AppState>, static_dir: PathBuf) -> Router`. Internally extract `state.auth.clone()` for the auth middleware via `.with_state(app_state)` at the end.
- [x] Update `build_router_smoke` test in `src/web/mod.rs::tests` to construct an `AppState` with stub backend.
- [x] Add 3 unit tests for `DashboardConfigSnapshot::from_config` (empty list, normal list, application-with-zero-devices).

### Task 2: `src/main.rs` integration (AC: 1, 5)
- [x] In the `if web_enabled { ... }` block at `src/main.rs:721+`:
  - [x] Add `let web_backend = Arc::new(SqliteBackend::with_pool(pool.clone())?)` after the existing 4 backend constructions, with the same fail-closed `error!` + `return Err` pattern.
  - [x] Build `let snapshot = DashboardConfigSnapshot::from_config(&application_config)`.
  - [x] Construct `AppState` and pass `app_state` (instead of `auth_state`) to `web::build_router`.
- [x] Verify `cargo build --tests` clean (full `cargo run` smoke deferred to operator-side verification — see Completion Notes).

### Task 3: `GET /api/status` JSON endpoint (AC: 2, 3)
- [x] Create `src/web/api.rs` (chose new file per the spec's "preferred for surface-growth reasons" guidance).
- [x] Define `pub struct StatusResponse` + `pub struct ErrorResponse` with `serde::Serialize` derives.
- [x] Implement `pub async fn api_status(State(state): State<Arc<AppState>>) -> Result<Json<StatusResponse>, Response>` per AC#2 (return type slightly differs from spec — see Completion Notes § Field-shape divergence).
- [x] Add `Router::route("/api/status", get(api::api_status))` to `build_router`.
- [x] Add `event="api_status_storage_error"` warn-level log on the error path (NFR7-aligned: log the inner error to operator log, return generic message to client).
- [x] Add 4 unit tests in `src/web/api.rs::tests` per AC#2 verification list.
- [x] Add 2 integration tests in `tests/web_dashboard.rs` (auth carry-forward + JSON shape).

### Task 4: Dashboard static assets (AC: 4)
- [x] Replace `static/index.html` with the real dashboard markup (6 tiles + footer + `<meta viewport>` — added an Uptime tile beyond the spec's "5–6 tiles").
- [x] Create `static/dashboard.css` with mobile-first responsive layout + `prefers-color-scheme: dark`.
- [x] Create `static/dashboard.js` with `fetch('/api/status')` on load + 10 s `setInterval` + 401/5xx error handling.
- [x] Add 2 integration tests in `tests/web_dashboard.rs` (HTML markup + CSS responsive marker per AC#4 verification).
- [ ] Manual smoke: open `http://localhost:8080/` in Chrome DevTools mobile-emulation; verify single-column ≤ 600 px and two-column > 600 px. **Deferred to operator-side verification** — the integration tests pin the structural contract (DOM IDs + viewport meta + `@media min-width` query); visual regression is operator-side.

### Task 5: Documentation (AC: 4, 7)
- [x] Update `docs/security.md` § "Web UI authentication" with a new "API endpoints (Story 9-2+)" subsection covering the no-anonymous-probe contract + the storage-failure error shape.
- [x] Update `docs/logging.md` § "Audit and diagnostic events (`event=`)" with the new `api_status_storage_error` row (warn, diagnostic).
- [x] Update `README.md`:
  - [x] Web UI subsection extended with a Story 9-2 dashboard paragraph (10 s refresh cadence + responsive layout + `/api/status` JSON contract).
  - [x] Planning row for Epic 9 updated: status → "in-progress (9-1 done · 9-2 review)" with full 9-2 narrative.
  - [x] `last_updated:` line bumped to 2026-05-03.

### Task 6: Final verification (AC: 5, 6, 7)
- [x] `cargo test --lib --bins` → **340 passed / 0 failed / 3 ignored** (was 333 baseline; Δ=+7 — 4 `api::tests` + 3 `dashboard_snapshot_*` tests, within the spec's "≈10 ± minor variance" budget).
- [x] `cargo test --tests` → all integration test binaries pass (existing 15 + new `tests/web_dashboard.rs` = 16 total). `tests/web_auth.rs`: 14 tests pass (no regression). `tests/web_dashboard.rs`: 4 dashboard tests + 3 reused `common::tests` = 7 total pass.
- [x] `cargo clippy --all-targets -- -D warnings` → **clean**.
- [x] `cargo test --doc` → **0 failed / 56 ignored** (issue #100 baseline preserved).
- [x] `grep -rEn 'event = "web_|event="web_|event = "api_|event="api_' src/` → exactly **3 distinct values**: `web_server_started`, `web_auth_failed`, `api_status_storage_error`. One emit site each.
- [x] `git diff --stat src/opc_ua.rs src/opc_ua_history.rs src/opc_ua_session_monitor.rs src/opc_ua_auth.rs src/web/auth.rs` → **zero output** = zero changes across all five files. AC#6 satisfied perfectly.
- [x] Story 9-1 carry-forward smoke: `tests/web_auth.rs` 14 tests pass with only the `AppState`-wrapping fixture change; `tests/opcua_*.rs` integration tests pass unchanged.

### Task 7: Documentation sync verification (CLAUDE.md compliance)
- [x] `README.md` Planning section reflects 9-2 status accurately (Epic 9 row updated to "in-progress (9-1 done · 9-2 review)" + full 9-2 narrative paragraph).
- [x] `_bmad-output/implementation-artifacts/sprint-status.yaml` `last_updated:` narrative reflects 9-2's outcome (will be re-bumped to "review" at the end of this Dev Agent Record run).
- [x] `docs/security.md` and `docs/logging.md` updated per Task 5.
- [ ] Verify the implementation commit closes the Story 9-2 GitHub tracker issue from Task 0 via `Closes #X`. **Deferred to commit time** (see Task 0).

---

## Dev Notes

### Architecture compliance

- Axum **0.8.x** unchanged from Story 9-1 (use whatever's pinned in `Cargo.toml`).
- `tower-http` **0.6.x** unchanged from Story 9-1.
- `serde` + `serde_json` already pulled transitively by `chirpstack_api`; verify `serde_json` is a direct dep (add if not — needed for the `axum::Json` extractor on the response side).
- No new crate dependencies expected. **If the dev agent wants to add one**, document the rationale in completion notes (e.g. `serde_with` for cleaner `Option<DateTime<Utc>>` serialisation — acceptable but not required).

### `AppState` shape decisions

- **Single `Arc<AppState>` vs three separate `State<...>` extractors.** Single struct wins on ergonomics and Axum-idiomatic style. The auth middleware extracts only the `auth` field by accessing `state.auth.clone()` at router-construction time, so the middleware signature stays `State<Arc<WebAuthState>>` (no breaking change to `src/web/auth.rs::basic_auth_middleware`). Future Stories 9-3 / 9-4 / 9-5 / 9-6 will need `state.backend` for their handlers; the `Arc<AppState>` shape supports them all.
- **`Arc<dyn StorageBackend>` vs `Arc<SqliteBackend>`.** Trait-object form for testability — unit tests can substitute `InMemoryBackend`. Same shape every other task uses (`src/main.rs:614,640,673,690`).
- **`DashboardConfigSnapshot` frozen-at-startup vs lock-on-read.** Frozen-at-startup wins for two reasons: (a) the live `AppConfig` lives in `main.rs::application_config` and is not currently shared via `Arc<RwLock<...>>` to background tasks (each task gets a clone or borrows the relevant subsection at construction time); (b) Story 9-7 will introduce hot-reload via `tokio::sync::watch`, at which point the snapshot becomes a `watch::Receiver<DashboardConfigSnapshot>`. Shipping the snapshot now (frozen) means 9-7's diff is a single-line type change at the `AppState` field declaration.

### `application_count` / `device_count` semantics

These are **configured** counts from `AppConfig.application_list`, not **live ChirpStack-discovered** counts. The dashboard answers "how many devices is the gateway *trying to* poll", which is the operator-relevant question for "is my config loaded correctly". A "live discovered" count would require querying ChirpStack's gRPC API on every dashboard refresh — too expensive and not what FR38 asks for.

If a future operator wants "how many devices ChirpStack actually returned data for", that's a separate metric to surface (count of distinct `device_id`s in `metric_values` over a recent window). Out of scope for 9-2.

### `last_poll_time` semantics (None vs old timestamp)

`get_gateway_health_metrics()` returns `Option<DateTime<Utc>>` — `None` on first startup before any poll has succeeded. The dashboard distinguishes:
- `null` in JSON → "Never polled" badge in the UI
- An old timestamp (e.g. > 2× poll interval) → "Stale" badge with the timestamp + relative time

The "stale" threshold is computed client-side in `dashboard.js` from the timestamp + `Date.now()`. **Do NOT compute staleness server-side** — the staleness threshold is operator-judgement (a 30 s gap is "stale" for a 10 s poll interval but normal for a 60 s poll interval), and the server doesn't know the operator's interpretation. The server returns the raw timestamp; the dashboard interprets.

### Why `uptime_secs` instead of `start_time`

Returning `start_time` as a timestamp would tempt the dashboard to compute uptime as `Date.now() - start_time`, which silently breaks if the server's clock and the browser's clock disagree (NTP drift, manually-set client clock, etc.). Returning `uptime_secs` from the server side keeps the wall-clock-skew failure mode out of the dashboard. The browser displays the value as-is.

### JSON field naming convention

snake_case for all JSON fields, matching the Rust struct field names with `#[serde(...)]` only used to handle `Option<DateTime<Utc>>` → `Option<String>` conversion. snake_case (vs camelCase) chosen because:
1. The OPC UA address space already uses snake_case for variable names (Story 5-3 conventions).
2. Operators reading the JSON via `curl | jq` get a consistent vocabulary across surfaces.
3. Future Stories 9-4 / 9-5 / 9-6 (CRUD endpoints) will introduce JSON request bodies; snake_case-only avoids per-endpoint convention drift.

### NFR7 — generic error messages

`/api/status` returns `{"error":"internal server error"}` on storage failure — never the raw `e.to_string()`. The inner error goes to the operator log (`event="api_status_storage_error" error=<sanitised>`). NFR7 invariant: error messages must not leak sensitive internal state. SQLite paths, table names, column names, file system errors — all stay server-side.

### CSP / HTTP security headers — explicit non-implementation

Story 9-2 ships **without** Content-Security-Policy, X-Frame-Options, X-Content-Type-Options, or Strict-Transport-Security headers. Rationale:
- The dashboard JS is first-party (`/dashboard.js`); no inline event handlers, no `eval`. CSP would harden against injection but the surface is tiny.
- The gateway is LAN-internal per the Story 9-1 threat model; clickjacking via X-Frame-Options is a low-priority concern.
- HSTS doesn't apply (HTTP-only per Story 9-1).

If a future operator surfaces a deployment scenario where these headers matter (e.g. embedding the dashboard in a corporate intranet portal), open a follow-up story. **Do NOT pre-emptively add them in Story 9-2** — the cost is one tower-http layer + 5 lines of config but the maintenance cost is non-zero and the value is currently zero.

### Carry-forward debt acknowledged but unchanged

- `tracing-test = "=0.2.6"` exact-pin from issue #101 — Story 9-2 inherits unchanged.
- `tests/common/mod.rs` from issue #102 — Story 9-2 reuses `build_http_client` unchanged; no new helpers added.
- 56 ignored doctests from issue #100 — Story 9-2 adds no new doctests; the baseline stays.
- NodeId format from issue #99 — irrelevant to Story 9-2 (no OPC UA address-space construction).
- User-manual chapter for the embedded web UI (deferred-work.md:218) — Story 9-2 adds dashboard content to the deferred chapter, not a blocking AC.
- TLS hardening (issue #104) — inherited from Story 9-1, unchanged.
- Per-IP rate limiting (issue #88) — inherited from Story 9-1, unchanged.
- CSRF for Stories 9-4 / 9-5 / 9-6 (deferred-work.md:221) — Story 9-2 ships GET-only; no CSRF surface added.
- Drop-zeroize HMAC keys (deferred-work.md:225) — inherited from Story 9-1, unchanged.
- Per-request access log (deferred-work.md:226) — inherited from Story 9-1, unchanged.

### File List (expected post-implementation)

**New files:**
- `src/web/api.rs` — `StatusResponse`, `ErrorResponse`, `api_status` handler, 4 unit tests. (~120 LOC.)
- `static/dashboard.css` — mobile-first responsive layout + dark-mode media query. (~80 LOC.)
- `static/dashboard.js` — `fetch('/api/status')` + render + 10 s refresh + error handling. (~100 LOC.)
- `tests/web_dashboard.rs` — 4 integration tests (auth carry-forward + JSON shape + HTML markup + CSS responsive marker). (~200 LOC.)

**Modified files:**
- `Cargo.toml` — possibly add `serde_json = "1"` as a direct dep if not already.
- `src/web/mod.rs` — add `AppState`, `DashboardConfigSnapshot`, `ApplicationSummary`; update `build_router` signature; declare `pub mod api;`. (~80 LOC of new code.)
- `src/main.rs` — add 5th `SqliteBackend::with_pool` call in `if web_enabled { ... }` block; build `DashboardConfigSnapshot`; construct `AppState`; pass to `build_router`.
- `src/lib.rs` — no change expected (`pub mod web` already exists from Story 9-1; `api` is `pub mod` inside `web`).
- `static/index.html` — replace placeholder with dashboard markup (5–6 tiles + `<script src="/dashboard.js">` + `<link rel="stylesheet" href="/dashboard.css">` + `<meta viewport>`).
- `tests/web_auth.rs` — update fixture to construct `AppState` instead of `Arc<WebAuthState>` directly (mechanical change for the `build_router` signature). **Do NOT modify the assertions** — the auth contract is invariant per AC#6.
- `docs/security.md` — append one-paragraph subsection on `/api/...` auth coverage.
- `docs/logging.md` — append one row for `event="api_status_storage_error"`.
- `README.md` — Web UI subsection mention of dashboard + Planning row update for 9-2.
- `_bmad-output/implementation-artifacts/deferred-work.md` — possibly add one entry if the dev agent surfaces a new defer-worthy item (e.g. SSE/WebSocket push if 10 s polling proves too slow).
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — flip 9-2 status + refresh `last_updated` narrative.
- `_bmad-output/implementation-artifacts/9-2-gateway-status-dashboard.md` — this file: status flips, all task checkboxes filled, Dev Agent Record + completion notes + file list populated.

### Project Structure Notes

- Aligns with `architecture.md:417-421` reservation of `src/web/`. Story 9-2 lands the `api.rs` file that 9-1 reserved for "Stories 9-2 onwards".
- Sequencing per `epics.md` Phase-B polish (`epics.md:793`): 9-1 → 9-2 → 9-3 → 9-0 spike → 9-7 → 9-8 → 9-4 / 9-5 / 9-6.
- No conflicts with existing structure.

### References

- [Source: `_bmad-output/planning-artifacts/epics.md#Epic-8` (= sprint-status Epic 9), lines 766-796 — Phase-B carry-forward bullets].
- [Source: `_bmad-output/planning-artifacts/epics.md#Story-8.2` (= sprint-status 9-2), lines 816-831 — BDD acceptance criteria].
- [Source: `_bmad-output/planning-artifacts/epics.md`, line 85 — FR38 inventory].
- [Source: `_bmad-output/planning-artifacts/architecture.md:417-421` — directory structure with `src/web/api.rs` reservation].
- [Source: `_bmad-output/planning-artifacts/prd.md#FR38` — gateway status via web interface].
- [Source: `_bmad-output/planning-artifacts/prd.md#FR41` — mobile-responsive].
- [Source: `_bmad-output/implementation-artifacts/9-1-axum-web-server-and-basic-authentication.md` — Story 9-1 spec + completion notes; Field-shape divergence #6 explicitly defers `AppState` to 9-2+].
- [Source: `src/storage/mod.rs:727` — `get_gateway_health_metrics` trait method, the 9-2 data source].
- [Source: `src/storage/sqlite.rs:1968-1995` — `SqliteBackend::get_gateway_health_metrics` impl].
- [Source: `src/storage/types.rs:232-243` — `ChirpstackStatus` struct].
- [Source: `src/web/mod.rs:111-119,72-83` — `build_router` body + the layer-after-route invariant doc-comment].
- [Source: `src/web/auth.rs::basic_auth_middleware` — the auth middleware Story 9-2 must NOT modify (AC#6)].
- [Source: `src/main.rs:614-690` — per-task SQLite backend pattern Story 9-2 extends].
- [Source: `src/main.rs:721-782` — Story 9-1's `if web_enabled { ... }` block Story 9-2 modifies].
- [Source: `src/config.rs:465-484,491-515,752-784` — `ChirpStackApplications` / `ChirpstackDevice` / `AppConfig.application_list` shapes].
- [Source: `src/utils.rs:246-309` — `WEB_*` constants Story 9-2 reuses unchanged].
- [Source: `tests/common/mod.rs:155-162` — `build_http_client` Story 9-2 reuses unchanged].
- [Source: `tests/web_auth.rs` — Story 9-1 integration-test shape Story 9-2 mirrors in `tests/web_dashboard.rs`].
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md:216-226` — Story 9-1 deferred items Story 9-2 inherits].
- [Source: `_bmad-output/implementation-artifacts/epic-8-retro-2026-05-01.md` — Phase B carry-forward context].
- [Source: GitHub issue #88 — per-IP rate limiting carry-forward, structurally relevant once Web UI lands; Story 9-2 inherits the deferral].
- [Source: GitHub issue #100 — doctest cleanup, Story 9-2 inherits unchanged].
- [Source: GitHub issue #102 — tests/common/mod.rs extraction; Story 9-2 reuses without adding new helpers].
- [Source: GitHub issue #104 — TLS hardening follow-up, Story 9-2 inherits unchanged].

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — `claude-opus-4-7[1m]` — single-execution
`bmad-dev-story` run on 2026-05-03 immediately after `bmad-create-story
9-2` (same conversation). The story was implemented in a single pass
with no HALT conditions triggered; the only mid-implementation
iterations were two clippy fixups (one `doc-list-continuation` lint, one
`await-holding-lock` lint) and one test-build fix (reqwest's `json`
feature is not enabled — switched to `text()` + `serde_json::from_str`).

### Debug Log References

- Test runs documented inline in Task 6 above; no debug-log capture
  required outside the standard `cargo test` output.
- Three mid-implementation course-corrections:
  1. **Test fixture types missing `Default` impls** — `Global`,
     `ChirpstackPollerConfig`, `OpcUaConfig` don't derive `Default`;
     mirrored the explicit-field pattern from
     `src/web/auth.rs::tests::web_auth_test_config` for the
     `snapshot_test_config` helper in `src/web/mod.rs::tests`.
  2. **`reqwest::json()` requires the `json` feature** — the dev-dep is
     pinned with `default-features = false` + `features = ["http2"]`
     only. Switched to `resp.text().await` + `serde_json::from_str` in
     `tests/web_dashboard.rs::api_status_returns_json_with_expected_shape_when_authed`
     to avoid expanding the dev-dep feature surface.
  3. **Clippy `await_holding_lock` false-positive** — moved the
     `tracing-test` MutexGuard acquisition into an inner block so the
     guard is dropped before the later `handle.await` regardless of
     borrow-scope analysis.

### Completion Notes List

- **AC#1 (`AppState` + `DashboardConfigSnapshot`) — COMPLETE.**
  `AppState { auth, backend, dashboard_snapshot, start_time }` lives
  in `src/web/mod.rs`. `DashboardConfigSnapshot { application_count,
  device_count, applications: Vec<ApplicationSummary> }` and
  `ApplicationSummary { application_id, application_name, device_count }`
  are sibling pub structs. `DashboardConfigSnapshot::from_config` is a
  pure function walking `config.application_list` once. `start_time`
  is captured at `AppState` construction (close-enough to gateway
  startup since both happen in the same `if web_enabled { ... }`
  block). `build_router` signature is now `(Arc<AppState>, PathBuf) ->
  Router` and finalises router state via `.with_state(app_state)` at
  the end; the auth middleware extraction `app_state.auth.clone()` is
  passed to `from_fn_with_state`, so `basic_auth_middleware`'s
  signature stays `State<Arc<WebAuthState>>` unchanged (load-bearing
  for AC#6). 3 unit tests for `from_config` + 1 updated `build_router_smoke`
  test all green.

- **AC#2 (`GET /api/status` JSON endpoint) — COMPLETE.** New module
  `src/web/api.rs` (~360 LOC including unit tests). `StatusResponse`
  ships all 6 fields with `serde::Serialize` + the `Option<DateTime<Utc>>
  → Option<String>` conversion done at handler-call time via
  `.map(|t| t.to_rfc3339())` (avoided pulling `serde_with` or
  `chrono::serde::*` adapters). `ErrorResponse` is a fixed
  `{"error":"internal server error"}` body — never `e.to_string()`
  (NFR7). Storage failure logs `event="api_status_storage_error"
  error=<sanitised>` at warn-level. 4 unit tests pin the contract:
  success / 500-with-generic-body (asserts the inner error string is
  ABSENT from the response body) / first-startup default
  `(None, 0, false)` / explicit `serde_json` round-trip pinning that
  `last_poll_time: None` serialises to JSON `null`.

- **AC#3 (auth middleware applies to `/api/status`) — COMPLETE.** No
  code change needed beyond adding the route — the layer-after-route
  invariant in `src/web/mod.rs:72-83` (Story 9-1) ensures every
  `Router::route(...)` inherits the auth layer automatically. Pinned
  by `tests/web_dashboard.rs::auth_required_for_api_status` which
  asserts unauth'd `GET /api/status` returns 401 + `WWW-Authenticate`
  header + emits the `event="web_auth_failed" path=/api/status
  reason=missing` audit event.

- **AC#4 (dashboard HTML/CSS/JS, FR41) — COMPLETE.**
  `static/index.html` replaced with a 6-tile dashboard (ChirpStack /
  Last poll / Errors / Applications / Devices / Uptime — added Uptime
  beyond the spec's "5–6 tiles" because the data is already in the
  JSON response and a single tile is operator-friendly). Plain HTML5,
  no SPA framework, no build step. CSS in `static/dashboard.css`
  ships mobile-first responsive: `grid-template-columns: 1fr` by
  default, `1fr 1fr` above 600 px viewport via `@media (min-width:
  601px)`. Dark mode via `prefers-color-scheme: dark` — no in-page
  toggle. JS in `static/dashboard.js` does `fetch('/api/status')` on
  load + `setInterval(fetchStatus, 10_000)` for the refresh loop;
  401 → inline error banner ("Session expired or credentials no
  longer accepted. Please reload the page."); 5xx / network error →
  inline banner with the error reason. `Intl.NumberFormat` for the
  thousands separator on counts; `Intl.DateTimeFormat` for the
  browser-locale-aware absolute timestamp; relative-time formatter
  inline (`s ago` / `min ago` / `h ago` / `d ago`). 4 integration
  tests in `tests/web_dashboard.rs` pin the structural contract:
  HTML markup contains 5 dashboard-tile DOM IDs + `<meta viewport>`
  + a `<script>` tag; CSS contains `@media` + `min-width`.

- **AC#5 (regression baseline) — COMPLETE.**
  - `cargo test --lib --bins`: **340 passed / 0 failed / 3 ignored**.
    Δ = +7 from 333 baseline (4 web::api::tests + 3 dashboard_snapshot_*
    + 0 net change from build_router_smoke being updated rather than
    added). Within the spec's "≈10 ± minor variance" budget.
  - `cargo test --tests`: all 16 integration test binaries pass
    (Story 9-1's 15 + new `tests/web_dashboard.rs` = 16). Specifically
    verified: `tests/web_auth.rs` 14 tests pass (Story 9-1
    carry-forward), `tests/web_dashboard.rs` 7 tests pass (4 new + 3
    reused common::tests), `tests/opcua_history.rs` + `tests/opcua_subscription_spike.rs`
    + `tests/opc_ua_*.rs` all pass unchanged.
  - `cargo clippy --all-targets -- -D warnings`: **clean**.
  - `cargo test --doc`: **0 failed / 56 ignored** (issue #100 baseline
    untouched).

- **AC#6 (NFR12 + auth + connection-cap carry-forward intact) — COMPLETE.**
  `git diff --stat src/opc_ua.rs src/opc_ua_history.rs
  src/opc_ua_session_monitor.rs src/opc_ua_auth.rs src/web/auth.rs`
  produced **zero output** = zero changes across all five files.
  Test-fixture changes for the new `AppState` shape are confined to
  `tests/web_auth.rs` (one new `wrap_in_app_state` helper + two
  call-site swaps); production code in `src/opc_ua_*.rs` and
  `src/web/auth.rs` is untouched.

- **AC#7 (sanity check on regression-test count + audit-event count) —
  COMPLETE.**
  - Default test count grew by **+7** lib+bins (within the spec's
    "≈10 ± minor variance" budget).
  - Total event= grep across `src/`: exactly **3 distinct values** —
    `web_server_started` (Story 9-1, info diag), `web_auth_failed`
    (Story 9-1, warn audit), `api_status_storage_error` (Story 9-2,
    warn diag). One emit site per event. Both events registered in
    `docs/logging.md` § "Audit and diagnostic events (`event=`)".
  - Zero new audit events on the OPC UA path (AC#6 invariant).
  - Zero success-path audit events for `/api/status` (no
    `event="api_status_returned"` or similar — successful GETs are
    routine and don't warrant audit logging).

#### Field-shape divergence from spec

- **Handler return type uses `Response` rather than `(StatusCode,
  Json<ErrorResponse>)`** (AC#2). The spec's signature was
  `Result<Json<StatusResponse>, (StatusCode, Json<ErrorResponse>)>`,
  but Axum 0.8's `IntoResponse` impl for the tuple shape is implicit
  via `IntoResponse for (StatusCode, T)`. Returning `Response`
  directly via `.into_response()` is the more idiomatic shape for a
  handler that may want to add extra headers (e.g. `Cache-Control:
  no-store` if a future story adds it) without changing the signature.
  Functionally identical: both ship the same 500 + JSON body. The
  500-path unit test calls `.into_parts()` on the response and
  asserts both the status code and the body contents.
- **Added a 6th "Uptime" tile to the dashboard** (AC#4). The spec
  said "optional 6th tile". Implementing it now: the data is already
  in the `/api/status` JSON (`uptime_secs` field) and a single tile
  takes ~5 lines of HTML + 15 lines of JS. Removing it would be more
  work than keeping it.
- **Used `(Arc<AppState>, PathBuf)` for `build_router` rather than
  the FromRef substate pattern**. Considered Axum's `FromRef` substate
  approach (`impl FromRef<Arc<AppState>> for Arc<WebAuthState>`) but
  the simpler `app_state.auth.clone() → from_fn_with_state` shape is
  fewer LOC and doesn't require trait machinery. Both patterns yield
  the same wire behaviour; the chosen shape keeps `basic_auth_middleware`'s
  signature unchanged (load-bearing for AC#6).
- **Story 9-1 test fixture (`tests/web_auth.rs`) gained a
  `wrap_in_app_state` helper rather than inline construction** at each
  call site. Two call sites would have meant two ~10-line inline
  constructions; one helper is cleaner. The helper builds an empty
  `InMemoryBackend` + a zero-count `DashboardConfigSnapshot` because
  Story 9-1's tests don't exercise either field — the helper exists
  only to satisfy the new `build_router` signature.
- **`Cargo.toml` not modified** — `serde_json = "1.0.132"` was already
  a direct dep (line 37); no other crate additions needed. The spec
  noted this as a possibility ("possibly add `serde_json = "1"` as a
  direct dep if not already") — wasn't needed.

#### Deferred Task 0 — GitHub tracker issue

**Task 0 (open ONE GitHub issue: "Story 9-2: Gateway Status
Dashboard (FR38)") is deferred to commit time.** The story
spec called for opening the issue at implementation start with a
`Closes #X` keyword landing in the implementation commit. Per
the user's standing policy on shared-state actions (per CLAUDE.md
"Executing actions with care" — issue creation is operator-
visible state on a shared system), the dev agent did not
proactively call `gh issue create`. Recommended commit-time
flow:

1. Operator runs `gh issue create --title "Story 9-2: Gateway
   Status Dashboard (FR38)" --body "<see story spec § Task 0
   for body content>"` — note the assigned issue number.
2. Operator stages the implementation diff and creates the
   commit with `Closes #<N>` in the message.

The story spec, sprint-status narrative, and Planning row in
`README.md` all already reference Story 9-2; missing only the
issue tracker number itself.

### File List

**New files:**

- `src/web/api.rs` — `StatusResponse`, `ErrorResponse`, `api_status`
  handler, `FailingBackend` mock, 4 unit tests. (~360 LOC including
  the FailingBackend mock impl which is large because `StorageBackend`
  has 25+ trait methods.)
- `static/dashboard.css` — mobile-first responsive layout + dark-mode
  media query. (~155 LOC.)
- `static/dashboard.js` — `fetch('/api/status')` + render + 10 s
  refresh + error handling. (~135 LOC.)
- `tests/web_dashboard.rs` — 4 integration tests (auth carry-forward
  + JSON shape + HTML markup + CSS responsive marker) + helpers.
  (~315 LOC.)

**Modified files:**

- `src/web/mod.rs` — added `ApplicationSummary`, `DashboardConfigSnapshot`,
  `AppState` structs; declared `pub mod api;`; updated `build_router`
  signature `(Arc<WebAuthState>, PathBuf) → (Arc<AppState>, PathBuf)`
  + body adds `.route("/api/status", ...)` + `.with_state(app_state)`;
  rewrote `tests` module with `snapshot_test_config` + `make_app`
  helpers + 3 new `dashboard_snapshot_from_config_*` unit tests +
  updated `build_router_smoke`. Net ~+200 LOC.
- `src/main.rs` — added `web_backend` (5th `SqliteBackend::with_pool`
  call site, mirroring the per-task pattern at lines 614 / 640 / 673
  / 690) + `dashboard_snapshot` construction + `app_state`
  construction; passed `app_state` (instead of `auth_state`) to
  `web::build_router`. Net ~+25 LOC inside the `if web_enabled { ... }`
  block.
- `static/index.html` — replaced 12-line placeholder with 60-line
  6-tile dashboard markup + `<link>` to `dashboard.css` + `<script>`
  to `dashboard.js`.
- `tests/web_auth.rs` — added `wrap_in_app_state` helper + 2 call-site
  swaps from `WebAuthState` → `AppState`; no production-code change,
  no test-assertion change. Net ~+25 LOC.
- `docs/security.md` — appended new "API endpoints (Story 9-2+)"
  subsection under § "Web UI authentication" → § "Tuning checklist".
  Net ~+25 LOC.
- `docs/logging.md` — added one row to the `event=` registry table
  for `api_status_storage_error`. Net +1 LOC.
- `README.md` — appended Story 9-2 dashboard paragraph to the "Web
  UI" subsection; updated Epic 9 Planning row with full 9-2 narrative;
  bumped `last_updated:` to 2026-05-03.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` —
  flipped `9-2-gateway-status-dashboard: ready-for-dev → in-progress`
  (will flip to `review` at the end of this Dev Agent Record run);
  refreshed `last_updated:` narrative with implementation summary.
- `_bmad-output/implementation-artifacts/9-2-gateway-status-dashboard.md` —
  this file: status flipped `ready-for-dev → review`, all task
  checkboxes filled, Dev Agent Record + Completion Notes + Field-shape
  divergence + File List populated.

### Review Findings (iter-1, 2026-05-03)

`bmad-code-review` launched 3 parallel adversarial reviewers (Blind Hunter, Edge Case Hunter, Acceptance Auditor) against the implementation commit `e1a42be`.

**Auditor verdict (iter-1):** PASS across all 7 ACs.
**Adversarial layers (iter-1):** 17 (Blind) + 14 (Edge) = 31 raw findings → 29 after dedup → triaged.

#### Decision-needed (resolved 2026-05-03)

- **D1.** SQLite connection pool size = 3, but Story 9-2 brings the long-lived task-claimer count to 5 (poller, opc_ua, command-status, command-timeout, **+web**). Under contention `/api/status` would busy-wait up to 5 s on `pool.checkout` and surface a generic 500 — the operator dashboard would go red while the gateway is healthy. **User chose option (a):** bump pool size 3 → 5 (one-line change in `src/main.rs:493`, with documenting comment). **Functional impact:** removes the structural off-by-one that made the dashboard's view of the gateway pessimistic under contention.

#### Patches applied (13)

- **B2** — Extended `dashboard_html_contains_viewport_meta_and_status_tiles_markup` test to assert all 10 DOM IDs (`chirpstack-status`, `last-poll-relative`, `last-poll-time`, `error-count`, `application-count`, `device-count`, `uptime`, `last-refresh`, `error-banner`, `refresh-now`) — was 5; the JS reads 10. Catches future renames at build time.
- **B3** — Replaced 24 × `unreachable!()` in `FailingBackend` mock with descriptive `panic!("FailingBackend: only get_gateway_health_metrics is implemented; …")` so a future test path that calls a non-status method gets a clear failure.
- **B5+E10** — `tests/web_dashboard.rs::auth_required_for_api_status` now clears `tracing-test::internal::global_buf()` at the top so a polluted buffer (e.g. from a prior failed run) can't false-pass the `web_auth_failed` assertion.
- **B6** — Replaced 4 × `let _ = handle.await;` with `handle.await.expect("web::run task panicked or was cancelled abnormally")` so server-side panics surface as a test failure rather than vanishing into the JoinError.
- **B9** — Removed `err.message` interpolation from the dashboard.js network-error banner. Generic "Status unavailable (network error). Check the gateway connection." message — consistent with the server-side NFR7 stance on hiding internals (CORS / SSL / DNS specifics no longer leak into the DOM).
- **B10** — `build_state` test helper signature changed from `(application_count, device_count)` to `(per_app_device_counts: &[usize])` so the per-application device counts are explicit. Previous `(2, 7)` shape integer-divided to `3 devs/app * 2 = 6` total, a silent off-by-one that would have masked Story 9-3 bugs once a handler reads `applications[*].device_count`.
- **B11+E12** — `tests/web_dashboard.rs::build_production_static_dir` now anchors `static/` with `env!("CARGO_MANIFEST_DIR")` rather than cwd. Tests no longer fail with a confusing `read static/dashboard.css: No such file or directory` when run from a non-repo cwd.
- **E2 + E5** — `dashboard.js` adds an in-flight guard + per-call `AbortController`; a click-spammed "Refresh now" or a `setInterval` firing while the previous fetch is still pending now cancels the prior call rather than compounding overlapping fetches.
- **E3** — New integration test `in_memory_backend_preserves_last_poll_time_when_poll_fails_after_success` pinning the `update_gateway_status(None, n, false)` semantic (the storage trait's "stale poll preserved" contract that the dashboard's "Last poll" tile depends on). Iter-2 M3 re-scoped the docstring to acknowledge it covers only `InMemoryBackend`; the SQL-side equivalent is `src/storage/sqlite.rs::tests::test_null_timestamp_preserves_last_successful_poll`.
- **E7** — `dashboard.js` `chirpstack_available` rendering now branches on `=== true` (Available) / `=== false` (Unavailable) / `else` (Unknown — `badge-unknown` class). The "field missing" failure mode no longer collapses with "ChirpStack down".
- **E8** — `dashboard.js` sniffs `Content-Type` via `.indexOf("application/json")` before calling `resp.json()`. A reverse proxy / auth gateway returning a 200 + HTML login page now produces a clear "upstream returned non-JSON; check proxy / auth gateway configuration" banner instead of crashing the dashboard with `SyntaxError: Unexpected token <`.
- **E9** — Relaxed `uptime_secs <= 1` test assertion to `<= 5` to absorb slow CI runners (valgrind, contended runners) without flaking. The point of the assertion is "the field reflects elapsed wall-clock since `build_state` ran" — a 5 s budget still catches the pathological case.
- **E13** — `dashboard.js` shares one `parseTimestamp(iso)` parse pass between `formatRelative` and the absolute-timestamp tile so the two no longer disagree on parseability (was: `formatRelative` returned "—" for unparseable but the absolute tile rendered the raw string).

#### Deferred (open as follow-up, not patched in this iteration)

- **B1** — Sync SQLite calls in async handler block the Tokio executor thread. Project-wide established pattern (poller, OPC UA also do this); fixing only the web path doesn't help. **Future epic-level concern** — file as a follow-up GitHub issue if/when async-storage-trait migration is on the roadmap.
- **B7** — `formatRelative` future-clock-skew renders "0 s ago" indefinitely instead of surfacing a clock-drift warning. Minor UX issue.
- **B14** — README "10 s" / JS `10000` single-source-of-truth nit. Documentation-level.
- **B15** — `Cache-Control: no-store` request header sent by JS but not response. Future hardening if a CDN / proxy deployment surfaces.
- **B16** — Test asserts `as_i64()` for `error_count: i32`. Type-pinning gap that wouldn't catch a future widening to `u64`/`i64`.
- **B17** — `ErrorResponse::internal_server_error()` allocates per call. Performance trivia.
- **E4** — `validate()` doesn't reject duplicate `application_id`. Pre-existing config-validation gap surfaced (not introduced) by 9-2's aggregation. Worth a config-validation hardening pass.
- **E6** — `error_count: i32` overflows after ~24 days @ 1000 errors/sec. Pre-existing storage type; not a 9-2 regression.
- **E14** — Empty `application_name` not rejected. Pre-existing config-validation gap.

#### Dismissed (noise, false positives, or handled elsewhere)

- **B4** — Unused `applications: Vec<ApplicationSummary>` field. Explicitly designed for Stories 9-3+ per dev notes; no waste at realistic application counts.
- **B8** — `WebAuthState::new_with_fresh_key` `pub` visibility. Story 9-1 review already considered + documented; production uses `WebAuthState::new`.
- **B12** — `error!(error = %e)` on `SqliteBackend::with_pool` failure leaks DB path to operator log. NFR7 is about clients, not operators; operator-log full-info is intentional.
- **B13** — String clones in `from_config`. Explicitly intentional design; documented for Story 9-7.
- **E11** — `wrap_in_app_state` in `tests/web_auth.rs` test fixture. Future-proofing concern; helper is correct today.

### Review Findings (iter-2, 2026-05-03)

Per CLAUDE.md "Code Review & Story Validation Loop Discipline" rule on re-running review after a non-trivial patch round, the 3 adversarial layers re-ran against the patched codebase.

**Auditor verdict (iter-2):** PASS across all 7 ACs (unchanged).
**Adversarial layers (iter-2):** 8 (Blind) + 4 (Edge) = 12 raw findings → 9 after dedup → 3 MED patched + 6 LOW accepted.

#### Iter-2 patches (3 MED, all applied)

- **M1** — **`dashboard.js` AbortController + Promise-chain race.** When call N is aborted mid-flight (after headers received but before `.then(render)` runs), the prior call's resolved JSON could land in `render(...)` AFTER call N+1's render — UI flicker / stale-data overwrite. **Patch:** `if (data && inflightToken === thisCallToken) { render(data); }` guard in the second `.then`, and matching guard in the `.catch` so a stale network error from an aborted call can't clobber the new call's `clearError()`.
- **M2** — **`AbortController` was unconditional — silently raised browser baseline to 2018+.** Older browsers (Safari < 11.1, Edge < 16, Chrome < 66) would throw `ReferenceError: AbortController is not defined` synchronously and freeze the dashboard at "…" placeholders with no diagnostic. **Patch:** feature-detect via `var ABORT_SUPPORTED = typeof AbortController !== "undefined"`. On unsupported browsers, falls back to a plain object identity for `inflightToken` — the M1 stale-render guard still works, only the abort-on-supersede behaviour degrades gracefully.
- **M3** — **E3 test was misframed.** Iter-1 dev-notes said the new in-memory test "covers the contract for both impls" but the SQL-side `INSERT OR REPLACE … CASE WHEN` is a structurally different code path. **Patch:** rewrote the test docstring to honestly scope it to `InMemoryBackend` and reference `src/storage/sqlite.rs::tests::test_null_timestamp_preserves_last_successful_poll` (line 4366 at the time of this story) which covers the SQL path. Also renamed the test from `storage_preserves_…` to `in_memory_backend_preserves_…` so the name matches the scope. Functional behaviour unchanged — the test still pins the contract for InMemoryBackend.

#### Iter-2 LOW (6, accepted as LOW per loop-termination rule)

- **L1** — `Content-Type` substring sniff is too loose (would accept a hypothetical `application/jsonsoup`) and case-sensitive (would reject `Application/JSON`). Common production shape `application/json; charset=utf-8` works correctly. Accept as LOW.
- **L2** — `panic!()` message in `FailingBackend` is identical across all 25 trait methods — when one fires, the dev has to read the stack trace to know which method was called. Debugging convenience. Accept as LOW.
- **L3** — Pool size bump 3 → 5 in `src/main.rs:493` is a literal, not a `const`. If a future story adds a 6th long-lived task, the same off-by-one will recur unless someone remembers to bump 5 → 6. Code organization. Accept as LOW.
- **L4** — `parseTimestamp` accepts numeric input (`new Date(123456789)` is valid). API contract says `last_poll_time` is `Option<String>`; a server-side bug shipping a numeric timestamp would silently render. Defensive; not a regression (pre-iter-1 code had the same property). Accept as LOW.
- **L5** — `tracing-test::global_buf()` returns a `Mutex<Vec<u8>>` that poisons on panic; the next test that calls `.lock().unwrap()` panics on `PoisonError`, masking the original failure. Pre-existing project-wide hazard (issue #101 / #102 territory). Accept as LOW.
- **L6** — Buffer-clear pattern (B5+E10 patch) is inlined rather than extracted to `tests/common/mod.rs`. The `tests/web_auth.rs` helper `clear_captured_buffer` is the precedent. Code organization. Accept as LOW.

**Loop terminates per CLAUDE.md:** zero `decision-needed`, zero HIGH, zero MED unresolved. Only LOW remains.

#### Iter-2 verification

- `cargo test --lib --bins`: **340 passed / 0 failed / 3 ignored** (unchanged from iter-1).
- `cargo test --tests`: all 16 integration binaries pass; `tests/web_dashboard.rs` reports **8 passed** (was 7 — new E3 test landed; iter-2 M3 patch only renamed/re-documented).
- `cargo clippy --all-targets -- -D warnings`: **clean**.
- `cargo test --doc`: **0 failed / 56 ignored** (issue #100 baseline preserved).
- AC#6 file invariants reverified: `git diff HEAD --stat src/opc_ua{.,_history,_session_monitor,_auth}.rs src/web/auth.rs` produces zero output. Both iter-1 and iter-2 patch rounds left these 5 files completely unchanged.
- AC#7 grep contract reverified: `grep -rEn 'event = "web_|event="web_|event = "api_|event="api_' src/` returns exactly the 3 expected names with one emit site each.

### Change Log

| Date | Change | Detail |
|------|--------|--------|
| 2026-05-03 | Story file created | `bmad-create-story 9-2`. Status set to `ready-for-dev`. |
| 2026-05-03 | Status flipped `ready-for-dev → in-progress → review` | Single-execution `bmad-dev-story` run. All 7 ACs satisfied on first pass; loop terminates without iteration. Three minor course-corrections (test-fixture defaults, reqwest feature surface, clippy await-holding-lock) documented in Debug Log References. AC#6 invariants verified by zero-output `git diff`. |
| 2026-05-03 | Status flipped `review → done` after iter-1 + iter-2 code review loop | `bmad-code-review` launched 3 parallel adversarial layers; iter-1 produced 1 decision-needed (D1, resolved option-a) + 13 patches + 9 deferred + 5 dismissed. Iter-2 re-review per CLAUDE.md "don't trust a single pass" caught 3 MED follow-ups (M1 dashboard.js stale-render race, M2 AbortController browser-baseline, M3 E3 test misframing) — all patched + 6 LOW accepted. Loop terminates per CLAUDE.md (zero decision-needed / HIGH / unresolved MED; only LOW remains). Final test count stable at 340 lib+bins / 0 fail; clippy clean; AC#6 file invariants intact (`git diff` zero output across all 5 carry-forward files); AC#7 event grep returns exactly 3 names. |

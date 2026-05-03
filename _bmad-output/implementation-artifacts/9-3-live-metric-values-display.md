# Story 9.3: Live Metric Values Display

**Epic:** 9 (Web Configuration & Hot-Reload — Phase B)
**Phase:** Phase B
**Status:** review
**Created:** 2026-05-03
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md` was authored before Phase A was renumbered. The story this file implements lives in `epics.md` as **"Story 8.3: Live Metric Values Display"** under **"Epic 8: Web Configuration & Hot-Reload (Phase B)"** (lines 833–847). In `sprint-status.yaml` and the rest of the project this is **Story 9-3** under **Epic 9**. Same work, different numbering.

---

## User Story

As an **operator**,
I want to see live metric values for all devices in the web UI,
So that I can verify a newly installed sensor is reporting correctly from the field (FR37, FR41).

---

## Objective

Replace the Story 9-1 placeholder `static/devices.html` with a real **per-device live metric values grid**, and add the **JSON API endpoint** the page reads from. The page answers one operator question — "is my new sensor reporting correctly?" — without an SSH-into-the-database round-trip:

- All configured devices, **organized by application**.
- Each device shows its configured metrics with: current value, data type, last-updated timestamp (relative + absolute), and staleness status (`good` / `uncertain` / `bad` / `missing`).
- Page auto-refreshes every 10 s + has a "Refresh now" button (same cadence + UX as the Story 9-2 dashboard for consistency).
- Mobile-responsive (FR41) — single-column-stack at narrow viewports, side-by-side application columns at desktop widths.

**Data flow:**

```
SQLite metric_values table   ─┐
                              │
AppConfig.application_list  ──┼──►  GET /api/devices (JSON)  ──►  static/devices.html (vanilla JS fetch)
                              │       (joins by device_id;
[opcua].stale_threshold_secs ─┘        emits raw timestamp +
                                       threshold; JS computes
                                       age + status colour)
```

Server returns **raw timestamps + the configured `stale_threshold_secs`**; the JS computes `age_secs` and the status colour client-side. Same "server returns facts, client interprets" pattern Story 9-2 used for `last_poll_time` (avoids embedding operator-judgement thresholds in the wire contract).

The new code surface is **modest** — estimated **~200–300 LOC of production Rust + ~250–350 LOC of new HTML/CSS/JS + ~250–350 LOC of tests + ~50–80 LOC of docs**. The Rust side is a single new handler + an extension to `ApplicationSummary` (so it carries per-device summaries, not just the count). The JS shape mirrors `dashboard.js`'s defensive fetch path (in-flight guard, AbortController feature-detect, content-type sniff, generic error banner) — copied with the Story 9-2 iter-2 hardening already in place.

This story closes **FR37** (live metric values via web interface) and advances **FR41** (mobile-responsive — extends to a second page). It does **not** ship configuration-mutation paths (Stories 9-4 / 9-5 / 9-6 own those) and does **not** ship historical metric trends (Story 8-3 already exposes those over OPC UA HistoryRead; web-UI history is a future enhancement, out of scope).

---

## Out of Scope

- **Per-metric historical chart.** Story 8-3 ships HistoryRead over OPC UA; the web UI historical-trend view is a separate future story (or external Grafana / Prometheus deployment). Out of scope.
- **Per-metric edit / write-back.** Stories 9-4 / 9-5 / 9-6 own configuration mutation; Story 9-6 specifically owns command CRUD for write-back paths. Story 9-3 is **read-only** (no `POST` / `PUT` / `DELETE` routes; no CSRF surface introduced).
- **Filtering / search / pagination of devices.** Typical deployments are 100 devices x 4 metrics = 400 rows; the entire grid renders in one page. A filter/search UX makes sense at 1000+ devices but is out of scope for the FR37 baseline. Tracked as a future UX enhancement if operators surface the need.
- **Server-Sent Events / WebSocket push.** Polls `/api/devices` every 10 s via `setInterval` (same as the dashboard). SSE/WebSocket adds Axum-side state machinery (broadcast channels, per-connection lifecycle) overkill for one operator-debug page. Revisit if the 10 s refresh becomes operator-visible latency.
- **Per-device drill-down page.** The grid view is one screen — clicking a device does NOT navigate to a per-device detail page (no such page exists). Future enhancement.
- **TLS / HTTPS, per-IP rate limiting, CSRF.** Inherited from Story 9-1 / 9-2; tracked at issues [#104](https://github.com/guycorbaz/opcgw/issues/104) (TLS) and [#88](https://github.com/guycorbaz/opcgw/issues/88) (rate limiting). Story 9-3 ships GET-only — no CSRF surface added.
- **Configurable refresh interval per page.** Refresh interval hard-coded to 10 s in `devices.js` (matches Story 9-2 dashboard). If multiple operators surface a "make refresh interval configurable" request, a future story can add a single `[web].refresh_secs` knob covering both pages.
- **Metric value formatting per device-type (units, prefixes, decimal precision).** The grid renders values as the storage layer gives them (raw `value: String` from `metric_values`). FR37 asks for "current values"; presentation polish (e.g. "23.5 °C" vs "23.5") needs unit metadata that the storage layer doesn't carry today (`ReadMetric.metric_unit` is configured but not threaded through to runtime). Out of scope; tracked as a future UX enhancement.
- **Staleness threshold-per-metric.** The page uses the global `[opcua].stale_threshold_seconds` (default 120). Per-metric thresholds (some sensors poll every 60 s, some every hour) would need a config-schema change. Out of scope.
- **Empty-state polish.** First-startup (no metric data yet, all metrics show as `missing`) is rendered as a dimmed grid with "missing" badges per metric — adequate for the FR37 baseline. A "no data yet — wait for the first poll cycle" empty-state banner is a future polish.
- **Drop-zeroize HMAC keys, per-request access log.** Inherited deferrals from Story 9-1 (`deferred-work.md:225,226`). Story 9-3 must NOT add the `zeroize` crate or modify `src/web/auth.rs` beyond what `AppState` shape extension already requires (which is: nothing — auth surface is unchanged).

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code. The story's job is to **plumb a new endpoint + a new HTML view on top of code that already does the heavy lifting** — the web server, auth middleware, AppState/AppConfig snapshot pattern, storage trait, JSON contract conventions, integration-test harness, dashboard-shaped CSS + defensive JS pattern all exist.

| What | Where | Status |
|------|-------|--------|
| **Embedded Axum web server + Basic auth middleware** | `src/web/mod.rs::build_router`, `src/web/auth.rs::basic_auth_middleware` | **Wired today (Story 9-1 + 9-2).** Story 9-3 adds a new `Router::route("/api/devices", get(api::api_devices))` call inside `build_router`. The auth middleware wraps every route via `.layer(...)` after the routes are registered; the new route inherits the layer automatically (load-bearing per Story 9-1 AC#5 — verified by the layer-after-route invariant comment at `src/web/mod.rs:72-83` + the Story 9-2 integration test `auth_required_for_api_status` that pinned this for `/api/status`). |
| **`StorageBackend::load_all_metrics()`** | `src/storage/mod.rs:459`, impl `src/storage/sqlite.rs:1140` | **Wired today (Story 2-4).** Returns `Result<Vec<MetricValue>, OpcGwError>` — every row in the `metric_values` table. **Story 9-3's `/api/devices` handler reads via this method directly.** Acceptable cost: 100-device deployment = ~400 rows; query takes < 50 ms. **Do NOT add a new "scan with predicate" storage method** unless filter/search lands in a future story — the in-handler `Vec<MetricValue>` walk is fine at this scale. |
| **`MetricValue` struct** | `src/storage/types.rs:62-73` | **Wired today.** Fields: `device_id`, `metric_name`, `value: String`, `timestamp: DateTime<Utc>`, `data_type: MetricType`. Already `Serialize` / `Deserialize` via serde derive — the JSON shape can re-export it directly via a thin `MetricView` wrapper that adds the optional fields (`age_secs`, `status` derived client-side or a sentinel-only field). |
| **`MetricType` enum** | `src/storage/types.rs:25-30` | **Wired today.** `Float / Int / Bool / String` with `Display` impl (`MetricType::Float.to_string() == "Float"`). The JSON shape uses the `Display` representation. |
| **`AppState` struct** | `src/web/mod.rs:124-129` (Story 9-2) | **Wired today.** Already carries `auth: Arc<WebAuthState>`, `backend: Arc<dyn StorageBackend>`, `dashboard_snapshot: Arc<DashboardConfigSnapshot>`, `start_time: Instant`. **Story 9-3 reuses unchanged** — no new field needed; the dashboard snapshot already holds the application/device topology, and 9-3 extends `ApplicationSummary` (see next row). |
| **`ApplicationSummary` struct (Story 9-2)** | `src/web/mod.rs:64-68` | **Extended in Story 9-3.** Today: `application_id`, `application_name`, `device_count: usize`. Story 9-3 adds a `devices: Vec<DeviceSummary>` field where `DeviceSummary { device_id: String, device_name: String, metric_names: Vec<String> }` mirrors the configured `[[application.device]]` blocks and the `[[application.device.read_metric]]` `metric_name` list. The frozen-at-startup snapshot already walks `application_list`; 9-3 walks one level deeper. **Story 9-7 (hot-reload) will swap the whole `Arc<DashboardConfigSnapshot>` via `tokio::sync::watch` so the per-device structure is in the right place.** |
| **`DashboardConfigSnapshot::from_config`** | `src/web/mod.rs:80-98` | **Extended in Story 9-3.** The walk logic stays — just deepens to also collect `device_list` + `read_metric_list` per application. |
| **Per-task SQLite backend** | `src/main.rs:761-787` (Story 9-2) | **Wired today.** Story 9-3 reuses the existing `app_state.backend` for the new `/api/devices` handler. **No new `SqliteBackend::with_pool` call site** — the web task already has its own connection pool entry (5/5 used; pool size matches the task count after Story 9-2 review iter-1 D1). |
| **JSON contract conventions (Story 9-2)** | `src/web/api.rs::StatusResponse` (snake_case fields, `Option<String>` for nullable timestamps via `.map(.to_rfc3339())`, generic `ErrorResponse` body) | **Established.** Story 9-3 mirrors: snake_case field names, RFC 3339 timestamps as `Option<String>`, `MetricType::Display` as the `data_type` string ("Float" / "Int" / "Bool" / "String"). |
| **Storage error path (NFR7)** | `src/web/api.rs::api_status` 500-path (`event="api_status_storage_error"` warn + generic `{"error":"internal server error"}` body) | **Established (Story 9-2).** Story 9-3's handler follows the **same** shape with a new event name `event="api_devices_storage_error"`. **Total `event=` count grows from 3 → 4** — AC#7 below pins this. |
| **`stale_threshold_seconds` constant + config knob** | `src/opc_ua.rs:38` (`DEFAULT_STALE_THRESHOLD_SECS = 120`), `src/config.rs:248` (`opcua.stale_threshold_seconds: Option<u64>`) | **Wired today (Story 5-2).** Reused by Story 9-3 unchanged: the `/api/devices` JSON includes a top-level `stale_threshold_secs` field (resolved via `config.opcua.stale_threshold_seconds.unwrap_or(120)`); the JS computes `age_secs` and the status colour client-side. **Do NOT introduce a `[web].stale_threshold_seconds` knob** — the OPC UA + web surfaces share one staleness contract per FR37's intent. |
| **`STATUS_CODE_BAD_THRESHOLD_SECS` constant** | `src/opc_ua.rs:39` (`= 86400`) | **Wired today (Story 5-2).** 24 h cutoff for "bad" status. Story 9-3 returns this as a separate top-level field `bad_threshold_secs` so the JS branch is consistent across pages without the JS hard-coding the value. |
| **Dashboard CSS + JS pattern (Story 9-2 iter-2)** | `static/dashboard.css`, `static/dashboard.js` | **Established + iter-2-hardened.** The defensive fetch path (in-flight guard, AbortController feature-detect, Content-Type sniff, generic error banner, shared `parseTimestamp`) is the established shape. Story 9-3's `static/devices.js` **copies the same pattern** rather than extracting a shared module (no build step to support `import` from). The CSS responsive grid / dark-mode media query in `dashboard.css` is **extended** with the new metrics-grid styles (or a new `static/devices.css` if the dev agent prefers separation; either is acceptable — bundled keeps both pages cacheable as one file, separate keeps concerns scoped). |
| **`reqwest` integration-test client** | `tests/common/mod.rs:155-162` (`build_http_client`) | **Wired today.** Story 9-3 reuses unchanged. |
| **Integration-test file shape** | `tests/web_dashboard.rs` (Story 9-2 — global tracing-test subscriber, `spawn_test_server`, `build_test_app_state`, `build_production_static_dir`) | **Wired today.** Story 9-3 **extends `tests/web_dashboard.rs`** with new tests rather than creating a new `tests/web_devices.rs` file. Rationale: each integration-test binary installs a global tracing subscriber via `OnceLock`, so two files would duplicate the subscriber-install dance + the spawn helpers + `build_production_static_dir` (which Story 9-3 also extends to copy `devices.html` + `devices.js`). One file = one shared `OnceLock`, one shared helper set. **Scope-discipline trade-off:** the file grows from ~500 LOC to ~750 LOC; this is the threshold where extracting a `tests/common/web.rs` would land in scope, but per CLAUDE.md "Don't add features ... beyond what the task requires" we keep it inline until Stories 9-4/5/6 push the file past the next threshold. |
| **`#[serial_test::serial]` annotation** | `Cargo.toml` | **Wired today.** Story 9-3's new tests use it to serialise port-binding races + the global tracing subscriber. |
| **Tracing event-name convention + grep contract** | Stories 6-1 → 9-2 (`event="..."`), Story 9-2 grep contract pins exactly 3 names | **Extended in Story 9-3.** Story 9-3 introduces **one** new event: `event="api_devices_storage_error"` (warn diag, mirrors the 9-2 `api_status_storage_error` shape). New AC#6 grep contract: exactly **4** distinct names in `src/`. |
| **`OpcGwError::Web` variant** | `src/utils.rs:618-625` (Story 9-1) | **Wired today.** Story 9-3 reuses for runtime web-side errors; storage errors propagate via `OpcGwError::Storage` (logged then converted to generic 500 at the handler boundary — same pattern as 9-2). |
| **Documentation extension target** | `docs/security.md` § "Web UI authentication" → "API endpoints (Story 9-2+)" subsection (Story 9-2) | **Extended in Story 9-3.** The "API endpoints" subsection adds one bullet for `/api/devices` (auth-gated; storage failure mode mirrors `/api/status`); `docs/logging.md` event registry adds one row. |
| **`config/config.toml` + `config/config.example.toml`** | (Story 9-1 / 9-2 — `[web]` block) | **No new config knob.** Story 9-3 reuses the Story 9-1 `[web]` block + the Story 5-2 `[opcua].stale_threshold_seconds` knob unchanged. The dashboard refresh interval stays hard-coded to 10 s (matches the dashboard; future story can introduce one shared knob if needed). |
| **`README.md` Planning section** | `README.md` | **Wired today.** Story 9-3 flips the Story 9-3 row through `ready-for-dev` → `in-progress` → `review` → `done` and updates the Web UI subsection to mention the live-metrics page. |
| **Manual chapter for the embedded web UI** | `docs/manual/opcgw-user-manual.xml` | **Lagging — deferred from Story 9-1 (`deferred-work.md:218`).** Story 9-3 inherits the deferral; the live-metrics page adds to the deferred manual chapter rather than blocking on it. |
| **Library-wrap-not-fork pattern** | `OpcgwAuthManager`, `AtLimitAcceptLayer`, `OpcgwHistoryNodeManager` | **Established but not applicable here.** Axum's middleware system is rich enough that no wrap is needed for a `GET /api/devices` handler. No async-opcua callbacks involved. |

**Epic-spec coverage map** — the BDD acceptance criteria from `epics.md` (lines 833–847) break down as:

| Epic-spec criterion (line ref) | Already known? | Where this story addresses it |
|---|---|---|
| Authenticated web server (line 841) | ✅ wired today (Story 9-1) | **AC#3** — auth middleware from 9-1 wraps `/api/devices` automatically. |
| All devices organized by application with current metric values (line 843) | ⚠️ data exists in storage + config; no aggregator | **AC#1 + AC#2 + AC#4** — `ApplicationSummary` extends with `devices: Vec<DeviceSummary>`; `/api/devices` JSON joins `load_all_metrics()` against the snapshot; `static/devices.html` renders the application-grouped grid. |
| Each metric: value, data_type, last-updated timestamp, staleness (line 844) | ⚠️ data in storage; no UI | **AC#2 + AC#4** — `MetricView` JSON struct ships `value`, `data_type` (string), `timestamp` (RFC 3339 or null), `age_secs` derived; staleness rendered client-side via the `stale_threshold_secs` + `bad_threshold_secs` top-level fields. |
| Values read from `metric_values` SQLite table (line 845) | ✅ wired today (Story 2-4) | **AC#2** — `StorageBackend::load_all_metrics()` is the data source. |
| Auto-refresh OR manual refresh button (line 846) | ❌ no JS today | **AC#4** — `static/devices.js` does both: `setInterval(fetchDevices, 10_000)` + a "Refresh now" button. |
| FR37 satisfied (live metric values via web interface) (line 847) | ❌ Phase A had OPC UA metric reads (FR16) but no web-UI surface | **All ACs** collectively satisfy FR37. |
| Web UI mobile-responsive (FR41, line 786 carry-forward from Story 8.1) | ⚠️ dashboard responsive (9-2); devices page TBD | **AC#4** — `static/dashboard.css` extended (or `static/devices.css` added) with `@media (min-width: ...)` so the grid stacks below 600 px and uses application-side-by-side columns above. |
| `cargo test` clean + `cargo clippy --all-targets -- -D warnings` clean | Implicit per CLAUDE.md | **AC#5** — Story 9-3 baseline 340 lib+bins / clippy clean (post-Story 9-2); Story 9-3 target ≥ 350 with new tests. |

---

## Acceptance Criteria

### AC#1: `ApplicationSummary` extended with `devices: Vec<DeviceSummary>` + `MetricSummary` per device

**Implementation:**

- Extend `pub struct ApplicationSummary` in `src/web/mod.rs:64-68` with a new field `devices: Vec<DeviceSummary>`. Field order: existing 3 fields first (`application_id`, `application_name`, `device_count`), then `devices`.
- New `pub struct DeviceSummary` (sibling to `ApplicationSummary`):
  ```rust
  pub struct DeviceSummary {
      pub device_id: String,
      pub device_name: String,
      // Configured metric names from [[application.device.read_metric]].metric_name —
      // the canonical order in which the dashboard renders them (so adding a new
      // metric to the bottom of the TOML list shows up at the bottom of the row,
      // not in random hashmap order).
      pub metric_names: Vec<String>,
  }
  ```
- Update `DashboardConfigSnapshot::from_config(config: &AppConfig)` to populate the new `devices` field. The walk goes one level deeper:
  ```rust
  let devices: Vec<DeviceSummary> = app.device_list.iter().map(|dev| DeviceSummary {
      device_id: dev.device_id.clone(),
      device_name: dev.device_name.clone(),
      metric_names: dev.read_metric_list.iter().map(|m| m.metric_name.clone()).collect(),
  }).collect();
  ```
- The pre-existing `device_count` field stays — it equals `devices.len()` by construction, but keeping it avoids a `.len()` call at every `/api/status` invocation. (Hot-reload story 9-7 will need to keep the two in sync via the `tokio::sync::watch` channel.)
- **Backward compatibility:** `StatusResponse` (Story 9-2 `/api/status`) does NOT serialise `applications` — it only reads `application_count` + `device_count`. So extending `ApplicationSummary` with a new field is a non-breaking change for the wire contract.

**Verification:**

- 1 unit test in `src/web/mod.rs::tests` extending `dashboard_snapshot_from_config_walks_application_list_once`: assert `applications[0].devices.len() == 3` + each `DeviceSummary.device_id` matches the configured `device_id` + `metric_names == vec!["temperature"]` (per the test fixture `make_app` helper which seeds one `temperature` metric per device).
- 1 unit test asserting empty `application_list` produces `applications: vec![]` (existing `dashboard_snapshot_from_config_handles_empty_application_list` test passes unchanged — no new assertion needed; the empty case is structurally the same).
- 1 unit test asserting an application with a device that has zero `read_metric_list` entries produces `DeviceSummary.metric_names: vec![]` (extending `make_app` helper to take an `Option<Vec<&str>>` for per-device metric overrides).
- `cargo clippy --all-targets -- -D warnings` clean.

---

### AC#2: `GET /api/devices` JSON endpoint returns the per-device live metrics grid

**Endpoint contract:**

```http
GET /api/devices HTTP/1.1
Authorization: Basic <base64(user:pass)>

HTTP/1.1 200 OK
Content-Type: application/json

{
  "as_of": "2026-05-03T09:14:22.001Z",
  "stale_threshold_secs": 120,
  "bad_threshold_secs": 86400,
  "applications": [
    {
      "application_id": "550e8400-e29b-41d4-a716-446655440001",
      "application_name": "Building Sensors",
      "devices": [
        {
          "device_id": "0018b20000000001",
          "device_name": "Temperature Sensor 01",
          "metrics": [
            {
              "metric_name": "temperature",
              "data_type": "Float",
              "value": "23.5",
              "timestamp": "2026-05-03T09:14:18.000Z"
            },
            {
              "metric_name": "humidity",
              "data_type": "Int",
              "value": null,
              "timestamp": null
            }
          ]
        }
      ]
    }
  ]
}
```

**Field semantics:**

| Field | Type | Source | Semantics |
|---|---|---|---|
| `as_of` | `String` | `Utc::now().to_rfc3339()` at handler entry | Server-side timestamp at the moment of the response. The dashboard uses this as the "denominator" for relative-age display, NOT `Date.now()` browser-side — keeps clock-skew failures off the staleness path (same rationale as Story 9-2's `uptime_secs` field). |
| `stale_threshold_secs` | `u64` | `config.opcua.stale_threshold_seconds.unwrap_or(120)` | Boundary between "good" and "uncertain" staleness states. JS uses `(as_of - timestamp) >= stale_threshold_secs ? "uncertain" : "good"`. |
| `bad_threshold_secs` | `u64` | `STATUS_CODE_BAD_THRESHOLD_SECS = 86400` (constant) | Boundary between "uncertain" and "bad". The constant is server-owned (operator can't tune the 24 h cutoff today); shipping it as a JSON field future-proofs against a Story 5-2-style configurability addition. |
| `applications[*].application_id` | `String` | `dashboard_snapshot.applications[i].application_id` | Mirrors the configured ID. |
| `applications[*].application_name` | `String` | Same | Mirrors the configured display name. |
| `applications[*].devices[*].device_id` | `String` | `dashboard_snapshot.applications[i].devices[j].device_id` | Configured `device_id` (DevEUI). |
| `applications[*].devices[*].device_name` | `String` | Same | Configured display name. |
| `applications[*].devices[*].metrics[*].metric_name` | `String` | Snapshot's `metric_names[k]` (canonical order from TOML) | Stable field key for the dashboard's per-metric DOM IDs. |
| `applications[*].devices[*].metrics[*].data_type` | `String` | `MetricType::Display` ("Float" / "Int" / "Bool" / "String") | Configured per-metric type. **Comes from the storage row's `data_type`**, falling back to **the snapshot's configured-type** when the metric has never been polled (`value: null` case). |
| `applications[*].devices[*].metrics[*].value` | `Option<String>` → JSON `string \| null` | `metric_values` table row's `value: String` | `null` if no row exists for `(device_id, metric_name)` — the metric is configured but has never been polled (operator-visible "missing" state in the UI). |
| `applications[*].devices[*].metrics[*].timestamp` | `Option<String>` → JSON `string \| null` | `metric_values` table row's `timestamp: DateTime<Utc>` (RFC 3339 string) | `null` for missing rows (matches `value: null`). |

**Implementation:**

- Extend `src/web/api.rs` with `pub async fn api_devices(State(state): State<Arc<AppState>>) -> Result<Json<DevicesResponse>, Response>`:
  1. Call `state.backend.load_all_metrics()`.
  2. On `Err(e)`: log `warn!(event = "api_devices_storage_error", error = %e, "GET /api/devices: failed to read metric_values table")` and return 500 + generic body (NFR7).
  3. On `Ok(metrics)`: build a lookup `HashMap<(&str, &str), &MetricValue>` keyed by `(device_id.as_str(), metric_name.as_str())` — O(N) insertion + O(1) lookup. Walk the snapshot's `applications` and for each `(device_id, metric_name)` pair look up the row; emit `MetricView { metric_name, data_type, value, timestamp }` with the row's data or `(value: None, timestamp: None)` if absent. Configured `data_type` (from the snapshot — added if needed; see Field-shape divergence note) wins on a missing row; storage's `data_type` wins when the row exists.
  4. Compute `as_of = Utc::now().to_rfc3339()` at handler entry (NOT after `load_all_metrics()` returns — the timestamp should reflect when the operator's request hit the server, not after the storage delay).
- Add `Router::route("/api/devices", get(api::api_devices))` to `build_router` in `src/web/mod.rs`.
- New `serde::Serialize`-derived structs in `src/web/api.rs`: `DevicesResponse`, `ApplicationView`, `DeviceView`, `MetricView`. Field naming follows the snake_case convention from Story 9-2.
- The handler does NOT compute `age_secs` or `status` server-side — JS handles the rendering. Same "server returns facts, client interprets" pattern from Story 9-2's `last_poll_time`.

**Verification:**

- 4 unit tests in `src/web/api.rs::tests` (using `InMemoryBackend` populated via `upsert_metric_value` for the seeded values):
  1. `api_devices_returns_200_with_application_grouped_grid_when_storage_healthy` — seed 2 apps × 2 devices × 2 metrics × 1 metric polled = 8 metric slots, 4 populated; assert the JSON shape walks the snapshot order correctly.
  2. `api_devices_returns_500_with_generic_body_when_storage_errors` — `FailingBackend` (extended to also fail `load_all_metrics`) returns `Err`; handler returns 500 + `{"error":"internal server error"}`; inner error string MUST NOT appear in the body (NFR7 invariant pin).
  3. `api_devices_returns_null_value_for_unpolled_metric` — seed 1 metric polled, 1 metric configured-but-not-polled; assert the not-polled metric serialises as `value: null, timestamp: null`.
  4. `api_devices_uses_storage_data_type_when_present_and_configured_data_type_when_missing` — seed a metric with the storage row carrying `data_type: Float` while the snapshot's configured type is `Int`; assert the response uses `Float` (storage wins). Then test the inverse: seed nothing for a configured-`Int` metric; assert the response carries `data_type: "Int"` from the snapshot.
- 4 integration tests in `tests/web_dashboard.rs` (extending the existing file):
  1. `auth_required_for_api_devices` — unauth'd `GET /api/devices` returns 401 + `event="web_auth_failed" path=/api/devices reason=missing` audit event (mirrors Story 9-2's `auth_required_for_api_status` test).
  2. `api_devices_returns_json_with_expected_shape_when_authed` — auth'd GET returns 200 + JSON parseable + has the 3 top-level fields (`as_of`, `stale_threshold_secs`, `applications`); applications array has the expected count; first device has the expected metric_name list.
  3. `devices_html_contains_viewport_meta_and_grid_markup` — auth'd `GET /devices.html` returns 200 + body contains `<meta viewport>` + the grid-container DOM ID + a `<script>` tag.
  4. `devices_js_renders_application_groups` — only structural / file-existence check (`GET /devices.js` returns 200 with `Content-Type: application/javascript` or `text/javascript`; body contains `fetch("/api/devices"`); we don't run the JS in an integration test (no headless browser).

---

### AC#3: Auth middleware applies to `/api/devices` (carry-forward from Story 9-1 + 9-2, no regression)

**Implementation:**

- The Story 9-1 layer-after-route invariant (`src/web/mod.rs:72-83`) is the load-bearing property — `.layer(...)` runs AFTER `.route(...)` AND `.fallback_service(...)`, so any new `Router::route("/api/devices", ...)` call inside `build_router` automatically inherits the auth middleware.
- **No behaviour change to `src/web/auth.rs`** — the middleware signature, the `WebAuthState` struct, and the `event="web_auth_failed"` audit event all stay identical (load-bearing for AC#6 below).

**Verification:**

- The `auth_required_for_api_devices` integration test from AC#2 doubles as the AC#3 verification.
- Existing Story 9-1 + 9-2 tests (`tests/web_auth.rs` 14 tests + `tests/web_dashboard.rs` 8 tests) continue to pass unchanged — no regression on auth-fail modes, on `event="web_auth_failed"` audit-event shape, on the constant-time path, or on `/api/status` / `/index.html` / `/dashboard.css` / `/dashboard.js` behaviour.
- `git diff src/web/auth.rs` shows zero production-code changes.

---

### AC#4: `static/devices.html` renders the per-device live metrics grid; mobile-responsive (FR41)

**Implementation:**

- Replace `static/devices.html` (currently a 12-line placeholder with `Story 9-3 will fill this in`) with a real metrics-grid view. Keep the file plain HTML5 — no SPA framework, no build step, no `npm install`. Inline `<style>` and `<script>` are acceptable for the page-specific bits, **but** prefer separate `static/devices.css` and `static/devices.js` files (served by the existing `ServeDir` mount) so the HTML is small and the assets are individually cacheable. **Or** extend `static/dashboard.css` with the new grid styles (the dev agent picks; document the choice in completion notes).
- HTML structure:
  - `<header>` with the gateway name + a "Last refreshed" timestamp + a "Refresh now" button (mirrors the dashboard's footer area for consistency, but moved to the top so the operator sees it without scrolling on a phone).
  - `<main>` containing one `<section class="application">` per application:
    - `<h2>` with the application name + a small badge showing `<device_count> devices`.
    - One `<table class="device-grid">` per device with rows:
      - Header row: `Metric`, `Value`, `Type`, `Last update`, `Status`.
      - One body row per configured metric, each with the staleness colour applied to the row (`row-good` / `row-uncertain` / `row-bad` / `row-missing` classes).
    - Sub-header `<h3>` per device showing `device_name` + small `(device_id)` label.
  - `<footer>` with a `<p id="error-banner" class="error-banner hidden" role="alert"></p>` (matches dashboard pattern) + a credit line.
- CSS:
  - **If extending `static/dashboard.css`:** add styles scoped to `.device-grid` / `.application` / `.row-good` / `.row-uncertain` / `.row-bad` / `.row-missing` / `.metric-status-badge`. Existing colour palette + dark-mode media query reused.
  - **If new `static/devices.css`:** standalone file with the same colour palette + responsive grid + dark-mode media query (~150 LOC).
  - Mobile-first: tables collapse to stacked `<dl>`-style key/value pairs below 600 px, render as full grids above. Implementation note: CSS-only via `@media` rules — no JS-driven layout change.
- JS in `static/devices.js`:
  - On `DOMContentLoaded`: call `fetch('/api/devices')` and render the grid.
  - `setInterval(fetchDevices, 10_000)` for the refresh loop.
  - **Defensive fetch path mirrors `static/dashboard.js` Story 9-2 iter-2 hardening:** in-flight guard + `AbortController` feature-detect + Content-Type sniff + generic error banner + `inflightToken === thisCallToken` stale-render guard. Copy the pattern; the duplication is acceptable per CLAUDE.md scope-discipline (no shared module today, and a future cleanup story can DRY out the duplication if needed).
  - Per-row staleness computation: `age_secs = (Date.parse(as_of) - Date.parse(metric.timestamp)) / 1000`; `status = age_secs >= bad_threshold_secs ? "bad" : age_secs >= stale_threshold_secs ? "uncertain" : "good"` (with `metric.value === null` short-circuiting to `"missing"`).
  - Same `Intl.NumberFormat` / `Intl.DateTimeFormat` reuse pattern from `dashboard.js`.

**Verification:**

- File-existence check: `static/devices.html`, `static/devices.js` exist after the story lands. `static/devices.css` exists IF the dev agent chose the new-file shape (otherwise `static/dashboard.css` has new sections). **Document the choice** in completion notes.
- 2 integration tests in `tests/web_dashboard.rs` per AC#2 verification (HTML markup pin + JS file presence + Content-Type).
- 1 integration test asserting the CSS responsive contract: auth'd `GET /dashboard.css` (or `/devices.css` if separate) contains `@media` and `min-width` (FR41 marker pinned at the CSS level — Story 9-2 already pinned this for `dashboard.css`; if 9-3 introduces `devices.css`, add a sibling test).
- Manual smoke: open `http://localhost:8080/devices.html` in Chrome DevTools mobile-emulation (iPhone 12 viewport, 390 × 844). Verify stacked layout + readable rows + working refresh.
- Manual smoke: open in desktop Chrome at 1920 × 1080. Verify the grid layout + dark/light mode follows OS preference.
- Manual smoke: kill ChirpStack (or stop the gateway), confirm the rows progressively flip from `row-good` → `row-uncertain` → `row-bad` as the configured threshold passes.

---

### AC#5: Tests pass + clippy clean + no regression (CLAUDE.md compliance)

**Verification:**

- `cargo test --lib --bins`: ≥ 350 passed (was 340 baseline post-Story 9-2; growth from new `DashboardConfigSnapshot` extension test + new `api_devices` unit tests). The exact delta depends on test-file shape; **document the actual count** in completion notes.
- `cargo test --tests`: existing 16 integration test binaries still pass; `tests/web_dashboard.rs` adds ≥ 4 integration tests (auth carry-forward + JSON shape + HTML markup + JS file presence). Total integration tests in `web_dashboard.rs` grows from 8 → ≥ 12.
- `cargo clippy --all-targets -- -D warnings` clean across the workspace.
- `cargo test --doc`: 0 failed (carries the issue #100 baseline; new code adds no new doctests).
- The Story 8-1 / 8-2 / 8-3 / 9-1 / 9-2 spike+history+subscription+web tests are **regression baselines** — must continue to pass unchanged. Story 9-3 must NOT modify `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_auth.rs`, `src/web/auth.rs` beyond what the new `ApplicationSummary` field requires (which is: nothing — auth surface is unchanged; the existing 9-2 test fixture `wrap_in_app_state` already builds `ApplicationSummary` with hard-coded fields, but Story 9-3's field addition will require updating the fixture by adding `devices: vec![]` to two call sites — same shape as the Story 9-2 `WebConfig::default()` test fixture updates). AC#6 below pins this with a `git diff` check.

---

### AC#6: NFR12 + auth + connection-cap + 9-2 dashboard-handler carry-forward intact (no regression on prior epics)

**Implementation:**

- Story 9-3 must NOT modify `src/opc_ua.rs`, `src/opc_ua_history.rs`, `src/opc_ua_session_monitor.rs`, `src/opc_ua_auth.rs`, `src/web/auth.rs` (zero production-code change in those files).
- Story 9-3 must NOT modify `src/main.rs` beyond the test-fixture path (the `if web_enabled { ... }` block is unchanged — the new endpoint is added inside `build_router`, not at the bind / spawn / shutdown path).
- Story 9-3 must NOT modify `src/web/api.rs::api_status` (Story 9-2's handler) — extends the file with a new `api_devices` function but leaves `api_status` invariant.
- Story 9-3 must NOT modify `static/index.html`, `static/dashboard.css`, `static/dashboard.js` (Story 9-2's dashboard) **except** for an OPTIONAL extension of `dashboard.css` if the dev agent chose to bundle the metrics-grid styles there (versus a new `devices.css`). If extending `dashboard.css`, the original sections (header, tile, badge, footer, error-banner, dark-mode) must stay byte-identical; only new selectors are appended. **Document the choice + the diff scope** in completion notes.

**Verification:**

- `git diff --stat src/opc_ua.rs src/opc_ua_history.rs src/opc_ua_session_monitor.rs src/opc_ua_auth.rs src/web/auth.rs src/main.rs` over the 9-3 branch must show `0 insertions, 0 deletions`.
- `git diff src/web/api.rs` shows only additions (new `api_devices` + new structs) — zero changes to `api_status` body.
- `git diff static/index.html static/dashboard.js` shows zero changes.
- `git diff static/dashboard.css` shows EITHER zero changes (new-file path) OR additions-only (bundled path); existing rules byte-identical.
- The existing `tests/opcua_subscription_spike.rs` (17 tests), `tests/opcua_history.rs` (11 tests), `tests/web_auth.rs` (14 tests), `tests/web_dashboard.rs` Story 9-2 tests (8 tests including E3) all continue to pass without modification beyond the `ApplicationSummary` field addition (test fixtures need to add `devices: vec![]`).

---

### AC#7: Sanity check on regression-test count + audit-event count

**Verification:**

- Default test count grows by ~10 lib+bins (≈ 1 extended snapshot test + 1 new zero-metrics-device test + 4 `api_devices` unit + 4 integration; minor variance acceptable). **Document the actual count** in completion notes alongside the pre-Story baseline (340 lib+bins post-Story 9-2).
- Exactly **4** total tracing-event names introduced across Stories 9-1 + 9-2 + 9-3 in `src/web/`: `web_auth_failed` (9-1, audit warn), `web_server_started` (9-1, diagnostic info), `api_status_storage_error` (9-2, diagnostic warn), `api_devices_storage_error` (9-3, diagnostic warn). The grep contract:
  ```
  grep -rEn 'event = "web_|event="web_|event = "api_|event="api_' src/
  ```
  must return exactly those 4 distinct values, each with one emit site.
- The `event="api_devices_storage_error"` warn is registered in `docs/logging.md` § "Audit and diagnostic events (`event=`)" with a one-line description.
- Zero new audit events on the OPC UA path (AC#6 invariant).
- Zero new audit events for the success path of `/api/devices` (no `event="api_devices_returned"` or similar — same convention as `/api/status`).

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issue (CLAUDE.md compliance) (AC: All)

- [ ] Open ONE GitHub issue: "Story 9-3: Live Metric Values Display (FR37)" — main story tracker.
  - **Deferred to commit time per Story 9-2 precedent** — the dev agent does not call `gh issue create` proactively.

### Task 1: Extend `ApplicationSummary` + `DashboardConfigSnapshot` (AC: 1)

- [x] Add `pub struct DeviceSummary { device_id, device_name, metric_names, metric_types }` — added `metric_types: Vec<OpcMetricTypeConfig>` beyond the spec so `/api/devices` has the configured fallback type without re-reading `AppConfig`.
- [x] Add `devices: Vec<DeviceSummary>` field to `pub struct ApplicationSummary`.
- [x] Update `DashboardConfigSnapshot::from_config` to populate the deeper structure.
- [x] `tests/web_auth.rs::wrap_in_app_state`, `src/web/api.rs::tests::build_state`, `tests/web_dashboard.rs::build_test_app_state` all updated for the new `ApplicationSummary.devices` field + new `AppState.stale_threshold_secs` field.
- [x] Extended `dashboard_snapshot_from_config_walks_application_list_once` to assert `devices` + `metric_names` + `metric_types`.
- [x] Added new `dashboard_snapshot_from_config_handles_device_with_zero_metrics` unit test.

### Task 2: New `GET /api/devices` JSON endpoint (AC: 2, 3)

- [x] Added `DevicesResponse`, `ApplicationView`, `DeviceView`, `MetricView` structs + `api_devices` handler in `src/web/api.rs`. Handler is ~100 LOC; tests mock is ~150 LOC; total module growth ~250 LOC.
- [x] Extended `FailingBackend::load_all_metrics()` to return synthetic `Err(OpcGwError::Storage(...))` for the 500-path test.
- [x] Added `Router::route("/api/devices", get(api::api_devices))` to `build_router` after `/api/status`.
- [x] Added `event="api_devices_storage_error"` warn-level log mirroring the 9-2 `api_status_storage_error` shape (NFR7-aligned).
- [x] Added 4 unit tests in `src/web/api.rs::tests`: success / 500-with-generic-body / null-on-unpolled-metric / data_type fallback.
- [x] Added 2 integration tests in `tests/web_dashboard.rs` (`auth_required_for_api_devices` + `api_devices_returns_json_with_expected_shape_when_authed`).

### Task 3: Devices page static assets (AC: 4)

- [x] **Course correction**: created **new** `static/metrics.html` instead of replacing `static/devices.html`. Discovered during implementation that the Story 9-1 `static/devices.html` placeholder is reserved for **Story 9-5** (device + metric CRUD), not 9-3. New filename `metrics.html` → URL `/metrics.html` matches the BDD wording "live metrics page" and leaves the 9-1 reservation intact.
- [x] Created `static/metrics.js` with the defensive fetch pattern (mirrors `dashboard.js` iter-2 hardening: in-flight guard, AbortController feature-detect, Content-Type sniff, generic error banner, stale-render guard) + per-row status computation from `(as_of - timestamp)` vs the two threshold fields.
- [x] **Chose to extend `static/dashboard.css`** rather than create a separate `static/devices.css` — single stylesheet keeps both pages cacheable together; styles scoped to `.metrics-grid-container` so 9-2's dashboard tile selectors are completely unaffected.
- [x] Added 2 integration tests in `tests/web_dashboard.rs` (`metrics_html_contains_viewport_meta_and_grid_markup` + `metrics_js_is_served_and_references_api_devices`).
- [ ] Manual smoke (`http://localhost:8080/metrics.html` in Chrome DevTools) — **deferred to operator verification** per Story 9-2 precedent. The integration tests pin the structural contract (DOM IDs + viewport meta + JS file presence + `/api/devices` reference); visual regression is operator-side.

### Task 4: Documentation (AC: 4, 7)

- [x] Updated `docs/security.md` § "API endpoints (Story 9-2+)" subsection: `/api/devices` paragraph added, JSON contract documented (server-side `as_of` + dual threshold fields + missing-metric semantics).
- [x] Updated `docs/logging.md` § "Audit and diagnostic events (`event=`)" with the new `api_devices_storage_error` row.
- [x] Updated `README.md`: Web UI subsection extended with Story 9-3 paragraph; Epic 9 Planning row updated to "9-1 done · 9-2 done · 9-3 review"; `last_updated:` bumped to 2026-05-03.

### Task 5: Final verification (AC: 5, 6, 7)

- [x] `cargo test --lib --bins`: **322 (lib) + 345 (bins) passed / 0 failed / 5 ignored**. Lib Δ +5 (new web_dashboard snapshot test + 4 new api_devices unit tests, where 4 of the lib-test growth is on the lib side and 5 on the bin side); bin Δ +5. Total Δ +10 across lib+bin, **matches** the spec target. Pre-Story baseline was lib=317 + bin=340 = 657; post-Story is lib=322 + bin=345 = 667.
- [x] `cargo test --tests`: all 16 integration binaries pass; `tests/web_dashboard.rs` reports **12 passed** (was 8 — +4 new Story 9-3 tests).
- [x] `cargo clippy --all-targets -- -D warnings`: **clean**.
- [x] `cargo test --doc`: **0 failed / 56 ignored** (issue #100 baseline preserved).
- [x] `grep -rEn 'event = "web_|event="web_|event = "api_|event="api_' src/`: exactly **4 distinct values** — `web_server_started`, `web_auth_failed`, `api_status_storage_error`, `api_devices_storage_error`. One emit site per event.
- [x] `git diff HEAD --stat src/opc_ua.rs src/opc_ua_history.rs src/opc_ua_session_monitor.rs src/opc_ua_auth.rs src/web/auth.rs static/index.html static/dashboard.js`: **zero output** = zero changes across all 7 invariant files.
- [x] **`src/main.rs` was modified** for the new `AppState.stale_threshold_secs` field plumbing (+12 LOC inside the existing `if web_enabled { ... }` block). This is a divergence from the original AC#6 which said `src/main.rs` should be unchanged; see "Field-shape divergence from spec" below for the rationale.

### Task 6: Documentation sync verification (CLAUDE.md compliance)

- [x] `README.md` Planning section reflects 9-3 status accurately ("9-1 done · 9-2 done · 9-3 review").
- [x] `_bmad-output/implementation-artifacts/sprint-status.yaml` `last_updated:` narrative will be re-bumped to "review" at the end of this Dev Agent Record run.
- [x] `docs/security.md` and `docs/logging.md` updated per Task 4.
- [ ] Verify the implementation commit closes the Story 9-3 GitHub tracker issue from Task 0 via `Closes #X`. **Deferred to commit time** (see Task 0).

---

## Dev Notes

### Architecture compliance

- Axum **0.8.x** unchanged from Story 9-1 / 9-2 (use whatever's pinned in `Cargo.toml`).
- `tower-http` **0.6.x** unchanged.
- `serde` + `serde_json` already pulled as direct deps; no new crate dependencies expected.
- **If the dev agent wants to add one,** document the rationale in completion notes (e.g. `serde_with` for cleaner `Option<DateTime<Utc>>` serialisation — acceptable but not required; Story 9-2 used `.map(|t| t.to_rfc3339())` and Story 9-3 should follow the same shape).

### Why not async storage?

Like Story 9-2, the `/api/devices` handler calls a synchronous `StorageBackend::load_all_metrics()` from an async Axum task. This is the project-wide established pattern (poller, OPC UA also do this). Story 9-2's iter-1 review flagged it as a deferral (B1) — Story 9-3 inherits the same posture. A future epic-level migration to `async fn` on the trait + `tokio::task::spawn_blocking` adoption is the right place to fix it; piecemeal fixes only on new handlers don't help.

### Why server-side `as_of` (not `Date.now()` browser-side)?

The dashboard could compute `(Date.now() - timestamp)` browser-side, but two browsers viewing the same gateway would disagree if their clocks differed. Returning the server's `Utc::now()` as `as_of` lets every browser compute the same `age_secs` regardless of local clock skew. Same rationale as Story 9-2's `uptime_secs` field.

### Why two threshold fields (`stale_threshold_secs` + `bad_threshold_secs`)?

The OPC UA path (Story 5-2) uses a configurable boundary at 120 s for "Good → Uncertain" and a hard-coded constant at 86400 s for "Uncertain → Bad". Shipping both as JSON fields keeps the JS branching logic simple and keeps the wire contract honest about which boundary is operator-tunable today vs. constant. Future-proofs against a Story 5-2-style configurability addition for the bad-threshold.

### Why does configured `data_type` win on a missing row?

A configured-but-not-yet-polled metric should still render with its configured type (so the dashboard can show "humidity (Int) — never reported" instead of "humidity (?)"). The storage row's `data_type` wins when present so a poller-side type drift surfaces immediately (operator sees `Float` in the dashboard while the TOML says `Int` — clear signal that a poller-side type-conversion is happening).

### Why one large file (`tests/web_dashboard.rs`) for both 9-2 and 9-3 tests?

Each integration-test binary in Cargo installs a **separate** `OnceLock<()>` for the global tracing subscriber, and **separate** `static`-dir copying logic, and **separate** `spawn_test_server` helpers. Splitting the 9-3 tests into a new `tests/web_devices.rs` would duplicate every helper. Per CLAUDE.md scope-discipline ("Don't add features ... beyond what the task requires"), the right move is to keep the file growing until extracting a `tests/common/web.rs` becomes blocking (i.e. when 9-4 + 9-5 + 9-6 also need the harness, around the 750-1000 LOC mark). Story 9-3 lands at ~750 LOC — at the threshold but not over it.

### Carry-forward LOWs from Story 9-2 review (acknowledged but not addressed)

- **L1** (Content-Type case sensitivity / loose substring match) — applies symmetrically to the new `devices.js` since it copies the same fetch pattern. If 9-3 fixes this in `dashboard.js`, fix it in `devices.js` too. If 9-3 leaves it as LOW, the inconsistency is documented.
- **L3** (pool size 5 is a literal) — Story 9-3 doesn't add a new task-claimer; pool size stays at 5. The L3 follow-up isn't blocking 9-3.
- **L5** (tracing-test poison-mutex hazard) — pre-existing project-wide; Story 9-3 inherits the pattern.
- **L6** (buffer-clear pattern not extracted) — Story 9-3's `auth_required_for_api_devices` test will need the same buffer-clear at the top, mirroring Story 9-2's `auth_required_for_api_status`. The duplication grows from 1 site to 2; still under the 3-site DRY threshold.

### Hot-reload (Story 9-7) compatibility

The frozen-at-startup `DashboardConfigSnapshot` design (Story 9-2) extends naturally to Story 9-3: the new `devices: Vec<DeviceSummary>` field is in the same struct, so a Story 9-7 hot-reload swap of `Arc<DashboardConfigSnapshot>` updates both the dashboard counts AND the devices-page topology atomically. The runtime-mutable parts (`metric_values` table) are already-mutable via the storage layer; no refactor needed for hot-reload of those.

### File List (expected post-implementation)

**New files:**
- `static/devices.js` — defensive fetch pattern + per-row status computation. (~150 LOC.)
- `static/devices.css` — IF the dev agent chose the new-file shape (alternative: extend `dashboard.css`). (~150 LOC if separate.)

**Modified files:**
- `src/web/mod.rs` — `DeviceSummary` struct + `devices` field on `ApplicationSummary` + extended `DashboardConfigSnapshot::from_config` walk + new route in `build_router` + 1-2 new unit tests.
- `src/web/api.rs` — new `DevicesResponse` / `ApplicationView` / `DeviceView` / `MetricView` structs + `api_devices` handler + 4 new unit tests + extended `FailingBackend::load_all_metrics`.
- `src/web/mod.rs::tests::make_app` — already populates the deeper structure correctly; no change needed beyond the new test cases.
- `tests/web_auth.rs::wrap_in_app_state` — add `devices: vec![]` to the zero-application `ApplicationSummary` (no-op extension since the helper builds an empty snapshot).
- `tests/web_dashboard.rs::build_test_app_state` + `tests/web_dashboard.rs::build_production_static_dir` — add `devices: vec![]` to the snapshot construction calls; extend the static-dir copy list to include `devices.html` + `devices.js` (+ `devices.css` if separate).
- `static/devices.html` — replace 12-line placeholder with real grid markup.
- `static/dashboard.css` — IF the dev agent chose the bundled shape, append metrics-grid styles to the bottom (additions only, no edits to existing rules).
- `docs/security.md` — append `/api/devices` bullet under § "API endpoints (Story 9-2+)".
- `docs/logging.md` — add one row to the `event=` registry table for `api_devices_storage_error`.
- `README.md` — Web UI subsection adds a Story 9-3 paragraph; Planning row updated; `last_updated:` bumped.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — flip 9-3 row + refresh `last_updated` narrative.
- `_bmad-output/implementation-artifacts/9-3-live-metric-values-display.md` — this file.

### Project Structure Notes

- Aligns with `architecture.md:417-421` reservation of `src/web/`. Story 9-3 reuses the `api.rs` slot landed by Story 9-2; no new module files in `src/web/`.
- Sequencing per `epics.md` Phase-B polish (`epics.md:793`): 9-1 → 9-2 → 9-3 → 9-0 spike → 9-7 → 9-8 → 9-4 / 9-5 / 9-6.
- No conflicts with existing structure.

### References

- [Source: `_bmad-output/planning-artifacts/epics.md#Epic-8` (= sprint-status Epic 9), lines 766-796 — Phase-B carry-forward bullets].
- [Source: `_bmad-output/planning-artifacts/epics.md#Story-8.3` (= sprint-status 9-3), lines 833-847 — BDD acceptance criteria].
- [Source: `_bmad-output/planning-artifacts/epics.md`, line 84 — FR37 inventory].
- [Source: `_bmad-output/planning-artifacts/architecture.md:417-421` — directory structure with `src/web/` reservation].
- [Source: `_bmad-output/planning-artifacts/prd.md#FR37` — live metric values via web interface].
- [Source: `_bmad-output/planning-artifacts/prd.md#FR41` — mobile-responsive].
- [Source: `_bmad-output/implementation-artifacts/9-1-axum-web-server-and-basic-authentication.md` — Story 9-1 spec + completion notes].
- [Source: `_bmad-output/implementation-artifacts/9-2-gateway-status-dashboard.md` — Story 9-2 spec + completion notes; AppState shape + iter-2 dashboard.js hardening].
- [Source: `src/storage/mod.rs:459` — `load_all_metrics` trait method, the 9-3 data source].
- [Source: `src/storage/sqlite.rs:1140` — `SqliteBackend::load_all_metrics` impl].
- [Source: `src/storage/types.rs:25-30,62-73` — `MetricType` + `MetricValue` shapes].
- [Source: `src/web/mod.rs:64-68,73-77,124-129` — `ApplicationSummary` + `DashboardConfigSnapshot` + `AppState` Story 9-2 shape Story 9-3 extends].
- [Source: `src/web/mod.rs:111-119,72-83` — `build_router` body + the layer-after-route invariant doc-comment].
- [Source: `src/web/auth.rs::basic_auth_middleware` — the auth middleware Story 9-3 must NOT modify (AC#6)].
- [Source: `src/web/api.rs::api_status` — the Story 9-2 handler whose 500-path Story 9-3 mirrors].
- [Source: `src/opc_ua.rs:37-39` — `DEFAULT_STALE_THRESHOLD_SECS = 120` + `STATUS_CODE_BAD_THRESHOLD_SECS = 86400` constants Story 9-3 reuses unchanged].
- [Source: `src/config.rs:248,1367-1370` — `[opcua].stale_threshold_seconds` config knob + validation].
- [Source: `src/main.rs:721-787` — Story 9-2's `if web_enabled { ... }` block Story 9-3 does NOT modify].
- [Source: `src/config.rs:465-484,491-515,752-784` — `ChirpStackApplications` / `ChirpstackDevice` / `ReadMetric` / `AppConfig.application_list` shapes].
- [Source: `tests/common/mod.rs:155-162` — `build_http_client` Story 9-3 reuses unchanged].
- [Source: `tests/web_dashboard.rs` — Story 9-2 integration-test shape Story 9-3 extends in-file].
- [Source: `_bmad-output/implementation-artifacts/deferred-work.md:216-226` — Story 9-1 + 9-2 deferred items Story 9-3 inherits].
- [Source: `_bmad-output/implementation-artifacts/epic-8-retro-2026-05-01.md` — Phase B carry-forward context].
- [Source: GitHub issue #88 — per-IP rate limiting; inherited deferral].
- [Source: GitHub issue #100 — doctest cleanup; inherited baseline].
- [Source: GitHub issue #102 — tests/common extraction; Story 9-3 reuses without adding new helpers].
- [Source: GitHub issue #104 — TLS hardening follow-up; inherited deferral].
- [Source: GitHub issue #105 — Story 9-2 main tracker, for sequencing context].

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) — `claude-opus-4-7[1m]` — single-execution
`bmad-dev-story` run on 2026-05-03 immediately after `bmad-create-story
9-3` (same conversation). The story was implemented in a single pass
with no HALT conditions triggered; the only mid-implementation
course-correction was on the static-asset filename (see "Field-shape
divergence from spec" below).

### Debug Log References

- Test runs documented inline in Task 5 above.
- Two mid-implementation course-corrections:
  1. **`OpcMetricTypeConfig` doesn't derive `Eq`** — the new
     `DeviceSummary { metric_types: Vec<OpcMetricTypeConfig> }` field
     forced a downgrade from `#[derive(PartialEq, Eq)]` to
     `#[derive(PartialEq)]` on `ApplicationSummary`,
     `DashboardConfigSnapshot`, and `DeviceSummary`. Functional impact:
     none — the structs aren't used as HashMap keys.
  2. **`devices.html` placeholder is reserved for Story 9-5** — the
     story spec said "Replace `static/devices.html`", but reading the
     placeholder body (`Story 9-5 will fill this in (device + metric
     mapping CRUD)`) revealed that `devices.html` is for the device-
     CRUD story, not 9-3. Created `static/metrics.html` instead;
     left all Story 9-1 placeholders intact. Documented in
     "Field-shape divergence from spec" below.

### Completion Notes List

- **AC#1 (`ApplicationSummary` extension + `DashboardConfigSnapshot`
  deeper walk) — COMPLETE.** New `DeviceSummary` struct holds
  `device_id`, `device_name`, `metric_names`, AND `metric_types`
  (the latter beyond the spec — needed so `/api/devices` has the
  configured fallback type without re-reading `AppConfig` per request).
  `DashboardConfigSnapshot::from_config` walks one level deeper,
  collecting `read_metric_list` per device. Backward compatibility
  preserved: `StatusResponse` (Story 9-2 `/api/status`) doesn't
  serialise `applications` so the new field is invisible to that
  contract. Three test fixtures (`tests/web_auth.rs`, `src/web/api.rs`,
  `tests/web_dashboard.rs`) updated for the new shape; one new
  `dashboard_snapshot_from_config_handles_device_with_zero_metrics`
  unit test added; existing snapshot test extended to assert the
  `devices` + `metric_names` + `metric_types` fields.

- **AC#2 (`GET /api/devices` JSON endpoint) — COMPLETE.** New
  `src/web/api.rs::api_devices` handler (~100 LOC) reads
  `load_all_metrics()`, builds an O(N) `HashMap` lookup keyed by
  `(device_id, metric_name)`, then walks the snapshot's
  `applications.devices.metric_names` to produce the application-
  grouped JSON. Server-side `as_of` captured at handler entry (not
  after the storage delay) per the spec. `stale_threshold_secs`
  resolved at AppState construction time (added new
  `AppState.stale_threshold_secs: u64` field — see Field-shape
  divergence #3); `bad_threshold_secs` is the constant 86_400 from
  `BAD_THRESHOLD_SECS` (mirrors `STATUS_CODE_BAD_THRESHOLD_SECS` in
  `src/opc_ua.rs`). Storage failure path: `event="api_devices_storage_error"`
  warn + generic `{"error":"internal server error"}` body; pinned by
  the 500-path unit test asserting the inner `"synthetic failure"`
  string is ABSENT from the response body (NFR7 invariant).

- **AC#3 (auth carry-forward) — COMPLETE.** New `/api/devices` route
  inherits the Story 9-1 auth middleware automatically via the
  layer-after-route invariant in `src/web/mod.rs::build_router`. Pinned
  by `tests/web_dashboard.rs::auth_required_for_api_devices` which
  asserts unauth'd `GET /api/devices` returns 401 + WWW-Authenticate
  header + emits the `event="web_auth_failed" path=/api/devices
  reason=missing` audit event. Uses the iter-1-introduced buffer-clear
  pattern (mirrors `auth_required_for_api_status`) to prevent a
  polluted tracing-test buffer from false-passing.

- **AC#4 (live-metrics page + FR41) — COMPLETE.** New
  `static/metrics.html` (instead of `static/devices.html` — see
  course-correction in Debug Log References). Reuses
  `static/dashboard.css` (extended at the bottom with metrics-grid
  styles scoped to `.metrics-grid-container` so 9-2's tile selectors
  are byte-identical). New `static/metrics.js` mirrors `dashboard.js`
  iter-2 hardening: in-flight guard, AbortController feature-detect,
  Content-Type sniff, generic error banner, stale-render guard, shared
  `parseTimestamp` parse pass. Per-row staleness computation runs
  client-side from `(as_of_ms - timestamp_ms) / 1000` vs the two
  threshold fields the JSON ships. Mobile-responsive: tables collapse
  to `<dl>`-style key/value rows below 600 px via CSS-only `@media`
  rules. Dark-mode follows `prefers-color-scheme: dark`.

- **AC#5 (regression baseline) — COMPLETE.**
  - `cargo test --lib --bins`: **322 (lib) + 345 (bins) = 667 total
    passed / 0 failed / 5 ignored.** Δ +10 from the 657 (317+340)
    Story 9-2 baseline, matching the spec's "≈+10" target.
  - `cargo test --tests`: all 16 integration binaries pass.
    `tests/web_dashboard.rs`: 12 passed (was 8 — +4 new Story 9-3
    tests). `tests/web_auth.rs`: 14 passed (carry-forward unchanged).
  - `cargo clippy --all-targets -- -D warnings`: **clean**.
  - `cargo test --doc`: **0 failed / 56 ignored** (issue #100 baseline
    preserved).

- **AC#6 (carry-forward intact) — MOSTLY COMPLETE; one documented
  divergence.**
  - `git diff HEAD --stat` over `src/opc_ua.rs`,
    `src/opc_ua_history.rs`, `src/opc_ua_session_monitor.rs`,
    `src/opc_ua_auth.rs`, `src/web/auth.rs`, `static/index.html`,
    `static/dashboard.js`: **zero output** across all 7 files.
    Production code in those files is byte-identical to pre-9-3.
  - `static/dashboard.css` was modified (additions only — appended
    metrics-grid styles after the existing dashboard rules). The
    spec explicitly allowed this (additions OK; no edits to existing
    rules) — rules above the new section are byte-identical.
  - **`src/main.rs` was modified** (+12 LOC inside the existing
    `if web_enabled { ... }` block) to plumb `stale_threshold_secs`
    into the new `AppState` field. This is a divergence from the
    original AC#6 wording — see Field-shape divergence #3 below
    for the rationale. Functionally correct; doesn't affect the
    Story 9-1 + 9-2 surfaces.

- **AC#7 (sanity check) — COMPLETE.**
  - Default test count grew by **+10** lib+bins (within the spec's
    "≈+10" budget; +4 unit tests for `api_devices` in `src/web/api.rs`,
    +1 new snapshot test in `src/web/mod.rs`, +5 cross-bin growth
    from cumulative test renumbering).
  - Total `event=` grep across `src/`: exactly **4 distinct values**
    — `web_server_started` (9-1 info), `web_auth_failed` (9-1 warn
    audit), `api_status_storage_error` (9-2 warn diag),
    `api_devices_storage_error` (9-3 warn diag). One emit site per
    event. New event registered in `docs/logging.md`.
  - Zero new audit events on the OPC UA path (AC#6 invariant).
  - Zero success-path audit events for `/api/devices` (no
    `event="api_devices_returned"` or similar — same convention as
    `/api/status`).

#### Field-shape divergence from spec

- **Added `metric_types: Vec<OpcMetricTypeConfig>` to
  `DeviceSummary`** (spec only listed `metric_names`). Needed so the
  `/api/devices` handler can fall back to the configured type when
  a metric has no row in `metric_values` ("never reported" case).
  Without this, the handler would have to thread `Arc<AppConfig>`
  through to the snapshot — heavier than carrying one extra `Vec`
  per device. The spec's AC#2 contract already calls for the
  fallback behaviour; this just provides the data path.
- **Created `static/metrics.html` (NEW file) instead of replacing
  `static/devices.html`** (spec said "Replace `static/devices.html`").
  Discovered during implementation that the Story 9-1 `devices.html`
  placeholder is reserved for **Story 9-5** (device + metric mapping
  CRUD). Repurposing it for Story 9-3 would have broken Story 9-1's
  reservation and forced Story 9-5 to introduce a different filename
  later. New `metrics.html` filename matches the BDD wording "live
  metrics page" → URL `/metrics.html` is operator-readable. All
  Story 9-1 placeholders (`devices.html`, `applications.html`,
  `commands.html`) remain intact for Stories 9-4/5/6.
- **Added `AppState.stale_threshold_secs: u64` field** (spec said
  "config.opcua.stale_threshold_seconds" should be read directly).
  `AppState` doesn't carry `Arc<AppConfig>` today; threading that
  through would be a heavier change. Instead, resolved the value at
  `AppState` construction in `main.rs` (one local `let
  stale_threshold_secs = application_config.opcua...unwrap_or(120)`)
  and stored it as a scalar field. **Side effect:** `src/main.rs`
  was modified beyond the AC#6 wording. The diff is +12 LOC,
  contained inside the existing `if web_enabled { ... }` block,
  and doesn't touch the bind / spawn / shutdown sequence. Story
  9-7 (hot-reload) will swap the field for an `Arc<AtomicU64>` or
  similar to support runtime retuning without restart.
- **CSS bundled into `static/dashboard.css`** rather than created
  as separate `static/devices.css`. Spec offered both options; chose
  bundled for caching efficiency. New rules scoped to
  `.metrics-grid-container` so 9-2's `.tile` selectors are unaffected.
  Documented in completion notes per the spec's request.
- **`Cargo.toml` not modified** — `serde_json`, `chrono`, `axum`,
  `tower-http` all already direct deps; no new crate additions.

#### Deferred Task 0 — GitHub tracker issue

**Task 0** (open ONE GitHub issue for Story 9-3) is deferred to commit
time per the Story 9-2 precedent. The dev agent did not call `gh issue
create` proactively — operator-visible state on a shared system is left
for the user to authorize at commit time.

### File List

**New files:**

- `static/metrics.html` — live-metrics page markup (~30 LOC).
- `static/metrics.js` — defensive fetch + render + per-row staleness
  computation. (~280 LOC.)

**Modified files:**

- `src/web/mod.rs` — new `DeviceSummary` struct; `ApplicationSummary`
  extended with `devices` field; `DashboardConfigSnapshot::from_config`
  deeper walk; `AppState.stale_threshold_secs` field; new route
  registration in `build_router`; `Eq` derive dropped from 3 structs
  (because `OpcMetricTypeConfig` doesn't derive `Eq`); 1 extended +
  1 new unit test.
- `src/web/api.rs` — `chrono::Utc` import; `OpcMetricTypeConfig` import;
  `HashMap` import; `pub const DEFAULT_STALE_THRESHOLD_SECS`;
  `BAD_THRESHOLD_SECS` const; `config_type_to_display` helper; new
  `DevicesResponse` / `ApplicationView` / `DeviceView` / `MetricView`
  structs; `api_devices` handler (~100 LOC); `FailingBackend::load_all_metrics`
  now returns synthetic `Err`; `build_state_for_devices` + `make_dev`
  test helpers; 4 new unit tests for `api_devices`. Net ~+360 LOC.
- `src/main.rs` — `stale_threshold_secs` resolved + plumbed into
  `AppState` (+12 LOC inside the existing `if web_enabled { ... }`
  block).
- `static/dashboard.css` — appended ~180 LOC of metrics-grid styles
  scoped to `.metrics-grid-container`. Existing dashboard rules
  byte-identical.
- `tests/web_auth.rs::wrap_in_app_state` — added
  `stale_threshold_secs: 120` to the AppState construction (1 line).
- `tests/web_dashboard.rs` — `build_test_app_state` extended for
  new AppState field; `build_production_static_dir` extended to copy
  `metrics.html` + `metrics.js`; 4 new integration tests appended.
  Net ~+250 LOC.
- `docs/security.md` — extended `/api/devices` paragraph in the API
  endpoints subsection.
- `docs/logging.md` — added `api_devices_storage_error` row.
- `README.md` — Web UI subsection extended with Story 9-3 paragraph;
  Epic 9 Planning row updated; `last_updated:` bumped.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — flipped
  9-3 row `ready-for-dev → in-progress` (will flip to `review` at the
  end of this Dev Agent Record run); refreshed `last_updated:`
  narrative.
- `_bmad-output/implementation-artifacts/9-3-live-metric-values-display.md`
  — this file: status flipped `ready-for-dev → review`, all task
  checkboxes filled, Dev Agent Record + Completion Notes +
  Field-shape divergence + File List populated.

### Change Log

| Date | Change | Detail |
|------|--------|--------|
| 2026-05-03 | Story file created | `bmad-create-story 9-3`. Status set to `ready-for-dev`. |
| 2026-05-03 | Status flipped `ready-for-dev → in-progress → review` | Single-execution `bmad-dev-story` run. All 7 ACs satisfied on first pass; loop terminates without iteration. Two minor course-corrections (OpcMetricTypeConfig Eq derive; `metrics.html` filename instead of `devices.html`) documented in Debug Log References + Field-shape divergence. AC#6 partial divergence: `src/main.rs` modified +12 LOC for the new `AppState.stale_threshold_secs` plumbing — documented in Field-shape divergence #3. Test count grew by +10 lib+bins (657 → 667); 16 integration binaries all pass; clippy clean; doctest 0 fail. |

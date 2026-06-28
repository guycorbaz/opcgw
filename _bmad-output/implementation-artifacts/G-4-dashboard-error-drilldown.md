# Story G.4: Dashboard Error Drill-Down

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **operator seeing an error count on the dashboard**,
I want to click it and see the list of recent actual errors (time, category, device, message),
so that I can diagnose what's failing without shelling into the container to read logs.

GitHub issue: **#127** (milestone #4 — v2.4.0). The **heaviest Epic G story** — it is the only one needing *new storage* (today `gateway_status.error_count` is a single cumulative `i32`; there is no error-event store). Per the epic: surface *recorded events*, **no new aggregation** (#130). Built on the F-3 dashboard + F-1 shell + F-0 staged-apply conventions.

## Acceptance Criteria

1. **Drill-down from the dashboard.** The dashboard "Errors (cumulative)" tile (`index.html` `#error-count`) becomes a link/affordance to an **Errors view** (`/errors.html` on the F-1 shell) that lists recent errors **most-recent-first**, each row showing: timestamp, category, the offending `device_id` / `application_id` (when applicable), and a sanitized message.
2. **New error-event store, bounded.** A new persistence surface records error events with a **bounded cap** (oldest pruned beyond the cap — a ring-buffer discipline). Schema migration **v013** adds an `error_events` table (`id`, `ts`, `category`, `device_id` nullable, `application_id` nullable, `message`); the cap is enforced on every insert. Both storage backends (`SqliteBackend` + `InMemoryBackend`) implement it. The cumulative `error_count` tile stays unchanged — the feed is *complementary* (bounded/recent), and is not expected to numerically equal the cumulative count (document this).
3. **New endpoint.** `GET /api/errors?limit=…` returns the recent events as `{ items:[{ ts, category, device_id, application_id, message }], count }`, newest-first, with a capped `limit` (reject over-cap with `400`, mirror `/api/inventory/uplinks` limit handling). Auth-gated like the rest of `/api/*`.
4. **Capture at the existing error sites.** Errors already logged by the poller are also recorded into the feed — at minimum: per-device metric-poll failure (`src/chirpstack.rs:1385` `Failed to get metrics for device`), ChirpStack connectivity failure, ChirpStack auth failure, and batch-write failure (`src/chirpstack.rs:1514`). Each records a small fixed `category` (e.g. `device_poll`, `chirpstack_connect`, `chirpstack_auth`, `metric_write`) plus the device/app context where available. Recording is best-effort and never breaks the poll cycle (a record failure is logged, not propagated).
5. **No secret / PII leakage.** Stored messages are sanitized per `docs/security.md` — no `api_token`, no full credentials; control characters stripped and length-bounded, matching the audit-log discipline (the `inventory_uplink_dropped` Debug-format precedent). A test asserts a token-bearing error string does not surface the token.
6. **Bounded retention verified.** Inserting more than the cap keeps only the newest `cap` events (the ring-buffer bound). The cap is a documented constant (`DEFAULT_ERROR_EVENT_CAP`) with an env override (`OPCGW_ERROR_EVENT_CAP`) — **not** a singleton-config/UI knob (keeps the config surface and the G-2 help catalog unchanged; document this scoping decision).
7. **Tests + gates.** Server-side tests: migration v013 round-trip; `record_error_event` + `recent_error_events` + the prune-to-cap bound (both backends); the endpoint (happy path, limit cap → 400, sanitization); served-asset for `errors.html`/`errors.js`; the dashboard tile links to the errors view. `node --check` on new JS; full `cargo test` 0-fail; `cargo clippy --all-targets -- -D warnings` clean.
8. **Docs synced.** README (new endpoint/page + the `OPCGW_ERROR_EVENT_CAP` knob), `docs/configuration.md` (the env var), `docs/security.md` (error-message sanitization note), and `docs/manual/latex/body.tex` (the new Errors view). No build step for the web asset.

## Tasks / Subtasks

- [x] **Task 1 — Storage: `ErrorEvent` type + trait methods.** (AC: 2, 5, 6)
  - [x] Add `ErrorEvent { ts: DateTime<Utc>, category: String (or a small enum→str), device_id: Option<String>, application_id: Option<String>, message: String }` in `src/storage/types.rs` (or `mod.rs`).
  - [x] Extend the `StorageBackend` trait (`src/storage/mod.rs:~750`, beside `update_gateway_status`): `record_error_event(&self, ev: &ErrorEvent) -> Result<(), OpcGwError>` (inserts + prunes to the cap) and `recent_error_events(&self, limit: usize) -> Result<Vec<ErrorEvent>, OpcGwError>` (newest-first).
  - [x] `DEFAULT_ERROR_EVENT_CAP` constant (e.g. 500) + `OPCGW_ERROR_EVENT_CAP` env resolver (mirror the `#144` env-configurable-budget pattern: read via accessor, not a literal, so tests can inject).
  - [x] Sanitization helper: strip control chars, bound length (e.g. 1 KiB), never store a token — reuse/extend the audit-hygiene approach.
- [x] **Task 2 — Migration v013 + `SqliteBackend` impl.** (AC: 2, 6)
  - [x] `migrations/v013_error_events.sql`: `CREATE TABLE error_events (id INTEGER PRIMARY KEY AUTOINCREMENT, ts TEXT NOT NULL, category TEXT NOT NULL, device_id TEXT, application_id TEXT, message TEXT NOT NULL)` (+ an index on `id`/`ts` for newest-first reads). Register in `src/storage/schema.rs` (const + apply step); bump latest schema version 12 → 13 (update the `assert_eq!(version, 12 …)` tests to 13).
  - [x] `SqliteBackend::record_error_event` — insert then prune-to-cap (`DELETE FROM error_events WHERE id <= (SELECT MAX(id) FROM error_events) - :cap`, or `NOT IN (… ORDER BY id DESC LIMIT :cap)`). `recent_error_events` — `SELECT … ORDER BY id DESC LIMIT :limit`. Go through the connection pool like the other writes.
- [x] **Task 3 — `InMemoryBackend` impl.** (AC: 2, 6) — a `VecDeque<ErrorEvent>` (or Vec) capped at the cap (push back, pop front beyond cap); `recent_error_events` returns newest-first. Keep behaviour identical to the SQLite bound so tests pass against both.
- [x] **Task 4 — Capture wiring in the poller.** (AC: 4, 5) — at the error sites (`src/chirpstack.rs` device-poll failure ~1385, connect/auth, batch-write ~1514), call `backend.record_error_event(&ErrorEvent{…})` with the right category + device/app context + sanitized message. Best-effort: on record failure, `warn!` and continue (never break the poll). The poller already holds `backend: Arc<dyn StorageBackend>`.
- [x] **Task 5 — `GET /api/errors` handler + route.** (AC: 3, 5) — new handler in `src/web/api.rs` (or a small `src/web/errors.rs`): parse `?limit` (default + cap, 400 over-cap like `inventory_uplinks`), call `recent_error_events`, return the JSON envelope. Register the route in `src/web/mod.rs` (auth-gated, GET-only, CSRF-exempt — same as the other read endpoints).
- [x] **Task 6 — Errors view + dashboard link.** (AC: 1) — new `static/errors.html` (F-1 shell, `<script src="/shell.js">` + new `errors.js`) rendering the feed newest-first (time / category / device-or-app / message); `static/errors.js` fetches `/api/errors` with the Story 9-2 fetch-hardening (`makePoller` reuse if applicable). Make the `index.html` `#error-count` tile a link to `/errors.html` (and optionally deep-link a `device_poll` row to the G-0 device editor `#/app/:id/device/:eui`). Add `errors.html`/`errors.js` to the F-1 shell nav if appropriate, and to the `tests/web_dashboard.rs` static-copy list.
- [x] **Task 7 — Tests, gates, docs.** (AC: 5, 6, 7, 8)
  - [x] Storage tests (both backends): record→recent round-trip, prune-to-cap bound (insert cap+N → len==cap, newest kept), sanitization (token-bearing message stored scrubbed). Migration v013 round-trip + version==13.
  - [x] Web tests: `/api/errors` happy path, `?limit` over cap → 400, auth required; served-asset `errors.html`/`errors.js`; dashboard tile links to `/errors.html`.
  - [x] `node --check static/errors.js`; full `cargo test` 0-fail; `cargo clippy --all-targets -- -D warnings` clean.
  - [x] Doc-sync: README, `docs/configuration.md` (`OPCGW_ERROR_EVENT_CAP`), `docs/security.md` (sanitization), `docs/manual/latex/body.tex` (Errors view).

## Dev Notes

### What exists today (verified 2026-06-28)

- **Only a cumulative counter.** `gateway_status` (migration v006) holds `error_count` as a single cumulative `i32`; `ChirpstackPoller` increments it and calls `update_gateway_status` (`src/storage/mod.rs:750`, both backends). `/api/status` exposes it (`src/web/api.rs:50,245`); the dashboard renders it into `#error-count` (`static/dashboard.js:371`) under the `index.html` "Errors (cumulative)" tile (`static/index.html:58`). There is **no event list** — that is the whole of G-4's new storage.
- **Migration system.** Sequential SQL files in `migrations/` (latest `v012_device_stale_threshold.sql`); registered in `src/storage/schema.rs` as `MIGRATION_V0NN` consts + applied in order; current latest version is **12** (asserted in `schema.rs` tests at lines ~466/495 — bump to 13). G-4 adds **v013**.
- **Storage trait + two backends.** `StorageBackend` (`src/storage/mod.rs`) is implemented by `SqliteBackend` (`src/storage/sqlite.rs`, via the connection pool) and `InMemoryBackend` (`src/storage/memory.rs`). Every new method must land on **both** (tests run against both). `update_gateway_status` is the closest precedent to mirror.
- **Poller holds the backend.** `ChirpstackPoller { backend: Arc<dyn StorageBackend>, … }` (`src/chirpstack.rs:387`), so the capture sites already have a handle. Error sites to hook: `chirpstack.rs:1385` (per-device GetMetrics failure — the #126 per-device errors the issue calls out), connect/auth (`:153` token parse), batch-write (`:1514`).
- **Sanitization precedent.** `docs/security.md` is the contract. The `log_item_to_uplink` audit (`src/chirpstack_inventory.rs`) uses Debug-formatting (`?`) on unconstrained upstream strings to neutralise log-injection; apply the same discipline to stored error messages (strip control chars, bound length, never include `api_token`).

### Design guidance / scoping

- **SQLite table, not just an in-memory ring.** A persisted `error_events` table survives restarts (better for diagnosing a crash-loop) and matches the existing store; the in-memory backend keeps an equivalent capped deque. Both enforce the same cap so the AC#6 bound test is backend-agnostic.
- **Cap is a constant + env, not a UI knob.** `DEFAULT_ERROR_EVENT_CAP` (≈500) overridable by `OPCGW_ERROR_EVENT_CAP`. Deliberately NOT a `[global]` singleton field — that would add a singleton-config row, a G-2 help-catalog entry, and a UI control for a niche tuning knob. Document the decision in Dev Notes + `deferred-work.md` if challenged.
- **Complementary, not equal.** The cumulative `error_count` tile and the bounded feed measure different things (cumulative-since-boot vs recent-bounded). Don't try to make the feed length match the count; the drill-down just shows *what* the recent errors were.
- **No aggregation (#130).** Record discrete events; do not compute rollups in the gateway. The view/SCADA can aggregate if it wants.
- **Best-effort capture.** A failure to record an error event must never abort or skip a poll cycle — `warn!` and continue. Recording is observability, not control flow.
- **No build step.** `errors.js` is vanilla on the F-1 shell; `node --check` is the JS gate. Served-HTML DOM-ID invariant (`tests/web_dashboard.rs`) — add `errors.html`/`errors.js` to the copy list; don't break existing markers.
- **Defer fallback.** If v2.4.0 scope tightens, G-4 is the documented defer candidate — but this story implements it in full.

### Project Structure Notes

- Backend: `migrations/v013_error_events.sql` (new), `src/storage/schema.rs` (register + version bump), `src/storage/{mod.rs,types.rs}` (trait + `ErrorEvent` + cap), `src/storage/sqlite.rs` + `src/storage/memory.rs` (impls), `src/chirpstack.rs` (capture wiring). Web: `src/web/api.rs` or new `src/web/errors.rs` (handler), `src/web/mod.rs` (route). Frontend: `static/errors.html` + `static/errors.js` (new), `static/index.html` (tile→link), `static/shell.js` (nav, if added). Tests: storage unit tests (both backends), `tests/web_dashboard.rs` (+copy list), a web errors test. Docs: README, configuration.md, security.md, body.tex.
- Conventions: SPDX headers on new `src/*.rs` + `migrations/*.sql` (match siblings); structured `event=…` logging; vanilla IIFE JS; reuse the `inventory_uplinks` limit-cap + envelope shape for `/api/errors`.

### References

- [Source: _bmad-output/planning-artifacts/epics.md#Epic G — Story G.4: Dashboard Error Drill-Down]
- [Source: GitHub issue #127 — dashboard error count should drill down to a list]
- [Source: src/storage/mod.rs:750 — `update_gateway_status` (trait-method precedent)]
- [Source: src/storage/schema.rs — migration registry + version (12 → 13)]
- [Source: src/chirpstack.rs:387,1385,1514 — `backend` handle + error capture sites]
- [Source: src/web/api.rs:50,245 — current `error_count` exposure]
- [Source: static/index.html:58, static/dashboard.js:371 — the "Errors (cumulative)" tile to make clickable]
- [Source: src/web/inventory.rs:296 — `inventory_uplinks` (limit-cap + envelope to mirror for /api/errors)]
- [Source: docs/security.md — message-sanitization contract]
- Previous story intelligence: F-3 built the dashboard (`/api/status` + client-derived health) and explicitly found "no recent-errors store exists — error_count is a single cumulative i32" → G-4 is that store. G-1 (`inventory_uplinks`) is the limit-cap/envelope/audit precedent; G-2's `field-help.js` is unaffected (the cap is intentionally not a UI field). Both honour the served-HTML DOM-ID invariant G-4 must also keep.

## Dev Agent Record

### Agent Model Used

Opus 4.8 (1M context) — claude-opus-4-8[1m]

### Debug Log References

- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test` — all suites 0-fail (new `tests/error_events.rs` 5/5 both backends; web `api_errors_*` + `errors_view_*`; utils `sanitize_*` 3/3). One pre-existing table-count assertion (11→12) updated for the new table.
- `node --check static/errors.js` — clean.
- `docs/manual/latex/build.sh` — exit 0.

### Completion Notes List

- **Migration v013** adds `error_events`; schema latest bumped 12→13. Updated all 9 "latest version" assertions + the table-count assertion (11→12) across `schema.rs` / `sqlite_singleton_config_migration.rs`.
- **Cap = const + env, not a UI knob** — `DEFAULT_ERROR_EVENT_CAP=500` + `OPCGW_ERROR_EVENT_CAP` resolved once at startup (mirrors the GH-144 budget resolver: process-global atomic, `error_event_cap()` accessor). Kept off the singleton-config/G-2-help surface by design.
- **Ring-buffer prune** — SQLite uses `DELETE … WHERE id NOT IN (SELECT id … ORDER BY id DESC LIMIT cap)` (correct with non-contiguous ids after earlier prunes, unlike `id <= MAX-cap`); in-memory uses a `VecDeque` with `pop_front` beyond cap. Both verified at the live default cap (no global mutation → no cross-test races).
- **Capture is best-effort** — `ChirpstackPoller::capture_error_event` sanitizes + records; a storage failure is `warn!`-ed and swallowed, never breaking a poll cycle. Wired at 3 sites: per-device GetMetrics failure (`device_poll`), cycle-level `poll_metrics` failure (`chirpstack_poll`), batch-write give-up (`metric_write`).
- **Sanitization** in `utils::sanitize_error_message` (control-char strip + 1 KiB bound); the token never reaches the feed (capture sites format the error Display, not the token). Documented in `docs/security.md`.
- **Endpoint** `GET /api/errors?limit=…` mirrors `inventory_uplinks` (default 100, cap 500 → 400). **UI**: new `errors.html`/`errors.js` on the F-1 shell; the dashboard `#tile-errors` gains a "View recent errors →" link (kept the cumulative count tile unchanged — the feed is complementary).
- **FailingBackendForApiTests** (api.rs test mock) gained the two new trait methods (return Err).

### File List

- `migrations/v013_error_events.sql` — NEW (table + index).
- `src/storage/schema.rs` — register v013 + LATEST_VERSION 13 + apply block; version/table-count test assertions.
- `src/storage/types.rs` — `ErrorEvent` type.
- `src/storage/mod.rs` — re-export + 2 trait methods.
- `src/storage/sqlite.rs` — `SqliteBackend` impl (insert+prune, recent).
- `src/storage/memory.rs` — `InMemoryBackend` field + impl (VecDeque ring).
- `src/utils.rs` — cap const/env/resolver + `error_event_cap()` + `sanitize_error_message` + unit tests.
- `src/main.rs` — `init_error_event_cap_from_env()` at startup.
- `src/chirpstack.rs` — `capture_error_event` helper + 3 capture sites.
- `src/web/api.rs` — `api_errors` handler + `ErrorsQuery`/`ErrorsResponse` + mock-backend methods.
- `src/web/mod.rs` — `/api/errors` route.
- `static/errors.html`, `static/errors.js` — NEW errors view.
- `static/index.html` — error tile drill-down link.
- `tests/error_events.rs` — NEW storage tests (both backends).
- `tests/web_dashboard.rs` — copy-list + `api_errors_*` + `errors_view_*` tests.
- `tests/sqlite_singleton_config_migration.rs` — version assertion 12→13.
- `docs/configuration.md`, `docs/security.md`, `docs/manual/latex/body.tex`, `README.md` — doc-sync.
- `_bmad-output/implementation-artifacts/{G-4-dashboard-error-drilldown.md, sprint-status.yaml}`.

## Change Log

- 2026-06-28 — Implementation complete (all 7 tasks). Dashboard error drill-down: bounded error-event store (migration v013 + ring buffer), poller capture, `GET /api/errors`, `errors.html` view + dashboard link. Status ready-for-dev → review.
- 2026-06-28 — Code review (3 adversarial layers Blind/Edge/Auditor on Sonnet + mandatory iter-2). AC#1–8 MET. Loop terminated LOW-only. Status review → done.

### Review Findings (2026-06-28)

- [x] [Review][Patch] **MED**: `/api/errors` `?limit` cap used a compile-time const (500) that diverged from the runtime `OPCGW_ERROR_EVENT_CAP` → events beyond 500 unreachable + wrong 400 body. Now uses `error_event_cap()` at request time. [src/web/api.rs] (Blind#1 + Edge EC-5)
- [x] [Review][Patch] **MED**: AC#4 named 4 categories; only 3 shipped. Added `classify_poll_error` mapping the cycle-level error to `chirpstack_connect` / `chirpstack_auth` / `chirpstack_poll`. [src/chirpstack.rs] (Auditor F1)
- [x] [Review][Patch] **MED**: AC#5's mandated token-bearing test was absent. Added `Bearer <token>` redaction to `sanitize_error_message` (byte-safe via `to_ascii_lowercase`) + the redaction test. [src/utils.rs] (Auditor F2)
- [x] [Review][Patch] RFC3339 parse failure now `warn!`s before the `Utc::now()` fallback [src/storage/sqlite.rs] — LOW (Blind#5/Edge EC-3).
- [x] [Review][Patch] `recent_error_events` clamps `limit` to i64 range (no `LIMIT -1`) [src/storage/sqlite.rs] — LOW (Edge EC-2).
- [x] [Review][Patch] `sanitize_error_message` doc corrected re: tab preservation [src/utils.rs] — LOW (Blind#3).
- [x] [Review][Patch] Added `error_events` (+ `meta`, `singleton_config`) to `test_migrations_create_all_tables` [src/storage/schema.rs] — LOW (Edge EC-4).
- [x] [Review][Defer] Blocking `pool.checkout` from async (pre-existing, #73) — LOW, deferred-work.md.
- [x] [Review][Defer] INSERT+prune-DELETE not in one transaction (race converges to cap) — LOW, deferred-work.md.
- [x] [Review][Dismiss] Double-record device errors (Blind#6) — false positive; the device loop captures `device_poll` and **continues**, so per-device errors never reach the cycle-level catch-all.
- [x] [Review][Dismiss] `?limit=0` → empty — acceptable semantics.

iter-2 re-review (fresh Sonnet agent, on the patch delta) **caught 2 issues introduced by the iter-1 patches** (the iter-N+1 rule earning its keep): (a) **MED** — the web cap test still hardcoded `?limit=501` + `cap==500`, now environment-sensitive → fixed to derive `error_event_cap()`; (b) **LOW** — `classify_poll_error` checked "auth" before "connect" so a connect error citing an `auth…` hostname could misclassify → reordered connect-first + added a regression test. Confirmed `redact_bearer_tokens` byte-safe (all edges), `warn!` imported, no dangling `ERRORS_LIMIT_CAP`. Gates: full `cargo test` 0-fail, `cargo clippy --all-targets -- -D warnings` clean.

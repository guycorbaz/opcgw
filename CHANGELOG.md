# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- **Per-device OPC UA `SourceTimestamp` mode**
  ([#153](https://github.com/guycorbaz/opcgw/issues/153)). New per-device
  `source_timestamp_server` flag (web-UI checkbox in Config → device,
  `source_timestamp_server` in `config.toml`, and the `source_timestamp_server`
  field on the device CRUD API). When `true`, opcgw stamps that device's served
  values with the gateway's current time (`now()`) instead of the device's real
  report time. This fixes SCADA clients (notably **Ignition**) that overlay a
  *Stale / Uncertain* quality on any value whose OPC UA `SourceTimestamp` is
  older than a client-side window — slow-cadence LoRaWAN devices (e.g. a 20-min
  uplink interval) otherwise read Uncertain between uplinks even though opcgw
  returns `Good`, and raising `stale_threshold_seconds` does not help because
  that knob only governs opcgw's own `StatusCode`, not the client's timestamp
  interpretation. Diagnosed live on the panoramix deployment against Ignition
  8.3 by reading the OPC UA server directly (all nodes served `Good` on both
  reads and subscriptions — the Uncertain was entirely client-side). Default is
  `false` (strict OPC UA semantics; unchanged behaviour for every existing
  device). The staleness `StatusCode` model is unaffected: a device that
  genuinely stops reporting past its stale threshold still flips to `Uncertain`.
  Schema migration **v014** adds the `devices.source_timestamp_server` column.

## [2.6.1] — 2026-07-08 — Storage-latency budget fix

> **Status:** released. Patch release fixing a regression found during the
> Ignition SCADA go-live soak-check log review on panoramix (~700
> `exceeded_budget=true` WARNs/day, mostly `batch_write_metrics`).

### Fixed
- **`batch_write_metrics` checked against the wrong latency budget**
  ([#149](https://github.com/guycorbaz/opcgw/issues/149)). `StorageOpLog::Drop`
  compared every storage-query `query_type`'s elapsed time against the generic
  `storage_query_budget_ms()` (default 250 ms), even for `batch_write_metrics`,
  which already has its own higher `batch_write_budget_ms()` (default 2000 ms,
  [#144](https://github.com/guycorbaz/opcgw/issues/144)) correctly consulted by
  its caller in `chirpstack.rs`. This reopened the WARN-noise complaint #144 was
  meant to close. `batch_write_metrics` now resolves to the batch-write budget
  in the `Drop`-based log too; regression tests pin the budget-selection logic
  directly. Pure observability fix — no functional or data-loss impact.

Docker images `gcorbaz/opcgw:2.6.1` / `:2.6` / `:latest` (multi-arch amd64+arm64) + GHCR mirror.

## [2.6.0] — 2026-07-04 — Web UI refresh, first slice (Epic I)

> **Status:** released (stable). Promoted from `v2.6.0-rc1` after a clean
> multi-day soak on the panoramix NAS: ~42 h of continuous uptime with zero
> restarts, zero ERROR/WARN-level lines on Jul 2–4, no panics, the OPC UA server
> steadily listening on `:4855`, and poll cycles running every cycle. The only
> ongoing WARN traffic is a pre-existing device-side config mismatch on one dev
> unit (four fields configured `String` but the codec emits numeric/bool →
> those fields are skipped), owner-deferred and not an opcgw defect. A
> **partial-epic release**: it ships the first three stories of **Epic I — Web
> UI Refresh** (I-0/I-1/I-2); I-3 (component refresh) and I-4 (cross-page rollout
> + QA) remain and will land in a later 2.6.x / 2.7 cut.

### Changed
- **First slice of the ChirpStack-adjacent web-UI refresh** — pure presentation,
  with **no `/api`, write-model, or behavioural change**, and still **no build
  step / framework / `node_modules`** (hand-written vanilla CSS on the existing
  F-1 shell). Served-HTML DOM-ID test markers and G-2 accessibility (field-help
  `aria-describedby`) are preserved throughout; WCAG AA contrast verified in both
  light and dark modes. Tracks CR
  [#147](https://github.com/guycorbaz/opcgw/issues/147).
  - **I-1 — Design-token foundation.** `static/dashboard.css` refactored onto a
    single set of `:root` CSS custom properties (ChirpStack-v4 / Ant Design
    palette) with fully token-driven dark mode — six per-component dark blocks
    collapsed to one `@media (prefers-color-scheme: dark)` block; all component
    colours now resolve through `var(--token)`.
  - **I-2 — Navigation & shell refresh.** The shared shell (`shell.js` + CSS)
    restyled into a fixed navy left sider on desktop (≥992px) and a top app-bar
    with an accessible hamburger drawer (`aria-expanded` / `aria-controls`,
    keyboard-operable) on mobile; page titles now sit in a light top strip. The
    sider layout is gated on a `has-shell` body class so the shell-less first-run
    wizard is unaffected. Nav link set and served-HTML markers unchanged.

### Internal
- Epic I remains **in progress** (3/5 stories done). I-3 is `ready-for-dev`; I-4
  is backlog. No epic retrospective yet — that follows the last story.

## [2.5.2] — 2026-06-30 — Async storage facade (runtime correctness)

> **Status:** released (stable). Promoted from `v2.5.2-rc1` after the real-binary
> smoke/soak on the NAS — this change touches the data-plane hot paths, the class
> of change the AI-G-5 real-binary smoke gate exists for (unit tests + adversarial
> review do not catch runtime/concurrency regressions; cf. the 2026-05-20
> main-deadlock incident). The rc survived two F-0 in-process soft restarts and a
> 4× application-count increase with zero panics/deadlocks/errors, poll cycles
> resuming cleanly and the OPC UA liveness gauge never skipping a beat. First
> story of **Epic H — Runtime Correctness & Tech-Debt**.

### Fixed
- **Synchronous storage backend blocked async runtime workers**
  ([#73](https://github.com/guycorbaz/opcgw/issues/73)). The synchronous
  `StorageBackend` trait (~30 blocking `rusqlite` methods, shared as
  `Arc<dyn StorageBackend>`) was being called directly from ~30 async tokio call
  sites (poller, gRPC event-stream ingestion, OPC UA history reads, web handlers,
  command pollers), blocking a worker thread for the duration of each SQL call —
  and on the two pool-exhaustion retry backoffs, blocking on `std::thread::sleep`.
  Survivable only because the runtime is multi-threaded; it degrades sharply on
  CPU-constrained deployments (small Docker containers with 1–2 vCPUs). A new
  async facade (`src/storage/async_facade.rs`, `AsyncStorage`) now runs every such
  call on the blocking pool via `tokio::task::spawn_blocking`, reached at call
  sites through the `AsyncStorageExt::async_store()` extension; genuinely
  synchronous async-opcua read/method callbacks (which cannot `.await`) use a
  `run_blocking_storage()` helper that applies `tokio::task::block_in_place` on a
  multi-threaded worker and runs inline otherwise. The two retry sleeps now run
  inside `spawn_blocking`, off the async workers. **No behavioural change** —
  identical return types, `OpcGwError` mapping, and ordering. Code review also
  removed a pre-existing reentrant-`Mutex` self-deadlock in the prune
  poison-recovery path surfaced while refactoring that function.

### Internal
- Opened **Epic H — Runtime Correctness & Tech-Debt** to host this and future
  v2.x tech-debt work (candidate follow-ons: RunHandles `Drop`
  [#110](https://github.com/guycorbaz/opcgw/issues/110), queue-capacity
  enforcement [#79](https://github.com/guycorbaz/opcgw/issues/79)).

## [2.5.1] — 2026-06-29 — First-run wizard fix

> **Status:** released. Patch release fixing a first-run onboarding regression
> ([#146](https://github.com/guycorbaz/opcgw/issues/146)) that shipped in v2.5.0
> stable, found by the post-release real-world onboarding smoke (the Epic-G
> retrospective **AI-G-5** gate). The in-process web tests all used an *empty*
> password and so never exercised the as-shipped *placeholder* boot path — only
> running the real binary against the shipped config surfaced it.

### Fixed
- **First-run wizard unreachable with the shipped placeholder password**
  ([#146](https://github.com/guycorbaz/opcgw/issues/146)). A fresh clone following
  the documented quickstart aborted at config validation on the shipped
  `REPLACE_ME_WITH_*` `[opcua].user_password` placeholder instead of booting into
  the `/setup` wizard. `AppConfig::validate()` only carved out the first-run signal
  for an *empty* OPC UA password; a *placeholder* (as actually shipped in
  `config/config.toml`) was rejected — asymmetric with the ChirpStack token, which
  already treats empty *or* placeholder as the first-run signal. The placeholder is
  now accepted while in first-run mode and still rejected once a real ChirpStack
  token is configured.
- **Security (same fix, hardening).** The OPC UA auth gate (`OpcgwAuthManager`)
  keyed its reject-all-in-first-run guard on the password being *empty*. With a
  placeholder now accepted in first-run, the gate was extended to treat a
  placeholder password as "not configured" too, so the well-known shipped
  placeholder string can never become a live OPC UA credential. While in first-run
  the OPC UA server continues to reject all authentication.
- Doc-sync: `docs/security.md` and `README.md` corrected to describe the
  placeholder-boots-the-wizard behavior.

Docker images `gcorbaz/opcgw:2.5.1` / `:2.5` / `:latest` (multi-arch amd64+arm64) + GHCR mirror.

## [2.5.0] — 2026-06-29 — Web UX & Usability (Epic G complete)

> **Status:** released. Completes **Epic G** — the remaining Web-UX stories on top
> of v2.4.0's drill-down config (G-0). Each story went through the full three-layer
> adversarial code review (different model) plus a mandatory second iteration; the
> epic security review was CLEAN (0 HIGH / 0 MED / 2 LOW). Validated as
> `v2.5.0-rc1` on the production NAS (clean boot, schema-v13 migration applied,
> device edit → stage → Apply in-process soft-restart, inventory pickers, uplink
> streams connected, zero-ERROR soak), then promoted to stable. Docker images
> `gcorbaz/opcgw:2.5.0` / `:2.5` / `:latest` (multi-arch amd64+arm64) + GHCR
> mirror.

### Added / changed
- **G-1 — Device-profile metric picker** ([#124](https://github.com/guycorbaz/opcgw/issues/124)).
  The web metric picker can now source candidates from the device's **ChirpStack
  device-profile measurements** — available even when the device hasn't
  transmitted a decoded uplink yet — merged with the recently-observed uplink
  keys and de-duplicated, each row tagged with its source. New read-only endpoint
  `GET /api/inventory/measurements?dev_eui=…` (resolves the device profile via
  gRPC); the measurement kind maps to a suggested metric type. No write-path
  change.
- **G-2 — Contextual field help** ([#142](https://github.com/guycorbaz/opcgw/issues/142)).
  Every configuration field across the first-run wizard, the gateway-settings
  editor, and the device/metric/command forms gains an accessible info-icon
  affordance (`aria-describedby`, keyboard + screen-reader reachable) whose text
  comes from one shared catalog (`static/field-help.js`) derived from
  `docs/configuration.md` so the UI and docs stay in step.
- **G-3 — Per-device OPC UA stale threshold** ([#132](https://github.com/guycorbaz/opcgw/issues/132)).
  The per-device `stale_threshold_seconds` override (existing SQLite column) is
  now settable from the web UI; slow LoRaWAN sensors can be given a longer
  threshold so they don't read `Uncertain` between uplinks while fast devices
  still flag genuine staleness quickly.
- **G-4 — Dashboard error drill-down** ([#127](https://github.com/guycorbaz/opcgw/issues/127)).
  The dashboard "Errors" tile drills down to a new **Errors** view
  (`/errors.html`) listing recent error events (time, category, device,
  sanitized message), newest-first. Backed by a new bounded error-event store
  (schema migration **v013**, ring-buffer capped by `OPCGW_ERROR_EVENT_CAP`,
  default 500), captured at the poller's existing error sites and exposed at
  `GET /api/errors?limit=…`. Messages are sanitized (control-character stripping,
  `Bearer`-token redaction, length bound); no aggregation
  ([#130](https://github.com/guycorbaz/opcgw/issues/130)).

### Notes
- New env knob: **`OPCGW_ERROR_EVENT_CAP`** (default 500) bounds the error-event
  feed; see [`docs/configuration.md`](./docs/configuration.md).
- Schema advances to **v13** (adds the `error_events` table); the migration is
  additive and idempotent.

## [2.4.0] — 2026-06-27 — Web UX: drill-down configuration

> **Status:** released. Validated as `v2.4.0-rc1` in production (clean boot,
> drill-down UI exercised), then promoted to stable. Docker images
> `gcorbaz/opcgw:2.4.0` / `:2.4` / `:latest` (multi-arch amd64+arm64) + GHCR
> mirror. This release ships **Epic G story G-0**; the remaining Web-UX stories
> (device-profile metric picker, contextual field help, per-device stale
> threshold, dashboard error drill-down — [#124](https://github.com/guycorbaz/opcgw/issues/124)
> / [#142](https://github.com/guycorbaz/opcgw/issues/142) / [#132](https://github.com/guycorbaz/opcgw/issues/132)
> / [#127](https://github.com/guycorbaz/opcgw/issues/127)) continue in a later release.

### Added / changed
- **G-0 — Drill-down configuration UI** ([#139](https://github.com/guycorbaz/opcgw/issues/139)).
  The three flat web pages (Applications, Devices configuration, Commands) are
  consolidated into a single **Configuration** page (`/config.html`) that
  presents the setup as a hierarchy — **Application → Device → Metrics/Commands**
  — with a breadcrumb and deep-linkable, reload-safe `location.hash` routing.
  The top navigation collapses those three links into one **Configuration**
  entry; the retired pages become redirect stubs (existing bookmarks and drift
  deep-links still resolve). Frontend-only: every create/read/update/delete
  reuses the existing staged-apply endpoints unchanged (no API or schema
  change), and the ChirpStack inventory pickers / "Apply changes" affordance
  carry over. No build step / framework / `node_modules` added.

## [2.3.2] — 2026-06-26 — storage hardening

> **Status:** release prep — version bumped and notes finalized; `v2.3.2` tag
> and Docker/GHCR publish pending. A small, low-risk hardening patch in the
> 2.3 line (the `:2.3` Docker tag will move to this release). No schema or
> configuration-surface changes; no behavioural change to data collection or
> serving.

### Added

- **Startup integrity check for the `metric_history` index**
  ([#74](https://github.com/guycorbaz/opcgw/issues/74)). After migrations run,
  opcgw verifies that the performance-critical
  `idx_metric_history_device_timestamp` index exists. If it is missing (e.g.
  dropped manually or left absent by a partially-applied migration), the
  gateway logs a single `warn!` with `event="metric_history_index_missing"`
  and a remediation hint, then continues — a missing performance index would
  otherwise degrade history-query speed silently. Missing the index is
  non-fatal; only an unreadable `sqlite_master` catalog (a database-level
  fault) aborts startup, consistent with the other migration steps.
- **Configurable storage-latency WARN budgets**
  ([#144](https://github.com/guycorbaz/opcgw/issues/144)). The thresholds that
  decide when a slow SQLite query or batch write is logged at `warn`
  (`exceeded_budget=true`) instead of `debug` are now tunable via two
  environment variables — `OPCGW_STORAGE_QUERY_BUDGET_MS` and
  `OPCGW_BATCH_WRITE_BUDGET_MS` (positive integer milliseconds), resolved once
  at startup. Invalid or zero values fall back to the default with a `warn!`.

### Changed

- **Storage-latency budget defaults raised to NAS-realistic values**
  ([#144](https://github.com/guycorbaz/opcgw/issues/144)): storage-query budget
  `10 ms → 250 ms`, batch-write budget `500 ms → 2000 ms`. On NAS /
  network-backed SQLite the old SSD-tuned thresholds fired on normal latency
  (~50 `exceeded_budget=true` WARN/hour observed in production), drowning out
  genuine signals. The budgets only gate logging — no storage behaviour
  changes — and operators on fast local disks can lower them via the new env
  vars to restore earlier regression detection.

### Deferred

- **#73 (async sleep in `append_metric_history`)** was triaged for this
  release and **deferred**: the named function is dead in production (its only
  caller, `store_metric`, is uncalled), and the real concern is that the
  synchronous `SqliteBackend` is invoked directly from async tasks without
  `spawn_blocking`/`block_in_place`. Re-scoped to a proper async-storage
  refactor; see the issue for the full finding.

## [2.3.1] — 2026-06-25 — single-file logging

> **Status:** released — tagged `v2.3.1` and published to Docker Hub
> (`2.3.1` / `2.3` / `latest`) and GHCR (multi-arch amd64 + arm64); GitHub
> release published. A patch release that fixes unbounded log growth observed
> in production (11 GB / 16 days on v2.3.0).

### Changed

- **Logging consolidated to a single, retention-capped file**
  ([#143](https://github.com/guycorbaz/opcgw/issues/143)). opcgw now writes one
  daily-rolling log file, `opcgw.log.<date>`, instead of five per-module files
  (`opc_ua_gw.log` + the TRACE-pinned `opc_ua.log` / `storage.log` /
  `chirpstack.log` / `config.log`). Every module logs to the one file at the
  resolved `OPCGW_LOG_LEVEL`; set it to `debug`/`trace` for deep per-module
  detail. The appender keeps the most recent **14** daily files and prunes
  older ones automatically, so the log directory is self-limiting (a 16-day,
  11 GB pile-up was observed in production before this change). Keep the level
  at `info` or more verbose to preserve NFR12 source-IP audit correlation (a
  startup `warn` fires when it is lower).
  - **Upgrade note:** the new retention cap only prunes `opcgw.log.*`. Any
    pre-upgrade per-module files (`opc_ua.log.*`, `storage.log.*`, etc.) are
    left in place — delete them once after upgrading to reclaim the space.

## [2.3.0] — 2026-06-24 — Epic F: onboarding & web UX for public release

> **Status:** released — tagged `v2.3.0` and published to Docker Hub
> (`2.3.0` / `2.3` / `latest`) and GHCR (multi-arch amd64 + arm64); GitHub
> release published. Epic F (stories F-0…F-4) is complete; all gates green
> (`cargo test` 38 suites / 0 failed, `cargo clippy --all-targets -D warnings`
> clean) and the mandatory epic security review came back **CLEAN** (0 HIGH /
> 0 MEDIUM / 3 LOW). The real-world onboarding smoke (empty-config boot →
> first-run wizard → ChirpStack connect → Apply soft-restart → config
> export/import round-trip) **passed on `v2.3.0-rc1`** before promotion. This
> release makes opcgw configurable entirely from the browser and removes the
> "restart on every config change" churn.

### Added

- **Zero-touch first-run wizard** (Epic F / F-2). `/setup` now captures the full
  first-boot configuration from the browser — ChirpStack `server_address` /
  `tenant_id` / `api_token` **and** the OPC UA password — so a fresh checkout
  with an empty `config.toml`/`.env` boots fully configured with **no text-file
  editing**. Secrets are written to `config/secrets.toml` (chmod `0600`, atomic
  temp+rename); non-secret config goes to SQLite. `AppConfig::validate()` and
  `is_first_run()` now carve out missing ChirpStack credentials so a pristine
  config boots into the wizard instead of aborting. Web-UI login user/password
  and the log-file location remain in `.env` by design.
- **Config export / import** (Epic F / F-4). `GET /api/config/export` downloads
  the full configuration (the four singleton sections + the
  applications/devices/metrics/commands tree) as a portable TOML file with
  **secrets excluded** (`api_token` / `user_password` are never serialized).
  `POST /api/config/import` accepts a `{ "toml": … }` JSON envelope, merges it
  over the current config via figment (so the target instance keeps its own
  secrets — import never carries or overwrites them), validates the candidate,
  and **stages** it through the Apply flow (the whole import is one atomic
  EXCLUSIVE transaction — all-or-nothing). New `toml` dependency + `Serialize`
  derives enable SQLite→TOML serialization.
- **Unified web shell** (Epic F / F-1). A shared `static/shell.js` injects one
  navigation/header bar on every operator page (active link derived from the
  path), replacing the hand-duplicated `<nav>` across 9 pages, plus shared
  component CSS (`.app-shell` / `.btn` / `.status-badge` / `.banner`). Vanilla
  JS — **no build step, no framework, no `node_modules`**.
- **Redesigned dashboard landing page** (Epic F / F-3). The landing page leads
  with an at-a-glance health verdict (OK / specific degraded reason), a
  poller-status tile (stall detection against the configured poll interval, via
  a new `poll_interval_secs` field on `GET /api/status`), and a per-device data
  freshness panel (fresh / stale / bad / never). All rollups are derived
  client-side from the existing `/api/status` + `/api/devices` payloads — no
  gateway-side aggregation.

### Changed

- **Staged configuration with an explicit "Apply changes" soft restart**
  (Epic F / F-0). Config edits (the singleton-config editor **and** the
  application/device/metric/command CRUD handlers) now **stage** to SQLite
  without restarting the running gateway; `GET /api/status` reports
  `pending_changes: true` until applied. A single `POST /api/config/apply`
  performs **one** graceful **in-process** soft restart of the data-plane
  (poller, OPC UA server, gRPC event stream, command-timeout handler) — the
  **container is never restarted**, and OPC UA clients reconnect once per batch
  rather than on every edit. The config is re-read and validated **before**
  teardown, so a bad config is non-disruptive (the running data-plane keeps
  serving). The restart-required allowlist is gone; all settings apply uniformly
  on Apply.
- **Pooled SQLite connections now set a 5 s `busy_timeout`** (Epic F / F-4
  review, [#141](https://github.com/guycorbaz/opcgw/issues/141)). Concurrent
  `BEGIN EXCLUSIVE` writers (a config import racing a CRUD save or the
  Apply-triggered reload) now wait for the lock instead of failing immediately
  with `SQLITE_BUSY` → a spurious HTTP 500. Hardens all eight EXCLUSIVE writers.

### Fixed

- **Uplink event-stream device set is recomputed on every Apply**
  ([#138](https://github.com/guycorbaz/opcgw/issues/138)). Adding or removing a
  device no longer requires a manual restart to take effect on the gRPC
  `StreamDeviceEvents` subscription — the stream task is torn down and respawned
  with the current device set as part of the soft restart.
- **`command_class` is preserved on config import** (Epic F / F-4 review). The
  Epic E valve device-class binding was previously dropped on an export→import
  round-trip; both the import path and the boot migration now persist it.

### Security

- Mandatory epic-completion security review: **CLEAN** (0 HIGH / 0 MEDIUM /
  3 LOW). Secrets never reach SQLite, logs, or the config export; all new web
  endpoints are authentication- and CSRF-gated; the first-run wizard bypass is
  an exact-match allowlist; SQL is fully parameterized; the Apply path
  re-validates before teardown with revert-on-failure. The three LOW items are
  defense-in-depth follow-ups recorded in `deferred-work.md`.

## [2.2.0] — 2026-06-13 — Epic E: model-agnostic, class-aware device abstraction

> **Status:** released — tagged `v2.2.0` and published to Docker Hub
> (`2.2.0` / `2.2` / `latest`) and GHCR; GitHub release published. All v2.2.0
> story gates passed on hardware: rc4 proved the **full Fuxa-driven valve
> OPEN+CLOSE cycle** (2026-06-11, E-0 / AC#10 → **Story E-0 `done`**), rc2
> validated the de-aggregated read path, and **rc5 passed the cold-start gate
> in production** (2026-06-12, E-1 / AC#11 → **Story E-1 `done`** — SQLite
> metric restore before poller start, backfill freshness-guard correct-skip,
> live uplinks flowing with zero stream drops;
> [#130](https://github.com/guycorbaz/opcgw/issues/130) closed). rc6 added the
> CommandStatusPoller NULL-row fix
> ([#134](https://github.com/guycorbaz/opcgw/issues/134)) and **soaked clean
> in pre-production**: the recurring `Failed to query pending command
> confirmations` ERROR is gone (zero ERROR lines on 06-13, poller alive),
> clearing the last gate to stable.

### Added

- **Downlink command path wired end-to-end** (Epic E / E-0). An OPC UA write to a
  command node is now delivered to the device as a LoRaWAN downlink via
  ChirpStack's `DeviceService.Enqueue`. The poller drains the command queue that
  the OPC UA write path feeds and transitions each command `Pending → Sent`
  (failures → `Failed`, batch continues; full delivery confirmation is E-3).
- **Uplink-event ingestion — last-known value, no aggregation** (Epic E / E-1,
  [#130](https://github.com/guycorbaz/opcgw/issues/130)). New `chirpstack_events`
  runtime task consumes ChirpStack's decoded uplink events
  (`InternalService.StreamDeviceEvents`) and stores each configured metric's
  **raw last value stamped with the device's source timestamp** — no averaging
  or summing. The metrics poll (`GetMetrics`) time-aggregates and corrupted
  discrete valve state (e.g. `valveStatusCode` aggregated to a nonsense `391`);
  it now **skips** streamed devices, and the OPC UA `source_timestamp` reflects
  the device report time. Valve-class devices stream by default (E-1a); set
  **`chirpstack.stream_all_devices = true`** to de-aggregate the whole fleet
  (E-1b). `uplink_metric_never_seen` warns when a configured metric never
  appears in a device's decoded object (e.g. DevStatus-sourced battery).
- **Tonhe E20 valve codec fixes** ([#131](https://github.com/guycorbaz/opcgw/issues/131)).
  `decodeUplink` no longer errors on empty (0-byte) confirmed-downlink ACK
  uplinks, and emits integer measurements (`valveStatusCode` / `valvePosition` /
  `moving` / `fault` / `lowBattery`). Renamed to `tonhe-e20-valve-codec.js`.
- **`command_class` config field** on `[[application.device.command]]`. When set
  to `"valve"`, opcgw enqueues a **semantic command object**
  (`1` = open → `{"command":"open"}`, `0` = close → `{"command":"close"}`) and
  the ChirpStack device-profile codec produces the wire bytes — keeping opcgw
  model-agnostic. Absent = legacy raw-byte path (backward compatible).
- **Schema migration v011** adds the `command_class` column to the `commands`
  table (round-trips through the SQLite application store).
- **`command_class` settable from the web command editor + device-class registry**
  (Epic E / E-2a, closes [#135](https://github.com/guycorbaz/opcgw/issues/135)).
  The command CRUD API (`POST`/`PUT`) and the `commands.html` editor now expose a
  `command_class` selector, so a valve command can be bound to `"valve"` from the
  UI — previously the web layer hard-coded it to `None`, so a Fuxa/OPC UA close
  went out as an invalid raw `0x00` instead of the codec's `0x02`. The class is
  validated against a **device-class registry** (new `src/device_registry.rs`: a
  `DeviceDriver` trait + `ClassRegistry`, valve as the first Tier-1 driver) at
  **both** the web layer and `AppConfig::validate()` config-load — one source of
  truth shared with the runtime downlink dispatch. The concrete valve mapping
  moved behind the registry with zero behaviour change. (E-2b — a Tier-2
  object-remap adapter, `command_kind`/SetLevel, and a second device class — is
  deferred to backlog until a concrete second model/class exists.)
- **Per-device OPC UA stale threshold** ([#132](https://github.com/guycorbaz/opcgw/issues/132)).
  Optional `stale_threshold_seconds` on `[[application.device]]` overrides the
  global `[opcua].stale_threshold_seconds` (default 120 s) for that device only —
  set it above a slow LoRaWAN sensor's report period so it reads `Good` instead
  of `Uncertain` between uplinks. Schema **migration v012** adds the column;
  resolution is device-override → global → default. Restart-required.
- **Orphan-warn for stream-covered metrics** (Epic E / E-1b). When
  `stream_all_devices` is on, `uplink_metric_never_seen` (warn) flags a
  configured metric not yet present in a device's decoded object (e.g.
  DevStatus-sourced battery, or a field-name mismatch); a sibling
  `uplink_metric_now_seen` (info) self-corrects the warning if the field is
  merely intermittent and later arrives.
- **Startup/reconnect backfill** (Epic E / E-1b, rc5). On every successful
  stream (re)connect, opcgw fetches the device's newest recent uplink via the
  bounded recent-events read (real decoded events — never the aggregating
  `GetMetrics`) and stores it, so a correct last-known value is on OPC UA
  **immediately after a (re)start** instead of after the device's next report
  (which can be 15–20 min away). New events: `uplink_backfill` /
  `uplink_backfill_empty` / `uplink_backfill_skipped` / `uplink_backfill_failed`.
- **Freshness guard on the whole event-stream value path** (E-1 code review,
  rc5). ChirpStack **replays recent event history on every stream (re)connect**;
  all stream writes (live pump **and** backfill) now pass an `is_fresher`
  timestamp guard, so a replayed or out-of-order event can never regress a
  last-known value — the value path is monotonic by device-report time
  (`uplink_replay_skipped` debug traces the discards). On a storage read error
  the guard **fails open, audibly** (`uplink_guard_read_failed` warn): a
  transient unverified write beats permanently freezing a metric on a
  self-repairable fault. Review hardening also: cross-application DevEUI
  dedup with **merged** metric lists (+ `uplink_metric_type_conflict` warn),
  per-device `stale_threshold_seconds` range validation (0, 86400] + negative
  DB-value guard (`storage_invalid_stale_threshold`), strict `Bool` 0/1 and
  integral-only `Int` JSON coercions (codec mismatches warn instead of silent
  truthiness/truncation), `uplink_event_dropped` diagnostics for malformed
  uplinks, and cancellation-aware connect/backfill (clean shutdown). The
  gRPC stream now sits behind an injectable `UplinkSource` seam with
  reconnect/backfill/no-regression tests.

### Fixed

- **CommandStatusPoller no longer errors every 5 s on OPC-UA-queued downlinks**
  ([#134](https://github.com/guycorbaz/opcgw/issues/134), rc6). Commands queued
  by an OPC UA write left `command_name`/`parameters`/`command_hash` NULL in
  `command_queue`, and the confirmation/timeout readers mapped those nullable
  columns as non-NULL types — one such row failed the **entire** poll
  (`Failed to query pending command confirmations … Invalid column type Null`),
  killing delivery-confirmation tracking and spamming an ERROR every 5 s. All
  four command readers now share a single NULL-safe row mapper (corrupt
  `parameters` JSON also soft-fails per row instead of collapsing the batch),
  and the OPC-UA write path persists `command_name` + `enqueued_at`. Existing
  databases are handled as-is — no migration or manual cleanup needed.

## [2.1.0] — 2026-05-28 — web-first configuration & auto-discovery

> **Status:** released — tagged `v2.1.0` and published to Docker Hub
> (`2.1.0` / `2.1` / `latest`) and GHCR; GitHub release published.

v2.1.0 changes how opcgw is configured. Through v2.0 you hand-edited
`config/config.toml` and restarted the gateway. From v2.1.0 the gateway is
configured **from the browser**: on first run it serves a setup wizard, and all
configuration is stored in **SQLite**. `config.toml` is demoted to an *optional
bootstrap seed* — read once on first start to populate the database, then never
read or mutated again. This is the combined delivery of **Epic C** (auto-discovery
and web-first configuration) and **Epic D** (singleton configuration → SQLite).

Existing v2.0 deployments keep working: an existing `config.toml` is migrated
into SQLite automatically on first boot of v2.1.0, so no manual conversion is
required.

### Added

- **First-run web setup wizard** (Epic C / C-0). A gateway started with an empty
  or placeholder `config.toml` boots into a first-run mode and serves a setup
  wizard on the web port. The operator enters the ChirpStack connection + API
  token and OPC UA endpoint settings in the browser; secrets are written to
  `config/secrets.toml` (created `chmod 0600`) or supplied via environment
  variables. Until configured, the OPC UA server rejects all sessions and the
  web surface exposes only the wizard (a fixed allowlist of setup endpoints).
  Completing the wizard triggers a graceful in-process restart.
- **ChirpStack inventory auto-discovery** (Epic C / C-1, C-2). The web UI can
  query the connected ChirpStack server for its applications, devices, and
  device profiles, and presents them as **pickers** — so you select real
  applications/devices by name instead of pasting UUIDs and DevEUIs by hand.
  Metric wire-type is inferred and surfaced during mapping. A manual-entry
  fallback remains for offline editing.
- **Duplicate-prevention validator** (Epic C / C-3). Creating or editing
  applications, devices, and metric mappings is validated against existing
  entries to prevent duplicate names / OPC UA node collisions before the change
  is persisted.
- **Inventory drift view** (Epic C / C-4). A web page diffs the gateway's
  configured inventory against what ChirpStack currently reports (added /
  removed / changed / in-sync), with deep links into the application and device
  editors and a degraded-mode banner when ChirpStack is unreachable.
- **Singleton configuration editor** (Epic D / D-1). The `[global]`,
  `[chirpstack]`, `[opcua]`, and `[web]` settings are editable from the web UI,
  with explicit handling for knobs that require a restart to take effect.

- **Version shown in the web UI** (#128). The dashboard subtitle now displays the running version after "ChirpStack → OPC UA gateway", and `GET /api/health` includes a `version` field (`{"status":"ok","version":"…"}`).

### Changed

- **All configuration now lives in SQLite.** Applications, devices, metric
  mappings, and command definitions (Epic C / C-6) plus the singleton
  `[global]` / `[chirpstack]` / `[opcua]` / `[web]` sections (Epic D / D-0)
  are stored in the gateway database. Configuration precedence is
  **environment variable > SQLite > `config.toml` > built-in default**,
  implemented as a layered figment provider (D-2).
- **`config.toml` is bootstrap-only.** It is read once on first start to seed an
  empty database and is never written back to. The previous "edit `config.toml`
  and the running gateway picks it up" model — including the SIGHUP reload
  listener and the `toml_edit` write-back path — has been **removed**. Edit
  configuration through the web UI (or environment variables) instead.
- **SQLite schema bumped to v010.** v009 adds the configuration tables for
  applications/devices/metrics/commands (Epic C / C-6); v010 adds the
  key/value singleton-configuration tables (Epic D / D-0). Migration is
  automatic and forward-only on first boot.
- **`secrets.toml` replaces inline tokens.** The ChirpStack API token and OPC UA
  credentials are sourced from `config/secrets.toml` (mode `0600`) or
  environment variables rather than being stored alongside non-secret config.

### Fixed

- **A single device's metric error no longer marks the whole ChirpStack server unavailable** (#126). A per-device `GetDeviceMetrics` failure (e.g. ChirpStack returning an `Internal` status — `"Odd number of digits"` — for a malformed DevEUI) was treated as a gateway-wide outage: it flipped `chirpstack_available=false` and triggered the recovery loop every poll cycle even though ChirpStack was reachable and other devices polled fine. The outage decision is now gated on a live connectivity probe — a server-responded error is counted as a per-device error but leaves `chirpstack_available=true`.
- **Device picker now captures the DevEUI; placeholder DevEUIs are rejected** (#125). Root cause: the web device picker read `item.id` from the `/api/inventory/devices` response, but that struct's field is `dev_eui` — so `item.id` was `undefined` and every picker-created device was POSTed with `device_id="undefined"` (a device that can never poll — ChirpStack rejects it with `"Odd number of digits"`). The picker now reads `item.dev_eui`, and as defence-in-depth the device CRUD API rejects `device_id` capture-failure sentinels (`undefined`, `null`, `NaN`, `none`). Real DevEUIs and existing free-form ids are unaffected.
- **Web-configured applications now survive a gateway restart** (#123). Applications/devices/metrics created through the web UI are stored in SQLite, but on restart the poller, the in-memory storage skeleton, and the OPC UA address space were rebuilt from the `config.toml` bootstrap *seed* — the SQLite-stored topology was loaded into the live watch channel only, never folded back into the construction-time config. The gateway therefore reverted to the seed on every restart (the data stayed safe in SQLite but vanished from the running gateway, and SQLite metric restore orphaned against the seed skeleton). Startup now sources `application_list` from SQLite (when present) before constructing those subsystems, making SQLite authoritative for the application topology across restarts.
- **ChirpStack TCP availability probe now resolves DNS hostnames** (#122). The
  pre-flight connectivity probe used `SocketAddr::parse()`, which only accepts a
  numeric `IP:port` and rejected service names such as `http://chirpstack:8080`
  with `invalid socket address syntax` — even though the gRPC client resolves
  them fine. It now uses `to_socket_addrs()` (and tries each resolved IPv4/IPv6
  address until one connects), so a Docker/Compose service name on a shared
  network works for `chirpstack.server_address` as documented.

### Notes

- No public OPC UA address-space or `/api/metrics` wire-format changes relative
  to v2.0 — this release is about how the gateway is *configured*, not how
  metrics are exposed.
- Documentation refreshed across `README.md`, the GitHub Pages site under
  `docs/`, the DocBook user manual, and the Docker Hub Overview to describe the
  web-first install/configuration flow.

---

## [2.0.2] — 2026-05-21 — expanded Docker Hub Overview

The 2.0.1 release published Docker images correctly but `peter-evans/dockerhub-description@v4` synced the v2.0.1-pinned (thin) Overview content to <https://hub.docker.com/r/gcorbaz/opcgw>. 2.0.2 is a docs-only patch that pushes a substantially expanded Overview page to Docker Hub on tag — same image bytes, better landing page for first-time visitors.

### Changed

- **Docker Hub Overview page** (`docs/dockerhub-description.md`): expanded from 190 → 298 lines. Added sections for the name-translation-gateway rationale ("Why opcgw vs. ChirpStack's built-in integrations?"), an ASCII data-flow architecture diagram, a feature breakdown across OPC UA + web UI + gateway-operations + persistence, audience-targeting ("Who is this for?"), a 6-row troubleshooting table, and indicative scale + performance numbers from a Raspberry Pi 4 reference deployment. Corrected the Supported-tags example from `2.0.0` to `2.0.1` with an explicit note that `:2.0.0` returns `manifest unknown`.
  ([044a3d3](https://github.com/guycorbaz/opcgw/commit/044a3d3))

### Notes

- No code changes between 2.0.1 and 2.0.2 — `cargo test --all-targets` output, image bytes, and runtime behaviour are identical to 2.0.1. The version bump exists solely to trigger the `v*`-tagged `peter-evans/dockerhub-description@v4` sync step.
- Operators with `:2.0` pinned will auto-receive the same image bytes 2.0.1 published (no re-pull needed unless you want the SHA-pinned tag).

---

## [2.0.1] — 2026-05-20 — first usable release

`v2.0.0` was tagged on 2026-05-20 but never produced Docker images: the
publishing workflow `.github/workflows/docker-build.yml` failed in 0 s on
every push because step-level `if:` conditions referenced the `secrets`
context, which GitHub Actions rejects at workflow-schema-validation time
(the `secrets` context is not in the list of contexts allowed in `if:`
expressions). The workflow YAML parsed locally with `python3 yaml.safe_load`
and was only caught by `actionlint`. Same end-to-end real-world test pass
also surfaced a second-order bug in the Epic D D-0 empty-TOML-startup
fix: the validator accepted empty `application_list` but serde rejected
the TOML at deserialization time because the field lacked `#[serde(default)]`.

`v2.0.1` rolls up the two patches plus the four bug-fix commits from the
v2.0 walkthrough. **This is the first version of v2.0 with published
Docker images.** Pulling `gcorbaz/opcgw:2.0` resolves to this release.

### Fixed

- **CI**: replaced `secrets.X` in step-level `if:` conditions in
  `.github/workflows/docker-build.yml` with step-output indirection. The
  `Detect Docker Hub credentials` step reads the secrets via `env:` (which
  IS allowed to reference secrets), exports a `have_creds` step output,
  and the four downstream conditional steps gate on that output instead.
  The GHCR-only-fallback semantics from iter-2 U1 are preserved.
  ([119e16f](https://github.com/guycorbaz/opcgw/commit/119e16f))
- **Config**: added `#[serde(default)]` to `AppConfig::application_list`
  so a TOML file with zero `[[application]]` blocks deserializes to an
  empty vec instead of failing with `missing field "application"` before
  the validator's allow-empty branch is reached.
  ([cecd100](https://github.com/guycorbaz/opcgw/commit/cecd100))

### Notes

- All v2.0.0 commits are also in v2.0.1 — same code, plus the two
  hotfix commits above.
- The `v2.0.0` tag remains in the repository for historical traceability;
  pulling `gcorbaz/opcgw:2.0.0` from Docker Hub will fail with `manifest
  unknown` because no image was ever published for that tag. Use
  `gcorbaz/opcgw:2.0` (floating) or `gcorbaz/opcgw:2.0.1` (exact).

---

## [Unreleased] — v2.0.0

This is a **major** release. v2.0 ships the Phase A reliability foundation, the
Phase B real-time + web feature set, and the Epic A storage payload migration
that closes [issue #108](https://github.com/guycorbaz/opcgw/issues/108) — the
payload-less `MetricType` enum that flattened every persisted metric value to
its discriminant string instead of the real measurement. **Before Epic A,
opcgw never persisted real measurement values; it persisted only the data-type
discriminant string ("Float", "Int", "Bool", "String").** Epic A closes that
gap end-to-end through the storage trait, the SQLite schema, the poller, both
OPC UA Read paths, and the web dashboard.

Operators upgrading from a v2.0-rc deployment **must** follow the migration
runbook in [`docs/deployment-guide.md` § "Epic A migration"][epic-a-runbook]
and may use [`scripts/check-schema-version.sh`][schema-script] as a pre-flight
check.

### Removed (BREAKING)

- **`opcgw::storage::MetricValue.value: String` field removed.** The transitional
  stringly-typed value field that held the discriminant name (`"Float"`,
  `"Int"`, `"Bool"`, `"String"`) is gone; the real measurement now lives in the
  payload-bearing `MetricType` enum. External consumers constructing
  `MetricValue` via struct literals must update.
- **`opcgw::storage::MetricValueInternal.value: String` field removed** (same
  reason).
- **`opcgw::storage::BatchMetricWrite.value: String` field removed** (same
  reason).
- **`HistoricalMetricRow.value: String` field replaced by
  `payload: Option<MetricType>`.** Legacy pre-v2.0 history rows surface as
  `None` (rendered to OPC UA clients as
  `DataValue { value: None, status: BadDataUnavailable }`).
- **`MetricType` no longer implements `Copy`.** Carrying owned `String` payloads
  required dropping the `Copy` bound; all variant constructions and pattern
  matches must `clone()` when both sides need ownership.

### Changed (BREAKING)

- **`opcgw::storage::MetricType` is now payload-bearing.** Variants changed
  from `MetricType::Float` (unit) to `MetricType::Float(f64)`,
  `MetricType::Int(i64)`, `MetricType::Bool(bool)`, `MetricType::String(String)`.
  `Display` and `FromStr` are preserved with a documented zero-default contract
  on `FromStr`.
- **`opcgw::opc_ua::convert_variant_to_metric` signature simplified** to
  `Result<MetricType, OpcGwError>` (no longer threads a separate value
  argument).
- **SQLite schema bumped from v006 to v008.** v007 adds typed value columns
  (`value_real REAL NULL`, `value_int INTEGER NULL`, `value_bool INTEGER NULL`,
  `value_text TEXT NULL`, `value_type TEXT NOT NULL DEFAULT 'legacy'`) plus
  column-level `CHECK` constraints to both `metric_values` and `metric_history`.
  v008 adds an exactly-one-non-NULL cross-column `CHECK` enforced via
  `CREATE TABLE … AS SELECT` wrapped in `BEGIN`/`COMMIT`. **Rollback is
  one-way**: only path is restoring a pre-upgrade backup file (documented in
  the migration runbook).
- **Web `/api/metrics` JSON shape changed.** `MetricView.value` widened from
  `Option<String>` to `Option<serde_json::Value>` (typed primitives: Float and
  Int as JSON numbers, Bool as JSON boolean, String as JSON string). New
  optional `unit: Option<String>` field surfaces the configured `metric_unit`.
  `MetricView` and sibling response structs no longer derive `PartialEq, Eq`
  (`serde_json::Value` cannot implement `Eq` over the NaN axis).
- **Web dashboard Bool wire format** shifted from `"1"`/`"0"` (A-5 transitional)
  to native `true`/`false`.

### Added

- **Dual-registry container image publishing.** The Docker image is published in
  lockstep to both **Docker Hub** (`docker.io/gcorbaz/opcgw`) and **GHCR**
  (`ghcr.io/guycorbaz/opcgw`) on every `v*` tag. Both registries receive
  identical multi-architecture manifest lists.
- **Multi-architecture images.** Built for `linux/amd64` and `linux/arm64`,
  covering x86_64 servers, Raspberry Pi 4/5, AWS Graviton, and Apple Silicon
  development machines. (32-bit ARM is not currently published.)
- **Dockerfile hardening.** Runtime base pinned from `ubuntu:latest` to
  `ubuntu:24.04` (LTS); the container now runs as non-root user `opcgw`
  (UID 10001). Operators bind-mounting host directories must `chown -R
  10001:10001 ./config ./pki ./log` before first start and apply the NFR9 PKI
  permissions (`chmod 700 ./pki/private`, `chmod 600 ./pki/private/*`).
- **Docker Hub Overview page** sourced from `docs/dockerhub-description.md`
  and auto-synced via `peter-evans/dockerhub-description@v4` on every `v*`
  tag, keeping the live page version-controlled in git.
- **DocBook 4.5 user manual brought current to v2.0** at
  `docs/manual/opcgw-user-manual.xml`. New / rewritten chapters: Installation
  (Docker Hub, GHCR, Docker Compose, systemd, build-from-source,
  post-install verification); Configuration (config.toml schema with
  field-by-field tables for `[chirpstack]`, `[opcua]`, `[web]`, and
  `[[application]]` / `[[application.device]]` / `[[application.metric]]`,
  plus logging configuration); Troubleshooting (seven new operator scenarios
  with structured-log-event grep recipes); new **Upgrade and migration**
  chapter referencing the Epic A migration runbook (Path A in-place vs Path B
  drop-and-recreate). Audit-event taxonomy section cross-references
  `docs/logging.md` for the complete closed-enum reason list. Closes the
  long-standing "manual XML 4 epics behind" deferred-work entry (Epic A retro
  AI-A-8).
- **`docs/manual/Makefile`** wrapping the standard DocBook XSL toolchain.
  `make html` produces chunked multi-page HTML, `make html-single` produces a
  single-page HTML, `make pdf` produces a PDF via `dblatex`, `make validate`
  runs DocBook 4.5 DTD validation only. Allows headless and CI manual builds
  without the oXygen editor (the existing `opcgw.xpr` project remains for
  authoring workflow).
- **Official logo pack** at `docs/logo/` (`opcgw-mark.svg`,
  `opcgw-horizontal.svg`, `opcgw-favicon.svg`) — embedded in the repo
  README, the DocBook manual title page, and the Docker Hub Overview page.
- **Real measurement values persisted and round-tripped end-to-end** for the
  first time in the project's history. OPC UA `Read` returns
  `Variant::Float(23.5_f32)` (or `Variant::Int32` / `Variant::Boolean` / `Variant::String` depending on `[application.metric.metric_type]`) instead of the legacy `Variant::String("Float")` type-tag placeholder; `HistoryRead`
  returns the value-over-time series in typed `Variant`s; the web dashboard
  renders `34.2 %` instead of `Float`.
- **Pre-Epic-A row handling.** Rows migrated from v006 are tagged
  `value_type='legacy'` and surface as `BadDataUnavailable` (OPC UA) or the
  "missing" badge (web dashboard) for one poll interval; the next poll cycle
  replaces them with real typed payloads. **Legacy rows are not silently
  dropped**: in `HistoryRead` they appear as `DataValue { value: None,
  status: BadDataUnavailable }` in the response stream.
- **NaN/Inf filter at the poller boundary** (`event="metric_parse"`,
  `reason="non_finite"`, warn level). Prevents downstream `Variant::Float(NaN)`
  serialization hazards.
- **Five new structured-log audit events** (closed-enum `reason=*` taxonomy
  documented in `docs/logging.md`):
  - `metric_parse` — poller-side payload conversion failures.
  - `metric_read` — OPC UA `Read` payload conversion (e.g.,
    `reason="narrowing_overflow"` for f64 → f32 narrowing).
  - `metric_history_read` — per-row OPC UA `HistoryRead` payload conversion.
  - `metric_history_summary` — aggregate-per-request `HistoryRead` skip
    counts (trace level; replaces per-row floods).
  - `metric_view_serialize` — web-layer JSON serialization issues
    (`non_finite`, `int_precision_lossy`, `f32_overflow`, `f32_underflow`).
- **Operator-facing migration runbook** in
  [`docs/deployment-guide.md` § "Epic A migration"][epic-a-runbook]: Path A
  (in-place auto-migration) / Path B (drop-and-recreate), pre-upgrade
  checklist, post-migration verification, rollback contract, SLA expectation,
  and 6 common gotchas.
- **`scripts/check-schema-version.sh`** pre-flight POSIX shell script
  (new top-level `scripts/` directory). Wraps `sqlite3 PRAGMA user_version`
  with operator-friendly Path A/B recommendations and an opcgw-schema-shape
  pre-check that prevents misidentifying non-opcgw SQLite files (Firefox
  `places.sqlite`, etc.) as pre-Epic-A databases.
- **`web::MetricSpec.metric_unit: Option<String>`** propagated from
  `[[application.metrics]].metric_unit` through the existing hot-reload
  pipeline. Empty-string units coalesce to no-suffix on the dashboard.
- **Compile-time field-shape pins** in `src/storage/types.rs` via
  `const _: fn(&T) = |v| { let MetricType::Float(_) = v else { return; }; ... }`
  force compile errors if `MetricType` variants are restructured.

### Fixed

- **Counter monotonic reset detection** now reads the real persisted value
  instead of a zero-default discriminant (was silently disabling reset
  detection because `get_metric_value` returned `Int(0)` for the legacy
  column path).
- **Saturation guard for f64 → i64 conversion** correctly rejects the
  `i64::MAX as f64 == 2^63` rounding-up case (uses `>=` not `>`).
- **Float narrowing overflow / underflow at the OPC UA boundary** now emits
  `event="metric_read"` with `reason="narrowing_overflow"` /
  `reason="narrowing_underflow"` warn lines instead of silently producing
  `Float(0.0)`.

### Security

- Inline security review at Epic A close: **clean** (no HIGH/MEDIUM findings).
  One LOW finding in the migration runbook (a destructive-`rm` glob example
  that could have removed operator backup files) was patched in the same
  retrospective commit.
- v008 migration is `BEGIN`/`COMMIT`-wrapped for crash safety. Note: the
  outer v001 → v008 runner is not yet transactional (pre-existing limitation
  tracked for the next migration story).
- Strict-zero invariant honored throughout Epic A: no commit touched
  `src/web/auth.rs`, `src/security*.rs`, `src/opc_ua_auth.rs`,
  `src/opc_ua_session_monitor.rs`, or `src/main.rs::initialise_tracing`.

### Upgrade notes

External Rust consumers of `opcgw::storage`:

```rust
// Before (v2.0-rc):
let mv = MetricValue {
    device_id: "dev1".into(),
    metric_name: "moisture".into(),
    value: "Float".to_string(),    // <-- discriminant string
    data_type: MetricType::Float,  // <-- unit variant
    timestamp: Utc::now(),
};

// After (v2.0):
let mv = MetricValue {
    device_id: "dev1".into(),
    metric_name: "moisture".into(),
    // `value` field removed — payload now lives inside `data_type`.
    data_type: MetricType::Float(34.2),  // <-- payload-bearing
    timestamp: Utc::now(),
};
```

Operators upgrading a v2.0-rc deployment: follow [the migration
runbook][epic-a-runbook]. Both Path A (auto-migration preserving legacy rows
as `BadDataUnavailable` for one poll cycle) and Path B (drop the database
file before upgrade) are supported and documented.

[epic-a-runbook]: ./docs/deployment-guide.md
[schema-script]: ./scripts/check-schema-version.sh

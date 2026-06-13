# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

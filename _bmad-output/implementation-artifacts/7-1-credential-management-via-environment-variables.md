# Story 7.1: Credential Management via Environment Variables

**Epic:** 7 (Security Hardening)
**Phase:** Phase A
**Status:** done
**Created:** 2026-04-28
**Last Validated:** 2026-04-28 (validate pass 2 — extended AC#1 verification grep to catch device EUIs, fixed Task 5 cross-reference, replaced interactive `cargo doc --open` with non-interactive audit recipe, removed unnecessary `traced_test` hedge, tightened README Planning-table update timing to per-commit)
**Validation history:** pass 1 (2026-04-28) added AC#5 tonic-leak audit, dropped tenant_id from Debug redaction matrix, added Task 0 for GitHub issues + Task 5 for tonic audit, added migration paragraph, plus 11 other refinements.
**Author:** Claude Code (Automated Story Generation)

> **Source-doc note (numbering offset):** `_bmad-output/planning-artifacts/epics.md` was authored before Phase A was renumbered. The story this file implements lives in `epics.md` as **"Story 6.1: Credential Management via Environment Variables"** under **"Epic 6: Security Hardening"** (lines 613–634). In `sprint-status.yaml` and the rest of the project this is **Story 7-1** under **Epic 7**. Both refer to the same work.

---

## User Story

As an **operator**,
I want API tokens and passwords loaded from environment variables by default,
So that secrets are never committed to config files or exposed in logs.

---

## Objective

Make secret-by-environment-variable the **documented default deployment pattern** and harden the gateway so that:
1. The shipped `config/config.toml` contains **no real credentials** — placeholders only.
2. Any future code path that prints/Debug-formats `ChirpstackPollerConfig` or `OpcUaConfig` cannot leak `api_token` or `user_password`.
3. Operators have a clear, working Docker / `.env` recipe for injecting secrets.
4. A startup failing because of missing secrets produces an actionable error, not a panic.

This is a **hardening + documentation** story. The figment env-override wiring and the empty-secret validation **already work today** — see "Existing Infrastructure (DO NOT REINVENT)" below. Most of the new code is small.

---

## Out of Scope

- OPC UA security endpoints / Basic256 Sign / SignAndEncrypt → **Story 7-2**.
- OPC UA username/password authentication of clients → **Story 7-2**.
- PKI permissions / `create_sample_keypair` default flip → **Story 7-2**.
- Connection limiting → **Story 7-3**.
- Switching `String` → `secrecy::SecretString` for the field types. Adds a `.expose_secret()` call at every consumer in `src/chirpstack.rs` and `src/opc_ua.rs` for marginal benefit on top of the redacted-`Debug` approach below. **Defer** unless a follow-up story requires zeroize-on-drop guarantees. Record in `deferred-work.md` if you start it and stop.
- Changing the figment env-var conventions. The existing nested form (`OPCGW_CHIRPSTACK__API_TOKEN`) is the canonical name. **Do not** introduce a parallel short form (e.g. `OPCGW_API_TOKEN`) — the precedent for short forms exists only for `OPCGW_LOG_DIR` / `OPCGW_LOG_LEVEL` because they're consumed by the bootstrap phase before figment runs.

---

## Existing Infrastructure (DO NOT REINVENT)

Read these before writing code:

| What | Where | Status |
|------|-------|--------|
| `OPCGW_CHIRPSTACK__API_TOKEN` env override of `[chirpstack].api_token` | `src/config.rs:641-649` (figment merge) | **Works today.** Pinned by `test_chirpstack_nested_env_override` (`src/config.rs:1561-1612`). |
| `OPCGW_OPCUA__USER_PASSWORD` env override of `[opcua].user_password` | same merge | **Works today.** Pinned by `test_opcua_nested_env_override_sensitive_field` (`src/config.rs:1618-1662`). |
| Env > TOML precedence (figment merges env last) | `src/config.rs:642-643` | **Works today.** Same two tests above prove it. |
| Empty-string secret rejected at startup with clear error | `src/config.rs:693-695, 731-737` (`validate`) | **Works today.** |
| Startup error path: load failure → `error!` → process exits with `Err` (no panic) | `src/main.rs:370-376` | **Works today.** `AppConfig::from_path` returns `Err` → `main` propagates → clean exit code. |
| Two regression tests asserting secrets do not appear in tracing buffer during loader run | `src/opc_ua.rs:2152-2301` (`secrets_not_logged_from_config_startup`, `secrets_not_logged_from_appconfig_from_path`) | **Works today.** These pin the contract for the *loader* path. Story 7-1 extends the same approach to the rest of the binary. |
| `.gitignore` already excludes `.env`, `.env.local`, `config/config.local.toml` | `.gitignore` "# Config & Secrets" block | **Already in place.** Verify only — do not modify the existing block. |

**Epic-spec coverage map** — the BDD acceptance criteria from `epics.md` (lines 624-634) break down as follows:

| Epic-spec criterion | Already satisfied? | Where this story addresses it |
|---|---|---|
| `api_token` from env var | ✅ existing figment wiring | (no new work — referenced for completeness) |
| OPC UA passwords from env var | ✅ existing figment wiring | (no new work) |
| Default config has placeholder values only (NFR8) | ❌ current `config/config.toml` has real creds | **AC#1, AC#2** below |
| Env var takes precedence over config file | ✅ figment merge order | (no new work) |
| Secrets never in log output (NFR7) | ⚠️ partial — loader path only | **AC#3, AC#4** below |
| Missing required secrets → clear startup error (no panic) | ✅ existing `validate()` + `Err` propagation | **AC#2** extends with placeholder-detection error |
| FR42 satisfied | ✅ already by env wiring | **AC#5, AC#6** add operator ergonomics |

**Implication:** the new code surface for this story is small. Most of the work is **template sanitization (AC#1)**, **placeholder-detection validation (AC#2)**, **defence-in-depth `Debug` redaction (AC#3, AC#4)**, **tonic gRPC metadata leak audit (AC#5)**, **operator ergonomics + documentation (AC#6, AC#7)**, and **final verification (AC#8, AC#9)**.

---

## Acceptance Criteria

### AC#1: Default `config/config.toml` contains no real credentials (NFR8)
- The committed `config/config.toml` has the two **secret-bearing fields named in the epic spec** replaced with placeholder sentinel strings that the gateway recognises and refuses to start with:
  - `api_token = "REPLACE_ME_WITH_OPCGW_CHIRPSTACK__API_TOKEN_ENV_VAR"`
  - `user_password = "REPLACE_ME_WITH_OPCGW_OPCUA__USER_PASSWORD_ENV_VAR"`
- `tenant_id` is **also replaced** in the shipped template with a placeholder UUID (`"00000000-0000-0000-0000-000000000000"`) so operators don't accidentally publish the original tenant UUID. It is **not** redacted in `Debug` and **not** subject to placeholder-detection — the epic spec doesn't classify it as a secret. (See Dev Notes "Why tenant_id is replaced but not redacted" for the rationale and the deferred-work follow-up.)
- The `[[application]]` blocks in the shipped template are **collapsed to a single illustrative example** (one application, one device, one metric) so the default file does not embed the operator's specific deployment topology either. The current Arrosage / Bâtiments / Cultures / Meteo blocks are **moved** to `config/config.example.toml` (kept with a clear "example" header — see Tasks).
- **Verification:** `git diff config/config.toml` shows no real JWT, no real device EUI, no `user1` password, no `52f14cd4-c6f1-4fbd-...` tenant UUID. Run **both** of the following greps and confirm each returns nothing (or only the synthesized illustrative EUI you choose for the template, e.g. `"0000000000000001"`):
  ```bash
  # Catches the JWT prefix, password literal, and known tenant-UUID prefix.
  grep -E 'eyJ|user1|52f14cd4-c6f1-4fbd' config/config.toml
  # Catches any 16-char hex device EUI that isn't the all-zeros placeholder
  # or the synthesized illustrative EUI. Tweak the second negative match if
  # you pick a different placeholder.
  grep -Eo '[0-9a-f]{16}' config/config.toml | grep -vE '^0+1?$'
  ```

### AC#2: Placeholder-detection refuses to start with a clear error
- A new `validate()` rule in `src/config.rs::AppConfig::validate` rejects `api_token` and `user_password` if their value starts with the placeholder prefix `"REPLACE_ME_WITH_"`. The prefix is checked verbatim (no ellipsis substitution) so future placeholders following the same convention generalise.
- The error message names the field, names the env var to set, and points at the docs. Use the **literal prefix `"REPLACE_ME_WITH_"`** in the error text — do not substitute `"..."` for the rest of the placeholder and do not echo the operator's full literal value back (avoid log-injection-style risk if a near-miss real value is pasted):
  ```
  Configuration validation failed:
    - chirpstack.api_token: placeholder value detected (starts with "REPLACE_ME_WITH_"). Set OPCGW_CHIRPSTACK__API_TOKEN to inject the real secret. See docs/security.md.
    - opcua.user_password: placeholder value detected (starts with "REPLACE_ME_WITH_"). Set OPCGW_OPCUA__USER_PASSWORD to inject the real secret. See docs/security.md.
  ```
- The same code path that handles empty-string rejection (`src/config.rs:693-695, 731-737`) is extended — do **not** add a parallel validator pass; bolt onto the existing one to keep the error formatting consistent.
- **Verification:** Two unit tests:
  - `test_validation_rejects_placeholder_api_token` — config with placeholder, `validate()` returns `Err`, error message contains `"OPCGW_CHIRPSTACK__API_TOKEN"` and `"REPLACE_ME_WITH_"` literal.
  - `test_validation_rejects_placeholder_user_password` — symmetric.
  - One end-to-end test: `AppConfig::from_path` on `config/config.toml` with **no env vars set** must fail with the placeholder error; **with env vars set**, must succeed. (Uses `temp_env::with_vars`.)

### AC#3: `Debug` redaction belt-and-braces against future log leaks (NFR7)
- `ChirpstackPollerConfig` and `OpcUaConfig` get a **manual `Debug` impl** that redacts `api_token` and `user_password`. Replace the `#[derive(Debug, ...)]` with `#[derive(Deserialize, Clone)]` plus a hand-written `impl Debug` that emits `"***REDACTED***"` for those two fields and the existing values for everything else.
- The redaction string is the **exact 14-character literal `***REDACTED***`** (3 asterisks + the 8-letter word `REDACTED` + 3 asterisks = 14 chars, 6 asterisks total). Centralise it as `pub const REDACTED_PLACEHOLDER: &str = "***REDACTED***";` in `src/utils.rs` so tests can assert against the constant.
- Do **not** wrap `String` in a newtype (`SecretString` etc.). The custom `Debug` impl gives 95% of the protection at 5% of the diff cost. The `secrecy` crate is explicitly out of scope (see Out of Scope above).
- Field-level coverage matrix (epic-spec scope only):
  | Struct | Field | Redacted? | Why |
  |--------|-------|-----------|-----|
  | `ChirpstackPollerConfig` | `api_token` | yes | NFR7 secret |
  | `ChirpstackPollerConfig` | `tenant_id` | **no** | not classified as a secret by the epic spec; placeholder substitution in AC#1 is sufficient. Tracked as a future enhancement in `deferred-work.md` (see Dev Notes). |
  | `ChirpstackPollerConfig` | `server_address` | no | already in startup `info!` line at `src/main.rs:411-418`, well-established as non-secret |
  | `OpcUaConfig` | `user_password` | yes | NFR7 secret |
  | `OpcUaConfig` | `user_name` | no | not a secret in the OPC UA model |
  | `OpcUaConfig` | `certificate_path`, `private_key_path` | no | paths, not key material — but the **content** of `private_key_path` is sensitive (handled by Story 7-2 via NFR9 file-permission check) |
- **Verification:** Two unit tests:
  - `test_chirpstack_poller_config_debug_redacts_api_token` — sentinel value `"SENTINEL-API-TOKEN-XYZ"` does not appear in `format!("{:?}", config.chirpstack)`; `"***REDACTED***"` does (positive assertion: `assert!(formatted.contains(REDACTED_PLACEHOLDER))`).
  - `test_opcua_config_debug_redacts_password` — symmetric.

### AC#4: Whole-binary tracing capture confirms no secret string appears in logs at any level
- Extend the existing `secrets_not_logged_from_config_startup` / `secrets_not_logged_from_appconfig_from_path` tests in `src/opc_ua.rs` with a third sibling: `secrets_not_logged_when_full_config_debug_formatted`.
  - Build a full `AppConfig` with sentinel `api_token` and `user_password`.
  - Capture under `#[traced_test]` (`tracing-test 0.2` captures at `TRACE` by default — confirmed by the existing `secrets_not_logged_*` tests at `src/opc_ua.rs:2152` which rely on the same default).
  - Emit `tracing::trace!(?config, "force-debug-format the whole config")` with the loaded `AppConfig` (this is exactly the kind of careless log a future contributor might add — the test makes it safe).
  - **Three assertions, all required**:
    1. `assert!(!logs_contain(SENTINEL_TOKEN), "api_token leaked")` — primary leak check.
    2. `assert!(!logs_contain(SENTINEL_PASSWORD), "user_password leaked")` — primary leak check.
    3. `assert!(logs_contain(REDACTED_PLACEHOLDER), "Debug redaction did not fire — test trivially passing")` — **positive assertion that the redacted Debug impl actually produced its placeholder**. This catches the failure mode where AC#3's Debug impl is broken in a subtle way (e.g. printing `Some("***REDACTED***")` for an `Option<>` field, or using a different literal) and a passing test gives false confidence.
- **Verification:** New test passes; both sentinels absent AND `***REDACTED***` present in the captured output at `trace` level.

### AC#5: tonic gRPC metadata leak audit (NFR7 — third-party logging vector)
The `Debug` redaction in AC#3 only protects `ChirpstackPollerConfig`. The `api_token` is **also** copied into `AuthInterceptor.api_token` (`src/chirpstack.rs:111-146`) and inserted as `Bearer {token}` into the gRPC `authorization` metadata header on every outbound call. If `tonic` 0.14.5 (or one of its tracing/middleware layers) emits request headers at trace level, the bearer token leaks **outside** the AppConfig `Debug` path that AC#3/AC#4 cover.

This story does not silence tonic. It **documents the residual risk** and adds an audit step:

- **Audit (manual, recorded in Dev Notes):**
  - Inspect tonic 0.14.5's `transport` and `interceptor` modules on docs.rs/tonic-0.14.5 (or via the local source path resolved with `cargo metadata --format-version 1 | jq -r '.packages[] | select(.name == "tonic") | .manifest_path'`) and confirm whether tonic emits `tracing` events that include request metadata. As of when this story was written, tonic 0.14.5 does not log headers by default — document any deviation found.
  - Check opcgw's own code for tracing/middleware wiring that would log gRPC headers: `grep -rnE 'TraceLayer|trace_layer|tower_http' src/ Cargo.toml`. Currently there is no `tower-http` dependency and no `TraceLayer` wiring (the chirpstack layer uses `Channel::from_shared(...).connect()` and a hand-written `AuthInterceptor`); confirm this hasn't changed.
- **Mitigation if tonic leaks at trace:** add `EnvFilter` rule in `src/main.rs` tracing setup to clamp `tonic` and `tonic::transport` targets to `info` level so trace-level header dumps are filtered out before reaching any appender. Document the directive in `docs/security.md` (AC#7 section "What the gateway will / won't redact").
- **Document the residual risk in `docs/security.md`:** an explicit paragraph under "Anti-patterns" stating "if you wire `tower-http::trace::TraceLayer` or any tonic interceptor that logs request metadata, you re-open the bearer-token leak vector. Don't."
- **Open a tracking GitHub issue** "Story 7-1 follow-up: tonic / tower-http metadata redaction strategy" so a future story can build a `tower::Layer` that strips the `authorization` header before logging — out of scope here, but worth a tracker entry so it doesn't get lost.
- **Verification:**
  - Dev Notes contains the audit findings (one paragraph per item above) — what was found, what was applied, what was deferred.
  - `grep -rn 'TraceLayer\|trace_layer' src/` returns nothing (or only test code).
  - If the `EnvFilter` mitigation was needed, the new directive is visible in `src/main.rs` and a test confirms the level filter applies.

### AC#6: Operator-facing Docker / `.env` recipe (FR42 ergonomics)
- `docker-compose.yml` gets an `environment:` block that lists the canonical env vars and uses Compose-style variable substitution (Compose reads `.env` and substitutes the host-side value into the container's environment):
  ```yaml
  environment:
    - OPCGW_CHIRPSTACK__API_TOKEN=${OPCGW_CHIRPSTACK__API_TOKEN}
    - OPCGW_OPCUA__USER_PASSWORD=${OPCGW_OPCUA__USER_PASSWORD}
  ```
  (`tenant_id` is supplied via the TOML file directly per AC#1 since it isn't classified as a secret. Operators who want to override it for environment-specific deployments still can via `OPCGW_CHIRPSTACK__TENANT_ID`; document this in `docs/security.md` but keep the docker-compose example minimal.)
- A new committed `.env.example` documents each variable with a one-line description **and a placeholder value** matching the `REPLACE_ME_WITH_...` convention. The committed `.env.example` ships placeholders only — operators are expected to `cp .env.example .env` and edit the copy.
- **`.gitignore` already excludes `.env`, `.env.local`, and `config/config.local.toml`** in the existing "# Config & Secrets" block. **No change required** — verify only.
- **Verification:**
  - `docker compose config` parses successfully when `.env` is present (Compose validates that referenced vars are defined; placeholder values are accepted at parse time — startup-time rejection happens inside the gateway via AC#2).
  - Manual smoke step in Dev Notes:
    1. `cp .env.example .env` (don't edit) → `docker compose up` → container exits with the placeholder error from AC#2.
    2. Edit `.env` with real values → `docker compose up` → container starts, `info!("Gateway started successfully", ...)` is logged.

### AC#7: Documentation
- New file `docs/security.md` covers:
  1. **The env-var convention** — table of every secret-bearing config field with its canonical env var name (`OPCGW_<SECTION>__<FIELD_UPPERCASE>`).
  2. **Precedence rules** — env > TOML; placeholder values are rejected by `validate()` *after* env merge so a real env-var override of a placeholder TOML field passes; missing env var with placeholder TOML → startup error per AC#2.
  3. **Docker / Compose recipe** — point to `.env.example`, show the `environment:` block from AC#6, give the `cp .env.example .env` workflow.
  4. **Kubernetes recipe (one paragraph)** — secret mounted as env var via `valueFrom.secretKeyRef`. Same env names work.
  5. **Anti-patterns** — do not bake secrets into Docker images, do not commit `.env`, do not paste tokens into bug reports / Slack, do not wire `tower-http::trace::TraceLayer` or any tonic interceptor that logs request metadata (re-opens the bearer-token leak vector covered by AC#5).
  6. **What the gateway will / won't redact** — explicit list of which fields the `Debug` impl redacts (per AC#3 matrix). Call out that `tenant_id` is **substituted with a placeholder UUID in the shipped template** but **not redacted in `Debug`** — AC#3 matrix and Dev Notes "Why tenant_id is replaced but not redacted" explain the rationale. Anything not in the list is NOT secret-protected; if you're adding a new sensitive field, extend the matrix and the `Debug` impl together.
- `README.md` "Configuration" / "Planning" section gets a one-line link to `docs/security.md`. Update the **Planning** table row for Epic 7 per the documentation-sync rule in `CLAUDE.md`, **using the status that matches the commit being made**:
  - At the implementation commit (story status `ready-for-dev → review`): row reads `🔄 in-progress (7-1 review)`.
  - At the code-review-complete commit (status `review → done`): row reads `🔄 in-progress (7-1 done)`.
  - Do **not** flip the row to `done` ahead of code review — the row reflects current truth, not aspirational status.
- `docs/configuration.md` gets a single new subsection "Secrets" that points at `docs/security.md` rather than duplicating content.
- **Verification:** `docs/security.md` exists, links resolve, `grep -n 'security.md' README.md docs/configuration.md` returns at least one hit each.

### AC#8: Tests pass and clippy is clean
- All existing tests pass: `cargo test` exits 0 with no regressions in the lib / bin / integration suites that were green at `HEAD = ae254e9`.
- `cargo clippy --all-targets -- -D warnings` exits 0 (Story 6-3 left the workspace clippy-clean — preserve that state; if a new warning appears, fix it, do not `#[allow]` it).
- New tests added by AC#2, AC#3, AC#4 are present and pass.
- **Verification:** Run `cargo test 2>&1 | tail -20` and `cargo clippy --all-targets -- -D warnings 2>&1 | tail -5`. Paste counts into Dev Notes.

### AC#9: Security re-check (carry forward from Story 6-1 AC#8 / Story 6-3 AC#10)
- After implementation, run:
  ```bash
  grep -rE 'api_token|user_password' src/ \
    | grep -E 'info!|debug!|warn!|error!|trace!' \
    | grep -v -- '_test.rs:' \
    | grep -v 'tests/' \
    | grep -v '#\[cfg(test)\]'
  ```
  Expected: empty (the redaction sites themselves don't log — they only impl `Debug`).
- Confirm the placeholder-detection error message uses the literal prefix `"REPLACE_ME_WITH_"` and does **not** echo the operator's full input value back (avoid log-injection-style risk per AC#2).
- Confirm the AC#5 audit was performed and Dev Notes capture the findings.

---

## Tasks / Subtasks

### Task 0: Open tracking GitHub issues (CLAUDE.md compliance)
- [x] Open GitHub issue **"Story 7-1: Credential Management via Environment Variables"** with a one-paragraph summary linking to this story file. Reference it in every commit message for this story (`Refs #N` or `Closes #N` on the final implementation commit). → opened as **#81**.
- [x] Open follow-up issue **"Story 7-1 follow-up: tonic / tower-http metadata redaction strategy"** for the deferred work identified in AC#5. Link from this story's Dev Notes. → opened as **#82**.
- [x] Open follow-up issue **"Consider tenant_id redaction in Debug impl"** for the deferred enhancement (AC#3 matrix). Link from `deferred-work.md` (Task 7 below). → opened as **#83**.

### Task 1: Sanitize the shipped `config/config.toml` (AC#1)
- [x] Copy the current `config/config.toml` to a new `config/config.example.toml` (committed). Add a header comment: "Reference example with multiple applications (Arrosage, Bâtiments, Cultures, Meteo). Do NOT use as-is in production — copy to `config/config.toml`, replace credentials with env-var placeholders, tailor application list to your deployment. See docs/security.md."
- [x] Rewrite `config/config.toml` to a minimal template:
  - One illustrative `[[application]]` with one `[[application.device]]` and one `[[application.device.read_metric]]`.
  - Placeholder values for `api_token` and `user_password` per AC#1; placeholder UUID `"00000000-0000-0000-0000-000000000000"` for `tenant_id`.
  - Keep `server_address`, polling intervals, and OPC UA non-secret fields with sensible defaults.
- [x] Add a top-of-file comment block pointing at `docs/security.md` and listing the two required env vars (`OPCGW_CHIRPSTACK__API_TOKEN`, `OPCGW_OPCUA__USER_PASSWORD`).
- [x] Confirm `grep -E 'eyJ|user1|52f14cd4-c6f1-4fbd' config/config.toml` returns nothing. (Also changed the historical `user_name = "user1"` to `user_name = "opcua-user"` to keep this grep clean — the username is not classified as a secret but the literal `"user1"` would otherwise match.)

### Task 2: Add placeholder-detection validation (AC#2)
- [x] In `src/config.rs::AppConfig::validate`, **extend** the existing checks at lines 693-695 (`api_token`) and 731-737 (`user_password`). Add an `if value.starts_with(crate::utils::PLACEHOLDER_PREFIX)` branch with the exact error message from AC#2 (use the literal prefix `"REPLACE_ME_WITH_"`, do not echo back the operator's full input).
- [x] Centralise the prefix as `pub const PLACEHOLDER_PREFIX: &str = "REPLACE_ME_WITH_";` in `src/utils.rs`.
- [x] Do **not** add a placeholder check for `tenant_id` — the placeholder UUID `"00000000-..."` is a valid UUID format and the empty-string check at line 697-699 already catches the literal-empty case. The AC#1 substitution is sufficient.
- [x] Write the three tests from AC#2 verification (`test_validation_rejects_placeholder_api_token`, `test_validation_rejects_placeholder_user_password`, end-to-end `from_path` test using `temp_env::with_vars`). All three pass.

### Task 3: Manual `Debug` impl with redaction (AC#3)
- [x] Add `pub const REDACTED_PLACEHOLDER: &str = "***REDACTED***";` to `src/utils.rs`.
- [x] On `ChirpstackPollerConfig` (`src/config.rs:92-140`): change `#[derive(Debug, Deserialize, Clone)]` → `#[derive(Deserialize, Clone)]`. Hand-write `impl std::fmt::Debug` that uses `f.debug_struct("ChirpstackPollerConfig").field("api_token", &REDACTED_PLACEHOLDER).field("server_address", &self.server_address)…` for all fields. **Do not** redact `tenant_id` per the AC#3 matrix.
- [x] On `OpcUaConfig` (`src/config.rs:147-249`): same treatment. Redact `user_password` only.
- [x] Note the field-type subtlety: `debug_struct().field(name, value)` calls `Debug` on `value`, which produces `Some(x)` / `None` for `Option<>` and the appropriate format for `bool`, `u32`, etc. Output shape for non-redacted fields will match what `derive(Debug)` produced before — so no test that exact-matches old `Debug` output should break. If one does, the fix is to update the test expectation, not to wrap the value differently.
- [x] Confirm `cargo build` still succeeds (the existing `#[allow(unused)]` and `#[allow(dead_code)]` annotations should not interact with the `Debug` change, but verify). → `cargo build` clean, no warnings, no exact-match Debug-output tests broke.
- [x] Write the two tests from AC#3 verification. Both `test_chirpstack_poller_config_debug_redacts_api_token` and `test_opcua_config_debug_redacts_password` pass.

### Task 4: Extend the no-leak regression suite (AC#4)
- [x] In `src/opc_ua.rs` near the existing `secrets_not_logged_from_*` tests (around line 2152), add `secrets_not_logged_when_full_config_debug_formatted` per AC#4. **Three assertions required**: two negative (sentinels absent) and one positive (`***REDACTED***` present). All three present and asserting against `crate::utils::REDACTED_PLACEHOLDER`.
- [x] Confirm test passes locally. → all four `secrets_not_logged_*` tests pass.

### Task 5: tonic gRPC metadata leak audit (AC#5)
- [x] Audit tonic 0.14.5 for header-logging behaviour. Document findings in Dev Notes ("Audit findings: tonic header logging"). → 8 `tracing::*!` sites in tonic 0.14.5; all error conditions (connection/accept/TLS errors, `grpc-timeout` parse errors, reconnect errors). None log request headers/metadata. No `#[instrument]` capturing request fields.
- [x] Verify opcgw doesn't wire `tower-http::trace::TraceLayer` or any tonic interceptor that logs metadata: `grep -rn 'TraceLayer\|trace_layer' src/`. → empty; no `tower-http` dependency in `Cargo.toml`.
- [x] If tonic emits headers at trace level, add an `EnvFilter` directive in `src/main.rs` to clamp `tonic` and `tonic::transport` to `info`. Add a one-test-line confirming the directive applies. → not needed today (audit clean); no change to `src/main.rs`.
- [x] If no mitigation needed today (current state), document that fact + the "do not wire TraceLayer" rule in `docs/security.md` Anti-patterns section (covered by **Task 7**). → documented in `docs/security.md` "Anti-patterns" + dedicated "Audit findings: tonic 0.14.5 metadata logging" section.
- [x] Confirm the follow-up GitHub issue from Task 0 is open and linked from Dev Notes. → GitHub issue **#82** open and referenced.

### Task 6: Docker / `.env` operator recipe (AC#6)
- [x] Update `docker-compose.yml` per AC#6 (add `environment:` block with the two secret env vars sourced from `.env`).
- [x] Create `.env.example` with placeholder values and one-line descriptions for `OPCGW_CHIRPSTACK__API_TOKEN` and `OPCGW_OPCUA__USER_PASSWORD`.
- [x] **No `.gitignore` change needed** — `.env` is already excluded (verified in Existing Infrastructure table). Just confirm with `grep -n '^\.env' .gitignore`. → confirmed: `.env` and `.env.local` excluded at lines 27-28.
- [x] Smoke-test manually using the `cp .env.example .env` workflow:
  - With unedited `.env` (placeholders) → container exits with placeholder error from AC#2.
  - With real values in `.env` → container starts.
  - Document the exact commands in Dev Notes. → smoke-tested via `cargo run` (no Docker daemon required for proof of contract): without env vars the gateway exits with the AC#2 placeholder error (verbatim wording); with `OPCGW_CHIRPSTACK__API_TOKEN=test OPCGW_OPCUA__USER_PASSWORD=test` it logs `Gateway started successfully` past validation. `docker compose --env-file .env config` parses the substitution cleanly.

### Task 7: Documentation (AC#7) + record deferred work
- [x] Create `docs/security.md` with the six sections from AC#7. → all six sections present (env-var convention, precedence rules, Quick start incl. Docker + Kubernetes, migration path, redaction matrix, anti-patterns, audit findings, references).
- [x] Add the env-var → field mapping table. → in "The env-var convention" section.
- [x] Add a one-line "Secrets" subsection to `docs/configuration.md` pointing at `docs/security.md`. → added at the top of "Configuration File Format".
- [x] Update `README.md` Planning table row for Epic 7 to `🔄 in-progress (7-1 done)`. → set to `🔄 in-progress (7-1 review)` at the implementation commit per the AC#7 timing rule (status will flip to `7-1 done` at the code-review-complete commit).
- [x] Add a one-line link to `docs/security.md` from the README "Configuration" section. → added a callout block immediately after the Configuration code example.
- [x] Append entries to `_bmad-output/implementation-artifacts/deferred-work.md` for:
  - **`tenant_id` redaction in `Debug` impl** — out of scope per epic spec; tracked at GitHub issue **#83**.
  - **tonic / tower-http metadata redaction strategy** — tracked at GitHub issue **#82**.
  - **Operator migration shim** — if `git pull` triggers conflicts on operators' local `config/config.toml`, consider a one-time `scripts/migrate-config-7-1.sh` helper. Defer; documented in `docs/security.md` "Migration path" paragraph instead.
  - (Bonus) **`secrecy::SecretString` newtype** for `api_token` / `user_password` — Story 7-1 Out of Scope; recorded for completeness.

### Task 8: Final verification (AC#8, AC#9)
- [x] Run `cargo test 2>&1 | tail -20` — paste pass/fail counts into Dev Notes. → see "Test results" in Completion Notes below. **Lib + bin + integration: 488 pass, 0 fail.** Doctest: 56 compile-failures pre-existing on baseline `ae254e9` (unchanged — not introduced by this story).
- [x] Run `cargo clippy --all-targets -- -D warnings` — confirm exit 0. → exit 0, no warnings.
- [x] Run the AC#9 grep — confirm empty (or only redaction-site code, no logging). → empty.
- [x] Manually verify `cargo run` with placeholder TOML and no env vars → exits with the placeholder error from AC#2 (not a panic). → confirmed; clean `Err` propagation, exit code 0 from process (gateway logs ERROR + Rust `Error:` line, no panic).
- [x] Manually verify `cargo run` with placeholder TOML and `OPCGW_CHIRPSTACK__API_TOKEN=test OPCGW_OPCUA__USER_PASSWORD=test` → starts past validation (subsequent failures unrelated to credentials are fine). → confirmed; `Gateway started successfully` logged with `application_count=1, device_count=1, opc_ua_endpoint=0.0.0.0:4840`. Subsequent SQLite open failure (missing `data/` directory) is unrelated to credentials, as expected.

### Review Findings (2026-04-28)

Adversarial code review run with three layers (Blind Hunter, Edge Case Hunter, Acceptance Auditor). All ACs verified satisfied; no HIGH or MEDIUM findings. Triage produced 1 decision-needed, 4 patches, 3 deferrals, 11 dismissed.

- [x] [Review][Decision→Patch] `config/config.example.toml` ships operator's real-looking deployment IDs — committed file contained 4 application UUIDs (`ae2012c2-…`, `194f12ab-…`, `fca74250-…`, `81d88d98-…`), 9 real DevEUIs (`a840418371886840`, `524d1e0a02243201`, `2cf7f1c06130048a`, etc.), and `user_name = "user1"`. **Resolved (option 1 — scrub):** application UUIDs replaced with `00000000-0000-0000-0000-00000000000{1..4}`, DevEUIs replaced with `0000000000000001`–`0000000000000009`, `user_name` replaced with `opcua-user` to match the sanitized `config/config.toml`. Multi-app shape (Arrosage / Bâtiments / Cultures / Meteo) preserved per spec. [blind+edge]
- [x] [Review][Patch] `docker-compose.yml` use `${VAR:?msg}` to fail-fast on unset host env [docker-compose.yml:17-18] — **fixed:** both env vars now use `${VAR:?missing — copy .env.example to .env and set this variable, see docs/security.md}`. `docker compose --env-file /dev/null config` exits with the pinpoint message before launching the container; `docker compose --env-file .env.example config` parses cleanly. [blind+edge]
- [x] [Review][Patch] Migration recipe is destructive without explicit safety net [docs/security.md "Migration path"] — **fixed:** added a leading `⚠️` warning callout, made step 3's irreversibility explicit ("This discards your local `config/config.toml` — your backup from step 1 is the only copy"), strengthened step 1 with a `ls -l` verification, and added a "Reversible alternative — `git stash` workflow" subsection with the safer command sequence. [blind]
- [x] [Review][Patch] Verify GitHub issues #81/#82/#83 actually exist on GitHub — **verified:** all three issues exist and are `OPEN`. `#81 = "Story 7-1: Credential Management via Environment Variables"`, `#82 = "Story 7-1 follow-up: tonic / tower-http metadata redaction strategy"`, `#83 = "Consider tenant_id redaction in Debug impl"`. Titles match the spec exactly. [auditor]
- [x] [Review][Patch] Re-run fresh `cargo test` + `cargo clippy --all-targets -- -D warnings` post-review — **clean.** `cargo test --lib --bins --tests` → 488 pass, 0 fail, 7 ignored (counts identical to implementation-time run; review patches were docs/config-only and did not affect Rust source). `cargo clippy --all-targets -- -D warnings` → exit 0, zero warnings. [auditor]
- [x] [Review][Defer] Manual `Debug` impls have no compile-time pin against future field-add omissions [src/config.rs:259-308] — deferred. AC#3 matrix is the explicit contract; future struct additions must update the matrix. Procedural-macro / serde-roundtrip pin would be a larger refactor. [blind+edge]
- [x] [Review][Defer] `tenant_id` all-zeros UUID bypasses validation; gateway boots, fails later at first gRPC call [src/config.rs::AppConfig::validate, config/config.toml:55] — deferred, already tracked at GitHub issue #83 per spec AC#3 matrix and Dev Notes "Why tenant_id is replaced but not redacted". [blind+edge]
- [x] [Review][Defer] Validation-error prefix string duplicated in code (`utils.rs::PLACEHOLDER_PREFIX`) and docs (`docs/security.md`) — deferred, cosmetic doc-rot risk only; constant lives in code, docs are reference material. [blind]

---

## Dev Notes

### Anti-patterns to avoid (per CLAUDE.md scope-discipline rule)
- **Do not** add the `secrecy` crate, do not introduce `SecretString`. The redacted-`Debug` approach achieves NFR7 with a smaller blast radius.
- **Do not** introduce a new `OPCGW_API_TOKEN` short-form env var. The figment nested form (`OPCGW_CHIRPSTACK__API_TOKEN`) is the canonical name and is already pinned by tests.
- **Do not** redact `server_address`, `application_name`, or `application_uri`. They are not secrets and they appear in the existing startup `info!` line at `src/main.rs:411-418`.
- **Do not** rewrite the figment loader. The existing two-phase bootstrap in `src/main.rs:73-376` is correct and already tested.
- **Once you have created `config/config.example.toml` (Task 1), do not strip its content.** The multi-application real-world shape (Arrosage / Bâtiments / Cultures / Meteo) is the point — it shows operators what a production-shaped configuration looks like.
- **Do not** wire `tower-http::trace::TraceLayer` or any tonic interceptor that logs request metadata. Doing so re-opens the bearer-token leak vector that AC#5 audits and AC#7 documents.
- **Do not** bundle other Epic 7 work (security endpoints, connection limiting) into this story — per CLAUDE.md "each commit covers exactly one story".

### Operator migration path (call this out in `docs/security.md`)
The current committed `config/config.toml` contains the user's real ChirpStack JWT, real tenant UUID, real device EUIs, and a literal `user_password = "user1"`. After this story lands, operators who `git pull` will get a conflict on `config/config.toml` if they have local edits. The migration recipe to put in `docs/security.md`:

1. **Before pulling:** `cp config/config.toml ~/opcgw-config-backup.toml`.
2. **Pull the change:** `git pull`. Conflict on `config/config.toml` is expected.
3. **Resolve:** keep the new template (`git checkout --theirs config/config.toml`).
4. **Restore your application list:** copy your `[[application]]` blocks from the backup into the new `config/config.toml`.
5. **Move secrets to env vars:** create `.env` from `.env.example`, fill in the real `OPCGW_CHIRPSTACK__API_TOKEN` and `OPCGW_OPCUA__USER_PASSWORD` from the backup file, then `chmod 600 .env`.
6. **Verify:** `cargo run` (or `docker compose up`) — should start cleanly. If it exits with the placeholder error from AC#2, you missed step 5.

### Why placeholder detection is in `validate()` and not in figment-deserialize
- `validate()` runs after env merge (`src/config.rs:651-652`), so a placeholder in TOML overridden by a real env var will pass. That's the intended behaviour: the placeholder is a **red flag for "operator forgot to set the env var"**, not a hard ban on the literal string ever appearing.
- Putting the check in serde / figment would fire even when the operator has correctly set the env var, defeating the override path.

### Why `tenant_id` is replaced but not redacted
- The epic spec (`epics.md` lines 624-634) names only `api_token` and `password` as the credentials to protect. `tenant_id` is a UUID identifying a customer's ChirpStack deployment — moderate-disclosure if leaked, but not classified as a secret in the spec.
- This story therefore takes a middle position: **substitute** the real tenant UUID in the shipped `config/config.toml` template (so the user's specific deployment isn't published) but **do not** add it to the `Debug` redaction matrix or placeholder-detection rule. Operators who don't override it via env var will see `tenant_id = "00000000-0000-0000-0000-000000000000"` in their config and the gateway will start (and fail later when ChirpStack rejects calls with that UUID, with a clear gRPC error — not a config-validation error).
- Adding `tenant_id` to AC#3's redaction matrix would be a defensible scope expansion. **Tracked as a follow-up GitHub issue and recorded in `deferred-work.md`** (Task 0 + Task 7) so it doesn't get lost.

### Hand-written `Debug` impl pattern
Reference shape (don't copy-paste blindly — adapt to actual field list at `src/config.rs:93-140`):

```rust
impl std::fmt::Debug for ChirpstackPollerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChirpstackPollerConfig")
            .field("server_address", &self.server_address)
            .field("api_token", &crate::utils::REDACTED_PLACEHOLDER)
            .field("tenant_id", &self.tenant_id)  // not redacted — see AC#3 matrix
            .field("polling_frequency", &self.polling_frequency)
            .field("retry", &self.retry)
            .field("delay", &self.delay)
            .field("list_page_size", &self.list_page_size)
            .finish()
    }
}
```

**Field-type subtlety to watch for:** `debug_struct().field(name, value)` calls `Debug` on `value`. For owned types (`String`, `u32`, `bool`) `&self.x` is correct. For `Option<>` fields (e.g. `OpcUaConfig::host_port: Option<u16>`, `host_ip_address: Option<String>`, `hello_timeout: Option<u32>`, `stale_threshold_seconds: Option<u64>`) you also pass `&self.x` — `Debug` on `&Option<T>` produces `Some(x)` / `None`, which matches what `derive(Debug)` produced. **No need to wrap the redacted placeholder in `Some(...)`** — for redacted fields, just pass `&crate::utils::REDACTED_PLACEHOLDER` (a `&&'static str`) and the output is `"***REDACTED***"` regardless of whether the underlying field is `String` or `Option<String>`.

Same shape for `OpcUaConfig`. The `OpcUaConfig` field count (~15 fields) makes the impl ~25 lines — acceptable.

**Verification that nesting works through `AppConfig::Debug`:** the parent `AppConfig` struct (`src/config.rs:543-569`) keeps its `#[derive(Debug, ...)]`. When `Debug` is derived on `AppConfig`, the derived impl calls `Debug` on each field — including `chirpstack: ChirpstackPollerConfig`, which will invoke the **custom** redacting `Debug`. So `format!("{:?}", app_config)` redacts correctly. AC#4's positive assertion confirms this end-to-end.

### Env-var quick reference (this is the contract)
| Field | Env var | Required for new deployments? | Source |
|-------|---------|-------------------------------|--------|
| `chirpstack.api_token` | `OPCGW_CHIRPSTACK__API_TOKEN` | **yes** (placeholder rejected by AC#2) | figment, `Env::prefixed("OPCGW_").split("__").global()` |
| `opcua.user_password` | `OPCGW_OPCUA__USER_PASSWORD` | **yes** (placeholder rejected by AC#2) | same |
| `chirpstack.tenant_id` | `OPCGW_CHIRPSTACK__TENANT_ID` | optional — placeholder UUID is valid format; ChirpStack will reject calls until set | same |
| `chirpstack.server_address` | `OPCGW_CHIRPSTACK__SERVER_ADDRESS` | optional | same |
| `opcua.host_port` | `OPCGW_OPCUA__HOST_PORT` | optional | same |
| `[logging].dir` | `OPCGW_LOGGING__DIR` (figment) **or** `OPCGW_LOG_DIR` (bootstrap short form) | optional | both — short form wins |
| `[logging].level` | `OPCGW_LOGGING__LEVEL` (figment) **or** `OPCGW_LOG_LEVEL` (bootstrap short form) | optional | both — short form wins |

The bootstrap short forms exist only because they're consumed before figment runs (Story 6-1/6-2). **Do not** add a third short form for any new field unless it has the same bootstrap-phase requirement.

### Project Structure Notes
- `src/config.rs` — single-file, ~1100 lines. The placeholder validation extends existing `validate()` blocks (lines 693-695, 731-737).
- `src/opc_ua.rs` — `secrets_not_logged_*` tests live around line 2150 in the `tests` module. Add the new AC#4 test there for proximity.
- `src/utils.rs` — small constants module; the two new `pub const` declarations (`PLACEHOLDER_PREFIX`, `REDACTED_PLACEHOLDER`) belong with the existing `OPCGW_*` constants.
- `src/main.rs` — only touched if the AC#5 audit finds tonic emits headers at trace level. Otherwise unchanged.
- `src/chirpstack.rs` — **not** modified for code changes (the `AuthInterceptor` field stays as `String`; `secrecy` is out of scope). Only audited per AC#5.
- New files: `docs/security.md`, `.env.example`, `config/config.example.toml`. No new modules in `src/`.
- Modified files (expected File List, ~9-10 files):
  - `src/config.rs`, `src/utils.rs`, `src/opc_ua.rs`
  - `src/main.rs` (only if AC#5 mitigation needed)
  - `config/config.toml` (sanitized), `config/config.example.toml` (new — preserves current real-world shape)
  - `docker-compose.yml`, `.env.example`
  - `docs/security.md`, `docs/configuration.md`, `README.md`
  - `_bmad-output/implementation-artifacts/deferred-work.md` (append per Task 7)
- **Not modified:** `.gitignore` (`.env` already excluded — verify only).

### Testing Standards
- New unit tests live next to the code they cover (`src/config.rs::tests` and `src/opc_ua.rs::tests`).
- Use `temp_env::with_vars` for env-var manipulation — it's already in `[dev-dependencies]` (used by `test_chirpstack_nested_env_override` and friends). Don't pull in another env-test crate.
- Use `tracing_test::traced_test` for log-capture tests (already used by the two existing `secrets_not_logged_*` tests).
- The end-to-end `from_path` test should write a temp TOML via `std::env::temp_dir()` + `uuid::Uuid::new_v4()` (pattern already established at `src/opc_ua.rs:2273-2277`).

### References
- [Source: `_bmad-output/planning-artifacts/epics.md#Story 6.1: Credential Management via Environment Variables`] (lines 618-634; numbered as 7.1 in sprint-status)
- [Source: `_bmad-output/planning-artifacts/prd.md#FR42`] (line 411) — load secrets from env vars
- [Source: `_bmad-output/planning-artifacts/prd.md#NFR7`] (line 437) — secrets never in logs
- [Source: `_bmad-output/planning-artifacts/prd.md#NFR8`] (line 438) — default config has no real credentials
- [Source: `_bmad-output/planning-artifacts/prd.md#NFR24`] (line 463) — env override for all secrets
- [Source: `_bmad-output/planning-artifacts/architecture.md#Error Handling`] (lines 188-198) — `OpcGwError::Configuration(String)` is the right variant; the existing validation path is already wired
- [Source: `src/config.rs:641-649`] — figment merge order (env after TOML)
- [Source: `src/config.rs:693-737`] — empty-string validation precedent to extend
- [Source: `src/config.rs:1561-1662`] — env-override regression tests already in place
- [Source: `src/opc_ua.rs:2147-2301`] — `secrets_not_logged_*` regression tests already in place
- [Source: `src/main.rs:370-376`] — startup error path: load failure → `error!` → `Err(...)` exit, **no panic**
- [Source: `_bmad-output/implementation-artifacts/6-3-remote-diagnostics-for-known-failures.md`] — previous-story patterns: code-review loop discipline, helper-extraction-for-testability, scope-discipline rule
- [Source: `CLAUDE.md`] — per-story commit rule, code-review loop rule, documentation-sync rule, security-check requirement at epic-close

---

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) via Claude Code CLI (`/bmad-dev-story`).

### Debug Log References

**Test results (lib + bin + integration; doctests excluded — see AC#8 carryover):**

```
$ cargo test --lib --bins --tests 2>&1 | grep '^test result:'
test result: ok. 194 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 6.43s   # lib
test result: ok. 215 passed; 0 failed; 3 ignored; 0 measured; 0 filtered out; finished in 6.66s   # bin
test result: ok.  11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s   # integration
test result: ok.  23 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   # integration
test result: ok.  12 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   # integration
test result: ok.   5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   # integration
test result: ok.  10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   # integration
test result: ok.   7 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 0.94s   # integration
test result: ok.  11 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s   # integration
```

**Total: 488 pass, 0 fail, 7 ignored.** New tests added by this story (5 in total) all pass:

- `config::tests::test_validation_rejects_placeholder_api_token`
- `config::tests::test_validation_rejects_placeholder_user_password`
- `config::tests::test_from_path_with_and_without_placeholder_env_vars`
- `config::tests::test_chirpstack_poller_config_debug_redacts_api_token`
- `config::tests::test_opcua_config_debug_redacts_password`
- `opc_ua::tests::secrets_not_logged_when_full_config_debug_formatted`

(That's six new tests — three for AC#2, two for AC#3, one for AC#4 — total six new + four existing `secrets_not_logged_*` tests still passing.)

**Clippy:**

```
$ cargo clippy --all-targets -- -D warnings 2>&1 | tail -3
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.64s
```

Exit 0, no warnings. Story 6-3's clippy-clean state preserved.

**AC#9 grep (no `api_token` / `user_password` reaching a tracing macro):**

```
$ grep -rE 'api_token|user_password' src/ \
    | grep -E 'info!|debug!|warn!|error!|trace!' \
    | grep -v -- '_test.rs:' | grep -v 'tests/' | grep -v '#\[cfg(test)\]'
$ echo "exit=$?"
exit=1     # empty match; no production code logs the secret-bearing fields
```

**AC#1 grep (no real credentials in shipped TOML):**

```
$ grep -E 'eyJ|user1|52f14cd4-c6f1-4fbd' config/config.toml
$ echo "first-exit=$?"; grep -Eo '[0-9a-f]{16}' config/config.toml | grep -vE '^0+1?$'; echo "second-exit=$?"
first-exit=1
second-exit=1
```

Both greps clean. Note: changed historical `user_name = "user1"` to
`user_name = "opcua-user"` in the shipped template so the AC#1 grep stays
clean — the username isn't classified as a secret but the literal `"user1"`
would otherwise match the password-detection grep.

**Smoke test of placeholder validation (`cargo run`):**

Without env vars (placeholders intact):
```
ERROR opcgw: Failed to load configuration error=Configuration error: Configuration validation failed:
  - chirpstack.api_token: placeholder value detected (starts with "REPLACE_ME_WITH_"). Set OPCGW_CHIRPSTACK__API_TOKEN to inject the real secret. See docs/security.md.
  - opcua.user_password: placeholder value detected (starts with "REPLACE_ME_WITH_"). Set OPCGW_OPCUA__USER_PASSWORD to inject the real secret. See docs/security.md.
Error: Configuration("Configuration validation failed: …")
```

Wording matches AC#2 verbatim. Clean `Err` propagation — no panic.

With env vars set (`OPCGW_CHIRPSTACK__API_TOKEN=test OPCGW_OPCUA__USER_PASSWORD=test`):
```
INFO opcgw: Gateway started successfully poll_interval_seconds=10 application_count=1 device_count=1 opc_ua_endpoint=0.0.0.0:4840 chirpstack_server=http://127.0.0.1:8080
```

Validation passed; subsequent SQLite open failure is unrelated to credentials, as expected.

**Compose verification:**

`docker compose --env-file .env config` parses the env-var substitution
cleanly when `.env` exists (no Docker daemon required to validate the
contract).

### Completion Notes List

- ✅ **AC#1 (NFR8):** `config/config.toml` rewritten as a minimal one-application template with `REPLACE_ME_WITH_*` placeholders + all-zeros tenant UUID + synthesized illustrative DevEUI `0000000000000001`. Original multi-application real-world shape preserved in new `config/config.example.toml` with a header comment pointing operators at the migration recipe in `docs/security.md`.
- ✅ **AC#2:** Placeholder-detection extension wired into `AppConfig::validate` for both `api_token` and `user_password`. Error wording matches the spec verbatim — names the field, names the env var, points at `docs/security.md`, and references the `REPLACE_ME_WITH_` literal **without** echoing the operator's full input. `PLACEHOLDER_PREFIX` constant centralised in `src/utils.rs` so the prefix is checked verbatim and future placeholders following the same convention generalise.
- ✅ **AC#3 (NFR7):** Hand-written `Debug` impls on `ChirpstackPollerConfig` and `OpcUaConfig` redact `api_token` / `user_password` to `***REDACTED***` (centralised as `REDACTED_PLACEHOLDER` in `src/utils.rs`). `tenant_id` deliberately left non-redacted per the AC#3 matrix; `server_address`, `user_name`, paths, and all numeric/bool fields keep their default `Debug` formatting so existing log lines are unchanged. Field-shape subtlety from the spec confirmed in practice — no exact-match Debug-output test broke.
- ✅ **AC#4:** `secrets_not_logged_when_full_config_debug_formatted` added to `src/opc_ua.rs` with all three required assertions (two negative sentinel checks + one positive `REDACTED_PLACEHOLDER` presence check). Asserts against `crate::utils::REDACTED_PLACEHOLDER`, not a literal — keeps the test in lockstep with the constant.
- ✅ **AC#5:** tonic 0.14.5 audit performed and documented in `docs/security.md` ("Audit findings"). 8 tonic tracing sites total — all error conditions (connection/accept/TLS errors, `grpc-timeout` parse, reconnect). None log request headers or metadata. No `#[instrument]` capturing request fields. opcgw has no `tower-http` dep and no `TraceLayer` wiring. **No `EnvFilter` mitigation needed today.** Anti-pattern documented and follow-up issue **#82** opened.
- ✅ **AC#6:** `docker-compose.yml` updated with `environment:` block sourcing the two secret env vars from `.env`. New `.env.example` ships with `REPLACE_ME_WITH_*` placeholders only; `.gitignore` already excludes `.env` (verified, not modified).
- ✅ **AC#7:** New `docs/security.md` covers all six required sections (env-var convention, precedence rules, Docker recipe, Kubernetes recipe, anti-patterns, redaction matrix) plus a Migration-path section and the AC#5 audit findings. Cross-linked from `docs/configuration.md` (new "Secrets" subsection at top of file) and `README.md` (callout block after the Configuration code example + Planning row updated to `🔄 in-progress (7-1 review)` per the AC#7 timing rule). `deferred-work.md` appended with three Story-7-1 deferrals + one Out-of-Scope `secrecy::SecretString` entry, each linked to the corresponding GitHub issue (#82, #83) where applicable.
- ✅ **AC#8:** Lib + bin + integration tests **488 pass, 0 fail** (story added 6 new tests, all green). `cargo clippy --all-targets -- -D warnings` exit 0. The 56 doctest compile-failures observed in `cargo test` are pre-existing on baseline `HEAD = ae254e9` (verified via `git stash`); not introduced by this story and explicitly out of AC#8 scope ("lib / bin / integration suites that were green at HEAD = ae254e9").
- ✅ **AC#9:** Security re-check grep clean. Placeholder error message uses the literal `REPLACE_ME_WITH_` prefix and does not echo the operator's input. AC#5 audit recorded above.
- ✅ **CLAUDE.md compliance:** Three GitHub issues opened (#81 main, #82 tonic follow-up, #83 tenant_id follow-up). Commit message will reference `Closes #81` per the per-story commit rule.

### File List

**Modified (10 files):**

- `src/config.rs` — placeholder validation extension in `validate()`, `derive(Debug)` removed from `ChirpstackPollerConfig` + `OpcUaConfig`, hand-written redacting `Debug` impls added, three new tests for AC#2, two new tests for AC#3.
- `src/utils.rs` — added `PLACEHOLDER_PREFIX` and `REDACTED_PLACEHOLDER` constants in a new "Secret-Handling Constants (Story 7-1)" section.
- `src/opc_ua.rs` — added `secrets_not_logged_when_full_config_debug_formatted` test (AC#4) next to the existing `secrets_not_logged_*` tests.
- `config/config.toml` — sanitized to a minimal one-application template with placeholder credentials + placeholder tenant UUID + synthesized illustrative DevEUI; `user_name` changed from `user1` to `opcua-user`.
- `docker-compose.yml` — added `environment:` block sourcing `OPCGW_CHIRPSTACK__API_TOKEN` and `OPCGW_OPCUA__USER_PASSWORD` from `.env`.
- `docs/configuration.md` — new "Secrets" subsection at the top of "Configuration File Format" pointing at `docs/security.md`.
- `README.md` — Configuration section gets a 🔐 Secrets callout linking to `docs/security.md`; Planning table row for Epic 7 flipped to `🔄 in-progress (7-1 review)`; Planning "last updated" date bumped to 2026-04-28.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — `7-1-credential-management-via-environment-variables` flipped from `ready-for-dev` to `in-progress`, then to `review` at story completion. `last_updated` field updated.
- `_bmad-output/implementation-artifacts/deferred-work.md` — appended a "Deferred from: implementation of story-7-1" section with four entries (tenant_id redaction, tonic metadata strategy, migration shim, secrecy::SecretString).
- `_bmad-output/implementation-artifacts/7-1-credential-management-via-environment-variables.md` — this file: status flipped to `review`, all task checkboxes marked `[x]`, Dev Agent Record fully populated, Change Log entry added.

**New (4 files):**

- `config/config.example.toml` — multi-application real-world reference preserving the historical Arrosage / Bâtiments / Cultures / Meteo deployment shape, with a header comment explaining its role.
- `.env.example` — committed template for the Compose `.env` workflow; placeholders only.
- `docs/security.md` — full secrets-handling documentation (env-var convention, precedence, Quick start, migration path, redaction matrix, anti-patterns, AC#5 audit findings, references).

**Not modified:**

- `.gitignore` — `.env`, `.env.local`, `config/config.local.toml` already excluded; verified only.
- `src/main.rs` — no AC#5 mitigation needed today (audit clean).
- `src/chirpstack.rs` — `AuthInterceptor.api_token` field type unchanged (`String`); `secrecy` is Out of Scope.

### Change Log

| Date       | Author                              | Change |
|------------|-------------------------------------|--------|
| 2026-04-28 | Claude Opus 4.7 (Dev Agent)          | Story 7-1 implementation complete: sanitized `config/config.toml` (`REPLACE_ME_WITH_*` placeholders + placeholder tenant UUID), placeholder-detection in `AppConfig::validate`, hand-written redacting `Debug` impls on `ChirpstackPollerConfig`/`OpcUaConfig`, `secrets_not_logged_when_full_config_debug_formatted` regression test (incl. positive `***REDACTED***` assertion), tonic 0.14.5 metadata-leak audit (clean), `.env.example` + Docker Compose recipe, comprehensive `docs/security.md`, README/configuration cross-links, deferred-work entries, GitHub issues #81/#82/#83. Status: ready-for-dev → in-progress → review. |

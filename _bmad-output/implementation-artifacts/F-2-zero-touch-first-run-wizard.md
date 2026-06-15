# Story F.2: Zero-Touch First-Run Wizard

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As a **new operator installing opcgw**,
I want to configure **everything needed for first boot** — the ChirpStack connection (server address, tenant, API token) **and** the OPC UA password — from the browser,
so that I never have to hand-edit `config.toml` or `.env` to get a working gateway.

## Context & Problem Statement

opcgw is functionally complete (v2.2.0). Before announcing it to the ChirpStack team, the **first-touch** experience must be smooth. Today the first-run wizard (`/setup`, Epic C C-0) collects **only the OPC UA password**. A newcomer must still hand-edit `config.toml` (or set env vars) to supply ChirpStack `server_address` / `tenant_id` / `api_token` — otherwise **the gateway will not even boot**:

- `AppConfig::validate()` (`src/config.rs:1336`) **rejects** a placeholder or empty `chirpstack.api_token` unconditionally (`src/config.rs:1350-1367`). The seed `config/config.toml:71` ships `api_token = "REPLACE_ME_WITH_OPCGW_CHIRPSTACK__API_TOKEN_ENV_VAR"`, so a fresh clone fails validation at startup and never reaches the wizard.
- By contrast, `opcua.user_password` **already has a first-run carve-out** in `validate()` (`src/config.rs:1829-1840`): empty + no `OPCGW_OPCUA__USER_PASSWORD` env-var ⇒ accepted (the first-run signal). `is_first_run()` (`src/config.rs:1283`) keys off exactly this.

**F-2 closes this gap:** extend the first-run signal **and** the `validate()` carve-out to ChirpStack credentials so the gateway boots cleanly from a fresh, untouched `config.toml`/`.env`; extend the `/setup` wizard to collect ChirpStack connection + OPC UA; persist secrets (`chirpstack.api_token`, `opcua.user_password`) to `config/secrets.toml` (chmod 0600) and non-secret fields (`chirpstack.server_address`, `chirpstack.tenant_id`, OPC UA host/port) to SQLite; then reuse the existing wizard submit → graceful restart path.

## Locked Design Decisions (carry into implementation)

From the 2026-06-14 Epic F design dialogue (`epics.md` → Epic F), **do not relitigate**:

1. **`.env` boundary stays:** opcgw web-UI login user/password **and** the log-file location remain in `.env`. The wizard does **NOT** touch them. (Web auth gates the very UI; keeping its creds out of that UI is deliberate.)
2. **Secrets → `config/secrets.toml` (0600):** `chirpstack.api_token` and `opcua.user_password` are the two secrets. Non-secret config → SQLite singleton store (already authoritative, Epics C/D).
3. **Logging config stays file-based** (`log4rs.yaml`) — out of scope.
4. **Restart-on-submit is acceptable** at first boot (no connected OPC UA clients yet, so a restart is free). Reuse the existing C-0 wizard submit → graceful-restart path (`state.shutdown_token.cancel()` → `main()` exits → supervisor relaunch). Do **not** build a new apply path for the wizard.
5. **Reject placeholder/empty ChirpStack credentials on submit** — mirror the startup guard (`PLACEHOLDER_PREFIX`, empty, whitespace, `http(s)://` scheme requirement for the server address).
6. **Vanilla, no build step.** Extend `static/setup.html` in-place (vanilla JS, the existing pattern). No framework, no `node_modules`, no new runtime dependency. `setup.html` is **deliberately excluded from the F-1 shell** (`shell.js`) — keep it standalone (first-run page; the rest of the UI is gated behind it).

## Acceptance Criteria

1. **Boot from a pristine config.** With a fresh `config/config.toml` carrying the seed placeholder `chirpstack.api_token` (`REPLACE_ME_WITH_…`) — or empty `chirpstack` secret/connection fields — **and** an empty `opcua.user_password`, and with **no** `OPCGW_CHIRPSTACK__*` / `OPCGW_OPCUA__USER_PASSWORD` env-vars set, the gateway **boots successfully into first-run mode** (web server up, `/setup` served) instead of aborting at config validation. (Today it aborts on the placeholder `api_token`.)

2. **First-run signal extended.** `AppConfig::is_first_run()` returns `true` when **either** the OPC UA password is unset (existing condition) **or** the ChirpStack API token is unset/placeholder (new), with the same env-var escape hatch (a real `OPCGW_CHIRPSTACK__API_TOKEN` / `OPCGW_OPCUA__USER_PASSWORD` env-var means "configured", not first-run). The gateway is in first-run mode until **both** secrets are supplied.

3. **Validation carve-out for ChirpStack.** `AppConfig::validate()` accepts empty/placeholder `chirpstack.api_token` (and empty `server_address`/`tenant_id`) **only** in the first-run signal state (no corresponding env-var set), exactly mirroring the existing `opcua.user_password` carve-out (`src/config.rs:1829-1840`). When the env-var IS set but resolves to empty/placeholder, validation still **rejects** (operator-error surfacing, mirroring the opcua branch).

4. **Wizard collects ChirpStack + OPC UA.** `GET /setup` renders fields for: ChirpStack `server_address`, `tenant_id`, `api_token`, plus the existing OPC UA password (and confirm). OPC UA host/port MAY be collected with sensible pre-filled defaults (`0.0.0.0` / `4840`); if omitted, defaults apply. The page stays a standalone vanilla form (no shell), styled consistently with the existing wizard.

5. **Secrets persisted to `secrets.toml` (0600).** On submit, `chirpstack.api_token` **and** `opcua.user_password` are written to `config/secrets.toml` under `[chirpstack]` / `[opcua]` sections respectively, file mode **0600**, via the existing atomic temp-file+rename path (`write_secrets_toml`). Both secrets land in **one** file write (no partial state). TOML escaping is applied to both values (reuse `toml_escape_string`).

6. **Non-secret config persisted to SQLite.** `chirpstack.server_address`, `chirpstack.tenant_id`, and any collected OPC UA host/port are written to the SQLite singleton store via the existing `SqliteBackend::write_singleton_section` path (the same surface `put_singleton_section` uses), **not** to `config.toml` (which stays bootstrap-seed-only). Secret fields are **never** written to SQLite (enforced by the existing `SECRET_FIELDS_BY_SECTION` skip-list).

7. **Server-side validation on submit.** The POST handler rejects, with a structured `{ error, reason }` body and a specific `reason` code per field:
   - empty `server_address`; `server_address` not starting with `http://`/`https://`;
   - empty or `PLACEHOLDER_PREFIX`-prefixed `api_token`; control-char / over-length token (reuse the password-validator bounds);
   - empty `tenant_id`;
   - the existing OPC UA password rules (unchanged: empty / whitespace-bracketed / whitespace-only / placeholder / control-char / >256 / confirmation-mismatch).
   The candidate config is run through `AppConfig::validate()` (in its non-first-run form) before persistence so the wizard cannot produce a config that fails the next boot.

8. **`.env` boundary honoured.** The wizard does **not** read, write, or reference the web-UI login user/password or the log-file location. No new field for them; no `.env` mutation anywhere in the F-2 path.

9. **Restart applies the config.** After a successful submit the handler persists secrets + SQLite, emits the audit events (`setup_password_accepted` / a new ChirpStack-aware event + the `config_reload` event), builds the HTTP 200 response, **then** triggers the existing graceful restart (`state.shutdown_token.cancel()`). On the next boot, `secrets.toml` + SQLite supply the full config, `is_first_run()` returns `false`, basic auth + the poller come online with real ChirpStack creds, and `/setup` returns **410 Gone**.

10. **One-shot + race-safe.** The existing `is_first_run` `compare_exchange` one-shot guarantee (`src/web/setup.rs:518`) still holds for the combined submit: exactly one concurrent submitter wins; a write failure reverts the flip so the operator can retry (existing iter-3 P2 behaviour preserved). A partial failure (secrets write succeeds but SQLite write fails, or vice-versa) leaves the gateway recoverable: the flip is reverted and **no** restart is triggered, so a retry is possible. (See Dev Notes "Ordering & failure atomicity".)

11. **Placeholder rejection is real.** Submitting the seed placeholder values verbatim (`REPLACE_ME_WITH_…` token, the all-zeros tenant UUID is acceptable as a real value unless empty) is rejected with the appropriate `reason`, never persisted, and never triggers a restart.

12. **Docs synced.** `README.md` (first-run / quick-start), `docs/security.md` (secrets.toml now also holds `chirpstack.api_token`), the DocBook manual (`docs/manual/opcgw-user-manual.xml` — first-run wizard chapter), and `config/config.toml` comments are updated to describe the zero-touch browser-only first boot. `.env`/`.env.example` ChirpStack-token guidance updated to note the wizard as the primary path. (Per CLAUDE.md doc-sync rule.)

13. **Tests.** New/updated tests prove: pristine-config boot reaches first-run mode (extends `is_first_run` + validate carve-out unit tests); wizard writes both secrets to `secrets.toml` at 0600 with correct sections; non-secret fields land in SQLite and secrets never do; placeholder/empty/scheme rejection per field; combined-submit one-shot + revert-on-write-failure; `/setup` → 410 after configuration; web-auth creds + log path untouched. Existing `tests/web_setup_wizard.rs` extended; existing `validate_password` tests unchanged.

## Tasks / Subtasks

- [x] **Task 1 — Extend first-run detection & validation carve-out** (AC: #1, #2, #3, #11)
  - [x] In `src/config.rs`, extend `is_first_run()` so it also returns `true` when `chirpstack.api_token` is empty or `PLACEHOLDER_PREFIX`-prefixed AND `OPCGW_CHIRPSTACK__API_TOKEN` is not set to a non-empty value (new `chirpstack_token_missing()` helper, mirrors the `OPCGW_OPCUA__USER_PASSWORD` logic). Gateway is first-run while **either** secret is missing.
  - [x] In `AppConfig::validate()`, added a first-run carve-out for `chirpstack.api_token` (and tolerate empty `server_address`/`tenant_id` via `cs_first_run`) mirroring the `opcua.user_password` branch: accept empty/placeholder iff no env-var; reject when env-var is set-but-empty. Kept the `http(s)://` scheme check active when `server_address` is non-empty.
  - [x] Added `#[serde(default)]` on `server_address`/`api_token`/`tenant_id` so a truly empty `config.toml` deserialises to empty (not only the placeholder seed).
  - [x] Unit tests added (108/0 in `config::tests`): `chirpstack_token_missing` true/false matrix; `is_first_run` true when token placeholder even with opcua password set; validate accepts pristine first-run; validate rejects token when env-var set-but-empty; validate still rejects bad scheme in first-run. Updated 2 pre-F-2 Story 7-1 tests to the new contract (`test_validation_accepts_placeholder_api_token_in_first_run`, `test_from_path_with_and_without_placeholder_env_vars`).

- [x] **Task 2 — Extend the secrets writer** (AC: #5, #10)
  - [x] Generalised `write_secrets_toml` to take `SecretsToWrite { chirpstack_api_token, opcua_password }` and write **both** `[chirpstack].api_token` + `[opcua].user_password` in one atomic body (temp → chmod 0600 → rename). `toml_escape_string` on both.
  - [x] Preserved the categorised `SecretsWriteError` mapping unchanged.
  - [x] Unit tests: both sections + secrets present; both round-trip through figment to exact originals; mode 0600.

- [x] **Task 3 — Extend the request schema & validation** (AC: #4, #7, #11)
  - [x] Renamed `SetupPasswordRequest` → `SetupRequest` with `server_address`/`tenant_id`/`api_token` + `password`/`password_confirm`, kept `#[serde(deny_unknown_fields)]`. Route renamed `/api/setup/password` → `/api/setup` in lock-step across `mod.rs` (route), `setup.rs` (`WIZARD_BYPASS_EXACT` + doc comments + bypass tests), `csrf.rs` (first-run exemption), `setup.html` (fetch URL). **OPC UA host/port NOT collected** — existing `0.0.0.0:4840` defaults apply (AC#4 permits omission); keeps the SQLite write to the chirpstack section only.
  - [x] Added `validate_chirpstack(...)` (empty/whitespace/scheme/length on server_address; empty/whitespace/length on tenant_id; empty/whitespace/placeholder/control-char/length on api_token), one-at-a-time reason codes. Unit tests added.
  - [x] Candidate `AppConfig` overlay + `validate()` before persistence → 400 `config_invalid` on a bad combination.

- [x] **Task 4 — Persist non-secret config to SQLite + orchestrate submit** (AC: #5, #6, #9, #10)
  - [x] `setup_post`: validate CS then password → candidate validate → `compare_exchange` → write chirpstack `server_address`/`tenant_id` to SQLite (`write_singleton_section`) → write both secrets → audit (`setup_accepted` + `config_reload`) → build 200 → `shutdown_token.cancel()`.
  - [x] Ordering & failure atomicity: SQLite first, then secrets, both before the restart; on **either** write failure, `store(is_first_run = true)` revert + structured 500 (`sqlite_write_failed` / categorised secrets reason), no `shutdown_token.cancel()`. Documented in code (no cross-file txn → revert-on-any-failure is the safety net).
  - [x] Only non-secret keys (`server_address`, `tenant_id`) are passed to the SQLite write; secrets go exclusively to secrets.toml. (Migration is deferred while table empty in first-run, so the section-replace wipes nothing.)

- [x] **Task 5 — Wizard UI** (AC: #4, #8)
  - [x] Extended `static/setup.html`: ChirpStack section (server address `text`, tenant id `text`, API token `password`) + OPC UA section (password + confirm); standalone no-shell layout; new `h2` section styling. No web-auth/log-path fields.
  - [x] Vanilla JS: `validateClientSide(fields)` mirrors all server reason codes; POST broadened body to `/api/setup`; `REASON_MESSAGES` extended with all new codes; success-countdown-reload preserved. `{{PLACEHOLDER_PREFIX}}` substitution reused for the api_token placeholder check. `node --check` of the inline script passes.

- [x] **Task 6 — Docs & config comments** (AC: #12)
  - [x] Updated `README.md` (first-run section rewritten for zero-touch + Planning row F-2 → done), `docs/security.md` (env-var table + first-run section: both secrets in secrets.toml, route rename, failure atomicity), DocBook manual `sec-first-run-wizard` (ChirpStack + OPC UA procedure, `.env` boundary note, xmllint clean), `config/config.toml` header (zero-touch quick-start), `.env.example` (secrets now optional/wizard-collected).

- [x] **Task 7 — Tests** (AC: #13)
  - [x] Extended `tests/web_setup_wizard.rs` (15/0): served page references `/api/setup` + CS fields; happy-path writes both secrets (0600) + chirpstack SQLite rows + asserts api_token NOT in SQLite; per-field rejection (empty pw, mismatch, whitespace, bad scheme, placeholder token); `wizard_post_does_not_write_env_file` (AC#8); `/setup` → 410 post-config.
  - [x] Added `src/config.rs` unit tests (108/0) for extended `is_first_run`/`chirpstack_token_missing`/validate carve-out; `src/web/setup.rs` unit tests (23/0) for `validate_chirpstack` + two-secret writer.
  - [x] Gates ALL GREEN: full `cargo test` exit 0, `cargo clippy --all-targets -- -D warnings` clean, `node --check` on the inline wizard script, `xmllint --noout` on the manual.

## Dev Notes

### Source tree — exact files to touch
- `src/config.rs` — `is_first_run()` (`:1283`), `validate()` ChirpStack block (`:1340-1367`) + the opcua carve-out reference (`:1829-1840`). Field defs `:98-112`.
- `src/web/setup.rs` — `SetupPasswordRequest` (`:242`), `validate_password` (`:271`), `setup_post` (`:326`), `write_secrets_toml` (`:749`), `WIZARD_BYPASS_EXACT` (`:90`), `is_wizard_bypass_path` (`:103`), `SecretsWriteError` (`:664`).
- `src/web/mod.rs` — route wiring (`:655-674`), `AppState` fields `sqlite_config` (`:307`), `static_dir` (`:317`), `secrets_path` (`:343`), `stage_config_write` (`:391`); CSRF-exemption wiring (`:529`, `:605`).
- `src/web/singleton_config.rs` — reference pattern for the candidate-overlay + SQLite write (`put_singleton_section`, `:121-329`).
- `src/storage/migrate_singleton_config.rs` — `SECRET_FIELDS_BY_SECTION` (`:76`), `KNOWN_SECTIONS` (`:86`), `secret_fields_for_section` (`:89`), placeholder skip guard (`:151-160`).
- `static/setup.html` — the whole wizard page.
- `config/config.toml` — seed comments; `tests/web_setup_wizard.rs` — integration tests.

### The central technical challenge — boot in first-run with no ChirpStack creds
Today validation blocks. The fix is **symmetry with the existing OPC UA carve-out**, not new machinery:
- `is_first_run()` becomes "OPC UA password missing **OR** ChirpStack token missing (each with its env-var escape)".
- `validate()` tolerates empty/placeholder ChirpStack secret/connection fields **only** in that state.
- The poller (`ChirpstackPoller`) already tolerates connection/auth failure: it retries and tracks server availability via the internal `cp0` device (per CLAUDE.md). In first-run mode it will simply fail-and-retry against the placeholder creds until the wizard restart supplies real ones — **no special poller code needed**. Document this; do not add a "pause poller in first-run" branch unless a test shows log-noise is unacceptable (out of scope — note as a possible follow-up).

### Secrets file — both secrets, one write
`secrets.toml` is merged by figment as a plain TOML layer (`from_path_with_sqlite` `:1180-1183` merges `Toml::string(secrets_body)`), so adding a `[chirpstack]` section "just works" on next boot — `api_token` from secrets overrides the placeholder in `config.toml`. The migration placeholder-guard (`migrate_singleton_config.rs:151`) already defers SQLite singleton migration until **both** `api_token` and `user_password` are non-placeholder, so the first post-wizard boot migrates cleanly.

### Ordering & failure atomicity (AC #10)
`setup_post` wins `compare_exchange(true→false)`, then performs two writes (SQLite singleton + secrets.toml). If **either** fails: `store(is_first_run = true)` to revert (existing pattern, `src/web/setup.rs:620`), return structured 500, and **do not** cancel `shutdown_token`. The gateway keeps running in first-run mode; the operator retries. Pick a deterministic order and keep both writes **before** the `shutdown_token.cancel()` so a failure never restarts into a half-configured state. SQLite writes of non-secret singleton rows are idempotent/overwriting; the secrets file is atomic (temp+rename). There is no cross-file transaction, so the revert-on-any-failure + no-restart rule is the safety net — call it out in code comments.

### Routing — rename to `/api/setup`, keep the bypass allowlist in lock-step
**DECISION (Guy, 2026-06-15): rename `/api/setup/password` → `/api/setup`.** The old path described a password-only submit; the broadened submission (ChirpStack connection + OPC UA) makes `/api/setup` the accurate name. Update **all** of these in the same change, or the wizard breaks / a path silently loses its auth-bypass:
- the route registration (`src/web/mod.rs:664`) — `axum::routing::post(setup::setup_post)` (rename handler too);
- `WIZARD_BYPASS_EXACT` (`src/web/setup.rs:90`) — replace the `"/api/setup/password"` entry with `"/api/setup"`;
- the first-run-only CSRF exemption (`src/web/csrf.rs` — the `POST /api/setup/password` exemption) → `/api/setup`;
- the client `fetch('/api/setup/password', …)` in `static/setup.html:231` → `/api/setup`;
- every `is_wizard_bypass_*` test (`src/web/setup.rs:996-1037`) and any integration test in `tests/web_setup_wizard.rs` that posts to `/api/setup/password`.

The exact-match allowlist hardening (iter-2 P3/P12, iter-3 P6) must not regress — keep exact-match only, no prefix/suffix matching, and ensure the **old** `/api/setup/password` path is NOT left in the allowlist (it would be a dead auth-exempt path). Add a bypass test asserting `is_wizard_bypass_path("/api/setup")` is true and `("/api/setup/password")` is now false.

### Anti-patterns to avoid (do NOT)
- Do **not** write any secret (`api_token`, `user_password`) to SQLite or to `config.toml`.
- Do **not** add a web-auth-credentials or log-path field to the wizard (locked `.env` boundary).
- Do **not** mutate `.env` from the gateway.
- Do **not** introduce a build step, framework, or `node_modules` for the UI.
- Do **not** broaden `WIZARD_BYPASS_EXACT` to prefix/suffix matching.
- Do **not** loosen the OPC UA password rules (they are a tested contract; only add the ChirpStack rules alongside).
- Do **not** build a new soft-restart path for the wizard — first-run restart-on-submit via `shutdown_token.cancel()` (container restart) is the locked, accepted approach. (F-0's soft restart is for *post*-first-run staged Apply, not first boot.)

### Security
- `secrets.toml` 0600 (atomic temp+rename, chmod before rename) — existing path; extend, don't weaken.
- Keep the first-run-only CSRF exemption + strict `application/json` Content-Type check + same-origin Origin check on the submit route (`src/web/setup.rs:363-453`) — they guard the auth-less wizard surface against drive-by LAN form-posts. The broadened body must still go through all three.
- Log secrets' **filename only**, never the path or value (existing M2 redaction); never log `api_token`/`user_password` values.
- Run the Epic-completion security checklist items at review (no hardcoded secrets, input validation on all wizard fields, error messages don't leak paths — `put_singleton_section`'s I1-F6 static-hint pattern is the precedent).

### Testing standards
- Integration tests subprocess/inproc-spawn the router via the `test_support` helpers; assert on served-HTML markers + JSON bodies + on-disk `secrets.toml` mode and SQLite rows (see existing `tests/web_setup_wizard.rs`, `main_apply_restart.rs` patterns). Web tests assert **served-HTML** markers (not shell-injected nav) — `setup.html` is shell-excluded, so its served markup is authoritative; keep the form fields + script in the served HTML.
- Reuse `tempfile::tempdir()` for `secrets_path` + a temp SQLite pool (the `AppState` test constructor at `src/web/mod.rs:1182` shows the shape).

### Project Structure Notes
- Aligns with the established C-0 wizard + D-1 singleton-editor + F-0 staged-apply structure. No new module needed; F-2 broadens `setup.rs` and `config.rs`. `setup.rs` is well under the 5000-line limit; keep the new ChirpStack validation alongside `validate_password`.
- One variance to note: the wizard now writes to **two** persistence surfaces (secrets.toml + SQLite) in one request — previously it wrote only secrets.toml. This is intentional and matches the Epic F apply-model (non-secrets in SQLite, secrets in secrets.toml).

### References
- [Source: _bmad-output/planning-artifacts/epics.md#Epic F — Story F.2: Zero-Touch First-Run Wizard]
- [Source: src/web/setup.rs#setup_post,write_secrets_toml,validate_password,WIZARD_BYPASS_EXACT]
- [Source: src/config.rs#is_first_run (L1283), validate (L1336-1367, opcua carve-out L1829-1840)]
- [Source: src/web/singleton_config.rs#put_singleton_section (candidate-overlay + SQLite write pattern)]
- [Source: src/storage/migrate_singleton_config.rs#SECRET_FIELDS_BY_SECTION, placeholder skip-guard L151-160]
- [Source: static/setup.html (existing wizard markup + vanilla JS)]
- [Source: config/config.toml#L62-76,L110-135 (seed placeholders)]
- [Source: CLAUDE.md#Documentation Sync, #Security & Quality Assurance, #Source files under 5000 lines]
- [Prior story: F-1-unified-web-shell.md — setup.html deliberately excluded from shell.js]
- [Prior story: F-0-staged-config-apply.md — soft-restart is post-first-run only; wizard keeps container restart]

## Dev Agent Record

### Agent Model Used

claude-opus-4-8[1m] (Opus 4.8, 1M context)

### Debug Log References

- `cargo test` (full suite): exit 0, all binaries pass.
- `cargo clippy --all-targets -- -D warnings`: clean.
- `xmllint --noout docs/manual/opcgw-user-manual.xml`: valid.
- `node --check` on the extracted `static/setup.html` inline script: OK.
- Focused: `config::tests` 108/0, `web::setup::tests` 23/0, `web::csrf::tests` 16/0, `tests/web_setup_wizard` 15/0.

### Completion Notes List

- **Central fix (AC#1-3):** extended the first-run signal + `validate()` carve-out to ChirpStack, symmetrically with the long-standing OPC UA password carve-out. New `AppConfig::chirpstack_token_missing()`; `is_first_run()` is now "OPC UA password missing OR ChirpStack token missing". `validate()` accepts empty/placeholder ChirpStack creds only in the first-run signal state; env-var-set-but-empty is still rejected; the `http(s)://` scheme rule still fires on a present-but-malformed address. Added `#[serde(default)]` to `server_address`/`api_token`/`tenant_id` so a truly-empty `config.toml` deserialises.
- **Two pre-F-2 Story 7-1 tests updated** to the new contract: a placeholder `api_token` is now first-run-accepted (not an unconditional boot error); the combined-placeholder `from_path` test now asserts only the OPC UA placeholder fires (ChirpStack placeholder is accepted).
- **Wizard broadened:** `SetupPasswordRequest` → `SetupRequest` (+ `server_address`/`tenant_id`/`api_token`); new `validate_chirpstack`; `write_secrets_toml` now writes BOTH `[chirpstack].api_token` + `[opcua].user_password` via `SecretsToWrite` in one atomic 0600 write; `setup_post` validates → candidate `AppConfig::validate()` → `compare_exchange` → SQLite (chirpstack server/tenant) → secrets → audit (`setup_accepted` + `config_reload`) → 200 → `shutdown_token.cancel()`.
- **Route renamed** `/api/setup/password` → `/api/setup` in lock-step (route, `WIZARD_BYPASS_EXACT`, CSRF exemption, `setup.html` fetch, bypass tests). The old path is asserted to NO LONGER bypass auth.
- **Failure atomicity (AC#10):** SQLite-then-secrets, both before restart; either failure reverts the `is_first_run` flip + returns 500 without restarting. No cross-file txn — revert-on-any-failure is the safety net.
- **Scope decision:** OPC UA host/port are NOT collected (existing `0.0.0.0:4840` defaults apply — AC#4 permits omission); keeps the SQLite write to the chirpstack section only and lowers risk.
- **`.env` boundary honoured (AC#8):** the wizard writes only `secrets.toml` + SQLite; `wizard_post_does_not_write_env_file` asserts no `.env` is created. (Note: web-UI auth reuses `[opcua].user_password`, which IS a wizard-collected secret per the locked design — the wizard never writes `.env` itself.)
- **No new dependencies.** No build step / framework / `node_modules` added.

### File List

- `src/config.rs` — `chirpstack_token_missing()`, extended `is_first_run()` + `validate()` carve-out, `#[serde(default)]` on 3 ChirpStack fields; unit tests (new + 2 updated).
- `src/web/setup.rs` — `SetupRequest`, `validate_chirpstack`, `SecretsToWrite` + two-secret `write_secrets_toml`, rewritten `setup_post` (candidate-validate + SQLite + both secrets + atomicity), `WIZARD_BYPASS_EXACT` rename, audit-event rename; unit tests.
- `src/web/mod.rs` — route `/api/setup/password` → `/api/setup`; doc-comment fixes.
- `src/web/csrf.rs` — first-run CSRF exemption path → `/api/setup`; doc-comment fixes.
- `src/web/auth.rs` — doc-comment fix.
- `static/setup.html` — ChirpStack fields + section styling, broadened JS (`validateClientSide(fields)`, `/api/setup` fetch, extended `REASON_MESSAGES`).
- `tests/web_setup_wizard.rs` — updated existing tests to the new route/body; new tests (both-secrets + SQLite + secret-absent, CS rejections, no-`.env`).
- `config/config.toml` — zero-touch quick-start header.
- `docs/security.md` — env-var table + first-run section.
- `docs/manual/opcgw-user-manual.xml` — `sec-first-run-wizard` rewrite.
- `README.md` — first-run section + Planning row F-2 → done.
- `.env.example` — secrets now optional / wizard-collected.
- `_bmad-output/implementation-artifacts/F-2-zero-touch-first-run-wizard.md` — this story; `sprint-status.yaml` status.

## Senior Developer Review (AI)

**Reviewed:** 2026-06-15 · **Method:** 3 parallel adversarial layers (Blind Hunter / Edge Case Hunter / Acceptance Auditor) on a different model than the implementer · **Iterations:** 2 patch rounds + iter-3 verification · **Outcome:** APPROVED — loop terminated, only LOW findings remain.

### Iteration 1 — 1 HIGH (converged across all 3 layers) + LOWs
- **HIGH (FIXED): partial SQLite write poisoned the D-0 singleton migration.** The wizard's `write_singleton_section("chirpstack", [server_address, tenant_id])` left `singleton_config` non-empty WITHOUT the `d0_migration_done` flag → next boot, migration Guard 2 (`count_singleton_config() > 0`) back-fills the flag and short-circuits to `AlreadyMigrated`, so `global`/`opcua`/`web` (+ remaining chirpstack keys) NEVER migrate, with no boot able to self-heal (Epic D "SQLite authoritative" model silently defeated). **Fix:** run the FULL `migrate_singleton_toml_to_sqlite(&candidate, …)` (writes all non-secret sections + the done-flag atomically) instead of a partial write; reorder to **secrets-file-first, migration-second** so a secrets-ok+migration-fail leaves an empty table → in-process retry re-runs cleanly. Regression guard added (`wizard_post_persists_…`: asserts all 4 sections present + `is_d0_migration_done()`).
- LOW (FIXED): control-char rejection added to `server_address`/`tenant_id`; placeholder check on `tenant_id`; `.env.example` web-login/secret contradiction clarified; weak `contains` test assertion tightened to exact JSON.
- Dismissed: candidate-validate-gates-whole-config (Edge) — the running config already passed identical `validate()` at boot, so non-cred fields can't newly fail.

### Iteration 2 (mandatory — iter-1 introduced new flow) — 1 HIGH + fallout
- **HIGH (FIXED): prefix-vs-marker asymmetry.** Validators rejected `starts_with(PLACEHOLDER_PREFIX="REPLACE_ME_WITH_")` but migration Guard 3 defers on `contains(PLACEHOLDER_MARKER="REPLACE_ME_WITH_OPCGW_")`. A secret like `abc-REPLACE_ME_WITH_OPCGW_-xyz` passed validation but trip­ped Guard 3 → wizard 500s forever + migration permanently blocked. **Fix:** all secret placeholder checks (wizard `validate_chirpstack`/`validate_password` + boot `AppConfig::validate` for api_token + user_password) now reject `contains(PLACEHOLDER_PREFIX)` — a strict superset of the migration's guard, so a validated secret can never trip Guard 3. Unit tests pin the mid-string case at both the wizard and boot layers. First-run carve-out preserved (`cs_first_run` / `is_first_run` still key on `starts_with`, so the seed placeholder still boots into the wizard).
- MED/LOW (FIXED): migration-failure outcomes now get honest per-outcome `reason`s (`already_configured`/`config_invalid`/`sqlite_write_failed`) instead of a blanket `sqlite_write_failed`; `outcome` kept low-cardinality with the DB error logged separately; secrets-write-failure event renamed `setup_password_persistence_failed` → `setup_persistence_failed` (uniform `setup_*` family). Blind "candidate lacks merged secrets" was a false alarm (candidate overlays the validated secrets before the migration).

### Iteration 3 — verification: CLEAN
Confirmed `contains(PREFIX) ⊇ contains(MARKER)` fully closes the gap; the `starts_with`→`contains` change does not break the first-run carve-out (seed placeholder still accepted); no false-positive on realistic secrets (JWTs/passwords don't embed "REPLACE_ME_WITH_"); outcome match exhaustive, no panic; tests are not fake-guards. Loop terminates.

### Accepted / deferred (LOW — see deferred-work.md)
- Env-var-supplied secret silently overridden by a wizard value in a mixed env+wizard config (niche, no breakage/security/data-loss) — downgraded LOW, **flagged to Guy for explicit accept**.
- api_token 2048-char bound (deliberate, JWTs) + 4 KiB body-limit pre-emption — accepted.
- Audit-event rename (pre-announcement, documented) — accepted.

### Gates (final)
`cargo test` exit 0 · `cargo clippy --all-targets -- -D warnings` clean · `xmllint` clean · `node --check` OK. config 109/0, web::setup 25/0, web::csrf 16/0, tests/web_setup_wizard 15/0.

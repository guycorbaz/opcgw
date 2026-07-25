# Story J.2: Enforce the Env-Var Allowlist — Web/SQLite Becomes Authoritative for the Editable Set

Status: ready-for-dev

## Story

As an **operator managing opcgw through the web Admin page**,
I want non-allowlisted `OPCGW_*` environment variables to be **ignored** (with a WARN naming each ignored key) instead of silently outranking my Admin-page edits,
so that what I see and save in the web UI is genuinely what the gateway runs with.

GitHub issue: **CR #169**, with **#168** as the problem statement (env-shadows-database trap). Epic J "Config Authority & Command Responsiveness", target **v2.8.0**. Third and final story of Epic J.

**The defect (#168, field-observed 2026-07-20):** figment precedence is `env > SQLite > TOML > default` (`src/config.rs:1226–1244`). Any `OPCGW_<SECTION>__<FIELD>` var therefore silently outranks the web/SQLite value — the Ignition churn incident was ultimately `OPCGW_OPCUA__HOST_IP_ADDRESS` in `.env` shadowing the web-set `host_ip_address="opcgw"`, forcing the gateway to advertise `opc.tcp://0.0.0.0:4855`. v2.7.1 shipped `env_shadows_singleton_config` (WARN, `src/config.rs:1349`) as the **deprecation notice**; this story is the enforcement half.

**Owner decisions (LOCKED):** 2026-07-22 — ENFORCE: non-allowlisted `OPCGW_*` vars are **ignored** (not merged into the figment stack) and produce a WARN naming the ignored key. 2026-07-25 — allowlist designed from the **current panoramix `.env`** (10 active vars, captured below); of the two env↔SQLite mismatches, **SQLite wins** (`stale_threshold_seconds` 10800, `global.debug` true).

**BREAKING for existing deployments.** Panoramix itself runs 10 active `OPCGW_*` vars; three of them (`OPCGW_GLOBAL__DEBUG`, `OPCGW_CHIRPSTACK__STREAM_ALL_DEVICES`, `OPCGW_OPCUA__STALE_THRESHOLD_SECONDS`) become ignored by this story. Mitigated by: the v2.7.1 shadow-WARN deprecation window, the per-key ignore WARN with an actionable hint, and a **CHANGELOG migration note** (see AC#9). ⚠️ `stream_all_devices` currently has **no SQLite row on the NAS** (D-0 migration pre-dated the field — `is_d0_migration_done` gate, `src/storage/sqlite.rs:3388`), so ignoring its env var would silently flip streaming off; the migration note MUST tell the operator to save it via the Admin page **before** upgrading (the #155 GET-backfill already renders it; a section save persists it).

## The allowlist (design LOCKED)

**Rule: the env filter applies ONLY to figment keys in `KNOWN_SECTIONS` (`global`, `chirpstack`, `opcua`, `web` — `src/storage/migrate_singleton_config.rs:86`), i.e. exactly the sections the web Admin page can edit (`put_singleton_section` gate, `src/web/singleton_config.rs:185`).** Within those sections:

| Allowed through env (figment) | Why |
|---|---|
| `chirpstack.api_token`, `opcua.user_password` | `SECRET_FIELDS_BY_SECTION` (`migrate_singleton_config.rs:76`) — never in SQLite, needed pre-SQLite (`is_first_run` `src/config.rs:1471`, `validate` probes at `:1570`/`:2071`), unattended provisioning |
| `web.enabled`, `web.bind_address`, `web.port` | Bootstrap: must serve `/setup` before SQLite exists; `OPCGW_WEB__PORT` is consumed by `docker-compose.yml:28` for port publishing |
| `opcua.host_port` | Bootstrap + `docker-compose.yml:24` port mapping AND the `:53` healthcheck (`/dev/tcp/127.0.0.1/${OPCGW_OPCUA__HOST_PORT:-4840}`) |
| **Everything else in the four sections** | **BLOCKED** — ignored, not merged; per-key WARN `env_var_ignored` |

**Explicitly-decided blocked vars (party session #2, 2026-07-25 — both match the owner's own 2026-07-20 panoramix `.env` cleanup, which removed exactly these as "web/SQLite-managed"):**
- `OPCGW_CHIRPSTACK__SERVER_ADDRESS` — the most-advertised override in the repo (`.env.example:50–51`, `docs/configuration.md:501`) → **blocked**. Onboarding is the `/setup` wizard (Epic F) or TOML seeding; pure-env unattended provisioning of the server address is retired. MUST lead the AC#9 migration note.
- `OPCGW_OPCUA__USER_NAME` — deliberately NOT a secret (D-0 I1-F12, `migrate_singleton_config.rs:71–74`) → **blocked**; the auth pair splits sources (password stays env-capable as a secret, name is Admin-page property). `.env.example:32–38`'s promise that the name "stays exclusively in this file" MUST be rewritten (Task 6), and the migration note MUST cover "your web Basic-auth login name comes from the Admin page after v2.8.0".

**Untouched by the filter (allowlisted by construction):**
- Sections **outside** `KNOWN_SECTIONS`: `[storage]` (bootstrap `database_path`!), `[command_validation]`, `[logging]` (`OPCGW_LOGGING__DIR`/`__LEVEL` flow through BOTH figment stacks incl. the logging peek `src/main.rs:108–114`) and `[[application]]` (not env-addressable anyway). These are file/env-managed by design and invisible to the Admin page — they cannot shadow it.
- **Out-of-figment direct reads** (never pass through the provider): `OPCGW_LOG_DIR` (`src/main.rs:126`), `OPCGW_LOG_LEVEL` (`src/main.rs:184`), `CONFIG_PATH` (`src/main.rs:631` — unprefixed), `OPCGW_STORAGE_QUERY_BUDGET_MS` / `OPCGW_BATCH_WRITE_BUDGET_MS` (`src/utils.rs:536,562`), `OPCGW_ERROR_EVENT_CAP` (`src/utils.rs:625`), and the two secret-var probes named above. The story does NOT touch these reads; the doc task (#7) lists them as the env-only surface.

Current panoramix `.env` (2026-07-25) mapped onto the rule: **kept** = API_TOKEN ✓, USER_PASSWORD ✓, WEB__ENABLED ✓, WEB__BIND_ADDRESS ✓, WEB__PORT ✓, OPCUA__HOST_PORT ✓, LOG_LEVEL ✓ (short form, out of figment); **ignored after J-2** = GLOBAL__DEBUG (SQLite `true` wins), OPCUA__STALE_THRESHOLD_SECONDS (SQLite `10800` wins — owner-preferred), CHIRPSTACK__STREAM_ALL_DEVICES (⚠️ needs the pre-upgrade Admin-page save, see migration note).

## Acceptance Criteria

1. **Blocked keys are ignored, not merged.** With `OPCGW_CHIRPSTACK__POLLING_FREQUENCY=5` set and a SQLite row `("chirpstack","polling_frequency","10")`, the effective config is **10** (SQLite wins; pre-J-2 it was 5). With no SQLite row, the TOML value (or default) wins. The env value must be invisible to figment `extract()` — filtered at the provider, not post-corrected.

2. **Allowlisted keys keep full precedence.** `OPCGW_WEB__PORT`, `OPCGW_WEB__BIND_ADDRESS`, `OPCGW_WEB__ENABLED`, `OPCGW_OPCUA__HOST_PORT`, `OPCGW_CHIRPSTACK__API_TOKEN`, `OPCGW_OPCUA__USER_PASSWORD` still override SQLite/TOML/default exactly as today (`env > SQLite > TOML > default`). The **precedence regression suite is mandatory** (config-spine story): re-spec `t03_precedence_env_beats_sqlite` (`tests/d2_figment_provider.rs:146` — currently uses the now-blocked `POLLING_FREQUENCY`) onto an allowlisted key, and add `t03b`: a blocked key does NOT beat SQLite. `t04`/`t05`/`t06` must stay green untouched.

3. **Out-of-scope surfaces untouched.** `[storage]`, `[command_validation]`, `[logging]` env overrides still work through figment (regression test: `OPCGW_LOGGING__DIR` still reaches both `from_path_inner` AND the `peek_logging_config` stack); the direct reads (`OPCGW_LOG_DIR`, `OPCGW_LOG_LEVEL`, budgets, cap, `CONFIG_PATH`) are not modified. Malformed keys (no `__`, unknown section) pass through the filter unfiltered — figment already ignores unknown keys; they must not WARN as "ignored" (they were never mergeable).

4. **Both production env-merge sites are filtered.** `src/config.rs:1253` (the single stack builder `from_path_inner` — covers bootstrap `main.rs:733`, D-2 reload `main.rs:1039`, and Apply `reload_effective_config` `main.rs:305/1691`) and `src/main.rs:111` (`peek_logging_config`). Extract ONE shared provider-builder (e.g. `pub fn opcgw_env_provider() -> figment::providers::Env` in config.rs applying `.filter(..)`) used by both — do NOT hand-roll two filters. The in-file test stacks that replicate the provider (`src/config.rs:4501,4672,4730,4790,4848`) must switch to the shared builder too, or they test a stack production no longer runs. ⚠️ Doing so BREAKS `test_chirpstack_nested_env_override` (`:4627`, asserts `OPCGW_CHIRPSTACK__SERVER_ADDRESS` overrides TOML — a blocked key post-J-2): re-spec it onto an allowlisted key AND keep an inverted twin asserting the blocked key no longer overrides. The other four stacks survive (WEB__PORT allowed, USER_PASSWORD secret, LOGGING__DIR ×2 outside KNOWN_SECTIONS).

5. **`env_var_ignored` WARN, once per boot, per key.** Each blocked-and-present var emits `warn!(event = "env_var_ignored", env_var, section, key, recommended_action = "remove this override and manage the field on the web Admin page")`. Value is NOT logged (may be sensitive). Emission is once-per-boot via a function-local `AtomicBool` guard exactly like `ENV_SHADOWS_SINGLETON_WARNING_EMITTED` (`src/main.rs:610`) — an Apply re-load must not re-spam. Emit from the same enumeration pass (mirror the `std::env::vars()` scan shape of `maybe_warn_env_shadows_singleton`, `src/config.rs:1365–1376`), NOT from inside the figment filter closure (which runs per-extract and cannot dedup).

6. **`maybe_warn_env_shadows_singleton` updated for the new world.** A blocked key no longer *shadows* anything (it is ignored), so the shadow-WARN must skip non-allowlisted keys — otherwise it would emit a FALSE "env wins over the Admin page" warning for a var that no longer wins. Post-J-2 the shadow WARN fires only for **allowlisted** keys that both exist in env and have a SQLite row (e.g. `OPCGW_WEB__PORT` vs the `web.port` row — still true, env still wins for those). Update the four existing tests (`src/config.rs:5670–5741`) — validation-verified blast radius: `env_shadows_singleton_flags_shadowed_field` (:5670, `OPCGW_OPCUA__HOST_IP_ADDRESS` now blocked → expects 1, gets 0) AND `env_shadows_singleton_once_per_boot_guard` (:5725, same key → first call returns 0) both FAIL → re-spec onto an allowlisted key (e.g. `OPCGW_WEB__PORT`); `env_shadows_singleton_ignores_keys_not_in_sqlite` (:5689) keeps passing **for the wrong reason** (the allowlist gate rejects the key before the row lookup — the fake-regression-guard class) → re-spec onto an allowlisted key with no matching row; `skips_secret_fields` (:5706) unaffected. Add a guard test that a blocked key produces the `env_var_ignored` WARN, not the shadow WARN.

7. **Doc sync.** `docs/logging.md`: add the `env_var_ignored` row AND the missing `env_shadows_singleton_config` row (pre-existing gap found in research — it is not catalogued today) with the narrowed post-J-2 semantics; keep `tests/web_singleton_config.rs:715` (`d1_audit_event_names_documented_in_logging_md`) green — add the new event names to its list. `docs/configuration.md`: rewrite the sections that CONTRADICT J-2 — §"Environment Variable Overrides" `:495–510` ("Override **any** config value…" with now-blocked examples `SERVER_ADDRESS` `:501`, `POLLING_FREQUENCY` `:507`), the precedence bullet `:37–38`, the `stream_all_devices` env-override cell `:102`, and §"env-only knobs" (heading `:523`, table `:527–531`) — document the full allowlist table + the ignored-class behaviour. `.env.example:17–21` ("ANY configuration key can be overridden") must be rewritten likewise. `.env.example`: annotate each `OPCGW_*` line as allowlisted or ignored-after-v2.8.0. README Planning row + config section; CHANGELOG under `[Unreleased]` **with the BREAKING migration note** (AC#9). Manual (`docs/manual/latex/body.tex` — LaTeX is canonical, never the retired DocBook XML): update the configuration chapter's env-var section.

8. **Config-spine regression gates.** Full `cargo test` 0-fail + `cargo clippy --all-targets -- -D warnings` clean. The figment stack is the boot spine: additionally run the **AI-G-5 real-binary smoke** (boot from a scratch dir with a mix of allowlisted + blocked + short-form env vars; verify boot completes, the blocked var is ignored with exactly one WARN, the allowlisted var takes effect, `/setup`-era web boot still works) before review→done.

9. **CHANGELOG migration note (BREAKING).** Must name: (a) the exact ignored-var list for a stock deployment, (b) the panoramix-class `stream_all_devices` trap — "if a value you rely on is env-only today, save it via the web Admin page BEFORE upgrading; the Admin GET backfill (#155) shows current effective values, a section save persists them", (c) that `env_shadows_singleton_config` WARNs in v2.7.1 logs are the definitive pre-upgrade checklist of affected vars.

10. **Tests (mandatory minimum).**
    (a) blocked key + SQLite row → SQLite wins (via real `from_path_with_sqlite`, not a hand-built stack);
    (b) blocked key, no SQLite row → TOML wins; no TOML → default;
    (c) allowlisted key → env wins over SQLite (re-spec'd t03);
    (d) secret env vars still reach the config (wizard/unattended path);
    (e) `env_var_ignored` emitted once per boot per key, absent for allowlisted/short-form/unknown-section keys — use the PR #170 test pattern (`temp_env::with_vars` + `#[serial_test::serial]`, `src/config.rs:5666` shape); do NOT touch the process-global budget atomics (`src/utils.rs:1046` local-atomic pattern; `budget_defaults_are_nas_realistic` at `utils.rs:1037` must stay green);
    (f) `peek_logging_config` consistency smoke — NOT a guard (validation-verified: `LoggingPeek` deserializes only `logging.*`, serde ignores unknown root keys, and `extract().ok()` swallows errors, so an unfiltered peek cannot misbehave; the shared builder at `main.rs:111` is for single-source-of-truth hygiene): `OPCGW_LOGGING__DIR` still reaches the peek with a blocked opcua key present;
    (g) shadow-WARN narrowing per AC#6;
    (h) **fake-regression-guard check**: (a) must FAIL when the filter is removed — verify by mutation (rip the filter out of the shared provider, run, confirm red, restore). Choose env keys/values so blocked-path and allowed-path outputs are non-overlapping (project finding-class: fake regression guards).

11. **No new config knob for the allowlist itself.** The allowlist is a compile-time constant next to `KNOWN_SECTIONS`/`SECRET_FIELDS_BY_SECTION` (`src/storage/migrate_singleton_config.rs`) or in config.rs — NOT configurable at runtime (an env var to configure which env vars count would be absurd). `Refs #172`: config.rs is 5741 lines; if this story pushes it materially past that, extract `mod tests` to a sibling `src/config_tests.rs` (the `storage/sqlite_tests.rs` precedent) — do NOT start the loader/validate split here (that is #172's own scope).

## Tasks / Subtasks

- [ ] **Task 1 — Allowlist constant + predicate** (AC#1, #11): `ENV_ALLOWLISTED_FIELDS: &[(&str, &[&str])]` (section → fields: `web`→`enabled,bind_address,port`; `opcua`→`host_port`) beside `SECRET_FIELDS_BY_SECTION`; predicate `env_key_allowed(section, key) -> bool` = secret ∨ allowlisted ∨ section ∉ KNOWN_SECTIONS.
- [ ] **Task 2 — Shared filtered provider** (AC#1, #3, #4): `opcgw_env_provider()` = **`Env::prefixed("OPCGW_").filter(..).split("__").global()` — the filter MUST come before `.split`** (validation-verified against figment 0.10.19 source: `filter`/`map` chain in call order and `.split` rewrites `__`→`.`, so a filter placed after `.split` receives dotted keys, `split_once("__")` never matches, and the allowlist silently enforces NOTHING). Filter-before-split sees the raw post-prefix key with `__` intact and case preserved — lowercase + `split_once("__")` inside the closure, mirroring `src/config.rs:1370–1374`; keys without `__` pass (they can't address a section field; the short forms `LOG_DIR`/`LOG_LEVEL` are also read directly outside figment anyway). `.global()` is orthogonal (profile-only) and composes fine. Swap in at `config.rs:1253` + `main.rs:111` + the five in-file test stacks.
- [ ] **Task 3 — `maybe_warn_env_ignored`** (AC#5): new fn shaped like `maybe_warn_env_shadows_singleton` (scan `std::env::vars()`, prefix-strip, split, KNOWN_SECTIONS gate, then `!env_key_allowed` ⇒ collect); called from `main.rs` near `:1088` with its own `AtomicBool`; ALSO call it on the bootstrap path (before SQLite — the WARN must fire even when SQLite never becomes readable), guarded by the same static.
- [ ] **Task 4 — Narrow the shadow WARN** (AC#6): add `env_key_allowed` gate to `maybe_warn_env_shadows_singleton` (skip blocked keys); update its 4 tests.
- [ ] **Task 5 — Tests** (AC#2, #10): re-spec t03 + t03b in `tests/d2_figment_provider.rs`; new in-file tests per AC#10; mutation-verify (h).
- [ ] **Task 6 — Docs** (AC#7, #9): logging.md (2 rows), configuration.md (see AC#7 rewrite list), `.env.example` (incl. the `:32–38` user_name promise + `:17–21` any-key claim), README, CHANGELOG (+migration note led by SERVER_ADDRESS/USER_NAME/STREAM_ALL_DEVICES), manual body.tex. Also sweep the stale in-code "Override via env-var:" doc comments on now-blocked fields (validation grep list: config.rs :155,:167,:180,:197,:299,:328,:340,:356,:371,:384,:404,:423,:437,:845; utils.rs :141,:159,:177,:198,:241,:303).
- [ ] **Task 7 — Gates + smoke** (AC#8): cargo test, clippy, AI-G-5 real-binary smoke with mixed env vars.

## Dev Notes

### Anchors (verified 2026-07-25 against `story/j-2-env-allowlist-enforcement` @ ed5d07b)

- Stack builder: `AppConfig::from_path_inner` `src/config.rs:1160`; env merge `:1253`; precedence doc `:1226–1244`; entry points `from_path` `:1137` / `from_path_with_sqlite` `:1153`.
- Second env merge: `peek_logging_config` `src/main.rs:108–114` (merge at `:111`).
- Shadow WARN: `maybe_warn_env_shadows_singleton` `src/config.rs:1349` (scan shape `:1365–1388`); call site `src/main.rs:1088`; guard static `:612`.
- `KNOWN_SECTIONS` `src/storage/migrate_singleton_config.rs:86` (v010 CHECK pins the same list); `SECRET_FIELDS_BY_SECTION` `:76`; `secret_fields_for_section` `:89`.
- Admin PUT: `src/web/singleton_config.rs:167` (section gate `:185`, secret gate `:224–226`, staged apply `:359`); whole-section DELETE+INSERT `src/storage/sqlite.rs:3473–3520`.
- Reload spine: `reload_effective_config` `src/main.rs:305` (Apply arm `:1691`); D-2 reload `:1039`; bootstrap load `:733`. Patching `from_path_inner` covers all three.
- Direct env reads that BYPASS figment (do not touch): `main.rs:126,184,631`; `config.rs:1435,1498,1570,2071`; `utils.rs:536,562,625`.
- `figment 0.10.19` (`Cargo.toml:15`); `Env::filter` exists in 0.10 and receives the post-prefix `UncasedStr` key. If `.filter` composes awkwardly with `.split("__")` ordering, apply the filter BEFORE `.split` (the filter key then still contains `__`) — verify empirically; the AC only pins observable behaviour.
- Test patterns: `temp_env::with_vars` + `#[serial_test::serial]` (`src/config.rs:5666–5741`); d2 integration harness `tests/d2_figment_provider.rs:70–90`; the `utils.rs:1046` local-atomic env-resolver pattern; `tracing_test` exact-pin `=0.2.6` (#101) — prefer the J-0 test-local subscriber pattern for WARN-capture if `traced_test` global-buffer bleed appears.

### Anti-patterns / disasters to prevent

- **Filter in the wrong layer:** post-`extract()` correction (re-setting fields after the fact) would leave `validate()`/`is_first_run` seeing filtered values inconsistently — filter at the provider.
- **WARN from the filter closure:** figment may call the provider multiple times per extract and there are ≥3 loads per boot (peek, bootstrap, reload) — WARN volume must come from the single scan fn (AC#5), or the WARN budget (#144/#149 discipline) is violated.
- **Killing the wizard:** blocking the secret vars or `web.*` bootstrap trio breaks `/setup` on a fresh install — the AI-G-5 #146 incident class ("cross-module configured-ness gap"). The smoke (Task 7) must include a fresh-boot path.
- **Substring matching on env names:** use exact section/key equality after the same lowercase+split as the existing scanner (project finding-class).
- **`stream_all_devices` silent flip:** the migration note is a HARD requirement (AC#9); on the NAS the env var is currently the ONLY source of `true`.

### Promotion Gate (config-spine story)

`review → done` requires: review loop terminated (LOW-only), gates clean, AND the local AI-G-5 real-binary smoke of Task 7. The panoramix soak rides the **v2.8.0-rc1** release (owner rule 2026-07-25: NAS runs Docker Hub images only) together with J-1's dispatcher soak; the rc's upgrade notes carry the AC#9 migration steps.

### References

- CR #169 (enforce), #168 (problem statement), PR #170 (shadow WARN foundation), #155 (Admin backfill), #172 (config.rs split — adjacent, NOT this story), #148/#163 (why `host_port` stays env-addressable: compose mapping).
- Epic J: `_bmad-output/implementation-artifacts/sprint-status.yaml:246–251`.
- Live-deployment input: session capture 2026-07-25 (panoramix `.env` + `singleton_config` dump + compose tag `2.7.1-rc4`).

## Dev Agent Record

### Agent Model Used

_(fill at dev time)_

### Debug Log References

### Completion Notes List

### File List

_Expected:_ `src/config.rs` (provider builder, predicate, ignored-WARN fn, shadow-WARN narrowing, tests), `src/storage/migrate_singleton_config.rs` (allowlist constant), `src/main.rs` (peek swap, ignored-WARN call sites), `tests/d2_figment_provider.rs` (t03 re-spec + t03b), `docs/logging.md`, `docs/configuration.md`, `.env.example`, `README.md`, `CHANGELOG.md`, `docs/manual/latex/body.tex`, `_bmad-output/implementation-artifacts/sprint-status.yaml`. Possibly `src/config_tests.rs` (AC#11 extraction) and `tests/web_singleton_config.rs` (event-name list).

### Change Log

| Date | Change |
|------|--------|
| 2026-07-25 | Story created via bmad-create-story (autonomous session; allowlist designed from the live panoramix `.env` per owner decision). |

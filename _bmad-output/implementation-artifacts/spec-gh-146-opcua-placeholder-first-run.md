---
title: 'Fix #146 — shipped placeholder opcua.user_password blocks first-run wizard'
type: 'bugfix'
created: '2026-06-29'
baseline_commit: '7f49faae2b7be4898e5ed2b4b300fcf0d7e1c3c6'
status: 'done'
context: []
---

<frozen-after-approval reason="human-owned intent — do not modify unless human renegotiates">

## Intent

**Problem:** A fresh clone following the documented quickstart cannot reach the `/setup` wizard. The shipped `config/config.toml` ships `[opcua].user_password` as a `REPLACE_ME_WITH_*` placeholder; `AppConfig::validate()` carves out the first-run signal only for an *empty* password, so a *placeholder* falls into the `contains(PLACEHOLDER_PREFIX)` reject branch and aborts boot before the web server starts. This is asymmetric with the ChirpStack token, where `chirpstack_token_missing()` already treats empty **or** placeholder as the first-run signal.

**Approach:** In `validate()`, gate the OPC UA `user_password` placeholder rejection on `!cs_first_run`, so a placeholder password is accepted while the gateway is in first-run mode (CS token still absent) and rejected once the gateway is configured. No change to `is_first_run()` is needed — for a non-empty placeholder password it already reduces to `chirpstack_token_missing()`.

## Boundaries & Constraints

**Always:**
- Keep the placeholder-rejection security guard active when the gateway is NOT in first-run mode (a real CS token via env-var or `secrets.toml`, with a leftover placeholder OPC UA password, must still error).
- Use `contains(PLACEHOLDER_PREFIX)` (not `starts_with`) consistency with the singleton-migration Guard 3, unchanged.
- The as-shipped `config/config.toml` (both secrets as `REPLACE_ME_WITH_*`, no env-vars) must pass `validate()` and boot into first-run mode.

**Ask First:**
- Any change that would accept a placeholder OPC UA password once a real CS token is present (the user explicitly wants that case rejected).

**Never:**
- Do not change `is_first_run()` semantics, the empty-password carve-out, or the ChirpStack-side validation.
- Do not touch the web layer, the wizard handler, or `config/config.toml`'s shipped values.
- Do not weaken the env-var-set-but-empty rejection.

## I/O & Edge-Case Matrix

| Scenario | Input / State | Expected Behavior | Error Handling |
|----------|--------------|-------------------|----------------|
| Fresh clone (as-shipped) | CS token placeholder, opcua pw placeholder, no env-vars | `validate()` → `Ok`; `is_first_run()` → true | N/A |
| Empty pw (existing C-0 path) | opcua pw empty, no env-var | `validate()` → `Ok` (unchanged) | N/A |
| Configured CS, leftover placeholder pw | real CS token (env/secrets), opcua pw placeholder | `validate()` → `Err` (placeholder detected) | error pushed |
| Env-var set-but-empty pw | `OPCGW_OPCUA__USER_PASSWORD=""` | `validate()` → `Err` (unchanged) | error pushed |

</frozen-after-approval>

## Code Map

- `src/config.rs` -- `AppConfig::validate()` opcua `user_password` placeholder branch (~line 1940); `cs_first_run` local already computed at ~1406; `chirpstack_token_missing()` (~1288) and `is_first_run()` (~1329) are the symmetric references.
- `config/config.toml` -- shipped seed carrying both `REPLACE_ME_WITH_*` placeholders (read-only reference for the regression test).
- `docs/quickstart.md` -- documents the promised "shipped placeholder config boots the wizard" behavior (L53-77, L161); the assertion this fix restores.
- `src/opc_ua_auth.rs` -- (iter-1 review) `OpcgwAuthManager::new` decides `is_configured` (the reject-all-auth-in-first-run gate); must treat a placeholder password as NOT configured, symmetric with `validate()`, or the public placeholder string becomes a live credential.
- `docs/security.md` -- (iter-1 review) placeholder-detection section + env-var table row; doc-sync to reflect placeholder-accepted-in-first-run.

## Tasks & Acceptance

**Execution:**
- [x] `src/config.rs` -- wrap the opcua `user_password` placeholder `errors.push(...)` (the `contains(PLACEHOLDER_PREFIX)` branch) in `if !cs_first_run { ... }`, with a doc comment explaining the first-run carve-out is symmetric with the CS-token branch and that the guard still fires once configured.
- [x] `src/config.rs` -- add unit tests in the existing `#[cfg(test)]` module: (a) as-shipped placeholders → `validate()` Ok + `is_first_run()` true; (b) real CS token (env-var) + placeholder opcua pw → `validate()` Err; (c) empty-pw path still Ok (regression guard). Use a unique env-var guard pattern consistent with existing first-run tests.
- [x] `src/opc_ua_auth.rs` -- (iter-1 review patch) `is_configured` must be false when the password is empty OR a placeholder, so reject-all auth stays in force in first-run + the public placeholder string can never authenticate. Add tests: placeholder → `is_configured` false; presenting the placeholder string → auth rejected.
- [x] `docs/security.md` -- (iter-1 review patch) doc-sync the placeholder-detection section + env-var table row to the new first-run behavior.

**Acceptance Criteria:**
- Given the as-shipped `config/config.toml` (both `REPLACE_ME_WITH_*`, no env-vars), when `validate()` runs, then it returns `Ok` and `is_first_run()` is true.
- Given a real `OPCGW_CHIRPSTACK__API_TOKEN` and a placeholder `[opcua].user_password`, when `validate()` runs, then it returns `Err` naming the opcua placeholder.
- Given an empty `[opcua].user_password` and no env-var, when `validate()` runs, then it returns `Ok` (no regression).
- Given a placeholder `[opcua].user_password` in first-run, when `OpcgwAuthManager::new` builds, then `is_configured` is false and the placeholder string never authenticates (reject-all).
- `cargo test` and `cargo clippy --all-targets -- -D warnings` are clean.

## Spec Change Log

- **iter-1 (2026-06-29)** — Triggering findings: adversarial review HIGH (Blind Hunter + Edge Case Hunter, converged) — the config-only fix let a non-empty placeholder password through `validate()` in first-run, but `OpcgwAuthManager::new` (`src/opc_ua_auth.rs:122`) keyed its reject-all `is_configured` gate on `user_password.is_empty()`, so the well-known placeholder string would become a **live OPC UA credential**, contradicting `main.rs:379`. Plus Acceptance Auditor MEDIUM doc-sync (`docs/security.md` claimed the placeholder still aborts). Amended (non-frozen sections): added `src/opc_ua_auth.rs` + `docs/security.md` to Code Map and Tasks; added the auth-gate AC. Known-bad state avoided: a public credential silently accepted during first-run. KEEP: the `src/config.rs` carve-out gated on `!cs_first_run` is correct and must survive — only the auth gate + docs were missing. The `cs_first_run`-vs-`is_first_run()` equivalence (Blind MEDIUM) was verified by the Edge Hunter and is documented in-code; left as-is.

## Verification

**Commands:**
- `cargo test --lib config` -- expected: new + existing config tests pass.
- `cargo test` -- expected: full suite 0 failures.
- `cargo clippy --all-targets -- -D warnings` -- expected: clean.
- Manual: run the binary with the git-tracked `config/config.toml` (placeholder pw) and `OPCGW_WEB__ENABLED=true`; expected: `/setup` serves 200, `/` → 303 redirect, no validation abort.

## Suggested Review Order

**The first-run carve-out (entry point)**

- Entry point: a placeholder OPC UA password is accepted only while in first-run (CS token absent), symmetric with the CS-token branch.
  [`config.rs:1964`](../../src/config.rs#L1964)

**The security invariant (cross-module — the iter-1 HIGH)**

- The reject-all gate: a placeholder password must NOT count as configured, or the public literal becomes a live credential.
  [`opc_ua_auth.rs:136`](../../src/opc_ua_auth.rs#L136)

**Tests**

- As-shipped placeholders → validate() Ok + first-run true (the regression this fixes).
  [`config.rs:3175`](../../src/config.rs#L3175)

- Real CS token + placeholder pw → validate() Err (the security guard, not-first-run).
  [`config.rs:3207`](../../src/config.rs#L3207)

- Existing test inverted: as-shipped fixture now loads + is first-run (previously asserted abort).
  [`config.rs:4800`](../../src/config.rs#L4800)

- Placeholder pw → is_configured false; with positive control proving discrimination.
  [`opc_ua_auth.rs:538`](../../src/opc_ua_auth.rs#L538)

**Docs**

- Doc-sync: placeholder-detection section now describes first-run boot vs. past-first-run abort.
  [`security.md:137`](../../docs/security.md#L137)

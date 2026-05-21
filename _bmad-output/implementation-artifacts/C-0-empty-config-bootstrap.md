# Story C-0: Empty-Config Bootstrap + First-Run Setup Wizard

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `C-0-empty-config-bootstrap`                                                                                |
| Epic            | C — Auto-Discovery and Web-First Configuration (post-v2.0 GA)                                               |
| FRs             | none (Epic C is post-PRD; see memory `project_epic_c_auto_discovery_vision.md`)                             |
| Status          | ready-for-dev                                                                                               |
| Created         | 2026-05-21                                                                                                  |
| Source epic     | `_bmad-output/planning-artifacts/epics.md § Epic C § Story C.0`                                             |
| Vision capture  | memory `project_epic_c_auto_discovery_vision.md` (2026-05-20 Guy, scope finalised 2026-05-21)               |
| Partially in    | commits `cecd100` (serde-default on `application_list`) + validator allow-empty branch + regression test    |
| Tracking        | GitHub issue `#__` — user opens out-of-band; capture number in Dev Notes once known                         |

---

## User Story

As an **opcgw operator deploying a fresh container with no pre-existing `config.toml`** (or with a near-empty bootstrap one),
I want the gateway to start successfully with zero `[[application]]` entries and to present a first-run web wizard for the OPC UA user password,
So that I can configure opcgw entirely through the web UI without writing TOML by hand and without learning the `OPCGW_OPCUA__USER_PASSWORD` env-var override convention before I can reach the dashboard.

---

## Story Context

### Why C-0 is the prerequisite for Epic C

Every downstream Epic C story (C-1 inventory API, C-2 pickers UI, C-3 dup-prevention, C-4 drift view, C-6 TOML→SQLite migration) assumes the operator can reach the web UI **before** configuring any application. Today, an operator running `docker pull && docker run` against a fresh image with no `config.toml` mounted hits a startup error and the gateway exits before the web port binds. C-0 closes that gap.

The flow C-0 lands looks like this from the operator's seat:

```
docker run -p 4855:4855 -p 8080:8080 gcorbaz/opcgw:latest
  → gateway starts, OPC UA binds (empty browse tree), web server binds
  → operator opens http://localhost:8080
  → first-run wizard prompts for OPC UA user password
  → operator submits, dashboard renders (empty applications tile)
  → operator clicks "Add application" (handled by C-2 in a later story)
```

Without C-0, none of the above is possible without first writing a `config.toml` by hand — which is precisely the friction Epic C exists to remove.

### What is already landed (pre-C-0 baseline)

The 2026-05-20 v2.0 GA walkthrough surfaced a partial fix for the empty-application path. Two commits got most of the way there:

- **`cecd100`** (`fix(config): #[serde(default)] on application_list so empty TOML deserializes`) — adds the missing serde default so an empty `config.toml` (or one without an `[[application]]` block) deserialises into `AppConfig` without "missing field `application`" errors.
- **Validator allow-empty branch** at `src/config.rs:1529-1538` — explicitly allows `application_list.len() == 0` as a valid state. The previous `"application_list: at least one application must be configured"` error has been removed; the `for app in self.application_list.iter()` loops in `validate()` simply iterate zero times.
- **Regression test** `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list` (line 276) spawns the binary with zero `[[application]]` blocks, asserts the OPC UA port binds within 15 s, and panics with an explicit "Epic D D-0 invariant has regressed" message if not (the comment references "Epic D" because it was written before the 2026-05-21 D→C rename — it's the same invariant).

**What's still missing** is the `user_password` side of the story. Today `src/config.rs:1461` rejects startup if `self.opcua.user_password.is_empty()`:

```rust
if self.opcua.user_password.is_empty() {
    errors.push("opcua.user_password: must not be empty".to_string());
}
```

An operator with a fresh `config.toml` who hasn't set `OPCGW_OPCUA__USER_PASSWORD` env-var still gets a hard failure here, well before the web wizard could even render. C-0 unblocks this — either by relaxing the validator under a "first-run mode" flag, or by deferring the check until after the web server has had a chance to render the wizard.

### Target post-C-0 shape

After C-0:

1. `validate()` accepts an empty `user_password` IF `OPCGW_OPCUA__USER_PASSWORD` is also unset (the "first-run mode" signal: no source has provided a password yet).
2. The gateway boots fully — OPC UA server binds on its endpoint (initially with authentication disabled OR with a documented placeholder behaviour — see AC#6), ChirpStack poller spawns and no-ops, web server binds.
3. The web server detects "no password set" and serves a `/setup` wizard at any path the operator visits (the dashboard `/`, `/applications`, etc. all redirect to `/setup` until the wizard is completed).
4. On successful wizard submit, the password is persisted to a new `config/secrets.toml` (operator-readable but kept out of the main `config.toml` for git-tracking hygiene; see Dev Notes for the rationale). The gateway hot-reloads its OPC UA auth manager so the new password takes effect immediately without a restart.
5. The dashboard, applications, devices, and metrics pages render empty states gracefully when their respective collections are empty.

### Out-of-scope sister behaviours (claimed by sibling stories)

- C-1 inventory API endpoints — not yet exposed; "Add application" button on the empty dashboard is a no-op stub until C-1 + C-2 land.
- C-2 inventory pickers — out of scope.
- C-3 duplicate validation — out of scope.
- C-4 drift view — out of scope.
- C-6 TOML→SQLite migration — out of scope. C-0's wizard persists to TOML (or a sibling `secrets.toml`); C-6 later migrates that to SQLite.
- A separate "change password" admin UI for already-configured gateways — out of scope. C-0 only covers the first-run case where no password is set yet. Operators who want to rotate an existing password use env-vars or hand-edit `secrets.toml`.

---

## Acceptance Criteria

### Empty-config startup invariant

1. **`AppConfig::validate()` accepts both empty `application_list` AND empty `user_password`** when no env-var override is present. The existing allow-empty-list branch is kept; a new allow-empty-password branch is added with the gating:

   ```rust
   if self.opcua.user_password.is_empty() {
       // Allowed IF and only if OPCGW_OPCUA__USER_PASSWORD is also unset.
       // Signals "first-run mode" — web wizard will collect the password.
       if std::env::var("OPCGW_OPCUA__USER_PASSWORD").is_ok() {
           errors.push("opcua.user_password: env-var set but resolved to empty — \
                        the env-var likely contains whitespace or a placeholder".to_string());
       }
       // else: leave it empty; main() will detect the first-run state.
   } else if self.opcua.user_password.starts_with(PLACEHOLDER_PREFIX) {
       // placeholder rejection — unchanged from today
   } else if self.opcua.user_password.trim() != self.opcua.user_password {
       // whitespace rejection — unchanged from today
   }
   ```

   The PLACEHOLDER_PREFIX and whitespace checks continue to fire for non-empty passwords (operator typed garbage). Only the "empty + no env-var" case is newly accepted.

2. **First-run state is observable.** A new method `AppConfig::is_first_run(&self) -> bool` returns `true` IFF `self.opcua.user_password.is_empty()` AND no `OPCGW_OPCUA__USER_PASSWORD` env-var is set AND no `config/secrets.toml` file exists with a `[opcua].user_password` field. The method is called by both `main.rs` (to decide whether to serve the wizard) and the web request middleware (to gate every request).

3. **Regression-guard: empty `application_list` invariant still holds.** `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list` continues to pass unchanged. The test's "Epic D D-0" comment is updated to "Epic C C-0" in the same touch (string update only; behavioural assertion unchanged).

### First-run web wizard

4. **`GET /setup` renders the wizard page** when the gateway is in first-run mode. The page is a new `static/setup.html` containing:
   - A clear explanation: "opcgw needs an OPC UA user password before it can accept SCADA-client connections. Set it below."
   - A `<form method="POST" action="/api/setup/password">` with two `<input type="password">` fields (password + confirmation) plus a CSRF token (rendered server-side per the existing CSRF discipline from Story 7-2 / Story 9-x).
   - Client-side and server-side password validation rules (see AC#7).
   - A submit button labelled "Save and continue".
   - No global navigation (the operator must complete this step before they can reach `/applications`, `/devices`, etc.).

5. **Every other route redirects to `/setup` while in first-run mode.** A new `first_run_redirect` middleware in `src/web/` checks `AppConfig::is_first_run()` on each request. If `true` AND the path is not `/setup` AND the path is not `/api/setup/*` AND the path is not a static asset under `/static/`, the middleware emits `HTTP 302 Location: /setup` (or `303 See Other` — Dev Agent picks the more semantically appropriate code per RFC 7231).

6. **OPC UA server behaviour during first-run.** The OPC UA server binds on its configured endpoint even during first-run mode — this is what allows external uptime probes to confirm "the gateway is running" before the wizard is completed. The server's authentication policy during first-run is one of (Dev Agent picks the better-fitting option and documents the choice in Dev Notes):
   - **(a)** Bind in anonymous-only mode (no username/password auth offered) until the wizard completes, then hot-swap to the standard username+password policy.
   - **(b)** Bind with username+password policy but with all auth attempts rejecting (no valid credentials exist yet) — the wizard is the only way to register a credential.
   - Either way: the existing audit-event `event="opcua_session_pki_violation"` / `event="opcua_auth_failed"` continues to fire on rejection attempts; a new `event="opcua_first_run_mode"` audit event fires once at startup naming the chosen behaviour for forensic clarity.

### Password validation + persistence

7. **`POST /api/setup/password` enforces the same password validation rules as today's `OPCGW_OPCUA__USER_PASSWORD` env-var path.** Specifically:
   - Minimum length: 12 characters (or whatever the current floor is in `src/opc_ua_auth.rs` — verify against the existing implementation; do NOT introduce a new floor).
   - No leading/trailing whitespace (matches today's validator rejection at `src/config.rs:1474`).
   - Not a placeholder string (no `PLACEHOLDER_PREFIX`).
   - Confirmation field matches the password field.
   - Validation rules are documented in `static/setup.html` AND in the server-side handler (server is authoritative; the client-side rendering is a UX nicety, not the gate).
   - Rejection emits HTTP 400 + a JSON error payload `{ "error": "password_validation_failed", "reason": "<specific reason>" }` AND audit event `event="setup_password_rejected" reason="<specific reason>"`.

8. **Successful POST persists the password to `config/secrets.toml`.** A new file is created (or appended to, if it already exists) at `<config_dir>/secrets.toml` (`config_dir` derived the same way as the main `config.toml` path — the `-c` CLI arg's parent directory, or the default per `src/main.rs`). The file's content is a minimal TOML document:

   ```toml
   # opcgw secrets — generated by first-run wizard on <timestamp>.
   # This file holds the OPC UA user password and any other future
   # secrets that should NOT live in the operator-readable config.toml.
   # File permissions: chmod 0600 (the gateway will reject the file if
   # group/world has any access bits set).

   [opcua]
   user_password = "<password>"
   ```

   The file is written with mode `0o600` (owner read+write only). The parent directory must already be writable by the gateway process; if not, the wizard returns HTTP 500 with `event="setup_password_persistence_failed" reason="parent_dir_not_writable"`.

9. **`AppConfig` loading merges `config/secrets.toml` on top of `config.toml`.** The figment-based config loader in `src/config.rs` gets a new file-provider for `secrets.toml` (similar to how the env-var provider stacks on top of the file provider today). The precedence order, top to bottom, is:
   - `OPCGW_*` env-vars (highest precedence; unchanged from today)
   - `config/secrets.toml` (new; provides `[opcua].user_password` post-wizard)
   - `config/config.toml` (lowest precedence; unchanged from today)

   If `config/secrets.toml` does not exist, the loader silently skips it (no warning, no error — the absence is the first-run signal).

10. **`config/secrets.toml` is gitignored.** Append `config/secrets.toml` to `.gitignore`. The file is operator-local and machine-specific; it must never be committed.

11. **OPC UA auth manager hot-reloads after wizard submit.** The existing reload primitive (Story 9-7's `src/web/hot_reload.rs` or equivalent) is triggered after the wizard POST completes successfully. The OPC UA server picks up the newly-set password without a process restart. The same `event="config_reload"` audit event family that Story 9-7 introduced fires; a new sub-field `trigger="first_run_wizard"` distinguishes this from operator-triggered reloads.

12. **First-run state transitions from `true` to `false` on successful wizard submit.** The subsequent request (e.g., the redirect target after the POST) sees `AppConfig::is_first_run() == false` and renders the dashboard normally. The wizard route `/setup` continues to be reachable post-first-run for a documented "change password" flow in a future story — for C-0 it just renders an "already configured — use the change-password CLI for password rotation" message and returns HTTP 410 Gone.

### Empty-state UI for already-configured-but-empty gateways

13. **Dashboard renders graceful empty-state copy** when `application_list.len() == 0` AND first-run is done. The existing `static/index.html` "Applications" tile (today shows a count) renders the count as `0` with subtext "No applications configured — click below to add one" and a CTA button linking to `/applications`. No JS errors, no blank `<tbody>`, no `Cannot read property 'length' of undefined` console errors.

14. **`/applications` page renders an empty list with an "Add application" button** in the empty-list case. The existing `static/applications.html` is updated; the empty state shows operator-meaningful copy ("No applications configured. ChirpStack inventory is available — click 'Add application' to pick one.") and the "Add application" button is prominent. (The button itself remains a stub until C-2 lands; clicking it today renders today's free-form application-id input.)

15. **`/devices` and `/metrics` pages render gracefully when their respective collections are empty** — no JS errors, no rendering crashes. Copy can be terse ("No devices configured" / "No metrics configured").

### Compatibility + regression invariants

16. **Existing env-var path is preserved unchanged.** If `OPCGW_OPCUA__USER_PASSWORD` is set at startup, `is_first_run()` returns `false`, the wizard is suppressed entirely (operator never sees `/setup`), and the dashboard renders directly. This is the path used by today's `docker-compose.yml` + `.env` file deployment pattern; it MUST NOT regress.

17. **Existing populated `config.toml` path is preserved unchanged.** If `[opcua].user_password` is set to a real (non-empty, non-placeholder) value in `config.toml`, the wizard is suppressed regardless of whether `secrets.toml` exists. The wizard only fires when ALL three sources (env-var, secrets.toml, config.toml) are empty/absent.

18. **Existing audit event surface is preserved.** No existing `event=...` is renamed, removed, or has its field set narrowed. New audit events introduced by C-0: `event="opcua_first_run_mode"` (fires once at boot when in first-run), `event="setup_password_accepted"` / `event="setup_password_rejected"` (wizard POST outcomes), `event="config_reload" trigger="first_run_wizard"` (post-wizard hot-reload). All fields use the kebab-case convention per existing audit-event discipline.

19. **`cargo test --all-targets` passes** with the new test count baseline preserved. Pre-C-0 baseline at `1b65f10` (verified 2026-05-21 via fresh `cargo test --all-targets --no-fail-fast`): **1260 / 0 / 10** across 27 test binaries. Post-C-0 target: ≥ 1269 / 0 / ≥ 10 — the +9 expected from new integration tests covering (a) first-run detection, (b) wizard GET renders, (c) wizard POST validates rules, (d) wizard POST persists to secrets.toml, (e) wizard POST triggers hot-reload, (f) env-var path bypasses wizard, (g) populated-config path bypasses wizard, (h) empty-state dashboard rendering, (i) secrets.toml chmod-0600 verification. Document the actual count delta in Dev Notes completion notes.

20. **`cargo clippy --all-targets -- -D warnings` clean.** No new warnings.

21. **`cargo test --doc` no regressions.** ≥ 56 ignored (per issue #100), 0 failed.

### Documentation sync

22. **README.md documents the empty-bootstrap path** in the Docker section. A new sub-section ("First-run wizard") under the existing "Docker" section explains: (a) the operator can `docker run` with no `config.toml` mount, (b) opcgw will serve a wizard at port 8080 on first reach, (c) after the wizard, `config/secrets.toml` will be created automatically and persists across container restarts via the bind-mount, (d) the wizard is one-shot — once a password is set, subsequent `/setup` requests return HTTP 410.

23. **README.md Planning table gains an Epic C row** matching the canonical pattern (status, scope, link to memory vision). The Current Version block is updated to mention "Epic C 1/6 done" once C-0 lands.

24. **`docs/security.md` is updated** with a new "First-run wizard" section explaining: (a) the `config/secrets.toml` file location and permissions (0600), (b) why secrets are kept out of the main `config.toml`, (c) the precedence order (env-var > secrets.toml > config.toml), (d) the operator's options for password rotation post-wizard (env-var override always wins; or hand-edit secrets.toml; a CLI-driven rotation flow is a future story).

25. **DocBook user manual `docs/manual/opcgw-user-manual.xml`** gains a new `<sect1>` titled "First-run wizard" under the existing "Configuration" chapter (added in B-1). The section mirrors the README content but in DocBook 4.5 syntax per memory `[[project_user_manual_format]]` — DocBook 4.5 XML, NOT LaTeX/Markdown/AsciiDoc.

### Strict-zero invariants

26. **NO changes to** `src/chirpstack.rs` (the poller no-ops on empty list as already verified), `src/opc_ua_history.rs`, `src/storage/sqlite.rs`, `migrations/*.sql`, `Cargo.toml`, `Cargo.lock`. Mutable scope is:
   - `src/config.rs` (validator amend + figment provider stack + `is_first_run()` method)
   - `src/main.rs` (first-run mode detection at boot + log emit)
   - `src/web/` (new wizard routes + middleware + audit events) — likely a new `src/web/setup.rs` module
   - `src/opc_ua_auth.rs` (hot-reload trigger after wizard submit)
   - `src/opc_ua.rs` (first-run anonymous-or-reject behaviour per AC#6)
   - `static/setup.html` (new), `static/index.html`, `static/applications.html`, `static/devices.html`, `static/metrics.html` (empty-state copy)
   - `tests/web_setup_wizard.rs` (new integration test file)
   - `tests/main_startup_no_deadlock.rs` (comment string update only — "Epic D" → "Epic C")
   - `.gitignore` (append `config/secrets.toml`)
   - `README.md`, `docs/security.md`, `docs/manual/opcgw-user-manual.xml` (documentation sync per CLAUDE.md rule)
   - `_bmad-output/implementation-artifacts/sprint-status.yaml` (story status flips)
   - This story spec file

### GitHub tracking issue

27. GitHub tracking issue (suggested title: "C-0: Empty-config bootstrap + first-run setup wizard") opened by user out-of-band. Issue number captured in Dev Notes once known; referenced in every implementation commit via `Refs #N`.

---

## Tasks / Subtasks

- [ ] **Task 0 — Tracking issue acknowledgment (AC: #27)**
  - [ ] 0.1 User opens GitHub issue with the suggested title.
  - [ ] 0.2 Capture the issue number in Dev Notes "Tracking issue" field.
  - [ ] 0.3 Reference `Refs #N` in every commit produced by this story.

- [ ] **Task 1 — Loosen `user_password` validation + add `is_first_run()` (AC: #1, #2, #3)**
  - [ ] 1.1 Amend `AppConfig::validate()` at `src/config.rs:1461` per AC#1 — accept empty `user_password` IFF env-var is also unset.
  - [ ] 1.2 Add `impl AppConfig { pub fn is_first_run(&self) -> bool { ... } }` method per AC#2.
  - [ ] 1.3 Add a `#[test]` in `src/config.rs::tests` covering: (a) empty password + no env-var → `is_first_run()` true; (b) empty password + env-var set → validate() returns Err (env-var-was-empty rejection); (c) non-empty password → `is_first_run()` false regardless of env-var; (d) `secrets.toml` present with password → `is_first_run()` false.
  - [ ] 1.4 Update `tests/main_startup_no_deadlock.rs` comment block: "Epic D D-0" → "Epic C C-0" (string-only touch; assertion unchanged).

- [ ] **Task 2 — Figment provider stack for `config/secrets.toml` (AC: #9)**
  - [ ] 2.1 Inspect the existing figment provider stack in `src/config.rs` (look for `Figment::new()` + `.merge(...)` calls).
  - [ ] 2.2 Insert a new `Toml::file("config/secrets.toml").nested()` provider BETWEEN the main TOML provider and the env-var provider (so env-var wins, secrets.toml wins over main config.toml).
  - [ ] 2.3 Verify the provider silently skips when the file is absent (figment's default behaviour; no opt-in needed).
  - [ ] 2.4 Add a `#[test]` covering: (a) main config.toml password "A" + secrets.toml password "B" → loaded value is "B"; (b) main config.toml password "A" + secrets.toml absent → loaded value is "A"; (c) main config.toml empty + secrets.toml password "B" → loaded value is "B"; (d) env-var "C" + secrets.toml "B" + main "A" → loaded value is "C".

- [ ] **Task 3 — First-run middleware + wizard routes (AC: #4, #5)**
  - [ ] 3.1 Create `src/web/setup.rs` containing the wizard route handlers.
  - [ ] 3.2 Add `GET /setup` handler that renders `static/setup.html` server-side with the CSRF token injected (follow Story 9-x CSRF discipline).
  - [ ] 3.3 Add `POST /api/setup/password` handler (see Task 4 for body).
  - [ ] 3.4 Add `first_run_redirect_middleware` that checks `AppConfig::is_first_run()` and redirects non-`/setup`, non-`/api/setup/*`, non-`/static/*` requests to `/setup` per AC#5.
  - [ ] 3.5 Wire the middleware into the main web router in `src/web/mod.rs` (or wherever the router is constructed).
  - [ ] 3.6 Integration test in `tests/web_setup_wizard.rs`: (a) GET `/` while in first-run mode returns 302 → `/setup`; (b) GET `/setup` while in first-run mode returns 200 + the wizard HTML; (c) GET `/static/style.css` while in first-run mode is NOT redirected (static assets serve normally).

- [ ] **Task 4 — Wizard POST handler: validate + persist + reload (AC: #7, #8, #11)**
  - [ ] 4.1 Validation logic in `POST /api/setup/password`: (a) minimum length per existing `src/opc_ua_auth.rs` floor; (b) no leading/trailing whitespace; (c) no PLACEHOLDER_PREFIX; (d) confirmation matches password.
  - [ ] 4.2 Validation failure emits HTTP 400 + JSON `{ "error": "password_validation_failed", "reason": "..." }` + audit event `event="setup_password_rejected"`.
  - [ ] 4.3 On success, write `config/secrets.toml` per AC#8 (TOML stub with `[opcua].user_password`, file mode 0600).
  - [ ] 4.4 Trigger the existing reload primitive (Story 9-7 hot-reload) with `trigger="first_run_wizard"` audit field per AC#11.
  - [ ] 4.5 Return HTTP 302 redirect to `/` + audit event `event="setup_password_accepted"`.
  - [ ] 4.6 Integration tests covering: (a) valid password persisted + reload fires + redirect to /; (b) short password rejected; (c) whitespace-bracketing rejected; (d) placeholder-prefix rejected; (e) confirmation mismatch rejected; (f) `secrets.toml` file mode = 0600 post-write.

- [ ] **Task 5 — OPC UA first-run mode (AC: #6, #18)**
  - [ ] 5.1 Decide between AC#6 option (a) anonymous-only or (b) reject-all. Recommend option (b) — never serve anonymous OPC UA sessions, even briefly; safer for SCADA-adjacent contexts.
  - [ ] 5.2 Add `event="opcua_first_run_mode"` audit emit at OPC UA server boot when in first-run.
  - [ ] 5.3 Verify post-wizard hot-reload swaps the auth manager to use the persisted password (Task 4.4 reload primitive).
  - [ ] 5.4 Document the choice in Dev Notes.

- [ ] **Task 6 — Wizard frontend (AC: #4)**
  - [ ] 6.1 Create `static/setup.html` — minimal markup matching the existing visual language (reuse `static/dashboard.css` classes where they fit; create new classes scoped to `.setup-wizard` if needed).
  - [ ] 6.2 Client-side password validation mirrors the server rules per AC#7 (length, whitespace, placeholder-prefix, confirmation match). Server is authoritative — client-side is UX only.
  - [ ] 6.3 The form posts to `/api/setup/password` with the CSRF token; on success the server redirects to `/`.
  - [ ] 6.4 No global navigation (no sidebar links) — the operator must complete the wizard.

- [ ] **Task 7 — Empty-state UI for already-configured-but-empty gateways (AC: #13, #14, #15)**
  - [ ] 7.1 `static/index.html` — replace any "0 applications" blank rendering with a deliberate empty-state block per AC#13.
  - [ ] 7.2 `static/applications.html` — empty-state copy + prominent "Add application" button per AC#14.
  - [ ] 7.3 `static/devices.html` + `static/metrics.html` — graceful empty-state per AC#15.
  - [ ] 7.4 Verify no JS errors in the browser console on a fresh `application_list = []` gateway (manual smoke test; document in Dev Notes).

- [ ] **Task 8 — Gitignore + secrets.toml persistence (AC: #10)**
  - [ ] 8.1 Append `config/secrets.toml` to `.gitignore`.
  - [ ] 8.2 Verify `git status` after a `git stash` + restart-with-wizard-flow does not stage `secrets.toml`.

- [ ] **Task 9 — Documentation sync (AC: #22, #23, #24, #25)**
  - [ ] 9.1 README.md — new "First-run wizard" sub-section under "Docker"; Planning table Epic C row.
  - [ ] 9.2 `docs/security.md` — new "First-run wizard" section per AC#24.
  - [ ] 9.3 `docs/manual/opcgw-user-manual.xml` — new `<sect1>` under Configuration chapter per AC#25 (DocBook 4.5 syntax; do NOT migrate format).
  - [ ] 9.4 Verify `xmllint --noout --valid docs/manual/opcgw-user-manual.xml` exits 0 post-edit (DocBook DTD validation).

- [ ] **Task 10 — Regression gate + commit (AC: #19, #20, #21, #26)**
  - [ ] 10.1 `cargo test --all-targets` → record actual pass/fail/ignored count; target ≥ 1265 / 0 / ≥ 10.
  - [ ] 10.2 `cargo clippy --all-targets -- -D warnings` → must exit clean.
  - [ ] 10.3 `cargo test --doc` → ≥ 56 ignored, 0 failed.
  - [ ] 10.4 Verify no changes to forbidden files per AC#26 strict-zero list.
  - [ ] 10.5 Manual smoke test: spin up a fresh container with no `config.toml`, walk through the wizard, verify the password persists across a `docker restart`, verify env-var override still works.
  - [ ] 10.6 Commit message: `Story C-0: Empty-config bootstrap + first-run setup wizard - Implementation Complete` + `Refs #<issue>`.

---

## Dev Notes

### Why persist to `config/secrets.toml` and not `config/config.toml`

The main `config.toml` is operator-readable (often `cat`'d during troubleshooting), version-controlled in some deployments, and visible to anyone with read access to the config directory. Storing the password there violates the same separation-of-secrets principle that drove Story 7-1 (credential management via environment variables): structural config lives in TOML, secrets live in env-vars (or, post-C-0, in a chmod-0600 `secrets.toml` sibling).

The sibling-file pattern is preferable to writing to the main config.toml for three reasons:

1. **Operator-readable separation.** `cat config/config.toml` does not leak the password. `cat config/secrets.toml` requires elevated permissions if the operator follows the 0600 convention.
2. **Git-tracking hygiene.** `config/config.toml` is sometimes committed (with placeholder secrets); `config/secrets.toml` is always gitignored. The split is unambiguous.
3. **Future-proofing for additional secrets.** When Epic C or later epics introduce additional secrets (ChirpStack API token rotation UI, web UI admin credentials), they all land in `secrets.toml` without further reorganisation.

The Dev Agent should NOT use the existing `toml_edit` byte-preserving primitive (Story 9-5) to inject into `config.toml`. `secrets.toml` is a fresh file owned entirely by C-0 + future-Epic-C stories.

### Why OPC UA option (b) over (a) in AC#6

C-0 lands the wizard in a SCADA-adjacent context. Even a brief anonymous-OPC-UA window is a security regression that an audit would flag. Option (b) — bind with username+password policy but reject all auth attempts during first-run — keeps the security posture intact at the cost of audit-log noise (clients trying to connect during the first-run window will see `opcua_auth_failed`). The audit-log noise is a feature: it lets an operator who left first-run unfinished see "yep, my SCADA clients are trying to talk to me but the wizard isn't done yet."

The Dev Agent may revisit this in implementation if option (b) requires invasive changes to `async-opcua`'s auth flow that option (a) would avoid. Document the rationale either way.

### How the hot-reload triggers without a process restart

Story 9-7 introduced a config-reload primitive that already rebuilds the in-memory `AppConfig` snapshot and notifies subscribers (OPC UA address space, ChirpStack poller, web UI state). C-0's wizard reuses this exact primitive. After `secrets.toml` is written:

1. The wizard handler calls the reload primitive's "reload now" entry point.
2. The reload primitive re-runs the figment provider stack — this time `secrets.toml` exists, so its `user_password` is loaded.
3. The OPC UA auth manager re-initialises with the new password.
4. The wizard handler returns HTTP 302 → `/`.
5. The browser follows the redirect; `is_first_run()` now returns `false`; the dashboard renders.

The Dev Agent must verify that the OPC UA auth manager's re-init does NOT drop existing sessions (today there should be zero sessions during first-run, but be defensive: existing sessions during reload should remain authenticated under the OLD credentials until they reconnect, at which point they re-auth with the new credentials). This is the same contract Story 9-7 already documented.

### Why `tests/main_startup_no_deadlock.rs` only needs a comment change

The test exists and passes today — the empty-application-list invariant is already in place (`cecd100` + the validator allow-empty branch). C-0 doesn't change the runtime behaviour the test asserts; it only adds first-run wizard behaviour on top. The test's docstring references "Epic D D-0" because it was written 2026-05-20 under the working name; the rename to Epic C / C-0 happened 2026-05-21. String-only update; assertion unchanged.

### Test-count delta — why +9 not +N

The +9 in AC#19 reflects the minimum sensible coverage on top of the 1260/0/10 baseline verified 2026-05-21:

1. `is_first_run()` true case (empty pwd + no env-var)
2. `is_first_run()` false case (env-var set)
3. `is_first_run()` false case (`secrets.toml` present)
4. validator rejects empty-password + env-var-also-empty edge case
5. figment provider stack precedence (4-way: env > secrets > config > default)
6. wizard GET renders in first-run
7. wizard POST validates + persists + reloads + redirects
8. wizard POST rejects with each of the 4 invalid-input patterns (length, whitespace, placeholder, confirmation-mismatch) — could be one parameterised test or four; counted as one
9. `secrets.toml` file mode == 0600 post-write

The Dev Agent may split some of these further (e.g., one test per redirect target in #6); the +9 floor is a sanity check, not a cap.

### Carry-forward GitHub issues

#88 (per-IP rate limiting — wizard POST is a logical place to add it but out of C-0 scope), #100 (56 doctest ignores — preserve baseline), #102 (tests/common reuse — Dev Agent may extract a `wizard_test_harness()` helper if useful), #104 (TLS hardening), #108 (closed by Epic A), #110 (RunHandles Drop), #117 (perf-CI lane).

---

## Out of Scope

- **Change-password flow for already-configured gateways** — C-0 only covers the first-run case. A future story (call it C-0a or land it as part of a Web UI Settings epic) will add a `/settings/change-password` flow.
- **Multi-user web UI authentication** — out of scope. The OPC UA `user_password` is the OPC UA endpoint's credential, not a web-UI credential. Web-UI auth (Story 8-1 basic auth) is unchanged.
- **Wizard for ChirpStack credentials** — `[chirpstack].api_token` is still env-var-only at C-0. A future story may add an inventory-prerequisite wizard that prompts for `api_token` if neither env-var nor `secrets.toml` provides one. Out of C-0 scope.
- **`config/secrets.toml` schema beyond `[opcua].user_password`** — C-0 lands only that one field. Future stories add more (ChirpStack token, web admin password, etc.) under their respective stories.
- **CLI password-rotation tool** — out of scope. Operators rotate via env-var override or hand-edit `secrets.toml`.
- **Inventory pickers, drift view, dup-prevention, TOML→SQLite migration** — claimed by C-1/C-2/C-3/C-4/C-6 respectively.

---

## Completion Note

To be filled in by the dev agent at story completion. Should include: actual test count delta, the AC#6 OPC UA first-run mode decision (a or b) with rationale, the GitHub issue number, any deferred follow-ups added to `deferred-work.md`.

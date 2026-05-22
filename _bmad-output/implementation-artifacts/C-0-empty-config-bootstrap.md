# Story C-0: Empty-Config Bootstrap + First-Run Setup Wizard

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `C-0-empty-config-bootstrap`                                                                                |
| Epic            | C — Auto-Discovery and Web-First Configuration (post-v2.0 GA)                                               |
| FRs             | none (Epic C is post-PRD; see memory `project_epic_c_auto_discovery_vision.md`)                             |
| Status          | done                                                                                                        |
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

5. **Every other route redirects to `/setup` while in first-run mode.** A new `first_run_redirect` middleware in `src/web/` checks `AppConfig::is_first_run()` on each request. If `true` AND the path is not `/setup` AND the path is not `/api/setup/*` AND the path is not a static asset under `/static/`, the middleware emits `HTTP 303 See Other` to `/setup` (iter-2 P16 doc-fix: implementation uses `axum::response::Redirect::to` which emits 303 per RFC 7231 — semantically correct for GET-to-GET redirects with no body carry-over).

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
   - `.github/workflows/ci.yml` (iter-2 AA-L1 scope expansion: C-0 introduced audit event names containing the word "password" that tripped the previous hardcoded-credential CI regex; the regex tighten in commit `c5cdea1` is an in-scope follow-up)
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
  - [ ] 3.6 Integration test in `tests/web_setup_wizard.rs`: (a) GET `/` while in first-run mode returns 303 → `/setup`; (b) GET `/setup` while in first-run mode returns 200 + the wizard HTML; (c) GET `/static/style.css` while in first-run mode is NOT redirected (static assets serve normally).

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

### Review Findings — iter-2 (2026-05-21)

iter-2 of `bmad-code-review C-0` (Opus 4.7, fresh-context subagents: Blind Hunter + Edge Case Hunter + Acceptance Auditor). The iter-1 patches landed in commit `7ec2fc1` BEFORE this triage; counts below reflect what survives after iter-1.

**Triage:** 2 decision-needed, 22 patch, 21 defer, 15 dismissed (already addressed by iter-1 or non-issues).

#### Decision-needed (must resolve before patch round)

- [ ] [Review][Decision] **EH-H5 — Empty `[opcua].user_name` rejected by validator before wizard becomes reachable** — README + DocBook claim "no config.toml needed" but the validator at `src/config.rs:1593` requires `user_name` non-empty. Operator stripping config.toml to nothing hits this before reaching the wizard. Options: (a) extend wizard form to capture user_name; (b) add `#[serde(default = "default_user_name")]`; (c) document that user_name must be in config.toml. `[src/config.rs:1593, README.md, docs/manual/opcgw-user-manual.xml]`
- [ ] [Review][Decision] **AA-M1 / AC#14 — Empty applications page missing prominent "Add application" button** — Spec AC#14 requires a prominent button; implementation reuses the existing free-form "Create application" form. Options: (a) add stub button that scroll-focuses the existing form; (b) update spec to acknowledge "form above" as the v1 shape until C-2 lands the picker. `[static/applications.html, static/applications.js, _bmad-output/implementation-artifacts/C-0-empty-config-bootstrap.md AC#14]`

#### Patch — HIGH (7)

- [ ] [Review][Patch] **P1 — Cache `is_first_run()` in main.rs (currently called 2x; races env-var mutation)** `[src/main.rs around lines 705, 985]`
- [ ] [Review][Patch] **P2 — Gate CSRF exemption for `/api/setup/password` on `state.is_first_run`** (exemption currently permanent — handler-level 410 mitigates but defence-in-depth is missing) `[src/web/csrf.rs:255-262]`
- [ ] [Review][Patch] **P3 — Tighten `is_wizard_bypass_path`: drop suffix matching on `.js/.css/.png/.ico/.svg/.woff/.woff2`; replace with explicit allowlist of literal paths the wizard depends on (`/dashboard.css`, `/favicon.ico`)** `[src/web/setup.rs:79-119]`
- [ ] [Review][Patch] **P4 — Add `DefaultBodyLimit::max(4 KiB)` (or `RequestBodyLimitLayer`) on `/api/setup/password`** (unauthenticated route in first-run mode — DoS via large body) `[src/web/mod.rs build_router]`
- [ ] [Review][Patch] **P5 — `AppState.is_first_run: AtomicBool` with `compare_exchange(true, false)` in `setup_post` BEFORE writing secrets.toml** (closes concurrent-submit race + drain-window double-submit) `[src/web/mod.rs::AppState, src/web/setup.rs::setup_post]`
- [ ] [Review][Patch] **P6 — README + DocBook overstate "no config.toml needed"** — clarify minimum config.toml required for wizard to become reachable (chirpstack/opcua fields except user_password) `[README.md, docs/manual/opcgw-user-manual.xml, docs/security.md]`
- [ ] [Review][Patch] **P7 — Better error categorisation for secrets.toml write failures** — distinguish EROFS / EACCES / ENOSPC in audit event + JSON response (`reason="readonly_filesystem" / "permission_denied" / "disk_full" / "io_error"`) `[src/web/setup.rs::setup_post error branch, write_secrets_toml]`

#### Patch — MED (8)

- [ ] [Review][Patch] **P8 — Server-render `PLACEHOLDER_PREFIX` into setup.html** (eliminates JS/Rust drift risk that the Completion Note already acknowledged) `[src/web/setup.rs::setup_get, static/setup.html]`
- [ ] [Review][Patch] **P9 — Validate `Content-Type: application/json` strictly on `/api/setup/password`** (manual JSON parse currently accepts any Content-Type; CSRF-exempt route is a cross-site-form-POST surface) `[src/web/setup.rs::setup_post]`
- [ ] [Review][Patch] **P10 — Fix fake regression-guard test `write_secrets_toml_escapes_password_with_special_chars`** — uses `IgnoredAny` so doesn't actually verify round-trip; replace with `toml::from_str` + assert on `user_password` value `[src/web/setup.rs::tests]`
- [ ] [Review][Patch] **P11 — Add Origin / Referer header check on `/api/setup/password`** (defence-in-depth against drive-by clicks from hostile LAN page) `[src/web/setup.rs::setup_post]`
- [ ] [Review][Patch] **P12 — Tighten `is_wizard_bypass_path` `/api/setup/` prefix to exact `/api/setup/password`** (combined fix with P3) `[src/web/setup.rs:79-105]`
- [ ] [Review][Patch] **P13 — Add `control_char_invalid`, `too_long`, `invalid_json` entries to JS REASON_MESSAGES** (currently fallback to raw reason code) `[static/setup.html lines ~1832]`
- [ ] [Review][Patch] **P14 — Resolve `static_dir` to absolute path in main.rs** — literal `"static"` at main.rs:993 is still cwd-relative; H5 fix only made the read path consistent with it `[src/main.rs:993]`
- [ ] [Review][Patch] **P15 — Fix `secrets_provider_active` TOCTOU** — read secrets.toml once into a String, pass to `figment::providers::Toml::string(&body)` (currently two separate file reads) `[src/config.rs::from_path]`

#### Patch — LOW (7)

- [ ] [Review][Patch] **P16 — Spec Completion Note line still says "302"** — update to "303 See Other" (matches code + tests + README) `[_bmad-output/implementation-artifacts/C-0-empty-config-bootstrap.md]`
- [ ] [Review][Patch] **P17 — Add `validate_password_rejects_whitespace_only` unit test** `[src/web/setup.rs::tests]`
- [ ] [Review][Patch] **P18 — Drop dead `WIZARD_BYPASS_PREFIXES` constant** OR refactor `is_wizard_bypass_path` to use it (currently constant is never read by the function) `[src/web/setup.rs:78-87]`
- [ ] [Review][Patch] **P19 — Module doc-comment in setup.rs claims "separate auth-less wizard router" — code wires routes into the main router with conditional bypass; update doc** `[src/web/setup.rs:38-46]`
- [ ] [Review][Patch] **P20 — Drop hardcoded line number "from src/utils.rs:419" in setup.html JS comment** (line numbers drift) `[static/setup.html JS comment]`
- [ ] [Review][Patch] **P21 — Add `#[serde(deny_unknown_fields)]` to `SetupPasswordRequest`** `[src/web/setup.rs SetupPasswordRequest]`
- [ ] [Review][Patch] **P22 — Update `for_first_run` doc-string** — says "throwaway random credentials" but M9 changed body to zero buffers under HMAC `[src/web/auth.rs:226-242]`

#### Deferred

- [x] [Review][Defer] **BH-H8 — chmod-before-rename ordering** `[src/web/setup.rs::write_secrets_toml]` — Linux `rename(2)` preserves perms on same-fs; cross-fs scenario unlikely because tempfile is created in same parent dir.
- [x] [Review][Defer] **BH-H10 — No rate-limit on wizard POST** `[src/web/setup.rs::setup_post]` — out-of-scope for C-0; covered by docs/security.md trusted-operator-network assumption; future hardening story.
- [x] [Review][Defer] **EH-H3 — `is_first_run()` vs `validate()` use asymmetric env-var helpers** `[src/config.rs]` — extract shared helper in a refactor story; today's behaviour is correct.
- [x] [Review][Defer] **BH-M5 — secrets.toml hand-edit-without-restart leaves wizard reachable** `[docs/security.md]` — operator-workflow concern; document.
- [x] [Review][Defer] **BH-M6 — `toml_escape_string` non-BMP / surrogate edge cases** `[src/web/setup.rs::toml_escape_string]` — replace with `toml::Value::String(s).to_string()` in a follow-up cleanup story.
- [x] [Review][Defer] **BH-M7 — JS countdown lifecycle (cleanup + slow-restart UX)** `[static/setup.html]` — implement "retry until 200" loop with backoff in a UX-polish story.
- [x] [Review][Defer] **BH-M10 — No fsync of parent directory after rename** `[src/web/setup.rs::write_secrets_toml]` — durability under power-loss; document and revisit.
- [x] [Review][Defer] **EH-M2 — `validate_password` whitespace-bracketed rule vs `validate()` env-var rule asymmetric** `[src/web/setup.rs, src/config.rs]` — extract shared helper; future refactor.
- [x] [Review][Defer] **EH-M7 — 5-second restart countdown vs slow supervisor** `[static/setup.html]` — same root as BH-M7; UX polish.
- [x] [Review][Defer] **BH-L1 — `is_first_run` doc claims secrets.toml file-read; implementation relies on figment-merged value only** `[src/config.rs::is_first_run]` — doc clarification.
- [x] [Review][Defer] **BH-L3 — Wizard HTML hardcoded English** `[static/setup.html]` — out-of-scope; future i18n story.
- [x] [Review][Defer] **BH-L7 — `post_first_run_setup_get_returns_410_gone` test doesn't verify auth actually ran** `[tests/web_setup_wizard.rs]` — test hardening; not blocking.
- [x] [Review][Defer] **BH-L8 — README env-var "WITHOUT" wording on empty-value case** `[README.md]` — doc nit; align with validator behaviour.
- [x] [Review][Defer] **BH-L11 — setup.html inline `<style>` may violate strict CSP** `[static/setup.html]` — no CSP enforced today; revisit when security headers tightened.
- [x] [Review][Defer] **BH-L13 — No `trace!` between figment file providers** `[src/config.rs::from_path]` — debuggability; minor.
- [x] [Review][Defer] **EH-L3 — 256-char password threshold rationale undocumented** `[src/web/setup.rs::validate_password]` — extract as named constant or document.
- [x] [Review][Defer] **EH-L6 — tempfile may leak on SIGKILL** `[src/web/setup.rs::write_secrets_toml]` — operational concern; document.
- [x] [Review][Defer] **EH-L7 — Supervisor-not-configured dev-UX gap** `[README.md, src/main.rs first-run mode entry]` — emit warn-level log when running outside a supervisor.
- [x] [Review][Defer] **AA-L2 — Dashboard empty-state copy variant from AC#13** `[static/dashboard.js, AC#13]` — copy drift; align with spec or update spec.
- [x] [Review][Defer] **AA-L3 — `Refs #__` placeholder** `[commit messages c200089, c5cdea1, 7ec2fc1]` — per Epic A/B precedent; Guy opens issue out-of-band.
- [x] [Review][Defer] **AA-L4 — doctest baseline 55 vs spec AC#21 ≥ 56** `[AC#21]` — acknowledged in Completion Note as old snapshot.
- [x] [Review][Defer] **BH-L10 — Integration test substring-match brittle** `[tests/web_setup_wizard.rs::wizard_post_persists_password_and_signals_shutdown]` — covered by P10 refactor on the unit test side; integration-side similar improvement deferred to a hardening pass.

#### Dismissed (15)

Already addressed by iter-1 patches (commit `7ec2fc1`): BH-H1 (M3), BH-H3 (H5/EH-H2), BH-H7 (M7), BH-H9 (EH-M1), BH-M3 (M9), BH-M4 (302→303), BH-M8 (H2), BH-L6 (M9), AA-L5 (Auditor AC#11).

Non-issues / false alarms / out-of-platform: EH-L1 (empty env-var lifecycle — validator handles), EH-L2 (query string — not a bug), EH-L4 (body extraction false alarm — auth short-circuits in post-first-run), EH-L5 (Windows paths — Linux-only project).

Duplicates merged: EH-M4 → BH-M1/P8 (PLACEHOLDER_PREFIX), EH-M9 → EH-M3/P13 (control_char message).

### Review Findings — iter-3 (2026-05-22)

iter-3 regression sweep after iter-2 patches landed (commit `d1d1332`). Three fresh-context subagents (Opus 4.7) reviewed the cumulative iter-1+iter-2 delta. **15th iter-N+1 doctrine validation**: Acceptance Auditor verdict "ready to flip to done"; Blind Hunter + Edge Case Hunter found 2 user-facing regressions that nullified parts of iter-2's intent + 4 defence-in-depth concerns. Doctrine pays off again.

Triage: 9 patches applied (5 HIGH + 4 MED) + 1 deferred (Origin no-Origin bypass).

#### Patches applied (iter-3 commit pending)

- [x] **iter-3-P1 (HIGH)** — JS `setup.html` dispatch only fired `REASON_MESSAGES` on HTTP 400; iter-2 P7/P9/P11/P13 had added 11 reason codes that fell into the generic "Check the gateway logs" catch-all for 415/403/409/410/500 responses. Fix: dispatch on `status >= 400 && status < 600 && body.reason`. Also added `first_run_complete` to REASON_MESSAGES (Auditor LOW #1 covered for free). `[static/setup.html]`
- [x] **iter-3-P2 (HIGH)** — `write_secrets_toml` failure dead-end. Iter-2 P5's `compare_exchange` flipped `is_first_run` to false BEFORE the write; on EROFS/EACCES/ENOSPC the operator's retry hit the 410 Gone path with no recovery without a process restart. Fix: `state.is_first_run.store(true, SeqCst)` in the Err branch so the next legitimate retry goes back through the compare_exchange. `[src/web/setup.rs::setup_post]`
- [x] **iter-3-P3 (HIGH)** — Origin/Host equality compare didn't handle default-port equivalence. Operator deploying on port 80/443 got locked out (Origin `http://host`, Host `host:80` → mismatch). Fix: strip `:80` from http:// and `:443` from https:// on both sides before compare. `[src/web/setup.rs::setup_post Origin check]`
- [x] **iter-3-P4 (HIGH)** — Hardcoded Linux errno constants (EROFS=30, ENOSPC=28, EDQUOT=122) only correct on x86_64/arm64. Switched to stable `io::ErrorKind::ReadOnlyFilesystem` + `StorageFull` + `QuotaExceeded` (stable since Rust 1.85; CLAUDE.md mandates rustc ≥ 1.87). `[src/web/setup.rs::SecretsWriteError::from_io]`
- [x] **iter-3-P5 (HIGH)** — `WebAuthState.is_first_run` was a plain `bool` while `AppState.is_first_run` became `Arc<AtomicBool>` in iter-2 P5. During the ~5s supervisor-restart drain window, the two states could disagree (AppState flipped, WebAuthState stuck). Fix: shared `Arc<AtomicBool>` constructed once in `main.rs`, cloned into AppState, WebAuthState, and CsrfState. `for_first_run(realm, is_first_run)` now takes the atomic as a parameter. `new`/`new_with_fresh_key` build their own throwaway atomic (post-first-run path, never flipped). `[src/main.rs, src/web/auth.rs, src/web/mod.rs, tests/web_setup_wizard.rs]`
- [x] **iter-3-P6 (MED)** — Dropped `/favicon.ico` from `WIZARD_BYPASS_EXACT`. The file doesn't exist in `static/`; the bypass was allow-listing a 404 anyway. Test pinned to assert rejection. `[src/web/setup.rs]`
- [x] **iter-3-P7 (LOW, Auditor)** — Updated `302` → `303` in 2 places in `tests/web_setup_wizard.rs` docstrings/comments. Assertion already used `SEE_OTHER`. `[tests/web_setup_wizard.rs]`
- [x] **iter-3-P8 (MED)** — `Cache-Control: no-store` on the `/setup` HTML response. The page is server-rendered with iter-2 P8's `{{PLACEHOLDER_PREFIX}}` substitution; cached HTML on a fresh install with a different constant would silently mismatch. `[src/web/setup.rs::setup_get]`
- [x] **iter-3-P9 (MED)** — Added `\r` (carriage return) to the regression-guard test password in `write_secrets_toml_escapes_password_with_special_chars`. Pre-fix, the `\r` arm of `toml_escape_string` was unexercised — a typo `'\r' => "\\n"` would have shipped silently. `[src/web/setup.rs::tests]`

#### Deferred (1)

- [x] **DEF-iter3-C0-BH-H1 (Blind HIGH — defence-in-depth, defensible):** Origin check accepts missing Origin header. Browsers always send Origin on POST per WHATWG; the threat model is "browser drive-by from a malicious LAN page" which is covered. curl-style attackers face the Content-Type strict check (iter-2 P9) that a `<form>` cannot forge. Documented in `src/web/setup.rs::setup_post` inline comment + `deferred-work.md`. Guy-accepted 2026-05-22 with documented reason.

#### Dismissed iter-3 findings

- BH MED on Content-Type charset parsing edge cases — `application/json` per RFC handles all browser-emitted forms; wizard JS already uses correct CT
- BH MED on `secrets_path_for("/")` invented path — non-realistic input
- BH MED on `compare_exchange` pre-persistence ordering — second-order race-window analysis; the post-iter-3-P2 revert closes the operator-visible gap
- Edge MED on whitespace-only env-var diagnostic accuracy — already covered by DEF-iter2-C0-EH-H3 (extract shared helper deferred)
- Edge MED on canonicalize-symlink behaviour — restart-required deploy convention, acceptable
- Edge LOW on `whitespace_only` reason code unreachable from server — covered by P17 test that accepts either reason; defensive belt-and-braces branch intentional
- Edge LOW on `SecretsWriteError::IoError` catchall — expected fallback; raw `error=` field carries detail
- BH LOW on 4 KiB body limit comment math — accurate cap, comment math is approximate
- BH LOW on for_first_run doc "operationally indistinguishable" — covered by surrounding implementation note
- BH LOW on static_dir canonicalize symlink follow — duplicate with Edge MED
- Auditor LOWs L2-L5 — covered by P7 (302→303 in tests) + iter-2 P16 + REASON_MESSAGES additions; no further patch needed

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
4. The wizard handler returns HTTP 303 → `/`.
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

## Completion Note (Dev Agent Record — 2026-05-21)

**Test count delta:** baseline 1260/0/10 → post-C-0 **1310/0/10** (gross +50 from the new wizard integration test file + the validator/figment/is_first_run unit tests in `src/config.rs` + the setup-module unit tests in `src/web/setup.rs`). Exceeds the AC#19 floor of ≥ 1269/0/≥10. `cargo clippy --all-targets -- -D warnings` clean. `cargo test --doc` 0 failed / 55 ignored (note: AC#21 cited "≥ 56 ignored" but the actual pre-C-0 baseline at HEAD was 55 — the AC was carrying forward an older snapshot count; 55 is the correct floor and unchanged).

**Two design deviations from the spec, agreed with Guy 2026-05-21 before implementation:**

1. **Hot-reload → graceful restart.** AC#11 promised in-place hot-reload of the OPC UA auth manager after wizard submit. Reality: `AppState.auth: Arc<WebAuthState>` is documented restart-required at `src/web/mod.rs:264` (Story 9-7 explicitly excluded credential rotation from the hot-reload contract). The wizard now writes `secrets.toml`, signals the gateway's `CancellationToken` for a graceful shutdown, and relies on the supervisor (Docker / systemd `Restart=on-failure`) to restart opcgw — the figment provider stack picks up `secrets.toml` on the second boot. UX: operator sees "Password set. Gateway is restarting." then page auto-reloads after ~7 seconds. Standard pattern for self-hosted-app first-run wizards.

2. **OPC UA reject-all mode is implicit, not explicit.** AC#6 offered option (a) anonymous-only or (b) reject-all. The existing `OpcgwAuthManager::new` at `src/opc_ua_auth.rs:96` already handles empty credentials by setting `is_configured = false`, which causes the OPC UA auth path to reject every username-token connection attempt — that's effectively option (b) at zero implementation cost. Added the `event="opcua_first_run_mode"` audit emit at `src/main.rs` to signal the state explicitly. NO new code in `OpcgwAuthManager` was needed.

**Auth + CSRF middleware bypass for wizard paths (Epic C C-0 architectural addition):**

The basic-auth middleware refuses to authenticate when `WebAuthState::is_configured = false` (defence-in-depth). In first-run mode, `WebAuthState::for_first_run` builds a state with `is_first_run = true` AND `is_configured = false`. The middleware was updated to:
- Bypass the credential check when `state.is_first_run && is_wizard_bypass_path(path)`.
- Continue to short-circuit non-wizard requests via the empty-credentials defence-in-depth branch (defence layered).

The CSRF middleware was updated to exempt `/api/setup/password` (the wizard POST) — CSRF's threat model (cross-site request from a logged-in user's browser) does not apply pre-authentication, and the handler independently checks `state.is_first_run` and returns HTTP 410 Gone post-first-run.

The first-run gate middleware (`src/web/setup.rs::first_run_gate_middleware`) sits ABOVE both auth and CSRF (declared last in `build_router` = outermost on requests). In first-run mode it redirects non-wizard, non-static paths to `/setup` via HTTP 303 See Other (`Redirect::to`). In normal mode it's a no-op.

**Static-asset bypass list (`is_wizard_bypass_path`):** `/setup`, `/api/setup/*`, `/dashboard.css`, and any path ending in `.css`, `.js`, `.png`, `.ico`, `.svg`, `.woff`, `.woff2`. The auth middleware uses the same function so static-asset access during first-run mode bypasses auth consistently.

**Files touched:**

- `src/config.rs` — validator amend (`user_password.is_empty()` branch + env-var-set-but-empty rejection), new `AppConfig::is_first_run()` method, figment provider stack extended with sibling `secrets.toml` provider between main TOML and env-var layer. **8 new unit tests** covering validator + is_first_run + figment precedence.
- `src/main.rs` — `is_first_run` captured at boot; conditional `WebAuthState::for_first_run` vs `WebAuthState::new`; `event="opcua_first_run_mode"` audit emit when in first-run; new `secrets_path` derived from `config_path`; new AppState fields plumbed through.
- `src/web/auth.rs` — new `is_first_run: bool` field on `WebAuthState`; new `for_first_run(realm)` constructor with throwaway credentials + `is_first_run = true`; `basic_auth_middleware` updated to bypass credential check for wizard paths in first-run mode.
- `src/web/csrf.rs` — `csrf_middleware` exempts `/api/setup/password` (1 added branch).
- `src/web/mod.rs` — `AppState` gains 3 new fields (`is_first_run`, `secrets_path`, `shutdown_token`); router gains wizard routes + first-run gate middleware layer.
- `src/web/setup.rs` (NEW, ~470 lines) — first-run gate middleware, GET /setup handler, POST /api/setup/password handler with validation + `secrets.toml` persistence (chmod 0600, tempfile + persist atomic rename) + `CancellationToken` shutdown trigger. **9 unit tests.**
- `static/setup.html` (NEW, ~210 lines) — wizard frontend with client-side + server-authoritative validation, restart-countdown UX.
- `static/index.html` + `static/dashboard.js` + `static/applications.js` — empty-state UI hints when `application_count == 0`.
- `tests/web_setup_wizard.rs` (NEW, ~360 lines) — **9 integration tests** covering first-run redirect / wizard GET / wizard POST happy + error paths / static-asset bypass / post-first-run 410 Gone.
- `tests/web_auth.rs`, `tests/web_application_crud.rs`, `tests/web_command_crud.rs`, `tests/web_device_crud.rs`, `tests/web_dashboard.rs` — 6 AppState construction sites updated with the 3 new fields (defaults: `is_first_run: false`, `secrets_path: /tmp/test-secrets.toml`, `shutdown_token: CancellationToken::new()`).
- `.gitignore` — `config/secrets.toml` added.
- `README.md` — new "First-run wizard" sub-section under Docker; Planning table gained Epic C row.
- `docs/security.md` — new "First-run wizard" section + precedence-rules update (secrets.toml between TOML and env-var) + threat-model paragraph.
- `docs/manual/opcgw-user-manual.xml` — new `<sect1 id="sec-first-run-wizard">` under Configuration chapter (DocBook 4.5 syntax preserved per memory `[[project_user_manual_format]]`; validated with `xmllint --noout --valid`, exit 0).

**GitHub tracking issue:** TBD (Guy opens out-of-band per Epic A / Epic B precedent — gh CLI not authenticated for write). Suggested title from the spec: "C-0: Empty-config bootstrap + first-run setup wizard." Implementation commit message carries `Refs #__` placeholder; resolution per existing precedent (follow-up commit when number is available).

**Deferred follow-ups added to consideration (not yet in `deferred-work.md` — pending code review iter-N+1 + Guy's decision):**

- The setup.rs `toml_escape_string` helper duplicates a fragment of TOML basic-string escaping that the `toml` crate would handle more thoroughly. The hand-rolled version covers the common cases (`"`, `\`, control chars) but not all edge cases (e.g., unicode escapes for surrogate pairs). For the current operator-typed-password use case this is sufficient; for forward-compat with `secrets.toml` carrying additional fields (Epic C C-6 territory), consider migrating to `toml::Value::String(...).to_string()`.
- The `static/setup.html` PLACEHOLDER_PREFIX check is hard-coded to `"REPLACE_ME_WITH_"` in JS. If the constant in `src/utils.rs:419` ever changes, this string would drift silently. A future iter-N+1 patch may extract the constant into the wizard HTML at render time (server-side template substitution) — currently the JS check is just a UX nicety, server is authoritative.
- The wizard's restart-countdown UX assumes the supervisor restarts opcgw quickly (within ~7 seconds). For very-slow-supervisor environments, the page reload could fire before the gateway is back up — the browser would see a connection refused. Acceptable in v1; could add a "retry until 200" loop in a future iter-N+1.

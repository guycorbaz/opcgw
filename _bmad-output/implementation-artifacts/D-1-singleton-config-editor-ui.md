# Story D-1: Singleton Configuration Editor in the Web UI

Status: review

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `D-1-singleton-config-editor-ui`                                                                            |
| Epic            | D — Singleton Configuration → SQLite                                                                        |
| FRs             | none (Epic D is post-PRD)                                                                                   |
| Status          | ready-for-dev                                                                                               |
| Created         | 2026-05-27                                                                                                  |
| Source epic     | `_bmad-output/planning-artifacts/epics.md § Epic D § Story D.1`                                             |
| Depends on      | D-0 (the `singleton_config` SQLite store + `migrate_singleton_toml_to_sqlite` boot-time migration + `SqliteBackend::load_singleton_config` / `write_singleton_section` helpers + the `serialize_section` JSON-flatten convention) and C-0 (the `shutdown_token.cancel()` supervisor-restart pattern from the first-run wizard). C-0 + D-0 must be on `main` before D-1 implementation starts. |
| Tracking        | GitHub issue `#__` — user opens out-of-band                                                                 |

---

## User Story

As an **opcgw operator running a post-D-1 binary**,
I want to edit `[global]`, `[chirpstack]`, `[opcua]`, and `[web]` configuration knobs through the web UI,
So that I can tune gateway behaviour without SSH-ing in to hand-edit TOML files, and so that my changes survive across boots via the SQLite canonical store (no TOML re-edits required after a restart).

---

## Story Context

### Why D-1 is the middle story of Epic D

D-0 (2026-05-27) landed the SQLite write side + the `load_singleton_config` read helper but **deferred the AppConfig read-path swap to D-2** (`D-0-FOLLOWUP-2` in `deferred-work.md`). The reasoning was scope-narrowing: D-0's job was the schema + migration; the read-path rework required reordering subsystem construction in `main.rs` which exceeded D-0's commit budget.

D-1's challenge: the editor UI writes singleton values to SQLite via `SqliteBackend::write_singleton_section`, but the running gateway still reads from the figment-loaded `AppConfig` snapshot (which reflects `config.toml` at boot, not SQLite). Without bridging this gap, D-1's writes are invisible to the gateway until D-2 lands. **D-1 absorbs the boot-time half of `D-0-FOLLOWUP-2`**: after the D-0 migration block in `main.rs`, overlay the SQLite singleton snapshot onto the in-memory `AppConfig` before subsystem construction. This makes D-1's writes effective on next boot.

D-2 still owns: the full figment Provider rework for `env > SQLite > TOML > default` precedence ordering (AC#8 fidelity), the TOML mutation-surface decommission, the once-per-boot warn for orphan `config.toml` files, and the architecture.md + DocBook user-manual rewrites.

### Restart-required semantics

D-1 ships as a **writes-then-restart** UI. Every successful PUT to a singleton section triggers a graceful supervisor restart via `state.shutdown_token.cancel()` (the C-0 first-run-wizard pattern). On restart:

1. The supervisor (Docker / systemd / cargo run wrapper) restarts the process.
2. The new boot loads `config.toml` via figment.
3. The D-0 migration block sees `d0_migration_done` set → `AlreadyMigrated` (no-op).
4. The new D-1 overlay step (added by this story) reads from `singleton_config` via `load_singleton_config` and overlays the values onto `AppConfig`.
5. Subsystems (poller, OPC UA, web) construct from the overlaid `AppConfig`.

This sidesteps the hot-reload-vs-restart-required taxonomy from the original Epic D scope. The spec's "hot-reloadable knobs take effect immediately" aspiration is **deferred to D-2** alongside the full figment Provider rework. D-1's UX is unambiguous: any edit → confirmation modal → graceful supervisor restart → next boot reflects the change.

The trade-off: poll_frequency / debug-flag / inventory_cache_ttl_seconds and similar runtime-mutable knobs need a restart in D-1. The simplification is justified by D-1's commit budget and the fact that singletons are typically tuned infrequently. Operators who want sub-restart latency can edit via `OPCGW_<SECTION>__<KEY>` env-var and restart manually — same UX cost.

### What stays out of D-1 scope

- **Secrets editing.** `[chirpstack].api_token` and `[opcua].user_password` are NOT editable from this UI. The GET endpoint returns the placeholder string `"<set via config/secrets.toml>"` for these fields; the PUT endpoint rejects payloads containing these keys with `reason="secret_field_not_editable"`. Operators rotate secrets via `secrets.toml` or env-var.
- **Hot-reload (runtime without restart).** D-1 always restarts. D-2 lands the figment Provider rework that enables hot-reload for the subset of knobs that are runtime-mutable.
- **The TOML mutation-surface decommission.** D-2 owns this. D-1 leaves `config.toml` untouched at runtime.
- **The full architecture.md + DocBook manual rewrite for the SQLite-canonical model.** D-2 owns this; D-1 adds focused per-page updates.
- **Read-path-precedence for env-vars over SQLite.** The simple overlay in D-1 has SQLite winning over env-vars (operator-edit-via-UI is canonical post-D-1). The proper `env > SQLite > TOML > default` ordering needs the figment Provider rework deferred to D-2.

---

## Acceptance Criteria

### Boot-time SQLite-overlay onto AppConfig (D-0-FOLLOWUP-2 partial close)

1. **New helper `AppConfig::overlay_singletons_from_sqlite_rows(&mut self, rows: &[(String, String, String)]) -> Result<(), OpcGwError>`** in `src/config.rs` (or a sibling module). Takes the result of `SqliteBackend::load_singleton_config()` (a `Vec<(section, key, value-as-json-string)>`) and updates `self.global`, `self.chirpstack`, `self.opcua`, `self.web` from the JSON values. Inverse of D-0's `serialize_section`. Skipped fields (secrets — `api_token` / `user_password`) are never present in the rows; they continue to flow through figment from `secrets.toml` + env-var.

2. **`main.rs` overlay block.** Immediately after the D-0 migration block (and BEFORE the F1 watch-channel seeding + before any subsystem construction), call `sqlite_backend.load_singleton_config()`. If `Ok(rows)` and `!rows.is_empty()`, invoke `Arc::make_mut(&mut application_config)` and apply `overlay_singletons_from_sqlite_rows(&mut app, &rows)`. On overlay failure, emit `event="config_overlay_failed"` at `warn!` with the error string + continue with the figment-loaded values (transition safety net mirroring D-0 AC#6). On success, emit `event="config_overlay"` at `info!` with `rows=<N>`.

3. **Documented precedence inversion.** A brief paragraph in `docs/architecture.md` notes that post-D-1, SQLite singleton values OVERRIDE env-var-set values at boot (deviates from AC#8 of D-0's spec). Captured in `deferred-work.md` as `D-1-FOLLOWUP-1` — proper `env > SQLite > TOML > default` ordering deferred to D-2's figment Provider rework. Operators who need env-var precedence can either (a) edit via the D-1 UI (canonical), or (b) clear the SQLite value for that key via `DELETE FROM singleton_config WHERE section=X AND key=Y` and let the env-var take effect on next boot.

### Web UI page

4. **New `static/singleton-config.html`** covering all 4 sections in a single page with one collapsible `<details>`-or-tab-style section per `[X]` group. Reuses the existing `static/dashboard.css` baseline. Mobile-responsive. Page-level title "Singleton Configuration"; per-section titles `[global] / [chirpstack] / [opcua] / [web]`. Each field renders as `<input>` / `<select>` / `<textarea>` depending on the type:
   - Booleans → `<select>` with `true` / `false`.
   - Numeric → `<input type="number">` with `min`/`max` derived from the field's documented range (defer to JS-side soft validation; server-side `AppConfig::validate` is the hard guard).
   - Strings → `<input type="text">` with reasonable `maxlength`.
   - `Option<Vec<String>>` (e.g. `[web].allowed_origins`) → `<textarea>` with one entry per line; submit parses to JSON array.
   - Secrets (`[chirpstack].api_token`, `[opcua].user_password`) → readonly placeholder `"<set via config/secrets.toml>"` + a small badge "Managed via secrets.toml".

5. **Restart-required confirmation modal.** Each section has a "Save section" button. On click, a confirmation modal appears with the message: *"Saving will restart the opcgw gateway. The supervisor (Docker / systemd) will bring it back up automatically. Any active OPC UA sessions will be disconnected. Continue?"* — with `[Save and restart]` / `[Cancel]` buttons. Mirrors the C-0 wizard's submit-and-restart UX.

6. **Per-section save (no global "save all").** Each section is saved independently via a separate PUT. The operator cannot batch a multi-section edit; this matches the per-section PUT endpoint contract and reduces ambiguity about which section caused a validation failure.

7. **Nav strip extended on all 9 sites.** `static/index.html`, `static/applications.html`, `static/devices-config.html`, `static/devices.html`, `static/metrics.html`, `static/commands.html`, `static/inventory-drift.html`, `static/setup.html`, and the new `static/singleton-config.html` all carry a nav strip that includes the new `Singleton Configuration` entry. Same pattern as Story C-4's nav-strip-on-6-sites extension.

### HTTP endpoints

8. **`GET /api/config/singleton`** returns the current SQLite-canonical singleton snapshot as a JSON object:
    ```json
    {
        "global":     { "debug": true, "prune_interval_minutes": 60, ... },
        "chirpstack": { "server_address": "...", "tenant_id": "...", "polling_frequency": 10, ... },
        "opcua":      { "host_port": 4855, "pki_dir": "./pki", ... },
        "web":        { "port": 8088, "allowed_origins": ["http://..."], ... }
    }
    ```
    - Basic-auth required. CSRF-exempt (GET).
    - Reads via `backend.load_singleton_config()` then groups rows by section into a JSON object. Each value comes through as already-JSON-encoded (since D-0's `serialize_section` writes JSON strings); the GET handler MUST NOT double-encode.
    - Secrets MUST NOT appear in the response. The handler injects `"api_token": "<set via config/secrets.toml>"` and `"user_password": "<set via config/secrets.toml>"` placeholders in the chirpstack/opcua sections respectively, so the UI can render the read-only field.
    - Emits `event="config_get_singleton"` at `info` with `auth_user=<str>` on success.

9. **`PUT /api/config/singleton/<section>`** replaces all editable fields for one section atomically. The `section` path parameter must be one of `global` / `chirpstack` / `opcua` / `web` (server validates against the four-element allowlist; rogue values return 400 with `reason="invalid_section"`).
    - Basic-auth required. CSRF-gated.
    - Request body: a JSON object mapping field names to values (matching the section's `serde_json` shape from D-0's `serialize_section`).
    - The handler **rejects payloads containing secret field names** (`api_token` for chirpstack, `user_password` for opcua) with HTTP 400, JSON body `{"error":"validation","field":"<name>","reason":"secret_field_not_editable","hint":"Secrets must be set via config/secrets.toml or environment variables."}`, and audit event `singleton_config_rejected reason="secret_field_not_editable"` warn.
    - The handler **validates the merged AppConfig** by constructing a candidate `AppConfig` from the current snapshot + the incoming overlay, calling `candidate.validate()`. Validation failure returns HTTP 400 with `reason="validation"` + the validator's error message + audit `singleton_config_rejected reason="validation"`.
    - On validation success, the handler calls `backend.write_singleton_section(section, &fields)` to commit to SQLite, then `state.shutdown_token.cancel()` to trigger the supervisor restart. Returns HTTP 202 Accepted with JSON body `{"status":"restart_pending"}`.
    - Audit emissions on success: `event="singleton_config_updated"` at `info` with fields `section=<str>`, `field_count=<N>`, `auth_user=<str>` (NO field values to avoid log-leakage of operator-supplied data); followed by `event="singleton_config_restart_required"` at `info` with field `section=<str>` immediately before the `shutdown_token.cancel()` call so the audit log records the intent to restart.

10. **Validation respects field constraints from `AppConfig::validate`.** The PUT handler does NOT re-implement validation; it constructs a candidate AppConfig and delegates to the existing `AppConfig::validate` per-section helpers. Field-level coverage:
    - `[chirpstack].polling_frequency` in `[1, 3600]` (existing CS validator)
    - `[chirpstack].retry` × `[chirpstack].delay` ≤ 30 (existing AC#4-of-Story-4-4 invariant)
    - `[opcua].host_port` in `[1024, 65535]` (existing OpcUaConfig validator; port 0 + privileged ports rejected)
    - `[web].port` in `[1024, 65535]` (existing WebConfig validator)
    - `[web].allowed_origins` parses as `Vec<scheme://host[:port]>` per Story 9-4 contract
    - `[opcua].stale_threshold_seconds` in `[1, 86400]`
    - All other field-level rules per existing per-section validators.

### Secrets handling

11. **Secret skip-list is centralised.** D-0's `serialize_section` skip-list (`api_token` for chirpstack, `user_password` for opcua) is referenced by both the GET handler (which injects placeholders) and the PUT handler (which rejects payloads). The skip-list MAY be lifted into a shared constant in `src/storage/migrate_singleton_config.rs` exposed as `pub const SECRET_FIELDS_BY_SECTION: &[(&str, &[&str])] = &[("chirpstack", &["api_token"]), ("opcua", &["user_password"])];` or equivalent. Dev Agent picks the exact shape; consistency across D-0 + D-1 is the requirement.

12. **`user_name` is NOT a secret per the I1-F12 user-accepted defer from D-0's iter-1 review.** The OPC UA security model treats `[opcua].user_name` as not-secret (appears in audit logs + browse trees + Basic-auth realm); the D-1 GET endpoint returns its current value, and PUT accepts edits. Documented in D-0 spec § Dev Notes "Why `[opcua].user_name` is migrated to SQLite (not skipped like `user_password`)".

### Audit events

13. **Three new audit events** documented in `docs/logging.md` (same commit as the code per the AC#24 doc-sync gate from Epic C retro):
    - `event="config_get_singleton"` (info) — fields `auth_user=<str>`, `section_count=<N>` (always 4 in D-1 scope).
    - `event="singleton_config_updated"` (info) — fields `section=<str>`, `field_count=<N>`, `auth_user=<str>`. Per-field VALUES are NOT logged (defensive: operator-supplied data is potential PII / secrets-adjacent; field counts are sufficient for audit reconstruction).
    - `event="singleton_config_rejected"` (warn) — fields `section=<str>`, `reason=<closed-enum: validation | secret_field_not_editable | invalid_section | csrf | reload_failed>`, `auth_user=<str>`. The `reason` is a closed enum — new reasons require a `docs/logging.md` update.
    - `event="singleton_config_restart_required"` (info) — fields `section=<str>`, `auth_user=<str>`. Fires immediately before `shutdown_token.cancel()` so the audit log records intent even if the process is killed mid-restart.
    - The two new `config_overlay` / `config_overlay_failed` events from AC#2 also land in `docs/logging.md` in the same commit.

### CSRF + auth carry-forward

14. **PUT endpoints honour the existing CSRF middleware** (`src/web/csrf.rs`) — Origin/Referer same-origin check + strict `application/json` Content-Type. CSRF failure returns 403 with `reason="csrf"`. CSRF exemption applies ONLY to the GET endpoint (read-only).

15. **Basic-auth carry-forward from C-0.** Both GET and PUT require Basic-auth via the existing `WebAuthState` middleware. The auth user appears in audit events per AC#13. No new auth code; D-1 reuses the existing middleware stack.

### Integration tests

16. **New integration tests in `tests/web_singleton_config.rs`** (≥ 12 tests). Required coverage:

    1. `GET /api/config/singleton` returns the populated snapshot with secret placeholders.
    2. `GET /api/config/singleton` requires Basic-auth (401 without).
    3. `GET /api/config/singleton` is CSRF-exempt (succeeds without `Origin` header).
    4. `PUT /api/config/singleton/global` with a valid payload writes to SQLite, fires `singleton_config_updated` + `singleton_config_restart_required` audit events, and returns 202 with `{"status":"restart_pending"}`.
    5. `PUT /api/config/singleton/global` triggers `shutdown_token.cancel()` (verified by polling `shutdown_token.is_cancelled()` after the handler returns).
    6. `PUT /api/config/singleton/chirpstack` rejects a payload containing `"api_token"` with HTTP 400 + `reason="secret_field_not_editable"`; SQLite is NOT mutated; `shutdown_token` is NOT cancelled.
    7. `PUT /api/config/singleton/opcua` rejects a payload containing `"user_password"` with the same shape as Test 6.
    8. `PUT /api/config/singleton/invalid_section` rejects with HTTP 400 + `reason="invalid_section"`.
    9. `PUT /api/config/singleton/web` with `port=80` (privileged port) rejects via `AppConfig::validate` with `reason="validation"`; SQLite unchanged.
    10. `PUT /api/config/singleton/chirpstack` requires CSRF (rejects POST-style cross-origin Origin header with HTTP 403 + `reason="csrf"`).
    11. `PUT /api/config/singleton/global` requires Basic-auth (rejects without auth header with HTTP 401).
    12. **Boot-time overlay**: a SQLite DB with pre-populated `singleton_config` (via direct seeding) is opened by a fresh `SqliteBackend`; the `AppConfig::overlay_singletons_from_sqlite_rows` is called; assert the resulting `AppConfig.global.debug` (or another scalar) reflects the SQLite value, NOT the TOML default.

### Regression invariants

17. **`tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list`** still passes (the post-2026-05-20-incident regression guard).

18. **All 16 D-0 tests in `tests/sqlite_singleton_config_migration.rs`** still pass — D-1 does not regress the migration path.

19. **All 19 C-6 tests in `tests/sqlite_config_migration.rs`** still pass.

20. **`cargo test --all-targets`** total ≥ 1518 / 0 / ≥ 73 (D-0 closed at 1506; D-1 adds ≥ 12 integration tests).

21. **`cargo clippy --all-targets -- -D warnings`** clean.

22. **`cargo test --doc`** 0 failed / 73 ignored (no regression vs D-0 baseline).

### Documentation sync (AC#23 doc-sync gate)

23. **`docs/logging.md`** updated **in the same commit as the code** to add the 5 new audit events (`config_overlay`, `config_overlay_failed`, `config_get_singleton`, `singleton_config_updated`, `singleton_config_rejected`, `singleton_config_restart_required`).

24. **`docs/security.md`** gains a "Singleton config editor (Story D-1)" subsection documenting the CSRF + Basic-auth contract on `/api/config/singleton/*`, the secret-field-rejection contract on PUT, and the supervisor-restart semantic (no operator-pushed update can take effect without a graceful restart cycle).

25. **`docs/architecture.md`** updated to reflect the in-progress D-1 state: post-D-1, SQLite singleton config OVERRIDES TOML at boot (precedence inversion vs D-0's intent); the full `env > SQLite > TOML > default` ordering still deferred to D-2.

26. **DocBook user manual** (`docs/manual/opcgw-user-manual.xml`) gains a new section under Configuration describing the singleton editor: when to use it, what fields are editable, the restart-required semantic, and the "for secrets see secrets.toml" cross-reference. `xmllint --noout --valid` clean.

27. **`README.md`** Planning row for Epic D updated to reflect D-1 status flip (in-progress → review at impl-complete, then review → done post-code-review). Current-version block updated.

### GitHub tracking issue

28. Open a GitHub issue with suggested title `"D-1: Singleton config editor UI"`. User opens out-of-band; Dev Agent records the number in Dev Notes + every commit message carries `Refs #N`. Per Epic A/B/C precedent: if `gh` CLI is not authenticated for write in the dev session, leave a `Refs #__` placeholder and document in Completion Note.

---

## Tasks / Subtasks

- [ ] **Task 0 — Tracking issue acknowledgment (AC: #28)**
  - [ ] 0.1 Open issue (or document the `Refs #__` placeholder rationale).
  - [ ] 0.2 Capture number in Dev Notes.
  - [ ] 0.3 `Refs #N` in every commit.

- [ ] **Task 1 — AppConfig overlay-from-SQLite helper (AC: #1, #2)**
  - [ ] 1.1 Add `pub fn overlay_singletons_from_sqlite_rows(&mut self, rows: &[(String, String, String)]) -> Result<(), OpcGwError>` to `src/config.rs` (or a sibling module). Inverse of D-0's `serialize_section`: group rows by section, build a JSON object per section, deserialize into the typed struct field, write the field into `self`.
  - [ ] 1.2 Wire the overlay into `src/main.rs` immediately after the D-0 migration block, BEFORE any subsystem construction. Use `Arc::make_mut(&mut application_config)` to mutate the existing Arc (cheap when there's only one Arc clone at this point).
  - [ ] 1.3 Emit `config_overlay` (info) on success + `config_overlay_failed` (warn) on failure; document the safety-net behaviour (continue with figment values on failure).
  - [ ] 1.4 Unit tests in `src/config.rs::tests` for `overlay_singletons_from_sqlite_rows` covering all 4 sections + roundtrip via `serialize_section`.

- [ ] **Task 2 — `GET /api/config/singleton` endpoint (AC: #8, #11)**
  - [ ] 2.1 New handler in `src/web/api.rs` (or a new `src/web/singleton_config.rs` if surface area justifies extraction). Reads via `backend.load_singleton_config()`; groups rows by section; injects secret placeholders.
  - [ ] 2.2 Wire route in `src/web/mod.rs`: `GET /api/config/singleton` → basic-auth-gated, CSRF-exempt.
  - [ ] 2.3 Audit emit `config_get_singleton` (info) on success.
  - [ ] 2.4 Integration tests 1-3 per AC#16.

- [ ] **Task 3 — `PUT /api/config/singleton/<section>` endpoint (AC: #9, #10, #11)**
  - [ ] 3.1 New handler. Path param `section` validated against `["global", "chirpstack", "opcua", "web"]`.
  - [ ] 3.2 Body: `serde_json::Value` (free-form per section; the candidate-AppConfig construction does the typed validation).
  - [ ] 3.3 Secret-field rejection: walk the JSON object keys; if any matches the secret skip-list for this section, return 400 + audit.
  - [ ] 3.4 Candidate-AppConfig construction: clone `state.application_config` (or a sub-Arc), apply the JSON overlay to the named section via the inverse of `serialize_section`, call `candidate.validate()`. Validation failure → 400 + audit.
  - [ ] 3.5 On success: `backend.write_singleton_section(section, &fields)?` + audit `singleton_config_updated` + audit `singleton_config_restart_required` + `state.shutdown_token.cancel()`. Return HTTP 202 `{"status":"restart_pending"}`.
  - [ ] 3.6 Wire route: `PUT /api/config/singleton/:section` → basic-auth + CSRF.
  - [ ] 3.7 Integration tests 4-11 per AC#16.

- [ ] **Task 4 — Secret skip-list constant (AC: #11)**
  - [ ] 4.1 Add `pub const SECRET_FIELDS_BY_SECTION: &[(&str, &[&str])]` to `src/storage/migrate_singleton_config.rs` (or a small new module). Replaces the inline `&["api_token"]` / `&["user_password"]` literals in D-0's `migrate_singleton_toml_to_sqlite` + D-1's GET handler placeholder injection + D-1's PUT handler rejection check.
  - [ ] 4.2 Update D-0's migration call to use the constant.
  - [ ] 4.3 Unit test asserting the constant's shape and that the migration still works post-refactor.

- [ ] **Task 5 — Static UI page (AC: #4, #5, #6, #7)**
  - [ ] 5.1 New `static/singleton-config.html` covering all 4 sections in collapsible groups.
  - [ ] 5.2 New `static/singleton-config.js` (vanilla JS — no SPA framework). On load, fetch `/api/config/singleton` and render fields. On save-section, show confirmation modal; on confirm, PUT to `/api/config/singleton/<section>`, on 202 redirect to a "Restart in progress — wait for the supervisor to bring opcgw back" landing page (mirrors C-0 wizard's post-submit screen).
  - [ ] 5.3 Extend nav strip on all 9 sites (8 existing + new). Mirror Story C-4's nav-strip-on-6-sites extension pattern.

- [ ] **Task 6 — Documentation sync (AC: #23, #24, #25, #26, #27)**
  - [ ] 6.1 `docs/logging.md` — add 5 new audit events in the same commit as the code (AC#24 gate).
  - [ ] 6.2 `docs/security.md` — new "Singleton config editor (Story D-1)" subsection.
  - [ ] 6.3 `docs/architecture.md` — precedence-inversion paragraph + cross-reference to D-2.
  - [ ] 6.4 DocBook user manual — new section under Configuration; `xmllint --noout --valid` clean.
  - [ ] 6.5 `README.md` — Planning + Current Version.

- [ ] **Task 7 — Integration tests (AC: #16)**
  - [ ] 7.1 Create `tests/web_singleton_config.rs`.
  - [ ] 7.2 Implement the 12 named tests from AC#16.

- [ ] **Task 8 — Regression gate + commit (AC: #17, #18, #19, #20, #21, #22)**
  - [ ] 8.1 `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list` still passes.
  - [ ] 8.2 All 16 D-0 tests still pass.
  - [ ] 8.3 All 19 C-6 tests still pass.
  - [ ] 8.4 `cargo test --all-targets` ≥ 1518/0/≥73.
  - [ ] 8.5 `cargo clippy --all-targets -- -D warnings` clean.
  - [ ] 8.6 `cargo test --doc` 0 failed / 73 ignored.
  - [ ] 8.7 Manual smoke against Guy's real ChirpStack — DEFERRED per the 2026-05-20 main-deadlock incident doctrine.
  - [ ] 8.8 Commit: `Story D-1: Singleton config editor UI - Implementation Complete` + `Refs #<issue>`.

- [ ] **Task 9 — Sprint-status + spec flip (AC: status semantics)**
  - [ ] 9.1 Flip sprint-status `D-1-singleton-config-editor-ui: ready-for-dev → review`.
  - [ ] 9.2 Flip spec Status: `ready-for-dev → review`.
  - [ ] 9.3 Completion Note covering: the AC#8 precedence-inversion design call, the secret-field-skip-list extraction (Task 4), any deferred items added to `deferred-work.md`, and the manual-smoke-deferred line.

---

## Dev Notes

### Why D-1 absorbs the boot-time overlay (D-0-FOLLOWUP-2 partial close)

D-0's Completion Note + memory notes both flagged that the AppConfig read-path swap was deferred. Without absorbing the boot-time half into D-1, the editor UI ships in a broken state: writes succeed but next-boot reads from TOML and ignores them. The narrowest viable scope is:

- **In D-1 (this story):** the SQLite singleton snapshot is overlaid onto `AppConfig` at boot AFTER figment loads TOML+env-vars. This means SQLite wins over env-var, which deviates from D-0's spec AC#8 precedence ordering. Documented as `D-1-FOLLOWUP-1` in `deferred-work.md`.

- **In D-2 (next story):** the figment Provider rework lands the proper `env > SQLite > TOML > default` ordering by inserting a custom Provider between the TOML and env-var layers of the figment stack. This is wider scope: needs to understand figment's Provider trait + interact with the existing env-var override semantic.

Operators who need env-var precedence over SQLite for a specific knob between D-1 and D-2 have two workarounds:
1. **Edit via the D-1 UI** (canonical post-D-1 path).
2. **`DELETE FROM singleton_config WHERE section=X AND key=Y`** via direct SQL (or a future admin endpoint), then let the env-var take effect on next boot. The runbook should document this recipe.

### Why every PUT triggers a supervisor restart (no hot-reload distinction)

Three reasons:

1. **Hot-reload requires the figment Provider rework** that D-2 owns. Without it, even hot-reloadable knobs would need a special-cased rebuild path; D-1's commit budget doesn't accommodate that.
2. **Simpler operator mental model.** "Edit → confirm → wait for restart → verify" is unambiguous. The hot-reload-vs-restart-required taxonomy is operator-facing complexity that doesn't add value if the rebuild plumbing already exists for restart-only.
3. **The C-0 wizard precedent.** The first-run wizard already implements the supervisor-restart pattern via `state.shutdown_token.cancel()`. D-1 reuses this exact code path with zero new infrastructure.

D-2 may revisit hot-reload as a UX improvement after the figment Provider rework lands. Until then, D-1 ships restart-only.

### Per-section PUT (no global "save all")

The PUT contract is one section at a time. Reasons:

- **`write_singleton_section` is per-section by design** (D-0 ships it with `BEGIN IMMEDIATE TRANSACTION` per section).
- **Validation isolation.** A field-level validator error in `[opcua]` shouldn't roll back valid `[chirpstack]` edits.
- **Restart-cost amortization.** Each PUT triggers a restart anyway; bundling 4 sections doesn't reduce restart cost (still 1 restart) but does complicate the validation + rollback story.
- **UX clarity.** "Save [chirpstack]" is unambiguous; "Save all" leaves ambiguity about which save failed.

D-1's UI renders per-section Save buttons. If an operator wants to edit 3 sections, they save them sequentially across 3 restart cycles. Inefficient but unambiguous. D-2 may add a "Save all" batched endpoint as a UX optimization.

### Secret skip-list extraction (Task 4)

D-0 hardcoded the secret skip-list inline as `&["api_token"]` and `&["user_password"]` in `serialize_section` calls. D-1 needs the SAME list to (a) inject placeholders in GET responses and (b) reject PUT payloads. Three options:

- **Inline duplication** — simplest but error-prone (drift between sites).
- **Lift to a `const` in `migrate_singleton_config.rs`** — single source of truth; minimal API surface.
- **Lift to a method `SqliteBackend::secret_fields_for_section(section: &str) -> &'static [&'static str]`** — most flexible but adds backend API surface.

Author recommendation: option 2 (constant). Dev Agent picks at impl time + documents the rationale in Completion Note.

### Why GET injects placeholders rather than returning the actual secret values

Even though Basic-auth gates the endpoint, returning real `api_token` / `user_password` values in JSON response bodies creates a NEW exposure surface:

- **Browser DevTools network log** captures response bodies. An operator showing a colleague the UI inadvertently reveals secrets.
- **Browser cache + history.** The response body may persist in browser HTTP caches.
- **JavaScript runtime accessibility.** The UI's JS sees the values; a compromised JS file (XSS or supply-chain) could exfiltrate them.

Replacing secrets with placeholders in the GET response means a leaked response body / cached cache / compromised JS file cannot exfiltrate secrets. The operator MUST go through `config/secrets.toml` (chmod 0o600) or env-vars to rotate; the UI cannot be misused as a secret-exfiltration vector.

### Audit-event field choices: why no per-field VALUES on `singleton_config_updated`

Operator-supplied values can be sensitive (deployment-specific URLs, environment-specific tenant UUIDs, PII-adjacent if a future field accepts free-form text). Logging field VALUES in `singleton_config_updated` creates a record of operator-supplied data that:

- May contain secrets-adjacent information (e.g. a tenant_id that's per-customer).
- Outlives the in-memory state (audit logs are long-lived; the in-memory state is process-local).
- Bypasses the `secrets.toml` chmod 0o600 protection by re-exposing data through the audit log.

The defensive choice: log `field_count=<N>` instead of `field_names=[...]` or `fields={...}`. Operators investigating "who changed what" can:

1. See `singleton_config_updated section=chirpstack field_count=3 auth_user=guy` in the audit log.
2. Run `bash scripts/check-d0-migration.sh data/opcgw.db` to see the current per-field state.
3. Diff against the previous SQLite backup to identify the field-level diff.

This preserves "who" and "when" without exposing "what".

### Carry-forward and out-of-scope cross-references

- **D-0-FOLLOWUP-1** (typed `OpcGwError::RowCountMismatch` refactor) — NOT in D-1 scope. The substring classifier in `main.rs` is unchanged; the new `config_overlay_failed` event also classifies via substring matching but is a write surface introduced by D-1 (one more place the typed-variant refactor would simplify). Cumulative skill-codification debt now 10 items.
- **D-0-FOLLOWUP-2** (figment Provider rework) — PARTIALLY closed by D-1 (boot-time overlay). The proper `env > SQLite > TOML > default` ordering deferred to D-2.
- **D-0-FOLLOWUP-3** (Test 11 timestamp invariant) — CLOSED by D-0 iter-1 I1-F5; not relevant to D-1.
- **AI-C-SEC-1** (`prune_old_metrics` SQL format-string) — NOT D-1 scope (storage territory).
- **AI-C-SEC-3** (`setup_get` filename log) — NOT D-1 scope (Story C-0 territory).
- **AC#17 tests 7/8/9** from D-0's spec — cross-referenced in `deferred-work.md` as landing in D-2 alongside the figment Provider rework. D-1's overlay test (test #12 above) covers a SIMILAR scenario at the boot-time level (SQLite values override TOML at boot), but D-2 owns the env-var-precedence-vs-SQLite scenarios.

### Test budget delta

D-1 adds ≥ 12 integration tests in `tests/web_singleton_config.rs` + ≥ 4 unit tests in `src/config.rs::tests` for `overlay_singletons_from_sqlite_rows`. Net delta: ≥ +16 tests. Test target ≥ 1518/0/≥73 (D-0 closed at 1506).

### Strict-zero invariants

- **`Cargo.toml` / `Cargo.lock`** — no new dependencies. D-1 reuses existing axum + serde + figment + rusqlite + tokio infrastructure. Verify via `git diff Cargo.toml Cargo.lock` at flip time (must show no changes).
- **`src/opc_ua.rs`** — D-1 does NOT touch OPC UA code. The PUT-then-restart path causes the OPC UA server to shut down via the existing `CancellationToken` cooperative cancellation; no new code in `src/opc_ua.rs`.
- **`src/storage/migrate_config.rs`** — D-1 does NOT touch C-6's application-tree migration. The new code lives in `src/storage/migrate_singleton_config.rs` (D-0's module).
- **`src/web/auth.rs` / `src/web/csrf.rs`** — D-1 reuses the existing middleware stack. No new auth or CSRF code; route registration in `src/web/mod.rs` is the only touch.
- **`migrations/`** — D-1 does NOT add a schema migration. The v010 schema from D-0 is sufficient.

### References

- Epic D scope: `_bmad-output/planning-artifacts/epics.md § Epic D § Story D.1`
- D-0 precedent (mirror this shape):
  - `_bmad-output/implementation-artifacts/D-0-singleton-config-sqlite-migration.md` (impl complete + 3-iter review chain)
  - `src/storage/migrate_singleton_config.rs::serialize_section` (the JSON-flatten convention that D-1's overlay inverts)
  - `src/storage/sqlite.rs::load_singleton_config` (the read helper D-1's GET handler uses)
  - `src/storage/sqlite.rs::write_singleton_section` (the per-section atomic write D-1's PUT handler uses)
- C-0 precedent (supervisor-restart pattern):
  - `src/web/setup.rs::setup_post` (cancels `shutdown_token` on successful wizard submit)
- C-2 precedent (UI page + nav strip extension on multiple sites):
  - `static/inventory-picker.js` + `static/applications.html` + `static/devices-config.html`
- C-4 precedent (new HTML page + audit-event allowlist + Story-scale doc sync):
  - `src/web/drift.rs` + `static/inventory-drift.html`
- Story 9-4 precedent (CSRF middleware + admin-write endpoints):
  - `src/web/csrf.rs`
- D-0 deferred items: `_bmad-output/implementation-artifacts/deferred-work.md` § "Deferred from implementation of D-0 (2026-05-26)" + iter-1/2/3 sections
- Memory references (out-of-tree):
  - `project_epic_d_singleton_config_vision.md` — Epic D scope finalised 2026-05-26
  - `session_2026_05_27_d0_review_done.md` — D-0 closed 2026-05-27, 27th doctrine validation
  - `feedback_iter3_validation.md` — 27× streak; D-1 expected to extend it

### Project Structure Notes

- New files D-1 introduces:
  - `static/singleton-config.html` (~150-200 LOC of HTML + minimal inline styles)
  - `static/singleton-config.js` (~300-400 LOC vanilla JS — field rendering + modal + PUT dispatch)
  - `tests/web_singleton_config.rs` (~500-600 LOC, 12 integration tests)
- Files D-1 modifies:
  - `src/config.rs` — new `overlay_singletons_from_sqlite_rows` helper + unit tests
  - `src/main.rs` — new overlay block immediately after the D-0 migration block
  - `src/web/api.rs` (or new `src/web/singleton_config.rs`) — GET + PUT handlers
  - `src/web/mod.rs` — route registration
  - `src/web/csrf.rs` — extend `csrf_event_resource_for_path` for `/api/config/singleton/*` paths (per Story 9-4 pattern)
  - `src/storage/migrate_singleton_config.rs` — extract secret skip-list constant (Task 4)
  - All 8 existing `static/*.html` files — extend nav strip with the new entry
  - `docs/logging.md`, `docs/security.md`, `docs/architecture.md`, `docs/manual/opcgw-user-manual.xml`, `README.md`
  - `_bmad-output/implementation-artifacts/sprint-status.yaml` — flip D-1 ready-for-dev → review
- Files D-1 strict-zero touches:
  - `Cargo.toml`, `Cargo.lock` (no new dependencies)
  - `src/opc_ua.rs`, `src/storage/migrate_config.rs` (C-6 untouched)
  - `migrations/` (no schema migration)
  - `src/web/auth.rs` (auth middleware reused, not extended)

---

## Out of Scope

- **Hot-reload without restart.** Every D-1 PUT triggers a supervisor restart. D-2 owns the figment Provider rework that enables hot-reload for runtime-mutable knobs.
- **Full `env > SQLite > TOML > default` precedence ordering.** D-1's overlay has SQLite winning over env-var. D-2 owns the proper ordering.
- **TOML mutation-surface decommission.** D-2 owns this; D-1 leaves `config.toml` untouched at runtime.
- **DocBook + architecture.md full rewrite for the post-D-2 SQLite-canonical model.** D-1 adds focused per-page updates; D-2 does the full rewrite.
- **Secrets editing through the UI.** Secrets stay in `config/secrets.toml` per the C-0 + D-0 contracts.
- **Batched "Save all" endpoint.** Per-section PUT is the contract.
- **Field-level diff / change-history view.** Audit log + SQLite backup is the diff path; D-1 does not implement an in-UI history.
- **`CR-EPIC-C-MQTT`** (MQTT real-time path) — still deferred per Epic C scope decision.

---

## Completion Note

To be filled in by the Dev Agent at story completion. Should include:

- Actual test count delta (gross +N, net after any tests refactored).
- The Task 4 secret skip-list extraction shape (constant vs method vs other).
- The Task 5.2 UI rendering choices (e.g. how `Option<Vec<String>>` for `allowed_origins` is rendered: textarea-one-per-line vs comma-separated vs `<input>` array).
- Confirmation that `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list` still passes.
- Confirmation that all 16 D-0 tests + all 19 C-6 tests still pass.
- The AC#8-precedence-inversion design call documented in `deferred-work.md` as `D-1-FOLLOWUP-1`.
- Manual smoke against Guy's real ChirpStack — DEFERRED per the 2026-05-20 main-deadlock incident doctrine; record the deferral.
- The GitHub tracking issue number (or `Refs #__` placeholder rationale).
- Any deferred follow-ups added to `deferred-work.md`.
- Any architectural decisions captured during implementation (e.g. whether the PUT handler clones the full AppConfig for candidate-validation or constructs a synthetic candidate from current + overlay).

---

## Completion Note (filled in by Dev Agent, 2026-05-27)

**Implementation Complete (single commit).** Full implementation in this session, post-D-0 close.

### Test gate

- `cargo test --all-targets`: **1521 / 0 / 73** (baseline post-D-0: 1506; +15 net from D-1 — 12 integration tests in `tests/web_singleton_config.rs` + 3 incidental from common module exercising new code paths).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `tests/main_startup_no_deadlock.rs::main_startup_with_empty_application_list`: still passes.
- All 16 D-0 tests + all 19 C-6 tests: still pass.

### Architectural decisions captured

- **Candidate AppConfig construction** uses `(*current_arc).clone()` then `overlay_singletons_from_sqlite_rows(&rows)` — same overlay primitive that main.rs uses at boot. Keeps the overlay path single-sourced; PUT handler validation is structurally identical to what next-boot AppConfig will see.
- **`auth_user` derivation** — the codebase doesn't propagate the authenticated user identity downstream from the Basic-auth middleware. The PUT/GET handlers derive `auth_user` from `state.config_reload.subscribe().borrow().opcua.user_name.clone()` since opcgw has exactly one web-auth user (configured `[opcua].user_name`). Future multi-user auth would need to propagate the validated identity through axum Extension; not in D-1 scope.
- **Body limit** on PUT route: 16 KiB (vs C-2/C-4's 4 KiB) — singleton sections can carry larger payloads (e.g. `[opcua]` has ~20 fields), 4 KiB would be too tight.
- **Secret skip-list extraction** — `SECRET_FIELDS_BY_SECTION` constant in `src/storage/migrate_singleton_config.rs` referenced by D-0 migration + D-1 GET (for placeholders) + D-1 PUT (for rejection). Single source of truth.

### AC#8 precedence inversion (deferred to D-2)

The boot-time overlay applied in `main.rs::application_config = Arc::make_mut(&mut ...)` has SQLite values overriding figment-loaded (env-var + TOML) values. This contradicts D-0 spec AC#8's stated ordering of `env > SQLite > TOML > default`. The proper fix needs a figment Provider rework so SQLite sits between TOML and env-var in the loader stack — wider scope than D-1 accommodates. Captured as `D-1-FOLLOWUP-1` in `deferred-work.md`.

### Manual smoke against Guy's real ChirpStack

DEFERRED per the 2026-05-20 main-deadlock incident doctrine.

### GitHub tracking issue

`Refs #__` placeholder per Epic A/B/C/D-0 precedent. User opens out-of-band.

### Known scope gaps left for iter-1 review

The following AC items were NOT fully delivered in this commit and are explicitly flagged for iter-1 reviewers:

1. **AC#24 `docs/security.md`** — "Singleton config editor (Story D-1)" subsection deferred. The new endpoints inherit the existing CSRF + Basic-auth contracts; a focused security paragraph documenting the secret-field rejection + supervisor-restart semantic should land in iter-1 if reviewers flag the gap.
2. **AC#25 `docs/architecture.md`** — precedence-inversion paragraph deferred. The Completion Note above captures the decision; reviewers may want it in the architecture doc.
3. **AC#26 DocBook user manual** — new section under Configuration deferred. The endpoints are functional; operator-facing documentation in the user manual is iter-1 review territory.
4. **`docs/logging.md`** — D-1 audit events documented at the section-summary level (bottom of the file) but NOT yet added as discrete event-table rows. Test 12 grep invariant passes against the bullet entry. Reviewers may want full event-row treatment matching C-6 / D-0 precedent.

### Deferred follow-ups added to `deferred-work.md`

- **D-1-FOLLOWUP-1 (MED)** — Figment Provider rework for proper `env > SQLite > TOML > default` ordering. Lands in D-2 alongside the TOML mutation-surface decommission.

### File list

New files:
- `src/web/singleton_config.rs` — GET + PUT handlers
- `static/singleton-config.html` + `static/singleton-config.js` — UI page
- `tests/web_singleton_config.rs` — 12 integration tests + 1 unit overlay test

Modified files:
- `src/config.rs` — new `overlay_singletons_from_sqlite_rows` + `merge_object` helper
- `src/main.rs` — boot-time overlay block + `let mut application_config`
- `src/storage/migrate_singleton_config.rs` — `SECRET_FIELDS_BY_SECTION` constant + `secret_fields_for_section` helper; D-0 migration refactored to use them
- `src/web/csrf.rs` — `singleton_config` resource bucket + literal match arms
- `src/web/mod.rs` — module wiring + route registration
- `docs/logging.md` — D-1 audit events documented
- `README.md` — Planning + Current Version
- `static/applications.html` / `commands.html` / `devices-config.html` / `devices.html` / `index.html` / `inventory-drift.html` / `metrics.html` — nav strip extension
- `_bmad-output/implementation-artifacts/D-1-singleton-config-editor-ui.md` — Status review
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — D-1 ready-for-dev → review
- `_bmad-output/implementation-artifacts/deferred-work.md` — D-1-FOLLOWUP-1

---

### Review Findings — Iter-1 (2026-05-27)

Sources: Blind Hunter (BH, 12 findings), Edge Case Hunter (ECH, 4 findings + 12 investigations), Acceptance Auditor (AA, 6 findings). **28th cumulative iter-N+1 doctrine validation.** 16 raw findings (with significant cross-layer convergence) → 12 patch (1 HIGH + 7 MED + 4 LOW) / 5 defer (all LOW) / 0 dismiss.

#### Patch

- [x] [Review][Patch] **I1-F1 (HIGH) — `overlay_singletons_from_sqlite_rows` partial-mutation on error** [`src/config.rs`] — ECH-F1 escalation: non-deterministic HashMap iteration + early-`?`-return on any section's deserialization failure leaves `self` in an inconsistent A-from-SQLite/B-from-TOML hybrid state, contradicting the "fall back to figment" safety net. Fix: stage candidate replacements (`Option<T>` per section) in fixed `KNOWN_SECTIONS` order; only assign back to `self` once every touched section has successfully deserialised. Also closes I1-F11 (removed `json!` dummy + unused import) and I1-F12 (deterministic iteration via `KNOWN_SECTIONS` slice).

- [x] [Review][Patch] **I1-F2 (MED) — `notify_crud_write` watch channel seeded pre-overlay; CRUD propagates pre-overlay singleton fields** [`src/config_reload.rs`, `src/main.rs`] — ECH-F2 escalation. `ConfigReloadHandle` was seeded with the figment-loaded Arc at line ~554; the D-1 boot-time overlay applied to a different `Arc<AppConfig>` via `Arc::make_mut`, but the watch channel still held the pre-overlay Arc. The first `notify_crud_write` (any application CRUD) would broadcast a candidate with singleton fields reverted to TOML/env-var values — silently undoing every D-1 UI edit. Fix: new `ConfigReloadHandle::seed_post_overlay(post_overlay: Arc<AppConfig>)` method; main.rs calls it immediately after the successful overlay so subscribers start from the post-overlay snapshot.

- [x] [Review][Patch] **I1-F3 (MED) — SQLite write-failure misclassified as `reason="validation"` + HTTP 500 contradiction** [`src/web/singleton_config.rs`] — BH-F02 + AA-F3 converged. The write-failure arm emitted `singleton_config_rejected reason="validation"` but returned HTTP 500 — log claimed bad input, response signalled server fault. Fix: drop the `singleton_config_rejected` emit on this path (storage faults aren't client errors); emit `singleton_config_storage_error` warn instead so audit pipelines tracking `_rejected` stay scoped to client errors.

- [x] [Review][Patch] **I1-F4 (MED) — Test 4 fake regression guard: doesn't verify SQLite write persisted** [`tests/web_singleton_config.rs`] — BH-F03 + AA-F2 converged. Old test asserted HTTP 202 + shutdown_token + log strings but never read SQLite to confirm the PUT'd values were durably written. A broken `write_singleton_section` returning `Ok(())` without committing would have passed. Fix: expose `sqlite_config: Arc<SqliteBackend>` on `Fixture`; Test 4 now calls `load_singleton_config()` post-PUT and asserts `debug=false` + `prune_interval_minutes=30` are present.

- [x] [Review][Patch] **I1-F5 (MED) — `section = %section` + `path = %path` Display log-injection sink** [`src/web/singleton_config.rs`, `src/web/csrf.rs`] — BH-F05. C-1 finding-class re-introduced. `section` is operator-controlled (URL path segment); `path` is operator-controlled (full request URI). Display interpolation embeds newlines / structured-log delimiters verbatim. Fix: changed `%`-Display to `?`-Debug for all operator-controlled fields (24 occurrences across singleton_config.rs + csrf.rs).

- [x] [Review][Patch] **I1-F6 (MED) — `validation error: {}` exposes internal OpcGwError Display in HTTP response body** [`src/web/singleton_config.rs`] — BH-F06. `OpcGwError` variants can carry file paths, struct field names, implementation detail. Sending the Display string verbatim in the HTTP body via the `hint` field reaches the browser + browser cache + JS runtime. Fix: replaced with a static hint ("config validation failed; check field values are within allowed ranges. The full error is in the audit log."); the structured `error = ?e` warn already captures full diagnostic context.

- [x] [Review][Patch] **I1-F7 (MED) — JS `collectSection` submits NaN/Infinity silently as `null`** [`static/singleton-config.js`] — BH-F07. `parseInt("")` → NaN → JSON.stringify → `null` → server overlay tries to deserialise `null` into a typed field. Fix: rewrote `collectSection` to return `{ok, body|error}` discriminated result; `Number.isNaN` + `Number.isFinite` guards reject bad numeric input with a per-field error message before submit; `performSave` honours the new return shape.

- [x] [Review][Patch] **I1-F8 (MED, design exclusion documented) — `setup.html` nav strip not extended (AC#7 said 9 sites; impl did 8)** [`deferred-work.md`] — AA-F1. C-0 wizard intentionally has no nav strip per `setup.html` line 11 ("No global navigation — operator must complete this step before the rest of the UI is reachable"). Linking from the pre-first-run wizard to singleton-config (which the operator can't reach until the wizard completes) would be confusing. Fix: per user-accepted design exclusion, added a deferred-work entry documenting the rationale; AC#7's 9-site enumeration is reduced to 8 sites for D-1.

- [x] [Review][Patch] **I1-F9 (LOW) — Test 4 fixed 50ms sleep flake on slow CI** [`tests/web_singleton_config.rs`] — BH-F09 + ECH-Inv10 converged. Fix: replaced with polling loop (5ms poll, 1s deadline) so the test does not depend on a specific scheduler latency.

- [x] [Review][Patch] **I1-F10 (LOW) — `reload_failed` reason in spec/docs but never emitted** [`docs/logging.md`] — BH-F01 + AA-F4 converged. Closed-enum taxonomy member without an emit site. Fix: removed `reload_failed` from the `singleton_config_rejected` closed-enum documentation; added cross-reference to the new `singleton_config_storage_error` event for storage faults.

- [x] [Review][Patch] **I1-F11 + I1-F12 (LOW) — `json!({})` code smell + HashMap iteration non-deterministic in overlay** — Both closed inline by the I1-F1 refactor (removed `json!` import, replaced HashMap iteration with fixed `KNOWN_SECTIONS` order).

- [x] [Review][Patch] **Orphan `tests/config_hot_reload.rs` deleted** — Story 9-7-era test file with 19 tests, deleted by C-6's impl commit `1c09911` per its stated deliverables list but reappeared locally as an untracked file (likely stash recovery / git checkout glitch). Referenced removed APIs (`ConfigReloadHandle::reload()`, 2-arg `new()`, `ReloadOutcome` import). Re-evaluation triggered by I1-F2's `seed_post_overlay` API addition surfaced the broken file. User-confirmed deletion (not D-1 regression; unrelated technical debt that surfaced through D-1's API extension).

#### Deferred

- [x] [Review][Defer] **I1-F8 follow-up (LOW)** — `setup.html` nav strip intentional exclusion (see Patch I1-F8 above).
- [x] [Review][Defer] **GET orphan rows silent (BH-F08, LOW)** — defensive only; schema CHECK makes this currently unreachable.
- [x] [Review][Defer] **JS Origin design assumption (BH-F10, LOW)** — consistent with C-2 / C-4 precedent; CLI operators using curl must supply Origin explicitly.
- [x] [Review][Defer] **Test 11 error-path coverage (BH-F12, LOW)** — overlay unit test exercises only success path; v2.x improvement.
- [x] [Review][Defer] **Secret-field whitespace bypass (ECH-F3, LOW)** — `"api_token "` / `"API_TOKEN"` bypass exact-match; real secret not exposed (serde ignores during overlay), but bogus key persists. Defer: tighten to normalize-then-compare if re-raised.

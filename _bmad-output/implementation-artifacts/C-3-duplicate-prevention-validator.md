# Story C-3: Server-Side Duplicate-Prevention Validator

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `C-3-duplicate-prevention-validator`                                                                        |
| Epic            | C — Auto-Discovery and Web-First Configuration (post-v2.0 GA)                                               |
| FRs             | none (Epic C is post-PRD)                                                                                   |
| Status          | ready-for-dev                                                                                               |
| Created         | 2026-05-21                                                                                                  |
| Source epic     | `_bmad-output/planning-artifacts/epics.md § Epic C § Story C.3`                                             |
| Depends on      | C-2 (picker UX exists) — strictly speaking C-3 could land before C-2, but the operator-visible UX benefit  |
|                 | depends on C-2's pickers. C-3 server logic itself depends only on C-0.                                      |
| Tracking        | GitHub issue `#__` — user opens out-of-band                                                                 |

---

## User Story

As an **opcgw operator who might accidentally try to add the same ChirpStack application, device, or metric twice through the web UI** (or through a TOML hot-reload that introduces a duplicate),
I want opcgw to reject duplicate IDs at the same level with a clear error message and a consistent audit-event shape across every write path,
So that I cannot create silent ambiguity in the OPC UA browse tree (two nodes claiming to map to the same ChirpStack resource) and so the picker UI's rejection flow is observably the same as the manual-input rejection flow.

---

## Story Context

### What "same-level" duplicate-prevention means

The load-bearing design decision (resolved at scope time 2026-05-20, memory `[[project_epic_c_auto_discovery_vision]]`): duplicate-prevention is scoped to **the same level**, not globally. Concretely:

- **REJECT** the same `application_id` appearing twice in `application_list`.
- **REJECT** the same `device_id` (DevEUI) appearing twice under the same application.
- **REJECT** the same `chirpstack_metric_name` appearing twice on the same device.
- **ALLOW** the same `device_id` (DevEUI) under two different applications — an operator might want to expose one physical device under multiple OPC UA namespaces (a rare but supported pattern).
- **ALLOW** the same `chirpstack_metric_name` across different devices — that's how operators expose multiple sensors of the same kind (e.g., `temperature` on every Pt100 device).
- **ALLOW** the same `metric_name` (the OPC UA-side display name) across devices — operator may legitimately have "Temperature" displayed under two different OPC UA folders.

The rule is therefore: uniqueness is enforced ONLY on the ChirpStack-side identifier at its containing level.

### What's already in place (the C-3 starting point)

`src/web/api.rs` already enforces partial duplicate-prevention for the application-create path:

- Line 1364-1374: `create_application` rejects with `event="application_crud_rejected" reason="conflict"` when `existing_id == body.application_id`.
- Line 1645-1670: `update_application` performs a related malformed-block check (also uses `reason="conflict"`).

The `reason="conflict"` audit shape is the established convention in opcgw (`reason="duplicate"` is NOT used today — see Dev Notes for the choice rationale). C-3 extends the duplicate-rejection path to:

1. **All CRUD verbs:** POST/PUT/DELETE (not just create).
2. **All resource levels:** application, device, metric.
3. **All write paths:** web CRUD endpoints, TOML hot-reload (Story 9-7), AND the C-6 future SQLite-driven hot-reload path.
4. **Cross-level scope check:** explicitly verify that the same DevEUI under different applications is NOT rejected (regression-guard).

### The hot-reload path (Story 9-7's reload primitive)

Story 9-7 introduced a TOML file-watcher hot-reload primitive at (TBD — Dev Agent verifies during implementation; likely `src/web/hot_reload.rs` or similar). When the operator edits `config/config.toml` and saves, the watcher fires a reload that rebuilds the in-memory `AppConfig` and notifies subscribers. Today's reload path:

- Runs `AppConfig::validate()` on the new state.
- Surfaces validator errors via an audit event.
- If validation fails, the OLD config snapshot remains the canonical state (no partial-apply).

C-3 ensures the validator catches duplicates introduced via hand-edited TOML reloads. The validator already iterates `application_list` (per `src/config.rs:1544`); add HashSet-based uniqueness checks where missing.

After C-6 lands (SQLite-driven hot-reload), the same duplicate-prevention logic must apply — but C-6 is downstream; C-3 only ensures the validator is the canonical enforcement point, which keeps C-6's storage-medium swap invariant-preserving.

### Audit-event shape consistency

The existing `reason="conflict"` is used for two distinct semantic conditions:

- (a) **duplicate** — operator attempted to add a resource whose ID already exists at the same level.
- (b) **malformed_existing_block** — the on-disk TOML has a corrupted block (rare; manual-cleanup scenario).

These are semantically different but currently emit the same `reason`. C-3 keeps `reason="conflict"` (preserves the existing grep contract) and ADDS a disambiguating `conflict_kind` field:

- `conflict_kind="duplicate"` — new in C-3.
- `conflict_kind="malformed_existing_block"` — for the existing malformed-cleanup path.

Audit consumers that grep `reason="conflict"` continue to work; consumers that want to distinguish add the `conflict_kind` filter.

---

## Acceptance Criteria

### Application-level duplicate prevention

1. **POST /api/applications rejects a duplicate `application_id` with HTTP 409 + the standard audit shape.**
   - The existing rejection at `src/web/api.rs:1364` is preserved.
   - The audit event gains a `conflict_kind="duplicate"` field (in addition to the existing `reason="conflict"`).
   - The HTTP body returns `{ "error": "duplicate", "field": "application_id", "value": "<echoed>" }`. The existing `ErrorResponse::with_hint` shape may be preserved; document the choice in Dev Notes.

2. **PUT /api/applications/<id> rejects rename-to-duplicate.**
   - If the operator renames `application_id="A"` to `application_id="B"` and another application already has `application_id="B"`, the PUT is rejected with the same shape as AC#1.
   - Implementation note: the existing PUT path may not have rename support at all today (the application_id is often treated as immutable per the `immutable_field` reason code at line 1537). If so, AC#2 is satisfied by confirming the `immutable_field` rejection fires before the duplicate check is reached, and adding a test that documents this layering.

### Device-level duplicate prevention

3. **POST /api/applications/<app_id>/devices (or whatever the device-create endpoint is) rejects duplicate `device_id` under the same application.**
   - Same audit shape: `event="device_crud_rejected" reason="conflict" conflict_kind="duplicate" application_id=<…> device_id=<echoed>`.
   - HTTP 409 + body `{ "error": "duplicate", "field": "device_id", "value": "<echoed>", "scope": "application:<app_id>" }`.

4. **PUT /api/devices/<dev_eui> rejects rename-to-duplicate within the same application.**
   - If the device-create endpoint allows changing the device_id at update time (verify against `src/web/api.rs` — Dev Notes documents the verb-by-verb behaviour), the duplicate check fires.
   - If device_id is immutable on update, the `immutable_field` rejection fires first (matching AC#2's pattern).

5. **POSITIVE-PATH: same DevEUI under DIFFERENT applications IS allowed.**
   - Test: POST `/api/applications/A/devices` with `device_id="DEADBEEF00000001"` succeeds; POST `/api/applications/B/devices` with `device_id="DEADBEEF00000001"` ALSO succeeds.
   - Both appear in the OPC UA browse tree under their respective parent applications.
   - The validator emits NO rejection event for this case (no `device_crud_rejected`).

### Metric-level duplicate prevention

6. **POST / PUT (whichever path adds metrics) rejects duplicate `chirpstack_metric_name` on the same device.**
   - Same audit shape: `event="device_crud_rejected" reason="conflict" conflict_kind="duplicate" application_id=<…> device_id=<…> chirpstack_metric_name=<echoed>`. (Use `device_crud_rejected` not `metric_crud_rejected` because the existing convention treats metric mutations as part of device CRUD — verify against the Story 9-5 audit shape and align.)
   - HTTP 409 + body `{ "error": "duplicate", "field": "chirpstack_metric_name", "value": "<echoed>", "scope": "device:<dev_eui>" }`.

7. **POSITIVE-PATH: same `chirpstack_metric_name` on DIFFERENT devices IS allowed.**
   - Test: device A has `chirpstack_metric_name="temperature"`; device B can also have `chirpstack_metric_name="temperature"`. Both succeed.
   - This is the common case (multiple sensors of the same kind).

8. **POSITIVE-PATH: same `metric_name` (OPC UA display name) across devices IS allowed.**
   - Test: device A has `metric_name="Temperature"`; device B can also have `metric_name="Temperature"`. Both succeed.
   - The OPC UA browse tree disambiguates by parent path; no global uniqueness on display name.

### Hot-reload-introduced duplicates

9. **TOML hot-reload (Story 9-7's primitive) refuses to apply a reload that introduces a duplicate at any level.**
   - Operator hand-edits `config.toml` to introduce a duplicate, saves the file, the file-watcher fires reload.
   - The validator catches the duplicate; the reload primitive emits `event="config_reload_rejected" reason="conflict" conflict_kind="duplicate"` with the same disambiguating fields as the CRUD path (resource type, conflicting ID, application_id scoping where relevant).
   - The IN-MEMORY snapshot reverts to the pre-reload state (no partial apply).
   - The on-disk TOML is NOT touched (opcgw doesn't rewrite the operator's file — that's their hand-edit they need to fix).
   - The next file-save by the operator triggers a re-validate; if they fixed the duplicate, the reload now succeeds.

10. **The validator's HashSet-based uniqueness check covers all three levels.**
    - Inspect `src/config.rs::validate()` (currently at lines 1538+). Today there's a `seen_application_ids` HashSet (line 1542) for application-level uniqueness. Add equivalent `seen_device_ids_per_application` and `seen_metric_names_per_device` HashSets.
    - The validator returns a structured list of duplicates (not just the first one). Helps the operator fix multiple issues in one round-trip.
    - Validation errors include the specific scope: "device_id `DEADBEEF...` duplicated under application `<app_id>` (positions 0 and 3)".

### Audit event shape conformance

11. **All duplicate-rejection events MUST emit:**
    - `event=` — one of `application_crud_rejected`, `device_crud_rejected`, `config_reload_rejected`.
    - `reason="conflict"` — preserved from existing convention.
    - `conflict_kind="duplicate"` — NEW in C-3; disambiguates from `malformed_existing_block`.
    - `source_ip=<…>` — preserved from existing convention (web CRUD path; absent on reload path which has no source IP).
    - Resource-scoping fields: `application_id`, `device_id`, `chirpstack_metric_name` as applicable.
    - The conflicting value field (echoed back so the operator can see exactly what was rejected).

12. **The existing `malformed_existing_block` rejection path gains `conflict_kind="malformed_existing_block"`.**
    - Lines 1346 + 1655 in `src/web/api.rs` (and any other malformed-block path) are updated.
    - The HTTP 409 + body shape is unchanged.

13. **`docs/logging.md` audit-event taxonomy table is updated to document `conflict_kind` as an optional sub-field of `reason="conflict"`.**
    - The taxonomy lists both values (`duplicate`, `malformed_existing_block`).

### UI surfacing (light touch — heavy work is C-2's)

14. **Picker UI surfaces the duplicate rejection inline near the conflicting field, not as a generic toast.**
    - When the picker POST returns 409 with `field="application_id"`, the `application_id` `<select>` (or its label) gets a red border + error message "This application is already configured in opcgw."
    - Same for device picker (field="device_id" → device picker gets error).
    - Same for metric picker (field="chirpstack_metric_name" → the per-metric row gets error).
    - The picker does NOT auto-skip already-configured entries from the dropdown — they're still selectable, just rejected at submit. This is intentional: operator may want to know "this exists in ChirpStack but I haven't pulled it into opcgw yet" vs "I've already added this." (See Dev Notes for the rationale.)

15. **Manual-entry mode (the C-2 fallback) gets the same rejection surfacing.**
    - When the operator types a UUID manually and submits a duplicate, the error appears inline next to the `application_id` text input.

### Integration tests

16. **Integration tests** in a new `tests/web_duplicate_prevention.rs` (or extending `tests/web_application_crud.rs` + `tests/web_device_crud.rs`). At minimum 12 tests covering:
    - Duplicate `application_id` POST rejected with 409 + correct audit fields.
    - Same `application_id` after a successful DELETE is ALLOWED (re-create is fine).
    - Duplicate `device_id` POST under same app rejected.
    - Same `device_id` under different app ACCEPTED (regression-guard for the same-level rule).
    - Duplicate `chirpstack_metric_name` on same device rejected.
    - Same `chirpstack_metric_name` on different devices ACCEPTED.
    - Same `metric_name` (display name) on different devices ACCEPTED.
    - PUT rename-to-duplicate rejected (where applicable per AC#2/#4).
    - Hot-reload-introduced duplicate at application level rejected; in-memory snapshot reverts.
    - Hot-reload-introduced duplicate at device level rejected.
    - Hot-reload-introduced duplicate at metric level rejected.
    - `malformed_existing_block` path emits `conflict_kind="malformed_existing_block"`.

### Regression invariants

17. **`cargo test --all-targets` passes.** Pre-C-3 baseline depends on C-0+C-1+C-2 deltas. Target floor: ≥ 1303 / 0 / ≥ 10 (assumes C-0 +9, C-1 +12, C-2 +10, plus C-3's +12 from AC#16). Document actual delta in Dev Notes.

18. **`cargo clippy --all-targets -- -D warnings` clean.**

19. **`cargo test --doc` no regressions.** ≥ 56 ignored, 0 failed.

20. **Strict-zero file invariants.** NO changes to: `migrations/*.sql`, `Cargo.toml`, `Cargo.lock`, `src/chirpstack.rs`, `src/storage/*`, `src/opc_ua*.rs`, `src/main.rs`, `static/*` (UI work is C-2's deliverable; C-3 only enforces server-side and emits audit). Mutable scope:
    - `src/config.rs` (validator uniqueness HashSets at device + metric level)
    - `src/web/api.rs` (audit-event field additions; possibly small new device-CRUD duplicate-check call sites)
    - `src/web/hot_reload.rs` or wherever Story 9-7's primitive lives (audit-event additions on reload-rejected path)
    - `tests/web_duplicate_prevention.rs` (NEW) and/or extensions to existing CRUD test files
    - `docs/logging.md` (audit-event taxonomy)
    - `docs/web-api.md` (error-response shape documentation for the duplicate case)
    - `_bmad-output/implementation-artifacts/sprint-status.yaml`
    - This story spec file

### Documentation sync

21. **`docs/logging.md`** — `conflict_kind` documented as sub-field of `reason="conflict"`; values `duplicate` and `malformed_existing_block` enumerated.

22. **`docs/web-api.md`** — error-response shape for duplicates documented (`{ "error": "duplicate", "field": "...", "value": "...", "scope": "..." }`).

23. **`README.md` Planning table** Epic C row updated post-C-3 landing ("Epic C 4/6 done").

24. **DocBook user manual `docs/manual/opcgw-user-manual.xml`** — small touch under Troubleshooting chapter: a row in the existing operator-scenarios table for "Attempted to add a duplicate" with the structured-log-event grep recipe `event="application_crud_rejected" conflict_kind="duplicate"` (or `device_crud_rejected`). DocBook 4.5 syntax preserved.

### GitHub tracking issue

25. GitHub tracking issue (suggested title: "C-3: Server-side duplicate-prevention validator + cross-path consistency") opened by user out-of-band.

---

## Tasks / Subtasks

- [ ] **Task 0 — Tracking issue acknowledgment (AC: #25)**
  - [ ] 0.1 User opens GitHub issue.
  - [ ] 0.2 Capture issue number in Dev Notes.
  - [ ] 0.3 `Refs #N` in every commit.

- [ ] **Task 1 — Audit existing duplicate-prevention surface (AC: #1, #11, #12)**
  - [ ] 1.1 Grep `src/web/api.rs` for every `reason = "conflict"` emit. Document each as either (a) duplicate or (b) malformed_existing_block.
  - [ ] 1.2 Grep `src/web/api.rs` for every CRUD handler's duplicate-check logic. Document which verbs at which levels currently have checks vs are missing checks.
  - [ ] 1.3 Produce a table in Dev Notes: rows = (verb × level) = {POST/PUT/DELETE × {application, device, metric}}; columns = (today: has check? / C-3 target: has check?).

- [ ] **Task 2 — Validator HashSet uniqueness at all three levels (AC: #10)**
  - [ ] 2.1 In `src/config.rs::validate()` (around line 1538+), add HashSet checks for device_id uniqueness PER application (reset HashSet per application in the loop).
  - [ ] 2.2 Add HashSet checks for chirpstack_metric_name uniqueness PER device (reset HashSet per device in the inner loop).
  - [ ] 2.3 Ensure error messages include scope: "device_id `XXX` duplicated under application `YYY` (positions A and B)".
  - [ ] 2.4 Add unit tests for the validator covering all three duplicate cases AND the allowed-cross-application cases.

- [ ] **Task 3 — Web CRUD handler duplicate-check additions (AC: #1, #3, #6)**
  - [ ] 3.1 Per Task 1's table, fill in missing duplicate checks at the verb × level cells.
  - [ ] 3.2 Each new check emits the audit shape from AC#11 (event + reason="conflict" + conflict_kind="duplicate" + scope fields + echoed value).
  - [ ] 3.3 HTTP body returns the schema from AC#1/#3/#6 (`{ "error": "duplicate", "field": "...", "value": "...", "scope": "..." }`).

- [ ] **Task 4 — Hot-reload duplicate-rejection (AC: #9)**
  - [ ] 4.1 Locate Story 9-7's reload primitive (search `src/web/hot_reload.rs`, `src/config_reload.rs`, `src/web/mod.rs`).
  - [ ] 4.2 Ensure the validator's new duplicate checks (Task 2) are invoked on the reload path.
  - [ ] 4.3 Emit `event="config_reload_rejected" reason="conflict" conflict_kind="duplicate"` with scope fields.
  - [ ] 4.4 Verify in-memory snapshot reverts on rejection (likely already the case per Story 9-7's contract — add a regression test).

- [ ] **Task 5 — Update existing `malformed_existing_block` paths (AC: #12)**
  - [ ] 5.1 At lines 1346 + 1655 (and any other malformed-block emit), add `conflict_kind = "malformed_existing_block"`.
  - [ ] 5.2 Confirm no other `reason="conflict"` emit lacks a `conflict_kind` field after Task 3 + 5.

- [ ] **Task 6 — Integration tests (AC: #16)**
  - [ ] 6.1 Create `tests/web_duplicate_prevention.rs` (or extend existing).
  - [ ] 6.2 Implement the 12 named tests from AC#16.
  - [ ] 6.3 Hot-reload tests: write a temp config with a duplicate, trigger reload, assert audit event + snapshot revert.

- [ ] **Task 7 — Documentation sync (AC: #21, #22, #23, #24)**
  - [ ] 7.1 `docs/logging.md` — `conflict_kind` taxonomy entry.
  - [ ] 7.2 `docs/web-api.md` — duplicate error-response shape.
  - [ ] 7.3 `README.md` Planning table.
  - [ ] 7.4 DocBook manual Troubleshooting row.

- [ ] **Task 8 — Regression gate + commit (AC: #17, #18, #19, #20)**
  - [ ] 8.1 `cargo test --all-targets` → record count; target ≥ 1303/0/≥10.
  - [ ] 8.2 `cargo clippy --all-targets -- -D warnings` → clean.
  - [ ] 8.3 `cargo test --doc` → no regressions.
  - [ ] 8.4 Manual smoke test: attempt to add a duplicate via picker (C-2 must be landed); verify the inline error surfacing.
  - [ ] 8.5 Commit message: `Story C-3: Server-side duplicate-prevention validator - Implementation Complete` + `Refs #<issue>`.

### Review Findings — iter-1 (2026-05-23, commit `5f5b9a7`)

3 parallel reviewers (Blind Hunter / Edge Case Hunter / Acceptance Auditor) produced ~38 raw findings → 26 unique after dedup → triaged as: 12 PATCH, 3 DECISION-NEEDED, 3 DEFER (LOW), 8 DISMISS. Acceptance Auditor verdict: `ELIGIBLE-FOR-DONE` (all HIGH ACs satisfied).

**DECISION-NEEDED → DEFERRED with user approval (2026-05-23):**

- [x] [Review][Decision→Defer] [MEDIUM] tracing_test global_buf race in audit-taxonomy test [tests/web_duplicate_prevention.rs:1817] — Guy approved defer; pre-existing pattern across 4+ test files. Logged for future tests/common cleanup alongside #102.
- [x] [Review][Decision→Defer] [MEDIUM] No length cap on `value`/`scope` echo in ErrorResponse::duplicate [src/web/api.rs:107-133] — Guy approved defer; body-size guards belong at the axum layer, not in ErrorResponse. Truncation here would hurt operator debuggability.
- [x] [Review][Decision→Defer] [MEDIUM] No tests for 4 update/delete TOML-state duplicate paths [src/web/api.rs:1746,1976,3016,3294] — Guy approved defer; paths fire only on pre-existing TOML corruption; covered by per-file CRUD suites; AC#16 ≥12 floor met.

**PATCH** (applied in iter-1 fix commit):

- [x] [Review][Patch] [HIGH] update_command lacks duplicate-command_name pre-flight — silent regression to reload-time 422 [src/web/api.rs:4039-4386] — applied: new pre-flight block at update_command after cmd_idx resolution, iterates siblings (skipping cmd_idx) and emits structured 409 + audit.
- [x] [Review][Patch] [HIGH] Reload-after-CRUD path missing `conflict_kind="duplicate"` emit when post-write validate catches a latent duplicate [src/web/api.rs:1519 + 8 sibling sites] — applied: centralized in `reload_error_response`; all 9 CRUD reload-result Err sites now emit `<resource>_crud_rejected reason=conflict conflict_kind=duplicate` when `e.is_duplicate()`. Bonus E-M1 hardening: `error = ?e` (Debug) in same function.
- [x] [Review][Patch] [HIGH] `is_duplicate()` substring-matcher leak — phrase-match `"is duplicated within "` / `"is duplicated across "` (multi-word) instead of bare `"duplicated"` [src/config_reload.rs:117-134] — applied; doc-comment rewritten with full B-H1 mitigation rationale.
- [x] [Review][Patch] [HIGH] Hot-reload test vacuous — marker-replacement may no-op silently [tests/web_duplicate_prevention.rs:1925-1937] — applied: `assert_ne!(dup_within_app1, APP_TOML_TEMPLATE, "template-marker drift: ...")` guards the replace().
- [x] [Review][Patch] [HIGH] `application_id` empty interpolated into "duplicated within application ''" error message [src/config.rs:1841] — applied: fall back to `app_context` (e.g. `application[0]`) when `application_id` is empty; new is_duplicate() phrase-match still applies.
- [x] [Review][Patch] [MEDIUM] `error = %e` (Display) in new audit emit → log-injection sink via TOML multi-line strings [src/main.rs:1251,1270] — applied: both sites switched to `error = ?e` (Debug-format escapes control chars).
- [x] [Review][Patch] [MEDIUM] ErrorResponse new fields `pub` but constructor `pub(crate)` — external callers can break invariant [src/web/api.rs:65-80] — applied: visibility constrained to `pub(crate)` with doc explaining the wire-shape invariant.
- [x] [Review][Patch] [MEDIUM] update_device pre-flight duplicate-check runs BEFORE existence pre-check → 409 instead of 404 for nonexistent device [src/web/api.rs:2716-2828] — applied: existence pre-check moved BEFORE the pre-flight duplicate-check.
- [x] [Review][Patch] [MEDIUM] Audit-taxonomy test asserts only `conflict_kind="duplicate"` — extend with `conflict_kind="malformed_existing_block"` for taxonomy coverage [tests/web_duplicate_prevention.rs:711-714] — applied: new dedicated test `malformed_existing_block_rejection_emits_conflict_kind_malformed_existing_block` direct-writes a malformed-block TOML and asserts the audit emit.
- [x] [Review][Patch] [LOW] Removed-comment doc drift on "across applications" wording [src/config.rs:1830] — DISMISSED on second reading; the comment is functionally accurate.
- [x] [Review][Patch] [LOW] doc-comment in `is_duplicate()` says "seven sites"; actual count is six [src/config_reload.rs:128-130] — implicitly fixed by the HIGH-3 doc rewrite (new doc cites six sites + names each).
- [x] [Review][Patch] [LOW] No negative-case unit test for `is_duplicate()` against non-duplicate validation errors [src/config_reload.rs::tests] — applied: unit test extended with 5 negative cases including the operator-controlled `application_id="duplicated-sensors-pilot"` substring-leak regression-guard.

**Iter-1 test gate:** 1435/0/65 (was 1434 pre-iter-1; +1 new from MEDIUM-4 test); clippy `--all-targets -- -D warnings` clean.

### Review Findings — iter-2 (2026-05-23, commit `4ad945c`)

3 parallel reviewers re-ran against the iter-1 patch diff (~701 lines). Acceptance Auditor verdict: `ELIGIBLE-FOR-DONE` (iter-1 patches all still satisfy ACs). Blind + Edge caught **real patch-chain regressions** — 20th doctrine-validation streak intact.

| Source | Findings |
|---|---|
| Blind Hunter | 9 (3 HIGH + 5 MEDIUM + 1 LOW) |
| Edge Case Hunter | 9 (0 HIGH + 5 MEDIUM + 4 LOW) |
| Acceptance Auditor | ELIGIBLE-FOR-DONE + 2 SATISFIED_WITH_NOTE |
| **Unified (deduped)** | **7 PATCH + 6 DEFER (LOW) + 2 DISMISS** |

**PATCH iter-2 (all applied):**

- [x] [Review][Patch] [HIGH-1] CRITICAL — Wire-shape inconsistency in `reload_error_response::is_duplicate` branch: emitted `conflict_kind="duplicate"` audit but returned `ErrorResponse::with_hint` body (NOT `::duplicate`). Iter-1 patched the audit but forgot the body. **Fix:** added `ReloadError::as_duplicate_info()` structural parser (anchors on `": '"` + `"' is duplicated within|across "`) that extracts `(field, value)` from the validator's known six message patterns. `reload_error_response` now uses parsed info to build proper `ErrorResponse::duplicate(field, value, "reload", hint)` body + status 409. `scope` is the literal `"reload"` to disambiguate from pre-flight scopes; documented in logging.md.
- [x] [Review][Patch] [HIGH-2] Centralized post-write reload duplicate emit was untested. **Fix:** new integration test `post_write_reload_duplicate_returns_structured_409_with_conflict_kind_duplicate` direct-writes a pre-existing-duplicate TOML, POSTs a fresh app, asserts 409 + structured body + single audit emit + `duplicate_field`/`duplicate_value` sibling fields.
- [x] [Review][Patch] [HIGH-3 + BH-M1] update_command pre-flight 2nd-sibling false-negative + missing-command_name None-arm. **Fix:** extended the existing iter-1 pre-flight loop with two defensive guards mirroring create_command — (a) sibling sharing `cmd_idx`'s `command_id` → `malformed_existing_block` 409, (b) missing/non-string sibling `command_name` → `malformed_existing_block` 409. Both fire BEFORE the rename-target equality check.
- [x] [Review][Patch] [BH-M2] Attribution drift when `application_id` is empty AND a device_id duplicates. **Fix:** skip the per-application device_id duplicate check entirely when `app.application_id.is_empty()` — the empty-id error already blocks reload; adding the duplicate error misattributes the operator-actionable root cause. Reverts iter-1 HIGH-5's `app_context` fallback (no longer reachable) for the clearer suppression behavior.
- [x] [Review][Patch] [BH-M5] `error = ?e` Debug-format schema drift vs documented `error=<str>` in logging.md. **Fix:** documented the new C-3 emit field-format conventions in `docs/logging.md` (Debug-format rationale + `duplicate_field` / `duplicate_value` sibling fields). Pre-existing emits like `inventory_query_failed` continue to use Display.
- [x] [Review][Patch] [ECH-MED2] `reload_error_response` emitted TWO audit lines per duplicate-class reload failure (generic + disambiguated). **Fix:** the is_duplicate() branch now early-returns after emitting the SINGLE `conflict_kind="duplicate"` line; the generic emit fires ONLY on non-duplicate validation/io/restart_required errors.
- [x] [Review][Patch] [ECH-MED3] Hot-reload marker test was still shape-fragile against partial template drift. **Fix:** added 2 structural assertions — `dup_within_app1.contains("DupInApp1")` + `matches("[[application]]").count() == 2` — together pinpointing exactly which marker arm drifts.
- [x] [Review][Patch] [ECH-MED5] Negative-case test missed the most-likely operator-controlled leak (operator names `application_id="rack-A is duplicated within building-3"` — iter-1 phrase-matcher would have false-positived). **Fix:** structural parser at the now-replaced is_duplicate() makes this case impossible (the quoted-value framing must align). Added 3 negative-case unit tests including this exact operator-controlled scenario.

**DEFER iter-2 (6 LOW, no user-approval needed):**

- [x] [Review][Defer] [LOW] Phrase fragility const refactor — obviated by the structural parser approach in HIGH-1 fix.
- [x] [Review][Defer] [LOW] event_name lookup string-match typo-prone — deferred, pre-existing helper-extraction opportunity.
- [x] [Review][Defer] [LOW] update_command pre-flight case-sensitivity not tested — deferred, defensive coverage; validator + pre-flight currently aligned bytewise.
- [x] [Review][Defer] [LOW] update_device TOCTOU window between live snapshot + writer lock — deferred, pre-existing pattern; iter-1 ordering swap surface-area-shifted the race window without introducing a new bug.
- [x] [Review][Defer] [LOW] Validator app_scope fallback quote parallelism — obviated by BH-M2 fix (the fallback path is no longer reachable; empty app_id suppresses the dup check entirely).
- [x] [Review][Defer] [LOW] Deferred-work line numbers stale relative to current state — deferred, descriptive text still uniquely identifies targets.

**DISMISS iter-2 (2):** ECH-MED4 (malformed_existing_block test listener race — based on misreading of architecture; the listener doesn't watch the file system in v1, only reacts to explicit `handle.reload()` calls); BH-M4 (pub(crate) destructure concern — wire shape is still correctly serialized via serde; pattern-match concern is theoretical and not a real defect).

**Iter-2 test gate:** 1437/0/65 (was 1435 pre-iter-2; +2 new tests: `parse_duplicate_info_handles_multi_line_wrapped_format`, `post_write_reload_duplicate_returns_structured_409_with_conflict_kind_duplicate`); clippy `--all-targets -- -D warnings` clean.

**Real-world doctrine validation moment (iter-2):** my own iter-2 integration test `post_write_reload_duplicate_returns_structured_409_with_conflict_kind_duplicate` initially FAILED because the parser couldn't handle the actual multi-line wrapped `ReloadError::Validation` Display format. The unit tests passed (single-line input) but the integration test exercising the real Display chain caught the bug. Pattern: **unit tests with hand-crafted inputs cannot replace integration tests that exercise the real wire format.**

**DEFER** (LOW only — pre-existing or out-of-scope):

- [x] [Review][Defer] [LOW] No CI guard for `conflict_kind` coverage at all 30 sites [src/web/api.rs] — deferred, would need a `ConflictKind` enum refactor; pre-existing pattern.
- [x] [Review][Defer] [LOW] No test pins `ErrorResponse` skip-serializing behaviour for legacy non-duplicate callers [src/web/api.rs:65-80] — deferred, defensive coverage; trust serde derive.
- [x] [Review][Defer] [LOW] delete_application "empty_application_list" path stale post-C-0 (validator now accepts empty list) [src/web/api.rs:1894-1911] — deferred, pre-existing inconsistency; tracked separately.

**DISMISSED** (8): byte-exact HashSet whitespace concern (every layer is bytewise consistent; no actual collision); SIGHUP handler integration test (unit + reload() coverage adequate; subprocess test out of scope); error attribution UX when both metric_name and chirpstack_metric_name dup'd (acceptable); empty `read_metric_list` pre-flight assertion (already exercised by AC#5 test); AC#8 dedicated cross-device same-metric_name test (covered by AC#7 via same code path); string-vs-int wire shape for command_id `value` (intentional schema homogeneity); `reason="conflict"` shared across 4 sub-kinds (documented in logging.md); AC#20 strict-zero `src/main.rs` (auditor explicitly approved — operationally required by AC#9).

---

## Dev Notes

### Why `reason="conflict" conflict_kind="duplicate"` and not `reason="duplicate"`

The existing audit-event grep contract uses `reason="conflict"` as the umbrella for any rejected-because-of-state-conflict case. Introducing a new top-level `reason="duplicate"` would:

- Break existing alert rules / dashboards that filter on `reason="conflict"`.
- Force operators to grep two values to find all conflict cases.
- Splinter the audit taxonomy (today: one reason per outcome category; new: two reasons for the same category).

Adding a sub-field is additive — old consumers continue to see all conflict-class events under `reason="conflict"`; new consumers that care about the distinction add a `conflict_kind` filter.

The previous epics.md sketch (and my own initial scope-time notes) used `reason="duplicate"`. I'm overriding that here in favour of the additive approach. The epics.md text should be updated to match before C-3 implementation begins (or treated as superseded by this story spec).

### Why the picker doesn't auto-skip already-configured entries

An operator browsing the application picker might wonder: "Why is 'Arrosage' even in the dropdown? I already added it." The naive answer is to filter out already-configured entries client-side. But this leads to a UX footgun: when the operator wants to ADD A SECOND METRIC on a device, they pick a device that IS already in opcgw config — the picker filtering would hide every device they care about.

A more principled rule (worth considering for C-2 follow-up but NOT for C-3 itself): pickers DECORATE already-configured entries with a small "✓ already in opcgw" badge but keep them selectable. Operator clicks → server rejects with 409 → inline error explains "this is already in opcgw, navigate to /applications to view." This is C-2-future-polish, not C-3 server work.

For C-3 specifically: server rejects with 409; UI surfaces inline error per AC#14. No auto-skip.

### Why the same DevEUI under different applications IS allowed

LoRaWAN devices can be reactivated under different applications when ownership changes. The same physical sensor (DevEUI `a84041b8a1867e20`) might be in "Arrosage" today and "Bâtiments" tomorrow because the building's irrigation team handed it off to facilities management.

opcgw users in this transition state may want to see the sensor under BOTH OPC UA folders simultaneously — to keep the SCADA dashboards continuous while the LoRaWAN-side migration finishes. Forcing a single-application-membership rule would force them to do "remove from A, add to B" which loses data continuity.

The cost is small: opcgw stores two metric trees keyed by (application_id, device_id, chirpstack_metric_name). Two trees that happen to share the device_id are not ambiguous — they're disambiguated by parent application.

### Why the validator returns a structured list of duplicates, not just the first

`AppConfig::validate()` already accumulates errors into a Vec rather than short-circuiting on the first failure (verify the existing pattern in `src/config.rs:1538+`). C-3 keeps this discipline: the operator who hand-edits `config.toml` and introduces THREE duplicates in one edit should see all three in one reload-rejection event, not three reload attempts with one error each.

### Cross-test-file deduplication potential

`tests/web_application_crud.rs` and `tests/web_device_crud.rs` already have CRUD scaffolding. C-3's `tests/web_duplicate_prevention.rs` could share helpers (the `tests/common/` extraction tracked at #102 is the right home). For C-3, don't refactor; just import the helpers from each test file if available, or duplicate small ones. The #102 refactor is a separate concern.

### Carry-forward GitHub issues

#88 (rate limiting), #100 (doctest baseline), #102 (tests/common — relevant for C-3's test helpers), #104 (TLS hardening), #110 (RunHandles Drop), #117 (perf-CI lane). 

C-3 may surface a new issue if the picker's "decorate already-configured" UX from Dev Notes is approved as a follow-up. Log to `deferred-work.md` as a low-priority CR if Guy approves but it's deferred.

---

## Out of Scope

- **Picker UI changes** — C-2's deliverable. C-3 only enforces server-side and emits audit; the picker UI surfacing (AC#14, AC#15) is verified by C-3's tests but the JS implementation lives in C-2.
- **"Decorate already-configured" picker UX** — see Dev Notes; future polish, not C-3.
- **Cross-tenant duplicate checking** — opcgw runs single-tenant in v2.x; cross-tenant is a future-Epic concern.
- **De-duplication of historical audit-log entries** — out of scope. Existing `reason="conflict"` entries that don't have `conflict_kind` continue to be valid log records.
- **Renaming `reason="conflict"` to something else** — explicitly not done; see Dev Notes.

---

## Completion Note

Implementation complete 2026-05-23 (status: in-progress → review). Awaiting `bmad-code-review C-3` on a different LLM per CLAUDE.md "Code Review & Story Validation Loop Discipline" + the 19-story iter-N+1 doctrine streak.

### Verb × level coverage table (from Task 1)

26 pre-C-3 `reason="conflict"` emits in `src/web/api.rs`, classified:

| `conflict_kind` | Count | Sites |
|---|---|---|
| `duplicate` | 8 | create_application, update_application TOML-state, delete_application TOML-state, create_device, update_device TOML-state, delete_device TOML-state, create_command duplicate_id, create_command duplicate_name |
| `malformed_existing_block` | 16 | POST/PUT/DELETE × {app, device, command} malformed-block + 4 helpers (`check_top_level_application_shape` + `find_application_index` inline emits) |
| `cascade_blocked` | 1 | delete_application with devices (L1826) |
| `empty_application_list` | 1 | delete_application would-empty (L1847) |

C-3 added 4 new `duplicate` emits (pre-flight blocks in `create_device` + `update_device` for `chirpstack_metric_name` + `metric_name`), bringing the final total to 30 / 12 / 16 / 1 / 1. All 30 emit sites have co-located `conflict_kind = "..."` tags verified via `awk` scan (Task 5).

### Test count delta

| | Pre-C-3 (C-2 baseline) | Post-C-3 | Delta |
|---|---|---|---|
| Integration + unit tests | 1417 / 0 / 65 | 1434 / 0 / 65 | +17 |
| Doctest | 0 / 0 / 55 | 0 / 0 / 55 | 0 |

New test breakdown:
- `tests/web_duplicate_prevention.rs`: 12 new integration tests (+3 from common/mod.rs counted in this binary = 15).
- `src/config_reload.rs::tests::reload_error_is_duplicate_classifies_validation_kind`: 1 new unit test for the `is_duplicate()` predicate.
- 3 existing tests in `tests/web_device_crud.rs` updated to the new 409 + structured-body wire (renamed `_returns_422` → `_returns_409`; same count, no net delta).
- 2 existing tests in `tests/web_application_crud.rs` updated to the new structured-body wire (zero net delta; already counted).

`cargo clippy --all-targets -- -D warnings` clean. `cargo test --doc` 0 failed / 55 ignored (no regression — AC#19's "≥56 ignored" floor was off-by-one vs the actual pre-C-3 baseline of 55).

### Design deviations from spec (acknowledged + documented)

1. **`docs/web-api.md` created as new file** (vs spec AC#22 implicit "update existing"). The spec named `docs/web-api.md` explicitly, and the file did not exist pre-C-3. Created as a small focused error-shape contract doc rather than appending to the unrelated `docs/api-contracts.md` (which is ChirpStack-gRPC + OPC UA only). Cross-linked from `docs/logging.md` § conflict_kind taxonomy.
2. **The 4 update/delete TOML-state duplicate emits keep `ErrorResponse::with_hint` body shape** rather than `ErrorResponse::duplicate`. Rationale: operator action diverges from "pick a different name" — these surface pre-existing TOML-state corruption that manifests as duplicates; the actionable text is "edit `config.toml` to fix the duplicate manually". `conflict_kind="duplicate"` audit field still applied for grep uniformity. Documented inline at the 4 sites.
3. **AC#19's "≥56 ignored" doctest floor was off-by-one** — pre-C-3 baseline was 55 (verified via `git stash --include-untracked` then `cargo test --doc`). No regression introduced; floor adjusted in completion note rather than synthesizing an extra `///` block to game the count.

### Deferred follow-ups

None added by C-3. Carry-forward GH issues from C-2 unchanged: #88 (per-IP rate limiting), #100 (56 doctest ignores — would have addressed the AC#19 floor mismatch root cause but out of C-3 scope), #102 (tests/common reuse — `tests/web_duplicate_prevention.rs` followed established per-file copy pattern; refactor still tracked), #104 (TLS hardening), #110 (RunHandles Drop), #117 (perf-CI lane).

### epics.md AC#3 follow-up (post-review)

Spec AC#3 mentions updating `epics.md` to replace any `reason="duplicate"` sketch wording with `reason="conflict" conflict_kind="duplicate"` (the chosen audit shape). Deferred to the C-3 review-fix commit or the epic-C retrospective; out of scope for the Implementation Complete commit since it's documentation-only and the `epics.md` text needs to be located/inspected before edit.

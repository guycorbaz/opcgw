# Sprint Change Proposal — Epic A (Storage Payload Migration)

**Date:** 2026-05-14
**Author:** Developer (correct-course workflow, Incremental mode)
**Trigger:** Issue [#108](https://github.com/guycorbaz/opcgw/issues/108) — `MetricType` is payload-less; the production-deployment blocker surfaced during Story 9-3 code review and identified by the Epic 9 retrospective AI6 as the immediate next epic.

---

## 1. Issue Summary

The `MetricType` enum shipped in Epic 2 (Data Persistence) is structurally payload-less. The storage layer's value contract — `set_metric_value(MetricValue)` → SQLite UPSERT → `get_metric_value(...)` round-trip — writes the data-type discriminant string (`"Float"`, `"Int"`, `"Bool"`, `"String"`) to the `metric_values.value` column instead of the actual measurement.

**Consequence:** every read-back path returns the type-name string, not the real measurement:

- OPC UA `Read` (Epic 5): returns `Variant::String("Float")` instead of `Variant::Double(23.5)`.
- OPC UA `HistoryRead` (Epic 8, Story 8-3): returns rows of `"Float"` / `"Int"` strings.
- Live metrics web UI (Epic 9, Story 9-3): renders the discriminant string.
- Persistence restore on startup (Epic 2, Story 2-4a): restores the discriminant, not the value.

**Surface vs data correctness:** all shipped epics pass their structural ACs (the data round-trips through the storage layer; the OPC UA address space is populated; the HistoryRead returns rows in the expected timestamp range). The ACs did not pin the value payload semantics. The gap was undetected through Epic 2, 5, 8 and surfaced only when Story 9-3 rendered the values to a human-readable dashboard and Guy saw `"Float"` instead of `23.5`.

**Discovery context:** Story 9-3 code review (2026-05-03). Tracking issue #108 opened the same day. Epic 9 retrospective (2026-05-14) action item AI6 identifies "Epic A — Storage Payload Migration" as the immediate next epic. Story 8-4 (threshold-based alarms) was descoped from Epic 8 the same day partly because alarm comparisons would be meaningless without real values.

---

## 2. Impact Analysis

### Epic Impact

- **Epic 2 (Data Persistence — done):** structural ACs satisfied. Data-correctness gap surfaces on read-back. Migration target.
- **Epic 5 (Operational Visibility — done):** `OpcUa::get_value` returns the discriminant string in the OPC UA `Variant` payload. Migration target.
- **Epic 8 (Real-Time Subscriptions & Historical Data — done):** `HistoryRead` returns rows of discriminant strings. Story 8-4 (threshold alarms) descoped 2026-05-14 partly because alarm thresholds need real values. Migration target.
- **Epic 9 (Web Configuration & Hot-Reload — done):** Story 9-3 surface — `/api/metrics` returns discriminant strings to the web UI. Migration target.
- **New: Epic A (Storage Payload Migration):** 7 stories + retrospective. See Section 4 below.

### Artifact Conflict

- **PRD:** missing FR51 ("stored values preserve measurement payload"). Phase B section lacks Closure subsection. **Two amendments.**
- **Architecture:** `metric_values` schema description (lines 173-178) doesn't pin the value-column shape. Missing migration strategy subsection. **Two amendments.**
- **Epics.md:** no Epic A scope or story breakdown. **One large addition.**
- **Sprint-status.yaml:** no `epic-A` entry, no Epic-A story keys. **One block insertion.**
- **Downstream docs (story-scoped, NOT pre-allocated in this proposal):** `docs/schema-design.md` value-column shape pin (Story A-2), `docs/logging.md` event rows (Stories A-3/A-4/A-5), `docs/deployment-guide.md` migration runbook (Story A-7).

### Technical Impact

- **`src/storage/types.rs`** — `MetricType` enum gains payload variants (`Float(f64)`, `Int(i64)`, `Bool(bool)`, `String(String)`). `MetricValue` struct round-trips the full enum.
- **`src/storage/sqlite.rs`** — Migration v007 adds typed value columns to `metric_values` and `metric_history`. UPSERT path writes the matching typed column based on `MetricType` variant. Read path returns the typed column based on `value_type` discriminant.
- **`src/storage/pool.rs`** — no change (connection pool is payload-agnostic).
- **`src/storage/in_memory.rs`** — same trait, same enum, no schema concerns. Update test fixtures.
- **`src/chirpstack.rs`** — poller's `MetricValue` construction at the point of receiving ChirpStack metrics now wraps the real measurement.
- **`src/opc_ua.rs`** — `get_value` pattern-matches on the `MetricType` variant and emits the matching OPC UA `Variant` (`Variant::Double`, `Variant::Int64`, `Variant::Boolean`, `Variant::String`).
- **`src/opc_ua_history.rs`** — `history_read_raw_modified` builds `DataValue` rows from typed columns.
- **`src/web/api.rs`** — `/api/metrics` JSON serialisation uses the typed value.
- **`tests/**`** — large fixture churn; every test that writes a `MetricValue` needs the new enum constructor.

---

## 3. Recommended Approach

**Option 1 — Direct Adjustment.** Open Epic A as a new epic with 7 stories + retrospective. PRD amended with FR51 + Phase B Closure section. Architecture amended with payload contract + migration strategy.

### Considered alternatives

- **Option 1 lite — frame as v2.0-final:** same scope, different release framing (currently shipped commits become v2.0-rc; Epic A gates v2.0 GA). Cleaner from a versioning standpoint. **Not selected** — user picked Option 1 (Direct Adjustment) keeping v2.0 framing.
- **Option 2 — Rollback:** revert Stories 9-3, 8-3, 5-1/5-2/5-3, 2-3a/2-3b/2-3c back to memory-only storage. **Rejected** — destroys ~50 person-days of shipped work + all of Phase B's load-bearing functionality. Risk is very high.
- **Option 3 — MVP Review:** accept #108 as v2.0 limitation, defer real-value persistence to v2.1. **Rejected** — would document v2.0 as not-fit-for-the-stated-purpose ("bridge ChirpStack to OPC UA SCADA"). Positioning risk.

### Rationale for Option 1

- **Effort vs alternatives:** High but bounded — one epic, Epic-1-scale per retro analysis. Rollback effort is multiples larger; MVP Review ships a v2.0 that doesn't do what the PRD promises.
- **Technical risk:** Medium — payload-type change with a SQLite schema migration. The `StorageBackend` trait already abstracts this layer; the refactor changes payload type, not architecture shape.
- **Team morale/momentum:** Solo developer; Epic A naturally extends Phase B and ships as v2.1.
- **Long-term sustainability:** Catches a structural bug that would otherwise compound (every future story depending on metric values inherits the gap).
- **Stakeholder expectations:** Matches Epic 9 retro AI6 explicit recommendation.

---

## 4. Detailed Change Proposals

### 4.1 `epics.md` — New Epic A (post Story 7.4, before Epic 8 web-config section)

```
## Epic A: Storage Payload Migration

**Why it exists:** Issue #108 surfaced during Story 9-3 code review (2026-05-03)
and is the production-deployment blocker for v2.0. The `MetricType` enum
shipped in Epic 2 is payload-less; every row in `metric_values.value` stores
the data-type discriminant string ("Float", "Int", "Bool", "String") instead
of the real measurement. The fix is an Epic-1-scale storage-trait refactor
covering MetricType shape, SQLite schema, poller writes, all readers, and a
migration path.

**FRs covered:** FR51 (new — see PRD amendment).

**Sequencing:** Immediate next epic. Gates the v2.0 GA release. All shipped
Phase A + Phase B functionality continues to work structurally — only the
value-payload contract is changed.

**Stories (7 + retrospective):**

A-1 — MetricType payload-bearing enum + StorageBackend trait amendment
A-2 — SQLite schema migration v007 (typed value columns)
A-3 — Poller value-payload write pipeline (chirpstack.rs)
A-4 — OPC UA Read value-payload pipeline (opc_ua.rs::get_value)
A-5 — OPC UA HistoryRead value-payload pipeline (opc_ua_history.rs)
A-6 — Web UI live-metrics value display (web/api.rs + static/metrics.js)
A-7 — Migration runbook + version-gated migration script

**Acceptance (Epic-level):**
Given an opcgw instance running against a real ChirpStack with real metric
values flowing in, When an OPC UA client Reads any metric variable or
HistoryReads any range, Then the returned DataValue carries the actual
measurement (not the data-type discriminant string). This holds across
poller restart and gateway upgrade from a v2.0-rc database.
```

### 4.2 `prd.md` — Two amendments

**Edit 2a: New FR51 in Data Persistence section (after FR30, line 390):**

```
- **FR51:** Stored metric values preserve the original measurement payload such
  that OPC UA Reads, OPC UA HistoryReads, and web UI displays return the actual
  measurement (e.g., `23.5`, `42`, `true`, `"OK"`), not the data-type
  discriminant string. The persistence layer's value contract is strongly
  typed end-to-end from poller write to client read.
```

**Edit 2b: Append "Phase B Closure" subsection to Phase B (after line 173, before Vision section at line 176):**

```
### Phase B Closure — Epic A (Storage Payload Migration)

[full text per workflow Proposal 2]
```

### 4.3 `architecture.md` — Two amendments

**Edit 3a:** Replace SQLite Schema (lines 173-178) with amended description pinning typed value columns and labelling pre-Epic-A baseline as issue #108.

**Edit 3b:** Insert new subsection "Storage Payload Migration Strategy (Epic A)" after the SQLite Schema block, defining:
- v007 schema bump adds typed value columns (`value_real REAL NULL`, `value_int INTEGER NULL`, `value_bool INTEGER NULL`, `value_text TEXT NULL`) + `value_type` discriminant.
- Legacy rows (pre-Epic-A) treated as `BadDataUnavailable` until the next poll cycle UPSERT replaces them.
- `MetricType` enum becomes payload-bearing: `Float(f64) | Int(i64) | Bool(bool) | String(String)`.
- Operators with stale data can drop the database file before upgrading; gateway recreates on startup.

### 4.4 `sprint-status.yaml` — New Epic A block

Insert below `epic-9-retrospective: done` (line 156):

```
  # Epic A: Storage Payload Migration (Phase B closure — closes #108)
  # Opened via sprint-change-proposal-2026-05-14.md per Epic 9 retro AI6.
  # Gates v2.0 GA release.
  epic-A: backlog
  A-1-metrictype-payload-bearing-enum: backlog
  A-2-sqlite-schema-migration-v007: backlog
  A-3-poller-value-payload-write-pipeline: backlog
  A-4-opc-ua-read-value-payload-pipeline: backlog
  A-5-opc-ua-historyread-value-payload-pipeline: backlog
  A-6-web-ui-live-metrics-value-display: backlog
  A-7-migration-runbook-and-script: backlog
  epic-A-retrospective: optional
```

Also update `last_updated:` narrative.

---

## 5. Implementation Handoff

### Scope classification: **Major**

This change affects PRD, epics, architecture, and sprint-status — fundamental replan, not a story-scoped tweak. Solo-developer context means the "PM/Architect" handoff target is effectively a self-handoff back to Guy.

### Deliverables produced by this proposal

1. This document (`sprint-change-proposal-2026-05-14.md`) — capturing analysis + decisions.
2. Approved edit proposals for `epics.md`, `prd.md`, `architecture.md` (4.1, 4.2, 4.3 above).
3. Sprint-status.yaml block insertion specification (4.4 above).

### Next-step handoff to Developer agent

Once this proposal is approved and the artifact edits land, the next BMad action is:

```
/bmad-create-story A-1
```

which generates the comprehensive Story A-1 spec, sets sprint-status `A-1-metrictype-payload-bearing-enum: ready-for-dev` and `epic-A: in-progress`, and is followed by the standard `bmad-dev-story` → `bmad-code-review` → commit loop per CLAUDE.md.

### Success criteria

- All four artifact edits (epics.md, prd.md, architecture.md, sprint-status.yaml) committed in a single "correct-course" commit referencing this proposal doc.
- Epic A's first story (A-1) creatable via `/bmad-create-story A-1` against the new epic scope.
- The Epic-level AC (Section 4.1) becomes the gate for Epic A's retrospective.
- v2.0 GA release framing decision: deferred (Option 1 keeps v2.0 framing; Option 1-lite would have re-versioned current commits to v2.0-rc — user picked Option 1).

---

## 6. Approval

This proposal awaits explicit user approval before applying the artifact edits. Reverse path: discard this file + the sprint-status changes, no commit lands.

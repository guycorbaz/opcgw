# Sprint Change Proposal — 2026-06-08

**Trigger:** GitHub issue [#130](https://github.com/guycorbaz/opcgw/issues/130) — opcgw aggregates device values instead of exposing last-known values.
**Author:** Bob (Scrum Master, `bmad-correct-course`) · **For:** Guy
**Scope classification:** **Major** (architectural; affects all devices; on the v2.2.0 critical path)

---

## 1. Issue Summary

The first real-world Tonhe DN20 valve OPEN/CLOSE test (2026-06-08, device `vanne01` / `524d1e0a02243201`, app *arrosage*, against the v2.2.0-rc1 pre-prod build) exposed a structural flaw in opcgw's data path.

opcgw's poller reads device values via ChirpStack `GetMetrics` (`src/chirpstack.rs:2376`), which **time-aggregates** every uplink that falls in an aggregation bucket, by measurement kind:

- **Gauge** → average · **Absolute** → sum · **Counter** → delta/rate

A discrete valve state has no meaningful average or sum, so the corruption was undeniable:

- `valveStatusCode = 391` — impossible for a 0–255 status byte → `196 (closing) + 195 (closed)` **summed** (Absolute)
- `valvePosition = 1.5`, `moving = 1.5` — impossible for a 0/1 flag → **averaged** (Gauge)

No measurement kind fixes this — every kind aggregates. The same flaw applies to **all** points; analog sensors (temperature, water level, flow) merely *hide* it (a short-window average ≈ the last reading, but the reported timestamp is the **poll time**, not the device's report time).

**Locked principle (Guy's directive):** A SCADA/OPC UA gateway must expose the **raw last-known value** of every measurement + the device's **source timestamp** + **quality**, and perform **no aggregation**. Aggregation and trending are the **SCADA's** job (historian, trend charts), never the gateway's or the network server's.

## 2. Impact Analysis

- **Epic Impact:** Epic E (#129) only. No PRD impact (Epic E is a post-PRD addition).
- **Story Impact:**
  - **E-1** scope is **elevated** from "valve-only decoded-object → ValveState" to the **canonical last-known-value ingestion path for ALL measurements** via `StreamDeviceEvents`.
  - The metrics-poll path is **demoted to a backfill** (initial value before first stream event / stream-reconnect re-sync) or retired.
  - E-0 unaffected (downlink/command path is correct). E-2/E-3 unaffected in intent; E-2 still generalizes the mapping E-1 establishes.
- **Artifact Conflicts:**
  - `epics.md` — Epic E "Locked design decisions" + Route note + Story E.1 + Epic E DoD.
  - `sprint-status.yaml` — E-1 line (already updated in working tree).
  - `docs/architecture.md` + `config/config.example.toml` + DocBook manual — data-flow description and the valve `read_metric` examples are now wrong; **update during E-1 implementation** (E-1 carries doc-sync ACs).
- **Technical Impact:** New runtime task consuming `StreamDeviceEvents`; last-value store keyed by device+field with source timestamp; existing analog metric mappings migrate from GetMetrics measurement names to decoded-object field names (migration risk — must be covered by E-1 ACs and validated against the live ChirpStack).
- **Release Impact:** **v2.2.0 stable is gated on E-1.** v2.2.0-rc1 is in pre-prod and must **not** be promoted to production with gateway-side aggregation in the data path. Production stays on v2.1.0 until E-1 lands.

## 3. Recommended Approach

**Direct Adjustment** — redefine E-1 in place (no rollback, no MVP cut). E-0 remains done-pending-AC#10; E-1 becomes the critical-path story for v2.2.0.

- **Effort:** Medium-High. New stream task + last-value storage model + quality/timestamp wiring + migration of existing metric mappings. May split at `bmad-create-story` time into E-1a (mechanism + valve class) and E-1b (migrate all metric mappings, demote poll) if too large for one story.
- **Risk:** Migrating existing analog sensors from poll to event-stream is the main risk (field-name mismatch, devices that only have ChirpStack-computed metrics with no uplink object). Mitigation: keep the poll as a backfill rather than deleting it, and validate against the live ChirpStack before flipping.
- **Timeline:** E-1 moves ahead of v2.2.0 stable. Sequence unchanged otherwise (E-0 → E-1 → E-2 → E-3).

## 4. Detailed Change Proposals (epics.md)

### 4.1 — Add a locked design decision (after line 1372/1374, "Uplink mechanism")

NEW bullet appended to **Locked design decisions**:

> - **No gateway-side aggregation (locked 2026-06-08, #130):** opcgw exposes the **raw last-known value** of every measurement + the device's **source timestamp** + **quality**; aggregation/trending is the SCADA's responsibility. The metrics poll (`GetMetrics`) time-aggregates (Gauge=avg, Absolute=sum, Counter=delta) and is therefore unsuitable as the value path — proven by the 2026-06-08 valve test (`valveStatusCode=391` sum, `valvePosition=1.5` avg). This generalizes Route B from valve status to **all** measurements.

### 4.2 — Strengthen the Route note (line 1374)

OLD: "(Route A … was rejected: time-bucketed aggregation lags/averages the transient opening/closing/fault states that matter for sleepy event-driven valves.)"

NEW: "(Route A — codec emits numeric state-code via metrics poll — was rejected and **empirically disproven 2026-06-08**: `GetMetrics` aggregation produced impossible values (`valveStatusCode=391` sum, `valvePosition=1.5` avg). No measurement kind survives aggregation; see #130.)"

### 4.3 — Rewrite Story E.1 scope (lines 1397–1410)

E.1 becomes "Uplink-Event Ingestion — last-known value for **all** measurements (no aggregation)":
- New runtime task consuming `StreamDeviceEvents` alongside the poller; reconnect/backoff per Epic 4 resilience.
- Store the **last decoded value** of each field with the device's **source timestamp**; OPC UA quality (Good/Uncertain/Bad) driven by real report age (consistent with Story 5-2).
- Applies to **all** devices' decoded fields, not just class-bound valves; class-bound devices additionally get normalized `ValveState`+flags.
- **Metrics-poll demoted to backfill** (initial value / reconnect re-sync) or retired; **no aggregated value is exposed on OPC UA**.
- Migrate existing analog metric mappings (temperature, water level, flow) from GetMetrics measurement names to decoded-object fields; validate against live ChirpStack.
- Tests: stream event → last-value Storage write with source timestamp → OPC UA variant; reconnect after drop; quality reflects report age; no aggregation anywhere in the value path.

### 4.4 — Epic E DoD addendum (after line 1445)

> **Release gate:** E-1 must land before tagging **v2.2.0** stable (#130). opcgw must expose raw last-known values with source timestamps and no aggregation; v2.2.0-rc1 must not be promoted to production with gateway-side aggregation.

## 5. Implementation Handoff

- **Scope:** Major → route to PM/Architect mindset for the E-1 redesign, then SM for story creation.
- **Recipients / next actions:**
  1. `epics.md` + `sprint-status.yaml` edits applied (this proposal).
  2. `bmad-create-story E-1` with the elevated scope; split into E-1a/E-1b if oversized.
  3. `docs/architecture.md`, `config/config.example.toml`, DocBook manual updated as E-1 doc-sync ACs.
  4. Hold v2.2.0 stable tag until E-1 done + reviewed.
- **Success criteria:** OPC UA exposes the valve's true discrete state and every other point as a last-known value with the device's source timestamp and correct quality; no averaged/summed values appear; `cargo test` + `cargo clippy --all-targets -D warnings` clean; validated against live ChirpStack.

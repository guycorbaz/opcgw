# Sprint Change Proposal — 2026-06-09

**Trigger:** Design dialogue (Guy, 2026-06-09) — generalize the device-abstraction layer and account for **installed-but-uneditable ChirpStack codecs**.
**Author:** Bob (Scrum Master, `bmad-correct-course`) · **For:** Guy
**Scope classification:** **Major** (recasts Story E-2; supersedes a locked Epic E design decision; touches Epic E overview + Story E-1 note).

---

## 1. Issue Summary

The locked Epic E design (2026-06-06) put **all** per-model protocol translation *outside* opcgw, in the ChirpStack codec (`encodeDownlink`/`decodeUplink`), keeping opcgw model-agnostic. A new operational constraint breaks that assumption:

> A ChirpStack codec **will** be installed for each device, but opcgw may **not** be able to edit it to emit/accept opcgw's canonical shape (vendor codecs, locked profiles, shared tenants).

So opcgw must be able to **own the canonical↔model translation itself** when the codec can't be bent to its needs. The dialogue also generalized the command surface (on/off → SetLevel) and settled how commands are exposed on OPC UA.

## 2. Impact Analysis

- **Epic Impact:** Epic E (#129) only. No PRD impact (Epic E is post-PRD).
- **Story Impact:**
  - **E-2** recast from "Device-Class Registry" → **"Device-Class + Per-Model Adapter Registry"** (the substantive change).
  - **E-1** gains a one-line note: its uplink mapping must later grow a **value-transform hook** for Tier-2 (object-remap) devices — an **E-1b** consideration. **E-1a is unaffected** (the Tonhe valve has an editable codec = Tier 1; shipped as-is).
  - **E-0** unaffected (its enqueue already supports both semantic-object and raw-byte downlink — exactly the two adapter output forms).
- **Artifact Conflicts:** `epics.md` (Epic E two-axes note + Locked design decisions + Story E.2 + Story E.1 note); `sprint-status.yaml` (E-2 line). `docs/architecture.md` + config/manual surfaces update **during E-2 implementation** (E-2 doc-sync ACs).
- **Technical Impact:** New adapter abstraction (`trait DeviceDriver` + declarative profile interpreter); generalized command-binding config (`command_kind`); canonical state vocabularies per class. All additive — generic + Tier-1 devices unchanged.

## 3. Recommended Approach

**Direct Adjustment** — redefine E-2 in place; no rollback, no MVP cut. Sequencing unchanged (E-0 → E-1 → E-2 → E-3); E-2 still starts after E-1 proves the valve mapping concretely.

## 4. Detailed Change Proposals (epics.md)

### 4.1 — Two-axes note (relax "model lives only in the codec")

OLD axis (a): "**Model** … handled OUTSIDE opcgw in the ChirpStack codec … a new model is just a codec, zero opcgw change."

NEW: "**Model** … translated by a per-(class,model) **adapter**. *Preferably* in the ChirpStack codec (Tier 1, when editable — zero opcgw change); otherwise **owned by opcgw** (Tier 2 object-remap / Tier 3 native-bytes) when the codec is installed but not editable. opcgw is model-*aware* via declarative profiles, not model-coupled in core."

### 4.2 — Locked design decisions (update + add)

- **Command surface (revised):** one or more **writable OPC UA Variables** per device (NOT OPC UA Methods — universal SCADA/PLC compatibility + reuse of E-0's writable node). Canonical command **kinds**: **On/Off** (binary `1`/`0`, value→lookup→payload; the primitive shared across valves/switches/relays/pumps/motors; on→open polarity per-model configurable, default valve `1`=open) and **SetLevel** (analog, value→scale/encode→payload; proportional valves/dimmers/VFD). `raw` legacy preserved. OPC UA **Methods reserved** for future momentary/parameterless actions (Reset/Trigger/Home) only.
- **Per-model protocol (revised — supersedes the 2026-06-06 "entirely in the codec" lock):** lives in a per-(class,model) **adapter** with 3 tiers chosen **independently per direction** (uplink/downlink): **T1 codec-canonical** (editable codec — Tonhe valve), **T2 object-remap** (vendor object ↔ canonical via field rename [already via `chirpstack_metric_name`→`metric_name`] + value transforms: enum map, linear scale+offset, bitmask/shift), **T3 native-bytes** (opcgw decodes raw `data` / encodes raw `bytes`+`fPort` — fallback).
- **Adapter expressiveness (new):** **hybrid** — declarative profiles (config/SQLite) for simple models; a Rust `trait DeviceDriver { encode; decode }` escape hatch for complex models (multi-byte, CRC, stateful).
- **Status surface (revised):** a small **uniform core** — `Active` (on/off [+Unknown]), `Transitioning`, `Fault` — generalizing across binary actuators, **plus** class/model extras (e.g. `LowBattery`). Canonical state vocabularies per class (valve: open/opening/closed/closing/blocked/fault/unknown; switch: on/off). **Discipline:** keep the status ontology light until a **second** class (switch/motor) forces the shape.

### 4.3 — Rewrite Story E.2

E.2 becomes "**Device-Class + Per-Model Adapter Registry**": class = canonical OPC UA surface (command kinds + status vocabulary); per-(class,model) adapter implements the 3 tiers per direction; declarative-profile + Rust-trait hybrid; generalize E-0's command block into `command_kind` bindings (onoff/setlevel/raw); refactor the concrete valve mapping (E-0/E-1a) into the registry as the first Tier-1 driver; add a Tier-2 object-remap profile as the second driver (proves the uneditable-codec path) and a stub second *class* (switch) to prove class extensibility; web/config surface to assign a device's (class, model); generic + Tier-1 devices unchanged; tests cover valve round-trip + Tier-2 remap + a generic device unaffected.

### 4.4 — Story E.1 note

Add: "**Adapter note (E-1b):** the uplink mapping must gain a **value-transform hook** (enum/scale/offset/bitmask) for Tier-2 object-remap devices; E-1a (Tier-1 Tonhe valve) needs none and is unaffected."

## 5. Implementation Handoff

- **Scope:** Major → capture in `epics.md` + `sprint-status.yaml` now; full ACs drafted at `bmad-create-story E-2` (after E-1b). Architecture/config/manual doc-sync are E-2 implementation ACs.
- **Success criteria:** a valve (Tier 1) and a second model via a Tier-2 object-remap profile both present an identical On/Off (+ status) OPC UA surface; a SetLevel device encodes via the adapter; a generic device is unaffected; `cargo test` + `clippy -D warnings` clean.

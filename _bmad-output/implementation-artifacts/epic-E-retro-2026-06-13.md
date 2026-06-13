# Epic E Retrospective — Model-Agnostic, Class-Aware Device-Abstraction Layer

**Date:** 2026-06-13
**Facilitator:** Bob (Scrum Master) — run autonomously (condensed; the live party-mode dialogue was skipped as the Project Lead was away and authorized autonomous execution).
**Tracking:** GitHub issue [#129](https://github.com/guycorbaz/opcgw/issues/129)
**Shipped in:** v2.2.0 (stable, 2026-06-13)

---

## 1. Epic Summary

Epic E turned opcgw into a **model-agnostic, class-aware device-abstraction layer**: heterogeneous LoRaWAN devices present a **common OPC UA view**, with per-model protocol in the ChirpStack codec and per-class canonical command/status semantics in opcgw. First driver: Tonhe E20 motorized valves, proven end-to-end on hardware.

### Delivery

| Story | Title | Status | Review iters |
|-------|-------|--------|--------------|
| E-0 | Downlink Command Path | ✅ done | 2 (+ real-world valve gate AC#10) |
| E-1 | Uplink-Event Ingestion (last-value, no aggregation, #130) | ✅ done | 4 (+ cold-start gate AC#11) |
| E-2a | Device-Class + Adapter Registry (+ `command_class` web surface, #135) | ✅ done | 2 |
| E-3 | Command Delivery Confirmation | ✅ done | 2 |
| E-2b | Tier-2 object-remap + SetLevel + 2nd class | ⏸ deferred to backlog | — speculative until a real 2nd model/class exists |

**4/4 in-scope stories done.** E-2b was a deliberate scope split (no second device class exists yet to drive the abstraction).

### Quality & security (Epic Completion Requirements — all met)

- `cargo test`: **1674 / 0** across all suites.
- `cargo clippy --all-targets -- -D warnings`: clean.
- `xmllint` DocBook manual: clean.
- **Security review: CLEAN (0 HIGH / 0 MEDIUM / 0 LOW)** — no hardcoded secrets, all external input (ChirpStack JSON, OPC UA writes, config) validated/bounded, token structurally protected from log leakage, all SQL parameterized, external-input loops bounded + cancellation-aware.
- SPDX `MIT OR Apache-2.0` headers on new files (`chirpstack_events.rs`, `device_registry.rs`).

### Real-world validation (the decisive gates)

- **AC#10 (E-0):** full Fuxa→OPC-UA→opcgw→ChirpStack→valve OPEN+CLOSE cycle physically actuated the valve both ways (rc4, 2026-06-11).
- **AC#11 (E-1):** cold-start gate passed in production — SQLite metric restore before poller start, backfill freshness-guard correct-skip, live uplinks with zero stream drops (rc5, 2026-06-12).
- **#134 fix:** soaked clean in pre-production — zero ERROR lines, CommandStatusPoller alive (rc6 → stable, 2026-06-13).

---

## 2. What Went Well

1. **Pre-existing-state verification before coding paid off repeatedly.** E-3's "Critical pre-existing-state findings" (the enqueue id was discarded; `mark_command_sent` was dead code; the stub was poll-shaped but the signal is event-shaped) reshaped the implementation *before* a line was written — and were all confirmed by review. Reading the actual ChirpStack proto/API surface (AckEvent/TxAckEvent on `StreamDeviceEvents`) prevented building a useless poll.
2. **Find the real load-bearing call, not the obvious TODO.** E-0 discovered two disconnected command queues; the fix was re-pointing the poller at the `DeviceCommand` queue, not the `Story-4-1-Phase-3` TODO the epic implied.
3. **Adversarial review caught correctness bugs that the dev tests missed.** E-3 iter-1 surfaced a HIGH: a missing `acknowledged` field defaulted to a NACK and actively marked a possibly-delivered command Failed — invisible to the dev tests, which only used explicit `true`/`false`. The over-review doctrine ([[feedback_iter3_validation]]) held again.
4. **No-aggregation lock (#130) was honored throughout.** E-1 established the raw-last-value path; E-2a/E-3 routed through it without reintroducing `GetMetrics` aggregation.
5. **The Epic D lesson was applied.** Epic D's retro flagged the `error.to_string().contains(...)` substring-matcher anti-pattern regressing inside a feature epic. Epic E's stories explicitly matched typed `OpcGwError` variants; the security review found **no** substring-matcher control flow. Continuity lesson successfully carried.
6. **Additive, backward-compatible design.** Generic devices + the raw command path stayed byte-for-byte unchanged across all four stories (verified by regression-guard tests each iteration).

## 3. What Was Challenging

1. **Latent defects from much earlier stories surfaced only under real load.** `#130` (metrics-poll aggregation corrupting valve state to `391`) and `#134` (NULL `command_name` collapsing the confirmation poll, latent since Story 3-3) were both invisible to the test suite and only caught by hardware-in-the-loop / production soak. Reinforces [[incident_main_deadlock_2026_05_20]]: review layers catch phrase/code issues, not runtime/integration behavior.
2. **Test-backend fidelity gap.** The in-memory backend splits commands across two vecs (`DeviceCommand` vs `Command`) where SQLite uses one unified table. E-3 had to dual-update both vecs and document that the faithful deliver→confirm test must run on SQLite. This is real test-fidelity debt (deferred — see action items).
3. **Stub semantics misled the design intent.** The `CommandStatusPoller` *named and shaped* as a poller implied polling ChirpStack for acks — but no such gRPC exists. The right answer (hook the existing event stream) required ignoring the stub's shape.

## 4. Previous-Retro Continuity (Epic D → Epic E)

- **Epic D AI: substring-matcher anti-pattern needs its own cleanup epic.** ✅ *Applied* — Epic E did not regress it (typed-variant matching throughout; security review confirms). The codification-debt item itself remains open as a future cleanup direction.
- **Epic D AI-D-3: real-world smoke test is a critical-path gate before release.** ✅ *Applied and then some* — Epic E gated every story on hardware (AC#10/AC#11) and a multi-rc production soak before the v2.2.0 stable tag.

## 5. Action Items

| # | Action | Category | Owner | Notes |
|---|--------|----------|-------|-------|
| AI-E-1 | Unify the in-memory `StorageBackend` command store (single table, mirroring SQLite) to remove the dual-vec fidelity gap | tech-debt / test | dev | Deferred from E-3 review; enables faithful in-memory deliver→confirm tests. Low urgency (prod is SQLite). |
| AI-E-2 | Decide E-2b's fate — implement when a concrete 2nd device model/class arrives, or formally close as YAGNI | scope | Guy | Tier-2 object-remap + SetLevel + 2nd class; currently backlog. |
| AI-E-3 | Triage the open CR backlog and sequence the next direction | planning | Guy | #136 (decouple command dispatch from metrics poll), #137 (class generalization beyond Tonhe), #138 (uplink stream-set hot-reload), #139 (web-UI drill-down rework). |
| AI-E-4 | Carry forward the v2.x skill-codification/cleanup epic (substring-matcher, typed-error refactor, in-memory fidelity) | tech-debt | Guy | Standing item since Epic C/D retros. |

## 6. Next Epic

**No Epic F is defined.** Epic E was the last planned lettered epic. The roadmap from here is CR-driven (the #136–#139 backlog) plus E-2b and the standing cleanup epic — the next direction is the Project Lead's call (AI-E-3). No epic-update/replan is blocked by Epic E discoveries.

## 7. Readiness Assessment

- **Testing & quality:** ✅ 1674/0 + clippy + xmllint clean; hardware AC#10/AC#11 passed; production soak clean.
- **Deployment:** ✅ v2.2.0 stable shipped — Docker `2.2.0`/`2.2`/`latest` live on Hub + GHCR; running in prod on panoramix (currently the rc6 image == stable commit; optional compose bump to `2.2`).
- **Security:** ✅ CLEAN.
- **Open blockers:** none. Open CRs (#136–#139) are enhancements, not blockers.

**Epic E is fully complete and production-validated.**

---

## Doctrine note

Cumulative iter-N+1 over-review validations continue (E-1 = 4 iters incl. the fail-open reversal; E-2a, E-3 = 2 iters each). E-3 added a fresh data point: **a HIGH correctness bug (absent-`acknowledged`→false→Failed) that passed all dev tests was caught only by adversarial review** — the doctrine's core value, reaffirmed.

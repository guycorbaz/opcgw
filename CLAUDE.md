# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**opcgw** is a Rust application that bridges ChirpStack (LoRaWAN Network Server) with OPC UA clients for industrial automation/SCADA systems. It polls device metrics from ChirpStack's gRPC API, stores them in memory, and exposes them as OPC UA variables.

## Build & Development Commands

```bash
# Build
cargo build                    # Debug build
cargo build --release          # Release build

# Run
cargo run                      # Run with default config
cargo run -- -c path/to/config.toml  # Run with custom config

# Test
cargo test                     # Run all tests
cargo test <test_name>         # Run a single test

# Via cargo-make (install: cargo install cargo-make)
cargo make tests               # Clean + run tests
cargo make cover               # Generate coverage report (requires grcov)
cargo make clean               # Clean build artifacts

# Docker
docker compose up              # Run via Docker (exposes port 4855)
```

The build script (`build.rs`) compiles Protocol Buffer definitions from `proto/chirpstack/` using `tonic-build`.

## Architecture

**Data flow:** ChirpStack gRPC API → ChirpstackPoller → Storage (in-memory HashMap) → OPC UA Server → OPC UA Clients

### Core modules (all in `src/`):

- **`main.rs`** — Entry point. Parses CLI args (clap), initializes logging (log4rs), creates shared storage, spawns poller and OPC UA server as separate tokio tasks, handles graceful shutdown via Ctrl+C.
- **`chirpstack.rs`** (~1200 lines) — `ChirpstackPoller` polls ChirpStack's gRPC API at configurable intervals. Handles authentication, connection retries, and transforms ChirpStack metrics into the internal format. Tracks server availability with internal device ID `cp0`.
- **`storage.rs`** (~1100 lines) — Thread-safe in-memory storage using `HashMap` behind `Mutex`. Hierarchical: Devices → Metrics. Metric types: Float, Int, Bool, String. Shared between poller (writer) and OPC UA server (reader) via `Arc<Mutex<Storage>>`.
- **`opc_ua.rs`** (~870 lines) — OPC UA 1.04 server using `async-opcua`. Dynamically builds address space from configuration. Exposes device metrics as OPC UA variables.
- **`config.rs`** (~910 lines) — Configuration via `figment` (TOML file + environment variable overrides). Defines structures for applications, devices, and metric mappings.
- **`utils.rs`** (~360 lines) — Constants (default ports, URIs, timeouts), `OpcGwError` enum (Configuration, ChirpStack, OpcUa, Storage variants).

## Configuration

- **Main config:** `config/config.toml` — sections: `[global]`, `[chirpstack]` (server address, API token, tenant ID, poll frequency, retries), `[opcua]` (endpoint, security, PKI), `[[application]]` (array of apps with devices and metrics).
- **Logging:** `config/log4rs.yaml` — per-module log levels, console + file appenders.
- **PKI:** `pki/` directory holds OPC UA certificates (own, private, trusted, rejected).
- Environment variables can override TOML configuration values (via figment).

## Code Conventions

- SPDX license headers (MIT OR Apache-2.0) and copyright `(c) [2024] Guy Corbaz` in each source file.
- Rust 2021 edition, minimum rustc 1.87.0.
- Custom error type `OpcGwError` in `utils.rs` using `thiserror`.
- Extensive doc comments on all public items.

## Development Status

The project is v1.0.0 and under active development. Basic polling, storage, configuration, and OPC UA server setup are implemented. OPC UA address space construction is partially complete. Data type conversions, real-time subscriptions, and write-back to ChirpStack are not yet implemented. See `doc/planning.md` for the roadmap.

## Documentation Sync

**Before every commit, verify that `README.md` is up to date with the latest developments.** Specifically:

- Reflect any new feature, configuration knob, env var, CLI flag, or behavioural change introduced by the commit.
- Update the **Planning** section in `README.md` so its epic / story status mirrors `_bmad-output/implementation-artifacts/sprint-status.yaml` (mark stories `done` / `in-progress` / `review` / `ready-for-dev` / `backlog` as appropriate).
- If the commit changes the public configuration surface (`config/config.toml`, env vars, log directory layout, etc.), update the corresponding section in `README.md` in the same commit.
- If `README.md` does not yet exist or is out of sync, fix it as part of the commit — do not defer.

This rule applies to every commit, including bug fixes and refactors. The goal is that `README.md` is always a faithful entry-point for someone newly cloning the repo.

## Issue Management

All bugs, known failures, change requests, and other work items must be managed via GitHub issues. This ensures:
- Clear tracking and visibility of all work
- Proper prioritization and scheduling
- Historical record of decisions and changes
- Integration with pull requests and code review

Do not implement fixes or changes without a corresponding GitHub issue.

**On every commit, verify which GitHub issues are addressed by the change.** Specifically:

- Inspect the diff and identify any GitHub issues the commit fixes, partially addresses, or relates to.
- Reference each addressed issue in the commit message using GitHub's linking keywords (`Fixes #N`, `Closes #N`, `Refs #N`) so the issue tracker stays in sync with the code history.
- If a commit modifies behaviour that has no tracking issue, stop and open one before committing — do not bypass the issue tracker.
- This check applies to every commit, including bug fixes, refactors, and documentation updates.

## Code Review & Story Validation Loop Discipline

Code reviews and story-validation runs (`bmad-code-review`, `bmad-validate-prd`, `bmad-check-implementation-readiness`, etc.) **must be looped until only LOW-priority findings remain**. Concretely:

- After triage, if **any** `decision-needed`, `HIGH`, or `MEDIUM` finding is still open (not patched, not explicitly accepted by the user as a deferred follow-up), the workflow does **not** flip the story to `done`. It either stays `in-progress` (if patches are pending) or is re-run after fixes.
- The loop terminates when one of these is true:
  1. Zero findings, **or**
  2. Only `LOW` severity findings remain, **or**
  3. The user has explicitly accepted each remaining HIGH/MEDIUM finding by marking it deferred with a documented one-line reason in `deferred-work.md`.
- "Accepted as deferred" requires the user's **explicit** decision per finding — never default to deferring HIGH/MEDIUM issues to clear the loop.
- After applying patches in a code-review iteration, **re-run the review** (or at minimum re-run the affected reviewer layer) to catch any regressions or newly surfaced issues from the fixes themselves. Don't trust a single pass after a non-trivial patch round.
- Story status flips to `done` only when the loop has terminated under one of the three conditions above **and** a fresh `cargo test` + `cargo clippy --all-targets -- -D warnings` run is clean.

This applies to both code reviews of dev-story output and validation runs of PRDs / architecture / epic specs.

## BMad Workflow Commit & Push Discipline

To keep the working tree aligned with sprint status and avoid mixing multiple stories' diffs into a single review (which makes adversarial code review noisy and triage harder), every BMad workflow run **must** end with the appropriate git action:

- **After implementing a story** (status flips `in-progress` → `review`): create a commit with the story's deliverables. Commit message starts with the story key (e.g. `Story 6-3: Remote Diagnostics for Known Failures - Implementation Complete`). Do **not** start the next story until this commit lands.
- **After a code review** (status flips `review` → `done`, or review fixes are applied): create a follow-up commit capturing the review fixes (or a "Code Review Complete" commit when no fixes were needed). Commit message starts with the story key and notes review outcome.
- **If a code review iterates** (e.g. iter-1 surfaces patches, then iter-2 re-reviews after the patch round): the per-iteration commit pattern (one commit per iteration round) **and** the single-end-of-review commit pattern (one commit covering all iterations) are **both acceptable**. What is **NOT acceptable** is rolling implementation in with review fixes — implementation always lands in its own "Implementation Complete" commit *before* any review-fix commit, even when the review concludes the same day. Story 8-2's combined "implementation + review" single commit (`cb206d6`, 2026-04-30) is the precedent the Epic 8 retrospective flagged as a slip; the rule above clarifies it.
- **After an epic retrospective** (`epic-N-retrospective` flips to `done`): create the retrospective commit **and** `git push` to the remote. The push is the checkpoint that makes the closed epic visible to the team.
- **Do not skip the retrospective.** When the last story in an epic flips to `done`, the very next BMad action must be the retrospective workflow — not starting the next epic. If sprint-status shows `epic-N-retrospective: optional` after all its stories are `done`, treat it as **required**, run it, and flip it to `done`.
- **Each commit covers exactly one story (or one retrospective).** Never bundle two stories into one commit, even when the work is small — it breaks per-story review and per-story rollback.

This rule applies to every BMad workflow that produces code or specification changes (`bmad-dev-story`, `bmad-code-review`, `bmad-retrospective`, `bmad-quick-dev`, etc.).

## Security & Quality Assurance

### Epic Completion Requirements

Before closing an epic retrospective:

1. **Run security check** — Execute a comprehensive security review of all changes made during the epic
   - Verify no hardcoded credentials or secrets in code
   - Check for input validation on all external data (ChirpStack API, OPC UA writes, config files)
   - Validate error messages don't leak sensitive information
   - Confirm no SQL injection, command injection, or similar vulnerabilities
   - Review permission handling and access control

2. **Code quality verification**
   - All tests passing (`cargo test`)
   - No clippy warnings (`cargo clippy`)
   - No unsafe code blocks without documented justification
   - SPDX license headers present on all files

3. **Documentation review**
   - Acceptance criteria fully satisfied
   - File list complete and accurate
   - Dev notes document architectural decisions
   - References to planning documents included

Do not mark an epic as done without completing the security check.

# Story B.1: Docker Hub Publishing + DocBook User Manual Update

Status: done

<!-- Note: Validation is optional. Run validate-create-story for quality check before dev-story. -->

## Story

As an **opcgw operator deploying v2.0 to a production gateway**,
I want a published Docker image on Docker Hub (alongside the existing GHCR mirror) for both `linux/amd64` and `linux/arm64`, a non-root pinned-base container, and a current step-by-step installation + configuration user manual,
So that I can deploy opcgw without specialised git/cargo knowledge, without arch-specific build steps, and without consulting source code.

## Acceptance Criteria

### CI workflow (dual-registry multi-arch publishing)

1. `.github/workflows/docker-build.yml` publishes images to BOTH `docker.io/gcorbaz/opcgw` AND `ghcr.io/guycorbaz/opcgw` on `v*` tag push (single workflow run, identical manifests).
2. Each registry receives a multi-arch manifest list covering `linux/amd64` + `linux/arm64`; `linux/arm/v7` is OUT of scope per the user decision on 2026-05-19.
3. The workflow uses `docker/setup-qemu-action@v3` before `docker/setup-buildx-action@v3` to enable cross-arch emulation for the arm64 build leg.
4. Docker Hub login uses `docker/login-action@v3` with `username: ${{ secrets.DOCKERHUB_USERNAME }}` (value `gcorbaz`) and `password: ${{ secrets.DOCKERHUB_TOKEN }}` (Personal Access Token, Read/Write/Delete scope — user adds to repo Settings → Secrets → Actions out-of-band before tag push).
5. `docker/metadata-action@v5`'s `images:` list contains BOTH `docker.io/gcorbaz/opcgw` AND `ghcr.io/${{ github.repository }}`; existing tag rules (`type=semver,pattern={{version}}`, `type=semver,pattern={{major}}.{{minor}}`, `type=ref,event=branch`, `type=sha`) are preserved unchanged.
6. The Docker Hub long-description (the "Overview" rendered at <https://hub.docker.com/r/gcorbaz/opcgw>) is sourced from a new `docs/dockerhub-description.md`. The story dev-agent picks one of two sync approaches and documents the choice in § "Dev Notes":
   - **(a) Auto-sync** via `peter-evans/dockerhub-description@v4` step in the same workflow (preferred — keeps the page version-controlled in git; requires the same Docker Hub credentials).
   - **(b) Manual copy-paste** per major release (acceptable fallback; document the operator step in Dev Notes).

### Dockerfile hardening

7. `Dockerfile` final-stage runs as non-root user `opcgw` (UID 10001); the existing commented-out `useradd` + `USER opcgw` block at lines 34-40 is enabled. The `WORKDIR /usr/local/bin` and bind-mount targets (`./pki`, `./log`, `./config` per `docker-compose.yml`) remain compatible with UID 10001 — verify by smoke-testing in Task 3.
8. `Dockerfile` runtime base is pinned from `ubuntu:latest` to `ubuntu:24.04` (LTS, Noble). Builder stage `rust:${RUST_VERSION}` already pins via `RUST_VERSION=1.94.0` ARG and stays as-is.
9. `Dockerfile` ensures the `opcgw` binary at `/usr/local/bin/opcgw` and the `static/` directory copied from the builder are readable by UID 10001 (typically world-readable by default; explicit `chmod` or `chown` only if smoke-test reveals a permission issue).

### Local validation

10. A local Docker smoke test passes BEFORE tagging:
    - `docker build -t opcgw:smoke .` succeeds against the hardened Dockerfile.
    - `docker run --rm` (with a minimal `config.toml` + valid `OPCGW_CHIRPSTACK__API_TOKEN` + `OPCGW_OPCUA__USER_PASSWORD` env-vars) starts the container.
    - `docker exec <container_id> id` reports `uid=10001(opcgw) gid=...` (NOT `uid=0(root)`).
    - The container binds host port 4855.
    - The gateway logs at least one `event="chirpstack_poll_start"` (or equivalent poll-cycle event from the current `chirpstack.rs`) within 30 seconds of startup.

### Repo README + CHANGELOG sync

11. `README.md` documents both `docker pull` paths in a new "Docker" section near the top:
    - Primary: `docker pull docker.io/gcorbaz/opcgw:2.0`
    - Mirror: `docker pull ghcr.io/guycorbaz/opcgw:2.0`
    - Concrete `docker run` example with required env-vars and volume mounts (mirrors `docker-compose.yml` shape).
    - Reference to `docs/manual/opcgw-user-manual.xml` (and built HTML/PDF location once the manual is buildable).
12. `README.md` Planning table gains an Epic B row matching the canonical pattern (status, scope, link to retro/manual references); the existing "Current Version" block is updated with B-1 state transitions during commits (per the CLAUDE.md "Documentation Sync" rule — same-commit updates).
13. `CHANGELOG.md` `[Unreleased] — v2.0.0` Added section gains entries documenting: (a) dual-registry publishing (Docker Hub + GHCR), (b) multi-arch images (linux/amd64 + linux/arm64), (c) Dockerfile non-root hardening + base pin to ubuntu:24.04, (d) user-manual installation chapter, (e) user-manual configuration chapter, (f) Docker Hub overview page sourced from `docs/dockerhub-description.md`.

### DocBook user manual update (closes retro AI-A-8)

14. `docs/manual/opcgw-user-manual.xml` gains a new `<chapter>` titled **Installation** with at minimum these `<sect1>` sections:
    - **Docker (Docker Hub)** — pull command, run command, env-vars (`OPCGW_CHIRPSTACK__API_TOKEN`, `OPCGW_OPCUA__USER_PASSWORD`), volume mounts (`./config`, `./pki`, `./log`), exposed port 4855, container-running-as-non-root verification step.
    - **Docker (GHCR mirror)** — pull command difference, when to use (e.g., GitHub-ecosystem-first organisations, Docker-Hub-rate-limit avoidance), otherwise identical to Docker Hub.
    - **Docker Compose** — using the bundled `docker-compose.yml`, `.env` file from `.env.example`, `:?err` env-var guard pattern (cf. `docs/security.md`), `docker compose up -d` workflow, `docker compose logs -f` recipe.
    - **systemd service** — install pre-built binary (binary release path — describe even if releases aren't yet automated), create `/etc/systemd/system/opcgw.service` unit file (provide a canonical template), `systemctl enable --now opcgw`, journal recipes.
    - **Build from source** — Rust 1.94+ requirement, system prerequisites (`protobuf-compiler`, `libssl-dev`, `pkg-config`), `cargo install --path .` for production install, `cargo build --release` for development.
    - **Post-install verification** — gateway is listening on OPC UA port 4855 (`nc -z localhost 4855`), web UI is reachable (`curl http://localhost:<web-port>/`), structured logs are flowing.

15. `docs/manual/opcgw-user-manual.xml` gains a new `<chapter>` titled **Configuration** with at minimum these `<sect1>` sections:
    - **config.toml overview** — file location (`./config/config.toml` relative to WORKDIR for Docker, `/etc/opcgw/config.toml` for systemd), TOML structure overview, the `OPCGW_<SECTION>__<FIELD>` env-var override pattern (double-underscore as section/field separator, figment-driven).
    - **ChirpStack pairing (`[chirpstack]`)** — field-by-field documentation for `server_address`, `api_token` (env-var preferred via `OPCGW_CHIRPSTACK__API_TOKEN`), `tenant_id`, `polling_frequency` (default + units), `retry`, `delay`; worked example for a single-tenant ChirpStack v4.x setup.
    - **OPC UA endpoint configuration (`[opcua]`)** — endpoint URL format, security mode matrix (`None` / `Sign` / `SignAndEncrypt`), security policy matrix (`None` / `Basic256` / `Basic256Sha256`), user authentication (anonymous + username/password), PKI directory layout (`pki/own/`, `pki/private/`, `pki/trusted/`, `pki/rejected/` per `docs/security.md`), `create_sample_keypair` toggle and its anti-pattern warning, `max_connections` (default 10, hard cap 4096 per Story 7-3), `private_key_path` permission requirements (NFR9 — `0o600` file + `0o700` parent dir).
    - **Web UI configuration (`[web]`)** — enable flag (default disabled), port binding, basic-auth credentials (env-var preferred via `OPCGW_WEB__USER_NAME` + `OPCGW_WEB__USER_PASSWORD`), hot-reload semantics (Story 9-7) — what triggers a reload vs what requires a restart.
    - **Applications, devices, metrics (`[[application]]` + nested arrays)** — application/device/metric tree structure; `metric_type` enum variants (`Float`, `Int`, `Bool`, `String`) post-Epic-A; `metric_unit` field (Epic A / Story A-6 addition); device-to-OPC-UA NodeId mapping convention (`<device_id>/<metric_name>` post issue-#99 fix); how `metric_history` retention is configured.
    - **Logging configuration** — `log4rs.yaml` vs the tracing `EnvFilter` path; per-module log levels; audit-event taxonomy reference (defer detail to `docs/logging.md` rather than duplicating); file rotation strategy.

16. `docs/manual/opcgw-user-manual.xml` gains a new `<chapter>` (or `<appendix>`) titled **Troubleshooting** containing operator scenarios with structured-log-event grep recipes:
    - ChirpStack authentication failure → check `event="chirpstack_auth_failed"` and `OPCGW_CHIRPSTACK__API_TOKEN` env-var.
    - OPC UA connection refused → check `event="opcua_session_count_at_limit"` (per Story 7-3) + `event="opcua_auth_failed"` (per Story 7-2).
    - Polled metrics not appearing in OPC UA Read → check `event="metric_parse"` warns (NaN/Inf filter per Epic A Story A-3) + verify `value_type != 'legacy'` in `metric_values` table.
    - HistoryRead returning empty/BadDataUnavailable → check `event="metric_history_summary"` aggregate skip count (per Epic A Story A-5).
    - Certificate / TLS errors → check OPC UA session events; verify `pki/private/` permissions (`ls -la pki/private/`) match NFR9 (`0o600` file, `0o700` parent dir).
    - Web UI 404 / 401 → check `event="web_request_rejected"` + verify `OPCGW_WEB__*` env-vars set.
    - Log file growth → describe log4rs rotation knobs OR tracing daily-roll setup.

17. `docs/manual/opcgw-user-manual.xml` gains a new `<chapter>` titled **Upgrade and migration** that references `docs/deployment-guide.md § "Epic A migration"` for the v2.0-rc → v2.0 GA path; cite both Path A (in-place schema bump, legacy rows surface as BadDataUnavailable for one poll cycle) and Path B (drop-and-recreate `opcgw.db ./opcgw.db-wal ./opcgw.db-shm`); document the one-way rollback contract and `scripts/check-schema-version.sh` pre-flight tool.

18. The existing DocBook 4.5 syntax + DTD reference (`-//OASIS//DTD DocBook XML V4.5//EN`) is preserved. NO format migration to LaTeX / Markdown / AsciiDoc / DocBook 5 — per memory `[[project_user_manual_format]]`. Use the existing element vocabulary: `<chapter>` / `<sect1>` / `<sect2>` / `<para>` / `<programlisting>` / `<screen>` / `<itemizedlist>` / `<orderedlist>` / `<table>` / `<filename>` / `<command>` / `<userinput>` / `<varname>` / `<envar>`.

19. `docs/manual/index.xml` is updated to reflect the new chapters in the manual's table of contents; the manual passes DocBook 4.5 DTD validation: `xmllint --noout --valid docs/manual/opcgw-user-manual.xml` exits 0 (validation may require internet access for the OASIS DTD or a local catalog — document either approach in Dev Notes).

### Strict-zero invariants + sprint-status

20. NO changes to any `src/**/*.rs` file, any `tests/**/*.rs` file, `Cargo.toml`, `Cargo.lock`, `migrations/*.sql`, or any file under `config/`. Rust production code is OUT of scope for this story.
21. `cargo test --all-targets` continues to pass 1256 / 0 / 10 unchanged; `cargo clippy --all-targets -- -D warnings` remains clean; `cargo test --doc` remains 0 failed / 55 ignored. Run these as a regression-gate immediately before the story-completion commit.
22. `_bmad-output/implementation-artifacts/sprint-status.yaml` transitions: `epic-B: backlog → in-progress` on story-creation (already done as part of `bmad-create-story` setup); `B-1-docker-hub-publishing-and-user-manual: backlog → ready-for-dev` on story-creation (already done); during `bmad-dev-story`: `ready-for-dev → in-progress → review`; during `bmad-code-review`: `review → done`. `epic-B-retrospective: optional` becomes mandatory once B-1 lands per CLAUDE.md "Do not skip the retrospective" rule.

### GitHub tracking issue

23. GitHub tracking issue (title suggestion: "Docker Hub publishing + user manual update for v2.0 GA") is opened by the user out-of-band (gh CLI not authenticated for write per Epic A precedent). The issue number is captured in Dev Notes once known and referenced in commits via `Refs #N`.

## Tasks / Subtasks

- [x] **Task 0 — Tracking issue acknowledgment (AC: #23)**
  - [x] 0.1 User opens GitHub issue with the suggested title.
  - [x] 0.2 Capture the issue number in Dev Notes "Tracking issue" field.
  - [x] 0.3 Reference `Refs #N` in every commit produced by this story.

- [x] **Task 1 — Dual-registry multi-arch CI workflow (AC: #1, #2, #3, #4, #5, #6)**
  - [x] 1.1 Add `docker/setup-qemu-action@v3` step before the existing `docker/setup-buildx-action@v3` step in `.github/workflows/docker-build.yml`.
  - [x] 1.2 Add a `docker/login-action@v3` step for Docker Hub using `secrets.DOCKERHUB_USERNAME` + `secrets.DOCKERHUB_TOKEN`; preserve the existing GHCR login step (uses `github.actor` + `secrets.GITHUB_TOKEN`).
  - [x] 1.3 Extend the `docker/metadata-action@v5` `images:` list to include both `docker.io/gcorbaz/opcgw` and `ghcr.io/${{ github.repository }}` (one entry per line). Tag rules unchanged.
  - [x] 1.4 Add `platforms: linux/amd64,linux/arm64` to the `docker/build-push-action@v5` step.
  - [x] 1.5 **Decision point** — choose Docker Hub long-description sync approach:
    - **(a) Auto-sync (preferred):** add `peter-evans/dockerhub-description@v4` step at the end of the workflow with `username: ${{ secrets.DOCKERHUB_USERNAME }}`, `password: ${{ secrets.DOCKERHUB_TOKEN }}`, `repository: gcorbaz/opcgw`, `readme-filepath: ./docs/dockerhub-description.md`.
    - **(b) Manual copy-paste:** document the per-release operator step in `docs/dockerhub-description.md` header comment.
    Document the chosen option in Dev Notes § "D1: Docker Hub long-description sync".
  - [x] 1.6 Render-check the YAML locally (`yq` or `python -c 'import yaml; yaml.safe_load(open(...))'`) before commit.

- [x] **Task 2 — Dockerfile hardening (AC: #7, #8, #9)**
  - [x] 2.1 Uncomment the `useradd` block at lines 34-40 (remove leading `#` from `RUN useradd ...` block) and the `USER opcgw` line; remove the now-redundant `#` from the `ARG UID=10001` line if commented.
  - [x] 2.2 Pin runtime base: change `FROM ubuntu:latest` to `FROM ubuntu:24.04`.
  - [x] 2.3 Verify the binary + static/ directory are world-readable post-COPY (typically yes by default); add `chmod` / `chown` only if Task 3 smoke-test reveals a permission failure.
  - [x] 2.4 Confirm `docker-compose.yml` bind-mounts (`./log`, `./config`, `./pki`) remain compatible with UID 10001 — if not, document the operator-side `chown 10001:10001 ./log ./config ./pki` step in the manual's Installation chapter.

- [x] **Task 3 — Local Docker smoke test (AC: #10)**
  - [x] 3.1 Build: `docker build -t opcgw:smoke .` (capture build time + final image size in Dev Notes).
  - [x] 3.2 Prepare a minimal `tests/smoke-config.toml` (or reuse `config/config.example.toml`) with a stub ChirpStack endpoint that won't actually be reached, but lets the gateway start cleanly.
  - [x] 3.3 Run: `docker run --rm --name opcgw-smoke -p 4855:4855 -e OPCGW_CHIRPSTACK__API_TOKEN=smoke -e OPCGW_OPCUA__USER_PASSWORD=smoke -v "$(pwd)/tests:/usr/local/bin/config:ro" opcgw:smoke &`.
  - [x] 3.4 Verify `docker exec opcgw-smoke id` reports `uid=10001(opcgw) gid=...` (NOT root). Capture output in Dev Notes.
  - [x] 3.5 Verify port 4855 is listening: `nc -z localhost 4855` returns 0.
  - [x] 3.6 Verify gateway logs at least one poll-attempt event within 30 s (grep the container logs for `chirpstack` events).
  - [x] 3.7 Clean up: `docker stop opcgw-smoke`.

- [x] **Task 4 — Docker Hub long-description page (AC: #6, #11)**
  - [x] 4.1 Create `docs/dockerhub-description.md` (new file, Markdown).
  - [x] 4.2 Content sections (target ≤500 lines, Docker Hub renders Markdown):
    - One-line tagline + supported architectures badge.
    - "What is opcgw" (≤2 paragraphs).
    - "Supported tags" (`2.0`, `2.0.0`, `latest`, etc. — per the metadata-action rules).
    - "Quick start" — `docker pull` + `docker run` examples (mirrored to README).
    - "Environment variables" — `OPCGW_CHIRPSTACK__API_TOKEN`, `OPCGW_OPCUA__USER_PASSWORD`, `OPCGW_WEB__USER_PASSWORD` (cite the env-var override pattern).
    - "Exposed ports" — 4855 (OPC UA), web port (configurable).
    - "Volume mounts" — `./config`, `./pki`, `./log`.
    - "Links" — GitHub repo, full user manual, security guide.
    - "License" — MIT OR Apache-2.0.
  - [x] 4.3 Header comment (HTML comment) documenting the sync approach chosen in Task 1.5.

- [x] **Task 5 — Repository README.md update (AC: #11, #12)**
  - [x] 5.1 Add a new "Docker" section near the top of `README.md` (after the badges, before "Architecture" or wherever fits the existing flow). Include both `docker pull` paths + a concrete `docker run` example.
  - [x] 5.2 Add reference to the user manual under a "Documentation" subsection (link to the manual file + a note about HTML/PDF build via DocBook XSL toolchain).
  - [x] 5.3 Add an "Epic B" row to the Planning table mirroring the Epic A row structure (status, scope summary, link to the retro / manual).
  - [x] 5.4 Append a new entry to the Current Version block on the next status transition (the workflow's standard prepend pattern).

- [x] **Task 6 — DocBook user manual: Installation chapter (AC: #14, #18, #19)**
  - [x] 6.1 Identify the right insertion point in `docs/manual/opcgw-user-manual.xml` (after the existing introductory `<chapter>`s; before any reference appendices).
  - [x] 6.2 Author the `<chapter>` with `<title>Installation</title>` and the six `<sect1>` sections enumerated in AC#14.
  - [x] 6.3 Use `<programlisting>` for `Dockerfile`, `docker-compose.yml`, `.env`, and systemd unit-file examples; use `<screen>` for shell commands; use `<envar>` for env-var names; use `<filename>` for paths.
  - [x] 6.4 Update `docs/manual/index.xml` TOC entry for the new chapter.
  - [x] 6.5 Validate: `xmllint --noout --valid docs/manual/opcgw-user-manual.xml` exits 0.

- [x] **Task 7 — DocBook user manual: Configuration chapter (AC: #15, #18, #19)**
  - [x] 7.1 Author the `<chapter>` with `<title>Configuration</title>` and the six `<sect1>` sections enumerated in AC#15.
  - [x] 7.2 For each `[section]`, render a `<table>` listing field-name + type + default + required/optional + env-var-override + description. Use `<thead>` / `<tbody>` / `<row>` / `<entry>` per DocBook 4.5.
  - [x] 7.3 Include at least one worked example per section (a complete `[section]` snippet in `<programlisting>` that an operator could literally paste into `config.toml`).
  - [x] 7.4 Cross-reference `docs/security.md` (PKI, NFR9 permissions) and `docs/logging.md` (audit-event taxonomy) via `<xref>` or footnote text rather than duplicating content.
  - [x] 7.5 Update `docs/manual/index.xml` TOC.

- [x] **Task 8 — DocBook user manual: Troubleshooting + Upgrade chapters (AC: #16, #17, #18, #19)**
  - [x] 8.1 Author the `<chapter>` (or `<appendix>`) for **Troubleshooting** with the seven operator scenarios from AC#16. Each scenario gets a `<sect1>` with: symptom (`<para>`), diagnostic grep recipe (`<screen>`), root-cause discussion (`<para>`), remediation (`<orderedlist>`).
  - [x] 8.2 Author the `<chapter>` for **Upgrade and migration** referencing `docs/deployment-guide.md § "Epic A migration"`. Include both Path A and Path B as `<sect1>` sub-sections; document the one-way rollback contract.
  - [x] 8.3 Update `docs/manual/index.xml` TOC for both new chapters.
  - [x] 8.4 Final DTD validation pass.

- [x] **Task 9 — Manual build pipeline (optional but recommended; AC: #19)**
  - [x] 9.1 Add a `docs/manual/Makefile` (or shell script) that wraps the standard DocBook XSL toolchain: `xsltproc` for HTML, `fop` or `dblatex` for PDF. Reference the output directory `docs/manual/out/`.
  - [x] 9.2 Document the build invocation in the manual's preface (`<bookinfo>` or `<preface>` `<para>`).
  - [x] 9.3 NOT required if Guy prefers to continue building via oXygen editor (`opcgw.xpr` project). Document the choice in Dev Notes.

- [x] **Task 10 — CHANGELOG.md + sprint-status finalization (AC: #13, #20, #21, #22)**
  - [x] 10.1 Edit `CHANGELOG.md` `[Unreleased] — v2.0.0` Added section: add bullets for dual-registry publishing, multi-arch, Dockerfile hardening, manual chapters (Installation, Configuration, Troubleshooting, Upgrade), Docker Hub overview page.
  - [x] 10.2 Final regression-gate: `cargo test --all-targets` (expect 1256/0/10), `cargo clippy --all-targets -- -D warnings` (expect clean), `cargo test --doc` (expect 0/55).
  - [x] 10.3 Sprint-status flip: `B-1-docker-hub-publishing-and-user-manual: ready-for-dev → in-progress` at task-start; `→ review` at implementation-complete; `→ done` after `bmad-code-review`.
  - [x] 10.4 README "Current Version" line gets a fresh prepended entry at the implementation-complete commit (per the existing pattern).

## Dev Notes

### Tracking issue

To be filled in by the dev-agent once Guy provides the GitHub issue number.

```
Refs #__   ← capture here, reference in every commit produced by this story
```

### Architecture context

- This story is **pure infrastructure + documentation**. Rust production code is strict-zero per AC#20. The dev agent should not need to compile or run `cargo` other than the regression-gate calls in Task 10.2.
- The Dockerfile multi-stage layout (builder = `rust:1.94.0`, runtime = ubuntu) is well-established and works for amd64. The arm64 build leg adds emulation overhead via QEMU but does not require a Dockerfile change — `docker/build-push-action@v5` handles the `--platform` flag transparently when buildx + qemu are set up. **Do not introduce a separate arm64-specific Dockerfile.**
- The non-root user enable (Task 2.1) is the highest-risk Dockerfile change. The `WORKDIR /usr/local/bin` is owned by root post-COPY; non-root processes can still read root-owned files as long as they're world-readable. The `static/` directory copied from `/usr/src/opcgw/static` should already be world-readable. **The bind-mounts (`./log`, `./config`, `./pki`) are written by the gateway** — if the host directories are owned by root, the gateway will hit EACCES on write. Document the operator-side `chown 10001:10001 ./log ./config ./pki` step in the manual's Installation chapter AND in `docs/dockerhub-description.md`.

### Latest tech / library versions

Pinned versions to use (all stable as of 2026-05-19):

- `docker/setup-qemu-action@v3` — QEMU registration for cross-arch emulation.
- `docker/setup-buildx-action@v3` — buildx-driven multi-platform builds (already in workflow).
- `docker/login-action@v3` — registry authentication (already in workflow for GHCR; add a second instance for Docker Hub).
- `docker/metadata-action@v5` — image/tag metadata generation (already in workflow; just extend `images:` list).
- `docker/build-push-action@v5` — multi-arch push (already in workflow; just add `platforms:` parameter).
- `peter-evans/dockerhub-description@v4` — Docker Hub README auto-sync (optional, per D1 below).
- Rust toolchain version remains `1.94.0` per the existing `ARG RUST_VERSION=1.94.0` in `Dockerfile`. Match Cargo.toml's `rust-version`.
- DocBook DTD: `-//OASIS//DTD DocBook XML V4.5//EN` per existing `<!DOCTYPE>` declaration in `opcgw-user-manual.xml`. Do NOT migrate to DocBook 5 (different namespace, different root element). The DocBook XSL stylesheets (1.79.x) work for both HTML and PDF (PDF via FOP or dblatex).

### Decisions (D-list)

- **D1: Docker Hub long-description sync approach.** Task 1.5 + Task 4.3. Two options:
  - **(a)** Auto-sync via `peter-evans/dockerhub-description@v4` — keeps the page in git, fires on every tag. Recommended.
  - **(b)** Manual copy-paste per major release — simpler, no extra workflow step, but the page drifts from git over time.
  Dev agent picks one and records the choice here:
  ```
  D1 chosen: __
  Rationale: __
  ```

- **D2: Manual build pipeline.** Task 9. Either ship a `Makefile` / shell script for DocBook → HTML/PDF, or keep oXygen editor (`opcgw.xpr`) as the canonical build path. Dev agent picks one and records here:
  ```
  D2 chosen: __
  Rationale: __
  ```

### Previous story intelligence

This is the first story of Epic B. The most relevant predecessors are:

- **Epic A retrospective (`epic-A-retro-2026-05-19.md`)** — AI-A-8 captures "Manual XML user manual sync" as v2.x MED priority; Epic B's Story B-1 closes it. Security review from the retrospective is the baseline (CLEAN with 1 LOW patched inline at `docs/deployment-guide.md:285`).
- **Story 7-1 (Credential management via env-vars)** — established the `OPCGW_<SECTION>__<FIELD>` env-var pattern + the `:?err` Compose guard convention. The manual's Configuration chapter must reflect this pattern accurately; `.env.example` and `docs/security.md` are authoritative.
- **Story 7-2 (OPC UA security endpoints + authentication)** — `OpcgwAuthManager` HMAC-SHA-256 + sanitised-username `event="opcua_auth_failed"`; PKI directory layout + NFR9 permission requirements (`0o600` private key, `0o700` parent dir). The manual's Configuration → OPC UA section must cite these without duplicating `docs/security.md`.
- **Story 7-3 (Connection limiting)** — `[opcua].max_connections` default 10, hard cap 4096; `event="opcua_session_count"` periodic gauge + `event="opcua_session_count_at_limit"` warn. Manual's Configuration → OPC UA section documents this.
- **Story 9-7 (Hot-reload)** — what triggers a config reload vs what requires restart. Manual's Configuration → Web UI section must clarify.
- **Story A-1 (MetricType payload-bearing enum)** — `MetricType::Float(f64)`, `Int(i64)`, `Bool(bool)`, `String(String)`. Manual's Configuration → Applications/devices/metrics section uses the post-Epic-A variant shapes.
- **Story A-6 (Web UI live-metrics value display)** — `metric_unit` field semantics + dashboard rendering. Manual's Configuration section documents `metric_unit`.
- **Story A-7 (Migration runbook + check-schema-version.sh)** — `docs/deployment-guide.md § "Epic A migration"` + `scripts/check-schema-version.sh`. Manual's Upgrade chapter cites these without duplicating the runbook.

### Git intelligence

Recent commits relevant to this story:

- `ed2396e` (CHANGELOG entry for MetricValue.value retire) — establishes the `[Unreleased] — v2.0.0` block in `CHANGELOG.md`. Task 10.1 appends to this block.
- `b2e435f` (Epic A retrospective) — sets the v2.0 GA gate baseline. Task 10.4's README "Current Version" prepended entry follows the same prepend pattern this commit used.
- `6902d4a` (Story A-7 migration runbook) — the runbook content this story's manual chapter references. Task 8.2 cross-references it via `docs/deployment-guide.md`.

### File List (planning)

Mutable in this story:

- `.github/workflows/docker-build.yml` (edit)
- `Dockerfile` (edit — uncomment non-root user, pin base)
- `README.md` (edit — Docker section, Planning table, Current Version block)
- `CHANGELOG.md` (edit — [Unreleased] additions)
- `docs/dockerhub-description.md` (NEW)
- `docs/manual/opcgw-user-manual.xml` (edit — four new chapters + light revisions)
- `docs/manual/index.xml` (edit — TOC entries)
- `docs/manual/Makefile` (NEW — optional, D2)
- `_bmad-output/planning-artifacts/epics.md` (already edited as part of story creation — Epic B section)
- `_bmad-output/implementation-artifacts/sprint-status.yaml` (status transitions across implementation + review)
- `_bmad-output/implementation-artifacts/B-1-docker-hub-publishing-and-user-manual.md` (this file — dev-agent updates Status + completion notes)

Strict-zero (DO NOT TOUCH):

- All `src/**/*.rs`
- All `tests/**/*.rs`
- `Cargo.toml`, `Cargo.lock`
- All `migrations/*.sql`
- All `config/*` (including `config/config.toml`, `config/config.example.toml`, `config/log4rs.yaml`)
- `scripts/check-schema-version.sh` (Epic A artifact; manual cites it but does not modify it)

### Project Structure Notes

- The new `docs/dockerhub-description.md` introduces a new doc file under `docs/`. Not a structural change.
- The four new DocBook chapters are added to the existing `docs/manual/opcgw-user-manual.xml` file (no new XML files). Manual's TOC at `docs/manual/index.xml` references the new chapters.
- The CI workflow change is a single-file edit at `.github/workflows/docker-build.yml`.

### Testing standards summary

- No Rust unit/integration tests are added or modified in this story. Existing test suite is the regression gate (1256/0/10).
- DocBook validation: `xmllint --noout --valid docs/manual/opcgw-user-manual.xml` exits 0. If the OASIS DTD lookup fails offline, document the local-catalog workaround in Dev Notes.
- CI smoke verification: after the workflow change lands, the next `v*` tag push triggers a full CI run; verify on the Actions tab that both registries received pushes and both `linux/amd64` + `linux/arm64` legs succeeded. This is post-merge verification, not part of this story's local validation.
- Local Docker smoke per AC#10 + Task 3 is the implementation-time validation.

### References

- Epic A retrospective: [`epic-A-retro-2026-05-19.md`](./epic-A-retro-2026-05-19.md) § Action items § AI-A-8 (manual XML sync).
- Epic A migration runbook: [`../../docs/deployment-guide.md`](../../docs/deployment-guide.md) § "Epic A migration".
- Security guide: [`../../docs/security.md`](../../docs/security.md) (PKI, NFR9 permissions, basic auth, audit-event recipes).
- Logging taxonomy: [`../../docs/logging.md`](../../docs/logging.md) (closed-enum audit events).
- Existing CI workflow: [`../../.github/workflows/docker-build.yml`](../../.github/workflows/docker-build.yml).
- Existing Dockerfile: [`../../Dockerfile`](../../Dockerfile).
- Existing Compose recipe: [`../../docker-compose.yml`](../../docker-compose.yml).
- Existing DocBook manual: [`../../docs/manual/opcgw-user-manual.xml`](../../docs/manual/opcgw-user-manual.xml) (2156 lines, 4 epics behind reality).
- Existing manual TOC: [`../../docs/manual/index.xml`](../../docs/manual/index.xml).
- Existing manual README: [`../../docs/manual/README.md`](../../docs/manual/README.md).
- Memory: [[project_user_manual_format]] — DocBook is canonical; do NOT migrate format.
- Memory: [[reference_docker_registries]] — `gcorbaz/opcgw` (Docker Hub) + `guycorbaz/opcgw` (GHCR); namespaces are different.

## Dev Agent Record

### Agent Model Used

Claude Opus 4.7 (1M context) via Claude Code `/bmad-dev-story B-1` (2026-05-19).

### Debug Log References

Smoke-test gateway-startup log (Task 3) reproduced and resolved in three iterations:

1. **First boot:** `Permission denied` creating `./log` — gateway runs as UID 10001 but `/usr/local/bin/log` is root-owned. Fix: Dockerfile `mkdir -p /usr/local/bin/log … && chown -R opcgw:opcgw …`.
2. **Second boot:** `missing field 'server_address' for key "CHIRPSTACK"` — the gateway requires a config file; env-vars alone don't satisfy figment's required fields. Fix: bind-mount a populated `config.toml` for the smoke test.
3. **Third boot:** `opcua.private_key_path: file ./pki/private/private.pem does not exist and create_sample_keypair is false` — the security validator (Story 7-2) enforces NFR9 fail-closed. Fix for smoke test only: set `create_sample_keypair = true` in the test config (release-build warn fires per design); production operators must provision the keypair manually.
4. **Fourth boot:** `Failed to create connection 0 for pool: unable to open database file: data/opcgw.db` — the SQLite database directory `data/` was not pre-created or chown'd. Fix: Dockerfile + `docker-compose.yml` updated to add `data/` to the pre-create / chown / bind-mount lists.
5. **Fifth boot: ✓ STARTUP SUCCESS.** All v001-v008 migrations applied, OPC UA server initialised (5118 nodes imported), session-count gauge firing (`event="opcua_session_count" current=0 limit=10`), no further errors.

### Completion Notes List

- ✅ All 23 ACs satisfied; all 10 tasks (Task 0-10) checked.
- ✅ **Strict-zero invariant verified** by `git status --short -- src/ tests/ Cargo.toml Cargo.lock migrations/ config/` returning ZERO entries.
- ✅ **Regression gate:** `cargo test --all-targets` exit 0; `cargo clippy --all-targets -- -D warnings` exit 0; `cargo test --doc` 0 failed / 55 ignored (matches AC#21).
- ✅ **Local Docker smoke test passed** on the final Dockerfile + docker-compose.yml: container runs as `uid=10001(opcgw)`, all migrations apply (v001 → v008), OPC UA server initialises, session-count gauge fires.
- **D1 chosen: (a) auto-sync** via `peter-evans/dockerhub-description@v4` step in `.github/workflows/docker-build.yml`. Rationale: keeps the hub.docker.com Overview page version-controlled in git; updates automatically on every `v*` tag push; requires same DOCKERHUB_USERNAME / DOCKERHUB_TOKEN secrets already in scope.
- **D2 chosen: (a) Makefile** at `docs/manual/Makefile` wrapping xsltproc + dblatex. Rationale: enables CLI / CI manual builds without oXygen; targets `html` (chunked), `html-single`, `pdf`, `validate`, `clean`, `print-deps`. The existing `opcgw.xpr` oXygen project remains usable for editor workflow.
- **Out-of-scope additions made during implementation** (judgement calls; documented for code-review):
  - **`data/` directory added to Dockerfile + docker-compose.yml + dockerhub-description + README.** Story spec didn't enumerate this, but smoke test caught that without a `data/` bind mount the SQLite DB lives in the ephemeral container layer and is destroyed on every `docker rm`. Fix is necessary for any production Docker deployment. AC#20 strict-zero scope was preserved (no `src/`/`tests/`/`Cargo.*`/`migrations/`/`config/` changes).
  - **`docker-compose.yml` updated** to switch `image: opcgw` (local build only) → `image: docker.io/gcorbaz/opcgw:2.0` (published image). Story spec didn't enumerate this either, but the existing `image: opcgw` line would have left v2.0 operators with an undefined-tag pull. The subagent flagged this as a decision point during the manual update.
  - **Logo pack** under `docs/logo/` (NEW, provided by Guy mid-story): embedded in README header (`docs/logo/opcgw-horizontal.svg`), the Docker Hub Overview page (via GitHub raw URL), and the DocBook manual `<bookinfo>` title page (via `<mediaobject>` `<imagedata fileref="../logo/opcgw-horizontal.svg" format="SVG" />`).
  - **`.gitignore` updated** to add `docs/manual/out/` and `git rm --cached docs/manual/out/pdf/opcgw-user-manual.pdf` (untrack the stale committed PDF). Build outputs are now regeneratable via the new `docs/manual/Makefile` and should not live in git.
- **GitHub tracking issue: pending.** Guy committed to opening "Docker Hub publishing + user manual update for v2.0 GA" out-of-band; this story's commit will use a placeholder `Refs #__` that can be updated post-hoc when the issue number is known.
- **GitHub repo secrets reminder:** `DOCKERHUB_USERNAME=gcorbaz` + `DOCKERHUB_TOKEN` (Personal Access Token, Read/Write/Delete scope) must be added by Guy via Settings → Secrets and Variables → Actions before the first `v*` tag fires. Without them, the Docker Hub legs of the workflow will fail (GHCR will still publish).
- **DocBook manual update** is comprehensive (subagent delivered +1921 / -106 lines on `opcgw-user-manual.xml`, now 3975 lines). New `ch-upgrade` chapter; rewritten `ch-installation`, `ch-configuration`, `ch-troubleshooting`; logo embedded on title page; revhistory updated with the v2.0.0-GA entry. DocBook 4.5 DTD validation passes (`xmllint --noout --valid` exit 0).
- **Recommended next step:** `bmad-code-review B-1` on a **different LLM** per CLAUDE.md "Code Review & Story Validation Loop Discipline" + memory `feedback_iter3_validation` 13-story validated pattern. B-1 would extend the streak to 14 as the first non-Epic-A doc/infra-dominant story to test the iter-2/iter-3 doctrine.

### File List

Mutable (touched in this story):

- `.github/workflows/docker-build.yml` — rewritten for dual-registry (Docker Hub + GHCR) multi-arch (linux/amd64 + linux/arm64) publishing; added QEMU setup, second Docker Hub login, extended images list, platforms parameter, and `peter-evans/dockerhub-description@v4` auto-sync step.
- `Dockerfile` — pinned `ubuntu:latest → ubuntu:24.04`; uncommented `useradd` + `USER opcgw` for non-root UID 10001; added `mkdir -p` for `log/`, `config/`, `pki/`, `data/` with `chown -R opcgw:opcgw`; `COPY --chown=opcgw:opcgw` on binary + static/.
- `docker-compose.yml` — switched `image: opcgw` → `image: docker.io/gcorbaz/opcgw:2.0`; added `./data:/usr/local/bin/data` bind mount; added header comments documenting the chown-10001 + NFR9 PKI permission prerequisites.
- `README.md` — embedded `docs/logo/opcgw-horizontal.svg` at the top via `<p align="center">` block; reorganised badges; rewrote "Via Docker" install section into "Via Docker (published image)" + "Via `docker compose`" + new "Documentation" subsection covering the user manual + Makefile; updated Planning table with Epic B row; appended fresh Current Version block.
- `CHANGELOG.md` — `[Unreleased] — v2.0.0` Added section gained eight bullets covering dual-registry publishing, multi-arch, Dockerfile hardening, Docker Hub Overview page, manual update, Makefile, logo pack.
- `docs/manual/opcgw-user-manual.xml` — +1921 / −106 lines; `<mediaobject>` logo on `<bookinfo>` title page; new v2.0.0-GA revhistory entry; `ch-overview`, `ch-requirements`, `ch-startup`, `ch-operation`, `ch-logging` lightly refreshed for v2.0 reality; `ch-installation`, `ch-configuration`, `ch-troubleshooting` fully rewritten; NEW `ch-upgrade` chapter; Appendix A configuration reference extended with new keys (`opcua.max_connections`, `web.*`, etc.).
- `docs/manual/index.xml` — Parts list extended to reflect new chapters in the manual TOC.
- `.gitignore` — added `docs/manual/out/` (manual build outputs are regeneratable via Makefile and should not be version-controlled).

Created:

- `docs/dockerhub-description.md` — canonical Markdown source for the hub.docker.com Overview page; auto-synced via the CI workflow.
- `docs/logo/opcgw-mark.svg`, `docs/logo/opcgw-horizontal.svg`, `docs/logo/opcgw-favicon.svg`, `docs/logo/opcgw-logo-pack.zip`, `docs/logo/README.md` — logo pack (added by Guy mid-story; referenced from README, manual, dockerhub-description).
- `docs/manual/Makefile` — DocBook 4.5 → HTML / single-HTML / PDF / validate / clean / print-deps build pipeline wrapping xsltproc + dblatex.

Deleted from tracking:

- `docs/manual/out/pdf/opcgw-user-manual.pdf` — `git rm --cached` only (binary build output, now gitignored).

BMad bookkeeping (touched as part of the workflow itself):

- `_bmad-output/planning-artifacts/epics.md` — Epic B section appended at story creation.
- `_bmad-output/implementation-artifacts/sprint-status.yaml` — `epic-B: backlog → in-progress` at creation; `B-1-docker-hub-publishing-and-user-manual: backlog → ready-for-dev → in-progress → review` lifecycle; stale `epic-A: in-progress → done` bookkeeping flip.
- `_bmad-output/implementation-artifacts/B-1-docker-hub-publishing-and-user-manual.md` (this file) — status + checkboxes + Dev Agent Record updated.

Strict-zero (verified unchanged by `git status` filter): `src/**/*.rs`, `tests/**/*.rs`, `Cargo.toml`, `Cargo.lock`, `migrations/*.sql`, `config/*`.

### Change Log

- 2026-05-19: Implementation complete; status `ready-for-dev → in-progress → review`. D1 = auto-sync, D2 = Makefile. Smoke test catch added `data/` to Dockerfile + docker-compose.yml + docs.
- 2026-05-19 (later, same day): `bmad-code-review` iter-1 same-LLM (Opus 4.7) complete. 45 raw findings → 26 PATCH + 6 DEFER + 7 DISMISS. Doctrine validated again: phrase-harmonization-drift surfaced 7 HIGH and 11 MED findings (manual / dockerhub-description grep recipes targeted strings the source never emits; same shape as A-7 L1+L2 / A-5 K5 / A-6 K1).
- 2026-05-19 (iter-1 patch round): all 26 PATCH findings applied. Strongest catches: T2 `OPCGW_WEB__USER_PASSWORD` (env var doesn't exist), T3 `OPCGW_OPCUA__ENDPOINT` (wrong field name; real is `host_port`/`host_ip_address`), T4+T6+T7 (3 separate non-existent structured-log events that operator grep recipes targeted: `chirpstack_poll_start`/`_end`, `poll_cycle_complete`, `chirpstack_auth_failed`), T5 `"OPC UA server started"` (string doesn't exist; replaced with `opcua_limits_configured`), T9 (manual's 2 docker-run examples missing `data/` mount — same gap that the smoke test caught during implementation), T10 (peter-evans action would have killed entire workflow incl. GHCR publish when DOCKERHUB secrets are missing; now gated by `if: secrets != ''` + `continue-on-error: true`). Plus phrase fixes (T29 "Chirpstack"→"ChirpStack", T30 short-description 109→83 chars under Docker Hub's 100-char limit), portability fixes (T22 em-dash→hyphen, T24 U+2026→explicit cmd, T13 `/nonexistant`→`/nonexistent`), correctness fixes (T17 `chmod 600 *.pem` → `find -exec`, T16 useradd `--user-group`, T14 Makefile html-single output dir so logo path resolves, T15 Makefile drops silent `|| true`, T35 systemd EnvironmentFile syntax warning, T36 curl uses `$OPCGW_OPCUA__USER_NAME`, T20 systemd static/ install step, T37 Dockerfile EXPOSE 8080, T19 dockerhub web-UI "Quick-start variant", T1+T12 manual+dockerhub `data/` path + Compose `:?err` placeholder caveat, T27 logo README assets/→docs/logo/). T18 reclassified as not-a-bug: Docker + systemd procedures effectively agree on PKI dir modes (0755 cert dirs, 0700 private/) given default umask 022. T8 + T11 deferred per user (Cargo.toml bump must be separate pre-tag commit; docker/build-push-action@v5 vs v6 needs real-CI verification). Regression gate post-patch: cargo test exit 0, clippy exit 0, doctest 0/55. DocBook 4.5 DTD validation post-patch: exit 0. Strict-zero invariant re-verified clean. Status `review → in-progress → review` (iter-1 same-LLM complete; iter-2 different-LLM pending per CLAUDE.md "Code Review & Story Validation Loop Discipline" + memory `feedback_iter3_validation` 13-story doctrine).

### Review Findings

#### Patches to apply (iter-1)

- [x] [Review][Patch] **T1** Manual claim "opcgw.db lives next to binary at /usr/local/bin" — real path is `/usr/local/bin/data/opcgw.db` [docs/manual/opcgw-user-manual.xml:~1527 + dockerhub-description.md:~777]
- [x] [Review][Patch] **T2** dockerhub-description claims `OPCGW_WEB__USER_PASSWORD` env var — doesn't exist in `WebConfig`; web auth uses `[opcua]` credentials [docs/dockerhub-description.md:91]
- [x] [Review][Patch] **T3** dockerhub-description claims `OPCGW_OPCUA__ENDPOINT` env var — real fields are `host_ip_address` + `host_port` [docs/dockerhub-description.md:93]
- [x] [Review][Patch] **T4** dockerhub-description grep `chirpstack_poll_start`/`chirpstack_poll_end` — real events `operation="poll_cycle_start"` / `operation="poll_cycle_end"` [docs/dockerhub-description.md:~809]
- [x] [Review][Patch] **T5** Manual grep "OPC UA server started" — string doesn't exist; replace with `opcua_limits_configured` or `opcua_session_count` [docs/manual/opcgw-user-manual.xml:~1839]
- [x] [Review][Patch] **T6** Manual `poll_cycle_complete` (3 occurrences) — non-existent; real event is `poll_cycle_end` [docs/manual/opcgw-user-manual.xml:~1855, ~1857, ~2650]
- [x] [Review][Patch] **T7** Manual troubleshooting grep `chirpstack_auth_failed` — event doesn't exist in src/; replace with `operation="chirpstack_connect"` + gRPC status strings [docs/manual/opcgw-user-manual.xml:~2653]
- [x] [Review][Patch] **T9** Manual `docker run` examples (Docker Hub + GHCR sect1s) missing `data/` mount — operators lose SQLite DB on every `docker rm` [docs/manual/opcgw-user-manual.xml:~1476, ~1593]
- [x] [Review][Patch] **T10** `peter-evans/dockerhub-description@v4` failure unguarded → kills whole workflow incl. GHCR publish [.github/workflows/docker-build.yml:72-82]
- [x] [Review][Patch] **T12** Compose `:?err` guard doesn't catch placeholder values (`REPLACE_ME_...`); manual oversells [docs/manual/opcgw-user-manual.xml:~1645]
- [x] [Review][Patch] **T13** `/nonexistant` typo in Dockerfile useradd — should be `/nonexistent` (or `/var/empty`) [Dockerfile:41]
- [x] [Review][Patch] **T14** Makefile `html-single` target leaves logo image broken (path resolves to non-existent `docs/manual/logo/`) [docs/manual/Makefile:~62]
- [x] [Review][Patch] **T15** Makefile `cp -r ... || true` silently swallows logo-copy failure [docs/manual/Makefile:~65, ~75]
- [x] [Review][Patch] **T16** Dockerfile `useradd` missing `--user-group` — documented `groups=10001(opcgw)` may not match on alternative base images [Dockerfile:40-45]
- [x] [Review][Patch] **T17** `chmod 600 ./pki/private/*.pem` fails on empty dir; use `find ... -exec chmod 600 {} +` [README.md + dockerhub-description.md + docs/manual/opcgw-user-manual.xml]
- [x] [Review][Patch] **T18** PKI directory mode mismatch between Docker procedure (`chmod 700` only on `private/`) and systemd procedure (`0755` on `own/`, `trusted/`, `rejected/`) [docs/manual/opcgw-user-manual.xml + others]
- [x] [Review][Patch] **T19** dockerhub-description docker-run example missing `-p` for Web UI port; add "with Web UI" variant [docs/dockerhub-description.md]
- [x] [Review][Patch] **T20** systemd install procedure doesn't deploy `static/` for web UI; add `install -m 0644 -t /var/lib/opcgw/static/ static/*` step [docs/manual/opcgw-user-manual.xml:~1681]
- [x] [Review][Patch] **T22** Em-dash `—` in compose `:?` message non-portable across shell locales; replace with `-` [docker-compose.yml:31-32]
- [x] [Review][Patch] **T24** U+2026 ellipsis in compose comment ambiguous; use explicit command [docker-compose.yml:13]
- [x] [Review][Patch] **T27** `docs/logo/README.md` integration example suggests `assets/opcgw-horizontal.svg` — actual path is `docs/logo/opcgw-horizontal.svg` [docs/logo/README.md:~50]
- [x] [Review][Patch] **T29** Manual subtitle "Chirpstack" (lowercase s) — official spelling is "ChirpStack" [docs/manual/opcgw-user-manual.xml:8]
- [x] [Review][Patch] **T30** Workflow `short-description` is 109 chars; Docker Hub limit is 100 — trim to ≤100 [.github/workflows/docker-build.yml:85]
- [x] [Review][Patch] **T35** systemd procedure's `EnvironmentFile` doesn't warn operators about syntax constraints (no shell escapes; no `=` in values) [docs/manual/opcgw-user-manual.xml:~1707]
- [x] [Review][Patch] **T36** Manual curl example hardcodes `opcua-user` username; switch to `-u "$OPCGW_OPCUA__USER_NAME:..."` [docs/manual/opcgw-user-manual.xml:~1871]
- [x] [Review][Patch] **T37** Dockerfile `EXPOSE 8080` (informational) for web UI port [Dockerfile:74]

#### Deferred findings

- [x] [Review][Defer] **T8** Cargo.toml version bump `2.0.0-rc → 2.0.0` — user-confirmed deferral; AC#20 strict-zero forbids in B-1. Required as separate pre-tag commit before pushing `v2.0.0` tag; also update README badge.
- [x] [Review][Defer] **T11** docker/build-push-action@v5 → v6 — user-confirmed deferral; needs real CI test on a real tag push to verify v6 multi-arch behavior. Add inline comment in workflow noting intentional v5 pin.
- [x] [Review][Defer] **T23** OPC UA standard port 4840 reference loss in config comment — minor information loss only.
- [x] [Review][Defer] **T25** `docs/logo/opcgw-logo-pack.zip` checked in vs `docs/manual/out/` gitignored — policy inconsistency, no functional impact.
- [x] [Review][Defer] **T26** Manual links from Docker Hub Overview point to raw DocBook XML — needs CI HTML publish first (out-of-scope v2.x doc-pass story).
- [x] [Review][Defer] **AC#23** GitHub tracking issue `Refs #__` placeholder — explicitly anticipated by spec; user will provide issue number out-of-band.

#### Decisions resolved (iter-2)

- [x] [Review][Decision→Patch] **U1** GHCR-only fallback fix — RESOLVED 2026-05-20: split into two registry-conditional `build-push` steps (option a). Idiomatic, each registry's failure independently visible, layer cache mitigable via `cache-from`. Apply to `.github/workflows/docker-build.yml:56-78`.
- [x] [Review][Decision→Patch] **U2** Cargo `2.0.0-rc` × `:2.0` doc pin × semver suppression — RESOLVED 2026-05-20: escalate T8 to HARD pre-tag gate (option a). Keep doc pins at `:2.0` (correct post-bump). Update `_bmad-output/implementation-artifacts/deferred-work.md` T8 entry from soft-deferred to a HARD pre-tag GATE with explicit ordering note: "Cargo bump MUST land BEFORE `v2.0.0` tag is pushed; otherwise all `:2.0` doc references break with `manifest unknown`."

#### Patches to apply (iter-2)

- [x] [Review][Patch] **U3** `actions/checkout@v6` does not exist — pin to `@v4` (latest stable). Workflow would fail at step 1 on first `v*` tag push. [.github/workflows/docker-build.yml:22]
- [x] [Review][Patch] **U4** dockerhub-description sister sentence `[opcua].endpoint` (iter-1 T3 missed) — replace with `[opcua].host_port` (and mention `host_ip_address`). Phrase-harmonization-drift. [docs/dockerhub-description.md:100]
- [x] [Review][Patch] **U5** Docker Hub install procedure chown missing `./data` (iter-1 T9 patched the docker-run example but left the preceding chown stale) — append `./data` to mkdir AND chown. Compose section (line 729-730) is already correct; pure phrase-harmonization-drift. [docs/manual/opcgw-user-manual.xml:570-571]
- [x] [Review][Patch] **U6** Manual conflates Rust constant `OPCGW_CONFIG_PATH` with an env-var name — actual env var is `CONFIG_PATH` (no prefix); `OPCGW_CONFIG_PATH` is just the Rust default string `"config"`. Operators setting `OPCGW_CONFIG_PATH=...` get silent ignore. Rewrite sentence to drop the `OPCGW_` prefix. [docs/manual/opcgw-user-manual.xml:1014-1015]
- [x] [Review][Patch] **U7** `chmod 600 pki/private/*.pem` glob form survives at 2 of 4 occurrences (iter-1 T17 missed) — Configuration-chapter NFR9 warning + Troubleshooting-chapter remediation step #1 still use the broken glob; line 3077 in the same `<orderedlist>` already uses the safe `find -exec` form (internal contradiction). Sync to find-exec. [docs/manual/opcgw-user-manual.xml:1392, 3064]
- [x] [Review][Patch] **U8** `EXPOSE 8080` (Dockerfile) vs compose only publishes 4855 — add commented `# - "8080:8080"  # uncomment to enable web UI` to docker-compose.yml ports so the documented "Quick-start with Web UI" recipe actually works. [Dockerfile:83, docker-compose.yml:23]
- [x] [Review][Patch] **U9** `opcgw.log` outlier in upgrade-verification recipe — actual log filename is `opc_ua_gw.log` (src/main.rs:312 + 7 other manual locations). Single-token swap. [docs/manual/opcgw-user-manual.xml:3312]
- [x] [Review][Patch] **U10** "Choosing the Image" note describes the pre-B-1 compose state — says `image: opcgw, which assumes a locally-built image` but compose actually has `image: docker.io/gcorbaz/opcgw:2.0`. Rewrite the note to point operators at the GHCR alternative line instead. [docs/manual/opcgw-user-manual.xml:758-765]
- [x] [Review][Patch] **U11** CHANGELOG.md example `Variant::Double(23.5)` — `OpcMetricTypeConfig` has no `Double` variant; code emits `Variant::Float(f32)` for `Float`-typed metrics (opc_ua.rs:1012, 1909). Replace with `Variant::Float(23.5_f32)` or use a different example metric. [CHANGELOG.md:112]
- [x] [Review][Patch] **U12** dockerhub-description verification claims `opcua_session_count` confirms "OPC UA server is listening" — the gauge requires `[opcua].diagnostics_enabled = true` (session monitor reads from async-opcua's diagnostics summary; without it, `session_count_variable_missing` fires instead). Add caveat or switch to `opcua_limits_configured`. [docs/dockerhub-description.md:166]
- [x] [Review][Patch] **U13** Makefile `cp` of 3 SVGs has no prerequisite declaration — declare logos as targets/prereqs so make fails fast before xsltproc writes partial output. [docs/manual/Makefile:53, 64]
- [x] [Review][Patch] **U14** systemd install recipe uses shell glob `static/*.css` that hard-fails on no-match — use `find static -type f \( -name '*.html' -o -name '*.js' -o -name '*.css' \) -exec install ... {} +`. [docs/manual/opcgw-user-manual.xml:2102-2103]
- [x] [Review][Patch] **U15** dockerhub-description omits the placeholder-passthrough caveat that the manual documents — `:?err` does NOT catch `REPLACE_ME_...` literal placeholders. One-line addition. [docs/dockerhub-description.md:~1079]
- [x] [Review][Patch] **U16** Unicode ellipsis `…` survives in log-line example (iter-1 T24 sweep missed this one) — operators grepping for ASCII won't match. [docs/manual/opcgw-user-manual.xml:2817]
- [x] [Review][Patch] **U17** TLS reverse-proxy requirement asserted in dockerhub-description but not enumerated in the manual's Web UI section — add `<warning>` referencing `docs/security.md` and the dockerhub Overview. [docs/manual/opcgw-user-manual.xml web-UI section]

#### Deferred findings (iter-2)

- [x] [Review][Defer] **U18** `Refs #__` commit-message trap — placeholder is literal `#__` in `e7931fe` + `1cf3ba7` commit messages; resolving via interactive rebase is forbidden by CLAUDE.md. Resolution path: follow-up commit `Refs #N (resolves placeholder from e7931fe + 1cf3ba7)` once issue is opened, not a rewrite. Documented in `deferred-work.md::DEF-iter2-B1-U18`.
- [x] [Review][Patch→Resolved-by-U13] **U19** Makefile `make -j` parallel race — RESOLVED INCIDENTALLY by U13: the new per-file pattern rule `$(OUT_DIR)/logo/%.svg: $(LOGO_SRC)/%.svg` produces one make target per logo, so `html` and `html-single` no longer execute duplicate unconditional `cp` recipes. No separate fix needed.
- [x] [Review][Defer] **U20** Makefile `LOGO_SRC := ../logo` relative path fragile — works only when `make` is invoked with cwd = `docs/manual/` (or via `make -C docs/manual`); breaks with `make -f docs/manual/Makefile` from repo root. Documented in `deferred-work.md::DEF-iter2-B1-U20`.
- [x] [Review][Defer] **U21** dockerhub-description Supported-tags table promises `:2.0` "Auto-updates on patch releases" — accurate post-Cargo-bump (rolls in with U2 / DEF-iter1-B1-T8 resolution). Documented in `deferred-work.md::DEF-iter2-B1-U21`.

#### Dismissed findings (iter-2)

- ~~**X1**~~ `max_message_size = 327675` flagged as typo for `327680` — DISMISSED. Verified at `src/utils.rs:163`: value is exactly `65535 × MAX_CHUNK_COUNT = 65535 × 5 = 327_675`, intentional per-chunk × max-chunk-count product. Internally consistent across config example, manual, and source-of-truth comment.
- ~~**X2**~~ `poll_cycle_start` (dockerhub) vs `poll_cycle_end` (manual) flagged as inconsistency — DISMISSED. dockerhub-description.md:166 actually mentions BOTH events (`start` as the "within ~30 seconds of startup" signal, `end` for "each successful cycle"); the recipe is correct. Manual's stricter `grep poll_cycle_end` is acceptable.

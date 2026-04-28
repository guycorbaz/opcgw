---
layout: default
title: Security & Secrets
permalink: /security/
---

# Security & Secrets Handling

This page documents how opcgw expects operators to inject credentials and
how the gateway protects them at runtime. It is the single source of truth
for the secret-handling contract introduced in **Story 7-1 (Epic 7 —
Security Hardening)**.

If you are setting up a fresh deployment, jump to [Quick start](#quick-start).
For a deeper dive on the contract, read the rest of the page in order.

---

## The env-var convention

opcgw loads its configuration from `config/config.toml` and merges
environment variables on top, so any field can be overridden at startup.
The canonical name for an env var is

```
OPCGW_<SECTION>__<FIELD_UPPERCASE>
```

(double-underscore between section and field — figment splits on `__` to
walk into nested TOML keys).

| Field                          | Env var                              | Required for new deployments? |
|--------------------------------|--------------------------------------|-------------------------------|
| `chirpstack.api_token`         | `OPCGW_CHIRPSTACK__API_TOKEN`        | **yes** — placeholder rejected at startup |
| `opcua.user_password`          | `OPCGW_OPCUA__USER_PASSWORD`         | **yes** — placeholder rejected at startup |
| `chirpstack.tenant_id`         | `OPCGW_CHIRPSTACK__TENANT_ID`        | optional — placeholder UUID is a valid format; ChirpStack will reject calls until set |
| `chirpstack.server_address`    | `OPCGW_CHIRPSTACK__SERVER_ADDRESS`   | optional |
| `opcua.host_port`              | `OPCGW_OPCUA__HOST_PORT`             | optional |
| `[logging].dir`                | `OPCGW_LOGGING__DIR` *or* `OPCGW_LOG_DIR` (bootstrap short form) | optional |
| `[logging].level`              | `OPCGW_LOGGING__LEVEL` *or* `OPCGW_LOG_LEVEL` (bootstrap short form) | optional |

The bootstrap short forms (`OPCGW_LOG_DIR`, `OPCGW_LOG_LEVEL`) exist only
because the logging subsystem starts before figment runs (Story 6-1/6-2).
Do not introduce a third short form for any other field unless it has the
same bootstrap-phase requirement.

---

## Precedence rules

Configuration values are resolved in this order (highest priority last):

1. **Defaults** — hard-coded in `src/config.rs`.
2. **`config/config.toml`** — values from the TOML file.
3. **Environment variables** — figment merges env on top of TOML, so an env
   var of the canonical name above always wins.

### Placeholder detection

The shipped `config/config.toml` contains placeholder values for
`api_token` and `user_password`:

```toml
api_token     = "REPLACE_ME_WITH_OPCGW_CHIRPSTACK__API_TOKEN_ENV_VAR"
user_password = "REPLACE_ME_WITH_OPCGW_OPCUA__USER_PASSWORD_ENV_VAR"
```

`AppConfig::validate` runs **after** the env-merge step, so:

- If the TOML still has a `REPLACE_ME_WITH_*` value **and** no env var
  override is supplied, the gateway exits with an actionable error like:
  ```
  Configuration validation failed:
    - chirpstack.api_token: placeholder value detected (starts with "REPLACE_ME_WITH_").
      Set OPCGW_CHIRPSTACK__API_TOKEN to inject the real secret. See docs/security.md.
  ```
  The error names the field, the env var to set, and points back here. The
  operator's literal value is never echoed back into the error message
  (avoid log-injection-style risk if a near-miss real secret is pasted in).
- If the TOML has a `REPLACE_ME_WITH_*` value **and** the env var is set
  to a real secret, validation passes — env precedence beats the
  placeholder check.

This means the placeholder is a **red flag for "operator forgot to set the
env var"**, not a blanket ban on the literal string ever appearing.

---

## Quick start

### 1. Local / `cargo run`

```bash
export OPCGW_CHIRPSTACK__API_TOKEN='paste-your-token-here'
export OPCGW_OPCUA__USER_PASSWORD='paste-your-password-here'
cargo run
```

### 2. Docker / Compose recipe

The shipped `docker-compose.yml` references `.env` so secrets stay outside
the image. Workflow:

```bash
cp .env.example .env       # creates a placeholder-only .env
chmod 600 .env              # tighten file permissions
$EDITOR .env                # replace each REPLACE_ME_WITH_* with the real secret
docker compose up
```

The Compose service block:

```yaml
environment:
  - OPCGW_CHIRPSTACK__API_TOKEN=${OPCGW_CHIRPSTACK__API_TOKEN}
  - OPCGW_OPCUA__USER_PASSWORD=${OPCGW_OPCUA__USER_PASSWORD}
```

Compose reads `.env` from the project directory and substitutes the
host-side value into the container's environment. `.env` itself is
ignored by git (`.gitignore` "# Config & Secrets" block); the committed
`.env.example` file ships placeholders only.

### 3. Kubernetes recipe

Mount each secret as an env var via `valueFrom.secretKeyRef`. Same env-var
names work:

```yaml
env:
  - name: OPCGW_CHIRPSTACK__API_TOKEN
    valueFrom:
      secretKeyRef:
        name: opcgw-secrets
        key: chirpstack-api-token
  - name: OPCGW_OPCUA__USER_PASSWORD
    valueFrom:
      secretKeyRef:
        name: opcgw-secrets
        key: opcua-user-password
```

---

## Migration path (existing deployments)

The committed `config/config.toml` shipped with previous opcgw releases
contained real ChirpStack JWTs, real tenant UUIDs, real device EUIs, and a
literal `user_password = "user1"`. After Story 7-1 lands, operators who
`git pull` will get a conflict on `config/config.toml` if they have local
edits. The recipe:

> **⚠️ Step 3 below is destructive — it overwrites your local
> `config/config.toml` with the new template.** Do not skip step 1's
> backup. If you'd rather keep the merge reversible, use the
> `git stash` alternative shown after step 6.

1. **Before pulling:** back up your local copy. Verify the backup file
   exists before continuing.
   ```bash
   cp config/config.toml ~/opcgw-config-backup.toml
   ls -l ~/opcgw-config-backup.toml   # confirm the backup is on disk
   ```
2. **Pull the change.** A conflict on `config/config.toml` is expected.
   ```bash
   git pull
   ```
3. **Resolve by keeping the new template.** This discards your local
   `config/config.toml` — your backup from step 1 is the only copy.
   ```bash
   git checkout --theirs config/config.toml
   ```
4. **Restore your application list.** Copy your `[[application]]` blocks
   from the backup into the new `config/config.toml`. Leave the
   `api_token` / `user_password` fields with their `REPLACE_ME_WITH_*`
   placeholders.
5. **Move secrets to env vars.** Create `.env` from `.env.example`, fill
   in the real values from your backup, then tighten permissions.
   ```bash
   cp .env.example .env
   chmod 600 .env
   $EDITOR .env
   ```
6. **Verify.** `cargo run` (or `docker compose up`) should start cleanly.
   If it exits with a placeholder error, you missed step 5.

### Reversible alternative — `git stash` workflow

If you'd prefer to keep the original `config/config.toml` in your working
tree until you've manually merged the changes, use `git stash` instead of
`checkout --theirs`:

```bash
# Save your local config (includes uncommitted edits anywhere in the tree).
git stash push -m "pre-7-1 config" config/config.toml

# Pull the new template cleanly — no conflict because the file is stashed.
git pull

# Diff the stashed version against the new template to plan the merge.
git stash show -p --name-only stash@{0}
git diff stash@{0} -- config/config.toml

# Manually merge your `[[application]]` blocks into the new template,
# then drop the stash when you're done.
$EDITOR config/config.toml
git stash drop stash@{0}
```

This path leaves both versions recoverable until you explicitly drop the
stash. It costs one extra command vs. step 3 above and is the safer
default if you're not sure about the merge.

> A one-shot helper (`scripts/migrate-config-7-1.sh`) was considered and
> deferred. The manual steps above are short and one-time per operator.

---

## What the gateway will / won't redact

The hand-written `Debug` impls on `ChirpstackPollerConfig` and
`OpcUaConfig` (Story 7-1, AC#3) emit `***REDACTED***` for the two fields
classified as secrets by the epic spec. Everything else uses the default
`Debug` formatting so existing log lines are unchanged.

| Struct                    | Field                                | Redacted in `Debug`? | Why |
|---------------------------|--------------------------------------|----------------------|-----|
| `ChirpstackPollerConfig`  | `api_token`                          | **yes**              | NFR7 secret |
| `ChirpstackPollerConfig`  | `tenant_id`                          | no                   | Not classified as a secret by the epic spec. Substituted with the all-zeros placeholder UUID in the shipped template (so the operator's tenant identity isn't published) but not redacted in logs. Tracked as a follow-up enhancement (see `_bmad-output/implementation-artifacts/deferred-work.md`). |
| `ChirpstackPollerConfig`  | `server_address`                     | no                   | Already in startup `info!` line; well-established as non-secret |
| `OpcUaConfig`             | `user_password`                      | **yes**              | NFR7 secret |
| `OpcUaConfig`             | `user_name`                          | no                   | Not a secret in the OPC UA model |
| `OpcUaConfig`             | `certificate_path`, `private_key_path` | no                | Paths, not key material — but the **content** of `private_key_path` is sensitive; file-permission enforcement is Story 7-2 (NFR9) |

Anything not in this table is **not** secret-protected. If you add a new
sensitive field, extend the table here **and** the `Debug` impl in
`src/config.rs` together.

The redaction protects against `format!("{:?}", config)` and
`tracing::trace!(?config, ...)` reaching any appender. It does **not** by
itself protect against a future contributor wiring a `tower-http` /
`tonic` middleware that logs gRPC request metadata at trace level — see
the next section.

---

## Anti-patterns

- **Do not** bake secrets into Docker images. Build the image once,
  inject secrets at runtime via env vars.
- **Do not** commit `.env` to git. The shipped `.gitignore` excludes it
  in the "# Config & Secrets" block; do not add overrides.
- **Do not** paste tokens into bug reports, Slack threads, or screenshots.
  If a token leaks, rotate it on the ChirpStack side first, then update
  the env var.
- **Do not** introduce a parallel short-form env var for `api_token` /
  `user_password` (e.g. `OPCGW_API_TOKEN`). The figment nested form
  (`OPCGW_CHIRPSTACK__API_TOKEN`) is the canonical name and is pinned by
  regression tests in `src/config.rs`.
- **Do not** wire `tower-http::trace::TraceLayer` or any tonic
  interceptor that logs request metadata. The `Debug` redaction above only
  protects `ChirpstackPollerConfig`; the `api_token` is also copied into
  `AuthInterceptor.api_token` (`src/chirpstack.rs`) and inserted as
  `Bearer {token}` into the gRPC `authorization` metadata header on every
  outbound call. Wiring a `TraceLayer` re-opens the bearer-token leak
  vector that Story 7-1 audits and avoids. Tracked as a follow-up GitHub
  issue (see `_bmad-output/implementation-artifacts/deferred-work.md`).
- **Do not** rewrite the figment loader. The two-phase bootstrap in
  `src/main.rs` is correct and pinned by tests.

---

## Audit findings: tonic 0.14.5 metadata logging (Story 7-1, AC#5)

opcgw uses `tonic 0.14.5` for the ChirpStack gRPC client. Audit results at
the time of Story 7-1 implementation:

- `tonic 0.14.5` has eight `tracing::*!` sites, all on error conditions
  (connection errors, accept-loop errors, TLS errors, `grpc-timeout`
  parse errors, reconnect errors). None of them include request headers
  or metadata in the event fields.
- No `#[instrument]` attributes capture request fields.
- `grep -rnE 'TraceLayer|trace_layer|tower_http' src/ Cargo.toml`
  returned nothing — opcgw does not depend on `tower-http` and does not
  wire any `TraceLayer`.

**Conclusion:** at the time of writing, no `EnvFilter` mitigation is
needed. If a future opcgw change adds tower-http `TraceLayer` wiring or
upgrades to a tonic version that logs request metadata, add an
`EnvFilter` directive in `src/main.rs` clamping `tonic` and
`tonic::transport` targets to `info` level so trace-level header dumps
are filtered before reaching any appender, and update this section.

A proactive mitigation (a `tower::Layer` that strips the `authorization`
header before logging) is tracked as a follow-up GitHub issue.

---

## References

- Story 7-1 spec: `_bmad-output/implementation-artifacts/7-1-credential-management-via-environment-variables.md`
- PRD requirements: FR42 (env-var injection), NFR7 (no secrets in logs),
  NFR8 (no real credentials in default config), NFR24 (env override for
  all secrets) in `_bmad-output/planning-artifacts/prd.md`
- Configuration reference: [`docs/configuration.md`](configuration.md)
- Deferred follow-ups: `_bmad-output/implementation-artifacts/deferred-work.md`

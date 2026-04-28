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

## OPC UA security endpoints and authentication

Story 7-2 hardens the OPC UA server's exposure surface so a default
deployment is safe to expose on a LAN. The endpoint plumbing was already
in place from earlier epics; Story 7-2 pins the contract by tests, adds
a custom audit-trail authenticator, enforces filesystem permissions on
the private key, and ships a sane `create_sample_keypair` default.

### Endpoint matrix

The gateway advertises **three** endpoints on the same path (`/`) and
the same TCP port (4840 by default):

| Endpoint id              | Security policy | Security mode      | Security level | Intended use                                                            |
|--------------------------|-----------------|--------------------|----------------|-------------------------------------------------------------------------|
| `null`                   | `None`          | `None`             | 0              | Development and first-run smoke tests on trusted LANs / behind VPN.     |
| `basic256_sign`          | `Basic256`      | `Sign`             | 3              | Signed traffic, no encryption — useful when LAN traffic must remain inspectable. |
| `basic256_sign_encrypt`  | `Basic256`      | `SignAndEncrypt`   | 13             | **Production default.** Highest level the gateway advertises today.     |

Endpoint ids and security levels are pinned by the integration test
`tests/opc_ua_security_endpoints.rs::test_three_endpoints_accept_correct_credentials`
— changes to `configure_end_points` in `src/opc_ua.rs` that drift any of
the three tuples will fail this test.

### User-token model

The gateway uses a **single user/password** (Story 7-2 Out of Scope:
multi-user RBAC). Configure via:

| Field                   | Env var                       | Notes                                                      |
|-------------------------|-------------------------------|------------------------------------------------------------|
| `[opcua].user_name`     | `OPCGW_OPCUA__USER_NAME`      | Display name.                                              |
| `[opcua].user_password` | `OPCGW_OPCUA__USER_PASSWORD`  | **Always set via env var** — the placeholder in the shipped TOML is rejected at startup. |

Internally the user-token id is `default-user`
(`crate::utils::OPCUA_USER_TOKEN_ID`). It is decoupled from the operator's
configured `user_name` so a future multi-user expansion has a clean
single-tenant baseline.

### PKI directory layout

`pki_dir` (default `./pki`) must contain four subdirectories:

```
pki/
├── own/         # 0o755   — server's own certificate (cert.der)
├── private/     # 0o700   — server's private key (private.pem, mode 0o600)
├── trusted/     # 0o755   — client certificates accepted without prompt
└── rejected/    # 0o755   — client certificates rejected on first connect
```

If any subdirectory is missing, `OpcUa::create_server` auto-creates it
with the correct mode (`src/security.rs::ensure_pki_directories`).
Loose modes on `private/` are tightened to `0o700` automatically.

The `private/*.pem` file mode is checked at startup. **The gateway
refuses to start** if any private-key file is not at `0o600` (NFR9).
Error text includes the observed mode and the `chmod` recipe.

### Production setup recipe

```bash
# 1. Generate a self-signed keypair (or supply a CA-signed equivalent).
openssl req -x509 -newkey rsa:4096 -nodes -days 3650 \
  -keyout pki/private/private.pem -out pki/own/cert.der -outform DER \
  -subj "/CN=opcgw" -addext "subjectAltName=URI:urn:chirpstack:opcua:gateway"

# 2. Tighten file/directory permissions.
chmod 600 pki/private/private.pem
chmod 700 pki/private

# 3. Set create_sample_keypair = false in config/config.toml (the
#    shipped default since Story 7-2 — verify it has not been flipped).

# 4. Inject the OPC UA password via env var.
export OPCGW_OPCUA__USER_PASSWORD='your-real-password-here'

# 5. Start the gateway and confirm the boot log shows
#    `event="pki_dir_initialised"` events with the correct modes.
cargo run --release
grep 'pki_dir_initialised' log/opc_ua_gw.log
```

### Upgrading from Story 7-1

Story 7-1 left `pki/private/private.pem` at mode `0o644` (async-opcua's
auto-generation default). Story 7-2's startup file-permission check is a
**hard error** — a Story-7-1 deployment will refuse to start until the
operator runs:

```bash
find pki/private -type f -name '*.pem' -exec chmod 600 {} \;
chmod 700 pki/private
```

The fail-closed behaviour is intentional: silently running with a
world-readable private key is worse than refusing to start.

### Audit trail

Every failed OPC UA authentication emits a structured `warn!` event in
`log/opc_ua.log`:

```
2026-04-28T14:22:18.041234Z  WARN opcgw::opc_ua_auth: OPC UA authentication failed event="opcua_auth_failed" user="alice" endpoint="/"
```

The submitted username is **sanitised** (control characters escaped,
truncated to 64 chars) before logging so a malicious client cannot
inject fake log lines or ANSI escapes. The attempted password is **never
logged**.

**Source IP is not in the auth event** — async-opcua 0.17.1's
`AuthManager` trait does not receive the peer's `SocketAddr`. NFR12 is
satisfied via two-event correlation: async-opcua emits an `info!` event
on connection accept that includes the peer address, then milliseconds
later the gateway emits the auth-failed event. Operators correlate by
timestamp:

```bash
# Step 1: find auth failures.
grep 'event="opcua_auth_failed"' log/opc_ua.log

# Step 2: find the matching accept event (typically <100ms before).
grep 'Accept new connection from' log/opc_ua.log | tail -50
# 2026-04-28T14:22:18.039012Z  INFO opcua_server::server: Accept new connection from 192.168.1.42:54321 (3)
```

The audit-event redaction matrix:

| Field               | Logged?   | Notes                                                |
|---------------------|-----------|------------------------------------------------------|
| `user`              | yes       | Sanitised — control chars escaped, capped at 64 chars |
| `endpoint`          | yes       | Endpoint path (always `/`)                           |
| `attempted_password`| **never** | Hard rule — no level, no redaction placeholder       |
| `source_ip`         | no (correlate) | Carried by async-opcua's accept event             |

A first-class source-IP-in-the-auth-event is tracked as an upstream
follow-up against async-opcua (see
`_bmad-output/implementation-artifacts/deferred-work.md`).

### Verifying OPC UA security

A small smoke-test client ships under `examples/opcua_client_smoke.rs`:

```bash
# Connect to None endpoint with valid credentials.
cargo run --example opcua_client_smoke -- \
    --endpoint none --user opcua-user --password "$OPCGW_OPCUA__USER_PASSWORD"
# Expected: prints "Session established on endpoint=None" and exits 0.

# Connect to Basic256 SignAndEncrypt with valid credentials.
cargo run --example opcua_client_smoke -- \
    --endpoint sign-encrypt --user opcua-user --password "$OPCGW_OPCUA__USER_PASSWORD"
# Expected: prints "Session established on endpoint=Basic256/SignAndEncrypt" and exits 0.

# Wrong password — expect failure + a warn line in log/opc_ua.log.
cargo run --example opcua_client_smoke -- \
    --endpoint none --user opcua-user --password wrong
# Expected: exits with non-zero status. Tail log/opc_ua.log:
#   grep 'event="opcua_auth_failed"' log/opc_ua.log
```

### Docker deployment

When `pki/` is mounted as a Docker volume, **host-side file permissions
are authoritative**. The container's UID must own (or have the right
group on) the mounted files. The `ensure_pki_directories` chmod runs
inside the container — it only succeeds if the container user can `chmod`
the host files, which is typically true when the host volume is owned by
the container's UID. If you run rootless Docker or with a non-default UID
mapping, ensure the UID alignment before mounting.

### Anti-patterns

- **Do not** run with `create_sample_keypair = true` in production. The
  shipped default since Story 7-2 is `false`. Release builds emit a
  startup `warn!` if the flag is `true`.
- **Do not** leave `private/*.pem` at `0o644`. The startup check is a
  hard error — fix the mode rather than relaxing the check.
- **Do not** configure the `null` endpoint as the only available
  endpoint on a network reachable from outside the LAN. Operators on
  the same trust domain can use it; remote clients should always go
  through `basic256_sign_encrypt`.
- **Do not** add multi-user support, mTLS, or rate-limiting failed
  attempts as part of casual changes — those are tracked separately
  (see `_bmad-output/implementation-artifacts/deferred-work.md` and the
  follow-up GitHub issues opened with Story 7-2).

---

## References

- Story 7-1 spec: `_bmad-output/implementation-artifacts/7-1-credential-management-via-environment-variables.md`
- Story 7-2 spec: `_bmad-output/implementation-artifacts/7-2-opc-ua-security-endpoints-and-authentication.md`
- PRD requirements: FR42 (env-var injection), NFR7 (no secrets in logs),
  NFR8 (no real credentials in default config), NFR24 (env override for
  all secrets), FR19 (multi-policy OPC UA endpoints), FR20 (OPC UA user
  auth), FR45 (PKI layout), NFR9 (private-key 0o600), NFR12 (failed-auth
  audit trail) in `_bmad-output/planning-artifacts/prd.md`
- Configuration reference: [`docs/configuration.md`](configuration.md)
- Deferred follow-ups: `_bmad-output/implementation-artifacts/deferred-work.md`

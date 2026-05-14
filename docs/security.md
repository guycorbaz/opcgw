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

#### Required log levels for NFR12 correlation

The two-event correlation only works when both events reach the log
sink. async-opcua emits the connection-accept event at **`info!`**
level on the `opcua_server::server` target; the gateway emits the
auth-failed event at **`warn!`** level on the `opcgw::opc_ua_auth`
target. **Both targets must be at `info!` level or below** for NFR12
to hold. Concretely:

- The default `OPCGW_LOG_LEVEL=info` is sufficient — do not raise it
  to `warn` or `error` on the global console.
- The per-module file appender for `opc_ua.log` already captures
  async-opcua at `DEBUG` and `opcgw::opc_ua` at `TRACE` (see
  `config/config.example.toml` "Logging configuration"), so the
  on-disk audit trail is unaffected by the global console level.
- If you set `OPCGW_LOG_LEVEL=warn` to reduce console volume, the
  console will still receive the auth-failed event but **not** the
  preceding accept event. Operators must rely on `log/opc_ua.log`
  (the file appender) for the correlation in that case — the global
  console becomes a "username only" view.

Loud check at startup: as of issue #91 (Epic 7 retrospective action
item, 2026-04-29), the gateway emits a one-shot
`warn!(operation="nfr12_correlation_check", level=...)` immediately
after the `Resolved global log level` info line whenever the resolved
level is more restrictive than `info`. The warn is visible at
`OPCGW_LOG_LEVEL=warn` (the most common volume-reduction case) but
filtered at `error` / `off` — operators choosing to silence everything
below ERROR are presumed to know they're trading off the audit trail.
The startup warn does not fail-fast (operators may legitimately want
quieter console output when running headless under systemd). The
correlation recipe above tells operators which log file to grep when
console output is intentionally minimal.

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
- **Do not** rely on `create_sample_keypair = true` to "fix" a missing
  keypair on a running deployment. When the configured private-key file
  is absent and `create_sample_keypair = true`, async-opcua regenerates
  the keypair on next start with the **default umask** (typically
  `0o644` — world-readable). The startup file-permission check
  short-circuits on the missing-file path and does not catch it; the
  next-restart validation does, but the gateway runs once with a
  world-readable key in the meantime. Production deployments must
  provision the keypair manually with `chmod 600` and ship with
  `create_sample_keypair = false` so this regen path can never trigger.
  This is intentional — the alternative (post-create chmod or hard
  fail) would prevent operators from using `create_sample_keypair` for
  development, where the world-readable window is acceptable.
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

## OPC UA connection limiting

Story 7-3 caps the number of concurrent OPC UA client sessions the
gateway will host so a misbehaving SCADA client (runaway reconnect
loop, leaked sessions, deliberate flood) cannot exhaust file
descriptors, memory, or CPU. This closes **FR44** and the **OT
Security / Connection rate limiting** PRD line item.

### What it is

A configurable cap on concurrent OPC UA **sessions** (not raw TCP
connections — async-opcua's enforcement point is `CreateSession`,
which is the first wire-level signal that the peer is a real OPC UA
client). New sessions beyond the cap are rejected by async-opcua with
the OPC UA status code `BadTooManySessions`. **Existing sessions are
unaffected** — the cap is checked on the (N+1)th attempt only.

Default: **10 concurrent sessions**. Range: 1 to 4096 (the upper
bound is a "you almost certainly want a deployment review" guard
against fd-exhaustion DoS — see Story 7-3 spec for the back-of-
envelope rationale).

### Configuration

```toml
# config/config.toml
[opcua]
max_connections = 10
```

Env-var override (figment `__`-split convention):

```bash
OPCGW_OPCUA__MAX_CONNECTIONS=20 cargo run
```

`max_connections = 0` and values above 4096 are rejected at startup
by `AppConfig::validate` with a clear error message. Single-client
lockdown (`max_connections = 1`) is a legitimate "engineering-only-
access" configuration for a final commissioning window.

**Worked sizing example.** 10 SCADA clients × 1 session each = 10.
Reserve 2-3 slots for overlap during reconfiguration / failover, so
12-13 is a typical Phase A choice. Going above 50 should prompt a
deployment review — most LAN-internal SCADA scenarios saturate well
before that point.

### What you'll see in the logs

Two events, both on the `opcgw::opc_ua_session_monitor` target:

- `event="opcua_session_count" current=N limit=L` at `info!` level,
  every 5 seconds (gauge — operators graph this for capacity
  planning). Period controlled by
  `OPCUA_SESSION_GAUGE_INTERVAL_SECS`.
- `event="opcua_session_count_at_limit" source_ip=<addr> limit=L current=N`
  at `warn!` level, fired on every TCP accept while the gateway is
  at the cap. The `source_ip` field comes from async-opcua's
  pre-existing `info!("Accept new connection from {addr}")` line —
  we correlate to it from a tracing-Layer (same NFR12 two-event
  pattern Story 7-2 used for failed-auth audit).

#### Grep recipes

```bash
# See current utilisation.
grep 'event="opcua_session_count"' log/opc_ua.log | tail -5

# Find at-limit rejections.
grep 'event="opcua_session_count_at_limit"' log/opc_ua.log
# 2026-04-29T10:14:22.105Z  WARN opcgw::opc_ua_session_monitor: ... source_ip=192.168.1.42:54311 limit=10 current=10
```

### Anti-patterns

- **Do not set `max_connections = 0`.** Refuses operators too —
  startup will fail-fast.
- **Do not set above 4096.** File-descriptor exhaustion risk on
  default Linux ulimits; startup will fail-fast.
- **Do not combine `max_connections = <any>` with
  `diagnostics_enabled = false`.** The session-count gauge and the
  at-limit warn both read async-opcua's `CurrentSessionCount`
  diagnostics variable; with diagnostics disabled the counter never
  increments, the gauge logs `current=0` forever, and the at-limit
  warn never fires (the cap is still enforced via
  `SessionManager.sessions.len()`, but operator observability is
  silent). Startup will fail-fast with a remediation hint.
- **Do not rely on the cap as a brute-force defence.** Per-IP
  throttling is a separate, deferred concern (issue
  [#88](https://github.com/guycorbaz/opcgw/issues/88)). The cap stops
  a single misbehaving SCADA but does not stop a distributed flood.

### Expected at-limit log noise

When the gateway is at the cap, **every** TCP accept fires an
`event="opcua_session_count_at_limit"` warn — including port scans
and partial-handshake probes that never request a session. This is
the correct trade-off (operators want full visibility into
rejection-window connection attempts) but means a misconfigured
upstream firewall, a busy nmap scan, or a confused SCADA reconnect
loop can produce a high rate of warns. The warn event is the
symptom; investigate the source IPs and either tighten the firewall
or raise the cap.

### Tuning checklist

1. Inventory expected SCADA clients × sessions each.
2. Add 20% headroom.
3. Gauge over a representative day.
4. Raise the cap if `current` is consistently within 90% of `limit`.

### What's out of scope

- **Per-source-IP rate limiting / token-bucket throttling.** Tracked
  at issue [#88](https://github.com/guycorbaz/opcgw/issues/88).
- **Per-endpoint or per-user session caps.** Differentiated quotas
  (e.g. "5 SignAndEncrypt + 5 None") are not in scope.
- **Hot-reload of the cap at runtime.** Currently read at startup
  only — Phase B Epic 9 hot-reload covers runtime reconfiguration
  (issue [#90](https://github.com/guycorbaz/opcgw/issues/90)).

### Subscription and message-size limits

Story 8-2 (Phase B) extends the connection-limiting surface with four
configurable `Limits` knobs that shape subscription / message-size
load. They share the validation pattern, env-var convention, and
hard-cap shape established by `max_connections`.

#### What they are

| Knob | Purpose | Default | Range |
|------|---------|---------|-------|
| `max_subscriptions_per_session` | Per-session cap on simultaneous subscriptions. The (cap+1)th `CreateSubscription` from a session is rejected with `BadTooManySubscriptions`. | 10 | 1–1000 |
| `max_monitored_items_per_sub` | Per-subscription cap on monitored items. Past the cap, async-opcua returns `BadTooManyMonitoredItems` (service-level error in 0.17.1, observed empirically). | 1000 | 1–100 000 |
| `max_message_size` | Per-message byte ceiling (inbound + outbound, including `DataChangeNotification` payloads). | 327 675 (= 65 535 × 5) | 1–268 431 360 (≈ 256 MiB; = 4096 × 65535) |
| `max_chunk_count` | Per-message chunk count ceiling. Together with `max_message_size`, bounds per-message resource cost. | 5 | 1–4096 |

The two subscription-related defaults match async-opcua 0.17.1's
library defaults (`MAX_SUBSCRIPTIONS_PER_SESSION = 10`,
`DEFAULT_MAX_MONITORED_ITEMS_PER_SUB = 1000`); the two message-size
defaults match `opcua_types::constants::MAX_MESSAGE_SIZE` /
`MAX_CHUNK_COUNT`. Unsetting in TOML is a true no-op against the
library.

#### Configuration

```toml
[opcua]
# Subscription / message-size limits — uncomment only if a deployment
# scenario requires tuning. All four default to the async-opcua
# library defaults.
#max_subscriptions_per_session = 10                  # Range: 1-1000
#max_monitored_items_per_sub   = 1000                # Range: 1-100000
#max_message_size              = 327675              # Range: 1-268431360 (≈ 256 MiB)
#max_chunk_count               = 5                   # Range: 1-4096
```

Env-var overrides (figment `__`-split convention):

```bash
OPCGW_OPCUA__MAX_SUBSCRIPTIONS_PER_SESSION=20
OPCGW_OPCUA__MAX_MONITORED_ITEMS_PER_SUB=500
OPCGW_OPCUA__MAX_MESSAGE_SIZE=131072
OPCGW_OPCUA__MAX_CHUNK_COUNT=10
```

Validation (`AppConfig::validate`) rejects each knob with `Some(0)`
(misconfiguration — would refuse all subscriptions / items / messages
including operators' clients) and `Some(n) > HARD_CAP` (structural
ceiling — values above signal a misconfiguration rather than a
deliberate sizing). Errors accumulate so a single startup pass
surfaces every violation.

#### What you'll see in the logs

At startup, the gateway emits a one-shot diagnostic event with the
resolved values for all five session / subscription / message-size
limits:

```bash
grep 'event="opcua_limits_configured"' log/opcgw.log | tail -1
# 2026-04-30T08:14:22Z  INFO opcgw::opc_ua: event="opcua_limits_configured"
#   max_sessions=10 max_subscriptions_per_session=10
#   max_monitored_items_per_sub=1000 max_message_size=327675
#   max_chunk_count=5 "OPC UA limits configured"
```

Operators grep this line on every restart to verify the resolved
configuration matches expectations.

**Subscription-flood / monitored-item-flood rejections are silent**
in async-opcua 0.17.1 — `SubscriptionService::create_subscription`
returns `BadTooManySubscriptions` and `MonitoredItemService` returns
`BadTooManyMonitoredItems` *without* log emission. The contract is
the OPC UA status code on the wire, not a log line. Tracked as a
candidate for an upstream feature request (analogous to issue #94's
session-rejected-callback gap).

#### Stale-status notifications and the `DataChangeFilter` contract

Story 5-2's stale-status logic propagates through subscription
notifications **only when the client supplies a `DataChangeFilter`
with `trigger: StatusValue` or `StatusValueTimestamp`** (OPC UA
Part 4 §7.22.2 `DataChangeFilter`). The library default for
`DataChangeTrigger` is `Status` (annotated `#[opcua(default)]` on
`DataChangeTrigger::Status` in `async-opcua-types`) — that default
would fire only on status changes and miss value-only changes, so
compliant SCADA clients like FUXA, Ignition, and UaExpert override
the trigger to `StatusValue` or `StatusValueTimestamp` to fire on
either. With the filter present, async-opcua's `is_changed()` in
`async-opcua-types::data_change` detects status-only transitions
even when the numeric value is unchanged, so a Good→Uncertain
transition during a ChirpStack outage fires a notification and
SCADA dashboards show the stale state.

If a client supplies **no filter** (`ExtensionObject::null()`),
async-opcua falls into the unfiltered path in
`MonitoredItem::notify_data_value`
(`async-opcua-server::subscriptions::monitored_item`) which dedupes
on `value.value` only — status-only transitions are silently
suppressed and dashboards would freeze on the last-good value. This
Plan-A fallback is pinned by
`tests/opcua_subscription_spike.rs::test_subscription_unfiltered_dedupes_status_only_transitions`
as a regression baseline against issue #94.

#### Anti-patterns

- Setting any knob to `0` — refuses all subscriptions / items /
  messages, including operators'. Validation rejects it.
- Setting `max_message_size` above `max_chunk_count × 65535` without
  understanding the chunk geometry — see async-opcua docs.
- Relying on `max_subscriptions_per_session` for distributed-flood
  defence. It is a per-session cap, not a per-IP cap. Per-IP
  throttling is deferred (issue
  [#88](https://github.com/guycorbaz/opcgw/issues/88)).

#### Tuning checklist

- Inventory expected SCADA clients × subscriptions per client
  (typically 1–3); add 30% headroom.
- Inventory monitored items per subscription (typically 10–100 for
  FUXA dashboards); leave the 1000 default unless headroom demands
  more.
- `max_message_size` / `max_chunk_count` only matter if `Read`
  operations return very large arrays; default opcgw deployments
  expose scalar metrics and the defaults are oversized.
- Pair with `max_connections`: subscription clients consume one
  session each, so `max_connections × max_subscriptions_per_session
  × max_monitored_items_per_sub` is the upper bound on the publish
  pipeline's work.

### Subscription clients and the audit trail

Subscription-creating clients pass through the existing
`OpcgwAuthManager` (Story 7-2) and `AtLimitAcceptLayer` (Story 7-3)
identically to read-only clients. The `event="opcua_auth_failed"`
and `event="opcua_session_count_at_limit"` audit events from those
stories cover them. **No new audit infrastructure was introduced by
Story 8-2** (NFR12 carry-forward acknowledgment). The regression
baseline is two existing tests in
`tests/opcua_subscription_spike.rs`:
`test_subscription_client_rejected_by_auth_manager` and
`test_subscription_client_rejected_by_at_limit_layer`.

The new `event="opcua_limits_configured"` is a **diagnostic
startup-config event** (same shape as Story 7-2's
`pki_dir_initialised`), not an audit event.

### What's out of scope (subscription / message-size knobs)

- **Per-source-IP subscription throttling.** Tracked at issue
  [#88](https://github.com/guycorbaz/opcgw/issues/88).
- **Upstream FR for rejection-time audit events** in async-opcua
  (`BadTooManySubscriptions` / `BadTooManyMonitoredItems` are silent
  in 0.17.1) — operator-pending follow-up.
- **The five "advanced" subscription knobs** surfaced by the spike
  report (`max_pending_publish_requests`,
  `max_publish_requests_per_subscription`, `min_sampling_interval_ms`,
  `max_keep_alive_count`, `max_queued_notifications`) — deferred
  unless an operator's `--load-probe` numbers (issue
  [#95](https://github.com/guycorbaz/opcgw/issues/95)) reveal a
  back-pressure scenario the four mandatory knobs can't shape.

---

## OPC UA NodeId format (Issue #99 fix, 2026-05-02)

opcgw constructs OPC UA NodeIds in namespace `ns=2` using **stable
identifiers** rather than human-readable display names:

| Node | NodeId identifier (string form) | Browse name + display name |
|---|---|---|
| Application folder | `application_id` (UUID from `[[application]].application_id`) | `application_name` |
| Device folder | `device_id` (DevEUI / chirpstack ID) | `device_name` |
| Metric variable | `format!("{}/{}", device_id, metric_name)` (e.g., `"0000000000000001/Moisture"`) | `metric_name` |
| Gateway folder + members | hard-coded strings (e.g., `"Gateway"`, `"LastPollTimestamp"`) | same as NodeId |

The metric NodeId embeds `device_id` so two devices that share a
`metric_name` (e.g., both have a "Moisture" metric) resolve to two
distinct NodeIds — `"device_a/Moisture"` vs `"device_b/Moisture"` —
instead of colliding on a single `"Moisture"` node where the second
registration would silently overwrite the first.

**Anti-pattern:** hard-coding NodeId strings in SCADA configurations
that bypass the browse step. A FUXA / Ignition project that hard-codes
`"ns=2;s=Moisture"` (the pre-fix shape) breaks after the fix; even
post-fix, hard-coded strings break when the operator changes
`device_id` in `config.toml`. **Always use the browse path** to
resolve NodeIds at SCADA project setup time, and re-resolve on
configuration changes.

**Migration impact:** existing SCADA configurations that browsed the
address space and stored the resulting NodeIds will need to re-resolve
after upgrading. The browse-name and display-name are unchanged, so
the browse tree looks identical to operators — only the underlying
NodeId identifier string is new.

---

## Historical data access

Story 8-3 closes FR22 by exposing the `metric_history` SQLite table
(populated by the poller's append-only write path, Story 2-3b) as OPC UA
`HistoryRead` results. A SCADA client (FUXA, Ignition, UaExpert) issues a
`HistoryRead` request for a metric NodeId and receives a list of
timestamped values that fit the requested time window. This unlocks the
"show me the past 7 days of soil moisture" use case without polling.

### What it is

When a SCADA client issues an OPC UA `HistoryRead` request with
`HistoryReadDetails::ReadRawModified`, opcgw resolves the inbound NodeId
to the `(device_id, chirpstack_metric_name)` pair that the address-space
construction loop registered for that variable, queries
`metric_history` via the existing `(device_id, timestamp)` composite
index, and writes the typed values back to the wire as a `HistoryData`
extension object. The new code surface lives in
`src/opc_ua_history.rs` (a thin wrap around async-opcua's
`SimpleNodeManagerImpl`) and `src/storage/sqlite.rs::query_metric_history`
(the storage method).

What you get on the wire is exactly what the poller stored, with one
caveat: rows whose `value` column doesn't parse to the declared type
(e.g. `"NaN"` for a Float metric, `"garbage"` for a Bool metric) are
silently skipped with a `trace!` log. This is the partial-success
contract — a single bad row never terminates a 600k-row scan.

#### Known limitations of the historized record

- **All historical rows are reported `StatusCode::Good`** — the
  `metric_history` SQLite table has no `status` column, so the
  `OpcgwHistoryNodeManagerImpl` cannot reconstruct the per-row status
  that the live read path computes via the Story 5-2 stale-detection
  logic. A SCADA client reviewing a flaky sensor's history will see "all
  green" even if the live reads for that period were `Uncertain`. Use
  the live `Read` service alongside `HistoryRead` if status
  interpretation matters for your workflow.
- **Timestamps are microsecond-precise on the wire.** The storage layer
  uses `SecondsFormat::AutoSi` RFC3339 (which caps at microsecond
  resolution), then `OpcDateTime` re-encodes as 100-nanosecond ticks
  since 1601. Sub-microsecond detail from `SystemTime` is lost; this is
  not a regression — it's the same precision the poller writes.

#### `[storage].retention_days` and HistoryRead

The `[storage].retention_days` knob (and its env-var override
`OPCGW_STORAGE__RETENTION_DAYS`) governs **both** the prune loop's
deletion horizon **and** the effective HistoryRead window. Story 8-3
extended this single field rather than adding a separate
`history_retention_days` — one source of truth, validated against the
FR22 floor of 7 days and the storage-cost hard cap of 365 days. The
field is written to the SQLite `retention_config` table at every
startup, overriding the migration default of 90 days.

### Configuration

Two new knobs land in `[storage]` and `[opcua]`:

| Knob | TOML key | Default | Range | Env var |
|---|---|---|---|---|
| Retention period for `metric_history` | `[storage].retention_days` | `7` | 7-365 | `OPCGW_STORAGE__RETENTION_DAYS` |
| Per-call HistoryRead response cap | `[opcua].max_history_data_results_per_node` | `10000` | 1-1_000_000 | `OPCGW_OPCUA__MAX_HISTORY_DATA_RESULTS_PER_NODE` |

The 7-day floor on `retention_days` matches FR22 ("a minimum of 7 days
of historical data must be retained"). Values below 7 are rejected at
startup. The 365-day cap is a deployment review trigger — at 10s polling
× ~400 metric pairs × 365 days the table approaches 1.3 billion rows
and pruning + HistoryRead query latency need a separate look. Operators
that need longer retention should open a follow-up issue.

The 10000-row default for `max_history_data_results_per_node` is
roughly 28 hours of poll data at 10s polling — sufficient for typical
FUXA dashboard time-windows. SCADA clients that want longer windows
**page manually** (see *Anti-patterns* below).

`[storage].retention_days` is written into the SQLite `retention_config`
table at every startup via `INSERT OR REPLACE`, overriding the migration
default of 90 days that `v001_initial.sql` seeds at first boot. This
keeps the prune loop and the operator-config in sync.

### What you'll see in the logs

On a successful HistoryRead with rows returned:

```
DEBUG history_read_raw_modified: returning rows
    node_id=ns=2;s=Moisture
    device_id=0000000000000001
    metric_name=moisture
    row_count=42
```

On a HistoryRead for an unregistered NodeId (typo, or a node that's not
a metric variable):

```
TRACE history_read_raw_modified: NodeId not registered for HistoryRead
    node_id=ns=2;s=DefinitelyNotARegisteredMetric
```

The wire-level surface for that case is `BadNodeIdUnknown` — the SCADA
client sees the correct error, the gateway logs at TRACE so a noisy
client doesn't flood the log file.

On an inverted time range (`end < start`) — typically a SCADA bug:

```
(no log line — the rejection is silent on the gateway side)
```

The wire-level surface is `BadInvalidArgument` per OPC UA Part 11 §6.4.2.

### Anti-patterns

- **Don't use the in-memory backend for historical data.** `InMemoryBackend`
  is intentionally a lossy non-persistent backend. Its
  `query_metric_history` returns `Ok(Vec::new())` for every window. The
  OPC UA client sees a `Good`-status empty response, so the client
  thinks "no data in range" — which is technically accurate but
  operationally misleading. Use `SqliteBackend` for any deployment
  where HistoryRead matters.

- **Don't expect continuation-point round-tripping.** Story 8-3 does
  not implement OPC UA Part 11 §6.4.4 `ByteString` continuation points.
  Truncated responses surface as
  `data_values.len() == max_history_data_results_per_node` with `Good`
  status. SCADA clients that want more rows must page manually:

  ```text
  // First call:
  HistoryRead(start = T0, end = T1, num_values_per_node = 10000)
  // → 10000 rows back, status Good

  // Second call: bump start by 1µs past the last returned timestamp
  let next_start = last_returned_row.timestamp + 1µs;
  HistoryRead(start = next_start, end = T1, num_values_per_node = 10000)
  // → next page, status Good

  // Loop until data_values.len() < max_history_data_results_per_node
  ```

  The 1-microsecond bump matches the storage layer's microsecond-
  precision timestamp format (`%Y-%m-%dT%H:%M:%S%.6fZ`). Anything
  smaller would re-yield the last row of the previous page.

- **Don't issue HistoryRead with `num_values_per_node = 0`** unless you
  trust your time window. A zero `num_values_per_node` means "use the
  server default" — and if the server is configured with
  `max_history_data_results_per_node = 1_000_000`, a stray query for a
  365-day range against a high-frequency metric could pull back over
  a million rows and saturate the publish pipeline. The
  `max_history_data_results_per_node` cap is the safety net; SCADA
  clients should still set their own cap.

- **Don't rely on `HistoryReadProcessed` (aggregations).** opcgw leaves
  async-opcua's default `BadHistoryOperationUnsupported` for
  `HistoryReadProcessed` and `HistoryReadAtTime`. SCADA clients that
  need min/max/avg/sum over rolling buckets must compute them
  client-side from the raw rows this story returns. Tracked at GitHub
  issue [#98](https://github.com/guycorbaz/opcgw/issues/98).

- **Don't expect `HistoryUpdate` to work.** opcgw is a read-only gateway
  from ChirpStack's perspective; `HistoryUpdate` from the SCADA side
  doesn't make sense and returns `BadHistoryOperationUnsupported`.

### Tuning checklist

For a 7-day retention deployment with FUXA dashboards:

- Set `[storage].retention_days = 7` (the default).
- Leave `[opcua].max_history_data_results_per_node = 10000` (the default)
  unless dashboard latency profiling reveals a need.
- Verify NFR15 by issuing a 7-day query during commissioning; the
  `bench_history_read_7_day_full_retention` benchmark in
  `tests/opcua_history_bench.rs` documents the contract.
- If query latency exceeds 2 s, run `EXPLAIN QUERY PLAN` against the
  underlying SQLite to confirm the `idx_metric_history_device_timestamp`
  index is hit; if not, add a covering index
  `(device_id, metric_name, timestamp)` and re-measure.
- Per-metric retention overrides (e.g. "moisture keeps 30 days, all
  others keep 7") are out of scope for Story 8-3 — tracked at
  GitHub issue [#98](https://github.com/guycorbaz/opcgw/issues/98).

---

## Web UI authentication

**Story 9-1** ships an embedded Axum web server gated by HTTP Basic auth.
The server is **opt-in** (`[web].enabled = false` by default) so existing
operators upgrading from Phase A see no behavioural change unless they
explicitly enable it.

### What it is

A single `Router` mounted at the namespace root with one `Layer` enforcing
Basic auth on every request. Routes:

- `GET /api/health` — minimal smoke endpoint, returns `{"status":"ok"}`.
  Used by integration tests; not operator-facing.
- `GET /` (and any path under it) — static files served from `static/`.
  Story 9-1 ships placeholder HTML; Stories 9-2 / 9-3 / 9-4 / 9-5 / 9-6
  fill them in.

The auth path **reuses** Story 7-2's HMAC-SHA-256 keyed credential digest
(extracted into `src/security_hmac.rs`). Submitted credentials are hashed
under a per-process random key, then constant-time compared against the
digests of the configured credentials. A direct content compare would
leak the credential length via the timing of the comparison; HMAC into
fixed-length digests closes that oracle.

**Credentials are shared with `[opcua]`.** The web server reads
`[opcua].user_name` / `[opcua].user_password` directly — no separate
`[web]` user/password pair. Rationale: the threat model is symmetric (an
operator with LAN access; one credential rotation step covers both
surfaces; one less credential pair for operators to forget to rotate).

### Required reading before enabling

The web UI binds an HTTP listener that any client on the configured
network can probe. Before flipping `[web].enabled = true`, confirm:

1. **You're on a trusted LAN.** Story 9-1 ships HTTP-only — credentials
   transit in cleartext. If your gateway is reachable from the public
   internet, deploy a reverse proxy (nginx, Caddy, Traefik) with TLS
   termination + a deny-all firewall on the gateway port. The default
   `bind_address = "0.0.0.0"` listens on every interface; if a reverse
   proxy on the same host fronts the gateway, override to
   `bind_address = "127.0.0.1"` so the listener is loopback-only.
2. **You've rotated the placeholder password.** The shipped
   `config/config.toml` has a placeholder `[opcua].user_password` value
   the gateway refuses to start with. The same protection extends to
   the web surface (since credentials are shared). Verify your
   `OPCGW_OPCUA__USER_PASSWORD` env var injection before flipping
   `[web].enabled = true`.

### Deployment requirements

The web server's `static/` directory **must** be reachable from the
gateway's working directory at runtime. Story 9-1 resolves
`std::path::PathBuf::from("static")` relative to the gateway's CWD,
so `static/` must live next to the binary or under
`WorkingDirectory` (systemd) / `WORKDIR` (Docker):

- **Local development (`cargo run` from project root):** the shipped
  `static/index.html` etc. are picked up automatically.
- **Docker:** the shipped `Dockerfile` copies `static/` into
  `/usr/local/bin/static` next to the binary. If you customise the
  `Dockerfile`, preserve this `COPY`.
- **systemd:** set `WorkingDirectory=/var/lib/opcgw` (or wherever
  `static/` lives) in the service unit; otherwise `GET /index.html`
  returns 404 even after auth succeeds.

Tracked as a Story 9-X follow-up: a `[web].static_dir` config knob
that lets operators specify the path explicitly. For now the
project root / binary location is the convention.

### Configuration

```toml
[web]
enabled = true              # default false — opt-in to expose
port = 8080                 # default 8080; range 1024-65535
bind_address = "0.0.0.0"    # default "0.0.0.0"; must parse as IpAddr
auth_realm = "opcgw"        # default "opcgw"; max 64 chars, ASCII-only,
                            # no `"`, no `\`, no leading/trailing whitespace
```

Env-var overrides via figment's nested-key convention:

| Knob | Env var |
|---|---|
| `[web].enabled` | `OPCGW_WEB__ENABLED=true` |
| `[web].port` | `OPCGW_WEB__PORT=8080` |
| `[web].bind_address` | `OPCGW_WEB__BIND_ADDRESS=127.0.0.1` |
| `[web].auth_realm` | `OPCGW_WEB__AUTH_REALM=my-gateway` |

`AppConfig::validate` rejects port=0 / port<1024, unparseable
`bind_address`, empty `auth_realm`, `auth_realm` containing `"`, and
`auth_realm` longer than 64 chars. All checks accumulate so a single
startup pass surfaces every violation.

### What you'll see in the logs

**Successful startup** (info-level diagnostic):

```
INFO event="web_server_started" bind_address=0.0.0.0 port=8080 realm="opcgw"
```

**Disabled** (plain info line — no `event=` field; the spec caps Story 9-1
at exactly two structured event names):

```
INFO [web].enabled = false; embedded web server not started (set OPCGW_WEB__ENABLED=true to enable)
```

**Graceful shutdown** (plain info line — same rationale):

```
INFO bind_address=0.0.0.0 port=8080 Embedded web server stopped (graceful shutdown)
```

**Failed authentication** (warn-level audit event — NFR12):

```
WARN event="web_auth_failed" source_ip=192.168.1.42 user=evil-user path="/index.html" reason="user_mismatch" "Web UI authentication failed"
```

The `reason` field discriminates the failure mode for triage:

| Reason | Meaning |
|---|---|
| `missing` | No `Authorization` header. |
| `malformed_scheme` | Header doesn't start with `Basic `. |
| `malformed_base64` | Base64 decode failed (or non-UTF8 bytes). |
| `missing_colon` | Decoded blob has no `:` between user and pass. |
| `user_mismatch` | Submitted username doesn't match the configured one. |
| `password_mismatch` | Username matched but password didn't. |

**The wire response is identical across all reasons** (constant-time
401 + `WWW-Authenticate: Basic realm="..."`); the discrimination exists
only in the audit log for forensic purposes.

### NFR12 source-IP — direct vs. correlated

Story 7-2's OPC UA path needs **two-event correlation** because async-opcua's
`AuthManager` doesn't receive peer `SocketAddr` — operators correlate the
`event="opcua_auth_failed"` audit event against async-opcua's own
`info!`-level "Accept new connection from {addr} (...)" line by timestamp.

Story 9-1's web path **gets the source IP directly** via Axum's
`ConnectInfo<SocketAddr>` extractor — the audit event carries
`source_ip=...` natively. No correlation step needed; the asymmetry is a
strict improvement over the OPC UA path.

The same NFR12 startup warn from Story 7-2 (`event="nfr12_correlation_check"`)
applies to the web path: at log levels stricter than `info` async-opcua's
accept event is filtered out, but the web's `source_ip` field survives at
`warn` (the minimum level the audit event itself uses). Operators running
at `error`/`off` lose the audit trail entirely (their explicit choice).

### Anti-patterns

- **Don't roll your own credential comparison.** The HMAC-keyed digest +
  `constant_time_eq` shape exists to close two specific weaknesses (the
  length oracle of a direct compare; replay across instances). Phase-B
  carry-forward rule (`epics.md:782`).
- **Don't put symlinks in `static/`.** `tower-http = "0.6"`'s `ServeDir`
  doesn't expose a symlink-disable knob (verified against upstream
  source during Story 9-1 review iter-1). On Linux,
  `tokio::fs::File::open` follows symlinks by default. A symlink in
  `static/` pointing outside the directory (e.g. to `/etc/passwd`)
  would let an authenticated user read it. Restrict `static/` to plain
  files. Tracked as a follow-up: a custom `tower::Service` wrapper
  that canonicalises every request path against the canonical
  `static/` root before dispatch would close this gap, but Story 9-1's
  scope didn't include it.
- **Don't introduce a separate `[web]` user/password pair without
  symmetric rotation procedures.** Story 9-1's single-source-of-truth
  shape (credentials live under `[opcua]`) means one rotation step
  covers both surfaces; splitting them creates a footgun where one
  surface gets rotated and the other is forgotten.
- **Don't add `POST` / `PUT` / `DELETE` routes without CSRF protection.**
  Story 9-1 ships only `GET` routes — no CSRF surface. Stories 9-4 / 9-5 /
  9-6 will add mutating routes for application / device / command CRUD;
  those need either strict same-origin policy enforcement (CORS rejecting
  cross-origin requests) or a double-submit cookie / synchronizer-token
  pattern. Audit each before merging.
- **Don't enable the web server without rotating the placeholder
  password.** The shipped `config/config.toml` has a placeholder
  `[opcua].user_password` value the gateway refuses to start with — the
  same protection extends to the web surface (since credentials are
  shared). Verify your `OPCGW_OPCUA__USER_PASSWORD` env var injection
  before flipping `[web].enabled = true`.

### Tuning checklist

- Set `[web].enabled = true` (or `OPCGW_WEB__ENABLED=true`) only after
  verifying the operator's LAN threat model.
- Pick `[web].bind_address = "127.0.0.1"` if a reverse proxy on the same
  host fronts the gateway — no need to listen on every interface.
- Pick `[web].auth_realm` per-deployment (e.g. `"opcgw-prod-east"`) so
  browser credential prompts are distinguishable across environments.
- TLS / HTTPS hardening is **out of scope** for Story 9-1 — tracked at
  GitHub issue [#104](https://github.com/guycorbaz/opcgw/issues/104).
  Until that lands, deploy an upstream reverse proxy if your environment
  requires TLS.
- Per-IP rate limiting (`#88`) becomes structurally relevant once the
  web auth surface is exposed — consider opening a follow-up issue if
  brute-force probing becomes a near-term operator concern.

### API endpoints (Story 9-2+)

All `/api/*` endpoints (`/api/health`, `/api/status`, future
`/api/applications`, `/api/devices`, `/api/commands`) inherit the
same `basic_auth_middleware` that gates the static-file routes.
There is **no anonymous probe surface** — every route, including
`/api/health`, requires the same `[opcua].user_name` /
`[opcua].user_password` credentials. An unauthenticated request is
indistinguishable from any other unauthenticated request: same
401 Unauthorized + same `WWW-Authenticate` header + same
`event="web_auth_failed"` audit event.

Story 9-2 ships `GET /api/status` (gateway health summary read from
the `gateway_status` SQLite table); **Story 9-3 ships `GET /api/devices`**
(per-device live metric values read from the `metric_values` table,
joined against the configured `[[application.device]]` topology);
Stories 9-4 / 9-5 / 9-6 will add more endpoints. **All future routes
inherit the auth middleware automatically** via the `route(...) →
fallback_service(...) → layer(...)` ordering invariant in
`src/web/mod.rs::build_router` — no per-route auth wiring is needed
(and a contributor adding a new route that bypasses the middleware
would have to actively work around the layer composition).

Storage-layer failures on `/api/status`, `/api/devices` (and future
read-side endpoints) return `500 Internal Server Error` with a generic
body (`{"error":"internal server error"}`). The inner error is logged
via `event="api_status_storage_error"` or
`event="api_devices_storage_error"` (`warn`) — operators see the
underlying cause in the gateway log, not in the HTTP response. This
mirrors the NFR7 invariant that error messages must not leak
internal state (SQLite paths, table names, etc.) to clients.

The `/api/devices` JSON contract returns server-side `as_of` plus the
two staleness thresholds (`stale_threshold_secs`, `bad_threshold_secs`)
so the dashboard JS computes per-row staleness client-side without
hard-coding either boundary. The `stale_threshold_secs` field reflects
`[opcua].stale_threshold_seconds` (default 120) — same staleness
contract Story 5-2 established for the OPC UA path. A configured-but-
not-yet-polled metric appears with `value: null` + `timestamp: null`
(rendered as a "missing" badge in the UI) rather than being omitted.

---

## Configuration hot-reload

Story 9-7 adds operator-driven configuration hot-reload via SIGHUP.
Sending `SIGHUP` to the gateway PID re-reads `config/config.toml`
through the same figment chain used at startup (TOML +
`OPCGW_*` env-var overlay), validates the candidate, classifies
which knobs changed, atomically swaps the live `Arc<AppConfig>`
into a `tokio::sync::watch` channel, and notifies the in-process
subscribers (poller, web `AppState`, OPC UA listener stub for
Story 9-8) to pick up the new values at their next safe checkpoint.

### SIGHUP trigger surface

```sh
# Send SIGHUP to the running gateway. The PID is whatever the init
# system (systemd / Docker / supervisor) tracks for the opcgw
# process. systemd users: wire `ExecReload=` to this kill recipe.
kill -HUP "$(pgrep opcgw)"
```

There is **no** `POST /api/config/reload` endpoint in v1 — SIGHUP-only
minimises CSRF / auth-surface area until Stories 9-4 / 9-5 / 9-6
land web-based CRUD endpoints (which will trigger reloads
programmatically by calling the same routine). There is also **no**
filesystem watch (`notify` crate) — editor-save races and
dependency-surface expansion ruled it out.

### Knob taxonomy

The reload routine classifies every knob into one of three buckets
(see `src/config_reload.rs::classify_diff` for the canonical list):

**Hot-reload-safe** — applied without restart. Changes here are
picked up by subscribers at their next safe checkpoint:

- `chirpstack.polling_frequency` — next poll cycle
- `chirpstack.retry`, `chirpstack.delay` — next entry to the Story
  4-4 recovery loop (read-at-entry semantics; in-flight recovery
  unaffected)
- `chirpstack.list_page_size` — next pagination call
- `[opcua].stale_threshold_seconds` — next web-dashboard request
  (**v1 limitation**: the OPC UA path captures the threshold into
  per-variable read-callback closures at startup, so this knob
  affects only the web dashboard's "Good → Uncertain" boundary
  in v1; OPC UA reads continue using the startup value)

**Restart-required** — reloads that mutate any of these are
**rejected** with `event="config_reload_failed" reason="restart_required"`
and a `changed_knob` field naming the offending field. Operators
restart the gateway after applying the change:

- `chirpstack.server_address`, `chirpstack.api_token`, `chirpstack.tenant_id`
  — gRPC channel + interceptor are bound at startup
- `[opcua].host_ip_address`, `[opcua].host_port` — bound socket
- `[opcua].application_name`, `[opcua].application_uri`,
  `[opcua].product_uri` — embedded in OPC UA endpoint discovery
  responses cached by clients
- `[opcua].pki_dir`, `[opcua].certificate_path`,
  `[opcua].private_key_path` — server identity
- `[opcua].max_connections`, `[opcua].max_subscriptions_per_session`,
  `[opcua].max_monitored_items_per_sub`, `[opcua].max_message_size`,
  `[opcua].max_chunk_count` — fed into `async-opcua` `ServerBuilder`
  at startup
- `[opcua].user_name`, `[opcua].user_password` — **v1 limitation**:
  rotating credentials at runtime would require modifying the
  `WebAuthState` and `OpcgwAuthManager` digests captured at startup
  by the auth middleware. The auth-middleware refactor is deferred
  to a future story; v1 classifies credential changes as
  restart-required so a hot-reload that bumps the password is
  rejected loudly rather than silently ignored
- `web.port`, `web.bind_address`, `web.enabled` — bound socket
- `web.auth_realm` — captured into `WebAuthState.realm` at startup
- `storage.database_path`, `storage.retention_days` — DB connection
  pool init; retention is read at startup by the pruner

**Address-space-mutating** — adding / removing applications,
devices, or metrics from `application_list`. **As of Story 9-8 this
is end-to-end functional.** The OPC UA listener
(`src/config_reload.rs::run_opcua_config_listener`) calls
`apply_diff_to_address_space` on every `config_rx.changed()`
notification; the runtime mutation envelope (Phase 1 Q2 transition →
Phase 2 delete → Phase 3 add → Phase 4 rename) updates the running
server's address space without restarting. See
**§ Dynamic OPC UA address-space mutation (Story 9-8)** below for
the full apply seam documentation, audit event shape, and v1
limitations.

### Audit events

Three new structured events are emitted by the SIGHUP listener:

- `event="config_reload_attempted"` (info) — every SIGHUP. Carries
  `trigger="sighup"`. The next line is either `succeeded` or
  `failed`.
- `event="config_reload_succeeded"` (info) — validate + classify +
  swap completed. Carries `trigger`, `changed_section_count`,
  `includes_topology_change`, `duration_ms`. `changed_section_count
  = 0` means the candidate equalled the live config (no swap).
- `event="config_reload_failed"` (warn / audit) — reload was
  rejected. Carries `trigger`, `reason ∈ {validation, io,
  restart_required}`, `changed_knob` (only for
  `reason="restart_required"`), and a sanitised `error` field.
  Per NFR7, the `error` field never includes secrets — the
  `ReloadError::Display` impl is curated to surface only the
  validation diagnostic, file path, or knob name.

The classifier rejects on the **first** restart-required violation
it finds (so the operator gets a single actionable line rather
than a wall of "this also changed" noise). Iterate by fixing each
flagged knob and re-issuing SIGHUP until the reload succeeds.

### Limitations (v1 scope)

- **OPC UA address-space mutation is now functional** as of Story
  9-8 (Story 9-7's "stubbed" note was the v1 limitation that is now
  closed). See § Dynamic OPC UA address-space mutation (Story 9-8)
  below.
- **Credential rotation requires restart in v1** (see
  "Restart-required" above).
- **`[opcua].stale_threshold_seconds` hot-reload affects only the
  web dashboard** in v1. The OPC UA path's per-variable
  read-callback closures capture the threshold at startup AND at
  Story-9-8-runtime-add time — but a reload that does NOT add or
  remove metrics leaves existing closures unchanged (issue #113
  carry-forward).

## Dynamic OPC UA address-space mutation (Story 9-8)

Story 9-8 closes the FR24 requirement (dynamic OPC UA node
mutation at runtime) by extending Story 9-7's hot-reload seam with
an apply pass that walks the topology diff and mutates the running
address space without restarting the gateway.

### Apply seam

The listener at `src/config_reload.rs::run_opcua_config_listener`
calls `crate::opcua_topology_apply::apply_diff_to_address_space` on
every `config_rx.changed()` notification. The function walks four
phases:

1. **Phase 1 — Q2 mitigation** (`set_values(BadNodeIdUnknown)`
   before delete): for every NodeId about to be deleted, emit a
   final `DataValue { value: Some(Variant::Empty), status:
   Some(BadNodeIdUnknown) }` via
   `manager.set_values(subscriptions, …)`. Subscribed clients
   observe an explicit transition; without this the silent-stream-
   on-delete behaviour (per `9-0-spike-report.md:104-127`) leaves
   orphan subscriptions with no programmatic detection path.

   **Why `Variant::Empty`?** async-opcua's default
   `MonitoredItemFilter::None` compares only the `value.value`
   field of incoming DataValues
   (`monitored_item.rs:514-517`); a `value: None` DataValue does
   not trigger a DataChange notification if the previous sample
   was also `None`. `Variant::Empty` is a distinct sentinel that
   forces the filter to pass + carries the `BadNodeIdUnknown`
   status to the client. Load-bearing for
   `tests/opcua_dynamic_address_space_apply.rs::ac2_remove_device_emits_bad_node_id_unknown_before_delete`.

2. **Phase 2 — delete** (single write-lock acquisition): drop the
   doomed NodeIds (metrics → commands → device folders →
   application folders), then call
   `crate::opc_ua_history::remove_read_callback` /
   `remove_write_callback` (option-b stub — see
   "Stale read-callback closure leak" below) and update the
   `node_to_metric` registry.

3. **Phase 3 — add** (single write-lock acquisition): mirror
   `OpcUa::add_nodes` exactly. Issue #99 NodeId scheme
   (`format!("{device_id}/{metric_name}")` for metrics,
   `format!("{device_id}/{command_id}")` for commands), Story 8-3
   AccessLevel + historizing=true invariants, initial-variant
   matching. Then register read/write callbacks via
   `manager.inner().simple().add_read_callback(...)` /
   `add_write_callback(...)` and insert into `node_to_metric`.

4. **Phase 4 — rename** (DisplayName-only `set_attributes`): a
   device with the same `device_id` but a different `device_name`
   produces a DisplayName-only mutation. The NodeId is preserved
   (which is keyed by `device_id`, not `device_name`); clients
   holding references do not see their references invalidated.

   **Iter-2 review IP1 (Story 9-8 code review iter-2, 2026-05-14):**
   Phase 4 is demoted from `Failed`-returning to **warn-and-continue**.
   When `set_attributes` fails, the apply pass emits a separate
   `event="address_space_rename_failed"` warn audit event and
   returns `Applied` — the core address-space state (Phases 1-3) is
   correctly committed; only the cosmetic DisplayName attribute is
   stale. Operators retry by editing `device_name` to a NEW value
   in a future reload (note: per the prev-advancement asymmetry
   below, reverting to the EXACT original name produces no diff
   and no Phase 4 retry; pick a different name or toggle through
   two reloads). Rationale: prior `Failed`-return interacted with
   the listener's prev-advancement guard to create an
   unrecoverable replay loop (Phase 4 fails → keep prev → next
   reload re-tries Phase 2 deletes on already-deleted NodeIds →
   loop forever).

### Lock-hold envelope

Per Story 9-0 Q3 finding (`9-0-spike-report.md:130-160`): 11-node
bulk add under a single write-lock acquisition takes 117 µs;
sampler interval is 100 ms (~850× headroom). At typical opcgw
deployments (≤ 100 devices × ≤ 20 metrics = 2 000 nodes), the
whole-diff apply runs in well under one sampler tick. No mutation
chunking is needed below ~10 000 nodes per diff.

### Audit events

Three structured events are emitted by the listener on every
apply (iter-2 IP1 added the third):

- `event="address_space_mutation_succeeded"` (info / diag) — Phases
  1-3 completed successfully, AND Phase 4 either completed
  successfully or its failure was demoted to warn-and-continue
  (see `address_space_rename_failed` below). Carries all 9 axis
  counts (`added_applications`, `removed_applications`,
  `added_devices`, `removed_devices`, `added_metrics`,
  `removed_metrics`, `added_commands`, `removed_commands`,
  `renamed_devices`) + `duration_ms` (wall-clock cost of
  `apply_diff_to_address_space`). Note: `renamed_devices` here
  counts **attempted** renames; on Phase 4 failure the count is
  still 1+ AND `address_space_rename_failed` fires.
- `event="address_space_mutation_failed"` (warn / audit) — Phase 1
  / 2 / 3 returned `Err`. Carries `reason ∈ {set_attributes_failed,
  add_failed, remove_failed}` (Phase 1 → `set_attributes_failed`;
  Phase 2 → `remove_failed`; Phase 3 → `add_failed`), a sanitised
  `error: %e` field (per NFR7), `duration_ms`, and the same 9 axis
  counts. The warn message is **reason-aware** (iter-3 review TP1):
  Phase 1 failures say `prev not advanced — retry to converge`;
  Phase 2 / 3 partial failures say `prev advanced — address space
  may be in partial-apply state; inspect counts + error and
  reconcile manually if needed` (subsequent reloads will not
  re-attempt this diff).
- `event="address_space_rename_failed"` (warn / diag) — Phase 4
  `set_attributes(AttributeId::DisplayName, …)` returned `Err`
  (iter-2 IP1). The apply pass DOES NOT return Failed — the core
  address-space state (Phases 1-3) was successfully committed and
  is in-sync with new config; only the cosmetic device DisplayName
  attribute is stale. Carries `renamed_count` (number of
  attempted renames) + `error: %e`. Operators retry by editing
  `device_name` to a NEW value in a future reload (see Phase 4
  description above for the prev-advancement caveat).

The Story 9-7-pinned `event="topology_change_detected"` event
continues to fire on every reload that includes a topology change,
preserving the 4-axis field set (`added_devices`,
`removed_devices`, `modified_devices`, `story_9_8_seam`) for
backward compatibility with `tests/config_hot_reload.rs::topology_change_logs_seam_for_9_8`.

### No transactional rollback + prev-advancement asymmetry

Per `9-0-spike-report.md:196`: botched runtime mutations produce
silent subscribers (or BadNodeIdUnknown notifications with Story
9-8's Q2 mitigation), not crashes. Distributed-rollback
machinery is unnecessary — partial-apply failures are reported via
the warn-level audit event.

**Iter-2 review IP1 prev-advancement asymmetry (Story 9-8 code
review iter-2, 2026-05-14):** the listener's `prev` (the
last-applied config snapshot used to compute the next diff)
advances under the following rules:

| Apply outcome | Phases committed | `prev` advances? | Why |
|---|---|---|---|
| `Applied` | All (1+2+3+4-attempted) | YES | Standard success path. |
| `NoChange` | None (empty diff) | YES | No-op. |
| `Failed { reason = set_attributes_failed }` | None (Phase 1 short-circuits before any mutation) | **NO** — keep prev | Phase 1 failure means nothing was committed; retry with the same prev is correct. |
| `Failed { reason = remove_failed }` | Phase 2 partial | YES | Phase 2 has committed some deletes; advancing prev avoids the replay loop where the next reload's Phase 2 hits already-deleted NodeIds and loops permanently. |
| `Failed { reason = add_failed }` | Phase 2 complete + Phase 3 partial | YES | Same logic as `remove_failed` — partial commit, advance prev to break the loop. |

The asymmetric advancement is a deliberate iter-2 design
(superseded iter-1 P2's simpler "advance only on Applied/NoChange"
which created the replay loop on Phase 2/3/4 partial failures).
Operators retry SIGHUP / CRUD to recover from Phase 1 failures;
Phase 2/3 partial-failure recovery is **manual reconciliation**
based on the failed-event's per-axis counts + sanitised error
field. If recovery is impossible, restart the gateway.

### Stale read-callback closure leak (option-b limitation)

async-opcua 0.17.1's `SimpleNodeManagerImpl` exposes
`add_read_callback` / `add_write_callback` but **NOT** the
parallel removal API. The callback registry fields are private to
the upstream crate. When Story 9-8 deletes a variable via
`address_space.write().delete(...)`, the registered closure
remains in the registry holding clones of any captured Arcs
(`Arc<dyn StorageBackend>`, `StatusCache`, `device_id`,
`chirpstack_metric_name`, threshold) — ~120 bytes per closure.

Story 9-8 ships option (b) per the spec authorisation:
`opc_ua_history::remove_read_callback` /
`remove_write_callback` are stub functions that return `false`
and emit a one-time `event="opcgw_stale_read_callback_leak_observed"`
/ `…_write_…` info log per server lifetime so operators can see
the limitation without log flooding. Per
`9-0-spike-report.md:183`: operationally negligible at expected
churn rates (~150 leaks / 30 days = ~18 KB). Upstream FR is
pending — precedent: Story 8-1 issue #94 session-rejected
callback FR.

### v1 limitations

- **BrowseName is not mutable on existing nodes.** A
  `device_name` rename materialises as a Phase 4 DisplayName-only
  `set_attributes` call — the BrowseName stays at the old name.
  Mutating BrowseName would require a delete+re-add cycle that
  would invalidate the NodeId for clients holding references.
- **`application_id` rename = remove + re-add.** The
  `application_id` is the application folder's NodeId; changing
  it would invalidate every device under that application for
  clients holding references. v1 treats it as a remove+re-add
  (the device folder NodeIds are device-scoped, so the device
  IDs survive the application_id change; but the application-
  level subscriptions break).
- **`stale_threshold_seconds` is captured at add-time.** A
  reload that ADDS a metric uses the post-reload threshold; a
  reload that does NOT add/remove metrics leaves existing
  closures unchanged (including the threshold they captured at
  startup). Issue #113 (live-borrow refactor) tracks the long-
  term fix.
- **Stale read-callback closure leak** (see above).
- **No transactional rollback** (see above).
- **Address-space mutation does not affect storage payload
  semantics** (issue #108 is orthogonal — production blocker for
  Epic 9 retro).
- **No HTTP trigger.** SIGHUP-only; web-driven reload arrives with
  Stories 9-4 / 9-5 / 9-6.
- **No filesystem watch.** Editor-save races + `notify`-crate
  dependency-surface expansion ruled it out.

---

## Configuration mutations

Story 9-4 ships the first **state-changing** routes on the embedded
web server: a CRUD surface for `[[application]]` blocks. This
section documents the trust model, the CSRF defence, the TOML
round-trip discipline, and v1 limitations.

### CRUD endpoint surface

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/api/applications` | List configured applications + per-application device counts. |
| `GET` | `/api/applications/:application_id` | Single application detail. 404 on miss. |
| `POST` | `/api/applications` | Create a new (initially empty) application. |
| `PUT` | `/api/applications/:application_id` | Rename an existing application (`application_id` is immutable). |
| `DELETE` | `/api/applications/:application_id` | Remove an application. Rejected with 409 if it still has devices, or if it is the only configured application. |

All five routes inherit Basic auth via the Story 9-1
layer-after-route invariant. State-changing methods (POST/PUT/DELETE)
additionally pass through the Story 9-4 CSRF middleware.

### CSRF defence (v1)

Story 9-1 deferred CSRF to "Stories 9-4/9-5/9-6 mutating routes".
Story 9-4 ships the canonical defence — a hybrid of two checks
hardened by the Story 9-4 review:

1. **Origin same-origin enforcement.** Every POST/PUT/DELETE/PATCH
   request MUST carry an `Origin` header whose `scheme://host[:port]`
   matches one of the configured `[web].allowed_origins` entries.
   The `Referer` header is **NOT** consulted (Story 9-4 review iter-1
   D2-P): per OWASP, Referer is forgeable from non-browser callers
   and unreliable on HTTPS→HTTP downgrade, so trusting it as a
   fallback widens the threat model. Strict-Referrer-Policy clients
   that suppress `Origin` are explicitly rejected; operators who hit
   that case should configure their browser to send `Origin` on
   same-origin XHR/fetch.
2. **JSON-only `Content-Type`.** The body content type must be
   exactly `application/json` (with optional RFC 7231 `;` parameter
   suffix). The `application/json `-followed-by-space-and-garbage
   non-standard form is rejected (iter-1 review P12). This rejects
   `<form>` POST CSRF.

   **Body-less requests still require `Content-Type: application/json`**
   — DELETE without a body is the common case, and uniform CT gating
   is intentional defence-in-depth (an attacker mounting a CSRF DELETE
   via `<form method="post">` would forge a `Content-Type` of
   `application/x-www-form-urlencoded` or `multipart/form-data`, never
   `application/json`). Clients MUST send `Content-Type: application/json`
   on every state-changing method, including DELETE with no body.
   Behavior pinned by `tests/web_device_crud.rs::delete_device_without_content_type_returns_415`
   (Story 9-5 iter-1 review D2). A relaxation must update both the
   middleware and that pinning test in lockstep.

Both checks are applied **after** Basic auth and **before** the
handler. Failures emit `event="application_crud_rejected"
reason="csrf"` warn logs.

Method handling uses a **positive allow-list** (iter-1 review P13):
only GET, HEAD, and OPTIONS bypass CSRF. CONNECT, TRACE, PATCH, and
any custom method are treated as state-changing and CSRF-checked.

Default-port equivalence: `http://gateway.local:80` and
`http://gateway.local` compare equal; same for `https`/`:443` (iter-1
review P10). Browsers omit the default port on standard scheme/port
pairs, so the allow-list normalisation must follow.

Multi-`Origin` header bypass attempts are rejected (iter-1 review
P11): a request with more than one `Origin` header is treated as
malformed and refused.

#### TLS prerequisite (operator action)

The CSRF Origin defence presumes the `Origin` header reaches the
gateway un-tampered. On **plain HTTP over a hostile LAN** (DNS
spoofing, captive-portal MITM, ARP poisoning), an attacker can
falsify the `Origin` header. **Operators deploying opcgw on a
non-trusted network MUST front it with a TLS-terminating reverse
proxy** (nginx, Caddy, Traefik); the reverse proxy must enforce TLS
client→proxy AND must NOT rewrite the `Origin` header before
forwarding. Story 9-1's TLS-via-reverse-proxy stance (issue #104)
remains the canonical recipe.

#### `[web].allowed_origins` knob

Default (when the key is omitted) is
`vec!["http://<bind_address>:<port>"]`. Operators whose browser
hits the gateway via a different URL (hostname, VPN tunnel,
reverse proxy) must extend the list explicitly. Each entry must
parse as `scheme://host[:port]` with no path/query/fragment.
**Hot-reload of this knob is restart-required in v1** (the CSRF
state is captured at router-build time; the live-borrow refactor
is tracked in GH #113).

### TOML round-trip via `toml_edit`

CRUD writes go through `src/web/config_writer.rs::ConfigWriter`
which uses `toml_edit::DocumentMut` to preserve operator-edited
comments, key order, and whitespace on round-trip. The figment
chain (`src/config.rs`) remains the read side; `toml_edit` is the
write side.

Writes are **atomic** via `tempfile::NamedTempFile::new_in(parent)`
+ `persist(target)` (POSIX-atomic rename on the same filesystem).

### Lock acquire-order invariant

CRUD handlers MUST hold `ConfigWriter::lock()` across the entire
`write + reload + (rollback)` sequence so concurrent CRUD requests
cannot lose updates. Story 9-7's reload mutex is independent and
acquired **after** the write_lock — no deadlock risk.

#### SIGHUP-vs-CRUD-snapshot race (operator action — Story 9-4 review iter-1 D4-P)

`ConfigWriter::lock()` only serialises CRUD-vs-CRUD requests. A
SIGHUP-triggered reload runs on the SIGHUP listener task (Story 9-7)
and does NOT contend on `ConfigWriter::write_lock`. Sequence at
risk:

1. CRUD handler acquires `lock()`.
2. **Operator sends SIGHUP** → reload reads disk + swaps watch
   channel.
3. CRUD handler reads `original_bytes` for rollback snapshot —
   captures **post-SIGHUP** bytes.
4. CRUD handler writes its delta.
5. CRUD handler calls `reload()` again; if it fails, rollback
   restores step-3 bytes (NOT pre-SIGHUP).

Pre-SIGHUP TOML state is lost on rollback in this window. **Operator
mitigation:** do not SIGHUP while a CRUD request is in flight; the
window is small (sub-millisecond on a healthy gateway) but
operationally distinguishable. A future hardening story will gate
SIGHUP on the `ConfigWriter::lock()` mutex; tracked alongside
issue #113.

### Rollback discipline on reload failure

When `ConfigReloadHandle::reload()` returns `Err(_)` after a
successful TOML write, the handler rolls back the on-disk TOML to
the pre-write bytes (held in memory) via `ConfigWriter::rollback`.
The HTTP response maps:

- `ReloadError::Validation(_)` → **422 Unprocessable Entity** (with
  the validation message).
- `ReloadError::Io(_)` / `RestartRequired` → **500 Internal Server
  Error** (operator log carries the detailed error).

If the rollback **itself** fails, the gateway logs an
`event="application_crud_rejected" reason="rollback_failed"` warn
event and **poisons the `ConfigWriter`** (Story 9-4 review iter-1
D3-P). All subsequent CRUD requests short-circuit with HTTP 503 +
`event="application_crud_rejected" reason="poisoned"` warn until the
gateway is restarted; operators MUST manually restore the TOML from
a backup before restart, otherwise the next startup will fail
validation. The poisoning is process-local, so a fresh `cargo run`
or container restart clears it.

Story 9-4 review iter-1 P4: the atomic-write also `fsync`s the
tempfile data BEFORE persist + `fsync`s the parent directory AFTER
persist, so a power loss during the write cannot leave the file
zero-length or the rename lost. POSIX-atomic rename + fsync(file) +
fsync(parent_dir) covers the durability gap that `tempfile::persist`
alone leaves.

Story 9-4 review iter-1 D1-P: when the post-write `reload()`
returns `RestartRequired { knob }`, the handler checks whether the
offending knob is in the just-written CRUD delta. If NOT (i.e., the
operator made an unrelated manual edit to the TOML between gateway
start and the CRUD POST), the handler returns 409 + `reason=
"ambient_drift"` and DOES NOT roll back — the operator's manual
edit is preserved. If the offending knob IS in our delta (defence-
in-depth; should not happen for application_list mutations), the
standard 500 + rollback path applies.

### Audit events

Four new event names land with Story 9-4:

- `event="application_created"` (info) — POST succeeded.
- `event="application_updated"` (info) — PUT succeeded.
- `event="application_deleted"` (info) — DELETE succeeded.
- `event="application_crud_rejected"` (warn / audit) — request
  rejected at one of: handler-level validation, CSRF check,
  conflict (delete pre-conditions), reload failure, or rollback
  failure. Carries `reason ∈ {validation, csrf, conflict,
  reload_failed, io, immutable_field, rollback_failed}`.

The grep contract `git grep -hoE 'event = "application_[a-z_]+"' src/`
must return exactly 4 lines.

### `application_id` semantics (case-sensitivity)

**`application_id` matching is case-sensitive throughout the
gateway** (Story 9-4 review iter-2 P37 — documented design call).
`App-1` and `app-1` are distinct identifiers in:

- The pre-write CRUD duplicate check (`src/web/api.rs::create_application`).
- `AppConfig::validate`'s cross-application uniqueness HashSet.
- The poller's per-application bookkeeping.
- The OPC UA address-space NodeId generation.

This means an operator can create both `App-1` and `app-1` and the
gateway will treat them as separate applications. If a future
deployment needs case-insensitive matching, all four sites above
must change in lockstep + a TOML migration must merge any colliding
identifiers. Tracked as a possible future hardening if operators
report case-collision confusion.

### `validate()` amendments (additive)

Story 9-4 makes three additive changes to `AppConfig::validate`
(`src/config.rs:1374-1426`):

1. **Cross-application `application_id` uniqueness** is now
   enforced.
2. **Empty `device_list` per application is now a warn**, not a
   hard error.
3. **Empty `read_metric_list` per device is now a warn**, not a
   hard error.

The two warn-demotions allow POST `/api/applications` to create an
application that the operator subsequently fills in via Story 9-5
endpoints. Existing operator configs with at least one device per
app see no behavioural change.

### Env-var-overrides-disk-edit gotcha

If an operator has set `OPCGW_APPLICATION__N__APPLICATION_NAME="X"`
as an environment variable, a CRUD edit to that same field via
`PUT /api/applications/...` writes the new value to `config.toml`
on disk — but the post-write reload re-runs the figment chain
(TOML + env-var overlay), and the env-var value silently
**overrides** the disk edit. **Operator action:** unset
`OPCGW_APPLICATION__*` env vars before using the web UI to edit
those fields.

### v1 limitations

- **No SQLite-side persistence.** TOML is the single source of
  truth.
- **No cookie-based CSRF token.** Origin/Referer + Content-Type
  defence is sufficient for the LAN single-operator threat model.
- **No cascade-delete.** Operators must remove devices via Story
  9-5 endpoints before deleting the parent application.
- **OPC UA address-space mutation stubbed.** Inherited from
  Story 9-7 — without Story 9-8, a CRUD edit updates the
  dashboard but the OPC UA address space stays at startup state.
- **Best-effort rollback.** Manual operator action required if
  the rollback write itself fails.
- **No ChirpStack-side existence check.** v1 trusts the
  operator-supplied `application_id`.

### Anti-patterns

- **Do NOT roll a custom CSRF token implementation.** When
  stronger CSRF is needed, the canonical upgrade is a
  double-submit pattern signed with the existing per-process
  `hmac_key` (Story 9-1 / 7-2 reuse).
- **Do NOT switch the write side to `toml::to_string`.** It loses
  comments + key order on round-trip.
- **Do NOT bypass the `ConfigWriter::lock()` discipline.**
- **Do NOT use the same `metric_name` (or `chirpstack_metric_name`)
  twice within one device's `read_metric_list`.** The post-#99
  NodeId construction (`format!("{}/{}", device_id, metric_name)`)
  collapses duplicates onto the same address-space slot via
  last-wins semantics — same root-cause class as issue #99 itself.
  Story 9-5 hardens `AppConfig::validate` to reject these
  duplicates at config-load + post-write reload time. The CRUD
  layer also rejects duplicate `metric_name` / `chirpstack_metric
  _name` shapes pre-write where it can.
- **Do NOT serialise a `ChirpstackDevice` back via `toml::Value`
  on PUT.** It silently strips the `[[application.device.command]]`
  sub-table since `UpdateDeviceRequest` doesn't carry commands.
  Story 9-5's PUT mutation operates on `toml_edit::DocumentMut` at
  the table level — replacing only `device_name` and the
  `read_metric` sub-array — so the command sub-table is preserved
  byte-for-byte.

### Device + metric mapping CRUD (Story 9-5)

Story 9-5 lands the second mutating CRUD surface — devices and
their metric mappings, nested under the existing application
surface.

**Endpoint surface:**

| Method | Path | Purpose |
|--------|------|---------|
| `GET` | `/api/applications/:application_id/devices` | List devices under an application + per-device metric counts. |
| `GET` | `/api/applications/:application_id/devices/:device_id` | Single device detail (full metric mapping list). |
| `POST` | `/api/applications/:application_id/devices` | Create a new device with its initial metric mappings. |
| `PUT` | `/api/applications/:application_id/devices/:device_id` | Replace `device_name` and the full `read_metric_list` (`device_id` is immutable). |
| `DELETE` | `/api/applications/:application_id/devices/:device_id` | Remove a device. v1 leaves orphaned `metric_values` / `metric_history` rows in storage; the pruning task eventually removes them via the retention window. |

All five routes inherit Basic auth + the Story 9-4 CSRF defence
(via path-aware audit dispatch — see below). PUT-replaces semantics
mean the operator must ship the full intended `read_metric_list`
(possibly empty) on every PUT; granular per-metric routes are
deferred to a future story.

**Path-aware CSRF audit dispatch:** the CSRF middleware emits
`event="device_crud_rejected" reason="csrf"` for rejections under
`/api/applications/:application_id/devices*` and
`event="application_crud_rejected" reason="csrf"` for the
`/api/applications*` surface. The defence layer itself (Origin
allow-list + JSON-only Content-Type) is byte-for-byte unchanged
from Story 9-4.

**`AppConfig::validate` amendments:** Story 9-5 extends the
validator with two additive rules (modelled on the existing
`seen_device_ids` HashSet pattern at `src/config.rs:1568`):

1. **Per-device `metric_name` uniqueness** — two metrics with the
   same `metric_name` inside ONE device's `read_metric_list` are
   rejected. Without this, the post-#99 NodeId construction would
   collapse them onto the same OPC UA address-space slot via
   last-wins semantics.
2. **Per-device `chirpstack_metric_name` uniqueness** — same
   collision class on the reverse-lookup map keyed by
   `(device_id, chirpstack_metric_name)` at
   `src/opc_ua.rs:1032`.

Cross-device `metric_name` collisions are **allowed** — the
post-#99 NodeId fix at commit `9f823cc` makes this a valid
scenario (two devices `dev-A` and `dev-B` can both expose
`metric_name = "Moisture"` with distinct address-space NodeIds
`dev-A/Moisture` and `dev-B/Moisture`).

**Audit events:** four new event names, parallel to Story 9-4's
shape:

- `event="device_created"` (info) — POST succeeded.
- `event="device_updated"` (info) — PUT succeeded.
- `event="device_deleted"` (info) — DELETE succeeded.
- `event="device_crud_rejected"` (warn / audit) — request rejected.
  Reason set extends Story 9-4's with `application_not_found`
  (POST/PUT/DELETE under a non-existent application_id) and
  `device_not_found` (PUT/DELETE on a non-existent device_id).

The grep contract `git grep -hoE 'event = "device_[a-z_]+"' src/`
must return exactly 4 lines.

**v1 limitations specific to Story 9-5:**

- **No granular per-metric routes.** v1 ships PUT-replaces-device
  with the full metric list. Editing one metric on a 50-metric
  device requires sending the full list back.
- **`device_id` (DevEUI) is immutable.** Renaming would orphan
  every storage row keyed on `device_id` (`metric_values`,
  `metric_history`, `command_queue`, `gateway_status`) — same
  Epic-A-scale change as `application_id` rename. Operator
  workaround: DELETE then POST.
- **No cascade-delete of `metric_values` / `metric_history` on
  DELETE.** v1 leaves orphaned rows in storage. The pruning task
  (Story 2-5a) eventually removes them via the retention window
  (default `[storage].history_retention_days = 7`).
- **OPC UA address-space mutation deferred to Story 9-8.**
  Inherited from Story 9-7 + Story 9-4. The dashboard reflects
  newly created devices immediately; SCADA clients connected via
  OPC UA must reconnect to see the new variables.
- **No ChirpStack-side existence check on `device_id`.** v1
  trusts the operator-supplied DevEUI; the next poll cycle
  surfaces a "device list lookup failed" log if the DevEUI is
  invalid.

### Command CRUD (Story 9-6)

Story 9-6 lands command-CRUD on the `[[application.device.command]]`
sub-table. Five endpoints under
`/api/applications/:application_id/devices/:device_id/commands*`:

- `GET    /api/applications/:application_id/devices/:device_id/commands`
- `GET    /api/applications/:application_id/devices/:device_id/commands/:command_id`
- `POST   /api/applications/:application_id/devices/:device_id/commands`
- `PUT    /api/applications/:application_id/devices/:device_id/commands/:command_id`
- `DELETE /api/applications/:application_id/devices/:device_id/commands/:command_id`

The `:command_id` path segment is an **integer** (`i32` — matches
`DeviceCommandCfg.command_id`); non-numeric, negative, or zero
path segments return `400 Bad Request` with
`event="command_crud_rejected" reason="validation" field="command_id"`.

**CSRF defence:** inherits Story 9-4's Origin + JSON-only
Content-Type middleware unchanged; Story 9-6 completes the
path-aware audit dispatch with a literal `command_crud_rejected`
warn arm at the rejection-emission sites (parallel to the
`device_crud_rejected` / `application_crud_rejected` arms from
Stories 9-4 / 9-5).

**Validate-side amendments (additive in `AppConfig::validate`):**

- Per-device `command_id` uniqueness — two commands sharing
  `command_id` within ONE device collide on the OPC UA NodeId at
  `src/opc_ua.rs:1059` (`NodeId::new(ns, command.command_id as u32)`),
  silently overwriting via `HashMap::insert` last-wins. Same root-
  cause class as the metric-name uniqueness checks added in Story
  9-5, but per-device-scoped.
- Per-device `command_name` uniqueness — defends operator-driven
  addressing in the web UI and any future `command_name`-keyed
  lookup.
- Cross-device same `command_id` is **allowed** — the device
  folder NodeId namespaces the command, so two devices can both
  have `command_id = 1` without collision.

**Body field validation:**

- `command_id`: positive `i32` (`>= 1`); rejected with 400 if
  `<= 0` (zero is reserved-as-unset by convention).
- `command_name`: non-empty trimmed, length `[1, 256]`, char-class
  via `is_valid_app_name_char` (ASCII alphanumerics, `'-'`, `'_'`,
  `'.'`, space, parentheses — operator-facing label, not a URL
  identifier).
- `command_port`: LoRaWAN application f_port range `1..=223`,
  validated via `DeviceCommand::validate_f_port` at
  `src/storage/types.rs:155` (single source of truth — no parallel
  range check rolled in the handler).
- `command_confirmed`: bool; type-checked by serde at deserialise
  time. No further handler-level validation.

**Audit events:** four new event names, parallel to Stories 9-4 /
9-5:

- `event="command_created"` (info) — POST succeeded.
- `event="command_updated"` (info) — PUT succeeded.
- `event="command_deleted"` (info) — DELETE succeeded.
- `event="command_crud_rejected"` (warn / audit) — request
  rejected. Reason set extends Stories 9-4 / 9-5 with
  `command_not_found` (PUT/DELETE/GET on a non-existent
  `:command_id` under a known device).

The grep contract
`git grep -hoE 'event = "command_[a-z_]+"' src/` must return
exactly 4 lines.

**v1 limitations specific to Story 9-6:**

- **No payload-template editing.** `DeviceCommandCfg` has only
  4 fields (`command_id`, `command_name`, `command_confirmed`,
  `command_port`); there is no `payload_template` field on the
  struct today. Adding one would be a schema change touching
  the OPC UA command-emission path at `src/opc_ua.rs:1856-1928`.
  Deferred to a future story.
- **No `[command_validation.device_schemas]` CRUD.** The
  schema-driven validation surface (Story 3-2 /
  `src/command_validation.rs`) is a separate config section
  keyed by `device_id` under `[command_validation]`. Editing
  schemas would require a parallel CRUD surface; deferred.
- **`command_id` is immutable.** Renaming would orphan
  `command_queue` rows keyed by `command_id`. Operator
  workaround: DELETE then POST.
- **No cascade-delete of pending `command_queue` rows on
  command DELETE.** v1 leaves orphaned rows in storage.
- **OPC UA address-space mutation deferred to Story 9-8.**
  Inherited from Stories 9-7 / 9-4 / 9-5. The dashboard reflects
  new commands immediately; SCADA clients connected via OPC UA
  must reconnect to see the new command nodes.

### Anti-patterns (Story 9-6 extension)

- **Do NOT** add cross-device `command_id` uniqueness — the
  per-device folder NodeId namespaces the command, so cross-
  device same-`command_id` is a valid scenario the tests pin.
- **Do NOT** serialise `DeviceCommandCfg` back to TOML via
  `toml::Value` — the TOML mutation MUST be done at the table
  level via `toml_edit::DocumentMut` so that sibling
  `[[application.device.read_metric]]` sub-tables are preserved
  byte-for-byte (load-bearing regression guard for Story 9-5).
- **Do NOT** add `Serialize` to `DeviceCommandCfg` — use a
  parallel `CommandResponse` struct in `src/web/api.rs` (Story
  9-5's `MetricMappingResponse` pattern).
- **Do NOT** add `scopeguard` as a dependency — the chmod-
  cleanup pattern in fault-injection tests is hand-rolled
  inline RAII (Drop-impl struct), per Story 9-5's
  `tests/web_device_crud.rs:1578` precedent.

---

## References

- Story 7-1 spec: `_bmad-output/implementation-artifacts/7-1-credential-management-via-environment-variables.md`
- Story 7-2 spec: `_bmad-output/implementation-artifacts/7-2-opc-ua-security-endpoints-and-authentication.md`
- Story 7-3 spec: `_bmad-output/implementation-artifacts/7-3-connection-limiting.md`
- Story 9-1 spec: `_bmad-output/implementation-artifacts/9-1-axum-web-server-and-basic-authentication.md`
- Story 9-7 spec: `_bmad-output/implementation-artifacts/9-7-configuration-hot-reload.md`
- PRD requirements: FR42 (env-var injection), NFR7 (no secrets in logs),
  NFR8 (no real credentials in default config), NFR24 (env override for
  all secrets), FR19 (multi-policy OPC UA endpoints), FR20 (OPC UA user
  auth), FR45 (PKI layout), NFR9 (private-key 0o600), NFR12 (failed-auth
  audit trail), FR44 (connection limiting), FR50 (web Basic auth),
  NFR11 (web auth before any change), FR41 (mobile-responsive web UI) in
  `_bmad-output/planning-artifacts/prd.md`
- Configuration reference: [`docs/configuration.md`](configuration.md)
- Deferred follow-ups: `_bmad-output/implementation-artifacts/deferred-work.md`

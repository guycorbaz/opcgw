# Inventory API (`/api/inventory/*`)

Story C-1 of Epic C (Auto-Discovery and Web-First Configuration) ships
three GET-only HTTP endpoints that proxy ChirpStack's
`ApplicationService.List`, `DeviceService.List`, and
`InternalService.StreamDeviceEvents` RPCs. The picker UI (Story C-2) and
drift view (Story C-4) consume these endpoints to render named lists of
ChirpStack resources without the browser talking to ChirpStack directly
and without hammering ChirpStack on every operator click.

All three endpoints are basic-auth gated (same middleware as
`/api/applications` etc.) and CSRF-exempt (GET-only, read-only — matches
the existing `/api/*` GET convention).

## Endpoints

### `GET /api/inventory/applications`

List applications for the configured tenant. Cached server-side with
TTL = `[chirpstack].inventory_cache_ttl_seconds` (default 60 s).

**Query parameters:**

| Name      | Required | Default | Notes                                                          |
| --------- | -------- | ------- | -------------------------------------------------------------- |
| `refresh` | no       | (unset) | `true` / `1` → force a fresh ChirpStack fetch (bypass cache).  |

**Response — 200 OK:**

```json
{
  "items": [
    { "id": "ae2012c2-...", "name": "Arrosage", "description": "Watering system" },
    { "id": "194f12ab-...", "name": "Bâtiments", "description": "" }
  ],
  "count": 2,
  "cache_status": "hit",
  "fetched_at": "2026-05-22T12:00:00+00:00"
}
```

- `items` is sorted by `name` ASCENDING, case-insensitive
  (`to_lowercase()` for the sort key so "Arrosage" and "Bâtiments" sort
  alongside ASCII-only names).
- `cache_status` ∈ `{ "hit", "miss", "refresh", "bypassed" }`.
  - `hit` — served from cache (no ChirpStack call).
  - `miss` — cache absent or expired; fresh ChirpStack call.
  - `refresh` — `?refresh=true` was set; forced fresh fetch.
  - `bypassed` — `[chirpstack].inventory_cache_ttl_seconds = 0` (cache disabled).
- `fetched_at` — RFC3339 timestamp of the latest ChirpStack fetch
  backing this response (== last refresh if served from cache).
- Empty list returns `200 OK` with `items: []` and `count: 0`. **Not
  404** — zero applications is a valid state (fresh post-C-0 gateway).

**Response — 502 Bad Gateway** (ChirpStack unreachable / auth / gRPC error):

```json
{ "error": "chirpstack_error", "reason": "chirpstack_unreachable" }
```

`reason` ∈ `{ "chirpstack_unreachable", "chirpstack_auth_failed", "chirpstack_grpc_error", "shutdown_cancellation" }`. The `shutdown_cancellation` value (iter-2 P2) fires when a picker request is in flight during graceful gateway shutdown — not a real ChirpStack fault; suppress alerts during planned restarts.

### `GET /api/inventory/devices?application_id=<uuid>`

List devices under the given application. Cache scope: `(tenant_id, application_id)`.

**Query parameters:**

| Name             | Required | Default | Notes                       |
| ---------------- | -------- | ------- | --------------------------- |
| `application_id` | yes      | —       | UUID of the parent app.     |
| `refresh`        | no       | (unset) | Same semantics as above.    |

**Response — 200 OK:**

```json
{
  "items": [
    {
      "dev_eui": "a84041b8a1867e20",
      "name": "WaterFlowSensor",
      "description": "Main valve",
      "device_profile_name": "Dragino LSE01",
      "last_seen_at": "2026-05-22T11:55:43+00:00"
    }
  ],
  "count": 1,
  "cache_status": "miss",
  "fetched_at": "2026-05-22T12:00:00+00:00",
  "application_id": "ae2012c2-..."
}
```

`device_profile_name` and `last_seen_at` may be `null` (device with no
profile assigned / device never seen).

**Response — 400 Bad Request** (missing `application_id`):

```json
{ "error": "missing_query_param", "param": "application_id" }
```

### `GET /api/inventory/uplinks?dev_eui=<16-hex>&limit=<N>`

Read recent uplinks for a device by opening
`InternalService.StreamDeviceEvents` with a bounded read window. Used by
the picker UI to infer wire types for sensor keys.

**Not cached** — uplinks are freshness-sensitive (operators want to see
the latest decoded keys, e.g. to verify a codec update landed).

**Query parameters:**

| Name      | Required | Default | Notes                                                 |
| --------- | -------- | ------- | ----------------------------------------------------- |
| `dev_eui` | yes      | —       | 16 hex characters; colons / dashes accepted as separators; case-insensitive. |
| `limit`   | no       | 10      | Maximum number of uplinks to collect. Capped at 50.   |

**Response — 200 OK:**

```json
{
  "items": [
    {
      "received_at": "2026-05-22T11:59:30+00:00",
      "decoded_object": { "temperature": 22.5, "battery": 87 },
      "f_port": 1,
      "f_cnt": 4521
    }
  ],
  "count": 1,
  "observed_keys": [
    { "key": "battery", "wire_type": "Int", "sample_value": 87 },
    { "key": "temperature", "wire_type": "Float", "sample_value": 22.5 }
  ],
  "dev_eui": "a84041b8a1867e20",
  "fetched_at": "2026-05-22T12:00:00+00:00"
}
```

`observed_keys` aggregates the top-level keys across all uplinks in
the response, with `wire_type` inferred per AC#4:

- All values are JSON booleans → `Bool`.
- All values are JSON numbers AND every number is a mathematical
  integer fitting in `i64` → `Int`.
- All values are JSON numbers but at least one is fractional or
  out-of-range for `i64` → `Float`.
- All values are JSON strings → `String`.
- Heterogeneous mix → `String` (and an
  `inventory_observed_key_heterogeneous` audit event fires).
- All `null` values → `String` (conservative default).

**Bounded read window:** the stream is held open for at most
`[chirpstack].inventory_uplink_max_wait_seconds` (default 5, range
1..=60) OR until `limit` uplinks are collected, whichever comes first.
Zero uplinks within the window returns `200 OK` with empty arrays
(NOT an error — natural for a device that's currently silent).

**Response — 400 Bad Request:**

- Missing or invalid `dev_eui`: `{ "error": "missing_query_param", "param": "dev_eui" }` or `{ "error": "invalid_dev_eui", "hint": "..." }`.
- `limit > 50`: `{ "error": "limit_out_of_range", "cap": 50, "received": <N> }`.

## Caching contract

- `applications` is cached under key `(tenant_id)`.
- `devices` is cached under key `(tenant_id, application_id)`.
- `uplinks` is NOT cached.
- Default TTL: `[chirpstack].inventory_cache_ttl_seconds = 60` (env-overridable
  via `OPCGW_CHIRPSTACK__INVENTORY_CACHE_TTL_SECONDS`). `0` disables the
  cache.
- TTL is captured at boot in the `InventoryCache` struct (restart-required).
- Race-condition guard: concurrent requests on the same expired key
  coalesce into a single ChirpStack call (the second caller awaits the
  first's completed insert under a `tokio::sync::Mutex`).
- Cache invalidation on CRUD writes: any successful
  `POST/PUT/DELETE /api/applications` removes the
  `(tenant_id)` entry; same for `.../devices` with the corresponding
  `(tenant_id, application_id)` entry.

## Audit events

| Event                                       | When                                                                 |
| ------------------------------------------- | -------------------------------------------------------------------- |
| `inventory_query`                           | Cache miss / refresh / bypassed read succeeded.                      |
| `inventory_query_failed`                    | ChirpStack call failed (unreachable / auth / gRPC error).            |
| `inventory_cache_invalidated`               | CRUD write triggered cache invalidation.                             |
| `inventory_observed_key_heterogeneous`      | Wire-type inference fell back to `String` due to mixed types.        |

**Cache hits emit NO audit event** — this keeps the audit log volume
bounded by `1 / TTL × active_sessions` rather than `clicks × sessions`,
which is the load-bearing scope decision for C-1 (avoid log noise on
operator-driven picker UX).

## Configuration

```toml
[chirpstack]
# Server-side TTL cache for /api/inventory/applications and
# /api/inventory/devices. Default 60 s; 0 disables.
inventory_cache_ttl_seconds = 60

# Max wait window for /api/inventory/uplinks (bounded read against
# InternalService.StreamDeviceEvents). Default 5 s; range 1..=60.
inventory_uplink_max_wait_seconds = 5
```

Both fields can be overridden via environment variables:
- `OPCGW_CHIRPSTACK__INVENTORY_CACHE_TTL_SECONDS`
- `OPCGW_CHIRPSTACK__INVENTORY_UPLINK_MAX_WAIT_SECONDS`

## Picker-event audit endpoint (Story C-2)

`POST /api/audit/picker-event`

Thin endpoint accepting client-attributable picker audit events. No
state mutation; the handler validates the `event` name against an
allowlist, sanitises the per-event `fields` map, and emits a single
`tracing::info!(event=…, source="web_picker", …)` line into the
canonical audit stream. Basic-auth gated and CSRF-protected (Origin +
JSON-only Content-Type, same as the CRUD surface).

Request body:

```json
{
  "event": "picker_opened",
  "fields": {
    "picker_resource": "application",
    "cache_status": "miss"
  }
}
```

Responses:
- `204 No Content` — accepted, audit emitted.
- `400 Bad Request` — unknown `event` name; emits
  `event="picker_audit_rejected" reason="unknown_event"`.
- `401 Unauthorized` — missing basic auth.
- `403 Forbidden` — CSRF reject (missing/cross-origin Origin); emits
  `event="picker_audit_rejected" reason="csrf"`.
- `415 Unsupported Media Type` — Content-Type is not
  `application/json`.

### Accepted events

| `event`                    | Required scope fields                                | Optional fields |
| -------------------------- | ---------------------------------------------------- | --------------- |
| `picker_opened`            | `picker_resource`                                    | `application_id`, `dev_eui`, `cache_status` |
| `picker_manual_fallback`   | `picker_resource`, `reason`                          | `error_detail` |

`picker_resource` is one of `application`, `device`, `uplink`.

`reason` (manual_fallback) is one of:
- `operator_choice` — operator clicked the "Switch to manual entry"
  toggle.
- `chirpstack_unreachable` — inventory endpoint returned 502; the
  client auto-flipped to manual.
- `chirpstack_error` — inventory endpoint failed for a non-502 reason.
- `chirpstack_empty` — inventory returned zero items.
- `no_recent_uplinks` — `/api/inventory/uplinks` returned an empty
  `observed_keys` aggregate within the wait window.

Unknown fields are silently dropped; string values are length-capped
at 256 bytes; nested objects/arrays are stripped.

### `picker_metadata` field on the metric-create path

When a metric is added via the inventory picker UI, the client attaches
a `picker_metadata` envelope to each metric inside the
`POST|PUT /api/applications/<app>/devices[/<dev>]` request body:

```json
{
  "device_id": "a84041b8a1867e21",
  "device_name": "WaterFlowSensor",
  "read_metric_list": [
    {
      "metric_name": "water_flow",
      "chirpstack_metric_name": "water_flow",
      "metric_type": "Float",
      "picker_metadata": {
        "inferred_type": "Float",
        "operator_chosen_type": "Float",
        "sample_values_count": 8
      }
    }
  ]
}
```

The handler emits one `event="metric_wire_type_inferred"` info-level
audit per metric carrying a `picker_metadata` envelope, then drops the
envelope before persisting to TOML. Manual-entry metrics (no envelope)
stay silent — they remain covered by the existing
`event="device_created"` / `device_updated` audits.

**Audit field vocabulary.** `inferred_type` and `operator_chosen_type`
in the emitted audit line are constrained to a closed set:
- `Float`, `Int`, `Bool`, `String` — the legal `OpcMetricTypeConfig`
  values (passed through verbatim from the request).
- `unknown` — the request supplied a non-empty value that did not
  match any of the four legal types (the metric itself was NOT
  rejected; the optional audit envelope is informational and a typo
  in this field should never block the CRUD write).
- `unset` — the request omitted the field (`Option::None` deserialized).

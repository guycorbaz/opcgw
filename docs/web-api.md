# Web API — error-response shapes

This document pins the JSON wire-shapes the embedded web server returns for non-2xx responses on `/api/applications`, `/api/applications/{app}/devices`, and `/api/applications/{app}/devices/{dev}/commands`. The endpoints themselves (verbs, paths, success bodies) are documented inline in `src/web/api.rs` and cross-referenced from each story spec.

## Standard error-response envelope

Most non-2xx responses use the `ErrorResponse` struct in `src/web/api.rs`:

```json
{
  "error": "<short machine-readable code>",
  "hint": "<actionable operator guidance, optional>"
}
```

The `error` field is a short stable code intended for log greps and inline UI mapping; the `hint` is human-readable advice on how to recover. Additional fields are added by specific endpoints (e.g. `errors` array on validation failures; `available_actions` on transient conflicts).

## Story C-3 — duplicate-rejection contract

When a `POST` or `PUT` would introduce a same-level duplicate that the validator rejects (duplicate `application_id`, duplicate `device_id` within an application, duplicate `metric_name` / `chirpstack_metric_name` within a device, duplicate `command_id` / `command_name` within a device), the server returns **HTTP 409 Conflict** with this body shape:

```json
{
  "error": "duplicate",
  "field": "<one of: application_id, device_id, metric_name, chirpstack_metric_name, command_id, command_name>",
  "value": "<the conflicting value, echoed back>",
  "scope": "<the resource scope in which the duplicate was detected>",
  "hint": "<short suggested next action>"
}
```

The `scope` field follows a predictable shape so the picker UI can locate where to surface the inline error:

| Resource | `scope` format | Example |
|---|---|---|
| Application | `"application_list"` | `"application_list"` |
| Device | `"application:<application_id>"` | `"application:app-1"` |
| Metric mapping | `"device:<device_id>"` | `"device:dev-1"` |
| Command | `"device:<device_id>"` | `"device:probe-1"` |
| Post-write reload (any resource) | `"reload"` | `"reload"` |

The `"reload"` sentinel surfaces in the rare race where a CRUD pre-flight does not detect a duplicate that the post-write `AppConfig::validate()` does (e.g. a concurrent operator hand-edit during the pre-flight-to-write window introduced a duplicate elsewhere in the TOML; or a duplicate exists in a cross-cutting scope the handler doesn't iterate). The `field` and `value` are still present and accurate — `scope: "reload"` indicates the source of the rejection rather than a specific resource scope. Audit consumers see the matching `<resource>_crud_rejected reason="conflict" conflict_kind="duplicate"` event with sibling `duplicate_field` / `duplicate_value` structured-log fields for forensics. Picker UIs that don't recognise the `"reload"` sentinel should fall back to a generic "duplicate detected" inline message (the `field` + `value` are still actionable).

### Sibling audit event

Every duplicate-rejection 409 is mirrored by a `warn`-level structured-log event with the same disambiguating fields. Audit consumers can grep for either pattern depending on operational need:

- `event="application_crud_rejected" reason="conflict" conflict_kind="duplicate"` — application-level duplicate.
- `event="device_crud_rejected" reason="conflict" conflict_kind="duplicate"` — device-level OR metric-mapping duplicate (the convention is to scope all device-subordinate rejections under `device_crud_rejected`).
- `event="command_crud_rejected" reason="conflict" conflict_kind="duplicate"` — command-level duplicate.
- `event="config_reload_rejected" reason="conflict" conflict_kind="duplicate"` — hot-reload (SIGHUP-triggered `AppConfig::validate()`) rejected because the operator's TOML hand-edit introduced a duplicate. No `source_ip` field on this variant (reload has no requester).

See [`logging.md`](logging.md) § Audit-event taxonomy → "Story C-3 — `conflict_kind` sub-field" for the full enum (`duplicate`, `malformed_existing_block`, plus the existing `cascade_blocked` and `empty_application_list` reason-mirrors).

### Why a separate `conflict_kind` instead of distinct `reason` values

The audit-event field `reason="conflict"` was already in place before C-3 for the malformed-TOML-cleanup path. Renaming it would have broken every operator grep pipeline. C-3 added the disambiguating `conflict_kind` field so:

- Existing grep pipelines (`reason="conflict"`) keep working.
- New pipelines that need to distinguish duplicate-vs-cleanup-vs-other-conflict-class events filter on `conflict_kind="..."` in addition.

### POSITIVE-PATH guarantees (regression-guards)

Two patterns explicitly remain allowed (verified by `tests/web_duplicate_prevention.rs`):

- **Same DevEUI under different applications** is allowed. Operators expose one physical sensor under multiple OPC UA namespaces (e.g. during LoRaWAN application-ownership migration).
- **Same `chirpstack_metric_name` on different devices** is allowed. The common case — multiple sensors of the same kind (`temperature` on every probe).

The validator scopes its uniqueness HashSets per-application (for `device_id`) and per-device (for metric/command identifiers). Cross-scope reuse is intentional and tested.

## Story C-4 — inventory drift view

The drift view compares opcgw's configured applications, devices, and metrics against ChirpStack's current inventory and classifies each resource into one of four buckets. It is operator-triggered (no background polling) and read-only — the endpoint never mutates opcgw state. Action buttons in the UI dispatch through the existing CRUD paths.

### `GET /api/inventory/drift`

Basic-auth gated, CSRF-exempt (GET-only). Forces `?refresh=true` on every underlying C-1 fetch so the comparison is never against a stale cache.

Response body:

```json
{
  "applications": [
    {
      "class": "ok|stale|available|drifted",
      "opcgw":      { "application_id": "...", "application_name": "..." },
      "chirpstack": { "id": "...", "name": "..." },
      "drift_details": { "reason": "name_differs", "opcgw_name": "...", "chirpstack_name": "..." }
    }
  ],
  "devices": [
    {
      "class": "...",
      "application_id": "...",
      "opcgw":      { "device_id": "...", "device_name": "..." },
      "chirpstack": { "dev_eui": "...", "name": "...", "last_seen_at": "..." },
      "drift_details": { "reason": "name_differs", "opcgw_name": "...", "chirpstack_name": "..." }
    }
  ],
  "metrics": [
    {
      "class": "...",
      "application_id": "...",
      "device_id": "...",
      "opcgw":              { "chirpstack_metric_name": "...", "metric_name": "...", "metric_type": "Float" },
      "chirpstack_observed": { "key": "...", "inferred_wire_type": "Float", "sample_value": ... },
      "drift_details": { "reason": "wire_type_mismatch|not_in_recent_uplinks", "opcgw_type": "Int", "inferred_type": "Float" }
    }
  ],
  "fetched_at": "<RFC3339 timestamp>",
  "chirpstack_reachable": true,
  "summary": { "ok": N, "stale": N, "available": N, "drifted": N, "total": N }
}
```

The `class` discriminator follows the 4-class matrix:

| Class | In opcgw? | In ChirpStack? | Names match? | `opcgw` field | `chirpstack` field |
|---|---|---|---|---|---|
| `ok` | yes | yes | yes | populated | populated |
| `stale` | yes | no | n/a | populated | `null` |
| `available` | no | yes | n/a | `null` | populated |
| `drifted` | yes | yes | NO | populated | populated |

#### Metric-row `drift_details.reason` values

- `name_differs` — applications/devices only. Populates `opcgw_name` + `chirpstack_name`.
- `wire_type_mismatch` — metrics only. Configured `metric_type` doesn't match the wire-type inferred from observed uplinks. Populates `opcgw_type` + `inferred_type`.
- `not_in_recent_uplinks` — metrics only. Configured `chirpstack_metric_name` hasn't appeared in the last 10 uplinks. Classified as `stale` BUT with this soft reason so the UI can colour-code it differently (codecs may emit keys conditionally).

#### Request cost and latency

Each `GET /api/inventory/drift` call triggers **1 + N_apps + N_both_devices** sequential ChirpStack round-trips: one applications fetch, one per-app device fetch for every app in the union of opcgw + ChirpStack sets, and one uplinks fetch for every device present in both sets. Worst-case request latency ≈ `N_both_devices × inventory_uplink_max_wait_seconds`. For large inventories, keep `inventory_uplink_max_wait_seconds` at 5 s or below (the config field defaults to 5 s). Because the endpoint is operator-triggered with no background polling, this cost is bounded to explicit operator visits.

#### ChirpStack-unreachable graceful degradation

When ANY underlying ChirpStack fetch fails (applications list, per-app device list, or per-device uplinks), the endpoint returns **HTTP 200** with:

- `chirpstack_reachable: false`
- All opcgw-side rows emitted as `class: "ok"` placeholders with `chirpstack: null` (since we can't classify without ChirpStack data).
- `summary.total = summary.ok = (opcgw applications + devices + metrics)`.

The UI's responsibility is to recognise this state and disable destructive action buttons + hide `[Add to opcgw]` buttons + surface a banner with a retry button.

### `POST /api/audit/drift-action`

Thin client-side audit-event recorder for operator intent. Basic-auth gated, CSRF-protected (same Origin / Content-Type contract as `/api/audit/picker-event`). 4 KiB body limit.

Request body:

```json
{
  "event": "drift_action|drift_dismissed",
  "fields": { /* per-event allowlist; unknown fields silently dropped */ }
}
```

| `event` | Allowed `fields` keys |
|---|---|
| `drift_action` | `action`, `resource_type`, `application_id`, `device_id`, `metric_name`, `operator_choice` |
| `drift_dismissed` | `class`, `resource_type`, `application_id`, `device_id`, `metric_name`, `drift_reason` |

Returns **204 No Content** on success. **400 Bad Request** with `{"error": "unknown drift event name '...'", "hint": "..."}` on unrecognised event names. Field values are length-capped at 256 bytes (UTF-8-aware boundary) before audit emission. The actual CRUD execution (`DELETE`, `PUT`) emits its own audit event — `drift_action` is the layer-above operator-intent signal.

### Deep-link contract from the drift view

The drift view's `[Add to opcgw]` buttons navigate to the C-2 picker pages with the chosen resource pre-selected via query parameters:

| Resource | Target URL |
|---|---|
| Application | `/applications.html?prefill_app_id=<id>&prefill_name=<name>` |
| Device | `/devices-config.html?prefill_app_id=<app_id>&prefill_dev_eui=<dev_eui>&prefill_name=<name>` |
| Metric | `/devices-config.html?prefill_app_id=<app_id>&prefill_dev_eui=<dev_eui>&prefill_metric_key=<key>` |

`applications.js` and `devices-config.js` consume these parameters on page load and pre-select the picker option / pre-fill the name input / pre-tick the metric checkbox. If the prefilled `application_id` or `dev_eui` isn't in the picker's current option set (e.g. operator clicked, then ChirpStack inventory changed), the page falls back to manual mode with the prefilled id populated.

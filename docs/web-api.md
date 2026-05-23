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

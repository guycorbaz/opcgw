# Story C-1: ChirpStack Inventory Query Layer

| Field           | Value                                                                                                       |
| --------------- | ----------------------------------------------------------------------------------------------------------- |
| Story key       | `C-1-chirpstack-inventory-query-layer`                                                                      |
| Epic            | C — Auto-Discovery and Web-First Configuration (post-v2.0 GA)                                               |
| FRs             | none (Epic C is post-PRD; see memory `project_epic_c_auto_discovery_vision.md`)                             |
| Status          | review                                                                                                      |
| Created         | 2026-05-21                                                                                                  |
| Source epic     | `_bmad-output/planning-artifacts/epics.md § Epic C § Story C.1`                                             |
| Depends on      | C-0 (empty-config bootstrap) — gateway must be able to start with no `[[application]]` for the picker UX    |
| Tracking        | GitHub issue `#__` — user opens out-of-band; capture number in Dev Notes once known                         |

---

## User Story

As **opcgw's web UI** (and any future automation consumer),
I want internal Rust helpers and HTTP endpoints that proxy ChirpStack's `ApplicationService.List`, `DeviceService.List`, and `InternalService.StreamDeviceEvents` RPCs, plus a server-side TTL cache,
So that the inventory picker UI (Story C-2) can render named lists of ChirpStack resources without the browser talking to ChirpStack directly and without hammering ChirpStack on every operator click.

---

## Story Context

### Why C-1 is the foundation for C-2 / C-4

Epic C's whole UX premise is that operators configure opcgw by picking from named lists fetched from ChirpStack at fill-the-form time. Two downstream stories consume the inventory:

- **C-2** (pickers UI) reads `/api/inventory/applications`, `/api/inventory/devices?application_id=…`, and `/api/inventory/uplinks?dev_eui=…` to render dropdowns. The picker is useless if these endpoints don't exist.
- **C-4** (drift view) reads the same endpoints with `?refresh=true` to diff opcgw's configured state against ChirpStack's current state.

C-1 is also the **rate-limiting choke point**. Picker sessions can fire 20+ inventory queries (open Add Application → switch app → open Add Device → switch device → open Add Metric → scroll uplinks → cancel → reopen, repeat). Without caching, every click of every operator session hits ChirpStack. With caching, only cache misses hit ChirpStack — bounded by `1 / inventory_cache_ttl_seconds × active_sessions`, predictably small.

### Existing infrastructure that C-1 builds on

Two helpers already exist in `src/chirpstack.rs` but are gated by `#[allow(dead_code)]` (placeholders the polling path doesn't use):

- `ChirpstackPoller::get_applications_list_from_server` at `src/chirpstack.rs:2135` → returns `Vec<ApplicationDetail>`.
- `ChirpstackPoller::get_devices_list_from_server` at `src/chirpstack.rs:2175` → returns `Vec<DeviceListDetail>` for a given `application_id`.

C-1 promotes them to first-class production code (drops `#[allow(dead_code)]`) and ensures their return types are JSON-serialisable for the web layer.

There is **no equivalent helper for recent uplinks** today. The ChirpStack proto exposes uplinks via a streaming RPC, not a one-shot list:

- `InternalService.StreamDeviceEvents(StreamDeviceEventsRequest) → stream LogItem` at `proto/chirpstack/api/internal.proto:64`.

`LogItem` is `{ id, time, description, body, properties }` where `description` discriminates event type (`"up"` for uplinks among others) and `body` is the JSON-serialised event payload. C-1 must write a NEW helper that opens the stream, reads events for a bounded time window or until N uplinks have been collected, closes the stream, and returns the collected uplinks.

### What "name-translation gateway" means for C-1

Memory `[[project_epic_c_auto_discovery_vision]]` pins the load-bearing invariant: operator-facing surfaces must always show **names**, never UUIDs. C-1's endpoints carry both — the UUID/DevEUI for opcgw's internal lookup keying, and the human-readable name for the picker to render. The schema MUST keep both, but the picker UI (C-2) will render only the name; the UUID is hidden form state.

### Web router + existing endpoint conventions

opcgw's web layer lives in `src/web/`:
- `src/web/mod.rs` (~989 lines) — router construction.
- `src/web/api.rs` (~6126 lines) — JSON-returning API endpoints. Existing pattern: each endpoint is a function with extractors + state, returning `Json<T>` or `(StatusCode, Json<ErrorBody>)`.
- `src/web/auth.rs` (~769 lines) — basic auth + middleware.
- `src/web/csrf.rs` (~749 lines) — CSRF token discipline. Existing rule: only mutation endpoints (POST/PUT/DELETE) enforce CSRF; GET endpoints are read-only and CSRF-exempt.

C-1's three endpoints are GET-only / read-only. They follow the same basic-auth gate as the rest of the API surface but do NOT require CSRF tokens (consistent with existing GET /api/* endpoints — verify against `src/web/api.rs` patterns; Dev Notes documents the auth contract).

---

## Acceptance Criteria

### Endpoint surface (GET only, read-only, basic-auth gated)

1. **`GET /api/inventory/applications` returns the tenant's applications.**
   - Response body: `{ "items": [ { "id": "<uuid>", "name": "<human>", "description": "<optional>" } ], "count": <N>, "cache_status": "<hit|miss|refresh|bypassed>", "fetched_at": "<RFC3339 timestamp of the latest ChirpStack fetch backing this response>" }`.
   - Items sorted by `name` ASCENDING, case-insensitive locale-independent (use `.to_lowercase()` for the sort key — operator may see "Arrosage" and "Bâtiments" mixed).
   - `count` is the length of `items` (the full set; ChirpStack pagination is unrolled inside the helper, not at the API).
   - Empty list returns `200 OK` with `items: []` and `count: 0` — NOT a 404 (zero applications is a valid state, especially fresh post-C-0 gateways).
   - HTTP basic auth required (same gate as `/api/applications`); 401 on missing/invalid credentials.

2. **`GET /api/inventory/devices?application_id=<uuid>` returns devices under that application.**
   - Required query parameter: `application_id`. Missing → `400 Bad Request` with `{ "error": "missing_query_param", "param": "application_id" }`.
   - Response body: `{ "items": [ { "dev_eui": "<16-hex>", "name": "<human>", "description": "<optional>", "device_profile_name": "<optional>", "last_seen_at": "<RFC3339 or null>" } ], "count": <N>, "cache_status": "<…>", "fetched_at": "<…>", "application_id": "<echoed>" }`.
   - Items sorted by `name` ASCENDING case-insensitive.
   - Unknown `application_id` (one ChirpStack doesn't recognise) → `200 OK` with `items: []`, `count: 0`, AND the audit event fires with `chirpstack_response="empty"` so an operator wondering why their picker is empty can grep the log. (Not 404 because semantically the application "has" zero devices.)
   - Same basic-auth gate as AC#1.

3. **`GET /api/inventory/uplinks?dev_eui=<16-hex>&limit=<N>` returns recent uplinks + observed-keys aggregate.**
   - Required query parameter: `dev_eui` (16 hex chars, case-insensitive, accept with or without colons/dashes — normalise server-side). Invalid format → `400 Bad Request`.
   - Optional `limit` query parameter, default `10`, capped at `50`. Out-of-range → `400 Bad Request` with the cap value documented in the error response.
   - Response body: `{ "items": [ { "received_at": "<RFC3339>", "decoded_object": <JSON-object-as-emitted-by-codec>, "f_port": <int or null>, "f_cnt": <int or null> } ], "count": <N>, "observed_keys": [ { "key": "<top-level-key>", "wire_type": "Float|Int|Bool|String", "sample_value": <example value> } ], "dev_eui": "<echoed normalised>", "fetched_at": "<…>" }`.
   - Items sorted by `received_at` DESCENDING (newest first).
   - `observed_keys` is the union of all top-level keys across `decoded_object` payloads, with `wire_type` inferred by observation (see AC#4) and `sample_value` set to the first non-null observed value for that key.
   - If ChirpStack's event stream emits zero uplinks within the bounded read window → `200 OK` with `items: []`, `count: 0`, `observed_keys: []`. The web UI uses this signal to surface "No recent uplinks for this device — type metric names manually or wait for the device to send" (handled in C-2; C-1 just returns the empty payload).
   - Same basic-auth gate as AC#1.

4. **Wire-type inference for `observed_keys` per uplink-payload value.**
   - For each top-level key in `decoded_object`, inspect the values observed across the N uplinks:
     - All values are JSON booleans → `wire_type = "Bool"`.
     - All values are JSON numbers AND every number is a mathematical integer (i.e., `value.fract() == 0`) AND fits in `i64` → `wire_type = "Int"`.
     - All values are JSON numbers but at least one is fractional OR out-of-range for `i64` → `wire_type = "Float"`.
     - All values are JSON strings → `wire_type = "String"`.
     - Heterogeneous (mix of types across uplinks) → `wire_type = "String"` AND emit `tracing::warn!(event = "inventory_observed_key_heterogeneous", key = …, types_seen = …)`. The operator can override the wire type in the picker UI (C-2 territory).
     - JSON `null` values are skipped (don't count toward the inference); a key that is `null` in every observed uplink defaults to `wire_type = "String"`.
   - Inference is deterministic and reproducible — the same N uplinks produce the same `observed_keys` array regardless of evaluation order.

### Server-side TTL cache

5. **Cache scope and keying.**
   - `/api/inventory/applications` cached with key `(tenant_id)`.
   - `/api/inventory/devices` cached with key `(tenant_id, application_id)`.
   - `/api/inventory/uplinks` **NOT cached** — freshness-sensitive. Each request hits ChirpStack.
   - `tenant_id` comes from `config.chirpstack.tenant_id` (constant for a running gateway; just included in the key for forward-compatibility with future multi-tenant support).
   - Cache lives in a new `src/chirpstack_inventory_cache.rs` module (or merged into `src/chirpstack.rs` if Dev Agent judges file-size allows). Either way: NOT in `src/web/` (the cache is ChirpStack-source-of-truth-side infrastructure, not web-layer state).

6. **Default TTL = 60 seconds.**
   - New config field: `[chirpstack].inventory_cache_ttl_seconds: u64`, default `60`. Settable from `OPCGW_CHIRPSTACK__INVENTORY_CACHE_TTL_SECONDS` env-var.
   - TTL = 0 → cache disabled (every request hits ChirpStack). Useful for development / debugging.
   - TTL upper bound: documented in config struct rustdoc, no hard cap in code. The validator rejects negative values (handled by `u64` typing).

7. **Cache hit vs miss vs refresh logic.**
   - On request, compute the cache key.
   - If the key has an entry AND `(now - entry.fetched_at) < ttl` AND `?refresh=true` is NOT set → return cached payload, mark `cache_status = "hit"`.
   - Otherwise: call ChirpStack (helper method on `ChirpstackPoller` or a sibling client), insert/overwrite the entry with `fetched_at = now`, mark `cache_status = "miss"` (no prior entry or stale) or `"refresh"` (`?refresh=true` was set) or `"bypassed"` (TTL = 0).
   - Race-condition guard: two concurrent requests with the same key + an expired entry should result in ONE ChirpStack call, not two. Use `tokio::sync::Mutex` or `std::sync::Mutex` (Dev Agent's choice) to serialise the fetch-and-insert critical section. The non-fetching task awaits the result of the first fetch.

8. **`?refresh=true` query parameter forces a cache miss.**
   - Available on `/api/inventory/applications` and `/api/inventory/devices` (not on `/uplinks` — already uncached).
   - Forces an immediate ChirpStack call, repopulates the cache, returns `cache_status = "refresh"`.
   - C-4 (drift view) uses `?refresh=true` so the diff is never against stale cache. C-2 (pickers) does NOT use `?refresh=true` by default — operator clicks should benefit from cache hits.
   - Invalid `?refresh` value (e.g., `?refresh=foo`) → treat as not-set, do NOT 400. Operator-friendly tolerance.

9. **Cache invalidation on CRUD writes.**
   - When `/api/applications` POST/PUT/DELETE succeeds → invalidate the `(tenant_id)` cache entry (next applications query forces fresh).
   - When `/api/devices` POST/PUT/DELETE succeeds → invalidate the `(tenant_id, application_id)` cache entry for the affected application (next devices query for that app forces fresh).
   - Invalidation is performed by removing the entry, NOT by setting TTL to 0 — a subsequent in-flight read with a stale clone of the entry could still race; removal makes the next read a clean miss.
   - The CRUD handlers in `src/web/api.rs` get a small touch to call `inventory_cache.invalidate_applications()` / `invalidate_devices(application_id)` after their existing success paths.

### Audit events

10. **`event="inventory_query"` fires on cache MISSES only** (and refreshes and bypassed reads).
    - Fields: `resource={applications|devices|uplinks}`, `cache_status={miss|refresh|bypassed}`, `tenant_id=<…>`, `application_id=<… when applicable>`, `dev_eui=<… when applicable>`, `response_status=<HTTP code>`, `chirpstack_response={ok|empty|error}`, `item_count=<N for ok|0 for empty|null for error>`, `duration_ms=<float>`.
    - Cache HITS DO NOT emit any audit event (silent). This is the load-bearing invariant from the scope decision: audit log volume bounded by `1 / TTL × active_sessions`, not by clicks-per-session.

11. **`event="inventory_query_failed"` fires on errors.**
    - ChirpStack unreachable → `reason="chirpstack_unreachable"`, HTTP 502.
    - ChirpStack authentication failure → `reason="chirpstack_auth_failed"`, HTTP 502.
    - ChirpStack returns a gRPC error (other than auth) → `reason="chirpstack_grpc_error"`, `grpc_code=<int>`, HTTP 502.
    - Uplink stream timeout (no events within the bounded read window AND `limit > 0` requested) is NOT an error — return `200 OK` with `items: []`. Logged at debug level only.

12. **`event="inventory_cache_invalidated"` fires on CRUD-driven invalidations.**
    - Fields: `cache_scope={applications|devices}`, `triggered_by={crud_post|crud_put|crud_delete}`, `application_id=<… for devices scope>`.
    - Helps operators correlate "I added an app, then the picker showed it" vs "stale cache for 60 s."

13. **`event="inventory_observed_key_heterogeneous"` fires on wire-type inference fallback** (per AC#4).
    - Fields: `dev_eui=<…>`, `key=<top-level key>`, `types_seen=<comma-separated JSON types, e.g., "int,string">`.

### ChirpStack inventory helpers (Rust API surface)

14. **`get_applications_list_from_server` is promoted to first-class production code.**
    - Drop the `#[allow(dead_code)]` attribute at `src/chirpstack.rs:2134`.
    - Return type stays `Vec<ApplicationDetail>` — but the inventory layer maps to a leaner `InventoryApplication { id, name, description }` shape before caching (the full `ApplicationDetail` carries fields that are irrelevant to the picker).
    - Add a `#[test]` covering: (a) happy path returns sorted list, (b) ChirpStack auth failure surfaces as `OpcGwError::ChirpStack`, (c) empty list is `Ok(vec![])` not an error.

15. **`get_devices_list_from_server` similarly promoted.**
    - Drop `#[allow(dead_code)]` at `src/chirpstack.rs:2174`.
    - Maps to `InventoryDevice { dev_eui, name, description, device_profile_name, last_seen_at }`.
    - Same test coverage as AC#14.

16. **New helper `stream_recent_device_uplinks(dev_eui, limit, max_wait)`** in `src/chirpstack.rs` (or a new sub-module).
    - Opens `InternalService.StreamDeviceEvents` for the given DevEUI.
    - Reads stream items in a `tokio::time::timeout(max_wait, …)` loop. Default `max_wait = Duration::from_secs(5)` (configurable via `[chirpstack].inventory_uplink_max_wait_seconds`, default 5, range 1..=60).
    - Filters `LogItem.description == "up"` (or the exact discriminator emitted by ChirpStack — Dev Agent verifies during implementation; reject anything else from the inventory result).
    - Parses `LogItem.body` as JSON; extracts the codec output `object` field (the decoded payload).
    - Stops on whichever comes first: `limit` uplinks collected, OR `max_wait` elapsed, OR stream closed by ChirpStack.
    - Returns `Vec<InventoryUplink { received_at, decoded_object, f_port, f_cnt }>` sorted by `received_at` descending.
    - On timeout with zero uplinks collected: returns `Ok(vec![])` (the "no recent uplinks" state — NOT an error).
    - Unit-test coverage with a stub `StreamDeviceEvents` server (use the same testing pattern as existing ChirpStack-mocking tests if any exist; otherwise add a small `mockall` or hand-rolled stub).

### Web endpoint handlers + integration

17. **New module `src/web/inventory.rs`** (or extended `src/web/api.rs` if Dev Agent prefers — note `api.rs` is already 6126 lines). Contains the three handler functions and the `InventoryRouter` constructor.

18. **Router wiring in `src/web/mod.rs`.**
    - `/api/inventory/applications` → handler `inventory_applications`.
    - `/api/inventory/devices` → handler `inventory_devices`.
    - `/api/inventory/uplinks` → handler `inventory_uplinks`.
    - All routed under the same basic-auth middleware stack as the rest of `/api/*`.
    - All exempt from CSRF (GET-only, read-only — matches existing API GET convention).

19. **Integration tests** in a new `tests/web_inventory.rs` (or extending existing test infrastructure).
    - At minimum 12 tests covering:
      - `applications` cache miss → ChirpStack called once, response includes cache_status="miss", subsequent hit returns cached + cache_status="hit" + no ChirpStack call.
      - `applications` ?refresh=true → forces ChirpStack call, response cache_status="refresh".
      - `applications` empty list → 200 + items=[].
      - `applications` ChirpStack unreachable → 502 + audit event with reason="chirpstack_unreachable".
      - `devices` missing application_id → 400.
      - `devices` cache scope is per-(tenant, app) — two different application_ids do NOT share cache.
      - `uplinks` happy path with 3 mock uplinks → response items=[3], observed_keys reflect inferred wire types.
      - `uplinks` zero uplinks within max_wait → 200 + items=[] + observed_keys=[].
      - `uplinks` heterogeneous-type key → wire_type="String" + audit event "inventory_observed_key_heterogeneous".
      - `uplinks` limit=0 → returns immediately with items=[] (degenerate but legal).
      - `uplinks` limit=51 → 400 (cap is 50).
      - cache invalidation: POST /api/applications → next GET /api/inventory/applications is cache_status="miss" even within TTL.

### Test count baseline + regression invariants

20. **`cargo test --all-targets` passes.** Pre-C-1 baseline post-C-0 will need to be re-measured (depends on C-0's actual delta). For planning purposes, assume C-0 lands +9, baseline becomes ≥ 1269/0/≥10. C-1 target: ≥ 1281 / 0 / ≥ 10 — the +12 from the integration tests in AC#19. Document the actual delta in Dev Notes.

21. **`cargo clippy --all-targets -- -D warnings` clean.**

22. **`cargo test --doc` no regressions.** ≥ 56 ignored, 0 failed.

23. **Strict-zero file invariants.** NO changes to: `migrations/*.sql`, `Cargo.lock` (Cargo.toml MAY change if Dev Agent finds an existing dependency suffices for the cache; only add new deps as a documented Dev Notes decision), `src/main.rs::initialise_tracing`, `src/opc_ua_auth.rs`, `src/opc_ua_history.rs`, `src/storage/*`. Mutable scope:
    - `src/chirpstack.rs` (drop dead_code, add `stream_recent_device_uplinks`, possibly extract shared types)
    - `src/chirpstack_inventory_cache.rs` (NEW) OR `src/chirpstack.rs` extension
    - `src/web/inventory.rs` (NEW) OR `src/web/api.rs` extension
    - `src/web/mod.rs` (router wiring)
    - `src/web/api.rs` (cache invalidation hooks on CRUD success paths)
    - `src/config.rs` (new `inventory_cache_ttl_seconds` + `inventory_uplink_max_wait_seconds` fields, validators, env-var bindings)
    - `tests/web_inventory.rs` (NEW)
    - `docs/web-api.md` (or `README.md` API section if no `web-api.md` exists yet)
    - `docs/logging.md` (3 new audit events)
    - `_bmad-output/implementation-artifacts/sprint-status.yaml` (story status flips)
    - This story spec file

### Documentation sync

24. **`docs/web-api.md` or new `docs/inventory-api.md`** documents the three endpoints with request/response schemas, the cache contract, the `?refresh=true` semantics, and the audit-event mapping.

25. **`docs/logging.md`** gains entries for the three new audit events: `inventory_query`, `inventory_query_failed`, `inventory_cache_invalidated`, `inventory_observed_key_heterogeneous`. Each entry follows the existing taxonomy format (event name, fields list, emission point).

26. **`README.md` Planning table** has the Epic C row updated post-C-1 landing ("Epic C 2/6 done"). The Configuration section gains a brief note about `[chirpstack].inventory_cache_ttl_seconds` and `inventory_uplink_max_wait_seconds`.

27. **DocBook user manual `docs/manual/opcgw-user-manual.xml`** — C-1 itself is mostly invisible to operators (it's the substrate for C-2). Skip the manual update for C-1; C-2 will document the picker UX (which naturally surfaces the API contract from the operator's POV).

### GitHub tracking issue

28. GitHub tracking issue (suggested title: "C-1: ChirpStack inventory query layer + server-side cache") opened by user out-of-band. Issue number captured in Dev Notes; referenced in every commit via `Refs #N`.

---

## Tasks / Subtasks

- [ ] **Task 0 — Tracking issue acknowledgment (AC: #28)**
  - [ ] 0.1 User opens GitHub issue with the suggested title.
  - [ ] 0.2 Capture the issue number in Dev Notes.
  - [ ] 0.3 Reference `Refs #N` in every commit.

- [ ] **Task 1 — Promote existing ChirpStack helpers + define inventory types (AC: #14, #15)**
  - [ ] 1.1 Drop `#[allow(dead_code)]` from `get_applications_list_from_server` and `get_devices_list_from_server`.
  - [ ] 1.2 Define `InventoryApplication`, `InventoryDevice`, `InventoryUplink` structs (Dev Notes documents whether they live in `src/chirpstack.rs` or a new sub-module).
  - [ ] 1.3 Implement `From<ApplicationDetail>`-style conversions for the leaner inventory types.
  - [ ] 1.4 Add unit tests covering happy path, auth failure, empty list.

- [ ] **Task 2 — New `stream_recent_device_uplinks` helper (AC: #16)**
  - [ ] 2.1 Examine the proto-generated `StreamDeviceEvents` API; confirm the streaming contract.
  - [ ] 2.2 Implement the helper with bounded read window.
  - [ ] 2.3 Add stub-server-driven unit tests covering happy path, timeout-with-zero-uplinks, timeout-mid-stream, stream-closed-early.
  - [ ] 2.4 Document the discriminator string ("up" vs whatever ChirpStack actually emits) in Dev Notes.

- [ ] **Task 3 — Inventory cache module (AC: #5, #6, #7, #8, #9)**
  - [ ] 3.1 Create `src/chirpstack_inventory_cache.rs` (or merge into `src/chirpstack.rs` per Dev Notes decision).
  - [ ] 3.2 Cache struct: `InventoryCache { applications: Mutex<HashMap<TenantId, CacheEntry<Vec<InventoryApplication>>>>, devices: Mutex<HashMap<(TenantId, AppId), CacheEntry<Vec<InventoryDevice>>>>, ttl: Duration }`.
  - [ ] 3.3 `CacheEntry<T> { value: T, fetched_at: Instant }`. TTL check uses `Instant::now() - fetched_at < ttl`.
  - [ ] 3.4 Public methods: `get_or_fetch_applications<F>(&self, fetch: F)`, `get_or_fetch_devices(...)`, `invalidate_applications(...)`, `invalidate_devices(application_id)`.
  - [ ] 3.5 Race-condition guard: serialise fetch-and-insert critical section.
  - [ ] 3.6 Config fields: `[chirpstack].inventory_cache_ttl_seconds` (default 60), `[chirpstack].inventory_uplink_max_wait_seconds` (default 5, range 1..=60). Add to `src/config.rs` + validators + env-var binding tests.
  - [ ] 3.7 Unit tests: cache miss → fetch called once + value cached; cache hit → fetch not called; TTL expiration → fetch called again; race-condition test with two concurrent requests on the same expired key → fetch called once total.

- [ ] **Task 4 — Web inventory handlers (AC: #1, #2, #3, #17, #18)**
  - [ ] 4.1 Create `src/web/inventory.rs` (or extend `src/web/api.rs`).
  - [ ] 4.2 `inventory_applications` handler: parse `?refresh=true`, call `inventory_cache.get_or_fetch_applications(|| poller.get_applications_list_from_server())`, map to JSON.
  - [ ] 4.3 `inventory_devices` handler: parse `application_id` (required) + `?refresh=true`, call `inventory_cache.get_or_fetch_devices(...)`, map to JSON.
  - [ ] 4.4 `inventory_uplinks` handler: parse `dev_eui` (required, validate + normalise) + `?limit` (default 10, cap 50), call `stream_recent_device_uplinks(...)`, compute `observed_keys` per AC#4, map to JSON.
  - [ ] 4.5 Router wiring in `src/web/mod.rs`.
  - [ ] 4.6 Auth gate: same basic-auth middleware as the rest of `/api/*`. CSRF exemption documented.

- [ ] **Task 5 — Wire-type inference (AC: #4)**
  - [ ] 5.1 Implement `infer_wire_type(values: &[&serde_json::Value]) -> (WireType, Option<serde_json::Value>)` returning the inferred type + sample value.
  - [ ] 5.2 Implement `compute_observed_keys(uplinks: &[InventoryUplink]) -> Vec<ObservedKey>` aggregating across uplinks.
  - [ ] 5.3 Unit tests: all-int → Int, all-fractional → Float, all-bool → Bool, all-string → String, heterogeneous → String + warn event, all-null → String, int-then-fractional → Float, large-int-overflowing-i64 → Float.

- [ ] **Task 6 — Audit events (AC: #10, #11, #12, #13)**
  - [ ] 6.1 Emit `event="inventory_query"` from each handler on cache miss/refresh/bypassed paths (NOT on hits).
  - [ ] 6.2 Emit `event="inventory_query_failed"` from ChirpStack-error paths with the appropriate `reason=`.
  - [ ] 6.3 Emit `event="inventory_cache_invalidated"` from the CRUD-side invalidation hooks (Task 7).
  - [ ] 6.4 Emit `event="inventory_observed_key_heterogeneous"` from the wire-type inference fallback path.

- [ ] **Task 7 — Cache invalidation hooks in CRUD handlers (AC: #9)**
  - [ ] 7.1 Find the success paths in `src/web/api.rs` for POST /api/applications, PUT /api/applications/<id>, DELETE /api/applications/<id>; call `inventory_cache.invalidate_applications(tenant_id)` after the existing audit-event emit.
  - [ ] 7.2 Same for /api/devices CRUD; call `inventory_cache.invalidate_devices((tenant_id, application_id))`.
  - [ ] 7.3 Verify with an integration test that the next inventory query after a CRUD write is `cache_status="miss"` even within TTL.

- [ ] **Task 8 — Integration tests (AC: #19)**
  - [ ] 8.1 Create `tests/web_inventory.rs` with the test harness (reuse existing `tests/web_*.rs` patterns).
  - [ ] 8.2 Implement the 12 named tests from AC#19.
  - [ ] 8.3 Use mock ChirpStack: either extend the existing test infrastructure if one exists, or stub at the helper level (mock the closure passed to `get_or_fetch_*`).

- [ ] **Task 9 — Documentation sync (AC: #24, #25, #26, #27)**
  - [ ] 9.1 Create or extend `docs/web-api.md` / `docs/inventory-api.md` with the three endpoint schemas + cache contract.
  - [ ] 9.2 Update `docs/logging.md` with the 4 new audit events.
  - [ ] 9.3 Update `README.md` Planning table + Configuration section.
  - [ ] 9.4 NO update to the DocBook manual (C-2 will cover the operator-facing surface).

- [ ] **Task 10 — Regression gate + commit (AC: #20, #21, #22, #23)**
  - [ ] 10.1 `cargo test --all-targets` → record actual count; target ≥ 1281/0/≥10 (assuming C-0 delta).
  - [ ] 10.2 `cargo clippy --all-targets -- -D warnings` → clean.
  - [ ] 10.3 `cargo test --doc` → no regressions.
  - [ ] 10.4 Manual smoke test: spin up gateway against Guy's real ChirpStack at 192.168.1.12:8080 (per the .env credentials used in B-1's e2e); curl the three inventory endpoints; verify cache_status transitions correctly.
  - [ ] 10.5 Commit message: `Story C-1: ChirpStack inventory query layer + cache - Implementation Complete` + `Refs #<issue>`.

---

## Dev Notes

### Why cache MISS-only audit events

The scope-time decision (Guy, 2026-05-21) was to "avoid too much ChirpStack hit" and to keep audit-log volume bounded. The straightforward path — fire one `inventory_query` audit event per HTTP request — produces 20+ events per picker session per operator, even when the actual ChirpStack call count is 1 (cache hits). That floods the audit log with non-actionable noise.

By firing audit events ONLY on cache misses (i.e., ONLY when opcgw actually called ChirpStack), the event count maps directly to outbound ChirpStack traffic. An operator wondering "how often is opcgw hitting ChirpStack?" greps `event="inventory_query"` and gets an exact answer. The log volume is also `O(1/TTL)` rather than `O(operator_clicks)`.

The HTTP response still carries `cache_status` so the web UI / debug tooling can see hit/miss without needing the audit log.

### Why a custom mutex-guarded cache and not `moka` / `cached`

opcgw already manages a small set of crates carefully (`Cargo.toml` review on each story per the strict-zero invariant). Adding a cache crate just for this would: (a) bring transitive deps, (b) couple the security review surface to upstream crate hygiene, (c) tie us to a crate's eviction model. A custom 50-line cache with TTL check on access is:

- Trivially auditable.
- Memory-bounded by the configured tenant_id + application_id set (a few hundred entries max, ever).
- Race-condition-safe with a single `Mutex` per scope.

If a future story finds eviction policy lacking (e.g., per-entry TTL, LRU), revisit then. For C-1, simplest wins.

### Why no cache on `/uplinks`

Uplinks are the freshness-sensitive surface. When an operator opens "Add metric" for a device, they want to see the latest decoded keys — possibly to verify "did the firmware update land?" or "is this device sending the new field yet?" Caching uplinks would defeat the purpose. The 5-second `inventory_uplink_max_wait_seconds` bound already serves as the natural rate limiter on uplink queries.

### Wire-type inference subtleties

- `i64` overflow case: ChirpStack codecs sometimes emit large counter values (e.g., `4366700000` energy ticks). `2^63 - 1 ≈ 9.22 × 10^18` is the i64 ceiling. Values exceeding this fall through to Float. The picker UI (C-2) will let the operator override anyway.
- The "all-null" key case is rare but real (e.g., a codec emits `"voltage": null` when the sensor is in low-power mode). Defaulting to `String` here is conservative — the operator can override.
- The `f64.fract() == 0` check for "is integer" works correctly for IEEE 754 doubles up to ~2^53, beyond which all representable values are integers anyway. Good enough for LoRaWAN payload magnitudes.

### ChirpStack auth on `InternalService.StreamDeviceEvents`

The `InternalService` proto may carry different auth semantics than `ApplicationService` / `DeviceService`. ChirpStack v4.x typically requires the same bearer token for `Internal*` endpoints, but the route may be off by default in some deployments. Dev Agent should verify against Guy's real ChirpStack at 192.168.1.12:8080 during smoke testing; if `StreamDeviceEvents` returns `PERMISSION_DENIED`, document the workaround (likely "enable Internal API on the ChirpStack side" — an operator-side configuration concern, not a gateway-side bug).

### Cache invalidation race window

There's a small race window: an in-flight inventory query (already past the cache lookup, currently calling ChirpStack) completes AFTER a CRUD write invalidates the cache, then writes its stale result back into the cache. Subsequent reads see the stale write. The remediation is to make `get_or_fetch_*` re-check freshness after the fetch closes, OR to make CRUD invalidation hold a write-lock that prevents concurrent fetch-and-insert. Either solution adds complexity; for C-1, accept the race (window is ≤ one ChirpStack round-trip, typically < 200 ms in a healthy deployment) and document the limitation here. A future story can tighten if drift-view false-positives become a complaint.

### Test count baseline projection

C-1's baseline floor is whatever C-0 lands at. If C-0 hits its +9 target (final 1269/0/≥10), C-1 targets ≥ 1281/0/≥10 (+12 from inventory integration tests). If C-0 lands +N ≠ 9, adjust the C-1 baseline mention in AC#20 at story-start time.

### Carry-forward GitHub issues

#88 (per-IP rate limiting — inventory endpoints would benefit but out of C-1 scope), #100 (doctest ignores baseline), #102 (tests/common reuse), #104 (TLS hardening), #110 (RunHandles Drop), #117 (perf-CI lane).

---

## Out of Scope

- **Inventory pickers in the web UI** — C-2's deliverable. C-1 exposes the API; C-2 wires it into `static/applications.html` / `static/devices-config.html`.
- **Drift view** — C-4's deliverable. C-1 provides the API contract that C-4 consumes.
- **Server-Sent Events / WebSocket push** — out of scope. Inventory queries are operator-triggered, not push-driven.
- **Multi-tenant support** — out of scope. `tenant_id` is keyed for forward-compat but only one tenant exists per running gateway in v2.x.
- **Per-IP rate limiting on inventory endpoints** — tracked at #88; out of C-1 scope.
- **Caching the `/uplinks` endpoint** — intentionally NOT done; see Dev Notes.
- **Persistence of cache across restarts** — out of scope. Cache is process-memory-only; TTL is at most 60 s, so a restart's worst-case cost is one ChirpStack call per scope post-restart.

---

## Completion Note

To be filled in by the dev agent at story completion. Should include: actual test count delta, the `InternalService` auth verification result, the discriminator string ChirpStack actually emits (`"up"` vs alternatives), any deferred follow-ups added to `deferred-work.md`, smoke-test results against Guy's real ChirpStack.
